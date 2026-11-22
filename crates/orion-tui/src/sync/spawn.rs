//! The RESOLVER's hands: each turn of the event loop, what the screen
//! shows goes in ([`step`]), and each key that came due goes out to the
//! fetch that answers it, off the loop. Every answer comes back on one
//! channel as a [`Landed`], for `event_loop::land_sync` to hand to the
//! resolver and then to the stores that keep what it says.

use std::time::Instant;

use super::{background, gate, scope, FetchError, Provider, QueryKey};
use crate::app::App;
use crate::fetch::Ticket;
use crate::pull_request::{ListAnswer, Lookup};

/// A fetch back on the loop: its ticket (which key, asked when) and what
/// it brought.
#[derive(Debug)]
pub struct Landed {
    pub ticket: Ticket<QueryKey>,
    pub answer: Answer,
}

/// What a fetch brought, by kind of key.
#[derive(Debug)]
pub enum Answer {
    /// A project's open list.
    OpenPrs {
        answer: Result<ListAnswer, FetchError>,
    },
    /// A checkout's own pull request, asked while it was on `branch`.
    /// `Ok` is an answer — found or none — never `Lookup::Unavailable`.
    BranchPr {
        branch: String,
        answer: Result<Lookup, FetchError>,
    },
    /// Every other GitHub key's answer (`sync::github`).
    GitHub(super::github::Answer),
    /// A Linear key's answer (`sync::linear`).
    Linear(super::linear::Answer),
    /// A usage or Microsoft 365 key's answer (`sync::services`).
    Service(super::services::Answer),
    /// A checkout's read or a project's fetch (`sync::local`).
    Local(Result<super::local::Answer, FetchError>),
    /// A key with nothing to ask it of — its project, checkout or Linear
    /// key gone from the tree, or a key its provider's module doesn't
    /// answer: it should never have come due.
    Unanswered,
}

impl Answer {
    /// Whether the fetch answered, for [`Resolver::land`](super::Resolver::land).
    pub fn outcome(&self) -> Result<(), &FetchError> {
        match self {
            Answer::OpenPrs { answer, .. } => answer.as_ref().map(|_| ()),
            Answer::BranchPr { answer, .. } => answer.as_ref().map(|_| ()),
            Answer::GitHub(answer) => answer.outcome(),
            Answer::Linear(answer) => answer.outcome(),
            Answer::Service(answer) => answer.outcome(),
            Answer::Local(answer) => answer.as_ref().and_then(|a| a.outcome()),
            Answer::Unanswered => Err(&FetchError::Missing),
        }
    }
}

/// One turn's worth of resolving: GitHub's pause as the gate last saw it,
/// whether anybody is looking, what the automations need (in the process
/// that owns them), what the screen shows — and every key that is due
/// spawned. Run after every turn of the event loop, which a `next_wake`
/// deadline firing is too. Hands back the fetches it started.
pub fn step(app: &mut App) -> Vec<Ticket<QueryKey>> {
    let now = Instant::now();
    app.resolver
        .observe_pause(Provider::GitHub, gate::github_paused_until());
    app.resolver.set_live(!app.idle());
    let needs = if app.sync_owner.is_some() {
        background::needs(app)
    } else {
        Vec::new()
    };
    app.resolver.set_background(needs);
    // The GIT WATCHER follows the selected checkout, and what it watches
    // needs only a backstop beat.
    crate::git_watch::follow(app, now);
    let watched = app.git_watch.watched().cloned().map(QueryKey::GitStatus);
    app.resolver.set_watched(watched);
    let seen = scope::visible(app);
    super::local::seed_fetches(app, &seen, now);
    app.resolver.visit(seen, now);
    let started = app.resolver.due(now);
    for ticket in &started {
        spawn(app, ticket.clone());
    }
    started
}

/// [`step`], keeping only what went to a service — the tickets a test of
/// GitHub, Linear or usage pacing reads. Local git's reads still start
/// (and stay in flight: a test has no channel), they just aren't listed.
#[cfg(test)]
pub fn step_remote(app: &mut App) -> Vec<Ticket<QueryKey>> {
    step(app)
        .into_iter()
        .filter(|t| t.key.provider() != Provider::Local)
        .collect()
}

/// Start the fetch `ticket` asks for, off the loop, by the provider that
/// answers it. A key whose repo or checkout is gone lands at once as
/// [`FetchError::Missing`], with no process spent finding that out.
/// Nothing at all without a channel (the unit tests, which land by hand).
pub fn spawn(app: &App, ticket: Ticket<QueryKey>) {
    let Some(tx) = app.sync_tx.clone() else {
        return;
    };
    match ticket.key.provider() {
        Provider::GitHub => super::github::spawn(app, ticket, tx),
        Provider::Linear => super::linear::spawn(app, ticket, tx),
        Provider::Usage | Provider::Microsoft => super::services::spawn(app, ticket, tx),
        Provider::Local => super::local::spawn(app, ticket, tx),
    }
}

/// Something changed what GitHub says of checkout `worktree` — a push can
/// open, move or close its pull request: its project's open list and its
/// branch lookup are both behind (`Resolver::invalidate`).
pub fn invalidate_checkout(app: &mut App, worktree: &orion_core::WorktreeId) {
    if let Some(project) = app
        .tree
        .worktrees
        .iter()
        .find(|w| &w.id == worktree)
        .map(|w| w.project_id.clone())
    {
        app.resolver.invalidate(&QueryKey::OpenPrs(project));
    }
    app.resolver
        .invalidate(&QueryKey::BranchPr(worktree.clone()));
}
