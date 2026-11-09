//! The PR REVIEW in its pull request's DIFF VIEWER: one screen for reading
//! a pull request's changes and reviewing them.
//!
//! A pull request's viewer carries its review (`DiffView::review`) from
//! the moment it opens, however it was opened. The viewer stays what it
//! is — the COMMIT LIST and the files down the left, the diff on the
//! right, its keys — and the review adds three things to it:
//!
//! - **Threads under their lines.** While the whole pull request is on
//!   screen ([`whole_pr`]: every commit ticked, as it opens) the reading
//!   pane is the review's: the file's diff with its threads, a cursor on
//!   its lines and threads, and the review's keys on it ([`handle_key`]).
//!   GitHub places a thread on the pull request's own diff, so a commit
//!   read on its own — or a few ticked together — is the plain diff, and
//!   `c` there says how to get back to the whole.
//! - **A thread count on each file** in the list.
//! - **The bar** along the bottom: the verdict and the summary, a panel
//!   `Tab` reaches after the diff (`DiffFocus::Bar`), and `⌘⇧R` from
//!   anywhere in the viewer sends the review.
//!
//! The viewer's cursor is the one that picks the file: [`sync`] puts its
//! file and its panel to the review before anything reads them, and a
//! review move that crosses files — `]` onto the next thread — moves the
//! viewer's cursor after it ([`run`]).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};

use super::draw::{Bar, Laid};
use super::layout::Stop;
use super::{bar, draw, keys, write, Focus, ReviewScreen, WHEEL_LINES};
use crate::app::{App, DiffFocus, DiffView, Overlay};
use crate::commit_list::Row;
use crate::diff_doc::DocLine;
use crate::flash::Flash;
use crate::hints::Hint;

/// The rows the commits, files and diff keep over the bar, however long
/// the summary.
const MIN_PANES_H: u16 = 8;

/// The review in the DIFF VIEWER on screen.
pub(crate) fn review_mut(app: &mut App) -> Option<&mut ReviewScreen> {
    match &mut app.overlay {
        Some(Overlay::Diff(view)) => view.review.as_deref_mut(),
        _ => None,
    }
}

/// Whether the viewer shows the whole pull request: every commit ticked —
/// or its one commit on screen — and none past the page read. Only then is
/// each file's diff the one GitHub places the threads on.
pub(crate) fn whole_pr(view: &DiffView) -> bool {
    let Some(list) = &view.commits else {
        return true;
    };
    if !list.loaded || list.has_older() || list.uncommitted.is_some() {
        return false;
    }
    list.all_ticked() || (list.commits.len() == 1 && list.on_screen(Row::Commit(0)))
}

/// Whether the review has the reading pane: the whole pull request on
/// screen, and the file under the viewer's cursor among the ones it read.
pub(crate) fn reviewing(view: &DiffView) -> bool {
    let Some(review) = &view.review else {
        return false;
    };
    let on = view.selected_file().map(|f| f.path.as_str());
    whole_pr(view) && on.is_some() && review.current().map(|f| f.path.as_str()) == on
}

/// The review's word for the viewer's panel.
fn review_focus(focus: DiffFocus) -> Focus {
    match focus {
        DiffFocus::Diff => Focus::Diff,
        DiffFocus::Bar => Focus::Bar,
        DiffFocus::Commits | DiffFocus::Files => Focus::Files,
    }
}

/// The viewer's file and panel put to its review.
pub(crate) fn sync(view: &mut DiffView) {
    let path = view.selected_file().map(|f| f.path.clone());
    let focus = view.focus;
    let Some(review) = view.review.as_deref_mut() else {
        return;
    };
    review.focus = review_focus(focus);
    let at = path.and_then(|p| review.file_list().iter().position(|f| f.path == p));
    if let Some(i) = at.filter(|i| *i != review.file) {
        review.pick_file(i);
    }
}

