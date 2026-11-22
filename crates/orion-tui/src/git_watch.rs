//! THE GIT WATCHER: the selected checkout's files and git, watched
//! (FSEvents on macOS, through `notify`), so its badge and band are read
//! again when something changed there — an agent's edit, a commit, a
//! checkout, a fetch — and not on a clock.
//!
//! It watches the checkout itself, recursively, and the git files that
//! say what it is: its HEAD and index, and the repo's refs and
//! `packed-refs` (for a linked worktree, under the main repo's `.git`:
//! `git_refs::dirs`). What can't change a checkout's status is let go of
//! in the callback, before it ever reaches the loop: git's objects and
//! logs, and the build and dependency dirs that churn during a build
//! ([`HEAVY`]) — nearly always ignored by git anyway.
//!
//! A change sends the checkout's id to the loop once (a flag keeps a
//! burst from queueing more), and the loop reads it after a
//! debounce (`sync::local::watch_delay`): `resolver.invalidate(GitStatus)`.
//! Moving the selection drops the old watch and starts the new one. A
//! watch that can't start is logged once per checkout, and the
//! RESOLVER's quicker unwatched beat (`policy`'s `live_every`) is what
//! keeps the badge current there.
//!
//! It also keeps the selection's REST ([`Rest`]): what THE RESOLVER asks
//! because of the selection — the checkout's read and branch lookup, the
//! project's lists and fetch — waits until the selection has stayed put
//! [`FOLLOW_REST`], so holding `j` past a dozen checkouts asks nothing for
//! the ones passed.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use notify::{RecursiveMode, Watcher};
use orion_core::{ProjectId, WorktreeId};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::App;
use crate::git_refs::GitDirs;

/// Directories anywhere in a checkout whose changes are never worth a
/// read: what builds, package managers and test runners write by the
/// thousand. Git ignores them in practice; a tracked file under one is
/// still caught by the safety net.
pub const HEAVY: &[&str] = &[
    "node_modules",
    "target",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".cache",
    ".parcel-cache",
    "dist",
    "build",
    "coverage",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".gradle",
    ".DS_Store",
];

/// How long FSEvents coalesces in the kernel before it wakes the
/// callback: a build's thousands of writes arrive as a few.
const FSEVENT_LATENCY: std::time::Duration = std::time::Duration::from_millis(250);

/// Which changed paths say a checkout's status may have moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relevance {
    root: PathBuf,
    dirs: Option<GitDirs>,
    /// The git files that matter, joined once: the callback asks of every
    /// path in every event.
    git: Option<GitPaths>,
}

/// A checkout's git files that say its status moved.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GitPaths {
    head: PathBuf,
    index: PathBuf,
    packed_refs: PathBuf,
    refs: PathBuf,
}

impl Relevance {
    /// What matters for the checkout at `root`, whose git is `dirs`.
    pub fn new(root: PathBuf, dirs: Option<GitDirs>) -> Self {
        let git = dirs.as_ref().map(|d| GitPaths {
            head: d.gitdir.join("HEAD"),
            index: d.gitdir.join("index"),
            packed_refs: d.common.join("packed-refs"),
            refs: d.common.join("refs"),
        });
        Relevance { root, dirs, git }
    }

    /// Whether a change at `path` may move the checkout's status: HEAD,
    /// the index, a ref; or any file of the checkout outside git and the
    /// [`HEAVY`] dirs.
    pub fn matters(&self, path: &Path) -> bool {
        if let (Some(d), Some(g)) = (&self.dirs, &self.git) {
            if path == g.head
                || path == g.index
                || path == g.packed_refs
                || path.starts_with(&g.refs)
            {
                return true;
            }
            if path.starts_with(&d.gitdir) || path.starts_with(&d.common) {
                return false;
            }
        }
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        !rel.components().any(|c| match c {
            Component::Normal(name) => name == ".git" || HEAVY.iter().any(|h| name == *h),
            _ => false,
        })
    }

