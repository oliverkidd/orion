//! `orion factory skip <ticket or session> [--backlog]`: **Not for orion**
//! from a shell, with or without orion running — the same
//! pipeline as the button ([`super::owner::not_for_the_factory`]), each
//! step printed as it goes:
//!
//! 1. the ticket (and every ticket on its branch) labelled `factory-skip`,
//!    commented on (once: a ticket that says so already isn't again), and
//!    moved to its team's Triage — or with `--backlog`, its Backlog
//!    ([`super::owner::skip_writes`]);
//! 2. the draft pull request its session opened closed, with a comment;
//! 3. its session archived through the daemon, as the TUI archives one;
//! 4. its worktree removed through the daemon, as the TUI removes one (its
//!    delete hooks and containers with it) — only when it is clean and
//!    every commit on it is on a remote or the base ([`why_keep`]), else
//!    kept, and why said ("kept: 2 unpushed commits") — and then its local
//!    branch, when every commit on it is on a remote
//!    ([`crate::clean_worktrees::delete_pushed_branch`]): nothing of it is
//!    only on this machine;
//! 5. the ticket marked parked and `skipped` in the builder ledger, which
//!    a running orion takes up on its next tick ([`super::Shared::
//!    adopt_skips`]), so it never restarts or nudges the session.
//!
//! A step that fails says why and the rest still run, and running it again
//! on a ticket already taken off is safe: the label and state are set
//! again, the comment isn't made twice, and a session already archived
//! or a worktree already gone is said and passed over. The target is the
//! ledger's Linear issue id, the ticket's identifier (`ENG-7`, any case),
//! or the name of the session working on it.
//!
//! An orion built before this command keeps its own copy of the ledger
//! and may write it back over the mark. The label is what keeps the
//! ticket off the intake either way.

use std::path::{Path, PathBuf};

use orion_core::{AgentId, ProjectId, WorktreeId};

use super::owner::{self, Write};
use super::{Entry, Ledger, Stage};

