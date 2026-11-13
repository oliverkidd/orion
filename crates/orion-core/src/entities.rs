use crate::ids::{AgentId, LinkId, ProjectId, TerminalId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Never run yet (gray).
    Fresh,
    /// Actively working (yellow).
    Running,
    /// Turn complete (green).
    Finished,
    /// Waiting on the user: permission prompt or question (red).
    NeedsFeedback,
    /// Process died with a nonzero exit while working.
    Terminated,
    /// Daemon restarted while the agent was live; PTY is gone.
    Disconnected,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Fresh => "fresh",
            AgentStatus::Running => "running",
            AgentStatus::Finished => "finished",
            AgentStatus::NeedsFeedback => "needs_feedback",
            AgentStatus::Terminated => "terminated",
            AgentStatus::Disconnected => "disconnected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "fresh" => AgentStatus::Fresh,
            "running" => AgentStatus::Running,
            "finished" => AgentStatus::Finished,
            "needs_feedback" => AgentStatus::NeedsFeedback,
            "terminated" => AgentStatus::Terminated,
            "disconnected" => AgentStatus::Disconnected,
            _ => return None,
        })
    }
}

/// Which agent CLI a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
    Cursor,
    /// xAI's Grok Build CLI. Status and its session id come from managed
    /// hooks in its home (see the daemon's `hooks::installer`); it has no
    /// hook that says it is waiting on you.
    Grok,
    /// OpenCode (opencode.ai): the `opencode` CLI. Status comes from a
    /// managed TypeScript plugin rather than shell hooks (see the daemon's
    /// `hooks::opencode_plugin`); the first prompt rides `--prompt`, since
    /// its positional is the project path, and a session resumes by
    /// `--session <id>`.
    OpenCode,
    /// A harness the `harnesses` map adds: the registry id travels beside
    /// the session (see `Agent::custom_harness`), never in this variant.
    /// Launches as its registry entry describes. A Claude account is no
    /// custom harness: it is [`AgentKind::Claude`] with the account's id.
    Custom,
}

impl AgentKind {
    /// Every kind, for callers that must cover all of them (menus, the
    /// boot-time CLI probe warm) and should fail to compile if one is added.
    /// `Custom` rides along: it never launches without its registry entry,
    /// so loops over ALL skip it explicitly where a bare kind is meaningless.
    pub const ALL: [AgentKind; 6] = [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Cursor,
        AgentKind::Grok,
        AgentKind::OpenCode,
        AgentKind::Custom,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
            AgentKind::Grok => "grok",
            AgentKind::OpenCode => "opencode",
            AgentKind::Custom => "custom",
        }
    }

    /// Parse a harness name from settings or the CLI. Bare `"custom"`
    /// never parses: a custom harness is meaningless without its registry
    /// id, which travels in its own field.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "claude" => AgentKind::Claude,
            "codex" => AgentKind::Codex,
            "cursor" => AgentKind::Cursor,
            "grok" => AgentKind::Grok,
            "opencode" => AgentKind::OpenCode,
            _ => return None,
        })
    }

    /// Binary the kind launches. Differs from `as_str` only for Cursor,
    /// whose agent CLI ships as `cursor-agent` (`cursor` opens the editor).
    /// `Custom` has no static program — its entry names it — so every
    /// launch path resolves through the harness registry first; the
    /// placeholder below only surfaces as a "not found on PATH" error if
    /// one ever launches it bare.
    pub fn cli_program(&self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor-agent",
            AgentKind::Grok => "grok",
            AgentKind::OpenCode => "opencode",
            AgentKind::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub repo_path: PathBuf,
    pub sort_order: i64,
}

impl Project {
    /// The name a project takes from disk: the last component of its repo
    /// path. This is the default `name`, and it stays the truth about where
    /// the project lives no matter what the row is later renamed to.
    pub fn folder_name(repo_path: &Path) -> String {
        repo_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    }

    /// The folder name to show beneath a renamed row, or None while the row
    /// still carries the folder's own name and repeating it would be noise.
    pub fn folder_subtitle(&self) -> Option<String> {
        let folder = Self::folder_name(&self.repo_path);
        (folder != self.name).then_some(folder)
    }
}

