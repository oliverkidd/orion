//! TUI state: the Elm-ish Model.

use crate::git_diff::DiffFile;
use crate::pull_request::{OpenPr, PrDetail, PullRequest};
use crate::text_input::TextInput;
use orion_core::{
    Agent, AgentId, AgentKind, AgentStatus, Link, LinkId, Project, ProjectId, SessionRef,
    TerminalId, TerminalTab, Worktree, WorktreeId,
};
use ratatui::layout::{Position, Rect};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Frame duration of the status-sweep text animation: the event loop's
/// repaint cadence while [`App::status_anim_active`] holds, and the step
/// size of [`App::sweep_phase`] (one text cell per frame).
pub const SWEEP_FRAME: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a ONE-SHOT SWEEP runs: the sweep a row takes when it changes
/// into a state that wants you — a session starting to need you or
/// crashing (red), a checkout's pull request merging (purple) — before it
/// settles into its still color. Two or three passes of the band: long
/// enough to catch the eye from another panel, short enough that motion
/// keeps meaning *news*. The one sweep that does not run out is the UNREAD
/// SHIMMER: a finish nobody has looked at sweeps until somebody does.
pub const ONE_SHOT_SWEEP: std::time::Duration = std::time::Duration::from_secs(5);

/// Frames in one pass of the STACK TRAIL: the light runs into a running
/// stack's `⬡` once every eight seconds — long enough for a full-width
/// rule, rare enough to stay out of the way.
pub const STACK_TRAIL_PERIOD: usize = 80;

/// Frames of the WORKING SPINNER, the mark a running session wears: a
/// star that breathes — out from its full `✻` to a `·` and back — one
/// frame every [`SPIN_FRAMES_PER_STEP`] sweep frames. Every frame is one
/// cell and a shape no other status uses (`●` `○` `◌` `✕`), so the mark
/// still reads with the colour gone. The first frame is the one held
/// still with the animations off.
pub const SPINNER: [&str; 10] = ["✻", "✽", "✻", "✶", "✳", "✢", "·", "✢", "✳", "✶"];
/// Sweep frames ([`SWEEP_FRAME`]) per frame of the spinner.
pub const SPIN_FRAMES_PER_STEP: usize = 1;

/// The WORKING SPINNER's frame at sweep phase `phase`.
pub fn spin_step(phase: usize) -> usize {
    phase / SPIN_FRAMES_PER_STEP
}

/// The WORKING SPINNER's glyph at `spin` ([`App::spin_phase`]); its first
/// frame, held still, with animations off.
pub fn spinner_frame(spin: Option<usize>) -> &'static str {
    SPINNER[spin.unwrap_or(0) % SPINNER.len()]
}

/// How many recently shown sessions keep their screen ([`App::term_cache`]):
/// enough for a rotation through the sessions of a couple of worktrees.
/// What bounds the memory is [`TERM_CACHE_CELLS`], not this.
pub const TERM_CACHE_MAX: usize = 6;
/// The most the kept screens may hold between them, in grid cells (32 bytes
/// each, so about 12 MB — half of what two entries of that size each used to
/// be allowed). An alt-screen CLI is a screen's worth, 200 KB; a shell whose
/// 10 000-line scrollback has filled is tens of megabytes on its own, and
/// used never to be kept at all — every return to it re-parsed the whole
/// ring, 33 ms of blank pane under the INPUT LATENCY PROBE. Now a screen
/// that does not fit is kept WITHOUT its history ([`AttachedTerm::
/// drop_history`]): the return paints on the keypress like any other, and
/// the history is replayed if the user scrolls up into it.
pub const TERM_CACHE_CELLS: usize = 400_000;

/// The git repository orion was started in: the directory it was launched
/// from, or the nearest one above it holding a `.git` — what the first
/// run's SPLASH offers to open with Enter, and what the open-project prompt
/// starts on. None when started outside a repository, or at the home
/// directory itself: a dotfiles repo at `~` is not the project anyone
/// launching from there means.
pub fn launch_repo() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let home = orion_core::env::home_dir();
    cwd.ancestors()
        .take_while(|dir| Some(*dir) != home.as_deref())
        .find(|dir| dir.join(".git").exists())
        .map(std::path::Path::to_path_buf)
}

pub use orion_core::clock::now_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Projects,
    Worktrees,
    Sessions,
    Terminal,
}

/// What a screen cell maps to; rebuilt on every draw for hit-testing.
#[derive(Debug, Clone, PartialEq)]
pub enum HitTarget {
    /// The GRID's background (registered after the cards, so they win).
    PanelBg(Focus),
    TerminalPane,
    /// A card in the LAUNCHER VIEW's GRID — a session or a terminal — by
    /// its place in `launcher::bands`: a click lands the cursor on it,
    /// inside its worktree.
    LauncherCard(crate::launcher::CardRef),
    /// A BAND's rule on the grid — the checkout's own row over its cards,
    /// by the band's place in `launcher::bands`: a click puts the cursor
    /// on the band, as `j`/`k` walking onto it do.
    LauncherBand(usize),
    /// A port on a RUN LINE — `:3000`, after its command — registered
    /// ahead of the line, so it wins: a click opens that port on
    /// localhost in the browser (`stacks::open_port`). The line's terminal
    /// and the port.
    RunPort(TerminalId, u16),
    /// An ACCOUNT CHIP — on a grid row, or the PANE's header — by the
    /// account's registry id, registered ahead of what it sits on, so it
    /// wins: the pointer resting on it puts the account and what is left
    /// on it in the FOOTER, and a click opens ACCOUNT USAGE on it.
    AccountTag(String),
    /// The PULL REQUEST on a band's rule — its `↗ #42 title` — registered
    /// ahead of the rule, so it wins: a click opens the pull request in
    /// the browser, through the very `launcher::open_pull_request` `⇧V`
    /// runs. Only a checkout with a pull request has one, and it is only
    /// as wide as its text, so the rest of the rule is still the band's.
    LauncherBandPr(WorktreeId),
    /// The `▸ 3 archived` line under a BAND, by the band's place in
    /// `launcher::bands`: a click puts the cursor on the band and folds
    /// or unfolds its ARCHIVED DRAWER, as `z` does
    /// (`event_loop::launcher::click_drawer`).
    LauncherDrawer(usize),
    /// An archived session's line in an unfolded ARCHIVED DRAWER: the
    /// band's place, then the line's. A click lands the cursor on it; a
    /// second brings it back (`event_loop::launcher::click_drawer_entry`).
    LauncherDrawerEntry(usize, usize),
    /// The `‹ sessions` crumb in a full-screen session's header
    /// (LAUNCHER VIEW): a click leaves the session for the grid, as `^q`
    /// does.
    LauncherCrumb,
    /// A PROJECT TAB in the LAUNCHER VIEW's header, by the project it
    /// names: a click opens that project's sessions, as `[` and `]`
    /// walking onto it do, and a right-click opens it with the project's
    /// own menu over it — new worktree, run, rename, remove.
    LauncherTab(ProjectId),
    /// The `×` on that tab. Its own target rather than a corner of the
    /// tab's, so a click on the cross can never read as a click on the
    /// tab it closes.
    LauncherTabClose(ProjectId),
    /// The `+` after the last tab: a click drops the PROJECT DROPDOWN
    /// under it — every project, the one in front of you ticked, narrowed
    /// by type-ahead, with a row for opening a folder that is not one yet
    /// — and the pick opens a tab.
    LauncherTabAdd,
    /// The MORE CHIP after the last tab drawn — `2 more ▾` — standing
    /// in for the tabs the row had no room for: a click, or a
    /// right-click, drops the list of just those under it
    /// ([`App::launcher_tabs_more`]), and the pick opens that project.
    LauncherTabMore,
    /// Draggable top edge of the LAUNCHER VIEW's PANE: the blank row the
    /// pane opens with, plus the grid row above it. Registered ahead of
    /// the cards so a card ending on that row never swallows the grab.
    LauncherPaneSplitter,
    /// The CLOSE BUTTON at the right end of that header: a click folds
    /// the pane away, the same `event_loop::launcher::toggle_pane` `^~`
    /// runs.
    LauncherPaneClose,
    /// The FULL-SCREEN BUTTON before those in the pane's header, and the
    /// NORMAL-SIZE BUTTON at the right end of a full-screen session's: a
    /// click toggles between the two, the same
    /// `event_loop::launcher::toggle_full_screen` `^F` runs.
    LauncherPaneZoom,
    /// The PR COUNT on the right of the LAUNCHER VIEW's header (`2 prs`):
    /// a click opens the open pull requests of the project in front of
    /// you — the modal `v` opens.
    LauncherPullRequests,
    /// The key cap in the empty GRID's welcome (`press ⌘N to start an agent`): a
    /// click opens the QUICK PROMPT, through the very
    /// `event_loop::launcher::open_box` the key runs.
    LauncherWelcomePrompt,
    /// HOME's own sky: a click anywhere on it goes back down to the grid,
    /// as Esc does.
    HomeBack,
    /// The grid header's `⇡ v…` and HOME's upgrade line: **Upgrade orion**.
    Upgrade,
    /// One of the footer's SPOTIFY READOUT buttons (`⏮  ⏸ ⏭`): a click
    /// sends Spotify that command (`event_loop::press_spotify`).
    FooterSpotify(crate::spotify::Button),
    /// The footer's `⌘K needs you N`: a click opens the NEEDS YOU BOARD,
    /// through the `open_palette` its key runs.
    FooterNeedsYou,
    /// The `↗ open in browser` BUTTON on the PULL REQUESTS MODAL's and the
    /// LINEAR VIEW's reading pane, pinned right on its top border: a click
    /// opens the row under the cursor in the browser, through the very
    /// function the modal's `o` runs (`pr_modal::open_in_browser`). The
    /// modals route their own mouse off the rects they write back at draw
    /// time, so this is never in the hit map: it is what `hover_crumb` holds while the
    /// pointer rests on the button (`ui::browser_button_under`), so the
    /// draw can underline it.
    ModalBrowser,
    /// A tab's label on the PULL REQUEST PAGE the pane reads
    /// (`pr_preview`): a click shows that tab, the pane taking the keys.
    /// Registered ahead of the pane, so it wins.
    PrPageTab(crate::pr_preview::PrTab),
    /// A listed row on that page — a file, a commit, a check — by its
    /// place in the tab: a click is Enter on it. Registered ahead of the
    /// pane, so it wins.
    PrPageRow(usize),
    /// A `<details>` summary on that page's description or comments: a
    /// click opens or shuts it. Registered ahead of the pane, so it wins.
    PrPageFold(crate::markdown::FoldKey),
    /// A session, an issue or `⌘G send to the PR fixer` in that page's WORK BLOCK
    /// (`pr_work`): a click lands on the session in the grid, opens the
    /// issue, or is `⌘G`. Registered ahead of the pane, so it wins.
    PrPageWork(crate::pr_work::WorkHit),
}

/// Default outer width of the diff modal's left column — its commits over
/// its files — room for a commit's subject on one row and each file's
/// path beside its counts. A modal under twice this keeps it to half.
pub const DEFAULT_DIFF_FILES_W: u16 = 68;
/// What [`DEFAULT_DIFF_FILES_W`] was before the column doubled. Every UI
/// blob saved since remembers it as if it had been dragged there, so a
/// restored 34 is read as never dragged.
pub const OLD_DEFAULT_DIFF_FILES_W: u16 = 34;
/// Default outer width of the TREE BROWSER's tree.
pub const DEFAULT_TREE_FILES_W: u16 = 34;
/// The diff modal's file list can't be dragged narrower than this.
pub const MIN_DIFF_FILES_W: u16 = 16;
/// How long the settings overlay remembers its tab / row / strip-vs-list
/// after closing. Reopened within this, it lands where you left it; later
/// than this the memory is stale and it opens fresh on the tab strip.
pub const SETTINGS_MEMORY_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// The diff pane always keeps at least this much width.
pub const MIN_DIFF_PANE_W: u16 = 24;

// ---- list-view arithmetic shared by every overlay with a cursor ----

/// First visible row of a stateless follow-window over a list: the window
/// slides only as far as it must to keep `selected` on its last row. One
/// definition so every overlay list scrolls the same way.
pub fn window_start(selected: usize, height: usize) -> usize {
    (selected + 1).saturating_sub(height)
}

/// Clamp an absolute cursor request onto a list of `len` rows: negative
/// lands on the first row, past-the-end on the last, and an empty list on
/// 0 — the same rule every overlay applies before indexing.
pub fn clamp_selection(index: i64, len: usize) -> usize {
    let max = len.saturating_sub(1) as i64;
    index.clamp(0, max) as usize
}

/// Furthest a pane of `view_height` rows can scroll into `lines` lines —
/// the scroll that puts the last line on the bottom row. Zero when it all
/// fits. Shared by the diff pane and the tree preview.
pub fn max_scroll(lines: usize, view_height: u16) -> u16 {
    (lines as u16).saturating_sub(view_height.max(1))
}

/// `scroll` moved by `delta` and held within `0..=max`.
pub fn scrolled_by(scroll: u16, delta: i32, max: u16) -> u16 {
    (scroll as i32 + delta).clamp(0, max as i32) as u16
}

/// Width of a split modal's left list when its boundary is dragged to
/// screen column `boundary_x`, clamped so the list keeps `MIN_DIFF_FILES_W`
/// and the right pane keeps `MIN_DIFF_PANE_W`. `None` when `area` is too
/// narrow to honor both minimums — the caller leaves the width alone.
pub fn clamp_files_width(area: Rect, boundary_x: i32) -> Option<u16> {
    let max = area.width.saturating_sub(MIN_DIFF_PANE_W);
    if max < MIN_DIFF_FILES_W {
        return None; // modal too small to honor the minimums
    }
    let want = (boundary_x - area.x as i32).max(0) as u16;
    Some(want.clamp(MIN_DIFF_FILES_W, max))
}

// ---- overlays ----

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Attach(SessionRef),
    RestartAgent(AgentId),
    RenameAgent(AgentId),
    ArchiveAgent(AgentId),
    UnarchiveAgent(AgentId),
    DeleteAgent(AgentId),
    NewAgent(WorktreeId),
    /// Picker result: create an agent of this kind (chains into the NEW
    /// SESSION box — a PR SESSION into its name prompt). `model`/`effort`
    /// are submenu choices: None means the row
    /// hasn't drilled into that submenu (its configured default applies);
    /// "default" is the submenu row that picks the default explicitly.
    NewAgentOfKind {
        worktree: WorktreeId,
        kind: AgentKind,
        /// Registry id when `kind` is [`AgentKind::Custom`]; None for
        /// built-ins. Carried through the MODEL submenu, the QUICK PROMPT
        /// box and the launch draft into `CreateAgent::custom_harness`.
        custom: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        /// OPEN PRS launch context (a PR SESSION): the pull request and
        /// the head branch its worktree is checked out on. Valid for every
        /// kind, and preserved through model/effort submenus and the name
        /// prompt.
        pr: Option<crate::pull_request::PrLaunch>,
        /// Set when the picker was opened from the QUICK PROMPT's `Tab`:
        /// the box to put back (with its typed text) instead of creating a
        /// session. Rides through the MODEL / EFFORT submenus like
        /// `pr`, and is what an abandoned picker restores.
        quick: Option<Box<crate::quick_prompt::QuickReturn>>,
    },
    /// Shell terminal in the worktree's directory; created immediately with
    /// a default name (no prompt), renameable later.
    NewTerminal(WorktreeId),
    RenameTerminal(TerminalId),
    CloseTerminal(TerminalId),
    NewWorktree(ProjectId),
    /// Hand a link row's URL to the browser — the one `account`'s **Open
    /// links in** picks, for a link that belongs to a session.
    OpenLink {
        url: String,
        account: Option<String>,
    },
    /// Read the selected open pull request's diff in the diff modal. Carries
    /// no id: the row is the selection, and the fetch reads it back off the
    /// cursor the same way `g` does.
    ViewPrDiff,
    /// Open the COMMENT BOX on the selected pull request. Like
    /// `ViewPrDiff`, carries no id: the row is the selection, and `y`
    /// reads it off the cursor the same way.
    CommentPullRequest,
    /// **Check out** on a pull request row: its checkout, found or cut
    /// (`pr_checkout`).
    CheckOutPr {
        project: ProjectId,
        pr: crate::pull_request::PrLaunch,
    },
    /// Expand the selected session card into its FOLLOW-UP COMPOSER, or
    /// fold it back up. Carries no id for the same reason `ViewPrDiff`
    /// doesn't: the card is the selection, and Space reads it off the
    /// cursor the same way.
    FollowUp,
    /// A card menu's **Duplicate**: the QUICK PROMPT on this session's
    /// settings — `launcher::duplicate_agent`, what `⇧P` runs on the card
    /// under the cursor. Carries the id, as the row's other verbs do.
    DuplicateAgent(AgentId),
    /// **Continue on** another account: carry this Claude session onto the
    /// harness `harness` (a registry id) — another account — and resume
    /// it there (`ClientRequest::ContinueAgentOn`).
    ContinueOn {
        id: AgentId,
        harness: String,
    },
    EditLink(LinkId),
    DeleteLink(LinkId),
    DeleteWorktree(WorktreeId),
    /// A band's **Pull**: fetch and fast-forward its checkout (`p`).
    PullWorktree(WorktreeId),
    /// A band's **Push**: send its branch's new commits up (`⇧P`).
    PushWorktree(WorktreeId),
    /// Start the worktree's RUN COMMAND, or stop it while it runs (`r`).
    ToggleRun(WorktreeId),
    /// A run line's **Restart stack**: stop the worktree's run, then start
    /// it again once it has gone.
    RestartRun(WorktreeId),
    /// Fire the worktree's OPEN COMMAND (`Shift+Enter`).
    OpenWorktree(WorktreeId),
    AddProject,
    RemoveProject(ProjectId),
    /// Retitle a project's row. Display only — the folder keeps its name and
    /// stays visible under the new one.
    RenameProject(ProjectId),
    /// PROJECT DROPDOWN row (the `+` after the LAUNCHER VIEW's
    /// PROJECT TABS): open this project — its sessions, and a tab for it
    /// first, next to the `+`, if it had none.
    OpenProject(ProjectId),
    ToggleArchived,
    /// Fold / unfold the PROJECT OPEN PRS GROUP (Worktrees panel menu).
    ToggleOpenPrs,
    /// Flip the `hide_draft_prs` SETTING from the Worktrees panel menu:
    /// drafts out of the group and `/`, or back in.
    ToggleDraftPrs,
    /// A row of the WORKTREE PICKER `^T` (or a click on the NEW AGENT
    /// box's branch) opens: aim this one launch at `target` — one of the project's
    /// checkouts, or a fresh worktree — and hand the box back with its
    /// text. It picks where the session runs, never what branch a checkout
    /// is on.
    PickLaunchWorktree {
        target: crate::quick_prompt::QuickTarget,
        back: Box<crate::quick_prompt::QuickReturn>,
    },
    /// A COMMAND PALETTE or OPEN MENU row: run the action as its key does.
    RunAction(crate::keymap::Action),
    /// A card menu's **Open Linear issue**: the LINEAR VIEW on the issue
    /// its checkout works on (`linear::worktree_issue`).
    OpenLinearIssue {
        project: ProjectId,
        id: String,
    },
}

/// Which submenu → (right arrow) opens from a menu row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmenuKind {
    /// Model list for a Claude/Codex session (new-session picker rows).
    Models,
    /// Effort list, offered once a model row is highlighted.
    Efforts,
}

impl MenuAction {
    /// The submenu this action's row expands into, if any. Drives both the
    /// `▸` indicator and the → key. New-session rows drill kind → model →
    /// effort; a row that already carries an effort is a leaf, and so is
    /// a model row whose effort list is empty (a Cursor family with no
    /// effort variants, like `auto`).
    pub fn submenu(&self) -> Option<SubmenuKind> {
        match self {
            MenuAction::NewAgentOfKind {
                kind,
                custom,
                model,
                effort,
                ..
            } => {
                if crate::config::model_choices(*kind, custom.as_deref()).is_empty() {
                    return None;
                }
                match (model, effort) {
                    (None, None) => Some(SubmenuKind::Models),
                    (Some(m), None)
                        if !crate::config::effort_choices(*kind, Some(m), custom.as_deref())
                            .is_empty() =>
                    {
                        Some(SubmenuKind::Efforts)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    pub destructive: bool,
    /// Right-aligned key hint, used by the command palette and Open menu.
    pub hint: Option<String>,
    /// The hint is a dim note about the row (the NEW AGENT PICKER's
    /// status note), not a key.
    pub note: bool,
}

impl MenuItem {
    /// A plain menu row.
    pub fn new(label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            action,
            destructive: false,
            hint: None,
            note: false,
        }
    }

    /// A row drawn in the warning color: it deletes, closes, or removes.
    pub fn destructive(label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            destructive: true,
            ..Self::new(label, action)
        }
    }

    /// Pin a key hint to the right edge of the row.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Pin a dim note to the right edge of the row, where a key hint goes.
    pub fn with_note(self, note: impl Into<String>) -> Self {
        Self {
            note: true,
            ..self.with_hint(note)
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    /// Optional title rendered in the border (used by picker-style menus).
    pub title: Option<String>,
    pub items: Vec<MenuItem>,
    /// Anchor position for context menus; `None` centers the menu in the
    /// frame (used by picker-style menus opened from the keyboard).
    pub at: Option<(u16, u16)>,
    pub hover: usize,
    /// Set during draw for click hit-testing.
    pub area: Rect,
    /// The menu ← returns to when this one is a submenu.
    pub parent: Option<Box<ContextMenu>>,
    /// Type-ahead over the rows — set on the MODEL / EFFORT submenus, where
    /// letters filter instead of moving (↑/↓ move there); None elsewhere.
    pub filter: Option<MenuFilter>,
}

/// A picker submenu's type-ahead: the query typed so far and the full row
/// set it narrows (`items` holds the visible subset, best match first).
#[derive(Debug, Clone)]
pub struct MenuFilter {
    pub query: String,
    pub all: Vec<MenuItem>,
    /// The most rows shown at once, for a list that can run to hundreds —
    /// the WORKTREE PICKER's branches. The menu does not scroll, so the
    /// rows past it are reached by typing. None shows every match.
    pub limit: Option<usize>,
}

impl ContextMenu {
    /// Narrow the rows to `query` (fuzzy, best match first; the full list
    /// in its own order when empty, hovering the ✓ row). Returns false —
    /// and changes nothing — when no row matches, so the list never
    /// empties; a no-op on a menu without a filter.
    pub fn set_filter(&mut self, query: &str) -> bool {
        let Some(filter) = &self.filter else {
            return false;
        };
        let labels: Vec<&str> = filter.all.iter().map(|i| i.label.as_str()).collect();
        let ranked = crate::fuzzy::rank(query, labels.iter().copied());
        if ranked.is_empty() {
            return false;
        }
        let items: Vec<MenuItem> = ranked
            .iter()
            .take(filter.limit.unwrap_or(usize::MAX))
            .map(|(i, _)| filter.all[*i].clone())
            .collect();
        self.hover = if query.trim().is_empty() {
            items
                .iter()
                .position(|i| i.label.ends_with(" ✓"))
                .unwrap_or(0)
        } else {
            0
        };
        self.items = items;
        if let Some(filter) = &mut self.filter {
            filter.query = query.to_string();
        }
        true
    }

    /// Append one typed character to the filter; false when it would leave
    /// no rows (nothing changes).
    pub fn type_filter(&mut self, c: char) -> bool {
        let query = format!("{}{c}", self.filter_query());
        self.set_filter(&query)
    }

    /// Backspace: drop the last character (widens, so it always succeeds).
    pub fn pop_filter(&mut self) {
        let mut query = self.filter_query().to_string();
        query.pop();
        self.set_filter(&query);
    }

    pub fn filter_query(&self) -> &str {
        self.filter.as_ref().map_or("", |f| f.query.as_str())
    }

    pub fn has_filter_text(&self) -> bool {
        !self.filter_query().is_empty()
    }

    /// Is this the LAUNCHER VIEW's PROJECT DROPDOWN? Its rows are
    /// OpenProject, and it gates its footer hint.
    pub fn is_project_picker(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.action, MenuAction::OpenProject(_)))
    }

    /// Is this the WORKTREE PICKER `^T` (or a click on the NEW AGENT
    /// box's branch) opens? It is drawn hanging from that branch.
    pub fn is_launch_worktree_picker(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.action, MenuAction::PickLaunchWorktree { .. }))
    }

    /// The harnessed launch under the cursor, if the hovered row starts
    /// one: the NEW AGENT PICKER, its PR sibling, the quick prompt
    /// picker, and their model/effort submenus all carry it. Gates the
    /// `?` jump to agent settings.
    pub fn hovered_agent_kind(&self) -> Option<(AgentKind, Option<String>)> {
        match &self.items.get(self.hover)?.action {
            MenuAction::NewAgentOfKind { kind, custom, .. } => Some((*kind, custom.clone())),
            _ => None,
        }
    }
}

/// Destructive action waiting behind a confirmation.
#[derive(Debug, Clone, PartialEq)]
pub enum PendingAction {
    /// AddProject aimed at a path that doesn't exist yet: create the
    /// directory, `git init` it (both daemon-side) and add it.
    CreateProjectDir(std::path::PathBuf),
    /// AddProject aimed at a folder that is in no git repository: `git
    /// init` it (daemon-side) and add it. A project is a repository, so
    /// the folder is never added as it stands.
    InitProjectRepo(std::path::PathBuf),
    /// `a` (or the row menu's Archive): archive the agent once the dialog
    /// is answered.
    ArchiveAgent(AgentId),
    DeleteAgent(AgentId),
    CloseTerminal(TerminalId),
    DeleteWorktree(WorktreeId),
    /// `p` on a checkout an agent is working in: pull it anyway.
    PullWorktree(WorktreeId),
    /// **Delete all sessions** in the worktrees panel: every deletable
    /// worktree of the selected project.
    DeleteAllWorktrees(Vec<WorktreeId>),
    /// The daemon answered a worktree delete with `WorktreeHasChanges`:
    /// these checkouts hold uncommitted or untracked files, and only a
    /// forced delete removes them — losing those files, never the commits
    /// on their branches.
    ForceDeleteWorktrees(Vec<WorktreeId>),
    /// **Delete all sessions**: every session row the panel currently
    /// shows — agents and terminals both.
    DeleteAllSessions {
        agents: Vec<AgentId>,
        terminals: Vec<TerminalId>,
    },
    RemoveProject(ProjectId),
    /// `d`, `Delete` or `Backspace` with the PROJECT TABS holding the
    /// keys: close the tab under the header's cursor once the dialog is
    /// answered (`event_loop::launcher::close_cursor_tab`, the `x`).
    CloseProjectTab(ProjectId),
    DeleteLink(LinkId),
    /// `^D` on one of the user's presets in Settings → Presets: drop the
    /// preset at `index` of the list, then the tab again.
    DeletePreset {
        index: usize,
    },
    /// `^R` on a system preset in Settings → Presets: drop the user's
    /// text from the preset at `index`, so it says orion's again.
    RestorePresetText {
        index: usize,
    },
    /// `R` in the settings overlay: rewrite config.json from the defaults
    /// (every setting and every hotkey), then reopen the overlay on them.
    ResetSettings,
    /// **Add account**, its name typed: create it, sharing the default
    /// account's setup (`Enter`/`y`) or not (`n`). Every answer reopens
    /// the settings overlay.
    AddClaudeAccount(crate::claude_accounts::NewAccount),
    /// `o` on a CLAUDE ACCOUNTS row: `claude auth logout` for account
    /// `id`, in the editor modal over the settings overlay.
    SignOutClaude {
        id: String,
    },
    /// `⌫` on an added account's row: take it out of config.json —
    /// keeping its config dir (`Enter`/`y`), listed under SAVED ON THIS
    /// MACHINE from then on, or moving it to the Trash (`t`). Every answer
    /// reopens the settings overlay.
    RemoveClaudeAccount {
        id: String,
    },
    Quit,
    /// **Restart orion** (`⌘⇧R`): quit, stop the daemon and every session
    /// in it, and start the binary again (`App::restart`).
    Restart,
    /// **Upgrade orion** (`⌘⇧G`): quit into `orion upgrade`, then
    /// start the TUI again on the new binary (`App::upgrade`).
    /// `restarts_daemon` is whether the release changes the daemon, which
    /// takes every session down with it (`update_check::restarts_daemon`):
    /// a red warning when it does, a green go-ahead when it doesn't.
    Upgrade {
        restarts_daemon: bool,
    },
}

impl PendingAction {
    /// Whether the dialog asks about losing something — the red frame
    /// every confirm wears but **Add account**'s question, a PULL's, and an
    /// upgrade that leaves the daemon running, which lose nothing either
    /// way.
    pub fn destructive(&self) -> bool {
        !matches!(
            self,
            PendingAction::AddClaudeAccount(_)
                | PendingAction::PullWorktree(_)
                | PendingAction::Upgrade {
                    restarts_daemon: false
                }
        )
    }

    /// Whether the dialog stands in for the settings overlay, which every
    /// answer — Esc too — puts back.
    pub fn from_settings(&self) -> bool {
        matches!(
            self,
            PendingAction::ResetSettings
                | PendingAction::AddClaudeAccount(_)
                | PendingAction::SignOutClaude { .. }
                | PendingAction::RemoveClaudeAccount { .. }
                | PendingAction::DeletePreset { .. }
                | PendingAction::RestorePresetText { .. }
        )
    }
}

#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: String,
    pub message: String,
    pub action: PendingAction,
    /// Full dialog rect, written back during draw (the `ContextMenu::area`
    /// pattern) so a click outside it can cancel like Esc.
    pub area: Rect,
}

/// The `?` keymap overlay. Carries nothing but its drawn rect, so a click
/// outside the box can close it.
#[derive(Debug, Clone, Default)]
pub struct HelpView {
    pub area: Rect,
}

/// What a PROMPT DIALOG is for. One is up at a time, inside
/// `Overlay::Prompt`, so the QUICK PROMPT's launch spec riding unboxed
/// costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    AddProject,
    NewWorktree {
        project: ProjectId,
        /// Random `<adj>-<noun>-<verb>` name minted when the prompt
        /// opened; shown in the label and used when Enter arrives on an
        /// empty input, so the name offered is the name created.
        suggestion: String,
    },
    /// The QUICK PROMPT's task: one multi-row box, opened by its hotkey
    /// from anywhere, that launches an AGENT in the selected WORKTREE with
    /// the typed text as its STARTING PROMPT. It carries the whole launch
    /// spec, resolved when the dialog opens so the title can show what
    /// Enter is about to start — and rewritten in place by the box's `Tab`
    /// / `Shift+Tab` pickers. The NEW AGENT PICKER never ends here: its
    /// pick creates the session outright.
    QuickPrompt(crate::quick_prompt::QuickLaunch),
    /// The next turn for a session already running, typed into a small
    /// modal and sent straight down that session's PTY — the LAUNCHER
    /// VIEW's follow-up, where the SESSIONS PANEL expands the card itself
    /// ([`App::follow_up`]). A grid of fixed-height cards has nowhere to
    /// grow a box, and a modal is what lets one card after another be
    /// prompted without ever stepping into a session. Multi-row: a turn
    /// is usually one line, but never only one line.
    FollowUp {
        id: AgentId,
    },
    /// A comment to post on the pull request under the cursor — a
    /// PROJECT OPEN PRS GROUP row or the Sessions panel's PR ROW — with
    /// `gh pr comment` (`y`, or **Comment…** from the row's menu).
    /// Multi-row: a review note is rarely one line, and it is markdown.
    /// Carries the pull request the box was opened on, so Enter posts where
    /// the title said it would.
    PrComment {
        number: u64,
        url: String,
        /// Row text, `#42 title` — what the box is titled with.
        label: String,
        /// The PULL REQUESTS MODAL the box stood in for (`c` there): Enter,
        /// Esc and an empty box all put it back on its row. None from the
        /// panels, where the box closes onto them. Boxed: the view is
        /// several times the size of the other variants.
        back: Option<Box<crate::pr_modal::PullRequestsView>>,
    },
    RenameAgent {
        id: AgentId,
    },
    RenameTerminal {
        id: TerminalId,
    },
    /// Retitle a project's row. The folder on disk is untouched; an empty
    /// name puts the row back on the folder's own name.
    RenameProject {
        id: ProjectId,
    },
    /// A typed SETTINGS OVERLAY row (`SettingKind::is_text`), opened by
    /// Enter on it: the one-line prompt stands in for the overlay while the
    /// value is typed, and both Enter and Esc put the overlay back on the
    /// row it left. An empty value is the row's default, not a cancel.
    /// `project` is the repo path of the project a PROJECT TAB row
    /// (`SettingKind::is_project`) was opened on, so the value lands in
    /// that project's entry even if the selection moves meanwhile; None
    /// for a top-level row.
    SettingText {
        kind: crate::config::SettingKind,
        project: Option<std::path::PathBuf>,
    },
    /// Rewrite a pinned link's URL.
    EditLink {
        id: LinkId,
    },
    /// Enter on a CLAUDE ACCOUNTS row (Settings → Accounts): the email to
    /// sign account `id` in as, which fills Claude's login page — empty
    /// leaves the choice to the browser. Enter runs `claude auth login` in
    /// the editor modal over the settings overlay; Esc puts the overlay
    /// back.
    ClaudeSignIn {
        id: String,
    },
    /// Enter on **Add account**: the new account's name — empty for the
    /// next `claude-N`, with no name. Enter asks whether to share the
    /// default account's setup; Esc puts the overlay back.
    AddClaudeAccount,
    /// `r` on a CLAUDE ACCOUNTS row: the name account `id` goes by,
    /// prefilled with the one it has. Enter saves it — empty takes it
    /// away, back to `Claude (a@b.co)` — and Esc keeps it; both put the
    /// overlay back on the row. The id and the dir never change.
    RenameClaudeAccount {
        id: String,
    },
    /// `g` on a CLAUDE ACCOUNTS row: the letter or glyph account `id`'s
    /// ACCOUNT CHIP shows, prefilled with the one it was given. Enter
    /// saves its first character — empty puts orion's letter back — and
    /// Esc keeps it; both put the overlay back on the row.
    AccountChipMark {
        id: String,
    },
    /// Enter on an account's **Open links in** row: the browser, then a
    /// profile after it (`Chrome Profile 1`), prefilled with the one it
    /// has. Enter saves it — empty puts the default browser back — and Esc
    /// keeps it; both put the overlay back on the row.
    LinkBrowser {
        id: String,
    },
    /// `c` on Settings › Connections › SLACK: token `step` of the three
    /// (`phone::TOKENS`), the ones typed before it carried along. Enter
    /// asks for the next, and after the third saves all three; Esc and an
    /// empty box put the overlay back with nothing saved.
    SlackToken {
        step: usize,
        tokens: Vec<String>,
    },
    /// `c` on Settings › Connections › MICROSOFT 365 with no app in the
    /// config: the Application (client) ID, then — `client_id` carried
    /// along — the Directory (tenant) ID; both on one line skip the
    /// second. After both, they are saved to the config and the sign-in
    /// starts; Esc and an empty box put the overlay back with nothing
    /// saved (`microsoft::submit_app_id`).
    MicrosoftAppId {
        client_id: Option<String>,
    },
}

impl PromptKind {
    /// Does this box's text become a turn for an agent on this machine —
    /// one that can open a file path written into it? The rest aren't
    /// prompts at all.
    pub fn reaches_local_agent(&self) -> bool {
        matches!(
            self,
            PromptKind::QuickPrompt(_) | PromptKind::FollowUp { .. }
        )
    }

