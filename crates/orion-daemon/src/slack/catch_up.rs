//! CATCH-UP: what Slack said while the bridge wasn't listening.
//!
//! Socket Mode hands over only what happens while a socket is open; with
//! the Mac asleep, the daemon restarting or the network gone, the message
//! events of that gap are never delivered, so the Needs you board never
//! heard of them. Two things close it:
//!
//! - **The inbox**: every message event the bridge hears is kept in the
//!   store (`slack_inbox`, [`crate::store::SLACK_INBOX_KEEP_MS`]), and a
//!   TUI asks for what it missed as it subscribes
//!   (`ClientRequest::SlackBacklog`) — a TUI that was closed while the
//!   daemon listened gets it all.
//! - **History**: the socket open is stamped every [`BEAT`]
//!   (`slack_listened`). A sign-in more than [`GAP`] after the last stamp
//!   reads the gap back from Slack's history with the user token
//!   ([`catch_up`]): `conversations.history` of the conversations the
//!   inbox saw in the last fortnight and of your direct messages
//!   (`users.conversations`, when the app has `im:read`/`mpim:read`), and
//!   `conversations.replies` of the threads you posted in and of any
//!   thread history shows a reply in. Each message not kept yet is kept
//!   and broadcast as the socket would have, oldest first per
//!   conversation. Calls go a [`PACE`] apart, under Slack's 50 a minute,
//!   and stop at [`MAX_CONVERSATIONS`] and [`MAX_THREADS`].

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::api::As;
use super::Session;
use crate::registry::Daemon;

/// How often the open socket is stamped.
pub const BEAT: Duration = Duration::from_secs(30);
/// A gap shorter than this is not read back: Slack retries an event it
/// could not deliver for about that long.
pub const GAP: Duration = Duration::from_secs(90);
/// The pause between two history calls.
const PACE: Duration = Duration::from_millis(1300);
/// How long a rate-limited call waits before its one retry.
const RATE_LIMITED_WAIT: Duration = Duration::from_secs(30);
/// The most conversations one catch-up reads.
const MAX_CONVERSATIONS: usize = 60;
/// The most threads one catch-up reads.
const MAX_THREADS: usize = 40;
/// How far back the inbox's conversations and threads count as live: as
/// far as it keeps them.
const LIVE_FOR_SECS: f64 = crate::store::SLACK_INBOX_KEEP_MS as f64 / 1000.0;
/// How many beats apart the inbox is pruned: hourly, so a daemon that a
/// patch release keeps running for weeks doesn't grow it without end.
const PRUNE_EVERY: u64 = 120;

/// Stamp the socket open, every [`BEAT`], for as long as it is polled.
pub async fn heartbeat(daemon: &Daemon) {
    let mut beat = 0u64;
    loop {
        beat = beat.wrapping_add(1);
        let now = orion_core::clock::now_ms();
        if let Err(e) = daemon.store.set_slack_listened(now) {
            tracing::warn!(error = %e, "slack listening not stamped");
        }
        if beat.is_multiple_of(PRUNE_EVERY) {
            let _ = daemon
                .store
                .prune_slack_inbox(now - crate::store::SLACK_INBOX_KEEP_MS);
        }
        tokio::time::sleep(BEAT).await;
    }
}

/// Signed in: when the last stamp is more than [`GAP`] old, read the gap
/// back from history, in the background. Call before [`heartbeat`]
/// stamps anew.
pub fn after_gap(daemon: &Arc<Daemon>, session: &Arc<Session>) {
    let Ok(Some(last)) = daemon.store.slack_listened() else {
        return;
    };
    let now = orion_core::clock::now_ms();
    if now - last < GAP.as_millis() as i64 {
        return;
    }
    // From a beat before the stamp: the socket may have died just after it.
    let from_ms = last - BEAT.as_millis() as i64;
    tracing::info!(
        gap_secs = (now - last) / 1000,
        "slack: reading back the gap"
    );
    let (daemon, session) = (daemon.clone(), session.clone());
    tokio::spawn(async move { catch_up(daemon, session, from_ms).await });
}

