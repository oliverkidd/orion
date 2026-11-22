//! BASE SYNC: a project's base branch kept current without a keypress.
//! As you switch to a project whose last fetch is older than `base_fetch`
//! (half an hour by default), or as the window takes focus back on one,
//! and at once after a merge from the PULL REQUESTS MODAL, the project's
//! ROOT WORKTREE fetches origin — which brings every worktree of the
//! project current too, since they share its remote-tracking refs — and,
//! when the root is on the base branch, fast-forwards it onto origin's
//! copy.
//!
//! The fetch is THE RESOLVER's `BaseFetch(project)` (`sync::local`): asked
//! as the project is on screen, never on a beat of its own and never while
//! you are AWAY. A project nobody visits is not fetched at all — its
//! worktrees are cut by the DAEMON, which fetches origin itself first.
//!
//! The fast-forward is the PULL's (`git_sync::pull`) with every question
//! answered "not now" instead of asked: it waits while an agent works in
//! the root, never merges a diverged branch, and leaves the checkout as it
//! is when git would overwrite uncommitted changes. A root on any other
//! branch is only fetched. Nothing here ever says it failed — a laptop
//! offline for an hour is not news — so a FLASH shows only when the base
//! moved.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use orion_core::ProjectId;

use crate::app::App;
use crate::flash::Flash;
use crate::git_proc::{head_branch, remote_git, run, FETCH_TIMEOUT};

/// How old a project's last fetch may be when it is visited before it
/// is fetched again: the `base_fetch` setting, half an hour unless it says
/// otherwise; None when it is `off`. Asked by THE RESOLVER's policy on
/// every turn of the loop, so the setting is looked at again at most every
/// [`SETTING_TTL`] — not two `stat`s a frame.
pub fn fresh_for() -> Option<Duration> {
    #[cfg(test)]
    {
        crate::config::Config::cached().base_fetch_after()
    }
    #[cfg(not(test))]
    {
        use std::sync::Mutex;
        use std::time::Instant;
        static KNOWN: Mutex<Option<(Instant, Option<Duration>)>> = Mutex::new(None);
        let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
        match *known {
            Some((at, span)) if at.elapsed() < SETTING_TTL => span,
            _ => {
                let span = crate::config::Config::cached().base_fetch_after();
                *known = Some((Instant::now(), span));
                span
            }
        }
    }
}

/// How long [`fresh_for`] trusts the setting it read.
#[cfg_attr(test, allow(dead_code))]
const SETTING_TTL: Duration = Duration::from_secs(10);

/// How a sync ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Origin fetched; the root stayed where it was — on another branch,
    /// already current, ahead of origin, or busy.
    Fetched,
    /// Origin fetched, and neither its base nor the root's own branch
    /// moved there: no checkout's `⇣` can have changed.
    Current,
    /// Origin fetched and the root, on the base branch, fast-forwarded
    /// `commits` onto `base`.
    Moved { base: String, commits: usize },
    /// No origin to fetch, or git's complaint: logged, never flashed.
    Skipped(String),
}

// ---- git ----

/// The whole sync of the root checkout at `root`. `may_move` is false
/// while anything else is working in it, which leaves it fetched only.
/// Blocking: run off the loop.
pub fn sync(root: &Path, base_setting: &str, may_move: bool, quit: &AtomicBool) -> Outcome {
    if !crate::git_sync::has_origin(root) {
        return Outcome::Skipped("no origin".into());
    }
    let before = measured(root, base_setting);
    if let Err(e) = remote_git(root, &["fetch", "--quiet", "origin"], FETCH_TIMEOUT, quit) {
        return Outcome::Skipped(e);
    }
    if before.is_some() && before == measured(root, base_setting) {
        return Outcome::Current;
    }
    if !may_move {
        return Outcome::Fetched;
    }
    let Some(t) = head_branch(root).and_then(|b| crate::git_sync::tracking(root, &b)) else {
        return Outcome::Fetched;
    };
    // Only the base branch: a root parked on a feature branch is the
    // user's to move.
    if !t.stored || crate::commit_list::resolve_base(root, base_setting).as_ref() != Some(&t.name) {
        return Outcome::Fetched;
    }
    let commits = match crate::git_sync::ahead_behind(root, "HEAD", "@{u}") {
        Ok((0, behind)) if behind > 0 => behind,
        Ok(_) => return Outcome::Fetched,
        Err(e) => return Outcome::Skipped(e),
    };
    match run(root, &["merge", "--ff-only", "--quiet", "@{u}"]) {
        Ok(_) => Outcome::Moved {
            base: t.name,
            commits,
        },
        Err(e) => Outcome::Skipped(e),
    }
}