    /// The checkout this box is addressed to — the one its Enter launches
    /// into — to rewrite when a stand-in becomes the real row. None for a
    /// box that names no checkout, and for a QUICK PROMPT about to cut
    /// one of its own.
    pub fn worktree_mut(&mut self) -> Option<&mut WorktreeId> {
        match self {
            PromptKind::QuickPrompt(launch) => launch.worktree_mut(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromptDialog {
    pub title: String,
    pub label: String,
    pub input: TextInput,
    pub kind: PromptKind,
    /// Live directory listing under the input (path prompts only): the
    /// typed parent's subdirectories narrowed by the partial segment.
    pub dirs: Vec<crate::completion::DirEntry>,
    /// Listing row highlighted by ↓↑; None = the typed path itself.
    pub hover: Option<usize>,
    /// Screen rect of the listing rows, written during draw for click
    /// hit-testing.
    pub list_area: Rect,
    /// Full dialog rect, written during draw so a click outside it can
    /// abandon the prompt like Esc.
    pub area: Rect,
    /// The text inside a task box's editor (multi-row prompts only),
    /// written during draw: a click there puts the caret where it points,
    /// and the wheel over it scrolls the text.
    pub editor_area: Rect,
    /// The QUICK PROMPT header's fields — `project demo ⌘P`, `worktree
    /// main ⌘.`, `agent Claude Tab`, `model opus ⌘/`, `effort high ⌘Y`,
    /// a preset's — and the cells each was drawn in, written during draw:
    /// a click on one opens the picker its key opens. Empty on every
    /// other box, and on a box too narrow to draw a field at all.
    pub detail_areas: Vec<(crate::launcher::BoxField, Rect)>,
    /// The branch alone in that header's `worktree main ⌘.`, written
    /// during draw: the WORKTREE PICKER hangs from it. Empty wherever the
    /// branch is not drawn or names nothing to pick — a PR SESSION's
    /// checkout is the DAEMON's.
    pub branch_area: Rect,
    /// A QUICK PROMPT opened on the SAVED DRAFT or the DRAFT slot rather
    /// than empty: its explanation line leads with `draft restored`, until
    /// the first edit makes the text simply what is being typed.
    pub draft_restored: bool,
    /// The FILE MENTION list an `@` puts up in a box whose text goes to a
    /// local agent (`mention`).
    pub mention: crate::mention::Mention,
}

impl PromptDialog {
    pub fn new(
        title: impl Into<String>,
        label: impl Into<String>,
        input: impl Into<String>,
        kind: PromptKind,
    ) -> Self {
        let mut prompt = Self {
            title: title.into(),
            label: label.into(),
            input: TextInput::with_text(input),
            kind,
            dirs: Vec::new(),
            hover: None,
            list_area: Rect::default(),
            area: Rect::default(),
            editor_area: Rect::default(),
            detail_areas: Vec::new(),
            branch_area: Rect::default(),
            draft_restored: false,
            mention: crate::mention::Mention::default(),
        };
        // The task and comment boxes hold line breaks; the rest are one
        // line. The field itself then knows which keys break a line and
        // whether a paste keeps its newlines.
        let multiline = prompt.is_multiline();
        prompt.input.set_multiline(multiline);
        prompt.refresh_dirs();
        prompt
    }

    /// Does Tab complete filesystem paths in this prompt?
    pub fn completes_paths(&self) -> bool {
        matches!(self.kind, PromptKind::AddProject)
    }

    /// The task prompts — the QUICK PROMPT and the FOLLOW-UP — and a pull
    /// request comment are the ones with a multi-row editor.
    pub fn is_multiline(&self) -> bool {
        matches!(
            self.kind,
            PromptKind::FollowUp { .. }
                | PromptKind::PrComment { .. }
                | PromptKind::QuickPrompt { .. }
        )
    }

    fn home() -> Option<std::path::PathBuf> {
        orion_core::env::home_dir()
    }

    /// Recompute `dirs` from `input` after any edit; the hover returns to
    /// the input row. Non-path prompts keep an empty listing.
    pub fn refresh_dirs(&mut self) {
        self.hover = None;
        self.dirs = if self.completes_paths() {
            crate::completion::list_dirs(&self.input, Self::home().as_deref())
        } else {
            Vec::new()
        };
    }

    /// Full path of the hovered listing row (typed parent + entry name).
    pub fn hovered_path(&self) -> Option<String> {
        let entry = self.dirs.get(self.hover?)?;
        let (parent, _) = crate::completion::split_input(&self.input);
        Some(format!("{parent}{}", entry.name))
    }

    /// ↓↑ over the listing; Up from the first row returns to the input.
    pub fn move_hover(&mut self, delta: i32) {
        if self.dirs.is_empty() {
            return;
        }
        let next = self.hover.map_or(-1, |h| h as i32) + delta;
        self.hover = (next >= 0).then(|| (next as usize).min(self.dirs.len() - 1));
    }

    /// → (or a click) on listing row `i`: step into that directory.
    pub fn dive(&mut self, i: usize) {
        let Some(entry) = self.dirs.get(i) else {
            return;
        };
        let (parent, _) = crate::completion::split_input(&self.input);
        self.input.set_text(format!("{parent}{}/", entry.name));
        self.refresh_dirs();
    }

    /// ← steps up: a typed partial segment is cleared first; from a bare
    /// "dir/" the last segment is dropped. "~/" expands so browsing keeps
    /// working above the home directory.
    pub fn ascend(&mut self) {
        let (parent, partial) = crate::completion::split_input(&self.input);
        if !partial.is_empty() {
            let parent = parent.to_string();
            self.input.set_text(parent);
            self.refresh_dirs();
            return;
        }
        let mut path = self.input.to_string();
        if path == "~/" {
            match Self::home() {
                Some(home) => path = format!("{}/", home.display()),
                None => return,
            }
        }
        if path.len() <= 1 {
            return; // "" or "/" — nowhere further up
        }
        path.pop(); // the trailing '/'
        let cut = path.rfind('/').map(|i| i + 1).unwrap_or(0);
        path.truncate(cut);
        self.input.set_text(path);
        self.refresh_dirs();
    }

    /// First visible listing row of the stateless follow-window for a list
    /// `height` rows tall.
    pub fn window_start(&self, height: usize) -> usize {
        self.hover.map_or(0, |h| h + 1).saturating_sub(height)
    }
}

/// One visible row of the diff-view file list: an index into `files` plus
/// the char positions of `path` the filter matched, for highlighting.
#[derive(Debug, Clone)]
pub struct DiffMatch {
    pub file: usize,
    pub positions: Vec<usize>,
}

/// Which panel of the DIFF VIEWER has the keys — the one wearing the
/// accent. `Tab` walks them in reading order: what is on screen (the
/// COMMIT LIST), its files, the diff.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffFocus {
    /// `↑`/`↓` walk the commits, `Space` ticks one.
    Commits,
    /// `↑`/`↓` walk the files; typing filters them.
    #[default]
    Files,
    /// `↑`/`↓` scroll the diff — or, reviewing a pull request, walk its
    /// lines and threads (`pr_review::viewer`).
    Diff,
    /// A pull request's review bar along the bottom: the verdict and the
    /// summary (`pr_review::bar`).
    Bar,
}

/// A list's viewport as last drawn: its first row, following the cursor —
/// until the wheel scrolls it, when it stays where the wheel put it for as
/// long as the cursor stays where it was (`pinned_at`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListScroll {
    pub top: usize,
    pub pinned_at: Option<usize>,
}

impl ListScroll {
    /// The first row of a list of `len` one-line rows, `height` tall, with
    /// the cursor on `cursor`: the last top, moved just enough to show the
    /// cursor — or, scrolled by the wheel since the cursor last moved, held.
    pub fn start(&self, cursor: usize, len: usize, height: usize) -> usize {
        let max_top = len.saturating_sub(height.max(1));
        let top = self.top.min(max_top);
        if self.pinned_at == Some(cursor) {
            return top;
        }
        if cursor < top {
            cursor
        } else if cursor >= top + height.max(1) {
            (cursor + 1 - height.max(1)).min(max_top)
        } else {
            top
        }
    }

    /// The wheel: `delta` rows, the cursor left where it is.
    pub fn wheel(&mut self, delta: i64, cursor: usize, len: usize, height: usize) {
        let max_top = len.saturating_sub(height.max(1)) as i64;
        let top = self.start(cursor, len, height) as i64;
        self.top = (top + delta).clamp(0, max_top) as usize;
        self.pinned_at = Some(cursor);
    }
}

/// The DIFF VIEWER (`⌘E`): the COMMIT LIST over the changed files down the
/// left, the selected file's diff on the right.
#[derive(Debug, Clone)]
pub struct DiffView {
    /// Checkout dir the diffs are read from.
    pub root: PathBuf,
    /// Branch name for the pane title.
    pub branch: String,
    pub files: Vec<DiffFile>,
    /// Type-to-filter query over `files` paths; always live.
    pub filter: TextInput,
    /// Visible rows: `files` narrowed by `filter`, best matches first
    /// (git order when the filter is empty); reviewed ✓ files always sink
    /// to the bottom.
    pub matches: Vec<DiffMatch>,
    /// Index into `matches` (not `files`).
    pub selected: usize,
    /// Diff text of the selected file (reloaded on selection change): what
    /// a reviewed ✓ fingerprints.
    pub diff: String,
    /// `diff` read for the reading pane, under the REVIEW HEAD
    /// (`diff_doc`). Shared with every frame's clone of the view.
    pub doc: std::sync::Arc<crate::diff_doc::DiffDoc>,
    /// The diff pane's top row: rows, not lines, since lines wrap.
    pub scroll: usize,
    /// Inner height and text width of the diff pane, written back during
    /// draw (the `ContextMenu::area` pattern) so paging and clamping track
    /// resizes and the wrap. Zero until the first draw.
    pub view_height: u16,
    pub view_width: u16,
    /// Screen rect of the file-list rows (filter row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
    /// The file list's viewport.
    pub files_scroll: ListScroll,
    /// The diff pane, as last drawn: a click there hands it the keys, and
    /// the wheel over it scrolls it.
    pub diff_area: Rect,
    /// Full modal rect, written back during draw; bounds the file-panel
    /// splitter drag and hit-tests its border.
    pub area: Rect,
    /// Outer width of the left column as the reader wants it: dragged
    /// there, or [`DEFAULT_DIFF_FILES_W`] while never dragged — which the
    /// draw holds to half a narrow modal, every frame, so a wider terminal
    /// gets it back.
    pub files_width: u16,
    /// The left column's width as last drawn: where its border is.
    pub column_w: u16,
    /// In-progress drag of the files/diff border: `boundary_x - grab column`
    /// at mouse-down, so the border tracks the pointer instead of jumping
    /// to it.
    pub files_drag: Option<i32>,
    /// Whether the repo has a commit; picks the diff command.
    pub head_ok: bool,
    /// Reviewed ✓ marks: file path → fingerprint of the approved diff text.
    /// Orion-side bookkeeping only (persisted via `review::store_marks`);
    /// never stages or otherwise touches git state.
    pub reviewed: HashMap<String, u64>,
    /// HEAD OID the marks are scoped to (empty on an unborn HEAD). A moved
    /// HEAD — commit, checkout — resets the worktree's marks on next open.
    pub head_key: String,
    /// BACKGROUND READS: with it, the file list and every file's diff are
    /// read off the loop (`git_diff::load_selected_diff`); without (a view
    /// built by a test), inline.
    pub jobs: Option<crate::view_jobs::Jobs>,
    /// This view's own ticket, carried by every diff it asks for, so text
    /// read for an earlier modal never lands in this one's cache.
    pub id: u64,
    /// The `git status` this view opened ahead of, by ticket: the list is
    /// empty and says `reading changes…` until the listing lands.
    pub listing: Option<u64>,
    /// The selected file's diff in flight, by ticket. `diff` keeps the text
    /// it had meanwhile (`view_jobs::STALE_GRACE`) — see `shown`.
    pub waiting: Option<u64>,
    /// The file `diff` is the diff of. Differs from the selected file while
    /// that one's read is in flight, which is when a reviewed ✓ — a
    /// fingerprint of the text on screen — must not be taken.
    pub shown: Option<String>,
    /// Diffs read while this modal has been open, newest last: walking
    /// back onto a file shows it on the keypress (and re-reads it behind,
    /// so an agent's edit meanwhile still shows up), and the row after the
    /// cursor is read ahead. Bounded by [`DIFF_CACHE_BYTES`]; gone with
    /// the modal.
    pub cache: Vec<(String, std::sync::Arc<str>)>,
    /// The file list folded into a directory tree (`Ctrl+t`, see
    /// `diff_tree`); `None` is the flat list. While it is up the cursor is
    /// the tree's — `selected` and `matches` stay current underneath, so
    /// toggling back lands on a list that is already right.
    pub tree: Option<crate::diff_tree::DiffTree>,
    /// The COMMIT LIST over the files (`commit_list`): the branch's own
    /// commits since its base, under **Uncommitted changes**, each with a
    /// box to tick. `None` for a pull request's view, which has no local
    /// branch to list.
    pub commits: Option<Box<crate::commit_list::CommitList>>,
    /// What the file list and every diff are of: what the COMMIT LIST put
    /// on screen when its file list last landed (`commit_list::land_scope`).
    pub scope: crate::git_diff::DiffScope,
    /// The panel with the keys.
    pub focus: DiffFocus,
    /// The REVIEW HEAD for each file's diff: a commit's message, what was
    /// ticked, a pull request's title; empty over the uncommitted changes.
    /// A view with no COMMIT LIST — a pull request's — names its pane after
    /// the head's title (`ui::diff_view`).
    pub head: Vec<crate::diff_doc::Head>,
    /// ✓ marks taken under a row that is not on screen — a commit's, the
    /// branch's — kept for as long as the modal is up and brought back
    /// with their row. The uncommitted changes' are on disk instead.
    pub scope_marks: HashMap<crate::git_diff::DiffScope, HashMap<String, u64>>,
    /// The PULL REQUESTS MODAL this viewer was opened from, as it was:
    /// the viewer is a level inside it, and Esc or a click outside puts
    /// it back on the same row and tab rather than closing onto the grid.
    pub back: Option<Box<crate::pr_modal::PullRequestsView>>,
    /// A file to put the cursor on once the next scope's files land — a
    /// pull request's file, asked for before its commits were read.
    pub want_path: Option<String>,
    /// A pull request read from this repo's git (`event_loop::
    /// open_pr_review`): where it was asked to open, and which pull request
    /// the title names (`#42 ›`).
    pub pr_review: Option<crate::event_loop::PrReview>,
    /// The files its branch conflicts in with its base, worked out locally
    /// (`diff_actions::ask`): the `U` marks and the top line's count.
    pub conflicts: crate::diff_actions::Conflicts,
    /// A pull request's review, written here (`pr_review::viewer`): its
    /// threads under their lines while the whole pull request is on
    /// screen, and the bar along the bottom that sends it. None over a
    /// checkout.
    pub review: Option<Box<crate::pr_review::ReviewScreen>>,
}

/// The most diff text a DIFF VIEWER keeps beyond the one on screen. Two
/// megabytes is a few hundred ordinary files' worth, and an entry over
/// [`DIFF_CACHE_ENTRY_MAX`] is never kept: re-reading one huge diff is
/// cheaper than holding it.
pub const DIFF_CACHE_BYTES: usize = 2 * 1024 * 1024;
pub const DIFF_CACHE_ENTRY_MAX: usize = 512 * 1024;

impl DiffView {
    /// A view up before its file list is: `g` opens this at once and
    /// `event_loop::land_view_answer` fills it when `git status` answers.
    /// Without BACKGROUND READS (a view a test builds) the listing is read
    /// inline and handed to the same landing.
    pub fn opening(
        root: PathBuf,
        branch: String,
        jobs: Option<crate::view_jobs::Jobs>,
        listing: u64,
    ) -> Self {
        let mut view = Self::new(root, branch, Vec::new(), true);
        view.jobs = jobs;
        view.listing = Some(listing);
        view.commits = Some(Box::new(crate::commit_list::CommitList::reading()));
        view
    }

    /// The cached diff of `path`, if this modal has read it.
    pub fn cached(&self, path: &str) -> Option<std::sync::Arc<str>> {
        self.cache
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, text)| text.clone())
    }

    /// Keep `diff` as the diff of `path`, dropping the oldest entries to
    /// stay inside the budget.
    pub fn cache_put(&mut self, path: &str, diff: &str) {
        self.cache.retain(|(p, _)| p != path);
        if diff.len() > DIFF_CACHE_ENTRY_MAX {
            return;
        }
        self.cache.push((path.to_string(), diff.into()));
        let mut held: usize = self.cache.iter().map(|(_, text)| text.len()).sum();
        while held > DIFF_CACHE_BYTES && self.cache.len() > 1 {
            held -= self.cache.remove(0).1.len();
        }
    }

    /// Put `diff` on screen as the diff of `path`, read for the pane
    /// (`diff_doc`). The REVIEW HEAD is not drawn over it: what the pane
    /// reads is named on its border (`ui::diff_view`), so the code starts on
    /// its first row. `keep_scroll` is a re-read of the file already
    /// showing: the reader's place is kept.
    pub fn show_diff(&mut self, path: Option<&str>, diff: String, keep_scroll: bool) {
        self.doc = std::sync::Arc::new(crate::diff_doc::DiffDoc::build(&[], path, &diff));
        self.diff = diff;
        self.shown = path.map(str::to_string);
        if !keep_scroll {
            self.scroll = 0;
        }
    }

    /// A new REVIEW HEAD for the diff on screen — a tick changed what it
    /// says, not which file is up.
    pub fn set_head(&mut self, head: Vec<crate::diff_doc::Head>) {
        self.head = head;
    }

    /// What the diff pane says for a scope with no file in it.
    pub fn empty_note(&self) -> &'static str {
        match self.scope {
            crate::git_diff::DiffScope::Uncommitted => "(no uncommitted changes)",
            crate::git_diff::DiffScope::Commit { .. } => "(an empty commit: it changes no file)",
            crate::git_diff::DiffScope::Ranges(_) => "(together, these commits change no file)",
        }
    }

    /// Where the left column and the diff sit: the whole modal, as last
    /// drawn — what the column/diff border's grab zone spans.
    pub fn panes_area(&self) -> Rect {
        self.area
    }

    pub fn new(root: PathBuf, branch: String, files: Vec<DiffFile>, head_ok: bool) -> Self {
        let mut view = Self {
            root,
            branch,
            files,
            filter: TextInput::new(),
            matches: Vec::new(),
            selected: 0,
            diff: String::new(),
            doc: Default::default(),
            scroll: 0,
            view_height: 0,
            view_width: 0,
            list_area: Rect::default(),
            files_scroll: ListScroll::default(),
            diff_area: Rect::default(),
            area: Rect::default(),
            files_width: DEFAULT_DIFF_FILES_W,
            column_w: 0,
            files_drag: None,
            head_ok,
            reviewed: HashMap::new(),
            head_key: String::new(),
            jobs: None,
            id: crate::view_jobs::ticket(),
            listing: None,
            waiting: None,
            shown: None,
            cache: Vec::new(),
            tree: None,
            commits: None,
            scope: crate::git_diff::DiffScope::Uncommitted,
            focus: DiffFocus::Files,
            head: Vec::new(),
            scope_marks: HashMap::new(),
            back: None,
            want_path: None,
            pr_review: None,
            conflicts: Default::default(),
            review: None,
        };
        view.apply_filter();
        view
    }

    /// The furthest the diff pane scrolls: its last row on its bottom row,
    /// counted at the width it was last drawn at (a row a line before it).
    pub fn max_scroll(&self) -> usize {
        match self.view_width {
            0 => self
                .doc
                .lines
                .len()
                .saturating_sub(usize::from(self.view_height.max(1))),
            width => self.doc.max_scroll(width, self.view_height),
        }
    }

    /// Screen x of the files/diff boundary — the column where the diff panel
    /// starts.
    pub fn splitter_x(&self) -> u16 {
        self.area.x + self.column_w
    }

    /// Move the files/diff boundary to `boundary_x`, clamped so the file list
    /// keeps `MIN_DIFF_FILES_W` and the diff pane keeps `MIN_DIFF_PANE_W`.
    pub fn set_files_width(&mut self, boundary_x: i32) {
        if let Some(width) = clamp_files_width(self.area, boundary_x) {
            self.files_width = width;
        }
    }

    /// Clamped relative scroll of the diff pane, in rows.
    pub fn scroll_by(&mut self, delta: i64) {
        self.scroll = (self.scroll as i64 + delta).clamp(0, self.max_scroll() as i64) as usize;
    }

    /// The panel `Tab` (`forward`) or `⇧Tab` hands the keys to: reading
    /// order, round again — the COMMIT LIST only where there is one.
    pub fn next_focus(&self, forward: bool) -> DiffFocus {
        let mut order = Vec::with_capacity(4);
        if self.commits.is_some() {
            order.push(DiffFocus::Commits);
        }
        order.extend([DiffFocus::Files, DiffFocus::Diff]);
        // A pull request's review bar, last.
        if self.review.is_some() {
            order.push(DiffFocus::Bar);
        }
        crate::nav::cycle(&order, self.focus, forward)
    }

    /// The line of the file at the top of the diff pane — the first line of
    /// code from there down, by its number in the file as it is now: where
    /// `⌘O` opens the file. The first line when there is none.
    pub fn line_on_screen(&self) -> u64 {
        let first = match self.view_width {
            0 => 0,
            width => self.doc.locate(self.scroll, width).0,
        };
        self.doc.lines[first.min(self.doc.lines.len())..]
            .iter()
            .find_map(|line| match line {
                crate::diff_doc::DocLine::Code { new: Some(n), .. } => Some(u64::from(*n)),
                _ => None,
            })
            .unwrap_or(1)
    }

    /// The file under the cursor and the line at the top of its diff
    /// ([`Self::line_on_screen`]): where `⌘O` and `Enter` on the diff open
    /// it. None on a directory's row, and for a file the change deletes.
    pub fn file_at_line(&self) -> Option<(String, u64)> {
        let file = self.selected_file().filter(|f| !f.is_deleted())?;
        Some((file.path.clone(), self.line_on_screen()))
    }

    /// A page of the diff pane: its height, less a row of overlap.
    pub fn page(&self) -> i64 {
        i64::from(self.view_height.max(2) - 1)
    }

    /// Clamped absolute selection in the list on screen — the filtered
    /// files, or the tree's rows; true when it changed (the caller reloads
    /// the diff).
    pub fn select(&mut self, index: i64) -> bool {
        if let Some(tree) = &mut self.tree {
            return tree.select(index);
        }
        let clamped = clamp_selection(index, self.matches.len());
        let changed = clamped != self.selected;
        self.selected = clamped;
        changed
    }

    /// The cursor's row in the list on screen: an index into `matches`, or
    /// into the tree's rows.
    pub fn cursor(&self) -> usize {
        self.tree.as_ref().map_or(self.selected, |t| t.selected)
    }

    /// How many rows the list on screen has.
    pub fn row_count(&self) -> usize {
        self.tree
            .as_ref()
            .map_or(self.matches.len(), |t| t.rows.len())
    }

    /// The file behind the current selection, if any row is visible — and,
    /// in the tree, if that row is a file's.
    pub fn selected_file(&self) -> Option<&DiffFile> {
        match &self.tree {
            Some(tree) => self.files.get(tree.selected_file()?),
            None => self.files.get(self.matches.get(self.selected)?.file),
        }
    }

    /// The directory the tree's cursor rests on, as its path.
    pub fn selected_dir(&self) -> Option<&str> {
        let node = self.tree.as_ref()?.selected_node()?;
        node.is_dir.then_some(node.path.as_str())
    }

    /// The selected row's path: the file's, or a tree directory's.
    pub fn selected_path(&self) -> Option<&str> {
        self.selected_file()
            .map(|f| f.path.as_str())
            .or_else(|| self.selected_dir())
    }

    /// Put the cursor on the row for `path`, in whichever list is showing.
    /// False, and the cursor left alone, when no row has it.
    pub fn select_path(&mut self, path: &str) -> bool {
        if let Some(tree) = &mut self.tree {
            return tree.select_path(path, &self.filter);
        }
        let row = self
            .matches
            .iter()
            .position(|m| self.files[m.file].path == path);
        if let Some(row) = row {
            self.selected = row;
        }
        row.is_some()
    }

    /// First visible row of the file list, `height` rows tall: where it was
    /// last drawn, following the cursor (`ListScroll`).
    pub fn window_start(&self, height: usize) -> usize {
        self.files_scroll
            .start(self.cursor(), self.row_count(), height)
    }

    /// Whether the cursor is still where opening the modal put it: the top
    /// of the flat list, the tree's home row. A reader who has moved keeps
    /// the file they are on when a fresh listing lands (`git_diff::
    /// fill_view`); one who has not gets what a fresh open gives.
    pub fn at_home(&self) -> bool {
        match &self.tree {
            Some(tree) => tree.selected == tree.home_row(),
            None => self.selected == 0,
        }
    }

    /// The next file down from the cursor in whichever list is showing —
    /// what the DIFF VIEWER reads ahead, `↓` being the key it is walked
    /// with. Tree directories are stepped over: they have no diff to read.
    pub fn file_after_cursor(&self) -> Option<&DiffFile> {
        match &self.tree {
            Some(tree) => tree
                .rows
                .iter()
                .skip(tree.selected + 1)
                .find_map(|row| tree.file_of[row.node])
                .and_then(|file| self.files.get(file)),
            None => self.files.get(self.matches.get(self.selected + 1)?.file),
        }
    }

    /// Recompute the rows from `filter` and send the cursor home — the top
    /// row, or in the tree the best match (its first file when the filter
    /// is empty); true when that moved it off the row it was on (the caller
    /// reloads the diff).
    pub fn apply_filter(&mut self) -> bool {
        let before = self.matches.get(self.selected).map(|m| m.file);
        self.recompute_matches();
        self.selected = 0;
        match &mut self.tree {
            Some(tree) => tree.apply_filter(&self.filter),
            None => before != self.matches.first().map(|m| m.file),
        }
    }

    /// Swap in a new file list (a pull request's diff refreshed under the
    /// modal): both lists are rebuilt — the tree keeping what the reader
    /// folded — and the cursor goes home; the caller moves it back
    /// (`select_path`) and reloads the diff.
    pub fn replace_files(&mut self, files: Vec<DiffFile>) {
        self.files = files;
        self.recompute_matches();
        self.selected = 0;
        if let Some(tree) = &self.tree {
            self.tree = Some(tree.rebuilt(&self.files, &self.filter));
        }
    }

    /// `Ctrl+t`: the other shape of the file list — flat paths, or the
    /// directory tree (`diff_tree`). The cursor stays on the file it was on;
    /// leaving the tree from a directory row, that directory's first file
    /// stands in, since the flat list has no row for a directory. True when
    /// the diff pane has something else to show (the caller reloads it).
    pub fn toggle_tree(&mut self) -> bool {
        let before = self.selected_file().map(|f| f.path.clone());
        let target = match self.tree.take() {
            Some(tree) => before.clone().or_else(|| {
                let under = tree.files_under(tree.rows.get(tree.selected)?.node);
                Some(self.files[*under.first()?].path.clone())
            }),
            None => {
                self.tree = Some(crate::diff_tree::DiffTree::new(&self.files, &self.filter));
                before.clone()
            }
        };
        if let Some(path) = target {
            self.select_path(&path);
        }
        before.is_none() || before != self.selected_file().map(|f| f.path.clone())
    }

    /// `Enter` on a tree directory's row, or a click on it: fold or unfold
    /// it. True when that moved the cursor onto another row.
    pub fn toggle_dir(&mut self, row: usize) -> bool {
        match &mut self.tree {
            Some(tree) => tree.toggle_row(row, &self.filter),
            None => false,
        }
    }

    /// `→` in the tree: open the directory, or step into an open one. True
    /// when the cursor moved onto another row.
    pub fn expand_selected(&mut self) -> bool {
        match &mut self.tree {
            Some(tree) => tree.expand_selected(&self.filter),
            None => false,
        }
    }

    /// `←` in the tree: fold the directory, or jump to the parent's row.
    /// True when the cursor moved onto another row.
    pub fn collapse_selected(&mut self) -> bool {
        match &mut self.tree {
            Some(tree) => tree.collapse_selected(&self.filter),
            None => false,
        }
    }

    /// What the diff pane shows for a tree directory's row: the files that
    /// changed under it, each with its status code and its ✓.
    pub fn dir_summary(&self) -> Option<String> {
        let tree = self.tree.as_ref()?;
        let row = tree.rows.get(tree.selected)?;
        let dir = &tree.nodes[row.node];
        if !dir.is_dir {
            return None;
        }
        let under = tree.files_under(row.node);
        let plural = if under.len() == 1 { "" } else { "s" };
        let mut out = format!("{}/ — {} changed file{plural}\n", dir.path, under.len());
        for file in under.into_iter().map(|f| &self.files[f]) {
            let name = file
                .path
                .strip_prefix(dir.path.as_str())
                .map_or(file.path.as_str(), |rest| rest.trim_start_matches('/'));
            let mark = if self.reviewed.contains_key(&file.path) {
                "✓"
            } else {
                " "
            };
            out.push_str(&format!("\n{} {mark} {name}", file.status_str()));
        }
        Some(out)
    }

    /// Rebuild the visible rows from `filter` and the reviewed marks: best
    /// matches first (git order when the filter is empty), reviewed ✓ files
    /// stably sunk to the bottom. The selection index is left alone —
    /// callers reset or fix it up.
    pub fn recompute_matches(&mut self) {
        self.matches = crate::fuzzy::rank(&self.filter, self.files.iter().map(|f| f.path.as_str()))
            .into_iter()
            .map(|(file, positions)| DiffMatch { file, positions })
            .collect();
        let files = &self.files;
        let reviewed = &self.reviewed;
        self.matches
            .sort_by_key(|m| reviewed.contains_key(&files[m.file].path));
    }

    /// Toggle the reviewed ✓ on the selected file and re-sink reviewed
    /// files. Marking keeps the selection row — with the marked file sunk,
    /// that lands on the next file in the list; unmarking advances to the
    /// next still-reviewed file so repeated presses clear a batch of marks.
    /// Only when the last visible mark is cleared does the selection follow
    /// the file back to its natural spot. `None` when no row is selected,
    /// otherwise whether the selected file changed (the caller reloads the
    /// diff; it persists `reviewed` either way).
    ///
    /// The tree keeps its order — nothing sinks there — so the same sweep
    /// moves the cursor instead: down to the next file still in the state
    /// this one just left, staying put when there is none. A directory's
    /// row has no mark of its own to toggle.
    pub fn toggle_reviewed(&mut self) -> Option<bool> {
        let path = self.selected_file()?.path.clone();
        // The mark is a fingerprint of the diff that was read — not of
        // whatever the pane still holds while this file's is in flight.
        if self.waiting.is_some() && self.shown.as_deref() != Some(path.as_str()) {
            return None;
        }
        let before = self.matches.get(self.selected).map(|m| m.file);
        let unmarked = self.reviewed.remove(&path).is_some();
        if !unmarked {
            let mark = crate::review::fingerprint(&self.diff);
            self.reviewed.insert(path.clone(), mark);
        }
        self.recompute_matches();
        if let Some(tree) = &mut self.tree {
            let (files, reviewed) = (&self.files, &self.reviewed);
            let next = (tree.selected + 1..tree.rows.len()).find(|&row| {
                tree.file_of[tree.rows[row].node]
                    .is_some_and(|f| reviewed.contains_key(&files[f].path) == unmarked)
            });
            tree.selected = next.unwrap_or(tree.selected);
            return Some(next.is_some());
        }
        let marks_visible = self
            .matches
            .iter()
            .any(|m| self.reviewed.contains_key(&self.files[m.file].path));
        if unmarked && marks_visible {
            // The reviewed zone is contiguous at the bottom and the selection
            // sat inside it, so one row down is the next still-marked file.
            self.selected = (self.selected + 1).min(self.matches.len().saturating_sub(1));
        } else if unmarked {
            if let Some(pos) = self
                .matches
                .iter()
                .position(|m| self.files[m.file].path == path)
            {
                self.selected = pos;
            }
        } else {
            self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        }
        Some(before != self.matches.get(self.selected).map(|m| m.file))
    }
}

// The `/` PALETTE lives in `palette.rs`; re-exported so the callers and
// tests that always reached it through `app::` keep working.
pub use crate::palette::{Palette, PaletteItem, PaletteMatch, PaletteTarget};

/// One visible row of the file finder: an index into `files` plus the char
/// positions of the path the query matched, for highlighting.
#[derive(Debug, Clone)]
pub struct FinderMatch {
    pub file: usize,
    pub positions: Vec<usize>,
}

/// Fuzzy file finder over every file of the selected worktree (`f`). A
/// query ending in a line — `app.rs:42` — matches on the path and opens
/// the file there. With nothing typed it leads with the checkout's changed
/// files, then the ones opened last.
#[derive(Debug, Clone)]
pub struct FileFinder {
    /// Checkout dir the listing was read from.
    pub root: PathBuf,
    /// Branch name for the modal title.
    pub branch: String,
    /// Editor command Enter launches (ORION_EDITOR, else fresh), captured
    /// at open time.
    pub editor: String,
    /// Paths relative to `root`, in git listing order.
    pub files: Vec<String>,
    /// Type-to-filter query over `files`; always live.
    pub query: TextInput,
    /// Visible rows: `files` narrowed by `query`, best matches first
    /// (listing order when the query is empty).
    pub matches: Vec<FinderMatch>,
    /// Index into `matches` (not `files`).
    pub selected: usize,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the result rows (query row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
    /// The `git ls-files` this finder opened ahead of, by ticket
    /// (`view_jobs`): the list is empty and says `listing files…` until
    /// [`FileFinder::set_files`]. None once the listing is in hand — and
    /// always, for a finder built with its files.
    pub listing: Option<u64>,
    /// The checkout's RECENT FILES, newest first (`crate::recent_files`).
    pub recents: Vec<String>,
    /// Up to [`FINDER_RECENTS`] of `recents` still in `files`, by index,
    /// newest first: found once per listing, not on every keystroke.
    recent_files: Vec<usize>,
    /// How many of `matches` lead as the `Recent` section: `recent_files`
    /// while the query is empty, none once anything is typed.
    pub recent: usize,
    /// The checkout's changed files as `git status` lists them, root
    /// relative (`with_changed`).
    pub changed: Vec<String>,
    /// Up to [`FINDER_CHANGED`] of `changed` still in `files`, by index, in
    /// status order.
    changed_at: Vec<usize>,
    /// How many of `matches` lead as the `Changed` section, above `Recent`:
    /// `changed_at` while the query is empty, none once anything is typed.
    pub changed_lead: usize,
    /// Line Enter opens the file at: 1, or the line of a clicked path
    /// whose candidates this finder lists (`with_line`).
    pub line: u64,
}

/// One drawn row of the FILE FINDER's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinderRow {
    /// A section's title: `Recent`, or `Files` over the rest.
    Header(&'static str),
    /// `matches[i]`.
    Match(usize),
}

/// How many recent files the FILE FINDER leads with.
pub const FINDER_RECENTS: usize = 3;
/// How many changed files the FILE FINDER leads with, above the recent
/// ones: a branch that touched hundreds would bury the rest.
pub const FINDER_CHANGED: usize = 10;

impl FileFinder {
    pub fn new(root: PathBuf, branch: String, editor: String, files: Vec<String>) -> Self {
        let mut finder = Self {
            root,
            branch,
            editor,
            files,
            query: TextInput::new(),
            matches: Vec::new(),
            selected: 0,
            area: Rect::default(),
            list_area: Rect::default(),
            listing: None,
            recents: Vec::new(),
            recent_files: Vec::new(),
            recent: 0,
            changed: Vec::new(),
            changed_at: Vec::new(),
            changed_lead: 0,
            line: 1,
        };
        finder.apply_filter();
        finder
    }

    /// The query as a path and the line it ends with, if any: `app.rs:42`
    /// is `app.rs` at 42.
    pub fn typed(&self) -> (&str, Option<u64>) {
        crate::links::split_line_suffix(self.query.as_str())
    }

    /// The line Enter opens the chosen file at: the query's own, else
    /// [`FileFinder::line`].
    pub fn open_line(&self) -> u64 {
        self.typed().1.unwrap_or(self.line)
    }

    /// Open the chosen file at `line` rather than its first.
    pub fn with_line(mut self, line: u64) -> Self {
        self.line = line;
        self
    }

    /// Lead with `recents` — the checkout's RECENT FILES, newest first —
    /// while the query is empty.
    pub fn with_recents(mut self, recents: Vec<String>) -> Self {
        self.recents = recents;
        self.find_lead_files();
        self.apply_filter();
        self
    }

    /// Lead with `changed` — the checkout's changed files, in `git status`
    /// order — above the recent ones while the query is empty.
    pub fn with_changed(mut self, changed: Vec<String>) -> Self {
        self.changed = changed;
        self.find_lead_files();
        self.apply_filter();
        self
    }

    /// Where `changed` and `recents` sit in `files`. A recent file that is
    /// also changed leads once, as changed.
    fn find_lead_files(&mut self) {
        self.changed_at = self.positions_of(&self.changed, FINDER_CHANGED, &[]);
        self.recent_files = self.positions_of(&self.recents, FINDER_RECENTS, &self.changed_at);
    }

    /// Up to `cap` of `wanted` in `files`, by index, in `wanted`'s order,
    /// leaving out the indices in `taken`: one pass over the listing.
    fn positions_of(&self, wanted: &[String], cap: usize, taken: &[usize]) -> Vec<usize> {
        let rank: HashMap<&str, usize> = wanted
            .iter()
            .enumerate()
            .map(|(i, r)| (r.as_str(), i))
            .collect();
        let mut found: Vec<(usize, usize)> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(file, f)| rank.get(f.as_str()).map(|&r| (r, file)))
            .filter(|(_, file)| !taken.contains(file))
            .collect();
        found.sort_unstable();
        found.into_iter().take(cap).map(|(_, file)| file).collect()
    }

