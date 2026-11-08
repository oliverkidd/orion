//! The board's LINEAR section: your unread Linear notifications — a
//! mention, a new comment on an issue you're in, an issue given to you —
//! one card per issue.
//!
//! Asked through [`crate::linear::graphql`] with each project's key, once
//! per distinct key (projects that share a workspace share a key), as
//! Linear's schema has it: `notifications(first, filter)` →
//! `NotificationConnection.nodes`, each a `Notification` (`id`, `type`,
//! `readAt`, `createdAt`) that is an `IssueNotification` carrying its
//! `issue { id identifier title }`. `NotificationFilter` has no `readAt`,
//! so the unread are picked here. `⌘D` marks a card's notifications read
//! (`notificationUpdate(id, input: { readAt })`).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use orion_core::ProjectId;
use serde_json::Value;

use super::model::{Card, CardKey, CardSource, CardTarget, Section};
use crate::app::App;

/// The notification types the board lists, by what each says on a card.
const KINDS: &[(&str, &str)] = &[
    ("issueMention", "mentioned you"),
    ("issueCommentMention", "mentioned you in a comment"),
    ("issueNewComment", "new comment"),
    ("issueAssignedToYou", "assigned to you"),
];

/// How many notifications one ask reads — the newest, read or not.
const LIMIT: usize = 50;

/// The ask: the newest notifications of [`KINDS`], with their issue.
fn query() -> String {
    let kinds: Vec<String> = KINDS.iter().map(|(k, _)| format!("\"{k}\"")).collect();
    format!(
        "query {{ notifications(first: {LIMIT}, filter: {{ type: {{ in: [{}] }} }}) {{ nodes {{ \
         id type readAt createdAt \
         ... on IssueNotification {{ issue {{ id identifier title }} }} }} }} }}",
        kinds.join(", ")
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

/// The unread issue notifications in an answer to [`query`]; an answer
/// with no notifications at all is Linear's error.
pub fn parse(json: &Value) -> Result<Vec<Note>, String> {
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
    Ok(nodes
        .iter()
        .filter(|n| n.get("readAt").is_none_or(Value::is_null))
        .filter_map(|n| {
            let issue_id = text(n, "/issue/id");
            let at = DateTime::parse_from_rfc3339(&text(n, "/createdAt")).ok()?;
            (!issue_id.is_empty()).then(|| Note {
                id: text(n, "/id"),
                kind: text(n, "/type"),
                at: at.with_timezone(&Utc),
                issue_id,
                identifier: text(n, "/issue/identifier"),
                title: text(n, "/issue/title"),
            })
        })
        .collect())
}

/// What a notification type says on a card.
fn says(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("new activity", |(_, says)| says)
}

/// Ask Linear for the unread notifications under `key`.
pub async fn fetch(key: String) -> Result<Vec<Note>, String> {
    let json = crate::linear::graphql(&key, &query(), serde_json::json!({})).await?;
    parse(&json)
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
        let mut cards = Vec::new();
        for (project, notes) in &app.needs_you.linear {
            let name = app
                .tree
                .projects
                .iter()
                .find(|p| &p.id == project)
                .map_or("", |p| p.name.as_str());
            // One card per issue, saying its newest news.
            let mut by_issue: BTreeMap<&str, &Note> = BTreeMap::new();
            for note in notes {
                let newest = by_issue.entry(&note.issue_id).or_insert(note);
                if note.at > newest.at {
                    *newest = note;
                }
            }
            for note in by_issue.into_values() {
                cards.push(Card::new(
                    Section::Linear,
                    CardTarget::LinearIssue {
                        project: project.clone(),
                        id: note.issue_id.clone(),
                    },
                    format!("{} {}", note.identifier, note.title),
                    format!("{name} · {}", says(&note.kind)),
                    note.at,
                    0,
                ));
            }
        }
        cards
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unread_issue_notifications_are_read_and_the_rest_left() {
        let json = serde_json::json!({ "data": { "notifications": { "nodes": [
            { "id": "n1", "type": "issueMention", "readAt": null,
              "createdAt": "2026-11-07T10:00:00.000Z",
              "issue": { "id": "i1", "identifier": "ENG-12", "title": "Fix login" } },
            { "id": "n2", "type": "issueNewComment", "readAt": "2026-11-07T11:00:00.000Z",
              "createdAt": "2026-11-07T10:30:00.000Z",
              "issue": { "id": "i2", "identifier": "ENG-13", "title": "Read" } },
            { "id": "n3", "type": "projectUpdateCreated", "readAt": null,
              "createdAt": "2026-11-07T10:30:00.000Z" }
        ] } } });
        let notes = parse(&json).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].identifier, "ENG-12");
        assert_eq!(says(&notes[0].kind), "mentioned you");
    }

    #[test]
    fn an_answer_without_notifications_is_linears_error() {
        let json = serde_json::json!({ "errors": [{ "message": "Authentication required" }] });
        assert_eq!(parse(&json).unwrap_err(), "Authentication required");
    }

    #[test]
    fn the_query_asks_for_the_listed_kinds_and_their_issue() {
        let q = query();
        assert!(q.contains("notifications(first: 50"), "{q}");
        assert!(q.contains("\"issueAssignedToYou\""), "{q}");
        assert!(
            q.contains("... on IssueNotification { issue { id identifier title } }"),
            "{q}"
        );
    }
}
