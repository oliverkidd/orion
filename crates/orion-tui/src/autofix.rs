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
//! * **Sending, unasked.** **Auto-fix my PRs** (`Config::pr_autofix`, on by
//!   default) sends the agent the moment a breakage has settled — no modal,
//!   nothing to confirm — until [`AUTO_ATTEMPTS`] tries in a row have not
//!   turned the PR green, when it stops and says so (a flash, and a desktop
//!   notification while the window is away). `⌘G` in the PULL REQUESTS
//!   MODAL ([`open_for`]) still opens the AUTOFIX MODAL for any PR — the
//!   issues as a checklist and a note — whatever the setting.
//! * **As the worktree's agent.** [`dispatch`] launches a PR SESSION
//!   (`CreatePrAgent`, so the DAEMON finds or cuts the PR's worktree) named
//!   `autofix-<n>` on the harness, account, model and effort of the session
//!   that did the work on the branch ([`source_agent`]) — with none, the
//!   default agent at the **PR autofix** SYSTEM PRESET's model and effort
//!   — its first prompt [`prompt`]: the pull request and what failed,
//!   wrapped in that preset's text, which is orion's own rules
//!   ([`PRESET_TEXT`]) while it holds none of the user's.
//! * **Standing down.** Another session live on the branch — mid-turn, or
//!   waiting on the user — does not hold the agent back. It is told where
//!   that session's conversation is ([`stand_down_preamble`]) and reads it
//!   first: when that session is already on this fix, the agent runs
//!   `orion autofix stand-down "<why>"`, which archives it, and the flash
//!   says why ([`note_stand_downs`]). A live session whose conversation
//!   can't be read holds it back instead, as every busy one used to.
//!
//! Unit vs e2e is read off each check's name ([`classify`]) — GitHub has
//! no such distinction.

use crate::agent_presets::AgentPreset;
use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::hints::Hint;
use crate::pr_store::PrStatus;
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
use std::collections::{HashMap, HashSet};
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

