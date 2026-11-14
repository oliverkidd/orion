use crate::compose::Stack;
use crate::entities::{
    Agent, AgentKind, AgentStatus, Entity, EntityId, Link, Project, TerminalTab, WaitReason,
    Worktree,
};
use crate::ids::{AgentId, LinkId, ProjectId, TerminalId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bump on any breaking change to these enums. The daemon refuses mismatched
/// clients; the client then offers a kill-and-restart of the old daemon.
pub const PROTOCOL_VERSION: u32 = 57;

/// What the reader of a frame its build can't decode is told to do. The
/// frames are positional msgpack, so a peer built from different protocol
/// types — a TUI installed over a DAEMON that is still running, with
/// `PROTOCOL_VERSION` left where it was — sends payloads the other side
/// can't read even though the handshake passed. Neither side can tell which
/// build is older, and `orion kill` alone only helps when the DAEMON is (a
/// live older client respawns its DAEMON from its own binary), so the hint
/// covers both.
pub const UNDECODABLE_FRAME_HINT: &str = "orion and its daemon are different builds, so a message \
     between them could not be read — make sure the newest orion is installed, then run \
     `orion kill` and start orion again";

/// Max IPC frame size (length prefix sanity bound).
pub const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionRef {
    Agent(AgentId),
    Terminal(TerminalId),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientRequest {
    Hello {
        protocol_version: u32,
    },
    /// Reply is one Snapshot, then deltas stream on this connection forever.
    Subscribe,

    // -- PTY plane --
    Attach {
        session: SessionRef,
        /// Resume point for gap-free re-attach; None = replay whole ring.
        from_seq: Option<u64>,
        cols: u16,
        rows: u16,
    },
    Detach {
        session: SessionRef,
    },
    Input {
        session: SessionRef,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Resize {
        session: SessionRef,
        cols: u16,
        rows: u16,
    },

    // -- entity CRUD (RPC-style; answered by Ack/Error with matching req_id) --
    /// Register a repo as a project. Refused when `path` resolves to a
    /// repo this machine already knows — one repo is one project.
    AddProject {
        req_id: u64,
        path: PathBuf,
        name: Option<String>,
        /// Make `path` a repository if it isn't one: create the directory
        /// when it doesn't exist on disk, and `git init` it when it isn't
        /// inside a repository. Set only after the user confirmed in the
        /// client.
        create_missing: bool,
    },
    RemoveProject {
        req_id: u64,
        id: ProjectId,
    },
    /// Retitle a project's row. Purely cosmetic: `repo_path` — the folder on
    /// disk — is never touched. An empty name resets the row to the folder's
    /// own name, which is the only way back from a rename.
    RenameProject {
        req_id: u64,
        id: ProjectId,
        name: String,
    },
    CreateWorktree {
        req_id: u64,
        project: ProjectId,
        branch: String,
        base: Option<String>,
        /// Check `branch` out as it already exists — local, or only on
        /// `origin` (`origin/feat` or `feat`) — instead of cutting a new
        /// branch of that name; `base` is ignored. A row already on the
        /// branch is the answer.
        existing: bool,
        /// The launch this checkout is cut for — the `CreateAgent` its Ack
        /// sends next — when there is one. A new branch off the project's
        /// own base (no `base`, not `existing`) for a launch that is the
        /// one its SPARE WORKTREE's CLI booted as takes the spare: its
        /// branch renamed to `branch`, its row sent, and its CLI handed to
        /// that `CreateAgent`. Anything else is cut as before.
        #[serde(default)]
        launch: Option<LaunchSpec>,
    },
    DeleteWorktree {
        req_id: u64,
        id: WorktreeId,
        /// Delete even when the checkout has uncommitted or untracked
        /// changes. Unforced, such a checkout is answered with
        /// `WorktreeHasChanges` and left exactly as it was.
        force: bool,
    },
    CreateAgent {
        req_id: u64,
        worktree: WorktreeId,
        name: String,
        kind: AgentKind,
        /// The registry id beside `kind`: a Claude account's id, or a
        /// custom harness's (see `Agent::custom_harness`). Persisted with
        /// the row like `model`.
        #[serde(default)]
        custom_harness: Option<String>,
        /// Model the CLI launches with; None = the CLI's own default.
        model: Option<String>,
        /// Reasoning effort the CLI launches with; None = the CLI's own default.
        effort: Option<String>,
        /// True when the user accepted the generated default name, marking
        /// the session eligible for one agent-driven auto-title (the CLI
        /// runs `orion rename` on its first prompt).
        auto_title: bool,
        /// The first turn handed to the CLI as its positional prompt
        /// (`claude "<text>"`, `codex "<text>"`, `cursor-agent "<text>"`) —
        /// what an AGENT PRESET launch composes from its prefix, the task
        /// and its postfix. Request-only: never persisted, so a RESUME can
        /// never replay it.
        #[serde(default)]
        starting_prompt: Option<String>,
        /// The mode the CLI starts in — plan, ask, or the edit it always
        /// starts in ([`crate::harness::AgentMode`]). Request-only like
        /// `starting_prompt`: a RESUME starts where the session left off,
        /// and a mode the harness does not have is dropped.
        #[serde(default)]
        mode: crate::harness::AgentMode,
        /// The AUTOMATION starting this session (`Agent::automation`), or
        /// None for a launch of yours. Persisted with the row.
        #[serde(default)]
        automation: Option<String>,
    },
    /// Create a local AGENT of any kind from an OPEN PRS row — a PR
    /// SESSION. It never runs in the ROOT WORKTREE: the daemon finds the
    /// PROJECT's worktree checked out on the PR's head branch, or creates
    /// one (fetching the branch from `origin`, or the PR ref for a fork),
    /// and every PR SESSION for that PR shares it. The PR URL is persisted
    /// as launch context so every cold spawn and RESUME rebuilds the same
    /// PR-scoped rule — naming that worktree — as Claude's appended system
    /// prompt, or the first prompt a Codex / Cursor cold spawn opens with.
    /// Separate from CreateAgent so ordinary callers cannot accidentally
    /// opt into a partial PR launch. The reply's `created` is the AGENT;
    /// a created worktree arrives as its own `EntityUpserted` first.
    CreatePrAgent {
        req_id: u64,
        /// The PROJECT whose repo the pull request is open on.
        project: ProjectId,
        name: String,
        kind: AgentKind,
        /// The registry id beside `kind`: a Claude account's id, or a
        /// custom harness's (see `Agent::custom_harness`). Persisted with
        /// the row like `model`.
        #[serde(default)]
        custom_harness: Option<String>,
        /// Model the CLI launches with; None = the CLI's own default.
        model: Option<String>,
        /// Reasoning effort the CLI launches with; None = default.
        effort: Option<String>,
        auto_title: bool,
        pr_url: String,
        /// The branch the PR SESSION's worktree is checked out on: the
        /// pull request's head branch (`gh`'s `headRefName`) for a
        /// same-repo pull request, and `<owner>/<headRefName>` for a
        /// fork's — a fork's `main` is not ours, and must match neither
        /// the ROOT WORKTREE nor a branch on `origin`. The client names
        /// it; the daemon fetches `origin`'s branch of that name and,
        /// finding none, seeds it from `refs/pull/N/head`.
        head: String,
        /// The CLI's positional first prompt — an AGENT PRESET picked on
        /// the OPEN PRS row composes one, under `CreateAgent`'s rules for
        /// `starting_prompt`. It rides beside the PR rule, which stays
        /// Claude's appended system prompt (or the opening of a Codex /
        /// Cursor cold spawn's first prompt). None: the CLI's own input
        /// is the first prompt. Request-only, never persisted.
        #[serde(default)]
        starting_prompt: Option<String>,
        /// The mode the CLI starts in, under `CreateAgent`'s rules.
        #[serde(default)]
        mode: crate::harness::AgentMode,
        /// The AUTOMATION starting it, under `CreateAgent`'s rules.
        #[serde(default)]
        automation: Option<String>,
    },
    /// CHECK OUT A PULL REQUEST: the PROJECT's checkout of the pull
    /// request's head branch — found, or cut exactly as a PR SESSION's is
    /// (`head` under `CreatePrAgent`'s rules, a fork's seeded from
    /// `refs/pull/N/head`) — with no session in it. The reply's `created`
    /// is the WORKTREE; a cut one arrives as its own `EntityUpserted`.
    CreatePrWorktree {
        req_id: u64,
        project: ProjectId,
        pr_url: String,
        head: String,
    },
    /// Fire-and-forget: pre-spawn every dead (non-archived) session under a
    /// worktree so attaching later replays an already-booted screen instead
    /// of watching a login shell + CLI boot. Sent once the worktree
    /// selection has rested (debounced client-side); already-alive sessions
    /// are untouched. No reply; a failed spawn degrades to today's lazy
    /// spawn-on-attach.
    PrewarmWorktreeSessions {
        worktree: WorktreeId,
        /// Pane size the sessions boot at, so the later Attach resizes to
        /// the same grid and full-screen apps need no reflow.
        cols: u16,
        rows: u16,
    },
    RenameAgent {
        req_id: u64,
        id: AgentId,
        name: String,
    },
    /// Agent-initiated one-shot title (`orion rename` inside the session's
    /// CLI). Applies only while the session still awaits its auto-title;
    /// answered with Error (informational, not a fault) once a title —
    /// user- or agent-set — already sticks, so a user rename is never
    /// clobbered by a late or repeated agent attempt.
    AutoRenameAgent {
        req_id: u64,
        id: AgentId,
        name: String,
    },
    /// `orion worktree <name>`, run by the agent from inside its own
    /// session: create the worktree `branch` under the agent's project (or
    /// take the existing one with that branch), re-home the agent row under
    /// it at once, and relocate the live session into it when its current
    /// turn ends — killed and respawned resumed there, with a prompt that
    /// tells the CLI where it now is. Replies with `WorktreeEntered`.
    EnterWorktree {
        req_id: u64,
        id: AgentId,
        branch: String,
        base: Option<String>,
    },
    /// `orion spawn "<task>"`, run by the agent from inside its own
    /// session: start a new AGENT beside it — same WORKTREE, and the same
    /// AGENT KIND / MODEL / EFFORT unless `kind` names another harness —
    /// with `starting_prompt` as the new CLI's first prompt, so it begins
    /// the task at once. The caller's own process is untouched. Answered
    /// with `Ack { created: Some(EntityId::Agent(..)) }`; the row reaches
    /// every TUI as an ordinary `EntityUpserted`.
    ///
    /// `worktree` (`orion spawn --worktree <name>`) starts it in the
    /// worktree on that branch of the caller's project instead, created
    /// from `base` when the branch has none, the way `EnterWorktree` does.
    SpawnSiblingAgent {
        req_id: u64,
        id: AgentId,
        kind: Option<AgentKind>,
        starting_prompt: String,
        #[serde(default)]
        worktree: Option<String>,
        #[serde(default)]
        base: Option<String>,
    },
    /// `orion open <file>…`, run by the agent from inside its own session:
    /// show these files to the user in every attached TUI's FILE TABS —
    /// one tab per file, the focused one previewed, Enter editing it.
    /// Paths are absolute (the CLI resolves them against its own cwd, which
    /// is the agent's). Answered with `Ack`; the files reach every
    /// subscriber as `FilesOpened`.
    OpenFiles {
        req_id: u64,
        id: AgentId,
        paths: Vec<PathBuf>,
        /// One per path: the line a `file:42` asked for, None for the top.
        lines: Vec<Option<u64>>,
    },
    /// Kills the PTY, sets archived=1.
    ArchiveAgent {
        req_id: u64,
        id: AgentId,
    },
    UnarchiveAgent {
        req_id: u64,
        id: AgentId,
    },
    DeleteAgent {
        req_id: u64,
        id: AgentId,
    },
    /// Respawn; resumes the stored session id (`claude --resume` /
    /// `codex resume` / `cursor-agent --resume`) when one is stored.
    RestartAgent {
        req_id: u64,
        id: AgentId,
    },
    /// **Continue on** another account: carry a Claude-dialect session onto
    /// another Claude-dialect harness. The daemon stops its PTY,
    /// copies its transcript (`projects/<slug>/<id>.jsonl`, and the `<id>/`
    /// folder beside it) from the session's Claude config dir to the same
    /// place under `harness`'s — never over a different file, never
    /// deleting the source — switches the row to `harness` (name, worktree,
    /// and the model and effort `harness` offers, kept), and resumes the
    /// stored session id there. Refused for an archived row, a session
    /// with no transcript to move, and a `harness` (registry id) that
    /// cannot take one ([`crate::harness::HarnessDescriptor::takes_claude_sessions`]).
    /// Answered with `Ack`; the row reaches every client as an upsert.
    ContinueAgentOn {
        req_id: u64,
        id: AgentId,
        harness: String,
    },
    /// `text` as a local agent's next turn: typed down its PTY and
    /// submitted when its CLI is up; otherwise the CLI is booted (resuming
    /// the stored session) with `text` as the first prompt it submits on
    /// its own, at `cols`×`rows`. Either way the turn is sent — a session
    /// the IDLE REAPER put down never swallows it. Refused for archived
    /// rows. Answered with `Ack`.
    SendTurn {
        req_id: u64,
        id: AgentId,
        text: String,
        cols: u16,
        rows: u16,
    },
    /// What a session last said and why it is waiting, if it is: the
    /// text of the last assistant message in its transcript. Answered by
    /// `ServerEvent::LastResponse` with the same req_id (not an Ack).
    LastResponse {
        req_id: u64,
        id: AgentId,
    },
    /// Answer a session that is waiting on you (`WaitReason`): the
    /// keystrokes its question or plan dialog takes, typed into the live
    /// CLI. A session with no dialog open — it is not running, or the
    /// daemon restarted since it asked — gets the answer as its next turn,
    /// in words (`SendTurn`). Answered with `Ack`.
    Answer {
        req_id: u64,
        id: AgentId,
        answer: Answer,
    },
    CreateTerminal {
        req_id: u64,
        worktree: WorktreeId,
        name: Option<String>,
    },
    /// Rewrite a link's URL. It is normalized daemon-side (a bare
    /// `github.com/...` gains an https:// scheme) and refused if it can't
    /// be made into an http(s) URL.
    UpdateLink {
        req_id: u64,
        id: LinkId,
        url: String,
    },
    DeleteLink {
        req_id: u64,
        id: LinkId,
    },
    RenameTerminal {
        req_id: u64,
        id: TerminalId,
        name: String,
    },
    CloseTerminal {
        req_id: u64,
        id: TerminalId,
    },
    /// `r` on a worktree: start the project's RUN COMMAND — its **Run
    /// command** setting, read fresh — in the worktree's RUN TERMINAL, a login shell running that
    /// line and nothing else. An exited run's row is reused; a run still
    /// going is left alone and named in the reply, so a second client's
    /// press never starts a second one. Answered with
    /// `Ack { created: Some(EntityId::Terminal(..)) }`; no command is an
    /// Error saying where to set one.
    StartRun {
        req_id: u64,
        worktree: WorktreeId,
    },
    /// `r` again: a `^C` into the worktree's RUN TERMINAL, the row sent
    /// as stopping (`RunState::stopping_until`) and dropped once the run
    /// has wound down; a second `StopRun`, or two minutes, kills it first.
    /// Nothing running is not an error.
    StopRun {
        req_id: u64,
        worktree: WorktreeId,
    },
    /// **Restart stack**: `StopRun`, then `StartRun` in the same row once
    /// the run has gone (`RunState::restarting` meanwhile). A run that
    /// isn't up just starts; with no run command it is an Error, and
    /// nothing is stopped.
    RestartRun {
        req_id: u64,
        worktree: WorktreeId,
    },

    /// Fire-and-forget opaque TUI blob (last selection etc.).
    SaveUiState {
        json: String,
    },

    /// Fire-and-forget: the user just opened this pull request, so
    /// everything up to `marker` has now been read.
    MarkPrSeen {
        url: String,
        marker: String,
    },

    /// Fire-and-forget: this agent's session is on screen, so a turn it
    /// finished unwatched (`Agent::unseen`) has now been looked at. The
    /// daemon answers with the agent's upsert when the flag actually flips.
    MarkAgentSeen {
        id: AgentId,
    },

    /// Fire-and-forget: AWAY BY HAND (⌘⇧A) on or off. Off is also what a
    /// TUI sends on its first key after the flag went on — you are back.
    /// Every subscriber hears `AwayChanged`.
    SetAway {
        on: bool,
    },

    /// Fire-and-forget: the Slack tokens in the Keychain changed (the
    /// Connections tab saved or deleted them). The daemon's Slack bridge
    /// reads them again and reconnects, and every subscriber hears the
    /// new `SlackStatus`.
    SlackReconnect,

    /// Fire-and-forget: post a FACTORY CARD — a Slack message from orion's
    /// bot that no session's wait owns (a pull request's verdict, a thin
    /// ticket's question to its reporter, the daily rules). The daemon
    /// posts it to the owner's DM, or to the DM of whoever
    /// `CardRecipient::Email` names, and remembers where it went
    /// (`factory_messages`), so a second post with the same `key` edits
    /// that message in place rather than posting another. When `image` is
    /// set, the file is uploaded into the message's thread. A card that
    /// can't be posted is answered by `ServerEvent::FactoryCardFailed`.
    PostFactoryCard {
        card: FactoryCard,
    },

    /// Fire-and-forget: rewrite the factory card posted under `key` in
    /// place — the TUI's own wording after an action, or a card closed
    /// from Needs you. A key never posted is a no-op.
    UpdateFactoryCard {
        key: String,
        card: FactoryCard,
    },

    /// One point-in-time memory reading — the daemon plus every live
    /// session's process subtree. Answered by `ServerEvent::Metrics` with
    /// the same req_id (not an Ack).
    GetMetrics {
        req_id: u64,
    },

    Shutdown,
}

/// A reply to a session waiting on you ([`ClientRequest::Answer`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    /// Pick this option (0-based) of the question, or of the plan or
    /// permission dialog.
    Option(usize),
    /// Answer the question in your own words.
    Text(String),
    /// Approve the plan.
    ApprovePlan,
    /// Keep planning, with this feedback; on a question, the same as
    /// `Text`.
    Steer(String),
    /// Every question of an `AskUserQuestion` answered at once, one
    /// [`Picked`] per question in its order.
    Answers(Vec<Picked>),
}

/// One question's answer in [`Answer::Answers`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Picked {
    /// These options (0-based); one for a single-select question.
    Options(Vec<usize>),
    /// Your own words.
    Text(String),
}

/// How orion's Slack connection stands (`ServerEvent::SlackStatus`).
/// All None with `configured` false: no tokens saved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackStatus {
    /// The three tokens are saved.
    pub configured: bool,
    /// Signed in and listening: the workspace's name (`auth.test`'s
    /// `team`).
    pub workspace: Option<String>,
    /// The installing user's Slack id (`U…`): whose DMs and mentions the
    /// inbox reads, and who the phone messages go to.
    pub user_id: Option<String>,
    /// The DM channel orion's bot posts phone messages in (`D…`). Its
    /// messages are the phone's, not the inbox's.
    pub phone_channel: Option<String>,
    /// Why it is not connected, as one line to show.
    pub problem: Option<String>,
}

