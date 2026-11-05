//! The harness registry: one behavior descriptor per agent CLI.
//!
//! The five known harnesses ship as a compiled-in table ([`builtin`]);
//! the user's config adds a `harnesses` map of [`HarnessOverride`]s over
//! it — switch one off, give it a name, a default model and effort, a
//! model list or an environment, or add a CLI by the name of its program
//! — and [`registry`] merges the two (plus the `claude_accounts` list,
//! each account a copy of Claude's row in its own config dir) into the
//! effective [`HarnessDescriptor`]s every surface reads: the `n` picker,
//! the `e` presets, spawn and resume, hooks, and the Accounts tab. How a
//! CLI is driven — its flags, its resume shape, its hook dialect — is the
//! compiled-in row's alone, with the three genuinely bespoke arg shapings
//! (Cursor's composed model id, Codex's `-c` config pair and its
//! positional resume, OpenCode's `--prompt` flag) carried as descriptor
//! fields.
//!
//! [`AgentKind`](crate::AgentKind) stays the identity — built-ins by
//! variant, a Claude account as `AgentKind::Claude` with the account's id
//! beside it, customs by id beside `AgentKind::Custom` ([`registry_id`],
//! [`HarnessDescriptor::session_kind`]) — so exhaustive matches still fail
//! to compile when a built-in is added. Behavior never branches on it
//! directly anymore; it resolves to a descriptor first.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::claude_account::ClaudeAccount;
use crate::AgentKind;

/// A model id the pickers offer, verbatim, before the `default` sentinel
/// the UI heads every list with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    /// The id passed to the CLI.
    pub id: String,
    /// One-line note the pickers show beside it (provider, generation).
    /// None renders no note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Which TUI-side catalogue supplies a harness's model (and, for Cursor,
/// effort) rows at runtime, in place of the descriptor's static lists:
/// Claude's `claude_models` / `availableModels` allowlist, or Cursor's
/// `--list-models` union over its seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessCatalog {
    Claude,
    Cursor,
}

impl HarnessCatalog {
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessCatalog::Claude => "claude",
            HarnessCatalog::Cursor => "cursor",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "claude" => HarnessCatalog::Claude,
            "cursor" => HarnessCatalog::Cursor,
            _ => return None,
        })
    }
}

/// The model/effort sentinel meaning "don't pass the flag — let the CLI
/// pick"; it heads every choice list and is what the daemon sees as None.
pub const DEFAULT_CHOICE: &str = "default";

/// How a harness takes its model: the flag carrying it, the default the
/// Accounts tab edits, and the static list the pickers offer (before the
/// `default` sentinel, which always means "don't pass the flag").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Flag carrying the model id (`--model`). None = the CLI takes no
    /// model flag; the Model row still edits the stored default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// Default model id. `"default"` means don't pass the flag.
    #[serde(default = "default_model_choice")]
    pub default: String,
    /// Static model list; a [`HarnessCatalog`] replaces it at runtime.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelEntry>,
    /// Runtime catalogue replacing [`ModelSpec::models`], if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<HarnessCatalog>,
}

/// How a harness takes its reasoning effort: a plain flag (`--effort`),
/// Codex's `-c model_reasoning_effort=` config pair, or nothing (composed
/// into the model id, or unsupported).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffortSpec {
    /// Flag carrying the effort (`--effort`). Mutually exclusive with
    /// `config_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// The `-c` style flag introducing `config_key=value` pairs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_flag: Option<String>,
    /// The config key, carried by `config_flag` as `key=value`. Requires
    /// `config_flag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_key: Option<String>,
    /// Default effort. `"default"` means don't pass anything.
    #[serde(default = "default_model_choice")]
    pub default: String,
    /// Static effort list, in the CLI's own order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    /// Whether effort is a concept for this harness at all: the Agents
    /// tab's Effort row and the effort submenus show only while set.
    #[serde(default)]
    pub offered: bool,
}

/// How a harness resumes a stored CLI session id: a flag (`--resume`,
/// `--session`), Codex's positional subcommand (`resume <id>`, with
/// `--cd`), or nothing (the CLI boots fresh and the id is ignored).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeSpec {
    /// Resume flag. Mutually exclusive with `subcommand`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// Positional subcommand naming the resume (`resume`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subcommand: Option<String>,
    /// Pass the worktree as `--cd <dir>` after the id. Only meaningful
    /// with `subcommand`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cd: bool,
}

/// How orion's guidance (worktree rules, PR scope) reaches the CLI:
/// appended to the system prompt through a flag, prepended to the first
/// prompt, or dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSpec {
    /// System-prompt flag (`--append-system-prompt`). Mutually exclusive
    /// with `prepend_to_first_prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_flag: Option<String>,
    /// Fold the guidance into the first prompt instead.
    #[serde(default, skip_serializing_if = "is_false")]
    pub prepend_to_first_prompt: bool,
}

/// What a launch asks the agent to do with the task: change the code
/// (the CLI's own default), plan the change without making it, or only
/// answer questions about the code. Cursor's agent picker offers all
/// three; Claude Code has a plan mode (`--permission-mode plan`) and no
/// read-only ask mode. Request-only, like a STARTING PROMPT: the mode is
/// where a session starts, and the CLI's own toggle moves it from there,
/// so a resume never forces it back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Make the change: no flag, the CLI starts as it always does.
    #[default]
    Edit,
    /// Read the code and propose a plan; no edits.
    Plan,
    /// Answer questions about the code; no edits.
    Ask,
}

impl AgentMode {
    /// The order the box's mode field cycles through.
    pub const ALL: [AgentMode; 3] = [AgentMode::Edit, AgentMode::Plan, AgentMode::Ask];

    pub fn as_str(self) -> &'static str {
        match self {
            AgentMode::Edit => "edit",
            AgentMode::Plan => "plan",
            AgentMode::Ask => "ask",
        }
    }
}

/// How a harness starts in a mode other than [`AgentMode::Edit`]: the flag
/// and the value it takes for each mode it has. A mode with no value is
/// one the CLI does not have, and the box does not offer it; a harness
/// with neither offers no mode at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeSpec {
    /// Flag carrying the mode (`--permission-mode`, `--mode`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// The flag's value for [`AgentMode::Plan`] (`plan`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The flag's value for [`AgentMode::Ask`] (`ask`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
}

impl ModeSpec {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The flag and value that start the CLI in `mode`; None for
    /// [`AgentMode::Edit`] and for a mode this harness does not have.
    pub fn args(&self, mode: AgentMode) -> Option<[&str; 2]> {
        let flag = self.flag.as_deref()?;
        let value = match mode {
            AgentMode::Edit => return None,
            AgentMode::Plan => self.plan.as_deref()?,
            AgentMode::Ask => self.ask.as_deref()?,
        };
        Some([flag, value])
    }

    /// The modes a launch on this harness can pick, in cycle order:
    /// [`AgentMode::Edit`] plus each one the CLI has. Edit alone when it
    /// has none — the box then draws no mode field.
    pub fn offered(&self) -> Vec<AgentMode> {
        AgentMode::ALL
            .into_iter()
            .filter(|&mode| mode == AgentMode::Edit || self.args(mode).is_some())
            .collect()
    }
}

