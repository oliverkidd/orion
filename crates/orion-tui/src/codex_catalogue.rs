//! The Codex MODEL list: what the installed `codex` offers, so a model
//! OpenAI ships shows up in the pickers without an orion release.
//!
//! `codex debug models` prints the CLI's model catalogue as JSON — the one
//! its own `/model` picker reads, refreshed from OpenAI when signed in and
//! the catalogue bundled with the binary otherwise. The models it lists
//! (`"visibility": "list"`), in its `priority` order, are the MODEL rows;
//! hidden ones (internal, review-only) are left out. It is cached in the
//! DATA DIR for a day ([`crate::model_cache`]). Until a listing has
//! landed — no `codex` installed, or a CLI too old for the command — the
//! descriptor's own list stands.

use std::process::{Command, Stdio};
use std::sync::{OnceLock, RwLock};

use crate::config::DEFAULT_CHOICE;

const CACHE_FILE: &str = "codex_models.json";

/// The installed list, [`DEFAULT_CHOICE`] first; None until one lands.
static CURRENT: OnceLock<RwLock<Option<&'static [&'static str]>>> = OnceLock::new();

/// The MODEL choices the installed `codex` lists, [`DEFAULT_CHOICE`] first;
/// None while there is no listing, so the descriptor's list stands.
pub fn models() -> Option<&'static [&'static str]> {
    let lock = CURRENT.get()?;
    *lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Install a listing. Not called from tests: the view is process-global.
fn install(ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let list: Vec<&'static str> = std::iter::once(DEFAULT_CHOICE)
        .chain(
            ids.iter()
                .filter(|id| *id != DEFAULT_CHOICE)
                .map(|id| Box::leak(id.clone().into_boxed_str()) as &'static str),
        )
        .collect();
    let list: &'static [&'static str] = Box::leak(list.into_boxed_slice());
    let lock = CURRENT.get_or_init(|| RwLock::new(None));
    *lock
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(list);
}

/// The ids `codex debug models` lists, in its priority order; empty when
/// the JSON is not a catalogue.
pub fn parse_catalog(json: &str) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Catalog {
        models: Vec<Model>,
    }
    #[derive(serde::Deserialize)]
    struct Model {
        slug: String,
        #[serde(default)]
        visibility: Option<String>,
        #[serde(default)]
        priority: Option<i64>,
    }
    let Ok(catalog) = serde_json::from_str::<Catalog>(json) else {
        return Vec::new();
    };
    let mut listed: Vec<Model> = catalog
        .models
        .into_iter()
        .filter(|m| m.visibility.as_deref().is_none_or(|v| v == "list"))
        .filter(|m| !m.slug.trim().is_empty() && !m.slug.contains(char::is_whitespace))
        .collect();
    // Stable: models with no priority keep the catalogue's order, last.
    listed.sort_by_key(|m| m.priority.unwrap_or(i64::MAX));
    let mut ids: Vec<String> = Vec::new();
    for model in listed {
        if !ids.contains(&model.slug) {
            ids.push(model.slug);
        }
    }
    ids
}

/// Run `codex debug models` and parse it; None when the CLI is missing,
/// fails, or lists nothing.
fn fetch_ids() -> Option<Vec<String>> {
    let output = Command::new(orion_core::AgentKind::Codex.cli_program())
        .args(["debug", "models"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ids = parse_catalog(&String::from_utf8_lossy(&output.stdout));
    (!ids.is_empty()).then_some(ids)
}

/// TUI startup: the cached listing now, a background `codex debug models`
/// when the cache is a day old.
pub fn bootstrap(codex_enabled: bool) {
    crate::model_cache::bootstrap(codex_enabled, CACHE_FILE, fetch_ids, install);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `codex debug models` printed (codex-cli 0.171.0), cut to
    /// the fields read.
    const CATALOG: &str = r#"{"models":[
        {"slug":"gpt-6.1-sol","visibility":"list","priority":4},
        {"slug":"gpt-6.2-sol","display_name":"GPT-6.2-Sol","visibility":"list","priority":1},
        {"slug":"codex-auto-review","visibility":"hide","priority":43},
        {"slug":"gpt-6.2-astra","visibility":"list","priority":2},
        {"slug":"gpt-5.5","visibility":"list","priority":13}
    ]}"#;

    #[test]
    fn listed_models_in_priority_order() {
        assert_eq!(
            parse_catalog(CATALOG),
            ["gpt-6.2-sol", "gpt-6.2-astra", "gpt-6.1-sol", "gpt-5.5"]
        );
    }

    #[test]
    fn anything_but_a_catalogue_lists_nothing() {
        assert!(parse_catalog("").is_empty());
        assert!(parse_catalog("error: unknown command debug").is_empty());
        assert!(parse_catalog(r#"{"models":[]}"#).is_empty());
    }

    /// A model with no visibility or priority (an older catalogue) is
    /// listed, after the ranked ones.
    #[test]
    fn unranked_models_follow_the_ranked() {
        let json = r#"{"models":[{"slug":"old-model"},{"slug":"new","priority":1}]}"#;
        assert_eq!(parse_catalog(json), ["new", "old-model"]);
    }
}
