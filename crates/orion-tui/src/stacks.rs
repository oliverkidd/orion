//! STACK STATUS in the TUI: the DAEMON's listing of every docker compose
//! stack on the machine (`ServerEvent::StacksChanged`), the band's `⬡`
//! reads it through [`App::stack_of`], and the Stacks modal (`⇧S`) lists
//! it — a tab per project, its checkouts in the order its bands stand on
//! the grid, then a tab for the stacks orion can't place — to start, stop
//! or take one down. Starting a checkout's stack is `⌘⇧S`'s
//! (`event_loop::toggle_stack_in`): its RUN COMMAND, in the pane.

use crate::app::{App, Overlay, PendingIntent};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::compose::{self, Stack, StackState, StackVerb};
use orion_core::protocol::ClientRequest;
use orion_core::{ProjectId, Worktree};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::HashMap;
use std::path::Path;

/// The mark a stack wears, on its band and in the modal.
pub const MARK: &str = "⬡";

// ---- the listing ----

/// `StacksChanged`: the new listing.
pub(crate) fn listed(app: &mut App, stacks: Option<Vec<Stack>>, error: Option<String>) {
    app.stacks = stacks;
    app.stacks_error = error;
    app.dirty = true;
}

/// The Ack or Error for `req_id`: if it was a stack verb, its row stops
/// saying it is running — the listing it poked shows the result, and an
/// Error's toast says why there is none.
pub(crate) fn settled(app: &mut App, req_id: u64) {
    app.stack_pending.retain(|_, (_, req)| *req != req_id);
}

/// A checkout's menu item for `⌘⇧S`: **Stop stack** while its run or its
/// compose stack is up, else **Start stack**.
pub(crate) fn menu_label(app: &App, worktree: &orion_core::WorktreeId) -> &'static str {
    if app.worktree_running(worktree) || app.stack_running(worktree) {
        "Stop stack"
    } else {
        "Start stack"
    }
}

/// Send `verb` for `project`, and mark its row busy until it lands —
/// unless a verb for it is already on its way.
pub(crate) fn send(app: &mut App, out: &mut Vec<ClientRequest>, project: &str, verb: StackVerb) {
    if app.stack_pending.contains_key(project) {
        return;
    }
    let req_id = app.alloc_req_id(PendingIntent::None);
    app.stack_pending
        .insert(project.to_string(), (verb, req_id));
    out.push(ClientRequest::StackAction {
        req_id,
        project: project.to_string(),
        verb,
    });
    app.dirty = true;
}

/// **Stop all stacks**, and `s` in the modal: every checkout's run
/// sent its `^C` (its trap winds its stack down), and every other running
/// stack stopped.
pub(crate) fn stop_all(app: &mut App, out: &mut Vec<ClientRequest>) {
    let runs: Vec<&Worktree> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| app.worktree_running(&w.id) && !app.runs_stopping.contains(&w.id))
        .collect();
    let checkouts = app.checkout_paths();
    let running: Vec<String> = app
        .stacks
        .iter()
        .flatten()
        .filter(|s| s.state() == StackState::Running)
        .filter(|s| {
            // A run's own trap stops its stack; stopping it under the run
            // would pull the containers out from under its server.
            compose::owner_of(s, &checkouts).is_none_or(|dir| !runs.iter().any(|w| w.path == dir))
        })
        .map(|s| s.project.clone())
        .collect();
    let runs: Vec<orion_core::WorktreeId> = runs.into_iter().map(|w| w.id.clone()).collect();
    if runs.is_empty() && running.is_empty() {
        app.flash = Some(crate::flash::Flash::note("no stack is running"));
        return;
    }
    for id in &runs {
        crate::event_loop::toggle_stack_in(app, id, out);
    }
    for project in &running {
        send(app, out, project, StackVerb::Stop);
    }
    // One line for the lot, not the last run's own.
    app.flash = Some(crate::flash::Flash::note(format!(
        "stopping {}",
        crate::bundle::plural(runs.len() + running.len(), "stack")
    )));
}

