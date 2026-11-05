//! TUI user settings, read from the same two layers the daemon reads —
//! `paths::config_path()` with `paths::config_local_path()` over it, see
//! `orion_core::settings` (each side deserializes only its own fields;
//! serde ignores the rest). Loaded fresh at each use so edits apply without
//! restarting the TUI. A missing file or unknown fields fall back to
//! defaults; a value this build can't read costs only its own key, which is
//! logged and left as stored.
//!
//! The settings overlay is the writer: it patches known keys and leaves
//! any other JSON fields (including future daemon keys) untouched, and a
//! key `config.local.json` holds is written back there, never into the
//! portable file.

use crate::agent_presets::{AgentPreset, PresetText};
use orion_core::compose::{WorktreeContainers, WORKTREE_CONTAINERS};
use orion_core::harness::HarnessDescriptor;
use orion_core::AgentKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Values the settings overlay cycles through for `session_idle_timeout`
/// (daemon-owned: how long unwatched idle sessions live before their PTY
/// is reaped).
pub const SESSION_IDLE_TIMEOUTS: &[&str] = &["off", "1m", "5m", "15m", "30m", "1h"];

/// The **Auto-fix my PRs** words (Settings → General), off and on, in the
/// order the row cycles them (`crate::autofix::Mode`).
pub const AUTOFIX_MODES: &[&str] = &[
    crate::autofix::Mode::Off.as_str(),
    crate::autofix::Mode::Auto.as_str(),
];

/// Values the settings overlay cycles through for `done_sound` (what rings
/// when a turn nobody watched reaches FINISHED) and `feedback_sound` (what
/// rings when one stops at NEEDS FEEDBACK, or dies mid-turn). `off` is
/// silence, `bell` the terminal BEL
/// (the one sound that reaches the local terminal over `orion ssh` — but
/// silent in Ghostty out of the box, whose `bell-features` default to
/// `no-audio`), the rest are macOS system sounds in `/System/Library/Sounds`,
/// played with `afplay`; see [`Config::done_sound`] for where a name falls
/// back to the bell. Hand-edited configs can name any sound in that folder.
pub const SOUNDS: &[&str] = &[
    "off",
    "bell",
    "Glass",
    "Ping",
    "Pop",
    "Hero",
    "Purr",
    "Tink",
    "Submarine",
    "Funk",
    "Blow",
    "Bottle",
    "Frog",
    "Morse",
    "Sosumi",
    "Basso",
];

/// Where the macOS system sounds live; `<name>.aiff` inside it.
const MACOS_SOUNDS_DIR: &str = "/System/Library/Sounds";

pub use orion_core::harness::DEFAULT_CHOICE;

/// What the overlay shows for a project's empty **Base branch**: the
/// daemon picks origin's default branch itself. Display only — the file
/// holds nothing, never this word.
pub const AUTO_CHOICE: &str = "auto";

/// The Projects tab with no project to list.
pub const NO_PROJECTS: &str = "no projects yet — each one you add gets its rows here";

/// What the Projects tab's **Setup command**, **Run command** and **Open
/// command** rows show while they are empty: there is nothing to run then.
pub const NO_COMMAND: &str = "not set";

// The static model/effort lists live in the core registry table now
// ([`orion_core::harness::builtin`]); what the pickers show is built
// below from the effective descriptor, so a `harnesses` override renames
// the rows everywhere at once. Claude's models still come from
// `claude_catalogue.rs` at runtime — CONFIG.JSON's `claude_models`, else
// Claude Code's own `availableModels`, else the aliases — and Cursor's
// from its catalogue (a seed plus a cached `cursor-agent --list-models`).

/// The `quick_prompt_kind` choices: every built-in harness id, by the name
/// the config file stores. Derived from the registry table rather than
/// spelled out, so a new built-in joins the cycle without a second edit.
pub fn agent_kind_names() -> Vec<String> {
    orion_core::harness::builtins()
        .iter()
        .map(|descriptor| descriptor.id.clone())
        .collect()
}

/// The model rows for a harness: [`DEFAULT_CHOICE`] first ("don't pass the
/// flag — let the CLI pick", what the daemon sees as None), then the
/// runtime catalogue (Claude/Cursor, already headed) or the descriptor's
/// static list.
pub fn model_choices(kind: AgentKind, custom: Option<&str>) -> Vec<String> {
    model_choices_in(&describe(kind, custom))
}

/// How a model id is shown in Accounts and the pickers: Claude family
/// aliases are the CLI's "latest of that family" names, so the row says so.
pub fn model_row_label(
    model: &str,
    catalog: Option<orion_core::harness::HarnessCatalog>,
) -> String {
    if catalog == Some(orion_core::harness::HarnessCatalog::Claude)
        && matches!(model, "opus" | "sonnet" | "haiku" | "fable")
    {
        format!("{model} · latest")
    } else {
        model.to_string()
    }
}

/// [`model_choices`] against an explicit descriptor, for callers that
/// already resolved one (the Accounts tab, the launch sites).
pub fn model_choices_in(descriptor: &orion_core::harness::HarnessDescriptor) -> Vec<String> {
    match descriptor.model.catalog {
        Some(orion_core::harness::HarnessCatalog::Claude) => crate::claude_catalogue::models()
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Some(orion_core::harness::HarnessCatalog::Cursor) => crate::cursor_catalogue::models()
            .iter()
            .map(|s| s.to_string())
            .collect(),
        None => headed(
            descriptor
                .model
                .models
                .iter()
                .map(|entry| entry.id.clone())
                .collect(),
        ),
    }
}

/// Effort rows for a harness given its chosen model (None or "default" =
/// the CLI's pick). Empty — no Effort row, no effort submenu — while the
/// harness offers no effort. Cursor's list follows the family (`-fast`
/// variants ride in the effort, `high-fast`); any other harness takes its
/// static list with any model.
pub fn effort_choices(kind: AgentKind, model: Option<&str>, custom: Option<&str>) -> Vec<String> {
    effort_choices_in(&describe(kind, custom), model)
}

/// [`effort_choices`] against an explicit descriptor, for callers that
/// already resolved one (the Accounts tab, the launch sites).
pub fn effort_choices_in(
    descriptor: &orion_core::harness::HarnessDescriptor,
    model: Option<&str>,
) -> Vec<String> {
    if !descriptor.effort.offered {
        return Vec::new();
    }
    if descriptor.model.catalog == Some(orion_core::harness::HarnessCatalog::Cursor) {
        return crate::cursor_catalogue::efforts(model)
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    headed(descriptor.effort.efforts.clone())
}

/// [`DEFAULT_CHOICE`] heading a choice list, without doubling a default
/// the source already carries.
fn headed(rest: Vec<String>) -> Vec<String> {
    headed_by(DEFAULT_CHOICE, rest)
}

/// `rest` behind `head` — the "follow the default" a row starts on — with
/// any `default` row of its own dropped, so the default is said once.
fn headed_by(head: &str, mut rest: Vec<String>) -> Vec<String> {
    rest.retain(|choice| !choice.eq_ignore_ascii_case(DEFAULT_CHOICE));
    rest.insert(0, head.to_string());
    rest
}

/// A blank row value as `label`, anything else as itself.
fn blank_as(value: &str, label: &str) -> String {
    match value.trim() {
        "" => label.into(),
        value => value.into(),
    }
}

/// `value` trimmed, while it is set and one of `choices`.
fn picked(value: &str, choices: &[String]) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && fits(value, choices)).then(|| value.to_string())
}

/// Whether `value` is one of `choices`, case-insensitively and trimmed —
/// how a form decides a saved or cycled choice still has a row.
pub(crate) fn fits(value: &str, choices: &[impl AsRef<str>]) -> bool {
    choices
        .iter()
        .any(|c| c.as_ref().eq_ignore_ascii_case(value.trim()))
}

/// The Claude accounts in `all` that orion would launch: on, and with
/// nothing wrong with their row.
fn claude_accounts_on(all: &[HarnessDescriptor]) -> Vec<&HarnessDescriptor> {
    orion_core::harness::usable(all)
        .into_iter()
        .filter(|entry| entry.is_claude_account())
        .collect()
}

/// Step `current` through an owned choice list, wrapping around; a value
/// off the list steps onto it. The owned twin of [`cycle_choice`], for
/// rows the registry builds at runtime.
pub(crate) fn cycle_owned(current: &str, choices: &[String], delta: i32) -> String {
    if choices.is_empty() {
        return current.to_string();
    }
    choices[cycled_index(current, choices, delta)].clone()
}

/// Where `current` lands in `choices` after `delta` steps, wrapping; a
/// value off the list (matched ignoring case) counts as the first.
/// Panics on an empty list.
fn cycled_index<S: AsRef<str>>(current: &str, choices: &[S], delta: i32) -> usize {
    let n = choices.len() as i32;
    let pos = choices
        .iter()
        .position(|c| c.as_ref().eq_ignore_ascii_case(current.trim()))
        .unwrap_or(0) as i32;
    (pos + delta).rem_euclid(n) as usize
}

/// The effort to launch with, given the harness, its model and the picked
/// effort. Most harnesses pass through. A composing harness (Cursor's
/// family-suffix shape: `--model <family>-<effort>`) fits instead: no
/// family → None; an effort the family ships → itself; anything else
/// ("default", unset, a suffix the family lacks) → None when the bare
/// family id exists, otherwise the family's fallback — most families have
/// no bare id, and a bare `--model claude-fable-5` is refused at spawn.
pub fn fit_effort(
    kind: AgentKind,
    model: Option<&str>,
    effort: Option<String>,
    custom: Option<&str>,
) -> Option<String> {
    fit_effort_in(&describe(kind, custom), model, effort)
}

/// [`fit_effort`] against an explicit descriptor, for callers that
/// already resolved one (the Accounts tab, the launch sites).
pub fn fit_effort_in(
    descriptor: &orion_core::harness::HarnessDescriptor,
    model: Option<&str>,
    effort: Option<String>,
) -> Option<String> {
    if !descriptor.compose_model_effort {
        return effort;
    }
    let family = model
        .map(str::trim)
        .filter(|m| !m.eq_ignore_ascii_case(DEFAULT_CHOICE))?;
    if descriptor.model.catalog == Some(orion_core::harness::HarnessCatalog::Cursor) {
        let choices = crate::cursor_catalogue::efforts(Some(family));
        if choices.is_empty() {
            return None;
        }
        let picked = effort
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| e != DEFAULT_CHOICE);
        return match picked {
            Some(e) if fits(&e, choices) => Some(e),
            _ if choices[0] == DEFAULT_CHOICE => None,
            _ => crate::cursor_catalogue::fallback_effort(family).map(String::from),
        };
    }
    if descriptor.effort.efforts.is_empty() {
        return None;
    }
    let picked = effort
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| e != DEFAULT_CHOICE);
    match picked {
        Some(e) if fits(&e, &descriptor.effort.efforts) => Some(e),
        _ => static_fallback_effort(&descriptor.effort.efforts).map(String::from),
    }
}

/// The fallback effort for a static list: `high`, else `medium`, else the
/// first the harness ships.
fn static_fallback_effort(efforts: &[String]) -> Option<&str> {
    efforts
        .iter()
        .find(|e| e.as_str() == "high")
        .or_else(|| efforts.iter().find(|e| e.as_str() == "medium"))
        .or_else(|| efforts.first())
        .map(String::as_str)
}

/// The effective descriptor `(kind, custom)` reads as: the registry row
/// with the `harnesses` map and the legacy per-harness keys folded in.
/// Loads the config fresh, like every other reader here. A broken entry still resolves (the picker, not the read,
/// hides it); launches refuse it with its reason. An id the registry no
/// longer names degrades to a placeholder under its own name, so rows
/// outliving their entry still render.
fn describe(kind: AgentKind, custom: Option<&str>) -> orion_core::harness::HarnessDescriptor {
    let id = match kind {
        AgentKind::Custom => custom.unwrap_or_default().trim(),
        _ => kind.as_str(),
    };
    let cfg = Config::load();
    if let Some(descriptor) = cfg
        .harness_registry()
        .into_iter()
        .find(|entry| entry.id == id)
    {
        return descriptor;
    }
    orion_core::harness::placeholder(id)
}

/// One setting row in the overlay; rows live inside a [`SettingsTab`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingSpec {
    pub kind: SettingKind,
    pub label: &'static str,
    pub hint: &'static str,
    /// Section header the row sits under, as `keymap::ActionSpec::group`
    /// is for the Keys tab. Empty means the tab lists the row bare;
    /// a tab whose rows all say so stays a flat list.
    pub group: &'static str,
}

/// What a tab shows. Ordinary tabs are a list of value settings. The
/// Projects tab is a list too, but of every project in turn: each one's
/// name, then these rows, which read and write that project's entry
/// instead of a top-level key ([`project_at`]). The
/// Keys tab is generated from [`crate::keymap::ACTIONS`] instead, so a
/// new action shows up there without being declared twice — and the Accounts
/// tab is generated from the harness registry, so a new CLI shows up
/// there without being declared twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBody {
    Values(&'static [SettingSpec]),
    Project(&'static [SettingSpec]),
    Keys,
    Accounts,
}

/// The Accounts tab's section listing every CLAUDE ACCOUNT, between the
/// head and the per-harness sections ([`Config::account_rows`]).
pub const ACCOUNTS_GROUP: &str = "Claude accounts";

/// What **Continue on** says after an account signed in as the session's
/// own email ([`Config::continue_targets`]).
pub const SAME_ACCOUNT: &str = " · same account";

/// The Accounts tab's static head: which account `⌘N` starts on. The
/// CLAUDE ACCOUNTS section and the per-harness sections below it are
/// generated from the registry (see [`Config::account_rows`] and
/// [`Config::agent_rows`]).
pub const ACCOUNTS_HEAD: &[SettingSpec] = &[SettingSpec {
    kind: SettingKind::QuickPromptKind,
    label: "New agents on",
    hint: "The account {quick_prompt} starts a new agent on, at that account's model and effort — or another harness",
    group: "",
}];

/// One tab of the settings overlay. Selection indices are per-tab: within
/// a `Values` tab they index its settings, within `Keys` they index
/// `keymap::ACTIONS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsTab {
    pub title: &'static str,
    pub body: TabBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    FollowNewSession,
    SessionIdleTimeout,
    PrAutofix,
    OutsideEditor,
    DoneSound,
    FeedbackSound,
    Theme,
    Animations,
    Spotify,
    /// The account (or harness) `⌘N` starts a new agent on.
    QuickPromptKind,
    /// A project's base branch: what its new worktrees start from.
    BaseBranch,
    /// A project's SETUP COMMAND: what runs once in each new worktree.
    SetupCommand,
    RunCommand,
    OpenCommand,
    /// A project's ON WORKTREE DELETE: what deleting one of its worktrees
    /// does to the docker compose projects started in it.
    WorktreeContainers,
    /// A project's read-only Linear row: where its `LINEAR_API_KEY` was
    /// found, if anywhere, and what the last test said
    /// (`linear::key_status`); Enter asks Linear whose key it is.
    LinearKey,
    /// The Presets tab's one row: Enter opens the AGENT PRESETS list.
    AgentPresets,
}

/// One harness field row in the Accounts tab. The tab renders one section
/// per registry entry — built-ins, Claude accounts and `harnesses` map ids
/// alike — with an Enabled row, a Model row, and an Effort row while the
/// harness offers effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessField {
    Enabled,
    Model,
    Effort,
}

/// One row of the Accounts tab's CLAUDE ACCOUNTS section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountRow {
    /// A Claude account, by registry id: built-in Claude (the default
    /// account), a `claude_accounts` entry, or a hand-written harness
    /// whose `env` pins a config dir.
    Account(String),
    /// **Add account**: a new config dir and its entry.
    Add,
}

impl HarnessField {
    pub fn label(self) -> &'static str {
        match self {
            HarnessField::Enabled => "Enabled",
            HarnessField::Model => "Model",
            HarnessField::Effort => "Effort",
        }
    }
}

impl SettingKind {
    /// A row whose value is typed, not toggled or cycled: Enter on it opens
    /// a one-line prompt pre-filled with the current value, and ←/→ have
    /// nothing to step through. [`Config::cycle`] leaves such a row alone;
    /// [`Config::set_project_text`] is what writes it.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            SettingKind::BaseBranch
                | SettingKind::SetupCommand
                | SettingKind::RunCommand
                | SettingKind::OpenCommand
        )
    }

    /// A row that says how things stand rather than holding a setting:
    /// nothing in the file behind it, nothing to cycle. Its value is the
    /// app's to tell (`linear::key_status`), and Enter is its one verb.
    pub fn is_status(self) -> bool {
        matches!(self, SettingKind::LinearKey)
    }

    /// A row on the PROJECTS TAB: its value is that project's, kept in
    /// its `projects` entry rather than at the top level of the file. A
    /// typed one ([`SettingKind::is_text`]) is written by
    /// [`Config::set_project_text`], a cycled one by
    /// [`Config::cycle_project`], and [`ProjectSettings::value_label`]
    /// reads both; the status row is the app's to tell.
    pub fn is_project(self) -> bool {
        matches!(
            self,
            SettingKind::BaseBranch
                | SettingKind::SetupCommand
                | SettingKind::RunCommand
                | SettingKind::OpenCommand
                | SettingKind::WorktreeContainers
                | SettingKind::LinearKey
        )
    }
}

/// Days since the Unix epoch of a proleptic Gregorian date (Howard
/// Hinnant's `days_from_civil`).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The tab strip, left to right: what orion does, the accounts it runs
/// agents on, each project's own settings, the presets, and the keys —
/// last because it is the biggest and the least casual.
pub const SETTINGS_TABS: &[SettingsTab] = &[
    SettingsTab {
        title: "General",
        body: TabBody::Values(GENERAL_SETTINGS),
    },
    // Generated from the harness registry: the head first, then the
    // CLAUDE ACCOUNTS, then one section per entry holding its model /
    // effort defaults, in registry order. A new CLI in the `harnesses`
    // map grows its own section with no code change.
    SettingsTab {
        title: "Accounts",
        body: TabBody::Accounts,
    },
    // Settings that belong to one project rather than to orion. The tab
    // lists every project under its own name, each with these rows, and a
    // row edits that project's entry in `projects` — so none of them reads
    // as a switch for every project at once.
    SettingsTab {
        title: "Projects",
        body: TabBody::Project(PROJECT_SETTINGS),
    },
    SettingsTab {
        title: "Presets",
        body: TabBody::Values(&[SettingSpec {
            kind: SettingKind::AgentPresets,
            label: "Agent presets",
            hint: "Enter opens the presets: orion's own — PR autofix, Linear Ticket, Week review — and yours",
            group: "",
        }]),
    },
    SettingsTab {
        title: "Keys",
        body: TabBody::Keys,
    },
];

/// The GENERAL TAB's rows, under their groups: how a launch goes, what
/// rings, and how orion looks.
pub const GENERAL_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        kind: SettingKind::FollowNewSession,
        label: "Follow new session",
        hint: "A new agent's card comes under the cursor, the grid scrolled to it, without entering it (off = stay on the card you're on)",
        group: "Launching",
    },
    SettingSpec {
        kind: SettingKind::SessionIdleTimeout,
        label: "Idle session timeout",
        hint: "Kill idle sessions in unviewed worktrees (busy ones spared; off disables)",
        group: "Launching",
    },
    SettingSpec {
        kind: SettingKind::PrAutofix,
        label: "Auto-fix my PRs",
        hint: "When one of your pull requests hits merge conflicts or failing checks, a fresh autofix agent starts on it at once, nothing to confirm: on the same agent, account, model and effort as the session that did the work — with none, your default agent at the PR autofix preset's model and effort. It is told that preset's text. ^G in the pull requests modal sends one whatever this says",
        group: "Launching",
    },
    SettingSpec {
        kind: SettingKind::OutsideEditor,
        label: "Open files in",
        hint: "What {open_outside} opens a file or checkout in; auto is the first installed of Cursor, VS Code, Sublime Text",
        group: "Launching",
    },
    SettingSpec {
        kind: SettingKind::DoneSound,
        label: "Turn finished",
        hint: "Ding, and notify an unfocused window, when a turn you haven't seen finishes (←/→ plays each; off silences both)",
        group: "Sounds",
    },
    SettingSpec {
        kind: SettingKind::FeedbackSound,
        label: "Needs you",
        hint: "Ring, and notify an unfocused window, when a turn stops to ask you or crashes (←/→ plays each; off silences both)",
        group: "Sounds",
    },
    SettingSpec {
        kind: SettingKind::Theme,
        label: "Theme",
        hint: "Accent colors used across the grid and overlays",
        group: "Look",
    },
    SettingSpec {
        kind: SettingKind::Animations,
        label: "Animations",
        hint: "Status text sweep and splash motion (off = fewer repaints)",
        group: "Look",
    },
    SettingSpec {
        kind: SettingKind::Spotify,
        label: "Spotify in footer",
        hint: "Show what Spotify is playing, with ⏮  ⏸ ⏭ buttons (macOS)",
        group: "Look",
    },
];

/// The PROJECTS TAB's rows, once under each project: where its worktrees
/// start, what runs and opens them, what deleting one does, and its
/// Linear key.
pub const PROJECT_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        kind: SettingKind::BaseBranch,
        label: "Base branch",
        hint: "Branch this project's new worktrees start from; Enter types one (empty = origin's default branch)",
        group: "",
    },
    SettingSpec {
        kind: SettingKind::SetupCommand,
        label: "Setup command",
        hint: "Shell line run once in each new worktree of this project, on a setup line in its band, e.g. just setup-worktree; {toggle_stack} (Start stack) waits for it",
        group: "",
    },
    SettingSpec {
        kind: SettingKind::RunCommand,
        label: "Run command",
        hint: "Shell line {toggle_stack} (Start stack) runs in a worktree's run terminal, e.g. just start",
        group: "",
    },
    SettingSpec {
        kind: SettingKind::OpenCommand,
        label: "Open command",
        hint: "Shell line {open_outside} → Open command runs to open a worktree of this project, e.g. open http://localhost:3000",
        group: "",
    },
    SettingSpec {
        kind: SettingKind::WorktreeContainers,
        label: "On worktree delete",
        hint: "What deleting one of this project's worktrees does to the docker compose projects started in it: leave them, stop them, remove them, or remove them with their volumes (data included)",
        group: "",
    },
    SettingSpec {
        kind: SettingKind::LinearKey,
        label: "Linear key",
        hint: "Where this project's LINEAR_API_KEY comes from — its .env.local, then its .env, then orion's own environment — never the key itself. Enter asks Linear whose key it is",
        group: "",
    },
];

