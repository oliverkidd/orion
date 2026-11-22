//! THE RULE REVIEW, knowledge's side: which rules wait on the owner's
//! sign-off, what a sign-off does, and the promotions into the agent docs.
//! The overlay and the banner are `crate::rule_review`.
//!
//! - **Pending** ([`pending_of`]): the active entries on this person's
//!   knowledge branch that the base doesn't have and that no review signed
//!   off (`State::signed`). Worked out off the loop every tick
//!   ([`refresh`]), from the working clone: the branch's `entries.jsonl`
//!   against `origin/<base>`'s.
//! - **When it asks**: at [`REVIEW_AT`] (09:00) — or the first launch after
//!   — when a rule recorded before that time waits ([`cutoff`]); the
//!   review then lists every rule waiting.
//! - **A sign-off** ([`sign_off`]): each dropped rule vetoed, each kept
//!   rule with a note rewritten to fit it (one model call a topic), every
//!   drop and note kept as a lesson in `lessons.jsonl` for later harvests;
//!   then the branch pushed and its pull request opened or brought up to
//!   date — the only push a harvest's rules ever get — and, with
//!   `factory.knowledge_auto_merge` on, merged once it stands
//!   (`super::runner::merge_due`).
//! - **Promotions** ([`start_promote_scan`]): once a week a model call
//!   picks the rules that hold repo-wide — the kind the always-loaded agent
//!   docs should say — and the next review offers them. One kept moves:
//!   its own pull request on the agent docs branch adds it to the docs
//!   (`Sharer::propose_docs`), and once that merges the harvested copies
//!   are retired, citing the doc ([`start_docs_merge`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::harvest::{self, ClaudeCli, Model, ModelAsk, JSON_ONLY};
use super::local::Local;
use super::runner::{self, note_published, spawn_once, InFlight, State};
use super::share::{self, Merge, Pr};
use super::store::{self, Entry, Lesson, Store, ACTIVE, DIR, ENTRIES_FILE};
use super::Step;

/// When the review asks, local time.
pub(crate) const REVIEW_AT: (u32, u32) = (9, 0);
/// How often the promotion scan runs.
const PROMOTE_EVERY_MS: i64 = 7 * orion_core::clock::DAY_MS;
/// The most promotions waiting at once.
const PROMOTE_MAX: usize = 5;
/// How much of the agent docs the scan reads, in chars.
const DOCS_CHARS: usize = 30_000;
/// How many times a refused agent docs merge is tried.
const DOCS_TRIES: u32 = 2;

/// A rule the scan found holds repo-wide: the entries it stands on, and
/// how the agent docs should say it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Candidate {
    pub ids: Vec<String>,
    pub rule: String,
    pub why: String,
}

impl Candidate {
    /// Its stable key: its first entry.
    pub(crate) fn key(&self) -> &str {
        self.ids.first().map(String::as_str).unwrap_or_default()
    }
}

/// The agent docs pull request a sign-off opened.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DocsPr {
    pub pr: Pr,
    /// The head pushed: the merge goes only while the pull request has it.
    pub pushed: String,
    /// The entries it promotes, retired once it merges.
    pub ids: Vec<String>,
    /// The docs it changed.
    pub paths: Vec<String>,
    /// Merges refused so far.
    pub failures: u32,
}

/// One project's review, as the TUI holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProjectPending {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    /// Every rule waiting, the owner's first.
    pub rules: Vec<Entry>,
    /// The promotions waiting.
    pub promote: Vec<Candidate>,
    /// The knowledge pull request, once a sign-off opened it.
    pub pr: Option<Pr>,
}

impl ProjectPending {
    /// How many items the review has for it.
    pub(crate) fn count(&self) -> usize {
        self.rules.len() + self.promote.len()
    }

    /// Whether something here is due a review at `cutoff`: a rule recorded
    /// before it, or a promotion.
    pub(crate) fn due(&self, cutoff: chrono::DateTime<chrono::Local>) -> bool {
        !self.promote.is_empty() || self.rules.iter().any(|e| recorded_before(e, cutoff))
    }
}

/// Whether `e` was recorded before `cutoff` (one with no time counts).
fn recorded_before(e: &Entry, cutoff: chrono::DateTime<chrono::Local>) -> bool {
    chrono::DateTime::parse_from_rfc3339(&e.recorded_at).map_or(true, |at| at < cutoff)
}

/// Today's [`REVIEW_AT`].
fn review_time() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(REVIEW_AT.0, REVIEW_AT.1, 0).unwrap_or_default()
}

/// The latest [`REVIEW_AT`] at or before `now`: today's from 09:00,
/// yesterday's before.
pub(crate) fn cutoff(now: chrono::DateTime<chrono::Local>) -> chrono::DateTime<chrono::Local> {
    use chrono::TimeZone;
    let day = match past_review_time(now) {
        true => now.date_naive(),
        false => now.date_naive().pred_opt().unwrap_or(now.date_naive()),
    };
    chrono::Local
        .from_local_datetime(&day.and_time(review_time()))
        .earliest()
        .unwrap_or(now)
}

/// Whether `now` is past today's [`REVIEW_AT`].
pub(crate) fn past_review_time(now: chrono::DateTime<chrono::Local>) -> bool {
    now.time() >= review_time()
}