// ---- the tabs and their rows ----

/// One of the modal's tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabId {
    /// A project: every checkout of it.
    Project(ProjectId),
    /// The stacks started somewhere orion has no checkout of.
    Outside,
}

/// A tab as the modal draws it.
#[derive(Debug, Clone)]
pub struct Tab {
    pub id: TabId,
    pub label: String,
    pub rows: Vec<Row>,
}

/// One line of a tab: a checkout and its stack, or a stack alone.
#[derive(Debug, Clone)]
pub struct Row {
    /// The checkout, on a project's tab; None on the Outside tab.
    pub worktree: Option<Worktree>,
    /// Its compose stack; None for a checkout that has none yet.
    pub stack: Option<Stack>,
    /// A further stack of a checkout that has several: it starts and
    /// stops by compose alone, its checkout's run being the first's.
    pub extra: bool,
    /// The WORKTREE column.
    pub place: String,
}

impl Row {
    /// What the cursor holds on to across a fresh listing.
    fn key(&self) -> String {
        match (&self.worktree, &self.stack) {
            (Some(w), Some(s)) if self.extra => format!("{}\t{}", w.id.0, s.project),
            (Some(w), _) => w.id.0.clone(),
            (None, Some(s)) => format!("\t{}", s.project),
            (None, None) => String::new(),
        }
    }

    fn running(&self, app: &App) -> bool {
        let run = !self.extra
            && self
                .worktree
                .as_ref()
                .is_some_and(|w| app.worktree_running(&w.id));
        run || self
            .stack
            .as_ref()
            .is_some_and(|s| s.state() == StackState::Running)
    }
}

/// Running before stopped, then by name.
fn by_state(a: &Stack, b: &Stack) -> std::cmp::Ordering {
    (a.state() != StackState::Running, &a.project)
        .cmp(&(b.state() != StackState::Running, &b.project))
}

/// A tab's label: its name, then how many of its rows are up.
fn tab_label(app: &App, name: &str, rows: &[Row]) -> String {
    match rows.iter().filter(|r| r.running(app)).count() {
        0 => name.to_string(),
        up => format!("{name} {up}"),
    }
}

/// The modal's tabs: one per PROJECT TAB in the header's order, then any
/// other project with a stack, then **Outside orion** when a stack
/// belongs to no checkout orion knows. A project's rows are every
/// checkout, in its bands' order on the grid.
pub fn tabs(app: &App) -> Vec<Tab> {
    let stacks: &[Stack] = app.stacks.as_deref().unwrap_or_default();
    let checkouts = app.checkout_paths();
    // Each stack under the checkout it belongs to; None, orion knows no
    // checkout it was started in.
    let mut owned: HashMap<Option<&Path>, Vec<&Stack>> = HashMap::new();
    for s in stacks {
        owned
            .entry(compose::owner_of(s, &checkouts))
            .or_default()
            .push(s);
    }
    let mut projects: Vec<&ProjectId> = app.launcher_tabs.iter().collect();
    if let Some(p) = app.selected_project() {
        projects.push(&p.id);
    }
    projects.extend(
        app.tree
            .worktrees
            .iter()
            .filter(|w| owned.contains_key(&Some(w.path.as_path())))
            .map(|w| &w.project_id),
    );
    let mut seen = std::collections::HashSet::new();
    projects.retain(|p| seen.insert(*p));
    let mut tabs: Vec<Tab> = projects
        .into_iter()
        .filter_map(|pid| {
            let project = app.tree.projects.iter().find(|p| &p.id == pid)?;
            let mut rows = Vec::new();
            for w in app.worktrees_in_band_order(pid) {
                if app.is_placeholder_worktree(&w.id) {
                    continue;
                }
                let place = crate::git_sync::label(w);
                // The first is the one `⌘⇧S` acts on (`App::stack_of`).
                let first = app.stack_of(&w.id);
                rows.push(Row {
                    worktree: Some(w.clone()),
                    stack: first.cloned(),
                    extra: false,
                    place: place.clone(),
                });
                let mut more: Vec<&Stack> = owned
                    .get(&Some(w.path.as_path()))
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|s| Some(&s.project) != first.map(|f| &f.project))
                    .collect();
                more.sort_by(|a, b| by_state(a, b));
                rows.extend(more.into_iter().map(|s| Row {
                    worktree: Some(w.clone()),
                    stack: Some(s.clone()),
                    extra: true,
                    place: place.clone(),
                }));
            }
            Some(Tab {
                id: TabId::Project(pid.clone()),
                label: tab_label(app, &project.name, &rows),
                rows,
            })
        })
        .collect();
    if let Some(outside) = owned.get_mut(&None) {
        outside.sort_by(|a, b| by_state(a, b));
        let rows: Vec<Row> = outside
            .iter()
            .map(|s| Row {
                worktree: None,
                place: match s.dirs.as_slice() {
                    [one] => crate::claude_accounts::tilde(one),
                    _ => "several checkouts".to_string(),
                },
                stack: Some((*s).clone()),
                extra: false,
            })
            .collect();
        tabs.push(Tab {
            id: TabId::Outside,
            label: tab_label(app, "Outside orion", &rows),
            rows,
        });
    }
    tabs
}