/// Index of the Keys tab, which the overlay special-cases.
pub fn keys_tab() -> usize {
    SETTINGS_TABS
        .iter()
        .position(|t| t.body == TabBody::Keys)
        .expect("SETTINGS_TABS declares a Keys tab")
}

/// Index of the Accounts tab, generated from the harness registry.
pub fn accounts_tab() -> usize {
    SETTINGS_TABS
        .iter()
        .position(|t| t.body == TabBody::Accounts)
        .expect("SETTINGS_TABS declares an Accounts tab")
}

pub fn tab_count() -> usize {
    SETTINGS_TABS.len()
}

/// The static value settings of a tab: the declared list, or the Accounts
/// head (its per-harness sections are generated — see
/// [`Config::agent_rows`]). Empty for the Keys tab.
pub fn tab_settings(tab: usize) -> &'static [SettingSpec] {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings) | TabBody::Project(settings)) => settings,
        Some(TabBody::Accounts) => ACCOUNTS_HEAD,
        _ => &[],
    }
}

/// How many selectable rows a tab holds, with `projects` in the tree —
/// the Projects tab has its rows once for each. The Accounts tab reads the
/// registry, so a new CLI grows it without a code change.
pub fn tab_len(tab: usize, projects: usize) -> usize {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings)) => settings.len(),
        Some(TabBody::Project(settings)) => settings.len() * projects,
        Some(TabBody::Keys) => crate::keymap::ACTIONS.len(),
        Some(TabBody::Accounts) => {
            let cfg = Config::load();
            ACCOUNTS_HEAD.len() + cfg.account_rows().len() + cfg.agent_rows().len()
        }
        None => 0,
    }
}

/// The static value setting at a tab-local index, if the tab declares one
/// there: the full list on ordinary tabs, the head on the Accounts tab
/// (its harness rows resolve through [`Config::agent_row`]), never on
/// Keys. On the Projects tab, the same row in every project's block
/// ([`project_at`] says whose).
pub fn setting_at(tab: usize, index: usize) -> Option<&'static SettingSpec> {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Project(settings)) => settings.get(index % settings.len()),
        _ => tab_settings(tab).get(index),
    }
}

/// Which project a Projects tab row is: its index in the tree's project
/// list, each project's rows following the one before's. None on every
/// other tab.
pub fn project_at(tab: usize, index: usize) -> Option<usize> {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Project(settings)) => Some(index / settings.len()),
        _ => None,
    }
}

/// Where an Accounts tab harness row lives, as `(tab, row)`. Reads the
/// registry, like the tab itself.
pub fn locate_agent(id: &str, field: HarnessField) -> Option<(usize, usize)> {
    let tab = accounts_tab();
    let cfg = Config::load();
    cfg.agent_rows()
        .iter()
        .position(|(row_id, row_field)| row_id == id && *row_field == field)
        .map(|i| (tab, cfg.agent_rows_from() + i))
}

/// Where the Accounts tab's CLAUDE ACCOUNTS section starts, as `(tab, row)`:
/// its first row, the default account's, right under the head.
pub fn locate_accounts() -> (usize, usize) {
    (accounts_tab(), ACCOUNTS_HEAD.len())
}

/// The row declared for `kind`, wherever it sits — for anything that
/// wants its label or hint by name (the typed-row prompt's title).
pub fn spec_for(kind: SettingKind) -> Option<&'static SettingSpec> {
    all_settings()
        .map(|(_, _, spec)| spec)
        .find(|spec| spec.kind == kind)
}

/// Every static value setting, tab by tab, for coverage checks. The
/// Accounts tab contributes its head; its harness rows are covered through
/// [`Config::agent_rows`].
pub fn all_settings() -> impl Iterator<Item = (usize, usize, &'static SettingSpec)> {
    SETTINGS_TABS.iter().enumerate().flat_map(|(t, tab)| {
        tab_settings(t).iter().enumerate().map(move |(i, s)| {
            let _ = tab;
            (t, i, s)
        })
    })
}

/// The one-line hint under the selected row, whatever kind of row it is.
/// The Accounts tab reads the registry for its harness rows.
pub fn hint_at(tab: usize, index: usize) -> String {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(_) | TabBody::Project(_)) => {
            setting_at(tab, index).map(setting_hint).unwrap_or_default()
        }
        Some(TabBody::Keys) => crate::keymap::spec_at(index)
            .map(|s| s.hint)
            .unwrap_or("")
            .to_string(),
        Some(TabBody::Accounts) => Config::load().agent_hint_by_index(index),
        None => String::new(),
    }
}

/// A value row's hint — led, on the row that picks an app, by what is
/// installed on this machine.
fn setting_hint(spec: &SettingSpec) -> String {
    let installed = match spec.kind {
        SettingKind::OutsideEditor => {
            crate::outside_editor::installed_names(&crate::outside_editor::Places::here())
        }
        _ => return spec.hint.to_string(),
    };
    let list = match installed.as_slice() {
        [] => "none".to_string(),
        names => names.join(", "),
    };
    format!("Installed: {list}. {}", spec.hint)
}

/// One terminal row of the settings overlay body, in display order.
/// Shared by the renderer and mouse hit-testing so they can't drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsRow {
    Blank,
    Header(String),
    /// A Projects tab heading: the name and repo path of the tree's
    /// project at this index, which the renderer reads off the app — the
    /// row map only knows how many there are. Not selectable.
    Project(usize),
    /// Label + value line for the value setting at this tab-local index.
    Setting(usize),
    /// Label + chord list for `keymap::ACTIONS[index]`.
    Hotkey(usize),
    /// A line of warning under the rows it is about — the Accounts tab's
    /// same-account note under its CLAUDE ACCOUNTS. Not selectable.
    Note(String),
}

impl SettingsRow {
    /// The tab-local selection index this row stands for, if it's one the
    /// cursor can land on.
    pub fn index(&self) -> Option<usize> {
        match self {
            SettingsRow::Setting(i) | SettingsRow::Hotkey(i) => Some(*i),
            _ => None,
        }
    }
}

/// A tab's rows top to bottom, with `projects` in the tree for the
/// Projects tab to give each its own block.
pub fn settings_rows(tab: usize, projects: usize) -> Vec<SettingsRow> {
    match SETTINGS_TABS.get(tab).map(|t| t.body) {
        Some(TabBody::Values(settings)) => grouped(
            settings.iter().map(|s| s.group.to_string()),
            SettingsRow::Setting,
        ),
        Some(TabBody::Project(settings)) => {
            if projects == 0 {
                return vec![SettingsRow::Note(NO_PROJECTS.into())];
            }
            let mut rows = Vec::new();
            for project in 0..projects {
                if project > 0 {
                    rows.push(SettingsRow::Blank);
                }
                rows.push(SettingsRow::Project(project));
                let first = project * settings.len();
                rows.extend((first..first + settings.len()).map(SettingsRow::Setting));
            }
            rows
        }
        Some(TabBody::Keys) => grouped(
            crate::keymap::ACTIONS.iter().map(|s| s.group.to_string()),
            SettingsRow::Hotkey,
        ),
        Some(TabBody::Accounts) => {
            let cfg = Config::load();
            let head = ACCOUNTS_HEAD.iter().map(|s| s.group.to_string());
            let accounts = cfg.account_rows();
            let section = accounts.iter().map(|_| ACCOUNTS_GROUP.to_string());
            let rows = cfg.agent_rows();
            let groups = rows.iter().map(|(id, _)| cfg.section_title(id));
            let mut out = grouped(head.chain(section).chain(groups), SettingsRow::Setting);
            // The same-account warning, under the accounts it is about:
            // after **Add account**.
            let add = accounts
                .iter()
                .position(|row| *row == AccountRow::Add)
                .unwrap_or(accounts.len() - 1);
            let last = SettingsRow::Setting(ACCOUNTS_HEAD.len() + add);
            if let Some(at) = out.iter().position(|row| *row == last) {
                let notes = cfg.account_notes().into_iter().map(SettingsRow::Note);
                out.splice(at + 1..at + 1, notes);
            }
            out
        }
        None => Vec::new(),
    }
}

/// Lays a table that is already in group order out under its section
/// headers: a header whenever the group name changes, a blank line
/// before every header but the first, and no header at all for a row
/// whose group is empty — so a tab with no groups is the bare list it
/// always was.
fn grouped(
    groups: impl Iterator<Item = String>,
    row: fn(usize) -> SettingsRow,
) -> Vec<SettingsRow> {
    let mut rows = Vec::new();
    let mut current: Option<String> = None;
    for (i, group) in groups.enumerate() {
        if !group.is_empty() && current.as_deref() != Some(&group) {
            if !rows.is_empty() {
                rows.push(SettingsRow::Blank);
            }
            rows.push(SettingsRow::Header(group.clone()));
            current = Some(group);
        }
        rows.push(row(i));
    }
    rows
}

/// One `account_tags` entry: the letters and tint an account's ACCOUNT
/// TAG takes in place of the ones orion would pick.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AccountTagOverride {
    /// Two characters; longer is cut to two.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// An index into the tag tints ([`crate::theme::ACCOUNT_TINTS`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// MOVED to each project's **Base branch** (`projects.<repo>.base_branch`):
    /// through 2.x one branch every project's new worktrees started from.
    /// Read, never written; [`Config::adopt_project_keys`] copies it into
    /// every project once the tree is known, and the save after drops it.
    #[serde(skip_serializing)]
    pub worktree_base_branch: Option<String>,
    /// MOVED to each project's **On worktree delete**
    /// (`projects.<repo>.worktree_containers`), as `worktree_base_branch`.
    #[serde(skip_serializing)]
    pub worktree_containers: Option<String>,
    /// OPEN IN APP: the GUI editor ⌘O hands a file or a checkout to —
    /// `auto` (the default: the first installed of Cursor, VS Code,
    /// Sublime Text, else what macOS's `open` picks), `cursor`, `vscode`
    /// or `sublime`. See `outside_editor`.
    pub outside_editor: String,
    /// LINEAR ACCOUNT: whose issues ⌘L lists. Empty — the default — is
    /// the owner of the project's `LINEAR_API_KEY`, which for a personal
    /// key is the user; an email names another member of the workspace,
    /// for a key a team shares. Always written to `config.local.json`
    /// ([`LOCAL_KEYS`]), so `orion ssh` never carries it to a remote.
    /// Hand-edited: no Settings row.
    pub linear_assignee_email: String,
    /// FIRST-RUN ONBOARDING has been finished or skipped. Local-only so a
    /// shared `config.json` does not skip the wizard on a new machine.
    #[serde(default)]
    pub onboarded: bool,
    /// The orion version this machine last saw WHAT'S NEW for
    /// (`crate::whats_new`): the next launch of a newer build opens on the
    /// releases since. Empty on a config from before it existed. Local-only.
    #[serde(default)]
    pub seen_version: String,
    /// How long an idle session in an unviewed worktree lives before the
    /// daemon reaps its PTY: "1m", "5m", "15m", "30m", "1h"; "off"
    /// disables. Owned by the daemon (which does the parsing and reaping);
    /// the TUI writes it so the settings overlay can cycle it.
    pub session_idle_timeout: String,
    /// What rings when a turn reaches FINISHED with nobody watching it:
    /// "off", "bell" (terminal BEL) or the name of a macOS system sound
    /// (`Glass` by default, `Ping`, …; see [`SOUNDS`]). Resolved by
    /// [`Config::done_sound`], which falls back to the bell wherever
    /// `afplay` can't reach the user's speakers. When it rings — once the
    /// finish settles, once per unseen spell, never inside the fold of the
    /// last sound — is up to `app::DoneSounds`. The one knob for both the DONE
    /// SOUND and the `<session> finished` desktop notification an
    /// unfocused terminal window gets: "off" silences the pair.
    pub done_sound: String,
    /// What rings when a turn stops at NEEDS FEEDBACK — a permission
    /// prompt or a question the agent is parked on — or its CLI dies with
    /// an error mid-turn. Same values and resolution as `done_sound`;
    /// `Sosumi` by default so red and green sound different from the next
    /// room. The one knob for both the FEEDBACK SOUND and the desktop
    /// notification an unfocused terminal window gets: "off" silences the
    /// pair.
    pub feedback_sound: String,
    /// PRESET TEXT: which side of the task a new AGENT PRESET's text goes
    /// — `prefix` (one box, sent before the task), `postfix` (one box,
    /// sent after it) or `prefix & postfix` (both). The PRESET EDITOR
    /// opens a new preset on the box(es) named here, and its Text row
    /// changes one preset; a stored side with text always shows. Resolved
    /// by [`Config::preset_text`]; `prefix` by default, the framing most
    /// people reach for and one box to fill. Hand-edited: no Settings row.
    pub preset_text: String,
    /// Color theme name (see `theme::THEMES`). Unknown names fall back to
    /// the default theme.
    pub theme: String,
    /// Master switch for the TUI's animations (the running/needs-feedback
    /// status-text sweep and the splash's motion). Off trades them for
    /// fewer repaints on constrained machines.
    pub animations: bool,
    /// SPOTIFY IN FOOTER: the footer's readout of what the Spotify desktop
    /// app is playing, `♪ Midnight City · M83  ⏮  ⏸ ⏭`, its glyphs buttons
    /// (`crate::spotify`). macOS only, asked over AppleScript; on by
    /// default, and nothing shows while Spotify is closed. Off, the poll
    /// stops too.
    pub spotify: bool,
    /// Leave draft pull requests out of the PROJECT OPEN PRS GROUP and the
    /// `/` PALETTE's pull-request rows, so browsing what's open shows only
    /// the rows asking for a reviewer. A view filter, not a fetch filter:
    /// the open list's query still returns the drafts and the cache holds
    /// them, so switching this off shows them again at once, and a draft
    /// marked ready on GitHub joins the rows on the next refresh. Never
    /// touches a checkout, its sessions, or the checkout's own PR ROW in
    /// the SESSIONS PANEL — those describe work you have, not work you are
    /// browsing. Off by default; hand-edited, with no Settings row.
    pub hide_draft_prs: bool,
    /// Whether the DIFF VIEWER lists its files as the directory tree
    /// rather than flat paths — no Settings row: `⌘B` flips an open viewer
    /// and remembers it here, so the next `⌘E` opens the same way. On by
    /// default.
    pub diff_tree_view: bool,
    /// MOVED to each project's remembered merge method
    /// (`projects.<repo>.merge_method`, written by every merge), as
    /// `worktree_base_branch`: through 2.x the one method every merge form
    /// opened on.
    #[serde(skip_serializing)]
    pub pr_merge_method: Option<String>,
    /// **Auto-fix my PRs** (Settings → General): when one of your pull
    /// requests hits merge conflicts or failing checks, send an agent to
    /// fix it at once — `auto`, the default — or not, `off`. An older
    /// orion's `ask` is rewritten to `auto` on load ([`Config::migrate`]).
    /// See `crate::autofix`.
    pub pr_autofix: String,
    /// MOVED to the **PR autofix** SYSTEM PRESET's model and effort:
    /// through 2.x the autofix agent's fallback model and effort. Read,
    /// never written; [`Config::move_into_presets`] carries them across as
    /// orion starts, and the save after drops them.
    #[serde(skip_serializing)]
    pub autofix_model: Option<String>,
    #[serde(skip_serializing)]
    pub autofix_effort: Option<String>,
    /// MOVED to the **Week review** SYSTEM PRESET's harness, model and
    /// effort, as `autofix_model`: the Claude account the week in review
    /// was written through (empty = the default agent's), and its model
    /// and effort (empty = sonnet, medium).
    #[serde(skip_serializing)]
    pub review_account: Option<String>,
    #[serde(skip_serializing)]
    pub review_model: Option<String>,
    #[serde(skip_serializing)]
    pub review_effort: Option<String>,
    /// PROJECT SETTINGS: one [`ProjectSettings`] per project set up
    /// differently from the rest, keyed by the project's repo path as the
    /// DAEMON stores it — what the Settings → Projects tab edits for the
    /// selected project. A project with no entry reads as the defaults
    /// ([`ProjectSettings::default`]), and an entry that says nothing they
    /// don't is dropped on save ([`Config::set_project`]), so
    /// the map names only the projects that differ. One key to the file's
    /// rules: a value in here this build can't read costs the whole map,
    /// not one project.
    pub projects: BTreeMap<PathBuf, ProjectSettings>,
    /// Default model/effort for new Claude / Codex / Cursor sessions.
    /// "default" means "don't pass the flag" (the CLI picks); any other
    /// value is passed through verbatim, so hand-edited configs can name
    /// models the pickers don't list. Cursor's pair is a family plus the
    /// effort suffix the daemon joins onto it (see `cursor_catalogue.rs`).
    pub claude_model: String,
    /// The Claude model rows the pickers, the AGENTS TAB and the PRESET
    /// EDITOR offer, verbatim, in place of the built-in aliases — for an
    /// organization allowlist or a provider (Bedrock, Vertex, a gateway)
    /// whose ids the aliases don't reach: `["claude-sonnet-5",
    /// "us.anthropic.claude-opus-5-v1:0"]`. Empty (the default) means the
    /// list follows Claude Code's own `availableModels` when one is on
    /// disk, else the aliases; see `claude_catalogue.rs`. Hand-edited only.
    pub claude_models: Vec<String>,
    pub claude_effort: String,
    pub codex_model: String,
    pub codex_effort: String,
    pub cursor_model: String,
    pub cursor_effort: String,
    /// OpenCode's `--model` id (`provider/model`, passed verbatim). No
    /// effort key: OpenCode has no effort flag — reasoning is a per-model
    /// variant picked inside its own TUI — so its Accounts section has no
    /// Effort row and nothing to store for one.
    pub opencode_model: String,
    /// Which AGENT KINDS the NEW AGENT PICKER offers. Off leaves that
    /// harness out of the picker and the PR SESSION picker (and, for
    /// Claude, out of the standing PREWARM POOL slot); sessions that already
    /// exist keep attaching, resuming and restarting as before. Off until
    /// first-run onboarding or Settings → Accounts turns one on. Tests keep
    /// them on so an empty `{}` fixture still offers every built-in.
    pub claude_enabled: bool,
    pub codex_enabled: bool,
    pub cursor_enabled: bool,
    pub opencode_enabled: bool,
    /// Per-harness deltas over the compiled-in registry (`harnesses` in
    /// config.json): disable a built-in, repoint a program, rename a
    /// flag, or define a whole new CLI. Merged by
    /// [`Config::harness_registry`]; the Accounts tab, the `n` picker, the
    /// `e` presets, spawn and hooks all read the merged rows. A hand edit
    /// that breaks one entry refuses its launches with the reason, never
    /// the whole file (see `orion_core::settings`). Written even when
    /// empty: a save patches only the keys it writes, so a map emptied by
    /// removing its last entry (an account's deltas, say) has to be. A
    /// file with no `harnesses` key at all reads as a fresh config does
    /// ([`default_harnesses`]); one that has the key is read as written.
    #[serde(default = "default_harnesses")]
    pub harnesses: BTreeMap<String, orion_core::harness::HarnessOverride>,
    /// CLAUDE ACCOUNTS beyond the default one (`claude_accounts`): each a
    /// Claude Code config dir under a stable id, `{"id": "claude-2",
    /// "config_dir": "~/.claude-2"}`, launched as Claude's own row with
    /// `CLAUDE_CONFIG_DIR` pointing there ([`Config::harness_registry`]).
    /// The Accounts tab's **Claude accounts** section and first-run
    /// onboarding add and remove them; empty by default.
    pub claude_accounts: Vec<orion_core::claude_account::ClaudeAccount>,
    /// The ACCOUNT TAG a Claude account's rows wear, set by hand: by
    /// account id (`claude` for the default one), `{"tag": "wk", "tint":
    /// 2}`, either half alone. An account with no entry gets two letters
    /// from its name or email and the next tint in line
    /// ([`crate::claude_accounts::account_tag`]). Written only while set.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub account_tags: BTreeMap<String, AccountTagOverride>,
    /// Which AGENT KIND the QUICK PROMPT hotkey launches. Its model and
    /// effort come from that kind's own defaults above, so the setting is
    /// one name, not a third model/effort pair. Read through
    /// [`Config::quick_prompt_kind`], which steps around a harness that has
    /// since been switched off.
    pub quick_prompt_kind: String,
    /// FOLLOW NEW SESSION: a QUICK PROMPT launch lands the cursor on the
    /// new session's card — the grid scrolled to it, the pane showing it,
    /// the keys still on the cards — so a run of launches can be watched
    /// going up. It selects the card and never enters its terminal. On by
    /// default, a config predating the key too. Off, the cursor, the pane
    /// and FOCUS stay on the session the user was on while the new card
    /// goes up in its band, as a BACKGROUND LAUNCH already does for
    /// another project. Only a launch fired from a session card holds
    /// still: with no card under the cursor there is nothing to keep, and
    /// the cursor lands on the new session either way.
    pub follow_new_session: bool,
    /// Hotkey overrides, keyed by `keymap::ActionSpec::id`; the value is a
    /// comma-separated chord list (`"j, down"`), and an empty string means
    /// deliberately unbound. Only rows that differ from the defaults are
    /// written, so the file stays small and new defaults reach existing
    /// installs. See [`crate::keymap`].
    pub keybindings: BTreeMap<String, String>,
    /// Keys the files held that this build could not read; they loaded as
    /// defaults. Never written: [`Config::save`] reads it to leave those
    /// stored values alone — most likely a newer orion's — unless the
    /// setting has been changed since. See `orion_core::settings`.
    #[serde(skip)]
    pub skipped: BTreeSet<String>,
}

