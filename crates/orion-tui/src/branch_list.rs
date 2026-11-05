//! The branches of each project's ROOT WORKTREE, for the QUICK PROMPT's
//! WORKTREE PICKER (`⌘.`): every local branch and every remote branch that
//! has no local twin, newest commit first — each a fresh worktree that
//! checks it out.
//!
//! Client-side git, like the diff viewer: a listing is one `git
//! for-each-ref` (milliseconds, painted from a per-checkout cache while the
//! fresh one lands), and a background `git fetch --all` — at most once a
//! minute per checkout — lists again when it lands. Every call runs off the
//! loop with its answer on `App::branch_list.tx`, and none is stacked on
//! one still running. Git that talks to a remote runs DETACHED, through
//! `crate::git_proc`: in a session of its own with stdin closed, so an
//! `ssh` prompt fails rather than painting over the frame.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orion_core::WorktreeId;

use crate::app::App;
use crate::git_proc::{read, remote_git, FETCH_TIMEOUT};

/// The least time between two background fetches of one checkout, so
/// opening and closing the picker never hammers a remote.
const FETCH_GAP: Duration = Duration::from_secs(60);
/// `git for-each-ref` record: NUL between fields, RS after each record (a
/// subject is one line, but a record separator costs nothing).
const REF_FORMAT: &str = "--format=%(refname)%00%(symref)%00%(HEAD)%00%(worktreepath)%00%(committerdate:unix)%00%(contents:subject)%1e";

/// One row: a local branch, or a remote-tracking branch nothing local
/// shadows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// What the row shows and the filter matches: `feature-x`, or
    /// `origin/feature-x` for a remote branch.
    pub name: String,
    pub remote: bool,
    /// The branch the root checkout is on.
    pub current: bool,
    /// The other checkout a local branch is checked out in. git keeps a
    /// branch in one checkout at a time.
    pub checked_out_at: Option<PathBuf>,
    /// Committer date of the tip, unix seconds.
    pub committed: i64,
    pub subject: String,
}

impl Branch {
    /// The local branch a checkout of it lands on: the name itself, or a
    /// remote branch's name without its remote (`origin/feature-x` →
    /// `feature-x`, what `git switch --track` creates).
    pub fn local_name(&self) -> &str {
        if self.remote {
            self.name
                .split_once('/')
                .map_or(&self.name, |(_, rest)| rest)
        } else {
            &self.name
        }
    }
}

/// Parse `git for-each-ref` output in [`REF_FORMAT`] into rows: the
/// current branch first, the other local branches next, then the remote
/// branches with no local branch of the same name. Within each group git's
/// newest-commit-first order holds. `origin/HEAD` points at a branch rather
/// than being one, and is dropped; so is a SPARE WORKTREE's branch, the
/// DAEMON's own until a launch takes it ([`orion_core::SPARE_PREFIX`]).
pub fn parse_refs(out: &str) -> Vec<Branch> {
    let mut local: Vec<Branch> = Vec::new();
    let mut remote: Vec<Branch> = Vec::new();
    for record in out.split('\x1e') {
        let record = record.trim_start_matches('\n');
        let mut fields = record.split('\0');
        let (Some(refname), Some(symref), Some(head), Some(worktree), Some(date)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let committed = date.trim().parse().unwrap_or(0);
        let subject = fields.next().unwrap_or("").trim().to_string();
        if let Some(name) = refname.strip_prefix("refs/heads/") {
            if orion_core::is_spare_branch(name) {
                continue;
            }
            let current = head == "*";
            local.push(Branch {
                name: name.to_string(),
                remote: false,
                current,
                checked_out_at: (!current && !worktree.is_empty()).then(|| worktree.into()),
                committed,
                subject,
            });
        } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
            if !symref.is_empty() || !name.contains('/') {
                continue;
            }
            remote.push(Branch {
                name: name.to_string(),
                remote: true,
                current: false,
                checked_out_at: None,
                committed,
                subject,
            });
        }
    }
    remote.retain(|r| !local.iter().any(|l| l.name == r.local_name()));
    // Stable: git's recency order survives within both halves.
    local.sort_by_key(|b| !b.current);
    local.extend(remote);
    local
}

/// Every branch of the checkout at `root`, as [`parse_refs`] orders them.
/// `Err` is a one-line message.
pub fn list_branches(root: &Path) -> Result<Vec<Branch>, String> {
    let args = [
        "for-each-ref",
        "--sort=-committerdate",
        REF_FORMAT,
        "refs/heads",
        "refs/remotes",
    ];
    read(root, &args).map(|out| parse_refs(&out))
}

