//! The TAKEAWAYS SCREEN (5.8): one meeting's takeaways, read by meeting
//! takeaways (`automations::meeting_takeaways`), for you to check before
//! they go into your todos.
//!
//! * **Left, FROM THE MEETING:** every takeaway, `✓` going in or `✗` left
//!   out, with its owner's initials. Space flips it; the ones that name you
//!   start accepted.
//! * **Right, WHERE THEY GO IN YOUR TODOS:** each list an accepted one goes
//!   in — the linked projects', Blue sky's — its groups under rules, what
//!   is already there dim and what comes in green.
//! * `m` puts the MOVE PICKER in the right side's place: a filter, then
//!   every linked project's groups and Blue sky's, then `+ new group
//!   "<filter>"`. Enter there puts the takeaway at that row.
//! * Enter adds the accepted ones ([`meeting_takeaways::add_to_todos`]);
//!   Esc leaves them for later, as they stand.
//!
//! INPUT PARITY: a click on a takeaway is the arrows landing on it; a click
//! on a picker row is Enter on it ([`choose`]).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::automations::meeting_takeaways::{self as mt, Meeting, Takeaway};
use crate::automations::view::spread;
use crate::hints::Hint;
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::todos::store;

pub(crate) mod keys {
    use crate::hints::Key;

    pub const ACCEPT: Key = Key::new(&["space"], "accept / leave out");
    pub const MOVE: Key = Key::new(&["m"], "move");
    /// The TODOS MODAL's priority key: here it steps through Linear's.
    pub const PRIORITY: Key = crate::todos::view::keys::PRIORITY;
    pub const ADD: Key = Key::new(&["enter"], "add to todos");
    pub const LATER: Key = crate::nav::CLOSE.doing("later");
    /// The move picker's.
    pub const PICK: Key = crate::nav::ROWS.doing("pick");
    pub const PUT: Key = Key::new(&["enter"], "put it here");
    pub const LEAVE: Key = crate::nav::CLOSE.doing("leave it where it was");
    #[cfg(test)]
    pub const ALL: &[Key] = &[ACCEPT, MOVE, PRIORITY, ADD, LATER, PICK, PUT, LEAVE];
}

/// The footnotes 5.8 names.
const LEFT_NOTE: &str = "✓ goes into your todos · ✗ left out. Ones that name you start accepted.";
const RIGHT_NOTE: &str =
    "Orion picked the project and group for each. m moves the selected one somewhere else.";
const PICK_NOTE: &str = "Every project's groups are here, filtered as you type.";

/// The widest the screen gets.
const WIDEST: u16 = 124;
/// Existing items shown under a group before `… N more`.
const EXISTING: usize = 3;

#[derive(Debug, Clone, Default)]
pub struct TakeawaysView {
    /// The meeting on screen, by its calendar id.
    pub meeting: String,
    /// The takeaway under the cursor.
    pub cursor: usize,
    /// `m`: the move picker, in the right side's place.
    pub picker: Option<Picker>,
    /// Esc goes back to the Automations screen.
    pub back: bool,
    /// Every list a takeaway can go in, as it opened ([`dests`]).
    pub lists: Vec<PathBuf>,
    /// The modal's rect and its rows', written back as it draws.
    pub area: Rect,
    hits: Vec<(Rect, Hit)>,
}

#[derive(Debug, Clone, Default)]
pub struct Picker {
    pub filter: TextInput,
    /// The row under the cursor, an index into [`spots`].
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Takeaway(usize),
    Spot(usize),
}

/// A row of the move picker: somewhere a takeaway can go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    pub dest: PathBuf,
    pub name: String,
    pub path: Vec<String>,
    /// `+ new group "<filter>"`.
    pub new: bool,
}

/// Open meeting `id`'s takeaways, every list they can go in read in.
pub fn open(app: &mut App, id: &str, back: bool) {
    let lists = dests(app);
    for key in &lists {
        crate::todos::view::load_list(app, key.clone());
    }
    app.overlay = Some(Overlay::Takeaways(Box::new(TakeawaysView {
        meeting: id.to_string(),
        back,
        lists,
        ..TakeawaysView::default()
    })));
    app.dirty = true;
}