/// One project's own settings — the PROJECTS TAB's rows — kept under the
/// project's repo path in [`Config::projects`]. Read through
/// [`Config::project`], which supplies the defaults for a project with no
/// entry; written through [`Config::set_project`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ProjectSettings {
    /// The branch this project's new WORKTREES start from when nobody
    /// named a base (a bare `orion worktree`, the QUICK PROMPT's **+ new
    /// worktree**). Empty — shown as `auto` — is origin's own default
    /// branch, `origin/HEAD` freshly fetched; a name (`dev`, `master`) is
    /// origin's fetched copy of that branch when origin has one, else the
    /// checkout's local branch of that name, else the default again. The
    /// DAEMON reads it (`orion-daemon/src/config.rs`); the TUI edits it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_branch: String,
    /// The SETUP COMMAND the DAEMON runs once in each worktree it creates
    /// for this project, on a setup line in its band — `just
    /// setup-worktree`, say. Empty (shown as `not set`) is none. The
    /// DAEMON reads it; the TUI only edits it. Left out of the file while
    /// empty, like `run_command`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub setup_command: String,
    /// The RUN COMMAND a menu's **Run** starts in this project's
    /// worktrees, typed on the Projects tab. Empty — the default, shown as
    /// `not set` — is none. The DAEMON reads it
    /// (`orion-daemon/src/config.rs`); the TUI only edits it. Left out
    /// of the file while empty, so an entry written before the row reads
    /// the same after a save.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub run_command: String,
    /// The OPEN COMMAND `Shift+Enter` / `Shift+O` fires on this project's
    /// worktrees, typed on the Projects tab — `open http://localhost:3000`,
    /// say. Empty (shown as `not set`) is none. The TUI both edits and runs it
    /// (`event_loop::open_worktree`): what it opens belongs on the machine
    /// the user sits at, never the DAEMON's. Left out of the file while
    /// empty, like `run_command`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub open_command: String,
    /// ON WORKTREE DELETE: what deleting one of this project's worktrees
    /// does to the docker compose projects started in it — `stop`,
    /// `remove` or `remove+volumes`; empty (shown as `off`) leaves them
    /// running. The DAEMON reads it (`containers`); the TUI cycles it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worktree_containers: String,
    /// The method this project's last merge from the PULL REQUESTS MODAL
    /// took — `squash`, `merge` or `rebase` — which its next merge form
    /// opens on ([`Config::merge_method`]). No row: every merge writes it.
    /// Empty is squash.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub merge_method: String,
    /// Keys in the entry this build doesn't know — a newer orion's, most
    /// likely, or the retired `hide_root_worktree` an older one wrote —
    /// carried through a save untouched, as the file's top-level keys
    /// are. An entry holding one is never dropped as "all default".
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

impl ProjectSettings {
    /// The overlay's label for a PROJECTS TAB row
    /// ([`SettingKind::is_project`]); empty for a row that is not one, and
    /// for the status row, which is the app's to tell.
    pub fn value_label(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::BaseBranch => blank_as(&self.base_branch, AUTO_CHOICE),
            SettingKind::SetupCommand => blank_as(&self.setup_command, NO_COMMAND),
            SettingKind::RunCommand => blank_as(&self.run_command, NO_COMMAND),
            SettingKind::OpenCommand => blank_as(&self.open_command, NO_COMMAND),
            SettingKind::WorktreeContainers => self.containers().as_str().into(),
            _ => String::new(),
        }
    }

    /// The **On worktree delete** policy; a word this build doesn't know
    /// is `off`.
    pub fn containers(&self) -> WorktreeContainers {
        WorktreeContainers::parse(&self.worktree_containers)
    }

    /// The stored text of a typed PROJECTS TAB row as its prompt pre-fills
    /// it — `""` for an unset row, never the `not set` the overlay shows
    /// in its place. Empty for a row that is not typed.
    pub fn text_value(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::BaseBranch => self.base_branch.clone(),
            SettingKind::SetupCommand => self.setup_command.clone(),
            SettingKind::RunCommand => self.run_command.clone(),
            SettingKind::OpenCommand => self.open_command.clone(),
            _ => String::new(),
        }
    }

    /// Write a typed value into a typed PROJECTS TAB row, trimmed. Empty
    /// puts the row back on its default. False for a row that is not a
    /// typed project row — nothing changes.
    pub fn set_text(&mut self, kind: SettingKind, value: &str) -> bool {
        let field = match kind {
            SettingKind::BaseBranch => &mut self.base_branch,
            SettingKind::SetupCommand => &mut self.setup_command,
            SettingKind::RunCommand => &mut self.run_command,
            SettingKind::OpenCommand => &mut self.open_command,
            _ => return false,
        };
        *field = value.trim().to_string();
        true
    }

    /// Step a cycled PROJECTS TAB row; false for any other row. `off` is
    /// stored as nothing, so a project back on the defaults leaves no
    /// entry.
    pub fn cycle(&mut self, kind: SettingKind, delta: i32) -> bool {
        if kind != SettingKind::WorktreeContainers {
            return false;
        }
        let step = if delta == 0 { 1 } else { delta };
        let next = cycle_choice(self.containers().as_str(), WORKTREE_CONTAINERS, step);
        self.worktree_containers = match WorktreeContainers::parse(next) {
            WorktreeContainers::Off => String::new(),
            _ => next.to_string(),
        };
        true
    }

    /// Keep `method` as the one this project merges by. Squash, where a
    /// project with no memory starts, is stored as nothing, so a project
    /// back on it leaves no entry.
    pub fn set_merge_method(&mut self, method: crate::pr_actions::MergeMethod) {
        self.merge_method = match method {
            crate::pr_actions::MergeMethod::Squash => String::new(),
            other => other.as_str().to_string(),
        };
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            worktree_base_branch: None,
            worktree_containers: None,
            outside_editor: crate::outside_editor::AUTO.into(),
            linear_assignee_email: String::new(),
            onboarded: false,
            seen_version: String::new(),
            session_idle_timeout: orion_core::settings::DEFAULT_SESSION_IDLE_TIMEOUT.into(),
            done_sound: "Glass".into(),
            feedback_sound: "Sosumi".into(),
            preset_text: PresetText::DEFAULT.as_str().into(),
            theme: "default".into(),
            animations: true,
            spotify: true,
            hide_draft_prs: false,
            diff_tree_view: true,
            pr_merge_method: None,
            pr_autofix: crate::autofix::Mode::Auto.as_str().into(),
            autofix_model: None,
            autofix_effort: None,
            review_account: None,
            review_model: None,
            review_effort: None,
            projects: BTreeMap::new(),
            claude_model: DEFAULT_CHOICE.into(),
            claude_models: Vec::new(),
            claude_effort: DEFAULT_CHOICE.into(),
            codex_model: DEFAULT_CHOICE.into(),
            codex_effort: DEFAULT_CHOICE.into(),
            cursor_model: DEFAULT_CHOICE.into(),
            cursor_effort: DEFAULT_CHOICE.into(),
            opencode_model: DEFAULT_CHOICE.into(),
            claude_enabled: cfg!(test),
            codex_enabled: cfg!(test),
            cursor_enabled: cfg!(test),
            opencode_enabled: cfg!(test),
            harnesses: default_harnesses(),
            claude_accounts: Vec::new(),
            account_tags: BTreeMap::new(),
            quick_prompt_kind: AgentKind::Claude.as_str().into(),
            follow_new_session: true,
            keybindings: BTreeMap::new(),
            skipped: BTreeSet::new(),
        }
    }
}

/// Keys a save always writes to `config.local.json`, never to the
/// `config.json` that `orion ssh` carries to other machines: who the user
/// is stays on this one. The local file is created for a key set away
/// from its default, and an unreadable one is never rewritten — the key
/// goes unsaved instead.
const LOCAL_KEYS: &[&str] = &["linear_assignee_email", "onboarded", "seen_version"];

/// Production starts with every harness off, including grok (whose
/// built-in default is on, so the map has to say otherwise) — a fresh
/// config, and a `config.json` with no `harnesses` key at all, which is
/// why the field's serde default is this too: serde's own default for a
/// missing field is an empty map, which left Grok Build on. Tests keep an
/// empty map so `{}` still offers the compiled-in set, unless they ask
/// for the shipped one ([`with_shipped_defaults`]).
fn default_harnesses() -> BTreeMap<String, orion_core::harness::HarnessOverride> {
    #[cfg(test)]
    if !SHIPPED_DEFAULTS.with(std::cell::Cell::get) {
        return BTreeMap::new();
    }
    let mut harnesses = BTreeMap::new();
    harnesses.insert(
        "grok".into(),
        orion_core::harness::HarnessOverride {
            enabled: Some(false),
            ..Default::default()
        },
    );
    harnesses
}

/// Load the settings, apply `edit` and save them: for a setting an action
/// changes as it happens (setup's rows, the version what's new was seen
/// at, the method a merge took, how `⌘B` left the file list). A save that
/// fails is logged, and nothing waits on it.
pub fn persist(edit: impl FnOnce(&mut Config)) {
    let mut cfg = Config::load();
    edit(&mut cfg);
    if let Err(err) = cfg.try_save() {
        tracing::warn!(%err, "couldn't save the settings");
    }
}

impl Config {
    pub fn load() -> Self {
        let cfg = load_layers(&settings_path(), &local_settings_path());
        // The Claude model rows follow `claude_models` live, as every
        // other hand edit does. Not under test: the list is process-global
        // and a test that never pinned the path would install the dev's.
        #[cfg(not(test))]
        crate::claude_catalogue::sync_config(&cfg.claude_models);
        cfg
    }

    /// Patch this config's known keys into the settings files, preserving
    /// any other fields already there: a key `config.local.json` holds goes
    /// back into it, every other key into `config.json`.
    pub fn save(&self) -> std::io::Result<()> {
        // A test that reaches a save without pinning the path would write
        // the dev's own settings file (and `ORION_DATA_DIR` only moves it
        // to their dev instance's, which is no better). Saves hang off
        // ordinary keystrokes now — `Shift+W` is one — so make the miss
        // loud instead of leaving it to be noticed in a diff later.
        #[cfg(test)]
        assert!(
            CONFIG_PATH_OVERRIDE.with(|p| p.borrow().is_some()),
            "Config::save() in a test without a path override — wrap the \
             test body in config::with_config_path (or with_default_config)"
        );
        self.write_layers(&settings_path(), &local_settings_path(), false)
    }

    /// [`save`] that no-ops in a test that never pinned a config path, so
    /// first-run onboarding can stamp `onboarded` without writing the
    /// developer's real settings.
    pub fn try_save(&self) -> std::io::Result<()> {
        #[cfg(test)]
        if CONFIG_PATH_OVERRIDE.with(|p| p.borrow().is_none()) {
            return Ok(());
        }
        self.save()
    }

    /// Put every setting back to its default and return the result.
    /// `config.json` is rewritten from scratch rather than patched like
    /// [`Config::save`] and `config.local.json` is removed, so keys the
    /// overlay doesn't own — anything hand-added — go too: a reset reads as
    /// if neither file had ever been edited.
    pub fn reset_to_defaults() -> std::io::Result<Self> {
        #[cfg(test)]
        assert!(
            CONFIG_PATH_OVERRIDE.with(|p| p.borrow().is_some()),
            "Config::reset_to_defaults() in a test without a path override — wrap \
             the test body in config::with_config_path (or with_default_config)"
        );
        let local = local_settings_path();
        match std::fs::remove_file(&local) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        let cfg = Self::default();
        cfg.write_layers(&settings_path(), &local, true)?;
        Ok(cfg)
    }

    /// Write this config's known keys into the two layers and swap each
    /// file that changed into place atomically. `fresh` starts `config.json`
    /// from an empty object instead of patching what is there.
    fn write_layers(&self, path: &Path, local: &Path, fresh: bool) -> std::io::Result<()> {
        use orion_core::settings;
        let mut root = if fresh {
            settings::Object::new()
        } else {
            patchable_object(path)?
        };
        // A local layer that isn't a readable object was ignored on load,
        // so it holds no keys — and is never rewritten from a partial view.
        let local_read = settings::read_object(local);
        let local_unreadable = local_read.is_err();
        let mut local_root = local_read.ok().flatten();
        let serde_json::Value::Object(known) = serde_json::to_value(self).map_err(invalid_data)?
        else {
            unreachable!("Config serializes to a JSON object");
        };
        let defaults = serde_json::to_value(Self::default()).map_err(invalid_data)?;
        let mut local_changed = false;
        for (key, value) in known {
            // A stored value this build couldn't read loaded as its default.
            // Unless it has been changed since, leave the stored one alone.
            if self.skipped.contains(&key) && defaults.get(&key) == Some(&value) {
                continue;
            }
            if LOCAL_KEYS.contains(&key.as_str()) {
                root.remove(&key);
                let at_default = defaults.get(&key) == Some(&value);
                let already_held = local_root
                    .as_ref()
                    .is_some_and(|held| held.contains_key(&key));
                // Stay out of `config.local.json` until the user sets the
                // key (or it was already there). An empty Linear email
                // is the default — "owner of the key" — and must not
                // appear just because another local key exists.
                if local_unreadable || (at_default && !already_held) {
                    continue;
                }
                let held = local_root.get_or_insert_with(settings::Object::new);
                if held.get(&key) != Some(&value) {
                    held.insert(key, value);
                    local_changed = true;
                }
                continue;
            }
            match local_root.as_mut() {
                Some(held) if held.contains_key(&key) => {
                    if held.get(&key) != Some(&value) {
                        held.insert(key, value);
                        local_changed = true;
                    }
                }
                _ => {
                    root.insert(key, value);
                }
            }
        }
        // A key that moved elsewhere goes once its value has: until then
        // the file keeps it for the move to read.
        for (key, waiting) in self.moved_keys() {
            if waiting {
                continue;
            }
            root.remove(key);
            if let Some(held) = local_root.as_mut() {
                local_changed |= held.remove(key).is_some();
            }
        }
        settings::write_json(path, &serde_json::Value::Object(root))?;
        if let (true, Some(held)) = (local_changed, local_root) {
            settings::write_json(local, &serde_json::Value::Object(held))?;
        }
        Ok(())
    }

