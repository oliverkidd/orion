//! The DIFF VIEWER (`⌘E`) on screen.
//!
//! A left column and the diff beside it, the diff taking the modal's whole
//! height — on a 150-column terminal a 34-column column and a diff a
//! hundred wide: every row of height goes to the code, which wraps, rather
//! than to a strip of commits stacked over it. A column nobody has dragged
//! keeps to two fifths of a narrow modal. The column is the COMMIT LIST
//! over the changed files, in reading order: what is on screen, its files,
//! the selected file's diff. A pull request's view has no commit list, and
//! the files take the column.
//!
//! - **The COMMIT LIST** gives each row a box: `[✓]` ticked, `[ ]` not. A
//!   commit's subject wraps under its box, and its short sha, age and
//!   counts sit on the row under it. Rows whose changes are in the diff on
//!   screen are bold. The panel's bottom edge says how many are ticked and
//!   how they are read.
//! - **The diff** is a `diff_doc::DiffDoc`: wrapped, numbered, syntax
//!   coloured, its file's facts (`added · +20 −0`) on the top edge, and
//!   what it shows — a commit's message, `commit 2 of 3`, the ticked
//!   commits — heading it. One dim line at its foot explains what is on
//!   screen and how to change it.
//! - **The keys** are on the modal's bottom border, for the panel that has
//!   them — which wears the accent — from [`diff_keys`], the table the key
//!   handler matches.

use super::{
    below_first_row, centered_rect_pct, empty_list_row, fuzzy_highlight_spans, panel_block,
    render_button, render_row, row_rect, search_line, truncate, visible_positions, NO_MATCHES,
    SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
};
use crate::app::{App, DiffFocus, DiffView, Overlay};
use crate::commit_list::{CommitList, Row, Showing};
use crate::diff_doc::wrap_words;
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
    pub const ALL: Key = Key::new(&["ctrl+a"], "all");
    /// The ticked commits TOGETHER or ONE AT A TIME.
    pub const MODE: Key = Key::new(&["ctrl+g"], "one at a time");
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
    pub const REVIEWED: Key = Key::new(&["ctrl+r"], "reviewed");
    pub const TREE: Key = Key::new(&["ctrl+t"], "tree");
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
    hints.push(crate::hints::ESC_CLOSE.hint());
    hints
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
/// dragged, the default, but no more than two fifths of a narrow modal, so
/// the diff keeps the room it wraps in.
fn column_width(view: &DiffView, area: Rect) -> u16 {
    let mut want = view.files_width;
    if want == crate::app::DEFAULT_DIFF_FILES_W {
        want = want.min(area.width * 2 / 5);
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
    let inner_w = col_w.saturating_sub(2);
    let heights = view
        .commits
        .as_ref()
        .map(|list| entry_heights(list, inner_w))
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
            commits_a,
            now,
            th,
        ));
    }
    let (list_inner, files_top) = draw_files(f, view, files_a, th);
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
        v.files_width = col_w;
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

/// How many lines each COMMIT LIST row takes at `inner_w`.
fn entry_heights(list: &CommitList, inner_w: u16) -> Vec<usize> {
    if !list.loaded {
        return vec![1];
    }
    let text_w = text_width(inner_w);
    (0..list.row_count())
        .map(|i| match list.row(i) {
            Some(Row::Commit(c)) => wrap_words(&list.commits[c].subject, text_w).len() + 1,
            Some(Row::Uncommitted) => wrap_words("Uncommitted changes", text_w).len() + 1,
            _ => 1,
        })
        .collect()
}

/// The cells a row's words get: the panel's inner width less the cursor's
/// `▌` and the `[✓] ` box.
fn text_width(inner_w: u16) -> usize {
    usize::from(inner_w).saturating_sub(1 + 4).max(1)
}

/// The COMMIT LIST panel. Returns the first row drawn and each drawn row's
/// rect, for the pointer.
fn draw_commits(
    f: &mut Frame,
    list: &CommitList,
    heights: &[usize],
    focused: bool,
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
    let text_w = text_width(inner.width);
    let mut hits = Vec::new();
    let mut y = inner.y;
    for i in top..list.row_count() {
        let Some(row) = list.row(i) else {
            break;
        };
        let h = heights.get(i).copied().unwrap_or(1) as u16;
        let bottom = inner.y + inner.height;
        if y >= bottom {
            break;
        }
        let rect = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: h.min(bottom - y),
        };
        let lines = entry_lines(list, row, text_w, now, th);
        render_button(
            f,
            rect,
            lines,
            i == list.selected,
            focused,
            th,
            0,
            th.accent,
        );
        hits.push((rect, i));
        y += h;
    }
    (top, hits)
}

