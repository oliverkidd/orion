//! FACTORY CARDS: the software factory's Slack messages that no session's
//! wait owns — a pull request's verdict, a plan with nothing to build, a
//! thin ticket's question to its reporter, the day's new rules. The TUI
//! builds each one (`orion_tui::factory::cards`); the daemon posts it as
//! the bot and hands the presses back.
//!
//! - **Where**: `CardRecipient::Owner` goes to the bot's DM with the
//!   installing user (the phone's). `CardRecipient::Email` is looked up
//!   (`users.lookupByEmail`, which needs `users:read.email`) and goes to
//!   the bot's DM with whoever has it (`conversations.open`). Nobody with
//!   that email — an external reporter, one not on Slack — is
//!   `FactoryCardFailure::NotOnSlack`, so the TUI can ask the owner
//!   instead.
//! - **One message per key**: where each card went is kept in the store
//!   (`factory_messages`), by the TUI's key. A second post under a key
//!   edits that message in place (`chat.update`) — unless it shows just
//!   that already; `UpdateFactoryCard` only ever edits. A message gone
//!   from Slack is posted afresh. A card nothing has touched in
//!   [`crate::store::FACTORY_MESSAGES_KEEP_MS`] is forgotten as the
//!   daemon starts.
//! - **Pictures**: a card's `image` is uploaded into its thread
//!   (`files.getUploadURLExternal`, the bytes, then
//!   `files.completeUploadExternal`; `files:write`) — again only when it
//!   is not the one uploaded last. One that can't be says so in the
//!   thread; the card itself stands.
//! - **Buttons**: `action_id` `orion_factory_<n>` (one tap) or
//!   `orion_factory_type_<n>` (words first: the same text modal as the
//!   phone's `Type an answer…`), `value` `<key>|<action>`. A link button
//!   (`orion_factory_link_<n>`, with a `url`) is opened by Slack itself;
//!   its press is acked and ignored ([`is_link`]), so the card keeps its
//!   buttons and the blocks stored keep their urls. A press counts
//!   from the owner, and from anyone else **only** on a card sent to them
//!   — the thin-ticket DM. It is broadcast to every TUI as
//!   `ServerEvent::FactoryAction`, after the message is updated in place
//!   to `✓ <label>` with its buttons gone. The TUI may rewrite it after
//!   with `UpdateFactoryCard`.
//! - **Only while you are away**: a card to you goes to Slack only while
//!   you count as away (`away`, the phone's own rule) and outside quiet
//!   hours, with Slack connected ([`Daemon::factory_to_slack`]). At the
//!   desk it is held instead (the store's `factory_held`): the TUI shows
//!   every card it posts in orion — a badge on its session, a line in the
//!   footer, Needs you — and its buttons work there. As you go away,
//!   every card still held goes to Slack, once ([`Daemon::factory_flush`],
//!   on each away poll and on ⌘⇧A); one answered in orion first was
//!   dropped by its `UpdateFactoryCard` and never goes. A card already on
//!   Slack is edited there whoever is at the desk, and a card to someone
//!   else (the thin-ticket DM) never waits ([`post_route`]).
//! - **Scopes**: a bot installed before the factory lacks
//!   `users:read.email` and `files:write`. Phone messages never need
//!   them; a card that does fails as `MissingScope`, and the TUI says
//!   once that the Slack app wants reinstalling.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use orion_core::factory_card::{ACTION, ACTION_LINK, ACTION_TYPE, SEP};
use orion_core::{CardRecipient, FactoryCard, FactoryCardFailure, ServerEvent, SlackUser};
use serde_json::{json, Value};

use crate::phone::{clip, escape, plain, LABEL_CHARS, SUMMARY_CHARS, TITLE_CHARS};
use crate::registry::Daemon;
use crate::slack::Session;

/// The text modal's `callback_id`, its one input block and its box.
const CALLBACK: &str = "orion_factory";
const BLOCK_WORDS: &str = "orion_factory_words";
const INPUT_TEXT: &str = "text";
/// The Slack errors that mean a recorded message is gone, so a post under
/// its key starts a new one.
const GONE: [&str; 3] = [
    "message_not_found",
    "channel_not_found",
    "cant_update_message",
];