/// What a SPARE WORKTREE's branch, and its directory in the WORKTREE DIR,
/// are named after (`orion-spare-3kq9xa`): the warm next worktree the
/// DAEMON keeps for each project in use, so whoever meets one in `git
/// worktree list` or a branch list knows whose it is. Every listing orion
/// shows leaves these out until a launch takes one — its branch is then
/// renamed, its directory keeps the name.
pub const SPARE_PREFIX: &str = "orion-spare-";

/// Whether `branch` is a SPARE WORKTREE's ([`SPARE_PREFIX`]).
pub fn is_spare_branch(branch: &str) -> bool {
    branch.starts_with(SPARE_PREFIX)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    pub id: WorktreeId,
    pub project_id: ProjectId,
    pub path: PathBuf,
    pub branch: String,
    pub is_main: bool,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub worktree_id: WorktreeId,
    pub name: String,
    pub status: AgentStatus,
    pub archived: bool,
    /// Epoch ms of the last archive; 0 = never archived (or archived before
    /// this field existed). Orders the ARCHIVED group newest-first.
    #[serde(default)]
    pub archived_at: i64,
    /// Finished a turn (running or needs-feedback → finished) that no client
    /// has looked at since. The Projects and Worktrees rows count these so
    /// the user knows how many terminals to go read; the pane landing on
    /// the session clears it (`ClientRequest::MarkAgentSeen`). Only ever
    /// true on a finished, unarchived row — leaving `finished` clears it.
    #[serde(default)]
    pub unseen: bool,
    /// Epoch ms of the last status change; 0 = unknown (pre-upgrade rows or
    /// never-run agents). Drives the TUI's RECENT session group.
    #[serde(default)]
    pub status_changed_at: i64,
    #[serde(default)]
    pub kind: AgentKind,
    /// The registry id beside `kind`: the CLAUDE ACCOUNT a
    /// [`AgentKind::Claude`] row runs on (None for the default account),
    /// or the `harnesses` map entry of an [`AgentKind::Custom`] one.
    /// Persisted beside the row so respawns find the same entry; None for
    /// every other built-in harness. See [`crate::harness::registry_id`].
    #[serde(default)]
    pub custom_harness: Option<String>,
    /// Model the CLI is launched with (claude `--model` / codex `-m`);
    /// None = the CLI's own default. Persisted so respawns keep it.
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort the CLI is launched with (claude `--effort` /
    /// codex `model_reasoning_effort`); None = the CLI's own default.
    #[serde(default)]
    pub effort: Option<String>,
    /// CLI session id used for resume (claude, codex, or cursor, per `kind`).
    pub session_id: Option<String>,
    pub sort_order: i64,
    /// True when the daemon currently holds a live PTY for this agent.
    pub alive: bool,
    /// The last few prompts typed into this session, oldest first — what
    /// the `UserPromptSubmit` hook carried, condensed to one line each
    /// (RECENT PROMPTS). Capped at [`RECENT_PROMPTS_KEPT`] by the daemon;
    /// the TUI shows however many its setting asks for, the newest at
    /// the bottom. Empty for every row that predates the capture.
    #[serde(default)]
    pub recent_prompts: Vec<PromptEntry>,
    /// Why a Claude session is waiting on you when the answer is its
    /// account, not a question: the usage limit it stopped on (see
    /// [`UsageLimit`]). Recorded with the turn into
    /// [`AgentStatus::NeedsFeedback`] and dropped as the row leaves it;
    /// read it through [`Agent::limit_reached`].
    #[serde(default)]
    pub usage_limit: Option<UsageLimit>,
    /// The AUTOMATION that started this session — its id, `pr_fixer`,
    /// `bug_fixer` … (the TUI's `automations::Automation::id`) — or None
    /// for one you started. Set once at create and persisted with the
    /// row; it drives the grid's badge, the Needs-you kind label and the
    /// automations' running cap.
    #[serde(default)]
    pub automation: Option<String>,
}

/// A Claude session stopped on its account rather than its work: a usage
/// limit, a billing stop, an account on hold. The DAEMON records one from
/// Claude Code's `StopFailure` hook — or from the notification that ends
/// its wait for a reset without continuing — and only ever beside the turn
/// going to NEEDS FEEDBACK, so the row is red with a reason; the next
/// prompt or tool call that moves the session takes both away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageLimit {
    /// What stopped it.
    pub reason: LimitReason,
    /// Claude's own line about it, on one line — `You've hit your session
    /// limit · resets 3:45pm`. None when the hook carried none.
    #[serde(default)]
    pub message: Option<String>,
}

