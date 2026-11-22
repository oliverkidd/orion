//! Local git through the RESOLVER: what a checkout's badge and band print,
//! and a project's fetch of origin. Never rate limited, but paced all the
//! same — a `git status` on a big checkout is tens of milliseconds and a
//! handful of processes, and a fetch is the network.
//!
//! | Key | Asked | Lands in |
//! |---|---|---|
//! | `GitStatus(selected checkout)` | once the selection rests on it (`git_watch::Rest`); when the GIT WATCHER sees it change (`git_watch`); while somebody is looking, a backstop every 5 min under the watcher, every 15 s where it couldn't start | the badge, its band, the DIFF VIEWER's file list (`event_loop::land_git_status`) |
//! | `GitStatus(each band)` | as the band comes on screen, when its last read is a minute old; when an agent's turn in it ends | the band's rule |
//! | `BaseFetch(selected project)` | as the project is switched to and rested on, or the window takes focus back on it, once its last fetch — this run's, or git's own `FETCH_HEAD` ([`seed_fetches`]) — is `base_fetch` old (30 min); at once after a merge (`base_sync::request_now`); never while AWAY | the root's fast-forward, every checkout's `⇣` (`base_sync::land`) |
//!
//! Neither runs on a beat of its own with nobody looking: an unfocused or
//! AWAY orion reads a checkout only when the watcher says it changed (and
//! then at most every half minute, or not until you are back), and fetches
//! nothing.

use std::time::{Duration, Instant, SystemTime};

use orion_core::{ProjectId, WorktreeId};
use tokio::sync::mpsc::UnboundedSender;

use super::spawn::{Answer as Landing, Landed};
use super::{FetchError, QueryKey, Seen};
use crate::app::App;
use crate::fetch::Ticket;

/// What a local fetch brought.
#[derive(Debug)]
pub enum Answer {
    /// A checkout's read. `files` None: git couldn't say.
    Git(crate::checkout_read::Read),
    /// A project's fetch, and what it did with the root.
    Base(crate::base_sync::Outcome),
}

impl Answer {
    /// Whether the fetch answered: a read git couldn't make is a failure,
    /// backed off; a fetch that couldn't reach origin is an answer — a
    /// laptop offline is not news, and the next visit past `base_fetch`
    /// tries again.
    pub fn outcome(&self) -> Result<(), &FetchError> {
        static UNREAD: FetchError = FetchError::Missing;
        match self {
            Answer::Git(read) if read.files.is_none() => Err(&UNREAD),
            Answer::Git(_) | Answer::Base(_) => Ok(()),
        }
    }
}

/// What the screen shows of local git: the selected checkout (the thing
/// you are in: a live safety-net beat), every other band the grid draws,
/// and the selected project's fetch — not while AWAY, and not with
/// `base_fetch` off.
pub fn visible(app: &App) -> Vec<(QueryKey, Seen)> {
    let mut seen = Vec::new();
    let selected = super::scope::rested_worktree(app).map(|w| w.id.clone());
    if let Some(id) = &selected {
        if !app.is_placeholder_worktree(id) {
            seen.push((QueryKey::GitStatus(id.clone()), Seen::Focused));
        }
    }
    if app.launcher_grid() {
        let current = app.selected_worktree().map(|w| &w.id);
        for w in app.visible_worktrees() {
            if Some(&w.id) != current && !app.is_placeholder_worktree(&w.id) {
                seen.push((QueryKey::GitStatus(w.id.clone()), Seen::Shown));
            }
        }
    }
    if let Some(project) = super::scope::rested_project(app) {
        if !app.away && crate::base_sync::fresh_for().is_some() {
            seen.push((QueryKey::BaseFetch(project.id.clone()), Seen::Shown));
        }
    }
    seen
}

/// What git itself knows of each project fetch coming on screen that
/// this run hasn't asked: the root's last fetch, by its `FETCH_HEAD`
/// (`git_refs::fetched_at`) — so a launch, or a project first visited,
/// fetches only when the last fetch, whoever ran it, is `base_fetch` old.
pub fn seed_fetches(app: &mut App, seen: &[(QueryKey, Seen)], now: Instant) {
    for (key, _) in seen {
        let QueryKey::BaseFetch(project) = key else {
            continue;
        };
        if app.resolver.known(key) {
            continue;
        }
        if let Some(at) = last_fetch(app, project, now) {
            app.resolver.seed(key, at);
        }
    }
}

