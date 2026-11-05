//! The PR REVIEW: a pull request read and reviewed in orion, the way
//! GitHub's Files tab reviews it, written into GitHub's own pending
//! review (`github`).
//!
//! `⌘⇧R` in the PULL REQUESTS MODAL — or Enter on the PULL REQUEST PAGE's
//! Reviews tab — opens it on the pull request under the cursor, as a level
//! inside the modal: Esc goes back to it on the same row. On the left, the
//! files it changes, each with how many review threads are on it; on the
//! right, the selected file's diff (`diff_doc`, read from the project's
//! git as the DIFF VIEWER reads a pull request), every thread a block
//! under the line it is on — the other reviewers' comments on a grey rule,
//! yours on the accent ([`layout`]). The cursor walks the lines and the
//! threads; `]` and `[` go to the next and the previous thread, file to
//! file. `c` comments on the line under the cursor — `⇧↑`/`⇧↓` first make
//! it a run of lines — in place under it, `r` replies to a thread, `x`
//! resolves it and `⌫` drops a pending comment of yours ([`write`]).
//! The bar along the bottom holds the verdict and a summary, and `⌘⇧R`
//! sends the review with everything pending ([`bar`]).
//!
//! Nothing here blocks the loop: the diff is read on the blocking pool,
//! and every GitHub call is a task of its own. Their answers land through
//! one channel (`State::tx`), matched to the screen by ticket; a read for
//! a screen since closed is dropped, and a write the footer reports. What
//! goes wrong is said in the footer.

pub mod bar;
mod draw;
pub mod github;
pub mod layout;
pub mod write;

use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::app::{App, Overlay};
use crate::diff_doc::DiffDoc;
use crate::flash::Flash;
use crate::pr_modal::PullRequestsView;
use github::{ReviewThread, Threads};
use layout::Stop;

pub(crate) use draw::draw;

/// Lines one wheel notch scrolls the diff: the modal's own.
const WHEEL_LINES: usize = crate::pr_modal::WHEEL_LINES as usize;

/// What the app keeps for the PR REVIEW beside the screen: where its
/// answers land.
#[derive(Debug, Default)]
pub struct State {
    /// None in unit tests, which send nothing.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
}

/// Which part of the screen has the keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    /// The files: `↑`/`↓` pick one, its diff beside them.
    Files,
    /// The diff: `↑`/`↓` walk its lines and threads.
    #[default]
    Diff,
    /// The bar: the verdict and the summary.
    Bar,
}

impl Focus {
    /// `Tab`'s next, or `⇧Tab`'s: files, diff, bar, round.
    fn step(self, forward: bool) -> Self {
        const ORDER: [Focus; 3] = [Focus::Files, Focus::Diff, Focus::Bar];
        let at = ORDER.iter().position(|f| *f == self).unwrap_or(0);
        ORDER[(at + if forward { 1 } else { 2 }) % 3]
    }
}

/// One file of the pull request, its diff read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFile {
    pub path: String,
    /// git's letter: `M`, `A`, `D`, `R` — `·` for a file only a thread
    /// still names, gone from the diff.
    pub status: char,
    pub lines: Option<crate::git_diff::LineChanges>,
    pub doc: DiffDoc,
}

/// What a file the pull request no longer changes says, under the threads
/// still on it.
const GONE: &str = "this file is no longer in the pull request's diff";

