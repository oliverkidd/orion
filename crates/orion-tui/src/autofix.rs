//! AUTOFIX: an agent sent to mend one of your pull requests — its merge
//! conflicts, its failing unit and e2e tests, its other failing checks —
//! that runs them locally until they pass and pushes the fix to the PR.
//!
//! * **Seeing it break.** Every project's open-PR list (`event_loop::
//!   note_open_prs_answer`) passes through [`note_list`]: a pull request
//!   the `gh` user opened (`OpenPr::mine`) that is in trouble is *watched*,
//!   and its body (`PrDetail`, the one place check names live) is asked
//!   for. [`land_detail`] reads each body that lands. Checks still running
//!   beside a failed one are waited out, up to [`SETTLE_LIMIT`], so one
//!   agent sees every failure at once; conflicts alone are acted on
//!   straight away. The open-PR list is the clock: the body is asked for
//!   again the moment the PR's row moves (a check finishes, a push), with
//!   [`RECHECK`] only as the fallback for a row that sits still.
//! * **Once per breakage.** What broke is a [`Fingerprint`] — the head
//!   commit, the conflicts and the failing checks' names — and a per-PR
//!   ledger (`Record`, kept in the PR cache) remembers the last one sent or
//!   dismissed: nothing asks again until a push or a different failure. A
//!   session at work in a checkout of the PR's branch — any, not only an
//!   `autofix-<n>` — holds the ask back until it is done, and a PR already
//!   being asked about is not asked about twice.
//! * **Asking.** **When a PR breaks** (`Config::pr_autofix`): `ask` queues
//!   the AUTOFIX MODAL — the issues as a checklist, the detected ones
//!   ticked, and a note — which opens only on a free screen ([`tick`]: no
//!   modal up, no terminal pane taking keys), with a desktop notification
//!   while the window is away; `auto` sends at once, until
//!   [`AUTO_ATTEMPTS`] tries in a row have not turned the PR green, when it
//!   asks instead. `⌘G` in the PULL REQUESTS MODAL ([`open_for`]) opens the
//!   same form for any PR, whatever the setting.
//! * **Sending.** [`dispatch`] launches a PR SESSION (`CreatePrAgent`, so
//!   the DAEMON finds or cuts the PR's worktree) named `autofix-<n>` on the
//!   default agent at the **Autofix model** / **Autofix effort**
//!   (`Config::autofix_launch`), its first prompt [`prompt`]: the pull
//!   request and what failed, under orion's own instructions or an AGENT
//!   PRESET's text in their place (**Autofix instructions**).
//!
//! Unit vs e2e is read off each check's name ([`classify`]) — GitHub has
//! no such distinction.

use crate::agent_presets::AgentPreset;
use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::hints::Hint;
use crate::pull_request::{CheckState, Checks, OpenPr, PrCheck, PrDetail, PrLaunch};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::{AgentStatus, ClientRequest, ProjectId};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often a watched pull request's checks are read again while some
/// are still running and its row has not moved — the fallback; a row that
/// moves has them read at once ([`note_list`]).
pub const RECHECK: Duration = Duration::from_secs(60);
/// How long running checks are waited out before acting on the failures
/// already in.
pub const SETTLE_LIMIT: Duration = Duration::from_secs(20 * 60);
/// `auto` sends this many times in a row without the PR going green, then
/// asks.
pub const AUTO_ATTEMPTS: u8 = 3;

/// **When a PR breaks** (Settings → Review).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Off,
    Ask,
    Auto,
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Ask => "ask",
            Mode::Auto => "auto",
        }
    }

    /// The stored word; anything else is off.
    pub fn parse(word: &str) -> Self {
        match word.trim().to_ascii_lowercase().as_str() {
            "ask" => Mode::Ask,
            "auto" => Mode::Auto,
            _ => Mode::Off,
        }
    }
}

/// One kind of thing the agent can be sent to fix — a row of the form, in
/// [`Issue::ALL`] order (which is also its index in [`Picks`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Issue {
    Conflicts,
    Unit,
    E2e,
    Other,
}

/// What is ticked, by [`Issue`] index.
pub type Picks = [bool; Issue::ALL.len()];

impl Issue {
    pub const ALL: [Issue; 4] = [Issue::Conflicts, Issue::Unit, Issue::E2e, Issue::Other];

    /// Its row on the form.
    pub fn label(self) -> &'static str {
        match self {
            Issue::Conflicts => "Merge conflicts",
            Issue::Unit => "Unit tests",
            Issue::E2e => "E2E tests",
            Issue::Other => "Other checks",
        }
    }

    /// What the prompt calls it, in its list and over its instructions.
    fn heading(self) -> &'static str {
        match self {
            Issue::Conflicts => "Merge conflicts",
            Issue::Unit => "Failing unit tests",
            Issue::E2e => "Failing e2e tests",
            Issue::Other => "Other failing checks",
        }
    }

    /// One word for its failed checks in a summary (`2 unit failing`).
    fn noun(self) -> &'static str {
        match self {
            Issue::Conflicts => "conflicts",
            Issue::Unit => "unit",
            Issue::E2e => "e2e",
            Issue::Other => "other",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Words in a check's workflow or name that make it an e2e suite — read
/// first, since "e2e tests" says "test" too.
const E2E_WORDS: &[&str] = &[
    "e2e",
    "end-to-end",
    "end to end",
    "playwright",
    "cypress",
    "integration",
    "smoke",
    "acceptance",
];
/// And a unit suite.
const UNIT_WORDS: &[&str] = &[
    "test", "unit", "jest", "vitest", "spec", "pytest", "nextest",
];

/// Which kind of failure a check is, by its workflow and name: e2e, unit,
/// or anything else (lint, typecheck, build). Never [`Issue::Conflicts`].
pub fn classify(check: &PrCheck) -> Issue {
    let words = format!("{} {}", check.workflow, check.name).to_ascii_lowercase();
    if E2E_WORDS.iter().any(|w| words.contains(w)) {
        Issue::E2e
    } else if UNIT_WORDS.iter().any(|w| words.contains(w)) {
        Issue::Unit
    } else {
        Issue::Other
    }
}

/// What is wrong with a pull request, as its body says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnosis {
    pub conflicts: bool,
    /// The failed checks of each kind.
    pub unit: Vec<PrCheck>,
    pub e2e: Vec<PrCheck>,
    pub other: Vec<PrCheck>,
    /// Some check has not finished.
    pub running: bool,
}

/// Read a pull request's body for what is wrong with it.
pub fn diagnose(detail: &PrDetail) -> Diagnosis {
    let mut out = Diagnosis {
        conflicts: detail.health.conflicts,
        ..Diagnosis::default()
    };
    for check in &detail.checks {
        match check.state {
            CheckState::Failed => match classify(check) {
                Issue::E2e => out.e2e.push(check.clone()),
                Issue::Unit => out.unit.push(check.clone()),
                _ => out.other.push(check.clone()),
            },
            CheckState::Running => out.running = true,
            CheckState::Passed | CheckState::Skipped => {}
        }
    }
    out
}

impl Diagnosis {
    fn has_failed_checks(&self) -> bool {
        !(self.unit.is_empty() && self.e2e.is_empty() && self.other.is_empty())
    }

    /// Nothing to fix.
    pub fn is_empty(&self) -> bool {
        !self.conflicts && !self.has_failed_checks()
    }