    /// A finder up before its listing is: `f` opens this at once, and
    /// `set_files` fills it when `git ls-files` answers. What is typed
    /// meanwhile is kept, and narrows the list the moment there is one.
    pub fn opening(root: PathBuf, branch: String, editor: String, listing: u64) -> Self {
        let mut finder = Self::new(root, branch, editor, Vec::new());
        finder.listing = Some(listing);
        finder
    }

    /// The listing landed: rank it by whatever the query holds by now.
    pub fn set_files(&mut self, files: Vec<String>) {
        self.files = files;
        self.listing = None;
        self.find_lead_files();
        self.apply_filter();
    }

    /// First visible row of the result list's stateless follow-window for a
    /// list of `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.display_row(self.selected), height)
    }

    /// The sections leading `matches` while the query is empty, in order,
    /// each with how many matches it holds: `Changed`, then `Recent` — an
    /// empty one left out.
    fn lead(&self) -> impl Iterator<Item = (&'static str, usize)> {
        [("Changed", self.changed_lead), ("Recent", self.recent)]
            .into_iter()
            .filter(|(_, n)| *n > 0)
    }

    /// Whether the `Files` header shows: there is a leading section and
    /// something listed after it.
    fn files_header(&self) -> bool {
        let led = self.changed_lead + self.recent;
        led > 0 && self.matches.len() > led
    }

    /// The list's rows as drawn: the matches, a header over each leading
    /// section, and — after them — one over the rest.
    pub fn row_count(&self) -> usize {
        self.matches.len() + self.lead().count() + usize::from(self.files_header())
    }

    /// The drawn row of `matches[index]`: below the header of its own
    /// section and of every one above it.
    pub fn display_row(&self, index: usize) -> usize {
        let mut headers = 0;
        let mut start = 0;
        for (_, n) in self.lead() {
            if index >= start {
                headers += 1;
            }
            start += n;
        }
        if self.files_header() && index >= start {
            headers += 1;
        }
        index + headers
    }

    /// What drawn row `row` is; None past the end.
    pub fn row(&self, row: usize) -> Option<FinderRow> {
        if row >= self.row_count() {
            return None;
        }
        let (mut r, mut first) = (row, 0);
        for (title, n) in self.lead() {
            if r == 0 {
                return Some(FinderRow::Header(title));
            }
            if r <= n {
                return Some(FinderRow::Match(first + r - 1));
            }
            r -= n + 1;
            first += n;
        }
        if self.files_header() {
            if r == 0 {
                return Some(FinderRow::Header("Files"));
            }
            r -= 1;
        }
        Some(FinderRow::Match(first + r))
    }

    /// Clamped absolute selection in the filtered list.
    pub fn select(&mut self, index: i64) {
        self.selected = clamp_selection(index, self.matches.len());
    }

    /// The path behind the current selection, if any row is visible.
    pub fn selected_path(&self) -> Option<&str> {
        self.files
            .get(self.matches.get(self.selected)?.file)
            .map(String::as_str)
    }

    /// Recompute `matches` from `query` and reset the selection to the top
    /// row. Best matches first, listing order when the query is empty; a
    /// line at the query's end (`:42`) is no part of the path it matches.
    /// With the query empty the changed files still listed lead, then the
    /// RECENT FILES, newest first, and the rest follow without them — so
    /// ⌘P then Enter opens the first changed file, or reopens the last one
    /// when nothing is changed.
    pub fn apply_filter(&mut self) {
        let (query, _) = self.typed();
        let mut matches: Vec<FinderMatch> =
            crate::fuzzy::rank(query, self.files.iter().map(String::as_str))
                .into_iter()
                .map(|(file, positions)| FinderMatch { file, positions })
                .collect();
        let (changed, recent): (&[usize], &[usize]) = if query.trim().is_empty() {
            (&self.changed_at, &self.recent_files)
        } else {
            (&[], &[])
        };
        if !changed.is_empty() || !recent.is_empty() {
            matches.retain(|m| !changed.contains(&m.file) && !recent.contains(&m.file));
            let lead = changed.iter().chain(recent).map(|&file| FinderMatch {
                file,
                positions: Vec::new(),
            });
            matches.splice(0..0, lead);
        }
        self.changed_lead = changed.len();
        self.recent = recent.len();
        self.matches = matches;
        self.selected = 0;
    }
}

/// Find-in-files overlay (`F`): live `git grep` over the selected worktree,
/// the hits under a header per file, the selected one's file previewed
/// beside them with its matches lit, and `⌘R` reading the query as a regex.
#[derive(Debug, Clone)]
pub struct GrepView {
    /// Checkout dir the search runs in.
    pub root: PathBuf,
    /// Branch name for the modal title.
    pub branch: String,
    /// Editor command Enter launches (ORION_EDITOR, else fresh), captured
    /// at open time.
    pub editor: String,
    /// The search text; every edit re-runs the grep.
    pub query: TextInput,
    /// Current results, best-first in git grep order (path, then line).
    pub hits: Vec<crate::grep_search::GrepHit>,
    /// The search stopped at the result cap — the title says so.
    pub truncated: bool,
    /// A failed grep's message, shown in the list area until the next edit.
    pub error: Option<String>,
    /// Index into `hits`.
    pub selected: usize,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the result rows (query row excluded), written back
    /// during draw so clicks can hit-test rows.
    pub list_area: Rect,
    /// BACKGROUND READS: with it, a search runs off the loop and lands in
    /// [`GrepView::land`]; without (a view built by a test), inline.
    pub jobs: Option<crate::view_jobs::Jobs>,
    /// The search in flight, by ticket. The hits on screen meanwhile are the
    /// previous query's — the title says `searching…`.
    pub waiting: Option<u64>,
    /// Stops the search in flight when the query moves on.
    pub cancel: crate::view_jobs::Cancel,
    /// How the query is read: the text itself, or a regex (`⌘R`).
    pub mode: crate::grep_search::Mode,
    /// The selected hit's file as the preview pane shows it, with its
    /// matches; None until one is read.
    pub preview: Option<std::sync::Arc<crate::grep_search::GrepPreview>>,
    /// The preview read in flight, by ticket.
    pub preview_waiting: Option<u64>,
    /// Stops the preview read in flight when the cursor moves on.
    pub preview_cancel: crate::view_jobs::Cancel,
}

impl GrepView {
    pub fn new(root: PathBuf, branch: String, editor: String) -> Self {
        Self {
            root,
            branch,
            editor,
            query: TextInput::new(),
            hits: Vec::new(),
            truncated: false,
            error: None,
            selected: 0,
            area: Rect::default(),
            list_area: Rect::default(),
            jobs: None,
            waiting: None,
            cancel: crate::view_jobs::Cancel::default(),
            mode: crate::grep_search::Mode::default(),
            preview: None,
            preview_waiting: None,
            preview_cancel: crate::view_jobs::Cancel::default(),
        }
    }

    /// `⌘R`: read the query the other way — as a regex, or as the text
    /// itself — and search again.
    pub fn toggle_regex(&mut self) {
        self.mode.regex = !self.mode.regex;
        self.run_search();
    }

    /// Re-run the grep for the current query and reset the selection to the
    /// top row. Queries under `MIN_QUERY_LEN` just clear the results.
    pub fn run_search(&mut self) {
        self.selected = 0;
        self.error = None;
        // Whatever was being searched for is no longer the query.
        self.cancel.cancel();
        self.waiting = None;
        if self.too_short() {
            self.hits.clear();
            self.truncated = false;
            self.load_preview();
            return;
        }
        let Some(jobs) = &self.jobs else {
            let result = crate::grep_search::search(&self.root, &self.query, self.mode);
            self.show(result);
            return;
        };
        let ticket = crate::view_jobs::ticket();
        self.waiting = Some(ticket);
        self.cancel = crate::view_jobs::Cancel::default();
        let (root, query, cancel, mode) = (
            self.root.clone(),
            self.query.to_string(),
            self.cancel.clone(),
            self.mode,
        );
        jobs.run(move || {
            // The next character, typed at speed, cancels this before git
            // is ever started.
            std::thread::sleep(crate::view_jobs::GREP_DEBOUNCE);
            if cancel.is_cancelled() {
                return None;
            }
            let result = crate::grep_search::search_streaming(&root, &query, mode, &cancel)?;
            Some(crate::view_jobs::Answer::Grep { ticket, result })
        });
    }

    /// Read the selected hit's file for the preview pane, with the query's
    /// matches in it — off the loop when there are BACKGROUND READS, the
    /// pane keeping what it showed until the read lands. The file already
    /// on show is not read again, only searched. With no hit the pane
    /// empties.
    pub fn load_preview(&mut self) {
        use crate::grep_search::read_preview;
        self.preview_cancel.cancel();
        self.preview_waiting = None;
        let Some(path) = self.selected_hit().map(|hit| hit.path.clone()) else {
            self.preview = None;
            return;
        };
        let (root, query, mode) = (self.root.clone(), self.query.to_string(), self.mode);
        let read = self
            .preview
            .as_ref()
            .filter(|shown| shown.path == path)
            .map(|shown| shown.file.clone());
        let Some(jobs) = &self.jobs else {
            self.preview = read_preview(&root, &path, &query, mode, read, None).map(Into::into);
            return;
        };
        let ticket = crate::view_jobs::ticket();
        self.preview_waiting = Some(ticket);
        self.preview_cancel = crate::view_jobs::Cancel::default();
        let cancel = self.preview_cancel.clone();
        jobs.run(move || {
            let preview = read_preview(&root, &path, &query, mode, read, Some(&cancel))?;
            Some(crate::view_jobs::Answer::GrepPreview {
                ticket,
                preview: preview.into(),
            })
        });
    }

    /// A background preview read came back: shown when it is the one being
    /// waited on, dropped when the cursor has moved on since.
    pub fn land_preview(
        &mut self,
        ticket: u64,
        preview: std::sync::Arc<crate::grep_search::GrepPreview>,
    ) {
        if self.preview_waiting == Some(ticket) {
            self.preview_waiting = None;
            self.preview = Some(preview);
        }
    }

    /// A background search's answer: shown when it is the one being waited
    /// on, dropped when the query has moved on since.
    pub fn land(
        &mut self,
        ticket: u64,
        result: Result<(Vec<crate::grep_search::GrepHit>, bool), String>,
    ) {
        if self.waiting != Some(ticket) {
            return;
        }
        self.waiting = None;
        self.show(result);
    }

    fn show(&mut self, result: Result<(Vec<crate::grep_search::GrepHit>, bool), String>) {
        self.selected = 0;
        match result {
            Ok((hits, truncated)) => {
                self.hits = hits;
                self.truncated = truncated;
            }
            Err(msg) => {
                self.hits.clear();
                self.truncated = false;
                self.error = Some(msg);
            }
        }
        self.load_preview();
    }

    /// The list as drawn: a header over each file's run of hits — git
    /// lists them a file at a time — then the hits.
    pub(crate) fn rows(&self) -> Vec<crate::ui::ListEntry> {
        let paths: Vec<&str> = self.hits.iter().map(|hit| hit.path.as_str()).collect();
        crate::ui::sections(&paths, true)
    }

    /// The query is too short to search with ([`grep_search::MIN_QUERY_LEN`]).
    ///
    /// [`grep_search::MIN_QUERY_LEN`]: crate::grep_search::MIN_QUERY_LEN
    pub fn too_short(&self) -> bool {
        self.query.chars().count() < crate::grep_search::MIN_QUERY_LEN
    }

    /// The drawn row of `hits[index]`; the top for a hit there isn't.
    pub fn display_row(&self, index: usize) -> usize {
        self.rows()
            .iter()
            .position(|entry| *entry == crate::ui::ListEntry::Row(index))
            .unwrap_or(0)
    }

    /// First visible row of the result list's stateless follow-window for a
    /// list of `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.display_row(self.selected), height)
    }

    /// Clamped absolute selection; a hit in another file reads that file
    /// for the preview.
    pub fn select(&mut self, index: i64) {
        let before = self.selected_hit().map(|hit| hit.path.clone());
        self.selected = clamp_selection(index, self.hits.len());
        if self.selected_hit().map(|hit| &hit.path) != before.as_ref() {
            self.load_preview();
        }
    }

    /// The hit behind the current selection, if any row is visible.
    pub fn selected_hit(&self) -> Option<&crate::grep_search::GrepHit> {
        self.hits.get(self.selected)
    }
}

/// Recent-hosts modal (`h`): destinations remembered by `orion ssh`.
/// Enter (or a click) quits the TUI and execs a fresh `orion ssh` at the
/// selected entry; `a` types a new destination, `d` forgets one. The rows
/// are a snapshot loaded when the modal opens — nothing else writes the
/// list while the TUI is up.
#[derive(Debug, Clone)]
pub struct HostsView {
    pub hosts: Vec<crate::hosts::HostEntry>,
    /// Cursor into `hosts`.
    pub selected: usize,
    /// Active "connect to a new destination" input (`a`), if any — typed as
    /// `user@host [dir]`, Enter connects like a `orion ssh` invocation.
    pub input: Option<TextInput>,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the host rows, written back during draw so clicks can
    /// hit-test rows.
    pub list_area: Rect,
}

impl HostsView {
    pub fn new(hosts: Vec<crate::hosts::HostEntry>) -> Self {
        Self {
            hosts,
            selected: 0,
            input: None,
            area: Rect::default(),
            list_area: Rect::default(),
        }
    }

    /// First visible row of the list's stateless follow-window for a list of
    /// `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }
}

/// How loudly the line under the settings body is speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warn,
}

#[derive(Debug, Clone, Default)]
pub struct SettingsView {
    /// Index into [`crate::config::SETTINGS_TABS`].
    pub tab: usize,
    /// Cursor row *within the current tab*.
    pub selected: usize,
    /// The tab strip itself has the cursor: ←/→ walk the tabs, ↓ drops
    /// back into the list. Stepping up off the top row is what puts it
    /// here, so arrows can steer tabs without ever fighting the ←/→ that
    /// cycles a setting's value. An overlay opened for the first time
    /// starts here (see [`App::settings_on_tabs`]).
    pub on_tabs: bool,
    /// Set during draw for click hit-testing.
    pub area: Rect,
    /// Screen x-range of each tab label, written during draw so clicks on
    /// the strip land on the right tab.
    pub tab_hits: Vec<(u16, u16)>,
    /// Body rect and the first body row visible in it, written during draw
    /// so click hit-testing agrees with what's actually on screen.
    pub body_area: Rect,
    pub first_row: usize,
    /// Transient line under the body: a failed write, a note on the row.
    pub notice: Option<(String, NoticeLevel)>,
    /// The install `i` asked about on a row whose program isn't on PATH
    /// (`install`): Enter runs it in the editor modal, anything else
    /// leaves it.
    pub install: Option<crate::install::Plan>,
}

impl SettingsView {
    /// `tab`/`selected`/`on_tabs` are the remembered cursor position
    /// (`App::settings_tab` / `App::settings_selected` /
    /// `App::settings_on_tabs`), the tab clamped in case the list shrank
    /// between builds — the row is [`App::settings_row`]'s to clamp, it
    /// knowing how many projects the Project tab lists.
    pub fn new(tab: usize, selected: usize, on_tabs: bool) -> Self {
        let tab = tab.min(crate::config::tab_count().saturating_sub(1));
        Self {
            tab,
            selected,
            on_tabs,
            ..Self::default()
        }
    }

    pub fn is_hotkeys(&self) -> bool {
        self.tab == crate::config::keys_tab()
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), NoticeLevel::Warn));
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), NoticeLevel::Info));
    }
}

#[derive(Debug, Clone)]
pub enum Overlay {
    Menu(ContextMenu),
    /// First-run setup: Claude and its accounts, the first project.
    Onboard(crate::onboard::OnboardView),
    /// The releases since the last one this machine ran, after an upgrade.
    WhatsNew(crate::whats_new::WhatsNewView),
    Confirm(ConfirmDialog),
    Prompt(PromptDialog),
    Help(HelpView),
    Settings(SettingsView),
    Diff(DiffView),
    Palette(Palette),
    Files(FileFinder),
    Grep(GrepView),
    Tree(crate::tree_browser::TreeBrowser),
    /// `orion open <file>…` from a session: the FILE TABS.
    FileTabs(crate::file_tabs::FileTabsView),
    /// `⇧U`: ACCOUNT USAGE — how much is left on each linked account.
    Usage(crate::usage::UsageView),
    /// **Clean unused worktrees** (`crate::clean_worktrees`).
    CleanWorktrees(crate::clean_worktrees::CleanView),
    /// The OPEN REPO modal (`crate::open_repo`).
    OpenRepo(crate::open_repo::OpenRepoView),
    /// CLONE FROM GITHUB (`crate::clone_repo`).
    CloneRepo(crate::clone_repo::CloneView),
    Hosts(HostsView),
    /// The AGENT PRESETS PICKER: `⌘U` in the box or over a pane, the
    /// palette's **Agent presets**, a pull request's.
    AgentPresets(crate::preset_overlays::AgentPresetsView),
    /// The PRESET EDITOR form, over Settings → Presets.
    AgentPresetEditor(crate::preset_overlays::AgentPresetEditor),
    /// `v`: the PULL REQUESTS MODAL — the project's open pull requests.
    PullRequests(crate::pr_modal::PullRequestsView),
    /// `⌘L`: the LINEAR VIEW — the open Linear issues assigned to the
    /// user, picked to fix together or to attach a pull request to.
    Linear(crate::linear::LinearView),
    /// `^P` in the LAUNCHER VIEW's box: the PROJECT PICKER.
    ProjectPicker(crate::launcher::ProjectPicker),
    /// The AUTOFIX MODAL: one of your pull requests broke (or `⌘G` in the
    /// PULL REQUESTS MODAL) — what to send the agent to fix.
    Autofix(Box<crate::automations::pr_fixer::AutofixForm>),
    /// `⌘J`: AUTOMATIONS — each one's on/off, preset, extra steer, and
    /// what it is running and has done (`crate::automations::view`).
    Automations(crate::automations::view::AutomationsView),
    /// `⌘I`: the TODOS MODAL — the selected project's own todo list.
    Todos(crate::todos::TodoView),
    /// `⌘⇧Y`: the WEEK IN REVIEW — what was finished in the last seven
    /// days, written up to read aloud (`crate::week_review`).
    WeekReview(Box<crate::week_review::WeekReviewView>),
    /// Enter on a conflicted file of a pull request: the CONFLICT
    /// RESOLVER, its merge's files to settle (`crate::conflicts`).
    Conflicts(Box<crate::conflicts::ConflictView>),
    /// A meeting's TAKEAWAYS SCREEN, from its Needs you card or its row
    /// on the Automations screen (`crate::takeaways_view`).
    Takeaways(Box<crate::takeaways_view::TakeawaysView>),
}

/// Rows optimistically removed for an in-flight DeleteWorktree, kept so an
/// Error reply can put them back exactly where they were.
#[derive(Debug, Clone)]
pub struct WorktreeRollback {
    /// Index the worktree held in `tree.worktrees`.
    pub index: usize,
    pub worktree: Worktree,
    /// Its agents, each with the index it held in `tree.agents`.
    pub agents: Vec<(usize, Agent)>,
}

/// The rows a launch into a WORKTREE that does not exist yet puts up the
/// moment Enter is pressed — a checkout row and its one session row,
/// under ids this client made up — so the panels never wait on the
/// DAEMON's `git worktree add` and CLI spawn. A QUICK PROMPT's ride the
/// PENDING INTENTs of its two creates, a PR SESSION's the one
/// `CreatePrAgent` that cuts the checkout and spawns in it: the Acks turn
/// them into the real rows, an Error takes them down
/// (`event_loop::placeholder`). The NEW WORKTREE modal puts up the
/// checkout row alone, under `PendingIntent::SelectCreatedWorktree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderRows {
    pub worktree: WorktreeId,
    pub agent: AgentId,
}

/// A `CreateAgent` (or `CreatePrAgent`) as a launch surface drafts it —
/// the NEW AGENT PICKER, the QUICK PROMPT, an AGENT PRESET's task box.
/// `event_loop::create_agent` turns it into the request
/// and the PENDING INTENT that attaches the row; a draft aimed at a
/// stand-in checkout waits on that checkout's own intent instead
/// (`PendingIntent::SelectCreatedWorktree::launch`).
///
/// An empty `name` takes the generated default (agent-1, …) and opts the
/// session into agent-driven auto-titling (`orion rename` on the first
/// prompt) — what every launch from the NEW AGENT PICKER and the QUICK
/// PROMPT does. A name a surface does set is the user's choice and stays.
#[derive(Debug, Clone)]
pub struct AgentLaunchDraft {
    pub worktree: WorktreeId,
    pub kind: AgentKind,
    /// Registry id when `kind` is [`AgentKind::Custom`].
    pub custom: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub name: String,
    /// An AGENT PRESET launch's composed first prompt (see
    /// `ClientRequest::CreateAgent::starting_prompt`).
    pub starting_prompt: Option<String>,
    /// The prompt (and its typed text) to bring back should the daemon
    /// refuse the create — so a rejected preset task is not lost.
    pub reopen_on_error: Option<(PromptKind, String)>,
    /// OPEN PRS launch context: this launch is a PR SESSION, scoped to that
    /// pull request and run in a checkout of its head branch.
    pub pr: Option<crate::pull_request::PrLaunch>,
    /// Enter and lock the TERMINAL PANE once the create is acked. True for
    /// every launch the user walked a picker to reach; never for the QUICK
    /// PROMPT.
    pub focus_pane: bool,
    /// The stand-in session row already on screen for this create (a
    /// QUICK PROMPT into a worktree that did not exist, a launch that
    /// waited on the NEW WORKTREE modal's checkout): the Ack turns it
    /// into the created row, an Error drops it.
    pub placeholder: Option<AgentId>,
    /// Land the cursors, the pane and FOCUS on the created session when
    /// its Ack arrives. False only for the second half of a two-request
    /// launch (a checkout cut first, then the session) whose first half
    /// the user navigated away from: the create is born in
    /// `App::left_behind`, so the manual move still outranks the follow.
    pub follow: bool,
    /// The mode the CLI starts in (see `ClientRequest::CreateAgent::mode`):
    /// edit for every launch but a QUICK PROMPT stepped to plan or ask.
    pub mode: orion_core::AgentMode,
    /// The TODO this launch was sent at: `create_agent` keeps it by the
    /// request (`App::todo_pending`) for the Ack to write the session onto.
    pub todo: Option<crate::todos::TodoRef>,
    /// The AUTOMATION starting this session: the tag the DAEMON keeps on
    /// the row (`Agent::automation`). Set through
    /// `crate::automations::launch`, which minds the running cap.
    pub automation: Option<crate::automations::Automation>,
}

impl AgentLaunchDraft {
    /// A launch of `kind` at `model` / `effort` into `worktree` with
    /// nothing else said: no name (the row titles itself), no first
    /// prompt, no pull request behind it, no box to bring back,
    /// no stand-in row, and the pane taken — what a launch the user walked
    /// a picker to reach looks like. Every launch surface starts from this
    /// and names only what is its own (`..AgentLaunchDraft::new(…)`), so a
    /// field added here has one default instead of one per surface that
    /// would otherwise have to remember it.
    pub fn new(
        worktree: WorktreeId,
        kind: AgentKind,
        model: Option<String>,
        effort: Option<String>,
    ) -> Self {
        Self {
            worktree,
            kind,
            custom: None,
            model,
            effort,
            name: String::new(),
            starting_prompt: None,
            reopen_on_error: None,
            pr: None,
            focus_pane: true,
            placeholder: None,
            follow: true,
            mode: orion_core::AgentMode::Edit,
            todo: None,
            automation: None,
        }
    }
}

/// What to do when an Ack (or Error) for this req_id arrives.
#[derive(Debug, Clone)]
pub enum PendingIntent {
    /// Attach the created session; `focus` also enters and locks the
    /// terminal pane. Off (the QUICK PROMPT's default) stops at selecting
    /// the row, so the pane previews the new session without taking the
    /// cursor off whatever panel the create was fired from.
    AttachCreated {
        focus: bool,
        /// The stand-in session row up for this create — a launch that
        /// waited on the NEW WORKTREE modal's checkout: the Ack turns it
        /// into the created row, an Error drops it.
        placeholder: Option<AgentId>,
    },
    /// Attach on success; on failure, reopen the box the task was typed
    /// into so a transient daemon/CLI error never makes the user retype it.
    AttachCreatedWithRetry {
        kind: PromptKind,
        task: String,
        focus: bool,
        /// The stand-in session row a QUICK PROMPT put up for this create:
        /// the Ack turns it into the created row, an Error drops it.
        placeholder: Option<AgentId>,
    },
    /// A follow-up `text` sent to agent `id` (`ClientRequest::SendTurn`).
    /// The Ack says it went — and, when the session was `booting`, that
    /// it was started with it; an Error puts `text` back in the box it came
    /// from, the LAUNCHER VIEW's modal when `modal`, else the card's.
    SendTurn {
        id: AgentId,
        text: String,
        booting: bool,
        modal: bool,
    },
    /// Select the added project and step into its Worktrees panel.
    SelectCreatedProject,
    /// CHECK OUT A PULL REQUEST: the checkout the DAEMON found or cut, and
    /// what it was for (`pr_checkout::landed`).
    CheckOutPr {
        then: crate::pr_checkout::Then,
    },
    /// The NEW WORKTREE modal's create. The stand-in row `placeholder`
    /// went up and took the cursor when Enter was pressed, exactly as the
    /// Ack used to leave the real row: the Ack only swaps the real id in.
    /// An Error takes the row down, puts the cursor and `focus` back where
    /// they were, and reopens `prompt` with `text` for a retry.
    SelectCreatedWorktree {
        placeholder: WorktreeId,
        focus: Focus,
        prompt: PromptKind,
        text: String,
        /// A launch fired into the stand-in while the DAEMON was still
        /// cutting the checkout (**Agent presets** on the new row, an AGENT PRESET
        /// run): its session row is up under the stand-in, and the Ack
        /// sends the create into the real checkout
        /// (`placeholder::defer_launch` / `replay_launch`). An Error
        /// takes that row down with the checkout's.
        launch: Option<Box<AgentLaunchDraft>>,
    },
    /// A QUICK PROMPT whose target was a worktree that did not exist yet:
    /// the Ack names the checkout the DAEMON cut, the cursor moves onto
    /// its row, and `launch` fires there with `text` as the task. On
    /// Error the box comes back with the text, like every other launch.
    LaunchInCreatedWorktree {
        launch: crate::quick_prompt::QuickLaunch,
        text: String,
        /// The stand-in rows on screen meanwhile.
        placeholder: PlaceholderRows,
    },
    /// A PR SESSION whose head branch had no checkout yet: the DAEMON
    /// fetches the branch, cuts the worktree and spawns the CLI in it
    /// behind this one request, and the Ack names only the session. The
    /// stand-in checkout row is adopted by the worktree's upsert as it
    /// lands (`placeholder::adopt_worktree`), the session row by the Ack,
    /// which then attaches it like `AttachCreated` — `focus` enters and
    /// locks the pane. An Error takes down whatever is still a stand-in
    /// and, like every other launch out of a box, brings `reopen`'s box
    /// back with what was typed in it — this create is the one most
    /// likely to be refused (a fetch offline, a fork since deleted, a
    /// branch git will not cut), and a review prompt is not retyped
    /// gladly. None for the `n` picker's name box, which has no task.
    AttachCreatedPrSession {
        focus: bool,
        placeholder: PlaceholderRows,
        reopen: Option<(PromptKind, String)>,
        /// The pull request the launch was fired from: where a cursor
        /// still on the refused stand-in goes back to. The row it left had
        /// no worktree, so `remember_context` recorded nothing to return
        /// to, and `restore_context` alone would drop the cursor on the
        /// ROOT WORKTREE — one `p` away from a session in the main
        /// checkout.
        pr_url: String,
    },
    /// Worktree removed optimistically; restore these rows on Error.
    DeleteWorktree(WorktreeRollback),
    /// A row renamed, archived, unarchived or deleted on the keypress
    /// (`event_loop::optimistic`): put it back on Error.
    Undo(Undo),
    /// `⌘⇧S`'s start, and **Restart stack**: the run comes up in the
    /// background. Its Ack moves nothing — no select, no attach, no pane —
    /// the upsert alone puts the run line in its band; an Error says why
    /// in the footer.
    RunStarted,
    None,
}

/// A row as it was before an OPTIMISTIC UPDATE changed it.
#[derive(Debug, Clone)]
pub enum Undo {
    /// Renamed or (un)archived in place: the row before, and the row the
    /// keypress showed. A refusal puts back only the fields the two differ
    /// in, and only those still showing the keypress's value.
    Restore {
        before: Box<orion_core::Entity>,
        shown: Box<orion_core::Entity>,
    },
    /// Deleted: the row, and the index it held in its `tree` list.
    Reinsert {
        index: usize,
        entity: Box<orion_core::Entity>,
    },
}

impl PendingIntent {
    /// Will this request's Ack move a cursor, the pane or FOCUS onto what
    /// it created? Those are the follows a manual move cancels
    /// (`App::left_behind`). The NEW WORKTREE modal's own Ack moves
    /// nothing — its select happened at Enter — until a launch is waiting
    /// on it.
    pub fn follows(&self) -> bool {
        match self {
            PendingIntent::AttachCreated { .. }
            | PendingIntent::AttachCreatedWithRetry { .. }
            | PendingIntent::AttachCreatedPrSession { .. }
            | PendingIntent::LaunchInCreatedWorktree { .. }
            | PendingIntent::SelectCreatedProject => true,
            PendingIntent::SelectCreatedWorktree { launch, .. } => launch.is_some(),
            _ => false,
        }
    }

    /// The stand-in worktree row this in-flight request is holding up.
    pub fn placeholder_worktree(&self) -> Option<&WorktreeId> {
        match self {
            PendingIntent::LaunchInCreatedWorktree { placeholder, .. }
            | PendingIntent::AttachCreatedPrSession { placeholder, .. } => {
                Some(&placeholder.worktree)
            }
            PendingIntent::SelectCreatedWorktree { placeholder, .. } => Some(placeholder),
            _ => None,
        }
    }

    /// The stand-in session row this in-flight request is holding up.
    pub fn placeholder_agent(&self) -> Option<&AgentId> {
        match self {
            PendingIntent::LaunchInCreatedWorktree { placeholder, .. }
            | PendingIntent::AttachCreatedPrSession { placeholder, .. } => Some(&placeholder.agent),
            PendingIntent::AttachCreatedWithRetry { placeholder, .. }
            | PendingIntent::AttachCreated { placeholder, .. } => placeholder.as_ref(),
            PendingIntent::SelectCreatedWorktree { launch, .. } => {
                launch.as_ref().and_then(|draft| draft.placeholder.as_ref())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Disconnected,
}

/// One row of the Sessions panel's PULL REQUESTS group: a previously saved
/// URL, or the pull request orion found on the worktree's branch — open,
/// draft, merged or closed alike.
#[derive(Debug, Clone)]
pub enum LinkRow {
    /// Discovered by `gh pr view`, backed by nothing in the store — so it
    /// can be opened but not edited or deleted.
    PullRequest(PullRequest),
    /// A saved link. `pr` is set when this link's URL *is* the detected
    /// pull request: the row then shows the PR's title and badge instead of
    /// a bare URL, and stays editable — it is still the user's own row.
    Saved { link: Link, pr: Option<PullRequest> },
}

impl LinkRow {
    pub fn url(&self) -> &str {
        match self {
            LinkRow::PullRequest(pr) => &pr.url,
            LinkRow::Saved { link, .. } => &link.url,
        }
    }

    /// The stored link behind the row; None for the detected pull request,
    /// which is what makes it un-editable and un-deletable.
    pub fn id(&self) -> Option<&LinkId> {
        match self {
            LinkRow::PullRequest(_) => None,
            LinkRow::Saved { link, .. } => Some(&link.id),
        }
    }

    /// The pull request this row stands for, however it got here.
    pub fn pull_request(&self) -> Option<&PullRequest> {
        match self {
            LinkRow::PullRequest(pr) => Some(pr),
            LinkRow::Saved { pr, .. } => pr.as_ref(),
        }
    }

    /// Row text: a pull request reads as `#42 title`, anything else as its
    /// URL with the noise (scheme, `www.`, trailing slash) stripped.
    pub fn label(&self) -> String {
        match self.pull_request() {
            Some(pr) => crate::pull_request::numbered_label(pr.number, &pr.title),
            None => pretty_url(self.url()),
        }
    }
}

/// A URL as a person reads it: no scheme, no `www.`, no trailing slash.
/// Purely cosmetic — the full URL is what gets opened.
pub fn pretty_url(url: &str) -> String {
    let bare = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let bare = bare.strip_prefix("www.").unwrap_or(bare);
    bare.strip_suffix('/').unwrap_or(bare).to_string()
}

/// One row in the Sessions panel: agents, then shell terminals, then the
/// worktree's links, then archived agents.
#[derive(Debug, Clone)]
pub enum SessionRow {
    Agent(Agent),
    Terminal(TerminalTab),
    Link(LinkRow),
}

impl SessionRow {
    pub fn name(&self) -> &str {
        match self {
            SessionRow::Agent(a) => &a.name,
            SessionRow::Terminal(t) => &t.name,
            SessionRow::Link(l) => l.url(),
        }
    }

    /// The attachable session behind the row. Link rows have none — they
    /// open a browser, not a PTY.
    pub fn sref(&self) -> Option<SessionRef> {
        match self {
            SessionRow::Agent(a) => Some(SessionRef::Agent(a.id.clone())),
            SessionRow::Terminal(t) => Some(SessionRef::Terminal(t.id.clone())),
            SessionRow::Link(_) => None,
        }
    }

    pub fn is_archived_agent(&self) -> bool {
        matches!(self, SessionRow::Agent(a) if a.archived)
    }

    pub fn as_link(&self) -> Option<&LinkRow> {
        match self {
            SessionRow::Link(l) => Some(l),
            _ => None,
        }
    }
}

/// One row of the WORKTREES PANEL, in cursor order — what
/// [`App::sel_worktree`] is a place in (see [`App::worktree_rows`]).
#[derive(Debug, Clone, Copy)]
pub enum WorktreeRow<'a> {
    /// A checkout listed on its own: the ROOT WORKTREE, or a branch no
    /// open pull request on screen is from.
    Checkout(&'a Worktree),
    /// A PROJECT OPEN PRS GROUP row.
    Pr(&'a OpenPr),
    /// A checkout on an open pull request's head branch — the one a PR
    /// SESSION launched off that row works in (`CreatePrAgent` cuts or
    /// reuses it), or one `n` cut on a branch a pull request was later
    /// opened from — listed under that pull request's row rather than
    /// among the plain checkouts, so the checkout and the pull request it
    /// is for read as one thing.
    PrCheckout(&'a Worktree),
}

impl<'a> WorktreeRow<'a> {
    /// The checkout the row is, wherever it sits: a plain one, or one
    /// nested under its pull request. None on a pull request row.
    pub fn checkout(self) -> Option<&'a Worktree> {
        match self {
            WorktreeRow::Checkout(w) | WorktreeRow::PrCheckout(w) => Some(w),
            WorktreeRow::Pr(_) => None,
        }
    }

    /// The pull request the row *is* — the OPEN PRS row itself, not the
    /// checkout nested under one: that row is a worktree, with a
    /// worktree's keys and sessions, and its pull request is one row up.
    pub fn open_pr(self) -> Option<&'a OpenPr> {
        match self {
            WorktreeRow::Pr(pr) => Some(pr),
            WorktreeRow::Checkout(_) | WorktreeRow::PrCheckout(_) => None,
        }
    }
}

/// A PANEL CURSOR: the row it is on BY IDENTITY, and where that row last
/// stood. Every list the cursors walk re-sorts under them on its own — on
/// a turn finishing, a session read or reaped, a checkout going dormant, a
/// pull request list landing — so the row is what is kept, and its index
/// is looked up in the rows as they stand ([`PanelCursor::index`]).
///
/// `last_index` answers only when the row is not in the list: it left
/// (deleted, archived away, another project's), or the cursor was put
/// where no row is yet. The cursor then stands where its row last was,
/// which is the row that slid up into its slot — the neighbour
/// [`App::reseat_cursors`] re-seats it on.
#[derive(Debug)]
struct PanelCursor<K> {
    key: Option<K>,
    last_index: std::cell::Cell<usize>,
}

impl<K> Default for PanelCursor<K> {
    fn default() -> Self {
        Self {
            key: None,
            last_index: std::cell::Cell::new(0),
        }
    }
}

impl<K> PanelCursor<K> {
    /// The cursor's index: where `find` finds its row, else where it last
    /// stood. A row found is remembered there, so a row that leaves later
    /// hands the cursor to the one in its last place, not its first.
    fn index(&self, find: impl FnOnce(&K) -> Option<usize>) -> usize {
        match self.key.as_ref().and_then(find) {
            Some(i) => {
                self.last_index.set(i);
                i
            }
            None => self.last_index.get(),
        }
    }

    /// Where to re-seat a cursor whose row is not among the `len` rows
    /// `find` searches: the row now in its last place, or the last row
    /// once the list is shorter than that. None for a cursor still on its
    /// row, or already resting on the empty list's nothing.
    fn reseat_at(&self, find: impl FnOnce(&K) -> Option<usize>, len: usize) -> Option<usize> {
        if self.key.as_ref().and_then(find).is_some() {
            return None;
        }
        let last = self.last_index.get();
        let at = last.min(len.saturating_sub(1));
        (len > 0 || self.key.is_some() || at != last).then_some(at)
    }

    fn set(&mut self, key: Option<K>, at: usize) {
        self.key = key;
        self.last_index.set(at);
    }
}

/// A WORKTREES PANEL row by what it is, for its cursor: a checkout keeps
/// its key whether it sits among the plain ones or under its pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorktreeRowKey {
    Checkout(WorktreeId),
    Pr(String),
}

impl WorktreeRow<'_> {
    fn key(self) -> WorktreeRowKey {
        match self {
            WorktreeRow::Checkout(w) | WorktreeRow::PrCheckout(w) => {
                WorktreeRowKey::Checkout(w.id.clone())
            }
            WorktreeRow::Pr(pr) => WorktreeRowKey::Pr(pr.url.clone()),
        }
    }

    /// [`WorktreeRow::key`] `== key`, without building one.
    fn has_key(self, key: &WorktreeRowKey) -> bool {
        match (self, key) {
            (
                WorktreeRow::Checkout(w) | WorktreeRow::PrCheckout(w),
                WorktreeRowKey::Checkout(id),
            ) => &w.id == id,
            (WorktreeRow::Pr(pr), WorktreeRowKey::Pr(url)) => &pr.url == url,
            _ => false,
        }
    }
}

/// A SESSIONS PANEL row by what it is, for its cursor: a session or a
/// terminal by its ref, a saved link by its id, the branch's detected pull
/// request by its URL.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionRowKey {
    Session(SessionRef),
    Link(LinkId),
    PullRequest(String),
}