/// `act` on the review, and what it moved put back to the viewer: the
/// panel with the keys, and — when it went to another file — the viewer's
/// cursor onto that file.
fn run(app: &mut App, act: impl FnOnce(&mut App)) {
    let before = review_mut(app).map(|r| r.file);
    act(app);
    let Some(Overlay::Diff(view)) = &mut app.overlay else {
        return;
    };
    let Some(review) = view.review.as_deref() else {
        return;
    };
    let moved = Some(review.file) != before;
    let path = review.current().map(|f| f.path.clone());
    view.focus = match review.focus {
        Focus::Diff => DiffFocus::Diff,
        Focus::Bar => DiffFocus::Bar,
        Focus::Files if view.focus == DiffFocus::Commits => DiffFocus::Commits,
        Focus::Files => DiffFocus::Files,
    };
    if let (true, Some(path)) = (moved, path) {
        if view.select_path(&path) {
            crate::git_diff::load_selected_diff(view);
        }
    }
    app.dirty = true;
}

/// Whether Esc is the review's — a comment being written, lines marked,
/// the bar — rather than the viewer's way out.
pub(crate) fn takes_esc(view: &DiffView) -> bool {
    view.review.as_deref().is_some_and(|r| {
        r.composer.is_some()
            || view.focus == DiffFocus::Bar
            || (r.anchor.is_some() && reviewing(view))
    })
}

/// What `c` says over a diff the threads aren't placed on.
fn whole_note() -> String {
    format!(
        "comments go on the whole pull request — {} ticks every commit",
        crate::ui::diff_keys::ALL.label()
    )
}

// ---- keys ----

/// A key in a pull request's viewer, the review's first: a comment being
/// written takes every one, `⌘⇧R` sends the review from anywhere, the bar
/// has its own, and the diff the review's while it has the reading pane.
/// False for what is the viewer's.
pub(crate) fn handle_key(app: &mut App, key: &KeyEvent) -> bool {
    let key = *key;
    let Some(Overlay::Diff(view)) = &mut app.overlay else {
        return false;
    };
    if view.review.is_none() {
        return false;
    }
    sync(view);
    let on = reviewing(view);
    let focus = view.focus;
    let writing = view.review.as_ref().is_some_and(|r| r.composer.is_some());
    if writing {
        run(app, |app| write::composer_key(app, key));
        return true;
    }
    if keys::SUBMIT.matches(&key) {
        run(app, bar::submit);
        return true;
    }
    match focus {
        // Tab and ⇧Tab are the viewer's, round its panels.
        DiffFocus::Bar if crate::nav::panel(&key).is_some() => false,
        DiffFocus::Bar => {
            run(app, |app| bar::bar_key(app, key));
            true
        }
        DiffFocus::Diff if on => diff_key(app, key),
        DiffFocus::Diff if keys::COMMENT.matches(&key) => {
            app.flash = Some(Flash::note(whole_note()));
            true
        }
        _ => false,
    }
}

/// A key on the reading pane while the review has it: the cursor along
/// the lines and threads, the comments, the threads. False for the
/// viewer's own — Tab, ⌘R, the commit steps, a letter for the filter.
fn diff_key(app: &mut App, key: KeyEvent) -> bool {
    let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
    let Some(review) = review_mut(app) else {
        return false;
    };
    let on_thread = review.thread_at_cursor().is_some();
    match key.code {
        _ if crate::nav::closes(&key) && review.anchor.is_some() => review.anchor = None,
        _ if keys::NEXT_THREAD.matches(&key) || keys::PREV_THREAD.matches(&key) => {
            let forward = keys::NEXT_THREAD.matches(&key);
            run(app, |app| {
                let jumped = review_mut(app).is_some_and(|r| r.jump_thread(forward));
                if !jumped {
                    app.flash = Some(Flash::note("no review threads on this pull request yet"));
                }
            });
        }
        _ if keys::RANGE.matches(&key) => review.extend_range(key.code == KeyCode::Down),
        _ if keys::COMMENT.matches(&key) => write::start_comment(app),
        _ if keys::REPLY.matches(&key) => write::start_reply(app),
        _ if keys::RESOLVE.matches(&key) => write::toggle_resolved(app),
        _ if keys::DROP.matches(&key) && on_thread => write::drop_pending(app),
        _ if !plain => return false,
        KeyCode::Up | KeyCode::Down => {
            review.anchor = None;
            review.step(if key.code == KeyCode::Up { -1 } else { 1 });
        }
        KeyCode::PageUp => review.page(false),
        KeyCode::PageDown | KeyCode::Char(' ') => review.page(true),
        KeyCode::Home => review.jump_to_end(false),
        KeyCode::End => review.jump_to_end(true),
        KeyCode::Enter => edit_at_cursor(app),
        _ => return false,
    }
    app.dirty = true;
    true
}

