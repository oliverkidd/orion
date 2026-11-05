//! What the PR REVIEW writes into GitHub's pending review: a comment on a
//! line or a run of lines (`c`, written in place under the line), a reply
//! to a thread (`r`), a thread resolved or opened again (`x`) and a
//! pending comment of yours dropped (`⌫`). Comments and replies are held
//! pending — nobody else sees them until the review is submitted — and
//! leaving the screen keeps them: the pending review is GitHub's. Each
//! write is one task off the loop, one at a time; what it made lands on
//! the screen as GitHub answered it, and a write that failed leaves what
//! was typed where it was, the footer saying why.

use crossterm::event::{KeyCode, KeyEvent};

use super::github::{self, NewThread, ReviewThread, ThreadComment};
use super::layout::{self, ComposerAt, Stop};
use super::{keys, Answer, ReviewFile, ReviewScreen};
use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::text_input::TextInput;

/// The comment being written, in place in the diff.
#[derive(Debug, Clone)]
pub struct Composer {
    pub target: Target,
    pub input: TextInput,
}

/// What the comment being written is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A new thread on lines `start..=end` of the diff of `path`.
    Lines {
        path: String,
        start: usize,
        end: usize,
    },
    /// A reply to the thread with this id.
    Reply(String),
}

/// One write, as it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Comment(NewThread),
    Reply { thread: String, body: String },
    Resolve { thread: String, resolve: bool },
    Drop { thread: String, comment: String },
}

impl Write {
    /// What the footer says while it is on its way.
    fn saying(&self) -> &'static str {
        match self {
            Write::Comment(_) => "adding your comment…",
            Write::Reply { .. } => "adding your reply…",
            Write::Resolve { resolve: true, .. } => "resolving the thread…",
            Write::Resolve { resolve: false, .. } => "opening the thread again…",
            Write::Drop { .. } => "dropping your comment…",
        }
    }

    /// What it was trying to do, for the footer when it couldn't.
    fn what(&self) -> &'static str {
        match self {
            Write::Comment(_) => "add the comment",
            Write::Reply { .. } => "add the reply",
            Write::Resolve { resolve: true, .. } => "resolve the thread",
            Write::Resolve { resolve: false, .. } => "open the thread again",
            Write::Drop { .. } => "drop the comment",
        }
    }
}

/// What a write made, as GitHub answered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wrote {
    /// A new thread, in pending review `review`.
    Comment {
        review: String,
        thread: ReviewThread,
    },
    /// A reply on `thread`, in pending review `review`.
    Reply {
        review: String,
        thread: String,
        comment: ThreadComment,
    },
    Resolved {
        thread: String,
        resolved: bool,
    },
    Dropped {
        thread: String,
        comment: String,
    },
}

impl Wrote {
    /// A comment or a reply: held in the pending review, nobody else
    /// seeing it yet.
    fn held(&self) -> bool {
        matches!(self, Wrote::Comment { .. } | Wrote::Reply { .. })
    }

    /// What the footer says it did, once the screen is gone.
    fn done(&self) -> &'static str {
        match self {
            Wrote::Comment { .. } | Wrote::Reply { .. } => "held in your pending review",
            Wrote::Resolved { resolved: true, .. } => "thread resolved",
            Wrote::Resolved {
                resolved: false, ..
            } => "thread opened again",
            Wrote::Dropped { .. } => "pending comment dropped",
        }
    }
}

impl ReviewScreen {
    /// Where the comment being written goes in `file`'s pane, when it is
    /// in that file.
    pub(super) fn composer_at(&self, file: &ReviewFile) -> Option<ComposerAt> {
        match &self.composer.as_ref()?.target {
            Target::Lines { path, end, .. } => {
                (*path == file.path).then_some(ComposerAt::Line(*end))
            }
            Target::Reply(id) => self
                .threads
                .iter()
                .position(|t| t.id == *id && t.path == file.path)
                .map(ComposerAt::Thread),
        }
    }

    /// The lines a comment is on or about to be: the composer's, or the
    /// run from the anchor to the cursor — the rows the draw marks.
    pub(super) fn marked_lines(&self) -> Option<(usize, usize)> {
        if let Some(Composer {
            target: Target::Lines { path, start, end },
            ..
        }) = &self.composer
        {
            return self
                .current()
                .is_some_and(|f| f.path == *path)
                .then_some((*start, *end));
        }
        let anchor = self.anchor?;
        match self.cursor {
            Some(Stop::Line(at)) => Some((anchor.min(at), anchor.max(at))),
            _ => None,
        }
    }

