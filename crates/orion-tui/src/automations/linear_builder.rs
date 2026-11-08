//! The BUG FIXER and the FEATURE BUILDER: Linear tickets assigned to you
//! that carry one of their labels, planned, approved by you, then built to
//! a draft PR. One implementation, [`LinearBuilder`], set up twice: each
//! with its own labels ([`labels_of`]) and its own SYSTEM PRESET.
//!
//! * **Watch.** Every [`POLL`] ([`interval`]; `ORION_LINEAR_BUILDER_POLL_SECS`
//!   overrides it, `0` turns it off), for each distinct Linear key across
//!   the projects, the tickets assigned to the key's owner that carry one
//!   of the runner's labels and wait in triage, the backlog or todo
//!   ([`issues_query`], through [`crate::linear::graphql`] with
//!   [`crate::linear::ISSUE_FIELDS`]). A ticket already picked up — in the
//!   LEDGER (`<data_dir>/automations/linear-builder.json`, issue id →
//!   session and state), or being placed or queued now — is skipped, by
//!   either runner.
//! * **Route.** A key one project uses goes to that project. A key several
//!   share asks the model once, through [`crate::oneshot::run_json`], with
//!   each ticket and each project's **What it is**
//!   ([`crate::projects::about`]) — see [`routed`].
//! * **Group.** Two or more new tickets for one project are put to the
//!   model once more; it joins only those that clearly share a cause or a
//!   component, and a failed ask is one ticket a group ([`grouped`]).
//! * **Launch.** One session a group, in a fresh worktree named as a ⌘L
//!   launch names it, through the QUICK PROMPT's own launch
//!   ([`super::launch_in_new_worktree`]): its Linear batch moves the
//!   tickets to In Progress and has the draft pull request attached to them
//!   (`linear::remember_submit`). It starts in plan mode, its first prompt
//!   the preset's text with the tickets and the project's related pull
//!   requests at `{task}` ([`context`]). A group waits — "queued, N
//!   running" — while `automations.max_running` automation sessions run.
//!   The plan's approval reaches you through Needs you and the phone, as
//!   any plan waiting does.
//! * **Labels.** Enter on the panel's **Labels** row picks them from the
//!   workspace's issue labels (`issueLabels`) — [`LabelPick`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use orion_core::{AgentId, AgentMode, AgentStatus, ClientRequest, ProjectId};
use serde::{Deserialize, Serialize};

use super::{
    config::AutomationsSettings, Automation, Dot, Panel, PanelList, PanelRow, Runner, SettingRow,
    Tone,
};
use crate::app::{App, Overlay};
use crate::linear::LinearIssue;
use crate::quick_prompt::{QuickLaunch, QuickTarget};

/// What both presets tell the agent after the tickets: the rules for the
/// plan, the build and the draft pull request.
macro_rules! builder_rules {
    () => {
        "How to work:
1. Read the ticket(s) above and every item under \"Related work to check\": an open draft or a recent merge may already touch this.
2. Find the product areas the change touches. Read every decision in the repo's `docs/decisions/<area>/` folders for those areas before you plan. If no folder fits, create a new area folder.
3. Write a plan and wait for approval. Never go against a logged decision without saying so in the plan.
4. After approval, build. Reuse the design system, the component library and the central registries; add no new helper where one exists.
5. Run the unit tests, and only the e2e specs that cover the change. Never run the full e2e suite.
6. Log any new product decision the user made while steering you as a file under `docs/decisions/<area>/`.
7. Push the branch and open a **draft** pull request with `gh pr create --draft`, titled \"<ticket IDs>: <short summary>\", whose description has these sections:

## What and why
Two lines, the tickets linked.

## Blast radius
The modules, the callers of changed functions and the screens touched, rated low, medium or high.

## Decisions
The decision files read, and anything gone against.

## Reuse
The components and helpers used; anything new, and why.

## Neighbours checked
The related flows, and how each was checked.

## UI
Before and after for each state: empty, loading, error, full, narrow. Only for UI changes.

## Performance
Before and after in ms. Only for changes to state.

## Check it yourself
A 3-step flow."
    };
}

/// What the **Bug fixer** preset says while it holds none of the user's.
pub const BUG_FIXER_TEXT: &str = concat!(
    "You are orion's bug fixer. Fix the Linear bug(s) below: find the cause before you change anything, and add a test that fails without the fix.

{task}

",
    builder_rules!()
);

/// What the **Feature builder** preset says while it holds none of the
/// user's.
pub const FEATURE_BUILDER_TEXT: &str = concat!(
    "You are orion's feature builder. Build the Linear feature(s) below, with tests.

{task}

",
    builder_rules!()
);

/// The headings every draft pull request's description has, in order.
pub const PR_SECTIONS: [&str; 8] = [
    "What and why",
    "Blast radius",
    "Decisions",
    "Reuse",
    "Neighbours checked",
    "UI",
    "Performance",
    "Check it yourself",
];

/// How often each runner looks for tickets.
pub const POLL: Duration = Duration::from_secs(2 * 60);

/// The look's cadence: `ORION_LINEAR_BUILDER_POLL_SECS` when set, `None`
/// when it is `0` (off); else [`POLL`].
pub fn interval() -> Option<Duration> {
    orion_core::env::secs_override(orion_core::env::LINEAR_BUILDER_POLL_SECS, POLL)
}

/// The labels each runner picks up while `automations.per.<id>.labels` is
/// empty.
pub const BUG_LABELS: &[&str] = &["Bug", "Regression", "Customer bug"];
pub const FEATURE_LABELS: &[&str] = &["Feature", "Improvement"];

/// The labels `which` picks tickets up by: its configured ones, else its
/// own defaults.
pub fn labels_of(settings: &AutomationsSettings, which: Automation) -> Vec<String> {
    let own = settings.of(which).labels;
    if !own.is_empty() {
        return own;
    }
    let defaults = match which {
        Automation::FeatureBuilder => FEATURE_LABELS,
        _ => BUG_LABELS,
    };
    defaults.iter().map(|l| l.to_string()).collect()
}

/// Whether `issue` carries one of `labels`, by name, in any case.
pub fn carries(issue: &LinearIssue, labels: &[String]) -> bool {
    issue.labels.iter().any(|tag| {
        labels
            .iter()
            .any(|l| l.trim().eq_ignore_ascii_case(tag.name.trim()))
    })
}

/// A state Linear counts as not yet started: triage, the backlog or todo.
const WAITING_STATES: [&str; 3] = ["triage", "backlog", "unstarted"];

/// The most tickets one look reads.
const ISSUES_LIMIT: usize = 50;

