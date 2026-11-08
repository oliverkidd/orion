//! AGENT PRESETS: saved launch definitions — an AGENT KIND, a MODEL / EFFORT
//! choice, the TEXT it sends with `{task}` where the task goes, and whether
//! to ask for a task at all — made in Settings → Presets and picked
//! wherever a launch is. Launching one asks for an optional task (or, with
//! `skip_task`, nothing) and hands the CLI its text, the task at `{task}`,
//! as its positional starting prompt.
//!
//! A plain JSON list in the DATA DIR beside `config.json`, in list order.
//! A missing or malformed file reads as empty — like the SSH HOSTS FILE it
//! is a convenience store, never load-bearing. Writes go through a temp
//! file + rename so a crash mid-write cannot truncate the list. The list is
//! read entry by entry, and a save keeps what this build can't read — a
//! preset for a harness a newer orion added, a field it has no name for —
//! so an older orion sharing the file never deletes a newer one's presets.
//! An entry from before a preset held one text — a `prefix` and a
//! `postfix` round the task — reads as its prefix, `{task}` and its
//! postfix ([`migrate_entry`]), and the next save writes it so.

use orion_core::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// What a preset that follows the box says where a harness goes: the
/// editor's Runs on row, the presets list's line.
pub const FOLLOW_BOX: &str = "the box's agent";
/// What the PR autofix preset runs on while it follows: the agent that
/// did the work on the pull request's branch.
pub const FOLLOW_PR: &str = "the PR's own agent";
/// What the week review is written through while it follows: the `⌘N`
/// default's account.
pub const FOLLOW_DEFAULT: &str = "your default agent";

/// Where a preset's text puts the task. Text that names no place for it
/// has the task after it, a blank line between ([`AgentPreset::compose`]).
pub const TASK: &str = "{task}";

/// The keys an entry from before one text held its text in, round the
/// task: read once ([`migrate_entry`]), never written.
const RETIRED_TEXT: [&str; 2] = ["prefix", "postfix"];

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
    /// What the preset sends: its TEXT, [`TASK`] where the task goes — at
    /// the end, after a blank line, when it names no place. May be empty,
    /// when the task goes alone.
    #[serde(default)]
    pub text: String,
    /// Launch without asking for a task: Enter on the preset starts the CLI
    /// on its text alone — a "commit and push" preset has nothing left to
    /// say. Off, the task is still asked for, but optional.
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
    /// request (`crate::automations::pr_fixer::prompt`); its model and effort are what
    /// that agent runs at when no session on the branch has run here to
    /// copy. It runs on the pull request's own agent.
    PrAutofix,
    /// The text around a Linear ticket launch's issues (`crate::linear`).
    LinearTicket,
    /// The rules a WEEK REVIEW is written by (`crate::week_review`), and
    /// the Claude account, model and effort that write it.
    WeekReview,
    /// What the BUG FIXER tells the agent it starts on a labelled Linear
    /// bug (`crate::automations::linear_builder`).
    BugFixer,
    /// What the FEATURE BUILDER tells the agent it starts on a labelled
    /// Linear feature (`crate::automations::linear_builder`).
    FeatureBuilder,
    /// What MEETING TAKEAWAYS asks of a meeting's transcript
    /// (`crate::automations::meeting_takeaways`), and what answers it.
    MeetingTakeaways,
    /// What the BLUE SKY BUILDER tells the agent it starts on an idea
    /// (`crate::automations::blue_sky`).
    BlueSky,
}

impl SystemPreset {
    pub const ALL: [SystemPreset; 7] = [
        SystemPreset::PrAutofix,
        SystemPreset::LinearTicket,
        SystemPreset::WeekReview,
        SystemPreset::BugFixer,
        SystemPreset::FeatureBuilder,
        SystemPreset::MeetingTakeaways,
        SystemPreset::BlueSky,
    ];

