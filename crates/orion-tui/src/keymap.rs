//! Rebindable application hotkeys.
//!
//! Every key the grid reacts to is an [`Action`] here, with a default
//! chord list. The Hotkeys tab of the settings overlay writes overrides
//! into the shared config JSON (`keybindings: { "<action id>": "j, down" }`),
//! and the event loop dispatches through [`Keymap::lookup`] instead of
//! matching raw `KeyCode`s.
//!
//! Two things this module knows that a naive keymap wouldn't:
//!
//! * **Normalization.** Terminals disagree about how a key arrives —
//!   `Shift+j` may come through as `Char('J')` with or without the shift
//!   bit, `Ctrl+]` is spelled `Ctrl+5` in the legacy encoding, `BackTab`
//!   is really `Shift+Tab`. [`KeyChord::from_event`] folds all of that into
//!   one canonical form so a binding matches whatever the emulator sends.
//!
//! * **Host reachability.** orion runs *inside* Terminal.app / Ghostty /
//!   tmux, which eat chords before we ever see them — Terminal.app every
//!   `⌘` combo, Ghostty the `⌘` ones it binds, most `^⇧` ones, `^←`.
//!   [`host_warning`] flags those at bind time so the user
//!   finds out at the moment of choosing, not the next time the key does
//!   nothing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

/// Which mode a binding is live in. The same chord may mean different
/// things in each, so conflicts are only conflicts within one scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The launcher grid, and a terminal pane that isn't input-locked.
    Global,
    /// Input-locked terminal pane, where every other key is forwarded to
    /// the child process.
    Terminal,
}

/// Everything a hotkey can do. Overlay-local keys (Esc to close, j/k inside
/// a picker, the line-editor bindings) are deliberately absent: they're the
/// modal grammar every overlay shares, not application hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // navigate
    FocusNext,
    FocusLeft,
    FocusRight,
    MoveDown,
    MoveUp,
    Activate,
    /// `⌘K`: the JUMP list — every project, worktree, session and open
    /// pull request, with adding a project as its last row.
    Palette,
    /// `.`: the next session in the PALETTE's attention order, no modal.
    NextAttention,
    /// `,`: the same walk backwards.
    PrevAttention,
    /// `]`: the LAUNCHER VIEW's next PROJECT TAB to the right — that
    /// project's sessions. The last tab goes no further.
    NextProjectTab,
    /// `[`: the tab to the left. The first tab goes no further.
    PrevProjectTab,
    /// `x`: close the PROJECT TAB the grid is on, landing on the tab that
    /// slides into its place.
    CloseProjectTab,
    /// The PROJECT DROPDOWN, the list the `+` after the PROJECT TABS drops
    /// — type to narrow it, Enter or a click opens the project. Unbound by
    /// default: a click on the `+` and ⌘K reach it.
    ProjectDropdown,
    // projects & worktrees
    AddProject,
    New,
    GitDiff,
    OpenRepo,
    /// `Shift+V`: the pull request of the session card under the cursor,
    /// in the browser — the `#42 title` line on the card. `v` lists the
    /// project's pull requests in orion; the shifted key goes to GitHub.
    OpenPullRequest,
    /// `Shift+I`: the GitHub issue the session card under the cursor was
    /// started from, in the browser. `i` lists the project's issues in
    /// orion; the shifted key goes to GitHub.
    OpenIssue,
    /// `Shift+T`: a terminal *outside* orion — a new Ghostty tab or
    /// Terminal.app window (the **Outside terminal** setting) in the
    /// selected worktree's directory, where `t` opens one inside orion; a
    /// silent no-op off macOS or over ssh.
    OpenOutsideTerminal,
    /// `Shift+R`: reload from GitHub now, past every timer — the project's
    /// open pull requests and issues, the worktree's PR, the one the pane
    /// is reading.
    RefreshPullRequests,
    /// `y`: reply — the COMMENT BOX on the card's pull request (the one
    /// `Shift+V` opens), or on the one the pane is reading, posted to
    /// GitHub with `gh pr comment` on Enter.
    CommentPullRequest,
    /// `i`: the ISSUES MODAL — the project's open GitHub issues, read in
    /// place, commented on, with a QUICK PROMPT or an AGENT PRESET launched
    /// on one.
    Issues,
    /// `v`: the PULL REQUESTS MODAL — the project's open pull requests,
    /// read in place, commented on, with a PR SESSION launched on one.
    PullRequests,
    /// `⌘L`: the LINEAR VIEW — open Linear issues assigned to you.
    Linear,
    /// `c`: the BRANCH SWITCHER — move the project's ROOT WORKTREE onto
    /// another branch, asking what to do with uncommitted changes.
    SwitchBranch,
    /// `Shift+Enter` / `Shift+O` / `Alt+Enter`, on the grid: fire the
    /// selected worktree's OPEN COMMAND — the project's **Open command**
    /// setting, else its `.orion.json` `open` (`open http://localhost:3000`,
    /// say). Three chords because only the kitty protocol carries a shifted
    /// Enter: Terminal.app sends a plain one and tmux flattens it, while
    /// `ESC CR` — what `/terminal-setup` gives Shift+Enter in VS Code, and
    /// Option+Enter on a Mac with Option as Meta — gets through both and
    /// parses as Alt+Enter. Its RUN COMMAND is the project menu's **Run**.
    OpenWorktree,
    // sessions
    NewTerminal,
    Rename,
    Archive,
    Unarchive,
    ToggleArchived,
    Delete,
    DeleteAll,
    /// The AGENT PRESETS list: saved launch definitions for the checkout
    /// under the cursor.
    AgentPresets,
    /// The QUICK PROMPT: type a task, launch an agent on it.
    QuickPrompt,
    /// `Space`: the FOLLOW-UP MODAL for the selected session card — its
    /// next turn, typed in a box over the grid and sent on Enter.
    FollowUp,
    /// `Shift+P`: the QUICK PROMPT on the card under the cursor's settings
    /// — the same harness, model, effort and worktree, and the issue it
    /// was started from — so the task is all there is to type. `p` opens
    /// the box on the Agents tab defaults; the shifted key opens it on
    /// the card.
    DuplicateSession,
    // files
    FindFile,
    Grep,
    TreeBrowser,
    // terminal
    UnlockTerminal,
    // general
    /// `^~`: fold the LAUNCHER VIEW's PANE away and give the cards the
    /// whole body, or bring it back. Folding it also unselects the card
    /// under the cursor, so nothing is selected and nothing is read.
    ToggleLauncherPane,
    /// `^F`: FULL-SCREEN the session in the PANE — the grid and its header
    /// give way to the PTY — or bring it back down to the pane beside the
    /// cards. From the cards it full-screens the one under the cursor.
    ToggleFullScreen,
    /// `` ` ``: walk the TERMINAL cards of the cursor's checkout, one after
    /// another and back round to the first. What the pane READS, where
    /// `^~` is whether it is drawn at all.
    PaneTabs,
    /// Open the Nth PROJECT TAB (1-based) in the LAUNCHER VIEW's header.
    SelectProjectTab(u8),
    Hosts,
    Settings,
    Metrics,
    Help,
    /// `⌘⇧P`: the COMMAND PALETTE — every action by name, with its key.
    CommandPalette,
    /// `⌘O`: the OPEN MENU — what is under the cursor, outside orion: the
    /// repo, pull request or issue on GitHub, the open command, Cursor,
    /// the outside terminal.
    OpenOutside,
    /// The checkout under the cursor in Cursor (the OPEN MENU's row).
    OpenInCursor,
    Quit,
}

