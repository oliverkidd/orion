//! Where a pull request stands, kept once (`App::prs`) and drawn from
//! there by every surface that names one: the PULL REQUESTS MODAL's rows
//! and page, the PR PREVIEW, the sidebar's PR ROW, the footer, the
//! LAUNCHER's bands and cards, the `/` PALETTE, Linear's work column, the
//! merge form and AUTOFIX.
//!
//! orion hears about one pull request from up to four answers: the
//! project's open list (every 15s for the selected project), the failing-
//! checks recheck folded into it, a checkout's own `gh pr view` lookup,
//! and the page the cursor rests on (`gh pr view N`) — plus the cache the
//! last launch left (`pr_cache`). Each lands on its own beat and keeps its
//! own copy (`OpenPr`, `PullRequest`, `PrDetail`) for what only it says:
//! list order, the meta line, the body, the conversation. What they all
//! say — the state, the draft flag, conflicts, checks — they say here
//! ([`PrStore::observe`]), each fact stamped with when its answer was
//! asked, under `fetch`'s rules: the newest asked answer wins, unknown
//! (`mergeable: UNKNOWN`, a truncated recheck) never overwrites known, and
//! a cached answer loses to any live one. A screen reads
//! [`PrStore::status`] and nothing else, so the list row and the page
//! beside it can no longer disagree about the same pull request.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::fetch::{Asked, Known};
use crate::pull_request::{
    Checks, Health, OpenPr, PrDetail, PullRequest, Standing, Trouble, STATE_MERGED, STATE_OPEN,
};

/// Every pull request orion has heard about, by URL.
///
/// One no row names any more is kept for [`RETIRED_KEEP`] past its newest
/// answer ([`PrStore::prune`]): a pull request a page said merged has left
/// every row, but a list asked before that page may still be on its way,
/// and only the merge remembered here keeps that list from bringing the
/// row back.
#[derive(Debug, Clone, Default)]
pub struct PrStore {
    facts: HashMap<String, PrFacts>,
}

/// How long the facts about a pull request no row names outlive its newest
/// answer. A list in flight answers within seconds (`pull_request::TIMEOUT`
/// is 20s); this is that with room to spare.
pub const RETIRED_KEEP: Duration = Duration::from_secs(10 * 60);

/// What is known about one pull request, each fact with when it was asked.
#[derive(Debug, Clone, Default)]
pub struct PrFacts {
    pub title: Known<String>,
    /// `gh`'s state string: `OPEN`, `MERGED` or `CLOSED`.
    pub state: Known<String>,
    pub is_draft: Known<bool>,
    pub conflicts: Known<bool>,
    pub checks: Known<Checks>,
    pub head_sha: Known<String>,
}

impl PrFacts {
    /// When the newest live answer about it was asked; `None` with only
    /// the cache's.
    fn newest(&self) -> Option<Instant> {
        [
            self.title.asked(),
            self.state.asked(),
            self.is_draft.asked(),
            self.conflicts.asked(),
            self.checks.asked(),
            self.head_sha.asked(),
        ]
        .into_iter()
        .filter_map(|asked| match asked {
            Some(Asked::At(at)) => Some(at),
            _ => None,
        })
        .max()
    }
}

/// What one answer said about one pull request, every part optional: a
/// part it didn't ask, or couldn't trust, is `None` and leaves the stored
/// fact as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrObservation {
    pub title: Option<String>,
    pub state: Option<String>,
    pub is_draft: Option<bool>,
    pub conflicts: Option<bool>,
    pub checks: Option<Checks>,
    pub head_sha: Option<String>,
}

