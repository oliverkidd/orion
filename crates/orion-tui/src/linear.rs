//! The LINEAR VIEW (`⌘L`): open Linear issues in two tabs — `My issues`,
//! assigned to you, and `Other issues`, the rest of your teams' — grouped
//! by status, a line per issue with its priority's letter, the reading
//! pane setting out its properties as Linear's sidebar does, and filtered
//! by words (a status, priority or label as well as the title), `key:value`
//! tokens and the FILTER PICK (`list_filter`).
//! Issues are picked together so one agent fixes them in one worktree and
//! opens one pull request. From the PULL REQUESTS MODAL the same list attaches a pull
//! request to the issues you mark (`attachmentLinkGitHubPR`) — and the
//! other way round, `⌘U` here flips to that modal as a PR PICK, Enter on
//! a pull request attaching it to the issues marked here. Both ends run
//! the one ATTACH ([`attach_issues`]). `⌘S` on
//! an issue lists its team's workflow states in the reading pane's place
//! ([`StatusPick`]) — read with the issues, so the list is up at once —
//! and Enter moves the issue to one (`issueUpdate`), the row saying so
//! before Linear has answered and put back if it refuses.
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
    panel_block, render_row, row_rect, search_line_lit, sections, truncate, visible_positions,
    ListEntry, SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
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

/// One open Linear issue: assigned to the configured user (`mine`), or
/// someone else's or nobody's in one of their teams.
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
    /// Assigned to the configured user: the `My issues` tab's, else
    /// `Other issues`'.
    #[serde(default)]
    pub mine: bool,
    /// RFC 3339.
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
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

/// `⌘S`: the issue under the cursor, and the states it can move to,
/// in the reading pane's place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPick {
    pub issue_id: String,
    pub identifier: String,
    pub states: Vec<LinearState>,
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
    /// The status picker, while it is up: every key but Esc is its own.
    pub status_pick: Option<StatusPick>,
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
            status_pick: None,
            tab: LinearTab::Mine,
            tab_hits: Vec::new(),
            tab_row: Rect::default(),
            list_start: 0,
            row_rects: Vec::new(),
            filter_pick: None,
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

/// What Linear last said about a project's open issues — the configured
/// user's and the rest of their teams' — and the workflow states of the
/// teams they belong to, by team id, in Linear's own order.
#[derive(Debug, Clone, Default)]
pub struct LinearList {
    pub list: Vec<LinearIssue>,
    pub states: HashMap<String, Vec<LinearState>>,
    /// Linear had more of the other issues than one page holds
    /// ([`OTHERS_LIMIT`]): the list says it shows the most recent.
    pub more: bool,
}