/// The look: tickets assigned to the key's owner, waiting to start, with
/// one of `labels` — as Linear's `IssueFilter` has it: `assignee.isMe`,
/// `state.type.in`, and `labels.some.or` of `name.eqIgnoreCase`. Aliased
/// `mine`, so `linear::parse_issues` reads it as it reads the LINEAR
/// VIEW's.
pub fn issues_query(labels: &[String]) -> (String, serde_json::Value) {
    let states: Vec<String> = WAITING_STATES.iter().map(|s| format!("\"{s}\"")).collect();
    let query = format!(
        "query($labels: [IssueLabelFilter!]) {{ mine: issues(first: {ISSUES_LIMIT}, filter: {{ \
         assignee: {{ isMe: {{ eq: true }} }}, \
         state: {{ type: {{ in: [{}] }} }}, \
         labels: {{ some: {{ or: $labels }} }} }}) {{ nodes {{ {} }} }} }}",
        states.join(", "),
        crate::linear::ISSUE_FIELDS
    );
    let labels: Vec<serde_json::Value> = labels
        .iter()
        .map(|l| serde_json::json!({ "name": { "eqIgnoreCase": l.trim() } }))
        .collect();
    (query, serde_json::json!({ "labels": labels }))
}

/// The workspace's issue labels: `issueLabels`, groups left out.
const LABELS_QUERY: &str =
    "query { issueLabels(first: 250) { nodes { name isGroup } pageInfo { hasNextPage } } }";

/// The label names in an answer to [`LABELS_QUERY`], groups left out.
fn parse_labels(json: &serde_json::Value) -> Result<Vec<String>, String> {
    let nodes = json
        .pointer("/data/issueLabels/nodes")
        .and_then(|n| n.as_array())
        .ok_or_else(|| {
            crate::linear::graphql_error(json).unwrap_or_else(|| "Linear sent no labels".into())
        })?;
    Ok(nodes
        .iter()
        .filter(|n| n.get("isGroup").and_then(|g| g.as_bool()) != Some(true))
        .filter_map(|n| Some(n.get("name")?.as_str()?.trim().to_string()))
        .filter(|n| !n.is_empty())
        .collect())
}

// ---- the ledger ----

/// Where a picked-up ticket's session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Its checkout and session are being made.
    Starting,
    /// Planning, in plan mode.
    Planning,
    /// Its plan is up for approval.
    Approving,
    /// Building, the plan approved.
    Building,
}

/// One picked-up ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The runner that picked it up (`Automation::id`).
    pub automation: String,
    pub identifier: String,
    pub title: String,
    pub project: ProjectId,
    /// The fresh worktree's branch: how its session is found.
    pub branch: String,
    #[serde(default)]
    pub session: Option<AgentId>,
    pub stage: Stage,
    /// When it was picked up, in ms.
    pub at: i64,
}

/// Every ticket the bug fixer and the feature builder have picked up, by
/// Linear issue id: never picked up twice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    pub issues: BTreeMap<String, Entry>,
}

/// A ticket whose session hasn't shown up in this long is let go, to be
/// picked up again on a later look.
const START_LIMIT_MS: i64 = 10 * 60 * 1000;
/// How long the ledger remembers a ticket.
const KEEP_MS: i64 = 60 * 24 * 60 * 60 * 1000;

/// The tickets routed and grouped, waiting for a place under the cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub which: Automation,
    pub project: ProjectId,
    pub issues: Vec<LinearIssue>,
}

/// The bug fixer's and feature builder's shared state (`Engine::
/// linear_builder`).
#[derive(Debug, Default)]
pub struct Shared {
    pub ledger: Ledger,
    /// Where the ledger is kept; None (the tests) keeps it in memory.
    path: Option<PathBuf>,
    /// Groups waiting for a place under the cap, oldest first.
    pub queue: Vec<Group>,
    /// Tickets being routed and grouped now.
    claimed: HashSet<String>,
    /// When each runner looks next.
    next_look: HashMap<Automation, Instant>,
}

impl Shared {
    /// Whether the ticket `issue_id` is picked up already — launched,
    /// queued or being placed.
    pub fn picked_up(&self, issue_id: &str) -> bool {
        self.ledger.issues.contains_key(issue_id)
            || self.claimed.contains(issue_id)
            || self
                .queue
                .iter()
                .any(|g| g.issues.iter().any(|i| i.id == issue_id))
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let written = serde_json::to_vec_pretty(&self.ledger)
            .map_err(std::io::Error::other)
            .and_then(|bytes| orion_core::settings::write_atomic(path, &bytes));
        if let Err(err) = written {
            tracing::warn!("automations: writing the Linear ledger: {err}");
        }
    }
}

/// Whether Linear's ticket `issue_id` has been picked up by the bug fixer
/// or the feature builder: Needs you leaves its assignment off the board.
pub fn picked_up(app: &App, issue_id: &str) -> bool {
    app.automations.linear_builder.picked_up(issue_id)
}

/// The ledger from the last run, as orion starts.
pub fn load(app: &mut App) {
    let path = orion_core::paths::data_dir()
        .join("automations")
        .join("linear-builder.json");
    let ledger = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let shared = &mut app.automations.linear_builder;
    shared.ledger = ledger;
    shared.path = Some(path);
}

// ---- the runner ----

/// The bug fixer or the feature builder.
pub struct LinearBuilder(pub Automation);

impl Runner for LinearBuilder {
    fn automation(&self) -> Automation {
        self.0
    }

    fn tick(&mut self, app: &mut App, out: &mut Vec<ClientRequest>) {
        follow(app);
        launch_queued(app, self.0, out);
        let now = Instant::now();
        let due = app
            .automations
            .linear_builder
            .next_look
            .get(&self.0)
            .is_none_or(|at| *at <= now);
        if let (true, Some(every)) = (due, interval()) {
            app.automations
                .linear_builder
                .next_look
                .insert(self.0, now + every);
            look(app, self.0);
        }
    }

    fn panel(&self, app: &App) -> Panel {
        panel(app, self.0)
    }

    fn edit(&mut self, app: &mut App, row: SettingRow) -> bool {
        match row {
            SettingRow::Labels => open_pick(app, self.0),
            _ => false,
        }
    }
}

/// What lands from off the loop.
#[derive(Debug)]
pub enum Answer {
    /// One key's tickets for `which`; `projects` are the projects that key
    /// is read from.
    Fetched {
        which: Automation,
        projects: Vec<ProjectId>,
        issues: Result<Vec<LinearIssue>, String>,
    },
    /// Tickets routed and grouped; `released` are the ones that could not
    /// be placed, to be looked at again.
    Placed {
        which: Automation,
        groups: Vec<Group>,
        released: Vec<String>,
    },
    /// The workspace's issue labels, for the labels pick.
    Labels(Result<Vec<String>, String>),
}

/// Ask Linear, under every distinct key, for `which`'s tickets.
fn look(app: &mut App, which: Automation) {
    let Some(tx) = app.automations.tx.clone() else {
        return;
    };
    let (query, variables) = issues_query(&labels_of(&app.automations.settings, which));
    for (key, projects) in crate::linear::keys_by_project(app) {
        let (tx, query, variables) = (tx.clone(), query.clone(), variables.clone());
        tokio::spawn(async move {
            let issues = crate::linear::graphql(&key, &query, variables)
                .await
                .and_then(|json| crate::linear::parse_issues(&json));
            let _ = tx.send(super::Answer::LinearBuilder(Answer::Fetched {
                which,
                projects,
                issues,
            }));
        });
    }
}