impl UsageLimit {
    /// The few words a card and a list line say it in.
    pub fn label(&self) -> &'static str {
        match self.reason {
            LimitReason::AccountOnHold => "account on hold",
            LimitReason::RateLimit | LimitReason::BillingError | LimitReason::QuotaWaitEnded => {
                "limit reached"
            }
        }
    }
}

/// Why a Claude session stopped on its account, under the names Claude
/// Code gives them — which are also how the row stores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitReason {
    /// `StopFailure`'s `rate_limit`: a usage limit — the session's, the
    /// week's, a model family's.
    RateLimit,
    /// `StopFailure`'s `billing_error`: a spend limit reached, or credits
    /// run out.
    BillingError,
    /// `StopFailure`'s `account_on_hold`.
    AccountOnHold,
    /// The `quota_auto_resume_disabled` notification: Claude's wait for
    /// a usage limit to reset ended without carrying the task on.
    #[serde(rename = "quota_auto_resume_disabled")]
    QuotaWaitEnded,
}

impl LimitReason {
    /// Claude Code's own name for it.
    pub fn as_str(self) -> &'static str {
        match self {
            LimitReason::RateLimit => "rate_limit",
            LimitReason::BillingError => "billing_error",
            LimitReason::AccountOnHold => "account_on_hold",
            LimitReason::QuotaWaitEnded => "quota_auto_resume_disabled",
        }
    }

    /// The reason a `StopFailure` error type names, when it stops the
    /// session on its account — a usage limit, a billing stop, a held
    /// account, none of which another try on the same account gets past.
    /// `overloaded`, `server_error` and the rest are an API having a bad
    /// moment; `authentication_failed` is a `/login` away on the same
    /// account. None for those.
    pub fn of_stop_failure(error: &str) -> Option<Self> {
        Some(match error {
            "rate_limit" => LimitReason::RateLimit,
            "billing_error" => LimitReason::BillingError,
            "account_on_hold" => LimitReason::AccountOnHold,
            _ => return None,
        })
    }
}

/// Why a session is waiting on you: what the DAEMON learnt from the hook
/// that turned it red (or from the PTY dying). A Claude `AskUserQuestion`
/// or `ExitPlanMode` names its question or plan in the hook's
/// `tool_input`; any other `PermissionRequest` names only the tool. Sent
/// beside the status on `StatusChanged` and with `LastResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaitReason {
    /// An `AskUserQuestion`: every question it asks, in the order its
    /// dialog's tabs show them.
    Question { questions: Vec<Asked> },
    /// An `ExitPlanMode`: the plan waiting for approval.
    Plan { plan: String },
    /// A permission prompt for this tool.
    Permission { tool: String },
    /// The CLI exited with an error.
    Crash { exit_code: Option<i32> },
}

/// One question of an `AskUserQuestion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    pub question: String,
    /// Its short tab label (`header`), empty when it has none.
    pub header: String,
    /// The option labels, in the order the dialog numbers them.
    pub options: Vec<String>,
    /// Several options may be picked.
    pub multi: bool,
    /// An option carries a `preview` (a mockup, a snippet). Claude Code
    /// shows a single-select question with previews side by side — a
    /// digit only moves the cursor, and your own words go in as notes —
    /// and a multi-select one as it shows any other.
    #[serde(default)]
    pub previews: bool,
}

/// The plan an `ExitPlanMode` shows when its input carries no plan text
/// and names no readable plan file.
pub const PLAN_IN_CLAUDE: &str = "(plan in Claude)";