/// The PR REVIEW on screen.
#[derive(Debug, Clone)]
pub struct ReviewScreen {
    pub url: String,
    pub number: u64,
    /// `#42 Fix login`, for the title.
    pub label: String,
    /// The checkout `gh` and git run in.
    pub dir: PathBuf,
    /// Matches the answers to the screen that asked.
    pub ticket: u64,
    /// You opened it: GitHub takes only a comment then.
    pub own: bool,
    /// Its node id, once GitHub has answered.
    pub pr_id: Option<String>,
    /// Your pending review's id, once there is one.
    pub pending: Option<String>,
    pub threads: Vec<ReviewThread>,
    /// The threads are being asked for.
    pub reading: bool,
    /// The files and their diffs: None while they are read, or why they
    /// could not be.
    pub files: Option<Result<Arc<Vec<ReviewFile>>, String>>,
    /// The file on screen.
    pub file: usize,
    pub focus: Focus,
    /// Where the cursor rests in the file's diff; None puts it on the
    /// first stop.
    pub cursor: Option<Stop>,
    /// The diff's first row on screen.
    pub scroll: usize,
    /// The cursor moved since the last draw: the draw brings it into view.
    pub follow: bool,
    /// The other end of the run of lines the next comment is on, while
    /// `⇧↑`/`⇧↓` mark one.
    pub anchor: Option<usize>,
    /// The comment being written.
    pub composer: Option<write::Composer>,
    /// What GitHub is being asked to do, while it is.
    pub saving: Option<String>,
    /// What the review says of the pull request, and in a summary.
    pub verdict: github::Verdict,
    pub summary: crate::text_input::TextInput,
    pub bar_row: bar::BarRow,
    /// Why the last submit didn't go, on the bar.
    pub notice: Option<String>,
    /// The PULL REQUESTS MODAL it was opened from, to go back to.
    pub back: Option<Box<PullRequestsView>>,
    // As of the last draw, for the mouse and the page keys.
    pub area: Rect,
    pub files_area: Rect,
    pub diff_area: Rect,
    pub file_rects: Vec<(Rect, usize)>,
    pub bar_area: Rect,
    pub verdict_rects: Vec<(Rect, github::Verdict)>,
    /// Each stop's first row in the diff and how many rows it takes.
    pub stop_rows: Vec<(Stop, usize, usize)>,
    pub diff_h: u16,
}

impl ReviewScreen {
    pub(crate) fn new(url: String, number: u64, label: String, dir: PathBuf, own: bool) -> Self {
        Self {
            url,
            number,
            label,
            dir,
            ticket: crate::view_jobs::ticket(),
            own,
            pr_id: None,
            pending: None,
            threads: Vec::new(),
            reading: true,
            files: None,
            file: 0,
            focus: Focus::Diff,
            cursor: None,
            scroll: 0,
            follow: true,
            anchor: None,
            composer: None,
            saving: None,
            verdict: github::Verdict::Comment,
            summary: crate::text_input::TextInput::multiline(),
            bar_row: bar::BarRow::Verdict,
            notice: None,
            back: None,
            area: Rect::default(),
            files_area: Rect::default(),
            diff_area: Rect::default(),
            file_rects: Vec::new(),
            bar_area: Rect::default(),
            verdict_rects: Vec::new(),
            stop_rows: Vec::new(),
            diff_h: 0,
        }
    }

    /// The files, once read.
    pub fn file_list(&self) -> &[ReviewFile] {
        match &self.files {
            Some(Ok(files)) => files,
            _ => &[],
        }
    }

    /// The file on screen.
    pub fn current(&self) -> Option<&ReviewFile> {
        self.file_list().get(self.file)
    }

    /// The pane's items for file `file`.
    fn items_of(&self, file: &ReviewFile) -> Vec<layout::Item> {
        layout::items(&file.doc, &file.path, &self.threads, self.composer_at(file))
    }

    /// The stops of the file on screen, in order.
    pub fn stops(&self) -> Vec<Stop> {
        self.current().map_or_else(Vec::new, |file| {
            layout::stops(&self.items_of(file), &file.doc, &self.threads)
        })
    }

