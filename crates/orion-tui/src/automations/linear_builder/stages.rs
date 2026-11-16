//! Where each picked-up ticket stands, brought up to date every tick
//! ([`follow`]), and what it records in its run (`factory::runs`):
//!
//! - **Starting → Planning**: its session found by its branch; one that
//!   never shows within ten minutes is let go, to be picked up again.
//! - **Approving**: its plan waits on you — or, when the plan's first
//!   line is `ORION-VERDICT: <already-fixed|duplicate|not-reproducible|
//!   wont-build> <reason>`, the no-change card asks whether to close the
//!   ticket instead ([`parse_verdict`]).
//! - **Building**: the plan answered — approved, or sent back with
//!   changes, which runs the session again too. Approved (its transcript
//!   says so), the protected policy files it names are written to the
//!   worktree's `.orion/approved-paths.json` for the repo's guard, afresh
//!   on every approval; sent back, or not known, the file is emptied
//!   (`approved_paths`).
//! - **PrOpen**, **Merged**: a pull request on its branch, open, merged.
//! - **Parked**: the ticket left your hands — Backlog, canceled, done by
//!   someone else, or no longer yours — and its session is archived.
//! - **Dead**: its session or worktree gone [`DEAD_MS`] with no pull
//!   request, said on a card. Its session back (or a pull request on its
//!   branch), it takes up where it was.
//! - **Stalled** (a card, not a stage): its session stopped while
//!   planning or building with no pull request and no question for
//!   [`STALL_MS`].
//! - **Held** on a card to you: let go when the ticket leaves your hands
//!   — out of Triage and Todo, canceled, done, unassigned or someone
//!   else's — to be checked afresh should it come back.
//! - A thin ticket's reporter is reminded once, after [`PING_MS`].
//! - A **leftover** an earlier orion wrote — no history, its session gone,
//!   no pull request — is let go quietly, to be picked up afresh
//!   ([`drop_leftovers`]). One whose session is there is followed like any
//!   other.
//!
//! Your cards on a ticket close as what they were about ends: Stalled
//! when its session runs again or a pull request opens, Dead when its
//! session comes back, and every one when the ticket leaves your hands —
//! so a card nobody answers needn't hold the factory's intake shut
//! (`automations::attention`).
//!
//! Each stage reached, each question asked (the plan, an AskUserQuestion)
//! and when it was answered go into the run record.

use std::collections::{HashMap, HashSet};

use orion_core::{AgentId, AgentStatus, WaitReason};

use super::{Entry, Stage, KEEP_MS, START_LIMIT_MS};
use crate::app::App;
use crate::factory::runs::{Question, RunRecord, StageAt};

/// A session stopped this long with nothing to show gets a Stalled card.
pub(crate) const STALL_MS: i64 = 10 * orion_core::clock::MINUTE_MS;
/// A session or worktree gone this long is dead.
pub(crate) const DEAD_MS: i64 = 30 * orion_core::clock::MINUTE_MS;
/// A thin ticket's reporter is reminded once after this long.
pub(crate) const PING_MS: i64 = 2 * orion_core::clock::DAY_MS;

/// What a plan says nothing should be built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    AlreadyFixed,
    Duplicate,
    NotReproducible,
    WontBuild,
}

/// A plan's `ORION-VERDICT:` line, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoChange {
    pub verdict: Verdict,
    pub reason: String,
    /// The line as written.
    pub line: String,
}

/// The verdict in `plan`'s first line —
/// `^ORION-VERDICT:\s*(already-fixed|duplicate|not-reproducible|wont-build)\s+(.+)$`
/// — or None for a plan that builds something.
pub(crate) fn parse_verdict(plan: &str) -> Option<NoChange> {
    let line = plan.lines().find(|l| !l.trim().is_empty())?.trim();
    let rest = line.strip_prefix("ORION-VERDICT:")?.trim_start();
    let (word, reason) = rest.split_once(char::is_whitespace)?;
    let verdict = match word {
        "already-fixed" => Verdict::AlreadyFixed,
        "duplicate" => Verdict::Duplicate,
        "not-reproducible" => Verdict::NotReproducible,
        "wont-build" => Verdict::WontBuild,
        _ => return None,
    };
    let reason = reason.trim();
    (!reason.is_empty()).then(|| NoChange {
        verdict,
        reason: reason.to_string(),
        line: line.to_string(),
    })
}