/// An `ExitPlanMode` input's plan: its `plan`, else the contents of its
/// `planFilePath`. None when neither has any text.
fn plan_text(input: &serde_json::Value) -> Option<String> {
    let inline = input.get("plan").and_then(|p| p.as_str()).map(str::trim);
    let text = match inline {
        Some(plan) if !plan.is_empty() => plan.to_string(),
        _ => {
            let path = input.get("planFilePath")?.as_str()?;
            std::fs::read_to_string(path).ok()?.trim().to_string()
        }
    };
    (!text.is_empty()).then_some(text)
}

/// Claude's question tool.
pub const ASK_USER_QUESTION_TOOL: &str = "AskUserQuestion";
/// Claude's plan approval tool.
pub const EXIT_PLAN_MODE_TOOL: &str = "ExitPlanMode";

impl WaitReason {
    /// The reason a dialog for `tool` raises, read off its input — a hook's
    /// `tool_input`, or a transcript's `tool_use.input` (the same shape).
    /// An `AskUserQuestion` whose input doesn't parse says no more than any
    /// other tool's permission prompt does. An `ExitPlanMode` is always a
    /// plan: its `plan`, else the file its `planFilePath` names (Claude
    /// Code 2.1.322 sends both), else [`PLAN_IN_CLAUDE`].
    pub fn from_tool(tool: &str, input: Option<&serde_json::Value>) -> Self {
        let parsed = match tool {
            ASK_USER_QUESTION_TOOL => input.and_then(Self::question),
            EXIT_PLAN_MODE_TOOL => Some(WaitReason::Plan {
                plan: input.and_then(plan_text).unwrap_or(PLAN_IN_CLAUDE.into()),
            }),
            _ => None,
        };
        parsed.unwrap_or_else(|| WaitReason::Permission {
            tool: tool.to_string(),
        })
    }

    /// Every question of an `AskUserQuestion` input: `{questions:
    /// [{question, header, options: [{label, description}],
    /// multiSelect}]}`. None when it has none.
    fn question(input: &serde_json::Value) -> Option<Self> {
        let questions: Vec<Asked> = input
            .get("questions")?
            .as_array()?
            .iter()
            .filter_map(|q| {
                let text = |key: &str| q.get(key).and_then(|v| v.as_str()).map(str::trim);
                let listed = q
                    .get("options")
                    .and_then(|o| o.as_array())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let options = listed
                    .iter()
                    .filter_map(|o| o.get("label").and_then(|l| l.as_str()))
                    .map(str::to_string)
                    .collect();
                let previews = listed.iter().any(|o| {
                    o.get("preview")
                        .and_then(|p| p.as_str())
                        .is_some_and(|p| !p.trim().is_empty())
                });
                Some(Asked {
                    question: text("question")?.to_string(),
                    header: text("header").unwrap_or_default().to_string(),
                    options,
                    multi: q
                        .get("multiSelect")
                        .and_then(|m| m.as_bool())
                        .unwrap_or(false),
                    previews,
                })
            })
            .collect();
        (!questions.is_empty()).then_some(WaitReason::Question { questions })
    }
}

impl Agent {
    /// The usage limit this session is stopped on, while it is: a red,
    /// unarchived row that has one recorded. A recorded limit on any other
    /// row is a moment stale — its clearing upsert is on the way — and
    /// says nothing.
    pub fn limit_reached(&self) -> Option<&UsageLimit> {
        self.usage_limit
            .as_ref()
            .filter(|_| self.status == AgentStatus::NeedsFeedback && !self.archived)
    }
}

/// The positive number that follows `marker` in `url` (`/issues/`,
/// `/pull/`), up to the next path or query separator.
pub fn url_number_after(url: &str, marker: &str) -> Option<u64> {
    let (_, tail) = url.split_once(marker)?;
    let digits = tail.split(['/', '?', '#']).next().unwrap_or_default();
    digits.parse::<u64>().ok().filter(|n| *n > 0)
}

/// How many prompts the daemon keeps per session: the most a TUI can be
/// asked to show, with room to spare so a raised setting has history to
/// draw from at once.
pub const RECENT_PROMPTS_KEPT: usize = 10;

