//! FACTORY CARDS: everything in the software factory that needs a person
//! — a pull request's verdict, a plan with nothing to build, a thin
//! ticket's question to its reporter, the day's new rules — goes out as
//! one Slack message from orion's bot, built here and posted by the
//! daemon (`ClientRequest::PostFactoryCard`).
//!
//! - **At the desk, in orion**: a card to you reaches Slack only while
//!   you are away — the daemon holds it otherwise, and sends what is
//!   still open, once each, as you go (`orion_daemon::factory`). In
//!   orion a card lives where its work is ([`place_of`]): on the row of
//!   the session it is about (`◆ card`, and the pane's rule says what it
//!   asks), or of its worktree when no session is live; `c` on that row
//!   opens its buttons ([`open_at_cursor`]). The session's
//!   own Needs you card stands for it — it waits on you while the card is
//!   up — so the board lists only the cards that are about no session
//!   (`needs_you::factory`). A press in either place closes it in the
//!   other.
//! - **Never stale** ([`reconcile`], on every turn of the automations): a card whose
//!   work moved on closes itself, on Slack too — its session at work
//!   again after it was idle (you wrote to it, here or on the phone, or
//!   the fixer did): "Picked up in orion"; the plan it was about answered
//!   in orion; its worktree removed; a kind of card orion no longer makes;
//!   a note that only says something finished, after a day.
//! - **The message** is in the phone's style: a context line
//!   `● Bug fixer · riplo-os · 10:42`, the card's title as the header,
//!   one section per body line (Slack mrkdwn, `*bold*`), then its
//!   buttons in one row.
//! - **The key** is the card's stable id, chosen by whoever posts it, and
//!   its prefix names the module that owns it: `issue:` intake and Linear
//!   (track B), `pr:` PR bodies and verdicts (track D), `kb:` knowledge
//!   (track F), `setup:` the setup check (track C), `week:` week in
//!   review (track G), `fix:` the PR fixer's *Can't fix* (track E). A
//!   second post under the same key edits the message in place.
//! - **Buttons** carry `<key>|<action>` as their value. Their
//!   `action_id`s are `orion_factory_<n>`, or `orion_factory_type_<n>`
//!   for one that asks for words (`Reply…`, `Send back…`): Slack wants
//!   every `action_id` in a row to differ, and the daemon opens its text
//!   modal for the second kind. A link button (`Open ticket`, `Open PR`)
//!   carries its `url`, which Slack opens on whichever device pressed it;
//!   its `action_id` is `orion_factory_link_<n>`, and the daemon acks its
//!   press and does nothing else, so the card keeps its buttons.
//! - **A press** comes back as `ServerEvent::FactoryAction` and is handed
//!   to [`on_action`], which passes it to the owning module. The daemon
//!   has already marked the message `✓ <label>`, its buttons gone; the
//!   module may rewrite it with [`close`], or put it back up with [`post`]
//!   when the press wasn't the card's answer (a Merge the card no longer
//!   stands behind).
//! - **Every card posted** is also kept in `App::factory_cards`, with
//!   whether it is still open, so orion can show the open ones —
//!   and on disk, in `<data_dir>/automations/factory-cards.json`, so a
//!   restarted orion can still close, edit or re-post the cards it sent
//!   (`load`, written after every change). A card acted on, closed or
//!   failed is forgotten [`FORGET_AFTER_MS`] after it was posted.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use orion_core::factory_card::{ACTION, ACTION_LINK, ACTION_SELECT, ACTION_TYPE, SEP};
use orion_core::slack_caps::{BUTTON, HEADER, MAX_BLOCKS, SECTION};
use orion_core::{CardRecipient, ClientRequest, FactoryCard, FactoryCardFailure};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::ui::truncate;
use orion_core::{AgentId, AgentStatus, WorktreeId};

/// How long a closed (or never posted) card is kept after it was posted.
pub(crate) const FORGET_AFTER_MS: i64 = 7 * orion_core::clock::DAY_MS;

/// Who a card goes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Recipient {
    /// The owner, in the bot's DM — as the phone's messages.
    Owner,
    /// Whoever has this email on Slack — a thin ticket's reporter.
    Email(String),
}

/// One button: the `action` its press hands back, its label, whether it
/// opens a text box first (`Reply…`), and the link it opens instead of
/// being handed back (`Open PR`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Button {
    pub action: String,
    pub label: String,
    pub asks_text: bool,
    pub url: Option<String>,
    /// The menu it is a choice in, by its name (`Not for orion…`): on
    /// Slack the buttons that share one are one labelled menu
    /// (`static_select`) rather than a button each. In orion each is a
    /// row of the card's menu as any button is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<String>,
}

