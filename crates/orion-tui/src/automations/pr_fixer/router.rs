//! The FIX ROUTER: one branch, one agent. Everything the PR fixer sends
//! about a pull request — a breakage ([`Work::Fix`]), the owner's review
//! comments or a reply from a card ([`Work::Words`]) — goes where
//! [`route`] says:
//!
//! 1. an `autofix-<n>` already live on the branch: a breakage waits for
//!    it (nothing is sent twice); words queue for it;
//! 2. the branch's own agent ([`branch_agent`]), idle and under
//!    [`CONTEXT_LIMIT`] of its window ([`context_used`]): a new turn
//!    (`event_loop::send_turn`);
//! 3. that agent busy — at work, starting, or waiting on the owner: the
//!    work is queued and routed again once it is idle ([`deliver_queued`]);
//! 4. that agent too full, its context unreadable, or no agent at all: a
//!    fresh `autofix-<n>` with a handover ([`handover`]) — the last two
//!    things the old agent said, and the plan as approved.
//!
//! How full an agent's context is comes off its transcript, read on the
//! blocking pool ([`measure`]) and kept for [`CONTEXT_TTL`]: until it is
//! in, the work waits for that agent like any other queued work. And from
//! the moment a fresh fixer is launched until it shows in the tree (or
//! [`LAUNCH_WAIT`] passes), later work for its pull request waits for it
//! rather than launch a second one ([`Route::Launching`]).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use orion_core::{AgentId, AgentStatus, ClientRequest, ProjectId};
use serde_json::Value;

use super::{AutofixForm, Issue};
use crate::app::App;
use crate::flash::Flash;
use crate::pull_request::{OpenPr, PrDetail};
use crate::ui::truncate;

/// Past this share of its model's window, the branch's agent hands over to
/// a fresh fixer rather than take another turn.
pub const CONTEXT_LIMIT: f64 = 0.75;
/// A model's context window, in tokens: [`WINDOW_1M`] when its id says
/// `[1m]`.
pub const WINDOW: u64 = 200_000;
pub const WINDOW_1M: u64 = 1_000_000;
/// How much of a transcript's end is read for its last usage and words.
const TAIL_BYTES: u64 = 1024 * 1024;
/// The most of each handed-over message, and of the plan, put in a prompt.
const HANDOVER_CHARS: usize = 4_000;
const PLAN_CHARS: usize = 12_000;
/// The heading of the comment the PR body track posts with the approved
/// plan.
const PLAN_HEADING: &str = "Plan as approved";
/// How long a measured context is trusted before it is read again.
pub const CONTEXT_TTL: Duration = Duration::from_secs(60);
/// A measurement that has not landed by now is asked for again.
const MEASURE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a launched fresh fixer is waited for to show in the tree.
pub const LAUNCH_WAIT: Duration = Duration::from_secs(2 * 60);
/// What an agent ends each comment it posts on a pull request with:
/// it posts through the owner's `gh`, so this is how its words are told
/// from the owner's ([`owner_comments`]).
pub const AGENT_MARKER: &str = "<!-- orion:agent -->";
/// The line every orion prompt that may have an agent comment on a pull
/// request carries.
pub const MARKER_RULE: &str = "End every comment or reply you post on the pull request with the line `<!-- orion:agent -->`, so orion can tell your words from the owner's.";

/// How full an agent's context was, and the last two things it said, as
/// its transcript read at `at` ([`measure`]).
#[derive(Debug, Clone)]
pub struct Context {
    pub at: Instant,
    /// None: no transcript to read, or no usage in it.
    pub used: Option<f64>,
    pub said: Vec<String>,
}

/// A fresh fixer launched for a pull request, not in the tree yet.
#[derive(Debug, Clone)]
pub struct Launch {
    pub at: Instant,
    pub project: ProjectId,
    pub branch: String,
    pub number: u64,
    /// The `autofix-<n>`s on the branch before it: the new one is the one
    /// not among them.
    pub before: Vec<AgentId>,
}

/// Where work on a pull request goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// An `autofix-<n>` is already live on the branch.
    FixerLive(AgentId),
    /// The branch's own agent, idle with room: a new turn.
    Turn(AgentId),
    /// The branch's agent is busy: queued until it is idle.
    Queue(AgentId),
    /// A fresh `autofix-<n>`, handed over from the agent that was too
    /// full (None: there was none).
    Fresh { from: Option<AgentId> },
    /// The branch's agent is idle, but how full its context is has not
    /// been read lately: [`measure`] it, and wait.
    Measure(AgentId),
    /// A fresh `autofix-<n>` was launched for this pull request and is
    /// not in the tree yet: wait for it.
    Launching,
}