/// The effective behavior of one harness: what the picker offers, what
/// spawn builds, which hooks install, and what the Accounts tab edits.
/// Built-ins come from [`builtin`]; user entries merge over them (or over
/// a [`placeholder`] for new ids) through [`HarnessOverride`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessDescriptor {
    /// Stable id: the builtin name, the Claude account's id, or the custom
    /// entry id.
    pub id: String,
    /// The kind a session on this row is stored as: the built-in's own,
    /// [`AgentKind::Claude`] for every Claude account — the extra ones
    /// carry their id beside it — and [`AgentKind::Custom`] for a CLI the
    /// `harnesses` map adds.
    pub kind: AgentKind,
    /// Display label for the picker, badges and tab sections.
    pub label: String,
    /// The CLI orion launches, resolved on PATH through the login shell.
    pub program: String,
    /// Environment the CLI is launched with, on top of the user's own —
    /// `CLAUDE_CONFIG_DIR` pointing a second Claude account at its own
    /// config dir, say. Restated after the login shell's profile has run,
    /// so a profile exporting the same name cannot undo it; a leading
    /// `~/` is expanded ([`HarnessDescriptor::launch_env`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Whether the picker and presets offer this harness.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Model flag, default and list.
    #[serde(default)]
    pub model: ModelSpec,
    /// Effort passing, default and list.
    #[serde(default)]
    pub effort: EffortSpec,
    /// Skip-permissions flag (`--yolo`, `--force`). None = the CLI has
    /// none and launches as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions_flag: Option<String>,
    /// Flag carrying the first prompt (`--prompt`) where the CLI's
    /// positional is not a prompt (OpenCode's is the project path). None =
    /// the prompt rides trailing, like every other CLI's positional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_flag: Option<String>,
    /// How a stored session id resumes.
    #[serde(default)]
    pub resume: ResumeSpec,
    /// How guidance text reaches the CLI.
    #[serde(default)]
    pub system: SystemSpec,
    /// Hook dialect to install, naming the built-in CLI whose hooks this
    /// harness speaks (`claude`, `codex`, `cursor`, `grok`, `opencode`).
    /// Sessions report status, prompts and permission waits like that
    /// harness; without one they stay process-based. A Claude-compatible
    /// CLI gets title sync with `claude`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<String>,
    /// Whether a relocated session resumes with the trailing "continue in
    /// this checkout" prompt (only for CLIs verified to open a resumed
    /// session on one).
    #[serde(default, skip_serializing_if = "is_false")]
    pub relocation_prompt: bool,
    /// Whether model and effort compose into one `--model {m}-{e}` id
    /// (Cursor's family-suffix shape).
    #[serde(default, skip_serializing_if = "is_false")]
    pub compose_model_effort: bool,
    /// How the CLI starts in plan or ask mode, where it has them.
    #[serde(default, skip_serializing_if = "ModeSpec::is_empty")]
    pub mode: ModeSpec,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// The hook dialects a `hooks` row can name, each the built-in CLI whose
/// hooks it is.
const HOOK_DIALECTS: [(&str, AgentKind); 5] = [
    ("claude", AgentKind::Claude),
    ("codex", AgentKind::Codex),
    ("cursor", AgentKind::Cursor),
    ("grok", AgentKind::Grok),
    ("opencode", AgentKind::OpenCode),
];

fn dialect_named(name: &str) -> Option<AgentKind> {
    HOOK_DIALECTS
        .iter()
        .find(|(dialect, _)| *dialect == name)
        .map(|(_, kind)| *kind)
}

/// Whether a dialect's hooks say when the session waits on you: Claude's
/// and Codex's `PermissionRequest`, OpenCode's `permission.asked` and
/// question tool. Cursor and Grok have no such event.
fn reports_waiting(dialect: AgentKind) -> bool {
    match dialect {
        AgentKind::Claude | AgentKind::Codex | AgentKind::OpenCode => true,
        AgentKind::Cursor | AgentKind::Grok | AgentKind::Custom => false,
    }
}

/// [`HarnessDescriptor::status_note`] for a CLI with no hook dialect.
pub const PROCESS_STATUS_NOTE: &str = "status from the process only";
/// [`HarnessDescriptor::status_note`] for a CLI that asks, unseen.
pub const NO_WAITING_NOTE: &str = "can't show it's waiting on you";

fn default_enabled() -> bool {
    true
}

fn default_model_choice() -> String {
    DEFAULT_CHOICE.into()
}

impl Default for ModelSpec {
    fn default() -> Self {
        Self {
            flag: Some("--model".into()),
            default: default_model_choice(),
            models: Vec::new(),
            catalog: None,
        }
    }
}

impl Default for EffortSpec {
    fn default() -> Self {
        Self {
            flag: None,
            config_flag: None,
            config_key: None,
            default: default_model_choice(),
            efforts: Vec::new(),
            offered: false,
        }
    }
}

impl HarnessDescriptor {
    /// What a session launched on this row stores: its kind, and the
    /// registry id beside it wherever the kind alone does not name the
    /// row — an extra Claude account's, a custom harness's. The inverse
    /// of [`registry_id`].
    pub fn session_kind(&self) -> (AgentKind, Option<String>) {
        let named = self.kind == AgentKind::Custom || self.id != self.kind.as_str();
        (self.kind, named.then(|| self.id.clone()))
    }