/// One posting or press at a time: a second post under a key must find
/// the first one's message, and the TUI's rewrite after a press must land
/// after the press's own `✓`.
static SENDING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A posted factory card, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactoryMessage {
    pub key: String,
    pub channel: String,
    pub ts: String,
    /// The Slack user it went to (the owner's id for an owner card).
    pub recipient: String,
    pub text: String,
    /// Its blocks as JSON.
    pub blocks: String,
    /// The picture last uploaded into its thread.
    pub image: Option<PathBuf>,
}

/// A factory button pressed, or its modal submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Press {
    /// The card's message.
    pub channel: String,
    pub ts: String,
    /// Who pressed it.
    pub user: String,
    pub key: String,
    pub action: String,
    /// The button's label.
    pub label: String,
    pub step: Step,
}

/// What a press does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Act on it: a one-tap button (no words), or a modal's submit.
    Act { text: Option<String> },
    /// Open the text modal, with the press's `trigger_id`.
    Modal { trigger: String },
}

/// Whether `payload` is a press on a factory link button: Slack has
/// opened its `url` already, and there is nothing more to do.
pub fn is_link(payload: &Value) -> bool {
    payload["type"] == "block_actions"
        && payload["actions"][0]["action_id"]
            .as_str()
            .is_some_and(|id| id.starts_with(&format!("{ACTION_LINK}_")))
}

/// A `block_actions` press on a factory button, routed — whoever pressed
/// it; [`allowed`] decides whether it counts. Any other button, and a
/// link button ([`is_link`]), is None.
pub fn route_action(payload: &Value) -> Option<Press> {
    if payload["type"] != "block_actions" || is_link(payload) {
        return None;
    }
    let action = payload["actions"].get(0)?;
    let id = action["action_id"].as_str()?;
    if !id.starts_with(&format!("{ACTION}_")) {
        return None;
    }
    let (key, act) = action["value"].as_str()?.rsplit_once(SEP)?;
    let container = &payload["container"];
    let step = match id.starts_with(&format!("{ACTION_TYPE}_")) {
        true => Step::Modal {
            trigger: payload["trigger_id"].as_str()?.to_string(),
        },
        false => Step::Act { text: None },
    };
    Some(Press {
        channel: container["channel_id"]
            .as_str()
            .or_else(|| payload["channel"]["id"].as_str())?
            .to_string(),
        ts: container["message_ts"]
            .as_str()
            .or_else(|| payload["message"]["ts"].as_str())?
            .to_string(),
        user: payload["user"]["id"].as_str()?.to_string(),
        key: key.to_string(),
        action: act.to_string(),
        label: action["text"]["text"].as_str().unwrap_or(act).to_string(),
        step,
    })
}