    /// The threads on `path`.
    pub fn threads_on<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a ReviewThread> + 'a {
        self.threads.iter().filter(move |t| t.path == path)
    }

    /// The pull request's node id, when a write may go now: GitHub has
    /// said which pull request this is, and answered the last write.
    fn writable(&self) -> Option<String> {
        self.pr_id.clone().filter(|_| self.saving.is_none())
    }

    /// Why a write can't go now ([`ReviewScreen::writable`]).
    fn unwritable(&self) -> &'static str {
        if self.saving.is_some() {
            "one moment — GitHub is still answering the last one"
        } else {
            "still reading the pull request from GitHub — try again in a moment"
        }
    }

    /// The thread the cursor rests on.
    pub fn thread_at_cursor(&self) -> Option<&ReviewThread> {
        match &self.cursor {
            Some(Stop::Thread(id)) => self.threads.iter().find(|t| t.id == *id),
            _ => None,
        }
    }

    /// Where the cursor is among `stops`: the first stop when it has none,
    /// or rests on one no longer there.
    fn cursor_index(&self, stops: &[Stop]) -> usize {
        self.cursor
            .as_ref()
            .and_then(|at| stops.iter().position(|s| s == at))
            .unwrap_or(0)
    }

    /// The cursor, or the first stop when it has none — or rests on one no
    /// longer there.
    pub fn cursor_or_first(&self) -> Option<Stop> {
        let stops = self.stops();
        stops.get(self.cursor_index(&stops)).cloned()
    }

    /// Show file `i`, from its top.
    fn pick_file(&mut self, i: usize) {
        let n = self.file_list().len();
        if n == 0 {
            return;
        }
        let i = i.min(n - 1);
        if i != self.file {
            self.file = i;
            self.cursor = None;
            self.anchor = None;
            self.scroll = 0;
            // The last draw's rows were the other file's.
            self.stop_rows.clear();
        }
        self.follow = true;
    }

    /// The cursor `delta` stops along, held to the ends.
    fn step(&mut self, delta: i64) {
        let stops = self.stops();
        if stops.is_empty() {
            return;
        }
        let to = crate::app::clamp_selection(self.cursor_index(&stops) as i64 + delta, stops.len());
        self.cursor = Some(stops[to].clone());
        self.follow = true;
    }

    /// The cursor a page along: onto the stop a pane's height further, as
    /// the last draw laid them out.
    fn page(&mut self, down: bool) {
        let height = usize::from(self.diff_h.max(1));
        let Some(cursor) = self.cursor_or_first() else {
            return;
        };
        let Some(from) = self
            .stop_rows
            .iter()
            .find(|(s, ..)| *s == cursor)
            .map(|(_, row, _)| *row)
        else {
            return self.step(if down { 1 } else { -1 });
        };
        let found = if down {
            self.stop_rows
                .iter()
                .rev()
                .find(|(_, row, _)| *row <= from + height)
        } else {
            self.stop_rows
                .iter()
                .find(|(_, row, _)| *row + height >= from)
        };
        match found {
            Some((stop, ..)) if *stop != cursor => {
                self.cursor = Some(stop.clone());
                self.follow = true;
            }
            _ => self.step(if down { 1 } else { -1 }),
        }
    }

    /// The cursor to the first stop, or the last.
    fn jump_to_end(&mut self, last: bool) {
        let stops = self.stops();
        let at = if last { stops.last() } else { stops.first() };
        self.cursor = at.cloned();
        self.follow = true;
    }

    /// The next thread after the cursor — `]` — or the one before it
    /// (`[`), from file to file, round either end. Whether there was one.
    fn jump_thread(&mut self, forward: bool) -> bool {
        // Every thread in reading order: (file, place in its pane, id) —
        // and where the cursor is in the file on screen.
        let mut order: Vec<(usize, usize, String)> = Vec::new();
        let mut here = 0;
        for (f, file) in self.file_list().iter().enumerate() {
            if f != self.file && self.threads_on(&file.path).next().is_none() {
                continue;
            }
            let items = self.items_of(file);
            for (at, item) in items.iter().enumerate() {
                if let layout::Item::Thread(t) = item {
                    order.push((f, at, self.threads[*t].id.clone()));
                }
            }
            if f == self.file {
                let stops: Vec<(usize, Stop)> = items
                    .iter()
                    .enumerate()
                    .filter_map(|(at, item)| {
                        Some((at, layout::stop_of(*item, &file.doc, &self.threads)?))
                    })
                    .collect();
                let walked: Vec<Stop> = stops.iter().map(|(_, s)| s.clone()).collect();
                here = stops
                    .get(self.cursor_index(&walked))
                    .map_or(0, |(at, _)| *at);
            }
        }
        if order.is_empty() {
            return false;
        }
        let now = (self.file, here);
        let next = if forward {
            order
                .iter()
                .find(|(f, at, _)| (*f, *at) > now)
                .or(order.first())
        } else {
            order
                .iter()
                .rev()
                .find(|(f, at, _)| (*f, *at) < now)
                .or(order.last())
        };
        let Some((f, _, id)) = next.cloned() else {
            return false;
        };
        self.pick_file(f);
        self.cursor = Some(Stop::Thread(id));
        self.follow = true;
        true
    }

    /// GitHub's answer about the pull request, taken in.
    fn take_threads(&mut self, read: Threads) {
        self.pr_id = Some(read.pr_id);
        self.own = read.mine;
        self.hold_to_allowed();
        if let Some(pending) = &read.pending {
            // What was written into it on github.com, to carry on from.
            if self.summary.trim().is_empty() && !pending.body.trim().is_empty() {
                self.summary = crate::text_input::TextInput::multiline_with_text(&pending.body);
            }
        }
        self.pending = read.pending.map(|p| p.id);
        self.threads = read.threads;
        self.reading = false;
        self.add_gone_files();
    }

    /// The diff read, taken in.
    fn take_files(&mut self, files: Result<Vec<ReviewFile>, String>) {
        self.files = Some(files.map(Arc::new));
        self.add_gone_files();
        self.follow = true;
    }

    /// A file a thread is on that the diff no longer changes gets a row of
    /// its own, its threads at the top of a diff that says so.
    fn add_gone_files(&mut self) {
        let Some(Ok(files)) = &self.files else {
            return;
        };
        let mut gone: Vec<&str> = self
            .threads
            .iter()
            .map(|t| t.path.as_str())
            .filter(|path| !files.iter().any(|f| f.path == *path))
            .collect();
        gone.sort_unstable();
        gone.dedup();
        if gone.is_empty() {
            return;
        }
        let mut all = files.as_ref().clone();
        all.extend(gone.into_iter().map(|path| ReviewFile {
            path: path.to_string(),
            status: '·',
            lines: None,
            doc: DiffDoc::build(&[], None, GONE),
        }));
        self.files = Some(Ok(Arc::new(all)));
    }
}