/// What the pipeline reaches outside the ledger: the sessions, projects
/// and worktrees orion knows, Linear and GitHub, git, and the daemon.
pub(crate) trait Outside {
    /// The ids of the live sessions named `name`.
    fn sessions_named(&self, name: &str) -> Vec<String>;
    /// Whether session `id` is live (not archived).
    fn is_live(&self, id: &AgentId) -> bool;
    /// The Linear key of project `project`.
    fn linear_key(&self, project: &ProjectId) -> Option<String>;
    /// The worktree of `project` on `branch`, its id and path, and the
    /// project's base branch setting.
    fn worktree(&self, project: &ProjectId, branch: &str) -> Option<(WorktreeId, PathBuf, String)>;
    /// Why the checkout at `path` must be kept ([`why_keep`]).
    fn why_keep(&self, path: &Path, base: &str) -> Option<String>;
    /// Run one Linear or GitHub write under `key`; `Some` says more about
    /// how it went.
    async fn write(&mut self, key: &str, write: &Write) -> Result<Option<String>, String>;
    /// Archive `session` through the daemon.
    async fn archive(&mut self, session: &AgentId) -> Result<(), String>;
    /// Remove worktree `id` through the daemon.
    async fn remove_worktree(&mut self, id: &WorktreeId) -> Result<(), String>;
    /// Delete `project`'s local `branch` when every commit on it is on a
    /// remote: whether there was one, or why it was kept.
    fn delete_branch(&mut self, project: &ProjectId, branch: &str) -> Result<bool, String>;
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

/// Why the checkout at `path` must not be removed: uncommitted or
/// untracked files, or commits on no remote branch and not in the base
/// (`base_setting`, the project's **Base branch**, resolved as the commit
/// lists resolve it). None when nothing in it would be lost — or it is
/// gone from disk already. Runs git: never on the UI thread.
pub(crate) fn why_keep(path: &Path, base_setting: &str) -> Option<String> {
    use crate::git_proc::read;
    if !path.exists() {
        return None;
    }
    let status = match read(path, &["status", "--porcelain"]) {
        Ok(status) => status,
        Err(why) => return Some(format!("kept: git can't read it ({})", why.trim())),
    };
    let changed = status.lines().filter(|l| !l.is_empty()).count();
    if changed > 0 {
        return Some(format!(
            "kept: {changed} uncommitted change{}",
            if changed == 1 { "" } else { "s" }
        ));
    }
    let base = crate::commit_list::resolve_base_cached(path, base_setting);
    let mut args = vec!["rev-list", "--count", "HEAD", "--not", "--remotes"];
    if let Some(base) = base.as_deref() {
        args.push(base);
    }
    let unpushed: usize = match read(path, &args) {
        Ok(n) => n.trim().parse().unwrap_or(0),
        Err(why) => return Some(format!("kept: git can't read it ({})", why.trim())),
    };
    (unpushed > 0).then(|| {
        format!(
            "kept: {unpushed} unpushed commit{}",
            if unpushed == 1 { "" } else { "s" }
        )
    })
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
    let tickets: Vec<owner::SkipTicket> = ids
        .iter()
        .filter_map(|gid| Some(owner::SkipTicket::of(gid, ledger.issues.get(gid)?)))
        .collect();
    let names: Vec<&str> = tickets.iter().map(|t| t.identifier.as_str()).collect();
    say(format!(
        "{}: taking {} off the factory, to {}",
        entry.identifier,
        names.join(", "),
        if backlog { "Backlog" } else { "Triage" }
    ));
    let key = outside
        .linear_key(&entry.project)
        .ok_or_else(|| format!("no Linear key for {}'s project", entry.identifier))?;
    for write in owner::skip_writes(&tickets, entry.pr_url.as_deref(), backlog) {
        let what = owner::describe(&write);
        match outside.write(&key, &write).await {
            Ok(None) => say(format!("✓ {what}")),
            Ok(Some(how)) => say(format!("✓ {what}: {how}")),
            Err(why) => say(format!("✗ {what}: {why}")),
        }
    }
    if entry.pr_url.is_none() {
        say("· no draft pull request to close".into());
    }
    match &entry.session {
        Some(session) if outside.is_live(session) => match outside.archive(session).await {
            Ok(()) => say(format!("✓ archive the session {session}")),
            Err(why) => say(format!("✗ archive the session {session}: {why}")),
        },
        Some(session) => say(format!("· the session {session} is archived already")),
        None => say("· no session to archive".into()),
    }
    let checkout_gone = match outside.worktree(&entry.project, &entry.branch) {
        Some((wid, path, base)) => {
            let shown = path.display().to_string();
            match outside.why_keep(&path, &base) {
                Some(why) => {
                    say(format!("· worktree {shown} {why}"));
                    false
                }
                None => match outside.remove_worktree(&wid).await {
                    Ok(()) => {
                        say(format!("✓ remove the worktree {shown}"));
                        true
                    }
                    Err(why) => {
                        say(format!("✗ remove the worktree {shown}: {why}"));
                        false
                    }
                },
            }
        }
        None => {
            say("· no worktree to remove".into());
            true
        }
    };
    if checkout_gone && !entry.branch.is_empty() {
        let branch = &entry.branch;
        match outside.delete_branch(&entry.project, branch) {
            Ok(true) => say(format!("✓ delete the branch {branch}")),
            Ok(false) => say(format!("· no local branch {branch}")),
            Err(why) => say(format!("· branch {branch} kept: {why}")),
        }
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

/// What the daemon's database says, for `orion factory skip`: the live
/// sessions named as the target, every live session, every project's
/// repository and every worktree, read-only.
#[derive(Debug, Clone, Default)]
pub struct Known {
    /// The ids of the live sessions named as the target.
    pub named: Vec<String>,
    /// The ids of every live session.
    pub live: Vec<String>,
    /// `(project id, repository)`.
    pub projects: Vec<(String, PathBuf)>,
    /// `(worktree id, project id, branch, path)`.
    pub worktrees: Vec<(String, String, String, PathBuf)>,
}

/// The real world: [`Known`], Linear and `gh` for the writes, git for the
/// worktree, the daemon for the archive and the removal.
pub(crate) struct Live(pub Known);

impl Live {
    /// The repository of `project`.
    fn repo(&self, project: &ProjectId) -> Option<&Path> {
        self.0
            .projects
            .iter()
            .find(|(id, _)| *id == project.0)
            .map(|(_, repo)| repo.as_path())
    }
}

impl Outside for Live {
    fn sessions_named(&self, _name: &str) -> Vec<String> {
        self.0.named.clone()
    }

    fn is_live(&self, id: &AgentId) -> bool {
        self.0.live.contains(&id.0)
    }

    fn linear_key(&self, project: &ProjectId) -> Option<String> {
        crate::linear::read_linear_key(self.repo(project)?)
    }

    fn worktree(&self, project: &ProjectId, branch: &str) -> Option<(WorktreeId, PathBuf, String)> {
        if branch.is_empty() {
            return None;
        }
        let (id, _, _, path) = self
            .0
            .worktrees
            .iter()
            .find(|(_, p, b, _)| *p == project.0 && b == branch)?;
        let base = self
            .repo(project)
            .map(|repo| crate::config::Config::load().project(repo).base_branch)
            .unwrap_or_default();
        Some((WorktreeId(id.clone()), path.clone(), base))
    }

    fn why_keep(&self, path: &Path, base: &str) -> Option<String> {
        why_keep(path, base)
    }

    async fn write(&mut self, key: &str, write: &Write) -> Result<Option<String>, String> {
        owner::run_write(key, write).await
    }

    async fn archive(&mut self, session: &AgentId) -> Result<(), String> {
        crate::ipc::archive_agent(session)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    async fn remove_worktree(&mut self, id: &WorktreeId) -> Result<(), String> {
        crate::ipc::delete_worktree(id)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    fn delete_branch(&mut self, project: &ProjectId, branch: &str) -> Result<bool, String> {
        let repo = self.repo(project).ok_or("its project isn't known")?;
        crate::clean_worktrees::delete_pushed_branch(repo, branch)
    }
}

/// Where the builder ledger lives.
pub(crate) fn ledger_path() -> PathBuf {
    crate::automations::state_dir().join("linear-builder.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linear, GitHub, git and the daemon, scripted: every call recorded,
    /// a write refused when its description says so, the worktree kept
    /// when `keep` says why.
    #[derive(Default)]
    struct Scripted {
        calls: Vec<String>,
        refuse: Option<&'static str>,
        archived: bool,
        keep: Option<&'static str>,
    }

    impl Outside for Scripted {
        fn sessions_named(&self, name: &str) -> Vec<String> {
            match name {
                "crash-fix" => vec!["s7".into()],
                _ => Vec::new(),
            }
        }

        fn is_live(&self, _id: &AgentId) -> bool {
            !self.archived
        }

        fn linear_key(&self, project: &ProjectId) -> Option<String> {
            (project.0 == "p").then(|| "lin_key".into())
        }

        fn worktree(
            &self,
            project: &ProjectId,
            branch: &str,
        ) -> Option<(WorktreeId, PathBuf, String)> {
            (project.0 == "p" && branch == "eng-7-crash").then(|| {
                (
                    WorktreeId("w7".into()),
                    PathBuf::from("/wt/eng-7-crash"),
                    String::new(),
                )
            })
        }

        fn why_keep(&self, _path: &Path, _base: &str) -> Option<String> {
            self.keep.map(str::to_string)
        }

        async fn write(&mut self, key: &str, write: &Write) -> Result<Option<String>, String> {
            let what = owner::describe(write);
            self.calls.push(format!("{key}: {what}"));
            match self.refuse {
                Some(word) if what.contains(word) => Err("refused".into()),
                _ => Ok(None),
            }
        }

        async fn archive(&mut self, session: &AgentId) -> Result<(), String> {
            self.calls.push(format!("archive {session}"));
            Ok(())
        }

        async fn remove_worktree(&mut self, id: &WorktreeId) -> Result<(), String> {
            self.calls.push(format!("remove {id}"));
            Ok(())
        }

        fn delete_branch(&mut self, _project: &ProjectId, branch: &str) -> Result<bool, String> {
            self.calls.push(format!("delete branch {branch}"));
            Ok(true)
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

    /// Label, comment and Backlog, the draft, the session, the worktree,
    /// the ledger — in that order, each said.
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
                "lin_key: comment \"Taken off orion by …\"",
                "lin_key: move the ticket to Backlog",
                &format!("lin_key: close the draft {url}"),
                "archive s7",
                "remove w7",
                "delete branch eng-7-crash",
            ]
        );
        assert_eq!(
            said,
            [
                "ENG-7: taking ENG-7 off the factory, to Backlog",
                "✓ label the ticket factory-skip",
                "✓ comment \"Taken off orion by …\"",
                "✓ move the ticket to Backlog",
                &format!("✓ close the draft {url}"),
                "✓ archive the session s7",
                "✓ remove the worktree /wt/eng-7-crash",
                "✓ delete the branch eng-7-crash",
                "✓ mark it parked in the builder ledger",
            ]
        );
        let ledger = read(&path).unwrap();
        let entry = &ledger.issues["uuid-ENG-7"];
        assert_eq!(entry.stage, Stage::Parked);
        assert!(entry.skipped);
    }

    /// Found by its session's name, it goes back to Triage; a refused step
    /// says so and the rest still run; a worktree with work on it is kept.
    #[test]
    fn by_session_name_to_triage_past_a_refusal_and_a_kept_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let path = ledger_with(dir.path(), None);
        let mut outside = Scripted {
            refuse: Some("label"),
            keep: Some("kept: 2 unpushed commits"),
            ..Scripted::default()
        };
        let said = run(&path, "crash-fix", false, &mut outside);
        assert_eq!(
            said,
            [
                "ENG-7: taking ENG-7 off the factory, to Triage",
                "✗ label the ticket factory-skip: refused",
                "✓ comment \"Taken off orion by …\"",
                "✓ move the ticket to Triage",
                "· no draft pull request to close",
                "✓ archive the session s7",
                "· worktree /wt/eng-7-crash kept: 2 unpushed commits",
                "✓ mark it parked in the builder ledger",
            ]
        );
        assert!(!outside.calls.iter().any(|c| c.starts_with("remove")));
        assert!(
            !outside.calls.iter().any(|c| c.starts_with("delete branch")),
            "a kept worktree keeps its branch"
        );
        assert!(read(&path).unwrap().issues["uuid-ENG-7"].skipped);
    }

    /// Run again on a ticket already taken off: the session archived
    /// already is passed over, and the rest runs again.
    #[test]
    fn running_it_again_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = ledger_with(dir.path(), None);
        run(&path, "ENG-7", false, &mut Scripted::default());
        let mut again = Scripted {
            archived: true,
            ..Scripted::default()
        };
        let said = run(&path, "ENG-7", false, &mut again);
        assert!(said.contains(&"· the session s7 is archived already".to_string()));
        assert!(said.contains(&"✓ move the ticket to Triage".to_string()));
        assert!(!again.calls.iter().any(|c| c.starts_with("archive")));
        assert!(again.calls.contains(&"remove w7".to_string()));
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

    /// A checkout with a change, or a commit on no remote, is kept and
    /// says why; a clean one whose commits are all pushed isn't; one gone
    /// from disk isn't either.
    #[test]
    fn a_worktree_is_kept_while_anything_in_it_would_be_lost() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str], at: &Path| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(at)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        let origin = dir.path().join("origin.git");
        std::fs::create_dir(&origin).unwrap();
        git(&["init", "--bare", "-q"], &origin);
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&["init", "-q", "-b", "main"], &repo);
        git(
            &["remote", "add", "origin", origin.to_str().unwrap()],
            &repo,
        );
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git(&["add", "."], &repo);
        git(&["commit", "-q", "-m", "a"], &repo);
        git(&["push", "-q", "origin", "main"], &repo);
        assert_eq!(why_keep(&repo, ""), None, "clean and pushed");
        git(&["checkout", "-q", "-b", "feature"], &repo);
        std::fs::write(repo.join("b.txt"), "b").unwrap();
        assert_eq!(
            why_keep(&repo, "").as_deref(),
            Some("kept: 1 uncommitted change")
        );
        git(&["add", "."], &repo);
        git(&["commit", "-q", "-m", "b"], &repo);
        std::fs::write(repo.join("c.txt"), "c").unwrap();
        git(&["add", "."], &repo);
        git(&["commit", "-q", "-m", "c"], &repo);
        assert_eq!(
            why_keep(&repo, "").as_deref(),
            Some("kept: 2 unpushed commits")
        );
        assert_eq!(why_keep(&dir.path().join("gone"), ""), None);
    }
}
