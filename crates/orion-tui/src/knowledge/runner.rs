//! The TUI's side of the store: the session snapshot kept as the tree
//! changes, harvests queued at merge and run one at a time in the
//! background, and the cards — the one-time `.gitignore` ask, the daily
//! "new rules" card with its vetoes, and a backfill's summary.
//!
//! Nothing here blocks the loop: a harvest runs on its own task, its
//! file-heavy parts on blocking threads, and the queue is kept on disk
//! (`<data_dir>/automations/knowledge-queue.json`) so a harvest cut off
//! by a quit runs on the next start. What the background wants posted is
//! left in the state file's `notices`, which [`tick`] posts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use orion_core::WorktreeId;
use serde::{Deserialize, Serialize};

use super::harvest::{self, ClaudeCli, Outcome, Session};
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
/// How many times a harvest is tried.
const ATTEMPTS: u32 = 2;
const QUEUE_FILE: &str = "knowledge-queue.json";
const STATE_FILE: &str = "knowledge-state.json";
/// What `.git/info/exclude` gets.
const EXCLUDED: &str = ".orion/knowledge/";

/// One merged pull request waiting to be harvested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Job {
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
    pub root: PathBuf,
    pub project: String,
    pub id: String,
    pub rule: String,
    #[serde(default)]
    pub vetoed: bool,
}

/// Something the background (or the CLI) wants a card for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Notice {
    /// `.orion/knowledge/` isn't ignored by the repo at `root`.
    Gitignore { root: PathBuf, project: String },
    /// A backfill finished.
    Backfilled { project: String, lines: Vec<String> },
}

/// What the cards remember between runs:
/// `<data_dir>/automations/knowledge-state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct State {
    /// The local date the daily card was last considered.
    pub last_daily: String,
    /// RFC 3339: entries recorded after this are new to the next card.
    pub since: String,
    /// Each daily card's key → the rules it lists, in order.
    pub daily: BTreeMap<String, Vec<Listed>>,
    /// The checkouts asked about `.gitignore` already.
    pub asked_gitignore: BTreeSet<PathBuf>,
    pub notices: Vec<Notice>,
}

/// The state file's lock within this process.
static STATE_LOCK: Mutex<()> = Mutex::new(());

impl State {
    fn load() -> State {
        read_json(&crate::automations::state_dir().join(STATE_FILE))
    }

    fn save(&self) {
        write_json(&crate::automations::state_dir().join(STATE_FILE), self);
    }