    /// The paths to watch for it: the checkout, recursively; and for a
    /// linked worktree, whose git lives elsewhere, its own git dir (HEAD,
    /// index), the repo's refs, and the repo's git dir itself for
    /// `packed-refs`.
    fn watches(&self) -> Vec<(PathBuf, RecursiveMode)> {
        let mut paths = vec![(self.root.clone(), RecursiveMode::Recursive)];
        if let Some(d) = &self.dirs {
            if !d.gitdir.starts_with(&self.root) {
                paths.push((d.gitdir.clone(), RecursiveMode::NonRecursive));
            }
            if !d.common.starts_with(&self.root) {
                paths.push((d.common.join("refs"), RecursiveMode::Recursive));
                paths.push((d.common.clone(), RecursiveMode::NonRecursive));
            }
        }
        paths
    }
}

/// One checkout under watch. Dropping it stops the watch.
pub struct Watch {
    pub worktree: WorktreeId,
    /// A change was sent and the loop hasn't read it yet: further ones
    /// send nothing until it has ([`Watch::taken`]).
    pending: Arc<AtomicBool>,
    _watcher: notify::RecommendedWatcher,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch")
            .field("worktree", &self.worktree)
            .finish_non_exhaustive()
    }
}

impl Watch {
    /// Watch the checkout `worktree` at `root`; each change that
    /// [`Relevance::matters`] sends its id on `tx`, once until
    /// [`taken`](Self::taken). Err says why the watch couldn't start.
    pub fn start(
        worktree: WorktreeId,
        root: &Path,
        tx: UnboundedSender<WorktreeId>,
    ) -> Result<Watch, String> {
        // Events carry the paths as the kernel has them: `/private/var`,
        // never the `/var` a symlink says.
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let dirs = crate::git_refs::dirs(&root).map(|d| GitDirs {
            gitdir: d.gitdir.canonicalize().unwrap_or(d.gitdir),
            common: d.common.canonicalize().unwrap_or(d.common),
        });
        let relevance = Relevance::new(root, dirs);
        let pending = Arc::new(AtomicBool::new(false));
        let flag = pending.clone();
        let id = worktree.clone();
        let filter = relevance.clone();
        let handler = move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else {
                return;
            };
            // One sent and not yet taken: nothing to do, so no path is
            // looked at — a build's events cost a load each.
            if matches!(event.kind, notify::EventKind::Access(_)) || flag.load(Ordering::SeqCst) {
                return;
            }
            if event.paths.iter().any(|p| filter.matters(p)) && !flag.swap(true, Ordering::SeqCst) {
                let _ = tx.send(id.clone());
            }
        };
        let config = notify::Config::default().with_fsevent_latency(FSEVENT_LATENCY);
        let mut watcher =
            notify::RecommendedWatcher::new(handler, config).map_err(|e| e.to_string())?;
        let ops = relevance
            .watches()
            .into_iter()
            .filter(|(path, _)| path.exists())
            .map(|(path, mode)| notify::PathOp::Watch(path, notify::WatchPathConfig::new(mode)))
            .collect();
        watcher.update_paths(ops).map_err(|e| e.to_string())?;
        Ok(Watch {
            worktree,
            pending,
            _watcher: watcher,
        })
    }

    /// The loop has read the change it was sent: the next one sends again.
    pub fn taken(&self) {
        self.pending.store(false, Ordering::SeqCst);
    }
}

/// The watcher's state on `App::git_watch`.
#[derive(Debug, Default)]
pub struct Watching {
    /// Where changes go; installed by the event loop. None in the unit
    /// tests: no watch starts, and the safety net is all there is.
    pub tx: Option<UnboundedSender<WorktreeId>>,
    /// The selected checkout's watch.
    pub current: Option<Watch>,
    /// The checkout whose watch couldn't start, so it isn't tried again
    /// every turn — only once it is selected anew.
    pub failed: Option<WorktreeId>,
    /// The checkouts a change was seen in, waiting out the debounce (or,
    /// while AWAY, for you to be back), and when they are read.
    pub changed: std::collections::BTreeSet<WorktreeId>,
    pub due: Option<std::time::Instant>,
    /// The checkout just selected, and when its watch starts: once the
    /// selection has rested [`FOLLOW_REST`], so walking the list with `j`
    /// starts no watch for every checkout passed.
    pub following: Option<(WorktreeId, PathBuf, std::time::Instant)>,
    /// Where the selection is and whether it has rested there.
    pub rest: Rest,
}

