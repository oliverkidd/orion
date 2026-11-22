//! Memory readings for the TUI's metrics modal: one machine-wide process
//! table (`orion_core::mem::process_table`, read in-process through
//! libproc on macOS), then a per-session sum over each PTY child's process
//! subtree (an agent CLI fans out into node workers, shells, MCP servers —
//! the user cares about the whole tree, not just the root).

use orion_core::mem::ProcEntry;
use orion_core::{MetricsSnapshot, SessionMetrics, SessionRef};
use std::collections::{HashMap, HashSet};

/// Take one reading. Blocking (walks the process table); call via
/// spawn_blocking.
pub fn collect(sessions: Vec<(SessionRef, u32, bool)>) -> MetricsSnapshot {
    snapshot_from_rows(
        &orion_core::mem::process_table().unwrap_or_default(),
        std::process::id(),
        &sessions,
        orion_core::mem::system_total_bytes().unwrap_or(0),
    )
}

/// Pure core, unit-testable: `rows` is the process table, one entry per
/// process.
fn snapshot_from_rows(
    rows: &[ProcEntry],
    daemon_pid: u32,
    sessions: &[(SessionRef, u32, bool)],
    system_total_bytes: u64,
) -> MetricsSnapshot {
    let mut rss: HashMap<u32, u64> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for row in rows {
        rss.insert(row.pid, row.rss_bytes);
        children.entry(row.ppid).or_default().push(row.pid);
    }

    let sessions = sessions
        .iter()
        .map(|(sref, pid, spare)| {
            let (rss_bytes, procs) = subtree_rss(*pid, &rss, &children);
            SessionMetrics {
                session: sref.clone(),
                pid: *pid,
                rss_bytes,
                procs,
                spare: *spare,
            }
        })
        .collect();

    MetricsSnapshot {
        daemon_pid,
        daemon_rss_bytes: rss.get(&daemon_pid).copied().unwrap_or(0),
        system_total_bytes,
        sessions,
    }
}

/// Sum RSS over `root` and every descendant. The visited set guards against
/// pid-reuse cycles in a racy process-table snapshot; a root that already exited
/// (absent from the table) reads as zero.
fn subtree_rss(
    root: u32,
    rss: &HashMap<u32, u64>,
    children: &HashMap<u32, Vec<u32>>,
) -> (u64, u32) {
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    let (mut bytes, mut procs) = (0u64, 0u32);
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(b) = rss.get(&pid) {
            bytes += b;
            procs += 1;
        }
        if let Some(kids) = children.get(&pid) {
            stack.extend(kids);
        }
    }
    (bytes, procs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{AgentId, TerminalId};

    fn sref(id: &str) -> SessionRef {
        SessionRef::Agent(AgentId(id.into()))
    }

    /// The tests read best as a `ps -axo pid=,ppid=,rss=` table (rss in KB).
    fn snapshot_from_table(
        table: &str,
        daemon_pid: u32,
        sessions: &[(SessionRef, u32, bool)],
        system_total_bytes: u64,
    ) -> MetricsSnapshot {
        let rows = orion_core::mem::parse_ps_rss_table(table);
        snapshot_from_rows(&rows, daemon_pid, sessions, system_total_bytes)
    }

    #[test]
    fn sums_each_sessions_whole_subtree() {
        // daemon 10 → session roots 20 and 30; 20 → {21, 22}, 22 → 23.
        // 99 is an unrelated process and must count nowhere.
        let table = "\
 10     1  1000
 20    10  2000
 21    20   300
 22    20   500
 23    22   200
 30    10    50
 99     1  9999
";
        let snap = snapshot_from_table(
            table,
            10,
            &[
                (sref("a"), 20, false),
                (SessionRef::Terminal(TerminalId("t".into())), 30, true),
            ],
            0,
        );
        assert_eq!(snap.daemon_rss_bytes, 1000 * 1024);
        assert_eq!(snap.sessions[0].rss_bytes, (2000 + 300 + 500 + 200) * 1024);
        assert_eq!(snap.sessions[0].procs, 4);
        assert_eq!(snap.sessions[1].rss_bytes, 50 * 1024);
        assert_eq!(snap.sessions[1].procs, 1);
        assert!(
            !snap.sessions[0].spare && snap.sessions[1].spare,
            "the flag rides along"
        );
    }

    #[test]
    fn exited_session_reads_zero() {
        let snap = snapshot_from_table(" 10 1 100\n", 10, &[(sref("a"), 555, false)], 0);
        assert_eq!(snap.sessions[0].rss_bytes, 0);
        assert_eq!(snap.sessions[0].procs, 0);
    }

    #[test]
    fn garbage_lines_are_skipped() {
        let table = "not numbers at all\n 20 10 100\n";
        let snap = snapshot_from_table(table, 10, &[(sref("a"), 20, false)], 0);
        assert_eq!(snap.sessions[0].rss_bytes, 100 * 1024);
        assert_eq!(snap.daemon_rss_bytes, 0);
    }

    /// The real table, read in-process: the daemon's own size is in it.
    #[test]
    fn a_live_reading_finds_this_process() {
        let snap = collect(vec![(sref("me"), std::process::id(), false)]);
        assert!(snap.daemon_rss_bytes > 0);
        assert!(snap.sessions[0].procs >= 1);
        assert!(snap.system_total_bytes > 0);
    }
}
