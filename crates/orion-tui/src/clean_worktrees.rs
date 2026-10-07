//! **Clean unused worktrees** (a COMMAND PALETTE row): every linked
//! worktree, across every project, that nothing is using — no live agent,
//! no terminal, no uncommitted or untracked file — in one modal, grouped by
//! project, every row ticked; Enter deletes the ticked ones.
//!
//! The session half of "unused" is read off the tree as the modal opens;
//! the files half is a `git status --porcelain` per checkout, the count the
//! DAEMON reads before an unforced delete (`git::changed_files`), run off
//! the loop with the rest of each row's git — its branch, whether that
//! branch is merged or pushed, its last commit — and landing on
//! `App::clean_worktrees.tx` to fill the modal. The deletes are unforced,
//! so a checkout written to after the check is kept and asked about, never
//! lost. Archived sessions don't count as use.
//!
//! With `b` on, each ticked row's local branch goes too — only one whose
//! every commit is on a remote (MERGED into the base branch, or PUSHED),
//! re-checked just before `git branch -D`, which runs once the daemon says
//! the checkout is gone ([`removed`]): git won't delete a branch a
//! worktree has checked out. A branch with UNPUSHED commits is always
//! kept. Remote branches are never touched.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::protocol::ClientRequest;
use orion_core::{ProjectId, Worktree, WorktreeId};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::bundle::plural;
use crate::flash::Flash;
use crate::theme::Theme;

// ---- the check ----

/// What deleting a row's branch would lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchState {
    /// Every commit is in the base branch.
    Merged,
    /// Every commit is on a remote, not all of them in the base branch.
    Pushed,
    /// This many commits are on no remote: the branch is always kept.
    Unpushed(usize),
    /// No branch to speak of — a detached HEAD, the base branch itself, a
    /// checkout gone from disk: nothing of it is deleted.
    None,
}

impl BranchState {
    fn deletable(&self) -> bool {
        matches!(self, BranchState::Merged | BranchState::Pushed)
    }

    fn label(&self) -> String {
        match self {
            BranchState::Merged => "merged".into(),
            BranchState::Pushed => "pushed".into(),
            BranchState::Unpushed(n) => format!("{n} unpushed"),
            BranchState::None => "—".into(),
        }
    }
}

/// One checkout the check found nothing changed in.
#[derive(Debug, Clone)]
pub struct Found {
    pub id: WorktreeId,
    /// The branch HEAD is on, when it is on one.
    pub branch: Option<String>,
    pub state: BranchState,
    /// Epoch seconds of HEAD's commit.
    pub committed_at: Option<i64>,
}

/// What lands on `App::clean_worktrees.tx`.
#[derive(Debug)]
pub enum Answer {
    /// The check: the checkouts git found clean, of those asked about.
    Checked { found: Vec<Found> },
    /// A branch delete after its checkout went; `Err` says why it didn't.
    BranchDeleted {
        branch: String,
        result: Result<(), String>,
    },
}

/// What outlives the modal.
#[derive(Default)]
pub struct Shared {
    /// Installed at startup like `git_sync.tx`. `None` in the unit tests,
    /// which check inline.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    /// Branches to delete once the daemon removes their checkout: the
    /// repository to run git in, and the branch.
    pub branch_after: HashMap<WorktreeId, (PathBuf, String)>,
    /// Branches deleted since the last Enter, for the flash.
    pub branches_deleted: usize,
}

/// "1 branch" / "3 branches".
fn branches(n: usize) -> String {
    format!("{n} branch{}", if n == 1 { "" } else { "es" })
}

/// Linked worktrees with no live agent and no terminal, in tree order. The
/// ROOT WORKTREE never; nor a stand-in git is still cutting.
pub(crate) fn idle(app: &App) -> Vec<&Worktree> {
    let tree = &app.tree;
    let busy: HashSet<&WorktreeId> = tree
        .agents
        .iter()
        .filter(|a| !a.archived)
        .map(|a| &a.worktree_id)
        .chain(tree.terminals.iter().map(|t| &t.worktree_id))
        .collect();
    tree.worktrees
        .iter()
        .filter(|w| !w.is_main && !busy.contains(&w.id) && !app.is_placeholder_worktree(&w.id))
        .collect()
}

