//! The DIFF VIEWER (`⌘E`) on screen.
//!
//! A left column and the diff beside it, the diff taking the modal's whole
//! height. The column is [`crate::app::DEFAULT_DIFF_FILES_W`] wide until
//! dragged, room for a commit's subject
//! on one row and a file's path beside its counts; it keeps to half of a
//! narrow modal. The column is the COMMIT LIST over the changed files, in
//! reading order: what is on screen, its files, the selected file's diff. A
//! pull request's view has no commit list, and the files take the column.
//!
//! - **The COMMIT LIST** is one row per commit: its box — `[✓]` ticked,
//!   `[ ]` not — and its subject, cut at the end, then its short sha, age
//!   and `+A −R` in columns on the right. Rows whose changes are in the
//!   diff on screen are bold. The panel's bottom edge says how many are
//!   ticked and how they are read.
//! - **The files** read the same way: status, ✓, the path cut at its end
//!   so it reads from its root, and the file's own `+A −R` on the right.
//! - **The diff** is a `diff_doc::DiffDoc`: wrapped, numbered, syntax
//!   coloured, starting on the pane's first row. Its border names what it
//!   is read as part of — the commit, the ticked, the uncommitted changes,
//!   a pull request — and the file's `+A −R` (`2 of 3` stepping);
//!   the file itself is the Files list's cursor. One dim line at its foot
//!   explains what is on screen and how to change it.
//! - **The keys** are on the modal's bottom border, for the panel that has
//!   them — which wears the accent — from [`diff_keys`], the table the key
//!   handler matches.

use super::{
    below_first_row, centered_rect_pct, empty_list_row, fuzzy_highlight_spans, panel_block,
    render_row, row_rect, search_line, truncate, truncate_cells, visible_positions, NO_MATCHES,
    SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
};
use crate::app::{App, DiffFocus, DiffView, Overlay};
use crate::bundle::plural;
use crate::commit_list::{CommitList, Row, Showing};
use crate::theme::Theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// The DIFF VIEWER's own keys: one table its key arm
/// (`event_loop::handle_overlay_key`) matches and [`diff_hints`] spells.
pub(crate) mod diff_keys {
    use crate::hints::Key;

    /// The COMMIT LIST: tick the cursor's row, or untick it.
    pub const TICK: Key = Key::new(&["space"], "tick");
    /// The COMMIT LIST: every commit ticked, or none.
    pub const ALL: Key = Key::new(&["cmd+a", "ctrl+a"], "all");
    /// The ticked commits TOGETHER or ONE AT A TIME.
    pub const MODE: Key = Key::new(&["cmd+g", "ctrl+g"], "one at a time");
    /// Older / newer: the cursor's commit, or the step through the ticked.
    pub const STEP: Key = Key::new(&["shift+left", "shift+right"], "commit").show(2);
    /// The next panel, or the one before.
    pub const PANEL: Key = Key::new(&["tab", "shift+tab"], "files");
    /// The commits: on to the files. A file: read its diff.
    pub const OPEN: Key = Key::new(&["enter"], "files");
    /// The diff's own scroll, with the keys on it.
    pub const LINES: Key = Key::new(&["up", "down"], "scroll").show(2);
    pub const PAGE: Key = Key::new(&["pgup", "pgdn", "space"], "page").show(2);
    /// The diff with the keys: back to the files.
    pub const BACK: Key = Key::new(&["left"], "files");
    /// The diff's scroll from anywhere else.
    pub const SCROLL: Key = Key::new(&["shift+up", "shift+down"], "scroll").show(2);
    pub const REVIEWED: Key = Key::new(&["cmd+r", "ctrl+r"], "reviewed");
    /// The grid's file tree chord (`Action::TreeBrowser`).
    pub const TREE: Key = Key::new(&["cmd+b", "ctrl+b"], "tree");
    pub const FOLD: Key = Key::new(&["left", "right"], "fold").show(2);

    /// Every key of the table, for the tests that hold the hints to it.
    #[cfg(test)]
    pub const ALL_KEYS: &[Key] = &[
        TICK,
        ALL,
        MODE,
        STEP,
        PANEL,
        OPEN,
        LINES,
        PAGE,
        BACK,
        SCROLL,
        REVIEWED,
        TREE,
        FOLD,
        crate::hints::IN_CURSOR,
    ];
}

