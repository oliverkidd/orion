//! PR BODY AND VERDICT: orion writes the top of every factory pull
//! request — one whose branch the bug fixer or the feature builder cut
//! (the builder LEDGER, `automations::linear_builder`) — from facts, and
//! sends the owner a card with its verdict: Merge, Skim, Review or
//! Blocked.
//!
//! - **Watching.** Each GIT POLL, [`due_fetches`] asks for the body of
//!   every factory PR in the projects' open lists whose row moved (a push,
//!   its checks), whose facts were just handed over (`orion pr facts
//!   set`), or that hasn't been read for [`REFRESH`] (Cubic's comments
//!   move no row).
//! - **Gathering.** Each body that lands passes [`land_detail`]. A factory
//!   PR with facts — from `<data_dir>/pr-facts/`, else the body's hidden
//!   block; a PR without facts is never touched — has the rest asked for
//!   off the loop ([`gather`]): the repo's contract on the base branch, the
//!   commits' messages, the pictures job's `pictures.json` on
//!   `pr-screenshots` (its grid downloaded for the card), and, until it is
//!   posted, the approved plan from the builder's transcript. A visible
//!   PR without pictures is looked at for pictures on their way, and one
//!   whose flow came after its push has the pictures job run for it, once
//!   per head ([`dispatch`]). A PR whose [`signature`] hasn't moved since
//!   its last gather isn't gathered again until its repo's contract is
//!   due a new read ([`CONTRACT_TTL`]); its commits' messages are read
//!   once per head.
//! - **Landing** ([`land`]). The verdict ([`verdict::verdict`]), the
//!   rows with their dots ([`rows::top`]) and the top
//!   ([`render::render`]); the body is written (`gh pr edit --body-file
//!   -`) only when its bytes differ. A visible PR whose pictures show no
//!   difference in any frame has its agent sent back, once per head, to
//!   stage the state its change affects ([`NO_DIFFERENCE`]). The plan
//!   goes up once as a "Plan as approved" comment. The verdict goes to
//!   the run record, and — once the checks have settled and no pictures
//!   are on their way — out as a card (`factory::cards`), edited in
//!   place as it changes; a PR gone Blocked has its card closed (the
//!   fixer says when it gives up). With `factory.auto_merge` on, a
//!   Merge-tier PR merges without a tap.
//! - **Merged or closed** anywhere — from the card, the PULL REQUESTS
//!   MODAL or GitHub — closes the card once ([`finished`]): a body that
//!   lands no longer open says so, and each GIT POLL looks at what `prs`
//!   knows of every PR whose card is up, asking again for the page of one
//!   that has left its project's open list ([`gone_fetches`]).
//! - **The card's buttons** come back through [`on_action`]: Merge (ready
//!   first if a draft, then `pr_actions`' merge), Send back… (to the
//!   branch's agent, [`send_back`]). Open PR is a link button, opened by
//!   Slack and never handed back. **Not for orion…** — one menu on
//!   Slack, its two choices — takes the ticket off and closes the draft;
//!   there is no vote on the verdict. Merge
//!   acts only while the card still stands ([`merge_stands`]): else the
//!   card says CI changed and is put back.

pub mod cli;
pub mod contract;
pub mod cubic;
pub mod dispatch;
pub mod facts;
pub mod pictures;
pub mod render;
pub mod rows;
pub mod trailers;
pub mod verdict;

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use orion_core::{AgentId, ProjectId};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::factory::cards::{self, escape, Button, Card, CardState, Press, Recipient};
use crate::pull_request::{CheckState, PrDetail, Standing};
use facts::Facts;
use verdict::{Kind, Tier, Verdict};

/// How long a factory PR goes unread when nothing on its row moves.
pub const REFRESH: Duration = Duration::from_secs(5 * 60);
/// How long a repo's contract is trusted before it is read again.
const CONTRACT_TTL: Duration = Duration::from_secs(10 * 60);
const GH_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a factory PR's facts file is looked at for a new hand-over.
const FACTS_EVERY: Duration = Duration::from_secs(30);
/// How long a PR is remembered after its card closed.
const FORGET_AFTER_MS: i64 = 7 * orion_core::clock::DAY_MS;

/// `factory` in the config file. No screen shows it yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FactorySettings {
    /// A Merge-tier factory PR merges without the owner's tap. Off.
    pub auto_merge: bool,
    /// orion merges its own knowledge pull request once the daily card's
    /// veto window has passed (`crate::knowledge`). On; off, the owner
    /// merges it by hand.
    pub knowledge_auto_merge: bool,
}

impl Default for FactorySettings {
    fn default() -> Self {
        FactorySettings {
            auto_merge: false,
            knowledge_auto_merge: true,
        }
    }
}

/// What `gh` says when what was asked for isn't there.
pub(crate) fn not_found(why: &str) -> bool {
    why.contains("HTTP 404") || why.contains("Not Found")
}

/// The data dir, or None under test so nothing reads or writes the real
/// user's files.
fn data_dir() -> Option<PathBuf> {
    (!cfg!(test)).then(orion_core::paths::data_dir)
}

/// Where `orion pr facts set` leaves facts.
pub fn facts_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("pr-facts"))
}

// ---- state ----

/// `App::pr_body`.
#[derive(Debug, Default)]
pub struct State {
    /// The loop's channel for what [`gather`] and the writes answer.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    /// `factory` in the config, mirrored (`event_loop::apply_config`).
    pub settings: FactorySettings,
    tracked: HashMap<String, Tracked>,
    /// Each repo's contract by `(owner/repo, base)`, and when it was read.
    contracts: HashMap<(String, String), (Instant, contract::Read)>,
    /// Each PR's commit messages, by URL, and the head they were read at.
    messages: HashMap<String, (String, Vec<String>)>,
    memory: Option<Memory>,
    /// `memory` changed since it was last written ([`State::flush`]).
    unsaved: bool,
}

/// One factory PR this run has seen.
#[derive(Debug, Default)]
struct Tracked {
    /// Its row as the open list last said it: a change asks again.
    row: String,
    facts_stamp: Option<SystemTime>,
    /// When its facts file was last looked at, and the stamp it had then.
    facts_seen: Option<(Instant, Option<SystemTime>)>,
    fetched: Option<Instant>,
    gathering: bool,
    /// What the last gather was for ([`signature`]).
    gathered: Option<u64>,
    /// The body orion last wrote.
    written: Option<String>,
}