pub fn land(app: &mut App, answer: Answer, out: &mut Vec<ClientRequest>) {
    match answer {
        Answer::Fetched {
            which,
            projects,
            issues,
        } => match issues {
            Ok(issues) => fetched(app, which, projects, issues),
            Err(why) => tracing::warn!("{}: asking Linear: {why}", which.name()),
        },
        Answer::Placed {
            which,
            groups,
            released,
        } => {
            placed(app, groups, released);
            if app.automations.is_on(which) {
                launch_queued(app, which, out);
            }
        }
        Answer::Labels(names) => {
            if let Some(Overlay::Automations(view)) = &mut app.overlay {
                if let Some(pick) = &mut view.labels {
                    pick.names = Some(names);
                }
            }
        }
    }
}

/// The tickets in `issues` that `which` picks up now: carrying one of its
/// labels, waiting to start, and not picked up already.
pub fn fresh(shared: &Shared, labels: &[String], issues: Vec<LinearIssue>) -> Vec<LinearIssue> {
    issues
        .into_iter()
        .filter(|i| carries(i, labels))
        .filter(|i| WAITING_STATES.contains(&i.status_type.as_str()))
        .filter(|i| !shared.picked_up(&i.id))
        .collect()
}

/// A look's tickets: the fresh ones claimed, and put to the model to be
/// routed and grouped off the loop.
fn fetched(app: &mut App, which: Automation, projects: Vec<ProjectId>, issues: Vec<LinearIssue>) {
    let labels = labels_of(&app.automations.settings, which);
    let tickets = fresh(&app.automations.linear_builder, &labels, issues);
    if tickets.is_empty() {
        return;
    }
    let Some(tx) = app.automations.tx.clone() else {
        return;
    };
    let mut candidates = Vec::new();
    // Only a choice between projects reads what each one is.
    let several = projects.len() > 1;
    for id in projects {
        let Some(project) = app.tree.projects.iter().find(|p| p.id == id).cloned() else {
            continue;
        };
        let about = match several {
            true => crate::projects::about(app, &project.repo_path),
            false => String::new(),
        };
        candidates.push(Candidate {
            project: project.id,
            name: project.name,
            about,
        });
    }
    if candidates.is_empty() {
        return;
    }
    let shared = &mut app.automations.linear_builder;
    shared.claimed.extend(tickets.iter().map(|t| t.id.clone()));
    let account = crate::oneshot::default_account(&crate::config::Config::load());
    tokio::spawn(async move {
        let answer = place(which, tickets, candidates, account).await;
        let _ = tx.send(super::Answer::LinearBuilder(answer));
    });
}

/// Placed tickets: queued in their groups, and the rest let go.
fn placed(app: &mut App, groups: Vec<Group>, released: Vec<String>) {
    let shared = &mut app.automations.linear_builder;
    for group in &groups {
        for issue in &group.issues {
            shared.claimed.remove(&issue.id);
        }
    }
    for id in &released {
        shared.claimed.remove(id);
    }
    shared.queue.extend(groups);
}

/// Start `which`'s queued groups while the cap has room.
fn launch_queued(app: &mut App, which: Automation, out: &mut Vec<ClientRequest>) {
    while super::may_start(app) {
        let queue = &mut app.automations.linear_builder.queue;
        let Some(at) = queue.iter().position(|g| g.which == which) else {
            return;
        };
        let group = queue.remove(at);
        if let Err(group) = launch(app, group, out) {
            app.automations.linear_builder.queue.insert(at, group);
            return;
        }
    }
}

/// One group's session, in a fresh worktree in plan mode — or the group
/// back, untouched, when the cap refused it. A group whose project is gone
/// is dropped, to be picked up again.
fn launch(app: &mut App, group: Group, out: &mut Vec<ClientRequest>) -> Result<(), Group> {
    if !app.tree.projects.iter().any(|p| p.id == group.project) {
        return Ok(());
    }
    let which = group.which;
    let cfg = crate::config::Config::load();
    let settings = &app.automations.settings;
    let preset = super::preset_of(settings, which);
    let extra = settings.of(which).extra_steer;
    let prompt = super::compose(&preset, &context(app, &group), &extra);
    let mut batch = crate::linear::LinearBatch::of(group.issues.clone());
    batch.task = prompt;
    let branch = batch.branch(&app.project_branches(&group.project));
    let target = QuickTarget::NewWorktree {
        project: group.project.clone(),
        branch: branch.clone(),
        existing: false,
    };
    // The preset's harness, account, model and effort; its text is the
    // batch's task already.
    let mut launch = QuickLaunch::of_preset(target, preset, None, &cfg);
    launch.preset = None;
    let launch = launch
        .with_linear(Some(batch))
        .with_mode(AgentMode::Plan, &cfg);
    if !super::launch_in_new_worktree(app, which, launch, out) {
        return Err(group);
    }
    let now = crate::app::now_ms();
    let shared = &mut app.automations.linear_builder;
    for issue in &group.issues {
        shared.ledger.issues.insert(
            issue.id.clone(),
            Entry {
                automation: which.id().into(),
                identifier: issue.identifier.clone(),
                title: issue.title.clone(),
                project: group.project.clone(),
                branch: branch.clone(),
                session: None,
                stage: Stage::Starting,
                at: now,
            },
        );
    }
    shared.save();
    Ok(())
}

/// How far back "Related work to check" reaches for merged pull requests.
const RELATED_DAYS: i64 = 14;

/// What goes at the preset's `{task}`: the tickets in full, then the
/// project's open draft pull requests and the ones merged lately.
pub fn context(app: &App, group: &Group) -> String {
    let mut out = format!(
        "## Linear tickets\n\n{}\n\n## Related work to check\n\n",
        crate::linear::issue_sections(&group.issues)
    );
    let since = chrono::Utc::now() - chrono::Duration::days(RELATED_DAYS);
    let mut related = Vec::new();
    if let Some(prs) = app.open_prs.get(&group.project) {
        for pr in &prs.list {
            if app.prs.status_or_open(&pr.url).is_draft() || pr.answered_draft {
                related.push(format!(
                    "- #{} {} (open draft) {}",
                    pr.number, pr.title, pr.url
                ));
            }
        }
        for pr in &prs.merged {
            let merged = chrono::DateTime::parse_from_rfc3339(&pr.meta.merged_at);
            if merged.is_ok_and(|at| at >= since) {
                let day = pr.meta.merged_at.get(..10).unwrap_or_default();
                related.push(format!(
                    "- #{} {} (merged {day}) {}",
                    pr.number, pr.title, pr.url
                ));
            }
        }
    }
    if related.is_empty() {
        out.push_str("Nothing open as a draft or merged lately.");
    } else {
        out.push_str(&related.join("\n"));
    }
    out
}