/// What a FACTORY CARD's buttons carry, which the TUI builds and the
/// daemon routes by.
pub mod factory_card {
    /// The `action_id` every factory button starts with (`<ACTION>_<n>`).
    pub const ACTION: &str = "orion_factory";
    /// The `action_id` prefix of a button that asks for words first: the
    /// daemon opens its text modal.
    pub const ACTION_TYPE: &str = "orion_factory_type";
    /// The `action_id` prefix of a button that opens its `url`: Slack
    /// opens it, and the daemon acks its press and does nothing else.
    pub const ACTION_LINK: &str = "orion_factory_link";
    /// Between a button value's key and its action (`<key>|<action>`).
    pub const SEP: char = '|';
}

/// Slack's caps on what a message may hold, in characters (or blocks):
/// the phone's messages and the factory's cards keep within them.
pub mod slack_caps {
    /// A section block's text.
    pub const SECTION: usize = 3000;
    /// A header block's text.
    pub const HEADER: usize = 150;
    /// A button's label.
    pub const BUTTON: usize = 75;
    /// The blocks in one message.
    pub const MAX_BLOCKS: usize = 50;
}

/// A FACTORY CARD (`ClientRequest::PostFactoryCard`): one Slack message
/// from orion's bot, built by the TUI, that no session's wait owns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactoryCard {
    /// The TUI's stable id for it, such as `pr:<url>:verdict` or
    /// `issue:<id>:thin`: a second post under the same key edits the
    /// message in place, and the key leads every button value
    /// (`<key>|<action>`).
    pub key: String,
    /// Whose DM it goes to.
    pub to: CardRecipient,
    /// The notification and fallback text.
    pub text: String,
    /// The Block Kit blocks, as Slack takes them.
    pub blocks: serde_json::Value,
    /// A picture uploaded into the message's thread — a PR's before and
    /// after grid.
    pub image: Option<PathBuf>,
}