impl Button {
    /// A button pressed in one tap.
    pub(crate) fn tap(action: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            label: label.into(),
            asks_text: false,
            url: None,
            menu: None,
        }
    }

    /// A choice in the menu named `menu`, taken in one pick.
    pub(crate) fn choice(
        action: impl Into<String>,
        label: impl Into<String>,
        menu: impl Into<String>,
    ) -> Self {
        Self {
            menu: Some(menu.into()),
            ..Self::tap(action, label)
        }
    }

    /// A button that asks for words first.
    pub(crate) fn text(action: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            label: label.into(),
            asks_text: true,
            url: None,
            menu: None,
        }
    }

    /// A button that opens `url` — in the browser of whatever device
    /// pressed it, Needs you's included — and is never handed back.
    pub(crate) fn link(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            action: "open".into(),
            label: label.into(),
            asks_text: false,
            url: Some(url.into()),
            menu: None,
        }
    }
}

/// A card to post: its key, who it is for, the context line's words,
/// its title, its body lines (Slack mrkdwn), its buttons, and a picture
/// for its thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Card {
    pub key: String,
    pub to: Recipient,
    pub context: String,
    pub title: String,
    pub body: Vec<String>,
    pub buttons: Vec<Button>,
    pub image: Option<PathBuf>,
}

/// Where a posted card stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CardState {
    /// Posted, nobody has acted on it.
    Open,
    /// A button was pressed: its action.
    Acted(String),
    /// Closed by its module ([`close`]), with the note it shows.
    Closed(String),
    /// The daemon couldn't post it.
    Failed(FactoryCardFailure),
}

impl CardState {
    /// Still up on Slack: open, or pressed and not closed yet.
    pub(crate) fn is_up(&self) -> bool {
        matches!(self, Self::Open | Self::Acted(_))
    }
}

/// A card as `App::factory_cards` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Posted {
    pub card: Card,
    pub state: CardState,
    /// When it was last posted (epoch ms).
    pub at_ms: i64,
    /// What [`reconcile`] has seen of its work since it was posted. Not
    /// kept: a restarted orion looks again before it closes anything.
    #[serde(skip)]
    pub watch: Watch,
}

/// What [`reconcile`] has seen of a card's work since it was posted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Watch {
    /// Its session was seen at rest — its turn over, or waiting on you —
    /// so a turn after this is a pickup.
    pub idle_seen: bool,
    /// The worktree it was last seen about ([`place_of`]): once that is
    /// gone from the tree, so is the card's work.
    pub worktree: Option<WorktreeId>,
    /// When a post the network stopped was last sent again (epoch ms).
    pub retried_at_ms: i64,
}

/// Post `card` (or edit it in place, when its key was posted before),
/// and keep it in `app.factory_cards`. The very card still open under its
/// key is left as it is.
pub(crate) fn post(app: &mut App, card: Card) {
    send(app, card, true);
}

/// [`post`], saying so in the footer as a new card arrives when `flash`.
fn send(app: &mut App, card: Card, flash: bool) {
    let unchanged = app
        .factory_cards
        .iter()
        .any(|p| p.card == card && p.state == CardState::Open);
    if unchanged {
        return;
    }
    // New, or back after it was closed: not a pressed card put back up.
    let fresh = !app
        .factory_cards
        .iter()
        .any(|p| p.card.key == card.key && p.state.is_up());
    if flash && fresh && card.to == Recipient::Owner && !app.away {
        // Where to answer it: on its row, when it has one; else Needs you.
        let line = match place_of(app, &card.key) {
            Some(_) => {
                let key =
                    crate::hints::key_or(&app.keymap, crate::keymap::Action::FactoryCard, "c");
                format!("◆ {} · {key} on its row to answer", card.title)
            }
            None => {
                let key = crate::hints::key_or(&app.keymap, crate::keymap::Action::Palette, "⌘K");
                format!("◆ {} · {key} to answer", card.title)
            }
        };
        app.flash = Some(crate::flash::Flash::note(line));
    }
    let time = chrono::Local::now().format("%H:%M").to_string();
    let request = ClientRequest::PostFactoryCard {
        card: wire(&card, &blocks(&card, &time), fresh),
    };
    app.outbox.push(request);
    let posted = Posted {
        card,
        state: CardState::Open,
        at_ms: orion_core::clock::now_ms(),
        watch: Watch::default(),
    };
    match app
        .factory_cards
        .iter_mut()
        .find(|p| p.card.key == posted.card.key)
    {
        Some(old) => *old = posted,
        None => app.factory_cards.push(posted),
    }
    save(app);
    app.dirty = true;
}

/// Where an open card to you shows in orion besides Needs you.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Place {
    /// On the row of the live session it is about.
    Session(AgentId),
    /// On its worktree's rule, with no session of it live.
    Worktree(WorktreeId),
}