/// What is remembered across runs, by PR url, in
/// `<data_dir>/automations/pr-body.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Memory {
    prs: BTreeMap<String, Remembered>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Remembered {
    /// The ticket (`RIPLO-12`): its run record.
    identifier: String,
    plan_posted: bool,
    /// The verdict line last put in the run record.
    recorded: Option<String>,
    /// The tier of the verdict last reached (`Tier::id`), carded or not.
    tier: Option<String>,
    card: Option<CardSent>,
    auto_merged: bool,
    /// The head the pictures job was last run for by hand ([`dispatch`]):
    /// never twice for one head.
    pictures_dispatched: Option<String>,
    /// When it was (epoch ms): its card waits [`dispatch::WAIT_MS`] for
    /// the pictures.
    pictures_dispatched_at_ms: Option<i64>,
    /// The head and flow the pictures job is known to have run for
    /// ([`dispatch::Ran`]): a flow uploaded again at the same head is run
    /// once more, the same flow never twice.
    pictures_ran: Option<dispatch::Ran>,
    /// The head its agent was last sent back for pictures that show no
    /// difference ([`NO_DIFFERENCE`]): never twice for one head.
    no_difference_sent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CardSent {
    tier: String,
    head_sha: String,
    closed: bool,
    /// When it closed (epoch ms): the PR is forgotten [`FORGET_AFTER_MS`]
    /// after.
    #[serde(default)]
    closed_at_ms: Option<i64>,
}

impl CardSent {
    fn close(&mut self) {
        self.closed = true;
        self.closed_at_ms = Some(orion_core::clock::now_ms());
    }
}

fn memory_path() -> Option<PathBuf> {
    (!cfg!(test)).then(|| crate::automations::state_dir().join("pr-body.json"))
}

impl State {
    fn memory(&mut self) -> &mut Memory {
        self.memory.get_or_insert_with(|| {
            let mut memory: Memory = memory_path()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            // A card closed before closings were timed is timed from now.
            let now = orion_core::clock::now_ms();
            for card in memory.prs.values_mut().filter_map(|r| r.card.as_mut()) {
                if card.closed && card.closed_at_ms.is_none() {
                    card.closed_at_ms = Some(now);
                }
            }
            memory
        })
    }

    fn remembered(&mut self, url: &str) -> &mut Remembered {
        self.memory().prs.entry(url.to_string()).or_default()
    }

    /// Mark `memory` to be written by the next [`State::flush`]: each
    /// entry point writes once, however many changes it made.
    fn save(&mut self) {
        self.unsaved = true;
    }

    /// Write `memory` if it changed, the PRs whose card closed over
    /// [`FORGET_AFTER_MS`] ago forgotten first.
    fn flush(&mut self) {
        if !std::mem::take(&mut self.unsaved) {
            return;
        }
        self.forget_closed(orion_core::clock::now_ms());
        let Some(path) = memory_path() else {
            return;
        };
        if let Err(err) = crate::pr_cache::write_json_atomic(&path, self.memory()) {
            tracing::warn!("pr_body: writing {}: {err}", path.display());
        }
    }

    /// Forget every PR whose card closed [`FORGET_AFTER_MS`] before
    /// `now_ms`, here and in `tracked`.
    fn forget_closed(&mut self, now_ms: i64) {
        let mut gone = Vec::new();
        self.memory().prs.retain(|url, r| {
            let closed_at = r.card.as_ref().and_then(|c| c.closed_at_ms);
            let keep = closed_at.is_none_or(|at| now_ms.saturating_sub(at) < FORGET_AFTER_MS);
            if !keep {
                gone.push(url.clone());
            }
            keep
        });
        for url in gone {
            self.tracked.remove(&url);
            self.messages.remove(&url);
        }
    }
}

// ---- which PRs ----

/// A factory PR: the ticket built on its branch, and where.
#[derive(Debug, Clone)]
struct Factory {
    dir: PathBuf,
    project_name: String,
    identifier: String,
    /// The runner that built it, by name (`Bug fixer`).
    runner: String,
    kind: Option<Kind>,
    session: Option<AgentId>,
}

/// The factory PR at `url` on `head`, if it is one: the builder ledger
/// has a ticket on that branch, in the project whose open list has it
/// (else the only project with that branch in the ledger).
fn factory_of(app: &App, url: &str, head: &str) -> Option<Factory> {
    use crate::automations::{linear_builder, Automation};
    let listed: Option<&ProjectId> = app
        .open_prs
        .iter()
        .find(|(_, o)| o.list.iter().any(|pr| pr.url == url))
        .map(|(id, _)| id);
    if head.is_empty() {
        return None;
    }
    let entry = match listed {
        Some(project) => linear_builder::factory_entry(app, project, head)?,
        None => {
            let mut on_branch = app
                .tree
                .projects
                .iter()
                .filter_map(|p| linear_builder::factory_entry(app, &p.id, head));
            let first = on_branch.next()?;
            if on_branch.next().is_some() {
                return None;
            }
            first
        }
    };
    let project = app.tree.projects.iter().find(|p| p.id == entry.project)?;
    let which = Automation::from_id(&entry.automation);
    Some(Factory {
        dir: project.repo_path.clone(),
        project_name: project.name.clone(),
        identifier: entry.identifier.clone(),
        runner: which.map_or("Factory", |a| a.name()).to_string(),
        kind: match which {
            Some(Automation::BugFixer) => Some(Kind::Bug),
            Some(Automation::FeatureBuilder) => Some(Kind::Feature),
            _ => None,
        },
        session: entry.session.clone(),
    })
}

/// `https://github.com/<owner>/<repo>/pull/<n>`'s three parts.
pub fn parse_url(url: &str) -> Option<(String, String, u64)> {
    let rest = url.trim().strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let (owner, repo, pull, number) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    (pull == "pull" && !owner.is_empty() && !repo.is_empty())
        .then(|| Some((owner.to_string(), repo.to_string(), number.parse().ok()?)))
        .flatten()
}

// ---- watching ----

/// The factory PRs whose body is due, each made in flight here: `(url,
/// number, dir)`, for the loop's `spawn_pr_detail`.
pub fn due_fetches(app: &mut App) -> Vec<(String, u64, PathBuf)> {
    let due = due_fetches_unsaved(app);
    app.pr_body.flush();
    due
}

fn due_fetches_unsaved(app: &mut App) -> Vec<(String, u64, PathBuf)> {
    let factory = crate::automations::linear_builder::factory_branches(app);
    let mut rows: Vec<(String, u64, ProjectId, String)> = Vec::new();
    for (project, open) in &app.open_prs {
        for pr in &open.list {
            if factory.contains_key(&(project, pr.head.as_str())) {
                let row = format!("{}|{:?}|{}", pr.head_sha, pr.answered, pr.answered_draft);
                rows.push((pr.url.clone(), pr.number, project.clone(), row));
            }
        }
    }
    let now = Instant::now();
    let dir_of_facts = facts_dir();
    let mut due = gone_fetches(app);
    for (url, number, project, row) in rows {
        let Some(dir) = app
            .tree
            .projects
            .iter()
            .find(|p| p.id == project)
            .map(|p| p.repo_path.clone())
        else {
            continue;
        };
        let in_flight = app.pr_detail_inflight.in_flight(&url);
        let t = app.pr_body.tracked.entry(url.clone()).or_default();
        let stamp = match t.facts_seen {
            Some((at, stamp)) if now.duration_since(at) < FACTS_EVERY => stamp,
            _ => {
                let stamp = dir_of_facts
                    .as_deref()
                    .and_then(|d| facts::stamp_for(d, &url));
                t.facts_seen = Some((now, stamp));
                stamp
            }
        };
        let stale = t.fetched.is_none_or(|at| now.duration_since(at) >= REFRESH);
        if !(t.row != row || t.facts_stamp != stamp || stale) || t.gathering || in_flight {
            continue;
        }
        if t.facts_stamp != stamp {
            // New facts: gather even if nothing else moved.
            t.gathered = None;
        }
        t.row = row;
        t.facts_stamp = stamp;
        t.fetched = Some(now);
        due.push((url, number, dir));
    }
    due
}

/// The PRs whose card is still up, each looked at: one `prs` says has
/// merged or closed has its card closed now ([`finished`]); one that has
/// left its project's open list has its page asked for again — at most
/// every [`REFRESH`] — so [`land_detail`] learns which.
fn gone_fetches(app: &mut App) -> Vec<(String, u64, PathBuf)> {
    let up: Vec<String> = app
        .pr_body
        .memory()
        .prs
        .iter()
        .filter(|(_, r)| r.card.as_ref().is_some_and(|c| !c.closed))
        .map(|(url, _)| url.clone())
        .collect();
    let now = Instant::now();
    let mut due = Vec::new();
    for url in up {
        if let Some(standing) = app.prs.status(&url).map(|s| s.standing) {
            if !standing.is_open() {
                finished(app, &url, standing);
                continue;
            }
        }
        let Some(detail) = app.pr_detail.get(&url) else {
            continue;
        };
        let number = detail.number;
        let Some(factory) = factory_of(app, &url, &detail.head) else {
            continue;
        };
        let listed = app
            .tree
            .projects
            .iter()
            .filter(|p| p.repo_path == factory.dir)
            .any(|p| {
                app.open_prs
                    .get(&p.id)
                    .is_none_or(|open| open.list.iter().any(|pr| pr.url == url))
            });
        if listed || app.pr_detail_inflight.in_flight(&url) {
            continue;
        }
        let t = app.pr_body.tracked.entry(url.clone()).or_default();
        if t.fetched.is_some_and(|at| now.duration_since(at) < REFRESH) {
            continue;
        }
        t.fetched = Some(now);
        due.push((url, number, factory.dir));
    }
    due
}

/// What a body's gather was for: a change in any of it gathers again.
fn signature(detail: &PrDetail, facts: &Facts, written: Option<&str>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    detail.head_sha.hash(&mut h);
    format!("{:?}", detail.checks).hash(&mut h);
    format!("{:?}", detail.answered).hash(&mut h);
    detail.answered_draft.hash(&mut h);
    format!("{:?}", detail.comments).hash(&mut h);
    format!("{:?}", detail.threads).hash(&mut h);
    serde_json::to_string(facts)
        .unwrap_or_default()
        .hash(&mut h);
    match written {
        Some(w) if render::same(w, &detail.body) => "written".hash(&mut h),
        _ => detail.body.hash(&mut h),
    }
    h.finish()
}

/// Whether the plan is on the PR already.
fn has_plan_comment(detail: &PrDetail) -> bool {
    detail
        .comments
        .iter()
        .any(|c| c.body.trim_start().starts_with(PLAN_HEADING))
}

const PLAN_HEADING: &str = "## Plan as approved";

/// A body landed, already observed into `App::prs`. A factory PR with
/// facts has the rest of what its verdict needs asked for.
pub fn land_detail(app: &mut App, url: &str, detail: Option<&PrDetail>) {
    land_detail_unsaved(app, url, detail);
    app.pr_body.flush();
}

fn land_detail_unsaved(app: &mut App, url: &str, detail: Option<&PrDetail>) {
    let Some(detail) = detail else {
        return;
    };
    if detail.answered_state != crate::pull_request::STATE_OPEN {
        finished(app, url, Standing::of(&detail.answered_state, false));
        return;
    }
    let Some(factory) = factory_of(app, url, &detail.head) else {
        return;
    };
    let facts = facts_dir()
        .and_then(|d| facts::read_for(&d, url))
        .or_else(|| facts::from_body(&detail.body));
    let Some(facts) = facts else {
        return;
    };
    let Some((owner, repo, number)) = parse_url(url) else {
        return;
    };
    let Some(tx) = app.pr_body.tx.clone() else {
        return;
    };
    let now = Instant::now();
    let contract = app
        .pr_body
        .contracts
        .get(&(format!("{owner}/{repo}"), detail.base.clone()))
        .filter(|(at, _)| now.duration_since(*at) < CONTRACT_TTL)
        .map(|(_, read)| read.clone());
    let written = app.pr_body.tracked.get(url).and_then(|t| t.written.clone());
    let sig = signature(detail, &facts, written.as_deref());
    let t = app.pr_body.tracked.entry(url.to_string()).or_default();
    // Nothing it reads has moved, and the contract it was read against
    // still holds: the last gather stands.
    if t.gathering || (t.gathered == Some(sig) && contract.is_some()) {
        return;
    }
    t.gathering = true;
    t.gathered = Some(sig);
    let remembered = app.pr_body.remembered(url);
    if remembered.identifier != factory.identifier {
        remembered.identifier = factory.identifier.clone();
        app.pr_body.save();
    }
    let want_plan = !app.pr_body.remembered(url).plan_posted && !has_plan_comment(detail);
    let builder = want_plan
        .then_some(factory.session.as_ref())
        .flatten()
        .and_then(|id| app.tree.agents.iter().find(|a| &a.id == id))
        .cloned();
    let pictures_dispatched = app.pr_body.remembered(url).pictures_dispatched.clone();
    let pictures_ran = app.pr_body.remembered(url).pictures_ran.clone();
    let messages = app
        .pr_body
        .messages
        .get(url)
        .filter(|(head, _)| *head == detail.head_sha)
        .map(|(_, messages)| messages.clone());
    let job = Job {
        url: url.to_string(),
        owner,
        repo,
        number,
        detail: detail.clone(),
        facts,
        factory,
        contract,
        messages,
        builder,
        cache: data_dir().map(|d| d.join("pr-body")),
        pictures_dispatched,
        pictures_ran,
    };
    tokio::spawn(async move {
        let gathered = gather(job).await;
        let _ = tx.send(Answer::Gathered(Box::new(gathered)));
    });
}

/// The last plan the session put up for approval (`ExitPlanMode`'s
/// `plan`), from its transcript's lines.
pub fn approved_plan(transcript: &str) -> Option<String> {
    let mut plan = None;
    for line in transcript.lines() {
        if !line.contains(orion_core::EXIT_PLAN_MODE_TOOL) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for item in content {
            if item.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && item.get("name").and_then(|n| n.as_str())
                    == Some(orion_core::EXIT_PLAN_MODE_TOOL)
            {
                if let Some(text) = item.pointer("/input/plan").and_then(|p| p.as_str()) {
                    if !text.trim().is_empty() {
                        plan = Some(text.trim().to_string());
                    }
                }
            }
        }
    }
    plan
}

// ---- gathering ----

/// What [`land_detail`] hands [`gather`].
#[derive(Debug)]
struct Job {
    url: String,
    owner: String,
    repo: String,
    number: u64,
    detail: PrDetail,
    facts: Facts,
    factory: Factory,
    /// The cached contract, when fresh: not asked again.
    contract: Option<contract::Read>,
    /// The commit messages read at this head already: not asked again.
    messages: Option<Vec<String>>,
    /// The session that built it, while the plan is still to be posted:
    /// its transcript holds the plan.
    builder: Option<orion_core::Agent>,
    /// Where the grid picture is downloaded for the card.
    cache: Option<PathBuf>,
    /// The head the pictures job was last run for by hand.
    pictures_dispatched: Option<String>,
    /// The head and flow it is known to have run for.
    pictures_ran: Option<dispatch::Ran>,
}

/// Everything the verdict needs. A None is "couldn't ask": nothing is
/// written on a partial picture.
#[derive(Debug)]
pub struct Gathered {
    job: Job,
    /// Read now, rather than from the cache.
    fresh_contract: bool,
    contract: Option<contract::Read>,
    messages: Option<Vec<String>>,
    pictures: Option<pictures::Manifest>,
    grid_file: Option<PathBuf>,
    plan: Option<String>,
    /// What was found of pictures on their way, for a visible PR without
    /// them at its head; None when not looked at.
    look: Option<dispatch::Look>,
    /// How long the build took, from its run record
    /// ([`rows::build_ms`]).
    build_ms: Option<i64>,
}

/// What lands on the loop.
#[derive(Debug)]
pub enum Answer {
    Gathered(Box<Gathered>),
    /// A body written, or why not.
    Wrote {
        url: String,
        result: Result<(), String>,
    },
    PlanPosted {
        url: String,
        result: Result<(), String>,
    },
    Merged {
        url: String,
        result: Result<String, String>,
    },
}

/// `gh pr view`'s commits, each message whole.
async fn commit_messages(url: &str) -> Option<Vec<String>> {
    let out = crate::pull_request::gh(None, &["pr", "view", url, "--json", "commits"], GH_TIMEOUT)
        .await?;
    parse_commit_messages(&out)
}

fn parse_commit_messages(json: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(
        v.get("commits")?
            .as_array()?
            .iter()
            .map(|c| {
                let s = |k| c.get(k).and_then(|x| x.as_str()).unwrap_or_default();
                format!("{}\n\n{}", s("messageHeadline"), s("messageBody"))
            })
            .collect(),
    )
}

async fn gather(job: Job) -> Gathered {
    let fresh_contract = job.contract.is_none();
    let contract_fut = async {
        match &job.contract {
            Some(read) => Some(read.clone()),
            None => contract::fetch(&job.owner, &job.repo, &job.detail.base).await,
        }
    };
    let messages_fut = async {
        match &job.messages {
            Some(messages) => Some(messages.clone()),
            None => commit_messages(&job.url).await,
        }
    };
    let (contract, messages, manifest) = tokio::join!(
        contract_fut,
        messages_fut,
        pictures::manifest(&job.owner, &job.repo, job.number)
    );
    let mut grid_file = None;
    if let (Some(read), Some(found), Some(cache)) = (&contract, &manifest, &job.cache) {
        let visible = read
            .as_ref()
            .map(|c| c.visible_paths.clone())
            .unwrap_or_default();
        let signal = pictures::of(&job.detail, &visible, found);
        if let Some(grid) = signal.grid() {
            // One per PR and head: a new push's pictures are a new file.
            let to = pictures::grid_file(cache, job.number, &job.detail.head_sha);
            if to.is_file()
                || pictures::download(&job.owner, &job.repo, job.number, grid, &to).await
            {
                grid_file = Some(to);
            }
        }
    }
    let look = match (&contract, &manifest) {
        (Some(Ok(rules)), Some(found)) => {
            let signal = pictures::of(&job.detail, &rules.visible_paths, found);
            if signal.visible && !signal.shown {
                let may_run = dispatch::wanted(&job.detail, &rules.visible_paths, found);
                let ran = dispatch::ran_at_head(
                    &job.detail,
                    job.pictures_ran.as_ref(),
                    job.pictures_dispatched.as_deref(),
                    found,
                );
                let gh = dispatch::Gh {
                    owner: &job.owner,
                    repo: &job.repo,
                };
                let repo = format!("{}/{}", job.owner, job.repo);
                let workflow = rules.pictures_workflow.as_deref();
                Some(dispatch::look(&gh, &repo, &job.detail, workflow, may_run, ran.as_ref()).await)
            } else {
                None
            }
        }
        _ => None,
    };
    let identifier = job.factory.identifier.clone();
    let build_ms = match identifier.is_empty() {
        true => None,
        false => tokio::task::spawn_blocking(move || {
            crate::factory::runs::read(&identifier)
                .as_ref()
                .and_then(rows::build_ms)
        })
        .await
        .ok()
        .flatten(),
    };
    let plan = match job.builder.clone() {
        Some(agent) => {
            // Finding the transcript lists the account's project dirs.
            let found = tokio::task::spawn_blocking(move || {
                crate::automations::pr_fixer::transcript_of(&agent, None)
            });
            match found.await.ok().flatten() {
                Some(path) => tokio::fs::read_to_string(path)
                    .await
                    .ok()
                    .and_then(|text| approved_plan(&text)),
                None => None,
            }
        }
        None => None,
    };
    Gathered {
        job,
        fresh_contract,
        contract,
        messages,
        pictures: manifest,
        grid_file,
        plan,
        look,
        build_ms,
    }
}

// ---- landing ----

/// The top's sentence, rows and notes.
fn top(
    g: &Gathered,
    rules: &contract::Contract,
    cubic: &cubic::Cubic,
    pictures: &pictures::Pictures,
    kind: Kind,
) -> rows::Top {
    let detail = &g.job.detail;
    let total = verdict::Size {
        lines: detail.additions + detail.deletions,
        files: detail.changed_files.max(detail.files.len() as u64),
        added: detail.additions,
        removed: detail.deletions,
    };
    rows::top(&rows::Inputs {
        facts: &g.job.facts,
        size: verdict::size(detail),
        total,
        kind,
        rules,
        build_ms: g.build_ms,
        pictures,
        cubic,
        rule_files: verdict::touched(detail, &rules.rule_paths),
        risky: verdict::touched(detail, &rules.risky_paths),
        checks: detail.checks.iter().map(|c| c.name.as_str()).collect(),
    })
}

/// What the PR's agent is told when every frame of its pictures shows no
/// difference.
pub const NO_DIFFERENCE: &str = "Your pictures flow shows no visible difference in any frame: \
     set up the state your change affects (data, open menus, hover) and upload the flow again.";

/// Send the agent of the PR at `url` back for pictures that show no
/// difference, through the PR fixer's router — once for `head`.
fn send_back_no_difference(app: &mut App, url: &str, head: &str) {
    if head.is_empty() || app.pr_body.remembered(url).no_difference_sent.as_deref() == Some(head) {
        return;
    }
    let mut out = Vec::new();
    let sent = crate::automations::pr_fixer::send_words(
        app,
        url,
        NO_DIFFERENCE.to_string(),
        verdict::NO_DIFFERENCE,
        &mut out,
    );
    app.outbox.extend(out);
    if sent {
        tracing::info!("pr_body: sent {url} back at {head}: its pictures show no difference");
        app.pr_body.remembered(url).no_difference_sent = Some(head.to_string());
        app.pr_body.save();
    }
}

/// An answer lands.
pub fn land(app: &mut App, answer: Answer) {
    match answer {
        Answer::Gathered(g) => land_gathered(app, *g),
        Answer::Wrote { url, result } => {
            if let Err(why) = result {
                tracing::warn!("pr_body: writing the body of {url}: {why}");
                if let Some(t) = app.pr_body.tracked.get_mut(&url) {
                    t.written = None;
                    t.gathered = None;
                }
            }
        }
        Answer::PlanPosted { url, result } => {
            if let Err(why) = result {
                tracing::warn!("pr_body: posting the plan on {url}: {why}");
                app.pr_body.remembered(&url).plan_posted = false;
                app.pr_body.save();
            }
        }
        Answer::Merged { url, result } => merged(app, &url, result),
    }
    app.pr_body.flush();
    app.dirty = true;
}

fn land_gathered(app: &mut App, g: Gathered) {
    let url = g.job.url.clone();
    if let Some(t) = app.pr_body.tracked.get_mut(&url) {
        t.gathering = false;
    }
    if let Some(look) = &g.look {
        let head = g.job.detail.head_sha.trim().to_string();
        let taken_here = g
            .pictures
            .as_ref()
            .is_some_and(|m| !head.is_empty() && m.head_sha.trim() == head);
        let remembered = app.pr_body.remembered(&url);
        let known_here = remembered
            .pictures_ran
            .as_ref()
            .is_some_and(|r| r.head == head);
        if look.dispatched {
            remembered.pictures_dispatched = Some(head.clone());
            remembered.pictures_dispatched_at_ms = Some(orion_core::clock::now_ms());
            remembered.pictures_ran = Some(dispatch::Ran {
                head,
                flow: look.flow_sha.clone(),
            });
            app.pr_body.save();
        } else if taken_here && !known_here && look.flow_sha.is_some() {
            // Taken at this head by a run orion didn't start: with the
            // flow that is up now.
            remembered.pictures_ran = Some(dispatch::Ran {
                head,
                flow: look.flow_sha.clone(),
            });
            app.pr_body.save();
        }
    }
    let (Some(read), Some(messages), Some(manifest)) = (&g.contract, &g.messages, &g.pictures)
    else {
        // Couldn't ask for all of it: try again on the next look.
        if let Some(t) = app.pr_body.tracked.get_mut(&url) {
            t.gathered = None;
        }
        return;
    };
    app.pr_body.messages.insert(
        url.clone(),
        (g.job.detail.head_sha.clone(), messages.clone()),
    );
    if g.fresh_contract {
        app.pr_body.contracts.insert(
            (
                format!("{}/{}", g.job.owner, g.job.repo),
                g.job.detail.base.clone(),
            ),
            (Instant::now(), read.clone()),
        );
    }
    let detail = &g.job.detail;
    let facts = &g.job.facts;
    let fallback = contract::Contract::default();
    let rules = read.as_ref().unwrap_or(&fallback);
    let mut texts: Vec<&str> = vec![detail.body.as_str()];
    texts.extend(detail.comments.iter().map(|c| c.body.as_str()));
    let (dismissed, unresolved) = cubic::declarations(texts);
    let cubic = cubic::Cubic {
        open: cubic::findings(&detail.threads),
        dismissed,
        unresolved,
    };
    let hooks = trailers::hooks(messages.iter().map(String::as_str));
    let pics = pictures::of(detail, &rules.visible_paths, manifest);
    let kind = g
        .job
        .factory
        .kind
        .unwrap_or_else(|| Kind::of_title(&detail.title));
    let v = verdict::verdict(facts, detail, read, &cubic, &hooks, &pics, kind);
    let remembered = app.pr_body.remembered(&url);
    if remembered.tier.as_deref() != Some(v.tier.id()) {
        remembered.tier = Some(v.tier.id().to_string());
        app.pr_body.save();
    }
    let grid = pics
        .shown
        .then(|| pics.grid().map(|p| p.url.clone()))
        .flatten();
    let top = top(&g, rules, &cubic, &pics, kind);
    let body = render::render(&detail.body, facts, &v, &top, grid.as_deref());
    if !render::same(&body, &detail.body) {
        write_body(app, &url, &body);
    }
    if let Some(plan) = &g.plan {
        post_plan(app, &url, plan);
    }
    record(app, &url, &g.job.factory.identifier, &v, &cubic);
    let remembered = app.pr_body.remembered(&url);
    let pictures_coming = g.look.as_ref().is_some_and(|look| {
        dispatch::coming(
            look,
            &detail.head_sha,
            remembered.pictures_dispatched.as_deref(),
            remembered.pictures_dispatched_at_ms,
            orion_core::clock::now_ms(),
        )
    });
    let settled = !detail.checks.iter().any(|c| c.state == CheckState::Running) && !pictures_coming;
    if pics.visible && pics.no_difference && !pictures_coming {
        send_back_no_difference(app, &url, &detail.head_sha);
    }
    card(app, &g, &v, &top, g.grid_file.as_deref(), settled);
    if settled && v.tier == Tier::Merge && app.pr_body.settings.auto_merge {
        let remembered = app.pr_body.remembered(&url);
        if !remembered.auto_merged {
            remembered.auto_merged = true;
            app.pr_body.save();
            tracing::info!("pr_body: auto-merging {url} (Merge tier, factory.auto_merge on)");
            merge(app, &url);
        }
    }
}

/// `gh pr edit <url> --body-file -`, off the loop.
fn write_body(app: &mut App, url: &str, body: &str) {
    let Some(tx) = app.pr_body.tx.clone() else {
        return;
    };
    if let Some(t) = app.pr_body.tracked.get_mut(url) {
        t.written = Some(body.to_string());
    }
    let (url, body) = (url.to_string(), body.to_string());
    tokio::spawn(async move {
        let mut cmd = tokio::process::Command::new("gh");
        cmd.args(["pr", "edit", url.as_str(), "--body-file", "-"]);
        let result = crate::pull_request::run_piped(cmd, &body, GH_TIMEOUT)
            .await
            .map(|_| ());
        let _ = tx.send(Answer::Wrote { url, result });
    });
}

/// The approved plan as a comment, once.
fn post_plan(app: &mut App, url: &str, plan: &str) {
    let Some(tx) = app.pr_body.tx.clone() else {
        return;
    };
    let remembered = app.pr_body.remembered(url);
    if remembered.plan_posted {
        return;
    }
    remembered.plan_posted = true;
    app.pr_body.save();
    let url = url.to_string();
    let body = format!("{PLAN_HEADING}\n\n{plan}\n");
    tokio::spawn(async move {
        let mut cmd = tokio::process::Command::new("gh");
        cmd.args(["pr", "comment", url.as_str(), "--body-file", "-"]);
        let result = crate::pull_request::run_piped(cmd, &body, GH_TIMEOUT)
            .await
            .map(|_| ());
        let _ = tx.send(Answer::PlanPosted { url, result });
    });
}

/// The verdict into the run record, when it changed.
fn record(app: &mut App, url: &str, identifier: &str, v: &Verdict, cubic: &cubic::Cubic) {
    let line = v.line();
    let remembered = app.pr_body.remembered(url);
    if remembered.recorded.as_deref() == Some(line.as_str()) || identifier.is_empty() {
        return;
    }
    remembered.recorded = Some(line);
    app.pr_body.save();
    let tier = v.tier.id().to_string();
    let why: Vec<String> = v.why().into_iter().map(String::from).collect();
    let dismissals = cubic.dismissed.clone();
    let url = url.to_string();
    crate::factory::runs::update(identifier, move |r| {
        r.pr_url = Some(url);
        r.verdict = Some(tier);
        r.verdict_reasons = why;
        r.cubic_dismissals = dismissals;
    });
}

fn card_key(url: &str) -> String {
    format!("pr:{url}:verdict")
}

/// The verdict card for a PR, per the card catalogue.
/// The verdict line, the rows with their dots, then what users get and
/// the link: the card leads with the dots, as the PR's top does.
fn verdict_card(
    url: &str,
    detail: &PrDetail,
    top: &rows::Top,
    v: &Verdict,
    factory: &Factory,
    image: Option<&Path>,
) -> Card {
    let mut body = vec![format!(
        "*{}*: {}",
        v.tier.word(),
        escape(&v.why().join("; "))
    )];
    if !top.rows.is_empty() {
        let rows: Vec<String> = top
            .rows
            .iter()
            .map(|r| format!("{} *{}*: {}", r.dot.mark(), r.label, escape(&r.cell)))
            .collect();
        body.push(rows.join("\n"));
    }
    let sentence = top.sentence.trim();
    if !sentence.is_empty() {
        body.push(escape(sentence));
    }
    body.push(format!("<{url}|#{} on GitHub>", detail.number));
    if v.tier == Tier::Review {
        let look: Vec<String> = v
            .why()
            .iter()
            .map(|why| format!("• Look at: {}", escape(why)))
            .collect();
        body.push(look.join("\n"));
    }
    // The card asks whether it merges; **Not for orion…** (a menu on
    // Slack) is the way out, and there is no vote on the verdict.
    let merge = Button::tap("merge", "Merge");
    let open = Button::link("Open PR", url);
    let send = Button::text("send_back", "Send back…");
    let mut buttons = match v.tier {
        Tier::Merge => vec![merge, open],
        Tier::Skim => vec![merge, open, send],
        _ => vec![open, merge, send],
    };
    buttons.extend(crate::automations::linear_builder::owner::not_for_the_factory_buttons());
    Card {
        key: card_key(url),
        to: Recipient::Owner,
        context: format!("{} · {}", factory.runner, factory.project_name),
        title: format!("#{} {}", detail.number, detail.title),
        body,
        buttons,
        image: (v.tier == Tier::Skim)
            .then(|| image.map(Path::to_path_buf))
            .flatten(),
    }
}

/// Post, edit or close the PR's card. Merge, Skim and Review go out once
/// `settled` — the checks have, and no pictures are on their way — and
/// again when the tier or the head moves; Blocked closes an open card
/// (the fixer's own card says when it gives up).
fn card(
    app: &mut App,
    g: &Gathered,
    v: &Verdict,
    top: &rows::Top,
    image: Option<&Path>,
    settled: bool,
) {
    let url = &g.job.url;
    let detail = &g.job.detail;
    let key = card_key(url);
    let sent = app.pr_body.remembered(url).card.clone();
    if v.tier == Tier::Blocked {
        if let Some(sent) = sent.filter(|s| !s.closed) {
            cards::close(app, &key, &format!("Blocked: {}", v.why().join("; ")));
            let mut sent = sent;
            sent.close();
            app.pr_body.remembered(url).card = Some(sent);
            app.pr_body.save();
        }
        return;
    }
    if !settled {
        return;
    }
    let now = CardSent {
        tier: v.tier.id().to_string(),
        head_sha: detail.head_sha.clone(),
        closed: false,
        closed_at_ms: None,
    };
    if sent.as_ref() == Some(&now) {
        return;
    }
    let card = verdict_card(url, detail, top, v, &g.job.factory, image);
    cards::post(app, card);
    app.pr_body.remembered(url).card = Some(now);
    app.pr_body.save();
}

// ---- the card's buttons ----

/// A press on a `pr:` card (`factory::cards::on_action`).
pub(crate) fn on_action(app: &mut App, press: Press) {
    on_press(app, press);
    app.pr_body.flush();
}

fn on_press(app: &mut App, press: Press) {
    let Some(url) = press
        .key
        .strip_prefix("pr:")
        .and_then(|k| k.strip_suffix(":verdict"))
    else {
        tracing::warn!(
            "pr_body: a press on {}, which isn't a verdict card",
            press.key
        );
        return;
    };
    let url = url.to_string();
    match press.action {
        "merge" if press.by_owner => merge(app, &url),
        "send_back" if press.by_owner => {
            let text = press.text.unwrap_or_default();
            if !text.trim().is_empty() {
                send_back(app, &url, &text);
            }
        }
        orion_core::factory_card::NOT_FOR_FACTORY
        | orion_core::factory_card::NOT_FOR_FACTORY_BACKLOG
            if press.by_owner =>
        {
            let backlog = press.action == orion_core::factory_card::NOT_FOR_FACTORY_BACKLOG;
            let identifier = app.pr_body.remembered(&url).identifier.clone();
            crate::automations::linear_builder::owner::not_for_the_factory_pr(
                app,
                &url,
                &identifier,
                press.key,
                backlog,
            );
        }
        other => tracing::info!("pr_body: {other} on {url} (owner: {})", press.by_owner),
    }
}

/// The tiers whose card's Merge may act.
const MERGEABLE: [Tier; 3] = [Tier::Merge, Tier::Skim, Tier::Review];
/// What a Merge the card no longer stands behind says.
const STALE: &str = "CI changed since this card: open the PR";

/// Whether a Merge on the card for `url` still stands: the PR's verdict
/// now is Merge, Skim or Review, its card is still up, and that card was
/// posted for the head the PR is on now.
fn merge_stands(app: &mut App, url: &str) -> bool {
    let key = card_key(url);
    let card_up = cards::state_of(app, &key).is_some_and(CardState::is_up);
    let head = app.pr_detail.get(url).map(|d| d.head_sha.clone());
    let listed = app
        .open_prs
        .values()
        .flat_map(|o| o.list.iter())
        .find(|pr| pr.url == url)
        .map(|pr| pr.head_sha.clone());
    let Some(remembered) = app.pr_body.memory().prs.get(url) else {
        return false;
    };
    let tier_ok = MERGEABLE
        .iter()
        .any(|t| remembered.tier.as_deref() == Some(t.id()));
    let Some(sent) = remembered.card.as_ref().filter(|s| !s.closed) else {
        return false;
    };
    tier_ok
        && card_up
        && head.as_deref() == Some(sent.head_sha.as_str())
        && listed.is_none_or(|h| h.is_empty() || h == sent.head_sha)
}

/// A Merge that no longer stands: not merged, and the card says why — put
/// back up while its verdict still allows a merge (the next look posts it
/// for the new head), else closed.
fn refuse_merge(app: &mut App, url: &str) {
    tracing::info!("pr_body: not merging {url}: the card no longer stands");
    app.flash = Some(crate::flash::Flash::failed(format!("Not merged: {STALE}")));
    let Some(remembered) = app.pr_body.memory().prs.get_mut(url) else {
        return;
    };
    let tier_ok = MERGEABLE
        .iter()
        .any(|t| remembered.tier.as_deref() == Some(t.id()));
    // A card this module has no record of sending is left alone.
    let Some(sent) = remembered.card.as_mut() else {
        return;
    };
    if tier_ok && !sent.closed {
        put_back(app, url, &format!("⚠ {STALE}"));
        return;
    }
    sent.close();
    app.pr_body.save();
    let key = card_key(url);
    if app.factory_cards.iter().any(|p| p.card.key == key) {
        cards::close(app, &key, &format!("Not merged: {STALE}"));
    }
}

/// Post the PR's card again as it was — buttons and all — with
/// `warning` at its top, replacing the one there before. Not for a card
/// that was closed.
fn put_back(app: &mut App, url: &str, warning: &str) {
    let open = app
        .pr_body
        .memory()
        .prs
        .get(url)
        .and_then(|r| r.card.as_ref())
        .is_some_and(|c| !c.closed);
    let key = card_key(url);
    let Some(mut card) = app
        .factory_cards
        .iter()
        .find(|p| p.card.key == key)
        .map(|p| p.card.clone())
        .filter(|_| open)
    else {
        return;
    };
    card.body.retain(|line| !line.starts_with('⚠'));
    card.body.insert(0, warning.to_string());
    cards::post(app, card);
}

/// The PR at `url` has merged or closed (`standing`), wherever that
/// happened: its card, if still up, closed with which — once — and a
/// merge's time into the run record, when it has none.
fn finished(app: &mut App, url: &str, standing: Standing) {
    let note = match standing {
        Standing::Merged => "Merged",
        Standing::Closed => "Closed",
        Standing::Open | Standing::Draft => return,
    };
    let Some(remembered) = app.pr_body.memory().prs.get_mut(url) else {
        return;
    };
    let Some(sent) = remembered.card.as_mut().filter(|s| !s.closed) else {
        return;
    };
    sent.close();
    let identifier = remembered.identifier.clone();
    app.pr_body.save();
    cards::close_if_up(app, &card_key(url), note);
    if standing == Standing::Merged && !identifier.is_empty() {
        let at = crate::app::now_ms();
        crate::factory::runs::update(&identifier, |r| {
            r.merged_at_ms.get_or_insert(at);
        });
    }
}

/// Merge the PR at `url` the way the PULL REQUESTS MODAL does
/// (`pr_actions::merge_from_card`), marked ready first if a draft — when
/// its card still stands behind it ([`merge_stands`]).
fn merge(app: &mut App, url: &str) {
    if !merge_stands(app, url) {
        refuse_merge(app, url);
        return;
    }
    let Some(tx) = app.pr_body.tx.clone() else {
        return;
    };
    let Some(detail) = app.pr_detail.get(url) else {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "Can't merge {url}: its details haven't loaded"
        )));
        return;
    };
    let Some(factory) = factory_of(app, url, &detail.head) else {
        return;
    };
    let (number, head, base) = (detail.number, detail.head.clone(), detail.base.clone());
    let draft = app.pr_is_draft(url);
    app.flash = Some(crate::flash::Flash::working(format!("merging #{number}…")));
    let url = url.to_string();
    tokio::spawn(async move {
        let result =
            crate::pr_actions::merge_from_card(&factory.dir, number, draft, &head, &base).await;
        let _ = tx.send(Answer::Merged { url, result });
    });
}