    /// Change the state on disk under the lock.
    pub(crate) fn update<T>(f: impl FnOnce(&mut State) -> T) -> T {
        let _held = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = State::load();
        let mut state = before.clone();
        let out = f(&mut state);
        if state != before {
            state.save();
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
pub(crate) fn notify(notice: Notice) {
    State::update(|s| {
        if !s.notices.contains(&notice) {
            s.notices.push(notice);
        }
    });
}

// ---- the queue ----

static QUEUE_LOCK: Mutex<()> = Mutex::new(());

fn queue_path() -> PathBuf {
    crate::automations::state_dir().join(QUEUE_FILE)
}

/// Change the queue on disk under the lock; written only when `f`
/// changed it.
fn with_queue<T>(f: impl FnOnce(&mut Vec<Job>) -> T) -> T {
    let _held = QUEUE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before: Vec<Job> = read_json(&queue_path());
    let mut jobs = before.clone();
    let out = f(&mut jobs);
    if jobs != before {
        write_json(&queue_path(), &jobs);
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
    let fresh = with_queue(|jobs| {
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
    with_queue(|jobs| {
        jobs.retain(|j| j.url != job.url);
        if let Some(again) = &retry {
            jobs.push(again.clone());
        }
    });
    if let (Some(again), Some(tx)) = (retry, worker()) {
        let _ = tx.send(again);
    }
}

/// Harvest `job`'s pull request. None when it was harvested before.
async fn run_job(job: &Job) -> Result<Option<Outcome>, String> {
    let model = ClaudeCli::from_config()?;
    let store = Store::of(&job.root);
    if store.harvested(job.number) {
        return Ok(None);
    }
    let root = job.root.clone();
    let ignored = tokio::task::spawn_blocking(move || store::ignored(&root))
        .await
        .unwrap_or(true);
    if !ignored {
        notify(Notice::Gitignore {
            root: job.root.clone(),
            project: job.project.clone(),
        });
    }
    let (pr, comments) = harvest::gather_pr(&job.root, job.number).await?;
    let for_sessions = job.clone();
    let sessions = tokio::task::spawn_blocking(move || sessions_of(&for_sessions))
        .await
        .map_err(|e| e.to_string())?;
    if let Some(run) = &job.run {
        record_corrections(run, harvest::corrections(&sessions));
    }
    let store = Store::of(&job.root);
    let (outcome, unchecked) = harvest::harvest_merge(
        &model,
        &store,
        &job.project,
        &job.root,
        &pr,
        &comments,
        &sessions,
    )
    .await;
    if let Some(why) = unchecked {
        tracing::warn!(
            "knowledge: #{} of {} is recorded unchecked: {why}",
            job.number,
            job.project
        );
    }
    outcome.map(Some)
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
    for s in Snapshot::load().sessions_on(&job.root, &job.branch) {
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
    note_sessions(app);
    enqueue(Job {
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

/// Keep the session snapshot up to date with the tree: read from disk
/// once, then held here and written when it changes.
fn note_sessions(app: &App) {
    static HELD: Mutex<Option<Snapshot>> = Mutex::new(None);
    let now = orion_core::clock::now_ms();
    let (projects, seen) = snapshot::sightings(&app.tree, now);
    let path = Snapshot::path();
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let snap = held.get_or_insert_with(|| Snapshot::load_from(&path));
    if snap.note(&projects, &seen, now) {
        if let Err(why) = snap.save_to(&path) {
            tracing::warn!("knowledge: couldn't save the session snapshot: {why}");
        }
    }
}

/// The loop's beat: the snapshot, the queue left from a last run, the
/// cards the background asked for, and the daily card when it is due.
pub(crate) fn tick(app: &mut App) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    if crate::oneshot::offline() {
        return;
    }
    closes(app);
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < EVERY) {
            return;
        }
        let first = last.is_none();
        *last = Some(Instant::now());
        if first {
            resume();
        }
    }
    note_sessions(app);
    let now = chrono::Local::now();
    let roots: Vec<(PathBuf, String)> = app
        .tree
        .projects
        .iter()
        .map(|p| (p.repo_path.clone(), p.name.clone()))
        .collect();
    // The notices to post, each `.gitignore` ask once a checkout, and the
    // daily card: one read and write of the state.
    let (notices, daily_card) = State::update(|s| {
        let notices: Vec<Notice> = std::mem::take(&mut s.notices)
            .into_iter()
            .filter(|n| match n {
                Notice::Gitignore { root, .. } => s.asked_gitignore.insert(root.clone()),
                Notice::Backfilled { .. } => true,
            })
            .collect();
        (notices, daily(s, now, &roots))
    });
    for notice in notices {
        post_notice(app, notice);
    }
    if let Some(card) = daily_card {
        cards::post(app, card);
    }
}

/// Hand the worker every job a last run left queued.
fn resume() {
    let jobs = with_queue(|jobs| jobs.clone());
    if let Some(tx) = worker() {
        for job in jobs {
            let _ = tx.send(job);
        }
    }
}

/// A notice as its card; [`tick`] has kept the `.gitignore` ask to once a
/// checkout.
fn post_notice(app: &mut App, notice: Notice) {
    match notice {
        Notice::Gitignore { root, project } => cards::post(app, gitignore_card(&root, &project)),
        Notice::Backfilled { project, lines } => cards::post(
            app,
            Card {
                key: format!("kb:backfill:{project}"),
                to: Recipient::Owner,
                context: format!("Knowledge · {project}"),
                title: format!("Knowledge backfill of {project} is done"),
                body: lines,
                buttons: vec![Button::tap("ok", "OK")],
                image: None,
            },
        ),
    }
}

fn gitignore_card(root: &Path, project: &str) -> Card {
    Card {
        key: format!("kb:gitignore:{}", root.display()),
        to: Recipient::Owner,
        context: format!("Knowledge · {project}"),
        title: format!("Keep {project}'s knowledge out of git"),
        body: vec![format!(
            "orion records decisions in `.orion/knowledge/` in {}, and the repo doesn't ignore it. Add `.orion/knowledge/` to its `.gitignore`, or have orion ignore it on this machine only (`.git/info/exclude`).",
            root.display()
        )],
        buttons: vec![
            Button::tap("exclude", "Ignore on this machine"),
            Button::tap("ok", "I'll add it"),
        ],
        image: None,
    }
}

/// The daily card, when it is due and there is something on it: the
/// owner's rules recorded since the last one, across `roots`.
pub(crate) fn daily(
    state: &mut State,
    now: chrono::DateTime<chrono::Local>,
    roots: &[(PathBuf, String)],
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
    let mut rules: Vec<(String, PathBuf, Entry)> = Vec::new();
    for (root, project) in roots {
        let store = Store::of(root);
        if !store.exists() {
            continue;
        }
        for e in store.entries() {
            let new =
                chrono::DateTime::parse_from_rfc3339(&e.recorded_at).is_ok_and(|at| at > since);
            if new && e.status == ACTIVE && e.is_owners() {
                rules.push((project.clone(), root.clone(), e));
            }
        }
    }
    state.last_daily = today.clone();
    state.since = now.to_rfc3339();
    if rules.is_empty() {
        return None;
    }
    rules.truncate(DAILY_MAX);
    let key = format!("kb:daily:{today}");
    let listed = rules
        .iter()
        .map(|(project, root, e)| Listed {
            root: root.clone(),
            project: project.clone(),
            id: e.id.clone(),
            rule: e.rule.clone(),
            vetoed: false,
        })
        .collect();
    state.daily.insert(key.clone(), listed);
    // Only cards not yet forgotten can still be pressed.
    let oldest = format!(
        "kb:daily:{}",
        (now - chrono::Duration::milliseconds(cards::FORGET_AFTER_MS)).format("%Y-%m-%d")
    );
    state.daily.retain(|k, _| k.as_str() >= oldest.as_str());
    let entries: Vec<&Entry> = rules.iter().map(|(_, _, e)| e).collect();
    let projects: Vec<&str> = rules.iter().map(|(p, _, _)| p.as_str()).collect();
    Some(daily_card(&key, &entries, &projects, &[]))
}

/// The daily card under `key`: each rule numbered, with its source and
/// quote; a Veto button for each one not vetoed, and All fine.
pub(crate) fn daily_card(key: &str, rules: &[&Entry], projects: &[&str], vetoed: &[usize]) -> Card {
    let n = rules.len();
    let noun = if n == 1 { "rule" } else { "rules" };
    let mut body = Vec::new();
    let mut buttons = Vec::new();
    for (i, (e, project)) in rules.iter().zip(projects).enumerate() {
        let k = i + 1;
        let quote = match e.quote.trim() {
            "" => String::new(),
            q => format!(": “{}”", crate::ui::truncate(q, 200)),
        };
        let line = format!("{k}. {} — _{project} #{}, {}{quote}_", e.rule, e.pr, e.by);
        if vetoed.contains(&k) {
            body.push(format!("~{k}. {}~ (vetoed)", e.rule));
        } else {
            body.push(line);
            buttons.push(Button::tap(format!("veto:{k}"), format!("Veto {k}")));
        }
    }
    buttons.push(Button::tap("fine", "All fine"));
    Card {
        key: key.to_string(),
        to: Recipient::Owner,
        context: "Knowledge".into(),
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
    if let Some(root) = key.strip_prefix("kb:gitignore:") {
        if press.action == "exclude" {
            // Git and the file off the loop; the card closes on a later
            // tick ([`closes`]), saying how it went.
            let root = PathBuf::from(root);
            let run = move || {
                let note = match exclude(&root) {
                    Ok(()) => "Ignored on this machine",
                    Err(why) => {
                        tracing::warn!(
                            "knowledge: couldn't exclude .orion/knowledge/ in {}: {why}",
                            root.display()
                        );
                        "Couldn't write .git/info/exclude"
                    }
                };
                CLOSES
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((key, note));
            };
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => drop(handle.spawn_blocking(run)),
                Err(_) => run(),
            }
            return closes(app);
        }
        return cards::close(app, &key, "You'll add it");
    }
    cards::close(app, &key, "OK");
}

fn daily_action(app: &mut App, key: &str, action: &str) {
    if action == "fine" {
        return cards::close(app, key, "All fine");
    }
    let Some(k) = action
        .strip_prefix("veto:")
        .and_then(|k| k.parse::<usize>().ok())
    else {
        return;
    };
    let rule = State::update(|s| {
        let listed = s.daily.get_mut(key)?;
        let rule = listed.get_mut(k.checked_sub(1)?)?;
        rule.vetoed = true;
        Some((rule.clone(), listed.clone()))
    });
    let Some((rule, listed)) = rule else {
        return;
    };
    let veto = rule.clone();
    let run = move || {
        if let Err(why) = Store::of(&veto.root).veto(&veto.id, &veto.project) {
            tracing::warn!("knowledge: veto of {} failed: {why}", veto.id);
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => drop(handle.spawn_blocking(run)),
        Err(_) => run(),
    }
    if listed.iter().all(|l| l.vetoed) {
        return cards::close(app, key, "Vetoed");
    }
    // The press took the buttons away: the card again, without that one.
    let entries: Vec<Entry> = listed
        .iter()
        .map(|l| {
            let store = Store::of(&l.root);
            store
                .entries()
                .into_iter()
                .find(|e| e.id == l.id)
                .unwrap_or_else(|| Entry {
                    rule: l.rule.clone(),
                    ..Entry::default()
                })
        })
        .collect();
    let refs: Vec<&Entry> = entries.iter().collect();
    let projects: Vec<&str> = listed.iter().map(|l| l.project.as_str()).collect();
    let vetoed: Vec<usize> = listed
        .iter()
        .enumerate()
        .filter(|(_, l)| l.vetoed)
        .map(|(i, _)| i + 1)
        .collect();
    cards::post(app, daily_card(key, &refs, &projects, &vetoed));
}

/// Cards the background finished with, to close on the loop: their key
/// and note.
static CLOSES: Mutex<Vec<(String, &'static str)>> = Mutex::new(Vec::new());

/// Close the cards the background finished with.
fn closes(app: &mut App) {
    let done = std::mem::take(&mut *CLOSES.lock().unwrap_or_else(|e| e.into_inner()));
    for (key, note) in done {
        cards::close(app, &key, note);
    }
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

/// Ignore the store in `root`'s `.git/info/exclude`. Blocks: run off the
/// loop.
fn exclude(root: &Path) -> Result<(), String> {
    let file = common_dir(root)?.join("info").join("exclude");
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.lines().any(|l| l.trim() == EXCLUDED) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(EXCLUDED);
    text.push('\n');
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, text).map_err(|e| e.to_string())
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

    fn store_with(entries: Vec<Entry>) -> (tempfile::TempDir, Store) {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        std::fs::create_dir_all(&store.dir).unwrap();
        std::fs::create_dir_all(store.dir.join(".git")).unwrap();
        store.append(&entries).unwrap();
        (repo, store)
    }

    fn entry(id: &str, by: &str, at: chrono::DateTime<chrono::Local>) -> Entry {
        Entry {
            id: id.into(),
            pr: 12,
            by: by.into(),
            rule: format!("rule {id}"),
            quote: "always back where they were".into(),
            recorded_at: at.to_rfc3339(),
            status: ACTIVE.into(),
            ..Entry::default()
        }
    }

    #[test]
    fn the_daily_card_lists_todays_owner_rules_once_after_half_past_six() {
        let (repo, _store) = store_with(vec![
            entry("k12-1", "owner", at(9, 0)),
            entry("k12-2", "agent", at(9, 0)),
            entry("k12-3", "plan", at(10, 0)),
            entry("k11-1", "owner", at(9, 0) - chrono::Duration::days(3)),
        ]);
        let roots = vec![(repo.path().to_path_buf(), "app".to_string())];
        let mut state = State {
            since: (at(9, 0) - chrono::Duration::days(1)).to_rfc3339(),
            ..State::default()
        };
        assert!(daily(&mut state, at(18, 29), &roots).is_none(), "not yet");
        let card = daily(&mut state, at(18, 31), &roots).unwrap();
        assert_eq!(card.key, "kb:daily:2026-11-14");
        assert_eq!(card.title, "2 new rules recorded as yours today");
        assert_eq!(
            card.body[0],
            "1. rule k12-1 — _app #12, owner: “always back where they were”_"
        );
        let actions: Vec<&str> = card.buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(actions, vec!["veto:1", "veto:2", "fine"]);
        assert_eq!(state.daily["kb:daily:2026-11-14"][1].id, "k12-3");
        assert!(daily(&mut state, at(19, 0), &roots).is_none(), "once a day");
    }

    #[test]
    fn a_day_with_nothing_new_posts_nothing() {
        let (repo, _store) = store_with(vec![entry("k12-2", "agent", at(9, 0))]);
        let roots = vec![(repo.path().to_path_buf(), "app".to_string())];
        let mut state = State::default();
        assert!(daily(&mut state, at(18, 30), &roots).is_none());
        assert_eq!(state.last_daily, "2026-11-14");
    }

    #[test]
    fn a_vetoed_rule_loses_its_button() {
        let e1 = entry("k12-1", "owner", at(9, 0));
        let e2 = entry("k12-3", "plan", at(9, 0));
        let card = daily_card("kb:daily:x", &[&e1, &e2], &["app", "app"], &[1]);
        assert_eq!(card.body[0], "~1. rule k12-1~ (vetoed)");
        let actions: Vec<&str> = card.buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(actions, vec!["veto:2", "fine"]);
    }

    #[test]
    fn exclude_adds_orion_once() {
        let repo = tempfile::tempdir().unwrap();
        crate::git_proc::run(repo.path(), &["init", "-q"]).unwrap();
        assert!(!store::ignored(repo.path()));
        exclude(repo.path()).unwrap();
        exclude(repo.path()).unwrap();
        assert!(store::ignored(repo.path()));
        let text = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert_eq!(text.matches(EXCLUDED).count(), 1);
    }
}