// ---- the modal ----

#[derive(Debug, Clone, Default)]
pub struct StacksView {
    /// The tab on show, by id: a project closing shifts the rest.
    pub tab: Option<TabId>,
    /// The cursor, by [`Row::key`]: a fresh listing reorders the rows, and
    /// the cursor stays on its row.
    pub selected: Option<String>,
    /// The stack ⌘W asked to take down, waiting for Enter (volumes kept)
    /// or ⌘W again (volumes too).
    pub confirm: Option<String>,
    pub area: Rect,
    pub list_area: Rect,
    /// Each tab label's x-range on the strip, for a click.
    pub tab_hits: Vec<(u16, u16)>,
    pub tab_row: Rect,
    /// The row drawn on the list's top line: the list scrolls to keep the
    /// cursor in view.
    pub first: usize,
}

impl StacksView {
    fn tab_in(&self, tabs: &[Tab]) -> usize {
        self.tab
            .as_ref()
            .and_then(|t| tabs.iter().position(|x| &x.id == t))
            .unwrap_or(0)
    }

    /// The rows of the tab on show.
    fn rows_in<'a>(&self, tabs: &'a [Tab]) -> &'a [Row] {
        tabs.get(self.tab_in(tabs))
            .map_or(&[], |t| t.rows.as_slice())
    }

    /// Show `tab`, the cursor back on its first row — unless it is on show.
    fn switch_to(&mut self, tab: &Tab) {
        if self.tab.as_ref() != Some(&tab.id) {
            self.tab = Some(tab.id.clone());
            self.selected = None;
            self.first = 0;
        }
    }

    fn row_in(&self, rows: &[Row]) -> usize {
        self.selected
            .as_ref()
            .and_then(|k| rows.iter().position(|r| &r.key() == k))
            .unwrap_or(0)
    }
}

pub(crate) mod keys {
    use crate::hints::Key;

    pub const TOGGLE: Key = Key::new(&["enter"], "start/stop");
    pub const TABS: Key = Key::new(&["left", "right", "tab", "shift+tab"], "project");
    pub const DOWN: Key = Key::new(&["cmd+w", "ctrl+w"], "take down");
    pub const STOP_ALL: Key = Key::new(&["s"], "stop all");
    pub const CLOSE: Key = Key::new(&["esc", "q", "shift+s"], "close");
    #[cfg(test)]
    pub const ALL: &[Key] = &[TOGGLE, TABS, DOWN, STOP_ALL, CLOSE];
}

/// `⇧S`: the modal, on the selected project's tab and the selected
/// worktree's row.
pub(crate) fn open(app: &mut App) {
    let worktree = app.selected_worktree().map(|w| w.id.clone());
    show(app, worktree.as_ref());
}