/// Read back everything said since `from_ms` (epoch ms) in the
/// conversations and threads that matter (see the module doc), keeping
/// and broadcasting each message not kept yet.
pub async fn catch_up(daemon: Arc<Daemon>, session: Arc<Session>, from_ms: i64) {
    let oldest = slack_ts(from_ms);
    let live_since = from_ms as f64 / 1000.0 - LIVE_FOR_SECS;
    let mut channels = daemon
        .store
        .slack_channels_since(live_since)
        .unwrap_or_default();
    if let Some(direct) = call(&session, "users.conversations", move |api| {
        api.call_form(
            "users.conversations",
            As::User,
            &[
                ("types", "im,mpim"),
                ("exclude_archived", "true"),
                ("limit", "200"),
            ],
        )
    })
    .await
    {
        for c in direct["channels"].as_array().into_iter().flatten() {
            let Some(id) = c["id"].as_str() else { continue };
            let kind = if c["is_mpim"].as_bool() == Some(true) {
                "mpim"
            } else {
                "im"
            };
            channels.push((id.to_string(), kind.to_string()));
        }
    }
    let channels = conversations(channels, &session.dm);
    let mut threads: Vec<(String, String)> = daemon
        .store
        .slack_threads_of(&session.me, live_since)
        .unwrap_or_default();
    let mut caught = 0usize;
    for (channel, kind) in &channels {
        let (c, o) = (channel.clone(), oldest.clone());
        let Some(reply) = call(&session, "conversations.history", move |api| {
            api.call_form(
                "conversations.history",
                As::User,
                &[("channel", &c), ("oldest", &o), ("limit", "200")],
            )
        })
        .await
        else {
            continue;
        };
        let said = messages(&reply, channel, kind);
        for event in &said {
            if let Some(thread) = replied_since(event, &oldest) {
                threads.insert(0, (channel.clone(), thread));
            }
        }
        caught += keep(&daemon, said);
    }
    let mut seen = std::collections::HashSet::new();
    threads.retain(|t| seen.insert(t.clone()));
    for (channel, thread) in threads.into_iter().take(MAX_THREADS) {
        let kind = channels
            .iter()
            .find(|(c, _)| *c == channel)
            .map(|(_, k)| k.clone())
            .unwrap_or_else(|| kind_of(&channel).to_string());
        let (c, t, o) = (channel.clone(), thread.clone(), oldest.clone());
        let Some(reply) = call(&session, "conversations.replies", move |api| {
            api.call_form(
                "conversations.replies",
                As::User,
                &[
                    ("channel", &c),
                    ("ts", &t),
                    ("oldest", &o),
                    ("limit", "200"),
                ],
            )
        })
        .await
        else {
            continue;
        };
        caught += keep(&daemon, messages(&reply, &channel, &kind));
    }
    tracing::info!(
        caught,
        conversations = channels.len(),
        "slack: caught up on the gap"
    );
}

/// The conversations to read: each once, the phone's DM (the bot's, not
/// yours to answer) left out, at most [`MAX_CONVERSATIONS`].
fn conversations(channels: Vec<(String, String)>, phone: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (channel, kind) in channels {
        if channel == phone || out.iter().any(|(c, _)| *c == channel) {
            continue;
        }
        let kind = if kind.is_empty() {
            kind_of(&channel).to_string()
        } else {
            kind
        };
        out.push((channel, kind));
    }
    out.truncate(MAX_CONVERSATIONS);
    out
}

/// A conversation's `channel_type` from its id, when nothing better is
/// known: `D…` a direct message, `G…` a private one, else a channel.
fn kind_of(channel: &str) -> &'static str {
    match channel.chars().next() {
        Some('D') => "im",
        Some('G') => "group",
        _ => "channel",
    }
}

/// `from_ms` as a Slack `ts`.
fn slack_ts(from_ms: i64) -> String {
    format!("{}.{:06}", from_ms / 1000, (from_ms % 1000) * 1000)
}