/// Bring the ledger up to date with the sessions: a starting ticket's
/// session found by its branch, a plan up for approval, an approved plan
/// building; a ticket whose session never showed let go, and the oldest
/// forgotten.
fn follow(app: &mut App) {
    let now = crate::app::now_ms();
    let tree = &app.tree;
    let reasons = &app.needs_you.reasons;
    let shared = &mut app.automations.linear_builder;
    let mut changed = false;
    shared.ledger.issues.retain(|_, entry| {
        if now - entry.at > KEEP_MS {
            changed = true;
            return false;
        }
        match (&entry.session, entry.stage) {
            (None, _) => {
                let found = tree.agents.iter().find(|a| {
                    a.automation.as_deref() == Some(entry.automation.as_str())
                        && tree.worktrees.iter().any(|w| {
                            w.id == a.worktree_id
                                && w.project_id == entry.project
                                && w.branch == entry.branch
                        })
                });
                match found {
                    Some(agent) => {
                        entry.session = Some(agent.id.clone());
                        entry.stage = Stage::Planning;
                        changed = true;
                    }
                    None if now - entry.at > START_LIMIT_MS => {
                        changed = true;
                        return false;
                    }
                    None => {}
                }
            }
            (Some(id), stage) => {
                let Some(agent) = tree.agents.iter().find(|a| &a.id == id) else {
                    return true;
                };
                let plan_up = agent.status == AgentStatus::NeedsFeedback
                    && matches!(reasons.get(id), Some(orion_core::WaitReason::Plan { .. }));
                let next = match stage {
                    _ if plan_up => Stage::Approving,
                    Stage::Approving if agent.status == AgentStatus::Running => Stage::Building,
                    stage => stage,
                };
                if next != stage {
                    entry.stage = next;
                    changed = true;
                }
            }
        }
        true
    });
    if changed {
        shared.save();
    }
}

// ---- routing and grouping ----

/// A project a ticket may go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub project: ProjectId,
    pub name: String,
    /// Its **What it is** line; empty while none is written.
    pub about: String,
}

/// The model and effort routing and grouping are asked at.
const PLACE_MODEL: &str = "sonnet";
const PLACE_EFFORT: &str = "low";
const PLACE_TIMEOUT: Duration = Duration::from_secs(120);

const ROUTE_SYSTEM: &str = "You route Linear tickets to the software project each belongs to. \
You are given the tickets and the projects, each with one line saying what it is. Pick the one \
project each ticket's work would be done in. Reply with a JSON object mapping each ticket's \
identifier to the exact name of its project.";

const GROUP_SYSTEM: &str = "You group Linear tickets that one engineer should fix together in \
one branch. Join tickets only where they clearly share a cause or a component; when in doubt, \
keep a ticket on its own. Reply with a JSON array of groups, each an array of ticket \
identifiers; every ticket appears in exactly one group.";

/// One ticket as routing and grouping read it.
fn ticket_line(issue: &LinearIssue) -> String {
    let desc = crate::ui::truncate(issue.description.trim(), 1200);
    format!("### {}: {}\n{desc}", issue.identifier, issue.title)
}

/// What routing asks.
pub fn route_prompt(tickets: &[LinearIssue], candidates: &[Candidate]) -> String {
    let mut out = String::from("Projects:\n");
    for c in candidates {
        let about = match c.about.trim() {
            "" => "(no description yet)",
            about => about,
        };
        out.push_str(&format!("- {}: {about}\n", c.name));
    }
    out.push_str("\nTickets:\n\n");
    let lines: Vec<String> = tickets.iter().map(ticket_line).collect();
    out.push_str(&lines.join("\n\n"));
    out
}

/// What grouping asks.
pub fn group_prompt(tickets: &[LinearIssue]) -> String {
    let lines: Vec<String> = tickets.iter().map(ticket_line).collect();
    format!("Tickets:\n\n{}", lines.join("\n\n"))
}

/// Each ticket with the project it goes to: the one candidate when there
/// is one, else the project the model named. A ticket the model put
/// nowhere it could — or every ticket, when the ask failed — comes back
/// in the second list, to be looked at again.
pub fn routed(
    tickets: Vec<LinearIssue>,
    candidates: &[Candidate],
    reply: Option<Result<HashMap<String, String>, String>>,
) -> (Vec<(ProjectId, LinearIssue)>, Vec<LinearIssue>) {
    if let [only] = candidates {
        return (
            tickets
                .into_iter()
                .map(|t| (only.project.clone(), t))
                .collect(),
            Vec::new(),
        );
    }
    let reply = match reply {
        Some(Ok(reply)) => reply,
        Some(Err(why)) => {
            tracing::warn!("automations: routing Linear tickets: {why}");
            HashMap::new()
        }
        None => HashMap::new(),
    };
    let mut placed = Vec::new();
    let mut left = Vec::new();
    for ticket in tickets {
        let named = reply.get(&ticket.identifier).and_then(|name| {
            candidates
                .iter()
                .find(|c| c.name.trim().eq_ignore_ascii_case(name.trim()))
        });
        match named {
            Some(c) => placed.push((c.project.clone(), ticket)),
            None => left.push(ticket),
        }
    }
    (placed, left)
}

/// `tickets` in the groups the model made of them: every ticket in one
/// group, the ones it left out or doubled on their own. A failed ask is
/// one ticket a group.
pub fn grouped(
    tickets: Vec<LinearIssue>,
    reply: Result<Vec<Vec<String>>, String>,
) -> Vec<Vec<LinearIssue>> {
    let reply = reply.unwrap_or_else(|why| {
        tracing::warn!("automations: grouping Linear tickets: {why}");
        Vec::new()
    });
    let mut left: Vec<Option<LinearIssue>> = tickets.into_iter().map(Some).collect();
    let mut take = |identifier: &str| {
        left.iter_mut()
            .find(|t| {
                t.as_ref()
                    .is_some_and(|t| t.identifier == identifier.trim())
            })
            .and_then(Option::take)
    };
    let mut groups: Vec<Vec<LinearIssue>> = reply
        .iter()
        .map(|ids| ids.iter().filter_map(|id| take(id)).collect::<Vec<_>>())
        .filter(|g| !g.is_empty())
        .collect();
    groups.extend(left.into_iter().flatten().map(|t| vec![t]));
    groups
}

/// One routing or grouping ask of the one-shot model on `account`.
async fn ask_json<T: serde::de::DeserializeOwned>(
    account: &Option<crate::oneshot::Account>,
    system: &str,
    prompt: String,
) -> Result<T, String> {
    let account = account.clone().ok_or("no Claude account is switched on")?;
    crate::oneshot::run_json(crate::oneshot::Ask {
        prompt,
        system: system.into(),
        account,
        model: PLACE_MODEL.into(),
        effort: PLACE_EFFORT.into(),
        timeout: PLACE_TIMEOUT,
    })
    .await
}

/// Route `tickets` among `candidates` and group them per project, through
/// the one-shot model on `account`.
pub async fn place(
    which: Automation,
    tickets: Vec<LinearIssue>,
    candidates: Vec<Candidate>,
    account: Option<crate::oneshot::Account>,
) -> Answer {
    let reply = match candidates.len() {
        1 => None,
        _ => Some(ask_json(&account, ROUTE_SYSTEM, route_prompt(&tickets, &candidates)).await),
    };
    let (placed, left) = routed(tickets, &candidates, reply);
    let mut by_project: BTreeMap<ProjectId, Vec<LinearIssue>> = BTreeMap::new();
    for (project, ticket) in placed {
        by_project.entry(project).or_default().push(ticket);
    }
    let mut groups = Vec::new();
    for (project, tickets) in by_project {
        let made = match tickets.len() {
            1 => vec![tickets],
            _ => {
                let reply = ask_json(&account, GROUP_SYSTEM, group_prompt(&tickets)).await;
                grouped(tickets, reply)
            }
        };
        groups.extend(made.into_iter().map(|issues| Group {
            which,
            project: project.clone(),
            issues,
        }));
    }
    Answer::Placed {
        which,
        groups,
        released: left.into_iter().map(|t| t.id).collect(),
    }
}

