//! The PULL REQUESTS MODAL: the selected project's open pull requests,
//! listed down the left in the PROJECT OPEN PRS GROUP's order — newest
//! first, the drafts sunk below the finished ones — gathered into
//! sections (yours, the ones waiting on your review, the rest), each row's
//! author, age, checks, review and labels on a line under its title, and
//! the one under the cursor read on the right, as the PULL REQUEST PAGE.
//! `⌘F` and `key:value` tokens in the filter narrow by facet
//! (`list_filter`). Two panels, and
//! `Tab` / `⇧Tab` hand the keys from one to the other as they do in the
//! DIFF VIEWER ([`PrFocus`]): the list's keys walk the pull requests and
//! type into its filter, the page's walk its tabs (`←`/`→`) and their rows
//! (`↑`/`↓`), and Enter acts on the row — the DIFF VIEWER at a file or a
//! commit, a check's page.
//!
//! `Enter` on the list puts an agent on the pull request: the QUICK
//! PROMPT for a PR SESSION, launched exactly as the group's row launches
//! it (`quick_prompt::pr_launch_for`) — the box's own `Tab` and `⇧Tab`
//! pick another harness or an AGENT PRESET. The create is a
//! `CreatePrAgent`: the DAEMON runs the session in the project's checkout
//! of the pull request's head branch — reused when one is there, cut
//! otherwise, its stand-in rows up under the pull request from the moment
//! Enter is pressed — and the PR's URL rides the harness's context.
//!
//! The list's filter is live from the moment the modal opens, as the
//! DIFF VIEWER's and the FILE FINDER's are: every letter typed narrows
//! the rows to the fuzzy matches of `#42 title` (`fuzzy::rank`), best
//! first, the cursor on the best — a letter typed on the page hands the
//! keys back to the list first — and Esc clears it before a second Esc
//! closes (Esc on the page goes back to the list). So the verbs are
//! chords: `⌘Y` leaves a comment (the COMMENT BOX the
//! group row's `y` opens, which comes back to the modal on its row),
//! `⌘E` reads the diff, `⌘O` opens the pull request in the
//! browser, `⌘R` asks GitHub again, `⌘N` opens a new pull request,
//! `⌘X` merges this one and `⌘W` closes it (`pr_actions`, each
//! form in the reading pane's place), and `⌘D` marks a draft ready for
//! review or a ready one a draft again, and `⌘⇧R` reviews it — approve,
//! request changes or comment (Enter on the Reviews tab opens the same
//! form). Each verb is one ⌘ chord — the
//! grid's letter where the grid does the same thing (`⌘E` changes, `⌘R`
//! refresh, `⌘O` open outside, `⌘N` new, `⌘W` close) — with its `^` twin
//! for a terminal that sends no ⌘.
//!
//! `⌘L` flips to the LINEAR VIEW to attach the pull request under the
//! cursor to the issues marked there. The way back is the PR PICK
//! ([`PrPick`]): the LINEAR VIEW's `⌘U` opens this modal carrying the
//! issues it marked, and here `Enter` on the list attaches the pull
//! request to them — the very ATTACH the other way runs
//! (`linear::attach_issues`) — and goes back to the LINEAR VIEW, while
//! Esc goes back to it as it was, marks and all.
//!
//! The DIFF VIEWER opened from here is a level inside the modal: it takes
//! the modal's frame, its title says which pull request it is reading,
//! and Esc comes back to the modal on the same row and tab
//! (`DiffView::back`).
//!
//! Nothing is fetched here the panels do not already keep. The rows are
//! the project's open list (`App::open_prs`) — kept warm on the OPEN PRS
//! beat and remembered across launches (`pr_cache`) — so the modal paints
//! at once, and opening it on a list older than [`FRESH`] asks again
//! underneath. The reading side is the PULL REQUEST PAGE the pane shows
//! (`pr_preview`) — fetched on the pane's debounce into the same
//! `App::pr_detail`, so a pull request read in one is read in the other.
//! A page is read again once it is older than `event_loop::PR_DETAIL_FRESH`
//! — on the cursor's next rest, and on the list's beat while the cursor
//! stays — so its checks keep up with CI. Opening the modal reads every one
//! of your own pull requests' pages at once, and the rest one at a time
//! behind them ([`Prefetch`]), so walking the list finds them read.
//! From the list its tabs are walked with `⇧←`/`⇧→` and a listing's rows
//! with `⇧↑`/`⇧↓`, without moving the keys.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use orion_core::{ClientRequest, ProjectId};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::app::{clamp_selection, App, HitTarget, Overlay, PendingPrDetail, PromptKind};
use crate::list_filter::{FacetKey, FilterPick, PickFacet, PickValue};
use crate::pr_preview::{Nav, PrTab};
use crate::pr_store::{PrStatus, PrStore};
use crate::pull_request::{Checks, OpenPr, PrSection, Review};
use crate::quick_prompt::{ModalUnder, QuickLaunch};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, empty_list_row, fuzzy_highlight_styled, layout_sections_spaced, panel_block,
    row_rect, search_line_lit, sections, truncate, ListEntry, SPLIT_MODAL_PCT,
    SPLIT_PANE_LAYOUT_MIN,
};

/// A list younger than this is what opening the modal shows, with no
/// second ask — the ISSUES MODAL's window, for the same reason: the list
/// the OPEN PRS beat landed moments ago *is* the answer. `r` asks
/// regardless.
pub(crate) const FRESH: std::time::Duration = crate::issues::FRESH;
/// How long the cursor rests on a row before its body and conversation
/// are fetched — the pane's own debounce, so walking the list with `j`
/// fetches only the rows actually paused on.
pub(crate) const DETAIL_DEBOUNCE: std::time::Duration = crate::event_loop::PR_DETAIL_DEBOUNCE;
/// The list column's share of the modal, and its floor. The ISSUES MODAL
/// is laid out the same.
pub(crate) const LIST_PCT: u16 = 38;
pub(crate) const MIN_LIST_W: u16 = 24;
/// Lines one wheel notch scrolls the reading pane.
pub(crate) const WHEEL_LINES: i32 = 3;
/// How many pages the prefetch lets be in flight at once, counting the
/// cursor's own: enough that a handful of your pull requests land
/// together, few enough not to stampede `gh`.
pub(crate) const PREFETCH_PARALLEL: usize = 3;
/// The gap between the prefetch's asks for the pull requests that aren't
/// yours: the list fills in underneath without a burst.
pub(crate) const PREFETCH_GAP: std::time::Duration = std::time::Duration::from_secs(1);

/// The pages the modal reads ahead of the cursor. `soon` holds your own
/// pull requests, asked for the moment there is room in flight; `later`
/// the rest, one per [`PREFETCH_GAP`]. Each is checked again as it is
/// taken, so one the cursor read meanwhile costs nothing. It lives on the
/// view, so closing the modal drops whatever was still queued.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prefetch {
    pub soon: std::collections::VecDeque<PendingPrDetail>,
    pub later: std::collections::VecDeque<PendingPrDetail>,
    /// When the next of `later` may go; None until one has.
    pub next: Option<std::time::Instant>,
}

impl Prefetch {
    fn is_empty(&self) -> bool {
        self.soon.is_empty() && self.later.is_empty()
    }

    fn holds(&self, url: &str) -> bool {
        self.soon.iter().chain(&self.later).any(|p| p.url == url)
    }
}

/// Which panel of the modal has the keys — the one wearing the accent.
/// `Tab` and `⇧Tab` hand them across, as the DIFF VIEWER's panels do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrFocus {
    /// `↑`/`↓` walk the pull requests; typing filters them.
    #[default]
    List,
    /// The PULL REQUEST PAGE: `←`/`→` its tabs, `↑`/`↓` a listing's rows
    /// or the prose, Enter the row under the cursor.
    Page,
}

impl PrFocus {
    /// Two panels: `Tab` and `⇧Tab` both land on the other one.
    pub fn other(self) -> Self {
        match self {
            PrFocus::List => PrFocus::Page,
            PrFocus::Page => PrFocus::List,
        }
    }
}

/// The PR PICK: the modal opened from the LINEAR VIEW's `⌘U` to choose
/// the pull request its marked issues are attached to — the mirror of the
/// LINEAR VIEW in `LinearMode::Attach`, which the modal's own `⌘L` opens
/// to choose the issues for a pull request. `Enter` on the list attaches
/// rather than prompting an agent, and `⌘L` has nowhere further to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrPick {
    /// The issues the chosen pull request is attached to.
    pub issues: Vec<crate::linear::LinearIssue>,
    /// The LINEAR VIEW it came from, as it was — marks, filter and cursor:
    /// where Esc and the attach both land.
    pub back: Box<crate::linear::LinearView>,
}

impl PrPick {
    /// `ENG-12, ENG-15`: the issues, by identifier, for the title.
    pub fn ids(&self) -> String {
        crate::linear::ids_of(&self.issues)
    }
}

/// The modal's own state. The rows live on the [`App`] (`open_prs`, keyed
/// by project), where the panels read them too; this holds only the
/// cursor, the reading pane's scroll, and the rects the mouse hit-tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestsView {
    pub project: ProjectId,
    /// The project's row name, for the list's title.
    pub project_name: String,
    /// The checkout `gh` runs from.
    pub dir: PathBuf,
    /// Cursor into the project's list.
    pub selected: usize,
    /// The pull request under the cursor, by URL: a refresh that reorders
    /// the list (a draft marked ready rises above the drafts) keeps the
    /// cursor on it, and one that retired it lands on its neighbour
    /// ([`list_changed`]).
    pub selected_url: Option<String>,
    /// Where the cursor sits among the rows as drawn — sections and the
    /// filter applied — as of the last move or draw: a refresh that
    /// retires the cursor's pull request lands on the row that took this
    /// place on screen, not the one that took its index in GitHub's order
    /// ([`list_changed`]).
    pub selected_row: usize,
    /// Top visible line of the reading side's body, under its head.
    pub scroll: u16,
    /// The reading side's body height and total line count as of the last
    /// draw, for paging and clamping.
    pub view_height: u16,
    pub body_lines: usize,
    /// The PULL REQUEST PAGE's tab and row cursor, and where its tabs and
    /// rows landed for the mouse.
    pub tabs: crate::pr_preview::PrTabs,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// The list rows and the reading pane, for wheel and click routing.
    pub list_area: Rect,
    pub body_area: Rect,
    /// The `↗ open in browser` BUTTON on the reading pane's top border
    /// (`ui::browser_button`), for the click and the pointer resting on
    /// it; `Rect::default()` — no point inside — while there is none.
    pub browser_area: Rect,
    /// The list's live filter: the rows narrow to the fuzzy matches of
    /// `#42 title`, best first ([`visible_rows`]), as every letter lands,
    /// and to the `key:value` tokens among the words ([`FACETS`]). Empty
    /// shows every row in the list's order, section by section.
    pub query: TextInput,
    /// The first list entry drawn — a row or a section header — as of the
    /// last draw: where the next draw's window starts from, so it moves
    /// only as far as the cursor makes it (`ui::stacked_rows`).
    pub list_start: usize,
    /// Each drawn row's rect, by its pull request's URL, as of the last
    /// draw: what a click hit-tests, rows being two lines tall and headers
    /// between them. By URL, not index, so a list that landed since the
    /// draw cannot turn a click into its neighbour.
    pub row_rects: Vec<(String, Rect)>,
    /// The FILTER PICK (`⌘F`) in the page's place, while it is up: every
    /// key is its own (`list_filter`).
    pub filter_pick: Option<FilterPick>,
    /// The last click on a row — when, and which pull request — so a
    /// second click on the same row inside the DOUBLE-CLICK window opens it
    /// in the browser (`event_loop::is_double_click`).
    pub last_row_click: Option<(std::time::Instant, u64)>,
    /// The panel with the keys.
    pub focus: PrFocus,
    /// A form in the reading pane's place — a new pull request, or the
    /// merge of the one under the cursor (`pr_actions`). While it is up
    /// every key is its own.
    pub form: Option<Box<crate::pr_actions::PrForm>>,
    /// The PR PICK, while the modal was opened from the LINEAR VIEW to
    /// choose a pull request for its marked issues; None opened on its
    /// own.
    pub pick: Option<PrPick>,
    /// The pages read ahead of the cursor.
    pub prefetch: Prefetch,
}

impl PullRequestsView {
    pub fn new(project: ProjectId, project_name: String, dir: PathBuf) -> Self {
        Self {
            project,
            project_name,
            dir,
            selected: 0,
            selected_url: None,
            selected_row: 0,
            scroll: 0,
            view_height: 0,
            body_lines: 0,
            tabs: Default::default(),
            area: Rect::default(),
            list_area: Rect::default(),
            body_area: Rect::default(),
            browser_area: Rect::default(),
            query: TextInput::new(),
            list_start: 0,
            row_rects: Vec::new(),
            filter_pick: None,
            last_row_click: None,
            focus: PrFocus::List,
            form: None,
            pick: None,
            prefetch: Prefetch::default(),
        }
    }

    pub fn max_scroll(&self) -> u16 {
        crate::app::max_scroll(self.body_lines, self.view_height)
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = crate::app::scrolled_by(self.scroll, delta, self.max_scroll());
    }
}

// ---- opening, fetching, following ----

/// The hotkey: the PULL REQUESTS MODAL for the selected PROJECT. Every
/// panel has one selected, so this works from any row; only a machine
/// with no project has nothing to list. The cursor starts on the pull request
/// the Worktrees cursor rests on, when it rests on one — the row the user
/// was already reading.
pub(crate) fn open(app: &mut App) {
    let Some(project) = app.selected_project().cloned() else {
        return;
    };
    show(
        app,
        PullRequestsView::new(project.id, project.name, project.repo_path),
    );
}

/// The LINEAR VIEW's `⌘U`: the modal for the LINEAR VIEW's project, as a
/// PR PICK for the issues it marked. It opens as the hotkey's does — on
/// the Worktrees cursor's pull request, a stale list asked for again.
pub(crate) fn open_pick(app: &mut App, pick: PrPick) {
    let mut view = PullRequestsView::new(
        pick.back.project.clone(),
        pick.back.project_name.clone(),
        pick.back.dir.clone(),
    );
    view.pick = Some(pick);
    show(app, view);
}

/// Put `view` up on the pull request the Worktrees cursor rests on, when
/// it rests on one of the project's — the row the user was already
/// reading — else the first.
fn show(app: &mut App, mut view: PullRequestsView) {
    let project = view.project.clone();
    let list = rows(app, &project);
    let start = app
        .selected_worktree_pr()
        .and_then(|pr| list.iter().position(|row| row.url == pr.url))
        .or_else(|| visible_rows("", list, &app.prs).first().map(|(i, _)| *i))
        .unwrap_or(0);
    view.selected = clamp_selection(start as i64, list.len());
    view.selected_url = list.get(view.selected).map(|pr| pr.url.clone());
    app.overlay = Some(Overlay::PullRequests(view));
    // A list the beat landed moments ago is the answer; an older one
    // paints now while a fresh copy lands underneath.
    if !is_fresh(app, &project) {
        request_list(app, &project);
    }
    schedule_detail(app);
    queue_prefetch(app, true);
    app.dirty = true;
}

/// Queue the list's pages for the prefetch, in the order the list shows
/// them. Opening (`opening`) asks for each of yours that isn't fresh —
/// what you came to look at — and every other one this session hasn't
/// read; a list landing later queues only what is still unread, so the
/// beat never turns into a sweep of every page.
fn queue_prefetch(app: &mut App, opening: bool) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let mut soon = Vec::new();
    let mut later = Vec::new();
    for (i, _) in visible_rows("", list, &app.prs) {
        let pr = &list[i];
        if view.prefetch.holds(&pr.url) {
            continue;
        }
        let pending = PendingPrDetail {
            url: pr.url.clone(),
            number: pr.number,
            dir: view.dir.clone(),
        };
        let unread = app.pr_detail_unread(&pr.url);
        if pr.mine && (unread || opening && app.pr_detail_owed(&pr.url)) {
            soon.push(pending);
        } else if unread {
            later.push(pending);
        }
    }
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    view.prefetch.soon.extend(soon);
    view.prefetch.later.extend(later);
}

/// When the prefetch next has a page to ask for: now for one of yours,
/// at the gap for the rest, never while [`PREFETCH_PARALLEL`] are in
/// flight (a landing runs the loop again, which asks again).
pub(crate) fn prefetch_delay(app: &App) -> Option<std::time::Duration> {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return None;
    };
    let queue = &view.prefetch;
    if queue.is_empty() || app.pr_detail_inflight.len() >= PREFETCH_PARALLEL {
        return None;
    }
    if !queue.soon.is_empty() {
        return Some(std::time::Duration::ZERO);
    }
    let wait = queue
        .next
        .map(|next| next.saturating_duration_since(std::time::Instant::now()));
    Some(wait.unwrap_or_default())
}

/// The pages due now, taken off the queue: as many of yours as there is
/// room for in flight, then one of the rest if its gap has passed. A
/// page read since it was queued is dropped, not asked for again.
pub(crate) fn take_prefetch(app: &mut App) -> Vec<PendingPrDetail> {
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return Vec::new();
    };
    let mut queue = std::mem::take(&mut view.prefetch);
    let mut room = PREFETCH_PARALLEL.saturating_sub(app.pr_detail_inflight.len());
    let mut due = Vec::new();
    while room > 0 {
        let Some(pending) = queue.soon.pop_front() else {
            break;
        };
        if app.pr_detail_owed(&pending.url) {
            due.push(pending);
            room -= 1;
        }
    }
    let now = std::time::Instant::now();
    if room > 0 && queue.soon.is_empty() && queue.next.is_none_or(|next| now >= next) {
        while let Some(pending) = queue.later.pop_front() {
            if app.pr_detail_unread(&pending.url) {
                due.push(pending);
                queue.next = Some(now + PREFETCH_GAP);
                break;
            }
        }
    }
    if let Some(Overlay::PullRequests(view)) = &mut app.overlay {
        view.prefetch = queue;
    }
    due
}

/// Esc out of the PR PICK: the LINEAR VIEW back as it was, marks and
/// all, with nothing attached — `linear::close`'s way back, the other way.
fn back_to_linear(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = app.overlay.take() else {
        return;
    };
    rearm_pane_detail(app);
    if let Some(pick) = view.pick {
        crate::linear::reopen(app, *pick.back);
    }
}

