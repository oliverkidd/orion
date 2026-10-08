//! The check behind the FOOTER's update indicator: is a newer orion
//! published on GitHub than the one running?
//!
//! "Latest" is resolved the way `install.sh` resolves it — GitHub's
//! `releases/latest` URL answers with a redirect to `/releases/tag/vX.Y.Z`
//! (drafts and pre-releases excluded), so one `HEAD` request names the
//! newest release. That is a plain web redirect, not an API call: it spends
//! no `gh` token (the Claude sessions on this box share that token with the
//! pull-request poll) and none of the API quota. The probe is `curl`, which
//! `install.sh` and `orion upgrade` already require.
//!
//! The indicator is a nudge, not a clock: a check runs at start and then on
//! a slow beat, off the event loop, and every failure — no network, no
//! curl, a redirect that is not a release tag — is "no news", not an error
//! worth a flash. Nothing here installs anything; `orion upgrade` does.

use std::time::Duration;

/// GitHub's "latest release" page for this repo.
pub const LATEST_URL: &str = "https://github.com/oliverkidd/orion/releases/latest";

/// How often the check re-runs once the TUI is up; the first runs at start.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// How long the probe may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The check's cadence: `ORION_UPDATE_CHECK_SECS` when set, `None` when it
/// is `0` (off — the e2e tests, whose footers must not depend on what
/// GitHub has published), else [`DEFAULT_INTERVAL`].
pub fn interval() -> Option<Duration> {
    orion_core::env::secs_override(orion_core::env::UPDATE_CHECK_SECS, DEFAULT_INTERVAL)
}

/// What one check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// A published release (`0.22.0`) strictly newer than this build.
    Newer(String),
    /// This build is the latest release, or a dev build ahead of it.
    Current,
    /// The check couldn't ask, or GitHub's answer wasn't a release tag.
    Unknown,
}

/// One check's answer to the loop. `asked` marks the check **Upgrade
/// orion** ran on demand, whose answer is shown whatever it is; the slow
/// beat's answers only ever light the indicator, so a re-check that can't
/// ask never clears one an earlier check lit (a release does not
/// un-publish).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: Status,
    pub asked: bool,
}

/// Run one check off the loop; `tx` hears what it found.
pub fn spawn(tx: tokio::sync::mpsc::UnboundedSender<Answer>, asked: bool) {
    tokio::spawn(async move {
        let status = check().await;
        let _ = tx.send(Answer { status, asked });
    });
}

/// Ask GitHub for the latest release and compare it with this build.
pub async fn check() -> Status {
    let Some(redirect) = probe(LATEST_URL).await else {
        return Status::Unknown;
    };
    let Some(tag) = tag_from_redirect(&redirect) else {
        return Status::Unknown;
    };
    if parse_version(tag).is_none() {
        return Status::Unknown;
    }
    match newer_than(tag, env!("CARGO_PKG_VERSION")) {
        Some(version) => Status::Newer(version),
        None => Status::Current,
    }
}

/// Where `url` redirects to, without following it — GitHub's answer to
/// `releases/latest` is the redirect itself. Not a redirect, and every
/// failure, are `None`.
async fn probe(url: &str) -> Option<String> {
    let mut cmd = tokio::process::Command::new("curl");
    cmd.args([
        "-sS",
        "-I", // HEAD: the Location header is the whole answer
        "-o",
        "/dev/null",
        "-w",
        "%{redirect_url}",
        "--max-time",
        "15",
        url,
    ])
    .stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let redirect = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!redirect.is_empty()).then_some(redirect)
}

/// The tag a `…/releases/tag/<tag>` URL names; `None` for any other shape
/// (a repo with no releases yet redirects to `/releases`).
pub fn tag_from_redirect(url: &str) -> Option<&str> {
    let (_, tag) = url.split_once("/releases/tag/")?;
    let tag = tag.trim_end_matches('/');
    (!tag.is_empty()).then_some(tag)
}

/// `vX.Y.Z` / `X.Y.Z` as its three numbers; anything else — a pre-release
/// suffix, a two-part tag — is `None`, so it never compares.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// `published` normalised to `X.Y.Z` when it is strictly newer than
/// `running`.
pub fn newer_than(published: &str, running: &str) -> Option<String> {
    let p = parse_version(published)?;
    let r = parse_version(running)?;
    (p > r).then(|| format!("{}.{}.{}", p.0, p.1, p.2))
}

/// Whether upgrading from `running` to `published` restarts the DAEMON,
/// and every session in it, by the release numbering: a release that
/// changes the daemon's code is a minor one (x.Y.0) and a major is never a
/// patch, so a jump that moves only the patch number keeps the daemon
/// running. A version that doesn't parse — a dev build's, say — reads as a
/// restart, the side a warning should err on.
pub fn restarts_daemon(published: &str, running: &str) -> bool {
    match (parse_version(published), parse_version(running)) {
        (Some(p), Some(r)) => (p.0, p.1) != (r.0, r.1),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redirect_names_the_tag() {
        let base = "https://github.com/oliverkidd/orion";
        assert_eq!(
            tag_from_redirect(&format!("{base}/releases/tag/v0.22.0")),
            Some("v0.22.0")
        );
        assert_eq!(
            tag_from_redirect(&format!("{base}/releases/tag/v0.22.0/")),
            Some("v0.22.0")
        );
        assert_eq!(
            tag_from_redirect(&format!("{base}/releases")),
            None,
            "no releases yet"
        );
        assert_eq!(tag_from_redirect(""), None);
    }

    #[test]
    fn versions_parse_with_or_without_the_v() {
        assert_eq!(parse_version("v0.22.0"), Some((0, 22, 0)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(
            parse_version("v0.22.0-rc1"),
            None,
            "pre-releases never compare"
        );
        assert_eq!(parse_version("v0.22"), None);
        assert_eq!(parse_version("v0.22.0.1"), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn only_a_strictly_newer_release_counts() {
        assert_eq!(newer_than("v0.22.0", "0.21.0").as_deref(), Some("0.22.0"));
        assert_eq!(newer_than("v1.0.0", "0.99.9").as_deref(), Some("1.0.0"));
        assert_eq!(
            newer_than("v0.21.10", "0.21.9").as_deref(),
            Some("0.21.10"),
            "numeric, not lexical"
        );
        assert_eq!(newer_than("v0.21.0", "0.21.0"), None, "up to date");
        assert_eq!(
            newer_than("v0.21.0", "0.22.0"),
            None,
            "a dev build ahead of the last release"
        );
        assert_eq!(newer_than("garbage", "0.21.0"), None);
    }

    #[test]
    fn only_a_patch_jump_keeps_the_daemon() {
        assert!(!restarts_daemon("0.22.3", "0.22.0"), "patches only");
        assert!(restarts_daemon("0.23.0", "0.22.4"), "a minor changed it");
        assert!(restarts_daemon("0.23.1", "0.22.0"), "across a minor");
        assert!(restarts_daemon("1.0.0", "0.22.0"), "a major");
        assert!(
            restarts_daemon("0.22.1", "dev"),
            "unknown errs on a restart"
        );
    }
}
