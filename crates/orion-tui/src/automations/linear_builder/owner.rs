//! The intake's FACTORY CARDS, their presses, and the Linear writes they
//! make. Every key is `issue:<Linear issue id>:<what>`:
//!
//! | `<what>` | To | Buttons (`action`) |
//! |---|---|---|
//! | `urgent` | you | Start (`start`) · Leave (`leave`) |
//! | `duplicate` | you | Start anyway (`start`) · Skip (`skip`) |
//! | `thin`, `thin-again` | the reporter | Open ticket (link) · I've updated it (`updated`) |
//! | `detail` | you | Open ticket (link) · Start anyway (`start`) |
//! | `nochange` | you | Close ticket (`close`) · Build anyway… (`build`, typed) |
//! | `stalled` | you | Continue (`continue`) · Reply… (`reply`, typed) · Park (`park`) |
//! | `dead` | you | Open ticket (link) · Park (`park`) |
//!
//! **Open ticket** is a link button: Slack (or Needs you) opens the
//! ticket, and nothing comes back here.
//!
//! A thin ticket's reporter is asked on Slack and the ticket unassigned
//! until they press **I've updated it**, which gives it back to you to be
//! checked again. When they can't be asked — not on Slack
//! ([`on_failed`]), outside the workspace, or you — the card comes to you
//! instead, and nothing is unassigned. Nothing is written to Linear's
//! comments, except the note on a ticket closed as not reproducible or
//! not to be built.

use orion_core::{AgentId, ClientRequest, FactoryCardFailure, ProjectId};

use super::stages::{self, NoChange, Verdict};
use super::{intake::Possible, Automation, Entry, Kind, Stage};
use crate::app::App;
use crate::factory::cards::{self, escape, Button, Card, Press, Recipient};
use crate::linear::{CloseAs, LinearIssue};

/// A Linear write the intake makes, sent off the loop under its
/// project's key ([`send_writes`]), in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Write {
    /// Assign the ticket to this Linear user, or to nobody.
    Assign { issue: String, user: Option<String> },
    /// Move it to its team's state for `as_`, looked up by type and name.
    Close {
        issue: String,
        team: String,
        as_: CloseAs,
    },
    /// Comment on it.
    Comment { issue: String, body: String },
    /// Mark it a duplicate of `of`.
    Duplicate { issue: String, of: String },
}

/// Send the writes waiting, each project's under its key, one after
/// another. Under test (no loop) they wait, to be read.
pub(crate) fn send_writes(app: &mut App) {
    if app.automations.tx.is_none() || app.automations.linear_builder.writes.is_empty() {
        return;
    }
    let writes = std::mem::take(&mut app.automations.linear_builder.writes);
    let mut by_key: Vec<(String, Vec<Write>)> = Vec::new();
    for (project, write) in writes {
        let key = app
            .tree
            .projects
            .iter()
            .find(|p| p.id == project)
            .and_then(|p| crate::linear::read_linear_key(&p.repo_path));
        let Some(key) = key else {
            tracing::warn!("automations: no Linear key to write {write:?} with");
            continue;
        };
        match by_key.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(write),
            None => by_key.push((key, vec![write])),
        }
    }
    for (key, writes) in by_key {
        tokio::spawn(async move {
            for write in writes {
                if let Err(why) = run(&key, &write).await {
                    tracing::warn!("automations: Linear {write:?}: {why}");
                }
            }
        });
    }
}

/// One write.
async fn run(key: &str, write: &Write) -> Result<(), String> {
    match write {
        Write::Assign { issue, user } => crate::linear::assign(key, issue, user.as_deref()).await,
        Write::Comment { issue, body } => crate::linear::comment(key, issue, body).await,
        Write::Duplicate { issue, of } => crate::linear::mark_duplicate(key, issue, of).await,
        Write::Close { issue, team, as_ } => {
            let states = crate::linear::team_states(key, team).await?;
            let state = crate::linear::state_for(&states, *as_)
                .ok_or_else(|| format!("the team has no {as_:?} state"))?;
            crate::linear::move_to(key, issue, &state.id).await
        }
    }
}

fn write(app: &mut App, project: &ProjectId, write: Write) {
    app.automations
        .linear_builder
        .writes
        .push((project.clone(), write));
}