/// `Enter` on the PR PICK's list: the pull request under the cursor
/// attached to the issues the LINEAR VIEW marked, through the very ATTACH
/// that view runs for a pull request (`linear::attach_issues`) — the
/// footer spins, then says what Linear took. The LINEAR VIEW comes back
/// with its marks spent, the batch done, as a launch spends them; the
/// filter and the cursor are where they were.
fn attach_to_selected(app: &mut App) {
    let Some(pr) = selected_pr(app) else {
        return;
    };
    let Some(Overlay::PullRequests(view)) = app.overlay.take() else {
        return;
    };
    let Some(pick) = view.pick else {
        app.overlay = Some(Overlay::PullRequests(view));
        return;
    };
    rearm_pane_detail(app);
    crate::linear::attach_issues(app, view.dir, pr.url, pr.number, &pick.issues);
    let mut back = *pick.back;
    back.marked.clear();
    crate::linear::reopen(app, back);
}

/// Close the modal. The pane behind reads the Worktrees cursor's pull
/// request again ([`rearm_pane_detail`]).
fn close(app: &mut App) {
    app.overlay = None;
    rearm_pane_detail(app);
}

/// The modal is gone — closed, or handed back to the LINEAR VIEW — and
/// the pane behind reads the Worktrees cursor's pull request again. The
/// modal's cursor may have taken over the fetch it was waiting on, so that
/// one is armed again, without touching the pane's scroll.
fn rearm_pane_detail(app: &mut App) {
    let pending = app.previewed_pr().and_then(|pr| {
        let dir = app.selected_project()?.repo_path.clone();
        pending_for(app, pr.url, pr.number, dir)
    });
    app.pending_pr_detail = pending.map(|p| (p, std::time::Instant::now() + DETAIL_DEBOUNCE));
}

/// Put the modal back as it was — the COMMENT BOX stood in for it — on the
/// same pull request, followed by URL in case the list moved underneath.
pub(crate) fn reopen(app: &mut App, view: PullRequestsView) {
    app.overlay = Some(Overlay::PullRequests(view));
    list_changed(app);
    schedule_detail(app);
    app.dirty = true;
}

/// The project's open pull requests, as the group shows them (drafts and
/// all: the modal lists everything open, whatever `hide_draft_prs` keeps
/// out of the panel).
fn rows<'a>(app: &'a App, project: &ProjectId) -> &'a [OpenPr] {
    app.open_prs
        .get(project)
        .map_or(&[], |open| open.list.as_slice())
}

/// Is there a filter to apply — words beyond whitespace, or a token?
fn has_query(view: &PullRequestsView) -> bool {
    crate::list_filter::parse(&view.query, FACETS).is_active()
}

/// The keys the filter line takes besides its words (`list_filter`):
/// `author:sam label:bug review:approved checks:failing is:draft`.
pub(crate) const FACETS: &[FacetKey] = &[
    FacetKey::new("author", "Author"),
    FacetKey::new("label", "Label"),
    FacetKey::new("review", "Review"),
    FacetKey::new("checks", "Checks"),
    FacetKey::new("is", "State"),
];

/// The filter's word for how a pull request's checks stand.
fn checks_word(checks: Checks) -> &'static str {
    match checks {
        Checks::Passing => "passing",
        Checks::Failing => "failing",
        Checks::Pending => "pending",
        Checks::Absent => "",
    }
}

/// A row's values for one of the [`FACETS`]: its meta line's, and where
/// it stands as `prs` says — the same checks verdict and state the row's
/// marks and badge draw, so `checks:failing` finds exactly the rows
/// wearing a ✗.
fn facet_values(pr: &OpenPr, key: &str, prs: &PrStore) -> Vec<String> {
    match key {
        "author" => vec![pr.meta.author.clone()],
        "label" => pr.meta.labels.iter().map(|l| l.name.clone()).collect(),
        "review" => vec![pr.meta.review.word().to_string()],
        "checks" => vec![checks_word(prs.status_or_open(&pr.url).health.checks).to_string()],
        "is" => {
            let status = prs.status_or_open(&pr.url);
            let mut is = vec![if status.is_draft() { "draft" } else { "ready" }.to_string()];
            if status.health.conflicts {
                is.push("conflicts".to_string());
            }
            is
        }
        _ => Vec::new(),
    }
}

/// The rows the filter leaves, top to bottom: indices into `list`, each
/// with the matched char positions of its `#42 title` (lit when drawn).
/// The words rank the rows best first and the tokens narrow them; then
/// they gather by [`PrSection`] — yours, then the ones waiting on your
/// review, then the rest — keeping that order inside each. With nothing
/// typed, every row in list order, section by section. Worked out afresh
/// on every call rather than kept — a project's open pull requests are a
/// handful — so it can never go stale against the list.
fn visible_rows(query: &str, list: &[OpenPr], prs: &PrStore) -> Vec<(usize, Vec<usize>)> {
    let parsed = crate::list_filter::parse(query, FACETS);
    let mut rows = crate::list_filter::narrow(
        &parsed,
        FACETS,
        list.len(),
        |i| prs.label(list[i].number, &list[i].url, &list[i].title),
        |i, key| facet_values(&list[i], key, prs),
    );
    rows.sort_by_key(|(i, _)| list[*i].section());
    rows
}

/// The row under the cursor, as an index into `list`: `selected` while
/// the filter shows it, else the filter's best match — a refresh may have
/// moved the cursor's pull request under a row the filter hides — and
/// `selected` clamped onto the list with nothing typed. None with no row
/// to be on: an empty list, or a filter nothing matches.
fn cursor_index(view: &PullRequestsView, list: &[OpenPr], prs: &PrStore) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    if !has_query(view) {
        return Some(clamp_selection(view.selected as i64, list.len()));
    }
    crate::list_filter::cursor_in(&visible_rows(&view.query, list, prs), view.selected)
}

/// A list that landed within [`FRESH`]: the modal opens on it as it is.
fn is_fresh(app: &App, project: &ProjectId) -> bool {
    app.open_prs
        .get(project)
        .is_some_and(|open| open.at.elapsed() < FRESH)
}

/// Ask for the project's open list on the loop's next turn, past its
/// beat — `Shift+R`'s path (`App::pr_refresh_requested`), which asks for
/// the selected project, the modal's. A lookup already in flight may have
/// been asked before whatever this is for (a merge, a new pull request):
/// it owes a fresh one, which starts the moment it lands.
pub(crate) fn request_list(app: &mut App, project: &ProjectId) {
    if let Some(open) = app.open_prs.get_mut(project) {
        open.due = std::time::Instant::now();
    }
    app.open_prs_inflight.want_fresh(project);
    app.pr_refresh_requested = true;
}

/// The pull request under the cursor, while the modal is up and the list
/// has a row the filter shows.
pub(crate) fn selected_pr(app: &App) -> Option<OpenPr> {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return None;
    };
    let list = rows(app, &view.project);
    cursor_index(view, list, &app.prs).and_then(|i| list.get(i).cloned())
}

/// `⌘G`: the AUTOFIX form for the pull request under the cursor — at
/// once from a fresh body, else once a fresh one lands
/// (`autofix::land_detail`), so it names the checks failing now.
fn autofix(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = selected_pr(app) else {
        return;
    };
    let fresh = app.pr_detail_fresh(&pr.url);
    if let (true, Some(detail)) = (fresh, app.pr_detail.get(&pr.url).cloned()) {
        crate::autofix::open_for(app, project, pr, &detail);
        return;
    }
    app.autofix.pending_open = Some(pr.url.clone());
    app.pr_detail_failed.remove(&pr.url);
    app.pending_pr_detail = Some((
        crate::app::PendingPrDetail {
            url: pr.url.clone(),
            number: pr.number,
            dir,
        },
        std::time::Instant::now(),
    ));
    app.flash = Some(crate::flash::Flash::working(format!(
        "Reading #{}'s checks…",
        pr.number
    )));
}

/// Whether the modal is what is up.
pub(crate) fn is_up(app: &App) -> bool {
    matches!(&app.overlay, Some(Overlay::PullRequests(_)))
}

/// The URL of the pull request under the cursor, for the browser.
fn selected_url(app: &App) -> Option<String> {
    selected_pr(app).map(|pr| pr.url)
}

/// The detail fetch a pull request is owed, if any (`App::pr_detail_owed`):
/// none for one read within `PR_DETAIL_FRESH`, in flight, or refused
/// moments ago. A body the cache hydrated (`pr_detail_stale`), or one read
/// longer ago, shows at once and is fetched fresh over the top, as the
/// pane's is.
fn pending_for(app: &App, url: String, number: u64, dir: PathBuf) -> Option<PendingPrDetail> {
    app.pr_detail_owed(&url)
        .then_some(PendingPrDetail { url, number, dir })
}

/// Arm (or disarm) the debounced fetch of the row under the modal's
/// cursor, on the slot the pane's own fetch uses (`App::pending_pr_detail`)
/// — the loop fires it and lands the answer in `App::pr_detail` either way.
/// While the modal is up this is the one that decides what that slot holds
/// (`event_loop::schedule_pr_detail` defers to it).
pub(crate) fn schedule_detail(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let dir = view.dir.clone();
    let pending = selected_pr(app).and_then(|pr| pending_for(app, pr.url, pr.number, dir));
    app.pending_pr_detail = pending.map(|p| (p, std::time::Instant::now() + DETAIL_DEBOUNCE));
}

/// The list under the modal changed — an answer landed, or a detail said
/// a pull request merged and retired its row. The cursor stays on its
/// pull request wherever the new list put it; one that is gone leaves the
/// cursor on the row that took its index, with that row's body asked for
/// and the pane rewound. Run from wherever `App::open_prs` changes.
pub(crate) fn list_changed(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let found = view
        .selected_url
        .as_ref()
        .and_then(|url| list.iter().position(|pr| &pr.url == url));
    let (index, url) = match found {
        Some(i) => (i, view.selected_url.clone()),
        None => {
            let visible = visible_rows(&view.query, list, &app.prs);
            let row = clamp_selection(view.selected_row as i64, visible.len());
            let i = visible.get(row).map_or_else(
                || clamp_selection(view.selected as i64, list.len()),
                |(i, _)| *i,
            );
            (i, list.get(i).map(|pr| pr.url.clone()))
        }
    };
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    let moved = url != view.selected_url;
    view.selected = index;
    view.selected_url = url;
    sync_row(app);
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    if moved {
        view.scroll = 0;
        view.tabs.rewind();
        schedule_detail(app);
    } else if app.pending_pr_detail.is_none() {
        // Still on the same pull request: the list's beat reads its page
        // again once it has aged out, so checks running under the reader
        // catch up on their own.
        schedule_detail(app);
    }
    queue_prefetch(app, false);
    app.dirty = true;
}

/// Move the cursor to `index` (clamped): the pane rewinds and the row's
/// body is asked for once the cursor rests.
fn select(app: &mut App, index: i64) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let next = clamp_selection(index, list.len());
    let url = list.get(next).map(|pr| pr.url.clone());
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    if next != view.selected || url != view.selected_url {
        view.selected = next;
        view.selected_url = url;
        view.scroll = 0;
        view.tabs.rewind();
    }
    sync_row(app);
    schedule_detail(app);
    app.dirty = true;
}

/// Settle [`PullRequestsView::selected_row`] onto where the cursor now
/// sits among the visible rows.
fn sync_row(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let row = cursor_index(view, list, &app.prs).and_then(|c| {
        visible_rows(&view.query, list, &app.prs)
            .iter()
            .position(|(i, _)| *i == c)
    });
    if let (Some(row), Some(Overlay::PullRequests(view))) = (row, &mut app.overlay) {
        view.selected_row = row;
    }
}

/// ↑/↓, the wheel: the cursor `delta` rows through the visible ones —
/// the filter's matches while one is typed — clamped at either end.
fn step(app: &mut App, delta: i64) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let Some(current) = cursor_index(view, list, &app.prs) else {
        return;
    };
    let visible = visible_rows(&view.query, list, &app.prs);
    let at = visible.iter().position(|(i, _)| *i == current).unwrap_or(0) as i64;
    let next = clamp_selection(at + delta, visible.len());
    if let Some((index, _)) = visible.get(next) {
        select(app, *index as i64);
    }
}

/// The filter's text changed: the cursor goes to its best match — the
/// pane rewinds onto it and its body is asked for, as any move does — or
/// stays where it is once nothing is typed, so the row just found keeps
/// the cursor after Esc has cleared the letters that found it. A filter
/// nothing matches moves nothing: the list says so, the pane has no row
/// to read, and the next letter or Backspace decides.
fn query_changed(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let list = rows(app, &view.project);
    let target = if has_query(view) {
        visible_rows(&view.query, list, &app.prs)
            .first()
            .map(|(i, _)| *i)
    } else {
        cursor_index(view, list, &app.prs)
    };
    match target {
        Some(index) => select(app, index as i64),
        None => schedule_detail(app),
    }
    app.dirty = true;
}

/// Esc: the filter cleared, the cursor staying on the row it was on.
fn clear_query(app: &mut App) {
    if let Some(Overlay::PullRequests(view)) = &mut app.overlay {
        view.query.clear();
    }
    query_changed(app);
}

/// A bracketed paste lands in the filter, as one line, and narrows the
/// rows as typing it would. True whenever the modal is up: the filter is
/// always live.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    if crate::pr_actions::form_up(app) {
        return crate::pr_actions::paste(app, text);
    }
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return false;
    };
    view.focus = PrFocus::List;
    view.query.insert_str(text);
    query_changed(app);
    true
}

/// `⌘R`: ask for the list again now, and the selected pull request's body
/// over the cached copy. The rows stay until the answer lands, the title
/// saying `refreshing…` meanwhile. A body already being read was asked
/// before the key, so it owes a fresh read when it lands.
fn refresh(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    request_list(app, &project);
    if let Some(pr) = selected_pr(app) {
        if !app.pr_detail_inflight.want_fresh(&pr.url) {
            app.pr_detail_failed.remove(&pr.url);
            app.pending_pr_detail = Some((
                PendingPrDetail {
                    url: pr.url,
                    number: pr.number,
                    dir,
                },
                std::time::Instant::now(),
            ));
        }
    }
    app.dirty = true;
}

// ---- launching ----

/// The launch the row under the cursor describes — the group row's, for
/// this pull request: the `quick_prompt_kind` SETTING's harness, the pull
/// request carried as `QuickLaunch::pr`, addressed to the project's root.
fn launch_for_selected(app: &App) -> Option<QuickLaunch> {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return None;
    };
    let pr = selected_pr(app)?;
    crate::quick_prompt::pr_launch_for(app, &view.project, &pr)
}

/// `Enter` on the list: the QUICK PROMPT for a PR SESSION on the pull
/// request. The box goes up over the modal, which stays on screen under
/// it: Esc puts the modal back on the row (`QuickLaunch::under`), and the
/// launch closes it onto the new session's card. The box's own `Tab` and
/// `⇧Tab` pick another harness or an AGENT PRESET for it.
fn open_prompt_for_selected(app: &mut App) {
    let under = ModalUnder::of(app.overlay.as_ref());
    if let Some(launch) = launch_for_selected(app) {
        crate::quick_prompt::open_pr_box(app, launch.with_under(under));
    }
}

/// `⌘Y`: the COMMENT BOX for the pull request under the cursor, carrying the
/// modal so Enter and Esc come back to it on the row. A draft a refused
/// post left for this pull request fills the box.
fn open_comment_for_selected(app: &mut App) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let view = view.clone();
    let Some(pr) = selected_pr(app) else {
        return;
    };
    let draft = app.pr_comment_drafts.remove(&pr.url).unwrap_or_default();
    crate::event_loop::reopen_prompt_with(
        app,
        PromptKind::PrComment {
            number: pr.number,
            label: pr.label(),
            url: pr.url,
            back: Some(Box::new(view)),
        },
        draft,
    );
}

/// `⌘O`, and a click on the reading pane's `↗ open in browser` button
/// (`HitTarget::ModalBrowser`): the pull request under the cursor in the
/// browser, through the very `event_loop::open_link` a card's `⇧V` and
/// `⇧I` run — the footer says when it could not — and the pull request is
/// marked read on the way out, its conversation about to be on screen.
/// Nothing under the cursor opens nothing. INPUT PARITY: the key and the
/// click end in the same state.
pub(crate) fn open_in_browser(app: &mut App, out: &mut Vec<ClientRequest>) {
    if let Some(url) = selected_url(app) {
        crate::event_loop::open_link(app, &url, out);
    }
}

// ---- keys and mouse ----

/// The row under the reading side's cursor, acted on — `⌘E` on a file or
/// a commit, `⌘O` on a check (`pr_preview::run_act`). False with nothing
/// to act on there: a tab of prose, or a body still on its way.
fn act_on_row(app: &mut App, out: &mut Vec<ClientRequest>) -> bool {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return false;
    };
    let tabs = view.tabs.clone();
    let Some(pr) = selected_pr(app) else {
        return false;
    };
    let Some(act) = app
        .pr_detail
        .get(&pr.url)
        .and_then(|detail| crate::pr_preview::row_act(detail, &tabs))
    else {
        return false;
    };
    crate::pr_preview::run_act(app, pr.number, &pr.url, &pr.label(), act, out);
    true
}

/// `⌘E`: the diff of what the reading side has under its cursor — the
/// file on Changes, the commit on Commits — else the whole pull request's.
fn diff(app: &mut App, out: &mut Vec<ClientRequest>) {
    let on = match &app.overlay {
        Some(Overlay::PullRequests(view)) => view.tabs.tab,
        _ => return,
    };
    if matches!(on, PrTab::Changes | PrTab::Commits) && act_on_row(app, out) {
        return;
    }
    if let Some(pr) = selected_pr(app) {
        crate::event_loop::request_pr_diff_for(app, pr.number, pr.url.clone(), pr.label());
    }
}

/// `⌘O`: the check under the cursor on Checks, else the pull request.
fn browser(app: &mut App, out: &mut Vec<ClientRequest>) {
    let on = match &app.overlay {
        Some(Overlay::PullRequests(view)) => view.tabs.tab,
        _ => return,
    };
    if on == PrTab::Checks && act_on_row(app, out) {
        return;
    }
    open_in_browser(app, out);
}