/// The rules waiting on a review: `branch`'s active entries the base
/// (`base`) doesn't have, not `signed` — the owner's (his words, picks and
/// approved plans) first, then the rest, each oldest first.
pub(crate) fn pending_of(
    branch: &[Entry],
    base: &[Entry],
    signed: &BTreeSet<String>,
) -> Vec<Entry> {
    let on_base: BTreeSet<&str> = base.iter().map(|e| e.id.as_str()).collect();
    let mut out: Vec<Entry> = branch
        .iter()
        .filter(|e| {
            e.status == ACTIVE && !on_base.contains(e.id.as_str()) && !signed.contains(&e.id)
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        (!a.is_owners(), &a.recorded_at, &a.id).cmp(&(!b.is_owners(), &b.recorded_at, &b.id))
    });
    out
}

/// The entries `entries.jsonl` has at `rev` in `clone` (blocking git).
fn entries_at(clone: &Path, rev: &str) -> Vec<Entry> {
    let file = format!("{DIR}/{ENTRIES_FILE}");
    store::read_entries(
        &crate::git_proc::read(clone, &["show", &format!("{rev}:{file}")]).unwrap_or_default(),
    )
}

/// The commits `refs` point at in `clone`, in order: None for one that
/// doesn't resolve (blocking git).
fn shas(clone: &Path, refs: &[&str]) -> Vec<Option<String>> {
    refs.iter()
        .map(|r| {
            crate::git_proc::read(
                clone,
                &["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")],
            )
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        })
        .collect()
}

/// One [`branch_and_base`] read: the branch's and the base's commits, and
/// their entries.
type Read = (String, String, Vec<Entry>, Vec<Entry>);

/// The working clone's knowledge branch `branch` and its `origin/<base>`,
/// as entries — read again only when either moved since the last read
/// (blocking git). None while either can't be read: no clone, no
/// knowledge branch yet, or the base never fetched — nothing is pending
/// then, so nothing unknown is ever pushed or listed.
pub(crate) fn branch_and_base(
    local: &Local,
    base: &str,
    branch: &str,
) -> Option<(Vec<Entry>, Vec<Entry>)> {
    static READ: Mutex<BTreeMap<String, Read>> = Mutex::new(BTreeMap::new());
    let clone = local.clone_dir();
    if base.is_empty() || branch.is_empty() || !clone.join(".git").exists() {
        return None;
    }
    let branch_ref = format!("refs/heads/{branch}");
    let base_ref = format!("refs/remotes/origin/{base}");
    let [Some(at_branch), Some(at_base)] =
        <[Option<String>; 2]>::try_from(shas(&clone, &[&branch_ref, &base_ref])).ok()?
    else {
        return None;
    };
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((b, a, ours, theirs)) = read.get(&local.id) {
        if *b == at_branch && *a == at_base {
            return Some((ours.clone(), theirs.clone()));
        }
    }
    let ours = entries_at(&clone, &at_branch);
    let theirs = entries_at(&clone, &at_base);
    read.insert(
        local.id.clone(),
        (at_branch, at_base, ours.clone(), theirs.clone()),
    );
    Some((ours, theirs))
}

/// [`branch_and_base`] off the loop, for this person's branch.
async fn read_branch_and_base(local: &Local, base: &str) -> Option<(Vec<Entry>, Vec<Entry>)> {
    let branch = share::branch_for(&runner::gh_login().await?);
    let (local, base) = (local.clone(), base.to_string());
    tokio::task::spawn_blocking(move || branch_and_base(&local, &base, &branch))
        .await
        .ok()
        .flatten()
}

// ---- the refresh ----

/// The last refresh's result, waiting for the TUI to take it.
static REFRESHED: Mutex<Option<Vec<ProjectPending>>> = Mutex::new(None);
/// Whether a refresh is on its way.
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// How long the pending review goes without being worked out again when
/// nothing has asked for it: the backstop for a change nothing here saw
/// (a teammate's merge landing on the base).
pub(crate) const REFRESH_BACKSTOP: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Something moved what the review waits on — a harvest committed, a
/// sign-off, the base fetched, the review opened — so the next beat works
/// it out again ([`refresh_due`]).
static REFRESH_WANTED: AtomicBool = AtomicBool::new(true);

/// When the last refresh was started.
static REFRESHED_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Ask for the pending review to be worked out again on the next beat.
pub(crate) fn want_refresh() {
    REFRESH_WANTED.store(true, Ordering::SeqCst);
}

/// Whether the beat should refresh now: asked for since the last one, or
/// [`REFRESH_BACKSTOP`] since it. Taking it resets both. It used to run on
/// every 30 s beat for every project — a `gh pr list` and a handful of
/// `git rev-parse` each time.
pub(crate) fn refresh_due(now: std::time::Instant) -> bool {
    let mut at = REFRESHED_AT.lock().unwrap_or_else(|e| e.into_inner());
    let wanted = REFRESH_WANTED.swap(false, Ordering::SeqCst);
    let due = wanted || at.is_none_or(|at| now.saturating_duration_since(at) >= REFRESH_BACKSTOP);
    if due {
        *at = Some(now);
    }
    due
}