fn close(app: &mut App) {
    let back = matches!(&app.overlay, Some(Overlay::Takeaways(v)) if v.back);
    app.overlay = None;
    if back {
        crate::automations::view::open(app);
    }
    app.dirty = true;
}

fn view_mut(app: &mut App) -> Option<&mut TakeawaysView> {
    match &mut app.overlay {
        Some(Overlay::Takeaways(view)) => Some(view),
        _ => None,
    }
}

/// The meeting on screen and the cursor's takeaway index, clamped.
fn on_screen(app: &App) -> Option<(&Meeting, usize)> {
    let Some(Overlay::Takeaways(view)) = &app.overlay else {
        return None;
    };
    let meeting = app.automations.meetings.meeting(&view.meeting)?;
    let at = view.cursor.min(meeting.takeaways.len().saturating_sub(1));
    Some((meeting, at))
}

/// Change the cursor's takeaway, and keep the change.
fn edit_takeaway(app: &mut App, change: impl FnOnce(&mut Takeaway)) {
    let Some(Overlay::Takeaways(view)) = &app.overlay else {
        return;
    };
    let (id, cursor) = (view.meeting.clone(), view.cursor);
    let shared = &mut app.automations.meetings;
    if let Some(t) = shared
        .meeting_mut(&id)
        .and_then(|m| m.takeaways.get_mut(cursor))
    {
        change(t);
        shared.save();
    }
    crate::needs_you::refresh(app);
}

/// What the name of the list `key` is: a project's, or Blue sky.
pub fn dest_name(app: &App, key: &Path) -> String {
    if store::is_blue_sky(key) {
        return crate::todos::view::BLUE_SKY_TAB.to_string();
    }
    app.tree
        .projects
        .iter()
        .find(|p| p.repo_path == key)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| orion_core::Project::folder_name(key))
}

/// Every list a takeaway can go in, in order: the linked projects', then
/// Blue sky's.
fn dests(app: &App) -> Vec<PathBuf> {
    let mut keys: Vec<PathBuf> = mt::linked(app).into_iter().map(|(_, k)| k).collect();
    keys.push(store::blue_sky_key());
    keys
}

/// The move picker's rows for `filter`: every group of every list, those
/// whose project and path hold each word typed, then `+ new group
/// "<filter>"` — in the list `takeaway` goes in, or the first there is.
pub fn spots(app: &App, keys: &[PathBuf], filter: &str, takeaway: Option<&Takeaway>) -> Vec<Spot> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    let mut out = Vec::new();
    for key in keys {
        let name = dest_name(app, key);
        let Some(file) = app.todos.get(key) else {
            continue;
        };
        for top in file.subgroups(None) {
            let mut paths = vec![vec![top.name.clone()]];
            paths.extend(
                file.subgroups(Some(top.id))
                    .map(|inner| vec![top.name.clone(), inner.name.clone()]),
            );
            for path in paths {
                let hay = format!("{name} {}", path.join(" ")).to_lowercase();
                if words.iter().all(|w| hay.contains(w.as_str())) {
                    out.push(Spot {
                        dest: key.clone(),
                        name: name.clone(),
                        path,
                        new: false,
                    });
                }
            }
        }
    }
    let typed = filter.trim();
    if !typed.is_empty() {
        let dest = takeaway
            .and_then(|t| t.dest.clone())
            .or_else(|| keys.first().cloned())
            .unwrap_or_else(store::blue_sky_key);
        out.push(Spot {
            name: dest_name(app, &dest),
            dest,
            path: vec![typed.to_string()],
            new: true,
        });
    }
    out
}

// ---- what a key or a click does ----

/// Space: the cursor's takeaway in or out.
fn toggle(app: &mut App) {
    edit_takeaway(app, |t| t.accepted = !t.accepted);
}

/// `⌘P`: the cursor's takeaway a step along Linear's priorities.
fn step_priority(app: &mut App) {
    edit_takeaway(app, |t| {
        let order = crate::linear::PRIORITY_ORDER;
        let at = order
            .iter()
            .position(|p| *p == t.priority)
            .unwrap_or(order.len() - 1);
        t.priority = order[(at + 1) % order.len()];
    });
}

/// `m`: the move picker.
fn open_picker(app: &mut App) {
    if on_screen(app).is_none_or(|(m, _)| m.takeaways.is_empty()) {
        return;
    }
    if let Some(view) = view_mut(app) {
        view.picker = Some(Picker::default());
    }
}