// ---- the cards ----

/// `issue:<id>:<what>`.
pub(crate) fn key(issue_id: &str, what: &str) -> String {
    format!("issue:{issue_id}:{what}")
}

/// A key's issue id and what its card is.
pub(crate) fn parse_key(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix("issue:")?.rsplit_once(':')
}

/// `<url|RIPLO-12> Title`: the ticket, linked.
fn ticket_line(identifier: &str, title: &str, url: &str) -> String {
    match url.is_empty() {
        true => format!("*{identifier}* {}", escape(title)),
        false => format!("<{url}|{identifier}> {}", escape(title)),
    }
}

fn project_name(app: &App, project: &ProjectId) -> String {
    app.tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map_or_else(|| project.to_string(), |p| p.name.clone())
}

/// A card about `entry`'s ticket, to you unless `to` says otherwise.
fn card(
    app: &App,
    entry: &Entry,
    key: String,
    to: Recipient,
    body: Vec<String>,
    buttons: Vec<Button>,
) -> Card {
    Card {
        key,
        to,
        context: format!(
            "{} · {}",
            entry.which().name(),
            project_name(app, &entry.project)
        ),
        title: format!("{} {}", entry.identifier, entry.title),
        body: body
            .into_iter()
            .chain([ticket_line(&entry.identifier, &entry.title, &entry.url)])
            .collect(),
        buttons,
        image: None,
    }
}

/// **Open ticket**: a link to the ticket.
fn open_ticket(entry: &Entry) -> Button {
    Button::link("Open ticket", entry.url.clone())
}

/// Keep `ticket` in the ledger at `stage`, not yet started, under
/// `project`.
fn hold(
    app: &mut App,
    which: Automation,
    project: &ProjectId,
    ticket: &LinearIssue,
    stage: Stage,
) -> Entry {
    let now = crate::app::now_ms();
    let mut entry = Entry {
        automation: which.id().into(),
        identifier: ticket.identifier.clone(),
        title: ticket.title.clone(),
        project: project.clone(),
        at: now,
        kind: Kind::of(which),
        url: ticket.url.clone(),
        team_id: ticket.team_id.clone(),
        owner_id: ticket.assignee_id.clone(),
        ..Entry::default()
    };
    entry.set_stage(stage, now);
    let shared = &mut app.automations.linear_builder;
    shared
        .ledger
        .issues
        .insert(ticket.id.clone(), entry.clone());
    shared.save();
    entry
}

/// An Urgent ticket: held, and you asked whether to start it now.
pub(crate) fn hold_urgent(
    app: &mut App,
    which: Automation,
    project: &ProjectId,
    ticket: &LinearIssue,
) {
    let entry = hold(app, which, project, ticket, Stage::Held);
    let waiting = match super::super::attention(app) {
        1 => "1 thing is waiting on you".to_string(),
        n => format!("{n} things are waiting on you"),
    };
    let card = card(
        app,
        &entry,
        key(&ticket.id, "urgent"),
        Recipient::Owner,
        vec![format!("Urgent priority. Start now? {waiting}.")],
        vec![Button::tap("start", "Start"), Button::tap("leave", "Leave")],
    );
    cards::post(app, card);
}

/// A ticket very like a live session's work: held, and you asked.
pub(crate) fn hold_duplicate(app: &mut App, p: Possible) {
    let entry = hold(app, p.which, &p.project, &p.issue, Stage::Held);
    let why = match p.why.trim() {
        "" => String::new(),
        why => format!(" ({})", escape(why)),
    };
    let card = card(
        app,
        &entry,
        key(&p.issue.id, "duplicate"),
        Recipient::Owner,
        vec![format!(
            "Looks like the work in your *{}* session{why}.",
            escape(&p.session)
        )],
        vec![
            Button::tap("start", "Start anyway"),
            Button::tap("skip", "Skip"),
        ],
    );
    cards::post(app, card);
}