    /// The top-level keys an older release kept that this one keeps
    /// elsewhere, each with whether its value is still waiting to move
    /// ([`Config::adopt_project_keys`]). Never written; a save drops each
    /// one once it is spent.
    fn moved_keys(&self) -> [(&'static str, bool); 8] {
        [
            ("worktree_base_branch", self.worktree_base_branch.is_some()),
            ("worktree_containers", self.worktree_containers.is_some()),
            ("pr_merge_method", self.pr_merge_method.is_some()),
            ("autofix_model", self.autofix_model.is_some()),
            ("autofix_effort", self.autofix_effort.is_some()),
            ("review_account", self.review_account.is_some()),
            ("review_model", self.review_model.is_some()),
            ("review_effort", self.review_effort.is_some()),
        ]
    }

    /// `theme` resolved to the palette the UI draws with.
    pub fn theme(&self) -> crate::theme::Theme {
        crate::theme::Theme::by_name(&self.theme)
    }

    /// The editor the file overlays launch: `ORION_EDITOR` when set,
    /// otherwise fresh ([`crate::editor::command`]).
    pub fn editor_command(&self) -> String {
        crate::editor::command()
    }

    /// The effective registry this config reads, every Claude account
    /// named after who it is signed in as — `Claude (a@b.co)`
    /// ([`crate::claude_accounts::label`]) — which is the name every
    /// picker, the Accounts tab, onboarding, presets and **Continue on**
    /// show. See [`Config::raw_harness_registry`] for the rows themselves.
    pub fn harness_registry(&self) -> Vec<HarnessDescriptor> {
        let mut all = self.raw_harness_registry();
        for entry in &mut all {
            if let Some(record) = crate::claude_accounts::record_of(entry) {
                entry.label = crate::claude_accounts::label(&entry.label, &record);
            }
        }
        all
    }

    /// The effective registry as launches read it: the compiled-in known
    /// harnesses with the `harnesses` map applied — Claude's extra
    /// accounts (`claude_accounts`) right after it — then map-only new
    /// ids — plus, for
    /// built-ins, the legacy per-harness keys (`claude_model`,
    /// `codex_enabled`, …) wherever the map stays silent on that field.
    /// The map wins where both speak; the Accounts tab writes the legacy
    /// keys for built-ins, so its edits apply without a migration. An
    /// account takes Claude's model and effort defaults, legacy keys
    /// included, until its own map entry names one. What `orion config
    /// harnesses` prints: labels as written, no email.
    pub fn raw_harness_registry(&self) -> Vec<HarnessDescriptor> {
        let mut all = orion_core::harness::registry(&self.harnesses, &self.claude_accounts);
        for entry in &mut all {
            if orion_core::harness::builtin(&entry.id).is_none() {
                continue;
            }
            let over = self.harnesses.get(&entry.id);
            let (legacy_enabled, legacy_model, legacy_effort) =
                self.legacy_harness_fields(&entry.id);
            if over.and_then(|o| o.enabled).is_none() {
                if let Some(enabled) = legacy_enabled {
                    entry.enabled = enabled;
                }
            }
            if over.and_then(|o| o.model_default.clone()).is_none() {
                if let Some(default) = legacy_model {
                    entry.model.default = default;
                }
            }
            if over.and_then(|o| o.effort_default.clone()).is_none() {
                if let Some(default) = legacy_effort {
                    entry.effort.default = default;
                }
            }
        }
        let claude = all
            .iter()
            .find(|entry| entry.id == AgentKind::Claude.as_str())
            .map(|claude| (claude.model.default.clone(), claude.effort.default.clone()));
        if let Some((model, effort)) = claude {
            for entry in &mut all {
                if !self.is_extra_account(&entry.id) {
                    continue;
                }
                let over = self.harnesses.get(&entry.id);
                if over.and_then(|o| o.model_default.as_ref()).is_none() {
                    entry.model.default = model.clone();
                }
                if over.and_then(|o| o.effort_default.as_ref()).is_none() {
                    entry.effort.default = effort.clone();
                }
            }
        }
        all
    }

    /// Whether `id` is a `claude_accounts` entry's — an account orion
    /// added, and so one it may remove.
    pub fn is_extra_account(&self, id: &str) -> bool {
        self.claude_accounts.iter().any(|a| a.id.trim() == id)
    }

    /// The legacy per-harness keys for a built-in id, as
    /// `(enabled, model, effort)` — `Some` only where the file differs
    /// from the default, i.e. where the user said something. Newer than
    /// the table, older than the `harnesses` map.
    fn legacy_harness_fields(&self, id: &str) -> (Option<bool>, Option<String>, Option<String>) {
        let (enabled, model, effort) = match id {
            "claude" => (
                &self.claude_enabled,
                &self.claude_model,
                &self.claude_effort,
            ),
            "codex" => (&self.codex_enabled, &self.codex_model, &self.codex_effort),
            "cursor" => (
                &self.cursor_enabled,
                &self.cursor_model,
                &self.cursor_effort,
            ),
            // OpenCode has no effort key: nothing to fall back to there.
            "opencode" => {
                return (
                    (!self.opencode_enabled).then_some(false),
                    non_default(&self.opencode_model),
                    None,
                )
            }
            _ => return (None, None, None),
        };
        (
            (!enabled).then_some(false),
            non_default(model),
            non_default(effort),
        )
    }

    /// The effective built-in descriptor `kind` reads as, regardless of
    /// whether the entry is enabled or valid (the picker hides broken
    /// rows; reads degrade gracefully).
    fn builtin_descriptor(&self, kind: AgentKind) -> HarnessDescriptor {
        let id = kind.as_str();
        self.harness_registry()
            .into_iter()
            .find(|entry| entry.id == id)
            .or_else(|| orion_core::harness::builtin(id))
            .expect("every built-in AgentKind describes")
    }

    /// The effective descriptor `(kind, custom)` reads as: the registry
    /// row, or a placeholder under its own name when the registry no
    /// longer names the id, so rows outliving their entry still render.
    pub fn effective_harness(&self, kind: AgentKind, custom: Option<&str>) -> HarnessDescriptor {
        let id = match kind {
            AgentKind::Custom => custom.unwrap_or_default().trim(),
            _ => kind.as_str(),
        };
        if let Some(descriptor) = self
            .harness_registry()
            .into_iter()
            .find(|entry| entry.id == id)
        {
            return descriptor;
        }
        orion_core::harness::placeholder(id)
    }

    /// The configured default model for new sessions of `kind`, as the
    /// daemon wants it: None = "default" = don't pass the flag.
    pub fn default_model(&self, kind: AgentKind) -> Option<String> {
        // Custom defaults resolve from the entry at the launch site,
        // where the id is known — never through this kind-only helper.
        if kind == AgentKind::Custom {
            return None;
        }
        self.builtin_descriptor(kind)
            .default_model()
            .map(str::to_string)
    }

    /// The configured default effort for new sessions of `kind`;
    /// None = "default" = don't pass the flag. A composing harness fits
    /// the effort against its configured family ([`fit_effort`]). An
    /// effort with no flag mapped is None whatever the file holds — spawn
    /// would drop it anyway.
    pub fn default_effort(&self, kind: AgentKind) -> Option<String> {
        if kind == AgentKind::Custom {
            return None;
        }
        let descriptor = self.builtin_descriptor(kind);
        if descriptor.effort.flag.is_none()
            && descriptor.effort.config_key.is_none()
            && !descriptor.compose_model_effort
        {
            return None;
        }
        let model = descriptor.default_model().map(str::to_string);
        let effort = descriptor.default_effort().map(str::to_string);
        fit_effort_in(&descriptor, model.as_deref(), effort)
    }

    /// Whether the NEW AGENT PICKER offers `kind` at all.
    pub fn kind_enabled(&self, kind: AgentKind) -> bool {
        if kind == AgentKind::Custom {
            // A bare Custom kind is never enabled: entries gate themselves.
            return false;
        }
        self.builtin_descriptor(kind).enabled
    }

    /// The AGENT KINDS the picker lists, in `AgentKind::ALL` order. Empty
    /// only from a hand-edited config: the overlay refuses to switch off
    /// the last one.
    pub fn enabled_kinds(&self) -> Vec<AgentKind> {
        AgentKind::ALL
            .into_iter()
            .filter(|kind| self.kind_enabled(*kind))
            .collect()
    }

    /// Every harness the picker and presets offer, in registry order:
    /// `(kind, None)` for built-ins, `(Custom, Some(id))` for customs.
    /// Disabled and broken entries are out (broken ones surface in the
    /// Accounts tab with their reason), and so is anything whose program is
    /// missing from PATH. The daemon stays authoritative at launch: it
    /// re-checks through the login shell, which can see shims PATH misses.
    ///
    /// A test that names no PATH ([`with_search_path`]) finds every program
    /// installed, so what a fixture's pickers offer never depends on the
    /// CLIs the machine running it happens to have.
    pub fn offered_harnesses(&self) -> Vec<(AgentKind, Option<String>)> {
        #[cfg(test)]
        if SEARCH_PATH.with(|p| p.borrow().is_none()) {
            return self.offered_harnesses_where(|_| true);
        }
        self.offered_harnesses_where(program_installed)
    }

    /// [`Config::offered_harnesses`] with the "is it installed" check
    /// passed in, so a test can name its own PATH instead of swapping the
    /// process-wide one under every other test's `git`.
    fn offered_harnesses_where(
        &self,
        installed: impl Fn(&str) -> bool,
    ) -> Vec<(AgentKind, Option<String>)> {
        let all = self.harness_registry();
        orion_core::harness::usable(&all)
            .into_iter()
            .filter(|entry| installed(&entry.program))
            .map(|entry| match AgentKind::parse(&entry.id) {
                Some(kind) => (kind, None),
                None => (AgentKind::Custom, Some(entry.id.clone())),
            })
            .collect()
    }

    /// Whether a preset may launch: its harness's Accounts tab switch,
    /// enabled and valid.
    pub fn preset_harness_usable(&self, preset: &crate::agent_presets::AgentPreset) -> bool {
        let id = match preset.kind {
            AgentKind::Custom => preset.custom_harness.clone().unwrap_or_default(),
            _ => preset.kind.as_str().to_string(),
        };
        self.harness_registry()
            .iter()
            .find(|entry| entry.id == id)
            .is_some_and(|entry| entry.enabled && entry.problem().is_none())
    }

    /// PRESET TEXT resolved: the side(s) of the task a new AGENT PRESET's
    /// text goes. `prefix` for a config that never set it or a hand edit
    /// off the list; `both` is taken for `prefix & postfix`.
    pub fn preset_text(&self) -> PresetText {
        PresetText::parse(&self.preset_text).unwrap_or(PresetText::DEFAULT)
    }

    /// The effective descriptor for a registry id, or a placeholder under
    /// its own name when the registry no longer names it, so rows
    /// outliving their entry still render.
    pub fn effective_harness_by_id(&self, id: &str) -> HarnessDescriptor {
        if let Some(descriptor) = self
            .harness_registry()
            .into_iter()
            .find(|entry| entry.id == id)
        {
            return descriptor;
        }
        orion_core::harness::placeholder(id)
    }

    /// Where `agent`'s Claude session can be carried on — **Continue on**
    /// another account: `(registry id, label)` for every other enabled
    /// Claude-dialect harness that resumes and keeps its sessions where
    /// orion can see them ([`orion_core::harness::continue_targets`]), in
    /// registry order. Empty for a session off Claude's dialect and an
    /// archived one — neither of which has anything to move. An
    /// account signed in as the session's own email is listed as what it
    /// is — `Claude (a@b.co) · same account`, whose limit is this one's.
    pub fn continue_targets(&self, agent: &orion_core::Agent) -> Vec<(String, String)> {
        if agent.archived {
            return Vec::new();
        }
        let from = self.effective_harness(agent.kind, agent.custom_harness.as_deref());
        if agent.kind != AgentKind::Claude && !from.claude_like() {
            return Vec::new();
        }
        let all = self.harness_registry();
        let email = crate::claude_accounts::email_of(&from);
        orion_core::harness::continue_targets(&all, &from.id)
            .into_iter()
            .map(|entry| {
                let mut label = entry.display_label().to_string();
                let same = crate::claude_accounts::email_of(entry)
                    .zip(email.as_deref())
                    .is_some_and(|(theirs, ours)| theirs.eq_ignore_ascii_case(ours));
                if same {
                    label.push_str(SAME_ACCOUNT);
                }
                (entry.id.clone(), label)
            })
            .collect()
    }

    /// `(id, field)` rows below the Accounts head, in registry order: every
    /// entry, enabled or not, valid or not (broken rows show their reason
    /// so they can be fixed); Effort only while the harness offers effort.
    pub fn agent_rows(&self) -> Vec<(String, HarnessField)> {
        let mut rows = Vec::new();
        for entry in self.harness_registry() {
            rows.push((entry.id.clone(), HarnessField::Enabled));
            rows.push((entry.id.clone(), HarnessField::Model));
            if entry.effort.offered {
                rows.push((entry.id.clone(), HarnessField::Effort));
            }
        }
        rows
    }

    /// The harness row at a tab-local Accounts index, or None while the
    /// index lands on the static head or the CLAUDE ACCOUNTS section.
    pub fn agent_row(&self, index: usize) -> Option<(String, HarnessField)> {
        self.agent_rows()
            .into_iter()
            .nth(index.checked_sub(self.agent_rows_from())?)
    }

    /// The tab-local Accounts index the first harness row sits at: under
    /// the head and the CLAUDE ACCOUNTS section.
    pub fn agent_rows_from(&self) -> usize {
        ACCOUNTS_HEAD.len() + self.account_rows().len()
    }

    /// The CLAUDE ACCOUNTS section's rows, below the Accounts head — and the
    /// Claude page of setup: every Claude account in registry order —
    /// the default one, built-in Claude, first; on or off — then **Add
    /// account**.
    pub fn account_rows(&self) -> Vec<AccountRow> {
        let mut rows: Vec<AccountRow> = self
            .raw_harness_registry()
            .into_iter()
            .filter(HarnessDescriptor::is_claude_account)
            .map(|entry| AccountRow::Account(entry.id))
            .collect();
        rows.push(AccountRow::Add);
        rows
    }

    /// The CLAUDE ACCOUNTS row at a tab-local Accounts index, or None off
    /// the section.
    pub fn account_row(&self, index: usize) -> Option<AccountRow> {
        self.account_rows()
            .into_iter()
            .nth(index.checked_sub(ACCOUNTS_HEAD.len())?)
    }

    /// The Accounts tab's section title for a harness: its label, and for a
    /// CLAUDE ACCOUNT other than the default one its config dir too, so
    /// two accounts signed in as one email still head two sections.
    pub fn section_title(&self, id: &str) -> String {
        let entry = self.effective_harness_by_id(id);
        let label = entry.display_label().to_string();
        match entry.pinned_claude_config_dir() {
            Some(dir) if entry.is_claude_account() && AgentKind::parse(id).is_none() => {
                format!("{label} · {}", crate::claude_accounts::tilde(&dir))
            }
            _ => label,
        }
    }

    /// What a CLAUDE ACCOUNTS row says beside its name: on or off, its
    /// config dir, and who it is signed in as — or which account it
    /// shares its email with. The **Add account** row says where a new
    /// one would go.
    pub fn account_value(&self, row: &AccountRow) -> String {
        match row {
            AccountRow::Add => self.next_account_dir(),
            AccountRow::Account(id) => match self.account_status(id) {
                Some((enabled, dir, state)) => {
                    let value = format!("{} · {dir} · {state}", on_off(enabled));
                    // The ACCOUNT TAG its rows wear on the grid, so the
                    // letters can be learned here.
                    match crate::claude_accounts::tag_of(id) {
                        Some(tag) => format!("{value} · tag {}", tag.letters.trim_end()),
                        None => value,
                    }
                }
                None => "n/a".into(),
            },
        }
    }

    /// [`Config::account_value`] without the switch, in two columns —
    /// the config dir and who it is signed in as — for the onboarding
    /// page, which lists the switches a page before. **Add account** has
    /// where a new one would go, and nothing to be signed in as.
    pub fn account_parts(&self, row: &AccountRow) -> (String, String) {
        match row {
            AccountRow::Add => (self.next_account_dir(), String::new()),
            AccountRow::Account(id) => match self.account_status(id) {
                Some((_, dir, state)) => (dir, state),
                None => ("n/a".into(), String::new()),
            },
        }
    }

    /// Where **Add account** would put a new account, unnamed.
    fn next_account_dir(&self) -> String {
        crate::claude_accounts::plan_new(self, "")
            .map(|new| crate::claude_accounts::tilde(&new.dir))
            .unwrap_or_else(|_| "n/a".into())
    }

    /// Account `id`'s switch, config dir and sign-in as the rows say them:
    /// `signed in`, `not signed in`, or `same as ~/.claude` when another
    /// account is signed in as its email.
    fn account_status(&self, id: &str) -> Option<(bool, String, String)> {
        let all = self.harness_registry();
        let entry = all.iter().find(|entry| entry.id == id)?;
        let dir = crate::claude_accounts::dir_of(entry)
            .map_or_else(|| "?".into(), |d| crate::claude_accounts::tilde(&d));
        let record = crate::claude_accounts::record_of(entry);
        let state = match record.as_ref().and_then(crate::claude_accounts::state_of) {
            None => "checking…".to_string(),
            Some(crate::claude_accounts::SignIn::Out) => "not signed in".to_string(),
            Some(crate::claude_accounts::SignIn::As(email)) => {
                let twin = all.iter().find(|other| {
                    other.id != entry.id
                        && crate::claude_accounts::email_of(other)
                            .is_some_and(|e| e.eq_ignore_ascii_case(&email))
                });
                match twin.and_then(crate::claude_accounts::dir_of) {
                    Some(other) => format!("same as {}", crate::claude_accounts::tilde(&other)),
                    None => "signed in".to_string(),
                }
            }
        };
        Some((entry.enabled, dir, state))
    }

    /// The hint under a CLAUDE ACCOUNTS row: which account it is and where
    /// it lives — the keys line under it says what its keys do.
    pub fn account_hint(&self, row: &AccountRow) -> String {
        let id = match row {
            AccountRow::Add => {
                let from = crate::claude_accounts::default_dir(self)
                    .map_or_else(|| "~/.claude".into(), |d| crate::claude_accounts::tilde(&d));
                return format!(
                    "A new config dir with its own login; then asks whether to share {from}'s \
                     CLAUDE.md, settings, skills…"
                );
            }
            AccountRow::Account(id) => id,
        };
        let entry = self.effective_harness_by_id(id);
        let dir = crate::claude_accounts::dir_of(&entry)
            .map_or_else(|| "?".into(), |d| crate::claude_accounts::tilde(&d));
        if AgentKind::parse(id) == Some(AgentKind::Claude) {
            format!(
                "The default account, in Claude Code's own {dir} — sign in again to switch logins"
            )
        } else if self.is_extra_account(id) {
            format!("{id}, in {dir} — an email typed at sign-in fills Claude's login page")
        } else {
            format!("config.json's harnesses entry {id}, in {dir} — edit the file to remove it")
        }
    }

    /// The warning lines under the CLAUDE ACCOUNTS section: a
    /// `claude_accounts` entry the registry left out, and why — a hand
    /// edit's — then one block per email two or more accounts are signed
    /// in as.
    pub fn account_notes(&self) -> Vec<String> {
        let all = self.harness_registry();
        let left_out = self.claude_accounts.iter().filter_map(|account| {
            let kept = all.iter().any(|entry| {
                entry.id == account.id.trim()
                    && entry.pinned_claude_config_dir() == Some(account.dir())
            });
            (!kept).then(|| {
                let why = account.problem().unwrap_or_else(|| {
                    format!("Claude account id `{}` is taken", account.id.trim())
                });
                format!("⚠ claude_accounts: {why} — left out")
            })
        });
        let same = crate::claude_accounts::same_account_groups(&all)
            .into_iter()
            .flat_map(|(email, dirs)| crate::claude_accounts::same_account_warning(&email, &dirs));
        left_out.chain(same).collect()
    }

    /// The value an Accounts harness row shows.
    pub fn agent_value(&self, id: &str, field: HarnessField) -> String {
        let descriptor = self.effective_harness_by_id(id);
        match field {
            HarnessField::Enabled => on_off(descriptor.enabled).into(),
            HarnessField::Model => {
                let raw = descriptor.model.default.clone();
                crate::config::model_row_label(&raw, descriptor.model.catalog)
            }
            HarnessField::Effort => {
                let choices = effort_choices_in(
                    &descriptor,
                    descriptor.default_model().map(str::to_string).as_deref(),
                );
                if choices.is_empty() {
                    "n/a".into()
                } else {
                    descriptor.effort.default.clone()
                }
            }
        }
    }

    /// The hint an Accounts harness row shows: what the row edits, with the
    /// entry's problem appended while it is broken.
    pub fn agent_hint(&self, id: &str, field: HarnessField) -> String {
        let descriptor = self.effective_harness_by_id(id);
        let label = descriptor.display_label();
        let mut hint = match field {
            HarnessField::Enabled => format!(
                "Offer {label} when you start an agent (off hides it; its running agents keep running)"
            ),
            HarnessField::Model => match descriptor.model.catalog {
                Some(orion_core::harness::HarnessCatalog::Claude) => format!(
                    "Default {label} model family (opus · latest, sonnet, haiku); full ids only if Claude Code's availableModels or config.json claude_models lists them"
                ),
                Some(orion_core::harness::HarnessCatalog::Cursor) => format!(
                    "Default model family for new {label} sessions; rows follow cursor-agent --list-models"
                ),
                None => format!("Default model for new {label} sessions (default = CLI's pick)"),
            },
            HarnessField::Effort => {
                if descriptor.compose_model_effort {
                    format!(
                        "Effort (and -fast) variant of the chosen {label} model; n/a while it is default or auto"
                    )
                } else if let Some(flag) = descriptor.effort.flag.as_deref() {
                    format!("Default reasoning effort ({flag}) for new {label} sessions")
                } else if let (Some(flag), Some(key)) = (
                    descriptor.effort.config_flag.as_deref(),
                    descriptor.effort.config_key.as_deref(),
                ) {
                    format!("Default reasoning effort ({flag} {key}=) for new {label} sessions")
                } else {
                    format!("Reserved until the {label} CLI documents a reasoning flag (default = unset)")
                }
            }
        };
        if let Some(problem) = descriptor.problem() {
            hint.push_str(&format!(" — broken: {problem}"));
        }
        hint
    }

    /// The hint for a tab-local Accounts index: the head row's own hint, or
    /// the harness row's.
    pub fn agent_hint_by_index(&self, index: usize) -> String {
        if let Some(spec) = ACCOUNTS_HEAD.get(index) {
            return spec.hint.to_string();
        }
        if let Some(row) = self.account_row(index) {
            return self.account_hint(&row);
        }
        match self.agent_row(index) {
            Some((id, field)) => self.agent_hint(&id, field),
            None => String::new(),
        }
    }

    /// Cycle an Accounts harness row: toggle Enabled, step Model / Effort
    /// through the rows the pickers offer. A composing harness refits its
    /// effort against the new model, so the row never holds an id the CLI
    /// would refuse.
    pub fn cycle_agent_row(&mut self, id: &str, field: HarnessField, delta: i32) {
        let Some(descriptor) = self
            .harness_registry()
            .into_iter()
            .find(|entry| entry.id == id)
        else {
            return;
        };
        let step = if delta == 0 { 1 } else { delta };
        match field {
            HarnessField::Enabled => self.set_harness_enabled(id, !descriptor.enabled),
            HarnessField::Model => {
                let choices = model_choices_in(&descriptor);
                let next = cycle_owned(&descriptor.model.default, &choices, step);
                self.set_harness_model(id, next.clone());
                let descriptor = self.effective_harness_by_id(id);
                if descriptor.compose_model_effort {
                    let choices = effort_choices_in(&descriptor, Some(&next));
                    if !fits(&descriptor.effort.default, &choices) {
                        let fitted = fit_effort_in(&descriptor, Some(&next), None)
                            .unwrap_or_else(|| DEFAULT_CHOICE.into());
                        self.set_harness_effort(id, fitted);
                    }
                }
            }
            HarnessField::Effort => {
                let choices = effort_choices_in(
                    &descriptor,
                    descriptor.default_model().map(str::to_string).as_deref(),
                );
                if choices.is_empty() {
                    return;
                }
                let next = cycle_owned(&descriptor.effort.default, &choices, step);
                self.set_harness_effort(id, next);
            }
        }
    }

    /// The Accounts tab's Effort row for harness `id`, set to `effort` —
    /// what Cycle effort writes as it steps the new-agent box, so the next
    /// box starts where this one was left. Only an effort the harness's
    /// default model offers (a Cursor box on another family steps itself
    /// alone); false when nothing changed.
    pub fn set_agent_effort(&mut self, id: &str, effort: &str) -> bool {
        let descriptor = self.effective_harness_by_id(id);
        let choices = effort_choices_in(&descriptor, descriptor.default_model());
        if !fits(effort, &choices) || descriptor.effort.default.eq_ignore_ascii_case(effort) {
            return false;
        }
        self.set_harness_effort(id, effort.to_string());
        true
    }

    /// Write an Enabled toggle: the `harnesses` map where it speaks, else
    /// the legacy layer (the built-in switch, the account's own).
    fn set_harness_enabled(&mut self, id: &str, enabled: bool) {
        if self.harnesses.get(id).and_then(|o| o.enabled).is_some() {
            self.harness_override_mut(id).enabled = Some(enabled);
            return;
        }
        if orion_core::harness::builtin(id).is_some() {
            self.set_legacy_enabled(id, enabled);
            return;
        }
        if let Some(account) = self
            .claude_accounts
            .iter_mut()
            .find(|account| account.id.trim() == id)
        {
            account.enabled = enabled;
            return;
        }
        self.harness_override_mut(id).enabled = Some(enabled);
    }

    /// Write a Model default: the map where it speaks, else the legacy
    /// built-in model key.
    fn set_harness_model(&mut self, id: &str, model: String) {
        if self
            .harnesses
            .get(id)
            .and_then(|o| o.model_default.clone())
            .is_some()
        {
            self.harness_override_mut(id).model_default = Some(model);
            return;
        }
        if orion_core::harness::builtin(id).is_some() {
            self.set_legacy_model(id, model);
            return;
        }
        self.harness_override_mut(id).model_default = Some(model);
    }

    /// Write an Effort default: the map where it speaks, else the legacy
    /// built-in effort key.
    fn set_harness_effort(&mut self, id: &str, effort: String) {
        if orion_core::harness::builtin(id).is_some()
            && self
                .harnesses
                .get(id)
                .and_then(|o| o.effort_default.clone())
                .is_none()
        {
            self.set_legacy_effort(id, effort);
            return;
        }
        self.harness_override_mut(id).effort_default = Some(effort);
    }

    /// The `harnesses` map entry for `id`, created when absent.
    fn harness_override_mut(&mut self, id: &str) -> &mut orion_core::harness::HarnessOverride {
        self.harnesses.entry(id.to_string()).or_default()
    }

    fn set_legacy_enabled(&mut self, id: &str, enabled: bool) {
        match id {
            "claude" => self.claude_enabled = enabled,
            "codex" => self.codex_enabled = enabled,
            "cursor" => self.cursor_enabled = enabled,
            "opencode" => self.opencode_enabled = enabled,
            _ => self.harness_override_mut(id).enabled = Some(enabled),
        }
    }

    fn set_legacy_model(&mut self, id: &str, model: String) {
        match id {
            "claude" => self.claude_model = model,
            "codex" => self.codex_model = model,
            "cursor" => self.cursor_model = model,
            "opencode" => self.opencode_model = model,
            _ => self.harness_override_mut(id).model_default = Some(model),
        }
    }

    fn set_legacy_effort(&mut self, id: &str, effort: String) {
        match id {
            "claude" => self.claude_effort = effort,
            "codex" => self.codex_effort = effort,
            "cursor" => self.cursor_effort = effort,
            _ => self.harness_override_mut(id).effort_default = Some(effort),
        }
    }

    /// The AGENT KIND the QUICK PROMPT launches: the `quick_prompt_kind`
    /// setting, stepped on to the first enabled kind when that harness has
    /// been switched off on the AGENTS TAB since it was chosen (an
    /// unreadable name, or a hand-edited config with every harness off,
    /// reads as Claude — the same default the picker starts from).
    pub fn quick_prompt_kind(&self) -> AgentKind {
        let configured = AgentKind::parse(&self.quick_prompt_kind).unwrap_or_default();
        if self.kind_enabled(configured) {
            return configured;
        }
        self.enabled_kinds().first().copied().unwrap_or(configured)
    }

    /// The harness the QUICK PROMPT launches, as a picker row names one:
    /// `(kind, None)` for a built-in ([`Config::quick_prompt_kind`]), and
    /// `(Custom, Some(id))` while `quick_prompt_kind` names another CLAUDE
    /// ACCOUNT that is on and launches — so `⌘N` can start on any
    /// account. One switched off or gone since steps aside as a built-in
    /// does, and an older build reading the id falls back to Claude.
    pub fn quick_prompt_harness(&self) -> (AgentKind, Option<String>) {
        let name = self.quick_prompt_kind.trim();
        if AgentKind::parse(name).is_none()
            && self.raw_harness_registry().iter().any(|entry| {
                entry.id == name
                    && entry.enabled
                    && entry.problem().is_none()
                    && entry.is_claude_account()
            })
        {
            return (AgentKind::Custom, Some(name.to_string()));
        }
        (self.quick_prompt_kind(), None)
    }

    /// What the Accounts tab's **New agents on** row cycles through: every built-in
    /// harness by the name the file stores, with the other CLAUDE
    /// ACCOUNTS — by registry id — right after `claude`.
    pub fn quick_prompt_choices(&self) -> Vec<String> {
        let accounts: Vec<String> = self
            .raw_harness_registry()
            .into_iter()
            .filter(|entry| entry.is_claude_account() && AgentKind::parse(&entry.id).is_none())
            .map(|entry| entry.id)
            .collect();
        let mut out = Vec::new();
        for name in agent_kind_names() {
            let claude = name == AgentKind::Claude.as_str();
            out.push(name);
            if claude {
                out.extend(accounts.iter().cloned());
            }
        }
        out
    }

    /// **Auto-fix my PRs**, read: anything but `off` is on.
    pub fn autofix_mode(&self) -> crate::autofix::Mode {
        crate::autofix::Mode::parse(&self.pr_autofix)
    }

    /// What the AUTOFIX agent launches on when no session on the branch
    /// has run here to copy: the default agent
    /// ([`Config::quick_prompt_harness`]) at `preset`'s model and effort —
    /// the **PR autofix** SYSTEM PRESET's — each falling back to that
    /// harness's own default when unset or not offered there (the default
    /// agent changed since it was picked), and the effort fitted to the
    /// model, as every launch surface does. `(kind, custom, model, effort)`.
    pub fn autofix_launch(
        &self,
        preset: &AgentPreset,
    ) -> (AgentKind, Option<String>, Option<String>, Option<String>) {
        let (kind, custom) = self.quick_prompt_harness();
        let descriptor = self.effective_harness(kind, custom.as_deref());
        let model = picked(
            preset.model.as_deref().unwrap_or_default(),
            &model_choices_in(&descriptor),
        )
        .or_else(|| descriptor.default_model().map(str::to_string));
        let effort = picked(
            preset.effort.as_deref().unwrap_or_default(),
            &effort_choices_in(&descriptor, model.as_deref()),
        )
        .or_else(|| descriptor.default_effort().map(str::to_string));
        let effort = fit_effort_in(&descriptor, model.as_deref(), effort);
        (kind, custom, model, effort)
    }

    /// The Claude account the WEEK IN REVIEW is written through: `pinned`
    /// — the **Week review** SYSTEM PRESET's harness id — when that is a
    /// Claude account that is on, else the default agent's when that is
    /// one ([`Config::quick_prompt_harness`]), else the first Claude
    /// account that is on. None with no Claude account on at all.
    pub fn review_account(&self, pinned: &str) -> Option<HarnessDescriptor> {
        let all = self.harness_registry();
        let on = claude_accounts_on(&all);
        let (kind, custom) = self.quick_prompt_harness();
        let default_agent = match kind {
            AgentKind::Custom => custom.unwrap_or_default(),
            other => other.as_str().to_string(),
        };
        let chosen = [pinned.trim(), default_agent.as_str()]
            .into_iter()
            .find_map(|id| on.iter().find(|entry| entry.id == id))
            .or(on.first())
            .map(|entry| (*entry).clone());
        chosen
    }

    /// Carry what an older release kept in this file for the agents orion
    /// starts itself onto their SYSTEM PRESETS in `presets`, which
    /// [`crate::agent_presets::ensure_system`] has seeded: the autofix
    /// agent's fallback model and effort onto **PR autofix**; the week
    /// review's account, model and effort onto **Week review** — the
    /// account as it resolves now, so the review goes on being written
    /// where it was. A Week review preset `new_review` — just added — is
    /// put on that account too, with no old key to say otherwise. The old
    /// keys are spent, and the next save drops them. Whether any were
    /// there to move.
    pub fn move_into_presets(&mut self, presets: &mut [AgentPreset], new_review: bool) -> bool {
        use crate::agent_presets::SystemPreset;
        let (model, effort) = (self.autofix_model.take(), self.autofix_effort.take());
        let autofix = model.is_some() || effort.is_some();
        if let Some(preset) = presets
            .iter_mut()
            .find(|p| autofix && p.system == Some(SystemPreset::PrAutofix))
        {
            // The preset's own model and effort meant nothing to autofix
            // before: the old keys' are what it ran at.
            if let Some(model) = model {
                preset.model = non_default(&model);
            }
            if let Some(effort) = effort {
                preset.effort = non_default(&effort);
            }
        }
        let (account, model, effort) = (
            self.review_account.take(),
            self.review_model.take(),
            self.review_effort.take(),
        );
        let review = account.is_some() || model.is_some() || effort.is_some();
        let through = (review || new_review)
            .then(|| self.review_account(account.as_deref().unwrap_or_default()))
            .flatten();
        if let Some(preset) = presets
            .iter_mut()
            .find(|p| (review || new_review) && p.system == Some(SystemPreset::WeekReview))
        {
            if let Some(entry) = through {
                (preset.kind, preset.custom_harness) = match AgentKind::parse(&entry.id) {
                    Some(kind) => (kind, None),
                    None => (AgentKind::Custom, Some(entry.id)),
                };
            }
            if let Some(model) = model.as_deref().and_then(non_default) {
                preset.model = Some(model);
            }
            if let Some(effort) = effort.as_deref().and_then(non_default) {
                preset.effort = Some(effort);
            }
        }
        autofix || review
    }

    /// Hotkeys as the event loop dispatches them: defaults with this
    /// config's overrides applied.
    pub fn keymap(&self) -> crate::keymap::Keymap {
        crate::keymap::Keymap::from_overrides(&self.keybindings)
    }

    /// The settings of the project checked out at `repo_path`: its
    /// `projects` entry, else the defaults every project without one gets.
    pub fn project(&self, repo_path: &Path) -> ProjectSettings {
        self.projects.get(repo_path).cloned().unwrap_or_default()
    }

    /// Store `settings` as the entry of the project at `repo_path`. An
    /// entry that says nothing the defaults don't is dropped rather than
    /// written, so `projects` names only the projects set up differently
    /// — and a project turned back to match the rest leaves no trace.
    pub fn set_project(&mut self, repo_path: &Path, settings: ProjectSettings) {
        if settings == ProjectSettings::default() {
            self.projects.remove(repo_path);
        } else {
            self.projects.insert(repo_path.to_path_buf(), settings);
        }
    }

    /// The stored text of a typed PROJECTS TAB row for the project at
    /// `repo_path`.
    pub fn project_text_value(&self, repo_path: &Path, kind: SettingKind) -> String {
        self.project(repo_path).text_value(kind)
    }

    /// Write a typed PROJECTS TAB row for the project at `repo_path`.
    /// False for a row that is not a typed project row.
    pub fn set_project_text(&mut self, repo_path: &Path, kind: SettingKind, value: &str) -> bool {
        self.edit_project(repo_path, |settings| settings.set_text(kind, value))
    }

    /// Step a cycled PROJECTS TAB row for the project at `repo_path` —
    /// what [`Config::cycle_kind`] is for a top-level row. False for a row
    /// that is not one.
    pub fn cycle_project(&mut self, repo_path: &Path, kind: SettingKind, delta: i32) -> bool {
        self.edit_project(repo_path, |settings| settings.cycle(kind, delta))
    }

    /// Apply `edit` to the entry of the project at `repo_path` and store
    /// it, when `edit` says it applied.
    fn edit_project(
        &mut self,
        repo_path: &Path,
        edit: impl FnOnce(&mut ProjectSettings) -> bool,
    ) -> bool {
        let mut settings = self.project(repo_path);
        if !edit(&mut settings) {
            return false;
        }
        self.set_project(repo_path, settings);
        true
    }

    /// Hand each project the settings an older release kept once for all
    /// of them — `worktree_base_branch`, `worktree_containers` and
    /// `pr_merge_method` — so a project that has no value of its own goes
    /// on starting worktrees where it did, deleting them does what it did,
    /// and its merge form opens on the method it did. `repos` is every
    /// project the tree holds; the ones `projects` names are taken too.
    /// The old keys are spent after, and the next save drops them from
    /// the file. They wait while the tree names no project — an empty
    /// snapshot says nothing about which there are — and while `projects`
    /// is a map this build can't read, which a save would write over.
    /// Whether anything moved.
    pub fn adopt_project_keys(&mut self, repos: &[PathBuf]) -> bool {
        if repos.is_empty() || self.skipped.contains("projects") {
            return false;
        }
        let mut every: BTreeSet<PathBuf> = self.projects.keys().cloned().collect();
        every.extend(repos.iter().cloned());
        let base = self.worktree_base_branch.take();
        let containers = self.worktree_containers.take();
        let merge = self.pr_merge_method.take();
        if base.is_none() && containers.is_none() && merge.is_none() {
            return false;
        }
        let base = base.unwrap_or_default();
        let base = base.trim();
        let containers = containers
            .map(|word| WorktreeContainers::parse(&word))
            .filter(|policy| *policy != WorktreeContainers::Off);
        let merge = merge.map(|word| crate::pr_actions::MergeMethod::parse(&word));
        for repo in every {
            let mut settings = self.project(&repo);
            if settings.base_branch.is_empty() {
                settings.base_branch = base.to_string();
            }
            if let Some(policy) = containers.filter(|_| settings.worktree_containers.is_empty()) {
                settings.worktree_containers = policy.as_str().to_string();
            }
            if let Some(method) = merge.filter(|_| settings.merge_method.is_empty()) {
                settings.set_merge_method(method);
            }
            self.set_project(&repo, settings);
        }
        true
    }

    /// The method the merge form of the project at `repo_path` opens on:
    /// the one its last merge took, squash before its first.
    pub fn merge_method(&self, repo_path: &Path) -> crate::pr_actions::MergeMethod {
        crate::pr_actions::MergeMethod::parse(&self.project(repo_path).merge_method)
    }

    /// Remember `method` as the one the project at `repo_path` merges by.
    pub fn remember_merge_method(
        &mut self,
        repo_path: &Path,
        method: crate::pr_actions::MergeMethod,
    ) {
        self.edit_project(repo_path, |settings| {
            settings.set_merge_method(method);
            true
        });
    }

    /// The overlay's label for `kind`. A PROJECTS TAB row read here shows
    /// the fallback — the overlay reads the selected project's through
    /// [`Config::project`] instead.
    pub fn value_label(&self, kind: SettingKind) -> String {
        match kind {
            SettingKind::FollowNewSession => on_off(self.follow_new_session).into(),
            SettingKind::SessionIdleTimeout => self.session_idle_timeout.clone(),
            SettingKind::PrAutofix => {
                on_off(self.autofix_mode() == crate::autofix::Mode::Auto).into()
            }
            SettingKind::OutsideEditor => crate::outside_editor::row_label(&self.outside_editor),
            SettingKind::DoneSound => self.done_sound.clone(),
            SettingKind::FeedbackSound => self.feedback_sound.clone(),
            SettingKind::Theme => self.theme.clone(),
            SettingKind::Animations => on_off(self.animations).into(),
            SettingKind::Spotify => on_off(self.spotify).into(),
            // A CLAUDE ACCOUNT — the default one too — reads as the name
            // it goes by, with its email.
            SettingKind::QuickPromptKind => match self.quick_prompt_harness() {
                (AgentKind::Custom, Some(id)) => self
                    .effective_harness_by_id(&id)
                    .display_label()
                    .to_string(),
                (AgentKind::Claude, None)
                    if AgentKind::parse(&self.quick_prompt_kind) == Some(AgentKind::Claude) =>
                {
                    self.effective_harness_by_id(AgentKind::Claude.as_str())
                        .display_label()
                        .to_string()
                }
                _ => self.quick_prompt_kind.clone(),
            },
            // A project row with no project to speak of: what one without
            // an entry would show. The Linear key is the app's to tell
            // (`linear::key_status`), not the file's.
            SettingKind::BaseBranch
            | SettingKind::SetupCommand
            | SettingKind::RunCommand
            | SettingKind::OpenCommand
            | SettingKind::WorktreeContainers
            | SettingKind::LinearKey => ProjectSettings::default().value_label(kind),
            SettingKind::AgentPresets => "orion's and yours".into(),
        }
    }

    /// `delta == 0` means activate (toggle a bool, cycle a choice forward).
    /// Non-zero delta cycles a choice; bools still toggle. `index` is
    /// tab-local — the Keys tab has no cyclable values and no-ops here,
    /// and so does a PROJECTS TAB row, which is one project's
    /// ([`Config::set_project_text`], [`Config::cycle_project`]). The
    /// Accounts tab resolves its head rows statically and its harness
    /// rows through the registry.
    pub fn cycle(&mut self, tab: usize, index: usize, delta: i32) {
        if tab == accounts_tab() {
            if let Some(spec) = ACCOUNTS_HEAD.get(index) {
                self.cycle_kind(spec.kind, delta);
            } else if let Some(row) = self.account_row(index) {
                // An account row's ←/→ is its switch; its Enter signs it
                // in (`event_loop`), and Add has nothing to cycle.
                if let AccountRow::Account(id) = row {
                    self.cycle_agent_row(&id, HarnessField::Enabled, delta);
                }
            } else if let Some((id, field)) = self.agent_row(index) {
                self.cycle_agent_row(&id, field, delta);
            }
            return;
        }
        let Some(spec) = setting_at(tab, index) else {
            return;
        };
        self.cycle_kind(spec.kind, delta);
    }

    /// Cycle one static setting row. The Accounts tab's harness rows cycle
    /// through [`Config::cycle_agent_row`] instead.
    pub(crate) fn cycle_kind(&mut self, kind: SettingKind, delta: i32) {
        let step = if delta == 0 { 1 } else { delta };
        match kind {
            SettingKind::FollowNewSession => {
                self.follow_new_session = !self.follow_new_session;
            }
            SettingKind::SessionIdleTimeout => {
                self.session_idle_timeout =
                    cycle_choice(&self.session_idle_timeout, SESSION_IDLE_TIMEOUTS, step).into();
            }
            SettingKind::PrAutofix => {
                let on = self.autofix_mode() == crate::autofix::Mode::Auto;
                self.pr_autofix = AUTOFIX_MODES[usize::from(!on)].into();
            }
            SettingKind::OutsideEditor => {
                let current = crate::outside_editor::Choice::parse(&self.outside_editor);
                self.outside_editor =
                    cycle_choice(current.as_str(), crate::outside_editor::CHOICES, step).into();
                crate::outside_editor::forget_hint_name();
            }
            SettingKind::DoneSound => {
                self.done_sound = cycle_choice(&self.done_sound, SOUNDS, step).into();
            }
            SettingKind::FeedbackSound => {
                self.feedback_sound = cycle_choice(&self.feedback_sound, SOUNDS, step).into();
            }
            SettingKind::Theme => {
                self.theme = cycle_choice(&self.theme, crate::theme::THEMES, step).into();
            }
            SettingKind::Animations => {
                self.animations = !self.animations;
            }
            SettingKind::Spotify => {
                self.spotify = !self.spotify;
            }
            SettingKind::QuickPromptKind => {
                self.quick_prompt_kind =
                    cycle_owned(&self.quick_prompt_kind, &self.quick_prompt_choices(), step);
            }
            // One project's, not the file's: see `set_project_text` and
            // `cycle_project`. The presets row opens a list.
            SettingKind::BaseBranch
            | SettingKind::SetupCommand
            | SettingKind::RunCommand
            | SettingKind::OpenCommand
            | SettingKind::WorktreeContainers
            | SettingKind::LinearKey
            | SettingKind::AgentPresets => {}
        }
    }
}

/// What the TUI plays for a status edge — the `done_sound` or
/// `feedback_sound` SETTING resolved against where the TUI is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sound {
    /// The terminal BEL (`\x07`), written through the attached terminal,
    /// which decides whether that is a sound, a flash, or a dock bounce.
    Bell,
    /// A sound file to hand to `afplay`.
    File(PathBuf),
}