/// Where the card under `key` shows: an `issue:` card on its ticket's
/// session (else worktree); a `pr:` or `fix:` card on the session of the
/// factory ticket whose pull request it is about, else the worktree on
/// that pull request's branch. Knowledge, setup and week cards are about
/// no checkout: None.
pub(crate) fn place_of(app: &App, key: &str) -> Option<Place> {
    let builder = &app.automations.linear_builder.ledger;
    let (prefix, rest) = key.split_once(':')?;
    let entry = match prefix {
        "issue" => {
            let (id, _) = crate::automations::linear_builder::owner::parse_key(key)?;
            builder.issues.get(id)
        }
        "pr" | "fix" => {
            let url = rest.rsplit_once(':').map_or(rest, |(url, _)| url);
            let entry = builder
                .issues
                .values()
                .find(|e| e.pr_url.as_deref() == Some(url));
            if entry.is_none() {
                return pr_worktree(app, url).map(Place::Worktree);
            }
            entry
        }
        _ => None,
    }?;
    let live = entry
        .session
        .as_ref()
        .filter(|id| app.tree.agents.iter().any(|a| &a.id == *id && !a.archived));
    if let Some(id) = live {
        return Some(Place::Session(id.clone()));
    }
    if entry.branch.is_empty() {
        return None;
    }
    app.tree
        .worktrees
        .iter()
        .find(|w| !w.is_main && w.project_id == entry.project && w.branch == entry.branch)
        .map(|w| Place::Worktree(w.id.clone()))
}

/// The worktree on the branch of the open pull request at `url`.
fn pr_worktree(app: &App, url: &str) -> Option<WorktreeId> {
    let (project, pr) = app.open_prs.iter().find_map(|(project, open)| {
        open.list
            .iter()
            .find(|pr| pr.url == url)
            .map(|pr| (project, pr))
    })?;
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.project_id == project && w.branch == pr.head)
        .map(|w| w.id.clone())
}

/// How many open cards to you show at `place`.
pub(crate) fn waiting_at(app: &App, place: &Place) -> usize {
    shown_at(app, place.clone()).count()
}

/// The open cards to you: what shows in orion and on Slack.
pub(crate) fn open_to_you(app: &App) -> impl Iterator<Item = &Posted> {
    app.factory_cards
        .iter()
        .filter(|p| p.state == CardState::Open && p.card.to == Recipient::Owner)
}

/// The open cards to you shown at `place`.
fn shown_at(app: &App, place: Place) -> impl Iterator<Item = &Posted> {
    open_to_you(app).filter(move |p| place_of(app, &p.card.key).as_ref() == Some(&place))
}

/// The open card to you shown at `place`, the newest first.
pub(crate) fn open_at<'a>(app: &'a App, place: &Place) -> Option<&'a Posted> {
    shown_at(app, place.clone()).max_by_key(|p| p.at_ms)
}

/// The open card to you on each live session, the newest each: one walk
/// of the cards, for a list of every session (`needs_you::agents`).
pub(crate) fn by_session(app: &App) -> std::collections::HashMap<AgentId, &Posted> {
    let mut out: std::collections::HashMap<AgentId, &Posted> = std::collections::HashMap::new();
    for p in open_to_you(app) {
        if let Some(Place::Session(id)) = place_of(app, &p.card.key) {
            let newer = out.get(&id).is_none_or(|q| p.at_ms > q.at_ms);
            if newer {
                out.insert(id, p);
            }
        }
    }
    out
}

/// Where the grid's cursor is in `bands`, as a card's place: the session
/// under it, else its band's worktree.
fn cursor_place(app: &App, bands: &[crate::launcher::Band]) -> Option<Place> {
    let band = &bands[crate::launcher::band_cursor(app, bands)?];
    let card = crate::launcher::card_cursor(app, band).and_then(|c| band.cards.get(c));
    Some(match card {
        Some(crate::launcher::Card::Session(row)) => Place::Session(row.agent.id.clone()),
        _ => Place::Worktree(band.worktree.clone()),
    })
}

/// The open card to you about what the grid's cursor is on: the
/// session's own, else its worktree's. The grid is laid out only when
/// some card is open, so a frame with none costs nothing.
pub(crate) fn at_cursor(app: &App) -> Option<&Posted> {
    open_to_you(app).next()?;
    at_cursor_in(app, &crate::launcher::bands(app))
}

/// [`at_cursor`] on the bands the caller has laid out already.
pub(crate) fn at_cursor_in<'a>(
    app: &'a App,
    bands: &[crate::launcher::Band],
) -> Option<&'a Posted> {
    open_to_you(app).next()?;
    let place = cursor_place(app, bands)?;
    open_at(app, &place).or_else(|| match place {
        Place::Session(id) => {
            let agent = app.tree.agents.iter().find(|a| a.id == id)?;
            open_at(app, &Place::Worktree(agent.worktree_id.clone()))
        }
        Place::Worktree(_) => None,
    })
}

/// `c` on the grid: the buttons of the card about the row under the
/// cursor, its words above them (`needs_you::factory::open_menu`) — or a
/// word that there is none.
pub(crate) fn open_at_cursor(app: &mut App) {
    match at_cursor(app).map(|p| p.card.key.clone()) {
        Some(key) => crate::needs_you::factory::open_menu(app, &key),
        None => {
            app.flash = Some(crate::flash::Flash::note(
                "No factory card waits on this row",
            ));
            app.dirty = true;
        }
    }
}

/// What a card asks, in a line: a verdict's first body line as plain
/// text (`Review: no pictures`) — its title is the pull request's — and
/// any other card's title (`Nothing to build: already shipped in #2075`).
pub(crate) fn summary(card: &Card) -> String {
    if !card.key.starts_with(PR_PREFIX) {
        return card.title.clone();
    }
    card.body
        .iter()
        .map(|line| plain(line))
        .map(|line| line.lines().next().unwrap_or("").trim().to_string())
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| card.title.clone())
}