    /// Only conflicts — nothing failed that running checks could add to.
    fn conflicts_only(&self) -> bool {
        self.conflicts && !self.has_failed_checks()
    }

    /// Whether `issue` was seen.
    pub fn has(&self, issue: Issue) -> bool {
        match issue {
            Issue::Conflicts => self.conflicts,
            _ => !self.failing(issue).is_empty(),
        }
    }

    /// The failed checks behind `issue` (none for conflicts).
    pub fn failing(&self, issue: Issue) -> &[PrCheck] {
        match issue {
            Issue::Conflicts => &[],
            Issue::Unit => &self.unit,
            Issue::E2e => &self.e2e,
            Issue::Other => &self.other,
        }
    }

    /// This breakage, at `head_sha`.
    pub fn fingerprint(&self, head_sha: &str) -> Fingerprint {
        let mut checks: Vec<String> = [&self.unit, &self.e2e, &self.other]
            .into_iter()
            .flatten()
            .map(|c| c.name.clone())
            .collect();
        checks.sort_unstable();
        Fingerprint {
            sha: head_sha.to_string(),
            conflicts: self.conflicts,
            checks,
        }
    }

    /// `merge conflicts · 2 unit failing · 1 other failing`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.conflicts {
            parts.push("merge conflicts".to_string());
        }
        for issue in [Issue::Unit, Issue::E2e, Issue::Other] {
            let n = self.failing(issue).len();
            if n > 0 {
                parts.push(format!("{n} {} failing", issue.noun()));
            }
        }
        if parts.is_empty() {
            "nothing failing".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// One breakage, said so that the same one says the same thing whatever
/// order the checks came back in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// The head commit.
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub conflicts: bool,
    /// Every failed check's name, sorted.
    #[serde(default)]
    pub checks: Vec<String>,
}

impl Fingerprint {
    /// Whether this is the breakage `pr`'s row shows, as far as a row can
    /// tell: the same head commit, the same conflicts, and — while its
    /// checks fail — failed checks named. A row names no checks, so a
    /// different set failing on the same commit is not told apart; a push,
    /// conflicts coming or going, or checks failing after a conflicts-only
    /// send are.
    fn covers(&self, pr: &OpenPr) -> bool {
        !pr.head_sha.is_empty()
            && self.sha == pr.head_sha
            && self.conflicts == pr.health.conflicts
            && (pr.health.checks != Checks::Failing || !self.checks.is_empty())
    }
}

/// What the ledger remembers of one pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The last breakage sent or dismissed.
    #[serde(default)]
    pub handled: Option<Fingerprint>,
    /// Agents sent since the PR was last green.
    #[serde(default)]
    pub attempts: u8,
}

/// A watched pull request: in trouble, its body asked for.
#[derive(Debug, Clone)]
pub struct Watch {
    pub project: ProjectId,
    pub number: u64,
    pub dir: PathBuf,
    pub first_seen: Instant,
    /// When its body is next asked for; None while an ask is out.
    pub next_fetch: Option<Instant>,
    /// Its row as the open list last showed it ([`RowState`]).
    pub row: RowState,
    /// The row moved while an ask was out: the answer may predate it, so
    /// the next one goes at once.
    pub moved: bool,
}

/// What of a pull request's row says its checks or commit changed: the
/// head commit, its health, and GitHub's tally of passed, failed and
/// running checks.
pub type RowState = (
    String,
    crate::pull_request::Health,
    Option<crate::pull_request::CheckTally>,
);

fn row_state(pr: &OpenPr) -> RowState {
    (pr.head_sha.clone(), pr.health, pr.meta.checks)
}

/// The app's AUTOFIX state (`App::autofix`).
#[derive(Debug, Clone, Default)]
pub struct State {
    /// By PR URL; persisted in the PR cache.
    pub ledger: HashMap<String, Record>,
    /// By PR URL.
    pub watching: HashMap<String, Watch>,
    /// Asks waiting for a free screen, oldest first.
    pub queue: VecDeque<AutofixForm>,
    /// The ledger changed since the PR cache was last written.
    pub dirty: bool,
    /// `⌘G` asked for this pull request's form, waiting on a fresh body.
    pub pending_open: Option<String>,
}

impl State {
    fn record(&mut self, url: &str) -> &mut Record {
        self.dirty = true;
        self.ledger.entry(url.to_string()).or_default()
    }

    /// Re-read `url`'s body after [`RECHECK`] — or at once when its row
    /// moved while this answer was out.
    fn recheck_later(&mut self, url: &str) {
        if let Some(w) = self.watching.get_mut(url) {
            let wait = if w.moved { Duration::ZERO } else { RECHECK };
            w.moved = false;
            w.next_fetch = Some(Instant::now() + wait);
        }
    }

    /// Stop watching `url` and drop any ask about it still waiting.
    fn forget(&mut self, url: &str) {
        self.watching.remove(url);
        self.queue.retain(|form| form.pr.url != url);
    }
}

/// Whether `url` is being asked about: queued, or the form on screen.
fn asking(app: &App, url: &str) -> bool {
    app.autofix.queue.iter().any(|form| form.pr.url == url)
        || matches!(&app.overlay, Some(Overlay::Autofix(form)) if form.pr.url == url)
}

// ---- detection ----

/// A project's open-PR list landed: watch each of the user's PRs in
/// trouble that this breakage has not been handled for, read a watched
/// one's body again now that its row moved, and forget the attempts — and
/// any waiting ask — of each one that has gone green.
pub fn note_list(app: &mut App, project: &ProjectId, list: &[OpenPr]) {
    for pr in list.iter().filter(|pr| pr.mine) {
        if pr.trouble().is_none() {
            if pr.health.checks != Checks::Pending
                && app
                    .autofix
                    .ledger
                    .get(&pr.url)
                    .is_some_and(|r| r.attempts > 0)
            {
                app.autofix.record(&pr.url).attempts = 0;
            }
            app.autofix.forget(&pr.url);
            continue;
        }
        if let Some(watch) = app.autofix.watching.get_mut(&pr.url) {
            let row = row_state(pr);
            if watch.row != row {
                watch.row = row;
                match &mut watch.next_fetch {
                    Some(at) => *at = Instant::now(),
                    None => watch.moved = true,
                }
            }
            continue;
        }
        if app.autofix_mode == Mode::Off || asking(app, &pr.url) {
            continue;
        }
        let handled = app
            .autofix
            .ledger
            .get(&pr.url)
            .and_then(|r| r.handled.as_ref())
            .is_some_and(|f| f.covers(pr));
        if handled {
            continue;
        }
        watch(app, project, pr, Instant::now());
    }
    // PRs that left the list were merged or closed.
    let open: HashSet<&str> = list.iter().map(|pr| pr.url.as_str()).collect();
    let gone: Vec<String> = app
        .autofix
        .watching
        .iter()
        .filter(|(url, w)| &w.project == project && !open.contains(url.as_str()))
        .map(|(url, _)| url.clone())
        .chain(
            app.autofix
                .queue
                .iter()
                .filter(|f| &f.project == project && !open.contains(f.pr.url.as_str()))
                .map(|f| f.pr.url.clone()),
        )
        .collect();
    for url in gone {
        app.autofix.forget(&url);
    }
}

