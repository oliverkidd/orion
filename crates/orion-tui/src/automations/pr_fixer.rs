//! The PR FIXER (AUTOFIX): an agent sent to mend one of your pull requests — its merge
//! conflicts, its failing unit and e2e tests, its other failing checks —
//! that runs them locally until they pass and pushes the fix to the PR.
//!
//! * **Seeing it break.** Every project's open-PR list (`event_loop::
//!   note_open_prs_answer`) passes through [`note_list`]: a pull request
//!   the `gh` user opened (`OpenPr::mine`) that is ready for review and in
//!   trouble is *watched*, and its body (`PrDetail`, the one place check
//!   names live) is asked for. A draft is left alone until it is marked
//!   ready, `⌘G` aside — except a **factory draft** (its branch is in the
//!   builder ledger, `linear_builder::factory_entry`), whose failing checks
//!   are fixed at once and again until green, its conflicts left until it
//!   is ready. [`land_detail`] reads each body that lands. Checks still running
//!   beside a failed one are waited out, up to [`SETTLE_LIMIT`], so one
//!   agent sees every failure at once; conflicts alone are acted on
//!   straight away. The rows are the clock — a project's open list, and
//!   the cross-repo `MyPrs` read that keeps your rows current while you
//!   are elsewhere (`sync::github`): a body is read again only when the
//!   PR's row moves ([`signal`]: its verdict, a push, a comment or review)
//!   since the body last read ([`Record::read`], kept across launches), by
//!   THE RESOLVER, on its demand ([`detail_needs`]) and its backoff. No
//!   timer of its own, but two scoped ones: a breakage whose checks are
//!   still settling is read every [`SETTLE_RECHECK`], and one whose branch
//!   agent's context is being measured every [`MEASURE_RECHECK`]. One held
//!   back — a session at work on the branch, the factory paused — asks
//!   nothing; it is looked at again from the body in hand
//!   ([`HOLD_RELOOK`]).
//! * **Once per breakage.** What broke is a [`Fingerprint`] — the head
//!   commit, the conflicts and the failing checks' names — and a per-PR
//!   ledger (`Record`, kept in the PR cache) remembers the last one sent or
//!   dismissed: nothing asks again until a push or a different failure. A
//!   session at work in a checkout of the PR's branch — any, not only an
//!   `autofix-<n>` — holds the ask back until it is done, and a PR already
//!   being asked about is not asked about twice.
//! * **Sending, unasked.** The **PR fixer** automation (`⌘J`, on by
//!   default) sends the agent the moment a breakage has settled — no modal,
//!   nothing to confirm — until [`AUTO_ATTEMPTS`] tries in a row have not
//!   turned the PR green, when it stops and says so (a flash, and a desktop
//!   notification while the window is away). `⌘G` in the PULL REQUESTS
//!   MODAL, or on the pane's PULL REQUEST PAGE ([`open_asked`]), still
//!   opens the AUTOFIX MODAL for any PR — the issues as a checklist and a
//!   note — whatever the setting. An unasked send never waits for a
//!   place under `automations.max_running` and holds none
//!   ([`super::uncapped`]); only the factory's pause at its account's
//!   usage limit holds it back ([`super::paused`]).
//! * **One branch, one agent.** [`dispatch`] hands the breakage to the
//!   FIX ROUTER ([`router`]): an `autofix-<n>` already live there keeps
//!   it; the branch's own agent, idle and under 75% of its context, gets
//!   it as a new turn; busy, it is queued until idle; too full, or with no
//!   agent, a fresh fixer starts with a handover. An `autofix-<n>` whose
//!   pull request has gone green, its turn done, is archived
//!   ([`archive_green`]).
//! * **Cancelled checks.** A cancelled job is no failure: a pull request
//!   whose checks ended with some cancelled and none failed is never sent
//!   an agent. A factory one has its cancelled runs started again with
//!   `gh run rerun`, once per head commit, the ledger keeping the commit
//!   (`Record::rerun`) so it is not asked twice ([`rerun_cancelled`]).
//! * **The owner's comments.** On a factory pull request, what the owner
//!   writes on GitHub — review comments with their file and line, and
//!   conversation comments — goes to the branch's agent through the same
//!   router, verbatim, once each ([`relay_comments`]).
//! * **Visitors and the checkout look.** A session started in another
//!   project that writes in the PR's checkout (`cd …/feature && git
//!   commit`) is on the branch for [`works_on`], from the daemon's visit
//!   book (`App::visits`): it is the branch's agent, told where the
//!   checkout is ([`visiting`]). Before a fresh fixer goes in, the checkout
//!   itself is looked at ([`checkout`]): someone orion can't see at work
//!   there holds it back.
//! * **The repo's flow.** A repo whose PR contract says `[flow] ready =
//!   "when-clean"` has its drafts fixed like ready pull requests, and each
//!   marked ready once clean ([`flow`]).
//! * **Cubic's findings.** Each of your open pull requests not in trouble
//!   — its checks still running count — is read again when its row moves
//!   ([`poll`]), so Cubic's findings go the moment they post and their
//!   push cancels the run in progress. A broken one has them sent with the
//!   breakage, in one turn ([`bundle_cubic`]). The Cubic threads on it that
//!   nobody has answered and that were never sent go to the branch's agent through the same router —
//!   the session that built it, which knows what was left out on purpose
//!   — to fix what is right as a new commit on top (so Cubic reviews the
//!   fix) and say why on the thread of each one it leaves
//!   ([`relay_cubic`]). Each thread goes once; after [`CUBIC_ROUNDS`]
//!   rounds what Cubic finds is the owner's.
//! * **Giving up, on a factory PR**, is a *Can't fix* card in Slack
//!   ([`give_up`]): Open PR (a link), Reply… (words for the branch's agent, and
//!   the tries start over), Park (left alone until it goes green).
//! * **A fresh fixer.** [`launch_fresh`] launches a PR SESSION
//!   (`CreatePrAgent`, so the DAEMON finds or cuts the PR's worktree) named
//!   `autofix-<n>` on the harness, account, model and effort of the session
//!   that did the work on the branch ([`source_agent`]) — with none, the
//!   default agent at the **PR autofix** SYSTEM PRESET's model and effort,
//!   and a harness that preset pins over both ([`launch_of`]) — its first
//!   prompt [`prompt`]: the pull request and what failed, at that preset's
//!   `{task}`, its text orion's own rules
//!   ([`PRESET_TEXT`]) while it holds none of the user's, and the
//!   automation's extra steer after it ([`super::compose`]). `⌘U` in
//!   Settings › Automations can put another preset in that one's place
//!   ([`super::preset_of`]). Every one is tagged `pr_fixer`.
//! * **Standing down.** Another session live on the branch — mid-turn, or
//!   waiting on the user — does not hold the agent back. It is told where
//!   that session's conversation is ([`stand_down_preamble`]) and reads it
//!   first: when that session is already on this fix, the agent runs
//!   `orion autofix stand-down "<why>"`, which archives it, and the flash
//!   says why ([`note_stand_downs`]). A live session whose conversation
//!   can't be read holds it back instead, as every busy one used to.
//!
//! What failed is one **Failing checks** list, grouped by the workflow
//! each check ran in, as GitHub groups them: which of them is a unit test,
//! an e2e spec or a lint is the agent's to read off the logs, under the
//! preset's rules for each.

use super::{Automation, Dot, Panel, PanelList, PanelRow, Runner, Tone};
use crate::agent_presets::AgentPreset;
use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::hints::Hint;
use crate::pr_store::PrStatus;
use crate::pull_request::{CheckState, Checks, OpenPr, PrCheck, PrDetail, PrLaunch};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::{AgentKind, AgentStatus, ClientRequest, ProjectId, SessionRef};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(crate) mod checkout;
pub(crate) mod flow;
pub(crate) mod router;
use router::Work;

/// How often a breakage's body is read again while checks beside its
/// failure are still running (up to [`SETTLE_LIMIT`]): GitHub's rollup
/// may already say failing and not move again when the rest finish, so
/// the row alone can't say when they have.
pub const SETTLE_RECHECK: Duration = Duration::from_secs(60);
/// How soon a body is read again while its branch agent's context is
/// being measured ([`router::measure`]).
const MEASURE_RECHECK: Duration = Duration::from_secs(3);
/// How often a breakage held back — a session at work on its branch, the
/// factory paused, a peer whose conversation can't be read — is looked at
/// again, from the body in hand: no GitHub, a local decision.
pub const HOLD_RELOOK: Duration = Duration::from_secs(30);
/// How long `gh run rerun` is given to start a cancelled run again.
const RERUN_TIMEOUT: Duration = Duration::from_secs(30);
/// How long running checks are waited out before acting on the failures
/// already in.
pub const SETTLE_LIMIT: Duration = Duration::from_secs(20 * 60);
/// `auto` sends this many times in a row without the PR going green, then
/// asks.
pub const AUTO_ATTEMPTS: u8 = 3;
/// How many rounds of Cubic's findings go to a pull request's agent;
/// after them, what Cubic finds is the owner's.
pub const CUBIC_ROUNDS: u8 = 3;

/// One kind of thing the agent can be sent to fix — a row of the form, in
/// [`Issue::ALL`] order (which is also its index in [`Picks`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Issue {
    Conflicts,
    Checks,
}

/// What is ticked, by [`Issue`] index.
pub type Picks = [bool; Issue::ALL.len()];

impl Issue {
    pub const ALL: [Issue; 2] = [Issue::Conflicts, Issue::Checks];

    /// Its row on the form, and its heading in the prompt's list.
    pub fn label(self) -> &'static str {
        match self {
            Issue::Conflicts => "Merge conflicts",
            Issue::Checks => "Failing checks",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// What is wrong with a pull request, as its body says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnosis {
    pub conflicts: bool,
    /// The failed checks, in the order GitHub listed them.
    pub failing: Vec<PrCheck>,
    /// The cancelled checks — no failure, nothing for an agent to fix, but
    /// a factory pull request's are started again ([`rerun_cancelled`]).
    pub cancelled: Vec<PrCheck>,
    /// Some check has not finished.
    pub running: bool,
    /// The files its head conflicts in with its base, as the repo merged
    /// them ([`want_conflicts`]); none until that is known, or when it
    /// could not say.
    pub conflicted: Vec<String>,
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
            CheckState::Failed => out.failing.push(check.clone()),
            CheckState::Running => out.running = true,
            CheckState::Cancelled => out.cancelled.push(check.clone()),
            CheckState::Passed | CheckState::Skipped => {}
        }
    }
    out
}

impl Diagnosis {
    /// Nothing to fix.
    pub fn is_empty(&self) -> bool {
        !self.conflicts && self.failing.is_empty()
    }

    /// Only conflicts — nothing failed that running checks could add to.
    fn conflicts_only(&self) -> bool {
        self.conflicts && self.failing.is_empty()
    }

    /// The Actions runs to start again when nothing is wrong but some
    /// checks were cancelled — nothing failed, nothing conflicts, nothing
    /// still running: each cancelled check's run, once, in the order they
    /// came. None otherwise, or for a cancelled check that is no Actions
    /// job.
    pub fn cancelled_runs(&self) -> Vec<u64> {
        if !self.is_empty() || self.running {
            return Vec::new();
        }
        let mut runs = Vec::new();
        for id in self.cancelled.iter().filter_map(PrCheck::run_id) {
            if !runs.contains(&id) {
                runs.push(id);
            }
        }
        runs
    }

    /// Whether `issue` was seen.
    pub fn has(&self, issue: Issue) -> bool {
        match issue {
            Issue::Conflicts => self.conflicts,
            Issue::Checks => !self.failing.is_empty(),
        }
    }

    /// The failed checks grouped by workflow, as the prompt and the form
    /// list them (`pr_preview::by_workflow`).
    pub fn failing_by_workflow(&self) -> Vec<(String, Vec<&PrCheck>)> {
        crate::pr_preview::by_workflow(&self.failing)
    }

    /// This breakage, at `head_sha`.
    pub fn fingerprint(&self, head_sha: &str) -> Fingerprint {
        let mut checks: Vec<String> = self.failing.iter().map(|c| c.name.clone()).collect();
        checks.sort_unstable();
        Fingerprint {
            sha: head_sha.to_string(),
            conflicts: self.conflicts,
            checks,
        }
    }

    /// `merge conflicts · 2 failing checks`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.conflicts {
            parts.push("merge conflicts".to_string());
        }
        if !self.failing.is_empty() {
            parts.push(crate::flash::plural(self.failing.len(), "failing check"));
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
    /// *Can't fix*'s Park: left alone until it goes green or `⌘G`.
    #[serde(default)]
    pub parked: bool,
    /// The newest of the owner's comments already relayed (RFC 3339),
    /// or seen on the first look; None before that look.
    #[serde(default)]
    pub comments_seen: Option<String>,
    /// The head commit whose cancelled runs were last started again
    /// ([`rerun_cancelled`]): not again on that commit.
    #[serde(default)]
    pub rerun: Option<String>,
    /// The Cubic review threads already sent to an agent, or passed over
    /// once the rounds ran out ([`relay_cubic`]): never sent again.
    #[serde(default)]
    pub cubic_sent: Vec<String>,
    /// Rounds of Cubic's findings sent ([`CUBIC_ROUNDS`] at most).
    #[serde(default)]
    pub cubic_rounds: u8,
    /// Its row's [`signal`] when its body was last read for the fixer: a
    /// row that has not moved since is not read again — not on the next
    /// list, and not on the next launch.
    #[serde(default)]
    pub read: Option<String>,
    /// The head commit the fixer last marked it ready at
    /// ([`flow::mark_ready`]): not asked again on that commit.
    #[serde(default)]
    pub readied: Option<String>,
}

/// A watched pull request: in trouble, its body read when its row moves.
#[derive(Debug, Clone)]
pub struct Watch {
    pub project: ProjectId,
    pub number: u64,
    pub dir: PathBuf,
    pub first_seen: Instant,
    /// Checks beside its failure still running when its body last
    /// landed: read every [`SETTLE_RECHECK`] until they settle.
    pub settling: bool,
    /// Its branch agent's context being measured: read every
    /// [`MEASURE_RECHECK`] until it is in.
    pub measuring: bool,
    /// Held back when its body last landed, looked at again from that
    /// body at this time ([`HOLD_RELOOK`]).
    pub relook: Option<Instant>,
}

/// What of a pull request's row says its body may have moved: its head
/// commit, conflicts and checks as `App::prs` has them (an unknown answer
/// never overwriting a known one), whether it is a draft, and its talk —
/// the comment and review-thread counts and `updatedAt`, which a reply
/// in a thread moves too. A body read at one signal is read again only
/// once the row says another.
pub fn signal(app: &App, pr: &OpenPr) -> String {
    let status = app.prs.status_or_open(&pr.url);
    format!(
        "{}|{:?}:{:?}|{:?}:{:?}|{:?}|{}|{}|{}",
        status.head_sha,
        status.conflicts_known,
        status.health.conflicts,
        status.checks_known,
        status.health.checks,
        status.standing,
        pr.meta.comments,
        pr.meta.threads,
        pr.meta.updated_at
    )
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
    /// By PR URL: the files its head conflicts in with its base, as
    /// `conflicts::local` answered ([`want_conflicts`]).
    pub conflicted: HashMap<String, Conflicted>,
    /// Sends waiting on their pull request's [`Conflicted`] answer, sent on
    /// the beat once it is in ([`send_waiting`]). One per pull request.
    pub waiting: Vec<AutofixForm>,
    /// Work held for a busy branch agent, routed again once it is idle
    /// ([`router::deliver_queued`]).
    pub queued: Vec<router::Queued>,
    /// By PR URL: the factory pull requests whose bodies are read for the
    /// owner's comments ([`relay_comments`]).
    pub polls: HashMap<String, Poll>,
    /// By agent: how full its context was when last read
    /// ([`router::measure`]).
    pub context: HashMap<orion_core::AgentId, router::Context>,
    /// By agent: context reads out on the blocking pool, and since when.
    pub measuring: HashMap<orion_core::AgentId, Instant>,
    /// By PR URL: bodies the fixer wants read, and the row [`signal`] it
    /// wanted them at — THE RESOLVER's background demand
    /// ([`detail_needs`]) until one lands.
    pub want: HashMap<String, String>,
    /// By PR URL: fresh fixers launched and not in the tree yet.
    pub launches: HashMap<String, router::Launch>,
    /// By PR URL: Cubic findings held back — the factory paused at its
    /// limit, or the owner in the session — and when to try again
    /// ([`retry_cubic`]).
    pub cubic_retry: HashMap<String, Instant>,
    /// By PR URL: whether its checkout was in use when last looked at,
    /// before a fresh fixer goes in ([`checkout`]).
    pub checkouts: HashMap<String, checkout::Look>,
    /// By PR URL: checkout looks out on the blocking pool, and since when.
    pub looking: HashMap<String, Instant>,
    /// By project: its PR flow as last read ([`flow`]).
    pub flows: HashMap<ProjectId, flow::Known>,
    /// By project: flow reads out on the blocking pool, and since when.
    pub flow_reads: HashMap<ProjectId, Instant>,
}

/// A pull request whose body is read for its talk: a factory one for the
/// owner's comments, a green one of yours for Cubic's findings — read
/// again whenever its row's [`signal`] moves.
#[derive(Debug, Clone)]
pub struct Poll {
    pub project: ProjectId,
    pub number: u64,
    pub dir: PathBuf,
    /// A factory pull request: the owner's comments are relayed.
    pub factory: bool,
    /// Green, with the PR fixer on: Cubic's findings are sent
    /// ([`cubic_wanted`]).
    pub cubic: bool,
}

/// What a pull request's head was asked to merge with its base — the
/// files that conflict, once `conflicts::local` has said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflicted {
    head_sha: String,
    base: String,
    /// None while the ask is out; the files once it landed — none when the
    /// repo could not say.
    files: Option<Vec<String>>,
}

impl Conflicted {
    /// Whether this is the answer for `head_sha` against `base`.
    fn is_for(&self, head_sha: &str, base: &str) -> bool {
        self.head_sha == head_sha && self.base == base
    }
}

impl State {
    fn record(&mut self, url: &str) -> &mut Record {
        self.dirty = true;
        self.ledger.entry(url.to_string()).or_default()
    }

    /// Look at `url`'s breakage again from the body in hand after
    /// [`HOLD_RELOOK`]: what held it back may have let go.
    fn relook_later(&mut self, url: &str) {
        if let Some(w) = self.watching.get_mut(url) {
            w.relook = Some(Instant::now() + HOLD_RELOOK);
        }
    }