/// The DIFF VIEWER's keys for the panel that has them, the verbs worded for
/// what the COMMIT LIST holds: `⇧←/⇧→ commit` with nothing ticked, `step`
/// through what is, `^G` between reading the ticked together and one at a
/// time once there are two.
pub(crate) fn diff_hints(view: &DiffView) -> Vec<crate::hints::Hint> {
    use diff_keys::*;
    let list = view.commits.as_ref();
    let ticked = list.map_or(0, |l| l.ticked.len());
    let step = match list {
        Some(_) if ticked == 0 => Some(STEP.hint()),
        Some(_) if ticked > 1 => Some(STEP.hint_as("step")),
        _ => None,
    };
    let mode = list.filter(|_| ticked > 1).map(|l| {
        MODE.hint_as(if l.one_at_a_time {
            "together"
        } else {
            "one at a time"
        })
    });
    let next = PANEL.hint_as(match view.next_focus(true) {
        DiffFocus::Commits => "commits",
        DiffFocus::Files => "files",
        DiffFocus::Diff => "diff",
    });
    let mut hints = Vec::new();
    match view.focus {
        DiffFocus::Commits => {
            hints.push(TICK.hint().kept());
            let all = list.is_some_and(|l| l.all_ticked());
            hints.push(ALL.hint_as(if all { "none" } else { "all" }));
            hints.extend(mode);
            hints.extend(step);
            hints.push(next);
        }
        DiffFocus::Files => {
            if view.selected_dir().is_some() {
                hints.push(OPEN.hint_as("fold"));
            } else {
                hints.push(OPEN.hint_as("read").kept());
            }
            hints.extend(step);
            hints.extend(mode);
            if list.is_some() && view.filter.is_empty() {
                let all = list.is_some_and(|l| l.all_ticked());
                hints.push(ALL.hint_as(if all { "untick all" } else { "tick all" }));
            }
            hints.push(next);
            if view.tree.is_some() {
                hints.push(FOLD.hint());
            }
            hints.push(SCROLL.hint());
            hints.push(REVIEWED.hint());
            hints.push(TREE.hint_as(if view.tree.is_some() {
                "flat list"
            } else {
                "tree"
            }));
            if view.prefetched.is_none() {
                hints.push(crate::hints::in_app_hint());
            }
        }
        DiffFocus::Diff => {
            hints.push(LINES.hint().kept());
            hints.push(PAGE.hint());
            hints.extend(step);
            hints.extend(mode);
            hints.push(BACK.hint());
            hints.push(next);
            hints.push(REVIEWED.hint());
            if view.prefetched.is_none() {
                hints.push(crate::hints::in_app_hint());
            }
        }
    }
    hints.push(match view.back {
        Some(_) => crate::hints::ESC_CLOSE.hint_as("back to the pull request"),
        None => crate::hints::ESC_CLOSE.hint(),
    });
    hints
}

/// The pull request a viewer opened from the PULL REQUESTS MODAL is
/// reading, as its top-left panel's title leads with it — `#42 ›` — so
/// the viewer reads as a level inside the modal.
fn crumb(view: &DiffView) -> Option<String> {
    view.back.as_ref()?;
    let url = view.pr_url.as_deref()?;
    let (_, rest) = url.split_once("/pull/")?;
    let number = rest.split('/').next()?;
    Some(format!("#{number} › "))
}

/// The dim line at the diff's foot: what is on screen, and the key that
/// changes it.
pub(crate) fn diff_explanation(view: &DiffView) -> String {
    use diff_keys::*;
    let Some(list) = &view.commits else {
        return if view.prefetched.is_some() {
            format!(
                "The pull request's whole diff. {} reads the file under the cursor; {} walks the panels.",
                OPEN.label(),
                PANEL.label()
            )
        } else {
            String::new()
        };
    };
    if !list.loaded {
        return "Reading the branch's commits…".to_string();
    }
    let ticked = list.ticked.len();
    let base = list.base.as_deref().unwrap_or("its base");
    let tick = TICK.label();
    match list.showing() {
        None => "Reading the next page of older commits…".to_string(),
        Some(Showing::Row(Row::Uncommitted)) if ticked == 0 => format!(
            "The uncommitted changes. {tick} on a commit ticks it, to read several at once."
        ),
        Some(Showing::Row(_)) if ticked == 0 => format!(
            "The commit under the cursor. {tick} ticks commits to read several — together or one at a time."
        ),
        Some(Showing::Row(_)) if ticked == 1 => {
            format!("The one ticked row. {tick} on another reads them together.")
        }
        Some(Showing::Row(_)) => {
            let (k, n) = list.step_place().unwrap_or((1, ticked));
            format!(
                "Commit {k} of the {n} ticked, oldest first. {} steps; {} reads them together.",
                STEP.label(),
                MODE.label()
            )
        }
        Some(Showing::Together(_)) if list.all_ticked() => format!(
            "Every commit made on the branch since {base} — none that merging {base} in brought. {} unticks them.",
            ALL.label()
        ),
        Some(Showing::Together(rows)) => format!(
            "The {} ticked as one diff. {} reads them one at a time.",
            rows.len(),
            STEP.label()
        ),
    }
}

/// The left column's width: what the reader dragged it to — or, never
/// dragged, the default, but no more than half a narrow modal, so the diff
/// keeps the room it wraps in.
fn column_width(view: &DiffView, area: Rect) -> u16 {
    let mut want = view.files_width;
    if want == crate::app::DEFAULT_DIFF_FILES_W {
        want = want.min(area.width / 2);
    }
    // Cap first, floor second: on a tiny screen the column keeps its
    // minimum and SPLIT_PANE_LAYOUT_MIN squeezes the diff pane instead.
    want.min(area.width.saturating_sub(crate::app::MIN_DIFF_PANE_W))
        .max(crate::app::MIN_DIFF_FILES_W)
}

