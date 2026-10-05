//! A checkout kept in step with its remote from the grid: a PULL (`p`, a
//! band's **Pull**) brings down what its upstream has, and a PUSH (`⇧P`,
//! **Push**) sends up what it has. Nothing else here reaches a remote on
//! its own — the band rule's `⇡⇣` count against whatever the last fetch
//! left — so these are the keypresses that make them current.
//!
//! Both are fast-forward only, and both fetch first. A pull moves the
//! branch onto its upstream when the upstream is simply ahead; a push
//! moves the upstream onto the branch when the branch is simply ahead. A
//! branch with commits of its own and new ones upstream is DIVERGED, and
//! either way is left as it is — never a merge, a rebase or a force — with
//! the reason in the FLASH. A pull that would overwrite uncommitted changes
//! stops with git's own complaint, so it never loses work; it asks first
//! only while an agent is working in the checkout, whose files may change
//! under it. A push touches no files and asks only before it lands
//! straight on the base branch (`origin/main`, say: on many repositories
//! that is a release).
//!
//! A pull is `git fetch` then `git merge --ff-only @{u}` rather than `git
//! pull`, so the user's `pull.rebase` / `pull.ff` can't change what `p`
//! does, and a push names its upstream (`branch.<b>.remote`, `.merge`) so
//! `push.default` can't either. A branch that tracks nothing — a linked
//! worktree is cut `--no-track` — has nothing to pull, so a pull fetches
//! origin alone (every worktree of a project shares its remote-tracking
//! refs, so that brings `⇣` current on every band of it), and a push
//! publishes it to origin under its own name, tracking it from then on.
//!
//! The git runs off the loop, detached like the BRANCH SWITCHER's (whose
//! helpers it borrows), one PULL or PUSH per checkout at a time; the answer
//! lands on `App::git_sync.tx` as a FLASH, and the project's checkouts are
//! read again at once so the band rules catch up.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orion_core::{Worktree, WorktreeId};

use crate::app::{App, ConfirmDialog, Focus, Overlay, PendingAction};
use crate::branch_switch::{head_branch, read, remote_git, run, FETCH_TIMEOUT};
use crate::bundle::plural;
use crate::flash::Flash;
use crate::keymap::Action;
use crate::pr_actions::PUSH_TIMEOUT;

/// How long a push held back from the base branch waits for the second
/// `⇧P` that sends it.
const ARM_WINDOW: Duration = Duration::from_secs(30);

/// Which way a checkout is being synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Pull,
    /// `confirmed` once the push onto the base branch has been asked about.
    Push {
        confirmed: bool,
    },
}

impl Op {
    /// A push not yet asked about.
    pub const PUSH: Op = Op::Push { confirmed: false };

    fn verb(self) -> &'static str {
        match self {
            Op::Pull => "pull",
            Op::Push { .. } => "push",
        }
    }

    fn doing(self) -> &'static str {
        match self {
            Op::Pull => "pulling",
            Op::Push { .. } => "pushing",
        }
    }
}

/// How a PULL or a PUSH ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Pull: fast-forwarded `commits` onto `upstream`.
    Pulled { upstream: String, commits: usize },
    /// Pull: nothing new upstream; `ahead` commits of its own to push.
    UpToDate { upstream: String, ahead: usize },
    /// Pull: the branch tracks nothing (or HEAD is detached); `fetched`
    /// when origin was fetched in its place.
    NoUpstream { fetched: bool },
    /// Push: `upstream` fast-forwarded by `commits`.
    Pushed { upstream: String, commits: usize },
    /// Push: a branch that tracked nothing, now on origin and tracking it.
    Published { upstream: String },
    /// Push: nothing the upstream lacks; `behind` commits to pull.
    NothingToPush { upstream: String, behind: usize },
    /// Push: `upstream` is the base branch, so it waits for a second `⇧P`.
    ConfirmPush { upstream: String, ahead: usize },
    /// Both sides moved: left as it was.
    Diverged {
        upstream: String,
        ahead: usize,
        behind: usize,
    },
    /// git's one-line complaint, the checkout and its upstream untouched.
    Failed(String),
}

/// What the event loop hands back to [`land`].
#[derive(Debug)]
pub struct Answer {
    pub worktree: WorktreeId,
    pub op: Op,
    pub outcome: Outcome,
}