impl Watching {
    /// The checkout whose watch is running: every change there is said.
    pub fn watched(&self) -> Option<&WorktreeId> {
        self.current.as_ref().map(|w| &w.worktree)
    }
}

/// How long the selection rests on a checkout before it is watched, and
/// before THE RESOLVER asks what the selection shows.
pub const FOLLOW_REST: std::time::Duration = std::time::Duration::from_millis(300);

/// The selection's REST: the project and checkout selected, and since
/// when. The first selection counts as rested — nothing was passed to
/// get there — and each move after starts the clock again.
#[derive(Debug, Default)]
pub struct Rest {
    at: Option<(Option<ProjectId>, Option<WorktreeId>)>,
    /// When the selection last moved; None once it has rested.
    moved: Option<std::time::Instant>,
}

impl Rest {
    /// Note the selection as `project` / `worktree` at `now`.
    pub fn note(
        &mut self,
        project: Option<ProjectId>,
        worktree: Option<WorktreeId>,
        now: std::time::Instant,
    ) {
        let here = Some((project, worktree));
        if self.at != here {
            if self.at.is_some() {
                self.moved = Some(now);
            }
            self.at = here;
        }
        if self.moved.is_some_and(|at| now >= at + FOLLOW_REST) {
            self.moved = None;
        }
    }

    /// Whether the selection has stayed put long enough to be asked about.
    pub fn rested(&self) -> bool {
        self.moved.is_none()
    }

    /// When the selection will have rested, while it hasn't.
    pub fn due(&self) -> Option<std::time::Instant> {
        self.moved.map(|at| at + FOLLOW_REST)
    }
}

/// Note where the selection is ([`Rest::note`]).
fn note_rest(app: &mut App, now: std::time::Instant) {
    let project = app.selected_project().map(|p| p.id.clone());
    let worktree = app.selected_worktree().map(|w| w.id.clone());
    app.git_watch.rest.note(project, worktree, now);
}

/// Follow the selection: watch the selected checkout, and only it — the
/// old watch dropped at once, the new one started once the selection has
/// rested ([`FOLLOW_REST`], `start_due`). Run on every turn of the loop;
/// nothing happens unless the selection moved. The selection's REST is
/// kept here too, watch or none.
pub fn follow(app: &mut App, now: std::time::Instant) {
    note_rest(app, now);
    if app.git_watch.tx.is_none() {
        return;
    }
    let wanted = app
        .selected_worktree()
        .filter(|w| !app.is_placeholder_worktree(&w.id))
        .map(|w| (w.id.clone(), w.path.clone()));
    let wanted_id = wanted.as_ref().map(|(id, _)| id);
    if app.git_watch.current.as_ref().map(|w| &w.worktree) == wanted_id {
        app.git_watch.following = None;
        return;
    }
    app.git_watch.current = None;
    let Some((id, path)) = wanted else {
        app.git_watch.following = None;
        return;
    };
    let already = app.git_watch.following.as_ref().map(|(f, _, _)| f) == Some(&id);
    if !already && app.git_watch.failed.as_ref() != Some(&id) {
        app.git_watch.following = Some((id, path, now + FOLLOW_REST));
    }
}

