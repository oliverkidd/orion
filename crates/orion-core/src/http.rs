//! HTTP: every web API orion calls itself — Linear, the usage providers,
//! Slack, Microsoft Graph — goes through one `curl`, with nothing secret
//! on its argv. The URL, the headers and the bearer token travel on
//! curl's stdin as a `--config -` file, and a body sits in a temp file
//! only this user can read, removed when the request ends.
//!
//! It blocks: an async caller runs it under `spawn_blocking`. curl's own
//! `--max-time` bounds it.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// One request.
#[derive(Debug, Clone, Default)]
pub struct Request<'a> {
    /// `GET`, `POST`, …; empty is curl's own choice (GET, or POST with a
    /// body).
    pub method: &'a str,
    pub url: &'a str,
    /// Whole header lines: `Accept: application/json`.
    pub headers: &'a [String],
    /// Sent as `Authorization: Bearer <token>`.
    pub bearer: Option<&'a str>,
    pub body: Option<&'a [u8]>,
    /// How long curl may take, start to finish. Zero is 20 seconds.
    pub timeout_secs: u64,
}

/// What came back: the HTTP status, and the body whatever the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    /// A 2xx.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Slack's own Web API.
pub const SLACK_API: &str = "https://slack.com/api";

/// The Slack Web API's base URL: [`SLACK_API`], or `ORION_SLACK_API_URL`
/// (a fake Slack in the e2e tests).
pub fn slack_api_url() -> String {
    crate::env::non_empty(crate::env::SLACK_API_URL)
        .map(|url| url.trim_end_matches('/').to_string())
        .unwrap_or_else(|| SLACK_API.to_string())
}

/// How long a request may take when it names no timeout.
const DEFAULT_TIMEOUT_SECS: u64 = 20;

/// Send `req` through curl. The error is curl's own first line — the
/// host not reached, the time running out — or that curl would not run;
/// an HTTP error status is a [`Response`], for the caller to read.
pub fn request(req: &Request) -> Result<Response, String> {
    let body = match req.body {
        Some(bytes) => Some(BodyFile::write(bytes)?),
        None => None,
    };
    let timeout = match req.timeout_secs {
        0 => DEFAULT_TIMEOUT_SECS,
        secs => secs,
    }
    .to_string();
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--max-time", &timeout, "--config", "-"]);
    cmd.args(["-w", "\n%{http_code}"]);
    if !req.method.is_empty() {
        cmd.args(["-X", req.method]);
    }
    if let Some(body) = &body {
        cmd.arg("--data-binary")
            .arg(format!("@{}", body.0.display()));
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config(req).as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("curl failed")
            .to_string());
    }
    let (body, status) = split_status(&out.stdout);
    Ok(Response {
        status,
        body: body.to_vec(),
    })
}

/// The `--config -` file curl reads on stdin: the URL and every header,
/// the bearer token among them.
fn config(req: &Request) -> String {
    let mut config = format!("url = \"{}\"\n", config_escape(req.url));
    let bearer = req
        .bearer
        .map(|token| format!("Authorization: Bearer {token}"));
    for header in req.headers.iter().chain(bearer.as_ref()) {
        config.push_str(&format!("header = \"{}\"\n", config_escape(header)));
    }
    config
}

/// curl's `-w "\n%{http_code}"` tail off the body.
fn split_status(out: &[u8]) -> (&[u8], u16) {
    let (body, tail) = match out.iter().rposition(|&b| b == b'\n') {
        Some(at) => (&out[..at], &out[at + 1..]),
        None => (&out[..0], out),
    };
    let code = std::str::from_utf8(tail)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    (body, code)
}

/// A value inside a curl config file's double quotes.
fn config_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A request body on disk for curl's `--data-binary @file`, readable by
/// this user alone and removed on drop.
struct BodyFile(PathBuf);

impl BodyFile {
    fn write(bytes: &[u8]) -> Result<BodyFile, String> {
        let path = std::env::temp_dir().join(format!(
            "orion-http-{}-{}.body",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
        let file = BodyFile(path);
        open.open(&file.0)
            .and_then(|mut f| f.write_all(bytes))
            .map_err(|e| e.to_string())?;
        Ok(file)
    }
}

impl Drop for BodyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_comes_off_the_tail() {
        assert_eq!(split_status(b"{\"a\":1}\n200"), (&b"{\"a\":1}"[..], 200));
        assert_eq!(split_status(b"\n429"), (&b""[..], 429));
        assert_eq!(split_status(b"garbage"), (&b""[..], 0));
    }

    #[test]
    fn the_token_and_headers_go_on_stdin_escaped() {
        let headers = ["Accept: application/json".to_string()];
        let config = config(&Request {
            url: "https://x.test/a\"b",
            headers: &headers,
            bearer: Some("t0k"),
            ..Request::default()
        });
        assert_eq!(
            config,
            "url = \"https://x.test/a\\\"b\"\n\
             header = \"Accept: application/json\"\n\
             header = \"Authorization: Bearer t0k\"\n"
        );
    }

    #[test]
    fn a_body_file_is_private_and_gone_after() {
        let file = BodyFile::write(b"{}").unwrap();
        let path = file.0.clone();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        drop(file);
        assert!(!path.exists());
    }
}
