//! The NEEDS YOU board's cards: one per thing waiting on you, offered by
//! a [`CardSource`] — the agents, Linear, code reviews, Slack, meetings —
//! and laid out in three [`Column`]s of [`Section`]s.
//!
//! **Done** and **Snooze** are kept in `<data dir>/needs-you.json`
//! ([`Store`]), by [`CardKey`]:
//! * Done hides a card until its source changes it. A source that
//!   changes a card — a session waiting again, a new comment — gives it a
//!   later `since`, and a card done at an earlier one comes back.
//! * Snooze hides a card for an hour ([`SNOOZE`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use orion_core::{AgentId, ProjectId};
use serde::{Deserialize, Serialize};

use crate::app::App;

/// How long Snooze hides a card.
pub const SNOOZE: chrono::Duration = chrono::Duration::hours(1);

/// The file Done and Snooze are kept in, under the DATA DIR.
pub const STORE_FILE: &str = "needs-you.json";

/// The board's three columns, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    Agents,
    LinearAndReviews,
    SlackAndMeetings,
}

impl Column {
    pub const ALL: [Column; 3] = [
        Column::Agents,
        Column::LinearAndReviews,
        Column::SlackAndMeetings,
    ];

    /// Its sections, top to bottom.
    pub fn sections(self) -> &'static [Section] {
        match self {
            Column::Agents => &[Section::Agents],
            Column::LinearAndReviews => &[Section::Linear, Section::CodeReviews],
            Column::SlackAndMeetings => &[Section::Slack, Section::Meetings],
        }
    }
}

/// One heading of cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Agents,
    Linear,
    CodeReviews,
    Slack,
    Meetings,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Agents,
        Section::Linear,
        Section::CodeReviews,
        Section::Slack,
        Section::Meetings,
    ];

    /// The column it sits in.
    pub fn column(self) -> Column {
        match self {
            Section::Agents => Column::Agents,
            Section::Linear | Section::CodeReviews => Column::LinearAndReviews,
            Section::Slack | Section::Meetings => Column::SlackAndMeetings,
        }
    }

    /// Its heading.
    pub fn title(self) -> &'static str {
        match self {
            Section::Agents => "AGENTS",
            Section::Linear => "LINEAR",
            Section::CodeReviews => "CODE REVIEWS",
            Section::Slack => "SLACK",
            Section::Meetings => "MEETINGS",
        }
    }

    /// What it says with no cards.
    pub fn empty(self) -> &'static str {
        match self {
            Section::Agents => "No agents waiting for you",
            Section::Linear => "No Linear issues waiting for you",
            Section::CodeReviews => "No code reviews waiting for you",
            Section::Slack => "No Slack messages waiting for you",
            Section::Meetings => "No meetings waiting for you",
        }
    }
}

/// What names a card across refreshes and restarts: its source and the
/// thing's own id — `session:<agent id>`, `linear:<project>:<ENG-12>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CardKey(pub String);

impl CardKey {
    /// The key of the thing `target` jumps to: one card per thing.
    pub fn of(target: &CardTarget) -> CardKey {
        CardKey(match target {
            CardTarget::Session(id) => format!("session:{id}"),
            CardTarget::LinearIssue { project, id } => format!("linear:{project}:{id}"),
            CardTarget::PullRequest { url, .. } => format!("pr:{url}"),
            CardTarget::SlackMessage { permalink } => format!("slack:{permalink}"),
            CardTarget::Meeting { id } => format!("meeting:{id}"),
        })
    }
}

/// Where Enter on a card jumps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardTarget {
    Session(AgentId),
    LinearIssue { project: ProjectId, id: String },
    PullRequest { project: ProjectId, url: String },
    SlackMessage { permalink: String },
    Meeting { id: String },
}

/// One thing waiting on you.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub key: CardKey,
    pub section: Section,
    /// The first line: the session's name, the issue's title.
    pub title: String,
    /// The second line: `<kind> · <project> · <reason>`.
    pub sub: String,
    /// Since when it has waited. A source that changes the card moves it
    /// on, which brings back one marked done.
    pub since: DateTime<Utc>,
    /// Higher sorts first within its section: a crash over a question.
    pub urgency: u8,
    pub target: CardTarget,
}

impl Card {
    /// A card for `target`, keyed by it.
    pub fn new(
        section: Section,
        target: CardTarget,
        title: impl Into<String>,
        sub: impl Into<String>,
        since: DateTime<Utc>,
        urgency: u8,
    ) -> Card {
        Card {
            key: CardKey::of(&target),
            section,
            title: title.into(),
            sub: sub.into(),
            since,
            urgency,
            target,
        }
    }
}

/// One kind of thing that can wait on you: what it offers the board now.
pub trait CardSource {
    fn cards(&self, app: &App) -> Vec<Card>;
}

