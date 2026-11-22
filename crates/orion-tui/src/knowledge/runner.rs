//! The TUI's side of knowledge: the session snapshot kept as the tree
//! changes, harvests queued at merge and run one at a time in the
//! background, the knowledge pull request merged once the owner's rule
//! review signed off everything on it, and the cards — the one-time
//! "ignores `.orion/decisions/`" card, a merge that failed twice, and a
//! backfill's summary.
//!
//! Nothing here blocks the loop: a harvest, a sign-off and a merge each run
//! on a task of their own, their git and file work on blocking threads.
//! Each project's queue and state are kept in its folder under the data
//! dir (`super::local`), so a harvest cut off by a quit runs on the next
//! start. What the background wants posted is left in the state's
//! `notices`, which [`tick`] posts.
//!
//! **The review gate.** A harvest commits on this machine and pushes
//! nothing. At 09:00 (or the first launch after) the RULE REVIEW opens
//! over orion with every rule waiting (`super::review`, `crate::rule_review`);
//! its sign-off pushes the branch and opens or updates the pull request.
//! With `factory.knowledge_auto_merge` on (the default), orion then merges
//! it itself ([`merge_due`], `Sharer::merge`): only when the review signed
//! off everything on it, it is mergeable, no check fails, and its head is
//! what the sign-off pushed. A check still running waits for a later tick;
//! a conflict is rebuilt and pushed (when nothing unsigned rode in); a
//! refusal is tried once more, then said on a card. Never forced.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use orion_core::WorktreeId;
use serde::{Deserialize, Serialize};

use super::consolidate;
use super::harvest::{self, ClaudeCli, Outcome, Session, Target};
use super::local::Local;
use super::review::{self, Candidate, DocsPr};
use super::share::{self, GhCli, Merge, Pr, Published, Sharer, SystemGit};
use super::snapshot::{self, Snapshot};
use super::store;
use super::{backfill, transcripts};
use super::{notes, records};
use crate::app::App;
use crate::factory::cards::{self, Button, Card, Press, Recipient};

/// How often [`tick`] does its work.
const EVERY: Duration = Duration::from_secs(30);
/// How early a run may come and still count as the beat: the `Knowledge`
/// chore is due [`EVERY`] after it started, a hair before this beat's
/// stamp is.
const BEAT_SLACK: Duration = Duration::from_secs(1);
/// How many times a merge that is refused is tried before a card says so.
const MERGE_TRIES: u32 = 2;
/// How often orion looks for topics due a staleness check. Cheap: git
/// alone, unless a topic nobody checked this week had its files change
/// (`harvest::STALE_AFTER_DAYS`).
const STALE_LOOK_MS: i64 = orion_core::clock::DAY_MS;
/// How many times a harvest is tried.
const ATTEMPTS: u32 = 2;
/// Where the queue of every project was kept before each had its own.
const OLD_QUEUE_FILE: &str = "knowledge-queue.json";

static GIT: SystemGit = SystemGit;
static GH: GhCli = GhCli;

/// One merged pull request waiting to be harvested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Job {
    /// The project's id.
    #[serde(default)]
    pub id: String,
    /// Its main checkout.
    pub root: PathBuf,
    pub project: String,
    pub number: u64,
    pub url: String,
    pub branch: String,
    #[serde(default)]
    pub attempts: u32,
    /// The factory run the branch was cut for (`RIPLO-123`), whose record
    /// gets the owner's corrections.
    #[serde(default)]
    pub run: Option<String>,
}

/// Something the background (or the CLI) wants a card for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Notice {
    /// The repo ignores `.orion/decisions/`.
    Gitignore,
    /// A backfill finished.
    Backfilled { lines: Vec<String> },
    /// The knowledge pull request was refused a merge twice.
    CantMerge { pr: Pr, why: String },
}

/// What a project's cards and pull request remember between runs:
/// `<data_dir>/knowledge/<project id>/state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct State {
    /// The entries a rule review signed off (or a backfill recorded),
    /// pruned to the ones still only on the branch.
    pub signed: BTreeSet<String>,
    /// The head the last sign-off pushed: the merge goes only while
    /// `pushed` is this one.
    pub signed_head: String,
    /// The local date (`YYYY-MM-DD`) the review last opened on its own.
    pub review_opened_on: String,
    /// The promotions into the agent docs the next review offers.
    pub promote: Vec<Candidate>,
    /// Epoch ms of the next promotion scan; 0 until first seen.
    pub promote_next: i64,
    /// Promotions the owner turned down, by their first entry.
    pub promote_declined: BTreeSet<String>,
    /// The agent docs pull request a sign-off opened, until it merges.
    pub docs_pr: Option<DocsPr>,
    /// Whether the "ignores `.orion/decisions/`" card went.
    pub asked_gitignore: bool,
    pub notices: Vec<Notice>,
    /// The open knowledge pull request.
    pub pr: Option<Pr>,
    /// The commit orion last pushed to its knowledge branch.
    pub pushed: String,
    /// Merges refused since the last sign-off.
    pub merge_failures: u32,
    /// Epoch ms the next look for topics due a staleness check is; 0
    /// until first seen. When each topic was checked is kept in the repo
    /// (`checked.jsonl`), shared by everyone's orion.
    pub stale_next: i64,
}

/// The state files' lock within this process.
static STATE_LOCK: Mutex<()> = Mutex::new(());