/// A string an answer left empty says nothing.
fn said(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

impl PrObservation {
    /// A row of a project's open list, the failing-checks recheck already
    /// folded in (`pull_request::list`). Every row is open — the list asks
    /// for nothing else — and its checks are the one verdict the row
    /// carries ([`OpenPr::listed_checks`]). A page folds its own checks
    /// ([`of_detail`](Self::of_detail)), and where the two disagree the one
    /// asked last stands, as with every other fact: the recheck already
    /// corrects the list's failing case, the one that matters.
    pub fn of_list_row(pr: &OpenPr) -> Self {
        Self {
            title: said(&pr.title),
            state: Some(STATE_OPEN.to_string()),
            is_draft: Some(pr.answered_draft),
            conflicts: pr.answered.conflicts,
            checks: pr.listed_checks(),
            head_sha: said(&pr.head_sha),
        }
    }

    /// A row of the list's merged tail (`ListAnswer::merged`): merged, by
    /// the query that brought it, and nothing asked of its conflicts or
    /// checks — a merged pull request is past both.
    pub fn of_merged_row(pr: &OpenPr) -> Self {
        Self {
            title: said(&pr.title),
            state: Some(STATE_MERGED.to_string()),
            head_sha: said(&pr.head_sha),
            ..Self::default()
        }
    }

    /// A checkout's own lookup (`gh pr view` on its branch). It asks no
    /// head commit.
    pub fn of_lookup(pr: &PullRequest) -> Self {
        Self {
            title: said(&pr.title),
            state: said(&pr.answered_state),
            is_draft: Some(pr.answered_draft),
            conflicts: pr.answered.conflicts,
            checks: pr.answered.checks,
            head_sha: None,
        }
    }

    /// One pull request's page (`gh pr view N`).
    pub fn of_detail(detail: &PrDetail) -> Self {
        Self {
            title: said(&detail.title),
            state: said(&detail.answered_state),
            is_draft: Some(detail.answered_draft),
            conflicts: detail.answered.conflicts,
            checks: detail.answered.checks,
            head_sha: said(&detail.head_sha),
        }
    }

    /// Only the draft flag — what `gh pr ready` (or its `--undo`) just
    /// made true.
    pub fn of_draft(is_draft: bool) -> Self {
        Self {
            is_draft: Some(is_draft),
            ..Self::default()
        }
    }
}

/// Where one pull request stands, as every surface draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrStatus {
    pub standing: Standing,
    /// Conflicts and checks as drawn: conflicts nobody has answered read
    /// as none ([`conflicts_known`](Self::conflicts_known) says whether
    /// that is a guess), checks nobody has answered as absent.
    pub health: Health,
    pub conflicts_known: bool,
    pub checks_known: bool,
    /// Empty while unknown.
    pub title: String,
    /// The head commit; empty while unknown.
    pub head_sha: String,
}

/// A pull request nothing has said anything about yet: open — the trust
/// `gh`'s own missing state gets — and healthy, with nothing known.
impl Default for PrStatus {
    fn default() -> Self {
        Self {
            standing: Standing::Open,
            health: Health::default(),
            conflicts_known: false,
            checks_known: false,
            title: String::new(),
            head_sha: String::new(),
        }
    }
}

impl PrStatus {
    /// Still taking work: open or a draft.
    pub fn is_open(&self) -> bool {
        self.standing.is_open()
    }

    /// What the row goes red for, while the pull request is still open:
    /// a merged or closed one is past needing its branch resolved.
    pub fn trouble(&self) -> Option<Trouble> {
        self.is_open().then(|| self.health.trouble()).flatten()
    }

    /// The badge's word: the trouble's while there is one, else the
    /// state's (`ready`, `draft`, `merged`, `closed`).
    pub fn word(&self) -> &'static str {
        self.standing.word(self.trouble())
    }

    pub fn is_draft(&self) -> bool {
        self.standing == Standing::Draft
    }

    /// The title the newest answer gave, or `copy`'s — the title the row
    /// in hand carries — while none has. Two surfaces naming the same pull
    /// request read it here, so a rename shows on both at once.
    pub fn title_or<'a>(&'a self, copy: &'a str) -> &'a str {
        if self.title.is_empty() {
            copy
        } else {
            &self.title
        }
    }

    /// Nothing stands in the way, and that is known rather than guessed:
    /// GitHub said the branch merges, and every check passed (or there
    /// are none). What AUTOFIX takes as "gone green" — an unknown
    /// conflicts answer is never green.
    pub fn is_green(&self) -> bool {
        self.conflicts_known
            && !self.health.conflicts
            && self.checks_known
            && matches!(self.health.checks, Checks::Passing | Checks::Absent)
    }
}

