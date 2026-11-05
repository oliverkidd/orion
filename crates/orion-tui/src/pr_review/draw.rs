//! The PR REVIEW on screen: the files on the left with their thread
//! counts, the file's diff on the right with its threads under their
//! lines, the keys along the bottom edge.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::github::{ReviewThread, ThreadComment};
use super::layout::{self, Item, Stop};
use super::{Focus, ReviewFile, ReviewScreen};
use crate::app::{App, Overlay};
use crate::markdown::Breaks;
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, count_style, empty_list_row, panel_block, render_row, row_rect, truncate,
    truncate_cells, truncate_left, SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
};

/// The file column's share of the modal, and its floor.
const FILES_PCT: u16 = 28;
const MIN_FILES_W: u16 = 24;
/// The cell left of the diff the cursor's `▌` stands in.
const MARGIN: usize = 1;
/// The rule a comment hangs on, and the cell after it.
const RULE: &str = "│ ";
/// The fewest cells a comment's text wraps in, however deep the gutter.
const MIN_TEXT_W: usize = 16;

/// What the diff pane laid out at the last draw, for the keys and the
/// mouse to read back.
struct Laid {
    lines: Vec<Line<'static>>,
    scroll: usize,
    stops: Vec<(Stop, usize, usize)>,
    cursor: Option<Stop>,
}

