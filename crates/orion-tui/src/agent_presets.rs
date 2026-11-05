//! AGENT PRESETS: saved launch definitions — an AGENT KIND, a MODEL / EFFORT
//! choice, optional prefix / postfix text, and whether to ask for a task at
//! all — that `e` lists for the checkout under the cursor. Launching one asks for an
//! optional task (or, with `skip_task`, nothing) and hands the CLI
//! `prefix + task + postfix` as its positional starting prompt.
//!
//! A plain JSON list in the DATA DIR beside `config.json`, in list order.
//! A missing or malformed file reads as empty — like the SSH HOSTS FILE it
//! is a convenience store, never load-bearing. Writes go through a temp
//! file + rename so a crash mid-write cannot truncate the list. The list is
//! read entry by entry, and a save keeps what this build can't read — a
//! preset for a harness a newer orion added, a field it has no name for —
//! so an older orion sharing the file never deletes a newer one's presets.

use orion_core::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// What a preset that follows the box says where a harness goes: the
/// editor's Harness row, the presets list's line.
pub const FOLLOW_BOX: &str = "the box's agent";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPreset {
    /// The row's label; unique (case-insensitively) within the list.
    pub name: String,
    /// The CLI the preset launches when it pins one ([`Self::follows_box`]
    /// false).
    #[serde(default)]
    pub kind: AgentKind,
    /// The registry id beside `kind`: a Claude account's, or a custom
    /// harness's.
    #[serde(default)]
    pub custom_harness: Option<String>,
    /// FOLLOW THE BOX: the preset runs on the account and harness of the
    /// box it is picked in — the `⌘N` default's when no box is up — rather
    /// than the one `kind` names: what every new preset does, so a preset
    /// moves with the account the user is on. `false` pins `kind`. Unset
    /// only on an entry an older release wrote, which reads by
    /// [`Self::follows_box`]'s rule until the next save writes it out.
    #[serde(default)]
    pub follow_box: Option<bool>,
    /// Launch model; None = follow the Settings → Accounts default.
    #[serde(default)]
    pub model: Option<String>,
    /// Launch effort; None = follow the Settings → Accounts default.
    #[serde(default)]
    pub effort: Option<String>,
    /// Text sent before the task (may be empty).
    #[serde(default)]
    pub prefix: String,
    /// Text sent after the task (may be empty).
    #[serde(default)]
    pub postfix: String,
    /// Launch without asking for a task: Enter on the preset starts the CLI
    /// on prefix + postfix alone — a "commit and push" preset has nothing
    /// left to say. Off, the task is still asked for, but optional.
    #[serde(default)]
    pub skip_task: bool,
    /// A SYSTEM PRESET: one orion itself reads its text from — the PR
    /// autofix agent's, a Linear ticket launch's, the week review's. Every
    /// list has one of each ([`ensure_system`]); its text can be edited,
    /// but it cannot be deleted or renamed, and it is never blank: the
    /// list holds nothing while it says what orion says
    /// ([`AgentPreset::with_builtin_text`]). None on a preset the user made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPreset>,
}

/// The SYSTEM PRESETS: the agents orion starts itself, and what it reads
/// each one's text, harness, model and effort for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemPreset {
    /// The text around what broke, for the agent AUTOFIX sends to a pull
    /// request (`crate::autofix::prompt`); its model and effort are what
    /// that agent runs at when no session on the branch has run here to
    /// copy. It runs on the pull request's own agent.
    PrAutofix,
    /// The text around a Linear ticket launch's issues (`crate::linear`).
    LinearTicket,
    /// The rules a WEEK REVIEW is written by (`crate::week_review`), and
    /// the Claude account, model and effort that write it.
    WeekReview,
}

impl SystemPreset {
    pub const ALL: [SystemPreset; 3] = [
        SystemPreset::PrAutofix,
        SystemPreset::LinearTicket,
        SystemPreset::WeekReview,
    ];