impl PrStore {
    /// `#42 title`, the title [`PrStatus::title_or`]'s: the newest answer's,
    /// else `title` — the copy in hand.
    pub fn label(&self, number: u64, url: &str, title: &str) -> String {
        let status = self.status(url);
        let title = status.as_ref().map_or(title, |s| s.title_or(title));
        crate::pull_request::numbered_label(number, title)
    }

    /// Take what one answer, asked at `asked`, said about `url`. Each fact
    /// moves only when the answer knew it and was asked no earlier than
    /// the one stored (`fetch::Known::observe`). True when anything a
    /// screen draws changed.
    pub fn observe(&mut self, url: &str, seen: PrObservation, asked: Asked) -> bool {
        let facts = self.facts.entry(url.to_string()).or_default();
        let mut changed = facts.title.observe(seen.title, asked);
        changed |= facts.state.observe(seen.state, asked);
        changed |= facts.is_draft.observe(seen.is_draft, asked);
        changed |= facts.conflicts.observe(seen.conflicts, asked);
        changed |= facts.checks.observe(seen.checks, asked);
        changed |= facts.head_sha.observe(seen.head_sha, asked);
        changed
    }

    /// One row of a list asked at `asked`, as [`observe`](Self::observe)
    /// takes it. A row the failing-checks recheck could not read in full
    /// (`cut`, `pull_request::ListAnswer::cut`) carries GitHub's rollup
    /// word unfolded: it says nothing of the checks while a verdict is
    /// known — that one stands — and is the verdict while none is, so a
    /// busy failing pull request seen for the first time still goes red.
    pub fn observe_list_row(&mut self, pr: &OpenPr, cut: bool, asked: Asked) -> bool {
        let mut seen = PrObservation::of_list_row(pr);
        if cut {
            let known = self.facts.get(&pr.url).is_some_and(|f| f.checks.is_known());
            seen.checks = if known { None } else { pr.answered.checks };
        }
        self.observe(&pr.url, seen, asked)
    }

    /// Where `url` stands, from the newest answer that knew each part;
    /// `None` for a pull request no answer has named.
    pub fn status(&self, url: &str) -> Option<PrStatus> {
        let facts = self.facts.get(url)?;
        let state = facts.state.get().map_or(STATE_OPEN, String::as_str);
        let is_draft = facts.is_draft.get().copied().unwrap_or(false);
        Some(PrStatus {
            standing: Standing::of(state, is_draft),
            health: Health {
                conflicts: facts.conflicts.get().copied().unwrap_or(false),
                checks: facts.checks.get().copied().unwrap_or_default(),
            },
            conflicts_known: facts.conflicts.is_known(),
            checks_known: facts.checks.is_known(),
            title: facts.title.get().cloned().unwrap_or_default(),
            head_sha: facts.head_sha.get().cloned().unwrap_or_default(),
        })
    }

    /// [`status`](Self::status), or an open pull request with nothing
    /// known — for a row that is open by construction (a list row) and
    /// must draw something even before its answer is observed.
    pub fn status_or_open(&self, url: &str) -> PrStatus {
        self.status(url).unwrap_or_default()
    }

    /// The facts about `url`, for code that needs a fact's asked time.
    pub fn facts(&self, url: &str) -> Option<&PrFacts> {
        self.facts.get(url)
    }

    /// Forget every pull request that is not `live` (on some row) and whose
    /// newest live answer is older than [`RETIRED_KEEP`] at `now`; one known
    /// only from the cache has none, and goes as soon as no row names it.
    pub fn prune(&mut self, live: &HashSet<String>, now: Instant) {
        self.facts.retain(|url, facts| {
            live.contains(url)
                || facts
                    .newest()
                    .is_some_and(|at| now.saturating_duration_since(at) < RETIRED_KEEP)
        });
    }
}
