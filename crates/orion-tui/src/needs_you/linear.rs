//! The board's LINEAR section: one card per live ticket of yours, from
//! two sources merged by issue:
//!
//! * the tickets assigned to you in a live state
//!   ([`crate::linear::LIVE_STATES`]: triage, todo, started — never the
//!   backlog, done or cancelled), and
//! * your unread notifications about a ticket in one of those states — a
//!   mention, a new comment on an issue you're in, an issue given to you.
//!
//! Both are asked in one request through [`crate::linear::graphql`] with
//! each project's key, once per distinct key (projects that share a
//! workspace share a key), as Linear's schema has it:
//! `notifications(first, filter)` → `NotificationConnection.nodes`, each a
//! `Notification` (`id`, `type`, `readAt`, `createdAt`, `snoozedUntilAt`)
//! that is an `IssueNotification` carrying its `issue { id identifier
//! title state { type name color } }`; and, aliased `mine` so
//! [`crate::linear::parse_issues`] reads it, `issues(filter: { assignee:
//! { isMe }, state: { type: { in } } })`. `NotificationFilter` has no
//! `readAt`, so the unread are picked here, and so is dropping a
//! notification snoozed until later and one about a ticket that isn't
//! live: neither is waiting on you, though Linear keeps both unread.
//!
//! A card with unread notifications sorts first; the rest go triage, then
//! todo, then started. `⌘D` marks a card's notifications read
//! (`notificationUpdate(id, input: { readAt })`) and hides it until the
//! ticket changes (its `updatedAt` is the card's `since`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use orion_core::ProjectId;
use serde_json::Value;

use super::model::{Card, CardKey, CardSource, CardTarget, Section};
use crate::app::App;
use crate::linear::{LinearIssue, LIVE_STATES};

/// The notification types the board lists, by what each says on a card.
const KINDS: &[(&str, &str)] = &[
    ("issueMention", "mentioned you"),
    ("issueCommentMention", "mentioned you in a comment"),
    ("issueNewComment", "new comment"),
    ("issueAssignedToYou", "assigned to you"),
];

/// How many notifications one ask reads — the newest, read or not — and
/// how many of your live tickets, the most recently touched first.
const LIMIT: usize = 50;

/// `"triage", "unstarted", "started"`, for a GraphQL list.
fn live_states() -> String {
    let states: Vec<String> = LIVE_STATES.iter().map(|s| format!("\"{s}\"")).collect();
    states.join(", ")
}

/// The ask: the newest notifications of [`KINDS`] with their issue, and
/// your live tickets.
fn query() -> String {
    let kinds: Vec<String> = KINDS.iter().map(|(k, _)| format!("\"{k}\"")).collect();
    format!(
        "query {{ notifications(first: {LIMIT}, filter: {{ type: {{ in: [{}] }} }}) {{ nodes {{ \
         id type readAt createdAt snoozedUntilAt \
         ... on IssueNotification {{ issue {{ id identifier title state {{ type name color }} }} }} }} }} \
         mine: issues(first: {LIMIT}, orderBy: updatedAt, filter: {{ \
         assignee: {{ isMe: {{ eq: true }} }}, state: {{ type: {{ in: [{}] }} }} }}) \
         {{ nodes {{ {} }} }} }}",
        kinds.join(", "),
        live_states(),
        crate::linear::ISSUE_FIELDS,
    )
}

/// One unread notification about an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub kind: String,
    pub at: DateTime<Utc>,
    pub issue_id: String,
    pub identifier: String,
    pub title: String,
}

/// A ticket's workflow state, as a notification's issue carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueState {
    /// Linear's state type: `triage`, `unstarted`, `started`…
    pub kind: String,
    /// The state's own name: `In Progress`.
    pub name: String,
    /// Its colour, as Linear's hex.
    pub color: String,
}

/// The tickets one ask heard of, beside the notifications.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tickets {
    /// Assigned to you, in a live state.
    pub mine: Vec<LinearIssue>,
    /// The state of each ticket a kept notification is about, by issue id.
    pub states: HashMap<String, IssueState>,
}

/// What one ask heard under a key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Heard {
    pub notes: Vec<Note>,
    pub tickets: Tickets,
}

