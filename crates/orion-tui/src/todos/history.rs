//! The DONE page: what was ticked today — or this week — across every
//! project, in the shape the lists had it: a section per project in the
//! tabs' order, its groups over what was ticked in them, a nested group
//! under its own. Opened over the TODOS MODAL's list by a click on `✓N
//! today` or `✓N this week` on its project row; `←`/`→` turn between the
//! two, `space` puts an item ticked by mistake back on its list, and Esc
//! goes back to the list.

use std::path::PathBuf;

use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::view::{self, indent, justify, keys, ruled, width_of, TodoView};
use super::{store, Item, TodoFile};
use crate::app::{App, Overlay};
use crate::theme::Theme;

/// The page's own state, on the [`TodoView`] while it is up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DonePage {
    /// This week's, rather than today's.
    pub week: bool,
    /// The cursor's row, by index into the rows as last drawn, and the
    /// first row drawn.
    pub selected: usize,
    pub start: usize,
    /// As last drawn, for the click: the span strip's labels' x-ranges
    /// and its row, the list's area and each drawn row's rect.
    pub span_hits: Vec<(u16, u16)>,
    pub span_row: Rect,
    pub list_area: Rect,
    pub row_rects: Vec<(usize, Rect)>,
}

/// One row of the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A project's section: its name and how many were ticked in it.
    Project { name: String, count: usize },
    /// A group something was ticked in, `depth` in from the project.
    Group {
        dir: PathBuf,
        group: u64,
        depth: u16,
    },
    /// A ticked item, under its group.
    Item { dir: PathBuf, id: u64, depth: u16 },
}

/// The page up over the list, on today's ticks or this `week`'s.
pub(crate) fn open(app: &mut App, week: bool) {
    if let Some(view) = view::view_mut(app) {
        view.page = Some(DonePage {
            week,
            ..DonePage::default()
        });
    }
}

/// Whether `day` falls in the page's span of `today`: the day itself,
/// or — the `week` — Monday to it.
fn in_span(day: NaiveDate, today: NaiveDate, week: bool) -> bool {
    if week {
        (super::monday(today)..=today).contains(&day)
    } else {
        day == today
    }
}

/// How many items were ticked on each day of `today`'s week, Monday
/// first, every project's added up: the project row's summary.
pub(crate) fn week_ticks(app: &App, today: NaiveDate) -> [usize; 7] {
    let mut days = [0; 7];
    for (_, _, dir) in view::projects(app) {
        let Some(file) = app.todos.get(&dir) else {
            continue;
        };
        for (total, n) in days.iter_mut().zip(file.week_ticks(today)) {
            *total += n;
        }
    }
    days
}

/// The page's rows: each project with something ticked in the span, in
/// the tabs' order, over its groups in the order they stand — each over
/// its own items ticked in the span, in the order they were ticked, then
/// its nested groups.
pub(crate) fn rows(app: &App, week: bool, today: NaiveDate) -> Vec<Row> {
    let mut out = Vec::new();
    for (_, name, dir) in view::projects(app) {
        let Some(file) = app.todos.get(&dir) else {
            continue;
        };
        let done = |i: &Item| i.done_on().is_some_and(|d| in_span(d, today, week));
        let count = file.items.iter().filter(|i| done(i)).count();
        if count == 0 {
            continue;
        }
        out.push(Row::Project { name, count });
        add_groups(file, &dir, None, 1, &done, &mut out);
    }
    out
}

