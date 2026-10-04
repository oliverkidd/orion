//! The harness registry: one behavior descriptor per agent CLI.
//!
//! The seven known harnesses ship as a compiled-in table ([`builtin`]);
//! the user's config adds a `harnesses` map of [`HarnessOverride`]s over
//! it — disable one, repoint a program, rename a flag, or define a whole
//! new CLI — and [`registry`] merges the two (plus the legacy
//! [`CustomHarness`] list and the `claude_accounts` list, each account a
//! copy of Claude's row in its own config dir) into the effective
//! [`HarnessDescriptor`]s every surface reads: the `n` picker, the `e`
//! presets, spawn and resume, hooks, and the Agents tab. Adding a CLI is a config edit; the verification is
//! that every behavior below is data, with the four genuinely bespoke arg
//! shapings (Cursor's composed model id, Codex's `-c` config pair and its
//! positional resume, OpenCode's `--prompt` flag, Cloud's `--cloud=`)
//! carried as descriptor fields.
//!
//! [`AgentKind`](crate::AgentKind) stays the identity — built-ins by
//! variant, customs by id beside `AgentKind::Custom` — so exhaustive
//! matches still fail to compile when a built-in is added. Behavior never
//! branches on it directly anymore; it resolves to a descriptor first.

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
/// Agents tab edits, and the static list the pickers offer (before the
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

/// How a harness takes its reasoning effort: a plain flag (`--effort`,
/// `--thinking`), Codex's `-c model_reasoning_effort=` config pair, or
/// nothing (composed into the model id, or unsupported).
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
    /// tab's Effort row and the effort submenus show only while set. Muse
    /// keeps a reserved row (stored, never sent); legacy customs have none.
    #[serde(default)]
    pub offered: bool,
}

/// How a harness resumes a stored CLI session id: a flag (`--resume`,
/// `--session-id`), Codex's positional subcommand (`resume <id>`, with
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

/// The effective behavior of one harness: what the picker offers, what
/// spawn builds, which hooks install, and what the Agents tab edits.
/// Built-ins come from [`builtin`]; user entries merge over them (or over
/// the legacy-custom shape for new ids) through [`HarnessOverride`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessDescriptor {
    /// Stable id: the builtin name or the custom entry id.
    pub id: String,
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
    /// harness speaks (`claude`, `codex`, `cursor`, `pi`, `opencode`). Sessions report
    /// status, prompts and permission waits like that harness; without one
    /// they stay process-based. A Claude-compatible CLI gets title sync
    /// with `claude`.
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
}

