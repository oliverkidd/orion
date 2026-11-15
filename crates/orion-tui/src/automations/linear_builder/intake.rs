//! INTAKE: what a fresh ticket is checked for before it is claimed, and
//! what a claimed one is given.
//!
//! - **Worked on already** ([`worked_on`]), in this order: linked to an
//!   orion worktree, or a worktree's branch names it; an open pull
//!   request's title, body or branch names it; a live session's title or
//!   branch names it. Skipped, and said once in the log. The ticket's own
//!   earlier run that ended (parked, no change, dead) counts for nothing:
//!   its kept worktree is no one at work, and the ticket starts afresh on
//!   a new branch.
//! - **Urgent** (priority 1) and **thin** ([`is_thin`]: a description
//!   under [`THIN_WORDS`] words with nothing attached and no image) are
//!   held on a card ([`super::owner`]) unless you cleared the ticket.
//! - **Very like a live session's work** ([`similar`]): the cheap model
//!   compares it with every live session's title and first prompt; a
//!   match is held on a card too.
//! - A bug's group gets its team's **related tickets** ([`related`]), and
//!   every group its tickets' **attachments**, downloaded from Linear
//!   ([`download`]).
//! - Its prompt's **related work** is the project's open drafts and
//!   lately merged pull requests that share its words ([`related_prs`]).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use orion_core::{AgentId, ProjectId};
use serde::Deserialize;

use super::{Automation, Related};
use crate::app::App;
use crate::linear::{names_issue, LinearIssue};

/// A description shorter than this many words, with nothing attached, is
/// thin.
pub(crate) const THIN_WORDS: usize = 20;

/// What the check says about a fresh ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Check {
    /// Claim it.
    Go,
    /// Someone is on it already: where.
    WorkedOn(String),
    /// Ask you first: it is Urgent.
    Urgent,
    /// Ask its reporter (or you) for more first.
    Thin,
}

/// The check, in the plan's order: worked on (never cleared away), then
/// — unless you said to start it — Urgent, then thin.
pub(crate) fn check(app: &App, projects: &[ProjectId], ticket: &LinearIssue) -> Check {
    if let Some(place) = worked_on(app, projects, ticket) {
        return Check::WorkedOn(place);
    }
    if app
        .automations
        .linear_builder
        .ledger
        .cleared
        .contains(&ticket.id)
    {
        return Check::Go;
    }
    if ticket.priority == 1 {
        return Check::Urgent;
    }
    if is_thin(ticket) {
        return Check::Thin;
    }
    Check::Go
}

/// Where `ticket` is being worked on already, if it is: an orion
/// worktree linked to it or whose branch names it (`linear::work_of`),
/// an open pull request whose title, body or branch names it, or a live
/// session whose title or branch names it.
pub(crate) fn worked_on(app: &App, projects: &[ProjectId], ticket: &LinearIssue) -> Option<String> {
    let id = ticket.identifier.as_str();
    let mine = |project: &ProjectId| projects.contains(project);
    // Its own earlier runs, ended, are no one at work on it.
    let ended = app
        .automations
        .linear_builder
        .ledger
        .previous
        .get(&ticket.id);
    let ended = |branch: &str| ended.is_some_and(|b| b.contains(branch));
    // 1. A link, or a worktree on a branch that names it.
    if let Some(branch) = app.linear_links.branch_of(&ticket.id) {
        let checkout = app
            .tree
            .worktrees
            .iter()
            .any(|w| !w.is_main && w.branch == branch && mine(&w.project_id));
        if checkout && !ended(branch) {
            return Some(format!("the worktree on {branch}"));
        }
    }
    for project in projects {
        let work = crate::linear::work_of(app, project, ticket);
        if let Some(branch) = work.and_then(|w| w.branch).filter(|b| !ended(b)) {
            return Some(format!("the worktree on {branch}"));
        }
    }
    // 2. An open pull request.
    for project in projects {
        let Some(prs) = app.open_prs.get(project) else {
            continue;
        };
        for pr in prs.list.iter().filter(|pr| !ended(&pr.head)) {
            let body = app.pr_detail.get(&pr.url).map_or("", |d| d.body.as_str());
            if names_issue(&pr.title, id) || names_issue(&pr.head, id) || names_issue(body, id) {
                return Some(format!("PR #{}", pr.number));
            }
        }
    }
    // 3. A live session.
    for agent in app.tree.agents.iter().filter(|a| !a.archived) {
        let branch = app
            .tree
            .worktrees
            .iter()
            .find(|w| w.id == agent.worktree_id && !w.is_main)
            .map(|w| w.branch.as_str())
            .unwrap_or_default();
        if ended(branch) {
            continue;
        }
        if names_issue(&agent.name, id) || names_issue(branch, id) {
            return Some(format!("the {} session", agent.name));
        }
    }
    None
}