/// Whether a state type is live ([`LIVE_STATES`]).
fn live(kind: &str) -> bool {
    LIVE_STATES.contains(&kind)
}

/// An answer to [`query`]: the unread issue notifications that still wait
/// on you — not snoozed past `now`, about a live ticket — and your live
/// tickets. An answer with no notifications at all is Linear's error.
pub fn parse(json: &Value, now: DateTime<Utc>) -> Result<Heard, String> {
    let nodes = json
        .pointer("/data/notifications/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            crate::linear::graphql_error(json)
                .unwrap_or_else(|| "Linear sent no notifications".into())
        })?;
    let text = |v: &Value, path: &str| {
        v.pointer(path)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut heard = Heard::default();
    for n in nodes {
        let unread = n.get("readAt").is_none_or(Value::is_null);
        let awake = DateTime::parse_from_rfc3339(&text(n, "/snoozedUntilAt"))
            .map_or(true, |until| until <= now);
        let state = IssueState {
            kind: text(n, "/issue/state/type"),
            name: text(n, "/issue/state/name"),
            color: text(n, "/issue/state/color"),
        };
        let issue_id = text(n, "/issue/id");
        let Ok(at) = DateTime::parse_from_rfc3339(&text(n, "/createdAt")) else {
            continue;
        };
        if !unread || !awake || !live(&state.kind) || issue_id.is_empty() {
            continue;
        }
        heard.notes.push(Note {
            id: text(n, "/id"),
            kind: text(n, "/type"),
            at: at.with_timezone(&Utc),
            issue_id: issue_id.clone(),
            identifier: text(n, "/issue/identifier"),
            title: text(n, "/issue/title"),
        });
        heard.tickets.states.insert(issue_id, state);
    }
    heard.tickets.mine = match crate::linear::parse_issues(json) {
        Ok(issues) => issues
            .into_iter()
            .filter(|i| live(&i.status_type))
            .collect(),
        Err(why) => {
            tracing::debug!("needs you: Linear's tickets for you: {why}");
            Vec::new()
        }
    };
    Ok(heard)
}

/// What a notification type says on a card.
fn says(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("new activity", |(_, says)| says)
}

/// Ask Linear for the unread notifications and live tickets under `key`.
pub async fn fetch(key: String) -> Result<Heard, String> {
    let json = crate::linear::graphql(&key, &query(), serde_json::json!({})).await?;
    parse(&json, Utc::now())
}

/// Mark notifications `ids` read, now.
pub async fn mark_read(key: String, ids: Vec<String>) -> Result<(), String> {
    let at = Utc::now().to_rfc3339();
    for id in ids {
        let json = crate::linear::graphql(
            &key,
            "mutation($id: String!, $at: DateTime!) { \
             notificationUpdate(id: $id, input: { readAt: $at }) { success } }",
            serde_json::json!({ "id": id, "at": at }),
        )
        .await?;
        crate::linear::mutation_result(&json, "notificationUpdate", "Linear kept it unread")?;
    }
    Ok(())
}

/// The issue a card stands for: its notifications, newest first, as
/// `(project, notes)`.
pub fn notes_of<'a>(app: &'a App, key: &CardKey) -> Option<(&'a ProjectId, Vec<&'a Note>)> {
    app.needs_you.linear.iter().find_map(|(project, notes)| {
        let of: Vec<&Note> = notes
            .iter()
            .filter(|n| &card_key(project, &n.issue_id) == key)
            .collect();
        (!of.is_empty()).then_some((project, of))
    })
}

fn card_key(project: &ProjectId, issue: &str) -> CardKey {
    CardKey::of(&CardTarget::LinearIssue {
        project: project.clone(),
        id: issue.to_string(),
    })
}

/// The LINEAR section's source.
pub struct Inbox;

impl CardSource for Inbox {
    fn cards(&self, app: &App) -> Vec<Card> {
        let (notes, tickets) = (&app.needs_you.linear, &app.needs_you.linear_tickets);
        let projects: BTreeSet<&ProjectId> = notes.keys().chain(tickets.keys()).collect();
        let none = Tickets::default();
        projects
            .into_iter()
            .flat_map(|project| {
                let notes = notes.get(project).map_or(&[][..], Vec::as_slice);
                project_cards(app, project, notes, tickets.get(project).unwrap_or(&none))
            })
            .collect()
    }
}

