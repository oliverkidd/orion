//! The LINEAR VIEW (`⌘L`): open Linear issues in two tabs — `My issues`,
//! assigned to you, and `Other issues`, the rest of your teams' — grouped
//! by status, a line per issue with its priority's letter, the reading
//! pane setting out its properties as Linear's sidebar does — who has it
//! and who reported it among them — and filtered
//! by words (a status, priority or label as well as the title), `key:value`
//! tokens and the FILTER PICK (`list_filter`).
//! Issues are picked together so one agent fixes them in one worktree and
//! opens one pull request. From the PULL REQUESTS MODAL the same list attaches a pull
//! request to the issues you mark (`attachmentLinkGitHubPR`, as a link
//! that closes them, so Linear's automations move them with it) — and the
//! other way round, `⌘U` here flips to that modal as a PR PICK, Enter on
//! a pull request attaching it to the issues marked here. Both ends run
//! the one ATTACH ([`attach_issues`]). `⌘.` links the marked issues to a
//! worktree instead ([`WorktreePick`]): the pull request its branch opens
//! later is attached to them as a ⌘L launch's is, or the one open on it
//! now there and then. `⌘S` on
//! an issue lists its team's workflow states in the reading pane's place
//! ([`PropPick`]) — read with the issues, so the list is up at once —
//! and Enter moves the issue to one (`issueUpdate`), the row saying so
//! before Linear has answered and put back if it refuses. `⌘P` sets its
//! priority and `⌘I` whose it is — you, nobody, or one of its team — the
//! same way ([`Change`]).
//!
//! The key is the project's `LINEAR_API_KEY` (`.env.local`, then `.env`,
//! then the process env). Only that one name is read. It is never logged,
//! never stored, never shown, and sent only to `api.linear.app` through
//! `curl --config -`. Settings → Linear gathers every option: **Link PRs
//! to Linear**, **Linear account** (whose issues are listed; empty = the
//! owner of that key), the **Task template**, and where the selected
//! project's key was found, with **Test connection** asking Linear whose
//! key it is ([`status_value`], [`test_connection`]).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use orion_core::{ClientRequest, ProjectId};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use crate::app::{clamp_selection, window_start, App, HitTarget, Overlay};
use crate::list_filter::{FacetKey, FilterPick, PickFacet, PickValue};
use crate::markdown::{self, Breaks};
use crate::pr_modal::PullRequestsView;
use crate::quick_prompt::{ModalUnder, QuickLaunch, QuickReturn, QuickTarget};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, empty_list_row, fuzzy_highlight_styled, layout_sections, list_header,
    panel_block, render_row, row_rect, search_line_idle, search_line_lit, sections, truncate,
    visible_positions, ListEntry, SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
};

const LIST_PCT: u16 = crate::pr_modal::LIST_PCT;
const MIN_LIST_W: u16 = crate::pr_modal::MIN_LIST_W;
const WHEEL_LINES: i32 = crate::pr_modal::WHEEL_LINES;
#[cfg(not(test))]
const TIMEOUT_SECS: &str = "20";
#[cfg(not(test))]
const LINEAR_URL: &str = "https://api.linear.app/graphql";
const KEY_NAME: &str = "LINEAR_API_KEY";
const ENV_FILES: &[&str] = &[".env.local", ".env"];

/// One Linear issue the view lists — open, or done within [`DONE_DAYS`]:
/// assigned to the configured user (`mine`), or someone else's or
/// nobody's in one of their teams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinearIssue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub description: String,
    pub status: String,
    #[serde(default)]
    pub status_type: String,
    /// The state's place in its team's workflow (Linear's `position`,
    /// scaled to an integer): what orders states of one type, so
    /// In Progress sits above In Staging.
    #[serde(default)]
    pub status_order: i64,
    /// The team the issue belongs to: whose workflow states it can move
    /// to (`LinearList::states`).
    #[serde(default)]
    pub team_id: String,
    /// Linear's priority: 0 none, 1 urgent, 2 high, 3 medium, 4 low.
    #[serde(default)]
    pub priority: u8,
    /// The workflow state's colour, as Linear's hex (`#f2c94c`).
    #[serde(default)]
    pub state_color: String,
    #[serde(default)]
    pub labels: Vec<LinearTag>,
    #[serde(default)]
    pub project: Option<LinearTag>,
    /// The assignee's display name; empty for nobody.
    #[serde(default)]
    pub assignee: String,
    /// The assignee's Linear id; empty for nobody.
    #[serde(default)]
    pub assignee_id: String,
    /// Who filed the issue, by display name — or, for one that came in
    /// through an integration (Slack, email), who asked there. Empty when
    /// Linear names neither.
    #[serde(default)]
    pub reporter: String,
    /// Assigned to the configured user: the `My issues` tab's, else
    /// `Other issues`'.
    #[serde(default)]
    pub mine: bool,
    /// RFC 3339.
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// When it was done; empty while it is open.
    #[serde(default)]
    pub completed_at: String,
    /// The GitHub pull requests Linear has on the issue — its GitHub
    /// integration's, or ones orion attached — as Linear last heard of
    /// them, open, merged or closed.
    #[serde(default)]
    pub prs: Vec<IssuePr>,
}

/// A pull request on a Linear issue, from the attachment's metadata:
/// enough for the work column to say where it stands without asking
/// GitHub ([`IssueWork`]) — for a pull request orion has heard nothing of
/// itself; one `App::prs` knows is drawn from there. Each part Linear
/// left out is unknown (`None`), never open, merged or clean by default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IssuePr {
    pub number: u64,
    pub url: String,
    /// The head branch; empty when Linear didn't say.
    #[serde(default)]
    pub branch: String,
    /// `open`, `merged` or `closed`, as Linear spells it.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub draft: Option<bool>,
    #[serde(default)]
    pub conflicts: Option<bool>,
}

impl IssuePr {
    /// A GitHub pull request attachment's node (`{ url metadata }`);
    /// `None` for any other kind of attachment.
    fn from_json(node: &serde_json::Value) -> Option<Self> {
        let url = node.get("url")?.as_str()?;
        let meta = node.get("metadata")?;
        let from_url = url
            .strip_prefix("https://github.com/")?
            .split("/pull/")
            .nth(1)?
            .split(['/', '#', '?'])
            .next()?
            .parse::<u64>()
            .ok();
        let number = meta.get("number").and_then(|n| n.as_u64()).or(from_url)?;
        let text = |key: &str| meta.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let flag = |key: &str| meta.get(key).and_then(|v| v.as_bool());
        Some(IssuePr {
            number,
            url: url.to_string(),
            branch: text("branch").unwrap_or_default(),
            state: text("status").filter(|state| !state.is_empty()),
            draft: flag("draft"),
            conflicts: flag("hasConflicts"),
        })
    }

    /// Where Linear's metadata says it stands; `None` when it didn't say.
    fn standing(&self) -> Option<crate::pull_request::Standing> {
        let state = self.state.as_ref()?;
        Some(crate::pull_request::Standing::of(
            &state.to_ascii_uppercase(),
            self.draft.unwrap_or(false),
        ))
    }
}

/// A label or a project: its name and Linear's hex colour for it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LinearTag {
    pub name: String,
    pub color: String,
}

/// Linear's word for a priority, as the rows and the filter say it.
pub fn priority_word(priority: u8) -> &'static str {
    match priority {
        1 => "Urgent",
        2 => "High",
        3 => "Medium",
        4 => "Low",
        _ => "No priority",
    }
}

/// Linear's priorities in the order the rows sort them and the FILTER PICK
/// lists them: urgent first, no priority last.
pub(crate) const PRIORITY_ORDER: [u8; 5] = [1, 2, 3, 4, 0];

/// What an issue nobody is assigned to says where a name would be.
const UNASSIGNED: &str = "unassigned";

/// Where a priority sorts: urgent first, no priority last.
pub(crate) fn priority_rank(priority: u8) -> usize {
    PRIORITY_ORDER
        .iter()
        .position(|p| *p == priority)
        .unwrap_or(PRIORITY_ORDER.len())
}

/// One of a team's workflow states — `Todo`, `In Progress`, `Done` —
/// with Linear's word for its kind (`unstarted`, `started`, `completed`,
/// …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinearState {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Linear's hex colour for it.
    #[serde(default)]
    pub color: String,
}

/// Someone an issue can be assigned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearUser {
    pub id: String,
    /// Their display name.
    pub name: String,
}

/// A property of an issue `issueUpdate` sets from here: `⌘S`, `⌘P`, `⌘I`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prop {
    Status,
    Priority,
    Assignee,
}

/// A value of one [`Prop`]: what a picker's row sets, and what a refusal
/// puts back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Status(LinearState),
    /// On Linear's scale ([`LinearIssue::priority`]).
    Priority(u8),
    /// `None` is nobody.
    Assignee(Option<LinearUser>),
}

impl Change {
    pub fn prop(&self) -> Prop {
        match self {
            Change::Status(_) => Prop::Status,
            Change::Priority(_) => Prop::Priority,
            Change::Assignee(_) => Prop::Assignee,
        }
    }

    /// What `issue`'s row says of `prop`. A state read off a row has no
    /// id: it is only ever put back, never sent.
    fn of(issue: &LinearIssue, prop: Prop) -> Self {
        match prop {
            Prop::Status => Change::Status(LinearState {
                id: String::new(),
                name: issue.status.clone(),
                kind: issue.status_type.clone(),
                color: issue.state_color.clone(),
            }),
            Prop::Priority => Change::Priority(issue.priority),
            Prop::Assignee => {
                Change::Assignee((!issue.assignee_id.is_empty()).then(|| LinearUser {
                    id: issue.assignee_id.clone(),
                    name: issue.assignee.clone(),
                }))
            }
        }
    }

    /// Whether `issue`'s row says this already.
    fn stands_on(&self, issue: &LinearIssue) -> bool {
        match self {
            Change::Status(state) => issue.status == state.name,
            Change::Priority(priority) => issue.priority == *priority,
            Change::Assignee(user) => {
                issue.assignee_id == user.as_ref().map_or("", |u| u.id.as_str())
            }
        }
    }

    /// Say it on `issue`'s row. `me` is the configured user's id: an
    /// issue assigned to them is the `My issues` tab's, any other the
    /// `Other issues`'.
    fn put_on(&self, issue: &mut LinearIssue, me: Option<&str>) {
        match self {
            Change::Status(state) => {
                issue.status = state.name.clone();
                issue.status_type = state.kind.clone();
                issue.state_color = state.color.clone();
            }
            Change::Priority(priority) => issue.priority = *priority,
            Change::Assignee(user) => {
                let (id, name) = user
                    .as_ref()
                    .map(|u| (u.id.clone(), u.name.clone()))
                    .unwrap_or_default();
                issue.mine = !id.is_empty() && me == Some(id.as_str());
                issue.assignee_id = id;
                issue.assignee = name;
            }
        }
    }

    /// What a refusal says.
    fn refused(&self, identifier: &str, why: &str) -> String {
        match self {
            Change::Status(_) => format!("couldn't move {identifier}: {why}"),
            Change::Priority(_) => format!("couldn't set {identifier}'s priority: {why}"),
            Change::Assignee(_) => format!("couldn't assign {identifier}: {why}"),
        }
    }
}

/// `⌘S`, `⌘P` or `⌘I`: the issue under the cursor, and what its status,
/// priority or assignee can be set to, in the reading pane's place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropPick {
    pub issue_id: String,
    pub identifier: String,
    pub prop: Prop,
    pub rows: Vec<Change>,
    pub selected: usize,
    /// The row the issue stands on, marked `current`.
    pub current: Option<usize>,
}

/// `⌘.`: the project's worktrees, in the reading pane's place, for the
/// marked issues (else the one under the cursor) to wait on. The pull
/// request the picked one's branch opens is attached to them
/// ([`link_worktree`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreePick {
    pub issues: Vec<LinearIssue>,
    pub branches: Vec<String>,
    pub selected: usize,
}

impl LinearIssue {
    pub fn label(&self) -> String {
        if self.title.trim().is_empty() {
            self.identifier.clone()
        } else {
            format!("{} {}", self.identifier, self.title)
        }
    }
}

/// The issues a ⌘L launch fixes together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearBatch {
    pub issues: Vec<LinearIssue>,
    pub task: String,
}

/// `ENG-12, ENG-15`: `issues` by identifier, as every title and message
/// names a set of them.
pub fn ids_of(issues: &[LinearIssue]) -> String {
    issues
        .iter()
        .map(|i| i.identifier.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl LinearBatch {
    pub fn ids(&self) -> String {
        ids_of(&self.issues)
    }

    pub fn title(&self) -> String {
        let ids = self.ids();
        if ids.is_empty() {
            "Linear".into()
        } else {
            format!("Linear {ids}")
        }
    }

    /// The fresh worktree's branch: one issue's is named after it; a
    /// batch takes the random name any other launch would, as a branch
    /// spelling out every identifier grew too long to read.
    pub fn branch(&self, taken: &[String]) -> String {
        match self.issues.as_slice() {
            [issue] => crate::branch_name::linear_name(&issue.identifier, &issue.title, taken),
            _ => crate::branch_name::random_name(taken),
        }
    }
}

/// `issues` written out in full, a markdown section each — identifier
/// and title, then state, priority and link, then the whole description
/// — as `{issues}` expands and as a typed task gets them appended
/// (`QuickLaunch::compose`). The agent works from this text alone: a
/// session needs no Linear access of its own to read what it is fixing.
pub fn issue_sections(issues: &[LinearIssue]) -> String {
    issues
        .iter()
        .map(|i| {
            let priority = if i.priority == 0 {
                ""
            } else {
                priority_word(i.priority)
            };
            let facts: Vec<&str> = [i.status.as_str(), priority, i.url.as_str()]
                .into_iter()
                .filter(|fact| !fact.is_empty())
                .collect();
            let desc = match i.description.trim() {
                "" => "(no description)",
                desc => desc,
            };
            format!(
                "### {}: {}\n{}\n\n{desc}",
                i.identifier,
                i.title,
                facts.join(" · ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What `{issues}` / `{ids}` / `{first_id}` expand to in the task template.
pub fn expand_template(template: &str, issues: &[LinearIssue]) -> String {
    let ids = ids_of(issues);
    let first = issues.first().map(|i| i.identifier.as_str()).unwrap_or("");
    template
        .replace("{issues}", &issue_sections(issues))
        .replace("{ids}", &ids)
        .replace("{first_id}", first)
}

/// Browse assigned issues, attach the current pull request to them, or
/// link one to a TODO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearMode {
    Browse,
    Attach {
        pr_url: String,
        pr_number: u64,
        back: Box<PullRequestsView>,
    },
    /// **Link existing…** from the TODOS MODAL: Enter links the issue
    /// under the cursor to `item` and goes back to `back`, Esc goes back
    /// with nothing linked.
    Link {
        item: u64,
        back: Box<crate::todos::TodoView>,
    },
}

/// The LINEAR VIEW's two lists: the issues assigned to you, and the rest
/// of your teams' open ones — someone else's, or nobody's yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinearTab {
    #[default]
    Mine,
    Others,
}

impl LinearTab {
    pub const ALL: [LinearTab; 2] = [LinearTab::Mine, LinearTab::Others];

    pub fn name(self) -> &'static str {
        match self {
            LinearTab::Mine => "My issues",
            LinearTab::Others => "Other issues",
        }
    }

    fn holds(self, issue: &LinearIssue) -> bool {
        issue.mine == (self == LinearTab::Mine)
    }

    fn other(self) -> Self {
        match self {
            LinearTab::Mine => LinearTab::Others,
            LinearTab::Others => LinearTab::Mine,
        }
    }
}

/// The modal's own state. The rows live on [`App::linear`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearView {
    pub project: ProjectId,
    pub project_name: String,
    pub dir: PathBuf,
    pub selected: usize,
    pub scroll: u16,
    pub view_height: u16,
    pub body_lines: usize,
    pub area: Rect,
    pub list_area: Rect,
    pub body_area: Rect,
    pub browser_area: Rect,
    pub query: TextInput,
    pub marked: BTreeSet<String>,
    pub mode: LinearMode,
    /// The status, priority or assignee picker, while it is up: every key
    /// but Esc is its own.
    pub prop_pick: Option<PropPick>,
    /// The worktree picker, while it is up: every key but Esc is its own.
    pub worktree_pick: Option<WorktreePick>,
    /// Which list shows: `My issues` or `Other issues`.
    pub tab: LinearTab,
    /// The tab strip's labels' screen x-ranges and its row, for the click.
    pub tab_hits: Vec<(u16, u16)>,
    pub tab_row: Rect,
    /// The first list entry drawn — a row or a status header — as of the
    /// last draw (`ui::stacked_rows`).
    pub list_start: usize,
    /// Each drawn row's rect, by index into the project's list, as of the
    /// last draw: what a click hit-tests.
    pub row_rects: Vec<(usize, Rect)>,
    /// The FILTER PICK (`⌘F`) in the reading pane's place, while it is
    /// up: every key is its own (`list_filter`).
    pub filter_pick: Option<FilterPick>,
    /// Which part of the list panel has the keys: the search line, where
    /// it opens and `space` types, or the rows, where `space` marks.
    pub focus: LinearFocus,
}

/// Where the LINEAR VIEW's keys go. `↓` off the search line hands them to
/// the rows, `↑` off the top row hands them back, and typing on the rows
/// goes back to the search line with what was typed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinearFocus {
    /// The search line: every key that edits a line is the filter's,
    /// `space` among them.
    #[default]
    Search,
    /// The rows: `space` marks the one under the cursor.
    List,
}

impl LinearView {
    pub fn new(project: ProjectId, project_name: String, dir: PathBuf, mode: LinearMode) -> Self {
        Self {
            project,
            project_name,
            dir,
            selected: 0,
            scroll: 0,
            view_height: 0,
            body_lines: 0,
            area: Rect::default(),
            list_area: Rect::default(),
            body_area: Rect::default(),
            browser_area: Rect::default(),
            query: TextInput::new(),
            marked: BTreeSet::new(),
            mode,
            prop_pick: None,
            worktree_pick: None,
            tab: LinearTab::Mine,
            tab_hits: Vec::new(),
            tab_row: Rect::default(),
            list_start: 0,
            row_rects: Vec::new(),
            filter_pick: None,
            focus: LinearFocus::Search,
        }
    }

    pub fn max_scroll(&self) -> u16 {
        crate::app::max_scroll(self.body_lines, self.view_height)
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let next = (self.scroll as i32 + delta).clamp(0, self.max_scroll() as i32);
        self.scroll = next as u16;
    }
}

/// What Linear last said about a project's issues — the open ones and
/// the ones done within [`DONE_DAYS`], the configured
/// user's and the rest of their teams' — and the workflow states of the
/// teams they belong to, by team id, in Linear's own order, with who the
/// user is and who is in their teams.
#[derive(Debug, Clone, Default)]
pub struct LinearList {
    pub list: Vec<LinearIssue>,
    pub states: HashMap<String, Vec<LinearState>>,
    /// The configured user — who `⌘I`'s `Me` assigns to. `None` when
    /// Linear knows nobody by Settings → Linear account.
    pub me: Option<LinearUser>,
    /// The members of each of the configured user's teams, by team id and
    /// then by name: who else `⌘I` can assign to.
    pub members: HashMap<String, Vec<LinearUser>>,
    /// Linear had more of the other issues than one page holds
    /// ([`OTHERS_LIMIT`]): the list says it shows the most recent.
    pub more: bool,
    /// The same for the configured user's own ([`MINE_LIMIT`]).
    pub more_mine: bool,
    /// The issues whose attachments ran past the page asked for: their
    /// `prs` are some of the pull requests, not all, so a landing list
    /// keeps the ones already known for them ([`keep_cut_prs`]).
    pub prs_cut: std::collections::HashSet<String>,
}

impl LinearList {
    /// The configured user's id.
    fn me_id(&self) -> Option<&str> {
        self.me.as_ref().map(|me| me.id.as_str())
    }

    /// Whether Linear had more of `tab`'s issues than the list holds.
    fn more_on(&self, tab: LinearTab) -> bool {
        match tab {
            LinearTab::Mine => self.more_mine,
            LinearTab::Others => self.more,
        }
    }
}

/// A finished Linear call, back on the loop.
#[derive(Debug, Clone)]
pub enum LinearAnswer {
    /// The project's lists (the ticket's key), asked from the checkout
    /// `dir` when the ticket says.
    List {
        ticket: crate::fetch::Ticket<ProjectId>,
        dir: PathBuf,
        list: Result<LinearList, String>,
    },
    /// The edit numbered `seq` — a `⌘S`, `⌘P` or `⌘I` — set `change` on
    /// an issue, or why not.
    Edited {
        project: ProjectId,
        issue_id: String,
        identifier: String,
        change: Change,
        seq: u64,
        result: Result<(), String>,
    },
    /// LINEAR AUTO-ATTACH linked a pull request to a link's issues —
    /// silent unless Linear refused.
    Attach(AttachRun),
    /// THE ATTACH the user asked for, from either end ([`attach_issues`]).
    Attached(AttachRun),
    /// The pull requests of the branches `project`'s issues moved off
    /// were taken off them ([`detach_stale`]): the first Linear would not
    /// drop, by issue identifier and why, when one was.
    Detached {
        project: ProjectId,
        dir: PathBuf,
        refused: Option<(String, String)>,
    },
    /// **Test connection**: who the key in `dir` belongs to.
    Viewer {
        dir: PathBuf,
        result: Result<Viewer, String>,
    },
    /// The TODOS MODAL's linked issues in the checkout the ticket names,
    /// as Linear has them now ([`request_linked`]): each with the
    /// identifier it was asked by, which a team move may since have
    /// renamed.
    Linked {
        ticket: crate::fetch::Ticket<PathBuf>,
        result: Result<Vec<(String, LinkedIssue)>, String>,
    },
    /// **Create in Triage** for `item` found more than one team and no
    /// remembered one: which to file it in is the user's to pick.
    Teams {
        dir: PathBuf,
        item: u64,
        result: Result<Vec<LinearTeam>, String>,
    },
    /// The issue **Create in Triage** made for `item` in `team`.
    Created {
        dir: PathBuf,
        item: u64,
        team: LinearTeam,
        result: Result<LinkedIssue, String>,
    },
}

/// An issue a TODO is linked to, as its chip draws it: what Linear says
/// of it, or what the LINEAR VIEW last read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedIssue {
    pub identifier: String,
    pub url: String,
    /// The state's name — `In Progress` — its kind — `started` — and its
    /// Linear colour.
    pub state: String,
    pub state_type: String,
    pub state_color: String,
    pub priority: u8,
}

impl LinkedIssue {
    pub fn of(issue: &LinearIssue) -> Self {
        Self {
            identifier: issue.identifier.clone(),
            url: issue.url.clone(),
            state: issue.status.clone(),
            state_type: issue.status_type.clone(),
            state_color: issue.state_color.clone(),
            priority: issue.priority,
        }
    }

    /// Done or canceled in Linear: a linked todo still open is ticked.
    pub fn finished(&self) -> bool {
        finished_kind(&self.state_type)
    }

    /// One `issue { identifier url state { name type color } priority }`
    /// node.
    fn from_json(v: &serde_json::Value) -> Option<Self> {
        let identifier = json_text(v, "/identifier");
        (!identifier.is_empty()).then(|| Self {
            identifier,
            url: json_text(v, "/url"),
            state: json_text(v, "/state/name"),
            state_type: json_text(v, "/state/type"),
            state_color: json_text(v, "/state/color"),
            priority: json_priority(v),
        })
    }
}

/// A state kind Linear counts as finished: done, or canceled.
pub(crate) fn finished_kind(kind: &str) -> bool {
    matches!(kind, "completed" | "canceled")
}

/// A Linear team **Create in Triage** can file into, with its triage
/// state when it takes issues into Triage at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearTeam {
    pub id: String,
    pub key: String,
    pub name: String,
    pub triage_state: Option<String>,
}

/// What **Create in Triage** files: the todo's text as the title, its
/// priority, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueDraft {
    pub title: String,
    pub priority: u8,
    pub description: String,
}

/// One run of attaches: a pull request linked to issues through
/// `attachmentLinkGitHubPR`, one after another, and what Linear said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachRun {
    /// The project whose issues they are, when orion knows it.
    pub project: Option<ProjectId>,
    /// The pull request's head branch, when orion knows it: what the
    /// [`LinkStore`] records against the issues.
    pub branch: Option<String>,
    pub pr_url: String,
    pub pr_number: u64,
    /// The issues Linear linked it to, as `(id, identifier)`.
    pub attached: Vec<(String, String)>,
    /// The attachments this run made, as `(issue id, attachment id)` —
    /// not those Linear had linked already. What a later move of the
    /// issue takes off again ([`LinkStore::remember`]).
    pub made: Vec<(String, String)>,
    /// The first issue it refused, by identifier, and why.
    pub refused: Option<(String, String)>,
}

impl AttachRun {
    fn ok(&self) -> bool {
        self.refused.is_none()
    }

    fn identifiers(&self) -> String {
        self.attached
            .iter()
            .map(|(_, identifier)| identifier.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What orion did to issues that a list asked before it may not know of
/// yet: the edits to a property (`⌘S`, `⌘P`, `⌘I`), by issue id and
/// property, and pull requests attached from here, by issue id.
/// Each is laid over every list that lands asked before Linear took it —
/// so a slow list asked before an edit can't put the old value back —
/// and is dropped by the first list asked after, which knows it
/// (`fetch`'s rule 1).
#[derive(Debug, Default)]
pub struct LocalEdits {
    moves: HashMap<(String, Prop), Move>,
    attached: HashMap<String, Vec<AttachedPr>>,
    /// Numbers every edit, so an answer knows whether a newer one has
    /// been asked for since.
    next_seq: u64,
}

/// The edits to one property of one issue: one sent to Linear at a time,
/// the last asked for queued behind it. Each property queues on its own,
/// so a `⌘P` never waits on a `⌘S`.
#[derive(Debug, Clone)]
struct Move {
    project: ProjectId,
    dir: PathBuf,
    identifier: String,
    /// What the row says: the last edit's, numbered `seq`.
    want: Change,
    seq: u64,
    /// The edit out at Linear now, by number; `None` once it answered.
    sending: Option<u64>,
    /// What Linear has, as far as orion knows: the row's before the
    /// first edit, then each one Linear took. What a refusal puts back.
    confirmed: Change,
    /// When Linear took the last edit, with none after it: a list asked
    /// later knows it. `None` while one is still to answer.
    landed: Option<std::time::Instant>,
}

/// A pull request attached to an issue from here, and when Linear took it.
#[derive(Debug, Clone)]
struct AttachedPr {
    project: ProjectId,
    pr: IssuePr,
    at: std::time::Instant,
}

impl LocalEdits {
    /// Lay what orion did over `project`'s list asked at `asked`: each
    /// edit or attach Linear had not taken by then put back on its issue,
    /// the rest forgotten — this list and every one after knows them.
    /// `me` is the configured user's id, as the list has it.
    fn lay_over(
        &mut self,
        project: &ProjectId,
        asked: std::time::Instant,
        list: &mut [LinearIssue],
        me: Option<&str>,
    ) {
        self.moves.retain(|(issue_id, _), mv| {
            if &mv.project != project {
                return true;
            }
            if mv.landed.is_some_and(|landed| asked > landed) {
                return false;
            }
            if let Some(issue) = list.iter_mut().find(|i| &i.id == issue_id) {
                mv.want.put_on(issue, me);
            }
            true
        });
        self.attached.retain(|issue_id, prs| {
            prs.retain(|a| &a.project != project || a.at >= asked);
            if let Some(issue) = list.iter_mut().find(|i| &i.id == issue_id) {
                for a in prs.iter().filter(|a| &a.project == project) {
                    if !issue.prs.iter().any(|p| p.url == a.pr.url) {
                        issue.prs.push(a.pr.clone());
                    }
                }
            }
            !prs.is_empty()
        });
    }
}

/// The TODOS MODAL's asks after its linked issues ([`request_linked`]):
/// one in flight per checkout, a `⌘R` meanwhile owed, and when what each
/// chip shows was asked.
#[derive(Debug, Default)]
pub struct LinkedAsks {
    flights: crate::fetch::Flights<PathBuf>,
    /// When each checkout's last answer was asked: a parked tab shown
    /// again asks again once that is older than the PULL REQUESTS
    /// MODAL's `FRESH`.
    dir_at: HashMap<PathBuf, std::time::Instant>,
    /// When the state each chip shows was asked, by checkout and
    /// identifier — by a [`fetch_linked`], or a `⌘S` Linear took. An
    /// answer asked before it is older news, dropped, so a chip never goes
    /// back. Per checkout, as each tab keeps its own chips.
    issue_at: HashMap<(PathBuf, String), std::time::Instant>,
}

impl LinkedAsks {
    /// `dir`'s chips were asked after within `FRESH`, or are being asked.
    pub(crate) fn fresh(&self, dir: &Path) -> bool {
        self.flights.in_flight(&dir.to_path_buf())
            || self
                .dir_at
                .get(dir)
                .is_some_and(|at| at.elapsed() < crate::pr_modal::FRESH)
    }

    /// Take a state of `identifier` asked at `asked` for `dir`'s chip —
    /// false when it already shows one asked later.
    pub(crate) fn accept(
        &mut self,
        dir: &Path,
        identifier: &str,
        asked: std::time::Instant,
    ) -> bool {
        let key = (dir.to_path_buf(), identifier.to_string());
        if self.issue_at.get(&key).is_some_and(|shown| *shown > asked) {
            return false;
        }
        self.issue_at.insert(key, asked);
        true
    }

    /// An answer for `dir` asked at `asked` landed.
    pub(crate) fn answered(&mut self, dir: &Path, asked: std::time::Instant) {
        let at = self.dir_at.entry(dir.to_path_buf()).or_insert(asked);
        *at = (*at).max(asked);
    }
}

/// The account a key belongs to, as Linear's `viewer` query names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub name: String,
    pub email: String,
}

/// Where **Test connection** stands for one project's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearTest {
    Testing,
    Passed(Viewer),
    Failed(String),
}

/// Where a project's `LINEAR_API_KEY` was found — never the key itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// One of the checkout's env files, by name.
    File(&'static str),
    /// orion's own environment.
    Environment,
}

impl KeySource {
    pub fn label(self) -> String {
        match self {
            KeySource::File(name) => format!("found in {name}"),
            KeySource::Environment => "found in orion's environment".into(),
        }
    }
}

/// Branch → Linear issues, per project, so a pull request cut from a ⌘L
/// launch, or from a worktree the issues were linked to (`⌘.`), can be
/// attached once GitHub lists it. A link outlives its attach: the LINEAR
/// VIEW still finds the worktree and pull request of every issue on it
/// by that branch ([`IssueWork`]) — while there is a branch to find:
/// a link not touched in [`LINK_KEEP_DAYS`] whose branch is in no
/// checkout and on no open pull request is forgotten
/// ([`prune`](Self::prune)), so the store holds the work in hand and
/// never every branch there ever was.
///
/// An issue is on one link at a time — ONE HOME: linking it to a branch
/// takes it off the one it was on, and hands back the attachment orion
/// made for that branch's pull request, for Linear to drop
/// ([`detach_stale`]). Linear moves an issue when the pull requests that
/// close it merge, so one left on a branch it has moved off would move
/// it for work done elsewhere. A pull request may still close several
/// issues.
///
/// Each issue on a link is attached or waiting, on its own. Waiting ones
/// are spent only when Linear takes them:
/// [`begin_attach`](Self::begin_attach) marks the link out and
/// [`finish_attach`](Self::finish_attach) settles it — those Linear took
/// attached, the rest back to wait for the next list, until
/// [`ATTACH_TRIES`] refusals in a row leave the link be.
#[derive(Debug, Clone, Default)]
pub struct LinkStore {
    path: Option<PathBuf>,
    /// Keyed by [`link_key`]. A link written before links were kept per
    /// project is keyed by its branch alone, and any project's branch of
    /// that name takes it, as then.
    links: HashMap<String, PendingLink>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PendingLink {
    issue_ids: Vec<String>,
    identifiers: Vec<String>,
    /// None of its issues waits: kept up to date for an older orion,
    /// which reads only this. A link written with it and no
    /// `attached_ids` had every issue attached ([`LinkStore::load`]).
    #[serde(default)]
    attached: bool,
    /// The issues a pull request from the branch took; the rest wait.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attached_ids: Vec<String>,
    /// The project whose branch it is; `None` on an older link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<ProjectId>,
    /// The branch; `None` on an older link, whose key it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    /// The attachments orion's own asks made, by issue id: the ones it
    /// may take off again. One Linear made itself is never here.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    attachments: HashMap<String, String>,
    /// Attaches Linear refused in a row.
    #[serde(default, skip_serializing_if = "is_zero")]
    failures: u8,
    /// When issues were last linked to it, in unix seconds: what
    /// [`LinkStore::prune`] ages it by. `None` on a link written before
    /// links were dated, which [`LinkStore::load`] dates from that load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    linked_at: Option<u64>,
    /// An attach is out for it now: not tried again until it answers.
    #[serde(skip)]
    sending: bool,
}

impl PendingLink {
    /// The issues still waiting on a pull request, as `(id, identifier)`.
    fn waiting(&self) -> Vec<(String, String)> {
        self.issue_ids
            .iter()
            .zip(&self.identifiers)
            .filter(|(id, _)| !self.attached_ids.contains(id))
            .map(|(id, identifier)| (id.clone(), identifier.clone()))
            .collect()
    }

    fn gave_up(&self) -> bool {
        self.failures >= ATTACH_TRIES
    }

    /// `(id, identifier)`s added after the issues already on it.
    fn add(&mut self, issues: &[(String, String)]) {
        for (id, identifier) in issues {
            if !self.issue_ids.contains(id) {
                self.issue_ids.push(id.clone());
                self.identifiers.push(identifier.clone());
            }
        }
    }

    /// `attached` brought in line with `attached_ids`.
    fn settle(&mut self) {
        self.attached = self.waiting().is_empty();
    }

    /// `ids` attached, and of `made` (`(issue id, attachment id)`) the
    /// attachments of the issues on it kept.
    fn attach(&mut self, ids: &[String], made: &[(String, String)]) {
        for id in ids {
            if self.issue_ids.contains(id) && !self.attached_ids.contains(id) {
                self.attached_ids.push(id.clone());
            }
        }
        for (id, attachment) in made {
            if self.issue_ids.contains(id) {
                self.attachments.insert(id.clone(), attachment.clone());
            }
        }
        self.settle();
    }

    /// Takes the issues `ids` off it; the attachments orion made for
    /// them.
    fn drop_issues(&mut self, ids: &[String]) -> Vec<StaleAttachment> {
        let mut stale = Vec::new();
        let issues = std::mem::take(&mut self.issue_ids)
            .into_iter()
            .zip(std::mem::take(&mut self.identifiers));
        for (issue, identifier) in issues {
            if !ids.contains(&issue) {
                self.issue_ids.push(issue);
                self.identifiers.push(identifier);
                continue;
            }
            self.attached_ids.retain(|attached| *attached != issue);
            if let Some(id) = self.attachments.remove(&issue) {
                stale.push(StaleAttachment { identifier, id });
            }
        }
        self.settle();
        stale
    }
}

/// An attachment orion made for a branch its issue has since moved off:
/// Linear's to drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleAttachment {
    /// The issue's, to say which a refusal is about.
    identifier: String,
    id: String,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

/// How many attaches in a row Linear may refuse a link before it is left
/// alone: a refusal that sticks (the issue gone, a key without access)
/// must not ask again on every list. Linking the branch again tries anew.
const ATTACH_TRIES: u8 = 3;

/// How long a link outlives its branch: one last linked longer ago than
/// this, whose branch no checkout is on and no open pull request is from,
/// is forgotten ([`LinkStore::prune`]). Long enough that a branch parked
/// for a quarter and checked out again still finds its issues.
const LINK_KEEP_DAYS: u64 = 90;

/// A link's key: the branch, then the project. Git never puts a `:` in a
/// branch name, so it never reads as an older link's bare branch.
fn link_key(project: &ProjectId, branch: &str) -> String {
    format!("{branch}:{}", project.0)
}

impl LinkStore {
    pub fn load(path: PathBuf) -> Self {
        let mut links: HashMap<String, PendingLink> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        // A link from before issues were attached one by one: `attached`
        // spoke for all of them.
        for link in links.values_mut() {
            if link.attached && link.attached_ids.is_empty() {
                link.attached_ids = link.issue_ids.clone();
            }
        }
        // And one from before links were dated: its age counts from now,
        // written down so the next launch does not start it again.
        let now = orion_core::clock::now_secs();
        let undated = links.values().any(|l| l.linked_at.is_none());
        for link in links.values_mut() {
            link.linked_at.get_or_insert(now);
        }
        let store = Self {
            path: Some(path),
            links,
        };
        if undated {
            store.persist();
        }
        store
    }

