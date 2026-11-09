//! Signing in to Microsoft 365 with the DEVICE-CODE FLOW, and keeping an
//! access token after.
//!
//! orion has no redirect URI to catch a browser sign-in on, so it asks
//! Entra for a short code ([`DeviceCode`]), shows it, opens the page it is
//! typed into, and polls the token endpoint until the user has signed in
//! there ([`Poll`]). The refresh token that comes back goes in the
//! Keychain (`dev.orion.microsoft` / `refresh`); every later Graph call
//! trades it for an access token ([`access_token`]), kept in memory for
//! the hour it lasts. Entra rotates the refresh token on each trade, and
//! the new one replaces the old.
//!
//! Every request is a form POST through [`orion_core::http`], so nothing
//! secret reaches curl's argv.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{encode, REFRESH, SERVICE};
use crate::config::MicrosoftSettings;

/// What orion asks for: the user, their calendar, their online meetings
/// and those meetings' transcripts, and a refresh token to keep asking.
pub const SCOPES: &str =
    "offline_access User.Read Calendars.Read OnlineMeetings.Read OnlineMeetingTranscript.Read.All";

/// The device-code grant, as the token endpoint names it.
const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// How long before an access token's end orion stops trusting it.
const EXPIRY_MARGIN: Duration = Duration::from_secs(120);

/// What Entra hands back for a sign-in to start: the code the user types,
/// the page they type it on, and how long and how often to poll.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    pub user_code: String,
    pub device_code: String,
    pub verification_uri: String,
    /// Seconds the code is good for.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// A token endpoint's success.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    /// Absent only if `offline_access` was not granted.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Seconds the access token is good for.
    #[serde(default)]
    pub expires_in: u64,
}

/// An OAuth error body: `{"error": "...", "error_description": "AADSTS…"}`.
#[derive(Debug, Clone, Deserialize)]
struct OauthError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// One poll of the token endpoint during a sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Not signed in in the browser yet: ask again after the interval.
    Pending,
    /// Asked too often: ask again, five seconds slower.
    SlowDown,
    Done(Tokens),
    /// The code ran out before anyone used it.
    Expired,
    /// The user said no in the browser.
    Declined,
    Failed(String),
}

/// What a sign-in does after a poll: wait this long and poll again, or
/// stop with the tokens or the reason it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Wait(Duration),
    Finish(Result<Tokens, String>),
}

/// A finished sign-in's word for a code nobody used in time.
pub const EXPIRED: &str = "the code ran out before it was used · c tries again";
/// …and for a sign-in refused in the browser.
pub const DECLINED: &str = "sign-in was declined in the browser";

impl Poll {
    /// What the sign-in does next, polling every `interval` so far.
    pub fn next(self, interval: Duration) -> Next {
        match self {
            Poll::Pending => Next::Wait(interval),
            Poll::SlowDown => Next::Wait(interval + Duration::from_secs(5)),
            Poll::Done(tokens) => Next::Finish(Ok(tokens)),
            Poll::Expired => Next::Finish(Err(EXPIRED.into())),
            Poll::Declined => Next::Finish(Err(DECLINED.into())),
            Poll::Failed(why) => Next::Finish(Err(why)),
        }
    }
}

/// `https://login.microsoftonline.com/<tenant>/oauth2/v2.0/<endpoint>`.
fn endpoint(ms: &MicrosoftSettings, which: &str) -> String {
    let tenant = match ms.tenant.trim() {
        "" => "organizations",
        tenant => tenant,
    };
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/{which}")
}

/// A form body, each value encoded.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// POST `body` as a form to `url`; the status and the body back.
fn post_form(url: &str, body: &str) -> Result<orion_core::http::Response, String> {
    let headers = ["Content-Type: application/x-www-form-urlencoded".to_string()];
    orion_core::http::request(&orion_core::http::Request {
        method: "POST",
        url,
        headers: &headers,
        body: Some(body.as_bytes()),
        ..Default::default()
    })
}

/// Ask Entra for a sign-in code (blocking).
pub fn request_code(ms: &MicrosoftSettings) -> Result<DeviceCode, String> {
    let body = form(&[("client_id", ms.client_id.trim()), ("scope", SCOPES)]);
    let response = post_form(&endpoint(ms, "devicecode"), &body)?;
    parse_device_code(response.status, &response.body)
}

/// The devicecode endpoint's answer: the code, or why there is none.
pub fn parse_device_code(status: u16, body: &[u8]) -> Result<DeviceCode, String> {
    if (200..300).contains(&status) {
        return serde_json::from_slice(body)
            .map_err(|e| format!("Microsoft's sign-in code didn't read: {e}"));
    }
    Err(error_text(status, body))
}

/// Poll the token endpoint once for `code` (blocking).
pub fn poll_once(ms: &MicrosoftSettings, code: &DeviceCode) -> Poll {
    let body = form(&[
        ("grant_type", GRANT),
        ("client_id", ms.client_id.trim()),
        ("device_code", &code.device_code),
    ]);
    match post_form(&endpoint(ms, "token"), &body) {
        Ok(response) => parse_poll(response.status, &response.body),
        Err(why) => Poll::Failed(why),
    }
}