/// A click on a band's `⬡`: the modal, on that worktree's row.
pub(crate) fn open_at(app: &mut App, worktree: &orion_core::WorktreeId) {
    show(app, Some(worktree));
}

fn show(app: &mut App, worktree: Option<&orion_core::WorktreeId>) {
    let project = worktree
        .and_then(|id| app.tree.worktrees.iter().find(|w| &w.id == id))
        .map(|w| w.project_id.clone())
        .or_else(|| app.selected_project().map(|p| p.id.clone()));
    app.overlay = Some(Overlay::Stacks(StacksView {
        tab: project.map(TabId::Project),
        selected: worktree.map(|w| w.0.clone()),
        ..StacksView::default()
    }));
    app.dirty = true;
}

/// Step the tab `by` (±1), stopping at the ends.
fn step_tab(view: &mut StacksView, tabs: &[Tab], by: i64) {
    let next = crate::app::clamp_selection(view.tab_in(tabs) as i64 + by, tabs.len());
    if let Some(t) = tabs.get(next) {
        view.switch_to(t);
    }
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let tabs = tabs(app);
    let Some(Overlay::Stacks(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    let rows = view.rows_in(&tabs);
    let row = view.row_in(rows);
    if let Some(project) = view.confirm.take() {
        if keys::TOGGLE.matches(&key) {
            send(app, out, &project, StackVerb::Down);
        } else if keys::DOWN.matches(&key) {
            send(app, out, &project, StackVerb::DownVolumes);
        }
        // Anything else — Esc first of all — is a change of mind.
        return;
    }
    match key.code {
        _ if keys::CLOSE.matches(&key) => app.overlay = None,
        KeyCode::Char('j' | 'k') | KeyCode::Down | KeyCode::Up => {
            let step = if matches!(key.code, KeyCode::Char('j') | KeyCode::Down) {
                1
            } else {
                -1
            };
            let next = crate::app::clamp_selection(row as i64 + step, rows.len());
            view.selected = rows.get(next).map(Row::key);
        }
        _ if keys::TABS.matches(&key) => {
            let back = matches!(key.code, KeyCode::Left | KeyCode::BackTab)
                || key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::SHIFT);
            step_tab(view, &tabs, if back { -1 } else { 1 });
        }
        _ if keys::TOGGLE.matches(&key) => {
            if let Some(r) = rows.get(row).cloned() {
                toggle(app, out, &r);
            }
        }
        _ if keys::DOWN.matches(&key) => {
            view.confirm = rows
                .get(row)
                .and_then(|r| r.stack.as_ref())
                .map(|s| s.project.clone());
        }
        _ if keys::STOP_ALL.matches(&key) => stop_all(app, out),
        _ => {}
    }
}

/// Enter: a checkout's stack started or stopped as `⌘⇧S` does it; a
/// stack alone, by compose — started with nothing running, stopped with
/// something.
fn toggle(app: &mut App, out: &mut Vec<ClientRequest>, row: &Row) {
    if let (Some(w), false) = (&row.worktree, row.extra) {
        crate::event_loop::toggle_stack_in(app, &w.id, out);
        return;
    }
    let Some(stack) = &row.stack else {
        return;
    };
    let verb = match stack.state() {
        StackState::Running => StackVerb::Stop,
        StackState::Stopped => StackVerb::Start,
    };
    send(app, out, &stack.project, verb);
}

pub(crate) fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    pos: Position,
    out: &mut Vec<ClientRequest>,
) {
    let tabs = tabs(app);
    let Some(Overlay::Stacks(view)) = &mut app.overlay else {
        return;
    };
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }
    app.dirty = true;
    if view.tab_row.contains(pos) {
        if let Some(t) = crate::ui::tab_hit(&view.tab_hits, pos.x).and_then(|i| tabs.get(i)) {
            view.confirm = None;
            view.switch_to(t);
        }
        return;
    }
    let rows = view.rows_in(&tabs);
    let Some(r) = crate::list_hit::row_at(view.list_area, view.first, rows.len(), pos)
        .and_then(|i| rows.get(i))
        .cloned()
    else {
        return;
    };
    view.confirm = None;
    view.selected = Some(r.key());
    // The `⬡` is a switch; the rest of the row is the cursor.
    if pos.x < view.list_area.x + MARK_W {
        toggle(app, out, &r);
    }
}