/// One rebindable row: what it does, where it's live, and what it starts as.
pub struct ActionSpec {
    pub action: Action,
    /// Stable key in the config file's `keybindings` object.
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    /// Section header in the Hotkeys tab.
    pub group: &'static str,
    pub scope: Scope,
    pub defaults: &'static [&'static str],
}

/// One positional PROJECT TAB shortcut: the bare digit. `⌘N` is what a
/// browser user reaches for, but Ghostty keeps ⌘1–⌘9 for its own tabs and
/// Terminal.app never sends ⌘ at all; digits are otherwise unbound on the
/// grid.
macro_rules! project_tab_slot {
    ($n:literal, $id:literal, $label:literal, $digit:literal) => {
        ActionSpec {
            action: Action::SelectProjectTab($n),
            id: $id,
            label: $label,
            hint: "Open the project on that tab of the header, counting from the left",
            group: "NAVIGATE",
            scope: Scope::Global,
            defaults: &[$digit],
        }
    };
}

pub const ACTIONS: &[ActionSpec] = &[
    // ---- NAVIGATE ----
    ActionSpec {
        action: Action::FocusNext,
        id: "focus_next",
        label: "Open / fold checkout",
        hint: "Open every card of the checkout under the cursor in place, or fold it back to one row — one checkout open at a time",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["tab"],
    },
    ActionSpec {
        action: Action::FocusLeft,
        id: "focus_left",
        label: "Focus left",
        hint: "Step to the card on the left, stopping at the row's first",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["left"],
    },
    ActionSpec {
        action: Action::FocusRight,
        id: "focus_right",
        label: "Focus right",
        hint: "Step to the card on the right, stopping at the row's last",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["right"],
    },
    ActionSpec {
        action: Action::MoveDown,
        id: "move_down",
        label: "Move down",
        hint: "Move the cursor down a row of cards; twice from the project tabs, into the one under their cursor",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["down"],
    },
    ActionSpec {
        action: Action::MoveUp,
        id: "move_up",
        label: "Move up",
        hint: "Move the cursor up a row of cards, stopping at the first; twice there, up to the project tabs",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["up"],
    },
    ActionSpec {
        action: Action::Activate,
        id: "activate",
        label: "Open / attach",
        hint: "Into the pane on the card under the cursor, or lock the pane's input",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["enter"],
    },
    ActionSpec {
        action: Action::Palette,
        id: "palette",
        label: "Jump to…",
        hint: "Search every project, worktree, session and open pull request at once; Enter jumps there, and the last row adds a project",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["cmd+k", "ctrl+k"],
    },
    ActionSpec {
        action: Action::NextAttention,
        id: "next_attention",
        label: "Next session needing you",
        hint: "Jump to the next session in the palette's attention order (needs feedback, running, unseen, then recency) in any project, wrapping",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["."],
    },
    ActionSpec {
        action: Action::PrevAttention,
        id: "prev_attention",
        label: "Prev session needing you",
        hint: "The same walk backwards: the session before this one in the palette's attention order, wrapping at the top",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &[","],
    },
    ActionSpec {
        action: Action::NextProjectTab,
        id: "next_project_tab",
        label: "Next project tab",
        hint: "Open the project on the tab to the right of this one in the header, stopping at the last",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["]"],
    },
    ActionSpec {
        action: Action::PrevProjectTab,
        id: "prev_project_tab",
        label: "Prev project tab",
        hint: "Open the project on the tab to the left, stopping at the first",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["["],
    },
    project_tab_slot!(1, "project_tab_1", "Project tab 1", "1"),
    project_tab_slot!(2, "project_tab_2", "Project tab 2", "2"),
    project_tab_slot!(3, "project_tab_3", "Project tab 3", "3"),
    project_tab_slot!(4, "project_tab_4", "Project tab 4", "4"),
    project_tab_slot!(5, "project_tab_5", "Project tab 5", "5"),
    project_tab_slot!(6, "project_tab_6", "Project tab 6", "6"),
    project_tab_slot!(7, "project_tab_7", "Project tab 7", "7"),
    project_tab_slot!(8, "project_tab_8", "Project tab 8", "8"),
    project_tab_slot!(9, "project_tab_9", "Project tab 9", "9"),
    ActionSpec {
        action: Action::CloseProjectTab,
        id: "close_project_tab",
        label: "Close project tab",
        hint: "Drop this project's tab from the header and open the tab beside it. Nothing is deleted — ⌘K opens it again",
        group: "NAVIGATE",
        scope: Scope::Global,
        defaults: &["x"],
    },
    ActionSpec {
        action: Action::ProjectDropdown,
        id: "project_dropdown",
        label: "Switch project",
        hint: "Drop the list of every project under the + in the header — type to narrow it, Enter or a click opens one. ⌘K reaches every project too",
        group: "NAVIGATE",
        scope: Scope::Global,
        // No key of its own: the header's `+` is a button, and ⌘K's jump
        // list holds every project the dropdown does.
        defaults: &[],
    },
    // ---- PROJECTS & WORKTREES ----
    ActionSpec {
        action: Action::AddProject,
        id: "add_project",
        label: "Open a folder as a project",
        hint: "Open a folder as a project in orion, from anywhere — ⌘K's last row",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::New,
        id: "new",
        label: "New session",
        hint: "Pick the harness first (→ drills into its model and effort), then the quick prompt opens set to it, in the checkout under the cursor; on the first-run splash, add a project",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::GitDiff,
        id: "git_diff",
        label: "Changes",
        hint: "Open the diff viewer for the checkout under the cursor — or, with the pane reading a pull request, that pull request's diff",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["cmd+e", "ctrl+e"],
    },
    ActionSpec {
        action: Action::OpenRepo,
        id: "open_repo",
        label: "Open repo in browser",
        hint: "Send the selected repo's git remote (GitHub, GitLab, …) to your browser",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::OpenPullRequest,
        id: "open_pull_request",
        label: "Open pull request in browser",
        hint: "Send the pull request of the session card under the cursor — its checkout's branch — to your browser, as a click on the card's #42 line does. v lists the pull requests in orion",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::OpenIssue,
        id: "open_issue",
        label: "Open issue in browser",
        hint: "Send the GitHub issue the session card under the cursor was started from to your browser. i lists the issues in orion",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::RefreshPullRequests,
        id: "refresh_pull_requests",
        label: "Reload from GitHub",
        hint: "Ask GitHub again now for the project's open pull requests and issues, the selected worktree's PR and the one the pane is reading",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["cmd+r", "ctrl+r"],
    },
    ActionSpec {
        action: Action::CommentPullRequest,
        id: "comment_pull_request",
        label: "Comment on pull request",
        hint: "Open a box to type a comment and post it through gh on the card's pull request, or, with the pane reading a pull request (a ⌘K jump lands on one), on that one",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::Issues,
        id: "issues",
        label: "GitHub issues",
        hint: "List the project's open issues; Enter prompts an agent on one, ⇧Tab launches a preset on it",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["i"],
    },
    ActionSpec {
        action: Action::PullRequests,
        id: "pull_requests",
        label: "GitHub pull requests",
        hint: "List the project's open pull requests; Enter prompts a PR session on one, ⇧Tab launches a preset on it, Tab picks a harness",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["v"],
    },
    ActionSpec {
        action: Action::Linear,
        id: "linear",
        label: "Linear issues",
        hint: "List Linear issues assigned to you; Space marks, Enter starts one agent on the marked set in one worktree",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["cmd+l", "ctrl+l"],
    },
    ActionSpec {
        action: Action::SwitchBranch,
        id: "switch_branch",
        label: "Switch root branch",
        hint: "Fuzzy-pick a branch or remote for the project's root checkout; uncommitted changes get stash / bring / commit / discard",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["c"],
    },
    ActionSpec {
        action: Action::OpenWorktree,
        id: "open_worktree",
        label: "Open checkout in editor",
        hint: "Open the selected checkout outside orion, usually in your editor: its project's Open command (Settings → Project), else .orion.json \"open\" — e.g. open http://localhost:3000",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    // ---- SESSIONS ----
    ActionSpec {
        action: Action::NewTerminal,
        id: "new_terminal",
        label: "New shell terminal",
        hint: "Spawn a plain shell inside orion, in the selected worktree's directory; ⌘O opens one outside it",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["t"],
    },
    // `t`'s shift pair: the same terminal, outside orion.
    ActionSpec {
        action: Action::OpenOutsideTerminal,
        id: "open_outside_terminal",
        label: "Terminal outside orion",
        hint: "Open the selected worktree's directory in the Outside terminal setting's app — a Ghostty tab or a Terminal.app window",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::Rename,
        id: "rename",
        label: "Rename",
        hint: "Rename the session under the cursor (the checkout's run command is the menu's Run / Stop)",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["r"],
    },
    ActionSpec {
        action: Action::Archive,
        id: "archive",
        label: "Archive / unarchive",
        hint: "Archive the selected agent (its PTY is released), or bring an archived one back",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["a"],
    },
    ActionSpec {
        action: Action::Unarchive,
        id: "unarchive",
        label: "Unarchive session",
        hint: "Bring an archived agent back into the list",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::ToggleArchived,
        id: "toggle_archived",
        label: "Show / hide archived",
        hint: "Swap the grid between the live sessions and the archived ones",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+a"],
    },
    ActionSpec {
        action: Action::Delete,
        id: "delete",
        label: "Delete selected",
        hint: "Remove the selected row, behind a confirmation. With the PROJECT TABS holding the keys, close the tab under their cursor, behind the same kind of confirmation — x closes it outright",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["backspace"],
    },
    ActionSpec {
        action: Action::DeleteAll,
        id: "delete_all",
        label: "Delete all sessions",
        hint: "Remove every session listed for the checkout under the cursor, behind a confirmation",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::AgentPresets,
        id: "agent_presets",
        label: "Agent presets",
        hint: "Saved launch presets (CLI, model, effort, prefix/postfix) for the checkout under the cursor; Enter asks for an optional task, or skips it; from the pull requests list, a PR session in that branch's worktree",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::QuickPrompt,
        id: "quick_prompt",
        label: "New agent",
        hint: "Type a task; Enter starts an agent on it (Tab picks the harness, model and effort; Settings → Agents sets the default)",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["cmd+i", "cmd+n", "ctrl+n"],
    },
    ActionSpec {
        action: Action::FollowUp,
        id: "follow_up",
        label: "Follow-up prompt",
        hint: "Open a box over the grid for the selected session's next turn, sent to the running agent on Enter; the pane stays as it is",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["space"],
    },
    ActionSpec {
        action: Action::DuplicateSession,
        id: "duplicate_session",
        label: "Duplicate session",
        hint: "Open the quick prompt set to launch what the selected card runs — the same harness, model, effort and worktree — so only the task is left to type",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &[],
    },
    // ---- FILES ----
    ActionSpec {
        action: Action::FindFile,
        id: "find_file",
        label: "Go to file",
        hint: "Fuzzy file finder for the selected worktree",
        group: "FILES",
        scope: Scope::Global,
        defaults: &["cmd+p", "ctrl+p"],
    },
    ActionSpec {
        action: Action::Grep,
        id: "grep",
        label: "Find in files",
        hint: "git grep across the selected worktree",
        group: "FILES",
        scope: Scope::Global,
        // ⌘⇧F as in Cursor; `^⇧F` needs the kitty protocol, so `⇧F` is
        // the twin a stock terminal delivers.
        defaults: &["cmd+shift+f", "ctrl+shift+f", "shift+f"],
    },
    ActionSpec {
        action: Action::TreeBrowser,
        id: "tree_browser",
        label: "File tree browser",
        hint: "Browse the worktree's files with a preview pane",
        group: "FILES",
        scope: Scope::Global,
        defaults: &["cmd+b", "ctrl+b"],
    },
    // ---- TERMINAL ----
    ActionSpec {
        action: Action::UnlockTerminal,
        id: "unlock_terminal",
        label: "Unlock terminal input",
        hint: "Leave the locked pane and go back to the card (Esc leaves; ⇧Esc sends Esc to the agent; ^q always works)",
        group: "TERMINAL",
        scope: Scope::Terminal,
        defaults: &["esc", "ctrl+q", "ctrl+]", "ctrl+shift+h"],
    },
    // ---- GENERAL ----
    ActionSpec {
        action: Action::ToggleLauncherPane,
        id: "toggle_launcher_pane",
        label: "Toggle pane",
        hint: "Fold the pane under the cards away — which also unselects the card it was reading — or bring it back. ⌘J works from inside the pane too",
        group: "GENERAL",
        scope: Scope::Global,
        // ⌘J, as in Cursor, reaches orion from inside the pane as well —
        // no one types a ⌘ chord as text. `^J` is its twin for terminals
        // that never send ⌘, from the cards only: in the pane it is the
        // agent's newline.
        defaults: &["cmd+j", "ctrl+j"],
    },
    ActionSpec {
        action: Action::ToggleFullScreen,
        id: "toggle_full_screen",
        label: "Full-screen session",
        hint: "Give the session in the pane the whole screen, or bring it back down beside the cards — from inside the pane too, where it is never forwarded to the agent. From the cards it full-screens the one under the cursor. Esc and ^q also bring a full-screen session back down",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["cmd+f", "ctrl+f"],
    },
    ActionSpec {
        action: Action::PaneTabs,
        id: "pane_tabs",
        label: "Terminals in the pane",
        hint: "Walk the cursor through the terminal cards of its checkout, one after another and back round to the first — the pane reads each",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["`"],
    },
    ActionSpec {
        action: Action::Hosts,
        id: "hosts",
        label: "SSH hosts",
        hint: "Connect to a saved ssh host (restarts orion over ssh)",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::Settings,
        id: "settings",
        label: "Settings",
        hint: "Open this settings overlay",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["s", "cmd+,"],
    },
    ActionSpec {
        action: Action::Metrics,
        id: "metrics",
        label: "Memory usage",
        hint: "RAM used by orion and every live agent's process tree",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::Help,
        id: "help",
        label: "Keyboard shortcuts",
        hint: "Every key orion answers to, on one page",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::CommandPalette,
        id: "command_palette",
        label: "Command palette",
        hint: "Every action by name, with its key: type to narrow, Enter runs it",
        group: "GENERAL",
        scope: Scope::Global,
        // `:` — vim's command line — is the key a stock terminal
        // delivers: it sends neither ⌘ nor `^⇧`.
        defaults: &["cmd+shift+p", "ctrl+shift+p", ":"],
    },
    ActionSpec {
        action: Action::OpenOutside,
        id: "open_outside",
        label: "Open outside orion",
        hint: "A menu of what the cursor is on, outside orion: the repo, pull request or issue on GitHub, the checkout's open command, Cursor, a terminal",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["cmd+o", "ctrl+o"],
    },
    ActionSpec {
        action: Action::OpenInCursor,
        id: "open_in_cursor",
        label: "Open checkout in Cursor",
        hint: "Open the checkout under the cursor in Cursor (⌘O on a file opens the file)",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::Quit,
        id: "quit",
        label: "Quit orion",
        hint: "Leave the TUI (sessions keep running in the daemon)",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["q", "ctrl+c"],
    },
];

pub fn spec_at(index: usize) -> Option<&'static ActionSpec> {
    ACTIONS.get(index)
}

