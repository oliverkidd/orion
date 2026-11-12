//! SOCKET MODE: one websocket to Slack, from the URL
//! `apps.connections.open` hands out, carrying every event and button
//! press for the app. Each envelope is handed on and acknowledged the
//! moment it arrives — Slack sends it again otherwise. The handler's
//! answer rides the acknowledgement as its `payload`: for a modal's
//! submit, that is the modal's reply (errors to show, or nothing to close
//! it). A `disconnect` (Slack refreshes connections every few hours) ends
//! the session cleanly, and the caller opens the next one.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

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
/// is the line to show: the socket would not open, or dropped.
pub async fn session(
    url: &str,
    stop: impl std::future::Future<Output = ()>,
    mut on_envelope: impl FnMut(Envelope) -> Option<Value>,
) -> Result<Ended, String> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("the Slack socket would not open: {e}"))?;
    let (mut tx, mut rx) = ws.split();
    tokio::pin!(stop);
    loop {
        let frame = tokio::select! {
            _ = &mut stop => {
                let _ = tx.send(Message::Close(None)).await;
                return Ok(Ended::Stopped);
            }
            frame = rx.next() => frame,
        };
        let text = match frame {
            None => return Err("the Slack socket closed".into()),
            Some(Err(e)) => return Err(format!("the Slack socket dropped: {e}")),
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) => return Err("Slack closed the socket".into()),
            // Pings are answered by tungstenite itself.
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
