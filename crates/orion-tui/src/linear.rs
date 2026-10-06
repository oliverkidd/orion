//! The LINEAR VIEW (`⌘L`): open Linear issues assigned to you, picked
//! together so one agent fixes them in one worktree and opens one pull
//! request. From the PULL REQUESTS MODAL the same list attaches a pull
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
use crate::markdown::{self, Breaks};
use crate::pr_modal::PullRequestsView;
use crate::quick_prompt::{ModalUnder, QuickLaunch, QuickReturn, QuickTarget};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, empty_list_row, fuzzy_highlight_styled, panel_block, render_row, row_rect,
    search_line, truncate, visible_positions, SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
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

/// One open Linear issue assigned to the configured user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinearIssue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub description: String,
    /// Linear's word for the priority — `Urgent`, `High`, … — or
    /// `No priority`; empty when Linear did not say.
    #[serde(default)]
    pub priority: String,
    pub status: String,
    #[serde(default)]
    pub status_type: String,
    /// The team the issue belongs to: whose workflow states it can move
    /// to (`LinearList::states`).
    #[serde(default)]
    pub team_id: String,
}

/// One of a team's workflow states — `Todo`, `In Progress`, `Done` —
/// with Linear's word for its kind (`unstarted`, `started`, `completed`,
/// …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinearState {
    pub id: String,
    pub name: String,
    pub kind: String,
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
            let facts: Vec<&str> = [i.status.as_str(), i.priority.as_str(), i.url.as_str()]
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

/// Browse assigned issues, or attach the current pull request to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearMode {
    Browse,
    Attach {
        pr_url: String,
        pr_number: u64,
        back: Box<PullRequestsView>,
    },
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
    pub cursor_row: usize,
    pub marked: BTreeSet<String>,
    pub mode: LinearMode,
    /// The status picker, while it is up: every key but Esc is its own.
    pub status_pick: Option<StatusPick>,
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
            cursor_row: 0,
            marked: BTreeSet::new(),
            mode,
            status_pick: None,
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

/// What Linear last said about a project's assigned issues, and the
/// workflow states of the teams they belong to, by team id, in Linear's
/// own order.
#[derive(Debug, Clone, Default)]
pub struct LinearList {
    pub list: Vec<LinearIssue>,
    pub states: HashMap<String, Vec<LinearState>>,
}

/// What [`fetch_assigned`] reads: the issues, and their teams' states.
type Assigned = (Vec<LinearIssue>, HashMap<String, Vec<LinearState>>);

/// A finished Linear call, back on the loop.
#[derive(Debug, Clone)]
pub enum LinearAnswer {
    List {
        project: ProjectId,
        list: Result<Assigned, String>,
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
}

/// Why Linear refused to move an issue, and the state it was in before
/// the row said otherwise — what [`land_answer`] puts back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRefused {
    pub why: String,
    pub status: String,
    pub status_type: String,
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
    let Some(pr) = selected_open_pr(app) else {
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

fn selected_open_pr(app: &App) -> Option<crate::pull_request::OpenPr> {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return None;
    };
    let list = app
        .open_prs
        .get(&view.project)
        .map(|o| o.list.as_slice())
        .unwrap_or(&[]);
    if list.is_empty() {
        return None;
    }
    let labels: Vec<String> = list.iter().map(|pr| pr.label()).collect();
    let i = if view.query.split_whitespace().next().is_none() {
        clamp_selection(view.selected as i64, list.len())
    } else {
        let ranked = crate::fuzzy::rank(view.query.as_str(), labels.iter().map(String::as_str));
        ranked
            .iter()
            .find(|(i, _)| *i == view.selected)
            .or(ranked.first())
            .map(|(i, _)| *i)?
    };
    list.get(i).cloned()
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
        let result = fetch_assigned(&dir, &email).await;
        let _ = tx.send(LinearAnswer::List {
            project,
            list: result,
        });
    });
}

