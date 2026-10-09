//! The PULL REQUEST PAGE: what the pane beside the grid shows while the
//! cursor rests on a pull request, and the PULL REQUESTS MODAL's reading
//! side — one implementation, laid out the way Cursor lays out a pull
//! request. A fixed head — the number and title, where it stands, and a
//! row of TABS: `Description · Changes 11 · Commits 4 · ✗ Checks 44/45 ·
//! ✓ Reviews`, each count beside its tab, a red ✗ or a green ✓ where the
//! checks and the reviews have a verdict, the active tab lit — over the
//! active tab's body, which scrolls under it.
//!
//! * **Description** — the body rendered as markdown, then the
//!   conversation.
//! * **Changes** — every file, its status and its `+`/`−`; acting on one
//!   opens the DIFF VIEWER on the pull request at that file.
//! * **Commits** — newest first, sha, subject, author and age; acting on
//!   one opens that commit's diff the same way.
//! * **Checks** — failed first, each with its status and how long it
//!   ran; acting on one opens its page in the browser.
//! * **Reviews** — GitHub's review decision, each reviewer's latest word,
//!   then the reviews themselves.
//!
//! Everything is one `gh pr view` (`pull_request::detail`), fetched off
//! the loop on the pane's debounce and kept with the rest (`pr_cache`), so
//! the tabs are up — their counts `…` — before it lands. The keys that
//! walk the tabs and their rows are [`keys`]; the surface that hosts the
//! page keeps a [`PrTabs`] of its own.
//!
//! Every line is laid out up front against the width — every wrap
//! decided before a cell is drawn — so scrolling is a slice and the line
//! count is exact: a pull request's body is arbitrary prose from someone
//! else's keyboard, with no natural row count, and a renderer that
//! wrapped at draw time could not tell the scroller how far down it may
//! go. The body is markdown and is rendered as markdown (the MARKDOWN
//! module) under GitHub's comment rule that a newline is a line break;
//! [`wrap`] stays for the plain text the panels wrap elsewhere.

use crate::markdown::{self, FoldKey, Folds};
use crate::pull_request::{
    CheckCounts, CheckState, PrCheck, PrComment, PrDetail, PrFile, Standing, REVIEW_REQUESTED,
};
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// Left inset of the body text, so prose doesn't hug the pane rule.
pub(crate) const INDENT: &str = " ";
/// Narrowest the body wraps to: below this, wrapping yields a word per line
/// and overflowing the pane reads better than that.
pub(crate) const MIN_BODY_W: usize = 20;
/// The fewest cells a row's title, path or name keeps before the columns
/// beside it give way — the PULL REQUESTS MODAL's rows and the page's
/// listings alike.
pub(crate) const MIN_TEXT_W: usize = 12;

/// Wrap `text` to `width` columns on word boundaries, honoring the hard
/// line breaks already in it. A word longer than the whole width (a URL, a
/// long path) is broken at the edge rather than being allowed to overflow.
/// An empty input is one empty line — a blank line in a body is a paragraph
/// break and has to survive.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.replace('\t', "    ").lines() {
        let mut line = String::new();
        let mut len = 0usize;
        for word in raw.split(' ') {
            let wlen = word.chars().count();
            // A word that can never fit: emit what we have, then break the
            // word across as many rows as it takes.
            if wlen > width {
                if len > 0 {
                    out.push(std::mem::take(&mut line));
                }
                let mut chunk = String::new();
                for c in word.chars() {
                    if chunk.chars().count() == width {
                        out.push(std::mem::take(&mut chunk));
                    }
                    chunk.push(c);
                }
                line = chunk;
                len = line.chars().count();
                continue;
            }
            let need = if len == 0 { wlen } else { wlen + 1 };
            if len + need > width {
                out.push(std::mem::take(&mut line));
                len = 0;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            line.push_str(word);
            len += wlen;
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Trim a row of styled segments to `width` columns. Whole segments fall
/// off the end first — a row built most-important-first loses its tail
/// before its head — and whatever segment straddles the edge is clipped
/// with an ellipsis. For the rows that are one line by design (a list
/// row beside a badge); ratatui silently clips an overwide line, taking
/// the rest of the row with it, so "it'll probably fit" is not good enough.
pub(crate) fn fit(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut kept: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let len = span.content.chars().count();
        if used + len <= width {
            used += len;
            kept.push(span);
            continue;
        }
        let room = width - used;
        if room > 1 {
            kept.push(Span::styled(
                crate::ui::truncate(&span.content, room),
                span.style,
            ));
        }
        break;
    }
    Line::from(kept)
}

/// Flow styled runs into rows at most `width` cells wide: words break at
/// spaces, mid-word only for a word wider than a whole row, and a run's
/// own spaces between two of its words keep its style. The first row is
/// led by `lead`, every row after it by `cont` — a hanging indent — and
/// each comes back as its spans, `lead`/`cont` first, so a caller can
/// still dress the row (a selection's marker and fill).
fn flow(runs: &[(String, Style)], width: usize, lead: &str, cont: &str) -> Vec<Vec<Span<'static>>> {
    let lead_w = lead.width().max(cont.width());
    let avail = width.saturating_sub(lead_w).max(1);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut cur = 0usize;
    // The spaces waiting for the next word, in the style they came in.
    let mut gap: Option<(Style, usize)> = None;
    let push = |row: &mut Vec<Span<'static>>, text: &str, style: Style| match row.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push_str(text),
        _ => row.push(Span::styled(text.to_string(), style)),
    };
    let mut new_row = |row: &mut Vec<Span<'static>>, cur: &mut usize| {
        rows.push(std::mem::take(row));
        *cur = 0;
    };
    for (text, style) in runs {
        for (i, word) in text.split(' ').enumerate() {
            if i > 0 {
                let n = gap.map_or(0, |(_, n)| n);
                gap = Some((*style, n + 1));
            }
            if word.is_empty() {
                continue;
            }
            let w = word.width();
            let space = if cur > 0 {
                gap.map_or(0, |(_, n)| n)
            } else {
                0
            };
            if cur > 0 && cur + space + w > avail {
                new_row(&mut row, &mut cur);
            } else if let (Some((gap_style, n)), true) = (gap, cur > 0) {
                push(&mut row, &" ".repeat(n), gap_style);
                cur += n;
            }
            gap = None;
            if w <= avail {
                push(&mut row, word, *style);
                cur += w;
                continue;
            }
            // Wider than any row: break it at the edge.
            for ch in word.chars() {
                let cw = ch.to_string().width();
                if cur > 0 && cur + cw > avail {
                    new_row(&mut row, &mut cur);
                }
                push(&mut row, &ch.to_string(), *style);
                cur += cw;
            }
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows.into_iter()
        .enumerate()
        .map(|(i, mut spans)| {
            let indent = if i == 0 { lead } else { cont };
            spans.insert(0, Span::raw(indent.to_string()));
            spans
        })
        .collect()
}

/// [`flow`]'s rows as lines.
fn flow_lines(
    runs: &[(String, Style)],
    width: usize,
    lead: &str,
    cont: &str,
) -> Vec<Line<'static>> {
    flow(runs, width, lead, cont)
        .into_iter()
        .map(Line::from)
        .collect()
}

// ---- the tabs ----

/// The PULL REQUEST PAGE's tabs, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum PrTab {
    #[default]
    Description,
    Changes,
    Commits,
    Checks,
    Reviews,
}

impl PrTab {
    pub const ALL: [PrTab; 5] = [
        PrTab::Description,
        PrTab::Changes,
        PrTab::Commits,
        PrTab::Checks,
        PrTab::Reviews,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PrTab::Description => "Description",
            PrTab::Changes => "Changes",
            PrTab::Commits => "Commits",
            PrTab::Checks => "Checks",
            PrTab::Reviews => "Reviews",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// The tab `delta` steps along the row, wrapping at either end.
    pub fn step(self, delta: i32) -> Self {
        let n = Self::ALL.len() as i32;
        Self::ALL[(self.index() as i32 + delta).rem_euclid(n) as usize]
    }

    /// A tab that lists things a cursor walks — files, commits, checks —
    /// rather than prose that scrolls.
    pub fn lists(self) -> bool {
        matches!(self, PrTab::Changes | PrTab::Commits | PrTab::Checks)
    }

    /// What acting on the row under the cursor does here, for the key
    /// hints: the DIFF VIEWER on a file or a commit, a check's page —
    /// and, on the tabs with no rows, the pull request in the browser.
    pub fn act_does(self) -> &'static str {
        match self {
            PrTab::Changes => "diff the file",
            PrTab::Commits => "diff the commit",
            PrTab::Checks => "open the check",
            PrTab::Description | PrTab::Reviews => "open in browser",
        }
    }
}

/// Where a reader stands on a PULL REQUEST PAGE: the tab, and the row
/// cursor of each tab that lists things. One per surface that shows the
/// page — the pane's (`App::pr_tabs`) and the modal's
/// (`PullRequestsView::tabs`) — so each keeps its own place. Moving to
/// another pull request rewinds the rows ([`PrTabs::rewind`]) and keeps
/// the tab: reading the checks of one pull request after another is the
/// common walk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrTabs {
    pub tab: PrTab,
    /// The row cursor of each tab, by [`PrTab::ALL`] index; only the
    /// listing tabs' move.
    rows: [usize; 5],
    /// The cursor moved since the last draw, which scrolls it into view.
    follow: bool,
    /// As of the last draw, for the mouse: every tab label's cell rect,
    /// and every listed row's visible rect by row.
    pub tab_hits: Vec<(Rect, PrTab)>,
    pub row_hits: Vec<(Rect, usize)>,
    /// The `<details>` turned in the description and the comments, and
    /// as of the last draw each summary's visible rect, for the mouse.
    pub folds: Folds,
    pub fold_hits: Vec<(Rect, FoldKey)>,
    /// As of the last draw, for paging: each listed row's first line and
    /// line count in the body, and the body's height.
    spans: Vec<(usize, usize)>,
    view_h: u16,
}

/// A move over the page: ↑/↓, PgUp/PgDn, Home/End, the wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// A line (a row, on a tab that lists things) down, or up for < 0.
    Line(i32),
    /// A page down, or up for < 0.
    Page(i32),
    Top,
    Bottom,
}

impl PrTabs {
    /// The row cursor of the tab on screen.
    pub fn row(&self) -> usize {
        self.rows[self.tab.index()]
    }