/// A picker row chosen — Enter, a click on it: the cursor's takeaway goes
/// there, accepted, and the picker goes.
fn choose(app: &mut App, index: usize) {
    let Some((meeting, at)) = on_screen(app) else {
        return;
    };
    let Some(Overlay::Takeaways(view)) = &app.overlay else {
        return;
    };
    let filter = view
        .picker
        .as_ref()
        .map(|p| p.filter.as_str().to_string())
        .unwrap_or_default();
    let Some(spot) = spots(app, &view.lists, &filter, meeting.takeaways.get(at))
        .into_iter()
        .nth(index)
    else {
        return;
    };
    edit_takeaway(app, |t| {
        t.dest = Some(spot.dest);
        t.group_path = spot.path;
        t.accepted = true;
    });
    if let Some(view) = view_mut(app) {
        view.picker = None;
    }
}

/// Enter: the accepted takeaways into your todos — or, one with nowhere to
/// go, the picker on it.
fn add(app: &mut App) {
    let Some(Overlay::Takeaways(view)) = &app.overlay else {
        return;
    };
    let id = view.meeting.clone();
    match mt::add_to_todos(app, &id) {
        Ok(n) => {
            close(app);
            let what = match n {
                1 => "1 takeaway".to_string(),
                n => format!("{n} takeaways"),
            };
            app.flash = Some(crate::flash::Flash::done(format!(
                "Added {what} to your todos"
            )));
        }
        Err(at) => {
            if let Some(view) = view_mut(app) {
                view.cursor = at;
            }
            open_picker(app);
            app.flash = Some(crate::flash::Flash::failed(
                "This one has nowhere to go yet: pick where".to_string(),
            ));
        }
    }
}