/// Work out every project's pending review off the loop — one refresh at
/// a time — for [`take_refreshed`]. `signed` forgets what has reached the
/// base as it goes.
pub(crate) fn refresh(projects: Vec<(String, String, PathBuf)>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if REFRESHING.swap(true, Ordering::SeqCst) {
        return;
    }
    handle.spawn(async move {
        let mut out = Vec::new();
        for (id, name, root) in projects {
            let base = share::base_of(&root, runner::gh()).await;
            let local = Local::of(&id);
            let read = read_branch_and_base(&local, &base).await;
            let (rules, promote, pr) = State::update(&local, |s| {
                let rules = match &read {
                    Some((branch, base)) => {
                        let on_base: BTreeSet<&str> = base.iter().map(|e| e.id.as_str()).collect();
                        s.signed.retain(|id| !on_base.contains(id.as_str()));
                        let active: BTreeSet<&str> = branch
                            .iter()
                            .filter(|e| e.status == ACTIVE)
                            .map(|e| e.id.as_str())
                            .collect();
                        s.promote
                            .retain(|c| c.ids.iter().all(|id| active.contains(id.as_str())));
                        pending_of(branch, base, &s.signed)
                    }
                    None => Vec::new(),
                };
                (rules, s.promote.clone(), s.pr.clone())
            });
            out.push(ProjectPending {
                id,
                name,
                root,
                rules,
                promote,
                pr,
            });
        }
        *REFRESHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(out);
        REFRESHING.store(false, Ordering::SeqCst);
        crate::chores::wake(crate::chores::Chore::Knowledge);
    });
}

/// The last refresh's result, once.
pub(crate) fn take_refreshed() -> Option<Vec<ProjectPending>> {
    REFRESHED.lock().unwrap_or_else(|e| e.into_inner()).take()
}

// ---- what the background says ----

/// What a sign-off or a promotion wants said in a flash: (ok, text).
static SAID: Mutex<Vec<(bool, String)>> = Mutex::new(Vec::new());

fn say(ok: bool, text: impl Into<String>) {
    SAID.lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((ok, text.into()));
    crate::chores::wake(crate::chores::Chore::Knowledge);
}

/// Post what the background said: its last failure if any went wrong,
/// else its last word.
pub(crate) fn post_said(app: &mut crate::app::App) {
    let said: Vec<(bool, String)> =
        std::mem::take(&mut *SAID.lock().unwrap_or_else(|e| e.into_inner()));
    let pick = said
        .iter()
        .rev()
        .find(|(ok, _)| !ok)
        .or_else(|| said.last())
        .cloned();
    if let Some((ok, text)) = pick {
        app.flash = Some(match ok {
            true => crate::flash::Flash::done(text),
            false => crate::flash::Flash::failed(text),
        });
        app.dirty = true;
    }
}

// ---- the sign-off ----

/// The owner's verdict on one rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub id: String,
    pub keep: bool,
    /// What's wrong with it: a kept rule is rewritten to fit; a dropped
    /// one's reason. Empty for none.
    pub note: String,
}

/// One project's sign-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignOff {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub verdicts: Vec<Verdict>,
    /// Promotions kept: moved into the agent docs.
    pub promote: Vec<Candidate>,
    /// Promotions turned down: never offered again.
    pub decline: Vec<Candidate>,
}

/// The projects with a sign-off on its way.
static SIGNING: InFlight = InFlight::new();

/// Sign off each of `all` off the loop — one at a time a project — and
/// say how it went. The picks on disk go once it went through; a failed
/// sign-off keeps them for the next try.
pub(crate) fn sign_off(all: Vec<SignOff>) {
    for one in all {
        let key = one.id.clone();
        spawn_once(&SIGNING, &key, async move {
            let result = run_sign_off(&one).await;
            // Signed or not, what waits may have moved.
            want_refresh();
            match result {
                Ok(line) => {
                    crate::rule_review::clear_draft(&one.id);
                    say(true, line)
                }
                Err(why) => {
                    tracing::warn!("knowledge: sign-off of {} failed: {why}", one.name);
                    say(
                        false,
                        format!("Rule review of {} didn't go through: {why}", one.name),
                    );
                }
            }
        });
    }
}

/// The rewrite call's rules.
const REWRITE_SYSTEM: &str = r#"You keep one topic page of a repository's knowledge store: the rules autonomous coding agents read before they build in that area. The owner reviewed some of its rules and left a note on each saying what is wrong with it. Rewrite each listed rule so it says what the owner's note says: one sentence, at most 30 words, present tense, the lesson rather than one ticket's spec, no PR numbers. Keep what the note doesn't contradict. Never invent beyond the rule and the note.

Your reply is one JSON object: {"rules": {"<id>": "<the rewritten rule>"}}"#;