    /// Show `tab`, its cursor brought into view. Whether it changed.
    pub fn switch(&mut self, tab: PrTab) -> bool {
        let changed = self.tab != tab;
        self.tab = tab;
        self.follow = true;
        changed
    }

    /// Put the cursor on row `row` of the tab on screen.
    pub fn select(&mut self, row: usize) {
        self.rows[self.tab.index()] = row;
        self.follow = true;
    }

    /// Another pull request: every tab's rows back to the top, and every
    /// `<details>` shut again. The tab stays what it was.
    pub fn rewind(&mut self) {
        self.rows = [0; 5];
        self.follow = false;
        self.folds.clear();
    }

    /// `nav` over the page: on a tab that lists things it moves the row
    /// cursor — the view following it on the next draw — so what Enter
    /// acts on is always on screen; on the others it scrolls `scroll`,
    /// held to `max`. Before the first draw of a listing (nothing to
    /// walk yet) it scrolls too.
    pub fn navigate(&mut self, nav: Nav, scroll: &mut u16, max: u16) {
        let page = i32::from(self.view_h.max(1));
        if !self.tab.lists() || self.spans.is_empty() {
            let to = match nav {
                Nav::Line(d) => i32::from(*scroll) + d,
                Nav::Page(d) => i32::from(*scroll) + d * page,
                Nav::Top => 0,
                Nav::Bottom => i32::from(max),
            };
            *scroll = u16::try_from(to.clamp(0, i32::from(max))).unwrap_or(0);
            return;
        }
        let last = self.spans.len() - 1;
        let at = self.row().min(last);
        let next = match nav {
            Nav::Line(d) => (at as i64 + i64::from(d)).clamp(0, last as i64) as usize,
            // A page: the row standing a body's height further along.
            Nav::Page(d) => {
                let from = self.spans[at].0 as i64;
                let target = from + i64::from(d) * i64::from(page);
                let found = if d > 0 {
                    self.spans
                        .iter()
                        .rposition(|(first, _)| *first as i64 <= target)
                } else {
                    self.spans
                        .iter()
                        .position(|(first, _)| *first as i64 >= target)
                };
                let found = found.unwrap_or(if d > 0 { last } else { 0 });
                // A row taller than a page still moves one.
                match found {
                    f if f == at && d > 0 => (at + 1).min(last),
                    f if f == at && d < 0 => at.saturating_sub(1),
                    f => f,
                }
            }
            Nav::Top => {
                *scroll = 0;
                0
            }
            Nav::Bottom => {
                *scroll = max;
                last
            }
        };
        self.select(next);
    }
}

/// The page's own keys, in the one table the pane's key handler
/// ([`pane_key`]) and the PULL REQUESTS MODAL match and their hints
/// spell. The pane walks the tabs with `Tab`; the modal, whose `Tab` and
/// `⇧Tab` launch and whose letters type into its filter, with `⇧←`/`⇧→`,
/// and walks a listing with `⇧↑`/`⇧↓`, its `↑`/`↓` being the list of
/// pull requests'.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const NEXT_TAB: Key = Key::new(&["tab"], "next tab");
    pub const PREV_TAB: Key = Key::new(&["shift+tab"], "previous tab");
    /// The two as one hint: `Tab/⇧Tab tabs`.
    pub const TABS: Key = Key::new(&["tab", "shift+tab"], "tabs").show(2);
    /// The modal's: `⇧←/⇧→ tabs`.
    pub const MODAL_TABS: Key = Key::new(&["shift+left", "shift+right"], "tabs").show(2);
    /// The modal's row cursor on a listing: `⇧↑/⇧↓ pick`.
    pub const MODAL_ROWS: Key = Key::new(&["shift+up", "shift+down"], "pick").show(2);
    #[cfg(test)]
    pub const ALL: &[Key] = &[NEXT_TAB, PREV_TAB, TABS, MODAL_TABS, MODAL_ROWS];
}

// ---- the page ----

/// What the page has to show: the pull request's row — its number and
/// title, all there is to say until its body lands — and the body once it
/// has, or word that it couldn't be read.
pub struct PageInput<'a> {
    pub number: u64,
    pub title: &'a str,
    pub detail: Option<&'a PrDetail>,
    /// Where it stands, as `App::prs` says — the border's state word —
    /// and, when the page is the cache's copy no live answer has replaced
    /// yet, the dim word beside it ([`freshness`]).
    pub status: Option<crate::pr_store::PrStatus>,
    pub freshness: Option<&'static str>,
    /// `gh` couldn't read it: the page says so rather than "reading it…".
    pub failed: bool,
    /// A comment of yours is on its way (the modal's COMMENT BOX).
    pub posting: bool,
    /// The key that opens the pull request in the browser, and the one
    /// that reads its whole diff — what the page's own sentences name.
    pub browser_key: String,
    pub diff_key: String,
    /// Unix seconds, for ages and durations.
    pub now: i64,
}

/// Where one tab's label landed: its row in the head and its columns,
/// `[from, to)`.
type TabSpot = (u16, u16, u16, PrTab);

/// Where the body's text starts: two cells in, the first of them the row
/// cursor's `▌` on a listing, so prose, rows and the rules over them line
/// up as the main page's list does under its bands.
const BODY: &str = "  ";

/// The page laid out for one width: the fixed head, the active tab's
/// body, and where the tabs and the listed rows landed.
pub struct Page {
    pub head: Vec<Line<'static>>,
    tabs: Vec<TabSpot>,
    pub body: Vec<Line<'static>>,
    /// Each listed row's first line in the body and its line count.
    spans: Vec<(usize, usize)>,
    folds: Vec<FoldSpot>,
}

/// A `<details>` summary on the page: the body lines it took, and which
/// one it is.
type FoldSpot = (std::ops::Range<usize>, FoldKey);

/// Lay the page out for `width` columns. `focused` is whether the surface
/// holding it has the keys — the row cursor and the active tab's rule
/// wear the accent then, and a quieter mark otherwise, as a list's do.
///
/// The head is what the frame's own border ([`border`]) leaves to say: the
/// sentence — who, from where into where, how much, how long ago — and
/// the tabs, underlined.
pub fn page(input: &PageInput, tabs: &PrTabs, focused: bool, width: usize, th: Theme) -> Page {
    let width = width.max(1);
    let mut head = vec![Line::from("")];
    if let Some(detail) = input.detail {
        head.push(sentence(detail, input.now, width, th));
        head.push(Line::from(""));
    }
    let (rows, hits) = tab_row(input, tabs.tab, width, th);
    let first_row = head.len() as u16;
    let last_row = first_row + (rows.len() as u16).saturating_sub(1);
    head.extend(rows);
    let tabs_at: Vec<TabSpot> = hits
        .into_iter()
        .map(|(row, from, to, tab)| (first_row + row, from, to, tab))
        .collect();
    head.push(tab_rule(&tabs_at, tabs.tab, last_row, focused, width, th));
    let (body, spans, folds) = body(input, tabs, focused, width, th);
    Page {
        head,
        tabs: tabs_at,
        body,
        spans,
        folds,
    }
}