impl State {
    fn load(local: &Local) -> State {
        read_json(&local.state_path())
    }

    /// Change `local`'s state on disk under the lock.
    pub(crate) fn update<T>(local: &Local, f: impl FnOnce(&mut State) -> T) -> T {
        let _held = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = State::load(local);
        let mut state = before.clone();
        let out = f(&mut state);
        if state != before {
            write_json(&local.state_path(), &state);
        }
        out
    }
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) {
    let _ = crate::pr_cache::write_json_atomic(path, value);
}

/// Leave a notice for the TUI to post as a card — from the background, or
/// from the CLI with no TUI running.
pub(crate) fn notify(local: &Local, notice: Notice) {
    State::update(local, |s| {
        if !s.notices.contains(&notice) {
            s.notices.push(notice);
        }
    });
}

/// Remember where a publish left the knowledge pull request: the one
/// open and the head pushed — or none, the branch having nothing the
/// base hasn't.
pub(crate) fn note_published(local: &Local, published: &Published) {
    State::update(local, |s| match &published.pr {
        Some(pr) => {
            s.pr = Some(pr.clone());
            s.pushed = published.head.clone();
        }
        None => {
            s.pr = None;
            s.pushed.clear();
            s.signed_head.clear();
        }
    });
}

// ---- the queue ----

static QUEUE_LOCK: Mutex<()> = Mutex::new(());

/// Change `local`'s queue on disk under the lock; written only when `f`
/// changed it.
fn with_queue<T>(local: &Local, f: impl FnOnce(&mut Vec<Job>) -> T) -> T {
    let _held = QUEUE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before: Vec<Job> = read_json(&local.queue_path());
    let mut jobs = before.clone();
    let out = f(&mut jobs);
    if jobs != before {
        write_json(&local.queue_path(), &jobs);
    }
    out
}

/// The worker's sender, the worker started on first use. None outside a
/// tokio runtime.
fn worker() -> Option<&'static tokio::sync::mpsc::UnboundedSender<Job>> {
    static TX: OnceLock<tokio::sync::mpsc::UnboundedSender<Job>> = OnceLock::new();
    if let Some(tx) = TX.get() {
        return Some(tx);
    }
    let handle = tokio::runtime::Handle::try_current().ok()?;
    Some(TX.get_or_init(|| {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Job>();
        handle.spawn(async move {
            while let Some(job) = rx.recv().await {
                work(job).await;
            }
        });
        tx
    }))
}

/// Queue `job`: kept on disk until it has run, and handed to the worker.
fn enqueue(job: Job) {
    let fresh = with_queue(&Local::of(&job.id), |jobs| {
        if jobs.iter().any(|j| j.url == job.url) {
            return false;
        }
        jobs.push(job.clone());
        true
    });
    if fresh {
        if let Some(tx) = worker() {
            let _ = tx.send(job);
        }
    }
}

/// Run one job, and take it off the queue — or put it back for one more
/// try when it failed. One refused at a usage limit stays queued as it
/// was, its tries untouched, and goes again once the factory's pause
/// lifts (`automations::limit`).
async fn work(job: Job) {
    let result = run_job(&job).await;
    if let Err(why) = &result {
        if crate::automations::limit::is_limit(why) {
            tracing::warn!(
                "knowledge: harvest of #{} of {} waits for the limit to reset: {why}",
                job.number,
                job.project
            );
            hold_for_reset(job);
            return;
        }
        // GitHub's rate limit is no fault of the job: parked, its tries
        // untouched, until GitHub's pause lifts (`sync::gate`).
        if rate_limited(why) {
            tracing::warn!(
                "knowledge: harvest of #{} of {} waits for GitHub's limit to reset",
                job.number,
                job.project
            );
            hold_for_github(job);
            return;
        }
    }
    if matches!(result, Ok(Some(_))) {
        // A harvest committed: the rule review has more to show.
        review::want_refresh();
    }
    let retry = match &result {
        Ok(Some(o)) => {
            tracing::info!(
                "knowledge: harvested #{} of {}: {} kept, {} dropped, topics {:?}",
                o.pr,
                job.project,
                o.kept,
                o.dropped,
                o.topics
            );
            None
        }
        Ok(None) => None,
        Err(why) => {
            tracing::warn!(
                "knowledge: harvest of #{} of {} failed: {why}",
                job.number,
                job.project
            );
            (job.attempts + 1 < ATTEMPTS).then(|| Job {
                attempts: job.attempts + 1,
                ..job.clone()
            })
        }
    };
    with_queue(&Local::of(&job.id), |jobs| {
        jobs.retain(|j| j.url != job.url);
        if let Some(again) = &retry {
            jobs.push(again.clone());
        }
    });
    if let (Some(again), Some(tx)) = (retry, worker()) {
        let _ = tx.send(again);
    }
}

/// Hand `job` — still on its queue — back to the worker once the factory's
/// pause lifts, or after [`crate::automations::limit::UNREAD_RESET`] when
/// none is kept.
fn hold_for_reset(job: Job) {
    let now = chrono::Local::now();
    let until = crate::automations::limit::current_at(now)
        .map(|p| p.until_ms)
        .unwrap_or_else(|| (now + crate::automations::limit::UNREAD_RESET).timestamp_millis());
    let wait = u64::try_from(until - now.timestamp_millis()).unwrap_or(0);
    let Some(tx) = worker() else {
        return;
    };
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        let _ = tx.send(job);
    });
}