impl Config {
    /// The sound to play for an unseen finish, or `None` for silence —
    /// which also stands down the `<session> finished` desktop
    /// notification. A named system sound only resolves to its file on a
    /// local terminal, and when the file exists — over ssh `afplay` would
    /// ring the *remote* box, so the bell stands in there, as it does for a
    /// name the sound folder doesn't hold.
    pub fn done_sound(&self) -> Option<Sound> {
        resolve_sound(&self.done_sound, orion_core::host::is_remote_session())
    }

    /// The sound to play when a turn stops to ask the user or dies
    /// mid-turn, or `None` for silence — which also stands down their
    /// desktop notifications, since
    /// `feedback_sound` is the one switch for both. Same fallbacks as
    /// [`Config::done_sound`].
    pub fn feedback_sound(&self) -> Option<Sound> {
        resolve_sound(&self.feedback_sound, orion_core::host::is_remote_session())
    }
}

fn resolve_sound(configured: &str, remote: bool) -> Option<Sound> {
    let name = configured.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("off") {
        return None;
    }
    if name.eq_ignore_ascii_case("bell") || remote {
        return Some(Sound::Bell);
    }
    // A sound name is a bare file stem; anything else (a path, a dot) is
    // not one, and the bell covers the typo.
    if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(Sound::Bell);
    }
    let path = Path::new(MACOS_SOUNDS_DIR).join(format!("{name}.aiff"));
    if path.is_file() {
        Some(Sound::File(path))
    } else {
        Some(Sound::Bell)
    }
}

/// The **Base branch** of the project at `repo`, read fresh from the
/// file: empty for origin's default branch, and with no project to say.
pub fn base_branch_of(repo: Option<&Path>) -> String {
    repo.map(|repo| Config::load().project(repo).base_branch)
        .unwrap_or_default()
}

/// Whether `kind`'s CLI resolves on this process's PATH right now. A fast
/// synchronous check for picker filtering only; the daemon re-probes
/// through the login shell at launch, which can see shims PATH misses.
/// Whether `program` resolves on this process's PATH right now — the
/// fast check behind [`Config::offered_harnesses`], for built-ins and
/// customs alike.
pub fn program_installed(program: &str) -> bool {
    program_on(&search_path(), program)
}

/// The PATH programs are looked up on: this process's — or, in a test
/// that named one ([`with_search_path`]), that test's, so no test swaps
/// the process-wide PATH under every other test's `git`.
pub fn search_path() -> std::ffi::OsString {
    #[cfg(test)]
    if let Some(path) = SEARCH_PATH.with(|p| p.borrow().clone()) {
        return path;
    }
    std::env::var_os("PATH").unwrap_or_default()
}

#[cfg(test)]
thread_local! {
    static SEARCH_PATH: std::cell::RefCell<Option<std::ffi::OsString>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with [`search_path`] — and so [`program_installed`] — reading
/// `path` on this test's thread.
#[cfg(test)]
pub fn with_search_path<T>(path: std::ffi::OsString, f: impl FnOnce() -> T) -> T {
    SEARCH_PATH.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// Whether `program` is a file in one of the directories `paths` lists,
/// PATH-style.
fn program_on(paths: &std::ffi::OsStr, program: &str) -> bool {
    crate::install::which(paths, program).is_some()
}

/// [`DEFAULT_CHOICE`] (or blank) → None; anything else passes through.
pub(crate) fn non_default(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && !value.eq_ignore_ascii_case(DEFAULT_CHOICE)).then(|| value.to_string())
}

fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

pub(crate) fn cycle_choice<'a>(current: &str, choices: &[&'a str], delta: i32) -> &'a str {
    choices[cycled_index(current, choices, delta)]
}

/// The two settings layers merged, `local` over `path`. What this build
/// can't read is logged and remembered in [`Config::skipped`], so a save
/// leaves it as stored.
fn load_layers(path: &Path, local: &Path) -> Config {
    let loaded = orion_core::settings::load::<Config>(path, local);
    for problem in &loaded.problems {
        tracing::warn!("{problem}");
    }
    if !loaded.skipped.is_empty() {
        tracing::warn!(keys = ?loaded.skipped, "settings this build can't read keep their defaults");
    }
    let mut cfg = Config {
        skipped: loaded.skipped,
        ..loaded.value
    };
    cfg.migrate();
    cfg
}

impl Config {
    /// What an older release stored, in the words this one keeps, so the
    /// next save writes it once and nothing reads the old word after: a
    /// `pr_autofix` of `ask` — from when autofix asked before it sent —
    /// is `auto`.
    fn migrate(&mut self) {
        if self.pr_autofix.trim().eq_ignore_ascii_case("ask") {
            self.pr_autofix = crate::autofix::Mode::Auto.as_str().into();
        }
    }
}

/// Test shorthand: the settings at `path`, local layer beside it.
#[cfg(test)]
fn load_from(path: &Path) -> Config {
    load_layers(path, &sibling_local_path(path))
}

/// The object a settings file holds, to patch keys into: empty when the
/// file is missing or isn't a JSON object (the save replaces it, as it
/// always has), an error only when the file is there and can't be read.
fn patchable_object(path: &Path) -> std::io::Result<orion_core::settings::Object> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(match serde_json::from_str(&raw) {
            Ok(serde_json::Value::Object(obj)) => obj,
            _ => orion_core::settings::Object::new(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(err) => Err(err),
    }
}

fn invalid_data(err: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, err)
}

fn settings_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = CONFIG_PATH_OVERRIDE.with(|p| p.borrow().clone()) {
            return path;
        }
    }
    orion_core::paths::config_path()
}

/// The local layer. A test's path override moves it too, beside the
/// overriding file, so no test reads the dev's own.
fn local_settings_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = CONFIG_PATH_OVERRIDE.with(|p| p.borrow().clone()) {
            return sibling_local_path(&path);
        }
    }
    orion_core::paths::config_local_path()
}

#[cfg(test)]
thread_local! {
    static CONFIG_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    static SHIPPED_DEFAULTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with the defaults a release ships — Grok Build off — where a
/// test otherwise keeps every harness on.
#[cfg(test)]
pub fn with_shipped_defaults<T>(f: impl FnOnce() -> T) -> T {
    SHIPPED_DEFAULTS.with(|cell| {
        let prev = cell.replace(true);
        let out = f();
        cell.set(prev);
        out
    })
}

/// Whether this test thread pinned the config to a file of its own —
/// what a read of anything the config names (an account's sign-in) needs
/// first, so no test reads the developer's.
#[cfg(test)]
pub fn config_pinned() -> bool {
    CONFIG_PATH_OVERRIDE.with(|p| p.borrow().is_some())
}

#[cfg(test)]
pub fn with_config_path<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    CONFIG_PATH_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// Test-only accessors: nothing in the app reads these any more.
#[cfg(test)]
impl Config {
    /// [`Config::save`] into `path`, with its local layer beside it.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        self.write_layers(path, &sibling_local_path(path), false)
    }
}

#[cfg(test)]
/// `config.local.json` beside `path`: the local layer of a settings file
/// that is not this orion's own — a test's, or [`Config::save_to`]'s.
fn sibling_local_path(path: &Path) -> PathBuf {
    path.with_file_name("config.local.json")
}

#[cfg(test)]
/// Where a static setting lives, as `(tab, row)`. The overlay addresses
/// settings by position, so anything that wants to talk about one by name
/// — tests, and anything that ever jumps the cursor to a named setting —
/// goes through here rather than hardcoding an index. Harness rows locate
/// through [`locate_agent`].
pub fn locate(kind: SettingKind) -> Option<(usize, usize)> {
    SETTINGS_TABS.iter().enumerate().find_map(|(t, tab)| {
        match tab.body {
            TabBody::Values(settings) | TabBody::Project(settings) => {
                settings.iter().position(|s| s.kind == kind)
            }
            TabBody::Accounts => ACCOUNTS_HEAD.iter().position(|s| s.kind == kind),
            TabBody::Keys => None,
        }
        .map(|i| (t, i))
    })
}