/// The page's top border, for the surface that frames it to draw: where
/// the pull request stands — `● Open`, `○ Draft`, a red `● Conflicts`
/// while it is open and cannot merge, the merged purple `● Merged`, a
/// faint `● Closed` — then `#42` and the title; and, for the border's
/// right end, what it changes: `+106 −4`. The state and the counts wait
/// for the body; the number and the title are there from the first paint.
/// `focused` lights the number in the accent, as the border it sits on is.
pub fn border(
    input: &PageInput,
    focused: bool,
    th: Theme,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let mut left = Vec::new();
    if input.detail.is_some() {
        let status = input.status.clone().unwrap_or_default();
        let (word, color) = state_word(&status, th);
        left.push(Span::styled(
            word,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        if let Some(fresh) = input.freshness {
            left.push(Span::styled(
                format!(" {fresh}"),
                Style::default().fg(th.dim),
            ));
        }
        left.push(Span::raw("  "));
    }
    // The newest answer's title, as the row beside it shows it; the
    // page's, then the row's own, while none has been observed.
    let title = input.detail.map_or(input.title, |d| d.title.as_str());
    let title = input.status.as_ref().map_or(title, |s| s.title_or(title));
    left.push(Span::styled(
        format!("#{} ", input.number),
        Style::default().fg(if focused { th.accent } else { th.dim }),
    ));
    left.push(Span::styled(
        title.to_string(),
        Style::default().fg(th.text).add_modifier(Modifier::BOLD),
    ));
    let right = input.detail.map_or_else(Vec::new, |d| {
        vec![
            Span::styled(format!("+{}", d.additions), Style::default().fg(th.added)),
            Span::styled(
                format!(" −{}", d.deletions),
                Style::default().fg(th.removed),
            ),
        ]
    });
    (left, right)
}

/// Where a pull request stands, as its border says it — `App::prs`'s
/// word, the one its list row and badge draw.
fn state_word(status: &crate::pr_store::PrStatus, th: Theme) -> (String, Color) {
    match status.standing {
        Standing::Open | Standing::Draft if status.health.conflicts => {
            ("● Conflicts".into(), th.err)
        }
        Standing::Draft => ("○ Draft".into(), th.muted),
        Standing::Open => ("● Open".into(), th.ok),
        Standing::Merged => ("● Merged".into(), th.merged),
        Standing::Closed => ("● Closed".into(), th.faint),
    }
}

/// The dim word beside the border's state while the page on screen is
/// the copy the cache hydrated (`pr_cache`) and no live answer has
/// replaced it: `updating…` while a read is on its way, else `cached` —
/// so a state word the last launch left is never passed off as current.
/// A page read live this session says nothing, however old.
pub fn freshness(app: &crate::app::App, url: &str) -> Option<&'static str> {
    let from_cache = app.pr_detail_stale.contains(url)
        && (!app.pr_detail_at.contains_key(url) || app.pr_detail_failed.contains(url));
    if !from_cache || !app.pr_detail.contains_key(url) {
        return None;
    }
    Some(if app.pr_detail_inflight.in_flight(&url.to_string()) {
        "updating…"
    } else {
        "cached"
    })
}

/// Styled runs' width in cells.
fn runs_w(runs: &[(String, Style)]) -> usize {
    runs.iter().map(|(t, _)| t.width()).sum()
}

/// Spans' width in cells.
fn spans_w(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// The line under the border, compact: who opened it, from which branch
/// into which, how many commits, how long ago — `webdevcody · feat/links →
/// main · 2 commits · 3d ago` — and its labels at the right end in their
/// GitHub colours. Always one line: the head branch gives way first, cut
/// from its left, then the age, then the labels, then the end of the line.
fn sentence(d: &PrDetail, now: i64, width: usize, th: Theme) -> Line<'static> {
    const MIN_HEAD: usize = 8;
    let dim = Style::default().fg(th.dim);
    let muted = Style::default().fg(th.muted);
    let sep = || (" · ".to_string(), Style::default().fg(th.faint));
    let age = crate::pull_request::rfc3339_secs(&d.created_at)
        .map(|at| crate::hosts::ago_label((now - at).max(0).saturating_mul(1000)))
        .filter(|a| !a.is_empty());
    let commits = d.commits.len();
    let build = |head: &str, with_age: bool| {
        let mut runs: Vec<(String, Style)> = Vec::new();
        let part = |runs: &mut Vec<(String, Style)>, more: Vec<(String, Style)>| {
            if !runs.is_empty() {
                runs.push(sep());
            }
            runs.extend(more);
        };
        if !d.author.is_empty() {
            let who = Style::default().fg(th.text).add_modifier(Modifier::BOLD);
            part(&mut runs, vec![(d.author.clone(), who)]);
        }
        if !d.head.is_empty() {
            part(
                &mut runs,
                vec![
                    (head.to_string(), muted),
                    (" → ".to_string(), dim),
                    (d.base.clone(), muted),
                ],
            );
        }
        if commits > 0 {
            let noun = if commits == 1 { "commit" } else { "commits" };
            part(&mut runs, vec![(format!("{commits} {noun}"), dim)]);
        }
        if let (true, Some(age)) = (with_age, &age) {
            part(&mut runs, vec![(age.clone(), dim)]);
        }
        runs
    };
    let mut labels: Vec<Span<'static>> = Vec::new();
    for label in &d.labels {
        if !labels.is_empty() {
            labels.push(Span::raw("  "));
        }
        let color = crate::theme::hex(&label.color).unwrap_or(th.muted);
        labels.push(Span::styled(label.name.clone(), Style::default().fg(color)));
    }
    // The indent before it, a cell of air after it.
    let room = width.saturating_sub(INDENT.width() + 1);
    let labels_w = if labels.is_empty() {
        0
    } else {
        spans_w(&labels) + 2
    };
    let avail = |with_labels: bool| room.saturating_sub(if with_labels { labels_w } else { 0 });
    let shortened = |with_age: bool, with_labels: bool| {
        let full = build(&d.head, with_age);
        match runs_w(&full).checked_sub(avail(with_labels)) {
            None | Some(0) => full,
            Some(over) => {
                let keep = d.head.width().saturating_sub(over).max(MIN_HEAD);
                build(&crate::ui::truncate_left(&d.head, keep), with_age)
            }
        }
    };
    let has_labels = !labels.is_empty();
    let (runs, with_labels) = [(true, true), (false, true), (true, false), (false, false)]
        .into_iter()
        .filter(|(_, l)| has_labels || !*l)
        .map(|(age, l)| (shortened(age, l), l))
        .find(|(runs, l)| runs_w(runs) <= avail(*l))
        .unwrap_or_else(|| (shortened(false, false), false));
    let text: Vec<Span<'static>> = runs
        .into_iter()
        .map(|(t, style)| Span::styled(t, style))
        .collect();
    let mut spans = vec![Span::raw(INDENT)];
    spans.extend(fit(text, avail(with_labels)).spans);
    if with_labels {
        let used = spans_w(&spans);
        let at = width.saturating_sub(1 + spans_w(&labels));
        spans.push(Span::raw(" ".repeat(at.saturating_sub(used))));
        spans.extend(labels);
    }
    Line::from(spans)
}

/// One tab's label: its name, then a count beside it — dim, `…` while the
/// body is on its way — or, on Checks and Reviews, the verdict in its
/// colour: the checks' tally (`✗ 1/3` crimson, `◐ 1/2` gold, `✓ 3/3`
/// green) and the review decision (`✓ Approved`, `✗ Changes requested`,
/// the gold `○ Review required`, or a muted `· Reviewed` for reviews that
/// came to no decision).
fn tab_label(
    tab: PrTab,
    input: &PageInput,
    th: Theme,
) -> (&'static str, Option<String>, Option<(String, Color)>) {
    let name = tab.name();
    let Some(d) = input.detail else {
        let count = (!input.failed && tab.lists()).then(|| "…".to_string());
        return (name, count, None);
    };
    match tab {
        PrTab::Description => (name, None, None),
        PrTab::Changes => (name, Some(d.changed_files.to_string()), None),
        PrTab::Commits => (name, Some(d.commits.len().to_string()), None),
        PrTab::Checks => {
            let n = CheckCounts::of(&d.checks);
            let verdict = (n.total > 0).then(|| {
                let (mark, color) = if n.failed > 0 {
                    ("✗", th.err)
                } else if n.running > 0 {
                    ("◐", th.warn)
                } else {
                    ("✓", th.ok)
                };
                (format!("{mark} {}/{}", n.ok, n.total), color)
            });
            (name, None, verdict)
        }
        PrTab::Reviews => {
            let reviewed = d.comments.iter().any(|c| !c.review_state.is_empty());
            let verdict = match d.review_decision.as_str() {
                "APPROVED" => Some(("✓ Approved", th.ok)),
                "CHANGES_REQUESTED" => Some(("✗ Changes requested", th.err)),
                "REVIEW_REQUIRED" => Some(("○ Review required", th.warn)),
                _ if reviewed => Some(("· Reviewed", th.muted)),
                _ => None,
            };
            (name, None, verdict.map(|(t, c)| (t.to_string(), c)))
        }
    }
}

/// The tab labels, as plain text — the row the tests and the docs read.
pub fn tab_texts(input: &PageInput, th: Theme) -> Vec<String> {
    PrTab::ALL
        .iter()
        .map(|tab| {
            let (name, count, verdict) = tab_label(*tab, input, th);
            let mut text = name.to_string();
            for more in count.into_iter().chain(verdict.map(|(v, _)| v)) {
                text.push(' ');
                text.push_str(&more);
            }
            text
        })
        .collect()
}

/// The row of tabs, three cells apart and wrapped onto as many rows as
/// the width needs — the one on show in bold — and where each label
/// landed: `(row, from, to, tab)`. A label too wide for a row of its own
/// keeps only its verdict's mark.
fn tab_row(
    input: &PageInput,
    active: PrTab,
    width: usize,
    th: Theme,
) -> (Vec<Line<'static>>, Vec<TabSpot>) {
    const GAP: usize = 3;
    let indent = INDENT.width();
    let mut rows: Vec<Vec<Span<'static>>> = vec![vec![Span::raw(INDENT)]];
    let mut hits = Vec::new();
    let mut x = indent;
    for tab in PrTab::ALL {
        let (name, count, verdict) = tab_label(tab, input, th);
        let name_style = if tab == active {
            Style::default().fg(th.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.muted)
        };
        let label = |verdict: Option<&(String, Color)>| {
            let mut spans = vec![Span::styled(name, name_style)];
            if let Some(count) = &count {
                spans.push(Span::styled(
                    format!(" {count}"),
                    Style::default().fg(th.dim),
                ));
            }
            if let Some((text, color)) = verdict {
                spans.push(Span::styled(
                    format!(" {text}"),
                    Style::default().fg(*color),
                ));
            }
            spans
        };
        let mut spans = label(verdict.as_ref());
        if spans_w(&spans) + indent > width {
            let mark = verdict.as_ref().map(|(text, color)| {
                (
                    text.split(' ').next().unwrap_or_default().to_string(),
                    *color,
                )
            });
            spans = label(mark.as_ref());
        }
        let w = spans_w(&spans);
        if x > indent && x + GAP + w > width {
            rows.push(vec![Span::raw(INDENT)]);
            x = indent;
        } else if x > indent {
            if let Some(row) = rows.last_mut() {
                row.push(Span::raw(" ".repeat(GAP)));
            }
            x += GAP;
        }
        let row = rows.len() - 1;
        hits.push((row as u16, x as u16, (x + w) as u16, tab));
        rows[row].extend(spans);
        x += w;
    }
    let lines = rows.into_iter().map(|spans| fit(spans, width)).collect();
    (lines, hits)
}

/// The rule under the tabs, drawn heavier under the one on show — in the
/// accent while the page has the keys, muted otherwise — when that tab is
/// on the last row of them; plain across otherwise.
fn tab_rule(
    spots: &[TabSpot],
    active: PrTab,
    last_row: u16,
    focused: bool,
    width: usize,
    th: Theme,
) -> Line<'static> {
    let edge = Style::default().fg(th.edge);
    let Some(&(_, from, to, _)) = spots
        .iter()
        .find(|(row, .., tab)| *tab == active && *row == last_row)
    else {
        return Line::from(Span::styled("─".repeat(width), edge));
    };
    let from = usize::from(from).saturating_sub(1).min(width);
    let to = (usize::from(to) + 1).clamp(from, width);
    let lit = Style::default().fg(if focused { th.accent } else { th.muted });
    Line::from(vec![
        Span::styled("─".repeat(from), edge),
        Span::styled("━".repeat(to - from), lit),
        Span::styled("─".repeat(width - to), edge),
    ])
}

/// `left`, then `right` pinned to the line's end a cell clear of it, the
/// whole exactly `width` cells: the left gives way — cut, with an
/// ellipsis — when the two do not fit.
fn columns(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let right_w = spans_w(&right);
    let room = width.saturating_sub(right_w + 1);
    let mut spans = fit(left, room.saturating_sub(1)).spans;
    let used = spans_w(&spans);
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(used + right_w + 1)),
    ));
    spans.extend(right);
    spans.push(Span::raw(" "));
    spans
}

/// `text` right-aligned in a column `w` cells wide.
fn right_in(text: &str, w: usize, style: Style) -> Span<'static> {
    Span::styled(format!("{text:>w$}"), style)
}