/// What the checkouts' `⇣` are measured against, as origin has it: its
/// copy of the base (the **Base branch**, else its default) and of the
/// root's own branch — read from the ref files, no process. None when the
/// files don't say, which counts as moved.
fn measured(root: &Path, base_setting: &str) -> Option<[Option<String>; 2]> {
    use crate::git_refs::{branch_sha, dirs, head_branch};
    let dirs = dirs(root)?;
    let base = match base_setting {
        "" => "origin/HEAD".to_string(),
        name => format!("origin/{name}"),
    };
    let base = branch_sha(&dirs, &base)?;
    let own = head_branch(&dirs).and_then(|b| branch_sha(&dirs, &format!("origin/{b}")));
    Some([Some(base), own])
}

// ---- the loop's side ----

/// Sync `project` now, past its freshness: a merge just landed, so
/// origin's base has moved. A failure's backoff still stands.
pub(crate) fn request_now(app: &mut App, project: &ProjectId) {
    let key = crate::sync::QueryKey::BaseFetch(project.clone());
    app.resolver.invalidate(&key);
    app.resolver.ask(key, std::time::Instant::now());
}

/// What a sync of `project` needs to start: its root's path, its base
/// setting, and whether it may be moved — not while anything else is
/// working in it. Err says why it can't start at all: no root yet, or one
/// a PULL, a PUSH or a branch switch has hold of.
pub(crate) fn start(app: &App, project: &ProjectId) -> Result<(PathBuf, String, bool), String> {
    let root = app
        .root_worktree(project)
        .ok_or_else(|| "no root worktree".to_string())?;
    if app.is_placeholder_worktree(&root) || app.git_sync.inflight.contains_key(&root) {
        return Err("the root is busy".into());
    }
    let path = app
        .tree
        .worktrees
        .iter()
        .find(|w| w.id == root)
        .map(|w| w.path.clone())
        .ok_or_else(|| "no root worktree".to_string())?;
    let busy = app
        .tree
        .agents
        .iter()
        .any(|a| a.worktree_id == root && !a.archived && app.spins(a));
    let base = app.tree.base_branch_at(&path);
    Ok((path, base, !busy))
}

