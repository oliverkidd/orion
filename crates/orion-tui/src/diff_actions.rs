//! What the DIFF VIEWER (`⌘E`) knows beyond the diff.
//!
//! **The top line** names the pull request the viewer is about, over its
//! panels: `#1342 Fix PDF header · ✗ 44/45 checks`. A checkout's viewer reads
//! its branch's pull request (`App::pull_requests`, what `gh pr view` found);
//! a pull request's viewer reads that one. Where it stands comes from the PR
//! STORE (`App::prs`) — the page's tally of its checks once the page has been
//! read (`App::pr_detail`), else the store's one word for them, and `merged`
//! or `closed` once it is past taking work. A branch with no pull request
//! has no line.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, DiffView};
use crate::pull_request::{CheckCounts, Checks, Standing};
use crate::theme::Theme;

/// The pull request a DIFF VIEWER is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewedPr {
    pub number: u64,
    pub url: String,
    pub title: String,
}

/// The checkout `view` reads, when it reads one: None for a pull request's
/// viewer, which reads the project's own git (`event_loop::open_pr_review`).
pub(crate) fn checkout<'a>(app: &'a App, view: &DiffView) -> Option<&'a orion_core::Worktree> {
    if view.pr_review.is_some() {
        return None;
    }
    app.tree.worktrees.iter().find(|w| w.path == view.root)
}

/// The pull request `view` is about: the one it reads, or its checkout's
/// branch's — titled by the newest answer that named it.
pub(crate) fn pull_request(app: &App, view: &DiffView) -> Option<ViewedPr> {
    let (number, url, copy) = match &view.pr_review {
        Some(review) => (review.number, review.url.clone(), String::new()),
        None => {
            let pr = app.pull_requests.get(&checkout(app, view)?.id)?.as_ref()?;
            (pr.number, pr.url.clone(), pr.title.clone())
        }
    };
    let title = [
        app.prs.status(&url).map(|status| status.title),
        app.pr_detail.get(&url).map(|d| d.title.clone()),
    ]
    .into_iter()
    .flatten()
    .find(|title| !title.is_empty())
    .unwrap_or(copy);
    Some(ViewedPr { number, url, title })
}

/// The viewer's top line, `width` cells wide: the pull request's number and
/// title, the title cut to leave room for where it stands. None without a
/// pull request.
pub(crate) fn top_line(
    app: &App,
    view: &DiffView,
    width: usize,
    th: Theme,
) -> Option<Line<'static>> {
    let pr = pull_request(app, view)?;
    let mut tail = Vec::new();
    if let Some((text, color)) = standing(app, &pr.url, th) {
        tail.push(Span::styled(" · ", Style::default().fg(th.faint)));
        tail.push(Span::styled(text, Style::default().fg(color)));
    }
    let number = format!(" #{} ", pr.number);
    let tail_w: usize = tail.iter().map(|s| s.content.width()).sum();
    let room = width.saturating_sub(number.width() + tail_w + 1);
    let mut spans = vec![
        Span::styled(number, Style::default().fg(th.muted)),
        Span::styled(
            crate::ui::truncate_cells(&pr.title, room),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ),
    ];
    spans.extend(tail);
    Some(Line::from(spans))
}

/// Where the pull request at `url` stands, after its title: `merged` or
/// `closed` once it is past taking work, in the colours its page's border
/// gives them; while it is open, its checks — the page's tally
/// (`✗ 44/45 checks`), else the store's one word for them. None while
/// nothing is known of them, or there are none.
fn standing(app: &App, url: &str, th: Theme) -> Option<(String, ratatui::style::Color)> {
    let status = app.prs.status_or_open(url);
    match status.standing {
        Standing::Merged => return Some((status.word().to_string(), th.merged)),
        Standing::Closed => return Some((status.word().to_string(), th.faint)),
        Standing::Open | Standing::Draft => {}
    }
    let counted = app.pr_detail.get(url).map(|d| CheckCounts::of(&d.checks));
    if let Some((tally, color)) = counted.and_then(|n| crate::pr_preview::check_tally(n, th)) {
        return Some((format!("{tally} checks"), color));
    }
    if !status.checks_known {
        return None;
    }
    match status.health.checks {
        Checks::Failing => Some(("✗ checks failing".into(), th.err)),
        Checks::Pending => Some(("◐ checks running".into(), th.warn)),
        Checks::Passing => Some(("✓ checks passed".into(), th.ok)),
        Checks::Absent => None,
    }
}
