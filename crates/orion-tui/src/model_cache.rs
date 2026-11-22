//! A CLI's MODEL list as its own listing command printed it
//! (`cursor-agent --list-models`, `codex debug models`), cached in the DATA
//! DIR so the pickers offer the models the installed CLI knows — the newest
//! among them — instead of only the list orion was built with.
//!
//! The TUI installs the cached list at startup and, when it is missing or
//! older than [`CACHE_TTL`], asks the CLI again on a background thread. A
//! CLI that is missing, fails (signed out), hangs past [`LIST_TIMEOUT`] or
//! lists nothing leaves the list as it was — and says so in the cache as a
//! FAILURE STAMP, so the next launches leave it alone for [`RETRY_AFTER`]
//! rather than shelling out again on every one ([`due`]).

use orion_core::clock::now_secs;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// How long a cached listing is trusted before a refresh.
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after a listing failed before it is asked again. A CLI that
/// is not installed, or signed out, stays that way for hours at a time;
/// asking every launch only spends a process start (and, for a hung CLI,
/// a thread) on the same answer.
pub const RETRY_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

/// How long a listing command may run before it is killed and counted a
/// failure. Both print from a local catalogue or one quick request; one
/// still going after this is stuck (a login prompt, a dead network).
pub const LIST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    /// When `ids` were listed; 0 when no listing has landed yet.
    #[serde(default)]
    fetched_at: u64,
    #[serde(default)]
    ids: Vec<String>,
    /// When the last listing failed, if the last one did. Cleared by the
    /// next success. Absent in a cache written before failures were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failed_at: Option<u64>,
}

fn read(path: &Path) -> Option<Cache> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write(path: &Path, cache: &Cache) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(cache)?)?;
    std::fs::rename(tmp, path)
}

/// A fresh listing, any failure stamp cleared.
fn landed(ids: &[String], now: u64) -> Cache {
    Cache {
        fetched_at: now,
        ids: ids.to_vec(),
        failed_at: None,
    }
}

/// `cached` with a failure stamped at `now`: the last good listing stays,
/// to be installed next launch as it was this one.
fn failed(cached: Option<Cache>, now: u64) -> Cache {
    Cache {
        failed_at: Some(now),
        ..cached.unwrap_or_default()
    }
}

/// Whether the CLI should be asked for its list at `now`: never asked, or
/// its listing older than [`CACHE_TTL`] — unless the last ask failed
/// within [`RETRY_AFTER`], which holds it back whatever the listing's age.
fn due(cached: Option<&Cache>, now: u64) -> bool {
    let Some(cache) = cached else {
        return true;
    };
    if let Some(failed_at) = cache.failed_at {
        if now.saturating_sub(failed_at) < RETRY_AFTER.as_secs() {
            return false;
        }
    }
    cache.fetched_at == 0 || now.saturating_sub(cache.fetched_at) >= CACHE_TTL.as_secs()
}

/// What `program args` printed on stdout, when it exits zero inside
/// [`LIST_TIMEOUT`]; None when it can't start, fails or runs over — and
/// one that runs over is killed, not left running behind the TUI
/// (`kill_on_drop`, the future dropped by the timeout). Blocking: call it
/// off the loop, as [`bootstrap`]'s thread does.
pub fn listing(program: &str, args: &[&str]) -> Option<String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(listing_within(program, args, LIST_TIMEOUT))
}