    /// `⇧↑`/`⇧↓`: the run of lines the next comment is on, one line longer
    /// or shorter at the cursor's end — never past its hunk.
    pub(super) fn extend_range(&mut self, down: bool) {
        let Some(Stop::Line(at)) = self.cursor_or_first() else {
            return;
        };
        let Some(file) = self.current() else {
            return;
        };
        let anchor = self.anchor.unwrap_or(at);
        let to = if down { at + 1 } else { at.saturating_sub(1) };
        if to == at || !layout::same_hunk(&file.doc, anchor, to) {
            return;
        }
        self.anchor = Some(anchor);
        self.cursor = Some(Stop::Line(to));
        self.follow = true;
    }

    fn thread_mut(&mut self, id: &str) -> Option<&mut ReviewThread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    /// What a write made, put on the screen.
    fn take(&mut self, wrote: Wrote) {
        match wrote {
            Wrote::Comment { review, thread } => {
                self.pending = Some(review);
                self.cursor = Some(Stop::Thread(thread.id.clone()));
                self.threads.push(thread);
                self.composer = None;
                self.anchor = None;
            }
            Wrote::Reply {
                review,
                thread,
                comment,
            } => {
                self.pending = Some(review);
                if let Some(t) = self.thread_mut(&thread) {
                    t.comments.push(comment);
                }
                self.composer = None;
            }
            Wrote::Resolved { thread, resolved } => {
                if let Some(t) = self.thread_mut(&thread) {
                    t.resolved = resolved;
                }
            }
            Wrote::Dropped { thread, comment } => {
                if let Some(t) = self.thread_mut(&thread) {
                    t.comments.retain(|c| c.id != comment);
                }
                // A thread with nothing left in it is gone from GitHub too.
                self.threads.retain(|t| !t.comments.is_empty());
            }
        }
        self.follow = true;
    }
}

/// The screen, while it is what is up.
fn screen(app: &mut App) -> Option<&mut ReviewScreen> {
    match &mut app.overlay {
        Some(Overlay::Review(view)) => Some(view),
        _ => None,
    }
}

/// `c`: a comment on the line under the cursor — or on the run of lines
/// `⇧↑`/`⇧↓` marked — written in place under it. On a thread, a reply.
pub(super) fn start_comment(app: &mut App) {
    let Some(view) = screen(app) else {
        return;
    };
    let Some(path) = view.current().map(|f| f.path.clone()) else {
        return;
    };
    let target = match view.cursor_or_first() {
        Some(Stop::Line(at)) => {
            let (start, end) = view.marked_lines().unwrap_or((at, at));
            Target::Lines { path, start, end }
        }
        Some(Stop::Thread(id)) => Target::Reply(id),
        None => return,
    };
    open_composer(view, target);
}

/// `r`: a reply at the foot of the thread under the cursor.
pub(super) fn start_reply(app: &mut App) {
    let Some(view) = screen(app) else {
        return;
    };
    if let Some(Stop::Thread(id)) = view.cursor_or_first() {
        open_composer(view, Target::Reply(id));
    }
}

fn open_composer(view: &mut ReviewScreen, target: Target) {
    view.composer = Some(Composer {
        target,
        input: TextInput::multiline(),
    });
    view.follow = true;
}

/// `x`: the thread under the cursor resolved — or, resolved, opened again.
pub(super) fn toggle_resolved(app: &mut App) {
    let Some(thread) = screen(app).and_then(|v| v.thread_at_cursor().cloned()) else {
        return;
    };
    send(
        app,
        Write::Resolve {
            thread: thread.id,
            resolve: !thread.resolved,
        },
    );
}

/// `⌫`: your last pending comment in the thread under the cursor dropped
/// from the review — the thread with it, when it was all there was.
pub(super) fn drop_pending(app: &mut App) {
    let Some(thread) = screen(app).and_then(|v| v.thread_at_cursor().cloned()) else {
        return;
    };
    let Some(comment) = thread.my_pending() else {
        app.flash = Some(Flash::note(
            "⌫ drops a pending comment of yours — this thread has none",
        ));
        return;
    };
    let comment = comment.id.clone();
    send(
        app,
        Write::Drop {
            thread: thread.id,
            comment,
        },
    );
}

/// A key while a comment is being written: Enter adds it to the review,
/// Esc puts it away, everything else is the field's.
pub(super) fn composer_key(app: &mut App, key: KeyEvent) {
    let Some(view) = screen(app) else {
        return;
    };
    let Some(composer) = &mut view.composer else {
        return;
    };
    match key.code {
        KeyCode::Esc => view.composer = None,
        _ if keys::ADD.matches(&key) && !composer.input.takes_newline(&key) => send_composer(app),
        _ => {
            composer.input.handle_key(&key);
            view.follow = true;
        }
    }
}

/// A paste into the comment being written.
pub(super) fn paste(view: &mut ReviewScreen, text: &str) -> bool {
    let Some(composer) = &mut view.composer else {
        return false;
    };
    composer.input.insert_str(text);
    view.follow = true;
    true
}