/// The checks in the Checks tab's order: grouped by the workflow they ran
/// in — a group per workflow, in the order GitHub listed them, the group
/// with the worst check first — failed first within each. The order its
/// rows are walked and acted on in ([`row_act`]). A check from no workflow
/// (a commit status, an app's run) groups under `Other`, or under `Checks`
/// when nothing ran in a workflow at all.
pub(crate) fn checks_in_order(d: &PrDetail) -> Vec<(String, Vec<&PrCheck>)> {
    let mut groups: Vec<(String, Vec<&PrCheck>)> = Vec::new();
    for check in &d.checks {
        match groups.iter_mut().find(|(wf, _)| *wf == check.workflow) {
            Some((_, rows)) => rows.push(check),
            None => groups.push((check.workflow.clone(), vec![check])),
        }
    }
    for (_, rows) in &mut groups {
        rows.sort_by_key(|c| c.state);
    }
    groups.sort_by_key(|(_, rows)| rows.first().map(|c| c.state));
    let only = groups.len() == 1;
    for (wf, _) in &mut groups {
        if wf.is_empty() {
            *wf = if only { "Checks" } else { "Other" }.to_string();
        }
    }
    groups
}

/// The active tab's body, and each listed row's span in it.
fn body(
    input: &PageInput,
    tabs: &PrTabs,
    focused: bool,
    width: usize,
    th: Theme,
) -> (Vec<Line<'static>>, Vec<(usize, usize)>, Vec<FoldSpot>) {
    let dim = Style::default().fg(th.dim);
    let body_w = width.saturating_sub(BODY.len() + 1).max(MIN_BODY_W);
    let say = |text: &str| -> Vec<Line<'static>> {
        let mut out = vec![Line::from("")];
        out.extend(flow_lines(&[(text.to_string(), dim)], width, BODY, BODY));
        out
    };
    let Some(detail) = input.detail else {
        let text = if input.failed {
            format!(
                "couldn't read this pull request — is gh installed and logged in? {} still opens it in the browser.",
                input.browser_key
            )
        } else {
            "reading it…".to_string()
        };
        return (say(&text), Vec::new(), Vec::new());
    };
    let mut listed = Listing {
        lines: vec![Line::from("")],
        spans: Vec::new(),
        cursor: tabs.row(),
        focused,
        width,
        th,
    };
    let mut prose = Prose {
        lines: vec![Line::from("")],
        spots: Vec::new(),
        folds: &tabs.folds,
        body_w,
        width,
        th,
    };
    match tabs.tab {
        PrTab::Description => {
            description(&mut prose, detail, input.now);
            if input.posting {
                prose.lines.push(Line::from(""));
                prose.lines.push(Line::from(Span::styled(
                    format!("{BODY}── posting your comment… ──"),
                    dim,
                )));
            }
            return (prose.lines, Vec::new(), prose.spots);
        }
        PrTab::Reviews => {
            reviews(&mut prose, detail, input.now);
            return (prose.lines, Vec::new(), prose.spots);
        }
        PrTab::Changes if detail.files.is_empty() => {
            return (say("no files changed"), Vec::new(), Vec::new())
        }
        PrTab::Commits if detail.commits.is_empty() => {
            return (say("no commits"), Vec::new(), Vec::new())
        }
        PrTab::Checks if detail.checks.is_empty() => {
            return (
                say("no checks ran on its head commit"),
                Vec::new(),
                Vec::new(),
            )
        }
        PrTab::Changes => {
            let total = detail.changed_files as usize;
            let noun = if total == 1 { "file" } else { "files" };
            listed.lines.push(crate::ui::section_rule(
                &format!("{total} {noun}"),
                th.muted,
                vec![
                    Span::styled(
                        format!("+{}", detail.additions),
                        Style::default().fg(th.added),
                    ),
                    Span::styled(
                        format!(" −{}", detail.deletions),
                        Style::default().fg(th.removed),
                    ),
                ],
                width,
                th,
            ));
            let counts =
                crate::ui::CountColumns::of(detail.files.iter().map(|f| Some(file_lines(f))));
            for file in &detail.files {
                listed.row(file_row(file, &counts, width, th));
            }
            if detail.files.len() < total {
                listed.lines.push(Line::from(""));
                listed.lines.extend(flow_lines(
                    &[(
                        format!(
                            "the first {} of {total} files — {} reads every one",
                            detail.files.len(),
                            input.diff_key
                        ),
                        dim,
                    )],
                    width,
                    BODY,
                    BODY,
                ));
            }
        }
        PrTab::Commits => {
            let mut authors: Vec<&str> = Vec::new();
            for c in &detail.commits {
                if !c.author.is_empty() && !authors.contains(&c.author.as_str()) {
                    authors.push(&c.author);
                }
            }
            let n = detail.commits.len();
            let right = match authors.as_slice() {
                [] => Vec::new(),
                [one] => vec![Span::styled(one.to_string(), dim)],
                many => vec![Span::styled(format!("{} authors", many.len()), dim)],
            };
            let noun = if n == 1 { "commit" } else { "commits" };
            listed.lines.push(crate::ui::section_rule(
                &format!("{n} {noun}"),
                th.muted,
                right,
                width,
                th,
            ));
            let cols = CommitCols {
                who: if authors.len() > 1 {
                    authors.iter().map(|a| a.width()).max().unwrap_or(0).min(16)
                } else {
                    0
                },
                add: detail
                    .commits
                    .iter()
                    .map(|c| c.additions.map_or(0, count_w))
                    .max()
                    .unwrap_or(0),
                del: detail
                    .commits
                    .iter()
                    .map(|c| c.deletions.map_or(0, count_w))
                    .max()
                    .unwrap_or(0),
                age: detail
                    .commits
                    .iter()
                    .map(|c| commit_age(c, input.now).width())
                    .max()
                    .unwrap_or(0),
            };
            for commit in &detail.commits {
                listed.row(commit_row(commit, &cols, input.now, width, th));
            }
        }
        PrTab::Checks => {
            for (i, (workflow, checks)) in checks_in_order(detail).into_iter().enumerate() {
                if i > 0 {
                    listed.lines.push(Line::from(""));
                }
                let n = CheckCounts::of(checks.iter().copied());
                let tally = if n.failed > 0 {
                    th.err
                } else if n.running > 0 {
                    th.warn
                } else {
                    th.dim
                };
                listed.lines.push(crate::ui::section_rule(
                    &workflow,
                    th.muted,
                    vec![Span::styled(
                        format!("{}/{}", n.ok, n.total),
                        Style::default().fg(tally),
                    )],
                    width,
                    th,
                ));
                for check in checks {
                    listed.row(check_row(check, input.now, width, th));
                }
            }
        }
    }
    (listed.lines, listed.spans, Vec::new())
}

/// The cells `+12` (or `−3`) takes; nothing for a zero, which is left out.
fn count_w(n: u64) -> usize {
    if n == 0 {
        0
    } else {
        1 + n.to_string().len()
    }
}

/// A count for a column: `+12`, or nothing for a zero.
fn count_text(sign: char, n: u64) -> String {
    if n == 0 {
        String::new()
    } else {
        format!("{sign}{n}")
    }
}

/// A Changes row as the DIFF VIEWER lists a committed file: its
/// two-letter status code (`M `) in the diff's colour, the path — its
/// folders dim, its file name bright, cut from the left so the name
/// stays — and its `+A −R` in the viewer's columns, a zero dim.
fn file_row(
    file: &PrFile,
    counts: &crate::ui::CountColumns,
    width: usize,
    th: Theme,
) -> Vec<Span<'static>> {
    let xy = [file.status(), ' '];
    let color = crate::ui::change_color(xy, th);
    let lead = BODY.len() + 3;
    // The counts go before the path shortens past [`MIN_TEXT_W`]; the path
    // keeps clear of them and of the cell of air `columns` leaves.
    let counts_w = counts.width().saturating_sub(1);
    let right = if width >= lead + MIN_TEXT_W + counts_w + 2 {
        counts.cells(Some(file_lines(file)), th)
    } else {
        Vec::new()
    };
    let room = width.saturating_sub(lead + if right.is_empty() { 2 } else { counts_w + 2 });
    let path = crate::ui::truncate_left(&file.path, room);
    let (dir, name) = match path.rfind('/') {
        Some(i) => path.split_at(i + 1),
        None => ("", path.as_str()),
    };
    let name_color = if xy[0] == 'D' { th.muted } else { th.text };
    let left = vec![
        Span::raw(BODY),
        Span::styled(
            format!("{} ", xy.iter().collect::<String>()),
            Style::default().fg(color),
        ),
        Span::styled(dir.to_string(), Style::default().fg(th.dim)),
        Span::styled(name.to_string(), Style::default().fg(name_color)),
    ];
    columns(left, right, width)
}

/// A changed file's counts as the DIFF VIEWER keeps them.
fn file_lines(file: &PrFile) -> crate::git_diff::LineChanges {
    crate::git_diff::LineChanges {
        added: file.additions,
        removed: file.deletions,
    }
}

/// The Commits tab's column widths: the author (only when more than one
/// wrote them), the counts and the age.
struct CommitCols {
    who: usize,
    add: usize,
    del: usize,
    age: usize,
}

/// How long ago a commit was authored, for its column.
fn commit_age(commit: &crate::pull_request::PrCommit, now: i64) -> String {
    crate::pull_request::rfc3339_secs(&commit.at)
        .map(|at| crate::hosts::ago_short(now - at))
        .unwrap_or_default()
}

/// A Commits row on one line: the short sha, the subject, and in columns
/// at the right the author (when more than one wrote them), what it adds
/// and removes, and how long ago — the author going first, then the
/// counts, before the subject shortens past [`MIN_TEXT_W`].
fn commit_row(
    commit: &crate::pull_request::PrCommit,
    cols: &CommitCols,
    now: i64,
    width: usize,
    th: Theme,
) -> Vec<Span<'static>> {
    let lead = BODY.len() + commit.short().width() + 2;
    let counts_w = if cols.add + cols.del > 0 {
        cols.add + 1 + cols.del + 2
    } else {
        0
    };
    let who_w = if cols.who > 0 { cols.who + 2 } else { 0 };
    let fits = |right: usize| width >= lead + MIN_TEXT_W + right + 2;
    let (who_w, counts_w) = if fits(who_w + counts_w + cols.age) {
        (who_w, counts_w)
    } else if fits(counts_w + cols.age) {
        (0, counts_w)
    } else {
        (0, 0)
    };
    let left = vec![
        Span::raw(BODY),
        Span::styled(commit.short().to_string(), Style::default().fg(th.muted)),
        Span::raw("  "),
        Span::styled(commit.subject.clone(), Style::default().fg(th.text)),
    ];
    let dim = Style::default().fg(th.dim);
    let mut right = Vec::new();
    if who_w > 0 {
        let who = crate::ui::truncate(&commit.author, cols.who);
        right.push(Span::styled(format!("{who:<w$}  ", w = cols.who), dim));
    }
    if counts_w > 0 {
        let add = commit
            .additions
            .map(|n| count_text('+', n))
            .unwrap_or_default();
        let del = commit
            .deletions
            .map(|n| count_text('−', n))
            .unwrap_or_default();
        right.push(right_in(&add, cols.add, Style::default().fg(th.added)));
        right.push(Span::raw(" "));
        right.push(right_in(&del, cols.del, Style::default().fg(th.removed)));
        right.push(Span::raw("  "));
    }
    right.push(right_in(&commit_age(commit, now), cols.age, dim));
    columns(left, right, width)
}