/// Who a factory card is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardRecipient {
    /// The installing user, in the bot's DM with them (the phone's).
    Owner,
    /// Whoever has this email in the workspace (`users.lookupByEmail`),
    /// in the bot's DM with them — a thin ticket's reporter.
    Email(String),
}

/// Who pressed a factory card's button (`ServerEvent::FactoryAction`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackUser {
    /// Their Slack id (`U…`).
    pub id: String,
    /// They are the installing user — the owner.
    pub is_owner: bool,
}

/// Why a factory card was not posted (`ServerEvent::FactoryCardFailed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactoryCardFailure {
    /// `CardRecipient::Email` matched nobody in the workspace — an
    /// external reporter, or one not on Slack. The TUI falls back to a
    /// card to the owner.
    NotOnSlack,
    /// Slack is not connected, so nothing can be posted.
    NotConnected,
    /// The Slack app lacks a scope the factory needs (`users:read.email`,
    /// `files:write`): it wants reinstalling.
    MissingScope(String),
    /// Anything else Slack refused, as one line.
    Other(String),
}

/// What a new session launches as — its harness (a Claude account is
/// `Claude` with the account's id beside it), model, effort and mode, as
/// `CreateAgent` carries them. A SPARE WORKTREE's CLI was booted as one,
/// and only a launch that is the same one can take it over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub kind: AgentKind,
    pub custom_harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: crate::harness::AgentMode,
}