/// Watch `pr`, its body asked for at `next_fetch`.
fn watch(app: &mut App, project: &ProjectId, pr: &OpenPr, next_fetch: Instant) {
    let Some(dir) = repo_dir(app, project) else {
        return;
    };
    app.autofix.watching.insert(
        pr.url.clone(),
        Watch {
            project: project.clone(),
            number: pr.number,
            dir,
            first_seen: Instant::now(),
            next_fetch: Some(next_fetch),
            row: row_state(pr),
            moved: false,
        },
    );
}

/// The project's repo path — where `gh` reads it from, as the open-PR list
/// that flagged the PR did.
fn repo_dir(app: &App, project: &ProjectId) -> Option<PathBuf> {
    app.tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map(|p| p.repo_path.clone())
}

/// The bodies whose turn has come: each watched PR's next fetch, made due
/// now and marked out until it lands. `(url, number, dir)`. One already on
/// its way (the pane's own read) is waited for instead — [`land_detail`]
/// reads every body that lands.
pub fn due_fetches(app: &mut App) -> Vec<(String, u64, PathBuf)> {
    let now = Instant::now();
    let mut due = Vec::new();
    for (url, watch) in app.autofix.watching.iter_mut() {
        if watch.next_fetch.is_some_and(|at| at <= now) {
            watch.next_fetch = None;
            if !app.pr_detail_inflight.contains(url) {
                due.push((url.clone(), watch.number, watch.dir.clone()));
            }
        }
    }
    due
}

/// A body landed (any body: a watched PR's, or the pane's): act on a
/// watched one's breakage once its checks have settled.
pub fn land_detail(
    app: &mut App,
    url: &str,
    detail: Option<&PrDetail>,
    out: &mut Vec<ClientRequest>,
) {
    if app.autofix.pending_open.as_deref() == Some(url) {
        app.autofix.pending_open = None;
        open_pending(app, url, detail);
    }
    let Some(watch) = app.autofix.watching.get(url).cloned() else {
        return;
    };
    let Some(detail) = detail else {
        // `gh` could not answer: try again on the next beat.
        app.autofix.recheck_later(url);
        return;
    };
    let diagnosis = diagnose(detail);
    let pr = open_pr(app, &watch.project, url);
    let (Some(pr), true, false) = (pr, detail.is_open(), diagnosis.is_empty()) else {
        app.autofix.forget(url);
        return;
    };
    let settling = diagnosis.running
        && !diagnosis.conflicts_only()
        && watch.first_seen.elapsed() < SETTLE_LIMIT;
    if settling || branch_busy(app, &watch.project, &pr) {
        app.autofix.recheck_later(url);
        return;
    }
    app.autofix.watching.remove(url);
    let record = app.autofix.ledger.get(url).cloned().unwrap_or_default();
    let form = AutofixForm::new(watch.project, pr, detail);
    if record.handled.as_ref() == Some(&form.fingerprint()) || asking(app, url) {
        return;
    }
    match app.autofix_mode {
        Mode::Off => {}
        Mode::Auto if record.attempts < AUTO_ATTEMPTS => dispatch(app, form, out),
        Mode::Ask | Mode::Auto => ask(app, form, record.attempts),
    }
    app.dirty = true;
}

/// Queue `form` for the next free screen — saying why when `auto` has run
/// out of tries — and tell the desktop while the window is away.
fn ask(app: &mut App, mut form: AutofixForm, attempts: u8) {
    if attempts >= AUTO_ATTEMPTS {
        form.notice = Some(format!(
            "autofix has tried {attempts} times without the checks going green"
        ));
    }
    if app.may_notify_desktop() {
        crate::event_loop::alerts::notify_text(
            "Autofix",
            &format!("{}: {}", form.pr.label(), form.diagnosis.summary()),
        );
    }
    app.autofix.queue.push_back(form);
}

/// `⌘G`'s body landed: the form, over the PULL REQUESTS MODAL if it is
/// still up on that pull request — from the cached body when `gh` could
/// not answer. Not for one merged or closed since.
fn open_pending(app: &mut App, url: &str, detail: Option<&PrDetail>) {
    let Some(pr) = crate::pr_modal::selected_pr(app).filter(|pr| pr.url == url) else {
        return;
    };
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let project = view.project.clone();
    match detail.or_else(|| app.pr_detail.get(url)).cloned() {
        Some(detail) if !detail.is_open() => {
            app.flash = Some(Flash::note(format!("#{} is no longer open", pr.number)));
        }
        Some(detail) => {
            app.flash = None;
            open_for(app, project, pr, &detail);
        }
        None => {
            app.flash = Some(Flash::failed(format!(
                "Couldn't read #{}'s checks",
                pr.number
            )));
        }
    }
}

/// The PR's row in its project's open list.
fn open_pr(app: &App, project: &ProjectId, url: &str) -> Option<OpenPr> {
    app.open_prs
        .get(project)?
        .list
        .iter()
        .find(|pr| pr.url == url)
        .cloned()
}

/// The session name an autofix agent on PR `number` goes by.
pub fn agent_name(number: u64) -> String {
    format!("autofix-{number}")
}

/// A session is at work (or waiting on the user) in one of `project`'s
/// checkouts of `pr`'s branch — someone is already on it, so an autofix
/// agent would only be a second pair of hands on the same branch. An
/// autofix agent counts from the moment it is sent, before its first turn.
fn branch_busy(app: &App, project: &ProjectId, pr: &OpenPr) -> bool {
    let fixer = agent_name(pr.number);
    app.tree.agents.iter().any(|agent| {
        !agent.archived
            && match agent.status {
                AgentStatus::Running | AgentStatus::NeedsFeedback => true,
                AgentStatus::Fresh => agent.name == fixer,
                _ => false,
            }
            && app.tree.worktrees.iter().any(|w| {
                w.id == agent.worktree_id && &w.project_id == project && w.branch == pr.head
            })
    })
}

/// The git beat: raise the oldest queued ask that still stands once the
/// screen is free — no modal up, and no terminal pane taking keys, where an
/// Enter meant for the agent would send this instead. Asks left over from
/// before autofix was switched off are dropped, and one whose branch a
/// session has started work on since goes back to being watched.
pub fn tick(app: &mut App) {
    if app.autofix_mode == Mode::Off {
        app.autofix.queue.clear();
        return;
    }
    if app.overlay.is_some() || app.term_locked {
        return;
    }
    while let Some(form) = app.autofix.queue.pop_front() {
        let handled = app
            .autofix
            .ledger
            .get(&form.pr.url)
            .and_then(|r| r.handled.as_ref());
        if handled == Some(&form.fingerprint()) {
            continue;
        }
        if branch_busy(app, &form.project, &form.pr) {
            watch(app, &form.project, &form.pr, Instant::now() + RECHECK);
            continue;
        }
        app.overlay = Some(Overlay::Autofix(Box::new(form)));
        app.dirty = true;
        return;
    }
}

// ---- the prompt ----

/// The branch names of a pull request's body that the form and the prompt
/// read — kept instead of the whole body, which every queued form and each
/// frame's clone of the overlay would otherwise carry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrRefs {
    /// The branch it merges into, and the one it comes from (GitHub's
    /// names, not the checkout's).
    pub base: String,
    pub head: String,
    pub head_sha: String,
}