    /// Forget `project`'s links last linked more than [`LINK_KEEP_DAYS`]
    /// before `now` (unix seconds) whose branch is not in `live` — the
    /// branches the project's checkouts are on and its open pull requests
    /// are from. An attach that is out keeps its link, and so does a link
    /// from before they were kept per project, which may be another
    /// project's. True when any went.
    pub(crate) fn prune(
        &mut self,
        project: &ProjectId,
        live: &std::collections::HashSet<String>,
        now: u64,
    ) -> bool {
        let before = self.links.len();
        self.links.retain(|_, link| {
            let (Some(branch), Some(linked_at)) = (&link.branch, link.linked_at) else {
                return true;
            };
            link.project.as_ref() != Some(project)
                || link.sending
                || live.contains(branch)
                || now.saturating_sub(linked_at) <= LINK_KEEP_DAYS * 24 * 60 * 60
        });
        let pruned = self.links.len() != before;
        if pruned {
            self.persist();
        }
        pruned
    }

    /// The key of `project`'s link on `branch`: its own, else an older
    /// one kept for any project.
    fn key_of(&self, project: &ProjectId, branch: &str) -> Option<String> {
        let own = link_key(project, branch);
        if self.links.contains_key(&own) {
            return Some(own);
        }
        self.links
            .get(branch)
            .filter(|l| l.project.is_none())
            .map(|_| branch.to_string())
    }

    /// `project`'s link on `branch`, made empty if it has none.
    fn link_mut(&mut self, project: &ProjectId, branch: &str) -> &mut PendingLink {
        let key = self
            .key_of(project, branch)
            .unwrap_or_else(|| link_key(project, branch));
        self.links.entry(key).or_insert_with(|| PendingLink {
            project: Some(project.clone()),
            branch: Some(branch.to_string()),
            linked_at: Some(orion_core::clock::now_secs()),
            ..PendingLink::default()
        })
    }

    /// ONE HOME: the issues `ids` taken off every link but `project`'s on
    /// `branch`, a link left with none forgotten. The attachments orion
    /// made for the branches they left.
    fn rehome(
        &mut self,
        project: &ProjectId,
        branch: &str,
        ids: &[String],
    ) -> Vec<StaleAttachment> {
        let home = self.key_of(project, branch);
        let mut stale = Vec::new();
        self.links.retain(|key, link| {
            if Some(key) == home.as_ref() {
                return true;
            }
            stale.extend(link.drop_issues(ids));
            !link.issue_ids.is_empty()
        });
        stale
    }

    /// Whether `issue_id` is on `project`'s link on `branch`.
    fn holds(&self, project: &ProjectId, branch: &str, issue_id: &str) -> bool {
        self.key_of(project, branch)
            .and_then(|key| self.links.get(&key))
            .is_some_and(|link| link.issue_ids.iter().any(|id| id == issue_id))
    }

    /// Sets `issues` waiting on `branch`'s next pull request, beside the
    /// issues already on it — those a pull request took stay on it,
    /// attached. Linking an issue again sets it waiting again, and gives
    /// the link a fresh set of tries. The branch is now each issue's ONE
    /// HOME: what they left behind on another comes back
    /// ([`rehome`](Self::rehome)).
    pub fn remember(
        &mut self,
        project: &ProjectId,
        branch: &str,
        issues: &[LinearIssue],
    ) -> Vec<StaleAttachment> {
        if branch.is_empty() || issues.is_empty() {
            return Vec::new();
        }
        let pairs: Vec<(String, String)> = issues
            .iter()
            .map(|i| (i.id.clone(), i.identifier.clone()))
            .collect();
        let link = self.link_mut(project, branch);
        link.add(&pairs);
        link.linked_at = Some(orion_core::clock::now_secs());
        link.attached_ids
            .retain(|id| !pairs.iter().any(|(again, _)| again == id));
        link.failures = 0;
        link.settle();
        let ids: Vec<String> = pairs.into_iter().map(|(id, _)| id).collect();
        let stale = self.rehome(project, branch, &ids);
        self.persist();
        stale
    }

    /// Issues `branch`'s open pull request was attached to on the spot:
    /// kept for show beside the rest, waiting on nothing. Issues still
    /// waiting on it keep waiting, for the next list to attach. `made`
    /// is the attachments the ask made (`(issue id, attachment id)`).
    /// The branch is now each issue's ONE HOME, as
    /// [`remember`](Self::remember)'s is.
    pub(crate) fn remember_attached(
        &mut self,
        project: &ProjectId,
        branch: &str,
        issues: &[(String, String)],
        made: &[(String, String)],
    ) -> Vec<StaleAttachment> {
        if branch.is_empty() || issues.is_empty() {
            return Vec::new();
        }
        let ids: Vec<String> = issues.iter().map(|(id, _)| id.clone()).collect();
        let link = self.link_mut(project, branch);
        link.add(issues);
        link.linked_at = Some(orion_core::clock::now_secs());
        link.attach(&ids, made);
        let stale = self.rehome(project, branch, &ids);
        self.persist();
        stale
    }

    /// The identifiers waiting on `branch`'s pull request — none once
    /// Linear has refused the link [`ATTACH_TRIES`] times
    /// ([`gave_up_on`](Self::gave_up_on)).
    pub fn pending(&self, project: &ProjectId, branch: &str) -> Vec<String> {
        self.key_of(project, branch)
            .and_then(|key| self.links.get(&key))
            .filter(|l| !l.gave_up())
            .map(|l| l.waiting().into_iter().map(|(_, ident)| ident).collect())
            .unwrap_or_default()
    }

    /// The identifiers on `branch` that Linear refused [`ATTACH_TRIES`]
    /// times in a row, no longer tried.
    pub fn gave_up_on(&self, project: &ProjectId, branch: &str) -> Vec<String> {
        self.key_of(project, branch)
            .and_then(|key| self.links.get(&key))
            .filter(|l| l.gave_up())
            .map(|l| l.waiting().into_iter().map(|(_, ident)| ident).collect())
            .unwrap_or_default()
    }

    /// The issues waiting on `branch`'s pull request, as `(id,
    /// identifier)`: the link marked out until
    /// [`finish_attach`](Self::finish_attach). `None` when nothing waits
    /// on it, an attach is already out, or Linear has refused it
    /// [`ATTACH_TRIES`] times.
    pub(crate) fn begin_attach(
        &mut self,
        project: &ProjectId,
        branch: &str,
    ) -> Option<Vec<(String, String)>> {
        let key = self.key_of(project, branch)?;
        let link = self
            .links
            .get_mut(&key)
            .filter(|l| !l.sending && !l.gave_up())?;
        let waiting = link.waiting();
        if waiting.is_empty() {
            return None;
        }
        link.sending = true;
        Some(waiting)
    }

    /// Linear answered the attach [`begin_attach`](Self::begin_attach)
    /// sent out: the issues it took (`attached`, by id) are attached —
    /// `made` the attachments the ask made for them, as `(issue id,
    /// attachment id)` — and any it refused, or that were added
    /// meanwhile, wait for the next list. A refusal counts against the
    /// link's tries; true when it was the last.
    pub(crate) fn finish_attach(
        &mut self,
        project: &ProjectId,
        branch: &str,
        attached: &[String],
        made: &[(String, String)],
        ok: bool,
    ) -> bool {
        let Some(link) = self
            .key_of(project, branch)
            .and_then(|key| self.links.get_mut(&key))
        else {
            return false;
        };
        link.sending = false;
        link.attach(attached, made);
        let gave_up = if ok {
            link.failures = 0;
            false
        } else {
            link.failures = link.failures.saturating_add(1);
            link.gave_up()
        };
        self.persist();
        gave_up
    }

    /// The branch `issue_id` is linked to — more than one only in a store
    /// written before ONE HOME — in name order.
    pub fn branches_of(&self, issue_id: &str) -> Vec<&str> {
        let mut branches: Vec<&str> = self
            .links
            .iter()
            .filter(|(_, l)| l.issue_ids.iter().any(|id| id == issue_id))
            .map(|(key, l)| l.branch.as_deref().unwrap_or(key))
            .collect();
        branches.sort_unstable();
        branches.dedup();
        branches
    }

    fn persist(&self) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.links) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// What is going on with one issue, as the work column on a row and the
/// right end of the page's border say it: the pull request that has it,
/// wherever it was opened, and the orion worktree on it with its
/// sessions' mark — a PR row with a worktree shows both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueWork {
    /// The branch of the orion worktree on the issue; `None` when no
    /// checkout is, as for a pull request opened outside orion.
    pub branch: Option<String>,
    /// The sessions' rollup and whether a finish there is unread; `None`
    /// with no session on the worktree.
    pub session: Option<(orion_core::entities::AgentStatus, bool)>,
    pub pr: Option<WorkPr>,
}

/// The pull request in [`IssueWork`], as a PR ROW paints it
/// ([`crate::pr_row::look`]). `standing` is `None` for one nobody has said
/// the state of — Linear's metadata left it out and orion has not heard of
/// it — drawn neutrally, with no word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkPr {
    pub number: u64,
    pub standing: Option<crate::pull_request::Standing>,
    pub trouble: Option<crate::pull_request::Trouble>,
}

impl WorkPr {
    /// `ready`, `draft`, `merged`, `closed`, or the trouble's word; nothing
    /// for a state nobody has said.
    fn word(self) -> &'static str {
        self.standing
            .map_or("", |standing| standing.word(self.trouble))
    }

    /// Its colours: the PR ROW's for its state, or dim end to end for a
    /// state nobody has said — neither the open muted nor any status.
    fn look(self, th: Theme) -> crate::pr_row::Look {
        match self.standing {
            Some(standing) => crate::pr_row::look(standing, self.trouble, th),
            None => crate::pr_row::Look {
                glyph: th.dim,
                label: th.dim,
                rail: th.dim,
                badge: th.dim,
            },
        }
    }

    /// Open before merged before closed before unknown, then the newest.
    fn rank(self) -> (u8, std::cmp::Reverse<u64>) {
        use crate::pull_request::Standing;
        let state = match self.standing {
            Some(Standing::Open | Standing::Draft) => 0,
            Some(Standing::Merged) => 1,
            Some(Standing::Closed) => 2,
            None => 3,
        };
        (state, std::cmp::Reverse(self.number))
    }
}

/// Whether `branch` names the issue `identifier` (`ENG-12`): the
/// identifier, any case, with no letter or digit before it and no digit
/// after — `eng-12-fix` and `feat/ENG-12` do, `eng-123` doesn't.
fn names_issue(branch: &str, identifier: &str) -> bool {
    let (branch, id) = (branch.to_ascii_lowercase(), identifier.to_ascii_lowercase());
    if id.is_empty() {
        return false;
    }
    branch.match_indices(&id).any(|(at, _)| {
        let before = branch[..at].chars().next_back();
        let after = branch[at + id.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// [`IssueWork`] for `issue` in `project`; `None` when nothing has it.
///
/// The worktree is a checkout on a branch the issue was linked to (`⌘.`, a
/// ⌘L launch), else one whose branch names it (`fix/eng-12-…`), else one
/// on an attached pull request's branch. A link whose checkout is gone
/// shows no worktree, but still finds its pull request.
///
/// The pull request is the best of those Linear has on the issue and the
/// project's open ones on its branches — open before merged before closed.
/// Where each stands is `App::prs`'s word for any pull request orion has
/// heard of, failing checks and all — the same the list row and the band
/// draw; Linear's metadata speaks only for one it never has. One from the
/// root checkout's branch (the `dev` → `main` release) is never the
/// issue's ([`root_branch`]).
pub(crate) fn work_of(app: &App, project: &ProjectId, issue: &LinearIssue) -> Option<IssueWork> {
    use crate::pull_request::Trouble;
    use orion_core::entities::Worktree;
    let known = |number: u64, url: &str| {
        app.prs.status(url).map(|status| WorkPr {
            number,
            standing: Some(status.standing),
            trouble: status.trouble(),
        })
    };
    let worktrees: Vec<&Worktree> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project && !w.is_main)
        .collect();
    let checkout = |branch: &str| worktrees.iter().copied().find(|w| w.branch == branch);
    let root = root_branch(app, project);
    let not_root = |branch: &str| root != Some(branch);
    let attached: Vec<&IssuePr> = issue.prs.iter().filter(|p| not_root(&p.branch)).collect();
    let linked = app.linear_links.branches_of(&issue.id);
    let worktree = linked
        .iter()
        .find_map(|b| checkout(b))
        .or_else(|| {
            worktrees
                .iter()
                .copied()
                .find(|w| names_issue(&w.branch, &issue.identifier))
        })
        .or_else(|| attached.iter().find_map(|p| checkout(&p.branch)));
    let session = worktree.and_then(|wt| {
        let agents: Vec<_> = app
            .tree
            .agents
            .iter()
            .filter(|a| a.worktree_id == wt.id && !a.archived)
            .collect();
        let status = crate::app::rollup(agents.iter().map(|a| a.status))?;
        Some((status, agents.iter().any(|a| a.unseen)))
    });
    let open = app
        .open_prs
        .get(project)
        .map(|o| o.list.as_slice())
        .unwrap_or_default();
    let ours = |head: &str| {
        not_root(head)
            && (worktree.is_some_and(|w| w.branch == head)
                || linked.contains(&head)
                || names_issue(head, &issue.identifier))
    };
    let live = open
        .iter()
        .filter(|pr| ours(&pr.head) || attached.iter().any(|p| p.url == pr.url))
        .map(|pr| {
            known(pr.number, &pr.url).unwrap_or(WorkPr {
                number: pr.number,
                standing: Some(crate::pull_request::Standing::Open),
                trouble: None,
            })
        });
    let from_linear = attached
        .iter()
        .filter(|p| !open.iter().any(|pr| pr.url == p.url))
        .map(|p| {
            known(p.number, &p.url).unwrap_or_else(|| {
                let standing = p.standing();
                let conflicts = standing.is_some_and(|s| s.is_open()) && p.conflicts == Some(true);
                WorkPr {
                    number: p.number,
                    standing,
                    trouble: conflicts.then_some(Trouble::Conflicts),
                }
            })
        });
    let pr = live.chain(from_linear).min_by_key(|p| p.rank());
    if worktree.is_none() && pr.is_none() {
        return None;
    }
    Some(IssueWork {
        branch: worktree.map(|w| w.branch.clone()),
        session,
        pr,
    })
}

/// `⌘L` on the grid: browse assigned issues for the selected project.
pub(crate) fn open(app: &mut App) {
    let Some(project) = app.selected_project().cloned() else {
        return;
    };
    open_on(
        app,
        project.id,
        project.name,
        project.repo_path,
        LinearMode::Browse,
    );
}

/// `⌘L` in the PULL REQUESTS MODAL: the same list, for attaching the PR.
pub(crate) fn open_attach(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let (project, name, dir) = (
        view.project.clone(),
        view.project_name.clone(),
        view.dir.clone(),
    );
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
        return;
    };
    let back = match &app.overlay {
        Some(Overlay::PullRequests(view)) => view.clone(),
        _ => return,
    };
    open_on(
        app,
        project,
        name,
        dir,
        LinearMode::Attach {
            pr_url: pr.url,
            pr_number: pr.number,
            back: Box::new(back),
        },
    );
}

/// **Link existing…** in the TODOS MODAL: the list, for picking the issue
/// `item` is linked to; `back` is the modal to go back to.
pub(crate) fn open_link(app: &mut App, item: u64, back: crate::todos::TodoView) {
    open_on(
        app,
        back.project.clone(),
        back.project_name.clone(),
        back.dir.clone(),
        LinearMode::Link {
            item,
            back: Box::new(back),
        },
    );
}

fn open_on(app: &mut App, project: ProjectId, name: String, dir: PathBuf, mode: LinearMode) {
    let mut view = LinearView::new(project.clone(), name, dir.clone(), mode);
    view.selected = clamp_selection(0, list_len(app, &project));
    app.overlay = Some(Overlay::Linear(view));
    request_list(app, project, dir, false);
    app.dirty = true;
}

pub(crate) fn reopen(app: &mut App, mut view: LinearView) {
    view.selected = clamp_selection(view.selected as i64, list_len(app, &view.project));
    app.overlay = Some(Overlay::Linear(view));
    app.dirty = true;
}

fn list_len(app: &App, project: &ProjectId) -> usize {
    app.linear.get(project).map_or(0, |l| l.list.len())
}

/// Ask Linear for `project`'s lists, off the loop — one ask at a time per
/// project. `fresh` is a refresh the user asked for (`⌘R`), or one an
/// edit needs: while a list is out, it is owed and asked as soon as that
/// one lands, never dropped.
pub(crate) fn request_list(app: &mut App, project: ProjectId, dir: PathBuf, fresh: bool) {
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    let now = crate::fetch::now();
    let ticket = if fresh {
        app.linear_flights.begin_fresh(project.clone(), now)
    } else {
        app.linear_flights.begin(project.clone(), now)
    };
    let Some(ticket) = ticket else {
        return;
    };
    app.linear_failed.remove(&project);
    app.dirty = true;
    let email = crate::config::Config::load()
        .linear_assignee_email
        .trim()
        .to_string();
    tokio::spawn(async move {
        let list = fetch_lists(&dir, &email).await;
        let _ = tx.send(LinearAnswer::List { ticket, dir, list });
    });
}

/// The LINEAR VIEW over `overlay`, wherever it stands: up, under the PR
/// PICK its `⌘U` opened, or under the QUICK PROMPT its Enter opened.
fn linear_view_mut(overlay: &mut Option<Overlay>) -> Option<&mut LinearView> {
    use crate::app::PromptKind;
    match overlay.as_mut()? {
        Overlay::Linear(view) => Some(view),
        Overlay::PullRequests(prs) => prs.pick.as_mut().map(|pick| pick.back.as_mut()),
        Overlay::Prompt(prompt) => match &mut prompt.kind {
            PromptKind::QuickPrompt(launch) => match &mut launch.under {
                Some(ModalUnder::Linear(view)) => Some(view.as_mut()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// A new list for `project` landed over `old`: the cursor stays on the
/// issue it was on, wherever that now sits, and is clamped only when the
/// issue has left the list.
fn follow_cursor(app: &mut App, project: &ProjectId, old: &[LinearIssue]) {
    let list = app
        .linear
        .get(project)
        .map(|l| l.list.as_slice())
        .unwrap_or_default();
    let Some(view) = linear_view_mut(&mut app.overlay) else {
        return;
    };
    if &view.project != project {
        return;
    }
    let was = old.get(view.selected).map(|issue| issue.id.as_str());
    view.selected = was
        .and_then(|id| list.iter().position(|issue| issue.id == id))
        .unwrap_or_else(|| clamp_selection(view.selected as i64, list.len()));
}

/// An issue whose attachments ran past the page keeps the pull requests
/// the last list knew it had: some of them is not none of them.
fn keep_cut_prs(fresh: &mut LinearList, old: &[LinearIssue]) {
    for issue in fresh
        .list
        .iter_mut()
        .filter(|issue| fresh.prs_cut.contains(&issue.id))
    {
        let Some(was) = old.iter().find(|o| o.id == issue.id) else {
            continue;
        };
        for pr in &was.prs {
            if !issue.prs.iter().any(|p| p.url == pr.url) {
                issue.prs.push(pr.clone());
            }
        }
    }
}

pub(crate) fn land_answer(app: &mut App, answer: LinearAnswer) {
    match answer {
        LinearAnswer::Linked { ticket, result } => {
            let Some(landed) = app.linear_linked.flights.land(&ticket) else {
                return;
            };
            let dir = ticket.key.clone();
            crate::todos::view::land_linked(app, dir.clone(), ticket.at, result);
            if landed.owed {
                let ids = crate::todos::view::linked_ids(app, &dir);
                request_linked(app, dir, ids);
            }
        }
        LinearAnswer::Teams { dir, item, result } => {
            crate::todos::view::land_teams(app, dir, item, result)
        }
        LinearAnswer::Created {
            dir,
            item,
            team,
            result,
        } => crate::todos::view::land_created(app, dir, item, team, result),
        LinearAnswer::Viewer { dir, result } => {
            let source = key_source(&dir);
            let test = match result {
                Ok(viewer) => LinearTest::Passed(viewer),
                Err(err) => LinearTest::Failed(err),
            };
            // The overlay's EXPLANATION line says it in full, where the
            // row's value column would cut a long error short.
            if let Some(Overlay::Settings(view)) = &mut app.overlay {
                match &test {
                    LinearTest::Passed(_) => view.info(format!("Linear: {}", test_label(&test))),
                    LinearTest::Failed(err) => view.warn(format!("Linear: {err}")),
                    LinearTest::Testing => {}
                }
            }
            app.linear_test = Some((dir, source, test));
            app.dirty = true;
        }
        LinearAnswer::List { ticket, dir, list } => {
            // A list asked again since (or never asked) answers nothing.
            let Some(landed) = app.linear_flights.land(&ticket) else {
                return;
            };
            let project = ticket.key.clone();
            match list {
                Ok(mut fetched) => {
                    let old = app.linear.remove(&project).unwrap_or_default();
                    keep_cut_prs(&mut fetched, &old.list);
                    let me = fetched.me_id().map(str::to_string);
                    app.linear_edits.lay_over(
                        &project,
                        ticket.at,
                        &mut fetched.list,
                        me.as_deref(),
                    );
                    app.linear_failed.remove(&project);
                    app.linear.insert(project.clone(), fetched);
                    follow_cursor(app, &project, &old.list);
                }
                Err(err) => {
                    app.linear_failed.insert(project.clone());
                    app.flash = Some(crate::flash::Flash::failed(err));
                }
            }
            app.dirty = true;
            if landed.owed {
                request_list(app, project, dir, false);
            }
        }
        LinearAnswer::Attach(run) => land_attach(app, run, true),
        LinearAnswer::Attached(run) => land_attach(app, run, false),
        LinearAnswer::Detached {
            project,
            dir,
            refused,
        } => {
            match refused {
                Some((identifier, why)) => {
                    app.flash = Some(crate::flash::Flash::failed(format!(
                        "couldn't take {identifier} off the PR it moved from: {why}"
                    )))
                }
                // The rows still show the pull request Linear just dropped.
                None => request_list(app, project, dir, true),
            }
            app.dirty = true;
        }
        LinearAnswer::Edited {
            project,
            issue_id,
            identifier,
            change,
            seq,
            result,
        } => land_edit(app, project, issue_id, identifier, change, seq, result),
    }
}

/// The configured user's id, as `project`'s list has it.
fn me_id(app: &App, project: &ProjectId) -> Option<String> {
    app.linear.get(project)?.me_id().map(str::to_string)
}

/// Linear answered the edit numbered `seq`. Taken, it is what Linear has
/// now — and a state is what the TODOS MODAL's chips say, so they are
/// told one. Refused, the row goes back to
/// what Linear had before it — only when no newer edit to that property
/// has been asked for since; otherwise the newer one stands, and a fresh
/// list says where Linear has the issue. A newer edit queued behind this
/// one goes out now.
fn land_edit(
    app: &mut App,
    project: ProjectId,
    issue_id: String,
    identifier: String,
    change: Change,
    seq: u64,
    result: Result<(), String>,
) {
    app.dirty = true;
    if let Err(why) = &result {
        app.flash = Some(crate::flash::Flash::failed(
            change.refused(&identifier, why),
        ));
    }
    let prop = change.prop();
    let key = (issue_id.clone(), prop);
    let Some(mv) = app.linear_edits.moves.get_mut(&key) else {
        return;
    };
    if mv.sending == Some(seq) {
        mv.sending = None;
    }
    let newer = mv.seq != seq;
    let dir = mv.dir.clone();
    match result {
        Ok(()) => {
            let now = crate::fetch::now();
            mv.confirmed = change.clone();
            if !newer {
                mv.landed = Some(now);
            }
            if let Change::Status(state) = change {
                let url = app
                    .linear
                    .get(&project)
                    .and_then(|l| l.list.iter().find(|i| i.id == issue_id))
                    .map(|i| (i.url.clone(), i.priority));
                let (url, priority) = url.unwrap_or_default();
                crate::todos::view::note_linked(
                    app,
                    LinkedIssue {
                        identifier,
                        url,
                        state: state.name,
                        state_type: state.kind,
                        state_color: state.color,
                        priority,
                    },
                    now,
                );
            }
        }
        Err(_) if newer => request_list(app, project, dir, true),
        Err(_) => {
            let back = mv.confirmed.clone();
            app.linear_edits.moves.remove(&key);
            let me = me_id(app, &project);
            if let Some(issue) = app
                .linear
                .get_mut(&project)
                .and_then(|l| l.list.iter_mut().find(|i| i.id == issue_id))
            {
                back.put_on(issue, me.as_deref());
            }
            request_list(app, project, dir, true);
        }
    }
    if newer {
        send_edit(app, &issue_id, prop);
    }
}

/// Linear answered an attach run. What it took shows at once: the pull
/// request on those issues' rows, and the branch remembered against them
/// — an AUTO-ATTACH's link spent, or put back for the next list when
/// Linear refused. Only the user's own attach, or a refusal, is said.
fn land_attach(app: &mut App, run: AttachRun, auto: bool) {
    app.dirty = true;
    if let (Some(project), Some(branch)) = (&run.project, &run.branch) {
        if auto {
            let ids: Vec<String> = run.attached.iter().map(|(id, _)| id.clone()).collect();
            let gave_up =
                app.linear_links
                    .finish_attach(project, branch, &ids, &run.made, run.ok());
            // An issue that moved to another branch while this was out
            // was attached all the same: off again.
            let moved_on = run
                .made
                .iter()
                .filter(|(issue, _)| !app.linear_links.holds(project, branch, issue))
                .map(|(issue, id)| StaleAttachment {
                    identifier: run
                        .attached
                        .iter()
                        .find(|(attached, _)| attached == issue)
                        .map(|(_, identifier)| identifier.clone())
                        .unwrap_or_default(),
                    id: id.clone(),
                })
                .collect();
            detach_stale(app, project, moved_on);
            if let Some((identifier, why)) = &run.refused {
                let last = if gave_up {
                    format!(" — gave up after {ATTACH_TRIES} tries")
                } else {
                    String::new()
                };
                app.flash = Some(crate::flash::Flash::failed(format!(
                    "couldn't attach PR #{} to {identifier}: {why}{last}",
                    run.pr_number
                )));
            }
        } else if !run.attached.is_empty() {
            let stale =
                app.linear_links
                    .remember_attached(project, branch, &run.attached, &run.made);
            detach_stale(app, project, stale);
        }
    }
    show_attached(app, &run);
    if auto {
        return;
    }
    app.flash = Some(match &run.refused {
        Some((identifier, why)) => crate::flash::Flash::failed(format!(
            "couldn't attach PR #{} to {identifier}: {why}",
            run.pr_number
        )),
        None => crate::flash::Flash::done(format!(
            "attached PR #{} to {}",
            run.pr_number,
            run.identifiers()
        )),
    });
}

/// The pull request `run` attached, on each issue it took, in whichever
/// project's list holds it — and laid over any list asked before Linear
/// took it ([`LocalEdits`]), so the work column says so without a `⌘R`.
fn show_attached(app: &mut App, run: &AttachRun) {
    let now = crate::fetch::now();
    let pr = IssuePr {
        number: run.pr_number,
        url: run.pr_url.clone(),
        branch: run.branch.clone().unwrap_or_default(),
        ..IssuePr::default()
    };
    for (project, list) in app.linear.iter_mut() {
        for issue in list
            .list
            .iter_mut()
            .filter(|i| run.attached.iter().any(|(id, _)| *id == i.id))
        {
            if !issue.prs.iter().any(|p| p.url == pr.url) {
                issue.prs.push(pr.clone());
            }
            app.linear_edits
                .attached
                .entry(issue.id.clone())
                .or_default()
                .push(AttachedPr {
                    project: project.clone(),
                    pr: pr.clone(),
                    at: now,
                });
        }
    }
}

/// Remember a ⌘L launch's branch so the PR it opens can be attached. A
/// launch aimed at the root checkout remembers nothing: its branch (`dev`)
/// opens only the release pull request into `main`, and that one must not
/// be linked to the issues ([`root_branch`]). A branch with a pull request
/// open already has nothing to wait for: it is attached there and then.
/// The branch is the issues' ONE HOME from here ([`LinkStore`]).
pub(crate) fn remember_submit(app: &mut App, launch: &QuickLaunch) {
    let Some(batch) = &launch.linear else {
        return;
    };
    if !crate::config::Config::load().linear_auto_attach {
        return;
    }
    let (project, branch) = match &launch.target {
        QuickTarget::NewWorktree {
            project, branch, ..
        } => (project.clone(), branch.clone()),
        QuickTarget::Worktree(id) => {
            let Some(w) = app
                .tree
                .worktrees
                .iter()
                .find(|w| &w.id == id && !w.is_main)
            else {
                return;
            };
            (w.project_id.clone(), w.branch.clone())
        }
    };
    let stale = app.linear_links.remember(&project, &branch, &batch.issues);
    detach_stale(app, &project, stale);
    let open = app
        .open_prs
        .get(&project)
        .and_then(|o| o.list.iter().find(|pr| pr.head == branch))
        .map(|pr| (pr.url.clone(), pr.number));
    if let (Some((url, number)), Some(dir)) = (open, project_dir(app, &project)) {
        if root_branch(app, &project) != Some(branch.as_str()) {
            attach_link(app, &project, dir, &branch, url, number);
        }
    }
}

/// The branch `project`'s root checkout is on — the long-lived one feature
/// branches merge into (`dev`). A pull request from it is a release (`dev`
/// → `main`): linked to the issues, it would hold them back, since Linear
/// moves an issue on a merge only once none of its pull requests is left
/// open, and its own merge would move them again after the release had.
fn root_branch<'a>(app: &'a App, project: &ProjectId) -> Option<&'a str> {
    app.tree
        .worktrees
        .iter()
        .find(|w| &w.project_id == project && w.is_main && !w.branch.is_empty())
        .map(|w| w.branch.as_str())
}

/// The checkout `project` is read from — whose key asks Linear.
fn project_dir(app: &App, project: &ProjectId) -> Option<PathBuf> {
    app.tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map(|p| p.repo_path.clone())
}

/// The project read from the checkout `dir`.
fn project_at(app: &App, dir: &Path) -> Option<ProjectId> {
    app.tree
        .projects
        .iter()
        .find(|p| p.repo_path == dir)
        .map(|p| p.id.clone())
}

/// When a pull request is listed on a remembered branch, attach it —
/// never one from the root checkout's branch ([`root_branch`]), whatever
/// is remembered for it. Every list tries again a link Linear refused,
/// up to [`ATTACH_TRIES`], so `previous` (the list before) no longer
/// matters: a link is spent by Linear taking it, not by a list naming it.
/// **Link PRs to Linear** decides only whether a ⌘L launch remembers its
/// branch ([`remember_submit`]): a worktree linked by hand (`⌘.`) is
/// attached either way.
pub(crate) fn attach_new_prs(
    app: &mut App,
    project: &ProjectId,
    _previous: Option<&[crate::pull_request::OpenPr]>,
    fresh: &[crate::pull_request::OpenPr],
) {
    let Some(dir) = project_dir(app, project) else {
        return;
    };
    let root = root_branch(app, project).map(str::to_string);
    for pr in fresh {
        if root.as_deref() == Some(pr.head.as_str()) {
            continue;
        }
        attach_link(
            app,
            project,
            dir.clone(),
            &pr.head,
            pr.url.clone(),
            pr.number,
        );
    }
}

/// Forget `project`'s links whose branch is long gone
/// ([`LinkStore::prune`]): run as its open list lands, which is what says
/// which branches still have a pull request.
pub(crate) fn prune_links(app: &mut App, project: &ProjectId) {
    let checkouts = app
        .tree
        .worktrees
        .iter()
        .filter(|w| &w.project_id == project)
        .map(|w| w.branch.clone());
    let pull_requests = app
        .open_prs
        .get(project)
        .into_iter()
        .flat_map(|open| open.list.iter().map(|pr| pr.head.clone()));
    let live = checkouts.chain(pull_requests).collect();
    app.linear_links
        .prune(project, &live, orion_core::clock::now_secs());
}

/// LINEAR AUTO-ATTACH: the pull request on `branch` linked to the issues
/// waiting on it, off the loop, when any are and none is out already.
fn attach_link(
    app: &mut App,
    project: &ProjectId,
    dir: PathBuf,
    branch: &str,
    pr_url: String,
    pr_number: u64,
) {
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    let Some(targets) = app.linear_links.begin_attach(project, branch) else {
        return;
    };
    let (project, branch) = (Some(project.clone()), Some(branch.to_string()));
    tokio::spawn(async move {
        let run = attach_all(&dir, targets, project, branch, pr_url, pr_number).await;
        let _ = tx.send(LinearAnswer::Attach(run));
    });
}

/// ONE HOME's other half: the attachments orion made for the branches
/// `project`'s issues have moved off (`stale`) dropped from Linear, off
/// the loop with the project checkout's key. Silent unless Linear refuses
/// ([`LinearAnswer::Detached`]); nothing is asked when none is stale.
fn detach_stale(app: &mut App, project: &ProjectId, stale: Vec<StaleAttachment>) {
    if stale.is_empty() {
        return;
    }
    let (Some(tx), Some(dir)) = (app.linear_tx.clone(), project_dir(app, project)) else {
        return;
    };
    let project = project.clone();
    tokio::spawn(async move {
        let mut refused = None;
        for StaleAttachment { identifier, id } in stale {
            if let Err(why) = detach_pr(&dir, &id).await {
                refused.get_or_insert((identifier, why));
            }
        }
        let _ = tx.send(LinearAnswer::Detached {
            project,
            dir,
            refused,
        });
    });
}

/// Link `pr_url` to each of `targets` (`(id, identifier)`), one after
/// another; Linear keeps one attachment per pull request, so asking twice
/// links once.
async fn attach_all(
    dir: &Path,
    targets: Vec<(String, String)>,
    project: Option<ProjectId>,
    branch: Option<String>,
    pr_url: String,
    pr_number: u64,
) -> AttachRun {
    let mut attached = Vec::new();
    let mut made = Vec::new();
    let mut refused = None;
    for (id, identifier) in targets {
        match attach_pr(dir, &id, &pr_url).await {
            Ok(attachment) => {
                if let Some(attachment) = attachment {
                    made.push((id.clone(), attachment));
                }
                attached.push((id, identifier));
            }
            Err(why) => {
                refused.get_or_insert((identifier, why));
            }
        }
    }
    AttachRun {
        project,
        branch,
        pr_url,
        pr_number,
        attached,
        made,
        refused,
    }
}

/// THE ATTACH: the pull request `#pr_number` at `pr_url` linked to each
/// of `issues` through `attachmentLinkGitHubPR`, one after another off the
/// loop, with the project checkout `dir`'s key. Both ends of the pairing
/// run it — Enter in the LINEAR VIEW opened from a pull request
/// ([`LinearMode::Attach`]) and Enter in the PR PICK opened from here
/// (`pr_modal::PrPick`) — so the two can never attach differently. The
/// footer spins while Linear is asked and says how it went once it has
/// answered ([`LinearAnswer::Attached`]); Linear keeps one attachment per
/// pull request, so asking twice links once.
pub(crate) fn attach_issues(
    app: &mut App,
    dir: PathBuf,
    pr_url: String,
    pr_number: u64,
    issues: &[LinearIssue],
) {
    if issues.is_empty() {
        return;
    }
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    let targets: Vec<(String, String)> = issues
        .iter()
        .map(|i| (i.id.clone(), i.identifier.clone()))
        .collect();
    let project = project_at(app, &dir);
    let branch = project.as_ref().and_then(|project| {
        app.open_prs
            .get(project)?
            .list
            .iter()
            .find(|pr| pr.url == pr_url)
            .map(|pr| pr.head.clone())
    });
    app.flash = Some(crate::flash::Flash::working(format!(
        "attaching PR #{pr_number} to {}…",
        ids_of(issues)
    )));
    app.dirty = true;
    tokio::spawn(async move {
        let run = attach_all(&dir, targets, project, branch, pr_url, pr_number).await;
        let _ = tx.send(LinearAnswer::Attached(run));
    });
}

/// The LINEAR VIEW's own keys: one table [`handle_key`] matches and
/// [`hints`] spells.
pub(crate) mod keys {
    use crate::hints::Key;

    /// On the rows only: on the search line `space` types.
    pub const MARK: Key = Key::new(&["space"], "mark");
    /// From the search line to the rows, where `space` marks.
    pub const ROWS: Key = Key::new(&["down"], "to list");
    pub const CONFIRM: Key = Key::new(&["enter"], "agent on marked");
    pub const PRESET: Key = Key::new(&["shift+tab"], "preset");
    pub const BROWSER: Key = crate::issues::keys::BROWSER;
    pub const REFRESH: Key = crate::issues::keys::REFRESH;
    /// The issue's workflow state.
    pub const STATUS: Key = Key::new(&["cmd+s", "ctrl+s"], "status");
    /// Its priority: the TODOS MODAL's own key for one.
    pub const PRIORITY: Key = crate::todos::view::keys::PRIORITY;
    /// Whose it is: `I`, as Linear assigns an issue to you — never `⌘A`,
    /// the filter line's select-all. Where no ⌘ arrives it is `^G`: `^I`
    /// is Tab there, and `^A` is what Ghostty types for `⌘←`.
    pub const ASSIGN: Key = Key::new(&["cmd+i", "ctrl+g"], "assign");
    /// The PR PICK: the marked issues attached to a pull request picked
    /// in the PULL REQUESTS MODAL. That modal's own hotkey (`⌘U`, `^V`
    /// its twin), as the modal's way here is this one's (`⌘L`).
    pub const ATTACH: Key = Key::new(&["cmd+u", "ctrl+v"], "attach to PR");
    /// The WORKTREE PICK: the marked issues wait on the pull request a
    /// worktree's branch opens. The grid's **Select worktree** chord
    /// (`⌘.`, `^T` its twin), as picking a worktree is everywhere.
    pub const WORKTREE: Key = Key::new(&["cmd+.", "ctrl+t"], "link to worktree");
    /// The property and worktree pickers' own.
    pub const PICK: Key = Key::new(&["up", "down"], "pick").show(2);
    pub const SET: Key = Key::new(&["enter"], "set");
    /// The priority picker's digits, each choosing at once — the TODOS
    /// MODAL's.
    pub const LEVEL: Key = crate::todos::view::keys::LEVEL;
    pub const LINK: Key = Key::new(&["enter"], "link");
    /// `My issues` ⇄ `Other issues`: plain or with ⇧, as the PULL
    /// REQUESTS MODAL's tab keys are.
    pub const TABS: Key = Key::new(&["left", "right", "shift+left", "shift+right"], "tabs").show(2);
    /// The FILTER PICK — the PULL REQUESTS MODAL's too.
    pub const FILTER: Key = crate::list_filter::keys::FILTER;
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        MARK, ROWS, CONFIRM, PRESET, BROWSER, REFRESH, STATUS, PRIORITY, ASSIGN, ATTACH, WORKTREE,
        PICK, SET, LEVEL, LINK, TABS, FILTER,
    ];
}

/// The keys along the modal's bottom edge, for browsing or for picking
/// the issues a pull request attaches to. Esc clears a typed filter first.
pub(crate) fn hints(view: &LinearView) -> Vec<crate::hints::Hint> {
    use crate::hints::Hint;
    let esc = if !view.query.is_empty() {
        "clear"
    } else if matches!(
        view.mode,
        LinearMode::Attach { .. } | LinearMode::Link { .. }
    ) {
        "back"
    } else {
        "close"
    };
    if let Some(pick) = &view.prop_pick {
        let mut hints = vec![keys::SET.hint().kept(), keys::PICK.hint()];
        if pick.prop == Prop::Priority {
            hints.push(Hint::new("1-4/0", keys::LEVEL.does));
        }
        hints.push(Hint::new("Esc", "cancel"));
        return hints;
    }
    if view.worktree_pick.is_some() {
        return vec![
            keys::LINK.hint().kept(),
            keys::PICK.hint(),
            Hint::new("Esc", "cancel"),
        ];
    }
    if view.filter_pick.is_some() {
        return crate::list_filter::hints();
    }
    let mark = match view.focus {
        LinearFocus::List => keys::MARK.hint(),
        LinearFocus::Search => keys::ROWS.hint(),
    };
    match view.mode {
        LinearMode::Browse => vec![
            mark,
            keys::CONFIRM.hint().kept(),
            keys::TABS.hint(),
            keys::FILTER.hint(),
            // An issue's own properties before where it is sent: a
            // narrow border cuts the hints from the right.
            keys::STATUS.hint(),
            keys::PRIORITY.hint(),
            keys::ASSIGN.hint(),
            keys::ATTACH.hint(),
            keys::WORKTREE.hint(),
            keys::PRESET.hint(),
            keys::BROWSER.hint(),
            keys::REFRESH.hint(),
            Hint::new("Esc", esc),
        ],
        LinearMode::Link { .. } => vec![
            keys::CONFIRM.hint_as("link to todo").kept(),
            keys::TABS.hint(),
            keys::FILTER.hint(),
            keys::BROWSER.hint(),
            Hint::new("Esc", esc),
        ],
        LinearMode::Attach { .. } => vec![
            mark,
            keys::CONFIRM.hint_as("attach marked to this PR").kept(),
            keys::TABS.hint(),
            keys::FILTER.hint(),
            keys::STATUS.hint(),
            keys::PRIORITY.hint(),
            keys::ASSIGN.hint(),
            keys::BROWSER.hint(),
            Hint::new("Esc", esc),
        ],
    }
}

/// A property picker for `issue` in the reading pane's place: `rows`,
/// the one `issue` stands on marked, and the cursor on it — or, `on_top`,
/// on the first whatever the issue has.
fn open_prop_pick(app: &mut App, issue: &LinearIssue, prop: Prop, rows: Vec<Change>, on_top: bool) {
    let current = rows.iter().position(|row| row.stands_on(issue));
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    view.filter_pick = None;
    view.prop_pick = Some(PropPick {
        issue_id: issue.id.clone(),
        identifier: issue.identifier.clone(),
        prop,
        rows,
        selected: current.filter(|_| !on_top).unwrap_or(0),
        current,
    });
}

/// `⌘S`: the status picker for the issue under the cursor, on the
/// state it is in. An issue whose team's states were not read says so.
fn open_status_pick(app: &mut App) {
    let Some(issue) = selected_issue(app).cloned() else {
        return;
    };
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    let states = app
        .linear
        .get(&view.project)
        .and_then(|l| l.states.get(&issue.team_id))
        .cloned()
        .unwrap_or_default();
    if states.is_empty() {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "Linear didn't say which states {} can move to — {} asks again",
            issue.identifier,
            keys::REFRESH.label()
        )));
        return;
    }
    let rows = states.into_iter().map(Change::Status).collect();
    open_prop_pick(app, &issue, Prop::Status, rows, false);
}

/// `⌘P`: Linear's priorities for the issue under the cursor, urgent
/// first, on its own.
fn open_priority_pick(app: &mut App) {
    let Some(issue) = selected_issue(app).cloned() else {
        return;
    };
    let rows = PRIORITY_ORDER.into_iter().map(Change::Priority).collect();
    open_prop_pick(app, &issue, Prop::Priority, rows, false);
}

/// `⌘I`: who the issue under the cursor can go to — the configured user
/// first, so `Enter` is "assign to me", then nobody, then the rest of its
/// team by name. The cursor opens on the top row, never on whose it is
/// now: that row is only marked. An issue Linear named nobody for says so.
fn open_assign_pick(app: &mut App) {
    let Some(issue) = selected_issue(app).cloned() else {
        return;
    };
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    let list = app.linear.get(&view.project);
    let me = list.and_then(|l| l.me.clone());
    let members = list
        .and_then(|l| l.members.get(&issue.team_id))
        .cloned()
        .unwrap_or_default();
    if me.is_none() && members.is_empty() {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "Linear didn't say who {} can go to — {} asks again",
            issue.identifier,
            keys::REFRESH.label()
        )));
        return;
    }
    let others = members
        .into_iter()
        .filter(|user| me.as_ref().is_none_or(|me| me.id != user.id));
    let rows = me
        .iter()
        .cloned()
        .map(Some)
        .chain([None])
        .chain(others.map(Some))
        .map(Change::Assignee)
        .collect();
    open_prop_pick(app, &issue, Prop::Assignee, rows, true);
}