/// A thin ticket: its reporter asked on Slack and the ticket unassigned
/// — or, when they can't be asked, you.
pub(crate) fn thin(app: &mut App, which: Automation, project: &ProjectId, ticket: &LinearIssue) {
    let updated = app
        .automations
        .linear_builder
        .ledger
        .updated
        .contains(&ticket.id);
    let reporter_asked = !ticket.reporter_email.is_empty()
        && !ticket.reporter_id.is_empty()
        && ticket.reporter_id != ticket.assignee_id
        && !updated;
    if !reporter_asked {
        let entry = hold(app, which, project, ticket, Stage::Held);
        post_detail(app, &ticket.id, &entry);
        return;
    }
    let mut entry = hold(app, which, project, ticket, Stage::WaitingReporter);
    entry.reporter_email = ticket.reporter_email.clone();
    let first_name = ticket
        .reporter
        .split_whitespace()
        .next()
        .unwrap_or("there")
        .to_string();
    if let Some(e) = app
        .automations
        .linear_builder
        .ledger
        .issues
        .get_mut(&ticket.id)
    {
        e.reporter_email = ticket.reporter_email.clone();
    }
    app.automations.linear_builder.save();
    post_thin(app, &ticket.id, &entry, &first_name, "thin");
    write(
        app,
        project,
        Write::Assign {
            issue: ticket.id.clone(),
            user: None,
        },
    );
}

/// The card to the reporter, under `what` (`thin`, or `thin-again` for
/// the reminder).
fn post_thin(app: &mut App, issue_id: &str, entry: &Entry, first_name: &str, what: &str) {
    let card = card(
        app,
        entry,
        key(issue_id, what),
        Recipient::Email(entry.reporter_email.clone()),
        vec![format!(
            "Hi {}, an agent is ready to pick this up but needs a bit more first: what you did, what you expected instead, a screenshot. Please add it to the Linear ticket; it's unassigned until then.",
            escape(first_name)
        )],
        vec![
            open_ticket(entry),
            Button::tap("updated", "I've updated it"),
        ],
    );
    cards::post(app, card);
}

/// The card to you about a thin ticket whose reporter can't be asked.
fn post_detail(app: &mut App, issue_id: &str, entry: &Entry) {
    let card = card(
        app,
        entry,
        key(issue_id, "detail"),
        Recipient::Owner,
        vec![format!(
            "{} needs more detail and its reporter can't be asked on Slack.",
            entry.identifier
        )],
        vec![open_ticket(entry), Button::tap("start", "Start anyway")],
    );
    cards::post(app, card);
}

/// The reporter's reminder, two days on: once.
pub(crate) fn ping(app: &mut App, issue_id: &str) {
    let Some(entry) = app.automations.linear_builder.entry(issue_id) else {
        return;
    };
    // The first name went with the first card; the reminder says hello.
    post_thin(app, issue_id, &entry, "there", "thin-again");
}

/// The no-change card, for the plan `nc` says nothing should be built.
pub(crate) fn post_no_change(app: &mut App, issue_id: &str, nc: &NoChange) {
    let Some(entry) = app.automations.linear_builder.entry(issue_id) else {
        return;
    };
    let reason = escape(&nc.reason);
    let line = match nc.verdict {
        Verdict::AlreadyFixed => format!("*Already fixed* {reason}"),
        Verdict::Duplicate => format!("*Duplicate of* {reason}"),
        Verdict::NotReproducible => format!("*Can't reproduce*: {reason}"),
        Verdict::WontBuild => format!("*Won't build*: {reason}"),
    };
    let card = card(
        app,
        &entry,
        key(issue_id, "nochange"),
        Recipient::Owner,
        vec![line],
        vec![
            Button::tap("close", "Close ticket"),
            Button::text("build", "Build anyway…"),
        ],
    );
    cards::post(app, card);
}

/// The Stalled card: the session stopped with no pull request and no
/// question.
pub(crate) fn post_stalled(app: &mut App, issue_id: &str, said: Option<String>) {
    let Some(entry) = app.automations.linear_builder.entry(issue_id) else {
        return;
    };
    let mut line = format!(
        "Stopped without a PR or a question {} minutes ago.",
        stages::STALL_MS / orion_core::clock::MINUTE_MS
    );
    if let Some(said) = said.filter(|s| !s.trim().is_empty()) {
        line.push_str(&format!(
            " Last thing it said: {}",
            escape(&crate::ui::truncate(said.trim(), 600))
        ));
    }
    let card = card(
        app,
        &entry,
        key(issue_id, "stalled"),
        Recipient::Owner,
        vec![line],
        vec![
            Button::tap("continue", "Continue"),
            Button::text("reply", "Reply…"),
            Button::tap("park", "Park"),
        ],
    );
    cards::post(app, card);
}

