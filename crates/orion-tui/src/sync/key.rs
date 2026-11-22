//! What the RESOLVER can be asked for: one [`QueryKey`] per question, at
//! the grain a screen shows it — one project's open pull requests, one
//! checkout's branch lookup — and the [`Provider`] that answers it, which
//! is what a rate limit pauses.
//!
//! Every query orion makes in the background is declared here: a key that
//! exists is a question with one owner, and nothing fetches on a timer of
//! its own. What each answers, and where its answer lands, is in the
//! module that spawns it (`github`, `linear`, `services`, `local`). A
//! Linear issue's comments keep their own reads (`linear::schedule_comments`):
//! one per issue the cursor rests on, already one at a time.

use orion_core::{ProjectId, WorktreeId};

/// One question the resolver keeps an answer to. Scoped per tab or section
/// — one project's list, one project's Linear view — never per row, except
/// where the thing on screen is inherently one item (a pull request's
/// page). Two screens asking the same key share
/// one fetch.
///
/// The `String` keys of Linear carry the API key's label or hash, never
/// the key itself: a key is logged and drawn.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum QueryKey {
    // ---- GitHub ----
    /// A project's open pull requests: the PROJECT OPEN PRS GROUP's rows
    /// and their status, and what the checkouts on their branches wear.
    OpenPrs(ProjectId),
    /// What only the PULL REQUESTS MODAL draws of a project's list: labels,
    /// review requests, the checks' tallies.
    PrExtras(ProjectId),
    /// A project's merged (7 days) and closed (30 days) pull requests — the
    /// modal's `Merged` and `Closed` sections.
    PrTails(ProjectId),
    /// One pull request's page, by URL.
    PrDetail(String),
    /// A checkout's own pull request, for a branch no open list has: did
    /// it merge, close, or never have one.
    BranchPr(WorktreeId),
    /// Your open pull requests across every repo, for the automations.
    MyPrs,
    /// Pull requests asking for your review, for the NEEDS YOU board.
    ReviewRequests,
    // ---- Linear ----
    /// A project's LINEAR VIEW lists.
    LinearView(ProjectId),
    /// A Linear key's notifications and live issues, for NEEDS YOU.
    LinearInbox(String),
    /// The Linear builders' intake for one key.
    LinearIntake(String),
    /// A Linear key's teams, members and states.
    LinearTeams(String),
    // ---- Other services ----
    /// One account's usage, by account id.
    Usage(String),
    /// The next twelve hours of meetings.
    Calendar,
    /// One meeting's transcript, by meeting id.
    Transcript(String),
    // ---- Local ----
    /// A checkout's git status.
    GitStatus(WorktreeId),
    /// A project root's `git fetch` of its base branch.
    BaseFetch(ProjectId),
}

/// Who answers a key. A rate limit pauses the whole provider: every key it
/// answers waits until the limit resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provider {
    GitHub,
    Linear,
    /// Microsoft Graph: the calendar and transcripts.
    Microsoft,
    /// The Claude and Cursor usage endpoints.
    Usage,
    /// git and the disk: never rate limited.
    Local,
}

impl QueryKey {
    /// The provider that answers this key.
    pub fn provider(&self) -> Provider {
        use QueryKey::*;
        match self {
            OpenPrs(_) | PrExtras(_) | PrTails(_) | PrDetail(_) | BranchPr(_) | MyPrs
            | ReviewRequests => Provider::GitHub,
            LinearView(_) | LinearInbox(_) | LinearIntake(_) | LinearTeams(_) => Provider::Linear,
            Usage(_) => Provider::Usage,
            Calendar | Transcript(_) => Provider::Microsoft,
            GitStatus(_) | BaseFetch(_) => Provider::Local,
        }
    }
}

impl Provider {
    /// How many of its keys may be fetching at once. A burst — walking
    /// past a dozen projects, a grid of bands coming on screen, an
    /// automation's pages after a long sleep — goes a few at a time, the
    /// rest on the next step after one lands: GitHub's secondary limit
    /// punishes concurrency, and each local read is a handful of `git`
    /// processes on the same disk.
    pub fn max_in_flight(self) -> usize {
        match self {
            Provider::GitHub => 4,
            Provider::Linear | Provider::Microsoft | Provider::Usage | Provider::Local => 2,
        }
    }

    /// The provider's name as a person would say it: `GitHub limit, back
    /// at 22:11`.
    pub fn name(self) -> &'static str {
        match self {
            Provider::GitHub => "GitHub",
            Provider::Linear => "Linear",
            Provider::Microsoft => "Microsoft",
            Provider::Usage => "usage",
            Provider::Local => "git",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_key_names_the_provider_a_rate_limit_would_pause() {
        let p = ProjectId("p".into());
        let w = WorktreeId("w".into());
        assert_eq!(QueryKey::OpenPrs(p.clone()).provider(), Provider::GitHub);
        assert_eq!(QueryKey::BranchPr(w.clone()).provider(), Provider::GitHub);
        assert_eq!(QueryKey::MyPrs.provider(), Provider::GitHub);
        assert_eq!(QueryKey::LinearView(p.clone()).provider(), Provider::Linear);
        assert_eq!(QueryKey::Usage("a".into()).provider(), Provider::Usage);
        assert_eq!(QueryKey::Calendar.provider(), Provider::Microsoft);
        assert_eq!(QueryKey::GitStatus(w).provider(), Provider::Local);
        assert_eq!(QueryKey::BaseFetch(p).provider(), Provider::Local);
    }
}