/// How much of a pull request's conversation the user had already seen the
/// last time they opened it. `marker` is the newest thing anyone else had
/// posted at that moment, as GitHub's RFC 3339 stamp — those sort
/// lexicographically, so "arrived since" is a string compare and orion
/// never has to consult a clock. Empty means the PR was opened while its
/// conversation was still empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrSeen {
    pub url: String,
    pub marker: String,
}

/// Memory usage of one live session: the PTY child plus every descendant
/// (an agent CLI typically fans out into node workers, shells, MCP servers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetrics {
    pub session: SessionRef,
    /// OS pid of the PTY child (the subtree's root).
    pub pid: u32,
    /// Resident set size summed over the whole subtree, bytes.
    pub rss_bytes: u64,
    /// Live processes in the subtree, the root included.
    pub procs: u32,
    /// The session is a SPARE WORKTREE's — its setup or its booted CLI,
    /// with no row a client knows: memory orion spends, not a session the
    /// user has.
    #[serde(default)]
    pub spare: bool,
}

/// Daemon-side half of the metrics modal's data; the client stacks its own
/// RSS on top. Session subtrees are daemon descendants, so `daemon_rss_bytes`
/// counts the daemon process alone — the total stays double-count-free.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub daemon_pid: u32,
    pub daemon_rss_bytes: u64,
    /// Physical memory installed on the machine, bytes; 0 = unknown.
    pub system_total_bytes: u64,
    pub sessions: Vec<SessionMetrics>,
}