    /// Stop watching `url`.
    fn forget(&mut self, url: &str) {
        self.watching.remove(url);
        self.held.remove(url);
        self.conflicted.remove(url);
        self.checkouts.remove(url);
        if !self.polls.contains_key(url) {
            self.want.remove(url);
        }
    }

    /// A send for `url` is waiting on its conflicting files.
    fn is_waiting(&self, url: &str) -> bool {
        self.waiting.iter().any(|form| form.pr.url == url)
    }
}

// ---- detection ----

/// A project's open-PR list landed — or `MyPrs` replaced its rows of
/// yours: watch each of the user's PRs in trouble that this breakage has
/// not been handled for, have a watched or polled one's body read again
/// when its row moved since it was last read ([`ask_if_moved`]), and
/// forget the attempts of each one that has gone green. Where each stands is
/// `App::prs`'s word, the list already observed into it. Green is known,
/// never guessed ([`PrStatus::is_green`]): while GitHub has not said
/// whether the branch merges, or checks are still running, a watch and its
/// attempts are left as they are. A draft is dropped — a factory draft
/// aside, watched for its failing checks alone ([`fixer_view`]).
pub fn note_list(app: &mut App, project: &ProjectId, list: &[OpenPr]) {
    flow::refresh(app, project);
    let fixes_drafts = flow::fixes_drafts(app, project);
    // orion's own knowledge pull request is orion's to rebuild and merge
    // (`knowledge::share`); a fixer pushing to its branch would race it.
    for pr in list
        .iter()
        .filter(|pr| pr.mine && !pr.head.starts_with(crate::knowledge::share::PREFIX))
    {
        let factory = is_factory(app, project, &pr.head);
        let status = fixer_view(app.prs.status_or_open(&pr.url), factory);
        let cubic = cubic_wanted(app, &status, factory || fixes_drafts);
        match factory || cubic {
            true => poll(app, project, pr, factory, cubic),
            false => {
                app.autofix.polls.remove(&pr.url);
                if !app.autofix.watching.contains_key(&pr.url) {
                    app.autofix.want.remove(&pr.url);
                }
            }
        }
        if status.is_green() {
            close_cant_fix(app, &pr.url, "Green now");
            let record = app.autofix.ledger.get(&pr.url);
            if record.is_some_and(|r| r.attempts > 0 || r.parked) {
                let record = app.autofix.record(&pr.url);
                record.attempts = 0;
                record.parked = false;
            }
            app.autofix.forget(&pr.url);
            app.autofix
                .queued
                .retain(|q| !(q.work.is_fix() && q.work.pr().url == pr.url));
            if status.is_draft() && fixes_drafts {
                if let Some(detail) = app.pr_detail.get(&pr.url).cloned() {
                    flow::mark_ready(app, project, pr, &detail);
                }
            }
            continue;
        }
        // A draft isn't asking for anything yet: no unasked fixer — unless
        // the repo's flow has orion mark drafts ready, when a draft is
        // where the fixing happens. `⌘G` still opens the form for it, and
        // marking it ready brings it back.
        if status.is_draft() && !factory && !fixes_drafts {
            app.autofix.forget(&pr.url);
            continue;
        }
        if app.autofix.watching.contains_key(&pr.url) {
            ask_if_moved(app, pr);
            continue;
        }
        // Not in trouble, but not known green either: nothing to watch —
        // unless it is a factory pull request whose checks ended
        // cancelled, whose body is read for the runs to start again.
        if fixer_trouble(&status).is_none() {
            if factory && wants_rerun(app, &pr.url, &status) {
                watch(app, project, pr);
            }
            continue;
        }
        if !app.automations.is_on(Automation::PrFixer) {
            continue;
        }
        let record = app.autofix.ledger.get(&pr.url);
        let handled = record
            .and_then(|r| r.handled.as_ref())
            .is_some_and(|f| f.covers(&status));
        if handled || record.is_some_and(|r| r.parked) {
            continue;
        }
        watch(app, project, pr);
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
    app.autofix
        .polls
        .retain(|url, p| &p.project != project || open.contains(url.as_str()));
    let (watching, polls) = (&app.autofix.watching, &app.autofix.polls);
    app.autofix
        .want
        .retain(|url, _| watching.contains_key(url) || polls.contains_key(url));
    close_settled_cant_fix(app);
}

/// Have `pr`'s body read when its row has moved ([`signal`]) since the
/// body was last read for the fixer — or since it was last asked for, a
/// read still on its way — and not otherwise. Asked: wanted
/// ([`State::want`], THE RESOLVER's demand) and its page invalidated, so
/// it is read now, a read already running owing one more.
fn ask_if_moved(app: &mut App, pr: &OpenPr) {
    let now = signal(app, pr);
    let read = app
        .autofix
        .ledger
        .get(&pr.url)
        .and_then(|r| r.read.as_deref());
    let asked = app.autofix.want.get(&pr.url).map(String::as_str);
    if read == Some(now.as_str()) || asked == Some(now.as_str()) {
        return;
    }
    app.autofix.want.insert(pr.url.clone(), now);
    app.resolver
        .invalidate(&crate::sync::QueryKey::PrDetail(pr.url.clone()));
}

/// Close each open *Can't fix* card whose pull request has merged or
/// closed since, as `App::prs` has it.
fn close_settled_cant_fix(app: &mut App) {
    let settled: Vec<(String, &str)> = app
        .factory_cards
        .iter()
        .filter(|p| p.state == crate::factory::cards::CardState::Open)
        .filter_map(|p| {
            let url = p
                .card
                .key
                .strip_prefix(CARD_PREFIX)?
                .strip_suffix(CANT_FIX)?;
            match app.prs.status_or_open(url).standing {
                crate::pull_request::Standing::Merged => Some((url.to_string(), "Merged")),
                crate::pull_request::Standing::Closed => Some((url.to_string(), "Closed")),
                _ => None,
            }
        })
        .collect();
    for (url, note) in settled {
        close_cant_fix(app, &url, note);
    }
}

/// Close `url`'s *Can't fix* card with `note`, while it is up.
fn close_cant_fix(app: &mut App, url: &str, note: &str) {
    crate::factory::cards::close_if_up(app, &cant_fix_key(url), note);
}

/// Whether `branch` in `project` is a factory branch: one the bug fixer
/// or the feature builder cut, in the builder ledger.
pub(crate) fn is_factory(app: &App, project: &ProjectId, branch: &str) -> bool {
    super::linear_builder::factory_entry(app, project, branch).is_some()
}

/// The ticket (`RIPLO-123`) a factory branch was cut for: its run
/// record's name.
pub(crate) fn factory_ticket(app: &App, project: &ProjectId, branch: &str) -> Option<String> {
    super::linear_builder::factory_entry(app, project, branch).map(|e| e.identifier.clone())
}

/// A pull request as the fixer sees it: a factory draft's conflicts are
/// left until it is ready, so it reads as though it had none — its checks
/// alone decide whether it is in trouble, and whether it is green.
fn fixer_view(mut status: PrStatus, factory: bool) -> PrStatus {
    if factory && status.is_draft() {
        status.health.conflicts = false;
    }
    status
}

/// Whether a factory pull request standing at `status` has checks that
/// ended cancelled — nothing failed, nothing running — on a head commit
/// whose cancelled runs were not started again yet, with the PR fixer on
/// and the pull request not parked.
fn wants_rerun(app: &App, url: &str, status: &PrStatus) -> bool {
    let record = app.autofix.ledger.get(url);
    status.health.checks == Checks::Cancelled
        && status.is_open()
        && !status.head_sha.is_empty()
        && app.automations.is_on(Automation::PrFixer)
        && !record.is_some_and(|r| r.parked)
        && record.and_then(|r| r.rerun.as_deref()) != Some(status.head_sha.as_str())
}

/// A factory pull request whose body shows checks that ended cancelled
/// and nothing else wrong: start its cancelled runs again with `gh run
/// rerun <run id>` — no agent, since a cancelled job said nothing about
/// the code — once per head commit, the commit kept in the ledger
/// (`Record::rerun`) so a later look does not ask again. The runs it
/// started; none when this commit's were already, or nothing is only
/// cancelled.
fn rerun_cancelled(
    app: &mut App,
    url: &str,
    dir: &std::path::Path,
    detail: &PrDetail,
    diagnosis: &Diagnosis,
) -> Vec<u64> {
    if diagnosis.cancelled.is_empty() || !diagnosis.is_empty() || diagnosis.running {
        return Vec::new();
    }
    let sha = match detail.head_sha.as_str() {
        "" => app.prs.status_or_open(url).head_sha,
        sha => sha.to_string(),
    };
    if sha.is_empty() || app.autofix.ledger.get(url).and_then(|r| r.rerun.as_deref()) == Some(&sha)
    {
        return Vec::new();
    }
    // Kept even when no run could be named, so the body is not read again
    // for it on every list.
    app.autofix.record(url).rerun = Some(sha);
    let runs = diagnosis.cancelled_runs();
    if runs.is_empty() {
        return runs;
    }
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let (dir, ids) = (dir.to_path_buf(), runs.clone());
        rt.spawn(async move {
            for id in ids {
                let id = id.to_string();
                crate::pull_request::gh(Some(&dir), &["run", "rerun", &id], RERUN_TIMEOUT).await;
            }
        });
    }
    app.flash = Some(Flash::note(format!(
        "PR fixer re-ran {} on #{}",
        crate::flash::plural(diagnosis.cancelled.len(), "cancelled check"),
        detail.number
    )));
    app.dirty = true;
    runs
}

/// What the fixer acts on in a pull request standing at `status` (seen
/// through [`fixer_view`]): a ready one's trouble, and a (factory) draft's
/// failing checks — a plain draft never gets this far.
fn fixer_trouble(status: &PrStatus) -> Option<crate::pull_request::Trouble> {
    match status.standing {
        crate::pull_request::Standing::Draft => status.health.trouble(),
        _ => status.trouble(),
    }
}

/// Read `pr`'s body for its talk — a factory pull request's for the
/// owner's comments, a green one's for Cubic's findings (`cubic`): the
/// first time, and whenever its row moves ([`ask_if_moved`]) — a comment,
/// a review or a reply moves `updatedAt`, a new thread the thread count,
/// turning green the checks. Never on a clock.
fn poll(app: &mut App, project: &ProjectId, pr: &OpenPr, factory: bool, cubic: bool) {
    match app.autofix.polls.get_mut(&pr.url) {
        Some(poll) => {
            poll.factory = factory;
            poll.cubic = cubic;
        }
        None => {
            let Some(dir) = repo_dir(app, project) else {
                return;
            };
            app.autofix.polls.insert(
                pr.url.clone(),
                Poll {
                    project: project.clone(),
                    number: pr.number,
                    dir,
                    factory,
                    cubic,
                },
            );
        }
    }
    ask_if_moved(app, pr);
}

/// Whether one of your pull requests, standing at `status` (seen through
/// [`fixer_view`]), has Cubic's findings sent the moment they post: the PR
/// fixer on, open, not a plain draft, and not in trouble — its checks
/// still running count, so the fix is pushed before CI finishes and the
/// run it cancels is not paid for twice. A pull request in trouble has
/// them sent with its breakage instead ([`bundle_cubic`]).
/// `drafts`: a factory pull request, or one in a repo whose flow fixes
/// drafts ([`flow::fixes_drafts`]).
fn cubic_wanted(app: &App, status: &PrStatus, drafts: bool) -> bool {
    app.automations.is_on(Automation::PrFixer)
        && status.is_open()
        && (drafts || !status.is_draft())
        && fixer_trouble(status).is_none()
}

/// A breakage about to go: Cubic's findings on it that nobody answered
/// and that were never sent go in the same turn ([`AutofixForm::cubic`]),
/// so one push answers both — while rounds remain ([`CUBIC_ROUNDS`]).
fn bundle_cubic(app: &mut App, url: &str, detail: &PrDetail, form: &mut AutofixForm) {
    let (new, rounds) = unsent_cubic(app, url, detail);
    if new.is_empty() || rounds >= CUBIC_ROUNDS {
        return;
    }
    let round = rounds + 1;
    form.cubic = router::cubic_turn(&form.pr, &new, round, CUBIC_ROUNDS);
    let ids: Vec<String> = new.iter().map(|t| t.id.clone()).collect();
    let record = app.autofix.record(url);
    record.cubic_sent.extend(ids);
    record.cubic_rounds = round;
}

/// Cubic's threads on `detail` that nobody answered and that were never
/// sent, and how many rounds have gone to `url` so far.
fn unsent_cubic<'a>(
    app: &App,
    url: &str,
    detail: &'a PrDetail,
) -> (Vec<&'a crate::pr_review::github::ReviewThread>, u8) {
    let record = app.autofix.ledger.get(url);
    let sent = record.map_or(&[][..], |r| r.cubic_sent.as_slice());
    let new = crate::pr_body::cubic::unanswered(&detail.threads)
        .filter(|t| !sent.contains(&t.id))
        .collect();
    (new, record.map_or(0, |r| r.cubic_rounds))
}

/// A factory pull request's body landed: the owner's comments on it that
/// are new since the last look go to the branch's agent through the
/// router, verbatim, once each. The first look only takes note.
fn relay_comments(app: &mut App, url: &str, detail: &PrDetail, out: &mut Vec<ClientRequest>) {
    let Some(project) = app
        .autofix
        .polls
        .get(url)
        .filter(|p| p.factory)
        .map(|p| p.project.clone())
    else {
        return;
    };
    let Some(pr) = open_pr(app, &project, url).cloned() else {
        return;
    };
    let comments = router::owner_comments(detail);
    let seen = app
        .autofix
        .ledger
        .get(url)
        .and_then(|r| r.comments_seen.clone());
    let (new, newest) = router::new_since(&comments, seen.as_deref());
    if seen.as_deref() != Some(newest.as_str()) {
        app.autofix.record(url).comments_seen = Some(newest);
    }
    if new.is_empty() {
        return;
    }
    let work = Work::Words {
        text: router::comments_turn(&pr, &new),
        plan: router::plan_comment(detail),
        reason: crate::flash::plural(new.len(), "review comment"),
        project,
        pr: Box::new(pr),
    };
    router::deliver(app, work, out);
}

/// One of your green pull requests' bodies landed: Cubic's threads on it
/// that nobody has answered ([`crate::pr_body::cubic::unanswered`]) and
/// that were never sent go to the branch's agent through the router, in
/// one turn ([`router::cubic_turn`]) — its own agent first, since it
/// knows what was left out on purpose. Each thread goes once. A fresh
/// fixer waits out the factory's pause at its limit — never the cap — and
/// a turn waits while the owner is using the session ([`in_use`]); once
/// [`CUBIC_ROUNDS`] rounds have gone, the owner is told instead.
fn relay_cubic(app: &mut App, url: &str, detail: &PrDetail, out: &mut Vec<ClientRequest>) {
    // The list said green; the body may have landed after a push since.
    let Some((project, factory)) = app
        .autofix
        .polls
        .get(url)
        .filter(|p| p.cubic)
        .map(|p| (p.project.clone(), p.factory))
    else {
        return;
    };
    let status = fixer_view(app.prs.status_or_open(url), factory);
    let drafts = factory || flow::fixes_drafts(app, &project);
    if !cubic_wanted(app, &status, drafts) {
        return;
    }
    let (new, rounds) = unsent_cubic(app, url, detail);
    if new.is_empty() {
        return;
    }
    let Some(pr) = open_pr(app, &project, url).cloned() else {
        return;
    };
    let ids = new.iter().map(|t| t.id.clone());
    if rounds >= CUBIC_ROUNDS {
        app.autofix.record(url).cubic_sent.extend(ids);
        over_to_you(
            app,
            format!(
                "Cubic raised {} more on {} after {CUBIC_ROUNDS} PR fixer rounds: over to you",
                crate::flash::plural(new.len(), "finding"),
                pr.label()
            ),
        );
        app.dirty = true;
        return;
    }
    // A fresh fixer is a new session, outside the cap: only the
    // factory's pause at its account's limit holds it back.
    // The owner's own conversation with the branch's agent isn't cut into
    // either: it waits while they are using that session — tried again
    // from the body in hand ([`retry_cubic`]).
    let wait = match router::route(app, &project, &pr.head, pr.number, url) {
        router::Route::Fresh { .. } => super::paused(app),
        router::Route::Turn(id) => in_use(app, &id),
        _ => false,
    };
    if wait {
        app.autofix
            .cubic_retry
            .insert(url.to_string(), Instant::now() + HOLD_RELOOK);
        return;
    }
    app.autofix.cubic_retry.remove(url);
    let round = rounds + 1;
    let work = Work::Words {
        text: router::cubic_turn(&pr, &new, round, CUBIC_ROUNDS),
        plan: router::plan_comment(detail),
        reason: crate::flash::plural(new.len(), "Cubic finding"),
        project,
        pr: Box::new(pr),
    };
    let record = app.autofix.record(url);
    record.cubic_sent.extend(ids);
    record.cubic_rounds = round;
    router::deliver(app, work, out);
}

/// The beat: each pull request whose Cubic findings [`relay_cubic`] held
/// back, its time come, tried again from the body already in hand.
fn retry_cubic(app: &mut App, out: &mut Vec<ClientRequest>) {
    let now = Instant::now();
    let due: Vec<String> = app
        .autofix
        .cubic_retry
        .iter()
        .filter(|(_, at)| **at <= now)
        .map(|(url, _)| url.clone())
        .collect();
    for url in due {
        app.autofix.cubic_retry.remove(&url);
        if let Some(detail) = app.pr_detail.get(&url).cloned() {
            relay_cubic(app, &url, &detail, out);
        }
    }
}

/// Watch `pr`: its body read now, unless it was read at the row it has
/// now — in this run or the last ([`Record::read`]) — when the body in
/// hand is looked at again instead, no GitHub asked.
fn watch(app: &mut App, project: &ProjectId, pr: &OpenPr) {
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
            settling: false,
            measuring: false,
            relook: None,
        },
    );
    ask_if_moved(app, pr);
    // Read at this row already: the body in hand is the one to decide on.
    let read = !app.autofix.want.contains_key(&pr.url);
    if read && app.pr_detail.contains_key(&pr.url) {
        if let Some(w) = app.autofix.watching.get_mut(&pr.url) {
            w.relook = Some(Instant::now());
        }
    }
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

