//! Application hotkeys.
//!
//! Every key the grid reacts to is an [`Action`] here, with a default
//! chord list. A hand-edited `keybindings` object in the config JSON
//! (`keybindings: { "<action id>": "j, down" }`) overrides them, the
//! Hotkeys tab of the settings overlay lists them, and the event loop
//! dispatches through [`Keymap::lookup`] instead of matching raw
//! `KeyCode`s.
//!
//! **Normalization.** Terminals disagree about how a key arrives —
//! `Shift+j` may come through as `Char('J')` with or without the shift
//! bit, `Ctrl+]` is spelled `Ctrl+5` in the legacy encoding, `BackTab` is
//! really `Shift+Tab`. [`KeyChord::from_event`] folds all of that into one
//! canonical form so a binding matches whatever the emulator sends.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::fmt;

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
    /// `⇧↑` / `⇧↓` / `⇧←` / `⇧→` on the grid: the cursor steps as the
    /// bare arrow does, and the MULTI-SELECT (`App::marked`) grows to
    /// every card between where the run started and where it is now —
    /// for `⌫` to delete them all behind one confirm.
    ExtendUp,
    ExtendDown,
    ExtendLeft,
    ExtendRight,
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
    /// The pull request of the session card under the cursor, in the
    /// browser — the `#42 title` line on the card. `⌘U` lists the
    /// project's pull requests in orion; this one goes to GitHub.
    OpenPullRequest,
    /// A terminal *outside* orion — a new Ghostty tab in the selected
    /// worktree's directory, where `t` opens one inside orion; a silent
    /// no-op over ssh.
    OpenOutsideTerminal,
    /// `Shift+R`: reload from GitHub now, past every timer — the project's
    /// open pull requests, the worktree's PR, the one the pane is reading.
    RefreshPullRequests,
    /// `y`: reply — the COMMENT BOX on the card's pull request (the one
    /// `Shift+V` opens), or on the one the pane is reading, posted to
    /// GitHub with `gh pr comment` on Enter.
    CommentPullRequest,
    /// `⌘U`: the PULL REQUESTS MODAL — the project's open pull requests,
    /// read in place, commented on, with a PR SESSION launched on one.
    PullRequests,
    /// `⌘L`: the LINEAR VIEW — open Linear issues assigned to you.
    Linear,
    /// The selected worktree's Linear issues — the ones the LINEAR VIEW
    /// shows it working on — copied to the clipboard in full. Unbound:
    /// the COMMAND PALETTE runs it.
    CopyLinearIssues,
    /// `⌘I`: the TODOS MODAL — every project's todo list, a tab each.
    Todos,
    /// `⌘⇧Y`: the WEEK IN REVIEW — what was finished in the last seven
    /// days, written up by product area with what to show and say.
    WeekReview,
    /// `p`: PULL the selected checkout — fetch, then fast-forward onto its
    /// upstream, never a merge or a rebase (`crate::git_sync`).
    PullWorktree,
    /// `⇧P`: PUSH the selected checkout's branch — fast-forward its
    /// upstream, or publish a branch that tracks nothing; never a force.
    PushWorktree,
    /// `Shift+Enter` / `Shift+O` / `Alt+Enter`, on the grid: fire the
    /// selected worktree's OPEN COMMAND — the project's **Open command**
    /// setting (`open http://localhost:3000`, say). Three chords because only the kitty protocol carries a shifted
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
    /// `z` on the grid: fold or unfold the ARCHIVED DRAWER under the
    /// band the cursor is on — that checkout's archived sessions, one
    /// faint line apiece, for `u` to bring back.
    ToggleArchivedDrawer,
    Delete,
    /// `⌘W`: close the agent or terminal the PANE shows — from the card,
    /// the pane, a locked pane or a full-screen session — behind the very
    /// confirm `Backspace` asks on its card. Never a modal, never a
    /// worktree or a project tab: over a modal the chord does nothing.
    ClosePane,
    DeleteAll,
    /// `⌘⌫` (`⇧D` its twin): delete the worktree of the band under the
    /// cursor from disk — every agent and terminal in it with it — behind
    /// one confirm that lists them. Never the main checkout.
    DeleteWorktree,
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
    /// `Shift+C`: **Continue on** another Claude account — the Claude
    /// session under the cursor, conversation and all, carried onto
    /// another Claude-dialect harness and resumed there. Opens the list of
    /// the accounts it can go to.
    ContinueOn,
    // files
    FindFile,
    Grep,
    TreeBrowser,
    /// **Open skills folder**: Claude Code's skills folder in the TREE
    /// BROWSER. The COMMAND PALETTE's alone — no key.
    OpenSkillsFolder,
    // terminal
    UnlockTerminal,
    /// `⌘U` in an agent's LOCKED PANE: the saved AGENT PRESETS as a picker,
    /// the pick pasted at the agent's caret, unsent. A shell's pane has no
    /// prompt to fill, so the chord stays the panels' there.
    InsertPreset,
    // general
    /// `^~`: fold the LAUNCHER VIEW's PANE away and give the cards the
    /// whole body, or bring it back. Folding it also unselects the card
    /// under the cursor, so nothing is selected and nothing is read.
    ToggleLauncherPane,
    /// `⌘F`: FULL-SCREEN the session in the PANE — the grid and its header
    /// give way to the PTY — or bring it back down to the pane beside the
    /// cards. From the cards it full-screens the one under the cursor.
    ToggleFullScreen,
    /// `` ` ``: walk the TERMINAL cards of the cursor's checkout, one after
    /// another and back round to the first. What the pane READS, where
    /// `^~` is whether it is drawn at all.
    PaneTabs,
    /// Open the Nth PROJECT TAB (1-based) in the LAUNCHER VIEW's header.
    SelectProjectTab(u8),
    /// `⌘G`: HOME — the splash with orion's animation, over the grid;
    /// Esc, Enter or any key that opens a project comes back to the grid
    /// where it was.
    Home,
    Hosts,
    Settings,
    /// The settings overlay on the Agents tab's CLAUDE ACCOUNTS: who each
    /// Claude Code login is, signing one in or out, adding another.
    ClaudeAccounts,
    /// `⇧U`: ACCOUNT USAGE — how much is left on each Claude and Cursor
    /// account, window by window.
    Usage,
    /// `⌘⇧S`: start the cursor's checkout's stack — its RUN COMMAND, in
    /// the pane — or stop it.
    ToggleStack,
    StopAllStacks,
    /// Delete every linked worktree, in every project, with no live
    /// session and no uncommitted changes, behind one confirm.
    CleanUnusedWorktrees,
    Help,
    /// `⌘⇧R`, HOME's key: stop the DAEMON and every session in it, then
    /// start orion again from the binary on disk, behind a confirm.
    Restart,
    /// Open ORION SETUP again, every step.
    RunSetup,
    /// `⌘⇧G`: install the newer published release the footer offers, then
    /// restart the daemon and this TUI on it.
    Upgrade,
    /// `⌘/`: pick the model for a new agent — searchable, like Cursor.
    SelectModel,
    /// `⌘⇧/`: cycle the effort / reasoning variant of the current model.
    CycleEffort,
    /// `⌘.`: pick the worktree a new agent will run in.
    SelectLaunchWorktree,
    /// `⌘⇧P`: the COMMAND PALETTE — every action by name, with its key.
    CommandPalette,
    /// `⌘O`: the OPEN MENU — what is under the cursor, outside orion: the
    /// repo, pull request or issue on GitHub, the open command, the OPEN
    /// IN APP editor, a Ghostty tab in the checkout.
    OpenOutside,
    /// The checkout under the cursor in the OPEN IN APP editor (the OPEN
    /// MENU's row). Its id keeps the name it shipped with, `open_in_cursor`,
    /// so a key bound to it stays bound.
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

