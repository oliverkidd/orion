//! FACTORY CARDS: everything in the software factory that needs a person
//! — a pull request's verdict, a plan with nothing to build, a thin
//! ticket's question to its reporter, the day's new rules — goes out as
//! one Slack message from orion's bot, built here and posted by the
//! daemon (`ClientRequest::PostFactoryCard`).
//!
//! - **At the desk, in orion**: a card to you reaches Slack only while
//!   you are away — the daemon holds it otherwise, and sends what is
//!   still open, once each, as you go (`orion_daemon::factory`). Every
//!   card to you shows in orion either way: a `◆` on the row of the
//!   session it is about, or of its worktree when no session is live
//!   ([`place_of`]); a line in the footer as a new one arrives while you
//!   are here; and the Needs you board (`needs_you::factory`), whose
//!   Enter opens its buttons. A press in either place closes it in the
//!   other.
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
//!   when the press wasn't the card's answer (a verdict's 👍 / 👎).
//! - **Every card posted** is also kept in `App::factory_cards`, with
//!   whether it is still open, so Needs you can mirror the open ones —
//!   and on disk, in `<data_dir>/automations/factory-cards.json`, so a
//!   restarted orion can still close, edit or re-post the cards it sent
//!   (`load`, written after every change). A card acted on, closed or
//!   failed is forgotten [`FORGET_AFTER_MS`] after it was posted.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use orion_core::factory_card::{ACTION, ACTION_LINK, ACTION_TYPE, SEP};
use orion_core::slack_caps::{BUTTON, HEADER, MAX_BLOCKS, SECTION};
use orion_core::{CardRecipient, ClientRequest, FactoryCard, FactoryCardFailure};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::App;
use crate::ui::truncate;
use orion_core::{AgentId, WorktreeId};

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
}

impl Button {
    /// A button pressed in one tap.
    pub(crate) fn tap(action: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            label: label.into(),
            asks_text: false,
            url: None,
        }
    }

    /// A button that asks for words first.
    pub(crate) fn text(action: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            label: label.into(),
            asks_text: true,
            url: None,
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
}

/// Post `card` (or edit it in place, when its key was posted before),
/// and keep it in `app.factory_cards`. The very card still open under its
/// key is left as it is.
pub(crate) fn post(app: &mut App, card: Card) {
    let unchanged = app
        .factory_cards
        .iter()
        .any(|p| p.card == card && p.state == CardState::Open);
    if unchanged {
        return;
    }
    let fresh = !app
        .factory_cards
        .iter()
        .any(|p| p.card.key == card.key && p.state == CardState::Open);
    if fresh && card.to == Recipient::Owner && !app.away {
        let key = crate::hints::key_or(&app.keymap, crate::keymap::Action::Palette, "⌘K");
        app.flash = Some(crate::flash::Flash::note(format!(
            "◆ {} · {key} to answer",
            card.title
        )));
    }
    let time = chrono::Local::now().format("%H:%M").to_string();
    let request = ClientRequest::PostFactoryCard {
        card: wire(&card, &blocks(&card, &time)),
    };
    app.outbox.push(request);
    let posted = Posted {
        card,
        state: CardState::Open,
        at_ms: orion_core::clock::now_ms(),
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
    app.factory_cards
        .iter()
        .filter(|p| p.state == CardState::Open && p.card.to == Recipient::Owner)
        .filter(|p| place_of(app, &p.card.key).as_ref() == Some(place))
        .count()
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
    let card = wire(&posted.card, &blocks);
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
        posted.state = CardState::Failed(reason.clone());
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
fn wire(card: &Card, blocks: &[Value]) -> FactoryCard {
    FactoryCard {
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
        let elements: Vec<Value> = card
            .buttons
            .iter()
            .enumerate()
            .map(|(n, b)| button(&card.key, n, b))
            .collect();
        blocks.push(json!({"type": "actions", "block_id": "orion_factory", "elements": elements}));
    }
    blocks
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
