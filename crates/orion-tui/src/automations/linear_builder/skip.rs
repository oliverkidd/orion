//! `orion factory skip <ticket or session> [--backlog]`: **Not for the
//! factory** from a shell, with or without orion running — the same
//! pipeline as the button ([`super::owner::not_for_the_factory`]), each
//! step printed as it goes:
//!
//! 1. the ticket (and every ticket on its branch) labelled `factory-skip`
//!    and commented on — and with `--backlog`, moved to its team's
//!    Backlog ([`super::owner::skip_writes`]);
//! 2. the draft pull request its session opened closed, with a comment;
//! 3. its session archived through the daemon, as the TUI archives one;
//! 4. the ticket marked parked and `skipped` in the builder ledger, which
//!    a running orion takes up on its next tick ([`super::Shared::
//!    adopt_skips`]), so it never restarts or nudges the session.
//!
//! A step that fails says why and the rest still run. The target is the
//! ledger's Linear issue id, the ticket's identifier (`ENG-7`, any case),
//! or the name of the session working on it.
//!
//! An orion built before this command keeps its own copy of the ledger
//! and may write it back over the mark. The label is what keeps the
//! ticket off the intake either way.

use std::path::{Path, PathBuf};

use orion_core::{AgentId, ProjectId};

use super::owner::{self, Write};
use super::{Entry, Ledger, Stage};

/// What the pipeline reaches outside the ledger: the sessions and
/// projects orion knows, Linear and GitHub, and the daemon.
pub(crate) trait Outside {
    /// The ids of the live sessions named `name`.
    fn sessions_named(&self, name: &str) -> Vec<String>;
    /// The Linear key of project `project`.
    fn linear_key(&self, project: &ProjectId) -> Option<String>;
    /// Run one Linear or GitHub write under `key`.
    async fn write(&mut self, key: &str, write: &Write) -> Result<(), String>;
    /// Archive `session` through the daemon.
    async fn archive(&mut self, session: &AgentId) -> Result<(), String>;
}

/// The ledger's id of the ticket `target` names: an id, an identifier, or
/// the name of its session (`sessions`, the ids of the sessions so named).
/// An entry at work wins over one that ended.
pub(crate) fn find(ledger: &Ledger, target: &str, sessions: &[String]) -> Option<String> {
    let target = target.trim();
    let matches = |id: &String, e: &Entry| {
        id == target
            || e.identifier.eq_ignore_ascii_case(target)
            || e.session.as_ref().is_some_and(|s| sessions.contains(&s.0))
    };
    let mut found: Vec<(&String, &Entry)> = ledger
        .issues
        .iter()
        .filter(|(id, e)| matches(id, e))
        .collect();
    found.sort_by_key(|(_, e)| !(e.stage.is_active() || e.stage == Stage::PrOpen));
    found.first().map(|(id, _)| (*id).clone())
}

/// The ledger ids of the tickets on `id`'s branch, `id` first.
fn group(ledger: &Ledger, id: &str) -> Vec<String> {
    let Some(entry) = ledger.issues.get(id) else {
        return Vec::new();
    };
    let mut ids = vec![id.to_string()];
    if !entry.branch.is_empty() {
        ids.extend(
            ledger
                .issues
                .iter()
                .filter(|(other, e)| {
                    *other != id && e.project == entry.project && e.branch == entry.branch
                })
                .map(|(other, _)| other.clone()),
        );
    }
    ids
}