    /// The label the picker and session rows show.
    pub fn display_label(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.id
        } else {
            &self.label
        }
    }

    /// The configured default model, or None for `"default"` (don't pass
    /// the flag) — the same sentinel the model settings use.
    pub fn default_model(&self) -> Option<&str> {
        let value = self.model.default.trim();
        (!value.is_empty() && !value.eq_ignore_ascii_case(DEFAULT_CHOICE)).then_some(value)
    }

    /// The configured default effort, or None for `"default"`. Callers
    /// composing Cursor ids fit it against the family first.
    pub fn default_effort(&self) -> Option<&str> {
        let value = self.effort.default.trim();
        (!value.is_empty() && !value.eq_ignore_ascii_case(DEFAULT_CHOICE)).then_some(value)
    }

    /// Whether this harness resumes `sid` in place (as opposed to booting
    /// fresh and ignoring it).
    pub fn resumes(&self) -> bool {
        self.resume.flag.is_some() || self.resume.subcommand.is_some()
    }

    /// Whether the harness takes a system prompt through a flag.
    pub fn system_append_flag(&self) -> Option<&str> {
        self.system.append_flag.as_deref()
    }

    /// The hook dialect this harness's sessions install, if any.
    pub fn hook_dialect(&self) -> Option<crate::AgentKind> {
        dialect_named(self.hooks.as_deref()?.trim())
    }

    /// What the NEW AGENT PICKER says beside this harness when its
    /// sessions show less than the whole range of status: a CLI with no
    /// hook dialect is only ever working or exited, and one that stops to
    /// ask (no skip-permissions flag) on a dialect with no waiting signal
    /// never shows that it is waiting on you. None for the full range.
    pub fn status_note(&self) -> Option<&'static str> {
        match self.hook_dialect() {
            None => Some(PROCESS_STATUS_NOTE),
            Some(dialect) if !reports_waiting(dialect) && self.permissions_flag.is_none() => {
                Some(NO_WAITING_NOTE)
            }
            Some(_) => None,
        }
    }

    /// Whether rows on this harness title-push and auto-title like Claude:
    /// any harness speaking its hook dialect, builtin Claude included. The
    /// title travels in hook replies, so a cleared `hooks` row is not
    /// claude-like even when its id is `claude`.
    pub fn claude_like(&self) -> bool {
        self.hook_dialect() == Some(AgentKind::Claude)
    }

    /// The `env` rows as the spawn sets them, a leading `~/` expanded
    /// against `$HOME`: the value reaches the CLI single-quoted, where no
    /// shell would expand it.
    pub fn launch_env(&self) -> Vec<(String, String)> {
        self.env
            .iter()
            .map(|(name, value)| (name.clone(), expand_home(value)))
            .collect()
    }

    /// The Claude config dir `env` pins this harness's CLI to — its
    /// `CLAUDE_CONFIG_DIR`, expanded. The one config dir orion can be sure
    /// of: the spawn restates it after the user's profile.
    pub fn pinned_claude_config_dir(&self) -> Option<PathBuf> {
        let dir = self.env.get(crate::env::CLAUDE_CONFIG_DIR)?.trim();
        (!dir.is_empty()).then(|| PathBuf::from(expand_home(dir)))
    }

    /// Where a Claude-dialect harness's sessions keep their transcripts:
    /// the dir `env` pins, else the one this process would use itself
    /// ([`crate::paths::claude_config_dir`]). None off the Claude dialect.
    pub fn claude_config_dir(&self) -> Option<PathBuf> {
        if !self.claude_like() {
            return None;
        }
        self.pinned_claude_config_dir()
            .or_else(crate::paths::claude_config_dir)
    }

    /// Whether this harness is a CLAUDE ACCOUNT: built-in Claude — the
    /// default account, in its own config dir or the one its `env` pins —
    /// or a `claude_accounts` entry, Claude's row in a dir of its own.
    pub fn is_claude_account(&self) -> bool {
        self.kind == AgentKind::Claude && self.claude_like()
    }

    /// Where Claude Code records who this account is signed in as
    /// ([`crate::claude_account::Record`]): its pinned dir's, or — for
    /// built-in Claude launched without one — this process's. None for a
    /// harness that is no Claude account.
    pub fn account_record(&self) -> Option<crate::claude_account::Record> {
        if !self.is_claude_account() {
            return None;
        }
        crate::claude_account::Record::of(self.pinned_claude_config_dir().as_deref())
    }

    /// Whether a Claude session can be continued on this harness — **Continue
    /// on**: it is a Claude account ([`Self::is_claude_account`]), resumes a
    /// stored session id, and launches.
    pub fn takes_claude_sessions(&self) -> bool {
        self.is_claude_account() && self.resumes() && self.problem().is_none()
    }

    /// Whether a session moved onto this harness keeps `model`: one it
    /// lists, any id at all when a runtime catalogue (Claude's own model
    /// list) or no list stands behind its model flag.
    pub fn offers_model(&self, model: &str) -> bool {
        self.model.flag.is_some()
            && (self.model.catalog.is_some()
                || self.model.models.is_empty()
                || self.model.models.iter().any(|entry| entry.id == model))
    }

    /// [`Self::offers_model`] for the reasoning effort: a harness that
    /// takes one at all, and lists this one or lists none.
    pub fn offers_effort(&self, effort: &str) -> bool {
        let takes = self.effort.flag.is_some()
            || self.effort.config_key.is_some()
            || self.compose_model_effort;
        self.effort.offered
            && takes
            && (self.effort.efforts.is_empty() || self.effort.efforts.iter().any(|e| e == effort))
    }

    /// Why this descriptor is unusable, or None when it launches. The
    /// picker hides broken entries; create paths refuse them with this.
    pub fn problem(&self) -> Option<String> {
        if self.id.trim().is_empty() {
            return Some("harness with an empty id".into());
        }
        if self.program.trim().is_empty() {
            return Some(format!("harness `{}` has no program", self.id.trim()));
        }
        if self.resume.flag.is_some() && self.resume.subcommand.is_some() {
            return Some(format!(
                "harness `{}` sets both a resume flag and a resume subcommand",
                self.id.trim()
            ));
        }
        if self.resume.cd && self.resume.subcommand.is_none() {
            return Some(format!(
                "harness `{}` passes --cd without a resume subcommand",
                self.id.trim()
            ));
        }
        if self.effort.flag.is_some() && self.effort.config_key.is_some() {
            return Some(format!(
                "harness `{}` sets both an effort flag and an effort config key",
                self.id.trim()
            ));
        }
        if self.effort.config_key.is_some() && self.effort.config_flag.is_none() {
            return Some(format!(
                "harness `{}` sets an effort config key without its flag",
                self.id.trim()
            ));
        }
        if self.system.append_flag.is_some() && self.system.prepend_to_first_prompt {
            return Some(format!(
                "harness `{}` sets both a system-prompt flag and first-prompt prepend",
                self.id.trim()
            ));
        }
        if self.compose_model_effort && self.model.flag.is_none() {
            return Some(format!(
                "harness `{}` composes model-effort ids without a model flag",
                self.id.trim()
            ));
        }
        if let Some(dialect) = self.hooks.as_deref().map(str::trim) {
            if dialect_named(dialect).is_none() {
                let names: Vec<&str> = HOOK_DIALECTS.iter().map(|(name, _)| *name).collect();
                return Some(format!(
                    "harness `{}` hooks `{dialect}`: name a built-in dialect ({})",
                    self.id.trim(),
                    names.join(", ")
                ));
            }
        }
        for name in self.env.keys() {
            if !is_env_name(name) {
                return Some(format!(
                    "harness `{}` env `{name}`: name a variable with letters, digits and underscores, not starting with a digit",
                    self.id.trim()
                ));
            }
            if crate::env::AGENT_SESSION_VARS.contains(&name.as_str()) {
                return Some(format!(
                    "harness `{}` env `{name}`: orion sets that one on every session itself",
                    self.id.trim()
                ));
            }
        }
        None
    }
}

/// `value` with a leading `~/` (or a bare `~`) expanded against `$HOME`;
/// anything else, and every value with no home to expand against, as is.
pub(crate) fn expand_home(value: &str) -> String {
    let rest = match value.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
        _ => return value.to_string(),
    };
    match crate::env::home_dir() {
        Some(home) => format!("{}{rest}", home.display()),
        None => value.to_string(),
    }
}

/// A name a POSIX shell can `export`: letters, digits and underscores,
/// not starting with a digit.
fn is_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn models(ids: &[&str]) -> Vec<ModelEntry> {
    ids.iter()
        .map(|id| ModelEntry {
            id: id.to_string(),
            hint: None,
        })
        .collect()
}

/// The row an id the registry does not name reads as: under its own name,
/// `id` as its program, a `--model` flag and nothing else — no effort, no
/// resume, no hooks. What a session outliving its entry renders with, and
/// what a map-only entry's deltas apply over.
pub fn placeholder(id: &str) -> HarnessDescriptor {
    HarnessDescriptor {
        id: id.trim().to_string(),
        kind: AgentKind::Custom,
        label: String::new(),
        program: id.trim().to_string(),
        env: BTreeMap::new(),
        enabled: true,
        model: ModelSpec::default(),
        effort: EffortSpec::default(),
        permissions_flag: None,
        prompt_flag: None,
        resume: ResumeSpec::default(),
        system: SystemSpec::default(),
        hooks: None,
        relocation_prompt: false,
        compose_model_effort: false,
        mode: ModeSpec::default(),
    }
}