/// Whether `why` — a step's failure — is GitHub's rate limit: a `gh` that
/// said so, or THE GATE refusing to ask while GitHub is paused.
pub(crate) fn rate_limited(why: &str) -> bool {
    crate::sync::gate::classify(why) == crate::sync::gate::Signal::RateLimit
}

/// Hand `job` — still on its queue — back to the worker once GitHub's
/// rate-limit pause lifts (`sync::gate`), or after the gate's
/// `UNKNOWN_PAUSE` when none is known.
fn hold_for_github(job: Job) {
    let wait = crate::sync::gate::github_paused_until()
        .map(|until| until.saturating_duration_since(Instant::now()))
        .unwrap_or(crate::sync::gate::UNKNOWN_PAUSE);
    let Some(tx) = worker() else {
        return;
    };
    tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        let _ = tx.send(job);
    });
}

/// The `gh` every knowledge step runs.
pub(super) fn gh() -> &'static GhCli {
    &GH
}

/// The signed-in GitHub login.
pub(super) async fn gh_login() -> Option<String> {
    use share::Gh as _;
    GH.login().await
}

/// The project at `root` (`local`'s), as orion writes its knowledge, on
/// the base [`share::base_of`] finds.
pub(super) async fn sharer(
    local: &Local,
    root: &Path,
    project: &str,
) -> Result<Sharer<'static>, String> {
    let base = share::base_of(root, &GH).await;
    Ok(Sharer {
        git: &GIT,
        gh: &GH,
        main: root.to_path_buf(),
        local: local.clone(),
        project: project.to_string(),
        base,
    })
}

/// Harvest `job`'s pull request into the knowledge branch, committed on
/// this machine for the next rule review. None when it was harvested before
/// or the repo ignores `.orion/decisions/`.
async fn run_job(job: &Job) -> Result<Option<Outcome>, String> {
    let model = ClaudeCli::from_config()?;
    let local = Local::of(&job.id);
    let root = job.root.clone();
    let ignored = tokio::task::spawn_blocking(move || store::ignored(&root))
        .await
        .unwrap_or(true);
    if ignored {
        notify(&local, Notice::Gitignore);
        tracing::warn!(
            "knowledge: {} ignores .orion/decisions/, so #{} isn't harvested",
            job.project,
            job.number
        );
        return Ok(None);
    }
    let share = sharer(&local, &job.root, &job.project).await?;
    let branch = share.open().await?;
    let store = branch.store();
    let records = harvest::with_lessons(records::for_check_in(&branch.clone), &store);
    let to = Target {
        store: &store,
        local: &local,
        project: &job.project,
        records: &records,
    };
    // The session's notes, once recorded and pushed, are done with.
    let mut noted_used = false;
    let outcome = match to.harvested(job.number) {
        true => None,
        false => {
            let (pr, comments) = harvest::gather_pr(&GH, &job.root, job.number).await?;
            let for_sessions = job.clone();
            let sessions = tokio::task::spawn_blocking(move || sessions_of(&for_sessions))
                .await
                .map_err(|e| e.to_string())?;
            if let Some(run) = &job.run {
                record_corrections(run, harvest::corrections(&sessions));
            }
            // A factory session that noted its rules: no extraction.
            let noted = notes::read_for(&notes::dir(), &job.url);
            let from_notes = noted.is_some();
            let (outcome, unchecked) =
                harvest::harvest_merge(&model, &to, &job.root, &pr, &comments, &sessions, noted)
                    .await;
            if let Some(why) = unchecked {
                tracing::warn!(
                    "knowledge: #{} of {} is recorded unchecked: {why}",
                    job.number,
                    job.project
                );
            }
            noted_used = from_notes;
            Some(outcome?)
        }
    };
    let mut message = format!("knowledge: harvest #{}", job.number);
    // Past the cap: the harvest committed alone, then the topics
    // consolidated in a commit of their own.
    if outcome.is_some() && consolidate::over_cap(&store) {
        share.commit(&message).await?;
        match consolidate::consolidate(&model, &to).await {
            Ok(done) => {
                tracing::info!("knowledge: {} — {}", job.project, done.line());
                message = consolidate::MESSAGE.to_string();
            }
            Err(why) => tracing::warn!("knowledge: consolidating {} failed: {why}", job.project),
        }
    }
    // Committed here only: the owner's next sign-off pushes it.
    share.save(&branch, &message).await?;
    drop(branch);
    if noted_used {
        notes::remove_for(&notes::dir(), &job.url);
    }
    Ok(outcome)
}

/// The owner's `corrections` on run `run`, into its record — in place
/// of any written before, so a harvest tried again writes them once.
fn record_corrections(run: &str, corrections: Vec<String>) {
    if corrections.is_empty() {
        return;
    }
    crate::factory::runs::update(run, |r| r.corrections = corrections);
}