/// The cursor a row up or down the takeaways.
fn step(app: &mut App, down: bool) {
    let len = on_screen(app).map_or(0, |(m, _)| m.takeaways.len());
    if let Some(view) = view_mut(app) {
        view.cursor = match down {
            true => (view.cursor + 1).min(len.saturating_sub(1)),
            false => view.cursor.saturating_sub(1),
        };
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(view) = view_mut(app) else {
        return;
    };
    if view.picker.is_some() {
        app.dirty = true;
        return picker_key(app, key);
    }
    app.dirty = true;
    match key.code {
        _ if keys::LATER.matches(&key) => close(app),
        _ if keys::ACCEPT.matches(&key) => toggle(app),
        _ if keys::MOVE.matches(&key) => open_picker(app),
        _ if keys::PRIORITY.matches(&key) => step_priority(app),
        _ if keys::ADD.matches(&key) => add(app),
        KeyCode::Down => step(app, true),
        KeyCode::Up => step(app, false),
        _ => {}
    }
}

fn picker_key(app: &mut App, key: KeyEvent) {
    let rows = {
        let Some((meeting, at)) = on_screen(app) else {
            return;
        };
        let Some(Overlay::Takeaways(view)) = &app.overlay else {
            return;
        };
        let filter = view
            .picker
            .as_ref()
            .map(|p| p.filter.as_str())
            .unwrap_or("");
        spots(app, &view.lists, filter, meeting.takeaways.get(at)).len()
    };
    let Some(picker) = view_mut(app).and_then(|v| v.picker.as_mut()) else {
        return;
    };
    match key.code {
        _ if keys::LEAVE.matches(&key) => {
            if let Some(view) = view_mut(app) {
                view.picker = None;
            }
        }
        _ if keys::PUT.matches(&key) => {
            let at = picker.cursor;
            choose(app, at);
        }
        KeyCode::Down => picker.cursor = (picker.cursor + 1).min(rows.saturating_sub(1)),
        KeyCode::Up => picker.cursor = picker.cursor.saturating_sub(1),
        _ => {
            if picker.filter.handle_key(&key).changed() {
                picker.cursor = 0;
            }
        }
    }
}

/// A click on a takeaway is the arrows landing on it; on a picker row,
/// Enter on it. The wheel walks the takeaways.
pub fn handle_mouse(app: &mut App, mouse: MouseEvent, at: Position) {
    let Some(view) = view_mut(app) else {
        return;
    };
    let hit = view
        .hits
        .iter()
        .find(|(rect, _)| rect.contains(at))
        .map(|(_, hit)| *hit);
    match mouse.kind {
        MouseEventKind::ScrollDown => step(app, true),
        MouseEventKind::ScrollUp => step(app, false),
        MouseEventKind::Down(MouseButton::Left) => match hit {
            Some(Hit::Takeaway(i)) => view.cursor = i,
            Some(Hit::Spot(i)) => choose(app, i),
            None => return,
        },
        _ => return,
    }
    app.dirty = true;
}

// ---- drawing ----

pub fn hints(view: &TakeawaysView, accepted: usize) -> Vec<Hint> {
    match view.picker {
        Some(_) => vec![
            Hint::new("type", "to filter"),
            keys::PICK.hint(),
            keys::PUT.hint().kept(),
            keys::LEAVE.hint().kept(),
        ],
        None => vec![
            keys::ACCEPT.hint(),
            keys::MOVE.hint(),
            keys::PRIORITY.hint(),
            keys::ADD.hint_as(format!("add {accepted} to todos")).kept(),
            keys::LATER.hint().kept(),
        ],
    }
}

pub fn draw(f: &mut Frame, app: &mut App, view: &TakeawaysView, th: Theme) {
    let screen = f.area();
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let height = screen.height.saturating_sub(2).min(38);
    let area = crate::ui::centered_rect(screen, width, height);
    let Some(meeting) = app.automations.meetings.meeting(&view.meeting).cloned() else {
        crate::ui::render_modal_frame(f, area, " Meeting takeaways ", &hints(view, 0), th);
        if let Some(Overlay::Takeaways(live)) = &mut app.overlay {
            live.area = area;
        }
        return;
    };
    let inner = crate::ui::render_modal_frame(
        f,
        area,
        " Meeting takeaways ",
        &hints(view, meeting.accepted()),
        th,
    );
    let mut hits = Vec::new();
    let dim = Style::default().fg(th.dim);
    let cursor = view.cursor.min(meeting.takeaways.len().saturating_sub(1));

    // The header: the meeting, and how many are yours.
    let head = format!("{} · {} · Teams", meeting.subject, meeting.when());
    let line = format!(
        "{} of {} takeaways are yours and already accepted. Check where they go, then Enter.",
        meeting.mine(),
        meeting.takeaways.len()
    );
    if let Some(row) = crate::ui::row_rect(inner, 0) {
        f.render_widget(
            Paragraph::new(Span::styled(
                head,
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            )),
            row,
        );
    }
    if let Some(row) = crate::ui::row_rect(inner, 1) {
        f.render_widget(
            Paragraph::new(Span::styled(line, Style::default().fg(th.muted))),
            row,
        );
    }
    let cols = Rect {
        y: inner.y + 3,
        height: inner.height.saturating_sub(3),
        ..inner
    };
    let left_w = (cols.width.saturating_sub(3)) / 2;
    let left = Rect {
        width: left_w,
        ..cols
    };
    let right = Rect {
        x: cols.x + left_w + 3,
        width: cols.width.saturating_sub(left_w + 3),
        ..cols
    };
    for y in cols.y..cols.y + cols.height {
        f.render_widget(
            Paragraph::new(Span::styled("│", Style::default().fg(th.edge))),
            Rect::new(cols.x + left_w + 1, y, 1, 1),
        );
    }

    // Left: FROM THE MEETING.
    let lw = usize::from(left.width);
    let note = crate::hints::explain_lines(LEFT_NOTE, left.width, 2);
    let list_h = usize::from(left.height).saturating_sub(note.len() + 2);
    if let Some(row) = crate::ui::row_rect(left, 0) {
        f.render_widget(Paragraph::new(Span::styled("FROM THE MEETING", dim)), row);
    }
    let first = (cursor + 1).saturating_sub(list_h.max(1));
    for (n, (i, t)) in meeting
        .takeaways
        .iter()
        .enumerate()
        .skip(first)
        .take(list_h)
        .enumerate()
    {
        let Some(row) = crate::ui::row_rect(left, n + 2) else {
            break;
        };
        let lit = |s: Style| match (i == cursor, view.picker.is_some()) {
            (true, false) => s.bg(th.sel_bg).add_modifier(Modifier::BOLD),
            (true, true) => s.bg(th.sel_bg_dim),
            _ => s,
        };
        let (mark, mark_style, text_style) = match t.accepted {
            true => (
                "✓ ",
                Style::default().fg(th.ok),
                Style::default().fg(th.text),
            ),
            false => ("✗ ", Style::default().fg(th.faint), dim),
        };
        let owner = format!(" {} ", t.owner);
        let budget = lw.saturating_sub(4 + owner.chars().count());
        let line = spread(
            vec![
                Span::styled(" ", lit(Style::default())),
                Span::styled(mark, lit(mark_style)),
                Span::styled(crate::ui::truncate(&t.text, budget), lit(text_style)),
            ],
            Span::styled(owner, lit(Style::default().fg(th.muted))),
            lw,
        );
        f.render_widget(Paragraph::new(line), row);
        hits.push((row, Hit::Takeaway(i)));
    }
    draw_note(f, left, &note, th);

    // Right: where they go, or the move picker in its place.
    match &view.picker {
        Some(picker) => draw_picker(
            f,
            app,
            &view.lists,
            &meeting,
            cursor,
            picker,
            right,
            th,
            &mut hits,
        ),
        None => draw_where(f, app, &view.lists, &meeting, cursor, right, th),
    }

    if let Some(Overlay::Takeaways(live)) = &mut app.overlay {
        live.area = area;
        live.cursor = cursor;
        live.hits = hits;
    }
}

/// A footnote at the bottom of `col`.
fn draw_note(f: &mut Frame, col: Rect, note: &[String], th: Theme) {
    let top = col.height.saturating_sub(note.len() as u16);
    for (i, text) in note.iter().enumerate() {
        let row = Rect::new(col.x, col.y + top + i as u16, col.width, 1);
        f.render_widget(
            Paragraph::new(Span::styled(text.clone(), Style::default().fg(th.dim))),
            row,
        );
    }
}

/// WHERE THEY GO IN YOUR TODOS: each list an accepted takeaway goes in,
/// its groups under rules, what is there dim and what comes in green.
fn draw_where(
    f: &mut Frame,
    app: &App,
    lists: &[PathBuf],
    meeting: &Meeting,
    cursor: usize,
    col: Rect,
    th: Theme,
) {
    let w = usize::from(col.width);
    let dim = Style::default().fg(th.dim);
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled("WHERE THEY GO IN YOUR TODOS", dim)),
        Line::from(""),
    ];
    let mut cursor_line = 0;
    let accepted: Vec<(usize, &Takeaway)> = meeting
        .takeaways
        .iter()
        .enumerate()
        .filter(|(_, t)| t.accepted)
        .collect();
    if accepted.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Nothing accepted · Space takes one in",
            dim,
        )));
    }
    let mut keys = lists.to_vec();
    // A list no longer linked that a takeaway still names.
    for (_, t) in &accepted {
        if let Some(dest) = &t.dest {
            if !keys.contains(dest) {
                keys.push(dest.clone());
            }
        }
    }
    let new_line = |i: usize, t: &Takeaway| {
        let lit = |s: Style| match i == cursor {
            true => s.bg(th.sel_bg).add_modifier(Modifier::BOLD),
            false => s,
        };
        let (letter, style) = crate::linear::priority_letter(t.priority, th);
        let mut spans = vec![
            Span::styled("  + ", lit(Style::default().fg(th.ok))),
            Span::styled(format!("{letter} "), lit(style)),
            Span::styled(
                crate::ui::truncate(&t.text, w.saturating_sub(16)),
                lit(Style::default().fg(th.ok)),
            ),
        ];
        if let Some(ticket) = &t.ticket {
            spans.push(Span::styled(
                format!("  {ticket}"),
                lit(Style::default().fg(th.muted)),
            ));
        }
        Line::from(spans)
    };
    for key in &keys {
        let here: Vec<&(usize, &Takeaway)> = accepted
            .iter()
            .filter(|(_, t)| t.dest.as_ref() == Some(key))
            .collect();
        if here.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(
            dest_name(app, key),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        )));
        let file = app.todos.get(key);
        // The groups in the order the first takeaway for each names them.
        let mut paths: Vec<Vec<String>> = Vec::new();
        for (_, t) in &here {
            let path = mt::clean_path(&t.group_path);
            if !paths.iter().any(|p| same_path(p, &path)) {
                paths.push(path);
            }
        }
        for path in paths {
            let found = file.and_then(|f| mt::find_group(f, &path));
            let label = match path.is_empty() {
                true => crate::todos::INBOX.to_uppercase(),
                false => path.join(" › ").to_uppercase(),
            };
            let right = match found {
                Some(_) => Vec::new(),
                None => vec![Span::styled("new group", Style::default().fg(th.ok))],
            };
            lines.push(crate::ui::section_rule(&label, th.muted, right, w, th));
            if let (Some(file), Some(group)) = (file, found) {
                let items = file.shown_items(group, &[]);
                for item in items.iter().take(EXISTING) {
                    let (letter, _) = crate::linear::priority_letter(item.priority, th);
                    lines.push(Line::from(Span::styled(
                        format!(
                            "    {letter} {}",
                            crate::ui::truncate(&item.text, w.saturating_sub(8))
                        ),
                        dim,
                    )));
                }
                if items.len() > EXISTING {
                    lines.push(Line::from(Span::styled(
                        format!("    … {} more", items.len() - EXISTING),
                        Style::default().fg(th.faint),
                    )));
                }
            }
            for (i, t) in here
                .iter()
                .filter(|(_, t)| same_path(&mt::clean_path(&t.group_path), &path))
            {
                if *i == cursor {
                    cursor_line = lines.len();
                }
                lines.push(new_line(*i, t));
            }
        }
        lines.push(Line::from(""));
    }
    let nowhere: Vec<&(usize, &Takeaway)> =
        accepted.iter().filter(|(_, t)| t.dest.is_none()).collect();
    if !nowhere.is_empty() {
        lines.push(crate::ui::section_rule(
            "NOWHERE YET",
            th.warn,
            vec![Span::styled("m picks where", dim)],
            w,
            th,
        ));
        for (i, t) in nowhere {
            if *i == cursor {
                cursor_line = lines.len();
            }
            lines.push(new_line(*i, t));
        }
    }
    let note = crate::hints::explain_lines(RIGHT_NOTE, col.width, 2);
    let fits = usize::from(col.height)
        .saturating_sub(note.len() + 1)
        .max(1);
    let first = (cursor_line + 1).saturating_sub(fits);
    for (n, line) in lines.into_iter().skip(first).take(fits).enumerate() {
        if let Some(row) = crate::ui::row_rect(col, n) {
            f.render_widget(Paragraph::new(line), row);
        }
    }
    draw_note(f, col, &note, th);
}