/// The compiled-in behavior of a known harness: what a fresh install
/// runs before any config exists. The Accounts tab, the picker, spawn and
/// hooks all read through here, so a new built-in is a new row plus its
/// exhaustive-match arms — and a new third-party CLI needs no row at all.
pub fn builtin(id: &str) -> Option<HarnessDescriptor> {
    let base = HarnessDescriptor {
        program: String::new(),
        kind: AgentKind::parse(id.trim())?,
        ..placeholder(id)
    };
    Some(match id.trim() {
        "claude" => HarnessDescriptor {
            label: "Claude".into(),
            program: "claude".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                default: default_model_choice(),
                models: models(&["fable", "opus", "sonnet", "haiku"]),
                catalog: Some(HarnessCatalog::Claude),
            },
            effort: EffortSpec {
                flag: Some("--effort".into()),
                default: default_model_choice(),
                efforts: ["low", "medium", "high", "xhigh", "max"]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                offered: true,
                ..EffortSpec::default()
            },
            resume: ResumeSpec {
                flag: Some("--resume".into()),
                ..ResumeSpec::default()
            },
            system: SystemSpec {
                append_flag: Some("--append-system-prompt".into()),
                ..SystemSpec::default()
            },
            hooks: Some("claude".into()),
            relocation_prompt: true,
            // Plan mode is a permission mode; Claude Code has no
            // read-only ask mode to start in.
            mode: ModeSpec {
                flag: Some("--permission-mode".into()),
                plan: Some("plan".into()),
                ask: None,
            },
            ..base
        },
        // Codex's models and efforts as its docs list them on 2026-11-05
        // (learn.chatgpt.com/docs/models: the recommended four, GPT-5.5
        // retired from ChatGPT sign-in on 2026-10-14; config-reference:
        // `model_reasoning_effort` up to `max` and `ultra`).
        "codex" => HarnessDescriptor {
            label: "Codex".into(),
            program: "codex".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                default: default_model_choice(),
                models: models(&["gpt-6-astra", "gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-luna"]),
                catalog: None,
            },
            effort: EffortSpec {
                config_flag: Some("-c".into()),
                config_key: Some("model_reasoning_effort".into()),
                default: default_model_choice(),
                efforts: ["low", "medium", "high", "xhigh", "max", "ultra"]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                offered: true,
                ..EffortSpec::default()
            },
            permissions_flag: Some("--yolo".into()),
            resume: ResumeSpec {
                flag: None,
                subcommand: Some("resume".into()),
                cd: true,
            },
            hooks: Some("codex".into()),
            relocation_prompt: true,
            ..base
        },
        "cursor" => HarnessDescriptor {
            label: "Cursor".into(),
            program: "cursor-agent".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                default: default_model_choice(),
                models: Vec::new(),
                catalog: Some(HarnessCatalog::Cursor),
            },
            effort: EffortSpec {
                default: default_model_choice(),
                offered: true,
                ..EffortSpec::default()
            },
            permissions_flag: Some("--force".into()),
            resume: ResumeSpec {
                flag: Some("--resume".into()),
                ..ResumeSpec::default()
            },
            hooks: Some("cursor".into()),
            compose_model_effort: true,
            mode: ModeSpec {
                flag: Some("--mode".into()),
                plan: Some("plan".into()),
                ask: Some("ask".into()),
            },
            ..base
        },
        // xAI's Grok Build, as docs.x.ai/build documents it (no `grok` was
        // installed to ask): `-m/--model`, `--effort`, `-r/--resume <id>`,
        // `--rules` appending to the system prompt (cli/reference), and
        // personal hooks in `~/.grok/hooks/*.json` posting a camelCase
        // payload with `sessionId` (features/hooks). No permission flag:
        // it keeps its own prompts, which no hook announces.
        "grok" => HarnessDescriptor {
            label: "Grok Build".into(),
            program: "grok".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                ..ModelSpec::default()
            },
            effort: EffortSpec {
                flag: Some("--effort".into()),
                offered: true,
                ..EffortSpec::default()
            },
            resume: ResumeSpec {
                flag: Some("--resume".into()),
                ..ResumeSpec::default()
            },
            system: SystemSpec {
                append_flag: Some("--rules".into()),
                ..SystemSpec::default()
            },
            hooks: Some("grok".into()),
            ..base
        },
        // OpenCode's model is a `provider/model` id (`opencode models`
        // lists what this machine has credentials for, out of models.dev's
        // catalogue; a hand-edited id passes verbatim). The list is its own
        // free model, then the newest Claude and GPT ids models.dev had on
        // 2026-11-05. No effort flag: reasoning is a per-model variant
        // chosen inside its own TUI. No permission flag either —
        // its prompts drive NEEDS FEEDBACK through the managed plugin. The
        // positional is the project path, so the first prompt rides
        // `--prompt`; a resume's `--prompt` is not submitted (verified on
        // opencode 1.18.32), so no relocation notice.
        "opencode" => HarnessDescriptor {
            label: "OpenCode".into(),
            program: "opencode".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                default: default_model_choice(),
                models: models(&[
                    "opencode/big-pickle",
                    "anthropic/claude-fable-5-1",
                    "anthropic/claude-opus-5-5",
                    "anthropic/claude-sonnet-5-5",
                    "openai/gpt-6.1-sol",
                ]),
                catalog: None,
            },
            prompt_flag: Some("--prompt".into()),
            resume: ResumeSpec {
                flag: Some("--session".into()),
                ..ResumeSpec::default()
            },
            hooks: Some("opencode".into()),
            ..base
        },
        _ => return None,
    })
}

/// Every known harness, in [`AgentKind::ALL`] order.
pub fn builtins() -> Vec<HarnessDescriptor> {
    AgentKind::ALL
        .into_iter()
        .filter(|&kind| kind != AgentKind::Custom)
        .map(|kind| builtin(kind.as_str()).expect("every built-in AgentKind has a descriptor row"))
        .collect()
}

/// One delta to a harness row: absent leaves the row alone, `null` clears
/// a nullable row, a value replaces it. Plain `Option` cannot tell absent
/// from `null` (serde reads both as `None`), so nullable rows use this.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Clearable<T> {
    /// The key is absent: keep the row's value.
    #[default]
    Keep,
    /// The key is `null`: clear the row.
    Clear,
    /// The key names a value: use it.
    Set(T),
}

impl<T> Clearable<T> {
    fn is_keep(value: &Clearable<T>) -> bool {
        matches!(value, Clearable::Keep)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Clearable<T> {
    fn deserialize<D: serde::Deserializer<'de>>(into: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(into).map(|opt| match opt {
            None => Clearable::Clear,
            Some(value) => Clearable::Set(value),
        })
    }
}

impl<T: Serialize> Serialize for Clearable<T> {
    fn serialize<S: serde::Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        match self {
            Clearable::Keep => out.serialize_none(),
            Clearable::Clear => out.serialize_none(),
            Clearable::Set(value) => value.serialize(out),
        }
    }
}

/// One entry of the config file's `harnesses` map: what a harness — a
/// built-in, a Claude account, or a CLI added by the name of its program
/// — has of the user's own. Every field is optional; the merge applies
/// what is set. How the CLI is driven is never here: its flags, resume
/// shape and hook dialect are the compiled-in row's. A field an older
/// release read (`program`, `hooks`, `resume_flag`, …) is kept in
/// `retired`, read by nothing and written back as it was.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessOverride {
    /// Whether the pickers and presets offer it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The name it goes by — a Claude account's, as the user typed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The default model new sessions launch with; `default` passes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_default: Option<String>,
    /// The default effort new sessions launch with; `default` passes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_default: Option<String>,
    /// The model list the pickers offer, in place of the row's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ModelEntry>>,
    /// The CLI's own environment, whole: a map replaces the row's, `null`
    /// clears it.
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub env: Clearable<BTreeMap<String, String>>,
    /// What an older release read here and this one does not.
    #[serde(flatten)]
    pub retired: BTreeMap<String, serde_json::Value>,
}