/// Keys while a property picker is up. A priority's digit sets it at
/// once, as the TODOS MODAL's pick does.
fn handle_pick_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    let Some(pick) = &mut view.prop_pick else {
        return;
    };
    let digit_row = match key.code {
        KeyCode::Char(c) if pick.prop == Prop::Priority && keys::LEVEL.matches(&key) => {
            let level = c.to_digit(10).unwrap_or(0) as u8;
            pick.rows
                .iter()
                .position(|row| *row == Change::Priority(level))
        }
        _ => None,
    };
    if let Some(row) = digit_row {
        pick.selected = row;
        set_prop(app);
        app.dirty = true;
        return;
    }
    match key.code {
        KeyCode::Esc => view.prop_pick = None,
        KeyCode::Down => pick.selected = clamp_selection(pick.selected as i64 + 1, pick.rows.len()),
        KeyCode::Up => pick.selected = clamp_selection(pick.selected as i64 - 1, pick.rows.len()),
        _ if keys::SET.matches(&key) => set_prop(app),
        _ => {}
    }
    app.dirty = true;
}

/// Enter in a property picker: the row says the new value at once, and
/// `issueUpdate` runs off the loop — put back if Linear refuses
/// ([`land_edit`]). One edit per property of an issue is out at a time: a
/// second to that property while one is out queues behind it, the latest
/// asked for winning, so Linear ends where the row does. The value the
/// issue already has closes the picker with nothing sent.
fn set_prop(app: &mut App) {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    let Some(pick) = view.prop_pick.take() else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(change) = pick.rows.get(pick.selected).cloned() else {
        return;
    };
    let me = me_id(app, &project);
    let Some(issue) = app
        .linear
        .get_mut(&project)
        .and_then(|l| l.list.iter_mut().find(|i| i.id == pick.issue_id))
    else {
        return;
    };
    if change.stands_on(issue) {
        return;
    }
    let prop = change.prop();
    let before = Change::of(issue, prop);
    change.put_on(issue, me.as_deref());
    let edits = &mut app.linear_edits;
    edits.next_seq += 1;
    let seq = edits.next_seq;
    let mv = edits
        .moves
        .entry((pick.issue_id.clone(), prop))
        .or_insert_with(|| Move {
            project: project.clone(),
            dir: dir.clone(),
            identifier: pick.identifier.clone(),
            want: change.clone(),
            seq,
            sending: None,
            confirmed: before,
            landed: None,
        });
    mv.want = change;
    mv.seq = seq;
    mv.dir = dir;
    mv.landed = None;
    if mv.sending.is_none() {
        send_edit(app, &pick.issue_id, prop);
    }
}

/// Send the edit the row says for `issue_id`'s `prop` to Linear, off the
/// loop.
fn send_edit(app: &mut App, issue_id: &str, prop: Prop) {
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    let Some(mv) = app
        .linear_edits
        .moves
        .get_mut(&(issue_id.to_string(), prop))
    else {
        return;
    };
    mv.sending = Some(mv.seq);
    let (project, dir, identifier, change, seq) = (
        mv.project.clone(),
        mv.dir.clone(),
        mv.identifier.clone(),
        mv.want.clone(),
        mv.seq,
    );
    let issue_id = issue_id.to_string();
    tokio::spawn(async move {
        let result = update_issue(&dir, &issue_id, &change).await;
        let _ = tx.send(LinearAnswer::Edited {
            project,
            issue_id,
            identifier,
            change,
            seq,
            result,
        });
    });
}

/// `⌘.` while browsing: the project's worktrees — every one but the root,
/// whose branch opens no pull request — for the issues `Enter` would
/// launch on.
fn open_worktree_pick(app: &mut App) {
    if !matches!(&app.overlay, Some(Overlay::Linear(v)) if v.mode == LinearMode::Browse) {
        return;
    }
    let issues = picked(app);
    if issues.is_empty() {
        return;
    }
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    let mut worktrees: Vec<&orion_core::Worktree> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| w.project_id == view.project && !w.is_main && !w.branch.is_empty())
        .collect();
    worktrees.sort_by_key(|w| w.sort_order);
    let branches: Vec<String> = worktrees.iter().map(|w| w.branch.clone()).collect();
    if branches.is_empty() {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "{} has no worktree to link to",
            view.project_name
        )));
        return;
    }
    if let Some(Overlay::Linear(view)) = &mut app.overlay {
        view.filter_pick = None;
        view.worktree_pick = Some(WorktreePick {
            issues,
            branches,
            selected: 0,
        });
    }
}

/// Keys while the worktree picker is up.
fn handle_worktree_pick_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    let Some(pick) = &mut view.worktree_pick else {
        return;
    };
    let n = pick.branches.len();
    match key.code {
        KeyCode::Esc => view.worktree_pick = None,
        KeyCode::Down => pick.selected = clamp_selection(pick.selected as i64 + 1, n),
        KeyCode::Up => pick.selected = clamp_selection(pick.selected as i64 - 1, n),
        _ if keys::LINK.matches(&key) => link_worktree(app),
        _ => {}
    }
    app.dirty = true;
}

/// Enter in the worktree picker. A branch with a pull request open now is
/// attached at once ([`attach_issues`]); otherwise the issues wait in the
/// [`LinkStore`] for the one it opens, which [`attach_new_prs`] attaches
/// as OPEN PRS first lists it — the ⌘L launch's own path.
fn link_worktree(app: &mut App) {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    let Some(pick) = view.worktree_pick.take() else {
        return;
    };
    let Some(branch) = pick.branches.get(pick.selected) else {
        return;
    };
    // Spent, as a ⌘U attach spends them.
    view.marked.clear();
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let open = app
        .open_prs
        .get(&project)
        .and_then(|o| o.list.iter().find(|pr| &pr.head == branch))
        .map(|pr| (pr.url.clone(), pr.number));
    // Remembered against the branch once Linear has taken them
    // ([`land_attach`]).
    if let Some((url, number)) = open {
        attach_issues(app, dir, url, number, &pick.issues);
        return;
    }
    let stale = app.linear_links.remember(&project, branch, &pick.issues);
    app.flash = Some(crate::flash::Flash::done(format!(
        "{} will attach to the PR {branch} opens",
        ids_of(&pick.issues)
    )));
    detach_stale(app, &project, stale);
}

pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return false;
    };
    view.query.insert_str(text);
    query_changed(app);
    true
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    if matches!(&app.overlay, Some(Overlay::Linear(v)) if v.prop_pick.is_some()) {
        handle_pick_key(app, key);
        return;
    }
    if matches!(&app.overlay, Some(Overlay::Linear(v)) if v.worktree_pick.is_some()) {
        handle_worktree_pick_key(app, key);
        return;
    }
    if matches!(&app.overlay, Some(Overlay::Linear(v)) if v.filter_pick.is_some()) {
        filter_pick_key(app, &key);
        return;
    }
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let page = view.view_height.max(1) as i32;
    let on_list = view.focus == LinearFocus::List;
    match key.code {
        KeyCode::Esc if !view.query.is_empty() => clear_query(app),
        KeyCode::Esc => close(app),
        KeyCode::Down if shift => view.scroll_by(1),
        KeyCode::Up if shift => view.scroll_by(-1),
        // ↓ off the search line hands the rows the keys, the cursor where
        // it was; ↑ off the top row hands them back.
        KeyCode::Down if !on_list => view.focus = LinearFocus::List,
        KeyCode::Down => {
            step(app, 1);
        }
        KeyCode::Up if !on_list => {}
        KeyCode::Up => {
            if !step(app, -1) {
                if let Some(Overlay::Linear(view)) = &mut app.overlay {
                    view.focus = LinearFocus::Search;
                }
            }
        }
        // ←/→ (⇧ or not) are the tabs', never the filter line's caret: a
        // filter is typed and backspaced, not edited mid-line.
        KeyCode::Left | KeyCode::Right if keys::TABS.matches(&key) => {
            let other = view.tab.other();
            switch_tab(app, other);
        }
        KeyCode::PageDown => view.scroll_by(page),
        KeyCode::PageUp => view.scroll_by(-page),
        KeyCode::Home => view.scroll = 0,
        KeyCode::End => view.scroll = view.max_scroll(),
        // On the search line `space` is the filter's, a word break.
        _ if on_list && keys::MARK.matches(&key) => toggle_mark(app),
        _ if keys::CONFIRM.matches(&key) => confirm(app),
        _ if keys::PRESET.matches(&key) => open_preset(app),
        _ if keys::BROWSER.matches(&key) => open_in_browser(app, out),
        _ if keys::REFRESH.matches(&key) => refresh(app),
        _ if keys::STATUS.matches(&key) => open_status_pick(app),
        _ if keys::PRIORITY.matches(&key) => open_priority_pick(app),
        _ if keys::ASSIGN.matches(&key) => open_assign_pick(app),
        _ if keys::ATTACH.matches(&key) => open_pr_pick(app),
        _ if keys::WORKTREE.matches(&key) => open_worktree_pick(app),
        _ if keys::FILTER.matches(&key) => view.filter_pick = Some(FilterPick::default()),
        // Everything else edits the filter — and typing on the rows takes
        // the keys back to the search line.
        _ => {
            if view.query.handle_key(&key).changed() {
                view.focus = LinearFocus::Search;
                query_changed(app);
            }
        }
    }
    app.dirty = true;
}

pub(crate) fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    pos: Position,
    out: &mut Vec<ClientRequest>,
) {
    if app.hover_crumb == Some(HitTarget::ModalBrowser)
        && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
    {
        open_in_browser(app, out);
        return;
    }
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    let list = view.list_area;
    let body = view.body_area;
    match mouse.kind {
        MouseEventKind::ScrollDown if list.contains(pos) => {
            step(app, 1);
        }
        MouseEventKind::ScrollUp if list.contains(pos) => {
            step(app, -1);
        }
        MouseEventKind::ScrollDown if body.contains(pos) => {
            if let Some(Overlay::Linear(view)) = &mut app.overlay {
                view.scroll_by(WHEEL_LINES);
            }
        }
        MouseEventKind::ScrollUp if body.contains(pos) => {
            if let Some(Overlay::Linear(view)) = &mut app.overlay {
                view.scroll_by(-WHEEL_LINES);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if view.tab_row.contains(pos) => {
            let hit = crate::ui::tab_hit(&view.tab_hits, pos.x);
            if let Some(tab) = hit.and_then(|i| LinearTab::ALL.get(i)) {
                switch_tab(app, *tab);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if list.contains(pos) => {
            if let Some(i) = row_under(app, pos) {
                if let Some(Overlay::Linear(view)) = &mut app.overlay {
                    view.selected = i;
                    view.scroll = 0;
                    view.focus = LinearFocus::List;
                }
            }
        }
        _ => {}
    }
    app.dirty = true;
}

/// The row under the pointer, by the rects the last draw laid the rows
/// out in — they are two lines tall, with status headers between them.
fn row_under(app: &App, pos: Position) -> Option<usize> {
    let Overlay::Linear(view) = app.overlay.as_ref()? else {
        return None;
    };
    crate::ui::row_hit(&view.row_rects, pos)
}

/// Show `tab`'s list, the cursor on its first row the filter leaves and
/// the window back at the top. Marks stay, whichever tab they were made
/// on: Enter takes every marked issue.
fn switch_tab(app: &mut App, tab: LinearTab) {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    if view.tab == tab {
        return;
    }
    view.tab = tab;
    view.list_start = 0;
    view.scroll = 0;
    let list = app
        .linear
        .get(&view.project)
        .map(|l| l.list.as_slice())
        .unwrap_or(&[]);
    if let Some((first, _)) = visible_rows(view, list).first() {
        view.selected = *first;
    }
}

/// A key while the FILTER PICK is up: its cursor moves, `space` adds or
/// takes out the value's token in the filter line — the rows narrowing
/// behind it — and Enter or Esc puts it away.
fn filter_pick_key(app: &mut App, key: &KeyEvent) {
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    let facets = pick_facets(rows(app, &view.project), view.tab, app.theme);
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    let changed =
        crate::list_filter::apply_key(&mut view.filter_pick, &mut view.query, FACETS, &facets, key);
    if changed {
        query_changed(app);
    }
    app.dirty = true;
}

/// The FILTER PICK's facets over the tab's issues: their states in the
/// sections' order, the priorities, labels and projects in Linear's
/// colours, and — on `Other issues` — who they are assigned to.
fn pick_facets(list: &[LinearIssue], tab: LinearTab, th: Theme) -> Vec<PickFacet> {
    use crate::list_filter::{by_count, fixed_values, plain_values, tally};
    let issues: Vec<&LinearIssue> = list.iter().filter(|i| tab.holds(i)).collect();
    let count = |key: &str| tally(issues.iter().copied(), |i| facet_values(i, key));
    // A label's or a project's dot, in the colour its first carrier gives.
    let dotted = |key: &str, tags: &dyn Fn(&LinearIssue) -> Vec<&LinearTag>| -> Vec<PickValue> {
        by_count(count(key))
            .into_iter()
            .map(|(value, count)| {
                let color = issues
                    .iter()
                    .flat_map(|i| tags(i))
                    .find(|t| t.name.eq_ignore_ascii_case(&value))
                    .and_then(|t| crate::theme::hex(&t.color))
                    .unwrap_or(th.muted);
                PickValue {
                    mark: Some(("●".to_string(), color)),
                    value,
                    count,
                }
            })
            .collect()
    };
    FACETS
        .iter()
        // Every issue on `My issues` is yours: nobody else to pick.
        .filter(|facet| facet.key != "assignee" || tab == LinearTab::Others)
        .map(|facet| {
            let values = match facet.key {
                "status" => count("status")
                    .into_iter()
                    .map(|(value, count)| {
                        let mark = issues.iter().find(|i| i.status == value).map(|i| {
                            let (glyph, color) = state_mark(i, th);
                            (glyph.to_string(), color)
                        });
                        PickValue { value, count, mark }
                    })
                    .collect(),
                "priority" => {
                    let words: Vec<&str> =
                        PRIORITY_ORDER.iter().map(|p| priority_word(*p)).collect();
                    fixed_values(&words, &count("priority"), |word| {
                        let p = PRIORITY_ORDER
                            .into_iter()
                            .find(|p| priority_word(*p) == word)?;
                        let (letter, style) = priority_letter(p, th);
                        Some((letter.to_string(), style.fg.unwrap_or(th.muted)))
                    })
                }
                "label" => dotted("label", &|i| i.labels.iter().collect()),
                "project" => dotted("project", &|i| i.project.iter().collect()),
                key => plain_values(by_count(count(key))),
            };
            PickFacet {
                key: *facet,
                values,
            }
        })
        .collect()
}

fn close(app: &mut App) {
    let Some(Overlay::Linear(view)) = app.overlay.take() else {
        return;
    };
    match view.mode {
        LinearMode::Attach { back, .. } => crate::pr_modal::reopen(app, *back),
        LinearMode::Link { back, .. } => crate::todos::reopen(app, *back),
        LinearMode::Browse => {}
    }
}

fn refresh(app: &mut App) {
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    request_list(app, view.project.clone(), view.dir.clone(), true);
}

fn clear_query(app: &mut App) {
    if let Some(Overlay::Linear(view)) = &mut app.overlay {
        view.query.clear();
    }
    query_changed(app);
}

fn query_changed(app: &mut App) {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    view.scroll = 0;
    let list = app
        .linear
        .get(&view.project)
        .map(|l| l.list.as_slice())
        .unwrap_or(&[]);
    if let Some(i) = cursor_index(view, list) {
        view.selected = i;
    }
}

/// Move the cursor `delta` rows over the ones the filter leaves; false
/// when it was already at that end.
fn step(app: &mut App, delta: i32) -> bool {
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return false;
    };
    let list = rows(app, &view.project);
    let visible = visible_rows(view, list);
    if visible.is_empty() {
        return false;
    }
    let here = visible
        .iter()
        .position(|(i, _)| *i == view.selected)
        .unwrap_or(0);
    let next = (here as i32 + delta).clamp(0, visible.len() as i32 - 1) as usize;
    if let Some(Overlay::Linear(view)) = &mut app.overlay {
        view.selected = visible[next].0;
        view.scroll = 0;
    }
    next != here
}

fn toggle_mark(app: &mut App) {
    let Some(issue) = selected_issue(app).cloned() else {
        return;
    };
    if let Some(Overlay::Linear(view)) = &mut app.overlay {
        if !view.marked.insert(issue.id.clone()) {
            view.marked.remove(&issue.id);
        }
    }
}

fn picked(app: &App) -> Vec<LinearIssue> {
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return Vec::new();
    };
    let list = rows(app, &view.project);
    let marked: Vec<LinearIssue> = list
        .iter()
        .filter(|i| view.marked.contains(&i.id))
        .cloned()
        .collect();
    if marked.is_empty() {
        selected_issue(app).into_iter().cloned().collect()
    } else {
        marked
    }
}

fn confirm(app: &mut App) {
    let issues = picked(app);
    if issues.is_empty() {
        return;
    }
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    match &view.mode {
        LinearMode::Browse => open_prompt(app, issues),
        LinearMode::Attach {
            pr_url, pr_number, ..
        } => {
            let (url, number, dir) = (pr_url.clone(), *pr_number, view.dir.clone());
            attach_issues(app, dir, url, number, &issues);
        }
        // The issue under the cursor — one todo is one issue.
        LinearMode::Link { .. } => {
            let Some(issue) = selected_issue(app).cloned() else {
                return;
            };
            let Some(Overlay::Linear(view)) = app.overlay.take() else {
                return;
            };
            if let LinearMode::Link { item, back } = view.mode {
                crate::todos::view::link_issue(app, *back, item, LinkedIssue::of(&issue));
            }
        }
    }
}

/// `⌘U` (`^V`) while browsing: the PULL REQUESTS MODAL as a PR PICK for
/// the issues `Enter` would launch on — the marked set, else the one
/// under the cursor. The view rides along whole, marks, filter and cursor
/// and all: Enter on a pull request there attaches it ([`attach_issues`])
/// and comes back here, Esc comes back with nothing sent. Picking for a
/// pull request this view was itself opened from has nowhere to go.
fn open_pr_pick(app: &mut App) {
    if !matches!(&app.overlay, Some(Overlay::Linear(v)) if v.mode == LinearMode::Browse) {
        return;
    }
    let issues = picked(app);
    if issues.is_empty() {
        return;
    }
    let Some(Overlay::Linear(view)) = app.overlay.take() else {
        return;
    };
    crate::pr_modal::open_pick(
        app,
        crate::pr_modal::PrPick {
            issues,
            back: Box::new(view),
        },
    );
}

fn open_prompt(app: &mut App, issues: Vec<LinearIssue>) {
    let Some(launch) = launch_for(app, issues) else {
        return;
    };
    let under = ModalUnder::of(app.overlay.as_ref());
    crate::quick_prompt::open_box(app, launch.with_under(under));
}

fn open_preset(app: &mut App) {
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    if view.mode != LinearMode::Browse {
        return;
    }
    let issues = picked(app);
    if issues.is_empty() {
        return;
    }
    let Some(launch) = launch_for(app, issues) else {
        return;
    };
    crate::quick_prompt::open_preset_picker(
        app,
        QuickReturn::fresh(launch.with_under(ModalUnder::of(app.overlay.as_ref()))),
    );
}

fn launch_for(app: &mut App, issues: Vec<LinearIssue>) -> Option<QuickLaunch> {
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return None;
    };
    let project = view.project.clone();
    let cfg = crate::config::Config::load();
    let task = expand_template(cfg.linear_template(), &issues);
    let taken = app.project_branches(&project);
    let batch = LinearBatch { issues, task };
    // A fresh worktree named after the batch (`LinearBatch::branch`),
    // never the project's root checkout: a batch of issues is a branch of
    // its own, and its pull request is what `remember_submit` links back.
    // The box's WORKTREE PICKER can still aim it at a checkout on purpose.
    let target = QuickTarget::NewWorktree {
        project,
        branch: batch.branch(&taken),
        existing: false,
    };
    Some(QuickLaunch::from_config(target, &cfg).with_linear(Some(batch)))
}

fn open_in_browser(app: &mut App, out: &mut Vec<ClientRequest>) {
    let url = selected_issue(app).map(|issue| issue.url.clone());
    if let Some(url) = url {
        crate::event_loop::open_link(app, &url, out);
    }
}

fn rows<'a>(app: &'a App, project: &ProjectId) -> &'a [LinearIssue] {
    app.linear
        .get(project)
        .map(|l| l.list.as_slice())
        .unwrap_or(&[])
}

/// The keys the filter line takes besides its words (`list_filter`):
/// `status:todo p:high label:bug project:"Export PDF" assignee:sam
/// reporter:ana`.
pub(crate) const FACETS: &[FacetKey] = &[
    FacetKey::new("status", "Status"),
    FacetKey::new("priority", "Priority").aliases(&["p"]),
    FacetKey::new("label", "Label"),
    FacetKey::new("project", "Project"),
    FacetKey::new("assignee", "Assignee"),
    FacetKey::new("reporter", "Reporter"),
];

/// An issue's values for one of the [`FACETS`].
fn facet_values(issue: &LinearIssue, key: &str) -> Vec<String> {
    match key {
        "status" => vec![issue.status.clone()],
        // `p:none` as well as the word the rows say.
        "priority" if issue.priority == 0 => vec![priority_word(0).to_string(), "none".into()],
        "priority" => vec![priority_word(issue.priority).to_string()],
        "label" => issue.labels.iter().map(|l| l.name.clone()).collect(),
        "project" => issue.project.iter().map(|p| p.name.clone()).collect(),
        "assignee" if issue.assignee.is_empty() => vec![UNASSIGNED.to_string()],
        "assignee" => vec![issue.assignee.clone()],
        "reporter" if !issue.reporter.is_empty() => vec![issue.reporter.clone()],
        _ => Vec::new(),
    }
}

/// The issues the filter line leaves, either tab's, top to bottom:
/// narrowed by its tokens and ranked by its words, then gathered by state
/// in the list's own order of states (`parse_issues`) — each with the
/// matched char positions of its `ENG-12 title`.
fn filtered(query: &str, list: &[LinearIssue]) -> Vec<(usize, Vec<usize>)> {
    let parsed = crate::list_filter::parse(query, FACETS);
    let mut rows = crate::list_filter::narrow(
        &parsed,
        FACETS,
        list.len(),
        |i| list[i].label(),
        |i, key| facet_values(&list[i], key),
    );
    rows.sort_by(|(a, _), (b, _)| {
        let (a, b) = (&list[*a], &list[*b]);
        status_rank(a)
            .cmp(&status_rank(b))
            .then_with(|| a.status_order.cmp(&b.status_order))
            .then_with(|| a.status.cmp(&b.status))
    });
    rows
}

/// [`filtered`]'s rows on `tab`.
fn on_tab(
    tab: LinearTab,
    rows: &[(usize, Vec<usize>)],
    list: &[LinearIssue],
) -> Vec<(usize, Vec<usize>)> {
    rows.iter()
        .filter(|(i, _)| tab.holds(&list[*i]))
        .cloned()
        .collect()
}

/// The issues the view shows: the tab's, as [`filtered`] leaves them.
fn visible_rows(view: &LinearView, list: &[LinearIssue]) -> Vec<(usize, Vec<usize>)> {
    on_tab(view.tab, &filtered(&view.query, list), list)
}

/// The issue under the cursor: `selected` while the view shows it, else
/// the first row it does show — a tab switch, a filter or a refresh may
/// have hidden it.
fn cursor_index(view: &LinearView, list: &[LinearIssue]) -> Option<usize> {
    crate::list_filter::cursor_in(&visible_rows(view, list), view.selected)
}

