//! The TUI's side of knowledge: the session snapshot kept as the tree
//! changes, harvests queued at merge and run one at a time in the
//! background, the knowledge pull request merged after its veto window,
//! and the cards — the daily "new rules" card with its vetoes, the
//! one-time "ignores `.orion/decisions/`" card, a merge that failed twice,
//! and a backfill's summary.
//!
//! Nothing here blocks the loop: a harvest, a veto and a merge each run on
//! a task of their own, their git and file work on blocking threads. Each
//! project's queue and state are kept in its folder under the data dir
//! (`super::local`), so a harvest cut off by a quit runs on the next
//! start. What the background wants posted is left in the state's
//! `notices`, which [`tick`] posts.
//!
//! **The veto window.** At 18:30 the daily card lists the rules recorded
//! as the owner's since the last one, each with a veto, and a link to the
//! knowledge pull request. With `factory.knowledge_auto_merge` on (the
//! default), the window closes [`WINDOW_MS`] later — at once on All fine
//! — and orion then merges the pull request itself ([`merge_due`],
//! `Sharer::merge`): only when it is mergeable, no check fails, and its
//! head is what orion last pushed. A check still running waits for a
//! later tick; a conflict is rebuilt and pushed; a refusal is tried once
//! more, then said on a card. Never forced.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use orion_core::WorktreeId;
use serde::{Deserialize, Serialize};

use super::harvest::{self, ClaudeCli, Outcome, Session, Target};
use super::local::Local;
use super::records;
use super::share::{self, GhCli, Merge, Pr, Published, Sharer, SystemGit};
use super::snapshot::{self, Snapshot};
use super::store::{self, Entry, Store, ACTIVE};
use super::{backfill, transcripts};
use crate::app::App;
use crate::factory::cards::{self, Button, Card, Press, Recipient};

/// How often [`tick`] does its work.
const EVERY: Duration = Duration::from_secs(30);
/// When the daily card goes, local time.
const DAILY_AT: (u32, u32) = (18, 30);
/// The most rules one daily card lists.
const DAILY_MAX: usize = 20;
/// How long after the daily card the knowledge pull request merges, unless
/// a veto or All fine came first.
pub(crate) const WINDOW_MS: i64 = 2 * 60 * 60 * 1000;
/// How many times a merge that is refused is tried before a card says so.
const MERGE_TRIES: u32 = 2;
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

/// One rule a daily card lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Listed {
    pub id: String,
    pub rule: String,
    #[serde(default)]
    pub vetoed: bool,
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

/// The veto window on the knowledge pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Window {
    /// Epoch ms it closes, and the merge may go.
    pub ends_at: i64,
}

/// What a project's cards and pull request remember between runs:
/// `<data_dir>/knowledge/<project id>/state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct State {
    /// The local date the daily card was last considered.
    pub last_daily: String,
    /// RFC 3339: entries recorded after this are new to the next card.
    pub since: String,
    /// Each daily card's key → the rules it lists, in order.
    pub daily: BTreeMap<String, Vec<Listed>>,
    /// Whether the "ignores `.orion/decisions/`" card went.
    pub asked_gitignore: bool,
    pub notices: Vec<Notice>,
    /// The open knowledge pull request.
    pub pr: Option<Pr>,
    /// The commit orion last pushed to its knowledge branch.
    pub pushed: String,
    /// The veto window, while one is open.
    pub window: Option<Window>,
    /// Merges refused since the window closed.
    pub merge_failures: u32,
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
            s.window = None;
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
/// try when it failed.
async fn work(job: Job) {
    let result = run_job(&job).await;
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

/// The project at `root` (`local`'s), as orion writes its knowledge: its
/// base branch found off the loop.
async fn sharer(local: &Local, root: &Path, project: &str) -> Result<Sharer<'static>, String> {
    let at = root.to_path_buf();
    let base = tokio::task::spawn_blocking(move || share::base_of(&at))
        .await
        .map_err(|e| e.to_string())?;
    Ok(Sharer {
        git: &GIT,
        gh: &GH,
        main: root.to_path_buf(),
        local: local.clone(),
        project: project.to_string(),
        base,
    })
}

/// Harvest `job`'s pull request into the knowledge branch, and push and
/// propose it. None when it was harvested before (anything left unpushed
/// is pushed all the same) or the repo ignores `.orion/decisions/`.
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
    let records = records::for_check_in(&branch.clone);
    let to = Target {
        store: &store,
        local: &local,
        project: &job.project,
        records: &records,
    };
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
            let (outcome, unchecked) =
                harvest::harvest_merge(&model, &to, &job.root, &pr, &comments, &sessions).await;
            if let Some(why) = unchecked {
                tracing::warn!(
                    "knowledge: #{} of {} is recorded unchecked: {why}",
                    job.number,
                    job.project
                );
            }
            Some(outcome?)
        }
    };
    let published = share
        .publish(&branch, &format!("knowledge: harvest #{}", job.number))
        .await?;
    drop(branch);
    note_published(&local, &published);
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