/// How long after a ticket was let go a look may take it back.
const SETTLE_MS: i64 = 5 * orion_core::clock::MINUTE_MS;

/// Tickets back in your hands in Triage or Todo, let go of to be checked
/// again: a thin ticket you gave back to yourself (still thin, it comes
/// to you, not its reporter), and a started one that was parked, closed
/// with no change or died and is back in Todo — its branch kept in
/// `Ledger::previous`, so its old worktree doesn't hold it back. One held
/// on a card, left or skipped stays.
pub(crate) fn returned(app: &mut App, issues: &[LinearIssue]) {
    let now = crate::app::now_ms();
    let shared = &mut app.automations.linear_builder;
    let mut let_go = Vec::new();
    for issue in issues {
        let Some(entry) = shared.ledger.issues.get(&issue.id) else {
            continue;
        };
        let since = entry.since();
        let heard = crate::pull_request::rfc3339_ms(&issue.updated_at).unwrap_or(0);
        let back = match entry.stage {
            super::Stage::WaitingReporter => now - since >= SETTLE_MS,
            super::Stage::Parked | super::Stage::Dead | super::Stage::NoChange => {
                let started = entry.history.iter().any(|(s, _)| s.is_active());
                started && heard > since && now - since >= SETTLE_MS
            }
            _ => false,
        };
        if back {
            let_go.push((issue.id.clone(), entry.stage));
        }
    }
    if let_go.is_empty() {
        return;
    }
    for (id, stage) in let_go {
        let Some(entry) = shared.ledger.issues.remove(&id) else {
            continue;
        };
        if stage == super::Stage::WaitingReporter {
            shared.ledger.updated.insert(id);
        } else if !entry.branch.is_empty() {
            shared.ledger.add_previous(id, entry.branch, now);
        }
    }
    shared.save();
}

/// Whether `ticket` is too thin to start: under [`THIN_WORDS`] words of
/// description, no attachment, and no image in the description.
pub(crate) fn is_thin(ticket: &LinearIssue) -> bool {
    let words = ticket.description.split_whitespace().count();
    words < THIN_WORDS && ticket.files.is_empty() && !has_image(&ticket.description)
}

fn has_image(markdown: &str) -> bool {
    markdown.contains("![") || markdown.contains(LINEAR_UPLOADS)
}

// ---- similarity ----

/// A live session, as the similarity check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveSession {
    pub id: AgentId,
    pub name: String,
    pub first_prompt: String,
}

/// The most sessions one similarity ask reads.
const SESSIONS_READ: usize = 40;

/// Every live session — not archived — the most recently busy first.
pub(crate) fn live_sessions(app: &App) -> Vec<LiveSession> {
    let mut agents: Vec<&orion_core::Agent> =
        app.tree.agents.iter().filter(|a| !a.archived).collect();
    agents.sort_by_key(|a| std::cmp::Reverse(a.status_changed_at));
    agents
        .into_iter()
        .take(SESSIONS_READ)
        .map(|a| LiveSession {
            id: a.id.clone(),
            name: a.name.clone(),
            first_prompt: a
                .recent_prompts
                .first()
                .map(|p| crate::ui::truncate(&p.text, 600))
                .unwrap_or_default(),
        })
        .collect()
}

/// What `place` needs besides the tickets: the Linear key they came in
/// under, the live sessions, the tickets you cleared, where attachments
/// go, and the project a held ticket is kept under.
#[derive(Debug, Clone)]
pub struct Extras {
    pub(crate) key: Option<String>,
    pub(crate) sessions: Vec<LiveSession>,
    pub(crate) cleared: BTreeSet<String>,
    pub(crate) attachments: PathBuf,
    pub(crate) home: ProjectId,
}

/// A ticket the model judged very like a live session's work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Possible {
    pub(crate) which: Automation,
    pub(crate) project: ProjectId,
    pub(crate) issue: LinearIssue,
    pub(crate) session: String,
    pub(crate) agent: AgentId,
    pub(crate) why: String,
}