// ---- answers ----

/// What the screen's reads and writes came back with.
#[derive(Debug)]
pub enum Answer {
    /// The pull request's threads, its pending review and its node id.
    Threads {
        ticket: u64,
        url: String,
        result: Result<Threads, String>,
    },
    /// Its files and their diffs.
    Diff {
        ticket: u64,
        result: Result<Vec<ReviewFile>, String>,
    },
    /// A write on pull request `number` — `what` it was trying to do —
    /// and what it made.
    Wrote {
        ticket: u64,
        number: u64,
        what: &'static str,
        result: Result<write::Wrote, String>,
    },
    /// The review on pull request `number` sent with `verdict`, or why not.
    Submitted {
        ticket: u64,
        number: u64,
        url: String,
        verdict: github::Verdict,
        result: Result<(), String>,
    },
}

/// The screen on display, when it is the one `ticket` was asked for.
fn screen_for(app: &mut App, ticket: u64) -> Option<&mut ReviewScreen> {
    match &mut app.overlay {
        Some(Overlay::Review(view)) if view.ticket == ticket => Some(view),
        _ => None,
    }
}

/// The threads as they stand now, onto pull request `url`'s page too: its
/// Reviews tab reads the same ones.
fn share_threads(app: &mut App, url: &str, threads: Vec<ReviewThread>) {
    if let Some(detail) = app.pr_detail.get_mut(url) {
        detail.threads = threads;
    }
}