/// One COMMIT LIST row's lines: its box and its subject wrapped under it,
/// then its short sha, age and counts.
fn entry_lines(
    list: &CommitList,
    row: Row,
    text_w: usize,
    now: i64,
    th: Theme,
) -> Vec<Vec<Span<'static>>> {
    let dim = Style::default().fg(th.dim);
    if row == Row::Older {
        let text = match list.paging {
            Some(_) => "reading older commits…".to_string(),
            None => format!("… {} older commits", list.total - list.commits.len()),
        };
        return vec![vec![Span::styled(truncate(&text, text_w + 4), dim)]];
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
    let (subject, stat, meta) = match row {
        Row::Commit(i) => {
            let c = &list.commits[i];
            let mut meta = vec![c.short.clone(), c.ago(now)];
            if !c.author.is_empty() {
                meta.insert(1, c.author.clone());
            }
            (c.subject.clone(), c.stat, meta)
        }
        _ => (
            "Uncommitted changes".to_string(),
            list.uncommitted,
            Vec::new(),
        ),
    };
    let mut lines: Vec<Vec<Span<'static>>> = wrap_words(&subject, text_w)
        .into_iter()
        .enumerate()
        .map(|(n, text)| {
            let lead = if n == 0 {
                Span::styled(mark, mark_style)
            } else {
                Span::raw("    ")
            };
            vec![lead, Span::styled(text, words)]
        })
        .collect();
    lines.push(meta_line(meta, stat, text_w, th));
    lines
}

/// The row under a commit's subject: short sha, author, age, then `+A −R`
/// — the author, then the age, given up to keep the counts on a narrow
/// column, and the counts last.
fn meta_line(
    mut meta: Vec<String>,
    stat: Option<crate::commit_list::Stat>,
    width: usize,
    th: Theme,
) -> Vec<Span<'static>> {
    let dim = Style::default().fg(th.dim);
    let counts: Vec<Span<'static>> = match stat.and_then(|s| s.lines.map(|l| (s.files, l))) {
        Some((files, lines)) => {
            let mut spans = Vec::new();
            if meta.is_empty() {
                let noun = if files == 1 { "file" } else { "files" };
                spans.push(Span::styled(format!("{files} {noun} "), dim));
            }
            spans.push(Span::styled(
                format!("+{}", lines.added),
                Style::default().fg(th.added),
            ));
            spans.push(Span::styled(
                format!(" −{}", lines.removed),
                Style::default().fg(th.removed),
            ));
            spans
        }
        None => Vec::new(),
    };
    let counts_w: usize = counts.iter().map(|s| s.content.width()).sum();
    let fits = |meta: &[String]| {
        let text = meta.join(" · ").width();
        let gap = if text > 0 && counts_w > 0 { 3 } else { 0 };
        text + gap + counts_w <= width
    };
    // The author goes first, then the age; the sha stays.
    while meta.len() > 1 && !fits(&meta) {
        let drop = if meta.len() == 3 { 1 } else { meta.len() - 1 };
        meta.remove(drop);
    }
    let mut spans = vec![Span::raw("    ")];
    let mut used = 0;
    for (n, part) in meta.iter().enumerate() {
        if n > 0 {
            spans.push(Span::styled(" · ", dim));
            used += 3;
        }
        let style = if n == 0 {
            Style::default().fg(th.accent)
        } else {
            dim
        };
        let part = truncate(part, width.saturating_sub(used));
        used += part.width();
        spans.push(Span::styled(part, style));
    }
    if counts_w > 0 && used + 3 + counts_w <= width {
        if used > 0 {
            spans.push(Span::styled(" · ", dim));
        }
        spans.extend(counts);
    } else if used == 0 {
        spans.extend(counts);
    }
    spans
}

