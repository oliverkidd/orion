//! STACK STATUS in the TUI: the DAEMON's listing of every docker compose
//! stack on the machine (`ServerEvent::StacksChanged`), the band's `⬡`
//! reads it through [`App::stack_of`], and the Stacks modal (`⇧S`) lists
//! it — every stack, orion's or not, with the worktree it belongs to — to
//! start, stop or take one down.

use crate::app::{App, Overlay, PendingIntent};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::compose::{self, Stack, StackState, StackVerb};
use orion_core::protocol::ClientRequest;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

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

/// Send `verb` for `project`, and mark its row busy until it lands —
/// unless a verb for it is already on its way.
fn send(app: &mut App, out: &mut Vec<ClientRequest>, project: &str, verb: StackVerb) {
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

/// **Stop all stacks**: every running stack, stopped.
pub(crate) fn stop_all(app: &mut App, out: &mut Vec<ClientRequest>) {
    let running: Vec<String> = app
        .stacks
        .iter()
        .flatten()
        .filter(|s| s.state() == StackState::Running)
        .map(|s| s.project.clone())
        .collect();
    if running.is_empty() {
        app.flash = Some(crate::flash::Flash::note("no stack is running"));
        return;
    }
    for project in &running {
        send(app, out, project, StackVerb::Stop);
    }
}

// ---- the rows ----

/// Which stacks the modal lists first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// Started in a worktree of the selected project.
    ThisProject,
    /// In another project's worktree.
    OtherProject,
    /// In a checkout orion doesn't know.
    Elsewhere,
}

/// One stack as the modal lists it.
#[derive(Debug, Clone)]
pub struct Row {
    pub stack: Stack,
    pub group: Group,
    /// The WORKTREE column.
    pub place: String,
}

