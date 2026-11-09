//! SLACK BRIDGE: the daemon's one connection to Slack, through an app the
//! user installs once (`docs/2026-11-07-needs-you/slack-manifest.json`).
//!
//! - Three tokens, saved by the Connections tab in the Keychain under
//!   `dev.orion.slack`: `app` (`xapp-…`, Socket Mode), `bot` (`xoxb-…`,
//!   the phone DMs) and `user` (`xoxp-…`, who you are and what you can
//!   read).
//! - Signing in: `auth.test` with the user token names you and the
//!   workspace; `conversations.open` with the bot token finds the DM the
//!   phone messages go in.
//! - Then one Socket Mode websocket (`socket`), opened again with backoff
//!   whenever it drops. Its button presses and DM replies answer waiting
//!   sessions (`phone`); every message event is forwarded to the TUIs as
//!   `ServerEvent::SlackMessage`, unfiltered.
//!
//! No tokens is no connection and no polling: the bridge waits for the
//! Connections tab's `SlackReconnect`.

pub mod api;
pub mod socket;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use orion_core::{ServerEvent, SlackStatus};
use serde_json::Value;

use crate::registry::Daemon;
use api::{Api, As};

use orion_core::secrets::SLACK_ACCOUNTS;

/// The first wait before opening the socket again, doubled each time it
/// fails in a row up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A socket that stayed up this long was healthy: the next failure starts
/// the backoff over.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
/// How many event ids are remembered, so an envelope Slack sends twice
/// (it retries one it thinks went unacknowledged) is handled once.
const SEEN_EVENTS: usize = 512;

/// The three tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub app: String,
    pub bot: String,
    pub user: String,
}

impl Tokens {
    /// The saved tokens — `ORION_SLACK_TOKENS` (`app,bot,user`, the e2e
    /// tests' fake Slack) in place of the Keychain when set. None unless
    /// all three are there. Runs `security`: keep it off the async
    /// workers.
    pub fn load() -> Option<Tokens> {
        let [app, bot, user] = SLACK_ACCOUNTS.map(orion_core::secrets::slack_token);
        Some(Tokens {
            app: app?,
            bot: bot?,
            user: user?,
        })
    }
}

/// Who and where, once signed in.
#[derive(Debug, Clone)]
pub struct Session {
    pub api: Api,
    /// The installing user: the only one whose replies count.
    pub me: String,
    /// The bot's own user id, whose messages are never replies.
    pub bot: String,
    /// The bot's DM with `me`, where the phone messages go.
    pub dm: String,
    pub team: String,
}

/// The bridge's state, on the daemon.
#[derive(Default)]
pub struct SlackBridge {
    status: Mutex<SlackStatus>,
    session: Mutex<Option<Arc<Session>>>,
    reconnect: tokio::sync::Notify,
    seen: Mutex<VecDeque<String>>,
}

impl SlackBridge {
    /// How the connection stands, as `ServerEvent::SlackStatus`.
    pub fn current(&self) -> ServerEvent {
        ServerEvent::SlackStatus {
            status: self.status.lock().unwrap().clone(),
        }
    }

    /// The signed-in session, while there is one.
    pub fn session(&self) -> Option<Arc<Session>> {
        self.session.lock().unwrap().clone()
    }

    /// Signed in, so the phone bridge has somewhere to send.
    pub fn connected(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }

    /// Read the tokens again and reconnect (`ClientRequest::SlackReconnect`).
    pub fn reconnect(&self) {
        self.reconnect.notify_one();
    }

    /// Whether `id` (an event's or a message's) is new; remembers it.
    fn first_sight(&self, id: &str) -> bool {
        let mut seen = self.seen.lock().unwrap();
        if seen.iter().any(|s| s == id) {
            return false;
        }
        if seen.len() == SEEN_EVENTS {
            seen.pop_front();
        }
        seen.push_back(id.to_string());
        true
    }
}