fn is_false(value: &bool) -> bool {
    !value
}

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
        match self.hooks.as_deref().map(str::trim) {
            Some("claude") => Some(crate::AgentKind::Claude),
            Some("codex") => Some(crate::AgentKind::Codex),
            Some("cursor") => Some(crate::AgentKind::Cursor),
            Some("pi") => Some(crate::AgentKind::Pi),
            Some("opencode") => Some(crate::AgentKind::OpenCode),
            _ => None,
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
    /// of: the spawn restates it after the user's profile. A `program`
    /// wrapper that exports the variable itself is invisible here.
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

    /// Whether this harness is a CLAUDE ACCOUNT orion can see into: it
    /// speaks Claude's dialect and its config dir is known — built-in
    /// Claude's own, or one `env` pins (a `claude_accounts` entry, or a
    /// hand-written `harnesses` entry). A `program` wrapper that exports
    /// `CLAUDE_CONFIG_DIR` itself still launches, but is not one: nothing
    /// says which dir, or which login, it runs under.
    pub fn is_claude_account(&self) -> bool {
        self.claude_like()
            && (AgentKind::parse(&self.id) == Some(AgentKind::Claude)
                || self.pinned_claude_config_dir().is_some())
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
    /// on**: it is a Claude account orion can see into
    /// ([`Self::is_claude_account`]), resumes a stored session id, and
    /// launches. A wrapper script that sets `CLAUDE_CONFIG_DIR` itself keeps
    /// launching, but nothing could be copied to where it would look.
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
            match dialect {
                "claude" | "codex" | "cursor" | "pi" | "opencode" => {}
                _ => {
                    return Some(format!(
                        "harness `{}` hooks `{dialect}`: name a built-in dialect (claude, codex, cursor, pi, opencode)",
                        self.id.trim()
                    ));
                }
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

/// The compiled-in behavior of a known harness: what a fresh install
/// runs before any config exists. The Agents tab, the picker, spawn and
/// hooks all read through here, so a new built-in is a new row plus its
/// exhaustive-match arms — and a new third-party CLI needs no row at all.
pub fn builtin(id: &str) -> Option<HarnessDescriptor> {
    let base = HarnessDescriptor {
        id: id.trim().to_string(),
        label: String::new(),
        program: String::new(),
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
            ..base
        },
        "codex" => HarnessDescriptor {
            label: "Codex".into(),
            program: "codex".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                default: default_model_choice(),
                models: models(&["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.5"]),
                catalog: None,
            },
            effort: EffortSpec {
                config_flag: Some("-c".into()),
                config_key: Some("model_reasoning_effort".into()),
                default: default_model_choice(),
                efforts: ["minimal", "low", "medium", "high", "xhigh"]
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
            ..base
        },
        "pi" => HarnessDescriptor {
            label: "Pi".into(),
            program: "pi".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                default: default_model_choice(),
                models: models(&["opus", "sonnet", "haiku", "gpt-5.5"]),
                catalog: None,
            },
            effort: EffortSpec {
                flag: Some("--thinking".into()),
                default: default_model_choice(),
                efforts: ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                offered: true,
                ..EffortSpec::default()
            },
            resume: ResumeSpec {
                flag: Some("--session-id".into()),
                ..ResumeSpec::default()
            },
            system: SystemSpec {
                append_flag: Some("--append-system-prompt".into()),
                ..SystemSpec::default()
            },
            hooks: Some("pi".into()),
            relocation_prompt: true,
            ..base
        },
        "muse" => HarnessDescriptor {
            label: "Muse".into(),
            program: "muse".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                default: default_model_choice(),
                models: Vec::new(),
                catalog: None,
            },
            effort: EffortSpec {
                default: default_model_choice(),
                offered: true,
                ..EffortSpec::default()
            },
            ..base
        },
        "grok" => HarnessDescriptor {
            label: "Grok Build".into(),
            program: "grok".into(),
            model: ModelSpec {
                flag: Some("--model".into()),
                ..ModelSpec::default()
            },
            effort: EffortSpec {
                flag: Some("--reasoning-effort".into()),
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
            ..base
        },
        // OpenCode's model is a `provider/model` id (`opencode models`
        // lists what this machine has credentials for; a hand-edited id
        // passes verbatim). No effort flag: reasoning is a per-model
        // variant chosen inside its own TUI. No permission flag either —
        // its prompts drive NEEDS FEEDBACK through the managed plugin
        // (`--auto` is one `permissions_flag` override away). The
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
                    "anthropic/claude-opus-5",
                    "anthropic/claude-sonnet-5",
                    "openai/gpt-5.5",
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

    fn apply_to(&self, current: &mut Option<T>)
    where
        T: Clone,
    {
        match self {
            Clearable::Keep => {}
            Clearable::Clear => *current = None,
            Clearable::Set(value) => *current = Some(value.clone()),
        }
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

/// One entry of the config file's `harnesses` map: the user's deltas over
/// a built-in row, or a whole new third-party harness. Every field is
/// optional; the merge applies what is set. Nullable rows (`program`,
/// `model_flag`, `permissions_flag`, `hooks`, …) are [`Clearable`] — a
/// value replaces the row and `null` clears it — so a built-in behavior
/// can be switched off as well as renamed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub program: Clearable<String>,
    /// The CLI's own environment, whole: a map replaces the row's, `null`
    /// clears it.
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub env: Clearable<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub model_flag: Clearable<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ModelEntry>>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub catalog: Clearable<HarnessCatalog>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub effort_flag: Clearable<String>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub effort_config_flag: Clearable<String>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub effort_config_key: Clearable<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_offered: Option<bool>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub permissions_flag: Clearable<String>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub prompt_flag: Clearable<String>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub resume_flag: Clearable<String>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub resume_subcommand: Clearable<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cd: Option<bool>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub system_append_flag: Clearable<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prepend_to_first_prompt: Option<bool>,
    #[serde(default, skip_serializing_if = "Clearable::is_keep")]
    pub hooks: Clearable<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relocation_prompt: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_model_effort: Option<bool>,
}

impl HarnessDescriptor {
    /// Apply the user's deltas in place. Set fields replace the row's;
    /// `null` clears a nullable one (a cleared program fails [`HarnessDescriptor::problem`]
    /// with its reason rather than launching nothing).
    pub fn apply(&mut self, over: &HarnessOverride) {
        if let Some(enabled) = over.enabled {
            self.enabled = enabled;
        }
        if let Some(label) = over.label.as_deref() {
            self.label = label.to_string();
        }
        match over.program.clone() {
            Clearable::Keep => {}
            Clearable::Clear => self.program.clear(),
            Clearable::Set(program) => self.program = program,
        }
        match &over.env {
            Clearable::Keep => {}
            Clearable::Clear => self.env.clear(),
            Clearable::Set(env) => self.env = env.clone(),
        }
        over.model_flag.apply_to(&mut self.model.flag);
        if let Some(default) = over.model_default.as_deref() {
            self.model.default = default.to_string();
        }
        if let Some(models) = over.models.clone() {
            self.model.models = models;
        }
        over.catalog.apply_to(&mut self.model.catalog);
        over.effort_flag.apply_to(&mut self.effort.flag);
        over.effort_config_flag
            .apply_to(&mut self.effort.config_flag);
        over.effort_config_key.apply_to(&mut self.effort.config_key);
        if let Some(default) = over.effort_default.as_deref() {
            self.effort.default = default.to_string();
        }
        if let Some(efforts) = over.efforts.clone() {
            self.effort.efforts = efforts;
        }
        if let Some(offered) = over.effort_offered {
            self.effort.offered = offered;
        }
        over.permissions_flag.apply_to(&mut self.permissions_flag);
        over.prompt_flag.apply_to(&mut self.prompt_flag);
        over.resume_flag.apply_to(&mut self.resume.flag);
        over.resume_subcommand.apply_to(&mut self.resume.subcommand);
        if let Some(cd) = over.resume_cd {
            self.resume.cd = cd;
        }
        over.system_append_flag
            .apply_to(&mut self.system.append_flag);
        if let Some(prepend) = over.system_prepend_to_first_prompt {
            self.system.prepend_to_first_prompt = prepend;
        }
        over.hooks.apply_to(&mut self.hooks);
        if let Some(relocation) = over.relocation_prompt {
            self.relocation_prompt = relocation;
        }
        if let Some(compose) = over.compose_model_effort {
            self.compose_model_effort = compose;
        }
    }
}

/// One user-defined harness, as the legacy `custom_harnesses` key holds
/// it. New harnesses belong in the `harnesses` map as full descriptors;
/// this list keeps reading, converting each entry to the descriptor shape
/// it always had (model flag, no effort row, no resume, optional hook
/// dialect). An id the map also names merges the map's deltas over the
/// entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomHarness {
    /// Stable id: what the picker shows the session under when the label
    /// is empty, and what persisted sessions point back at. Lowercase
    /// letters, digits and hyphens; must be unique within the list and
    /// must not collide with a built-in id (`claude`, `codex`, `cursor`,
    /// `pi`, `muse`, `opencode`).
    pub id: String,
    /// Display label for the picker and session rows. Empty falls back
    /// to the id.
    #[serde(default)]
    pub label: String,
    /// The CLI orion launches, resolved on PATH through the login shell
    /// like every other harness.
    pub program: String,
    /// Whether the NEW AGENT PICKER offers this entry.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Default model id. `"default"` (the default) means don't pass the
    /// flag — let the CLI pick; any other value is passed verbatim.
    #[serde(default = "default_model_choice")]
    pub model: String,
    /// The flag that carries the model id. `"--model"` unless the CLI
    /// spells it otherwise.
    #[serde(default = "default_model_flag")]
    pub model_flag: String,
    /// Hook dialect to install for this entry's sessions, naming the
    /// built-in CLI whose hooks the entry's program speaks: `"claude"`,
    /// `"codex"`, `"cursor"` or `"pi"`. With one set the sessions report
    /// status, prompts and permission waits exactly like that harness;
    /// without one they stay process-based (running while the PTY is
    /// live, never waiting-on-you). A Claude-compatible CLI gets title
    /// sync and auto-title with `"claude"`.
    #[serde(default)]
    pub hooks: Option<String>,
}

fn default_model_flag() -> String {
    "--model".into()
}

impl CustomHarness {
    /// The label the picker and session rows show.
    pub fn display_label(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.id
        } else {
            &self.label
        }
    }

    /// The descriptor shape this entry always had, before the map's
    /// deltas (if any) apply over it.
    pub fn as_descriptor(&self) -> HarnessDescriptor {
        HarnessDescriptor {
            id: self.id.clone(),
            label: self.label.clone(),
            program: self.program.clone(),
            env: BTreeMap::new(),
            enabled: self.enabled,
            model: ModelSpec {
                flag: Some(self.model_flag.clone()),
                default: self.model.clone(),
                models: Vec::new(),
                catalog: None,
            },
            effort: EffortSpec::default(),
            permissions_flag: None,
            prompt_flag: None,
            resume: ResumeSpec::default(),
            system: SystemSpec::default(),
            hooks: self.hooks.clone(),
            relocation_prompt: false,
            compose_model_effort: false,
        }
    }

    /// Why this entry is unusable, or None when it launches. The picker
    /// hides invalid entries; create paths refuse them with this message.
    pub fn problem(&self) -> Option<String> {
        if self.id.trim().is_empty() {
            return Some("custom harness with an empty id".into());
        }
        if !self
            .id
            .trim()
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Some(format!(
                "custom harness id `{}`: use lowercase letters, digits and hyphens",
                self.id.trim()
            ));
        }
        if crate::AgentKind::parse(self.id.trim()).is_some() {
            return Some(format!(
                "custom harness id `{}` collides with a built-in harness",
                self.id.trim()
            ));
        }
        self.as_descriptor().problem()
    }

    /// The hook dialect this entry's sessions install, if any.
    pub fn hook_dialect(&self) -> Option<crate::AgentKind> {
        self.as_descriptor().hook_dialect()
    }
}