fn selected_issue(app: &App) -> Option<&LinearIssue> {
    let Overlay::Linear(view) = app.overlay.as_ref()? else {
        return None;
    };
    let list = rows(app, &view.project);
    let i = cursor_index(view, list)?;
    list.get(i)
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &LinearView, th: Theme, backdrop: bool) {
    let list_focused = !backdrop;
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let list_w = (area.width * LIST_PCT / 100)
        .max(MIN_LIST_W)
        .min(area.width.saturating_sub(SPLIT_PANE_LAYOUT_MIN));
    let [list_a, body_a] = Layout::horizontal([
        Constraint::Length(list_w),
        Constraint::Min(SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);

    let issues: Vec<LinearIssue> = rows(app, &view.project).to_vec();
    let inflight = app.linear_flights.in_flight(&view.project);
    let failed = app.linear_failed.contains(&view.project);
    let more = app
        .linear
        .get(&view.project)
        .is_some_and(|l| l.more_on(view.tab));
    let parsed = crate::list_filter::parse(&view.query, FACETS);
    // One pass of the filter over both tabs: the tab's rows, and each
    // tab's count for its label.
    let all = filtered(&view.query, &issues);
    let visible = on_tab(view.tab, &all, &issues);
    let cursor = crate::list_filter::cursor_in(&visible, view.selected);
    let cursor_row = cursor
        .and_then(|c| visible.iter().position(|(i, _)| *i == c))
        .unwrap_or(0);

    let in_tab = issues.iter().filter(|i| view.tab.holds(i)).count();
    let count = if parsed.is_active() {
        format!("{}/{in_tab}", visible.len())
    } else {
        in_tab.to_string()
    };
    let marked = view.marked.len();
    let head = match &view.mode {
        LinearMode::Browse => format!("Linear — {} ({count})", view.project_name),
        LinearMode::Attach { pr_number, .. } => {
            format!("Linear → PR #{pr_number} — {} ({count})", view.project_name)
        }
        LinearMode::Link { .. } => format!("Linear → todo — {} ({count})", view.project_name),
    };
    let title = if inflight {
        format!("{head}, refreshing…")
    } else if marked > 0 {
        format!("{head}, {marked} marked")
    } else {
        head
    };
    let side_up =
        view.prop_pick.is_some() || view.worktree_pick.is_some() || view.filter_pick.is_some();
    let list_focused = list_focused && !side_up;
    let rows_focused = list_focused && view.focus == LinearFocus::List;
    let block = panel_block(&title, list_focused, th);
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    // The tabs on the first line, each with how many rows it shows under
    // the filter; the filter under them, its tokens lit.
    let tab_row = row_rect(list_inner, 0).unwrap_or_default();
    let labels: Vec<String> = LinearTab::ALL
        .iter()
        .map(|tab| {
            let n = all.iter().filter(|(i, _)| tab.holds(&issues[*i])).count();
            format!("{} {n}", tab.name())
        })
        .collect();
    let active = LinearTab::ALL
        .iter()
        .position(|t| *t == view.tab)
        .unwrap_or(0);
    let (strip, tab_hits) = crate::ui::tab_strip(
        tab_row.x,
        tab_row.width,
        labels.iter().map(String::as_str),
        active,
        false,
        th,
    );
    f.render_widget(Paragraph::new(Line::from(strip)), tab_row);
    let below_tabs = crate::ui::below_first_row(list_inner);
    if let Some(query_area) = row_rect(below_tabs, 0) {
        let placeholder = format!("search title, status, label… {} pick", keys::FILTER.label());
        let line = if list_focused && view.focus == LinearFocus::Search {
            search_line_lit(&view.query, &placeholder, query_area, th, &parsed.spans)
        } else {
            search_line_idle(&view.query, &placeholder, query_area, th)
        };
        f.render_widget(Paragraph::new(line), query_area);
    }
    let mut rows_area = crate::ui::below_first_row(below_tabs);
    // A page short of what Linear has says so, and where the rest is.
    if more {
        if let Some(note) = row_rect(rows_area, 0) {
            let limit = match view.tab {
                LinearTab::Mine => MINE_LIMIT,
                LinearTab::Others => OTHERS_LIMIT,
            };
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!("showing the {limit} most recently updated · more on Linear"),
                    Style::default().fg(th.dim),
                )),
                note,
            );
        }
        rows_area = crate::ui::below_first_row(rows_area);
    }
    if in_tab == 0 {
        let text = if failed {
            "couldn't list Linear issues — check LINEAR_API_KEY and Settings → Linear account"
        } else if inflight || app.linear_tx.is_some() && !app.linear.contains_key(&view.project) {
            "asking Linear…"
        } else if view.tab == LinearTab::Mine {
            "no open issues assigned to you"
        } else {
            "no other open issues in your teams"
        };
        empty_list_row(f, rows_area, text, th);
    } else if visible.is_empty() {
        empty_list_row(f, rows_area, "no issues match", th);
    }
    let budget = (rows_area.width as usize).saturating_sub(2);
    let now = orion_core::clock::now_secs() as i64;
    // What is going on with each issue, for the rows' work column and the
    // page's border.
    let works: HashMap<usize, Option<IssueWork>> = visible
        .iter()
        .map(|(i, _)| (*i, work_of(app, &view.project, &issues[*i])))
        .chain(cursor.map(|c| (c, work_of(app, &view.project, &issues[c]))))
        .collect();
    let widths = WorkWidths::of(works.values().flatten());
    let spin = app.spin_phase();
    // A header over each status, a line per issue.
    let keys: Vec<&str> = visible
        .iter()
        .map(|(i, _)| issues[*i].status.as_str())
        .collect();
    let entries = sections(&keys, true);
    let (list_start, drawn) =
        layout_sections(&entries, |_| 1, cursor_row, view.list_start, rows_area);
    let mut row_rects = Vec::with_capacity(drawn.len());
    for (entry, rect) in drawn {
        match entry {
            ListEntry::Header { first, count } => {
                let issue = &issues[visible[first].0];
                let (glyph, color) = state_mark(issue, th);
                let mark = Span::styled(format!("{glyph} "), Style::default().fg(color));
                let line = Line::from(list_header(&issue.status, count, Some(mark), th));
                f.render_widget(Paragraph::new(line), rect);
            }
            ListEntry::Row(v) => {
                let (index, positions) = &visible[v];
                let issue = &issues[*index];
                let marked = view.marked.contains(&issue.id);
                let work = WorkColumn {
                    work: works.get(index).cloned().flatten(),
                    widths,
                    spin,
                };
                let line = title_spans(issue, positions, marked, budget, &work, th);
                render_row(f, rect, line, Some(*index) == cursor, rows_focused, th);
                row_rects.push((*index, rect));
            }
        }
    }

    let current = cursor.and_then(|i| issues.get(i));
    let body_title = current
        .map(|i| i.identifier.clone())
        .unwrap_or_else(|| "Linear".into());
    let mut block = panel_block(&body_title, false, th);
    let body_inner = block.inner(body_a);
    let read_a = reading_area(body_inner);
    let lines: Vec<Line> = match current {
        Some(issue) => body_lines(issue, read_a.width as usize, now, th),
        None => Vec::new(),
    };
    let max_scroll = (lines.len() as u16).saturating_sub(read_a.height.max(1));
    let scroll = view.scroll.min(max_scroll);
    if max_scroll > 0 {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {}/{} ", scroll + 1, lines.len()),
                Style::default().fg(th.dim),
            ))
            .right_aligned(),
        );
    }
    f.render_widget(block, body_a);
    let browser_area = match current {
        Some(_) => crate::ui::browser_button(
            f,
            body_a,
            (body_title.chars().count() + 2) as u16,
            app.hover_crumb == Some(HitTarget::ModalBrowser),
            th,
        ),
        None => Rect::default(),
    };
    // orion's side of the issue at the border's right end, left of the
    // `↗`: kept apart from Linear's properties under it.
    let border_work = cursor
        .and_then(|c| works.get(&c).cloned().flatten())
        .filter(|_| browser_area.width > 0);
    if let Some(work) = border_work {
        let spans = border_work_spans(&work, spin, th);
        let w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let title_end = body_a.x + 1 + body_title.chars().count() as u16 + 2;
        if let Some(x) = browser_area
            .x
            .checked_sub(w as u16 + 1)
            .filter(|x| *x > title_end)
        {
            let at = Rect {
                x,
                y: body_a.y,
                width: w as u16,
                height: 1,
            };
            f.render_widget(Paragraph::new(Line::from(spans)), at);
        }
    }
    let mut filter_pick = view.filter_pick;
    if let Some(pick) = &view.prop_pick {
        let me = app.linear.get(&view.project).and_then(|l| l.me_id());
        draw_prop_pick(f, body_inner, pick, me, th);
    } else if let Some(pick) = &view.worktree_pick {
        draw_worktree_pick(f, body_inner, pick, &view.project, &app.linear_links, th);
    } else if let Some(pick) = &mut filter_pick {
        let facets = pick_facets(&issues, view.tab, th);
        pick.clamp(&facets);
        crate::list_filter::draw_pick(f, body_inner, &facets, &parsed, pick, th);
    } else {
        let shown: Vec<Line> = lines.iter().skip(scroll as usize).cloned().collect();
        f.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), read_a);
    }
    // The modal's keys along its bottom edge — none while a box over it
    // has the keys.
    if !backdrop {
        let reserve = if max_scroll > 0 { 12 } else { 0 };
        crate::hints::draw_on_border(f, area, &hints(view), reserve, th);
    }

    if let Some(Overlay::Linear(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = rows_area;
        v.list_start = list_start;
        v.row_rects = row_rects;
        v.tab_hits = tab_hits;
        v.tab_row = tab_row;
        v.filter_pick = filter_pick;
        v.body_area = body_inner;
        v.browser_area = browser_area;
        v.view_height = read_a.height;
        v.body_lines = lines.len();
        if let Some(index) = cursor {
            v.selected = index;
        }
        v.scroll = scroll;
    }
}

/// Where an issue's state stands, as a glyph — `◑` started, `○` not yet,
/// `◌` in the backlog, `◇` in triage — in the state's own Linear colour,
/// else quieter the further off it is.
fn state_mark(issue: &LinearIssue, th: Theme) -> (&'static str, ratatui::style::Color) {
    state_glyph(&issue.status_type, &issue.state_color, th)
}

/// [`state_mark`] from a state's kind and Linear colour alone — what a
/// todo linked to an issue has of it (`todos`). A finished issue, which
/// the lists here never hold, is a filled `●`.
pub(crate) fn state_glyph(
    kind: &str,
    color: &str,
    th: Theme,
) -> (&'static str, ratatui::style::Color) {
    let (glyph, fallback) = match kind {
        "started" => ("◑", th.muted),
        "unstarted" => ("○", th.dim),
        "backlog" => ("◌", th.faint),
        "triage" => ("◇", th.warn),
        "completed" => ("●", th.done),
        _ => ("·", th.dim),
    };
    (glyph, crate::theme::hex(color).unwrap_or(fallback))
}

/// [`priority_letter`] as one styled span.
pub(crate) fn priority_mark(priority: u8, th: Theme) -> Span<'static> {
    let (letter, style) = priority_letter(priority, th);
    Span::styled(letter, style)
}

/// A priority as one letter on a row — `U`rgent, `H`igh, `M`edium,
/// `L`ow — louder the higher it is; a faint `·` for none.
fn priority_letter(priority: u8, th: Theme) -> (&'static str, Style) {
    match priority {
        1 => (
            "U",
            Style::default().fg(th.err).add_modifier(Modifier::BOLD),
        ),
        2 => ("H", Style::default().fg(th.warn)),
        3 => ("M", Style::default().fg(th.muted)),
        4 => ("L", Style::default().fg(th.dim)),
        _ => ("·", Style::default().fg(th.faint)),
    }
}

/// A row, one line: the mark's tick, the state glyph, the priority's
/// letter, `ENG-12` dim and the title, the chars the filter matched lit.
/// The rest of the issue is the reading pane's.
fn title_spans(
    issue: &LinearIssue,
    positions: &[usize],
    marked: bool,
    budget: usize,
    work: &WorkColumn,
    th: Theme,
) -> Vec<Span<'static>> {
    // A marked row is ticked in the accent — it is a choice the keys
    // made, not a status, so not the `●` a session's STATUS MARK is.
    let mut spans = vec![if marked {
        Span::styled(
            "✓ ",
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("  ")
    }];
    let (glyph, color) = state_mark(issue, th);
    spans.push(Span::styled(
        format!("{glyph} "),
        Style::default().fg(color),
    ));
    let (letter, style) = priority_letter(issue.priority, th);
    spans.push(Span::styled(format!("{letter} "), style));
    // The tick, the state glyph and the priority, each with its space.
    const MARKS_W: usize = 6;
    let full = issue.label();
    let room = budget.saturating_sub(MARKS_W + work.width());
    let label = truncate(&full, room);
    let positions = visible_positions(positions, &label, &full);
    let ident_w = issue.identifier.chars().count().min(label.chars().count());
    let split = positions.partition_point(|&p| p < ident_w);
    let ident: String = label.chars().take(ident_w).collect();
    let rest: String = label.chars().skip(ident_w).collect();
    spans.extend(fuzzy_highlight_styled(
        &ident,
        &positions[..split],
        Style::default().fg(th.dim),
        th,
    ));
    let rest_positions: Vec<usize> = positions[split..].iter().map(|p| p - ident_w).collect();
    spans.extend(fuzzy_highlight_styled(
        &rest,
        &rest_positions,
        Style::default().fg(th.text),
        th,
    ));
    if work.width() > 0 {
        spans.push(Span::raw(
            " ".repeat(room.saturating_sub(label.chars().count())),
        ));
        spans.extend(work.spans(th));
    }
    spans
}

/// [`IssueWork`] on the page's border, the row's marks with the branch
/// spelled out: ` ◐ ⎇ eng-12-fix… ↗ #42 ready `, the branch cut to
/// [`BORDER_BRANCH_W`].
fn border_work_spans(work: &IssueWork, spin: Option<usize>, th: Theme) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];
    if let Some((status, unseen)) = work.session {
        spans.push(crate::ui::status_dot(Some(status), unseen, spin, th));
    }
    spans.push(scope_span(work, th));
    if let Some(branch) = &work.branch {
        spans.push(Span::styled(
            format!("{} ", truncate(branch, BORDER_BRANCH_W)),
            Style::default().fg(th.muted),
        ));
    }
    if let Some(pr) = work.pr {
        spans.extend(pr_spans(pr, 0, 0, th));
        spans.push(Span::raw(" "));
    }
    spans
}

/// The most of a branch's name the page's border shows.
const BORDER_BRANCH_W: usize = 24;

/// Where the work is, as the launcher's band marks a checkout: `⎇` for an
/// orion worktree, `⇢` for a pull request opened outside orion, dim both.
fn scope_span(work: &IssueWork, th: Theme) -> Span<'static> {
    let glyph = match (&work.branch, work.pr) {
        (Some(_), _) => "⎇ ",
        (None, Some(_)) => "⇢ ",
        (None, None) => "  ",
    };
    Span::styled(glyph, Style::default().fg(th.dim))
}

/// A pull request as the band draws one ([`crate::pr_row::look`]): `↗ `,
/// `#42` right-aligned to `num_w` and the state word padded to `word_w`.
fn pr_spans(pr: WorkPr, num_w: usize, word_w: usize, th: Theme) -> Vec<Span<'static>> {
    let look = pr.look(th);
    vec![
        Span::styled("↗ ", Style::default().fg(look.glyph)),
        Span::styled(
            format!("{:>num_w$}", format!("#{}", pr.number)),
            Style::default().fg(look.label),
        ),
        Span::styled(
            format!(" {:<word_w$}", pr.word()),
            Style::default().fg(look.badge),
        ),
    ]
}

/// How wide the work column's pull request runs on a list: the widest
/// `#42` and state word; both 0 when no row has a pull request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WorkWidths {
    num: usize,
    word: usize,
}

impl WorkWidths {
    /// `None` when no row has work: the list has no column at all.
    fn of<'a>(works: impl Iterator<Item = &'a IssueWork>) -> Option<Self> {
        works.fold(None, |widths, work| {
            let mut widths = widths.unwrap_or(WorkWidths::default());
            if let Some(pr) = work.pr {
                widths.num = widths.num.max(pr.number.to_string().len() + 1);
                widths.word = widths.word.max(pr.word().len());
            }
            Some(widths)
        })
    }

    /// The cells a row's `↗ #42 ready` takes: 0 on a list without one.
    fn pr(self) -> usize {
        if self.num == 0 {
            return 0;
        }
        "↗ ".chars().count() + self.num + 1 + self.word
    }
}

/// The work column at a row's right end: the sessions' STATUS MARK, where
/// the work is ([`scope_span`]), then the pull request as the band draws
/// it, its number and word in columns as wide as the list's widest. A row
/// with a pull request and an orion worktree shows both. Blank on a row
/// nothing has, and no column at all on a list where nothing does.
pub(crate) struct WorkColumn {
    work: Option<IssueWork>,
    widths: Option<WorkWidths>,
    spin: Option<usize>,
}

impl WorkColumn {
    #[cfg(test)]
    pub(crate) fn none() -> Self {
        WorkColumn {
            work: None,
            widths: None,
            spin: None,
        }
    }

    /// The cells the column takes, the space before it and after it
    /// included.
    fn width(&self) -> usize {
        // ` ● ⎇ `, then `↗ #42 ready ` when the list has a pull request.
        self.widths
            .map_or(0, |w| 6 + if w.pr() > 0 { w.pr() + 1 } else { 0 })
    }

    fn spans(&self, th: Theme) -> Vec<Span<'static>> {
        let Some(widths) = self.widths else {
            return Vec::new();
        };
        let Some(work) = &self.work else {
            return vec![Span::raw(" ".repeat(self.width()))];
        };
        let mut spans = vec![
            Span::raw(" "),
            match work.session {
                Some((status, unseen)) => {
                    crate::ui::status_dot(Some(status), unseen, self.spin, th)
                }
                None => Span::raw("  "),
            },
            scope_span(work, th),
        ];
        if widths.pr() > 0 {
            match work.pr {
                Some(pr) => spans.extend(pr_spans(pr, widths.num, widths.word, th)),
                None => spans.push(Span::raw(" ".repeat(widths.pr()))),
            }
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(" "));
        spans
    }
}

/// `Oct 5` for an RFC 3339 stamp this year, `Oct 5 2025` for one before.
fn short_date(stamp: &str, now: i64) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut ymd = stamp
        .get(..10)?
        .splitn(3, '-')
        .map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let month = MONTHS.get(usize::try_from(m - 1).ok()?)?;
    const SECS_PER_DAY: i64 = 86_400;
    let (this_year, _, _) = orion_core::crashlog::civil_from_days(now.div_euclid(SECS_PER_DAY));
    Some(if y >= this_year {
        format!("{month} {d}")
    } else {
        format!("{month} {d} {y}")
    })
}

/// A property picker in the reading pane's place: what it is for, then a
/// row per value — a state with its kind dim beside it, a priority with
/// its letter and digit, the configured user (`me`, by id) as `Me` — the
/// cursor's lit and the one the issue stands on marked.
fn draw_prop_pick(f: &mut Frame, area: Rect, pick: &PropPick, me: Option<&str>, th: Theme) {
    let dim = Style::default().fg(th.dim);
    let heading = match pick.prop {
        Prop::Status => format!("Move {} to…", pick.identifier),
        Prop::Priority => format!("Set {}'s priority to…", pick.identifier),
        Prop::Assignee => format!("Assign {} to…", pick.identifier),
    };
    let rows = pick.rows.iter().enumerate().map(|(i, row)| {
        let mut spans = match row {
            Change::Status(state) => vec![
                Span::raw(state.name.clone()),
                Span::styled(format!("  {}", state.kind), dim),
            ],
            Change::Priority(priority) => vec![
                priority_mark(*priority, th),
                Span::raw(format!(" {}", priority_word(*priority))),
                Span::styled(format!("  {priority}"), dim),
            ],
            Change::Assignee(Some(user)) if me == Some(user.id.as_str()) => vec![
                Span::raw("Me"),
                Span::styled(format!("  {}", user.name), dim),
            ],
            Change::Assignee(Some(user)) => vec![Span::raw(user.name.clone())],
            Change::Assignee(None) => vec![Span::raw("No assignee")],
        };
        if pick.current == Some(i) {
            spans.push(Span::styled("  current", dim));
        }
        spans
    });
    draw_side_pick(f, area, heading, rows, pick.selected, th);
}

fn draw_worktree_pick(
    f: &mut Frame,
    area: Rect,
    pick: &WorktreePick,
    project: &ProjectId,
    links: &LinkStore,
    th: Theme,
) {
    let rows = pick.branches.iter().map(|branch| {
        let mut spans = vec![Span::raw(branch.clone())];
        let waiting = links.pending(project, branch);
        if !waiting.is_empty() {
            spans.push(Span::styled(
                format!("  {} waiting", waiting.join(", ")),
                Style::default().fg(th.dim),
            ));
        }
        // Refused too often to be tried again: linking it here again does.
        let refused = links.gave_up_on(project, branch);
        if !refused.is_empty() {
            spans.push(Span::styled(
                format!("  {} · couldn't attach", refused.join(", ")),
                Style::default().fg(th.dim),
            ));
        }
        spans
    });
    draw_side_pick(
        f,
        area,
        format!("Attach {} to the PR opened from…", ids_of(&pick.issues)),
        rows,
        pick.selected,
        th,
    );
}

/// A picker in the reading pane's place: `heading` in bold, then `rows`
/// under a blank line, scrolled to keep `selected` in view.
fn draw_side_pick<'a>(
    f: &mut Frame,
    area: Rect,
    heading: String,
    rows: impl Iterator<Item = Vec<Span<'a>>>,
    selected: usize,
    th: Theme,
) {
    if let Some(row) = row_rect(area, 0) {
        f.render_widget(
            Paragraph::new(Span::styled(
                heading,
                Style::default().add_modifier(Modifier::BOLD),
            )),
            row,
        );
    }
    const HEAD_ROWS: u16 = 2;
    let list = Rect {
        y: area.y.saturating_add(HEAD_ROWS),
        height: area.height.saturating_sub(HEAD_ROWS),
        ..area
    };
    let start = window_start(selected, list.height as usize);
    for (i, spans) in rows.enumerate().skip(start) {
        let Some(row) = row_rect(list, i - start) else {
            break;
        };
        render_row(f, row, spans, i == selected, true, th);
    }
}

/// The widest the description runs: a line past this is hard to read.
const READ_MAX_W: u16 = 88;

/// The reading pane's text area: a column in from either edge, no wider
/// than [`READ_MAX_W`].
fn reading_area(inner: Rect) -> Rect {
    let pad = if inner.width > 4 { 1 } else { 0 };
    Rect {
        x: inner.x + pad,
        width: inner.width.saturating_sub(pad * 2).min(READ_MAX_W),
        ..inner
    }
}

/// The issue's properties at the head of the page, as the PULL REQUEST
/// PAGE carries its own under its border: a dim name, then the value in
/// Linear's colours — status and priority, assignee and reporter, side by
/// side while the pane has room for two ([`PAIR_W`] each), one per row
/// otherwise; the project and the labels on rows of their own; then the
/// dates.
fn properties(issue: &LinearIssue, width: usize, now: i64, th: Theme) -> Vec<Line<'static>> {
    const NAME_W: usize = 10;
    let paired = width >= PAIR_W * 2;
    let cell_w = if paired { PAIR_W } else { width };
    let room = cell_w.saturating_sub(NAME_W + 1);
    let name = |n: &str| Span::styled(format!("{n:<NAME_W$}"), Style::default().fg(th.dim));
    let text = |t: &str, style: Style| Span::styled(truncate(t, room.saturating_sub(2)), style);
    let plain = Style::default().fg(th.text);
    let quiet = Style::default().fg(th.dim);
    let dot = |tag: &LinearTag| {
        let color = crate::theme::hex(&tag.color).unwrap_or(th.muted);
        vec![
            Span::styled("● ", Style::default().fg(color)),
            text(&tag.name, plain),
        ]
    };
    let cell = |label: &str, value: Vec<Span<'static>>| {
        let mut spans = vec![name(label)];
        spans.extend(value);
        spans
    };
    let (glyph, color) = state_mark(issue, th);
    let status = cell(
        "Status",
        vec![
            Span::styled(format!("{glyph} "), Style::default().fg(color)),
            text(&issue.status, plain),
        ],
    );
    let priority = cell(
        "Priority",
        vec![
            priority_mark(issue.priority, th),
            Span::raw(" "),
            text(priority_word(issue.priority), plain),
        ],
    );
    // Someone by name, or what stands for nobody.
    let person = |label: &str, name: &str, nobody: &'static str| {
        cell(
            label,
            vec![if name.is_empty() {
                Span::styled(nobody, quiet)
            } else {
                text(name, plain)
            }],
        )
    };
    let assignee = person("Assignee", &issue.assignee, UNASSIGNED);
    let reporter = person("Reporter", &issue.reporter, "unknown");
    let project = cell(
        "Project",
        match &issue.project {
            Some(project) => dot(project),
            None => vec![Span::styled("none", quiet)],
        },
    );
    let dates: Vec<Vec<Span<'static>>> = [
        ("Created", &issue.created_at),
        ("Updated", &issue.updated_at),
        ("Completed", &issue.completed_at),
    ]
    .into_iter()
    .filter_map(|(label, stamp)| {
        let day = short_date(stamp, now)?;
        Some(cell(
            label,
            vec![Span::styled(day, Style::default().fg(th.muted))],
        ))
    })
    .collect();
    let mut labels = vec![name("Labels")];
    if issue.labels.is_empty() {
        labels.push(Span::styled("none", quiet));
    }
    for (i, label) in issue.labels.iter().enumerate() {
        if i > 0 {
            labels.push(Span::raw("   "));
        }
        labels.extend(dot(label));
    }
    let mut lines = Vec::new();
    let pair = |lines: &mut Vec<Line<'static>>, cells: Vec<Vec<Span<'static>>>| {
        if paired {
            let mut spans = Vec::new();
            for (i, cell) in cells.into_iter().enumerate() {
                if i > 0 {
                    let used: usize = spans.iter().map(|s: &Span| s.content.chars().count()).sum();
                    spans.push(Span::raw(" ".repeat(PAIR_W.saturating_sub(used))));
                }
                spans.extend(cell);
            }
            lines.push(crate::pr_preview::fit(spans, width));
        } else {
            for cell in cells {
                lines.push(crate::pr_preview::fit(cell, width));
            }
        }
    };
    pair(&mut lines, vec![status, priority]);
    pair(&mut lines, vec![assignee, reporter]);
    lines.push(crate::pr_preview::fit(project, width));
    lines.push(crate::pr_preview::fit(labels, width));
    if !dates.is_empty() {
        pair(&mut lines, dates);
    }
    lines
}

/// One property's column when two sit side by side ([`properties`]).
const PAIR_W: usize = 34;

/// The reading pane's text, `width` wide: the title, wrapped and bold,
/// the properties under it over a rule, then the description.
fn body_lines(issue: &LinearIssue, width: usize, now: i64, th: Theme) -> Vec<Line<'static>> {
    let title = Style::default().fg(th.text).add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = crate::pr_preview::wrap(&issue.title, width)
        .into_iter()
        .map(|row| Line::from(Span::styled(row, title)))
        .collect();
    lines.push(Line::default());
    lines.extend(properties(issue, width, now, th));
    lines.push(Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(th.faint),
    )));
    lines.push(Line::default());
    if issue.description.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "No description",
            Style::default().fg(th.dim),
        )));
    } else {
        lines.extend(markdown::render(
            &issue.description,
            width.max(20),
            Breaks::Hard,
            Style::default(),
            th,
        ));
    }
    lines
}

// ---- Linear HTTP (key never on argv) ----

/// What each listed issue is read with: the row and its meta line, the
/// reading pane, its team's workflow states for `⌘S`, and the pull
/// requests attached to it for the work column — the same fields whoever's
/// issues are asked for.
const ISSUE_FIELDS: &str =
    "id identifier title url description priority createdAt updatedAt completedAt \
    state { name type color position } labels { nodes { name color } } project { name color } \
    assignee { id displayName } creator { displayName } externalUserCreator { name } \
    team { id states { nodes { id name type position color } } } \
    attachments(first: 25) { nodes { url metadata } pageInfo { hasNextPage } }";

/// How many of the configured user's issues one ask lists.
const MINE_LIMIT: usize = 100;
/// How many of the rest of their teams' — Linear's most a page holds, the
/// most recently touched first.
pub const OTHERS_LIMIT: usize = 250;

/// How many of the configured user's teams `⌘I` knows the members of,
/// and how many of each: an issue in a team past that is still theirs or
/// nobody's to make it.
const TEAMS_LIMIT: usize = 25;
const MEMBERS_LIMIT: usize = 100;

/// Not done and not canceled: the open issues.
const OPEN_STATES: &str = r#"state: { type: { nin: ["completed", "canceled"] } }"#;

/// How far back the LINEAR VIEW's done issues reach: a week of what was
/// finished, under the open ones, as the PULL REQUESTS MODAL keeps a week
/// of merges (`pull_request::MERGED_DAYS`).
pub const DONE_DAYS: u32 = 7;
/// How many done issues each tab's ask lists, the most recently touched
/// first.
const DONE_LIMIT: usize = 50;

/// Done — not canceled — within [`DONE_DAYS`]: Linear reads the duration
/// back from now, so the week is cut where the issues are kept and an
/// older one is never sent.
fn done_states() -> String {
    format!(r#"state: {{ type: {{ eq: "completed" }} }} completedAt: {{ gt: "-P{DONE_DAYS}D" }}"#)
}

/// Both tabs' issues in one ask, as aliased lists: `mine`, assigned to
/// the key's owner — or to `email`, when Settings → Linear account names
/// someone — and `others`, open issues in that person's teams assigned to
/// someone else or to nobody; then `mineDone` and `othersDone`, the same
/// two cut to the issues done lately ([`done_states`]). With them `me`,
/// who that person is, and their `teams`' members: who `⌘I` assigns to.
async fn fetch_lists(dir: &Path, email: &str) -> Result<LinearList, String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let (query, variables) = lists_query(email);
    let json = graphql(&key, &query, variables).await?;
    parse_lists(&json)
}

/// [`fetch_lists`]' query and its variables.
fn lists_query(email: &str) -> (String, serde_json::Value) {
    // `me` picks out the configured user — as an assignee, and among a
    // team's members.
    let (head, me, not_me, me_field, variables) = if email.is_empty() {
        (
            "query",
            "isMe: { eq: true }",
            "isMe: { eq: false }",
            "me: viewer { id displayName }",
            serde_json::json!({}),
        )
    } else {
        (
            "query($email: String!)",
            "email: { eq: $email }",
            "email: { neq: $email }",
            "me: users(first: 1, filter: { email: { eq: $email } }) { nodes { id displayName } }",
            serde_json::json!({ "email": email }),
        )
    };
    let done = done_states();
    let query = format!(
        r#"{head} {{
          mine: issues(first: {MINE_LIMIT}, orderBy: updatedAt, filter: {{
            assignee: {{ {me} }}
            {OPEN_STATES}
          }}) {{ nodes {{ {ISSUE_FIELDS} }} pageInfo {{ hasNextPage }} }}
          others: issues(first: {OTHERS_LIMIT}, orderBy: updatedAt, filter: {{
            team: {{ members: {{ some: {{ {me} }} }} }}
            or: [{{ assignee: {{ null: true }} }}, {{ assignee: {{ {not_me} }} }}]
            {OPEN_STATES}
          }}) {{ nodes {{ {ISSUE_FIELDS} }} pageInfo {{ hasNextPage }} }}
          mineDone: issues(first: {DONE_LIMIT}, orderBy: updatedAt, filter: {{
            assignee: {{ {me} }}
            {done}
          }}) {{ nodes {{ {ISSUE_FIELDS} }} }}
          othersDone: issues(first: {DONE_LIMIT}, orderBy: updatedAt, filter: {{
            team: {{ members: {{ some: {{ {me} }} }} }}
            or: [{{ assignee: {{ null: true }} }}, {{ assignee: {{ {not_me} }} }}]
            {done}
          }}) {{ nodes {{ {ISSUE_FIELDS} }} }}
          {me_field}
          teams(first: {TEAMS_LIMIT}, filter: {{ members: {{ some: {{ {me} }} }} }}) {{
            nodes {{ id members(first: {MEMBERS_LIMIT}) {{ nodes {{ id displayName }} }} }}
          }}
        }}"#
    );
    (query, variables)
}

/// Set `change` on issue `issue_id`: its state, priority or assignee —
/// a null `assigneeId` is nobody.
async fn update_issue(dir: &Path, issue_id: &str, change: &Change) -> Result<(), String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let (field, gql_type, value, refusal) = match change {
        Change::Status(state) => (
            "stateId",
            "String!",
            serde_json::json!(state.id),
            "Linear did not move the issue",
        ),
        Change::Priority(priority) => (
            "priority",
            "Int!",
            serde_json::json!(priority),
            "Linear did not set the priority",
        ),
        Change::Assignee(user) => (
            "assigneeId",
            "String",
            serde_json::json!(user.as_ref().map(|u| u.id.as_str())),
            "Linear did not assign the issue",
        ),
    };
    let query = format!(
        r#"mutation($id: String!, ${field}: {gql_type}) {{
          issueUpdate(id: $id, input: {{ {field}: ${field} }}) {{ success }}
        }}"#
    );
    let mut variables = serde_json::json!({ "id": issue_id });
    variables[field] = value;
    let json = graphql(&key, &query, variables).await?;
    mutation_result(&json, "issueUpdate", refusal)
}

/// The fields a linked todo's chip draws, as [`LinkedIssue::from_json`]
/// reads them.
const LINKED_FIELDS: &str = "identifier url state { name type color } priority";

/// Every issue in `identifiers` in one ask, one aliased `issue(id:)` per
/// identifier, each paired with the identifier it was asked by — a team
/// move renames `ENG-12`, and Linear still finds it by the old name. One
/// Linear cannot find (deleted) is left out: it fails the whole ask —
/// `issue` is never null, so Linear nulls `data` — and then each is asked
/// on its own, the missing ones failing alone. An error every one of them
/// hits (the key) is the answer's.
async fn fetch_linked(
    dir: &Path,
    identifiers: &[String],
) -> Result<Vec<(String, LinkedIssue)>, String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let json = graphql(&key, &linked_query(identifiers), serde_json::json!({})).await?;
    let first = match parse_linked(&json, identifiers) {
        Ok(found) => return Ok(found),
        Err(err) if identifiers.len() == 1 => return Err(err),
        Err(err) => err,
    };
    let mut found = Vec::new();
    let mut failed = 0;
    for id in identifiers {
        let one = std::slice::from_ref(id);
        match graphql(&key, &linked_query(one), serde_json::json!({}))
            .await
            .and_then(|json| parse_linked(&json, one))
        {
            Ok(issues) => found.extend(issues),
            Err(_) => failed += 1,
        }
    }
    if failed == identifiers.len() {
        return Err(first);
    }
    Ok(found)
}

/// [`fetch_linked`]'s query: `query { i0: issue(id: "RIP-412") { … } … }`.
fn linked_query(identifiers: &[String]) -> String {
    let fields: Vec<String> = identifiers
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let id = serde_json::Value::String(id.clone());
            format!("i{i}: issue(id: {id}) {{ {LINKED_FIELDS} }}")
        })
        .collect();
    format!("query {{ {} }}", fields.join(" "))
}

fn parse_linked(
    json: &serde_json::Value,
    identifiers: &[String],
) -> Result<Vec<(String, LinkedIssue)>, String> {
    let Some(data) = json.get("data").filter(|d| d.is_object()) else {
        return Err(graphql_error(json).unwrap_or_else(|| "Linear said nothing".into()));
    };
    Ok(identifiers
        .iter()
        .enumerate()
        .filter_map(|(i, asked)| {
            let issue = LinkedIssue::from_json(data.get(format!("i{i}"))?)?;
            Some((asked.clone(), issue))
        })
        .collect())
}

/// The teams the key in `dir` can file into, each with its triage state.
async fn fetch_teams(dir: &Path) -> Result<Vec<LinearTeam>, String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let json = graphql(
        &key,
        "query { teams { nodes { id key name triageEnabled states { nodes { id type } } } } }",
        serde_json::json!({}),
    )
    .await?;
    parse_teams(&json)
}