/// A `view_submission` of the factory's text modal, routed: the press it
/// was opened by ([`modal`]'s `private_metadata`), with the words typed.
pub fn route_submission(payload: &Value) -> Option<Press> {
    let view = &payload["view"];
    if payload["type"] != "view_submission" || view["callback_id"] != CALLBACK {
        return None;
    }
    let meta: Value = serde_json::from_str(view["private_metadata"].as_str()?).ok()?;
    let (key, action) = meta["value"].as_str()?.rsplit_once(SEP)?;
    let text = view["state"]["values"][BLOCK_WORDS][INPUT_TEXT]["value"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    Some(Press {
        channel: meta["channel"].as_str()?.to_string(),
        ts: meta["ts"].as_str()?.to_string(),
        user: payload["user"]["id"].as_str()?.to_string(),
        key: key.to_string(),
        action: action.to_string(),
        label: meta["label"].as_str().unwrap_or(action).to_string(),
        step: Step::Act { text },
    })
}

/// Whether `user`'s press on a card sent to `recipient` counts: the
/// owner's (`me`) always does; anyone else's only on their own card.
pub fn allowed(user: &str, me: &str, recipient: &str) -> bool {
    user == me || user == recipient
}

/// The text modal a "Type…" button opens: one box, labelled with the
/// button, remembering the press in its `private_metadata`.
pub fn modal(press: &Press) -> Value {
    let title = press.label.trim_end_matches('…').trim();
    let meta = json!({"channel": press.channel, "ts": press.ts,
        "value": format!("{}{SEP}{}", press.key, press.action), "label": title});
    json!({
        "type": "modal",
        "callback_id": CALLBACK,
        "private_metadata": meta.to_string(),
        "title": plain(&clip(title, TITLE_CHARS)),
        "submit": plain("Send"),
        "close": plain("Cancel"),
        "blocks": [{"type": "input", "block_id": BLOCK_WORDS, "optional": false,
            "label": plain(&clip(title, LABEL_CHARS)),
            "element": {"type": "plain_text_input", "action_id": INPUT_TEXT, "multiline": true}}],
    })
}

/// `blocks` once acted on: the buttons gone, and `✓ <label>` at the foot
/// — with the start of the words typed, for a modal's.
pub fn acted(blocks: &[Value], label: &str, text: Option<&str>) -> Vec<Value> {
    let label = escape(label.trim_end_matches('…').trim());
    let foot = match text {
        Some(words) => format!("✓ {label}: {}", escape(&clip(words, SUMMARY_CHARS))),
        None => format!("✓ {label}"),
    };
    let mut out: Vec<Value> = blocks
        .iter()
        .filter(|b| b["type"] != "actions")
        .cloned()
        .collect();
    out.push(json!({"type": "context", "elements": [{"type": "mrkdwn", "text": foot}]}));
    out
}

/// Who and where `to` is: the Slack user and the bot's DM with them.
/// `lookup` finds an email's user, `open` a user's DM.
pub fn resolve(
    to: &CardRecipient,
    me: &str,
    dm: &str,
    lookup: impl FnOnce(&str) -> Result<String, String>,
    open: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(String, String), FactoryCardFailure> {
    match to {
        CardRecipient::Owner => Ok((me.to_string(), dm.to_string())),
        CardRecipient::Email(email) => {
            let user = lookup(email).map_err(|e| failure(&e, "users:read.email"))?;
            if user == me {
                return Ok((user, dm.to_string()));
            }
            let channel = open(&user).map_err(|e| failure(&e, "im:write"))?;
            Ok((user, channel))
        }
    }
}

/// Slack's error code as a card's failure; `scope` is what the call
/// needs, for a `missing_scope`.
fn failure(error: &str, scope: &str) -> FactoryCardFailure {
    match error {
        "users_not_found" | "user_not_found" | "user_disabled" => FactoryCardFailure::NotOnSlack,
        "missing_scope" => FactoryCardFailure::MissingScope(scope.to_string()),
        "not_authed" | "invalid_auth" | "token_revoked" => FactoryCardFailure::NotConnected,
        other => FactoryCardFailure::Other(other.to_string()),
    }
}

/// Where a post goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostRoute {
    /// To Slack: posted, or the message under its key edited.
    Slack,
    /// Held, for orion to show, until you go away.
    Hold,
}

/// Where a card to `to` goes: a card to someone else, and one already on
/// Slack (`on_slack`), go there; a card to you goes only while
/// `to_slack` — you are away — and is held otherwise.
pub fn post_route(to: &CardRecipient, to_slack: bool, on_slack: bool) -> PostRoute {
    match to {
        CardRecipient::Email(_) => PostRoute::Slack,
        CardRecipient::Owner if to_slack || on_slack => PostRoute::Slack,
        CardRecipient::Owner => PostRoute::Hold,
    }
}

/// Run `call` against Slack off the async workers.
async fn blocking<T: Send + 'static>(
    call: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(call)
        .await
        .map_err(|e| e.to_string())?
}

impl Daemon {
    /// `ClientRequest::PostFactoryCard`: post `card`, or edit the message
    /// already up under its key; tell every TUI when it can't be.
    pub fn factory_post(self: &Arc<Self>, card: FactoryCard) {
        let daemon = self.clone();
        tokio::spawn(async move { daemon.factory_send(card, true).await });
    }