/// The effective registry both halves of orion read: the built-ins in
/// [`AgentKind::ALL`] order with the `harnesses` map applied — Claude's
/// extra accounts right after it — then the legacy list in order (each with
/// its map deltas), then map-only new ids in key order. Callers hide
/// entries that are disabled or [`problem`][HarnessDescriptor::problem]-broken;
/// see [`usable`].
///
/// An account is Claude's effective row in its own config dir
/// ([`ClaudeAccount::descriptor`]), and the map's entry for its id is
/// deltas over that, as for a built-in. An account whose entry is broken,
/// or whose id a built-in, a legacy entry or an earlier account already
/// holds, is left out (the TUI's Agents tab names it and why).
pub fn registry(
    overrides: &BTreeMap<String, HarnessOverride>,
    customs: &[CustomHarness],
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
    let accounts = registered_accounts(accounts, customs);
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
    for entry in customs {
        let mut descriptor = entry.as_descriptor();
        if let Some(over) = overrides.get(&descriptor.id) {
            descriptor.apply(over);
        }
        out.push(descriptor);
    }
    let mut extra: Vec<&String> = overrides
        .keys()
        .filter(|id| {
            builtin(id).is_none()
                && !customs.iter().any(|entry| &entry.id == *id)
                && !accounts
                    .iter()
                    .any(|account| account.id.trim() == id.as_str())
        })
        .collect();
    extra.sort();
    for id in extra {
        let mut descriptor = HarnessDescriptor {
            id: id.clone(),
            label: String::new(),
            program: String::new(),
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
        };
        // A map-only entry that maps effort shows its Effort row like a
        // built-in; a bare program stays Model-only like a legacy custom.
        if let Some(over) = overrides.get(id) {
            descriptor.apply(over);
            if over.effort_offered.is_none()
                && (descriptor.effort.flag.is_some()
                    || descriptor.effort.config_key.is_some()
                    || !descriptor.effort.efforts.is_empty())
            {
                descriptor.effort.offered = true;
            }
        }
        out.push(descriptor);
    }
    out
}