/// Slack mrkdwn as plain text: bold and italic marks gone, `<url|label>`
/// as its label, the escapes undone (`needs_you::slack::plain`).
pub(crate) fn plain(line: &str) -> String {
    crate::needs_you::slack::plain(line, &Default::default()).replace(['*', '_'], "")
}

/// A pull request's verdict card's key prefix (`pr_body`).
const PR_PREFIX: &str = "pr:";
/// The knowledge backfill's summary card's key prefix
/// (`knowledge::runner`).
const BACKFILL_PREFIX: &str = "kb:backfill:";
/// The week in review card's key prefix (`factory::week`).
const WEEK_PREFIX: &str = "week:";

/// Kinds of card this orion no longer makes, closed on sight: a session
/// that dies or stalls is relaunched or nudged now.
const RETIRED: [&str; 2] = [":dead", ":stalled"];
/// The knowledge daily card's key prefix: gone too, its rules reviewed in
/// orion's RULE REVIEW now (`crate::rule_review`).
const RETIRED_DAILY: &str = "kb:daily:";
/// A card that only says something finished — the knowledge backfill —
/// closes after this.
const NOTE_KEEP_MS: i64 = orion_core::clock::DAY_MS;
/// The week's card closes before the next week's comes.
const WEEK_KEEP_MS: i64 = 6 * orion_core::clock::DAY_MS;
/// What a card whose session went back to work says as it closes.
pub(crate) const PICKED_UP: &str = "Picked up in orion";
/// …and while you are away, when it was the fixer or the phone.
pub(crate) const WORKING_AGAIN: &str = "The agent is working again";
/// How often a post the network stopped is sent again…
const RETRY_MS: i64 = 5 * orion_core::clock::MINUTE_MS;
/// …for as long as this after it was first posted.
const RETRY_FOR_MS: i64 = orion_core::clock::DAY_MS;

/// Whether `p` is a post the network stopped that is still being sent
/// again ([`reconcile`]): not up yet, but on its way.
pub(crate) fn awaiting_retry(p: &Posted, now_ms: i64) -> bool {
    // Passing trouble — Slack not signed in yet, the network, a rate
    // limit, Slack itself — not a refusal.
    matches!(&p.state, CardState::Failed(why) if why.is_transient())
        && now_ms.saturating_sub(p.at_ms) < RETRY_FOR_MS
}

/// What a card on a plan answered in orion says as it closes.
pub(crate) const ANSWERED: &str = "Answered in orion";
/// What a card whose worktree was removed says as it closes.
pub(crate) const WORKTREE_GONE: &str = "Worktree removed";
/// What a kind of card orion no longer makes says as it closes.
const RETIRED_NOTE: &str = "No longer needed";
/// What a card that only said something finished says as it closes.
const EXPIRED: &str = "Expired";

/// Close every open card to you whose work has moved on, here and on
/// Slack — see the module's doc. Run on every turn of the automations
/// (the `Automations` CHORE), once the tree
/// is in.
pub(crate) fn reconcile(app: &mut App) {
    if app.tree.worktrees.is_empty() {
        return;
    }
    let now = orion_core::clock::now_ms();
    // A post the network stopped — the day's rules while the Wi-Fi was
    // down — goes again every few minutes until Slack takes it, for a
    // day: it keeps the time it was first posted.
    let retry: Vec<(Card, i64)> = app
        .factory_cards
        .iter()
        .filter(|p| awaiting_retry(p, now))
        .filter(|p| now.saturating_sub(p.watch.retried_at_ms) >= RETRY_MS)
        .map(|p| (p.card.clone(), p.at_ms))
        .collect();
    for (card, first_at_ms) in retry {
        tracing::info!(
            "factory: posting {} again after a network failure",
            card.key
        );
        let key = card.key.clone();
        send(app, card, false);
        if let Some(p) = app.factory_cards.iter_mut().find(|p| p.card.key == key) {
            p.at_ms = first_at_ms;
            p.watch.retried_at_ms = now;
        }
    }
    let mut closing: Vec<(String, &'static str)> = Vec::new();
    let mut seen: Vec<(String, Watch)> = Vec::new();
    for p in open_to_you(app) {
        let key = p.card.key.as_str();
        let age = now.saturating_sub(p.at_ms);
        if RETIRED.iter().any(|end| key.ends_with(end)) || key.starts_with(RETIRED_DAILY) {
            closing.push((key.to_string(), RETIRED_NOTE));
            continue;
        }
        if (key.starts_with(BACKFILL_PREFIX) && age > NOTE_KEEP_MS)
            || (key.starts_with(WEEK_PREFIX) && age > WEEK_KEEP_MS)
        {
            closing.push((key.to_string(), EXPIRED));
            continue;
        }
        let mut watch = p.watch.clone();
        match place_of(app, key) {
            Some(Place::Session(id)) => {
                let agent = app.tree.agents.iter().find(|a| a.id == id);
                watch.worktree = agent.map(|a| a.worktree_id.clone());
                let status = agent.map(|a| a.status);
                if status == Some(AgentStatus::Running) && watch.idle_seen {
                    let note = match (key.ends_with(":nochange"), app.away) {
                        (true, _) => ANSWERED,
                        (false, false) => PICKED_UP,
                        (false, true) => WORKING_AGAIN,
                    };
                    closing.push((key.to_string(), note));
                    continue;
                }
                // At rest: its turn over, or waiting on you — not booting.
                watch.idle_seen |= matches!(
                    status,
                    Some(AgentStatus::Finished | AgentStatus::NeedsFeedback)
                );
            }
            Some(Place::Worktree(id)) => watch.worktree = Some(id),
            None => {
                // Nowhere to show it — closed only once the worktree it
                // was last seen about has really gone from the tree.
                let gone = watch
                    .worktree
                    .as_ref()
                    .is_some_and(|w| !app.tree.worktrees.iter().any(|t| &t.id == w));
                if gone {
                    closing.push((key.to_string(), WORKTREE_GONE));
                    continue;
                }
            }
        }
        if watch != p.watch {
            seen.push((key.to_string(), watch));
        }
    }
    for (key, watch) in seen {
        if let Some(p) = app.factory_cards.iter_mut().find(|p| p.card.key == key) {
            p.watch = watch;
        }
    }
    for (key, note) in closing {
        tracing::info!("factory: card {key} closed: {note}");
        close(app, &key, note);
    }
}

/// Where the card under `key` stands, if it was posted.
pub(crate) fn state_of<'a>(app: &'a App, key: &str) -> Option<&'a CardState> {
    app.factory_cards
        .iter()
        .find(|p| p.card.key == key)
        .map(|p| &p.state)
}