/// The projects with a merge on its way, so a tick starts no second one.
static MERGING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The loop's beat: the snapshot, the queue left from a last run, and
/// for each project the cards the background asked for, the daily card
/// when it is due, and the merge once its window has closed.
pub(crate) fn tick(app: &mut App) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    if crate::oneshot::offline() {
        return;
    }
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < EVERY) {
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
    let auto = app.pr_body.settings.knowledge_auto_merge;
    let projects: Vec<(String, String, PathBuf)> = app
        .tree
        .projects
        .iter()
        .map(|p| (p.id.to_string(), p.name.clone(), p.repo_path.clone()))
        .collect();
    for (id, name, root) in projects {
        let local = Local::of(&id);
        // The notices to post, the `.gitignore` card once, the daily card
        // and whether the merge is due: one read and write of the state.
        let (notices, daily_card, merge) = State::update(&local, |s| {
            let notices: Vec<Notice> = std::mem::take(&mut s.notices)
                .into_iter()
                .filter(|n| match n {
                    Notice::Gitignore => !std::mem::replace(&mut s.asked_gitignore, true),
                    _ => true,
                })
                .collect();
            let card = daily(s, now, &name, &local, auto);
            let merge = merge_due(s, now.timestamp_millis(), auto)
                .then(|| (s.pr.clone(), s.pushed.clone()));
            (notices, card, merge)
        });
        for notice in notices {
            post_notice(app, &id, &name, notice);
        }
        if let Some(card) = daily_card {
            cards::post(app, card);
        }
        if let Some((Some(pr), pushed)) = merge {
            start_merge(local, root, name, pr, pushed);
        }
    }
}

/// Whether the knowledge pull request should be merged now: the switch
/// on, a pull request orion pushed to, and its veto window closed.
pub(crate) fn merge_due(state: &State, now_ms: i64, auto: bool) -> bool {
    auto && state.pr.is_some()
        && !state.pushed.is_empty()
        && state.window.as_ref().is_some_and(|w| now_ms >= w.ends_at)
}