/// Draw the DIFF VIEWER over the screen and write back where everything
/// went (`view` is a clone of `app.overlay`'s).
pub(super) fn draw(f: &mut Frame, app: &mut App, view: &DiffView, th: Theme) {
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let col_w = column_width(view, area);
    let [column, diff_a] = Layout::horizontal([
        Constraint::Length(col_w),
        Constraint::Min(SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);

    // The column: the COMMIT LIST as tall as its rows, up to a little
    // under half; the files below it.
    let now = crate::app::now_ms();
    let heights = view
        .commits
        .as_ref()
        .map(|list| vec![1; list.row_count()])
        .unwrap_or_default();
    let commits_h = match &view.commits {
        None => 0,
        Some(list) if !list.loaded => 3,
        Some(_) => {
            let want = heights.iter().sum::<usize>() as u16 + 2;
            let cap = (column.height * 9 / 20).max(5);
            want.clamp(3, cap).min(column.height.saturating_sub(6))
        }
    };
    let [commits_a, files_a] =
        Layout::vertical([Constraint::Length(commits_h), Constraint::Min(0)]).areas(column);
    let mut commit_draw = None;
    if let (Some(list), true) = (&view.commits, commits_h >= 3) {
        commit_draw = Some(draw_commits(
            f,
            list,
            &heights,
            view.focus == DiffFocus::Commits,
            crumb(view),
            commits_a,
            now,
            th,
        ));
    }
    // With no COMMIT LIST over them, the files lead with the crumb.
    let files_crumb = commit_draw.is_none().then(|| crumb(view)).flatten();
    let (list_inner, files_top) = draw_files(f, view, files_crumb, files_a, th);
    let (diff_inner, scroll) = draw_diff(f, view, diff_a, th);

    // The viewer's keys along its bottom edge, under both columns and
    // clear of the diff's scroll position.
    let reserve = if view.max_scroll() > 0 { 8 } else { 0 };
    crate::hints::draw_on_border(f, area, &diff_hints(view), reserve, th);

    // Write-back (draw works on a clone): page size and wrap width for the
    // keys, the scroll re-clamped so a resize never strands it, and every
    // rect the pointer is tested against.
    if let Some(Overlay::Diff(v)) = &mut app.overlay {
        v.view_height = diff_inner.height;
        v.view_width = diff_inner.width;
        v.scroll = scroll;
        v.list_area = list_inner;
        v.files_scroll.top = files_top;
        v.diff_area = diff_a;
        v.area = area;
        v.column_w = col_w;
        if let Some(list) = &mut v.commits {
            match commit_draw {
                Some((top, hits)) => {
                    list.area = commits_a;
                    list.top = top;
                    list.hits = hits;
                }
                None => {
                    list.area = Rect::default();
                    list.hits.clear();
                }
            }
        }
    }
}

/// The COMMIT LIST panel. Returns the first row drawn and each drawn row's
/// rect, for the pointer.
#[allow(clippy::too_many_arguments)]
fn draw_commits(
    f: &mut Frame,
    list: &CommitList,
    heights: &[usize],
    focused: bool,
    crumb: Option<String>,
    area: Rect,
    now: i64,
    th: Theme,
) -> (usize, Vec<(Rect, usize)>) {
    let title = match (&list.base, list.merge_base.is_some()) {
        _ if !list.loaded => "Commits".to_string(),
        (Some(base), true) if list.commits.is_empty() => format!("Commits · none since {base}"),
        (Some(base), false) => format!("Commits · no history in common with {base}"),
        (None, _) => "Commits · no base branch to compare with".to_string(),
        (Some(base), true) if list.has_older() => {
            format!(
                "Commits · {} of {} since {base}",
                list.commits.len(),
                list.total
            )
        }
        (Some(base), true) => format!("Commits · {} since {base}", list.total),
    };
    let title = format!("{}{title}", crumb.unwrap_or_default());
    let mut block = panel_block(&title, focused, th);
    let ticked = list.ticked.len();
    if ticked > 0 {
        let how = match (ticked, list.one_at_a_time) {
            (1, _) => "1 ticked".to_string(),
            _ if list.all_ticked() && !list.one_at_a_time => "all ticked · together".to_string(),
            (n, false) => format!("{n} ticked · together"),
            (n, true) => format!("{n} ticked · one at a time"),
        };
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {how} "),
                Style::default().fg(if focused { th.accent } else { th.muted }),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if !list.loaded {
        empty_list_row(f, inner, "reading commits…", th);
        return (0, Vec::new());
    }
    let top = list.window_top(heights, usize::from(inner.height));
    let columns = CommitColumns::of(list, now);
    let mut hits = Vec::new();
    for (n, i) in (top..list.row_count()).enumerate() {
        let (Some(row), Some(rect)) = (list.row(i), row_rect(inner, n)) else {
            break;
        };
        let spans = entry_line(list, row, &columns, row_width(inner), now, th);
        render_row(f, rect, spans, i == list.selected, focused, th);
        hits.push((rect, i));
    }
    (top, hits)
}

/// The right-hand columns of the COMMIT LIST's rows, each as wide as its
/// widest entry so they read straight down: short sha (or the uncommitted
/// row's file count), age, lines added, lines removed.
struct CommitColumns {
    who: usize,
    age: usize,
    counts: CountColumns,
}

impl CommitColumns {
    fn of(list: &CommitList, now: i64) -> Self {
        let files = list.uncommitted.map(|s| plural(s.files, "file").width());
        let who = list
            .commits
            .iter()
            .map(|c| c.short.width())
            .chain(files)
            .max()
            .unwrap_or(0);
        let age = list
            .commits
            .iter()
            .map(|c| short_age(&c.ago(now)).width())
            .max()
            .unwrap_or(0);
        let stats = list
            .commits
            .iter()
            .map(|c| c.stat)
            .chain([list.uncommitted]);
        let counts = CountColumns::of(stats.map(|s| s.and_then(|s| s.lines)));
        Self { who, age, counts }
    }
}

/// `3h ago` as the COMMIT LIST's age column prints it: `3h`.
fn short_age(ago: &str) -> &str {
    ago.strip_suffix(" ago").unwrap_or(ago)
}

/// The fewest cells a COMMIT LIST row keeps for its subject before the
/// sha and age give theirs up to it, the counts staying.
const MIN_SUBJECT_W: usize = 16;

/// One COMMIT LIST row: its box and its subject, cut at the end to fit,
/// then the sha, age and counts in their columns on the right.
fn entry_line(
    list: &CommitList,
    row: Row,
    columns: &CommitColumns,
    width: usize,
    now: i64,
    th: Theme,
) -> Vec<Span<'static>> {
    let dim = Style::default().fg(th.dim);
    if row == Row::Older {
        let text = match list.paging {
            Some(_) => "reading older commits…".to_string(),
            None => format!("… {} older commits", list.total - list.commits.len()),
        };
        return vec![Span::styled(truncate(&text, width), dim)];
    }
    let ticked = list.ticked.contains(&row);
    let any_ticked = !list.ticked.is_empty();
    let (mark, mark_style) = if ticked {
        (
            "[✓] ",
            Style::default().fg(th.ok).add_modifier(Modifier::BOLD),
        )
    } else {
        ("[ ] ", dim)
    };
    let mut words = Style::default();
    if list.on_screen(row) {
        words = words.fg(th.text).add_modifier(Modifier::BOLD);
    } else if any_ticked && !ticked {
        words = words.fg(th.muted);
    }
    let (subject, who, age, lines) = match row {
        Row::Commit(i) => {
            let c = &list.commits[i];
            let age = short_age(&c.ago(now)).to_string();
            let who = Span::styled(c.short.clone(), Style::default().fg(th.accent));
            (c.subject.as_str(), who, age, c.stat.and_then(|s| s.lines))
        }
        _ => {
            let stat = list.uncommitted;
            let files = stat.map(|s| plural(s.files, "file")).unwrap_or_default();
            let lines = stat.and_then(|s| s.lines);
            (
                "Uncommitted changes",
                Span::styled(files, dim),
                String::new(),
                lines,
            )
        }
    };
    let counts = columns.counts.spans(lines, th);
    // Padded by chars: sha, age and `N files` are one cell to a char.
    let mut right = vec![
        Span::styled(format!("{:<w$}", who.content, w = columns.who), who.style),
        Span::styled(format!(" {age:>w$}  ", w = columns.age), dim),
    ];
    right.extend(counts.iter().cloned());
    let room = |right: &[Span]| left_room(width, spans_width(right)).saturating_sub(mark.width());
    if room(&right) < MIN_SUBJECT_W {
        right = counts;
    }
    let subject = truncate_cells(subject, room(&right));
    let left = vec![Span::styled(mark, mark_style), Span::styled(subject, words)];
    pin_right(left, right, width)
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// A list row's own width: its rect's, less the cursor's `▌`.
fn row_width(rect: Rect) -> usize {
    usize::from(rect.width).saturating_sub(1)
}

/// The cells a row of `width` leaves its left side beside `right_w` pinned
/// to its right edge, one cell kept between them.
fn left_room(width: usize, right_w: usize) -> usize {
    width.saturating_sub(right_w + usize::from(right_w > 0))
}

/// `left` padded out so `right` ends on the row's last cell of `width`.
fn pin_right<'a>(mut left: Vec<Span<'a>>, right: Vec<Span<'a>>, width: usize) -> Vec<Span<'a>> {
    let gap = width.saturating_sub(spans_width(&left) + spans_width(&right));
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    left
}

/// A list's `+A −R` column pair, each as wide as its widest entry, so the
/// counts of every row line up on the right edge.
struct CountColumns {
    added: usize,
    removed: usize,
}

impl CountColumns {
    fn of(lines: impl Iterator<Item = Option<crate::git_diff::LineChanges>>) -> Self {
        let mut cols = Self {
            added: 0,
            removed: 0,
        };
        // The sign and every digit are a cell each.
        let cells = |n: u64| 1 + n.checked_ilog10().map_or(1, |d| d as usize + 1);
        for l in lines.flatten() {
            cols.added = cols.added.max(cells(l.added));
            cols.removed = cols.removed.max(cells(l.removed));
        }
        cols
    }

    /// No row of the list was counted: no columns at all.
    fn is_empty(&self) -> bool {
        self.added == 0
    }

    /// The cells the pair takes, its trailing space included.
    fn width(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.added + self.removed + 2
        }
    }

    /// `+A −R ` padded to the columns — blank for a row not counted (yet),
    /// nothing at all for a list with no counts.
    fn spans(&self, lines: Option<crate::git_diff::LineChanges>, th: Theme) -> Vec<Span<'static>> {
        if self.is_empty() {
            return Vec::new();
        }
        let Some(l) = lines else {
            return vec![Span::raw(" ".repeat(self.width()))];
        };
        vec![
            Span::styled(
                format!("{:>w$}", format!("+{}", l.added), w = self.added),
                count_style(l.added, th.added, th),
            ),
            Span::styled(
                format!(" {:>w$} ", format!("−{}", l.removed), w = self.removed),
                count_style(l.removed, th.removed, th),
            ),
        ]
    }
}

/// A `+A` or `−R` count in its colour — dim when nothing went that way.
fn count_style(n: u64, color: ratatui::style::Color, th: Theme) -> Style {
    Style::default().fg(if n == 0 { th.dim } else { color })
}

/// A file row's gutter ahead of its path: the two-letter status code and
/// a space, then the ✓ (or its blank) and a space.
const GUTTER_W: usize = 5;

/// The changed-file list — flat paths, or the directory tree (`Ctrl+t`) —
/// under its always-live filter. Returns the rows' rect and the first row
/// drawn.
fn draw_files(
    f: &mut Frame,
    view: &DiffView,
    crumb: Option<String>,
    area: Rect,
    th: Theme,
) -> (Rect, usize) {
    let focused = view.focus == DiffFocus::Files;
    let mut title = crumb.unwrap_or_default();
    title += &if view.listing.is_some() && view.files.is_empty() {
        "Files (…)".to_string()
    } else if view.filter.is_empty() {
        format!("Files ({})", view.files.len())
    } else {
        format!("Files ({}/{})", view.matches.len(), view.files.len())
    };
    if !view.reviewed.is_empty() {
        title.push_str(&format!(" · {}✓", view.reviewed.len()));
    }
    let block = panel_block(&title, focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    // First row: the always-on fuzzy filter input.
    if let Some(filter_area) = row_rect(inner, 0) {
        let line = search_line(&view.filter, "type to filter…", filter_area, th);
        f.render_widget(Paragraph::new(line), filter_area);
    }
    let list_inner = below_first_row(inner);
    if view.listing.is_some() && view.files.is_empty() {
        empty_list_row(f, list_inner, "reading changes…", th);
    } else if view.files.is_empty() {
        empty_list_row(f, list_inner, "no files changed", th);
    } else if view.row_count() == 0 {
        empty_list_row(f, list_inner, NO_MATCHES, th);
    }
    let start = view.window_start(list_inner.height as usize);
    // Both lists open a row the same way: the status code, then the ✓ —
    // so the two columns read straight down whichever is up.
    let gutter = |file: Option<&crate::git_diff::DiffFile>, reviewed: bool| {
        let status = match file {
            Some(file) => Span::styled(
                format!("{} ", file.status_str()),
                Style::default().fg(super::change_color(file.xy, th)),
            ),
            None => Span::raw("   "),
        };
        let mark = if reviewed {
            Span::styled("✓ ", Style::default().fg(th.ok))
        } else {
            Span::raw("  ")
        };
        vec![status, mark]
    };
    // Each file's `+A −R` pinned to the row's right edge; the path gets
    // what is left, cut at its end so it reads from its root.
    let counts = CountColumns::of(view.files.iter().map(|f| f.lines));
    let row_w = row_width(list_inner);
    let text_w = left_room(row_w, counts.width()).saturating_sub(GUTTER_W);
    match &view.tree {
        None => {
            for (row, (i, m)) in view.matches.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let file = &view.files[m.file];
                let mut spans = gutter(Some(file), view.reviewed.contains_key(&file.path));
                let shown = truncate_cells(&file.path, text_w);
                let positions = visible_positions(&m.positions, &shown, &file.path);
                spans.extend(fuzzy_highlight_spans(&shown, positions, th));
                if let Some(orig) = &file.orig_path {
                    let rest = text_w.saturating_sub(shown.width());
                    if rest > 3 {
                        spans.push(Span::styled(
                            truncate(&format!(" ← {orig}"), rest),
                            Style::default().fg(th.dim),
                        ));
                    }
                }
                let spans = pin_right(spans, counts.spans(file.lines, th), row_w);
                render_row(f, row_area, spans, i == view.selected, focused, th);
            }
        }
        // The TREE BROWSER's rows behind the flat list's gutter: a
        // directory wears the fold marker and the accent, and its ✓ once
        // every file under it has one.
        Some(tree) => {
            let done = tree.reviewed_nodes(&view.files, &view.reviewed);
            for (row, (i, r)) in tree.rows.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let node = &tree.nodes[r.node];
                let file = tree.file_of[r.node].map(|f| &view.files[f]);
                let indent = "  ".repeat(node.depth);
                let marker = if !node.is_dir {
                    "  "
                } else if tree.is_open(r.node, !view.filter.is_empty()) {
                    "▾ "
                } else {
                    "▸ "
                };
                let budget = text_w.saturating_sub(indent.chars().count() + 2);
                let shown = truncate_cells(&node.name, budget);
                let mut spans = gutter(file, done[r.node]);
                spans.push(Span::raw(indent));
                spans.push(Span::styled(marker, Style::default().fg(th.accent)));
                if node.is_dir {
                    spans.push(Span::styled(shown, Style::default().fg(th.accent)));
                } else {
                    let positions = visible_positions(&r.positions, &shown, &node.name);
                    spans.extend(fuzzy_highlight_spans(&shown, positions, th));
                }
                let lines = file.and_then(|f| f.lines);
                let spans = pin_right(spans, counts.spans(lines, th), row_w);
                render_row(f, row_area, spans, i == tree.selected, focused, th);
            }
        }
    }
    (list_inner, start)
}