/// [`close`] the card under `key` with `note`, if it is still up.
pub(crate) fn close_if_up(app: &mut App, key: &str, note: &str) {
    if state_of(app, key).is_some_and(CardState::is_up) {
        close(app, key, note);
    }
}

/// Close the card under `key`: its buttons go, and `note` shows under it
/// (`✓ Merged`). Acting in Needs you closes the Slack card this way.
pub(crate) fn close(app: &mut App, key: &str, note: &str) {
    let Some(posted) = app.factory_cards.iter_mut().find(|p| p.card.key == key) else {
        tracing::warn!("factory: no card {key} to close");
        return;
    };
    posted.state = CardState::Closed(note.to_string());
    // The context line keeps the time it was posted with.
    let mut blocks = blocks(&posted.card, &posted_time(posted.at_ms));
    blocks.retain(|b| b["type"] != "actions");
    blocks.push(context(&format!("✓ {}", escape(note))));
    let card = wire(&posted.card, &blocks, false);
    app.outbox.push(ClientRequest::UpdateFactoryCard {
        key: key.to_string(),
        card,
    });
    save(app);
    app.dirty = true;
}

/// `ServerEvent::FactoryAction`: a button pressed on the card under
/// `key` (with the words typed, for one that asks), by the owner or the
/// one person the card went to. Handed to the module whose prefix the
/// key carries.
pub(crate) fn on_action(
    app: &mut App,
    key: &str,
    action: &str,
    text: Option<String>,
    by_owner: bool,
) {
    if let Some(posted) = app.factory_cards.iter_mut().find(|p| p.card.key == key) {
        posted.state = CardState::Acted(action.to_string());
        save(app);
    }
    app.dirty = true;
    let press = Press {
        key,
        action,
        text,
        by_owner,
    };
    match key.split_once(':').map(|(prefix, _)| prefix) {
        // Intake and Linear: a held ticket, a thin ticket's question, a
        // plan with nothing to build, a stalled session.
        Some("issue") => crate::automations::linear_builder::owner::on_action(app, press),
        // A factory session's phone message: **Not for orion**.
        Some("session") => crate::automations::linear_builder::owner::on_session_action(app, press),
        // A verdict card's buttons.
        Some("pr") => crate::pr_body::on_action(app, press),
        // Knowledge: the daily card's vetoes, the `.gitignore` ask and a
        // backfill's summary.
        Some("kb") => crate::knowledge::on_action(app, press),
        // The factory's setup check: **Open template**.
        Some("setup") => crate::automations::linear_builder::setup::on_action(app, press),
        // The week in review card and its retro's fixes.
        Some("week") => super::week::on_action(app, press),
        Some("fix") => {
            crate::automations::pr_fixer::on_card(app, press.key, press.action, press.text)
        }
        _ => tracing::warn!("factory: a press on {key} ({action}), which no module owns"),
    }
}

