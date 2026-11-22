//! The Cursor MODEL / EFFORT catalogue.
//!
//! `cursor-agent --model` takes one flat id per (family, effort, fast) triple —
//! `claude-opus-5-thinking-high`, `gpt-5.6-sol-xhigh-fast`, `gpt-5.3-codex` — and
//! rejects the `family[effort=…]` bracket form its `--help` describes. orion
//! keeps the *family* as the MODEL and the *suffix* (`high`, `high-fast`,
//! `fast`) as the EFFORT, and the DAEMON joins them with a `-` at spawn.
//!
//! One parser ([`split_id`]) feeds two sources: [`SEED_IDS`], the catalogue as
//! observed on 2026-08-28, and — at runtime — `cursor-agent --list-models`,
//! cached in the DATA DIR for a day ([`crate::model_cache`]; a failed listing
//! waits six hours) and refreshed on a background thread by [`bootstrap`]. The two are unioned (the CLI's order
//! first, see [`merged`]), so a family Cursor adds shows up by itself, ahead
//! of the seed's, while nothing the seed knows is lost: `--list-models` has
//! left accepted variants out (`claude-fable-5-low`, …), and a signed-out CLI
//! lists nothing at all. On 2026-11-21 (cursor-agent 2026.08.31, signed in)
//! it listed 284 ids in 60 families, Claude Opus 5.5, Fable 5.2 and Sonnet
//! 5.5 among them, every seed family but `gemini-3.7-flash` too.
//!
//! Lists are handed out as `&'static` slices — the same shape as
//! `CLAUDE_MODELS` — by leaking each installed catalogue once; a refresh happens
//! at most once per TUI process.

use std::sync::{OnceLock, RwLock};

use crate::config::DEFAULT_CHOICE;

/// Catalogue ids as `cursor-agent` accepted them on 2026-08-28 (cursor-agent
/// 2026.08.25), curated to the current families. Order is the pick order.
pub const SEED_IDS: &[&str] = &[
    // auto
    "auto",
    // claude-fable-5
    "claude-fable-5-low",
    "claude-fable-5-medium",
    "claude-fable-5-high",
    "claude-fable-5-xhigh",
    "claude-fable-5-max",
    // claude-fable-5-thinking
    "claude-fable-5-thinking-low",
    "claude-fable-5-thinking-medium",
    "claude-fable-5-thinking-high",
    "claude-fable-5-thinking-xhigh",
    "claude-fable-5-thinking-max",
    // claude-opus-5
    "claude-opus-5-low",
    "claude-opus-5-low-fast",
    "claude-opus-5-medium",
    "claude-opus-5-medium-fast",
    "claude-opus-5-high",
    "claude-opus-5-high-fast",
    // claude-opus-5-thinking
    "claude-opus-5-thinking-low",
    "claude-opus-5-thinking-low-fast",
    "claude-opus-5-thinking-medium",
    "claude-opus-5-thinking-medium-fast",
    "claude-opus-5-thinking-high",
    "claude-opus-5-thinking-high-fast",
    "claude-opus-5-thinking-xhigh",
    "claude-opus-5-thinking-xhigh-fast",
    "claude-opus-5-thinking-max",
    "claude-opus-5-thinking-max-fast",
    // claude-sonnet-5
    "claude-sonnet-5-low",
    "claude-sonnet-5-medium",
    "claude-sonnet-5-high",
    "claude-sonnet-5-xhigh",
    "claude-sonnet-5-max",
    // claude-sonnet-5-thinking
    "claude-sonnet-5-thinking-low",
    "claude-sonnet-5-thinking-medium",
    "claude-sonnet-5-thinking-high",
    "claude-sonnet-5-thinking-xhigh",
    "claude-sonnet-5-thinking-max",
    // gpt-5.6-sol
    "gpt-5.6-sol-none",
    "gpt-5.6-sol-none-fast",
    "gpt-5.6-sol-low",
    "gpt-5.6-sol-low-fast",
    "gpt-5.6-sol-medium",
    "gpt-5.6-sol-medium-fast",
    "gpt-5.6-sol-high",
    "gpt-5.6-sol-high-fast",
    "gpt-5.6-sol-xhigh",
    "gpt-5.6-sol-xhigh-fast",
    "gpt-5.6-sol-max",
    "gpt-5.6-sol-max-fast",
    // gpt-5.6-terra
    "gpt-5.6-terra-none",
    "gpt-5.6-terra-none-fast",
    "gpt-5.6-terra-low",
    "gpt-5.6-terra-low-fast",
    "gpt-5.6-terra-medium",
    "gpt-5.6-terra-medium-fast",
    "gpt-5.6-terra-high",
    "gpt-5.6-terra-high-fast",
    "gpt-5.6-terra-xhigh",
    "gpt-5.6-terra-xhigh-fast",
    "gpt-5.6-terra-max",
    "gpt-5.6-terra-max-fast",
    // gpt-5.6-luna
    "gpt-5.6-luna-none",
    "gpt-5.6-luna-none-fast",
    "gpt-5.6-luna-low",
    "gpt-5.6-luna-low-fast",
    "gpt-5.6-luna-medium",
    "gpt-5.6-luna-medium-fast",
    "gpt-5.6-luna-high",
    "gpt-5.6-luna-high-fast",
    "gpt-5.6-luna-xhigh",
    "gpt-5.6-luna-xhigh-fast",
    "gpt-5.6-luna-max",
    "gpt-5.6-luna-max-fast",
    // gpt-5.5
    "gpt-5.5-none",
    "gpt-5.5-none-fast",
    "gpt-5.5-low",
    "gpt-5.5-low-fast",
    "gpt-5.5-medium",
    "gpt-5.5-medium-fast",
    "gpt-5.5-high",
    "gpt-5.5-high-fast",
    "gpt-5.5-extra-high",
    "gpt-5.5-extra-high-fast",
    // gpt-5.3-codex
    "gpt-5.3-codex-low",
    "gpt-5.3-codex-low-fast",
    "gpt-5.3-codex",
    "gpt-5.3-codex-fast",
    "gpt-5.3-codex-high",
    "gpt-5.3-codex-high-fast",
    "gpt-5.3-codex-xhigh",
    "gpt-5.3-codex-xhigh-fast",
    // gpt-5.2
    "gpt-5.2",
    "gpt-5.2-low",
    "gpt-5.2-low-fast",
    "gpt-5.2-fast",
    "gpt-5.2-high",
    "gpt-5.2-high-fast",
    "gpt-5.2-xhigh",
    "gpt-5.2-xhigh-fast",
    // composer-2.5
    "composer-2.5",
    "composer-2.5-fast",
    // cursor-grok-4.6
    "cursor-grok-4.6-low",
    "cursor-grok-4.6-low-fast",
    "cursor-grok-4.6-medium",
    "cursor-grok-4.6-medium-fast",
    "cursor-grok-4.6-high",
    "cursor-grok-4.6-high-fast",
    "cursor-grok-4.6-xhigh",
    "cursor-grok-4.6-xhigh-fast",
    // gemini-3.7-flash
    "gemini-3.7-flash-low",
    "gemini-3.7-flash-medium",
    "gemini-3.7-flash-high",
];