/// The bodies the fixer needs read, for THE RESOLVER's background demand
/// (`sync::github::needs`): each it wants ([`State::want`]) once — read
/// when invalidated, retried on the resolver's backoff while that fails,
/// never on a clock (`sync::github::ON_CHANGE`) — and the two scoped
/// beats: a breakage settling every [`SETTLE_RECHECK`], one whose branch
/// agent is being measured every [`MEASURE_RECHECK`]. A breakage held
/// back needs nothing ([`HOLD_RELOOK`] looks again from the body in
/// hand), and none while the factory is paused at its account's limit:
/// nothing could be sent.
pub fn detail_needs(app: &App) -> Vec<(String, Duration)> {
    let paused = super::paused(app);
    let held = |url: &str| {
        paused
            && app
                .autofix
                .watching
                .get(url)
                .is_some_and(|w| w.relook.is_some())
    };
    let mut needs: Vec<(String, Duration)> = app
        .autofix
        .want
        .keys()
        .filter(|url| !held(url))
        .map(|url| (url.clone(), crate::sync::github::ON_CHANGE))
        .collect();
    for (url, watch) in &app.autofix.watching {
        if paused {
            continue;
        }
        if watch.measuring {
            needs.push((url.clone(), MEASURE_RECHECK));
        } else if watch.settling {
            needs.push((url.clone(), SETTLE_RECHECK));
        }
    }
    needs
}

/// Whether AUTOFIX is in the middle of something: a breakage watched, a
/// fix queued for a busy agent, or a fresh fixer on its way — when the
/// automations' `MyPrs` read keeps its quicker beat.
pub fn mid_cycle(app: &App) -> bool {
    !app.autofix.watching.is_empty()
        || !app.autofix.queued.is_empty()
        || !app.autofix.launches.is_empty()
}

/// The beat's look at each breakage held back, once its [`HOLD_RELOOK`]
/// is up: its decision taken again from the body in hand — the session
/// that held it may be done, the pause lifted.
fn relook_held(app: &mut App, out: &mut Vec<ClientRequest>) {
    let now = Instant::now();
    let due: Vec<String> = app
        .autofix
        .watching
        .iter()
        .filter(|(_, w)| w.relook.is_some_and(|at| at <= now))
        .map(|(url, _)| url.clone())
        // A fresh read on its way decides instead.
        .filter(|url| !app.autofix.want.contains_key(url))
        .collect();
    for url in due {
        if let Some(w) = app.autofix.watching.get_mut(&url) {
            w.relook = None;
        }
        match app.pr_detail.get(&url).cloned() {
            Some(detail) => decide(app, &url, &detail, out),
            // No body in hand: read one.
            None => {
                app.autofix.want.insert(url.clone(), String::new());
                app.resolver
                    .invalidate(&crate::sync::QueryKey::PrDetail(url));
            }
        }
    }
}

/// A body landed (any body: a watched PR's, or the pane's), already
/// observed into `App::prs`: whatever the fixer wanted read of it is read
/// ([`Record::read`] takes the row's [`signal`], so the next list asks
/// again only once the row moves), its talk relayed, and a watched one's
/// breakage acted on once its checks have settled ([`decide`]). A body
/// that couldn't be read is left wanted: THE RESOLVER tries again on its
/// backoff.
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
    let Some(detail) = detail else {
        return;
    };
    if let Some(wanted) = app.autofix.want.remove(url) {
        let read = open_anywhere(app, url).map_or(wanted, |(_, pr)| signal(app, &pr));
        if app.autofix.ledger.get(url).and_then(|r| r.read.as_ref()) != Some(&read) {
            app.autofix.record(url).read = Some(read);
        }
    }
    let before = (out.len(), app.autofix.queued.len());
    relay_comments(app, url, detail, out);
    relay_cubic(app, url, detail, out);
    let relayed = before != (out.len(), app.autofix.queued.len());
    decide(app, url, detail, out);
    // Clean, with nothing just sent its way: a draft in a `when-clean`
    // repo goes ready.
    if !relayed {
        if let Some((project, pr)) = open_anywhere(app, url) {
            flow::mark_ready(app, &project, &pr, detail);
        }
    }
}

/// A watched pull request's body in hand — just landed, or looked at again
/// ([`relook_held`]): act on its breakage once its checks have settled. A
/// body that shows nothing wrong forgets the watch only once GitHub has
/// said the branch merges; with conflicts unknown it waits for the row to
/// say (a row moving has it read again).
fn decide(app: &mut App, url: &str, detail: &PrDetail, out: &mut Vec<ClientRequest>) {
    let Some(watch) = app.autofix.watching.get_mut(url) else {
        return;
    };
    watch.settling = false;
    watch.measuring = false;
    watch.relook = None;
    let watch = watch.clone();
    let pr = open_pr(app, &watch.project, url).cloned();
    let factory = pr
        .as_ref()
        .is_some_and(|pr| is_factory(app, &watch.project, &pr.head));
    let status = app.prs.status_or_open(url);
    // Turned draft since the list: left alone, as the list would leave it.
    if status.is_draft() && !factory && !flow::fixes_drafts(app, &watch.project) {
        app.autofix.forget(url);
        return;
    }
    let status = fixer_view(status, factory);
    let diagnosis = diagnose(detail, status.health.conflicts);
    // A factory pull request's cancelled runs, started again: nothing for
    // an agent, and nothing more to watch until the list moves.
    if factory && status.is_open() && !diagnosis.cancelled.is_empty() && diagnosis.is_empty() {
        if !diagnosis.running {
            rerun_cancelled(app, url, &watch.dir, detail, &diagnosis);
        }
        app.autofix.forget(url);
        return;
    }
    if diagnosis.is_empty() && status.is_open() && pr.is_some() && !status.conflicts_known {
        return;
    }
    let (Some(pr), true, false) = (pr, status.is_open(), diagnosis.is_empty()) else {
        app.autofix.forget(url);
        close_settled_cant_fix(app);
        return;
    };
    // Its conflicting files, asked as soon as it is seen to conflict, so
    // they are in by the time it is sent.
    if diagnosis.conflicts {
        want_conflicts(app, &watch.project, url, pr.number, &PrRefs::of(detail));
    }
    let settling = diagnosis.running
        && !diagnosis.conflicts_only()
        && watch.first_seen.elapsed() < SETTLE_LIMIT;
    if settling {
        if let Some(w) = app.autofix.watching.get_mut(url) {
            w.settling = true;
        }
        return;
    }
    if fixer_on_it(app, &watch.project, &pr) || app.autofix.is_waiting(url) {
        app.autofix.relook_later(url);
        return;
    }
    // Only a fresh fixer is a new session: it alone checks with the
    // branch's other live sessions, and waits out the factory's pause.
    // Whether it is one waits on the branch agent's context, read off the
    // loop: the body is read again shortly, by when it is in.
    let mut to = router::route(app, &watch.project, &pr.head, pr.number, url);
    if let router::Route::Measure(agent) = &to {
        router::measure(app, agent);
        to = router::route(app, &watch.project, &pr.head, pr.number, url);
    }
    if matches!(to, router::Route::Measure(_)) {
        if let Some(w) = app.autofix.watching.get_mut(url) {
            w.measuring = true;
        }
        return;
    }
    let fresh = matches!(to, router::Route::Fresh { .. });
    // A fresh fixer goes into the PR's checkout: not while someone orion
    // can't see is at work there ([`checkout`]).
    if fresh {
        let paths: Vec<PathBuf> = app
            .tree
            .worktrees
            .iter()
            .filter(|w| w.project_id == watch.project && w.branch == pr.head)
            .map(|w| w.path.clone())
            .collect();
        match checkout::verdict(app, url, &paths) {
            // Asked: decided again once the look lands.
            None => return,
            Some(Some(why)) => {
                if app.autofix.held.insert(url.to_string()) {
                    app.flash = Some(Flash::note(format!(
                        "PR fixer held #{}: its checkout is in use ({why})",
                        pr.number
                    )));
                    app.dirty = true;
                }
                app.autofix.relook_later(url);
                return;
            }
            Some(None) => {}
        }
    }
    // The branch's other live sessions, each with the conversation the
    // agent reads before it touches anything — or, for one whose
    // conversation can't be found, nothing sent yet.
    let mut peers = Vec::new();
    let mut unreadable = None;
    let live = match fresh {
        true => live_on_branch(app, &watch.project, &pr),
        false => Vec::new(),
    };
    let cfg = (!live.is_empty()).then(crate::config::Config::load);
    for agent in live {
        match transcript_of(agent, cfg.as_ref()) {
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
                "PR fixer held #{}: {name} is busy on its branch",
                pr.number
            )));
            app.dirty = true;
        }
        app.autofix.relook_later(url);
        return;
    }
    let record = app.autofix.ledger.get(url).cloned().unwrap_or_default();
    // Never the cap ([`super::uncapped`]): only the factory paused at its
    // account's limit keeps the breakage waiting, still watched.
    let on = app.automations.is_on(Automation::PrFixer);
    if fresh && on && record.attempts < AUTO_ATTEMPTS && super::paused(app) {
        app.autofix.relook_later(url);
        return;
    }
    app.autofix.watching.remove(url);
    app.autofix.held.remove(url);
    let mut form = AutofixForm::new(watch.project, pr, detail, status.health.conflicts);
    if record.handled.as_ref() == Some(&form.fingerprint()) {
        return;
    }
    if !on {
        return;
    }
    if record.attempts < AUTO_ATTEMPTS {
        form.peers = peers;
        bundle_cubic(app, url, detail, &mut form);
        send(app, form, out);
    } else {
        give_up(app, &form, record.attempts);
    }
    app.dirty = true;
}

/// [`AUTO_ATTEMPTS`] sends have not turned the PR green: say so — a flash,
/// and the desktop while the window is away; on a factory pull request,
/// the *Can't fix* card instead ([`cant_fix_card`]) — and leave this
/// breakage be until a push or a different failure.
fn give_up(app: &mut App, form: &AutofixForm, attempts: u8) {
    app.autofix.record(&form.pr.url).handled = Some(form.fingerprint());
    if is_factory(app, &form.project, &form.pr.head) {
        let card = cant_fix_card(app, form, attempts);
        crate::factory::cards::post(app, card);
        return;
    }
    let text = format!(
        "{} is still failing after {attempts} PR fixer tries: over to you",
        form.pr.label()
    );
    over_to_you(app, text);
}

/// Hand a pull request back to the owner, saying `text`: a flash, and
/// the desktop while the window is away.
fn over_to_you(app: &mut App, text: String) {
    if app.may_notify_desktop() {
        crate::event_loop::alerts::notify_text("PR fixer", &text);
    }
    app.flash = Some(Flash::failed(text));
}

/// Words for the agent of open pull request `url` — the owner's, said for
/// the agent in `text` — through the FIX ROUTER: a new turn for the
/// branch's own agent, held for it while busy, or a fresh fixer with a
/// handover. What `reason` says goes in the run record (`send back`).
/// False when `url` is on no project's open list. The verdict card's
/// Send back… comes through here.
pub(crate) fn send_words(
    app: &mut App,
    url: &str,
    text: String,
    reason: &str,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let Some((project, pr)) = open_anywhere(app, url) else {
        return false;
    };
    let plan = app.pr_detail.get(url).and_then(router::plan_comment);
    let work = Work::Words {
        project,
        pr: Box::new(pr),
        text,
        plan,
        reason: reason.to_string(),
    };
    router::deliver(app, work, out);
    true
}

/// Open pull request `url`'s row and its project, whichever project's
/// open list has it.
pub(crate) fn open_anywhere(app: &App, url: &str) -> Option<(ProjectId, OpenPr)> {
    app.open_prs.iter().find_map(|(project, open)| {
        let pr = open.list.iter().find(|pr| pr.url == url)?;
        Some((project.clone(), pr.clone()))
    })
}

/// The key a factory pull request's *Can't fix* card goes by.
fn cant_fix_key(url: &str) -> String {
    format!("{CARD_PREFIX}{url}{CANT_FIX}")
}

/// What every PR fixer card's key starts with: `factory::cards` hands
/// its presses to [`on_card`].
pub(crate) const CARD_PREFIX: &str = "fix:";
const CANT_FIX: &str = ":cantfix";

/// *Can't fix*: "CI still red after 3 fixes: <check>." with Open PR,
/// Reply… and Park.
fn cant_fix_card(app: &App, form: &AutofixForm, attempts: u8) -> crate::factory::cards::Card {
    use crate::factory::cards::{Button, Card, Recipient};
    let what = match form.diagnosis.failing.is_empty() {
        true => "merge conflicts".to_string(),
        false => form
            .diagnosis
            .failing
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    };
    let project = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == form.project)
        .map_or_else(String::new, |p| format!(" · {}", p.name));
    let mut buttons = vec![
        Button::link("Open PR", form.pr.url.clone()),
        Button::text("reply", "Reply…"),
        Button::tap("park", "Park"),
    ];
    // A factory ticket's pull request can be taken off orion from here too.
    let factory = app
        .automations
        .linear_builder
        .ledger
        .issues
        .values()
        .any(|e| e.pr_url.as_deref() == Some(form.pr.url.as_str()));
    if factory {
        buttons.extend(super::linear_builder::owner::not_for_the_factory_buttons());
    }
    Card {
        key: cant_fix_key(&form.pr.url),
        to: Recipient::Owner,
        context: format!("PR fixer{project}"),
        title: form.pr.label(),
        body: vec![
            format!("CI still red after {attempts} fixes: {what}."),
            format!("<{}|Open #{} on GitHub>", form.pr.url, form.pr.number),
        ],
        buttons,
        image: None,
    }
}

/// A press on a PR fixer card (`fix:` keys, from `factory::cards`): on
/// *Can't fix* (whose Open PR is a link, never handed back), Reply… sends the owner's words to
/// the branch's agent and starts the tries over; Park leaves the pull
/// request alone until it goes green or is sent by `⌘G`; **Not for
/// orion** (a factory ticket's) takes its ticket off and closes the draft.
pub(crate) fn on_card(app: &mut App, key: &str, action: &str, text: Option<String>) {
    let Some(url) = key
        .strip_prefix(CARD_PREFIX)
        .and_then(|rest| rest.strip_suffix(CANT_FIX))
        .map(str::to_string)
    else {
        tracing::warn!("pr fixer: a press on {key}, which it doesn't know");
        return;
    };
    if let Some(backlog) = super::linear_builder::owner::not_for_the_factory_choice(action) {
        app.autofix.record(&url).parked = true;
        app.autofix.forget(&url);
        super::linear_builder::owner::not_for_the_factory_pr(app, &url, "", key, backlog);
        crate::factory::cards::close_if_up(app, key, "Not for orion");
        return;
    }
    let target = open_anywhere(app, &url);
    match action {
        "park" => {
            app.autofix.record(&url).parked = true;
            app.autofix.forget(&url);
            crate::factory::cards::close(app, key, "Parked");
        }
        "reply" => {
            let words = text.unwrap_or_default();
            let (Some(number), false) = (target.map(|(_, pr)| pr.number), words.trim().is_empty())
            else {
                crate::factory::cards::close(app, key, "Nothing sent");
                return;
            };
            let record = app.autofix.record(&url);
            record.attempts = 0;
            record.parked = false;
            let text = format!(
                "The owner replied about pull request #{number} ({url}), whose checks are still failing after {AUTO_ATTEMPTS} fixes:\n\n{}\n\nDo it in this pull request: commit with the repo's commit skill, push, then stop: orion watches CI.",
                words.trim(),
            );
            let mut out = Vec::new();
            send_words(app, &url, text, "owner's reply", &mut out);
            app.outbox.append(&mut out);
            crate::factory::cards::close(app, key, "Sent to the branch's agent");
        }
        other => tracing::warn!("pr fixer: unknown action {other} on {key}"),
    }
}

/// `⌘G` — in the PULL REQUESTS MODAL, or on the pane's PULL REQUEST PAGE
/// (and a click on its `⌘G send to the PR fixer`): the form for the pull request
/// it shows, at once from a fresh body, else once a fresh one lands
/// ([`land_detail`]), so it names the checks failing now.
pub(crate) fn open_asked(app: &mut App) {
    let Some((project, url, number)) = shown(app) else {
        return;
    };
    let Some(pr) = still_open(app, &project, &url, number) else {
        return;
    };
    let fresh = app.pr_detail_fresh(&url);
    if let (true, Some(detail)) = (fresh, app.pr_detail.get(&url).cloned()) {
        open_for(app, project, pr, &detail);
        return;
    }
    let Some(dir) = repo_dir(app, &project) else {
        return;
    };
    app.autofix.pending_open = Some(url.clone());
    app.pr_detail_failed.remove(&url);
    app.pending_pr_detail = Some((
        crate::app::PendingPrDetail { url, number, dir },
        Instant::now(),
    ));
    app.flash = Some(Flash::working(format!("Reading #{number}'s checks…")));
}

/// The pull request `⌘G` is about, as `(project, url, number)`: the one
/// under the PULL REQUESTS MODAL's cursor while the modal is up, else the
/// one the pane's PULL REQUEST PAGE reads — nothing over any other modal.
fn shown(app: &App) -> Option<(ProjectId, String, u64)> {
    match &app.overlay {
        Some(Overlay::PullRequests(view)) => {
            let pr = crate::pr_modal::selected_pr(app)?;
            Some((view.project.clone(), pr.url, pr.number))
        }
        Some(_) => None,
        None => {
            let pr = app.previewed_pr()?;
            Some((app.selected_project()?.id.clone(), pr.url, pr.number))
        }
    }
}

/// `url`'s row on `project`'s open list — None, and the footer saying so,
/// for one that has merged or closed.
fn still_open(app: &mut App, project: &ProjectId, url: &str, number: u64) -> Option<OpenPr> {
    let status = app.prs.status_or_open(url);
    let row = open_pr(app, project, url)
        .filter(|_| status.is_open())
        .cloned();
    if row.is_none() {
        let gone = match status.standing {
            crate::pull_request::Standing::Merged => "has merged",
            _ => "is no longer open",
        };
        app.flash = Some(Flash::note(format!("#{number} {gone}")));
    }
    row
}

/// `⌘G`'s body landed: the form, if what asked for it still shows that
/// pull request — from the cached body when `gh` could not answer. Not for
/// one merged or closed since.
fn open_pending(app: &mut App, url: &str, detail: Option<&PrDetail>) {
    let Some((project, _, number)) = shown(app).filter(|(_, shown, _)| shown == url) else {
        return;
    };
    match detail.or_else(|| app.pr_detail.get(url)).cloned() {
        Some(detail) => {
            if let Some(pr) = still_open(app, &project, url, number) {
                app.flash = None;
                open_for(app, project, pr, &detail);
            }
        }
        None => {
            app.flash = Some(Flash::failed(format!("Couldn't read #{number}'s checks")));
        }
    }
}