/// Land one answer.
pub(crate) fn land(app: &mut App, answer: Answer) {
    match answer {
        Answer::Threads {
            ticket,
            url,
            result,
        } => match result {
            Ok(read) => {
                share_threads(app, &url, read.threads.clone());
                if let Some(view) = screen_for(app, ticket) {
                    view.take_threads(read);
                }
            }
            Err(why) => {
                if let Some(view) = screen_for(app, ticket) {
                    view.reading = false;
                    let number = view.number;
                    app.flash = Some(Flash::failed(format!(
                        "couldn't read #{number}'s review threads: {why}"
                    )));
                }
            }
        },
        Answer::Diff { ticket, result } => {
            if let Some(view) = screen_for(app, ticket) {
                view.take_files(result);
            }
        }
        Answer::Wrote {
            ticket,
            number,
            what,
            result,
        } => write::land(app, ticket, number, what, result),
        Answer::Submitted {
            ticket,
            number,
            url,
            verdict,
            result,
        } => bar::land_submitted(app, ticket, number, url, verdict, result),
    }
    app.dirty = true;
}

// ---- opening and leaving ----

/// `⌘⇧R` in the PULL REQUESTS MODAL, and Enter on its page's Reviews tab:
/// the pull request under the cursor reviewed, a level inside the modal.
pub(crate) fn open_from_modal(app: &mut App) {
    let Some(Overlay::PullRequests(modal)) = &app.overlay else {
        return;
    };
    let modal = modal.clone();
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
        return;
    };
    let mut view = ReviewScreen::new(
        pr.url.clone(),
        pr.number,
        pr.label(),
        modal.dir.clone(),
        pr.mine,
    );
    view.back = Some(Box::new(modal));
    open(app, view);
}

/// Enter on the pane's PULL REQUEST PAGE, on its Reviews tab: the pull
/// request it reads, reviewed over the panels.
pub(crate) fn open_from_pane(app: &mut App, pr: &crate::app::PreviewedPr) {
    let Some(dir) = app.selected_project().map(|p| p.repo_path.clone()) else {
        return;
    };
    let view = ReviewScreen::new(pr.url.clone(), pr.number, pr.label.clone(), dir, false);
    open(app, view);
}

/// Put `view` up, its threads and its diff asked for. The pull request's
/// page must be in — its head commit and base are what the diff is read
/// between — and the threads it read open the screen while GitHub is
/// asked again.
fn open(app: &mut App, mut view: ReviewScreen) {
    let Some((base, tip, threads)) = app
        .pr_detail
        .get(&view.url)
        .filter(|d| !d.head_sha.is_empty() && !d.base.is_empty())
        .map(|d| (d.base.clone(), d.head_sha.clone(), d.threads.clone()))
    else {
        app.flash = Some(Flash::note(format!(
            "#{} is still loading — try again in a moment",
            view.number
        )));
        return;
    };
    view.threads = threads;
    ask_threads(app, &view);
    read_diff(app, &view, base, tip);
    app.flash = None;
    app.overlay = Some(Overlay::Review(Box::new(view)));
    app.dirty = true;
}

/// Ask GitHub for `view`'s threads, off the loop.
fn ask_threads(app: &App, view: &ReviewScreen) {
    let Some(tx) = app.pr_review.tx.clone() else {
        return;
    };
    let (dir, number, ticket, url) = (view.dir.clone(), view.number, view.ticket, view.url.clone());
    tokio::spawn(async move {
        let result = github::read(&dir, number, github::TIMEOUT).await;
        let _ = tx.send(Answer::Threads {
            ticket,
            url,
            result,
        });
    });
}