// ---- the panel ----

/// `ENG-12 title`, or `ENG-12, ENG-15 title` for a group: how a session's
/// tickets are named on a row.
fn tickets_label(entries: &[&Entry]) -> String {
    let ids: Vec<&str> = entries.iter().map(|e| e.identifier.as_str()).collect();
    let title = entries.first().map_or("", |e| e.title.as_str());
    format!("{} {title}", ids.join(", "))
}

/// RUNNING: each session at a group of `which`'s tickets, and each group
/// queued; DONE THIS WEEK.
pub fn panel(app: &App, which: Automation) -> Panel {
    let shared = &app.automations.linear_builder;
    // The ledger's entries by session (or by branch, while starting).
    let mut sessions: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for entry in shared.ledger.issues.values() {
        if entry.automation != which.id() {
            continue;
        }
        sessions
            .entry(entry.branch.as_str())
            .or_default()
            .push(entry);
    }
    let mut rows = Vec::new();
    let mut plans = 0;
    let mut waiting = 0;
    let mut working: Option<String> = None;
    for entries in sessions.values() {
        let first = entries[0];
        let agent = first
            .session
            .as_ref()
            .and_then(|id| app.tree.agents.iter().find(|a| &a.id == id));
        let (state, tone) = match agent {
            None if first.session.is_none() => ("starting", Tone::Working),
            None => continue,
            Some(a) if a.archived || !super::occupies(a) => continue,
            Some(a) if a.status == AgentStatus::NeedsFeedback => {
                if first.stage == Stage::Approving {
                    plans += 1;
                    ("plan to approve", Tone::Waiting)
                } else {
                    waiting += 1;
                    ("waiting on you", Tone::Waiting)
                }
            }
            Some(_) if first.stage == Stage::Building => ("building", Tone::Working),
            Some(_) => ("planning", Tone::Working),
        };
        if tone == Tone::Working && working.is_none() {
            working = Some(first.identifier.clone());
        }
        rows.push(PanelRow {
            text: tickets_label(entries),
            state: state.into(),
            tone,
            session: agent.map(|a| a.id.clone()),
        });
    }
    let running = super::running(app);
    let queued: Vec<&Group> = shared.queue.iter().filter(|g| g.which == which).collect();
    for group in &queued {
        let ids: Vec<&str> = group.issues.iter().map(|i| i.identifier.as_str()).collect();
        let title = group.issues.first().map_or("", |i| i.title.as_str());
        rows.push(PanelRow {
            text: format!("{} {title}", ids.join(", ")),
            state: format!("queued, {running} running"),
            tone: Tone::Quiet,
            session: None,
        });
    }
    let verb = match which {
        Automation::BugFixer => "fixing",
        _ => "building",
    };
    let (dot, state) = match (plans, waiting, working) {
        (1, _, _) => (Dot::Waiting, "1 plan waiting".to_string()),
        (n, _, _) if n > 1 => (Dot::Waiting, format!("{n} plans waiting")),
        (_, n, _) if n > 0 => (Dot::Waiting, format!("{n} waiting on you")),
        (_, _, Some(id)) => (Dot::Working, format!("{verb} {id}")),
        _ if !queued.is_empty() => (Dot::Idle, format!("{} queued", queued.len())),
        _ => (Dot::Idle, "watching Linear".to_string()),
    };
    Panel {
        dot,
        state,
        lists: vec![
            PanelList {
                title: "RUNNING",
                empty: "Nothing running",
                rows,
            },
            super::done_this_week(app, which),
        ],
    }
}

// ---- the labels pick ----

/// Enter on **Labels**: the workspace's issue labels, the runner's ticked,
/// in the panel's place. Space ticks, Enter keeps the ticks, Esc leaves
/// them as they were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelPick {
    pub which: Automation,
    /// The workspace's labels: None while Linear is asked.
    pub names: Option<Result<Vec<String>, String>>,
    pub ticked: Vec<String>,
    pub cursor: usize,
    /// A word about the last thing tried.
    pub notice: Option<&'static str>,
}

impl LabelPick {
    /// The rows: Linear's labels, and any ticked one Linear no longer has,
    /// each once whatever its case, in alphabetical order.
    pub fn rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = Vec::new();
        let listed = match &self.names {
            Some(Ok(names)) => names.as_slice(),
            _ => &[],
        };
        for name in listed.iter().chain(&self.ticked) {
            if !rows.iter().any(|r| r.eq_ignore_ascii_case(name)) {
                rows.push(name.clone());
            }
        }
        rows.sort_by_key(|r| r.to_lowercase());
        rows
    }

    pub fn is_ticked(&self, name: &str) -> bool {
        self.ticked.iter().any(|t| t.eq_ignore_ascii_case(name))
    }

    /// Row `at` ticked or unticked.
    pub fn toggle(&mut self, at: usize) {
        let Some(name) = self.rows().into_iter().nth(at) else {
            return;
        };
        self.cursor = at;
        self.notice = None;
        if self.is_ticked(&name) {
            self.ticked.retain(|t| !t.eq_ignore_ascii_case(&name));
        } else {
            self.ticked.push(name);
        }
    }

    /// What the pick says while it has no rows.
    pub fn status(&self) -> Option<String> {
        match &self.names {
            None => Some("Asking Linear for its labels…".into()),
            Some(Err(why)) => Some(format!("Couldn't read Linear's labels: {why}")),
            Some(Ok(_)) => None,
        }
    }
}

/// Open the pick on `which`'s labels, and ask Linear for the workspace's.
fn open_pick(app: &mut App, which: Automation) -> bool {
    let ticked = labels_of(&app.automations.settings, which);
    let keys: Vec<String> = crate::linear::keys_by_project(app)
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    let names = match (keys.is_empty(), app.automations.tx.clone()) {
        (true, _) => Some(Err("no project has a LINEAR_API_KEY".into())),
        (false, None) => Some(Err("orion isn't running its loop".into())),
        (false, Some(tx)) => {
            tokio::spawn(async move {
                let mut names: Vec<String> = Vec::new();
                let mut failed = None;
                for key in keys {
                    match crate::linear::graphql(&key, LABELS_QUERY, serde_json::json!({}))
                        .await
                        .and_then(|json| parse_labels(&json))
                    {
                        Ok(found) => names.extend(found),
                        Err(why) => failed = Some(why),
                    }
                }
                let answer = match (names.is_empty(), failed) {
                    (true, Some(why)) => Err(why),
                    _ => Ok(names),
                };
                let _ = tx.send(super::Answer::LinearBuilder(Answer::Labels(answer)));
            });
            None
        }
    };
    let Some(Overlay::Automations(view)) = &mut app.overlay else {
        return false;
    };
    view.labels = Some(LabelPick {
        which,
        names,
        ticked,
        cursor: 0,
        notice: None,
    });
    true
}

