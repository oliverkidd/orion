//! NEEDS YOU: everything waiting on you, in one board on ⌘K — agents
//! waiting or crashed, Linear issues, code reviews asked of you, Slack
//! messages and meetings.
//!
//! [`model`] is what the board is made of: the cards each source offers,
//! the column and section each sits in, and what Done and Snooze hide.
//! The sources are [`agents`], [`linear`], [`reviews`], [`slack`] and
//! [`meetings`]; [`board`] lays
//! the cards out, walks them and draws them. [`State`] is what the app
//! keeps for it between frames: the store, what the last polls heard,
//! and why each session waits.
//!
//! The POLL ([`poll`]) asks Linear for your notifications and `gh` for
//! the reviews asked of you on a slow beat ([`interval`]), and as the
//! board opens when the last ask is a while ago. `ORION_NEEDS_YOU_POLL_SECS=0`
//! turns it off, as the e2e tests and the screenshots do; a section whose
//! row is off in Settings › Connections › GITHUB AND LINEAR asks nothing.

pub mod agents;
pub mod board;
pub mod linear;
pub mod meetings;
pub mod model;
pub mod reviews;
pub mod slack;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use orion_core::{AgentId, ProjectId};

use crate::app::App;
use model::{Card, CardSource, Store};

/// The poll's beat: the reviews search runs every five minutes.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Opening the board asks again when the last ask is older than this.
const STALE: Duration = Duration::from_secs(60);

/// The poll's cadence: `ORION_NEEDS_YOU_POLL_SECS` when set, `None` when
/// it is `0` (off); else [`DEFAULT_INTERVAL`].
pub fn interval() -> Option<Duration> {
    orion_core::env::secs_override(orion_core::env::NEEDS_YOU_POLL_SECS, DEFAULT_INTERVAL)
}

/// What a poll lands on the loop.
#[derive(Debug)]
pub enum Answer {
    /// The unread notifications under the key `project` reads (None:
    /// couldn't ask, which keeps what was heard before).
    Linear {
        project: ProjectId,
        notes: Option<Vec<linear::Note>>,
    },
    /// The open pull requests asking for your review.
    Reviews(Option<Vec<reviews::Found>>),
    /// A Slack name or link asked for (`slack::Lookup`).
    Slack(slack::Looked),
}

/// What the app keeps for the board.
#[derive(Debug, Default)]
pub struct State {
    /// Done and snoozed cards (`<data dir>/needs-you.json`).
    pub store: Store,
    /// Unread Linear notifications, under the first project with each key.
    pub linear: HashMap<ProjectId, Vec<linear::Note>>,
    /// What the last reviews search found.
    pub reviews: Vec<reviews::Found>,
    /// Why each session waits, as the daemon last said
    /// (`StatusChanged.reason`), or the code a crashed one's PTY left with.
    pub reasons: HashMap<AgentId, orion_core::WaitReason>,
    /// Settings › Connections › GITHUB AND LINEAR, as last read.
    pub inbox: crate::config::InboxSettings,
    /// When the poll last asked.
    pub asked: Option<Instant>,
    /// The SLACK section: the messages heard, and Settings' rules.
    pub slack: slack::Slack,
    /// Where the poll's answers, and the Slack asks', go.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
}

/// Every card a source offers now, hidden or not.
fn offered(app: &App) -> Vec<Card> {
    let sources: [&dyn CardSource; 5] = [
        &agents::Agents,
        &linear::Inbox,
        &reviews::Reviews,
        &slack::Inbox,
        &meetings::Meetings,
    ];
    sources.iter().flat_map(|s| s.cards(app)).collect()
}

/// Every card the board shows now: each source's, less what Done and
/// Snooze hide and what Settings › Connections turned off, in board order.
pub fn cards(app: &App) -> Vec<Card> {
    let mut all = offered(app);
    if !app.needs_you.inbox.linear_mentions {
        all.retain(|c| c.section != model::Section::Linear);
    }
    if !app.needs_you.inbox.code_reviews {
        all.retain(|c| c.section != model::Section::CodeReviews);
    }
    app.needs_you.store.visible(all, chrono::Utc::now())
}

/// The footer's count: how many cards the board would show.
pub fn count(app: &App) -> usize {
    cards(app).len()
}

/// Bring an open board's cards up to date with the app.
pub fn refresh(app: &mut App) {
    if !matches!(&app.overlay, Some(crate::app::Overlay::Palette(_))) {
        return;
    }
    let fresh = cards(app);
    if let Some(crate::app::Overlay::Palette(palette)) = &mut app.overlay {
        palette.board.set_cards(fresh);
    }
}

/// A session's status changed: keep the reason the daemon gave, and
/// forget one it no longer waits for. A crash with no reason of its own
/// keeps the exit code its PTY reported.
pub fn status_changed(
    app: &mut App,
    agent: &AgentId,
    status: orion_core::AgentStatus,
    reason: Option<orion_core::WaitReason>,
) {
    use orion_core::AgentStatus;
    let reasons = &mut app.needs_you.reasons;
    match (status, reason) {
        (AgentStatus::NeedsFeedback | AgentStatus::Terminated, Some(reason)) => {
            reasons.insert(agent.clone(), reason);
        }
        (AgentStatus::Terminated, None)
            if matches!(
                reasons.get(agent),
                Some(orion_core::WaitReason::Crash { .. })
            ) => {}
        _ => {
            reasons.remove(agent);
        }
    }
}