/// Read `view`'s diff from git — `base` to `tip`, the head fetched by its
/// pull ref when the repo lacks it — on the blocking pool.
fn read_diff(app: &App, view: &ReviewScreen, base: String, tip: String) {
    let Some(tx) = app.pr_review.tx.clone() else {
        return;
    };
    let (dir, number, ticket) = (view.dir.clone(), view.number, view.ticket);
    let quit = Arc::clone(&app.git_sync.quit);
    tokio::task::spawn_blocking(move || {
        let result = read_files(&dir, number, &base, &tip, &quit);
        let _ = tx.send(Answer::Diff { ticket, result });
    });
}

/// Every file pull request `number` changes, `base` to `tip`, its diff
/// read and laid out: the diff GitHub reviews, from where the branch left
/// its base.
pub(crate) fn read_files(
    dir: &std::path::Path,
    number: u64,
    base: &str,
    tip: &str,
    quit: &std::sync::atomic::AtomicBool,
) -> Result<Vec<ReviewFile>, String> {
    let listing = crate::event_loop::read_pr_opening(dir, number, base, tip, quit)?;
    let merge_base = listing
        .commits
        .and_then(|c| c.merge_base)
        .ok_or_else(|| format!("no history in common with {base}"))?;
    let scope = crate::git_diff::DiffScope::Commit {
        sha: tip.to_string(),
        parent: Some(merge_base),
    };
    let files = crate::git_diff::scope_files(dir, &scope)?;
    let mut read = Vec::with_capacity(files.len());
    for file in files {
        // orion is quitting: no more git for a screen nobody will see.
        if quit.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("stopped".into());
        }
        let text = crate::git_diff::scoped_diff(dir, &scope, &file, true);
        read.push(ReviewFile {
            doc: DiffDoc::build(&[], Some(&file.path), &text),
            status: file.xy[0],
            lines: file.lines,
            path: file.path,
        });
    }
    Ok(read)
}

/// Esc: back to the PULL REQUESTS MODAL it was opened from, on the same
/// row and tab — or, opened from the pane, back to the panels. Whatever is
/// pending stays pending: it is GitHub's, there for the next visit.
pub(crate) fn back(app: &mut App) {
    let Some(Overlay::Review(view)) = app.overlay.take() else {
        return;
    };
    match view.back {
        Some(modal) => crate::pr_modal::reopen(app, *modal),
        None => app.dirty = true,
    }
}

/// `⌘R`: the threads asked for again — someone may have said something.
fn refresh(app: &mut App) {
    let Some(Overlay::Review(view)) = &mut app.overlay else {
        return;
    };
    view.reading = true;
    if let Some(Overlay::Review(view)) = &app.overlay {
        ask_threads(app, view);
    }
}

// ---- keys ----

/// The screen's own keys: one table [`handle_key`] matches and [`hints`]
/// spells.
pub(crate) mod keys {
    use crate::hints::Key;