/// The `claude_accounts` entries the registry carries: valid ones, each id
/// once, none a legacy entry already holds.
fn registered_accounts<'a>(
    accounts: &'a [ClaudeAccount],
    customs: &[CustomHarness],
) -> Vec<&'a ClaudeAccount> {
    let mut kept: Vec<&ClaudeAccount> = Vec::new();
    for account in accounts {
        let id = account.id.trim();
        if account.problem().is_none()
            && !customs.iter().any(|entry| entry.id.trim() == id)
            && !kept.iter().any(|other| other.id.trim() == id)
        {
            kept.push(account);
        }
    }
    kept
}

/// Every entry the picker and presets offer: enabled and valid, in
/// registry order. Invalid entries are left out (create paths refuse them
/// with their reason); the Agents tab still lists them so they can be
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

/// Resolve a session's harness to its effective descriptor: built-ins by
/// kind, customs by registry id. `Err` names why the launch is refused —
/// an unknown id, or the entry's [`problem`][HarnessDescriptor::problem].
pub fn resolve<'a>(
    all: &'a [HarnessDescriptor],
    kind: AgentKind,
    custom: Option<&str>,
) -> Result<&'a HarnessDescriptor, String> {
    let id = match kind {
        AgentKind::Custom => custom.unwrap_or_default().trim(),
        _ => kind.as_str(),
    };
    let Some(descriptor) = all.iter().find(|entry| entry.id == id) else {
        if kind == AgentKind::Custom {
            return Err(format!("custom harness `{id}` is no longer defined"));
        }
        return Err(format!("harness `{id}` is not in the registry"));
    };
    if let Some(problem) = descriptor.problem() {
        return Err(problem);
    }
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(id: &str) -> CustomHarness {
        CustomHarness {
            id: id.into(),
            label: String::new(),
            program: "agy".into(),
            enabled: true,
            model: default_model_choice(),
            model_flag: default_model_flag(),
            hooks: None,
        }
    }

    #[test]
    fn builtins_cover_every_builtin_kind() {
        let all = builtins();
        assert_eq!(all.len(), 7);
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
        let pi = builtin("pi").unwrap();
        assert_eq!(pi.resume.flag.as_deref(), Some("--session-id"));
        assert_eq!(pi.effort.flag.as_deref(), Some("--thinking"));
        let muse = builtin("muse").unwrap();
        assert!(!muse.resumes());
        assert_eq!(muse.hooks, None);
        assert!(muse.effort.offered, "muse keeps its reserved Effort row");
        let opencode = builtin("opencode").unwrap();
        assert_eq!(opencode.program, "opencode");
        assert_eq!(opencode.prompt_flag.as_deref(), Some("--prompt"));
        assert_eq!(opencode.resume.flag.as_deref(), Some("--session"));
        assert_eq!(opencode.hook_dialect(), Some(AgentKind::OpenCode));
        assert_eq!(opencode.permissions_flag, None, "its prompts drive red");
        assert!(!opencode.effort.offered, "no effort flag, no Effort row");
        assert!(!opencode.relocation_prompt);
        assert!(!opencode.claude_like());
    }

    #[test]
    fn overrides_apply_field_by_field_with_null_clearing() {
        let over: HarnessOverride = serde_json::from_value(serde_json::json!({
            "enabled": false,
            "program": "/opt/claude",
            "permissions_flag": null,
            "resume_flag": "--continue",
            "effort_default": "max",
        }))
        .unwrap();
        let mut descriptor = builtin("claude").unwrap();
        descriptor.apply(&over);
        assert!(!descriptor.enabled);
        assert_eq!(descriptor.program, "/opt/claude");
        assert_eq!(descriptor.resume.flag.as_deref(), Some("--continue"));
        assert_eq!(descriptor.default_effort(), Some("max"));
        // Absent fields leave the row alone.
        assert_eq!(descriptor.model.flag.as_deref(), Some("--model"));
        assert_eq!(descriptor.hooks.as_deref(), Some("claude"));

        // `null` on a tri-state clears the behavior it named.
        let clear: HarnessOverride = serde_json::from_value(serde_json::json!({
            "permissions_flag": null,
            "hooks": null,
        }))
        .unwrap();
        let mut codex = builtin("codex").unwrap();
        codex.apply(&clear);
        assert_eq!(codex.permissions_flag, None);
        let mut claude = builtin("claude").unwrap();
        claude.apply(&clear);
        assert_eq!(claude.hooks, None);
        assert!(!claude.claude_like());
    }

    #[test]
    fn registry_orders_builtins_then_legacy_then_new_ids() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "agy".into(),
            HarnessOverride {
                program: Clearable::Set("agy".into()),
                ..HarnessOverride::default()
            },
        );
        overrides.insert(
            "claude".into(),
            HarnessOverride {
                enabled: Some(false),
                ..HarnessOverride::default()
            },
        );
        let customs = vec![custom("zed")];
        let all = registry(&overrides, &customs, &[]);
        let ids: Vec<&str> = all.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["claude", "codex", "cursor", "pi", "muse", "grok", "opencode", "zed", "agy"]
        );
        assert!(!all[0].enabled, "the claude override applied");
        assert_eq!(all[7].program, "agy", "legacy entry converts");
        assert_eq!(all[8].program, "agy", "map-only entry resolves");
        assert!(
            !all[8].effort.offered,
            "a bare program stays Model-only like a legacy custom"
        );
    }

    #[test]
    fn map_only_entry_mapping_effort_shows_its_row() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "agy".into(),
            HarnessOverride {
                program: Clearable::Set("agy".into()),
                effort_flag: Clearable::Set("--effort".into()),
                efforts: Some(vec!["low".into(), "high".into()]),
                ..HarnessOverride::default()
            },
        );
        let all = registry(&overrides, &[], &[]);
        let agy = all.iter().find(|entry| entry.id == "agy").unwrap();
        assert!(agy.effort.offered);
        assert_eq!(agy.problem(), None);
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
        let all = registry(&BTreeMap::new(), &[custom("agy")], &[]);
        assert_eq!(resolve(&all, AgentKind::Claude, None).unwrap().id, "claude");
        assert_eq!(
            resolve(&all, AgentKind::Custom, Some("agy"))
                .unwrap()
                .program,
            "agy"
        );
        assert!(resolve(&all, AgentKind::Custom, Some("gone")).is_err());
        assert!(resolve(&all, AgentKind::Custom, None).is_err());
        let mut broken = custom("broken");
        broken.program = "  ".into();
        let all = registry(&BTreeMap::new(), &[broken], &[]);
        assert!(resolve(&all, AgentKind::Custom, Some("broken")).is_err());
        assert_eq!(usable(&all).len(), 7, "the broken entry is hidden");
    }

    #[test]
    fn legacy_validation_still_names_its_problems() {
        assert!(custom("").problem().is_some());
        assert!(custom("Agy").problem().is_some());
        assert!(custom("claude").problem().is_some());
        assert!(custom("muse").problem().is_some());
        assert!(custom("grok").problem().is_some());
        assert!(custom("opencode").problem().is_some());
        let claude_hooks = CustomHarness {
            hooks: Some("claude".into()),
            ..custom("agy")
        };
        assert_eq!(claude_hooks.problem(), None);
        assert_eq!(claude_hooks.hook_dialect(), Some(crate::AgentKind::Claude));
    }

    /// The second Claude account as config.json writes it: `env` merges
    /// over the row like any other field, `null` clears it, and
    /// `orion config harnesses` prints it.
    #[test]
    fn env_rides_the_override_and_null_clears_it() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "claude-b".to_string(),
            serde_json::from_value::<HarnessOverride>(serde_json::json!({
                "label": "Claude B",
                "program": "claude",
                "env": {"CLAUDE_CONFIG_DIR": "~/.claude-b"},
                "hooks": "claude",
                "resume_flag": "--resume",
                "catalog": "claude",
            }))
            .unwrap(),
        );
        let all = registry(&overrides, &[], &[]);
        let b = all.iter().find(|entry| entry.id == "claude-b").unwrap();
        assert_eq!(b.problem(), None);
        assert_eq!(b.env["CLAUDE_CONFIG_DIR"], "~/.claude-b");
        let printed = serde_json::to_value(b).unwrap();
        assert_eq!(printed["env"]["CLAUDE_CONFIG_DIR"], "~/.claude-b");
        // A row without one prints none.
        let claude = serde_json::to_value(builtin("claude").unwrap()).unwrap();
        assert!(claude.get("env").is_none());

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

    /// **Continue on**: every other enabled Claude-dialect harness that
    /// resumes and keeps its sessions where orion can see them — built-in
    /// Claude, or one whose `env` pins a config dir. A wrapper, a harness
    /// off the dialect, one that boots fresh, and a switched-off one are
    /// not offered.
    #[test]
    fn continue_targets_are_the_other_claude_accounts() {
        let account = |id: &str, extra: serde_json::Value| {
            let mut over = serde_json::json!({
                "program": "claude",
                "hooks": "claude",
                "resume_flag": "--resume",
            });
            over.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            (
                id.to_string(),
                serde_json::from_value::<HarnessOverride>(over).unwrap(),
            )
        };
        let overrides: BTreeMap<String, HarnessOverride> = [
            account("claude-b", serde_json::json!({"env": {"CLAUDE_CONFIG_DIR": "~/.claude-b"}})),
            account("claude-c", serde_json::json!({"env": {"CLAUDE_CONFIG_DIR": "~/.claude-c"}, "enabled": false})),
            account("claude-d", serde_json::json!({"env": {"CLAUDE_CONFIG_DIR": "~/.claude-d"}, "resume_flag": null})),
            account("wrapper", serde_json::json!({"program": "/home/me/bin/claude-b"})),
        ]
        .into_iter()
        .collect();
        let all = registry(&overrides, &[], &[]);
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
    /// can't be an account — broken, or an id already taken — stays out.
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
                    "program": "/opt/claude",
                    "enabled": false,
                    "label": "Work",
                }))
                .unwrap(),
            ),
            (
                "claude-2".to_string(),
                serde_json::from_value(serde_json::json!({"model_default": "opus"})).unwrap(),
            ),
        ]
        .into_iter()
        .collect();
        let accounts = [
            account("claude-2", "~/.claude-2"),
            account("claude-2", "~/.claude-again"),
            account("agy", "~/.claude-agy"),
            account("Bad Id", "~/.claude-bad"),
            account("claude-3", "/srv/claude-3"),
        ];
        let all = registry(&overrides, &[custom("agy")], &accounts);
        let ids: Vec<&str> = all.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "claude", "claude-2", "claude-3", "codex", "cursor", "pi", "muse", "grok",
                "opencode", "agy"
            ]
        );
        let two = &all[1];
        assert_eq!(
            two.program, "/opt/claude",
            "Claude's row, overrides and all"
        );
        assert!(two.enabled, "the account's own switch, not Claude's");
        assert_eq!(two.label, "", "unnamed: never Claude's label");
        assert_eq!(
            two.env["CLAUDE_CONFIG_DIR"], "~/.claude-2",
            "the first entry wins"
        );
        assert_eq!(two.default_model(), Some("opus"), "its map entry applied");
        assert_eq!(all[2].env["CLAUDE_CONFIG_DIR"], "/srv/claude-3");
        assert_eq!(
            all.last().unwrap().program,
            "agy",
            "the legacy entry kept its id"
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
        let legacy = CustomHarness {
            hooks: Some("claude".into()),
            ..custom("agy")
        }
        .as_descriptor();
        assert!(
            legacy.offers_model("opus"),
            "an empty list passes ids verbatim"
        );
        assert!(!legacy.offers_effort("high"), "no Effort row, no effort");
    }
}