/// Enter on the reading pane: the file in the BUILT-IN EDITOR at the line
/// under the cursor — the line a thread is on, on a thread.
fn edit_at_cursor(app: &mut App) {
    let Some(review) = review_mut(app) else {
        return;
    };
    let Some(file) = review.current().filter(|f| f.status != 'D') else {
        return;
    };
    let line = match review.cursor_or_first() {
        Some(Stop::Line(at)) => file.doc.lines[at.min(file.doc.lines.len())..]
            .iter()
            .find_map(|line| match line {
                DocLine::Code { new: Some(n), .. } => Some(u64::from(*n)),
                _ => None,
            }),
        Some(Stop::Thread(_)) => review
            .thread_at_cursor()
            .and_then(|t| t.shown_line())
            .map(u64::from),
        None => None,
    };
    let path = file.path.clone();
    crate::diff_actions::edit_at(app, path, line.unwrap_or(1));
}

/// A paste in a pull request's viewer: into the comment being written,
/// or the summary on the bar.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(Overlay::Diff(view)) = &mut app.overlay else {
        return false;
    };
    sync(view);
    let Some(review) = view.review.as_deref_mut() else {
        return false;
    };
    let took = write::paste(review, text) || bar::paste(review, text);
    if took {
        app.dirty = true;
    }
    took
}

// ---- the mouse ----

/// The mouse in a pull request's viewer, the review's first: the bar's
/// verdicts and summary, and — while it has the reading pane — a click
/// on a line or a thread puts the cursor there and the wheel scrolls it.
/// False for what is the viewer's.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent) -> bool {
    let pos = Position::new(mouse.column, mouse.row);
    let Some(Overlay::Diff(view)) = &mut app.overlay else {
        return false;
    };
    if view.review.is_none() {
        return false;
    }
    sync(view);
    let on = reviewing(view);
    let Some(review) = view.review.as_deref_mut() else {
        return false;
    };
    let in_diff = on && review.diff_area.contains(pos);
    let click = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
    // A comment being written keeps the keys: a click elsewhere would
    // move the viewer off the file it is on.
    if review.composer.is_some() && click && !in_diff {
        return true;
    }
    let took = match mouse.kind {
        MouseEventKind::ScrollDown if in_diff => {
            review.scroll += WHEEL_LINES;
            review.follow = false;
            true
        }
        MouseEventKind::ScrollUp if in_diff => {
            review.scroll = review.scroll.saturating_sub(WHEEL_LINES);
            review.follow = false;
            true
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(&(_, verdict)) = review.verdict_rects.iter().find(|(r, _)| r.contains(pos))
            {
                review.focus = Focus::Bar;
                review.bar_row = bar::BarRow::Verdict;
                review.verdict = verdict;
                review.notice = None;
                true
            } else if review.bar_area.contains(pos) {
                review.focus = Focus::Bar;
                true
            } else if in_diff {
                let row = review.scroll + usize::from(pos.y.saturating_sub(review.diff_area.y));
                let hit = review
                    .stop_rows
                    .iter()
                    .find(|(_, first, n)| (*first..first + n).contains(&row))
                    .map(|(stop, ..)| stop.clone());
                review.focus = Focus::Diff;
                if let Some(stop) = hit {
                    review.cursor = Some(stop);
                }
                true
            } else {
                false
            }
        }
        _ => false,
    };
    if took {
        view.focus = match review.focus {
            Focus::Bar => DiffFocus::Bar,
            Focus::Diff => DiffFocus::Diff,
            Focus::Files => view.focus,
        };
        app.dirty = true;
    }
    took
}

// ---- drawing ----

