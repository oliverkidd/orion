//! The PR REVIEW's bottom bar: the verdict — Comment, Approve or Request
//! changes, only a comment on a pull request you opened yourself, as
//! GitHub has it — and an optional summary; and the submit that sends the
//! review with everything pending in it (`⌘⇧R` anywhere, Enter on the
//! bar). With nothing pending, the review is sent at once as a new one.
//! Once GitHub takes it the screen gives way to the pull request's
//! Reviews tab, its page read again.

use crossterm::event::{KeyCode, KeyEvent};

use super::github::{self, Verdict};
use super::{Answer, Focus, ReviewScreen};
use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::pr_preview::PrTab;

/// Which row of the bar has the keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BarRow {
    #[default]
    Verdict,
    Summary,
}

impl ReviewScreen {
    /// How many of your comments the review holds, waiting to be sent.
    pub fn pending_comments(&self) -> usize {
        self.threads.iter().map(|t| t.pending_count()).sum()
    }

    /// The verdicts GitHub takes on this pull request, in the bar's order.
    pub fn verdicts(&self) -> Vec<Verdict> {
        Verdict::ALL
            .into_iter()
            .filter(|v| v.allowed(self.own))
            .collect()
    }

    /// The verdict a step along the ones allowed, round either end.
    fn step_verdict(&mut self, forward: bool) {
        let allowed = self.verdicts();
        let at = allowed.iter().position(|v| *v == self.verdict).unwrap_or(0);
        let n = allowed.len();
        let to = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        self.verdict = allowed[to];
        self.notice = None;
    }

    /// GitHub says whose pull request it is: on your own only a comment is
    /// taken.
    pub(super) fn hold_to_allowed(&mut self) {
        if !self.verdict.allowed(self.own) {
            self.verdict = Verdict::Comment;
        }
    }
}

/// A key with the bar holding them: `←`/`→` the verdict and `↓` the
/// summary on its first row, the summary's own keys on the second — `↑`
/// off its top back up — and Enter sends.
pub(super) fn bar_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Review(view)) = &mut app.overlay else {
        return;
    };
    let newline = view.summary.takes_newline(&key);
    match (view.bar_row, key.code) {
        (_, KeyCode::Esc) => view.focus = Focus::Diff,
        (_, KeyCode::Enter) if !newline => submit(app),
        (BarRow::Verdict, KeyCode::Left | KeyCode::Right) => {
            view.step_verdict(key.code == KeyCode::Right);
        }
        (BarRow::Verdict, KeyCode::Char(' ')) => view.step_verdict(true),
        (BarRow::Verdict, KeyCode::Down) => view.bar_row = BarRow::Summary,
        (BarRow::Verdict, _) => {
            // A letter typed on the verdict is the summary's.
            if view.summary.handle_key(&key).changed() {
                view.bar_row = BarRow::Summary;
                view.notice = None;
            }
        }
        (BarRow::Summary, _) => {
            let edit = view.summary.handle_key(&key);
            if !edit.consumed() && key.code == KeyCode::Up {
                view.bar_row = BarRow::Verdict;
            }
            if edit.changed() {
                view.notice = None;
            }
        }
    }
}

/// A paste into the summary.
pub(super) fn paste(view: &mut ReviewScreen, text: &str) -> bool {
    if view.focus != Focus::Bar {
        return false;
    }
    view.bar_row = BarRow::Summary;
    view.summary.insert_str(text);
    true
}

/// `⌘⇧R`, and Enter on the bar: the review sent — the verdict, the
/// summary and everything pending. A comment still being written is
/// finished first, and a comment or a change request with nothing pending
/// needs a word to say.
pub(super) fn submit(app: &mut App) {
    let tx = app.pr_review.tx.clone();
    let Some(Overlay::Review(view)) = &mut app.overlay else {
        return;
    };
    if view.composer.is_some() {
        app.flash = Some(Flash::note(
            "finish the comment first — Enter adds it, Esc puts it away",
        ));
        return;
    }
    let Some(pr_id) = view.writable() else {
        app.flash = Some(Flash::note(view.unwritable()));
        return;
    };
    let summary = view.summary.trim().to_string();
    if view.verdict != Verdict::Approve && summary.is_empty() && view.pending_comments() == 0 {
        view.notice = Some(format!(
            "{} needs something said: a comment on a line, or a summary",
            view.verdict.label()
        ));
        view.focus = Focus::Bar;
        view.bar_row = BarRow::Summary;
        return;
    }
    view.notice = None;
    view.saving = Some("sending your review…".into());
    let (dir, pending, ticket, number, url, verdict) = (
        view.dir.clone(),
        view.pending.clone(),
        view.ticket,
        view.number,
        view.url.clone(),
        view.verdict,
    );
    #[cfg(test)]
    tests::SUBMITTED.with_borrow_mut(|s| s.push((verdict, pending.clone(), summary.clone())));
    let Some(tx) = tx else {
        return;
    };
    tokio::spawn(async move {
        let result = github::submit(&dir, &pr_id, pending.as_deref(), verdict, &summary).await;
        let _ = tx.send(Answer::Submitted {
            ticket,
            number,
            url,
            verdict,
            result,
        });
    });
}

/// The review landed. Sent: the footer says so, and the screen gives way
/// to the pull request's Reviews tab — in the modal it came from, or the
/// pane's — its page read again. Refused: the bar says why, everything
/// still pending.
pub(super) fn land_submitted(
    app: &mut App,
    ticket: u64,
    number: u64,
    url: String,
    verdict: Verdict,
    result: Result<(), String>,
) {
    let on_screen = super::screen_for(app, ticket).is_some();
    if let Err(why) = result {
        if let Some(view) = super::screen_for(app, ticket) {
            view.saving = None;
            view.notice = Some(why.clone());
        }
        app.flash = Some(Flash::failed(format!(
            "couldn't send the review of #{number}: {why}"
        )));
        return;
    }
    app.flash = Some(Flash::done(verdict.done(number)));
    app.pr_detail_inflight.want_fresh(&url);
    app.pr_detail_stale.insert(url.clone());
    if !on_screen {
        return;
    }
    let Some(Overlay::Review(view)) = app.overlay.take() else {
        return;
    };
    match view.back {
        Some(modal) => crate::pr_modal::land_on(app, Some(*modal), &url, PrTab::Reviews),
        None => {
            app.pr_tabs.switch(PrTab::Reviews);
            if app.previewed_pr().is_some_and(|pr| pr.url == url) {
                app.pending_pr_detail = Some((
                    crate::app::PendingPrDetail {
                        url,
                        number,
                        dir: view.dir,
                    },
                    std::time::Instant::now(),
                ));
            }
        }
    }
    app.dirty = true;
}

/// What the bar says beside the verdicts: what the review will send.
pub(super) fn pending_words(view: &ReviewScreen) -> String {
    match view.pending_comments() {
        0 => "nothing pending".to_string(),
        n => crate::flash::plural(n, "pending comment"),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::Verdict;
    use std::cell::RefCell;

    thread_local! {
        /// Every review [`super::submit`] sent off under test: its
        /// verdict, the pending review and the summary.
        pub(in crate::pr_review) static SUBMITTED: RefCell<Vec<(Verdict, Option<String>, String)>> =
            const { RefCell::new(Vec::new()) };
    }

    pub(in crate::pr_review) fn take_submitted() -> Vec<(Verdict, Option<String>, String)> {
        SUBMITTED.with_borrow_mut(std::mem::take)
    }
}