/// A session's PTY exited with `code`: its crash card says it.
pub fn exited(app: &mut App, agent: &AgentId, code: i32) {
    app.needs_you.reasons.insert(
        agent.clone(),
        orion_core::WaitReason::Crash {
            exit_code: Some(code),
        },
    );
}

/// Load the store, as the app starts.
pub fn load(app: &mut App) {
    app.needs_you.store = Store::load(&Store::path());
    slack::loaded(app);
}

/// One beat of the poll: ask Linear under every distinct project key,
/// and `gh` for the reviews — each only while its row is on.
pub fn poll(app: &mut App) {
    let Some(tx) = app.needs_you.tx.clone() else {
        return;
    };
    app.needs_you.asked = Some(Instant::now());
    let inbox = app.needs_you.inbox.clone();
    if inbox.linear_mentions {
        for (key, projects) in crate::linear::keys_by_project(app) {
            // Kept under the first project with the key.
            let (tx, project) = (tx.clone(), projects[0].clone());
            tokio::spawn(async move {
                let notes = linear::fetch(key).await.ok();
                let _ = tx.send(Answer::Linear { project, notes });
            });
        }
    }
    if inbox.code_reviews {
        tokio::spawn(async move {
            let _ = tx.send(Answer::Reviews(reviews::search().await));
        });
    }
}

/// The board opened: ask again when the last ask is stale.
pub fn opened(app: &mut App) {
    let stale = app.needs_you.asked.is_none_or(|at| at.elapsed() >= STALE);
    if stale && interval().is_some() {
        poll(app);
    }
}

/// A poll's answer lands: kept, and the board brought up to date.
pub fn land(app: &mut App, answer: Answer) {
    match answer {
        Answer::Linear { project, notes } => {
            if let Some(notes) = notes {
                app.needs_you.linear.insert(project, notes);
            }
        }
        Answer::Reviews(found) => {
            if let Some(found) = found {
                app.needs_you.reviews = found;
            }
        }
        Answer::Slack(looked) => slack::landed(app, looked),
    }
    let live = offered(app);
    if app.needs_you.store.prune(&live, chrono::Utc::now()) {
        save(app);
    }
    refresh(app);
    app.dirty = true;
}

fn save(app: &App) {
    // Unit tests drive the board on a bare `App`: never the real data dir.
    if cfg!(test) {
        return;
    }
    let _ = app.needs_you.store.save(&Store::path());
}

/// `⌘D` on the board's card: hidden until its source changes it, and a
/// Linear card's notifications marked read in Linear too.
pub fn done(app: &mut App) {
    let Some(card) = selected(app) else {
        return;
    };
    app.needs_you.store.done(&card);
    if card.section == model::Section::Linear {
        mark_read(app, &card);
    }
    save(app);
    refresh(app);
    app.dirty = true;
}

/// `⌘Z` on the board's card: hidden for an hour.
pub fn snooze(app: &mut App) {
    let Some(card) = selected(app) else {
        return;
    };
    app.needs_you.store.snooze(&card, chrono::Utc::now());
    save(app);
    refresh(app);
    app.dirty = true;
}

/// The card under the open board's cursor.
pub fn selected(app: &App) -> Option<Card> {
    match &app.overlay {
        Some(crate::app::Overlay::Palette(palette)) => palette.board.selected().cloned(),
        _ => None,
    }
}

/// Mark the notifications behind a Linear card read — here at once, and
/// in Linear off the loop.
fn mark_read(app: &mut App, card: &Card) {
    let Some((project, notes)) = linear::notes_of(app, &card.key) else {
        return;
    };
    let ids: Vec<String> = notes.iter().map(|n| n.id.clone()).collect();
    let project = project.clone();
    let key = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == project)
        .and_then(|p| crate::linear::read_linear_key(&p.repo_path));
    if let Some(list) = app.needs_you.linear.get_mut(&project) {
        list.retain(|n| !ids.contains(&n.id));
    }
    if let Some(key) = key {
        tokio::spawn(async move {
            if let Err(why) = linear::mark_read(key, ids).await {
                tracing::warn!("needs you: marking Linear notifications read: {why}");
            }
        });
    }
}

/// The FOOTER's `⌘K needs you N` segment's spans, or None
/// when the board's key is unbound.
pub fn footer_segment(app: &App, hovered: bool) -> Option<Vec<ratatui::text::Span<'static>>> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    let th = app.theme;
    let key = crate::hints::key(&app.keymap, crate::keymap::Action::Palette)?;
    let n = count(app);
    let color = if n > 0 { th.err } else { th.ok };
    Some(vec![
        Span::styled(key, crate::hints::key_style(th)),
        Span::styled(
            " needs you ",
            crate::ui::footer::footer_button_style(th, hovered),
        ),
        Span::styled(n.to_string(), Style::default().fg(color)),
    ])
}