/// What `EnterWorktree` did to the agent's live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnterOutcome {
    /// The agent already lived in that worktree; nothing changed.
    AlreadyThere,
    /// The row moved; the live session respawns inside the worktree, resumed,
    /// once its current turn ends.
    Relocating,
    /// The row moved and nothing was running: the next launch lands there.
    NextLaunch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerEvent {
    HelloOk {
        protocol_version: u32,
        daemon_pid: u32,
    },
    Incompatible {
        daemon_protocol_version: u32,
    },
    Snapshot {
        projects: Vec<Project>,
        worktrees: Vec<Worktree>,
        agents: Vec<Agent>,
        terminals: Vec<TerminalTab>,
        links: Vec<Link>,
        /// How far the user has read into each pull request they've opened.
        pr_seen: Vec<PrSeen>,
        ui_state: Option<String>,
    },

    Ack {
        req_id: u64,
        created: Option<EntityId>,
    },
    /// Reply to `EnterWorktree`: the worktree the agent now belongs to, and
    /// what that meant for its process.
    WorktreeEntered {
        req_id: u64,
        worktree: Worktree,
        outcome: EnterOutcome,
    },
    /// Reply to an unforced `DeleteWorktree` whose checkout has `files`
    /// files with uncommitted or untracked changes: no session was killed
    /// and nothing was removed. Only a `force: true` resend deletes it.
    WorktreeHasChanges {
        req_id: u64,
        id: WorktreeId,
        files: usize,
    },
    Error {
        req_id: Option<u64>,
        message: String,
    },

    // -- deltas (pushed to all subscribers) --
    EntityUpserted {
        entity: Entity,
    },
    EntityRemoved {
        id: EntityId,
    },
    StatusChanged {
        agent: AgentId,
        status: AgentStatus,
        /// Epoch ms the change was stamped with (matches the persisted
        /// `status_changed_at`, so clients regroup consistently).
        changed_at: i64,
        /// The agent's `unseen` flag after this change: set when a live
        /// turn just finished, cleared when it left `finished`.
        #[serde(default)]
        unseen: bool,
        /// Why it is waiting on you, when the change is into
        /// `needs_feedback` (a question, a plan, a permission prompt) or
        /// `terminated` (a crash) and the daemon knows. None otherwise.
        #[serde(default)]
        reason: Option<WaitReason>,
    },

    /// `orion open` from an agent session: the files the user asked to
    /// see, for every subscriber to raise its FILE TABS on. `root` is the
    /// agent's worktree checkout — the editor's cwd, and what the tab
    /// labels are relative to.
    FilesOpened {
        agent: AgentId,
        root: PathBuf,
        paths: Vec<PathBuf>,
        /// One per path, as `OpenFiles` asked: the line each tab opens on.
        lines: Vec<Option<u64>>,
    },

    /// STACK STATUS: every docker compose project on the machine, sent
    /// when it changes and right after `Snapshot`. `stacks: None` is docker
    /// out of reach, `error` saying why; `Some(vec![])` is no stacks.
    StacksChanged {
        stacks: Option<Vec<Stack>>,
        error: Option<String>,
    },

    // -- PTY plane (only to clients attached to that session) --
    /// Ring replay on attach; client resets its parser before applying.
    Scrollback {
        session: SessionRef,
        base_seq: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// Live coalesced output. `seq` = byte offset of the first byte.
    Output {
        session: SessionRef,
        seq: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    SessionExited {
        session: SessionRef,
        exit_code: Option<i32>,
    },
    /// The child's kitty-keyboard-protocol flags changed (or, right after
    /// Scrollback on attach, the current value). 0 = legacy encoding.
    KittyFlags {
        session: SessionRef,
        flags: u8,
    },

    /// Reply to `ClientRequest::GetMetrics`.
    Metrics {
        req_id: u64,
        snapshot: MetricsSnapshot,
    },

    /// AWAY changed (`orion_core::away`): you count as away, or are back.
    /// `by_hand` is the ⌘⇧A flag, which the next key in any TUI clears.
    /// Sent right after `Snapshot` too.
    AwayChanged {
        away: bool,
        by_hand: bool,
    },

    /// How the Slack connection stands, when it changes and right after
    /// `Snapshot`.
    SlackStatus {
        status: SlackStatus,
    },

    /// A Slack message event from the installing user's token — a DM, a
    /// group DM, a channel or a private channel message they can see
    /// (`message.im`, `message.mpim`, `message.channels`,
    /// `message.groups`) — forwarded as it came, unfiltered: the TUI
    /// decides what is for you.
    SlackMessage {
        /// The conversation (`C…`, `G…`, `D…`).
        channel: String,
        /// `im`, `mpim`, `channel` or `group`.
        channel_type: String,
        /// The sender's user id; None for a bot or a system message.
        user: Option<String>,
        text: String,
        /// The message's own timestamp, its id in the conversation.
        ts: String,
        /// The thread it is a reply in (the parent's `ts`), if it is one.
        thread_ts: Option<String>,
        /// `message_changed`, `bot_message`, … — None for a plain message.
        subtype: Option<String>,
        /// The whole Slack event as JSON, for anything the fields above
        /// leave out.
        event_json: String,
    },

    /// A button on a factory card was pressed (its value `<key>|<action>`),
    /// by the owner, or by the one non-owner the card was sent to. A
    /// "Type…" button's modal brings the words typed as `text`. Every TUI
    /// hears it; the card's owning module acts on it.
    FactoryAction {
        key: String,
        action: String,
        text: Option<String>,
        by: SlackUser,
    },

    /// `ClientRequest::PostFactoryCard` (or `UpdateFactoryCard`) under
    /// `key` was not posted, and why — so the TUI can fall back.
    FactoryCardFailed {
        key: String,
        reason: FactoryCardFailure,
    },

    /// Reply to `ClientRequest::LastResponse`: the text of the session's
    /// last assistant message (None when there is no transcript, or no
    /// text in it), and why it is waiting on you, if it is.
    LastResponse {
        req_id: u64,
        text: Option<String>,
        reason: Option<WaitReason>,
    },
}

#[cfg(test)]
mod stack_tests {
    use super::*;

    /// A factory card's blocks are JSON inside a msgpack frame: they come
    /// back as they went, and so does a press with its words.
    #[test]
    fn factory_frames_round_trip() {
        let card = FactoryCard {
            key: "pr:https://github.com/o/r/pull/7:verdict".into(),
            to: CardRecipient::Email("ann@example.com".into()),
            text: "Fix the login loop".into(),
            blocks: serde_json::json!([{"type": "section", "text": {"type": "mrkdwn",
                "text": "*Merge*"}}, {"type": "actions", "elements": [{"value": "k|merge"}]}]),
            image: Some(PathBuf::from("/tmp/grid.png")),
        };
        let req = ClientRequest::PostFactoryCard { card: card.clone() };
        let back: ClientRequest = rmp_serde::from_slice(&rmp_serde::to_vec(&req).unwrap()).unwrap();
        let ClientRequest::PostFactoryCard { card: back } = back else {
            panic!("{back:?}");
        };
        assert_eq!(back, card);

        let ev = ServerEvent::FactoryAction {
            key: card.key.clone(),
            action: "send_back".into(),
            text: Some("use the old flow".into()),
            by: SlackUser {
                id: "U2".into(),
                is_owner: false,
            },
        };
        let back: ServerEvent = rmp_serde::from_slice(&rmp_serde::to_vec(&ev).unwrap()).unwrap();
        let ServerEvent::FactoryAction { text, by, .. } = back else {
            panic!("{back:?}");
        };
        assert_eq!(text.as_deref(), Some("use the old flow"));
        assert!(!by.is_owner);
    }

    #[test]
    fn stack_frames_round_trip() {
        let ev = ServerEvent::StacksChanged {
            stacks: Some(vec![Stack {
                project: "riplo-wt".into(),
                dirs: vec![PathBuf::from("/src/wt")],
                running: 4,
                total: 5,
            }]),
            error: None,
        };
        let back: ServerEvent = rmp_serde::from_slice(&rmp_serde::to_vec(&ev).unwrap()).unwrap();
        let ServerEvent::StacksChanged {
            stacks: Some(s),
            error: None,
        } = back
        else {
            panic!("{back:?}");
        };
        assert_eq!((s[0].running, s[0].total), (4, 5));
    }
}