/// The PR's row in its project's open list.
pub(crate) fn open_pr<'a>(app: &'a App, project: &ProjectId, url: &str) -> Option<&'a OpenPr> {
    app.open_prs
        .get(project)?
        .list
        .iter()
        .find(|pr| pr.url == url)
}

/// The session name an autofix agent on PR `number` goes by.
pub fn agent_name(number: u64) -> String {
    format!("autofix-{number}")
}

/// Whether `agent` works in one of `project`'s checkouts of `branch`.
pub(crate) fn on_branch(
    app: &App,
    agent: &orion_core::Agent,
    project: &ProjectId,
    branch: &str,
) -> bool {
    app.tree
        .worktrees
        .iter()
        .any(|w| w.id == agent.worktree_id && &w.project_id == project && w.branch == branch)
}

/// Whether `agent` works on `branch` in `project`: one of the project's
/// checkouts of the branch is its own ([`on_branch`]), or it wrote in one
/// lately ([`visiting`]) — a session started in another project that
/// `cd`s into the checkout to commit, which orion would otherwise not
/// count as on the branch at all.
pub(crate) fn works_on(
    app: &App,
    agent: &orion_core::Agent,
    project: &ProjectId,
    branch: &str,
) -> bool {
    on_branch(app, agent, project, branch) || visiting(app, agent, project, branch).is_some()
}

/// The checkout of `branch` in `project` that `agent` — not one of its
/// own — wrote in within [`orion_core::touched_paths::VISIT_TTL_MS`], as
/// the daemon's visit book has it (`App::visits`).
pub(crate) fn visiting<'a>(
    app: &'a App,
    agent: &orion_core::Agent,
    project: &ProjectId,
    branch: &str,
) -> Option<&'a orion_core::Worktree> {
    let now = orion_core::clock::now_ms();
    app.visits
        .iter()
        .filter(|v| v.agent == agent.id && now - v.at < orion_core::touched_paths::VISIT_TTL_MS)
        .find_map(|v| {
            app.tree.worktrees.iter().find(|w| {
                w.id == v.worktree
                    && w.id != agent.worktree_id
                    && &w.project_id == project
                    && w.branch == branch
            })
        })
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
    fixer(app, project, branch, number).is_some()
}

/// The `autofix-<number>` [`fixing`] finds: what the PULL REQUEST PAGE's
/// AUTOFIX line shows at work (`pr_work`).
pub(crate) fn fixer<'a>(
    app: &'a App,
    project: &ProjectId,
    branch: &str,
    number: u64,
) -> Option<&'a orion_core::Agent> {
    let name = agent_name(number);
    app.tree.agents.iter().find(|agent| {
        !agent.archived
            && agent.name == name
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
                && works_on(app, agent, project, &pr.head)
        })
        .collect()
}

/// Where Claude Code keeps `agent`'s conversation: its account's config
/// dir, `projects/<cwd slug>/<session id>.jsonl` — looked for under every
/// slug, as `orion-daemon`'s CONTINUE ON does. None off a Claude account,
/// before the session has an id, or with no such file. Under `cfg`, else
/// the config loaded here. Disk work: off the UI thread where it can be.
pub(crate) fn transcript_of(
    agent: &orion_core::Agent,
    cfg: Option<&crate::config::Config>,
) -> Option<PathBuf> {
    let session = agent.session_id.as_deref()?;
    let id = orion_core::harness::registry_id(agent.kind, agent.custom_harness.as_deref());
    if id.is_empty() {
        return None;
    }
    let entry = match cfg {
        Some(cfg) => cfg.effective_harness_by_id(id),
        None => crate::config::Config::load().effective_harness_by_id(id),
    };
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
            !a.archived && !a.name.starts_with("autofix-") && works_on(app, a, project, &pr.head)
        })
        .max_by_key(|a| crate::app::last_interaction_ms(a))
}

/// The name `orion autofix stand-down` gives the session it archives,
/// ahead of its reason: what [`note_stand_downs`] knows it by.
pub const STOOD_DOWN: &str = "autofix · stood down: ";

// ---- the automation ----

/// The PR fixer's [`Runner`]. Its state is `App::autofix`, which the
/// open-PR lists and PR bodies drive as they land ([`note_list`],
/// [`land_detail`]); the tick sends what waited on its conflicting files
/// and flashes the agents that stood down.
pub struct PrFixer;

impl Runner for PrFixer {
    fn automation(&self) -> Automation {
        Automation::PrFixer
    }

    fn tick(&mut self, app: &mut App, out: &mut Vec<ClientRequest>) {
        send_waiting(app, out);
        retry_cubic(app, out);
        relook_held(app, out);
        router::deliver_queued(app, out);
        archive_green(app, out);
        note_stand_downs(app);
    }

    fn panel(&self, app: &App) -> Panel {
        let mut open = Vec::new();
        for project in &app.tree.projects {
            let Some(prs) = app.open_prs.get(&project.id) else {
                continue;
            };
            for pr in prs.list.iter().filter(|pr| pr.mine) {
                open.push(open_pr_row(app, &project.id, pr));
            }
        }
        let fixing: Vec<&PanelRow> = open.iter().filter(|r| r.session.is_some()).collect();
        let stuck = open.iter().filter(|r| r.tone == Tone::Waiting).count();
        let (dot, state) = match (fixing.first(), stuck) {
            (Some(row), _) => (
                Dot::Working,
                format!("fixing {}", row.text.split(' ').next().unwrap_or_default()),
            ),
            (None, n) if n > 0 => (Dot::Waiting, format!("{n} needs you")),
            (None, _) => (Dot::Idle, "watching your PRs".into()),
        };
        Panel {
            dot,
            state,
            lists: vec![
                PanelList {
                    title: "YOUR OPEN PRS",
                    empty: "No open PRs of yours",
                    rows: open,
                },
                super::past_sessions(app, Automation::PrFixer),
            ],
        }
    }
}

/// One of your open pull requests on the PR fixer's panel: `#n title`,
/// and where the fixer stands on it.
fn open_pr_row(app: &App, project: &ProjectId, pr: &OpenPr) -> PanelRow {
    let record = app.autofix.ledger.get(&pr.url);
    let attempts = record.map_or(0, |r| r.attempts);
    let factory = is_factory(app, project, &pr.head);
    let status = fixer_view(app.prs.status_or_open(&pr.url), factory);
    let queued = app
        .autofix
        .queued
        .iter()
        .find(|q| q.work.pr().url == pr.url)
        .and_then(|q| match &q.agent {
            Some(id) => app
                .tree
                .agents
                .iter()
                .find(|a| &a.id == id)
                .map(|a| a.name.clone()),
            // Held for the fresh fixer on its way.
            None => Some(agent_name(pr.number)),
        });
    let (state, tone, session) = match (fixer(app, project, &pr.head, pr.number), queued) {
        (Some(agent), _) if agent.status == AgentStatus::NeedsFeedback => (
            "fixing, waiting on you".to_string(),
            Tone::Waiting,
            Some(agent.id.clone()),
        ),
        (Some(agent), _) => (
            format!(
                "fixing {}, try {} of {AUTO_ATTEMPTS}",
                match fixer_trouble(&status) {
                    Some(crate::pull_request::Trouble::Conflicts) => "conflicts",
                    Some(crate::pull_request::Trouble::FailingChecks) => "checks",
                    None => "it",
                },
                attempts.max(1)
            ),
            Tone::Working,
            Some(agent.id.clone()),
        ),
        (None, _) if status.is_draft() && !factory => {
            ("draft, left alone".into(), Tone::Quiet, None)
        }
        (None, _) if status.is_green() => ("green".into(), Tone::Done, None),
        // Held for a busy branch agent: not at work on it yet.
        (None, Some(agent)) => (format!("queued for {agent}"), Tone::Quiet, None),
        (None, None) if record.is_some_and(|r| r.parked) => ("parked".into(), Tone::Quiet, None),
        (None, None) if attempts >= AUTO_ATTEMPTS => {
            ("gave up, needs you".into(), Tone::Waiting, None)
        }
        (None, None) if fixer_trouble(&status).is_some() => {
            ("waiting for checks".into(), Tone::Quiet, None)
        }
        (None, None) if status.health.checks == Checks::Cancelled => {
            let rerun = record.and_then(|r| r.rerun.as_deref()) == Some(status.head_sha.as_str());
            let word = if rerun {
                "re-ran cancelled checks"
            } else {
                "checks cancelled"
            };
            (word.into(), Tone::Quiet, None)
        }
        (None, None) => ("watching".into(), Tone::Quiet, None),
    };
    PanelRow {
        text: format!("#{} {}", pr.number, pr.title),
        state,
        tone,
        session,
        url: Some(pr.url.clone()),
    }
}

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
            app.flash = Some(Flash::note(format!("PR fixer stood down: {why}")));
            app.dirty = true;
        }
    }
}

/// The beat: archive each `autofix-<n>` whose pull request has gone
/// green with its turn done — its push in, nothing more for it to do.
/// One still at work, or waiting on the owner, is left be; so is one the
/// owner is using ([`in_use`]).
pub fn archive_green(app: &mut App, out: &mut Vec<ClientRequest>) {
    let done: Vec<orion_core::AgentId> = app
        .tree
        .agents
        .iter()
        .filter(|a| !a.archived && !router::busy(a.status) && !in_use(app, &a.id))
        .filter(|a| {
            let Some(number) = a
                .name
                .strip_prefix("autofix-")
                .and_then(|n| n.parse::<u64>().ok())
            else {
                return false;
            };
            let Some(wt) = app.tree.worktrees.iter().find(|w| w.id == a.worktree_id) else {
                return false;
            };
            app.open_prs.get(&wt.project_id).is_some_and(|open| {
                open.list.iter().any(|pr| {
                    pr.number == number
                        && pr.head == wt.branch
                        && fixer_view(
                            app.prs.status_or_open(&pr.url),
                            is_factory(app, &wt.project_id, &wt.branch),
                        )
                        .is_green()
                })
            })
        })
        .map(|a| a.id.clone())
        .collect();
    for id in done {
        crate::event_loop::archive_agent_now(app, id, out);
    }
}

/// How long after the owner last typed into a session it counts as in
/// use ([`in_use`]).
pub const IN_USE_FOR: Duration = Duration::from_secs(15 * 60);

