//! The intake's FACTORY CARDS, their presses, and the Linear writes they
//! make. Every key is `issue:<Linear issue id>:<what>`:
//!
//! | `<what>` | To | Buttons (`action`) |
//! |---|---|---|
//! | `urgent` | you | Start (`start`) · Leave (`leave`) |
//! | `duplicate` | you | Start anyway (`start`) · Skip (`skip`) |
//! | `thin`, `thin-again` | the reporter | Open ticket (link) · I've updated it (`updated`) |
//! | `detail` | you | Open ticket (link) · Start anyway (`start`) |
//! | `nochange` | you | Close ticket (`close`) · Build anyway… (`build`, typed) · Not for the factory |
//! | `stalled` | you | Continue (`continue`) · Reply… (`reply`, typed) · Park (`park`) · Not for the factory |
//! | `dead` | you | Open ticket (link) · Park (`park`) · Not for the factory |
//!
//! **Open ticket** is a link button: Slack (or Needs you) opens the
//! ticket, and nothing comes back here.
//!
//! **Not for the factory** ([`not_for_the_factory`]) is two choices —
//! **back to Triage** (`not_for_factory`) and **move to Backlog**
//! (`not_for_factory_backlog`) — on these cards, on the PR verdict card
//! (`pr_body`), on a factory session's plan and question on the phone (a
//! `session:<id>` press, from the daemon), in a factory session's menu in
//! orion, and as `orion factory skip` ([`super::skip`]). Each parks the
//! ticket as **Park** does — its session archived, its cards closed —
//! labels it `factory-skip` (made in its team when missing), comments
//! "Taken off the factory by <you>" (once), sets its team's Triage or
//! Backlog state, closes the draft pull request it opened with a comment
//! saying why ([`skip_writes`]), and removes the session's worktree when
//! nothing in it would be lost ([`super::skip::why_keep`]). The intake
//! never picks up a `factory-skip` ticket.
//!
//! A thin ticket's reporter is asked on Slack and the ticket unassigned
//! until they press **I've updated it**, which gives it back to you to be
//! checked again. When they can't be asked — not on Slack
//! ([`on_failed`]), outside the workspace, or you — the card comes to you
//! instead, and nothing is unassigned. Nothing is written to Linear's
//! comments, except the note on a ticket closed as not reproducible or
//! not to be built.

use orion_core::factory_card::{
    NOT_FOR_FACTORY, NOT_FOR_FACTORY_BACKLOG, NOT_FOR_FACTORY_BACKLOG_LABEL,
    NOT_FOR_FACTORY_TRIAGE_LABEL,
};
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
    /// Put the label `name` on it, made in `team` when it has none.
    Label {
        issue: String,
        team: String,
        name: String,
    },
    /// Comment "Taken off the factory by <the key's owner>", unless it
    /// says so already.
    TakenOff { issue: String },
    /// Move it to its team's Triage or Backlog, by the state's type.
    ToState {
        issue: String,
        team: String,
        to: TeamState,
    },
    /// Close the pull request at `url` with `comment`, if it is an open
    /// draft.
    CloseDraft { url: String, comment: String },
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

/// One write; `Some` says more about how it went (`already said`).
async fn run(key: &str, write: &Write) -> Result<Option<String>, String> {
    let done = |r: Result<(), String>| r.map(|()| None);
    match write {
        Write::Assign { issue, user } => {
            done(crate::linear::assign(key, issue, user.as_deref()).await)
        }
        Write::Comment { issue, body } => done(crate::linear::comment(key, issue, body).await),
        Write::Duplicate { issue, of } => done(crate::linear::mark_duplicate(key, issue, of).await),
        Write::Label { issue, team, name } => {
            done(crate::linear::add_label(key, issue, team, name).await)
        }
        Write::TakenOff { issue } => {
            let comments = crate::linear::issue_comments(key, issue).await?;
            if said_taken_off(&comments) {
                return Ok(Some("already said".into()));
            }
            let who = crate::linear::viewer_name(key)
                .await
                .ok()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| "its owner".into());
            done(crate::linear::comment(key, issue, &taken_off(&who)).await)
        }
        Write::ToState { issue, team, to } => {
            let states = crate::linear::team_states(key, team).await?;
            let state = crate::linear::state_of_kind(&states, to.kind())
                .ok_or_else(|| format!("the team has no {} state", to.name()))?;
            done(crate::linear::move_to(key, issue, &state.id).await)
        }
        Write::CloseDraft { url, comment } => close_draft(url, comment).await,
        Write::Close { issue, team, as_ } => {
            let states = crate::linear::team_states(key, team).await?;
            let state = crate::linear::state_for(&states, *as_)
                .ok_or_else(|| format!("the team has no {as_:?} state"))?;
            done(crate::linear::move_to(key, issue, &state.id).await)
        }
    }
}