    /// The files and the diff, to and fro.
    pub const PANEL: Key = Key::new(&["tab", "shift+tab"], "diff");
    pub const MOVE: Key = Key::new(&["up", "down"], "move").show(2);
    pub const PAGE: Key = Key::new(&["pgup", "pgdn"], "page").show(2);
    /// The files: read the one under the cursor.
    pub const OPEN: Key = Key::new(&["enter", "right"], "diff");
    /// The diff: back to the files.
    pub const FILES: Key = Key::new(&["left"], "files");
    /// The next thread, file to file — and the one before.
    pub const NEXT_THREAD: Key = Key::new(&["]"], "next thread");
    pub const PREV_THREAD: Key = Key::new(&["["], "previous thread");
    pub const THREADS: Key = Key::new(&["]", "["], "thread").show(2);
    /// A comment on the line under the cursor, or the lines marked.
    pub const COMMENT: Key = Key::new(&["c"], "comment");
    /// The run of lines the next comment is on, a line longer or shorter.
    pub const RANGE: Key = Key::new(&["shift+up", "shift+down"], "lines").show(2);
    pub const REPLY: Key = Key::new(&["r"], "reply");
    pub const RESOLVE: Key = Key::new(&["x"], "resolve");
    /// Your pending comment in the thread under the cursor, dropped.
    pub const DROP: Key = Key::new(&["backspace"], "drop yours");
    /// The comment being written, into the pending review.
    pub const ADD: Key = Key::new(&["enter"], "add to review");
    pub const NEWLINE: Key = crate::ui::task_keys::NEWLINE;
    /// The bar: the verdict.
    pub const VERDICT: Key = Key::new(&["left", "right"], "verdict").show(2);
    /// The review sent, with everything pending: the modal's review chord,
    /// from anywhere — and Enter on the bar.
    pub const SUBMIT: Key = Key::new(&["cmd+shift+r", "ctrl+shift+r"], "submit");
    pub const SEND: Key = Key::new(&["enter"], "submit");
    pub const REFRESH: Key = crate::hints::REFRESH;
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        PANEL,
        MOVE,
        PAGE,
        OPEN,
        FILES,
        NEXT_THREAD,
        PREV_THREAD,
        THREADS,
        COMMENT,
        RANGE,
        REPLY,
        RESOLVE,
        DROP,
        ADD,
        NEWLINE,
        VERDICT,
        SUBMIT,
        SEND,
        REFRESH,
    ];
}

/// The keys along the screen's bottom edge, for the part that has them.
pub(crate) fn hints(view: &ReviewScreen) -> Vec<crate::hints::Hint> {
    use keys::*;
    let mut hints = Vec::new();
    if view.composer.is_some() {
        hints.push(ADD.hint().kept());
        hints.push(NEWLINE.hint());
        hints.push(crate::hints::ESC_CLOSE.hint_as("cancel"));
        return hints;
    }
    match view.focus {
        Focus::Files => {
            hints.push(MOVE.hint_as("file").kept());
            hints.push(OPEN.hint());
            hints.push(SUBMIT.hint());
            hints.push(THREADS.hint());
            hints.push(PANEL.hint_as("diff"));
        }
        Focus::Bar => {
            hints.push(SEND.hint().kept());
            if view.bar_row == bar::BarRow::Verdict {
                hints.push(VERDICT.hint());
            } else {
                hints.push(NEWLINE.hint());
            }
            hints.push(PANEL.hint_as("files"));
            hints.push(crate::hints::ESC_CLOSE.hint_as("diff"));
            return hints;
        }
        Focus::Diff => {
            match view.thread_at_cursor() {
                Some(thread) => {
                    hints.push(REPLY.hint().kept());
                    hints.push(RESOLVE.hint_as(if thread.resolved { "reopen" } else { "resolve" }));
                    if thread.my_pending().is_some() {
                        hints.push(DROP.hint());
                    }
                }
                None => {
                    let lines = view.marked_lines().map_or(1, |(a, b)| b - a + 1);
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
            hints.push(FILES.hint());
            hints.push(PANEL.hint_as("review"));
        }
    }
    hints.push(REFRESH.hint());
    hints.push(match view.back {
        _ if view.anchor.is_some() => crate::hints::ESC_CLOSE.hint_as("clear the lines"),
        Some(_) => crate::hints::ESC_CLOSE.hint_as("back to the pull request"),
        None => crate::hints::ESC_CLOSE.hint(),
    });
    hints
}

/// A paste on the screen: into the comment being written.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    app.dirty = true;
    match &mut app.overlay {
        Some(Overlay::Review(view)) => write::paste(view, text) || bar::paste(view, text),
        _ => false,
    }
}

/// A key on the screen.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    use keys::*;
    app.dirty = true;
    if matches!(&app.overlay, Some(Overlay::Review(v)) if v.composer.is_some()) {
        return write::composer_key(app, key);
    }
    if REFRESH.matches(&key) {
        return refresh(app);
    }
    if SUBMIT.matches(&key) {
        return bar::submit(app);
    }
    let Some(Overlay::Review(view)) = &mut app.overlay else {
        return;
    };
    if PANEL.matches(&key) {
        view.focus = view
            .focus
            .step(key.code == KeyCode::Tab && !key.modifiers.contains(KeyModifiers::SHIFT));
        return;
    }
    if view.focus == Focus::Bar {
        return bar::bar_key(app, key);
    }
    let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
    let on_diff = view.focus == Focus::Diff;
    match key.code {
        KeyCode::Esc if view.anchor.is_some() => view.anchor = None,
        KeyCode::Esc => back(app),
        _ if NEXT_THREAD.matches(&key) => {
            view.jump_thread(true);
        }
        _ if PREV_THREAD.matches(&key) => {
            view.jump_thread(false);
        }
        _ if on_diff && RANGE.matches(&key) => view.extend_range(key.code == KeyCode::Down),
        _ if on_diff && COMMENT.matches(&key) => write::start_comment(app),
        _ if on_diff && REPLY.matches(&key) => write::start_reply(app),
        _ if on_diff && RESOLVE.matches(&key) => write::toggle_resolved(app),
        _ if on_diff && DROP.matches(&key) => write::drop_pending(app),
        _ if !plain => {}
        _ if view.focus == Focus::Files => match key.code {
            KeyCode::Up => view.pick_file(view.file.saturating_sub(1)),
            KeyCode::Down => view.pick_file(view.file + 1),
            KeyCode::Home => view.pick_file(0),
            KeyCode::End => view.pick_file(usize::MAX),
            _ if OPEN.matches(&key) => view.focus = Focus::Diff,
            _ => {}
        },
        KeyCode::Up | KeyCode::Down => {
            view.anchor = None;
            view.step(if key.code == KeyCode::Up { -1 } else { 1 });
        }
        KeyCode::PageUp => view.page(false),
        KeyCode::PageDown => view.page(true),
        KeyCode::Home => view.jump_to_end(false),
        KeyCode::End => view.jump_to_end(true),
        _ if FILES.matches(&key) => view.focus = Focus::Files,
        _ => {}
    }
}