fn add_groups(
    file: &TodoFile,
    dir: &PathBuf,
    parent: Option<u64>,
    depth: u16,
    done: &dyn Fn(&Item) -> bool,
    out: &mut Vec<Row>,
) {
    if depth as usize > super::MAX_DEPTH {
        return;
    }
    for group in file.subgroups(parent) {
        let subtree = file.subtree(group.id);
        let any = file
            .items
            .iter()
            .any(|i| subtree.contains(&i.group) && done(i));
        if !any {
            continue;
        }
        out.push(Row::Group {
            dir: dir.clone(),
            group: group.id,
            depth,
        });
        let mut items: Vec<&Item> = file
            .items
            .iter()
            .filter(|i| i.group == group.id && done(i))
            .collect();
        items.sort_by_key(|i| i.done);
        out.extend(items.into_iter().map(|i| Row::Item {
            dir: dir.clone(),
            id: i.id,
            depth: depth + 1,
        }));
        add_groups(file, dir, Some(group.id), depth + 1, done, out);
    }
}

/// The cursor's row in `rows`: an item, the one it was on or the nearest
/// after it — else before it.
fn resolve(selected: usize, rows: &[Row]) -> Option<usize> {
    let at = selected.min(rows.len().checked_sub(1)?);
    let item = |i: &usize| matches!(rows[*i], Row::Item { .. });
    (at..rows.len())
        .find(item)
        .or_else(|| (0..at).rev().find(item))
}

fn page(app: &App) -> Option<&DonePage> {
    view::view(app)?.page.as_ref()
}

fn page_mut(app: &mut App) -> Option<&mut DonePage> {
    view::view_mut(app)?.page.as_mut()
}

/// The rows as they stand now, and the cursor's among them.
fn rows_now(app: &App) -> Option<(Vec<Row>, Option<usize>)> {
    let page = page(app)?;
    let rows = rows(app, page.week, super::today());
    let at = resolve(page.selected, &rows);
    Some((rows, at))
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let page_rows = view::view(app).map_or(1, |v| v.list_area.height.max(1)) as i32;
    match key.code {
        KeyCode::Esc => {
            if let Some(view) = view::view_mut(app) {
                view.page = None;
            }
        }
        _ if keys::SPAN.matches(&key) => {
            let week = page(app).is_some_and(|p| !p.week);
            open(app, week);
        }
        _ if keys::UNDO.matches(&key) => undo(app),
        KeyCode::Down => step(app, 1),
        KeyCode::Up => step(app, -1),
        KeyCode::PageDown => step(app, page_rows),
        KeyCode::PageUp => step(app, -page_rows),
        KeyCode::Home => step(app, i32::MIN / 2),
        KeyCode::End => step(app, i32::MAX / 2),
        _ => {}
    }
}

/// The cursor `delta` items on, over the headers.
fn step(app: &mut App, delta: i32) {
    let Some((rows, Some(at))) = rows_now(app) else {
        return;
    };
    let items: Vec<usize> = (0..rows.len())
        .filter(|i| matches!(rows[*i], Row::Item { .. }))
        .collect();
    let here = items.iter().position(|i| *i == at).unwrap_or(0) as i64;
    let next = (here + i64::from(delta)).clamp(0, items.len() as i64 - 1) as usize;
    if let Some(page) = page_mut(app) {
        page.selected = items[next];
    }
}

/// `space`: the cursor's item open again, back on its list where it
/// stood — off the page, the next one coming up under the cursor.
fn undo(app: &mut App) {
    let Some((rows, Some(at))) = rows_now(app) else {
        return;
    };
    let Row::Item { dir, id, .. } = &rows[at] else {
        return;
    };
    let Some(file) = app.todos.get_mut(dir) else {
        return;
    };
    let Some(item) = file.item_mut(*id) else {
        return;
    };
    item.done = None;
    let text = item.text.clone();
    store::save(file);
    app.dirty = true;
    app.flash = Some(crate::flash::Flash::done(format!(
        "back on the list: {text}"
    )));
}

pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) {
    let Some(page) = page(app) else {
        return;
    };
    let list = page.list_area;
    match mouse.kind {
        MouseEventKind::ScrollDown if list.contains(pos) => step(app, 1),
        MouseEventKind::ScrollUp if list.contains(pos) => step(app, -1),
        MouseEventKind::Down(MouseButton::Left) if page.span_row.contains(pos) => {
            if let Some(hit) = crate::ui::tab_hit(&page.span_hits, pos.x) {
                open(app, hit == 1);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if list.contains(pos) => {
            let Some(row) = crate::ui::row_hit(&page.row_rects, pos) else {
                return;
            };
            let Some((rows, _)) = rows_now(app) else {
                return;
            };
            if matches!(rows.get(row), Some(Row::Item { .. })) {
                if let Some(page) = page_mut(app) {
                    page.selected = row;
                }
            }
        }
        _ => {}
    }
}

/// The page in `inner`, the modal's frame drawn round it: the span strip
/// — `Today`, `This week` — then the rows, the cursor's lit.
pub(crate) fn draw(
    f: &mut Frame,
    app: &mut App,
    view: &TodoView,
    inner: Rect,
    th: Theme,
    focused: bool,
) {
    let Some(page) = view.page.clone() else {
        return;
    };
    let today = super::today();
    let days = week_ticks(app, today);
    let at_today = {
        use chrono::Datelike;
        days[today.weekday().num_days_from_monday() as usize]
    };
    let labels = [
        format!("Today ✓{at_today}"),
        format!("This week ✓{}", days.iter().sum::<usize>()),
    ];
    let span_row = crate::ui::row_rect(inner, 0).unwrap_or_default();
    let (strip, span_hits) = crate::ui::tab_strip(
        span_row.x,
        span_row.width,
        labels.iter().map(String::as_str),
        usize::from(page.week),
        false,
        th,
    );
    f.render_widget(Paragraph::new(Line::from(strip)), span_row);
    let since = if page.week {
        format!("since {}", view::day_label(super::monday(today)))
    } else {
        view::day_label(today)
    };
    let right = Rect {
        width: span_row.width.saturating_sub(1),
        ..span_row
    };
    let line = Line::from(Span::styled(since, Style::default().fg(th.dim)))
        .alignment(ratatui::layout::Alignment::Right);
    f.render_widget(Paragraph::new(line), right);

    let list_area = crate::ui::below_first_row(crate::ui::below_first_row(inner));
    let rows = rows(app, page.week, today);
    let cursor = resolve(page.selected, &rows);
    let mut row_rects = Vec::new();
    let mut start = 0;
    if rows.is_empty() {
        let words = if page.week {
            "nothing ticked this week yet"
        } else {
            "nothing ticked today yet"
        };
        if let Some(line) = crate::ui::row_rect(list_area, 0) {
            let text = Span::styled(words, Style::default().fg(th.dim));
            f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), text])), line);
        }
    } else {
        let budget = (list_area.width as usize).saturating_sub(2);
        // A project past the first stands a blank line below what is over it.
        let heights: Vec<u16> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| 1 + u16::from(i > 0 && matches!(r, Row::Project { .. })))
            .collect();
        let at = cursor.unwrap_or(0);
        let (first, drawn) = crate::ui::stacked_rows(&heights, at, at, page.start, list_area);
        start = first;
        for (i, rect) in drawn {
            let rect = if heights[i] > 1 {
                Rect {
                    y: rect.y + 1,
                    height: rect.height.saturating_sub(1),
                    ..rect
                }
            } else {
                rect
            };
            if rect.height == 0 {
                continue;
            }
            let spans = row_spans(app, &rows[i], page.week, budget, th);
            let at_cursor = cursor == Some(i);
            crate::ui::render_row(f, rect, spans, at_cursor, at_cursor && focused, th);
            row_rects.push((i, rect));
        }
    }
    if let Some(Overlay::Todos(v)) = &mut app.overlay {
        if let Some(p) = &mut v.page {
            p.span_hits = span_hits;
            p.span_row = span_row;
            p.list_area = list_area;
            p.row_rects = row_rects;
            p.start = start;
            if let Some(at) = cursor {
                p.selected = at;
            }
        }
        v.list_area = list_area;
    }
}