pub fn index_of(action: Action) -> Option<usize> {
    ACTIONS.iter().position(|s| s.action == action)
}

/// The row for an action: its label and hint, for anything that names
/// what a key did (the KEY COMBO DISPLAY, for one).
pub fn spec_of(action: Action) -> Option<&'static ActionSpec> {
    index_of(action).and_then(spec_at)
}

// ---- chords ----

/// A single key press: one key plus the modifiers held with it, in the one
/// canonical spelling [`KeyChord::from_event`] produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

/// The modifiers a binding can name. META/HYPER are dropped: no terminal
/// reports them for a key a user could reasonably choose.
const KEPT_MODS: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SHIFT)
    .union(KeyModifiers::SUPER);

impl KeyChord {
    /// Canonicalize a crossterm event into a chord, so the same physical
    /// press compares equal however the emulator spelled it:
    ///
    /// * `Char('J')` and `shift + Char('j')` both become `shift+j`;
    /// * shift is dropped from punctuation and digits, where the glyph
    ///   already carries it (`?` is `?`, never `shift+?`);
    /// * `BackTab` becomes `shift+tab`;
    /// * `ctrl+5` — the legacy encoding's name for byte 0x1D — becomes
    ///   `ctrl+]`, which is what the user actually pressed.
    pub fn from_event(key: &KeyEvent) -> Self {
        let mut mods = key.modifiers & KEPT_MODS;
        let mut code = key.code;
        match code {
            KeyCode::Char(c) => {
                if c.is_alphabetic() {
                    if c.is_uppercase() {
                        mods |= KeyModifiers::SHIFT;
                    }
                    code = KeyCode::Char(c.to_lowercase().next().unwrap_or(c));
                } else {
                    mods.remove(KeyModifiers::SHIFT);
                }
            }
            KeyCode::BackTab => {
                code = KeyCode::Tab;
                mods |= KeyModifiers::SHIFT;
            }
            _ => {}
        }
        if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('5') {
            code = KeyCode::Char(']');
        }
        Self { code, mods }
    }

