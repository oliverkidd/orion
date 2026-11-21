//! A CLI's MODEL list as its own listing command printed it
//! (`cursor-agent --list-models`, `codex debug models`), cached in the DATA
//! DIR so the pickers offer the models the installed CLI knows — the newest
//! among them — instead of only the list orion was built with.
//!
//! The TUI installs the cached list at startup and, when it is missing or
//! older than [`CACHE_TTL`], asks the CLI again on a background thread. A
//! CLI that is missing, fails (signed out) or lists nothing leaves the list
//! as it was.

use orion_core::clock::now_secs;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// How long a cached listing is trusted before a refresh.
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    fetched_at: u64,
    ids: Vec<String>,
}

fn read(path: &Path) -> Option<Cache> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write(path: &Path, ids: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let cache = Cache {
        fetched_at: now_secs(),
        ids: ids.to_vec(),
    };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&cache)?)?;
    std::fs::rename(tmp, path)
}

/// TUI startup for one CLI's list: `install` the cached ids at once and,
/// when the cache (`file` in the DATA DIR) is missing or stale, `fetch`
/// fresh ones on a background thread, cache them and install them.
/// Skipped when the harness is switched off and under `ORION_AGENT_CMD`
/// (tests, STUB AGENTS), so no test ever shells out.
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
        install(&cache.ids);
    }
    let fresh =
        cached.is_some_and(|c| now_secs().saturating_sub(c.fetched_at) < CACHE_TTL.as_secs());
    if fresh {
        return;
    }
    std::thread::spawn(move || {
        if let Some(ids) = fetch() {
            if let Err(err) = write(&path, &ids) {
                tracing::warn!(path = %path.display(), "model list cache not written: {err}");
            }
            install(&ids);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trips_and_ages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("models.json");
        assert!(read(&path).is_none());
        write(&path, &["auto".to_string(), "gpt-5.2".to_string()]).unwrap();
        let cache = read(&path).unwrap();
        assert_eq!(cache.ids, vec!["auto", "gpt-5.2"]);
        assert!(now_secs().saturating_sub(cache.fetched_at) < CACHE_TTL.as_secs());
        assert!(
            !path.with_extension("json.tmp").exists(),
            "tmp renamed away"
        );
    }
}
