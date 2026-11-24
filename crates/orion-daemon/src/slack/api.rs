//! The Slack Web API calls the phone bridge makes, each through
//! `orion_core::http` — the token on curl's stdin, never its argv. They
//! block: async callers run them under `spawn_blocking`.

use serde_json::{json, Value};

use super::Tokens;

/// The API's base URL (`orion_core::http::slack_api_url`, which the TUI's
/// Needs-you board calls too).
pub fn base_url() -> String {
    orion_core::http::slack_api_url()
}

/// The error a call Slack answered 429 to reads as.
pub const RATELIMITED: &str = "ratelimited";

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

    fn token(&self, who: As) -> &str {
        match who {
            As::App => &self.tokens.app,
            As::Bot => &self.tokens.bot,
            As::User => &self.tokens.user,
        }
    }

    /// POST `method` with a JSON body as `who`. Slack answers 200 with
    /// `ok: false` and an `error` code for most failures; that code is the
    /// error here (`invalid_auth`, `channel_not_found`), as is a non-2xx
    /// status or curl failing to reach Slack at all.
    pub fn call(&self, method: &str, who: As, body: &Value) -> Result<Value, String> {
        let token = self.token(who);
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

    /// POST `method` as `who` with form fields — for the methods that take
    /// no JSON body (`users.lookupByEmail`, the `files.*` uploads). Its
    /// reply reads as [`Api::call`]'s.
    pub fn call_form(
        &self,
        method: &str,
        who: As,
        fields: &[(&str, &str)],
    ) -> Result<Value, String> {
        let token = self.token(who);
        let url = format!("{}/{method}", self.base);
        let headers = ["Content-Type: application/x-www-form-urlencoded".to_string()];
        let body = form(fields);
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

    /// `users.lookupByEmail` as the bot: the Slack id of whoever has
    /// `email` in the workspace. Nobody is Slack's `users_not_found`; a
    /// bot without `users:read.email` is `missing_scope`.
    pub fn lookup_by_email(&self, email: &str) -> Result<String, String> {
        let reply = self.call_form("users.lookupByEmail", As::Bot, &[("email", email)])?;
        match reply["user"]["id"].as_str() {
            Some(id) => Ok(id.to_string()),
            None => Err("users.lookupByEmail gave no user".into()),
        }
    }

    /// Upload `bytes` as `filename` into the thread `thread_ts` of
    /// `channel`, as the bot, the way Slack now wants it: ask for an
    /// upload URL (`files.getUploadURLExternal`), send the bytes there,
    /// then share the file (`files.completeUploadExternal`). A bot
    /// without `files:write` is `missing_scope`.
    pub fn upload(
        &self,
        channel: &str,
        thread_ts: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let length = bytes.len().to_string();
        let ticket = self.call_form(
            "files.getUploadURLExternal",
            As::Bot,
            &[("filename", filename), ("length", &length)],
        )?;
        let (url, id) = (str_at(&ticket, "upload_url"), str_at(&ticket, "file_id"));
        if url.is_empty() || id.is_empty() {
            return Err("files.getUploadURLExternal gave no upload_url".into());
        }
        let headers = ["Content-Type: application/octet-stream".to_string()];
        let sent = orion_core::http::request(&orion_core::http::Request {
            method: "POST",
            url: &url,
            headers: &headers,
            bearer: None,
            body: Some(bytes),
            timeout_secs: 60,
        })?;
        if !sent.ok() {
            return Err(format!("the upload answered HTTP {}", sent.status));
        }
        let files = json!([{ "id": id, "title": filename }]).to_string();
        self.call_form(
            "files.completeUploadExternal",
            As::Bot,
            &[
                ("files", &files),
                ("channel_id", channel),
                ("thread_ts", thread_ts),
            ],
        )
        .map(drop)
    }

    /// `views.open` as the bot: `view` as a modal, for the press whose
    /// `trigger_id` this is (it lasts three seconds).
    pub fn views_open(&self, trigger_id: &str, view: &Value) -> Result<(), String> {
        let body = json!({ "trigger_id": trigger_id, "view": view });
        self.call("views.open", As::Bot, &body).map(drop)
    }
}

/// A Web API reply read: the JSON when `ok`, else Slack's error code.
fn reply(method: &str, status: u16, body: &[u8]) -> Result<Value, String> {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    match parsed {
        Some(v) if v["ok"].as_bool() == Some(true) => Ok(v),
        Some(v) if v["error"].is_string() => Err(v["error"].as_str().unwrap_or_default().into()),
        _ if status == 429 => Err(RATELIMITED.into()),
        _ => Err(format!("{method} answered HTTP {status}")),
    }
}

/// `fields` as an `application/x-www-form-urlencoded` body.
fn form(fields: &[(&str, &str)]) -> String {
    let encode = |text: &str| {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                b' ' => "+".to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
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

    #[test]
    fn a_form_body_is_url_encoded() {
        assert_eq!(
            form(&[
                ("email", "ann+qa@example.com"),
                ("files", r#"[{"id":"F 1"}]"#)
            ]),
            "email=ann%2Bqa%40example.com&files=%5B%7B%22id%22%3A%22F+1%22%7D%5D"
        );
    }

    /// Slack's real endpoint, with a token it has never issued: the error
    /// comes back through `orion_core::http` as Slack's own code. Needs
    /// the network, so a machine without it skips.
    #[test]
    #[ignore = "reaches slack.com"]
    fn slack_refuses_a_bogus_token() {
        let api = Api::new(
            orion_core::http::SLACK_API.into(),
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
