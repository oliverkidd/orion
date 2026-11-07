//! The PR REVIEW's parts of the DIFF VIEWER's screen (`pr_review::viewer`
//! puts them there): the file's diff with its threads under their lines,
//! each file's thread count for the file list, and the bar along the
//! bottom.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::bar::BarRow;
use super::github;
use super::github::{ReviewThread, ThreadComment};
use super::layout::{self, Item, Stop};
use super::write::{Composer, Target};
use super::{ReviewFile, ReviewScreen};
use crate::markdown::Breaks;
use crate::text_input::TextView;
use crate::theme::Theme;
/// The cell left of the diff the cursor's `▌` stands in.
const MARGIN: usize = 1;
/// The rule a comment hangs on, and the cell after it.
const RULE: &str = "│ ";
/// The fewest cells a comment's text wraps in, however deep the gutter.
const MIN_TEXT_W: usize = 16;
/// The most rows the summary shows before it scrolls.
const SUMMARY_ROWS: usize = 3;
/// What sits between two of the bar's verdicts.
const VERDICT_GAP: &str = "   ";

/// What the diff pane laid out at the last draw, for the keys and the
/// mouse to read back.
pub(crate) struct Laid {
    lines: Vec<Line<'static>>,
    pub scroll: usize,
    pub stops: Vec<(Stop, usize, usize)>,
    pub cursor: Option<Stop>,
    /// The cells the comment being written wraps in, and its rows.
    pub composer: Option<TextView>,
}

/// The file on screen's diff with its threads, drawn into `inner` — the
/// DIFF VIEWER's reading pane, inside its border. None with no file.
pub(crate) fn draw_body(
    f: &mut Frame,
    view: &ReviewScreen,
    inner: Rect,
    focused: bool,
    th: Theme,
) -> Option<Laid> {
    let file = view.current()?;
    let mut laid = lay_out(view, file, inner.width, inner.height, focused, th);
    f.render_widget(Paragraph::new(std::mem::take(&mut laid.lines)), inner);
    Some(laid)
}

/// How many threads are on `path`, and whether any is still open — the
/// file list's count, right of the path.
pub(crate) fn thread_count(view: &ReviewScreen, path: &str, th: Theme) -> Vec<Span<'static>> {
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
    let word = if open == 0 {
        Span::styled(format!("✓{}", threads.len()), Style::default().fg(th.dim))
    } else {
        Span::styled(
            format!("✎{open}"),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        )
    };
    spans.push(word);
    spans.push(Span::raw(" "));
    spans
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
    let marked = view.marked_lines();
    let now = orion_core::clock::now_ms() / 1000;
    let mut composer_at = None;
    let mut composer_view = None;
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
            Item::Composer => view.composer.as_ref().map(|composer| {
                let (rows, at) = composer_lines(composer, view, usize::from(content_w), indent, th);
                composer_view = Some(at);
                rows
            }),
        };
        let n = match (&lines, item) {
            (Some(lines), _) => lines.len(),
            (None, Item::Line(i)) => line_rows[*i] as usize,
            (None, _) => 0,
        };
        if *item == Item::Composer {
            composer_at = Some((row, n, lead));
        }
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
    // What has to be in view: the comment being written, else the cursor.
    let at_cursor = composer_at.or_else(|| {
        stops
            .iter()
            .zip(&leads)
            .find(|((s, ..), _)| Some(s) == cursor.as_ref())
            .map(|((_, first, n), lead)| (*first, *n, *lead))
    });
    if let (true, Some((first, n, lead))) = (view.follow, at_cursor) {
        let (first, n, lead) = (&first, &n, &lead);
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
        let in_marked =
            matches!((item, marked), (Item::Line(i), Some((a, b))) if (a..=b).contains(i));
        let here = cursor.is_some() && item_stops[k] == cursor && view.composer.is_none();
        let mark = if here || in_marked {
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
        composer: composer_view,
    }
}

/// The comment being written, as a block in the diff: on the accent's rule
/// what it is on — the lines, or the thread it answers — then the live
/// field, every row of it. And the cells the field was laid out in, for
/// its ↑/↓.
fn composer_lines(
    composer: &Composer,
    view: &ReviewScreen,
    width: usize,
    indent: usize,
    th: Theme,
) -> (Vec<Line<'static>>, TextView) {
    let text_w = width.saturating_sub(indent + RULE.width()).max(MIN_TEXT_W);
    let lead = || {
        vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(RULE, Style::default().fg(th.accent)),
        ]
    };
    let on = match &composer.target {
        Target::Lines { start, end, .. } if start != end => {
            let line = |i: usize| {
                view.current()
                    .and_then(|f| layout::side_line(&f.doc, i))
                    .map_or(0, |(_, n)| n)
            };
            format!("Comment on lines {}–{}", line(*start), line(*end))
        }
        Target::Lines { end, .. } => {
            let line = view
                .current()
                .and_then(|f| layout::side_line(&f.doc, *end))
                .map_or(0, |(_, n)| n);
            format!("Comment on line {line}")
        }
        Target::Reply(_) => "Reply".to_string(),
    };
    let mut head = lead();
    head.push(Span::styled(
        on,
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
    ));
    if let Some(saving) = &view.saving {
        head.push(Span::styled(
            format!(" · {saving}"),
            Style::default().fg(th.warn),
        ));
    }
    let mut out = vec![crate::pr_preview::fit(head, width)];
    let (rows, _) = crate::ui::multiline_input_lines(&composer.input, text_w, th.accent, th);
    let shown = rows.len();
    for (n, row) in rows.into_iter().enumerate() {
        let mut spans = lead();
        spans.extend(row.spans);
        if n == 0 && composer.input.is_empty() {
            spans.push(Span::styled(
                "held in your pending review until you submit it",
                Style::default().fg(th.dim),
            ));
        }
        out.push(crate::pr_preview::fit(spans, width));
    }
    let at = TextView {
        width: u16::try_from(text_w).unwrap_or(u16::MAX),
        height: u16::try_from(shown).unwrap_or(u16::MAX).max(1),
        top: 0,
    };
    (out, at)
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
            head.push(Span::styled(
                format!(" · {}", crate::pr_preview::ago(&c.at, now)),
                dim,
            ));
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