    /// Parse a config-file spelling (`"ctrl+shift+f"`, `"shift+tab"`, `"/"`).
    /// Unknown key names return None, which the loader reports and skips.
    pub fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            return None;
        }
        // '+' is both the separator and a bindable key, so a chord ending
        // in one — "+" alone, or "ctrl++" — names the plus key itself.
        let (mod_part, name) = match spec.strip_suffix('+') {
            Some(rest) => (rest.trim_end_matches('+'), "+".to_string()),
            None => match spec.rfind('+') {
                Some(i) => (&spec[..i], spec[i + 1..].trim().to_lowercase()),
                None => ("", spec.to_lowercase()),
            },
        };
        let mut mods = KeyModifiers::NONE;
        for part in mod_part.split('+').filter(|p| !p.trim().is_empty()) {
            mods |= match part.trim().to_lowercase().as_str() {
                "ctrl" | "control" | "^" => KeyModifiers::CONTROL,
                "alt" | "opt" | "option" | "meta" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                "cmd" | "super" | "win" => KeyModifiers::SUPER,
                _ => return None,
            };
        }
        let code = parse_key_name(&name)?;
        Some(Self::from_event(&KeyEvent::new(code, mods)))
    }

    /// Config-file spelling. Round-trips through [`KeyChord::parse`].
    pub fn spec(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push_str("ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("alt+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push_str("shift+");
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            out.push_str("cmd+");
        }
        out.push_str(&key_name(self.code));
        out
    }

    /// Compact on-screen spelling: `^q`, `⇧Tab`, `⌥p`, `⌘k`, `↓`.
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push('^');
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push('⌥');
        }
        if self.mods.contains(KeyModifiers::SUPER) {
            out.push('⌘');
        }
        let shift = self.mods.contains(KeyModifiers::SHIFT);
        match self.code {
            // A shifted letter reads best as the capital itself.
            KeyCode::Char(c) if shift && c.is_alphabetic() => {
                out.push('⇧');
                out.extend(c.to_uppercase());
            }
            // macOS spells ⌘ chords with the capital: ⌘K, not ⌘k.
            KeyCode::Char(c) if self.mods.contains(KeyModifiers::SUPER) && c.is_alphabetic() => {
                out.extend(c.to_uppercase());
            }
            _ => {
                if shift {
                    out.push('⇧');
                }
                out.push_str(&key_display(self.code));
            }
        }
        out
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

/// The named keys, each with its config spelling, its on-screen glyph, and
/// the alternate config spellings `parse` also accepts — one table so the
/// three lookups below can never disagree about a key. Letters, function
/// keys and BackTab are handled by the arms around the lookups instead.
const KEY_NAMES: &[KeyRow] = &[
    (KeyCode::Up, "up", "↑", &[]),
    (KeyCode::Down, "down", "↓", &[]),
    (KeyCode::Left, "left", "←", &[]),
    (KeyCode::Right, "right", "→", &[]),
    (KeyCode::Enter, "enter", "Enter", &["return", "cr"]),
    (KeyCode::Tab, "tab", "Tab", &[]),
    (KeyCode::Esc, "esc", "Esc", &["escape"]),
    (KeyCode::Char(' '), "space", "Space", &[]),
    (KeyCode::Backspace, "backspace", "⌫", &["bs"]),
    (KeyCode::Delete, "delete", "Del", &["del"]),
    (KeyCode::Insert, "insert", "Ins", &["ins"]),
    (KeyCode::Home, "home", "Home", &[]),
    (KeyCode::End, "end", "End", &[]),
    (KeyCode::PageUp, "pgup", "PgUp", &["pageup"]),
    (KeyCode::PageDown, "pgdn", "PgDn", &["pagedown"]),
];

/// One `KEY_NAMES` row: `(code, config spelling, glyph, alternate spellings)`.
type KeyRow = (KeyCode, &'static str, &'static str, &'static [&'static str]);

/// The `KEY_NAMES` row for `code`, with BackTab folded onto Tab — the
/// shift bit carries the difference, so both spell and display as `tab`.
fn key_row(code: KeyCode) -> Option<&'static KeyRow> {
    let code = if code == KeyCode::BackTab {
        KeyCode::Tab
    } else {
        code
    };
    KEY_NAMES.iter().find(|(c, ..)| *c == code)
}

fn parse_key_name(name: &str) -> Option<KeyCode> {
    if name == "backtab" {
        return Some(KeyCode::BackTab);
    }
    if let Some((code, ..)) = KEY_NAMES
        .iter()
        .find(|(_, canonical, _, aliases)| *canonical == name || aliases.contains(&name))
    {
        return Some(*code);
    }
    if let Some(n) = name.strip_prefix('f') {
        if let Ok(n) = n.parse::<u8>() {
            if (1..=20).contains(&n) {
                return Some(KeyCode::F(n));
            }
        }
    }
    let mut chars = name.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(KeyCode::Char(c))
}