pub(crate) fn land_answer(app: &mut App, answer: LinearAnswer) {
    match answer {
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
                Ok((list, states)) => {
                    let n = list.len();
                    app.linear_failed.remove(&project);
                    app.linear
                        .insert(project.clone(), LinearList { list, states });
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
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        MARK, CONFIRM, PRESET, BROWSER, REFRESH, STATUS, ATTACH, PICK, SET,
    ];
}

/// The keys along the modal's bottom edge, for browsing or for picking
/// the issues a pull request attaches to. Esc clears a typed filter first.
pub(crate) fn hints(view: &LinearView) -> Vec<crate::hints::Hint> {
    use crate::hints::Hint;
    let esc = if !view.query.is_empty() {
        "clear"
    } else if matches!(view.mode, LinearMode::Attach { .. }) {
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
    match view.mode {
        LinearMode::Browse => vec![
            keys::MARK.hint(),
            keys::CONFIRM.hint().kept(),
            keys::ATTACH.hint(),
            keys::STATUS.hint(),
            keys::PRESET.hint(),
            keys::BROWSER.hint(),
            keys::REFRESH.hint(),
            Hint::new("Esc", esc),
        ],
        LinearMode::Attach { .. } => vec![
            keys::MARK.hint(),
            keys::CONFIRM.hint_as("attach marked to this PR").kept(),
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
    let (status, status_type) = (issue.status.clone(), issue.status_type.clone());
    issue.status = state.name.clone();
    issue.status_type = state.kind.clone();
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

fn row_under(app: &App, pos: Position) -> Option<usize> {
    let Overlay::Linear(view) = app.overlay.as_ref()? else {
        return None;
    };
    let list = rows(app, &view.project);
    let visible = visible_rows(&view.query, list);
    let start = window_start(view.cursor_row, view.list_area.height as usize);
    let y = pos.y.checked_sub(view.list_area.y)? as usize;
    visible.get(start + y).map(|(i, _)| *i)
}

fn close(app: &mut App) {
    let Some(Overlay::Linear(view)) = app.overlay.take() else {
        return;
    };
    if let LinearMode::Attach { back, .. } = view.mode {
        crate::pr_modal::reopen(app, *back);
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
    let visible = visible_rows(&view.query, list);
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
    if matches!(view.mode, LinearMode::Attach { .. }) {
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

fn has_query(view: &LinearView) -> bool {
    view.query.split_whitespace().next().is_some()
}

fn visible_rows(query: &TextInput, list: &[LinearIssue]) -> Vec<(usize, Vec<usize>)> {
    let labels: Vec<String> = list.iter().map(|i| i.label()).collect();
    crate::fuzzy::rank(query.as_str(), labels.iter().map(String::as_str))
}

fn cursor_index(view: &LinearView, list: &[LinearIssue]) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    if !has_query(view) {
        return Some(clamp_selection(view.selected as i64, list.len()));
    }
    let visible = visible_rows(&view.query, list);
    if visible.iter().any(|(i, _)| *i == view.selected) {
        Some(view.selected)
    } else {
        visible.first().map(|(i, _)| *i)
    }
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
    let visible = visible_rows(&view.query, &issues);
    let cursor = cursor_index(view, &issues);
    let cursor_row = cursor
        .and_then(|c| visible.iter().position(|(i, _)| *i == c))
        .unwrap_or(0);

    let count = if has_query(view) {
        format!("{}/{}", visible.len(), issues.len())
    } else {
        issues.len().to_string()
    };
    let marked = view.marked.len();
    let head = match &view.mode {
        LinearMode::Browse => format!("Linear — {} ({count})", view.project_name),
        LinearMode::Attach { pr_number, .. } => {
            format!("Linear → PR #{pr_number} — {} ({count})", view.project_name)
        }
    };
    let title = if inflight {
        format!("{head}, refreshing…")
    } else if marked > 0 {
        format!("{head}, {marked} marked")
    } else {
        head
    };
    let block = panel_block(&title, list_focused, th);
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    if let Some(query_area) = row_rect(list_inner, 0) {
        let line = search_line(&view.query, "type to filter…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let rows_area = crate::ui::below_first_row(list_inner);
    if issues.is_empty() {
        let text = if failed {
            "couldn't list Linear issues — check LINEAR_API_KEY and Settings → Linear account"
        } else if inflight || app.linear_tx.is_some() && !app.linear.contains_key(&view.project) {
            "asking Linear…"
        } else {
            "no open issues assigned to you"
        };
        empty_list_row(f, rows_area, text, th);
    } else if visible.is_empty() {
        empty_list_row(f, rows_area, "no issues match", th);
    }
    let start = window_start(cursor_row, rows_area.height as usize);
    for (row, (index, positions)) in visible.iter().enumerate().skip(start) {
        let Some(row_area) = row_rect(rows_area, row - start) else {
            break;
        };
        let issue = &issues[*index];
        // A marked row is ticked in the accent — it is a choice the keys
        // made, not a status, so not the `●` a session's STATUS MARK is.
        let marked = view.marked.contains(&issue.id);
        let tick = if marked { "✓ " } else { "  " };
        let full = format!("{tick}{}", issue.label());
        // The status, behind the mark of where it stands — `◑` started,
        // `○` not yet, `◌` in the backlog — quieter the further off it is.
        let (state_mark, state_color) = match issue.status_type.as_str() {
            "started" => ("◑ ", th.muted),
            "unstarted" => ("○ ", th.dim),
            "backlog" => ("◌ ", th.faint),
            _ => ("", th.dim),
        };
        let status = if issue.status.is_empty() {
            String::new()
        } else {
            format!("{state_mark}{}", issue.status)
        };
        let budget = (rows_area.width as usize).saturating_sub(2);
        let status_w = status.chars().count();
        let text_budget = budget.saturating_sub(if status_w > 0 { status_w + 2 } else { 0 });
        let label = truncate(&full, text_budget);
        let pos = visible_positions(positions, &label, &full);
        let used = label.chars().count();
        let mut spans = fuzzy_highlight_styled(&label, pos, Style::default(), th);
        if marked {
            // The tick alone takes the accent; the label after it keeps
            // its own style and highlights.
            if let Some(first) = spans.first().cloned() {
                if let Some(rest) = first.content.strip_prefix("✓ ") {
                    let rest = rest.to_string();
                    spans[0] = Span::styled(
                        "✓ ",
                        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                    );
                    if !rest.is_empty() {
                        spans.insert(1, Span::styled(rest, first.style));
                    }
                }
            }
        }
        if status_w > 0 && used + status_w < budget {
            spans.push(Span::raw(" ".repeat(budget - used - status_w)));
            spans.push(Span::styled(status, Style::default().fg(state_color)));
        }
        render_row(f, row_area, spans, Some(*index) == cursor, list_focused, th);
    }

    let current = cursor.and_then(|i| issues.get(i));
    let body_title = current
        .map(|i| i.identifier.clone())
        .unwrap_or_else(|| "Linear".into());
    let width = body_a.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = match current {
        Some(issue) => body_lines(issue, width, th),
        None => Vec::new(),
    };
    let mut block = panel_block(&body_title, false, th);
    let body_inner = block.inner(body_a);
    let max_scroll = (lines.len() as u16).saturating_sub(body_inner.height.max(1));
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
    if let Some(pick) = &view.status_pick {
        draw_status_pick(f, body_inner, pick, th);
    } else {
        let shown: Vec<Line> = lines.iter().skip(scroll as usize).cloned().collect();
        f.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), body_inner);
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
        v.cursor_row = cursor_row;
        v.body_area = body_inner;
        v.browser_area = browser_area;
        v.view_height = body_inner.height;
        v.body_lines = lines.len();
        if let Some(index) = cursor {
            v.selected = index;
        }
        v.scroll = scroll;
    }
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

fn body_lines(issue: &LinearIssue, width: usize, th: Theme) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            issue.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("{} · {}", issue.status, issue.url),
            Style::default().fg(th.dim),
        )),
        Line::from(""),
    ];
    if issue.description.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "(no description)",
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

/// What each listed issue is read with: the row, the reading pane, and
/// its team's workflow states for `⌘S` — the same fields whoever's
/// issues are asked for.
const ISSUE_FIELDS: &str = "id identifier title url description priorityLabel state { name type } \
    team { id states { nodes { id name type position } } }";

async fn fetch_assigned(dir: &Path, email: &str) -> Result<Assigned, String> {
    let key = read_linear_key(dir).ok_or_else(|| NO_KEY.to_string())?;
    let (query, variables) = if email.is_empty() {
        (
            format!(
                r#"query {{
              viewer {{
                assignedIssues(first: 100, filter: {{ state: {{ type: {{ nin: ["completed", "canceled"] }} }} }}) {{
                  nodes {{ {ISSUE_FIELDS} }}
                }}
              }}
            }}"#
            ),
            serde_json::json!({}),
        )
    } else {
        (
            format!(
                r#"query($email: String!) {{
              issues(first: 100, filter: {{
                assignee: {{ email: {{ eq: $email }} }}
                state: {{ type: {{ nin: ["completed", "canceled"] }} }}
              }}) {{
                nodes {{ {ISSUE_FIELDS} }}
              }}
            }}"#
            ),
            serde_json::json!({ "email": email }),
        )
    };
    let json = graphql(&key, &query, variables).await?;
    let issues = parse_issues(&json, email.is_empty())?;
    Ok((issues, parse_states(&json, email.is_empty())))
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

/// The issues' nodes in a [`fetch_assigned`] answer: the viewer's own, or
/// an assignee's by email.
fn issue_nodes(json: &serde_json::Value, viewer: bool) -> Option<&Vec<serde_json::Value>> {
    let at = if viewer {
        "/data/viewer/assignedIssues/nodes"
    } else {
        "/data/issues/nodes"
    };
    json.pointer(at)?.as_array()
}

/// Each team's workflow states, from the issues' `team` fields, in
/// Linear's `position` order.
fn parse_states(json: &serde_json::Value, viewer: bool) -> HashMap<String, Vec<LinearState>> {
    let mut out: HashMap<String, Vec<LinearState>> = HashMap::new();
    for team in issue_nodes(json, viewer)
        .into_iter()
        .flatten()
        .filter_map(|n| n.get("team"))
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

fn parse_issues(json: &serde_json::Value, viewer: bool) -> Result<Vec<LinearIssue>, String> {
    if let Some(err) = graphql_error(json) {
        return Err(err);
    }
    let Some(nodes) = issue_nodes(json, viewer) else {
        return Err("Linear returned no issue list".into());
    };
    let mut issues: Vec<LinearIssue> = nodes.iter().filter_map(issue_from).collect();
    issues.sort_by(|a, b| {
        status_rank(&a.status_type)
            .cmp(&status_rank(&b.status_type))
            .then_with(|| a.status.cmp(&b.status))
            .then_with(|| a.identifier.cmp(&b.identifier))
    });
    Ok(issues)
}

fn issue_from(value: &serde_json::Value) -> Option<LinearIssue> {
    Some(LinearIssue {
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
        priority: value
            .get("priorityLabel")
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
            priority: String::new(),
            status: "In Progress".into(),
            status_type: "started".into(),
            team_id: "t1".into(),
        }
    }

    /// The teams' states ride the issue list, in Linear's order; `⌘S`
    /// lists them on the issue's own state, Enter moves the issue there —
    /// the row says so before Linear answers, and a refusal puts it back.
    #[test]
    fn ctrl_s_moves_an_issue_to_another_state() {
        let json = serde_json::json!({"data": {"viewer": {"assignedIssues": {"nodes": [
            {"id": "1", "identifier": "ENG-12", "title": "Login", "url": "https://linear.app/x/issue/ENG-12",
             "state": {"name": "In Progress", "type": "started"},
             "team": {"id": "t1", "states": {"nodes": [
                {"id": "s3", "name": "Done", "type": "completed", "position": 3.0},
                {"id": "s1", "name": "Todo", "type": "unstarted", "position": 1.0},
                {"id": "s2", "name": "In Progress", "type": "started", "position": 2.0}
             ]}}}
        ]}}}});
        let issues = parse_issues(&json, true).unwrap();
        assert_eq!(issues[0].team_id, "t1");
        let states = parse_states(&json, true);
        let names: Vec<&str> = states["t1"].iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Todo", "In Progress", "Done"]);

        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.linear.insert(
            project.clone(),
            LinearList {
                list: issues,
                states,
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
                "viewer": {
                    "assignedIssues": {
                        "nodes": [{
                            "id": "abc",
                            "identifier": "ENG-1",
                            "title": "T",
                            "url": "https://linear.app/x/issue/ENG-1",
                            "description": "d",
                            "priorityLabel": "High",
                            "state": { "name": "Todo", "type": "unstarted" }
                        }]
                    }
                }
            }
        });
        let list = parse_issues(&json, true).unwrap();
        assert_eq!(list[0].identifier, "ENG-1");
        assert_eq!(list[0].status_type, "unstarted");
        assert_eq!(list[0].priority, "High");
    }
}