impl Daemon {
    /// Set the bridge's status and session, telling every TUI when the
    /// status changed.
    fn set_slack(&self, status: SlackStatus, session: Option<Arc<Session>>) {
        *self.slack.session.lock().unwrap() = session;
        let changed = {
            let mut current = self.slack.status.lock().unwrap();
            let changed = *current != status;
            *current = status;
            changed
        };
        if changed {
            self.broadcast(self.slack.current());
        }
    }
}

/// The bridge's loop, for the daemon's life: sign in with the saved
/// tokens, hold the socket open, and start again on a drop (with backoff)
/// or on `SlackReconnect`.
pub async fn run(daemon: Arc<Daemon>) {
    let mut backoff = FIRST_BACKOFF;
    loop {
        if daemon.shutdown.is_cancelled() {
            return;
        }
        let tokens = tokio::task::spawn_blocking(Tokens::load)
            .await
            .ok()
            .flatten();
        let Some(tokens) = tokens else {
            daemon.set_slack(SlackStatus::default(), None);
            if !wait(&daemon, None).await {
                return;
            }
            continue;
        };
        let api = Api::new(api::base_url(), tokens);
        let started = tokio::time::Instant::now();
        let ended = match sign_in(api).await {
            Ok(session) => {
                let session = Arc::new(session);
                daemon.set_slack(
                    SlackStatus {
                        configured: true,
                        workspace: Some(session.team.clone()),
                        user_id: Some(session.me.clone()),
                        phone_channel: Some(session.dm.clone()),
                        problem: None,
                    },
                    Some(session.clone()),
                );
                listen(&daemon, &session).await
            }
            // Not signed in: nothing to post with either.
            Err(problem) => {
                daemon.set_slack(SlackStatus::default(), None);
                Err(problem)
            }
        };
        match ended {
            Ok(socket::Ended::Refresh) => backoff = FIRST_BACKOFF,
            Ok(socket::Ended::Stopped) => backoff = FIRST_BACKOFF,
            Err(problem) => {
                tracing::warn!(%problem, "slack bridge down");
                if started.elapsed() >= HEALTHY_RUN {
                    backoff = FIRST_BACKOFF;
                }
                // A dropped socket keeps the session: the phone still
                // posts through the Web API meanwhile.
                let session = daemon.slack.session();
                let status = SlackStatus {
                    configured: true,
                    problem: Some(problem),
                    ..daemon.slack.status.lock().unwrap().clone()
                };
                daemon.set_slack(status, session);
                if !wait(&daemon, Some(backoff)).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// Wait for `SlackReconnect`, or `timeout` when given. False at shutdown.
async fn wait(daemon: &Daemon, timeout: Option<Duration>) -> bool {
    let sleep = async {
        match timeout {
            Some(t) => tokio::time::sleep(t).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = daemon.shutdown.cancelled() => false,
        _ = daemon.slack.reconnect.notified() => true,
        _ = sleep => true,
    }
}

/// Who the tokens are, and the phone's DM. The error is the status line.
async fn sign_in(api: Api) -> Result<Session, String> {
    tokio::task::spawn_blocking(move || {
        let me = api
            .auth_test(As::User)
            .map_err(|e| slack_problem("user", &e))?;
        let bot = api
            .auth_test(As::Bot)
            .map_err(|e| slack_problem("bot", &e))?;
        let dm = api
            .open_dm(&me.user_id)
            .map_err(|e| slack_problem("bot", &e))?;
        Ok(Session {
            api,
            me: me.user_id,
            bot: bot.user_id,
            dm,
            team: me.team,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One line for the Connections tab about a call `token` failed with
/// Slack's `error`.
fn slack_problem(token: &str, error: &str) -> String {
    match error {
        "invalid_auth" | "not_authed" | "token_revoked" | "account_inactive" => {
            format!("Slack refused the {token} token ({error}); press c to paste it again")
        }
        "missing_scope" => {
            format!("the {token} token lacks a scope; reinstall the app from its manifest")
        }
        _ => format!("Slack: {error}"),
    }
}

/// Open Socket Mode and read it until it ends. Every envelope is handled
/// on its own task: an answer types its keys a beat apart, and the socket
/// must not wait on them.
async fn listen(daemon: &Arc<Daemon>, session: &Arc<Session>) -> Result<socket::Ended, String> {
    let api = session.api.clone();
    let url = tokio::task::spawn_blocking(move || api.connections_open())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| slack_problem("app", &e))?;
    let stop = async {
        tokio::select! {
            _ = daemon.shutdown.cancelled() => {}
            _ = daemon.slack.reconnect.notified() => {}
        }
    };
    socket::session(&url, stop, |envelope| {
        let daemon = daemon.clone();
        let session = session.clone();
        tokio::spawn(async move { handle(&daemon, &session, envelope).await });
    })
    .await
}

/// One envelope: a message event (the inbox's, and maybe a reply in the
/// phone's DM) or a button press on a phone message.
async fn handle(daemon: &Arc<Daemon>, session: &Arc<Session>, envelope: socket::Envelope) {
    match envelope.kind.as_str() {
        "events_api" => {
            let payload = &envelope.payload;
            if let Some(id) = payload["event_id"].as_str() {
                if !daemon.slack.first_sight(id) {
                    return;
                }
            }
            let event = &payload["event"];
            if event["type"].as_str() != Some("message") {
                return;
            }
            if let Some(forward) = inbox_event(event) {
                daemon.broadcast(forward);
            }
            if let Some(route) = crate::phone::route_message(event, session) {
                // The same message can come once per token that sees it.
                let key = format!("{}:{}", session.dm, event["ts"].as_str().unwrap_or(""));
                if daemon.slack.first_sight(&key) {
                    daemon.phone_reply(session, route).await;
                }
            }
        }
        "interactive" => {
            if let Some(route) = crate::phone::route_action(&envelope.payload, session) {
                daemon.phone_reply(session, route).await;
            }
        }
        _ => {}
    }
}

/// A message event as the TUIs hear it: every one with a conversation and
/// a timestamp, unfiltered.
pub fn inbox_event(event: &Value) -> Option<ServerEvent> {
    let text = |key: &str| event[key].as_str().map(str::to_string);
    Some(ServerEvent::SlackMessage {
        channel: text("channel")?,
        channel_type: text("channel_type").unwrap_or_default(),
        user: text("user"),
        text: text("text").unwrap_or_default(),
        ts: text("ts")?,
        thread_ts: text("thread_ts"),
        subtype: text("subtype"),
        event_json: event.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_message_event_is_forwarded_as_it_came() {
        let event = json!({"type": "message", "channel": "C1", "channel_type": "channel",
            "user": "U2", "text": "hi <@U1>", "ts": "1.2", "thread_ts": "1.0"});
        let Some(ServerEvent::SlackMessage {
            channel,
            channel_type,
            user,
            text,
            ts,
            thread_ts,
            subtype,
            event_json,
        }) = inbox_event(&event)
        else {
            panic!()
        };
        assert_eq!(
            (channel.as_str(), channel_type.as_str(), user.as_deref()),
            ("C1", "channel", Some("U2"))
        );
        assert_eq!((text.as_str(), ts.as_str()), ("hi <@U1>", "1.2"));
        assert_eq!((thread_ts.as_deref(), subtype), (Some("1.0"), None));
        assert_eq!(serde_json::from_str::<Value>(&event_json).unwrap(), event);
        assert!(inbox_event(&json!({"type": "message"})).is_none());
    }

    #[test]
    fn an_event_seen_twice_is_handled_once() {
        let bridge = SlackBridge::default();
        assert!(bridge.first_sight("Ev1"));
        assert!(!bridge.first_sight("Ev1"));
        for n in 0..SEEN_EVENTS {
            bridge.first_sight(&n.to_string());
        }
        assert!(bridge.first_sight("Ev1"), "forgotten once pushed out");
    }

    #[test]
    fn a_refused_token_says_which_and_what_to_do() {
        assert_eq!(
            slack_problem("bot", "invalid_auth"),
            "Slack refused the bot token (invalid_auth); press c to paste it again"
        );
        assert_eq!(slack_problem("app", "boom"), "Slack: boom");
    }
}