    /// The name the preset is listed under, and the name an older list's
    /// own preset is adopted by.
    pub const fn name(self) -> &'static str {
        match self {
            SystemPreset::PrAutofix => "PR autofix",
            SystemPreset::LinearTicket => "Linear Ticket",
            SystemPreset::WeekReview => "Week review",
        }
    }

    /// What orion uses the preset for, in the few words its row in
    /// Settings → Presets says beside its name; `{linear}` is the Linear
    /// key, as the live keymap spells it (`hints::expand`).
    pub const fn purpose(self) -> &'static str {
        match self {
            SystemPreset::PrAutofix => "sent when one of your PRs breaks",
            SystemPreset::LinearTicket => "wraps every {linear} launch",
            SystemPreset::WeekReview => "writes your week review",
        }
    }

    /// orion's own text for the preset, as `(prefix, postfix)`: what it
    /// says while the list holds none of the user's.
    pub fn builtin_text(self) -> (&'static str, &'static str) {
        match self {
            SystemPreset::PrAutofix => crate::autofix::PRESET_TEXT,
            SystemPreset::LinearTicket => (crate::linear::TICKET_TEXT, ""),
            SystemPreset::WeekReview => (crate::week_review::brief_rules(), ""),
        }
    }

    /// The preset as a list that never had one gets it: orion's text, and
    /// — the week review's — Claude at sonnet · medium.
    pub fn default_preset(self) -> AgentPreset {
        let (model, effort) = match self {
            SystemPreset::WeekReview => (
                Some(crate::week_review::DEFAULT_MODEL.into()),
                Some(crate::week_review::DEFAULT_EFFORT.into()),
            ),
            _ => (None, None),
        };
        AgentPreset {
            name: self.name().into(),
            kind: AgentKind::default(),
            custom_harness: None,
            follow_box: Some(true),
            model,
            effort,
            prefix: String::new(),
            postfix: String::new(),
            skip_task: false,
            system: Some(self),
        }
    }
}

/// Give `presets` one of each SYSTEM PRESET: a preset already named for
/// one (an older list's own "PR autofix") becomes it, text and all; a
/// missing one is added, empty, at the end — after the presets the user
/// made, which keep their places. True when anything changed.
pub fn ensure_system(presets: &mut Vec<AgentPreset>) -> bool {
    let mut changed = false;
    for which in SystemPreset::ALL {
        if presets.iter().any(|p| p.system == Some(which)) {
            continue;
        }
        changed = true;
        match presets
            .iter_mut()
            .find(|p| p.system.is_none() && p.name.trim().eq_ignore_ascii_case(which.name()))
        {
            Some(own) => own.system = Some(which),
            None => presets.push(which.default_preset()),
        }
    }
    changed
}

/// The list as stored, with its SYSTEM PRESETS made sure of — and the
/// settings an older release kept in `config.json` for the agents they
/// carry moved onto them ([`crate::config::Config::move_into_presets`]) —
/// saved when that changed it: run once as orion starts, so the presets
/// list shows them from then on. The moved keys leave `config.json` only
/// once the presets holding them are saved.
pub fn seed_system() {
    let mut presets = load();
    let mut cfg = crate::config::Config::load();
    let added = ensure_system(&mut presets);
    let moved = cfg.move_into_presets(&mut presets);
    if !added && !moved {
        return;
    }
    if let Err(err) = save(&presets) {
        tracing::warn!(%err, "couldn't save the system presets");
        return;
    }
    if moved {
        if let Err(err) = cfg.try_save() {
            tracing::warn!(%err, "couldn't drop the settings the system presets took");
        }
    }
}

/// SYSTEM PRESET `which` as the list holds it — or as it would once
/// [`ensure_system`] ran, for a list that has not been seeded yet. Its
/// text as stored: blank while it is orion's ([`AgentPreset::with_builtin_text`]).
pub fn system(which: SystemPreset) -> AgentPreset {
    load_seeded()
        .into_iter()
        .find(|p| p.system == Some(which))
        .unwrap_or_else(|| which.default_preset())
}

/// The list as stored with its SYSTEM PRESETS made sure of
/// ([`ensure_system`]), as Settings → Presets shows it and its editor
/// indexes it — the stored list itself once orion has started
/// ([`seed_system`]).
pub fn load_seeded() -> Vec<AgentPreset> {
    let mut presets = load();
    ensure_system(&mut presets);
    presets
}

