//! THE GATE: how a provider's failures are told apart, and the pause a
//! rate limit puts on the whole provider.
//!
//! GitHub's limit is the one orion runs into: 5,000 GraphQL points an hour
//! per token, shared with every Claude session's own `gh`. Once it is
//! spent every call fails until the hour resets, and a fetch that retried
//! on its own backoff kept spending what was left the moment it came back
//! (and said `couldn't refresh` meanwhile, with no word of why). So every
//! `gh` orion runs comes through [`github`]: a failure that reads as a rate
//! limit asks GitHub when it resets (`gh api rate_limit`, which costs
//! nothing), and every GitHub call — the RESOLVER's fetches and a user's
//! own merge alike — waits until then without spawning a process. Pausing
//! and resuming are logged once each, at warn.
//!
//! Linear, Microsoft Graph and the usage endpoints are asked only by the
//! RESOLVER: a rate limit in what one of them says ([`failure`]) pauses
//! its provider in the resolver, for a while of its own — Linear and Graph
//! ten minutes, a usage endpoint's 429 an hour.
//!
//! The decisions are pure functions ([`classify`], [`pause_for`],
//! [`Pause`]); the process-wide pause and the process spawning are a thin
//! layer over them.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Why a fetch has no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The provider is rate limited until `until`: nothing is asked of it
    /// before then.
    RateLimited { until: Instant },
    /// It answered with an error: what it printed.
    Failed(String),
    /// Nothing could ask: the tool isn't installed, it timed out, or the
    /// checkout to ask from is gone.
    Missing,
}

impl FetchError {
    /// What a caller that only wants `gh`'s complaint gets: the stderr of
    /// a failed call, nothing for a call that never ran, and a sentence
    /// for a pause (which no `gh` message test will mistake for an
    /// answer).
    pub fn into_stderr(self) -> String {
        match self {
            FetchError::Failed(stderr) => stderr,
            FetchError::Missing => String::new(),
            FetchError::RateLimited { .. } => "GitHub rate limit: paused until it resets".into(),
        }
    }

    /// One line for the log and the UI.
    pub fn summary(&self) -> String {
        match self {
            FetchError::RateLimited { .. } => "rate limited".into(),
            FetchError::Failed(stderr) => crate::pull_request::first_line(stderr)
                .map_or_else(|| "failed".into(), |line| line.chars().take(120).collect()),
            FetchError::Missing => "couldn't ask".into(),
        }
    }
}

/// What a failed `gh`'s stderr says about the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// It names a rate limit — primary (`API rate limit exceeded`,
    /// GraphQL's `RATE_LIMITED`) or secondary — or is an HTTP 429.
    RateLimit,
    /// A bare HTTP 403: a spent limit answers with one, and so does a
    /// token that may not see the repo. The limit endpoint tells them
    /// apart ([`pause_for`]).
    MaybeLimit,
    /// Any other failure.
    Other,
}

/// Sort `stderr` from a failed `gh` into a [`Signal`].
pub fn classify(stderr: &str) -> Signal {
    let lower = stderr.to_ascii_lowercase();
    let names_limit = ["rate limit", "rate_limit", "ratelimit"]
        .iter()
        .any(|m| lower.contains(m));
    if names_limit || lower.contains("http 429") {
        Signal::RateLimit
    } else if lower.contains("http 403") {
        Signal::MaybeLimit
    } else {
        Signal::Other
    }
}

/// How long a secondary limit (too much at once, rather than too much
/// this hour) holds GitHub off: GitHub asks for at least a minute.
pub const SECONDARY_PAUSE: Duration = Duration::from_secs(60);

/// How long a rate limit holds GitHub off when its reset time can't be
/// learned.
pub const UNKNOWN_PAUSE: Duration = Duration::from_secs(5 * 60);

