//! The DONE panel: the TODOS MODAL's right column, always beside the
//! lists — your ticked todos and nothing else, across every project, over
//! a span: **Today**, the **Week** from Monday, the **Month** from the
//! 1st, the **Quarter** from its first day. Under the span a bar chart of
//! it — a bar a day, a bar a week for the quarter (and for a month too
//! narrow for a bar a day) — then what was ticked, a section per project
//! in the tabs' order, newest first, each with when: the time today, the
//! day this week, the date further back.
//!
//! `Tab` from the lists puts the keys here and `Tab` / `⇧Tab` (or Esc)
//! hand them back; `←`/`→` turn the span, and `Space` puts an item ticked
//! by mistake back on its list. The panel is one across the project tabs: turning to
//! another project's tab keeps it as it is.

use std::cmp::Reverse;
use std::path::Path;

use chrono::{Datelike, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::view::{self, indent, justify, ruled, width_of};
use super::{store, Item, TodoFile};
use crate::app::{App, Overlay};
use crate::theme::Theme;

/// The spans the panel reads, in the strip's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    Today,
    #[default]
    Week,
    Month,
    Quarter,
}

impl Period {
    pub const ALL: [Period; 4] = [Period::Today, Period::Week, Period::Month, Period::Quarter];

    fn label(self) -> &'static str {
        match self {
            Period::Today => "Today",
            Period::Week => "Week",
            Period::Month => "Month",
            Period::Quarter => "Quarter",
        }
    }

    /// `this week`: what the count says it counts.
    fn this(self) -> &'static str {
        match self {
            Period::Today => "today",
            Period::Week => "this week",
            Period::Month => "this month",
            Period::Quarter => "this quarter",
        }
    }

    /// The next span along the strip — `back`, the one before — round
    /// the ends.
    fn turned(self, back: bool) -> Period {
        let n = Period::ALL.len();
        let step = if back { n - 1 } else { 1 };
        Period::ALL[(self as usize + step) % n]
    }

    /// The span's first day, for `today`.
    pub fn start(self, today: NaiveDate) -> NaiveDate {
        match self {
            Period::Today => today,
            Period::Week => super::monday(today),
            Period::Month => today.with_day(1).unwrap_or(today),
            Period::Quarter => {
                let month = today.month0() / 3 * 3 + 1;
                NaiveDate::from_ymd_opt(today.year(), month, 1).unwrap_or(today)
            }
        }
    }

    /// The span's last day, days still to come included: the chart draws
    /// them, empty.
    fn end(self, today: NaiveDate) -> NaiveDate {
        let (start, len) = (self.start(today), self.len());
        match len {
            Len::Days(days) => start + chrono::Days::new(days - 1),
            Len::Months(months) => start
                .checked_add_months(chrono::Months::new(months))
                .and_then(|next| next.pred_opt())
                .unwrap_or(today),
        }
    }

    fn len(self) -> Len {
        match self {
            Period::Today => Len::Days(1),
            Period::Week => Len::Days(7),
            Period::Month => Len::Months(1),
            Period::Quarter => Len::Months(3),
        }
    }

    /// When an item ticked `at` was, as its row says it: the time today,
    /// the day this week, the date further back.
    fn when(self, at: chrono::DateTime<chrono::Local>) -> String {
        match self {
            Period::Today => at.format("%H:%M").to_string(),
            Period::Week => at.format("%a").to_string(),
            Period::Month | Period::Quarter => at.format(DATE).to_string(),
        }
    }

    /// What stands right of the count: the day, or where the span began.
    fn since(self, today: NaiveDate) -> String {
        match self {
            Period::Today => view::day_label(today),
            Period::Week => format!("since {}", view::day_label(self.start(today))),
            Period::Month | Period::Quarter => format!("since {}", self.start(today).format(DATE)),
        }
    }
}

/// How long a span runs.
enum Len {
    Days(u64),
    Months(u32),
}

/// A date further back than the week, as the panel writes it: `3 Nov`.
const DATE: &str = "%-d %b";

/// The panel's own state, on the [`super::TodoView`] and handed from tab
/// to tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DonePanel {
    pub period: Period,
    /// The keys are the panel's, not the list's.
    pub focused: bool,
    /// The cursor's row, by index into the rows as last drawn, and the
    /// first row drawn.
    pub selected: usize,
    pub start: usize,
    /// As last drawn, for the mouse: the whole column — none while the
    /// modal is too narrow for it — the span strip's labels' x-ranges
    /// and its row, the list's area and each drawn row's rect.
    pub area: Option<Rect>,
    pub period_hits: Vec<(u16, u16)>,
    pub period_row: Rect,
    pub list_area: Rect,
    pub row_rects: Vec<(usize, Rect)>,
}