/// The first ticket identifier `text` names (`RIPLO-12`), uppercased.
pub(crate) fn named_ticket(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find_map(|word| {
            let (team, number) = word.split_once('-')?;
            let ok = !team.is_empty()
                && team.chars().all(|c| c.is_ascii_alphabetic())
                && !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit());
            ok.then(|| word.to_ascii_uppercase())
        })
}

/// `stage` reached at `at`, in a run record.
pub(crate) fn push_stage(record: &mut RunRecord, stage: Stage, at: i64) {
    record.stages.push(StageAt {
        stage: stage.name().into(),
        at_ms: at,
    });
}

/// A question's tag, from the `[product]`-style tag it was asked with.
fn tag_of(text: &str) -> Option<String> {
    ["product", "arch", "security", "dep", "scope", "copy", "ux"]
        .into_iter()
        .find(|t| text.contains(&format!("[{t}]")))
        .map(str::to_string)
}

/// What to write in a run record.
enum Rec {
    Stage(Stage),
    Session(String),
    Ask(Question),
    Answered(Option<String>),
}

/// What to do once the ledger is walked.
enum Do {
    Ping(String),
    NoChange(String, NoChange),
    Stalled(String, AgentId),
    Dead(String),
    Archive(AgentId),
    /// A plan answered in the worktree at this path by this session:
    /// approved, or sent back — its transcript says which.
    Approved(std::path::PathBuf, String, AgentId),
    /// Your cards on the ticket closed, with this note: what they were
    /// about has ended.
    Close(String, &'static str),
}

/// Bring the ledger up to date with the sessions, the pull requests and
/// Linear: see the module's doc.
pub(crate) fn follow(app: &mut App) {
    if app.automations.linear_builder.adopt_skips() {
        app.automations.linear_builder.save();
    }
    let now = crate::app::now_ms();
    let mut todo: Vec<Do> = Vec::new();
    let mut records: Vec<(String, Rec)> = Vec::new();
    {
        let tree = &app.tree;
        let reasons = &app.needs_you.reasons;
        let open_prs = &app.open_prs;
        let lists = &app.linear;
        let shared = &mut app.automations.linear_builder;
        let ctx = Ctx {
            tree,
            reasons,
            open_prs,
            lists,
        };
        let mut changed = drop_leftovers(&mut shared.ledger, ctx, now);
        shared.ledger.issues.retain(|id, entry| {
            if now - entry.at > KEEP_MS {
                changed = true;
                return false;
            }
            let before = entry.clone();
            let mut rec = |r: Rec| records.push((entry_identifier(&before), r));
            let keep = step(id, entry, now, ctx, &mut rec, &mut todo);
            changed |= !keep || *entry != before;
            keep
        });
        changed |= shared.ledger.prune_previous(now);
        if changed {
            shared.save();
        }
        for (identifier, rec) in records {
            crate::factory::runs::update(&identifier, |r| apply(r, rec, now));
        }
    }
    let mut carded: HashSet<String> = HashSet::new();
    for item in todo {
        match item {
            Do::Ping(id) => super::owner::ping(app, &id),
            Do::NoChange(id, nc) => {
                if carded.insert(group_key(app, &id)) {
                    super::owner::post_no_change(app, &id, &nc);
                }
            }
            Do::Stalled(id, session) => {
                if carded.insert(group_key(app, &id)) {
                    stalled(app, id, &session);
                }
            }
            Do::Dead(id) => {
                if carded.insert(group_key(app, &id)) {
                    super::owner::post_dead(app, &id);
                }
            }
            Do::Archive(session) => super::owner::archive(app, session),
            Do::Approved(dir, plan, session) => {
                super::approved_paths::approve(app, dir, plan, &session)
            }
            Do::Close(id, note) => super::owner::close_cards(app, &id, note),
        }
    }
}

fn entry_identifier(entry: &Entry) -> String {
    entry.identifier.clone()
}

/// A session's group: one card for all its tickets.
fn group_key(app: &App, id: &str) -> String {
    app.automations
        .linear_builder
        .ledger
        .issues
        .get(id)
        .filter(|e| !e.branch.is_empty())
        .map_or_else(|| id.to_string(), |e| format!("{}:{}", e.project, e.branch))
}

/// One record change.
fn apply(record: &mut RunRecord, rec: Rec, now: i64) {
    match rec {
        Rec::Stage(stage) => push_stage(record, stage, now),
        Rec::Session(id) => {
            if !record.sessions.contains(&id) {
                record.sessions.push(id);
            }
        }
        Rec::Ask(q) => record.questions.push(q),
        Rec::Answered(answer) => {
            for q in record
                .questions
                .iter_mut()
                .filter(|q| q.answered_at_ms.is_none())
            {
                q.answered_at_ms = Some(now);
                if q.answer.is_none() {
                    q.answer = answer.clone();
                }
            }
        }
    }
}

/// What [`step`] reads.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    tree: &'a crate::app::Tree,
    reasons: &'a HashMap<AgentId, WaitReason>,
    open_prs: &'a HashMap<orion_core::ProjectId, crate::app::OpenPrs>,
    lists: &'a HashMap<orion_core::ProjectId, crate::linear::LinearList>,
}