/// One positional PROJECT TAB shortcut: `⌘N`, as in a browser. A ⌘ chord
/// reaches orion from inside a locked agent pane too, so a tab is one
/// press away wherever the keys are; orion's Ghostty block unbinds
/// Ghostty's own ⌘1–⌘9.
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
        defaults: &["cmd+k"],
    },
    ActionSpec {
        action: Action::NextAttention,
        id: "next_attention",
        label: "Next session needing you",
        hint: "Jump to the next session in the palette's attention order (needs you or crashed, finished unread, running, then recency) in any project, wrapping",
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
    project_tab_slot!(1, "project_tab_1", "Project tab 1", "cmd+1"),
    project_tab_slot!(2, "project_tab_2", "Project tab 2", "cmd+2"),
    project_tab_slot!(3, "project_tab_3", "Project tab 3", "cmd+3"),
    project_tab_slot!(4, "project_tab_4", "Project tab 4", "cmd+4"),
    project_tab_slot!(5, "project_tab_5", "Project tab 5", "cmd+5"),
    project_tab_slot!(6, "project_tab_6", "Project tab 6", "cmd+6"),
    project_tab_slot!(7, "project_tab_7", "Project tab 7", "cmd+7"),
    project_tab_slot!(8, "project_tab_8", "Project tab 8", "cmd+8"),
    project_tab_slot!(9, "project_tab_9", "Project tab 9", "cmd+9"),
    ActionSpec {
        action: Action::CloseProjectTab,
        id: "close_project_tab",
        label: "Close project tab",
        hint: "Drop this project's tab from the header and open the tab beside it. Nothing is deleted — {palette} opens it again",
        group: "NAVIGATE",
        scope: Scope::Global,
        // No key: a tab's `×`, its right-click menu and the COMMAND
        // PALETTE close it. A bare `x` closed one by accident.
        defaults: &[],
    },
    ActionSpec {
        action: Action::ProjectDropdown,
        id: "project_dropdown",
        label: "Switch project",
        hint: "Drop the list of every project under the + in the header — type to narrow it, Enter or a click opens one. {palette} reaches every project too",
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
        hint: "Open a folder as a project in orion, from anywhere — the last row of {palette}",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::New,
        id: "new",
        label: crate::agent_picker::NEW_AGENT_PICKER_TITLE,
        hint: "Pick the agent's harness first (→ drills into its model and effort), then the new-agent box opens set to it, in the checkout under the cursor; on the first-run splash, add a project",
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
        defaults: &["cmd+e"],
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
        hint: "Send the pull request of the session card under the cursor — its checkout's branch — to your browser, as a click on the card's #42 line does. {pull_requests} lists the pull requests in orion",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::RefreshPullRequests,
        id: "refresh_pull_requests",
        label: "Reload from GitHub",
        hint: "Ask GitHub again now for the project's open pull requests, the selected worktree's PR and the one the pane is reading",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["cmd+r"],
    },
    ActionSpec {
        action: Action::CommentPullRequest,
        id: "comment_pull_request",
        label: "Comment on pull request",
        hint: "Open a box to type a comment and post it through gh on the card's pull request, or, with the pane reading a pull request (a {palette} jump lands on one), on that one",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::PullRequests,
        id: "pull_requests",
        label: "GitHub pull requests",
        hint: "List the project's open pull requests; Enter prompts a PR session on one, ⇧Tab launches a preset on it, Tab picks a harness",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        // ⌘U: every window orion opens is a ⌘ chord, and ⌘P is Go to
        // file, as in Cursor.
        defaults: &["cmd+u"],
    },
    ActionSpec {
        action: Action::Linear,
        id: "linear",
        label: "Linear issues",
        hint: "List Linear issues assigned to you; type to search, ↓ to the list where Space marks, Enter starts one agent on the marked set in one worktree",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["cmd+l"],
    },
    ActionSpec {
        action: Action::CopyLinearIssues,
        id: "copy_linear_issues",
        label: "Copy linked Linear issues",
        hint: "Copy the selected worktree's Linear issues — linked to it, named in its branch, or on its pull request — each with its title, status, link and description, as its agent was given them",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        // No key: ⌘C is the terminal's on the grid, and this is run
        // seldom enough for the COMMAND PALETTE.
        defaults: &[],
    },
    ActionSpec {
        action: Action::Todos,
        id: "todos",
        label: "Todos",
        hint: "Your todo lists, a tab per project: tick off, prioritise, send an agent or a Linear issue from any item",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        // ⌘I is the one ⌘ letter neither macOS nor Ghostty keeps.
        defaults: &["cmd+i"],
    },
    ActionSpec {
        action: Action::WeekReview,
        id: "week_review",
        label: "My week in review",
        hint: "What was merged and done in the last 7 days, written up by product area with what to show and what to say; just yours or everyone's",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        // ⌘⇧W would read better, and Ghostty keeps it for good. Every `^`
        // letter is spoken for, so its twin for a terminal that sends no
        // ⌘ is the grid's `⇧W`, for week.
        defaults: &["cmd+shift+y", "shift+w"],
    },
    ActionSpec {
        action: Action::PullWorktree,
        id: "pull_worktree",
        label: "Pull from remote",
        hint: "Fetch and fast-forward the selected checkout onto the branch it tracks; never merges, rebases or touches uncommitted work",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["p"],
    },
    ActionSpec {
        action: Action::PushWorktree,
        id: "push_worktree",
        label: "Push to remote",
        hint: "Push the selected checkout's new commits to the branch it tracks, or publish a branch that tracks nothing to origin; never forces, and asks before pushing straight to the base branch",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["shift+p"],
    },
    ActionSpec {
        action: Action::OpenWorktree,
        id: "open_worktree",
        label: "Open checkout in editor",
        hint: "Open the selected checkout outside orion, usually in your editor: its project's Open command (Settings → Projects) — e.g. open http://localhost:3000",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &[],
    },
    // ---- SESSIONS ----
    ActionSpec {
        action: Action::NewTerminal,
        id: "new_terminal",
        label: "New shell terminal",
        hint: "A terminal: a plain shell inside orion — a session like an agent, but no AI CLI — in the selected worktree's directory; {open_outside} opens one outside it",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["t"],
    },
    // `t`'s shift pair: the same terminal, outside orion.
    ActionSpec {
        action: Action::OpenOutsideTerminal,
        id: "open_outside_terminal",
        label: "Terminal outside orion",
        hint: "Open the selected worktree's directory in a Ghostty tab",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::Rename,
        id: "rename",
        label: "Rename",
        hint: "Rename the session under the cursor",
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
        // Never a bare letter: a stray `a` aimed at a prompt that lands on
        // the grid must not file a session away.
        defaults: &["cmd+shift+a"],
    },
    ActionSpec {
        action: Action::Unarchive,
        id: "unarchive",
        label: "Unarchive session",
        hint: "Bring the archived agent under the cursor back into the list",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["cmd+shift+u"],
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
        action: Action::ToggleArchivedDrawer,
        id: "toggle_archived_drawer",
        label: "Show / hide worktree's archived",
        hint: "Fold or unfold the archived sessions listed under the worktree the cursor is on",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["z"],
    },
    ActionSpec {
        action: Action::Delete,
        id: "delete",
        label: "Delete selected",
        hint: "Remove the selected row — or every card picked out with Shift+arrows or Cmd-click — behind a confirmation. With the PROJECT TABS holding the keys, close the tab under their cursor, behind the same kind of confirmation",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["backspace"],
    },
    ActionSpec {
        action: Action::ExtendUp,
        id: "extend_up",
        label: "Select up",
        hint: "Pick out the cards from where the run started to the one above, for Backspace to delete together. Cmd-click (Option-click) adds or drops one card; Esc lets them all go",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+up"],
    },
    ActionSpec {
        action: Action::ExtendDown,
        id: "extend_down",
        label: "Select down",
        hint: "Pick out the cards from where the run started to the one below, for Backspace to delete together",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+down"],
    },
    ActionSpec {
        action: Action::ExtendLeft,
        id: "extend_left",
        label: "Select left",
        hint: "Pick out the cards from where the run started to the one on the left, for Backspace to delete together",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+left"],
    },
    ActionSpec {
        action: Action::ExtendRight,
        id: "extend_right",
        label: "Select right",
        hint: "Pick out the cards from where the run started to the one on the right, for Backspace to delete together",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+right"],
    },
    ActionSpec {
        action: Action::DeleteWorktree,
        id: "delete_worktree",
        label: "Delete worktree",
        hint: "Delete the worktree under the cursor from disk, with every agent and terminal in it, behind one confirmation that lists them. The main checkout is never deleted",
        group: "SESSIONS",
        scope: Scope::Global,
        // ⌘⌫ beside ⌫'s delete-one; `⇧D` is its twin for a terminal that
        // never sends ⌘. In a LOCKED PANE ⌘⌫ stays the agent's kill-line.
        defaults: &["cmd+backspace", "shift+d"],
    },
    ActionSpec {
        action: Action::ClosePane,
        id: "close_pane",
        label: "Close agent or terminal",
        hint: "Close the agent or terminal the pane shows — from its card, the pane, or typing inside it — behind the same confirmation Backspace asks. Does nothing over a modal",
        group: "SESSIONS",
        scope: Scope::Global,
        // ⌘W, as every Mac app closes what is in front, and ⌘ reaches
        // orion from inside a locked pane.
        defaults: &["cmd+w"],
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
        hint: "An agent is an AI coding CLI on a task ({new_terminal} starts a terminal, a plain shell; both are sessions). Type the task; Enter starts it (Tab picks the harness; {select_model} the model; {cycle_effort} the effort; {select_launch_worktree} the worktree)",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["cmd+n"],
    },
    ActionSpec {
        action: Action::SelectModel,
        id: "select_model",
        label: "Select model",
        hint: "Searchable model list for the new-agent box (opens the box first if it is not up); type to narrow, Enter picks",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["cmd+/"],
    },
    ActionSpec {
        action: Action::CycleEffort,
        id: "cycle_effort",
        label: "Cycle effort",
        hint: "Step the new-agent box's effort / reasoning variant (default, low, high, …), shown in its header; with no box up, the Agents default",
        group: "SESSIONS",
        scope: Scope::Global,
        // `⌘Y` first: macOS keeps ⌘⇧/ for every app's Help menu, so in
        // Ghostty that press opens Help and never reaches orion. ⌘⇧/ stays
        // bound behind it — `cmd+?`, the one spelling every way a terminal sends it
        // folds to ([`KeyChord::from_event`]) — for a terminal or a Mac
        // that lets it through.
        defaults: &["cmd+y", "cmd+?"],
    },
    ActionSpec {
        action: Action::SelectLaunchWorktree,
        id: "select_launch_worktree",
        label: "Select worktree",
        hint: "Pick which worktree a new agent runs in (opens the box first if it is not up); type to narrow, or choose + new worktree. Cursor's workspace picker",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["cmd+."],
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
    ActionSpec {
        action: Action::ContinueOn,
        id: "continue_on",
        label: "Continue on another account",
        hint: "Carry the Claude session under the cursor onto another Claude account (Settings → Agents → Claude accounts), conversation and all, and resume it there; for a session stopped at a usage limit",
        group: "SESSIONS",
        scope: Scope::Global,
        defaults: &["shift+c"],
    },
    // ---- FILES ----
    ActionSpec {
        action: Action::FindFile,
        id: "find_file",
        label: "Go to file",
        hint: "Fuzzy file finder for the selected worktree",
        group: "FILES",
        scope: Scope::Global,
        defaults: &["cmd+p"],
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
        defaults: &["cmd+shift+f", "shift+f"],
    },
    ActionSpec {
        action: Action::TreeBrowser,
        id: "tree_browser",
        label: "File tree browser",
        hint: "Browse the worktree's files with a preview pane",
        group: "FILES",
        scope: Scope::Global,
        defaults: &["cmd+b"],
    },
    ActionSpec {
        action: Action::OpenSkillsFolder,
        id: "skills_folder",
        label: "Open skills folder",
        hint: "Your Claude Code skills (~/.claude/skills) in the file tree browser",
        group: "FILES",
        scope: Scope::Global,
        defaults: &[],
    },
    // ---- TERMINAL ----
    ActionSpec {
        action: Action::UnlockTerminal,
        id: "unlock_terminal",
        label: "Unlock terminal input",
        hint: "Leave the locked pane and go back to the card (Esc leaves; ⇧Esc sends Esc to the agent; ^Q always works)",
        group: "TERMINAL",
        scope: Scope::Terminal,
        defaults: &["esc", "ctrl+q", "ctrl+]", "ctrl+shift+h"],
    },
    ActionSpec {
        action: Action::InsertPreset,
        id: "insert_preset",
        label: "Insert preset",
        hint: "In an agent's locked pane: pick a saved preset and its text goes in at the caret, unsent",
        group: "TERMINAL",
        scope: Scope::Terminal,
        defaults: &["cmd+u"],
    },
    // ---- GENERAL ----
    ActionSpec {
        action: Action::ToggleLauncherPane,
        id: "toggle_launcher_pane",
        label: "Toggle pane",
        hint: "Fold the pane under the cards away — which also unselects the card it was reading — or bring it back. {toggle_launcher_pane} works from inside the pane too",
        group: "GENERAL",
        scope: Scope::Global,
        // ⌘J, as in Cursor, reaches orion from inside the pane as well —
        // no one types a ⌘ chord as text.
        defaults: &["cmd+j"],
    },
    ActionSpec {
        action: Action::ToggleFullScreen,
        id: "toggle_full_screen",
        label: "Full-screen session",
        hint: "Give the session in the pane the whole screen, or bring it back down beside the cards — from inside the pane too, where it is never forwarded to the agent. From the cards it full-screens the one under the cursor. Esc and ^Q also bring a full-screen session back down",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["cmd+f"],
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
        action: Action::Home,
        id: "home",
        label: "Home",
        hint: "The home screen: orion's animation and the ways into a project. Esc or Enter goes back to the grid where you were; a click on the footer's orion version does the same as this key",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["cmd+g"],
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
        action: Action::ClaudeAccounts,
        id: "claude_accounts",
        label: "Claude accounts",
        hint: "Settings → Agents → Claude accounts: who each Claude Code login is signed in as, sign one in or out, add another",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::Usage,
        id: "usage",
        label: "Account usage",
        hint: "How much is left on each Claude and Cursor account — session, day, week and month — read every 15 minutes",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["shift+u"],
    },
    ActionSpec {
        action: Action::ToggleStack,
        id: "toggle_stack",
        label: "Start/stop stack",
        hint: "Start the selected checkout's stack — its run command (Settings → Projects) — with the pane on it as it boots; on a running one, stop it (a ^C, so it winds down in view; again kills it)",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["cmd+shift+s"],
    },
    ActionSpec {
        action: Action::StopAllStacks,
        id: "stop_all_stacks",
        label: "Stop all stacks",
        hint: "Stop every checkout's running stack — a ^C to each run, so each winds down in view",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::CleanUnusedWorktrees,
        id: "clean_unused_worktrees",
        label: "Clean unused worktrees",
        hint: "List every worktree, in every project, with no agent or terminal and no uncommitted changes, grouped by project and all ticked; Enter deletes the ticked ones. b deletes their merged and pushed branches too; a branch with unpushed commits is always kept, and the main checkout is never deleted",
        group: "PROJECTS & WORKTREES",
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
        action: Action::RunSetup,
        id: "run_setup",
        label: "Run setup",
        hint: "Open Orion setup again, every step: agents and their CLIs, Claude accounts, the Open in app editor, worktrees, Linear and autofix, with i to install whatever is missing",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &[],
    },
    ActionSpec {
        action: Action::Restart,
        id: "restart",
        label: "Restart orion",
        hint: "Stop the daemon and every agent and terminal in it, then start orion again from scratch, behind a confirmation. Agents resume, and any cut off mid-turn carry on; terminals start a new shell",
        group: "GENERAL",
        scope: Scope::Global,
        // ⌘⇧R, HOME's key, behind the confirm: nothing here is one stray
        // letter.
        defaults: &["cmd+shift+r"],
    },
    ActionSpec {
        action: Action::Upgrade,
        id: "upgrade",
        label: "Upgrade orion",
        hint: "Install the newer release the footer offers, then restart orion and its daemon on it, behind a confirmation. Agents resume, and any cut off mid-turn carry on; terminals start a new shell",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["cmd+shift+g"],
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
        defaults: &["cmd+shift+p", ":"],
    },
    ActionSpec {
        action: Action::OpenOutside,
        id: "open_outside",
        label: "Open outside orion",
        hint: "A menu of what the cursor is on, outside orion: the repo, pull request or issue on GitHub, the checkout's open command, your editor app, a terminal",
        group: "GENERAL",
        scope: Scope::Global,
        defaults: &["cmd+o"],
    },
    ActionSpec {
        action: Action::OpenInCursor,
        id: "open_in_cursor",
        label: "Open checkout in app",
        hint: "Open the checkout under the cursor in the Open in app editor — Cursor, VS Code, … ({open_outside} on a file opens the file)",
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
        // Not a bare `q`: a stray letter must not ask to quit.
        defaults: &["ctrl+c"],
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

/// `key` with an Escape carrying ⌘ turned back into the ⌘. it was
/// pressed as. macOS binds ⌘. to Cancel — `cancelOperation:`, the very
/// command Escape sends — in every app, so a terminal can hand orion the
/// press as an Escape, ⌘ still held, and every Esc arm that ignores the
/// modifiers read it as a bare Esc: **Select worktree** (`⌘.`) closed the
/// new-agent box it was pressed in. No one presses ⌘Esc meaning Esc, so
/// the loop folds it here before any handler sees it, and
/// [`KeyChord::from_event`] does the same for a chord built elsewhere.
/// Ghostty is asked to send ⌘. as the KITTY PROTOCOL spells it in the
/// first place ([`crate::ghostty_config::SENT_AS_KITTY`]); this is the net
/// under a terminal or a config that does not.
pub fn untangle_cmd_period(key: KeyEvent) -> KeyEvent {
    if key.code == KeyCode::Esc && key.modifiers.contains(KeyModifiers::SUPER) {
        KeyEvent {
            code: KeyCode::Char('.'),
            ..key
        }
    } else {
        key
    }
}

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
    /// * a shifted symbol is its glyph, without the shift: `?` is `?`,
    ///   never `shift+?` — and `shift + /`, which the kitty protocol
    ///   sends for ⌘⇧/ where a legacy terminal sends `?`, is `?` too
    ///   (the US layout's pairs, [`SHIFTED`]), so `cmd+?`, `⌘⇧/` and
    ///   `⌘?` all name the one chord whichever spelling arrives;
    /// * `BackTab` becomes `shift+tab`;
    /// * `ctrl+5` — the legacy encoding's name for byte 0x1D — becomes
    ///   `ctrl+]`, which is what the user actually pressed, and `ctrl+7`
    ///   (byte 0x1F) becomes `ctrl+/`;
    /// * an Escape carrying ⌘ is `cmd+.` — macOS's Cancel, the key it
    ///   turns ⌘. into ([`untangle_cmd_period`]).
    pub fn from_event(key: &KeyEvent) -> Self {
        let key = untangle_cmd_period(*key);
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
                    // ⌘⇧/ arrives as `/` with shift (the kitty protocol's
                    // base key) or as `?` with or without it (a legacy
                    // terminal, or kitty's alternate keys): one chord,
                    // `cmd+?`, distinct from ⌘/ either way.
                    if mods.contains(KeyModifiers::SHIFT) {
                        if let Some(&(_, shifted)) = SHIFTED.iter().find(|(base, _)| *base == c) {
                            code = KeyCode::Char(shifted);
                        }
                    }
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
        // `^/` sends byte 0x1F in the legacy encoding, which crossterm
        // names `ctrl+7`; the kitty protocol sends the `/` itself.
        if mods.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('7') {
            code = KeyCode::Char('/');
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

    /// The on-screen spelling — THE key label: every hint, footer, help
    /// row and settings row prints a chord through here, so one key reads
    /// the same everywhere. `^Q`, `⇧Tab`, `⌥P`, `⌘K`, `⌘?`, `↓`: a
    /// letter under a modifier is its capital, as macOS's menus spell it,
    /// and a bare letter stays the letter you type.
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
            // macOS spells chords with the capital: ⌘K and ^K, not ⌘k.
            KeyCode::Char(c)
                if self.mods.intersects(
                    KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT,
                ) && c.is_alphabetic() =>
            {
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

/// The US layout's shifted symbols, `(base, shifted)`: what
/// [`KeyChord::from_event`] folds a shifted base key onto, so `shift + /`
/// and `?` are one chord.
const SHIFTED: &[(char, char)] = &[
    ('/', '?'),
    ('1', '!'),
    ('2', '@'),
    ('3', '#'),
    ('4', '$'),
    ('5', '%'),
    ('6', '^'),
    ('7', '&'),
    ('8', '*'),
    ('9', '('),
    ('0', ')'),
    ('-', '_'),
    ('=', '+'),
    ('[', '{'),
    (']', '}'),
    ('\\', '|'),
    (';', ':'),
    ('\'', '"'),
    (',', '<'),
    ('.', '>'),
    ('`', '~'),
];

/// The base key a shifted symbol is typed with on a US layout — `/` for
/// `?` — for another program's spelling of the chord (Ghostty's
/// `super+shift+/`).
pub fn unshifted(c: char) -> Option<char> {
    SHIFTED
        .iter()
        .find(|(_, shifted)| *shifted == c)
        .map(|(base, _)| *base)
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

/// The chords of `all` the hints print: the ⌘ ones, or all of them when
/// there are none — a modal's own key table spelled the way the keymap's
/// actions are.
pub fn shown(all: &[KeyChord]) -> Vec<KeyChord> {
    let cmd: Vec<KeyChord> = all
        .iter()
        .filter(|c| c.mods.contains(KeyModifiers::SUPER))
        .copied()
        .collect();
    if cmd.is_empty() {
        all.to_vec()
    } else {
        cmd
    }
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

    /// The chords Help and the footer print for an action: its ⌘ ones,
    /// or every chord when it has none ([`shown`]). A bare-letter
    /// fallback beside a ⌘ chord (`s` for `⌘,`) stays bound, and only
    /// Settings → Hotkeys lists it.
    pub fn shown_chords(&self, action: Action) -> Vec<KeyChord> {
        shown(self.chords(action))
    }

    /// [`Self::label`] without the fallbacks ([`Self::shown_chords`]).
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
}

fn parse_list(specs: &[&str]) -> Vec<KeyChord> {
    let mut chords = Vec::new();
    for chord in specs.iter().filter_map(|s| KeyChord::parse(s)) {
        if !chords.contains(&chord) {
            chords.push(chord);
        }
    }
    chords
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyChord {
        KeyChord::from_event(&KeyEvent::new(code, mods))
    }

    /// `⌘⌫` deletes the worktree, and so does `⇧D`; plain `⌫` stays the
    /// delete of one row.
    #[test]
    fn cmd_backspace_and_shift_d_delete_the_worktree() {
        let map = Keymap::default();
        let at = |code, mods| map.lookup(Scope::Global, &ev(code, mods));
        assert_eq!(
            at(KeyCode::Backspace, KeyModifiers::SUPER),
            Some(Action::DeleteWorktree)
        );
        assert_eq!(
            at(KeyCode::Char('D'), KeyModifiers::SHIFT),
            Some(Action::DeleteWorktree)
        );
        assert_eq!(
            at(KeyCode::Backspace, KeyModifiers::NONE),
            Some(Action::Delete)
        );
    }

    #[test]
    fn cmd_slash_and_shift_cmd_slash_are_distinct() {
        let slash = KeyChord::parse("cmd+/").unwrap();
        let shift = KeyChord::parse("shift+cmd+/").unwrap();
        assert_ne!(slash, shift);
        assert_eq!(slash.spec(), "cmd+/");
        assert_eq!(shift.spec(), "cmd+?", "⌘⇧/ is ⌘? on a US layout");
        assert_eq!(KeyChord::parse("cmd+?").unwrap(), shift);
        assert_eq!(slash.display(), "⌘/");
        assert_eq!(shift.display(), "⌘?");
        let map = Keymap::default();
        let at = |spec: &str| map.lookup(Scope::Global, &KeyChord::parse(spec).unwrap());
        assert_eq!(at("cmd+/"), Some(Action::SelectModel));
        assert_eq!(at("ctrl+/"), None, "no ^ twin");
        assert_eq!(at("shift+cmd+/"), Some(Action::CycleEffort));
        assert_eq!(at("cmd+?"), Some(Action::CycleEffort));
        assert_eq!(at("cmd+y"), Some(Action::CycleEffort));
        assert_eq!(at("cmd+."), Some(Action::SelectLaunchWorktree));
        // Terminals deliver ⌘⇧/ as SUPER+SHIFT+/ (keep SHIFT) or as ⌘?.
        let from_slash = ev(
            KeyCode::Char('/'),
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
        );
        assert_eq!(from_slash, shift);
        assert_eq!(
            map.lookup(Scope::Global, &from_slash),
            Some(Action::CycleEffort)
        );
        let from_qmark = ev(KeyCode::Char('?'), KeyModifiers::SUPER);
        assert_eq!(
            map.lookup(Scope::Global, &from_qmark),
            Some(Action::CycleEffort)
        );
    }

    /// macOS keeps ⌘⇧/ for every app's Help menu, so in Ghostty it opens
    /// Help and never reaches orion: Cycle effort leads with a chord
    /// neither macOS nor Ghostty takes — `⌘Y` — and keeps ⌘⇧/ bound
    /// behind it.
    #[test]
    fn cycle_effort_leads_with_a_chord_the_mac_lets_through() {
        let map = Keymap::default();
        let chords = map.chords(Action::CycleEffort);
        let cmd = shown(chords);
        assert_eq!(cmd.first().map(KeyChord::display).as_deref(), Some("⌘Y"));
        assert!(crate::ghostty_config::releases(&cmd[0]));
        assert!(chords.contains(&KeyChord::parse("shift+cmd+/").unwrap()));
    }

    /// ⌘⇧/ reaches orion spelled three ways — Ghostty's kitty protocol
    /// sends the base key `/` with SHIFT, its alternate keys (and a
    /// legacy terminal) the glyph `?` with or without SHIFT — and every
    /// one of them is the CycleEffort chord, whichever spelling the
    /// keymap holds: the default, or a capture of any one of them.
    #[test]
    fn every_spelling_of_shift_cmd_slash_is_one_chord() {
        let spellings = [
            ev(
                KeyCode::Char('?'),
                KeyModifiers::SHIFT | KeyModifiers::SUPER,
            ),
            ev(
                KeyCode::Char('/'),
                KeyModifiers::SHIFT | KeyModifiers::SUPER,
            ),
            ev(KeyCode::Char('?'), KeyModifiers::SUPER),
        ];
        let map = Keymap::default();
        for chord in spellings {
            assert_eq!(chord.spec(), "cmd+?");
            assert_eq!(chord.display(), "⌘?");
            assert_eq!(map.lookup(Scope::Global, &chord), Some(Action::CycleEffort));
            // A rebind written in any one spelling answers the others.
            let rebound =
                Keymap::from_overrides(&BTreeMap::from([("cycle_effort".into(), chord.spec())]));
            for other in spellings {
                assert_eq!(
                    rebound.lookup(Scope::Global, &other),
                    Some(Action::CycleEffort)
                );
            }
        }
        // ⌘/ stays Select model however the shift-less key arrives.
        assert_eq!(
            map.lookup(Scope::Global, &ev(KeyCode::Char('/'), KeyModifiers::SUPER)),
            Some(Action::SelectModel)
        );
        // The same fold for the rest of the row: ⌘⇧1 is ⌘!, never ⌘1.
        assert_eq!(
            ev(
                KeyCode::Char('1'),
                KeyModifiers::SHIFT | KeyModifiers::SUPER
            ),
            KeyChord::parse("cmd+!").unwrap()
        );
        assert_ne!(
            ev(
                KeyCode::Char('1'),
                KeyModifiers::SHIFT | KeyModifiers::SUPER
            ),
            KeyChord::parse("cmd+1").unwrap()
        );
        assert_eq!(unshifted('?'), Some('/'));
        assert_eq!(unshifted('a'), None);
    }

    /// One label for a chord everywhere: a letter under any modifier is
    /// its capital, a bare letter the letter typed.
    #[test]
    fn chords_display_in_one_spelling() {
        let show = |spec: &str| KeyChord::parse(spec).unwrap().display();
        assert_eq!(show("ctrl+q"), "^Q");
        assert_eq!(show("cmd+k"), "⌘K");
        assert_eq!(show("ctrl+shift+p"), "^⇧P");
        assert_eq!(show("alt+p"), "⌥P");
        assert_eq!(show("shift+a"), "⇧A");
        assert_eq!(show("t"), "t");
        assert_eq!(show("backspace"), "⌫");
        assert_eq!(show("ctrl+/"), "^/");
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

    /// An Escape carrying ⌘ — macOS's Cancel, which is what ⌘. becomes
    /// on a Mac — is ⌘. everywhere: the chord the keymap binds to
    /// **Select worktree**, and an event no Esc arm can take for a bare
    /// Esc. A plain or shifted Escape stays Escape.
    #[test]
    fn an_escape_holding_cmd_is_cmd_period() {
        let cancel = KeyEvent::new(KeyCode::Esc, KeyModifiers::SUPER);
        assert_eq!(
            KeyChord::from_event(&cancel),
            KeyChord::parse("cmd+.").unwrap()
        );
        assert_eq!(
            Keymap::default().lookup(Scope::Global, &KeyChord::from_event(&cancel)),
            Some(Action::SelectLaunchWorktree)
        );
        assert_eq!(untangle_cmd_period(cancel).code, KeyCode::Char('.'));
        assert_eq!(untangle_cmd_period(cancel).modifiers, KeyModifiers::SUPER);
        for kept in [KeyModifiers::NONE, KeyModifiers::SHIFT, KeyModifiers::ALT] {
            let esc = KeyEvent::new(KeyCode::Esc, kept);
            assert_eq!(untangle_cmd_period(esc), esc);
            assert_eq!(KeyChord::from_event(&esc).code, KeyCode::Esc);
        }
    }

    #[test]
    fn defaults_do_not_collide_within_a_scope() {
        let map = Keymap::default();
        let mut owners: Vec<(Scope, KeyChord, &str)> = Vec::new();
        for (i, spec) in ACTIONS.iter().enumerate() {
            for chord in map.chords_at(i) {
                if let Some((_, _, owner)) = owners
                    .iter()
                    .find(|(scope, bound, _)| *scope == spec.scope && bound == chord)
                {
                    panic!("{} default {chord} collides with {owner}", spec.id);
                }
                owners.push((spec.scope, *chord, spec.id));
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
    fn shift_enter_is_its_own_chord() {
        let map = Keymap::default();
        let shift_enter = ev(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(map.lookup(Scope::Global, &shift_enter), None);
        assert_eq!(
            map.lookup(Scope::Global, &ev(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::Activate)
        );
        assert_eq!(shift_enter.spec(), "shift+enter");
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

    /// `^/` in a legacy terminal is byte 0x1F, which crossterm names
    /// `ctrl+7`: it is the chord `ctrl+/`, as a hand-edited binding
    /// spells it.
    #[test]
    fn legacy_ctrl_7_is_ctrl_slash() {
        let legacy = ev(KeyCode::Char('7'), KeyModifiers::CONTROL);
        assert_eq!(legacy, KeyChord::parse("ctrl+/").unwrap());
        assert_eq!(legacy.display(), "^/");
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
        // ^q in the panels is nobody's: the scopes never read the same
        // press.
        assert_eq!(map.lookup(Scope::Global, &ctrl_q), None);
        assert_eq!(
            map.lookup(Scope::Global, &KeyChord::parse("ctrl+c").unwrap()),
            Some(Action::Quit)
        );
    }

    /// `p` PULLs the selected checkout: a bare letter, since a
    /// fast-forward loses nothing.
    /// `⇧P` PUSHes it: a modified chord, since a push is seen by others.
    #[test]
    fn p_pulls_and_shift_p_pushes() {
        let map = Keymap::default();
        let at = |spec: &str| map.lookup(Scope::Global, &KeyChord::parse(spec).unwrap());
        assert_eq!(at("p"), Some(Action::PullWorktree));
        assert_eq!(at("shift+p"), Some(Action::PushWorktree));
    }

    /// A stray letter must not quit, close a tab, or archive or unarchive a
    /// session: those ship on modified chords, or none.
    #[test]
    fn stray_letters_quit_close_and_archive_nothing() {
        let map = Keymap::default();
        for bare in ["q", "x", "a", "u"] {
            assert_eq!(
                map.lookup(Scope::Global, &KeyChord::parse(bare).unwrap()),
                None,
                "{bare} is bound"
            );
        }
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
        assert_eq!(at("ctrl+o"), None);
        assert!(map.chords(Action::OpenOutsideTerminal).is_empty());
        assert_eq!(map.label(Action::NewTerminal), "t");
    }

    /// Help and the footer print an action's ⌘ chords, and every chord of
    /// one that has none.
    #[test]
    fn shown_chords_are_the_cmd_ones() {
        let map = Keymap::default();
        let spell = |chords: Vec<KeyChord>| {
            chords
                .iter()
                .map(KeyChord::display)
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(spell(map.shown_chords(Action::FindFile)), "⌘P");
        assert_eq!(spell(map.shown_chords(Action::Settings)), "⌘,", "not `s`");
        assert_eq!(spell(map.shown_chords(Action::NewTerminal)), "t");
        for n in 1..=9u8 {
            let action = Action::SelectProjectTab(n);
            assert_eq!(spell(map.shown_chords(action)), format!("⌘{n}"));
        }
    }

    /// The `^` twin of every ⌘ chord is gone: the ⌘ chord is the one key,
    /// and only `:` (palette) and `s` (settings) stay beside theirs.
    #[test]
    fn no_cmd_chord_has_a_ctrl_twin() {
        for spec in ACTIONS {
            let chords = parse_list(spec.defaults);
            if chords.iter().any(|c| c.mods.contains(KeyModifiers::SUPER)) {
                assert!(
                    chords
                        .iter()
                        .all(|c| !c.mods.contains(KeyModifiers::CONTROL)
                            || c.mods.contains(KeyModifiers::SUPER)),
                    "{} keeps a ^ twin: {:?}",
                    spec.id,
                    spec.defaults
                );
            }
        }
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

    /// No default chord is one Orion.app keeps for itself: every ⌘ default
    /// reaches orion there.
    #[test]
    fn no_default_is_a_chord_ghostty_keeps() {
        let map = Keymap::default();
        for (i, spec) in ACTIONS.iter().enumerate() {
            for chord in map.chords_at(i) {
                if chord.mods.contains(KeyModifiers::SUPER) {
                    assert!(
                        crate::ghostty_config::releases(chord),
                        "{}: {}",
                        spec.id,
                        chord.spec()
                    );
                }
            }
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
