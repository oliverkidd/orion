//! The board's CODE REVIEWS section: pull requests that ask you to
//! review them. Two places say so, deduped by URL:
//! * each project's open list (`App::open_prs`), whose rows carry
//!   `PrMeta::review_requested` — Enter lands on the row, as a `⌘K` jump
//!   to it does;
//! * one `gh search prs --review-requested=@me --state=open` on the
//!   board's poll, which reaches repos that are no orion project — Enter
//!   opens those in the browser.

use chrono::{DateTime, Utc};
use orion_core::ProjectId;
use serde::Deserialize;

use super::model::{Card, CardSource, CardTarget, Section};
use crate::app::App;

/// The `gh search prs` fields read, as `gh` names them.
pub const SEARCH_FIELDS: &str = "url,number,title,repository,author,createdAt";

/// One row of `gh search prs --json SEARCH_FIELDS`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub url: String,
    pub number: u64,
    pub title: String,
    pub repository: Repository,
    pub author: Author,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub name_with_owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Author {
    pub login: String,
}

/// `gh search prs`'s answer; anything else is no answer.
pub fn parse(stdout: &str) -> Option<Vec<Found>> {
    serde_json::from_str(stdout).ok()
}

/// Ask GitHub for the open pull requests that ask you to review them.
pub async fn search() -> Option<Vec<Found>> {
    let out = crate::pull_request::gh(
        None,
        &[
            "search",
            "prs",
            "--review-requested=@me",
            "--state=open",
            "--limit",
            "50",
            "--json",
            SEARCH_FIELDS,
        ],
        std::time::Duration::from_secs(20),
    )
    .await?;
    parse(&out)
}

fn card(
    project: Option<ProjectId>,
    url: &str,
    label: String,
    place: &str,
    author: &str,
    since: DateTime<Utc>,
) -> Card {
    let sub = if author.is_empty() {
        place.to_string()
    } else {
        format!("{place} · {author}")
    };
    Card::new(
        Section::CodeReviews,
        CardTarget::PullRequest {
            project,
            url: url.to_string(),
        },
        label,
        sub,
        since,
        0,
    )
}

/// The CODE REVIEWS section's source.
pub struct Reviews;

impl CardSource for Reviews {
    fn cards(&self, app: &App) -> Vec<Card> {
        let mut cards: Vec<Card> = Vec::new();
        for project in &app.tree.projects {
            let Some(open) = app.open_prs.get(&project.id) else {
                continue;
            };
            for pr in open
                .list
                .iter()
                .filter(|pr| pr.meta.review_requested && !pr.is_settled())
            {
                let since = DateTime::parse_from_rfc3339(&pr.meta.created_at)
                    .map(|at| at.with_timezone(&Utc))
                    .unwrap_or_default();
                cards.push(card(
                    Some(project.id.clone()),
                    &pr.url,
                    pr.label(),
                    &project.name,
                    &pr.meta.author,
                    since,
                ));
            }
        }
        for found in &app.needs_you.reviews {
            if cards.iter().any(
                |c| matches!(&c.target, CardTarget::PullRequest { url, .. } if url == &found.url),
            ) {
                continue;
            }
            cards.push(card(
                None,
                &found.url,
                crate::pull_request::numbered_label(found.number, &found.title),
                &found.repository.name_with_owner,
                &found.author.login,
                found.created_at,
            ));
        }
        cards
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `gh search prs --json` really answers in (checked
    /// against gh 2.x on 2026-11-08).
    #[test]
    fn a_search_answer_parses() {
        let out = r#"[{"author":{"id":"U_1","is_bot":false,"login":"sam","type":"User","url":"https://github.com/sam"},
            "createdAt":"2026-11-06T18:53:10Z","number":2019,
            "repository":{"name":"riplo-os","nameWithOwner":"RiploData/riplo-os"},
            "title":"Review slides","url":"https://github.com/RiploData/riplo-os/pull/2019"}]"#;
        let found = parse(out).unwrap();
        assert_eq!(found[0].repository.name_with_owner, "RiploData/riplo-os");
        assert_eq!(found[0].author.login, "sam");
        assert_eq!(parse("not json"), None);
    }
}