/// Show `tab` on the reading side, its body from the top.
fn switch_tab(view: &mut PullRequestsView, tab: PrTab) {
    if view.tabs.switch(tab) {
        view.scroll = 0;
    }
}

/// Enter on the page: the row under its cursor acted on — the DIFF
/// VIEWER at a file or a commit, a check's page — on Reviews the review
/// form (`⌘⇧R`), and on the Description the pull request in the
/// browser, as Enter on the pane's page does.
fn act(app: &mut App, out: &mut Vec<ClientRequest>) {
    let tab = match &app.overlay {
        Some(Overlay::PullRequests(view)) => view.tabs.tab,
        _ => return,
    };
    if tab.lists() {
        act_on_row(app, out);
    } else if tab == PrTab::Reviews {
        crate::pr_actions::open_review(app);
    } else {
        open_in_browser(app, out);
    }
}

/// Keys in the PULL REQUESTS MODAL. A form in the reading pane's place
/// takes every key (`pr_actions::handle_key`). Otherwise `Tab` / `⇧Tab`
/// move the keys between the list and the page ([`PrFocus`]); the list's
/// filter is always live, so letters type — the modal's own hotkey and
/// `q` among them, a letter on the page handing the keys back to the list
/// — and the verbs are chords. Esc steps back: off the page, then the
/// filter, then the modal.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    use crate::pr_preview::keys as page;
    if crate::pr_actions::form_up(app) {
        crate::pr_actions::handle_key(app, key);
        return;
    }
    if matches!(&app.overlay, Some(Overlay::PullRequests(v)) if v.filter_pick.is_some()) {
        filter_pick_key(app, &key);
        return;
    }
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let plain = key.modifiers.is_empty();
    let on_page = view.focus == PrFocus::Page;
    let max = view.max_scroll();
    match key.code {
        KeyCode::Esc if on_page => view.focus = PrFocus::List,
        // Two-stage escape, like every fuzzy overlay: a typed filter is
        // cleared before the second Esc closes the modal.
        KeyCode::Esc if !view.query.is_empty() => clear_query(app),
        // The PR PICK's last Esc is the LINEAR VIEW's, not the panels'.
        KeyCode::Esc if view.pick.is_some() => back_to_linear(app),
        KeyCode::Esc => close(app),
        // Tab / ⇧Tab (a shifted Tab under the kitty protocol too): the
        // keys to the other panel.
        _ if keys::PANEL.matches(&key) => view.focus = view.focus.other(),
        // The page's tabs: ←/→ with the keys on it, ⇧←/⇧→ from anywhere,
        // round either end.
        KeyCode::Left | KeyCode::Right if page::MODAL_TABS.matches(&key) || (on_page && plain) => {
            let delta = if key.code == KeyCode::Right { 1 } else { -1 };
            let tab = view.tabs.tab.step(delta);
            switch_tab(view, tab);
        }
        // ↑/↓ walk a listing's rows or scroll prose with the keys on the
        // page, and ⇧↑/⇧↓ do from the list; on the list ↑/↓ walk the rows
        // the filter leaves.
        KeyCode::Down if shift || on_page => {
            view.tabs.navigate(Nav::Line(1), &mut view.scroll, max)
        }
        KeyCode::Up if shift || on_page => view.tabs.navigate(Nav::Line(-1), &mut view.scroll, max),
        KeyCode::Down => step(app, 1),
        KeyCode::Up => step(app, -1),
        // The reading side scrolls by the page — a listing walks its rows
        // by the page.
        KeyCode::PageDown => view.tabs.navigate(Nav::Page(1), &mut view.scroll, max),
        KeyCode::PageUp => view.tabs.navigate(Nav::Page(-1), &mut view.scroll, max),
        KeyCode::Home => view.tabs.navigate(Nav::Top, &mut view.scroll, max),
        KeyCode::End => view.tabs.navigate(Nav::Bottom, &mut view.scroll, max),
        // Enter: the page's row with the keys there, else an agent on the
        // pull request.
        _ if on_page && keys::ACT.matches(&key) => act(app, out),
        // The PR PICK's Enter attaches the issues it carries instead.
        _ if view.pick.is_some() && keys::ATTACH.matches(&key) => attach_to_selected(app),
        _ if keys::PROMPT.matches(&key) => open_prompt_for_selected(app),
        _ if keys::COMMENT.matches(&key) => open_comment_for_selected(app),
        _ if keys::DIFF.matches(&key) => diff(app, out),
        _ if keys::BROWSER.matches(&key) => browser(app, out),
        _ if keys::REVIEW.matches(&key) => crate::pr_actions::open_review(app),
        _ if keys::REFRESH.matches(&key) => refresh(app),
        _ if keys::NEW.matches(&key) => crate::pr_actions::open_create(app),
        // ⌘X cuts a SELECTION in the filter before it merges.
        _ if keys::MERGE.matches(&key) && view.query.selected().is_none() => {
            crate::pr_actions::open_merge(app)
        }
        _ if keys::CLOSE.matches(&key) => crate::pr_actions::open_close(app),
        _ if keys::READY.matches(&key) => crate::pr_actions::toggle_draft(app),
        _ if keys::AUTOFIX.matches(&key) => autofix(app),
        // A PR PICK already came from the LINEAR VIEW: `⌘L` there would
        // stack the two views on each other, so it does nothing.
        _ if keys::LINEAR.matches(&key) && view.pick.is_some() => {}
        _ if keys::LINEAR.matches(&key) => crate::linear::open_attach(app),
        // The FILTER PICK in the page's place, the keys its own.
        _ if keys::FILTER.matches(&key) => {
            view.focus = PrFocus::List;
            view.filter_pick = Some(FilterPick::default());
        }
        // Everything else feeds the always-live fuzzy filter, which edits
        // like a terminal line (see text_input) — and typing hands the
        // list the keys.
        _ => {
            if view.query.handle_key(&key).changed() {
                view.focus = PrFocus::List;
                query_changed(app);
            }
        }
    }
    app.dirty = true;
}

/// A key while the FILTER PICK is up: its cursor moves, `space` adds or
/// takes out the value's token in the filter line — the rows narrowing
/// behind it — and Enter or Esc puts it away.
fn filter_pick_key(app: &mut App, key: &KeyEvent) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let facets = pick_facets(rows(app, &view.project), &app.prs, app.theme);
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    let changed =
        crate::list_filter::apply_key(&mut view.filter_pick, &mut view.query, FACETS, &facets, key);
    if changed {
        query_changed(app);
    }
    app.dirty = true;
}

/// The FILTER PICK's facets over the project's rows: who opened them, their
/// labels (in their GitHub colours), and the fixed words for review,
/// checks and state — each value with how many rows carry it.
fn pick_facets(list: &[OpenPr], prs: &PrStore, th: Theme) -> Vec<PickFacet> {
    use crate::list_filter::{by_count, fixed_values, plain_values, tally};
    let count = |key: &str| tally(list.iter(), |pr| facet_values(pr, key, prs));
    let label_colour = |name: &str| {
        list.iter()
            .flat_map(|pr| &pr.meta.labels)
            .find(|l| l.name.eq_ignore_ascii_case(name))
            .and_then(|l| crate::theme::hex(&l.color))
            .unwrap_or(th.muted)
    };
    FACETS
        .iter()
        .map(|facet| {
            let values = match facet.key {
                "label" => by_count(count("label"))
                    .into_iter()
                    .map(|(value, count)| PickValue {
                        mark: Some(("●".to_string(), label_colour(&value))),
                        value,
                        count,
                    })
                    .collect(),
                "review" => fixed_values(
                    &["approved", "changes", "required"],
                    &count("review"),
                    |_| None,
                ),
                "checks" => {
                    fixed_values(&["passing", "failing", "pending"], &count("checks"), |_| {
                        None
                    })
                }
                "is" => fixed_values(&["ready", "draft", "conflicts"], &count("is"), |_| None),
                key => plain_values(by_count(count(key))),
            };
            PickFacet {
                key: *facet,
                values,
            }
        })
        .collect()
}

/// Mouse in the PULL REQUESTS MODAL: the wheel moves the cursor over the
/// rows the filter leaves and scrolls the reading side over it, a click on
/// a row selects it and hands the list the keys (a launch is `Enter`, not
/// a click — the row is something to read first), a double-click on a row
/// opens that pull request in the browser — the very open `⌘O` and
/// the `↗ open in browser` button run — a click on the page hands it the
/// keys, on a tab shows it, on a file, a commit or a check is Enter on
/// it, and a click outside closes (`overlay_close`); everything else is
/// swallowed. A form up in the reading pane's place has the mouse
/// (`pr_actions::handle_mouse`).
pub(crate) fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: Position,
    out: &mut Vec<ClientRequest>,
) {
    if crate::pr_actions::form_up(app) {
        crate::pr_actions::handle_mouse(app, mouse, mouse_pos);
        return;
    }
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return;
    };
    let over_body = view.body_area.contains(mouse_pos);
    let on_button = view.browser_area.contains(mouse_pos);
    let on_tab = view
        .tabs
        .tab_hits
        .iter()
        .find(|(rect, _)| rect.contains(mouse_pos))
        .map(|(_, tab)| *tab);
    let on_row = view
        .tabs
        .row_hits
        .iter()
        .find(|(rect, _)| rect.contains(mouse_pos))
        .map(|(_, row)| *row);
    let on_fold = view
        .tabs
        .fold_hits
        .iter()
        .find(|(rect, _)| rect.contains(mouse_pos))
        .map(|(_, key)| *key);
    match mouse.kind {
        MouseEventKind::ScrollUp if over_body => view.scroll_by(-WHEEL_LINES),
        MouseEventKind::ScrollDown if over_body => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => step(app, -1),
        MouseEventKind::ScrollDown => step(app, 1),
        // The `↗ open in browser` button, before the rows: the very open
        // `⌘O` runs.
        MouseEventKind::Down(MouseButton::Left) if on_button => open_in_browser(app, out),
        MouseEventKind::Down(MouseButton::Left) if on_tab.is_some() => {
            view.focus = PrFocus::Page;
            if let Some(tab) = on_tab {
                switch_tab(view, tab);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if on_row.is_some() => {
            view.focus = PrFocus::Page;
            if let Some(row) = on_row {
                view.tabs.select(row);
            }
            act_on_row(app, out);
        }
        // A `<details>` summary opens or shuts, as on github.com.
        MouseEventKind::Down(MouseButton::Left) if on_fold.is_some() => {
            view.focus = PrFocus::Page;
            if let Some(key) = on_fold {
                view.tabs.folds.toggle(key);
            }
        }
        // A click on the page hands it the keys.
        MouseEventKind::Down(MouseButton::Left) if over_body => view.focus = PrFocus::Page,
        MouseEventKind::Down(MouseButton::Left) => {
            // Rows are two lines tall with headers between them: the rects
            // the last draw laid them out in say which one is under the
            // pointer.
            let url = view
                .row_rects
                .iter()
                .find(|(_, rect)| rect.contains(mouse_pos))
                .map(|(url, _)| url.clone());
            let project = view.project.clone();
            let hit = url.and_then(|url| {
                rows(app, &project)
                    .iter()
                    .enumerate()
                    .find(|(_, pr)| pr.url == url)
                    .map(|(index, pr)| (index, pr.number))
            });
            let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
                return;
            };
            if let Some((index, number)) = hit {
                view.focus = PrFocus::List;
                let double = crate::event_loop::is_double_click(&mut view.last_row_click, number);
                select(app, index as i64);
                if double {
                    open_in_browser(app, out);
                }
            }
        }
        _ => {}
    }
    app.dirty = true;
}

/// The PULL REQUESTS MODAL's own keys: one table [`handle_key`] matches
/// and [`hints`] spells — the reading side's tabs and rows from the PULL
/// REQUEST PAGE's (`pr_preview::keys`). The letters are the filter's, so
/// the verbs are chords — the ISSUES MODAL's, where the two do the same
/// thing.
pub(crate) mod keys {
    use crate::hints::Key;

    /// Enter on the list: the QUICK PROMPT for an agent on the pull
    /// request.
    pub const PROMPT: Key = crate::issues::keys::PROMPT;
    /// The keys to the other panel: the page from the list, the list from
    /// the page.
    pub const PANEL: Key = Key::new(&["tab", "shift+tab"], "page");
    /// [`PANEL`] as the page names it: back to the list.
    pub const BACK: Key = Key::new(&["shift+tab", "tab"], "list");
    /// The page's own, with the keys on it.
    pub const PAGE_TABS: Key = Key::new(&["left", "right"], "tabs").show(2);
    pub const PAGE_ROWS: Key = Key::new(&["up", "down"], "pick").show(2);
    pub const ACT: Key = Key::new(&["enter"], "open");
    pub const COMMENT: Key = crate::issues::keys::COMMENT;
    /// The grid's changes chord (`Action::GitDiff`), for the pull
    /// request's.
    pub const DIFF: Key = Key::new(&["cmd+e", "ctrl+e"], "diff");
    pub const BROWSER: Key = crate::issues::keys::BROWSER;
    pub const REFRESH: Key = crate::issues::keys::REFRESH;
    pub const READ: Key = crate::issues::keys::READ;
    /// A new pull request, from a branch of the project's.
    pub const NEW: Key = Key::new(&["cmd+n", "ctrl+n"], "new PR");
    /// Merge the pull request under the cursor.
    pub const MERGE: Key = Key::new(&["cmd+x", "ctrl+x"], "merge");
    /// Close the pull request under the cursor without merging it.
    /// ⌘W, the close every modal's remove verb shares — it asks first.
    pub const CLOSE: Key = Key::new(&["cmd+w", "ctrl+w"], "close PR");
    /// Review the pull request under the cursor: approve it, ask for
    /// changes, or comment (`pr_actions::open_review`). ⌘R's letter, held
    /// with ⇧ — ⌘R itself is refresh everywhere.
    pub const REVIEW: Key = Key::new(&["cmd+shift+r", "ctrl+shift+r"], "review");
    /// Mark it ready for review, or a draft again.
    pub const READY: Key = Key::new(&["cmd+d", "ctrl+d"], "ready/draft");
    /// Linear issues to attach the pull request to.
    pub const LINEAR: Key = Key::new(&["cmd+l", "ctrl+l"], "Linear");
    /// The AUTOFIX form: an agent sent at the pull request's conflicts and
    /// failing checks (`crate::autofix`).
    pub const AUTOFIX: Key = Key::new(&["cmd+g", "ctrl+g"], "autofix");
    /// Enter on the PR PICK's list: the pull request attached to the
    /// issues the LINEAR VIEW marked.
    pub const ATTACH: Key = Key::new(&["enter"], "attach to this PR");
    pub const TABS: Key = crate::pr_preview::keys::MODAL_TABS;
    pub const ROWS: Key = crate::pr_preview::keys::MODAL_ROWS;
    /// The FILTER PICK — the LINEAR VIEW's too.
    pub const FILTER: Key = crate::list_filter::keys::FILTER;
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        PROMPT, PANEL, BACK, PAGE_TABS, PAGE_ROWS, ACT, COMMENT, DIFF, BROWSER, REFRESH, READ, NEW,
        MERGE, CLOSE, READY, REVIEW, LINEAR, AUTOFIX, ATTACH, TABS, ROWS, FILTER,
    ];
}

/// The keys along the modal's bottom edge, for the panel that has them —
/// `⌘E` and `⌘O` named for what they reach on the tab showing, and Enter
/// on the page for what it does there. A form up in the reading pane's
/// place says its own (`pr_actions::hints`). Esc steps back off the page,
/// then clears a typed filter, then closes — or, in a PR PICK, goes back
/// to the LINEAR VIEW, whose issues `Enter` attaches the row to.
pub(crate) fn hints(view: &PullRequestsView) -> Vec<crate::hints::Hint> {
    use crate::hints::Hint;
    if let Some(form) = &view.form {
        return crate::pr_actions::hints(form);
    }
    if view.filter_pick.is_some() {
        return crate::list_filter::hints();
    }
    let tab = view.tabs.tab;
    let diff = match tab {
        PrTab::Changes => keys::DIFF.hint_as("diff the file"),
        PrTab::Commits => keys::DIFF.hint_as("diff the commit"),
        _ => keys::DIFF.hint(),
    };
    let browser = if tab == PrTab::Checks {
        keys::BROWSER.hint_as("open the check")
    } else {
        keys::BROWSER.hint()
    };
    if view.focus == PrFocus::Page {
        let act_does = if tab == PrTab::Reviews {
            "review"
        } else {
            tab.act_does()
        };
        return vec![
            keys::ACT.hint_as(act_does).kept(),
            keys::PAGE_TABS.hint(),
            keys::PAGE_ROWS.hint_as(if tab.lists() { "pick" } else { "scroll" }),
            keys::BACK.hint(),
            keys::REVIEW.hint(),
            keys::COMMENT.hint(),
            keys::MERGE.hint(),
            keys::CLOSE.hint(),
            keys::READY.hint(),
            keys::AUTOFIX.hint(),
            diff,
            browser,
            keys::READ.hint(),
            Hint::new("Esc", "list"),
        ];
    }
    // The PR PICK's Enter attaches, its `⌘L` has nowhere to go, and its
    // last Esc goes back to the LINEAR VIEW.
    let picking = view.pick.is_some();
    let enter = if picking {
        keys::ATTACH.hint()
    } else {
        keys::PROMPT.hint()
    };
    let mut hints = vec![
        enter.kept(),
        keys::PANEL.hint().kept(),
        keys::FILTER.hint(),
        keys::TABS.hint(),
    ];
    if tab.lists() {
        hints.push(keys::ROWS.hint());
    }
    hints.extend([
        keys::NEW.hint(),
        keys::MERGE.hint(),
        keys::CLOSE.hint(),
        keys::READY.hint(),
        keys::AUTOFIX.hint(),
        keys::REVIEW.hint(),
        keys::COMMENT.hint(),
        diff,
        browser,
        keys::READ.hint(),
    ]);
    if !picking {
        hints.push(keys::LINEAR.hint());
    }
    let esc = if !view.query.is_empty() {
        "clear"
    } else if picking {
        "back"
    } else {
        "close"
    };
    hints.extend([keys::REFRESH.hint(), Hint::new("Esc", esc)]);
    hints
}