/// How long to pause GitHub after a failure that `signal` describes, given
/// what `gh api rate_limit` said (`None` when it couldn't say) at `now_secs`
/// epoch seconds. A spent resource pauses until the latest reset among the
/// spent ones; a named limit with none spent is a secondary limit; a bare
/// 403 with none spent was never a limit at all (`None`: no pause).
pub fn pause_for(signal: Signal, limits: Option<&str>, now_secs: u64) -> Option<Duration> {
    if signal == Signal::Other {
        return None;
    }
    let Some(limits) = limits.and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
    else {
        return (signal == Signal::RateLimit).then_some(UNKNOWN_PAUSE);
    };
    let spent_reset = limits
        .get("resources")
        .and_then(|r| r.as_object())
        .into_iter()
        .flat_map(|r| r.values())
        .filter(|r| r.get("remaining").and_then(|n| n.as_u64()) == Some(0))
        .filter_map(|r| r.get("reset").and_then(|n| n.as_u64()))
        .max();
    match spent_reset {
        // A second's slack: the reset is GitHub's clock, not ours.
        Some(reset) => Some(Duration::from_secs(reset.saturating_sub(now_secs) + 1)),
        None => (signal == Signal::RateLimit).then_some(SECONDARY_PAUSE),
    }
}

/// A provider's pause: until when nothing is asked of it.
#[derive(Debug, Default)]
pub struct Pause {
    until: Option<Instant>,
}

impl Pause {
    /// Pause until `until` (never shortening one already longer). True
    /// when this starts a pause rather than extending one — the moment to
    /// say so in the log.
    pub fn set(&mut self, until: Instant, now: Instant) -> bool {
        let started = self.until.is_none_or(|was| was <= now);
        self.until = Some(self.until.map_or(until, |was| was.max(until)));
        started
    }

    /// The pause still running at `now`, and whether one just ran out
    /// (the moment to say it has lifted, once).
    pub fn check(&mut self, now: Instant) -> (Option<Instant>, bool) {
        match self.until {
            Some(until) if until > now => (Some(until), false),
            Some(_) => {
                self.until = None;
                (None, true)
            }
            None => (None, false),
        }
    }
}

/// GitHub's pause, process-wide: one `gh` hitting the limit stops them all.
static GITHUB_PAUSE: Mutex<Pause> = Mutex::new(Pause { until: None });

fn github_pause() -> std::sync::MutexGuard<'static, Pause> {
    GITHUB_PAUSE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Until when GitHub is paused, if it is. Says once in the log that a
/// pause has lifted, the first time it is asked after.
pub fn github_paused_until() -> Option<Instant> {
    let (until, lifted) = github_pause().check(Instant::now());
    if lifted {
        tracing::warn!("GitHub rate limit reset: resuming GitHub calls");
    }
    until
}

/// Pause every GitHub call for `wait`.
fn pause_github(wait: Duration) -> Instant {
    let now = Instant::now();
    let until = now + wait;
    if github_pause().set(until, now) {
        tracing::warn!(
            resumes_in_secs = wait.as_secs(),
            "GitHub rate limit: pausing every GitHub call until it resets"
        );
    }
    until
}

/// Run `gh` with `args` (in `dir` when given) under `timeout`, through the
/// gate: nothing is spawned while GitHub is paused, and a failure that
/// reads as a rate limit pauses it.
pub async fn github(
    dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<String, FetchError> {
    if let Some(until) = github_paused_until() {
        return Err(FetchError::RateLimited { until });
    }
    match spawn_gh(dir, args, timeout).await {
        Ok(out) => Ok(out),
        Err(None) => Err(FetchError::Missing),
        Err(Some(stderr)) => Err(judge(stderr, timeout).await),
    }
}

/// [`github`] for a `gh` command built elsewhere — one that writes its
/// request on stdin, or a test's stand-in for `gh` — under the same
/// pause: nothing is spawned while GitHub is paused, and a failure that
/// reads as a rate limit pauses it.
pub async fn github_piped(
    cmd: tokio::process::Command,
    stdin: &str,
    timeout: Duration,
) -> Result<String, FetchError> {
    if let Some(until) = github_paused_until() {
        return Err(FetchError::RateLimited { until });
    }
    match crate::pull_request::run_piped(cmd, stdin, timeout).await {
        Ok(out) => Ok(out),
        Err(stderr) => Err(judge(stderr, timeout).await),
    }
}

/// What a failed `gh` that printed `stderr` comes to: a failure of its
/// own, or — when it reads as a rate limit and GitHub's limits agree — a
/// pause of every GitHub call.
async fn judge(stderr: String, timeout: Duration) -> FetchError {
    let signal = classify(&stderr);
    if signal == Signal::Other {
        return FetchError::Failed(stderr);
    }
    // Another call may have paused GitHub while this one ran.
    if let Some(until) = github_paused_until() {
        return FetchError::RateLimited { until };
    }
    let Some(limits) = probe_limits(timeout).await else {
        // A probe that ran meanwhile found the limit: its pause stands.
        return match github_paused_until() {
            Some(until) => FetchError::RateLimited { until },
            None => FetchError::Failed(stderr),
        };
    };
    let now_secs = orion_core::clock::now_secs();
    match pause_for(signal, limits.as_deref(), now_secs) {
        Some(wait) => FetchError::RateLimited {
            until: pause_github(wait),
        },
        None => FetchError::Failed(stderr),
    }
}

/// How long one `gh api rate_limit` answer serves every other failure
/// that asks: a burst of 403s is one probe.
const PROBE_REUSE: Duration = Duration::from_secs(10);

/// The last probe's answer and when it came.
static PROBED: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);

