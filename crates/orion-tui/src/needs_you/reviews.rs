//! The board's CODE REVIEWS section: pull requests that ask you to
//! review them. Two places say so, deduped by URL:
//! * each project's open list (`App::open_prs`), whose rows carry
//!   `PrMeta::review_requested` — Enter lands on the row, as a `⌘K` jump
//!   to it does;
//! * one `gh search prs --review-requested=@me --state=open` as the board
//!   opens (THE RESOLVER's `ReviewRequests`, a minute fresh), which
//!   reaches repos that are no orion project — Enter opens those in the
//!   browser.

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
/// How many pull requests [`search`] asks for: an answer with fewer
/// holds every review asked of you.
pub const LIMIT: usize = 50;

/// A search that held every review asked of you landed: a list row the
/// modal's extras last flagged as asking for your review and the search
/// no longer has has had its review — the flag was carried from an
/// extras answer older than this one (`pull_request::carry_extras`), and
/// would keep the card and the modal's `Review requested` section until
/// the modal was next opened. True when a row changed.
pub fn settle_flags(app: &mut App, found: &[Found]) -> bool {
    if found.len() >= LIMIT {
        return false;
    }
    let mut changed = false;
    for open in app.open_prs.values_mut() {
        for pr in open.list.iter_mut().filter(|pr| pr.meta.review_requested) {
            if !found.iter().any(|f| f.url == pr.url) {
                pr.meta.review_requested = false;
                changed = true;
            }
        }
    }
    changed
}

pub fn parse(stdout: &str) -> Option<Vec<Found>> {
    serde_json::from_str(stdout).ok()
}

/// Ask GitHub for the open pull requests that ask you to review them —
/// THE RESOLVER's `ReviewRequests`, asked when the board opens and its
/// last answer is over a minute old, never on a beat of its own.
pub async fn search() -> Result<Vec<Found>, crate::sync::FetchError> {
    let out = crate::sync::gate::github(
        None,
        &[
            "search",
            "prs",
            "--review-requested=@me",
            "--state=open",
            "--limit",
            &LIMIT.to_string(),
            "--json",
            SEARCH_FIELDS,
        ],
        std::time::Duration::from_secs(20),
    )
    .await?;
    parse(&out).ok_or_else(|| {
        crate::sync::FetchError::Failed("GitHub's answer didn't read as a search".into())
    })
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
                    app.prs.label(pr.number, &pr.url, &pr.title),
                    &project.name,
                    &pr.meta.author,
                    since,
                ));
            }
        }
        for found in &app.needs_you.reviews {
            // The search answers from GitHub's index, which trails a merge
            // or close: what `prs` knows of the pull request stands.
            if app.prs.status(&found.url).is_some_and(|s| !s.is_open()) {
                continue;
            }
            if cards.iter().any(
                |c| matches!(&c.target, CardTarget::PullRequest { url, .. } if url == &found.url),
            ) {
                continue;
            }
            cards.push(card(
                None,
                &found.url,
                app.prs.label(found.number, &found.url, &found.title),
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

    fn found(n: u64) -> Found {
        Found {
            url: format!("https://github.com/o/r/pull/{n}"),
            number: n,
            title: format!("Old title {n}"),
            repository: Repository {
                name_with_owner: "o/r".into(),
            },
            author: Author {
                login: "sam".into(),
            },
            created_at: DateTime::<Utc>::default(),
        }
    }

    /// The search trails GitHub: a pull request `prs` knows has merged
    /// is no review waiting, and a known title is the newest one.
    #[test]
    fn a_search_row_behind_what_prs_knows_gives_way() {
        use crate::fetch::Asked;
        use crate::pr_store::PrObservation;
        let mut app = App::new();
        let (merged, renamed) = (found(1), found(2));
        app.prs.observe(
            &merged.url,
            PrObservation {
                state: Some(crate::pull_request::STATE_MERGED.into()),
                ..Default::default()
            },
            Asked::At(crate::fetch::now()),
        );
        app.prs.observe(
            &renamed.url,
            PrObservation {
                title: Some("New title".into()),
                ..Default::default()
            },
            Asked::At(crate::fetch::now()),
        );
        app.needs_you.reviews = vec![merged, renamed];
        let cards = Reviews.cards(&app);
        assert_eq!(cards.len(), 1, "the merged one is gone");
        assert_eq!(cards[0].title, "#2 New title");
    }

    /// A review flag the modal's extras left on a row goes once a search
    /// that holds every review asked of you no longer has it — the review
    /// was given — and stays while the search was cut short.
    #[test]
    fn a_complete_search_clears_a_carried_review_flag() {
        let mut app = App::new();
        let p = orion_core::ProjectId("p".into());
        let mut row = crate::pull_request::OpenPr {
            number: 3,
            title: "t".into(),
            url: found(3).url,
            answered_draft: false,
            answered: Default::default(),
            head: "b".into(),
            mine: false,
            head_sha: "s".into(),
            meta: Default::default(),
        };
        row.meta.review_requested = true;
        app.open_prs.insert(
            p.clone(),
            crate::app::OpenPrs {
                list: vec![row],
                merged: Vec::new(),
                at: std::time::Instant::now(),
            },
        );
        let many: Vec<Found> = (100..100 + LIMIT as u64).map(found).collect();
        assert!(!settle_flags(&mut app, &many), "cut short: no answer");
        assert!(app.open_prs[&p].list[0].meta.review_requested);
        assert!(settle_flags(&mut app, &[found(9)]));
        assert!(!app.open_prs[&p].list[0].meta.review_requested);
    }
}