fn key_name(code: KeyCode) -> String {
    if let Some((_, name, ..)) = key_row(code) {
        return (*name).into();
    }
    match code {
        KeyCode::Char(c) => c.to_lowercase().to_string(),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn key_display(code: KeyCode) -> String {
    if let Some((_, _, glyph, _)) = key_row(code) {
        return (*glyph).into();
    }
    match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    }
}

// ---- host-terminal reachability ----

/// How likely a chord is to actually reach orion from inside the user's
/// terminal emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Every emulator delivers it.
    Fine,
    /// Arrives in some setups only — worth saying so, still bindable.
    Risky,
    /// Effectively never arrives: the emulator or the OS takes it first.
    Blocked,
}

impl Reach {
    pub fn is_fine(self) -> bool {
        self == Reach::Fine
    }
}

/// Ctrl+letter chords whose legacy control byte is another key's, with the
/// warning each earns — a table so adding one can't drift the wording.
/// The message returns as `&'static str`, hence full strings per entry.
const CTRL_COLLISIONS: &[(char, &str)] = &[
    (
        'm',
        "^m is the same byte as Enter in terminals without the kitty protocol",
    ),
    (
        'i',
        "^i is the same byte as Tab in terminals without the kitty protocol",
    ),
    (
        '[',
        "^[ is the same byte as Esc in terminals without the kitty protocol",
    ),
    (
        'h',
        "^h is the same byte as ⌫ in terminals without the kitty protocol",
    ),
];

static CMD_SHOWN: AtomicBool = AtomicBool::new(false);

/// Whether Help, the footers and the palettes print the ⌘ chords: set
/// once at startup, when orion runs inside a terminal that sends ⌘.
pub fn set_cmd_shown(shown: bool) {
    CMD_SHOWN.store(shown, Ordering::Relaxed);
}

/// Whether the hints print ⌘ chords ([`set_cmd_shown`]).
pub fn cmd_shown() -> bool {
    CMD_SHOWN.load(Ordering::Relaxed)
}

/// [`Keymap::shown_chords`]'s pick: the ⌘ chords of `all` when `cmd`,
/// the rest otherwise — or all of them, when that side has none.
fn shown_side(all: &[KeyChord], cmd: bool) -> Vec<KeyChord> {
    let side: Vec<KeyChord> = all
        .iter()
        .filter(|c| c.mods.contains(KeyModifiers::SUPER) == cmd)
        .copied()
        .collect();
    if side.is_empty() {
        all.to_vec()
    } else {
        side
    }
}

/// The ⌘ chords Ghostty binds outright by default (Ghostty 1.3), in
/// orion's spelling: they never reach a program running inside it. Its
/// `performable` bindings — ⌘C copy, ⌘K clear, ⌘J / ⌘E on a selection,
/// ⌘⇧F / ⌘G on a search — pass the key on when there is nothing to do, so
/// they are not here; nor are the three [`crate::ghostty_config::UNBINDS`]
/// orion releases.
const GHOSTTY_KEEPS: &[&str] = &[
    "cmd+v",
    "cmd+=",
    "cmd++",
    "cmd+-",
    "cmd+0",
    "cmd+shift+j",
    "cmd+1",
    "cmd+2",
    "cmd+3",
    "cmd+4",
    "cmd+5",
    "cmd+6",
    "cmd+7",
    "cmd+8",
    "cmd+9",
    "cmd+enter",
    "shift+cmd+enter",
    "cmd+q",
    "cmd+a",
    "shift+cmd+t",
    "cmd+z",
    "shift+cmd+z",
    "cmd+home",
    "cmd+end",
    "cmd+pgup",
    "cmd+pgdn",
    "shift+cmd+up",
    "shift+cmd+down",
    "cmd+up",
    "cmd+down",
    "cmd+left",
    "cmd+right",
    "cmd+backspace",
    "cmd+w",
    "shift+cmd+w",
    "alt+cmd+w",
    "cmd+t",
    "cmd+[",
    "cmd+]",
    "cmd+d",
    "shift+cmd+d",
    "cmd+f",
    "alt+cmd+i",
    "ctrl+cmd+f",
    "shift+cmd+v",
];

/// Whether the host terminal is likely to swallow `chord` before orion
/// sees it, and why. orion is always a guest inside Terminal.app, Ghostty,
/// iTerm, tmux or an ssh session, and each of those claims keys for itself
/// — so the honest answer here is a probability, not a fact. Nothing is
/// rejected on the strength of it; the settings overlay just says so.
pub fn host_warning(chord: &KeyChord) -> (Reach, Option<&'static str>) {
    let m = chord.mods;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let shift = m.contains(KeyModifiers::SHIFT);
    let alt = m.contains(KeyModifiers::ALT);

    if m.contains(KeyModifiers::SUPER) {
        let listed = |specs: &[&str]| {
            specs
                .iter()
                .filter_map(|spec| KeyChord::parse(&spec.replace("super+", "cmd+")))
                .any(|listed| listed == *chord)
        };
        if listed(crate::ghostty_config::UNBINDS) {
            return (
                Reach::Risky,
                Some("Ghostty gives this ⌘ chord to orion only with Settings → General → Ghostty keybinds on; Terminal.app never sends ⌘"),
            );
        }
        if listed(GHOSTTY_KEEPS) {
            return (
                Reach::Blocked,
                Some("Ghostty keeps this ⌘ chord for itself (ghostty +list-keybinds --default)"),
            );
        }
        return (
            Reach::Risky,
            Some("⌘ reaches orion in Ghostty and kitty only — Terminal.app and orion browser never send it; the ^ twin works everywhere"),
        );
    }
    // Mission Control owns these on stock macOS (they switch Spaces).
    if ctrl && matches!(chord.code, KeyCode::Left | KeyCode::Right) && !shift && !alt {
        return (
            Reach::Risky,
            Some("macOS Mission Control takes ^← / ^→ unless you turn its Spaces shortcuts off"),
        );
    }
    // Legacy encoding has no shifted Enter: without the kitty protocol the
    // terminal sends the same \r a bare Enter does.
    if shift && !ctrl && !alt && chord.code == KeyCode::Enter {
        return (
            Reach::Risky,
            Some("⇧Enter needs the kitty keyboard protocol — Ghostty/kitty send it, Terminal.app sends a plain Enter and tmux flattens it to one; ⇧O and ⌥Enter arrive everywhere"),
        );
    }
    if ctrl && shift {
        return (
            Reach::Risky,
            Some(
                "^⇧ needs the kitty keyboard protocol — Ghostty/kitty send it, Terminal.app won't",
            ),
        );
    }
    if ctrl {
        // Legacy control bytes that collide with a key of their own.
        if let Some((_, why)) = CTRL_COLLISIONS
            .iter()
            .find(|(c, _)| chord.code == KeyCode::Char(*c))
        {
            return (Reach::Risky, Some(why));
        }
        match chord.code {
            KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Esc => {
                return (
                    Reach::Risky,
                    Some("ctrl + this key needs the kitty keyboard protocol to be distinguishable"),
                )
            }
            // Only a handful of punctuation has a control byte at all.
            KeyCode::Char(c)
                if !c.is_ascii_alphabetic()
                    && !matches!(c, ' ' | '@' | '[' | '\\' | ']' | '^' | '_' | '/') =>
            {
                return (
                    Reach::Risky,
                    Some("most terminals have no encoding for ctrl + this key"),
                )
            }
            _ => {}
        }
    }
    if alt {
        return (
            Reach::Risky,
            Some("⌥ only arrives if your terminal sends Option as Meta (Terminal.app: off by default)"),
        );
    }
    if let KeyCode::F(n) = chord.code {
        if n >= 13 {
            return (Reach::Risky, Some("few terminals emit F13 and above"));
        }
    }
    (Reach::Fine, None)
}