/// The token endpoint's answer to a device-code poll.
pub fn parse_poll(status: u16, body: &[u8]) -> Poll {
    if (200..300).contains(&status) {
        return match serde_json::from_slice(body) {
            Ok(tokens) => Poll::Done(tokens),
            Err(e) => Poll::Failed(format!("Microsoft's tokens didn't read: {e}")),
        };
    }
    match serde_json::from_slice::<OauthError>(body) {
        Ok(err) => match err.error.as_str() {
            "authorization_pending" => Poll::Pending,
            "slow_down" => Poll::SlowDown,
            "expired_token" | "code_expired" => Poll::Expired,
            "authorization_declined" | "access_denied" => Poll::Declined,
            _ => Poll::Failed(explain(&err)),
        },
        Err(_) => Poll::Failed(error_text(status, body)),
    }
}

/// Trade a refresh token for tokens (blocking).
fn refresh(ms: &MicrosoftSettings, refresh_token: &str) -> Result<Tokens, String> {
    let body = form(&[
        ("grant_type", "refresh_token"),
        ("client_id", ms.client_id.trim()),
        ("refresh_token", refresh_token),
        ("scope", SCOPES),
    ]);
    let response = post_form(&endpoint(ms, "token"), &body)?;
    if !response.ok() {
        return Err(error_text(response.status, &response.body));
    }
    serde_json::from_slice(&response.body)
        .map_err(|e| format!("Microsoft's tokens didn't read: {e}"))
}

/// An error body as the one line worth showing.
fn error_text(status: u16, body: &[u8]) -> String {
    match serde_json::from_slice::<OauthError>(body) {
        Ok(err) => explain(&err),
        Err(_) => format!("Microsoft answered {status}"),
    }
}

/// An OAuth error in words. The AADSTS codes that mean the app id in
/// `config.json` is wrong, or the app isn't set up for this flow, say what
/// to fix; any other is Entra's own first sentence, without the trace ids
/// it appends.
fn explain(err: &OauthError) -> String {
    let code = err
        .error_description
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find_map(|word| word.strip_prefix("AADSTS"))
        .unwrap_or("");
    match code {
        // Not an app id; no such app; no tenant to find it in.
        "700038" | "700016" | "50059" | "90002" => {
            "Microsoft doesn't know these app IDs · o clears them, then c to add them again".into()
        }
        // "Allow public client flows" is off on the app.
        "7000218" => {
            "turn on Allow public client flows for the Orion app in Entra (see setup)".into()
        }
        // A permission nobody consented to.
        "65001" => "an admin must grant consent to the Orion app in Entra (see setup)".into(),
        _ => {
            let text = err
                .error_description
                .split(" Trace ID:")
                .next()
                .unwrap_or_default()
                .trim();
            match text {
                "" => err.error.clone(),
                text => text.to_string(),
            }
        }
    }
}

/// The access token in hand, and when it stops being good.
static ACCESS: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Keep `tokens`: the refresh token in the Keychain, the access token in
/// memory.
pub fn keep(tokens: &Tokens) -> Result<(), String> {
    if let Some(refresh) = tokens.refresh_token.as_deref() {
        orion_core::secrets::set(SERVICE, REFRESH, refresh)?;
    }
    let lasts = Duration::from_secs(tokens.expires_in.max(60));
    if let Ok(mut access) = ACCESS.lock() {
        *access = Some((tokens.access_token.clone(), Instant::now() + lasts));
    }
    Ok(())
}

/// A Graph access token (blocking): the one in memory while it is good,
/// else a fresh one for the refresh token in the Keychain. `fresh` drops
/// the one in memory first — what a Graph 401 asks for.
/// `ORION_MICROSOFT_TOKEN` stands in for all of it in the tests.
pub fn access_token(ms: &MicrosoftSettings, fresh: bool) -> Result<String, String> {
    if let Some(token) = orion_core::env::non_empty(orion_core::env::MICROSOFT_TOKEN) {
        return Ok(token);
    }
    if let Ok(mut access) = ACCESS.lock() {
        if fresh {
            *access = None;
        }
        if let Some((token, until)) = access.as_ref() {
            if Instant::now() + EXPIRY_MARGIN < *until {
                return Ok(token.clone());
            }
        }
    }
    let refresh_token =
        orion_core::secrets::get(SERVICE, REFRESH).ok_or_else(|| "not signed in".to_string())?;
    let tokens = refresh(ms, &refresh_token)?;
    keep(&tokens)?;
    Ok(tokens.access_token)
}