impl SessionRow {
    fn key(&self) -> SessionRowKey {
        match self {
            SessionRow::Agent(_) | SessionRow::Terminal(_) => {
                SessionRowKey::Session(self.sref().expect("a session row has a ref"))
            }
            SessionRow::Link(LinkRow::Saved { link, .. }) => SessionRowKey::Link(link.id.clone()),
            SessionRow::Link(LinkRow::PullRequest(pr)) => {
                SessionRowKey::PullRequest(pr.url.clone())
            }
        }
    }

    /// [`SessionRow::key`] `== key`, without building one.
    fn has_key(&self, key: &SessionRowKey) -> bool {
        match (self, key) {
            (SessionRow::Agent(a), SessionRowKey::Session(SessionRef::Agent(id))) => &a.id == id,
            (SessionRow::Terminal(t), SessionRowKey::Session(SessionRef::Terminal(id))) => {
                &t.id == id
            }
            (SessionRow::Link(LinkRow::Saved { link, .. }), SessionRowKey::Link(id)) => {
                &link.id == id
            }
            (SessionRow::Link(LinkRow::PullRequest(pr)), SessionRowKey::PullRequest(url)) => {
                &pr.url == url
            }
            _ => false,
        }
    }
}

/// What a click landed on, for the double-click window. Sessions are their
/// own reference; a checkout is its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    Session(SessionRef),
    Worktree(WorktreeId),
}

/// Aggregate status for a worktree row, loudest first — needs you, then
/// working, then finished, then the rest — archived agents excluded.
/// Free-standing so the `/` palette can roll a
/// row up straight from the tree, with no `App` in hand.
pub fn worktree_rollup(tree: &Tree, worktree_id: &WorktreeId) -> Option<AgentStatus> {
    rollup(
        tree.agents
            .iter()
            .filter(|a| &a.worktree_id == worktree_id && !a.archived)
            .map(|a| a.status),
    )
}

/// The same aggregate over every worktree of a project.
pub fn project_rollup(tree: &Tree, project_id: &ProjectId) -> Option<AgentStatus> {
    let wt_ids: Vec<&WorktreeId> = tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project_id)
        .map(|w| &w.id)
        .collect();
    rollup(
        tree.agents
            .iter()
            .filter(|a| wt_ids.contains(&&a.worktree_id) && !a.archived)
            .map(|a| a.status),
    )
}

/// How many sessions under a worktree finished a turn nobody has looked at
/// yet (`Agent::unseen`) — the row's count badge, the number of terminals
/// to go read. Archived rows are out of sight, so they don't count.
pub fn worktree_unseen(tree: &Tree, worktree_id: &WorktreeId) -> usize {
    tree.agents
        .iter()
        .filter(|a| &a.worktree_id == worktree_id && !a.archived && a.unseen)
        .count()
}

/// The same count over every worktree of a project.
pub fn project_unseen(tree: &Tree, project_id: &ProjectId) -> usize {
    tree.worktrees
        .iter()
        .filter(|w| &w.project_id == project_id)
        .map(|w| worktree_unseen(tree, &w.id))
        .sum()
}

/// Whether the agent changed into a state that wants you — NEEDS YOU or
/// FAILED (a crash) — recently enough (`ONE_SHOT_SWEEP`) to still be
/// sweeping at `now` epoch ms. `status_changed_at` is that change itself.
/// The stamp is the DAEMON's clock and `now` this client's, so the window
/// is taken either side of it: a skewed pair sweeps a little off-time,
/// never for an hour.
pub fn fresh_alarm(agent: &Agent, now: i64) -> bool {
    let window = ONE_SHOT_SWEEP.as_millis() as i64;
    matches!(
        agent.status,
        AgentStatus::NeedsFeedback | AgentStatus::Terminated
    ) && !agent.archived
        && agent.status_changed_at > 0
        && (now - agent.status_changed_at).abs() < window
}

/// A finished turn nobody has looked at yet: DONE, NOT SEEN, the state the
/// UNREAD SHIMMER marks until the session is read. `unseen` is only ever
/// true on a finished row.
pub fn unread_finish(agent: &Agent) -> bool {
    agent.unseen && !agent.archived && agent.status == AgentStatus::Finished
}

/// Whether `agent` is drawn COLD: its PTY is gone — reaped, or lost to a
/// daemon restart — and nothing it last did is still true. A crash, or a
/// finish nobody has read, still is, and keeps its color; working or
/// asking stopped being true with the process, so the row draws gray and
/// still, and counts on no PROJECT TAB.
pub fn drawn_cold(agent: &Agent) -> bool {
    !agent.alive && agent.status != AgentStatus::Terminated && !unread_finish(agent)
}

/// A session put up as working before any PTY is behind it: a QUICK
/// PROMPT's stand-in, there before the intent that marks it one is
/// filed. A real row is never running without its process for longer
/// than the daemon takes to say so.
fn starting(agent: &Agent) -> bool {
    agent.status == AgentStatus::Running && !agent.alive
}

/// A session that is mid-turn or blocked on the user. These count as
/// interacting *now*, so they head the sessions list however long the turn
/// has taken — the point is to keep what needs attention in view.
pub fn is_active_status(s: AgentStatus) -> bool {
    matches!(s, AgentStatus::Running | AgentStatus::NeedsFeedback)
}

/// The stamp the recency sorts order a session on: its last interaction,
/// epoch ms, 0 = never run. A working session counts as interacting *now*
/// — it is producing output as you look at it, so it holds the top of the
/// list however long the turn has taken — and "now" is [`i64::MAX`], not
/// the clock: newer than any stamp, whenever the sort runs.
///
/// Never the clock. A sort read off the clock re-orders rows with nothing
/// in the tree changed: a session finishing in the same millisecond the
/// client handled it tied a working one at `now`, and the rows swapped
/// back and forth between frames as the clock broke the tie each way.
pub fn last_interaction_ms(a: &Agent) -> i64 {
    if is_active_status(a.status) {
        i64::MAX
    } else {
        a.status_changed_at
    }
}

/// Sort key for "most recently interacted with, first". Applied with a
/// stable sort, so never-run sessions (stamp 0) fall to the bottom of their
/// group in tree order. The SESSIONS panel and the LAUNCHER's grid both
/// order on it, so a session sits in the same place in either.
///
/// Working and blocked sessions all count as interacting *now*, so the raw
/// stamp breaks that tie: among them the newest turn leads — which is what
/// puts the session just launched at the top of the list, rather than under
/// every session that has been mid-turn for an hour (a launch handed a
/// first prompt is created `running`, stamped as it is created). Only live
/// turns are ordered that way; for every other row the last key is the
/// first one again, so nothing else moves.
pub fn recency_key(a: &Agent) -> (std::cmp::Reverse<i64>, std::cmp::Reverse<i64>) {
    (
        std::cmp::Reverse(last_interaction_ms(a)),
        std::cmp::Reverse(a.status_changed_at),
    )
}

/// The two stamps a worktree or project row derives from the sessions
/// under it. `interacted` is the newest [`last_interaction_ms`] — the sort
/// key, where a working session counts as now — and `stamped` the newest
/// raw `status_changed_at`, which the row's "23m ago" label reads so a
/// checkout with an hour-long turn in it says "1h ago" exactly like the
/// session does. Both 0 when nothing under the row has ever run.
///
/// Archived sessions count: the stamp records when work last happened
/// there, and archiving a row is housekeeping, not activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recency {
    pub interacted: i64,
    pub stamped: i64,
}

impl Recency {
    fn of<'a>(agents: impl Iterator<Item = &'a Agent>) -> Recency {
        agents.fold(Recency::default(), |mut r, a| {
            r.absorb(a);
            r
        })
    }

    /// Fold one more session in, for the one-pass rollups below.
    fn absorb(&mut self, a: &Agent) {
        self.interacted = self.interacted.max(last_interaction_ms(a));
        self.stamped = self.stamped.max(a.status_changed_at);
    }
}

/// EVERY checkout's [`Recency`], rolled up in ONE pass over the sessions.
///
/// The row sorts want all of them at once. Asking [`worktree_recency`]
/// per row walks the whole session list again for each — and `sort_by_key`
/// repeats the key on every COMPARISON, not once per row, so a project
/// with a couple of hundred sessions under one checkout pays tens of
/// thousands of id comparisons per sort. The sorts run several times per
/// frame and twice per turn of the event loop, which is enough to starve
/// the keyboard. One pass here, one hash lookup per comparison there.
pub fn worktree_recencies(tree: &Tree) -> HashMap<&WorktreeId, Recency> {
    let mut by_worktree: HashMap<&WorktreeId, Recency> = HashMap::new();
    for a in &tree.agents {
        by_worktree.entry(&a.worktree_id).or_default().absorb(a);
    }
    by_worktree
}

/// The same for every project, on the same one-pass footing — and with
/// the checkout→project map hashed too, where [`project_recency`] walks a
/// `Vec` of the project's checkouts once per session.
pub fn project_recencies(tree: &Tree) -> HashMap<&ProjectId, Recency> {
    let owner: HashMap<&WorktreeId, &ProjectId> = tree
        .worktrees
        .iter()
        .map(|w| (&w.id, &w.project_id))
        .collect();
    let mut by_project: HashMap<&ProjectId, Recency> = HashMap::new();
    for a in &tree.agents {
        let Some(project) = owner.get(&a.worktree_id) else {
            continue;
        };
        by_project.entry(project).or_default().absorb(a);
    }
    by_project
}

/// When a worktree last saw a turn: the newest stamp of any session in it.
pub fn worktree_recency(tree: &Tree, worktree_id: &WorktreeId) -> Recency {
    Recency::of(tree.agents.iter().filter(|a| &a.worktree_id == worktree_id))
}

/// The same over every worktree of a project.
pub fn project_recency(tree: &Tree, project_id: &ProjectId) -> Recency {
    let wt_ids: Vec<&WorktreeId> = tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project_id)
        .map(|w| &w.id)
        .collect();
    Recency::of(
        tree.agents
            .iter()
            .filter(|a| wt_ids.contains(&&a.worktree_id)),
    )
}

/// How loudly one status asks for a human: needs-feedback > running >
/// finished > gone > fresh. The order every rollup reads, and the one the
/// LAUNCHER VIEW's cards sort by, so a project card, a panel row and the
/// `/` PALETTE all agree on which session is the one to look at.
pub fn status_rank(s: AgentStatus) -> u8 {
    match s {
        AgentStatus::NeedsFeedback => 4,
        AgentStatus::Running => 3,
        AgentStatus::Finished => 2,
        AgentStatus::Terminated | AgentStatus::Disconnected => 1,
        AgentStatus::Fresh => 0,
    }
}

/// Priority-ordered aggregate: needs-feedback > running > finished > fresh.
pub fn rollup(statuses: impl Iterator<Item = AgentStatus>) -> Option<AgentStatus> {
    let mut best: Option<AgentStatus> = None;
    for s in statuses {
        best = Some(match best {
            Some(b) if status_rank(b) >= status_rank(s) => b,
            _ => s,
        });
    }
    best
}

/// Client-side mirror of the entity tree: every project on this machine,
/// with the checkouts, sessions, terminals and links under them.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub projects: Vec<Project>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
    pub terminals: Vec<TerminalTab>,
    pub links: Vec<Link>,
}

impl Tree {
    /// Any project at all? None is a first run: the splash, with the way
    /// to open one, is all there is to draw.
    pub fn has_projects(&self) -> bool {
        !self.projects.is_empty()
    }

    /// The project registered for the repo at `path`, or for any checkout
    /// of it — the folder the user named when a project already stands
    /// for it. An exact path match, so it is run on canonical paths
    /// ([`crate::event_loop`]'s add prompt canonicalizes first).
    pub fn project_at_path(&self, path: &std::path::Path) -> Option<&Project> {
        self.projects.iter().find(|p| {
            p.repo_path == path
                || self
                    .worktrees
                    .iter()
                    .any(|w| w.project_id == p.id && w.path == path)
        })
    }

    /// The repo of the project the checkout at `path` belongs to.
    pub fn repo_of(&self, path: &std::path::Path) -> Option<std::path::PathBuf> {
        self.project_at_path(path).map(|p| p.repo_path.clone())
    }

    /// The **Base branch** (Settings → Projects) of the project the
    /// checkout at `path` belongs to, read fresh from the file — for a
    /// one-off press; a beat reads it off the loop
    /// ([`crate::config::base_branch_of`]).
    pub fn base_branch_at(&self, path: &std::path::Path) -> String {
        crate::config::base_branch_of(self.repo_of(path).as_deref())
    }
}

/// What the pane's terminal emulation reports back beyond the screen
/// itself. One thing today: an OSC 52 clipboard write from the program in
/// the pane — Claude Code's fullscreen renderer copying its own selection
/// over ssh, vim's OSC 52 clipboard, tmux with `set-clipboard` on. In a
/// plain terminal that write lands on the clipboard; here it lands in the
/// emulation, so the parser parks it and the event loop passes it on to
/// the terminal the user is sitting at (`App::pending_clipboard`, the route
/// orion's own copy takes on a remote host).
#[derive(Debug, Default)]
pub struct TermCallbacks {
    /// Base64 payload of the latest OSC 52 write, until drained.
    pub clipboard: Option<String>,
}

impl vt100::Callbacks for TermCallbacks {
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8], data: &[u8]) {
        // vt100 only gets here with a base64-clean payload.
        self.clipboard = Some(String::from_utf8_lossy(data).into_owned());
    }
}

pub struct AttachedTerm {
    pub sref: SessionRef,
    pub parser: vt100::Parser<TermCallbacks>,
    pub exited: bool,
    /// Size the parser (and daemon PTY) currently uses.
    pub cols: u16,
    pub rows: u16,
    /// The child's kitty keyboard flags (daemon-tracked); picks the key
    /// encoding dialect. 0 = legacy.
    pub kitty_flags: u8,
    /// Whether any PTY bytes have reached this parser yet. False means the
    /// grid is blank because the session is still booting — attaching to a
    /// reaped session replays an empty ring, and an agent CLI takes seconds
    /// to paint its first frame. The pane says so instead of showing an
    /// unexplained void.
    pub painted: bool,
    /// The grid is blank because the session has no screen yet — the
    /// daemon replayed an empty ring, or the session was reaped and this
    /// attach is what boots it — as opposed to a replay that hasn't landed.
    /// The pane's "starting session…" notice shows only in the first case:
    /// flashing it for the frame a live session's replay takes reads as a
    /// hiccup on every switch.
    pub booting: bool,
    /// Byte offset the next PTY byte should carry, in the daemon's ring
    /// numbering: the end of everything this parser has processed. A
    /// re-attach asks for output from here, so a session shown a moment ago
    /// comes back as a gap-free delta onto the screen it left rather than a
    /// megabyte replay into a fresh parser (see [`App::term_cache`]).
    pub next_seq: u64,
    /// The scrollback was let go while this screen sat in
    /// [`App::term_cache`]: what is on screen is exact, what is above it is
    /// gone. Scrolling up asks the DAEMON for the whole ring again
    /// (`event_loop::rehydrate_history`), which rebuilds both.
    pub history_dropped: bool,
    /// The scroll offset to land on once that replay has rebuilt the
    /// history: the notch that asked for it.
    pub pending_scroll: Option<usize>,
}

/// Lines of scrollback the attached pane's parser keeps.
const SCROLLBACK_LINES: usize = 10_000;

impl AttachedTerm {
    pub fn new(sref: SessionRef, cols: u16, rows: u16) -> Self {
        Self {
            sref,
            parser: vt100::Parser::new_with_callbacks(
                rows,
                cols,
                SCROLLBACK_LINES,
                TermCallbacks::default(),
            ),
            exited: false,
            cols,
            rows,
            kitty_flags: 0,
            painted: false,
            booting: false,
            next_seq: 0,
            history_dropped: false,
            pending_scroll: None,
        }
    }

    /// Reset the parser (fresh replay is about to arrive).
    pub fn reset(&mut self) {
        self.parser = vt100::Parser::new_with_callbacks(
            self.rows,
            self.cols,
            SCROLLBACK_LINES,
            TermCallbacks::default(),
        );
        self.exited = false;
        self.painted = false;
        self.booting = false;
        self.next_seq = 0;
        self.history_dropped = false;
    }

    /// Apply a ring replay. One that continues exactly where this parser
    /// left off (`base_seq == next_seq`, the answer to an attach that asked
    /// `from_seq`) is appended and the screen stays; anything else — a
    /// first attach, a ring that wrapped past what was seen, a new process
    /// under the same session — rebuilds the screen from scratch. Returns
    /// whether it was rebuilt, so a selection anchored to the old cells can
    /// be dropped. An empty replay is the daemon saying nothing has painted
    /// yet, which is what `booting` reports.
    pub fn apply_scrollback(&mut self, base_seq: u64, data: &[u8]) -> bool {
        // A parser that watched its process exit has nothing worth
        // continuing: the only replay that can follow is the next process's.
        let rebuilt = base_seq != self.next_seq || self.exited;
        if rebuilt {
            self.reset();
        }
        self.next_seq = base_seq + data.len() as u64;
        self.feed(data);
        if !self.painted {
            self.booting = true;
        }
        // A replay asked for to get the history back lands the reader
        // where the scroll that asked for it was headed.
        if let Some(scroll) = self.pending_scroll.take() {
            if rebuilt {
                self.set_scroll(scroll);
            }
        }
        rebuilt
    }

    /// Apply live output that follows what the parser holds. Bytes a replay
    /// already covered are skipped: a second Attach of the session on
    /// screen (the history replay) can cross a frame the first one's
    /// forwarder had already queued, and parsing those bytes twice would
    /// garble the screen the replay just rebuilt.
    pub fn apply_output(&mut self, seq: u64, data: &[u8]) {
        let end = seq + data.len() as u64;
        if end <= self.next_seq {
            return;
        }
        let covered = self.next_seq.saturating_sub(seq) as usize;
        self.next_seq = end;
        self.feed(&data[covered.min(data.len())..]);
    }

    fn feed(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.painted = true;
            self.booting = false;
        }
        self.parser.process(data);
    }

    /// Regrid to `cols`×`rows` when that differs from the current size.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (self.cols, self.rows) != (cols, rows) {
            self.cols = cols;
            self.rows = rows;
            self.parser.screen_mut().set_size(rows, cols);
        }
    }

    /// Footprint in grid cells: the visible grid plus every scrollback row
    /// the primary screen holds — also while a full-screen program is up
    /// over it, which the offset-clamping trick this used to be could not
    /// see (the vendored `vt100` now says, `Screen::scrollback_rows`).
    pub fn estimated_cells(&self) -> usize {
        let lines = self.parser.screen().scrollback_rows();
        (lines + self.rows as usize) * self.cols as usize
    }

    /// Let the scrollback go, keeping the screen: what a kept screen that
    /// is over budget does instead of not being kept.
    pub fn drop_history(&mut self) {
        if self.parser.screen().scrollback_rows() == 0 {
            return;
        }
        self.set_scroll(0);
        self.parser.screen_mut().clear_scrollback();
        self.history_dropped = true;
    }

    /// How far up the history the view is, in lines above the live edge —
    /// 0 is the live tail. The parser's own offset, which output arriving
    /// while scrolled back moves up to keep the view still (a counter of
    /// ours went stale under it); or, while a let-go history is on its way
    /// back, the notch that asked for it (`pending_scroll`), which is where
    /// the replay lands.
    pub fn scroll_offset(&self) -> usize {
        self.pending_scroll
            .unwrap_or_else(|| self.parser.screen().scrollback())
    }

    /// Scroll the view to `scroll` lines above the live edge, as far as the
    /// history held allows: a notch past the top of it lands on the top,
    /// not on an offset the next notch down would have to work off before
    /// the view moved again — the counter this used to keep beside the
    /// parser's offset climbed past the top forever, and the way back down
    /// took as many notches as were spent up there.
    pub fn set_scroll(&mut self, scroll: usize) {
        self.parser.screen_mut().set_scrollback(scroll);
    }

    /// The program's latest OSC 52 clipboard write, if one arrived since the
    /// last call (see `TermCallbacks`).
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.parser.callbacks_mut().clipboard.take()
    }
}

/// Opaque UI state persisted in the daemon's DB for session restore.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct UiState {
    pub project: Option<String>,
    pub worktree: Option<String>,
    pub session_agent: Option<String>,
    pub show_archived: bool,
    pub collapsed: bool,
    /// The PROJECT OPEN PRS GROUP folded down to its header; absent in
    /// older blobs, which keep it open.
    #[serde(default)]
    pub open_prs_collapsed: bool,
    /// Diff modal file-list width; absent in older blobs.
    #[serde(default)]
    pub diff_files_width: Option<u16>,
    /// The diff modal's file list is the directory tree (`Ctrl+t`); absent
    /// in older blobs, which keep the flat list.
    #[serde(default)]
    pub diff_tree: bool,
    /// Height the LAUNCHER VIEW's pane was dragged to; absent in older
    /// blobs, and None in ones written before the edge was ever dragged,
    /// both of which open the pane on its default share.
    #[serde(default)]
    pub launcher_pane_h: Option<u16>,
    /// Width the pane was dragged to beside the cards; absent in older
    /// blobs, which open a side pane on its default half.
    #[serde(default)]
    pub launcher_pane_w: Option<u16>,
    /// The LAUNCHER VIEW's pane was folded away (`^~`); absent in older
    /// blobs, which open with it showing.
    #[serde(default)]
    pub launcher_pane_hidden: bool,
    /// The LAUNCHER VIEW's PROJECT TABS, by project id, far left first;
    /// absent in older blobs, which open with the one project restored.
    #[serde(default)]
    pub launcher_tabs: Vec<String>,
    /// Every PROJECT TAB was closed ([`App::projects_closed`]): the next
    /// start opens on the splash too. Absent in older blobs.
    #[serde(default)]
    pub projects_closed: bool,
    /// The checkouts whose ARCHIVED DRAWER was left unfolded
    /// ([`App::archived_open`]), by worktree id. Absent in older blobs.
    #[serde(default)]
    pub archived_open: Vec<String>,
}

/// A mouse selection over the terminal pane (drag or double-click word),
/// with inclusive `(col, line)` endpoints: a pane-relative column and the
/// screen's HISTORY LINE (`vt100::Screen::history_base`) — a number every
/// row keeps as the view scrolls and as new output pushes it up into the
/// scrollback. Anchored that way the highlight stays on its text while
/// the pane scrolls under a drag (the EDGE AUTO-SCROLL, the wheel) and
/// while the agent keeps printing, and the copy at release reads rows
/// that have since left the screen.
/// Orion owns the mouse (the emulator's native shift+drag never reaches us
/// reliably — Terminal.app has no such bypass at all), so selection is
/// implemented app-side and copied to the system clipboard when it completes.
/// The highlight persists after mouse-up; it's cleared by the next click,
/// the wheel, typing into the PTY, a resize, or a replay that rebuilds the
/// screen (its numbering starts over). A drag still in progress survives
/// every one of those but the click: the button is the user's, and nothing
/// here lets go of the selection until it comes up.
#[derive(Debug, Clone, Copy)]
pub struct TermSelection {
    pub anchor: (u16, u64),
    pub head: (u16, u64),
    /// Still being dragged (button down). Cleared on mouse-up.
    pub dragging: bool,
    /// A real selection, not just an armed click. Set once a drag leaves its
    /// starting cell (and kept if it returns), or immediately for a
    /// double-click word selection — which may be a single cell, so
    /// `anchor == head` can't be the "just a click" test.
    pub active: bool,
    /// Where the pointer last was, in host cells, while dragging. The EDGE
    /// AUTO-SCROLL re-reads the head from here on every tick, so a pointer
    /// resting past the pane's top or bottom edge keeps selecting as the
    /// history scrolls under it.
    pub pointer: (u16, u16),
}

impl TermSelection {
    /// Endpoints normalized to line-major order: (start, end).
    pub fn bounds(&self) -> ((u16, u64), (u16, u64)) {
        let anchor_key = (self.anchor.1, self.anchor.0);
        let head_key = (self.head.1, self.head.0);
        if anchor_key <= head_key {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// Mouse pointer shape the outer terminal should show, requested via the
/// xterm OSC 22 pointer-shape escape (CSS cursor names, per the kitty
/// pointer-shapes protocol). Mouse handlers record the want here; the event
/// loop emits the escape when it changes. Terminals that don't support the
/// sequence (Terminal.app) parse and drop it, so requesting is always safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerShape {
    #[default]
    Default,
    /// Horizontal-resize arrows over a draggable panel boundary.
    ColResize,
    /// Vertical-resize arrows over the LAUNCHER VIEW's pane boundary.
    RowResize,
}

impl PointerShape {
    /// The shape's name inside the OSC 22 escape.
    pub fn osc_name(self) -> &'static str {
        match self {
            PointerShape::Default => "default",
            PointerShape::ColResize => "col-resize",
            PointerShape::RowResize => "row-resize",
        }
    }
}

/// What `pull_request::list` last said about one project's open pull
/// requests, and the timer deciding when to ask again. Held per project
/// rather than refetched per repaint because every answer is a `gh`
/// process and a GitHub API call, and the list changes on the order of
/// minutes.
#[derive(Debug, Clone)]
pub struct OpenPrs {
    /// Open pull requests: newest first, with the drafts sunk below every
    /// finished one (`pull_request::drafts_last`, applied as the answer
    /// lands).
    pub list: Vec<OpenPr>,
    /// The pull requests merged within `pull_request::MERGED_DAYS`, the
    /// latest merge first, then the ones closed without merging within
    /// `pull_request::CLOSED_DAYS`: the PULL REQUESTS MODAL's `Merged` and
    /// `Closed` sections, and no other surface's — the group, the counts,
    /// the palette and AUTOFIX read `list`. Kept from the last answer that
    /// said, where one did not (`ListAnswer::merged`).
    pub merged: Vec<OpenPr>,
    /// When this answer landed. Switching projects pulls the next lookup
    /// forward, but never past this plus [`OPEN_PRS_MIN_AGE`] — otherwise
    /// bouncing between two projects would spend an API call per keystroke.
    pub at: std::time::Instant,
    /// When the next lookup is due, and the step that produced that
    /// deadline: a steady beat once a repo has proved it has pull requests,
    /// a doubling backoff while it hasn't.
    pub due: std::time::Instant,
    pub step: std::time::Duration,
}

/// What a debounced pull-request detail fetch needs: which PR, and the
/// checkout to run `gh` from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPrDetail {
    pub url: String,
    pub number: u64,
    pub dir: PathBuf,
}

/// A finished `gh pr comment`, back on the loop: which pull request (and
/// the row text that titled its box), the text that was sent — handed
/// back to the box when the post failed — and what `gh` said: the new
/// comment's URL, or why it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCommentAnswer {
    pub number: u64,
    pub url: String,
    pub label: String,
    pub body: String,
    pub result: Result<String, String>,
}

/// The pull request the TERMINAL PANE is reading instead of a session,
/// wherever the cursor found it — a PROJECT OPEN PRS GROUP row or the
/// SESSIONS PANEL's PR ROW. Just enough to fetch, title and scroll it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewedPr {
    pub number: u64,
    pub url: String,
    /// Row text, `#42 title` — what the pane says while the body loads.
    pub label: String,
}

/// How recently a project's open-PR list may have been fetched and still be
/// refetched on arrival — at a project, at a sidebar panel, or back at the
/// terminal window. Walking the project list, or a flurry of focus events,
/// must not turn into one API call per gesture; a few seconds is enough to
/// coalesce those while still beating the steady beat by a wide margin.
pub const OPEN_PRS_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(5);

/// Which status edge a desktop notification is about — what its summary
/// says, and which sound's setting switches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    /// The edge into NEEDS FEEDBACK: a question, a permission prompt, or a
    /// usage limit (`FeedbackAlert::limit`). Rides the FEEDBACK SOUND.
    NeedsFeedback,
    /// A live turn whose CLI died with an error — the edge from RUNNING or
    /// NEEDS FEEDBACK into TERMINATED. Nothing will carry it on until the
    /// user does, so it rides the FEEDBACK SOUND too.
    Crashed,
    /// A turn that finished with nobody looking, once it has settled
    /// ([`DoneSounds`]). Rides the DONE SOUND.
    Finished,
}

/// One session a desktop notification names: the edge it took, the row's
/// name and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackAlert {
    /// What happened to it.
    pub kind: AlertKind,
    /// The session row's name.
    pub session: String,
    /// `<project> · <branch>`, the worktree it runs in; empty when the tree
    /// no longer holds them.
    pub place: String,
    /// What the session is waiting on when it is not a question: the label
    /// of the usage limit it stopped on (`limit reached`), from
    /// `Agent::limit_reached`.
    pub limit: Option<&'static str>,
}

/// DONE SETTLE: how long a finish has to stand before its DONE SOUND
/// rings. Long enough to outlast the flickers — a subagent's `SubagentStart`
/// healing a finish its `Stop` raced, a queued message starting the next
/// turn — short enough to still read as the moment it finished.
pub const DONE_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

/// DONE FOLD: a DONE SOUND due within this long of the last sound of either
/// kind is not rung — the one that just rang already said "go and look".
pub const DONE_FOLD: std::time::Duration = std::time::Duration::from_secs(10);

/// The DONE SOUND between a status edge and the speaker. A finish rings
/// only when it is news:
///
/// - UNSEEN only. A turn that finishes in the pane on screen was watched
///   — the edge takes its `Agent::unseen` straight down — so it rings
///   nothing; neither does an archived row's, which never raises the flag.
/// - SETTLE. The sound is due [`DONE_SETTLE`] after the edge, and stands
///   down if by then the row has left FINISHED, been seen, or gone
///   (deleted, archived). A finish that flickers rings once, where it
///   lands.
/// - ONCE PER UNSEEN SPELL. An agent whose finish has been announced rings
///   nothing more until the user has seen it: the wake-up turns a task
///   notification or a queued message starts, each finishing on its own,
///   and the subagent drain that reopens a finished row and closes it
///   again 180 s later, are one thing to go and read, not a sound each.
/// - FOLD. A sound due within [`DONE_FOLD`] of the last sound of either
///   kind is dropped. The finish still counts as announced — the sound
///   that just rang covers it — and still gets its desktop notification.
///
/// The event loop feeds it (`finished` on the edge, `seen` from
/// `mark_agent_seen`), wakes for [`DoneSounds::next_due`], and rings what
/// [`DoneSounds::settle`] hands back.
#[derive(Debug, Default)]
pub struct DoneSounds {
    /// Finishes waiting out the SETTLE, each with when it is due. Another
    /// finish of the same agent meanwhile starts its wait over.
    pub due: HashMap<AgentId, std::time::Instant>,
    /// Agents announced in their current UNSEEN SPELL.
    pub announced: std::collections::HashSet<AgentId>,
    /// When a sound of either kind last rang, for the FOLD.
    pub last_rang: Option<std::time::Instant>,
}

impl DoneSounds {
    /// A live turn of `agent` reached FINISHED at `now` and is unseen after
    /// the pane on screen had its say: start the SETTLE — unless this
    /// UNSEEN SPELL has been announced already.
    pub fn finished(&mut self, agent: &AgentId, now: std::time::Instant) {
        if !self.announced.contains(agent) {
            self.due.insert(agent.clone(), now + DONE_SETTLE);
        }
    }

    /// The user has seen `agent` — its pane came on screen, here or in
    /// another client: its UNSEEN SPELL is over, so its next finish is news
    /// again, and a finish still settling is not.
    pub fn seen(&mut self, agent: &AgentId) {
        self.announced.remove(agent);
        self.due.remove(agent);
    }

    /// When the next settling finish falls due — the event loop's wake-up
    /// for it, since nothing else need be moving on screen by then.
    pub fn next_due(&self) -> Option<std::time::Instant> {
        self.due.values().min().copied()
    }

    /// The finishes whose SETTLE ran out by `now` and that still stand —
    /// the row in `tree` still FINISHED, still unseen, not archived — in
    /// tree order, each now announced for its UNSEEN SPELL. Those that no
    /// longer stand are dropped without a sound.
    pub fn settle(&mut self, tree: &Tree, now: std::time::Instant) -> Vec<AgentId> {
        if self.next_due().is_none_or(|due| due > now) {
            return Vec::new();
        }
        let mut ripe = std::collections::HashSet::new();
        self.due.retain(|agent, due| {
            let keep = *due > now;
            if !keep {
                ripe.insert(agent.clone());
            }
            keep
        });
        let settled: Vec<AgentId> = tree
            .agents
            .iter()
            .filter(|a| ripe.contains(&a.id) && unread_finish(a))
            .map(|a| a.id.clone())
            .collect();
        // A deleted agent is never seen again to end its spell.
        self.announced
            .retain(|agent| tree.agents.iter().any(|a| a.id == *agent));
        self.announced.extend(settled.iter().cloned());
        settled
    }

    /// Whether a DONE SOUND due `now` rings, rather than FOLD into a sound
    /// that rang within [`DONE_FOLD`].
    pub fn may_ring(&self, now: std::time::Instant) -> bool {
        self.last_rang
            .is_none_or(|rang| now.saturating_duration_since(rang) >= DONE_FOLD)
    }

    /// A sound of either kind just rang.
    pub fn rang(&mut self, now: std::time::Instant) {
        self.last_rang = Some(now);
    }
}

/// The FOLLOW-UP COMPOSER: the box a session card grows when it is
/// expanded, and the next turn being typed into it.
///
/// One at a time, because it owns the keyboard while it is open: the
/// SESSIONS PANEL's own keys (`j`, `a`, `d`…) are letters, so a card with a
/// live box takes every key the panel would otherwise act on. It is bound
/// to the AGENT rather than to a row index — the list re-sorts on every
/// status change, and the box has to stay on the card it was opened on.
pub struct FollowUp {
    pub agent: AgentId,
    /// Multi-line, like the QUICK PROMPT's box: Enter sends, Shift+Enter /
    /// ⌥Enter / `^J` break the line.
    pub input: TextInput,
}