/// Where work on pull request `number` (`url`), on `branch` of `project`,
/// goes now. Reads nothing off the disk.
pub fn route(app: &App, project: &ProjectId, branch: &str, number: u64, url: &str) -> Route {
    if let Some(fixer) = super::fixer(app, project, branch, number) {
        return Route::FixerLive(fixer.id.clone());
    }
    if launch_pending(app, url) {
        return Route::Launching;
    }
    let Some(agent) = branch_agent(app, project, branch) else {
        return Route::Fresh { from: None };
    };
    if busy(agent.status) {
        return Route::Queue(agent.id.clone());
    }
    let Some(context) = cached(app, &agent.id) else {
        return Route::Measure(agent.id.clone());
    };
    match context.used {
        Some(used) if used < CONTEXT_LIMIT => Route::Turn(agent.id.clone()),
        _ => Route::Fresh {
            from: Some(agent.id.clone()),
        },
    }
}

/// `agent`'s context as last measured, while it is under [`CONTEXT_TTL`]
/// old.
pub fn cached<'a>(app: &'a App, agent: &AgentId) -> Option<&'a Context> {
    app.autofix
        .context
        .get(agent)
        .filter(|c| c.at.elapsed() < CONTEXT_TTL)
}

/// Read how full `agent`'s context is off its transcript — on the
/// blocking pool, and inline where there is none (the unit tests) —
/// unless a read is already out.
pub fn measure(app: &mut App, agent: &AgentId) {
    let out = app
        .autofix
        .measuring
        .get(agent)
        .is_some_and(|at| at.elapsed() < MEASURE_TIMEOUT);
    if out {
        return;
    }
    let Some(row) = app.tree.agents.iter().find(|a| &a.id == agent).cloned() else {
        return;
    };
    app.autofix.measuring.insert(agent.clone(), Instant::now());
    match app.view_jobs.clone() {
        Some(jobs) => jobs.run(move || {
            let (used, said) = read_context(&row);
            Some(crate::view_jobs::Answer::FixerContext {
                agent: row.id,
                used,
                said,
            })
        }),
        None => {
            let (used, said) = read_context(&row);
            land_context(app, row.id, used, said);
        }
    }
}

/// [`measure`]'s answer, kept for [`CONTEXT_TTL`].
pub(crate) fn land_context(app: &mut App, agent: AgentId, used: Option<f64>, said: Vec<String>) {
    app.autofix.measuring.remove(&agent);
    app.autofix.context.insert(
        agent,
        Context {
            at: Instant::now(),
            used,
            said,
        },
    );
    app.dirty = true;
}

/// How full `agent`'s context is and its last two messages, off the
/// tail of its transcript. Disk work: never on the UI thread.
fn read_context(agent: &orion_core::Agent) -> (Option<f64>, Vec<String>) {
    let Some(tail) = transcript_tail(agent) else {
        return (None, Vec::new());
    };
    (
        context_in(&tail, agent.model.as_deref()),
        last_said(&tail, 2),
    )
}

/// The last `n` things `agent` said ([`last_said`]), off the tail of its
/// transcript; none when it has none. Disk work: never on the UI thread.
pub(crate) fn last_said_of(agent: &orion_core::Agent, n: usize) -> Vec<String> {
    transcript_tail(agent)
        .map(|tail| last_said(&tail, n))
        .unwrap_or_default()
}

/// The last [`TAIL_BYTES`] of `agent`'s transcript.
fn transcript_tail(agent: &orion_core::Agent) -> Option<String> {
    super::transcript_of(agent, None).and_then(|path| tail(&path, TAIL_BYTES))
}

/// Whether a fresh fixer launched for `url` is still on its way: launched
/// under [`LAUNCH_WAIT`] ago, and not in the tree yet.
pub fn launch_pending(app: &App, url: &str) -> bool {
    app.autofix
        .launches
        .get(url)
        .is_some_and(|l| l.at.elapsed() < LAUNCH_WAIT && !launched(app, l))
}

/// Whether the fixer `launch` started is in the tree.
fn launched(app: &App, launch: &Launch) -> bool {
    autofix_ids(app, &launch.project, &launch.branch, launch.number)
        .iter()
        .any(|id| !launch.before.contains(id))
}