fn read(path: &Path) -> Result<Ledger, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Take the ticket `target` names off the factory, every step said
/// through `say`. Err when there is nothing to take off: no ledger, no
/// such ticket, or no Linear key for its project.
pub(crate) async fn skip(
    ledger_path: &Path,
    target: &str,
    backlog: bool,
    outside: &mut impl Outside,
    say: &mut impl FnMut(String),
) -> Result<(), String> {
    let ledger = read(ledger_path)?;
    let sessions = outside.sessions_named(target);
    let id = find(&ledger, target, &sessions)
        .ok_or_else(|| format!("no factory ticket or session matches {target}"))?;
    let entry = ledger.issues[&id].clone();
    let ids = group(&ledger, &id);
    let tickets: Vec<(String, String, String)> = ids
        .iter()
        .filter_map(|gid| {
            let e = ledger.issues.get(gid)?;
            Some((gid.clone(), e.team_id.clone(), e.identifier.clone()))
        })
        .collect();
    let names: Vec<&str> = tickets.iter().map(|(_, _, i)| i.as_str()).collect();
    say(format!(
        "{}: taking {} off the factory{}",
        entry.identifier,
        names.join(", "),
        if backlog { ", to Backlog" } else { "" }
    ));
    let key = outside
        .linear_key(&entry.project)
        .ok_or_else(|| format!("no Linear key for {}'s project", entry.identifier))?;
    for write in owner::skip_writes(&tickets, entry.pr_url.as_deref(), backlog) {
        let what = owner::describe(&write);
        match outside.write(&key, &write).await {
            Ok(()) => say(format!("✓ {what}")),
            Err(why) => say(format!("✗ {what}: {why}")),
        }
    }
    if entry.pr_url.is_none() {
        say("· no draft pull request to close".into());
    }
    match &entry.session {
        Some(session) => match outside.archive(session).await {
            Ok(()) => say(format!("✓ archive the session {session}")),
            Err(why) => say(format!("✗ archive the session {session}: {why}")),
        },
        None => say("· no session to archive".into()),
    }
    // Read again: orion may have written it meanwhile.
    match mark(ledger_path, &ids) {
        Ok(()) => say("✓ mark it parked in the builder ledger".into()),
        Err(why) => say(format!("✗ mark it parked in the builder ledger: {why}")),
    }
    Ok(())
}

/// `ids` parked and skipped in the ledger at `path`, written atomically.
fn mark(path: &Path, ids: &[String]) -> Result<(), String> {
    let mut ledger = read(path)?;
    let now = crate::app::now_ms();
    for id in ids {
        if let Some(e) = ledger.issues.get_mut(id) {
            e.set_stage(Stage::Parked, now);
            e.skipped = true;
        }
    }
    let bytes = serde_json::to_vec_pretty(&ledger).map_err(|e| e.to_string())?;
    orion_core::settings::write_atomic(path, &bytes).map_err(|e| e.to_string())
}

/// The real world: the daemon's database for names and projects, Linear
/// and `gh` for the writes, the daemon for the archive.
pub(crate) struct Live {
    pub sessions: Vec<String>,
    pub projects: Vec<(String, PathBuf)>,
}

impl Outside for Live {
    fn sessions_named(&self, _name: &str) -> Vec<String> {
        self.sessions.clone()
    }

    fn linear_key(&self, project: &ProjectId) -> Option<String> {
        let (_, dir) = self.projects.iter().find(|(id, _)| *id == project.0)?;
        crate::linear::read_linear_key(dir)
    }

    async fn write(&mut self, key: &str, write: &Write) -> Result<(), String> {
        owner::run_write(key, write).await
    }