impl DonePanel {
    /// The panel off screen, the modal too narrow for it: the keys stay
    /// with the lists.
    pub(crate) fn hide(&mut self) {
        self.area = None;
        self.focused = false;
    }

    /// The panel on `period`, from the top of its list.
    fn turn_to(&mut self, period: Period) {
        self.period = period;
        self.selected = 0;
        self.start = 0;
    }
}

/// One row of the panel's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row<'a> {
    /// A project's section: its name and how many were ticked in it.
    Project { name: String, count: usize },
    /// A ticked item, under its project's list.
    Item { dir: &'a Path, item: &'a Item },
}

impl Row<'_> {
    fn is_item(&self) -> bool {
        matches!(self, Row::Item { .. })
    }
}

/// Every project's list, in the tabs' order, with its name.
fn lists(app: &App) -> impl Iterator<Item = (String, &Path, &TodoFile)> {
    view::projects(app)
        .into_iter()
        .filter_map(|(_, name, dir)| {
            let (dir, file) = app.todos.get_key_value(&dir)?;
            Some((name, dir.as_path(), file))
        })
}

/// The list's rows: each project with something ticked in the span, in
/// the tabs' order, over what was ticked in it, the latest first.
pub(crate) fn rows(app: &App, period: Period, today: NaiveDate) -> Vec<Row<'_>> {
    let span = period.start(today)..=today;
    let mut out = Vec::new();
    for (name, dir, file) in lists(app) {
        let mut items: Vec<&Item> = file
            .items
            .iter()
            .filter(|i| i.done_on().is_some_and(|day| span.contains(&day)))
            .collect();
        if items.is_empty() {
            continue;
        }
        items.sort_by_key(|i| Reverse(i.done));
        out.push(Row::Project {
            name,
            count: items.len(),
        });
        out.extend(items.into_iter().map(|item| Row::Item { dir, item }));
    }
    out
}

/// One bar of the chart: the days it counts, and the word under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub label: Option<String>,
}

/// A bar a day from `from` to `to`, each labelled by `label`.
fn daily(
    from: NaiveDate,
    to: NaiveDate,
    label: impl Fn(NaiveDate) -> Option<String>,
) -> Vec<Bucket> {
    from.iter_days()
        .take_while(|day| *day <= to)
        .map(|day| Bucket {
            from: day,
            to: day,
            label: label(day),
        })
        .collect()
}

/// A bar a week, Monday to Sunday, over `from` to `to` — the first and
/// last cut to them — each labelled with the month it begins, where one
/// does (and the first with its own).
fn weekly(from: NaiveDate, to: NaiveDate) -> Vec<Bucket> {
    let mut out = Vec::new();
    let mut monday = super::monday(from);
    while monday <= to {
        let (start, end) = (monday.max(from), (monday + chrono::Days::new(6)).min(to));
        let first = start
            .iter_days()
            .take_while(|d| *d <= end)
            .find(|d| d.day() == 1)
            .or(out.is_empty().then_some(start));
        out.push(Bucket {
            from: start,
            to: end,
            label: first.map(|d| d.format("%b").to_string()),
        });
        monday = monday + chrono::Days::new(7);
    }
    out
}

/// The chart's bars for `period` of `today`, in `width` columns: a bar a
/// day — the week's seven for today and the week, the month's days for
/// the month, a week a bar when those do not fit — and a bar a week for
/// the quarter.
pub(crate) fn buckets(period: Period, today: NaiveDate, width: usize) -> Vec<Bucket> {
    let (from, to) = (period.start(today), period.end(today));
    match period {
        Period::Today | Period::Week => {
            let week = Period::Week;
            daily(week.start(today), week.end(today), |d| {
                d.format("%a").to_string().chars().next().map(String::from)
            })
        }
        // A label a week: the 1st, the 8th, the 15th…
        Period::Month if to.day() as usize <= width => daily(from, to, |d| {
            ((d.day() - 1) % 7 == 0).then(|| d.day().to_string())
        }),
        Period::Month | Period::Quarter => weekly(from, to),
    }
}

/// How many items were ticked in each of `buckets` — which run on from
/// one another, in order — every project's.
fn counts(app: &App, buckets: &[Bucket]) -> Vec<usize> {
    let mut out = vec![0; buckets.len()];
    let (Some(first), Some(last)) = (buckets.first(), buckets.last()) else {
        return out;
    };
    let days = lists(app).flat_map(|(_, _, file)| file.items.iter().filter_map(Item::done_on));
    for day in days.filter(|d| (first.from..=last.to).contains(d)) {
        out[buckets.partition_point(|b| b.to < day)] += 1;
    }
    out
}