/// Commits on `rev` that no remote-tracking branch has.
fn unpushed(root: &Path, rev: &str) -> Option<usize> {
    crate::git_proc::read(root, &["rev-list", "--count", rev, "--not", "--remotes"])
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The checkout at `path`, read: `None` when it has changes, or git can't
/// read it. One already gone from disk is clean — its delete only drops
/// git's bookkeeping — with no branch to delete.
fn inspect(id: WorktreeId, path: &Path, base_setting: &str) -> Option<Found> {
    use crate::git_proc::read;
    let gone = Found {
        id,
        branch: None,
        state: BranchState::None,
        committed_at: None,
    };
    if !path.exists() {
        return Some(gone);
    }
    let status = read(path, &["status", "--porcelain"]).ok()?;
    if status.lines().any(|l| !l.is_empty()) {
        return None;
    }
    let committed_at = read(path, &["log", "-1", "--format=%ct"])
        .ok()
        .and_then(|s| s.trim().parse().ok());
    let base = crate::commit_list::resolve_base_cached(path, base_setting);
    let branch = crate::git_proc::head_branch(path);
    let state = match &branch {
        // The base branch itself is never one to clean away.
        Some(b)
            if base
                .as_deref()
                .is_some_and(|base| base.ends_with(&format!("/{b}"))) =>
        {
            BranchState::None
        }
        Some(_) => match unpushed(path, "HEAD") {
            Some(0) => {
                let merged = base.is_some_and(|base| {
                    read(path, &["merge-base", "--is-ancestor", "HEAD", &base]).is_ok()
                });
                if merged {
                    BranchState::Merged
                } else {
                    BranchState::Pushed
                }
            }
            Some(n) => BranchState::Unpushed(n),
            None => BranchState::None,
        },
        None => BranchState::None,
    };
    Some(Found {
        branch,
        state,
        committed_at,
        ..gone
    })
}

/// Read every candidate at once; the clean ones, in the order given.
fn check(candidates: Vec<(WorktreeId, PathBuf)>, base_setting: &str) -> Vec<Found> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = candidates
            .into_iter()
            .map(|(id, path)| scope.spawn(move || inspect(id, &path, base_setting)))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    })
}

// ---- the modal ----

/// One worktree row.
#[derive(Debug, Clone)]
pub struct Row {
    pub id: WorktreeId,
    pub project: ProjectId,
    pub name: String,
    pub branch: Option<String>,
    pub state: BranchState,
    pub committed_at: Option<i64>,
    /// Ticked: Enter deletes it.
    pub on: bool,
}

/// A line of the list: a project's header, or a worktree under it.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Project(ProjectId),
    Worktree(usize),
}

#[derive(Debug, Clone, Default)]
pub struct CleanView {
    /// How many checkouts are being read; 0 once the check has landed.
    pub checking: usize,
    /// Grouped by project, in tree order.
    pub rows: Vec<Row>,
    /// Index into the list's lines (headers included).
    pub cursor: usize,
    /// `b`: delete merged and pushed branches with their worktrees.
    pub branches: bool,
    pub area: Rect,
    pub list_area: Rect,
    /// The branches switch's line, for a click.
    pub branches_line: Rect,
    /// The line drawn at the list's top: it scrolls to keep the cursor in view.
    pub first: usize,
}

impl CleanView {
    fn items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            if i == 0 || self.rows[i - 1].project != r.project {
                items.push(Item::Project(r.project.clone()));
            }
            items.push(Item::Worktree(i));
        }
        items
    }

    fn chosen(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.on)
    }

    /// The ticked rows whose branch goes with them.
    fn doomed_branches(&self) -> impl Iterator<Item = &Row> {
        self.chosen()
            .filter(|r| self.branches && r.state.deletable() && r.branch.is_some())
    }

    /// Tick or untick line `at`: a header does its whole project — all on
    /// unless every one already is.
    fn toggle(&mut self, at: usize) {
        match self.items().get(at) {
            Some(Item::Worktree(i)) => self.rows[*i].on = !self.rows[*i].on,
            Some(Item::Project(p)) => {
                let all = self.rows.iter().filter(|r| &r.project == p).all(|r| r.on);
                for r in self.rows.iter_mut().filter(|r| &r.project == p) {
                    r.on = !all;
                }
            }
            None => {}
        }
    }

    fn toggle_all(&mut self) {
        let all = self.rows.iter().all(|r| r.on);
        for r in &mut self.rows {
            r.on = !all;
        }
    }
}