/// One prompt in a session's RECENT PROMPTS history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEntry {
    /// The prompt as one line: whitespace runs collapsed, clipped with an
    /// ellipsis past the daemon's cap. Never empty.
    pub text: String,
    /// Epoch ms when the prompt was submitted (the hook's arrival).
    pub submitted_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalTab {
    pub id: TerminalId,
    pub worktree_id: WorktreeId,
    pub name: String,
    pub sort_order: i64,
    /// True when the daemon currently holds a live PTY for this terminal.
    pub alive: bool,
    /// Set on a RUN TERMINAL — the one `r` starts on a worktree: the
    /// project's run command it was launched with, run through the
    /// login shell in place of an interactive one. While its PTY is alive
    /// the worktree is RUNNING. On a SETUP TERMINAL, the setup command it
    /// runs the same way. None for a plain shell tab.
    #[serde(default)]
    pub run_command: Option<String>,
    /// A SETUP TERMINAL: the project's setup command, which the DAEMON
    /// ran here once, as it created the worktree. Its row reads `setting
    /// up…` while it runs, goes on a clean exit, and stays as `setup
    /// failed (1)` otherwise. Never the worktree's RUN TERMINAL.
    #[serde(default)]
    pub setup: bool,
    /// Where a RUN TERMINAL's run is at, as the DAEMON knows it right now.
    /// Stamped on every send, never stored; the default on a plain shell.
    #[serde(default)]
    pub run: RunState,
}

impl TerminalTab {
    /// The worktree's RUN TERMINAL — what **Start stack** starts and stops
    /// — rather than a shell or its SETUP TERMINAL.
    pub fn is_run(&self) -> bool {
        self.run_command.is_some() && !self.setup
    }
}

/// What a RUN LINE says about its run beyond whether its PTY lives
/// ([`TerminalTab::alive`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunState {
    /// Sent its `^C` by a stop and not yet gone: the epoch ms its
    /// two-minute kill lands at, so every client counts down to the same
    /// moment (`stopping… 1:52`). None for a run nobody is stopping, and
    /// always None once the PTY is gone.
    pub stopping_until: Option<i64>,
    /// That stop is a restart's: once the run has gone it starts again in
    /// this row (`restarting… 1:52`) rather than taking the row with it.
    pub restarting: bool,
    /// A run that ended on its own, nobody having stopped it: the code it
    /// exited with — the run line's faint `done` for 0, a red `exited 1`
    /// for anything else. None while it runs, and for one whose ending
    /// this DAEMON did not see (a run from before it restarted). A run a
    /// stop ended has no row left to carry one.
    pub exit_code: Option<i32>,
    /// The localhost ports the live run has printed —
    /// `http://localhost:3000` is 3000 — first seen first, kept after the
    /// lines scroll away: the run line's `:3000 :5173`, the first of them
    /// what ⌘O's **Open app** opens. Empty once the run has ended, and
    /// again from a start.
    pub ports: Vec<u16>,
    /// On a SETUP TERMINAL: a **Start stack** pressed while it runs is
    /// waiting for it. The run starts once the setup exits cleanly, and
    /// the footer says so meanwhile.
    pub run_queued: bool,
}