/// The eighths a bar can stand, bottom to top.
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// The chart's height in rows: twice a bar's eighths to tell days apart.
const CHART_ROWS: usize = 2;
/// Each bar's width and the gap after it, the widest first: the chart
/// takes the first that fits.
const BAR_SHAPES: [(usize, usize); 4] = [(3, 1), (2, 1), (1, 1), (1, 0)];

/// The eighths of the chart's height a bar of `count` stands against the
/// busiest bucket's `max`: none for none, and at least two for any — one
/// would read as the baseline an empty bucket draws.
fn level(count: usize, max: usize) -> usize {
    let top = CHART_ROWS * BARS.len();
    if count == 0 {
        return 0;
    }
    (count * top).div_ceil(max.max(1)).clamp(2, top)
}

/// The cell a bar `level` eighths tall draws on chart row `row` (from the
/// top): its part of the bar, else on the bottom row a faint baseline —
/// a dot for a bucket still to come — else nothing.
fn cell(level: usize, row: usize, future: bool) -> char {
    let floor = (CHART_ROWS - 1 - row) * BARS.len();
    let bottom = row + 1 == CHART_ROWS;
    match level.saturating_sub(floor) {
        0 if !bottom => ' ',
        0 if future => '·',
        0 => BARS[0],
        k => BARS[k.min(BARS.len()) - 1],
    }
}

/// The chart, `width` columns at most: its bar rows, top first, then the
/// row of labels under them, each under the middle of its bar. A bar's
/// height is its count against the busiest; the bucket today is in stands
/// out from the rest, and one with none or still to come is faint.
pub(crate) fn chart(
    buckets: &[Bucket],
    counts: &[usize],
    today: NaiveDate,
    width: usize,
    th: Theme,
) -> Vec<Line<'static>> {
    let n = buckets.len();
    let (bar_w, gap) = BAR_SHAPES
        .into_iter()
        .find(|(bar, gap)| n * bar + n.saturating_sub(1) * gap <= width)
        .unwrap_or((1, 0));
    let max = counts.iter().copied().max().unwrap_or(0);
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new(); CHART_ROWS];
    let mut labels: Vec<Span<'static>> = Vec::new();
    // The columns the label row holds so far.
    let mut written = 0;
    for (i, (bucket, &count)) in buckets.iter().zip(counts).enumerate() {
        let x = i * (bar_w + gap);
        if x + bar_w > width {
            break;
        }
        let current = (bucket.from..=bucket.to).contains(&today);
        let future = bucket.from > today;
        let color = match (current, future || count == 0) {
            (_, true) => th.faint,
            (true, false) => th.text,
            (false, false) => th.muted,
        };
        let level = level(count, max);
        for (r, row) in rows.iter_mut().enumerate() {
            if i > 0 && gap > 0 {
                row.push(Span::raw(" ".repeat(gap)));
            }
            let bar = cell(level, r, future).to_string().repeat(bar_w);
            row.push(Span::styled(bar, Style::default().fg(color)));
        }
        // Under the middle of its bar, a cell clear of the one before.
        if let Some(label) = &bucket.label {
            let w = label.chars().count();
            let x = x + bar_w.saturating_sub(w) / 2;
            if (labels.is_empty() || x > written) && x + w <= width {
                labels.push(Span::raw(" ".repeat(x - written)));
                let fg = if current { th.text } else { th.dim };
                labels.push(Span::styled(label.clone(), Style::default().fg(fg)));
                written = x + w;
            }
        }
    }
    let mut lines: Vec<Line<'static>> = rows.into_iter().map(Line::from).collect();
    lines.push(Line::from(labels));
    lines
}

/// The cursor's row in `rows`: an item, the one it was on or the nearest
/// after it — else before it.
fn resolve(selected: usize, rows: &[Row]) -> Option<usize> {
    let at = selected.min(rows.len().checked_sub(1)?);
    (at..rows.len())
        .find(|i| rows[*i].is_item())
        .or_else(|| (0..at).rev().find(|i| rows[*i].is_item()))
}

fn panel(app: &App) -> Option<&DonePanel> {
    Some(&view::view(app)?.done)
}

fn panel_mut(app: &mut App) -> Option<&mut DonePanel> {
    Some(&mut view::view_mut(app)?.done)
}

/// The rows as they stand now, and the cursor's among them.
fn rows_now(app: &App) -> Option<(Vec<Row<'_>>, Option<usize>)> {
    let panel = panel(app)?;
    let rows = rows(app, panel.period, super::today());
    let at = resolve(panel.selected, &rows);
    Some((rows, at))
}

/// `Tab` from the lists: the keys onto the panel, when it is there.
pub(crate) fn focus(app: &mut App) {
    if let Some(panel) = panel_mut(app).filter(|p| p.area.is_some()) {
        panel.focused = true;
    }
}