fn parse_teams(json: &serde_json::Value) -> Result<Vec<LinearTeam>, String> {
    let Some(nodes) = json.pointer("/data/teams/nodes").and_then(|v| v.as_array()) else {
        return Err(graphql_error(json).unwrap_or_else(|| "Linear listed no teams".into()));
    };
    let text = |v: &serde_json::Value, key: &str| {
        v.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(nodes
        .iter()
        .map(|team| {
            let triage = team
                .get("triageEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let triage_state = team
                .pointer("/states/nodes")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .find(|s| s.get("type").and_then(|t| t.as_str()) == Some("triage"))
                .map(|s| text(s, "id"))
                .filter(|id| triage && !id.is_empty());
            LinearTeam {
                id: text(team, "id"),
                key: text(team, "key"),
                name: text(team, "name"),
                triage_state,
            }
        })
        .collect())
}

/// `draft` filed in `team` — into its Triage, set explicitly: an issue a
/// team member makes through the API lands in the default state
/// otherwise.
async fn create_issue(
    dir: &Path,
    team: &LinearTeam,
    draft: &IssueDraft,
) -> Result<LinkedIssue, String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let mut input = serde_json::json!({
        "teamId": team.id,
        "title": draft.title,
        "priority": draft.priority,
        "description": draft.description,
    });
    if let Some(state) = &team.triage_state {
        input["stateId"] = serde_json::Value::String(state.clone());
    }
    let json = graphql(
        &key,
        &format!(
            "mutation($input: IssueCreateInput!) {{ issueCreate(input: $input) {{ success issue {{ {LINKED_FIELDS} }} }} }}"
        ),
        serde_json::json!({ "input": input }),
    )
    .await?;
    mutation_result(&json, "issueCreate", "Linear did not create the issue")?;
    json.pointer("/data/issueCreate/issue")
        .and_then(LinkedIssue::from_json)
        .ok_or_else(|| "Linear did not say which issue it made".into())
}

/// The team to file in: the remembered one, else the only one. None when
/// there are several to pick from — or the remembered one is gone.
fn pick_team(teams: &[LinearTeam], remembered: Option<&str>) -> Option<LinearTeam> {
    if let Some(team) = remembered.and_then(|id| teams.iter().find(|t| t.id == id)) {
        return Some(team.clone());
    }
    match teams {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Ask Linear, off the loop, how the issues the TODOS MODAL in `dir`
/// links to stand now: they land as [`LinearAnswer::Linked`]. One ask per
/// checkout at a time; asked again while one is out, it is owed, and
/// asked afresh — with the identifiers linked by then — as soon as that
/// one lands.
pub(crate) fn request_linked(app: &mut App, dir: PathBuf, identifiers: Vec<String>) {
    if identifiers.is_empty() {
        return;
    }
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    let Some(ticket) = app
        .linear_linked
        .flights
        .begin_fresh(dir.clone(), crate::fetch::now())
    else {
        return;
    };
    tokio::spawn(async move {
        let result = fetch_linked(&dir, &identifiers).await;
        let _ = tx.send(LinearAnswer::Linked { ticket, result });
    });
}

/// Where **Create in Triage** files: a team the user just picked, or the
/// one the list remembers (by id), if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamChoice {
    Picked(LinearTeam),
    Remembered(Option<String>),
}

/// The footer's line while **Create in Triage** waits on Linear.
pub(crate) const CREATING: &str = "creating the issue in Linear…";

/// **Create in Triage** for todo `item`, off the loop: the teams asked
/// for unless one was picked, the issue made in the one [`pick_team`]
/// finds — or, with several and none remembered, the teams sent back to
/// pick from ([`LinearAnswer::Teams`]).
pub(crate) fn create_for_todo(
    app: &mut App,
    dir: PathBuf,
    item: u64,
    draft: IssueDraft,
    choice: TeamChoice,
) {
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    // One create per todo at a time: a second Enter while Linear is
    // still answering would file it twice.
    if !app.todo_creates.insert((dir.clone(), item)) {
        return;
    }
    app.flash = Some(crate::flash::Flash::working(CREATING));
    tokio::spawn(async move {
        let team = match choice {
            TeamChoice::Picked(team) => team,
            TeamChoice::Remembered(remembered) => match fetch_teams(&dir).await {
                Err(err) => {
                    let _ = tx.send(LinearAnswer::Teams {
                        dir,
                        item,
                        result: Err(err),
                    });
                    return;
                }
                Ok(teams) => match pick_team(&teams, remembered.as_deref()) {
                    Some(team) => team,
                    None => {
                        let _ = tx.send(LinearAnswer::Teams {
                            dir,
                            item,
                            result: Ok(teams),
                        });
                        return;
                    }
                },
            },
        };
        let result = create_issue(&dir, &team, &draft).await;
        let _ = tx.send(LinearAnswer::Created {
            dir,
            item,
            team,
            result,
        });
    });
}

/// A mutation's answer: `Ok` when `data.<field>.success` is true, else
/// Linear's own error, else `refused`.
fn mutation_result(json: &serde_json::Value, field: &str, refused: &str) -> Result<(), String> {
    let success = json
        .pointer(&format!("/data/{field}/success"))
        .and_then(|v| v.as_bool());
    if success == Some(true) {
        return Ok(());
    }
    Err(graphql_error(json).unwrap_or_else(|| refused.to_string()))
}

/// The issues' nodes in a [`fetch_lists`] answer, each with whether it
/// came in the `mine` list. None when the answer has neither list.
fn issue_nodes(json: &serde_json::Value) -> Option<Vec<(&serde_json::Value, bool)>> {
    let mine = json.pointer("/data/mine/nodes").and_then(|v| v.as_array());
    let others = json
        .pointer("/data/others/nodes")
        .and_then(|v| v.as_array());
    if mine.is_none() && others.is_none() {
        return None;
    }
    // The done lists ride along: an answer without them lists the open
    // issues alone.
    let done = |path: &str| json.pointer(path).and_then(|v| v.as_array());
    let mine_done = done("/data/mineDone/nodes");
    let others_done = done("/data/othersDone/nodes");
    let mut nodes = Vec::new();
    for (list, is_mine) in [
        (mine, true),
        (others, false),
        (mine_done, true),
        (others_done, false),
    ] {
        nodes.extend(list.into_iter().flatten().map(|n| (n, is_mine)));
    }
    Some(nodes)
}

/// Each team's workflow states, from the issues' `team` fields, in
/// Linear's `position` order.
fn parse_states(json: &serde_json::Value) -> HashMap<String, Vec<LinearState>> {
    let mut out: HashMap<String, Vec<LinearState>> = HashMap::new();
    for team in issue_nodes(json)
        .into_iter()
        .flatten()
        .filter_map(|(n, _)| n.get("team"))
    {
        let Some(id) = team.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if out.contains_key(id) {
            continue;
        }
        let mut states: Vec<(f64, LinearState)> = team
            .pointer("/states/nodes")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|s| {
                Some((
                    s.get("position").and_then(|p| p.as_f64()).unwrap_or(0.0),
                    LinearState {
                        id: s.get("id")?.as_str()?.to_string(),
                        name: s.get("name")?.as_str()?.to_string(),
                        kind: s
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        color: s
                            .get("color")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    },
                ))
            })
            .collect();
        states.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.insert(id.to_string(), states.into_iter().map(|(_, s)| s).collect());
    }
    out
}

/// A node's `{ id displayName }` as someone to assign to; None without
/// either.
fn user_at(value: &serde_json::Value) -> Option<LinearUser> {
    let user = LinearUser {
        id: json_text(value, "/id"),
        name: json_text(value, "/displayName"),
    };
    (!user.id.is_empty() && !user.name.is_empty()).then_some(user)
}

/// The configured user in a [`fetch_lists`] answer: the `viewer`, or the
/// one user Settings → Linear account's email found.
fn parse_me(json: &serde_json::Value) -> Option<LinearUser> {
    let me = json.pointer("/data/me")?;
    user_at(me.pointer("/nodes/0").unwrap_or(me))
}

/// Each of the configured user's teams' members, by team id, by name.
fn parse_members(json: &serde_json::Value) -> HashMap<String, Vec<LinearUser>> {
    let teams = json.pointer("/data/teams/nodes").and_then(|v| v.as_array());
    teams
        .into_iter()
        .flatten()
        .filter_map(|team| {
            let id = team.get("id")?.as_str()?.to_string();
            let mut members: Vec<LinearUser> = team
                .pointer("/members/nodes")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(user_at)
                .collect();
            members.sort_by_key(|user| user.name.to_lowercase());
            Some((id, members))
        })
        .collect()
}

/// The pull request at `url` linked to the issue as one that closes it —
/// the attachment's id, or `None` when Linear had it linked already
/// ([`already_attached`]).
///
/// `linkKind: closes` is what makes the link count: Linear's pull request
/// automations (in progress when it opens, the team's merge state when it
/// merges) move only the issues a pull request closes, as one whose branch
/// or title names the issue does. Left out, Linear files the link as
/// `links`, which shows the pull request on the issue and moves nothing.
async fn attach_pr(dir: &Path, issue_id: &str, url: &str) -> Result<Option<String>, String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let json = graphql(
        &key,
        r#"mutation($issueId: String!, $url: String!) {
          attachmentLinkGitHubPR(issueId: $issueId, url: $url, linkKind: closes) {
            success
            attachment { id }
          }
        }"#,
        serde_json::json!({ "issueId": issue_id, "url": url }),
    )
    .await?;
    match mutation_result(
        &json,
        "attachmentLinkGitHubPR",
        "Linear did not attach the pull request",
    ) {
        Ok(()) => Ok(json
            .pointer("/data/attachmentLinkGitHubPR/attachment/id")
            .and_then(|id| id.as_str())
            .map(str::to_string)),
        Err(why) if already_attached(&why) => Ok(None),
        Err(why) => Err(why),
    }
}

/// The attachment `attachment` dropped from its issue (`attachmentDelete`).
async fn detach_pr(dir: &Path, attachment: &str) -> Result<(), String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let json = graphql(
        &key,
        r#"mutation($id: String!) { attachmentDelete(id: $id) { success } }"#,
        serde_json::json!({ "id": attachment }),
    )
    .await?;
    mutation_result(
        &json,
        "attachmentDelete",
        "Linear did not drop the pull request",
    )
}

/// Whether Linear refused an attach because the pull request is on the
/// issue already — its own GitHub integration links one whose branch or
/// title names the issue, often before orion asks. That is the link made,
/// not a failure.
fn already_attached(why: &str) -> bool {
    why.to_ascii_lowercase().contains("duplicate attachment")
}

async fn graphql(
    key: &str,
    query: &str,
    variables: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // A test never reaches Linear: it answers from the stub it set, or
    // not at all.
    #[cfg(test)]
    {
        GRAPHQL_SENT.lock().unwrap().push(variables);
        let stub = *GRAPHQL_STUB.lock().unwrap();
        match stub {
            Some(stub) => stub(key, query),
            None => Err("no network in tests".into()),
        }
    }
    #[cfg(not(test))]
    curl_graphql(key, query, variables).await
}

/// What a test answers Linear's GraphQL with: `(key, query)` in, the
/// JSON Linear would have sent back out.
#[cfg(test)]
type GraphqlStub = fn(&str, &str) -> Result<serde_json::Value, String>;

/// The stub [`graphql`] answers from under test — none, and it fails.
#[cfg(test)]
static GRAPHQL_STUB: std::sync::Mutex<Option<GraphqlStub>> = std::sync::Mutex::new(None);

/// The variables of every request [`graphql`] was asked to send under
/// test, in order — what a test reads to see which issue and which pull
/// request an ATTACH named.
#[cfg(test)]
static GRAPHQL_SENT: std::sync::Mutex<Vec<serde_json::Value>> = std::sync::Mutex::new(Vec::new());

/// What [`graphql`] has been asked to send since this test's
/// [`with_graphql_stub`] began.
#[cfg(test)]
pub(crate) fn graphql_sent() -> Vec<serde_json::Value> {
    GRAPHQL_SENT.lock().unwrap().clone()
}