/// The selection has rested: start watching it. A watch that can't start
/// is said once, and the checkout is left to the safety net until it is
/// selected anew.
fn start_due(app: &mut App, now: std::time::Instant) {
    let Some(tx) = app.git_watch.tx.clone() else {
        return;
    };
    let Some((id, path, at)) = app.git_watch.following.clone() else {
        return;
    };
    if at > now {
        return;
    }
    app.git_watch.following = None;
    match Watch::start(id.clone(), &path, tx) {
        Ok(watch) => {
            app.git_watch.current = Some(watch);
            app.git_watch.failed = None;
        }
        Err(why) => {
            let every = crate::sync::policy::policy(&crate::sync::QueryKey::GitStatus(id.clone()))
                .live_every
                .map_or_else(
                    || "on visits".to_string(),
                    |d| format!("every {} s", d.as_secs()),
                );
            tracing::warn!(worktree = %id.0, "can't watch the checkout, reading it {every} instead: {why}");
            app.git_watch.failed = Some(id);
        }
    }
}

/// The watcher saw `worktree` change: read it after the debounce, which
/// a burst doesn't push back.
pub fn changed(app: &mut App, worktree: WorktreeId, now: std::time::Instant) {
    app.git_watch.changed.insert(worktree);
    if let Some(delay) = crate::sync::local::watch_delay(app) {
        let at = now + delay;
        app.git_watch.due = Some(app.git_watch.due.map_or(at, |due| due.min(at)));
    }
}

/// The watcher's next deadline: the changes waiting are due to be read
/// (none while AWAY: they wait for the CATCH-UP), or the selection has
/// rested long enough to be watched and asked about.
pub fn due(app: &App) -> Option<std::time::Instant> {
    let changes = app
        .git_watch
        .due
        .filter(|_| !app.git_watch.changed.is_empty());
    let follow = app.git_watch.following.as_ref().map(|(_, _, at)| *at);
    let rest = app.git_watch.rest.due();
    changes.into_iter().chain(follow).chain(rest).min()
}

/// The watcher's deadline came: the selection that rested is watched, and
/// once the debounce is out every checkout a change was seen in is read
/// again (`Resolver::invalidate`) and its watch may send the next one.
pub fn on_deadline(app: &mut App, now: std::time::Instant) {
    start_due(app, now);
    if app.git_watch.due.is_some_and(|at| at <= now) {
        flush(app);
    }
}

/// Every checkout a change was seen in, read again.
fn flush(app: &mut App) {
    app.git_watch.due = None;
    for worktree in std::mem::take(&mut app.git_watch.changed) {
        app.resolver
            .invalidate(&crate::sync::QueryKey::GitStatus(worktree.clone()));
        if let Some(watch) = app
            .git_watch
            .current
            .as_ref()
            .filter(|w| w.worktree == worktree)
        {
            watch.taken();
        }
    }
}