/// A Checks row: its mark, its name, and at the right end how it went —
/// how long it ran, `running 2m`, or what GitHub called a failure that was
/// not a plain one (`timed out · 15m`) — in the mark's colour when it
/// failed or still runs.
fn check_row(check: &PrCheck, now: i64, width: usize, th: Theme) -> Vec<Span<'static>> {
    let (mark, color) = match check.state {
        CheckState::Failed => ("✗", th.err),
        CheckState::Running => ("◐", th.warn),
        CheckState::Passed => ("✓", th.ok),
        CheckState::Skipped => ("–", th.dim),
    };
    let word = check.word.to_lowercase().replace('_', " ");
    let tail = match (check.state, check.duration(now)) {
        (CheckState::Running, Some(so_far)) => format!("running {so_far}"),
        (CheckState::Running, None) => "queued".into(),
        (CheckState::Skipped, _) => word,
        (CheckState::Failed, Some(took)) if word != "failure" => format!("{word} · {took}"),
        (_, Some(took)) => took,
        (CheckState::Failed, None) => word,
        (CheckState::Passed, None) => String::new(),
    };
    let tail_color = match check.state {
        CheckState::Failed => th.err,
        CheckState::Running => th.warn,
        _ => th.dim,
    };
    let name_color = if check.state == CheckState::Skipped {
        th.dim
    } else {
        th.text
    };
    let left = vec![
        Span::raw(BODY),
        Span::styled(mark, Style::default().fg(color)),
        Span::styled(format!(" {}", check.name), Style::default().fg(name_color)),
    ];
    // The tail gives way before the name shortens past [`MIN_TEXT_W`].
    let tail_fits = width >= BODY.len() + 2 + MIN_TEXT_W + tail.width() + 2;
    let right = if tail.is_empty() || !tail_fits {
        Vec::new()
    } else {
        vec![Span::styled(tail, Style::default().fg(tail_color))]
    };
    columns(left, right, width)
}

/// A listing tab's lines as they are built: each row's line, the cursor's
/// dressed as the main page's list dresses its own.
struct Listing {
    lines: Vec<Line<'static>>,
    spans: Vec<(usize, usize)>,
    cursor: usize,
    focused: bool,
    width: usize,
    th: Theme,
}

impl Listing {
    /// One row — a line laid out to the width, led by [`BODY`] — the
    /// cursor's wearing `▌` in the accent and the FOCUSED PANEL TINT
    /// across the line while the page has the keys, a dim `▌` otherwise.
    fn row(&mut self, mut spans: Vec<Span<'static>>) {
        let index = self.spans.len();
        let first = self.lines.len();
        if index == self.cursor {
            let th = self.th;
            let mark = if self.focused { th.accent } else { th.dim };
            if let Some(lead) = spans.first_mut() {
                *lead = Span::styled("▌ ", Style::default().fg(mark));
            }
            if self.focused {
                let used = spans_w(&spans);
                spans.push(Span::raw(" ".repeat(self.width.saturating_sub(used))));
                for span in &mut spans {
                    span.style = span.style.bg(th.focus_tint);
                }
            }
        }
        self.lines.push(Line::from(spans));
        self.spans.push((first, self.lines.len() - first));
    }
}

/// The prose tabs' lines as they are built, and where the summaries of
/// the `<details>` in their GitHub bodies landed — the description is
/// body 0, `comments[i]` body `i + 1`, so a comment's FOLDS stay its own
/// on either tab.
struct Prose<'a> {
    lines: Vec<Line<'static>>,
    spots: Vec<FoldSpot>,
    folds: &'a Folds,
    body_w: usize,
    width: usize,
    th: Theme,
}

impl Prose<'_> {
    /// GitHub markdown behind the inset, its `<details>` as the reader
    /// left them.
    fn github(&mut self, text: &str, body: usize) {
        let (lines, folds) = markdown::render_folding(
            text,
            self.body_w,
            Style::default().fg(self.th.muted),
            self.th,
            &self.folds.flipped(body),
        );
        let at = self.lines.len();
        self.spots.extend(folds.into_iter().map(|f| {
            let key = FoldKey {
                body,
                index: f.index,
            };
            (f.rows.start + at..f.rows.end + at, key)
        }));
        self.lines.extend(markdown::indent(lines, BODY));
    }
}

/// The Description tab: the body as markdown, then the conversation under
/// a rule of its own.
fn description(p: &mut Prose, detail: &PrDetail, now: i64) {
    let th = p.th;
    if detail.body.trim().is_empty() {
        p.lines.push(Line::from(Span::styled(
            format!("{BODY}(no description)"),
            Style::default().fg(th.dim),
        )));
    } else {
        p.github(detail.body.trim_end(), 0);
    }
    if !detail.comments.is_empty() {
        p.lines.push(Line::from(""));
        p.lines.push(crate::ui::section_rule(
            "Conversation",
            th.muted,
            vec![Span::styled(
                detail.comments.len().to_string(),
                Style::default().fg(th.dim),
            )],
            p.width,
            th,
        ));
        for (i, c) in detail.comments.iter().enumerate() {
            p.lines.push(Line::from(""));
            comment_lines(p, c, i + 1, now);
        }
    }
}

/// The Reviews tab: GitHub's decision on a rule, each reviewer's latest
/// word in a column under it — and who has been asked and not answered —
/// then the reviews themselves under a rule of their own.
fn reviews(p: &mut Prose, detail: &PrDetail, now: i64) {
    let (th, width) = (p.th, p.width);
    let dim = Style::default().fg(th.dim);
    let out = &mut p.lines;
    let reviewers = detail.reviewers();
    let decision = match detail.review_decision.as_str() {
        "APPROVED" => ("Approved", th.ok),
        "CHANGES_REQUESTED" => ("Changes requested", th.err),
        "REVIEW_REQUIRED" => ("Review required", th.warn),
        _ => ("Reviewers", th.muted),
    };
    let count = match reviewers.len() {
        0 => Vec::new(),
        1 => vec![Span::styled("1 reviewer", dim)],
        n => vec![Span::styled(format!("{n} reviewers"), dim)],
    };
    out.push(crate::ui::section_rule(
        decision.0, decision.1, count, width, th,
    ));
    if reviewers.is_empty() {
        out.push(Line::from(Span::styled(
            format!("{BODY}No reviews yet"),
            dim,
        )));
    }
    let who_w = reviewers
        .iter()
        .map(|(who, _)| who.width())
        .max()
        .unwrap_or(0)
        .min(24);
    for (who, state) in &reviewers {
        let (mark, word, color) = match state.as_str() {
            "APPROVED" => ("✓", "approved", th.ok),
            "CHANGES_REQUESTED" => ("✗", "changes requested", th.err),
            "DISMISSED" => ("–", "dismissed", th.dim),
            REVIEW_REQUESTED => ("○", "review requested", th.muted),
            _ => ("·", "commented", th.dim),
        };
        let who = crate::ui::truncate(who, who_w);
        let spans = vec![
            Span::raw(BODY),
            Span::styled(mark, Style::default().fg(color)),
            Span::styled(format!(" {who:<who_w$}  "), Style::default().fg(th.text)),
            Span::styled(word, Style::default().fg(color)),
        ];
        out.push(fit(spans, width));
    }
    let said: Vec<(usize, &PrComment)> = detail
        .comments
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.review_state.is_empty())
        .collect();
    if !said.is_empty() {
        out.push(Line::from(""));
        out.push(crate::ui::section_rule(
            "Reviews",
            th.muted,
            vec![Span::styled(said.len().to_string(), dim)],
            width,
            th,
        ));
        for (i, c) in said {
            p.lines.push(Line::from(""));
            comment_lines(p, c, i + 1, now);
        }
    }
}

/// One comment: who said it, bold, and their verdict when it came as a
/// review — loud, in the colour its mark wears on the Reviews tab — with
/// how long ago at the right end; then its body rendered as markdown —
/// `body` on the page, for its FOLDS.
fn comment_lines(p: &mut Prose, c: &PrComment, body: usize, now: i64) {
    let (th, width) = (p.th, p.width);
    let mut left = vec![
        Span::raw(BODY),
        Span::styled(
            c.author.clone(),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(verdict) = c.verdict() {
        let color = match verdict {
            "approved" => th.ok,
            "changes requested" => th.err,
            _ => th.muted,
        };
        left.push(Span::styled(
            format!("  {verdict}"),
            Style::default().fg(color),
        ));
    }
    let age = crate::pull_request::rfc3339_secs(&c.at)
        .map(|at| crate::hosts::ago_short(now - at))
        .unwrap_or_default();
    p.lines.push(Line::from(columns(
        left,
        vec![Span::styled(age, Style::default().fg(th.dim))],
        width,
    )));
    if !c.body.trim().is_empty() {
        p.github(c.body.trim_end(), body);
    }
}

// ---- drawing ----

/// What [`draw`] left on screen: the body's rect, the scroll it settled
/// on, and how many lines the body holds — the caller's to write back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drawn {
    pub body: Rect,
    pub scroll: u16,
    pub lines: usize,
}

/// Draw `page` into `area`: the head fixed at the top, the body under it
/// from `scroll` — clamped, and moved just far enough to show the row
/// cursor when it has just moved. Records on `tabs` where the tabs, the
/// visible rows and the `<details>` summaries landed, for the mouse, and
/// what paging needs.
pub fn draw(f: &mut Frame, area: Rect, page: &Page, tabs: &mut PrTabs, scroll: u16) -> Drawn {
    let head_h = (page.head.len() as u16).min(area.height);
    f.render_widget(
        Paragraph::new(page.head.clone()),
        Rect {
            height: head_h,
            ..area
        },
    );
    let body = Rect {
        y: area.y + head_h,
        height: area.height - head_h,
        ..area
    };
    let view_h = usize::from(body.height.max(1));
    let max = page.body.len().saturating_sub(view_h);
    let mut top = usize::from(scroll).min(max);
    if tabs.follow {
        if let Some(&(first, n)) = page.spans.get(tabs.row()) {
            if first < top {
                top = first;
            } else if first + n > top + view_h {
                top = (first + n).saturating_sub(view_h).min(first);
            }
        }
        tabs.follow = false;
    }
    let shown: Vec<Line> = page.body.iter().skip(top).take(view_h).cloned().collect();
    f.render_widget(Paragraph::new(shown), body);

    tabs.tab_hits = page
        .tabs
        .iter()
        .filter(|(row, ..)| *row < head_h)
        .map(|&(row, from, to, tab)| {
            let from = from.min(area.width);
            (
                Rect {
                    x: area.x + from,
                    y: area.y + row,
                    width: to.min(area.width).saturating_sub(from),
                    height: 1,
                },
                tab,
            )
        })
        .collect();
    // The part of body lines `first..end` on screen, if any.
    let on_screen = |first: usize, end: usize| {
        let from = first.max(top);
        let to = end.min(top + usize::from(body.height));
        (from < to).then(|| Rect {
            x: body.x,
            y: body.y + (from - top) as u16,
            width: body.width,
            height: (to - from) as u16,
        })
    };
    tabs.row_hits = page
        .spans
        .iter()
        .enumerate()
        .filter_map(|(row, &(first, n))| on_screen(first, first + n).map(|rect| (rect, row)))
        .collect();
    tabs.fold_hits = page
        .folds
        .iter()
        .filter_map(|(rows, key)| on_screen(rows.start, rows.end).map(|rect| (rect, *key)))
        .collect();
    tabs.spans = page.spans.clone();
    tabs.view_h = body.height;
    Drawn {
        body,
        scroll: u16::try_from(top).unwrap_or(u16::MAX),
        lines: page.body.len(),
    }
}

// ---- acting on a row ----

/// What acting on the row under the cursor does: the DIFF VIEWER on the
/// pull request at a file, on one of its commits, or a check's page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAct {
    File(String),
    Commit { sha: String, subject: String },
    Check { name: String, url: String },
}