/// `ServerEvent::FactoryCardFailed`: the card under `key` was not posted.
/// A missing scope says once, for the whole run, that the Slack app wants
/// reinstalling. A thin ticket's question to its reporter falls back to
/// a card to you (`linear_builder::owner::on_failed`).
pub(crate) fn on_failed(app: &mut App, key: &str, reason: FactoryCardFailure) {
    static SAID_REINSTALL: AtomicBool = AtomicBool::new(false);
    tracing::warn!("factory: card {key} not posted: {reason:?}");
    if let FactoryCardFailure::MissingScope(scope) = &reason {
        if !SAID_REINSTALL.swap(true, Ordering::Relaxed) {
            app.flash = Some(crate::flash::Flash::failed(format!(
                "Factory cards need the Slack app reinstalled (it lacks {scope})"
            )));
        }
    }
    if let Some(posted) = app.factory_cards.iter_mut().find(|p| p.card.key == key) {
        // A close Slack didn't take is still closed here: only a post
        // that never went up is a failed card.
        if !matches!(posted.state, CardState::Closed(_)) {
            posted.state = CardState::Failed(reason.clone());
        }
    }
    save(app);
    if key.starts_with("issue:") {
        crate::automations::linear_builder::owner::on_failed(app, key, &reason);
    }
    app.dirty = true;
}

// ---- on disk ----

/// `factory-cards.json`'s shape.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    cards: Vec<Posted>,
}

/// Where the cards are kept; None under test, so nothing touches the
/// real user's files.
fn store_path() -> Option<PathBuf> {
    (!cfg!(test)).then(|| crate::automations::state_dir().join("factory-cards.json"))
}

/// The cards a previous run posted, for `App::factory_cards` at startup.
pub(crate) fn load() -> Vec<Posted> {
    store_path()
        .map(|path| load_from(&path, orion_core::clock::now_ms()))
        .unwrap_or_default()
}

/// The cards kept at `path`, the ones long closed forgotten. A missing
/// or unreadable file is none.
fn load_from(path: &Path, now_ms: i64) -> Vec<Posted> {
    let mut cards = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
        .unwrap_or_default()
        .cards;
    prune(&mut cards, now_ms);
    cards
}

/// Drop the cards acted on, closed or failed that were posted more than
/// [`FORGET_AFTER_MS`] before `now_ms`; open ones stay.
fn prune(cards: &mut Vec<Posted>, now_ms: i64) {
    cards
        .retain(|p| p.state == CardState::Open || now_ms.saturating_sub(p.at_ms) < FORGET_AFTER_MS);
}

/// `app.factory_cards` to disk, atomically.
fn save(app: &mut App) {
    prune(&mut app.factory_cards, orion_core::clock::now_ms());
    if let Some(path) = store_path() {
        save_to(&path, &app.factory_cards);
    }
}

fn save_to(path: &Path, cards: &[Posted]) {
    /// [`Stored`], borrowed.
    #[derive(Serialize)]
    struct StoredRef<'a> {
        cards: &'a [Posted],
    }
    if let Err(err) = crate::pr_cache::write_json_atomic(path, &StoredRef { cards }) {
        tracing::warn!("factory: writing {}: {err}", path.display());
    }
}

/// One press, as the owning module's handler gets it.
#[derive(Debug)]
pub(crate) struct Press<'a> {
    pub key: &'a str,
    pub action: &'a str,
    pub text: Option<String>,
    pub by_owner: bool,
}

/// The request's card: the wire recipient, the notification line, the
/// blocks and the picture.
fn wire(card: &Card, blocks: &[Value], fresh: bool) -> FactoryCard {
    FactoryCard {
        fresh,
        key: card.key.clone(),
        to: match &card.to {
            Recipient::Owner => CardRecipient::Owner,
            Recipient::Email(email) => CardRecipient::Email(email.clone()),
        },
        text: card.title.clone(),
        blocks: Value::Array(blocks.to_vec()),
        image: card.image.clone(),
    }
}

/// `card` in the phone's layout, its context line stamped `time`.
pub(crate) fn blocks(card: &Card, time: &str) -> Vec<Value> {
    let mut blocks = vec![
        context_line(card, time),
        json!({"type": "header", "text": {"type": "plain_text", "emoji": true,
            "text": truncate(&card.title, HEADER)}}),
    ];
    let room = MAX_BLOCKS - blocks.len() - 2;
    blocks.extend(
        card.body
            .iter()
            .filter(|line| !line.trim().is_empty())
            .take(room)
            .map(|line| {
                json!({"type": "section", "text": {"type": "mrkdwn",
                "text": truncate(line, SECTION)}})
            }),
    );
    if !card.buttons.is_empty() {
        let mut elements: Vec<Value> = Vec::new();
        let mut menus: Vec<&str> = Vec::new();
        for (n, b) in card.buttons.iter().enumerate() {
            match b.menu.as_deref() {
                None => elements.push(button(&card.key, n, b)),
                // The first choice of a menu stands for all of its.
                Some(name) if !menus.contains(&name) => {
                    menus.push(name);
                    elements.push(menu(&card.key, n, name, &card.buttons));
                }
                Some(_) => {}
            }
        }
        blocks.push(json!({"type": "actions", "block_id": "orion_factory", "elements": elements}));
    }
    blocks
}

/// The menu named `name`: every button of the card that is a choice in
/// it, as a labelled `static_select`.
fn menu(key: &str, n: usize, name: &str, buttons: &[Button]) -> Value {
    let options: Vec<Value> = buttons
        .iter()
        .filter(|b| b.menu.as_deref() == Some(name))
        .map(|b| {
            json!({"text": {"type": "plain_text", "emoji": true, "text": truncate(&b.label, BUTTON)},
                "value": format!("{key}{SEP}{}", b.action)})
        })
        .collect();
    json!({"type": "static_select", "action_id": format!("{ACTION_SELECT}_{n}"),
        "placeholder": {"type": "plain_text", "emoji": true, "text": truncate(name, BUTTON)},
        "options": options})
}