/// Somebody is looking again: what the watcher held while AWAY (or
/// spaced out while unfocused) is read now.
pub fn catch_up(app: &mut App, now: std::time::Instant) {
    if !app.git_watch.changed.is_empty() {
        app.git_watch.due = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked() -> Relevance {
        Relevance::new(
            PathBuf::from("/w/feat"),
            Some(GitDirs {
                gitdir: PathBuf::from("/r/.git/worktrees/feat"),
                common: PathBuf::from("/r/.git"),
            }),
        )
    }

    #[test]
    fn edits_and_refs_matter_and_builds_and_objects_dont() {
        let r = linked();
        assert!(r.matters(Path::new("/w/feat/src/main.rs")));
        assert!(r.matters(Path::new("/w/feat/README.md")));
        assert!(r.matters(Path::new("/r/.git/worktrees/feat/HEAD")));
        assert!(r.matters(Path::new("/r/.git/worktrees/feat/index")));
        assert!(r.matters(Path::new("/r/.git/refs/remotes/origin/main")));
        assert!(r.matters(Path::new("/r/.git/packed-refs")));
        assert!(!r.matters(Path::new("/w/feat/target/debug/orion")));
        assert!(!r.matters(Path::new("/w/feat/web/node_modules/x/index.js")));
        assert!(!r.matters(Path::new("/w/feat/.DS_Store")));
        assert!(!r.matters(Path::new("/r/.git/objects/ab/cdef")));
        assert!(!r.matters(Path::new("/r/.git/worktrees/feat/logs/HEAD")));
        assert!(!r.matters(Path::new("/r/.git/worktrees/other/HEAD")));
        assert!(!r.matters(Path::new("/r/.git/index")), "the root's index");
        assert!(!r.matters(Path::new("/elsewhere/file")));
    }

    /// The ROOT WORKTREE's git is inside it: one recursive watch covers
    /// it, and its own `.git` internals still don't count.
    #[test]
    fn the_root_worktree_is_one_watch() {
        let r = Relevance::new(
            PathBuf::from("/r"),
            Some(GitDirs {
                gitdir: PathBuf::from("/r/.git"),
                common: PathBuf::from("/r/.git"),
            }),
        );
        assert_eq!(r.watches().len(), 1);
        assert!(r.matters(Path::new("/r/.git/HEAD")));
        assert!(!r.matters(Path::new("/r/.git/objects/pack/x")));
        assert!(!r.matters(Path::new("/r/.git/FETCH_HEAD")));
        assert_eq!(linked().watches().len(), 4);
    }

    /// A real watch on a real checkout: an edit sends the id once, and a
    /// second edit before the loop has taken it sends nothing more.
    #[test]
    fn an_edit_in_a_watched_checkout_reaches_the_loop_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(ok.success());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let id = WorktreeId("w".into());
        let watch = Watch::start(id.clone(), root, tx).expect("watch starts");
        // FSEvents can take a moment to start delivering.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut n = 0;
        let got = loop {
            std::fs::write(root.join("file.txt"), format!("{n}")).unwrap();
            n += 1;
            std::thread::sleep(std::time::Duration::from_millis(200));
            if let Ok(got) = rx.try_recv() {
                break Some(got);
            }
            if std::time::Instant::now() > deadline {
                break None;
            }
        };
        assert_eq!(got, Some(id));
        std::fs::write(root.join("file.txt"), "again").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(rx.try_recv().is_err(), "held until taken");
        watch.taken();
    }

    /// The first selection has rested already; a move starts the clock,
    /// and the selection rests once it has stayed put long enough —
    /// however many moves came before.
    #[test]
    fn the_selection_rests_once_it_stays_put() {
        let t0 = std::time::Instant::now();
        let w = |n: &str| Some(WorktreeId(n.into()));
        let p = Some(ProjectId("p".into()));
        let mut rest = Rest::default();
        rest.note(p.clone(), w("a"), t0);
        assert!(rest.rested(), "nothing was passed to get here");
        let ms = std::time::Duration::from_millis;
        rest.note(p.clone(), w("b"), t0 + ms(100));
        rest.note(p.clone(), w("c"), t0 + ms(200));
        assert!(!rest.rested());
        assert_eq!(rest.due(), Some(t0 + ms(200) + FOLLOW_REST));
        rest.note(p.clone(), w("c"), t0 + ms(300));
        assert!(!rest.rested(), "not long enough");
        rest.note(p.clone(), w("c"), t0 + ms(200) + FOLLOW_REST);
        assert!(rest.rested());
        assert_eq!(rest.due(), None);
        rest.note(Some(ProjectId("q".into())), None, t0 + ms(900));
        assert!(!rest.rested(), "a project move counts too");
    }

    /// A change is read after the debounce, however many follow it, and
    /// held while AWAY until you are back.
    #[test]
    fn changes_wait_out_the_debounce() {
        let mut app = App::new();
        app.window_focused = true;
        let t0 = std::time::Instant::now();
        let w1 = WorktreeId("w1".into());
        changed(&mut app, w1.clone(), t0);
        changed(
            &mut app,
            w1.clone(),
            t0 + std::time::Duration::from_millis(400),
        );
        let at = t0 + crate::sync::local::WATCH_DEBOUNCE;
        assert_eq!(due(&app), Some(at), "a burst is one read");
        on_deadline(&mut app, at);
        assert!(app.git_watch.changed.is_empty());
        assert_eq!(due(&app), None);
        app.away = true;
        changed(&mut app, w1.clone(), t0);
        assert_eq!(due(&app), None, "away: held");
        app.away = false;
        catch_up(&mut app, at);
        assert_eq!(due(&app), Some(at), "back: read now");
    }
}