    /// `ClientRequest::UpdateFactoryCard`: edit the message under `key`
    /// in place; a key never posted is left alone.
    pub fn factory_update(self: &Arc<Self>, key: String, card: FactoryCard) {
        let daemon = self.clone();
        tokio::spawn(async move {
            daemon
                .factory_send(FactoryCard { key, ..card }, false)
                .await
        });
    }

    async fn factory_send(self: &Arc<Self>, card: FactoryCard, may_post: bool) {
        let _one_at_a_time = SENDING.lock().await;
        let key = card.key.clone();
        if may_post {
            let on_slack = self.store.factory_message(&key).ok().flatten().is_some();
            if post_route(&card.to, self.factory_to_slack(), on_slack) == PostRoute::Hold {
                if let Err(e) = self.store.hold_factory_card(&card) {
                    tracing::warn!(error = %e, %key, "factory card not held");
                }
                return;
            }
            // Going to Slack now: nothing waits under its key any more.
            let _ = self.store.drop_held_factory_card(&key);
        } else if self.store.drop_held_factory_card(&key).unwrap_or(false) {
            // Closed in orion before it ever reached Slack.
            tracing::info!(%key, "held factory card closed in orion");
            return;
        }
        if let Err(reason) = self.factory_send_or_fail(card, may_post).await {
            tracing::warn!(%key, ?reason, "factory card not posted");
            self.broadcast(ServerEvent::FactoryCardFailed { key, reason });
        }
    }

    /// Whether a card to you goes to Slack now: Slack is connected, you
    /// count as away, and it isn't quiet hours — the phone's own rule.
    pub(crate) fn factory_to_slack(&self) -> bool {
        let settings = crate::config::Config::load().away;
        self.slack.connected()
            && self.away.is_away()
            && !settings.is_quiet(crate::away::local_minute())
    }

    /// Every card held while you were at the desk, to Slack now that you
    /// are away — each once. One Slack drops on the way is held again.
    pub(crate) async fn factory_flush(self: &Arc<Self>) {
        // The store first: it is cheap, and nothing is held most ticks.
        if !self.store.any_held_factory_cards().unwrap_or(false) || !self.factory_to_slack() {
            return;
        }
        let _one_at_a_time = SENDING.lock().await;
        let cards = match self.store.take_held_factory_cards() {
            Ok(cards) => cards,
            Err(e) => {
                tracing::warn!(error = %e, "held factory cards not read");
                return;
            }
        };
        tracing::info!(cards = cards.len(), "away: held factory cards to Slack");
        for card in cards {
            let key = card.key.clone();
            match self.factory_send_or_fail(card.clone(), true).await {
                Ok(()) => {}
                Err(FactoryCardFailure::NotConnected) => {
                    let _ = self.store.hold_factory_card(&card);
                }
                Err(reason) => {
                    tracing::warn!(%key, ?reason, "held factory card not posted");
                    self.broadcast(ServerEvent::FactoryCardFailed { key, reason });
                }
            }
        }
    }

    /// Post or edit `card` on Slack. The caller holds [`SENDING`].
    async fn factory_send_or_fail(
        self: &Arc<Self>,
        card: FactoryCard,
        may_post: bool,
    ) -> Result<(), FactoryCardFailure> {
        let session = self
            .slack
            .session()
            .ok_or(FactoryCardFailure::NotConnected)?;
        let blocks: Vec<Value> = card.blocks.as_array().cloned().unwrap_or_default();
        let shown = Value::from(blocks.clone()).to_string();
        let posted = self.store.factory_message(&card.key).ok().flatten();
        let updated = match posted {
            // Already showing just this: nothing to edit.
            Some(message) if message.text == card.text && message.blocks == shown => Some(message),
            Some(mut message) => {
                let (api, m, text, b) = (
                    session.api.clone(),
                    message.clone(),
                    card.text.clone(),
                    blocks.clone(),
                );
                match blocking(move || api.update(&m.channel, &m.ts, &text, &b)).await {
                    Ok(()) => {
                        message.text = card.text.clone();
                        message.blocks = shown;
                        Some(message)
                    }
                    // Gone from Slack: post it afresh.
                    Err(e) if may_post && GONE.contains(&e.as_str()) => None,
                    Err(e) => return Err(failure(&e, "chat:write")),
                }
            }
            None if !may_post => {
                tracing::info!(key = %card.key, "no factory card to update");
                return Ok(());
            }
            None => None,
        };
        let mut message = match updated {
            Some(message) => message,
            None => self.factory_post_new(&session, &card, &blocks).await?,
        };
        if let Some(image) = card.image.filter(|i| message.image.as_ref() != Some(i)) {
            if self.factory_upload(&session, &message, &image).await {
                message.image = Some(image);
            }
        }
        if let Err(e) = self.store.upsert_factory_message(&message) {
            tracing::warn!(error = %e, key = %message.key, "factory card not recorded");
        }
        Ok(())
    }