/// Enter in the composer: what it holds sent, as a new thread or a reply.
fn send_composer(app: &mut App) {
    let Some(view) = screen(app) else {
        return;
    };
    let Some(composer) = &view.composer else {
        return;
    };
    let body = composer.input.trim().to_string();
    if body.is_empty() {
        app.flash = Some(Flash::note("write something first — Esc puts it away"));
        return;
    }
    let write = match &composer.target {
        Target::Reply(thread) => Write::Reply {
            thread: thread.clone(),
            body,
        },
        Target::Lines { path, start, end } => {
            let Some(doc) = view
                .file_list()
                .iter()
                .find(|f| f.path == *path)
                .map(|f| &f.doc)
            else {
                return;
            };
            let (Some((side, line)), Some(first)) =
                (layout::side_line(doc, *end), layout::side_line(doc, *start))
            else {
                return;
            };
            Write::Comment(NewThread {
                path: path.clone(),
                side,
                line,
                start: (start != end).then_some(first),
                body,
            })
        }
    };
    send(app, write);
}

/// Send `write`, off the loop — one at a time, and only once GitHub has
/// said which pull request this is.
fn send(app: &mut App, write: Write) {
    let tx = app.pr_review.tx.clone();
    let Some(view) = screen(app) else {
        return;
    };
    if view.saving.is_some() {
        app.flash = Some(Flash::note(
            "one moment — GitHub is still answering the last one",
        ));
        return;
    }
    let Some(pr_id) = view.pr_id.clone() else {
        app.flash = Some(Flash::note(
            "still reading the pull request from GitHub — try again in a moment",
        ));
        return;
    };
    #[cfg(test)]
    tests::SENT.with_borrow_mut(|sent| sent.push(write.clone()));
    view.saving = Some(write.saying().to_string());
    let (dir, pending, ticket, number) = (
        view.dir.clone(),
        view.pending.clone(),
        view.ticket,
        view.number,
    );
    let Some(tx) = tx else {
        return;
    };
    tokio::spawn(async move {
        let what = write.what();
        let result = run(&dir, &pr_id, pending, write).await;
        let _ = tx.send(Answer::Wrote {
            ticket,
            number,
            what,
            result,
        });
    });
}

/// The GitHub calls behind `write`.
async fn run(
    dir: &std::path::Path,
    pr_id: &str,
    pending: Option<String>,
    write: Write,
) -> Result<Wrote, String> {
    match write {
        Write::Comment(new) => github::comment(dir, pr_id, pending, &new)
            .await
            .map(|(review, thread)| Wrote::Comment { review, thread }),
        Write::Reply { thread, body } => github::reply(dir, pr_id, pending, &thread, &body)
            .await
            .map(|(review, comment)| Wrote::Reply {
                review,
                thread,
                comment,
            }),
        Write::Resolve { thread, resolve } => github::resolve(dir, &thread, resolve)
            .await
            .map(|resolved| Wrote::Resolved { thread, resolved }),
        Write::Drop { thread, comment } => github::delete(dir, &comment)
            .await
            .map(|()| Wrote::Dropped { thread, comment }),
    }
}

/// A write landed. On the screen that sent it, what it made goes up — or,
/// refused, what was typed stays, the footer saying why. With the screen
/// gone it still happened: the footer says so.
pub(super) fn land(
    app: &mut App,
    ticket: u64,
    number: u64,
    what: &str,
    result: Result<Wrote, String>,
) {
    match (super::screen_for(app, ticket), result) {
        (Some(view), Ok(wrote)) => {
            view.saving = None;
            let held = wrote.held();
            view.take(wrote);
            let (url, threads) = (view.url.clone(), view.threads.clone());
            super::share_threads(app, &url, threads);
            if held {
                app.flash = Some(Flash::done("held in your pending review"));
            }
        }
        (Some(view), Err(why)) => {
            view.saving = None;
            app.flash = Some(Flash::failed(format!("couldn't {what}: {why}")));
        }
        (None, Ok(wrote)) => {
            app.flash = Some(Flash::done(format!("#{number}: {}", wrote.done())));
        }
        (None, Err(why)) => {
            app.flash = Some(Flash::failed(format!("#{number}: couldn't {what}: {why}")));
        }
    }
    app.dirty = true;
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::RefCell;

    thread_local! {
        /// Every write [`super::send`] sent off, under test.
        pub(in crate::pr_review) static SENT: RefCell<Vec<super::Write>> = const { RefCell::new(Vec::new()) };
    }

    /// The writes sent since the last call.
    pub(in crate::pr_review) fn take_sent() -> Vec<super::Write> {
        SENT.with_borrow_mut(std::mem::take)
    }
}
