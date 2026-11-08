//! SECRETS: tokens orion keeps in the macOS Keychain, as generic passwords
//! through `/usr/bin/security`. Orion's own items sit under services named
//! `dev.orion.<service>` ([`get`], [`set`], [`delete`]); another app's
//! item — Claude Code's sign-in, Cursor's key — is read as it is named
//! ([`read_item`]).
//!
//! It lives in `orion-core` so the TUI and the daemon read the same
//! items: the daemon's phone bridge reads the Slack tokens the
//! Connections tab saved.

use std::process::{Command, Stdio};

/// The `security` tool. Claude Code saves its items with it too, so its
/// items let orion read them without a prompt.
const SECURITY: &str = "/usr/bin/security";

/// What every one of orion's own services starts with.
pub const PREFIX: &str = "dev.orion.";

/// The Slack app's tokens (`dev.orion.slack`): the Connections tab saves
/// them, the daemon's Slack bridge reads them.
pub const SLACK_SERVICE: &str = "slack";
/// Their accounts, in the order the Connections tab asks for them: the
/// app-level token (`xapp-…`), the bot's (`xoxb-…`), the user's (`xoxp-…`).
pub const SLACK_ACCOUNTS: [&str; 3] = ["app", "bot", "user"];

/// The Keychain service orion keeps `service` under: `dev.orion.<service>`.
pub fn service_name(service: &str) -> String {
    format!("{PREFIX}{service}")
}

/// Orion's secret `account` of `service` — `get("slack", "bot")` — or None
/// when it was never saved, or there is no Keychain to ask.
pub fn get(service: &str, account: &str) -> Option<String> {
    read_item(&service_name(service), Some(account))
}

/// Save `value` as orion's secret `account` of `service`, replacing one
/// already there. The error is the one line worth showing.
///
/// The command goes to `security -i` on stdin, never on argv, so the
/// token never shows in `ps`.
pub fn set(service: &str, account: &str, value: &str) -> Result<(), String> {
    let line = format!(
        "add-generic-password -U -s {} -a {} -w {}\n",
        quoted(&service_name(service)),
        quoted(account),
        quoted(value),
    );
    let mut child = Command::new(SECURITY)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "the Keychain could not be reached".to_string())?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin
            .write_all(line.as_bytes())
            .map_err(|_| "the Keychain could not be reached".to_string())?;
    }
    let out = child
        .wait_with_output()
        .map_err(|_| "the Keychain could not be reached".to_string())?;
    // `security -i` exits 0 even when a command in it fails; a failure
    // still prints its complaint.
    let why = String::from_utf8_lossy(&out.stderr);
    match why.lines().map(str::trim).find(|l| !l.is_empty()) {
        None if out.status.success() => Ok(()),
        Some(line) => Err(line.to_string()),
        None => Err("the Keychain refused".to_string()),
    }
}

/// One argument for a `security -i` command line: double-quoted, with
/// `\` and `"` escaped.
fn quoted(arg: &str) -> String {
    format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Forget orion's secret `account` of `service`. One that was never saved
/// is already forgotten.
pub fn delete(service: &str, account: &str) {
    let _ = run(&[
        "delete-generic-password",
        "-s",
        &service_name(service),
        "-a",
        account,
    ]);
}

/// Any generic password's secret, by its full service name and, when
/// given, its account. A secret saved as something other than plain text
/// comes back decoded from the hex `security -w` prints it as.
pub fn read_item(service: &str, account: Option<&str>) -> Option<String> {
    let mut cmd = Command::new(SECURITY);
    cmd.args(["find-generic-password", "-s", service]);
    if let Some(account) = account {
        cmd.args(["-a", account]);
    }
    let out = cmd.arg("-w").stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| unhex(text).unwrap_or_else(|| text.to_string()))
}

/// Run `security` with `args`; its first line of complaint on failure.
fn run(args: &[&str]) -> Result<(), String> {
    let out = Command::new(SECURITY)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "the Keychain could not be reached".to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr);
    Err(why
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("the Keychain refused")
        .to_string())
}

/// `security -w` prints a secret that is not plain text as hex: a JSON
/// blob saved that way comes back as `7b22…`.
fn unhex(text: &str) -> Option<String> {
    if !text.len().is_multiple_of(2) || !text.starts_with("7b") {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orions_services_carry_its_prefix() {
        assert_eq!(service_name("slack"), "dev.orion.slack");
    }

    #[test]
    fn a_json_blob_comes_back_from_hex() {
        assert_eq!(unhex("7b7d").as_deref(), Some("{}"));
        assert_eq!(unhex("plain"), None);
        assert_eq!(unhex("7b7"), None);
    }

    /// A real Keychain round trip under a throwaway service. Skipped where
    /// there is no `security` (CI on Linux), or no Keychain to write to.
    #[test]
    fn a_secret_is_saved_read_and_forgotten() {
        if !std::path::Path::new(SECURITY).exists() {
            return;
        }
        let service = format!("test-{}", ulid::Ulid::generate());
        if set(&service, "me", "s3cret").is_err() {
            return;
        }
        assert_eq!(get(&service, "me").as_deref(), Some("s3cret"));
        // Quotes, a backslash and spaces survive the trip through stdin.
        set(&service, "me", r#"xoxb-"a" \b c"#).unwrap();
        assert_eq!(get(&service, "me").as_deref(), Some(r#"xoxb-"a" \b c"#));
        delete(&service, "me");
        assert_eq!(get(&service, "me"), None);
        delete(&service, "me");
    }
}