    /// Post `card` afresh to whoever it is for.
    async fn factory_post_new(
        &self,
        session: &Arc<Session>,
        card: &FactoryCard,
        blocks: &[Value],
    ) -> Result<FactoryMessage, FactoryCardFailure> {
        let (api, to, me, dm) = (
            session.api.clone(),
            card.to.clone(),
            session.me.clone(),
            session.dm.clone(),
        );
        let (text, b) = (card.text.clone(), blocks.to_vec());
        let (recipient, channel, ts) = tokio::task::spawn_blocking(move || {
            let (user, channel) = resolve(
                &to,
                &me,
                &dm,
                |email| api.lookup_by_email(email),
                |user| api.open_dm(user),
            )?;
            let ts = api
                .post(&channel, &text, Some(&b), None)
                .map_err(|e| failure(&e, "chat:write"))?;
            Ok((user, channel, ts))
        })
        .await
        .map_err(|e| FactoryCardFailure::Other(e.to_string()))??;
        tracing::info!(key = %card.key, %channel, %ts, "factory card posted");
        Ok(FactoryMessage {
            key: card.key.clone(),
            channel,
            ts,
            recipient,
            text: card.text.clone(),
            blocks: Value::from(blocks.to_vec()).to_string(),
            image: None,
        })
    }