/// A URL pinned to a worktree — the pull request, the ticket, the design
/// doc for whatever that checkout is for. Orion never fetches these; they
/// are bookmarks the user opens in a browser from the Sessions panel. The
/// open pull request shown above them is discovered from git, not stored
/// here (see the TUI's `PullRequest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub id: LinkId,
    pub worktree_id: WorktreeId,
    /// Always http(s) — normalized on the way in, so opening one can never
    /// hand the OS a scheme the user didn't intend.
    pub url: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Entity {
    Project(Project),
    Worktree(Worktree),
    Agent(Agent),
    Terminal(TerminalTab),
    Link(Link),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityId {
    Project(ProjectId),
    Worktree(WorktreeId),
    Agent(AgentId),
    Terminal(TerminalId),
    Link(LinkId),
}

#[cfg(test)]
mod wait_reason_tests {
    use super::*;
    use serde_json::json;

    /// The `tool_input` Claude Code 2.1.319 (and 2.1.322) posts on
    /// `PreToolUse` and `PermissionRequest` for its question and plan
    /// dialogs.
    #[test]
    fn question_and_plan_parse_from_the_hook_tool_input() {
        let ask = json!({"questions": [
            {"question": "Which color do you like?", "header": "Color", "multiSelect": false,
             "options": [{"label": "Red", "description": "Red"}, {"label": "Green", "description": "Green"}]},
            {"question": "A second one", "options": []}
        ]});
        assert_eq!(
            WaitReason::from_tool(ASK_USER_QUESTION_TOOL, Some(&ask)),
            WaitReason::Question {
                questions: vec![
                    Asked {
                        question: "Which color do you like?".into(),
                        header: "Color".into(),
                        options: vec!["Red".into(), "Green".into()],
                        multi: false,
                        previews: false,
                    },
                    Asked {
                        question: "A second one".into(),
                        header: String::new(),
                        options: vec![],
                        multi: false,
                        previews: false,
                    },
                ]
            }
        );
        let multi = json!({"questions": [{"question": "Fruits?", "multiSelect": true,
            "options": [{"label": "Apple"}, {"label": "Pear"}]}]});
        assert!(matches!(
            WaitReason::from_tool(ASK_USER_QUESTION_TOOL, Some(&multi)),
            WaitReason::Question { questions } if questions[0].multi
        ));
        // Claude Code 2.1.323's side-by-side layout: any option's preview.
        let previews = json!({"questions": [{"question": "Layout?", "multiSelect": false,
            "options": [{"label": "Grid", "preview": "+--+--+"}, {"label": "List"}]}]});
        assert!(matches!(
            WaitReason::from_tool(ASK_USER_QUESTION_TOOL, Some(&previews)),
            WaitReason::Question { questions } if questions[0].previews
        ));
        // A reason stored before `previews` was read has none.
        let stored: Asked = serde_json::from_value(
            json!({"question": "Q", "header": "", "options": [], "multi": false}),
        )
        .unwrap();
        assert!(!stored.previews);
        let plan = json!({"plan": "# Create hello.txt\n\n1. Write it.\n", "planFilePath": "/x.md"});
        assert_eq!(
            WaitReason::from_tool(EXIT_PLAN_MODE_TOOL, Some(&plan)),
            WaitReason::Plan {
                plan: "# Create hello.txt\n\n1. Write it.".into()
            }
        );
    }

    #[test]
    fn anything_else_is_the_tool_permission() {
        assert_eq!(
            WaitReason::from_tool("Bash", Some(&json!({"command": "ls"}))),
            WaitReason::Permission {
                tool: "Bash".into()
            }
        );
        // A question or plan whose input drifted still names its tool.
        assert_eq!(
            WaitReason::from_tool(ASK_USER_QUESTION_TOOL, Some(&json!({"questions": []}))),
            WaitReason::Permission {
                tool: ASK_USER_QUESTION_TOOL.into()
            }
        );
    }

    /// An `ExitPlanMode` is a plan however little its input says: the
    /// plan file stands in for a missing `plan`, and with neither the card
    /// still reaches the phone as a plan.
    #[test]
    fn a_plan_without_its_text_is_still_a_plan() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plan.md");
        std::fs::write(&file, "# From the file\n\n1. Do it.\n").unwrap();
        assert_eq!(
            WaitReason::from_tool(
                EXIT_PLAN_MODE_TOOL,
                Some(&json!({"planFilePath": file.to_str().unwrap()}))
            ),
            WaitReason::Plan {
                plan: "# From the file\n\n1. Do it.".into()
            }
        );
        let in_claude = WaitReason::Plan {
            plan: PLAN_IN_CLAUDE.into(),
        };
        assert_eq!(
            WaitReason::from_tool(EXIT_PLAN_MODE_TOOL, Some(&json!({}))),
            in_claude
        );
        assert_eq!(WaitReason::from_tool(EXIT_PLAN_MODE_TOOL, None), in_claude);
        assert_eq!(
            WaitReason::from_tool(
                EXIT_PLAN_MODE_TOOL,
                Some(&json!({"plan": " ", "planFilePath": "/nowhere/plan.md"}))
            ),
            in_claude
        );
    }
}