/// A finished Linear call, back on the loop.
#[derive(Debug, Clone)]
pub enum LinearAnswer {
    List {
        project: ProjectId,
        list: Result<LinearList, String>,
    },
    /// An issue moved to another state — or why not, with the state it
    /// had, to put back.
    Status {
        project: ProjectId,
        issue_id: String,
        identifier: String,
        state: LinearState,
        result: Result<(), StatusRefused>,
    },
    /// LINEAR AUTO-ATTACH linked a pull request to one issue — silent
    /// unless Linear refused.
    Attach { result: Result<(), String> },
    /// THE ATTACH the user asked for, from either end ([`attach_issues`]):
    /// the pull request, the identifiers Linear linked it to, and the
    /// first it refused, with why.
    Attached {
        pr_number: u64,
        attached: Vec<String>,
        refused: Option<(String, String)>,
    },
    /// **Test connection**: who the key in `dir` belongs to.
    Viewer {
        dir: PathBuf,
        result: Result<Viewer, String>,
    },
    /// The TODOS MODAL's linked issues in the checkout `dir`, as Linear
    /// has them now ([`request_linked`]).
    Linked {
        dir: PathBuf,
        result: Result<Vec<LinkedIssue>, String>,
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

/// Why Linear refused to move an issue, and the state it was in before
/// the row said otherwise — what [`land_answer`] puts back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRefused {
    pub why: String,
    pub status: String,
    pub status_type: String,
    pub state_color: String,
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

/// Branch → Linear issues, so a pull request cut from a ⌘L launch can be
/// attached once GitHub lists it.
#[derive(Debug, Clone, Default)]
pub struct LinkStore {
    path: Option<PathBuf>,
    links: HashMap<String, PendingLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingLink {
    issue_ids: Vec<String>,
    identifiers: Vec<String>,
}

impl LinkStore {
    pub fn load(path: PathBuf) -> Self {
        let links = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            links,
        }
    }

    pub fn remember(&mut self, branch: &str, issues: &[LinearIssue]) {
        if branch.is_empty() || issues.is_empty() {
            return;
        }
        self.links.insert(
            branch.to_string(),
            PendingLink {
                issue_ids: issues.iter().map(|i| i.id.clone()).collect(),
                identifiers: issues.iter().map(|i| i.identifier.clone()).collect(),
            },
        );
        self.persist();
    }

    pub(crate) fn take(&mut self, branch: &str) -> Option<PendingLink> {
        let link = self.links.remove(branch)?;
        self.persist();
        Some(link)
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
    request_list(app, project, dir);
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

fn request_list(app: &mut App, project: ProjectId, dir: PathBuf) {
    if app.linear_inflight.contains(&project) {
        return;
    }
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    app.linear_inflight.insert(project.clone());
    app.linear_failed.remove(&project);
    app.dirty = true;
    let email = crate::config::Config::load()
        .linear_assignee_email
        .trim()
        .to_string();
    tokio::spawn(async move {
        let result = fetch_lists(&dir, &email).await;
        let _ = tx.send(LinearAnswer::List {
            project,
            list: result,
        });
    });
}

pub(crate) fn land_answer(app: &mut App, answer: LinearAnswer) {
    match answer {
        LinearAnswer::Linked { dir, result } => crate::todos::view::land_linked(app, dir, result),
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
        LinearAnswer::List { project, list } => {
            app.linear_inflight.remove(&project);
            match list {
                Ok(fetched) => {
                    let n = fetched.list.len();
                    app.linear_failed.remove(&project);
                    app.linear.insert(project.clone(), fetched);
                    if let Some(Overlay::Linear(view)) = &mut app.overlay {
                        if view.project == project {
                            view.selected = clamp_selection(view.selected as i64, n);
                        }
                    }
                }
                Err(err) => {
                    app.linear_failed.insert(project);
                    app.flash = Some(crate::flash::Flash::failed(err));
                }
            }
            app.dirty = true;
        }
        LinearAnswer::Attach { result } => {
            if let Err(err) = result {
                app.flash = Some(crate::flash::Flash::failed(err));
            }
        }
        // The wait the footer spun for is over: it says what Linear took,
        // or the first issue it would not take and why.
        LinearAnswer::Attached {
            pr_number,
            attached,
            refused,
        } => {
            app.flash = Some(match refused {
                Some((identifier, why)) => crate::flash::Flash::failed(format!(
                    "couldn't attach PR #{pr_number} to {identifier}: {why}"
                )),
                None => crate::flash::Flash::done(format!(
                    "attached PR #{pr_number} to {}",
                    attached.join(", ")
                )),
            });
            app.dirty = true;
        }
        LinearAnswer::Status {
            project,
            issue_id,
            identifier,
            result,
            ..
        } => {
            // The row already reads the new status; a refusal puts back
            // what it said before the move, and says why.
            if let Err(refused) = result {
                if let Some(issue) = app
                    .linear
                    .get_mut(&project)
                    .and_then(|l| l.list.iter_mut().find(|i| i.id == issue_id))
                {
                    issue.status = refused.status;
                    issue.status_type = refused.status_type;
                    issue.state_color = refused.state_color;
                }
                app.flash = Some(crate::flash::Flash::failed(format!(
                    "couldn't move {identifier}: {}",
                    refused.why
                )));
            }
            app.dirty = true;
        }
    }
}

/// Remember a ⌘L launch's branch so the PR it opens can be attached.
pub(crate) fn remember_submit(app: &mut App, launch: &QuickLaunch) {
    let Some(batch) = &launch.linear else {
        return;
    };
    if !crate::config::Config::load().linear_auto_attach {
        return;
    }
    let branch = match &launch.target {
        QuickTarget::NewWorktree { branch, .. } => branch.clone(),
        QuickTarget::Worktree(id) => app
            .tree
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .map(|w| w.branch.clone())
            .unwrap_or_default(),
    };
    app.linear_links.remember(&branch, &batch.issues);
}

/// When a new pull request appears on a remembered branch, attach it.
pub(crate) fn attach_new_prs(
    app: &mut App,
    project: &ProjectId,
    previous: Option<&[crate::pull_request::OpenPr]>,
    fresh: &[crate::pull_request::OpenPr],
) {
    if !crate::config::Config::load().linear_auto_attach {
        return;
    }
    let dir = app
        .tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map(|p| p.repo_path.clone());
    let Some(dir) = dir else {
        return;
    };
    for pr in fresh {
        let was = previous.is_some_and(|was| was.iter().any(|old| old.url == pr.url));
        if was {
            continue;
        }
        let Some(link) = app.linear_links.take(&pr.head) else {
            continue;
        };
        for id in link.issue_ids {
            spawn_attach(app, dir.clone(), id, pr.url.clone());
        }
    }
}

fn spawn_attach(app: &mut App, dir: PathBuf, issue_id: String, pr_url: String) {
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    tokio::spawn(async move {
        let result = attach_pr(&dir, &issue_id, &pr_url).await;
        let _ = tx.send(LinearAnswer::Attach { result });
    });
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
    app.flash = Some(crate::flash::Flash::working(format!(
        "attaching PR #{pr_number} to {}…",
        ids_of(issues)
    )));
    app.dirty = true;
    tokio::spawn(async move {
        let mut attached = Vec::new();
        let mut refused = None;
        for (id, identifier) in targets {
            match attach_pr(&dir, &id, &pr_url).await {
                Ok(()) => attached.push(identifier),
                Err(why) => {
                    refused.get_or_insert((identifier, why));
                }
            }
        }
        let _ = tx.send(LinearAnswer::Attached {
            pr_number,
            attached,
            refused,
        });
    });
}

/// The LINEAR VIEW's own keys: one table [`handle_key`] matches and
/// [`hints`] spells.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const MARK: Key = Key::new(&["space"], "mark");
    pub const CONFIRM: Key = Key::new(&["enter"], "agent on marked");
    pub const PRESET: Key = Key::new(&["shift+tab"], "preset");
    pub const BROWSER: Key = crate::issues::keys::BROWSER;
    pub const REFRESH: Key = crate::issues::keys::REFRESH;
    /// The issue's workflow state.
    pub const STATUS: Key = Key::new(&["cmd+s", "ctrl+s"], "status");
    /// The PR PICK: the marked issues attached to a pull request picked
    /// in the PULL REQUESTS MODAL. That modal's own hotkey (`⌘U`, `^V`
    /// its twin), as the modal's way here is this one's (`⌘L`).
    pub const ATTACH: Key = Key::new(&["cmd+u", "ctrl+v"], "attach to PR");
    /// The status picker's own.
    pub const PICK: Key = Key::new(&["up", "down"], "pick").show(2);
    pub const SET: Key = Key::new(&["enter"], "set status");
    /// `My issues` ⇄ `Other issues`: plain or with ⇧, as the PULL
    /// REQUESTS MODAL's tab keys are.
    pub const TABS: Key = Key::new(&["left", "right", "shift+left", "shift+right"], "tabs").show(2);
    /// The FILTER PICK — the PULL REQUESTS MODAL's too.
    pub const FILTER: Key = crate::list_filter::keys::FILTER;
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        MARK, CONFIRM, PRESET, BROWSER, REFRESH, STATUS, ATTACH, PICK, SET, TABS, FILTER,
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
    if view.status_pick.is_some() {
        return vec![
            keys::SET.hint().kept(),
            keys::PICK.hint(),
            Hint::new("Esc", "cancel"),
        ];
    }
    if view.filter_pick.is_some() {
        return crate::list_filter::hints();
    }
    match view.mode {
        LinearMode::Browse => vec![
            keys::MARK.hint(),
            keys::CONFIRM.hint().kept(),
            keys::TABS.hint(),
            keys::FILTER.hint(),
            keys::ATTACH.hint(),
            keys::STATUS.hint(),
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
            keys::MARK.hint(),
            keys::CONFIRM.hint_as("attach marked to this PR").kept(),
            keys::TABS.hint(),
            keys::FILTER.hint(),
            keys::STATUS.hint(),
            keys::BROWSER.hint(),
            Hint::new("Esc", esc),
        ],
    }
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
    let selected = states
        .iter()
        .position(|s| s.name == issue.status)
        .unwrap_or(0);
    if let Some(Overlay::Linear(view)) = &mut app.overlay {
        view.filter_pick = None;
        view.status_pick = Some(StatusPick {
            issue_id: issue.id,
            identifier: issue.identifier,
            states,
            selected,
        });
    }
}

/// Keys while the status picker is up.
fn handle_pick_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    let Some(pick) = &mut view.status_pick else {
        return;
    };
    match key.code {
        KeyCode::Esc => view.status_pick = None,
        KeyCode::Down => {
            pick.selected = clamp_selection(pick.selected as i64 + 1, pick.states.len())
        }
        KeyCode::Up => pick.selected = clamp_selection(pick.selected as i64 - 1, pick.states.len()),
        _ if keys::SET.matches(&key) => set_status(app),
        _ => {}
    }
    app.dirty = true;
}

/// Enter in the status picker: the row says the new state at once, and
/// `issueUpdate` runs off the loop — put back if Linear refuses. The
/// state the issue is already in closes the picker with nothing sent.
fn set_status(app: &mut App) {
    let Some(Overlay::Linear(view)) = &mut app.overlay else {
        return;
    };
    let Some(pick) = view.status_pick.take() else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(state) = pick.states.get(pick.selected).cloned() else {
        return;
    };
    let Some(issue) = app
        .linear
        .get_mut(&project)
        .and_then(|l| l.list.iter_mut().find(|i| i.id == pick.issue_id))
    else {
        return;
    };
    if issue.status == state.name {
        return;
    }
    let (status, status_type, state_color) = (
        issue.status.clone(),
        issue.status_type.clone(),
        issue.state_color.clone(),
    );
    issue.status = state.name.clone();
    issue.status_type = state.kind.clone();
    issue.state_color = state.color.clone();
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    let (issue_id, identifier) = (pick.issue_id, pick.identifier);
    tokio::spawn(async move {
        let result = update_state(&dir, &issue_id, &state.id)
            .await
            .map_err(|why| StatusRefused {
                why,
                status,
                status_type,
                state_color,
            });
        let _ = tx.send(LinearAnswer::Status {
            project,
            issue_id,
            identifier,
            state,
            result,
        });
    });
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
    if matches!(&app.overlay, Some(Overlay::Linear(v)) if v.status_pick.is_some()) {
        handle_pick_key(app, key);
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
    match key.code {
        KeyCode::Esc if !view.query.is_empty() => clear_query(app),
        KeyCode::Esc => close(app),
        KeyCode::Down if shift => view.scroll_by(1),
        KeyCode::Up if shift => view.scroll_by(-1),
        KeyCode::Down => step(app, 1),
        KeyCode::Up => step(app, -1),
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
        _ if keys::MARK.matches(&key) => toggle_mark(app),
        _ if keys::CONFIRM.matches(&key) => confirm(app),
        _ if keys::PRESET.matches(&key) => open_preset(app),
        _ if keys::BROWSER.matches(&key) => open_in_browser(app, out),
        _ if keys::REFRESH.matches(&key) => refresh(app),
        _ if keys::STATUS.matches(&key) => open_status_pick(app),
        _ if keys::ATTACH.matches(&key) => open_pr_pick(app),
        _ if keys::FILTER.matches(&key) => view.filter_pick = Some(FilterPick::default()),
        _ => {
            if view.query.handle_key(&key).changed() {
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
        MouseEventKind::ScrollDown if list.contains(pos) => step(app, 1),
        MouseEventKind::ScrollUp if list.contains(pos) => step(app, -1),
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
    request_list(app, view.project.clone(), view.dir.clone());
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

fn step(app: &mut App, delta: i32) {
    let Some(Overlay::Linear(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let visible = visible_rows(view, list);
    if visible.is_empty() {
        return;
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
    // The project's root, as an issue's box starts; the box's WORKTREE
    // PICKER offers a fresh worktree first (`LinearBatch::branch`).
    let target = app
        .root_worktree(&project)
        .map(QuickTarget::Worktree)
        .unwrap_or_else(|| QuickTarget::NewWorktree {
            project,
            branch: batch.branch(&taken),
            existing: false,
        });
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
/// `status:todo p:high label:bug project:"Export PDF" assignee:sam`.
pub(crate) const FACETS: &[FacetKey] = &[
    FacetKey::new("status", "Status"),
    FacetKey::new("priority", "Priority").aliases(&["p"]),
    FacetKey::new("label", "Label"),
    FacetKey::new("project", "Project"),
    FacetKey::new("assignee", "Assignee"),
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
        status_rank(&a.status_type)
            .cmp(&status_rank(&b.status_type))
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
    let inflight = app.linear_inflight.contains(&view.project);
    let failed = app.linear_failed.contains(&view.project);
    let more = app.linear.get(&view.project).is_some_and(|l| l.more);
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
    let side_up = view.status_pick.is_some() || view.filter_pick.is_some();
    let list_focused = list_focused && !side_up;
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
        let line = search_line_lit(&view.query, &placeholder, query_area, th, &parsed.spans);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let mut rows_area = crate::ui::below_first_row(below_tabs);
    if more && view.tab == LinearTab::Others {
        if let Some(note) = row_rect(rows_area, 0) {
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!("showing the {OTHERS_LIMIT} most recently updated"),
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
                let line = title_spans(issue, positions, marked, budget, th);
                render_row(f, rect, line, Some(*index) == cursor, list_focused, th);
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
    let (read_a, side_a) = reading_areas(body_inner);
    let lines: Vec<Line> = match current {
        Some(issue) => body_lines(issue, read_a.width as usize, side_a.is_none(), now, th),
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
    let mut filter_pick = view.filter_pick;
    if let Some(pick) = &view.status_pick {
        draw_status_pick(f, body_inner, pick, th);
    } else if let Some(pick) = &mut filter_pick {
        let facets = pick_facets(&issues, view.tab, th);
        pick.clamp(&facets);
        crate::list_filter::draw_pick(f, body_inner, &facets, &parsed, pick, th);
    } else {
        let shown: Vec<Line> = lines.iter().skip(scroll as usize).cloned().collect();
        f.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), read_a);
        if let (Some(side), Some(issue)) = (side_a, current) {
            let rule = ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::LEFT)
                .border_style(Style::default().fg(th.faint));
            let inner = rule.inner(side);
            f.render_widget(rule, side);
            let inner = Rect {
                x: inner.x + 1,
                width: inner.width.saturating_sub(1),
                ..inner
            };
            let props = properties(issue, inner.width as usize, now, th);
            f.render_widget(Paragraph::new(props), inner);
        }
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
    let label = truncate(&full, budget.saturating_sub(MARKS_W));
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
    spans
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

/// The status picker in the reading pane's place: what it is for, then a
/// row per state, the cursor's lit and the kind of each dim beside it.
fn draw_status_pick(f: &mut Frame, area: Rect, pick: &StatusPick, th: Theme) {
    if let Some(row) = row_rect(area, 0) {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("Move {} to…", pick.identifier),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            row,
        );
    }
    // The states start under the heading and a blank row.
    const HEAD_ROWS: u16 = 2;
    let rows = Rect {
        y: area.y.saturating_add(HEAD_ROWS),
        height: area.height.saturating_sub(HEAD_ROWS),
        ..area
    };
    let start = window_start(pick.selected, rows.height as usize);
    for (i, state) in pick.states.iter().enumerate().skip(start) {
        let Some(row) = row_rect(rows, i - start) else {
            break;
        };
        let spans = vec![
            Span::raw(state.name.clone()),
            Span::styled(format!("  {}", state.kind), Style::default().fg(th.dim)),
        ];
        render_row(f, row, spans, i == pick.selected, true, th);
    }
}

/// The reading pane at least this wide sets the issue's properties in a
/// column of their own on its right, as Linear does; narrower, they stack
/// under the title.
const SIDE_MIN_W: u16 = 72;
/// The properties column, its rule included.
const SIDE_W: u16 = 30;
/// The widest the description runs: a line past this is hard to read.
const READ_MAX_W: u16 = 88;

/// The reading pane's inside, split: the text — a column in from either
/// edge, no wider than [`READ_MAX_W`] — and, when it is wide enough, the
/// properties column on the right.
fn reading_areas(inner: Rect) -> (Rect, Option<Rect>) {
    let (text, side) = if inner.width >= SIDE_MIN_W {
        let [text, side] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(SIDE_W)]).areas(inner);
        (text, Some(side))
    } else {
        (inner, None)
    };
    let pad = if text.width > 4 { 1 } else { 0 };
    let text = Rect {
        x: text.x + pad,
        width: text.width.saturating_sub(pad * 2).min(READ_MAX_W),
        ..text
    };
    (text, side)
}

/// The issue's properties, a row each as Linear's sidebar lists them: a
/// dim name, then the value in Linear's colours — one row per label.
fn properties(issue: &LinearIssue, width: usize, now: i64, th: Theme) -> Vec<Line<'static>> {
    const NAME_W: usize = 10;
    let room = width.saturating_sub(NAME_W);
    let name = |n: &str| Span::styled(format!("{n:<NAME_W$}"), Style::default().fg(th.dim));
    let text = |t: &str| {
        Span::styled(
            truncate(t, room.saturating_sub(2)),
            Style::default().fg(th.text),
        )
    };
    let dot = |tag: &LinearTag| {
        let color = crate::theme::hex(&tag.color).unwrap_or(th.muted);
        vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::styled(
                truncate(&tag.name, room.saturating_sub(2)),
                Style::default().fg(th.text),
            ),
        ]
    };
    let mut lines = Vec::new();
    let mut row = |label: &str, value: Vec<Span<'static>>| {
        let mut spans = vec![name(label)];
        spans.extend(value);
        lines.push(Line::from(spans));
    };
    let (glyph, color) = state_mark(issue, th);
    row(
        "Status",
        vec![
            Span::styled(format!("{glyph} "), Style::default().fg(color)),
            text(&issue.status),
        ],
    );
    row(
        "Priority",
        vec![
            priority_mark(issue.priority, th),
            Span::raw(" "),
            text(priority_word(issue.priority)),
        ],
    );
    let who = if issue.assignee.is_empty() {
        Span::styled(UNASSIGNED, Style::default().fg(th.dim))
    } else {
        text(&issue.assignee)
    };
    row("Assignee", vec![who]);
    match &issue.project {
        Some(project) => row("Project", dot(project)),
        None => row(
            "Project",
            vec![Span::styled("none", Style::default().fg(th.dim))],
        ),
    }
    if issue.labels.is_empty() {
        row(
            "Labels",
            vec![Span::styled("none", Style::default().fg(th.dim))],
        );
    }
    for (i, label) in issue.labels.iter().enumerate() {
        row(if i == 0 { "Labels" } else { "" }, dot(label));
    }
    for (label, stamp) in [
        ("Created", &issue.created_at),
        ("Updated", &issue.updated_at),
    ] {
        if let Some(day) = short_date(stamp, now) {
            row(
                label,
                vec![Span::styled(day, Style::default().fg(th.muted))],
            );
        }
    }
    lines
}

/// The reading pane's text, `width` wide: the title, wrapped and bold,
/// the properties under it when they have no column of their own
/// (`stacked`), then the description.
fn body_lines(
    issue: &LinearIssue,
    width: usize,
    stacked: bool,
    now: i64,
    th: Theme,
) -> Vec<Line<'static>> {
    let title = Style::default().fg(th.text).add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = crate::pr_preview::wrap(&issue.title, width)
        .into_iter()
        .map(|row| Line::from(Span::styled(row, title)))
        .collect();
    lines.push(Line::default());
    if stacked {
        lines.extend(properties(issue, width, now, th));
        lines.push(Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(th.faint),
        )));
        lines.push(Line::default());
    }
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
/// reading pane, and its team's workflow states for `⌘S` — the same
/// fields whoever's issues are asked for.
const ISSUE_FIELDS: &str = "id identifier title url description priority createdAt updatedAt \
    state { name type color } labels { nodes { name color } } project { name color } \
    assignee { displayName } \
    team { id states { nodes { id name type position color } } }";

/// How many of the configured user's issues one ask lists.
const MINE_LIMIT: usize = 100;
/// How many of the rest of their teams' — Linear's most a page holds, the
/// most recently touched first.
pub const OTHERS_LIMIT: usize = 250;

/// Not done and not canceled: the open issues.
const OPEN_STATES: &str = r#"state: { type: { nin: ["completed", "canceled"] } }"#;

/// Both tabs' issues in one ask, as two aliased lists: `mine`, assigned to
/// the key's owner — or to `email`, when Settings → Linear account names
/// someone — and `others`, open issues in that person's teams assigned to
/// someone else or to nobody.
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
    let (head, me, not_me, variables) = if email.is_empty() {
        (
            "query",
            "isMe: { eq: true }",
            "isMe: { eq: false }",
            serde_json::json!({}),
        )
    } else {
        (
            "query($email: String!)",
            "email: { eq: $email }",
            "email: { neq: $email }",
            serde_json::json!({ "email": email }),
        )
    };
    let query = format!(
        r#"{head} {{
          mine: issues(first: {MINE_LIMIT}, orderBy: updatedAt, filter: {{
            assignee: {{ {me} }}
            {OPEN_STATES}
          }}) {{ nodes {{ {ISSUE_FIELDS} }} }}
          others: issues(first: {OTHERS_LIMIT}, orderBy: updatedAt, filter: {{
            team: {{ members: {{ some: {{ {me} }} }} }}
            or: [{{ assignee: {{ null: true }} }}, {{ assignee: {{ {not_me} }} }}]
            {OPEN_STATES}
          }}) {{ nodes {{ {ISSUE_FIELDS} }} pageInfo {{ hasNextPage }} }}
        }}"#
    );
    (query, variables)
}

/// Move issue `issue_id` to the workflow state `state_id`.
async fn update_state(dir: &Path, issue_id: &str, state_id: &str) -> Result<(), String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let json = graphql(
        &key,
        r#"mutation($id: String!, $stateId: String!) {
          issueUpdate(id: $id, input: { stateId: $stateId }) { success }
        }"#,
        serde_json::json!({ "id": issue_id, "stateId": state_id }),
    )
    .await?;
    mutation_result(&json, "issueUpdate", "Linear did not move the issue")
}