/// The condensed sessions that worked on `job`'s branch: the snapshot's
/// Claude sessions found by id; with none, any Claude transcript of the
/// project that ran on the branch.
fn sessions_of(job: &Job) -> Vec<Session> {
    let roots = transcripts::claude_roots();
    let mut found = Vec::new();
    let snap = Snapshot::load(&Local::of(&job.id), &job.root);
    for s in snap.sessions_on(&job.branch) {
        match s.harness.as_str() {
            "claude" | "custom" => match transcripts::find_claude(&roots, &s.id) {
                Some(path) => found.push(transcripts::Found {
                    shape: super::condense::Shape::Claude,
                    path,
                }),
                None => tracing::info!("knowledge: no transcript for session {} of #{}", s.id, job.number),
            },
            other => tracing::info!(
                "knowledge: #{}'s {other} session {} is left out (only Claude transcripts are harvested at merge)",
                job.number,
                s.id
            ),
        }
    }
    if found.is_empty() && !backfill::bases(&job.root).contains(&job.branch) {
        let folder = orion_core::Project::folder_name(&job.root);
        found = transcripts::list(&roots, None, &folder)
            .into_iter()
            .filter(|f| transcripts::scan(f, "").branches.contains(&job.branch))
            .collect();
    }
    found.sort();
    found.dedup();
    found.iter().filter_map(backfill::session_of).collect()
}

// ---- hooks the app calls ----

/// `App::note_merge_landed`: `worktree`'s pull request just merged.
/// Note who sat on its branch, and queue its harvest.
pub(crate) fn merge_landed(app: &App, worktree: &WorktreeId) {
    if crate::oneshot::offline() {
        return;
    }
    let Some(wt) = app.tree.worktrees.iter().find(|w| &w.id == worktree) else {
        return;
    };
    let Some(project) = app.tree.projects.iter().find(|p| p.id == wt.project_id) else {
        return;
    };
    let Some(Some(pr)) = app.pull_requests.get(worktree) else {
        return;
    };
    // orion's own knowledge pull request records nothing.
    if wt.branch.starts_with(share::PREFIX) {
        return;
    }
    note_sessions(app);
    enqueue(Job {
        id: project.id.to_string(),
        root: project.repo_path.clone(),
        project: project.name.clone(),
        number: pr.number,
        url: pr.url.clone(),
        branch: wt.branch.clone(),
        attempts: 0,
        run: crate::automations::linear_builder::factory_entry(app, &wt.project_id, &wt.branch)
            .map(|e| e.identifier.clone()),
    });
}

/// Keep each project's session snapshot up to date with the tree: read
/// from disk once, then held here and written when it changes.
fn note_sessions(app: &App) {
    static HELD: Mutex<BTreeMap<String, Snapshot>> = Mutex::new(BTreeMap::new());
    let now = orion_core::clock::now_ms();
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    for seen in snapshot::sightings(&app.tree, now) {
        let local = Local::of(&seen.id);
        let snap = held
            .entry(seen.id.clone())
            .or_insert_with(|| Snapshot::load(&local, &seen.root));
        if snap.note(&seen.name, &seen.root, &seen.sightings, now) {
            if let Err(why) = snap.save_to(&local.snapshot_path()) {
                tracing::warn!("knowledge: couldn't save the session snapshot: {why}");
            }
        }
    }
}

/// Whether the daily look for stale topics is due at `now_ms`, moving the
/// next one on a day when it is. The first sight of a project starts its
/// day.
pub(crate) fn stale_due(state: &mut State, now_ms: i64) -> bool {
    cadence_due(&mut state.stale_next, now_ms, STALE_LOOK_MS)
}

/// Whether a step that runs every `every_ms` is due at `now_ms`, moving
/// `next` on when it is. The first sight (`next` still 0) starts the
/// period and runs nothing.
pub(super) fn cadence_due(next: &mut i64, now_ms: i64, every_ms: i64) -> bool {
    if *next == 0 {
        *next = now_ms + every_ms;
        return false;
    }
    if now_ms < *next {
        return false;
    }
    *next = now_ms + every_ms;
    true
}

/// The keys of the background steps on their way — a project's merge,
/// staleness pass, sign-off … — so a tick starts no second one.
pub(super) struct InFlight(Mutex<BTreeSet<String>>);

/// A key held in an [`InFlight`] set; dropping it — the task done, or
/// panicked — lets the next one start.
pub(super) struct Held {
    set: &'static InFlight,
    key: String,
}

impl InFlight {
    pub(super) const fn new() -> Self {
        InFlight(Mutex::new(BTreeSet::new()))
    }

    /// Hold `key`: None while it is held already.
    pub(super) fn take(&'static self, key: &str) -> Option<Held> {
        let fresh = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string());
        fresh.then(|| Held {
            set: self,
            key: key.to_string(),
        })
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.set
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

/// Spawn `work` on the runtime unless `key` is already in `set`, the key
/// held until it finishes.
pub(super) fn spawn_once<F>(set: &'static InFlight, key: &str, work: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(held) = set.take(key) else {
        return;
    };
    handle.spawn(async move {
        work.await;
        drop(held);
    });
}

/// The projects with a staleness pass on its way.
static STALING: InFlight = InFlight::new();

/// Run the staleness pass for the project at `root` off the loop: one at
/// a time a project, and never beside its harvests (the branch's lock).
fn start_staleness(local: Local, root: PathBuf, project: String) {
    let key = local.id.clone();
    spawn_once(&STALING, &key, async move {
        match run_staleness(&local, &root, &project).await {
            Ok(changed) => tracing::info!(
                "knowledge: staleness pass of {project} changed {:?}",
                changed
            ),
            Err(why) => tracing::warn!("knowledge: staleness pass of {project} failed: {why}"),
        }
    });
}