/// The act of the row under the cursor, on a tab that lists things.
pub fn row_act(detail: &PrDetail, tabs: &PrTabs) -> Option<RowAct> {
    let row = tabs.row();
    match tabs.tab {
        PrTab::Changes => detail.files.get(row).map(|f| RowAct::File(f.path.clone())),
        PrTab::Commits => detail.commits.get(row).map(|c| RowAct::Commit {
            sha: c.sha.clone(),
            subject: c.subject.clone(),
        }),
        PrTab::Checks => checks_in_order(detail)
            .into_iter()
            .flat_map(|(_, checks)| checks)
            .nth(row)
            .map(|c| RowAct::Check {
                name: c.name.clone(),
                url: c.url.clone(),
            }),
        PrTab::Description | PrTab::Reviews => None,
    }
}

/// Run `act` for pull request `number` (`url`, titled `label`): the DIFF
/// VIEWER opened through the pull request's own entry point — the one
/// `⌘E` and the modal's `^G` take — at the file, or on the commit's own
/// diff, fetched the same way; a check's page in the browser.
pub(crate) fn run_act(
    app: &mut crate::app::App,
    number: u64,
    url: &str,
    label: &str,
    act: RowAct,
    out: &mut Vec<orion_core::ClientRequest>,
) {
    match act {
        RowAct::File(path) => {
            app.pr_diff_at = Some((url.to_string(), path));
            crate::event_loop::request_pr_diff_for(app, number, url.to_string(), label.to_string());
        }
        // The commit from the project's own repo when it has it — under its
        // message — else fetched alone from GitHub (`open_pr_review`).
        RowAct::Commit { sha, .. } => crate::event_loop::open_pr_review(
            app,
            number,
            url.to_string(),
            label.to_string(),
            crate::event_loop::PrReviewAt::Commit(sha),
        ),
        // A check with no page of its own opens nothing.
        RowAct::Check { url, .. } if url.is_empty() => {}
        RowAct::Check { url, .. } => crate::event_loop::open_link(app, &url, out),
    }
}

/// The pull request the pane reads, with its body when it has landed.
fn pane_pr(app: &crate::app::App) -> Option<(crate::app::PreviewedPr, Option<PrDetail>)> {
    let pr = app.previewed_pr()?;
    let detail = app.pr_detail.get(&pr.url).cloned();
    Some((pr, detail))
}

/// A key while the pane holds the keys reading a pull request — asked
/// before the pane's own (`launcher::pane_key`): `Tab`/`⇧Tab` walk the
/// tabs; on a tab that lists things `↑`/`↓`, `PgUp`/`PgDn` and
/// `Home`/`End` walk its rows and Enter (`activate`) acts on the one
/// under the cursor. What it did, for the KEY COMBO DISPLAY; None for a
/// key it leaves to the pane — scrolling prose, Enter's browser.
pub(crate) fn pane_key(
    app: &mut crate::app::App,
    key: &crossterm::event::KeyEvent,
    activate: bool,
    out: &mut Vec<orion_core::ClientRequest>,
) -> Option<&'static str> {
    use crossterm::event::KeyCode;
    let (pr, detail) = pane_pr(app)?;
    if keys::NEXT_TAB.matches(key) || keys::PREV_TAB.matches(key) {
        let delta = if keys::NEXT_TAB.matches(key) { 1 } else { -1 };
        let tab = app.pr_tabs.tab.step(delta);
        app.pr_tabs.switch(tab);
        app.pr_preview_scroll = 0;
        app.dirty = true;
        return Some(if delta > 0 {
            "Next tab"
        } else {
            "Previous tab"
        });
    }
    if !app.pr_tabs.tab.lists() {
        return None;
    }
    if activate {
        let act = detail.as_ref().and_then(|d| row_act(d, &app.pr_tabs))?;
        run_act(app, pr.number, &pr.url, &pr.label, act, out);
        return Some("Open");
    }
    let nav = match key.code {
        KeyCode::Up => Nav::Line(-1),
        KeyCode::Down => Nav::Line(1),
        KeyCode::PageUp => Nav::Page(-1),
        KeyCode::PageDown => Nav::Page(1),
        KeyCode::Home => Nav::Top,
        KeyCode::End => Nav::Bottom,
        _ => return None,
    };
    if !key.modifiers.is_empty() {
        return None;
    }
    let max = app.pr_preview_max_scroll();
    let mut scroll = app.pr_preview_scroll;
    app.pr_tabs.navigate(nav, &mut scroll, max);
    app.pr_preview_scroll = scroll;
    app.dirty = true;
    Some("Move the cursor")
}

/// A click on one of the pane's tab labels: that tab.
pub(crate) fn click_tab(app: &mut crate::app::App, tab: PrTab) {
    if app.pr_tabs.switch(tab) {
        app.pr_preview_scroll = 0;
    }
    app.dirty = true;
}

/// A click on a `<details>` summary in the pane: it opens, or shuts — as
/// a click on one does on github.com.
pub(crate) fn click_fold(app: &mut crate::app::App, key: FoldKey) {
    app.pr_tabs.folds.toggle(key);
    app.dirty = true;
}