/// Every stack, the selected project's first, then other projects', then
/// the rest; running before stopped in each, then by name.
pub fn rows(app: &App) -> Vec<Row> {
    let Some(stacks) = &app.stacks else {
        return Vec::new();
    };
    let selected = app.selected_project().map(|p| p.id.clone());
    let checkouts = app.checkout_paths();
    let mut rows: Vec<Row> = stacks
        .iter()
        .map(|stack| {
            let owner = compose::owner_of(stack, &checkouts)
                .and_then(|dir| app.tree.worktrees.iter().find(|w| w.path == dir));
            let (group, place) = match owner {
                Some(w) => {
                    let project = app.tree.projects.iter().find(|p| p.id == w.project_id);
                    let mine = Some(&w.project_id) == selected.as_ref();
                    let name = if w.is_main { "main" } else { w.branch.as_str() };
                    let place = match project {
                        Some(p) if !mine || w.is_main => format!("{name} · {}", p.name),
                        _ => name.to_string(),
                    };
                    let group = if mine {
                        Group::ThisProject
                    } else {
                        Group::OtherProject
                    };
                    (group, place)
                }
                None if stack.dirs.len() > 1 => (Group::Elsewhere, "several checkouts".to_string()),
                None => (
                    Group::Elsewhere,
                    stack
                        .dirs
                        .first()
                        .map(|d| crate::claude_accounts::tilde(d))
                        .unwrap_or_default(),
                ),
            };
            Row {
                stack: stack.clone(),
                group,
                place,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        (
            a.group,
            a.stack.state() != StackState::Running,
            &a.stack.project,
        )
            .cmp(&(
                b.group,
                b.stack.state() != StackState::Running,
                &b.stack.project,
            ))
    });
    rows
}

// ---- the modal ----

#[derive(Debug, Clone, Default)]
pub struct StacksView {
    /// The cursor, by project: a fresh listing reorders the rows, and the
    /// cursor stays on its stack.
    pub selected: Option<String>,
    /// The stack ⌘W asked to take down, waiting for Enter (volumes kept)
    /// or ⌘W again (volumes too).
    pub confirm: Option<String>,
    pub area: Rect,
    pub list_area: Rect,
    /// The row drawn on the list's top line: the list scrolls to keep the
    /// cursor in view.
    pub first: usize,
}

impl StacksView {
    fn row_in(&self, rows: &[Row]) -> usize {
        self.selected
            .as_ref()
            .and_then(|p| rows.iter().position(|r| &r.stack.project == p))
            .unwrap_or(0)
    }
}

pub(crate) mod keys {
    use crate::hints::Key;

    pub const TOGGLE: Key = Key::new(&["enter"], "start/stop");
    pub const DOWN: Key = Key::new(&["cmd+w", "ctrl+w"], "take down");
    pub const STOP_ALL: Key = Key::new(&["s"], "stop all");
    pub const CLOSE: Key = Key::new(&["esc", "q", "shift+s"], "close");
    #[cfg(test)]
    pub const ALL: &[Key] = &[TOGGLE, DOWN, STOP_ALL, CLOSE];
}

/// `⇧S`: the modal, the cursor on the selected worktree's stack.
pub(crate) fn open(app: &mut App) {
    let worktree = app.selected_worktree().map(|w| w.id.clone());
    show(app, worktree.as_ref());
}

/// A click on a band's `⬡`: the modal, on that worktree's stack.
pub(crate) fn open_at(app: &mut App, worktree: &orion_core::WorktreeId) {
    show(app, Some(worktree));
}

fn show(app: &mut App, worktree: Option<&orion_core::WorktreeId>) {
    let selected = worktree
        .and_then(|w| app.stack_of(w))
        .map(|s| s.project.clone());
    app.overlay = Some(Overlay::Stacks(StacksView {
        selected,
        ..StacksView::default()
    }));
    app.dirty = true;
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let all = rows(app);
    let Some(Overlay::Stacks(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    let row = view.row_in(&all);
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
            let next = crate::app::clamp_selection(row as i64 + step, all.len());
            view.selected = all.get(next).map(|r| r.stack.project.clone());
        }
        _ if keys::TOGGLE.matches(&key) => {
            if let Some(r) = all.get(row) {
                toggle(app, out, &r.stack);
            }
        }
        _ if keys::DOWN.matches(&key) => {
            view.confirm = all.get(row).map(|r| r.stack.project.clone());
        }
        _ if keys::STOP_ALL.matches(&key) => stop_all(app, out),
        _ => {}
    }
}

/// Enter: start a stack with nothing running, stop one with something.
fn toggle(app: &mut App, out: &mut Vec<ClientRequest>, stack: &Stack) {
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
    let all = rows(app);
    let Some(Overlay::Stacks(view)) = &mut app.overlay else {
        return;
    };
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }
    let Some(r) = crate::list_hit::row_at(view.list_area, view.first, all.len(), pos)
        .and_then(|i| all.get(i))
    else {
        return;
    };
    view.confirm = None;
    view.selected = Some(r.stack.project.clone());
    // The `⬡` is a switch; the rest of the row is the cursor.
    if pos.x < view.list_area.x + MARK_W {
        toggle(app, out, &r.stack);
    }
    app.dirty = true;
}