    /// The name the preset is listed under, and the name an older list's
    /// own preset is adopted by.
    pub const fn name(self) -> &'static str {
        match self {
            SystemPreset::PrAutofix => "PR autofix",
            SystemPreset::LinearTicket => "Linear Ticket",
            SystemPreset::WeekReview => "Week review",
            SystemPreset::BugFixer => "Bug fixer",
            SystemPreset::FeatureBuilder => "Feature builder",
            SystemPreset::MeetingTakeaways => "Meeting takeaways",
            SystemPreset::BlueSky => "Blue sky builder",
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
            SystemPreset::BugFixer => "fixes your labelled Linear bugs",
            SystemPreset::FeatureBuilder => "builds your labelled Linear features",
            SystemPreset::MeetingTakeaways => "pulls takeaways from your meetings",
            SystemPreset::BlueSky => "builds your blue sky ideas",
        }
    }

    /// orion's own text for the preset: what it says while the list holds
    /// none of the user's.
    pub fn builtin_text(self) -> &'static str {
        match self {
            SystemPreset::PrAutofix => crate::automations::pr_fixer::PRESET_TEXT,
            SystemPreset::LinearTicket => crate::linear::TICKET_TEXT,
            SystemPreset::WeekReview => crate::week_review::brief_rules(),
            SystemPreset::BugFixer => crate::automations::linear_builder::BUG_FIXER_TEXT,
            SystemPreset::FeatureBuilder => {
                crate::automations::linear_builder::FEATURE_BUILDER_TEXT
            }
            SystemPreset::MeetingTakeaways => crate::automations::meeting_takeaways::PRESET_TEXT,
            SystemPreset::BlueSky => crate::automations::blue_sky::PRESET_TEXT,
        }
    }

    /// The preset as a list that never had one gets it: orion's text, and
    /// — the one-shot asks', the week review's and meeting takeaways' —
    /// Claude at sonnet · medium.
    pub fn default_preset(self) -> AgentPreset {
        let (model, effort) = match self {
            SystemPreset::WeekReview | SystemPreset::MeetingTakeaways => (
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
            text: String::new(),
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
/// saved when that changed it, or when an entry still holds its text the
/// way an older release did ([`migrate_entry`]): run once as orion
/// starts, so the presets list shows them from then on and the file says
/// what this build reads. The moved keys leave `config.json` only once the
/// presets holding them are saved.
pub fn seed_system() {
    let path = store_path();
    let (mut presets, migrated) = read_from(&path);
    let mut cfg = crate::config::Config::load();
    let added = ensure_system(&mut presets);
    let moved = cfg.move_into_presets(&mut presets);
    if !added && !moved && !migrated {
        return;
    }
    if let Err(err) = save_to(&path, &presets) {
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
        if let Some(which) = self.system.filter(|_| !self.has_text()) {
            self.text = which.builtin_text().into();
        }
        self
    }

    /// The preset as the list should hold it: a SYSTEM PRESET whose text
    /// is orion's word for word holds none — so it follows orion's text
    /// when that changes — and clearing it is the way back to orion's.
    pub fn without_builtin_text(mut self) -> Self {
        if let Some(which) = self.system {
            if self.text.trim() == which.builtin_text().trim() {
                self.text.clear();
            }
        }
        self
    }

    /// What a preset that follows runs on, in its own words: the box's
    /// agent for a launch ([`FOLLOW_BOX`]); for orion's own, the pull
    /// request's agent ([`FOLLOW_PR`]) or the default agent
    /// ([`FOLLOW_DEFAULT`]), there being no box.
    pub fn follow_label(&self) -> &'static str {
        follow_label(self.system)
    }

    /// `claude · opus · high` / `codex · gpt-5.5` / `cursor` — the kind plus
    /// whichever of model and effort the preset pins; a CLAUDE ACCOUNT's
    /// email in place of the kind on a machine with more than one, and
    /// [`Self::follow_label`] for a preset that follows.
    pub fn spec_label(&self) -> String {
        let harness = match self.follows_box() {
            true => self.follow_label().to_string(),
            false => crate::quick_prompt::harness_name(self.kind, self.custom_harness.as_deref()),
        };
        let mut parts = vec![harness];
        parts.extend(self.model.iter().cloned());
        parts.extend(self.effort.iter().cloned());
        parts.join(" · ")
    }

    /// True when the preset sends any text of its own with the task.
    pub fn has_text(&self) -> bool {
        !self.text.replace(TASK, "").trim().is_empty()
    }

    /// The starting prompt: the text with `task` where it goes
    /// ([`Self::halves`]) — at the first [`TASK`], or after the text with a
    /// blank line between when it names no place. Sent with no task, the
    /// marker goes and so does the gap it leaves. Empty when both are: the
    /// launch then has no starting prompt at all.
    pub fn compose(&self, task: &str) -> String {
        let (before, after) = self.halves();
        let out = match task.trim() {
            "" => close_gap(&before, &after),
            task => format!("{before}{task}{after}"),
        };
        out.trim().to_string()
    }

    /// The text either side of where the task goes, its own spacing kept
    /// so that the task between them reads as [`Self::compose`] sends it:
    /// what comes before the first [`TASK`] and what follows it — a later
    /// marker closed up, the task having one place — or, for text that
    /// names no place, all of it and the blank line the task goes under.
    pub fn halves(&self) -> (String, String) {
        let text = self.text.trim();
        match text.split_once(TASK) {
            Some((before, after)) => {
                let mut rest = after.split(TASK);
                let first = rest.next().unwrap_or_default().to_string();
                (
                    before.to_string(),
                    rest.fold(first, |out, piece| close_gap(&out, piece)),
                )
            }
            None if text.is_empty() => (String::new(), String::new()),
            None => (format!("{text}\n\n"), String::new()),
        }
    }

    /// The preset's text for a prompt already open, whose task is not ours
    /// to see: what goes before the task, the caret where the task goes,
    /// or what follows it, parted from a task already typed; with text on
    /// both sides, both as [`Self::compose`] joins them with no task.
    /// Empty when the preset holds no text.
    pub fn standalone_text(&self) -> String {
        match self.halves() {
            (before, after) if before.trim().is_empty() => after,
            (before, after) if after.trim().is_empty() => before,
            _ => self.compose(""),
        }
    }
}

/// [`AgentPreset::follow_label`] for a preset that is `system`, or none.
pub fn follow_label(system: Option<SystemPreset>) -> &'static str {
    match system {
        Some(SystemPreset::PrAutofix) => FOLLOW_PR,
        Some(SystemPreset::WeekReview | SystemPreset::MeetingTakeaways) => FOLLOW_DEFAULT,
        Some(
            SystemPreset::LinearTicket
            | SystemPreset::BugFixer
            | SystemPreset::FeatureBuilder
            | SystemPreset::BlueSky,
        )
        | None => FOLLOW_BOX,
    }
}

/// `head` and `tail` joined where a task marker came out of the text with
/// no task to stand in it, the longer of the two breaks either side kept:
/// a blank line between when the marker had a paragraph to itself, a line
/// break when a line, a space when it sat inside one — so `Fix {task} now`
/// reads `Fix now`, and a marker on its own line leaves no hole. `head`'s
/// own lead and `tail`'s own end are left as they are.
fn close_gap(head: &str, tail: &str) -> String {
    let (left, right) = (head.trim_end(), tail.trim_start());
    let breaks = |gap: &str| gap.matches('\n').count();
    let gap = breaks(&head[left.len()..]).max(breaks(&tail[..tail.len() - right.len()]));
    let sep = match gap {
        0 => " ",
        1 => "\n",
        _ => "\n\n",
    };
    match (left.is_empty(), right.is_empty()) {
        (true, _) => right.to_string(),
        (_, true) => left.to_string(),
        _ => format!("{left}{sep}{right}"),
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
    read_from(path).0
}

/// The list at `path`, and whether a preset in it held its text the way
/// an older release wrote it ([`migrate_entry`]) — so the file is behind
/// what this build reads until the next save.
fn read_from(path: &Path) -> (Vec<AgentPreset>, bool) {
    let mut migrated = false;
    let presets = read_entries(path)
        .into_iter()
        .filter_map(|mut entry| {
            let old = migrate_entry(&mut entry);
            let mut preset = serde_json::from_value::<AgentPreset>(entry).ok()?;
            migrated |= old;
            // An entry an older release wrote says nothing of following:
            // settle it by the rule it reads by, so the next save writes
            // it out once.
            preset.follow_box = Some(preset.follows_box());
            Some(preset)
        })
        .collect();
    (presets, migrated)
}

/// The raw entries at `path`: none for a missing or malformed file.
fn read_entries(path: &Path) -> Vec<Value> {
    orion_core::settings::read_array(path)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// An entry from before a preset held one text, made one: its prefix,
/// [`TASK`] and its postfix — each that holds anything — a blank line
/// apart, as its text; the retired keys gone either way. An entry that
/// already has text of its own keeps it. (A `{task}` an old prefix spelled
/// out was only ever words; it is a marker from now on, and the first is
/// where the task goes.) True when the entry held either key.
fn migrate_entry(entry: &mut Value) -> bool {
    let Value::Object(map) = entry else {
        return false;
    };
    let old: Vec<Option<Value>> = RETIRED_TEXT.iter().map(|key| map.remove(*key)).collect();
    if old.iter().all(Option::is_none) {
        return false;
    }
    let has_text = map
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    if !has_text {
        let side = |at: usize| {
            old[at]
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let (prefix, postfix) = (side(0), side(1));
        if !prefix.is_empty() || !postfix.is_empty() {
            let text = [prefix.as_str(), TASK, postfix.as_str()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            map.insert("text".into(), Value::String(text));
        }
    }
    true
}

/// The `name` a raw preset entry carries, as the file spells it.
pub(crate) fn entry_name(entry: &Value) -> Option<&str> {
    entry.get("name")?.as_str()
}

/// Write `presets` in order. Each keeps the fields its stored entry (same
/// name) had that this build doesn't know — never the retired prefix and
/// postfix, which its text has taken — and every stored entry this build
/// can't read goes after them unless a preset now holds its name.
fn save_to(store: &Path, presets: &[AgentPreset]) -> std::io::Result<()> {
    let stored = read_entries(store);
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
        let mut entry: Map<String, Value> = named(&preset.name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for key in RETIRED_TEXT {
            entry.remove(key);
        }
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
            text: String::new(),
            skip_task: false,
            system: None,
        }
    }

    fn with_text(text: &str) -> AgentPreset {
        AgentPreset {
            text: text.into(),
            ..preset("p", AgentKind::Claude)
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
                text: "Fix it my way.".into(),
                ..preset("pr AUTOFIX", AgentKind::Claude)
            },
            preset("reviewer", AgentKind::Codex),
        ];
        assert!(ensure_system(&mut presets));
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "pr AUTOFIX",
                "reviewer",
                "Linear Ticket",
                "Week review",
                "Bug fixer",
                "Feature builder",
                "Meeting takeaways",
                "Blue sky builder"
            ]
        );
        assert_eq!(presets[0].system, Some(SystemPreset::PrAutofix));
        assert_eq!(presets[0].text, "Fix it my way.", "the user's text kept");
        assert_eq!(presets[2].system, Some(SystemPreset::LinearTicket));
        assert!(!presets[2].has_text(), "orion's text is kept as none");
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
            let builtin = which.builtin_text();
            assert!(!builtin.trim().is_empty(), "{which:?} has text of orion's");
            let shown = blank.clone().with_builtin_text();
            assert_eq!(shown.text, builtin);
            assert!(shown.has_text());
            assert_eq!(shown.clone().without_builtin_text(), blank, "kept as none");
            let cleared = AgentPreset {
                text: format!("  {TASK} \n"),
                ..shown
            };
            assert_eq!(
                cleared.with_builtin_text().text,
                builtin,
                "clearing it — a lone marker is no text — puts orion's back"
            );
            let own = AgentPreset {
                text: "Mine.".into(),
                ..blank
            };
            assert_eq!(own.clone().with_builtin_text(), own);
            assert_eq!(own.clone().without_builtin_text(), own);
        }
        let users = AgentPreset {
            text: crate::linear::TICKET_TEXT.into(),
            ..preset("mine", AgentKind::Claude)
        };
        assert_eq!(users.clone().without_builtin_text(), users);
        assert_eq!(
            preset("mine", AgentKind::Claude).with_builtin_text().text,
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
                text: format!("Be strict.\n\n{TASK}\n\nRun the tests."),
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
        presets[0].text = "Be strict.".into();
        presets.push(preset("scratch", AgentKind::Claude));
        save_to(&path, &presets).unwrap();

        let saved: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let names: Vec<&str> = saved.iter().filter_map(entry_name).collect();
        assert_eq!(names, ["reviewer", "scratch", "future"]);
        assert_eq!(saved[0]["pinned"], true);
        assert_eq!(saved[0]["text"], "Be strict.");
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
        assert!(presets[0].text.is_empty());
        assert!(!presets[0].skip_task, "an old record still asks for a task");
    }

    /// The task goes where `{task}` is; after the text, a blank line
    /// between, when the text names no place; and alone with no text.
    /// Sent with no task the marker goes, and the gap it leaves with it.
    #[test]
    fn compose_puts_the_task_at_its_marker_or_after_the_text() {
        assert_eq!(
            with_text("").compose(""),
            "",
            "nothing at all composes to nothing"
        );
        assert_eq!(with_text("").compose("  do it \n"), "do it");
        assert_eq!(
            with_text("PRE\n").compose("do it"),
            "PRE\n\ndo it",
            "no marker: after"
        );
        let around = with_text(&format!("PRE\n\n{TASK}\n\nPOST"));
        assert_eq!(around.compose("do it"), "PRE\n\ndo it\n\nPOST");
        assert_eq!(around.compose(" \n"), "PRE\n\nPOST", "the task is optional");
        assert!(around.has_text());
        let after = with_text(&format!("  {TASK}\n\nPOST"));
        assert_eq!(after.compose("line1\nline2"), "line1\nline2\n\nPOST");
        assert_eq!(after.compose(""), "POST");
        let inline = with_text(&format!("Fix {TASK} now."));
        assert_eq!(inline.compose("the login"), "Fix the login now.");
        assert_eq!(
            inline.compose(""),
            "Fix now.",
            "a marker inside a line leaves one space"
        );
        let lines = with_text(&format!("A\n{TASK}\nB"));
        assert_eq!(
            lines.compose(""),
            "A\nB",
            "a line to itself leaves one break"
        );
        // The task has one place: the first marker; a later one closes up.
        let twice = with_text(&format!("a {TASK} b {TASK} c"));
        assert_eq!(twice.compose("X"), "a X b c");
        assert_eq!(twice.compose(""), "a b c");
        assert!(!with_text(TASK).has_text(), "a lone marker is no text");
        assert_eq!(with_text(TASK).compose("go"), "go");
    }

    /// The text either side of the task, for a box or a pane to put the
    /// caret between, spaced so the task between them reads as `compose`
    /// sends it: all of it before, and a blank line, for text with no
    /// marker.
    #[test]
    fn halves_and_standalone_text_leave_room_for_the_task_where_it_goes() {
        let mut preset = with_text("");
        assert_eq!(preset.halves(), (String::new(), String::new()));
        assert_eq!(preset.standalone_text(), "");
        preset.text = "  frame \n".into();
        assert_eq!(preset.halves(), ("frame\n\n".into(), String::new()));
        assert_eq!(preset.standalone_text(), "frame\n\n");
        preset.text = format!("frame\n\n{TASK}\n\n finish ");
        assert_eq!(preset.halves(), ("frame\n\n".into(), "\n\n finish".into()));
        assert_eq!(preset.standalone_text(), "frame\n\nfinish");
        preset.text = format!("{TASK}\n\nfinish");
        assert_eq!(preset.standalone_text(), "\n\nfinish");
        preset.text = format!("Fix {TASK} now.");
        assert_eq!(preset.halves(), ("Fix ".into(), " now.".into()));
        assert_eq!(preset.standalone_text(), "Fix now.");
        // Whatever the text, the halves round a task are what compose sends.
        for text in [
            "a {task} b {task} c",
            "PRE\n\n{task}",
            "{task}\n\nPOST",
            "plain",
            "x {task}y",
        ] {
            preset.text = text.into();
            let (before, after) = preset.halves();
            assert_eq!(
                format!("{before}T{after}").trim(),
                preset.compose("T"),
                "{text}"
            );
        }
    }

    /// An entry from before one text — a prefix and a postfix round the
    /// task — reads as its prefix, `{task}` and its postfix, each that
    /// holds anything; and the save writes `text` alone, the retired keys
    /// gone, whatever was in the stored entry.
    #[test]
    fn an_old_prefix_and_postfix_read_as_one_text_round_the_task() {
        let (_dir, path) = store();
        std::fs::write(
            &path,
            r#"[
              {"name": "both", "prefix": "Be strict.", "postfix": "Run the tests."},
              {"name": "before", "prefix": "Plan first.\n", "postfix": ""},
              {"name": "after", "prefix": "  ", "postfix": "Then push."},
              {"name": "none", "prefix": "", "postfix": ""},
              {"name": "new", "text": "Mine.", "prefix": "stale"},
              {"name": "blank", "text": " ", "prefix": "Kept."}
            ]"#,
        )
        .unwrap();
        let (presets, migrated) = read_from(&path);
        assert!(migrated);
        let texts: Vec<&str> = presets.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Be strict.\n\n{task}\n\nRun the tests.",
                "Plan first.\n\n{task}",
                "{task}\n\nThen push.",
                "",
                "Mine.",
                "Kept.\n\n{task}",
            ]
        );
        assert_eq!(
            presets[0].compose("Fix auth"),
            "Be strict.\n\nFix auth\n\nRun the tests."
        );
        assert_eq!(
            presets[1].compose(""),
            "Plan first.",
            "as the prefix alone sent"
        );
        save_to(&path, &presets).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("prefix") && !raw.contains("postfix"), "{raw}");
        assert_eq!(
            read_from(&path),
            (presets, false),
            "read the same from then on"
        );
    }

    /// The user's own list, as their `agent_presets.json` holds it — Linear
    /// Ticket and PR autofix adopted as orion's (PR autofix never asking
    /// for a task), Subagent Delegation theirs, all on Claude with nothing
    /// to say they follow — comes through the start of a new build: each
    /// text is its prefix then `{task}`, orion's two stay orion's and keep
    /// their model and effort, Week review joins them, everything follows
    /// the box, and the file is rewritten once in the new shape.
    #[test]
    fn the_users_presets_come_through_the_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent_presets.json");
        let ticket = "You have a range of issues here that need fixing. please investigsate each upfront by reading the linear tickets.";
        let autofix = "On the PR linked to this worktree the ci/cd has some failing tasks. Only fix what is listed below.";
        let delegation =
            "Please now setof opus 5.5 subagents on high effort to enact the planned work.";
        let entry = |name: &str, prefix: &str, model: &str, effort: &str, extra: &str| {
            format!(
                r#"{{"custom_harness": null, "effort": {effort}, "kind": "claude", "model": {model},
                    "name": "{name}", "postfix": "", "prefix": "{prefix}", "skip_task": {extra}}}"#
            )
        };
        let file = format!(
            "[{}, {}, {}]",
            entry(
                "Linear Ticket",
                ticket,
                "\"opus\"",
                "\"medium\"",
                r#"false, "system": "linear_ticket""#
            ),
            entry(
                "PR autofix",
                autofix,
                "\"opus\"",
                "\"medium\"",
                r#"true, "system": "pr_autofix""#
            ),
            entry("Subagent Delegation", delegation, "null", "null", "false"),
        );
        std::fs::write(&path, file).unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            with_presets_path(path.clone(), || {
                seed_system();
                let presets = load();
                let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
                assert_eq!(
                    names,
                    [
                        "Linear Ticket",
                        "PR autofix",
                        "Subagent Delegation",
                        "Week review",
                        "Bug fixer",
                        "Feature builder",
                        "Meeting takeaways",
                        "Blue sky builder"
                    ]
                );
                assert_eq!(presets[0].text, format!("{ticket}\n\n{TASK}"));
                assert_eq!(presets[1].text, format!("{autofix}\n\n{TASK}"));
                assert_eq!(presets[2].text, format!("{delegation}\n\n{TASK}"));
                assert_eq!(presets[0].system, Some(SystemPreset::LinearTicket));
                assert_eq!(presets[1].system, Some(SystemPreset::PrAutofix));
                assert!(presets[1].skip_task);
                assert_eq!(presets[2].system, None);
                assert_eq!(
                    (presets[0].model.as_deref(), presets[0].effort.as_deref()),
                    (Some("opus"), Some("medium"))
                );
                assert!(presets.iter().all(AgentPreset::follows_box));
                let raw = std::fs::read_to_string(&path).unwrap();
                assert!(!raw.contains("\"prefix\""), "rewritten once:\n{raw}");
                // The ticket's text names no `{issues}`: the issues go where
                // its task does, after the user's words.
                let issue: crate::linear::LinearIssue = serde_json::from_value(serde_json::json!({
                    "id": "1", "identifier": "ENG-12", "title": "Login",
                    "url": "https://linear.app/x/issue/ENG-12", "status": "Todo"
                }))
                .unwrap();
                let sent = crate::linear::ticket_task(&presets[0], &[issue]);
                assert!(sent.starts_with(ticket), "{sent}");
                assert!(sent.contains("\n\n### ENG-12: Login"), "{sent}");
            })
        });
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
        // orion's own say what following means for them: there is no box.
        let autofix = SystemPreset::PrAutofix.default_preset();
        assert_eq!(autofix.spec_label(), FOLLOW_PR);
        let review = SystemPreset::WeekReview.default_preset();
        assert_eq!(
            review.spec_label(),
            format!("{FOLLOW_DEFAULT} · sonnet · medium")
        );
        assert_eq!(
            SystemPreset::LinearTicket.default_preset().spec_label(),
            FOLLOW_BOX
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
               "text": "Plan it.", "system": "linear_ticket"},
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