// ---- drawing ----

/// The right end of a list row: the checks' mark and the review's in
/// this many cells, the cell of air after them included…
const MARKS_W: usize = 4;
/// …then the state word, `conflicts` the widest, in a column this wide…
const WORD_W: usize = 9;
/// …then two cells, then how long ago it was opened, at least this wide.
const AGE_W: usize = 3;

/// How long ago `pr` was opened, as its row's last column says it: `3d`,
/// `21m`, `now` — empty for a row the list said nothing about.
fn age_of(pr: &OpenPr, now: i64) -> String {
    crate::pull_request::rfc3339_secs(&pr.meta.created_at)
        .map(|at| crate::hosts::ago_short(now - at))
        .unwrap_or_default()
}

/// The columns every row of the list lines up on: the widest `#42`, so
/// the titles start in one column, and the widest age.
struct RowCols {
    number: usize,
    age: usize,
}

impl RowCols {
    fn of(rows: &[OpenPr], now: i64) -> Self {
        RowCols {
            number: rows
                .iter()
                .map(|pr| pr.number.to_string().len() + 1)
                .max()
                .unwrap_or(0),
            age: rows
                .iter()
                .map(|pr| age_of(pr, now).chars().count())
                .max()
                .unwrap_or(0)
                .max(AGE_W),
        }
    }
}

/// One list row on one line, the main page's LIST row's shape: the
/// cursor's `▌` — `cursor_focus` is whether the list has the keys, on the
/// cursor's row — or two cells of air, the BAND's pull request arrow `↗`
/// in `pr_row::look`'s colours, `#42` in a column as wide as the list's
/// widest, the title (faint for a draft), and at the right end the status
/// column ([`status_spans`]: `✓ ○ ready`), then how long ago it was
/// opened. The title gives way to all of it. The chars the filter matched
/// (`positions`, into the row's `#42 title`) are lit. Where it stands —
/// the arrow's colour, the badge, the checks' mark — is `stands`, its
/// `App::prs` status.
#[allow(clippy::too_many_arguments)]
fn row_line(
    pr: &OpenPr,
    stands: &PrStatus,
    positions: &[usize],
    cursor_focus: Option<bool>,
    cols: &RowCols,
    width: usize,
    now: i64,
    th: Theme,
) -> Line<'static> {
    let mark = match cursor_focus {
        Some(true) => Span::styled("▌ ", Style::default().fg(th.accent)),
        Some(false) => Span::styled("▌ ", Style::default().fg(th.dim)),
        None => Span::raw("  "),
    };
    let trouble = stands.trouble();
    let look = crate::pr_row::look(stands.standing, trouble, th);
    let number = format!("#{}", pr.number);
    let number_w = number.chars().count();
    // The positions split where the number ends: the title's own count
    // from its first char, past the space.
    let split = positions.partition_point(|&p| p < number_w);
    let title_positions: Vec<usize> = positions[split..]
        .iter()
        .filter_map(|p| p.checked_sub(number_w + 1))
        .collect();
    let mut spans = vec![mark, Span::styled("↗ ", Style::default().fg(look.glyph))];
    spans.extend(fuzzy_highlight_styled(
        &number,
        &positions[..split],
        Style::default().fg(th.dim),
        th,
    ));
    spans.push(Span::raw(" ".repeat(cols.number + 2 - number_w)));
    let lead = 2 + 2 + cols.number + 2;
    // The status column gives way before the title shortens past
    // MIN_TEXT_W: the checks' and review's marks first, then the word.
    let fits = |status_w: usize| {
        width > lead + crate::pr_preview::MIN_TEXT_W + 1 + status_w + 2 + cols.age
    };
    let status = if fits(MARKS_W + WORD_W) {
        Some(true)
    } else {
        fits(WORD_W).then_some(false)
    };
    let status_w = status.map_or(0, |marks| WORD_W + 2 + if marks { MARKS_W } else { 0 });
    let room = width.saturating_sub(lead + 1 + status_w + cols.age + 1);
    let title = truncate(stands.title_or(&pr.title), room);
    let shown = title.chars().count();
    let lit: Vec<usize> = title_positions.into_iter().filter(|&p| p < shown).collect();
    let title_color = if stands.is_draft() && trouble.is_none() {
        th.faint
    } else {
        th.text
    };
    spans.extend(fuzzy_highlight_styled(
        &title,
        &lit,
        Style::default().fg(title_color),
        th,
    ));
    let used = lead + shown;
    let age = age_of(pr, now);
    let mut right: Vec<Span<'static>> = Vec::new();
    if let Some(marks) = status {
        right.extend(status_spans(pr, stands, marks, th));
        right.push(Span::raw("  "));
    }
    right.push(Span::styled(
        format!("{age:>w$}", w = cols.age),
        Style::default().fg(th.dim),
    ));
    let right_w: usize = right.iter().map(|s| s.content.chars().count()).sum();
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(used + right_w + 1)),
    ));
    spans.extend(right);
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// A row's status column: the checks' mark and the review's — `✓ ○`, a
/// blank where either has nothing to say — when `marks`, then the state
/// word the BAND puts after `#42`, [`WORD_W`] wide: the trouble's
/// (`conflicts`, `failing`), else `ready` or `draft`, in `pr_row::look`'s
/// colours.
fn status_spans(pr: &OpenPr, status: &PrStatus, marks: bool, th: Theme) -> Vec<Span<'static>> {
    let trouble = status.trouble();
    let word = status.word();
    let look = crate::pr_row::look(status.standing, trouble, th);
    let checks = pr.meta.checks.map(|_| match status.health.checks {
        Checks::Failing => ("✗", th.err),
        Checks::Pending => ("◐", th.warn),
        _ => ("✓", th.ok),
    });
    let review = match pr.meta.review {
        Review::Approved => Some(("✓", th.ok)),
        Review::Changes => Some(("✗", th.err)),
        Review::Required => Some(("○", th.muted)),
        Review::None => None,
    };
    let mark = |glyph: Option<(&'static str, ratatui::style::Color)>| match glyph {
        Some((g, color)) => Span::styled(g, Style::default().fg(color)),
        None => Span::raw(" "),
    };
    let word = Span::styled(format!("{word:<WORD_W$}"), Style::default().fg(look.badge));
    if !marks {
        return vec![word];
    }
    vec![
        mark(checks),
        Span::raw(" "),
        mark(review),
        Span::raw(" "),
        word,
    ]
}

/// What a draw records for the mouse and the next draw — the modal, the
/// list's rows and where each landed — handing back the view for what
/// the reading side records on top.
fn record_list(
    app: &mut App,
    area: Rect,
    list_area: Rect,
    list_start: usize,
    row_rects: Vec<(String, Rect)>,
) -> Option<&mut PullRequestsView> {
    let Some(Overlay::PullRequests(v)) = &mut app.overlay else {
        return None;
    };
    v.area = area;
    v.list_area = list_area;
    v.list_start = list_start;
    v.row_rects = row_rects;
    Some(v)
}