impl HarnessDescriptor {
    /// Apply the user's deltas in place: each set field replaces the
    /// row's, and a `null` `env` clears it.
    pub fn apply(&mut self, over: &HarnessOverride) {
        if let Some(enabled) = over.enabled {
            self.enabled = enabled;
        }
        if let Some(label) = over.label.as_deref() {
            self.label = label.to_string();
        }
        if let Some(default) = over.model_default.as_deref() {
            self.model.default = default.to_string();
        }
        if let Some(default) = over.effort_default.as_deref() {
            self.effort.default = default.to_string();
        }
        if let Some(models) = over.models.clone() {
            self.model.models = models;
        }
        match &over.env {
            Clearable::Keep => {}
            Clearable::Clear => self.env.clear(),
            Clearable::Set(env) => self.env = env.clone(),
        }
    }
}

/// The effective registry both halves of orion read: the built-ins in
/// [`AgentKind::ALL`] order with the `harnesses` map applied — Claude's
/// extra accounts right after it — then map-only new ids in key order,
/// each a CLI by the name of its program ([`placeholder`]). Callers hide
/// entries that are disabled or [`problem`][HarnessDescriptor::problem]-broken;
/// see [`usable`].
///
/// An account is Claude's effective row in its own config dir
/// ([`ClaudeAccount::descriptor`]), and the map's entry for its id is
/// deltas over that, as for a built-in. An account whose entry is broken,
/// or whose id an earlier account already holds, is left out (the TUI's
/// Accounts tab names it and why).
pub fn registry(
    overrides: &BTreeMap<String, HarnessOverride>,
    accounts: &[ClaudeAccount],
) -> Vec<HarnessDescriptor> {
    let mut out: Vec<HarnessDescriptor> = builtins()
        .into_iter()
        .map(|mut descriptor| {
            if let Some(over) = overrides.get(&descriptor.id) {
                descriptor.apply(over);
            }
            descriptor
        })
        .collect();
    let accounts = registered_accounts(accounts);
    if let Some(at) = out.iter().position(|entry| entry.id == "claude") {
        let claude = out[at].clone();
        let rows = accounts.iter().map(|account| {
            let mut descriptor = account.descriptor(&claude);
            if let Some(over) = overrides.get(&descriptor.id) {
                descriptor.apply(over);
            }
            descriptor
        });
        out.splice(at + 1..at + 1, rows.collect::<Vec<_>>());
    }
    let mut extra: Vec<(&String, &HarnessOverride)> = overrides
        .iter()
        .filter(|(id, _)| {
            builtin(id).is_none()
                && !accounts
                    .iter()
                    .any(|account| account.id.trim() == id.as_str())
        })
        .collect();
    extra.sort_by_key(|(id, _)| *id);
    // A map-only entry is a CLI by the name of its program: `--model`,
    // a trailing prompt, and status from the process.
    for (id, over) in extra {
        let mut descriptor = placeholder(id);
        descriptor.apply(over);
        out.push(descriptor);
    }
    out
}

/// The ids of the `claude_accounts` entries the registry carries: what a
/// session on one of them stores beside `AgentKind::Claude`.
pub fn account_ids(accounts: &[ClaudeAccount]) -> Vec<String> {
    registered_accounts(accounts)
        .into_iter()
        .map(|account| account.id.trim().to_string())
        .collect()
}

/// The `claude_accounts` entries the registry carries: valid ones, each id
/// once.
fn registered_accounts(accounts: &[ClaudeAccount]) -> Vec<&ClaudeAccount> {
    let mut kept: Vec<&ClaudeAccount> = Vec::new();
    for account in accounts {
        let id = account.id.trim();
        if account.problem().is_none() && !kept.iter().any(|other| other.id.trim() == id) {
            kept.push(account);
        }
    }
    kept
}

/// Every entry the picker and presets offer: enabled and valid, in
/// registry order. Invalid entries are left out (create paths refuse them
/// with their reason); the Accounts tab still lists them so they can be
/// fixed.
pub fn usable(all: &[HarnessDescriptor]) -> Vec<&HarnessDescriptor> {
    all.iter()
        .filter(|entry| entry.enabled && entry.problem().is_none())
        .collect()
}

/// Every harness a Claude session on `from` (a registry id) can be
/// continued on — **Continue on** another account: enabled, not
/// `from` itself, and [`HarnessDescriptor::takes_claude_sessions`], in
/// registry order. What the TUI offers; the DAEMON asks
/// `takes_claude_sessions` alone, since `enabled` gates the pickers, never
/// a request.
pub fn continue_targets<'a>(
    all: &'a [HarnessDescriptor],
    from: &str,
) -> Vec<&'a HarnessDescriptor> {
    all.iter()
        .filter(|entry| entry.enabled && entry.id != from && entry.takes_claude_sessions())
        .collect()
}

/// The registry id a session's `(kind, custom_harness)` names: the
/// Claude account's id for a Claude row that carries one, the entry's id
/// for a custom harness — empty while it has none — and the kind's own
/// name for everything else. The inverse of
/// [`HarnessDescriptor::session_kind`].
pub fn registry_id(kind: AgentKind, custom: Option<&str>) -> &str {
    let custom = custom.map(str::trim).filter(|id| !id.is_empty());
    match (kind, custom) {
        (AgentKind::Claude | AgentKind::Custom, Some(id)) => id,
        (AgentKind::Custom, None) => "",
        _ => kind.as_str(),
    }
}

