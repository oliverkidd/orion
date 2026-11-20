//! LISTENERS: every process on the machine listening on a TCP port, with
//! the directory it runs in — how STACK STATUS knows an app is serving,
//! whoever started it (`stacks`). Two `lsof` calls a poll: the listening
//! sockets, then the working directories of the processes behind them.
//! Docker's published ports belong to the engine's process, whose
//! directory is no checkout's, so a container's port never counts here:
//! containers are read from docker itself.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// How long one `lsof` or `ps` may take: a wedged one must not stall the
/// poll.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// One listening socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub pid: u32,
    /// Its parent's pid (0 when lsof didn't say).
    pub ppid: u32,
    pub port: u16,
    /// The process's working directory.
    pub cwd: PathBuf,
}

/// Every TCP listener on the machine with its process's directory; None
/// when `lsof` couldn't be asked — unknown, never "nothing listens".
pub async fn scan() -> Option<Vec<Listener>> {
    let sockets = lsof(&["-nP", "-iTCP", "-sTCP:LISTEN", "-FpRn"]).await?;
    let ports = parse_sockets(&sockets);
    if ports.is_empty() {
        return Some(Vec::new());
    }
    let pids = ports
        .keys()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // A process gone between the two calls is simply missing here.
    let cwds = lsof(&["-a", "-d", "cwd", "-p", &pids, "-Fpn"])
        .await
        .map(|out| parse_cwds(&out))
        .unwrap_or_default();
    Some(
        ports
            .into_iter()
            .filter_map(|(pid, (ppid, ports))| Some((pid, ppid, ports, cwds.get(&pid)?.clone())))
            .flat_map(|(pid, ppid, ports, cwd)| {
                ports.into_iter().map(move |port| Listener {
                    pid,
                    ppid,
                    port,
                    cwd: cwd.clone(),
                })
            })
            .collect(),
    )
}

/// Run `lsof args`: its stdout, or None when it can't be run or overran.
/// lsof exits 1 when it found nothing to list, which is an answer.
async fn lsof(args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(lsof_path());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    // A partial listing (a socket it couldn't read) still lists.
    let answered = out.status.success() || out.status.code() == Some(1) || !out.stdout.is_empty();
    answered.then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// lsof where macOS keeps it, else whatever PATH finds: the DAEMON runs
/// under launchd's thin PATH.
fn lsof_path() -> &'static str {
    if std::path::Path::new("/usr/sbin/lsof").is_file() {
        "/usr/sbin/lsof"
    } else {
        "lsof"
    }
}

/// `lsof -FpRn` over listening sockets: each pid's parent and its ports,
/// once each, in the order listed. A name is `*:3000`, `127.0.0.1:3000`
/// or `[::1]:3000`.
fn parse_sockets(out: &str) -> BTreeMap<u32, (u32, Vec<u16>)> {
    let mut map: BTreeMap<u32, (u32, Vec<u16>)> = BTreeMap::new();
    let mut pid = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.trim().parse().ok();
        } else if let (Some(ppid), Some(pid)) = (line.strip_prefix('R'), pid) {
            map.entry(pid).or_default().0 = ppid.trim().parse().unwrap_or(0);
        } else if let (Some(name), Some(pid)) = (line.strip_prefix('n'), pid) {
            let Some(port) = name.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) else {
                continue;
            };
            let ports = &mut map.entry(pid).or_default().1;
            if !ports.contains(&port) {
                ports.push(port);
            }
        }
    }
    map.retain(|_, (_, ports)| !ports.is_empty());
    map
}

/// `lsof -a -d cwd -Fpn`: each pid's working directory.
fn parse_cwds(out: &str) -> BTreeMap<u32, PathBuf> {
    let mut map = BTreeMap::new();
    let mut pid = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.trim().parse().ok();
        } else if let (Some(path), Some(pid)) = (line.strip_prefix('n'), pid) {
            map.insert(pid, PathBuf::from(path));
        }
    }
    map
}

/// Each of `pids`' parent and process group, from one `ps`; a pid `ps`
/// didn't list is missing.
pub async fn process_info(pids: &[u32]) -> BTreeMap<u32, (u32, u32)> {
    if pids.is_empty() {
        return BTreeMap::new();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut cmd = tokio::process::Command::new("/bin/ps");
    cmd.args(["-o", "pid=,ppid=,pgid=", "-p", &list])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let Ok(Ok(out)) = tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await else {
        return BTreeMap::new();
    };
    parse_process_info(&String::from_utf8_lossy(&out.stdout))
}

fn parse_process_info(out: &str) -> BTreeMap<u32, (u32, u32)> {
    out.lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace().map(|c| c.parse::<u32>().ok());
            Some((cols.next()??, (cols.next()??, cols.next()??)))
        })
        .collect()
}

/// Send `signal` to process `pid`, or with `group` to its whole process
/// group. Whether it was delivered: a process already gone is not.
pub fn signal(pid: u32, group: bool, signal: nix::sys::signal::Signal) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let target = nix::unistd::Pid::from_raw(if group { -raw } else { raw });
    nix::sys::signal::kill(target, signal).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sockets_parse_each_pids_ports_once() {
        let out = "p101\nR1\nf20\nn*:3000\nf21\nn[::1]:3000\np202\nR101\nf5\nn127.0.0.1:5173\nnlocalhost:bad\np303\nR1\n";
        let map = parse_sockets(out);
        assert_eq!(map[&101], (1, vec![3000]));
        assert_eq!(map[&202], (101, vec![5173]));
        assert!(!map.contains_key(&303), "no port, no listener");
    }

    #[test]
    fn process_info_reads_parent_and_group() {
        let map = parse_process_info("  101   1  101\n 202 101  101\nbad\n");
        assert_eq!(map[&101], (1, 101));
        assert_eq!(map[&202], (101, 101));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn cwds_parse_per_pid() {
        let out = "p101\nfcwd\nn/src/wt\np202\nfcwd\nn/src/other dir\n";
        let map = parse_cwds(out);
        assert_eq!(map[&101], PathBuf::from("/src/wt"));
        assert_eq!(map[&202], PathBuf::from("/src/other dir"));
    }
}