/// A click on a listed row in the pane: the cursor onto it, and what
/// Enter on it does — the mouse is another way to press the keys.
pub(crate) fn click_row(
    app: &mut crate::app::App,
    row: usize,
    out: &mut Vec<orion_core::ClientRequest>,
) {
    app.pr_tabs.select(row);
    app.dirty = true;
    let Some((pr, Some(detail))) = pane_pr(app) else {
        return;
    };
    if let Some(act) = row_act(&detail, &app.pr_tabs) {
        run_act(app, pr.number, &pr.url, &pr.label, act, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::{PrCommit, PrLabel};

    #[test]
    fn wrap_breaks_on_words_and_keeps_hard_breaks() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        // A blank line is a paragraph break and must survive.
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        // Nothing at all is still one row: the caller renders it.
        assert_eq!(wrap("", 10), [""]);
    }

    /// A word wider than the pane can't be allowed to overflow the rect —
    /// ratatui would clip it and the rest of the line with it.
    #[test]
    fn wrap_breaks_a_word_too_long_to_fit() {
        assert_eq!(
            wrap("https://example.dev/a/b", 8),
            ["https://", "example.", "dev/a/b"]
        );
        assert_eq!(
            wrap("hi https://example.dev", 8),
            ["hi", "https://", "example.", "dev"]
        );
        // Every row respects the budget, whatever the input.
        for row in wrap("supercalifragilistic and some ordinary words", 9) {
            assert!(row.chars().count() <= 9, "{row:?} is too wide");
        }
    }

    fn detail(body: &str, comments: Vec<PrComment>) -> PrDetail {
        PrDetail {
            number: 42,
            url: "https://github.com/o/r/pull/42".into(),
            title: "Attach links".into(),
            answered_state: "OPEN".into(),
            answered_draft: false,
            answered: Default::default(),
            author: "webdevcody".into(),
            base: "main".into(),
            head: "feat/links".into(),
            additions: 106,
            deletions: 4,
            changed_files: 2,
            body: body.into(),
            comments,
            ..Default::default()
        }
    }

    /// The fixture pull request's tabs filled in: two files, two commits,
    /// a failed, a running and a passed check, a requested change.
    fn full() -> PrDetail {
        let mut d = detail(
            "Makes the row.",
            vec![PrComment {
                author: "kate".into(),
                at: "2026-09-03T10:00:00Z".into(),
                review_state: "CHANGES_REQUESTED".into(),
                body: "one nit".into(),
            }],
        );
        d.files = vec![
            PrFile {
                path: "src/app.rs".into(),
                additions: 100,
                deletions: 4,
                change: "MODIFIED".into(),
            },
            PrFile {
                path: "src/links.rs".into(),
                additions: 6,
                deletions: 0,
                change: "ADDED".into(),
            },
        ];
        d.commits = vec![
            PrCommit {
                sha: "bbbbbbb2222".into(),
                subject: "Dedupe the PR row".into(),
                author: "webdevcody".into(),
                at: "2026-10-01T10:00:00Z".into(),
                additions: Some(12),
                deletions: Some(3),
                files: Some(1),
            },
            PrCommit {
                sha: "aaaaaaa1111".into(),
                subject: "Attach links".into(),
                author: "webdevcody".into(),
                at: "2026-09-30T10:00:00Z".into(),
                ..Default::default()
            },
        ];
        let check = |name: &str, state, word: &str| PrCheck {
            name: name.into(),
            workflow: "CI".into(),
            state,
            word: word.into(),
            started: "2026-10-02T10:00:00Z".into(),
            completed: if state == CheckState::Running {
                String::new()
            } else {
                "2026-10-02T10:03:12Z".into()
            },
            url: format!("https://github.com/o/r/actions/{name}"),
        };
        d.checks = vec![
            check("test", CheckState::Failed, "FAILURE"),
            check("lint", CheckState::Running, "IN_PROGRESS"),
            check("build", CheckState::Passed, "SUCCESS"),
        ];
        d.review_decision = "CHANGES_REQUESTED".into();
        d
    }

    const NOW: i64 = 1_791_000_000;

    fn input(detail: Option<&PrDetail>) -> PageInput<'_> {
        PageInput {
            number: 42,
            title: "Attach links",
            detail,
            status: detail.map(|d| {
                let mut prs = crate::pr_store::PrStore::default();
                prs.observe(
                    &d.url,
                    crate::pr_store::PrObservation::of_detail(d),
                    crate::fetch::Asked::Cached,
                );
                prs.status_or_open(&d.url)
            }),
            freshness: None,
            failed: false,
            posting: false,
            browser_key: "Enter".into(),
            diff_key: "^E".into(),
            now: NOW,
        }
    }

    fn text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn on(tab: PrTab) -> PrTabs {
        let mut tabs = PrTabs::default();
        tabs.switch(tab);
        tabs
    }

    /// The border says where it stands, the number and the title, and what
    /// it changes; the page under it leads with the sentence — who, from
    /// where into where, how many commits, how long ago, the labels at its
    /// right end — then the tabs, the verdict after Checks and Reviews,
    /// over the Description: the body, then the conversation on its rule.
    #[test]
    fn the_border_and_the_sentence_say_where_it_stands_then_the_tabs() {
        let mut d = full();
        d.created_at = "2026-09-30T04:00:00Z".into();
        d.labels = vec![PrLabel {
            name: "ui".into(),
            color: "1d76db".into(),
        }];
        let th = Theme::default();
        let (left, right) = border(&input(Some(&d)), false, th);
        assert_eq!(text(&[Line::from(left)]), "● Open  #42 Attach links");
        assert_eq!(text(&[Line::from(right)]), "+106 −4");
        let page = page(&input(Some(&d)), &PrTabs::default(), true, 100, th);
        let head = text(&page.head);
        let sentence = head.lines().nth(1).unwrap_or_default();
        assert!(
            sentence.starts_with(" webdevcody · feat/links → main · 2 commits · 3d ago"),
            "{head}"
        );
        assert!(
            sentence.ends_with("ui"),
            "the labels at the end: {sentence:?}"
        );
        assert_eq!(
            sentence.chars().count(),
            99,
            "a cell of air after: {sentence:?}"
        );
        assert!(
            head.contains(
                " Description   Changes 2   Commits 2   Checks ✗ 1/3   Reviews ✗ Changes requested"
            ),
            "{head}"
        );
        let body = text(&page.body);
        assert!(body.contains("Makes the row."), "{body}");
        assert!(body.contains(" ── Conversation ─"), "{body}");
        assert!(body.contains("  kate  changes requested"), "{body}");
        // Before the body lands the border has the number and the title only.
        let (left, right) = border(&input(None), true, th);
        assert_eq!(text(&[Line::from(left)]), "#42 Attach links");
        assert!(right.is_empty());
    }

    /// A sentence too long for the line gives the head branch up first, cut
    /// from its left, then the age, then the labels: always one line.
    #[test]
    fn the_sentence_keeps_to_one_line() {
        let mut d = full();
        d.head = "feat/a-branch-name-long-enough-to-crowd-the-line".into();
        d.created_at = "2026-09-30T04:00:00Z".into();
        d.labels = vec![PrLabel {
            name: "enhancement".into(),
            color: "a2eeef".into(),
        }];
        let th = Theme::default();
        let line = |w| text(&[sentence(&d, NOW, w, th)]);
        let roomy = line(120);
        assert!(
            roomy.contains(&d.head) && roomy.contains("3d ago"),
            "{roomy}"
        );
        let cut = line(80);
        assert!(
            cut.contains("…") && cut.contains("crowd-the-line → main"),
            "{cut}"
        );
        assert!(cut.contains("enhancement"), "{cut}");
        for w in 1..=120 {
            assert!(line(w).chars().count() <= w.max(1), "{w}: {:?}", line(w));
        }
        assert!(!line(40).contains("enhancement"), "{}", line(40));
    }

    /// The labels from `gh`'s JSON: counts, and the checks' and reviews'
    /// verdicts — failed, running, passing, none — and `…` while the body
    /// is on its way.
    #[test]
    fn tab_labels_count_and_mark_what_gh_said() {
        let th = Theme::default();
        assert_eq!(
            tab_texts(&input(None), th),
            [
                "Description",
                "Changes …",
                "Commits …",
                "Checks …",
                "Reviews"
            ],
            "loading"
        );
        let mut failed = input(None);
        failed.failed = true;
        assert_eq!(
            tab_texts(&failed, th),
            ["Description", "Changes", "Commits", "Checks", "Reviews"],
            "nothing to count"
        );
        let mut d = full();
        assert_eq!(
            tab_texts(&input(Some(&d)), th),
            [
                "Description",
                "Changes 2",
                "Commits 2",
                "Checks ✗ 1/3",
                "Reviews ✗ Changes requested"
            ]
        );
        d.checks.remove(0);
        d.review_decision = "REVIEW_REQUIRED".into();
        assert_eq!(
            tab_texts(&input(Some(&d)), th)[3..],
            ["Checks ◐ 1/2", "Reviews ○ Review required"]
        );
        d.checks.remove(0);
        d.review_decision = "APPROVED".into();
        assert_eq!(
            tab_texts(&input(Some(&d)), th)[3..],
            ["Checks ✓ 1/1", "Reviews ✓ Approved"]
        );
        d.checks.clear();
        d.review_decision.clear();
        assert_eq!(
            tab_texts(&input(Some(&d)), th)[3..],
            ["Checks", "Reviews · Reviewed"],
            "no checks, a review and no decision"
        );
        d.comments.clear();
        assert_eq!(tab_texts(&input(Some(&d)), th)[4], "Reviews", "no reviews");
        // The marks wear the verdict's color.
        let d = full();
        let page = page(&input(Some(&d)), &PrTabs::default(), true, 100, th);
        let cross = page
            .head
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains('✗'))
            .expect("a ✗");
        assert_eq!(cross.style.fg, Some(th.err));
    }

    /// Changes lists each file with its status and its + and − in columns
    /// under a rule with the whole diff's; Commits newest first, one line
    /// each — sha, subject, counts, age — under a rule naming who wrote
    /// them; Checks under a rule per workflow, failed first, how long each
    /// ran at the right; Reviews the decision on its rule, who stands where,
    /// then the reviews.
    #[test]
    fn each_tab_lists_its_own() {
        let d = full();
        let th = Theme::default();
        let body = |tab| text(&page(&input(Some(&d)), &on(tab), true, 100, th).body);
        let changes = body(PrTab::Changes);
        assert!(changes.contains(" ── 2 files ─"), "{changes}");
        assert!(changes.contains("+106 −4 ── "), "{changes}");
        assert!(changes.contains("▌ M  src/app.rs"), "{changes}");
        assert!(changes.contains("+100 −4 \n"), "{changes}");
        assert!(changes.contains("  A  src/links.rs"), "{changes}");
        let links = changes
            .lines()
            .find(|l| l.contains("src/links.rs"))
            .unwrap();
        assert!(
            links.ends_with(" +6 −0 "),
            "a zero is kept, dim, as the DIFF VIEWER keeps it: {links:?}"
        );
        let commits = body(PrTab::Commits);
        assert!(commits.contains(" ── 2 commits ─"), "{commits}");
        assert!(
            commits.contains("─ webdevcody ── "),
            "one author, on the rule: {commits}"
        );
        assert!(
            commits.contains("▌ bbbbbbb  Dedupe the PR row"),
            "{commits}"
        );
        assert!(commits.contains("+12 −3  1d \n"), "{commits}");
        assert!(
            commits.trim_end().ends_with("2d"),
            "no counts: none drawn — {commits}"
        );
        assert!(
            commits.find("bbbbbbb").unwrap() < commits.find("aaaaaaa").unwrap(),
            "newest first"
        );
        let checks = body(PrTab::Checks);
        assert!(checks.contains(" ── CI ─"), "{checks}");
        assert!(checks.contains(" 1/3 ── "), "{checks}");
        assert!(checks.contains("▌ ✗ test"), "{checks}");
        assert!(checks.contains("3m 12s \n"), "{checks}");
        assert!(checks.contains("  ◐ lint"), "{checks}");
        assert!(checks.contains("running"), "{checks}");
        assert!(checks.contains("  ✓ build"), "{checks}");
        let reviews = body(PrTab::Reviews);
        assert!(reviews.contains(" ── Changes requested ─"), "{reviews}");
        assert!(reviews.contains("  ✗ kate  changes requested"), "{reviews}");
        assert!(reviews.contains(" ── Reviews ─"), "{reviews}");
        assert!(reviews.contains("one nit"), "{reviews}");
    }

    /// Checks group by the workflow they ran in, the group with the worst
    /// check first, a check from no workflow under `Other` — and the row
    /// cursor walks them, and acts on them, in that order.
    #[test]
    fn checks_group_by_workflow_worst_first() {
        let mut d = full();
        for (name, workflow, state) in [
            ("pages", "Docs", CheckState::Passed),
            ("vercel", "", CheckState::Passed),
        ] {
            d.checks.push(PrCheck {
                name: name.into(),
                workflow: workflow.into(),
                state,
                word: "SUCCESS".into(),
                started: String::new(),
                completed: String::new(),
                url: format!("https://ci/{name}"),
            });
        }
        d.checks.rotate_left(3);
        let groups: Vec<(String, Vec<&str>)> = checks_in_order(&d)
            .into_iter()
            .map(|(wf, checks)| (wf, checks.iter().map(|c| c.name.as_str()).collect()))
            .collect();
        assert_eq!(
            groups,
            [
                ("CI".to_string(), vec!["test", "lint", "build"]),
                ("Docs".to_string(), vec!["pages"]),
                ("Other".to_string(), vec!["vercel"]),
            ]
        );
        let mut tabs = on(PrTab::Checks);
        tabs.select(3);
        assert_eq!(
            row_act(&d, &tabs),
            Some(RowAct::Check {
                name: "pages".into(),
                url: "https://ci/pages".into()
            })
        );
        let out = text(&page(&input(Some(&d)), &tabs, true, 80, Theme::default()).body);
        assert!(out.contains("▌ ✓ pages"), "{out}");
        assert!(
            out.find("── CI").unwrap() < out.find("── Docs").unwrap(),
            "{out}"
        );
    }

    /// A list GitHub cut short says so, and names the key that reads the
    /// whole diff.
    #[test]
    fn a_cut_short_file_list_says_so() {
        let mut d = full();
        d.changed_files = 240;
        let out = text(
            &page(
                &input(Some(&d)),
                &on(PrTab::Changes),
                true,
                100,
                Theme::default(),
            )
            .body,
        );
        assert!(
            out.contains("the first 2 of 240 files — ^E reads every one"),
            "{out}"
        );
    }

    /// An empty description says so rather than rendering a silent gap that
    /// reads as "still loading" — and a pull request still loading or that
    /// couldn't be read says that.
    #[test]
    fn the_body_says_what_it_has_not_got() {
        let th = Theme::default();
        let empty = detail("   \n", vec![]);
        let out = text(&page(&input(Some(&empty)), &PrTabs::default(), true, 60, th).body);
        assert!(out.contains("(no description)"), "{out}");
        assert!(!out.contains("── "), "no conversation rule: {out}");
        let out = text(&page(&input(None), &on(PrTab::Checks), true, 60, th).body);
        assert!(out.contains("reading it…"), "{out}");
        let mut failed = input(None);
        failed.failed = true;
        let out = text(&page(&failed, &PrTabs::default(), true, 60, th).body);
        assert!(out.contains("couldn't read this pull request"), "{out}");
        assert!(out.contains("Enter still opens it in the browser"), "{out}");
        let out = text(&page(&input(Some(&empty)), &on(PrTab::Checks), true, 60, th).body);
        assert!(out.contains("no checks ran"), "{out}");
    }

    /// The border's state word: open, a draft, a conflict in red while it
    /// is open — a merged one is past it — merged and closed.
    #[test]
    fn the_border_names_where_it_stands() {
        let th = Theme::default();
        let state = |d: &PrDetail| {
            let (left, _) = border(&input(Some(d)), false, th);
            (left[0].content.to_string(), left[0].style.fg)
        };
        let mut d = detail("", vec![]);
        assert_eq!(state(&d), ("● Open".into(), Some(th.ok)));
        d.answered_draft = true;
        assert_eq!(state(&d), ("○ Draft".into(), Some(th.muted)));
        d.answered.conflicts = Some(true);
        assert_eq!(state(&d), ("● Conflicts".into(), Some(th.err)));
        d.answered_state = "MERGED".into();
        assert_eq!(state(&d), ("● Merged".into(), Some(th.merged)));
        d.answered_state = "CLOSED".into();
        assert_eq!(state(&d), ("● Closed".into(), Some(th.faint)));
    }

    /// Every rendered row has to fit the pane — head, tabs and every tab's
    /// body — or ratatui clips it: everything wraps instead.
    #[test]
    fn no_rendered_line_overflows_the_pane() {
        let mut d = full();
        d.body = "A description with a very long unbroken token: \
                  https://github.com/oliverkidd/orion/pull/12345/files#diff-abcdef"
            .into();
        d.title = "A title long enough that a narrow pane has to wrap it twice over".into();
        d.files[0].path = "crates/orion-tui/src/a/very/deep/path/to/a/file/name.rs".into();
        for w in [24usize, 40, 80] {
            for tab in PrTab::ALL {
                let page = page(&input(Some(&d)), &on(tab), true, w, Theme::default());
                for line in page.head.iter().chain(page.body.iter()) {
                    let len: usize = line.spans.iter().map(|s| s.content.width()).sum();
                    assert!(len <= w, "width {w}, {tab:?}: {len} cols in {line:?}");
                }
            }
        }
        // A narrow pane wraps the tab row rather than cutting a tab off.
        let narrow = page(
            &input(Some(&d)),
            &PrTabs::default(),
            true,
            30,
            Theme::default(),
        );
        let tab_rows = narrow
            .tabs
            .iter()
            .map(|(row, ..)| *row)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(tab_rows.len() > 1, "{:?}", narrow.tabs);
        assert!(
            text(&narrow.head).contains("Reviews ✗"),
            "{}",
            text(&narrow.head)
        );
    }

    /// The tabs step round both ends; a listing tab's rows are walked by
    /// line and by page, Home and End go to the ends — and on prose the
    /// same moves scroll.
    #[test]
    fn navigation_walks_rows_on_a_listing_and_scrolls_prose() {
        for key in keys::ALL {
            assert!(key.parses(), "{:?}", key.chords);
        }
        assert_eq!(keys::TABS.label(), "Tab/⇧Tab");
        assert_eq!(keys::MODAL_TABS.label(), "⇧←/⇧→");
        assert_eq!(PrTab::Description.step(-1), PrTab::Reviews);
        assert_eq!(PrTab::Reviews.step(1), PrTab::Description);
        assert_eq!(PrTab::Changes.step(2), PrTab::Checks);

        let mut tabs = on(PrTab::Commits);
        tabs.spans = vec![(1, 2), (3, 2), (5, 2), (7, 2), (9, 2)];
        tabs.view_h = 4;
        let mut scroll = 0;
        tabs.navigate(Nav::Line(1), &mut scroll, 10);
        assert_eq!(tabs.row(), 1);
        tabs.navigate(Nav::Line(-5), &mut scroll, 10);
        assert_eq!(tabs.row(), 0, "held at the top");
        tabs.navigate(Nav::Page(1), &mut scroll, 10);
        assert_eq!(tabs.row(), 2, "a body's height further");
        tabs.navigate(Nav::Bottom, &mut scroll, 10);
        assert_eq!((tabs.row(), scroll), (4, 10));
        tabs.navigate(Nav::Page(-1), &mut scroll, 10);
        assert_eq!(tabs.row(), 2);
        tabs.navigate(Nav::Top, &mut scroll, 10);
        assert_eq!((tabs.row(), scroll), (0, 0));
        assert_eq!(scroll, 0, "rows move, the draw follows");

        let mut prose = on(PrTab::Description);
        prose.view_h = 5;
        prose.navigate(Nav::Line(1), &mut scroll, 12);
        assert_eq!(scroll, 1);
        prose.navigate(Nav::Page(1), &mut scroll, 12);
        assert_eq!(scroll, 6);
        prose.navigate(Nav::Bottom, &mut scroll, 12);
        assert_eq!(scroll, 12);

        // Another pull request: rows rewind, the tab stays.
        tabs.select(3);
        tabs.rewind();
        assert_eq!((tabs.tab, tabs.row()), (PrTab::Commits, 0));
    }

    /// The row under the cursor acts by its tab: a file's diff, a
    /// commit's, a check's page — and nothing on the tabs of prose.
    #[test]
    fn the_row_under_the_cursor_acts_by_its_tab() {
        let d = full();
        let mut tabs = on(PrTab::Changes);
        tabs.select(1);
        assert_eq!(
            row_act(&d, &tabs),
            Some(RowAct::File("src/links.rs".into()))
        );
        tabs.switch(PrTab::Commits);
        assert_eq!(
            row_act(&d, &tabs),
            Some(RowAct::Commit {
                sha: "bbbbbbb2222".into(),
                subject: "Dedupe the PR row".into()
            })
        );
        tabs.switch(PrTab::Checks);
        tabs.select(2);
        assert_eq!(
            row_act(&d, &tabs),
            Some(RowAct::Check {
                name: "build".into(),
                url: "https://github.com/o/r/actions/build".into()
            })
        );
        tabs.switch(PrTab::Reviews);
        assert_eq!(row_act(&d, &tabs), None);
    }

    /// Drawn into a rect: the head stays put while the body scrolls, the
    /// cursor's row is scrolled into view when it moves, and the tabs and
    /// the visible rows are where the mouse finds them.
    #[test]
    fn the_draw_keeps_the_head_and_follows_the_cursor() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut d = full();
        for i in 0..30 {
            d.files.push(PrFile {
                path: format!("src/f{i}.rs"),
                additions: 1,
                deletions: 1,
                change: "MODIFIED".into(),
            });
        }
        d.changed_files = 32;
        let th = Theme::default();
        let mut tabs = on(PrTab::Changes);
        let area = Rect::new(0, 0, 60, 14);
        let mut term = Terminal::new(TestBackend::new(60, 14)).unwrap();
        let mut redraw = |tabs: &mut PrTabs, scroll| {
            let page = page(&input(Some(&d)), tabs, true, 60, th);
            let mut drawn = None;
            term.draw(|f| drawn = Some(draw(f, area, &page, tabs, scroll)))
                .unwrap();
            let buf = term.backend().buffer().clone();
            let screen = (0..14)
                .map(|y| {
                    (0..60)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            (screen, drawn.unwrap())
        };
        let (screen, first) = redraw(&mut tabs, 0);
        let sentence = " webdevcody · feat/links → main · 2 commits";
        assert!(
            screen.lines().nth(1).unwrap().starts_with(sentence),
            "{screen}"
        );
        assert_eq!(first.lines, 2 + 32, "the blank, the rule, the rows");
        assert!(first.body.y > 3, "the head sits above the body");
        let (rect, tab) = tabs.tab_hits[1];
        assert_eq!(tab, PrTab::Changes);
        assert!(rect.y + 1 < first.body.y, "the tabs sit over the rule");
        assert!(
            screen
                .lines()
                .nth(usize::from(rect.y))
                .unwrap()
                .contains("Changes 32"),
            "{screen}"
        );
        assert_eq!(
            tabs.row_hits[0].0.y,
            first.body.y + 2,
            "row 0 under the blank and the rule"
        );

        tabs.select(25);
        let (screen, followed) = redraw(&mut tabs, 0);
        assert!(followed.scroll > 0, "scrolled to the cursor");
        assert!(screen.contains("▌ M  src/f23.rs"), "{screen}");
        assert!(
            screen.lines().nth(1).unwrap().starts_with(sentence),
            "the head stays: {screen}"
        );
        assert!(tabs.row_hits.iter().any(|(_, row)| *row == 25));
    }
}