// ---- the map ----

/// What an action with no chord shows in place of one, everywhere a chord
/// list is rendered — the hotkeys tab and the help labels must agree.
pub const UNBOUND: &str = "—";

/// Resolved bindings: one chord list per entry of [`ACTIONS`], in the same
/// order, so an index is a stable handle for both the UI and the config.
#[derive(Debug, Clone)]
pub struct Keymap {
    binds: Vec<Vec<KeyChord>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            binds: ACTIONS.iter().map(|s| parse_list(s.defaults)).collect(),
        }
    }
}

impl Keymap {
    /// Defaults with the config's `keybindings` overrides applied. An
    /// override naming an unknown action or an unparseable chord is logged
    /// and skipped, so one bad line can't strand the user without a keymap.
    /// An empty string is a deliberate unbind.
    pub fn from_overrides(overrides: &BTreeMap<String, String>) -> Self {
        let mut map = Self::default();
        for (id, spec) in overrides {
            let Some(index) = ACTIONS.iter().position(|s| s.id == id) else {
                tracing::warn!("ignoring keybinding for unknown action {id:?}");
                continue;
            };
            if spec.trim().is_empty() {
                map.binds[index].clear();
                continue;
            }
            let mut chords = Vec::new();
            for part in spec.split(',') {
                match KeyChord::parse(part) {
                    Some(chord) if !chords.contains(&chord) => chords.push(chord),
                    Some(_) => {}
                    None => tracing::warn!("ignoring unparseable keybinding {part:?} for {id}"),
                }
            }
            map.binds[index] = chords;
        }
        map
    }

    /// Only what differs from the defaults, for writing back to the config.
    pub fn overrides(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (index, spec) in ACTIONS.iter().enumerate() {
            if self.binds[index] != parse_list(spec.defaults) {
                out.insert(spec.id.to_string(), self.spec_list(index));
            }
        }
        out
    }

    /// The action `chord` triggers in `scope`, if any.
    pub fn lookup(&self, scope: Scope, chord: &KeyChord) -> Option<Action> {
        ACTIONS
            .iter()
            .enumerate()
            .find(|(i, s)| s.scope == scope && self.binds[*i].contains(chord))
            .map(|(_, s)| s.action)
    }

    pub fn chords(&self, action: Action) -> &[KeyChord] {
        index_of(action).map_or(&[], |i| self.binds[i].as_slice())
    }