/// The mouse on the screen: a click on a file reads it, a click in the
/// diff puts the cursor on what it hit, and the wheel scrolls the diff.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let pos = Position::new(mouse.column, mouse.row);
    app.dirty = true;
    let Some(Overlay::Review(view)) = &mut app.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollDown if view.diff_area.contains(pos) => {
            view.scroll += WHEEL_LINES;
            view.follow = false;
        }
        MouseEventKind::ScrollUp if view.diff_area.contains(pos) => {
            view.scroll = view.scroll.saturating_sub(WHEEL_LINES);
            view.follow = false;
        }
        MouseEventKind::ScrollDown if view.files_area.contains(pos) => {
            view.pick_file(view.file + 1);
        }
        MouseEventKind::ScrollUp if view.files_area.contains(pos) => {
            view.pick_file(view.file.saturating_sub(1));
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(&(_, verdict)) = view.verdict_rects.iter().find(|(r, _)| r.contains(pos)) {
                view.focus = Focus::Bar;
                view.bar_row = bar::BarRow::Verdict;
                view.verdict = verdict;
                view.notice = None;
            } else if view.bar_area.contains(pos) {
                view.focus = Focus::Bar;
            } else if let Some(&(_, i)) = view.file_rects.iter().find(|(r, _)| r.contains(pos)) {
                view.focus = Focus::Files;
                view.pick_file(i);
            } else if view.diff_area.contains(pos) {
                let row = view.scroll + usize::from(pos.y.saturating_sub(view.diff_area.y));
                let hit = view
                    .stop_rows
                    .iter()
                    .find(|(_, first, n)| (*first..first + n).contains(&row));
                view.focus = Focus::Diff;
                if let Some((stop, ..)) = hit {
                    view.cursor = Some(stop.clone());
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