/// Whether `comments` already say the ticket was taken off the factory.
pub(crate) fn said_taken_off(comments: &[String]) -> bool {
    comments
        .iter()
        .any(|c| c.trim_start().starts_with(&taken_off("")))
}

/// Where **Not for the factory** puts a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TeamState {
    Triage,
    Backlog,
}

impl TeamState {
    /// The state's type, as Linear has it.
    pub(crate) fn kind(self) -> &'static str {
        match self {
            TeamState::Triage => "triage",
            TeamState::Backlog => "backlog",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            TeamState::Triage => "Triage",
            TeamState::Backlog => "Backlog",
        }
    }
}

/// What one write does, in a line: what `orion factory skip` prints.
pub(crate) fn describe(write: &Write) -> String {
    match write {
        Write::Assign { user: Some(_), .. } => "assign the ticket".into(),
        Write::Assign { user: None, .. } => "unassign the ticket".into(),
        Write::Close { as_, .. } => format!("close the ticket ({as_:?})"),
        Write::Comment { .. } => "comment on the ticket".into(),
        Write::Duplicate { of, .. } => format!("mark the ticket a duplicate of {of}"),
        Write::Label { name, .. } => format!("label the ticket {name}"),
        Write::TakenOff { .. } => "comment \"Taken off the factory by …\"".into(),
        Write::ToState { to, .. } => format!("move the ticket to {}", to.name()),
        Write::CloseDraft { url, .. } => format!("close the draft {url}"),
    }
}

/// Run one write under the Linear key `key` — for `orion factory skip`,
/// which runs them itself, in order.
pub(crate) async fn run_write(key: &str, write: &Write) -> Result<Option<String>, String> {
    run(key, write).await
}

/// A ticket **Not for the factory** takes off: its ledger id, team and
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkipTicket {
    pub id: String,
    pub team: String,
    pub identifier: String,
}

impl SkipTicket {
    /// Ticket `id` as the ledger has it.
    pub(crate) fn of(id: &str, e: &Entry) -> Self {
        Self {
            id: id.to_string(),
            team: e.team_id.clone(),
            identifier: e.identifier.clone(),
        }
    }
}

/// The writes **Not for the factory** makes, in order: each ticket
/// labelled [`super::intake::SKIP_LABEL`], commented on (once: not again
/// on one that says so already) and moved to its team's Triage, or its
/// Backlog (`backlog`); then the draft pull request at `pr` closed with a
/// comment naming the tickets. Safe to make twice.
pub(crate) fn skip_writes(tickets: &[SkipTicket], pr: Option<&str>, backlog: bool) -> Vec<Write> {
    let mut writes = Vec::new();
    for t in tickets {
        writes.push(Write::Label {
            issue: t.id.clone(),
            team: t.team.clone(),
            name: super::intake::SKIP_LABEL.into(),
        });
        writes.push(Write::TakenOff {
            issue: t.id.clone(),
        });
        writes.push(Write::ToState {
            issue: t.id.clone(),
            team: t.team.clone(),
            to: match backlog {
                true => TeamState::Backlog,
                false => TeamState::Triage,
            },
        });
    }
    if let Some(url) = pr {
        let names: Vec<&str> = tickets.iter().map(|t| t.identifier.as_str()).collect();
        writes.push(Write::CloseDraft {
            url: url.to_string(),
            comment: format!(
                "Closed: {} was taken off orion's software factory (Not for the factory).",
                names.join(", ")
            ),
        });
    }
    writes
}

/// The comment on a ticket taken off the factory.
pub(crate) fn taken_off(who: &str) -> String {
    format!("Taken off the factory by {who}")
}