/// A key while the pick is up: ↑↓ move, Space ticks, Enter keeps the
/// ticks, Esc leaves them.
pub fn pick_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Automations(view)) = &mut app.overlay else {
        return;
    };
    let Some(pick) = &mut view.labels else {
        return;
    };
    let n = pick.rows().len();
    match key.code {
        _ if crate::nav::closes(&key) => view.labels = None,
        KeyCode::Up => pick.cursor = pick.cursor.saturating_sub(1),
        KeyCode::Down => pick.cursor = (pick.cursor + 1).min(n.saturating_sub(1)),
        KeyCode::Char(' ') => {
            let at = pick.cursor;
            pick.toggle(at);
        }
        KeyCode::Enter => {
            if pick.ticked.is_empty() {
                pick.notice = Some("Tick at least one label");
                return;
            }
            let (which, ticked) = (pick.which, pick.ticked.clone());
            view.labels = None;
            super::view::change_settings(app, |s| s.of_mut(which).labels = ticked);
        }
        _ => {}
    }
}

/// A click on the pick's row `at`: Space on it.
pub fn pick_click(app: &mut App, at: usize) {
    if let Some(Overlay::Automations(view)) = &mut app.overlay {
        if let Some(pick) = &mut view.labels {
            pick.toggle(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linear::LinearTag;
    use orion_core::{Agent, AgentKind, Worktree, WorktreeId};

    fn issue(id: &str, labels: &[&str]) -> LinearIssue {
        LinearIssue {
            id: format!("uuid-{id}"),
            identifier: id.into(),
            title: format!("Ticket {id}"),
            url: format!("https://linear.app/t/issue/{id}"),
            status: "Todo".into(),
            status_type: "unstarted".into(),
            labels: labels
                .iter()
                .map(|l| LinearTag {
                    name: (*l).into(),
                    ..LinearTag::default()
                })
                .collect(),
            ..LinearIssue::default()
        }
    }

    fn candidate(name: &str) -> Candidate {
        Candidate {
            project: ProjectId(format!("p-{name}")),
            name: name.into(),
            about: format!("{name} does things"),
        }
    }

    #[test]
    fn the_label_filter_takes_the_runners_labels_in_any_case() {
        let settings = AutomationsSettings::default();
        let bug = labels_of(&settings, Automation::BugFixer);
        assert_eq!(bug, ["Bug", "Regression", "Customer bug"]);
        let feature = labels_of(&settings, Automation::FeatureBuilder);
        let shared = Shared::default();
        let issues = vec![
            issue("ENG-1", &["bug"]),
            issue("ENG-2", &["Feature"]),
            issue("ENG-3", &["Docs"]),
            issue("ENG-4", &[]),
            LinearIssue {
                status_type: "started".into(),
                ..issue("ENG-5", &["Bug"])
            },
        ];
        let ids = |v: Vec<LinearIssue>| v.into_iter().map(|i| i.identifier).collect::<Vec<_>>();
        assert_eq!(ids(fresh(&shared, &bug, issues.clone())), ["ENG-1"]);
        assert_eq!(ids(fresh(&shared, &feature, issues)), ["ENG-2"]);

        // Configured labels replace the defaults.
        let mut settings = AutomationsSettings::default();
        settings.of_mut(Automation::BugFixer).labels = vec!["Crash".into()];
        assert_eq!(labels_of(&settings, Automation::BugFixer), ["Crash"]);
    }

    #[test]
    fn a_ticket_in_the_ledger_or_being_placed_is_skipped() {
        let mut shared = Shared::default();
        let labels = vec!["Bug".to_string()];
        shared.ledger.issues.insert(
            "uuid-ENG-1".into(),
            Entry {
                automation: "bug_fixer".into(),
                identifier: "ENG-1".into(),
                title: String::new(),
                project: ProjectId("p".into()),
                branch: "eng-1".into(),
                session: None,
                stage: Stage::Starting,
                at: 0,
            },
        );
        shared.claimed.insert("uuid-ENG-2".into());
        shared.queue.push(Group {
            which: Automation::FeatureBuilder,
            project: ProjectId("p".into()),
            issues: vec![issue("ENG-3", &["Bug"])],
        });
        let issues = ["ENG-1", "ENG-2", "ENG-3", "ENG-4"]
            .map(|id| issue(id, &["Bug"]))
            .to_vec();
        let left = fresh(&shared, &labels, issues);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].identifier, "ENG-4");

        // The ledger round-trips through its file.
        let json = serde_json::to_string(&shared.ledger).unwrap();
        let back: Ledger = serde_json::from_str(&json).unwrap();
        assert_eq!(back, shared.ledger);
    }

    #[test]
    fn grouping_falls_back_to_one_ticket_a_group() {
        let tickets = vec![
            issue("ENG-1", &[]),
            issue("ENG-2", &[]),
            issue("ENG-3", &[]),
        ];
        let ids = |groups: Vec<Vec<LinearIssue>>| {
            groups
                .into_iter()
                .map(|g| g.into_iter().map(|i| i.identifier).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(grouped(tickets.clone(), Err("timed out".into()))),
            [vec!["ENG-1"], vec!["ENG-2"], vec!["ENG-3"]]
        );
        // A good reply joins what it joins; a ticket it doubles or leaves
        // out stands alone, an unknown one is dropped.
        let reply = vec![
            vec!["ENG-3".to_string(), "ENG-1".to_string()],
            vec!["ENG-1".into(), "ENG-9".into()],
        ];
        assert_eq!(
            ids(grouped(tickets, Ok(reply))),
            [vec!["ENG-3", "ENG-1"], vec!["ENG-2"]]
        );
    }

    #[test]
    fn routing_needs_no_ask_for_one_project_and_reads_the_reply_for_several() {
        let tickets = vec![issue("ENG-1", &[]), issue("ENG-2", &[])];
        let one = [candidate("web")];
        let (placed, left) = routed(tickets.clone(), &one, None);
        assert!(left.is_empty());
        assert!(placed.iter().all(|(p, _)| p == &one[0].project));

        let several = [candidate("web"), candidate("api")];
        let reply = HashMap::from([
            ("ENG-1".to_string(), "API".to_string()),
            ("ENG-2".to_string(), "mobile".to_string()),
        ]);
        let (placed, left) = routed(tickets.clone(), &several, Some(Ok(reply)));
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].0, several[1].project);
        assert_eq!(placed[0].1.identifier, "ENG-1");
        assert_eq!(
            left[0].identifier, "ENG-2",
            "a project nobody has is let go"
        );

        let (placed, left) = routed(tickets, &several, Some(Err("no account".into())));
        assert!(placed.is_empty());
        assert_eq!(left.len(), 2, "a failed ask lets every ticket go");

        let prompt = route_prompt(&[issue("ENG-1", &[])], &several);
        assert!(prompt.contains("- web: web does things"), "{prompt}");
        assert!(prompt.contains("### ENG-1: Ticket ENG-1"), "{prompt}");
    }

    fn app_with_project() -> App {
        let mut app = App::new();
        app.tree.projects.push(orion_core::Project {
            id: ProjectId("p".into()),
            name: "web".into(),
            repo_path: PathBuf::from("/nonexistent/web"),
            sort_order: 0,
        });
        app.tree.worktrees.push(Worktree {
            id: WorktreeId("root".into()),
            project_id: ProjectId("p".into()),
            path: PathBuf::from("/nonexistent/web"),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        });
        app.automations.settings.of_mut(Automation::BugFixer).on = Some(true);
        app
    }

    fn running_agent(id: &str) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId("root".into()),
            name: id.into(),
            status: AgentStatus::Running,
            archived: false,
            archived_at: 0,
            unseen: false,
            status_changed_at: 0,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: Some("pr_fixer".into()),
        }
    }

    #[test]
    fn a_group_waits_at_the_cap_and_starts_in_plan_mode_when_a_place_frees() {
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("presets.json"), || {
            crate::config::with_config_path(dir.path().join("config.json"), || {
                let mut app = app_with_project();
                app.automations.settings.max_running = 1;
                app.tree.agents.push(running_agent("busy"));
                app.automations.linear_builder.queue.push(Group {
                    which: Automation::BugFixer,
                    project: ProjectId("p".into()),
                    issues: vec![issue("ENG-7", &["Bug"])],
                });
                let mut out = Vec::new();
                launch_queued(&mut app, Automation::BugFixer, &mut out);
                assert!(out.is_empty(), "nothing starts past the cap");
                let p = panel(&app, Automation::BugFixer);
                assert_eq!(p.lists[0].rows[0].state, "queued, 1 running");
                assert_eq!(p.state, "1 queued");
                assert!(picked_up(&app, "uuid-ENG-7"));

                // A place frees: the group goes, in plan mode, into a fresh
                // worktree named after the ticket, tagged and counted.
                app.tree.agents.clear();
                launch_queued(&mut app, Automation::BugFixer, &mut out);
                assert!(app.automations.linear_builder.queue.is_empty());
                let create = out
                    .iter()
                    .find_map(|r| match r {
                        ClientRequest::CreateWorktree { branch, launch, .. } => {
                            Some((branch.clone(), launch.clone()))
                        }
                        _ => None,
                    })
                    .expect("a worktree is cut");
                assert!(create.0.contains("eng-7"), "{}", create.0);
                assert_eq!(create.1.unwrap().mode, AgentMode::Plan);
                assert_eq!(super::super::running(&app), 1, "the checkout holds a place");
                let entry = &app.automations.linear_builder.ledger.issues["uuid-ENG-7"];
                assert_eq!(entry.stage, Stage::Starting);
                assert_eq!(entry.branch, create.0);
                assert!(app.overlay.is_none(), "no box comes up");
                let p = panel(&app, Automation::BugFixer);
                assert_eq!(p.lists[0].rows[0].state, "starting");
            })
        })
    }

    #[test]
    fn the_session_is_found_by_its_branch_and_its_plan_waits_on_you() {
        let mut app = app_with_project();
        app.automations.linear_builder.ledger.issues.insert(
            "uuid-ENG-7".into(),
            Entry {
                automation: "bug_fixer".into(),
                identifier: "ENG-7".into(),
                title: "Crash".into(),
                project: ProjectId("p".into()),
                branch: "eng-7-crash".into(),
                session: None,
                stage: Stage::Starting,
                at: crate::app::now_ms(),
            },
        );
        app.tree.worktrees.push(Worktree {
            id: WorktreeId("w7".into()),
            project_id: ProjectId("p".into()),
            path: PathBuf::from("/nonexistent/w7"),
            branch: "eng-7-crash".into(),
            is_main: false,
            sort_order: 1,
        });
        let mut agent = running_agent("s7");
        agent.worktree_id = WorktreeId("w7".into());
        agent.automation = Some("bug_fixer".into());
        app.tree.agents.push(agent);
        follow(&mut app);
        let p = panel(&app, Automation::BugFixer);
        assert_eq!(p.lists[0].rows[0].state, "planning");
        assert_eq!(p.state, "fixing ENG-7");

        app.tree.agents[0].status = AgentStatus::NeedsFeedback;
        app.needs_you.reasons.insert(
            AgentId("s7".into()),
            orion_core::WaitReason::Plan {
                plan: "Fix it".into(),
            },
        );
        follow(&mut app);
        let p = panel(&app, Automation::BugFixer);
        assert_eq!(p.state, "1 plan waiting");
        assert_eq!(p.dot, Dot::Waiting);
        assert_eq!(p.lists[0].rows[0].text, "ENG-7 Crash");
        assert_eq!(p.lists[0].rows[0].session, Some(AgentId("s7".into())));

        app.tree.agents[0].status = AgentStatus::Running;
        app.needs_you.reasons.clear();
        follow(&mut app);
        assert_eq!(
            panel(&app, Automation::BugFixer).lists[0].rows[0].state,
            "building"
        );
    }

    #[test]
    fn the_preset_text_names_every_pull_request_section() {
        for text in [BUG_FIXER_TEXT, FEATURE_BUILDER_TEXT] {
            for heading in PR_SECTIONS {
                assert!(text.contains(&format!("## {heading}\n")), "{heading}");
            }
            assert!(text.contains("{task}"));
            assert!(text.contains("docs/decisions/"));
            assert!(text.contains("Never run the full e2e suite"));
            assert!(text.contains("draft"));
        }
    }

    #[test]
    fn the_queries_ask_linear_as_its_schema_has_it() {
        let (query, vars) = issues_query(&["Bug".into(), " Regression ".into()]);
        assert!(query.contains("mine: issues(first: 50"), "{query}");
        assert!(
            query.contains("assignee: { isMe: { eq: true } }"),
            "{query}"
        );
        assert!(
            query.contains(r#"state: { type: { in: ["triage", "backlog", "unstarted"] } }"#),
            "{query}"
        );
        assert!(
            query.contains("labels: { some: { or: $labels } }"),
            "{query}"
        );
        assert_eq!(
            vars["labels"][1],
            serde_json::json!({ "name": { "eqIgnoreCase": "Regression" } })
        );
        let json = serde_json::json!({ "data": { "issueLabels": { "nodes": [
            { "name": "Bug", "isGroup": false },
            { "name": "Area", "isGroup": true },
            { "name": " Feature ", "isGroup": false }
        ] } } });
        assert_eq!(parse_labels(&json).unwrap(), ["Bug", "Feature"]);
    }

    #[test]
    fn the_labels_pick_ticks_and_keeps_at_least_one() {
        let mut pick = LabelPick {
            which: Automation::BugFixer,
            names: Some(Ok(vec!["feature".into(), "Bug".into()])),
            ticked: vec!["bug".into(), "Gone".into()],
            cursor: 0,
            notice: None,
        };
        // Linear's spelling wins; a ticked label it no longer has stays.
        assert_eq!(pick.rows(), ["Bug", "feature", "Gone"]);
        pick.toggle(1);
        assert!(pick.is_ticked("Feature"));
        pick.toggle(0);
        assert!(!pick.is_ticked("Bug"));
        assert_eq!(pick.ticked, ["Gone", "feature"]);
    }

    /// Enter on **Labels** opens the pick; Space and a click on a row both
    /// tick it (INPUT PARITY); Enter saves, never with nothing ticked.
    #[test]
    fn the_labels_row_opens_the_pick_and_saves_what_is_ticked() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let press = |app: &mut App, code: KeyCode| {
            super::super::view::handle_key(
                app,
                KeyEvent::new(code, KeyModifiers::NONE),
                &mut Vec::new(),
            )
        };
        let pick = |app: &App| match &app.overlay {
            Some(Overlay::Automations(v)) => v.labels.clone(),
            _ => None,
        };
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("presets.json"), || {
            crate::config::with_config_path(dir.path().join("config.json"), || {
                let mut app = App::new();
                super::super::view::open(&mut app);
                // The bug fixer, into its panel, down to Labels.
                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Enter);
                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Enter);
                let opened = pick(&app).expect("the pick is up");
                assert_eq!(opened.which, Automation::BugFixer);
                assert!(opened.status().unwrap().contains("LINEAR_API_KEY"));
                // Linear's labels land.
                land(
                    &mut app,
                    Answer::Labels(Ok(vec!["Bug".into(), "Crash".into()])),
                    &mut Vec::new(),
                );
                // Rows: Bug, Crash, Customer bug, Regression. Space on Crash.
                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Char(' '));
                let by_key = pick(&app).unwrap().ticked;
                pick_click(&mut app, 1);
                assert!(!pick(&app).unwrap().is_ticked("Crash"));
                // A click on the row is Space on it.
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
                terminal
                    .draw(|f| {
                        let Some(Overlay::Automations(v)) = app.overlay.clone() else {
                            unreachable!()
                        };
                        super::super::view::draw(f, &mut app, &v, crate::theme::Theme::default());
                    })
                    .unwrap();
                let screen: String = {
                    let buf = terminal.backend().buffer();
                    (0..buf.area.height)
                        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
                        .map(|(x, y)| buf[(x, y)].symbol().to_string())
                        .collect()
                };
                assert!(screen.contains("[ ] Crash"), "{screen}");
                let at = (0..40u16)
                    .flat_map(|y| (0..137u16).map(move |x| (x, y)))
                    .find(|(x, y)| {
                        let buf = terminal.backend().buffer();
                        buf[(*x, *y)].symbol() == "C"
                            && buf[(*x + 1, *y)].symbol() == "r"
                            && buf[(*x + 2, *y)].symbol() == "a"
                    })
                    .unwrap();
                let click = MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: at.0,
                    row: at.1,
                    modifiers: KeyModifiers::NONE,
                };
                super::super::view::handle_mouse(
                    &mut app,
                    click,
                    ratatui::layout::Position::new(at.0, at.1),
                    &mut Vec::new(),
                );
                assert_eq!(pick(&app).unwrap().ticked, by_key, "a click is Space");

                // Enter keeps the ticks.
                press(&mut app, KeyCode::Enter);
                assert!(pick(&app).is_none());
                let saved = crate::config::Config::load().automations;
                assert_eq!(
                    saved.of(Automation::BugFixer).labels,
                    ["Bug", "Regression", "Customer bug", "Crash"]
                );

                // Nothing ticked is refused; Esc leaves the labels be.
                press(&mut app, KeyCode::Enter);
                // With no Linear key the rows are the ticked labels alone,
                // so each untick takes its row with it.
                while !pick(&app).unwrap().ticked.is_empty() {
                    pick_click(&mut app, 0);
                }
                press(&mut app, KeyCode::Enter);
                assert_eq!(pick(&app).unwrap().notice, Some("Tick at least one label"));
                press(&mut app, KeyCode::Esc);
                assert!(pick(&app).is_none());
                assert_eq!(
                    app.automations
                        .settings
                        .of(Automation::BugFixer)
                        .labels
                        .len(),
                    4
                );
            })
        })
    }

    #[test]
    fn needs_you_leaves_off_an_assignment_an_automation_picked_up() {
        use crate::needs_you::linear::Note;
        use crate::needs_you::model::CardSource;
        let mut app = app_with_project();
        let note = |id: &str, kind: &str| Note {
            id: format!("n-{id}-{kind}"),
            kind: kind.into(),
            at: chrono::Utc::now(),
            issue_id: format!("uuid-{id}"),
            identifier: id.into(),
            title: String::new(),
        };
        app.needs_you.linear.insert(
            ProjectId("p".into()),
            vec![
                note("ENG-1", "issueAssignedToYou"),
                note("ENG-2", "issueAssignedToYou"),
                note("ENG-3", "issueMention"),
            ],
        );
        app.automations
            .linear_builder
            .claimed
            .insert("uuid-ENG-1".into());
        app.automations
            .linear_builder
            .claimed
            .insert("uuid-ENG-3".into());
        let cards = crate::needs_you::linear::Inbox.cards(&app);
        let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles.len(), 2, "{titles:?}");
        assert!(titles.iter().all(|t| !t.starts_with("ENG-1")), "{titles:?}");
        assert!(
            titles.iter().any(|t| t.starts_with("ENG-3")),
            "a mention still shows"
        );
    }

    /// The real model, once, on two made-up tickets and two made-up
    /// projects: `cargo test -p orion-tui linear_builder::tests::
    /// routes_and_groups_for_real -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn routes_and_groups_for_real() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let tickets = vec![
            LinearIssue {
                description: "Tapping Save on the profile screen in the iOS app crashes when the avatar is a HEIC image.".into(),
                title: "iOS app crashes saving a HEIC avatar".into(),
                ..issue("ENG-101", &["Bug"])
            },
            LinearIssue {
                description: "The invoices API returns 500 when the customer has no billing address.".into(),
                title: "Invoices endpoint 500s without a billing address".into(),
                ..issue("ENG-102", &["Bug"])
            },
        ];
        let candidates = vec![
            Candidate {
                about:
                    "A React Native mobile app for customers to manage their profile and orders."
                        .into(),
                ..candidate("mobile")
            },
            Candidate {
                about: "The Node billing backend: invoices, payments and the REST API behind them."
                    .into(),
                ..candidate("billing-api")
            },
        ];
        let account = crate::oneshot::Account {
            program: "claude".into(),
            env: Vec::new(),
            label: "default".into(),
        };
        let answer = rt.block_on(place(
            Automation::BugFixer,
            tickets.clone(),
            candidates,
            Some(account.clone()),
        ));
        println!("routed: {answer:#?}");
        // Both on one project, to see the grouping ask's answer too.
        let grouped: Result<Vec<Vec<String>>, String> = rt.block_on(ask_json(
            &Some(account),
            GROUP_SYSTEM,
            group_prompt(&tickets),
        ));
        println!("grouped: {grouped:#?}");
        let Answer::Placed {
            groups, released, ..
        } = answer
        else {
            panic!("not placed");
        };
        assert!(released.is_empty(), "{released:?}");
        assert_eq!(groups.len(), 2);
        assert!(grouped.is_ok(), "{grouped:?}");
    }
}