/// One ticket brought up to date; false lets it go.
fn step(
    id: &str,
    entry: &mut Entry,
    now: i64,
    ctx: Ctx,
    rec: &mut dyn FnMut(Rec),
    todo: &mut Vec<Do>,
) -> bool {
    let stage_to = |entry: &mut Entry, stage: Stage, rec: &mut dyn FnMut(Rec)| {
        if entry.set_stage(stage, now) {
            rec(Rec::Stage(stage));
        }
    };
    let close = |todo: &mut Vec<Do>, note| todo.push(Do::Close(id.to_string(), note));
    match entry.stage {
        Stage::WaitingReporter => {
            if !entry.pinged && now - entry.since() >= PING_MS {
                entry.pinged = true;
                todo.push(Do::Ping(id.to_string()));
            }
            return true;
        }
        Stage::Held => {
            // Out of your hands, it is let go, its card with it.
            let gone = held_left_your_hands(ctx, id, entry);
            if gone {
                close(todo, LEFT);
            }
            return !gone;
        }
        Stage::PrOpen => {
            if let Some(url) = merged_pr(ctx, entry) {
                entry.pr_url = Some(url);
                stage_to(entry, Stage::Merged, rec);
            }
            return true;
        }
        Stage::Dead => {
            if left_your_hands(ctx, id, entry) {
                stage_to(entry, Stage::Parked, rec);
                close(todo, LEFT);
                return true;
            }
            if !(session_back(ctx, entry) || has_pr(ctx, entry)) {
                return true;
            }
            // Back: where it was before it died, and on from there.
            let was = entry
                .history
                .iter()
                .rev()
                .map(|(s, _)| *s)
                .find(|s| s.is_active())
                .unwrap_or(Stage::Planning);
            entry.gone_since = None;
            stage_to(entry, was, rec);
            close(todo, "Its session is back");
        }
        stage if !stage.is_active() => return true,
        _ => {}
    }
    // A pull request on its branch.
    if !entry.branch.is_empty() {
        if let Some(url) = merged_pr(ctx, entry) {
            entry.pr_url = Some(url);
            entry.stalled = false;
            stage_to(entry, Stage::Merged, rec);
            close(todo, "Merged");
            return true;
        }
        if let Some(url) = open_pr(ctx, entry) {
            entry.pr_url = Some(url);
            entry.stalled = false;
            stage_to(entry, Stage::PrOpen, rec);
            close(todo, "PR opened");
            return true;
        }
    }
    // The ticket out of your hands.
    if left_your_hands(ctx, id, entry) {
        stage_to(entry, Stage::Parked, rec);
        if let Some(session) = entry.session.clone() {
            todo.push(Do::Archive(session));
        }
        close(todo, LEFT);
        return true;
    }
    let Some(session) = entry.session.clone() else {
        let found = ctx.tree.agents.iter().find(|a| {
            a.automation.as_deref() == Some(entry.automation.as_str())
                && ctx.tree.worktrees.iter().any(|w| {
                    w.id == a.worktree_id
                        && w.project_id == entry.project
                        && w.branch == entry.branch
                })
        });
        return match found {
            Some(agent) => {
                entry.session = Some(agent.id.clone());
                rec(Rec::Session(agent.id.to_string()));
                stage_to(entry, Stage::Planning, rec);
                true
            }
            None => now - entry.at <= START_LIMIT_MS,
        };
    };
    let agent = ctx
        .tree
        .agents
        .iter()
        .find(|a| a.id == session && !a.archived);
    let checkout = ctx
        .tree
        .worktrees
        .iter()
        .find(|w| w.project_id == entry.project && w.branch == entry.branch);
    let Some(agent) = agent.filter(|_| checkout.is_some()) else {
        let since = *entry.gone_since.get_or_insert(now);
        if now - since >= DEAD_MS {
            stage_to(entry, Stage::Dead, rec);
            todo.push(Do::Dead(id.to_string()));
        }
        return true;
    };
    entry.gone_since = None;
    let waiting = match agent.status {
        AgentStatus::NeedsFeedback => ctx.reasons.get(&session),
        _ => None,
    };
    let plan = match waiting {
        Some(WaitReason::Plan { plan }) => Some(plan.as_str()),
        _ => None,
    };
    let next = match entry.stage {
        _ if plan.is_some() => Stage::Approving,
        Stage::Approving if agent.status == AgentStatus::Running => Stage::Building,
        stage => stage,
    };
    if next != entry.stage {
        let was = entry.stage;
        stage_to(entry, next, rec);
        if was == Stage::Approving && next == Stage::Building {
            rec(Rec::Answered(Some("approved".into())));
            entry.asking = false;
            if let (Some(plan), Some(w)) = (entry.plan.take(), checkout) {
                todo.push(Do::Approved(w.path.clone(), plan, session.clone()));
            }
        }
    }
    if let Some(plan) = plan {
        if entry.plan.as_deref() != Some(plan) {
            entry.plan = Some(plan.to_string());
        }
        if !entry.asking {
            entry.asking = true;
            rec(Rec::Ask(Question {
                text: plan
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .into(),
                tag: Some("plan".into()),
                asked_at_ms: now,
                ..Question::default()
            }));
        }
        if let Some(nc) = parse_verdict(plan) {
            // A plan you sent back is answered until the session moves on.
            let answered = entry
                .steered_at
                .is_some_and(|at| agent.status_changed_at <= at);
            if !answered && entry.verdict.as_deref() != Some(nc.line.as_str()) {
                entry.verdict = Some(nc.line.clone());
                todo.push(Do::NoChange(id.to_string(), nc));
            }
        }
    }
    if let Some(WaitReason::Question { questions }) = waiting {
        if !entry.asking {
            entry.asking = true;
            for q in questions {
                rec(Rec::Ask(Question {
                    text: q.question.clone(),
                    tag: tag_of(&format!("{} {}", q.header, q.question)),
                    asked_at_ms: now,
                    ..Question::default()
                }));
            }
        }
    }
    if agent.status == AgentStatus::Running {
        if entry.stalled {
            entry.stalled = false;
            close(todo, "Running again");
        }
        if entry.asking {
            entry.asking = false;
            rec(Rec::Answered(None));
        }
    }
    let quiet = agent.status == AgentStatus::Finished
        && now - agent.status_changed_at >= STALL_MS
        && matches!(entry.stage, Stage::Planning | Stage::Building)
        && entry.pr_url.is_none();
    if quiet && !entry.stalled {
        entry.stalled = true;
        todo.push(Do::Stalled(id.to_string(), session));
    }
    true
}