/// Merge the knowledge pull request, off the loop: once at a time a
/// project.
fn start_merge(local: Local, root: PathBuf, project: String, pr: Pr, pushed: String) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if !MERGING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(local.id.clone())
    {
        return;
    }
    handle.spawn(async move {
        let wanted = crate::config::Config::load().merge_method(&root);
        let result = match sharer(&local, &root, &project).await {
            Ok(share) => {
                let merged = share.merge(&pr, &pushed, wanted).await;
                if let Ok(Merge::Conflicting) = &merged {
                    // The base moved under it: rebased — rebuilt, when
                    // that conflicts — and pushed for the next tick.
                    let rebuilt = async {
                        let branch = share.open().await?;
                        let published = share
                            .publish(&branch, &format!("knowledge: rebuilt on {}", share.base))
                            .await?;
                        drop(branch);
                        note_published(&local, &published);
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
        MERGING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&local.id);
    });
}

/// What a merge attempt leaves in the state: a merged or closed pull
/// request forgotten; a refusal counted, and on the second a card, the
/// window closed until the next day's.
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
                s.window = None;
                s.merge_failures = 0;
            });
        }
        Ok(Merge::Waiting(why)) => {
            tracing::info!("knowledge: #{} of {project} waits: {why}", pr.number);
        }
        Ok(Merge::Conflicting) => {}
        Err(why) => {
            tracing::warn!("knowledge: #{} of {project} can't merge: {why}", pr.number);
            State::update(local, |s| {
                s.merge_failures += 1;
                if s.merge_failures >= MERGE_TRIES {
                    s.window = None;
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
            "orion tried twice and stopped until tomorrow's card. Merge it yourself once it's fixed, or leave it for then.".into(),
        ],
        buttons: vec![Button::link("Open PR", &pr.url), Button::tap("ok", "OK")],
        image: None,
    }
}

/// The daily card for project `project` (`local`'s), when it is due and
/// there is something on it: the owner's rules this machine recorded
/// since the last one. When it is due, the veto window on the knowledge
/// pull request opens, card or not (`auto` on).
pub(crate) fn daily(
    state: &mut State,
    now: chrono::DateTime<chrono::Local>,
    project: &str,
    local: &Local,
    auto: bool,
) -> Option<Card> {
    let today = now.format("%Y-%m-%d").to_string();
    let at = chrono::NaiveTime::from_hms_opt(DAILY_AT.0, DAILY_AT.1, 0)?;
    if now.time() < at || state.last_daily == today {
        return None;
    }
    let since = match state.since.as_str() {
        "" => (now - chrono::Duration::days(1)).to_rfc3339(),
        since => since.to_string(),
    };
    let since = chrono::DateTime::parse_from_rfc3339(&since).ok()?;
    let store = Store::of(&local.clone_dir());
    let mut rules: Vec<Entry> = store
        .entries()
        .into_iter()
        .filter(|e| {
            let new =
                chrono::DateTime::parse_from_rfc3339(&e.recorded_at).is_ok_and(|at| at > since);
            new && e.status == ACTIVE && e.is_owners() && local.has_source(e.pr)
        })
        .collect();
    state.last_daily = today.clone();
    state.since = now.to_rfc3339();
    if auto && state.pr.is_some() {
        state.window = Some(Window {
            ends_at: now.timestamp_millis() + WINDOW_MS,
        });
        state.merge_failures = 0;
    }
    if rules.is_empty() {
        return None;
    }
    rules.truncate(DAILY_MAX);
    let key = format!("kb:daily:{}:{today}", local.id);
    let listed = rules
        .iter()
        .map(|e| Listed {
            id: e.id.clone(),
            rule: e.rule.clone(),
            vetoed: false,
        })
        .collect();
    state.daily.insert(key.clone(), listed);
    // Only cards not yet forgotten can still be pressed.
    let oldest = (now - chrono::Duration::milliseconds(cards::FORGET_AFTER_MS))
        .format("%Y-%m-%d")
        .to_string();
    state.daily.retain(|k, _| {
        k.rsplit(':')
            .next()
            .is_some_and(|day| day >= oldest.as_str())
    });
    let refs: Vec<&Entry> = rules.iter().collect();
    Some(daily_card(&key, project, &refs, &[], state, auto))
}

/// The daily card under `key`: each rule numbered, with its source and
/// quote; a Veto button for each one not vetoed, All fine, and the
/// knowledge pull request's link.
pub(crate) fn daily_card(
    key: &str,
    project: &str,
    rules: &[&Entry],
    vetoed: &[usize],
    state: &State,
    auto: bool,
) -> Card {
    let n = rules.len();
    let noun = if n == 1 { "rule" } else { "rules" };
    let mut body = Vec::new();
    let mut buttons = Vec::new();
    for (i, e) in rules.iter().enumerate() {
        let k = i + 1;
        let quote = match e.quote.trim() {
            "" => String::new(),
            q => format!(": “{}”", crate::ui::truncate(q, 200)),
        };
        if vetoed.contains(&k) {
            body.push(format!("~{k}. {}~ (vetoed)", e.rule));
        } else {
            body.push(format!("{k}. {} — _#{}, {}{quote}_", e.rule, e.pr, e.by));
            buttons.push(Button::tap(format!("veto:{k}"), format!("Veto {k}")));
        }
    }
    buttons.push(Button::tap("fine", "All fine"));
    if let Some(pr) = &state.pr {
        let hours = WINDOW_MS / (60 * 60 * 1000);
        body.push(match auto {
            true => format!(
                "_orion merges #{} in {hours} hours unless you veto; All fine merges it now._",
                pr.number
            ),
            false => format!("_They're shared once #{} merges._", pr.number),
        });
        buttons.push(Button::link("Open PR", &pr.url));
    }
    Card {
        key: key.to_string(),
        to: Recipient::Owner,
        context: format!("Knowledge · {project}"),
        title: format!("{n} new {noun} recorded as yours today"),
        body,
        buttons,
        image: None,
    }
}

/// A press on a `kb:` card.
pub(crate) fn on_action(app: &mut App, press: Press) {
    let key = press.key.to_string();
    if key.starts_with("kb:daily:") {
        return daily_action(app, &key, press.action);
    }
    cards::close(app, &key, "OK");
}

/// The project a `kb:<kind>:<id>:…` key is about: its folder, name and
/// main checkout.
fn project_of(app: &App, key: &str) -> Option<(Local, String, PathBuf)> {
    let id = key.split(':').nth(2)?;
    let project = app.tree.projects.iter().find(|p| p.id.as_str() == id)?;
    Some((
        Local::of(id),
        project.name.clone(),
        project.repo_path.clone(),
    ))
}

fn daily_action(app: &mut App, key: &str, action: &str) {
    let Some((local, project, root)) = project_of(app, key) else {
        return cards::close(app, key, "That project is gone");
    };
    if action == "fine" {
        let now = orion_core::clock::now_ms();
        State::update(&local, |s| {
            if let Some(window) = &mut s.window {
                window.ends_at = window.ends_at.min(now);
            }
        });
        return cards::close(app, key, "All fine");
    }
    let Some(k) = action
        .strip_prefix("veto:")
        .and_then(|k| k.parse::<usize>().ok())
    else {
        return;
    };
    let rule = State::update(&local, |s| {
        let listed = s.daily.get_mut(key)?;
        let rule = listed.get_mut(k.checked_sub(1)?)?;
        rule.vetoed = true;
        Some((rule.clone(), listed.clone(), s.clone()))
    });
    let Some((rule, listed, state)) = rule else {
        return;
    };
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let (local, project, root) = (local.clone(), project.clone(), root.clone());
        handle.spawn(async move {
            if let Err(why) = veto(&local, &root, &project, &rule.id).await {
                tracing::warn!("knowledge: veto of {} failed: {why}", rule.id);
            }
        });
    }
    if listed.iter().all(|l| l.vetoed) {
        return cards::close(app, key, "Vetoed");
    }
    // The press took the buttons away: the card again, without that one.
    let entries: Vec<Entry> = {
        let all = Store::of(&local.clone_dir()).entries();
        listed
            .iter()
            .map(|l| {
                all.iter()
                    .find(|e| e.id == l.id)
                    .cloned()
                    .unwrap_or_else(|| Entry {
                        rule: l.rule.clone(),
                        ..Entry::default()
                    })
            })
            .collect()
    };
    let refs: Vec<&Entry> = entries.iter().collect();
    let vetoed: Vec<usize> = listed
        .iter()
        .enumerate()
        .filter(|(_, l)| l.vetoed)
        .map(|(i, _)| i + 1)
        .collect();
    let auto = app.pr_body.settings.knowledge_auto_merge;
    cards::post(app, daily_card(key, &project, &refs, &vetoed, &state, auto));
}