/// What a sign-off does for `one`, in order: drops vetoed, noted rules
/// rewritten, lessons kept, everything reviewed remembered as signed, the
/// branch pushed and its pull request kept up to date; then the kept
/// promotions proposed in the agent docs. The flash's line.
async fn run_sign_off(one: &SignOff) -> Result<String, String> {
    let local = Local::of(&one.id);
    let share = runner::sharer(&local, &one.root, &one.name).await?;
    let login = runner::gh_login().await.unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();
    let branch = share.open().await?;
    let store = branch.store();
    let entries: BTreeMap<String, Entry> = store
        .entries()
        .into_iter()
        .map(|e| (e.id.clone(), e))
        .collect();
    let mut lessons = Vec::new();
    let (mut dropped, mut rewritten) = (0usize, 0usize);
    for v in one.verdicts.iter().filter(|v| !v.keep) {
        let Some(e) = entries.get(&v.id) else {
            continue;
        };
        store.retire(&v.id, &one.name, store::VETOED, v.note.trim())?;
        dropped += 1;
        lessons.push(lesson(e, store::LESSON_DROPPED, &v.note, &login, &now));
    }
    // The kept rules with a note, by topic: one rewrite a topic.
    let mut by_topic: BTreeMap<String, Vec<(&Entry, &str)>> = BTreeMap::new();
    for v in one
        .verdicts
        .iter()
        .filter(|v| v.keep && !v.note.trim().is_empty())
    {
        if let Some(e) = entries.get(&v.id) {
            by_topic
                .entry(e.topic.clone())
                .or_default()
                .push((e, v.note.trim()));
        }
    }
    if !by_topic.is_empty() {
        match ClaudeCli::from_config() {
            Ok(model) => {
                for (topic, notes) in &by_topic {
                    match rewrite_rules(&model, topic, store.page(topic).as_deref(), notes).await {
                        Ok(new) => {
                            for (id, rule) in new {
                                if store.rewrite(&id, &rule).is_ok() {
                                    rewritten += 1;
                                }
                            }
                        }
                        Err(why) => {
                            tracing::warn!("knowledge: rewriting {topic}'s rules failed: {why}")
                        }
                    }
                }
            }
            Err(why) => tracing::warn!("knowledge: no model to rewrite noted rules: {why}"),
        }
        for notes in by_topic.values() {
            for (e, note) in notes {
                lessons.push(lesson(e, store::LESSON_REWRITTEN, note, &login, &now));
            }
        }
    }
    if !lessons.is_empty() {
        store.append_lessons(&lessons)?;
    }
    if dropped > 0 || rewritten > 0 {
        store.write_index(&one.name)?;
    }
    let reviewed: Vec<String> = one.verdicts.iter().map(|v| v.id.clone()).collect();
    let message = format!(
        "knowledge: signed off {} ({dropped} dropped, {rewritten} rewritten)",
        crate::flash::plural(one.verdicts.len(), "rule")
    );
    // Committed before anything counts as signed, so a failure here can't
    // leave a dropped rule signed off and back on the branch.
    share.save(&branch, &message).await?;
    let signed = State::update(&local, |s| {
        s.signed.extend(reviewed.iter().cloned());
        s.signed.clone()
    });
    // Anything recorded while the review was open is still unsigned: it
    // goes up with the rest but holds the merge for the next review.
    let unsigned_left =
        !pending_of(&store.entries(), &share.base_entries().await, &signed).is_empty();
    let published = share.publish(&branch, &message).await?;
    drop(branch);
    note_published(&local, &published);
    State::update(&local, |s| {
        s.signed_head = match unsigned_left {
            false => published.head.clone(),
            true => String::new(),
        };
        s.merge_failures = 0;
        for c in &one.decline {
            s.promote_declined.insert(c.key().to_string());
        }
        let gone: BTreeSet<&str> = one
            .decline
            .iter()
            .chain(one.promote.iter())
            .map(Candidate::key)
            .collect();
        s.promote.retain(|c| !gone.contains(c.key()));
    });
    let mut line = format!(
        "Signed off {} · {dropped} dropped · {rewritten} rewritten",
        crate::flash::plural(one.verdicts.len(), "rule")
    );
    if let Some(pr) = &published.pr {
        let auto = crate::config::Config::load().factory.knowledge_auto_merge;
        line.push_str(&match (auto, unsigned_left) {
            (true, false) => format!(" · #{} merges once checks pass", pr.number),
            (_, true) => format!(" · #{} waits for the rules recorded meanwhile", pr.number),
            (false, false) => format!(" · #{} is yours to merge", pr.number),
        });
    }
    if !one.promote.is_empty() {
        match promote(&local, &share, &one.name, &one.promote).await {
            Ok(Some(pr)) => line.push_str(&format!(" · agent docs in #{}", pr.number)),
            Ok(None) => {}
            Err(why) => {
                tracing::warn!(
                    "knowledge: promoting into {}'s agent docs failed: {why}",
                    one.name
                );
                line.push_str(" · the agent docs promotion failed (see the log)");
            }
        }
    }
    Ok(line)
}

fn lesson(e: &Entry, verdict: &str, note: &str, by: &str, at: &str) -> Lesson {
    Lesson {
        id: e.id.clone(),
        pr: e.pr,
        topic: e.topic.clone(),
        rule: e.rule.clone(),
        verdict: verdict.to_string(),
        note: note.trim().to_string(),
        by: by.to_string(),
        at: at.to_string(),
    }
}

/// Ask `model` to rewrite `notes`' rules (one topic's) to fit each note:
/// `(id, new rule)` for each it rewrote.
pub(crate) async fn rewrite_rules(
    model: &dyn Model,
    topic: &str,
    page: Option<&str>,
    notes: &[(&Entry, &str)],
) -> Result<Vec<(String, String)>, String> {
    let list: Vec<Value> = notes
        .iter()
        .map(|(e, note)| json!({"id": e.id, "rule": e.rule, "why": e.why, "quote": e.quote, "owner_note": note}))
        .collect();
    let prompt = format!(
        "Topic: {topic}\n\nIts page:\n{}\n\nThe rules to rewrite, each with the owner's note:\n{}\n\n{JSON_ONLY}",
        page.unwrap_or("(none)"),
        serde_json::to_string_pretty(&list).unwrap_or_default()
    );
    let ask = ModelAsk {
        system: format!("{REWRITE_SYSTEM}\n\n{JSON_ONLY}"),
        prompt,
        cwd: None,
        read_tools: false,
        step: Step::Harvest,
    };
    let v = harvest::ask_json(model, ask, |v| v.get("rules").is_some_and(Value::is_object)).await?;
    let asked: BTreeSet<&str> = notes.iter().map(|(e, _)| e.id.as_str()).collect();
    Ok(v.get("rules")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(id, _)| asked.contains(id.as_str()))
        .filter_map(|(id, rule)| {
            let rule = rule.as_str()?.trim();
            (!rule.is_empty()).then(|| (id.clone(), rule.to_string()))
        })
        .collect())
}