/// When `project`'s root last fetched, on the loop's clock.
fn last_fetch(app: &App, project: &ProjectId, now: Instant) -> Option<Instant> {
    let root = app.tree.projects.iter().find(|p| &p.id == project)?;
    let at = crate::git_refs::fetched_at(&crate::git_refs::dirs(&root.repo_path)?)?;
    let age = SystemTime::now().duration_since(at).unwrap_or_default();
    now.checked_sub(age)
}

/// Start the read or fetch `ticket` asks for, on the blocking pool. A
/// checkout or project that is gone lands at once as
/// [`FetchError::Missing`]; a root another git has hold of, as a failure
/// that backs off.
pub fn spawn(app: &App, ticket: Ticket<QueryKey>, tx: UnboundedSender<Landed>) {
    let send = move |ticket, answer| {
        let _ = tx.send(Landed {
            ticket,
            answer: Landing::Local(answer),
        });
    };
    match ticket.key.clone() {
        QueryKey::GitStatus(worktree) => {
            let Some(path) = checkout_path(app, &worktree) else {
                return send(ticket, Err(FetchError::Missing));
            };
            let repo = app.tree.repo_of(&path);
            tokio::task::spawn_blocking(move || {
                let read = crate::checkout_read::read(&path, repo.as_deref());
                send(ticket, Ok(Answer::Git(read)));
            });
        }
        QueryKey::BaseFetch(project) => match crate::base_sync::start(app, &project) {
            Ok((root, base, may_move)) => {
                let quit = app.git_sync.quit.clone();
                tokio::task::spawn_blocking(move || {
                    let outcome = crate::base_sync::sync(&root, &base, may_move, &quit);
                    send(ticket, Ok(Answer::Base(outcome)));
                });
            }
            Err(why) => send(ticket, Err(FetchError::Failed(why))),
        },
        other => {
            tracing::debug!(key = ?other, "not a local key");
            send(ticket, Err(FetchError::Missing));
        }
    }
}

/// Checkout `worktree`'s path, while it is on disk.
fn checkout_path(app: &App, worktree: &WorktreeId) -> Option<std::path::PathBuf> {
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.id == worktree)
        .map(|w| w.path.clone())
        .filter(|path| path.is_dir())
}

/// The GIT WATCHER's debounce: a burst of saves is one read, half a
/// second after the first of them, while somebody is looking.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);

/// The same while the window is in the background: the badge may be on a
/// screen you glance at, but a working agent's every save is not worth a
/// read.
pub const WATCH_DEBOUNCE_UNFOCUSED: Duration = Duration::from_secs(30);

/// How long a change the GIT WATCHER saw waits before its checkout is
/// read: half a second while somebody looks, half a minute while the
/// window is unfocused, and until you are back while AWAY (None: held for
/// the catch-up).
pub fn watch_delay(app: &App) -> Option<Duration> {
    if app.away {
        None
    } else if app.idle() {
        Some(WATCH_DEBOUNCE_UNFOCUSED)
    } else {
        Some(WATCH_DEBOUNCE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_read_backs_off_and_an_offline_fetch_does_not() {
        let unread = Answer::Git(crate::checkout_read::Read::default());
        assert!(unread.outcome().is_err());
        let read = Answer::Git(crate::checkout_read::Read {
            files: Some(Vec::new()),
            ..Default::default()
        });
        assert!(read.outcome().is_ok());
        let offline = Answer::Base(crate::base_sync::Outcome::Skipped("offline".into()));
        assert!(offline.outcome().is_ok());
    }

    /// Away, the watcher's changes wait for you; unfocused, they are read
    /// half a minute apart; looked at, at once.
    #[test]
    fn the_watchers_debounce_follows_who_is_looking() {
        let mut app = App::new();
        app.window_focused = true;
        assert_eq!(watch_delay(&app), Some(WATCH_DEBOUNCE));
        app.window_focused = false;
        assert_eq!(watch_delay(&app), Some(WATCH_DEBOUNCE_UNFOCUSED));
        app.away = true;
        assert_eq!(watch_delay(&app), None);
    }
}