/// `git fetch --all`, stopped past [`FETCH_TIMEOUT`] or once `quit` is
/// raised (the TUI is leaving, and the runtime's shutdown waits on this
/// thread). True when it finished.
pub fn fetch(root: &Path, quit: &AtomicBool) -> bool {
    remote_git(root, &["fetch", "--all", "--quiet"], FETCH_TIMEOUT, quit).is_ok()
}

/// What the event loop hands back to [`land_answer`].
#[derive(Debug)]
pub enum Answer {
    Listed {
        worktree: WorktreeId,
        list: Result<Vec<Branch>, String>,
    },
    Fetched {
        worktree: WorktreeId,
        ok: bool,
    },
}

/// The listings, on `App::branch_list`.
#[derive(Debug, Default)]
pub struct Shared {
    /// Where the off-loop git answers go; installed at startup. `None` in
    /// the unit tests, which then list inline and never fetch.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    /// Each checkout's last listing, so the picker paints at once while the
    /// fresh one lands underneath.
    pub lists: HashMap<WorktreeId, Vec<Branch>>,
    /// Checkouts with a listing in flight — never two at once — and the
    /// ones asked for again meanwhile, listed once the first lands.
    pub listing: HashSet<WorktreeId>,
    pub relist: HashSet<WorktreeId>,
    /// Checkouts with a background fetch running, and when each last
    /// started one ([`FETCH_GAP`]).
    pub fetching: HashSet<WorktreeId>,
    pub fetched: HashMap<WorktreeId, Instant>,
    /// Raised when the TUI goes away, so a fetch still running is stopped
    /// rather than holding the runtime's shutdown for its whole budget.
    pub quit: Arc<AtomicBool>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

/// Bring `worktree`'s branch listing up to date — the WORKTREE PICKER
/// lists the root's branches from [`Shared::lists`]: a fresh listing off
/// the loop, and a background fetch when the last is a minute old, which
/// lists again when it lands.
pub(crate) fn warm(app: &mut App, worktree: &WorktreeId) {
    let Some(root) = checkout_path(app, worktree) else {
        return;
    };
    request_list(app, worktree.clone(), root.clone());
    request_fetch(app, worktree.clone(), root);
}

fn checkout_path(app: &App, worktree: &WorktreeId) -> Option<PathBuf> {
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.id == worktree)
        .map(|w| w.path.clone())
}

/// List the branches, off the loop (inline in the unit tests). One request
/// per checkout at a time; asking again while one runs lists once more
/// when it lands. A checkout that isn't on disk is said so without a
/// process.
fn request_list(app: &mut App, worktree: WorktreeId, root: PathBuf) {
    if !root.is_dir() {
        let list = Err(format!("{} is not on disk", root.display()));
        land_answer(app, Answer::Listed { worktree, list });
        return;
    }
    let Some(tx) = app.branch_list.tx.clone() else {
        let list = list_branches(&root);
        land_answer(app, Answer::Listed { worktree, list });
        return;
    };
    if !app.branch_list.listing.insert(worktree.clone()) {
        app.branch_list.relist.insert(worktree);
        return;
    }
    tokio::task::spawn_blocking(move || {
        let list = list_branches(&root);
        let _ = tx.send(Answer::Listed { worktree, list });
    });
}

/// Refresh the remotes in the background: skipped while one runs, within
/// [`FETCH_GAP`] of the last, and in the unit tests.
fn request_fetch(app: &mut App, worktree: WorktreeId, root: PathBuf) {
    let Some(tx) = app.branch_list.tx.clone() else {
        return;
    };
    let shared = &mut app.branch_list;
    let recent = shared
        .fetched
        .get(&worktree)
        .is_some_and(|at| at.elapsed() < FETCH_GAP);
    if !root.is_dir() || recent || shared.fetching.contains(&worktree) {
        return;
    }
    shared.fetching.insert(worktree.clone());
    shared.fetched.insert(worktree.clone(), Instant::now());
    let quit = shared.quit.clone();
    tokio::task::spawn_blocking(move || {
        let ok = fetch(&root, &quit);
        let _ = tx.send(Answer::Fetched { worktree, ok });
    });
}