// ---- the push with nothing to review ----

/// The projects with a push on its way.
static PUSHING: InFlight = InFlight::new();

/// Publish `branch` as `message` when every rule on it is signed off —
/// the sign-off's head then, so it may merge. False, nothing pushed, when
/// a rule waits on a review.
pub(crate) async fn publish_if_all_signed(
    local: &Local,
    share: &share::Sharer<'_>,
    branch: &share::Branch,
    message: &str,
) -> Result<bool, String> {
    let signed = State::update(local, |s| s.signed.clone());
    if !pending_of(
        &branch.store().entries(),
        &share.base_entries().await,
        &signed,
    )
    .is_empty()
    {
        return Ok(false);
    }
    let published = share.publish(branch, message).await?;
    note_published(local, &published);
    State::update(local, |s| s.signed_head = published.head.clone());
    Ok(true)
}

/// At the morning's review time with nothing waiting: push what was
/// committed since the last sign-off (a staleness check, a consolidation,
/// a promotion's retirement) — checked again with the branch held, so a
/// rule nobody signed off is never pushed.
pub(crate) fn push_if_waiting(local: Local, root: PathBuf, project: String) {
    let key = local.id.clone();
    spawn_once(&PUSHING, &key, async move {
        let result = async {
            let share = runner::sharer(&local, &root, &project).await?;
            let branch = share.open().await?;
            if share.unpushed(&branch).await {
                publish_if_all_signed(&local, &share, &branch, "knowledge: checks and tidying")
                    .await?;
            }
            Ok::<_, String>(())
        }
        .await;
        if let Err(why) = result {
            tracing::warn!("knowledge: pushing {project}'s waiting commits failed: {why}");
        }
    });
}

/// After a push from the CLI (`orion knowledge backfill`, `consolidate`)
/// of `branch`: the rules of `prs` count as signed off — you ran it — and,
/// when nothing else on the branch waits on a review, the pushed `head`
/// may merge as a sign-off's would.
pub(crate) fn after_cli_push(local: &Local, base: &str, branch: &str, prs: &[u64], head: &str) {
    let Some((ours, theirs)) = branch_and_base(local, base, branch) else {
        return;
    };
    State::update(local, |s| {
        let on_base: BTreeSet<&str> = theirs.iter().map(|e| e.id.as_str()).collect();
        s.signed.extend(
            ours.iter()
                .filter(|e| prs.contains(&e.pr) && !on_base.contains(e.id.as_str()))
                .map(|e| e.id.clone()),
        );
        if !head.is_empty() && pending_of(&ours, &theirs, &s.signed).is_empty() {
            s.signed_head = head.to_string();
        }
    });
}

// ---- promotions ----

/// Whether the weekly promotion scan is due at `now_ms`, moving the next
/// one on when it is. The first sight of a project starts its week.
pub(crate) fn promote_due(state: &mut State, now_ms: i64) -> bool {
    runner::cadence_due(&mut state.promote_next, now_ms, PROMOTE_EVERY_MS)
}

/// The scan's rules.
const PROMOTE_SYSTEM: &str = r#"You read a repository's two kinds of guidance for autonomous coding agents. Its agent docs are loaded into every agent's context, so they hold what is true everywhere. Its knowledge base holds rules by area, read only when an agent touches that area.

Pick the knowledge-base rules that belong in the agent docs: repo-wide engineering principles, true well outside the area they were recorded in, that every agent should always know (for example: go through the registry, never hardcode a list; store intent such as theme references, never resolved values; fix the shared component, never fork a copy; one source of truth for X). Never pick a rule about one feature, one screen or one vendor, and never one the agent docs already say. Several rules that state one principle are one pick citing every id.

Pick at most 5; fewer is usual, none is fine. Your reply is one JSON object: {"candidates": [{"ids": ["k12-1"], "rule": "<the principle as the agent docs should say it, one sentence, at most 30 words>", "why": "<at most 15 words>"}]}"#;

/// The projects with a scan on its way.
static SCANNING: InFlight = InFlight::new();

/// Run the promotion scan for the project at `root` off the loop, one at
/// a time a project.
pub(crate) fn start_promote_scan(local: Local, root: PathBuf, project: String) {
    let key = local.id.clone();
    spawn_once(&SCANNING, &key, async move {
        match scan(&local, &root, &project).await {
            Ok(n) => tracing::info!("knowledge: promotion scan of {project} found {n}"),
            Err(why) => tracing::warn!("knowledge: promotion scan of {project} failed: {why}"),
        }
    });
}

/// The repo's always-loaded agent docs at `repo`: `AGENTS.md`, else
/// `CLAUDE.md`, cut to [`DOCS_CHARS`].
fn agent_docs(repo: &Path) -> String {
    ["AGENTS.md", "CLAUDE.md"]
        .iter()
        .find_map(|f| std::fs::read_to_string(repo.join(f)).ok())
        .map(|t| crate::ui::truncate(&t, DOCS_CHARS))
        .unwrap_or_default()
}