impl PrRefs {
    pub fn of(detail: &PrDetail) -> Self {
        Self {
            base: detail.base.clone(),
            head: detail.head.clone(),
            head_sha: detail.head_sha.clone(),
        }
    }

    /// The base branch, or `fallback` when GitHub did not say.
    fn base_or<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.base.is_empty() {
            fallback
        } else {
            &self.base
        }
    }
}

/// The agent's first prompt: the pull request and the ticked issues —
/// with the failed checks behind each — and the user's note, under
/// orion's own instructions, or between `preset`'s prefix and postfix in
/// their place.
pub fn prompt(
    pr: &OpenPr,
    refs: &PrRefs,
    diagnosis: &Diagnosis,
    picks: &Picks,
    note: &str,
    preset: Option<&AgentPreset>,
) -> String {
    let base = match refs.base.as_str() {
        "" => "the base branch".to_string(),
        name => format!("`{name}`"),
    };
    let ticked: Vec<Issue> = Issue::ALL
        .into_iter()
        .filter(|i| picks[i.index()])
        .collect();
    let context = context(pr, refs, diagnosis, &ticked, &base, note);
    let parts: Vec<String> = match preset {
        Some(preset) => vec![preset.prefix.clone(), context, preset.postfix.clone()],
        None => {
            let mut parts = vec![format!(
                "You are orion's PR autofixer. Fix the issues below on this pull request, then commit and push to its branch. Do not open a new pull request, force-push, merge the PR, or change unrelated code.\n\n{context}"
            )];
            parts.extend(ticked.iter().map(|issue| {
                format!(
                    "## {}\n{}",
                    issue.heading(),
                    instructions(*issue, &base, pr.number)
                )
            }));
            parts.push(format!(
                "When everything above passes locally, run the relevant suites once more, commit with a message that says what you fixed, and `git push`. Then `gh pr checks {n} --watch`; if a check you were asked to fix fails again, go round once more (at most twice). Finish with a short summary: what you changed, and anything you could not fix and why.",
                n = pr.number
            ));
            parts
        }
    };
    parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What the agent is pointed at: the pull request, its branches, each
/// ticked issue with the checks behind it, and the user's note.
fn context(
    pr: &OpenPr,
    refs: &PrRefs,
    diagnosis: &Diagnosis,
    ticked: &[Issue],
    base: &str,
    note: &str,
) -> String {
    let head = if refs.head.is_empty() {
        &pr.head
    } else {
        &refs.head
    };
    let mut lines = vec![
        format!("Pull request: {} (#{} \"{}\")", pr.url, pr.number, pr.title),
        format!(
            "Branch: `{head}` into {base}. This worktree is checked out on `{}`.",
            pr.head
        ),
        String::new(),
        "Fix:".to_string(),
    ];
    for issue in ticked {
        let heading = issue.heading();
        let checks = diagnosis.failing(*issue);
        match issue {
            Issue::Conflicts => lines.push(format!("- {heading} with {base}")),
            _ if checks.is_empty() => lines.push(format!(
                "- {heading} (none named by GitHub — check `gh pr checks {}`)",
                pr.number
            )),
            _ => {
                lines.push(format!("- {heading}:"));
                lines.extend(checks.iter().map(|c| format!("  - {}", check_line(c))));
            }
        }
    }
    let note = note.trim();
    if !note.is_empty() {
        lines.push(String::new());
        lines.push(format!("Note from the user: {note}"));
    }
    lines.join("\n")
}

/// `name (workflow) url`, leaving out what GitHub did not say.
fn check_line(check: &PrCheck) -> String {
    let mut line = check.name.clone();
    if !check.workflow.is_empty() && check.workflow != check.name {
        line.push_str(&format!(" ({})", check.workflow));
    }
    if !check.url.is_empty() {
        line.push_str(&format!(" {}", check.url));
    }
    line
}

/// orion's own instructions for one ticked issue, under its heading.
fn instructions(issue: Issue, base: &str, number: u64) -> String {
    match issue {
        Issue::Conflicts => format!(
            "Fetch and merge {base} from origin. Resolve each conflict by understanding both sides' intent; never drop one side's work silently. Regenerate lockfiles and generated files rather than hand-merging them. Build, run the tests that cover the conflicted files, and commit the merge."
        ),
        Issue::Unit => format!(
            "Read the failure logs (`gh pr checks {number}`, then `gh run view <run-id> --log-failed`). Reproduce each failure locally with this repo's test command, fix the code (change a test only when this PR intentionally changed the behaviour it checks), and re-run until they pass."
        ),
        Issue::E2e => "Pull the failing test names out of the CI logs, run exactly those tests locally with this repo's e2e command, fix, and re-run until they pass. Run a test that now passes 3 times to rule out flakiness. If a test fails only in CI and passes reliably locally, say so rather than guessing at a fix.".to_string(),
        Issue::Other => "Reproduce each locally (lint, typecheck, build — whatever the check runs), fix, and re-run until it passes.".to_string(),
    }
}

// ---- the form ----

/// A row of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Issue(Issue),
    Note,
}

impl Row {
    const ORDER: [Row; 5] = [
        Row::Issue(Issue::Conflicts),
        Row::Issue(Issue::Unit),
        Row::Issue(Issue::E2e),
        Row::Issue(Issue::Other),
        Row::Note,
    ];

    fn step(self, down: bool) -> Self {
        crate::pr_actions::step_clamped(&Self::ORDER, self, down)
    }
}

/// The AUTOFIX MODAL: which of a pull request's issues to send the agent
/// at, and a note for it.
#[derive(Debug, Clone)]
pub struct AutofixForm {
    pub project: ProjectId,
    pub pr: OpenPr,
    pub refs: PrRefs,
    pub diagnosis: Diagnosis,
    pub picks: Picks,
    pub row: Row,
    pub note: TextInput,
    /// Why the last Enter went nowhere, or why `auto` asked instead.
    pub notice: Option<String>,
    /// The PULL REQUESTS MODAL this was opened over (`⌘G`), put back when
    /// it closes.
    pub under: Option<Box<crate::pr_modal::PullRequestsView>>,
    /// As of the last draw: the modal, and each row's rect, for the mouse.
    pub area: Rect,
    pub rows: Vec<(Rect, Row)>,
}

impl AutofixForm {
    /// The form for `pr` as `detail` diagnoses it: what was detected
    /// ticked, the caret on the first of them.
    pub fn new(project: ProjectId, pr: OpenPr, detail: &PrDetail) -> Self {
        let diagnosis = diagnose(detail);
        let picks = Issue::ALL.map(|issue| diagnosis.has(issue));
        let row = Row::ORDER
            .into_iter()
            .find(|row| matches!(row, Row::Issue(i) if diagnosis.has(*i)))
            .unwrap_or(Row::Issue(Issue::Conflicts));
        Self {
            project,
            pr,
            refs: PrRefs::of(detail),
            diagnosis,
            picks,
            row,
            note: TextInput::new(),
            notice: None,
            under: None,
            area: Rect::default(),
            rows: Vec::new(),
        }
    }

    fn toggle(&mut self, issue: Issue) {
        let i = issue.index();
        self.picks[i] = !self.picks[i];
        self.notice = None;
    }

    fn fingerprint(&self) -> Fingerprint {
        self.diagnosis.fingerprint(&self.refs.head_sha)
    }
}