/// Effort words in pick order; `-fast` twins follow their base.
const EFFORT_ORDER: &[&str] = &[
    "none",
    "low",
    "medium",
    "high",
    "xhigh",
    "extra-high",
    "max",
];
const FAST: &str = "fast";

const CACHE_FILE: &str = "cursor_models.json";

/// One model family and the effort suffixes it ships. `efforts` leads with
/// [`DEFAULT_CHOICE`] only when the bare family id exists (`gpt-5.3-codex`,
/// `auto`); most families have no bare id, so "default" for them is the
/// [`Catalogue::fallback_effort`] the launch substitutes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub name: String,
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Catalogue {
    pub families: Vec<Family>,
}

/// Split a catalogue id into (family, suffix): the trailing `-fast`, then a
/// trailing effort word (`extra-high` counts as one), are the suffix; the
/// rest is the family. `claude-4.6-sonnet-medium-thinking` ends in neither,
/// so it is a bare family of its own — ugly, but it launches.
pub fn split_id(id: &str) -> (String, String) {
    let id = id.trim();
    let (base, fast) = match id.strip_suffix("-fast") {
        Some(base) => (base, true),
        None => (id, false),
    };
    let mut family = base;
    let mut effort = "";
    // Longest word first: `-extra-high` must win over its `-high` tail.
    let mut words = EFFORT_ORDER.to_vec();
    words.sort_by_key(|w| std::cmp::Reverse(w.len()));
    for word in words {
        if let Some(rest) = base.strip_suffix(&format!("-{word}")) {
            if !rest.is_empty() {
                family = rest;
                effort = word;
                break;
            }
        }
    }
    let suffix = match (effort, fast) {
        ("", false) => String::new(),
        ("", true) => FAST.to_string(),
        (e, false) => e.to_string(),
        (e, true) => format!("{e}-{FAST}"),
    };
    (family.to_string(), suffix)
}