/// The ROWS MEMO: where the cursor is, worked out once for a stretch that
/// asks it over and over and changes none of what the answer is built
/// from — one frame (`ui::draw`), one [`App::reading_url`].
///
/// A frame asks a dozen times: the grid and the pane's strip for the card
/// under the cursor, the pane for whether it is reading a pull request,
/// the footer for its breadcrumb. Every asking
/// re-sorted the projects and the selected project's checkouts from a
/// roll-up of every session on the machine, and cloned the checkout's
/// session rows to hand back one of them — on a machine with a few
/// hundred sessions, most of a debug build's frame, and a wheel notch
/// over the pane waited behind it.
///
/// Off outside a stretch, where everything is built fresh as it always
/// was: a handler that moves the cursor and then asks where it is must
/// get the new answer. Within one, the kept rows also answer only for the
/// cursor and the tree's shape they were built for ([`RowsKey`]), so a
/// stretch that moved either would rebuild rather than read stale rows.
#[derive(Default)]
pub struct RowsMemo {
    armed: std::cell::Cell<bool>,
    key: std::cell::Cell<Option<RowsKey>>,
    /// [`App::project_rows`].
    projects: std::cell::RefCell<Option<Vec<usize>>>,
    /// [`App::visible_worktrees`], as indices into `tree.worktrees`.
    worktrees: std::cell::RefCell<Option<Vec<usize>>>,
    /// [`App::visible_session_rows`].
    sessions: std::cell::RefCell<Option<Vec<SessionRow>>>,
}

/// What the kept rows were built for: the selection (by how many times a
/// cursor has moved, [`App::cursor_moves`]) and how many of each thing the
/// tree holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RowsKey {
    cursor: u64,
    shape: [usize; 5],
    show_archived: bool,
    /// How many ARCHIVED DRAWERS are open: an open one lists its
    /// checkout's archived rows ([`App::sessions_in`]).
    drawers: usize,
}

impl RowsMemo {
    /// Start a stretch: nothing kept yet, everything asked from here on
    /// is kept until [`RowsMemo::disarm`].
    pub fn arm(&self) {
        self.clear();
        self.armed.set(true);
    }

    /// End the stretch and let go of what it kept.
    pub fn disarm(&self) {
        self.armed.set(false);
        self.clear();
    }

    /// Run `f` as a stretch of its own, or as part of the one already
    /// running.
    pub fn hold<R>(&self, f: impl FnOnce() -> R) -> R {
        if self.armed.get() {
            return f();
        }
        self.arm();
        let out = f();
        self.disarm();
        out
    }

    fn clear(&self) {
        self.key.set(None);
        self.projects.take();
        self.worktrees.take();
        self.sessions.take();
    }

    /// `read` what `slot` keeps for `key`, building it the first time the
    /// stretch asks; outside a stretch, `read` a fresh build.
    fn with<T, R>(
        &self,
        slot: fn(&Self) -> &std::cell::RefCell<Option<T>>,
        key: RowsKey,
        build: impl FnOnce() -> T,
        read: impl FnOnce(&T) -> R,
    ) -> R {
        if !self.armed.get() {
            return read(&build());
        }
        if self.key.get() != Some(key) {
            self.clear();
            self.key.set(Some(key));
        }
        if slot(self).borrow().is_none() {
            // Built before it is stored: a build asks the slots below it
            // (the session rows ask for the checkout, which asks for the
            // project), never its own.
            let built = build();
            *slot(self).borrow_mut() = Some(built);
        }
        read(slot(self).borrow().as_ref().expect("built above"))
    }
}

pub struct App {
    pub tree: Tree,
    pub focus: Focus,
    /// The three panel cursors — the project, the Worktrees row and the
    /// Sessions row — each held by the row it is on, not by where that row
    /// stands ([`PanelCursor`]). Read with [`App::sel_project`] and the
    /// rest, moved with [`App::set_sel_project`] and the rest.
    project_cursor: PanelCursor<ProjectId>,
    worktree_cursor: PanelCursor<WorktreeRowKey>,
    /// The HELD CHECKOUT: the one the Worktrees cursor is on, and whether
    /// it was DORMANT when the cursor arrived. It keeps that group, and so
    /// its place, for as long as the cursor stays on it — clicking a
    /// dormant card wakes its checkout, and reading a live one's last
    /// unread finish puts it to sleep, and either used to move the row out
    /// from under the click. It goes to its group as the cursor leaves.
    /// Set by [`App::hold_cursor_worktree`] on every move of that cursor.
    worktree_hold: Option<(WorktreeId, bool)>,
    session_cursor: PanelCursor<SessionRowKey>,
    /// Bumped on every cursor move: what the ROWS MEMO keys on
    /// ([`RowsKey`]), so rows kept for one selection are never read for
    /// another.
    cursor_moves: u64,
    pub term: Option<AttachedTerm>,
    /// Screens of the sessions the pane showed most recently, most recent
    /// first — at most [`TERM_CACHE_MAX`], each under [`TERM_CACHE_CELLS`].
    /// Coming back to one puts its last screen up on the frame the cursor
    /// moves and asks the daemon only for the bytes it missed
    /// ([`AttachedTerm::next_seq`]), instead of replaying the whole ring
    /// into a fresh parser. Only live, painted, real sessions are kept: a
    /// reaped or exited one comes back as a new process whose ring starts
    /// over, and a QUICK PROMPT stand-in never had a PTY.
    pub term_cache: Vec<AttachedTerm>,
    /// Input lock: keys forward to the attached PTY. Focusing the terminal
    /// pane alone (Tab / arrows) does NOT lock — Enter or a click does.
    pub term_locked: bool,
    pub conn: ConnState,
    pub hits: Vec<(Rect, HitTarget)>,
    /// Inner rect of the terminal pane from the last draw.
    pub term_area: Rect,
    /// Where the host terminal's own cursor is parked once a frame is
    /// flushed: the cell under the cursor of the PTY the keyboard is
    /// headed for — the editor modal's, else the attached session's — or
    /// None to leave it wherever the frame's last diff run ended. The host
    /// cursor stays hidden either way (the pane paints its own), but a
    /// CJK input method anchors its composition — the preedit text and
    /// the candidate window — to the hardware cursor's cell, hidden or
    /// not, so an unparked cursor put Japanese preedit at the edge of the
    /// window instead of at the prompt (#53). Set by `ui::draw`, applied
    /// by the event loop after the frame.
    pub host_cursor: Option<Position>,
    pub dirty: bool,
    pub should_quit: bool,
    /// Set with `should_quit` when the hosts picker chose a destination:
    /// after teardown the binary execs `orion ssh` at it, replacing this
    /// process with a fresh connection.
    pub pending_ssh: Option<crate::hosts::HostEntry>,
    /// Set with `should_quit` by **Restart orion**: after teardown the
    /// binary stops the daemon and execs itself afresh (`crate::restart`).
    /// **Upgrade orion** confirmed: quit into [`crate::Exit::Upgrade`].
    pub upgrade: bool,
    pub restart: bool,
    /// The FOOTER's one line in place of its key hints, until the next key.
    pub flash: Option<crate::flash::Flash>,
    /// The newest release published on GitHub (`0.22.0`) when it is newer
    /// than this build — the footer's `⇡ v0.22.0` beside the version
    /// nameplate. `None` until the update check finds one; a check that
    /// can't ask leaves it as it was.
    pub update_available: Option<String>,
    /// Where an on-demand update check (**Upgrade orion**) sends its
    /// answer; `None` while the checker is off (the e2e tests).
    pub update_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::update_check::Answer>>,
    /// What Spotify is playing, for the footer's SPOTIFY READOUT
    /// (`crate::spotify`): `None` while it is closed, stopped or empty, the
    /// readout off, or no poll has answered yet.
    pub spotify: Option<crate::spotify::NowPlaying>,
    /// The Spotify polls and button commands out: the beat skips while
    /// any are, rather than start another `osascript` beside them.
    pub spotify_in_flight: usize,
    /// Clicks on the readout's buttons so far. Every poll carries the count
    /// it set out under, and a track one heard before the latest click is
    /// dropped — it would undo the glyph the click just flipped.
    pub spotify_seq: u64,
    /// The AUTOMATION prompt was denied (`-1743`): the poll has stopped for
    /// the rest of the run, the footer having said once where to allow it.
    pub spotify_denied: bool,
    /// The **Spotify in footer** setting. Mirrors the config, refreshed at
    /// startup and when the settings overlay applies a change; off here
    /// until startup applies it.
    pub spotify_enabled: bool,
    /// AWAY, as the daemon last said (`ServerEvent::AwayChanged`).
    pub away: bool,
    /// AWAY BY HAND (⌘⇧A) is on: the next key sends `SetAway { on: false }`.
    pub away_by_hand: bool,
    /// How the daemon's Slack connection stands (`ServerEvent::SlackStatus`).
    pub slack_status: orion_core::SlackStatus,
    /// Requests queued by a handler that has no request list to hand —
    /// the settings overlay's — sent with the loop's own on its next turn.
    pub outbox: Vec<orion_core::ClientRequest>,
    /// Where the Spotify polls and commands answer — the loop's channel,
    /// held here so a click on one of the readout's buttons can reach it.
    pub spotify_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::spotify::Answer>>,
    /// The last `h`/`l` (or ←/→) that landed on the end of the panel row,
    /// or `k`/`j` (↑/↓) on a panel's first row, and stayed put, with when
    /// it arrived: a second press of the same action inside `DOUBLE_TAP`
    /// jumps the boundary the way ⇧Tab / Tab would. Any other key in
    /// between clears it.
    pub edge_tap: Option<(crate::keymap::Action, std::time::Instant)>,
    /// The key that just unarchived a card, watched until the host reports
    /// it let go, so a held `u` unarchives once (event_loop/release_watch.rs).
    pub release_watch: Option<crate::event_loop::ReleaseWatch>,
    pub overlay: Option<Overlay>,
    /// The expanded session card's FOLLOW-UP COMPOSER, or None with every
    /// card folded. Not an `Overlay`: it draws inside the SESSIONS PANEL
    /// and the panels stay live around it — what it takes is the keyboard,
    /// not the screen.
    pub follow_up: Option<FollowUp>,
    pub show_archived: bool,
    /// The checkouts whose ARCHIVED DRAWER is unfolded on the grid (`z`,
    /// or a click on its `▸ N archived` line): their archived sessions
    /// listed one faint line apiece under the band, where the cursor
    /// walks onto them and `u` brings one back. Rides the UI-state blob.
    pub archived_open: std::collections::HashSet<WorktreeId>,
    /// The Worktrees panel's OPEN PRS group folded down to its header (a
    /// click on it). Like `show_archived`, it rides the UI-state blob so a
    /// restart brings it back folded.
    pub open_prs_collapsed: bool,
    /// The session in the pane is FULL-SCREEN (`^F`, `zoom_pane`) — the
    /// grid and its header give way to the PTY.
    pub collapsed: bool,
    /// Draft pull requests left out of the PROJECT OPEN PRS GROUP and the
    /// `/` PALETTE; mirrors CONFIG.JSON's `hide_draft_prs` (Settings →
    /// Appearance, or the Worktrees panel menu). Read on every look at
    /// the list (`listed_open_prs`), never applied to what is stored.
    pub hide_draft_prs: bool,
    /// Nothing in the LAUNCHER VIEW's GRID is selected: the aim has been
    /// let go of — by a click on the air between the cards, or by the
    /// first Esc (`event_loop::launcher::clear_aim`). No card is drawn
    /// wearing the cursor while it is set, and the box `p` opens has no
    /// card to read a checkout off, so it is aimed at the project's ROOT
    /// BRANCH (`event_loop::launcher::open_box`) — the way out of a box
    /// locked onto whichever worktree the cursor was last parked in.
    /// Anything that puts the cursor back on a card takes the aim back
    /// (`event_loop::launcher::take_aim`).
    pub launcher_unaimed: bool,
    /// The GRID's MULTI-SELECT: the cards picked out together — by
    /// `⇧↑`/`⇧↓`/`⇧←`/`⇧→` over a range, or `⌘`-click (`⌥`-click its
    /// twin) one at a time — for `⌫` to delete in one confirm. In grid
    /// order (`launcher::grid_order`) after a range, in click order after
    /// a toggle; empty with nothing picked, which is most of the time. A
    /// bare arrow, a plain click or Esc lets it go.
    pub marked: Vec<SessionRef>,
    /// Where the MULTI-SELECT's range is measured from: the card the
    /// first `⇧`-arrow left, or the last one `⌘`-clicked.
    pub mark_anchor: Option<SessionRef>,
    /// Height the LAUNCHER VIEW's PANE was dragged to, in rows; None until
    /// its top edge is dragged, which leaves the pane on its default share
    /// of the body. Re-clamped to the body on every draw
    /// (`launcher::pane_height`), so a height kept from a taller window
    /// never squeezes the cards out.
    pub launcher_pane_h: Option<u16>,
    /// Width the PANE was dragged to while it stands beside the cards, in
    /// columns: [`App::launcher_pane_h`]'s twin for a pane on the right,
    /// kept apart from it so switching sides never reads a height as a
    /// width. Re-clamped the same way (`launcher::pane_width`).
    pub launcher_pane_w: Option<u16>,
    /// The LAUNCHER VIEW's PANE is folded away (`^~`): the GRID takes the
    /// whole body and no session is read under it. Hiding it also lets
    /// the card under the cursor go (`event_loop::launcher::toggle_pane`
    /// runs the same `clear_aim` the first Esc does), so the cards stand
    /// on their own — nothing selected, nothing being read. It is
    /// remembered across restarts with the pane's height.
    pub launcher_pane_hidden: bool,
    /// How far the whole GRID is scrolled: rows of its panel layout
    /// (`launcher::panel_layout`) above the window's top edge, the way a
    /// terminal's screen scrolls through its history, the whole list as
    /// one. The wheel moves it a few rows a notch
    /// (`event_loop::launcher::wheel_grid`) and the cursor stays where
    /// it is — a trackpad never swaps the pane out from under the card
    /// being read; a cursor move pulls it just far enough to bring the
    /// cursor's card whole on screen; and every draw holds it within the
    /// layout, so one kept from a taller list never leaves the window
    /// empty. Settled by `ui::launcher_view` each frame from the three
    /// fields after it.
    pub launcher_scroll: u16,
    /// The wheel put the scroll where it is: the draw leaves it there,
    /// whatever the cursor's card does, until a key or a landing asks
    /// for that card again ([`App::launcher_reveal`]) or the cursor moves
    /// onto another ([`App::launcher_scroll_on`]). Otherwise the scroll
    /// keeps in step with the cursor's card — a card that moves down the
    /// grid as a new session lands over it stays on screen.
    pub launcher_scroll_held: bool,
    /// The card the scroll last kept on screen: a frame that finds the
    /// cursor on another one scrolls to that, wheel or no wheel.
    pub launcher_scroll_on: Option<SessionRef>,
    /// A key asked for the cursor's card whether or not the cursor moved
    /// (`j` against the grid's edge after a wheel away, a landing on the
    /// card already selected): the next frame scrolls to it.
    pub launcher_reveal: bool,
    /// In-progress drag of that edge: `boundary row - grab row` at
    /// mouse-down, so the edge tracks the pointer instead of jumping by
    /// one depending on which of the two grab rows was caught.
    pub launcher_pane_drag: Option<i32>,
    /// A PROJECT TAB being dragged along the header to a new place
    /// (`event_loop::launcher::drag_tab`): the tab pressed, and the tab
    /// it last traded places with — held so a pointer still over that
    /// one, before the next draw moves the hit rects, does not trade them
    /// straight back.
    pub launcher_tab_drag: Option<(ProjectId, Option<ProjectId>)>,
    /// That edge is under the mouse, or being dragged: its grip lights up.
    /// Only ever set in terminals that report plain mouse motion;
    /// elsewhere the grip rests until a drag takes hold.
    pub hover_launcher_pane: bool,
    /// The header button under the pointer, from the last mouse report:
    /// a PROJECT TAB, its `×`, the `+` after them, a full-screen
    /// session's `‹ sessions`, or one of the footer's buttons. The header
    /// marks that one for as long as
    /// it is there, so a word reads as the button it is before anyone
    /// clicks to find out. Carries `hover_launcher_pane`'s caveat — only
    /// terminals that report plain motion ever set it, and elsewhere the
    /// header rests plain.
    pub hover_crumb: Option<HitTarget>,
    /// The LAUNCHER VIEW's PROJECT TABS: every project opened on the
    /// SESSIONS level since its tab was last closed, in the order the
    /// user keeps them — a new tab goes on at the right, and only a drag
    /// along the header (`event_loop::launcher::drag_tab`) ever moves
    /// one. Kept up by [`App::settle_project_tabs`] and
    /// [`App::ensure_project_tab`], closed one at a time by
    /// `event_loop::launcher::close_tab`, and remembered across restarts.
    pub launcher_tabs: Vec<ProjectId>,
    /// Every PROJECT TAB has been closed: orion is back on the SPLASH it
    /// opens on before there is any project, with the projects themselves
    /// and their sessions untouched. Set by closing the last tab
    /// (`event_loop::launcher::close_tab`); any way into a project — the
    /// splash's Enter, `+`, `o`, `/` — clears it
    /// ([`App::reopen_projects`]). Remembered across restarts.
    pub projects_closed: bool,
    /// The upgrade confirm has been put up this run: the SPLASH's Enter,
    /// which offers a waiting release first, goes back to beginning — the
    /// footer's `⇡ v…` still offers it. Never remembered across a restart.
    pub splash_upgrade_asked: bool,
    /// HOME is up: the SPLASH, with orion's animation, drawn over a grid
    /// that is still there underneath, cursor and all — `⌘G` (or a click
    /// on the footer's nameplate) puts it up, and Esc, Enter, an arrow or
    /// any way into a project takes it down onto that grid again
    /// (`event_loop::home_key`). Never remembered across a restart.
    pub home: bool,
    /// The PROJECT TABS have the keyboard, and this is the tab their
    /// cursor is on: `k`,`k` (↑,↑) on the GRID's top row walks up into the
    /// header (`event_loop::launcher::focus_tabs`), `h` / `l` move this
    /// cursor along the tabs and switch the grid to each project as they
    /// pass, on the card it was last left on, and Enter — or `j`,`j` back
    /// down — hands the keys back to that card
    /// (`event_loop::launcher::choose_tab`). None with the keys on the
    /// cards, which is every other moment: Esc, a click anywhere, and any
    /// other key hand them back. Never remembered across a restart.
    pub launcher_tab_cursor: Option<ProjectId>,
    /// The PROJECT TABS the header last had no room for, in tab order —
    /// the ones its MORE CHIP stands in for and lists when clicked.
    /// Written by the draw, as the hit rects are, so the list a click
    /// drops is the one the chip on screen counts.
    pub launcher_tabs_more: Vec<ProjectId>,
    /// The git repository this instance was started in
    /// ([`launch_repo`]), read once at launch: the folder the first run's
    /// SPLASH opens on Enter and the open-project prompt starts on.
    pub launch_repo: Option<PathBuf>,
    /// The whole body the LAUNCHER VIEW splits, from the last draw.
    /// `body_area` there is the grid's half alone, so the pane drag takes
    /// its bounds from here.
    pub launcher_body: Rect,
    /// The last key press, spelled for the bottom-left of the screen with
    /// what it did, while the press is fresh; the loop clears it after
    /// `key_combo::LINGER`. See `key_combo.rs`.
    pub key_combo: Option<crate::key_combo::KeyCombo>,
    pub next_req_id: u64,
    pub pending: HashMap<u64, PendingIntent>,
    /// In-flight creates — keys of `pending` — the user has navigated away
    /// from since firing them: a key or a click moved a cursor or FOCUS
    /// while the DAEMON worked (a PR SESSION's fetch and `git worktree
    /// add` are seconds). Their Ack still turns the stand-ins into the
    /// real rows, but leaves the cursors, the pane and FOCUS where the
    /// user put them — a manual move outranks a selection-follow. Filled
    /// by `event_loop::handle_terminal_event`, emptied by the Ack or Error.
    pub left_behind: std::collections::HashSet<u64>,
    /// Session created by us, awaiting its upsert to fix the selection.
    pub select_when_seen: Option<SessionRef>,
    /// The session this client just launched, held first in the sessions
    /// lists until its own first turn starts. It is the launch with nothing
    /// to submit that needs this — one carrying a task is created `running`
    /// and leads on its stamp alone ([`recency_key`]). Without it such a row
    /// arrives `fresh`, stamped a moment ago, while every session mid-turn
    /// counts as interacting *now* ([`last_interaction_ms`]) — so the new
    /// card landed *below* the working ones and only jumped to the top left
    /// a second later, when its first turn began. Cleared by that first
    /// status change (recency holds the card there from then on), and
    /// replaced by the next launch; a row that has gone away just stops
    /// matching, so nothing has to clear it.
    pub just_launched: Option<AgentId>,
    /// Project added by us, awaiting its upsert to fix the selection.
    pub select_project_when_seen: Option<ProjectId>,
    /// Worktree created by us, awaiting its upsert to fix the selection.
    pub select_worktree_when_seen: Option<WorktreeId>,
    /// A pull request's checkout whose Ack beat its upsert here, and what
    /// it was for (`pr_checkout::seen`).
    pub pr_checkout_waiting: Option<(WorktreeId, crate::pr_checkout::Then)>,
    /// Last selected worktree per project — switching back to a project
    /// returns to the worktree the user left it on.
    pub last_worktree_for_project: HashMap<ProjectId, WorktreeId>,
    /// Last selected session per worktree — switching back to a worktree
    /// re-shows the session the user left it on.
    pub last_session_for_worktree: HashMap<WorktreeId, SessionRef>,
    /// Debounced session prewarm: the worktree whose dead sessions the
    /// daemon should pre-spawn once the selection has rested on it past the
    /// deadline — armed on every worktree context switch, so walking the
    /// list doesn't boot every CLI it passes.
    pub pending_prewarm: Option<(WorktreeId, std::time::Instant)>,
    /// A refused PR SESSION's box that could not come back because another
    /// modal was up when the refusal landed — the DAEMON fetches before it
    /// refuses, seconds after Enter, so the user has usually moved on. It
    /// is never put over that modal (whose own text would go); the next
    /// quick prompt opened on the same pull request starts from it
    /// instead, as a refused pull request comment does.
    pub parked_pr_prompt: Option<(String, String)>,
    /// The QUICK PROMPT boxes abandoned with something typed in them
    /// (`quick_prompt::QuickDraft`) — Esc, a click outside, the HARDWIRED
    /// UNLOCK — one per place (worktree, or fresh worktree in a project).
    /// The next box opened at that place takes it back, so a press that
    /// closes the box costs nothing typed; in memory only — the SAVED
    /// DRAFT below is what outlives the process.
    pub quick_draft: crate::quick_prompt::QuickDrafts,
    /// The SAVED DRAFT (`saved_draft`): the QUICK PROMPT's unsent text on
    /// disk, per place, written as it is typed, so a window closed mid-sentence keeps
    /// the sentence for the next box. The main loop installs the DATA
    /// DIR's at startup; the unit tests leave it `None` (or install a
    /// temporary one), so no test touches the real user's draft.
    pub saved_draft: Option<crate::saved_draft::SavedDraft>,
    /// Debounced attach: the session the pane is showing but the daemon has
    /// not been told about yet. Stepping a selection is not a decision to
    /// boot a CLI — walking the grid past four cards must not cold-spawn
    /// four agents and abandon three of them.
    pub pending_attach: Option<(SessionRef, std::time::Instant)>,
    /// What this connection is attached to daemon-side. Lags `term.sref`
    /// while an attach waits out its debounce, so the Detach that precedes
    /// the next Attach names the session the daemon actually holds.
    pub attached_sref: Option<SessionRef>,
    /// Mouse drag-selection over the terminal pane, if any.
    pub term_selection: Option<TermSelection>,
    /// The next EDGE AUTO-SCROLL tick: set while a drag-selection's pointer
    /// rests past the pane's top or bottom edge, so the history keeps
    /// scrolling under it on a fixed beat with no further mouse report;
    /// None once the pointer is back inside or the button is up.
    pub next_drag_autoscroll: Option<std::time::Instant>,
    /// The session whose program holds the left button: it asked for the
    /// mouse (Claude Code's fullscreen renderer, vim `mouse=a`, htop), the
    /// press on the pane went to it, and the drag and release that follow
    /// go to it too — wherever the pointer has wandered by then. Cleared by
    /// the release; a press on anything else starts over.
    pub term_mouse_grab: Option<SessionRef>,
    /// Last left-click on the terminal pane (time + pane-relative cell), for
    /// double-click detection.
    pub last_term_click: Option<(std::time::Instant, (u16, u16))>,
    /// Last left-click on a session row (time + session), for double-click
    /// attach detection (a single click only selects the row).
    pub last_session_click: Option<(std::time::Instant, RowKey)>,
    /// Last left-click on the LAUNCHER VIEW's pane edge, for the
    /// double-click that snaps the edge to the middle of the body
    /// (`event_loop::launcher::center_pane`). The edge is one target
    /// however wide its grab zone, so the click carries no identity: two
    /// presses anywhere along it within the window make the double.
    pub last_pane_edge_click: Option<(std::time::Instant, ())>,
    /// URLs detected on the visible screen during the last draw; hit-tested
    /// on a ⌘, ⌥ or ^click and underlined by the renderer.
    pub term_links: Vec<crate::links::TermLink>,
    /// File paths detected on the visible screen during the last draw;
    /// a ⌘, ⌥ or ^click opens them in the editor modal.
    pub term_file_links: Vec<crate::links::FileLink>,
    /// File-list width of the diff modal, remembered across opens.
    pub diff_files_width: u16,
    /// The diff modal opens with its files as a directory tree, as `⌘B`
    /// last left a viewer (`diff_tree_view` in config.json, mirrored by
    /// `apply_config`).
    pub diff_tree: bool,
    /// Selected tab of the settings modal, remembered across opens.
    pub settings_tab: usize,
    /// Cursor row of the settings modal, one per tab, remembered across
    /// opens so switching tabs and coming back lands where you left.
    pub settings_selected: Vec<usize>,
    /// Where the settings cursor was parked when the overlay last closed:
    /// on the tab strip, or down in the list. True until the first visit
    /// puts it somewhere, so a fresh overlay opens with the strip focused
    /// and ←/→ immediately mean "walk the tabs".
    pub settings_on_tabs: bool,
    /// When the settings overlay was last closed. The remembered position
    /// above is only worth restoring while it's still fresh in the user's
    /// head: a reopen more than [`SETTINGS_MEMORY_TTL`] after this forgets
    /// it and starts over like a first open. `None` until the first close.
    pub settings_closed_at: Option<std::time::Instant>,
    /// Hotkeys as the panels dispatch them: `config.keymap()`, cached here
    /// because a keymap lookup happens on every single key press. The
    /// event loop refreshes it at startup and whenever a binding changes.
    pub keymap: crate::keymap::Keymap,
    /// Pointer shape the outer terminal should currently show (OSC 22).
    pub pointer_shape: PointerShape,
    /// Base64 payload waiting to go out as an OSC 52 clipboard request, set
    /// when the copy has to be delegated to the attached terminal (see
    /// `copy_and_flash`). The main loop writes and clears it.
    pub pending_clipboard: Option<String>,
    /// The DONE SOUND's finishes on their way to the speaker — settling,
    /// announced, and when anything last rang (see [`DoneSounds`]). The
    /// main loop rings what settles, once per frame however many rows
    /// settled together, and names each in a desktop notification while
    /// the terminal window is in the background.
    pub done_sounds: DoneSounds,
    /// Sessions that reached NEEDS FEEDBACK, or crashed mid-turn, since the
    /// last frame, one entry each: the main loop rings the FEEDBACK SOUND
    /// (`Config::feedback_sound`) once for the lot, posts a desktop
    /// notification per entry while the terminal window is in the
    /// background, and clears it. A session whose pane the user is locked
    /// into typing at, window focused, is never queued — that prompt, or
    /// that error, is already in front of them.
    pub pending_feedback: Vec<FeedbackAlert>,
    /// A sound the settings overlay just stepped `done_sound` or
    /// `feedback_sound` onto: the main loop plays it once, so picking one
    /// is hearing it.
    pub pending_sound_preview: Option<crate::config::Sound>,
    /// Whether the terminal window has focus, from the focus reports
    /// (mode 1004) `setup_terminal` asks for. True until the terminal says
    /// otherwise, so one that never reports (tmux without `focus-events`)
    /// keeps every desktop notification off rather than posting them while
    /// the user is looking.
    pub window_focused: bool,
    /// Body rect (everything above the footer) from the last draw; bounds
    /// splitter drags.
    pub body_area: Rect,
    /// Short machine hostname, shown at the far left of the footer.
    pub hostname: String,
    /// Running inside an ssh session (SSH_CONNECTION/SSH_TTY) — the footer
    /// colors the hostname as a remote warning.
    pub is_remote: bool,
    /// Active color theme. From config (`theme`); the event loop refreshes
    /// it when the setting changes.
    pub theme: crate::theme::Theme,
    /// Embedded editor modal (find-in-files Enter), above every overlay.
    pub editor: Option<crate::editor_term::EditorTerm>,
    /// MARKDOWN PAGE: a `.md` file opened to read, above every overlay
    /// and under the editor its Enter opens.
    pub page: Option<crate::markdown_view::MarkdownPage>,
    /// fresh's install, while the FLASH that names it is up: a file was to
    /// open and fresh isn't installed, and `i` runs this
    /// (`event_loop::offer_editor_install`).
    pub editor_install: Option<crate::install::Plan>,
    /// Where editor reader threads send output; the main loop installs it.
    pub editor_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::editor_term::EditorEvent>>,
    /// Stamp for the current editor spawn, so a closed editor's buffered
    /// events can't touch its successor.
    pub editor_generation: u64,
    /// Where a CLAUDE ACCOUNTS read off the loop lands, with whether a
    /// name changed; the main loop installs it
    /// (`claude_accounts::request_refresh`). None in a unit test, which
    /// reads inline.
    pub accounts_tx: Option<tokio::sync::mpsc::UnboundedSender<(crate::fetch::Ticket<()>, bool)>>,
    /// The accounts read running off the loop, at most one: a forced read
    /// asked while one runs is owed, and starts once it lands.
    pub accounts_reads: crate::fetch::Flights<()>,
    /// When the accounts were last asked to be read, so the slow beat
    /// spaces its reads out.
    pub accounts_polled: Option<std::time::Instant>,
    /// ACCOUNT USAGE: every linked account's last reading, kept here so
    /// the modal opens on it and the slow beat keeps it warm.
    pub usage: crate::usage::Usage,
    /// Changed-file count of the selected worktree's checkout (staged +
    /// unstaged + untracked), the worktree panel's bottom badge. Keyed by
    /// worktree so a selection change can't show another checkout's count;
    /// the inner `None` means the checkout wasn't readable. The event loop
    /// refreshes it on a slow poll and on a changed selection — off the
    /// loop, since a `git status` is tens of milliseconds on a big checkout
    /// and the badge is not worth a late frame; the selection guard means a
    /// pending answer shows nothing rather than another checkout's count.
    pub git_changes: Option<(WorktreeId, Option<usize>)>,
    /// The selected checkout's read running off the loop, at most one at a
    /// time, so a repaint can't stack `git status` processes. Its ticket
    /// carries when the read was asked, and a PULL or PUSH while it runs
    /// marks it owed (`event_loop::reread_checkouts`).
    pub git_reads: crate::fetch::Flights<WorktreeId>,
    /// The last changed-file count read in each checkout, and when: what
    /// the LAUNCHER VIEW's cards print beside their branch. Fed by the
    /// selected checkout's own reads (`git_changes`) and by a sweep that
    /// spends each poll tick on one other checkout the grid lists, the
    /// least recently read first. The count is None when git couldn't say.
    pub worktree_changes: HashMap<WorktreeId, (Option<usize>, std::time::Instant)>,
    /// The checkout the sweep is reading right now, at most one; the
    /// selected checkout's `git_reads` for every other one.
    pub sweep_reads: crate::fetch::Flights<WorktreeId>,
    /// When the read behind each checkout's counts (`worktree_changes`,
    /// `worktree_lines`, `worktree_ahead`) was asked. Two readers feed
    /// them — the selected checkout's and the sweep — so an answer asked
    /// before the one already taken is dropped (`fetch` rule 1): a slow
    /// read from before a PULL never puts its counts back over the read
    /// after it.
    pub worktree_read_at: HashMap<WorktreeId, std::time::Instant>,
    /// The lines added and removed in each checkout, read beside its
    /// changed-file count: what its band's rule prints after `*3`. Only a
    /// checkout with changed lines has an entry.
    pub worktree_lines: HashMap<WorktreeId, crate::git_diff::LineChanges>,
    /// How far each checkout's HEAD is from the base it was cut from, read
    /// beside its changed files: commits ahead, commits behind — a band's
    /// `⇡4 ⇣1`. Only a checkout that is either has an entry.
    pub worktree_ahead: HashMap<WorktreeId, (usize, usize)>,
    /// STACK STATUS: every docker compose stack on the machine, as the
    /// DAEMON last saw it — `None` while docker is out of reach (or before
    /// the first answer).
    pub stacks: Option<Vec<orion_core::compose::Stack>>,
    /// What `gh pr view` last said about each worktree's branch: `Some(pr)`
    /// when one exists, `None` when the lookup came back empty (no PR, no
    /// `gh`, no remote). A missing key means "not looked up yet" — briefly,
    /// for the selected project: its selected checkout is asked on every
    /// tick, and the others take turns on a sweep, so every row learns its
    /// merge without being visited. An empty answer is re-asked on a
    /// backing-off timer (`pr_recheck`), since the PR a session opens
    /// appears well after the first lookup; a found one keeps being
    /// re-asked on a beat, for its conversation and its state. At startup
    /// the found ones come back from the last run's cache (`pr_cache`), so
    /// the rows are painted before the first lookup answers.
    pub pull_requests: HashMap<WorktreeId, Option<PullRequest>>,
    /// When this client saw a checkout's pull request turn merged
    /// (`note_merge_landed`) — what times the row's ONE-SHOT SWEEP. Only a
    /// merge seen to happen is stamped: a row the cache hydrated as merged,
    /// or whose first answer ever says merged, landed some other day and
    /// paints solid purple from the first frame.
    pub merge_landed: HashMap<WorktreeId, std::time::Instant>,
    /// The ATTENTION WALK in progress (`.` / `,`): the session it last
    /// landed on and the ring as it stood when the walk began. Landing on
    /// an unread finish reads it, which re-sorts it behind the running
    /// sessions; walking on from the ring as it was keeps the next step
    /// from skipping them. A walk resumes from this only while the cursor
    /// is still where it landed and the same sessions are in the ring.
    pub attention_walk: Option<(AgentId, Vec<AgentId>)>,
    /// How far the user has read into each pull request's conversation,
    /// keyed by PR URL — the daemon's `pr_seen` rows, plus whatever this
    /// session has marked since. What's newer than the mark is what the
    /// row's unread badge counts.
    pub pr_seen: HashMap<String, String>,
    /// Worktrees with a lookup in flight, so a repaint can't stack a second
    /// `gh` process on the first. Each carries when it was asked, so its
    /// answer lands in `prs` under that stamp; a branch switch cancels
    /// the old branch's (`fetch::Flights::cancel`), whose answer is then
    /// dropped.
    pub pr_inflight: crate::fetch::Flights<WorktreeId>,
    /// Where every pull request stands — state, draft, conflicts, checks —
    /// from the newest answer that knew (`pr_store`). Every surface that
    /// draws or decides on a pull request's status reads it; the copies
    /// (`pull_requests`, `open_prs`, `pr_detail`) keep only what their own
    /// answer alone says: list order, meta, the body.
    pub prs: crate::pr_store::PrStore,
    /// When to ask `gh` about a worktree again, and the step that produced
    /// that deadline: a steady beat once its pull request is known (a quick
    /// one for the selected checkout, so the unread-comment count keeps up;
    /// a slow one for the rest, which only have to keep up with a merge), a
    /// doubling backoff while it isn't. Switching into a worktree drops its
    /// entry, so arriving somewhere always asks again promptly; so does its
    /// pull request leaving the project's open list.
    pub pr_recheck: HashMap<WorktreeId, (std::time::Instant, std::time::Duration)>,
    /// What `pull_request::list` last said about each project's open pull requests
    /// — the group at the bottom of the Worktrees panel. A missing key
    /// means "never asked"; only the selected project is ever asked, so a
    /// machine with thirty projects still costs one call per refresh.
    pub open_prs: HashMap<ProjectId, OpenPrs>,
    /// Projects with a list lookup in flight, so a repaint can't stack a
    /// second `gh` on the first — and whether one asked for meanwhile
    /// (`⌘R`, a merge from the modal) is owed when it lands.
    pub open_prs_inflight: crate::fetch::Flights<ProjectId>,
    /// Projects whose last list lookup came back with no answer — `gh`
    /// failed, timed out, or the checkout is gone. The list kept on screen
    /// is then the last one that worked, so the PULL REQUESTS MODAL says
    /// `couldn't refresh` rather than pass it off as current; the next
    /// answer that lands clears it.
    pub open_prs_failed: std::collections::HashSet<ProjectId>,
    /// Bodies and conversations of the pull requests the cursor has rested
    /// on, keyed by URL. A second API call on top of the list, so it is
    /// fetched only for the row actually being read (and, while the PULL
    /// REQUESTS MODAL is up, for your own pull requests ahead of it), and
    /// read again once it is older than `event_loop::PR_DETAIL_FRESH` —
    /// the description holds still, but the checks don't (`pr_detail_at`).
    pub pr_detail: HashMap<String, PrDetail>,
    /// Pull requests whose detail is in flight, and ones `gh` couldn't
    /// answer for — the pane says "couldn't reach gh" rather than spinning
    /// on a request that already came back empty. A page asked to be read
    /// again while its fetch runs is owed a fresh one when it lands.
    pub pr_detail_inflight: crate::fetch::Flights<String>,
    pub pr_detail_failed: std::collections::HashSet<String>,
    /// When `gh` last answered (or refused) each pull request's detail,
    /// this session. A page older than `event_loop::PR_DETAIL_FRESH` is
    /// read again the next time a cursor rests on it — its checks run on
    /// their own clock, not the list's — and a refusal that old is asked
    /// again rather than kept. No entry (a body put there by hand, or by
    /// the cache, which marks it `pr_detail_stale` instead) never ages.
    pub pr_detail_at: HashMap<String, std::time::Instant>,
    /// Debounced detail fetch: the pull request under the cursor and when
    /// its lookup is due. Re-armed on every move, so walking a list of a
    /// hundred rows fetches only the ones actually paused on.
    pub pending_pr_detail: Option<(PendingPrDetail, std::time::Instant)>,
    /// `r` on a worktree or pull-request row asked for the pull requests
    /// *now*: the event loop runs the two list lookups on its next turn
    /// instead of waiting for the git tick, then clears this.
    pub pr_refresh_requested: bool,
    /// Top visible line of the reading pane — the pull request under a
    /// cursor (`App::reading_url`) — and the pane's total
    /// line count as of the last draw (for clamping).
    pub pr_preview_scroll: u16,
    pub pr_preview_lines: usize,
    /// Where the pane's reader stands on the PULL REQUEST PAGE: its tab,
    /// and the row cursor of the tabs that list things.
    pub pr_tabs: crate::pr_preview::PrTabs,
    /// The file the DIFF VIEWER opens a pull request's diff at — the
    /// Changes tab's row acted on — by the pull request's URL. Taken by
    /// the next pull-request diff to open, whichever it is.
    pub pr_diff_at: Option<(String, String)>,
    /// Pull requests (by URL) with a `gh pr comment` still running. A
    /// second box sent on the same pull request waits for the first to
    /// land rather than racing it.
    pub pr_comment_inflight: std::collections::HashSet<String>,
    /// Comment text a refused post handed back while another modal was
    /// up, by PR URL: the next COMMENT BOX opened on that pull request
    /// starts from it, so the refusal cost nothing typed.
    pub pr_comment_drafts: HashMap<String, String>,
    /// Where a finished `gh pr comment` lands (`PrCommentAnswer`);
    /// installed by the loop at startup (the `vim_tx` precedent), so key
    /// handlers can start the post without the loop's channels in hand.
    pub pr_comment_tx: Option<tokio::sync::mpsc::UnboundedSender<PrCommentAnswer>>,
    /// Where the PULL REQUESTS MODAL's forms' git and `gh` land — a new
    /// pull request, a merge (`pr_actions::Answer`); None in unit tests,
    /// which send nothing.
    pub pr_actions_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::pr_actions::Answer>>,
    /// The WEEK IN REVIEW's fetches and the review being written: on the
    /// app, not its modal, so closing the modal loses neither.
    pub week_review: crate::week_review::State,
    /// MICROSOFT 365: the sign-in, and the calendar behind the footer's
    /// next meeting.
    pub microsoft: crate::microsoft::State,
    /// Each project's **What it is** being written (`projects`).
    pub project_about: crate::projects::State,
    /// NEEDS YOU: Done and Snooze, what its poll last heard, and crashed
    /// sessions' exit codes (`crate::needs_you`).
    pub needs_you: crate::needs_you::State,
    /// Where the PR REVIEW's reads and writes land.
    pub pr_review: crate::pr_review::State,
    /// The on-disk memory of every pull-request answer (`pr_cache`), when
    /// this instance has one: the main loop installs the real one at
    /// startup and hydrates from it; the unit tests leave it `None`, so no
    /// test touches the real user's cache. Written whenever
    /// `pr_cache_dirty` says something in it changed — at most once per
    /// GIT POLL, plus once on quit.
    pub pr_cache: Option<crate::pr_cache::PrCache>,
    pub pr_cache_dirty: bool,
    /// AUTOFIX: the ledger, the watched pull requests and the queued asks
    /// (`crate::automations::pr_fixer`).
    pub autofix: crate::automations::pr_fixer::State,
    /// AUTOMATIONS: their runners, their settings mirrored, and the
    /// creates still on their way (`crate::automations`).
    pub automations: crate::automations::Engine,
    /// Where a file dropped onto a prompt box bound for an agent is copied
    /// before macOS deletes it (`dropped_files`), and where `^V` keeps the
    /// clipboard's image (`clipboard_image`): the main loop installs the
    /// DATA DIR's `attachments/` at startup; the unit tests leave it
    /// `None`, so a paste there is never staged into the real user's dir.
    pub attachments_dir: Option<std::path::PathBuf>,
    /// Bodies in `pr_detail` that came from the cache rather than from
    /// `gh`. The pane shows them at once; resting the cursor on their row
    /// fetches a fresh copy over the top, as it would fetch a missing one,
    /// and the answer takes the URL out of here.
    pub pr_detail_stale: std::collections::HashSet<String>,
    /// Bodies in `pr_detail` read since the PR CACHE last wrote: the ones
    /// its next flush writes, each to its own file (`pr_cache::snapshot`).
    pub pr_detail_unsaved: std::collections::HashSet<String>,
    /// Which files each conflicting pull request conflicts in, as the
    /// repo answered for its head, and the CONFLICT RESOLVER's merge and
    /// commit on their way (`crate::conflicts`).
    pub conflicts: crate::conflicts::Known,
    /// LINEAR VIEW rows, the lists in flight (one per project, a ⌘R
    /// asked meanwhile owed), failed asks, and the answer channel.
    pub linear: std::collections::HashMap<ProjectId, crate::linear::LinearList>,
    pub linear_flights: crate::fetch::Flights<ProjectId>,
    pub linear_failed: std::collections::HashSet<ProjectId>,
    pub linear_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::linear::LinearAnswer>>,
    /// What orion did to issues that a list asked before it may not know
    /// of yet — `⌘S` moves and attached pull requests — laid over every
    /// such list as it lands.
    pub linear_edits: crate::linear::LocalEdits,
    /// Branches a ⌘L launch cut, so a pull request on one can be attached.
    pub linear_links: crate::linear::LinkStore,
    /// The worktree whose linked issues **Copy linked Linear issues** will
    /// copy once its project's list lands — asked for because orion had
    /// not read it yet.
    pub linear_copy: Option<WorktreeId>,
    /// The reads of the issues' comments, as the LINEAR VIEW's cursor
    /// rests on each.
    pub linear_comments: crate::linear::CommentReads,
    /// Each project's lists asked as it is selected.
    pub linear_prefetch: crate::linear::Prefetch,
    /// The on-disk copy of the lists (`linear_cache`), when this app keeps
    /// one — the main loop's does, a unit test's never — and whether a
    /// list landed since it was last written.
    pub linear_cache: Option<crate::linear_cache::LinearCache>,
    pub linear_cache_dirty: bool,
    /// Each project's TODOS list, by its checkout, read when the modal
    /// first opens on it and kept for the session — saved on every change
    /// (`todos::store`).
    pub todos: HashMap<std::path::PathBuf, crate::todos::TodoFile>,
    /// Creates sent at a TODO, by request id: the Ack writes the session
    /// onto the item, an Error forgets it.
    pub todo_pending: HashMap<u64, crate::todos::TodoRef>,
    /// What `⌘X`/`⌘C` last took in the TODOS MODAL: pasted back into it,
    /// the items themselves go in.
    pub todo_clip: Option<crate::todos::view::TodoClip>,
    /// The last **Linear key** test (Settings → Projects): the checkout
    /// whose key it tried, where that key was found, and what Linear said
    /// — what the row says while that is still the key on show.
    pub linear_test: Option<(
        std::path::PathBuf,
        Option<crate::linear::KeySource>,
        crate::linear::LinearTest,
    )>,
    /// BACKGROUND READS for the worktree views (`view_jobs`): the DIFF
    /// VIEWER, the FILE FINDER, its grep view and the TREE BROWSER are
    /// handed a clone when they open, and their git and disk reads land
    /// through the main loop instead of holding it. None with no loop
    /// running (unit tests), where those views read inline.
    pub view_jobs: Option<crate::view_jobs::Jobs>,
    /// A `g` on a checkout the changed-files badge called clean: the ticket
    /// of the `git status` checking that, and the checkout (path, branch)
    /// to open the DIFF VIEWER on if git disagrees.
    pub diff_probe: Option<(u64, PathBuf, String)>,
    /// The changed files the badge's last `git status` listed, and the
    /// checkout they are in (`event_loop::keep_changed_files`): what `g`
    /// opens the DIFF VIEWER on while its own `git status` runs. One
    /// checkout's worth, capped, replaced by every poll.
    pub changed_files: Option<(WorktreeId, Vec<crate::git_diff::DiffFile>)>,
    /// Rows deleted here ahead of the DAEMON's answer
    /// (`event_loop::optimistic`), each with its latest version: an upsert
    /// or a status flip of one of them is kept here rather than shown, so
    /// the row stays down — and a refusal puts back the row as the DAEMON
    /// last had it, not as it was on the keypress.
    pub deleting: HashMap<orion_core::EntityId, orion_core::Entity>,
    /// The WORKTREE PICKER's branch listings: their answer channel, cache
    /// and fetch throttle (`branch_list`).
    pub branch_list: crate::branch_list::Shared,
    /// The PULLs and PUSHes running, and where their answers go
    /// (`crate::git_sync`).
    pub git_sync: crate::git_sync::Shared,
    /// **Clean unused worktrees**' check and where it lands
    /// (`crate::clean_worktrees`).
    pub clean_worktrees: crate::clean_worktrees::Shared,
    /// The OPEN REPO modal's scan and where it lands (`crate::open_repo`).
    pub open_repo: crate::open_repo::Shared,
    /// CLONE FROM GITHUB's `gh` listing and clones (`crate::clone_repo`).
    pub clone_repo: crate::clone_repo::Shared,
    /// BASE SYNC: each project's root fetched, and fast-forwarded on its
    /// base branch, on the **Fetch base branch** beat (`crate::base_sync`).
    pub base_sync: crate::base_sync::Shared,
    /// Latest daemon metrics reading (daemon + per-session process trees),
    /// for the footer's memory/session readout. Refreshed on a slow poll.
    pub last_metrics: Option<orion_core::MetricsSnapshot>,
    /// This TUI process's own RSS, sampled alongside each metrics request
    /// (the daemon can't see us).
    pub client_rss_bytes: u64,
    /// Launch instant; the first-run splash animation and the status-sweep
    /// text animation are pure functions of time elapsed since this.
    pub splash_epoch: std::time::Instant,
    /// The MERGE CONFETTI in the air, drawn over everything until it lands
    /// (`confetti`); the event loop ticks while it is here.
    pub confetti: Option<crate::confetti::Confetti>,
    /// The last frame drew the empty GRID's welcome, orion and all
    /// (`ui::launcher_view`). Cleared at the top of every `ui::draw` and
    /// set again by the welcome itself, so between frames it says what is
    /// on screen — which is what keeps the sky ticking
    /// ([`App::welcome_active`]).
    pub welcome_on_screen: bool,
    /// The `animations` setting: master switch for the status-text sweep
    /// and the splash's motion (off = fewer repaints). Mirrors the config,
    /// refreshed at startup and when the settings overlay applies a change.
    pub animations: bool,
    /// ROUND CAPS: the terminal draws Powerline's half-circle glyphs
    /// itself, so a short filled piece — a chip, a tab, a keycap — can
    /// end in a round cap instead of a square edge ([`crate::ui::round_caps`]).
    /// Ghostty does, Nerd Font or not, and Orion.app is Ghostty; anywhere
    /// else the pieces stay square rather than risk a tofu box.
    pub round_caps: bool,
    /// The ROWS MEMO, armed by the frame and by [`App::reading_url`].
    pub rows_memo: RowsMemo,
    /// The WAKE ORDER the live checkouts stand in ([`App::worktree_order`]).
    wake_order: std::cell::RefCell<WakeOrder>,
}