fn button(key: &str, n: usize, b: &Button) -> Value {
    let action_id = match (&b.url, b.asks_text) {
        (Some(_), _) => format!("{ACTION_LINK}_{n}"),
        (None, true) => format!("{ACTION_TYPE}_{n}"),
        (None, false) => format!("{ACTION}_{n}"),
    };
    let mut button = json!({"type": "button", "action_id": action_id,
        "value": format!("{key}{SEP}{}", b.action),
        "text": {"type": "plain_text", "emoji": true, "text": truncate(&b.label, BUTTON)}});
    // Slack refuses a whole message over an empty `url`.
    if let Some(url) = b.url.as_deref().filter(|u| !u.is_empty()) {
        button["url"] = url.into();
    }
    if n == 0 && !b.asks_text {
        button["style"] = "primary".into();
    }
    button
}

fn context_line(card: &Card, time: &str) -> Value {
    context(&format!("● {} · {time}", card.context))
}

fn context(text: &str) -> Value {
    json!({"type": "context", "elements": [{"type": "mrkdwn", "text": text}]})
}

fn posted_time(at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at_ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// Slack's three mrkdwn control characters, escaped: the one escape for
/// every card's words.
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A post the network stopped is sent again every few minutes, for a
    /// day from when it was first posted, without a footer line each
    /// time; a close that failed is never posted again.
    #[test]
    fn a_post_the_network_stopped_goes_again_for_a_day() {
        let mut app = App::new();
        app.tree.worktrees.push(orion_core::Worktree {
            id: WorktreeId("w".into()),
            project_id: orion_core::ProjectId("p".into()),
            path: PathBuf::from("/nonexistent/w"),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        });
        let offline =
            || FactoryCardFailure::Other("curl: (6) Could not resolve host: slack.com".into());
        post(&mut app, card());
        let first_at = app.factory_cards[0].at_ms;
        on_failed(&mut app, &card().key, offline());
        app.outbox.clear();
        app.flash = None;
        reconcile(&mut app);
        assert!(matches!(
            app.outbox.last(),
            Some(ClientRequest::PostFactoryCard { .. })
        ));
        assert_eq!(app.flash, None, "no footer line for a retry");
        assert_eq!(app.factory_cards[0].at_ms, first_at, "keeps its first time");
        // Failed again at once: not sent again for five minutes.
        on_failed(&mut app, &card().key, offline());
        app.outbox.clear();
        reconcile(&mut app);
        assert!(app.outbox.is_empty());
        assert!(awaiting_retry(&app.factory_cards[0], first_at + 1));
        assert!(!awaiting_retry(
            &app.factory_cards[0],
            first_at + RETRY_FOR_MS
        ));
        // Slack refusing it is no network failure.
        app.factory_cards[0].state = CardState::Failed(FactoryCardFailure::NotOnSlack);
        assert!(!awaiting_retry(&app.factory_cards[0], first_at + 1));
    }

    fn card() -> Card {
        Card {
            key: "pr:https://github.com/o/r/pull/7:verdict".into(),
            to: Recipient::Owner,
            context: "Bug fixer · riplo-os".into(),
            title: "Fix the login loop".into(),
            body: vec!["*Merge*: nothing risky".into(), String::new()],
            buttons: vec![
                Button::tap("merge", "Merge"),
                Button::text("send_back", "Send back…"),
            ],
            image: None,
        }
    }

    #[test]
    fn a_card_is_laid_out_as_the_phones_messages_are() {
        let blocks = blocks(&card(), "10:42");
        let types: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["context", "header", "section", "actions"]);
        assert_eq!(
            blocks[0]["elements"][0]["text"],
            "● Bug fixer · riplo-os · 10:42"
        );
        assert_eq!(blocks[1]["text"]["text"], "Fix the login loop");
        assert_eq!(blocks[2]["text"]["type"], "mrkdwn");
        assert_eq!(blocks[2]["text"]["text"], "*Merge*: nothing risky");
    }

    #[test]
    fn each_button_carries_its_key_and_action() {
        let blocks = blocks(&card(), "10:42");
        let buttons = blocks[3]["elements"].as_array().unwrap();
        assert_eq!(buttons[0]["action_id"], "orion_factory_0");
        assert_eq!(
            buttons[0]["value"],
            "pr:https://github.com/o/r/pull/7:verdict|merge"
        );
        assert_eq!(buttons[0]["style"], "primary");
        assert_eq!(buttons[1]["action_id"], "orion_factory_type_1");
        assert_eq!(
            buttons[1]["value"],
            "pr:https://github.com/o/r/pull/7:verdict|send_back"
        );
        assert_eq!(buttons[1]["text"]["text"], "Send back…");
        assert!(buttons[0].get("url").is_none());
    }

    #[test]
    fn a_link_button_carries_its_url_under_its_own_action_id() {
        let mut c = card();
        c.buttons
            .insert(1, Button::link("Open PR", "https://github.com/o/r/pull/7"));
        let blocks = blocks(&c, "10:42");
        let link = &blocks[3]["elements"][1];
        assert_eq!(link["action_id"], "orion_factory_link_1");
        assert_eq!(link["url"], "https://github.com/o/r/pull/7");
        assert_eq!(link["text"]["text"], "Open PR");
    }

    #[test]
    fn post_sends_the_card_and_keeps_one_record_per_key() {
        let mut app = App::new();
        post(&mut app, card());
        let mut again = card();
        again.to = Recipient::Email("ann@example.com".into());
        post(&mut app, again);
        assert_eq!(app.factory_cards.len(), 1);
        assert_eq!(app.factory_cards[0].state, CardState::Open);
        let Some(ClientRequest::PostFactoryCard { card: sent }) = app.outbox.last() else {
            panic!("{:?}", app.outbox);
        };
        assert_eq!(sent.to, CardRecipient::Email("ann@example.com".into()));
        assert_eq!(
            sent.blocks[3]["elements"][1]["action_id"],
            "orion_factory_type_1"
        );
    }

    #[test]
    fn close_drops_the_buttons_and_a_failure_is_kept() {
        let mut app = App::new();
        post(&mut app, card());
        close(&mut app, &card().key, "Merged");
        let Some(ClientRequest::UpdateFactoryCard { key, card: sent }) = app.outbox.last() else {
            panic!("{:?}", app.outbox);
        };
        assert_eq!(key, &card().key);
        let blocks = sent.blocks.as_array().unwrap();
        assert!(blocks.iter().all(|b| b["type"] != "actions"));
        assert_eq!(blocks.last().unwrap()["elements"][0]["text"], "✓ Merged");
        // A close Slack didn't take is still closed here…
        on_failed(&mut app, &card().key, FactoryCardFailure::NotOnSlack);
        assert_eq!(
            app.factory_cards[0].state,
            CardState::Closed("Merged".into())
        );
        // …while a post that never went up is a failed card.
        post(&mut app, card());
        on_failed(&mut app, &card().key, FactoryCardFailure::NotOnSlack);
        assert_eq!(
            app.factory_cards[0].state,
            CardState::Failed(FactoryCardFailure::NotOnSlack)
        );
    }

    #[test]
    fn posting_the_open_card_again_sends_nothing() {
        let mut app = App::new();
        post(&mut app, card());
        post(&mut app, card());
        assert_eq!(app.outbox.len(), 1, "the same card, still open");
        on_action(&mut app, &card().key, "merge", None, true);
        post(&mut app, card());
        assert_eq!(app.outbox.len(), 2, "pressed, it goes back up");
        on_action(&mut app, "pr:nobody-posted", "merge", None, true);
        assert_eq!(app.factory_cards.len(), 1);
    }

    #[test]
    fn a_press_marks_the_card_acted() {
        let mut app = App::new();
        post(&mut app, card());
        on_action(&mut app, &card().key, "merge", None, true);
        assert_eq!(app.factory_cards[0].state, CardState::Acted("merge".into()));
    }

    #[test]
    fn the_cards_outlive_a_restart_and_long_closed_ones_are_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automations").join("factory-cards.json");
        let now = 100 * FORGET_AFTER_MS;
        let at = |key: &str, state: CardState, at_ms: i64| Posted {
            card: Card {
                key: key.into(),
                ..card()
            },
            state,
            at_ms,
            watch: Watch::default(),
        };
        let old = now - FORGET_AFTER_MS - 1;
        let kept = vec![
            at("pr:open", CardState::Open, old),
            at("pr:acted", CardState::Acted("right".into()), old),
            at("pr:closed-now", CardState::Closed("Merged".into()), now - 1),
            at(
                "pr:closed-long-ago",
                CardState::Closed("Merged".into()),
                old,
            ),
            at(
                "pr:failed-long-ago",
                CardState::Failed(FactoryCardFailure::NotOnSlack),
                old,
            ),
        ];
        save_to(&path, &kept);
        let loaded = load_from(&path, now);
        let keys: Vec<&str> = loaded.iter().map(|p| p.card.key.as_str()).collect();
        assert_eq!(keys, ["pr:open", "pr:closed-now"]);
        assert_eq!(loaded[0], kept[0], "the whole card comes back");
        assert!(load_from(&dir.path().join("none.json"), now).is_empty());
    }

    #[test]
    fn a_card_loaded_after_a_restart_can_still_be_closed() {
        let mut app = App::new();
        app.factory_cards = vec![Posted {
            card: card(),
            state: CardState::Open,
            at_ms: orion_core::clock::now_ms(),
            watch: Watch::default(),
        }];
        close(&mut app, &card().key, "Merged");
        assert!(matches!(
            app.outbox.last(),
            Some(ClientRequest::UpdateFactoryCard { .. })
        ));
        assert_eq!(
            app.factory_cards[0].state,
            CardState::Closed("Merged".into())
        );
    }
}