/// The panel's own keys, while it has them.
pub(crate) mod keys {
    use crate::hints::Key;

    use crate::nav;

    /// Into the panel, from the lists: the other panel, either way round.
    pub const INTO: Key = nav::PANELS.doing("done");
    /// Back to the lists: the other panel, either way round.
    pub const BACK: Key = nav::PANELS.doing("lists");
    /// The span: Today, Week, Month, Quarter, round the ends.
    pub const PERIOD: Key = nav::SIDEWAYS.doing("span");
    /// The item ticked by mistake, open again.
    pub const UNDO: Key = Key::new(&["space"], "not done");
    pub const MOVE: Key = nav::ROWS;
    #[cfg(test)]
    pub const ALL: &[Key] = &[INTO, BACK, PERIOD, UNDO, MOVE];
}

/// The keys along the modal's bottom edge while the panel has them.
pub(crate) fn hints() -> Vec<crate::hints::Hint> {
    vec![
        keys::UNDO.hint().kept(),
        keys::MOVE.hint(),
        keys::PERIOD.hint(),
        keys::BACK.hint(),
    ]
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let page = panel(app).map_or(1, |p| p.list_area.height.max(1)) as i32;
    match key.code {
        _ if keys::BACK.matches(&key) || crate::nav::CLOSE.matches(&key) => {
            if let Some(panel) = panel_mut(app) {
                panel.focused = false;
            }
        }
        _ if keys::PERIOD.matches(&key) => {
            let back = crate::nav::side(&key) == Some(-1);
            if let Some(panel) = panel_mut(app) {
                panel.turn_to(panel.period.turned(back));
            }
        }
        _ if keys::UNDO.matches(&key) => undo(app),
        _ if keys::MOVE.matches(&key) => step(app, if key.code == KeyCode::Up { -1 } else { 1 }),
        KeyCode::PageDown => step(app, page),
        KeyCode::PageUp => step(app, -page),
        KeyCode::Home => step(app, i32::MIN / 2),
        KeyCode::End => step(app, i32::MAX / 2),
        _ => {}
    }
}

/// The cursor `delta` items on, over the project headers.
fn step(app: &mut App, delta: i32) {
    let Some((rows, Some(at))) = rows_now(app) else {
        return;
    };
    let items: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].is_item()).collect();
    let here = items.iter().position(|i| *i == at).unwrap_or(0) as i64;
    let next = crate::app::clamp_selection(here + i64::from(delta), items.len());
    if let Some(panel) = panel_mut(app) {
        panel.selected = items[next];
    }
}