/// The order the LIVE checkouts stand in: the order they went live in, as
/// this TUI saw it. A checkout waking from DORMANT goes to the bottom of
/// the live ones, right above the divider it came up from, so nothing
/// above it moves. The checkouts live when a project's list is first
/// drawn take their places in creation order.
#[derive(Default)]
struct WakeOrder {
    /// Each live checkout's place: lower stands higher.
    seq: HashMap<WorktreeId, u64>,
    next: u64,
}

impl WakeOrder {
    /// Bring the places up to date with `rows`, one project's checkouts in
    /// tree order, each with whether it sorts live: a newly live one takes
    /// the next place, a dormant one gives its place up.
    fn note<'a>(&mut self, rows: impl Iterator<Item = (&'a WorktreeId, bool)>) {
        for (id, live) in rows {
            if !live {
                self.seq.remove(id);
            } else if !self.seq.contains_key(id) {
                self.seq.insert(id.clone(), self.next);
                self.next += 1;
            }
        }
    }

    fn place(&self, id: &WorktreeId) -> u64 {
        self.seq.get(id).copied().unwrap_or(u64::MAX)
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            tree: Tree::default(),
            // The GRID is where FOCUS lives; the PANE under it is the only
            // other place it can go.
            focus: Focus::Sessions,
            project_cursor: PanelCursor::default(),
            worktree_cursor: PanelCursor::default(),
            worktree_hold: None,
            session_cursor: PanelCursor::default(),
            cursor_moves: 0,
            follow_up: None,
            term: None,
            term_cache: Vec::new(),
            term_locked: false,
            conn: ConnState::Disconnected,
            hits: Vec::new(),
            term_area: Rect::default(),
            host_cursor: None,
            dirty: true,
            should_quit: false,
            pending_ssh: None,
            restart: false,
            splash_upgrade_asked: false,
            upgrade: false,
            flash: None,
            update_available: None,
            update_tx: None,
            spotify: None,
            spotify_in_flight: 0,
            spotify_seq: 0,
            spotify_denied: false,
            spotify_enabled: false,
            away: false,
            away_by_hand: false,
            slack_status: Default::default(),
            outbox: Vec::new(),
            spotify_tx: None,
            edge_tap: None,
            release_watch: None,
            overlay: None,
            show_archived: false,
            archived_open: Default::default(),
            open_prs_collapsed: false,
            collapsed: false,
            hide_draft_prs: false,
            launcher_unaimed: false,
            marked: Vec::new(),
            mark_anchor: None,
            launcher_pane_h: None,
            launcher_pane_w: None,
            launcher_pane_hidden: false,
            launcher_scroll: 0,
            launcher_scroll_held: false,
            launcher_scroll_on: None,
            launcher_reveal: false,
            launcher_pane_drag: None,
            launcher_tab_drag: None,
            hover_launcher_pane: false,
            hover_crumb: None,
            launcher_tabs: Vec::new(),
            projects_closed: false,
            home: false,
            launcher_tab_cursor: None,
            launcher_tabs_more: Vec::new(),
            launch_repo: None,
            launcher_body: Rect::default(),
            key_combo: None,
            next_req_id: 1,
            pending: HashMap::new(),
            left_behind: std::collections::HashSet::new(),
            select_when_seen: None,
            just_launched: None,
            select_project_when_seen: None,
            select_worktree_when_seen: None,
            pr_checkout_waiting: None,
            last_worktree_for_project: HashMap::new(),
            last_session_for_worktree: HashMap::new(),
            pending_prewarm: None,
            parked_pr_prompt: None,
            quick_draft: Default::default(),
            saved_draft: None,
            pending_attach: None,
            attached_sref: None,
            term_selection: None,
            next_drag_autoscroll: None,
            term_mouse_grab: None,
            last_term_click: None,
            last_session_click: None,
            last_pane_edge_click: None,
            term_links: Vec::new(),
            term_file_links: Vec::new(),
            diff_files_width: DEFAULT_DIFF_FILES_W,
            diff_tree: false,
            settings_tab: 0,
            settings_selected: vec![0; crate::config::tab_count()],
            settings_on_tabs: true,
            settings_closed_at: None,
            keymap: crate::keymap::Keymap::default(),
            pointer_shape: PointerShape::default(),
            pending_clipboard: None,
            done_sounds: DoneSounds::default(),
            pending_feedback: Vec::new(),
            pending_sound_preview: None,
            window_focused: true,
            body_area: Rect::default(),
            hostname: orion_core::host::hostname(),
            is_remote: orion_core::host::is_remote_session(),
            theme: crate::theme::Theme::default(),
            editor: None,
            page: None,
            editor_install: None,
            editor_tx: None,
            editor_generation: 0,
            accounts_tx: None,
            accounts_reads: Default::default(),
            accounts_polled: None,
            usage: crate::usage::Usage::default(),
            git_changes: None,
            git_reads: Default::default(),
            worktree_changes: HashMap::new(),
            sweep_reads: Default::default(),
            worktree_read_at: HashMap::new(),
            worktree_lines: HashMap::new(),
            worktree_ahead: HashMap::new(),
            stacks: None,
            pull_requests: HashMap::new(),
            merge_landed: HashMap::new(),
            attention_walk: None,
            pr_seen: HashMap::new(),
            pr_inflight: Default::default(),
            prs: Default::default(),
            pr_recheck: HashMap::new(),
            open_prs: HashMap::new(),
            open_prs_inflight: Default::default(),
            open_prs_failed: std::collections::HashSet::new(),
            pr_detail: HashMap::new(),
            pr_detail_inflight: Default::default(),
            pr_detail_failed: std::collections::HashSet::new(),
            pr_detail_at: HashMap::new(),
            pending_pr_detail: None,
            pr_refresh_requested: false,
            pr_preview_scroll: 0,
            pr_preview_lines: 0,
            pr_tabs: Default::default(),
            pr_diff_at: None,
            pr_comment_inflight: std::collections::HashSet::new(),
            pr_comment_drafts: HashMap::new(),
            pr_comment_tx: None,
            pr_actions_tx: None,
            week_review: Default::default(),
            microsoft: Default::default(),
            project_about: Default::default(),
            needs_you: Default::default(),
            pr_review: Default::default(),
            pr_cache: None,
            pr_cache_dirty: false,
            autofix: crate::automations::pr_fixer::State::default(),
            automations: Default::default(),
            attachments_dir: None,
            pr_detail_stale: std::collections::HashSet::new(),
            pr_detail_unsaved: std::collections::HashSet::new(),
            conflicts: crate::conflicts::Known::default(),
            linear: HashMap::new(),
            linear_flights: crate::fetch::Flights::default(),
            linear_failed: std::collections::HashSet::new(),
            linear_tx: None,
            linear_edits: crate::linear::LocalEdits::default(),
            linear_links: crate::linear::LinkStore::default(),
            linear_copy: None,
            linear_comments: Default::default(),
            linear_prefetch: Default::default(),
            linear_cache: None,
            linear_cache_dirty: false,
            todos: HashMap::new(),
            todo_pending: HashMap::new(),
            todo_clip: None,
            linear_test: None,
            view_jobs: None,
            diff_probe: None,
            changed_files: None,
            deleting: HashMap::new(),
            branch_list: Default::default(),
            git_sync: Default::default(),
            clean_worktrees: Default::default(),
            open_repo: Default::default(),
            clone_repo: Default::default(),
            base_sync: Default::default(),
            last_metrics: None,
            client_rss_bytes: 0,
            splash_epoch: std::time::Instant::now(),
            confetti: None,
            welcome_on_screen: false,
            animations: true,
            round_caps: false,
            rows_memo: RowsMemo::default(),
            wake_order: std::cell::RefCell::default(),
        }
    }

    /// Remembered cursor row for a settings tab, clamped to what that tab
    /// currently holds.
    pub fn settings_row(&self, tab: usize) -> usize {
        self.settings_selected
            .get(tab)
            .copied()
            .unwrap_or(0)
            .min(self.settings_len(tab).saturating_sub(1))
    }

    /// A settings tab's rows, the Project tab's a block per project.
    pub fn settings_rows(&self, tab: usize) -> Vec<crate::config::SettingsRow> {
        crate::config::settings_rows(tab, self.tree.projects.len())
    }

    /// How many selectable rows a settings tab holds.
    pub fn settings_len(&self, tab: usize) -> usize {
        crate::config::tab_len(tab, self.tree.projects.len())
    }

    /// The project a Project tab row edits — in tree order, which a
    /// project's recent use never reshuffles under the cursor.
    pub fn settings_project(&self, tab: usize, index: usize) -> Option<&Project> {
        self.tree
            .projects
            .get(crate::config::project_at(tab, index)?)
    }

    /// Record where the settings cursor is parked, so the next open lands
    /// in the same place.
    pub fn remember_settings_focus(&mut self, on_tabs: bool) {
        self.settings_on_tabs = on_tabs;
    }

    /// Stamp the moment the settings overlay went away, starting the
    /// [`SETTINGS_MEMORY_TTL`] clock on the remembered position.
    pub fn note_settings_closed(&mut self) {
        self.settings_closed_at = Some(std::time::Instant::now());
    }

    /// The remembered settings position has gone stale: the overlay was
    /// closed more than [`SETTINGS_MEMORY_TTL`] ago. Never true before the
    /// first close — there's nothing to forget yet.
    pub fn settings_memory_expired(&self) -> bool {
        self.settings_closed_at
            .is_some_and(|closed| closed.elapsed() >= SETTINGS_MEMORY_TTL)
    }

    /// Drop the remembered settings position so the next open looks like
    /// the very first one: first tab, top row, cursor on the tab strip.
    pub fn forget_settings_focus(&mut self) {
        self.settings_tab = 0;
        self.settings_selected = vec![0; crate::config::tab_count()];
        self.settings_on_tabs = true;
        self.settings_closed_at = None;
    }

    pub fn remember_settings_row(&mut self, tab: usize, row: usize) {
        if self.settings_selected.len() < crate::config::tab_count() {
            self.settings_selected.resize(crate::config::tab_count(), 0);
        }
        if let Some(slot) = self.settings_selected.get_mut(tab) {
            *slot = row;
        }
    }

    /// The folder name of [`App::launch_repo`] while it is not a project
    /// yet — what the SPLASH's "Enter: open …" names. None once it is one,
    /// or when orion was started outside a repository.
    pub fn launch_repo_name(&self) -> Option<String> {
        let repo = self.launch_repo.as_ref()?;
        if self.tree.project_at_path(repo).is_some() {
            return None;
        }
        repo.file_name().map(|n| n.to_string_lossy().into_owned())
    }

    /// The release the SPLASH's Enter installs rather than beginning: a
    /// newer one that is known and has not been offered yet this run.
    pub fn splash_upgrade(&self) -> Option<&str> {
        if self.splash_upgrade_asked || !self.splash_showing() || self.editor.is_some() {
            return None;
        }
        self.update_available.as_deref()
    }

    /// The LAUNCHER VIEW is what the body draws — which is orion's only
    /// view — once this machine knows a project and one is open. With none
    /// at all (a first run), or every tab closed ([`App::projects_closed`]),
    /// the splash's "open a project" comes first.
    pub fn launcher_active(&self) -> bool {
        self.tree.has_projects() && !self.projects_closed && !self.home
    }

    /// A project is being opened: out of the all-tabs-closed SPLASH — or
    /// HOME — and back onto the grid. Run by every move that selects a
    /// project, so neither flag outlives the first project landed on.
    pub fn reopen_projects(&mut self) {
        if self.projects_closed || self.home {
            self.projects_closed = false;
            self.home = false;
            self.dirty = true;
        }
    }

    /// Keep the PROJECT TABS true to the tree: a tab whose project is gone
    /// goes, and the project the grid is on gets one at the right end if
    /// it has none — however it got there, whether a tab, the `+`
    /// dropdown, a `/` jump, a folder just opened or the restore at boot.
    /// A project already open keeps its place: neither looking at a tab
    /// nor working in it moves it, so `⌘N` and the tab under the pointer
    /// mean the same project from one moment to the next. Run by the
    /// view's draw, as
    /// [`App::settle_launcher_focus`] is, and by the tab keys before they
    /// read the list.
    pub fn settle_project_tabs(&mut self) {
        let projects = &self.tree.projects;
        self.launcher_tabs
            .retain(|id| projects.iter().any(|p| &p.id == id));
        // The header's cursor needs a tab to be on and the keys to be on
        // the grid: a tab closed or a project dropped under it, or the
        // pane taking the keys, hands them back to the cards.
        if self.launcher_tab_cursor.as_ref().is_some_and(|id| {
            !self.launcher_tabs.contains(id) || self.focus == Focus::Terminal || self.collapsed
        }) {
            self.launcher_tab_cursor = None;
            self.dirty = true;
        }
        // Every tab closed: the selection stays where it was under the
        // splash, and gets no tab back until a project is opened.
        if self.projects_closed {
            return;
        }
        let Some(id) = self.selected_project().map(|p| p.id.clone()) else {
            return;
        };
        if !self.launcher_tabs.contains(&id) {
            self.launcher_tabs.push(id);
            self.dirty = true;
        }
    }

    /// Something was just done in `project` — a session launched, a
    /// checkout cut, a turn sent, a key typed into one of its sessions —
    /// so it has a PROJECT TAB: one with none gets one at the right end
    /// (a launch fired into a project from another's grid is work in it
    /// too). A tab already open stays exactly where it is; the header's
    /// order is the user's ([`App::launcher_tabs`]). Cheap enough for
    /// every keystroke typed at an agent: an open tab is one scan of a
    /// short list.
    pub fn ensure_project_tab(&mut self, project: &ProjectId) {
        if self.launcher_tabs.contains(project)
            || !self.tree.projects.iter().any(|p| &p.id == project)
        {
            return;
        }
        self.reopen_projects();
        self.launcher_tabs.push(project.clone());
        self.dirty = true;
    }

    /// Move the PROJECT TAB `moving` into the place `onto` holds, the
    /// tabs between sliding over by one — a drag along the header
    /// (`event_loop::launcher::drag_tab`). False, with nothing changed,
    /// when either has no tab or they are the same one.
    pub fn move_project_tab(&mut self, moving: &ProjectId, onto: &ProjectId) -> bool {
        let tabs = &mut self.launcher_tabs;
        let (Some(from), Some(to)) = (
            tabs.iter().position(|id| id == moving),
            tabs.iter().position(|id| id == onto),
        ) else {
            return false;
        };
        if from == to {
            return false;
        }
        let id = tabs.remove(from);
        tabs.insert(to, id);
        self.dirty = true;
        true
    }

    /// The project `session` runs in, None for a row this client has not
    /// seen.
    pub fn project_of_session(&self, session: &SessionRef) -> Option<&ProjectId> {
        let worktree = match session {
            SessionRef::Agent(id) => self
                .tree
                .agents
                .iter()
                .find(|a| &a.id == id)
                .map(|a| &a.worktree_id),
            SessionRef::Terminal(id) => self
                .tree
                .terminals
                .iter()
                .find(|t| &t.id == id)
                .map(|t| &t.worktree_id),
        }?;
        self.tree
            .worktrees
            .iter()
            .find(|w| &w.id == worktree)
            .map(|w| &w.project_id)
    }

    /// The GRID is what the body is showing: the LAUNCHER VIEW is on and
    /// no session has been opened full-screen over it (`collapsed`, which
    /// `ui::draw` hands to the pane before it ever reaches the view).
    pub fn launcher_grid(&self) -> bool {
        self.launcher_active() && !self.collapsed
    }

    /// Take the keyboard back from the session in the PANE.
    ///
    /// Every UNASKED drop goes through here: a redraw that finds the pane
    /// gone ([`App::settle_launcher_focus`]), a row archived, deleted or
    /// reaped out from under the pane showing it, a stand-in replaced by
    /// the session it stood for.
    ///
    /// The guard lives here rather than at the call sites: they run from
    /// draws and from daemon events and cannot know whether the lock was
    /// held, and a redraw on every frame would be waste. Only a lock that
    /// was actually HELD marks the frame dirty.
    pub fn release_terminal(&mut self) {
        if self.term_locked {
            self.term_locked = false;
            self.dirty = true;
        }
    }

    /// FOCUS as the LAUNCHER VIEW's GRID has it: the cards, or the PANE
    /// under them while something is in it — a click into the pane types
    /// into that session where it stands, and the hatch (`^q`) comes back
    /// out to the cards. Every other focus — a restored UI state parked on
    /// a panel this view doesn't draw — lands on the cards. Run by the
    /// view's draw.
    pub fn settle_launcher_focus(&mut self) {
        // Only the SESSIONS level draws a pane (`launcher::split`), only
        // while it is unfolded (`^~`) and only with a card wearing the
        // cursor for it to read: with nothing under the grid there is
        // nowhere for focus to rest, so it comes back to the cards
        // rather than sitting on a pane that is no longer on screen.
        if self.focus == Focus::Terminal
            && (self.term.is_some() || self.pane_reads_page())
            && !self.launcher_pane_hidden
            && self.launcher_aimed()
        {
            return;
        }
        self.focus = Focus::Sessions;
        // The lock goes with the pane however FOCUS got off it: a click
        // on the air between the cards sets FOCUS itself
        // (`event_loop`'s `PanelBg` arm) before letting the card go, and
        // the input lock left behind would have gone on eating keys with
        // no pane on screen to type into. `release_terminal` is a no-op
        // on a lock that was never held.
        self.release_terminal();
    }

    /// The splash is what the body is showing: no project on this machine
    /// yet (first run), and no session full-screen over it. True whether
    /// it's animating or drawn as a still frame, so the footer can key its
    /// hints off it.
    pub fn splash_showing(&self) -> bool {
        !self.collapsed && !self.launcher_active()
    }

    /// The animated splash is on screen and should be ticking: nothing in
    /// the tree yet (first run), no session full-screen, no editor modal
    /// covering the body, animations enabled (off, the splash still draws
    /// — as a still frame).
    pub fn splash_active(&self) -> bool {
        self.animations && self.splash_showing() && self.editor.is_none()
    }

    /// The empty GRID's welcome is on screen and its orion should be
    /// ticking — the same cadence and the same switch as the splash's.
    /// Off, the welcome still draws, as a still frame.
    pub fn welcome_active(&self) -> bool {
        self.animations && self.welcome_on_screen && self.editor.is_none()
    }

    /// Something on screen moves, so the clock should be ticking: a
    /// running session's WORKING SPINNER, a finish nobody has read (the
    /// UNREAD SHIMMER, on its row and its project's tab, until it is read),
    /// or a ONE-SHOT SWEEP still inside its window — a session that just
    /// started needing you or crashed, a checkout whose pull request was
    /// just seen to merge. Any agent in one of those states surfaces
    /// somewhere — its own row, a tab, the jump list — unless the panels
    /// are hidden (collapsed, editor modal, splash) or animations are
    /// switched off. A merged checkout only shows while its project is
    /// selected, so only those keep the clock running; a FLASH saying what
    /// it waits on turns the footer's spinner, and a Spotify track playing
    /// sweeps the footer's readout. The one-shots run out on the
    /// clock, so an idle app with everything read repaints nothing.
    pub fn status_anim_active(&self) -> bool {
        // A CONFLICT RESOLVER at work has a log to show as it grows, spinner
        // or no spinner.
        if self.editor.is_none()
            && matches!(&self.overlay, Some(Overlay::Conflicts(v)) if v.running())
        {
            return true;
        }
        // A stop's countdown ticks, animations or none.
        if self
            .tree
            .terminals
            .iter()
            .any(|t| t.run.stopping_until.is_some())
        {
            return true;
        }
        let now = now_ms();
        self.animations
            && !self.collapsed
            && self.editor.is_none()
            && !self.splash_active()
            && (self.tree.agents.iter().any(|a| {
                !a.archived && (self.spins(a) || self.shows_unread(a) || fresh_alarm(a, now))
            }) || self.visible_worktrees().iter().any(|w| {
                self.merge_sweeping(&w.id)
                    || (self.any_stack_running() && self.stack_running(&w.id))
            }) || self
                .flash
                .as_ref()
                .is_some_and(|f| f.kind == crate::flash::FlashKind::Working)
                || self.update_available.is_some()
                || self.spotify.as_ref().is_some_and(|np| np.playing)
                // A WEEK IN REVIEW's `Show` lines sweep while it is read.
                || matches!(&self.overlay, Some(Overlay::WeekReview(v)) if v.shimmers()))
    }

    /// Whether `agent`'s dot is the turning WORKING SPINNER: a session
    /// still starting, or one running with its PTY alive. A row left
    /// running by a process that is gone is drawn COLD and still.
    pub fn spins(&self, agent: &Agent) -> bool {
        self.is_placeholder_agent(&agent.id)
            || (agent.status == AgentStatus::Running && !drawn_cold(agent))
    }

    /// Whether an unread finish shimmers anywhere: on the grid's own rows
    /// and on an open PROJECT TAB (the lit one is always open), in the
    /// jump list while it is up, and on every row of the old panels. One
    /// in a project whose tab is closed shows nowhere, and keeps no clock
    /// running.
    fn shows_unread(&self, agent: &Agent) -> bool {
        unread_finish(agent)
            && (!self.launcher_active()
                || matches!(self.overlay, Some(Overlay::Palette(_)))
                || self.tree.worktrees.iter().any(|w| {
                    w.id == agent.worktree_id && self.launcher_tabs.contains(&w.project_id)
                }))
    }

    /// Whether a desktop notification reaches anyone: the terminal window
    /// is in the background, and this is the machine the user sits at (not
    /// over `orion ssh`, where the desktop is the wrong one).
    pub fn may_notify_desktop(&self) -> bool {
        !self.window_focused && !self.is_remote
    }

    /// STACK STATUS: the compose stack started in `worktree`'s checkout,
    /// if any — a nested checkout's stack is its own, not its parent's.
    pub fn stack_of(&self, worktree: &WorktreeId) -> Option<&orion_core::compose::Stack> {
        let stacks = self.stacks.as_deref().filter(|s| !s.is_empty())?;
        let wt = self.tree.worktrees.iter().find(|w| &w.id == worktree)?;
        orion_core::compose::stack_in(stacks, &wt.path, &self.checkout_paths())
    }

    /// Every checkout's directory, for [`orion_core::compose::owner_of`].
    pub fn checkout_paths(&self) -> Vec<&std::path::Path> {
        self.tree
            .worktrees
            .iter()
            .map(|w| w.path.as_path())
            .collect()
    }

    /// Whether any stack on the machine is up — the cheap check before
    /// asking which band it is on.
    fn any_stack_running(&self) -> bool {
        self.stacks.iter().flatten().any(|s| s.running > 0)
    }

    /// Whether `worktree`'s stack is up: its band's `⬡` runs the trail.
    pub fn stack_running(&self, worktree: &WorktreeId) -> bool {
        self.stack_of(worktree)
            .is_some_and(|s| s.state() == orion_core::compose::StackState::Running)
    }

    /// Whether `worktree`'s MERGED BAND is still inside its ONE-SHOT SWEEP.
    pub fn merge_sweeping(&self, worktree: &WorktreeId) -> bool {
        self.worktree_wears_merge(worktree) && self.merge_is_fresh(worktree)
    }

    /// This client just saw `worktree`'s pull request turn merged: start
    /// the row's ONE-SHOT SWEEP, and tick the todos its agents were sent
    /// at. Stamps that have run out go as new ones arrive, so the map holds
    /// a few seconds' worth at most.
    pub fn note_merge_landed(&mut self, worktree: WorktreeId) {
        crate::todos::merge_landed(self, &worktree);
        self.merge_landed
            .retain(|_, at| at.elapsed() < ONE_SHOT_SWEEP);
        self.merge_landed
            .insert(worktree, std::time::Instant::now());
    }

    /// Whether the checkout's merge was seen to land within the last
    /// `ONE_SHOT_SWEEP` — the purple row still sweeps; after, it is solid.
    pub fn merge_is_fresh(&self, worktree_id: &WorktreeId) -> bool {
        self.merge_landed
            .get(worktree_id)
            .is_some_and(|at| at.elapsed() < ONE_SHOT_SWEEP)
    }

    /// Whether the session's change into needing you, or its crash, still
    /// sweeps ([`fresh_alarm`]).
    pub fn agent_fresh_alarm(&self, agent: &Agent) -> bool {
        fresh_alarm(agent, now_ms())
    }

    /// The WORKING SPINNER's frame right now, or `None` with the
    /// animations off (a still `◐`).
    pub fn spin_phase(&self) -> Option<usize> {
        self.animations.then(|| spin_step(self.sweep_phase()))
    }

    /// Frame counter for the status-sweep text animation — a pure function
    /// of elapsed time (same model as the splash), so a missed tick just
    /// skips ahead instead of stuttering.
    pub fn sweep_phase(&self) -> usize {
        (self.splash_epoch.elapsed().as_millis() / SWEEP_FRAME.as_millis()) as usize
    }

    /// Is the GRID aimed at a card — is there something for the PANE
    /// along the bottom to read? The selection itself is never let go of
    /// (see [`App::launcher_unaimed`]); this is whether the view is
    /// pointed at it, which `event_loop::launcher::take_aim` sets and
    /// `clear_aim` drops, and which `ui::launcher_view`'s `wearing`
    /// draws the cursor's card by.
    pub fn launcher_aimed(&self) -> bool {
        !self.launcher_unaimed
    }

    /// The whole GRID laid out the way this frame draws it: every band's
    /// entries stacked under its rule (`launcher::panel_layout`). The
    /// draw, the wheel and the keys all read this one, so what `j`/`k`
    /// walk is what is on screen.
    pub fn panel_layout(&self, bands: &[crate::launcher::Band]) -> crate::launcher::PanelLayout {
        crate::launcher::panel_layout(self.body_area, bands)
    }

    /// The band the keys walk, and its lines: whichever band the cursor is
    /// on, every band being a column of entries. None with the cursor on
    /// no band.
    pub fn walked_band(
        &self,
        bands: &[crate::launcher::Band],
    ) -> Option<(usize, crate::launcher::ExpandedLayout)> {
        let index = crate::launcher::band_cursor(self, bands)?;
        let layout = self.panel_layout(bands).bands.swap_remove(index).content;
        Some((index, layout))
    }

    /// `body` in two the way the LAUNCHER VIEW draws it: the grid's half
    /// and the PANE — down the right, or along the bottom of a narrow
    /// window ([`App::launcher_pane_side`]) — or the whole body and no
    /// pane at all. `launcher::split_at` is the geometry — whether there is
    /// room for a pane, and how the two share the rows or the columns;
    /// this is the one place the fold (`^~`) and the aim are read, so the
    /// draw, the drag grip and the keys all agree about whether a pane is
    /// on screen.
    ///
    /// No card wearing the cursor, no pane: the pane is the selected
    /// session, so with nothing selected there is nothing for it to be
    /// and the grid takes the whole body back. Clicking a card selects
    /// it and the pane opens beside the cards; letting the card go —
    /// Esc, a click on the air between them — collapses it again.
    pub fn launcher_split(&self, body: Rect) -> (Rect, Option<Rect>) {
        if self.launcher_pane_hidden || !self.launcher_aimed() {
            return (body, None);
        }
        let side = crate::launcher::fitted_side(body);
        let want = if side.beside() {
            self.launcher_pane_w
        } else {
            self.launcher_pane_h
        };
        crate::launcher::split_at(body, side, want)
    }

    /// The side the PANE is laid out on this frame: down the right, or
    /// the bottom on a body too narrow to stand it beside the grid
    /// (`launcher::fitted_side`). The drag, the grip and the pointer's
    /// arrows all read this one, so none of them can disagree with the
    /// draw about which way the pane's edge runs.
    pub fn launcher_pane_side(&self) -> crate::launcher::PaneSide {
        crate::launcher::fitted_side(self.launcher_body)
    }

    /// Where the edge between the cards and the PANE sits on this frame,
    /// by `launcher::pane_boundary`'s measure — a row under the cards, a
    /// column beside them. None with no pane on screen.
    pub fn launcher_pane_boundary(&self) -> Option<i32> {
        let pane = self.launcher_split(self.launcher_body).1?;
        Some(crate::launcher::pane_boundary(
            self.launcher_pane_side(),
            pane,
        ))
    }

    /// Move the LAUNCHER VIEW's pane boundary to `boundary` — the screen
    /// row the pane starts on under the cards, or beside them the column
    /// `launcher::pane_boundary` names — and remember the height, or the
    /// width, that leaves it. `launcher::pane_height` and `pane_width` do
    /// the clamping, so a drag off either end rests against the pane's own
    /// minimum or against what the grid keeps: the header plus one row of
    /// cards, or one column of them. A body with no room for a pane at all
    /// remembers nothing: there is no edge on screen to have grabbed.
    pub fn set_launcher_pane(&mut self, boundary: i32) {
        use crate::launcher::PaneSide;
        let body = self.launcher_body;
        let side = self.launcher_pane_side();
        let want = match side {
            PaneSide::Bottom => i32::from(body.y) + i32::from(body.height) - boundary,
            PaneSide::Right => i32::from(body.x) + i32::from(body.width) - boundary,
        };
        let want = want.clamp(0, i32::from(u16::MAX)) as u16;
        if side.beside() {
            if let Some(w) = crate::launcher::pane_width(body, Some(want)) {
                self.launcher_pane_w = Some(w);
            }
        } else if let Some(h) = crate::launcher::pane_height(body, Some(want)) {
            self.launcher_pane_h = Some(h);
        }
    }

    /// Where the edge between the cards and the PANE sits when the two
    /// share the body evenly: the middle row of the body under a pane
    /// along the bottom, its middle column beside one on the right —
    /// measured as [`App::launcher_pane_boundary`] measures the edge, so
    /// handing it to [`App::set_launcher_pane`] lands the edge there.
    pub fn launcher_pane_midpoint(&self) -> i32 {
        use crate::launcher::PaneSide;
        let body = self.launcher_body;
        match self.launcher_pane_side() {
            PaneSide::Bottom => i32::from(body.y) + i32::from(body.height / 2),
            PaneSide::Right => i32::from(body.x) + i32::from(body.width / 2),
        }
    }

    pub fn alloc_req_id(&mut self, intent: PendingIntent) -> u64 {
        let id = self.next_req_id;
        self.next_req_id += 1;
        self.pending.insert(id, intent);
        id
    }

    /// Is the left button down, as far as orion knows — a press came and
    /// its release has not: a panel splitter being dragged (the LAUNCHER
    /// VIEW's pane edge, the diff and tree modals' file-list border), a
    /// program in the pane holding the button, or a drag-selection under
    /// way — in the pane or in a modal's text field? While it is, the
    /// host terminal is left exactly as it is:
    /// re-asking it for its modes mid-drag is a change under a gesture in
    /// progress, and xterm.js takes the `?1000h` in
    /// that re-ask as the end of the drag — it drops its motion listener
    /// and reports nothing more until the next press. A motion report
    /// with no button named is still the drag.
    pub fn mouse_held(&self) -> bool {
        let splitter = self.launcher_pane_drag.is_some()
            || match &self.overlay {
                Some(Overlay::Diff(view)) => view.files_drag.is_some(),
                Some(Overlay::Tree(view)) => view.files_drag.is_some(),
                _ => false,
            };
        splitter
            || crate::field_mouse::held()
            || self.term_mouse_grab.is_some()
            || self.term_selection.is_some_and(|s| s.dragging)
    }

    /// Is this worktree row a stand-in (a QUICK PROMPT's, or the NEW
    /// WORKTREE modal's) the DAEMON has not answered for yet? The
    /// in-flight intent is the one record of it —
    /// once the Ack or Error takes the intent, the row is real or gone.
    pub fn is_placeholder_worktree(&self, id: &WorktreeId) -> bool {
        self.pending
            .values()
            .any(|intent| intent.placeholder_worktree() == Some(id))
    }

    /// Is this session row a QUICK PROMPT stand-in the DAEMON has not
    /// answered for yet?
    pub fn is_placeholder_agent(&self, id: &AgentId) -> bool {
        self.pending
            .values()
            .any(|intent| intent.placeholder_agent() == Some(id))
    }

    pub fn is_placeholder_session(&self, sref: &SessionRef) -> bool {
        match sref {
            SessionRef::Agent(id) => self.is_placeholder_agent(id),
            SessionRef::Terminal(_) => false,
        }
    }

    /// The agent whose prompt is live in the pane, for `⌘U` to paste a preset
    /// into. None for a shell, a stand-in or a session that has exited.
    pub fn pane_prompt_agent(&self) -> Option<&AgentId> {
        let term = self.term.as_ref().filter(|term| !term.exited)?;
        let SessionRef::Agent(id) = &term.sref else {
            return None;
        };
        (!self.is_placeholder_agent(id)).then_some(id)
    }

    /// The pane is showing a stand-in: there is no PTY behind it to type
    /// into, and nothing to attach.
    pub fn pane_shows_placeholder(&self) -> bool {
        self.term
            .as_ref()
            .is_some_and(|t| self.is_placeholder_session(&t.sref))
    }

    /// The mouse protocol the program in the pane has asked for, and
    /// whether it wants SGR coordinates. `None` when nothing there can take
    /// a report: no session, one whose process has exited (its last screen
    /// is still worth selecting from), a stand-in pane with no PTY behind
    /// it, or a pull request showing in the pane instead of a terminal.
    pub fn child_mouse_mode(&self) -> (vt100::MouseProtocolMode, bool) {
        let mouseless = (vt100::MouseProtocolMode::None, false);
        let Some(term) = &self.term else {
            return mouseless;
        };
        // Asked on every wheel notch over the pane: one reading of the
        // cursor for all three (`RowsMemo`).
        let reading_something_else = self
            .rows_memo
            .hold(|| self.pane_shows_placeholder() || self.previewed_pr().is_some());
        if term.exited || reading_something_else {
            return mouseless;
        }
        let screen = term.parser.screen();
        (
            screen.mouse_protocol_mode(),
            screen.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr,
        )
    }

    /// Projects panel rows in display order, each an index into
    /// `tree.projects` — every project gets one.
    ///
    /// Most recently interacted with first — the newest stamp under any of
    /// the project's worktrees, so the project you just worked in heads the
    /// column (mirrors the sessions list; there is no manual reorder). The
    /// sort is stable, so never-run projects keep tree order at the bottom
    /// instead of shuffling between frames.
    pub fn project_rows(&self) -> Vec<usize> {
        self.rows_memo.with(
            |m| &m.projects,
            self.rows_key(),
            || self.build_project_rows(),
            Vec::clone,
        )
    }

    /// What [`RowsMemo::with`] keys the kept rows on.
    fn rows_key(&self) -> RowsKey {
        RowsKey {
            cursor: self.cursor_moves,
            shape: [
                self.tree.projects.len(),
                self.tree.worktrees.len(),
                self.tree.agents.len(),
                self.tree.terminals.len(),
                self.tree.links.len(),
            ],
            show_archived: self.show_archived,
            drawers: self.archived_open.len(),
        }
    }

    fn build_project_rows(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = self
            .tree
            .projects
            .iter()
            .enumerate()
            .map(|(i, _)| i)
            .collect();
        // One pass for every project rather than one per comparison the
        // sort makes (`project_recencies`).
        let recencies = project_recencies(&self.tree);
        rows.sort_by_key(|i| {
            // The raw stamp breaks the tie every project with a session
            // mid-turn shares, so the project just launched into leads —
            // the sessions list's own rule (`recency_key`).
            let r = recencies
                .get(&self.tree.projects[*i].id)
                .copied()
                .unwrap_or_default();
            (
                std::cmp::Reverse(r.interacted),
                std::cmp::Reverse(r.stamped),
            )
        });
        rows
    }

    /// Index into `tree.projects` of the selected Projects-panel row.
    pub fn selected_project_index(&self) -> Option<usize> {
        let rows = self.project_rows();
        rows.get(self.project_index_in(&rows)).copied()
    }

    // ---- the PANEL CURSORS ([`PanelCursor`]) ----

    /// Where the project cursor stands in [`App::project_rows`].
    pub fn sel_project(&self) -> usize {
        self.project_index_in(&self.project_rows())
    }

    fn project_index_in(&self, rows: &[usize]) -> usize {
        self.project_cursor
            .index(|id| rows.iter().position(|&i| &self.tree.projects[i].id == id))
    }

    /// Put the project cursor on row `index` of [`App::project_rows`]: on
    /// that project, wherever it moves from here.
    pub fn set_sel_project(&mut self, index: usize) {
        let key = self
            .project_rows()
            .get(index)
            .map(|&i| self.tree.projects[i].id.clone());
        self.project_cursor.set(key, index);
        self.cursor_moves += 1;
    }

    /// Where the Worktrees cursor stands in [`App::worktree_rows`].
    pub fn sel_worktree(&self) -> usize {
        self.worktree_index_in(&self.worktree_rows())
    }

    /// [`App::sel_worktree`] in `rows`, the Worktrees rows already in hand.
    pub fn worktree_index_in(&self, rows: &[WorktreeRow<'_>]) -> usize {
        self.worktree_cursor
            .index(|key| rows.iter().position(|row| row.has_key(key)))
    }

    /// The row the Worktrees cursor is on, as the rows stand now.
    fn selected_worktree_row(&self) -> Option<WorktreeRow<'_>> {
        let rows = self.worktree_rows();
        rows.get(self.worktree_index_in(&rows)).copied()
    }

    /// Put the Worktrees cursor on row `index` of [`App::worktree_rows`].
    pub fn set_sel_worktree(&mut self, index: usize) {
        let key = self.worktree_rows().get(index).map(|row| row.key());
        self.worktree_cursor.set(key, index);
        self.cursor_moves += 1;
        self.hold_cursor_worktree();
    }

    /// Hold the checkout the Worktrees cursor is on, in the group it is in
    /// now ([`App::worktree_hold`]) — unless it is already the one held,
    /// whose group stands from when the cursor arrived. Off a checkout (a
    /// pull request or issue row, nothing), nothing is held.
    fn hold_cursor_worktree(&mut self) {
        let Some(WorktreeRowKey::Checkout(id)) = &self.worktree_cursor.key else {
            self.worktree_hold = None;
            return;
        };
        if self
            .worktree_hold
            .as_ref()
            .is_some_and(|(held, _)| held == id)
        {
            return;
        }
        let dormant = !self.live_worktrees().contains(id);
        self.worktree_hold = Some((id.clone(), dormant));
    }

    /// Where the Sessions cursor stands in [`App::visible_session_rows`].
    pub fn sel_session(&self) -> usize {
        self.with_session_rows(|rows| self.session_index_in(rows))
    }

    /// [`App::sel_session`] in `rows`, the session rows already in hand.
    pub fn session_index_in(&self, rows: &[SessionRow]) -> usize {
        self.session_cursor
            .index(|key| rows.iter().position(|row| row.has_key(key)))
    }

    /// Put the Sessions cursor on row `index` of
    /// [`App::visible_session_rows`].
    pub fn set_sel_session(&mut self, index: usize) {
        let key = self.with_session_rows(|rows| rows.get(index).map(SessionRow::key));
        self.session_cursor.set(key, index);
        self.cursor_moves += 1;
    }

    /// Re-seat each cursor whose row has left its list onto the row now in
    /// its place ([`PanelCursor::reseat_at`]), the project first, since the
    /// rows under it are its project's. A cursor still on its row stays
    /// on it, wherever it moved.
    pub fn reseat_cursors(&mut self) {
        let project = {
            let rows = self.project_rows();
            let find = |id: &ProjectId| rows.iter().position(|&i| &self.tree.projects[i].id == id);
            self.project_cursor
                .reseat_at(find, rows.len())
                .map(|at| (rows.get(at).map(|&i| self.tree.projects[i].id.clone()), at))
        };
        if let Some((key, at)) = project {
            self.project_cursor.set(key, at);
            self.cursor_moves += 1;
        }
        let worktree = {
            let rows = self.worktree_rows();
            let find = |key: &WorktreeRowKey| rows.iter().position(|row| row.has_key(key));
            self.worktree_cursor
                .reseat_at(find, rows.len())
                .map(|at| (rows.get(at).map(|row| row.key()), at))
        };
        if let Some((key, at)) = worktree {
            self.worktree_cursor.set(key, at);
            self.cursor_moves += 1;
            self.hold_cursor_worktree();
        }
        let session = self.with_session_rows(|rows| {
            let find = |key: &SessionRowKey| rows.iter().position(|row| row.has_key(key));
            self.session_cursor
                .reseat_at(find, rows.len())
                .map(|at| (rows.get(at).map(SessionRow::key), at))
        });
        if let Some((key, at)) = session {
            self.session_cursor.set(key, at);
            self.cursor_moves += 1;
        }
    }

    /// Checkout `from` is `to` from here on — a stand-in replaced by the
    /// real row the DAEMON cut: a cursor on it stays on it.
    pub fn rekey_worktree(&mut self, from: &WorktreeId, to: &WorktreeId) {
        if self.worktree_cursor.key.as_ref() == Some(&WorktreeRowKey::Checkout(from.clone())) {
            self.worktree_cursor.key = Some(WorktreeRowKey::Checkout(to.clone()));
            self.cursor_moves += 1;
            if let Some((held, _)) = &mut self.worktree_hold {
                if held == from {
                    *held = to.clone();
                }
            }
        }
    }

    /// Session `from` is `to` from here on — a stand-in replaced by the
    /// real row: a cursor on it stays on it.
    pub fn rekey_session(&mut self, from: &SessionRef, to: &SessionRef) {
        if self.session_cursor.key.as_ref() == Some(&SessionRowKey::Session(from.clone())) {
            self.session_cursor.key = Some(SessionRowKey::Session(to.clone()));
            self.cursor_moves += 1;
        }
    }

    /// The project giving the current selection its context.
    pub fn selected_project(&self) -> Option<&Project> {
        self.tree.projects.get(self.selected_project_index()?)
    }

    /// `project`'s ROOT WORKTREE — its main checkout — when the tree has
    /// one. Stand-ins count; `launcher::root_checkout` is the one that
    /// steps over a checkout still being cut.
    pub fn root_worktree(&self, project: &ProjectId) -> Option<WorktreeId> {
        self.tree
            .worktrees
            .iter()
            .find(|w| &w.project_id == project && w.is_main)
            .map(|w| w.id.clone())
    }

    /// The checkout under the Worktrees cursor — a plain row or one nested
    /// under its pull request. None while the cursor is on a pull request.
    pub fn selected_worktree(&self) -> Option<&Worktree> {
        self.selected_worktree_row().and_then(WorktreeRow::checkout)
    }

    /// A checkout's last-read changed-file count, for its cards: None until
    /// one has been read there, or when git couldn't say.
    pub fn worktree_changes(&self, id: &WorktreeId) -> Option<usize> {
        self.worktree_changes.get(id).and_then(|(count, _)| *count)
    }

    /// A checkout's last-read line counts, for its cards: None until one
    /// has been read there, or when nothing changed by the line.
    pub fn worktree_lines(&self, id: &WorktreeId) -> Option<crate::git_diff::LineChanges> {
        self.worktree_lines.get(id).copied()
    }

    /// A checkout's last-read commits ahead of and behind its base; none
    /// either way until one has been read there.
    pub fn worktree_ahead_behind(&self, id: &WorktreeId) -> (usize, usize) {
        self.worktree_ahead.get(id).copied().unwrap_or_default()
    }

    /// Does the cache describe a different worktree than the selection?
    /// The event loop refreshes before drawing when it does, so the badge
    /// never lags a j/k by a poll interval.
    pub fn git_changes_stale(&self) -> bool {
        self.git_changes.as_ref().map(|(id, _)| id) != self.selected_worktree().map(|w| &w.id)
    }

    /// Whether the daemon currently holds a live PTY for `sref`. Attaching
    /// to one only replays its ring; attaching to a session the IDLE REAPER
    /// took cold-spawns an agent CLI.
    pub fn session_is_live(&self, sref: &SessionRef) -> bool {
        match sref {
            SessionRef::Agent(id) => self.tree.agents.iter().any(|a| &a.id == id && a.alive),
            SessionRef::Terminal(id) => self.tree.terminals.iter().any(|t| &t.id == id && t.alive),
        }
    }

    /// Put a screen the pane is leaving aside for a quick return, when it
    /// is worth keeping: a real session that has painted and is still live
    /// (see [`App::term_cache`]). The cache is most-recent-first and
    /// bounded twice — [`TERM_CACHE_MAX`] screens, [`TERM_CACHE_CELLS`]
    /// between them; whatever it already held for this session is
    /// replaced. Over the cell budget, histories go before screens do,
    /// oldest first: a screen without its scrollback is a fiftieth of the
    /// size and still paints the return on the keypress.
    pub fn stash_term(&mut self, term: AttachedTerm) {
        self.term_cache.retain(|t| t.sref != term.sref);
        let keep = term.painted
            && !term.exited
            && !self.is_placeholder_session(&term.sref)
            && self.session_is_live(&term.sref);
        if !keep {
            return;
        }
        let mut term = term;
        // Left before the history it asked back had landed: still without
        // it, and no longer asking.
        if term.pending_scroll.take().is_some() {
            term.history_dropped = true;
        }
        self.term_cache.insert(0, term);
        self.term_cache.truncate(TERM_CACHE_MAX);
        self.prune_term_cache();
        let held = |cache: &[AttachedTerm]| -> usize {
            cache.iter().map(AttachedTerm::estimated_cells).sum()
        };
        while held(&self.term_cache) > TERM_CACHE_CELLS {
            let oldest_with_history = self
                .term_cache
                .iter()
                .rposition(|t| t.parser.screen().scrollback_rows() > 0);
            match oldest_with_history {
                Some(i) => self.term_cache[i].drop_history(),
                None => {
                    self.term_cache.pop();
                }
            }
        }
    }

    /// The kept screen for `sref`, if there is one and its session is
    /// still live — a session that died meanwhile comes back as a new
    /// process, and its old screen would only mislead for a frame.
    pub fn take_cached_term(&mut self, sref: &SessionRef) -> Option<AttachedTerm> {
        let i = self.term_cache.iter().position(|t| &t.sref == sref)?;
        let term = self.term_cache.remove(i);
        self.session_is_live(sref).then_some(term)
    }

    /// Drop kept screens whose session is gone or no longer live.
    pub fn prune_term_cache(&mut self) {
        let live: Vec<bool> = self
            .term_cache
            .iter()
            .map(|t| self.session_is_live(&t.sref))
            .collect();
        let mut i = 0;
        self.term_cache.retain(|_| {
            let keep = live[i];
            i += 1;
            keep
        });
    }

    /// The full row list the panel shows — what [`App::sel_session`] is a
    /// place in.
    pub fn visible_session_rows(&self) -> Vec<SessionRow> {
        self.with_session_rows(<[SessionRow]>::to_vec)
    }

    /// `read` the rows [`App::visible_session_rows`] lists.
    fn with_session_rows<R>(&self, read: impl FnOnce(&[SessionRow]) -> R) -> R {
        self.rows_memo.hold(|| {
            self.rows_memo.with(
                |m| &m.sessions,
                self.rows_key(),
                || self.build_session_rows(),
                |rows| read(rows),
            )
        })
    }

    fn build_session_rows(&self) -> Vec<SessionRow> {
        // All four groups below hang off the SAME checkout, and each one
        // asking `selected_worktree` for it re-sorts every checkout of the
        // project — four sorts to build one list of rows. Asked once here
        // and handed down.
        let Some(wt) = self.selected_worktree() else {
            return vec![];
        };
        let wt = &wt.id;
        let agents = self.sessions_in(wt);
        let (active, _) = self.group_counts_in(wt);
        let active = active.min(agents.len());
        let mut rows: Vec<SessionRow> = agents[..active]
            .iter()
            .cloned()
            .map(SessionRow::Agent)
            .collect();
        rows.extend(self.terminals_in(wt).into_iter().map(SessionRow::Terminal));
        rows.extend(self.links_in(wt).into_iter().map(SessionRow::Link));
        rows.extend(agents[active..].iter().cloned().map(SessionRow::Agent));
        rows
    }

    /// The selected worktree's PULL REQUESTS group: the pull request on its branch
    /// first (however it got there), then any previously saved links in list
    /// order. New saved links are no longer exposed through the TUI.
    /// A saved link that *is* the pull request is shown once, as the
    /// pull-request row — a duplicate would just be the same destination
    /// twice.
    pub fn visible_links(&self) -> Vec<LinkRow> {
        let Some(wt) = self.selected_worktree() else {
            return vec![];
        };
        self.links_in(&wt.id)
    }

    /// The same for a checkout already in hand ([`App::sessions_in`]).
    fn links_in(&self, wt: &WorktreeId) -> Vec<LinkRow> {
        let saved: Vec<&Link> = self
            .tree
            .links
            .iter()
            .filter(|l| &l.worktree_id == wt)
            .collect();
        let pr = self.pull_requests.get(wt).cloned().flatten();
        let matched = pr
            .as_ref()
            .and_then(|p| saved.iter().position(|l| l.url == p.url));
        let mut rows: Vec<LinkRow> = Vec::new();
        match (&pr, matched) {
            (Some(pr), Some(i)) => rows.push(LinkRow::Saved {
                link: saved[i].clone(),
                pr: Some(pr.clone()),
            }),
            (Some(pr), None) => rows.push(LinkRow::PullRequest(pr.clone())),
            (None, _) => {}
        }
        for (i, link) in saved.iter().enumerate() {
            if Some(i) != matched {
                rows.push(LinkRow::Saved {
                    link: (*link).clone(),
                    pr: None,
                });
            }
        }
        rows
    }

    /// The link row under the cursor, when the cursor is on one.
    pub fn selected_link(&self) -> Option<LinkRow> {
        match self.selected_session_row() {
            Some(SessionRow::Link(l)) => Some(l),
            _ => None,
        }
    }

    pub fn selected_session_row(&self) -> Option<SessionRow> {
        self.with_session_rows(|rows| rows.get(self.session_index_in(rows)).cloned())
    }

    /// The selected row's agent, when it is one (terminal rows return None).
    pub fn selected_session(&self) -> Option<Agent> {
        match self.selected_session_row() {
            Some(SessionRow::Agent(a)) => Some(a),
            _ => None,
        }
    }

    // ---- the FOLLOW-UP COMPOSER ----

    /// Can this row grow a FOLLOW-UP COMPOSER? An agent with a local PTY
    /// behind it, and only that: an ARCHIVED row's turn is over, a QUICK
    /// PROMPT stand-in has no session yet, and a TERMINAL or a PULL
    /// REQUEST row was never a conversation to follow up on.
    pub fn takes_follow_up(&self, row: &SessionRow) -> bool {
        matches!(
            row,
            SessionRow::Agent(a)
                if !a.archived && !self.is_placeholder_agent(&a.id)
        )
    }

    /// The row the open FOLLOW-UP COMPOSER belongs to — an index into
    /// [`App::visible_session_rows`], found by AGENT so a re-sorted list
    /// keeps the box on its own card. None with nothing expanded, or when
    /// the agent it was opened on has left the list (archived, deleted, or
    /// the panel moved to another checkout).
    pub fn follow_up_row(&self) -> Option<usize> {
        let id = &self.follow_up.as_ref()?.agent;
        self.visible_session_rows()
            .iter()
            .position(|row| matches!(row, SessionRow::Agent(a) if &a.id == id))
    }

    /// Is the composer live — open, and on a card still in the list? What
    /// decides whether the SESSIONS PANEL's keys are the box's.
    pub fn follow_up_live(&self) -> bool {
        self.follow_up_row().is_some()
    }

    /// The same for a checkout already in hand ([`App::sessions_in`]).
    fn terminals_in(&self, wt: &WorktreeId) -> Vec<TerminalTab> {
        self.tree
            .terminals
            .iter()
            .filter(|t| &t.worktree_id == wt)
            .cloned()
            .collect()
    }

    /// First free `prefix-N` name within the selected worktree. A QUICK
    /// PROMPT stand-in counts as taken: its create is in flight and will
    /// land under that name (the create it stands for takes the name off
    /// the stand-in itself — see `create_agent`).
    pub fn default_session_name(&self, prefix: &str) -> String {
        let taken: Vec<String> = self
            .visible_sessions()
            .iter()
            .map(|a| a.name.clone())
            .collect();
        let mut n = 1;
        loop {
            let candidate = format!("{prefix}-{n}");
            if !taken.contains(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    /// Worktrees of the selected project, in [`App::worktree_order`].
    pub fn visible_worktrees(&self) -> Vec<&Worktree> {
        self.rows_memo
            .with(
                |m| &m.worktrees,
                self.rows_key(),
                || self.build_visible_worktrees(),
                Vec::clone,
            )
            .into_iter()
            .map(|i| &self.tree.worktrees[i])
            .collect()
    }

    /// [`App::visible_worktrees`], as indices into `tree.worktrees`.
    fn build_visible_worktrees(&self) -> Vec<usize> {
        match self.selected_project() {
            Some(project) => self.worktree_order(&project.id),
            None => vec![],
        }
    }

    /// `project`'s checkouts in the order its BANDS stand on the grid
    /// ([`App::worktree_order`]), whichever project is selected.
    pub fn worktrees_in_band_order(&self, project: &ProjectId) -> Vec<&Worktree> {
        self.worktree_order(project)
            .into_iter()
            .map(|i| &self.tree.worktrees[i])
            .collect()
    }

    /// `project`'s checkouts as indices into `tree.worktrees`, in the one
    /// order every list of them uses: the root checkout first, then the
    /// LIVE ones in the order they went live ([`WakeOrder`]), then the
    /// DORMANT ones ([`App::worktree_dormant`]) oldest first: in tree
    /// order, which is the order the DAEMON registered them in (its store
    /// loads them by `created_at`), with a checkout cut since appended.
    ///
    /// Never by recency. Ordering on the last turn moved a checkout every
    /// time one of its sessions started or finished one, and the list
    /// reshuffled under the reader all day. The one move left is a whole
    /// checkout going cold and sinking below the divider, or waking and
    /// coming up to sit right above it, at the bottom of the live ones —
    /// never back into the slot it was made in, which shifted every live
    /// checkout below that slot. The HELD CHECKOUT under the cursor does
    /// not make even that move until the cursor leaves it
    /// ([`App::worktree_hold`]).
    pub fn worktree_order(&self, project: &ProjectId) -> Vec<usize> {
        let worktrees = &self.tree.worktrees;
        let rows: Vec<usize> = (0..worktrees.len())
            .filter(|&i| &worktrees[i].project_id == project)
            .collect();
        let live = self.live_worktrees();
        let dormant: Vec<bool> = rows
            .iter()
            .map(|&i| self.dormant_in(&live, &worktrees[i].id))
            .collect();
        let mut wake = self.wake_order.borrow_mut();
        wake.note(
            rows.iter()
                .zip(&dormant)
                .map(|(&i, &dormant)| (&worktrees[i].id, !dormant)),
        );
        let mut keyed: Vec<(usize, bool)> = rows.iter().copied().zip(dormant).collect();
        // A stable sort: the dormant ones keep tree order.
        keyed.sort_by_key(|&(i, dormant)| {
            let w = &worktrees[i];
            let place = if dormant { 0 } else { wake.place(&w.id) };
            (std::cmp::Reverse(w.is_main), dormant, place)
        });
        keyed.into_iter().map(|(i, _)| i).collect()
    }

    /// Whether checkout `id` sorts as DORMANT, given the `live` ones: the
    /// group the HELD CHECKOUT arrived in, for that one.
    fn dormant_in(&self, live: &HashSet<&WorktreeId>, id: &WorktreeId) -> bool {
        match &self.worktree_hold {
            Some((held, dormant)) if held == id => *dormant,
            _ => !live.contains(id),
        }
    }

    /// Whether checkout `id` is DORMANT: no session in it is doing anything
    /// or has anything to say. Every unarchived session is drawn cold
    /// ([`drawn_cold`]), or it has none. A checkout still being cut, or
    /// with a session still starting up, is live. The HELD CHECKOUT is in
    /// the group it was in when the cursor arrived ([`App::worktree_hold`]).
    pub fn worktree_dormant(&self, id: &WorktreeId) -> bool {
        self.dormant_in(&self.live_worktrees(), id)
    }

    /// Every checkout with a live session in it, in one pass over the
    /// sessions: a sort asks once per comparison.
    pub(crate) fn live_worktrees(&self) -> HashSet<&WorktreeId> {
        let mut live: HashSet<&WorktreeId> = self
            .tree
            .agents
            .iter()
            .filter(|a| {
                !a.archived && (!drawn_cold(a) || starting(a) || self.is_placeholder_agent(&a.id))
            })
            .map(|a| &a.worktree_id)
            .collect();
        // A stand-in checkout has no path until the DAEMON's upsert names
        // it — and that, unlike the in-flight intent, is true from the
        // moment the row goes up, before the intent is filed.
        live.extend(
            self.tree
                .worktrees
                .iter()
                .filter(|w| w.path.as_os_str().is_empty() || self.is_placeholder_worktree(&w.id))
                .map(|w| &w.id),
        );
        live
    }

    /// Every branch `project` already has a checkout for — hidden ROOT
    /// WORKTREE included — so a generated branch name never collides.
    pub fn project_branches(&self, project: &ProjectId) -> Vec<String> {
        self.tree
            .worktrees
            .iter()
            .filter(|w| &w.project_id == project)
            .map(|w| w.branch.clone())
            .collect()
    }

    /// The selected project's open pull requests, every one the list query
    /// answered with — drafts included whatever `hide_draft_prs` says, and
    /// whether or not the group under the checkouts is showing them. The
    /// list the fetch cap (`pull_request::LIST_LIMIT`) is measured
    /// against. Empty until the first answer (or when the repo genuinely
    /// has none).
    pub fn all_open_prs(&self) -> &[OpenPr] {
        self.selected_project()
            .and_then(|p| self.open_prs.get(&p.id))
            .map(|o| o.list.as_slice())
            .unwrap_or_default()
    }

    /// The open pull requests the PROJECT OPEN PRS GROUP lists once it is
    /// open: the whole answer, or — with `hide_draft_prs` on — only the
    /// rows asking for a reviewer. What its header counts while it is
    /// folded. A view over [`App::all_open_prs`], filtered on every read
    /// rather than once on arrival, so switching the setting off shows
    /// every draft at once with no refetch, and a draft marked ready on
    /// GitHub joins the rows on the refresh that says so.
    pub fn listed_open_prs(&self) -> Vec<&OpenPr> {
        self.all_open_prs()
            .iter()
            .filter(|pr| !(self.hide_draft_prs && self.pr_is_draft(&pr.url)))
            .collect()
    }

    /// Whether `url` is a draft, as `prs` last heard — what hiding drafts
    /// (`hide_draft_prs`) and their place in the list go by.
    pub fn pr_is_draft(&self, url: &str) -> bool {
        self.prs.status(url).is_some_and(|s| s.is_draft())
    }

    /// The open pull requests with rows under the checkouts: the listed
    /// ones, or none while the group is folded — a folded group has no
    /// rows for the cursor to walk into, the way a collapsed ARCHIVED
    /// group has none.
    pub fn visible_open_prs(&self) -> Vec<&OpenPr> {
        if self.open_prs_collapsed {
            return Vec::new();
        }
        self.listed_open_prs()
    }

    /// The Worktrees panel's rows in cursor order — what
    /// [`App::sel_worktree`] is a place in: the project's checkouts, then the pull requests still open
    /// on its repo, each followed by the checkout on its head branch when
    /// the project has one. A PR SESSION works in a checkout of the pull
    /// request's head branch (`CreatePrAgent` cuts it, or reuses the one
    /// already there), and that checkout listed among the plain ones left
    /// no way to tell which pull request it was for — so it sits under
    /// the pull request's row instead, indented, and the plain rows above
    /// are the checkouts that are nobody's PR. The ROOT WORKTREE never
    /// nests, whatever branch it is on; a branch two open pull requests
    /// share nests under the first listed. Only a pull request *on
    /// screen* takes its checkout: with the group folded, or the pull
    /// request a draft `hide_draft_prs` keeps out, the checkout is a plain
    /// row again — hiding pull requests must never hide work you have.
    ///
    /// A cursor parked on a pull request has no selected worktree, which
    /// is the truth about it; one on a nested checkout has that worktree
    /// and no pull request — the row is a worktree, its pull request is
    /// the row above.
    pub fn worktree_rows(&self) -> Vec<WorktreeRow<'_>> {
        // The checkouts and the pull requests each ask for the selected
        // project: one sort of the projects between them.
        self.rows_memo.hold(|| self.build_worktree_rows())
    }

    fn build_worktree_rows(&self) -> Vec<WorktreeRow<'_>> {
        let checkouts = self.visible_worktrees();
        let prs = self.visible_open_prs();
        // Which listed pull request each checkout nests under, if any.
        let under: Vec<Option<usize>> = checkouts
            .iter()
            .map(|w| {
                if w.is_main {
                    return None;
                }
                prs.iter().position(|pr| pr.head == w.branch)
            })
            .collect();
        let mut rows: Vec<WorktreeRow<'_>> = checkouts
            .iter()
            .zip(&under)
            .filter(|(_, under)| under.is_none())
            .map(|(w, _)| WorktreeRow::Checkout(w))
            .collect();
        for (i, pr) in prs.iter().enumerate() {
            rows.push(WorktreeRow::Pr(pr));
            rows.extend(
                checkouts
                    .iter()
                    .zip(&under)
                    .filter(|(_, under)| **under == Some(i))
                    .map(|(w, _)| WorktreeRow::PrCheckout(w)),
            );
        }
        rows
    }

    /// How many rows the Worktrees panel has — see [`App::worktree_rows`].
    pub fn worktree_row_count(&self) -> usize {
        self.worktree_rows().len()
    }

    /// The Worktrees row of checkout `id`, wherever it sits — among the
    /// plain checkouts or under its pull request. None when it is not a
    /// row of the selected project (or is the hidden ROOT WORKTREE).
    pub fn worktree_row_of(&self, id: &WorktreeId) -> Option<usize> {
        self.worktree_rows()
            .iter()
            .position(|row| row.checkout().is_some_and(|w| &w.id == id))
    }

    /// The Worktrees row of the open pull request at `url`: its own row,
    /// not the checkout nested under it. None while the group is folded
    /// or the pull request is not listed.
    pub fn open_pr_row_of(&self, url: &str) -> Option<usize> {
        self.worktree_rows()
            .iter()
            .position(|row| row.open_pr().is_some_and(|pr| pr.url == url))
    }

    /// The open pull request under the Worktrees cursor, when it's on one.
    /// Mutually exclusive with [`App::selected_worktree`]: a pull request
    /// row has no checkout, and a checkout nested under a pull request is
    /// still a checkout, not the pull request.
    pub fn selected_worktree_pr(&self) -> Option<&OpenPr> {
        self.selected_worktree_row().and_then(WorktreeRow::open_pr)
    }

    /// The pull request the pane should be reading: the PROJECT OPEN PRS
    /// GROUP row under the Worktrees cursor, or — while the SESSIONS PANEL
    /// has focus — the PR ROW under its cursor (a saved LINK that *is* the
    /// branch's pull request counts; a bare URL does not).
    ///
    /// The Sessions half is keyed on focus on purpose: a session is still
    /// attached behind that pane, and stepping into it (`l`, a click) has
    /// to bring the terminal back so what you type is what you see. The
    /// Worktrees half never needs that — a PR row there has no checkout, so
    /// there is nothing behind the pane to return to.
    pub fn previewed_pr(&self) -> Option<PreviewedPr> {
        if let Some(pr) = self.selected_worktree_pr() {
            return Some(PreviewedPr {
                number: pr.number,
                url: pr.url.clone(),
                label: self.prs.label(pr.number, &pr.url, &pr.title),
            });
        }
        // The LAUNCHER VIEW's pane holds the keys while it reads the pull
        // request (`→` off the row's last card, Enter on a band with no
        // cards, a click on it): it goes on reading it there.
        if !matches!(self.focus, Focus::Sessions | Focus::Terminal) {
            return None;
        }
        let row = match self.selected_session_row() {
            Some(SessionRow::Link(row)) => row,
            Some(_) => return None,
            // Nothing under the cursor: a band whose own `gh pr view` has
            // not answered yet, its row still to come, while the project's
            // open list already names its pull request — the band's rule
            // shows it (`launcher::row_pr`). The pane reads that one now,
            // so `→` has a page to focus while it loads, and the row that
            // lands later is the same URL, the reader left in place.
            None => {
                let worktree = self.selected_worktree()?;
                let pr = crate::launcher::row_pr(
                    self,
                    &worktree.id,
                    &worktree.project_id,
                    &worktree.branch,
                )?;
                return Some(PreviewedPr {
                    number: pr.number,
                    label: crate::pull_request::numbered_label(pr.number, &pr.title),
                    url: pr.url,
                });
            }
        };
        let pr = row.pull_request()?;
        Some(PreviewedPr {
            number: pr.number,
            url: pr.url.clone(),
            label: self.prs.label(pr.number, &pr.url, &pr.title),
        })
    }

    /// What the pane is reading, by URL — the pull request under a cursor
    /// — so the loop can tell a turn that changed it
    /// (`note_preview_change`) from one that left the reader in place.
    pub fn reading_url(&self) -> Option<String> {
        // Twice a turn of the event loop, and `previewed_pr` walks to the
        // cursor more than once (`RowsMemo`).
        self.rows_memo.hold(|| self.previewed_pr().map(|pr| pr.url))
    }

    /// The pane is reading a page — a pull request — rather than showing a
    /// PTY: FOCUS in it scrolls the page, and there is nothing to type
    /// into.
    pub fn pane_reads_page(&self) -> bool {
        self.reading_url().is_some()
    }

    /// Session rows for the selected worktree: the live agents, then (when
    /// shown) the archived ones.
    ///
    /// The live rows are ordered by last interaction, newest first, so the
    /// session you just ran surfaces at the top and the list reads as a
    /// history of what you have been doing. Working sessions count as
    /// interacting now, which keeps them on top however long the turn has
    /// taken.
    pub fn visible_sessions(&self) -> Vec<Agent> {
        let Some(wt) = self.selected_worktree() else {
            return vec![];
        };
        self.sessions_in(&wt.id)
    }

    /// The same for a checkout the caller has already found, so the rows
    /// builder does not pay for `selected_worktree` to find it again
    /// ([`App::visible_session_rows`]).
    fn sessions_in(&self, wt: &WorktreeId) -> Vec<Agent> {
        // Stable throughout, so ties — never-run rows especially, which all
        // stamp 0 — keep tree order instead of shuffling between frames.
        let mut rows: Vec<Agent> = self
            .tree
            .agents
            .iter()
            .filter(|a| &a.worktree_id == wt && !a.archived)
            .cloned()
            .collect();
        rows.sort_by_key(recency_key);
        // A stable pass over the top of it: the session just launched
        // leads the list from the moment its row arrives, rather than
        // sitting under the working ones until its own turn starts.
        if let Some(id) = &self.just_launched {
            rows.sort_by_key(|a| &a.id != id);
        }
        // The ARCHIVED VIEW lists every checkout's archived rows; on the
        // live grid, a checkout whose ARCHIVED DRAWER is open lists its
        // own, so the cursor can rest on one there.
        if self.show_archived || self.archived_open.contains(wt) {
            let mut archived: Vec<Agent> = self
                .tree
                .agents
                .iter()
                .filter(|a| &a.worktree_id == wt && a.archived)
                .cloned()
                .collect();
            // Most recently archived first; pre-`archived_at` rows (stamp 0)
            // keep tree order at the bottom (stable sort).
            archived.sort_by_key(|a| std::cmp::Reverse(a.archived_at));
            rows.extend(archived);
        }
        rows
    }

    /// The same for a checkout already in hand ([`App::sessions_in`]).
    fn group_counts_in(&self, wt: &WorktreeId) -> (usize, usize) {
        let live = self
            .tree
            .agents
            .iter()
            .filter(|a| &a.worktree_id == wt && !a.archived)
            .count();
        let archived = self
            .tree
            .agents
            .iter()
            .filter(|a| &a.worktree_id == wt && a.archived)
            .count();
        (live, archived)
    }

    /// Delay until the pending worktree-sessions prewarm is due, so the
    /// event loop can wake up and fire it. None when nothing is armed.
    pub fn prewarm_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_prewarm.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// How long until the debounced attach should be sent, if one is armed.
    pub fn attach_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_attach.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// Whether `gh` should be asked about this worktree now: not while an
    /// answer is in flight, and not before the timer the last answer armed.
    /// A found PR no longer retires the worktree — the PR doesn't change,
    /// but its conversation does, and the unread badge is only as fresh as
    /// the last poll.
    pub fn pr_lookup_due(&self, worktree: &WorktreeId) -> bool {
        if self.pr_inflight.in_flight(worktree) {
            return false;
        }
        match self.pr_recheck.get(worktree) {
            Some((due, _)) => std::time::Instant::now() >= *due,
            None => true,
        }
    }

    /// Whether the open list should be asked for this project now: not while
    /// an answer is in flight, and not before the timer the last answer
    /// armed. A project orion has never asked about is always due.
    pub fn open_prs_lookup_due(&self, project: &ProjectId) -> bool {
        if self.open_prs_inflight.in_flight(project) {
            return false;
        }
        match self.open_prs.get(project) {
            Some(open) => std::time::Instant::now() >= open.due,
            None => true,
        }
    }

    /// Furthest the pull-request preview may scroll: its last line pinned
    /// to the bottom of the pane. Zero while the preview fits, so a wheel
    /// flick on a short PR does nothing instead of scrolling it off screen.
    pub fn pr_preview_max_scroll(&self) -> u16 {
        (self.pr_preview_lines as u16).saturating_sub(self.term_area.height.max(1))
    }

    /// Delay until the debounced pull-request detail fetch is due. None when
    /// the cursor isn't resting on a pull request that still needs one.
    pub fn pr_detail_delay(&self) -> Option<std::time::Duration> {
        let (_, at) = self.pending_pr_detail.as_ref()?;
        Some(at.saturating_duration_since(std::time::Instant::now()))
    }

    /// The PULL REQUESTS MODAL's prefetch (`pr_modal::Prefetch`): when it
    /// next has a page to ask for. None when it has none, or enough are
    /// already in flight — a landing re-runs the loop, which asks again.
    pub fn pr_prefetch_delay(&self) -> Option<std::time::Duration> {
        crate::pr_modal::prefetch_delay(self)
    }

    /// Whether `url`'s page was read from `gh` within
    /// `event_loop::PR_DETAIL_FRESH` and nothing has said it is behind
    /// since — what a cursor resting on it shows without asking again.
    pub fn pr_detail_fresh(&self, url: &str) -> bool {
        self.pr_detail.contains_key(url)
            && !self.pr_detail_stale.contains(url)
            && self.pr_detail_recent(url)
    }

    /// Whether resting a cursor on `url` should ask `gh` for its page:
    /// not while one is in flight, nor while a fresh one is here
    /// ([`Self::pr_detail_fresh`]) or `gh` refused it moments ago.
    pub fn pr_detail_owed(&self, url: &str) -> bool {
        if self.pr_detail_inflight.in_flight(&url.to_string()) {
            return false;
        }
        if self.pr_detail_failed.contains(url) {
            return !self.pr_detail_recent(url);
        }
        !self.pr_detail_fresh(url)
    }

    /// Whether `url`'s page has never been read from `gh` this session —
    /// missing, or only the cache's copy — and isn't being read now: what
    /// the modal's background prefetch fills in, ignoring age.
    pub fn pr_detail_unread(&self, url: &str) -> bool {
        !self.pr_detail_inflight.in_flight(&url.to_string())
            && !self.pr_detail_failed.contains(url)
            && (!self.pr_detail.contains_key(url) || self.pr_detail_stale.contains(url))
    }

    /// `gh`'s last word on `url` came within `PR_DETAIL_FRESH`, or there
    /// is no record of when it came.
    fn pr_detail_recent(&self, url: &str) -> bool {
        self.pr_detail_at
            .get(url)
            .is_none_or(|at| at.elapsed() < crate::event_loop::PR_DETAIL_FRESH)
    }

    /// Every pull request still on some row, by URL: each project's open
    /// list and its merged tail, and each checkout's own PR ROW whatever
    /// its state — a merged pull request stays on that row (see
    /// `pull_request::PullRequest`). What the per-URL caches — bodies in
    /// memory, diffs on disk — are pruned to.
    pub fn live_pr_urls(&self) -> std::collections::HashSet<String> {
        self.open_prs
            .values()
            .flat_map(|o| o.list.iter().chain(&o.merged).map(|pr| pr.url.clone()))
            .chain(
                self.pull_requests
                    .values()
                    .flatten()
                    .map(|pr| pr.url.clone()),
            )
            .collect()
    }

    /// Aggregate status for a worktree row ([`worktree_rollup`]), archived
    /// agents excluded.
    pub fn worktree_rollup(&self, worktree_id: &WorktreeId) -> Option<AgentStatus> {
        worktree_rollup(&self.tree, worktree_id)
    }

    /// Whether the checkout's row wears its pull request's merge instead of
    /// its sessions' status. The PR ROW keeps a merged pull request
    /// (`pull_requests` holds it, state and all), and a checkout whose
    /// branch has landed is the one to archive or delete — so its row says
    /// so in purple, from across the room. A live session still wins: a
    /// running or asking agent is exactly the thing not to delete a
    /// checkout out from under, and its spinner or crimson is the warning.
    pub fn worktree_wears_merge(&self, worktree_id: &WorktreeId) -> bool {
        let merged = self
            .pull_requests
            .get(worktree_id)
            .and_then(Option::as_ref)
            .and_then(|pr| self.prs.status(&pr.url))
            .is_some_and(|s| s.standing == crate::pull_request::Standing::Merged);
        merged
            && !matches!(
                self.worktree_rollup(worktree_id),
                Some(AgentStatus::Running | AgentStatus::NeedsFeedback)
            )
    }

    /// The worktree's RUN TERMINAL — the terminal `r` started there, still
    /// running or exited on its own — when it has one.
    pub fn run_terminal(&self, worktree_id: &WorktreeId) -> Option<&TerminalTab> {
        self.tree
            .terminals
            .iter()
            .find(|t| &t.worktree_id == worktree_id && t.is_run())
    }

    /// Whether the worktree's RUN COMMAND is up right now: the RUNNING
    /// badge its row wears, and what `r` stops.
    pub fn worktree_running(&self, worktree_id: &WorktreeId) -> bool {
        self.run_terminal(worktree_id).is_some_and(|t| t.alive)
    }

    /// When the worktree's run, sent its `^C` by a stop and still going,
    /// is killed (epoch ms) — the DAEMON's word, so every client agrees.
    /// None for a run nobody is stopping, and for no run at all.
    pub fn run_stopping(&self, worktree_id: &WorktreeId) -> Option<i64> {
        self.run_terminal(worktree_id)?.run.stopping_until
    }

    /// Sessions under a worktree that went green with nobody looking.
    /// Test-only: the grid reads the free functions through the palette.
    #[cfg(test)]
    pub fn worktree_unseen(&self, worktree_id: &WorktreeId) -> usize {
        worktree_unseen(&self.tree, worktree_id)
    }

    #[cfg(test)]
    pub fn project_unseen(&self, project_id: &ProjectId) -> usize {
        project_unseen(&self.tree, project_id)
    }

    /// The two places FOCUS can rest: the LAUNCHER VIEW's GRID of cards
    /// and the PANE under them. The three columns the other variants name
    /// are no longer drawn.
    pub fn focus_visible(&self, focus: Focus) -> bool {
        matches!(focus, Focus::Sessions | Focus::Terminal)
    }

    fn focus_rank(focus: Focus) -> u8 {
        match focus {
            Focus::Projects => 1,
            Focus::Worktrees => 2,
            Focus::Sessions => 3,
            Focus::Terminal => 4,
        }
    }

    pub fn next_visible_focus(&self, focus: Focus) -> Focus {
        let rank = Self::focus_rank(focus);
        [
            Focus::Projects,
            Focus::Worktrees,
            Focus::Sessions,
            Focus::Terminal,
        ]
        .into_iter()
        .find(|candidate| Self::focus_rank(*candidate) > rank && self.focus_visible(*candidate))
        .unwrap_or(focus)
    }

    pub fn previous_visible_focus(&self, focus: Focus) -> Focus {
        let rank = Self::focus_rank(focus);
        [
            Focus::Terminal,
            Focus::Sessions,
            Focus::Worktrees,
            Focus::Projects,
        ]
        .into_iter()
        .find(|candidate| Self::focus_rank(*candidate) < rank && self.focus_visible(*candidate))
        .unwrap_or(focus)
    }

    /// Where `target` was drawn on the last frame — the rect the hit was
    /// registered with. A dropdown hangs off the word it belongs to, so
    /// it needs the word's own cell and not the pointer's.
    pub fn hit_rect(&self, target: &HitTarget) -> Option<Rect> {
        self.hits
            .iter()
            .find(|(_, t)| t == target)
            .map(|(rect, _)| *rect)
    }

    pub fn hit_at(&self, x: u16, y: u16) -> Option<HitTarget> {
        self.hits
            .iter()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
            })
            .map(|(_, t)| t.clone())
    }
}