    pub fn chords_at(&self, index: usize) -> &[KeyChord] {
        self.binds.get(index).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// On-screen chord list for a row: `j ↓`, or `—` when unbound.
    pub fn display_at(&self, index: usize) -> String {
        let chords = self.chords_at(index);
        if chords.is_empty() {
            return UNBOUND.into();
        }
        chords
            .iter()
            .map(|c| c.display())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The first chord for an action, for help text and footer hints.
    pub fn first(&self, action: Action) -> Option<KeyChord> {
        self.chords(action).first().copied()
    }

    /// Help-style label for an action: every chord it answers to, or `—`.
    pub fn label(&self, action: Action) -> String {
        index_of(action).map_or_else(|| UNBOUND.into(), |i| self.display_at(i))
    }

    /// The chords Help and the footer print for an action: the ⌘ ones in
    /// a terminal that sends ⌘ ([`set_cmd_shown`]), the rest elsewhere.
    /// ⌘ never reaches orion in Terminal.app or `orion browser`, and every
    /// ⌘ default has a `^` twin beside it, so whichever side the host
    /// cannot use stays bound as a silent alias that only Settings →
    /// Hotkeys lists. An action bound on one side alone shows that side.
    pub fn shown_chords(&self, action: Action) -> Vec<KeyChord> {
        shown_side(self.chords(action), cmd_shown())
    }

    /// [`Self::label`] without the ⌘ aliases ([`Self::shown_chords`]).
    pub fn shown_label(&self, action: Action) -> String {
        let chords = self.shown_chords(action);
        if chords.is_empty() {
            return UNBOUND.into();
        }
        chords
            .iter()
            .map(|c| c.display())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The first of [`Self::shown_chords`], for a footer hint.
    pub fn shown_first(&self, action: Action) -> Option<KeyChord> {
        self.shown_chords(action).first().copied()
    }

    fn spec_list(&self, index: usize) -> String {
        self.binds[index]
            .iter()
            .map(|c| c.spec())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Other rows that already answer to `chord` in the same scope as
    /// `index`. Cross-scope reuse is fine — the panels and a locked
    /// terminal never read the same keystroke.
    pub fn conflicts(&self, index: usize, chord: &KeyChord) -> Vec<usize> {
        let Some(spec) = ACTIONS.get(index) else {
            return Vec::new();
        };
        ACTIONS
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != index && s.scope == spec.scope && self.binds[*i].contains(chord))
            .map(|(i, _)| i)
            .collect()
    }

    /// Bind `chord` to `index`, replacing its chords (or appending when
    /// `add`). Any conflicting row loses the chord — one keystroke can
    /// only mean one thing, and silently shadowing the loser would be
    /// worse than taking it away visibly.
    pub fn bind(&mut self, index: usize, chord: KeyChord, add: bool) {
        for other in self.conflicts(index, &chord) {
            self.binds[other].retain(|c| *c != chord);
        }
        let slot = &mut self.binds[index];
        if add {
            if !slot.contains(&chord) {
                slot.push(chord);
            }
        } else {
            slot.clear();
            slot.push(chord);
        }
    }

    pub fn reset(&mut self, index: usize) {
        if let Some(spec) = ACTIONS.get(index) {
            self.binds[index] = parse_list(spec.defaults);
        }
    }

    pub fn clear(&mut self, index: usize) {
        if let Some(slot) = self.binds.get_mut(index) {
            slot.clear();
        }
    }

    /// A row sharing a chord with another action in the same scope. The
    /// overlay warns before it lets you make one, so this only comes from a
    /// hand-edited config — but when it does, [`Keymap::lookup`] silently
    /// gives the key to whichever action is declared first, and a row that
    /// looks bound while doing nothing is exactly the confusing state the
    /// duplicate warning exists to prevent.
    pub fn is_ambiguous(&self, index: usize) -> bool {
        self.chords_at(index)
            .iter()
            .any(|c| !self.conflicts(index, c).is_empty())
    }

    /// Who else claims this row's chords, as a readable list.
    pub fn shadowed_by(&self, index: usize) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self
            .chords_at(index)
            .iter()
            .flat_map(|c| self.conflicts(index, c))
            .filter_map(|i| ACTIONS.get(i).map(|s| s.label))
            .collect();
        out.dedup();
        out
    }

    /// Rows whose chords the host terminal probably eats, with the worst
    /// verdict across the row's chords. A ⌘ chord the host might not send
    /// is no worry on a row that also has a chord without ⌘: that twin is
    /// the one such a host uses.
    pub fn reach_at(&self, index: usize) -> Reach {
        let chords = self.chords_at(index);
        let has_twin = chords
            .iter()
            .any(|c| !c.mods.contains(KeyModifiers::SUPER));
        chords
            .iter()
            .map(|c| match host_warning(c).0 {
                Reach::Risky if has_twin && c.mods.contains(KeyModifiers::SUPER) => Reach::Fine,
                reach => reach,
            })
            .fold(Reach::Fine, |worst, r| match (worst, r) {
                (Reach::Blocked, _) | (_, Reach::Blocked) => Reach::Blocked,
                (Reach::Risky, _) | (_, Reach::Risky) => Reach::Risky,
                _ => Reach::Fine,
            })
    }
}

fn parse_list(specs: &[&str]) -> Vec<KeyChord> {
    specs.iter().filter_map(|s| KeyChord::parse(s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyChord {
        KeyChord::from_event(&KeyEvent::new(code, mods))
    }

    #[test]
    fn every_default_parses_and_round_trips() {
        for spec in ACTIONS {
            for raw in spec.defaults {
                let chord = KeyChord::parse(raw)
                    .unwrap_or_else(|| panic!("{}: {raw:?} does not parse", spec.id));
                let reparsed = KeyChord::parse(&chord.spec())
                    .unwrap_or_else(|| panic!("{}: {raw:?} does not round-trip", spec.id));
                assert_eq!(chord, reparsed, "{}: {raw:?}", spec.id);
            }
        }
    }

    #[test]
    fn action_ids_are_unique() {
        let mut ids: Vec<&str> = ACTIONS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate action id");
    }

    #[test]
    fn defaults_do_not_collide_within_a_scope() {
        let map = Keymap::default();
        for (i, _) in ACTIONS.iter().enumerate() {
            for chord in map.chords_at(i) {
                assert!(
                    map.conflicts(i, chord).is_empty(),
                    "{} default {chord} collides with {:?}",
                    ACTIONS[i].id,
                    map.conflicts(i, chord)
                        .iter()
                        .map(|j| ACTIONS[*j].id)
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn shift_letters_normalize_both_spellings() {
        let upper = ev(KeyCode::Char('J'), KeyModifiers::SHIFT);
        let bare_upper = ev(KeyCode::Char('J'), KeyModifiers::NONE);
        let lower_shift = ev(KeyCode::Char('j'), KeyModifiers::SHIFT);
        assert_eq!(upper, bare_upper);
        assert_eq!(upper, lower_shift);
        assert_eq!(upper.spec(), "shift+j");
        assert_eq!(upper.display(), "⇧J");
        assert_ne!(upper, ev(KeyCode::Char('j'), KeyModifiers::NONE));
    }

    #[test]
    fn punctuation_drops_the_shift_bit() {
        // '?' is shift+/ on a US layout; the glyph already says so.
        assert_eq!(
            ev(KeyCode::Char('?'), KeyModifiers::SHIFT),
            ev(KeyCode::Char('?'), KeyModifiers::NONE)
        );
        assert_eq!(KeyChord::parse("?").unwrap().spec(), "?");
    }

    #[test]
    fn shift_enter_is_its_own_chord_and_flagged_as_kitty_only() {
        let map = Keymap::default();
        let shift_enter = ev(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(map.lookup(Scope::Global, &shift_enter), None);
        assert_eq!(
            map.lookup(Scope::Global, &ev(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::Activate)
        );
        assert_eq!(shift_enter.spec(), "shift+enter");
        assert_eq!(host_warning(&shift_enter).0, Reach::Risky);
    }

    #[test]
    fn backtab_is_shift_tab() {
        assert_eq!(
            ev(KeyCode::BackTab, KeyModifiers::NONE),
            KeyChord::parse("shift+tab").unwrap()
        );
    }

    #[test]
    fn legacy_ctrl_5_is_ctrl_bracket() {
        assert_eq!(
            ev(KeyCode::Char('5'), KeyModifiers::CONTROL),
            KeyChord::parse("ctrl+]").unwrap()
        );
    }

    #[test]
    fn parses_the_literal_plus_key() {
        let plus = KeyChord::parse("+").unwrap();
        assert_eq!(plus.code, KeyCode::Char('+'));
        assert_eq!(KeyChord::parse("ctrl++").unwrap().code, KeyCode::Char('+'));
    }

    #[test]
    fn lookup_respects_scope() {
        let map = Keymap::default();
        let ctrl_q = KeyChord::parse("ctrl+q").unwrap();
        assert_eq!(
            map.lookup(Scope::Terminal, &ctrl_q),
            Some(Action::UnlockTerminal)
        );
        // ^q in the panels is free — the scopes never read the same press.
        assert_eq!(map.lookup(Scope::Global, &ctrl_q), None);
        assert_eq!(
            map.lookup(Scope::Global, &KeyChord::parse("q").unwrap()),
            Some(Action::Quit)
        );
    }

    /// `t` is a shell terminal inside orion; one outside it is a row of
    /// the OPEN MENU `⌘O` drops, with no letter of its own.
    #[test]
    fn t_is_a_terminal_in_orion_and_cmd_o_reaches_one_outside() {
        let map = Keymap::default();
        let at = |spec: &str| map.lookup(Scope::Global, &KeyChord::parse(spec).unwrap());
        assert_eq!(at("t"), Some(Action::NewTerminal));
        assert_eq!(at("shift+t"), None);
        assert_eq!(at("cmd+o"), Some(Action::OpenOutside));
        assert_eq!(at("ctrl+o"), Some(Action::OpenOutside));
        assert!(map.chords(Action::OpenOutsideTerminal).is_empty());
        assert_eq!(map.label(Action::NewTerminal), "t");
    }

    /// Help and the footer print one side of each ⌘ / `^` pair: the ⌘
    /// chords where the terminal sends ⌘, the `^` twins elsewhere. A
    /// binding on one side alone shows on both.
    #[test]
    fn shown_chords_pick_the_side_the_terminal_sends() {
        let map = Keymap::default();
        let finder = map.chords(Action::FindFile);
        let spell = |chords: Vec<KeyChord>| {
            chords.iter().map(KeyChord::display).collect::<Vec<_>>().join(" ")
        };
        assert_eq!(spell(shown_side(finder, false)), "^p");
        assert_eq!(spell(shown_side(finder, true)), "⌘P");
        let help = [KeyChord::parse("cmd+k").unwrap()];
        assert_eq!(spell(shown_side(&help, false)), "⌘K", "⌘ alone still shows");
        for n in 1..=9u8 {
            let action = Action::SelectProjectTab(n);
            assert_eq!(spell(shown_side(map.chords(action), true)), n.to_string());
        }
    }

    #[test]
    fn binding_steals_the_chord_from_its_previous_owner() {
        let mut map = Keymap::default();
        let g = KeyChord::parse("ctrl+e").unwrap();
        let open_repo = index_of(Action::OpenRepo).unwrap();
        assert_eq!(
            map.conflicts(open_repo, &g),
            vec![index_of(Action::GitDiff).unwrap()]
        );
        map.bind(open_repo, g, false);
        assert_eq!(map.lookup(Scope::Global, &g), Some(Action::OpenRepo));
        assert!(!map.chords(Action::GitDiff).contains(&g));
    }

    #[test]
    fn overrides_round_trip_through_the_config_shape() {
        let mut map = Keymap::default();
        let quit = index_of(Action::Quit).unwrap();
        map.bind(quit, KeyChord::parse("ctrl+x").unwrap(), false);
        let saved = map.overrides();
        assert_eq!(saved.get("quit").map(String::as_str), Some("ctrl+x"));
        // Untouched actions stay out of the file.
        assert!(!saved.contains_key("help"));
        let loaded = Keymap::from_overrides(&saved);
        assert_eq!(
            loaded.lookup(Scope::Global, &KeyChord::parse("ctrl+x").unwrap()),
            Some(Action::Quit)
        );
        assert_eq!(
            loaded.lookup(Scope::Global, &KeyChord::parse("q").unwrap()),
            None
        );
    }

    #[test]
    fn an_empty_override_means_unbound() {
        let map = Keymap::from_overrides(&BTreeMap::from([("help".into(), String::new())]));
        assert!(map.chords(Action::Help).is_empty());
        assert_eq!(map.label(Action::Help), "—");
    }

    #[test]
    fn a_broken_override_falls_back_instead_of_stranding_the_user() {
        let map = Keymap::from_overrides(&BTreeMap::from([
            ("quit".into(), "nonsense+zz".into()),
            ("no_such_action".into(), "x".into()),
        ]));
        assert!(map.chords(Action::Quit).is_empty(), "the row is cleared");
        assert_eq!(
            map.lookup(Scope::Global, &KeyChord::parse("s").unwrap()),
            Some(Action::Settings),
            "the rest of the map is untouched"
        );
    }

    #[test]
    fn a_hand_edited_duplicate_is_flagged_on_both_rows() {
        // Nothing in the overlay can produce this; a text editor can.
        let map = Keymap::from_overrides(&BTreeMap::from([("open_repo".into(), "ctrl+e".into())]));
        let open_repo = index_of(Action::OpenRepo).unwrap();
        let diff = index_of(Action::GitDiff).unwrap();
        assert!(map.is_ambiguous(open_repo));
        assert!(map.is_ambiguous(diff));
        assert_eq!(map.shadowed_by(open_repo), vec!["Changes"]);
        // The defaults themselves are always unambiguous.
        assert!(Keymap::default()
            .binds
            .iter()
            .enumerate()
            .all(|(i, _)| !Keymap::default().is_ambiguous(i)));
    }

    #[test]
    fn cmd_chords_are_reported_by_who_keeps_them() {
        let (reach, why) = host_warning(&KeyChord::parse("cmd+]").unwrap());
        assert_eq!(reach, Reach::Blocked, "Ghostty's own");
        assert!(why.unwrap().contains('⌘'));
        for freed in ["cmd+shift+p", "cmd+n", "cmd+,"] {
            let (reach, why) = host_warning(&KeyChord::parse(freed).unwrap());
            assert_eq!(reach, Reach::Risky, "{freed}");
            assert!(why.unwrap().contains("Ghostty keybinds"), "{freed}");
        }
        for passes in ["cmd+k", "cmd+p", "cmd+e", "cmd+j", "cmd+c"] {
            let (reach, _) = host_warning(&KeyChord::parse(passes).unwrap());
            assert_eq!(reach, Reach::Risky, "{passes}");
        }
    }

    /// No default chord is one Ghostty keeps for itself: every ⌘ default
    /// either passes through it or is one orion's block releases.
    #[test]
    fn no_default_is_a_chord_ghostty_keeps() {
        let map = Keymap::default();
        for (i, spec) in ACTIONS.iter().enumerate() {
            for chord in map.chords_at(i) {
                assert_ne!(host_warning(chord).0, Reach::Blocked, "{}: {}", spec.id, chord.spec());
            }
        }
    }

    #[test]
    fn risky_chords_are_flagged_but_allowed() {
        for spec in ["ctrl+shift+f", "ctrl+left", "alt+p", "ctrl+m", "f13"] {
            let chord = KeyChord::parse(spec).unwrap();
            let (reach, why) = host_warning(&chord);
            assert_eq!(reach, Reach::Risky, "{spec}");
            assert!(why.is_some(), "{spec}");
        }
    }

    #[test]
    fn ordinary_chords_are_clean() {
        for spec in ["q", "shift+j", "/", "ctrl+q", "down", "enter", "f5"] {
            let chord = KeyChord::parse(spec).unwrap();
            assert!(host_warning(&chord).0.is_fine(), "{spec} should be fine");
        }
    }

    #[test]
    fn every_bound_action_ships_with_a_reachable_chord() {
        // Some defaults are deliberately iffy (^] is a fallback hatch, the
        // ⌘ chords Ghostty-only), but no bound action may be *only*
        // reachable through a chord the host terminal is likely to eat.
        // An action shipped with no key at all is a COMMAND PALETTE row.
        let map = Keymap::default();
        for (i, spec) in ACTIONS.iter().enumerate() {
            let chords = map.chords_at(i);
            assert!(
                chords.is_empty() || chords.iter().any(|c| host_warning(c).0.is_fine()),
                "{} has no chord a stock terminal delivers",
                spec.id
            );
        }
    }

    // Pins every named key's config spelling, on-screen glyph, and the
    // alternate spellings `parse` accepts, so a table rewrite can't drift
    // a single string the hotkeys tab or a saved config depends on.
    #[test]
    fn named_keys_keep_their_spellings_and_glyphs() {
        let table: &[(KeyCode, &str, &str, &[&str])] = &[
            (KeyCode::Up, "up", "↑", &[]),
            (KeyCode::Down, "down", "↓", &[]),
            (KeyCode::Left, "left", "←", &[]),
            (KeyCode::Right, "right", "→", &[]),
            (KeyCode::Enter, "enter", "Enter", &["return", "cr"]),
            (KeyCode::Tab, "tab", "Tab", &[]),
            (KeyCode::BackTab, "tab", "Tab", &[]),
            (KeyCode::Esc, "esc", "Esc", &["escape"]),
            (KeyCode::Backspace, "backspace", "⌫", &["bs"]),
            (KeyCode::Delete, "delete", "Del", &["del"]),
            (KeyCode::Insert, "insert", "Ins", &["ins"]),
            (KeyCode::Home, "home", "Home", &[]),
            (KeyCode::End, "end", "End", &[]),
            (KeyCode::PageUp, "pgup", "PgUp", &["pageup"]),
            (KeyCode::PageDown, "pgdn", "PgDn", &["pagedown"]),
            (KeyCode::Char(' '), "space", "Space", &[]),
        ];
        for (code, name, glyph, aliases) in table {
            assert_eq!(key_name(*code), *name, "{code:?}");
            assert_eq!(key_display(*code), *glyph, "{code:?}");
            if *code != KeyCode::BackTab {
                assert_eq!(parse_key_name(name), Some(*code), "{name}");
            }
            for alias in *aliases {
                assert_eq!(parse_key_name(alias), Some(*code), "{alias}");
            }
        }
        assert_eq!(parse_key_name("backtab"), Some(KeyCode::BackTab));
        // The open-ended arms: letters, function keys, and crossterm's
        // Debug spelling for anything else.
        assert_eq!(key_name(KeyCode::Char('Q')), "q");
        assert_eq!(key_display(KeyCode::Char('Q')), "Q");
        assert_eq!(parse_key_name("q"), Some(KeyCode::Char('q')));
        assert_eq!(parse_key_name("qq"), None);
        assert_eq!(key_name(KeyCode::F(12)), "f12");
        assert_eq!(key_display(KeyCode::F(12)), "F12");
        assert_eq!(parse_key_name("f12"), Some(KeyCode::F(12)));
        assert_eq!(parse_key_name("f21"), None);
        assert_eq!(key_name(KeyCode::Null), "null");
        assert_eq!(key_display(KeyCode::Null), "Null");
    }
}