/// What the reading pane reads, as its border names it — the commit on
/// screen, what was ticked, the uncommitted changes, a pull request — and,
/// stepping through the ticked one at a time, its place among them.
fn reading(view: &DiffView) -> (String, Option<(usize, usize)>) {
    if let Some(list) = &view.commits {
        let title = list
            .showing()
            .and_then(|showing| list.reading_title(&showing));
        return (
            title.unwrap_or_else(|| view.branch.clone()),
            list.step_place(),
        );
    }
    let title = view.head.iter().find_map(|line| match line {
        crate::diff_doc::Head::Title(title) => Some(title.clone()),
        _ => None,
    });
    (title.unwrap_or_else(|| view.branch.clone()), None)
}

/// The reading pane: the selected file's diff — or, on a tree directory's
/// row, what changed under it — wrapped at its width, its explanation at
/// its foot. The file itself is the Files list's cursor; the border says
/// what it is read as part of, and the file's `+A −R` on the right.
/// Returns the diff's own rect and the scroll it was drawn at, clamped.
fn draw_diff(f: &mut Frame, view: &DiffView, area: Rect, th: Theme) -> (Rect, usize) {
    let focused = view.focus == DiffFocus::Diff;
    let (title, place) = reading(view);
    let facts = &view.doc.facts;
    let facts_style = Style::default().fg(if focused { th.accent } else { th.muted });
    let mut right: Vec<Span> = Vec::new();
    if let Some((k, n)) = place {
        right.push(Span::styled(format!("{k} of {n}"), facts_style));
    }
    // A change with no lines to count — a binary file, a mode change, a
    // pure rename — says what it is instead.
    let what = if facts.added + facts.removed > 0 {
        vec![
            Span::styled(
                format!("+{}", facts.added),
                count_style(facts.added as u64, th.added, th),
            ),
            Span::styled(
                format!(" −{}", facts.removed),
                count_style(facts.removed as u64, th.removed, th),
            ),
        ]
    } else {
        facts
            .status
            .map(|status| vec![Span::styled(status, facts_style)])
            .unwrap_or_default()
    };
    if !right.is_empty() && !what.is_empty() {
        right.push(Span::styled(" · ", Style::default().fg(th.dim)));
    }
    right.extend(what);
    let right_w = spans_width(&right);
    // The border's corners and the spaces around each title.
    let room =
        usize::from(area.width).saturating_sub(6 + if right_w > 0 { right_w + 3 } else { 0 });
    let title = truncate_cells(&title, room);
    let mut block = panel_block(&title, focused, th);
    if right_w > 0 {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(right);
        spans.push(Span::raw(" "));
        block = block.title_top(Line::from(spans).right_aligned());
    }
    let inner = block.inner(area);
    // The explanation at the foot, right above the keys.
    let explain = diff_explanation(view);
    let (body, explain_row) = crate::hints::explain_area(&explain, inner, 2);
    let max_scroll = view.doc.max_scroll(body.width, body.height);
    let scroll = view.scroll.min(max_scroll);
    if let Some(pct) = (scroll * 100).checked_div(max_scroll) {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {pct}% "),
                Style::default().fg(th.dim),
            ))
            .right_aligned(),
        );
    }
    f.render_widget(block, area);
    let rows = view.doc.render(scroll, body.width, body.height, th);
    f.render_widget(Paragraph::new(rows), body);
    crate::hints::draw_explain(f, explain_row, &explain, th);
    (body, scroll)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commit_list::{Commit, CommitListing, Stat};
    use crate::git_diff::{DiffFile, LineChanges};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn commit(n: usize, subject: &str) -> Commit {
        Commit {
            sha: format!("sha{n}"),
            short: format!("abc{n}def"),
            parents: vec![format!("sha{}", n + 1)],
            author: "Dana".into(),
            time: 0,
            subject: subject.into(),
            body: "The webhook dispatcher drops events when the endpoint restarts, so retry it with a doubling wait.".into(),
            stat: Some(Stat {
                files: 1,
                lines: Some(LineChanges {
                    added: 20,
                    removed: 1,
                }),
            }),
        }
    }

    /// A worktree view with a dirty tree and three commits, one file whose
    /// diff has a line far longer than any pane.
    fn view() -> DiffView {
        let file = DiffFile {
            path: "src/retry.rs".into(),
            orig_path: None,
            xy: ['M', ' '],
            lines: None,
        };
        let mut view = DiffView::new(
            "/nonexistent-orion-diff-test".into(),
            "feat".into(),
            vec![file],
            true,
        );
        view.commits = Some(Box::new(CommitList::from_listing(CommitListing {
            base: Some("origin/main".into()),
            merge_base: Some("mb".into()),
            head: Some("sha0".into()),
            total: 3,
            commits: vec![
                commit(0, "Try six times"),
                commit(
                    1,
                    "Point the notes at the retry helper, which now lives in its own module",
                ),
                commit(2, "Add a retry helper with exponential backoff"),
            ],
            uncommitted: Some(Stat {
                files: 2,
                lines: Some(LineChanges {
                    added: 124,
                    removed: 0,
                }),
            }),
        })));
        let long = "x".repeat(400);
        let diff = format!(
            "diff --git a/src/retry.rs b/src/retry.rs\n--- a/src/retry.rs\n+++ b/src/retry.rs\n@@ -1,2 +1,3 @@ pub fn retry()\n fn a() {{}}\n+    let wait = \"{long}\";\n-old\n context"
        );
        view.head = view.commits.as_ref().unwrap().commits[2].head(0, Some("commit 1 of 2"));
        view.show_diff(Some("src/retry.rs"), diff, false);
        view
    }

    fn screen(view: DiffView, width: u16, height: u16) -> (String, App) {
        let mut app = App::new();
        app.overlay = Some(Overlay::Diff(view));
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| super::super::draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        (text, app)
    }

    /// Nothing the viewer draws runs off its pane: at a wide terminal and
    /// a narrow one, the 400-character line wraps inside the diff, every
    /// row of it within the frame.
    #[test]
    fn everything_wraps_inside_the_modal() {
        for (width, height) in [(150, 42), (100, 30), (72, 30)] {
            let (text, app) = screen(view(), width, height);
            let Some(Overlay::Diff(v)) = &app.overlay else {
                panic!("the viewer is up");
            };
            let pane = v.diff_area;
            let xs = text.lines().nth(usize::from(pane.y) + 3).unwrap_or("");
            assert!(!xs.is_empty());
            let wrapped = text
                .lines()
                .skip(usize::from(pane.y))
                .take(usize::from(pane.height))
                .filter(|l| l.contains("xxxxxxxx"))
                .count();
            let rows = usize::from(v.view_height);
            let code_w = usize::from(v.view_width).saturating_sub(10);
            assert!(
                wrapped >= (400 / code_w.max(1)).min(rows / 2),
                "{width}x{height}: the long line wraps onto {wrapped} rows\n{text}"
            );
            // Each of those rows ends inside the pane's right border.
            for line in text.lines().filter(|l| l.contains("xxxxxxxx")) {
                let chars: Vec<char> = line.chars().collect();
                let edge = usize::from(pane.x + pane.width - 1);
                assert_eq!(chars.get(edge), Some(&'│'), "{width}: {line}");
            }
            assert!(text.contains("[ ] Uncommitted"), "{text}");
            assert!(text.contains("╭ Uncommitted changes ─"), "{text}");
        }
    }

    #[test]
    fn the_hints_come_from_the_key_table_in_every_panel_and_state() {
        let mut v = view();
        for focus in [DiffFocus::Commits, DiffFocus::Files, DiffFocus::Diff] {
            v.focus = focus;
            crate::hints::assert_hints_from(&diff_hints(&v), diff_keys::ALL_KEYS);
        }
        v.focus = DiffFocus::Commits;
        let text = crate::hints::text(&diff_hints(&v), 200);
        assert!(
            text.starts_with("Space tick · ^A all · ⇧←/⇧→ commit · Tab files"),
            "{text}"
        );
        let list = v.commits.as_mut().unwrap();
        list.ticked.extend([Row::Commit(0), Row::Commit(2)]);
        let text = crate::hints::text(&diff_hints(&v), 200);
        assert!(text.contains("^G one at a time · ⇧←/⇧→ step"), "{text}");
        v.commits.as_mut().unwrap().toggle_mode();
        let text = crate::hints::text(&diff_hints(&v), 200);
        assert!(text.contains("^G together"), "{text}");
        v.focus = DiffFocus::Diff;
        let text = crate::hints::text(&diff_hints(&v), 200);
        assert!(text.starts_with("↑↓ scroll · PgUp/PgDn page"), "{text}");
        assert!(text.ends_with("Esc close"), "{text}");
        // A pull request's view: no commit keys at all.
        let mut pr = view();
        pr.commits = None;
        let text = crate::hints::text(&diff_hints(&pr), 200);
        assert!(!text.contains('⇧') || text.contains("⇧↑"), "{text}");
        assert!(!text.contains("^G") && !text.contains("Space"), "{text}");
    }

    /// The explanation says what is on screen in each state, and the keys
    /// it names are the table's.
    #[test]
    fn the_explanation_follows_the_ticks() {
        let mut v = view();
        assert!(diff_explanation(&v).starts_with("The uncommitted changes"));
        let list = v.commits.as_mut().unwrap();
        list.select(2);
        assert!(diff_explanation(&v).starts_with("The commit under the cursor"));
        let list = v.commits.as_mut().unwrap();
        list.tick_all();
        let text = diff_explanation(&v);
        assert!(
            text.starts_with("Every commit made on the branch since origin/main"),
            "{text}"
        );
        assert!(text.contains("^A unticks"), "{text}");
        let list = v.commits.as_mut().unwrap();
        list.toggle_mode();
        assert!(
            diff_explanation(&v).starts_with("Commit 2 of the 3 ticked"),
            "stepping starts on the cursor's commit: {}",
            diff_explanation(&v)
        );
    }

    /// `⌘O` opens the file at the line at the top of the diff: the first
    /// line of code from there down, by its number in the new file.
    #[test]
    fn the_line_on_screen_follows_the_scroll() {
        let mut v = view();
        assert_eq!(
            v.line_on_screen(),
            1,
            "not drawn yet: the first line of code"
        );
        v.view_width = 80;
        let added = v
            .doc
            .lines
            .iter()
            .position(|l| matches!(l, crate::diff_doc::DocLine::Code { new: Some(2), .. }))
            .unwrap();
        v.scroll = (0..added).map(|i| v.doc.rows_of(i, 80)).sum();
        assert_eq!(v.line_on_screen(), 2);
        v.scroll = v.doc.total_rows(80);
        assert_eq!(v.line_on_screen(), 1, "past the end: the first line");
    }

    /// A file row keeps its path's start, cut at the end, with its counts
    /// pinned to the right edge in columns every row shares; the commit
    /// rows the same, a subject cut rather than wrapped.
    #[test]
    fn rows_keep_their_start_and_pin_their_counts_right() {
        let mut v = view();
        let counted = |added, removed| Some(LineChanges { added, removed });
        v.files = vec![
            DiffFile {
                path: "crates/orion-core/src/webhooks/dispatcher_with_backpressure.rs".into(),
                orig_path: None,
                xy: ['M', ' '],
                lines: counted(22, 12),
            },
            DiffFile {
                path: "src/retry.rs".into(),
                orig_path: None,
                xy: ['A', ' '],
                lines: counted(124, 0),
            },
        ];
        v.recompute_matches();
        let (text, app) = screen(v, 150, 42);
        let Some(Overlay::Diff(v)) = &app.overlay else {
            panic!("the viewer is up");
        };
        let default = crate::app::DEFAULT_DIFF_FILES_W;
        assert_eq!(v.column_w, default, "the column's default\n{text}");
        let row = |needle: &str| {
            let line = text
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle}\n{text}"));
            // The left column only, borders included.
            line.chars()
                .skip(usize::from(v.area.x))
                .take(usize::from(default))
                .collect::<String>()
        };
        let long = row("crates/orion-core/src/webhooks/dispatch");
        assert!(
            long.contains("│▌M    crates/orion-core/src/webhooks/dispatcher_with"),
            "{long}"
        );
        assert!(long.contains("…"), "cut at the end: {long}");
        assert!(long.ends_with(" +22 −12 │"), "{long}");
        assert!(
            row("src/retry.rs").ends_with("+124  −0 │"),
            "the columns line up"
        );
        let commit = row("Point the notes at the retry helper");
        assert!(
            commit.contains("[ ] Point the notes at the retry helper"),
            "{commit}"
        );
        assert!(commit.contains("…"), "a long subject is cut: {commit}");
        assert!(commit.contains("… abc1def "), "{commit}");
        assert!(commit.ends_with("  +20 −1 │"), "{commit}");
        assert!(
            row("[ ] Uncommitted changes").ends_with("2 files         +124 −0 │"),
            "{text}"
        );
    }

    /// The diff pane's border names what is read and the file's counts;
    /// the commit's message is not drawn over the code.
    #[test]
    fn the_border_names_the_commit_and_the_counts() {
        let mut v = view();
        v.commits.as_mut().unwrap().select(3);
        let (text, _) = screen(v, 150, 42);
        let top = text
            .lines()
            .find(|l| l.contains("Add a retry helper with exponential backoff ─"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(top.contains(" +1 −1 ╮"), "{top}");
        assert!(
            !text.contains("abc2def · Dana"),
            "no meta line over the diff\n{text}"
        );
        assert!(
            !text.contains("The webhook dispatcher drops events"),
            "no body over the diff\n{text}"
        );
        // Stepping through the ticked, the place joins the counts.
        let mut v = view();
        let list = v.commits.as_mut().unwrap();
        list.ticked.extend([Row::Commit(0), Row::Commit(2)]);
        list.toggle_mode();
        let (text, _) = screen(v, 150, 42);
        assert!(text.contains("of 2 · +1 −1 ╮"), "{text}");
    }
}