/// The Stalled card of a session gone for good.
pub(crate) fn post_dead(app: &mut App, issue_id: &str) {
    let Some(entry) = app.automations.linear_builder.entry(issue_id) else {
        return;
    };
    let card = card(
        app,
        &entry,
        key(issue_id, "dead"),
        Recipient::Owner,
        vec![format!(
            "Its session or worktree has been gone {} minutes, with no PR.",
            stages::DEAD_MS / orion_core::clock::MINUTE_MS
        )],
        vec![open_ticket(&entry), Button::tap("park", "Park")],
    );
    cards::post(app, card);
}

// ---- the presses ----

/// A press on an `issue:` card.
pub(crate) fn on_action(app: &mut App, press: Press) {
    let Some((id, what)) = parse_key(press.key) else {
        tracing::warn!("factory: {} isn't an issue card", press.key);
        return;
    };
    let (id, key) = (id.to_string(), press.key.to_string());
    let text = press.text.clone().filter(|t| !t.trim().is_empty());
    match (what, press.action) {
        ("urgent" | "duplicate" | "detail", "start") => start(app, &id, &key),
        ("urgent", "leave") => park(app, &id, &key, "Left for you"),
        ("duplicate", "skip") => park(app, &id, &key, "Skipped"),
        ("thin" | "thin-again", "updated") => reporter_updated(app, &id),
        ("nochange", "close") => close_ticket(app, &id, &key),
        ("nochange", "build") => build_anyway(app, &id, &key, text),
        ("stalled", "continue") => {
            let turn = "Continue where you left off: finish the approved work and open the draft pull request, or ask me what you need.".to_string();
            turn_to(app, &id, &key, turn, "Sent: continue")
        }
        ("stalled", "reply") => match text {
            Some(text) => turn_to(app, &id, &key, text, "Sent"),
            None => repost(app, &key),
        },
        ("stalled" | "dead", "park") => park(app, &id, &key, "Parked"),
        _ => tracing::warn!("factory: {} on {key} does nothing", press.action),
    }
}

/// The daemon couldn't post an `issue:` card. A thin ticket's question
/// to its reporter comes to you instead, and the ticket is yours again.
pub(crate) fn on_failed(app: &mut App, key: &str, reason: &FactoryCardFailure) {
    let Some((id, what)) = parse_key(key) else {
        return;
    };
    if !matches!(what, "thin" | "thin-again") {
        return;
    }
    let id = id.to_string();
    let shared = &mut app.automations.linear_builder;
    let Some(entry) = shared.ledger.issues.get_mut(&id) else {
        return;
    };
    if entry.stage != Stage::WaitingReporter {
        return;
    }
    tracing::info!(
        "{}: its reporter can't be asked ({reason:?}); asking you",
        entry.identifier
    );
    entry.set_stage(Stage::Held, crate::app::now_ms());
    let entry = entry.clone();
    shared.save();
    if !entry.owner_id.is_empty() {
        write(
            app,
            &entry.project,
            Write::Assign {
                issue: id.clone(),
                user: Some(entry.owner_id.clone()),
            },
        );
    }
    post_detail(app, &id, &entry);
}

/// Put a card back as it was: the daemon took its buttons off with the
/// press.
fn repost(app: &mut App, key: &str) {
    let card = app
        .factory_cards
        .iter()
        .find(|p| p.card.key == key)
        .map(|p| p.card.clone());
    if let Some(card) = card {
        cards::post(app, card);
    }
}

/// Your cards on a ticket that something besides a press can end
/// (`stages::follow`, [`park`]).
const ENDED_BY_THE_TICKET: [&str; 5] = ["urgent", "duplicate", "detail", "stalled", "dead"];