/// The ` ⬡ ` gutter each row starts with.
const MARK_W: u16 = 3;
const STATE_W: usize = 15;
const WIDEST: u16 = 110;
/// Lines under the list: a blank, then the confirm or what the row is.
const NOTE_H: u16 = 2;

/// The STATE column: a verb on its way, then the run and the containers.
fn state_of(app: &App, r: &Row) -> String {
    if let Some((verb, _)) = r
        .stack
        .as_ref()
        .and_then(|s| app.stack_pending.get(&s.project))
    {
        return verb.progress().to_string();
    }
    let run = match &r.worktree {
        Some(w) if !r.extra && app.worktree_running(&w.id) => {
            Some(app.runs_stopping.contains(&w.id))
        }
        _ => None,
    };
    let containers = r.stack.as_ref().map(|s| match s.state() {
        StackState::Running => format!("{}/{} up", s.running, s.total),
        StackState::Stopped => format!("{}/{}", s.running, s.total),
    });
    match (run, containers) {
        (Some(true), _) => "stopping…".to_string(),
        (Some(false), Some(c)) => format!("run · {c}"),
        (Some(false), None) => "running".to_string(),
        (None, Some(c)) => c,
        (None, None) => "—".to_string(),
    }
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &StacksView, th: Theme) {
    let screen = f.area();
    let tabs = tabs(app);
    let tab = view.tab_in(&tabs);
    let rows = view.rows_in(&tabs);
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let inner_w = usize::from(width.saturating_sub(2));
    // The gutter, then WORKTREE (a little over half) and STACK sharing
    // what STATE leaves.
    let flex = inner_w.saturating_sub(usize::from(MARK_W) + STATE_W + 2);
    let place_w = flex * 11 / 20;
    let stack_w = flex.saturating_sub(place_w + 1);
    let selected = view.row_in(rows);

    let dim = Style::default().fg(th.dim);
    let header = Style::default().fg(th.muted).add_modifier(Modifier::BOLD);
    // The tab strip and its rule go in on draw, once the frame is placed.
    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "   {:<place_w$} {:<stack_w$} {:<STATE_W$}",
                "WORKTREE", "STACK", "STATE"
            ),
            header,
        )),
    ];
    let list_top = lines.len() as u16;

    match &app.stacks {
        None => {
            let why = match app.stacks_error.as_deref() {
                Some(compose::NO_DOCKER_CLI) => " No docker CLI found — orion looks on PATH, in ~/.orbstack/bin, /usr/local/bin, /opt/homebrew/bin and Docker Desktop's app bundle.".to_string(),
                Some(e) => format!(" Docker isn't answering: {e}"),
                None => " Asking docker…".to_string(),
            };
            lines.push(Line::from(Span::styled(why, Style::default().fg(th.warn))));
        }
        Some(_) if rows.is_empty() => {
            lines.push(Line::from(Span::styled(" No checkouts here.", dim)));
        }
        Some(_) => {}
    }
    // As many rows as fit, scrolled to keep the cursor on screen.
    let fits = usize::from(screen.height.saturating_sub(4 + list_top + NOTE_H).max(1));
    let first = view
        .first
        .min(selected)
        .max((selected + 1).saturating_sub(fits));
    for (i, r) in rows.iter().enumerate().skip(first).take(fits) {
        let running = r.running(app);
        let sel = |s: Style| {
            if i == selected {
                s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                s
            }
        };
        let mark = match (&r.stack, running) {
            (_, true) => Span::styled(format!(" {MARK} "), sel(Style::default().fg(th.ok))),
            (Some(_), false) => {
                Span::styled(format!(" {MARK} "), sel(Style::default().fg(th.faint)))
            }
            (None, false) => Span::styled("   ", sel(Style::default())),
        };
        let text = Style::default().fg(if running { th.text } else { th.muted });
        let stack = r.stack.as_ref().map_or("—", |s| s.project.as_str());
        lines.push(Line::from(vec![
            mark,
            Span::styled(
                format!("{:<place_w$} ", crate::ui::truncate(&r.place, place_w)),
                sel(text),
            ),
            Span::styled(
                format!("{:<stack_w$} ", crate::ui::truncate(stack, stack_w)),
                sel(Style::default().fg(th.muted)),
            ),
            Span::styled(
                format!("{:<STATE_W$}", state_of(app, r)),
                sel(Style::default().fg(if running { th.ok } else { th.dim })),
            ),
        ]));
    }
    // Every tab as tall as the tallest, so a switch never resizes it.
    let tallest = tabs
        .iter()
        .map(|t| t.rows.len())
        .max()
        .unwrap_or(0)
        .min(fits);
    while lines.len() < usize::from(list_top) + tallest {
        lines.push(Line::from(""));
    }

    // Below the list: the confirm, else what Enter does to the row.
    lines.push(Line::from(""));
    if let Some(project) = &view.confirm {
        lines.push(Line::from(Span::styled(
            format!(
                " take down {project}? enter keeps its volumes · {} deletes them too · esc cancels",
                keys::DOWN.hint().key
            ),
            Style::default().fg(th.warn),
        )));
    } else if let Some(r) = rows.get(selected) {
        let note = match (&r.stack, &r.worktree) {
            (Some(s), _) => {
                let dirs: Vec<String> =
                    s.dirs.iter().map(|d| crate::claude_accounts::tilde(d)).collect();
                format!(" started in {}", dirs.join(", "))
            }
            (None, Some(_)) => {
                " no stack yet — enter starts its run command (Settings → Project, or .orion.json \"run\")".to_string()
            }
            (None, None) => String::new(),
        };
        lines.push(Line::from(Span::styled(note, dim)));
    }

    let running = tabs
        .iter()
        .flat_map(|t| &t.rows)
        .filter(|r| r.running(app))
        .count();
    let toggle = match rows.get(selected) {
        Some(r) if r.running(app) => keys::TOGGLE.hint_as("stop"),
        _ => keys::TOGGLE.hint_as("start"),
    };
    let mut hints = vec![toggle];
    if tabs.len() > 1 {
        hints.push(keys::TABS.show(2).hint());
    }
    if rows.get(selected).is_some_and(|r| r.stack.is_some()) {
        hints.push(keys::DOWN.hint());
    }
    hints.push(keys::STOP_ALL.hint_as(format!("stop all ({running})")));
    hints.push(crate::hints::ESC_CLOSE.hint());
    let height = (lines.len() as u16 + 2).min(screen.height.saturating_sub(2));
    let area = crate::ui::centered_rect(screen, width, height);
    let title = format!(" Stacks · {running} running ");
    let inner = crate::ui::render_modal_frame(f, area, title, &hints, th);
    let tab_row = Rect { height: 1, ..inner };
    let (strip, tab_hits) = crate::ui::tab_strip(
        tab_row.x,
        tab_row.width,
        tabs.iter().map(|t| t.label.as_str()),
        tab,
        false,
        th,
    );
    lines[0] = Line::from(strip);
    lines[1] = crate::ui::strip_rule(inner.width, th);
    f.render_widget(Paragraph::new(lines), inner);
    let selected_key = rows.get(selected).map(Row::key);
    let tab_id = tabs.get(tab).map(|t| t.id.clone());
    if let Some(Overlay::Stacks(v)) = &mut app.overlay {
        v.area = area;
        v.tab = tab_id;
        v.selected = selected_key;
        v.first = first;
        v.tab_hits = tab_hits;
        v.tab_row = tab_row;
        v.list_area = Rect {
            x: inner.x,
            y: inner.y + list_top,
            width: inner.width,
            height: (rows.len().saturating_sub(first).min(fits) as u16)
                .min(inner.height.saturating_sub(list_top)),
        };
    }
}