pub(crate) mod keys {
    use crate::hints::Key;

    pub const TICK: Key = Key::new(&["space"], "tick");
    pub const ALL: Key = Key::new(&["a"], "all");
    pub const BRANCHES: Key = Key::new(&["b"], "branches");
    pub const DELETE: Key = Key::new(&["enter"], "delete");
    pub const CLOSE: Key = Key::new(&["esc", "q"], "close");
    #[cfg(test)]
    pub const EVERY: &[Key] = &[TICK, ALL, BRANCHES, DELETE, CLOSE];
}

/// The palette row: open the modal on the idle worktrees and read them off
/// the loop, or say there are none.
pub(crate) fn start(app: &mut App) {
    let candidates: Vec<(WorktreeId, PathBuf)> = idle(app)
        .into_iter()
        .map(|w| (w.id.clone(), w.path.clone()))
        .collect();
    app.dirty = true;
    if candidates.is_empty() {
        app.flash = Some(Flash::note(
            "no unused worktrees: every linked worktree has a session",
        ));
        return;
    }
    app.overlay = Some(Overlay::CleanWorktrees(CleanView {
        checking: candidates.len(),
        ..CleanView::default()
    }));
    let base = crate::config::Config::load().worktree_base_branch;
    let Some(tx) = app.clean_worktrees.tx.clone() else {
        let found = check(candidates, &base);
        land(app, Answer::Checked { found });
        return;
    };
    tokio::task::spawn_blocking(move || {
        let _ = tx.send(Answer::Checked {
            found: check(candidates, &base),
        });
    });
}

/// An answer landed: the check fills the modal, if it is still up; a
/// branch delete counts toward the flash, or says why it failed.
pub(crate) fn land(app: &mut App, answer: Answer) {
    app.dirty = true;
    match answer {
        Answer::Checked { found } => {
            // A session started in one while git was reading keeps it.
            let still_idle: Vec<Worktree> = idle(app).into_iter().cloned().collect();
            let order: HashMap<ProjectId, usize> = app
                .tree
                .projects
                .iter()
                .enumerate()
                .map(|(i, p)| (p.id.clone(), i))
                .collect();
            let Some(Overlay::CleanWorktrees(view)) = &mut app.overlay else {
                return;
            };
            if view.checking == 0 {
                return;
            }
            let found: HashMap<WorktreeId, Found> =
                found.into_iter().map(|f| (f.id.clone(), f)).collect();
            let mut rows: Vec<Row> = still_idle
                .into_iter()
                .filter_map(|w| {
                    let f = found.get(&w.id)?.clone();
                    Some(Row {
                        id: w.id,
                        project: w.project_id,
                        name: w.branch,
                        branch: f.branch,
                        state: f.state,
                        committed_at: f.committed_at,
                        on: true,
                    })
                })
                .collect();
            // Grouped by project, in the tree's project order.
            rows.sort_by_key(|r| order.get(&r.project).copied().unwrap_or(usize::MAX));
            view.rows = rows;
            view.checking = 0;
            // The first worktree, past its project's header.
            view.cursor = usize::from(!view.rows.is_empty());
        }
        Answer::BranchDeleted { branch, result } => match result {
            Ok(()) => {
                app.clean_worktrees.branches_deleted += 1;
                app.flash = Some(Flash::done(format!(
                    "deleted {}",
                    branches(app.clean_worktrees.branches_deleted)
                )));
            }
            Err(why) => {
                app.flash = Some(Flash::failed(format!("kept branch {branch}: {why}")));
            }
        },
    }
}