/// The staleness pass: the topics nobody checked this week whose files
/// changed since their last check, checked again against the base as
/// origin has it (`harvest::staleness`). Who checked what is read from
/// the branch (the base's `checked.jsonl` and this person's) and from
/// everyone else's knowledge branch, so one person's orion makes each
/// week's check and the rest find nothing due. A check that asked the
/// model is committed, its dates with it, whether or not a page changed,
/// and goes up with the next push.
async fn run_staleness(local: &Local, root: &Path, project: &str) -> Result<Vec<String>, String> {
    let model = ClaudeCli::from_config()?;
    let at = root.to_path_buf();
    let ignored = tokio::task::spawn_blocking(move || store::ignored(&at))
        .await
        .unwrap_or(true);
    if ignored {
        return Ok(Vec::new());
    }
    let share = sharer(local, root, project).await?;
    let tree = harvest::CheckTree::of_base(root, &share.base).await?;
    let result = async {
        let branch = share.open().await?;
        let store = branch.store();
        let records = harvest::with_lessons(records::for_check_in(&branch.clone), &store);
        let to = Target {
            store: &store,
            local,
            project,
            records: &records,
        };
        let mut checked = store.checked();
        store::merge_checked(
            &mut checked,
            share.others_checked(&branch).await.into_values(),
        );
        let today = chrono::Local::now().date_naive();
        let out = harvest::staleness(&model, &to, &tree.path, &mut checked, today).await?;
        if out.asked {
            store.save_checked(&checked)?;
            share.save(&branch, "knowledge: staleness check").await?;
        }
        Ok(out.changed)
    }
    .await;
    tree.remove().await;
    result
}

/// The merges on their way — a project's knowledge pull request, or its
/// agent docs one — so a tick starts no second one.
pub(super) static MERGING: InFlight = InFlight::new();

/// The loop's beat: the snapshot, the queue left from a last run, the
/// rule review's pending list, and for each project the cards the
/// background asked for, the morning's review, the merge once the review
/// signed it off, the staleness pass and the promotion scan.
pub(crate) fn tick(app: &mut App) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    if crate::oneshot::offline() {
        return;
    }
    // What the last refresh found and what the background said land at
    // once, not on the next beat: either wakes this tick
    // (`chores::wake`).
    if let Some(found) = review::take_refreshed() {
        crate::rule_review::refreshed(app, found);
    }
    review::post_said(app);
    crate::rule_review::maybe_open(app);
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() + BEAT_SLACK < EVERY) {
            return;
        }
        let first = last.is_none();
        *last = Some(Instant::now());
        if first {
            resume(app);
        }
    }
    note_sessions(app);
    let now = chrono::Local::now();
    let now_ms = now.timestamp_millis();
    let today = now.format("%Y-%m-%d").to_string();
    let auto = app.pr_body.settings.knowledge_auto_merge;
    let projects: Vec<(String, String, PathBuf)> = app
        .tree
        .projects
        .iter()
        .map(|p| (p.id.to_string(), p.name.clone(), p.repo_path.clone()))
        .collect();
    if review::refresh_due(Instant::now()) {
        review::refresh(projects.clone());
    }
    let cutoff = review::cutoff(now);
    let morning = review::past_review_time(now) && app.rule_review.loaded;
    let opened_today = app.rule_review.opened_on == today;
    for (id, name, root) in projects {
        let local = Local::of(&id);
        let held = app.rule_review.pending.iter().find(|p| p.id == id);
        let pending_any = held.is_some_and(|p| !p.rules.is_empty());
        let due = held.is_some_and(|p| p.due(cutoff));
        // The notices to post, the `.gitignore` card once, the morning's
        // review, and what is due: one read and write of the state.
        let (notices, nine, merge, stale, promote, docs) = State::update(&local, |s| {
            let notices: Vec<Notice> = std::mem::take(&mut s.notices)
                .into_iter()
                .filter(|n| match n {
                    Notice::Gitignore => !std::mem::replace(&mut s.asked_gitignore, true),
                    _ => true,
                })
                .collect();
            let nine = morning && s.review_opened_on != today;
            if nine {
                s.review_opened_on = today.clone();
            }
            let merge = merge_due(s, auto).then(|| (s.pr.clone(), s.pushed.clone()));
            let stale = stale_due(s, now_ms);
            let promote = review::promote_due(s, now_ms);
            let docs = s.docs_pr.clone();
            (notices, nine, merge, stale, promote, docs)
        });
        for notice in notices {
            post_notice(app, &id, &name, notice);
        }
        if nine {
            match (due, pending_any) {
                // Once a day: a launch that opened it already counts.
                (true, _) if opened_today => {}
                (true, _) => {
                    app.rule_review.want_open = true;
                    app.rule_review.opened_on = today.clone();
                }
                // Nothing to review: what was committed since the last
                // sign-off goes up on its own (checked again, held).
                (false, false) => {
                    review::push_if_waiting(local.clone(), root.clone(), name.clone())
                }
                (false, true) => {}
            }
        }
        if let Some((Some(pr), pushed)) = merge {
            if readiness_due(app, &pr.url, Instant::now()) {
                start_merge(local.clone(), root.clone(), name.clone(), pr, pushed);
            }
        }
        if stale {
            start_staleness(local.clone(), root.clone(), name.clone());
        }
        if promote {
            review::start_promote_scan(local.clone(), root.clone(), name.clone());
        }
        if let Some(docs) = docs.filter(|d| readiness_due(app, &d.pr.url, Instant::now())) {
            review::start_docs_merge(local, root, name, docs, auto);
        }
    }
    crate::rule_review::maybe_open(app);
}