async fn listing_within(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(output) => output.ok()?,
        Err(_) => {
            tracing::info!(program, "model listing timed out");
            return None;
        }
    };
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// TUI startup for one CLI's list: `install` the cached ids at once and,
/// when the cache (`file` in the DATA DIR) says a listing is [`due`],
/// `fetch` fresh ones on a background thread, cache them and install
/// them. A fetch that comes back with nothing (`None`: missing, failed,
/// timed out, unparsable or empty) stamps the failure instead. Skipped
/// when the harness is switched off and under `ORION_AGENT_CMD` (tests,
/// STUB AGENTS), so no test ever shells out.
pub fn bootstrap(
    enabled: bool,
    file: &str,
    fetch: fn() -> Option<Vec<String>>,
    install: fn(&[String]),
) {
    if !enabled || orion_core::env::non_empty(orion_core::env::AGENT_CMD).is_some() {
        return;
    }
    let path = orion_core::paths::data_dir().join(file);
    let cached = read(&path);
    if let Some(cache) = &cached {
        if !cache.ids.is_empty() {
            install(&cache.ids);
        }
    }
    if !due(cached.as_ref(), now_secs()) {
        return;
    }
    std::thread::spawn(move || {
        let fetched = fetch();
        let cache = match &fetched {
            Some(ids) => landed(ids, now_secs()),
            None => failed(cached, now_secs()),
        };
        if let Err(err) = write(&path, &cache) {
            tracing::warn!(path = %path.display(), "model list cache not written: {err}");
        }
        if let Some(ids) = fetched {
            install(&ids);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 60 * 60;

    #[test]
    fn cache_round_trips_and_ages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("models.json");
        assert!(read(&path).is_none());
        let ids = ["auto".to_string(), "gpt-5.2".to_string()];
        write(&path, &landed(&ids, now_secs())).unwrap();
        let cache = read(&path).unwrap();
        assert_eq!(cache.ids, vec!["auto", "gpt-5.2"]);
        assert_eq!(cache.failed_at, None);
        assert!(!due(Some(&cache), now_secs()));
        assert!(
            !path.with_extension("json.tmp").exists(),
            "tmp renamed away"
        );
    }

    #[test]
    fn a_cache_from_before_failures_were_kept_still_reads() {
        let old: Cache = serde_json::from_str(r#"{"fetched_at": 5, "ids": ["a"]}"#).unwrap();
        assert_eq!((old.fetched_at, old.failed_at), (5, None));
        let stamp_only: Cache = serde_json::from_str(r#"{"failed_at": 9}"#).unwrap();
        assert!(stamp_only.ids.is_empty());
    }

    #[test]
    fn never_listed_is_due() {
        assert!(due(None, 1_000 * HOUR));
    }

    #[test]
    fn a_listing_is_trusted_for_a_day() {
        let now = 1_000 * HOUR;
        let cache = landed(&["a".into()], now);
        assert!(!due(Some(&cache), now + 23 * HOUR));
        assert!(due(Some(&cache), now + 24 * HOUR));
    }

    #[test]
    fn a_failure_holds_the_next_ask_back_six_hours() {
        let now = 1_000 * HOUR;
        // Nothing ever listed (no CLI installed): not every launch.
        let first = failed(None, now);
        assert!(!due(Some(&first), now + 5 * HOUR));
        assert!(due(Some(&first), now + 6 * HOUR));
        // A day-old listing whose refresh failed: the old ids stay and the
        // stamp, not the listing's age, decides.
        let stale = failed(Some(landed(&["a".into()], now - 30 * HOUR)), now);
        assert_eq!(stale.ids, ["a"], "the last good listing kept");
        assert!(!due(Some(&stale), now + HOUR));
        assert!(due(Some(&stale), now + 6 * HOUR));
        // A success clears the stamp; the day's trust is back.
        let again = landed(&["b".into()], now + 6 * HOUR);
        assert_eq!(again.failed_at, None);
        assert!(!due(Some(&again), now + 7 * HOUR));
    }

    #[tokio::test]
    async fn a_listing_is_stdout_of_a_clean_exit() {
        let out = listing_within("sh", &["-c", "echo a; echo b"], Duration::from_secs(5)).await;
        assert_eq!(out.as_deref(), Some("a\nb\n"));
        assert_eq!(
            listing_within("sh", &["-c", "echo a; exit 1"], Duration::from_secs(5)).await,
            None,
            "a failing CLI lists nothing"
        );
        assert_eq!(
            listing_within("orion-no-such-cli", &[], Duration::from_secs(5)).await,
            None,
            "a missing CLI lists nothing"
        );
    }

    #[tokio::test]
    async fn a_listing_that_hangs_is_given_up_on() {
        let began = std::time::Instant::now();
        let out = listing_within("sleep", &["30"], Duration::from_millis(200)).await;
        assert_eq!(out, None);
        assert!(began.elapsed() < Duration::from_secs(10));
    }
}