/// Whether `entry` is a leftover from before the factory: written by an
/// earlier orion (no history), at work by its stage, its session gone or
/// archived and no pull request on its branch. Only once the daemon's
/// tree has its project and its project's pull requests have been read,
/// so a leftover is never mistaken while orion is still starting.
fn leftover(ctx: Ctx, entry: &Entry) -> bool {
    let known = ctx.tree.projects.iter().any(|p| p.id == entry.project)
        && ctx.open_prs.contains_key(&entry.project);
    let gone = entry.session.as_ref().is_some_and(|session| {
        !ctx.tree
            .agents
            .iter()
            .any(|a| &a.id == session && !a.archived)
    });
    entry.history.is_empty() && entry.stage.is_active() && known && gone && !has_pr(ctx, entry)
}

/// Let go, quietly, the leftovers an earlier orion wrote ([`leftover`]):
/// no Dead card each, and the ticket, still yours in Todo, is picked up
/// afresh — its old branch kept in `Ledger::previous`, so its worktree
/// doesn't hold it back. Whether any were.
fn drop_leftovers(ledger: &mut super::Ledger, ctx: Ctx, now: i64) -> bool {
    let ids: Vec<String> = ledger
        .issues
        .iter()
        .filter(|(_, entry)| leftover(ctx, entry))
        .map(|(id, _)| id.clone())
        .collect();
    for id in &ids {
        let Some(entry) = ledger.issues.remove(id) else {
            continue;
        };
        tracing::info!(
            "automations: let go of {} ({}, on {}), left over from an earlier orion: its session is gone",
            entry.identifier,
            entry.stage.name(),
            entry.branch,
        );
        if !entry.branch.is_empty() {
            ledger.add_previous(id.clone(), entry.branch, now);
        }
    }
    !ids.is_empty()
}