/// When each knowledge or agent docs pull request (by URL) was last asked
/// about with `gh pr view` ([`share::Sharer::readiness_of`]), what its row
/// said then, and how long to wait before asking again with no row.
static READINESS: Mutex<BTreeMap<String, Readiness>> = Mutex::new(BTreeMap::new());

#[derive(Debug, Clone)]
struct Readiness {
    row: Option<String>,
    at: Instant,
    gap: Duration,
}

/// The first wait between asks about a knowledge pull request no row
/// has, doubling to [`READINESS_CAP`].
const READINESS_GAP: Duration = Duration::from_secs(5 * 60);
const READINESS_CAP: Duration = Duration::from_secs(30 * 60);

/// What `url`'s row says of whether it may merge — its state, conflicts,
/// checks and head as `App::prs` has them, from a project's open list or
/// the automations' `MyPrs` (it is your own pull request). None when no
/// row has it.
fn readiness_row(app: &App, url: &str) -> Option<String> {
    let s = app.prs.status(url)?;
    Some(format!(
        "{:?}|{:?}{}|{:?}{}|{}",
        s.standing,
        s.health.conflicts,
        s.conflicts_known,
        s.health.checks,
        s.checks_known,
        s.head_sha
    ))
}

/// Whether to ask GitHub about knowledge pull request `url` now — what a
/// merge (or noticing one) starts with. It used to be asked every 30 s
/// until a person merged it, rate limit or not. Now: when its row has
/// moved since the last ask (the rows are read anyway); else every
/// [`READINESS_CAP`] as a backstop; and with no row at all, after
/// [`READINESS_GAP`], doubling.
fn readiness_due(app: &App, url: &str, now: Instant) -> bool {
    let row = readiness_row(app, url);
    let mut asked = READINESS.lock().unwrap_or_else(|e| e.into_inner());
    let due = match asked.get(url) {
        None => true,
        Some(last) if row.is_some() && last.row != row => true,
        Some(last) => now.saturating_duration_since(last.at) >= last.gap,
    };
    if due {
        let gap = match (&row, asked.get(url)) {
            (Some(_), _) => READINESS_CAP,
            (None, Some(last)) if last.row.is_none() => (last.gap * 2).min(READINESS_CAP),
            (None, _) => READINESS_GAP,
        };
        asked.insert(url.to_string(), Readiness { row, at: now, gap });
    }
    due
}

/// Whether the knowledge pull request should be merged now: the switch
/// on, a pull request orion pushed to, and its head the one the last
/// sign-off pushed — which held only signed-off rules. Harvests committed
/// since wait here, unpushed, and don't hold it back.
pub(crate) fn merge_due(state: &State, auto: bool) -> bool {
    auto && state.pr.is_some() && !state.pushed.is_empty() && state.pushed == state.signed_head
}

/// Merge the knowledge pull request, off the loop: once at a time a
/// project.
fn start_merge(local: Local, root: PathBuf, project: String, pr: Pr, pushed: String) {
    let key = local.id.clone();
    spawn_once(&MERGING, &key, async move {
        let wanted = crate::config::Config::load().merge_method(&root);
        let result = match sharer(&local, &root, &project).await {
            Ok(share) => {
                let merged = share.merge(&pr, &pushed, wanted).await;
                if let Ok(Merge::Conflicting) = &merged {
                    // The base moved under it: rebased — rebuilt, when
                    // that conflicts — and pushed for the next tick, when
                    // everything on it is still signed off; else it waits
                    // for the next review.
                    let rebuilt = async {
                        let branch = share.open().await?;
                        let message = format!("knowledge: rebuilt on {}", share.base);
                        if !review::publish_if_all_signed(&local, &share, &branch, &message).await?
                        {
                            State::update(&local, |s| s.signed_head.clear());
                        }
                        Ok::<_, String>(())
                    }
                    .await;
                    if let Err(why) = rebuilt {
                        tracing::warn!("knowledge: rebuilding #{} failed: {why}", pr.number);
                    }
                }
                merged
            }
            Err(why) => Err(why),
        };
        merge_result(&local, &project, &pr, result);
    });
}

/// What a merge attempt leaves in the state: a merged or closed pull
/// request forgotten; a refusal counted, and on the second a card, the
/// merge held until the next sign-off.
pub(crate) fn merge_result(local: &Local, project: &str, pr: &Pr, result: Result<Merge, String>) {
    match result {
        Ok(Merge::Merged) | Ok(Merge::Closed) => {
            tracing::info!(
                "knowledge: #{} of {project} is done with: {result:?}",
                pr.number
            );
            State::update(local, |s| {
                s.pr = None;
                s.pushed.clear();
                s.signed_head.clear();
                s.merge_failures = 0;
            });
        }
        Ok(Merge::Waiting(why)) => {
            tracing::info!("knowledge: #{} of {project} waits: {why}", pr.number);
        }
        Ok(Merge::Conflicting) => {}
        // GitHub's limit is no refusal of the merge: it goes again once
        // the limit resets, and never counts toward the card.
        Err(why) if rate_limited(&why) => {
            tracing::info!(
                "knowledge: #{} of {project} waits for GitHub's limit",
                pr.number
            );
        }
        Err(why) => {
            tracing::warn!("knowledge: #{} of {project} can't merge: {why}", pr.number);
            State::update(local, |s| {
                s.merge_failures += 1;
                if s.merge_failures >= MERGE_TRIES {
                    s.signed_head.clear();
                    s.merge_failures = 0;
                    s.notices.push(Notice::CantMerge {
                        pr: pr.clone(),
                        why,
                    });
                }
            });
        }
    }
}

