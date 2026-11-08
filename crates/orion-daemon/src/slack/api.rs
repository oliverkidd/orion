//! The Slack Web API calls the phone bridge makes, each through
//! `orion_core::http` — the token on curl's stdin, never its argv. They
//! block: async callers run them under `spawn_blocking`.

use serde_json::{json, Value};

use super::Tokens;

/// Slack's own Web API.
pub const DEFAULT_BASE: &str = "https://slack.com/api";

/// The API's base URL: [`DEFAULT_BASE`], or `ORION_SLACK_API_URL` (a fake
/// Slack in the e2e tests).
pub fn base_url() -> String {
    orion_core::env::non_empty(orion_core::env::SLACK_API_URL)
        .map(|url| url.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
}

/// Which of the three tokens a call goes out with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum As {
    /// `xapp-…`: opens the Socket Mode connection.
    App,
    /// `xoxb-…`: Orion's bot — the phone DMs.
    Bot,
    /// `xoxp-…`: the installing user — who they are.
    User,
}

/// What `auth.test` says a token is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub user_id: String,
    pub team: String,
}

/// One workspace's Web API, with its tokens.
#[derive(Debug, Clone)]
pub struct Api {
    base: String,
    tokens: Tokens,
}

impl Api {
    pub fn new(base: String, tokens: Tokens) -> Self {
        Self { base, tokens }
    }

    /// POST `method` with a JSON body as `who`. Slack answers 200 with
    /// `ok: false` and an `error` code for most failures; that code is the
    /// error here (`invalid_auth`, `channel_not_found`), as is a non-2xx
    /// status or curl failing to reach Slack at all.
    pub fn call(&self, method: &str, who: As, body: &Value) -> Result<Value, String> {
        let token = match who {
            As::App => &self.tokens.app,
            As::Bot => &self.tokens.bot,
            As::User => &self.tokens.user,
        };
        let url = format!("{}/{method}", self.base);
        let headers = ["Content-Type: application/json; charset=utf-8".to_string()];
        let body = body.to_string();
        let response = orion_core::http::request(&orion_core::http::Request {
            method: "POST",
            url: &url,
            headers: &headers,
            bearer: Some(token),
            body: Some(body.as_bytes()),
            timeout_secs: 15,
        })?;
        reply(method, response.status, &response.body)
    }

    /// `auth.test`: who `who`'s token belongs to, in which workspace.
    pub fn auth_test(&self, who: As) -> Result<Identity, String> {
        let reply = self.call("auth.test", who, &json!({}))?;
        Ok(Identity {
            user_id: str_at(&reply, "user_id"),
            team: str_at(&reply, "team"),
        })
    }

    /// `apps.connections.open`: the websocket URL for one Socket Mode
    /// connection.
    pub fn connections_open(&self) -> Result<String, String> {
        let reply = self.call("apps.connections.open", As::App, &json!({}))?;
        match str_at(&reply, "url") {
            url if url.is_empty() => Err("apps.connections.open gave no url".into()),
            url => Ok(url),
        }
    }

    /// `conversations.open`: the bot's DM channel with `user`.
    pub fn open_dm(&self, user: &str) -> Result<String, String> {
        let reply = self.call("conversations.open", As::Bot, &json!({ "users": user }))?;
        match reply["channel"]["id"].as_str() {
            Some(id) => Ok(id.to_string()),
            None => Err("conversations.open gave no channel".into()),
        }
    }

    /// `chat.postMessage` as the bot: its `ts`. `text` is the
    /// notification's line, `blocks` what shows.
    pub fn post(
        &self,
        channel: &str,
        text: &str,
        blocks: Option<&[Value]>,
        thread_ts: Option<&str>,
    ) -> Result<String, String> {
        let mut body = json!({ "channel": channel, "text": text, "unfurl_links": false });
        if let Some(blocks) = blocks {
            body["blocks"] = Value::from(blocks.to_vec());
        }
        if let Some(thread) = thread_ts {
            body["thread_ts"] = thread.into();
        }
        let reply = self.call("chat.postMessage", As::Bot, &body)?;
        Ok(str_at(&reply, "ts"))
    }

    /// `chat.update`: the bot's message `ts` rewritten.
    pub fn update(
        &self,
        channel: &str,
        ts: &str,
        text: &str,
        blocks: &[Value],
    ) -> Result<(), String> {
        let body = json!({ "channel": channel, "ts": ts, "text": text, "blocks": blocks });
        self.call("chat.update", As::Bot, &body).map(drop)
    }

    /// `chat.postEphemeral`: a line only `user` sees, in `thread_ts`'s
    /// thread when given.
    pub fn post_ephemeral(
        &self,
        channel: &str,
        user: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<(), String> {
        let mut body = json!({ "channel": channel, "user": user, "text": text });
        if let Some(thread) = thread_ts {
            body["thread_ts"] = thread.into();
        }
        self.call("chat.postEphemeral", As::Bot, &body).map(drop)
    }
}

/// A Web API reply read: the JSON when `ok`, else Slack's error code.
fn reply(method: &str, status: u16, body: &[u8]) -> Result<Value, String> {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    match parsed {
        Some(v) if v["ok"].as_bool() == Some(true) => Ok(v),
        Some(v) if v["error"].is_string() => Err(v["error"].as_str().unwrap_or_default().into()),
        _ if status == 429 => Err("ratelimited".into()),
        _ => Err(format!("{method} answered HTTP {status}")),
    }
}

fn str_at(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_is_its_json_or_slacks_error_code() {
        assert_eq!(
            reply("auth.test", 200, br#"{"ok":true,"user_id":"U1"}"#).unwrap()["user_id"],
            "U1"
        );
        assert_eq!(
            reply("auth.test", 200, br#"{"ok":false,"error":"invalid_auth"}"#),
            Err("invalid_auth".into())
        );
        assert_eq!(reply("x", 429, b""), Err("ratelimited".into()));
        assert_eq!(
            reply("x", 502, b"<html>"),
            Err("x answered HTTP 502".into())
        );
    }

    /// Slack's real endpoint, with a token it has never issued: the error
    /// comes back through `orion_core::http` as Slack's own code. Needs
    /// the network, so a machine without it skips.
    #[test]
    #[ignore = "reaches slack.com"]
    fn slack_refuses_a_bogus_token() {
        let api = Api::new(
            DEFAULT_BASE.into(),
            Tokens {
                app: "xapp-1-bogus".into(),
                bot: "xoxb-bogus".into(),
                user: "xoxp-bogus".into(),
            },
        );
        assert_eq!(api.auth_test(As::Bot), Err("invalid_auth".into()));
        assert_eq!(api.connections_open(), Err("invalid_auth".into()));
    }
}