/// `⌘G` in the PULL REQUESTS MODAL: the form for `pr`, over the modal,
/// from the body already in hand.
pub fn open_for(app: &mut App, project: ProjectId, pr: OpenPr, detail: &PrDetail) {
    let mut form = AutofixForm::new(project, pr, detail);
    if let Some(Overlay::PullRequests(view)) = app.overlay.take() {
        form.under = Some(Box::new(view));
    }
    app.overlay = Some(Overlay::Autofix(Box::new(form)));
    app.dirty = true;
}

/// Close the form: the PULL REQUESTS MODAL it stood on comes back.
fn close(app: &mut App) -> Option<AutofixForm> {
    let Some(Overlay::Autofix(form)) = app.overlay.take() else {
        return None;
    };
    let mut form = *form;
    app.overlay = form.under.take().map(|view| Overlay::PullRequests(*view));
    app.dirty = true;
    Some(form)
}

/// Send the agent: a PR SESSION named `autofix-<n>` on the default agent,
/// told [`prompt`]. The ledger counts it and the PR stops being watched.
pub fn dispatch(app: &mut App, form: AutofixForm, out: &mut Vec<ClientRequest>) {
    let Some(worktree) = app.root_worktree(&form.project) else {
        app.flash = Some(Flash::failed(
            "Autofix: the project has no checkout to start from",
        ));
        return;
    };
    let cfg = crate::config::Config::load();
    let (kind, custom, model, effort) = cfg.autofix_launch();
    let named = cfg.autofix_preset.trim();
    let preset = (!named.is_empty())
        .then(|| {
            crate::agent_presets::load()
                .into_iter()
                .find(|p| p.name.eq_ignore_ascii_case(named))
        })
        .flatten();
    let text = prompt(
        &form.pr,
        &form.refs,
        &form.diagnosis,
        &form.picks,
        form.note.as_str(),
        preset.as_ref(),
    );
    let draft = crate::app::AgentLaunchDraft {
        custom,
        name: agent_name(form.pr.number),
        starting_prompt: Some(text),
        pr: Some(PrLaunch::of(&form.pr)),
        focus_pane: false,
        follow: false,
        ..crate::app::AgentLaunchDraft::new(worktree, kind, model, effort)
    };
    crate::event_loop::create_agent(app, draft, out);
    let url = form.pr.url.clone();
    let record = app.autofix.record(&url);
    record.handled = Some(form.fingerprint());
    record.attempts = record.attempts.saturating_add(1);
    app.autofix.forget(&url);
    let sent: Vec<&str> = Issue::ALL
        .into_iter()
        .filter(|i| form.picks[i.index()])
        .map(|i| i.label())
        .collect();
    let missing = if !named.is_empty() && preset.is_none() {
        format!(" (preset \"{named}\" is gone — built-in instructions)")
    } else {
        String::new()
    };
    app.flash = Some(Flash::working(format!(
        "Autofix sent to #{}: {}{missing}",
        form.pr.number,
        sent.join(", ").to_lowercase()
    )));
    app.dirty = true;
}

/// Esc: not now — this breakage is not asked about again.
fn dismiss(app: &mut App) {
    let Some(form) = close(app) else {
        return;
    };
    // `⌘G` was asked for; nothing to remember.
    if form.under.is_none() {
        app.autofix.record(&form.pr.url).handled = Some(form.fingerprint());
    }
}

pub mod keys {
    use crate::hints::Key;

    pub const OPTION: Key = Key::new(&["up", "down"], "option").show(2);
    pub const TICK: Key = Key::new(&["space"], "tick");
    pub const SEND: Key = Key::new(&["enter"], "send autofix");
    #[cfg(test)]
    pub const ALL: &[Key] = &[OPTION, TICK, SEND];
}

/// The keys along the modal's bottom edge.
pub fn hints(form: &AutofixForm) -> Vec<Hint> {
    let mut hints = vec![keys::SEND.hint().kept(), keys::OPTION.hint()];
    if form.row != Row::Note {
        hints.push(keys::TICK.hint());
    }
    hints.push(Hint::new("Esc", "not now").kept());
    hints
}

pub fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    if key.code == KeyCode::Esc {
        dismiss(app);
        return;
    }
    let Some(Overlay::Autofix(form)) = &mut app.overlay else {
        return;
    };
    let on_note = form.row == Row::Note;
    match key.code {
        KeyCode::Down | KeyCode::Tab => form.row = form.row.step(true),
        KeyCode::Up | KeyCode::BackTab => form.row = form.row.step(false),
        KeyCode::Char(' ') if !on_note => {
            if let Row::Issue(issue) = form.row {
                form.toggle(issue);
            }
        }
        KeyCode::Enter => submit(app, out),
        _ if on_note && form.note.handle_key(&key).changed() => form.notice = None,
        _ => {}
    }
    app.dirty = true;
}

/// Enter: send the agent at what is ticked — refused with nothing ticked.
fn submit(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Autofix(form)) = &mut app.overlay else {
        return;
    };
    if !form.picks.iter().any(|p| *p) {
        form.notice = Some("tick something to fix".into());
        return;
    }
    if let Some(form) = close(app) {
        dispatch(app, form, out);
    }
}

/// A paste lands in the note.
pub fn paste(app: &mut App, text: &str) {
    if let Some(Overlay::Autofix(form)) = &mut app.overlay {
        form.row = Row::Note;
        form.note.insert_str(&text.replace('\n', " "));
        app.dirty = true;
    }
}

/// A click on an issue ticks it; on the note, puts the caret there. (A
/// click outside is Esc, as on every modal: `overlay_close`.)
pub fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return;
    }
    let Some(Overlay::Autofix(form)) = &mut app.overlay else {
        return;
    };
    let at = Position::new(mouse.column, mouse.row);
    if let Some(&(_, row)) = form.rows.iter().find(|(r, _)| r.contains(at)) {
        form.row = row;
        if let Row::Issue(issue) = row {
            form.toggle(issue);
        }
        app.dirty = true;
    }
}

// ---- drawing ----

const MODAL_W: u16 = 84;
/// The narrowest and the shortest the modal shrinks to on a small screen.
const MODAL_MIN_W: u16 = 24;
const MODAL_MIN_H: u16 = 8;
/// How wide an issue's name is drawn, so the notes after them line up.
const LABEL_W: usize = 16;
/// The base branch's name when GitHub did not say.
const SOME_BASE: &str = "its base";
const EXPLAIN: &str = "The agent runs in the PR's worktree on your default agent: it fixes what is ticked, runs the tests locally until they pass, then commits and pushes to the PR.";
const EXPLAIN_ROWS: u16 = 3;

/// What each issue row says after its box.
fn row_note(form: &AutofixForm, issue: Issue) -> String {
    let failing = form.diagnosis.failing(issue);
    match issue {
        Issue::Conflicts if form.diagnosis.conflicts => format!(
            "{} has moved on — merge it in and resolve",
            form.refs.base_or(SOME_BASE)
        ),
        _ if !form.diagnosis.has(issue) => "none detected".into(),
        _ => {
            let names: Vec<&str> = failing.iter().map(|c| c.name.as_str()).collect();
            format!("{} failing: {}", failing.len(), names.join(", "))
        }
    }
}