/// A git answer landed.
pub(crate) fn land_answer(app: &mut App, answer: Answer) {
    match answer {
        Answer::Listed { worktree, list } => {
            app.branch_list.listing.remove(&worktree);
            if let Ok(list) = list {
                app.branch_list.lists.insert(worktree.clone(), list);
            }
            if app.branch_list.relist.remove(&worktree) {
                if let Some(root) = checkout_path(app, &worktree) {
                    request_list(app, worktree, root);
                }
            }
        }
        Answer::Fetched { worktree, ok } => {
            app.branch_list.fetching.remove(&worktree);
            if let (true, Some(root)) = (ok, checkout_path(app, &worktree)) {
                request_list(app, worktree, root);
            }
        }
    }
    app.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A repo with `main` and `feature`, on `main`.
    fn repo(dir: &tempfile::TempDir) -> PathBuf {
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("a.txt"), "main\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        git(&repo, &["switch", "-q", "-c", "feature"]);
        std::fs::write(repo.join("a.txt"), "feature\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "feature work"]);
        git(&repo, &["switch", "-q", "main"]);
        repo
    }

    fn record(
        refname: &str,
        symref: &str,
        head: &str,
        wt: &str,
        date: i64,
        subject: &str,
    ) -> String {
        format!("{refname}\0{symref}\0{head}\0{wt}\0{date}\0{subject}\x1e\n")
    }

    #[test]
    fn refs_parse_current_first_then_locals_then_remotes_nothing_local_shadows() {
        let out = [
            record("refs/heads/newest", "", " ", "", 30, "newest work"),
            record("refs/heads/main", "", "*", "/repo", 20, "on main"),
            record(
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
                " ",
                "",
                20,
                "",
            ),
            record("refs/remotes/origin/main", "", " ", "", 20, "on main"),
            record(
                "refs/heads/held",
                "",
                " ",
                "/repo-worktrees/held",
                10,
                "held",
            ),
            record(
                "refs/remotes/origin/only-remote",
                "",
                " ",
                "",
                5,
                "far away",
            ),
            record("refs/remotes/upstream/feat/deep", "", " ", "", 4, "nested"),
            record(
                "refs/heads/orion-spare-3kq9xa",
                "",
                " ",
                "/repo-worktrees/orion-spare-3kq9xa",
                3,
                "on main",
            ),
        ]
        .concat();
        let rows = parse_refs(&out);
        let names: Vec<&str> = rows.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "main",
                "newest",
                "held",
                "origin/only-remote",
                "upstream/feat/deep"
            ],
            "origin/HEAD is a pointer, origin/main has a local twin, and the spare is the daemon's"
        );
        assert!(rows[0].current && rows[0].checked_out_at.is_none());
        assert_eq!(rows[2].checked_out_at, Some("/repo-worktrees/held".into()));
        assert!(rows[3].remote && rows[3].local_name() == "only-remote");
        assert_eq!(rows[4].local_name(), "feat/deep");
        assert_eq!(rows[1].subject, "newest work");
        assert_eq!(rows[1].committed, 30);
    }

    #[test]
    fn a_real_repo_lists_its_branches_its_remotes_and_the_ones_held_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let origin = repo(&dir);
        git(&origin, &["branch", "only-on-origin"]);
        let clone = dir.path().join("clone");
        git(
            dir.path(),
            &["clone", "-q", origin.to_str().unwrap(), "clone"],
        );
        git(&clone, &["branch", "held"]);
        git(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                dir.path().join("wt").to_str().unwrap(),
                "held",
            ],
        );
        let rows = list_branches(&clone).unwrap();
        let names: Vec<&str> = rows.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names[0], "main", "the current branch leads: {names:?}");
        assert!(rows[0].current);
        assert!(names.contains(&"origin/feature"), "{names:?}");
        assert!(names.contains(&"origin/only-on-origin"), "{names:?}");
        assert!(!names.contains(&"origin/main"), "main is local: {names:?}");
        assert!(!names.iter().any(|n| n.ends_with("/HEAD")), "{names:?}");
        let held = rows.iter().find(|b| b.name == "held").unwrap();
        assert!(held.checked_out_at.is_some(), "{held:?}");
    }

    #[test]
    fn a_fetch_with_no_remotes_finishes_and_a_raised_quit_stops_one() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo(&dir);
        assert!(fetch(&repo, &AtomicBool::new(false)));
        // Whatever the fetch was doing, a raised flag ends the wait at once.
        let started = Instant::now();
        let _ = fetch(&repo, &AtomicBool::new(true));
        assert!(started.elapsed() < crate::git_proc::STOP_GRACE + Duration::from_secs(1));
    }
}