/// How long one `gh` call on the draft may take.
const GH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Close the pull request at `url` with `comment` — only while it is an
/// open draft: one marked ready, merged or closed is left as it is.
async fn close_draft(url: &str, comment: &str) -> Result<Option<String>, String> {
    use crate::pull_request::run_gh;
    let view = run_gh(
        None,
        &["pr", "view", url, "--json", "state,isDraft"],
        GH_TIMEOUT,
    )
    .await?;
    let view: serde_json::Value = serde_json::from_str(&view).map_err(|e| e.to_string())?;
    if !is_open_draft(&view) {
        return Ok(Some("not an open draft: left as it is".into()));
    }
    run_gh(
        None,
        &["pr", "close", url, "--comment", comment],
        GH_TIMEOUT,
    )
    .await
    .map(|_| None)
}

/// Whether `gh pr view --json state,isDraft` says an open draft.
pub(crate) fn is_open_draft(view: &serde_json::Value) -> bool {
    view["state"] == "OPEN" && view["isDraft"] == true
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

/// **Not for the factory**'s two choices, on every card about a ticket
/// the factory took.
pub(crate) fn not_for_the_factory_buttons() -> [Button; 2] {
    [
        Button::tap(NOT_FOR_FACTORY, NOT_FOR_FACTORY_TRIAGE_LABEL),
        Button::tap(NOT_FOR_FACTORY_BACKLOG, NOT_FOR_FACTORY_BACKLOG_LABEL),
    ]
}

/// `buttons`, then **Not for the factory**'s two.
fn and_not_for_the_factory(mut buttons: Vec<Button>) -> Vec<Button> {
    buttons.extend(not_for_the_factory_buttons());
    buttons
}

/// Whether `action` is one of **Not for the factory**'s choices, and
/// which: `Some(true)` to Backlog.
pub(crate) fn not_for_the_factory_choice(action: &str) -> Option<bool> {
    match action {
        NOT_FOR_FACTORY => Some(false),
        NOT_FOR_FACTORY_BACKLOG => Some(true),
        _ => None,
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
        and_not_for_the_factory(vec![
            Button::tap("close", "Close ticket"),
            Button::text("build", "Build anyway…"),
        ]),
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
        and_not_for_the_factory(vec![
            Button::tap("continue", "Continue"),
            Button::text("reply", "Reply…"),
            Button::tap("park", "Park"),
        ]),
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
        and_not_for_the_factory(vec![open_ticket(&entry), Button::tap("park", "Park")]),
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
        ("nochange" | "stalled" | "dead", action) if press.by_owner => {
            match not_for_the_factory_choice(action) {
                Some(backlog) => not_for_the_factory(app, &id, Some(&key), None, backlog),
                None => tracing::warn!("factory: {action} on {key} does nothing"),
            }
        }
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

/// What a card taken off the factory closes on.
const OFF_THE_FACTORY: &str = "Not for the factory";

/// **Not for the factory** on the ticket `id` (and its group): parked as
/// [`park`] parks it — its session archived, its cards closed, the
/// pressed one under `key` too — then [`skip_writes`]: each ticket
/// labelled `factory-skip` with a comment saying who took it off, moved
/// to its team's Backlog when `backlog` (else its Triage), and its draft
/// pull request (`pr`, else the ledger's) closed with a comment — and its
/// worktree removed once git says nothing in it would be lost
/// ([`remove_worktree`]).
pub(crate) fn not_for_the_factory(
    app: &mut App,
    id: &str,
    key: Option<&str>,
    pr: Option<String>,
    backlog: bool,
) {
    let Some(entry) = app.automations.linear_builder.entry(id) else {
        if let Some(key) = key {
            cards::close_if_up(app, key, OFF_THE_FACTORY);
        }
        return;
    };
    let group = group_of(&app.automations.linear_builder, id, &entry);
    let tickets: Vec<SkipTicket> = group
        .iter()
        .filter_map(|gid| {
            let e = app.automations.linear_builder.ledger.issues.get(gid)?;
            Some(SkipTicket::of(gid, e))
        })
        .collect();
    let pr = pr.or_else(|| entry.pr_url.clone());
    let pressed = key.map_or_else(|| self::key(id, "nochange"), str::to_string);
    park(app, id, &pressed, OFF_THE_FACTORY);
    let shared = &mut app.automations.linear_builder;
    for gid in &group {
        if let Some(e) = shared.ledger.issues.get_mut(gid) {
            e.skipped = true;
        }
    }
    shared.save();
    for gid in &group {
        cards::close_if_up(app, &self::key(gid, "nochange"), OFF_THE_FACTORY);
    }
    if let Some(url) = &pr {
        cards::close_if_up(app, &format!("pr:{url}:verdict"), OFF_THE_FACTORY);
    }
    for w in skip_writes(&tickets, pr.as_deref(), backlog) {
        write(app, &entry.project, w);
    }
    remove_worktree(app, &entry);
}

/// Remove the worktree `entry`'s session worked in, through orion's own
/// delete (its hooks and containers with it) — once git, off the loop,
/// says it is clean and every commit is on a remote or the base
/// ([`super::skip::why_keep`]). One kept says why in the footer.
fn remove_worktree(app: &mut App, entry: &Entry) {
    let Some(w) = app
        .tree
        .worktrees
        .iter()
        .find(|w| !w.is_main && w.project_id == entry.project && w.branch == entry.branch)
        .cloned()
    else {
        return;
    };
    let base = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == w.project_id)
        .map(|p| {
            crate::config::Config::load()
                .project(&p.repo_path)
                .base_branch
        })
        .unwrap_or_default();
    match app.automations.tx.clone() {
        Some(tx) => {
            tokio::task::spawn_blocking(move || {
                let keep = super::skip::why_keep(&w.path, &base);
                let answer = super::Answer::Worktree { id: w.id, keep };
                let _ = tx.send(crate::automations::Answer::LinearBuilder(answer));
            });
        }
        None => {
            let keep = super::skip::why_keep(&w.path, &base);
            worktree_checked(app, w.id, keep);
        }
    }
}

/// [`remove_worktree`]'s check landed: removed, or kept with why.
pub(crate) fn worktree_checked(app: &mut App, id: orion_core::WorktreeId, keep: Option<String>) {
    match keep {
        None => {
            let mut out = Vec::new();
            crate::event_loop::delete_worktrees(app, vec![id], &mut out);
            app.outbox.extend(out);
        }
        Some(why) => {
            let branch = app
                .tree
                .worktrees
                .iter()
                .find(|w| w.id == id)
                .map_or_else(String::new, |w| format!(" {}", w.branch));
            app.flash = Some(crate::flash::Flash::note(format!("Worktree{branch} {why}")));
            app.dirty = true;
        }
    }
}

/// **Not for the factory** on the factory ticket whose pull request is at
/// `url` (the verdict card's), found by the ledger's pull request or by
/// its `identifier`.
pub(crate) fn not_for_the_factory_pr(
    app: &mut App,
    url: &str,
    identifier: &str,
    key: &str,
    backlog: bool,
) {
    let issues = &app.automations.linear_builder.ledger.issues;
    let id = issues
        .iter()
        .find(|(_, e)| e.pr_url.as_deref() == Some(url))
        .or_else(|| {
            issues
                .iter()
                .find(|(_, e)| !identifier.is_empty() && e.identifier == identifier)
        })
        .map(|(id, _)| id.clone());
    match id {
        Some(id) => not_for_the_factory(app, &id, Some(key), Some(url.to_string()), backlog),
        None => cards::close_if_up(app, key, OFF_THE_FACTORY),
    }
}

/// The factory ticket session `agent` is at work on — planning, building
/// or its draft open — if it is one's.
pub(crate) fn ticket_of_session(app: &App, agent: &AgentId) -> Option<String> {
    app.automations
        .linear_builder
        .ledger
        .issues
        .iter()
        .filter(|(_, e)| e.stage.is_active() || e.stage == Stage::PrOpen)
        .find(|(_, e)| e.session.as_ref() == Some(agent))
        .map(|(id, _)| id.clone())
}

/// **Not for the factory** on session `agent` — from its menu in orion,
/// or its plan or question on the phone (`session:<id>`).
pub(crate) fn not_for_the_factory_session(app: &mut App, agent: &AgentId, backlog: bool) {
    match ticket_of_session(app, agent) {
        Some(id) => not_for_the_factory(app, &id, None, None, backlog),
        None => tracing::info!("factory: {agent} works on no factory ticket"),
    }
}

/// A press about a session rather than a card (`session:<agent id>`).
pub(crate) fn on_session_action(app: &mut App, press: Press) {
    let Some(agent) = press
        .key
        .strip_prefix(orion_core::factory_card::SESSION_PREFIX)
    else {
        return;
    };
    match not_for_the_factory_choice(press.action).filter(|_| press.by_owner) {
        Some(backlog) => not_for_the_factory_session(app, &AgentId(agent.to_string()), backlog),
        None => tracing::warn!("factory: {} on session {agent} does nothing", press.action),
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