/// Every `autofix-<number>` on `branch` of `project`, archived or not.
fn autofix_ids(app: &App, project: &ProjectId, branch: &str, number: u64) -> Vec<AgentId> {
    let name = super::agent_name(number);
    app.tree
        .agents
        .iter()
        .filter(|a| a.name == name && super::on_branch(app, a, project, branch))
        .map(|a| a.id.clone())
        .collect()
}

/// The branch's own agent: the session most recently at work in one of
/// `project`'s checkouts of `branch`, archived ones aside — the builder,
/// or the fixer that took over from it.
pub fn branch_agent<'a>(
    app: &'a App,
    project: &ProjectId,
    branch: &str,
) -> Option<&'a orion_core::Agent> {
    app.tree
        .agents
        .iter()
        .filter(|a| !a.archived && super::on_branch(app, a, project, branch))
        .max_by_key(|a| crate::app::last_interaction_ms(a))
}

/// Starting, at work, or waiting on the owner: not taking a new turn now.
pub fn busy(status: AgentStatus) -> bool {
    matches!(
        status,
        AgentStatus::Fresh | AgentStatus::Running | AgentStatus::NeedsFeedback
    )
}

/// [`context_used`] on a transcript's `tail`: the newest assistant
/// message's `usage`, as (input + cache read + cache creation) / the
/// window of `model`.
pub fn context_in(tail: &str, model: Option<&str>) -> Option<f64> {
    let usage = tail
        .lines()
        .rev()
        .filter_map(own_line)
        .filter(|line| line_type(line) == Some("assistant"))
        .find_map(|line| line.pointer("/message/usage").cloned())?;
    let tokens = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let used = tokens("input_tokens")
        + tokens("cache_read_input_tokens")
        + tokens("cache_creation_input_tokens");
    Some(used as f64 / window(model) as f64)
}

/// A model's window: a million tokens when its id says `[1m]`.
pub fn window(model: Option<&str>) -> u64 {
    match model.is_some_and(|m| m.to_ascii_lowercase().contains("[1m]")) {
        true => WINDOW_1M,
        false => WINDOW,
    }
}

/// The last `n` things the session said — its newest assistant messages
/// with text, each one's text blocks joined — oldest first.
pub fn last_said(tail: &str, n: usize) -> Vec<String> {
    let mut said: Vec<String> = Vec::new();
    let mut current: Option<Value> = None;
    let mut texts: Vec<String> = Vec::new();
    for line in tail.lines().rev().filter_map(own_line) {
        if line_type(&line) != Some("assistant") {
            continue;
        }
        let line_texts: Vec<String> = line
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        if line_texts.is_empty() {
            continue;
        }
        let id = line.pointer("/message/id").cloned().unwrap_or(Value::Null);
        if current.as_ref().is_some_and(|c| *c != id) {
            said.push(std::mem::take(&mut texts).join("\n\n"));
            if said.len() == n {
                break;
            }
        }
        current = Some(id);
        let mut line_texts = line_texts;
        line_texts.append(&mut texts);
        texts = line_texts;
    }
    if said.len() < n && !texts.is_empty() {
        said.push(texts.join("\n\n"));
    }
    said.reverse();
    said
}

/// The approved plan the PR body track posted on the pull request — the
/// newest comment headed [`PLAN_HEADING`] — when there is one.
pub fn plan_comment(detail: &PrDetail) -> Option<String> {
    detail
        .comments
        .iter()
        .rev()
        .find(|c| is_plan(&c.body))
        .map(|c| c.body.trim().to_string())
}

fn is_plan(body: &str) -> bool {
    body.trim_start()
        .trim_start_matches('#')
        .trim_start()
        .trim_start_matches("**")
        .starts_with(PLAN_HEADING)
}

/// What a fresh fixer is told about the work before it: what `from`
/// last said (`said`, oldest first) and the plan as approved. Empty with
/// neither.
pub fn handover(from: Option<&str>, said: &[String], plan: Option<&str>) -> String {
    let mut parts = Vec::new();
    if !said.is_empty() {
        let name = from.unwrap_or("the session before you");
        parts.push(format!(
            "Handover: you take over this branch from the session \"{name}\", whose context is nearly full. The last things it said:"
        ));
        parts.extend(said.iter().map(|s| quote(&truncate(s, HANDOVER_CHARS))));
    }
    if let Some(plan) = plan {
        parts.push(format!(
            "The plan the owner approved for this pull request:\n\n{}",
            truncate(plan, PLAN_CHARS)
        ));
    }
    parts.join("\n\n")
}