    /// Upload `image` into `message`'s thread. One that can't be says so
    /// there; false then.
    async fn factory_upload(
        &self,
        session: &Arc<Session>,
        message: &FactoryMessage,
        image: &std::path::Path,
    ) -> bool {
        static SAID_SCOPE: AtomicBool = AtomicBool::new(false);
        let (api, m, path) = (session.api.clone(), message.clone(), image.to_path_buf());
        let uploaded = blocking(move || {
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "picture.png".into());
            api.upload(&m.channel, &m.ts, &name, &bytes)
        })
        .await;
        let Err(e) = uploaded else {
            return true;
        };
        let why = match e.as_str() {
            "missing_scope" => {
                if !SAID_SCOPE.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        "the Slack app lacks files:write: reinstall it from its manifest"
                    );
                }
                "reinstall the Slack app so orion can upload files (files:write)".to_string()
            }
            other => other.to_string(),
        };
        tracing::warn!(key = %message.key, error = %e, "factory picture not uploaded");
        let (api, m) = (session.api.clone(), message.clone());
        let note = format!("The picture wasn't uploaded: {why}");
        let _ = blocking(move || api.post(&m.channel, &note, None, Some(&m.ts))).await;
        false
    }

    /// A press on a factory card (or its modal's submit): the modal
    /// opened, or — when it counts — the card marked `✓ <label>` and
    /// every TUI told.
    pub(crate) async fn factory_press(self: &Arc<Self>, session: &Arc<Session>, press: Press) {
        let Some(message) = self
            .store
            .factory_message_at(&press.channel, &press.ts)
            .ok()
            .flatten()
        else {
            tracing::info!(key = %press.key, "a press on a factory card orion has no record of");
            return;
        };
        if message.key != press.key || !allowed(&press.user, &session.me, &message.recipient) {
            tracing::info!(key = %message.key, user = %press.user, "factory press ignored: not theirs");
            return;
        }
        let text = match &press.step {
            // The trigger lasts three seconds: nothing else waits first.
            Step::Modal { trigger } => {
                let (api, view, trigger) = (session.api.clone(), modal(&press), trigger.clone());
                if let Err(e) = blocking(move || api.views_open(&trigger, &view)).await {
                    tracing::warn!(error = %e, "factory modal not opened");
                }
                return;
            }
            Step::Act { text } => text.clone(),
        };
        let _one_at_a_time = SENDING.lock().await;
        let blocks: Vec<Value> = serde_json::from_str(&message.blocks).unwrap_or_default();
        let blocks = acted(&blocks, &press.label, text.as_deref());
        let (api, m, b) = (session.api.clone(), message.clone(), blocks.clone());
        let line = format!("✓ {}", press.label.trim_end_matches('…').trim());
        match blocking(move || api.update(&m.channel, &m.ts, &line, &b)).await {
            Ok(()) => {
                let message = FactoryMessage {
                    blocks: Value::from(blocks).to_string(),
                    ..message
                };
                if let Err(e) = self.store.upsert_factory_message(&message) {
                    tracing::warn!(error = %e, "factory card not recorded");
                }
            }
            Err(e) => tracing::warn!(error = %e, "factory card not marked acted on"),
        }
        tracing::info!(key = %press.key, action = %press.action, "factory card acted on");
        self.broadcast(ServerEvent::FactoryAction {
            key: press.key,
            action: press.action,
            text,
            by: SlackUser {
                is_owner: press.user == session.me,
                id: press.user,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(user: &str, action_id: &str, value: &str) -> Value {
        json!({"type": "block_actions", "user": {"id": user}, "trigger_id": "T1",
            "container": {"type": "message", "message_ts": "100.1", "channel_id": "D2"},
            "actions": [{"action_id": action_id, "value": value,
                "text": {"type": "plain_text", "text": "Send back…"}}]})
    }

    #[test]
    fn a_button_routes_to_its_key_and_action() {
        let tap = route_action(&press(
            "U1",
            "orion_factory_0",
            "pr:https://x/pull/7:verdict|merge",
        ))
        .unwrap();
        assert_eq!(
            (tap.key.as_str(), tap.action.as_str(), &tap.step),
            (
                "pr:https://x/pull/7:verdict",
                "merge",
                &Step::Act { text: None }
            )
        );
        assert_eq!((tap.channel.as_str(), tap.ts.as_str()), ("D2", "100.1"));
        let typed = route_action(&press("U1", "orion_factory_type_1", "pr:k|send_back")).unwrap();
        assert_eq!(
            typed.step,
            Step::Modal {
                trigger: "T1".into()
            }
        );
        // The phone's own buttons, and a value with no action, are not ours.
        assert_eq!(route_action(&press("U1", "orion_option_0", "0")), None);
        assert_eq!(route_action(&press("U1", "orion_factory_0", "nokey")), None);
    }

    #[test]
    fn a_card_to_you_waits_while_you_are_at_the_desk() {
        let owner = CardRecipient::Owner;
        // At the desk: held, for orion to show; nothing goes to Slack.
        assert_eq!(post_route(&owner, false, false), PostRoute::Hold);
        // Away: to Slack.
        assert_eq!(post_route(&owner, true, false), PostRoute::Slack);
        // Already on Slack (posted while away): edited there either way.
        assert_eq!(post_route(&owner, false, true), PostRoute::Slack);
        // A reporter's question is theirs: it never waits on you.
        let reporter = CardRecipient::Email("ann@example.com".into());
        assert_eq!(post_route(&reporter, false, false), PostRoute::Slack);
    }

    /// At the desk a post is held and nothing reaches Slack; closed in
    /// orion, the held card is dropped, so going away later sends nothing.
    #[tokio::test]
    async fn a_card_held_then_closed_in_orion_never_reaches_slack() {
        let daemon = crate::registry::tests::test_daemon();
        let card = FactoryCard {
            key: "issue:R-1:stalled".into(),
            to: CardRecipient::Owner,
            text: "R-1".into(),
            blocks: json!([]),
            image: None,
        };
        let mut events = daemon.events.subscribe();
        daemon.factory_send(card.clone(), true).await;
        assert!(daemon.store.any_held_factory_cards().unwrap(), "held");
        assert!(daemon.store.factory_message(&card.key).unwrap().is_none());
        assert!(events.try_recv().is_err(), "no failure: nothing was sent");
        daemon.factory_send(card.clone(), false).await;
        assert!(!daemon.store.any_held_factory_cards().unwrap(), "dropped");
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn a_link_buttons_press_is_ignored() {
        let link = press("U1", "orion_factory_link_1", "pr:https://x/7:verdict|open");
        assert!(is_link(&link));
        assert_eq!(route_action(&link), None);
        let tap = press("U1", "orion_factory_0", "pr:https://x/7:verdict|merge");
        assert!(!is_link(&tap));
        assert!(route_action(&tap).is_some());
    }

    #[test]
    fn a_non_owners_press_counts_only_on_their_own_card() {
        // An owner card: its recipient is the owner.
        assert!(allowed("U1", "U1", "U1"));
        assert!(!allowed("U9", "U1", "U1"));
        // The thin-ticket DM to U2: U2 and the owner count, nobody else.
        assert!(allowed("U2", "U1", "U2"));
        assert!(allowed("U1", "U1", "U2"));
        assert!(!allowed("U9", "U1", "U2"));
    }

    #[test]
    fn a_type_button_opens_a_modal_whose_submit_brings_the_words() {
        let opened =
            route_action(&press("U2", "orion_factory_type_1", "issue:R-1:thin|reply")).unwrap();
        let view = modal(&opened);
        assert_eq!(view["callback_id"], "orion_factory");
        assert_eq!(view["title"]["text"], "Send back");
        let submit = json!({"type": "view_submission", "user": {"id": "U2"},
            "view": {"callback_id": "orion_factory",
                "private_metadata": view["private_metadata"],
                "state": {"values": {"orion_factory_words": {"text":
                    {"type": "plain_text_input", "value": "  steps added  "}}}}}});
        let routed = route_submission(&submit).unwrap();
        assert_eq!(
            (routed.key.as_str(), routed.action.as_str()),
            ("issue:R-1:thin", "reply")
        );
        assert_eq!(
            (
                routed.channel.as_str(),
                routed.ts.as_str(),
                routed.user.as_str()
            ),
            ("D2", "100.1", "U2")
        );
        assert_eq!(
            routed.step,
            Step::Act {
                text: Some("steps added".into())
            }
        );
        // The phone's modals are not the factory's.
        let mut phone = submit.clone();
        phone["view"]["callback_id"] = "orion_type".into();
        assert_eq!(route_submission(&phone), None);
    }

    #[test]
    fn acted_on_drops_the_buttons_and_says_what() {
        let blocks = vec![
            json!({"type": "header"}),
            json!({"type": "actions", "elements": []}),
        ];
        let after = acted(&blocks, "Merge", None);
        assert!(after.iter().all(|b| b["type"] != "actions"));
        assert_eq!(after.last().unwrap()["elements"][0]["text"], "✓ Merge");
        let typed = acted(&blocks, "Send back…", Some("use <b>"));
        assert_eq!(
            typed.last().unwrap()["elements"][0]["text"],
            "✓ Send back: use &lt;b&gt;"
        );
    }

    #[test]
    fn the_email_path_finds_the_dm_or_falls_back() {
        let open = |user: &str| Ok(format!("D-{user}"));
        let to = CardRecipient::Email("ann@example.com".into());
        assert_eq!(
            resolve(&to, "U1", "D1", |_| Ok("U2".into()), open),
            Ok(("U2".into(), "D-U2".into()))
        );
        assert_eq!(
            resolve(&to, "U1", "D1", |_| Err("users_not_found".into()), open),
            Err(FactoryCardFailure::NotOnSlack)
        );
        assert_eq!(
            resolve(&to, "U1", "D1", |_| Err("missing_scope".into()), open),
            Err(FactoryCardFailure::MissingScope("users:read.email".into()))
        );
        // The owner's own email is the owner's DM.
        assert_eq!(
            resolve(&to, "U1", "D1", |_| Ok("U1".into()), open),
            Ok(("U1".into(), "D1".into()))
        );
        assert_eq!(
            resolve(&CardRecipient::Owner, "U1", "D1", |_| unreachable!(), open),
            Ok(("U1".into(), "D1".into()))
        );
    }
}