impl AgentPreset {
    /// Whether the preset runs on the box's own account and harness
    /// ([`Self::follow_box`]). An entry an older release wrote names a
    /// harness and no more; one left on the default Claude account — where
    /// every preset started, with nothing to say "follow" — follows, and
    /// one put on anything else keeps it.
    pub fn follows_box(&self) -> bool {
        self.follow_box
            .unwrap_or(self.kind == AgentKind::Claude && self.custom_harness.is_none())
    }

    /// The preset with the text it sends: a SYSTEM PRESET whose list holds
    /// no text of the user's says orion's ([`SystemPreset::builtin_text`]),
    /// so it is never blank. Any other preset as it is.
    pub fn with_builtin_text(mut self) -> Self {
        if let Some(which) = self.system.filter(|_| !self.has_wrapping()) {
            let (prefix, postfix) = which.builtin_text();
            self.prefix = prefix.into();
            self.postfix = postfix.into();
        }
        self
    }

    /// The preset as the list should hold it: a SYSTEM PRESET whose text
    /// is orion's word for word holds none — so it follows orion's text
    /// when that changes — and clearing it is the way back to orion's.
    pub fn without_builtin_text(mut self) -> Self {
        if let Some(which) = self.system {
            let (prefix, postfix) = which.builtin_text();
            if self.prefix.trim() == prefix.trim() && self.postfix.trim() == postfix.trim() {
                self.prefix.clear();
                self.postfix.clear();
            }
        }
        self
    }

    /// `claude · opus · high` / `codex · gpt-5.5` / `cursor` — the kind plus
    /// whichever of model and effort the preset pins; a CLAUDE ACCOUNT's
    /// email in place of the kind on a machine with more than one, and
    /// [`FOLLOW_BOX`] for a preset that follows the box.
    pub fn spec_label(&self) -> String {
        let harness = match self.follows_box() {
            true => FOLLOW_BOX.to_string(),
            false => crate::quick_prompt::harness_name(self.kind, self.custom_harness.as_deref()),
        };
        let mut parts = vec![harness];
        parts.extend(self.model.iter().cloned());
        parts.extend(self.effort.iter().cloned());
        parts.join(" · ")
    }

    /// True when the preset wraps the task in any text at all.
    pub fn has_wrapping(&self) -> bool {
        !self.prefix.trim().is_empty() || !self.postfix.trim().is_empty()
    }

    /// The starting prompt: prefix, task and postfix — each trimmed, empty
    /// parts skipped — joined by a blank line. Empty when all three are:
    /// the launch then has no starting prompt at all.
    pub fn compose(&self, task: &str) -> String {
        [self.prefix.as_str(), task, self.postfix.as_str()]
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The preset's text for a prompt already open, whose task is not ours
    /// to see: a prefix with the blank line its task goes under, a postfix
    /// with the blank line parting it from the task above, and both as
    /// [`Self::compose`] joins them with no task between. Empty when the
    /// preset holds no text.
    pub fn standalone_text(&self) -> String {
        let (prefix, postfix) = (self.prefix.trim(), self.postfix.trim());
        match (prefix.is_empty(), postfix.is_empty()) {
            (true, true) => String::new(),
            (false, true) => format!("{prefix}\n\n"),
            (true, false) => format!("\n\n{postfix}"),
            (false, false) => format!("{prefix}\n\n{postfix}"),
        }
    }
}

/// Which side of the task a preset's text goes: a PREFIX before it, a
/// POSTFIX after it, or both. The PRESET EDITOR shows one box per side —
/// its Text row picks — and `preset_text` (a hand-edited key,
/// [`crate::config::Config::preset_text`]) is what a new preset starts on:
/// prefix alone by default, the framing most people reach for and one box
/// to fill. Never stored on the preset — a side is what it holds, so the
/// form derives it ([`PresetText::for_preset`]) and an older orion reads
/// the file unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetText {
    Prefix,
    Postfix,
    Both,
}

impl PresetText {
    /// In cycle order, as the Text row and the setting step them.
    pub const ALL: [PresetText; 3] = [PresetText::Prefix, PresetText::Postfix, PresetText::Both];
    /// What a config that never set `preset_text` means.
    pub const DEFAULT: PresetText = PresetText::Prefix;