/// The bar's rows along the bottom of the viewer's `area`, for a pull
/// request's: the panels keep the rest.
pub(crate) fn split_bar(view: &DiffView, area: Rect) -> (Rect, Option<Rect>) {
    let Some(review) = &view.review else {
        return (area, None);
    };
    let height = draw::bar_height(review, area.width).min(area.height.saturating_sub(MIN_PANES_H));
    let [panes, bar] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(height)]).areas(area);
    (panes, Some(bar))
}

/// What the draw left on screen, written back to the review on `app`'s
/// viewer: the reading pane's layout while the review had it, and the
/// bar's verdicts and summary.
pub(crate) fn write_back(
    review: &mut ReviewScreen,
    diff: Rect,
    laid: Option<Laid>,
    bar_area: Rect,
    bar: Option<Bar>,
) {
    review.diff_area = diff;
    review.diff_h = diff.height;
    match laid {
        Some(laid) => {
            review.scroll = laid.scroll;
            review.stop_rows = laid.stops;
            review.cursor = laid.cursor;
            review.follow = false;
            // ↑/↓ in the comment walk the rows it was drawn in.
            if let (Some(composer), Some(at)) = (&mut review.composer, laid.composer) {
                composer.input.set_view(at);
            }
        }
        None => review.stop_rows.clear(),
    }
    review.bar_area = bar_area;
    if let Some(bar) = bar {
        review.verdict_rects = bar.verdicts;
        if let Some(at) = bar.summary {
            review.summary.set_view(at);
        }
    }
}

/// The keys along the viewer's bottom edge while the review has them: the
/// comment being written, the bar, and the reading pane it has. None for
/// the viewer's own (`ui::diff_view::diff_hints`).
pub(crate) fn hints(view: &DiffView) -> Option<Vec<Hint>> {
    use crate::hints::ESC_CLOSE;
    use crate::ui::diff_keys;
    use keys::*;
    let review = view.review.as_deref()?;
    if review.composer.is_some() {
        return Some(vec![
            ADD.hint().kept(),
            NEWLINE.hint(),
            ESC_CLOSE.hint_as("cancel"),
        ]);
    }
    let next = diff_keys::PANEL.hint_as(panel_name(view.next_focus(true)));
    let mut hints = Vec::new();
    match view.focus {
        DiffFocus::Bar => {
            hints.push(SEND.hint().kept());
            if review.bar_row == bar::BarRow::Verdict {
                hints.push(VERDICT.hint());
            } else {
                hints.push(NEWLINE.hint());
            }
            hints.push(next);
            hints.push(ESC_CLOSE.hint_as("diff"));
            return Some(hints);
        }
        DiffFocus::Diff if reviewing(view) => {}
        _ => return None,
    }
    match review.thread_at_cursor() {
        Some(thread) => {
            hints.push(REPLY.hint().kept());
            hints.push(RESOLVE.hint_as(if thread.resolved { "reopen" } else { "resolve" }));
            if thread.my_pending().is_some() {
                hints.push(DROP.hint());
            }
        }
        None => {
            let lines = review.marked_lines().map_or(1, |(a, b)| b - a + 1);
            hints.push(match lines {
                1 => COMMENT.hint().kept(),
                n => COMMENT.hint_as(format!("comment on {n} lines")).kept(),
            });
            hints.push(RANGE.hint());
        }
    }
    hints.push(SUBMIT.hint());
    hints.push(MOVE.hint());
    hints.push(THREADS.hint());
    hints.push(PAGE.hint());
    if view.file_at_line().is_some() {
        hints.push(diff_keys::EDIT.hint());
    }
    hints.push(next);
    hints.push(diff_keys::REVIEWED.hint());
    hints.push(match view.back {
        _ if review.anchor.is_some() => ESC_CLOSE.hint_as("clear the lines"),
        Some(_) => ESC_CLOSE.hint_as("back to the pull request"),
        None => ESC_CLOSE.hint(),
    });
    Some(hints)
}

/// What `Tab` calls the panel it goes to.
pub(crate) fn panel_name(focus: DiffFocus) -> &'static str {
    match focus {
        DiffFocus::Commits => "commits",
        DiffFocus::Files => "files",
        DiffFocus::Diff => "diff",
        DiffFocus::Bar => "review",
    }
}