/// Every rule in force on `store`'s pages, with the ids it stands on.
fn page_rules(store: &Store) -> Vec<(Vec<String>, String, String)> {
    let mut out = Vec::new();
    for (topic, page) in store.pages() {
        let mut counting = false;
        for line in page.lines() {
            if let Some(head) = line.strip_prefix("## ") {
                counting = matches!(head.trim(), "Rules in force" | "Constraints");
                continue;
            }
            if !counting || !line.starts_with("- ") {
                continue;
            }
            let ids: Vec<String> = store::line_ids(line)
                .into_iter()
                .map(str::to_string)
                .collect();
            if ids.is_empty() {
                continue;
            }
            let text = line
                .split("<!--")
                .next()
                .unwrap_or(line)
                .trim_start_matches("- ")
                .trim();
            out.push((ids, text.to_string(), topic.clone()));
        }
    }
    out
}

/// The scan: the pages' rules and the agent docs to the model; what it
/// picks (not declined, not offered or proposed already, every id on a
/// page) kept for the next review. How many it added.
async fn scan(local: &Local, root: &Path, project: &str) -> Result<usize, String> {
    let model = ClaudeCli::from_config()?;
    let share = runner::sharer(local, root, project).await?;
    let branch = share.open().await?;
    let store = branch.store();
    let docs = agent_docs(&branch.clone);
    let state = State::update(local, |s| s.clone());
    let taken: BTreeSet<String> = state
        .promote
        .iter()
        .flat_map(|c| c.ids.iter().cloned())
        .chain(state.docs_pr.iter().flat_map(|d| d.ids.iter().cloned()))
        .chain(state.promote_declined.iter().cloned())
        .collect();
    let rules: Vec<(Vec<String>, String, String)> = page_rules(&store)
        .into_iter()
        .filter(|(ids, _, _)| !ids.iter().any(|id| taken.contains(id)))
        .collect();
    drop(branch);
    let room = PROMOTE_MAX.saturating_sub(state.promote.len());
    if rules.is_empty() || room == 0 {
        return Ok(0);
    }
    let listing = rules
        .iter()
        .map(|(ids, text, topic)| format!("- [{}] {text} ({topic})", ids.join(" ")))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "The agent docs (always loaded):\n{}\n\nThe knowledge base's rules, each with the ids it stands on:\n{listing}\n\n{JSON_ONLY}",
        match docs.trim() {
            "" => "(none)",
            d => d,
        }
    );
    let ask = ModelAsk {
        system: format!("{PROMOTE_SYSTEM}\n\n{JSON_ONLY}"),
        prompt,
        cwd: None,
        read_tools: false,
        step: Step::Harvest,
    };
    let v = harvest::ask_json(&model, ask, |v| {
        v.get("candidates").is_some_and(Value::is_array)
    })
    .await?;
    let known: BTreeSet<&str> = rules
        .iter()
        .flat_map(|(ids, _, _)| ids.iter().map(String::as_str))
        .collect();
    let found = candidates_from(&v, &known);
    let added = State::update(local, |s| {
        let mut added = 0;
        for c in found.into_iter().take(room) {
            if !s.promote.iter().any(|p| p.key() == c.key()) {
                s.promote.push(c);
                added += 1;
            }
        }
        added
    });
    Ok(added)
}

/// The scan's reply as candidates: each with a rule and ids all in
/// `known`.
pub(crate) fn candidates_from(v: &Value, known: &BTreeSet<&str>) -> Vec<Candidate> {
    v.get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let ids: Vec<String> = c
                .get("ids")?
                .as_array()?
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect();
            let rule = c.get("rule")?.as_str()?.trim().to_string();
            let why = c
                .get("why")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            (!ids.is_empty() && !rule.is_empty() && ids.iter().all(|i| known.contains(i.as_str())))
                .then_some(Candidate { ids, rule, why })
        })
        .collect()
}

/// The promotion call's rules.
const PLACE_SYSTEM: &str = r#"You add rules to a repository's agent docs: the instructions every coding agent working in it reads. You are in a checkout of the repository. Read AGENTS.md (or CLAUDE.md when that is the file) and the docs it links.

Put each rule where it fits: in AGENTS.md, or in the linked agent doc for that subject. Match the doc's voice and format: one bullet or sentence per rule. If the doc already says something close, sharpen that line instead of adding another. When AGENTS.md and CLAUDE.md hold the same text, make the same edit in both. Edit only Markdown files outside .orion/, and change nothing else.

After editing, your reply is one JSON object: {"placed": [{"ids": ["k12-1"], "path": "<the file you added it to>"}]}"#;