/// Test-only accessors: nothing in the app reads these any more.
#[cfg(test)]
impl App {
    /// The cached changed-file count when it belongs to the selected
    /// worktree; `None` while unknown or the checkout is unreadable.
    pub fn selected_worktree_changes(&self) -> Option<usize> {
        let wt = self.selected_worktree()?;
        match &self.git_changes {
            Some((id, count)) if *id == wt.id => *count,
            _ => None,
        }
    }

    /// (live, archived) agent counts for the selected worktree.
    pub fn session_group_counts(&self) -> (usize, usize) {
        let Some(wt) = self.selected_worktree() else {
            return (0, 0);
        };
        self.group_counts_in(&wt.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the ROWS MEMO ----

    /// Two projects: `api` with its root `w0` and a `feat` checkout `w1`,
    /// `web` with its root `w2`; one session in each checkout, stamped in
    /// that order so `web` is the project used most recently.
    fn a_memo_tree() -> App {
        let mut app = App::new();
        app.tree.projects = ["api", "web"]
            .iter()
            .enumerate()
            .map(|(i, name)| Project {
                id: ProjectId(format!("p{i}")),
                name: (*name).into(),
                repo_path: format!("/tmp/{name}").into(),
                sort_order: 0,
            })
            .collect();
        app.tree.worktrees = [("w0", "p0", true), ("w1", "p0", false), ("w2", "p1", true)]
            .iter()
            .map(|(id, project, is_main)| Worktree {
                id: WorktreeId((*id).into()),
                project_id: ProjectId((*project).into()),
                path: format!("/tmp/{id}").into(),
                branch: if *is_main {
                    "main".into()
                } else {
                    "feat".into()
                },
                is_main: *is_main,
                sort_order: 0,
            })
            .collect();
        app.tree.agents = (0..3)
            .map(|i| Agent {
                id: AgentId(format!("a{i}")),
                worktree_id: WorktreeId(format!("w{i}")),
                name: format!("s{i}"),
                status: AgentStatus::Finished,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                sort_order: 0,
                status_changed_at: 1_000 * (i as i64 + 1),
                alive: true,
                recent_prompts: Vec::new(),
                usage_limit: None,
                automation: None,
            })
            .collect();
        app
    }

    fn project_names(app: &App) -> Vec<String> {
        app.project_rows()
            .iter()
            .map(|&i| app.tree.projects[i].name.clone())
            .collect()
    }

    /// Outside a stretch nothing is kept: a handler that changes the tree
    /// and then asks gets the answer for the tree it just changed.
    #[test]
    fn outside_a_stretch_every_answer_is_fresh() {
        let mut app = a_memo_tree();
        assert_eq!(project_names(&app), ["web", "api"]);
        app.tree.agents[0].status_changed_at = 9_000;
        assert_eq!(project_names(&app), ["api", "web"], "api just saw a turn");
    }

    /// A stretch keeps its rows only for the cursor and the tree's shape
    /// they were built for: moving a cursor or adding a row inside one
    /// is answered anew, never from the rows kept before it.
    #[test]
    fn a_stretch_rebuilds_for_a_moved_cursor_or_a_new_row() {
        let mut app = a_memo_tree();
        app.set_sel_project(1); // `api`, behind `web`
        app.rows_memo.arm();
        let ids = |app: &App| -> Vec<String> {
            app.visible_worktrees()
                .iter()
                .map(|w| w.id.0.clone())
                .collect()
        };
        assert_eq!(ids(&app), ["w0", "w1"]);
        let session = |app: &App| match app.selected_session_row() {
            Some(SessionRow::Agent(a)) => Some(a.id.0),
            _ => None,
        };
        assert_eq!(session(&app).as_deref(), Some("a0"));
        app.set_sel_worktree(1);
        assert_eq!(
            session(&app).as_deref(),
            Some("a1"),
            "the cursor moved to `feat` inside the stretch: its session, not the root's"
        );
        app.tree.worktrees.push(Worktree {
            id: WorktreeId("w3".into()),
            project_id: ProjectId("p0".into()),
            path: "/tmp/w3".into(),
            branch: "fix".into(),
            is_main: false,
            sort_order: 0,
        });
        assert_eq!(
            ids(&app),
            ["w0", "w1", "w3"],
            "a checkout arrived inside it"
        );
        app.rows_memo.disarm();
    }

    /// A frame is a stretch that ends with the frame: once it is drawn,
    /// the next question is answered fresh.
    #[test]
    fn a_frame_keeps_nothing_past_itself() {
        let mut app = a_memo_tree();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert_eq!(project_names(&app), ["web", "api"]);
        app.tree.agents[0].status_changed_at = 9_000;
        assert_eq!(project_names(&app), ["api", "web"]);
    }

    // ---- the pane's screen ----

    /// A screen that watched its process exit is rebuilt by whatever replay
    /// comes next, even one numbered as a continuation: the only bytes
    /// that can follow an exit are the next process's, from a ring of its
    /// own.
    #[test]
    fn an_exited_screen_is_rebuilt_by_the_next_replay() {
        let mut term = AttachedTerm::new(SessionRef::Agent(AgentId("a".into())), 20, 4);
        assert!(
            !term.apply_scrollback(0, b"old"),
            "a first replay onto an empty parser continues it"
        );
        term.exited = true;
        assert!(
            term.apply_scrollback(3, b"new"),
            "after an exit the screen starts over"
        );
        assert!(!term.exited, "the exit mark clears with it");
        let contents = term.parser.screen().contents();
        assert!(
            contents.contains("new") && !contents.contains("old"),
            "{contents:?}"
        );
        assert_eq!(term.next_seq, 6);
    }

    /// The "starting session…" notice is the daemon's empty replay, not the
    /// wait for the replay: unknown before it lands, booting after an empty
    /// one, gone with the first byte.
    #[test]
    fn an_empty_replay_means_booting_until_bytes_arrive() {
        let mut term = AttachedTerm::new(SessionRef::Agent(AgentId("a".into())), 20, 4);
        assert!(!term.booting, "before the daemon answers, nothing is known");
        term.apply_scrollback(0, b"");
        assert!(term.booting && !term.painted, "an empty replay: booting");
        term.apply_output(0, b"hi");
        assert!(term.painted && !term.booting, "the first bytes end it");
        assert_eq!(term.next_seq, 2);
    }

    // ---- shared list arithmetic ----

    #[test]
    fn window_start_slides_only_to_keep_the_cursor_visible() {
        assert_eq!(window_start(0, 5), 0);
        assert_eq!(window_start(4, 5), 0, "last row still fits");
        assert_eq!(window_start(5, 5), 1);
        assert_eq!(window_start(12, 5), 8);
        assert_eq!(window_start(3, 0), 4, "a zero-height list still computes");
    }

    #[test]
    fn clamp_selection_pins_to_the_list() {
        assert_eq!(clamp_selection(3, 0), 0, "empty list");
        assert_eq!(clamp_selection(-2, 4), 0);
        assert_eq!(clamp_selection(2, 4), 2);
        assert_eq!(clamp_selection(9, 4), 3);
    }

    #[test]
    fn max_scroll_and_scrolled_by_pin_the_pane() {
        assert_eq!(max_scroll(10, 4), 6);
        assert_eq!(max_scroll(3, 4), 0, "everything fits");
        assert_eq!(max_scroll(3, 0), 2, "a zero-height pane counts as one row");
        assert_eq!(scrolled_by(2, 3, 6), 5);
        assert_eq!(scrolled_by(2, 10, 6), 6);
        assert_eq!(scrolled_by(2, -10, 6), 0);
    }

    #[test]
    fn clamp_files_width_honors_both_minimums() {
        let area = Rect::new(10, 0, 100, 20);
        // Boundary at column 50 → 40 columns of list.
        assert_eq!(clamp_files_width(area, 50), Some(40));
        assert_eq!(
            clamp_files_width(area, 0),
            Some(MIN_DIFF_FILES_W),
            "left of the modal"
        );
        assert_eq!(
            clamp_files_width(area, 200),
            Some(100 - MIN_DIFF_PANE_W),
            "the right pane keeps its minimum"
        );
        let tiny = Rect::new(0, 0, MIN_DIFF_FILES_W + MIN_DIFF_PANE_W - 1, 20);
        assert_eq!(clamp_files_width(tiny, 5), None, "too small to honor both");
    }

    // ---- worktree links ----

    fn link_app() -> (App, WorktreeId) {
        let mut app = App::new();
        let project_id = ProjectId("p1".into());
        let worktree_id = WorktreeId("w1".into());
        app.tree.projects.push(Project {
            id: project_id.clone(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        });
        app.tree.worktrees.push(Worktree {
            id: worktree_id.clone(),
            project_id,
            path: "/tmp/demo".into(),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        });
        (app, worktree_id)
    }

    fn link(id: &str, worktree: &WorktreeId, url: &str, sort_order: i64) -> Link {
        Link {
            id: LinkId(id.into()),
            worktree_id: worktree.clone(),
            url: url.into(),
            sort_order,
        }
    }

    fn pr(url: &str) -> PullRequest {
        PullRequest {
            number: 7,
            url: url.into(),
            title: "Attach links".into(),
            answered_state: crate::pull_request::STATE_OPEN.into(),
            answered_draft: false,
            answered: Default::default(),
            activity: Vec::new(),
        }
    }

    #[test]
    fn saved_links_list_in_order_under_the_pull_request() {
        let (mut app, wt) = link_app();
        app.tree
            .links
            .push(link("l1", &wt, "https://a.dev/spec", 0));
        app.tree
            .links
            .push(link("l2", &wt, "https://b.dev/issue", 1));
        // Another worktree's links never leak into this list.
        app.tree
            .links
            .push(link("l3", &WorktreeId("other".into()), "https://c.dev", 0));

        let rows = app.visible_links();
        let urls: Vec<&str> = rows.iter().map(|l| l.url()).collect();
        assert_eq!(urls, ["https://a.dev/spec", "https://b.dev/issue"]);

        app.pull_requests
            .insert(wt, Some(pr("https://github.com/o/r/pull/7")));
        let rows = app.visible_links();
        assert_eq!(
            rows[0].url(),
            "https://github.com/o/r/pull/7",
            "the pull request leads the list"
        );
        assert_eq!(rows[0].label(), "#7 Attach links");
        assert!(rows[0].id().is_none(), "it is not a stored row");
        assert_eq!(rows.len(), 3);
    }

    /// A link the user pasted before orion found the PR is the same
    /// destination: it shows once, as the pull-request row, and stays
    /// deletable because it is still the user's own row.
    #[test]
    fn a_saved_link_matching_the_pull_request_is_shown_once() {
        let (mut app, wt) = link_app();
        let url = "https://github.com/o/r/pull/7";
        app.tree
            .links
            .push(link("l1", &wt, "https://a.dev/spec", 0));
        app.tree.links.push(link("l2", &wt, url, 1));
        app.pull_requests.insert(wt, Some(pr(url)));

        let rows = app.visible_links();
        assert_eq!(rows.len(), 2, "no duplicate row for the same URL");
        assert_eq!(rows[0].url(), url);
        assert_eq!(rows[0].label(), "#7 Attach links", "shown as the PR");
        assert_eq!(
            rows[0].id().map(|id| id.as_str()),
            Some("l2"),
            "still the stored row, so it can be edited and deleted"
        );
        assert_eq!(rows[1].url(), "https://a.dev/spec");
    }

    #[test]
    fn links_sit_between_terminals_and_archived_sessions() {
        let (mut app, wt) = link_app();
        app.tree.agents.push(Agent {
            id: AgentId("a1".into()),
            worktree_id: wt.clone(),
            name: "live".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            unseen: false,
            status_changed_at: 0,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: None,
        });
        app.tree.agents.push(Agent {
            id: AgentId("a2".into()),
            name: "old".into(),
            archived: true,
            ..app.tree.agents[0].clone()
        });
        app.tree.terminals.push(TerminalTab {
            id: TerminalId("t1".into()),
            worktree_id: wt.clone(),
            name: "shell".into(),
            sort_order: 0,
            alive: true,
            run_command: None,
            setup: false,
            run: Default::default(),
        });
        app.tree
            .links
            .push(link("l1", &wt, "https://a.dev/spec", 0));
        app.show_archived = true;

        let rows = app.visible_session_rows();
        let names: Vec<&str> = rows.iter().map(|r| r.name()).collect();
        assert_eq!(names, ["live", "shell", "https://a.dev/spec", "old"]);
    }

    #[test]
    fn link_rows_have_no_session_to_attach() {
        let (mut app, wt) = link_app();
        app.tree
            .links
            .push(link("l1", &wt, "https://a.dev/spec", 0));
        let rows = app.visible_session_rows();
        assert!(rows[0].sref().is_none(), "a link is not attachable");
        assert!(!rows[0].is_archived_agent());
    }

    #[test]
    fn pretty_url_strips_the_parts_nobody_reads() {
        assert_eq!(pretty_url("https://www.example.com/a/b"), "example.com/a/b");
        assert_eq!(pretty_url("http://x.dev/"), "x.dev");
        assert_eq!(pretty_url("https://x.dev"), "x.dev");
        // Not a URL shape we produce, but the function must not panic.
        assert_eq!(pretty_url(""), "");
    }

    /// Codex (ratatui inline viewport) inserts chat history by scrolling a
    /// TOP-ANCHORED DECSTBM region, which stock vt100 discards instead of
    /// saving — leaving nothing to scroll back to. This exercises the
    /// vendored vt100 patch through the real dependency, so it also fails if
    /// the `[patch.crates-io]` wiring is ever dropped.
    #[test]
    fn top_anchored_region_scroll_lands_in_scrollback() {
        let sref = SessionRef::Agent(AgentId::from("test-agent".to_string()));
        let mut term = AttachedTerm::new(sref, 80, 24);

        // Codex-style history insert: region rows 1..=10 (viewport below),
        // cursor at region bottom, newlines scroll history off the top.
        term.parser.process(b"\x1b[1;10r\x1b[10;1H");
        for i in 0..20 {
            term.parser
                .process(format!("history line {i}\r\n").as_bytes());
        }
        term.parser.process(b"\x1b[r");

        term.set_scroll(5);
        assert_eq!(
            term.parser.screen().scrollback(),
            5,
            "rows scrolled out of a top-anchored region must be recallable"
        );
        let top_row = term.parser.screen().contents();
        let top_row = top_row.lines().next().unwrap_or("");
        assert!(
            top_row.starts_with("history line"),
            "scrolled-back view should show an evicted history line, got {top_row:?}"
        );
    }

    /// The alternate screen (vim, htop) has no scrollback buffer, so region
    /// scrolls there must stay discarded even with the vendored patch.
    #[test]
    fn alternate_screen_region_scroll_stays_unscrollable() {
        let sref = SessionRef::Agent(AgentId::from("test-agent".to_string()));
        let mut term = AttachedTerm::new(sref, 80, 24);

        term.parser.process(b"\x1b[?1049h\x1b[1;10r\x1b[10;1H");
        for i in 0..20 {
            term.parser.process(format!("alt line {i}\r\n").as_bytes());
        }

        term.set_scroll(5);
        assert_eq!(
            term.parser.screen().scrollback(),
            0,
            "alternate screen must not accumulate scrollback"
        );
    }

    #[test]
    fn toggle_reviewed_sinks_marks_and_moves_the_selection() {
        let files = ["a", "b", "c"]
            .map(|p| DiffFile {
                path: p.into(),
                orig_path: None,
                xy: ['M', ' '],
                lines: None,
            })
            .to_vec();
        let mut v = DiffView::new("/nonexistent-review".into(), "main".into(), files, true);
        let order = |v: &DiffView| -> Vec<String> {
            v.matches
                .iter()
                .map(|m| v.files[m.file].path.clone())
                .collect()
        };

        // Mark the middle file: it sinks and the next file takes its row.
        v.select(1);
        assert_eq!(v.toggle_reviewed(), Some(true), "moved on to c");
        assert_eq!(order(&v), ["a", "c", "b"]);
        assert_eq!(v.selected_file().unwrap().path, "c");

        // Mark c too: the reviewed zone keeps git order and the selection
        // row lands in it (nothing unreviewed is left below c).
        assert_eq!(v.toggle_reviewed(), Some(true));
        assert_eq!(order(&v), ["a", "b", "c"]);
        assert_eq!(v.selected_file().unwrap().path, "b");

        // Unmark b: it pops back to its natural spot but the selection
        // advances to the next still-marked file (c), so repeated presses
        // clear a batch of marks.
        assert_eq!(v.toggle_reviewed(), Some(true), "advanced to c");
        assert_eq!(order(&v), ["a", "b", "c"]);
        assert_eq!(v.selected_file().unwrap().path, "c");
        assert_eq!(v.reviewed.len(), 1, "only c is still marked");

        // Unmark c — the last mark: nothing left to batch through, so the
        // selection follows the file back to its natural spot — same file,
        // no diff reload.
        assert_eq!(v.toggle_reviewed(), Some(false), "c stays selected");
        assert!(v.reviewed.is_empty());
        assert_eq!(order(&v), ["a", "b", "c"]);
        assert_eq!(v.selected_file().unwrap().path, "c");

        // With every other file reviewed, marking keeps the file selected —
        // there is nowhere further to advance.
        assert_eq!(v.toggle_reviewed(), Some(false), "c stays selected");
        v.select(1);
        assert_eq!(v.toggle_reviewed(), Some(false), "b stays selected");
        v.select(0);
        assert_eq!(v.toggle_reviewed(), Some(false), "a stays selected");
        assert_eq!(order(&v), ["a", "b", "c"]);
        assert_eq!(v.reviewed.len(), 3);

        // Batch unmark from the top of the reviewed zone: each press clears
        // the selected mark and lands on the next one down.
        assert_eq!(v.toggle_reviewed(), Some(true), "a cleared, on to b");
        assert_eq!(v.selected_file().unwrap().path, "b");
        assert_eq!(v.toggle_reviewed(), Some(true), "b cleared, on to c");
        assert_eq!(v.selected_file().unwrap().path, "c");
        assert_eq!(v.toggle_reviewed(), Some(false), "last mark, c stays");
        assert!(v.reviewed.is_empty());
        assert_eq!(order(&v), ["a", "b", "c"]);

        // No visible row (dead-end filter): toggling is a no-op.
        v.filter = "zzz".into();
        v.apply_filter();
        assert_eq!(v.toggle_reviewed(), None);
    }

    // ---- the recency rollups ----

    /// The one-pass rollups must answer exactly what the per-row functions
    /// answer, for every checkout and every project — including the ones
    /// with no sessions under them at all, which the maps have no entry for
    /// and the sorts read as a default stamp.
    ///
    /// They exist only to stop the row sorts re-walking every session per
    /// COMPARISON (`worktree_recencies`); the moment they disagree with
    /// `worktree_recency` the launcher silently reorders itself.
    #[test]
    fn the_one_pass_rollups_agree_with_the_per_row_stamps() {
        use orion_core::AgentKind;

        let mut tree = Tree::default();
        for p in 0..3 {
            tree.projects.push(Project {
                id: ProjectId(format!("p{p}")),
                name: format!("p{p}"),
                repo_path: format!("/tmp/p{p}").into(),
                sort_order: p,
            });
            for w in 0..3 {
                tree.worktrees.push(Worktree {
                    id: WorktreeId(format!("p{p}w{w}")),
                    project_id: ProjectId(format!("p{p}")),
                    path: format!("/tmp/p{p}w{w}").into(),
                    branch: if w == 0 {
                        "main".into()
                    } else {
                        format!("b{w}")
                    },
                    is_main: w == 0,
                    sort_order: w,
                });
            }
        }
        // Sessions on some checkouts and not others, live turns among them
        // (which count as interacting NOW, the tie the raw stamp breaks).
        for (i, wt) in ["p0w0", "p0w0", "p0w1", "p1w2", "p2w0"].iter().enumerate() {
            tree.agents.push(Agent {
                id: AgentId(format!("a{i}")),
                worktree_id: WorktreeId((*wt).into()),
                name: format!("a{i}"),
                status: if i % 2 == 0 {
                    AgentStatus::Finished
                } else {
                    AgentStatus::Running
                },
                archived: i == 4,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                sort_order: 0,
                status_changed_at: 100 * (i as i64 + 1),
                alive: true,
                recent_prompts: Vec::new(),
                usage_limit: None,
                automation: None,
            });
        }

        let by_worktree = worktree_recencies(&tree);
        for w in &tree.worktrees {
            assert_eq!(
                by_worktree.get(&w.id).copied().unwrap_or_default(),
                worktree_recency(&tree, &w.id),
                "checkout {:?}",
                w.id
            );
        }

        let by_project = project_recencies(&tree);
        for p in &tree.projects {
            assert_eq!(
                by_project.get(&p.id).copied().unwrap_or_default(),
                project_recency(&tree, &p.id),
                "project {:?}",
                p.id
            );
        }
    }
}