/// The note on a card closed because its ticket left your hands.
const LEFT: &str = "No longer in your hands";

/// Whether `entry`'s session is there again — not archived, its checkout
/// with it.
fn session_back(ctx: Ctx, entry: &Entry) -> bool {
    let Some(session) = &entry.session else {
        return false;
    };
    let agent = ctx
        .tree
        .agents
        .iter()
        .any(|a| &a.id == session && !a.archived);
    let checkout = ctx
        .tree
        .worktrees
        .iter()
        .any(|w| w.project_id == entry.project && w.branch == entry.branch);
    agent && checkout
}

/// Whether a pull request, open or merged, is on `entry`'s branch.
fn has_pr(ctx: Ctx, entry: &Entry) -> bool {
    !entry.branch.is_empty() && (open_pr(ctx, entry).is_some() || merged_pr(ctx, entry).is_some())
}

/// The open pull request on `entry`'s branch.
fn open_pr(ctx: Ctx, entry: &Entry) -> Option<String> {
    let prs = ctx.open_prs.get(&entry.project)?;
    prs.list
        .iter()
        .find(|pr| pr.head == entry.branch)
        .map(|pr| pr.url.clone())
}

/// The merged pull request on `entry`'s branch.
fn merged_pr(ctx: Ctx, entry: &Entry) -> Option<String> {
    let prs = ctx.open_prs.get(&entry.project)?;
    prs.merged
        .iter()
        .find(|pr| pr.head == entry.branch && !pr.meta.merged_at.is_empty())
        .map(|pr| pr.url.clone())
}

/// Whether Linear's last word on the ticket — the LINEAR VIEW's lists,
/// only when heard after it was picked up — says it left your hands:
/// moved to the backlog, canceled, done by someone else (its pull request
/// never merged), unassigned or given to someone else.
fn left_your_hands(ctx: Ctx, id: &str, entry: &Entry) -> bool {
    heard_since(ctx, id, entry.at).is_some_and(|issue| {
        matches!(
            issue.status_type.as_str(),
            "backlog" | "canceled" | "completed"
        ) || not_yours(issue, entry)
    })
}

/// [`left_your_hands`] for a ticket held on a card, not yet started:
/// heard since it was held, out of Triage and Todo — started, put back,
/// canceled or done — unassigned, or someone else's.
fn held_left_your_hands(ctx: Ctx, id: &str, entry: &Entry) -> bool {
    heard_since(ctx, id, entry.since()).is_some_and(|issue| {
        !matches!(issue.status_type.as_str(), "triage" | "unstarted") || not_yours(issue, entry)
    })
}

/// The LINEAR VIEW's word on ticket `id`, when heard after `since` (ms).
fn heard_since<'a>(ctx: Ctx<'a>, id: &str, since: i64) -> Option<&'a crate::linear::LinearIssue> {
    let issue = ctx
        .lists
        .values()
        .flat_map(|l| &l.list)
        .find(|i| i.id == id)?;
    let heard = crate::pull_request::rfc3339_ms(&issue.updated_at).unwrap_or(0);
    (heard > since).then_some(issue)
}

/// Unassigned, or given to someone other than whom it came in for.
fn not_yours(issue: &crate::linear::LinearIssue, entry: &Entry) -> bool {
    issue.assignee_id.is_empty()
        || (!entry.owner_id.is_empty() && issue.assignee_id != entry.owner_id)
}

/// The Stalled card on ticket `id`, with the last thing `session` said
/// off its transcript — read on the blocking pool, the card posted when
/// it lands (`linear_builder::Answer::Stalled`). With no loop (the
/// tests), read here.
fn stalled(app: &mut App, id: String, session: &AgentId) {
    use crate::automations::pr_fixer::router::last_said_of;
    let agent = app.tree.agents.iter().find(|a| &a.id == session).cloned();
    match (app.automations.tx.clone(), agent) {
        (Some(tx), Some(agent)) => {
            tokio::task::spawn_blocking(move || {
                let said = last_said_of(&agent, 1).pop();
                let answer = super::Answer::Stalled { issue: id, said };
                let _ = tx.send(crate::automations::Answer::LinearBuilder(answer));
            });
        }
        (_, agent) => {
            let said = agent.and_then(|agent| last_said_of(&agent, 1).pop());
            super::owner::post_stalled(app, &id, said);
        }
    }
}