// ---- the owner's comments ----

/// One comment the owner left on their pull request: when, where (a file
/// and line, for a review comment) and what it says, verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerComment {
    pub at: String,
    /// `` `src/a.rs` line 12 ``; None for a comment on the whole pull
    /// request.
    pub place: Option<String>,
    pub body: String,
}

/// The owner's comments on their own pull request `detail`, oldest
/// first: its conversation's comments and review bodies by its author,
/// and the review comments on its lines written as the signed-in user
/// and submitted. What orion and its agents write as that same user — the
/// approved plan, a Claude Code footer, orion's hidden blocks, Cubic
/// lines — is left out.
pub fn owner_comments(detail: &PrDetail) -> Vec<OwnerComment> {
    let mut out: Vec<OwnerComment> = detail
        .comments
        .iter()
        .filter(|c| !detail.author.is_empty() && c.author.eq_ignore_ascii_case(&detail.author))
        .filter(|c| !c.body.trim().is_empty() && !machine_written(&c.body))
        .map(|c| OwnerComment {
            at: c.at.clone(),
            place: None,
            body: c.body.trim().to_string(),
        })
        .collect();
    for thread in &detail.threads {
        for c in thread.comments.iter().filter(|c| c.mine && !c.pending) {
            if c.body.trim().is_empty() || machine_written(&c.body) {
                continue;
            }
            out.push(OwnerComment {
                at: c.at.clone(),
                place: Some(format!("`{}` {}", thread.path, thread.place())),
                body: c.body.trim().to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.at.cmp(&b.at));
    out
}

/// Written by orion or an agent through the owner's `gh`, not by the
/// owner: an agent signs its comments with [`AGENT_MARKER`], and orion's
/// own blocks are hidden `<!-- orion…` comments too.
fn machine_written(body: &str) -> bool {
    is_plan(body)
        || body.contains(AGENT_MARKER)
        || body.contains("<!-- orion")
        || body.contains("Generated with [Claude Code]")
        || body.contains("Co-Authored-By: Claude")
        || body.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with("Cubic dismissals:") || l.starts_with("Cubic unresolved:")
        })
}

/// The comments newer than `seen` (an RFC 3339 time), and the newest
/// time among all of them — what `seen` becomes. With `seen` None — the
/// first look at this pull request — nothing is new: what was said before
/// orion watched is history.
pub fn new_since(comments: &[OwnerComment], seen: Option<&str>) -> (Vec<OwnerComment>, String) {
    let newest = comments
        .iter()
        .map(|c| c.at.as_str())
        .chain(seen)
        .max()
        .unwrap_or_default()
        .to_string();
    let Some(seen) = seen else {
        return (Vec::new(), newest);
    };
    let new = comments
        .iter()
        .filter(|c| c.at.as_str() > seen)
        .cloned()
        .collect();
    (new, newest)
}

/// The owner's `comments` as the agent reads them: each one's place and
/// its words, verbatim.
pub fn comments_text(comments: &[OwnerComment]) -> String {
    comments
        .iter()
        .map(|c| {
            let place = match &c.place {
                Some(place) => format!("On {place}:"),
                None => "On the pull request:".to_string(),
            };
            format!("{place}\n{}", quote(&c.body))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ---- delivery ----

/// Work for a pull request's agent.
#[derive(Debug, Clone)]
pub enum Work {
    /// A breakage, as `land_detail` or `⌘G` diagnosed it.
    Fix(Box<AutofixForm>),
    /// Words for the agent: the owner's review comments, or a reply typed
    /// on a card — `text` already said for the agent.
    Words {
        project: ProjectId,
        pr: Box<OpenPr>,
        text: String,
        /// The approved plan, for a fresh fixer's handover.
        plan: Option<String>,
        /// What the run record says it was (`review comment`).
        reason: String,
    },
}

impl Work {
    pub fn project(&self) -> &ProjectId {
        match self {
            Work::Fix(form) => &form.project,
            Work::Words { project, .. } => project,
        }
    }

    pub fn pr(&self) -> &OpenPr {
        match self {
            Work::Fix(form) => &form.pr,
            Work::Words { pr, .. } => pr,
        }
    }

    pub fn is_fix(&self) -> bool {
        matches!(self, Work::Fix(_))
    }
}

/// Work waiting for a busy agent to be idle — or, with no agent, for the
/// fresh fixer launched for its pull request to show.
#[derive(Debug, Clone)]
pub struct Queued {
    pub agent: Option<AgentId>,
    pub work: Work,
}

/// Send `work` where [`route`] says.
pub fn deliver(app: &mut App, work: Work, out: &mut Vec<ClientRequest>) {
    deliver_with(app, work, &mut None, out);
}

/// [`deliver`], with the config a fresh fixer is launched under loaded
/// into `cfg` at most once for every delivery that shares it.
fn deliver_with(
    app: &mut App,
    work: Work,
    cfg: &mut Option<crate::config::Config>,
    out: &mut Vec<ClientRequest>,
) {
    let (project, pr) = (work.project().clone(), work.pr().clone());
    let mut to = route(app, &project, &pr.head, pr.number, &pr.url);
    if let Route::Measure(agent) = &to {
        // Inline in the unit tests: routed again at once.
        measure(app, agent);
        to = route(app, &project, &pr.head, pr.number, &pr.url);
    }
    match to {
        // A breakage never goes to a second fixer: the live one is on it,
        // and the PR is looked at again once it is done.
        Route::FixerLive(_) if work.is_fix() => {
            app.flash = Some(Flash::note(format!(
                "{} is already on #{}",
                super::agent_name(pr.number),
                pr.number
            )));
            app.dirty = true;
        }
        Route::FixerLive(id) | Route::Queue(id) => {
            let note = format!(
                "PR fixer: #{} waits for {} to finish its turn",
                pr.number,
                agent_label(app, &id)
            );
            queue(app, Some(id), work, Some(note));
        }
        // Measured on the blocking pool: delivered on the beat once in.
        Route::Measure(id) => queue(app, Some(id), work, None),
        Route::Launching => {
            let note = format!(
                "PR fixer: #{} waits for {} to start",
                pr.number,
                super::agent_name(pr.number)
            );
            queue(app, None, work, Some(note));
        }
        Route::Turn(id) => {
            let text = turn_text(&work);
            if crate::event_loop::send_turn(app, &id, text, false, out) {
                let name = agent_label(app, &id);
                sent(app, &work, &name, false);
            }
        }
        Route::Fresh { from } => {
            let said = from
                .as_ref()
                .and_then(|id| cached(app, id))
                .map(|c| c.said.clone())
                .unwrap_or_default();
            let from_name = from.as_ref().map(|id| agent_label(app, id));
            let plan = match &work {
                Work::Fix(form) => form.plan.clone(),
                Work::Words { plan, .. } => plan.clone(),
            };
            let handover = handover(from_name.as_deref(), &said, plan.as_deref());
            let before = autofix_ids(app, &project, &pr.head, pr.number);
            let cfg = cfg.get_or_insert_with(crate::config::Config::load);
            if super::launch_fresh(app, &work, &handover, cfg, out) {
                app.autofix.launches.insert(
                    pr.url.clone(),
                    Launch {
                        at: Instant::now(),
                        project,
                        branch: pr.head.clone(),
                        number: pr.number,
                        before,
                    },
                );
                sent(app, &work, &super::agent_name(pr.number), true);
            }
        }
    }
}

/// Hold `work` for `agent` (None: the fresh fixer on its way), saying
/// `note`: the newest breakage replaces an older one for the same pull
/// request, and words for it join the words already held.
fn queue(app: &mut App, agent: Option<AgentId>, work: Work, note: Option<String>) {
    let pr = work.pr().clone();
    match work {
        Work::Fix(form) => {
            // Handled for now — the open list doesn't watch it again
            // unless it breaks differently — and counted once delivered.
            app.autofix.record(&pr.url).handled = Some(form.fingerprint());
            app.autofix.forget(&pr.url);
            app.autofix
                .queued
                .retain(|q| !(q.work.is_fix() && q.work.pr().url == pr.url));
            app.autofix.queued.push(Queued {
                agent: agent.clone(),
                work: Work::Fix(form),
            });
        }
        Work::Words { text, .. } if app.autofix.queued.iter().any(|q| is_words_for(q, &pr.url)) => {
            for q in app
                .autofix
                .queued
                .iter_mut()
                .filter(|q| is_words_for(q, &pr.url))
            {
                if let Work::Words { text: held, .. } = &mut q.work {
                    held.push_str("\n\n");
                    held.push_str(&text);
                }
                q.agent = agent.clone();
            }
        }
        words => app.autofix.queued.push(Queued {
            agent: agent.clone(),
            work: words,
        }),
    }
    if let Some(note) = note {
        app.flash = Some(Flash::note(note));
    }
    app.dirty = true;
}

fn is_words_for(q: &Queued, url: &str) -> bool {
    !q.work.is_fix() && q.work.pr().url == url
}

/// The beat: each queued piece of work whose agent is idle now — or gone —
/// and measured, routed again; work held for a fresh fixer goes to it
/// once it shows, to wait for its turn to end. A breakage whose pull
/// request has moved on to another head since (the busy agent pushed) is
/// dropped: the open list watches the new head if it breaks too.
pub fn deliver_queued(app: &mut App, out: &mut Vec<ClientRequest>) {
    let landed: Vec<String> = app
        .autofix
        .launches
        .iter()
        .filter(|(_, l)| l.at.elapsed() >= LAUNCH_WAIT || launched(app, l))
        .map(|(url, _)| url.clone())
        .collect();
    for url in landed {
        app.autofix.launches.remove(&url);
    }
    app.autofix
        .context
        .retain(|_, c| c.at.elapsed() < CONTEXT_TTL);
    let mut cfg = None;
    for q in std::mem::take(&mut app.autofix.queued) {
        if let Work::Fix(form) = &q.work {
            let head = app.prs.status_or_open(&form.pr.url).head_sha;
            if !head.is_empty() && !form.refs.head_sha.is_empty() && head != form.refs.head_sha {
                continue;
            }
        }
        let Some(agent) = &q.agent else {
            let pr = q.work.pr();
            if launch_pending(app, &pr.url) {
                app.autofix.queued.push(q);
            } else if let Some(fixer) = super::fixer(app, q.work.project(), &pr.head, pr.number) {
                let agent = Some(fixer.id.clone());
                app.autofix.queued.push(Queued { agent, ..q });
            } else {
                deliver_with(app, q.work, &mut cfg, out);
            }
            continue;
        };
        let measuring = app
            .autofix
            .measuring
            .get(agent)
            .is_some_and(|at| at.elapsed() < MEASURE_TIMEOUT);
        let waiting = measuring
            || app
                .tree
                .agents
                .iter()
                .any(|a| &a.id == agent && !a.archived && busy(a.status));
        if waiting {
            app.autofix.queued.push(q);
        } else {
            deliver_with(app, q.work, &mut cfg, out);
        }
    }
}

/// What the ledger, the flash and the run record keep of work sent to
/// `to` (`fresh`: a new `autofix-<n>`).
fn sent(app: &mut App, work: &Work, to: &str, fresh: bool) {
    let pr = work.pr().clone();
    let reason = match work {
        Work::Fix(form) => {
            let record = app.autofix.record(&pr.url);
            record.handled = Some(form.fingerprint());
            record.attempts = record.attempts.saturating_add(1);
            app.autofix.forget(&pr.url);
            let what: Vec<String> = Issue::ALL
                .into_iter()
                .filter(|i| form.picks[i.index()])
                .map(|i| match i {
                    Issue::Conflicts => "conflict".to_string(),
                    Issue::Checks => format!(
                        "ci: {}",
                        form.diagnosis
                            .failing
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                })
                .collect();
            app.flash = Some(Flash::working(format!(
                "{to} on #{}: {}",
                pr.number,
                form.diagnosis.summary()
            )));
            what.join("; ")
        }
        Work::Words { reason, .. } => {
            app.flash = Some(Flash::working(format!("{to} on #{}: {reason}", pr.number)));
            reason.clone()
        }
    };
    app.dirty = true;
    let Some(identifier) = super::factory_ticket(app, work.project(), &pr.head) else {
        return;
    };
    let send = crate::factory::runs::FixerSend {
        at_ms: orion_core::clock::now_ms(),
        reason,
        to: to.to_string(),
        fresh,
    };
    crate::factory::runs::update(&identifier, |r| r.fixer_sends.push(send));
}

/// `work` as a new turn for the branch's own agent, ending on
/// [`MARKER_RULE`].
pub fn turn_text(work: &Work) -> String {
    match work {
        Work::Fix(form) => format!(
            "orion's PR fixer: pull request #{n} needs fixing. Fix only what is listed under \"Fix\" below, on this branch: no new pull request, no force-push, nothing unrelated.\n\n{task}\n\nRead each failing check's logs with `gh run view <run-id> --log-failed`; no local e2e unless the logs aren't enough. Then commit with the repo's commit skill, push, then stop: orion watches CI.\n\n{MARKER_RULE}",
            n = form.pr.number,
            task = super::task_of(form),
        ),
        Work::Words { text, .. } => format!("{text}\n\n{MARKER_RULE}"),
    }
}

/// The owner's new review comments on `pr`, as the agent reads them.
pub fn comments_turn(pr: &OpenPr, comments: &[OwnerComment]) -> String {
    format!(
        "The owner commented on your pull request #{n} ({url}). Do what each comment asks, in this pull request on `{head}`: commit with the repo's commit skill, push, then stop: orion watches CI. Answer on the pull request only where a comment asks a question.\n\n{body}",
        n = pr.number,
        url = pr.url,
        head = pr.head,
        body = comments_text(comments),
    )
}

/// The name a session goes by, for the flash and the record.
fn agent_label(app: &App, id: &AgentId) -> String {
    app.tree
        .agents
        .iter()
        .find(|a| &a.id == id)
        .map_or_else(|| "the branch's agent".to_string(), |a| a.name.clone())
}

// ---- reading a transcript ----

/// The last `bytes` of `path`, its first, cut line dropped.
fn tail(path: &Path, bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(match start {
        0 => text,
        _ => text
            .split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default(),
    })
}

/// A whole line of the session's own conversation, parsed: subagent
/// lines (`isSidechain`) and a cut line are None.
fn own_line(line: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(line).ok()?;
    let sidechain = value
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (!sidechain).then_some(value)
}

fn line_type(line: &Value) -> Option<&str> {
    line.get("type")?.as_str()
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr_review::github::{ReviewThread, Side, ThreadComment};
    use crate::pull_request::PrComment;

    fn assistant(id: &str, text: &str, used: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "id": id,
                "content": [{"type": "text", "text": text}],
                "usage": {
                    "input_tokens": 10,
                    "cache_read_input_tokens": used - 10 - 100,
                    "cache_creation_input_tokens": 100,
                    "output_tokens": 999
                }
            }
        })
        .to_string()
    }

    /// Context is the newest assistant usage — input, cache read and cache
    /// creation, never output — over the model's window, a million for a
    /// `[1m]` model; a subagent's line is not the session's own.
    #[test]
    fn context_is_the_last_usage_over_the_window() {
        let sidechain = serde_json::json!({
            "type": "assistant", "isSidechain": true,
            "message": {"id": "s", "usage": {"input_tokens": 190_000}}
        })
        .to_string();
        let tail = [
            assistant("a", "first", 20_000),
            assistant("b", "second", 160_000),
            sidechain,
            "{\"type\":\"user\"}".into(),
        ]
        .join("\n");
        assert_eq!(context_in(&tail, Some("opus")), Some(0.8));
        assert_eq!(context_in(&tail, Some("opus[1m]")), Some(0.16));
        assert_eq!(context_in("{\"type\":\"user\"}", None), None);
        assert_eq!(window(Some("claude-opus-4-5[1M]")), WINDOW_1M);
        assert_eq!(window(None), WINDOW);
    }

    /// The last two things said are whole messages — a message's text
    /// lines joined, tool calls between them skipped — oldest first.
    #[test]
    fn the_handover_quotes_the_last_two_messages_and_the_plan() {
        let tool = serde_json::json!({
            "type": "assistant",
            "message": {"id": "c", "content": [{"type": "tool_use", "name": "Bash"}]}
        })
        .to_string();
        let tail = [
            assistant("a", "one", 1_000),
            assistant("b", "two", 1_000),
            assistant("c", "three", 1_000),
            tool,
            assistant("c", "three more", 1_000),
        ]
        .join("\n");
        assert_eq!(last_said(&tail, 2), ["two", "three\n\nthree more"]);
        let text = handover(
            Some("RIPLO-1"),
            &last_said(&tail, 2),
            Some("## Plan as approved\n1. Do it"),
        );
        assert!(text.contains("\"RIPLO-1\""), "{text}");
        assert!(text.contains("> two\n\n> three"), "{text}");
        assert!(text.contains("1. Do it"), "{text}");
        assert_eq!(handover(None, &[], None), "");
        // A long message is clipped to its cap, the `…` included.
        let long = "x".repeat(HANDOVER_CHARS + 10);
        let text = handover(None, &[long], None);
        let quoted = text.lines().last().unwrap().trim_start_matches("> ");
        assert_eq!(quoted.chars().count(), HANDOVER_CHARS);
        assert!(quoted.ends_with('…'));
    }

    fn comment(author: &str, at: &str, body: &str) -> PrComment {
        PrComment {
            author: author.into(),
            at: at.into(),
            review_state: String::new(),
            body: body.into(),
        }
    }

    fn page() -> PrDetail {
        PrDetail {
            author: "owner".into(),
            comments: vec![
                comment("owner", "2026-11-14T10:00:00Z", "## Plan as approved\n1. x"),
                comment("owner", "2026-11-14T11:00:00Z", "Rename the flag please"),
                comment("cubic-dev-ai", "2026-11-14T11:30:00Z", "P1: unsafe"),
                comment(
                    "owner",
                    "2026-11-14T11:40:00Z",
                    "Done.\n\n🤖 Generated with [Claude Code](https://claude.com)",
                ),
            ],
            threads: vec![ReviewThread {
                id: "t".into(),
                path: "src/a.rs".into(),
                line: Some(12),
                start_line: None,
                original_line: None,
                side: Side::Right,
                resolved: false,
                outdated: false,
                comments: vec![
                    ThreadComment {
                        id: "c1".into(),
                        author: "owner".into(),
                        body: "Use the registry here".into(),
                        at: "2026-11-14T12:00:00Z".into(),
                        mine: true,
                        pending: false,
                    },
                    ThreadComment {
                        id: "c2".into(),
                        author: "owner".into(),
                        body: "not sent yet".into(),
                        at: "2026-11-14T12:05:00Z".into(),
                        mine: true,
                        pending: true,
                    },
                ],
            }],
            ..PrDetail::default()
        }
    }

    /// Only what the owner wrote, submitted: the plan orion posted, an
    /// agent's footer-signed comment, Cubic and an unsent review comment
    /// are left out; a review comment keeps its file and line.
    #[test]
    fn the_owners_comments_are_the_owners_alone() {
        let found = owner_comments(&page());
        let bodies: Vec<&str> = found.iter().map(|c| c.body.as_str()).collect();
        assert_eq!(bodies, ["Rename the flag please", "Use the registry here"]);
        assert_eq!(found[1].place.as_deref(), Some("`src/a.rs` line 12"));
        let text = comments_text(&found);
        assert!(
            text.contains("On `src/a.rs` line 12:\n> Use the registry here"),
            "{text}"
        );
        assert!(text.contains("On the pull request:\n> Rename the flag please"));
        assert_eq!(
            plan_comment(&page()).as_deref(),
            Some("## Plan as approved\n1. x")
        );
    }

    /// What an agent posted through the owner's `gh`, signed with the
    /// marker every orion prompt asks for, is not the owner's; and every
    /// turn the router sends asks for it.
    #[test]
    fn an_agents_signed_comment_is_not_the_owners() {
        let mut detail = page();
        detail.comments.push(comment(
            "owner",
            "2026-11-14T13:00:00Z",
            &format!("Renamed it, as asked.\n\n{AGENT_MARKER}"),
        ));
        let bodies: Vec<String> = owner_comments(&detail)
            .into_iter()
            .map(|c| c.body)
            .collect();
        assert_eq!(bodies, ["Rename the flag please", "Use the registry here"]);
        assert!(MARKER_RULE.contains(AGENT_MARKER));
        assert!(super::super::PRESET_TEXT.contains(AGENT_MARKER));
        let fix = crate::automations::pr_fixer::sample_form();
        let words = Work::Words {
            project: ProjectId("p".into()),
            pr: Box::new(fix.pr.clone()),
            text: "Do it".into(),
            plan: None,
            reason: String::new(),
        };
        assert!(turn_text(&words).ends_with(MARKER_RULE));
        assert!(turn_text(&Work::Fix(Box::new(fix))).ends_with(MARKER_RULE));
    }

    /// The first look baselines — nothing said before is relayed — and
    /// after it only what is newer, once.
    #[test]
    fn comments_are_relayed_once_and_history_never() {
        let all = owner_comments(&page());
        let (new, seen) = new_since(&all, None);
        assert!(new.is_empty(), "history");
        assert_eq!(seen, "2026-11-14T12:00:00Z");
        let (new, again) = new_since(&all, Some("2026-11-14T11:00:00Z"));
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].body, "Use the registry here");
        assert_eq!(again, seen);
        let (new, _) = new_since(&all, Some(&seen));
        assert!(new.is_empty(), "relayed once");
    }

    /// A session's transcript tail drops the line the cut fell in.
    #[test]
    fn the_tail_drops_its_cut_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, "first line\nsecond\nthird\n").unwrap();
        assert_eq!(tail(&path, 100).unwrap(), "first line\nsecond\nthird\n");
        assert_eq!(tail(&path, 10).unwrap(), "third\n");
    }
}