/// The PULL REQUESTS MODAL: the list down the left, the reading pane on
/// the right. `backdrop` draws it as the layer under a QUICK PROMPT box
/// opened from it (`QuickLaunch::under`): dim frames and an unfocused
/// cursor row, the box in front having the eye.
pub(crate) fn draw(
    f: &mut Frame,
    app: &mut App,
    view: &PullRequestsView,
    th: Theme,
    backdrop: bool,
) {
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let list_w = (area.width * LIST_PCT / 100)
        .max(MIN_LIST_W)
        .min(area.width.saturating_sub(SPLIT_PANE_LAYOUT_MIN));
    let [list_a, body_a] = Layout::horizontal([
        Constraint::Length(list_w),
        Constraint::Min(SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);

    let rows: Vec<OpenPr> = rows(app, &view.project).to_vec();
    let inflight = app.open_prs_inflight.in_flight(&view.project);
    let asked = app.open_prs.contains_key(&view.project);
    // The last ask came back with nothing — these rows are the last
    // answer that worked, however old — and no second ask is running yet.
    let stale = app.open_prs_failed.contains(&view.project) && !inflight;
    // The rows the filter leaves, and where the cursor sits among them.
    let parsed = crate::list_filter::parse(&view.query, FACETS);
    let visible = visible_rows(&view.query, &rows, &app.prs);
    let cursor = cursor_index(view, &rows, &app.prs);
    let cursor_row = cursor
        .and_then(|c| visible.iter().position(|(i, _)| *i == c))
        .unwrap_or(0);

    // ---- left: the list ----
    // The count reads `matches/all` while a filter is on.
    let count = if parsed.is_active() {
        format!("{}/{}", visible.len(), rows.len())
    } else {
        rows.len().to_string()
    };
    // A PR PICK names the issues it attaches to, as the LINEAR VIEW
    // opened from a pull request names the pull request (`Linear → PR #42`).
    let head = match &view.pick {
        Some(pick) => format!("Pull requests → {}", pick.ids()),
        None => "Pull requests".to_string(),
    };
    let title = format!(
        "{head} — {} ({}{})",
        view.project_name,
        count,
        if inflight { ", refreshing…" } else { "" }
    );
    // The panel with the keys wears the accent — neither, while a box
    // over the modal or a form in the page's place has them.
    let side_up = view.form.is_some() || view.filter_pick.is_some();
    let list_focused = !backdrop && !side_up && view.focus == PrFocus::List;
    let page_focused = !backdrop && !side_up && view.focus == PrFocus::Page;
    let block = panel_block(&title, list_focused, th);
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    // The always-live filter on a row of its own, a blank line either side
    // of it and two cells in, as the rows' text is; its `key:value` tokens
    // lit, and what `⌘F` narrows by at its right end while there is room.
    if let Some(query_area) = row_rect(list_inner, 1) {
        let hint = format!("{} label · author · checks", keys::FILTER.label());
        let hint_w = hint.chars().count() as u16;
        let typed = view.query.chars().count() as u16;
        let show_hint = query_area.width >= 2 + typed.max(16) + 2 + hint_w + 2;
        let field = Rect {
            x: query_area.x + 2,
            width: query_area
                .width
                .saturating_sub(2 + if show_hint { hint_w + 4 } else { 1 }),
            ..query_area
        };
        let line = search_line_lit(&view.query, "type to filter…", field, th, &parsed.spans);
        f.render_widget(Paragraph::new(line), field);
        if show_hint {
            let at = Rect {
                x: query_area.x + query_area.width - 2 - hint_w,
                width: hint_w,
                ..query_area
            };
            f.render_widget(
                Paragraph::new(Span::styled(hint, Style::default().fg(th.faint))),
                at,
            );
        }
    }
    let mut rows_area = Rect {
        y: list_inner.y + 3.min(list_inner.height),
        height: list_inner.height.saturating_sub(3),
        ..list_inner
    };
    // A list GitHub could not be asked for says so on a row of its own
    // under the filter, never only in a title a narrow list would cut:
    // rows that stopped refreshing look exactly like current ones (#106).
    if stale {
        let retry = keys::REFRESH.label();
        let note = if rows.is_empty() {
            format!("couldn't ask GitHub ({retry} retries)")
        } else {
            format!("couldn't refresh ({retry} retries)")
        };
        if let Some(note_area) = row_rect(rows_area, 0) {
            let note = Line::from(vec![
                Span::raw("  "),
                Span::styled(note, Style::default().fg(th.warn)),
            ]);
            f.render_widget(Paragraph::new(note), note_area);
        }
        rows_area = crate::ui::below_first_row(rows_area);
    }
    if rows.is_empty() {
        if inflight || !asked {
            empty_list_row(f, rows_area, "asking GitHub…", th);
        } else if !stale {
            empty_list_row(f, rows_area, "no open pull requests", th);
        }
    } else if visible.is_empty() {
        empty_list_row(f, rows_area, "no pull requests match", th);
    }
    // Sections — yours, waiting on your review, the rest — each under a
    // rule the way the main page's checkouts sit under their bands, a blank
    // line between them; a row one line, the main page's LIST row's shape.
    let keys: Vec<PrSection> = visible.iter().map(|(i, _)| rows[*i].section()).collect();
    let entries = sections(&keys, true);
    let (list_start, drawn) =
        layout_sections_spaced(&entries, |_| 1, cursor_row, view.list_start, rows_area);
    let width = rows_area.width as usize;
    let now = orion_core::clock::now_secs() as i64;
    let cols = RowCols::of(&rows, now);
    let mut row_rects = Vec::with_capacity(drawn.len());
    for (entry, rect) in drawn {
        match entry {
            ListEntry::Header { first, count } => {
                let rule = crate::ui::section_rule(
                    keys[first].name(),
                    th.muted,
                    vec![Span::styled(count.to_string(), Style::default().fg(th.dim))],
                    width,
                    th,
                );
                let at = Rect {
                    y: rect.y + rect.height - 1,
                    height: 1,
                    ..rect
                };
                f.render_widget(Paragraph::new(rule), at);
            }
            ListEntry::Row(v) => {
                let (index, positions) = &visible[v];
                let pr = &rows[*index];
                let on = Some(*index) == cursor;
                let line = row_line(
                    pr,
                    &app.prs.status_or_open(&pr.url),
                    positions,
                    on.then_some(list_focused),
                    &cols,
                    width,
                    now,
                    th,
                );
                let mut row = Paragraph::new(line);
                if on && list_focused {
                    row = row.style(Style::default().bg(th.focus_tint));
                }
                f.render_widget(row, rect);
                row_rects.push((pr.url.clone(), rect));
            }
        }
    }

    // ---- right: the FILTER PICK in the page's place, while it is up ----
    if let Some(pick) = &view.filter_pick {
        let block = panel_block("Filter", !backdrop, th);
        let inner = block.inner(body_a);
        f.render_widget(block, body_a);
        let facets = pick_facets(&rows, &app.prs, th);
        let mut pick = *pick;
        pick.clamp(&facets);
        crate::list_filter::draw_pick(f, inner, &facets, &parsed, &pick, th);
        if !backdrop {
            crate::hints::draw_on_border(f, area, &hints(view), 0, th);
        }
        if let Some(v) = record_list(app, area, rows_area, list_start, row_rects) {
            v.body_area = Rect::default();
            v.browser_area = Rect::default();
            v.tabs.tab_hits.clear();
            v.tabs.row_hits.clear();
            v.tabs.fold_hits.clear();
            v.filter_pick = Some(pick);
        }
        return;
    }

    // ---- right: a form in the page's place, while one is up ----
    if let Some(form) = &view.form {
        let drawn = crate::pr_actions::draw(f, body_a, form, !backdrop, th);
        if !backdrop {
            crate::hints::draw_on_border(f, area, &hints(view), drawn.foot_w, th);
        }
        if let Some(v) = record_list(app, area, rows_area, list_start, row_rects) {
            v.body_area = Rect::default();
            v.browser_area = Rect::default();
            if let Some(form) = &mut v.form {
                crate::pr_actions::write_back(form, drawn);
            }
        }
        return;
    }

    // ---- right: the reading side, the PULL REQUEST PAGE ----
    let current = cursor.and_then(|i| rows.get(i));
    let input = current.map(|pr| crate::pr_preview::PageInput {
        number: pr.number,
        title: &pr.title,
        detail: app.pr_detail.get(&pr.url),
        status: app.prs.status(&pr.url),
        freshness: crate::pr_preview::freshness(app, &pr.url),
        failed: app.pr_detail_failed.contains(&pr.url),
        posting: app.pr_comment_inflight.contains(&pr.url),
        browser_key: keys::BROWSER.label(),
        diff_key: keys::DIFF.label(),
        now,
    });
    // The frame says where it stands, the number and the title — the page
    // under it starts on the sentence — and at its right end what it
    // changes, then the `↗` that opens it in the browser.
    let (head, tail) = match &input {
        Some(input) => crate::pr_preview::border(input, page_focused, th),
        None => (
            vec![Span::styled("Pull request", Style::default().fg(th.muted))],
            Vec::new(),
        ),
    };
    let tail_w = if current.is_some() {
        crate::ui::border_tail_width(&tail)
    } else {
        0
    };
    // The corners, a cell of air either side of the title, the tail, and a
    // cell between the two.
    let room = (body_a.width as usize).saturating_sub(2 + 2 + tail_w as usize + 1);
    let head = crate::pr_preview::fit(head, room).spans;
    let head_w: usize = head.iter().map(|s| s.content.chars().count()).sum();
    let block = crate::ui::panel_block_spans(head, page_focused, th);
    let body_inner = block.inner(body_a);
    f.render_widget(block, body_a);
    let mut tabs = view.tabs.clone();
    let drawn = input.as_ref().map(|input| {
        let page =
            crate::pr_preview::page(input, &tabs, page_focused, body_inner.width as usize, th);
        crate::pr_preview::draw(f, body_inner, &page, &mut tabs, view.scroll)
    });
    // Where the body is read to, on the bottom border, once it scrolls.
    let (scroll, lines, body_h) = drawn.map_or((0, 0, 0), |d| (d.scroll, d.lines, d.body.height));
    let max_scroll = (lines as u16).saturating_sub(body_h.max(1));
    if max_scroll > 0 && body_a.height > 0 {
        let at = Line::from(Span::styled(
            format!(" {}/{} ", scroll + 1, lines),
            Style::default().fg(th.dim),
        ))
        .right_aligned();
        f.render_widget(
            Paragraph::new(at),
            Rect {
                x: body_a.x + 1,
                y: body_a.y + body_a.height - 1,
                width: body_a.width.saturating_sub(2),
                height: 1,
            },
        );
    }
    // The counts and the `↗` over the top border, once the block has
    // drawn it — only with a row to open.
    let browser_area = match current {
        Some(_) => crate::ui::border_tail(
            f,
            body_a,
            tail,
            (head_w + 2) as u16,
            app.hover_crumb == Some(HitTarget::ModalBrowser),
            th,
        ),
        None => Rect::default(),
    };
    // The modal's keys along its bottom edge — none while a box over it
    // has the keys: its own border says them.
    if !backdrop {
        let reserve = if max_scroll > 0 { 12 } else { 0 };
        crate::hints::draw_on_border(f, area, &hints(view), reserve, th);
    }

    // Write-back (draw works on a clone): the rects the mouse hit-tests,
    // the pane's size for paging, and the clamped cursor and scroll.
    if let Some(v) = record_list(app, area, rows_area, list_start, row_rects) {
        v.body_area = body_inner;
        v.browser_area = browser_area;
        v.view_height = body_h;
        v.body_lines = lines;
        v.tabs = tabs;
        // A cursor the filter had to move (see `cursor_index`) is settled
        // onto its row, URL and all, so a refresh follows that one.
        if let Some(index) = cursor {
            if index != v.selected {
                v.selected = index;
                v.selected_url = rows.get(index).map(|pr| pr.url.clone());
            }
            v.selected_row = cursor_row;
        }
        v.scroll = scroll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::{Checks, PrDetail, PrLaunch};
    use crate::quick_prompt::QuickTarget;
    use orion_core::WorktreeId;

    const DIR: &str = "/nonexistent/orion-pr-modal";

    fn pr(number: u64, title: &str, is_draft: bool) -> OpenPr {
        OpenPr {
            number,
            title: title.into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            answered_draft: is_draft,
            answered: Default::default(),
            head: format!("branch-{number}"),
            mine: false,
            head_sha: String::new(),
            meta: Default::default(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn cmd(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SUPER)
    }

    fn shifted(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    /// An app with one project (`demo`), its ROOT WORKTREE when `root`,
    /// and `list` as the project's open pull requests, landed just now.
    fn app_with(list: Vec<OpenPr>, root: bool) -> (App, ProjectId) {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.tree.projects.push(orion_core::Project {
            id: project.clone(),
            name: "demo".into(),
            repo_path: DIR.into(),
            sort_order: 0,
        });
        if root {
            app.tree.worktrees.push(orion_core::Worktree {
                id: WorktreeId("w-root".into()),
                project_id: project.clone(),
                path: DIR.into(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            });
        }
        let now = std::time::Instant::now();
        app.prs = observed(&list);
        app.open_prs.insert(
            project.clone(),
            crate::app::OpenPrs {
                list,
                at: now,
                due: now + std::time::Duration::from_secs(60),
                step: std::time::Duration::from_secs(60),
            },
        );
        (app, project)
    }

    /// `App::prs` as it stands once `list` has landed.
    fn observed(list: &[OpenPr]) -> PrStore {
        let mut prs = PrStore::default();
        for pr in list {
            prs.observe(
                &pr.url,
                crate::pr_store::PrObservation::of_list_row(pr),
                crate::fetch::Asked::Cached,
            );
        }
        prs
    }

    /// Config and presets pinned to temp files, so a launch resolves its
    /// harness off neither of the dev's own.
    fn pinned(f: impl FnOnce()) {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            crate::agent_presets::with_presets_path(dir.path().join("presets.json"), f)
        });
    }

    fn view(app: &App) -> &PullRequestsView {
        match &app.overlay {
            Some(Overlay::PullRequests(v)) => v,
            other => panic!("expected the pull requests modal, got {other:?}"),
        }
    }

    fn pending_url(app: &App) -> Option<&str> {
        app.pending_pr_detail.as_ref().map(|(p, _)| p.url.as_str())
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let Some(Overlay::PullRequests(v)) = app.overlay.clone() else {
                panic!("no pull requests modal");
            };
            draw(f, app, &v, app.theme, false);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The hotkey opens on the project's open list as it is — no second
    /// ask for a list that just landed — with the first row's body asked
    /// for on the debounce; an older list is asked for again underneath.
    #[test]
    fn opening_reads_the_first_pull_request_and_asks_only_for_a_stale_list() {
        let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
        open(&mut app);
        assert_eq!(view(&app).project, project);
        assert_eq!(view(&app).selected, 0);
        assert_eq!(
            view(&app).selected_url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
        assert_eq!(pending_url(&app), Some("https://github.com/o/r/pull/42"));
        assert!(!app.pr_refresh_requested, "fresh: nothing to ask");

        app.overlay = None;
        let stale = std::time::Instant::now()
            .checked_sub(FRESH + std::time::Duration::from_secs(1))
            .expect("machine up for a minute");
        app.open_prs.get_mut(&project).unwrap().at = stale;
        open(&mut app);
        assert!(app.pr_refresh_requested, "stale: asked on the next turn");
        assert!(app.open_prs_lookup_due(&project), "past its beat");
    }

    /// A body already read arms nothing; one the cache hydrated is shown
    /// and fetched fresh over the top, as the pane's is.
    #[test]
    fn a_read_body_arms_nothing_and_a_hydrated_one_is_fetched_again() {
        let (mut app, _) = app_with(vec![pr(42, "Fix login", false)], true);
        let url = "https://github.com/o/r/pull/42".to_string();
        app.pr_detail.insert(url.clone(), detail(42, "Fix login"));
        open(&mut app);
        assert_eq!(pending_url(&app), None, "read: nothing to ask");
        app.overlay = None;
        app.pr_detail_stale.insert(url.clone());
        open(&mut app);
        assert_eq!(pending_url(&app), Some(url.as_str()));
    }

    /// ↑/↓ walk the rows, each arming its own fetch; only
    /// Esc closes — the hotkey and `q` type into the filter, as every
    /// letter does — and a typed filter takes the first Esc.
    #[test]
    fn arrows_walk_the_rows_and_only_esc_closes() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
            true,
        );
        open(&mut app);
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(view(&app).selected, 1);
        assert_eq!(pending_url(&app), Some("https://github.com/o/r/pull/41"));
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(view(&app).selected, 1, "clamped at the last row");
        handle_key(&mut app, key(KeyCode::Up), &mut Vec::new());
        assert_eq!(view(&app).selected, 0);
        for letter in ['q', 'v'] {
            handle_key(&mut app, key(KeyCode::Char(letter)), &mut Vec::new());
            assert!(
                matches!(&app.overlay, Some(Overlay::PullRequests(_))),
                "{letter} types rather than closing"
            );
        }
        assert_eq!(view(&app).query.as_str(), "qv");
        handle_key(&mut app, key(KeyCode::Esc), &mut Vec::new());
        assert!(
            matches!(&app.overlay, Some(Overlay::PullRequests(_))),
            "the first Esc only clears the filter"
        );
        assert!(view(&app).query.is_empty());
        handle_key(&mut app, key(KeyCode::Esc), &mut Vec::new());
        assert!(app.overlay.is_none(), "the second closes");
    }

    /// A refresh that reorders the list keeps the cursor on its pull
    /// request; one that retired it leaves the cursor on the row that took
    /// its place, reading that one from the top.
    #[test]
    fn the_cursor_follows_its_pull_request_across_a_new_list() {
        let (mut app, project) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
            true,
        );
        open(&mut app);
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        // The loop fired #41's fetch: in flight, nothing armed.
        app.pending_pr_detail = None;
        app.pr_detail_inflight
            .begin("https://github.com/o/r/pull/41".into(), crate::fetch::now());
        app.open_prs.get_mut(&project).unwrap().list = vec![
            pr(43, "New", false),
            pr(42, "Fix login", false),
            pr(41, "Spike", true),
        ];
        list_changed(&mut app);
        assert_eq!(view(&app).selected, 2, "still on #41");
        assert_eq!(pending_url(&app), None, "same row: nothing re-armed");

        if let Some(Overlay::PullRequests(v)) = &mut app.overlay {
            v.scroll = 5;
        }
        app.open_prs.get_mut(&project).unwrap().list =
            vec![pr(43, "New", false), pr(42, "Fix login", false)];
        list_changed(&mut app);
        assert_eq!(view(&app).selected, 1, "#41 merged: its neighbour");
        assert_eq!(
            view(&app).selected_url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
        assert_eq!(view(&app).scroll, 0, "a different pull request");
        assert_eq!(pending_url(&app), Some("https://github.com/o/r/pull/42"));
    }

    /// A merged row's cursor lands on the row that took its place on
    /// screen. GitHub's order is not the drawn one — your own pull
    /// requests are gathered at the top — so the row that took its index
    /// in the list may sit in another section entirely.
    #[test]
    fn a_retired_row_hands_the_cursor_to_its_neighbour_on_screen() {
        let mine = |number, title| OpenPr {
            mine: true,
            ..pr(number, title, false)
        };
        // Drawn: #43, #41 (yours), then #42, #40.
        let (mut app, project) = app_with(
            vec![
                mine(43, "Mine A"),
                pr(42, "Theirs A", false),
                mine(41, "Mine B"),
                pr(40, "Theirs B", false),
            ],
            true,
        );
        open(&mut app);
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(
            view(&app).selected_url.as_deref(),
            Some("https://github.com/o/r/pull/41")
        );
        app.open_prs.get_mut(&project).unwrap().list = vec![
            mine(43, "Mine A"),
            pr(42, "Theirs A", false),
            pr(40, "Theirs B", false),
        ];
        list_changed(&mut app);
        assert_eq!(
            view(&app).selected_url.as_deref(),
            Some("https://github.com/o/r/pull/42"),
            "the row drawn under #41, not #40 at its old index"
        );
    }

    /// Enter on the list opens the QUICK PROMPT for a PR SESSION on the
    /// row under the cursor — the box's own Tab and Shift+Tab pick a
    /// harness or a preset for it, so the modal keeps neither.
    #[test]
    fn enter_starts_a_pr_session_on_the_row() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            let expected = PrLaunch {
                url: "https://github.com/o/r/pull/41".into(),
                head: "branch-41".into(),
                number: 41,
            };
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, key(KeyCode::Enter), &mut Vec::new());
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("Enter: expected the box, got {:?}", app.overlay);
            };
            let PromptKind::QuickPrompt(launch) = &prompt.kind else {
                panic!("{:?}", prompt.kind);
            };
            assert_eq!(launch.pr.as_ref(), Some(&expected));
            assert_eq!(
                launch.target,
                QuickTarget::Worktree(WorktreeId("w-root".into()))
            );
            assert!(prompt.title.contains("PR #41"), "{}", prompt.title);
        });
    }

    /// Tab and Shift+Tab (either spelling a terminal has for it) hand the
    /// keys between the list and the page, as the DIFF VIEWER's panels
    /// do: on the page ←/→ walk its tabs, ↑/↓ a listing's rows, and Enter
    /// acts on the row; a letter typed there hands the keys back to the
    /// list's filter, and Esc steps back to the list before it closes.
    #[test]
    fn tab_moves_the_keys_between_the_list_and_the_page() {
        use crate::pr_preview::PrTab;
        use crate::pull_request::PrFile;
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Spike", false)],
            true,
        );
        let url = "https://github.com/o/r/pull/42".to_string();
        let mut d = detail(42, "Fix login");
        d.files = ["src/login.rs", "src/auth.rs"]
            .iter()
            .map(|path| PrFile {
                path: path.to_string(),
                additions: 2,
                deletions: 1,
                change: "MODIFIED".into(),
            })
            .collect();
        app.pr_detail.insert(url.clone(), d);
        open(&mut app);
        let mut out = Vec::new();
        assert_eq!(view(&app).focus, PrFocus::List);
        handle_key(&mut app, key(KeyCode::Tab), &mut out);
        assert_eq!(view(&app).focus, PrFocus::Page);
        assert!(
            matches!(&app.overlay, Some(Overlay::PullRequests(_))),
            "Tab launches nothing"
        );
        let shot = screen(&mut app, 140, 34);
        assert!(shot.contains("←→ tabs"), "{shot}");
        assert!(shot.contains("⇧Tab list"), "{shot}");
        crate::hints::assert_hints_from(&hints(view(&app)), keys::ALL);

        // ←/→ walk the tabs, ↑/↓ the Changes rows — the list stays put.
        handle_key(&mut app, key(KeyCode::Right), &mut out);
        assert_eq!(view(&app).tabs.tab, PrTab::Changes);
        screen(&mut app, 140, 34);
        handle_key(&mut app, key(KeyCode::Down), &mut out);
        assert_eq!(view(&app).tabs.row(), 1);
        assert_eq!(view(&app).selected, 0, "the list's cursor never moved");
        handle_key(&mut app, key(KeyCode::Enter), &mut out);
        assert_eq!(app.pr_diff_at, Some((url, "src/auth.rs".to_string())));
        app.overlay = None;
        open(&mut app);

        for back in [key(KeyCode::BackTab), shifted(KeyCode::Tab)] {
            handle_key(&mut app, key(KeyCode::Tab), &mut out);
            assert_eq!(view(&app).focus, PrFocus::Page);
            handle_key(&mut app, back, &mut out);
            assert_eq!(view(&app).focus, PrFocus::List, "{back:?}");
        }

        // A letter on the page types into the list's filter.
        handle_key(&mut app, key(KeyCode::Tab), &mut out);
        handle_key(&mut app, key(KeyCode::Char('s')), &mut out);
        assert_eq!(view(&app).focus, PrFocus::List);
        assert_eq!(view(&app).query.as_str(), "s");

        // Esc: off the page, then the filter, then the modal.
        handle_key(&mut app, key(KeyCode::Tab), &mut out);
        handle_key(&mut app, key(KeyCode::Esc), &mut out);
        assert_eq!(view(&app).focus, PrFocus::List);
        assert_eq!(view(&app).query.as_str(), "s", "the filter kept");
        handle_key(&mut app, key(KeyCode::Esc), &mut out);
        assert!(view(&app).query.is_empty());
        handle_key(&mut app, key(KeyCode::Esc), &mut out);
        assert!(app.overlay.is_none());
    }

    /// `⌘N` opens the new pull request form in the reading pane's
    /// place, from the branch the Worktrees cursor's checkout is on, and
    /// `⌘X` the merge of the row under the cursor; Esc puts the page
    /// back either way, and the form has every key meanwhile.
    #[test]
    fn the_forms_open_in_the_pages_place() {
        use crate::pr_actions::{CreateField, PrForm};
        pinned(|| {
            let dir = tempfile::tempdir().unwrap();
            let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
            app.tree.projects[0].repo_path = dir.path().to_path_buf();
            app.tree.worktrees.push(orion_core::Worktree {
                id: WorktreeId("w-feature".into()),
                project_id: project.clone(),
                path: dir.path().join("feature"),
                branch: "feature/login".into(),
                is_main: false,
                sort_order: 1,
            });
            app.sel_worktree = app
                .worktree_rows()
                .iter()
                .position(|row| row.checkout().is_some_and(|w| w.id.0 == "w-feature"))
                .expect("the feature checkout has a row");
            open(&mut app);
            let mut out = Vec::new();
            handle_key(&mut app, ctrl('n'), &mut out);
            let Some(form) = &view(&app).form else {
                panic!("no form");
            };
            let PrForm::Create(create) = form.as_ref() else {
                panic!("{form:?}");
            };
            assert_eq!(create.from.as_str(), "feature/login");
            assert_eq!(create.field, CreateField::Title);
            handle_key(&mut app, key(KeyCode::Char('q')), &mut out);
            assert!(view(&app).query.is_empty(), "the form took the letter");
            let shot = screen(&mut app, 140, 34);
            assert!(shot.contains("New pull request"), "{shot}");
            assert!(shot.contains("From   feature/login"), "{shot}");
            assert!(shot.contains("Enter create PR"), "{shot}");
            // Enter with nothing to merge into says so and sends nothing.
            handle_key(&mut app, key(KeyCode::Enter), &mut out);
            let Some(PrForm::Create(create)) = view(&app).form.as_deref() else {
                panic!("the form stays");
            };
            assert_eq!(
                create.notice.as_deref(),
                Some("which branch does it merge into?")
            );
            handle_key(&mut app, key(KeyCode::Esc), &mut out);
            assert!(view(&app).form.is_none());
            assert!(matches!(&app.overlay, Some(Overlay::PullRequests(_))));

            handle_key(&mut app, ctrl('x'), &mut out);
            let Some(PrForm::Merge(merge)) = view(&app).form.as_deref() else {
                panic!("no merge form");
            };
            assert_eq!(merge.number, 42);
            let shot = screen(&mut app, 140, 34);
            assert!(shot.contains("Merge #42"), "{shot}");
            assert!(shot.contains("squash and merge"), "{shot}");
            assert!(shot.contains("its details are still loading"), "{shot}");
            assert!(shot.contains("known once its details are in"), "{shot}");
            // The body lands: the form takes the branch and the base.
            let url = "https://github.com/o/r/pull/42".to_string();
            let mut d = detail(42, "Fix login");
            d.head = "pr-42".into();
            if let Some(Overlay::PullRequests(v)) = &mut app.overlay {
                if let Some(PrForm::Merge(m)) = v.form.as_deref_mut() {
                    m.head = "pr-42".into();
                }
            }
            app.open_prs.get_mut(&project).unwrap().list[0].head = "pr-42".into();
            app.pr_detail.insert(url.clone(), d);
            crate::pr_actions::detail_landed(&mut app, &url);
            let Some(PrForm::Merge(merge)) = view(&app).form.as_deref() else {
                panic!("the form stays");
            };
            assert_eq!(merge.branch.as_deref(), Some("pr-42"));
            assert_eq!(merge.base, "main");
            assert!(merge.delete_branch, "the setting's default");
            let shot = screen(&mut app, 140, 34);
            assert!(shot.contains("pr-42 → main"), "{shot}");
            assert!(shot.contains("pr-42 on GitHub, once merged"), "{shot}");
            handle_key(&mut app, key(KeyCode::Right), &mut out);
            let Some(PrForm::Merge(merge)) = view(&app).form.as_deref() else {
                panic!("the form stays");
            };
            assert_eq!(merge.method, crate::pr_actions::MergeMethod::Merge);
            handle_key(&mut app, key(KeyCode::Esc), &mut out);
            assert!(view(&app).form.is_none());
        });
    }

    /// A merge form opened on a page the cache hydrated says what that
    /// page said; a live page landing while it is open brings the warnings
    /// up to date — a conflict resolved since is no longer one to resolve.
    #[test]
    fn the_merge_forms_warnings_follow_a_fresh_page() {
        use crate::fetch::Asked;
        use crate::pr_actions::PrForm;
        use crate::pr_store::PrObservation;
        pinned(|| {
            let (mut app, _) = app_with(vec![pr(42, "Fix login", false)], true);
            let url = "https://github.com/o/r/pull/42".to_string();
            let mut cached = detail(42, "Fix login");
            cached.answered.conflicts = Some(true);
            app.prs
                .observe(&url, PrObservation::of_detail(&cached), Asked::Cached);
            app.pr_detail.insert(url.clone(), cached);
            app.pr_detail_stale.insert(url.clone());
            open(&mut app);
            handle_key(&mut app, ctrl('x'), &mut Vec::new());
            let warnings = |app: &App| match view(app).form.as_deref() {
                Some(PrForm::Merge(merge)) => merge.warnings.clone(),
                other => panic!("no merge form: {other:?}"),
            };
            let conflicts = "it has conflicts to resolve first".to_string();
            assert!(warnings(&app).contains(&conflicts), "{:?}", warnings(&app));

            let mut fresh = detail(42, "Fix login");
            fresh.answered.conflicts = Some(false);
            let asked = Asked::At(crate::fetch::now());
            app.prs
                .observe(&url, PrObservation::of_detail(&fresh), asked);
            app.pr_detail.insert(url.clone(), fresh);
            crate::pr_actions::detail_landed(&mut app, &url);
            assert!(!warnings(&app).contains(&conflicts), "{:?}", warnings(&app));
        });
    }

    /// `⌘W` opens the close form on the row: the caret in its comment,
    /// the branch kept until it is known to be ours and ticked. A close
    /// that lands puts the page back and says so; a refused one says why
    /// on the form.
    #[test]
    fn cmd_w_closes_the_row_through_its_form() {
        use crate::pr_actions::{Answer, CloseRow, PrForm};
        pinned(|| {
            let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
            open(&mut app);
            let mut out = Vec::new();
            handle_key(&mut app, ctrl('w'), &mut out);
            let Some(PrForm::Close(close)) = view(&app).form.as_deref() else {
                panic!("no close form");
            };
            assert_eq!(close.number, 42);
            assert_eq!(close.row, CloseRow::Comment);
            for c in "dup".chars() {
                handle_key(&mut app, key(KeyCode::Char(c)), &mut out);
            }
            assert!(view(&app).query.is_empty(), "the form took the letters");
            // Space on the box does nothing while the branch is unknown.
            handle_key(&mut app, key(KeyCode::Tab), &mut out);
            handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
            let Some(PrForm::Close(close)) = view(&app).form.as_deref() else {
                panic!("the form stays");
            };
            assert_eq!(close.comment.as_str(), "dup");
            assert_eq!(close.row, CloseRow::DeleteBranch);
            assert!(!close.delete_branch);
            let shot = screen(&mut app, 140, 34);
            assert!(shot.contains("Close #42"), "{shot}");
            assert!(shot.contains("closes without merging"), "{shot}");
            assert!(shot.contains("known once its details are in"), "{shot}");
            assert!(shot.contains("Enter close PR"), "{shot}");

            // The body lands: the branch is ours, and Space ticks it.
            let url = "https://github.com/o/r/pull/42".to_string();
            let mut d = detail(42, "Fix login");
            d.head = "branch-42".into();
            app.pr_detail.insert(url.clone(), d);
            crate::pr_actions::detail_landed(&mut app, &url);
            handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
            let Some(PrForm::Close(close)) = view(&app).form.as_deref() else {
                panic!("the form stays");
            };
            assert_eq!(close.branch.as_deref(), Some("branch-42"));
            assert!(close.delete_branch);
            let ticket = close.ticket;
            let shot = screen(&mut app, 140, 34);
            assert!(shot.contains("branch-42 → main"), "{shot}");
            assert!(shot.contains("branch-42 on GitHub, once closed"), "{shot}");

            crate::pr_actions::land_answer(
                &mut app,
                Answer::Closed {
                    project: project.clone(),
                    ticket,
                    result: Err("not allowed".into()),
                },
            );
            let Some(PrForm::Close(close)) = view(&app).form.as_deref() else {
                panic!("a refused close keeps the form");
            };
            assert_eq!(close.notice.as_deref(), Some("not allowed"));
            assert_eq!(close.comment.as_str(), "dup", "nothing typed is lost");

            crate::pr_actions::land_answer(
                &mut app,
                Answer::Closed {
                    project,
                    ticket,
                    result: Ok("closed #42, branch deleted".into()),
                },
            );
            assert!(view(&app).form.is_none());
            assert!(app.pr_refresh_requested, "the list is asked for again");
            let flash = format!("{:?}", app.flash);
            assert!(flash.contains("closed #42, branch deleted"), "{flash}");
        });
    }

    /// Enter on the Reviews tab — and `⌘⇧R` from anywhere — opens the
    /// review form: ←/→ pick the verdict, one that needs a word asks for
    /// it, and a review sent closes onto the Reviews tab, the pull request
    /// read again.
    #[test]
    fn the_reviews_tab_reviews_through_its_form() {
        use crate::pr_actions::{Answer, PrForm, ReviewRow, ReviewVerdict};
        pinned(|| {
            let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
            open(&mut app);
            let mut out = Vec::new();
            handle_key(&mut app, key(KeyCode::Tab), &mut out);
            if let Some(Overlay::PullRequests(v)) = &mut app.overlay {
                v.tabs.switch(PrTab::Reviews);
            }
            assert!(hints(view(&app)).iter().any(|h| h.does == "review"));
            handle_key(&mut app, key(KeyCode::Enter), &mut out);
            let Some(PrForm::Review(form)) = view(&app).form.as_deref() else {
                panic!("no review form");
            };
            assert_eq!(form.number, 42);
            assert_eq!(form.verdict, ReviewVerdict::Approve);
            assert_eq!(form.row, ReviewRow::Verdict);
            let shot = screen(&mut app, 140, 34);
            assert!(shot.contains("Review #42"), "{shot}");
            assert!(shot.contains("Enter approve"), "{shot}");

            // → asks for changes, which needs a word: Enter says so and
            // puts the caret in the box.
            handle_key(&mut app, key(KeyCode::Right), &mut out);
            handle_key(&mut app, key(KeyCode::Enter), &mut out);
            let Some(PrForm::Review(form)) = view(&app).form.as_deref() else {
                panic!("the form stays");
            };
            assert_eq!(form.verdict, ReviewVerdict::RequestChanges);
            assert_eq!(form.row, ReviewRow::Body);
            assert_eq!(
                form.notice.as_deref(),
                Some("Request changes needs a comment")
            );
            for c in "nit".chars() {
                handle_key(&mut app, key(KeyCode::Char(c)), &mut out);
            }
            let Some(PrForm::Review(form)) = view(&app).form.as_deref() else {
                panic!("the form stays");
            };
            assert_eq!(form.body.as_str(), "nit");
            assert!(form.notice.is_none(), "typing clears the notice");
            assert!(view(&app).query.is_empty(), "the form took the letters");
            let ticket = form.ticket;

            crate::pr_actions::land_answer(
                &mut app,
                Answer::Reviewed {
                    project: project.clone(),
                    url: "https://github.com/o/r/pull/42".into(),
                    ticket,
                    result: Err("not allowed".into()),
                },
            );
            let Some(PrForm::Review(form)) = view(&app).form.as_deref() else {
                panic!("a refused review keeps the form");
            };
            assert_eq!(form.notice.as_deref(), Some("not allowed"));
            assert_eq!(form.body.as_str(), "nit", "nothing typed is lost");

            crate::pr_actions::land_answer(
                &mut app,
                Answer::Reviewed {
                    project,
                    url: "https://github.com/o/r/pull/42".into(),
                    ticket,
                    result: Ok("requested changes on #42".into()),
                },
            );
            assert!(view(&app).form.is_none());
            assert_eq!(view(&app).tabs.tab, PrTab::Reviews);
            assert!(app.pr_refresh_requested, "the list is asked for again");
            assert!(app
                .pr_detail_stale
                .contains("https://github.com/o/r/pull/42"));
            let flash = format!("{:?}", app.flash);
            assert!(flash.contains("requested changes on #42"), "{flash}");

            // ⌘⇧R from the list opens it too.
            handle_key(&mut app, key(KeyCode::Esc), &mut out);
            handle_key(
                &mut app,
                KeyEvent::new(
                    KeyCode::Char('r'),
                    KeyModifiers::SUPER | KeyModifiers::SHIFT,
                ),
                &mut out,
            );
            assert!(matches!(
                view(&app).form.as_deref(),
                Some(PrForm::Review(_))
            ));
        });
    }

    /// On the user's own pull request GitHub takes only a comment: that is
    /// the one verdict, and the caret starts in the box.
    #[test]
    fn your_own_pull_request_is_only_commented_on() {
        use crate::pr_actions::{PrForm, ReviewRow, ReviewVerdict};
        pinned(|| {
            let mut mine = pr(7, "Mine", false);
            mine.mine = true;
            let (mut app, _) = app_with(vec![mine], true);
            open(&mut app);
            let mut out = Vec::new();
            handle_key(
                &mut app,
                KeyEvent::new(
                    KeyCode::Char('r'),
                    KeyModifiers::SUPER | KeyModifiers::SHIFT,
                ),
                &mut out,
            );
            handle_key(&mut app, key(KeyCode::Tab), &mut out);
            handle_key(&mut app, key(KeyCode::Right), &mut out);
            let Some(PrForm::Review(form)) = view(&app).form.as_deref() else {
                panic!("no review form");
            };
            assert_eq!(form.verdict, ReviewVerdict::Comment);
            assert_eq!(form.row, ReviewRow::Verdict);
            let shot = screen(&mut app, 140, 34);
            assert!(shot.contains("only a comment"), "{shot}");
            crate::hints::assert_hints_from(&hints(view(&app)), crate::pr_actions::keys::ALL);
        });
    }

    /// A draft flipped lands in the footer and has the pull request read
    /// again, so the page and the badge follow.
    #[test]
    fn a_flipped_draft_is_read_again() {
        use crate::pr_actions::Answer;
        let (mut app, project) = app_with(vec![pr(42, "Fix login", true)], true);
        open(&mut app);
        let url = "https://github.com/o/r/pull/42".to_string();
        app.pr_detail.insert(url.clone(), detail(42, "Fix login"));
        app.pending_pr_detail = None;
        crate::pr_actions::land_answer(
            &mut app,
            Answer::Readied {
                project,
                url: url.clone(),
                number: 42,
                ready: true,
                result: Ok(()),
            },
        );
        assert!(app.pr_detail_stale.contains(&url));
        assert_eq!(pending_url(&app), Some(url.as_str()));
        assert!(app.pr_refresh_requested);
        let flash = format!("{:?}", app.flash);
        assert!(flash.contains("#42 is ready for review"), "{flash}");
    }

    /// `⌘Y` (`^Y` with no ⌘) opens the COMMENT BOX on the row, carrying the
    /// modal; Esc puts the modal back on the same pull request.
    #[test]
    fn c_opens_the_comment_box_and_esc_comes_back_to_the_row() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, ctrl('y'), &mut Vec::new());
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("Ctrl+y: expected the comment box, got {:?}", app.overlay);
            };
            assert!(prompt.is_multiline());
            assert_eq!(prompt.title, "Comment on #41 Spike");
            let PromptKind::PrComment { number, back, .. } = &prompt.kind else {
                panic!("{:?}", prompt.kind);
            };
            assert_eq!(*number, 41);
            assert_eq!(back.as_ref().map(|v| v.selected), Some(1));

            let mut out = Vec::new();
            crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Esc), &mut out);
            assert_eq!(view(&app).selected, 1, "back on #41");
        });
    }

    /// `⌘Y` is `^Y`: the grid's `y` (reply) as a chord, onto the same
    /// COMMENT BOX for the same row.
    #[test]
    fn cmd_y_opens_the_comment_box_as_ctrl_y_does() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, cmd('y'), &mut Vec::new());
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("⌘Y: expected the comment box, got {:?}", app.overlay);
            };
            let PromptKind::PrComment { number, back, .. } = &prompt.kind else {
                panic!("{:?}", prompt.kind);
            };
            assert_eq!(*number, 41);
            assert_eq!(
                back.as_ref().map(|v| v.query.as_str()),
                Some(""),
                "not typed into the filter"
            );
        });
    }

    /// `Enter` puts the QUICK PROMPT up over the modal, not in its place:
    /// the list stays on screen under the box, the box's Esc leaves the
    /// modal on the pull request it was opened on, and its launch closes
    /// the modal with it.
    #[test]
    fn enter_stacks_the_box_over_the_modal() {
        pinned(|| {
            let (mut app, _) = app_with(
                vec![pr(42, "Fix login", false), pr(41, "Spike", true)],
                true,
            );
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
            handle_key(&mut app, key(KeyCode::Enter), &mut Vec::new());
            let Some(Overlay::Prompt(prompt)) = &app.overlay else {
                panic!("expected the box, got {:?}", app.overlay);
            };
            let PromptKind::QuickPrompt(launch) = &prompt.kind else {
                panic!("{:?}", prompt.kind);
            };
            assert!(
                matches!(&launch.under, Some(ModalUnder::PullRequests(v)) if v.selected == 1),
                "{:?}",
                launch.under
            );
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
            term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let buf = term.backend().buffer();
            let screen: String = buf.content().iter().map(|c| c.symbol()).collect();
            assert!(
                screen.contains("Pull requests — demo"),
                "the modal under the box"
            );
            assert!(screen.contains("New agent · PR #41"), "the box over it");
            assert!(
                screen.contains("Esc back to pull requests"),
                "and says where Esc goes"
            );

            let mut out = Vec::new();
            crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Esc), &mut out);
            assert_eq!(view(&app).selected, 1, "Esc: back on #41");

            handle_key(&mut app, key(KeyCode::Enter), &mut Vec::new());
            for c in "review it".chars() {
                crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Char(c)), &mut out);
            }
            crate::event_loop::handle_overlay_key(&mut app, key(KeyCode::Enter), &mut out);
            assert!(
                out.iter().any(|r| matches!(
                    r,
                    orion_core::ClientRequest::CreatePrAgent { pr_url, .. }
                        if pr_url == "https://github.com/o/r/pull/41"
                )),
                "the PR session is created: {out:?}"
            );
            assert!(app.overlay.is_none(), "the launch closes the modal");
        });
    }

    fn detail(number: u64, title: &str) -> PrDetail {
        PrDetail {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: title.into(),
            answered_state: crate::pull_request::STATE_OPEN.into(),
            answered_draft: false,
            answered: Default::default(),
            author: "webdevcody".into(),
            base: "main".into(),
            head: format!("branch-{number}"),
            additions: 3,
            deletions: 1,
            changed_files: 2,
            body: "Stops the login bounce.".into(),
            comments: vec![],
            ..Default::default()
        }
    }

    /// The list reads like the group — a draft and a pull request GitHub
    /// says cannot merge wear their badge — and the pane reads the row
    /// under the cursor: its number and title on the border while the body
    /// is on its way, where it stands and what it changes once it lands.
    #[test]
    fn it_draws_the_rows_and_reads_the_one_under_the_cursor() {
        let mut failing = pr(40, "Bump deps", false);
        failing.answered = crate::pull_request::Answered {
            conflicts: Some(false),
            checks: Some(Checks::Failing),
        };
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), failing, pr(41, "Spike", true)],
            true,
        );
        open(&mut app);
        let before = screen(&mut app, 120, 30);
        assert!(before.contains("Pull requests — demo (3)"), "{before}");
        assert!(before.contains("↗ #42  Fix login"), "{before}");
        assert!(before.contains("ready"), "{before}");
        assert!(before.contains("failing"), "{before}");
        assert!(before.contains("draft"), "{before}");
        assert!(before.contains("╮╭ #42 Fix login ─"), "{before}");
        assert!(before.contains("reading it…"), "{before}");
        assert!(view(&app).list_area.height > 0, "rects written back");

        app.pr_detail.insert(
            "https://github.com/o/r/pull/42".into(),
            detail(42, "Fix login"),
        );
        let after = screen(&mut app, 120, 30);
        assert!(after.contains("╭ ● Open  #42 Fix login ─"), "{after}");
        assert!(after.contains("─ +3 −1  ↗ ╮"), "{after}");
        assert!(after.contains("Stops the login bounce."), "{after}");
        assert!(!after.contains("reading it…"), "{after}");

        app.pr_comment_inflight
            .insert("https://github.com/o/r/pull/42".into());
        let posting = screen(&mut app, 120, 30);
        assert!(posting.contains("posting your comment…"), "{posting}");
    }

    /// The reading side is the PULL REQUEST PAGE: its tabs over the body,
    /// walked with ⇧←/⇧→ round either end — ↑/↓ still the list's — a
    /// listing's rows with ⇧↑/⇧↓; `⌘E` diffs the file or the commit under
    /// the cursor and `⌘O` opens the check, and the border names each by
    /// what it reaches there, from the modal's own table. A click on a tab
    /// shows it and a click on a row acts on it. Moving to another pull
    /// request rewinds the rows and keeps the tab.
    #[test]
    fn the_reading_side_walks_the_pages_tabs_and_rows() {
        use crate::pr_preview::PrTab;
        use crate::pull_request::{CheckState, PrCheck, PrCommit, PrFile};
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Spike", false)],
            true,
        );
        let url = "https://github.com/o/r/pull/42".to_string();
        let mut d = detail(42, "Fix login");
        d.files = ["src/login.rs", "src/auth.rs"]
            .iter()
            .map(|path| PrFile {
                path: path.to_string(),
                additions: 2,
                deletions: 1,
                change: "MODIFIED".into(),
            })
            .collect();
        d.commits = vec![PrCommit {
            sha: "abcdef0123".into(),
            subject: "Stop the bounce".into(),
            author: "kate".into(),
            at: String::new(),
            ..Default::default()
        }];
        d.checks = vec![PrCheck {
            name: "test".into(),
            workflow: String::new(),
            state: CheckState::Failed,
            word: "FAILURE".into(),
            started: String::new(),
            completed: String::new(),
            url: "https://github.com/o/r/actions/runs/9".into(),
        }];
        app.pr_detail.insert(url.clone(), d);
        open(&mut app);
        let shot = screen(&mut app, 140, 34);
        assert!(
            shot.contains("Description   Changes 2   Commits 1   Checks ✗ 0/1   Reviews"),
            "{shot}"
        );
        assert!(shot.contains("Stops the login bounce."), "{shot}");
        assert!(shot.contains("⇧←/⇧→ tabs"), "{shot}");

        let mut out = Vec::new();
        handle_key(&mut app, shifted(KeyCode::Left), &mut out);
        assert_eq!(view(&app).tabs.tab, PrTab::Reviews, "round the left end");
        handle_key(&mut app, shifted(KeyCode::Right), &mut out);
        handle_key(&mut app, shifted(KeyCode::Right), &mut out);
        assert_eq!(view(&app).tabs.tab, PrTab::Changes);
        assert!(view(&app).query.is_empty(), "the filter never saw them");
        let shot = screen(&mut app, 140, 34);
        let row = shot
            .lines()
            .find(|l| l.contains("src/login.rs"))
            .unwrap_or_default();
        assert!(row.contains("│▌ M  src/login.rs"), "{shot}");
        assert!(row.contains(" +2 −1 │"), "{row:?}");
        assert!(
            hints(view(&app)).iter().any(|h| h.does == "diff the file"),
            "{shot}"
        );
        crate::hints::assert_hints_from(&hints(view(&app)), keys::ALL);
        handle_key(&mut app, shifted(KeyCode::Down), &mut out);
        assert_eq!(view(&app).tabs.row(), 1);
        handle_key(&mut app, ctrl('e'), &mut out);
        assert_eq!(
            app.pr_diff_at,
            Some((url.clone(), "src/auth.rs".to_string()))
        );

        // ⌘O on Checks: the check, not the pull request.
        handle_key(&mut app, shifted(KeyCode::Right), &mut out);
        handle_key(&mut app, shifted(KeyCode::Right), &mut out);
        assert_eq!(view(&app).tabs.tab, PrTab::Checks);
        screen(&mut app, 140, 34);
        crate::hints::assert_hints_from(&hints(view(&app)), keys::ALL);
        crate::event_loop::take_opened();
        handle_key(&mut app, ctrl('o'), &mut out);
        assert_eq!(
            crate::event_loop::take_opened(),
            ["https://github.com/o/r/actions/runs/9"]
        );

        // The mouse: a tab's label, then a row.
        let (changes, _) = view(&app)
            .tabs
            .tab_hits
            .iter()
            .find(|(_, tab)| *tab == PrTab::Changes)
            .copied()
            .expect("the Changes label");
        let click = |at: Position| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        let at = Position::new(changes.x + 1, changes.y);
        handle_mouse(&mut app, click(at), at, &mut out);
        assert_eq!(view(&app).tabs.tab, PrTab::Changes);
        screen(&mut app, 140, 34);
        app.pr_diff_at = None;
        let (row, index) = view(&app).tabs.row_hits[0];
        assert_eq!(index, 0);
        let at = Position::new(row.x + 2, row.y);
        handle_mouse(&mut app, click(at), at, &mut out);
        assert_eq!(app.pr_diff_at, Some((url, "src/login.rs".to_string())));
        assert_eq!(
            view(&app).focus,
            PrFocus::Page,
            "a click hands the page the keys"
        );

        // Another pull request: the rows rewind, the tab stays.
        handle_key(&mut app, shifted(KeyCode::Down), &mut out);
        handle_key(&mut app, key(KeyCode::BackTab), &mut out);
        handle_key(&mut app, key(KeyCode::Down), &mut out);
        assert_eq!(
            (view(&app).tabs.tab, view(&app).tabs.row()),
            (PrTab::Changes, 0)
        );
        let shot = screen(&mut app, 140, 34);
        assert!(shot.contains("Changes …"), "#41 still loading: {shot}");
    }

    /// A `<details>` in a comment — Cubic's prompt for AI agents — opens
    /// on a click on its summary and shuts on another, as on github.com,
    /// and is shut again on another pull request.
    #[test]
    fn a_click_on_a_details_summary_opens_and_shuts_it() {
        use crate::pull_request::PrComment;
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Spike", false)],
            true,
        );
        let mut d = detail(42, "Fix login");
        d.comments = vec![PrComment {
            author: "cubic-dev-ai".into(),
            at: String::new(),
            review_state: String::new(),
            body: "2 issues found\n\n<details>\n<summary>Prompt for AI agents</summary>\n\n```text\nCheck the token expiry\n```\n\n</details>\n".into(),
        }];
        app.pr_detail
            .insert("https://github.com/o/r/pull/42".into(), d);
        open(&mut app);
        let shut = screen(&mut app, 140, 34);
        assert!(shut.contains("▸ Prompt for AI agents"), "{shut}");
        assert!(!shut.contains("Check the token expiry"), "{shut}");

        let click = |at: Position| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        let mut out = Vec::new();
        let (rect, _) = view(&app).tabs.fold_hits[0];
        let at = Position::new(rect.x + 4, rect.y);
        handle_mouse(&mut app, click(at), at, &mut out);
        let opened = screen(&mut app, 140, 34);
        assert!(opened.contains("▾ Prompt for AI agents"), "{opened}");
        assert!(opened.contains("Check the token expiry"), "{opened}");
        assert_eq!(view(&app).focus, PrFocus::Page);

        let (rect, _) = view(&app).tabs.fold_hits[0];
        let at = Position::new(rect.x + 4, rect.y);
        handle_mouse(&mut app, click(at), at, &mut out);
        let again = screen(&mut app, 140, 34);
        assert!(!again.contains("Check the token expiry"), "{again}");

        handle_mouse(&mut app, click(at), at, &mut out);
        assert!(!view(&app).tabs.folds.flipped(1).is_empty());
        handle_key(&mut app, key(KeyCode::BackTab), &mut out);
        handle_key(&mut app, key(KeyCode::Down), &mut out);
        assert!(
            view(&app).tabs.folds.flipped(1).is_empty(),
            "another pull request starts shut"
        );
    }

    /// An empty list says whether GitHub is still being asked or has said
    /// nothing is open.
    #[test]
    fn an_empty_list_says_why() {
        let (mut app, project) = app_with(vec![], true);
        open(&mut app);
        assert!(screen(&mut app, 100, 20).contains("no open pull requests"));
        app.open_prs.remove(&project);
        assert!(screen(&mut app, 100, 20).contains("asking GitHub…"));
    }

    /// A list GitHub could not be asked for says so, on a row of its own
    /// under the filter where a narrow modal cannot cut it off — rows that
    /// stopped refreshing must not pass for current ones (#106). The rows
    /// stay, and stay clickable under the note; a retry in flight says
    /// `refreshing…` instead, and an answer clears it.
    #[test]
    fn a_list_that_could_not_be_refreshed_says_so() {
        let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
        open(&mut app);
        let fine = screen(&mut app, 100, 20);
        assert!(!fine.contains("couldn't refresh"), "{fine}");
        let first_row = view(&app).list_area.y;

        app.open_prs_failed.insert(project.clone());
        let stale = screen(&mut app, 100, 20);
        assert!(stale.contains("couldn't refresh (^R retries)"), "{stale}");
        assert!(stale.contains("#42 Fix login"), "{stale}");
        assert_eq!(
            view(&app).list_area.y,
            first_row + 1,
            "the rows' hit area starts under the note"
        );

        let ticket = app
            .open_prs_inflight
            .begin(project.clone(), crate::fetch::now())
            .unwrap();
        // Wide enough for the title to say it in full.
        let retrying = screen(&mut app, 160, 20);
        assert!(!retrying.contains("couldn't refresh"), "{retrying}");
        assert!(retrying.contains("refreshing…"), "{retrying}");
        app.open_prs_inflight.land(&ticket);

        app.open_prs.get_mut(&project).unwrap().list = vec![];
        let never = screen(&mut app, 100, 20);
        assert!(never.contains("couldn't ask GitHub"), "{never}");
        assert!(!never.contains("no open pull requests"), "{never}");

        app.open_prs_failed.remove(&project);
        let answered = screen(&mut app, 100, 20);
        assert!(!answered.contains("couldn't"), "{answered}");
        assert!(answered.contains("no open pull requests"), "{answered}");
    }

    /// `⌘O` and a click on the reading pane's `↗` button run one open: the
    /// footer names where the browser went either way (INPUT PARITY), and
    /// the modal stays up. The button is drawn pinned right on the pane's
    /// top border, after what the pull request changes, its rect written
    /// back for the click; the pointer resting on it is what `hover_crumb`
    /// holds, and a cell to its left is the frame's.
    #[test]
    fn o_and_the_browser_button_open_the_pull_request_the_same_way() {
        let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
        app.overlay = Some(Overlay::PullRequests(PullRequestsView::new(
            project,
            "demo".into(),
            DIR.into(),
        )));
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("─ ↗ ╮"), "{shot}");
        let button = view(&app).browser_area;
        assert!(button.width > 0, "the button's rect is written back");
        let at = Position::new(button.x, button.y);
        assert_eq!(
            crate::ui::browser_button_under(&app, at),
            Some(HitTarget::ModalBrowser)
        );
        assert_eq!(
            crate::ui::browser_button_under(&app, Position::new(button.x - 1, button.y)),
            None
        );

        let mut out = Vec::new();
        handle_key(&mut app, ctrl('o'), &mut out);
        assert_eq!(
            crate::event_loop::take_opened(),
            ["https://github.com/o/r/pull/42"]
        );
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, at, &mut out);
        assert_eq!(
            crate::event_loop::take_opened(),
            ["https://github.com/o/r/pull/42"]
        );
        assert!(
            matches!(app.overlay, Some(Overlay::PullRequests(_))),
            "the modal stays up"
        );
    }

    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            handle_key(app, key(KeyCode::Char(c)), &mut Vec::new());
        }
    }

    /// Typing narrows the rows the moment the modal is up — no key to
    /// press first — to the fuzzy matches, best first, the cursor on the
    /// best with its body asked for as any move's is, the count reading
    /// `matches/all`; the modal's own hotkey types too. ↑/↓ walk the
    /// matches alone. The first Esc clears the filter, the cursor staying
    /// on the row it found, and the second closes.
    #[test]
    fn typing_filters_the_rows_at_once_and_esc_clears_before_closing() {
        let (mut app, _) = app_with(
            vec![
                pr(42, "Fix login", false),
                pr(41, "Docs pass", false),
                pr(40, "Login page", true),
            ],
            true,
        );
        open(&mut app);
        let esc = |app: &App| hints(view(app)).last().map(|h| h.does.clone());
        assert_eq!(esc(&app).as_deref(), Some("close"));
        type_str(&mut app, "login");
        assert_eq!(view(&app).query.as_str(), "login");
        assert_eq!(esc(&app).as_deref(), Some("clear"), "Esc clears first");
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("(2/3)"), "{shot}");
        assert!(
            !shot.contains("#41"),
            "the docs row is filtered out:\n{shot}"
        );
        let found = selected_pr(&app).expect("a row under the cursor");
        assert_ne!(found.number, 41);
        assert_eq!(
            pending_url(&app),
            Some(found.url.as_str()),
            "its body is asked for"
        );

        // `v` types, rather than closing.
        handle_key(&mut app, key(KeyCode::Char('v')), &mut Vec::new());
        assert_eq!(view(&app).query.as_str(), "loginv");
        assert!(matches!(&app.overlay, Some(Overlay::PullRequests(_))));
        handle_key(&mut app, key(KeyCode::Backspace), &mut Vec::new());

        let first = selected_pr(&app).unwrap().number;
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        let second = selected_pr(&app).unwrap().number;
        assert_ne!(first, second);
        assert_ne!(second, 41, "↓ walks the matches alone");
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(
            selected_pr(&app).unwrap().number,
            second,
            "and stops at the last one"
        );

        // The first Esc clears the filter, the cursor staying put; the
        // second closes.
        handle_key(&mut app, key(KeyCode::Esc), &mut Vec::new());
        assert!(matches!(&app.overlay, Some(Overlay::PullRequests(_))));
        assert!(view(&app).query.is_empty());
        assert_eq!(
            selected_pr(&app).unwrap().number,
            second,
            "the row found keeps the cursor"
        );
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("(3)"), "{shot}");
        assert!(shot.contains("type to filter…"), "{shot}");
        handle_key(&mut app, key(KeyCode::Esc), &mut Vec::new());
        assert!(app.overlay.is_none());
    }

    /// A filter nothing matches empties the list and says so — nothing
    /// under the cursor, no body asked for — and the row is back the
    /// moment the filter widens. Ctrl+u kills the typed filter, as in any
    /// line editor, and never scrolls the pane.
    #[test]
    fn a_filter_nothing_matches_says_so_and_leaves_the_cursor_put() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Docs pass", false)],
            true,
        );
        open(&mut app);
        select(&mut app, 1);
        assert_eq!(selected_pr(&app).unwrap().number, 41);
        type_str(&mut app, "fix");
        assert_eq!(
            selected_pr(&app).unwrap().number,
            42,
            "the cursor goes to the best match"
        );
        type_str(&mut app, "zzz");
        let shot = screen(&mut app, 120, 40);
        assert!(shot.contains("no pull requests match"), "{shot}");
        assert!(shot.contains("(0/2)"), "{shot}");
        assert!(selected_pr(&app).is_none(), "nothing under the cursor");
        assert_eq!(pending_url(&app), None);
        for _ in 0..3 {
            handle_key(&mut app, key(KeyCode::Backspace), &mut Vec::new());
        }
        assert_eq!(
            selected_pr(&app).unwrap().number,
            42,
            "the row is back as the filter widens"
        );
        if let Some(Overlay::PullRequests(v)) = &mut app.overlay {
            v.scroll = 3;
        }
        handle_key(&mut app, ctrl('u'), &mut Vec::new());
        assert!(view(&app).query.is_empty(), "Ctrl+u kills the typed filter");
        assert_eq!(view(&app).scroll, 3, "and does not scroll the pane");
        assert_eq!(
            selected_pr(&app).unwrap().number,
            42,
            "the row found keeps the cursor"
        );
        handle_key(&mut app, ctrl('u'), &mut Vec::new());
        assert_eq!(
            view(&app).scroll,
            3,
            "with nothing typed, it still does not scroll — PgUp does"
        );
    }

    /// A click on a row while a filter is typed picks that row — the row
    /// math counting the filter's matches, not the whole list — and the
    /// filter stays.
    #[test]
    fn a_row_click_counts_the_matches_not_the_list() {
        let (mut app, _) = app_with(
            vec![
                pr(42, "Fix login", false),
                pr(41, "Docs pass", false),
                pr(40, "Login page", false),
            ],
            true,
        );
        open(&mut app);
        type_str(&mut app, "login");
        screen(&mut app, 120, 40);
        let list = view(&app).list_area;
        // The second visible row, under the section's rule: the second
        // match, whichever it is.
        let at = Position::new(list.x + 1, list.y + 2);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.x,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, click, at, &mut Vec::new());
        assert_eq!(view(&app).query.as_str(), "login", "the filter is kept");
        let picked = selected_pr(&app).unwrap();
        let list = rows(&app, &view(&app).project);
        let visible = visible_rows("login", list, &app.prs);
        assert_eq!(picked.number, list[visible[1].0].number);
        assert_ne!(picked.number, 41);
    }

    /// A double-click on a row opens that pull request in the browser —
    /// the same open as `⌘O` and the button (INPUT PARITY) — and the
    /// modal stays up on the row. A single click only selects, and two
    /// clicks on different rows are two single clicks.
    #[test]
    fn a_double_click_on_a_row_opens_it_in_the_browser() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Docs pass", false)],
            true,
        );
        open(&mut app);
        screen(&mut app, 120, 40);
        let list = view(&app).list_area;
        let click_at = |app: &mut App, row: u16| {
            let at = Position::new(list.x + 1, list.y + row);
            let click = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: at.x,
                row: at.y,
                modifiers: KeyModifiers::NONE,
            };
            let mut out = Vec::new();
            handle_mouse(app, click, at, &mut out);
        };

        // The section's rule, then the rows.
        let opened = crate::event_loop::take_opened;
        click_at(&mut app, 2);
        assert_eq!(selected_pr(&app).unwrap().number, 41);
        assert!(opened().is_empty(), "one click only selects");
        click_at(&mut app, 1);
        assert_eq!(selected_pr(&app).unwrap().number, 42);
        assert!(
            opened().is_empty(),
            "a click on another row is a single click"
        );
        click_at(&mut app, 1);
        assert_eq!(
            opened(),
            ["https://github.com/o/r/pull/42"],
            "the second click on the row opens it"
        );
        assert!(
            matches!(app.overlay, Some(Overlay::PullRequests(_))),
            "the modal stays up"
        );
        click_at(&mut app, 1);
        assert!(
            opened().is_empty(),
            "a double-click is spent: the third click starts over"
        );
    }

    /// A bracketed paste lands in the filter as one line and narrows the
    /// rows as typing it would; with no modal up it is not this one's.
    #[test]
    fn a_paste_lands_in_the_filter_as_one_line() {
        let (mut app, _) = app_with(
            vec![pr(42, "Fix login", false), pr(41, "Docs pass", false)],
            true,
        );
        open(&mut app);
        assert!(paste(&mut app, "docs\npass"));
        assert_eq!(view(&app).query.as_str(), "docs pass");
        assert_eq!(selected_pr(&app).unwrap().number, 41);
        app.overlay = None;
        assert!(!paste(&mut app, "x"));
    }

    /// A refresh that retires the row under the cursor while a filter is
    /// typed lands the cursor on the filter's next match, never on a row
    /// the filter hides.
    #[test]
    fn a_refresh_under_a_filter_lands_on_a_visible_row() {
        let (mut app, project) = app_with(
            vec![
                pr(42, "Fix login", false),
                pr(41, "Docs pass", false),
                pr(40, "Login page", false),
            ],
            true,
        );
        open(&mut app);
        type_str(&mut app, "login");
        select(&mut app, 2);
        assert_eq!(selected_pr(&app).unwrap().number, 40);
        app.open_prs.get_mut(&project).unwrap().list =
            vec![pr(42, "Fix login", false), pr(41, "Docs pass", false)];
        list_changed(&mut app);
        assert_eq!(
            selected_pr(&app).unwrap().number,
            42,
            "not #41, which the filter hides"
        );
        screen(&mut app, 120, 40);
        assert_eq!(view(&app).selected, 0, "settled onto the row it shows");
        assert_eq!(
            view(&app).selected_url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
    }

    /// The verbs the letters used to be are chords now: ⌘R asks GitHub
    /// again and ⌘E — the grid's changes chord — asks for the diff, each
    /// on its `^` twin where the terminal sends no ⌘, while the plain
    /// letters go to the filter. ⌘X cuts a selection in the filter rather
    /// than merge.
    #[test]
    fn the_verb_chords_run_and_the_plain_letters_type() {
        let (mut app, project) = app_with(vec![pr(42, "Fix login", false)], true);
        open(&mut app);
        handle_key(&mut app, ctrl('r'), &mut Vec::new());
        assert_eq!(app.flash, None, "the title says it is refreshing");
        assert!(app.pr_refresh_requested);
        assert!(app.open_prs_lookup_due(&project));
        app.flash = None;
        for chord in [ctrl('e'), cmd('e')] {
            app.flash = None;
            handle_key(&mut app, chord, &mut Vec::new());
            assert!(
                app.flash
                    .as_deref()
                    .is_some_and(|f| f.starts_with("repo path missing on disk")),
                "{chord:?} reaches the diff fetch: {:?}",
                app.flash
            );
        }
        for letter in "rgoc".chars() {
            handle_key(&mut app, key(KeyCode::Char(letter)), &mut Vec::new());
        }
        assert_eq!(view(&app).query.as_str(), "rgoc");
        assert!(matches!(&app.overlay, Some(Overlay::PullRequests(_))));
        // ⌘A selects the filter's text and ⌘X cuts it — no merge form.
        handle_key(&mut app, cmd('a'), &mut Vec::new());
        handle_key(&mut app, cmd('x'), &mut Vec::new());
        assert_eq!(view(&app).query.as_str(), "", "⌘X cut the selection");
        assert!(view(&app).form.is_none(), "and did not merge");
        handle_key(&mut app, cmd('x'), &mut Vec::new());
        assert!(
            matches!(
                view(&app).form.as_deref(),
                Some(crate::pr_actions::PrForm::Merge(_))
            ),
            "with nothing selected, ⌘X merges"
        );
    }

    /// On a narrow frame the title gives way to the button, never the
    /// other way round; with no pull request to open there is no button,
    /// and no stale rect.
    #[test]
    fn the_title_gives_way_to_the_browser_button_and_an_empty_list_has_none() {
        let title = "A title long enough to crowd a narrow frame";
        let (mut app, project) = app_with(vec![pr(42, title, false)], true);
        app.overlay = Some(Overlay::PullRequests(PullRequestsView::new(
            project,
            "demo".into(),
            DIR.into(),
        )));
        let shot = screen(&mut app, 60, 20);
        let top = shot.lines().find(|l| l.contains('╭')).unwrap_or_default();
        assert!(top.contains("#42 A title") && top.contains("…"), "{shot}");
        assert!(top.contains(" ↗ ╮"), "{shot}");
        assert!(view(&app).browser_area.width > 0);

        let (mut app, project) = app_with(vec![], true);
        app.overlay = Some(Overlay::PullRequests(PullRequestsView::new(
            project,
            "demo".into(),
            DIR.into(),
        )));
        let shot = screen(&mut app, 120, 40);
        assert!(!shot.contains("↗"), "{shot}");
        assert_eq!(view(&app).browser_area, Rect::default());
    }

    /// A row with the meta the full list query brings: opened by `author`
    /// an hour ago, its checks and review as given, mine or not.
    fn rich(number: u64, title: &str, author: &str, mine: bool) -> OpenPr {
        use crate::pull_request::{CheckTally, PrLabel, PrMeta};
        let hour_ago = orion_core::clock::now_secs() - 3600;
        OpenPr {
            mine,
            meta: PrMeta {
                author: author.into(),
                created_at: orion_core::crashlog::format_timestamp(hour_ago),
                review_requested: false,
                review: Review::Approved,
                comments: 5,
                labels: vec![PrLabel {
                    name: "analytics".into(),
                    color: "1d76db".into(),
                }],
                checks: Some(CheckTally {
                    passed: 27,
                    failed: 0,
                    pending: 2,
                }),
            },
            answered: crate::pull_request::Answered {
                conflicts: Some(false),
                checks: Some(Checks::Pending),
            },
            ..pr(number, title, false)
        }
    }

    /// The list gathers into sections — yours, the ones waiting on your
    /// review, everyone else's — each under a rule with its count, a blank
    /// line between them, and each row is one line: the dot, the number in
    /// its column, the title, the checks' and the review's marks, the age.
    /// ↓ walks the rows in that order and never lands on a header.
    #[test]
    fn rows_gather_into_sections_one_line_each() {
        let mut asked = rich(41, "Speed up the grid", "jacobrvl", false);
        asked.meta.review_requested = true;
        let (mut app, _) = app_with(
            vec![
                rich(43, "Others' work", "sam", false),
                asked,
                rich(42, "My fix", "me", true),
            ],
            true,
        );
        open(&mut app);
        let shot = screen(&mut app, 240, 40);
        let at = |needle: &str| {
            shot.find(needle)
                .unwrap_or_else(|| panic!("{needle} missing from\n{shot}"))
        };
        assert!(at(" ── Yours ─") < at("↗ #42  My fix"), "{shot}");
        assert!(at("↗ #42  My fix") < at(" ── Review requested ─"), "{shot}");
        assert!(
            at(" ── Review requested ─") < at("↗ #41  Speed up the grid"),
            "{shot}"
        );
        assert!(
            at("↗ #41  Speed up the grid") < at(" ── Others ─"),
            "{shot}"
        );
        assert!(at(" ── Others ─") < at("↗ #43  Others' work"), "{shot}");
        let row = shot
            .lines()
            .find(|l| l.contains("#42  My fix"))
            .unwrap_or_default();
        assert!(
            row.contains("◐ ✓ ready       1h │"),
            "checks, review, word, age: {row:?}"
        );
        let lines: Vec<&str> = shot.lines().collect();
        let others = lines
            .iter()
            .position(|l| l.contains("── Others ─"))
            .unwrap();
        assert!(
            lines[others - 1].contains("│                "),
            "a blank line over a section: {shot}"
        );

        // The cursor opens on the first row shown, and ↓ walks the
        // sections' order.
        assert_eq!(selected_pr(&app).unwrap().number, 42);
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(selected_pr(&app).unwrap().number, 41);
        handle_key(&mut app, key(KeyCode::Down), &mut Vec::new());
        assert_eq!(selected_pr(&app).unwrap().number, 43);
    }

    /// A list that is all one section still sits under its rule, as a
    /// checkout's sessions sit under their band however many there are.
    #[test]
    fn one_section_alone_keeps_its_rule() {
        let (mut app, _) = app_with(
            vec![rich(43, "A", "sam", false), rich(44, "B", "kim", false)],
            true,
        );
        open(&mut app);
        let shot = screen(&mut app, 160, 40);
        assert!(shot.contains(" ── Others ─"), "{shot}");
        assert!(shot.contains("#43  A"), "{shot}");
    }

    /// `key:value` tokens in the filter narrow by a facet — label, author,
    /// checks, review, state — beside the fuzzy words, and the count says
    /// `matches/all`.
    #[test]
    fn tokens_in_the_filter_narrow_by_facet() {
        let mut failing = rich(40, "Bump deps", "dependabot", false);
        failing.answered.checks = Some(Checks::Failing);
        failing.meta.checks = Some(crate::pull_request::CheckTally {
            passed: 27,
            failed: 2,
            pending: 0,
        });
        failing.meta.labels.clear();
        let mut draft = rich(39, "Spike", "sam", false);
        draft.answered_draft = true;
        let list = vec![rich(42, "Fix login", "sam", false), failing, draft];
        let prs = observed(&list);
        let shown = |query: &str| -> Vec<u64> {
            visible_rows(query, &list, &prs)
                .iter()
                .map(|(i, _)| list[*i].number)
                .collect()
        };
        assert_eq!(shown("checks:failing"), vec![40]);
        assert_eq!(shown("label:analytics"), vec![42, 39]);
        assert_eq!(shown("-label:analytics"), vec![40]);
        assert_eq!(shown("author:sam is:draft"), vec![39]);
        assert_eq!(shown("author:sam fix"), vec![42]);
        assert_eq!(shown("review:approved"), vec![42, 40, 39]);
        assert_eq!(
            shown("label:"),
            vec![42, 40, 39],
            "a key mid-typing narrows nothing"
        );

        let (mut app, _) = app_with(list, true);
        open(&mut app);
        type_str(&mut app, "checks:failing");
        assert_eq!(selected_pr(&app).unwrap().number, 40);
        let shot = screen(&mut app, 160, 40);
        assert!(shot.contains("Pull requests — demo (1/3)"), "{shot}");
    }

    /// `⌘F` puts the FILTER PICK in the page's place: ←/→ walk the facets,
    /// ↑/↓ their values, `space` writes the value's token into the filter
    /// line (the rows narrowing behind it) and takes it out again, and
    /// Enter puts the picker away with the filter kept.
    #[test]
    fn the_filter_pick_writes_tokens_into_the_filter() {
        let mut failing = rich(40, "Bump deps", "dependabot", false);
        failing.answered.checks = Some(Checks::Failing);
        failing.meta.checks = Some(crate::pull_request::CheckTally {
            passed: 27,
            failed: 2,
            pending: 0,
        });
        let (mut app, _) = app_with(vec![rich(42, "Fix login", "sam", false), failing], true);
        open(&mut app);
        let mut out = Vec::new();
        handle_key(&mut app, cmd('f'), &mut out);
        assert!(view(&app).filter_pick.is_some());
        let shot = screen(&mut app, 160, 40);
        assert!(shot.contains("Filter"), "{shot}");
        assert!(shot.contains("dependabot"), "{shot}");
        crate::hints::assert_hints_from(&hints(view(&app)), crate::list_filter::keys::ALL);

        // Author → Label → Review → Checks; `failing` is its second value.
        for _ in 0..3 {
            handle_key(&mut app, key(KeyCode::Right), &mut out);
        }
        handle_key(&mut app, key(KeyCode::Down), &mut out);
        handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
        assert_eq!(view(&app).query.as_str(), "checks:\"failing\"");
        assert_eq!(selected_pr(&app).unwrap().number, 40);
        let shot = screen(&mut app, 160, 40);
        assert!(shot.contains("✓ failing"), "the value is ticked\n{shot}");

        handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
        assert_eq!(view(&app).query.as_str(), "", "space again takes it out");
        handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
        handle_key(&mut app, key(KeyCode::Enter), &mut out);
        assert!(view(&app).filter_pick.is_none());
        assert_eq!(
            view(&app).query.as_str(),
            "checks:\"failing\"",
            "the filter stays"
        );
        assert!(
            matches!(app.overlay, Some(Overlay::PullRequests(_))),
            "Enter launched nothing"
        );
    }

    /// A click on a row selects it; a click on a section's rule, or the
    /// blank line over it, selects nothing.
    #[test]
    fn a_click_selects_a_row_and_never_a_rule() {
        let (mut app, _) = app_with(
            vec![
                rich(43, "Others' work", "sam", false),
                rich(42, "My fix", "me", true),
            ],
            true,
        );
        open(&mut app);
        screen(&mut app, 160, 40);
        let list = view(&app).list_area;
        let click = |app: &mut App, y: u16| {
            let at = Position::new(list.x + 2, y);
            let ev = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: at.x,
                row: at.y,
                modifiers: KeyModifiers::NONE,
            };
            handle_mouse(app, ev, at, &mut Vec::new());
        };
        // Yours, #42, a blank, Others, #43.
        assert_eq!(selected_pr(&app).unwrap().number, 42);
        click(&mut app, list.y + 4);
        assert_eq!(selected_pr(&app).unwrap().number, 43);
        click(&mut app, list.y);
        assert_eq!(selected_pr(&app).unwrap().number, 43, "a rule is no row");
        click(&mut app, list.y + 2);
        assert_eq!(
            selected_pr(&app).unwrap().number,
            43,
            "nor the blank over it"
        );
        click(&mut app, list.y + 1);
        assert_eq!(selected_pr(&app).unwrap().number, 42);
    }

    /// However narrow the list, no row's line runs past it: the title gives
    /// way first, then the status column — the cursor's row as any other.
    #[test]
    fn no_row_runs_past_a_narrow_list() {
        let (mut app, _) = app_with(
            vec![
                rich(
                    42,
                    "A rather long title for a pull request",
                    "someone-long",
                    false,
                ),
                rich(41, "Mine", "me", true),
            ],
            true,
        );
        open(&mut app);
        let now = orion_core::clock::now_secs() as i64;
        let rows = rows(&app, &view(&app).project).to_vec();
        let cols = RowCols::of(&rows, now);
        for width in (MIN_LIST_W as usize - 2)..=120 {
            for pr in &rows {
                for cursor in [None, Some(true), Some(false)] {
                    let line = row_line(
                        pr,
                        &app.prs.status_or_open(&pr.url),
                        &[],
                        cursor,
                        &cols,
                        width,
                        now,
                        app.theme,
                    );
                    let w: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                    assert!(w <= width, "row {w} > {width}: {line:?}");
                }
            }
        }
        for w in [60u16, 80, 100, 160] {
            screen(&mut app, w, 30);
        }
    }

    fn url(number: u64) -> String {
        format!("https://github.com/o/r/pull/{number}")
    }

    fn mine(number: u64, title: &str) -> OpenPr {
        OpenPr {
            mine: true,
            ..pr(number, title, false)
        }
    }

    /// An instant `PR_DETAIL_FRESH` and a bit ago: a page read then has
    /// aged out.
    fn aged() -> std::time::Instant {
        std::time::Instant::now()
            .checked_sub(crate::event_loop::PR_DETAIL_FRESH + std::time::Duration::from_secs(1))
            .expect("machine up for a minute")
    }

    fn read_at(app: &mut App, number: u64, at: std::time::Instant) {
        app.pr_detail.insert(url(number), detail(number, "read"));
        app.pr_detail_at.insert(url(number), at);
    }

    fn urls<'a>(due: impl IntoIterator<Item = &'a PendingPrDetail>) -> Vec<String> {
        due.into_iter().map(|p| p.url.clone()).collect()
    }

    /// A page read moments ago shows as it is; one read longer ago than
    /// `PR_DETAIL_FRESH` is read again when the cursor rests on it — the
    /// Checks tab must not say `pending` until `⌘R` forces it.
    #[test]
    fn a_page_older_than_the_window_is_read_again_on_the_next_rest() {
        let (mut app, _) = app_with(vec![pr(42, "Fix login", false)], true);
        read_at(&mut app, 42, std::time::Instant::now());
        open(&mut app);
        assert_eq!(pending_url(&app), None, "fresh: shown as it is");

        app.overlay = None;
        read_at(&mut app, 42, aged());
        open(&mut app);
        assert_eq!(
            pending_url(&app),
            Some(url(42).as_str()),
            "aged: asked again"
        );
    }

    /// The list's beat, landing while the cursor stays on a pull request,
    /// reads its page again once that page has aged — checks running under
    /// the reader catch up without a key.
    #[test]
    fn the_lists_beat_reads_the_cursors_aged_page_again() {
        let (mut app, _) = app_with(vec![pr(42, "Fix login", false)], true);
        read_at(&mut app, 42, std::time::Instant::now());
        open(&mut app);
        list_changed(&mut app);
        assert_eq!(pending_url(&app), None, "still fresh");

        app.pr_detail_at.insert(url(42), aged());
        list_changed(&mut app);
        assert_eq!(pending_url(&app), Some(url(42).as_str()));
    }

    /// A refusal is kept for the window — no hammering a `gh` that just
    /// failed — and asked again after it, rather than for the session.
    #[test]
    fn a_refusal_is_asked_again_once_it_ages() {
        let (mut app, _) = app_with(vec![pr(42, "Fix login", false)], true);
        app.pr_detail_failed.insert(url(42));
        app.pr_detail_at.insert(url(42), std::time::Instant::now());
        assert!(!app.pr_detail_owed(&url(42)));
        app.pr_detail_at.insert(url(42), aged());
        assert!(app.pr_detail_owed(&url(42)));
    }

    /// Opening the modal reads every one of your pull requests that isn't
    /// fresh at once, up to `PREFETCH_PARALLEL` in flight, and the others
    /// this session hasn't read one per `PREFETCH_GAP` behind them. Pages
    /// already read — fresh, or someone else's however old — wait for the
    /// cursor.
    #[test]
    fn opening_reads_yours_at_once_and_the_rest_one_at_a_time() {
        let (mut app, _) = app_with(
            vec![
                mine(50, "Mine, unread"),
                mine(49, "Mine, aged"),
                mine(48, "Mine, fresh"),
                pr(47, "Theirs, unread", false),
                pr(46, "Theirs, aged", false),
                pr(45, "Theirs, unread too", false),
            ],
            true,
        );
        read_at(&mut app, 49, aged());
        read_at(&mut app, 48, std::time::Instant::now());
        read_at(&mut app, 46, aged());
        open(&mut app);
        let queue = &view(&app).prefetch;
        assert_eq!(urls(&queue.soon), [url(50), url(49)]);
        assert_eq!(urls(&queue.later), [url(47), url(45)]);
        assert_eq!(prefetch_delay(&app), Some(std::time::Duration::ZERO));

        let due = take_prefetch(&mut app);
        assert_eq!(
            urls(&due),
            [url(50), url(49), url(47)],
            "yours, then one of theirs"
        );
        for p in &due {
            app.pr_detail_inflight
                .begin(p.url.clone(), crate::fetch::now());
        }
        assert_eq!(
            prefetch_delay(&app),
            None,
            "three in flight: wait for a landing"
        );

        app.pr_detail_inflight.clear();
        let gap = prefetch_delay(&app).expect("one still queued");
        assert!(gap > std::time::Duration::ZERO, "theirs wait out the gap");
        assert!(take_prefetch(&mut app).is_empty(), "not before the gap");
    }

    /// A page the cursor read while it sat in the queue is dropped when
    /// its turn comes, and closing the modal drops the queue with it.
    #[test]
    fn the_prefetch_skips_what_was_read_meanwhile_and_dies_with_the_modal() {
        let (mut app, _) = app_with(vec![mine(50, "Mine"), mine(49, "Also mine")], true);
        open(&mut app);
        read_at(&mut app, 50, std::time::Instant::now());
        assert_eq!(urls(&take_prefetch(&mut app)), [url(49)]);

        open(&mut app);
        close(&mut app);
        assert_eq!(prefetch_delay(&app), None);
        assert!(take_prefetch(&mut app).is_empty());
    }
}