/// The ` ⬡ ` gutter each row starts with.
const MARK_W: u16 = 3;
const STATE_W: usize = 15;
const WIDEST: u16 = 110;
/// Lines under the list: a blank, then the confirm or where it ran from.
const NOTE_H: u16 = 2;

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &StacksView, th: Theme) {
    let screen = f.area();
    let all = rows(app);
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let inner_w = usize::from(width.saturating_sub(2));
    // The gutter, then STACK (a little over half) and WORKTREE sharing
    // what STATE leaves.
    let flex = inner_w.saturating_sub(usize::from(MARK_W) + STATE_W + 2);
    let stack_w = flex * 11 / 20;
    let place_w = flex.saturating_sub(stack_w + 1);
    let selected = view.row_in(&all);

    let dim = Style::default().fg(th.dim);
    let header = Style::default().fg(th.muted).add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        format!(
            "   {:<stack_w$} {:<place_w$} {:<STATE_W$}",
            "STACK", "WORKTREE", "STATE"
        ),
        header,
    ))];
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
        Some(_) if all.is_empty() => {
            lines.push(Line::from(Span::styled(
                " No compose stacks on this machine.",
                dim,
            )));
        }
        Some(_) => {}
    }
    // As many rows as fit, scrolled to keep the cursor on screen.
    let fits = usize::from(screen.height.saturating_sub(4 + list_top + NOTE_H).max(1));
    let first = view
        .first
        .min(selected)
        .max((selected + 1).saturating_sub(fits));
    for (i, r) in all.iter().enumerate().skip(first).take(fits) {
        let running = r.stack.state() == StackState::Running;
        let sel = |s: Style| {
            if i == selected {
                s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                s
            }
        };
        let state = match app.stack_pending.get(&r.stack.project) {
            Some((verb, _)) => verb.progress().to_string(),
            None if running => format!("{}/{} up", r.stack.running, r.stack.total),
            None => format!("{}/{}", r.stack.running, r.stack.total),
        };
        let mark = Style::default().fg(if running { th.ok } else { th.faint });
        let text = Style::default().fg(if running { th.text } else { th.muted });
        lines.push(Line::from(vec![
            Span::styled(format!(" {MARK} "), sel(mark)),
            Span::styled(
                format!(
                    "{:<stack_w$} ",
                    crate::ui::truncate(&r.stack.project, stack_w)
                ),
                sel(text),
            ),
            Span::styled(
                format!("{:<place_w$} ", crate::ui::truncate(&r.place, place_w)),
                sel(Style::default().fg(th.muted)),
            ),
            Span::styled(
                format!("{:<STATE_W$}", state),
                sel(Style::default().fg(if running { th.ok } else { th.dim })),
            ),
        ]));
    }

    // Below the list: the confirm, else where the selected stack ran from.
    lines.push(Line::from(""));
    if let Some(project) = &view.confirm {
        lines.push(Line::from(Span::styled(
            format!(
                " take down {project}? enter keeps its volumes · {} deletes them too · esc cancels",
                keys::DOWN.hint().key
            ),
            Style::default().fg(th.warn),
        )));
    } else if let Some(r) = all.get(selected) {
        let dirs: Vec<String> = r
            .stack
            .dirs
            .iter()
            .map(|d| crate::claude_accounts::tilde(d))
            .collect();
        lines.push(Line::from(Span::styled(
            format!(" started in {}", dirs.join(", ")),
            dim,
        )));
    }

    let running = all
        .iter()
        .filter(|r| r.stack.state() == StackState::Running)
        .count();
    let toggle = match all.get(selected).map(|r| r.stack.state()) {
        Some(StackState::Running) => keys::TOGGLE.hint_as("stop"),
        _ => keys::TOGGLE.hint_as("start"),
    };
    let hints = [
        toggle,
        keys::DOWN.hint(),
        keys::STOP_ALL.hint_as(format!("stop all ({running})")),
        crate::hints::ESC_CLOSE.hint(),
    ];
    let height = (lines.len() as u16 + 2).min(screen.height.saturating_sub(2));
    let area = crate::ui::centered_rect(screen, width, height);
    let title = format!(" Stacks · {running} running ");
    let inner = crate::ui::render_modal_frame(f, area, title, &hints, th);
    f.render_widget(Paragraph::new(lines), inner);
    let selected_project = all.get(selected).map(|r| r.stack.project.clone());
    if let Some(Overlay::Stacks(v)) = &mut app.overlay {
        v.area = area;
        v.selected = selected_project;
        v.first = first;
        v.list_area = Rect {
            x: inner.x,
            y: inner.y + list_top,
            width: inner.width,
            height: (all.len().saturating_sub(first).min(fits) as u16)
                .min(inner.height.saturating_sub(list_top)),
        };
    }
}