/// The syncs' state on `App::git_sync`.
#[derive(Debug, Default)]
pub struct Shared {
    /// Where the off-loop answers go; installed at startup like
    /// `branch_switch.tx`. `None` in the unit tests, which never run git
    /// from the loop.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    /// Checkouts with a PULL or a PUSH running — never two in one.
    pub inflight: HashMap<WorktreeId, Op>,
    /// A push held back from the base branch, and when: the checkout's
    /// next PUSH within [`ARM_WINDOW`] goes ahead. Asked by a second press
    /// rather than a dialog, which would open seconds after the key — over
    /// whatever was opened since, or under keys meant for a pane.
    pub armed: Option<(WorktreeId, Instant)>,
    /// Raised when the TUI goes away, so a fetch or a push still running
    /// is stopped rather than holding the runtime's shutdown for its whole
    /// budget.
    pub quit: Arc<AtomicBool>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

// ---- git ----

/// What HEAD's branch tracks: `branch.<b>.remote` and `.merge`, and the
/// name it goes by. A fork's pull request checkout tracks its read-only
/// `refs/pull/N/head`, which no fetch refspec stores, so `@{u}` can't
/// resolve it: `stored` says whether one can.
#[derive(Debug)]
struct Tracking {
    branch: String,
    remote: String,
    merge: String,
    /// `origin/feat`, or `origin/pull/7/head` for an unstored one.
    name: String,
    stored: bool,
}

/// What `branch` tracks; None when it tracks nothing.
fn tracking(root: &Path, branch: &str) -> Option<Tracking> {
    let config = |key: &str| {
        read(
            root,
            &["config", "--get", &format!("branch.{branch}.{key}")],
        )
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    };
    let (remote, merge) = (config("remote")?, config("merge")?);
    let stored = read(
        root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .ok()
    .map(|out| out.trim().to_string())
    .filter(|u| !u.is_empty());
    let short = merge
        .strip_prefix("refs/heads/")
        .or_else(|| merge.strip_prefix("refs/"))
        .unwrap_or(&merge);
    Some(Tracking {
        branch: branch.to_string(),
        name: stored
            .clone()
            .unwrap_or_else(|| format!("{remote}/{short}")),
        stored: stored.is_some(),
        remote,
        merge,
    })
}

fn has_origin(root: &Path) -> bool {
    read(root, &["remote"]).is_ok_and(|out| out.lines().any(|r| r == "origin"))
}

/// Fetch what `t` tracks, and say where its tip is: `@{u}` for a stored
/// upstream — the whole remote fetched, so every band's `⇣` catches up
/// too — else the commit the fetch of its ref alone brought back.
fn fetch_tip(root: &Path, t: &Tracking, quit: &AtomicBool) -> Result<String, String> {
    if t.stored {
        remote_git(
            root,
            &["fetch", "--quiet", t.remote.as_str()],
            FETCH_TIMEOUT,
            quit,
        )?;
        return Ok("@{u}".into());
    }
    let args = ["fetch", "--quiet", t.remote.as_str(), t.merge.as_str()];
    remote_git(root, &args, FETCH_TIMEOUT, quit)?;
    // By commit: an agent's own fetch would rewrite FETCH_HEAD.
    read(root, &["rev-parse", "--verify", "FETCH_HEAD^{commit}"]).map(|sha| sha.trim().to_string())
}

/// Fetch what `t` tracks and count HEAD against it: the tip to move onto,
/// and the commits each side has that the other lacks.
fn measure(root: &Path, t: &Tracking, quit: &AtomicBool) -> Result<(String, usize, usize), String> {
    let tip = fetch_tip(root, t, quit)?;
    let (ahead, behind) = ahead_behind(root, &tip)?;
    Ok((tip, ahead, behind))
}

/// Commits HEAD has that `tip` lacks, and the other way round.
fn ahead_behind(root: &Path, tip: &str) -> Result<(usize, usize), String> {
    let range = format!("HEAD...{tip}");
    let counts = read(root, &["rev-list", "--left-right", "--count", &range])?;
    let mut nums = counts.split_whitespace().map(str::parse::<usize>);
    match (nums.next(), nums.next()) {
        (Some(Ok(ahead)), Some(Ok(behind))) => Ok((ahead, behind)),
        _ => Err(format!("unreadable count: {}", counts.trim())),
    }
}

/// The whole PULL of the checkout at `root`. Blocking: run off the loop.
pub fn pull(root: &Path, quit: &AtomicBool) -> Outcome {
    let Some(t) = head_branch(root).and_then(|b| tracking(root, &b)) else {
        if !has_origin(root) {
            return Outcome::NoUpstream { fetched: false };
        }
        return match remote_git(root, &["fetch", "--quiet", "origin"], FETCH_TIMEOUT, quit) {
            Ok(()) => Outcome::NoUpstream { fetched: true },
            Err(e) => Outcome::Failed(e),
        };
    };
    let (tip, ahead, behind) = match measure(root, &t, quit) {
        Ok(measured) => measured,
        Err(e) => return Outcome::Failed(e),
    };
    let upstream = t.name;
    if behind == 0 {
        return Outcome::UpToDate { upstream, ahead };
    }
    if ahead > 0 {
        return Outcome::Diverged {
            upstream,
            ahead,
            behind,
        };
    }
    match run(root, &["merge", "--ff-only", "--quiet", &tip]) {
        Ok(_) => Outcome::Pulled {
            upstream,
            commits: behind,
        },
        Err(e) => Outcome::Failed(e),
    }
}

/// The whole PUSH of the checkout at `root`. `base_setting` is the
/// `worktree_base_branch` SETTING, which names the branch a push asks
/// about unless `confirmed`. Blocking: run off the loop.
pub fn push(root: &Path, base_setting: &str, confirmed: bool, quit: &AtomicBool) -> Outcome {
    let Some(branch) = head_branch(root) else {
        return Outcome::Failed("HEAD is detached: no branch to push".into());
    };
    let refspec = format!("refs/heads/{branch}");
    let Some(t) = tracking(root, &branch) else {
        if !has_origin(root) {
            return Outcome::Failed("no origin to push to".into());
        }
        let publish = format!("{refspec}:{refspec}");
        let args = [
            "push",
            "--quiet",
            "--set-upstream",
            "origin",
            publish.as_str(),
        ];
        return match remote_git(root, &args, PUSH_TIMEOUT, quit) {
            Ok(()) => Outcome::Published {
                upstream: format!("origin/{branch}"),
            },
            Err(e) => Outcome::Failed(e),
        };
    };
    if t.merge.starts_with("refs/pull/") {
        return Outcome::Failed(format!(
            "{} tracks a pull request's ref ({}): push to its fork from a terminal",
            t.branch, t.name
        ));
    }
    let (_, ahead, behind) = match measure(root, &t, quit) {
        Ok(measured) => measured,
        Err(e) => return Outcome::Failed(e),
    };
    let upstream = t.name.clone();
    if ahead == 0 {
        return Outcome::NothingToPush { upstream, behind };
    }
    if behind > 0 {
        return Outcome::Diverged {
            upstream,
            ahead,
            behind,
        };
    }
    if !confirmed
        && crate::commit_list::resolve_base_cached(root, base_setting).as_ref() == Some(&upstream)
    {
        return Outcome::ConfirmPush { upstream, ahead };
    }
    let send = format!("{refspec}:{}", t.merge);
    let args = ["push", "--quiet", t.remote.as_str(), send.as_str()];
    match remote_git(root, &args, PUSH_TIMEOUT, quit) {
        Ok(()) => Outcome::Pushed {
            upstream,
            commits: ahead,
        },
        Err(e) => Outcome::Failed(e),
    }
}

// ---- the loop's side ----

/// The checkout `p` and `⇧P` act on: the band under the cursor, else the
/// selected project's root — the BRANCH SWITCHER's aim, linked worktrees
/// included.
pub(crate) fn target(app: &App) -> Option<WorktreeId> {
    let on_checkout = matches!(
        app.focus,
        Focus::Worktrees | Focus::Sessions | Focus::Terminal
    );
    match app.selected_worktree().filter(|_| on_checkout) {
        Some(w) => Some(w.id.clone()),
        None => app
            .selected_project()
            .and_then(|p| app.root_worktree(&p.id)),
    }
}

/// `p` / `⇧P`: sync `worktree` `op`'s way — a pull asking first while an
/// agent works there, and a push confirmed by the second press it asked
/// for.
pub(crate) fn request(app: &mut App, worktree: WorktreeId, op: Op) {
    // Any other sync, or a press past the window, lets an armed push go.
    let armed = app
        .git_sync
        .armed
        .take()
        .is_some_and(|(id, at)| id == worktree && at.elapsed() < ARM_WINDOW);
    let Some(name) = refused(app, &worktree) else {
        return;
    };
    let working = || {
        app.tree
            .agents
            .iter()
            .any(|a| a.worktree_id == worktree && !a.archived && app.spins(a))
    };
    let op = match op {
        Op::Pull if working() => {
            app.overlay = Some(Overlay::Confirm(ConfirmDialog {
                title: "Pull".into(),
                message: format!(
                    "An agent is working in {name}. Pull anyway? Files may change under it."
                ),
                action: PendingAction::PullWorktree(worktree),
                area: ratatui::layout::Rect::default(),
            }));
            return;
        }
        Op::Push { .. } if armed => Op::Push { confirmed: true },
        op => op,
    };
    start(app, worktree, op);
}

/// The checkout's label, or None — with the FLASH saying why — when it
/// can't sync now: a stand-in not on disk yet, a sync already running,
/// or a branch switch, whose git would trip over its index lock.
fn refused(app: &mut App, worktree: &WorktreeId) -> Option<String> {
    let w = app.tree.worktrees.iter().find(|w| &w.id == worktree)?;
    if app.is_placeholder_worktree(worktree) {
        return None;
    }
    let name = label(w);
    let why = if let Some(running) = app.git_sync.inflight.get(worktree) {
        format!("already {} {name}", running.doing())
    } else if app.branch_switch.switching.contains_key(worktree) {
        format!("{name} is switching branch")
    } else {
        return Some(name);
    };
    app.flash = Some(Flash::note(why));
    None
}

/// Sync `worktree` now — [`request`] past its questions, and a confirm's
/// yes.
pub(crate) fn start(app: &mut App, worktree: WorktreeId, op: Op) {
    let Some(name) = refused(app, &worktree) else {
        return;
    };
    let Some(root) = app
        .tree
        .worktrees
        .iter()
        .find(|w| w.id == worktree)
        .map(|w| w.path.clone())
    else {
        return;
    };
    app.git_sync.inflight.insert(worktree.clone(), op);
    app.flash = Some(Flash::working(format!("{} {name}…", op.doing())));
    app.dirty = true;
    let Some(tx) = app.git_sync.tx.clone() else {
        return;
    };
    let quit = app.git_sync.quit.clone();
    tokio::task::spawn_blocking(move || {
        let outcome = match op {
            Op::Pull => pull(&root, &quit),
            Op::Push { confirmed } => {
                let base = crate::config::Config::load().worktree_base_branch;
                push(&root, &base, confirmed, &quit)
            }
        };
        let _ = tx.send(Answer {
            worktree,
            op,
            outcome,
        });
    });
}

/// A sync landed: say how it went — a push held back from the base
/// branch arming the second press that sends it — and have the project's
/// checkouts read again, so their band rules catch up now.
pub(crate) fn land(app: &mut App, answer: Answer) {
    let Answer {
        worktree,
        op,
        outcome,
    } = answer;
    app.git_sync.inflight.remove(&worktree);
    app.dirty = true;
    let Some(w) = app.tree.worktrees.iter().find(|w| w.id == worktree) else {
        return;
    };
    let project = w.project_id.clone();
    let mut flash = flash_for(&label(w), op, &outcome);
    match &outcome {
        Outcome::ConfirmPush { .. } => {
            app.git_sync.armed = Some((worktree.clone(), Instant::now()));
            let again = crate::hints::act(&app.keymap, Action::PushWorktree, "push")
                .map_or_else(|| "Push".to_string(), |h| h.key.to_string());
            flash.text.push_str(&format!(" · {again} again to push"));
        }
        Outcome::Pushed { .. } | Outcome::Published { .. } | Outcome::NothingToPush { .. } => {
            if let Some(n) = app.worktree_changes(&worktree).filter(|&n| n > 0) {
                flash
                    .text
                    .push_str(&format!(" · {} not pushed", plural(n, "uncommitted file")));
            }
        }
        _ => {}
    }
    app.flash = Some(flash);
    if matches!(outcome, Outcome::Pushed { .. } | Outcome::Published { .. }) {
        // A push can open, move or close the branch's pull request.
        app.pr_refresh_requested = true;
    }
    if !matches!(outcome, Outcome::Failed(_)) {
        let ours: HashSet<WorktreeId> = app
            .tree
            .worktrees
            .iter()
            .filter(|w| w.project_id == project)
            .map(|w| w.id.clone())
            .collect();
        crate::event_loop::reread_checkouts(app, &ours);
    }
}

/// `⌂ main` or `⎇ feat`: the checkout as its band's rule names it.
fn label(w: &Worktree) -> String {
    let mark = if w.is_main { '⌂' } else { '⎇' };
    format!("{mark} {}", w.branch)
}

fn commits(n: usize) -> String {
    plural(n, "commit")
}

/// The FLASH for how `op` on `name` ended.
fn flash_for(name: &str, op: Op, outcome: &Outcome) -> Flash {
    match outcome {
        Outcome::Pulled { upstream, commits: n } => {
            Flash::done(format!("{name} pulled {} from {upstream}", commits(*n)))
        }
        Outcome::UpToDate { upstream, ahead } => {
            let push = if *ahead > 0 {
                format!(" · ⇡{ahead} to push")
            } else {
                String::new()
            };
            Flash::done(format!("{name} is up to date with {upstream}{push}"))
        }
        Outcome::NoUpstream { fetched: true } => {
            Flash::note(format!("fetched origin · {name} tracks no remote branch yet"))
        }
        Outcome::NoUpstream { fetched: false } => {
            Flash::note(format!("{name} tracks no remote branch, and there is no origin"))
        }
        Outcome::Pushed { upstream, commits: n } => {
            Flash::done(format!("{name} pushed {} to {upstream}", commits(*n)))
        }
        Outcome::Published { upstream } => {
            Flash::done(format!("{name} published to {upstream}"))
        }
        Outcome::NothingToPush { upstream, behind } => {
            let pull = if *behind > 0 {
                format!(" · ⇣{behind} to pull")
            } else {
                String::new()
            };
            Flash::note(format!("{name} has nothing to push to {upstream}{pull}"))
        }
        Outcome::ConfirmPush { upstream, ahead } => Flash::note(format!(
            "{name} would push {} straight to {upstream}",
            commits(*ahead)
        )),
        Outcome::Diverged {
            upstream,
            ahead,
            behind,
        } => Flash::failed(format!(
            "{name} and {upstream} have both moved (⇡{ahead} ⇣{behind}): rebase or merge it in a terminal"
        )),
        Outcome::Failed(e) => Flash::failed(format!("{} {name}: {e}", op.verb())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::FlashKind;
    use std::path::PathBuf;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
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

    fn commit(repo: &Path, file: &str, body: &str) {
        std::fs::write(repo.join(file), body).unwrap();
        git(repo, &["add", file]);
        git(repo, &["commit", "-q", "-m", body]);
    }

    /// A bare `origin` with one commit on `main`, the checkout under test
    /// cloned from it (`local`), and a second clone (`other`) to push new
    /// work upstream from.
    struct Repos {
        _dir: tempfile::TempDir,
        origin: PathBuf,
        local: PathBuf,
        other: PathBuf,
    }

    fn repos() -> Repos {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let seed = dir.path().join("seed");
        std::fs::create_dir(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        identity(&seed);
        commit(&seed, "a.txt", "one\n");
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
        let local = clone("local");
        let other = clone("other");
        Repos {
            _dir: dir,
            origin,
            local,
            other,
        }
    }

    fn upstream_moves(r: &Repos, n: usize) {
        for i in 0..n {
            commit(&r.other, "b.txt", &format!("upstream {i}\n"));
        }
        git(&r.other, &["push", "-q", "origin", "main"]);
    }

    /// `local` on `feat`, published to origin and tracking it.
    fn on_tracked_feature(r: &Repos) {
        git(&r.local, &["switch", "-q", "-c", "feat"]);
        git(&r.local, &["push", "-q", "-u", "origin", "feat"]);
    }

    fn quit() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn origin_tip(r: &Repos, branch: &str) -> String {
        git(&r.origin, &["rev-parse", branch])
    }

    // ---- pull ----

    #[test]
    fn behind_only_fast_forwards_and_counts() {
        let r = repos();
        upstream_moves(&r, 3);
        assert_eq!(
            pull(&r.local, &quit()),
            Outcome::Pulled {
                upstream: "origin/main".into(),
                commits: 3
            }
        );
        assert_eq!(
            git(&r.local, &["rev-parse", "HEAD"]),
            git(&r.local, &["rev-parse", "origin/main"])
        );
    }

    #[test]
    fn level_is_up_to_date_and_reports_ahead() {
        let r = repos();
        commit(&r.local, "c.txt", "mine\n");
        assert_eq!(
            pull(&r.local, &quit()),
            Outcome::UpToDate {
                upstream: "origin/main".into(),
                ahead: 1
            }
        );
    }

    #[test]
    fn diverged_never_merges() {
        let r = repos();
        upstream_moves(&r, 1);
        commit(&r.local, "c.txt", "mine\n");
        let before = git(&r.local, &["rev-parse", "HEAD"]);
        assert_eq!(
            pull(&r.local, &quit()),
            Outcome::Diverged {
                upstream: "origin/main".into(),
                ahead: 1,
                behind: 1
            }
        );
        assert_eq!(git(&r.local, &["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn local_changes_in_the_way_fail_and_keep_head() {
        let r = repos();
        commit(&r.other, "a.txt", "theirs\n");
        git(&r.other, &["push", "-q", "origin", "main"]);
        std::fs::write(r.local.join("a.txt"), "mine, uncommitted\n").unwrap();
        let before = git(&r.local, &["rev-parse", "HEAD"]);
        match pull(&r.local, &quit()) {
            Outcome::Failed(e) => assert!(e.contains("would be overwritten"), "{e}"),
            other => panic!("expected a failure, got {other:?}"),
        }
        assert_eq!(git(&r.local, &["rev-parse", "HEAD"]), before);
        assert_eq!(
            std::fs::read_to_string(r.local.join("a.txt")).unwrap(),
            "mine, uncommitted\n"
        );
    }

    #[test]
    fn no_upstream_fetches_origin() {
        let r = repos();
        git(
            &r.local,
            &["switch", "-q", "--no-track", "-c", "feat", "origin/main"],
        );
        upstream_moves(&r, 2);
        let before = git(&r.local, &["rev-parse", "origin/main"]);
        assert_eq!(
            pull(&r.local, &quit()),
            Outcome::NoUpstream { fetched: true }
        );
        assert_ne!(git(&r.local, &["rev-parse", "origin/main"]), before);
        assert_eq!(
            git(&r.local, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feat"
        );
    }

    #[test]
    fn detached_head_is_no_upstream() {
        let r = repos();
        git(&r.local, &["switch", "-q", "--detach", "HEAD"]);
        assert_eq!(
            pull(&r.local, &quit()),
            Outcome::NoUpstream { fetched: true }
        );
    }

    #[test]
    fn no_origin_fetches_nothing() {
        let r = repos();
        git(
            &r.local,
            &["switch", "-q", "--no-track", "-c", "feat", "origin/main"],
        );
        git(&r.local, &["remote", "remove", "origin"]);
        assert_eq!(
            pull(&r.local, &quit()),
            Outcome::NoUpstream { fetched: false }
        );
    }

    #[test]
    fn an_unreachable_remote_fails_with_gits_line() {
        let r = repos();
        git(
            &r.local,
            &["remote", "set-url", "origin", "/nonexistent/origin.git"],
        );
        match pull(&r.local, &quit()) {
            Outcome::Failed(e) => assert!(!e.is_empty() && !e.starts_with("fatal:"), "{e}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_raised_quit_cancels_the_fetch() {
        let r = repos();
        let quit = AtomicBool::new(true);
        assert_eq!(
            pull(&r.local, &quit),
            Outcome::Failed("fetch cancelled".into())
        );
    }

    /// A fork's pull request checkout tracks `refs/pull/N/head`, which no
    /// refspec stores: it still pulls, as `git pull` would.
    #[test]
    fn a_pull_requests_unstored_ref_still_pulls() {
        let r = repos();
        git(
            &r.origin,
            &["update-ref", "refs/pull/7/head", "refs/heads/main"],
        );
        git(&r.local, &["switch", "-q", "-c", "pr-7"]);
        git(&r.local, &["config", "branch.pr-7.remote", "origin"]);
        git(
            &r.local,
            &["config", "branch.pr-7.merge", "refs/pull/7/head"],
        );
        upstream_moves(&r, 2);
        git(
            &r.origin,
            &["update-ref", "refs/pull/7/head", "refs/heads/main"],
        );
        assert_eq!(
            pull(&r.local, &quit()),
            Outcome::Pulled {
                upstream: "origin/pull/7/head".into(),
                commits: 2
            }
        );
        assert_eq!(
            git(&r.local, &["rev-parse", "HEAD"]),
            origin_tip(&r, "main")
        );
    }

    // ---- push ----

    #[test]
    fn a_tracked_branch_pushes_its_new_commits() {
        let r = repos();
        on_tracked_feature(&r);
        commit(&r.local, "c.txt", "one\n");
        commit(&r.local, "c.txt", "two\n");
        assert_eq!(
            push(&r.local, "", false, &quit()),
            Outcome::Pushed {
                upstream: "origin/feat".into(),
                commits: 2
            }
        );
        assert_eq!(
            origin_tip(&r, "feat"),
            git(&r.local, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn a_branch_that_tracks_nothing_is_published_and_tracks_it() {
        let r = repos();
        git(
            &r.local,
            &["switch", "-q", "--no-track", "-c", "feat", "origin/main"],
        );
        commit(&r.local, "c.txt", "mine\n");
        assert_eq!(
            push(&r.local, "", false, &quit()),
            Outcome::Published {
                upstream: "origin/feat".into()
            }
        );
        assert_eq!(
            origin_tip(&r, "feat"),
            git(&r.local, &["rev-parse", "HEAD"])
        );
        assert_eq!(
            git(&r.local, &["rev-parse", "--abbrev-ref", "@{u}"]),
            "origin/feat"
        );
    }

    #[test]
    fn the_base_branch_asks_before_it_is_pushed_to() {
        let r = repos();
        commit(&r.local, "c.txt", "mine\n");
        let before = origin_tip(&r, "main");
        assert_eq!(
            push(&r.local, "main", false, &quit()),
            Outcome::ConfirmPush {
                upstream: "origin/main".into(),
                ahead: 1
            }
        );
        assert_eq!(origin_tip(&r, "main"), before, "asking pushes nothing");
        assert_eq!(
            push(&r.local, "main", true, &quit()),
            Outcome::Pushed {
                upstream: "origin/main".into(),
                commits: 1
            }
        );
        assert_eq!(
            origin_tip(&r, "main"),
            git(&r.local, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn nothing_new_is_nothing_to_push_and_says_whats_to_pull() {
        let r = repos();
        on_tracked_feature(&r);
        assert_eq!(
            push(&r.local, "", false, &quit()),
            Outcome::NothingToPush {
                upstream: "origin/feat".into(),
                behind: 0
            }
        );
        git(&r.other, &["fetch", "-q", "origin"]);
        git(&r.other, &["switch", "-q", "feat"]);
        commit(&r.other, "d.txt", "theirs\n");
        git(&r.other, &["push", "-q", "origin", "feat"]);
        assert_eq!(
            push(&r.local, "", false, &quit()),
            Outcome::NothingToPush {
                upstream: "origin/feat".into(),
                behind: 1
            }
        );
    }

    #[test]
    fn a_diverged_branch_is_never_forced() {
        let r = repos();
        upstream_moves(&r, 1);
        commit(&r.local, "c.txt", "mine\n");
        let before = origin_tip(&r, "main");
        assert_eq!(
            push(&r.local, "", true, &quit()),
            Outcome::Diverged {
                upstream: "origin/main".into(),
                ahead: 1,
                behind: 1
            }
        );
        assert_eq!(origin_tip(&r, "main"), before);
    }

    #[test]
    fn a_detached_head_has_nothing_to_push() {
        let r = repos();
        git(&r.local, &["switch", "-q", "--detach", "HEAD"]);
        assert_eq!(
            push(&r.local, "", false, &quit()),
            Outcome::Failed("HEAD is detached: no branch to push".into())
        );
    }

    #[test]
    fn a_fork_pull_requests_ref_is_not_pushed_to() {
        let r = repos();
        git(&r.local, &["switch", "-q", "-c", "pr-7"]);
        git(&r.local, &["config", "branch.pr-7.remote", "origin"]);
        git(
            &r.local,
            &["config", "branch.pr-7.merge", "refs/pull/7/head"],
        );
        git(
            &r.origin,
            &["update-ref", "refs/pull/7/head", "refs/heads/main"],
        );
        git(&r.local, &["fetch", "-q", "origin", "refs/pull/7/head"]);
        match push(&r.local, "", false, &quit()) {
            Outcome::Failed(e) => assert!(e.contains("fork"), "{e}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_pre_push_hook_that_refuses_says_why() {
        use std::os::unix::fs::PermissionsExt;
        let r = repos();
        on_tracked_feature(&r);
        commit(&r.local, "c.txt", "mine\n");
        let hook = r.local.join(".git/hooks/pre-push");
        std::fs::write(&hook, "#!/bin/sh\necho 'tests failed' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let before = origin_tip(&r, "feat");
        assert_eq!(
            push(&r.local, "", false, &quit()),
            Outcome::Failed("rejected: tests failed".into())
        );
        assert_eq!(origin_tip(&r, "feat"), before);
    }

    // ---- flash ----

    #[test]
    fn each_outcome_reads_as_its_flash() {
        let up = || "origin/feat".to_string();
        let pull = Op::Pull;
        let push = Op::PUSH;
        let cases = [
            (
                pull,
                Outcome::Pulled {
                    upstream: up(),
                    commits: 1,
                },
                FlashKind::Done,
                "⎇ feat pulled 1 commit from origin/feat",
            ),
            (
                pull,
                Outcome::Pulled {
                    upstream: up(),
                    commits: 3,
                },
                FlashKind::Done,
                "⎇ feat pulled 3 commits from origin/feat",
            ),
            (
                pull,
                Outcome::UpToDate {
                    upstream: up(),
                    ahead: 0,
                },
                FlashKind::Done,
                "⎇ feat is up to date with origin/feat",
            ),
            (
                pull,
                Outcome::UpToDate {
                    upstream: up(),
                    ahead: 2,
                },
                FlashKind::Done,
                "⎇ feat is up to date with origin/feat · ⇡2 to push",
            ),
            (
                pull,
                Outcome::Diverged {
                    upstream: up(),
                    ahead: 2,
                    behind: 3,
                },
                FlashKind::Failed,
                "⎇ feat and origin/feat have both moved (⇡2 ⇣3): rebase or merge it in a terminal",
            ),
            (
                pull,
                Outcome::NoUpstream { fetched: true },
                FlashKind::Note,
                "fetched origin · ⎇ feat tracks no remote branch yet",
            ),
            (
                pull,
                Outcome::NoUpstream { fetched: false },
                FlashKind::Note,
                "⎇ feat tracks no remote branch, and there is no origin",
            ),
            (
                pull,
                Outcome::Failed("fetch timed out".into()),
                FlashKind::Failed,
                "pull ⎇ feat: fetch timed out",
            ),
            (
                push,
                Outcome::Pushed {
                    upstream: up(),
                    commits: 2,
                },
                FlashKind::Done,
                "⎇ feat pushed 2 commits to origin/feat",
            ),
            (
                push,
                Outcome::Published { upstream: up() },
                FlashKind::Done,
                "⎇ feat published to origin/feat",
            ),
            (
                push,
                Outcome::NothingToPush {
                    upstream: up(),
                    behind: 0,
                },
                FlashKind::Note,
                "⎇ feat has nothing to push to origin/feat",
            ),
            (
                push,
                Outcome::NothingToPush {
                    upstream: up(),
                    behind: 4,
                },
                FlashKind::Note,
                "⎇ feat has nothing to push to origin/feat · ⇣4 to pull",
            ),
            (
                push,
                Outcome::ConfirmPush {
                    upstream: up(),
                    ahead: 1,
                },
                FlashKind::Note,
                "⎇ feat would push 1 commit straight to origin/feat",
            ),
            (
                push,
                Outcome::Failed("rejected: tests failed".into()),
                FlashKind::Failed,
                "push ⎇ feat: rejected: tests failed",
            ),
        ];
        for (op, outcome, kind, text) in cases {
            let flash = flash_for("⎇ feat", op, &outcome);
            assert_eq!(
                (flash.kind, flash.text.as_str()),
                (kind, text),
                "{outcome:?}"
            );
        }
    }
}