/// Enter: delete the ticked worktrees, and note the branches that go once
/// their checkouts have.
fn delete(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::CleanWorktrees(view)) = &app.overlay else {
        return;
    };
    let ids: Vec<WorktreeId> = view.chosen().map(|r| r.id.clone()).collect();
    if ids.is_empty() {
        return;
    }
    let branches: Vec<(WorktreeId, ProjectId, String)> = view
        .doomed_branches()
        .filter_map(|r| Some((r.id.clone(), r.project.clone(), r.branch.clone()?)))
        .collect();
    for (id, project, branch) in &branches {
        if let Some(p) = app.tree.projects.iter().find(|p| &p.id == project) {
            app.clean_worktrees
                .branch_after
                .insert(id.clone(), (p.repo_path.clone(), branch.clone()));
        }
    }
    app.clean_worktrees.branches_deleted = 0;
    app.overlay = None;
    app.flash = Some(Flash::working(if branches.is_empty() {
        format!("deleting {}…", plural(ids.len(), "worktree"))
    } else {
        format!(
            "deleting {} and {}…",
            plural(ids.len(), "worktree"),
            self::branches(branches.len())
        )
    }));
    crate::event_loop::delete_worktrees(app, ids, out);
}

/// The daemon removed worktree `id`: when its branch was to go with it,
/// delete that now, off the loop — if it still has nothing unpushed.
pub(crate) fn removed(app: &mut App, id: &WorktreeId) {
    let Some((repo, branch)) = app.clean_worktrees.branch_after.remove(id) else {
        return;
    };
    let Some(tx) = app.clean_worktrees.tx.clone() else {
        return;
    };
    tokio::task::spawn_blocking(move || {
        let rev = format!("refs/heads/{branch}");
        let result = match unpushed(&repo, &rev) {
            Some(0) => crate::git_proc::run(&repo, &["branch", "-D", &branch]).map(|_| ()),
            Some(n) => Err(format!("{} on no remote", plural(n, "commit"))),
            None => Err("git couldn't read it".into()),
        };
        let _ = tx.send(Answer::BranchDeleted { branch, result });
    });
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::CleanWorktrees(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    let lines = view.items().len();
    match key.code {
        _ if keys::CLOSE.matches(&key) => app.overlay = None,
        KeyCode::Char('j') | KeyCode::Down => {
            view.cursor = crate::app::clamp_selection(view.cursor as i64 + 1, lines);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            view.cursor = crate::app::clamp_selection(view.cursor as i64 - 1, lines);
        }
        _ if keys::TICK.matches(&key) => view.toggle(view.cursor),
        _ if keys::ALL.matches(&key) => view.toggle_all(),
        _ if keys::BRANCHES.matches(&key) => view.branches = !view.branches,
        _ if keys::DELETE.matches(&key) => delete(app, out),
        _ => {}
    }
}

/// A click on a line ticks it, as `space` there would; one on the
/// branches switch flips it.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) {
    let Some(Overlay::CleanWorktrees(view)) = &mut app.overlay else {
        return;
    };
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }
    app.dirty = true;
    if view.branches_line.contains(pos) {
        view.branches = !view.branches;
        return;
    }
    if let Some(at) = crate::list_hit::row_at(view.list_area, view.first, view.items().len(), pos) {
        view.cursor = at;
        view.toggle(at);
    }
}

const WIDEST: u16 = 84;
const BRANCH_W: usize = 20;
const AGE_W: usize = 5;
/// The ` [x] ` each line starts with.
const BOX_W: usize = 5;
/// Lines under the list: a blank, the switch, the summary (two).
const FOOT_H: u16 = 4;

fn tick(on: Option<bool>) -> &'static str {
    match on {
        Some(true) => " [x] ",
        Some(false) => " [ ] ",
        None => " [-] ",
    }
}