/// A sync landed: its checkouts are read again so their `⇣` counts catch
/// up — unless origin brought nothing they are measured against — and a
/// base that moved says so.
pub(crate) fn land(app: &mut App, project: ProjectId, outcome: Outcome) {
    match &outcome {
        Outcome::Skipped(why) => {
            tracing::debug!(project = %project.0, "base sync skipped: {why}");
            return;
        }
        Outcome::Current => return,
        // The base moved: what the rule review waits on may have reached
        // it (`knowledge::review::refresh`).
        Outcome::Moved { commits, .. } if *commits > 0 => {
            crate::knowledge::review::want_refresh();
        }
        _ => {}
    }
    match &outcome {
        Outcome::Moved { base, commits } if *commits > 0 => {
            let name = app
                .tree
                .projects
                .iter()
                .find(|p| p.id == project)
                .map_or_else(|| project.0.clone(), |p| p.name.clone());
            app.flash = Some(Flash::note(format!(
                "{name}: pulled {} from {base}",
                crate::flash::plural(*commits, "commit")
            )));
        }
        _ => {}
    }
    let ours: HashSet<_> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| w.project_id == project)
        .map(|w| w.id.clone())
        .collect();
    crate::event_loop::reread_checkouts(app, &ours);
    app.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn identity(repo: &Path) {
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "t"]);
        git(repo, &["config", "commit.gpgsign", "false"]);
    }

    fn commit(repo: &Path, file: &str) {
        std::fs::write(repo.join(file), file).unwrap();
        git(repo, &["add", file]);
        git(repo, &["commit", "-q", "-m", file]);
    }

    /// An origin with `main`, a clone of it (the root), and a second
    /// clone standing in for everyone else pushing to origin.
    struct Repos {
        _dir: tempfile::TempDir,
        root: PathBuf,
        other: PathBuf,
    }

    fn repos() -> Repos {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let seed = dir.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        identity(&seed);
        commit(&seed, "a");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        let clone = |name: &str| {
            let path = dir.path().join(name);
            git(
                dir.path(),
                &[
                    "clone",
                    "-q",
                    origin.to_str().unwrap(),
                    path.to_str().unwrap(),
                ],
            );
            identity(&path);
            path
        };
        let (root, other) = (clone("root"), clone("other"));
        Repos {
            _dir: dir,
            root,
            other,
        }
    }

    /// Someone else lands `n` commits on origin's main.
    fn others_merge(r: &Repos, n: usize) {
        for i in 0..n {
            commit(&r.other, &format!("theirs-{i}"));
        }
        git(&r.other, &["push", "-q", "origin", "main"]);
    }

    fn quit() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// A fetch that brought nothing the checkouts are measured against
    /// says so, and reads none of them again.
    #[test]
    fn a_fetch_that_moved_nothing_is_current() {
        let r = repos();
        assert_eq!(sync(&r.root, "", true, &quit()), Outcome::Current);
        commit(&r.other, "elsewhere");
        git(&r.other, &["push", "-q", "origin", "main:side"]);
        assert_eq!(
            sync(&r.root, "", true, &quit()),
            Outcome::Current,
            "another branch moved"
        );
        others_merge(&r, 1);
        assert!(matches!(
            sync(&r.root, "", true, &quit()),
            Outcome::Moved { .. }
        ));
    }

    #[test]
    fn a_root_on_the_base_branch_fast_forwards() {
        let r = repos();
        others_merge(&r, 2);
        let outcome = sync(&r.root, "", true, &quit());
        assert_eq!(
            outcome,
            Outcome::Moved {
                base: "origin/main".into(),
                commits: 2
            }
        );
        assert_eq!(
            git(&r.root, &["rev-parse", "HEAD"]),
            git(&r.other, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn a_busy_root_is_only_fetched() {
        let r = repos();
        others_merge(&r, 1);
        let before = git(&r.root, &["rev-parse", "HEAD"]);
        assert_eq!(sync(&r.root, "", false, &quit()), Outcome::Fetched);
        assert_eq!(git(&r.root, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            git(&r.root, &["rev-parse", "origin/main"]),
            git(&r.other, &["rev-parse", "HEAD"]),
            "the fetch still landed"
        );
    }

    #[test]
    fn a_root_on_another_branch_is_only_fetched() {
        let r = repos();
        git(&r.root, &["checkout", "-q", "-b", "feat"]);
        git(&r.root, &["push", "-q", "-u", "origin", "feat"]);
        others_merge(&r, 1);
        let before = git(&r.root, &["rev-parse", "HEAD"]);
        assert_eq!(sync(&r.root, "", true, &quit()), Outcome::Fetched);
        assert_eq!(git(&r.root, &["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn a_diverged_root_is_left_alone() {
        let r = repos();
        commit(&r.root, "mine");
        others_merge(&r, 1);
        let before = git(&r.root, &["rev-parse", "HEAD"]);
        assert_eq!(sync(&r.root, "", true, &quit()), Outcome::Fetched);
        assert_eq!(git(&r.root, &["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn uncommitted_changes_in_the_way_keep_head() {
        let r = repos();
        others_merge(&r, 1);
        std::fs::write(r.root.join("theirs-0"), "local edit").unwrap();
        let before = git(&r.root, &["rev-parse", "HEAD"]);
        assert!(matches!(
            sync(&r.root, "", true, &quit()),
            Outcome::Skipped(_)
        ));
        assert_eq!(git(&r.root, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            std::fs::read_to_string(r.root.join("theirs-0")).unwrap(),
            "local edit"
        );
    }
}