    async fn archive(&mut self, session: &AgentId) -> Result<(), String> {
        crate::ipc::archive_agent(session)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// Where the builder ledger lives.
pub(crate) fn ledger_path() -> PathBuf {
    crate::automations::state_dir().join("linear-builder.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linear, GitHub and the daemon, scripted: every call recorded, a
    /// write refused when its description says so.
    #[derive(Default)]
    struct Scripted {
        calls: Vec<String>,
        refuse: Option<&'static str>,
    }

    impl Outside for Scripted {
        fn sessions_named(&self, name: &str) -> Vec<String> {
            match name {
                "crash-fix" => vec!["s7".into()],
                _ => Vec::new(),
            }
        }

        fn linear_key(&self, project: &ProjectId) -> Option<String> {
            (project.0 == "p").then(|| "lin_key".into())
        }

        async fn write(&mut self, key: &str, write: &Write) -> Result<(), String> {
            let what = owner::describe(write);
            self.calls.push(format!("{key}: {what}"));
            match self.refuse {
                Some(word) if what.contains(word) => Err("refused".into()),
                _ => Ok(()),
            }
        }

        async fn archive(&mut self, session: &AgentId) -> Result<(), String> {
            self.calls.push(format!("archive {session}"));
            Ok(())
        }
    }

    fn ledger_with(dir: &Path, pr: Option<&str>) -> PathBuf {
        let mut ledger = Ledger::default();
        let mut entry = Entry {
            automation: "bug_fixer".into(),
            identifier: "ENG-7".into(),
            project: ProjectId("p".into()),
            branch: "eng-7-crash".into(),
            session: Some(AgentId("s7".into())),
            team_id: "t1".into(),
            pr_url: pr.map(str::to_string),
            ..Entry::default()
        };
        entry.set_stage(Stage::Building, 1);
        ledger.issues.insert("uuid-ENG-7".into(), entry);
        let path = dir.join("linear-builder.json");
        std::fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
        path
    }

    fn run(path: &Path, target: &str, backlog: bool, outside: &mut Scripted) -> Vec<String> {
        let mut said = Vec::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(skip(path, target, backlog, outside, &mut |l| said.push(l)))
            .unwrap();
        said
    }

    /// Label and comment, Backlog, the draft, the session, the ledger —
    /// in that order, each said.
    #[test]
    fn the_steps_run_in_order_and_each_is_said() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://github.com/o/r/pull/9";
        let path = ledger_with(dir.path(), Some(url));
        let mut outside = Scripted::default();
        let said = run(&path, "eng-7", true, &mut outside);
        assert_eq!(
            outside.calls,
            [
                "lin_key: label the ticket factory-skip",
                "lin_key: comment \"Taken off the factory by …\"",
                "lin_key: move the ticket to Backlog",
                &format!("lin_key: close the draft {url}"),
                "archive s7",
            ]
        );
        assert_eq!(
            said,
            [
                "ENG-7: taking ENG-7 off the factory, to Backlog",
                "✓ label the ticket factory-skip",
                "✓ comment \"Taken off the factory by …\"",
                "✓ move the ticket to Backlog",
                &format!("✓ close the draft {url}"),
                "✓ archive the session s7",
                "✓ mark it parked in the builder ledger",
            ]
        );
        let ledger = read(&path).unwrap();
        let entry = &ledger.issues["uuid-ENG-7"];
        assert_eq!(entry.stage, Stage::Parked);
        assert!(entry.skipped);
    }

    /// Found by its session's name; left as is, nothing moves to Backlog;
    /// a refused step says so and the rest still run.
    #[test]
    fn by_session_name_left_as_is_and_past_a_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let path = ledger_with(dir.path(), None);
        let mut outside = Scripted {
            refuse: Some("label"),
            ..Scripted::default()
        };
        let said = run(&path, "crash-fix", false, &mut outside);
        assert_eq!(
            said,
            [
                "ENG-7: taking ENG-7 off the factory",
                "✗ label the ticket factory-skip: refused",
                "✓ comment \"Taken off the factory by …\"",
                "· no draft pull request to close",
                "✓ archive the session s7",
                "✓ mark it parked in the builder ledger",
            ]
        );
        assert!(read(&path).unwrap().issues["uuid-ENG-7"].skipped);
    }

    #[test]
    fn an_unknown_ticket_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = ledger_with(dir.path(), None);
        let mut outside = Scripted::default();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let out = rt.block_on(skip(&path, "ENG-99", false, &mut outside, &mut |_| {}));
        assert_eq!(
            out,
            Err("no factory ticket or session matches ENG-99".into())
        );
        assert!(outside.calls.is_empty());
    }
}