#[cfg(test)]
/// Index of the Projects tab, whose rows are the selected project's.
pub fn project_tab() -> usize {
    SETTINGS_TABS
        .iter()
        .position(|t| matches!(t.body, TabBody::Project(_)))
        .expect("SETTINGS_TABS declares a Projects tab")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_civil_counts_from_the_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(
            days_from_civil(2026, 9, 24) - days_from_civil(2026, 8, 22),
            33
        );
    }
    use crate::keymap::Keymap;

    /// An older orion's `pr_autofix: "ask"` is `auto` once loaded, and a
    /// save writes `auto` back in its place.
    #[test]
    fn an_old_autofix_ask_is_rewritten_to_auto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"pr_autofix": "ask"}"#).unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.pr_autofix, "auto");
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["pr_autofix"], "auto");
    }

    /// The config file the newest release wrote, every key off its
    /// default; see [`the_newest_releases_config_still_loads_every_key`].
    /// Replace it when a release changes the keys it writes.
    const NEWEST_RELEASE_CONFIG: &str = include_str!("../../orion-core/fixtures/config-2.2.1.json");

    fn read_json_file(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Run `f` with the config pinned to an empty temp file, so every
    /// registry read (Accounts rows, choice lists, tab lengths) sees a
    /// fresh install — never the dev's own file.
    fn with_empty_config<T>(f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        with_config_path(path, f)
    }

    /// Where an Accounts harness row lives in `cfg`'s own rows, without
    /// loading anything: [`locate_agent`] reads the live file, which a
    /// test's in-memory config may have left behind.
    fn locate_in(cfg: &Config, id: &str, field: HarnessField) -> Option<(usize, usize)> {
        let tab = accounts_tab();
        cfg.agent_rows()
            .iter()
            .position(|(row_id, row_field)| row_id == id && *row_field == field)
            .map(|i| (tab, cfg.agent_rows_from() + i))
    }

    /// The compatibility rule in docs/configuration.md, against the file
    /// the newest release wrote: a key this build still has loads to
    /// exactly the value that release wrote — a type change would leave it
    /// unreadable, or read back as something else here — and a key this
    /// build has retired loads as nothing. A save writes every one of them
    /// back as it was, the retired ones untouched.
    #[test]
    fn the_newest_releases_config_still_loads_every_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, NEWEST_RELEASE_CONFIG).unwrap();
        let loaded = load_from(&path);
        assert!(
            loaded.skipped.is_empty(),
            "keys this build can't read: {:?}",
            loaded.skipped
        );
        let known = serde_json::to_value(&loaded).unwrap();
        let fixture: serde_json::Value = serde_json::from_str(NEWEST_RELEASE_CONFIG).unwrap();
        let fixture = fixture.as_object().unwrap();
        // A moved key is never written, so it is checked where it waits
        // for its migration instead.
        let moved = [
            ("worktree_base_branch", &loaded.worktree_base_branch),
            ("worktree_containers", &loaded.worktree_containers),
            ("pr_merge_method", &loaded.pr_merge_method),
            ("autofix_model", &loaded.autofix_model),
            ("autofix_effort", &loaded.autofix_effort),
            ("review_account", &loaded.review_account),
            ("review_model", &loaded.review_model),
            ("review_effort", &loaded.review_effort),
        ];
        assert_eq!(moved.len(), loaded.moved_keys().len(), "every moved key");
        let mut retired = 0;
        for (key, value) in fixture {
            if let Some((_, held)) = moved.iter().find(|(name, _)| name == key) {
                assert_eq!(held.as_deref(), value.as_str(), "`{key}` waits as written");
                continue;
            }
            match known.get(key) {
                Some(loaded) => assert_eq!(loaded, value, "`{key}` no longer loads as written"),
                None => retired += 1,
            }
        }
        assert!(retired > 0, "the release wrote keys this build has retired");
        loaded.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        for (key, value) in fixture {
            assert_eq!(saved.get(key), Some(value), "`{key}` after a save");
        }
    }

    /// A retired key — one an older orion wrote that this build no longer
    /// has, at the top level or inside a project's entry — loads as
    /// nothing and costs nothing, and a save writes it back where it was
    /// for the older builds that still read it. An entry that only carries
    /// such a key is kept for it, not dropped as all-default.
    #[test]
    fn a_retired_key_loads_as_nothing_and_survives_a_save() {
        let other = Path::new("/tmp/other");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
              "confirm_on_archive": true,
              "show_all_worktrees": false,
              "projects": {
                "/tmp/other": { "hide_root_worktree": true, "future_row": "x" }
              }
            }"#,
        )
        .unwrap();
        let mut cfg = load_from(&path);
        assert!(cfg.skipped.is_empty(), "{:?}", cfg.skipped);
        assert_eq!(
            cfg.project(other).other.get("hide_root_worktree"),
            Some(&serde_json::json!(true)),
            "an entry's key rides along unread"
        );

        cfg.spotify = false;
        let kept = cfg.project(other);
        cfg.set_project(other, kept);
        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        assert_eq!(saved["confirm_on_archive"], true);
        assert_eq!(saved["show_all_worktrees"], false);
        assert_eq!(
            saved["projects"],
            serde_json::json!({
                "/tmp/other": { "hide_root_worktree": true, "future_row": "x" }
            })
        );
    }

    /// A value this build can't read — a newer orion's, most likely — costs
    /// only its own key, and a save leaves it as stored until the setting is
    /// changed here.
    #[test]
    fn an_unreadable_key_keeps_the_rest_and_outlives_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"theme": "ocean", "spotify": "auto", "animations": false}"#,
        )
        .unwrap();
        let mut cfg = load_from(&path);
        assert_eq!(cfg.theme, "ocean");
        assert!(!cfg.animations);
        assert!(cfg.spotify);
        assert_eq!(cfg.skipped, BTreeSet::from(["spotify".to_string()]));

        cfg.done_sound = "Ping".into();
        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        assert_eq!(saved["spotify"], "auto", "left as stored");
        assert_eq!(saved["done_sound"], "Ping");
        assert_eq!(saved["theme"], "ocean");

        let (t, r) = locate(SettingKind::Spotify).unwrap();
        cfg.cycle(t, r, 1);
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["spotify"],
            false,
            "changing it here is a real edit"
        );
    }

    /// `config.local.json` wins key by key, and a key it holds is saved back
    /// into it — never copied into the portable `config.json`.
    #[test]
    fn the_local_layer_overrides_and_keeps_its_own_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let local = dir.path().join("config.local.json");
        std::fs::write(&path, r#"{"done_sound": "Ping", "theme": "ocean"}"#).unwrap();
        std::fs::write(&local, r#"{"done_sound": "Pop"}"#).unwrap();
        let mut cfg = load_from(&path);
        assert_eq!(cfg.done_sound, "Pop");
        assert_eq!(cfg.theme, "ocean");

        cfg.theme = "forest".into();
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["theme"], "forest");
        assert_eq!(
            read_json_file(&path)["done_sound"],
            "Ping",
            "the local value stays local"
        );
        assert_eq!(
            read_json_file(&local),
            serde_json::json!({"done_sound": "Pop"})
        );

        cfg.done_sound = "Hero".into();
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&local)["done_sound"], "Hero");
        assert_eq!(read_json_file(&path)["done_sound"], "Ping");
    }

    #[test]
    fn reset_removes_the_local_layer_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let local = dir.path().join("config.local.json");
        with_config_path(path.clone(), || {
            std::fs::write(&local, r#"{"theme": "rose"}"#).unwrap();
            assert_eq!(Config::load().theme, "rose");
            Config::reset_to_defaults().unwrap();
            assert!(!local.exists());
            assert_eq!(Config::load().theme, Config::default().theme);
        });
    }

    #[test]
    fn reset_rewrites_the_file_from_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        with_config_path(path.clone(), || {
            let mut cfg = Config {
                theme: "midnight".into(),
                animations: false,
                ..Config::default()
            };
            cfg.keybindings.insert("git_diff".into(), "f9".into());
            cfg.save().unwrap();
            // A key the overlay doesn't own survives an ordinary save…
            let mut root: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            root["hand_added_key"] = serde_json::json!(false);
            std::fs::write(&path, serde_json::to_vec_pretty(&root).unwrap()).unwrap();
            Config::load().save().unwrap();
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(
                raw.contains("hand_added_key"),
                "save() patches, keeping foreign keys:\n{raw}"
            );

            // …but not a reset: the file starts over from an empty object.
            let reset = Config::reset_to_defaults().unwrap();
            assert!(reset.animations);
            assert!(reset.keybindings.is_empty());
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(
                !raw.contains("hand_added_key"),
                "foreign key survived:\n{raw}"
            );
            let loaded = Config::load();
            assert_eq!(loaded.theme, Config::default().theme);
            assert!(loaded.animations);
            assert!(loaded.keybindings.is_empty());
        });
    }

    /// A project's **Base branch** is a typed row of its own on the
    /// PROJECTS TAB, `auto` until typed, kept in that project's entry and
    /// nowhere else.
    #[test]
    fn base_branch_is_a_typed_project_row_that_defaults_to_auto() {
        let demo = Path::new("/tmp/demo");
        let other = Path::new("/tmp/other");
        let mut cfg = Config::default();
        assert_eq!(
            cfg.project(demo).value_label(SettingKind::BaseBranch),
            AUTO_CHOICE
        );
        assert!(SettingKind::BaseBranch.is_text() && SettingKind::BaseBranch.is_project());
        let (tab, _) = locate(SettingKind::BaseBranch).unwrap();
        assert_eq!(tab, project_tab(), "sits on the Projects tab");
        assert_eq!(
            spec_for(SettingKind::BaseBranch).map(|s| s.label),
            Some("Base branch")
        );

        assert!(cfg.set_project_text(demo, SettingKind::BaseBranch, "  dev "));
        assert_eq!(cfg.project(demo).base_branch, "dev", "trimmed");
        assert_eq!(cfg.project_text_value(demo, SettingKind::BaseBranch), "dev");
        assert_eq!(cfg.project(other).base_branch, "", "another project's own");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        assert_eq!(saved["projects"]["/tmp/demo"]["base_branch"], "dev");
        assert!(saved.get("worktree_base_branch").is_none());
        assert_eq!(load_from(&path).project(demo).base_branch, "dev");

        // Empty is the way back to auto, and leaves no entry behind.
        assert!(cfg.set_project_text(demo, SettingKind::BaseBranch, "   "));
        cfg.save_to(&path).unwrap();
        assert_eq!(read_json_file(&path)["projects"], serde_json::json!({}));
    }

    /// The global `worktree_base_branch` and `worktree_containers` an
    /// older release kept move into every project once the tree is known —
    /// a project's own value wins — and the save after drops both keys.
    /// Until then a save leaves them in the file for the move to read.
    #[test]
    fn the_old_global_base_branch_and_containers_move_into_every_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
              "worktree_base_branch": "dev",
              "worktree_containers": "remove+volumes",
              "projects": {
                "/src/riplo-os": { "run_command": "just start" },
                "/src/pinned": { "base_branch": "main", "worktree_containers": "stop" }
              }
            }"#,
        )
        .unwrap();
        let mut cfg = load_from(&path);
        assert!(cfg.skipped.is_empty(), "{:?}", cfg.skipped);

        // A save before the tree is known keeps both for later.
        cfg.theme = "ocean".into();
        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        assert_eq!(saved["worktree_base_branch"], "dev");
        assert_eq!(saved["worktree_containers"], "remove+volumes");

        let mut cfg = load_from(&path);
        let tree = [PathBuf::from("/src/riplo-os"), PathBuf::from("/src/new")];
        assert!(cfg.adopt_project_keys(&tree));
        let riplo = cfg.project(Path::new("/src/riplo-os"));
        assert_eq!(riplo.base_branch, "dev");
        assert_eq!(riplo.containers(), WorktreeContainers::RemoveVolumes);
        assert_eq!(
            riplo.run_command, "just start",
            "the rest of the entry kept"
        );
        assert_eq!(cfg.project(Path::new("/src/new")).base_branch, "dev");
        let pinned = cfg.project(Path::new("/src/pinned"));
        assert_eq!(pinned.base_branch, "main", "a project's own value wins");
        assert_eq!(pinned.containers(), WorktreeContainers::Stop);
        assert!(!cfg.adopt_project_keys(&tree), "once");

        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        assert!(saved.get("worktree_base_branch").is_none(), "{saved}");
        assert!(saved.get("worktree_containers").is_none(), "{saved}");
        assert_eq!(saved["projects"]["/src/new"]["base_branch"], "dev");
        assert_eq!(
            saved["projects"]["/src/riplo-os"]["worktree_containers"],
            "remove+volumes"
        );
        let mut again = load_from(&path);
        assert!(!again.adopt_project_keys(&tree), "nothing left to move");
        assert_eq!(again.project(Path::new("/src/new")).base_branch, "dev");
    }

    /// The old keys wait for a tree that names a project — an empty
    /// snapshot spends nothing, even with `projects` naming some — and for
    /// a `projects` map this build can read.
    #[test]
    fn the_old_global_keys_wait_for_a_project() {
        let mut cfg: Config = serde_json::from_str(
            r#"{"worktree_base_branch": "dev", "projects": {"/src/old": {"run_command": "x"}}}"#,
        )
        .unwrap();
        assert!(!cfg.adopt_project_keys(&[]));
        assert_eq!(cfg.worktree_base_branch.as_deref(), Some("dev"));
        assert!(cfg.adopt_project_keys(&[PathBuf::from("/src/app")]));
        assert_eq!(cfg.project(Path::new("/src/app")).base_branch, "dev");
        assert_eq!(cfg.project(Path::new("/src/old")).base_branch, "dev");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"worktree_base_branch": "dev", "projects": {"/src/app": {"run_command": 7}}}"#,
        )
        .unwrap();
        let mut cfg = load_from(&path);
        assert!(cfg.skipped.contains("projects"));
        assert!(!cfg.adopt_project_keys(&[PathBuf::from("/src/app")]));
    }

    /// The old global **Merge method** becomes every project's memory of
    /// its last merge, so the merge form opens where it did; a project
    /// that already merged its own way keeps that. After, each merge
    /// writes its own project's method — squash, the default, as nothing.
    #[test]
    fn the_old_merge_method_moves_into_every_project_and_merges_remember_theirs() {
        use crate::pr_actions::MergeMethod;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
              "pr_merge_method": "rebase",
              "projects": { "/src/own": { "merge_method": "merge" } }
            }"#,
        )
        .unwrap();
        let mut cfg = load_from(&path);
        let (app, own) = (Path::new("/src/app"), Path::new("/src/own"));
        assert!(cfg.adopt_project_keys(&[app.to_path_buf()]));
        assert_eq!(cfg.merge_method(app), MergeMethod::Rebase);
        assert_eq!(cfg.merge_method(own), MergeMethod::Merge, "its own wins");
        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        assert!(saved.get("pr_merge_method").is_none(), "{saved}");
        assert_eq!(saved["projects"]["/src/app"]["merge_method"], "rebase");

        // A merge by squash is the default again: nothing to keep.
        cfg.remember_merge_method(app, MergeMethod::Squash);
        assert_eq!(cfg.merge_method(app), MergeMethod::Squash);
        cfg.save_to(&path).unwrap();
        assert!(read_json_file(&path)["projects"].get("/src/app").is_none());

        // A squash global moves nothing: squash is where a project starts.
        let mut squash: Config = serde_json::from_str(r#"{"pr_merge_method": "squash"}"#).unwrap();
        assert!(squash.adopt_project_keys(&[app.to_path_buf()]));
        assert!(squash.projects.is_empty(), "{:?}", squash.projects);
    }

    #[test]
    fn done_sound_defaults_to_bell_cycles_persists_and_resolves() {
        let mut cfg = Config::default();
        assert_eq!(cfg.done_sound, "Glass");
        // A config predating the key dings too.
        let old: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(old.done_sound, "Glass");

        let (tab, row) = locate(SettingKind::DoneSound).unwrap();
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.done_sound, "bell");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.done_sound, "off");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.done_sound, "Basso", "the list wraps");
        cfg.cycle(tab, row, 0);
        assert_eq!(cfg.done_sound, "off");
        cfg.cycle(tab, row, 1);
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.done_sound, "Glass");
        assert_eq!(cfg.value_label(SettingKind::DoneSound), "Glass");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).done_sound, "Glass");

        // Silence, the bell, and every reason a name falls back to it.
        assert_eq!(resolve_sound("off", false), None);
        assert_eq!(resolve_sound("OFF", false), None);
        assert_eq!(resolve_sound("", false), None);
        assert_eq!(resolve_sound("bell", false), Some(Sound::Bell));
        assert_eq!(
            resolve_sound("Glass", true),
            Some(Sound::Bell),
            "over ssh afplay would ring the remote box"
        );
        assert_eq!(resolve_sound("NoSuchSound", false), Some(Sound::Bell));
        assert_eq!(resolve_sound("../etc/passwd", false), Some(Sound::Bell));
        assert_eq!(
            resolve_sound("Glass", false),
            Some(Sound::File(Path::new(MACOS_SOUNDS_DIR).join("Glass.aiff")))
        );
    }

    #[test]
    fn feedback_sound_defaults_to_sosumi_cycles_persists_and_resolves() {
        let mut cfg = Config::default();
        assert_eq!(cfg.feedback_sound, "Sosumi");
        assert_ne!(
            cfg.feedback_sound, cfg.done_sound,
            "red and green must sound different"
        );
        // A config predating the key rings too.
        let old: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(old.feedback_sound, "Sosumi");
        // …and one that only ever set the done sound keeps it.
        let old: Config = serde_json::from_str(r#"{"done_sound": "Ping"}"#).unwrap();
        assert_eq!(old.done_sound, "Ping");
        assert_eq!(old.feedback_sound, "Sosumi");

        // Its row sits right after the done sound on the General tab.
        let (tab, row) = locate(SettingKind::FeedbackSound).unwrap();
        assert_eq!(locate(SettingKind::DoneSound).unwrap(), (tab, row - 1));
        assert_eq!(SETTINGS_TABS[tab].title, "General");
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.feedback_sound, "Basso");
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.feedback_sound, "off", "the list wraps");
        cfg.cycle(tab, row, -1);
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.feedback_sound, "Sosumi");
        assert_eq!(cfg.value_label(SettingKind::FeedbackSound), "Sosumi");
        assert_eq!(cfg.done_sound, "Glass", "the done sound is its own row");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.feedback_sound = "off".into();
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.feedback_sound, "off");
        assert_eq!(loaded.feedback_sound(), None, "off is silence for both");
        assert_eq!(loaded.done_sound, "Glass");

        // The same resolution as the done sound: the bell over ssh,
        // silence for off.
        assert_eq!(resolve_sound("Sosumi", true), Some(Sound::Bell));
        assert_eq!(
            resolve_sound("Sosumi", false),
            Some(Sound::File(Path::new(MACOS_SOUNDS_DIR).join("Sosumi.aiff")))
        );
    }

    #[test]
    fn cycle_toggles_bools_and_walks_session_idle_timeout() {
        let mut cfg = Config::default();
        let (t, r) = locate(SettingKind::FollowNewSession).unwrap();
        assert!(cfg.follow_new_session);
        cfg.cycle(t, r, 0);
        assert!(!cfg.follow_new_session);
        cfg.cycle(t, r, 1);
        assert!(cfg.follow_new_session);

        assert_eq!(cfg.session_idle_timeout, "5m");
        let (t, r) = locate(SettingKind::SessionIdleTimeout).unwrap();
        cfg.cycle(t, r, 0);
        assert_eq!(cfg.session_idle_timeout, "15m");
        cfg.cycle(t, r, -1);
        assert_eq!(cfg.session_idle_timeout, "5m");
        cfg.cycle(t, r, -1);
        assert_eq!(cfg.session_idle_timeout, "1m");
    }

    /// A `config.json` without a `harnesses` key — `{}`, or a file from
    /// before orion first rewrote it — starts Grok Build off, as a fresh
    /// config does. A map that is there is read as written: grok on when
    /// it says so, and an empty one leaves the compiled-in default (on).
    #[test]
    fn a_config_without_harnesses_starts_grok_off() {
        with_shipped_defaults(|| {
            let grok = |cfg: &Config| {
                cfg.raw_harness_registry()
                    .into_iter()
                    .find(|entry| entry.id == "grok")
                    .expect("grok is built in")
                    .enabled
            };
            assert!(!grok(&Config::default()));
            let missing: Config = serde_json::from_str("{}").unwrap();
            assert!(!grok(&missing), "no key reads as a fresh config");
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            std::fs::write(&path, r#"{"claude_enabled": true, "theme": "ocean"}"#).unwrap();
            assert!(!grok(&load_from(&path)), "nor through the layered load");

            let on: Config =
                serde_json::from_str(r#"{"harnesses": {"grok": {"enabled": true}}}"#).unwrap();
            assert!(grok(&on), "an explicit switch stands");
            let empty: Config = serde_json::from_str(r#"{"harnesses": {}}"#).unwrap();
            assert!(empty.harnesses.is_empty(), "an explicit map, as written");
            assert!(grok(&empty));
        });
        let test: Config = serde_json::from_str("{}").unwrap();
        assert!(test.harnesses.is_empty(), "tests keep the compiled-in set");
    }

    #[test]
    fn outside_editor_defaults_to_auto_cycles_and_persists() {
        let mut cfg = Config::default();
        assert_eq!(cfg.outside_editor, "auto");
        let (tab, row) = locate(SettingKind::OutsideEditor).unwrap();
        for next in ["cursor", "vscode", "sublime", "auto"] {
            cfg.cycle(tab, row, 1);
            assert_eq!(cfg.outside_editor, next);
        }
        cfg.outside_editor = "VSCode".into();
        cfg.cycle(tab, row, 1);
        assert_eq!(
            cfg.outside_editor, "sublime",
            "a hand-typed case still steps on"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).outside_editor, "sublime");
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.outside_editor, "auto", "a config predating the key");
    }

    #[test]
    fn session_idle_timeout_cycles_and_persists() {
        let mut cfg = Config::default();
        assert_eq!(cfg.session_idle_timeout, "5m");
        let (tab, row) = locate(SettingKind::SessionIdleTimeout).unwrap();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.session_idle_timeout, "15m");
        cfg.cycle(tab, row, -2);
        assert_eq!(cfg.session_idle_timeout, "1m");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.session_idle_timeout, "off");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).session_idle_timeout, "off");
    }

    #[test]
    fn theme_cycles_through_presets_and_resolves() {
        let mut cfg = Config::default();
        assert_eq!(cfg.theme, "default");
        assert_eq!(cfg.theme(), crate::theme::Theme::default());
        let (tab, theme_row) = locate(SettingKind::Theme).unwrap();
        cfg.cycle(tab, theme_row, 1);
        assert_eq!(cfg.theme, "ocean");
        assert_ne!(cfg.theme(), crate::theme::Theme::default());
        cfg.cycle(tab, theme_row, -1);
        assert_eq!(cfg.theme, "default");
        // Unknown names (hand-edited config) cycle from the start and
        // resolve to the default palette rather than erroring.
        cfg.theme = "sparkle".into();
        assert_eq!(cfg.theme(), crate::theme::Theme::default());
    }

    #[test]
    fn animations_default_on_toggle_and_persist() {
        let mut cfg = Config::default();
        assert!(cfg.animations);
        let (tab, row) = locate(SettingKind::Animations).unwrap();
        cfg.cycle(tab, row, 0);
        assert!(!cfg.animations);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(!load_from(&path).animations);
        // A config predating the key keeps animations on.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.animations);
    }

    /// **Auto-fix my PRs** is on by default, toggling off and back on; with
    /// no session to copy, the agent runs on the default agent at the PR
    /// autofix preset's model — one that harness no longer offers falls
    /// back to its own default.
    #[test]
    fn autofix_toggles_and_falls_back_to_the_presets_model() {
        let mut cfg = Config::default();
        assert_eq!(cfg.value_label(SettingKind::PrAutofix), "on");
        cfg.cycle_kind(SettingKind::PrAutofix, 1);
        assert_eq!(cfg.autofix_mode(), crate::autofix::Mode::Off);
        assert_eq!(cfg.value_label(SettingKind::PrAutofix), "off");
        cfg.cycle_kind(SettingKind::PrAutofix, 1);
        assert_eq!(cfg.autofix_mode(), crate::autofix::Mode::Auto);
        let missing: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.autofix_mode(), crate::autofix::Mode::Auto);

        let preset = crate::agent_presets::SystemPreset::PrAutofix.default_preset();
        let defaults = cfg.autofix_launch(&preset);
        let gone = AgentPreset {
            model: Some("no-such-model".into()),
            ..preset.clone()
        };
        assert_eq!(cfg.autofix_launch(&gone), defaults);
        let descriptor = cfg.effective_harness(AgentKind::Claude, None);
        if let Some(model) = model_choices_in(&descriptor).get(1) {
            let picked = AgentPreset {
                model: Some(model.clone()),
                ..preset
            };
            assert_eq!(cfg.autofix_launch(&picked).2.as_ref(), Some(model));
        }
    }

    /// The autofix and week review settings an older release kept in the
    /// file move onto their system presets once — the review's account as
    /// it resolved then, the default agent's — and the save after drops
    /// the old keys. Empty values mean what they meant: the default
    /// agent's own model, sonnet · medium for the review.
    #[test]
    fn the_old_autofix_and_review_keys_move_onto_the_system_presets() {
        use crate::agent_presets::{ensure_system, SystemPreset};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let work = dir.path().join("work");
        std::fs::write(
            &path,
            format!(
                r#"{{"autofix_model": "", "autofix_effort": "high",
                    "review_account": "", "review_model": "", "review_effort": "low",
                    "quick_prompt_kind": "claude-work",
                    "claude_accounts": [{{"id": "claude-work", "config_dir": "{}"}}]}}"#,
                work.display()
            ),
        )
        .unwrap();
        let mut cfg = load_from(&path);
        assert!(cfg.skipped.is_empty(), "{:?}", cfg.skipped);
        let mut presets = vec![AgentPreset {
            model: Some("opus".into()),
            ..SystemPreset::PrAutofix.default_preset()
        }];
        ensure_system(&mut presets);
        assert!(cfg.move_into_presets(&mut presets, true));
        let of = |which| presets.iter().find(|p| p.system == Some(which)).unwrap();
        let autofix = of(SystemPreset::PrAutofix);
        assert_eq!(autofix.model, None, "empty was the default agent's own");
        assert_eq!(autofix.effort.as_deref(), Some("high"));
        let review = of(SystemPreset::WeekReview);
        assert_eq!(
            (review.kind, review.custom_harness.as_deref()),
            (AgentKind::Custom, Some("claude-work")),
            "written where it was: the default agent's account"
        );
        assert_eq!(review.model.as_deref(), Some("sonnet"));
        assert_eq!(review.effort.as_deref(), Some("low"));
        assert!(!cfg.move_into_presets(&mut presets, false), "once");

        cfg.save_to(&path).unwrap();
        let saved = read_json_file(&path);
        for key in [
            "autofix_model",
            "autofix_effort",
            "review_account",
            "review_model",
            "review_effort",
        ] {
            assert!(saved.get(key).is_none(), "{key} dropped: {saved}");
        }
        assert_eq!(saved["quick_prompt_kind"], "claude-work");

        // A file from before the review had keys of its own: a new Week
        // review preset still goes on the account it was written through,
        // the default agent's, and nothing in the file moved.
        let mut cfg: Config = serde_json::from_str(&format!(
            r#"{{"quick_prompt_kind": "claude-work",
                "claude_accounts": [{{"id": "claude-work", "config_dir": "{}"}}]}}"#,
            work.display()
        ))
        .unwrap();
        let mut presets = Vec::new();
        ensure_system(&mut presets);
        assert!(!cfg.move_into_presets(&mut presets, true));
        let review = presets
            .iter()
            .find(|p| p.system == Some(SystemPreset::WeekReview))
            .unwrap();
        assert_eq!(review.custom_harness.as_deref(), Some("claude-work"));
    }

    /// SPOTIFY IN FOOTER: on out of the box, toggled off from its
    /// General row, persisted under `spotify`; a config predating the
    /// key turns it on, and a saved `false` is honoured.
    #[test]
    fn spotify_default_on_toggle_and_persist() {
        let mut cfg = Config::default();
        assert!(cfg.spotify);
        let (tab, row) = locate(SettingKind::Spotify).unwrap();
        assert_eq!(SETTINGS_TABS[tab].title, "General");
        cfg.cycle(tab, row, 0);
        assert!(!cfg.spotify);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert!(!load_from(&path).spotify);
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw.get("spotify"), Some(&serde_json::json!(false)));

        let older: Config = serde_json::from_str("{}").unwrap();
        assert!(older.spotify, "a config predating the key turns it on");
    }

    /// FOLLOW NEW SESSION starts on, leads General's Launching rows,
    /// toggles like any bool and persists under its own key; a config
    /// predating the key reads as on.
    #[test]
    fn follow_new_session_is_on_by_default_and_persists() {
        let mut cfg = Config::default();
        assert!(cfg.follow_new_session, "a launch lands on its new card");
        assert_eq!(cfg.value_label(SettingKind::FollowNewSession), "on");
        let (tab, row) = locate(SettingKind::FollowNewSession).unwrap();
        assert_eq!((SETTINGS_TABS[tab].title, row), ("General", 0));

        cfg.cycle(tab, row, 0);
        assert!(!cfg.follow_new_session);
        assert_eq!(cfg.value_label(SettingKind::FollowNewSession), "off");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#""follow_new_session": false"#), "{raw}");
        assert!(!load_from(&path).follow_new_session, "off survives a save");

        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.follow_new_session, "a missing key reads as on");
    }

    /// **Run command** on the Projects tab: a typed row (Enter prompts,
    /// ←/→ and cycle change nothing) kept per repo path as `run_command`,
    /// shown as `not set` while empty and left out of the file while
    /// empty, so an entry with nothing else in
    /// it is dropped rather than written.
    #[test]
    fn run_command_is_a_typed_project_row_kept_per_repo_path() {
        let demo = Path::new("/tmp/demo");
        let other = Path::new("/tmp/other");
        let mut cfg = Config::default();
        assert!(SettingKind::RunCommand.is_text());
        assert!(SettingKind::RunCommand.is_project());
        let (tab, row) = locate(SettingKind::RunCommand).unwrap();
        assert_eq!(tab, project_tab());
        assert_eq!(row, 2, "under the setup command");
        assert_eq!(cfg.project(demo).run_command, "");
        assert_eq!(
            cfg.project(demo).value_label(SettingKind::RunCommand),
            NO_COMMAND
        );
        assert_eq!(cfg.value_label(SettingKind::RunCommand), "not set");
        assert_eq!(cfg.project_text_value(demo, SettingKind::RunCommand), "");

        // The tab's cycle never touches a typed row.
        for delta in [0, 1, -1] {
            cfg.cycle(tab, row, delta);
        }
        assert!(cfg.projects.is_empty());
        assert!(
            !cfg.set_project_text(demo, SettingKind::Theme, "dark"),
            "not a project row"
        );
        assert!(cfg.projects.is_empty());

        assert!(cfg.set_project_text(demo, SettingKind::RunCommand, "  npm run dev "));
        assert_eq!(cfg.project(demo).run_command, "npm run dev", "trimmed");
        assert_eq!(
            cfg.project(demo).value_label(SettingKind::RunCommand),
            "npm run dev"
        );
        assert_eq!(
            cfg.project_text_value(demo, SettingKind::RunCommand),
            "npm run dev"
        );
        assert_eq!(
            cfg.project(other).value_label(SettingKind::RunCommand),
            NO_COMMAND,
            "one project's, not every project's"
        );
        assert_eq!(
            cfg.project(demo).open_command,
            "",
            "open's key is untouched"
        );

        // Persisted in the project's entry, under its path, beside no key
        // it did not set.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert!(cfg.set_project_text(other, SettingKind::OpenCommand, "open x"));
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["projects"],
            serde_json::json!({
                "/tmp/demo": { "run_command": "npm run dev" },
                "/tmp/other": { "open_command": "open x" },
            })
        );
        let loaded = load_from(&path);
        assert_eq!(loaded.project(demo).run_command, "npm run dev");
        assert_eq!(loaded.project(other).run_command, "");

        // Empty unsets it — and drops the entry.
        assert!(cfg.set_project_text(demo, SettingKind::RunCommand, "   "));
        assert_eq!(cfg.projects.keys().collect::<Vec<_>>(), [other]);
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["projects"],
            serde_json::json!({ "/tmp/other": { "open_command": "open x" } })
        );
    }

    /// **Setup command** on the Projects tab: the same typed per-project
    /// row as Run command, between the base branch and it, kept as
    /// `setup_command` — what the DAEMON runs in each new worktree. Its
    /// own key, left out of the file while empty.
    #[test]
    fn setup_command_is_a_typed_project_row_over_run_command() {
        let demo = Path::new("/tmp/demo");
        let mut cfg = Config::default();
        assert!(SettingKind::SetupCommand.is_text());
        assert!(SettingKind::SetupCommand.is_project());
        let (tab, row) = locate(SettingKind::SetupCommand).unwrap();
        assert_eq!(tab, project_tab());
        assert_eq!(row, 1, "under the base branch");
        assert_eq!(
            cfg.project(demo).value_label(SettingKind::SetupCommand),
            NO_COMMAND
        );
        cfg.cycle(tab, row, 0);
        assert!(cfg.projects.is_empty(), "a typed row cycles nothing");

        assert!(cfg.set_project_text(demo, SettingKind::SetupCommand, " just setup-worktree "));
        assert_eq!(cfg.project(demo).setup_command, "just setup-worktree");
        assert_eq!(cfg.project(demo).run_command, "", "run's key is untouched");
        assert_eq!(
            cfg.project_text_value(demo, SettingKind::SetupCommand),
            "just setup-worktree"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["projects"],
            serde_json::json!({ "/tmp/demo": { "setup_command": "just setup-worktree" } })
        );
        assert_eq!(
            load_from(&path).project(demo).setup_command,
            "just setup-worktree"
        );
        assert!(cfg.set_project_text(demo, SettingKind::SetupCommand, ""));
        assert!(cfg.projects.is_empty(), "empty unsets it");
    }

    /// **Open command** on the Projects tab: the same typed per-project row
    /// as Run command, right under it, kept as `open_command` — what
    /// `Shift+Enter` / `Shift+O` runs.
    /// Its own key, so setting it leaves `run_command` alone; empty drops
    /// it from the entry.
    #[test]
    fn open_command_is_a_typed_project_row_under_run_command() {
        let demo = Path::new("/tmp/demo");
        let mut cfg = Config::default();
        assert!(SettingKind::OpenCommand.is_text());
        assert!(SettingKind::OpenCommand.is_project());
        let (tab, row) = locate(SettingKind::OpenCommand).unwrap();
        assert_eq!(tab, project_tab());
        assert_eq!(
            row,
            locate(SettingKind::RunCommand).unwrap().1 + 1,
            "right under Run command"
        );
        assert_eq!(cfg.project(demo).open_command, "");
        assert_eq!(
            cfg.project(demo).value_label(SettingKind::OpenCommand),
            NO_COMMAND
        );
        assert_eq!(cfg.value_label(SettingKind::OpenCommand), "not set");

        // A typed row: cycling its tab row changes nothing.
        cfg.cycle(tab, row, 0);
        assert!(cfg.projects.is_empty());

        assert!(cfg.set_project_text(
            demo,
            SettingKind::OpenCommand,
            "  open http://localhost:3000 "
        ));
        assert_eq!(
            cfg.project(demo).open_command,
            "open http://localhost:3000",
            "trimmed"
        );
        assert_eq!(cfg.project(demo).run_command, "", "run's key is untouched");
        assert_eq!(
            cfg.project_text_value(demo, SettingKind::OpenCommand),
            "open http://localhost:3000"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["projects"],
            serde_json::json!({
                "/tmp/demo": { "open_command": "open http://localhost:3000" },
            })
        );
        assert_eq!(
            load_from(&path).project(demo).open_command,
            "open http://localhost:3000"
        );

        // Empty unsets it — and drops the entry.
        assert!(cfg.set_project_text(demo, SettingKind::OpenCommand, " "));
        assert!(cfg.projects.is_empty());
    }

    /// The Projects tab: every project in turn, a line naming it then its
    /// rows — each of them a project row, and no project row anywhere
    /// else. A row knows its project and setting by index alone.
    #[test]
    fn the_project_tab_lists_every_project_under_its_name() {
        use SettingsRow::{Blank, Project, Setting};
        let tab = project_tab();
        assert_eq!(SETTINGS_TABS[tab].title, "Projects");
        assert!(tab < keys_tab());
        let n = PROJECT_SETTINGS.len();
        let mut rows = vec![Project(0)];
        rows.extend((0..n).map(Setting));
        rows.extend([Blank, Project(1)]);
        rows.extend((n..2 * n).map(Setting));
        assert_eq!(settings_rows(tab, 2), rows);
        assert_eq!(tab_len(tab, 2), 2 * n);
        let kind = |i| setting_at(tab, i).map(|s| s.kind);
        assert_eq!(
            PROJECT_SETTINGS.iter().map(|s| s.kind).collect::<Vec<_>>(),
            [
                SettingKind::BaseBranch,
                SettingKind::SetupCommand,
                SettingKind::RunCommand,
                SettingKind::OpenCommand,
                SettingKind::WorktreeContainers,
                SettingKind::LinearKey,
            ]
        );
        assert_eq!(kind(n + 1), Some(SettingKind::SetupCommand));
        assert_eq!(kind(n + 2), Some(SettingKind::RunCommand));
        assert_eq!(project_at(tab, n - 1), Some(0));
        assert_eq!(project_at(tab, n), Some(1));
        assert_eq!(project_at(0, 2), None);
        assert_eq!(
            settings_rows(tab, 0),
            [SettingsRow::Note(NO_PROJECTS.into())]
        );
        assert_eq!(tab_len(tab, 0), 0);
        for (t, _, spec) in all_settings() {
            assert_eq!(
                spec.kind.is_project(),
                t == tab,
                "{:?} sits on {}",
                spec.kind,
                SETTINGS_TABS[t].title
            );
        }
    }

    /// A project's **On worktree delete** is a row of its own on the
    /// PROJECTS TAB: it starts `off`, cycles the four policies for that
    /// project alone and saves the word the daemon reads in its entry —
    /// `off` as nothing at all; a hand edit off the list reads, and steps
    /// on, as `off`.
    #[test]
    fn on_worktree_delete_is_off_by_default_and_cycles_per_project() {
        let demo = Path::new("/tmp/demo");
        let mut cfg = Config::default();
        assert_eq!(
            cfg.project(demo)
                .value_label(SettingKind::WorktreeContainers),
            "off"
        );
        assert_eq!(
            locate(SettingKind::WorktreeContainers).map(|(t, _)| t),
            Some(project_tab())
        );
        let mut seen = Vec::new();
        for _ in 0..WORKTREE_CONTAINERS.len() {
            assert!(cfg.cycle_project(demo, SettingKind::WorktreeContainers, 1));
            seen.push(
                cfg.project(demo)
                    .value_label(SettingKind::WorktreeContainers),
            );
        }
        assert_eq!(seen, ["stop", "remove", "remove+volumes", "off"], "wraps");
        assert!(cfg.projects.is_empty(), "off leaves no entry");
        cfg.cycle_project(demo, SettingKind::WorktreeContainers, -1);
        assert_eq!(cfg.project(demo).worktree_containers, "remove+volumes");
        assert_eq!(
            cfg.project(Path::new("/tmp/other")).containers(),
            WorktreeContainers::Off
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        assert_eq!(
            read_json_file(&path)["projects"]["/tmp/demo"]["worktree_containers"],
            "remove+volumes"
        );
        assert_eq!(
            load_from(&path).project(demo).containers(),
            WorktreeContainers::RemoveVolumes
        );

        let mut cfg: Config =
            serde_json::from_str(r#"{"projects": {"/tmp/demo": {"worktree_containers": "nuke"}}}"#)
                .unwrap();
        assert_eq!(cfg.project(demo).containers(), WorktreeContainers::Off);
        cfg.cycle_project(demo, SettingKind::WorktreeContainers, 1);
        assert_eq!(cfg.project(demo).worktree_containers, "stop");
    }

    /// The QUICK PROMPT's harness: one name, cycled over every AGENT KIND,
    /// read back through the fallback that steps around a harness switched
    /// off since it was chosen.
    #[test]
    fn quick_prompt_kind_cycles_every_harness_and_persists() {
        let names: Vec<String> = AgentKind::ALL
            .iter()
            .filter(|k| **k != AgentKind::Custom)
            .map(|k| k.as_str().to_string())
            .collect();
        assert_eq!(agent_kind_names(), names, "one choice per kind");

        let mut cfg = Config::default();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);
        let (tab, row) = locate(SettingKind::QuickPromptKind).unwrap();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.value_label(SettingKind::QuickPromptKind), "codex");
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Codex);
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.cycle(tab, row, 2);
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).quick_prompt_kind(), AgentKind::Cursor);

        // A config predating the key, and a name nothing parses, both read
        // as Claude rather than refusing to launch.
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);
        let cfg: Config = serde_json::from_str(r#"{"quick_prompt_kind":"gemini"}"#).unwrap();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Claude);

        // The chosen harness switched off on the AGENTS TAB steps on to the
        // first one still enabled.
        let cfg: Config = serde_json::from_str(
            r#"{"quick_prompt_kind":"codex","codex_enabled":false,"claude_enabled":false}"#,
        )
        .unwrap();
        assert_eq!(cfg.quick_prompt_kind(), AgentKind::Cursor);
    }

    #[test]
    fn harness_toggles_default_on_and_persist() {
        let mut cfg = Config::default();
        assert!(cfg.claude_enabled && cfg.codex_enabled && cfg.cursor_enabled);
        assert!(cfg.opencode_enabled);
        let builtin: Vec<AgentKind> = AgentKind::ALL
            .into_iter()
            .filter(|kind| *kind != AgentKind::Custom)
            .collect();
        assert_eq!(cfg.enabled_kinds(), builtin);
        assert!(
            !cfg.kind_enabled(AgentKind::Custom),
            "a bare Custom kind is never enabled: entries gate themselves"
        );

        let (tab, row) = locate_in(&cfg, "codex", HarnessField::Enabled).unwrap();
        cfg.cycle(tab, row, 0);
        assert!(!cfg.codex_enabled);
        assert!(!cfg.kind_enabled(AgentKind::Codex));
        assert_eq!(
            cfg.enabled_kinds(),
            vec![
                AgentKind::Claude,
                AgentKind::Cursor,
                AgentKind::Grok,
                AgentKind::OpenCode
            ],
            "the disabled kind drops out, order kept"
        );
        assert!(
            !cfg.enabled_kinds().contains(&AgentKind::Custom),
            "a bare Custom kind never lists"
        );
        // ←/→ toggle a bool just like Enter does.
        cfg.cycle(tab, row, -1);
        assert!(cfg.codex_enabled);
        cfg.cycle(tab, row, 1);
        assert!(!cfg.codex_enabled);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert!(loaded.claude_enabled);
        assert!(!loaded.codex_enabled);
        assert!(loaded.cursor_enabled);
        // A config predating the keys offers every built-in harness (a
        // bare Custom kind never lists — entries come from the registry).
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.enabled_kinds().len(), AgentKind::ALL.len() - 1);

        // Every kind off is representable (a hand edit), and reads as empty.
        let cfg: Config = serde_json::from_str(
            r#"{"claude_enabled":false,"codex_enabled":false,"cursor_enabled":false,"opencode_enabled":false,"harnesses":{"grok":{"enabled":false}}}"#,
        )
        .unwrap();
        assert!(cfg.enabled_kinds().is_empty());
    }

    #[test]
    fn grok_is_builtin_and_settings_persist_without_legacy_keys() {
        let mut cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.offered_harnesses().contains(&(AgentKind::Grok, None)));
        assert_eq!(AgentKind::parse("grok"), Some(AgentKind::Grok));
        assert_eq!(AgentKind::Grok.cli_program(), "grok");
        cfg.set_harness_enabled("grok", false);
        cfg.set_harness_model("grok", "model-id".into());
        cfg.set_harness_effort("grok", "high".into());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let loaded = load_from(&path);
        assert!(!loaded.kind_enabled(AgentKind::Grok));
        assert_eq!(
            loaded.default_model(AgentKind::Grok).as_deref(),
            Some("model-id")
        );
        assert_eq!(
            loaded.default_effort(AgentKind::Grok).as_deref(),
            Some("high")
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved["harnesses"]["grok"]["enabled"], false);
        assert_eq!(saved["harnesses"]["grok"]["model_default"], "model-id");
        assert_eq!(saved["harnesses"]["grok"]["effort_default"], "high");
    }

    /// A second Claude account's `env` survives the Accounts tab saving its
    /// section — the map is written back typed — and makes it a place a
    /// Claude session can be carried to; a Codex row has nowhere to go.
    #[test]
    fn a_second_accounts_env_survives_a_save_and_makes_it_a_target() {
        let mut cfg: Config = serde_json::from_str(
            r#"{"harnesses": {"claude-b": {"label": "Claude B", "program": "claude",
                "hooks": "claude", "resume_flag": "--resume",
                "env": {"CLAUDE_CONFIG_DIR": "~/.claude-b"}}}}"#,
        )
        .unwrap();
        cfg.set_harness_model("claude-b", "opus".into());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["harnesses"]["claude-b"]["env"]["CLAUDE_CONFIG_DIR"],
            "~/.claude-b"
        );
        let loaded = load_from(&path);
        let agent = |kind: AgentKind, custom: Option<&str>| orion_core::Agent {
            id: orion_core::AgentId("a".into()),
            worktree_id: orion_core::WorktreeId("w".into()),
            name: "a".into(),
            status: orion_core::AgentStatus::NeedsFeedback,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            custom_harness: custom.map(str::to_string),
            model: None,
            effort: None,
            session_id: Some("sid".into()),
            sort_order: 0,
            status_changed_at: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
        };
        assert_eq!(
            loaded.continue_targets(&agent(AgentKind::Claude, None)),
            vec![("claude-b".to_string(), "Claude B".to_string())]
        );
        assert_eq!(
            loaded.continue_targets(&agent(AgentKind::Custom, Some("claude-b"))),
            vec![("claude".to_string(), "Claude".to_string())]
        );
        assert!(loaded
            .continue_targets(&agent(AgentKind::Codex, None))
            .is_empty());
        let archived = orion_core::Agent {
            archived: true,
            ..agent(AgentKind::Claude, None)
        };
        assert!(loaded.continue_targets(&archived).is_empty());
    }

    #[test]
    fn offered_harnesses_list_usable_entries_in_registry_order() {
        let cfg = Config::default();
        assert_eq!(
            cfg.offered_harnesses(),
            vec![
                (AgentKind::Claude, None),
                (AgentKind::Codex, None),
                (AgentKind::Cursor, None),
                (AgentKind::Grok, None),
                (AgentKind::OpenCode, None),
            ]
        );

        // Disabled and broken entries are out; the tab still lists them
        // (with their reason) so they can be fixed.
        let cfg: Config = serde_json::from_str(
            r#"{"harnesses": {
                "agy": {"program": "agy"},
                "off": {"program": "off", "enabled": false},
                "broken": {"program": ""}
            }}"#,
        )
        .unwrap();
        let offered = cfg.offered_harnesses();
        assert_eq!(offered.len(), 6);
        assert_eq!(offered[5], (AgentKind::Custom, Some("agy".into())));
        let rows = cfg.agent_rows();
        assert!(rows.contains(&("off".to_string(), HarnessField::Enabled)));
        assert!(rows.contains(&("broken".to_string(), HarnessField::Enabled)));
        assert!(
            cfg.agent_hint("broken", HarnessField::Enabled)
                .contains("broken:"),
            "the tab names the reason"
        );

        // An entry is offered only when its program is on PATH, built-ins
        // and customs alike.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("agy"), "").unwrap();
        let hiding = serde_json::from_str::<Config>(
            r#"{"harnesses": {
                "agy": {"program": "agy"},
                "missing": {"program": "definitely-not-on-path"}
            }}"#,
        )
        .unwrap();
        let offered =
            hiding.offered_harnesses_where(|program| program_on(dir.path().as_os_str(), program));
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0], (AgentKind::Custom, Some("agy".into())));
    }

    /// Safety: one broken `harnesses` entry never takes the rest down.
    /// The picker hides it, the tab shows its reason, and every other
    /// harness launches exactly as before.
    #[test]
    fn a_broken_harness_entry_isolates_itself() {
        let cfg: Config = serde_json::from_str(
            r#"{"harnesses": {
                "codex": {"program": ""},
                "agy": {"program": "agy", "resume_flag": "--resume"}
            }}"#,
        )
        .unwrap();
        let offered = cfg.offered_harnesses();
        assert!(
            !offered.iter().any(|(kind, _)| *kind == AgentKind::Codex),
            "the broken built-in hides"
        );
        assert!(
            offered.contains(&(AgentKind::Custom, Some("agy".into()))),
            "the valid newcomer offers"
        );
        assert_eq!(cfg.default_model(AgentKind::Claude), None);
        assert_eq!(cfg.default_model(AgentKind::Codex), None);
        assert!(cfg
            .agent_hint("codex", HarnessField::Enabled)
            .contains("broken:"));
        // The entry still resolves for reads (placeholder-free), while
        // launches refuse it with the reason.
        let codex = cfg.effective_harness_by_id("codex");
        assert!(codex.problem().is_some());
        assert_eq!(
            orion_core::harness::resolve(&cfg.harness_registry(), AgentKind::Codex, None)
                .unwrap_err(),
            "harness `codex` has no program"
        );
    }

    /// Safety: a `harnesses` map that fails to parse costs only its own
    /// key — every other setting keeps its value, and the registry reads
    /// as a fresh install.
    #[test]
    fn an_unreadable_harnesses_map_costs_only_that_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"harnesses": {"claude": {"enabled": "yes"}}, "theme": "ocean"}"#,
        )
        .unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.theme, "ocean");
        assert_eq!(cfg.skipped, BTreeSet::from(["harnesses".to_string()]));
        assert!(cfg.harness_registry().iter().all(|entry| entry.enabled));
    }

    #[test]
    fn model_effort_defaults_resolve_and_cycle() {
        let mut cfg = Config::default();
        // "default" everywhere → no flags for any kind.
        assert_eq!(cfg.default_model(AgentKind::Claude), None);
        assert_eq!(cfg.default_effort(AgentKind::Claude), None);
        assert_eq!(cfg.default_model(AgentKind::Codex), None);
        assert_eq!(cfg.default_effort(AgentKind::Codex), None);

        cfg.claude_model = "opus".into();
        cfg.codex_effort = "high".into();
        assert_eq!(
            cfg.default_model(AgentKind::Claude).as_deref(),
            Some("opus")
        );
        assert_eq!(cfg.default_effort(AgentKind::Claude), None);
        assert_eq!(cfg.default_model(AgentKind::Codex), None);
        assert_eq!(
            cfg.default_effort(AgentKind::Codex).as_deref(),
            Some("high")
        );
        assert_eq!(AgentKind::parse("pi"), None, "Pi is gone");
        assert_eq!(AgentKind::parse("muse"), None, "and so is Muse");
        // OpenCode passes a `provider/model` id through verbatim and has
        // no effort key at all: no Effort row to locate.
        assert_eq!(cfg.default_model(AgentKind::OpenCode), None);
        assert_eq!(cfg.default_effort(AgentKind::OpenCode), None);
        cfg.opencode_model = "anthropic/claude-sonnet-5".into();
        assert_eq!(
            cfg.default_model(AgentKind::OpenCode).as_deref(),
            Some("anthropic/claude-sonnet-5")
        );
        assert_eq!(cfg.default_effort(AgentKind::OpenCode), None);
        assert_eq!(AgentKind::parse("opencode"), Some(AgentKind::OpenCode));
        assert_eq!(AgentKind::OpenCode.cli_program(), "opencode");
        assert!(locate_in(&cfg, "opencode", HarnessField::Model).is_some());
        assert!(
            locate_in(&cfg, "opencode", HarnessField::Effort).is_none(),
            "no Effort row"
        );
        cfg.claude_effort = "xhigh".into();
        let (tab, row) = locate_in(&cfg, "claude", HarnessField::Effort).unwrap();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.agent_value("claude", HarnessField::Effort), "max");
        cfg.cycle(tab, row, 1);
        assert_eq!(
            cfg.agent_value("claude", HarnessField::Effort),
            DEFAULT_CHOICE,
            "wraps"
        );
        cfg.claude_effort = DEFAULT_CHOICE.into();
        // Cursor: the family is the model; the effort only counts when
        // that family ships it.
        assert_eq!(cfg.default_model(AgentKind::Cursor), None);
        assert_eq!(cfg.default_effort(AgentKind::Cursor), None);
        cfg.cursor_effort = "high".into();
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor),
            None,
            "no family, no suffix to join"
        );
        cfg.cursor_model = "claude-opus-5".into();
        assert_eq!(
            cfg.default_model(AgentKind::Cursor).as_deref(),
            Some("claude-opus-5")
        );
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor).as_deref(),
            Some("high")
        );
        cfg.cursor_effort = "max".into();
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor).as_deref(),
            Some("high"),
            "Opus 5 has no max variant and no bare id: its fallback launches"
        );
        cfg.cursor_model = "gpt-5.3-codex".into();
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor),
            None,
            "a family with a bare id: default really is no suffix"
        );
        cfg.cursor_effort = "high-fast".into();
        assert_eq!(
            cfg.default_effort(AgentKind::Cursor).as_deref(),
            Some("high-fast")
        );

        // The settings rows walk the same choice lists the submenus show.
        let (tab, row) = locate_in(&cfg, "claude", HarnessField::Model).unwrap();
        cfg.claude_model = "default".into();
        cfg.cycle(tab, row, 1);
        assert_eq!(cfg.claude_model, "fable");
        cfg.cycle(tab, row, -1);
        assert_eq!(cfg.claude_model, "default");
        let (tab, row) = locate_in(&cfg, "codex", HarnessField::Effort).unwrap();
        cfg.cycle(tab, row, 0);
        assert_eq!(
            cfg.codex_effort, "xhigh",
            "activate steps forward from high"
        );
    }

    #[test]
    fn cursor_settings_rows_follow_the_family() {
        let mut cfg = Config::default();
        let (tab, model_row) = locate_in(&cfg, "cursor", HarnessField::Model).unwrap();
        let (_, effort_row) = locate_in(&cfg, "cursor", HarnessField::Effort).unwrap();
        // No family: the effort row is n/a and does not cycle.
        assert_eq!(cfg.agent_value("cursor", HarnessField::Effort), "n/a");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "default");
        // default → auto (still no efforts) → claude-fable-5, which has no
        // bare id, so the effort lands on its fallback at once.
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "auto");
        assert_eq!(cfg.agent_value("cursor", HarnessField::Effort), "n/a");
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "claude-fable-5");
        assert_eq!(cfg.cursor_effort, "high");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "xhigh");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "max");
        // fable-5-thinking has max; Opus 5 doesn't → back to its fallback.
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "claude-fable-5-thinking");
        assert_eq!(cfg.cursor_effort, "max", "a shared effort survives");
        cfg.cycle(tab, model_row, 1);
        assert_eq!(cfg.cursor_model, "claude-opus-5");
        assert_eq!(cfg.cursor_effort, "high");
        cfg.cycle(tab, effort_row, 1);
        assert_eq!(cfg.cursor_effort, "high-fast", "-fast rides in the effort");
        // A family with a bare id offers default again, and ← wraps onto
        // its last fast variant.
        cfg.cursor_model = "gpt-5.3-codex".into();
        cfg.cursor_effort = "default".into();
        cfg.cycle(tab, effort_row, -1);
        assert_eq!(cfg.cursor_effort, "xhigh-fast");
    }

    #[test]
    fn fit_effort_resolves_cursor_pairs() {
        let cursor = orion_core::harness::builtin("cursor").unwrap();
        let fit = |m: Option<&str>, e: Option<&str>| fit_effort_in(&cursor, m, e.map(String::from));
        assert_eq!(fit(None, Some("high")), None, "no family, nothing to join");
        assert_eq!(fit(Some("default"), Some("high")), None);
        assert_eq!(
            fit(Some("auto"), Some("high")),
            None,
            "auto has no variants"
        );
        assert_eq!(fit(Some("nope"), Some("high")), None);
        assert_eq!(fit(Some("gpt-5.3-codex"), None), None, "bare id exists");
        assert_eq!(fit(Some("gpt-5.3-codex"), Some("bogus")), None);
        assert_eq!(
            fit(Some("gpt-5.3-codex"), Some("fast")).as_deref(),
            Some("fast")
        );
        assert_eq!(fit(Some("claude-fable-5"), None).as_deref(), Some("high"));
        assert_eq!(
            fit(Some("claude-fable-5"), Some("default")).as_deref(),
            Some("high")
        );
        assert_eq!(
            fit(Some("claude-fable-5"), Some("MAX ")).as_deref(),
            Some("max")
        );
        assert_eq!(
            fit(Some("gpt-5.5"), Some("xhigh")).as_deref(),
            Some("high"),
            "spelled extra-high there"
        );
        assert_eq!(
            fit(Some("gpt-5.5"), Some("extra-high-fast")).as_deref(),
            Some("extra-high-fast")
        );
        let codex = orion_core::harness::builtin("codex").unwrap();
        assert_eq!(
            fit_effort_in(&codex, None, Some("high".into())).as_deref(),
            Some("high"),
            "claude/codex pass through"
        );
    }

    /// `claude_models` is hand-edited only: empty by default, written back
    /// as `[]` so the key is discoverable, and read back verbatim — a
    /// Bedrock id or an org's full model name survives the round trip.
    #[test]
    fn claude_models_key_round_trips_and_defaults_empty() {
        assert!(Config::default().claude_models.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        Config::default().save_to(&path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["claude_models"], serde_json::json!([]));

        std::fs::write(
            &path,
            r#"{"claude_models": ["claude-sonnet-5", "us.anthropic.claude-opus-5-v1:0"]}"#,
        )
        .unwrap();
        let cfg = load_from(&path);
        assert_eq!(
            cfg.claude_models,
            vec![
                "claude-sonnet-5".to_string(),
                "us.anthropic.claude-opus-5-v1:0".to_string()
            ]
        );
        cfg.save_to(&path).unwrap();
        assert_eq!(load_from(&path).claude_models, cfg.claude_models);
    }

    #[test]
    fn save_persists_model_effort_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config {
            claude_model: "sonnet".into(),
            codex_effort: "xhigh".into(),
            cursor_model: "gpt-5.6-sol".into(),
            cursor_effort: "none".into(),
            ..Config::default()
        };
        cfg.save_to(&path).unwrap();
        let reread = load_from(&path);
        assert_eq!(reread.claude_model, "sonnet");
        assert_eq!(reread.claude_effort, "default");
        assert_eq!(reread.codex_model, "default");
        assert_eq!(reread.codex_effort, "xhigh");
        assert_eq!(reread.cursor_model, "gpt-5.6-sol");
        assert_eq!(reread.cursor_effort, "none");
    }

    #[test]
    fn save_patches_known_keys_and_keeps_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
  "animations": false,
  "future_daemon_flag": true,
  "session_idle_timeout": "15m"
}
"#,
        )
        .unwrap();

        let mut cfg = load_from(&path);
        assert!(!cfg.animations);
        assert_eq!(cfg.session_idle_timeout, "15m");
        cfg.spotify = false;
        cfg.animations = true;
        cfg.session_idle_timeout = "1h".into();
        cfg.save_to(&path).unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["spotify"], false);
        assert_eq!(saved["animations"], true);
        assert_eq!(saved["session_idle_timeout"], "1h");
        assert_eq!(saved["future_daemon_flag"], true);
    }

    #[test]
    fn tabs_cover_every_setting_once_and_rows_match() {
        with_empty_config(|| {
            // Every SettingKind appears exactly once across the tabs.
            let mut kinds: Vec<SettingKind> = all_settings().map(|(_, _, s)| s.kind).collect();
            let total = kinds.len();
            kinds.sort_by_key(|k| format!("{k:?}"));
            kinds.dedup();
            assert_eq!(kinds.len(), total, "a kind repeats across tabs");

            // Each tab's rows walk its own index space, in order.
            for (t, tab) in SETTINGS_TABS.iter().enumerate() {
                let indices: Vec<usize> = settings_rows(t, 1)
                    .into_iter()
                    .filter_map(|row| row.index())
                    .collect();
                assert_eq!(
                    indices,
                    (0..tab_len(t, 1)).collect::<Vec<_>>(),
                    "{} rows",
                    tab.title
                );
            }

            // A value tab carries headers exactly when its rows name
            // groups; Keys and Accounts always do.
            for (t, tab) in SETTINGS_TABS.iter().enumerate() {
                let headers = settings_rows(t, 1)
                    .into_iter()
                    .filter(|row| matches!(row, SettingsRow::Header(_)))
                    .count();
                match tab.body {
                    TabBody::Values(settings) | TabBody::Project(settings) => {
                        let grouped = settings.iter().any(|s| !s.group.is_empty());
                        assert_eq!(headers > 0, grouped, "{}", tab.title);
                    }
                    TabBody::Keys => assert!(headers > 0, "the Keys tab groups its rows"),
                    TabBody::Accounts => assert!(headers > 0, "the Accounts tab groups its rows"),
                }
            }
        });
    }

    /// Settings is five tabs, and General holds what is left of orion's
    /// own switches under three groups.
    #[test]
    fn settings_is_five_tabs_and_general_has_three_groups() {
        let titles: Vec<&str> = SETTINGS_TABS.iter().map(|t| t.title).collect();
        assert_eq!(
            titles,
            ["General", "Accounts", "Projects", "Presets", "Keys"]
        );
        let general: Vec<(&str, &str)> = GENERAL_SETTINGS
            .iter()
            .map(|s| (s.group, s.label))
            .collect();
        assert_eq!(
            general,
            [
                ("Launching", "Follow new session"),
                ("Launching", "Idle session timeout"),
                ("Launching", "Auto-fix my PRs"),
                ("Launching", "Open files in"),
                ("Sounds", "Turn finished"),
                ("Sounds", "Needs you"),
                ("Look", "Theme"),
                ("Look", "Animations"),
                ("Look", "Spotify in footer"),
            ]
        );
        let project: Vec<&str> = PROJECT_SETTINGS.iter().map(|s| s.label).collect();
        assert_eq!(
            project,
            [
                "Base branch",
                "Setup command",
                "Run command",
                "Open command",
                "On worktree delete",
                "Linear key"
            ]
        );
    }

    #[test]
    fn accounts_tab_groups_its_rows_per_harness() {
        with_empty_config(|| {
            let tab = accounts_tab();
            assert_eq!(SETTINGS_TABS[tab].title, "Accounts");
            let cfg = Config::load();

            // Read the rows back the way the screen shows them: the head's
            // rows first, under no header, then a header and the labels
            // under it, with a blank between sections.
            let mut sections: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];
            for row in settings_rows(tab, 1) {
                match row {
                    SettingsRow::Header(title) => sections.push((title, Vec::new())),
                    SettingsRow::Setting(i) => {
                        let label = match (ACCOUNTS_HEAD.get(i), cfg.account_row(i)) {
                            (Some(spec), _) => spec.label.to_string(),
                            (None, Some(AccountRow::Account(id))) => id,
                            (None, Some(AccountRow::Add)) => "Add account".to_string(),
                            (None, None) => cfg
                                .agent_row(i)
                                .map(|(_, field)| field.label().to_string())
                                .expect("every Accounts row resolves"),
                        };
                        sections.last_mut().unwrap().1.push(label);
                    }
                    SettingsRow::Blank => {
                        assert!(!sections[0].1.is_empty(), "no leading blank")
                    }
                    SettingsRow::Project(_) | SettingsRow::Hotkey(_) | SettingsRow::Note(_) => {
                        unreachable!()
                    }
                }
            }
            assert_eq!(
                sections,
                vec![
                    (String::new(), vec!["New agents on".to_string()]),
                    (
                        "Claude accounts".to_string(),
                        vec!["claude".to_string(), "Add account".to_string()]
                    ),
                    (
                        "Claude".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "Codex".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "Cursor".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "Grok Build".to_string(),
                        vec![
                            "Enabled".to_string(),
                            "Model".to_string(),
                            "Effort".to_string()
                        ]
                    ),
                    (
                        "OpenCode".to_string(),
                        vec!["Enabled".to_string(), "Model".to_string()]
                    ),
                ]
            );

            // One blank line separates the sections and nothing else does.
            let blanks = settings_rows(tab, 1)
                .into_iter()
                .filter(|row| *row == SettingsRow::Blank)
                .count();
            assert_eq!(blanks, sections.len() - 1);
        });
    }

    /// A new CLI in the registry grows its own Accounts section — model and
    /// effort rows included — with no code change, and the picker offers
    /// it in the same order.
    #[test]
    fn accounts_tab_grows_a_section_per_registry_entry() {
        with_empty_config(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.json");
            std::fs::write(
                &path,
                r#"{"harnesses": {
                    "agy": {
                        "program": "agy",
                        "model_default": "big-1",
                        "effort_flag": "--effort",
                        "efforts": ["low", "high"],
                        "resume_flag": "--resume",
                        "hooks": "claude"
                    }
                }}"#,
            )
            .unwrap();
            with_config_path(path, || {
                let tab = accounts_tab();
                let cfg = Config::load();
                let sections: Vec<String> = settings_rows(tab, 1)
                    .into_iter()
                    .filter_map(|row| match row {
                        SettingsRow::Header(title) => Some(title),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    sections,
                    vec![
                        "Claude accounts",
                        "Claude",
                        "Codex",
                        "Cursor",
                        "Grok Build",
                        "OpenCode",
                        "agy"
                    ]
                );
                let (_, model_row) =
                    locate_agent("agy", HarnessField::Model).expect("the newcomer locates");
                assert_eq!(cfg.agent_value("agy", HarnessField::Model), "big-1");
                let (_, effort_row) =
                    locate_agent("agy", HarnessField::Effort).expect("its effort row shows");
                assert_eq!(cfg.agent_value("agy", HarnessField::Effort), "default");
                assert_eq!(
                    tab_len(tab, 1),
                    ACCOUNTS_HEAD.len() + cfg.account_rows().len() + cfg.agent_rows().len()
                );
                // ... and the picker offers it after the built-ins.
                let offered = cfg.offered_harnesses();
                assert_eq!(
                    offered.last(),
                    Some(&(AgentKind::Custom, Some("agy".to_string())))
                );
                // Cycling its rows edits the map entry a save persists:
                // an off-list hand edit steps onto the list, effort
                // steps forward through its rows.
                let mut cfg = cfg;
                cfg.cycle(tab, model_row, 0);
                assert_eq!(
                    cfg.harnesses["agy"].model_default.as_deref(),
                    Some("default"),
                    "an off-list hand edit steps onto the offered rows"
                );
                cfg.cycle(tab, effort_row, 1);
                assert_eq!(cfg.harnesses["agy"].effort_default.as_deref(), Some("low"));
            });
        });
    }

    #[test]
    fn every_tab_holds_something() {
        with_empty_config(|| {
            assert!(tab_count() >= 2);
            for (t, tab) in SETTINGS_TABS.iter().enumerate() {
                assert!(tab_len(t, 1) > 0, "{} is empty", tab.title);
                assert!(!tab.title.is_empty());
            }
            assert_eq!(tab_len(keys_tab(), 1), crate::keymap::ACTIONS.len());
        });
    }

    #[test]
    fn keybindings_round_trip_through_the_config_file() {
        let mut cfg = Config::default();
        assert!(cfg.keybindings.is_empty(), "no overrides out of the box");
        cfg.keybindings = BTreeMap::from([("quit".into(), "f9".into())]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        cfg.save_to(&path).unwrap();
        let reloaded = load_from(&path);
        assert_eq!(
            reloaded.keybindings.get("quit").map(String::as_str),
            Some("f9")
        );
        assert_eq!(
            reloaded.keymap().lookup(
                crate::keymap::Scope::Global,
                &crate::keymap::KeyChord::parse("f9").unwrap()
            ),
            Some(crate::keymap::Action::Quit)
        );
        // A config predating the key still gets the full default keymap.
        let old: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(
            old.keymap().label(crate::keymap::Action::Quit),
            Keymap::default().label(crate::keymap::Action::Quit)
        );
    }

    #[test]
    fn save_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        Config::default().save_to(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["follow_new_session"], true);
        assert_eq!(saved["spotify"], true);
        assert_eq!(saved["session_idle_timeout"], "5m");
    }
}