/// Hand the worker every job a last run left queued — the old shared
/// queue's moved into each project's first.
fn resume(app: &App) {
    let old = crate::automations::state_dir().join(OLD_QUEUE_FILE);
    let moved: Vec<Job> = read_json(&old);
    for job in moved {
        let project = app.tree.projects.iter().find(|p| p.repo_path == job.root);
        if let Some(project) = project {
            let job = Job {
                id: project.id.to_string(),
                ..job
            };
            with_queue(&Local::of(&job.id), |jobs| {
                if !jobs.iter().any(|j| j.url == job.url) {
                    jobs.push(job);
                }
            });
        }
    }
    let _ = std::fs::remove_file(&old);
    let Some(tx) = worker() else {
        return;
    };
    for local in Local::all() {
        for job in with_queue(&local, |jobs| jobs.clone()) {
            let _ = tx.send(job);
        }
    }
}

/// A notice as its card; [`tick`] has kept the `.gitignore` card to once.
fn post_notice(app: &mut App, id: &str, project: &str, notice: Notice) {
    let card = match notice {
        Notice::Gitignore => gitignore_card(id, project),
        Notice::Backfilled { lines } => Card {
            key: format!("kb:backfill:{id}"),
            to: Recipient::Owner,
            context: format!("Knowledge · {project}"),
            title: format!("Knowledge backfill of {project} is done"),
            body: lines,
            buttons: vec![Button::tap("ok", "OK")],
            image: None,
        },
        Notice::CantMerge { pr, why } => cant_merge_card(id, project, &pr, &why),
    };
    cards::post(app, card);
}

pub(crate) fn gitignore_card(id: &str, project: &str) -> Card {
    Card {
        key: format!("kb:gitignore:{id}"),
        to: Recipient::Owner,
        context: format!("Knowledge · {project}"),
        title: format!("{project} ignores .orion/decisions/, so harvested rules can't be shared"),
        body: vec![format!(
            "orion proposes the rules it harvests at merge in a pull request, under `{}/`. While {project}'s `.gitignore` covers `.orion/decisions/`, nothing is harvested: take it out to share them.",
            store::DIR
        )],
        buttons: vec![Button::tap("ok", "OK")],
        image: None,
    }
}

fn cant_merge_card(id: &str, project: &str, pr: &Pr, why: &str) -> Card {
    Card {
        key: format!("kb:merge:{id}:{}", pr.number),
        to: Recipient::Owner,
        context: format!("Knowledge · {project}"),
        title: format!("Knowledge PR #{} can't merge: {why}", pr.number),
        body: vec![
            "orion tried twice and stopped until your next rule review signs off. Merge it yourself once it's fixed, or leave it for then.".into(),
        ],
        buttons: vec![Button::link("Open PR", &pr.url), Button::tap("ok", "OK")],
        image: None,
    }
}

/// A press on a `kb:` card: every one is an OK. A daily card an older
/// orion posted closes as no longer needed (`factory::cards::reconcile`).
pub(crate) fn on_action(app: &mut App, press: Press) {
    let key = press.key.to_string();
    cards::close(app, &key, "OK");
}