/// Take entry `id` out on the knowledge branch, and push and propose it.
async fn veto(local: &Local, root: &Path, project: &str, id: &str) -> Result<(), String> {
    let share = sharer(local, root, project).await?;
    let branch = share.open().await?;
    branch.store().veto(id, project)?;
    let published = share
        .publish(&branch, &format!("knowledge: veto {id}"))
        .await?;
    drop(branch);
    note_published(local, &published);
    Ok(())
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
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 11, 14, h, m, 0)
            .unwrap()
    }

    /// A project's folder whose working clone holds `entries`, each
    /// harvested on this machine but the ones in `elsewhere`.
    fn local_with(entries: Vec<Entry>, elsewhere: &[u64]) -> (tempfile::TempDir, Local) {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::at("01P", dir.path());
        let store = Store::of(&local.clone_dir());
        store.append(&entries).unwrap();
        for e in &entries {
            if !elsewhere.contains(&e.pr) {
                local.write_source(e.pr, "dossier").unwrap();
            }
        }
        (dir, local)
    }

    fn entry(id: &str, pr: u64, by: &str, at: chrono::DateTime<chrono::Local>) -> Entry {
        Entry {
            id: id.into(),
            pr,
            by: by.into(),
            rule: format!("rule {id}"),
            quote: "always back where they were".into(),
            recorded_at: at.to_rfc3339(),
            status: ACTIVE.into(),
            ..Entry::default()
        }
    }

    fn pr() -> Pr {
        Pr {
            number: 77,
            url: "https://github.com/o/app/pull/77".into(),
        }
    }

    #[test]
    fn the_daily_card_lists_todays_owner_rules_once_after_half_past_six() {
        let (_dir, local) = local_with(
            vec![
                entry("k12-1", 12, "owner", at(9, 0)),
                entry("k12-2", 12, "agent", at(9, 0)),
                entry("k12-3", 12, "plan", at(10, 0)),
                entry("k11-1", 11, "owner", at(9, 0) - chrono::Duration::days(3)),
                entry("k20-1", 20, "owner", at(9, 0)),
            ],
            &[20],
        );
        let mut state = State {
            since: (at(9, 0) - chrono::Duration::days(1)).to_rfc3339(),
            pr: Some(pr()),
            pushed: "abc".into(),
            ..State::default()
        };
        assert!(
            daily(&mut state, at(18, 29), "app", &local, true).is_none(),
            "not yet"
        );
        let card = daily(&mut state, at(18, 31), "app", &local, true).unwrap();
        assert_eq!(card.key, "kb:daily:01P:2026-11-14");
        assert_eq!(card.context, "Knowledge · app");
        assert_eq!(
            card.title, "2 new rules recorded as yours today",
            "not another person's #20"
        );
        assert_eq!(
            card.body[0],
            "1. rule k12-1 — _#12, owner: “always back where they were”_"
        );
        assert_eq!(
            card.body[2],
            "_orion merges #77 in 2 hours unless you veto; All fine merges it now._"
        );
        let actions: Vec<&str> = card.buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(actions, vec!["veto:1", "veto:2", "fine", "open"]);
        assert_eq!(card.buttons[3].url.as_deref(), Some(pr().url.as_str()));
        assert_eq!(state.daily["kb:daily:01P:2026-11-14"][1].id, "k12-3");
        assert!(
            daily(&mut state, at(19, 0), "app", &local, true).is_none(),
            "once a day"
        );
    }

    #[test]
    fn a_day_with_nothing_new_posts_nothing_but_still_opens_the_window() {
        let (_dir, local) = local_with(vec![entry("k12-2", 12, "agent", at(9, 0))], &[]);
        let mut state = State {
            pr: Some(pr()),
            pushed: "abc".into(),
            ..State::default()
        };
        assert!(daily(&mut state, at(18, 30), "app", &local, true).is_none());
        assert_eq!(state.last_daily, "2026-11-14");
        assert_eq!(
            state.window,
            Some(Window {
                ends_at: at(18, 30).timestamp_millis() + WINDOW_MS
            })
        );
    }

    #[test]
    fn the_merge_waits_for_the_window_and_the_switch() {
        let (_dir, local) = local_with(vec![entry("k12-1", 12, "owner", at(9, 0))], &[]);
        let mut state = State {
            pr: Some(pr()),
            pushed: "abc".into(),
            ..State::default()
        };
        assert!(
            !merge_due(&state, at(9, 0).timestamp_millis(), true),
            "no window yet"
        );
        daily(&mut state, at(18, 30), "app", &local, true).unwrap();
        let card_at = at(18, 30).timestamp_millis();
        assert!(
            !merge_due(&state, card_at + WINDOW_MS - 1, true),
            "inside the window"
        );
        assert!(merge_due(&state, card_at + WINDOW_MS, true), "it closed");
        assert!(
            merge_due(&state, card_at + 5 * WINDOW_MS, true),
            "or the next run after"
        );
        assert!(
            !merge_due(&state, card_at + WINDOW_MS, false),
            "switched off"
        );
        let unpushed = State {
            pushed: String::new(),
            ..state.clone()
        };
        assert!(!merge_due(&unpushed, card_at + WINDOW_MS, true));

        // Switched off, no window opens, and the card says the PR waits.
        let mut off = State {
            pr: Some(pr()),
            pushed: "abc".into(),
            ..State::default()
        };
        let card = daily(&mut off, at(18, 30), "app", &local, false).unwrap();
        assert_eq!(off.window, None);
        assert!(card
            .body
            .contains(&"_They're shared once #77 merges._".to_string()));
    }

    #[test]
    fn a_refused_merge_is_tried_once_more_then_carded() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::at("01P", dir.path());
        State::update(&local, |s| {
            s.pr = Some(pr());
            s.pushed = "abc".into();
            s.window = Some(Window { ends_at: 1 });
        });
        merge_result(&local, "app", &pr(), Ok(Merge::Waiting("checks".into())));
        merge_result(&local, "app", &pr(), Err("a check is failing".into()));
        let s = State::load(&local);
        assert_eq!(
            (s.merge_failures, s.window.is_some()),
            (1, true),
            "tried again next tick"
        );
        assert!(s.notices.is_empty());
        merge_result(&local, "app", &pr(), Err("a check is failing".into()));
        let s = State::load(&local);
        assert_eq!(s.window, None, "no more tries until tomorrow");
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
        assert_eq!(card.buttons[0].url.as_deref(), Some(pr().url.as_str()));

        merge_result(&local, "app", &pr(), Ok(Merge::Merged));
        let s = State::load(&local);
        assert_eq!((s.pr, s.pushed.as_str()), (None, ""));
    }

    #[test]
    fn a_vetoed_rule_loses_its_button() {
        let e1 = entry("k12-1", 12, "owner", at(9, 0));
        let e2 = entry("k12-3", 12, "plan", at(9, 0));
        let card = daily_card(
            "kb:daily:x",
            "app",
            &[&e1, &e2],
            &[1],
            &State::default(),
            true,
        );
        assert_eq!(card.body[0], "~1. rule k12-1~ (vetoed)");
        let actions: Vec<&str> = card.buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(actions, vec!["veto:2", "fine"]);
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
