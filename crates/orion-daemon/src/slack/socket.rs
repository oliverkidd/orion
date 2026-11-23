//! SOCKET MODE: one websocket to Slack, from the URL
//! `apps.connections.open` hands out, carrying every event and button
//! press for the app. Each envelope is handed on and acknowledged the
//! moment it arrives — Slack sends it again otherwise. The handler's
//! answer rides the acknowledgement as its `payload`: for a modal's
//! submit, that is the modal's reply (errors to show, or nothing to close
//! it). A `disconnect` (Slack refreshes connections every few hours) ends
//! the session cleanly, and the caller opens the next one.
//!
//! A socket can die without a word: the Mac sleeps, or the network under
//! it changes, and the connection is left half-open — nothing more ever
//! arrives, and nothing says so, while the Web API still posts. Slack
//! pings a live socket regularly, so one quiet for
//! [`Liveness::quiet`] is pinged, and one that still says nothing within
//! [`Liveness::answer`] has dropped: the caller opens the next one.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

/// How long a socket may say nothing before it is taken for dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liveness {
    /// Nothing for this long, and the socket is pinged.
    pub quiet: Duration,
    /// Nothing for this long after the ping, and it has dropped.
    pub answer: Duration,
}

impl Default for Liveness {
    fn default() -> Self {
        Liveness {
            quiet: Duration::from_secs(20),
            answer: Duration::from_secs(10),
        }
    }
}

/// What a socket that stopped answering ended with.
pub const WENT_QUIET: &str = "the Slack socket went quiet: no answer to a ping";

/// One envelope's content: `events_api` (a subscribed event),
/// `interactive` (a button press) and the rest, with its payload.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub kind: String,
    pub payload: Value,
}

/// How a session ended without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Slack asked for a new connection (`disconnect`): open the next one
    /// at once.
    Refresh,
    /// The caller stopped it — shutdown, or new tokens.
    Stopped,
}

/// Read `url`'s websocket until it ends, handing every envelope to
/// `on_envelope` and acknowledging it with what that returns as the ack's
/// `payload` (None, a bare ack). `on_envelope` must not block: the ack
/// waits on it. `stop` resolving ends it as [`Ended::Stopped`]. The error
/// is the line to show: the socket would not open, dropped, or went quiet
/// past `liveness`.
pub async fn session(
    url: &str,
    liveness: Liveness,
    stop: impl std::future::Future<Output = ()>,
    mut on_envelope: impl FnMut(Envelope) -> Option<Value>,
) -> Result<Ended, String> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("the Slack socket would not open: {e}"))?;
    tracing::info!("slack socket open");
    let (mut tx, mut rx) = ws.split();
    tokio::pin!(stop);
    let mut pinged = false;
    loop {
        let wait = match pinged {
            true => liveness.answer,
            false => liveness.quiet,
        };
        let frame = tokio::select! {
            _ = &mut stop => {
                let _ = tx.send(Message::Close(None)).await;
                return Ok(Ended::Stopped);
            }
            frame = tokio::time::timeout(wait, rx.next()) => frame,
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(_) if pinged => return Err(WENT_QUIET.into()),
            Err(_) => {
                tx.send(Message::Ping(Default::default()))
                    .await
                    .map_err(|e| format!("the Slack socket dropped: {e}"))?;
                pinged = true;
                continue;
            }
        };
        // Anything at all — a ping, a pong, an envelope — is a live socket.
        pinged = false;
        let text = match frame {
            None => return Err("the Slack socket closed".into()),
            Some(Err(e)) => return Err(format!("the Slack socket dropped: {e}")),
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) => return Err("Slack closed the socket".into()),
            // Pings are answered by tungstenite itself; a pong only
            // says the socket is alive.
            Some(Ok(_)) => continue,
        };
        let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let reply = match frame["type"].as_str() {
            Some("hello") | None => None,
            Some("disconnect") => return Ok(Ended::Refresh),
            Some(kind) => on_envelope(Envelope {
                kind: kind.to_string(),
                payload: frame["payload"].clone(),
            }),
        };
        if let Some(ack) = ack_for(&frame, reply) {
            tx.send(Message::Text(ack.into()))
                .await
                .map_err(|e| format!("the Slack socket dropped: {e}"))?;
        }
    }
}

/// The acknowledgement a frame needs: its `envelope_id` sent straight
/// back, with `payload` when there is one. None for a frame that carries
/// no id (`hello`, `disconnect`).
pub fn ack_for(frame: &Value, payload: Option<Value>) -> Option<String> {
    let id = frame["envelope_id"].as_str()?;
    let mut ack = json!({ "envelope_id": id });
    if let Some(payload) = payload {
        ack["payload"] = payload;
    }
    Some(ack.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: Liveness = Liveness {
        quiet: Duration::from_millis(150),
        answer: Duration::from_millis(150),
    };

    /// A local websocket that says hello and then nothing more, reading
    /// what comes (so pings get their pongs) only when `answers`; its URL.
    async fn quiet_server(answers: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (mut tx, mut rx) = ws.split();
            tx.send(Message::Text(r#"{"type":"hello"}"#.into()))
                .await
                .unwrap();
            match answers {
                // Reading answers each ping with a pong.
                true => while let Some(Ok(_)) = rx.next().await {},
                // Held open, never read: a half-open connection.
                false => std::future::pending::<()>().await,
            }
            drop(tx);
        });
        url
    }

    /// A socket that goes silent — the Mac slept, the network moved — is
    /// pinged, and with no answer it has dropped, so the bridge opens the
    /// next one instead of waiting on it for ever.
    #[tokio::test]
    async fn a_socket_that_stops_answering_has_dropped() {
        let url = quiet_server(false).await;
        let ended = tokio::time::timeout(
            Duration::from_secs(5),
            session(&url, FAST, std::future::pending(), |_| None),
        )
        .await
        .expect("a dead socket was waited on for ever");
        assert_eq!(ended, Err(WENT_QUIET.to_string()));
    }

    /// One that answers its pings stays up through many quiet spells.
    #[tokio::test]
    async fn a_quiet_socket_that_answers_its_pings_stays_up() {
        let url = quiet_server(true).await;
        let stop = tokio::time::sleep(Duration::from_millis(1500));
        assert_eq!(
            session(&url, FAST, stop, |_| None).await,
            Ok(Ended::Stopped)
        );
    }

    #[test]
    fn every_envelope_is_acknowledged_by_its_id() {
        let frame = json!({"envelope_id": "e-1", "type": "events_api", "payload": {}});
        assert_eq!(
            ack_for(&frame, None).as_deref(),
            Some(r#"{"envelope_id":"e-1"}"#)
        );
        assert_eq!(ack_for(&json!({"type": "hello"}), None), None);
        // A modal's reply rides its submit's ack.
        let errors = json!({"response_action": "errors", "errors": {"b": "no"}});
        assert_eq!(
            serde_json::from_str::<Value>(&ack_for(&frame, Some(errors.clone())).unwrap()).unwrap(),
            json!({"envelope_id": "e-1", "payload": errors})
        );
    }
}