/// **Auto-fix my PRs** (Settings → General): on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Off,
    Auto,
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Auto => "auto",
        }
    }

    /// The stored word: `off` is off, and anything else on — the default.
    pub fn parse(word: &str) -> Self {
        match word.trim().to_ascii_lowercase().as_str() {
            "off" => Mode::Off,
            _ => Mode::Auto,
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

/// Read a pull request's body for what is wrong with it — its failed and
/// running checks — with `conflicts` as `App::prs` says (the newest answer
/// that knew; an unknown one reads as none).
pub fn diagnose(detail: &PrDetail, conflicts: bool) -> Diagnosis {
    let mut out = Diagnosis {
        conflicts,
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
    /// Whether this is the breakage a pull request standing at `status`
    /// (`App::prs`) shows, as far as a row can tell: the same head commit,
    /// the same conflicts, and — while its checks fail — failed checks
    /// named. A row names no checks, so a different set failing on the
    /// same commit is not told apart; a push, conflicts coming or going, or
    /// checks failing after a conflicts-only send are.
    fn covers(&self, status: &PrStatus) -> bool {
        !status.head_sha.is_empty()
            && self.sha == status.head_sha
            && self.conflicts == status.health.conflicts
            && (status.health.checks != Checks::Failing || !self.checks.is_empty())
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

/// `pr`'s [`RowState`]: the head commit and health as `App::prs` has
/// them, the tally off the row's meta line.
fn row_state(app: &App, pr: &OpenPr) -> RowState {
    let status = app.prs.status_or_open(&pr.url);
    (status.head_sha, status.health, pr.meta.checks)
}

/// The app's AUTOFIX state (`App::autofix`).
#[derive(Debug, Clone, Default)]
pub struct State {
    /// By PR URL; persisted in the PR cache.
    pub ledger: HashMap<String, Record>,
    /// By PR URL.
    pub watching: HashMap<String, Watch>,
    /// PRs whose send a live session nobody can read is holding back,
    /// already said once ([`land_detail`]).
    pub held: HashSet<String>,
    /// Every stood-down `autofix-<n>` already flashed — and, before the
    /// first look, every one archived by an earlier run (`None`).
    pub stood_down: Option<HashSet<orion_core::AgentId>>,
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

    /// Stop watching `url`.
    fn forget(&mut self, url: &str) {
        self.watching.remove(url);
        self.held.remove(url);
    }
}

// ---- detection ----

/// A project's open-PR list landed: watch each of the user's PRs in
/// trouble that this breakage has not been handled for, read a watched
/// one's body again now that its row moved, and forget the attempts of
/// each one that has gone green. Where each stands is
/// `App::prs`'s word, the list already observed into it. Green is known,
/// never guessed ([`PrStatus::is_green`]): while GitHub has not said
/// whether the branch merges, or checks are still running, a watch and its
/// attempts are left as they are.
pub fn note_list(app: &mut App, project: &ProjectId, list: &[OpenPr]) {
    for pr in list.iter().filter(|pr| pr.mine) {
        let status = app.prs.status_or_open(&pr.url);
        if status.is_green() {
            if app
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
        let row = row_state(app, pr);
        if let Some(watch) = app.autofix.watching.get_mut(&pr.url) {
            if watch.row != row {
                watch.row = row;
                match &mut watch.next_fetch {
                    Some(at) => *at = Instant::now(),
                    None => watch.moved = true,
                }
            }
            continue;
        }
        // Not in trouble, but not known green either: nothing to watch.
        if status.trouble().is_none() {
            continue;
        }
        if app.autofix_mode == Mode::Off {
            continue;
        }
        let handled = app
            .autofix
            .ledger
            .get(&pr.url)
            .and_then(|r| r.handled.as_ref())
            .is_some_and(|f| f.covers(&status));
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
    let row = row_state(app, pr);
    app.autofix.watching.insert(
        pr.url.clone(),
        Watch {
            project: project.clone(),
            number: pr.number,
            dir,
            first_seen: Instant::now(),
            next_fetch: Some(next_fetch),
            row,
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
            if !app.pr_detail_inflight.in_flight(url) {
                due.push((url.clone(), watch.number, watch.dir.clone()));
            }
        }
    }
    due
}

/// A body landed (any body: a watched PR's, or the pane's), already
/// observed into `App::prs`: act on a watched one's breakage once its
/// checks have settled. A body that shows nothing wrong forgets the watch
/// only once GitHub has said the branch merges; with conflicts unknown it
/// is read again on the next beat.
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
    let status = app.prs.status_or_open(url);
    let diagnosis = diagnose(detail, status.health.conflicts);
    let pr = open_pr(app, &watch.project, url);
    if diagnosis.is_empty() && status.is_open() && pr.is_some() && !status.conflicts_known {
        app.autofix.recheck_later(url);
        return;
    }
    let (Some(pr), true, false) = (pr, status.is_open(), diagnosis.is_empty()) else {
        app.autofix.forget(url);
        return;
    };
    let settling = diagnosis.running
        && !diagnosis.conflicts_only()
        && watch.first_seen.elapsed() < SETTLE_LIMIT;
    if settling || fixer_on_it(app, &watch.project, &pr) {
        app.autofix.recheck_later(url);
        return;
    }
    // The branch's other live sessions, each with the conversation the
    // agent reads before it touches anything — or, for one whose
    // conversation can't be found, nothing sent yet.
    let mut peers = Vec::new();
    let mut unreadable = None;
    for agent in live_on_branch(app, &watch.project, &pr) {
        match transcript_of(agent) {
            Some(transcript) => peers.push(Peer {
                name: agent.name.clone(),
                waiting: agent.status == AgentStatus::NeedsFeedback,
                transcript,
            }),
            None => {
                unreadable = Some(agent.name.clone());
                break;
            }
        }
    }
    if let Some(name) = unreadable {
        if app.autofix.held.insert(url.to_string()) {
            app.flash = Some(Flash::note(format!(
                "Autofix held for #{}: {name} is busy on its branch",
                pr.number
            )));
            app.dirty = true;
        }
        app.autofix.recheck_later(url);
        return;
    }
    app.autofix.watching.remove(url);
    app.autofix.held.remove(url);
    let record = app.autofix.ledger.get(url).cloned().unwrap_or_default();
    let mut form = AutofixForm::new(watch.project, pr, detail, status.health.conflicts);
    if record.handled.as_ref() == Some(&form.fingerprint()) {
        return;
    }
    match app.autofix_mode {
        Mode::Off => {}
        Mode::Auto if record.attempts < AUTO_ATTEMPTS => {
            form.peers = peers;
            dispatch(app, form, out);
        }
        Mode::Auto => give_up(app, &form, record.attempts),
    }
    app.dirty = true;
}

/// [`AUTO_ATTEMPTS`] sends have not turned the PR green: say so — a flash,
/// and the desktop while the window is away — and leave this breakage be
/// until a push or a different failure.
fn give_up(app: &mut App, form: &AutofixForm, attempts: u8) {
    let text = format!(
        "{} is still failing after {attempts} autofix runs: over to you",
        form.pr.label()
    );
    if app.may_notify_desktop() {
        crate::event_loop::alerts::notify_text("Autofix", &text);
    }
    app.flash = Some(Flash::failed(text));
    app.autofix.record(&form.pr.url).handled = Some(form.fingerprint());
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
    let open = app.prs.status_or_open(url).is_open();
    match detail.or_else(|| app.pr_detail.get(url)).cloned() {
        Some(_) if !open => {
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

/// Whether `agent` works in one of `project`'s checkouts of `branch`.
fn on_branch(app: &App, agent: &orion_core::Agent, project: &ProjectId, branch: &str) -> bool {
    app.tree
        .worktrees
        .iter()
        .any(|w| w.id == agent.worktree_id && &w.project_id == project && w.branch == branch)
}

/// An `autofix-<n>` for `pr` is already starting, at work or waiting on
/// the user: there is never a second one on a pull request.
fn fixer_on_it(app: &App, project: &ProjectId, pr: &OpenPr) -> bool {
    fixing(app, project, &pr.head, pr.number)
}

/// Whether an `autofix-<number>` is starting, at work or waiting on the
/// user in one of `project`'s checkouts of `branch` — what the DIFF
/// VIEWER's Enter on a conflicted file leaves the conflict to.
pub(crate) fn fixing(app: &App, project: &ProjectId, branch: &str, number: u64) -> bool {
    let fixer = agent_name(number);
    app.tree.agents.iter().any(|agent| {
        !agent.archived
            && agent.name == fixer
            && matches!(
                agent.status,
                AgentStatus::Fresh | AgentStatus::Running | AgentStatus::NeedsFeedback
            )
            && on_branch(app, agent, project, branch)
    })
}

/// The other sessions live on `pr`'s branch — mid-turn, or waiting on the
/// user — that the agent checks with before it starts ([`Peer`]).
fn live_on_branch<'a>(
    app: &'a App,
    project: &ProjectId,
    pr: &OpenPr,
) -> Vec<&'a orion_core::Agent> {
    app.tree
        .agents
        .iter()
        .filter(|agent| {
            !agent.archived
                && !agent.name.starts_with("autofix-")
                && matches!(
                    agent.status,
                    AgentStatus::Running | AgentStatus::NeedsFeedback
                )
                && on_branch(app, agent, project, &pr.head)
        })
        .collect()
}

/// Where Claude Code keeps `agent`'s conversation: its account's config
/// dir, `projects/<cwd slug>/<session id>.jsonl` — looked for under every
/// slug, as `orion-daemon`'s CONTINUE ON does. None off a Claude account,
/// before the session has an id, or with no such file.
fn transcript_of(agent: &orion_core::Agent) -> Option<PathBuf> {
    let session = agent.session_id.as_deref()?;
    let id = match agent.kind {
        orion_core::AgentKind::Custom => agent.custom_harness.as_deref()?.trim().to_string(),
        kind => kind.as_str().to_string(),
    };
    let entry = crate::config::Config::load().effective_harness_by_id(&id);
    let dir = crate::claude_accounts::dir_of(&entry)?;
    let file = format!("{session}.jsonl");
    std::fs::read_dir(dir.join("projects"))
        .ok()?
        .flatten()
        .map(|slug| slug.path().join(&file))
        .find(|path| path.is_file())
}

/// The session whose harness, account, model and effort the agent copies:
/// the one that did the work on the branch — the most recently active
/// there, `autofix-<n>`s aside. None when nothing has run on it here.
fn source_agent<'a>(
    app: &'a App,
    project: &ProjectId,
    pr: &OpenPr,
) -> Option<&'a orion_core::Agent> {
    app.tree
        .agents
        .iter()
        .filter(|a| {
            !a.archived && !a.name.starts_with("autofix-") && on_branch(app, a, project, &pr.head)
        })
        .max_by_key(|a| crate::app::last_interaction_ms(a))
}

/// The name `orion autofix stand-down` gives the session it archives,
/// ahead of its reason: what [`note_stand_downs`] knows it by.
pub const STOOD_DOWN: &str = "autofix · stood down: ";

/// The git beat: flash each `autofix-<n>` that stood down since the last
/// look, with its reason. The first look only takes note — those were
/// archived before this run.
pub fn note_stand_downs(app: &mut App) {
    let down: Vec<(orion_core::AgentId, String)> = app
        .tree
        .agents
        .iter()
        .filter(|a| a.archived)
        .filter_map(|a| {
            let why = a.name.strip_prefix(STOOD_DOWN)?;
            Some((a.id.clone(), why.to_string()))
        })
        .collect();
    let Some(seen) = &mut app.autofix.stood_down else {
        app.autofix.stood_down = Some(down.into_iter().map(|(id, _)| id).collect());
        return;
    };
    for (id, why) in down {
        if seen.insert(id) {
            app.flash = Some(Flash::note(format!("Autofix stood down: {why}")));
            app.dirty = true;
        }
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

/// orion's own text for the PR autofix SYSTEM PRESET, as `(prefix,
/// postfix)`: the rules, then — after what broke — how to finish. What the
/// agent is told while the preset holds none of the user's
/// ([`crate::agent_presets::SystemPreset::builtin_text`]).
pub const PRESET_TEXT: (&str, &str) = (
    "You are orion's PR autofixer. Fix the pull request below so it can merge, and push the fix to its branch. Do not open a new pull request, force-push, merge it, or change unrelated code.

Rules:
- Fix only what is listed under \"Fix\" below.
- Merge conflicts: fetch the base branch from origin and merge it. Resolve each conflict by understanding both sides' intent; never drop one side's work silently. Regenerate lockfiles and generated files rather than hand-merging them, and commit the merge.
- Failing unit tests: read the failure logs (`gh pr checks`, then `gh run view <run-id> --log-failed`), reproduce each failing test locally with this repo's test command, and fix it. Change a test only when this pull request meant to change the behaviour it checks.
- Failing e2e tests: take the failing specs from the CI logs and re-run only those specs locally, never the full suite — unless earlier attempts failed the same way, then run the full suite once.
- Any other failing check (lint, typecheck, build): reproduce it locally and fix it.",
    "Then commit with a message that says what you fixed and `git push`. Watch the pull request with `gh pr checks --watch` until it is green; when a check fails again, fix it, push, and watch again. Finish with a short summary: what you changed, and anything you could not fix and why.",
);

/// The agent's first prompt: the pull request and the ticked issues —
/// with the failed checks behind each — and the user's note, between the
/// PR autofix preset's prefix and postfix — orion's own
/// ([`PRESET_TEXT`]) while it holds none of the user's.
pub fn prompt(
    pr: &OpenPr,
    refs: &PrRefs,
    diagnosis: &Diagnosis,
    picks: &Picks,
    note: &str,
    preset: &AgentPreset,
) -> String {
    let base = match refs.base.as_str() {
        "" => "the base branch".to_string(),
        name => format!("`{name}`"),
    };
    let ticked: Vec<Issue> = Issue::ALL
        .into_iter()
        .filter(|i| picks[i.index()])
        .collect();
    preset
        .clone()
        .with_builtin_text()
        .compose(&context(pr, refs, diagnosis, &ticked, &base, note))
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
    /// The branch's other live sessions, for an unasked send to check
    /// with first ([`stand_down_preamble`]). Empty from `⌘G`, which the
    /// user asked for.
    pub peers: Vec<Peer>,
}

/// Another session live on the pull request's branch when the agent is
/// sent, and where its conversation is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    /// Waiting on the user, rather than mid-turn.
    pub waiting: bool,
    pub transcript: PathBuf,
}

/// What the agent is told ahead of its task while other sessions are live
/// on the branch: read the end of each one's conversation, and stand down
/// — archiving itself, every file untouched — when one is already on this
/// fix; else carry on, and look again before pushing. Empty with none.
pub fn stand_down_preamble(peers: &[Peer], pr: &OpenPr) -> String {
    if peers.is_empty() {
        return String::new();
    }
    let sessions: Vec<String> = peers
        .iter()
        .map(|p| {
            let state = if p.waiting {
                "waiting on the user"
            } else {
                "working"
            };
            format!("- \"{}\" ({state}): {}", p.name, p.transcript.display())
        })
        .collect();
    format!(
        "Before you change anything, check the other sessions on this branch. Read the end of each one's conversation:\n\n{}\n\nIf any of them is doing work that will land on this same pull request (#{n}): editing, committing or pushing on `{head}` to fix or extend this change, or asking the user something about it, run `orion autofix stand-down \"<session name>: <one-line reason>\"` and stop. Do not edit any files first. If none is, carry on with the task below, and read those conversations once more before you push, standing down the same way if one has started on this fix since.",
        sessions.join("\n"),
        n = pr.number,
        head = pr.head,
    )
}

impl AutofixForm {
    /// The form for `pr` as `detail` diagnoses it, with `conflicts` as
    /// `App::prs` says: what was detected ticked, the caret on the first of
    /// them.
    pub fn new(project: ProjectId, pr: OpenPr, detail: &PrDetail, conflicts: bool) -> Self {
        let diagnosis = diagnose(detail, conflicts);
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
            peers: Vec::new(),
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
    let conflicts = app.prs.status_or_open(&pr.url).health.conflicts;
    let mut form = AutofixForm::new(project, pr, detail, conflicts);
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

/// Send the agent: a PR SESSION named `autofix-<n>`, on the harness,
/// account, model and effort of the session that did the work on the
/// branch ([`source_agent`]) — the PR autofix preset's without one — told
/// [`prompt`], under [`stand_down_preamble`] while other sessions are live
/// there. The ledger counts it and the PR stops being watched.
pub fn dispatch(app: &mut App, form: AutofixForm, out: &mut Vec<ClientRequest>) {
    let Some(worktree) = app.root_worktree(&form.project) else {
        app.flash = Some(Flash::failed(
            "Autofix: the project has no checkout to start from",
        ));
        return;
    };
    // The PR autofix SYSTEM PRESET: its text, and the model and effort
    // the agent runs at when there is no session to copy.
    let preset = crate::agent_presets::system(crate::agent_presets::SystemPreset::PrAutofix);
    let (kind, custom, model, effort) = match source_agent(app, &form.project, &form.pr) {
        Some(a) => (
            a.kind,
            a.custom_harness.clone(),
            a.model.clone(),
            a.effort.clone(),
        ),
        None => crate::config::Config::load().autofix_launch(&preset),
    };
    let task = prompt(
        &form.pr,
        &form.refs,
        &form.diagnosis,
        &form.picks,
        form.note.as_str(),
        &preset,
    );
    let text = [stand_down_preamble(&form.peers, &form.pr), task]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
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
    app.flash = Some(Flash::working(format!(
        "{} started on #{}: {}",
        agent_name(form.pr.number),
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
fn lines(form: &AutofixForm, th: Theme) -> Vec<(Option<Row>, Vec<Span<'static>>)> {
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
    // Drawn where it lands (`draw`), for the mouse to find it there.
    lines.push((Some(Row::Note), Vec::new()));
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
    let lines = lines(form, th);
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
        let spans = match row {
            Some(Row::Note) => note_field(form, rect, th),
            _ => spans,
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

/// The note on its `row`.
fn note_field(form: &AutofixForm, row: Rect, th: Theme) -> Vec<Span<'static>> {
    crate::ui::form_field(
        "Note",
        &form.note,
        "anything the agent should know (optional)",
        form.row == Row::Note,
        row,
        th,
    )
}

/// A form for a pull request with conflicts — what the overlay tests open.
#[cfg(test)]
pub(crate) fn sample_form() -> AutofixForm {
    let detail = PrDetail {
        number: 7,
        url: "https://github.com/o/r/pull/7".into(),
        answered_state: "OPEN".into(),
        answered: crate::pull_request::Answered {
            conflicts: Some(true),
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
        answered_draft: false,
        answered: detail.answered,
        head: "feat".into(),
        mine: true,
        head_sha: String::new(),
        meta: Default::default(),
    };
    AutofixForm::new(ProjectId("p".into()), pr, &detail, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::Answered;

    /// [`diagnose`] with the conflicts the body itself answered.
    fn diag(detail: &PrDetail) -> Diagnosis {
        diagnose(detail, detail.answered.conflicts == Some(true))
    }

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
            answered_state: "OPEN".into(),
            answered: Answered {
                conflicts: Some(conflicts),
                ..Answered::default()
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
            answered_draft: false,
            answered: Default::default(),
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
        let d = diag(&detail(
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
        let a = diag(&detail(
            false,
            vec![
                check("a test", "", CheckState::Failed),
                check("b test", "", CheckState::Failed),
            ],
        ));
        let b = diag(&detail(
            false,
            vec![
                check("b test", "", CheckState::Failed),
                check("a test", "", CheckState::Failed),
            ],
        ));
        assert_eq!(a.fingerprint("x"), b.fingerprint("x"));
        assert_ne!(a.fingerprint("x"), a.fingerprint("y"));
    }

    /// The PR autofix preset as a list that never touched it holds it:
    /// orion's own text.
    fn orions() -> AgentPreset {
        crate::agent_presets::SystemPreset::PrAutofix.default_preset()
    }

    /// The prompt names only what is ticked, between orion's rules and how
    /// to finish: fix only what is listed, re-run only the failed e2e specs,
    /// push, and watch the PR until it is green.
    #[test]
    fn the_prompt_says_only_what_is_ticked_under_orions_rules() {
        let detail = detail(true, vec![check("e2e", "", CheckState::Failed)]);
        let text = prompt(
            &pr(),
            &PrRefs::of(&detail),
            &diag(&detail),
            &[false, false, true, false],
            "flaky on CI",
            &orions(),
        );
        assert!(text.starts_with("You are orion's PR autofixer."), "{text}");
        assert!(text.contains("https://github.com/o/r/pull/7"));
        assert!(text.contains("`feat` into `main`"));
        assert!(text.contains("https://ci/e2e"));
        assert!(text.contains("Note from the user: flaky on CI"));
        let fix = &text[text.find("\nFix:\n").unwrap()..text.find("Note from the user").unwrap()];
        assert!(fix.contains("- Failing e2e tests:"), "{fix}");
        assert!(!fix.contains("- Merge conflicts"), "{fix}");
        assert!(!fix.contains("- Failing unit tests"), "{fix}");
        for rule in [
            "Fix only what is listed",
            "re-run only those specs locally, never the full suite — unless earlier attempts failed the same way",
            "reproduce each failing test locally",
            "`gh pr checks --watch` until it is green",
        ] {
            assert!(text.contains(rule), "{rule}:\n{text}");
        }
        let at = |needle: &str| text.find(needle).unwrap();
        assert!(at("Rules:") < at("Pull request:"), "the rules lead");
        assert!(
            at("Note from the user") < at("git push"),
            "finishing ends it"
        );
    }

    #[test]
    fn a_preset_replaces_orions_text_but_keeps_the_context() {
        let detail = detail(true, Vec::new());
        let preset = AgentPreset {
            prefix: "Do it my way.".into(),
            postfix: "Then push.".into(),
            ..orions()
        };
        let text = prompt(
            &pr(),
            &PrRefs::of(&detail),
            &diag(&detail),
            &[true, false, false, false],
            "",
            &preset,
        );
        assert!(text.starts_with("Do it my way."));
        assert!(text.ends_with("Then push."));
        assert!(text.contains("- Merge conflicts with `main`"));
        assert!(!text.contains("orion's PR autofixer"));
    }

    #[test]
    fn the_form_ticks_what_was_detected() {
        let detail = detail(false, vec![check("unit", "", CheckState::Failed)]);
        let form = AutofixForm::new(ProjectId("p".into()), pr(), &detail, false);
        assert_eq!(form.picks, [false, true, false, false]);
        assert_eq!(form.row, Row::Issue(Issue::Unit));
    }

    #[test]
    fn modes_read_leniently() {
        assert_eq!(
            Mode::parse("ASK"),
            Mode::Auto,
            "an older orion's ask sends now"
        );
        assert_eq!(Mode::parse("auto"), Mode::Auto);
        assert_eq!(Mode::parse(" Off "), Mode::Off);
        assert_eq!(Mode::parse(""), Mode::Auto, "on unless switched off");
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
                merged: Vec::new(),
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
            answered: Answered {
                conflicts: Some(conflicts),
                checks: Some(if failing {
                    Checks::Failing
                } else {
                    Checks::Passing
                }),
            },
            ..pr()
        }
    }

    /// The list as `note_open_prs_answer` lands it: its rows observed into
    /// `App::prs`, asked just now, then handed over.
    fn listed(app: &mut App) {
        let project = ProjectId("p".into());
        let list = app.open_prs[&project].list.clone();
        for pr in &list {
            app.prs.observe(
                &pr.url,
                crate::pr_store::PrObservation::of_list_row(pr),
                crate::fetch::Asked::At(crate::fetch::now()),
            );
        }
        note_list(app, &project, &list);
    }

    /// A body as `land_pr_detail` lands it: observed, then handed over.
    fn landed(app: &mut App, detail: &PrDetail) -> Vec<ClientRequest> {
        app.prs.observe(
            &detail.url,
            crate::pr_store::PrObservation::of_detail(detail),
            crate::fetch::Asked::At(crate::fetch::now()),
        );
        let mut out = Vec::new();
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("agent_presets.json"), || {
            crate::config::with_config_path(dir.path().join("config.json"), || {
                land_detail(app, &detail.url.clone(), Some(detail), &mut out)
            })
        });
        out
    }

    /// The prompt the agent `out` starts went with, if one did.
    fn sent_prompt(out: &[ClientRequest]) -> Option<String> {
        out.iter().find_map(|req| match req {
            ClientRequest::CreatePrAgent {
                starting_prompt, ..
            } => starting_prompt.clone(),
            _ => None,
        })
    }

    /// A checkout of the PR's branch with session `name` in it.
    fn session_on_branch(app: &mut App, name: &str, status: AgentStatus) -> orion_core::AgentId {
        let wt = orion_core::WorktreeId("feat-wt".into());
        if !app.tree.worktrees.iter().any(|w| w.id == wt) {
            app.tree.worktrees.push(orion_core::Worktree {
                id: wt.clone(),
                project_id: ProjectId("p".into()),
                path: "/tmp/repo-worktrees/feat".into(),
                branch: pr().head,
                is_main: false,
                sort_order: 0,
            });
        }
        let id = orion_core::AgentId(format!("id-{name}"));
        app.tree.agents.push(orion_core::Agent {
            id: id.clone(),
            worktree_id: wt,
            name: name.into(),
            status,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: orion_core::AgentKind::Codex,
            custom_harness: None,
            model: Some("gpt-5.5".into()),
            effort: Some("high".into()),
            session_id: None,
            sort_order: 0,
            status_changed_at: 1,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
        });
        id
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
        let mut app = app_with(vec![theirs], Mode::Auto);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "not mine");

        let mut app = app_with(vec![broken(true, false)], Mode::Off);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "off");

        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
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
        let mut app = app_with(vec![broken(false, true)], Mode::Auto);
        listed(&mut app);
        let running = detail(
            false,
            vec![
                check("unit", "", CheckState::Failed),
                check("e2e", "", CheckState::Running),
            ],
        );
        let out = landed(&mut app, &running);
        assert!(out.is_empty(), "still settling");
        assert!(
            app.autofix.watching[&pr().url].next_fetch.is_some(),
            "asked again later"
        );

        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        listed(&mut app);
        let out = landed(
            &mut app,
            &detail(true, vec![check("e2e", "", CheckState::Running)]),
        );
        assert!(sent_prompt(&out).is_some(), "conflicts go at once");
        assert!(app.autofix.watching.is_empty());
    }

    #[test]
    fn a_sent_breakage_is_not_sent_again_until_it_changes() {
        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(sent_prompt(&out).is_some(), "sent unasked");
        assert!(app.overlay.is_none(), "no modal");

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
        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
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

    /// One `autofix-<n>` at a time: while one is starting, at work or
    /// waiting on the user, a second is never sent, and the watch waits.
    #[test]
    fn a_second_autofix_never_starts_for_one_pr() {
        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        session_on_branch(&mut app, "autofix-7", AgentStatus::Fresh);
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(out.is_empty(), "{out:?}");
        assert!(
            app.autofix.watching.contains_key(&pr().url),
            "still watched"
        );
    }

    /// A live session on the branch whose conversation can't be read holds
    /// the agent back — said once — rather than send it in blind.
    #[test]
    fn a_live_session_with_no_conversation_to_read_holds_autofix() {
        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        session_on_branch(&mut app, "billing", AgentStatus::Running);
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(out.is_empty(), "{out:?}");
        assert!(app.autofix.watching.contains_key(&pr().url));
        assert!(app
            .flash
            .as_deref()
            .is_some_and(|f| f.contains("billing is busy")));
        app.flash = None;
        landed(&mut app, &detail(true, Vec::new()));
        assert_eq!(app.flash, None, "said once");
    }

    /// The agent runs as the session that did the work on the branch —
    /// its harness, model and effort — and, with another session live
    /// there whose conversation it can read, opens on checking it.
    #[test]
    fn autofix_copies_the_branchs_agent_and_checks_with_the_live_one() {
        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        session_on_branch(&mut app, "worker", AgentStatus::Finished);
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        let copied = out.iter().find_map(|req| match req {
            ClientRequest::CreatePrAgent {
                kind,
                model,
                effort,
                ..
            } => Some((*kind, model.clone(), effort.clone())),
            _ => None,
        });
        assert_eq!(
            copied,
            Some((
                orion_core::AgentKind::Codex,
                Some("gpt-5.5".into()),
                Some("high".into())
            ))
        );
        assert!(!sent_prompt(&out)
            .unwrap()
            .contains("Before you change anything"));

        // A Claude session mid-turn, whose transcript is on disk.
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        let slug = claude.join("projects").join("-tmp-repo-worktrees-feat");
        std::fs::create_dir_all(&slug).unwrap();
        std::fs::write(slug.join("sid-1.jsonl"), "{}\n").unwrap();
        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        let live = session_on_branch(&mut app, "billing", AgentStatus::NeedsFeedback);
        let agent = app.tree.agents.iter_mut().find(|a| a.id == live).unwrap();
        agent.kind = orion_core::AgentKind::Claude;
        agent.session_id = Some("sid-1".into());
        listed(&mut app);
        let places = crate::paths_util::Places {
            home: Some(home.path().to_path_buf()),
            claude: Some(claude.clone()),
            ..Default::default()
        };
        let out =
            crate::paths_util::with_places(places, || landed(&mut app, &detail(true, Vec::new())));
        let prompt = sent_prompt(&out).expect("sent");
        assert!(prompt.starts_with("Before you change anything"), "{prompt}");
        assert!(
            prompt.contains("\"billing\" (waiting on the user)"),
            "{prompt}"
        );
        assert!(prompt.contains("sid-1.jsonl"), "{prompt}");
        assert!(prompt.contains("orion autofix stand-down"), "{prompt}");
        assert!(
            prompt.contains("Merge conflicts"),
            "the task follows: {prompt}"
        );
    }

    /// A stand-down flashes its reason once; the ones archived before
    /// this run are taken note of, never flashed.
    #[test]
    fn a_stand_down_is_flashed_once_with_its_reason() {
        let mut app = app_with(Vec::new(), Mode::Auto);
        let old = session_on_branch(&mut app, "old", AgentStatus::Finished);
        let set = |app: &mut App, id: &orion_core::AgentId, why: &str| {
            let a = app.tree.agents.iter_mut().find(|a| &a.id == id).unwrap();
            a.archived = true;
            a.name = format!("{STOOD_DOWN}{why}");
        };
        set(&mut app, &old, "from before");
        note_stand_downs(&mut app);
        assert_eq!(app.flash, None, "an earlier run's");

        let new = session_on_branch(&mut app, "new", AgentStatus::Running);
        set(&mut app, &new, "billing: already on it");
        note_stand_downs(&mut app);
        assert_eq!(
            app.flash.as_deref(),
            Some("Autofix stood down: billing: already on it")
        );
        app.flash = None;
        note_stand_downs(&mut app);
        assert_eq!(app.flash, None, "once");
    }

    /// Green is known, never guessed: while GitHub has not said whether
    /// the branch merges (`mergeable: UNKNOWN`), a list whose checks pass
    /// and a page with nothing failing leave the watch and the attempts
    /// where they are. Once it says the branch merges, the PR is green.
    #[test]
    fn unknown_conflicts_are_never_green() {
        let unsure = |checks: Checks| OpenPr {
            answered: Answered {
                conflicts: None,
                checks: Some(checks),
            },
            ..pr()
        };
        let mut app = app_with(vec![unsure(Checks::Failing)], Mode::Auto);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                handled: None,
                attempts: 2,
            },
        );
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));

        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = vec![unsure(Checks::Passing)];
        listed(&mut app);
        assert!(
            app.autofix.watching.contains_key(&pr().url),
            "still watched"
        );
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 2, "attempts kept");

        let quiet = PrDetail {
            answered: Answered::default(),
            ..detail(false, vec![check("unit", "", CheckState::Passed)])
        };
        landed(&mut app, &quiet);
        assert!(
            app.autofix.watching.contains_key(&pr().url),
            "a page with nothing failing but conflicts unknown keeps it"
        );
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 2);

        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = vec![broken(false, false)];
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "known green");
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 0);
    }

    #[test]
    fn auto_sends_until_its_attempts_run_out_then_says_so() {
        let mut app = app_with(vec![broken(true, false)], Mode::Auto);
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(app.overlay.is_none(), "sent, not asked");
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
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(out.is_empty(), "no fourth agent");
        assert!(app.overlay.is_none(), "and no modal");
        assert!(app
            .flash
            .as_deref()
            .is_some_and(|f| f.contains("still failing after 3")));
        assert!(
            app.autofix.ledger[&pr().url].handled.is_some(),
            "said once per breakage"
        );
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
            true,
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
        let mut app = app_with(vec![row(1)], Mode::Auto);
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
}