/// Run `f` with Linear's GraphQL answered by `stub`, one test at a time.
#[cfg(test)]
pub(crate) fn with_graphql_stub<T>(stub: GraphqlStub, f: impl FnOnce() -> T) -> T {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    GRAPHQL_SENT.lock().unwrap().clear();
    *GRAPHQL_STUB.lock().unwrap() = Some(stub);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    *GRAPHQL_STUB.lock().unwrap() = None;
    out.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// One GraphQL request to Linear through `curl`, the key on its stdin
/// config and the body in a temp file, never on argv.
#[cfg(not(test))]
async fn curl_graphql(
    key: &str,
    query: &str,
    variables: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({ "query": query, "variables": variables }).to_string();
    // Body stays off argv (and off the key's stdin config). std, not the
    // `tempfile` crate: that one is a test-only dep of this crate.
    let body_path = std::env::temp_dir().join(format!(
        "orion-linear-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&body_path, body.as_bytes()).map_err(|e| e.to_string())?;
    let _cleanup = DeleteOnDrop(body_path.clone());
    let config =
        format!("header = \"Authorization: {key}\"\nheader = \"Content-Type: application/json\"\n");
    let mut cmd = tokio::process::Command::new("curl");
    cmd.args([
        "-sS",
        "--max-time",
        TIMEOUT_SECS,
        "--config",
        "-",
        "-X",
        "POST",
        LINEAR_URL,
        "--data-binary",
        &format!("@{}", body_path.display()),
    ])
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("couldn't run curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(config.as_bytes()).await;
    }
    let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Linear request failed: {}",
            err.lines().next().unwrap_or("curl error")
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "Linear returned something that wasn't JSON".into())
}

/// A [`fetch_lists`] answer as the list keeps it: both lists' issues,
/// their teams' states, whether Linear had more of either, and the
/// issues whose attachments it cut short.
fn parse_lists(json: &serde_json::Value) -> Result<LinearList, String> {
    let more = |path: &str| {
        json.pointer(path)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let prs_cut = issue_nodes(json)
        .into_iter()
        .flatten()
        .filter(|(node, _)| {
            node.pointer("/attachments/pageInfo/hasNextPage")
                .and_then(|v| v.as_bool())
                == Some(true)
        })
        .filter_map(|(node, _)| Some(node.get("id")?.as_str()?.to_string()))
        .collect();
    Ok(LinearList {
        list: parse_issues(json)?,
        states: parse_states(json),
        me: parse_me(json),
        members: parse_members(json),
        more: more("/data/others/pageInfo/hasNextPage"),
        more_mine: more("/data/mine/pageInfo/hasNextPage"),
        prs_cut,
    })
}

/// The issues of both lists, in the order the LINEAR VIEW's sections go:
/// by where their state stands (triage, todo, the backlog, started,
/// done, duplicate, cancelled), the state's place in its workflow, its name, then — for the done — the
/// latest finished first, then priority — urgent first, none last —
/// and the most recently touched first.
fn parse_issues(json: &serde_json::Value) -> Result<Vec<LinearIssue>, String> {
    if let Some(err) = graphql_error(json) {
        return Err(err);
    }
    let Some(nodes) = issue_nodes(json) else {
        return Err("Linear returned no issue list".into());
    };
    let mut issues: Vec<LinearIssue> = nodes
        .iter()
        .filter_map(|(node, mine)| {
            issue_from(node).map(|issue| LinearIssue {
                mine: *mine,
                ..issue
            })
        })
        .collect();
    issues.sort_by(|a, b| {
        status_rank(a)
            .cmp(&status_rank(b))
            .then_with(|| a.status_order.cmp(&b.status_order))
            .then_with(|| a.status.cmp(&b.status))
            .then_with(|| b.completed_at.cmp(&a.completed_at))
            .then_with(|| priority_rank(a.priority).cmp(&priority_rank(b.priority)))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.identifier.cmp(&b.identifier))
    });
    Ok(issues)
}

/// A node's `{ name color }` as a tag; None without a name.
fn tag_at(value: &serde_json::Value) -> Option<LinearTag> {
    let name = value.get("name")?.as_str()?.to_string();
    let color = value
        .get("color")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();
    (!name.is_empty()).then_some(LinearTag { name, color })
}

/// The string at `path` in an answer's node, or empty.
fn json_text(value: &serde_json::Value, path: &str) -> String {
    value
        .pointer(path)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// A node's `priority`, on Linear's 0–4 scale whatever it says.
fn json_priority(value: &serde_json::Value) -> u8 {
    value
        .get("priority")
        .and_then(|p| p.as_u64())
        .unwrap_or(0)
        .min(4) as u8
}

fn issue_from(value: &serde_json::Value) -> Option<LinearIssue> {
    let text = |path: &str| json_text(value, path);
    Some(LinearIssue {
        priority: json_priority(value),
        state_color: text("/state/color"),
        labels: value
            .pointer("/labels/nodes")
            .and_then(|n| n.as_array())
            .into_iter()
            .flatten()
            .filter_map(tag_at)
            .collect(),
        project: value.get("project").and_then(tag_at),
        assignee: text("/assignee/displayName"),
        assignee_id: text("/assignee/id"),
        reporter: Some(text("/creator/displayName"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| text("/externalUserCreator/name")),
        mine: false,
        created_at: text("/createdAt"),
        updated_at: text("/updatedAt"),
        completed_at: text("/completedAt"),
        id: value.get("id")?.as_str()?.to_string(),
        identifier: value.get("identifier")?.as_str()?.to_string(),
        title: value
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        url: value.get("url")?.as_str()?.to_string(),
        description: value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status: value
            .pointer("/state/name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status_type: value
            .pointer("/state/type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status_order: value
            .pointer("/state/position")
            .and_then(|v| v.as_f64())
            .map_or(0, |p| (p * 1000.0).round() as i64),
        team_id: value
            .pointer("/team/id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        prs: value
            .pointer("/attachments/nodes")
            .and_then(|n| n.as_array())
            .into_iter()
            .flatten()
            .filter_map(IssuePr::from_json)
            .collect(),
    })
}

fn status_rank(issue: &LinearIssue) -> u8 {
    // Duplicate is a cancelled state to Linear, but reads as closed
    // rather than dropped: before cancelled.
    if issue.status.eq_ignore_ascii_case("duplicate") {
        return 5;
    }
    match issue.status_type.as_str() {
        "triage" => 0,
        "unstarted" => 1,
        "backlog" => 2,
        "started" => 3,
        "completed" => 4,
        "canceled" => 6,
        _ => 7,
    }
}

#[cfg(not(test))]
struct DeleteOnDrop(PathBuf);

#[cfg(not(test))]
impl Drop for DeleteOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn graphql_error(json: &serde_json::Value) -> Option<String> {
    json.get("errors")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|err| err.get("message"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// `LINEAR_API_KEY` from the project's env files, then the process env.
/// Symlinks are followed only when they stay inside the checkout or its
/// parent (the usual `../<repo>` layout of a worktree's main folder).
pub fn read_linear_key(dir: &Path) -> Option<String> {
    find_key(dir).map(|(key, _)| key)
}

/// Where [`read_linear_key`] would find the key — what the LINEAR TAB's
/// **API key** row says. The key is read to know it is there, and dropped.
pub fn key_source(dir: &Path) -> Option<KeySource> {
    find_key(dir).map(|(_, source)| source)
}

fn find_key(dir: &Path) -> Option<(String, KeySource)> {
    for name in ENV_FILES {
        let Some(path) = readable_env_file(dir, name) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(key) = parse_env_key(&text) {
            return Some((key, KeySource::File(name)));
        }
    }
    std::env::var(KEY_NAME)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|key| (key, KeySource::Environment))
}

/// The value the LINEAR TAB (and the onboarding wizard's Linear page)
/// shows on a status row ([`SettingKind::is_status`]) for the selected
/// project: where its key comes from, and what the last **Test
/// connection** said. None for any other row. Never the key.
///
/// [`SettingKind::is_status`]: crate::config::SettingKind::is_status
pub fn status_value(app: &App, kind: crate::config::SettingKind) -> Option<String> {
    use crate::config::SettingKind;
    if !kind.is_status() {
        return None;
    }
    let Some(project) = app.selected_project() else {
        return Some("no project selected".into());
    };
    let dir = &project.repo_path;
    Some(match kind {
        SettingKind::LinearKey => match key_source(dir) {
            Some(source) => format!("{} · {}", source.label(), project.name),
            None => format!("not found for {}", project.name),
        },
        // The last test of this project's key — while it is still the
        // key it tested: one found, moved or dropped since says nothing.
        _ => match &app.linear_test {
            Some((tested, source, test)) if tested == dir && *source == key_source(dir) => {
                test_label(test)
            }
            _ => "not tested".into(),
        },
    })
}

/// A test's word for the row: `testing…`, `✓ Jane Doe · jane@acme.com`,
/// `✗ <why>`.
fn test_label(test: &LinearTest) -> String {
    match test {
        LinearTest::Testing => "testing…".into(),
        LinearTest::Passed(v) => match (v.name.is_empty(), v.email.is_empty()) {
            (false, false) => format!("✓ {} · {}", v.name, v.email),
            (false, true) => format!("✓ {}", v.name),
            (true, _) => format!("✓ {}", v.email),
        },
        LinearTest::Failed(err) => format!("✗ {err}"),
    }
}

/// **Test connection**: ask Linear, with the selected project's key, who
/// that key belongs to — the `viewer` query, off the loop. The row says
/// `testing…` until the answer lands ([`land_answer`]). No key is an
/// answer at once, with no call made.
pub(crate) fn test_connection(app: &mut App) {
    let Some(dir) = app.selected_project().map(|p| p.repo_path.clone()) else {
        return;
    };
    let Some((key, source)) = find_key(&dir) else {
        if let Some(Overlay::Settings(view)) = &mut app.overlay {
            view.warn(format!("Linear: {NO_KEY}"));
        }
        app.linear_test = Some((dir, None, LinearTest::Failed(NO_KEY.into())));
        app.dirty = true;
        return;
    };
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    app.linear_test = Some((dir.clone(), Some(source), LinearTest::Testing));
    app.dirty = true;
    tokio::spawn(async move {
        let result = fetch_viewer(&key).await;
        let _ = tx.send(LinearAnswer::Viewer { dir, result });
    });
}

/// What a project with no key says, wherever it is asked.
const NO_KEY: &str = "no LINEAR_API_KEY in this project's .env / .env.local (or the process env)";

async fn fetch_viewer(key: &str) -> Result<Viewer, String> {
    let json = graphql(
        key,
        "query { viewer { name email } }",
        serde_json::json!({}),
    )
    .await?;
    parse_viewer(&json)
}

/// Linear's answer to the `viewer` query: the account's name and email,
/// or the error it gave.
fn parse_viewer(json: &serde_json::Value) -> Result<Viewer, String> {
    if let Some(err) = graphql_error(json) {
        return Err(err);
    }
    let viewer = json
        .pointer("/data/viewer")
        .filter(|v| v.is_object())
        .ok_or_else(|| "Linear returned no account".to_string())?;
    let field = |key: &str| {
        viewer
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(Viewer {
        name: field("name"),
        email: field("email"),
    })
}

fn readable_env_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if meta.file_type().is_symlink() {
        let target = std::fs::canonicalize(&path).ok()?;
        let root = std::fs::canonicalize(dir).ok()?;
        let parent = root.parent().unwrap_or(&root);
        if target.starts_with(&root) || target.starts_with(parent) {
            return Some(target);
        }
        return None;
    }
    meta.is_file().then_some(path)
}

/// The value of `LINEAR_API_KEY` in an env-file body. Other names are ignored.
pub fn parse_env_key(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let (name, value) = line.split_once('=')?;
        if name.trim() != KEY_NAME {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn issue(id: &str, ident: &str, title: &str) -> LinearIssue {
        LinearIssue {
            id: id.into(),
            identifier: ident.into(),
            status_order: 0,
            title: title.into(),
            url: format!("https://linear.app/x/issue/{ident}"),
            description: String::new(),
            status: "In Progress".into(),
            status_type: "started".into(),
            team_id: "t1".into(),
            priority: 0,
            state_color: String::new(),
            labels: Vec::new(),
            project: None,
            assignee: String::new(),
            assignee_id: String::new(),
            reporter: String::new(),
            mine: true,
            created_at: String::new(),
            updated_at: String::new(),
            completed_at: String::new(),
            prs: Vec::new(),
        }
    }

    /// The teams' states ride the issue list, in Linear's order; `⌘S`
    /// lists them on the issue's own state, Enter moves the issue there —
    /// the row says so before Linear answers, and a refusal puts it back.
    #[test]
    fn ctrl_s_moves_an_issue_to_another_state() {
        let json = serde_json::json!({"data": {"mine": {"nodes": [
            {"id": "1", "identifier": "ENG-12", "title": "Login", "url": "https://linear.app/x/issue/ENG-12",
             "state": {"name": "In Progress", "type": "started"},
             "team": {"id": "t1", "states": {"nodes": [
                {"id": "s3", "name": "Done", "type": "completed", "position": 3.0},
                {"id": "s1", "name": "Todo", "type": "unstarted", "position": 1.0},
                {"id": "s2", "name": "In Progress", "type": "started", "position": 2.0}
             ]}}}
        ]}}});
        let issues = parse_issues(&json).unwrap();
        assert_eq!(issues[0].team_id, "t1");
        let states = parse_states(&json);
        let names: Vec<&str> = states["t1"].iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Todo", "In Progress", "Done"]);

        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.linear.insert(
            project.clone(),
            LinearList {
                list: issues,
                states,
                ..Default::default()
            },
        );
        app.overlay = Some(Overlay::Linear(LinearView::new(
            project.clone(),
            "demo".into(),
            PathBuf::from("/nonexistent"),
            LinearMode::Browse,
        )));
        let mut out = Vec::new();
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        handle_key(&mut app, ctrl_s, &mut out);
        let Some(Overlay::Linear(view)) = &app.overlay else {
            panic!("the modal stays");
        };
        let pick = view.prop_pick.as_ref().expect("the picker");
        assert_eq!(pick.selected, 1, "on the state it is in");
        assert_eq!(pick.current, Some(1));
        crate::hints::assert_hints_from(&hints(view), keys::ALL);
        handle_key(&mut app, KeyEvent::from(KeyCode::Down), &mut out);
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), &mut out);
        let Some(Overlay::Linear(view)) = &app.overlay else {
            panic!("the modal stays");
        };
        assert!(view.prop_pick.is_none());
        assert_eq!(app.linear[&project].list[0].status, "Done");

        let done = app.linear[&project].states["t1"][2].clone();
        land_answer(
            &mut app,
            LinearAnswer::Edited {
                project: project.clone(),
                issue_id: "1".into(),
                identifier: "ENG-12".into(),
                change: Change::Status(done),
                seq: 1,
                result: Err("not allowed".into()),
            },
        );
        assert_eq!(app.linear[&project].list[0].status, "In Progress");
        assert_eq!(
            app.flash.as_deref(),
            Some("couldn't move ENG-12: not allowed")
        );
    }

    #[test]
    fn template_expands_ids_and_first() {
        let issues = [
            issue("1", "ENG-12", "Login"),
            issue("2", "ENG-15", "Logout"),
        ];
        let out = expand_template("Fix {ids} starting with {first_id}\n{issues}", &issues);
        assert!(out.contains("ENG-12, ENG-15"));
        assert!(out.contains("starting with ENG-12"));
        assert!(out.contains("### ENG-12: Login\nIn Progress · https://linear.app/x/issue/ENG-12\n\n(no description)"));
        assert!(out.contains("### ENG-15: Logout"));
    }

    #[test]
    fn env_key_reads_only_linear() {
        let text = "OTHER=no\nLINEAR_API_KEY=lin_api_secret\nAWS_SECRET=x\n";
        assert_eq!(parse_env_key(text).as_deref(), Some("lin_api_secret"));
        assert_eq!(
            parse_env_key("export LINEAR_API_KEY='quoted'\n").as_deref(),
            Some("quoted")
        );
        assert_eq!(parse_env_key("FOO=bar\n"), None);
    }

    #[test]
    fn batch_names_and_branch() {
        let batch = LinearBatch {
            issues: vec![
                issue("1", "ENG-12", "Fix login redirect"),
                issue("2", "ENG-15", "x"),
            ],
            task: "go".into(),
        };
        assert_eq!(batch.ids(), "ENG-12, ENG-15");
        assert_eq!(batch.title(), "Linear ENG-12, ENG-15");
        let branch = batch.branch(&[]);
        assert_eq!(
            branch.split('-').count(),
            3,
            "a batch takes a random <adj>-<noun>-<verb> name: {branch}"
        );
        assert!(!branch.contains("eng"), "{branch}");
        let one = LinearBatch {
            issues: vec![issue("1", "ENG-12", "Fix login redirect")],
            task: "go".into(),
        };
        assert_eq!(one.branch(&[]), "eng-12-fix-login-redirect");
    }

    #[test]
    fn link_store_adds_to_what_a_branch_waits_on() {
        let p1 = ProjectId("p1".into());
        let mut store = LinkStore::default();
        store.remember(&p1, "feat", &[issue("1", "ENG-1", "a")]);
        store.remember(
            &p1,
            "feat",
            &[issue("1", "ENG-1", "a"), issue("2", "ENG-2", "b")],
        );
        assert_eq!(store.pending(&p1, "feat"), ["ENG-1", "ENG-2"]);
        assert!(store.pending(&p1, "other").is_empty());
        assert!(store.pending(&ProjectId("p2".into()), "feat").is_empty());
        let ids: Vec<String> = store
            .begin_attach(&p1, "feat")
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["1", "2"]);
    }

    #[test]
    fn link_store_remembers_and_takes() {
        let p1 = ProjectId("p1".into());
        let mut store = LinkStore::default();
        store.remember(&p1, "eng-12-fix", &[issue("abc", "ENG-12", "Fix")]);
        assert!(store.begin_attach(&p1, "other").is_none());
        let link = store.begin_attach(&p1, "eng-12-fix").unwrap();
        assert_eq!(link, [("abc".to_string(), "ENG-12".to_string())]);
        assert!(store.begin_attach(&p1, "eng-12-fix").is_none(), "out");
        store.finish_attach(&p1, "eng-12-fix", &["abc".into()], &[], true);
        assert!(store.begin_attach(&p1, "eng-12-fix").is_none(), "spent");
        assert!(store.pending(&p1, "eng-12-fix").is_empty());
    }

    /// A project with `files` written into a fresh checkout, selected.
    fn app_on(files: &[(&str, &str)]) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        let mut app = App::new();
        app.tree.projects.push(orion_core::Project {
            id: ProjectId("p1".into()),
            name: "demo".into(),
            repo_path: dir.path().into(),
            sort_order: 0,
        });
        (app, dir)
    }

    const FAKE_KEY: &str = "lin_api_test_never_shown";

    // ---- the ATTACH, from either end ----

    fn open_pr(number: u64, title: &str) -> crate::pull_request::OpenPr {
        crate::pull_request::OpenPr {
            number,
            title: title.into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            answered_draft: false,
            answered: Default::default(),
            head: format!("branch-{number}"),
            mine: false,
            head_sha: String::new(),
            meta: Default::default(),
        }
    }

    fn press(app: &mut App, key: KeyEvent) {
        crate::event_loop::handle_overlay_key(app, key, &mut Vec::new());
    }

    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// `demo` with a key in its `.env`, three issues assigned (ENG-1..3)
    /// and two pull requests open (#42, then #41), both lists landed just
    /// now, and an answer channel for Linear's.
    fn paired() -> (
        App,
        tempfile::TempDir,
        tokio::sync::mpsc::UnboundedReceiver<LinearAnswer>,
    ) {
        let line = format!("LINEAR_API_KEY={FAKE_KEY}\n");
        let (mut app, dir) = app_on(&[(".env", &line)]);
        let project = ProjectId("p1".into());
        app.linear.insert(
            project.clone(),
            LinearList {
                list: vec![
                    issue("1", "ENG-1", "Login"),
                    issue("2", "ENG-2", "Logout"),
                    issue("3", "ENG-3", "Signup"),
                ],
                states: HashMap::new(),
                ..Default::default()
            },
        );
        let now = std::time::Instant::now();
        app.open_prs.insert(
            project,
            crate::app::OpenPrs {
                merged: Vec::new(),
                list: vec![open_pr(42, "Fix login"), open_pr(41, "Spike")],
                at: now,
                due: now + std::time::Duration::from_secs(60),
                step: std::time::Duration::from_secs(60),
            },
        );
        // A list "in flight" is not asked for again: opening the view
        // stays on the loop, with no runtime under it.
        let _ = app
            .linear_flights
            .begin(ProjectId("p1".into()), std::time::Instant::now());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        app.linear_tx = Some(tx);
        (app, dir, rx)
    }

    /// The LINEAR VIEW browsing `demo`, ENG-1 and ENG-3 marked.
    fn browse_marked(app: &mut App) {
        open(app);
        // ↓ hands the rows the keys; `space` on the search line types.
        press(app, plain(KeyCode::Down));
        press(app, plain(KeyCode::Char(' ')));
        press(app, plain(KeyCode::Down));
        press(app, plain(KeyCode::Down));
        press(app, plain(KeyCode::Char(' ')));
        let Some(Overlay::Linear(view)) = &app.overlay else {
            panic!("the LINEAR VIEW, got {:?}", app.overlay);
        };
        assert_eq!(view.marked, BTreeSet::from(["1".into(), "3".into()]));
    }

    /// The view opens on the search line, where `space` is a word break;
    /// ↓ hands the rows the keys and `space` marks there; ↑ off the top
    /// row, or typing, hands them back.
    #[test]
    fn space_types_on_the_search_line_and_marks_on_the_rows() {
        let (mut app, _dir, _rx) = paired();
        open(&mut app);
        let view = |app: &App| match &app.overlay {
            Some(Overlay::Linear(view)) => view.clone(),
            other => panic!("the LINEAR VIEW, got {other:?}"),
        };
        assert_eq!(view(&app).focus, LinearFocus::Search);
        for c in "lo ".chars() {
            press(&mut app, plain(KeyCode::Char(c)));
        }
        assert_eq!(view(&app).query.as_str(), "lo ");
        assert!(view(&app).marked.is_empty(), "space typed, not marked");

        press(&mut app, plain(KeyCode::Down));
        assert_eq!(view(&app).focus, LinearFocus::List);
        press(&mut app, plain(KeyCode::Char(' ')));
        assert_eq!(view(&app).marked, BTreeSet::from(["1".into()]));
        assert_eq!(view(&app).query.as_str(), "lo ");

        press(&mut app, plain(KeyCode::Up));
        assert_eq!(view(&app).focus, LinearFocus::Search, "↑ off the top row");
        press(&mut app, plain(KeyCode::Down));
        press(&mut app, plain(KeyCode::Char('x')));
        assert_eq!(view(&app).focus, LinearFocus::Search, "typing goes back");
        assert_eq!(view(&app).query.as_str(), "lo x");
    }

    /// Typing in a Linear box never drops the issues: the text goes first
    /// and the marked issues follow in full, under the preset made from
    /// the box's own picker — which goes straight onto the box on save,
    /// its prefix written in. The box cuts a fresh worktree, never the
    /// project's root checkout (`dev` here).
    #[test]
    fn a_typed_task_keeps_the_issues_under_a_preset_made_on_the_spot() {
        let (mut app, dir, _rx) = paired();
        app.tree.worktrees.push(orion_core::Worktree {
            id: orion_core::WorktreeId("w1".into()),
            project_id: ProjectId("p1".into()),
            path: dir.path().into(),
            branch: "dev".into(),
            is_main: true,
            sort_order: 0,
        });
        let store = dir.path().join("agent_presets.json");
        crate::agent_presets::with_presets_path(store, || {
            browse_marked(&mut app);
            press(&mut app, plain(KeyCode::Enter));
            press(
                &mut app,
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::SUPER),
            );
            press(
                &mut app,
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::SUPER),
            );
            for c in "Linear Ticket".chars() {
                press(&mut app, plain(KeyCode::Char(c)));
            }
            let Some(Overlay::AgentPresetEditor(editor)) = &mut app.overlay else {
                panic!("the preset editor, got {:?}", app.overlay);
            };
            editor.prefix.insert_str("Plan first.");
            press(&mut app, plain(KeyCode::Enter));
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("the save hands the box back, got {:?}", app.overlay);
            };
            assert!(
                prompt.title.contains("Linear ENG-1, ENG-3 · Linear Ticket"),
                "{}",
                prompt.title
            );
            assert!(
                prompt.title.contains("new worktree"),
                "a fresh worktree, not dev: {}",
                prompt.title
            );
            assert_eq!(
                prompt.input.as_str(),
                "Plan first.\n\n",
                "the prefix is in the box"
            );
            for c in "go".chars() {
                press(&mut app, plain(KeyCode::Char(c)));
            }
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("the box, got {:?}", app.overlay);
            };
            let crate::app::PromptKind::QuickPrompt(launch) = &prompt.kind else {
                panic!("a quick prompt, got {:?}", prompt.kind);
            };
            let starting_prompt = launch.compose(prompt.input.as_str().trim());
            let mut out = Vec::new();
            crate::event_loop::handle_overlay_key(&mut app, plain(KeyCode::Enter), &mut out);
            assert!(
                matches!(
                    out.as_slice(),
                    [ClientRequest::CreateWorktree {
                        existing: false,
                        ..
                    }]
                ),
                "the worktree is cut first, the launch riding it: {out:?}"
            );
            assert_eq!(
                Some(starting_prompt.as_str()),
                Some(
                    "Plan first.\n\ngo\n\nThe Linear issues, in full — everything you need is here, no Linear access required:\n\n\
                     ### ENG-1: Login\nIn Progress · https://linear.app/x/issue/ENG-1\n\n(no description)\n\n\
                     ### ENG-3: Signup\nIn Progress · https://linear.app/x/issue/ENG-3\n\n(no description)"
                )
            );
        });
    }

    /// Press `attach` with Linear answered `attachmentLinkGitHubPR`, and
    /// land what it says: the variables each request named, in order.
    fn attach_through(
        app: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<LinearAnswer>,
        attach: impl FnOnce(&mut App),
    ) -> Vec<serde_json::Value> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        with_graphql_stub(
            |key, query| {
                assert_eq!(key, FAKE_KEY, "the project's key");
                assert!(query.contains("attachmentLinkGitHubPR"), "{query}");
                assert!(
                    query.contains("linkKind: closes"),
                    "a link Linear's automations move the issue on: {query}"
                );
                Ok(serde_json::json!({"data": {"attachmentLinkGitHubPR": {"success": true}}}))
            },
            || {
                rt.block_on(async {
                    attach(app);
                    assert_eq!(
                        app.flash.as_ref().map(|f| f.kind),
                        Some(crate::flash::FlashKind::Working),
                        "the footer spins while Linear is asked"
                    );
                    let answer = rx.recv().await.expect("an answer");
                    land_answer(app, answer);
                });
                graphql_sent()
            },
        )
    }

    fn attached(issue_id: &str, number: u64) -> serde_json::Value {
        serde_json::json!({
            "issueId": issue_id,
            "url": format!("https://github.com/o/r/pull/{number}"),
        })
    }

    /// The LINEAR VIEW's `⌘U` (`^V` its twin) flips to the PULL REQUESTS
    /// MODAL as a PR PICK for the issues it marked — titled for them,
    /// `Enter` named for the attach — and `Enter` on a pull request there
    /// links it to each through `attachmentLinkGitHubPR`, the footer saying
    /// so, and comes back to the LINEAR VIEW with the marks spent.
    #[test]
    fn cmd_u_attaches_the_marked_issues_to_a_picked_pull_request() {
        for flip in [
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::SUPER),
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
        ] {
            let (mut app, _dir, mut rx) = paired();
            browse_marked(&mut app);
            let Some(Overlay::Linear(view)) = &app.overlay else {
                unreachable!();
            };
            assert!(hints(view).iter().any(|h| h.does == "attach to PR"));
            press(&mut app, flip);
            let Some(Overlay::PullRequests(prs)) = &app.overlay else {
                panic!("the PR PICK, got {:?}", app.overlay);
            };
            let pick = prs.pick.as_ref().expect("a PR PICK");
            assert_eq!(pick.ids(), "ENG-1, ENG-3");
            let shown = crate::pr_modal::hints(prs);
            crate::hints::assert_hints_from(&shown, crate::pr_modal::keys::ALL);
            assert_eq!(shown[0].does, "attach to this PR");
            assert!(shown.iter().all(|h| h.does != "Linear"), "no ⌘L onward");
            assert_eq!(shown.last().map(|h| h.does.as_str()), Some("back"));
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
            term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let screen: String = term
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(
                screen.contains("Pull requests → ENG-1, ENG-3 — demo"),
                "{screen}"
            );

            press(&mut app, plain(KeyCode::Down));
            let sent = attach_through(&mut app, &mut rx, |app| press(app, plain(KeyCode::Enter)));
            assert_eq!(sent, [attached("1", 41), attached("3", 41)]);
            assert_eq!(
                app.flash.as_deref(),
                Some("attached PR #41 to ENG-1, ENG-3")
            );
            let Some(Overlay::Linear(view)) = &app.overlay else {
                panic!("back on the LINEAR VIEW, got {:?}", app.overlay);
            };
            assert_eq!(view.mode, LinearMode::Browse);
            assert!(view.marked.is_empty(), "the batch is spent");
            assert_eq!(view.selected, 2, "the cursor where it was");
        }
    }

    /// `paired` with a root worktree and two more: `branch-41`, whose
    /// pull request is open, and `feature-x`, which has none yet.
    fn paired_with_worktrees() -> (
        App,
        tempfile::TempDir,
        tokio::sync::mpsc::UnboundedReceiver<LinearAnswer>,
    ) {
        let (mut app, dir, rx) = paired();
        for (n, branch, is_main) in [
            (0, "dev", true),
            (2, "feature-x", false),
            (1, "branch-41", false),
        ] {
            app.tree.worktrees.push(orion_core::Worktree {
                id: orion_core::WorktreeId(format!("w{n}")),
                project_id: ProjectId("p1".into()),
                path: dir.path().join(branch),
                branch: branch.into(),
                is_main,
                sort_order: n,
            });
        }
        (app, dir, rx)
    }

    /// `⌘.` lists the project's worktrees but the root, and Enter on one
    /// with no pull request leaves the marked issues waiting on its
    /// branch, nothing asked of Linear yet — until OPEN PRS first lists a
    /// pull request from it, which is attached to them as a ⌘L launch's
    /// would be.
    #[test]
    fn cmd_period_links_the_marked_issues_to_a_worktree_without_a_pr() {
        let (mut app, _dir, mut rx) = paired_with_worktrees();
        browse_marked(&mut app);
        assert!(hints(the_view(&app))
            .iter()
            .any(|h| h.does == "link to worktree"));
        press(
            &mut app,
            KeyEvent::new(KeyCode::Char('.'), KeyModifiers::SUPER),
        );
        let view = the_view(&app);
        let pick = view.worktree_pick.as_ref().expect("the worktree picker");
        assert_eq!(pick.branches, ["branch-41", "feature-x"]);
        crate::hints::assert_hints_from(&hints(view), keys::ALL);
        let screen = shot(&mut app, 140, 40);
        assert!(
            screen.contains("Attach ENG-1, ENG-3 to the PR opened from…"),
            "{screen}"
        );

        press(&mut app, plain(KeyCode::Down));
        press(&mut app, plain(KeyCode::Enter));
        let view = the_view(&app);
        assert!(view.worktree_pick.is_none());
        assert!(view.marked.is_empty(), "the batch is spent");
        let project = ProjectId("p1".into());
        assert_eq!(
            app.linear_links.pending(&project, "feature-x"),
            ["ENG-1", "ENG-3"]
        );
        assert_eq!(
            app.flash.as_deref(),
            Some("ENG-1, ENG-3 will attach to the PR feature-x opens")
        );
        assert!(rx.try_recv().is_err(), "nothing asked of Linear yet");

        let previous = app.open_prs[&project].list.clone();
        let mut fresh = previous.clone();
        let mut opened = open_pr(43, "Feature X");
        opened.head = "feature-x".into();
        fresh.insert(0, opened);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let sent = with_graphql_stub(
            |_, _| Ok(serde_json::json!({"data": {"attachmentLinkGitHubPR": {"success": true}}})),
            || {
                rt.block_on(async {
                    attach_new_prs(&mut app, &project, Some(&previous), &fresh);
                    let answer = rx.recv().await.expect("an answer");
                    land_answer(&mut app, answer);
                });
                graphql_sent()
            },
        );
        let mut sent = sent;
        sent.sort_by_key(|v| v["issueId"].as_str().unwrap_or_default().to_string());
        assert_eq!(sent, [attached("1", 43), attached("3", 43)]);
        assert!(
            app.linear_links.pending(&project, "feature-x").is_empty(),
            "taken"
        );
    }

    /// A pull request from the root checkout's branch is the release
    /// (`dev` → `main`): it never takes the issues, even ones remembered
    /// for that branch, while a feature branch's in the same answer does.
    #[test]
    fn a_release_pr_from_the_root_branch_is_never_attached() {
        let (mut app, _dir, mut rx) = paired_with_worktrees();
        let project = ProjectId("p1".into());
        app.linear_links
            .remember(&project, "dev", &[issue("1", "ENG-1", "Login")]);
        app.linear_links
            .remember(&project, "feature-x", &[issue("3", "ENG-3", "Signup")]);
        let previous = app.open_prs[&project].list.clone();
        let mut fresh = previous.clone();
        for (number, head) in [(43, "feature-x"), (44, "dev")] {
            let mut opened = open_pr(number, head);
            opened.head = head.into();
            fresh.insert(0, opened);
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let sent = with_graphql_stub(
            |_, _| Ok(serde_json::json!({"data": {"attachmentLinkGitHubPR": {"success": true}}})),
            || {
                rt.block_on(async {
                    attach_new_prs(&mut app, &project, Some(&previous), &fresh);
                    let answer = rx.recv().await.expect("an answer");
                    land_answer(&mut app, answer);
                    assert!(rx.try_recv().is_err(), "one attach, not two");
                });
                graphql_sent()
            },
        );
        assert_eq!(sent, [attached("3", 43)]);
    }

    /// A worktree whose pull request is open already has nothing to wait
    /// for: Enter (`^T` the twin of `⌘.`) attaches it there and then.
    #[test]
    fn ctrl_t_on_a_worktree_with_an_open_pr_attaches_at_once() {
        let (mut app, _dir, mut rx) = paired_with_worktrees();
        browse_marked(&mut app);
        press(
            &mut app,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
        );
        let sent = attach_through(&mut app, &mut rx, |app| press(app, plain(KeyCode::Enter)));
        assert_eq!(sent, [attached("1", 41), attached("3", 41)]);
        assert_eq!(
            app.flash.as_deref(),
            Some("attached PR #41 to ENG-1, ENG-3")
        );
        assert!(app
            .linear_links
            .pending(&ProjectId("p1".into()), "branch-41")
            .is_empty());
    }

    /// Esc backs out of the PR PICK one step at a time — off the page,
    /// then a typed filter — and lands on the LINEAR VIEW as it was,
    /// marks and all, with nothing sent; `⌘L` there goes nowhere.
    #[test]
    fn esc_backs_out_of_the_pr_pick_with_the_marks_kept() {
        let (mut app, _dir, mut rx) = paired();
        browse_marked(&mut app);
        press(
            &mut app,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::SUPER),
        );
        press(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::SUPER),
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::PullRequests(v)) if v.pick.is_some()),
            "⌘L stays in the PR PICK: {:?}",
            app.overlay
        );
        press(&mut app, plain(KeyCode::Tab));
        press(&mut app, plain(KeyCode::Esc));
        press(&mut app, plain(KeyCode::Char('x')));
        press(&mut app, plain(KeyCode::Esc));
        assert!(
            matches!(&app.overlay, Some(Overlay::PullRequests(v)) if v.query.is_empty()),
            "the first Escs step off the page and clear: {:?}",
            app.overlay
        );
        press(&mut app, plain(KeyCode::Esc));
        let Some(Overlay::Linear(view)) = &app.overlay else {
            panic!("back on the LINEAR VIEW, got {:?}", app.overlay);
        };
        assert_eq!(view.mode, LinearMode::Browse);
        assert_eq!(view.marked, BTreeSet::from(["1".into(), "3".into()]));
        assert_eq!(view.selected, 2);
        assert!(app.flash.is_none());
        assert!(rx.try_recv().is_err(), "nothing asked of Linear");
    }

    /// Both ends run the one ATTACH: `⌘L` from a pull request, the same
    /// issues marked and `Enter`, sends Linear exactly what the PR PICK
    /// does and the footer says the same — and its Esc goes back to the
    /// pull request. A LINEAR VIEW opened from a pull request has no
    /// `⌘U` of its own.
    #[test]
    fn both_ends_attach_the_same_way() {
        let (mut app, _dir, mut rx) = paired();
        crate::pr_modal::open(&mut app);
        press(&mut app, plain(KeyCode::Down));
        press(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::SUPER),
        );
        let Some(Overlay::Linear(view)) = &app.overlay else {
            panic!("the LINEAR VIEW, got {:?}", app.overlay);
        };
        assert!(matches!(
            view.mode,
            LinearMode::Attach { pr_number: 41, .. }
        ));
        press(&mut app, plain(KeyCode::Down));
        press(&mut app, plain(KeyCode::Char(' ')));
        press(&mut app, plain(KeyCode::Down));
        press(&mut app, plain(KeyCode::Down));
        press(&mut app, plain(KeyCode::Char(' ')));
        press(
            &mut app,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::SUPER),
        );
        assert!(
            matches!(&app.overlay, Some(Overlay::Linear(_))),
            "no PR PICK from a pull request's own"
        );
        let sent = attach_through(&mut app, &mut rx, |app| press(app, plain(KeyCode::Enter)));
        assert_eq!(sent, [attached("1", 41), attached("3", 41)]);
        assert_eq!(
            app.flash.as_deref(),
            Some("attached PR #41 to ENG-1, ENG-3")
        );
        press(&mut app, plain(KeyCode::Esc));
        let Some(Overlay::PullRequests(prs)) = &app.overlay else {
            panic!("back on the pull request, got {:?}", app.overlay);
        };
        assert_eq!(prs.selected, 1);
        assert!(prs.pick.is_none());
    }

    /// A refusal names the first issue Linear would not take, and why.
    #[test]
    fn a_refused_attach_says_which_issue_and_why() {
        let mut app = App::new();
        land_answer(
            &mut app,
            LinearAnswer::Attached(AttachRun {
                project: None,
                branch: None,
                pr_url: "https://github.com/o/r/pull/41".into(),
                pr_number: 41,
                attached: vec![("1".into(), "ENG-1".into())],
                made: Vec::new(),
                refused: Some(("ENG-3".into(), "Entity not found".into())),
            }),
        );
        assert_eq!(
            app.flash.as_ref().map(|f| (f.kind, f.text.as_str())),
            Some((
                crate::flash::FlashKind::Failed,
                "couldn't attach PR #41 to ENG-3: Entity not found"
            ))
        );
    }

    /// Where the key was found, never what it is: `.env.local` before
    /// `.env`, then orion's environment — and no key says so.
    #[test]
    fn the_key_row_says_where_the_key_is_and_never_what() {
        use crate::config::SettingKind;
        let line = format!("LINEAR_API_KEY={FAKE_KEY}\n");
        let (app, _dir) = app_on(&[(".env", &line), (".env.local", &line)]);
        let shown = status_value(&app, SettingKind::LinearKey).unwrap();
        assert_eq!(shown, "found in .env.local · demo");
        let (app, _dir) = app_on(&[(".env", &line), (".env.local", "OTHER=1\n")]);
        let shown = status_value(&app, SettingKind::LinearKey).unwrap();
        assert_eq!(shown, "found in .env · demo");
        assert!(!shown.contains(FAKE_KEY));
        let (app, dir) = app_on(&[(".env", "OTHER=1\n")]);
        let shown = status_value(&app, SettingKind::LinearKey).unwrap();
        match std::env::var(KEY_NAME)
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            Some(_) => assert_eq!(key_source(dir.path()), Some(KeySource::Environment)),
            None => assert_eq!(shown, "not found for demo"),
        }
        assert_eq!(
            status_value(&app, SettingKind::LinearTest).as_deref(),
            Some("not tested")
        );
        assert_eq!(
            status_value(&app, SettingKind::LinearAccount),
            None,
            "not a status row"
        );
        assert_eq!(
            status_value(&App::new(), SettingKind::LinearKey).as_deref(),
            Some("no project selected")
        );
    }

    /// **Test connection** asks Linear's `viewer` with the project's key
    /// — stubbed here: no test reaches Linear — and the row says whose it
    /// is, or Linear's own error, the settings overlay saying it in full.
    #[test]
    fn test_connection_names_the_keys_account_or_the_error() {
        use crate::config::SettingKind;
        let line = format!("LINEAR_API_KEY={FAKE_KEY}\n");
        let (mut app, dir) = app_on(&[(".env", &line)]);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let run = |app: &mut App| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            app.linear_tx = Some(tx);
            rt.block_on(async {
                test_connection(app);
                assert_eq!(
                    status_value(app, SettingKind::LinearTest).as_deref(),
                    Some("testing…")
                );
                let answer = rx.recv().await.expect("an answer");
                land_answer(app, answer);
            });
        };
        with_graphql_stub(
            |key, query| {
                assert_eq!(key, FAKE_KEY, "the project's key, as read");
                assert!(query.contains("viewer"), "{query}");
                Ok(
                    serde_json::json!({"data": {"viewer": {"name": "Jane Doe", "email": "jane@acme.dev"}}}),
                )
            },
            || run(&mut app),
        );
        assert_eq!(
            status_value(&app, SettingKind::LinearTest).as_deref(),
            Some("✓ Jane Doe · jane@acme.dev")
        );
        assert_eq!(
            app.linear_test.as_ref().map(|(d, ..)| d.as_path()),
            Some(dir.path())
        );
        // A key moved since says nothing about this one.
        std::fs::rename(dir.path().join(".env"), dir.path().join(".env.local")).unwrap();
        assert_eq!(
            status_value(&app, SettingKind::LinearTest).as_deref(),
            Some("not tested")
        );
        std::fs::rename(dir.path().join(".env.local"), dir.path().join(".env")).unwrap();

        app.overlay = Some(Overlay::Settings(crate::app::SettingsView::new(
            0, 0, false,
        )));
        with_graphql_stub(
            |_, _| Ok(serde_json::json!({"errors": [{"message": "Authentication required"}]})),
            || run(&mut app),
        );
        assert_eq!(
            status_value(&app, SettingKind::LinearTest).as_deref(),
            Some("✗ Authentication required")
        );
        let Some(Overlay::Settings(view)) = &app.overlay else {
            panic!("the overlay stays");
        };
        assert_eq!(
            view.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("Linear: Authentication required")
        );

        // No key at all is an answer on the spot, with nothing sent.
        let (mut bare, _dir) = app_on(&[]);
        if std::env::var(KEY_NAME).is_err() {
            test_connection(&mut bare);
            assert_eq!(
                status_value(&bare, SettingKind::LinearTest),
                Some(format!("✗ {NO_KEY}"))
            );
        }
    }

    /// The LINEAR TAB drawn: every row — the switch on by default, the
    /// account, the template, where the key is and the test — and the
    /// key's value nowhere on screen.
    #[test]
    fn the_linear_tab_draws_every_row_and_never_the_key() {
        let line = format!("LINEAR_API_KEY={FAKE_KEY}\n");
        let (mut app, _dir) = app_on(&[(".env", &line)]);
        let tab = crate::config::SETTINGS_TABS
            .iter()
            .position(|t| t.title == "Linear")
            .expect("a Linear tab");
        let config = tempfile::tempdir().unwrap();
        crate::config::with_config_path(config.path().join("config.json"), || {
            app.overlay = Some(Overlay::Settings(crate::app::SettingsView::new(
                tab, 0, false,
            )));
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
            term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let buf = term.backend().buffer().clone();
            let screen: String = (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                        + "\n"
                })
                .collect();
            for needle in [
                " Linear ",
                "Link PRs to Linear",
                "[on]",
                "Linear account",
                "[the key's owner]",
                "Task template",
                "[default]",
                "Connection",
                "API key",
                "[found in .env · demo]",
                "Test connection",
                "[not tested]",
                "Enter toggle",
            ] {
                assert!(screen.contains(needle), "{needle}:\n{screen}");
            }
            assert!(
                !screen.contains(FAKE_KEY),
                "the key is never shown:\n{screen}"
            );
        });
    }

    #[test]
    fn parse_viewer_list() {
        let json = serde_json::json!({
            "data": {
                "mine": {
                    "nodes": [{
                        "id": "abc",
                        "identifier": "ENG-1",
                        "title": "T",
                        "url": "https://linear.app/x/issue/ENG-1",
                        "description": "d",
                        "priority": 2,
                        "state": { "name": "Todo", "type": "unstarted" }
                    }]
                }
            }
        });
        let list = parse_issues(&json).unwrap();
        assert_eq!(list[0].identifier, "ENG-1");
        assert_eq!(list[0].status_type, "unstarted");
        assert_eq!(priority_word(list[0].priority), "High");
        assert!(list[0].mine);
    }

    /// An issue with a priority, labels and a status of its own.
    fn rich(id: &str, ident: &str, title: &str, status: (&str, &str), priority: u8) -> LinearIssue {
        LinearIssue {
            status: status.0.into(),
            status_type: status.1.into(),
            priority,
            labels: vec![LinearTag {
                name: "Export PDF".into(),
                color: "#26b5ce".into(),
            }],
            created_at: "2026-10-05T22:44:07.232Z".into(),
            ..issue(id, ident, title)
        }
    }

    /// An app with `list` as the project's issues and the LINEAR VIEW up.
    fn view_on(list: Vec<LinearIssue>) -> App {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.linear.insert(
            project.clone(),
            LinearList {
                list,
                states: HashMap::new(),
                ..Default::default()
            },
        );
        app.overlay = Some(Overlay::Linear(LinearView::new(
            project,
            "demo".into(),
            PathBuf::from("/nonexistent"),
            LinearMode::Browse,
        )));
        app
    }

    fn the_view(app: &App) -> &LinearView {
        match &app.overlay {
            Some(Overlay::Linear(v)) => v,
            other => panic!("expected the LINEAR VIEW, got {other:?}"),
        }
    }

    fn shot(app: &mut App, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let Some(Overlay::Linear(v)) = app.overlay.clone() else {
                panic!("no LINEAR VIEW");
            };
            draw(f, app, &v, app.theme, false);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn selected_id(app: &App) -> Option<String> {
        selected_issue(app).map(|i| i.identifier.clone())
    }

    /// Both lists come in one answer: the configured user's (`mine`) and
    /// the rest of their teams' (`others`), each issue knowing which, with
    /// its priority, labels, project, assignee and state colour — sorted
    /// by state, then priority (none last), then the latest touched.
    #[test]
    fn both_lists_parse_with_their_meta() {
        let json = serde_json::json!({"data": {
            "mine": {"nodes": [
                {"id": "1", "identifier": "ENG-1", "title": "Low", "url": "https://linear.app/x/issue/ENG-1",
                 "priority": 4, "updatedAt": "2026-10-01T00:00:00Z",
                 "state": {"name": "Todo", "type": "unstarted", "color": "#26b5ce"}},
                {"id": "2", "identifier": "ENG-2", "title": "Urgent", "url": "https://linear.app/x/issue/ENG-2",
                 "priority": 1, "createdAt": "2026-10-05T22:44:07.232Z",
                 "labels": {"nodes": [{"name": "Export PDF", "color": "#26b5ce"}, {"name": ""}]},
                 "project": {"name": "Exports", "color": "#f2c94c"},
                 "assignee": {"id": "u1", "displayName": "me"},
                 "creator": {"displayName": "Ana"},
                 "externalUserCreator": {"name": "Someone on Slack"},
                 "state": {"name": "Todo", "type": "unstarted"}}
            ]},
            "others": {"nodes": [
                {"id": "3", "identifier": "ENG-3", "title": "Theirs", "url": "https://linear.app/x/issue/ENG-3",
                 "priority": 0, "assignee": null,
                 "creator": null, "externalUserCreator": {"name": "Someone on Slack"},
                 "state": {"name": "In Progress", "type": "started"}}
            ], "pageInfo": {"hasNextPage": true}},
            "me": {"id": "u1", "displayName": "me"},
            "teams": {"nodes": [
                {"id": "t1", "members": {"nodes": [
                    {"id": "u3", "displayName": "cy"},
                    {"id": "u2", "displayName": "Bo"},
                    {"id": "u1", "displayName": "me"},
                    {"id": "u4"}
                ]}}
            ]}
        }});
        let fetched = parse_lists(&json).unwrap();
        assert!(fetched.more);
        let ids: Vec<&str> = fetched.list.iter().map(|i| i.identifier.as_str()).collect();
        assert_eq!(
            ids,
            ["ENG-2", "ENG-1", "ENG-3"],
            "todo first, urgent before low, then started"
        );
        let urgent = &fetched.list[0];
        assert!(urgent.mine);
        assert_eq!(urgent.priority, 1);
        assert_eq!(urgent.labels.len(), 1, "a nameless label is dropped");
        assert_eq!(urgent.project.as_ref().unwrap().name, "Exports");
        assert_eq!(urgent.assignee, "me");
        assert_eq!(urgent.assignee_id, "u1");
        assert_eq!(urgent.reporter, "Ana", "whoever filed it in Linear");
        assert_eq!(fetched.list[1].state_color, "#26b5ce");
        assert_eq!(fetched.list[1].reporter, "", "Linear named nobody");
        assert!(!fetched.list[2].mine);
        assert_eq!(fetched.list[2].assignee, "");
        assert_eq!(fetched.list[2].assignee_id, "");
        assert_eq!(
            fetched.list[2].reporter, "Someone on Slack",
            "filed through an integration"
        );
        // Who `⌘I` assigns to: the configured user, and each team's
        // members by name, a nameless one dropped.
        assert_eq!(fetched.me, Some(user("u1", "me")));
        assert_eq!(
            fetched.members["t1"],
            [user("u2", "Bo"), user("u3", "cy"), user("u1", "me")]
        );
        // With Settings → Linear account set, `me` is the one user found.
        let by_email = serde_json::json!({"data": {
            "mine": {"nodes": []}, "others": {"nodes": []},
            "me": {"nodes": [{"id": "u2", "displayName": "Bo"}]}
        }});
        assert_eq!(parse_lists(&by_email).unwrap().me, Some(user("u2", "Bo")));
        let nobody = serde_json::json!({"data": {
            "mine": {"nodes": []}, "others": {"nodes": []}, "me": {"nodes": []}
        }});
        assert_eq!(parse_lists(&nobody).unwrap().me, None);
        // An answer with neither list is a miss.
        assert!(parse_lists(&serde_json::json!({"data": {}})).is_err());
    }

    /// The done lists land under the open issues, each on its own tab:
    /// the latest finished first, whatever its priority.
    #[test]
    fn the_done_lists_sort_under_the_open_issues_latest_first() {
        let done = |id: &str, at: &str, priority: u8| {
            serde_json::json!({
                "id": id, "identifier": format!("ENG-{id}"), "title": "Shipped",
                "url": format!("https://linear.app/x/issue/ENG-{id}"), "priority": priority,
                "completedAt": at, "state": {"name": "Done", "type": "completed"}
            })
        };
        let json = serde_json::json!({"data": {
            "mine": {"nodes": [
                {"id": "1", "identifier": "ENG-1", "title": "Open", "url": "https://linear.app/x/issue/ENG-1",
                 "state": {"name": "In Progress", "type": "started"}}
            ]},
            "others": {"nodes": []},
            "mineDone": {"nodes": [
                done("2", "2026-10-06T09:00:00.000Z", 1),
                done("3", "2026-10-09T09:00:00.000Z", 4)
            ]},
            "othersDone": {"nodes": [done("4", "2026-10-08T09:00:00.000Z", 0)]}
        }});
        let fetched = parse_lists(&json).unwrap();
        let ids: Vec<&str> = fetched.list.iter().map(|i| i.identifier.as_str()).collect();
        assert_eq!(ids, ["ENG-1", "ENG-3", "ENG-4", "ENG-2"]);
        let mine: Vec<bool> = fetched.list.iter().map(|i| i.mine).collect();
        assert_eq!(mine, [true, true, false, true]);
        assert_eq!(fetched.list[1].completed_at, "2026-10-09T09:00:00.000Z");
        assert!(fetched.list[0].completed_at.is_empty(), "open");
    }

    /// The one ask names both lists; with Settings → Linear account set,
    /// that person stands in for the key's owner on both.
    #[test]
    fn the_lists_query_asks_for_mine_and_the_teams_others() {
        let (query, vars) = lists_query("");
        assert!(query.contains("mine: issues(first: 100"), "{query}");
        assert!(query.contains("others: issues(first: 250"), "{query}");
        // Each has its done list beside it: finished, not canceled, and
        // cut to the week by Linear itself.
        for done in ["mineDone", "othersDone"] {
            let (_, list) = query
                .split_once(&format!("{done}: issues(first: 50"))
                .expect(done);
            let filter = list.split_once("nodes").expect("a filter").0;
            assert!(
                filter.contains(r#"state: { type: { eq: "completed" } }"#),
                "{filter}"
            );
            assert!(
                filter.contains(r#"completedAt: { gt: "-P7D" }"#),
                "{filter}"
            );
        }
        assert!(
            query.contains("assignee: { isMe: { eq: true } }"),
            "{query}"
        );
        assert!(
            query.contains("members: { some: { isMe: { eq: true } } }"),
            "{query}"
        );
        assert!(query.contains("{ assignee: { null: true } }"), "{query}");
        assert!(query.contains("pageInfo { hasNextPage }"), "{query}");
        assert!(query.contains("creator { displayName }"), "{query}");
        assert!(query.contains("me: viewer { id displayName }"), "{query}");
        assert!(
            query.contains(
                "teams(first: 25, filter: { members: { some: { isMe: { eq: true } } } })"
            ),
            "{query}"
        );
        assert_eq!(vars, serde_json::json!({}));
        let (query, vars) = lists_query("sam@x.co");
        assert!(query.starts_with("query($email: String!)"), "{query}");
        assert!(query.contains("email: { neq: $email }"), "{query}");
        assert!(!query.contains("isMe"), "{query}");
        assert!(
            query.contains("me: users(first: 1, filter: { email: { eq: $email } })"),
            "{query}"
        );
        assert!(
            query.contains(
                "teams(first: 25, filter: { members: { some: { email: { eq: $email } } } })"
            ),
            "{query}"
        );
        assert_eq!(vars, serde_json::json!({"email": "sam@x.co"}));
    }

    /// The view opens on `My issues`, grouped under a header per status
    /// with its count, each row reading its priority, labels and day
    /// under its title; ↓ skips the headers. `⇧→` and a click on the tab
    /// show `Other issues`, whose rows also say whose they are.
    #[test]
    fn tabs_split_mine_from_others_and_sections_group_by_status() {
        let mut theirs = rich("4", "ENG-4", "Their bug", ("Todo", "unstarted"), 3);
        theirs.mine = false;
        theirs.assignee = "Sam".into();
        let mut app = view_on(vec![
            rich("1", "ENG-1", "Started one", ("In Progress", "started"), 2),
            rich("2", "ENG-2", "Todo one", ("Todo", "unstarted"), 1),
            rich("3", "ENG-3", "Todo two", ("Todo", "unstarted"), 0),
            theirs,
        ]);
        let screen = shot(&mut app, 200, 40);
        assert!(screen.contains("My issues 3"), "{screen}");
        assert!(screen.contains("Other issues 1"), "{screen}");
        let at = |needle: &str| {
            screen
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} missing from\n{screen}"))
        };
        assert!(at("TODO 2") < at("ENG-2 Todo one"), "{screen}");
        assert!(at("ENG-3 Todo two") < at("IN PROGRESS 1"), "{screen}");
        assert!(at("IN PROGRESS 1") < at("ENG-1 Started one"), "{screen}");
        assert!(screen.contains("◑ H ENG-1 Started one"), "{screen}");
        assert!(screen.contains("○ U ENG-2 Todo one"), "{screen}");
        assert!(screen.contains("○ · ENG-3 Todo two"), "{screen}");
        assert!(!screen.contains("Their bug"), "{screen}");

        assert_eq!(selected_id(&app).as_deref(), Some("ENG-1"));
        // The first ↓ takes the keys off the search line, then ↑ moves.
        handle_key(&mut app, KeyEvent::from(KeyCode::Down), &mut Vec::new());
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-1"));
        handle_key(&mut app, KeyEvent::from(KeyCode::Up), &mut Vec::new());
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-3"));

        let shift_right = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        handle_key(&mut app, shift_right, &mut Vec::new());
        assert_eq!(the_view(&app).tab, LinearTab::Others);
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-4"));
        let screen = shot(&mut app, 200, 40);
        assert!(screen.contains("Priority  M Medium"), "{screen}");
        assert!(screen.contains("Assignee  Sam"), "{screen}");
        assert!(!screen.contains("Started one"), "{screen}");

        // A click on the first tab brings `My issues` back.
        let (from, _) = the_view(&app).tab_hits[0];
        let row = the_view(&app).tab_row;
        let at = Position::new(from + 1, row.y);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, at, &mut Vec::new());
        assert_eq!(the_view(&app).tab, LinearTab::Mine);
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-2"));
    }

    /// Tokens narrow by status, priority, label, project and assignee —
    /// beside the fuzzy words — within the tab showing.
    #[test]
    fn tokens_narrow_the_issues_by_facet() {
        let mut theirs = rich("4", "ENG-4", "Their bug", ("Todo", "unstarted"), 2);
        theirs.mine = false;
        theirs.assignee = "Sam".into();
        theirs.reporter = "Ana".into();
        let list = vec![
            rich("1", "ENG-1", "Started one", ("In Progress", "started"), 2),
            rich("2", "ENG-2", "Todo one", ("Todo", "unstarted"), 1),
            LinearIssue {
                labels: vec![LinearTag {
                    name: "bug".into(),
                    color: String::new(),
                }],
                ..rich("3", "ENG-3", "Todo two", ("Todo", "unstarted"), 0)
            },
            theirs,
        ];
        let shown = |tab: LinearTab, q: &str| -> Vec<String> {
            on_tab(tab, &filtered(q, &list), &list)
                .iter()
                .map(|(i, _)| list[*i].identifier.clone())
                .collect()
        };
        assert_eq!(shown(LinearTab::Mine, "p:high"), ["ENG-1"]);
        assert_eq!(
            shown(LinearTab::Mine, "p:urgent p:high"),
            ["ENG-2", "ENG-1"]
        );
        assert_eq!(shown(LinearTab::Mine, "p:none"), ["ENG-3"]);
        assert_eq!(shown(LinearTab::Mine, "status:todo label:bug"), ["ENG-3"]);
        assert_eq!(shown(LinearTab::Mine, "status:in-progress"), ["ENG-1"]);
        assert_eq!(
            shown(LinearTab::Mine, "label:export -label:bug"),
            ["ENG-2", "ENG-1"]
        );
        assert_eq!(shown(LinearTab::Others, "assignee:sam p:high"), ["ENG-4"]);
        assert_eq!(shown(LinearTab::Others, "reporter:ana"), ["ENG-4"]);
        assert!(shown(LinearTab::Mine, "reporter:ana").is_empty());
        assert_eq!(shown(LinearTab::Mine, "two"), ["ENG-3"]);
    }

    /// `⌘F` puts the FILTER PICK in the reading pane's place; `space` on a
    /// priority writes `priority:` into the filter line, the rows and the
    /// tab's count narrowing behind it, and Esc puts it away.
    #[test]
    fn the_filter_pick_writes_a_priority_token() {
        let mut app = view_on(vec![
            rich("1", "ENG-1", "Started one", ("In Progress", "started"), 2),
            rich("2", "ENG-2", "Todo one", ("Todo", "unstarted"), 1),
        ]);
        let mut out = Vec::new();
        let cmd_f = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::SUPER);
        handle_key(&mut app, cmd_f, &mut out);
        assert!(the_view(&app).filter_pick.is_some());
        crate::hints::assert_hints_from(&hints(the_view(&app)), crate::list_filter::keys::ALL);
        let screen = shot(&mut app, 200, 40);
        assert!(screen.contains("Status"), "{screen}");
        assert!(screen.contains("Priority"), "{screen}");
        // Status → Priority; Urgent is its first value.
        handle_key(&mut app, KeyEvent::from(KeyCode::Right), &mut out);
        handle_key(&mut app, KeyEvent::from(KeyCode::Char(' ')), &mut out);
        assert_eq!(the_view(&app).query.as_str(), "priority:\"Urgent\"");
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-2"));
        let screen = shot(&mut app, 200, 40);
        assert!(screen.contains("My issues 1"), "{screen}");
        assert!(screen.contains("(1/2)"), "{screen}");
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), &mut out);
        assert!(the_view(&app).filter_pick.is_none());
        assert_eq!(the_view(&app).query.as_str(), "priority:\"Urgent\"");
        assert!(
            matches!(app.overlay, Some(Overlay::Linear(_))),
            "Esc closed the picker only"
        );
    }

    /// A row is one line — the state glyph, the priority's letter, the
    /// identifier and title — a click on it selects it, and no row runs
    /// past a narrow pane. The labels are the reading pane's.
    #[test]
    fn rows_are_one_line_with_a_priority_letter() {
        let mut app = view_on(vec![
            rich(
                "1",
                "ENG-1",
                "A first issue with a long title",
                ("Todo", "unstarted"),
                2,
            ),
            rich("2", "ENG-2", "A second one", ("Todo", "unstarted"), 4),
        ]);
        let screen = shot(&mut app, 200, 40);
        assert!(screen.contains("○ H ENG-1 A first issue"), "{screen}");
        assert!(screen.contains("○ L ENG-2 A second one"), "{screen}");
        let (index, rect) = the_view(&app).row_rects[1];
        assert_eq!(rect.height, 1);
        let at = Position::new(rect.x + 2, rect.y);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, at, &mut Vec::new());
        assert_eq!(
            selected_issue(&app).map(|i| i.id.clone()),
            Some(rows(&app, &the_view(&app).project)[index].id.clone())
        );
        for w in [60u16, 90, 140] {
            shot(&mut app, w, 30);
            let budget = (the_view(&app).list_area.width as usize).saturating_sub(2);
            for issue in rows(&app, &the_view(&app).project) {
                let title = title_spans(issue, &[], true, budget, &WorkColumn::none(), app.theme);
                let width: usize = title.iter().map(|s| s.content.chars().count()).sum();
                assert!(width <= budget, "title at {w}");
            }
        }
    }

    /// The reading pane lists the issue's properties as Linear's sidebar
    /// does — in a column of their own when it is wide, under the title
    /// when it is not — and wraps a long title rather than cutting it.
    #[test]
    fn the_reading_pane_heads_the_text_with_the_properties() {
        let mut long = rich(
            "1",
            "ENG-1",
            "A title long enough that a narrow reading pane has to wrap it onto more lines",
            ("In Progress", "started"),
            2,
        );
        long.assignee = "Sam".into();
        long.reporter = "Ana".into();
        long.description = "Body text.".into();
        let mut app = view_on(vec![long]);
        let row = |screen: &str, needle: &str| {
            screen
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no {needle}\n{screen}"))
                .to_string()
        };
        let wide = shot(&mut app, 220, 40);
        // Two to a row while the pane has room: status beside priority,
        // assignee beside reporter; the project and the labels on rows of
        // their own.
        let status = row(&wide, "Status");
        assert!(
            status.contains("In Progress") && status.contains("High"),
            "{wide}"
        );
        let people = row(&wide, "Assignee");
        assert!(
            people.contains("Sam") && people.contains("Reporter") && people.contains("Ana"),
            "{wide}"
        );
        assert!(!row(&wide, "Project").contains("Labels"), "{wide}");
        assert!(row(&wide, "Labels").contains("● Export PDF"), "{wide}");

        let narrow = shot(&mut app, 110, 40);
        assert!(
            !row(&narrow, "Status").contains("Priority"),
            "one to a row\n{narrow}"
        );
        assert!(row(&narrow, "Reporter").contains("Ana"), "{narrow}");
        assert!(
            narrow.contains("it onto more lines"),
            "the title wraps\n{narrow}"
        );
        for screen in [&wide, &narrow] {
            let at = |needle: &str| screen.lines().position(|l| l.contains(needle)).unwrap();
            assert!(
                at("A title") < at("Status") && at("Status") < at("Body text."),
                "title, properties, text\n{screen}"
            );
        }
    }

    #[test]
    fn a_branch_names_an_issue_by_its_identifier_alone() {
        assert!(names_issue("eng-12-fix-login", "ENG-12"));
        assert!(names_issue("fix/ENG-12", "ENG-12"));
        assert!(names_issue("riplo-1004-riplo-1007-more", "RIPLO-1007"));
        assert!(!names_issue("eng-123-other", "ENG-12"));
        assert!(!names_issue("xeng-12", "ENG-12"));
        assert!(!names_issue("main", "ENG-12"));
    }

    /// A worktree in project `p1` on `branch`, as `w{n}`.
    fn worktree_on(app: &mut App, n: usize, branch: &str, is_main: bool) {
        app.tree.worktrees.push(orion_core::Worktree {
            id: orion_core::WorktreeId(format!("w{n}")),
            project_id: ProjectId("p1".into()),
            path: PathBuf::from(format!("/wt/{n}")),
            branch: branch.into(),
            is_main,
            sort_order: n as i64,
        });
    }

    /// A session in the worktree `w{n}`.
    fn agent_in(app: &mut App, n: usize, status: orion_core::AgentStatus) {
        app.tree.agents.push(orion_core::Agent {
            id: orion_core::AgentId(format!("a{n}")),
            worktree_id: orion_core::WorktreeId(format!("w{n}")),
            name: "a".into(),
            status,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: Default::default(),
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: true,
            issue_url: None,
            recent_prompts: Vec::new(),
            usage_limit: None,
        });
    }

    /// A pull request Linear has on an issue, from `repo`.
    fn attached_pr(repo: &str, number: u64, branch: &str, state: &str) -> IssuePr {
        IssuePr {
            number,
            url: format!("https://github.com/o/{repo}/pull/{number}"),
            branch: branch.into(),
            state: Some(state.into()),
            draft: Some(false),
            conflicts: Some(false),
        }
    }

    /// The project's open pull requests, as `gh` last listed them.
    fn open_list(app: &mut App, list: Vec<crate::pull_request::OpenPr>) {
        let now = std::time::Instant::now();
        app.open_prs.insert(
            ProjectId("p1".into()),
            crate::app::OpenPrs {
                merged: Vec::new(),
                list,
                at: now,
                due: now,
                step: std::time::Duration::from_secs(60),
            },
        );
    }

    /// The row on `screen` that holds `needle`.
    fn line_with(screen: &str, needle: &str) -> String {
        screen
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} on\n{screen}"))
            .to_string()
    }

    /// Where an attached pull request stands is `App::prs`'s word once
    /// orion has heard of it — Linear's metadata may be hours behind — and
    /// one whose metadata says nothing of its state is neither open nor
    /// anything else: no word, a neutral look, ranked after every known one.
    #[test]
    fn the_work_columns_pull_request_stands_where_orion_heard_it() {
        use crate::pull_request::Standing;
        let mut stale = rich("1", "ENG-1", "Stale", ("In Review", "started"), 2);
        let shipped = attached_pr("other", 305, "ana/stale", "open");
        let shipped_url = shipped.url.clone();
        stale.prs = vec![shipped];
        let bare_pr = |number: u64| IssuePr {
            number,
            url: format!("https://github.com/o/other/pull/{number}"),
            branch: format!("ana/bare-{number}"),
            ..IssuePr::default()
        };
        let mut bare = rich("2", "ENG-2", "Bare", ("In Review", "started"), 2);
        bare.prs = vec![bare_pr(77), attached_pr("other", 12, "ana/old", "closed")];
        let mut alone = rich("3", "ENG-3", "Alone", ("In Review", "started"), 2);
        alone.prs = vec![bare_pr(78)];
        let mut app = view_on(vec![stale, bare, alone]);
        app.prs.observe(
            &shipped_url,
            crate::pr_store::PrObservation {
                state: Some(crate::pull_request::STATE_MERGED.into()),
                ..Default::default()
            },
            crate::fetch::Asked::At(crate::fetch::now()),
        );
        let project = ProjectId("p1".into());
        let list = rows(&app, &project).to_vec();
        let work = |ident: &str| {
            let issue = list.iter().find(|i| i.identifier == ident).unwrap();
            work_of(&app, &project, issue).and_then(|w| w.pr).unwrap()
        };
        let stale = work("ENG-1");
        assert_eq!(
            (stale.number, stale.standing),
            (305, Some(Standing::Merged))
        );
        assert_eq!(stale.word(), "merged", "orion's word, not Linear's open");

        let bare = work("ENG-2");
        assert_eq!(
            (bare.number, bare.standing),
            (12, Some(Standing::Closed)),
            "an unknown state ranks after every known one"
        );
        let alone = work("ENG-3");
        assert_eq!((alone.number, alone.standing), (78, None));
        assert_eq!(alone.word(), "", "no word for a state nobody said");
        let th = Theme::default();
        assert_eq!(alone.look(th).badge, th.dim, "drawn neutrally");
        assert_ne!(
            alone.look(th),
            crate::pr_row::look(Standing::Open, None, th),
            "never as open"
        );
    }

    /// An issue a worktree picked up — linked with `⌘.`, or named in its
    /// branch — wears its sessions' mark and `⎇` on its row, and the branch
    /// on the page's border; links outlive their attach.
    #[test]
    fn picked_up_issues_show_their_session_and_worktree() {
        let mut app = view_on(vec![
            rich("1", "ENG-1", "Linked by hand", ("Todo", "unstarted"), 2),
            rich(
                "2",
                "ENG-2",
                "Named by its branch",
                ("Todo", "unstarted"),
                3,
            ),
            rich("3", "ENG-3", "Nobody's", ("Todo", "unstarted"), 3),
        ]);
        worktree_on(&mut app, 1, "solar-lemur", false);
        worktree_on(&mut app, 2, "fix/eng-2-thing", false);
        agent_in(&mut app, 1, orion_core::AgentStatus::NeedsFeedback);
        let project = ProjectId("p1".into());
        app.linear_links.remember_attached(
            &project,
            "solar-lemur",
            &[("1".into(), "ENG-1".into())],
            &[],
        );
        let list = rows(&app, &project).to_vec();
        let work = |i: usize| work_of(&app, &project, &list[i]);
        let linked = work(0).expect("linked by hand, kept after its attach");
        assert_eq!(linked.branch.as_deref(), Some("solar-lemur"));
        assert_eq!(
            linked.session.map(|(s, _)| s),
            Some(orion_core::AgentStatus::NeedsFeedback)
        );
        assert_eq!(
            work(1).expect("named").branch.as_deref(),
            Some("fix/eng-2-thing")
        );
        assert_eq!(work(1).unwrap().session, None);
        assert_eq!(work(2), None);

        let screen = shot(&mut app, 220, 40);
        let row = |needle: &str| line_with(&screen, needle);
        assert!(row("ENG-1 Linked by hand").contains("● ⎇"), "{screen}");
        assert!(row("ENG-2 Named").contains('⎇'), "{screen}");
        assert!(!row("ENG-3 Nobody's").contains('⎇'), "{screen}");
        assert!(
            row("ENG-1 ─").contains("● ⎇ solar-lemur"),
            "the border\n{screen}"
        );
    }

    /// The pull request that has an issue shows wherever it was opened:
    /// orion's own beside the worktree's working agent, one Linear has
    /// from outside orion behind `⇢`, a merged one once it has left the
    /// open list, and one whose worktree is gone. The release pull request
    /// from the root checkout's branch is never an issue's.
    #[test]
    fn an_issues_pull_request_shows_wherever_it_was_opened() {
        let mut ours = rich("1", "ENG-1", "Ours", ("In Review", "started"), 2);
        ours.prs = vec![attached_pr("r", 42, "fix/eng-1-login", "open")];
        let mut outside = rich("2", "ENG-2", "Outside", ("In Review", "started"), 2);
        outside.prs = vec![IssuePr {
            conflicts: Some(true),
            ..attached_pr("other", 305, "ana/chevron-rows", "open")
        }];
        let mut landed = rich("3", "ENG-3", "Landed", ("In Review", "started"), 2);
        landed.prs = vec![
            attached_pr("r", 1354, "dev", "open"),
            attached_pr("r", 290, "eng-3-old", "closed"),
            attached_pr("r", 301, "eng-3-scopes", "merged"),
        ];
        let mut release = rich("4", "ENG-4", "Release only", ("Todo", "unstarted"), 2);
        release.prs = vec![attached_pr("r", 1354, "dev", "open")];
        let gone = rich("5", "ENG-5", "Gone", ("Todo", "unstarted"), 2);
        let mut app = view_on(vec![ours, outside, landed, release, gone]);
        worktree_on(&mut app, 0, "dev", true);
        worktree_on(&mut app, 1, "fix/eng-1-login", false);
        agent_in(&mut app, 1, orion_core::AgentStatus::Running);
        app.linear_links.remember(
            &ProjectId("p1".into()),
            "gone-branch",
            &[issue("5", "ENG-5", "Gone")],
        );
        let pr_on = |number: u64, head: &str| crate::pull_request::OpenPr {
            url: format!("https://github.com/o/r/pull/{number}"),
            head: head.into(),
            ..open_pr(number, "x")
        };
        open_list(
            &mut app,
            vec![
                pr_on(42, "fix/eng-1-login"),
                pr_on(50, "gone-branch"),
                pr_on(1354, "dev"),
            ],
        );
        use crate::pull_request::{Standing, Trouble};
        let project = ProjectId("p1".into());
        let list = rows(&app, &project).to_vec();
        let work = |ident: &str| {
            let issue = list.iter().find(|i| i.identifier == ident).unwrap();
            work_of(&app, &project, issue)
        };
        let ours = work("ENG-1").unwrap();
        assert_eq!(ours.branch.as_deref(), Some("fix/eng-1-login"));
        assert_eq!(
            ours.session.map(|(s, _)| s),
            Some(orion_core::AgentStatus::Running)
        );
        assert_eq!(
            ours.pr.map(|p| (p.number, p.standing)),
            Some((42, Some(Standing::Open)))
        );
        let outside = work("ENG-2").unwrap();
        assert_eq!(outside.branch, None);
        assert_eq!(outside.pr.and_then(|p| p.trouble), Some(Trouble::Conflicts));
        assert_eq!(
            work("ENG-3").unwrap().pr.map(|p| (p.number, p.standing)),
            Some((301, Some(Standing::Merged))),
            "merged beats closed; the release is nobody's"
        );
        assert_eq!(work("ENG-4"), None, "the release pull request alone");
        let gone = work("ENG-5").unwrap();
        assert_eq!(gone.branch, None, "the link's checkout is gone");
        assert_eq!(gone.pr.map(|p| p.number), Some(50));

        let screen = shot(&mut app, 220, 40);
        let row = |needle: &str| line_with(&screen, needle);
        assert!(row("ENG-1 Ours").contains("◐ ⎇ ↗  #42 ready"), "{screen}");
        assert!(
            row("ENG-2 Outside").contains("⇢ ↗ #305 conflicts"),
            "{screen}"
        );
        assert!(row("ENG-3 Landed").contains("⇢ ↗ #301 merged"), "{screen}");
        assert!(!row("ENG-4 Release").contains('↗'), "{screen}");
        assert!(row("ENG-5 Gone").contains("⇢ ↗  #50 ready"), "{screen}");
    }

    /// A GitHub pull request attachment reads from Linear's metadata; any
    /// other attachment is skipped.
    #[test]
    fn pull_requests_read_from_linears_attachments() {
        let issue = issue_from(&serde_json::json!({
            "id": "1", "identifier": "ENG-1", "url": "u",
            "attachments": { "nodes": [
                { "url": "https://github.com/o/r/pull/1358", "metadata": {
                    "number": 1358, "branch": "fix/eng-1", "status": "open",
                    "draft": true, "hasConflicts": false } },
                { "url": "https://www.figma.com/file/abc", "metadata": {} },
            ] },
        }))
        .unwrap();
        assert_eq!(
            issue.prs,
            [IssuePr {
                number: 1358,
                url: "https://github.com/o/r/pull/1358".into(),
                branch: "fix/eng-1".into(),
                state: Some("open".into()),
                draft: Some(true),
                conflicts: Some(false),
            }]
        );
        assert_eq!(
            issue.prs[0].standing(),
            Some(crate::pull_request::Standing::Draft)
        );
    }

    /// Plain ←/→ flip the tabs — the filter line has no caret to move.
    #[test]
    fn left_and_right_switch_tabs() {
        let mut other = rich("2", "ENG-2", "Theirs", ("Todo", "unstarted"), 3);
        other.mine = false;
        let mut app = view_on(vec![
            rich("1", "ENG-1", "Mine", ("Todo", "unstarted"), 2),
            other,
        ]);
        let mut out = Vec::new();
        handle_key(&mut app, KeyEvent::from(KeyCode::Right), &mut out);
        assert_eq!(the_view(&app).tab, LinearTab::Others);
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-2"));
        handle_key(&mut app, KeyEvent::from(KeyCode::Left), &mut out);
        assert_eq!(the_view(&app).tab, LinearTab::Mine);
        let shift_right = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        handle_key(&mut app, shift_right, &mut out);
        assert_eq!(the_view(&app).tab, LinearTab::Others);
    }

    /// Plain words find issues by their status, priority, labels and
    /// project as well as their title, a word that names none of them
    /// still fuzzy-matching the title.
    #[test]
    fn words_search_status_priority_and_labels_too() {
        let mut bug = rich("1", "ENG-1", "Login breaks", ("In Progress", "started"), 2);
        bug.labels = vec![LinearTag {
            name: "Bug".into(),
            color: String::new(),
        }];
        let app = view_on(vec![
            bug,
            rich("2", "ENG-2", "Export the report", ("Todo", "unstarted"), 4),
        ]);
        let list = rows(&app, &the_view(&app).project).to_vec();
        let ids = |query: &str| -> Vec<String> {
            filtered(query, &list)
                .into_iter()
                .map(|(i, _)| list[i].identifier.clone())
                .collect()
        };
        assert_eq!(ids("bug"), ["ENG-1"]);
        assert_eq!(ids("high"), ["ENG-1"]);
        assert_eq!(ids("in progress"), ["ENG-1"]);
        assert_eq!(ids("todo low"), ["ENG-2"]);
        assert_eq!(
            ids("high login"),
            ["ENG-1"],
            "a facet word and a title word"
        );
        assert_eq!(
            ids("pdf report"),
            ["ENG-2"],
            "the project-less label Export PDF"
        );
        assert!(ids("high report").is_empty());
    }

    #[test]
    fn short_dates_name_the_year_only_when_it_is_not_this_one() {
        let oct_2026 = 1_791_000_000; // 2026-10-03
        assert_eq!(
            short_date("2026-10-05T22:44:07Z", oct_2026).as_deref(),
            Some("Oct 5")
        );
        assert_eq!(
            short_date("2025-01-31T00:00:00Z", oct_2026).as_deref(),
            Some("Jan 31 2025")
        );
        assert_eq!(short_date("", oct_2026), None);
        assert_eq!(short_date("2026-13-01T00:00:00Z", oct_2026), None);
    }

    // ---- the TODOS MODAL's Linear ----

    /// A project with a key, its todo list one `Emails` group holding a
    /// high-priority `run plan`, and the TODOS MODAL up on it, the cursor
    /// on the item — with Linear's answers coming back on the channel.
    fn todo_app() -> (
        App,
        tempfile::TempDir,
        tokio::sync::mpsc::UnboundedReceiver<LinearAnswer>,
        u64,
    ) {
        let line = format!("LINEAR_API_KEY={FAKE_KEY}\n");
        let (mut app, dir) = app_on(&[(".env", &line)]);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        app.linear_tx = Some(tx);
        let mut file = crate::todos::TodoFile::new(dir.path());
        let group = file.add_group(None, "Emails");
        let item = file.add_item(group, "run plan", crate::todos::today());
        file.item_mut(item).unwrap().priority = 2;
        app.todos.insert(dir.path().into(), file);
        app.overlay = Some(Overlay::Todos(crate::todos::TodoView::new(
            ProjectId("p1".into()),
            "demo".into(),
            dir.path().into(),
        )));
        press(&mut app, plain(KeyCode::Down));
        (app, dir, rx, item)
    }

    fn todo_item(app: &App, dir: &tempfile::TempDir, item: u64) -> crate::todos::Item {
        app.todos[dir.path()].item(item).unwrap().clone()
    }

    fn todo_view(app: &App) -> &crate::todos::TodoView {
        match &app.overlay {
            Some(Overlay::Todos(view)) => view,
            _ => panic!("not the todos modal"),
        }
    }

    fn one_thread() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// Land every answer Linear sends until `done` says the flow is over.
    fn run_linear(
        app: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<LinearAnswer>,
        act: impl FnOnce(&mut App),
    ) {
        one_thread().block_on(async {
            act(app);
            let answer = rx.recv().await.expect("an answer");
            land_answer(app, answer);
        });
    }

    fn cmd(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SUPER)
    }

    /// The issue Linear makes, as `issueCreate` answers.
    fn created_issue() -> serde_json::Value {
        serde_json::json!({"data": {"issueCreate": {"success": true, "issue": {
            "identifier": "RIP-431", "url": "https://linear.app/x/issue/RIP-431",
            "state": {"name": "Triage", "type": "triage", "color": "#fc7840"}, "priority": 2
        }}}})
    }

    /// `⌘L` → **Create in Triage**: the one team's triage state set on
    /// the issue, the item's priority and where it came from with it, and
    /// the item linked to what Linear made — the team remembered.
    #[test]
    fn create_in_triage_files_the_todo_and_links_it() {
        let (mut app, dir, mut rx, item) = todo_app();
        press(&mut app, cmd('l'));
        let pick = todo_view(&app).pick.clone().expect("the menu");
        assert_eq!(
            pick.kind,
            crate::todos::view::PickKind::Menu(vec![
                crate::todos::view::MenuAction::Create,
                crate::todos::view::MenuAction::Link,
            ])
        );
        crate::hints::assert_hints_from(
            &crate::todos::view::hints(todo_view(&app)),
            crate::todos::view::keys::ALL,
        );
        let sent = with_graphql_stub(
            |key, query| {
                assert_eq!(key, FAKE_KEY);
                if query.contains("teams") {
                    return Ok(serde_json::json!({"data": {"teams": {"nodes": [
                        {"id": "t1", "key": "RIP", "name": "Riplo", "triageEnabled": true,
                         "states": {"nodes": [{"id": "s-todo", "type": "unstarted"}, {"id": "s-triage", "type": "triage"}]}}
                    ]}}}));
                }
                assert!(query.contains("issueCreate"), "{query}");
                Ok(created_issue())
            },
            || {
                run_linear(&mut app, &mut rx, |app| {
                    press(app, plain(KeyCode::Enter));
                });
                graphql_sent()
            },
        );
        let input = &sent[1]["input"];
        assert_eq!(input["teamId"], "t1");
        assert_eq!(input["stateId"], "s-triage");
        assert_eq!(input["priority"], 2);
        assert_eq!(input["title"], "run plan");
        assert_eq!(input["description"], "From orion todos · Emails");
        assert_eq!(
            todo_item(&app, &dir, item).linear.as_deref(),
            Some("RIP-431")
        );
        assert_eq!(app.todos[dir.path()].linear_team.as_deref(), Some("t1"));
        assert_eq!(app.flash.as_deref(), Some("Created RIP-431 in Triage"));
        assert!(todo_view(&app).linked.contains_key("RIP-431"));
    }

    /// A team that takes no issues into Triage gets the issue in its
    /// default state, no `stateId` sent, and the footer says so.
    #[test]
    fn a_team_without_triage_gets_no_state_id() {
        let (mut app, dir, mut rx, item) = todo_app();
        let sent = with_graphql_stub(
            |_, query| {
                if query.contains("teams") {
                    return Ok(serde_json::json!({"data": {"teams": {"nodes": [
                        {"id": "t1", "key": "RIP", "name": "Riplo", "triageEnabled": false,
                         "states": {"nodes": [{"id": "s-todo", "type": "unstarted"}]}}
                    ]}}}));
                }
                Ok(created_issue())
            },
            || {
                run_linear(&mut app, &mut rx, |app| {
                    press(app, cmd('l'));
                    press(app, plain(KeyCode::Enter));
                });
                graphql_sent()
            },
        );
        assert!(sent[1]["input"].get("stateId").is_none(), "{sent:?}");
        assert_eq!(
            todo_item(&app, &dir, item).linear.as_deref(),
            Some("RIP-431")
        );
        assert_eq!(
            app.flash.as_deref(),
            Some("Linear: Riplo has no Triage — created RIP-431 in its default state")
        );
    }

    /// Several teams and none remembered: they come back to pick from, and
    /// the one picked is where the issue goes — and is remembered.
    #[test]
    fn several_teams_are_picked_from() {
        let (mut app, dir, mut rx, item) = todo_app();
        with_graphql_stub(
            |_, query| {
                if query.contains("teams") {
                    return Ok(serde_json::json!({"data": {"teams": {"nodes": [
                        {"id": "t1", "key": "ENG", "name": "Eng", "triageEnabled": false, "states": {"nodes": []}},
                        {"id": "t2", "key": "RIP", "name": "Riplo", "triageEnabled": true,
                         "states": {"nodes": [{"id": "s-triage", "type": "triage"}]}}
                    ]}}}));
                }
                Ok(created_issue())
            },
            || {
                run_linear(&mut app, &mut rx, |app| {
                    press(app, cmd('l'));
                    press(app, plain(KeyCode::Enter));
                });
                let pick = todo_view(&app).pick.clone().expect("the team pick");
                assert!(
                    matches!(pick.kind, crate::todos::view::PickKind::Team(ref t) if t.len() == 2)
                );
                run_linear(&mut app, &mut rx, |app| {
                    press(app, plain(KeyCode::Down));
                    press(app, plain(KeyCode::Enter));
                });
                let sent = graphql_sent();
                assert_eq!(sent.last().unwrap()["input"]["teamId"], "t2");
            },
        );
        assert_eq!(app.todos[dir.path()].linear_team.as_deref(), Some("t2"));
        assert_eq!(
            todo_item(&app, &dir, item).linear.as_deref(),
            Some("RIP-431")
        );
    }

    /// The linked issues are asked about in one aliased query; one done in
    /// Linear ticks its todo — and only that way: the chip says the rest.
    #[test]
    fn a_linked_issue_done_in_linear_ticks_its_todo() {
        let (mut app, dir, mut rx, item) = todo_app();
        app.todos
            .get_mut(dir.path())
            .unwrap()
            .item_mut(item)
            .unwrap()
            .linear = Some("RIP-412".into());
        // Seen in progress when it was linked: the way into done ticks.
        app.todos
            .get_mut(dir.path())
            .unwrap()
            .item_mut(item)
            .unwrap()
            .linear_seen = Some("started".into());
        with_graphql_stub(
            |_, query| {
                assert!(query.contains(r#"i0: issue(id: "RIP-412")"#), "{query}");
                Ok(serde_json::json!({"data": {"i0": {
                    "identifier": "RIP-412", "url": "https://linear.app/x/issue/RIP-412",
                    "state": {"name": "Done", "type": "completed", "color": "#5e6ad2"}, "priority": 2
                }}}))
            },
            || {
                run_linear(&mut app, &mut rx, |app| {
                    press(app, cmd('r'));
                });
            },
        );
        assert!(todo_item(&app, &dir, item).done.is_some(), "ticked");
        assert_eq!(
            app.flash.as_deref(),
            Some("ticked RIP-412 — done in Linear")
        );
        assert_eq!(todo_view(&app).linked["RIP-412"].state, "Done");
        // Unticked by hand, it stays open: the issue was done already.
        app.todos
            .get_mut(dir.path())
            .unwrap()
            .item_mut(item)
            .unwrap()
            .done = None;
        app.flash = None;
        with_graphql_stub(
            |_, _| {
                Ok(serde_json::json!({"data": {"i0": {
                    "identifier": "RIP-412", "url": "u",
                    "state": {"name": "Done", "type": "completed", "color": ""}, "priority": 2
                }}}))
            },
            || run_linear(&mut app, &mut rx, |app| press(app, cmd('r'))),
        );
        assert!(
            todo_item(&app, &dir, item).done.is_none(),
            "the untick sticks"
        );
        assert!(app.flash.is_none());
    }

    /// One linked issue Linear cannot find nulls the whole aliased answer;
    /// each is then asked alone, and the one that is there still lands.
    #[test]
    fn a_missing_linked_issue_does_not_hide_the_rest() {
        let (mut app, dir, mut rx, item) = todo_app();
        let file = app.todos.get_mut(dir.path()).unwrap();
        file.item_mut(item).unwrap().linear = Some("RIP-1".into());
        let g = file.groups[0].id;
        let gone = file.add_item(g, "gone", crate::todos::today());
        file.item_mut(gone).unwrap().linear = Some("RIP-2".into());
        with_graphql_stub(
            |_, query| {
                if query.contains("RIP-2") {
                    return Ok(serde_json::json!({"data": null,
                        "errors": [{"message": "Entity not found: Issue"}]}));
                }
                Ok(serde_json::json!({"data": {"i0": {
                    "identifier": "RIP-1", "url": "u",
                    "state": {"name": "Todo", "type": "unstarted", "color": ""}, "priority": 0
                }}}))
            },
            || run_linear(&mut app, &mut rx, |app| press(app, cmd('r'))),
        );
        let linked = &todo_view(&app).linked;
        assert!(linked.contains_key("RIP-1"), "{linked:?}");
        assert!(!linked.contains_key("RIP-2"));
        assert!(app.flash.is_none(), "{:?}", app.flash);
    }

    /// While an item's issue is being made, its menu offers no second
    /// Create, and nothing more is sent.
    #[test]
    fn create_in_triage_is_not_sent_twice() {
        let (mut app, dir, mut rx, item) = todo_app();
        with_graphql_stub(
            |_, query| {
                if query.contains("teams") {
                    return Ok(serde_json::json!({"data": {"teams": {"nodes": [
                        {"id": "t1", "key": "RIP", "name": "Riplo", "triageEnabled": false,
                         "states": {"nodes": []}}
                    ]}}}));
                }
                Ok(created_issue())
            },
            || {
                run_linear(&mut app, &mut rx, |app| {
                    press(app, cmd('l'));
                    press(app, plain(KeyCode::Enter));
                    assert!(app.todo_creates.contains(&(dir.path().into(), item)));
                    press(app, cmd('l'));
                    assert_eq!(
                        todo_view(app).pick.as_ref().map(|p| p.kind.clone()),
                        Some(crate::todos::view::PickKind::Menu(vec![
                            crate::todos::view::MenuAction::Link
                        ]))
                    );
                    press(app, plain(KeyCode::Esc));
                });
            },
        );
        assert!(app.todo_creates.is_empty(), "answered");
        assert!(rx.try_recv().is_err(), "one create only");
    }

    /// **Link existing…** opens this list to pick from — `Enter link to
    /// todo` — and Enter goes back to the TODOS MODAL with the item linked
    /// to the issue under the cursor; Esc goes back with nothing linked.
    #[test]
    fn link_existing_round_trips_through_the_linear_view() {
        let (mut app, dir, _rx, item) = todo_app();
        // Nothing asked of Linear here: the list is already in.
        app.linear_tx = None;
        app.linear.insert(
            ProjectId("p1".into()),
            LinearList {
                list: vec![issue("1", "ENG-1", "Login")],
                ..Default::default()
            },
        );
        let to_linear = |app: &mut App| {
            press(app, cmd('l'));
            press(app, plain(KeyCode::Down));
            press(app, plain(KeyCode::Enter));
            let Some(Overlay::Linear(view)) = &app.overlay else {
                panic!("the Linear view");
            };
            assert!(matches!(view.mode, LinearMode::Link { .. }));
            assert!(hints(view).iter().any(|h| h.does == "link to todo"));
        };
        to_linear(&mut app);
        press(&mut app, plain(KeyCode::Esc));
        assert!(todo_view(&app).pick.is_none());
        assert_eq!(todo_item(&app, &dir, item).linear, None);
        to_linear(&mut app);
        press(&mut app, plain(KeyCode::Enter));
        assert_eq!(todo_item(&app, &dir, item).linear.as_deref(), Some("ENG-1"));
        assert!(todo_view(&app).linked.contains_key("ENG-1"));
        // Linked, the menu offers the browser and the way back out.
        press(&mut app, cmd('l'));
        assert_eq!(
            todo_view(&app).pick.as_ref().map(|p| p.kind.clone()),
            Some(crate::todos::view::PickKind::Menu(vec![
                crate::todos::view::MenuAction::Browser,
                crate::todos::view::MenuAction::Unlink,
            ]))
        );
        press(&mut app, plain(KeyCode::Down));
        press(&mut app, plain(KeyCode::Enter));
        assert_eq!(todo_item(&app, &dir, item).linear, None, "unlinked");
    }

    // ---- newest asked wins: ⌘S, the cursor, attaches ----

    /// Team `t1`'s workflow: Todo, In Progress, Done.
    fn workflow() -> HashMap<String, Vec<LinearState>> {
        let state = |id: &str, name: &str, kind: &str| LinearState {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
            color: String::new(),
        };
        HashMap::from([(
            "t1".to_string(),
            vec![
                state("s1", "Todo", "unstarted"),
                state("s2", "In Progress", "started"),
                state("s3", "Done", "completed"),
            ],
        )])
    }

    /// `issues` as a list answer, with [`workflow`]'s states.
    fn listed(issues: Vec<LinearIssue>) -> Result<LinearList, String> {
        Ok(LinearList {
            list: issues,
            states: workflow(),
            ..Default::default()
        })
    }

    /// `ident` in `status` (one of [`workflow`]'s).
    fn in_state(id: &str, ident: &str, status: &str) -> LinearIssue {
        let kind = match status {
            "Todo" => "unstarted",
            "Done" => "completed",
            _ => "started",
        };
        LinearIssue {
            status: status.into(),
            status_type: kind.into(),
            ..issue(id, ident, "x")
        }
    }

    /// `demo` with a key, ENG-1 and ENG-2 in progress, and the LINEAR
    /// VIEW up on it — no list asked yet — with Linear's answers coming
    /// back on the channel.
    fn moving() -> (
        App,
        tempfile::TempDir,
        tokio::sync::mpsc::UnboundedReceiver<LinearAnswer>,
    ) {
        let line = format!("LINEAR_API_KEY={FAKE_KEY}\n");
        let (mut app, dir) = app_on(&[(".env", &line)]);
        let project = ProjectId("p1".into());
        app.linear.insert(
            project.clone(),
            listed(vec![
                in_state("1", "ENG-1", "In Progress"),
                in_state("2", "ENG-2", "In Progress"),
            ])
            .unwrap(),
        );
        app.overlay = Some(Overlay::Linear(LinearView::new(
            project,
            "demo".into(),
            dir.path().into(),
            LinearMode::Browse,
        )));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        app.linear_tx = Some(tx);
        (app, dir, rx)
    }

    /// `⌘S` on the row at `index`, Enter on `state`.
    fn move_to(app: &mut App, index: usize, state: &str) {
        if let Some(Overlay::Linear(view)) = &mut app.overlay {
            view.selected = index;
        }
        let mut out = Vec::new();
        handle_key(
            app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut out,
        );
        if let Some(Overlay::Linear(view)) = &mut app.overlay {
            let pick = view.prop_pick.as_mut().expect("the status picker");
            pick.selected = pick
                .rows
                .iter()
                .position(|row| matches!(row, Change::Status(s) if s.name == state))
                .unwrap();
        }
        handle_key(app, KeyEvent::from(KeyCode::Enter), &mut out);
    }

    fn status_of(app: &App, id: &str) -> String {
        app.linear[&ProjectId("p1".into())]
            .list
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.status.clone())
            .unwrap_or_default()
    }

    /// A list for `demo`, asked at the ticket's time, landing.
    fn land_list(
        app: &mut App,
        ticket: crate::fetch::Ticket<ProjectId>,
        dir: &tempfile::TempDir,
        issues: Vec<LinearIssue>,
    ) {
        land_answer(
            app,
            LinearAnswer::List {
                ticket,
                dir: dir.path().into(),
                list: listed(issues),
            },
        );
    }

    fn ask_list(app: &mut App) -> crate::fetch::Ticket<ProjectId> {
        app.linear_flights
            .begin(ProjectId("p1".into()), crate::fetch::now())
            .expect("no list out")
    }

    /// A list asked before a `⌘S` — or after it, but before Linear took
    /// it — never puts the old state back; the first list asked after
    /// Linear took it is the truth again, whatever it says.
    #[test]
    fn a_ctrl_s_outlasts_lists_asked_before_linear_took_it() {
        let (mut app, dir, mut rx) = moving();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        with_graphql_stub(
            |_, _| Ok(serde_json::json!({"data": {"issueUpdate": {"success": true}}})),
            || {
                rt.block_on(async {
                    let before = ask_list(&mut app);
                    move_to(&mut app, 1, "Done");
                    assert_eq!(status_of(&app, "2"), "Done", "at once");
                    let stale = vec![
                        in_state("1", "ENG-1", "In Progress"),
                        in_state("2", "ENG-2", "In Progress"),
                    ];
                    land_list(&mut app, before, &dir, stale.clone());
                    assert_eq!(status_of(&app, "2"), "Done", "a list asked before");
                    let meanwhile = ask_list(&mut app);
                    let answer = rx.recv().await.expect("Linear's answer");
                    land_answer(&mut app, answer);
                    land_list(&mut app, meanwhile, &dir, stale);
                    assert_eq!(
                        status_of(&app, "2"),
                        "Done",
                        "a list asked before Linear took it"
                    );
                    let after = ask_list(&mut app);
                    land_list(
                        &mut app,
                        after,
                        &dir,
                        vec![
                            in_state("1", "ENG-1", "In Progress"),
                            in_state("2", "ENG-2", "Todo"),
                        ],
                    );
                });
            },
        );
        assert_eq!(status_of(&app, "2"), "Todo", "moved on in Linear since");
        assert!(
            app.linear_edits.moves.is_empty(),
            "nothing left to lay over"
        );
    }

    /// Linear's first answer to `issueUpdate` refuses, every later one
    /// takes it; a list is the one issue in progress.
    fn refuse_the_first_move(_: &str, query: &str) -> Result<serde_json::Value, String> {
        static MOVES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if query.contains("issueUpdate") {
            if MOVES.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Ok(serde_json::json!({"errors": [{"message": "not allowed"}]}));
            }
            return Ok(serde_json::json!({"data": {"issueUpdate": {"success": true}}}));
        }
        Ok(serde_json::json!({"data": {"mine": {"nodes": [
            {"id": "1", "identifier": "ENG-1", "url": "https://linear.app/x/issue/ENG-1",
             "state": {"name": "In Progress", "type": "started"}}
        ]}, "others": {"nodes": []}}}))
    }

    /// The states each `issueUpdate` sent named, in order.
    fn moves_sent() -> Vec<String> {
        graphql_sent()
            .iter()
            .filter_map(|v| v.get("stateId")?.as_str().map(str::to_string))
            .collect()
    }

    /// Two quick `⌘S` on one issue go out one after the other, the second
    /// once the first has answered. The first refused leaves the second
    /// on the row — it is newer — and asks for a fresh list, which the
    /// second outlasts; Linear ends where the row does.
    #[test]
    fn two_quick_ctrl_s_go_out_in_order_past_a_refused_first() {
        let (mut app, _dir, mut rx) = moving();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let landed = with_graphql_stub(refuse_the_first_move, || {
            rt.block_on(async {
                move_to(&mut app, 0, "Done");
                move_to(&mut app, 0, "Todo");
                assert_eq!(status_of(&app, "1"), "Todo");
                let first = rx.recv().await.expect("the first's answer");
                assert!(matches!(first, LinearAnswer::Edited { seq: 1, .. }));
                assert_eq!(moves_sent(), ["s3"], "the second waits its turn");
                land_answer(&mut app, first);
                assert_eq!(status_of(&app, "1"), "Todo", "the newer one stands");
                assert_eq!(
                    app.flash.as_deref(),
                    Some("couldn't move ENG-1: not allowed")
                );
                let mut landed = Vec::new();
                for _ in 0..2 {
                    let answer = rx.recv().await.expect("an answer");
                    landed.push(matches!(answer, LinearAnswer::List { .. }));
                    land_answer(&mut app, answer);
                }
                landed.sort();
                (landed, moves_sent())
            })
        });
        assert_eq!(landed.0, [false, true], "a fresh list and the second move");
        assert_eq!(landed.1, ["s3", "s1"], "in the order they were asked");
        assert_eq!(status_of(&app, "1"), "Todo");
    }

    /// A refused `⌘S` with none after it puts back the state Linear has,
    /// and asks for a fresh list to be sure.
    #[test]
    fn a_lone_refused_ctrl_s_puts_back_what_linear_has() {
        let (mut app, _dir, mut rx) = moving();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        with_graphql_stub(
            |_, _| Ok(serde_json::json!({"errors": [{"message": "not allowed"}]})),
            || {
                rt.block_on(async {
                    move_to(&mut app, 0, "Done");
                    assert_eq!(status_of(&app, "1"), "Done");
                    let answer = rx.recv().await.expect("Linear's answer");
                    land_answer(&mut app, answer);
                });
            },
        );
        assert_eq!(status_of(&app, "1"), "In Progress");
        assert!(app.linear_edits.moves.is_empty());
        assert!(app.linear_flights.in_flight(&ProjectId("p1".into())));
    }

    // ---- ⌘P and ⌘I: the same edit, another property ----

    fn user(id: &str, name: &str) -> LinearUser {
        LinearUser {
            id: id.into(),
            name: name.into(),
        }
    }

    /// [`moving`], with Ana (`u1`) the configured user and Bo and Cy her
    /// teammates: ENG-1 is hers, on `My issues`, ENG-2 Bo's, on `Other
    /// issues`.
    fn staffed() -> (
        App,
        tempfile::TempDir,
        tokio::sync::mpsc::UnboundedReceiver<LinearAnswer>,
    ) {
        let (mut app, dir, rx) = moving();
        app.linear
            .insert(ProjectId("p1".into()), staff(team_issues()));
        (app, dir, rx)
    }

    /// ENG-1, Ana's, and ENG-2, Bo's, as Linear lists them.
    fn team_issues() -> Vec<LinearIssue> {
        let of = |id: &str, ident: &str, who: &LinearUser| LinearIssue {
            assignee: who.name.clone(),
            assignee_id: who.id.clone(),
            mine: who.id == "u1",
            ..in_state(id, ident, "In Progress")
        };
        vec![
            of("1", "ENG-1", &user("u1", "Ana")),
            of("2", "ENG-2", &user("u2", "Bo")),
        ]
    }

    /// `issues` as a list answer that knows who Ana is and who is in `t1`.
    fn staff(issues: Vec<LinearIssue>) -> LinearList {
        LinearList {
            me: Some(user("u1", "Ana")),
            members: HashMap::from([(
                "t1".to_string(),
                vec![user("u1", "Ana"), user("u2", "Bo"), user("u3", "Cy")],
            )]),
            ..listed(issues).unwrap()
        }
    }

    /// [`staffed`]'s own list landing again, asked at the ticket's time.
    fn land_staffed(
        app: &mut App,
        ticket: crate::fetch::Ticket<ProjectId>,
        dir: &tempfile::TempDir,
    ) {
        land_answer(
            app,
            LinearAnswer::List {
                ticket,
                dir: dir.path().into(),
                list: Ok(staff(team_issues())),
            },
        );
    }

    /// The cursor on the row at `index`, on the tab that shows it.
    fn cursor_on(app: &mut App, index: usize) {
        let mine = app.linear[&ProjectId("p1".into())].list[index].mine;
        if let Some(Overlay::Linear(view)) = &mut app.overlay {
            view.selected = index;
            view.tab = if mine {
                LinearTab::Mine
            } else {
                LinearTab::Others
            };
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn row_of(app: &App, id: &str) -> LinearIssue {
        app.linear[&ProjectId("p1".into())]
            .list
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .expect("the issue")
    }

    fn the_pick(app: &App) -> &PropPick {
        the_view(app).prop_pick.as_ref().expect("the picker")
    }

    /// The variables of each `issueUpdate` sent, in order.
    fn edits_sent() -> Vec<serde_json::Value> {
        graphql_sent()
            .into_iter()
            .filter(|v| v.get("id").is_some())
            .collect()
    }

    fn took_it(_: &str, _: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": {"issueUpdate": {"success": true}}}))
    }

    fn refused_it(_: &str, _: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"errors": [{"message": "not allowed"}]}))
    }

    /// `⌘P` lists Linear's priorities on the issue's own; a digit sets
    /// one at once — the row saying so before Linear answers, and a list
    /// asked before Linear took it never putting the old one back.
    #[test]
    fn cmd_p_sets_a_priority_by_its_digit() {
        let (mut app, dir, mut rx) = staffed();
        with_graphql_stub(took_it, || {
            one_thread().block_on(async {
                let before = ask_list(&mut app);
                cursor_on(&mut app, 0);
                press(&mut app, cmd('p'));
                let pick = the_pick(&app);
                assert_eq!(pick.prop, Prop::Priority);
                assert_eq!(pick.rows[pick.selected], Change::Priority(0));
                assert_eq!(pick.current, Some(pick.selected));
                // The digits are spelled as a range, not from the table.
                let (digits, shown): (Vec<_>, Vec<_>) = hints(the_view(&app))
                    .into_iter()
                    .partition(|h| h.does == keys::LEVEL.does);
                crate::hints::assert_hints_from(&shown, keys::ALL);
                assert_eq!(digits.len(), 1, "{digits:?}");
                let screen = shot(&mut app, 160, 30);
                assert!(screen.contains("Set ENG-1's priority to…"), "{screen}");
                assert!(
                    line_with(&screen, "No priority").contains("current"),
                    "{screen}"
                );

                press(&mut app, plain(KeyCode::Char('2')));
                assert!(the_view(&app).prop_pick.is_none());
                assert_eq!(row_of(&app, "1").priority, 2, "at once");
                land_staffed(&mut app, before, &dir);
                assert_eq!(row_of(&app, "1").priority, 2, "a list asked before");
                let answer = rx.recv().await.expect("Linear's answer");
                land_answer(&mut app, answer);
            });
            assert_eq!(
                edits_sent(),
                [serde_json::json!({"id": "1", "priority": 2})]
            );
        });
        assert_eq!(row_of(&app, "1").priority, 2);
        assert_eq!(app.flash.as_deref(), None);
    }

    /// A refused `⌘P` puts the priority Linear has back and says why.
    #[test]
    fn a_refused_cmd_p_puts_the_priority_back() {
        let (mut app, _dir, mut rx) = staffed();
        with_graphql_stub(refused_it, || {
            run_linear(&mut app, &mut rx, |app| {
                cursor_on(app, 0);
                press(app, ctrl('p'));
                press(app, plain(KeyCode::Up));
                press(app, plain(KeyCode::Enter));
                assert_eq!(row_of(app, "1").priority, 4, "the row above none: low");
            });
        });
        assert_eq!(row_of(&app, "1").priority, 0);
        assert_eq!(
            app.flash.as_deref(),
            Some("couldn't set ENG-1's priority: not allowed")
        );
        assert!(app.linear_edits.moves.is_empty());
    }

    /// `⌘I` opens on `Me`, whoever has the issue: Enter makes it yours,
    /// and it is on `My issues` at once. `No assignee` takes it off you
    /// again, to `Other issues`.
    #[test]
    fn cmd_i_assigns_the_issue_to_me_or_to_nobody() {
        let (mut app, dir, mut rx) = staffed();
        with_graphql_stub(took_it, || {
            one_thread().block_on(async {
                cursor_on(&mut app, 1);
                press(&mut app, cmd('i'));
                let pick = the_pick(&app);
                assert_eq!(pick.prop, Prop::Assignee);
                let rows: Vec<&str> = pick
                    .rows
                    .iter()
                    .map(|row| match row {
                        Change::Assignee(Some(user)) => user.name.as_str(),
                        _ => "nobody",
                    })
                    .collect();
                assert_eq!(rows, ["Ana", "nobody", "Bo", "Cy"], "you once, on top");
                assert_eq!((pick.selected, pick.current), (0, Some(2)));
                crate::hints::assert_hints_from(&hints(the_view(&app)), keys::ALL);
                let screen = shot(&mut app, 160, 30);
                assert!(screen.contains("Assign ENG-2 to…"), "{screen}");
                assert!(line_with(&screen, "Me ").contains("Ana"), "{screen}");
                assert!(screen.contains("No assignee"), "{screen}");
                assert!(line_with(&screen, "Bo").contains("current"), "{screen}");

                let before = ask_list(&mut app);
                press(&mut app, plain(KeyCode::Enter));
                let mine = row_of(&app, "2");
                assert_eq!((mine.assignee.as_str(), mine.mine), ("Ana", true));
                land_staffed(&mut app, before, &dir);
                let mine = row_of(&app, "2");
                assert_eq!(
                    (mine.assignee.as_str(), mine.mine),
                    ("Ana", true),
                    "a list asked before"
                );
                let answer = rx.recv().await.expect("Linear's answer");
                land_answer(&mut app, answer);

                // The twin where no ⌘ arrives, on what is now yours.
                cursor_on(&mut app, 1);
                press(&mut app, ctrl('g'));
                assert_eq!(the_pick(&app).current, Some(0));
                press(&mut app, plain(KeyCode::Down));
                press(&mut app, plain(KeyCode::Enter));
                let nobodys = row_of(&app, "2");
                assert_eq!(
                    (nobodys.assignee.as_str(), nobodys.assignee_id.as_str()),
                    ("", "")
                );
                assert!(!nobodys.mine);
                let answer = rx.recv().await.expect("Linear's answer");
                land_answer(&mut app, answer);
            });
            assert_eq!(
                edits_sent(),
                [
                    serde_json::json!({"id": "2", "assigneeId": "u1"}),
                    serde_json::json!({"id": "2", "assigneeId": null}),
                ]
            );
        });
        assert_eq!(app.flash.as_deref(), None);
    }

    /// A refused `⌘I` gives the issue back to whoever Linear has it with,
    /// on the tab it came from.
    #[test]
    fn a_refused_cmd_i_puts_the_assignee_and_the_tab_back() {
        let (mut app, _dir, mut rx) = staffed();
        with_graphql_stub(refused_it, || {
            run_linear(&mut app, &mut rx, |app| {
                cursor_on(app, 1);
                press(app, cmd('i'));
                press(app, plain(KeyCode::Enter));
                assert!(row_of(app, "2").mine);
            });
        });
        let back = row_of(&app, "2");
        assert_eq!(
            (back.assignee.as_str(), back.assignee_id.as_str(), back.mine),
            ("Bo", "u2", false)
        );
        assert_eq!(
            app.flash.as_deref(),
            Some("couldn't assign ENG-2: not allowed")
        );
    }

    /// An issue Linear named nobody for — no configured user, no team
    /// members — opens no picker.
    #[test]
    fn cmd_i_with_nobody_to_assign_to_says_so() {
        let (mut app, _dir, _rx) = moving();
        press(&mut app, cmd('i'));
        assert!(the_view(&app).prop_pick.is_none());
        let said = app.flash.as_deref().unwrap_or_default().to_string();
        assert!(
            said.starts_with("Linear didn't say who ENG-1 can go to"),
            "{said}"
        );
    }

    /// Each property of an issue queues on its own: a `⌘P` goes out while
    /// the `⌘S` before it is still at Linear, and both land.
    #[test]
    fn a_status_and_a_priority_edit_go_out_side_by_side() {
        let (mut app, _dir, mut rx) = staffed();
        with_graphql_stub(took_it, || {
            one_thread().block_on(async {
                move_to(&mut app, 0, "Done");
                press(&mut app, cmd('p'));
                press(&mut app, plain(KeyCode::Char('1')));
                assert_eq!(app.linear_edits.moves.len(), 2);
                assert!(
                    app.linear_edits.moves.values().all(|m| m.sending.is_some()),
                    "neither waits on the other"
                );
                for _ in 0..2 {
                    let answer = rx.recv().await.expect("Linear's answer");
                    land_answer(&mut app, answer);
                }
            });
            let mut sent = edits_sent();
            sent.sort_by_key(|v| v.get("priority").is_some());
            assert_eq!(
                sent,
                [
                    serde_json::json!({"id": "1", "stateId": "s3"}),
                    serde_json::json!({"id": "1", "priority": 1}),
                ]
            );
        });
        let row = row_of(&app, "1");
        assert_eq!((row.status.as_str(), row.priority), ("Done", 1));
        assert!(app.linear_edits.moves.values().all(|m| m.landed.is_some()));
        assert_eq!(app.flash.as_deref(), None);
    }

    /// A list that lands puts the cursor back on the issue it was on,
    /// wherever that now sits; with the issue gone, it is clamped.
    #[test]
    fn the_cursor_follows_its_issue_across_a_refresh() {
        let ident = |n: &str| in_state(n, &format!("ENG-{n}"), "In Progress");
        let mut app = view_on(vec![ident("1"), ident("2"), ident("3")]);
        let dir = tempfile::tempdir().unwrap();
        if let Some(Overlay::Linear(view)) = &mut app.overlay {
            view.selected = 1;
        }
        let ticket = ask_list(&mut app);
        land_list(
            &mut app,
            ticket,
            &dir,
            vec![ident("2"), ident("3"), ident("1")],
        );
        assert_eq!(the_view(&app).selected, 0);
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-2"));
        if let Some(Overlay::Linear(view)) = &mut app.overlay {
            view.selected = 2;
        }
        let ticket = ask_list(&mut app);
        land_list(&mut app, ticket, &dir, vec![ident("3")]);
        assert_eq!(the_view(&app).selected, 0, "ENG-1 is gone");
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-3"));
    }

    /// The pull request an attach linked is on the issue's row the moment
    /// Linear takes it — and stays there over a list asked before, which
    /// can't know it, until a list asked after says what Linear has.
    #[test]
    fn an_attach_shows_at_once_and_outlasts_an_older_list() {
        let (mut app, dir, mut rx) = paired();
        let project = ProjectId("p1".into());
        app.linear_flights.cancel(&project);
        let before = ask_list(&mut app);
        let issues = vec![issue("1", "ENG-1", "Login")];
        let sent = attach_through(&mut app, &mut rx, |app| {
            let url = "https://github.com/o/r/pull/41".to_string();
            attach_issues(app, dir.path().into(), url, 41, &issues);
        });
        assert_eq!(sent, [attached("1", 41)]);
        let prs = |app: &App| -> Vec<(u64, String)> {
            app.linear[&project].list[0]
                .prs
                .iter()
                .map(|p| (p.number, p.branch.clone()))
                .collect()
        };
        assert_eq!(prs(&app), [(41, "branch-41".to_string())], "at once");
        assert_eq!(
            app.linear_links.branches_of("1"),
            ["branch-41"],
            "remembered against its branch"
        );
        land_list(&mut app, before, &dir, vec![issue("1", "ENG-1", "Login")]);
        assert_eq!(
            prs(&app),
            [(41, "branch-41".to_string())],
            "a list asked before"
        );
        let after = ask_list(&mut app);
        land_list(&mut app, after, &dir, vec![issue("1", "ENG-1", "Login")]);
        assert!(prs(&app).is_empty(), "a list asked after is Linear's word");
    }

    // ---- the LinkStore ----

    /// `linear-links.json` from before links were kept per project — or
    /// per issue — still loads: its links wait on any project's branch of
    /// that name, `attached: true` reading as every issue attached. New
    /// links on one branch name in two projects stay apart, on disk too.
    #[test]
    fn older_links_load_and_two_projects_keep_one_branch_name_apart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("linear-links.json");
        std::fs::write(
            &path,
            r#"{
              "done": {"issue_ids": ["1"], "identifiers": ["ENG-1"], "attached": true},
              "wait": {"issue_ids": ["2"], "identifiers": ["ENG-2"]}
            }"#,
        )
        .unwrap();
        let (p1, p2) = (ProjectId("p1".into()), ProjectId("p2".into()));
        let mut store = LinkStore::load(path.clone());
        assert!(store.pending(&p1, "done").is_empty(), "attached already");
        assert_eq!(store.branches_of("1"), ["done"]);
        assert_eq!(store.pending(&p1, "wait"), ["ENG-2"]);
        assert_eq!(store.pending(&p2, "wait"), ["ENG-2"], "any project's");
        store.remember(&p1, "feature-x", &[issue("3", "ENG-3", "c")]);
        store.remember(&p2, "feature-x", &[issue("4", "ENG-4", "d")]);
        let store = LinkStore::load(path);
        assert_eq!(store.pending(&p1, "feature-x"), ["ENG-3"]);
        assert_eq!(store.pending(&p2, "feature-x"), ["ENG-4"]);
    }

    /// A link outlives its branch by ninety days and no more: one whose
    /// branch a checkout or an open pull request is still on stays however
    /// old, another project's is not this prune's, and a link from before
    /// they were dated starts its ninety days when it is first loaded.
    #[test]
    fn a_link_is_forgotten_ninety_days_after_its_branch_is_gone() {
        const DAY: u64 = 24 * 60 * 60;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("linear-links.json");
        std::fs::write(
            &path,
            r#"{"old:p1": {"issue_ids": ["9"], "identifiers": ["ENG-9"],
                           "project": "p1", "branch": "old"}}"#,
        )
        .unwrap();
        let (p1, p2) = (ProjectId("p1".into()), ProjectId("p2".into()));
        let mut store = LinkStore::load(path.clone());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("linked_at"),
            "dated on the load that found it undated"
        );
        store.remember(&p1, "gone", &[issue("1", "ENG-1", "a")]);
        store.remember(&p1, "kept", &[issue("2", "ENG-2", "b")]);
        store.remember(&p2, "gone", &[issue("3", "ENG-3", "c")]);
        let now = orion_core::clock::now_secs();
        let live: std::collections::HashSet<String> = ["kept".to_string()].into();

        assert!(!store.prune(&p1, &live, now + 89 * DAY), "not yet");
        assert_eq!(store.branches_of("1"), ["gone"]);
        assert!(store.prune(&p1, &live, now + 91 * DAY));
        assert!(store.branches_of("1").is_empty(), "its branch is gone");
        assert!(store.branches_of("9").is_empty(), "the older one too");
        assert_eq!(store.branches_of("2"), ["kept"], "still worked on");
        assert_eq!(store.branches_of("3"), ["gone"], "another project's");
        assert!(!store.prune(&p1, &live, now + 91 * DAY), "nothing left to");

        let back = LinkStore::load(path);
        assert!(back.branches_of("1").is_empty(), "forgotten on disk too");
        assert_eq!(back.branches_of("2"), ["kept"]);
    }

    /// Linking more issues to a branch whose pull request took some
    /// already keeps those on it: the new ones wait, the old ones still
    /// find the branch.
    #[test]
    fn linking_a_branch_again_keeps_the_issues_it_had() {
        let p1 = ProjectId("p1".into());
        let mut store = LinkStore::default();
        store.remember_attached(&p1, "feat", &[("1".into(), "ENG-1".into())], &[]);
        store.remember(&p1, "feat", &[issue("2", "ENG-2", "b")]);
        assert_eq!(store.pending(&p1, "feat"), ["ENG-2"]);
        assert_eq!(store.branches_of("1"), ["feat"]);
        assert_eq!(store.branches_of("2"), ["feat"]);
    }

    /// A link Linear refused is not spent: the next list tries it again,
    /// until [`ATTACH_TRIES`] refusals in a row, when the footer says it
    /// gave up, the worktree pick stops calling it waiting, and no list
    /// asks again.
    #[test]
    fn a_refused_link_is_tried_on_the_next_list_until_it_gives_up() {
        let (mut app, _dir, mut rx) = paired_with_worktrees();
        let project = ProjectId("p1".into());
        app.linear_links
            .remember(&project, "feature-x", &[issue("1", "ENG-1", "Login")]);
        let mut fresh = app.open_prs[&project].list.clone();
        let mut opened = open_pr(43, "Feature X");
        opened.head = "feature-x".into();
        fresh.insert(0, opened);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let sent = with_graphql_stub(
            |_, _| Ok(serde_json::json!({"errors": [{"message": "Entity not found"}]})),
            || {
                rt.block_on(async {
                    for _ in 0..ATTACH_TRIES {
                        assert_eq!(app.linear_links.pending(&project, "feature-x"), ["ENG-1"]);
                        attach_new_prs(&mut app, &project, None, &fresh);
                        let answer = rx.recv().await.expect("an answer");
                        land_answer(&mut app, answer);
                    }
                    attach_new_prs(&mut app, &project, None, &fresh);
                });
                graphql_sent()
            },
        );
        assert_eq!(sent.len(), ATTACH_TRIES as usize, "not asked a fourth time");
        assert_eq!(
            app.flash.as_deref(),
            Some("couldn't attach PR #43 to ENG-1: Entity not found — gave up after 3 tries")
        );
        assert!(app.linear_links.pending(&project, "feature-x").is_empty());
        assert_eq!(
            app.linear_links.gave_up_on(&project, "feature-x"),
            ["ENG-1"]
        );
    }

    /// A pull request Linear's own GitHub integration linked first is
    /// refused as a duplicate: that is the link made, so it is spent
    /// silently and never tried again.
    #[test]
    fn a_link_linear_already_made_counts_as_attached() {
        let (mut app, _dir, mut rx) = paired_with_worktrees();
        let project = ProjectId("p1".into());
        app.linear_links
            .remember(&project, "feature-x", &[issue("1", "ENG-1", "Login")]);
        let mut fresh = app.open_prs[&project].list.clone();
        let mut opened = open_pr(43, "Feature X");
        opened.head = "feature-x".into();
        fresh.insert(0, opened);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let sent = with_graphql_stub(
            |_, _| {
                Ok(serde_json::json!({"errors": [
                    {"message": "Duplicate attachment for duplicate url"}]}))
            },
            || {
                rt.block_on(async {
                    attach_new_prs(&mut app, &project, None, &fresh);
                    let answer = rx.recv().await.expect("an answer");
                    land_answer(&mut app, answer);
                    attach_new_prs(&mut app, &project, None, &fresh);
                });
                graphql_sent()
            },
        );
        assert_eq!(sent.len(), 1, "not asked again");
        assert!(app.flash.is_none(), "{:?}", app.flash.as_deref());
        assert!(app.linear_links.pending(&project, "feature-x").is_empty());
        assert_eq!(app.linear_links.branches_of("1"), ["feature-x"]);
    }

    /// A ⌘L launch onto a worktree whose pull request is open already has
    /// nothing to wait for: its issues are attached there and then.
    #[test]
    fn a_launch_onto_a_branch_with_an_open_pr_attaches_at_once() {
        let (mut app, dir, mut rx) = paired_with_worktrees();
        let project = ProjectId("p1".into());
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let cfg = crate::config::Config::load();
            let target = QuickTarget::Worktree(orion_core::WorktreeId("w1".into()));
            let launch = QuickLaunch::from_config(target, &cfg).with_linear(Some(LinearBatch {
                issues: vec![issue("2", "ENG-2", "Logout")],
                task: "go".into(),
            }));
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let sent = with_graphql_stub(
                |_, _| {
                    Ok(serde_json::json!({"data": {"attachmentLinkGitHubPR": {"success": true}}}))
                },
                || {
                    rt.block_on(async {
                        remember_submit(&mut app, &launch);
                        let answer = rx.recv().await.expect("an answer");
                        land_answer(&mut app, answer);
                    });
                    graphql_sent()
                },
            );
            assert_eq!(sent, [attached("2", 41)]);
        });
        assert!(app.linear_links.pending(&project, "branch-41").is_empty());
        assert_eq!(app.linear_links.branches_of("2"), ["branch-41"]);
        assert!(app.flash.is_none(), "an auto-attach that worked is silent");
    }

    /// ONE HOME: an issue linked to another branch leaves the one it was
    /// on, handing back the attachment orion made there — never one
    /// Linear made itself — and a link left with no issue is forgotten.
    #[test]
    fn an_issue_linked_to_another_branch_leaves_the_one_it_was_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("linear-links.json");
        let p1 = ProjectId("p1".into());
        let mut store = LinkStore::load(path.clone());
        let both = [issue("1", "ENG-1", "a"), issue("2", "ENG-2", "b")];
        assert!(store.remember(&p1, "old", &both).is_empty());
        // Linear had ENG-2 linked already: only ENG-1's is orion's.
        let made = [("1".to_string(), "att-1".to_string())];
        store.finish_attach(&p1, "old", &["1".into(), "2".into()], &made, true);

        let mut store = LinkStore::load(path.clone());
        let stale = store.remember(&p1, "new", &both[..1]);
        assert_eq!(
            stale,
            [StaleAttachment {
                identifier: "ENG-1".into(),
                id: "att-1".into()
            }]
        );
        assert_eq!(store.branches_of("1"), ["new"]);
        assert_eq!(store.pending(&p1, "new"), ["ENG-1"]);
        assert_eq!(store.branches_of("2"), ["old"], "the rest stay");
        assert!(store.pending(&p1, "old").is_empty());

        let moved = [("2".to_string(), "ENG-2".to_string())];
        assert!(
            store.remember_attached(&p1, "new", &moved, &[]).is_empty(),
            "Linear's own link is Linear's to keep"
        );
        assert_eq!(store.branches_of("2"), ["new"]);
        assert!(store.begin_attach(&p1, "old").is_none(), "forgotten");
        let again = store.remember(&p1, "new", &both[..1]);
        assert!(again.is_empty(), "its own branch again moves nothing");
        let back = LinkStore::load(path);
        assert_eq!(back.branches_of("1"), ["new"]);
        assert_eq!(back.branches_of("2"), ["new"]);
    }

    /// A ⌘L launch that takes an issue to another worktree takes it off
    /// the pull request it was on: orion drops the attachment it made
    /// there, so that pull request's merge no longer moves the issue.
    #[test]
    fn a_launch_onto_another_worktree_takes_the_issue_off_its_old_pr() {
        let (mut app, dir, mut rx) = paired_with_worktrees();
        let project = ProjectId("p1".into());
        app.linear_links
            .remember(&project, "branch-41", &[issue("2", "ENG-2", "Logout")]);
        let open = app.open_prs[&project].list.clone();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let cfg = crate::config::Config::load();
            let target = QuickTarget::Worktree(orion_core::WorktreeId("w2".into()));
            let launch = QuickLaunch::from_config(target, &cfg).with_linear(Some(LinearBatch {
                issues: vec![issue("2", "ENG-2", "Logout")],
                task: "go".into(),
            }));
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let sent = with_graphql_stub(
                |_, query| {
                    Ok(if query.contains("attachmentDelete") {
                        serde_json::json!({"data": {"attachmentDelete": {"success": true}}})
                    } else {
                        serde_json::json!({"data": {"attachmentLinkGitHubPR": {
                            "success": true, "attachment": {"id": "att-41"}}}})
                    })
                },
                || {
                    rt.block_on(async {
                        attach_new_prs(&mut app, &project, None, &open);
                        let answer = rx.recv().await.expect("the attach");
                        land_answer(&mut app, answer);
                        remember_submit(&mut app, &launch);
                        let answer = rx.recv().await.expect("the detach");
                        let sent = graphql_sent();
                        land_answer(&mut app, answer);
                        sent
                    })
                },
            );
            assert_eq!(
                sent,
                [attached("2", 41), serde_json::json!({"id": "att-41"})]
            );
        });
        assert_eq!(app.linear_links.branches_of("2"), ["feature-x"]);
        assert_eq!(app.linear_links.pending(&project, "feature-x"), ["ENG-2"]);
        assert!(app.flash.is_none(), "{:?}", app.flash.as_deref());
    }

    /// An issue that moves to another branch while its attach to the old
    /// one is still out is attached there all the same: that attachment
    /// is dropped as the answer lands.
    #[test]
    fn an_attach_that_lands_after_its_issue_moved_on_is_taken_off() {
        let (mut app, _dir, mut rx) = paired_with_worktrees();
        let project = ProjectId("p1".into());
        let moving = [issue("2", "ENG-2", "Logout")];
        app.linear_links.remember(&project, "branch-41", &moving);
        let open = app.open_prs[&project].list.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let sent = with_graphql_stub(
            |_, query| {
                Ok(if query.contains("attachmentDelete") {
                    serde_json::json!({"data": {"attachmentDelete": {"success": true}}})
                } else {
                    serde_json::json!({"data": {"attachmentLinkGitHubPR": {
                        "success": true, "attachment": {"id": "att-41"}}}})
                })
            },
            || {
                rt.block_on(async {
                    attach_new_prs(&mut app, &project, None, &open);
                    let moved = app.linear_links.remember(&project, "feature-x", &moving);
                    assert!(moved.is_empty(), "nothing made yet");
                    let answer = rx.recv().await.expect("the attach");
                    land_answer(&mut app, answer);
                    let answer = rx.recv().await.expect("the detach");
                    assert!(matches!(
                        answer,
                        LinearAnswer::Detached { refused: None, .. }
                    ));
                    graphql_sent()
                })
            },
        );
        assert_eq!(
            sent,
            [attached("2", 41), serde_json::json!({"id": "att-41"})]
        );
        assert_eq!(app.linear_links.branches_of("2"), ["feature-x"]);
        assert!(app.flash.is_none(), "{:?}", app.flash.as_deref());
    }

    /// An old pull request Linear would not drop is said, by issue.
    #[test]
    fn a_refused_detach_says_which_issue_and_why() {
        let (mut app, dir, _rx) = paired();
        land_answer(
            &mut app,
            LinearAnswer::Detached {
                project: ProjectId("p1".into()),
                dir: dir.path().into(),
                refused: Some(("ENG-2".into(), "Entity not found".into())),
            },
        );
        assert_eq!(
            app.flash.as_ref().map(|f| (f.kind, f.text.as_str())),
            Some((
                crate::flash::FlashKind::Failed,
                "couldn't take ENG-2 off the PR it moved from: Entity not found"
            ))
        );
    }

    // ---- the TODOS MODAL's chips ----

    /// What Linear says of `ident`, in `state`.
    fn linked(ident: &str, state: &str, kind: &str) -> LinkedIssue {
        LinkedIssue {
            identifier: ident.into(),
            url: format!("https://linear.app/x/issue/{ident}"),
            state: state.into(),
            state_type: kind.into(),
            state_color: String::new(),
            priority: 0,
        }
    }

    /// `todo_app`'s item linked to `ident`, last seen `seen`.
    fn link_item(
        app: &mut App,
        dir: &tempfile::TempDir,
        item: u64,
        ident: &str,
        seen: Option<&str>,
    ) {
        let todo = app
            .todos
            .get_mut(dir.path())
            .unwrap()
            .item_mut(item)
            .unwrap();
        todo.linear = Some(ident.into());
        todo.linear_seen = seen.map(str::to_string);
    }

    /// An answer asked before the one a chip shows is older news: it
    /// lands without moving the chip, or what the todo last saw, back.
    #[test]
    fn an_older_linked_answer_never_moves_a_chip_back() {
        let (mut app, dir, _rx, item) = todo_app();
        link_item(&mut app, &dir, item, "RIP-1", Some("started"));
        let older = crate::fetch::now();
        let newer = crate::fetch::now();
        let land = |app: &mut App, at, issue| {
            crate::todos::view::land_linked(
                app,
                dir.path().into(),
                at,
                Ok(vec![("RIP-1".into(), issue)]),
            )
        };
        land(&mut app, newer, linked("RIP-1", "Todo", "unstarted"));
        land(&mut app, older, linked("RIP-1", "In Progress", "started"));
        assert_eq!(todo_view(&app).linked["RIP-1"].state, "Todo");
        assert_eq!(
            todo_item(&app, &dir, item).linear_seen.as_deref(),
            Some("unstarted")
        );
    }

    /// Linking a todo from the LINEAR VIEW asks Linear at once, and that
    /// first answer is only what the todo was linked in — done already,
    /// it never ticks the todo; the next answer done again doesn't either.
    #[test]
    fn the_first_answer_after_linking_never_ticks_the_todo() {
        let (mut app, dir, mut rx, item) = todo_app();
        let back = todo_view(&app).clone();
        with_graphql_stub(
            |_, _| {
                Ok(serde_json::json!({"data": {"i0": {
                    "identifier": "RIP-1", "url": "u",
                    "state": {"name": "Done", "type": "completed", "color": ""}, "priority": 0
                }}}))
            },
            || {
                run_linear(&mut app, &mut rx, |app| {
                    crate::todos::view::link_issue(
                        app,
                        back,
                        item,
                        linked("RIP-1", "In Progress", "started"),
                    );
                });
                run_linear(&mut app, &mut rx, |app| press(app, cmd('r')));
            },
        );
        let todo = todo_item(&app, &dir, item);
        assert!(todo.done.is_none(), "never ticked");
        assert_eq!(todo.linear_seen.as_deref(), Some("completed"));
        assert_eq!(todo_view(&app).linked["RIP-1"].state, "Done");
    }

    /// A team move renames an issue; Linear still finds it by the old
    /// identifier, and the todo is linked by the new one from then on.
    #[test]
    fn a_renamed_issue_renames_the_todos_link() {
        let (mut app, dir, mut rx, item) = todo_app();
        link_item(&mut app, &dir, item, "ENG-12", Some("started"));
        with_graphql_stub(
            |_, query| {
                assert!(query.contains(r#"i0: issue(id: "ENG-12")"#), "{query}");
                Ok(serde_json::json!({"data": {"i0": {
                    "identifier": "OPS-3", "url": "u",
                    "state": {"name": "In Progress", "type": "started", "color": ""}, "priority": 0
                }}}))
            },
            || run_linear(&mut app, &mut rx, |app| press(app, cmd('r'))),
        );
        assert_eq!(todo_item(&app, &dir, item).linear.as_deref(), Some("OPS-3"));
        let linked = &todo_view(&app).linked;
        assert!(linked.contains_key("OPS-3"), "{linked:?}");
        assert!(!linked.contains_key("ENG-12"));
    }
}