/// The changed-file list — flat paths, or the directory tree (`Ctrl+t`) —
/// under its always-live filter. Returns the rows' rect and the first row
/// drawn.
fn draw_files(f: &mut Frame, view: &DiffView, area: Rect, th: Theme) -> (Rect, usize) {
    let focused = view.focus == DiffFocus::Files;
    let mut title = if view.listing.is_some() && view.files.is_empty() {
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
    match &view.tree {
        None => {
            for (row, (i, m)) in view.matches.iter().enumerate().skip(start).enumerate() {
                let Some(row_area) = row_rect(list_inner, row) else {
                    break;
                };
                let file = &view.files[m.file];
                let budget = (list_inner.width as usize).saturating_sub(5);
                let mut spans = gutter(Some(file), view.reviewed.contains_key(&file.path));
                let (shown, positions) = path_tail(&file.path, &m.positions, budget);
                let used = shown.chars().count();
                spans.extend(fuzzy_highlight_spans(&shown, &positions, th));
                if let Some(orig) = &file.orig_path {
                    let rest = budget.saturating_sub(used);
                    if rest > 3 {
                        spans.push(Span::styled(
                            truncate(&format!(" ← {orig}"), rest),
                            Style::default().fg(th.dim),
                        ));
                    }
                }
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
                let budget =
                    (list_inner.width as usize).saturating_sub(5 + indent.chars().count() + 2);
                let shown = truncate(&node.name, budget);
                let mut spans = gutter(file, done[r.node]);
                spans.push(Span::raw(indent));
                spans.push(Span::styled(marker, Style::default().fg(th.accent)));
                if node.is_dir {
                    spans.push(Span::styled(shown, Style::default().fg(th.accent)));
                } else {
                    let positions = visible_positions(&r.positions, &shown, &node.name);
                    spans.extend(fuzzy_highlight_spans(&shown, positions, th));
                }
                render_row(f, row_area, spans, i == tree.selected, focused, th);
            }
        }
    }
    (list_inner, start)
}

/// The reading pane: the selected file's diff under the REVIEW HEAD — or,
/// on a tree directory's row, what changed under it — wrapped at its
/// width, its explanation at its foot. Returns the diff's own rect and the
/// scroll it was drawn at, clamped.
fn draw_diff(f: &mut Frame, view: &DiffView, area: Rect, th: Theme) -> (Rect, usize) {
    let focused = view.focus == DiffFocus::Diff;
    // The file's path, a tree directory's, or — with neither — what the
    // viewer is of: the branch, the pull request.
    let sel_path = match view.selected_dir() {
        Some(dir) => format!("{dir}/"),
        None => view.selected_path().unwrap_or(&view.branch).to_string(),
    };
    let reviewed = view.reviewed.contains_key(&sel_path);
    let facts = &view.doc.facts;
    // The file's facts on the top edge's right: what happened to it and
    // how many lines each way.
    let mut what: Vec<String> = Vec::new();
    if let Some(status) = facts.status {
        what.push(match &facts.from {
            Some(from) => format!("{status} from {from}"),
            None => status.to_string(),
        });
    }
    if facts.added + facts.removed > 0 {
        what.push(format!("+{} −{}", facts.added, facts.removed));
    }
    let what = what.join(" · ");
    let room = (area.width as usize).saturating_sub(6);
    let what = if what.width() + 12 <= room {
        what
    } else {
        String::new()
    };
    let mut title = sel_path.clone();
    if reviewed {
        title.push_str(" ✓");
    }
    let title_room = room.saturating_sub(if what.is_empty() { 0 } else { what.width() + 3 });
    let title = truncate_left(&title, title_room);
    let mut block = panel_block(&title, focused, th);
    if !what.is_empty() {
        block = block.title_top(
            Line::from(Span::styled(
                format!(" {what} "),
                Style::default().fg(if focused { th.accent } else { th.muted }),
            ))
            .right_aligned(),
        );
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

/// `path` cut to `max` columns from the left — its file name kept, behind
/// a `…` — with the filter's matched positions moved to match.
fn path_tail(path: &str, positions: &[usize], max: usize) -> (String, Vec<usize>) {
    let len = path.chars().count();
    if len <= max {
        return (path.to_string(), positions.to_vec());
    }
    let cut = len - max.saturating_sub(1);
    let shown = std::iter::once('…').chain(path.chars().skip(cut)).collect();
    let moved = positions
        .iter()
        .filter(|&&p| p >= cut)
        .map(|p| p - cut + 1)
        .collect();
    (shown, moved)
}

/// `text` cut to `max` cells from the left, keeping its end — a path's
/// file name — behind a `…`.
fn truncate_left(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    let mut out: Vec<char> = Vec::new();
    let mut used = 1;
    for ch in text.chars().rev() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > max {
            break;
        }
        used += w;
        out.push(ch);
    }
    out.push('…');
    out.into_iter().rev().collect()
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
    /// row of it within the frame, and the commit list's subjects wrap
    /// under their boxes.
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
            assert!(text.contains("[ ] Uncommitted changes"), "{text}");
            assert!(
                text.contains("commit 1 of 2 · Add a retry helper"),
                "{text}"
            );
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

    #[test]
    fn a_long_path_keeps_its_file_name() {
        assert_eq!(
            truncate_left("crates/orion-tui/src/retry.rs", 12),
            "…rc/retry.rs"
        );
        assert_eq!(truncate_left("a.rs", 12), "a.rs");
        let (shown, positions) = path_tail(".claude/settings.local.json", &[0, 8, 26], 20);
        assert_eq!(shown, "…settings.local.json");
        assert_eq!(positions, [1, 19], "the matches that are still in sight");
        assert_eq!(path_tail("a.rs", &[0], 20), ("a.rs".to_string(), vec![0]));
    }
}