/// `cards` in board order: most urgent first, then the longest waiting.
pub fn sort(cards: &mut [Card]) {
    cards.sort_by(|a, b| {
        b.urgency
            .cmp(&a.urgency)
            .then(a.since.cmp(&b.since))
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// What Done and Snooze hide, as `needs-you.json` keeps it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    /// Done: each card's `since` when it was marked, so a later one shows.
    #[serde(default)]
    done: HashMap<CardKey, DateTime<Utc>>,
    /// Snoozed: when each card shows again.
    #[serde(default)]
    snoozed: HashMap<CardKey, DateTime<Utc>>,
}

impl Store {
    /// `<data dir>/needs-you.json`.
    pub fn path() -> PathBuf {
        orion_core::paths::data_dir().join(STORE_FILE)
    }

    /// The store at `path`; empty when it is missing or unreadable.
    pub fn load(path: &Path) -> Store {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write the store to `path` whole.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        orion_core::settings::write_atomic(path, &bytes)
    }

    /// Mark `card` done: hidden until its source moves its `since` on.
    pub fn done(&mut self, card: &Card) {
        self.snoozed.remove(&card.key);
        self.done.insert(card.key.clone(), card.since);
    }

    /// Hide `card` for [`SNOOZE`] from `now`.
    pub fn snooze(&mut self, card: &Card, now: DateTime<Utc>) {
        self.snoozed.insert(card.key.clone(), now + SNOOZE);
    }

    /// Whether `card` is hidden at `now`.
    pub fn hides(&self, card: &Card, now: DateTime<Utc>) -> bool {
        self.done.get(&card.key).is_some_and(|at| card.since <= *at)
            || self
                .snoozed
                .get(&card.key)
                .is_some_and(|until| now < *until)
    }

    /// `cards` without the hidden ones, in board order.
    pub fn visible(&self, mut cards: Vec<Card>, now: DateTime<Utc>) -> Vec<Card> {
        cards.retain(|card| !self.hides(card, now));
        sort(&mut cards);
        cards
    }

    /// Forget snoozes that have run out, and dones whose card no source
    /// offers any more. True when anything was forgotten.
    pub fn prune(&mut self, live: &[Card], now: DateTime<Utc>) -> bool {
        let before = (self.done.len(), self.snoozed.len());
        self.snoozed.retain(|_, until| now < *until);
        self.done
            .retain(|key, _| live.iter().any(|card| &card.key == key));
        before != (self.done.len(), self.snoozed.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn card(id: &str, since: i64, urgency: u8) -> Card {
        Card::new(
            Section::Meetings,
            CardTarget::Meeting { id: id.into() },
            id,
            "",
            at(since),
            urgency,
        )
    }

    #[test]
    fn each_section_sits_in_its_column() {
        for column in Column::ALL {
            for section in column.sections() {
                assert_eq!(section.column(), column);
            }
        }
        let listed: usize = Column::ALL.iter().map(|c| c.sections().len()).sum();
        assert_eq!(listed, Section::ALL.len());
    }

    #[test]
    fn a_done_card_stays_hidden_until_its_source_changes_it() {
        let mut store = Store::default();
        let standup = card("standup", 100, 0);
        store.done(&standup);
        assert!(store.hides(&standup, at(10_000)));
        let moved_on = card("standup", 200, 0);
        assert!(!store.hides(&moved_on, at(10_000)));
    }

    #[test]
    fn a_snoozed_card_comes_back_after_an_hour() {
        let mut store = Store::default();
        let standup = card("standup", 100, 0);
        store.snooze(&standup, at(1_000));
        assert!(store.hides(&standup, at(1_000 + 3_599)));
        assert!(!store.hides(&standup, at(1_000 + 3_600)));
        assert!(store.prune(std::slice::from_ref(&standup), at(1_000 + 3_600)));
        assert_eq!(store, Store::default());
    }

    #[test]
    fn done_is_forgotten_once_no_source_offers_the_card() {
        let mut store = Store::default();
        let (a, b) = (card("a", 1, 0), card("b", 1, 0));
        store.done(&a);
        store.done(&b);
        assert!(store.prune(std::slice::from_ref(&a), at(0)));
        assert!(store.hides(&a, at(0)));
        assert!(!store.prune(&[a], at(0)));
    }

    #[test]
    fn the_most_urgent_then_the_longest_waiting_come_first() {
        let store = Store::default();
        let cards = vec![
            card("new", 300, 1),
            card("old", 100, 1),
            card("crash", 400, 9),
        ];
        let order: Vec<String> = store
            .visible(cards, at(0))
            .into_iter()
            .map(|c| c.title)
            .collect();
        assert_eq!(order, ["crash", "old", "new"]);
    }

    #[test]
    fn the_store_survives_a_round_trip_and_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STORE_FILE);
        assert_eq!(Store::load(&path), Store::default());
        let mut store = Store::default();
        store.done(&card("a", 5, 0));
        store.snooze(&card("b", 5, 0), at(50));
        store.save(&path).unwrap();
        assert_eq!(Store::load(&path), store);
    }
}
