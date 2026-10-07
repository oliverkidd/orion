//! BASE SYNC: every project's base branch kept current without a keypress.
//! On the **Fetch base branch** beat (Settings → Review), and at once after
//! a merge from the PULL REQUESTS MODAL, each project's ROOT WORKTREE
//! fetches origin — which brings every worktree of the project current
//! too, since they share its remote-tracking refs — and, when the root is
//! on the base branch, fast-forwards it onto origin's copy.
//!
//! The fast-forward is the PULL's (`git_sync::pull`) with every question
//! answered "not now" instead of asked: it waits while an agent works in
//! the root, never merges a diverged branch, and leaves the checkout as it
//! is when git would overwrite uncommitted changes. A root on any other
//! branch is only fetched. Nothing here ever says it failed — a laptop
//! offline for an hour is not news — so a FLASH shows only when the base
//! moved.
//!
//! One project at a time, off the loop (`crate::git_proc`), the most
//! overdue first; the answer lands on `App::base_sync.tx`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use orion_core::ProjectId;

use crate::app::App;
use crate::flash::Flash;
use crate::git_proc::{head_branch, remote_git, run, FETCH_TIMEOUT};

/// The **Fetch base branch** choices (Settings → Review), the default
/// first: how often each project's root fetches origin.
pub const INTERVALS: &[&str] = &["5m", "15m", "off"];

/// The beat a **Fetch base branch** value names; None for `off`, which
/// leaves only the fetch after a merge. Anything off the list reads as
/// the default.
pub fn interval(setting: &str) -> Option<Duration> {
    match setting {
        "off" => None,
        "15m" => Some(Duration::from_secs(15 * 60)),
        _ => Some(Duration::from_secs(5 * 60)),
    }
}

/// How a sync ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Origin fetched; the root stayed where it was — on another branch,
    /// already current, ahead of origin, or busy.
    Fetched,
    /// Origin fetched and the root, on the base branch, fast-forwarded
    /// `commits` onto `base`.
    Moved { base: String, commits: usize },
    /// No origin to fetch, or git's complaint: logged, never flashed.
    Skipped(String),
}

/// What the event loop hands back to [`land`].
#[derive(Debug)]
pub struct Answer {
    pub project: ProjectId,
    pub outcome: Outcome,
}

/// The syncs' state on `App::base_sync`.
#[derive(Debug, Default)]
pub struct Shared {
    /// Where the off-loop answers go; installed at startup. `None` in the
    /// unit tests, which never run git from the loop.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    /// The project being synced, if any: one at a time.
    pub inflight: Option<ProjectId>,
    /// When each project is next due. A project not in here is due now —
    /// every project syncs once soon after the TUI starts.
    pub due: HashMap<ProjectId, Instant>,
    /// The **Fetch base branch** beat; None is `off`.
    pub every: Option<Duration>,
}

// ---- git ----

/// The whole sync of the root checkout at `root`. `may_move` is false
/// while anything else is working in it, which leaves it fetched only.
/// Blocking: run off the loop.
pub fn sync(root: &Path, base_setting: &str, may_move: bool, quit: &AtomicBool) -> Outcome {
    if !crate::git_sync::has_origin(root) {
        return Outcome::Skipped("no origin".into());
    }
    if let Err(e) = remote_git(root, &["fetch", "--quiet", "origin"], FETCH_TIMEOUT, quit) {
        return Outcome::Skipped(e);
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

// ---- the loop's side ----

/// Sync `project` on the loop's next beat, past its timer: a merge just
/// landed, so origin's base has moved.
pub(crate) fn request_now(app: &mut App, project: &ProjectId) {
    app.base_sync.due.insert(project.clone(), Instant::now());
}

/// The root of the most overdue project, with whether it may be moved:
/// None while a sync runs or nothing is due. A root a PULL, a PUSH or a
/// branch switch has hold of is passed over this beat.
fn next_due(app: &App) -> Option<(ProjectId, PathBuf, bool)> {
    if app.base_sync.inflight.is_some() {
        return None;
    }
    let now = Instant::now();
    app.tree
        .projects
        .iter()
        .filter_map(|p| {
            let due = app.base_sync.due.get(&p.id).copied();
            // Never synced, with no beat: only a merge asks.
            let due = match (due, app.base_sync.every) {
                (Some(at), _) => at,
                (None, Some(_)) => now,
                (None, None) => return None,
            };
            (due <= now).then_some((due, p))
        })
        .min_by_key(|(due, _)| *due)
        .and_then(|(_, p)| {
            let root = app.root_worktree(&p.id)?;
            if app.is_placeholder_worktree(&root)
                || app.git_sync.inflight.contains_key(&root)
                || app.branch_switch.switching.contains_key(&root)
            {
                return None;
            }
            let path = app
                .tree
                .worktrees
                .iter()
                .find(|w| w.id == root)?
                .path
                .clone();
            let busy = app
                .tree
                .agents
                .iter()
                .any(|a| a.worktree_id == root && !a.archived && app.spins(a));
            Some((p.id.clone(), path, !busy))
        })
}

/// The git-poll beat: start the most overdue project's sync, if any.
pub(crate) fn tick(app: &mut App) {
    let Some((project, root, may_move)) = next_due(app) else {
        return;
    };
    app.base_sync.inflight = Some(project.clone());
    let Some(tx) = app.base_sync.tx.clone() else {
        return;
    };
    let quit = app.git_sync.quit.clone();
    tokio::task::spawn_blocking(move || {
        let base = crate::config::Config::load().worktree_base_branch;
        let outcome = sync(&root, &base, may_move, &quit);
        let _ = tx.send(Answer { project, outcome });
    });
}

/// A sync landed: the project is next due a beat from now, its checkouts
/// are read again so their `⇣` counts catch up, and a base that moved says
/// so.
pub(crate) fn land(app: &mut App, answer: Answer) {
    let Answer { project, outcome } = answer;
    app.base_sync.inflight = None;
    match app.base_sync.every {
        Some(every) => {
            app.base_sync
                .due
                .insert(project.clone(), Instant::now() + every);
        }
        None => {
            app.base_sync.due.remove(&project);
        }
    }
    match &outcome {
        Outcome::Skipped(why) => {
            tracing::debug!(project = %project.0, "base sync skipped: {why}");
            return;
        }
        Outcome::Moved { base, commits } if *commits > 0 => {
            let name = app
                .tree
                .projects
                .iter()
                .find(|p| p.id == project)
                .map_or_else(|| project.0.clone(), |p| p.name.clone());
            app.flash = Some(Flash::note(format!(
                "{name}: pulled {} from {base}",
                crate::bundle::plural(*commits, "commit")
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

    #[test]
    fn the_interval_reads_off_as_none_and_junk_as_the_default() {
        assert_eq!(interval("off"), None);
        assert_eq!(interval("15m"), Some(Duration::from_secs(900)));
        assert_eq!(interval("yolo"), Some(Duration::from_secs(300)));
    }
}
