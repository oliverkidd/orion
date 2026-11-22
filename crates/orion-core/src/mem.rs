//! Small memory probes shared by the daemon and the TUI client.
//!
//! On macOS they ask the kernel directly — `libproc` for a process's
//! resident size and the process table, `sysctlbyname` for the machine's
//! memory — where they used to spawn `ps` and `sysctl`: a spawned probe
//! costs a fork, an exec and 2–50 ms of CPU each time, and the footer's
//! readout asked every few seconds. Elsewhere (Linux CI) they fall back to
//! those commands. Either way none of them belongs on an event loop's hot
//! path: the daemon runs its sweep on the blocking pool.

/// One process as the process-table probe reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    /// Resident set size, bytes.
    pub rss_bytes: u64,
}

/// Resident set size of one process, bytes.
pub fn process_rss_bytes(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        macos::task_all_info(pid).map(|info| info.ptinfo.pti_resident_size)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout).ok()?;
        text.trim().parse::<u64>().ok().map(|kb| kb * 1024)
    }
}

/// Physical memory installed on this machine, bytes. It cannot change
/// while the process runs, so it is read once and remembered.
pub fn system_total_bytes() -> Option<u64> {
    static TOTAL: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *TOTAL.get_or_init(read_system_total_bytes)
}

#[cfg(target_os = "macos")]
fn read_system_total_bytes() -> Option<u64> {
    macos::sysctl_u64(c"hw.memsize")
}

#[cfg(not(target_os = "macos"))]
fn read_system_total_bytes() -> Option<u64> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}

/// Every process this user may inspect, with its parent and resident
/// size: what the metrics reading sums session subtrees over. A process
/// owned by someone else (root's daemons) is left out on macOS — the
/// kernel won't say its size — which costs nothing, since no session's
/// subtree holds one. None when the table can't be read at all.
pub fn process_table() -> Option<Vec<ProcEntry>> {
    #[cfg(target_os = "macos")]
    {
        let pids = macos::all_pids()?;
        Some(
            pids.into_iter()
                .filter_map(|pid| {
                    let info = macos::task_all_info(pid)?;
                    Some(ProcEntry {
                        pid,
                        ppid: info.pbsd.pbi_ppid,
                        rss_bytes: info.ptinfo.pti_resident_size,
                    })
                })
                .collect(),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let out = std::process::Command::new("ps")
            .args(["-axo", "pid=,ppid=,rss="])
            .output()
            .ok()?;
        Some(parse_ps_rss_table(&String::from_utf8(out.stdout).ok()?))
    }
}

/// `ps -axo pid=,ppid=,rss=` output (rss in KB) as entries; lines that
/// don't read as three numbers are skipped.
pub fn parse_ps_rss_table(table: &str) -> Vec<ProcEntry> {
    table
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace().map(|c| c.parse::<u64>().ok());
            let pid = u32::try_from(cols.next()??).ok()?;
            let ppid = u32::try_from(cols.next()??).ok()?;
            let kb = cols.next()??;
            Some(ProcEntry {
                pid,
                ppid,
                rss_bytes: kb * 1024,
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::CStr;
    use std::mem::{size_of, MaybeUninit};

    /// Slack on top of the count the kernel gave: processes born between
    /// asking how many there are and listing them still fit.
    const PID_SLACK: usize = 64;

    /// Every pid on the machine, from `proc_listallpids`.
    pub(super) fn all_pids() -> Option<Vec<u32>> {
        // SAFETY: a null buffer asks only for the count.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let count = usize::try_from(count).ok()?;
        let mut pids: Vec<libc::c_int> = vec![0; count + PID_SLACK];
        let bytes = libc::c_int::try_from(pids.len() * size_of::<libc::c_int>()).ok()?;
        // SAFETY: the buffer is `bytes` long and lives across the call.
        let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        let listed = usize::try_from(listed).ok()?.min(pids.len());
        pids.truncate(listed);
        Some(
            pids.into_iter()
                .filter_map(|pid| u32::try_from(pid).ok())
                .filter(|&pid| pid > 0)
                .collect(),
        )
    }

    /// `PROC_PIDTASKALLINFO` for `pid`: its BSD info (parent, group) and
    /// its task info (resident size) in one call. None for a process gone,
    /// or one this user may not inspect.
    pub(super) fn task_all_info(pid: u32) -> Option<libc::proc_taskallinfo> {
        let pid = libc::c_int::try_from(pid).ok()?;
        let size = libc::c_int::try_from(size_of::<libc::proc_taskallinfo>()).ok()?;
        let mut info = MaybeUninit::<libc::proc_taskallinfo>::zeroed();
        // SAFETY: the buffer is exactly one `proc_taskallinfo`, and the
        // kernel writes at most `size` bytes into it.
        let wrote = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTASKALLINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        // SAFETY: a full-size answer filled the struct; zeroed before.
        (wrote == size).then(|| unsafe { info.assume_init() })
    }

    /// A 64-bit `sysctl` by name.
    pub(super) fn sysctl_u64(name: &CStr) -> Option<u64> {
        let mut value: u64 = 0;
        let mut len = size_of::<u64>();
        // SAFETY: `value` is `len` bytes; no new value is set.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut u64).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && len == size_of::<u64>()).then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_size_and_the_machine_has_memory() {
        let rss = process_rss_bytes(std::process::id()).expect("own rss");
        assert!(rss > 1024 * 1024, "at least a MB resident: {rss}");
        let total = system_total_bytes().expect("memsize");
        assert!(total > rss);
        assert_eq!(system_total_bytes(), Some(total), "remembered");
    }

    #[test]
    fn the_table_holds_this_process_under_its_parent() {
        let table = process_table().expect("table");
        let me = table
            .iter()
            .find(|e| e.pid == std::process::id())
            .expect("this process is listed");
        assert_eq!(me.ppid, std::os::unix::process::parent_id());
        assert!(me.rss_bytes > 0);
    }

    #[test]
    fn a_ps_table_parses_and_skips_garbage() {
        let rows = parse_ps_rss_table(" 10 1 100\nnot numbers\n 20 10 3\n");
        assert_eq!(
            rows,
            vec![
                ProcEntry {
                    pid: 10,
                    ppid: 1,
                    rss_bytes: 100 * 1024
                },
                ProcEntry {
                    pid: 20,
                    ppid: 10,
                    rss_bytes: 3 * 1024
                },
            ]
        );
    }
}