/// Draw the screen and write back where everything went (`view` is a
/// clone of `app.overlay`'s).
pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &ReviewScreen, th: Theme) {
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let files_w = (area.width * FILES_PCT / 100)
        .max(MIN_FILES_W)
        .min(area.width.saturating_sub(SPLIT_PANE_LAYOUT_MIN));
    let [files_a, diff_a] = Layout::horizontal([
        Constraint::Length(files_w),
        Constraint::Min(SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);

    let (files_inner, file_rects) = draw_files(f, view, files_a, th);
    let (diff_inner, laid) = draw_diff(f, view, diff_a, th);
    crate::hints::draw_on_border(f, area, &super::hints(view), 0, th);

    if let Some(Overlay::Review(v)) = &mut app.overlay {
        v.area = area;
        v.files_area = files_inner;
        v.diff_area = diff_inner;
        v.file_rects = file_rects;
        v.diff_h = diff_inner.height;
        if let Some(laid) = laid {
            v.scroll = laid.scroll;
            v.stop_rows = laid.stops;
            v.cursor = laid.cursor;
            v.follow = false;
        }
    }
}

/// How many threads are on `path`, and whether any is still open — the
/// file column's count.
fn thread_count(view: &ReviewScreen, path: &str, th: Theme) -> Vec<Span<'static>> {
    let threads: Vec<&ReviewThread> = view.threads_on(path).collect();
    if threads.is_empty() {
        return Vec::new();
    }
    let open = threads.iter().filter(|t| !t.resolved).count();
    let pending = threads.iter().any(|t| t.pending_count() > 0);
    let mut spans = Vec::new();
    if pending {
        spans.push(Span::styled("● ", Style::default().fg(th.accent)));
    }
    spans.push(if open == 0 {
        Span::styled(format!("✓{}", threads.len()), Style::default().fg(th.dim))
    } else {
        Span::styled(
            threads.len().to_string(),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        )
    });
    spans
}

/// What the panes say while there is no file to show: the diff still being
/// read, why it couldn't be — `why` the diff pane's to say — or a pull
/// request that changes nothing.
fn nothing_to_show(view: &ReviewScreen, why: bool) -> Option<String> {
    match &view.files {
        None => Some("reading the diff…".to_string()),
        Some(Err(err)) if why => Some(format!("couldn't read the diff: {err}")),
        Some(Err(_)) => Some("couldn't read the diff".to_string()),
        Some(Ok(files)) if files.is_empty() => Some("no files changed".to_string()),
        Some(Ok(_)) => None,
    }
}

/// The file column: each file's status, its path and its thread count.
fn draw_files(
    f: &mut Frame,
    view: &ReviewScreen,
    area: Rect,
    th: Theme,
) -> (Rect, Vec<(Rect, usize)>) {
    let focused = view.focus == Focus::Files;
    let files = view.file_list();
    let mut title = format!("Review {}", view.label);
    if view.reading {
        title.push_str(" · reading threads…");
    } else if !files.is_empty() {
        title.push_str(&format!(" · {} files", files.len()));
    }
    let title = truncate(&title, usize::from(area.width.saturating_sub(4)));
    let block = panel_block(&title, focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Some(say) = nothing_to_show(view, false) {
        empty_list_row(f, inner, &say, th);
    }
    let height = usize::from(inner.height);
    let top = crate::app::window_start(view.file, height);
    let width = usize::from(inner.width).saturating_sub(1);
    let mut rects = Vec::new();
    for (row, (i, file)) in files.iter().enumerate().skip(top).enumerate() {
        let Some(rect) = row_rect(inner, row) else {
            break;
        };
        let count = thread_count(view, &file.path, th);
        let count_w: usize = count.iter().map(|s| s.content.width()).sum();
        let status = Span::styled(
            format!("{} ", file.status),
            Style::default().fg(crate::ui::change_color([file.status, ' '], th)),
        );
        let room = width.saturating_sub(2 + count_w + usize::from(count_w > 0));
        let path = truncate_cells(&file.path, room);
        let gap = width.saturating_sub(2 + path.width() + count_w);
        let mut spans = vec![status, Span::raw(path), Span::raw(" ".repeat(gap))];
        spans.extend(count);
        render_row(f, rect, spans, i == view.file, focused, th);
        rects.push((rect, i));
    }
    (inner, rects)
}

/// The diff pane: the file's diff with its threads, or what is keeping it.
fn draw_diff(f: &mut Frame, view: &ReviewScreen, area: Rect, th: Theme) -> (Rect, Option<Laid>) {
    let focused = view.focus == Focus::Diff;
    let file = view.current();
    let counts = file.and_then(|file| file.lines).map(|lines| {
        Line::from(vec![
            Span::styled(
                format!(" +{}", lines.added),
                count_style(lines.added, th.added, th),
            ),
            Span::styled(
                format!(" −{} ", lines.removed),
                count_style(lines.removed, th.removed, th),
            ),
        ])
    });
    // The path, cut from its root, clear of the counts at the right.
    let room = usize::from(area.width).saturating_sub(8 + counts.as_ref().map_or(0, Line::width));
    let title = file.map_or_else(String::new, |file| truncate_left(&file.path, room));
    let mut block = panel_block(&title, focused, th);
    if let Some(counts) = counts {
        block = block.title_top(counts.right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(file) = file else {
        let say = nothing_to_show(view, true).unwrap_or_default();
        empty_list_row(f, inner, &say, th);
        return (inner, None);
    };
    let mut laid = lay_out(view, file, inner.width, inner.height, focused, th);
    f.render_widget(Paragraph::new(std::mem::take(&mut laid.lines)), inner);
    (inner, Some(laid))
}

/// The file's pane laid out `width` × `height`: every item's rows, the
/// window of them the scroll — brought round to the cursor when it moved —
/// shows, the cursor's `▌` beside its rows.
fn lay_out(
    view: &ReviewScreen,
    file: &ReviewFile,
    width: u16,
    height: u16,
    focused: bool,
    th: Theme,
) -> Laid {
    let content_w = width.saturating_sub(MARGIN as u16).max(1);
    let indent = file
        .doc
        .gutter_width(content_w)
        .min(usize::from(content_w) / 3);
    let items = view.items_of(file);
    let item_stops: Vec<Option<Stop>> = items
        .iter()
        .map(|item| layout::stop_of(*item, &file.doc, &view.threads))
        .collect();
    // The cursor, or the first stop when it rests on none of these.
    let cursor = view
        .cursor
        .clone()
        .filter(|c| item_stops.iter().flatten().any(|s| s == c))
        .or_else(|| item_stops.iter().flatten().next().cloned());
    let line_rows = file.doc.row_counts(content_w);
    let now = orion_core::clock::now_ms() / 1000;
    // Every item's rows: a line of the diff counted without being drawn, a
    // thread drawn now — there are few.
    let mut drawn: Vec<Option<Vec<Line<'static>>>> = Vec::with_capacity(items.len());
    let mut counts = Vec::with_capacity(items.len());
    let mut stops = Vec::new();
    // Where each stop's lead starts: the rows since the stop before it —
    // a hunk's line over its first line of code — scrolled into view with
    // it when the cursor goes up to it.
    let mut leads = Vec::new();
    let mut lead = 0;
    let mut row = 0;
    for (item, stop) in items.iter().zip(&item_stops) {
        let lines = match item {
            Item::Line(_) => None,
            Item::Thread(t) => {
                let here = stop.is_some() && *stop == cursor;
                Some(thread_lines(
                    &view.threads[*t],
                    usize::from(content_w),
                    indent,
                    here,
                    now,
                    th,
                ))
            }
            Item::Composer => Some(Vec::new()),
        };
        let n = match (&lines, item) {
            (Some(lines), _) => lines.len(),
            (None, Item::Line(i)) => line_rows[*i] as usize,
            (None, _) => 0,
        };
        if let Some(stop) = stop {
            stops.push((stop.clone(), row, n));
            leads.push(lead);
            lead = row + n;
        }
        drawn.push(lines);
        counts.push(n);
        row += n;
    }
    let total = row;
    let height = usize::from(height.max(1));
    let mut scroll = view.scroll.min(total.saturating_sub(height));
    let at_cursor = stops
        .iter()
        .zip(&leads)
        .find(|((s, ..), _)| Some(s) == cursor.as_ref());
    if let (true, Some(((_, first, n), lead))) = (view.follow, at_cursor) {
        if *first < scroll {
            scroll = if first + n - lead <= height {
                *lead
            } else {
                *first
            };
        } else if first + n > scroll + height {
            scroll = (first + n).saturating_sub(height).min(*first);
        }
    }
    let mut lines = Vec::with_capacity(height);
    let mut at = 0;
    for (k, item) in items.iter().enumerate() {
        let n = counts[k];
        if at + n <= scroll {
            at += n;
            continue;
        }
        if at >= scroll + height {
            break;
        }
        let rows = match (drawn[k].take(), item) {
            (Some(rows), _) => rows,
            (None, Item::Line(i)) => file.doc.render_line(*i, content_w, th),
            (None, _) => Vec::new(),
        };
        let here = cursor.is_some() && item_stops[k] == cursor;
        let mark = if here {
            Span::styled(
                "▌",
                Style::default().fg(if focused { th.accent } else { th.dim }),
            )
        } else {
            Span::raw(" ")
        };
        for (r, line) in rows.into_iter().enumerate() {
            let y = at + r;
            if y < scroll || y >= scroll + height {
                continue;
            }
            let mut spans = vec![mark.clone()];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        at += n;
    }
    Laid {
        lines,
        scroll,
        stops,
        cursor,
    }
}

/// `2h`: how long ago `at` (RFC 3339) was.
fn ago(at: &str, now: i64) -> String {
    crate::pull_request::rfc3339_secs(at)
        .map(|at| crate::hosts::ago_short(now - at))
        .unwrap_or_default()
}

/// The rule a comment hangs on: the accent for yours, grey for theirs.
fn rule(c: &ThreadComment, indent: usize, th: Theme) -> Vec<Span<'static>> {
    let color = if c.mine { th.accent } else { th.dim };
    vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(RULE, Style::default().fg(color)),
    ]
}

/// One thread as the block under its line: each comment its author, how
/// long ago and its body as markdown, hung on its rule. A resolved thread
/// is one row until the cursor rests on it.
fn thread_lines(
    t: &ReviewThread,
    width: usize,
    indent: usize,
    here: bool,
    now: i64,
    th: Theme,
) -> Vec<Line<'static>> {
    let text_w = width.saturating_sub(indent + RULE.width()).max(MIN_TEXT_W);
    let dim = Style::default().fg(th.dim);
    let Some(first) = t.comments.first() else {
        return Vec::new();
    };
    if t.resolved && !here {
        let said = first.body.lines().next().unwrap_or_default();
        let mut spans = rule(first, indent, th);
        spans.push(Span::styled("✓ resolved · ", Style::default().fg(th.ok)));
        spans.push(Span::styled(format!("{}: {said}", first.author), dim));
        return vec![crate::pr_preview::fit(spans, width)];
    }
    // What the thread is beside what is said in it: where it was, when a
    // push left it behind or it runs over lines, and how it stands.
    let mut tags: Vec<(String, Style)> = Vec::new();
    if t.outdated || t.line.is_none() || t.start_line.is_some() {
        tags.push((t.place(), dim));
    }
    if t.outdated {
        tags.push(("outdated".into(), Style::default().fg(th.warn)));
    }
    if t.resolved {
        tags.push(("resolved".into(), Style::default().fg(th.ok)));
    }
    let mut out = Vec::new();
    for (n, c) in t.comments.iter().enumerate() {
        let mut head = rule(c, indent, th);
        head.push(Span::styled(
            c.author.clone(),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ));
        if c.pending {
            head.push(Span::styled(" · pending", Style::default().fg(th.accent)));
        } else {
            head.push(Span::styled(format!(" · {}", ago(&c.at, now)), dim));
        }
        if n == 0 {
            for (tag, style) in &tags {
                head.push(Span::styled(" · ", dim));
                head.push(Span::styled(tag.clone(), *style));
            }
        }
        out.push(crate::pr_preview::fit(head, width));
        let base = Style::default().fg(th.text);
        for line in crate::markdown::render(c.body.trim_end(), text_w, Breaks::Hard, base, th) {
            let mut spans = rule(c, indent, th);
            spans.extend(line.spans);
            out.push(Line::from(spans));
        }
    }
    out
}