/// One probe at a time ([`probe_limits`]).
static PROBING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What `gh api rate_limit` says (inner None: it couldn't say), asked
/// once for a burst of failures: one probe runs at a time, and its answer
/// serves those that waited on it for [`PROBE_REUSE`]. Outer None when a
/// probe that ran while this one waited paused GitHub — nothing left to
/// ask.
async fn probe_limits(timeout: Duration) -> Option<Option<String>> {
    let _one = PROBING.lock().await;
    if github_paused_until().is_some() {
        return None;
    }
    let probed = || PROBED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, limits)) = probed().as_ref() {
        if at.elapsed() < PROBE_REUSE {
            return Some(limits.clone());
        }
    }
    let limits = spawn_gh(None, &["api", "rate_limit"], timeout).await.ok();
    *probed() = Some((Instant::now(), limits.clone()));
    Some(limits)
}

/// Spawn `gh` itself: stdout on success, `Err(Some(stderr))` on a bad
/// exit, `Err(None)` when it could not be run or timed out.
async fn spawn_gh(
    dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<String, Option<String>> {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let out = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(_)) | Err(_) => return Err(None),
    };
    if !out.status.success() {
        return Err(Some(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// How long a Linear rate limit holds Linear off. Linear's limits are a
/// leaky bucket refilled over the hour (1,500 requests and 250,000
/// complexity points per key); its answer names no reset, so a spent
/// bucket is left ten minutes to refill rather than poked every backoff.
pub const LINEAR_PAUSE: Duration = Duration::from_secs(10 * 60);

/// How long Graph's 429 holds Microsoft off: it names a `Retry-After`
/// orion's HTTP layer doesn't read, and it is rarely more than minutes.
pub const GRAPH_PAUSE: Duration = Duration::from_secs(10 * 60);

/// How long a usage endpoint's 429 holds every usage read off. Anthropic's
/// usage endpoint is undocumented and answers a client that keeps asking
/// with 429s for good: an hour at the least.
pub const USAGE_PAUSE: Duration = Duration::from_secs(60 * 60);

/// Why a Linear read failed, from what it said (`linear::graphql`'s
/// error): a rate limit — HTTP 429, or GraphQL's `RATELIMITED` — pauses
/// Linear for [`LINEAR_PAUSE`] from `now`; anything else is a failure of
/// its own.
pub fn linear_failure(why: String, now: Instant) -> FetchError {
    failure(why, LINEAR_PAUSE, now)
}

/// Why a service's read failed, from what it said: one that names a rate
/// limit or an HTTP 429 pauses the service for `pause` from `now`;
/// anything else is a failure of its own.
pub fn failure(why: String, pause: Duration, now: Instant) -> FetchError {
    match classify(&why) {
        Signal::RateLimit => FetchError::RateLimited { until: now + pause },
        _ => FetchError::Failed(why),
    }
}

/// When `until` falls on the wall clock, `22:11` — for "GitHub limit, back
/// at 22:11".
pub fn wall_time(until: Instant, now: Instant) -> String {
    let wait = until.saturating_duration_since(now);
    let at = chrono::Local::now() + chrono::Duration::from_std(wait).unwrap_or_default();
    at.format("%H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_messages_are_told_from_other_failures() {
        for limited in [
            "GraphQL: API rate limit exceeded for user ID 1. (RATE_LIMITED)",
            "gh: API rate limit exceeded for 1.2.3.4",
            "You have exceeded a secondary rate limit. Please wait a few minutes",
            "HTTP 429: Too Many Requests",
            "type: RATE_LIMITED",
        ] {
            assert_eq!(classify(limited), Signal::RateLimit, "{limited}");
        }
        assert_eq!(
            classify("HTTP 403: Resource not accessible by integration"),
            Signal::MaybeLimit
        );
        for other in [
            "no pull requests found for branch \"x\"",
            "error connecting to api.github.com",
            "",
        ] {
            assert_eq!(classify(other), Signal::Other, "{other}");
        }
    }

    const LIMITS: &str = r#"{"resources": {
        "core": {"limit": 5000, "remaining": 4000, "reset": 2000, "used": 1000},
        "graphql": {"limit": 5000, "remaining": 0, "reset": 1600, "used": 5000}
    }}"#;

    /// A spent resource pauses until it resets; the one that isn't spent
    /// says nothing about it.
    #[test]
    fn a_spent_limit_pauses_until_its_reset() {
        assert_eq!(
            pause_for(Signal::RateLimit, Some(LIMITS), 1000),
            Some(Duration::from_secs(601))
        );
        assert_eq!(
            pause_for(Signal::MaybeLimit, Some(LIMITS), 1000),
            Some(Duration::from_secs(601)),
            "a 403 with the limit spent is the limit"
        );
    }

    #[test]
    fn a_limit_with_nothing_spent_is_secondary_and_a_403_is_not_a_limit() {
        let fresh = r#"{"resources": {"graphql": {"remaining": 10, "reset": 1600}}}"#;
        assert_eq!(
            pause_for(Signal::RateLimit, Some(fresh), 1000),
            Some(SECONDARY_PAUSE)
        );
        assert_eq!(pause_for(Signal::MaybeLimit, Some(fresh), 1000), None);
        assert_eq!(pause_for(Signal::Other, Some(LIMITS), 1000), None);
    }

    #[test]
    fn an_unknown_reset_pauses_a_named_limit_only() {
        assert_eq!(pause_for(Signal::RateLimit, None, 0), Some(UNKNOWN_PAUSE));
        assert_eq!(
            pause_for(Signal::RateLimit, Some("{oops"), 0),
            Some(UNKNOWN_PAUSE)
        );
        assert_eq!(pause_for(Signal::MaybeLimit, None, 0), None);
    }

    /// A pause is reported once as starting and once as lifting, and a
    /// second limit while paused only ever extends it.
    #[test]
    fn a_pause_starts_extends_and_lifts_once() {
        let t0 = Instant::now();
        let mut pause = Pause::default();
        assert_eq!(pause.check(t0), (None, false));
        assert!(pause.set(t0 + Duration::from_secs(60), t0), "starts");
        assert!(
            !pause.set(t0 + Duration::from_secs(30), t0),
            "already paused"
        );
        assert_eq!(
            pause.check(t0 + Duration::from_secs(10)),
            (Some(t0 + Duration::from_secs(60)), false),
            "never shortened"
        );
        assert_eq!(pause.check(t0 + Duration::from_secs(61)), (None, true));
        assert_eq!(
            pause.check(t0 + Duration::from_secs(62)),
            (None, false),
            "said once"
        );
    }

    #[test]
    fn a_linear_rate_limit_pauses_linear_and_anything_else_is_a_failure() {
        let now = Instant::now();
        for limited in [
            "Linear rate limit: RATELIMITED (HTTP 429)",
            "Linear returned something that wasn't JSON (HTTP 429)",
            "Rate limit exceeded",
        ] {
            assert_eq!(
                linear_failure(limited.into(), now),
                FetchError::RateLimited {
                    until: now + LINEAR_PAUSE
                },
                "{limited}"
            );
        }
        assert_eq!(
            linear_failure("Authentication required".into(), now),
            FetchError::Failed("Authentication required".into())
        );
    }

    #[test]
    fn a_failure_sums_up_as_its_first_line() {
        let e = FetchError::Failed("\nHTTP 502: Bad Gateway\nmore".into());
        assert_eq!(e.summary(), "HTTP 502: Bad Gateway");
        assert_eq!(FetchError::Missing.summary(), "couldn't ask");
        assert_eq!(FetchError::Missing.into_stderr(), "");
    }
}