/// A row's line: a project a section as a top-level group is — its name
/// in capitals, a rule across, how many were ticked; a group its name; an
/// item its box, priority and text, and when it was ticked against the
/// right edge — the time today, the day this week.
fn row_spans(app: &App, row: &Row, week: bool, budget: usize, th: Theme) -> Vec<Span<'static>> {
    match row {
        Row::Project { name, count } => {
            let left = vec![Span::styled(
                name.to_uppercase(),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            )];
            let right = vec![Span::styled(
                format!("✓{count}"),
                Style::default().fg(th.ok),
            )];
            ruled(left, right, budget, th)
        }
        Row::Group { dir, group, depth } => {
            let name = app
                .todos
                .get(dir)
                .and_then(|f| f.group(*group))
                .map_or_else(String::new, |g| g.name.clone());
            let color = if *depth == 1 { th.text } else { th.muted };
            vec![
                indent(*depth),
                Span::styled(
                    crate::ui::truncate(&name, budget.saturating_sub(2 * *depth as usize)),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]
        }
        Row::Item { dir, id, depth } => {
            let Some(item) = app.todos.get(dir).and_then(|f| f.item(*id)) else {
                return Vec::new();
            };
            let when = item.done.map_or_else(String::new, |at| {
                if week {
                    view::day_label(at.date_naive())
                } else {
                    at.format("%H:%M").to_string()
                }
            });
            let left = vec![
                indent(*depth),
                Span::styled("☑ ", Style::default().fg(th.ok)),
                crate::linear::priority_mark(item.priority, th),
                Span::raw(" "),
            ];
            let right = vec![Span::styled(when, Style::default().fg(th.faint))];
            let room = budget.saturating_sub(width_of(&left) + width_of(&right) + 1);
            let text = vec![Span::styled(
                crate::ui::truncate(&item.text, room),
                Style::default().fg(th.muted),
            )];
            justify(left, text, right, budget)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(d: u32, h: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 10, d, h, 0, 0)
            .unwrap()
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    /// Today is the day itself; the week runs from its Monday to it.
    #[test]
    fn the_span_is_the_day_or_its_week_so_far() {
        // 2026-10-07 is a Wednesday.
        assert!(in_span(day(7), day(7), false));
        assert!(!in_span(day(6), day(7), false));
        assert!(in_span(day(5), day(7), true));
        assert!(!in_span(day(4), day(7), true), "last week's Sunday");
        assert!(!in_span(day(8), day(7), true));
    }

    /// The groups something was ticked in, in the order they stand, each
    /// over its own ticks in tick order and then its nested groups — a
    /// group with nothing ticked in the span left out.
    #[test]
    fn a_project_keeps_its_shape() {
        let mut file = TodoFile::new(std::path::Path::new("/r"));
        let ui = file.add_group(None, "UI");
        let later = file.add_group(Some(ui), "Later");
        let quiet = file.add_group(None, "Quiet");
        let a = file.add_item(ui, "a", day(1));
        let b = file.add_item(ui, "b", day(1));
        let c = file.add_item(later, "c", day(1));
        let old = file.add_item(quiet, "old", day(1));
        file.toggle(b, at(7, 9));
        file.toggle(a, at(7, 10));
        file.toggle(c, at(6, 9));
        file.toggle(old, at(1, 9));
        let mut out = Vec::new();
        let dir = PathBuf::from("/r");
        let done = |i: &Item| i.done_on().is_some_and(|d| in_span(d, day(7), true));
        add_groups(&file, &dir, None, 1, &done, &mut out);
        let item = |id, depth| Row::Item {
            dir: dir.clone(),
            id,
            depth,
        };
        let group = |group, depth| Row::Group {
            dir: dir.clone(),
            group,
            depth,
        };
        assert_eq!(
            out,
            [
                group(ui, 1),
                item(b, 2),
                item(a, 2),
                group(later, 2),
                item(c, 3)
            ]
        );
    }
}