/// What Enter does, said plainly, and the branches it keeps.
fn summary(view: &CleanView) -> (String, Option<String>) {
    let chosen: Vec<&Row> = view.chosen().collect();
    if chosen.is_empty() {
        return (" Nothing ticked.".into(), None);
    }
    let first = if view.branches {
        format!(
            " Deletes {} and {}.",
            plural(chosen.len(), "worktree"),
            branches(view.doomed_branches().count())
        )
    } else {
        format!(
            " Deletes {}. Branches are kept.",
            plural(chosen.len(), "worktree")
        )
    };
    let kept: Vec<&str> = chosen
        .iter()
        .filter(|r| view.branches && matches!(r.state, BranchState::Unpushed(_)))
        .map(|r| r.name.as_str())
        .collect();
    let second = (!kept.is_empty())
        .then(|| format!(" Keeps {}'s branch: unpushed commits.", kept.join(", ")));
    (first, second)
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &CleanView, th: Theme) {
    let screen = f.area();
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let inner_w = usize::from(width.saturating_sub(2));
    // Worktree rows indent their box two cells under the header's.
    let name_w = inner_w.saturating_sub(2 + BOX_W + BRANCH_W + 1 + AGE_W);
    let dim = Style::default().fg(th.dim);
    let items = view.items();
    let now = crate::app::now_ms() / 1000;

    let mut lines: Vec<Line> = Vec::new();
    if view.checking > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                " Checking {} for changes…",
                plural(view.checking, "worktree")
            ),
            dim,
        )));
    } else if view.rows.is_empty() {
        lines.push(Line::from(Span::styled(
            " Nothing to clean: every idle worktree has uncommitted changes.",
            dim,
        )));
    }
    let fits = usize::from(screen.height.saturating_sub(4 + FOOT_H).max(1));
    let cursor = view.cursor.min(items.len().saturating_sub(1));
    let first = view
        .first
        .min(cursor)
        .max((cursor + 1).saturating_sub(fits));
    for (i, item) in items.iter().enumerate().skip(first).take(fits) {
        let sel = |s: Style| {
            if i == cursor {
                s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                s
            }
        };
        let line = match item {
            Item::Project(p) => {
                let kids: Vec<&Row> = view.rows.iter().filter(|r| &r.project == p).collect();
                let on = kids.iter().filter(|r| r.on).count();
                let state = match on {
                    0 => Some(false),
                    n if n == kids.len() => Some(true),
                    _ => None,
                };
                let name = app
                    .tree
                    .projects
                    .iter()
                    .find(|x| &x.id == p)
                    .map_or("?", |x| x.name.as_str());
                let text = format!("{name} ");
                let n = format!("{on}/{}", kids.len());
                let pad = inner_w.saturating_sub(BOX_W + text.chars().count() + n.chars().count());
                Line::from(vec![
                    Span::styled(tick(state), sel(Style::default().fg(th.muted))),
                    Span::styled(
                        text,
                        sel(Style::default().fg(th.text).add_modifier(Modifier::BOLD)),
                    ),
                    Span::styled(format!("{n}{}", " ".repeat(pad)), sel(dim)),
                ])
            }
            Item::Worktree(r) => {
                let r = &view.rows[*r];
                let (label, color) = match (&r.state, view.branches && r.on) {
                    (BranchState::Unpushed(_), true) => {
                        (format!("keep · {}", r.state.label()), th.warn)
                    }
                    (s, true) if s.deletable() => (format!("delete · {}", s.label()), th.err),
                    (BranchState::Unpushed(_), false) => (r.state.label(), th.warn),
                    (s, _) => (s.label(), th.dim),
                };
                let age = r
                    .committed_at
                    .map(|t| crate::hosts::ago_short(now - t))
                    .unwrap_or_default();
                Line::from(vec![
                    Span::styled(
                        format!("  {}", tick(Some(r.on))),
                        sel(Style::default().fg(if r.on { th.err } else { th.muted })),
                    ),
                    Span::styled(
                        format!("{:<name_w$}", crate::ui::truncate(&r.name, name_w)),
                        sel(Style::default().fg(if r.on { th.text } else { th.dim })),
                    ),
                    Span::styled(
                        format!("{:<BRANCH_W$} ", crate::ui::truncate(&label, BRANCH_W)),
                        sel(Style::default().fg(color)),
                    ),
                    Span::styled(format!("{age:>AGE_W$}"), sel(dim)),
                ])
            }
        };
        lines.push(line);
    }
    let list_h = lines.len();

    lines.push(Line::from(""));
    let switch_at = lines.len();
    lines.push(Line::from(vec![
        Span::styled(
            tick(Some(view.branches)),
            Style::default().fg(if view.branches { th.err } else { th.muted }),
        ),
        Span::styled(
            "Delete merged and pushed branches too",
            Style::default().fg(th.text),
        ),
    ]));
    let (sum, kept) = summary(view);
    lines.push(Line::from(Span::styled(sum, Style::default().fg(th.muted))));
    if let Some(kept) = kept {
        lines.push(Line::from(Span::styled(kept, Style::default().fg(th.warn))));
    }

    let chosen = view.chosen().count();
    let hints = vec![
        keys::TICK.hint(),
        keys::ALL.hint(),
        keys::BRANCHES.hint(),
        if chosen == 0 {
            keys::DELETE.hint_as("nothing ticked")
        } else {
            keys::DELETE.hint_as(format!("delete {chosen}")).kept()
        },
        crate::hints::ESC_CLOSE.hint(),
    ];
    let height = (lines.len() as u16 + 2).min(screen.height.saturating_sub(2));
    let area = crate::ui::centered_rect(screen, width, height);
    let title = if view.rows.is_empty() {
        " Clean unused worktrees ".to_string()
    } else {
        format!(" Clean unused worktrees · {} idle ", view.rows.len())
    };
    let inner = crate::ui::render_modal_frame(f, area, title, &hints, th);
    f.render_widget(Paragraph::new(lines), inner);
    if let Some(Overlay::CleanWorktrees(v)) = &mut app.overlay {
        v.area = area;
        v.first = first;
        v.cursor = cursor;
        v.list_area = Rect {
            height: (if view.rows.is_empty() { 0 } else { list_h } as u16).min(inner.height),
            ..inner
        };
        v.branches_line = Rect {
            y: inner.y + switch_at as u16,
            height: 1,
            ..inner
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use orion_core::{Agent, AgentId, AgentKind, AgentStatus, Project};

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: ProjectId(id.into()),
            name: name.into(),
            repo_path: PathBuf::from("/nowhere"),
            sort_order: 0,
        }
    }

    fn worktree(id: &str, project: &str, branch: &str, is_main: bool) -> Worktree {
        Worktree {
            id: WorktreeId(id.into()),
            project_id: ProjectId(project.into()),
            // Gone from disk, so it checks clean without a repository.
            path: PathBuf::from(format!("/nonexistent/orion-test/{id}")),
            branch: branch.into(),
            is_main,
            sort_order: 0,
        }
    }

    fn agent(id: &str, worktree: &str, archived: bool) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId(worktree.into()),
            name: id.into(),
            status: AgentStatus::Finished,
            archived,
            archived_at: 0,
            unseen: false,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: !archived,
            issue_url: None,
            recent_prompts: Vec::new(),
            usage_limit: None,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.tree.projects = vec![project("p1", "orion"), project("p2", "riplo")];
        app.tree.worktrees = vec![
            worktree("root1", "p1", "main", true),
            worktree("busy", "p1", "busy-branch", false),
            worktree("idle2", "p2", "idle-two", false),
            worktree("idle1", "p1", "idle-one", false),
            worktree("old", "p2", "archived-only", false),
        ];
        app.tree.agents = vec![agent("a1", "busy", false), agent("a2", "old", true)];
        app
    }

    fn found(id: &str, state: BranchState) -> Found {
        Found {
            id: WorktreeId(id.into()),
            branch: Some(id.into()),
            state,
            committed_at: None,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn view(app: &App) -> &CleanView {
        match &app.overlay {
            Some(Overlay::CleanWorktrees(v)) => v,
            _ => panic!("modal not open"),
        }
    }

    /// The modal, opened and filled with `found`.
    fn opened(found: Vec<Found>) -> App {
        let mut app = app();
        app.overlay = Some(Overlay::CleanWorktrees(CleanView {
            checking: 3,
            ..CleanView::default()
        }));
        land(&mut app, Answer::Checked { found });
        app
    }

    fn three() -> Vec<Found> {
        vec![
            found("idle2", BranchState::Pushed),
            found("idle1", BranchState::Merged),
            found("old", BranchState::Unpushed(3)),
        ]
    }

    #[test]
    fn every_key_parses() {
        for k in keys::EVERY {
            assert!(k.parses(), "{:?}", k.chords);
        }
    }

    #[test]
    fn idle_skips_the_root_and_any_checkout_with_a_live_session() {
        let app = app();
        let idle: Vec<&str> = idle(&app).iter().map(|w| w.id.0.as_str()).collect();
        assert_eq!(idle, ["idle2", "idle1", "old"]);
    }

    #[test]
    fn rows_group_by_project_all_ticked() {
        let app = opened(three());
        let v = view(&app);
        let names: Vec<&str> = v.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["idle-one", "idle-two", "archived-only"]);
        assert!(v.rows.iter().all(|r| r.on));
        assert_eq!(
            v.items(),
            [
                Item::Project(ProjectId("p1".into())),
                Item::Worktree(0),
                Item::Project(ProjectId("p2".into())),
                Item::Worktree(1),
                Item::Worktree(2),
            ]
        );
        assert_eq!(v.cursor, 1);
    }

    #[test]
    fn dirty_checkouts_and_ones_that_got_a_session_are_left_out() {
        let mut app = app();
        app.overlay = Some(Overlay::CleanWorktrees(CleanView {
            checking: 3,
            ..CleanView::default()
        }));
        app.tree.agents.push(agent("a3", "idle2", false));
        // `old` is absent: git found changes in it.
        land(
            &mut app,
            Answer::Checked {
                found: vec![
                    found("idle2", BranchState::Pushed),
                    found("idle1", BranchState::Merged),
                ],
            },
        );
        let names: Vec<&str> = view(&app).rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["idle-one"]);
    }

    #[test]
    fn space_on_a_project_ticks_its_whole_group() {
        let mut app = opened(three());
        let mut out = Vec::new();
        handle_key(&mut app, key(KeyCode::Down), &mut out);
        handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
        let on: Vec<bool> = view(&app).rows.iter().map(|r| r.on).collect();
        assert_eq!(on, [true, false, false]);
        handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
        assert!(view(&app).rows.iter().all(|r| r.on));
        handle_key(&mut app, key(KeyCode::Char('a')), &mut out);
        assert!(view(&app).rows.iter().all(|r| !r.on));
    }

    #[test]
    fn enter_deletes_the_ticked_and_queues_only_safe_branches() {
        let mut app = opened(three());
        let mut out = Vec::new();
        handle_key(&mut app, key(KeyCode::Char('b')), &mut out);
        let (sum, kept) = summary(view(&app));
        assert_eq!(sum, " Deletes 3 worktrees and 2 branches.");
        assert_eq!(
            kept.as_deref(),
            Some(" Keeps archived-only's branch: unpushed commits.")
        );
        handle_key(&mut app, key(KeyCode::Enter), &mut out);
        assert!(app.overlay.is_none());
        let mut deleted: Vec<&str> = out
            .iter()
            .filter_map(|r| match r {
                ClientRequest::DeleteWorktree {
                    id, force: false, ..
                } => Some(id.0.as_str()),
                _ => None,
            })
            .collect();
        deleted.sort();
        assert_eq!(deleted, ["idle1", "idle2", "old"]);
        let mut queued: Vec<&str> = app
            .clean_worktrees
            .branch_after
            .keys()
            .map(|id| id.0.as_str())
            .collect();
        queued.sort();
        assert_eq!(queued, ["idle1", "idle2"]);
    }

    #[test]
    fn without_b_no_branch_is_queued() {
        let mut app = opened(vec![found("idle1", BranchState::Merged)]);
        let mut out = Vec::new();
        handle_key(&mut app, key(KeyCode::Enter), &mut out);
        assert!(app.clean_worktrees.branch_after.is_empty());
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn start_without_a_channel_checks_inline() {
        let mut app = app();
        start(&mut app);
        let v = view(&app);
        assert_eq!(v.checking, 0);
        // Gone from disk: clean, with no branch to delete.
        assert_eq!(v.rows.len(), 3);
        assert!(v.rows.iter().all(|r| r.state == BranchState::None));
    }
}