const SIMILAR_SYSTEM: &str = "You compare new Linear tickets with the coding sessions already \
running. For each ticket, name a session only when it is very likely already doing the same \
work: the same bug or the same feature, not merely the same area. Reply with a JSON object \
mapping each ticket's identifier to {\"session\": the session's exact name or null, \"why\": \
one short clause}.";

/// What the similarity check asks.
pub(crate) fn similar_prompt(tickets: &[LinearIssue], sessions: &[LiveSession]) -> String {
    let mut out = String::from("Sessions:\n");
    for s in sessions {
        let prompt = match s.first_prompt.trim() {
            "" => "(no prompt seen)",
            p => p,
        };
        out.push_str(&format!("- {}: {prompt}\n", s.name));
    }
    out.push_str("\nTickets:\n\n");
    let lines: Vec<String> = tickets.iter().map(super::ticket_line).collect();
    out.push_str(&lines.join("\n\n"));
    out
}

/// One ticket's answer.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Match {
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub why: String,
}

/// `tickets` split by the model's `reply`: the ones to go on, and the
/// ones it matched to a live session by name. A ticket you cleared, a
/// name no session has, or a failed ask, goes on.
pub(crate) fn judged(
    which: Automation,
    tickets: Vec<LinearIssue>,
    extras: &Extras,
    reply: Result<HashMap<String, Match>, String>,
) -> (Vec<LinearIssue>, Vec<Possible>) {
    let reply = reply.unwrap_or_else(|why| {
        tracing::warn!("automations: comparing tickets with live sessions: {why}");
        HashMap::new()
    });
    let mut keep = Vec::new();
    let mut possible = Vec::new();
    for ticket in tickets {
        let named = reply
            .get(&ticket.identifier)
            .filter(|_| !extras.cleared.contains(&ticket.id))
            .and_then(|m| {
                let name = m.session.as_deref()?.trim();
                let s = extras
                    .sessions
                    .iter()
                    .find(|s| s.name.trim().eq_ignore_ascii_case(name))?;
                Some((s, m.why.trim().to_string()))
            });
        match named {
            Some((s, why)) => possible.push(Possible {
                which,
                project: extras.home.clone(),
                issue: ticket,
                session: s.name.clone(),
                agent: s.id.clone(),
                why,
            }),
            None => keep.push(ticket),
        }
    }
    (keep, possible)
}

/// The similarity check: asked only for the tickets you haven't cleared,
/// and only when there is a live session to compare with.
pub(crate) async fn similar(
    which: Automation,
    account: &Option<crate::oneshot::Account>,
    tickets: Vec<LinearIssue>,
    extras: &Extras,
) -> (Vec<LinearIssue>, Vec<Possible>) {
    let asked: Vec<LinearIssue> = tickets
        .iter()
        .filter(|t| !extras.cleared.contains(&t.id))
        .cloned()
        .collect();
    if asked.is_empty() || extras.sessions.is_empty() {
        return (tickets, Vec::new());
    }
    let reply = super::ask_json(
        account,
        SIMILAR_SYSTEM,
        similar_prompt(&asked, &extras.sessions),
    )
    .await;
    judged(which, tickets, extras, reply)
}

// ---- related tickets ----

/// How many of the team's tickets one ask reads, and how many go in the
/// prompt.
const RELATED_READ: usize = 100;
const RELATED_KEPT: usize = 8;

/// The team's tickets a bug may share a cause with: open, or completed
/// or canceled in the last 90 days — aliased `mine`, so
/// `linear::parse_issues` reads them.
pub(crate) fn related_query(team: &str) -> (String, serde_json::Value) {
    let query = format!(
        "query($team: ID!) {{ mine: issues(first: {RELATED_READ}, orderBy: updatedAt, filter: {{ \
         team: {{ id: {{ eq: $team }} }}, \
         or: [{{ state: {{ type: {{ nin: [\"completed\", \"canceled\"] }} }} }}, \
         {{ completedAt: {{ gt: \"-P90D\" }} }}, {{ canceledAt: {{ gt: \"-P90D\" }} }}] }}) \
         {{ nodes {{ id identifier title url description state {{ name type }} }} }} }}"
    );
    (query, serde_json::json!({ "team": team }))
}