/// Forget the sign-in: the Keychain's refresh token and email, and the
/// access token in memory.
pub fn forget() {
    // A test's `o` must not sign the developer's own orion out.
    #[cfg(not(test))]
    {
        orion_core::secrets::delete(SERVICE, REFRESH);
        orion_core::secrets::delete(SERVICE, super::EMAIL);
    }
    if let Ok(mut access) = ACCESS.lock() {
        *access = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Microsoft's documented devicecode success.
    const CODE: &str = r#"{
        "user_code": "FBQR3SHV9",
        "device_code": "GAQABAAEAAAD--DLA3VO7QrddgJg7WevrJ",
        "verification_uri": "https://microsoft.com/devicelogin",
        "expires_in": 900,
        "interval": 5,
        "message": "To sign in, use a web browser to open the page https://microsoft.com/devicelogin and enter the code FBQR3SHV9 to authenticate."
    }"#;

    fn error(code: &str, description: &str) -> Vec<u8> {
        serde_json::json!({
            "error": code,
            "error_description": description,
            "error_codes": [70016],
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_device_code_reads() {
        let code = parse_device_code(200, CODE.as_bytes()).unwrap();
        assert_eq!(code.user_code, "FBQR3SHV9");
        assert_eq!(code.verification_uri, "https://microsoft.com/devicelogin");
        assert_eq!((code.expires_in, code.interval), (900, 5));
    }

    #[test]
    fn an_unknown_app_id_says_what_to_fix() {
        // What the real endpoint answers for a client id that is no app
        // (checked by hand against login.microsoftonline.com).
        let body = br#"{"error":"unauthorized_client","error_description":"AADSTS700038: 00000000-0000-0000-0000-000000000000 is not a valid application identifier. Trace ID: 88f0dd96 Correlation ID: 678a0f31 Timestamp: 2026-11-08 17:05:49Z","error_codes":[700038]}"#;
        let why = parse_device_code(400, body).unwrap_err();
        assert_eq!(
            why,
            "Microsoft doesn't know these app IDs · o clears them, then c to add them again"
        );
        let body = br#"{"error":"invalid_request","error_description":"AADSTS50059: No tenant-identifying information found in either the request or implied by any provided credentials. Trace ID: 8f0e Correlation ID: 6cca Timestamp: 2026-11-08 17:05:53Z","error_codes":[50059]}"#;
        assert!(parse_device_code(400, body)
            .unwrap_err()
            .contains("app IDs"));
    }

    #[test]
    fn another_error_is_its_first_sentence_without_trace_ids() {
        let body = error(
            "invalid_scope",
            "AADSTS70011: The provided value for scope is not valid. Trace ID: abc Correlation ID: def",
        );
        assert_eq!(
            parse_device_code(400, &body).unwrap_err(),
            "AADSTS70011: The provided value for scope is not valid."
        );
        assert_eq!(
            parse_device_code(502, b"<html>bad gateway</html>").unwrap_err(),
            "Microsoft answered 502"
        );
    }

    #[test]
    fn polling_walks_pending_slow_down_success_and_expiry() {
        let interval = Duration::from_secs(5);
        let pending = parse_poll(
            400,
            &error(
                "authorization_pending",
                "AADSTS70016: OAuth 2.0 device flow error. Authorization is pending.",
            ),
        );
        assert_eq!(pending, Poll::Pending);
        assert_eq!(pending.next(interval), Next::Wait(interval));

        let slow = parse_poll(400, &error("slow_down", ""));
        assert_eq!(slow.next(interval), Next::Wait(Duration::from_secs(10)));

        let done = parse_poll(
            200,
            br#"{"token_type":"Bearer","scope":"User.Read Calendars.Read","expires_in":3599,"ext_expires_in":3599,"access_token":"eyJ0eXAi","refresh_token":"AwABAAAA","id_token":"eyJ0eXAi"}"#,
        );
        let Next::Finish(Ok(tokens)) = done.next(interval) else {
            panic!("a success finishes");
        };
        assert_eq!(tokens.access_token, "eyJ0eXAi");
        assert_eq!(tokens.refresh_token.as_deref(), Some("AwABAAAA"));
        assert_eq!(tokens.expires_in, 3599);

        let expired = parse_poll(
            400,
            &error("expired_token", "AADSTS70020: The provided value for the input parameter 'device_code' is not valid. This device code has expired."),
        );
        assert_eq!(expired.next(interval), Next::Finish(Err(EXPIRED.into())));

        let declined = parse_poll(400, &error("authorization_declined", ""));
        assert_eq!(declined.next(interval), Next::Finish(Err(DECLINED.into())));
    }

    #[test]
    fn a_form_encodes_its_values() {
        assert_eq!(
            form(&[("scope", "offline_access User.Read"), ("x", "a&b=c")]),
            "scope=offline_access%20User.Read&x=a%26b%3Dc"
        );
    }

    #[test]
    fn the_tenant_defaults_to_organizations() {
        let ms = MicrosoftSettings {
            tenant: String::new(),
            ..Default::default()
        };
        assert_eq!(
            endpoint(&ms, "token"),
            "https://login.microsoftonline.com/organizations/oauth2/v2.0/token"
        );
    }

    /// The real devicecode endpoint, asked with a client id that is no
    /// app: its error must read as the app-id hint. Network, so by hand:
    /// `cargo test -p orion-tui microsoft::auth -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_endpoint_refuses_a_bogus_app_id_readably() {
        let ms = MicrosoftSettings {
            client_id: "00000000-0000-0000-0000-000000000000".into(),
            ..Default::default()
        };
        let why = request_code(&ms).unwrap_err();
        eprintln!("{why}");
        assert!(why.contains("app IDs"), "{why}");
    }
}
