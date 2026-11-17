//! RUN RECORDS: one JSON file per factory run, at
//! `<data_dir>/automations/runs/<identifier>.json` (`RIPLO-123.json`) —
//! the ticket and its sessions, every question asked and how long it
//! waited, the owner's corrections, the stages with their times, the
//! pull request with its verdict (and, in older records, the owner's
//! 👍/👎 on it), the fixes
//! sent, the merge, and the bugs that followed it. Week in review and the
//! weekly retro read them; nothing else does — there are no metrics.
//!
//! Every track writes through [`update`]: read, change, write back
//! whole and atomically, one writer at a time. Every field has a serde default, so a record
//! written before a field existed still loads, and a track adds what it
//! records without touching the others.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// A run's ticket kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunKind {
    Bug,
    Feature,
}

/// A question the run asked the owner.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Question {
    pub text: String,
    /// `product`, `arch`, `security`, `dep`, `scope`, or a plan approval
    /// (`plan`).
    pub tag: Option<String>,
    pub asked_at_ms: i64,
    pub answered_at_ms: Option<i64>,
    pub answer: Option<String>,
}

/// A stage the run reached, and when.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StageAt {
    pub stage: String,
    pub at_ms: i64,
}

/// Work sent to a fixer: why, and to which session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct FixerSend {
    pub at_ms: i64,
    /// What broke: `ci: <check>`, `conflict`, `review comment`, …
    pub reason: String,
    /// The session it went to, by name or id.
    pub to: String,
    /// It went to a fresh fixer with a handover, rather than the branch's
    /// own agent.
    pub fresh: bool,
}

/// A bug that followed the merge within 14 days.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct FollowUp {
    /// The fixing pull request, or the reopened ticket.
    pub what: String,
    pub at_ms: i64,
}

/// The lines a merged pull request changed in one file, on the merged
/// side: what a later fix is checked against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ChangedLines {
    pub path: String,
    /// First and last line of each run of changed lines, 1-based.
    pub ranges: Vec<(u32, u32)>,
}

/// One run's record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RunRecord {
    /// The ticket (`RIPLO-123`).
    pub identifier: String,
    /// The ticket's title: what the run set out to build.
    pub title: String,
    pub kind: Option<RunKind>,
    /// The sessions that worked on it (agent ids).
    pub sessions: Vec<String>,
    pub questions: Vec<Question>,
    /// Owner messages that redirected the agent, from the condensed
    /// transcripts at merge.
    pub corrections: Vec<String>,
    pub stages: Vec<StageAt>,
    pub pr_url: Option<String>,
    /// `merge`, `skim`, `review` or `blocked`, with its reasons.
    pub verdict: Option<String>,
    pub verdict_reasons: Vec<String>,
    /// The owner's "Verdict right?" (👍 true, 👎 false), from the vote
    /// the verdict card had until it was dropped: kept for older records.
    pub verdict_right: Option<bool>,
    pub cubic_dismissals: Vec<String>,
    pub fixer_sends: Vec<FixerSend>,
    pub merged_at_ms: Option<i64>,
    /// The project's main checkout, where a later fix is blamed.
    pub checkout: Option<PathBuf>,
    /// What the pull request landed as on its base branch: its merge (or
    /// squash) commit, then its own commits. Recorded at merge.
    pub merge_commits: Vec<String>,
    /// The lines it changed, recorded at merge.
    pub changed_lines: Vec<ChangedLines>,
    pub follow_ups: Vec<FollowUp>,
    /// The later fixes (their pull request URLs) already found not to
    /// touch its lines, so a sweep doesn't ask for their diffs again.
    pub checked_fixes: Vec<String>,
}

impl RunRecord {
    /// Every moment the record speaks of (epoch ms): a stage, a question
    /// asked or answered, a fix sent, the merge, a follow-up.
    fn moments(&self) -> impl Iterator<Item = i64> + '_ {
        let stages = self.stages.iter().map(|s| s.at_ms);
        let asked = self.questions.iter().map(|q| q.asked_at_ms);
        let answered = self.questions.iter().filter_map(|q| q.answered_at_ms);
        let sent = self.fixer_sends.iter().map(|f| f.at_ms);
        let follow = self.follow_ups.iter().map(|f| f.at_ms);
        stages
            .chain(asked)
            .chain(answered)
            .chain(sent)
            .chain(self.merged_at_ms)
            .chain(follow)
    }

    /// The latest of its moments, 0 for none.
    pub(crate) fn last_at_ms(&self) -> i64 {
        self.moments().max().unwrap_or(0)
    }

    /// Whether anything in it happened between `from_ms` and `to_ms`.
    pub(crate) fn active_between(&self, from_ms: i64, to_ms: i64) -> bool {
        self.moments().any(|at| (from_ms..=to_ms).contains(&at))
    }
}

/// Where the records live; None under test, so no test reads or writes
/// the real user's records.
pub(crate) fn dir() -> Option<PathBuf> {
    (!cfg!(test)).then(|| crate::automations::state_dir().join("runs"))
}