/// `space`: the cursor's item open again, back on its list where it
/// stood — off the panel, the next one coming up under the cursor.
fn undo(app: &mut App) {
    let Some((rows, Some(at))) = rows_now(app) else {
        return;
    };
    let Row::Item { dir, item } = &rows[at] else {
        return;
    };
    let (dir, id) = (dir.to_path_buf(), item.id);
    let Some(file) = app.todos.get_mut(&dir) else {
        return;
    };
    let Some(item) = file.item_mut(id) else {
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

/// A mouse event over the panel: a click on a span turns to it, one on
/// an item puts the cursor — and the keys — there, the wheel walks the
/// items. False when it was not over the panel.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) -> bool {
    let Some(panel) = panel(app).filter(|p| p.area.is_some_and(|a| a.contains(pos))) else {
        return false;
    };
    let (list, strip) = (panel.list_area, panel.period_row);
    match mouse.kind {
        MouseEventKind::ScrollDown if list.contains(pos) => step(app, 1),
        MouseEventKind::ScrollUp if list.contains(pos) => step(app, -1),
        MouseEventKind::Down(MouseButton::Left) if strip.contains(pos) => {
            let hit =
                crate::ui::tab_hit(&panel.period_hits, pos.x).and_then(|i| Period::ALL.get(i));
            if let (Some(period), Some(panel)) = (hit.copied(), panel_mut(app)) {
                panel.turn_to(period);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if list.contains(pos) => {
            let row = crate::ui::row_hit(&panel.row_rects, pos);
            let on_item = row.is_some_and(|row| {
                rows_now(app).is_some_and(|(rows, _)| rows.get(row).is_some_and(Row::is_item))
            });
            if let (Some(row), true, Some(panel)) = (row, on_item, panel_mut(app)) {
                panel.selected = row;
                panel.focused = true;
            }
        }
        _ => {}
    }
    true
}

/// The header's rows over the list: the span strip, a gap, the count,
/// the chart, its labels, a gap.
const COUNT_ROW: usize = 2;
const CHART_ROW: usize = COUNT_ROW + 1;
const HEAD_ROWS: u16 = (CHART_ROW + CHART_ROWS + 2) as u16;
/// The cells the panel's lines keep clear at either edge: `render_row`'s
/// gutter on the left, a margin on the right.
const PAD: usize = 1;

/// The panel in `area`, framed: the span strip, the count and the chart,
/// then the list, the cursor's row lit while the panel has the keys.
pub(crate) fn draw(
    f: &mut Frame,
    app: &mut App,
    panel: &DonePanel,
    area: Rect,
    th: Theme,
    focused: bool,
) {
    let block = crate::ui::panel_block("Done", focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let today = super::today();
    let budget = (inner.width as usize).saturating_sub(2 * PAD);
    let rows = rows(app, panel.period, today);
    let (period_row, period_hits) = draw_head(f, app, panel.period, &rows, inner, today, th);
    let list_area = Rect {
        y: inner.y + HEAD_ROWS.min(inner.height),
        height: inner.height.saturating_sub(HEAD_ROWS),
        ..inner
    };
    let cursor = resolve(panel.selected, &rows);
    let (start, row_rects) = draw_list(f, panel, &rows, cursor, list_area, budget, th, focused);
    if let Some(Overlay::Todos(v)) = &mut app.overlay {
        let p = &mut v.done;
        p.area = Some(area);
        p.period_hits = period_hits;
        p.period_row = period_row;
        p.list_area = list_area;
        p.row_rects = row_rects;
        p.start = start;
        if let Some(at) = cursor {
            p.selected = at;
        }
    }
}

/// The span strip, the count — and, where it fits, where the span began
/// — and the chart: the strip's row and its labels' x-ranges.
fn draw_head(
    f: &mut Frame,
    app: &App,
    period: Period,
    rows: &[Row],
    inner: Rect,
    today: NaiveDate,
    th: Theme,
) -> (Rect, Vec<(u16, u16)>) {
    let period_row = crate::ui::row_rect(inner, 0).unwrap_or_default();
    let (strip, period_hits) = crate::ui::tab_strip(
        period_row.x,
        period_row.width,
        Period::ALL.iter().map(|p| p.label()),
        period as usize,
        false,
        th,
    );
    f.render_widget(Paragraph::new(Line::from(strip)), period_row);

    let ticked = rows.iter().filter(|r| r.is_item()).count();
    let count = vec![
        Span::raw(" ".repeat(PAD)),
        Span::styled(
            format!("✓{ticked}"),
            Style::default().fg(th.ok).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", period.this()), Style::default().fg(th.dim)),
    ];
    if let Some(line) = crate::ui::row_rect(inner, COUNT_ROW) {
        let since = period.since(today);
        let room = (line.width as usize).saturating_sub(width_of(&count) + 2 * PAD);
        f.render_widget(Paragraph::new(Line::from(count)), line);
        if since.chars().count() <= room {
            let right = Rect {
                width: line.width.saturating_sub(PAD as u16),
                ..line
            };
            let since = Line::from(Span::styled(since, Style::default().fg(th.faint)));
            f.render_widget(Paragraph::new(since.alignment(Alignment::Right)), right);
        }
    }

    let width = (inner.width as usize).saturating_sub(2 * PAD);
    let buckets = buckets(period, today, width);
    let counts = counts(app, &buckets);
    for (i, line) in chart(&buckets, &counts, today, width, th)
        .into_iter()
        .enumerate()
    {
        if let Some(row) = crate::ui::row_rect(inner, CHART_ROW + i) {
            f.render_widget(Paragraph::new(line), crate::ui::after_lead(row, PAD));
        }
    }
    (period_row, period_hits)
}

/// The list in `area`, from the row last drawn first: a section per
/// project, a blank line over each past the first, the cursor's header
/// kept in sight over it. The first row drawn, and each drawn row's rect.
#[allow(clippy::too_many_arguments)]
fn draw_list(
    f: &mut Frame,
    panel: &DonePanel,
    rows: &[Row],
    cursor: Option<usize>,
    area: Rect,
    budget: usize,
    th: Theme,
    focused: bool,
) -> (usize, Vec<(usize, Rect)>) {
    if rows.is_empty() {
        let words = format!("nothing ticked {} yet", panel.period.this());
        crate::ui::empty_list_row(f, crate::ui::after_lead(area, PAD), &words, th);
        return (0, Vec::new());
    }
    let heights: Vec<u16> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| 1 + u16::from(i > 0 && !r.is_item()))
        .collect();
    let at = cursor.unwrap_or(0);
    let header = rows[..=at].iter().rposition(|r| !r.is_item()).unwrap_or(0);
    let (start, drawn) = crate::ui::stacked_rows(&heights, header, at, panel.start, area);
    let mut row_rects = Vec::new();
    for (i, rect) in drawn {
        let rect = if heights[i] > 1 {
            crate::ui::below_first_row(rect)
        } else {
            rect
        };
        if rect.height == 0 {
            continue;
        }
        let spans = row_spans(&rows[i], panel.period, budget, th);
        // Lit only while the keys are here: a quiet column otherwise.
        let lit = focused && cursor == Some(i);
        crate::ui::render_row(f, rect, spans, lit, lit, th);
        row_rects.push((i, rect));
    }
    (start, row_rects)
}

/// A row's line: a project a section as the lists' are — its name in
/// capitals, a rule across, how many were ticked; an item its box and
/// text, and when it was ticked against the right edge.
fn row_spans(row: &Row, period: Period, budget: usize, th: Theme) -> Vec<Span<'static>> {
    match row {
        Row::Project { name, count } => {
            let left = vec![Span::styled(
                name.to_uppercase(),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            )];
            let right = vec![Span::styled(count.to_string(), Style::default().fg(th.dim))];
            ruled(left, right, budget, th)
        }
        Row::Item { item, .. } => {
            let when = item.done.map(|at| period.when(at)).unwrap_or_default();
            let left = vec![indent(1), Span::styled("☑ ", Style::default().fg(th.ok))];
            let right = vec![Span::styled(when, Style::default().fg(th.faint))];
            let room = budget.saturating_sub(width_of(&left) + width_of(&right) + 1);
            let text = vec![Span::styled(
                crate::ui::truncate_cells(&item.text, room),
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

    fn day(m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, d).unwrap()
    }

    /// Each span starts where its name says — the week on Monday, the
    /// month on the 1st, the quarter on its first month's — and runs to
    /// its own end, days to come and all.
    #[test]
    fn each_span_starts_and_ends_where_it_says() {
        // Thursday 5 November 2026.
        let today = day(11, 5);
        let span = |p: Period| (p.start(today), p.end(today));
        assert_eq!(span(Period::Today).0, today);
        assert_eq!(span(Period::Week), (day(11, 2), day(11, 8)));
        assert_eq!(span(Period::Month), (day(11, 1), day(11, 30)));
        assert_eq!(span(Period::Quarter), (day(10, 1), day(12, 31)));
        assert_eq!(Period::Quarter.start(day(2, 14)), day(1, 1));
        assert_eq!(Period::Week.turned(false), Period::Month);
        assert_eq!(Period::Today.turned(true), Period::Quarter);
    }

    /// A bar a day for the week and the month — the month a bar a week
    /// when it is too narrow for its days — and a bar a week for the
    /// quarter, the first and last week cut to it, each labelled with the
    /// month it begins.
    #[test]
    fn the_chart_is_a_bar_a_day_or_a_week() {
        let today = day(11, 5);
        let week = buckets(Period::Week, today, 40);
        assert_eq!(week.len(), 7);
        let labels: Vec<_> = week.iter().filter_map(|b| b.label.clone()).collect();
        assert_eq!(labels, ["M", "T", "W", "T", "F", "S", "S"]);
        assert_eq!(buckets(Period::Today, today, 40), week);
        assert_eq!(buckets(Period::Month, today, 40).len(), 30);
        // November 2026 starts on a Sunday and ends on a Monday.
        let narrow = buckets(Period::Month, today, 20);
        assert_eq!(narrow.len(), 6, "weeks, cut to the month");
        assert_eq!((narrow[0].from, narrow[0].to), (day(11, 1), day(11, 1)));
        assert_eq!((narrow[5].from, narrow[5].to), (day(11, 30), day(11, 30)));
        let quarter = buckets(Period::Quarter, today, 40);
        assert_eq!(quarter.first().unwrap().from, day(10, 1));
        assert_eq!(quarter.last().unwrap().to, day(12, 31));
        let months: Vec<_> = quarter.iter().filter_map(|b| b.label.clone()).collect();
        assert_eq!(months, ["Oct", "Nov", "Dec"]);
    }

    /// A bar's height is its count against the busiest bucket's, over two
    /// rows; none is a baseline and a day to come a dot; the labels sit
    /// under their bars, never over each other.
    #[test]
    fn bars_stand_as_tall_as_their_counts() {
        let th = App::new().theme;
        let today = day(11, 5);
        let week = buckets(Period::Week, today, 40);
        let text =
            |line: &Line| -> String { line.spans.iter().map(|s| s.content.as_ref()).collect() };
        let counts = [4, 0, 2, 1, 0, 0, 0];
        let rows: Vec<String> = chart(&week, &counts, today, 20, th)
            .iter()
            .map(text)
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0], "██                  ",
            "only the busiest reaches the top"
        );
        assert_eq!(rows[1], "██ ▁▁ ██ ▄▄ ·· ·· ··");
        assert_eq!(rows[2].trim_end(), "M  T  W  T  F  S  S");
        // Wider, the bars widen and the labels sit under their middles.
        let rows: Vec<String> = chart(&week, &counts, today, 40, th)
            .iter()
            .map(text)
            .collect();
        assert_eq!(rows[1], "███ ▁▁▁ ███ ▄▄▄ ··· ··· ···");
        assert_eq!(rows[2].trim_end(), " M   T   W   T   F   S   S");
        // Too narrow for a gap: a cell a bar, the labels thinned to fit.
        let month = buckets(Period::Month, today, 30);
        let counts = vec![1; month.len()];
        let rows: Vec<String> = chart(&month, &counts, today, 30, th)
            .iter()
            .map(text)
            .collect();
        assert_eq!(rows[1].chars().count(), 30);
        assert_eq!(rows[2].trim_end(), "1      8      15     22     29");
    }

    /// An app with two projects' lists, a week of ticks in them, and the
    /// modal up on the first: 10am Thursday 5 November.
    fn modal_with_ticks() -> App {
        let mut app = App::new();
        for (i, name) in ["orion", "riplo-os"].iter().enumerate() {
            app.tree.projects.push(orion_core::Project {
                id: orion_core::ProjectId(format!("p{i}")),
                name: (*name).into(),
                repo_path: format!("/tmp/{name}").into(),
                sort_order: i as i64,
            });
        }
        let at = |d: u32, h: u32| {
            chrono::Local
                .with_ymd_and_hms(2026, 11, d, h, 5, 0)
                .unwrap()
        };
        // Each item's project and text, and the day and hour it was
        // ticked — day 0 for one still open.
        let items = [
            (
                "orion",
                "the done panel beside the lists, styled like the rest",
                5,
                9,
            ),
            ("orion", "week review screen", 4, 16),
            ("orion", "session rows show their todo", 3, 11),
            ("orion", "open item", 0, 0),
            ("riplo-os", "ship the deck folders", 2, 10),
            ("riplo-os", "old tick", 1, 10),
        ];
        for (name, text, d, h) in items {
            let dir = std::path::PathBuf::from(format!("/tmp/{name}"));
            let file = app
                .todos
                .entry(dir.clone())
                .or_insert_with(|| super::super::TodoFile::new(&dir));
            let g = file.inbox();
            let id = file.add_item(g, text, day(10, 1));
            if d > 0 {
                file.toggle(id, at(d, h));
            }
        }
        super::super::view::open(&mut app);
        app
    }

    /// The modal drawn `w` × `h`, a line a row.
    fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let Some(Overlay::Todos(v)) = app.overlay.clone() else {
                panic!("no todos modal");
            };
            let th = app.theme;
            super::super::view::draw(f, app, &v, th, false);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    /// The panel down the right of the lists at every usual width: its
    /// spans, the count, the chart and the ticks by project, each on its
    /// own row and inside its frame — and no panel at all on a window too
    /// narrow for both.
    #[test]
    fn the_panel_draws_beside_the_lists_at_every_width() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 5, 10, 0, 0)
            .unwrap();
        super::super::with_now(now, || {
            let mut app = modal_with_ticks();
            for w in [80u16, 120, 200] {
                let rows = screen(&mut app, w, 32);
                let shot = rows.join("\n");
                let done = rows
                    .iter()
                    .position(|r| r.contains("╭ Done "))
                    .expect(&shot);
                // The panel's columns: from its frame's corner on.
                let byte = rows[done].find("╭ Done ").unwrap();
                let x = rows[done][..byte].chars().count();
                let panel: Vec<String> = rows[done..]
                    .iter()
                    .map(|r| r.chars().skip(x).collect())
                    .collect();
                let has = |needle: &str| panel.iter().any(|r| r.contains(needle));
                assert!(has("Today") && has("Week") && has("Quarter"), "{shot}");
                assert!(has("✓4 this week"), "{shot}");
                // Where the span began, while the row has room for it.
                assert_eq!(has("since Mon 2 Nov"), w > 80, "{shot}");
                assert!(
                    panel.iter().any(|r| r.replace(' ', "").contains("MTWTFSS")),
                    "the days under the bars: {shot}"
                );
                assert!(has("ORION"), "{shot}");
                assert!(has("RIPLO-OS"), "{shot}");
                assert!(!has("old tick"), "last week's: {shot}");
                assert!(!has("open item"), "{shot}");
                // Every row of the panel ends on its frame: nothing spills.
                for r in &panel[1..20] {
                    assert!(r.trim_end().ends_with('│'), "{w}: {r:?}\n{shot}");
                }
            }
            // The newest first, with the day it was ticked.
            let rows = screen(&mut app, 120, 32);
            let at = |needle: &str| rows.iter().position(|r| r.contains(needle)).unwrap();
            assert!(at("the done panel") < at("week review screen"));
            assert!(at("week review screen") < at("session rows"));
            assert!(rows[at("week review screen")].contains("Wed"));

            // `Tab`, then `→`: the month, a bar a day — and the quarter, a
            // bar a week under its months — the dates beside the items.
            let key = |code: KeyCode| KeyEvent::from(code);
            super::super::view::handle_key(&mut app, key(KeyCode::Tab), &mut Vec::new());
            assert!(view::view(&app).unwrap().done.focused, "Tab: the panel");
            super::super::view::handle_key(&mut app, key(KeyCode::Right), &mut Vec::new());
            for w in [80u16, 120] {
                let shot = screen(&mut app, w, 32).join("\n");
                assert!(shot.contains("✓5 this month"), "{shot}");
                assert!(shot.contains("1 Nov"), "the 1st's tick, dated: {shot}");
            }
            super::super::view::handle_key(&mut app, key(KeyCode::Right), &mut Vec::new());
            let shot = screen(&mut app, 120, 32).join("\n");
            assert!(shot.contains("✓5 this quarter"), "{shot}");
            assert!(shot.contains("Oct") && shot.contains("Dec"), "{shot}");
            // `←` turns it back, and stays on the panel.
            super::super::view::handle_key(&mut app, key(KeyCode::Left), &mut Vec::new());
            let shot = screen(&mut app, 120, 32).join("\n");
            assert!(shot.contains("✓5 this month"), "{shot}");
            assert!(view::view(&app).unwrap().done.focused, "← stays on the panel");

            // Too narrow for both: the lists alone, and `Tab` goes nowhere.
            super::super::view::handle_key(&mut app, key(KeyCode::Tab), &mut Vec::new());
            assert!(!view::view(&app).unwrap().done.focused, "Tab: back to the lists");
            let rows = screen(&mut app, 60, 24).join("\n");
            assert!(!rows.contains("╭ Done "), "{rows}");
            super::super::view::handle_key(&mut app, key(KeyCode::Tab), &mut Vec::new());
            assert!(!view::view(&app).unwrap().done.focused);
        });
    }

    /// The list is what was ticked in the span, a section per project in
    /// the tabs' order, the latest tick first — a tick from before the
    /// span, and an open item, left out.
    #[test]
    fn the_list_is_the_spans_ticks_by_project_latest_first() {
        let mut app = App::new();
        for (i, name) in ["api", "web"].iter().enumerate() {
            app.tree.projects.push(orion_core::Project {
                id: orion_core::ProjectId(format!("p{i}")),
                name: (*name).into(),
                repo_path: format!("/tmp/{name}").into(),
                sort_order: i as i64,
            });
        }
        let at = |d: u32, h: u32| {
            chrono::Local
                .with_ymd_and_hms(2026, 11, d, h, 0, 0)
                .unwrap()
        };
        let mut api = super::super::TodoFile::new(std::path::Path::new("/tmp/api"));
        let g = api.add_group(None, "G");
        let early = api.add_item(g, "early", day(11, 1));
        let late = api.add_item(g, "late", day(11, 1));
        let old = api.add_item(g, "last week", day(10, 1));
        api.add_item(g, "open", day(11, 1));
        api.toggle(early, at(3, 9));
        api.toggle(late, at(4, 9));
        api.toggle(old, at(1, 9));
        let mut web = super::super::TodoFile::new(std::path::Path::new("/tmp/web"));
        let w = web.add_group(None, "W");
        let shipped = web.add_item(w, "shipped", day(11, 1));
        web.toggle(shipped, at(5, 8));
        app.todos.insert("/tmp/api".into(), api);
        app.todos.insert("/tmp/web".into(), web);
        // Each row as `project count`, or an item's list and id.
        let shape = |rows: Vec<Row>| -> Vec<String> {
            rows.iter()
                .map(|r| match r {
                    Row::Project { name, count } => format!("{name} {count}"),
                    Row::Item { dir, item } => format!("{} {}", dir.display(), item.id),
                })
                .collect()
        };
        assert_eq!(
            shape(rows(&app, Period::Week, day(11, 5))),
            [
                "api 2".to_string(),
                format!("/tmp/api {late}"),
                format!("/tmp/api {early}"),
                "web 1".to_string(),
                format!("/tmp/web {shipped}"),
            ]
        );
        assert_eq!(rows(&app, Period::Today, day(11, 5)).len(), 2);
        assert_eq!(
            rows(&app, Period::Month, day(11, 5)).len(),
            6,
            "the 1st's too"
        );
        let week = buckets(Period::Week, day(11, 5), 40);
        assert_eq!(
            counts(&app, &week),
            [0, 1, 1, 1, 0, 0, 0],
            "Sunday's is last week's"
        );
    }
}