/// Resolve a session's harness to its effective descriptor: built-ins by
/// kind, a Claude account and a custom harness by the id beside it
/// ([`registry_id`]). `Err` names why the launch is refused — an unknown
/// id, a Claude row on an id that is no Claude account, or the entry's
/// [`problem`][HarnessDescriptor::problem]. A custom row naming a Claude
/// account still finds it: a row an older release stored before its
/// account was set up again.
pub fn resolve<'a>(
    all: &'a [HarnessDescriptor],
    kind: AgentKind,
    custom: Option<&str>,
) -> Result<&'a HarnessDescriptor, String> {
    let id = registry_id(kind, custom);
    let found = all.iter().find(|entry| entry.id == id);
    let Some(descriptor) = found.filter(|entry| kind != AgentKind::Claude || entry.kind == kind)
    else {
        return Err(match kind {
            AgentKind::Custom => format!("custom harness `{id}` is no longer defined"),
            AgentKind::Claude if id != kind.as_str() => {
                format!("Claude account `{id}` is no longer set up (Settings → Accounts)")
            }
            _ => format!("harness `{id}` is not in the registry"),
        });
    };
    if let Some(problem) = descriptor.problem() {
        return Err(problem);
    }
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map-only entry: a CLI by the name of its program, its id.
    fn cli() -> HarnessOverride {
        HarnessOverride::default()
    }

    #[test]
    fn builtins_cover_every_builtin_kind() {
        let all = builtins();
        assert_eq!(all.len(), 5);
        for kind in AgentKind::ALL {
            if kind == AgentKind::Custom {
                continue;
            }
            let descriptor = builtin(kind.as_str()).expect("a row per built-in");
            assert_eq!(descriptor.id, kind.as_str());
            assert!(descriptor.enabled);
            assert_eq!(descriptor.problem(), None, "{kind:?} ships valid");
        }
        // Spot-check the shapes spawn relies on.
        let claude = builtin("claude").unwrap();
        assert_eq!(claude.program, "claude");
        assert_eq!(claude.model.flag.as_deref(), Some("--model"));
        assert_eq!(claude.effort.flag.as_deref(), Some("--effort"));
        assert_eq!(claude.resume.flag.as_deref(), Some("--resume"));
        assert!(claude.relocation_prompt);
        let codex = builtin("codex").unwrap();
        assert_eq!(codex.permissions_flag.as_deref(), Some("--yolo"));
        assert_eq!(codex.resume.subcommand.as_deref(), Some("resume"));
        assert!(codex.resume.cd);
        assert_eq!(
            codex.effort.config_key.as_deref(),
            Some("model_reasoning_effort")
        );
        let cursor = builtin("cursor").unwrap();
        assert_eq!(cursor.program, "cursor-agent");
        assert!(cursor.compose_model_effort);
        assert!(!cursor.relocation_prompt);
        assert_eq!(builtin("pi"), None, "Pi is no built-in any more");
        assert_eq!(builtin("muse"), None, "nor is Muse");
        let opencode = builtin("opencode").unwrap();
        assert_eq!(opencode.program, "opencode");
        assert_eq!(opencode.prompt_flag.as_deref(), Some("--prompt"));
        assert_eq!(opencode.resume.flag.as_deref(), Some("--session"));
        assert_eq!(opencode.hook_dialect(), Some(AgentKind::OpenCode));
        assert_eq!(opencode.permissions_flag, None, "its prompts drive red");
        assert!(!opencode.effort.offered, "no effort flag, no Effort row");
        assert!(!opencode.relocation_prompt);
        assert!(!opencode.claude_like());
        let grok = builtin("grok").unwrap();
        assert_eq!(grok.effort.flag.as_deref(), Some("--effort"));
        assert_eq!(grok.resume.flag.as_deref(), Some("--resume"));
        assert_eq!(grok.system_append_flag(), Some("--rules"));
        assert_eq!(grok.hook_dialect(), Some(AgentKind::Grok));
        assert_eq!(grok.permissions_flag, None, "it keeps its own prompts");
        assert!(!grok.claude_like());
    }

    /// The static lists are what the CLIs offer today: Codex's four
    /// recommended models and its efforts up to `ultra` (`minimal` and
    /// the GPT-5.5 / 5.6 ids gone), OpenCode's `provider/model` ids
    /// on the newest Claude and GPT generations.
    #[test]
    fn builtin_model_lists_are_the_clis_current_ones() {
        let ids = |d: &HarnessDescriptor| -> Vec<String> {
            d.model.models.iter().map(|m| m.id.clone()).collect()
        };
        let codex = builtin("codex").unwrap();
        assert_eq!(
            ids(&codex),
            ["gpt-6-astra", "gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-luna"]
        );
        assert_eq!(
            codex.effort.efforts,
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert!(codex.offers_model("gpt-6.1-sol"));
        assert!(!codex.offers_model("gpt-5.5"), "retired");
        assert!(codex.offers_effort("ultra"));
        assert!(!codex.offers_effort("minimal"));

        let opencode = builtin("opencode").unwrap();
        assert_eq!(
            ids(&opencode),
            [
                "opencode/big-pickle",
                "anthropic/claude-fable-5-1",
                "anthropic/claude-opus-5-5",
                "anthropic/claude-sonnet-5-5",
                "openai/gpt-6.1-sol",
            ]
        );
        assert!(ids(&opencode).iter().all(|id| id.contains('/')));
    }

    /// The picker's note: none where every status can show; a hookless
    /// CLI says it is the process alone; a CLI that asks on a dialect with
    /// no waiting signal says red is out of reach — unless a
    /// skip-permissions flag leaves it nothing to ask.
    #[test]
    fn status_notes_say_what_a_row_cannot_show() {
        for id in ["claude", "codex", "cursor", "opencode"] {
            assert_eq!(builtin(id).unwrap().status_note(), None, "{id}");
        }
        let grok = builtin("grok").unwrap();
        assert_eq!(grok.status_note(), Some(NO_WAITING_NOTE));
        let yolo = HarnessDescriptor {
            permissions_flag: Some("--always-approve".into()),
            ..grok.clone()
        };
        assert_eq!(yolo.status_note(), None);
        let asking_cursor = HarnessDescriptor {
            permissions_flag: None,
            ..builtin("cursor").unwrap()
        };
        assert_eq!(asking_cursor.status_note(), Some(NO_WAITING_NOTE));
        assert_eq!(placeholder("agy").status_note(), Some(PROCESS_STATUS_NOTE));
        let hookless = HarnessDescriptor {
            hooks: None,
            ..grok
        };
        assert_eq!(hookless.status_note(), Some(PROCESS_STATUS_NOTE));
    }

    /// Every named dialect resolves, `grok` among them, and an unknown one
    /// is refused with the list.
    #[test]
    fn hooks_rows_name_a_built_in_dialect() {
        for (name, kind) in HOOK_DIALECTS {
            let row = HarnessDescriptor {
                hooks: Some(format!(" {name} ")),
                ..placeholder("agy")
            };
            assert_eq!(row.hook_dialect(), Some(kind), "{name}");
            assert_eq!(row.problem(), None, "{name}");
        }
        let row = HarnessDescriptor {
            hooks: Some("pi".into()),
            ..placeholder("agy")
        };
        assert_eq!(row.hook_dialect(), None);
        assert_eq!(
            row.problem().as_deref(),
            Some(
                "harness `agy` hooks `pi`: name a built-in dialect (claude, codex, cursor, grok, opencode)"
            )
        );
    }

    /// An entry is what a harness has of the user's own — its switch, its
    /// name, its default model and effort, its model list, its
    /// environment — and nothing that drives the CLI: a field an older
    /// release read (`program`, `hooks`, a flag) is kept, unread, and
    /// written back as it was.
    #[test]
    fn overrides_hold_the_users_own_and_keep_what_they_no_longer_read() {
        let written = serde_json::json!({
            "enabled": false,
            "label": "Personal",
            "model_default": "opus",
            "effort_default": "max",
            "models": [{"id": "opus"}],
            "env": {"ANTHROPIC_LOG": "debug"},
            "program": "/opt/claude",
            "hooks": null,
            "resume_flag": "--continue",
        });
        let over: HarnessOverride = serde_json::from_value(written.clone()).unwrap();
        let mut claude = builtin("claude").unwrap();
        claude.apply(&over);
        assert!(!claude.enabled);
        assert_eq!(claude.label, "Personal");
        assert_eq!(claude.default_model(), Some("opus"));
        assert_eq!(claude.default_effort(), Some("max"));
        assert_eq!(claude.model.models.len(), 1);
        assert_eq!(claude.env["ANTHROPIC_LOG"], "debug");
        // How Claude is driven stays Claude's.
        assert_eq!(claude.program, "claude");
        assert_eq!(claude.hooks.as_deref(), Some("claude"));
        assert_eq!(claude.resume.flag.as_deref(), Some("--resume"));
        assert!(claude.claude_like());
        assert_eq!(
            over.retired.keys().collect::<Vec<_>>(),
            ["hooks", "program", "resume_flag"]
        );
        assert_eq!(
            serde_json::to_value(&over).unwrap(),
            written,
            "kept as written"
        );
    }

    #[test]
    fn registry_orders_builtins_then_new_ids() {
        let mut overrides = BTreeMap::new();
        overrides.insert("zed".into(), cli());
        overrides.insert("agy".into(), cli());
        overrides.insert(
            "claude".into(),
            HarnessOverride {
                enabled: Some(false),
                ..HarnessOverride::default()
            },
        );
        let all = registry(&overrides, &[]);
        let ids: Vec<&str> = all.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["claude", "codex", "cursor", "grok", "opencode", "agy", "zed"]
        );
        assert!(!all[0].enabled, "the claude override applied");
        assert_eq!(all[5].program, "agy", "a map-only entry runs its id");
        assert!(!all[5].effort.offered, "a bare program stays Model-only");
        assert_eq!(all[5].hook_dialect(), None, "status from the process");
        assert_eq!(all[5].problem(), None);
    }

    #[test]
    fn conflicting_shapes_are_problems() {
        let mut descriptor = builtin("codex").unwrap();
        descriptor.resume.flag = Some("--resume".into());
        assert!(descriptor.problem().is_some(), "flag plus subcommand");
        let mut descriptor = builtin("claude").unwrap();
        descriptor.system.prepend_to_first_prompt = true;
        assert!(descriptor.problem().is_some(), "flag plus prepend");
        let mut descriptor = builtin("claude").unwrap();
        descriptor.hooks = Some("tmux".into());
        assert!(descriptor.problem().is_some(), "unknown dialect");
        let mut descriptor = builtin("codex").unwrap();
        descriptor.effort.config_flag = None;
        assert!(
            descriptor.problem().is_some(),
            "config key without its flag"
        );
        let empty_program = HarnessDescriptor {
            program: String::new(),
            ..builtin("claude").unwrap()
        };
        assert!(empty_program.problem().is_some());
    }

    #[test]
    fn resolve_names_missing_and_broken_entries() {
        let all = registry(&BTreeMap::from([("agy".into(), cli())]), &[]);
        assert_eq!(resolve(&all, AgentKind::Claude, None).unwrap().id, "claude");
        assert_eq!(
            resolve(&all, AgentKind::Custom, Some("agy"))
                .unwrap()
                .program,
            "agy"
        );
        assert!(resolve(&all, AgentKind::Custom, Some("gone")).is_err());
        assert!(resolve(&all, AgentKind::Custom, None).is_err());
        let broken = HarnessOverride {
            env: Clearable::Set(BTreeMap::from([("MY-VAR".into(), "x".into())])),
            ..HarnessOverride::default()
        };
        let all = registry(&BTreeMap::from([("broken".into(), broken)]), &[]);
        assert!(resolve(&all, AgentKind::Custom, Some("broken")).is_err());
        assert_eq!(usable(&all).len(), 5, "the broken entry is hidden");
    }

    /// A Claude account's sessions are Claude's, the account's id beside
    /// the kind; a custom harness's are Custom with its id; a built-in's
    /// carry its kind alone — and every pair resolves back to its row. A
    /// Claude row on an id that is no account is refused rather than
    /// launched as whatever holds the id, and a custom row an older
    /// release stored for an account still finds it.
    #[test]
    fn a_claude_account_is_claude_with_its_id_beside() {
        let accounts = [ClaudeAccount {
            id: "claude-2".into(),
            config_dir: "~/.claude-2".into(),
            name: String::new(),
            enabled: true,
        }];
        let all = registry(&BTreeMap::from([("agy".into(), cli())]), &accounts);
        let kinds: Vec<(AgentKind, Option<String>)> =
            all.iter().map(HarnessDescriptor::session_kind).collect();
        assert_eq!(
            kinds,
            [
                (AgentKind::Claude, None),
                (AgentKind::Claude, Some("claude-2".into())),
                (AgentKind::Codex, None),
                (AgentKind::Cursor, None),
                (AgentKind::Grok, None),
                (AgentKind::OpenCode, None),
                (AgentKind::Custom, Some("agy".into())),
            ]
        );
        for entry in &all {
            let (kind, custom) = entry.session_kind();
            assert_eq!(registry_id(kind, custom.as_deref()), entry.id);
            assert_eq!(resolve(&all, kind, custom.as_deref()).unwrap().id, entry.id);
        }
        assert_eq!(registry_id(AgentKind::Claude, Some("  ")), "claude");
        assert_eq!(registry_id(AgentKind::Codex, Some("x")), "codex");
        assert_eq!(registry_id(AgentKind::Custom, None), "");

        let err = resolve(&all, AgentKind::Claude, Some("agy")).unwrap_err();
        assert!(err.contains("Claude account `agy`"), "{err}");
        let err = resolve(&all, AgentKind::Claude, Some("claude-9")).unwrap_err();
        assert!(err.contains("no longer set up"), "{err}");
        assert_eq!(
            resolve(&all, AgentKind::Custom, Some("claude-2"))
                .unwrap()
                .id,
            "claude-2"
        );
    }

    /// `env` merges over the row like any other field, `null` clears it.
    /// A map-only entry that pins `CLAUDE_CONFIG_DIR` is no Claude account:
    /// it runs its own program, with no hooks to speak Claude's dialect.
    #[test]
    fn env_rides_the_override_and_null_clears_it() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "claude-b".to_string(),
            serde_json::from_value::<HarnessOverride>(serde_json::json!({
                "label": "Claude B",
                "env": {"CLAUDE_CONFIG_DIR": "~/.claude-b"},
            }))
            .unwrap(),
        );
        let all = registry(&overrides, &[]);
        let b = all.iter().find(|entry| entry.id == "claude-b").unwrap();
        assert_eq!(b.problem(), None);
        assert_eq!(b.env["CLAUDE_CONFIG_DIR"], "~/.claude-b");
        assert!(!b.is_claude_account() && !b.takes_claude_sessions());
        assert_eq!(
            b.session_kind(),
            (AgentKind::Custom, Some("claude-b".into()))
        );

        let mut cleared = b.clone();
        cleared.apply(&serde_json::from_value(serde_json::json!({"env": null})).unwrap());
        assert!(cleared.env.is_empty());
        let mut replaced = b.clone();
        replaced.apply(&serde_json::from_value(serde_json::json!({"env": {"FOO": "1"}})).unwrap());
        assert_eq!(replaced.env.keys().collect::<Vec<_>>(), ["FOO"]);
    }

    #[test]
    fn env_names_a_shell_cannot_export_or_orion_owns_are_problems() {
        for name in [
            "",
            "1ABC",
            "MY-VAR",
            "A B",
            "ORION_AGENT_ID",
            "ORION_API_TOKEN",
        ] {
            let mut descriptor = builtin("claude").unwrap();
            descriptor.env.insert(name.into(), "x".into());
            assert!(descriptor.problem().is_some(), "{name:?}");
        }
        let mut descriptor = builtin("claude").unwrap();
        descriptor.env.insert("_MY_VAR2".into(), "x".into());
        assert_eq!(descriptor.problem(), None);
    }

    #[test]
    fn launch_env_expands_a_leading_home() {
        let home = crate::env::home_dir().unwrap();
        let mut descriptor = builtin("claude").unwrap();
        for (name, value) in [
            ("CLAUDE_CONFIG_DIR", "~/.claude-b"),
            ("BARE", "~"),
            ("PLAIN", "/opt/x"),
            ("TILDE_USER", "~other/x"),
            ("MIDDLE", "a~/b"),
        ] {
            descriptor.env.insert(name.into(), value.into());
        }
        let env: BTreeMap<String, String> = descriptor.launch_env().into_iter().collect();
        assert_eq!(
            env["CLAUDE_CONFIG_DIR"],
            format!("{}/.claude-b", home.display())
        );
        assert_eq!(env["BARE"], home.display().to_string());
        assert_eq!(env["PLAIN"], "/opt/x");
        assert_eq!(env["TILDE_USER"], "~other/x", "only the user's own home");
        assert_eq!(env["MIDDLE"], "a~/b");
        assert_eq!(
            descriptor.pinned_claude_config_dir(),
            Some(home.join(".claude-b"))
        );
        assert_eq!(descriptor.claude_config_dir(), Some(home.join(".claude-b")));
        // Unpinned, a Claude row's dir is this process's; off the dialect,
        // there is none.
        let claude = builtin("claude").unwrap();
        assert_eq!(claude.pinned_claude_config_dir(), None);
        assert_eq!(
            claude.claude_config_dir(),
            crate::paths::claude_config_dir()
        );
        assert_eq!(builtin("codex").unwrap().claude_config_dir(), None);
    }

    /// **Continue on**: every other enabled Claude account — built-in
    /// Claude and each `claude_accounts` entry. A switched-off one is not
    /// offered, nor is a map-only CLI whose `env` pins a Claude config dir:
    /// it speaks no Claude dialect.
    #[test]
    fn continue_targets_are_the_other_claude_accounts() {
        let account = |id: &str, enabled: bool| ClaudeAccount {
            id: id.into(),
            config_dir: format!("~/.{id}"),
            name: String::new(),
            enabled,
        };
        let wrapper = HarnessOverride {
            env: Clearable::Set(BTreeMap::from([(
                "CLAUDE_CONFIG_DIR".into(),
                "~/.claude-w".into(),
            )])),
            ..HarnessOverride::default()
        };
        let all = registry(
            &BTreeMap::from([("wrapper".into(), wrapper)]),
            &[account("claude-b", true), account("claude-c", false)],
        );
        let ids = |from: &str| -> Vec<String> {
            continue_targets(&all, from)
                .into_iter()
                .map(|entry| entry.id.clone())
                .collect()
        };
        assert_eq!(ids("claude"), ["claude-b"]);
        assert_eq!(ids("claude-b"), ["claude"]);
        assert_eq!(ids("wrapper"), ["claude", "claude-b"]);
    }

    /// `claude_accounts` entries are Claude's effective row in their own
    /// dirs, listed right after it; the map's entry for an account's id is
    /// deltas over that row, never a second harness; and an entry that
    /// can't be an account — broken, a built-in's id, or an id already
    /// taken — stays out.
    #[test]
    fn claude_accounts_are_claude_in_their_own_dirs() {
        let account = |id: &str, dir: &str| ClaudeAccount {
            id: id.into(),
            config_dir: dir.into(),
            name: String::new(),
            enabled: true,
        };
        let overrides: BTreeMap<String, HarnessOverride> = [
            (
                "claude".to_string(),
                serde_json::from_value(serde_json::json!({
                    "env": {"ANTHROPIC_LOG": "debug"},
                    "enabled": false,
                    "label": "Work",
                    "model_default": "sonnet",
                }))
                .unwrap(),
            ),
            (
                "claude-2".to_string(),
                serde_json::from_value(serde_json::json!({"model_default": "opus"})).unwrap(),
            ),
            ("agy".to_string(), cli()),
        ]
        .into_iter()
        .collect();
        let accounts = [
            account("claude-2", "~/.claude-2"),
            account("claude-2", "~/.claude-again"),
            account("codex", "~/.claude-codex"),
            account("Bad Id", "~/.claude-bad"),
            account("claude-3", "/srv/claude-3"),
        ];
        let all = registry(&overrides, &accounts);
        let ids: Vec<&str> = all.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            ["claude", "claude-2", "claude-3", "codex", "cursor", "grok", "opencode", "agy"]
        );
        let two = &all[1];
        assert_eq!(
            two.env["ANTHROPIC_LOG"], "debug",
            "Claude's row, overrides and all"
        );
        assert!(two.enabled, "the account's own switch, not Claude's");
        assert_eq!(two.label, "", "unnamed: never Claude's label");
        assert_eq!(
            two.env["CLAUDE_CONFIG_DIR"], "~/.claude-2",
            "the first entry wins"
        );
        assert_eq!(two.default_model(), Some("opus"), "its map entry applied");
        assert_eq!(all[2].default_model(), None, "never Claude's own model");
        assert_eq!(all[2].env["CLAUDE_CONFIG_DIR"], "/srv/claude-3");
        assert_eq!(
            all.last().unwrap().program,
            "agy",
            "the map-only entry kept its id"
        );
        assert!(all.iter().all(|entry| entry.problem().is_none()));
        assert!(two.is_claude_account() && two.takes_claude_sessions());
        assert_eq!(
            two.account_record().unwrap().file,
            crate::env::home_dir()
                .unwrap()
                .join(".claude-2")
                .join(".claude.json")
        );
        assert!(!all.last().unwrap().is_claude_account());
        let ids = |from: &str| -> Vec<String> {
            let mut enabled = all.clone();
            enabled[0].enabled = true;
            continue_targets(&enabled, from)
                .into_iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("claude"), ["claude-2", "claude-3"]);
        assert_eq!(ids("claude-3"), ["claude", "claude-2"]);
    }

    #[test]
    fn a_moved_session_keeps_the_model_and_effort_its_target_offers() {
        let claude = builtin("claude").unwrap();
        assert!(claude.offers_model("opus"));
        assert!(
            claude.offers_model("claude-opus-5[1m]"),
            "the catalogue takes any id"
        );
        assert!(claude.offers_effort("high"));
        assert!(!claude.offers_effort("minimal"));
        let mut listed = claude.clone();
        listed.model.catalog = None;
        assert!(listed.offers_model("sonnet"));
        assert!(!listed.offers_model("claude-opus-5[1m]"));
        listed.model.flag = None;
        assert!(!listed.offers_model("sonnet"), "no flag, no model");
        let bare = HarnessDescriptor {
            hooks: Some("claude".into()),
            ..placeholder("agy")
        };
        assert!(
            bare.offers_model("opus"),
            "an empty list passes ids verbatim"
        );
        assert!(!bare.offers_effort("high"), "no Effort row, no effort");
    }

    /// Claude starts in plan through its permission mode and has no ask;
    /// Cursor has both through `--mode`; a harness with neither offers
    /// edit alone, and edit is never a flag.
    #[test]
    fn modes_map_to_each_clis_own_flag() {
        let claude = builtin("claude").unwrap().mode;
        assert_eq!(claude.offered(), [AgentMode::Edit, AgentMode::Plan]);
        assert_eq!(
            claude.args(AgentMode::Plan),
            Some(["--permission-mode", "plan"])
        );
        assert_eq!(claude.args(AgentMode::Ask), None);
        assert_eq!(claude.args(AgentMode::Edit), None);

        let cursor = builtin("cursor").unwrap().mode;
        assert_eq!(cursor.offered(), AgentMode::ALL);
        assert_eq!(cursor.args(AgentMode::Ask), Some(["--mode", "ask"]));

        for id in ["codex", "grok", "opencode"] {
            assert_eq!(
                builtin(id).unwrap().mode.offered(),
                [AgentMode::Edit],
                "{id} has no mode to start in"
            );
        }
    }
}