    /// The label the setting stores and both rows show.
    pub const fn as_str(self) -> &'static str {
        match self {
            PresetText::Prefix => "prefix",
            PresetText::Postfix => "postfix",
            PresetText::Both => "prefix & postfix",
        }
    }

    /// A label back to its side, case-insensitively and trimmed; `both` is
    /// accepted for a hand edit. None for anything else.
    pub fn parse(value: &str) -> Option<PresetText> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("both") {
            return Some(PresetText::Both);
        }
        Self::ALL
            .into_iter()
            .find(|side| side.as_str().eq_ignore_ascii_case(value))
    }

    pub fn has_prefix(self) -> bool {
        matches!(self, PresetText::Prefix | PresetText::Both)
    }

    pub fn has_postfix(self) -> bool {
        matches!(self, PresetText::Postfix | PresetText::Both)
    }

    /// The side (or sides) two flags name; None when neither is set.
    pub fn of_sides(prefix: bool, postfix: bool) -> Option<PresetText> {
        match (prefix, postfix) {
            (true, true) => Some(PresetText::Both),
            (true, false) => Some(PresetText::Prefix),
            (false, true) => Some(PresetText::Postfix),
            (false, false) => None,
        }
    }

    /// What the editor shows for a stored preset under the `default`
    /// setting: every side the preset already holds text on, plus the
    /// side(s) the setting names — so nothing saved is ever hidden, and a
    /// blank preset opens the way a new one would.
    pub fn for_preset(preset: &AgentPreset, default: PresetText) -> PresetText {
        Self::of_sides(
            !preset.prefix.trim().is_empty() || default.has_prefix(),
            !preset.postfix.trim().is_empty() || default.has_postfix(),
        )
        .unwrap_or(default)
    }
}

pub fn load() -> Vec<AgentPreset> {
    load_from(&store_path())
}

/// Persist the whole list, in order.
pub fn save(presets: &[AgentPreset]) -> std::io::Result<()> {
    save_to(&store_path(), presets)
}

pub(crate) fn store_path() -> PathBuf {
    // Never the developer's own list under test: a test that pinned none
    // reads (and writes) a file of this thread's that starts out missing.
    #[cfg(test)]
    let path = PRESETS_PATH_OVERRIDE
        .with(|p| p.borrow().clone())
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "orion-test-presets-{}-{:?}.json",
                std::process::id(),
                std::thread::current().id()
            ))
        });
    #[cfg(not(test))]
    let path = orion_core::paths::data_dir().join(orion_core::paths::PRESETS_FILE_NAME);
    path
}

fn load_from(path: &Path) -> Vec<AgentPreset> {
    let mut presets: Vec<AgentPreset> = orion_core::settings::read_list(path).0;
    // An entry an older release wrote says nothing of following: settle
    // it by the rule it reads by, so the next save writes it out once.
    for preset in &mut presets {
        preset.follow_box = Some(preset.follows_box());
    }
    presets
}

/// The `name` a raw preset entry carries, as the file spells it.
pub(crate) fn entry_name(entry: &Value) -> Option<&str> {
    entry.get("name")?.as_str()
}