/// One project's cards: a card per live ticket of yours, and per ticket
/// a notification is about, the two merged by issue.
fn project_cards(app: &App, project: &ProjectId, notes: &[Note], tickets: &Tickets) -> Vec<Card> {
    let place = app
        .tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map_or("", |p| p.name.as_str());
    // A ticket the bug fixer or feature builder picked up is theirs to act
    // on, not yours: its assignment and its card as one of yours.
    let picked_up = |issue: &str| crate::automations::linear_builder::picked_up(app, issue);
    // Each ticket's newest news.
    let mut newest: BTreeMap<&str, &Note> = BTreeMap::new();
    for note in notes {
        if note.kind == "issueAssignedToYou" && picked_up(&note.issue_id) {
            continue;
        }
        let kept = newest.entry(&note.issue_id).or_insert(note);
        if note.at > kept.at {
            *kept = note;
        }
    }
    let mut cards = Vec::new();
    for issue in &tickets.mine {
        if picked_up(&issue.id) || !live(&issue.status_type) {
            continue;
        }
        let note = newest.remove(issue.id.as_str());
        // Hidden by ⌘D until the ticket changes, or new news comes.
        let updated = DateTime::parse_from_rfc3339(&issue.updated_at)
            .ok()
            .map(|at| at.with_timezone(&Utc));
        let since = note.map(|n| n.at).max(updated).unwrap_or_default();
        let state = IssueState {
            kind: issue.status_type.clone(),
            name: issue.status.clone(),
            color: issue.state_color.clone(),
        };
        let title = format!("{} {}", issue.identifier, issue.title);
        cards.push(card(
            app,
            project,
            place,
            &issue.id,
            title,
            Some(&state),
            note,
            since,
        ));
    }
    for note in newest.into_values() {
        let title = format!("{} {}", note.identifier, note.title);
        let state = tickets.states.get(&note.issue_id);
        cards.push(card(
            app,
            project,
            place,
            &note.issue_id,
            title,
            state,
            Some(note),
            note.at,
        ));
    }
    cards
}