/// Words worth matching on: three letters or more, not the commonest.
fn words(text: &str) -> BTreeSet<String> {
    const COMMON: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "when", "from", "are", "was", "not", "but",
        "have", "has", "into", "after", "before", "should", "would", "can", "cant", "does",
        "doesn", "isn", "its", "it's", "you", "your", "our", "they", "them", "there", "then",
        "what", "which", "will", "just", "also", "out", "on", "all", "any", "one",
    ];
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !COMMON.contains(&w.as_str()))
        .collect()
}

/// `candidates` ranked by how many words they share with `bug` (its
/// tickets' titles and descriptions), the bug's own tickets and those
/// sharing nothing left out: the top [`RELATED_KEPT`], each with a
/// two-line summary.
pub(crate) fn rank(bug: &[LinearIssue], candidates: Vec<LinearIssue>) -> Vec<Related> {
    let target: BTreeSet<String> = bug
        .iter()
        .flat_map(|i| words(&format!("{} {}", i.title, i.description)))
        .collect();
    let mut scored: Vec<(usize, LinearIssue)> = candidates
        .into_iter()
        .filter(|c| !bug.iter().any(|b| b.id == c.id))
        .map(|c| {
            let theirs = words(&format!("{} {}", c.title, c.description));
            (theirs.intersection(&target).count(), c)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.identifier.cmp(&b.1.identifier))
    });
    scored
        .into_iter()
        .take(RELATED_KEPT)
        .map(|(_, c)| Related {
            summary: c
                .description
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(2)
                .map(|l| crate::ui::truncate(l, 160))
                .collect::<Vec<_>>()
                .join("\n"),
            identifier: c.identifier,
            title: c.title,
            status: c.status,
        })
        .collect()
}

// ---- related pull requests ----

/// How many pull requests "Related work to check" lists at most.
pub(crate) const RELATED_PRS_KEPT: usize = 6;
/// The least share of a pull request's weighted words the tickets must
/// share for it to count as related.
const RELATED_PR_SHARE: f64 = 0.25;
/// The least number of words a related pull request shares with them:
/// one word in common is a coincidence.
const RELATED_PR_WORDS: usize = 2;

/// The words a pull request's title or branch, or a ticket, is matched
/// on: [`words`] less numbers and the words every pull request uses.
fn pr_words(text: &str) -> BTreeSet<String> {
    const BOILERPLATE: &[&str] = &[
        "fix", "fixes", "fixed", "add", "adds", "added", "update", "updates", "updated", "bump",
        "bumps", "chore", "feat", "refactor", "wip", "draft", "make", "makes", "use", "uses",
        "now", "new", "more", "less", "don", "won",
    ];
    words(text)
        .into_iter()
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()) && !BOILERPLATE.contains(&w.as_str()))
        .collect()
}