/// The bar's rows: its borders, the verdicts, and the summary as tall as
/// what is typed, up to [`SUMMARY_ROWS`].
pub(crate) fn bar_height(view: &ReviewScreen, width: u16) -> u16 {
    let field_w = usize::from(width.saturating_sub(2)).saturating_sub(FIELD_LEAD + 1);
    let rows = view
        .summary
        .rows(field_w.max(1))
        .len()
        .clamp(1, SUMMARY_ROWS);
    3 + rows as u16
}

/// The cells a bar row's label takes: `form_label`'s, and a space.
const FIELD_LEAD: usize = 9;

/// What [`draw_bar`] left on screen, for the write-back.
pub(crate) struct Bar {
    /// The cells its foot — why the last submit didn't go, or what is on
    /// its way — takes at the right of the bottom edge.
    pub foot_w: u16,
    pub verdicts: Vec<(Rect, github::Verdict)>,
    pub summary: Option<TextView>,
}

/// The bar: the verdicts — only a comment on your own pull request — and
/// how much is pending on its first row, the summary under them.
pub(crate) fn draw_bar(
    f: &mut Frame,
    view: &ReviewScreen,
    area: Rect,
    focused: bool,
    th: Theme,
) -> Bar {
    let (inner, foot_w) = crate::ui::form_frame(
        f,
        area,
        "Your review",
        view.notice.as_deref(),
        view.saving.as_deref(),
        focused,
        th,
    );
    let mut out = Bar {
        foot_w,
        verdicts: Vec::new(),
        summary: None,
    };
    let [verdict_a, summary_a] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    let on_verdict = focused && view.bar_row == BarRow::Verdict;
    let mut spans = vec![
        crate::ui::form_label("Verdict", on_verdict, th),
        Span::raw(" "),
    ];
    let mut x = verdict_a.x + FIELD_LEAD as u16;
    for (i, verdict) in github::Verdict::ALL.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(VERDICT_GAP));
            x += VERDICT_GAP.width() as u16;
        }
        let allowed = verdict.allowed(view.own);
        let chosen = verdict == view.verdict;
        let text = format!("{} {}", if chosen { "●" } else { "○" }, verdict.label());
        let style = match (chosen, allowed) {
            (true, _) if on_verdict => Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            (true, _) => Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            (false, true) => Style::default().fg(th.muted),
            (false, false) => Style::default().fg(th.faint),
        };
        let w = text.width() as u16;
        if allowed {
            out.verdicts
                .push((Rect::new(x, verdict_a.y, w, 1), verdict));
        }
        x += w;
        spans.push(Span::styled(text, style));
    }
    if view.own {
        spans.push(Span::styled(
            " · yours, so GitHub takes a comment",
            Style::default().fg(th.dim),
        ));
    }
    let right = vec![Span::styled(
        super::bar::pending_words(view),
        Style::default().fg(if view.pending_comments() > 0 {
            th.accent
        } else {
            th.dim
        }),
    )];
    // A cell clear of the border.
    let width = usize::from(verdict_a.width).saturating_sub(1);
    f.render_widget(
        Paragraph::new(Line::from(pin_right(spans, right, width))),
        verdict_a,
    );

    let on_summary = focused && view.bar_row == BarRow::Summary;
    f.render_widget(
        Paragraph::new(Line::from(crate::ui::form_label("Summary", on_summary, th))),
        summary_a,
    );
    let field = crate::ui::field_after(summary_a, FIELD_LEAD);
    let field = Rect {
        height: summary_a.height,
        ..field
    };
    if on_summary {
        out.summary = Some(crate::ui::draw_multiline_input(f, &view.summary, field, th).0);
    } else {
        crate::ui::draw_multiline_idle(
            f,
            &view.summary,
            field,
            "optional — what the review says as a whole",
            th,
        );
    }
    out
}

/// `left`, then `right` ending on the row's last of `width` cells — or,
/// with no room for both, `left` alone.
fn pin_right(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let used = |spans: &[Span]| spans.iter().map(|s| s.content.width()).sum::<usize>();
    let (l, r) = (used(&left), used(&right));
    if l + 1 + r <= width {
        left.push(Span::raw(" ".repeat(width - l - r)));
        left.extend(right);
    }
    left
}