/// Pick-order rank of a suffix: default, bare fast, then each effort word
/// with its fast twin right behind it.
fn suffix_rank(suffix: &str) -> (usize, bool) {
    if suffix.is_empty() {
        return (0, false);
    }
    let (base, fast) = match suffix.strip_suffix("-fast") {
        Some(base) => (base, true),
        None if suffix == FAST => ("", true),
        None => (suffix, false),
    };
    let rank = if base.is_empty() {
        0
    } else {
        EFFORT_ORDER
            .iter()
            .position(|w| *w == base)
            .map_or(EFFORT_ORDER.len() + 1, |p| p + 1)
    };
    (rank, fast)
}

impl Catalogue {
    /// Build from ids, deduplicating, families in first-seen order, efforts
    /// in pick order.
    pub fn from_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Catalogue {
        let mut families: Vec<(String, Vec<String>)> = Vec::new();
        for id in ids {
            let id = id.trim();
            if id.is_empty() || id.contains(char::is_whitespace) {
                continue;
            }
            let (family, suffix) = split_id(id);
            let slot = match families.iter_mut().find(|(f, _)| *f == family) {
                Some(slot) => slot,
                None => {
                    families.push((family, Vec::new()));
                    families.last_mut().expect("just pushed")
                }
            };
            if !slot.1.contains(&suffix) {
                slot.1.push(suffix);
            }
        }
        let families = families
            .into_iter()
            .map(|(name, mut suffixes)| {
                suffixes.sort_by_key(|s| suffix_rank(s));
                let efforts = suffixes
                    .into_iter()
                    .map(|s| {
                        if s.is_empty() {
                            DEFAULT_CHOICE.to_string()
                        } else {
                            s
                        }
                    })
                    .collect::<Vec<_>>();
                // A family whose only variant is the bare id has no effort
                // choice at all (`auto`).
                let efforts = if efforts.len() == 1 && efforts[0] == DEFAULT_CHOICE {
                    Vec::new()
                } else {
                    efforts
                };
                Family { name, efforts }
            })
            .collect();
        Catalogue { families }
    }

    pub fn family(&self, name: &str) -> Option<&Family> {
        let name = name.trim();
        self.families
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// What "default" launches for a family with no bare id: `high` if it
    /// ships one, else `medium`, else its first non-fast variant, else its
    /// first. None for a family with a bare id (default = no suffix) or none.
    pub fn fallback_effort(&self, name: &str) -> Option<&str> {
        let family = self.family(name)?;
        let efforts = &family.efforts;
        if efforts.is_empty() || efforts[0] == DEFAULT_CHOICE {
            return None;
        }
        ["high", "medium"]
            .iter()
            .find_map(|want| efforts.iter().find(|e| e == want))
            .or_else(|| efforts.iter().find(|e| !e.ends_with(FAST)))
            .or_else(|| efforts.first())
            .map(String::as_str)
    }
}

/// Parse `cursor-agent --list-models` output (`<id> - <label>` lines under an
/// "Available models" header) into ids.
pub fn parse_list_models(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (id, _label) = line.split_once(" - ")?;
            let id = id.trim();
            (!id.is_empty() && !id.contains(char::is_whitespace)).then(|| id.to_string())
        })
        .collect()
}

// --- the installed, 'static view --------------------------------------------

struct Installed {
    models: &'static [&'static str],
    families: Vec<(&'static str, &'static [&'static str])>,
    fallbacks: Vec<(&'static str, Option<&'static str>)>,
}

fn leak(catalogue: &Catalogue) -> &'static Installed {
    let mut models: Vec<&'static str> = vec![DEFAULT_CHOICE];
    let mut families = Vec::new();
    let mut fallbacks = Vec::new();
    for family in &catalogue.families {
        let name: &'static str = Box::leak(family.name.clone().into_boxed_str());
        let efforts: Vec<&'static str> = family
            .efforts
            .iter()
            .map(|e| {
                if e == DEFAULT_CHOICE {
                    DEFAULT_CHOICE
                } else {
                    Box::leak(e.clone().into_boxed_str()) as &'static str
                }
            })
            .collect();
        models.push(name);
        families.push((
            name,
            Box::leak(efforts.into_boxed_slice()) as &'static [&'static str],
        ));
        fallbacks.push((
            name,
            catalogue
                .fallback_effort(&family.name)
                .map(|e| Box::leak(e.to_string().into_boxed_str()) as &'static str),
        ));
    }
    Box::leak(Box::new(Installed {
        models: Box::leak(models.into_boxed_slice()),
        families,
        fallbacks,
    }))
}