/// Change the record of the run on `identifier` with `f`, creating it on
/// first use, and write it back.
pub(crate) fn update(identifier: &str, f: impl FnOnce(&mut RunRecord)) {
    let Some(dir) = dir() else {
        return;
    };
    if let Err(err) = update_in(&dir, identifier, f) {
        tracing::warn!("run record {identifier}: {err}");
    }
}

/// The record of the run on `identifier`, if there is one.
pub(crate) fn read(identifier: &str) -> Option<RunRecord> {
    read_in(&dir()?, identifier).ok().flatten()
}

/// Every record, for week in review and the retro.
pub(crate) fn all() -> Vec<RunRecord> {
    dir().map(|dir| all_in(&dir)).unwrap_or_default()
}

fn all_in(dir: &Path) -> Vec<RunRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect()
}

fn path_in(dir: &Path, identifier: &str) -> PathBuf {
    let name: String = identifier
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                true => c,
                false => '_',
            },
        )
        .collect();
    dir.join(format!("{name}.json"))
}

fn read_in(dir: &Path, identifier: &str) -> std::io::Result<Option<RunRecord>> {
    match std::fs::read(path_in(dir, identifier)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(Into::into),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// [`update`] in `dir`, under [`WRITE_LOCK`]: the UI thread and the
/// follow-up sweep's workers both write records, and a read-modify-write
/// that interleaved with another would lose one of the two. A record that
/// is there but no longer parses (a torn write from an older orion, a hand
/// edit) is moved aside to `<name>.corrupt-<ms>` and started afresh, said
/// in the log, rather than block every later write to it.
fn update_in(dir: &Path, identifier: &str, f: impl FnOnce(&mut RunRecord)) -> std::io::Result<()> {
    let _held = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = path_in(dir, identifier);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let parsed = bytes.map(|bytes| serde_json::from_slice::<RunRecord>(&bytes));
    let mut record = match parsed {
        None => RunRecord::default(),
        Some(Ok(record)) => record,
        Some(Err(err)) => {
            let aside =
                path.with_extension(format!("json.corrupt-{}", orion_core::clock::now_ms()));
            std::fs::rename(&path, &aside)?;
            tracing::warn!(
                "run record {identifier} was unreadable ({err}): moved to {}, started afresh",
                aside.display()
            );
            RunRecord::default()
        }
    };
    if record.identifier.is_empty() {
        record.identifier = identifier.to_string();
    }
    f(&mut record);
    crate::pr_cache::write_json_atomic(&path, &record)
}

/// One read-modify-write of a run record at a time, in this process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_creates_then_changes_one_record() {
        let dir = tempfile::tempdir().unwrap();
        update_in(dir.path(), "RIPLO-1", |r| {
            r.kind = Some(RunKind::Bug);
            r.stages.push(StageAt {
                stage: "planning".into(),
                at_ms: 1,
            });
        })
        .unwrap();
        update_in(dir.path(), "RIPLO-1", |r| r.verdict = Some("merge".into())).unwrap();
        let record = read_in(dir.path(), "RIPLO-1").unwrap().unwrap();
        assert_eq!(record.identifier, "RIPLO-1");
        assert_eq!(record.kind, Some(RunKind::Bug));
        assert_eq!(record.stages.len(), 1);
        assert_eq!(record.verdict.as_deref(), Some("merge"));
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "no temp file is left behind");
    }

    #[test]
    fn an_old_record_loads_and_a_broken_one_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("RIPLO-2.json"),
            r#"{"identifier":"RIPLO-2"}"#,
        )
        .unwrap();
        let record = read_in(dir.path(), "RIPLO-2").unwrap().unwrap();
        assert!(record.questions.is_empty() && record.merged_at_ms.is_none());
        std::fs::write(dir.path().join("RIPLO-3.json"), "{not json").unwrap();
        update_in(dir.path(), "RIPLO-3", |r| r.verdict = Some("x".into())).unwrap();
        let record = read_in(dir.path(), "RIPLO-3").unwrap().unwrap();
        assert_eq!(record.identifier, "RIPLO-3");
        assert_eq!(record.verdict.as_deref(), Some("x"));
        let aside: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("RIPLO-3.json.corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1, "the broken one is kept aside");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&aside[0])).unwrap(),
            "{not json"
        );
        assert_eq!(all_in(dir.path()).len(), 2, "and not read as a record");
    }

    /// Writers on many threads at once lose nothing: each change lands,
    /// and no temp file is left behind.
    #[test]
    fn concurrent_updates_all_land() {
        let dir = tempfile::tempdir().unwrap();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let dir = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    for i in 0..10 {
                        update_in(&dir, "RIPLO-9", |r| {
                            r.fixer_sends.push(FixerSend {
                                at_ms: t * 100 + i,
                                ..FixerSend::default()
                            })
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let record = read_in(dir.path(), "RIPLO-9").unwrap().unwrap();
        assert_eq!(record.fixer_sends.len(), 80);
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "no temp file is left behind");
    }

    #[test]
    fn an_identifier_never_leaves_the_directory() {
        assert_eq!(
            path_in(Path::new("/r"), "../x/y"),
            PathBuf::from("/r/___x_y.json")
        );
    }
}