/// A history or replies answer's messages as the message events the
/// socket would have sent — `channel` and `channel_type` added — oldest
/// first.
fn messages(reply: &Value, channel: &str, kind: &str) -> Vec<Value> {
    let mut out: Vec<Value> = reply["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["ts"].is_string())
        .map(|m| {
            let mut event = m.clone();
            event["type"] = "message".into();
            event["channel"] = channel.into();
            event["channel_type"] = kind.into();
            event
        })
        .collect();
    out.sort_by(|a, b| ts_of(a).total_cmp(&ts_of(b)));
    out
}

fn ts_of(event: &Value) -> f64 {
    event["ts"]
        .as_str()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0.0)
}

/// The thread `event` starts, when it has a reply newer than `oldest`:
/// those replies are not in the channel's history.
fn replied_since(event: &Value, oldest: &str) -> Option<String> {
    let latest: f64 = event["latest_reply"].as_str()?.parse().ok()?;
    let oldest: f64 = oldest.parse().ok()?;
    (latest > oldest).then(|| event["thread_ts"].as_str().map(str::to_string))?
}

/// Keep each of `events` not kept yet and broadcast it: how many were new.
fn keep(daemon: &Daemon, events: Vec<Value>) -> usize {
    events
        .iter()
        .filter(|event| super::keep_and_forward(daemon, event))
        .count()
}

/// One history call, a [`PACE`] after the last, off the async workers:
/// its answer, or None (logged) — after one retry when rate-limited.
async fn call(
    session: &Arc<Session>,
    method: &'static str,
    run: impl Fn(&super::api::Api) -> Result<Value, String> + Clone + Send + 'static,
) -> Option<Value> {
    for attempt in 0..2 {
        tokio::time::sleep(PACE).await;
        let (api, run) = (session.api.clone(), run.clone());
        let answer = tokio::task::spawn_blocking(move || run(&api)).await.ok()?;
        match answer {
            Ok(reply) => return Some(reply),
            Err(e) if e == super::api::RATELIMITED && attempt == 0 => {
                tokio::time::sleep(RATE_LIMITED_WAIT).await;
            }
            Err(e) => {
                // A token without `im:read` can't list direct messages:
                // the ones the inbox saw are read all the same.
                tracing::info!(error = %e, method, "slack catch-up call failed");
                return None;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn history_reads_as_the_sockets_events_oldest_first() {
        let reply = json!({"ok": true, "messages": [
            {"type": "message", "user": "U2", "text": "later", "ts": "200.000002"},
            {"type": "message", "user": "U2", "text": "first", "ts": "100.000001",
             "thread_ts": "100.000001", "reply_count": 2, "latest_reply": "250.000000"},
        ]});
        let events = messages(&reply, "D9", "im");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["text"], "first");
        assert_eq!(
            (&events[0]["channel"], &events[0]["channel_type"]),
            (&json!("D9"), &json!("im"))
        );
        assert_eq!(
            replied_since(&events[0], "150.000000"),
            Some("100.000001".into())
        );
        assert_eq!(replied_since(&events[0], "260.000000"), None);
        assert_eq!(replied_since(&events[1], "0"), None, "no thread");
        let ServerEvent::SlackMessage { channel, ts, .. } =
            crate::slack::inbox_event(&events[1]).unwrap()
        else {
            panic!()
        };
        assert_eq!((channel.as_str(), ts.as_str()), ("D9", "200.000002"));
    }

    #[test]
    fn conversations_are_read_once_each_without_the_phones_dm() {
        let got = conversations(
            vec![
                ("C1".into(), "channel".into()),
                ("D0PHONE".into(), "im".into()),
                ("C1".into(), "channel".into()),
                ("D2".into(), String::new()),
                ("G3".into(), String::new()),
            ],
            "D0PHONE",
        );
        assert_eq!(
            got,
            vec![
                ("C1".to_string(), "channel".to_string()),
                ("D2".into(), "im".into()),
                ("G3".into(), "group".into()),
            ]
        );
    }

    #[test]
    fn a_gap_starts_at_its_ms() {
        assert_eq!(slack_ts(1_795_462_555_335), "1795462555.335000");
    }

    use orion_core::ServerEvent;
}