/// Move `promote` into the repo's agent docs: the agent docs branch made
/// on the base, the model adding each rule where it fits, the change
/// proposed in its own pull request — remembered, with the entries it
/// promotes, for [`start_docs_merge`]. None when nothing changed.
async fn promote(
    local: &Local,
    share: &share::Sharer<'_>,
    project: &str,
    promote: &[Candidate],
) -> Result<Option<Pr>, String> {
    let model = ClaudeCli::from_config()?;
    let (lock, name) = share.open_docs_branch().await?;
    let listing = promote
        .iter()
        .map(|c| format!("- [{}] {}", c.ids.join(" "), c.rule))
        .collect::<Vec<_>>()
        .join("\n");
    let ask = ModelAsk {
        system: format!("{PLACE_SYSTEM}\n\n{JSON_ONLY}"),
        prompt: format!("The rules to add, each with its ids:\n{listing}\n\n{JSON_ONLY}"),
        cwd: Some(local.clone_dir()),
        read_tools: true,
        step: Step::Harvest,
    };
    let reply = model.ask_editing(ask).await?;
    let placed =
        crate::oneshot::parse_json_where(&reply, |v: &Value| v.get("placed").is_some()).ok();
    let paths: Vec<String> = placed
        .as_ref()
        .and_then(|v| v.get("placed"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("path").and_then(Value::as_str).map(str::to_string))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let body = format!(
        "Rules the owner's review in orion moved from {project}'s harvested knowledge base into the agent docs, which every agent reads:\n\n{listing}\n\nOnce this merges, orion retires the harvested copies so each rule lives in one place."
    );
    let message = format!(
        "agent docs: promote {} from the knowledge base",
        crate::flash::plural(promote.len(), "rule")
    );
    let proposed = share.propose_docs(&name, &message, &body).await;
    drop(lock);
    let Some((pr, head)) = proposed? else {
        return Ok(None);
    };
    let ids: Vec<String> = promote.iter().flat_map(|c| c.ids.iter().cloned()).collect();
    State::update(local, |s| {
        let mut docs = s.docs_pr.take().unwrap_or_default();
        docs.pr = pr.clone();
        docs.pushed = head.clone();
        docs.failures = 0;
        for id in &ids {
            if !docs.ids.contains(id) {
                docs.ids.push(id.clone());
            }
        }
        for p in &paths {
            if !docs.paths.contains(p) {
                docs.paths.push(p.clone());
            }
        }
        s.docs_pr = Some(docs);
    });
    Ok(Some(pr))
}

/// Merge the agent docs pull request `docs` off the loop, as the
/// knowledge one merges (with `auto` off, only notice it merged); once
/// merged, the harvested copies of what it
/// promoted are retired on the knowledge branch, citing the doc.
pub(crate) fn start_docs_merge(
    local: Local,
    root: PathBuf,
    project: String,
    docs: DocsPr,
    auto: bool,
) {
    let key = format!("{}:docs", local.id);
    spawn_once(&runner::MERGING, &key, async move {
        let result = async {
            let share = runner::sharer(&local, &root, &project).await?;
            // Switched off, it is yours to merge: orion only notices
            // when it has, and retires the copies then.
            let merged = match auto {
                true => {
                    let wanted = crate::config::Config::load().merge_method(&root);
                    share.merge_pr(&docs.pr, &docs.pushed, wanted).await
                }
                false => share
                    .readiness_of(&docs.pr, &docs.pushed)
                    .await
                    .map(|r| match r {
                        share::Ready::Merged => Merge::Merged,
                        share::Ready::Closed => Merge::Closed,
                        _ => Merge::Waiting("yours to merge".into()),
                    }),
            };
            match merged {
                Ok(Merge::Merged) => {
                    let branch = share.open().await?;
                    let store = branch.store();
                    let cite = match docs.paths.is_empty() {
                        true => "the agent docs".to_string(),
                        false => docs.paths.join(", "),
                    };
                    for id in &docs.ids {
                        let _ = store.retire(
                            id,
                            &project,
                            store::VETOED,
                            &format!("promoted to {cite}"),
                        );
                    }
                    store.write_index(&project)?;
                    share
                        .save(&branch, &format!("knowledge: promoted into {cite}"))
                        .await?;
                    State::update(&local, |s| s.docs_pr = None);
                    say(
                        true,
                        format!(
                            "Agent docs #{} merged: {} now in {cite}",
                            docs.pr.number,
                            crate::flash::plural(docs.ids.len(), "rule")
                        ),
                    );
                }
                Ok(Merge::Closed) => State::update(&local, |s| {
                    for id in &docs.ids {
                        s.promote_declined.insert(id.clone());
                    }
                    s.docs_pr = None;
                }),
                Ok(Merge::Waiting(_)) | Ok(Merge::Conflicting) => {}
                // GitHub's limit is no failure of the merge: it goes again
                // once the limit resets.
                Err(why) if runner::rate_limited(&why) => {
                    tracing::info!(
                        "knowledge: agent docs #{} waits for GitHub's limit",
                        docs.pr.number
                    );
                }
                Err(why) => {
                    let gave_up = State::update(&local, |s| {
                        let Some(d) = s.docs_pr.as_mut() else {
                            return false;
                        };
                        d.failures += 1;
                        let out = d.failures >= DOCS_TRIES;
                        if out {
                            s.docs_pr = None;
                        }
                        out
                    });
                    if gave_up {
                        say(
                            false,
                            format!(
                                "Agent docs #{} can't merge: {why}. Merge it yourself.",
                                docs.pr.number
                            ),
                        );
                    }
                }
            }
            Ok::<_, String>(())
        }
        .await;
        if let Err(why) = result {
            tracing::warn!("knowledge: agent docs merge for {project} failed: {why}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(id: &str, by: &str, at: &str) -> Entry {
        Entry {
            id: id.into(),
            by: by.into(),
            rule: format!("rule {id}"),
            recorded_at: at.into(),
            status: ACTIVE.into(),
            ..Entry::default()
        }
    }

    #[test]
    fn pending_is_the_branchs_new_active_unsigned_rules_the_owners_first() {
        let base = vec![entry("k1-1", "owner", "2026-11-01T10:00:00Z")];
        let mut dropped = entry("k3-2", "owner", "2026-11-18T10:00:00Z");
        dropped.status = store::VETOED.into();
        let branch = vec![
            base[0].clone(),
            entry("k3-1", "agent", "2026-11-18T09:00:00Z"),
            dropped,
            entry("k4-1", "owner", "2026-11-18T11:00:00Z"),
            entry("k4-2", "plan", "2026-11-18T10:30:00Z"),
            entry("k5-1", "owner", "2026-11-18T12:00:00Z"),
        ];
        let signed: BTreeSet<String> = ["k5-1".to_string()].into();
        let ids: Vec<String> = pending_of(&branch, &base, &signed)
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, ["k4-2", "k4-1", "k3-1"]);
    }

    #[test]
    fn the_cutoff_is_the_latest_nine_oclock() {
        let at = |h, m| {
            chrono::Local
                .with_ymd_and_hms(2026, 11, 19, h, m, 0)
                .unwrap()
        };
        let yesterday = chrono::Local
            .with_ymd_and_hms(2026, 11, 18, 9, 0, 0)
            .unwrap();
        assert_eq!(cutoff(at(8, 59)), yesterday);
        assert_eq!(cutoff(at(9, 0)), at(9, 0));
        assert_eq!(cutoff(at(23, 0)), at(9, 0));
        assert!(!past_review_time(at(8, 59)) && past_review_time(at(9, 0)));
    }

    #[test]
    fn a_project_is_due_for_a_rule_before_the_cutoff_or_a_promotion() {
        let cut = chrono::Local
            .with_ymd_and_hms(2026, 11, 19, 9, 0, 0)
            .unwrap();
        let before = cut - chrono::Duration::hours(1);
        let after = cut + chrono::Duration::hours(1);
        let mut p = ProjectPending {
            rules: vec![entry("k1-1", "owner", &after.to_rfc3339())],
            ..ProjectPending::default()
        };
        assert!(!p.due(cut), "only a rule recorded after nine");
        p.rules.push(entry("k1-2", "owner", &before.to_rfc3339()));
        assert!(p.due(cut));
        let q = ProjectPending {
            promote: vec![Candidate {
                ids: vec!["k1-1".into()],
                rule: "r".into(),
                why: String::new(),
            }],
            ..ProjectPending::default()
        };
        assert!(q.due(cut));
        assert_eq!(p.count() + q.count(), 3);
    }

    #[test]
    fn the_promotion_scan_is_weekly_from_first_sight() {
        let mut s = State::default();
        assert!(!promote_due(&mut s, 1000), "first sight starts the week");
        assert!(!promote_due(&mut s, 1000 + PROMOTE_EVERY_MS - 1));
        assert!(promote_due(&mut s, 1000 + PROMOTE_EVERY_MS));
        assert!(!promote_due(&mut s, 1000 + PROMOTE_EVERY_MS + 1));
    }

    #[test]
    fn candidates_need_a_rule_and_ids_on_the_pages() {
        let known: BTreeSet<&str> = ["k1-1", "k2-1"].into();
        let v = json!({"candidates": [
            {"ids": ["k1-1", "k2-1"], "rule": "Go through the registry.", "why": "repo-wide"},
            {"ids": ["k9-9"], "rule": "Made up."},
            {"ids": [], "rule": "No ids."},
            {"ids": ["k1-1"], "rule": "  "}
        ]});
        let found = candidates_from(&v, &known);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key(), "k1-1");
        assert_eq!(found[0].rule, "Go through the registry.");
    }

    struct Rewrites;
    impl Model for Rewrites {
        fn ask(&self, ask: ModelAsk) -> futures::future::BoxFuture<'_, Result<String, String>> {
            assert!(ask.prompt.contains("owner_note"), "{}", ask.prompt);
            Box::pin(async {
                Ok(json!({"rules": {"k1-1": "Selection always beats hover.", "k7-7": "not asked"}}).to_string())
            })
        }
    }

    #[tokio::test]
    async fn a_noted_rule_is_rewritten_and_nothing_unasked_is_taken() {
        let e = entry("k1-1", "owner", "");
        let out = rewrite_rules(
            &Rewrites,
            "editor/tables",
            None,
            &[(&e, "it's every element")],
        )
        .await
        .unwrap();
        assert_eq!(
            out,
            vec![(
                "k1-1".to_string(),
                "Selection always beats hover.".to_string()
            )]
        );
    }

    #[test]
    fn page_rules_read_the_rule_sections_with_their_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::of(dir.path());
        store
            .write_page(
                "editor/tables",
                "# editor/tables\nScope: tables\n\n## Rules in force\n- One rule. (#1) <!-- k1-1 k2-1 -->\n\n## Constraints\n- A limit. (#3) (agent call) <!-- k3-1 -->\n\n## Open questions\n- Asked? (#4) <!-- k4-1 -->\n\n## Replaced\n",
            )
            .unwrap();
        let rules = page_rules(&store);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].0, vec!["k1-1", "k2-1"]);
        assert_eq!(rules[0].1, "One rule. (#1)");
        assert_eq!(rules[1].2, "editor/tables");
    }
}