/// `root`'s git common dir, asked of git once a checkout.
pub(super) fn common_dir(root: &Path) -> Result<PathBuf, String> {
    static KNOWN: Mutex<BTreeMap<PathBuf, PathBuf>> = Mutex::new(BTreeMap::new());
    if let Some(dir) = KNOWN.lock().unwrap_or_else(|e| e.into_inner()).get(root) {
        return Ok(dir.clone());
    }
    let common = crate::git_proc::read(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let dir = PathBuf::from(common.trim());
    KNOWN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(root.to_path_buf(), dir.clone());
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> Pr {
        Pr {
            number: 77,
            url: "https://github.com/o/app/pull/77".into(),
        }
    }

    #[test]
    fn the_merge_waits_for_a_sign_off_and_the_switch() {
        let mut state = State {
            pr: Some(pr()),
            pushed: "abc".into(),
            ..State::default()
        };
        assert!(!merge_due(&state, true), "nothing signed off yet");
        state.signed_head = "abc".into();
        assert!(merge_due(&state, true), "the sign-off pushed this head");
        assert!(!merge_due(&state, false), "switched off");
        let later = State {
            pushed: "def".into(),
            ..state.clone()
        };
        assert!(!merge_due(&later, true), "pushed since without a sign-off");
        let unpushed = State {
            pushed: String::new(),
            signed_head: String::new(),
            ..state.clone()
        };
        assert!(!merge_due(&unpushed, true));
    }

    /// GitHub's rate limit is never a refusal of the merge: however often
    /// it says so, nothing is counted and no card goes.
    #[test]
    fn a_rate_limit_is_no_merge_failure() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::at("01R", dir.path());
        State::update(&local, |s| {
            s.pr = Some(pr());
            s.pushed = "abc".into();
            s.signed_head = "abc".into();
        });
        for why in [
            "GitHub rate limit: paused until it resets",
            "GraphQL: API rate limit exceeded for user ID 1.",
        ] {
            assert!(rate_limited(why));
            merge_result(&local, "app", &pr(), Err(why.into()));
        }
        let s = State::load(&local);
        assert_eq!(s.merge_failures, 0);
        assert_eq!(s.signed_head, "abc", "still due once the limit resets");
        assert!(s.notices.is_empty());
        assert!(!rate_limited("a check is failing"));
    }

    /// A knowledge pull request is asked about when its row moves, and
    /// otherwise on a slow backstop — never on the 30 s beat; one no row
    /// has, five minutes apart, doubling.
    #[test]
    fn readiness_is_asked_when_the_row_moves() {
        let url = "https://github.com/o/app/pull/9123";
        let mut app = App::new();
        let t0 = Instant::now();
        assert!(readiness_due(&app, url, t0), "first look");
        assert!(!readiness_due(&app, url, t0 + Duration::from_secs(30)));
        assert!(
            readiness_due(&app, url, t0 + READINESS_GAP),
            "no row: five minutes"
        );
        let t1 = t0 + READINESS_GAP;
        assert!(!readiness_due(&app, url, t1 + READINESS_GAP), "then ten");
        assert!(readiness_due(&app, url, t1 + READINESS_GAP * 2));
        let t2 = t1 + READINESS_GAP * 2;
        app.prs.observe(
            url,
            crate::pr_store::PrObservation {
                state: Some(crate::pull_request::STATE_OPEN.into()),
                checks: Some(crate::pull_request::Checks::Pending),
                ..Default::default()
            },
            crate::fetch::Asked::Cached,
        );
        assert!(
            readiness_due(&app, url, t2 + Duration::from_secs(1)),
            "a row came"
        );
        assert!(
            !readiness_due(&app, url, t2 + Duration::from_secs(60)),
            "still"
        );
        app.prs.observe(
            url,
            crate::pr_store::PrObservation {
                checks: Some(crate::pull_request::Checks::Passing),
                ..Default::default()
            },
            crate::fetch::Asked::At(crate::fetch::now()),
        );
        assert!(
            readiness_due(&app, url, t2 + Duration::from_secs(61)),
            "it moved"
        );
    }

    #[test]
    fn a_refused_merge_is_tried_once_more_then_carded() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::at("01P", dir.path());
        State::update(&local, |s| {
            s.pr = Some(pr());
            s.pushed = "abc".into();
            s.signed_head = "abc".into();
        });
        merge_result(&local, "app", &pr(), Ok(Merge::Waiting("checks".into())));
        merge_result(&local, "app", &pr(), Err("a check is failing".into()));
        let s = State::load(&local);
        assert_eq!(
            (s.merge_failures, s.signed_head.as_str()),
            (1, "abc"),
            "tried again next tick"
        );
        assert!(s.notices.is_empty());
        merge_result(&local, "app", &pr(), Err("a check is failing".into()));
        let s = State::load(&local);
        assert_eq!(s.signed_head, "", "no more tries until the next sign-off");
        assert_eq!(
            s.notices,
            vec![Notice::CantMerge {
                pr: pr(),
                why: "a check is failing".into()
            }]
        );
        let card = cant_merge_card("01P", "app", &pr(), "a check is failing");
        assert_eq!(
            card.title,
            "Knowledge PR #77 can't merge: a check is failing"
        );
        assert!(card.body[0].contains("next rule review"));
        assert_eq!(card.buttons[0].url.as_deref(), Some(pr().url.as_str()));

        merge_result(&local, "app", &pr(), Ok(Merge::Merged));
        let s = State::load(&local);
        assert_eq!((s.pr, s.pushed.as_str()), (None, ""));
    }

    #[test]
    fn an_older_state_file_reads_with_its_daily_card_fields_ignored() {
        let old = r#"{"last_daily":"2026-11-18","since":"2026-11-18T18:30:00+00:00","daily":{"kb:daily:01P:2026-11-18":[{"id":"k1-1","rule":"r","vetoed":false}]},"pr":{"number":77,"url":"u"},"pushed":"abc","window":{"ends_at":1},"merge_failures":0}"#;
        let s: State = serde_json::from_str(old).unwrap();
        assert_eq!(s.pushed, "abc");
        assert_eq!(s.signed_head, "", "an older orion's push is no sign-off");
        assert!(!merge_due(&s, true));
    }

    #[test]
    fn the_look_for_stale_topics_is_daily_from_first_sight() {
        let day = orion_core::clock::DAY_MS;
        let hour = day / 24;
        let mut state = State::default();
        assert!(!stale_due(&mut state, 1_000), "first sight starts the day");
        assert_eq!(state.stale_next, 1_000 + day);
        assert!(!stale_due(&mut state, 1_000 + 23 * hour));
        assert!(stale_due(&mut state, 1_000 + day));
        assert_eq!(state.stale_next, 1_000 + 2 * day);
        assert!(!stale_due(&mut state, 1_000 + day + hour), "once a day");
    }

    #[test]
    fn the_gitignore_card_says_why_rules_cant_be_shared() {
        let card = gitignore_card("01P", "riplo-os");
        assert_eq!(card.key, "kb:gitignore:01P");
        assert_eq!(
            card.title,
            "riplo-os ignores .orion/decisions/, so harvested rules can't be shared"
        );
    }
}