/// Close every one of your cards still up on ticket `id` that its
/// ticket's end closes, with `note`.
pub(crate) fn close_cards(app: &mut App, id: &str, note: &str) {
    for what in ENDED_BY_THE_TICKET {
        cards::close_if_up(app, &key(id, what), note);
    }
}

/// **Start** / **Start anyway**: out of the ledger and cleared, so the
/// next look — now — starts it with no Urgent, thin or similarity check.
fn start(app: &mut App, id: &str, key: &str) {
    let shared = &mut app.automations.linear_builder;
    shared.ledger.issues.remove(id);
    shared.ledger.cleared.insert(id.to_string());
    shared.save();
    shared.look_now();
    cards::close_if_up(app, key, "Starting");
}

/// **Leave**, **Skip**, **Park**: parked — its session, if it has one,
/// archived — and never picked up again while it stays as it is.
fn park(app: &mut App, id: &str, key: &str, note: &str) {
    let now = crate::app::now_ms();
    let shared = &mut app.automations.linear_builder;
    let Some(entry) = shared.entry(id) else {
        cards::close_if_up(app, key, note);
        return;
    };
    let mut sessions = Vec::new();
    let group = group_of(shared, id, &entry);
    for gid in &group {
        let Some(e) = shared.ledger.issues.get_mut(gid) else {
            continue;
        };
        e.set_stage(Stage::Parked, now);
        sessions.extend(e.session.clone());
        let identifier = e.identifier.clone();
        crate::factory::runs::update(&identifier, |r| stages::push_stage(r, Stage::Parked, now));
    }
    shared.save();
    sessions.dedup();
    for session in sessions {
        archive(app, session);
    }
    cards::close_if_up(app, key, note);
    // Parked, the group's other cards are over too.
    for gid in &group {
        close_cards(app, gid, note);
    }
}

/// The ledger ids of `entry`'s group: every ticket on its branch, or the
/// one ticket when it has none yet.
fn group_of(shared: &super::Shared, id: &str, entry: &Entry) -> Vec<String> {
    if entry.branch.is_empty() {
        return vec![id.to_string()];
    }
    shared
        .ledger
        .issues
        .iter()
        .filter(|(_, e)| e.project == entry.project && e.branch == entry.branch)
        .map(|(id, _)| id.clone())
        .collect()
}

/// **I've updated it**: the ticket back to you, to be checked again on
/// the next look — and, still thin, to come to you rather than its
/// reporter.
fn reporter_updated(app: &mut App, id: &str) {
    let shared = &mut app.automations.linear_builder;
    let Some(entry) = shared.entry(id) else {
        return;
    };
    if entry.stage != Stage::WaitingReporter {
        return;
    }
    shared.ledger.issues.remove(id);
    shared.ledger.updated.insert(id.to_string());
    shared.save();
    shared.look_now();
    if !entry.owner_id.is_empty() {
        write(
            app,
            &entry.project,
            Write::Assign {
                issue: id.to_string(),
                user: Some(entry.owner_id.clone()),
            },
        );
    }
    let thanks = "Thanks: it's back with the team";
    cards::close_if_up(app, &key(id, "thin"), thanks);
    cards::close_if_up(app, &key(id, "thin-again"), thanks);
}

/// What closing a ticket with nothing to build writes to Linear, as the
/// verdict says: Done (already fixed); Duplicate, related to the ticket
/// the reason names when it names one; Canceled with the reason as a
/// comment (can't reproduce, won't build). Never back to Todo.
pub(crate) fn close_writes(issue: &str, team: &str, nc: &NoChange) -> Vec<Write> {
    let close = |as_| Write::Close {
        issue: issue.to_string(),
        team: team.to_string(),
        as_,
    };
    match nc.verdict {
        Verdict::AlreadyFixed => vec![close(CloseAs::Done)],
        Verdict::Duplicate => {
            let mut writes: Vec<Write> = stages::named_ticket(&nc.reason)
                .map(|of| Write::Duplicate {
                    issue: issue.to_string(),
                    of,
                })
                .into_iter()
                .collect();
            writes.push(close(CloseAs::Duplicate));
            writes
        }
        Verdict::NotReproducible | Verdict::WontBuild => {
            let what = match nc.verdict {
                Verdict::NotReproducible => "Can't reproduce",
                _ => "Won't build",
            };
            vec![
                Write::Comment {
                    issue: issue.to_string(),
                    body: format!("{what}: {} (closed by orion's factory)", nc.reason),
                },
                close(CloseAs::Canceled),
            ]
        }
    }
}

