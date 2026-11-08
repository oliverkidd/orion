//! MICROSOFT GRAPH reads: a GET of `https://graph.microsoft.com/v1.0/…`
//! under the signed-in user's access token ([`super::auth`]), through
//! [`orion_core::http`]. A 401 — the token went stale early — is asked
//! again once with a fresh one.
//!
//! Every call blocks: run it under `spawn_blocking`.

use serde::de::DeserializeOwned;

use super::auth;
use crate::config::MicrosoftSettings;

/// Graph's root.
pub const ROOT: &str = "https://graph.microsoft.com/v1.0";

/// GET `path` (`/me/calendarView?…`, from [`ROOT`]) with `headers`
/// besides the token; the body of a 2xx, else the error in words.
pub fn get(ms: &MicrosoftSettings, path: &str, headers: &[String]) -> Result<Vec<u8>, String> {
    let url = format!("{ROOT}{path}");
    let mut fresh = false;
    loop {
        let token = auth::access_token(ms, fresh)?;
        let response = orion_core::http::request(&orion_core::http::Request {
            method: "GET",
            url: &url,
            headers,
            bearer: Some(&token),
            ..Default::default()
        })?;
        match response.status {
            401 if !fresh => fresh = true,
            _ if response.ok() => return Ok(response.body),
            status => return Err(error_text(status, &response.body)),
        }
    }
}

/// [`get`], the body read as JSON.
pub fn get_json<T: DeserializeOwned>(
    ms: &MicrosoftSettings,
    path: &str,
    headers: &[String],
) -> Result<T, String> {
    let body = get(ms, path, headers)?;
    serde_json::from_slice(&body).map_err(|e| format!("Microsoft Graph's answer didn't read: {e}"))
}

/// Graph's error body, `{"error": {"code": …, "message": …}}`, as its
/// message.
fn error_text(status: u16, body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("Microsoft Graph answered {status}"))
}

/// Who is signed in: `/me`'s mail, else their sign-in name.
pub fn email(ms: &MicrosoftSettings) -> Result<String, String> {
    let me: serde_json::Value = get_json(ms, "/me?$select=mail,userPrincipalName", &[])?;
    parse_email(&me).ok_or_else(|| "Microsoft Graph didn't say who you are".to_string())
}

fn parse_email(me: &serde_json::Value) -> Option<String> {
    ["mail", "userPrincipalName"]
        .iter()
        .filter_map(|key| me.get(key)?.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_email_is_mail_else_the_sign_in_name() {
        let me = serde_json::json!({"mail": "ada@riplo.ai", "userPrincipalName": "ada@riplo.onmicrosoft.com"});
        assert_eq!(parse_email(&me).as_deref(), Some("ada@riplo.ai"));
        let me =
            serde_json::json!({"mail": null, "userPrincipalName": "ada@riplo.onmicrosoft.com"});
        assert_eq!(
            parse_email(&me).as_deref(),
            Some("ada@riplo.onmicrosoft.com")
        );
    }

    #[test]
    fn an_error_is_graphs_message() {
        let body = br#"{"error":{"code":"InvalidAuthenticationToken","message":"Access token has expired or is not yet valid."}}"#;
        assert_eq!(
            error_text(401, body),
            "Access token has expired or is not yet valid."
        );
        assert_eq!(error_text(503, b""), "Microsoft Graph answered 503");
    }
}