/// Whether the owner is using session `id`: the pane shows it, or is
/// about to (attached, or its attach waiting out its debounce), its
/// follow-up composer is open, or they typed into it within
/// [`IN_USE_FOR`].
fn in_use(app: &App, id: &orion_core::AgentId) -> bool {
    let this = |sref: &SessionRef| matches!(sref, SessionRef::Agent(a) if a == id);
    app.term.as_ref().is_some_and(|t| this(&t.sref))
        || app.attached_sref.as_ref().is_some_and(this)
        || app
            .pending_attach
            .as_ref()
            .is_some_and(|(sref, _)| this(sref))
        || app.follow_up.as_ref().is_some_and(|f| &f.agent == id)
        || app
            .typed_at
            .get(id)
            .is_some_and(|at| at.elapsed() < IN_USE_FOR)
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

/// orion's own text for the PR autofix SYSTEM PRESET: the rules, then —
/// at `{task}`, what broke — how to finish. What the agent is told while
/// the preset holds none of the user's
/// ([`crate::agent_presets::SystemPreset::builtin_text`]).
pub const PRESET_TEXT: &str = "You are orion's PR autofixer. Fix the pull request below so it can merge, and push the fix to its branch. Do not open a new pull request, force-push, merge it, or change unrelated code. Never use `--no-verify`.

Rules:
- Fix only what is listed under \"Fix\" below.
- Merge conflicts: fetch the base branch from origin and merge it. Resolve each conflict by understanding both sides' intent; never drop one side's work silently. Regenerate lockfiles and generated files rather than hand-merging them, and commit the merge.
- Failing checks: read each one's failure logs (`gh pr checks`, then `gh run view <run-id> --log-failed`) to see what kind of check it is.
  - Unit tests: reproduce each failing test locally with this repo's test command, and fix it. Change a test only when this pull request meant to change the behaviour it checks.
  - E2E tests: work from the failing specs' CI logs. No local e2e unless the logs aren't enough; then run only the failing specs, at most twice, as the testing rule below says.
  - Any other check (lint, typecheck, build): reproduce it locally and fix it.
- Cubic (the review bot): judge each open finding against what this pull request means to do. Fix a P0 or P1 unless it is wrong; never redo something that was left out on purpose. Dismiss a finding only with a reason posted on the pull request: a reply on its thread, and a `Cubic dismissals: <finding>: <why>` line in a comment. If the same P1 comes back a third time, stop and say so in your summary.
- A review comment from the owner is done in this pull request, never in a new one.
- End every comment or reply you post on the pull request with the line `<!-- orion:agent -->`, so orion can tell your words from the owner's.

{task}

Then commit with the repo's commit skill and push, then stop: orion watches CI and sends you back if a check fails again. Finish with a short summary: what you changed, and anything you could not fix and why.";

/// The agent's first prompt: the pull request and the ticked issues —
/// the failed checks by workflow — and the user's note, at the PR autofix
/// preset's `{task}` — orion's own text ([`PRESET_TEXT`]) while it holds
/// none of the user's.
#[allow(clippy::too_many_arguments)]
pub fn prompt(
    pr: &OpenPr,
    refs: &PrRefs,
    diagnosis: &Diagnosis,
    picks: &Picks,
    note: &str,
    preset: &AgentPreset,
    rule: &str,
    extra: &str,
) -> String {
    super::compose_with(preset, &task(pr, refs, diagnosis, picks, note), rule, extra)
}

/// What fills the preset's `{task}` — and a breakage's turn
/// ([`router::turn_text`]): [`context`] with the ticked issues.
fn task(pr: &OpenPr, refs: &PrRefs, diagnosis: &Diagnosis, picks: &Picks, note: &str) -> String {
    let base = match refs.base.as_str() {
        "" => "the base branch".to_string(),
        name => format!("`{name}`"),
    };
    let ticked: Vec<Issue> = Issue::ALL
        .into_iter()
        .filter(|i| picks[i.index()])
        .collect();
    context(pr, refs, diagnosis, &ticked, &base, note)
}

/// What the agent is pointed at: the pull request, its branches, each
/// ticked issue — the failing checks under their workflows — and the
/// user's note.
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
        let heading = issue.label();
        match issue {
            Issue::Conflicts if diagnosis.conflicted.is_empty() => {
                lines.push(format!("- {heading} with {base}"))
            }
            Issue::Conflicts => {
                lines.push(format!("- {heading} with {base}, in:"));
                lines.extend(diagnosis.conflicted.iter().map(|f| format!("  - {f}")));
            }
            Issue::Checks if diagnosis.failing.is_empty() => lines.push(format!(
                "- {heading} (none named by GitHub — check `gh pr checks {}`)",
                pr.number
            )),
            Issue::Checks => {
                lines.push(format!("- {heading}:"));
                for (workflow, checks) in diagnosis.failing_by_workflow() {
                    lines.push(format!("  - {workflow}:"));
                    lines.extend(checks.iter().map(|c| format!("    - {}", check_line(c))));
                }
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

/// What the PR autofix preset's `{task}` is filled with in the PRESET
/// EDITOR's preview: [`context`]'s own words for a made-up pull request
/// with one failing check.
pub(crate) fn sample_task() -> String {
    let pr = OpenPr {
        number: 42,
        title: "Fix the login redirect".into(),
        url: "https://github.com/you/app/pull/42".into(),
        answered_draft: false,
        answered: Default::default(),
        head: "fix-login".into(),
        mine: true,
        head_sha: String::new(),
        meta: Default::default(),
    };
    let refs = PrRefs {
        base: "main".into(),
        head: "fix-login".into(),
        head_sha: String::new(),
    };
    let diagnosis = Diagnosis {
        failing: vec![PrCheck {
            name: "unit-tests".into(),
            workflow: "CI".into(),
            state: CheckState::Failed,
            word: String::new(),
            started: String::new(),
            completed: String::new(),
            url: String::new(),
        }],
        ..Diagnosis::default()
    };
    context(&pr, &refs, &diagnosis, &[Issue::Checks], "`main`", "")
}

/// `name url` — its workflow is the line over it — leaving out a page
/// GitHub did not give.
fn check_line(check: &PrCheck) -> String {
    let mut line = check.name.clone();
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
    const ORDER: [Row; 3] = [
        Row::Issue(Issue::Conflicts),
        Row::Issue(Issue::Checks),
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
    /// The plan as approved, off the pull request's comments: a fresh
    /// fixer's handover ([`router::handover`]).
    pub plan: Option<String>,
    /// Cubic's findings sent with the breakage, as the agent reads them
    /// ([`bundle_cubic`]); empty for none.
    pub cubic: String,
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
            plan: router::plan_comment(detail),
            cubic: String::new(),
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
    if conflicts {
        want_conflicts(app, &form.project, &form.pr.url, form.pr.number, &form.refs);
        form.diagnosis.conflicted = files_for(app, &form).unwrap_or_default();
    }
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

/// Ask the repo which files pull request `number` at `url` conflicts in —
/// its head merged with its base in memory (`conflicts::local`), the head
/// fetched by its pull ref first when the repo lacks it — off the loop,
/// unless its head and base were asked already.
fn want_conflicts(app: &mut App, project: &ProjectId, url: &str, number: u64, refs: &PrRefs) {
    let (head_sha, base) = (refs.head_sha.clone(), refs.base.clone());
    let asked = app
        .autofix
        .conflicted
        .get(url)
        .is_some_and(|known| known.is_for(&head_sha, &base));
    if asked || head_sha.is_empty() || base.is_empty() {
        return;
    }
    let Some(dir) = repo_dir(app, project) else {
        return;
    };
    app.autofix.conflicted.insert(
        url.to_string(),
        Conflicted {
            head_sha: head_sha.clone(),
            base: base.clone(),
            files: None,
        },
    );
    let (url, quit) = (url.to_string(), app.git_sync.quit.clone());
    crate::conflicts::off_loop(app, move || {
        let merged = crate::conflicts::local(&dir, &base, &head_sha).or_else(|_| {
            crate::conflicts::fetch_pull_head(&dir, number, &head_sha, &quit)?;
            crate::conflicts::local(&dir, &base, &head_sha)
        });
        crate::conflicts::Answer::Autofix {
            url,
            files: merged.map(|local| local.files),
            head_sha,
            base,
        }
    });
}

/// [`want_conflicts`]'s answer: the files kept for the head and base it
/// was asked for — and shown on the form up on that pull request.
pub(crate) fn land_conflicts(
    app: &mut App,
    url: &str,
    head_sha: &str,
    base: &str,
    files: Result<Vec<String>, String>,
) {
    let Some(known) = app.autofix.conflicted.get_mut(url) else {
        return;
    };
    if !known.is_for(head_sha, base) {
        return;
    }
    let files = files.unwrap_or_else(|why| {
        tracing::debug!(url, "autofix: no conflicting files: {why}");
        Vec::new()
    });
    known.files = Some(files.clone());
    if let Some(Overlay::Autofix(form)) = &mut app.overlay {
        if form.pr.url == url && form.refs.head_sha == head_sha && form.refs.base == base {
            form.diagnosis.conflicted = files;
        }
    }
}

/// The files `form`'s pull request conflicts in for its head and base, as
/// [`want_conflicts`] was answered — none when its head was never asked
/// about — or None while that answer is out.
fn files_for(app: &App, form: &AutofixForm) -> Option<Vec<String>> {
    match app.autofix.conflicted.get(&form.pr.url) {
        Some(known) if known.is_for(&form.refs.head_sha, &form.refs.base) => known.files.clone(),
        _ => Some(Vec::new()),
    }
}

/// Send `form`'s agent ([`dispatch`]) — naming the files its pull request
/// conflicts in, when it conflicts: while the repo has yet to say which,
/// it waits, and goes on the beat once it has ([`send_waiting`]).
fn send(app: &mut App, mut form: AutofixForm, out: &mut Vec<ClientRequest>) {
    if form.diagnosis.conflicts {
        want_conflicts(app, &form.project, &form.pr.url, form.pr.number, &form.refs);
        let Some(files) = files_for(app, &form) else {
            app.autofix.waiting.retain(|w| w.pr.url != form.pr.url);
            app.autofix.waiting.push(form);
            return;
        };
        form.diagnosis.conflicted = files;
    }
    dispatch(app, form, out);
}

/// The beat: each send [`send`] left waiting whose conflicting files are
/// in, sent.
pub fn send_waiting(app: &mut App, out: &mut Vec<ClientRequest>) {
    for mut form in std::mem::take(&mut app.autofix.waiting) {
        match files_for(app, &form) {
            Some(files) => {
                form.diagnosis.conflicted = files;
                dispatch(app, form, out);
            }
            None => app.autofix.waiting.push(form),
        }
    }
}

/// Send the breakage: wherever the FIX ROUTER says — a new turn for the
/// branch's own agent, held for it while it is busy, or a fresh
/// `autofix-<n>` ([`launch_fresh`]). The ledger counts it once it goes,
/// and the PR stops being watched.
pub fn dispatch(app: &mut App, form: AutofixForm, out: &mut Vec<ClientRequest>) {
    router::deliver(app, Work::Fix(Box::new(form)), out);
}

/// What a breakage's turn is pointed at: [`task`] for `form`.
fn task_of(form: &AutofixForm) -> String {
    let task = task(
        &form.pr,
        &form.refs,
        &form.diagnosis,
        &form.picks,
        form.note.as_str(),
    );
    match form.cubic.as_str() {
        "" => task,
        cubic => format!("{task}\n\nIn the same push:\n\n{cubic}"),
    }
}

/// Launch a fresh fixer for `work`: a PR SESSION named `autofix-<n>`, on
/// [`launch_of`]'s harness, account, model and effort — told [`prompt`]
/// for a breakage, under [`stand_down_preamble`] while other sessions are
/// live there, or the owner's words under the preset for words — then
/// `handover`. False when the project has no checkout to start from.
fn launch_fresh(
    app: &mut App,
    work: &Work,
    handover: &str,
    cfg: &crate::config::Config,
    out: &mut Vec<ClientRequest>,
) -> bool {
    let (project, pr) = (work.project().clone(), work.pr().clone());
    let Some(worktree) = app.root_worktree(&project) else {
        app.flash = Some(Flash::failed(
            "PR fixer: the project has no checkout to start from",
        ));
        return false;
    };
    // The PR fixer's preset — the PR autofix SYSTEM PRESET unless `⌘U`
    // picked another: its text, and what the agent runs on.
    let preset = super::preset_of(&cfg.automations, Automation::PrFixer);
    let (kind, custom, model, effort) = launch_of(app, &project, &pr, &preset, cfg);
    let rule = cfg.automations.local_e2e.rule();
    let extra = &cfg.automations.of(Automation::PrFixer).extra_steer;
    let (preamble, task) = match work {
        Work::Fix(form) => (
            stand_down_preamble(&form.peers, &form.pr),
            super::compose_with(&preset, &task_of(form), rule, extra),
        ),
        Work::Words { text, .. } => (
            String::new(),
            super::compose_with(
                &preset,
                &format!(
                    "Pull request: {} (#{} \"{}\")\nThis worktree is checked out on `{}`.\n\nFix:\n- What is asked below, in this pull request.\n\n{text}",
                    pr.url, pr.number, pr.title, pr.head
                ),
                rule,
                extra,
            ),
        ),
    };
    // A preset of the user's may not say how to sign a PR comment.
    let marker = match task.contains(router::AGENT_MARKER) {
        true => String::new(),
        false => router::MARKER_RULE.to_string(),
    };
    let text = [preamble, task, marker, handover.to_string()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let draft = crate::app::AgentLaunchDraft {
        custom,
        name: agent_name(pr.number),
        starting_prompt: Some(text),
        pr: Some(PrLaunch::of(&pr)),
        focus_pane: false,
        follow: false,
        automation: Some(Automation::PrFixer),
        ..crate::app::AgentLaunchDraft::new(worktree, kind, model, effort)
    };
    crate::event_loop::create_agent(app, draft, out);
    true
}

/// What a fresh fixer on `pr` runs on, by the PR autofix `preset`'s Runs on row:
/// following — the default — the harness, account, model and effort of
/// the session that did the work on the branch ([`source_agent`]) — on the
/// factory account in place of its Claude account while one is chosen
/// ([`super::account::onto`]) — and the preset's own with none; a harness
/// the preset pins, while it is on, at
/// the preset's model and effort ([`crate::config::Config::autofix_launch`]).
/// `(kind, custom, model, effort)`.
fn launch_of(
    app: &App,
    project: &ProjectId,
    pr: &OpenPr,
    preset: &AgentPreset,
    cfg: &crate::config::Config,
) -> (AgentKind, Option<String>, Option<String>, Option<String>) {
    match source_agent(app, project, pr).filter(|_| !cfg.autofix_pins(preset)) {
        Some(a) => super::account::onto(
            cfg,
            (
                a.kind,
                a.custom_harness.clone(),
                a.model.clone(),
                a.effort.clone(),
            ),
        ),
        None => cfg.autofix_launch(preset),
    }
}

pub mod keys {
    use crate::hints::Key;

    pub const OPTION: Key = Key::new(&["up", "down"], "option").show(2);
    pub const TICK: Key = Key::new(&["space"], "tick");
    pub const SEND: Key = Key::new(&["enter"], "send");
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
    // Esc: not now. `⌘G` asked for the form, so there is nothing to
    // remember: an unasked send is still the breakage's to make.
    if crate::nav::closes(&key) {
        close(app);
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
        // Asked for by hand: a parked pull request is back in play.
        if let Some(record) = app.autofix.ledger.get_mut(&form.pr.url) {
            record.parked = false;
        }
        send(app, form, out);
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
const EXPLAIN: &str = "Goes to the branch's own agent as a new turn when it's idle with room, else to a fresh agent in the PR's worktree: it fixes what is ticked, then commits and pushes to the PR.";
const EXPLAIN_ROWS: u16 = 3;

/// What each issue row says after its box; the failing checks' names
/// follow on lines of their own, a workflow each ([`workflow_rows`]).
fn row_note(form: &AutofixForm, issue: Issue) -> String {
    match issue {
        _ if !form.diagnosis.has(issue) => "none detected".into(),
        Issue::Conflicts if form.diagnosis.conflicted.is_empty() => format!(
            "{} has moved on — merge it in and resolve",
            form.refs.base_or(SOME_BASE)
        ),
        Issue::Conflicts => format!(
            "{}: {}",
            crate::flash::plural(form.diagnosis.conflicted.len(), "file"),
            form.diagnosis.conflicted.join(", ")
        ),
        Issue::Checks => format!("{} failing", form.diagnosis.failing.len()),
    }
}

/// Under the **Failing checks** row, a line per workflow: its name in the
/// labels' column, then the checks that failed in it.
fn workflow_rows(form: &AutofixForm, th: Theme) -> Vec<Vec<Span<'static>>> {
    // Past the caret and the box, under the label.
    const UNDER_LABEL: &str = "       ";
    form.diagnosis
        .failing_by_workflow()
        .into_iter()
        .map(|(workflow, checks)| {
            let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
            let workflow = crate::ui::truncate(&workflow, LABEL_W - 1);
            vec![
                Span::raw(UNDER_LABEL),
                Span::styled(
                    format!("{workflow:<LABEL_W$}"),
                    Style::default().fg(th.muted),
                ),
                Span::styled(names.join(", "), Style::default().fg(th.warn)),
            ]
        })
        .collect()
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
    for issue in Issue::ALL {
        lines.push((Some(Row::Issue(issue)), issue_row(form, issue, th)));
        if issue == Issue::Checks {
            lines.extend(workflow_rows(form, th).into_iter().map(|row| (None, row)));
        }
    }
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
        crate::ui::modal_block(format!(" PR fixer #{} ", form.pr.number), th),
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
        assert_eq!(d.failing, [check("lint", "", CheckState::Failed)]);
        assert!(d.running);
        assert_eq!(d.summary(), "merge conflicts · 1 failing check");
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
    /// to finish: fix only what is listed, work e2e failures from their
    /// logs, Cubic's P1s, the owner's comments in this PR, then commit with
    /// the repo's skill, push and stop — orion watches CI.
    #[test]
    fn the_prompt_says_only_what_is_ticked_under_orions_rules() {
        let detail = detail(true, vec![check("e2e", "", CheckState::Failed)]);
        let text = prompt(
            &pr(),
            &PrRefs::of(&detail),
            &diag(&detail),
            &[false, true],
            "flaky on CI",
            &orions(),
            "",
            "",
        );
        assert!(text.starts_with("You are orion's PR autofixer."), "{text}");
        assert!(text.contains("https://github.com/o/r/pull/7"));
        assert!(text.contains("`feat` into `main`"));
        assert!(text.contains("https://ci/e2e"));
        assert!(text.contains("Note from the user: flaky on CI"));
        let fix = &text[text.find("\nFix:\n").unwrap()..text.find("Note from the user").unwrap()];
        assert!(fix.contains("- Failing checks:"), "{fix}");
        assert!(!fix.contains("- Merge conflicts"), "{fix}");
        for rule in [
            "Fix only what is listed",
            "fetch the base branch from origin and merge it",
            "`gh run view <run-id> --log-failed`",
            "No local e2e unless the logs aren't enough",
            "reproduce each failing test locally",
            "Any other check (lint, typecheck, build): reproduce it locally and fix it",
            "Dismiss a finding only with a reason posted on the pull request",
            "never redo something that was left out on purpose",
            "If the same P1 comes back a third time, stop",
            "A review comment from the owner is done in this pull request",
            "Never use `--no-verify`",
            "commit with the repo's commit skill and push, then stop: orion watches CI",
        ] {
            assert!(text.contains(rule), "{rule}:\n{text}");
        }
        assert!(!text.contains("--watch"), "orion watches CI, not the agent");
        let at = |needle: &str| text.find(needle).unwrap();
        assert!(at("Rules:") < at("Pull request:"), "the rules lead");
        assert!(
            at("Note from the user") < at("orion watches CI and"),
            "finishing ends it"
        );
    }

    #[test]
    fn a_preset_replaces_orions_text_but_keeps_the_context() {
        let detail = detail(true, Vec::new());
        let preset = AgentPreset {
            text: "Do it my way.\n\n{task}\n\nThen push.".into(),
            ..orions()
        };
        let text = prompt(
            &pr(),
            &PrRefs::of(&detail),
            &diag(&detail),
            &[true, false],
            "",
            &preset,
            "",
            "",
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
        assert_eq!(form.picks, [false, true]);
        assert_eq!(form.row, Row::Issue(Issue::Checks));
    }

    /// Unit, e2e and the rest are one **Failing checks** list, grouped by
    /// the workflow each ran in as GitHub groups them, a check from none
    /// under `Other`: nothing is guessed from a check's name.
    #[test]
    fn failing_checks_are_one_list_grouped_by_workflow() {
        let detail = detail(
            false,
            vec![
                check("unit tests", "CI", CheckState::Failed),
                check("chromium", "Playwright", CheckState::Failed),
                check("lint", "CI", CheckState::Failed),
                check("build", "CI", CheckState::Passed),
                check("vercel", "", CheckState::Failed),
            ],
        );
        let form = AutofixForm::new(ProjectId("p".into()), pr(), &detail, false);
        assert_eq!(form.diagnosis.summary(), "4 failing checks");
        let text = prompt(
            &pr(),
            &form.refs,
            &form.diagnosis,
            &form.picks,
            "",
            &orions(),
            "",
            "",
        );
        let fix = &text[text.find("\nFix:\n").unwrap()..text.find("Then commit").unwrap()];
        assert_eq!(
            fix.trim(),
            [
                "Fix:",
                "- Failing checks:",
                "  - CI:",
                "    - unit tests https://ci/unit tests",
                "    - lint https://ci/lint",
                "  - Playwright:",
                "    - chromium https://ci/chromium",
                "  - Other:",
                "    - vercel https://ci/vercel",
            ]
            .join("\n")
        );
    }

    // ---- detection, on an App ----

    fn app_with(list: Vec<OpenPr>, on: bool) -> App {
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
            },
        );
        app.automations.settings.of_mut(Automation::PrFixer).on = Some(on);
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

    /// The bodies the fixer wants read now, and how often (THE
    /// RESOLVER's background demand).
    fn needs(app: &App) -> Vec<(String, Duration)> {
        let mut needs = detail_needs(app);
        needs.sort();
        needs
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
            automation: None,
        });
        id
    }

    /// The PR autofix preset's Runs on row: following, the agent runs on
    /// the session that did the work on the branch — Codex here — and on
    /// the default agent with none; a harness it pins wins over both while
    /// it is on, and while it is off the following rule comes back.
    #[test]
    fn the_autofix_agent_runs_on_the_prs_own_agent_unless_the_preset_pins_one() {
        let mut app = App::new();
        session_on_branch(&mut app, "did-the-work", AgentStatus::Finished);
        let detail = detail(true, Vec::new());
        let form = AutofixForm::new(ProjectId("p".into()), pr(), &detail, false);
        let mut cfg = crate::config::Config::default();
        let follows = orions();
        assert_eq!(
            launch_of(&app, &form.project, &form.pr, &follows, &cfg).0,
            AgentKind::Codex
        );
        assert_eq!(
            launch_of(&App::new(), &form.project, &form.pr, &follows, &cfg).0,
            cfg.quick_prompt_harness().0,
            "no session on the branch: the default agent"
        );
        let pinned = AgentPreset {
            kind: AgentKind::Cursor,
            follow_box: Some(false),
            ..orions()
        };
        assert_eq!(
            launch_of(&app, &form.project, &form.pr, &pinned, &cfg).0,
            AgentKind::Cursor
        );
        cfg.cursor_enabled = false;
        assert_eq!(
            launch_of(&app, &form.project, &form.pr, &pinned, &cfg).0,
            AgentKind::Codex,
            "a pinned harness switched off: the PR's own agent again"
        );
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
        let mut app = app_with(vec![theirs], true);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "not mine");

        let mut app = app_with(vec![broken(true, false)], false);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "off");

        let mut app = app_with(vec![broken(true, false)], true);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));
        assert_eq!(
            needs(&app),
            vec![(pr().url, crate::sync::github::ON_CHANGE)],
            "its body is asked for at once, and on no clock"
        );
        listed(&mut app);
        assert_eq!(app.autofix.want.len(), 1, "and only once");
    }

    #[test]
    fn running_checks_are_waited_out_but_conflicts_alone_are_not() {
        let mut app = app_with(vec![broken(false, true)], true);
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
        assert!(app.autofix.watching[&pr().url].settling);
        assert_eq!(
            needs(&app),
            vec![(pr().url, SETTLE_RECHECK)],
            "asked again on the settle beat"
        );

        let mut app = app_with(vec![broken(true, false)], true);
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
        let mut app = app_with(vec![broken(true, false)], true);
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
        let mut app = app_with(vec![broken(true, false)], true);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                handled: handled("abc123", true),
                attempts: 0,
                ..Record::default()
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
        let mut app = app_with(vec![broken(true, false)], true);
        session_on_branch(&mut app, "autofix-7", AgentStatus::Fresh);
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(out.is_empty(), "{out:?}");
        assert!(
            app.autofix.watching.contains_key(&pr().url),
            "still watched"
        );
    }

    /// The fixer's beat — queued work routed, green fixers archived — as
    /// the automations tick runs it.
    fn beat(app: &mut App) -> Vec<ClientRequest> {
        let mut out = Vec::new();
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("agent_presets.json"), || {
            crate::config::with_config_path(dir.path().join("config.json"), || {
                PrFixer.tick(app, &mut out)
            })
        });
        out
    }

    /// The turn `out` sends, as `(agent, text)`, if one goes.
    fn sent_turn(out: &[ClientRequest]) -> Option<(orion_core::AgentId, String)> {
        out.iter().find_map(|req| match req {
            ClientRequest::SendTurn { id, text, .. } => Some((id.clone(), text.clone())),
            _ => None,
        })
    }

    /// A Claude session `name` on the PR's branch whose transcript ends on
    /// `said` with `used` tokens of context in play, and the places that
    /// find it: run [`landed`] and [`beat`] under them.
    fn claude_on_branch(
        app: &mut App,
        home: &tempfile::TempDir,
        name: &str,
        status: AgentStatus,
        said: &[&str],
        used: u64,
    ) -> (orion_core::AgentId, crate::paths_util::Places) {
        let claude = home.path().join(".claude");
        let slug = claude.join("projects").join("-tmp-repo-worktrees-feat");
        std::fs::create_dir_all(&slug).unwrap();
        let session = format!("sid-{name}");
        let lines: Vec<String> = said
            .iter()
            .enumerate()
            .map(|(i, text)| {
                serde_json::json!({
                    "type": "assistant",
                    "message": {
                        "id": format!("m{i}"),
                        "content": [{"type": "text", "text": text}],
                        "usage": {"input_tokens": used, "output_tokens": 5}
                    }
                })
                .to_string()
            })
            .collect();
        std::fs::write(slug.join(format!("{session}.jsonl")), lines.join("\n")).unwrap();
        let id = session_on_branch(app, name, status);
        let agent = app.tree.agents.iter_mut().find(|a| a.id == id).unwrap();
        agent.kind = orion_core::AgentKind::Claude;
        agent.model = Some("opus".into());
        agent.session_id = Some(session);
        let places = crate::paths_util::Places {
            home: Some(home.path().to_path_buf()),
            claude: Some(claude),
            ..Default::default()
        };
        (id, places)
    }

    /// The router's second branch: the branch's own agent, idle with room
    /// in its context, gets the breakage as a new turn — no new session —
    /// and the try counts.
    #[test]
    fn an_idle_branch_agent_with_room_gets_the_breakage_as_a_turn() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "RIPLO-1",
            AgentStatus::Finished,
            &["Opened the draft."],
            100_000,
        );
        listed(&mut app);
        let page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        let out = crate::paths_util::with_places(places, || landed(&mut app, &page));
        let (to, text) = sent_turn(&out).expect("a turn");
        assert_eq!(to, id);
        assert!(text.starts_with("orion's PR fixer"), "{text}");
        assert!(text.contains("- Failing checks:"), "{text}");
        assert!(text.contains("--log-failed"), "{text}");
        assert!(text.contains("then stop: orion watches CI"), "{text}");
        assert!(sent_prompt(&out).is_none(), "no new session");
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 1);
        assert!(app.autofix.watching.is_empty());
    }

    /// Turn the session `id` into one started in another project that
    /// wrote in the PR's checkout `ago_ms` ago: its own checkout is
    /// elsewhere, the daemon's visit book has it at work on the branch.
    fn make_visitor(app: &mut App, id: &orion_core::AgentId, ago_ms: i64) {
        let other = orion_core::WorktreeId("blue-sky".into());
        app.tree.projects.push(orion_core::Project {
            id: ProjectId("q".into()),
            name: "blue-sky".into(),
            repo_path: "/tmp/blue-sky".into(),
            sort_order: 1,
        });
        app.tree.worktrees.push(orion_core::Worktree {
            id: other.clone(),
            project_id: ProjectId("q".into()),
            path: "/tmp/blue-sky".into(),
            branch: "master".into(),
            is_main: true,
            sort_order: 0,
        });
        app.tree
            .agents
            .iter_mut()
            .find(|a| &a.id == id)
            .unwrap()
            .worktree_id = other;
        app.visits = vec![orion_core::AgentVisit {
            agent: id.clone(),
            worktree: orion_core::WorktreeId("feat-wt".into()),
            at: orion_core::clock::now_ms() - ago_ms,
        }];
    }

    /// The module-platform collision: the session that built the branch
    /// lives in another project and works in the PR's checkout with `cd`.
    /// Seen through the visit book, it is the branch's agent — idle, it
    /// gets the breakage as a turn told where the checkout is; busy, the
    /// breakage waits for it. Never a fresh fixer beside it.
    #[test]
    fn a_session_from_another_project_at_work_on_the_branch_is_its_agent() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "GTM Autopilot",
            AgentStatus::Finished,
            &["Committed phase 6."],
            100_000,
        );
        make_visitor(&mut app, &id, 60_000);
        listed(&mut app);
        let page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        let out = crate::paths_util::with_places(places.clone(), || landed(&mut app, &page));
        let (to, text) = sent_turn(&out).expect("a turn");
        assert_eq!(to, id);
        assert!(
            text.contains("checked out at `/tmp/repo-worktrees/feat`"),
            "{text}"
        );
        assert!(sent_prompt(&out).is_none(), "no fixer beside it");

        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "GTM Autopilot",
            AgentStatus::Running,
            &["Merging dev."],
            10_000,
        );
        make_visitor(&mut app, &id, 60_000);
        listed(&mut app);
        let out = crate::paths_util::with_places(places, || landed(&mut app, &page));
        assert_eq!(launches(&out), 0, "{out:?}");
        assert_eq!(app.autofix.queued.len(), 1, "waits for it");
    }

    /// Someone orion can't see committing in the PR's checkout a moment
    /// ago: no fresh fixer goes in beside them; the breakage waits, still
    /// watched, and the flash says why once.
    #[test]
    fn a_checkout_in_use_holds_a_fresh_fixer() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        };
        git(&["init", "-q", "-b", "feat"]);
        std::fs::write(repo.path().join("a.txt"), "one").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "wip"]);

        let mut app = app_with(vec![broken(false, true)], true);
        app.tree.worktrees.push(orion_core::Worktree {
            id: orion_core::WorktreeId("feat-wt".into()),
            project_id: ProjectId("p".into()),
            path: repo.path().to_path_buf(),
            branch: pr().head,
            is_main: false,
            sort_order: 0,
        });
        listed(&mut app);
        let page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        let out = landed(&mut app, &page);
        assert_eq!(launches(&out), 0, "{out:?}");
        assert!(
            app.autofix.watching.contains_key(&pr().url),
            "still watched"
        );
        assert!(
            app.flash
                .as_deref()
                .is_some_and(|f| f.contains("checkout is in use (a git commit")),
            "{:?}",
            app.flash
        );
    }

    /// A visit past the visit book's two hours says nothing: with no other
    /// session on the branch, a fresh fixer goes.
    #[test]
    fn an_old_visit_does_not_hold_the_branch() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "GTM Autopilot",
            AgentStatus::Finished,
            &["Done yesterday."],
            100_000,
        );
        make_visitor(&mut app, &id, orion_core::touched_paths::VISIT_TTL_MS + 1);
        listed(&mut app);
        let page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        let out = crate::paths_util::with_places(places, || landed(&mut app, &page));
        assert!(sent_turn(&out).is_none(), "{out:?}");
        assert!(sent_prompt(&out).is_some(), "a fresh fixer");
    }

    /// The router's third branch: a busy branch agent — mid-turn, or
    /// waiting on the owner — has the breakage held for it, and gets it on
    /// the beat once it is idle. Held, it is not watched again.
    #[test]
    fn a_busy_branch_agent_gets_the_breakage_once_idle() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "RIPLO-1",
            AgentStatus::Running,
            &["Working."],
            10_000,
        );
        listed(&mut app);
        let page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        let out = crate::paths_util::with_places(places.clone(), || landed(&mut app, &page));
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(app.autofix.queued.len(), 1);
        assert!(app.autofix.watching.is_empty());
        assert!(app
            .flash
            .as_deref()
            .is_some_and(|f| f.contains("waits for RIPLO-1")));
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "held, not watched again");
        assert!(crate::paths_util::with_places(places.clone(), || beat(&mut app)).is_empty());

        app.tree
            .agents
            .iter_mut()
            .find(|a| a.id == id)
            .unwrap()
            .status = AgentStatus::NeedsFeedback;
        assert!(
            crate::paths_util::with_places(places.clone(), || beat(&mut app)).is_empty(),
            "waiting on the owner is busy too"
        );
        app.tree
            .agents
            .iter_mut()
            .find(|a| a.id == id)
            .unwrap()
            .status = AgentStatus::Finished;
        let out = crate::paths_util::with_places(places, || beat(&mut app));
        assert_eq!(sent_turn(&out).map(|(to, _)| to), Some(id));
        assert!(app.autofix.queued.is_empty());
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 1);
    }

    /// A breakage held for a busy agent that pushed meanwhile — the PR on
    /// another head now — is dropped rather than sent stale.
    #[test]
    fn a_held_breakage_is_dropped_once_the_branch_moves_on() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "RIPLO-1",
            AgentStatus::Running,
            &["Working."],
            10_000,
        );
        listed(&mut app);
        let page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        crate::paths_util::with_places(places.clone(), || landed(&mut app, &page));
        assert_eq!(app.autofix.queued.len(), 1);
        set_list(
            &mut app,
            vec![OpenPr {
                head_sha: "def456".into(),
                answered: Answered {
                    conflicts: Some(false),
                    checks: Some(Checks::Pending),
                },
                ..pr()
            }],
        );
        listed(&mut app);
        app.tree
            .agents
            .iter_mut()
            .find(|a| a.id == id)
            .unwrap()
            .status = AgentStatus::Finished;
        let out = crate::paths_util::with_places(places, || beat(&mut app));
        assert!(sent_turn(&out).is_none(), "{out:?}");
        assert!(app.autofix.queued.is_empty());
    }

    /// The router's fourth branch: an agent past 75% of its context hands
    /// over to a fresh `autofix-<n>` — told the last two things it said
    /// and the plan as approved.
    #[test]
    fn a_full_branch_agent_hands_over_to_a_fresh_fixer() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        let (_, places) = claude_on_branch(
            &mut app,
            &home,
            "RIPLO-1",
            AgentStatus::Finished,
            &["First.", "Pushed the fix for the flag.", "Draft PR is up."],
            800_000,
        );
        listed(&mut app);
        let mut page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        page.comments.push(crate::pull_request::PrComment {
            author: "o".into(),
            at: "2026-11-14T10:00:00Z".into(),
            review_state: String::new(),
            body: "## Plan as approved\n1. Add the flag".into(),
        });
        let out = crate::paths_util::with_places(places, || landed(&mut app, &page));
        assert!(sent_turn(&out).is_none(), "too full for a turn");
        let prompt = sent_prompt(&out).expect("a fresh fixer");
        assert!(
            prompt.starts_with("You are orion's PR autofixer."),
            "{prompt}"
        );
        let handover = &prompt[prompt.find("Handover:").expect("handed over")..];
        assert!(handover.contains("\"RIPLO-1\""), "{handover}");
        assert!(
            handover.contains("> Pushed the fix for the flag.\n\n> Draft PR is up."),
            "{handover}"
        );
        assert!(
            !handover.contains("First."),
            "the last two only: {handover}"
        );
        assert!(handover.contains("1. Add the flag"), "{handover}");
        assert!(out.iter().any(|r| matches!(
            r,
            ClientRequest::CreatePrAgent { name, .. } if name == "autofix-7"
        )));
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 1);
    }

    /// The router's first branch, asked directly: a live `autofix-<n>`
    /// keeps the work — a breakage is never sent to a second one.
    #[test]
    fn a_live_fixer_keeps_the_branch() {
        let mut app = app_with(vec![broken(false, true)], true);
        let fixer = session_on_branch(&mut app, "autofix-7", AgentStatus::Running);
        session_on_branch(&mut app, "builder", AgentStatus::Finished);
        let project = ProjectId("p".into());
        assert_eq!(
            router::route(&app, &project, "feat", 7, &pr().url),
            router::Route::FixerLive(fixer)
        );
        let mut out = Vec::new();
        let form = AutofixForm::new(project, pr(), &detail(false, Vec::new()), false);
        dispatch(&mut app, form, &mut out);
        assert!(out.is_empty(), "{out:?}");
        assert!(app.autofix.queued.is_empty());
    }

    // ---- drafts ----

    /// The builder ledger names the PR's branch: it is a factory PR.
    fn make_factory(app: &mut App) {
        app.automations.linear_builder.ledger.issues.insert(
            "issue-1".into(),
            super::super::linear_builder::Entry {
                automation: "bug_fixer".into(),
                identifier: "RIPLO-1".into(),
                title: "Fix the flag".into(),
                project: ProjectId("p".into()),
                branch: pr().head,
                session: None,
                stage: super::super::linear_builder::Stage::Building,
                at: 0,
                ..Default::default()
            },
        );
    }

    fn draft(conflicts: bool, failing: bool) -> OpenPr {
        OpenPr {
            answered_draft: true,
            ..broken(conflicts, failing)
        }
    }

    fn set_list(app: &mut App, list: Vec<OpenPr>) {
        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = list;
    }

    /// A draft isn't asking for anything: no fixer is sent unasked, and
    /// once it is marked ready its breakage is fixed as any other.
    #[test]
    fn a_draft_is_never_fixed_unasked() {
        let mut app = app_with(vec![draft(true, true)], true);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty());
        set_list(&mut app, vec![broken(true, true)]);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url), "ready now");
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(sent_prompt(&out).is_some(), "{out:?}");
    }

    /// orion's own knowledge pull request is never sent a fixer, however
    /// broken it is: orion rebuilds and merges it itself.
    #[test]
    fn the_knowledge_pull_request_is_never_fixed() {
        let knowledge = OpenPr {
            head: format!("{}someone", crate::knowledge::share::PREFIX),
            ..broken(true, true)
        };
        let mut app = app_with(vec![knowledge], true);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty());
    }

    /// A ready PR turned back to draft drops its watch — from the list,
    /// or from a body landing before the list says so.
    #[test]
    fn a_pr_turned_draft_drops_its_watch() {
        let mut app = app_with(vec![broken(true, false)], true);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));
        set_list(&mut app, vec![draft(true, false)]);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty());

        let mut app = app_with(vec![broken(true, false)], true);
        listed(&mut app);
        let page = PrDetail {
            answered_draft: true,
            ..detail(true, Vec::new())
        };
        let out = landed(&mut app, &page);
        assert!(out.is_empty(), "{out:?}");
        assert!(app.autofix.watching.is_empty());
    }

    /// `⌘G` still opens the form for a draft.
    #[test]
    fn cmd_g_still_opens_the_form_for_a_draft() {
        let mut app = app_with(vec![draft(true, false)], true);
        listed(&mut app);
        crate::pr_modal::open(&mut app);
        assert_eq!(
            crate::pr_modal::selected_pr(&app).map(|p| p.url),
            Some(pr().url)
        );
        app.autofix.pending_open = Some(pr().url);
        let page = PrDetail {
            answered_draft: true,
            ..detail(true, Vec::new())
        };
        landed(&mut app, &page);
        assert!(matches!(app.overlay, Some(Overlay::Autofix(_))));
    }

    /// The panel says a draft is left alone — a factory draft aside.
    #[test]
    fn the_panel_says_a_draft_is_left_alone() {
        let mut app = app_with(vec![draft(true, true)], true);
        listed(&mut app);
        let project = ProjectId("p".into());
        assert_eq!(
            open_pr_row(&app, &project, &draft(true, true)).state,
            "draft, left alone"
        );
        make_factory(&mut app);
        assert_eq!(
            open_pr_row(&app, &project, &draft(true, true)).state,
            "waiting for checks"
        );
    }

    /// A row whose checks ended with some cancelled and none failed.
    fn cancelled_row() -> OpenPr {
        OpenPr {
            answered: Answered {
                conflicts: Some(false),
                checks: Some(Checks::Cancelled),
            },
            ..pr()
        }
    }

    /// A cancelled Actions job of run `run`.
    fn cancelled(name: &str, run: u64) -> PrCheck {
        PrCheck {
            word: "CANCELLED".into(),
            url: format!("https://github.com/o/r/actions/runs/{run}/job/1"),
            ..check(name, "Verify app", CheckState::Cancelled)
        }
    }

    /// A cancelled check is no failure: nothing to fix, no fingerprint —
    /// and its run is named once to start again, but only while nothing
    /// failed, conflicts or runs.
    #[test]
    fn a_cancelled_check_is_not_a_failure() {
        let page = detail(
            false,
            vec![
                cancelled("Wait for Vercel preview", 11),
                cancelled("MCP checks", 11),
                cancelled("Build app", 12),
                check("unit", "CI", CheckState::Passed),
            ],
        );
        let d = diag(&page);
        assert!(d.is_empty(), "{d:?}");
        assert!(!d.has(Issue::Checks));
        assert_eq!(d.cancelled.len(), 3);
        assert_eq!(d.cancelled_runs(), [11, 12]);
        assert_eq!(d.summary(), "nothing failing");

        let mut busy = page.clone();
        busy.checks.push(check("e2e", "CI", CheckState::Running));
        assert!(diag(&busy).cancelled_runs().is_empty(), "still running");
        let mut red = page.clone();
        red.checks.push(check("lint", "CI", CheckState::Failed));
        let d = diag(&red);
        assert!(d.cancelled_runs().is_empty(), "a failure is the agent's");
        assert_eq!(d.fingerprint("abc123").checks, ["lint"]);
    }

    /// A factory pull request whose checks ended cancelled has its
    /// cancelled runs started again — no agent — once per head commit,
    /// the ledger keeping the commit. Another push asks again; a plain
    /// pull request is left alone.
    #[test]
    fn a_factory_prs_cancelled_runs_are_rerun_once_per_head() {
        let mut app = app_with(vec![cancelled_row()], true);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "not a factory PR");
        assert_eq!(
            open_pr_row(&app, &ProjectId("p".into()), &cancelled_row()).state,
            "checks cancelled"
        );

        make_factory(&mut app);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));
        let page = detail(
            false,
            vec![
                cancelled("MCP checks", 11),
                cancelled("Build app", 11),
                check("unit", "CI", CheckState::Passed),
            ],
        );
        let out = landed(&mut app, &page);
        assert!(out.is_empty(), "no agent: {out:?}");
        assert_eq!(
            app.autofix.ledger[&pr().url].rerun.as_deref(),
            Some("abc123")
        );
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 0);
        assert!(app.autofix.watching.is_empty());
        let flash = app.flash.take().expect("said so");
        assert_eq!(flash.text, "PR fixer re-ran 2 cancelled checks on #7");
        assert_eq!(
            open_pr_row(&app, &ProjectId("p".into()), &cancelled_row()).state,
            "re-ran cancelled checks"
        );

        // The same commit is not asked about again.
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "once per head");
        let d = diag(&page);
        let dir = std::path::PathBuf::from("/tmp/repo");
        assert!(rerun_cancelled(&mut app, &pr().url, &dir, &page, &d).is_empty());
        assert!(app.flash.is_none());

        // A push: the new head's cancelled runs are started again.
        let pushed = OpenPr {
            head_sha: "def456".into(),
            ..cancelled_row()
        };
        set_list(&mut app, vec![pushed]);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url), "a new head");
        let page = PrDetail {
            head_sha: "def456".into(),
            ..page
        };
        landed(&mut app, &page);
        assert_eq!(
            app.autofix.ledger[&pr().url].rerun.as_deref(),
            Some("def456")
        );
    }

    /// A factory draft (its branch in the builder ledger) is not left
    /// alone: its failing checks are sent at once — its conflicts are not,
    /// while it is a draft — and a draft whose checks pass is green
    /// whatever its conflicts. Once ready, its conflicts count again.
    #[test]
    fn a_factory_drafts_failing_checks_are_fixed_but_not_its_conflicts() {
        let mut app = app_with(vec![draft(true, false)], true);
        make_factory(&mut app);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "conflicts alone wait");

        set_list(&mut app, vec![draft(true, true)]);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));
        let page = PrDetail {
            answered_draft: true,
            ..detail(true, vec![check("unit", "CI", CheckState::Failed)])
        };
        let out = landed(&mut app, &page);
        let prompt = sent_prompt(&out).expect("sent at once");
        let fix = &prompt[prompt.find("\nFix:\n").unwrap()..];
        assert!(fix.contains("- Failing checks:"), "{fix}");
        assert!(!fix.contains("Merge conflicts"), "{fix}");

        // Its checks pass, its conflicts stay: green, the tries forgotten.
        set_list(&mut app, vec![draft(true, false)]);
        listed(&mut app);
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 0);

        // Marked ready, its conflicts are fixed like any ready PR's.
        set_list(&mut app, vec![broken(true, false)]);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));
    }

    /// Out of tries, a factory PR raises the *Can't fix* card — no flash
    /// — and Park leaves it alone until it goes green.
    #[test]
    fn a_factory_pr_out_of_tries_raises_cant_fix_and_park_leaves_it() {
        let mut app = app_with(vec![broken(false, true)], true);
        make_factory(&mut app);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                attempts: AUTO_ATTEMPTS,
                ..Record::default()
            },
        );
        listed(&mut app);
        let page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        let out = landed(&mut app, &page);
        assert!(out.is_empty(), "{out:?}");
        // At the desk the card's arrival is the footer's only line.
        assert!(
            app.flash.as_ref().is_none_or(|f| f.text.starts_with("◆ ")),
            "a card, not a flash: {:?}",
            app.flash
        );
        let key = cant_fix_key(&pr().url);
        let card = &app.factory_cards.last().expect("a card").card;
        assert_eq!(card.key, key);
        assert_eq!(card.body[0], "CI still red after 3 fixes: unit.");
        let labels: Vec<&str> = card.buttons.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Open PR", "Reply…", "Park"]);

        crate::factory::cards::on_action(&mut app, &key, "park", None, true);
        assert!(app.autofix.ledger[&pr().url].parked);
        // A push — a new breakage — is still left alone while parked.
        let pushed = OpenPr {
            head_sha: "def456".into(),
            ..broken(false, true)
        };
        set_list(&mut app, vec![pushed.clone()]);
        listed(&mut app);
        assert!(app.autofix.watching.is_empty(), "parked");
        assert_eq!(
            open_pr_row(&app, &ProjectId("p".into()), &pushed).state,
            "parked"
        );
        set_list(&mut app, vec![broken(false, false)]);
        listed(&mut app);
        assert!(!app.autofix.ledger[&pr().url].parked, "green unparks");
    }

    /// Reply… on *Can't fix* sends the owner's words to the branch's agent
    /// and starts the tries over.
    #[test]
    fn a_reply_on_cant_fix_goes_to_the_branchs_agent() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        make_factory(&mut app);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "RIPLO-1",
            AgentStatus::Finished,
            &["Tried."],
            10_000,
        );
        app.autofix.ledger.insert(
            pr().url,
            Record {
                attempts: AUTO_ATTEMPTS,
                ..Record::default()
            },
        );
        let key = cant_fix_key(&pr().url);
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            crate::paths_util::with_places(places, || {
                crate::factory::cards::on_action(
                    &mut app,
                    &key,
                    "reply",
                    Some("The flake is in the fixture, not the code".into()),
                    true,
                )
            })
        });
        let (to, text) = sent_turn(&app.outbox).expect("a turn");
        assert_eq!(to, id);
        assert!(
            text.contains("The flake is in the fixture, not the code"),
            "{text}"
        );
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 0);
    }

    /// An `autofix-<n>` whose PR has gone green, its turn done, is
    /// archived; one still at work, or on a PR still red, is not.
    #[test]
    fn a_green_fixer_is_archived() {
        let archived = |out: &[ClientRequest]| -> Vec<orion_core::AgentId> {
            out.iter()
                .filter_map(|r| match r {
                    ClientRequest::ArchiveAgent { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect()
        };
        let mut app = app_with(vec![broken(false, false)], true);
        let done = session_on_branch(&mut app, "autofix-7", AgentStatus::Finished);
        session_on_branch(&mut app, "builder", AgentStatus::Finished);
        listed(&mut app);
        assert_eq!(archived(&beat(&mut app)), [done], "the fixer alone");

        let mut app = app_with(vec![broken(false, false)], true);
        session_on_branch(&mut app, "autofix-7", AgentStatus::Running);
        listed(&mut app);
        assert!(archived(&beat(&mut app)).is_empty(), "still at work");

        let mut app = app_with(vec![broken(false, true)], true);
        session_on_branch(&mut app, "autofix-7", AgentStatus::Finished);
        listed(&mut app);
        assert!(archived(&beat(&mut app)).is_empty(), "still red");
    }

    /// The owner's comments on a factory PR go to the branch's agent,
    /// verbatim with their file and line, once each; what was there on
    /// the first look is history. A PR that isn't a factory PR's is not
    /// read for them.
    #[test]
    fn the_owners_new_comments_are_relayed_once() {
        use crate::pr_review::github::{ReviewThread, Side, ThreadComment};
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, false)], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "RIPLO-1",
            AgentStatus::Finished,
            &["Up."],
            10_000,
        );
        listed(&mut app);
        assert!(
            app.autofix.polls.get(&pr().url).is_some_and(|p| !p.factory),
            "read for Cubic alone: not a factory PR"
        );
        make_factory(&mut app);
        listed(&mut app);
        assert_eq!(app.autofix.want.len(), 1, "read at once");
        listed(&mut app);
        assert_eq!(app.autofix.want.len(), 1, "and not again till it moves");

        let comment = |at: &str, body: &str| crate::pull_request::PrComment {
            author: "o".into(),
            at: at.into(),
            review_state: String::new(),
            body: body.into(),
        };
        let mut page = PrDetail {
            author: "o".into(),
            comments: vec![comment("2026-11-14T09:00:00Z", "old note")],
            ..detail(false, Vec::new())
        };
        let land = |app: &mut App, page: &PrDetail| {
            crate::paths_util::with_places(places.clone(), || landed(app, page))
        };
        assert!(
            land(&mut app, &page).is_empty(),
            "the first look is history"
        );

        page.comments
            .push(comment("2026-11-14T10:00:00Z", "Call it `beta_flag`"));
        page.threads.push(ReviewThread {
            id: "t".into(),
            path: "src/flag.rs".into(),
            line: Some(4),
            start_line: None,
            original_line: None,
            side: Side::Right,
            resolved: false,
            outdated: false,
            comments: vec![ThreadComment {
                id: "c".into(),
                author: "o".into(),
                body: "This belongs in the registry".into(),
                at: "2026-11-14T10:05:00Z".into(),
                mine: true,
                pending: false,
            }],
        });
        let out = land(&mut app, &page);
        let (to, text) = sent_turn(&out).expect("relayed");
        assert_eq!(to, id);
        assert!(
            text.contains("On the pull request:\n> Call it `beta_flag`"),
            "{text}"
        );
        assert!(
            text.contains("On `src/flag.rs` line 4:\n> This belongs in the registry"),
            "{text}"
        );
        assert!(!text.contains("old note"), "{text}");
        assert!(land(&mut app, &page).is_empty(), "once");
    }

    /// A review thread Cubic opened on line `line`, answered by nobody.
    fn cubic_thread(id: &str, line: u32, body: &str) -> crate::pr_review::github::ReviewThread {
        use crate::pr_review::github::{ReviewThread, Side, ThreadComment};
        ReviewThread {
            id: id.into(),
            path: "src/a.rs".into(),
            line: Some(line),
            start_line: None,
            original_line: Some(line),
            side: Side::Right,
            resolved: false,
            outdated: false,
            comments: vec![ThreadComment {
                id: format!("{id}-c"),
                author: "cubic-dev-ai[bot]".into(),
                body: body.into(),
                at: "2026-11-14T10:00:00Z".into(),
                mine: false,
                pending: false,
            }],
        }
    }

    /// Cubic's findings on a green pull request — not only a factory one —
    /// go to the branch's own agent as a turn, once each: one answered on
    /// its thread, or resolved, is left be. The turn asks for a judgement
    /// against what the agent knows, a new commit on top, and a reason on
    /// the thread of each one left; a later finding is the next round.
    #[test]
    fn cubics_findings_on_a_green_pr_go_to_the_branchs_agent_once() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, false)], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "builder",
            AgentStatus::Finished,
            &["Built it."],
            10_000,
        );
        listed(&mut app);
        assert_eq!(app.autofix.want.len(), 1, "a green PR is read");

        let mut answered = cubic_thread("PRRT_answered", 9, "**P1** stale cache");
        answered
            .comments
            .push(crate::pr_review::github::ThreadComment {
                author: "o".into(),
                body: "Intended: the cache is rebuilt on boot".into(),
                mine: true,
                ..answered.comments[0].clone()
            });
        let mut resolved = cubic_thread("PRRT_resolved", 11, "**P0** gone");
        resolved.resolved = true;
        let mut page = PrDetail {
            threads: vec![
                cubic_thread("PRRT_leak", 3, "**P1** the handle leaks on error"),
                answered,
                resolved,
            ],
            ..detail(false, Vec::new())
        };
        let land = |app: &mut App, page: &PrDetail| {
            crate::paths_util::with_places(places.clone(), || landed(app, page))
        };
        let out = land(&mut app, &page);
        let (to, text) = sent_turn(&out).expect("sent");
        assert_eq!(to, id, "the agent that built it");
        for needle in [
            "Cubic, the review bot, left 1 finding on your pull request #7",
            "round 1 of 3",
            "1. P1 on `src/a.rs` line 3 — thread `PRRT_leak`:\n> **P1** the handle leaks on error",
            "left out on purpose",
            "as a new commit on top (no amend, no force-push)",
            "addPullRequestReviewThreadReply",
            "resolveReviewThread",
            "Cubic dismissals:",
            router::AGENT_MARKER,
        ] {
            assert!(text.contains(needle), "{needle}:\n{text}");
        }
        assert!(!text.contains("stale cache"), "answered: {text}");
        assert!(!text.contains("gone"), "resolved: {text}");
        assert!(land(&mut app, &page).is_empty(), "once");
        assert_eq!(app.autofix.ledger[&pr().url].cubic_rounds, 1);

        page.threads
            .push(cubic_thread("PRRT_next", 5, "**P2** name it"));
        app.tree
            .agents
            .iter_mut()
            .filter(|a| a.id == id)
            .for_each(|a| a.status = AgentStatus::Finished);
        let out = land(&mut app, &page);
        let text = match sent_turn(&out) {
            Some((_, text)) => text,
            None => match &app.autofix.queued[..] {
                [q] => router::turn_text(&q.work),
                other => panic!("neither sent nor held: {other:?}"),
            },
        };
        assert!(text.contains("round 2 of 3"), "{text}");
        assert!(
            text.contains("PRRT_next") && !text.contains("PRRT_leak"),
            "{text}"
        );
    }

    /// A project whose PR contract says `[flow] ready = "when-clean"`.
    fn when_clean(app: &mut App) {
        app.autofix.flows.insert(
            ProjectId("p".into()),
            flow::Known {
                at: Instant::now(),
                flow: crate::pr_body::contract::Flow {
                    ready: crate::pr_body::contract::Ready::WhenClean,
                },
            },
        );
    }

    /// A green draft's body: every check passed at its head.
    fn green_draft_page(threads: Vec<crate::pr_review::github::ReviewThread>) -> PrDetail {
        PrDetail {
            answered_draft: true,
            answered: Answered {
                conflicts: Some(false),
                checks: Some(Checks::Passing),
            },
            threads,
            ..detail(false, vec![check("unit", "CI", CheckState::Passed)])
        }
    }

    /// In a `when-clean` repo a draft is where the fixing happens: its
    /// failing checks are sent unasked, as a ready pull request's are.
    #[test]
    fn a_when_clean_repos_failing_draft_is_fixed() {
        let mut app = app_with(vec![draft(false, true)], true);
        listed(&mut app);
        assert!(
            app.autofix.watching.is_empty(),
            "a plain repo leaves drafts be"
        );

        let mut app = app_with(vec![draft(false, true)], true);
        when_clean(&mut app);
        listed(&mut app);
        assert!(app.autofix.watching.contains_key(&pr().url));
        let page = PrDetail {
            answered_draft: true,
            ..detail(false, vec![check("unit", "CI", CheckState::Failed)])
        };
        let out = landed(&mut app, &page);
        assert!(sent_prompt(&out).is_some(), "{out:?}");
    }

    /// A clean draft in a `when-clean` repo is marked ready once per head:
    /// not with a P1 from Cubic open, not while a session works on its
    /// branch, and never in a repo whose flow leaves drafts to the owner.
    #[test]
    fn a_clean_draft_is_marked_ready_once() {
        let green = OpenPr {
            answered_draft: true,
            answered: Answered {
                conflicts: Some(false),
                checks: Some(Checks::Passing),
            },
            ..pr()
        };
        let readied = |app: &App| {
            app.autofix
                .ledger
                .get(&pr().url)
                .and_then(|r| r.readied.clone())
        };

        let mut app = app_with(vec![green.clone()], true);
        listed(&mut app);
        landed(&mut app, &green_draft_page(Vec::new()));
        assert_eq!(readied(&app), None, "manual flow");

        let p1 = cubic_thread("PRRT_p1", 3, "**P1** the lock is never released");
        let mut answered = p1.clone();
        answered
            .comments
            .push(crate::pr_review::github::ThreadComment {
                author: "oliverkidd".into(),
                body: "Released in drop().".into(),
                mine: true,
                ..p1.comments[0].clone()
            });
        let mut app = app_with(vec![green.clone()], true);
        when_clean(&mut app);
        listed(&mut app);
        let out = landed(&mut app, &green_draft_page(vec![answered.clone()]));
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(readied(&app).as_deref(), Some("abc123"));
        assert!(app
            .flash
            .as_deref()
            .is_some_and(|f| f.contains("marked #7 ready")));
        app.flash = None;
        landed(&mut app, &green_draft_page(vec![answered.clone()]));
        assert!(app.flash.is_none(), "once per head");

        let mut app = app_with(vec![green.clone()], true);
        when_clean(&mut app);
        listed(&mut app);
        landed(&mut app, &green_draft_page(vec![p1]));
        assert_eq!(readied(&app), None, "a P1 open");

        let mut app = app_with(vec![green], true);
        when_clean(&mut app);
        session_on_branch(&mut app, "builder", AgentStatus::Running);
        listed(&mut app);
        landed(&mut app, &green_draft_page(vec![answered]));
        assert_eq!(readied(&app), None, "someone is at work on it");
    }

    /// Cubic's findings go the moment they post, while CI still runs, so
    /// their fix is pushed before the run finishes. On a broken pull
    /// request they ride with the breakage, in one turn and one push. Not
    /// sent with the PR fixer off; once the rounds have run out, the owner
    /// is told and nothing goes.
    #[test]
    fn cubic_goes_while_checks_run_and_stops_after_its_rounds() {
        let home = tempfile::tempdir().unwrap();
        let running = OpenPr {
            answered: Answered {
                conflicts: Some(false),
                checks: Some(Checks::Pending),
            },
            ..pr()
        };
        let mut app = app_with(vec![running], true);
        let (id, places) = claude_on_branch(
            &mut app,
            &home,
            "builder",
            AgentStatus::Finished,
            &["Pushed."],
            10_000,
        );
        listed(&mut app);
        assert_eq!(app.autofix.want.len(), 1, "read while CI runs");
        let page = PrDetail {
            threads: vec![cubic_thread("PRRT_a", 3, "**P1** leak")],
            ..detail(false, vec![check("e2e", "", CheckState::Running)])
        };
        let out = crate::paths_util::with_places(places, || landed(&mut app, &page));
        let (to, text) = sent_turn(&out).expect("sent before CI is done");
        assert_eq!(to, id);
        assert!(text.contains("PRRT_a"), "{text}");

        let page = PrDetail {
            threads: vec![cubic_thread("PRRT_a", 3, "**P1** leak")],
            ..detail(false, vec![check("unit", "", CheckState::Failed)])
        };
        let mut app = app_with(vec![broken(false, true)], true);
        session_on_branch(&mut app, "builder", AgentStatus::Finished);
        listed(&mut app);
        let out = landed(&mut app, &page);
        let prompt = sent_prompt(&out).expect("the breakage goes");
        assert!(prompt.contains("- Failing checks:"), "{prompt}");
        assert!(prompt.contains("In the same push:"), "{prompt}");
        assert!(prompt.contains("thread `PRRT_a`"), "{prompt}");
        let record = &app.autofix.ledger[&pr().url];
        assert_eq!(
            (record.cubic_sent.clone(), record.cubic_rounds),
            (vec!["PRRT_a".to_string()], 1)
        );

        let green = PrDetail {
            threads: page.threads.clone(),
            ..detail(false, Vec::new())
        };
        let mut app = app_with(vec![broken(false, false)], false);
        listed(&mut app);
        assert!(app.autofix.polls.is_empty(), "the PR fixer is off");
        assert!(landed(&mut app, &green).is_empty());

        let mut app = app_with(vec![broken(false, false)], true);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                cubic_rounds: CUBIC_ROUNDS,
                ..Record::default()
            },
        );
        listed(&mut app);
        assert!(landed(&mut app, &green).is_empty(), "no more rounds");
        assert_eq!(app.autofix.ledger[&pr().url].cubic_sent, ["PRRT_a"]);
        let flash = app
            .flash
            .as_ref()
            .map(|f| f.text.clone())
            .unwrap_or_default();
        assert!(flash.contains("over to you"), "{flash}");
    }

    /// The agent runs as the session that did the work on the branch —
    /// its harness, model and effort — when that session can't take the
    /// work as a turn (its conversation can't be read here).
    #[test]
    fn autofix_copies_the_branchs_agent_when_it_cannot_take_the_turn() {
        let mut app = app_with(vec![broken(true, false)], true);
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
    }

    /// What a fresh fixer is told while other sessions are live on the
    /// branch: read each one's conversation, and stand down when one is
    /// already on it.
    #[test]
    fn the_stand_down_preamble_names_each_live_session() {
        let peers = [Peer {
            name: "billing".into(),
            waiting: true,
            transcript: "/x/sid-1.jsonl".into(),
        }];
        let text = stand_down_preamble(&peers, &pr());
        assert!(text.starts_with("Before you change anything"), "{text}");
        assert!(text.contains("\"billing\" (waiting on the user)"), "{text}");
        assert!(text.contains("sid-1.jsonl"), "{text}");
        assert!(text.contains("orion autofix stand-down"), "{text}");
        assert_eq!(stand_down_preamble(&[], &pr()), "");
    }

    /// A stand-down flashes its reason once; the ones archived before
    /// this run are taken note of, never flashed.
    #[test]
    fn a_stand_down_is_flashed_once_with_its_reason() {
        let mut app = app_with(Vec::new(), true);
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
            Some("PR fixer stood down: billing: already on it")
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
        let mut app = app_with(vec![unsure(Checks::Failing)], true);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                handled: None,
                attempts: 2,
                ..Record::default()
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
        let mut app = app_with(vec![broken(true, false)], true);
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

        let mut app = app_with(vec![broken(true, false)], true);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                handled: None,
                attempts: AUTO_ATTEMPTS,
                ..Record::default()
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

    /// The cap never holds the PR fixer back: with every place taken, a
    /// breakage still launches its fixer at once. Only the factory paused
    /// at its account's limit keeps it waiting, still watched.
    #[test]
    fn a_full_cap_never_holds_the_fixer_back() {
        let launched = |out: &[ClientRequest]| {
            out.iter().any(|req| {
                matches!(req, ClientRequest::CreatePrAgent { name, .. } if name == "autofix-7")
            })
        };
        let mut app = app_with(vec![broken(true, false)], true);
        app.automations.settings.max_running = 1;
        app.automations.launching.insert(99, Automation::BugFixer);
        assert!(!super::super::may_start(&app), "the cap is full");
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(launched(&out), "{out:?}");

        let mut app = app_with(vec![broken(true, false)], true);
        app.automations.account.paused = Some(super::super::limit::Pause {
            account: "claude".into(),
            label: "Work".into(),
            program: "claude".into(),
            env: Vec::new(),
            until_ms: i64::MAX,
            said: "You've hit your weekly limit".into(),
        });
        listed(&mut app);
        let out = landed(&mut app, &detail(true, Vec::new()));
        assert!(!launched(&out), "paused at the limit: {out:?}");
        assert!(
            app.autofix.watching.contains_key(&pr().url),
            "still watched"
        );
    }

    #[test]
    fn going_green_forgets_the_attempts() {
        let mut app = app_with(vec![broken(false, false)], true);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                handled: handled("x", false),
                attempts: 2,
                ..Record::default()
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
            "PR fixer #7",
            "#7 Add things",
            "merge conflicts · 2 failing checks",
            "[x] Merge conflicts",
            "main has moved on",
            "[x] Failing checks  2 failing",
            "CI              unit tests, lint",
            "Note",
            "send",
            "not now",
        ] {
            assert!(shot.contains(text), "{text} missing:\n{shot}");
        }
    }

    /// The rows are the clock: a settling breakage is read again on the
    /// settle beat, not on every list, and at once when its row moves —
    /// a push, a comment (`updatedAt`) — whenever that is.
    #[test]
    fn a_settling_pr_is_read_again_as_soon_as_its_row_moves() {
        let row = |updated: &str| OpenPr {
            meta: crate::pull_request::PrMeta {
                updated_at: updated.into(),
                ..Default::default()
            },
            ..broken(false, true)
        };
        let mut app = app_with(vec![row("1")], true);
        listed(&mut app);
        assert_eq!(app.autofix.want.len(), 1);
        let running = detail(
            false,
            vec![
                check("unit", "", CheckState::Failed),
                check("e2e", "", CheckState::Running),
            ],
        );
        landed(&mut app, &running);
        assert!(app.autofix.want.is_empty(), "read");
        assert_eq!(needs(&app), vec![(pr().url, SETTLE_RECHECK)], "settling");

        listed(&mut app);
        assert!(app.autofix.want.is_empty(), "the row sat still");

        app.open_prs.get_mut(&ProjectId("p".into())).unwrap().list = vec![row("2")];
        listed(&mut app);
        assert_eq!(app.autofix.want.len(), 1, "the row moved");

        // The relaunch's ledger says it was read at this row: not again.
        let read = app.autofix.ledger[&pr().url].read.clone();
        landed(&mut app, &running);
        assert_ne!(
            app.autofix.ledger[&pr().url].read,
            read,
            "read at the new row"
        );
        app.autofix.watching.clear();
        listed(&mut app);
        assert!(app.autofix.want.is_empty(), "nothing moved since the read");
    }

    // ---- the conflicting files ----

    /// [`app_with`] on a real repo whose `feature` conflicts with `main`
    /// in `a.txt` and `gone.txt`, and a page of #7 at `feature`'s tip.
    fn conflicted_app(dir: &tempfile::TempDir) -> (App, PrDetail) {
        let (_, clone) = crate::conflicts::tests::conflicted_clone(dir);
        let head = crate::conflicts::tests::git(&clone, &["rev-parse", "HEAD"]);
        let mut app = app_with(vec![broken(true, false)], true);
        app.tree.projects[0].repo_path = clone;
        let page = PrDetail {
            head_sha: head,
            ..detail(true, Vec::new())
        };
        (app, page)
    }

    /// The conflict's line in the prompt names each file the repo says
    /// conflicts, under orion's rule to merge the base in and resolve.
    #[test]
    fn the_prompt_names_the_files_that_conflict() {
        let page = detail(true, Vec::new());
        let mut diagnosis = diag(&page);
        diagnosis.conflicted = vec!["a.txt".into(), "src/gone.rs".into()];
        let text = prompt(
            &pr(),
            &PrRefs::of(&page),
            &diagnosis,
            &[true, false],
            "",
            &orions(),
            "",
            "",
        );
        assert!(
            text.contains("- Merge conflicts with `main`, in:\n  - a.txt\n  - src/gone.rs\n"),
            "{text}"
        );
    }

    /// An unasked send to a conflicting pull request names the files its
    /// head conflicts in with its base, as the repo merges them
    /// (`conflicts::local`).
    #[test]
    fn an_unasked_send_names_the_files_the_repo_says_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, page) = conflicted_app(&dir);
        listed(&mut app);
        let out = landed(&mut app, &page);
        let prompt = sent_prompt(&out).expect("sent");
        assert!(
            prompt.contains("- Merge conflicts with `main`, in:\n  - a.txt\n  - gone.txt"),
            "{prompt}"
        );
        assert!(app.autofix.waiting.is_empty());
    }

    /// The repo's answer is off the loop: the send waits for it — the PR
    /// not asked about again meanwhile — and goes on the beat after it
    /// lands, the files named.
    #[tokio::test]
    async fn a_send_waits_for_its_files_and_goes_on_the_beat() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, page) = conflicted_app(&dir);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.view_jobs = Some(crate::view_jobs::Jobs::new(tx));
        listed(&mut app);
        let out = landed(&mut app, &page);
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(app.autofix.waiting.len(), 1);
        let mut out = Vec::new();
        send_waiting(&mut app, &mut out);
        assert!(out.is_empty(), "the files are not in yet");

        listed(&mut app);
        assert!(landed(&mut app, &page).is_empty(), "no second send");
        assert_eq!(app.autofix.waiting.len(), 1);

        loop {
            match rx.recv().await {
                Some(crate::view_jobs::Answer::Conflicts(answer)) => {
                    crate::conflicts::land(&mut app, answer);
                    break;
                }
                // The project's PR flow, read beside it.
                Some(crate::view_jobs::Answer::RepoFlow { project, flow }) => {
                    flow::land(&mut app, project, flow)
                }
                other => panic!("the repo's answer, not {other:?}"),
            }
        }
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("agent_presets.json"), || {
            crate::config::with_config_path(dir.path().join("config.json"), || {
                send_waiting(&mut app, &mut out)
            })
        });
        let prompt = sent_prompt(&out).expect("sent on the beat");
        assert!(prompt.contains("  - a.txt\n  - gone.txt"), "{prompt}");
        assert!(app.autofix.waiting.is_empty());
        assert_eq!(app.autofix.ledger[&pr().url].attempts, 1);
    }

    /// `⌘G`'s form names the conflicting files on its Merge conflicts row.
    #[test]
    fn the_form_names_the_conflicting_files() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, page) = conflicted_app(&dir);
        app.prs.observe(
            &page.url,
            crate::pr_store::PrObservation::of_detail(&page),
            crate::fetch::Asked::At(crate::fetch::now()),
        );
        open_for(&mut app, ProjectId("p".into()), pr(), &page);
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
        assert!(
            shot.contains("[x] Merge conflicts 2 files: a.txt, gone.txt"),
            "{shot}"
        );
    }

    // ---- one fresh fixer per pull request ----

    /// How many fresh fixers `out` launches.
    fn launches(out: &[ClientRequest]) -> usize {
        out.iter()
            .filter(|r| matches!(r, ClientRequest::CreatePrAgent { .. }))
            .count()
    }

    /// A breakage and the owner's words held for a busy agent that turns
    /// out too full: the beat launches one fresh fixer for the first, and
    /// the second waits for it — handed to it once it shows, and sent as
    /// its turn once that turn ends.
    #[test]
    fn held_work_launches_one_fresh_fixer_and_the_rest_waits_for_it() {
        let home = tempfile::tempdir().unwrap();
        let mut app = app_with(vec![broken(false, true)], true);
        let (builder, places) = claude_on_branch(
            &mut app,
            &home,
            "RIPLO-1",
            AgentStatus::Running,
            &["Working."],
            800_000,
        );
        listed(&mut app);
        let page = detail(false, vec![check("unit", "CI", CheckState::Failed)]);
        crate::paths_util::with_places(places.clone(), || landed(&mut app, &page));
        let mut out = Vec::new();
        assert!(send_words(
            &mut app,
            &pr().url,
            "Rename the flag".into(),
            "send back",
            &mut out
        ));
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(app.autofix.queued.len(), 2);

        app.tree
            .agents
            .iter_mut()
            .find(|a| a.id == builder)
            .unwrap()
            .status = AgentStatus::Finished;
        let out = crate::paths_util::with_places(places.clone(), || beat(&mut app));
        assert_eq!(launches(&out), 1, "{out:?}");
        assert!(sent_turn(&out).is_none(), "{out:?}");
        assert_eq!(app.autofix.queued.len(), 1);
        assert_eq!(app.autofix.queued[0].agent, None, "waits for the launch");
        assert_eq!(
            open_pr_row(&app, &ProjectId("p".into()), &pr()).state,
            "queued for autofix-7"
        );
        let out = crate::paths_util::with_places(places.clone(), || beat(&mut app));
        assert_eq!(launches(&out), 0, "never a second: {out:?}");

        // The fixer shows: the words wait out its turn, then go to it.
        let (fixer, _) = claude_on_branch(
            &mut app,
            &home,
            "autofix-7",
            AgentStatus::Running,
            &["Fixing."],
            10_000,
        );
        let out = crate::paths_util::with_places(places.clone(), || beat(&mut app));
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(app.autofix.queued[0].agent, Some(fixer.clone()));
        assert!(app.autofix.launches.is_empty(), "the launch landed");
        let row = app.tree.agents.iter_mut().find(|a| a.id == fixer).unwrap();
        row.status = AgentStatus::Finished;
        row.status_changed_at = 2;
        let out = crate::paths_util::with_places(places, || beat(&mut app));
        let (to, text) = sent_turn(&out).expect("a turn for the fixer");
        assert_eq!(to, fixer);
        assert!(text.contains("Rename the flag"), "{text}");
        assert_eq!(launches(&out), 0);
        assert!(app.autofix.queued.is_empty());
    }

    /// One body with the owner's new comment and a breakage: the comment
    /// launches the fresh fixer, and the breakage is held for it rather
    /// than launch a second — until the launch is given up on.
    #[test]
    fn a_comment_and_a_breakage_in_one_body_launch_one_fixer() {
        let mut app = app_with(vec![broken(false, true)], true);
        make_factory(&mut app);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                comments_seen: Some("2026-11-14T09:00:00Z".into()),
                ..Record::default()
            },
        );
        listed(&mut app);
        let page = PrDetail {
            author: "o".into(),
            comments: vec![crate::pull_request::PrComment {
                author: "o".into(),
                at: "2026-11-14T10:00:00Z".into(),
                review_state: String::new(),
                body: "Call it `beta_flag`".into(),
            }],
            ..detail(false, vec![check("unit", "CI", CheckState::Failed)])
        };
        let out = landed(&mut app, &page);
        assert_eq!(launches(&out), 1, "{out:?}");
        assert!(sent_prompt(&out).unwrap().contains("Call it `beta_flag`"));
        assert_eq!(app.autofix.queued.len(), 1);
        assert!(app.autofix.queued[0].work.is_fix());
        assert_eq!(app.autofix.queued[0].agent, None);

        let fixer = session_on_branch(&mut app, "autofix-7", AgentStatus::Fresh);
        let out = beat(&mut app);
        assert_eq!(launches(&out), 0, "{out:?}");
        assert_eq!(app.autofix.queued[0].agent, Some(fixer));

        // A launch that never shows is waited for only so long.
        let mut app = app_with(vec![broken(false, true)], true);
        let mut out = Vec::new();
        assert!(send_words(&mut app, &pr().url, "a".into(), "x", &mut out));
        assert_eq!(launches(&out), 1);
        assert!(router::launch_pending(&app, &pr().url));
        app.autofix.launches.get_mut(&pr().url).unwrap().at =
            Instant::now().checked_sub(router::LAUNCH_WAIT).unwrap();
        assert!(!router::launch_pending(&app, &pr().url));
    }

    /// With the blocking pool, an idle agent's context is read off the
    /// loop: the words wait for it, quietly, and are routed once it is in
    /// — here a session with no transcript to read, so a fresh fixer. The
    /// reading is kept: routing again does not read again.
    #[tokio::test]
    async fn the_context_is_read_off_the_loop_and_kept() {
        let mut app = app_with(vec![broken(false, false)], true);
        let id = session_on_branch(&mut app, "RIPLO-1", AgentStatus::Finished);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.view_jobs = Some(crate::view_jobs::Jobs::new(tx));
        let mut out = Vec::new();
        assert!(send_words(&mut app, &pr().url, "One".into(), "x", &mut out));
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(app.autofix.queued[0].agent, Some(id.clone()));
        assert!(app.flash.is_none(), "held quietly: {:?}", app.flash);
        assert!(beat(&mut app).is_empty(), "still measuring");

        match rx.recv().await {
            Some(crate::view_jobs::Answer::FixerContext { agent, used, said }) => {
                assert_eq!((agent.clone(), used), (id.clone(), None));
                router::land_context(&mut app, agent, used, said)
            }
            other => panic!("the context, not {other:?}"),
        }
        let project = ProjectId("p".into());
        assert_eq!(
            router::route(&app, &project, "feat", 7, &pr().url),
            router::Route::Fresh { from: Some(id) },
            "kept"
        );
        let out = beat(&mut app);
        assert_eq!(launches(&out), 1, "{out:?}");
        assert!(sent_prompt(&out).unwrap().contains("One"));
        assert!(app.autofix.measuring.is_empty());
    }

    // ---- archive on green, the owner's sessions aside ----

    /// A green fixer the owner is using — the pane on it, or typed into
    /// in the last 15 minutes — is not archived; once they have been away
    /// that long, it is.
    #[test]
    fn a_green_fixer_in_use_is_left_be() {
        let archived = |out: &[ClientRequest]| {
            out.iter()
                .any(|r| matches!(r, ClientRequest::ArchiveAgent { .. }))
        };
        let mut app = app_with(vec![broken(false, false)], true);
        let done = session_on_branch(&mut app, "autofix-7", AgentStatus::Finished);
        listed(&mut app);
        app.attached_sref = Some(SessionRef::Agent(done.clone()));
        assert!(!archived(&beat(&mut app)), "attached");
        app.attached_sref = None;
        app.typed_at
            .insert(done.clone(), Instant::now() - Duration::from_secs(5 * 60));
        assert!(!archived(&beat(&mut app)), "typed into lately");
        app.typed_at.insert(
            done,
            Instant::now()
                .checked_sub(IN_USE_FOR + Duration::from_secs(1))
                .unwrap(),
        );
        assert!(archived(&beat(&mut app)), "left alone long enough");
    }

    // ---- Can't fix, settled ----

    /// Raise *Can't fix* on a factory PR out of tries; its key.
    fn cant_fix_raised(app: &mut App) -> String {
        make_factory(app);
        app.autofix.ledger.insert(
            pr().url,
            Record {
                attempts: AUTO_ATTEMPTS,
                ..Record::default()
            },
        );
        listed(app);
        landed(
            app,
            &detail(false, vec![check("unit", "CI", CheckState::Failed)]),
        );
        let key = cant_fix_key(&pr().url);
        assert_eq!(
            card_state(app, &key),
            crate::factory::cards::CardState::Open
        );
        key
    }

    fn card_state(app: &App, key: &str) -> crate::factory::cards::CardState {
        app.factory_cards
            .iter()
            .find(|p| p.card.key == key)
            .expect("the card")
            .state
            .clone()
    }

    /// *Can't fix* closes on its own once its pull request goes green,
    /// merges or closes — not only on Park or Reply….
    #[test]
    fn cant_fix_closes_when_the_pr_goes_green_or_merges() {
        use crate::factory::cards::CardState;
        let mut app = app_with(vec![broken(false, true)], true);
        let key = cant_fix_raised(&mut app);
        set_list(&mut app, vec![broken(false, false)]);
        listed(&mut app);
        assert_eq!(
            card_state(&app, &key),
            CardState::Closed("Green now".into())
        );

        let mut app = app_with(vec![broken(false, true)], true);
        let key = cant_fix_raised(&mut app);
        app.prs.observe(
            &pr().url,
            crate::pr_store::PrObservation::of_tail_row(&pr()),
            crate::fetch::Asked::At(crate::fetch::now()),
        );
        set_list(&mut app, Vec::new());
        listed(&mut app);
        assert_eq!(card_state(&app, &key), CardState::Closed("Merged".into()));
    }
}