/// **Close ticket**: each ticket of the session's group closed as its
/// verdict says, the session archived and its worktree — no commits on
/// it — deleted; no change.
fn close_ticket(app: &mut App, id: &str, key: &str) {
    let now = crate::app::now_ms();
    let shared = &mut app.automations.linear_builder;
    let Some(entry) = shared.entry(id) else {
        return;
    };
    let Some(nc) = entry.verdict.as_deref().and_then(stages::parse_verdict) else {
        tracing::warn!("{}: no verdict to close on", entry.identifier);
        return;
    };
    let mut writes = Vec::new();
    for gid in group_of(shared, id, &entry) {
        let Some(e) = shared.ledger.issues.get_mut(&gid) else {
            continue;
        };
        writes.extend(close_writes(&gid, &e.team_id, &nc));
        e.set_stage(Stage::NoChange, now);
        let identifier = e.identifier.clone();
        crate::factory::runs::update(&identifier, |r| stages::push_stage(r, Stage::NoChange, now));
    }
    shared.save();
    for w in writes {
        write(app, &entry.project, w);
    }
    if let Some(session) = entry.session.clone() {
        archive(app, session);
    }
    let worktree = app
        .tree
        .worktrees
        .iter()
        .find(|w| !w.is_main && w.project_id == entry.project && w.branch == entry.branch)
        .map(|w| w.id.clone());
    if let Some(worktree) = worktree {
        let mut out = Vec::new();
        crate::event_loop::delete_worktrees(app, vec![worktree], &mut out);
        app.outbox.extend(out);
    }
    let note = match nc.verdict {
        Verdict::AlreadyFixed => "Closed: Done",
        Verdict::Duplicate => "Closed: Duplicate",
        _ => "Closed: Canceled",
    };
    cards::close_if_up(app, key, note);
}

/// **Build anyway…**: your words to the session as its plan's steer.
fn build_anyway(app: &mut App, id: &str, key: &str, text: Option<String>) {
    let Some(session) = session_of(app, id) else {
        return;
    };
    // The next plan's verdict, even word for word this one, asks again —
    // once the session has moved on from the plan answered here.
    if let Some(e) = app.automations.linear_builder.ledger.issues.get_mut(id) {
        e.verdict = None;
        e.steered_at = Some(crate::app::now_ms());
        app.automations.linear_builder.save();
    }
    let steer = text.unwrap_or_else(|| "Build it anyway.".into());
    let mut out = Vec::new();
    crate::event_loop::send(app, &mut out, |req_id| ClientRequest::Answer {
        req_id,
        id: session,
        answer: orion_core::Answer::Steer(steer),
    });
    app.outbox.extend(out);
    cards::close_if_up(app, key, "Sent back to plan");
}

/// A Stalled card's **Continue** or **Reply…**: `text` to the session as
/// its next turn.
fn turn_to(app: &mut App, id: &str, key: &str, text: String, note: &str) {
    let Some(session) = session_of(app, id) else {
        return;
    };
    let mut out = Vec::new();
    let sent = crate::event_loop::send_turn(app, &session, text, false, &mut out);
    app.outbox.extend(out);
    if let Some(e) = app.automations.linear_builder.ledger.issues.get_mut(id) {
        e.stalled = false;
    }
    match sent {
        true => cards::close_if_up(app, key, note),
        false => repost(app, key),
    }
}

fn session_of(app: &App, id: &str) -> Option<AgentId> {
    app.automations
        .linear_builder
        .ledger
        .issues
        .get(id)
        .and_then(|e| e.session.clone())
}

/// Archive `session`, if it is still there and not archived.
pub(crate) fn archive(app: &mut App, session: AgentId) {
    let live = app
        .tree
        .agents
        .iter()
        .any(|a| a.id == session && !a.archived);
    if !live {
        return;
    }
    let mut out = Vec::new();
    crate::event_loop::archive_agent_now(app, session, &mut out);
    app.outbox.extend(out);
}