static CURRENT: OnceLock<RwLock<&'static Installed>> = OnceLock::new();

fn current() -> &'static Installed {
    let lock =
        CURRENT.get_or_init(|| RwLock::new(leak(&Catalogue::from_ids(SEED_IDS.iter().copied()))));
    *lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The MODEL choices: [`DEFAULT_CHOICE`], then every family.
pub fn models() -> &'static [&'static str] {
    current().models
}

/// The EFFORT choices for a family; empty for no family, "default", an
/// unknown family, or one with no effort variants.
pub fn efforts(family: Option<&str>) -> &'static [&'static str] {
    let Some(name) = family
        .map(str::trim)
        .filter(|m| !m.eq_ignore_ascii_case(DEFAULT_CHOICE))
    else {
        return &[];
    };
    current()
        .families
        .iter()
        .find(|(f, _)| f.eq_ignore_ascii_case(name))
        .map_or(&[], |(_, efforts)| efforts)
}

/// See [`Catalogue::fallback_effort`].
pub fn fallback_effort(family: &str) -> Option<&'static str> {
    let name = family.trim();
    current()
        .fallbacks
        .iter()
        .find(|(f, _)| f.eq_ignore_ascii_case(name))
        .and_then(|(_, e)| *e)
}

/// Replace the installed catalogue with `runtime_ids` ∪ seed ([`merged`]).
/// Not called from tests: the view is process-global.
pub fn install(runtime_ids: &[String]) {
    let merged = merged(runtime_ids);
    let installed = leak(&merged);
    let lock = CURRENT.get_or_init(|| RwLock::new(installed));
    *lock
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = installed;
}

/// The catalogue `cursor-agent --list-models` and the seed make together:
/// the CLI's families first, in its order — its own picker's — then the
/// seed's others. Seed first would put a family Cursor added after every
/// family the seed knew.
pub fn merged(runtime_ids: &[String]) -> Catalogue {
    Catalogue::from_ids(
        runtime_ids
            .iter()
            .map(String::as_str)
            .chain(SEED_IDS.iter().copied()),
    )
}

// --- refresh -----------------------------------------------------------------

/// Run `cursor-agent --list-models` and parse it; None when the CLI is
/// missing, fails (not logged in), runs past
/// [`crate::model_cache::LIST_TIMEOUT`], or lists nothing.
pub fn fetch_ids() -> Option<Vec<String>> {
    let out = crate::model_cache::listing(
        orion_core::AgentKind::Cursor.cli_program(),
        &["--list-models"],
    )?;
    let ids = parse_list_models(&out);
    (!ids.is_empty()).then_some(ids)
}

