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
//! a slow beat ([`Beat`]), off the event loop, and every failure — no
//! network, no curl, a redirect that is not a release tag — is "no news",
//! not an error worth a flash. Nothing here installs anything; `orion
//! upgrade` does.

use std::time::{Duration, Instant};

/// GitHub's "latest release" page for this repo.
pub const LATEST_URL: &str = "https://github.com/oliverkidd/orion/releases/latest";

/// How often the check re-runs once the TUI is up; the first runs at start.
/// Releases land a few times a day at most, and **Upgrade orion** asks
/// afresh whenever it is opened, so a slower beat only delays the nudge.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// How long the probe may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The check's cadence: `ORION_UPDATE_CHECK_SECS` when set, `None` when it
/// is `0` (off — the e2e tests, whose footers must not depend on what
/// GitHub has published), else [`DEFAULT_INTERVAL`].
pub fn interval() -> Option<Duration> {
    orion_core::env::secs_override(orion_core::env::UPDATE_CHECK_SECS, DEFAULT_INTERVAL)
}

/// The slow beat: when the next background check runs. The first is due
/// at once; each one run puts the next an interval on.
#[derive(Debug, Clone)]
pub struct Beat {
    next: Instant,
    every: Duration,
}

impl Beat {
    /// A beat whose first check is due at `now`.
    pub fn starting(now: Instant, every: Duration) -> Self {
        Beat { next: now, every }
    }

    /// When the next check should run, or None while none should be
    /// armed:
    ///
    /// - once a newer release is `known` — a release never un-publishes, so
    ///   asking again could only light what is already lit (a release newer
    ///   still is found by **Upgrade orion**'s own ask, or the next launch);
    /// - while `idle` (unfocused, or away) — nobody sees the indicator, and
    ///   a check a sleeping laptop wakes for is a network request for no
    ///   one. The deadline is kept, not moved: the first turn somebody looks
    ///   again finds it past and runs the check then.
    pub fn deadline(&self, known: bool, idle: bool) -> Option<Instant> {
        (!known && !idle).then_some(self.next)
    }

    /// A check just ran at `now`: the next is an interval on.
    pub fn ran(&mut self, now: Instant) {
        self.next = now + self.every;
    }
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

    const HOUR: Duration = Duration::from_secs(60 * 60);

    #[test]
    fn the_beat_checks_at_launch_then_every_six_hours() {
        let start = Instant::now();
        let mut beat = Beat::starting(start, DEFAULT_INTERVAL);
        assert_eq!(beat.deadline(false, false), Some(start), "at launch");
        beat.ran(start);
        assert_eq!(beat.deadline(false, false), Some(start + 6 * HOUR));
    }

    #[test]
    fn a_known_release_ends_the_beat() {
        let start = Instant::now();
        let beat = Beat::starting(start, DEFAULT_INTERVAL);
        assert_eq!(beat.deadline(true, false), None);
        assert_eq!(beat.deadline(true, true), None);
    }

    /// Idle holds a due check rather than skipping it: back from away, the
    /// deadline is the one that passed, so the check runs on that turn.
    #[test]
    fn an_idle_beat_waits_for_somebody_to_look() {
        let start = Instant::now();
        let mut beat = Beat::starting(start, DEFAULT_INTERVAL);
        beat.ran(start);
        assert_eq!(beat.deadline(false, true), None, "away: not armed");
        let back = beat.deadline(false, false).unwrap();
        assert_eq!(back, start + 6 * HOUR, "the passed deadline, due at once");
        beat.ran(start + 9 * HOUR);
        assert_eq!(beat.deadline(false, false), Some(start + 15 * HOUR));
    }

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