/// The fields a linked todo's chip draws, as [`LinkedIssue::from_json`]
/// reads them.
const LINKED_FIELDS: &str = "identifier url state { name type color } priority";

/// Every issue in `identifiers` in one ask, one aliased `issue(id:)` per
/// identifier. One Linear cannot find (moved, deleted) is left out: it
/// fails the whole ask — `issue` is never null, so Linear nulls `data` —
/// and then each is asked on its own, the missing ones failing alone. An
/// error every one of them hits (the key) is the answer's.
async fn fetch_linked(dir: &Path, identifiers: &[String]) -> Result<Vec<LinkedIssue>, String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let json = graphql(&key, &linked_query(identifiers), serde_json::json!({})).await?;
    let first = match parse_linked(&json, identifiers.len()) {
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
            .and_then(|json| parse_linked(&json, 1))
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

fn parse_linked(json: &serde_json::Value, count: usize) -> Result<Vec<LinkedIssue>, String> {
    let Some(data) = json.get("data").filter(|d| d.is_object()) else {
        return Err(graphql_error(json).unwrap_or_else(|| "Linear said nothing".into()));
    };
    Ok((0..count)
        .filter_map(|i| data.get(format!("i{i}")))
        .filter_map(LinkedIssue::from_json)
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
/// links to stand now: they land as [`LinearAnswer::Linked`].
pub(crate) fn request_linked(app: &mut App, dir: PathBuf, identifiers: Vec<String>) {
    if identifiers.is_empty() {
        return;
    }
    let Some(tx) = app.linear_tx.clone() else {
        return;
    };
    tokio::spawn(async move {
        let result = fetch_linked(&dir, &identifiers).await;
        let _ = tx.send(LinearAnswer::Linked { dir, result });
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
    let mut nodes = Vec::new();
    for (list, is_mine) in [(mine, true), (others, false)] {
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

async fn attach_pr(dir: &Path, issue_id: &str, url: &str) -> Result<(), String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let json = graphql(
        &key,
        r#"mutation($issueId: String!, $url: String!) {
          attachmentLinkGitHubPR(issueId: $issueId, url: $url) { success }
        }"#,
        serde_json::json!({ "issueId": issue_id, "url": url }),
    )
    .await?;
    mutation_result(
        &json,
        "attachmentLinkGitHubPR",
        "Linear did not attach the pull request",
    )
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
/// their teams' states, and whether Linear had more of the others.
fn parse_lists(json: &serde_json::Value) -> Result<LinearList, String> {
    Ok(LinearList {
        list: parse_issues(json)?,
        states: parse_states(json),
        more: json
            .pointer("/data/others/pageInfo/hasNextPage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// The issues of both lists, in the order the LINEAR VIEW's sections go:
/// by where their state stands (started, not yet, the backlog, the
/// rest), the state's name, then priority — urgent first, none last —
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
        status_rank(&a.status_type)
            .cmp(&status_rank(&b.status_type))
            .then_with(|| a.status.cmp(&b.status))
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
        mine: false,
        created_at: text("/createdAt"),
        updated_at: text("/updatedAt"),
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
        team_id: value
            .pointer("/team/id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn status_rank(kind: &str) -> u8 {
    match kind {
        "started" => 0,
        "unstarted" => 1,
        "backlog" => 2,
        _ => 3,
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
            mine: true,
            created_at: String::new(),
            updated_at: String::new(),
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
                more: false,
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
        let pick = view.status_pick.as_ref().expect("the picker");
        assert_eq!(pick.selected, 1, "on the state it is in");
        crate::hints::assert_hints_from(&hints(view), keys::ALL);
        handle_key(&mut app, KeyEvent::from(KeyCode::Down), &mut out);
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), &mut out);
        let Some(Overlay::Linear(view)) = &app.overlay else {
            panic!("the modal stays");
        };
        assert!(view.status_pick.is_none());
        assert_eq!(app.linear[&project].list[0].status, "Done");

        let done = app.linear[&project].states["t1"][2].clone();
        land_answer(
            &mut app,
            LinearAnswer::Status {
                project: project.clone(),
                issue_id: "1".into(),
                identifier: "ENG-12".into(),
                state: done,
                result: Err(StatusRefused {
                    why: "not allowed".into(),
                    status: "In Progress".into(),
                    status_type: "started".into(),
                    state_color: String::new(),
                }),
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
    fn link_store_remembers_and_takes() {
        let mut store = LinkStore::default();
        store.remember("eng-12-fix", &[issue("abc", "ENG-12", "Fix")]);
        assert!(store.take("other").is_none());
        let link = store.take("eng-12-fix").unwrap();
        assert_eq!(link.identifiers, ["ENG-12"]);
        assert!(store.take("eng-12-fix").is_none());
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
            is_draft: false,
            health: Default::default(),
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
                more: false,
            },
        );
        let now = std::time::Instant::now();
        app.open_prs.insert(
            project,
            crate::app::OpenPrs {
                list: vec![open_pr(42, "Fix login"), open_pr(41, "Spike")],
                at: now,
                due: now + std::time::Duration::from_secs(60),
                step: std::time::Duration::from_secs(60),
            },
        );
        // A list "in flight" is not asked for again: opening the view
        // stays on the loop, with no runtime under it.
        app.linear_inflight.insert(ProjectId("p1".into()));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        app.linear_tx = Some(tx);
        (app, dir, rx)
    }

    /// The LINEAR VIEW browsing `demo`, ENG-1 and ENG-3 marked.
    fn browse_marked(app: &mut App) {
        open(app);
        press(app, plain(KeyCode::Char(' ')));
        press(app, plain(KeyCode::Down));
        press(app, plain(KeyCode::Down));
        press(app, plain(KeyCode::Char(' ')));
        let Some(Overlay::Linear(view)) = &app.overlay else {
            panic!("the LINEAR VIEW, got {:?}", app.overlay);
        };
        assert_eq!(view.marked, BTreeSet::from(["1".into(), "3".into()]));
    }

    /// Typing in a Linear box never drops the issues: the text goes first
    /// and the marked issues follow in full, under the preset made from
    /// the box's own picker — which goes straight onto the box on save.
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
            for c in "go".chars() {
                press(&mut app, plain(KeyCode::Char(c)));
            }
            let mut out = Vec::new();
            crate::event_loop::handle_overlay_key(&mut app, plain(KeyCode::Enter), &mut out);
            let [ClientRequest::CreateAgent {
                starting_prompt, ..
            }] = out.as_slice()
            else {
                panic!("one CreateAgent, got {out:?}");
            };
            assert_eq!(
                starting_prompt.as_deref(),
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
            LinearAnswer::Attached {
                pr_number: 41,
                attached: vec!["ENG-1".into()],
                refused: Some(("ENG-3".into(), "Entity not found".into())),
            },
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
                more: false,
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
                 "assignee": {"displayName": "me"},
                 "state": {"name": "Todo", "type": "unstarted"}}
            ]},
            "others": {"nodes": [
                {"id": "3", "identifier": "ENG-3", "title": "Theirs", "url": "https://linear.app/x/issue/ENG-3",
                 "priority": 0, "assignee": null,
                 "state": {"name": "In Progress", "type": "started"}}
            ], "pageInfo": {"hasNextPage": true}}
        }});
        let fetched = parse_lists(&json).unwrap();
        assert!(fetched.more);
        let ids: Vec<&str> = fetched.list.iter().map(|i| i.identifier.as_str()).collect();
        assert_eq!(
            ids,
            ["ENG-3", "ENG-2", "ENG-1"],
            "started first, then urgent before low"
        );
        let urgent = &fetched.list[1];
        assert!(urgent.mine);
        assert_eq!(urgent.priority, 1);
        assert_eq!(urgent.labels.len(), 1, "a nameless label is dropped");
        assert_eq!(urgent.project.as_ref().unwrap().name, "Exports");
        assert_eq!(urgent.assignee, "me");
        assert_eq!(fetched.list[2].state_color, "#26b5ce");
        assert!(!fetched.list[0].mine);
        assert_eq!(fetched.list[0].assignee, "");
        // An answer with neither list is a miss.
        assert!(parse_lists(&serde_json::json!({"data": {}})).is_err());
    }

    /// The one ask names both lists; with Settings → Linear account set,
    /// that person stands in for the key's owner on both.
    #[test]
    fn the_lists_query_asks_for_mine_and_the_teams_others() {
        let (query, vars) = lists_query("");
        assert!(query.contains("mine: issues(first: 100"), "{query}");
        assert!(query.contains("others: issues(first: 250"), "{query}");
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
        assert_eq!(vars, serde_json::json!({}));
        let (query, vars) = lists_query("sam@x.co");
        assert!(query.starts_with("query($email: String!)"), "{query}");
        assert!(query.contains("email: { neq: $email }"), "{query}");
        assert!(!query.contains("isMe"), "{query}");
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
        assert!(at("IN PROGRESS 1") < at("ENG-1 Started one"), "{screen}");
        assert!(at("ENG-1 Started one") < at("TODO 2"), "{screen}");
        assert!(at("TODO 2") < at("ENG-2 Todo one"), "{screen}");
        assert!(screen.contains("◑ H ENG-1 Started one"), "{screen}");
        assert!(screen.contains("○ U ENG-2 Todo one"), "{screen}");
        assert!(screen.contains("○ · ENG-3 Todo two"), "{screen}");
        assert!(!screen.contains("Their bug"), "{screen}");

        assert_eq!(selected_id(&app).as_deref(), Some("ENG-1"));
        handle_key(&mut app, KeyEvent::from(KeyCode::Down), &mut Vec::new());
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-2"));

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
        assert_eq!(selected_id(&app).as_deref(), Some("ENG-1"));
    }

    /// Tokens narrow by status, priority, label, project and assignee —
    /// beside the fuzzy words — within the tab showing.
    #[test]
    fn tokens_narrow_the_issues_by_facet() {
        let mut theirs = rich("4", "ENG-4", "Their bug", ("Todo", "unstarted"), 2);
        theirs.mine = false;
        theirs.assignee = "Sam".into();
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
            ["ENG-1", "ENG-2"]
        );
        assert_eq!(shown(LinearTab::Mine, "p:none"), ["ENG-3"]);
        assert_eq!(shown(LinearTab::Mine, "status:todo label:bug"), ["ENG-3"]);
        assert_eq!(shown(LinearTab::Mine, "status:in-progress"), ["ENG-1"]);
        assert_eq!(
            shown(LinearTab::Mine, "label:export -label:bug"),
            ["ENG-1", "ENG-2"]
        );
        assert_eq!(shown(LinearTab::Others, "assignee:sam p:high"), ["ENG-4"]);
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
                let title = title_spans(issue, &[], true, budget, app.theme);
                let width: usize = title.iter().map(|s| s.content.chars().count()).sum();
                assert!(width <= budget, "title at {w}");
            }
        }
    }

    /// The reading pane lists the issue's properties as Linear's sidebar
    /// does — in a column of their own when it is wide, under the title
    /// when it is not — and wraps a long title rather than cutting it.
    #[test]
    fn the_reading_pane_lists_properties_beside_or_under_the_text() {
        let mut long = rich(
            "1",
            "ENG-1",
            "A title long enough that a narrow reading pane has to wrap it onto more lines",
            ("In Progress", "started"),
            2,
        );
        long.assignee = "Sam".into();
        long.description = "Body text.".into();
        let mut app = view_on(vec![long]);
        let wide = shot(&mut app, 220, 40);
        let row = |screen: &str, needle: &str| {
            screen
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no {needle}\n{screen}"))
                .to_string()
        };
        let status = row(&wide, "Status");
        assert!(status.contains("In Progress"), "{wide}");
        assert!(row(&wide, "Priority").contains("High"), "{wide}");
        assert!(row(&wide, "Labels").contains("● Export PDF"), "{wide}");
        assert!(row(&wide, "Assignee").contains("Sam"), "{wide}");
        // Beside the text: the title's first line and Status share a row.
        assert!(status.contains("││ A title long"), "{wide}");

        let narrow = shot(&mut app, 110, 40);
        let status = row(&narrow, "Status");
        assert!(
            status.contains("││ Status"),
            "stacked under the title\n{narrow}"
        );
        assert!(
            narrow.contains("it onto more lines"),
            "the title wraps\n{narrow}"
        );
        let title_at = narrow.lines().position(|l| l.contains("A title")).unwrap();
        let body_at = narrow
            .lines()
            .position(|l| l.contains("Body text."))
            .unwrap();
        let status_at = narrow.lines().position(|l| l.contains("Status")).unwrap();
        assert!(title_at < status_at && status_at < body_at, "{narrow}");
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

    /// Land every answer Linear sends until `done` says the flow is over.
    fn run_linear(
        app: &mut App,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<LinearAnswer>,
        act: impl FnOnce(&mut App),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
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
}