/// TUI startup: the cached catalogue now, a background `--list-models`
/// when the cache is a day old ([`crate::model_cache`]).
pub fn bootstrap(cursor_enabled: bool) {
    crate::model_cache::bootstrap(cursor_enabled, CACHE_FILE, fetch_ids, install);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_id_peels_fast_then_effort() {
        assert_eq!(split_id("auto"), ("auto".into(), "".into()));
        assert_eq!(
            split_id("gpt-5.3-codex"),
            ("gpt-5.3-codex".into(), "".into())
        );
        assert_eq!(
            split_id("gpt-5.3-codex-fast"),
            ("gpt-5.3-codex".into(), "fast".into())
        );
        assert_eq!(
            split_id("claude-opus-5-thinking-high"),
            ("claude-opus-5-thinking".into(), "high".into())
        );
        assert_eq!(
            split_id("claude-opus-5-thinking-max-fast"),
            ("claude-opus-5-thinking".into(), "max-fast".into())
        );
        assert_eq!(
            split_id("gpt-5.5-extra-high"),
            ("gpt-5.5".into(), "extra-high".into())
        );
        assert_eq!(
            split_id("gpt-5.6-sol-none-fast"),
            ("gpt-5.6-sol".into(), "none-fast".into())
        );
        // Neither a fast nor an effort tail: a bare family of its own.
        assert_eq!(
            split_id("claude-4.6-sonnet-medium-thinking"),
            ("claude-4.6-sonnet-medium-thinking".into(), "".into())
        );
        // An id that *is* an effort word stays a family, not an empty one.
        assert_eq!(split_id("high"), ("high".into(), "".into()));
    }

    #[test]
    fn from_ids_groups_dedups_and_orders() {
        let cat = Catalogue::from_ids([
            "gpt-5.3-codex-xhigh",
            "gpt-5.3-codex",
            "gpt-5.3-codex-fast",
            "gpt-5.3-codex-low",
            "gpt-5.3-codex-low",
            "auto",
            "claude-fable-5-max",
            "claude-fable-5-low",
            "bad id",
            "",
        ]);
        assert_eq!(
            cat.families
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-5.3-codex", "auto", "claude-fable-5"]
        );
        assert_eq!(
            cat.family("gpt-5.3-codex").unwrap().efforts,
            vec!["default", "fast", "low", "xhigh"]
        );
        assert!(
            cat.family("auto").unwrap().efforts.is_empty(),
            "bare-only = no choice"
        );
        assert_eq!(
            cat.family("claude-fable-5").unwrap().efforts,
            vec!["low", "max"]
        );
        assert_eq!(
            cat.family("CLAUDE-FABLE-5").map(|f| f.name.as_str()),
            Some("claude-fable-5")
        );
    }

    #[test]
    fn fallback_prefers_high_then_medium_then_a_non_fast_variant() {
        let cat = Catalogue::from_ids([
            "gpt-5.3-codex",
            "gpt-5.3-codex-high",
            "claude-fable-5-low",
            "claude-fable-5-high",
            "gemini-x-low",
            "gemini-x-medium",
            "odd-none-fast",
            "odd-none",
            "onlyfast-low-fast",
            "auto",
        ]);
        assert_eq!(
            cat.fallback_effort("gpt-5.3-codex"),
            None,
            "bare id = default is real"
        );
        assert_eq!(cat.fallback_effort("claude-fable-5"), Some("high"));
        assert_eq!(cat.fallback_effort("gemini-x"), Some("medium"));
        assert_eq!(cat.fallback_effort("odd"), Some("none"));
        assert_eq!(cat.fallback_effort("onlyfast"), Some("low-fast"));
        assert_eq!(cat.fallback_effort("auto"), None);
        assert_eq!(cat.fallback_effort("nope"), None);
    }

    #[test]
    fn seed_matches_the_static_view() {
        let cat = Catalogue::from_ids(SEED_IDS.iter().copied());
        assert_eq!(models()[0], DEFAULT_CHOICE);
        assert_eq!(
            models()[1..].to_vec(),
            cat.families
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(models()[1], "auto");
        assert!(efforts(Some("auto")).is_empty());
        assert!(efforts(None).is_empty());
        assert!(efforts(Some("default")).is_empty());
        assert!(efforts(Some("not-a-family")).is_empty());
        assert_eq!(
            efforts(Some("claude-opus-5")),
            &[
                "low",
                "low-fast",
                "medium",
                "medium-fast",
                "high",
                "high-fast"
            ]
        );
        assert_eq!(efforts(Some("gpt-5.3-codex"))[0], DEFAULT_CHOICE);
        assert_eq!(fallback_effort("gpt-5.3-codex"), None);
        assert_eq!(fallback_effort("claude-fable-5"), Some("high"));
        assert_eq!(fallback_effort("gpt-5.5"), Some("high"));
        assert_eq!(fallback_effort("auto"), None);
        for family in &cat.families {
            assert!(
                family.efforts.is_empty()
                    || family
                        .efforts
                        .iter()
                        .all(|e| e == DEFAULT_CHOICE || !e.is_empty()),
                "{}: no empty suffix survives",
                family.name
            );
        }
    }

    /// A family the CLI lists comes before the seed's, so a model Cursor
    /// added is at the top of the picker; the seed's variants still merge
    /// into a family both know.
    #[test]
    fn the_clis_families_lead_the_seeds() {
        let runtime = ["auto", "claude-opus-6-high", "claude-fable-5-high"].map(String::from);
        let cat = merged(&runtime);
        let names: Vec<&str> = cat.families.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names[..3], ["auto", "claude-opus-6", "claude-fable-5"]);
        assert!(cat.family("claude-fable-5").unwrap().efforts.len() > 1);
        assert!(cat.family("gemini-3.7-flash").is_some(), "seed kept");
    }

    #[test]
    fn parse_list_models_takes_the_id_column() {
        let text = "Available models\n\nauto - Auto (default)\ngpt-5.3-codex-low - Codex 5.3 Low\n                    claude-opus-5-thinking-high - Claude Opus 5 1M Thinking\nnot a model line\n";
        assert_eq!(
            parse_list_models(text),
            vec!["auto", "gpt-5.3-codex-low", "claude-opus-5-thinking-high"]
        );
        assert!(parse_list_models("").is_empty());
    }
}