/// Which of `prs` — each a `(title, branch)` — look related to `tickets`
/// (their titles and descriptions), best first, at most
/// [`RELATED_PRS_KEPT`], as indices into `prs`.
///
/// Each word is weighted by how rare it is among `prs` (its inverse
/// document frequency), so the words every branch carries — an author's
/// prefix, the team's key — count for little. A pull request is related
/// when it shares at least [`RELATED_PR_WORDS`] words with the tickets
/// and they carry at least [`RELATED_PR_SHARE`] of its own words' weight.
pub(crate) fn related_prs(tickets: &[LinearIssue], prs: &[(&str, &str)]) -> Vec<usize> {
    let target: BTreeSet<String> = tickets
        .iter()
        .flat_map(|i| pr_words(&format!("{} {}", i.title, i.description)))
        .collect();
    let theirs: Vec<BTreeSet<String>> = prs
        .iter()
        .map(|(title, branch)| pr_words(&format!("{title} {branch}")))
        .collect();
    let mut df: HashMap<&str, usize> = HashMap::new();
    for word in theirs.iter().flatten() {
        *df.entry(word.as_str()).or_default() += 1;
    }
    let n = prs.len() as f64;
    let idf = |w: &str| (1.0 + n / df.get(w).copied().unwrap_or(1) as f64).ln();
    let mut scored: Vec<(f64, f64, usize)> = theirs
        .iter()
        .enumerate()
        .filter_map(|(i, words)| {
            let total: f64 = words.iter().map(|w| idf(w)).sum();
            let shared: Vec<&String> = words.intersection(&target).collect();
            let weight: f64 = shared.iter().map(|w| idf(w)).sum();
            let share = if total > 0.0 { weight / total } else { 0.0 };
            (shared.len() >= RELATED_PR_WORDS && share >= RELATED_PR_SHARE)
                .then_some((share, weight, i))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.1.total_cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .take(RELATED_PRS_KEPT)
        .map(|(_, _, i)| i)
        .collect()
}

/// The related tickets of the bug `issues`, from their team.
pub(crate) async fn related(key: &str, issues: &[LinearIssue]) -> Vec<Related> {
    let Some(team) = issues
        .iter()
        .map(|i| i.team_id.as_str())
        .find(|t| !t.is_empty())
    else {
        return Vec::new();
    };
    let (query, variables) = related_query(team);
    match crate::linear::graphql(key, &query, variables)
        .await
        .and_then(|json| crate::linear::parse_issues(&json))
    {
        Ok(candidates) => rank(issues, candidates),
        Err(why) => {
            tracing::warn!("automations: reading related tickets: {why}");
            Vec::new()
        }
    }
}

// ---- attachments ----

/// Where Linear keeps what is uploaded to it. Only this host is sent the
/// Linear key: any other attachment is a link, never downloaded with it.
const LINEAR_UPLOADS: &str = "uploads.linear.app";

/// The images and files `issue` carries that orion downloads: every
/// image in its description (`![…](url)`), every link to Linear's
/// uploads in it, and its attachments that aren't pull requests — each
/// one on Linear's own upload host, once.
pub(crate) fn attachment_urls(issue: &LinearIssue) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for (at, _) in issue.description.match_indices("](") {
        let rest = &issue.description[at + 2..];
        let url = rest
            .split(|c: char| c == ')' || c.is_whitespace())
            .next()
            .unwrap_or_default();
        let image = issue.description[..at]
            .rfind('[')
            .is_some_and(|open| open > 0 && issue.description[..open].ends_with('!'));
        if url.starts_with("https://") && (image || url.contains(LINEAR_UPLOADS)) {
            urls.push(url.to_string());
        }
    }
    urls.extend(issue.files.iter().cloned());
    let mut seen = BTreeSet::new();
    urls.retain(|u| on_linear(u) && seen.insert(u.clone()));
    urls
}

fn on_linear(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| host == LINEAR_UPLOADS)
}

/// The file an attachment is saved as: its URL's last segment, made safe,
/// after its place in the list.
pub(crate) fn file_name(url: &str, n: usize) -> String {
    let last = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .unwrap_or_default();
    let safe: String = last
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                true => c,
                false => '_',
            },
        )
        .collect();
    match safe.trim_matches('.').is_empty() {
        true => format!("{n}-attachment"),
        false => format!("{n}-{safe}"),
    }
}

/// Where `identifier`'s attachments go under `root`.
pub(crate) fn dir_of(root: &Path, identifier: &str) -> PathBuf {
    let safe: String = identifier
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() || c == '-' {
            true => c,
            false => '_',
        })
        .collect();
    root.join(safe)
}

/// Download each of `issues`' attachments ([`attachment_urls`]) with the
/// Linear key into `<root>/<identifier>/`: the absolute paths of the ones
/// saved, a file there already kept as it is.
pub(crate) async fn download(key: &str, issues: &[LinearIssue], root: &Path) -> Vec<PathBuf> {
    let mut saved = Vec::new();
    for issue in issues {
        let dir = dir_of(root, &issue.identifier);
        for (n, url) in attachment_urls(issue).into_iter().enumerate() {
            let path = dir.join(file_name(&url, n + 1));
            if path.is_file() {
                saved.push(path);
                continue;
            }
            let headers = vec![format!("Authorization: {key}")];
            let fetched = tokio::task::spawn_blocking(move || {
                orion_core::http::request(&orion_core::http::Request {
                    method: "GET",
                    url: &url,
                    headers: &headers,
                    timeout_secs: 60,
                    ..Default::default()
                })
            })
            .await;
            let body = match fetched {
                Ok(Ok(response)) if response.ok() => response.body,
                Ok(Ok(response)) => {
                    tracing::warn!(
                        "attachment of {}: HTTP {}",
                        issue.identifier,
                        response.status
                    );
                    continue;
                }
                Ok(Err(why)) => {
                    tracing::warn!("attachment of {}: {why}", issue.identifier);
                    continue;
                }
                Err(why) => {
                    tracing::warn!("attachment of {}: {why}", issue.identifier);
                    continue;
                }
            };
            let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, body));
            match written {
                Ok(()) => saved.push(path),
                Err(why) => tracing::warn!("attachment of {}: {why}", issue.identifier),
            }
        }
    }
    saved
}