/// Write `presets` in order. Each keeps the fields its stored entry (same
/// name) had that this build doesn't know, and every stored entry this build
/// can't read goes after them unless a preset now holds its name.
fn save_to(store: &Path, presets: &[AgentPreset]) -> std::io::Result<()> {
    let stored = orion_core::settings::read_array(store)
        .ok()
        .flatten()
        .unwrap_or_default();
    let named = |name: &str| {
        stored
            .iter()
            .find(|entry| entry_name(entry).is_some_and(|n| n.eq_ignore_ascii_case(name)))
    };
    let mut entries = Vec::with_capacity(presets.len());
    for preset in presets {
        let Value::Object(known) = serde_json::to_value(preset)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?
        else {
            unreachable!("a preset serializes to a JSON object");
        };
        let mut entry = named(&preset.name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        entry.extend(known);
        entries.push(Value::Object(entry));
    }
    for entry in &stored {
        let unreadable = serde_json::from_value::<AgentPreset>(entry.clone()).is_err();
        let taken = entry_name(entry)
            .is_some_and(|n| presets.iter().any(|p| p.name.eq_ignore_ascii_case(n)));
        if unreadable && !taken {
            entries.push(entry.clone());
        }
    }
    orion_core::settings::write_json(store, &Value::Array(entries))
}

#[cfg(test)]
thread_local! {
    static PRESETS_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test hook (the `with_config_path` pattern): route this thread's preset
/// store at `path` for the duration of `f`.
#[cfg(test)]
pub fn with_presets_path<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    PRESETS_PATH_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent_presets.json");
        (dir, path)
    }

    fn preset(name: &str, kind: AgentKind) -> AgentPreset {
        AgentPreset {
            name: name.into(),
            kind,
            custom_harness: None,
            follow_box: Some(false),
            model: None,
            effort: None,
            prefix: String::new(),
            postfix: String::new(),
            skip_task: false,
            system: None,
        }
    }

    /// A list with its own "PR autofix" keeps it, text and all, as the
    /// system preset; the Linear and Week review ones it lacks are added
    /// on orion's text — the review's on sonnet · medium; a second pass
    /// changes nothing; and the tag survives the file.
    #[test]
    fn system_presets_adopt_a_preset_by_name_and_fill_in_the_rest() {
        let (_dir, path) = store();
        let mut presets = vec![
            AgentPreset {
                prefix: "Fix it my way.".into(),
                ..preset("pr AUTOFIX", AgentKind::Claude)
            },
            preset("reviewer", AgentKind::Codex),
        ];
        assert!(ensure_system(&mut presets));
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["pr AUTOFIX", "reviewer", "Linear Ticket", "Week review"]
        );
        assert_eq!(presets[0].system, Some(SystemPreset::PrAutofix));
        assert_eq!(presets[0].prefix, "Fix it my way.", "the user's text kept");
        assert_eq!(presets[2].system, Some(SystemPreset::LinearTicket));
        assert!(!presets[2].has_wrapping(), "orion's text is kept as none");
        let review = &presets[3];
        assert_eq!(review.system, Some(SystemPreset::WeekReview));
        assert_eq!(
            (review.model.as_deref(), review.effort.as_deref()),
            (Some("sonnet"), Some("medium"))
        );
        assert!(!ensure_system(&mut presets), "once is enough");

        save_to(&path, &presets).unwrap();
        assert_eq!(load_from(&path), presets);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#""system": "pr_autofix""#), "{raw}");
        assert!(
            !raw.contains(r#""system": null"#),
            "a user's preset has none"
        );
    }

    /// A system preset is never blank: one holding no text says orion's,
    /// one holding the user's says theirs; and its text saved as orion's
    /// word for word, or cleared, is kept as nothing — so it follows
    /// orion's text when that changes. A user's preset is left alone.
    #[test]
    fn a_system_preset_always_holds_text_and_clearing_it_restores_orions() {
        for which in SystemPreset::ALL {
            let blank = which.default_preset();
            let (prefix, postfix) = which.builtin_text();
            assert!(!prefix.trim().is_empty(), "{which:?} has text of orion's");
            let shown = blank.clone().with_builtin_text();
            assert_eq!(
                (shown.prefix.as_str(), shown.postfix.as_str()),
                (prefix, postfix)
            );
            assert!(shown.has_wrapping());
            assert_eq!(shown.clone().without_builtin_text(), blank, "kept as none");
            let cleared = AgentPreset {
                prefix: "  ".into(),
                postfix: String::new(),
                ..shown
            };
            assert_eq!(
                cleared.with_builtin_text().prefix,
                prefix,
                "clearing it puts orion's back"
            );
            let own = AgentPreset {
                prefix: "Mine.".into(),
                ..blank
            };
            assert_eq!(own.clone().with_builtin_text(), own);
            assert_eq!(own.clone().without_builtin_text(), own);
        }
        let users = AgentPreset {
            prefix: crate::linear::TICKET_TEXT.into(),
            ..preset("mine", AgentKind::Claude)
        };
        assert_eq!(users.clone().without_builtin_text(), users);
        assert_eq!(
            preset("mine", AgentKind::Claude).with_builtin_text().prefix,
            ""
        );
    }

    #[test]
    fn missing_or_malformed_file_reads_empty() {
        let (_dir, path) = store();
        assert!(load_from(&path).is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn save_round_trips_in_order_and_leaves_no_temp_file() {
        let (_dir, path) = store();
        let presets = vec![
            AgentPreset {
                model: Some("opus".into()),
                effort: Some("high".into()),
                prefix: "Be strict.".into(),
                postfix: "Run the tests.".into(),
                ..preset("reviewer", AgentKind::Claude)
            },
            AgentPreset {
                skip_task: true,
                ..preset("scratch", AgentKind::Codex)
            },
        ];
        save_to(&path, &presets).unwrap();
        assert_eq!(load_from(&path), presets);
        assert!(!path.with_extension("json.tmp").exists());
        // A rewrite replaces, never appends.
        save_to(&path, &presets[1..]).unwrap();
        assert_eq!(load_from(&path), presets[1..].to_vec());
    }

    /// A preset for a harness this build has never heard of — a newer
    /// orion's — survives a save, and so does a field this build doesn't know.
    #[test]
    fn a_save_keeps_what_a_newer_orion_wrote() {
        let (_dir, path) = store();
        std::fs::write(
            &path,
            r#"[
                {"name": "reviewer", "kind": "codex", "pinned": true},
                {"name": "future", "kind": "antigravity", "model": "x"}
            ]"#,
        )
        .unwrap();
        let mut presets = load_from(&path);
        assert_eq!(presets.len(), 1, "only the readable preset lists");
        presets[0].prefix = "Be strict.".into();
        presets.push(preset("scratch", AgentKind::Claude));
        save_to(&path, &presets).unwrap();

        let saved: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let names: Vec<&str> = saved.iter().filter_map(entry_name).collect();
        assert_eq!(names, ["reviewer", "scratch", "future"]);
        assert_eq!(saved[0]["pinned"], true);
        assert_eq!(saved[0]["prefix"], "Be strict.");
        assert_eq!(saved[2]["kind"], "antigravity");
        assert_eq!(load_from(&path).len(), 2);
    }

    #[test]
    fn a_name_only_record_deserializes_with_defaults() {
        let (_dir, path) = store();
        std::fs::write(&path, r#"[{"name": "old"}]"#).unwrap();
        let presets = load_from(&path);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "old");
        assert_eq!(presets[0].kind, AgentKind::Claude);
        assert_eq!(presets[0].model, None);
        assert!(presets[0].prefix.is_empty() && presets[0].postfix.is_empty());
        assert!(!presets[0].skip_task, "an old record still asks for a task");
    }

    #[test]
    fn compose_skips_empty_parts_and_trims() {
        let mut p = preset("p", AgentKind::Claude);
        assert_eq!(p.compose(""), "", "nothing at all composes to nothing");
        assert_eq!(p.compose("  do it \n"), "do it");
        p.prefix = "PRE\n".into();
        assert_eq!(p.compose("do it"), "PRE\n\ndo it");
        p.postfix = "  POST".into();
        assert_eq!(p.compose("do it"), "PRE\n\ndo it\n\nPOST");
        assert_eq!(p.compose(" \n"), "PRE\n\nPOST", "the task is optional");
        p.prefix = "   ".into();
        assert_eq!(p.compose("line1\nline2"), "line1\nline2\n\nPOST");
        assert!(p.has_wrapping());
        p.postfix.clear();
        assert!(!p.has_wrapping());
    }

    #[test]
    fn standalone_text_leaves_room_for_the_task_on_the_side_it_goes() {
        let mut preset = preset("p", AgentKind::Claude);
        assert_eq!(preset.standalone_text(), "");
        preset.prefix = "  frame \n".into();
        assert_eq!(preset.standalone_text(), "frame\n\n");
        preset.postfix = " finish ".into();
        assert_eq!(preset.standalone_text(), "frame\n\nfinish");
        preset.prefix.clear();
        assert_eq!(preset.standalone_text(), "\n\nfinish");
    }

    #[test]
    fn preset_text_labels_round_trip_and_both_is_an_alias() {
        for side in PresetText::ALL {
            assert_eq!(PresetText::parse(side.as_str()), Some(side));
            assert_eq!(
                PresetText::parse(&format!(" {} ", side.as_str().to_uppercase())),
                Some(side)
            );
        }
        assert_eq!(PresetText::parse(" both "), Some(PresetText::Both));
        assert_eq!(PresetText::parse("suffix"), None);
        assert_eq!(PresetText::parse(""), None);
        assert_eq!(PresetText::DEFAULT, PresetText::Prefix);
        assert!(PresetText::Prefix.has_prefix() && !PresetText::Prefix.has_postfix());
        assert!(!PresetText::Postfix.has_prefix() && PresetText::Postfix.has_postfix());
        assert!(PresetText::Both.has_prefix() && PresetText::Both.has_postfix());
        assert_eq!(PresetText::of_sides(false, false), None);
    }

    /// A stored side is never hidden: the form shows what the preset holds
    /// on top of what the setting names, and a blank preset follows the
    /// setting alone.
    #[test]
    fn for_preset_shows_every_side_with_text_and_the_setting_for_the_rest() {
        use PresetText::*;
        let blank = preset("p", AgentKind::Claude);
        let pre = AgentPreset {
            prefix: "PRE".into(),
            ..blank.clone()
        };
        let post = AgentPreset {
            postfix: "POST".into(),
            ..blank.clone()
        };
        let both = AgentPreset {
            prefix: "PRE".into(),
            postfix: "POST".into(),
            ..blank.clone()
        };
        assert_eq!(PresetText::for_preset(&blank, Prefix), Prefix);
        assert_eq!(PresetText::for_preset(&blank, Postfix), Postfix);
        assert_eq!(PresetText::for_preset(&blank, Both), Both);
        assert_eq!(PresetText::for_preset(&pre, Prefix), Prefix);
        assert_eq!(
            PresetText::for_preset(&pre, Postfix),
            Both,
            "the prefix it holds stays on screen"
        );
        assert_eq!(
            PresetText::for_preset(&post, Prefix),
            Both,
            "the postfix it holds stays on screen"
        );
        assert_eq!(PresetText::for_preset(&post, Postfix), Postfix);
        assert_eq!(PresetText::for_preset(&both, Prefix), Both);
        let spaces = AgentPreset {
            postfix: "  \n".into(),
            ..blank
        };
        assert_eq!(
            PresetText::for_preset(&spaces, Prefix),
            Prefix,
            "whitespace is no text"
        );
    }

    #[test]
    fn spec_label_names_only_what_is_pinned() {
        assert_eq!(preset("p", AgentKind::Cursor).spec_label(), "cursor");
        let full = AgentPreset {
            model: Some("opus".into()),
            effort: Some("high".into()),
            ..preset("p", AgentKind::Claude)
        };
        assert_eq!(full.spec_label(), "claude · opus · high");
        let model_only = AgentPreset {
            model: Some("gpt-5.5".into()),
            ..preset("p", AgentKind::Codex)
        };
        assert_eq!(model_only.spec_label(), "codex · gpt-5.5");
        let following = AgentPreset {
            follow_box: Some(true),
            ..full
        };
        assert_eq!(
            following.spec_label(),
            format!("{FOLLOW_BOX} · opus · high")
        );
    }

    /// FOLLOW THE BOX: every new preset follows; an entry an older release
    /// wrote follows when it sat on the default Claude account — where
    /// every preset started — and keeps any other harness it named, a
    /// Claude account among them. Load settles it, and the next save
    /// writes it out.
    #[test]
    fn a_preset_follows_the_box_unless_it_named_a_harness() {
        assert!(SystemPreset::LinearTicket.default_preset().follows_box());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent_presets.json");
        std::fs::write(
            &path,
            r#"[
              {"name": "Linear Ticket", "kind": "claude", "custom_harness": null,
               "prefix": "Plan it.", "system": "linear_ticket"},
              {"name": "On riplo", "kind": "claude",
               "custom_harness": "claude-engineering-riplo-ai"},
              {"name": "On codex", "kind": "codex"},
              {"name": "Old riplo", "kind": "custom",
               "custom_harness": "claude-engineering-riplo-ai"}
            ]"#,
        )
        .unwrap();
        let presets = load_from(&path);
        let follows: Vec<bool> = presets.iter().map(AgentPreset::follows_box).collect();
        assert_eq!(follows, [true, false, false, false]);
        save_to(&path, &presets).unwrap();
        let saved: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let written: Vec<&Value> = saved.iter().map(|entry| &entry["follow_box"]).collect();
        assert_eq!(
            written,
            [
                &Value::Bool(true),
                &Value::Bool(false),
                &Value::Bool(false),
                &Value::Bool(false)
            ]
        );
        assert_eq!(load_from(&path), presets, "settled once");
    }
}