/// One issue's row: the caret, its box, its name and what was found.
fn issue_row(form: &AutofixForm, issue: Issue, th: Theme) -> Vec<Span<'static>> {
    let on = form.row == Row::Issue(issue);
    let detected = form.diagnosis.has(issue);
    let text = Style::default().fg(th.text);
    let dim = Style::default().fg(th.dim);
    let name = if on {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else if detected {
        text
    } else {
        dim
    };
    vec![
        Span::styled(
            if on { " › " } else { "   " },
            Style::default().fg(th.accent),
        ),
        Span::styled(
            format!("{} ", crate::pr_actions::check(form.picks[issue.index()])),
            text,
        ),
        Span::styled(format!("{:<LABEL_W$}", issue.label()), name),
        Span::styled(
            row_note(form, issue),
            if detected {
                Style::default().fg(th.warn)
            } else {
                dim
            },
        ),
    ]
}

/// The form's rows top to bottom, each with the row it is for the mouse.
fn lines(form: &AutofixForm, inner_w: usize, th: Theme) -> Vec<(Option<Row>, Vec<Span<'static>>)> {
    let indent = crate::pr_preview::INDENT;
    let mut lines = vec![
        (
            None,
            vec![Span::styled(
                format!("{indent}{}", form.pr.label()),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            )],
        ),
        (
            None,
            vec![Span::styled(
                format!(
                    "{indent}{} → {} · {}",
                    form.pr.head,
                    form.refs.base_or(SOME_BASE),
                    form.diagnosis.summary()
                ),
                Style::default().fg(th.dim),
            )],
        ),
        (None, Vec::new()),
    ];
    lines.extend(
        Issue::ALL
            .into_iter()
            .map(|issue| (Some(Row::Issue(issue)), issue_row(form, issue, th))),
    );
    lines.push((None, Vec::new()));
    lines.push((
        Some(Row::Note),
        crate::ui::form_field(
            "Note",
            &form.note,
            "anything the agent should know (optional)",
            form.row == Row::Note,
            inner_w,
            th,
        ),
    ));
    if let Some(notice) = &form.notice {
        lines.push((None, Vec::new()));
        lines.push((
            None,
            vec![Span::styled(
                format!("{indent}⚠ {notice}"),
                Style::default().fg(th.warn),
            )],
        ));
    }
    lines
}