/// A ticket's card: `ENG-12 Fix login` over `web · In Progress ·
/// mentioned you`, its state's glyph, and its place in the column — the
/// notified first, then triage, todo, started ([`LIVE_STATES`]' order).
#[allow(clippy::too_many_arguments)]
fn card(
    app: &App,
    project: &ProjectId,
    place: &str,
    issue: &str,
    title: String,
    state: Option<&IssueState>,
    note: Option<&Note>,
    since: DateTime<Utc>,
) -> Card {
    let sub: Vec<&str> = [
        place,
        state.map_or("", |s| s.name.as_str()),
        note.map_or("", |n| says(&n.kind)),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect();
    let rank = state
        .and_then(|s| LIVE_STATES.iter().position(|live| *live == s.kind))
        .map_or(0, |at| (LIVE_STATES.len() - 1 - at) as u8);
    let urgency = if note.is_some() {
        LIVE_STATES.len() as u8
    } else {
        rank
    };
    let card = Card::new(
        Section::Linear,
        CardTarget::LinearIssue {
            project: project.clone(),
            id: issue.to_string(),
        },
        title,
        sub.join(" · "),
        since,
        urgency,
    );
    match state {
        Some(s) => card.with_mark(crate::linear::state_glyph(&s.kind, &s.color, app.theme)),
        None => card,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, state: &str, snoozed: Value) -> Value {
        serde_json::json!({ "id": format!("n-{id}"), "type": "issueNewComment", "readAt": null,
          "createdAt": "2026-11-07T10:00:00.000Z", "snoozedUntilAt": snoozed,
          "issue": { "id": id, "identifier": id, "title": "t",
                     "state": { "type": state, "name": state, "color": "#f2c94c" } } })
    }

    fn noon() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-07T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn ids(heard: &Heard) -> Vec<&str> {
        heard.notes.iter().map(|n| n.identifier.as_str()).collect()
    }

    #[test]
    fn the_unread_issue_notifications_are_read_and_the_rest_left() {
        let json = serde_json::json!({ "data": { "notifications": { "nodes": [
            { "id": "n1", "type": "issueMention", "readAt": null,
              "createdAt": "2026-11-07T10:00:00.000Z",
              "issue": { "id": "i1", "identifier": "ENG-12", "title": "Fix login",
                         "state": { "type": "started", "name": "In Progress" } } },
            { "id": "n2", "type": "issueNewComment", "readAt": "2026-11-07T11:00:00.000Z",
              "createdAt": "2026-11-07T10:30:00.000Z",
              "issue": { "id": "i2", "identifier": "ENG-13", "title": "Read",
                         "state": { "type": "started" } } },
            { "id": "n3", "type": "projectUpdateCreated", "readAt": null,
              "createdAt": "2026-11-07T10:30:00.000Z" }
        ] } } });
        let heard = parse(&json, Utc::now()).unwrap();
        assert_eq!(ids(&heard), ["ENG-12"]);
        assert_eq!(says(&heard.notes[0].kind), "mentioned you");
        assert_eq!(heard.tickets.states["i1"].name, "In Progress");
        assert!(heard.tickets.mine.is_empty(), "no `mine` in the answer");
    }

    #[test]
    fn an_answer_without_notifications_is_linears_error() {
        let json = serde_json::json!({ "errors": [{ "message": "Authentication required" }] });
        assert_eq!(
            parse(&json, Utc::now()).unwrap_err(),
            "Authentication required"
        );
    }

    #[test]
    fn a_ticket_that_isnt_live_and_a_snoozed_note_wait_on_nobody() {
        let json = serde_json::json!({ "data": { "notifications": { "nodes": [
            note("open", "started", Value::Null),
            note("triaged", "triage", Value::Null),
            note("todo", "unstarted", Value::Null),
            note("later", "backlog", Value::Null),
            note("done", "completed", Value::Null),
            note("dropped", "canceled", Value::Null),
            note("snoozed", "unstarted", "2026-11-08T09:00:00.000Z".into()),
            note("woken", "started", "2026-11-07T09:00:00.000Z".into()),
        ] } } });
        let heard = parse(&json, noon()).unwrap();
        assert_eq!(ids(&heard), ["open", "triaged", "todo", "woken"]);
    }

    #[test]
    fn your_live_tickets_come_in_the_same_answer() {
        let issue = |id: &str, kind: &str| {
            serde_json::json!({ "id": id, "identifier": id, "title": "t", "url": "https://linear.app/x",
              "updatedAt": "2026-11-07T10:00:00.000Z",
              "state": { "name": kind, "type": kind, "color": "#f2c94c", "position": 1 } })
        };
        let json = serde_json::json!({ "data": {
            "notifications": { "nodes": [] },
            "mine": { "nodes": [issue("a", "started"), issue("b", "backlog"),
                                issue("c", "completed"), issue("d", "triage")] }
        } });
        let heard = parse(&json, noon()).unwrap();
        let mine: Vec<&str> = heard.tickets.mine.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(mine, ["d", "a"], "triage before started; the rest left out");
    }

    #[test]
    fn the_query_asks_for_the_notes_and_your_live_tickets_at_once() {
        let q = query();
        assert!(q.contains("notifications(first: 50"), "{q}");
        assert!(q.contains("\"issueAssignedToYou\""), "{q}");
        assert!(
            q.contains(
                "... on IssueNotification { issue { id identifier title state { type name color } } }"
            ),
            "{q}"
        );
        assert!(
            q.contains(
                "mine: issues(first: 50, orderBy: updatedAt, filter: { \
                 assignee: { isMe: { eq: true } }, \
                 state: { type: { in: [\"triage\", \"unstarted\", \"started\"] } } })"
            ),
            "{q}"
        );
        assert!(q.contains(crate::linear::ISSUE_FIELDS), "{q}");
    }

    fn ticket(id: &str, kind: &str, status: &str) -> LinearIssue {
        LinearIssue {
            id: format!("uuid-{id}"),
            identifier: id.into(),
            title: "Fix login".into(),
            status: status.into(),
            status_type: kind.into(),
            updated_at: "2026-11-07T09:00:00.000Z".into(),
            mine: true,
            ..LinearIssue::default()
        }
    }

    fn mention(id: &str) -> Note {
        Note {
            id: format!("n-{id}"),
            kind: "issueMention".into(),
            at: noon(),
            issue_id: format!("uuid-{id}"),
            identifier: id.into(),
            title: "Fix login".into(),
        }
    }

    fn board(mine: Vec<LinearIssue>, notes: Vec<Note>) -> App {
        let mut app = App::new();
        let project = ProjectId("p".into());
        app.needs_you.linear.insert(project.clone(), notes);
        app.needs_you.linear_tickets.insert(
            project,
            Tickets {
                mine,
                states: HashMap::new(),
            },
        );
        app
    }

    fn titles(app: &App) -> Vec<String> {
        super::super::cards(app)
            .into_iter()
            .map(|c| c.title)
            .collect()
    }

    #[test]
    fn an_assigned_live_ticket_without_a_notification_gets_a_card() {
        let app = board(
            vec![
                ticket("ENG-1", "started", "In Progress"),
                ticket("ENG-2", "unstarted", "Todo"),
                ticket("ENG-3", "triage", "Triage"),
            ],
            Vec::new(),
        );
        assert_eq!(
            titles(&app),
            ["ENG-3 Fix login", "ENG-2 Fix login", "ENG-1 Fix login"],
            "triage, then todo, then in progress"
        );
        let cards = Inbox.cards(&app);
        let started = cards.iter().find(|c| c.title.starts_with("ENG-1")).unwrap();
        assert_eq!(started.sub, "In Progress");
        assert_eq!(started.mark.map(|(glyph, _)| glyph), Some("◑"));
    }

    #[test]
    fn a_backlog_done_or_cancelled_ticket_gets_no_card() {
        let app = board(
            vec![
                ticket("ENG-1", "backlog", "Backlog"),
                ticket("ENG-2", "completed", "Done"),
                ticket("ENG-3", "canceled", "Canceled"),
                ticket("ENG-4", "canceled", "Duplicate"),
            ],
            Vec::new(),
        );
        assert!(titles(&app).is_empty());
    }

    #[test]
    fn an_assigned_ticket_that_mentions_you_is_one_card_first() {
        let app = board(
            vec![
                ticket("ENG-1", "triage", "Triage"),
                ticket("ENG-2", "started", "In Progress"),
            ],
            vec![mention("ENG-2")],
        );
        let cards = super::super::cards(&app);
        let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["ENG-2 Fix login", "ENG-1 Fix login"]);
        assert_eq!(cards[0].sub, "In Progress · mentioned you");
        assert_eq!(
            cards[0].since,
            noon(),
            "the newer of the note and the ticket"
        );
    }

    #[test]
    fn a_ticket_an_automation_picked_up_gets_no_card() {
        let mut app = board(
            vec![
                ticket("ENG-1", "unstarted", "Todo"),
                ticket("ENG-2", "unstarted", "Todo"),
            ],
            Vec::new(),
        );
        app.automations
            .linear_builder
            .queue
            .push(crate::automations::linear_builder::Group {
                which: crate::automations::Automation::BugFixer,
                project: ProjectId("p".into()),
                issues: vec![ticket("ENG-1", "unstarted", "Todo")],
            });
        assert_eq!(titles(&app), ["ENG-2 Fix login"]);
    }

    #[test]
    fn done_on_a_ticket_hides_it_until_the_ticket_changes() {
        let mut app = board(vec![ticket("ENG-1", "started", "In Progress")], Vec::new());
        let card = Inbox.cards(&app).remove(0);
        app.needs_you.store.done(&card);
        assert!(titles(&app).is_empty());
        let tickets = app.needs_you.linear_tickets.values_mut().next().unwrap();
        tickets.mine[0].updated_at = "2026-11-07T11:00:00.000Z".into();
        assert_eq!(titles(&app), ["ENG-1 Fix login"]);
    }
}