fn merged(app: &mut App, url: &str, result: Result<String, String>) {
    match result {
        Ok(message) => {
            app.flash = Some(crate::flash::Flash::done(message));
            finished(app, url, Standing::Merged);
        }
        Err(why) => {
            app.flash = Some(crate::flash::Flash::failed(format!("Merge failed: {why}")));
        }
    }
}

/// Send the owner's words back to the agent on the PR's branch, through
/// the PR fixer's router (`automations::pr_fixer::send_words`).
pub(crate) fn send_back(app: &mut App, pr_url: &str, text: &str) {
    let words = format!("The owner sent this pull request back:\n\n{}", text.trim());
    let mut out = Vec::new();
    let sent = crate::automations::pr_fixer::send_words(app, pr_url, words, "send back", &mut out);
    app.outbox.extend(out);
    if !sent {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "Couldn't send back {pr_url}: it isn't open on any project"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orion_merges_its_knowledge_pr_unless_switched_off() {
        let read = |json: &str| serde_json::from_str::<FactorySettings>(json).unwrap();
        assert!(FactorySettings::default().knowledge_auto_merge);
        assert!(!FactorySettings::default().auto_merge);
        let older = read(r#"{"auto_merge": true}"#);
        assert!(
            older.auto_merge && older.knowledge_auto_merge,
            "a file from before the switch"
        );
        assert!(!read(r#"{"knowledge_auto_merge": false}"#).knowledge_auto_merge);
        let cfg: crate::config::Config =
            serde_json::from_str(r#"{"factory": {"knowledge_auto_merge": false}}"#).unwrap();
        assert!(!cfg.factory.knowledge_auto_merge);
    }

    #[test]
    fn a_pull_request_url_splits_into_its_parts() {
        assert_eq!(
            parse_url("https://github.com/RiploData/riplo-os/pull/2108"),
            Some(("RiploData".into(), "riplo-os".into(), 2108))
        );
        assert_eq!(parse_url("https://github.com/o/r/issues/1"), None);
        assert_eq!(parse_url("https://gitlab.com/o/r/pull/1"), None);
    }

    #[test]
    fn the_approved_plan_is_the_last_one_put_up() {
        let line = |plan: &str| {
            serde_json::json!({"type": "assistant", "message": {"content": [
                {"type": "text", "text": "here"},
                {"type": "tool_use", "name": "ExitPlanMode", "input": {"plan": plan}}
            ]}})
            .to_string()
        };
        let transcript = format!(
            "{}\n{{\"type\":\"user\"}}\nnot json ExitPlanMode\n{}\n",
            line("first plan"),
            line("## Plan\n1. do it")
        );
        assert_eq!(
            approved_plan(&transcript).as_deref(),
            Some("## Plan\n1. do it")
        );
        assert_eq!(approved_plan("{}"), None);
    }

    #[test]
    fn commit_messages_are_whole() {
        let json =
            r#"{"commits":[{"messageHeadline":"fix: x","messageBody":"Hook: e2e skipped"}]}"#;
        assert_eq!(
            parse_commit_messages(json).unwrap(),
            ["fix: x\n\nHook: e2e skipped"]
        );
    }

    #[test]
    fn the_card_follows_the_catalogue() {
        let factory = Factory {
            dir: PathBuf::new(),
            project_name: "riplo-os".into(),
            identifier: "RIPLO-1".into(),
            runner: "Bug fixer".into(),
            kind: Some(Kind::Bug),
            session: None,
        };
        let detail = PrDetail {
            number: 7,
            title: "Fix the login loop".into(),
            ..Default::default()
        };
        let top = rows::Top {
            sentence: "No more <loop>.".into(),
            rows: vec![
                rows::Row {
                    dot: rows::Dot::Green,
                    label: "Size",
                    cell: "+3 −1 in 1 file".into(),
                },
                rows::Row {
                    dot: rows::Dot::Amber,
                    label: "Your calls",
                    cell: "1: sign-in <wording>".into(),
                },
            ],
            notes: vec![],
        };
        let reason = |tier, text: &str| verdict::Reason {
            tier,
            text: text.into(),
        };
        let labels = |tier, why: &str| {
            let v = Verdict {
                tier,
                reasons: vec![reason(tier, why)],
            };
            let card = verdict_card(
                "https://github.com/o/r/pull/7",
                &detail,
                &top,
                &v,
                &factory,
                Some(Path::new("/g.png")),
            );
            assert_eq!(card.key, "pr:https://github.com/o/r/pull/7:verdict");
            assert_eq!(card.context, "Bug fixer · riplo-os");
            assert!(!card.body.iter().any(|l| l.contains("Verdict right")));
            assert!(card.body.contains(&"No more &lt;loop&gt;.".to_string()));
            // The dots lead, right under the verdict.
            assert_eq!(
                card.body[1],
                "🟢 *Size*: +3 −1 in 1 file\n🟡 *Your calls*: 1: sign-in &lt;wording&gt;"
            );
            assert_eq!(card.body[3], "<https://github.com/o/r/pull/7|#7 on GitHub>");
            (
                card.buttons
                    .iter()
                    .map(|b| b.label.clone())
                    .collect::<Vec<_>>(),
                card,
            )
        };
        let (off, back) = (
            "Not for orion: back to Triage",
            "Not for orion: move to Backlog",
        );
        let (merge, card) = labels(Tier::Merge, "nothing risky, CI green");
        assert_eq!(merge, ["Merge", "Open PR", off, back]);
        // The two choices are one menu on Slack.
        let blocks = cards::blocks(&card, "10:42");
        let actions = blocks.last().unwrap()["elements"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[2]["type"], "static_select");
        assert_eq!(actions[2]["placeholder"]["text"], "Not for orion…");
        assert_eq!(
            actions[2]["options"][1]["value"],
            "pr:https://github.com/o/r/pull/7:verdict|not_for_factory_backlog"
        );
        assert!(card.image.is_none());
        let (skim, card) = labels(Tier::Skim, "visible change, pictures below");
        assert_eq!(skim, ["Merge", "Open PR", "Send back…", off, back]);
        assert_eq!(card.image.as_deref(), Some(Path::new("/g.png")));
        assert!(card.buttons[2].asks_text);
        let (review, card) = labels(Tier::Review, "risky: src/auth.rs");
        assert_eq!(review, ["Open PR", "Merge", "Send back…", off, back]);
        assert!(card
            .body
            .iter()
            .any(|l| l == "• Look at: risky: src/auth.rs"));
    }

    fn gathered() -> Gathered {
        Gathered {
            job: Job {
                url: "https://github.com/o/r/pull/7".into(),
                owner: "o".into(),
                repo: "r".into(),
                number: 7,
                detail: PrDetail {
                    number: 7,
                    head_sha: "abc".into(),
                    ..Default::default()
                },
                facts: Facts::default(),
                factory: Factory {
                    dir: PathBuf::new(),
                    project_name: "riplo-os".into(),
                    identifier: String::new(),
                    runner: "Bug fixer".into(),
                    kind: None,
                    session: None,
                },
                contract: None,
                messages: None,
                builder: None,
                cache: None,
                pictures_dispatched: None,
                pictures_ran: None,
            },
            fresh_contract: false,
            contract: None,
            messages: None,
            pictures: None,
            grid_file: None,
            plan: None,
            look: None,
            build_ms: None,
        }
    }

    /// A visible PR (its `PR pictures` check ran) without pictures, read
    /// whole, its checks settled.
    fn visible_without_pictures() -> Gathered {
        let mut g = gathered();
        g.job.detail.checks = vec![crate::pull_request::PrCheck {
            name: pictures::CHECK.into(),
            workflow: String::new(),
            state: CheckState::Passed,
            word: "SUCCESS".into(),
            started: String::new(),
            completed: String::new(),
            url: String::new(),
        }];
        g.job.facts.gaps = vec!["No pictures: only shows with two sessions in one table".into()];
        g.contract = Some(Ok(contract::Contract::default()));
        g.messages = Some(vec![]);
        g.pictures = Some(pictures::Manifest::default());
        g
    }

    #[test]
    fn a_visible_change_without_pictures_is_a_review_card_unless_they_are_coming() {
        let flow = dispatch::Look {
            flow: true,
            ..Default::default()
        };
        // Its flow is up and a run for it is going: the card waits.
        let mut app = App::new();
        let mut g = visible_without_pictures();
        g.look = Some(dispatch::Look {
            going: true,
            ..flow.clone()
        });
        land_gathered(&mut app, g);
        assert!(app.outbox.is_empty(), "pictures on their way");
        assert_eq!(app.pr_body.remembered(URL).tier.as_deref(), Some("review"));
        // orion ran the job for this head just now: it waits too, for
        // half an hour.
        let mut g = visible_without_pictures();
        g.look = Some(dispatch::Look {
            dispatched: true,
            ..flow.clone()
        });
        land_gathered(&mut app, g);
        assert!(app.outbox.is_empty(), "the job just run");
        let mut g = visible_without_pictures();
        g.look = Some(flow.clone());
        land_gathered(&mut app, g);
        assert!(app.outbox.is_empty(), "within half an hour of the run");
        app.pr_body.remembered(URL).pictures_dispatched_at_ms =
            Some(orion_core::clock::now_ms() - dispatch::WAIT_MS);
        let mut g = visible_without_pictures();
        g.look = Some(flow);
        land_gathered(&mut app, g);
        assert_eq!(
            posted_card(&app).card.body[0],
            "*Review*: no pictures: only shows with two sessions in one table"
        );
        // Nothing on its way at all: a Review card at once.
        let mut app = App::new();
        land_gathered(&mut app, visible_without_pictures());
        assert!(posted_card(&app).card.body[0].starts_with("*Review*: no pictures"));
    }

    #[test]
    fn a_dispatch_is_remembered_with_its_flow_so_the_next_gather_skips_it() {
        let mut app = App::new();
        let mut g = gathered();
        g.look = Some(dispatch::Look {
            flow: true,
            flow_sha: Some("f1".into()),
            dispatched: true,
            ..Default::default()
        });
        land_gathered(&mut app, g);
        let remembered = app.pr_body.remembered(URL);
        assert_eq!(remembered.pictures_dispatched.as_deref(), Some("abc"));
        assert!(remembered.pictures_dispatched_at_ms.is_some());
        let ran = remembered.pictures_ran.clone().expect("what ran");
        assert!(ran.covers("abc", "f1"), "{ran:?}");
        assert!(!ran.covers("abc", "f2"), "a flow uploaded again");
        assert!(!ran.covers("def", "f1"), "a new push");
        // Remembered across runs.
        let json = serde_json::to_string(&app.pr_body.memory().clone()).unwrap();
        let back: Memory = serde_json::from_str(&json).unwrap();
        assert_eq!(back.prs[URL].pictures_dispatched.as_deref(), Some("abc"));
        assert_eq!(back.prs[URL].pictures_ran, Some(ran));
    }

    #[test]
    fn pictures_taken_at_the_head_remember_the_flow_they_were_taken_with() {
        let mut app = app_listing_the_pr();
        let mut g = showing_no_difference("abc");
        g.look = Some(dispatch::Look {
            flow: true,
            flow_sha: Some("f1".into()),
            ..Default::default()
        });
        land_gathered(&mut app, g);
        let ran = app.pr_body.remembered(URL).pictures_ran.clone();
        assert_eq!(
            ran,
            Some(dispatch::Ran {
                head: "abc".into(),
                flow: Some("f1".into()),
            })
        );
        // The agent uploads its flow again: the next look runs the job.
        let none = pictures::Manifest::default();
        let detail = PrDetail {
            head_sha: "abc".into(),
            ..Default::default()
        };
        let at_head = dispatch::ran_at_head(&detail, ran.as_ref(), None, &none).unwrap();
        assert!(!at_head.covers("abc", "f2"));
        assert!(at_head.covers("abc", "f1"));
    }

    /// An app with `URL` on its project's open list, at head `abc`: the
    /// fixer's router can reach it.
    fn app_listing_the_pr() -> App {
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
                list: vec![crate::pull_request::OpenPr {
                    number: 7,
                    title: "Add things".into(),
                    url: URL.into(),
                    answered_draft: false,
                    answered: Default::default(),
                    head: "feat".into(),
                    mine: true,
                    head_sha: "abc".into(),
                    meta: Default::default(),
                }],
                at: now,
                due: now,
                step: Duration::from_secs(15),
            },
        );
        app
    }

    /// A visible PR whose pictures, taken at `head`, show no difference in
    /// any frame.
    fn showing_no_difference(head: &str) -> Gathered {
        let mut g = visible_without_pictures();
        g.job.detail.head_sha = head.into();
        g.pictures = Some(pictures::Manifest {
            head_sha: head.into(),
            grids: vec![pictures::Grid {
                file: "grid-01.png".into(),
                url: "https://x/grid-01.png".into(),
            }],
            frames: vec![
                pictures::Frame {
                    verdict: "no visible difference".into(),
                    difference: Some(0.0),
                    ..Default::default()
                },
                pictures::Frame {
                    verdict: "no visible difference".into(),
                    difference: Some(0.0),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        g
    }

    /// The words the fixer's router launched a fresh fixer with, if any.
    fn launched_with(app: &App) -> Vec<String> {
        app.outbox
            .iter()
            .filter_map(|r| match r {
                orion_core::ClientRequest::CreatePrAgent {
                    starting_prompt, ..
                } => starting_prompt.clone(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn pictures_showing_no_difference_are_review_and_send_the_agent_back_once_a_head() {
        let mut app = app_listing_the_pr();
        land_gathered(&mut app, showing_no_difference("abc"));
        let posted = posted_card(&app);
        assert_eq!(posted.card.body[0], "*Review*: pictures show no difference");
        assert!(
            posted.card.body[1].contains("🔴 *Pictures*: no visible difference in any frame"),
            "{:?}",
            posted.card.body
        );
        assert!(posted.card.image.is_none(), "no grid that shows nothing");
        let sent = launched_with(&app);
        assert_eq!(sent.len(), 1, "{:?}", app.outbox);
        assert!(sent[0].contains(NO_DIFFERENCE), "{}", sent[0]);
        assert_eq!(
            app.pr_body.remembered(URL).no_difference_sent.as_deref(),
            Some("abc")
        );
        // The same head again: not sent twice.
        let (outbox, queued) = (app.outbox.len(), app.autofix.queued.len());
        land_gathered(&mut app, showing_no_difference("abc"));
        assert_eq!(app.outbox.len(), outbox, "{:?}", app.outbox);
        assert_eq!(app.autofix.queued.len(), queued);
        // A new push whose pictures still show nothing: once more.
        land_gathered(&mut app, showing_no_difference("def"));
        assert_eq!(
            app.pr_body.remembered(URL).no_difference_sent.as_deref(),
            Some("def")
        );
        assert!(
            app.autofix.queued.len() > queued || app.outbox.len() > outbox,
            "sent for the new head"
        );
        // Pictures that show the change send nothing.
        let mut app = app_listing_the_pr();
        let mut g = showing_no_difference("abc");
        if let Some(m) = g.pictures.as_mut() {
            m.frames[0].verdict = "changed".into();
            m.frames[0].difference = Some(0.2);
        }
        land_gathered(&mut app, g);
        assert!(launched_with(&app).is_empty());
        assert_eq!(app.pr_body.remembered(URL).no_difference_sent, None);
        assert!(posted_card(&app).card.body[0].starts_with("*Skim*"));
    }

    #[test]
    fn the_card_waits_for_settled_checks_goes_once_and_closes_when_blocked() {
        use orion_core::ClientRequest;
        let mut app = App::new();
        let g = gathered();
        let v = |tier| Verdict {
            tier,
            reasons: vec![verdict::Reason {
                tier,
                text: "why".into(),
            }],
        };
        card(
            &mut app,
            &g,
            &v(Tier::Merge),
            &rows::Top::default(),
            None,
            false,
        );
        assert!(app.outbox.is_empty(), "checks still running");
        card(
            &mut app,
            &g,
            &v(Tier::Merge),
            &rows::Top::default(),
            None,
            true,
        );
        assert!(matches!(
            app.outbox.last(),
            Some(ClientRequest::PostFactoryCard { .. })
        ));
        let sent = app.outbox.len();
        card(
            &mut app,
            &g,
            &v(Tier::Merge),
            &rows::Top::default(),
            None,
            true,
        );
        assert_eq!(app.outbox.len(), sent, "same tier, same head: not again");
        card(
            &mut app,
            &g,
            &v(Tier::Review),
            &rows::Top::default(),
            None,
            true,
        );
        assert_eq!(app.outbox.len(), sent + 1, "a new tier edits it in place");
        card(
            &mut app,
            &g,
            &v(Tier::Blocked),
            &rows::Top::default(),
            None,
            true,
        );
        assert!(matches!(
            app.outbox.last(),
            Some(ClientRequest::UpdateFactoryCard { .. })
        ));
        let closed = app.outbox.len();
        card(
            &mut app,
            &g,
            &v(Tier::Blocked),
            &rows::Top::default(),
            None,
            true,
        );
        assert_eq!(app.outbox.len(), closed, "closed once");
    }

    const URL: &str = "https://github.com/o/r/pull/7";

    fn verdict_of(tier: Tier) -> Verdict {
        Verdict {
            tier,
            reasons: vec![verdict::Reason {
                tier,
                text: "why".into(),
            }],
        }
    }

    /// An app with the Merge card for `URL` up, posted for head `abc`, and
    /// its page loaded at that head.
    fn app_with_card() -> App {
        let mut app = App::new();
        card(
            &mut app,
            &gathered(),
            &verdict_of(Tier::Merge),
            &rows::Top::default(),
            None,
            true,
        );
        app.pr_body.remembered(URL).tier = Some("merge".into());
        app.pr_detail.insert(
            URL.into(),
            PrDetail {
                number: 7,
                head_sha: "abc".into(),
                ..Default::default()
            },
        );
        app
    }

    fn posted_card(app: &App) -> &cards::Posted {
        app.factory_cards
            .iter()
            .find(|p| p.card.key == card_key(URL))
            .expect("the card")
    }

    #[test]
    fn merge_stands_only_while_the_card_does() {
        let mut app = app_with_card();
        assert!(merge_stands(&mut app, URL), "Merge, up, same head");
        app.pr_detail.get_mut(URL).unwrap().head_sha = "def".into();
        assert!(!merge_stands(&mut app, URL), "a new head");
        app.pr_detail.get_mut(URL).unwrap().head_sha = "abc".into();
        app.pr_body.remembered(URL).tier = Some("blocked".into());
        assert!(!merge_stands(&mut app, URL), "Blocked now");
        app.pr_body.remembered(URL).tier = Some("review".into());
        assert!(merge_stands(&mut app, URL), "Review may merge");
        cards::close(&mut app, &card_key(URL), "Blocked: red");
        assert!(!merge_stands(&mut app, URL), "the card closed");
        assert!(!merge_stands(&mut App::new(), URL), "never carded");
    }

    #[test]
    fn a_stale_merge_is_refused_and_the_card_says_why() {
        use orion_core::ClientRequest;
        // The head moved: not merged, the card back up with a warning.
        let mut app = app_with_card();
        app.pr_detail.get_mut(URL).unwrap().head_sha = "def".into();
        cards::on_action(&mut app, &card_key(URL), "merge", None, true);
        assert!(app
            .flash
            .as_ref()
            .is_some_and(|f| f.text.contains("CI changed since this card")));
        let posted = posted_card(&app);
        assert_eq!(posted.state, CardState::Open);
        assert_eq!(posted.card.body[0], format!("⚠ {STALE}"));
        assert!(posted.card.buttons.iter().any(|b| b.action == "merge"));
        assert!(matches!(
            app.outbox.last(),
            Some(ClientRequest::PostFactoryCard { .. })
        ));

        // Gone Blocked behind a card the last run left up: closed instead.
        let mut app = app_with_card();
        app.pr_body.remembered(URL).tier = Some("blocked".into());
        cards::on_action(&mut app, &card_key(URL), "merge", None, true);
        assert_eq!(
            posted_card(&app).state,
            CardState::Closed(format!("Not merged: {STALE}"))
        );
        assert!(app.pr_body.remembered(URL).card.as_ref().unwrap().closed);
    }

    /// A PR's card asks whether it merges, with **Not for orion…** as
    /// the way out, and no vote.
    #[test]
    fn the_verdict_card_asks_about_the_merge_with_a_way_out() {
        let app = app_with_card();
        let actions: Vec<&str> = posted_card(&app)
            .card
            .buttons
            .iter()
            .map(|b| b.action.as_str())
            .collect();
        assert_eq!(
            actions,
            [
                "merge",
                "open",
                "not_for_factory",
                "not_for_factory_backlog"
            ]
        );
    }

    #[test]
    fn a_pr_whose_card_closed_a_week_ago_is_forgotten() {
        let mut app = app_with_card();
        app.pr_body.tracked.insert(URL.into(), Tracked::default());
        let now = orion_core::clock::now_ms();
        app.pr_body.forget_closed(now);
        assert!(app.pr_body.memory().prs.contains_key(URL), "still up");
        app.pr_body.remembered(URL).card.as_mut().unwrap().close();
        app.pr_body.forget_closed(now + FORGET_AFTER_MS - 1);
        assert!(app.pr_body.memory().prs.contains_key(URL), "closed lately");
        app.pr_body.forget_closed(now + FORGET_AFTER_MS + 1);
        assert!(!app.pr_body.memory().prs.contains_key(URL));
        assert!(!app.pr_body.tracked.contains_key(URL));
    }

    #[test]
    fn a_pr_merged_or_closed_elsewhere_closes_its_card_once() {
        use crate::pull_request::{STATE_CLOSED, STATE_MERGED};
        // Its page says merged.
        let mut app = app_with_card();
        let merged = PrDetail {
            number: 7,
            answered_state: STATE_MERGED.into(),
            ..Default::default()
        };
        land_detail(&mut app, URL, Some(&merged));
        assert_eq!(posted_card(&app).state, CardState::Closed("Merged".into()));
        let sent = app.outbox.len();
        land_detail(&mut app, URL, Some(&merged));
        assert_eq!(app.outbox.len(), sent, "once");

        // `prs` hears it closed — the list, the modal — and the next look
        // closes the card.
        let mut app = app_with_card();
        let closed = PrDetail {
            answered_state: STATE_CLOSED.into(),
            ..merged
        };
        app.prs.observe(
            URL,
            crate::pr_store::PrObservation::of_detail(&closed),
            crate::fetch::Asked::At(crate::fetch::now()),
        );
        assert!(due_fetches(&mut app).is_empty());
        assert_eq!(posted_card(&app).state, CardState::Closed("Closed".into()));
    }
}