pub fn draw(f: &mut Frame, app: &mut App, form: &AutofixForm, th: Theme) {
    let frame = f.area();
    let width = MODAL_W
        .min(frame.width.saturating_sub(2))
        .max(frame.width.min(MODAL_MIN_W));
    let inner_w = usize::from(width.saturating_sub(2));
    let lines = lines(form, inner_w, th);
    let explain_rows = crate::hints::explain_lines(EXPLAIN, width.saturating_sub(2), EXPLAIN_ROWS)
        .len() as u16
        + 1;
    let want = lines.len() as u16 + explain_rows + 2;
    let height = want
        .min(frame.height.saturating_sub(2))
        .max(frame.height.min(MODAL_MIN_H));
    let area = crate::ui::centered_rect(frame, width, height);
    f.render_widget(Clear, area);
    let block = crate::hints::modal_block(
        crate::ui::modal_block(format!(" Autofix #{} ", form.pr.number), th),
        &hints(form),
        area.width,
        th,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let (body, explain_row) = crate::hints::explain_area(EXPLAIN, inner, EXPLAIN_ROWS);
    crate::hints::draw_explain(f, explain_row, EXPLAIN, th);
    let mut rows = Vec::new();
    for (i, (row, spans)) in lines.into_iter().enumerate() {
        let Some(rect) = crate::ui::row_rect(body, i) else {
            break;
        };
        f.render_widget(Paragraph::new(crate::pr_preview::fit(spans, inner_w)), rect);
        if let Some(row) = row {
            rows.push((rect, row));
        }
    }
    if let Some(Overlay::Autofix(live)) = &mut app.overlay {
        live.area = area;
        live.rows = rows;
    }
}

/// A form for a pull request with conflicts — what the overlay tests open.
#[cfg(test)]
pub(crate) fn sample_form() -> AutofixForm {
    let detail = PrDetail {
        number: 7,
        url: "https://github.com/o/r/pull/7".into(),
        state: "OPEN".into(),
        health: crate::pull_request::Health {
            conflicts: true,
            ..Default::default()
        },
        base: "main".into(),
        head: "feat".into(),
        ..PrDetail::default()
    };
    let pr = OpenPr {
        number: 7,
        title: "Add things".into(),
        url: detail.url.clone(),
        is_draft: false,
        health: detail.health,
        head: "feat".into(),
        mine: true,
        head_sha: String::new(),
        meta: Default::default(),
    };
    AutofixForm::new(ProjectId("p".into()), pr, &detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::Health;

    fn check(name: &str, workflow: &str, state: CheckState) -> PrCheck {
        PrCheck {
            name: name.into(),
            workflow: workflow.into(),
            state,
            word: String::new(),
            started: String::new(),
            completed: String::new(),
            url: format!("https://ci/{name}"),
        }
    }

    fn detail(conflicts: bool, checks: Vec<PrCheck>) -> PrDetail {
        PrDetail {
            number: 7,
            url: "https://github.com/o/r/pull/7".into(),
            title: "Add things".into(),
            state: "OPEN".into(),
            health: Health {
                conflicts,
                ..Health::default()
            },
            base: "main".into(),
            head: "feat".into(),
            head_sha: "abc123".into(),
            checks,
            ..PrDetail::default()
        }
    }

    fn pr() -> OpenPr {
        OpenPr {
            number: 7,
            title: "Add things".into(),
            url: "https://github.com/o/r/pull/7".into(),
            is_draft: false,
            health: Health::default(),
            head: "feat".into(),
            mine: true,
            head_sha: "abc123".into(),
            meta: Default::default(),
        }
    }

    #[test]
    fn checks_are_classified_by_name() {
        let kind = |name: &str, wf: &str| classify(&check(name, wf, CheckState::Failed));
        assert_eq!(kind("e2e tests (shard 2)", "CI"), Issue::E2e);
        assert_eq!(kind("chromium", "Playwright"), Issue::E2e);
        assert_eq!(kind("unit tests", "CI"), Issue::Unit);
        assert_eq!(kind("vitest", ""), Issue::Unit);
        assert_eq!(kind("lint", "CI"), Issue::Other);
        assert_eq!(kind("typecheck", ""), Issue::Other);
    }

    #[test]
    fn diagnosis_lists_failures_and_notices_running_checks() {
        let d = diagnose(&detail(
            true,
            vec![
                check("lint", "", CheckState::Failed),
                check("unit", "", CheckState::Passed),
                check("e2e", "", CheckState::Running),
            ],
        ));
        assert!(d.conflicts);
        assert_eq!(d.other.len(), 1);
        assert!(d.unit.is_empty() && d.e2e.is_empty());
        assert!(d.running);
        assert_eq!(d.summary(), "merge conflicts · 1 other failing");
    }

    #[test]
    fn the_fingerprint_ignores_check_order_but_not_the_head() {
        let a = diagnose(&detail(
            false,
            vec![
                check("a test", "", CheckState::Failed),
                check("b test", "", CheckState::Failed),
            ],
        ));
        let b = diagnose(&detail(
            false,
            vec![
                check("b test", "", CheckState::Failed),
                check("a test", "", CheckState::Failed),
            ],
        ));
        assert_eq!(a.fingerprint("x"), b.fingerprint("x"));
        assert_ne!(a.fingerprint("x"), a.fingerprint("y"));
    }

    #[test]
    fn the_prompt_says_only_what_is_ticked() {
        let detail = detail(true, vec![check("e2e", "", CheckState::Failed)]);
        let text = prompt(
            &pr(),
            &PrRefs::of(&detail),
            &diagnose(&detail),
            &[false, false, true, false],
            "flaky on CI",
            None,
        );
        assert!(text.contains("https://github.com/o/r/pull/7"));
        assert!(text.contains("`feat` into `main`"));
        assert!(text.contains("## Failing e2e tests"));
        assert!(text.contains("https://ci/e2e"));
        assert!(text.contains("Note from the user: flaky on CI"));
        assert!(!text.contains("## Merge conflicts"));
        assert!(!text.contains("## Failing unit tests"));
    }

    #[test]
    fn a_preset_replaces_the_instructions_but_keeps_the_context() {
        let detail = detail(true, Vec::new());
        let preset = AgentPreset {
            name: "mine".into(),
            kind: orion_core::AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            prefix: "Do it my way.".into(),
            postfix: "Then push.".into(),
            skip_task: true,
        };
        let text = prompt(
            &pr(),
            &PrRefs::of(&detail),
            &diagnose(&detail),
            &[true, false, false, false],
            "",
            Some(&preset),
        );
        assert!(text.starts_with("Do it my way."));
        assert!(text.ends_with("Then push."));
        assert!(text.contains("- Merge conflicts with `main`"));
        assert!(!text.contains("orion's PR autofixer"));
    }

    #[test]
    fn the_form_ticks_what_was_detected() {
        let detail = detail(false, vec![check("unit", "", CheckState::Failed)]);
        let form = AutofixForm::new(ProjectId("p".into()), pr(), &detail);
        assert_eq!(form.picks, [false, true, false, false]);
        assert_eq!(form.row, Row::Issue(Issue::Unit));
    }

    #[test]
    fn modes_read_leniently() {
        assert_eq!(Mode::parse("ASK"), Mode::Ask);
        assert_eq!(Mode::parse("auto"), Mode::Auto);
        assert_eq!(Mode::parse("sometimes"), Mode::Off);
    }

    // ---- detection, on an App ----

    fn app_with(list: Vec<OpenPr>, mode: Mode) -> App {
        let mut app = App::new();
        let project = ProjectId("p".into());
        app.tree.projects.push(orion_core::Project {
            id: project.clone(),
            name: "repo".into(),
            repo_path: "/tmp/repo".into(),
            sort_order: 0,
        });
        app.tree.worktrees.push(orion_core::Worktree {
            id: orion_core::WorktreeId("root".into()),
            project_id: project.clone(),
            path: "/tmp/repo".into(),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        });
        let now = Instant::now();
        app.open_prs.insert(
            project,
            crate::app::OpenPrs {
                list,
                at: now,
                due: now,
                step: Duration::from_secs(15),
            },
        );
        app.autofix_mode = mode;
        app
    }

    fn broken(conflicts: bool, failing: bool) -> OpenPr {
        OpenPr {
            health: Health {
                conflicts,
                checks: if failing {
                    Checks::Failing
                } else {
                    Checks::Passing
                },
            },
            ..pr()
        }
    }

    fn listed(app: &mut App) {
        let project = ProjectId("p".into());
        let list = app.open_prs[&project].list.clone();
        note_list(app, &project, &list);
    }

    fn landed(app: &mut App, detail: &PrDetail) -> Vec<ClientRequest> {
        let mut out = Vec::new();
        crate::config::with_config_path(
            tempfile::tempdir().unwrap().path().join("config.json"),
            || land_detail(app, &detail.url.clone(), Some(detail), &mut out),
        );
        out
    }

    fn handled(sha: &str, conflicts: bool) -> Option<Fingerprint> {
        Some(Fingerprint {
            sha: sha.into(),
            conflicts,
            checks: Vec::new(),
        })
    }

    #[test]
    fn only_my_broken_prs_are_watched_and_only_when_on() {
        let theirs = OpenPr {
            mine: false,
            ..broken(true, false)
        };
        let mut app = app_with(vec![theirs], Mode::Ask);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "not mine");

        let mut app = app_with(vec![broken(true, false)], Mode::Off);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "off");

        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));
        assert_eq!(
            due_fetches(&mut app).len(),
            1,
            "its body is asked for at once"
        );
        assert!(due_fetches(&mut app).is_empty(), "and only once");
    }

    #[test]
    fn running_checks_are_waited_out_but_conflicts_alone_are_not() {
        let mut app = app_with(vec![broken(false, true)], Mode::Ask);
        listed(&mut app);
        let running = detail(
            false,
            vec![
                check("unit", "", CheckState::Failed),
                check("e2e", "", CheckState::Running),
            ],
        );
        landed(&mut app, &running);
        assert!(app.autofix.queue.is_empty(), "still settling");
        assert!(
            app.autofix.watching[&pr().url].next_fetch.is_some(),
            "asked again later"
        );

        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        listed(&mut app);
        landed(
            &mut app,
            &detail(true, vec![check("e2e", "", CheckState::Running)]),
        );
        assert_eq!(app.autofix.queue.len(), 1, "conflicts go at once");
        assert!(app.autofix.watching.is_empty());
    }

    #[test]
    fn a_dismissed_breakage_is_not_asked_about_again_until_it_changes() {
        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        listed(&mut app);
        landed(&mut app, &detail(true, Vec::new()));
        tick(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::Autofix(_))));
        handle_key(&mut app, KeyEvent::from(KeyCode::Esc), &mut Vec::new());
        assert!(app.overlay.is_none());
        assert_eq!(
            app.autofix.ledger[&pr().url].attempts,
            0,
            "a dismissal is no attempt"
        );

        listed(&mut app);
        assert!(
            app.autofix.watching.is_empty(),
            "same commit, same conflicts"
        );

        // Checks failing on top are news.
        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = vec![broken(true, true)];
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));

        // So is a push.
        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                handled: handled("abc123", true),
                attempts: 0,
            },
        );
        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = vec![OpenPr {
            head_sha: "def456".into(),
            ..broken(true, false)
        }];
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));
    }

    /// The form on screen is not asked about again while it waits for an
    /// answer — the next list would otherwise queue a second one, and a
    /// second Enter send a second agent.
    #[test]
    fn a_pr_on_screen_is_not_asked_about_twice() {
        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        listed(&mut app);
        landed(&mut app, &detail(true, Vec::new()));
        tick(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::Autofix(_))));
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "it is up already");
        assert!(app.autofix.queue.is_empty());
    }

    #[test]
    fn a_waiting_ask_goes_when_its_pr_is_fixed_merged_or_autofix_is_off() {
        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        listed(&mut app);
        landed(&mut app, &detail(true, Vec::new()));
        assert_eq!(app.autofix.queue.len(), 1);
        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = vec![broken(false, false)];
        listed(&mut app);
        assert!(app.autofix.queue.is_empty(), "green");

        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        listed(&mut app);
        landed(&mut app, &detail(true, Vec::new()));
        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = Vec::new();
        listed(&mut app);
        assert!(app.autofix.queue.is_empty(), "merged");

        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        listed(&mut app);
        landed(&mut app, &detail(true, Vec::new()));
        app.autofix_mode = Mode::Off;
        tick(&mut app);
        assert!(app.overlay.is_none() && app.autofix.queue.is_empty(), "off");
    }

    #[test]
    fn auto_sends_until_its_attempts_run_out_then_asks() {
        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(app.autofix.queue.is_empty(), "sent, not asked");
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 1);
        assert!(
            out.iter().any(|req| matches!(
                req,
                ClientRequest::CreatePrAgent { name, starting_prompt: Some(p), .. }
                    if name == "autofix-7" && p.contains("Merge conflicts")
            )),
            "{out:?}"
        );

        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                handled: None,
                attempts: AUTO_ATTEMPTS,
            },
        );
        listed(&mut app);
        landed(&mut app, &detail(true, Vec::new()));
        assert_eq!(app.autofix.queue.len(), 1, "asks instead");
        assert!(app.autofix.queue[0].notice.is_some());
    }

    #[test]
    fn going_green_forgets_the_attempts() {
        let mut app = app_with(vec![broken(false, false)], Mode::Auto);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                handled: handled("x", false),
                attempts: 2,
            },
        );
        listed(&mut app);
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 0);
    }

    #[test]
    fn a_queued_ask_waits_for_a_free_screen() {
        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        app.autofix.queue.push_back(sample_form());
        app.overlay = Some(Overlay::Help(crate::app::HelpView::default()));
        tick(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::Help(_))));
        app.overlay = None;
        app.term_locked = true;
        tick(&mut app);
        assert!(app.overlay.is_none(), "not over a pane taking keys");
        app.term_locked = false;
        tick(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::Autofix(_))));
    }

    #[test]
    fn enter_with_nothing_ticked_is_refused() {
        let mut app = App::new();
        let mut form = sample_form();
        form.picks = [false; Issue::ALL.len()];
        app.overlay = Some(Overlay::Autofix(Box::new(form)));
        handle_key(&mut app, KeyEvent::from(KeyCode::Enter), &mut Vec::new());
        match &app.overlay {
            Some(Overlay::Autofix(form)) => assert!(form.notice.is_some()),
            other => panic!("the form closed: {other:?}"),
        }
    }

    #[test]
    fn the_form_hints_come_from_its_keys() {
        crate::hints::assert_hints_from(&hints(&sample_form()), keys::ALL);
    }

    /// The modal names the PR, what broke and each row's checks, ticks
    /// what was detected, and keeps its keys on the border.
    #[test]
    fn the_modal_draws_the_pr_its_issues_and_its_keys() {
        let mut app = App::new();
        let detail = detail(
            true,
            vec![
                check("unit tests", "CI", CheckState::Failed),
                check("lint", "CI", CheckState::Failed),
            ],
        );
        app.overlay = Some(Overlay::Autofix(Box::new(AutofixForm::new(
            ProjectId("p".into()),
            pr(),
            &detail,
        ))));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let shot: String = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        for text in [
            "Autofix #7",
            "#7 Add things",
            "merge conflicts · 1 unit failing · 1 other failing",
            "[x] Merge conflicts",
            "[x] Unit tests",
            "1 failing: unit tests",
            "[ ] E2E tests",
            "none detected",
            "[x] Other checks",
            "Note",
            "send autofix",
            "not now",
        ] {
            assert!(shot.contains(text), "{text} missing:\n{shot}");
        }
    }

    /// A session on `feat` (the PR's branch) in a checkout of project `p`.
    fn session_on_branch(app: &mut App, name: &str, status: AgentStatus) {
        app.tree.worktrees.push(orion_core::Worktree {
            id: orion_core::WorktreeId("feat".into()),
            project_id: ProjectId("p".into()),
            path: "/tmp/repo-feat".into(),
            branch: "feat".into(),
            is_main: false,
            sort_order: 1,
        });
        app.tree.agents.push(orion_core::Agent {
            id: orion_core::AgentId(name.into()),
            worktree_id: orion_core::WorktreeId("feat".into()),
            name: name.into(),
            status,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: orion_core::AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            cloud_session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: true,
            issue_url: None,
            recent_prompts: Vec::new(),
            usage_limit: None,
        });
    }

    /// The open list is the clock: a check finishing moves the row, and the
    /// body is read again on that beat rather than a minute later.
    #[test]
    fn a_settling_pr_is_read_again_as_soon_as_its_row_moves() {
        let tally = |failed, pending| {
            Some(crate::pull_request::CheckTally {
                passed: 0,
                failed,
                pending,
            })
        };
        let row = |pending| OpenPr {
            meta: crate::pull_request::PrMeta {
                checks: tally(1, pending),
                ..Default::default()
            },
            ..broken(false, true)
        };
        let mut app = app_with(vec![row(1)], Mode::Ask);
        listed(&mut app);
        assert_eq!(due_fetches(&mut app).len(), 1);
        let running = detail(
            false,
            vec![
                check("unit", "", CheckState::Failed),
                check("e2e", "", CheckState::Running),
            ],
        );
        landed(&mut app, &running);
        assert!(due_fetches(&mut app).is_empty(), "settling");

        listed(&mut app);
        assert!(due_fetches(&mut app).is_empty(), "the row sat still");

        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = vec![row(0)];
        listed(&mut app);
        assert_eq!(due_fetches(&mut app).len(), 1, "the e2e run finished");

        // Moving while that read is out: the next goes at once, whatever
        // the answer.
        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = vec![row(1)];
        listed(&mut app);
        landed(&mut app, &running);
        assert_eq!(due_fetches(&mut app).len(), 1);
    }

    /// Anyone at work on the PR's branch holds the ask back, not only an
    /// autofix agent — and an idle session does not.
    #[test]
    fn a_session_at_work_on_the_branch_holds_the_ask_back() {
        for (status, held) in [
            (AgentStatus::Running, true),
            (AgentStatus::NeedsFeedback, true),
            (AgentStatus::Finished, false),
            (AgentStatus::Fresh, false),
        ] {
            let mut app = app_with(vec![broken(true, false)], Mode::Ask);
            session_on_branch(&mut app, "mine", status);
            listed(&mut app);
            landed(&mut app, &detail(true, Vec::new()));
            assert_eq!(app.autofix.queue.is_empty(), held, "{status:?}");
            assert_eq!(app.autofix.watching.contains_key(&pr().url), held);
        }
        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        session_on_branch(&mut app, &agent_name(7), AgentStatus::Fresh);
        listed(&mut app);
        landed(&mut app, &detail(true, Vec::new()));
        assert!(app.autofix.queue.is_empty(), "an autofix agent just sent");
    }

    /// A session that starts on the branch while the ask waits for a free
    /// screen takes it off the queue, back to being watched.
    #[test]
    fn a_queued_ask_steps_aside_for_a_session_that_started_on_the_branch() {
        let mut app = app_with(vec![broken(true, false)], Mode::Ask);
        listed(&mut app);
        landed(&mut app, &detail(true, Vec::new()));
        assert_eq!(app.autofix.queue.len(), 1);
        session_on_branch(&mut app, "mine", AgentStatus::Running);
        tick(&mut app);
        assert!(app.overlay.is_none());
        assert!(app.autofix.queue.is_empty());
        assert!(app.autofix.watching.contains_key(&pr().url));
    }
}