fn same_path(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// The MOVE PICKER, in the right side's place: its title, the filter, a
/// row a group, and `+ new group "<filter>"`.
#[allow(clippy::too_many_arguments)]
fn draw_picker(
    f: &mut Frame,
    app: &App,
    lists: &[PathBuf],
    meeting: &Meeting,
    cursor: usize,
    picker: &Picker,
    col: Rect,
    th: Theme,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let w = usize::from(col.width);
    let dim = Style::default().fg(th.dim);
    let takeaway = meeting.takeaways.get(cursor);
    let text = takeaway.map(|t| t.text.as_str()).unwrap_or_default();
    let title = format!(
        "MOVE \"{}\" TO",
        crate::ui::truncate(text, w.saturating_sub(10))
    );
    if let Some(row) = crate::ui::row_rect(col, 0) {
        f.render_widget(Paragraph::new(Span::styled(title, dim)), row);
    }
    if let Some(row) = crate::ui::row_rect(col, 2) {
        let mut spans = vec![Span::styled("> ", Style::default().fg(th.accent))];
        let field = Rect {
            x: row.x + 2,
            width: row.width.saturating_sub(2),
            ..row
        };
        spans.extend(crate::ui::input_spans(&picker.filter, field, th.accent, th));
        f.render_widget(Paragraph::new(Line::from(spans)), row);
    }
    let rows = spots(app, lists, picker.filter.as_str(), takeaway);
    let note = crate::hints::explain_lines(PICK_NOTE, col.width, 2);
    let fits = usize::from(col.height)
        .saturating_sub(4 + note.len() + 1)
        .max(1);
    let at = picker.cursor.min(rows.len().saturating_sub(1));
    let first = (at + 1).saturating_sub(fits);
    let name_w = rows
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(18);
    let current = takeaway.map(|t| (t.dest.clone(), mt::clean_path(&t.group_path)));
    if rows.is_empty() {
        if let Some(row) = crate::ui::row_rect(col, 4) {
            f.render_widget(
                Paragraph::new(Span::styled("  Type a name for a new group", dim)),
                row,
            );
        }
    }
    for (n, (i, spot)) in rows.iter().enumerate().skip(first).take(fits).enumerate() {
        let Some(row) = crate::ui::row_rect(col, n + 4) else {
            break;
        };
        let lit = |s: Style| match i == at {
            true => s.bg(th.sel_bg).add_modifier(Modifier::BOLD),
            false => s,
        };
        let here = current.as_ref().is_some_and(|(dest, path)| {
            dest.as_ref() == Some(&spot.dest) && same_path(path, &spot.path)
        });
        let name = format!(" {:<name_w$}  ", crate::ui::truncate(&spot.name, name_w));
        let path = match spot.new {
            true => Span::styled(
                format!("+ new group \"{}\"", spot.path.join(" › ")),
                lit(Style::default().fg(th.accent)),
            ),
            false => Span::styled(spot.path.join(" › "), lit(Style::default().fg(th.text))),
        };
        let right = match here {
            true => "where it is now ",
            false => "",
        };
        let line = spread(
            vec![Span::styled(name, lit(Style::default().fg(th.muted))), path],
            Span::styled(right, lit(dim)),
            w,
        );
        f.render_widget(Paragraph::new(line), row);
        hits.push((row, Hit::Spot(i)));
    }
    draw_note(f, col, &note, th);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::meeting_takeaways::Stage;
    use chrono::TimeZone;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn takeaway(text: &str, mine: bool, dest: Option<&str>, path: &[&str]) -> Takeaway {
        Takeaway {
            text: text.into(),
            owner: "AL".into(),
            mine,
            accepted: mine,
            dest: dest.map(PathBuf::from),
            group_path: path.iter().map(|s| s.to_string()).collect(),
            priority: 0,
            ticket: None,
        }
    }

    /// A meeting with three takeaways, two yours, on a screen with Blue
    /// sky's list read in: groups `Product` and `Tools › CLI`.
    fn app_on_screen() -> App {
        let mut app = App::new();
        let mut sky = crate::todos::TodoFile::new(&store::blue_sky_key());
        let product = sky.add_group(None, "Product");
        let tools = sky.add_group(None, "Tools");
        sky.add_group(Some(tools), "CLI");
        sky.add_item(
            product,
            "Old idea",
            chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        );
        app.todos.insert(store::blue_sky_key(), sky);
        let mut m = mt::Meeting::ended(&crate::microsoft::calendar::Event {
            id: "evt-1".into(),
            subject: "Standup".into(),
            start: chrono::Utc.with_ymd_and_hms(2026, 11, 8, 10, 0, 0).unwrap(),
            end: chrono::Utc
                .with_ymd_and_hms(2026, 11, 8, 10, 32, 0)
                .unwrap(),
            online: true,
            join_url: None,
        });
        m.stage = Stage::ToPlace;
        m.takeaways = vec![
            takeaway("A habit tracker", true, Some("blue sky"), &["Product"]),
            takeaway("Theirs", false, Some("blue sky"), &["Product"]),
            takeaway("A CLI for it", true, Some("blue sky"), &["Tools", "CLI"]),
        ];
        app.automations.meetings.kept.meetings.push(m);
        app.overlay = Some(Overlay::Takeaways(Box::new(TakeawaysView {
            meeting: "evt-1".into(),
            lists: vec![store::blue_sky_key()],
            ..TakeawaysView::default()
        })));
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn view(app: &App) -> &TakeawaysView {
        match &app.overlay {
            Some(Overlay::Takeaways(v)) => v,
            other => panic!("not the takeaways screen: {other:?}"),
        }
    }

    fn draw(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn the(app: &App) -> &Meeting {
        app.automations.meetings.meeting("evt-1").unwrap()
    }

    #[test]
    fn the_screen_says_what_5_8_says() {
        let mut app = app_on_screen();
        let text = draw(&mut app);
        assert!(
            text.contains(
                "2 of 3 takeaways are yours and already accepted. Check where they go, then Enter."
            ),
            "{text}"
        );
        assert!(text.contains("FROM THE MEETING"), "{text}");
        assert!(text.contains("WHERE THEY GO IN YOUR TODOS"), "{text}");
        assert!(text.contains("✓ A habit tracker"), "{text}");
        assert!(text.contains("✗ Theirs"), "{text}");
        assert!(text.contains("TOOLS › CLI"), "{text}");
        assert!(text.contains("Old idea"), "existing items show: {text}");
        assert!(text.contains("Enter add 2 to todos"), "{text}");
    }

    #[test]
    fn space_flips_a_takeaway_and_priority_steps() {
        let mut app = app_on_screen();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert!(the(&app).takeaways[1].accepted);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(the(&app).takeaways[1].priority, 1, "none steps to urgent");
        press(&mut app, KeyCode::Esc);
        assert!(app.overlay.is_none(), "Esc: later");
        assert_eq!(the(&app).stage, Stage::ToPlace, "still waiting on you");
    }

    #[test]
    fn the_picker_filters_and_puts_it_in_a_new_group() {
        let mut app = app_on_screen();
        press(&mut app, KeyCode::Char('m'));
        assert!(view(&app).picker.is_some());
        let text = draw(&mut app);
        assert!(text.contains("MOVE \"A habit tracker\" TO"), "{text}");
        assert!(text.contains("where it is now"), "{text}");
        let all = spots(&app, &view(&app).lists, "", the(&app).takeaways.first());
        let names: Vec<String> = all.iter().map(|s| s.path.join(" › ")).collect();
        assert_eq!(names, ["Product", "Tools", "Tools › CLI"]);
        for c in "launch".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        let text = draw(&mut app);
        assert!(text.contains("+ new group \"launch\""), "{text}");
        press(&mut app, KeyCode::Enter);
        assert!(view(&app).picker.is_none());
        assert_eq!(the(&app).takeaways[0].group_path, ["launch"]);
        // Esc leaves it where it was.
        press(&mut app, KeyCode::Char('m'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Esc);
        assert!(view(&app).picker.is_none());
        assert_eq!(the(&app).takeaways[0].group_path, ["launch"]);
    }

    /// INPUT PARITY: a click on a picker row is Enter with the cursor on
    /// it; a click on a takeaway is the arrows landing there.
    #[test]
    fn a_click_is_the_keys() {
        let by_keys = {
            let mut app = app_on_screen();
            press(&mut app, KeyCode::Char('m'));
            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Enter);
            the(&app).takeaways[0].clone()
        };
        let by_mouse = {
            let mut app = app_on_screen();
            press(&mut app, KeyCode::Char('m'));
            draw(&mut app);
            let (rect, _) = view(&app)
                .hits
                .iter()
                .find(|(_, h)| *h == Hit::Spot(2))
                .copied()
                .expect("the third row is on screen");
            let at = Position::new(rect.x + 2, rect.y);
            handle_mouse(
                &mut app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: at.x,
                    row: at.y,
                    modifiers: KeyModifiers::NONE,
                },
                at,
            );
            assert!(view(&app).picker.is_none());
            the(&app).takeaways[0].clone()
        };
        assert_eq!(by_mouse, by_keys);
        assert_eq!(by_keys.group_path, ["Tools", "CLI"]);

        let mut app = app_on_screen();
        draw(&mut app);
        let (rect, _) = *view(&app)
            .hits
            .iter()
            .find(|(_, h)| *h == Hit::Takeaway(2))
            .unwrap();
        let at = Position::new(rect.x + 2, rect.y);
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: at.x,
                row: at.y,
                modifiers: KeyModifiers::NONE,
            },
            at,
        );
        let mut keyed = app_on_screen();
        press(&mut keyed, KeyCode::Down);
        press(&mut keyed, KeyCode::Down);
        assert_eq!(view(&app).cursor, view(&keyed).cursor);
    }

    #[test]
    fn the_hints_come_from_the_key_table() {
        let mut view = TakeawaysView::default();
        crate::hints::assert_hints_from(&hints(&view, 2), keys::ALL);
        view.picker = Some(Picker::default());
        let shown: Vec<Hint> = hints(&view, 2).into_iter().skip(1).collect();
        crate::hints::assert_hints_from(&shown, keys::ALL);
        assert!(keys::ALL.iter().all(crate::hints::Key::parses));
    }
}
