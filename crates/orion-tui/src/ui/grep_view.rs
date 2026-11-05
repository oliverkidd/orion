//! FIND IN FILES (`⌘⇧F`) on screen.
//!
//! The query on the first row, then the hits: a header per file — its path
//! and how many hits it has — over a row per hit, its line number and the
//! line. Beside them, when the modal is wide enough, the selected hit's file
//! as the TREE BROWSER previews it — numbered and highlighted — with the
//! hit's line lifted off the background a third of the way down the pane
//! and every match of the query in view lit, the way `git grep -o` found it.

use super::{
    below_first_row, centered_rect_pct, editor_name, finder_keys, modal_block, render_row,
    row_rect, search_line, token_style, truncate, ListEntry, GREP_MODAL_PCT, NO_MATCHES,
};
use crate::app::{App, GrepView, Overlay};
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// The narrowest inner width that splits the modal into the list and the
/// preview; under it the list has the modal to itself.
const PREVIEW_MIN_W: u16 = 90;
/// The list's share of a split modal, in percent.
const LIST_PCT: u16 = 45;
/// How far down the preview pane the hit's line sits: a third of the way,
/// more of what follows it in view than of what leads up to it.
const HIT_AT_DIVISOR: usize = 3;

pub(super) fn draw(f: &mut Frame, app: &mut App, view: &GrepView, th: Theme) {
    let area = centered_rect_pct(f.area(), GREP_MODAL_PCT.0, GREP_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let block = crate::hints::modal_block(
        modal_block(title(view), th),
        &[
            finder_keys::OPEN.hint_as(format!("edit in {}", editor_name(&view.editor))),
            finder_keys::REGEX.hint_as(if view.mode.regex { "text" } else { "regex" }),
            crate::hints::COPY_PATH.hint(),
            crate::hints::in_app_hint(),
            crate::hints::ESC_CLOSE.hint(),
        ],
        area.width,
        th,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    // First row: the always-live grep query.
    if let Some(query_area) = row_rect(inner, 0) {
        let placeholder = if view.mode.regex {
            "type a regex…"
        } else {
            "type to search…"
        };
        let line = search_line(&view.query, placeholder, query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let body = below_first_row(inner);
    let (list, preview) = if body.width >= PREVIEW_MIN_W {
        let list_w = body.width * LIST_PCT / 100;
        let list = Rect {
            width: list_w,
            ..body
        };
        let preview = Rect {
            x: body.x + list_w + 1,
            width: body.width.saturating_sub(list_w + 1),
            ..body
        };
        let rule = (0..body.height)
            .map(|_| Line::from(Span::styled("│", Style::default().fg(th.edge))))
            .collect::<Vec<_>>();
        f.render_widget(
            Paragraph::new(rule),
            Rect {
                x: body.x + list_w,
                width: 1,
                ..body
            },
        );
        (list, Some(preview))
    } else {
        (body, None)
    };
    draw_list(f, view, list, th);
    if let Some(pane) = preview {
        draw_preview(f, view, pane, th);
    }

    // Write-back (draw works on a clone): rects for mouse hit-testing.
    if let Some(Overlay::Grep(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = list;
    }
}

/// The modal's title: the branch, how the query is read, and how many hits.
fn title(view: &GrepView) -> String {
    let how = if view.mode.regex { " (regex)" } else { "" };
    let count = if view.too_short() {
        String::new()
    } else if view.waiting.is_some() {
        " (searching…)".to_string()
    } else if view.truncated {
        format!(" ({}+ hits)", view.hits.len())
    } else {
        format!(" ({} hits)", view.hits.len())
    };
    format!(" Find in files{how} — {}{count} ", view.branch)
}

/// The hits, a header over each file's.
fn draw_list(f: &mut Frame, view: &GrepView, list: Rect, th: Theme) {
    // Placeholder row: error, too-short query, or an empty result.
    let placeholder = if let Some(err) = &view.error {
        Some(Span::styled(err.clone(), Style::default().fg(th.err)))
    } else if view.too_short() {
        Some(Span::styled(
            format!(
                "type at least {} characters to search",
                crate::grep_search::MIN_QUERY_LEN
            ),
            Style::default().fg(th.dim),
        ))
    } else if view.hits.is_empty() && view.waiting.is_none() {
        Some(Span::styled(NO_MATCHES, Style::default().fg(th.dim)))
    } else {
        None
    };
    if let (Some(span), Some(row_area)) = (placeholder, row_rect(list, 0)) {
        f.render_widget(Paragraph::new(span), row_area);
    }

    let number_w = view
        .hits
        .iter()
        .map(|hit| hit.line.to_string().len())
        .max()
        .unwrap_or(1);
    let budget = (list.width as usize).saturating_sub(2);
    let rows = view.rows();
    let selected = rows
        .iter()
        .position(|entry| *entry == ListEntry::Row(view.selected))
        .unwrap_or(0);
    let start = crate::app::window_start(selected, list.height as usize);
    for (n, entry) in rows.iter().enumerate().skip(start) {
        let Some(row_area) = row_rect(list, n - start) else {
            break;
        };
        match *entry {
            ListEntry::Header { first, count } => {
                let path = &view.hits[first].path;
                let tail = format!("  {count}");
                let path_w = budget.saturating_sub(tail.chars().count());
                let spans = vec![
                    Span::raw(" "),
                    Span::styled(
                        truncate(path, path_w),
                        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(tail, Style::default().fg(th.dim)),
                ];
                f.render_widget(Paragraph::new(Line::from(spans)), row_area);
            }
            ListEntry::Row(i) => {
                let hit = &view.hits[i];
                let number = format!("   {:>number_w$}  ", hit.line);
                let text_w = budget.saturating_sub(number.chars().count());
                let spans = vec![
                    Span::styled(number, Style::default().fg(th.dim)),
                    Span::raw(truncate(&hit.text, text_w)),
                ];
                render_row(f, row_area, spans, i == view.selected, true, th);
            }
        }
    }
}

/// The selected hit's file around its line, numbered and highlighted, the
/// hit's line lifted and every match in view lit.
fn draw_preview(f: &mut Frame, view: &GrepView, pane: Rect, th: Theme) {
    let (Some(hit), Some(read)) = (view.selected_hit(), &view.preview) else {
        return;
    };
    if read.path != hit.path || pane.height == 0 {
        return;
    }
    // A file there is nothing to show of — binary, gone — says so, dim.
    if !read.file.is_file {
        let note = Span::styled(read.file.text.clone(), Style::default().fg(th.dim));
        f.render_widget(Paragraph::new(note), pane);
        return;
    }
    let lines = &read.file.lines;
    let height = pane.height as usize;
    let at = (hit.line as usize).saturating_sub(1);
    let top = at
        .saturating_sub(height / HIT_AT_DIVISOR)
        .min(lines.len().saturating_sub(height));
    let gutter = lines.len().max(1).to_string().len() + 1;
    let lit = Style::default().bg(th.sel_bg).add_modifier(Modifier::BOLD);
    // The matches are in line order: the ones on screen are one run — and
    // none while they are of a query typed over since.
    let current = read.query == view.query.as_str() && read.mode == view.mode;
    let first = read
        .matches
        .partition_point(|(line, _)| *line <= top as u64);
    let shown = if current { &read.matches[first..] } else { &[] };
    let mut matches = shown.iter().peekable();
    let texts = read.file.text.lines().skip(top);
    for (row, (index, text)) in (top..lines.len()).zip(texts).take(height).enumerate() {
        let mut spans = vec![Span::styled(
            format!("{:>w$} ", index + 1, w = gutter - 1),
            Style::default().fg(th.edge),
        )];
        spans.extend(
            lines[index]
                .iter()
                .map(|(kind, text)| Span::styled(text.as_str(), token_style(*kind, th))),
        );
        let mut line = Line::from(spans);
        if index == at {
            line = line.style(Style::default().bg(th.sel_bg_dim));
        }
        let row_area = Rect {
            y: pane.y + row as u16,
            height: 1,
            ..pane
        };
        f.render_widget(Paragraph::new(line), row_area);
        // Each match's columns, measured on from the one before it, and
        // none past the pane's edge.
        let (mut byte, mut col) = (0, gutter);
        while let Some((_, range)) = matches.next_if(|(line, _)| *line == index as u64 + 1) {
            let width = |from: usize, to: usize| text.get(from..to).map_or(0, |s| s.width());
            let from = col + width(byte, range.start);
            let to = from + width(range.start, range.end);
            (byte, col) = (range.end, to);
            if from >= pane.width as usize {
                continue;
            }
            let cells = Rect {
                x: pane.x + from as u16,
                width: (to.min(pane.width as usize) - from) as u16,
                ..row_area
            };
            f.buffer_mut().set_style(cells, lit);
        }
    }
}
