//! WEEK IN REVIEW: what was finished in the last seven days — merged
//! pull requests, Linear issues done, todos ticked — written up as a short
//! review to read aloud with the product on screen: what changed, by
//! product area, with what to show and what to say. It is an automation
//! (`crate::automations::week_review`): run on its **When**, by `r` on the
//! Automations screen, or by "Run week in review now" in `⌘⇧P`, and each
//! run is one review a project that had changes, read on a page with a
//! tab a project ([`open`]).
//!
//! * **Gathering is orion's.** A run fetches every pull request merged in
//!   the week in every project, description and all ([`fetch_merged`]):
//!   a review is written from what a pull request says it does. Linear's
//!   done issues and the ticked todos come from the lists the app already
//!   holds ([`gather`]); a project with none of the three is left out
//!   ([`split`]).
//! * **Writing is a model's, one ask a project.** [`prompt`] puts a
//!   project's week in one text and [`write`] sends it, as one
//!   [`oneshot::run`], through a Claude account orion runs — the
//!   automation's preset's, else the default agent's
//!   ([`oneshot::account_for`]) — with no tools: nothing runs in a
//!   checkout, no session appears on the grid, and there is nothing to
//!   tidy up after. [`AT_ONCE`] are written at a time.
//! * **Counting is orion's again.** The model is told to list what earns
//!   a line and nothing else; [`check`] holds the reply to its shape
//!   (every number a pull request that was sent, every owner its author)
//!   and [`finish`] appends how many were merged and not listed — exact,
//!   because nothing was asked to add it up.
//!
//! A run is kept under the DATA DIR as `reviews/all/<to>/<project>.md`,
//! one a project, with a `meta.json` beside them ([`Meta`]), the newest
//! [`KEEP`] runs. Nothing here touches the disk or starts a process unless
//! the main loop installed a directory and a channel ([`State`]): the
//! unit tests never install the channel.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::ProjectId;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::hints::Hint;
use crate::oneshot::{self, Account, Reply};
use crate::pull_request::{bool_at, rfc3339_secs, str_at, u64_at};
use crate::theme::Theme;
use crate::ui::{centered_rect_pct, panel_block, row_rect, truncate, SPLIT_MODAL_PCT};

/// How far back a review reaches: the PULL REQUESTS MODAL's week of
/// merges, and Linear's week of done issues.
pub const DAYS: i64 = crate::pull_request::MERGED_DAYS;
// Linear cuts its done issues to its own week on Linear's side: a review
// reaching further back than that would find none of them there.
const _: () = assert!(DAYS == crate::linear::DONE_DAYS as i64);
const DAY_SECS: i64 = 24 * 60 * 60;
/// How far back a month's review reaches.
const MONTH_DAYS: i64 = 30;
/// How many runs are kept; the oldest go as a new one lands.
const KEEP: usize = 12;
/// How many of a run's reviews are written at once.
const AT_ONCE: usize = 3;
/// How much of a pull request's description goes in the prompt: its
/// summary paragraph is what matters…
const BODY_CUT: usize = 600;
/// …unless it is a very large one ([`BIG_ADDITIONS`]). A feature branch
/// merged in one go often has only its list of commits for a description,
/// and the first lines of that say nothing.
const BIG_BODY_CUT: usize = 6000;
const BIG_ADDITIONS: u64 = 5000;
/// How much of a Linear issue's description goes in.
const ISSUE_CUT: usize = 1500;
/// A fetch younger than this is what the modal opens on, with no second
/// ask: bodies are heavy, and a merged pull request does not change.
const FRESH: Duration = Duration::from_secs(5 * 60);
/// How long one page of descriptions may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(45);
/// How many pages [`fetch_merged`] reads before it gives up on a week: a
/// guard against a walk that never ends, set where no repo's week reaches
/// — a thousand merged pull requests touched in seven days.
const PAGES_MAX: usize = 10;
/// How long the write may take before it is given up on.
const WRITE_TIMEOUT: Duration = Duration::from_secs(300);

/// The model a review is written by while the **Week review** SYSTEM
/// PRESET names none: the best blend of speed and judgement when the
/// brief was tried on a real week.
pub const DEFAULT_MODEL: &str = "sonnet";
/// Its effort while the preset names none: medium takes about 45 seconds
/// on a busy week, low about 30 with fewer details.
pub const DEFAULT_EFFORT: &str = "medium";

/// Whose work a review covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Whose {
    /// The pull requests you opened and the issues assigned to you.
    #[default]
    Mine,
    /// Everyone's, each point naming who owned it.
    Everyone,
}

/// Which projects a review covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Where {
    #[default]
    Project,
    All,
}

/// How far back a review reaches, to today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Range {
    #[default]
    Week,
    Fortnight,
    Month,
}

impl Range {
    pub const ALL: [Range; 3] = [Range::Week, Range::Fortnight, Range::Month];

    /// The days it covers, today among them.
    pub fn days(self) -> i64 {
        match self {
            Range::Week => DAYS,
            Range::Fortnight => 2 * DAYS,
            Range::Month => MONTH_DAYS,
        }
    }

    /// What the review's own title calls it.
    fn title(self) -> &'static str {
        match self {
            Range::Week => "Week in review",
            Range::Fortnight => "Two weeks in review",
            Range::Month => "Month in review",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Scope {
    pub whose: Whose,
    pub wher: Where,
    /// A review kept from before there was a choice is a week's.
    #[serde(default)]
    pub range: Range,
}

impl Scope {
    /// The days a review of this scope written at `now` covers.
    fn period(self, now: i64) -> (NaiveDate, NaiveDate) {
        period(now, self.range.days())
    }

    /// `just me` or `whole team`, as the automation's **Whose** says it.
    fn whose_label(self) -> &'static str {
        match self.whose {
            Whose::Mine => "just me",
            Whose::Everyone => "whole team",
        }
    }
}

/// One pull request merged in the period, as [`fetch_merged`] read it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MergedPr {
    /// The project's name: what tells two projects' `#12` apart.
    pub project: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub login: String,
    /// The author's GitHub name; empty where they gave none.
    pub name: String,
    /// The signed-in `gh` user opened it.
    pub mine: bool,
    pub base: String,
    pub head: String,
    pub additions: u64,
    pub deletions: u64,
    pub files: u64,
    /// RFC 3339.
    pub merged_at: String,
    /// The description. Sent to the model, never kept with the review —
    /// and shared, so what a review would be written from is gathered
    /// afresh on every frame without copying a week of descriptions.
    #[serde(skip)]
    pub body: std::sync::Arc<str>,
}

impl MergedPr {
    /// One standing branch moved into another — `dev` into `main` — which
    /// repeats what the week's other pull requests already say. A feature
    /// branch merged into `main` is not one.
    fn is_release(&self) -> bool {
        const TARGETS: &[&str] = &["main", "master", "production", "prod"];
        const STANDING: &[&str] = &[
            "dev",
            "develop",
            "development",
            "staging",
            "stage",
            "next",
            "main",
            "master",
        ];
        TARGETS.contains(&self.base.as_str()) && STANDING.contains(&self.head.as_str())
    }

    /// `#412`, or `app#412` where the review covers several projects.
    fn reference(&self, several: bool) -> String {
        if several {
            format!("{}#{}", project_slug(&self.project), self.number)
        } else {
            format!("#{}", self.number)
        }
    }
}

/// A project's name as a pull request reference carries it: lower case,
/// everything that is not a letter or digit a `-`.
fn project_slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.trim_matches('-').to_string()
}

/// A Linear issue done in the period.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DoneIssue {
    pub identifier: String,
    pub title: String,
    pub assignee: String,
    /// RFC 3339: when Linear completed it, else when the last of its pull
    /// requests merged in the period did.
    pub done_at: String,
    #[serde(skip)]
    pub description: String,
}

/// A todo ticked in the period.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DoneTodo {
    pub text: String,
    /// Unix seconds.
    pub done_at: i64,
}

/// Everything one review is written from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Material {
    pub prs: Vec<MergedPr>,
    pub issues: Vec<DoneIssue>,
    pub todos: Vec<DoneTodo>,
}

impl Material {
    fn is_empty(&self) -> bool {
        self.prs.is_empty() && self.issues.is_empty() && self.todos.is_empty()
    }

    /// Pull requests from more than one project: a bare `#12` would then
    /// name two of them.
    fn several_projects(&self) -> bool {
        let mut names = self.prs.iter().map(|pr| pr.project.as_str());
        match names.next() {
            Some(first) => names.any(|name| name != first),
            None => false,
        }
    }
}

// ---- fetching ----

/// Every merged pull request, most recently touched first, a page at a
/// time: the PULL REQUESTS MODAL's merged tail ([`crate::pull_request`])
/// with each one's description, size and branches, and its author's name.
const BODIES_QUERY: &str =
    "query($owner: String!, $repo: String!, $n: Int!, $after: String) { \
    repository(owner: $owner, name: $repo) { \
    pullRequests(states: MERGED, first: $n, after: $after, orderBy: {field: UPDATED_AT, direction: DESC}) { \
    pageInfo { hasNextPage endCursor } \
    nodes { number url title body additions deletions changedFiles baseRefName headRefName \
    mergedAt updatedAt viewerDidAuthor author { __typename login ... on User { name } } } } } }";

/// One page of [`BODIES_QUERY`]: its pull requests merged inside the
/// window, and where the next page starts while the week can run on.
struct Page {
    rows: Vec<MergedPr>,
    more: Option<String>,
}

/// Parse one page of the window from `since` (unix seconds). A pull
/// request merged inside the window was last touched inside it too, so a
/// page that ends on one touched before it is the last worth reading.
/// Bots' pull requests (dependency bumps) are left out here: no review
/// has a line for one.
fn parse_page(json: &str, project: &str, since: i64) -> Option<Page> {
    let answer: serde_json::Value = serde_json::from_str(json).ok()?;
    let tail = answer.pointer("/data/repository/pullRequests")?;
    let nodes = tail.get("nodes")?.as_array()?;
    let rows = nodes
        .iter()
        .filter_map(|v| {
            let merged_at = str_at(v, "mergedAt");
            rfc3339_secs(&merged_at).filter(|at| *at >= since)?;
            let author = v.get("author").cloned().unwrap_or_default();
            if str_at(&author, "__typename") == "Bot" {
                return None;
            }
            Some(MergedPr {
                project: project.to_string(),
                number: v.get("number")?.as_u64()?,
                title: str_at(v, "title"),
                url: str_at(v, "url"),
                login: str_at(&author, "login"),
                name: str_at(&author, "name"),
                mine: bool_at(v, "viewerDidAuthor"),
                base: str_at(v, "baseRefName"),
                head: str_at(v, "headRefName"),
                additions: u64_at(v, "additions"),
                deletions: u64_at(v, "deletions"),
                files: u64_at(v, "changedFiles"),
                merged_at,
                body: str_at(v, "body").into(),
            })
        })
        .collect();
    let past_window = nodes
        .last()
        .and_then(|v| rfc3339_secs(&str_at(v, "updatedAt")))
        .is_some_and(|touched| touched < since);
    let has_next = bool_at(&tail["pageInfo"], "hasNextPage");
    let more = tail
        .pointer("/pageInfo/endCursor")
        .and_then(|c| c.as_str())
        .filter(|_| has_next && !past_window)
        .map(str::to_string);
    Some(Page { rows, more })
}

/// Every pull request merged in `dir`'s repo in the last `days` before
/// `now`, the latest merge first, descriptions and all. `None` when
/// GitHub could not be asked, or a page of the window could not be read:
/// a review written from part of a week would pass it off as the whole.
pub async fn fetch_merged(
    dir: PathBuf,
    project: String,
    now: i64,
    days: i64,
) -> Option<Vec<MergedPr>> {
    let since = now - days * DAY_SECS;
    let page_size = format!("n={}", crate::pull_request::MERGED_PAGE);
    let mut rows = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..PAGES_MAX {
        let cursor = after.take().map(|c| format!("after={c}"));
        let mut vars = vec![page_size.as_str()];
        if let Some(cursor) = &cursor {
            vars.push(cursor.as_str());
        }
        let out =
            crate::pull_request::repo_graphql(&dir, BODIES_QUERY, &vars, FETCH_TIMEOUT).await?;
        let page = parse_page(&out, &project, since)?;
        rows.extend(page.rows);
        match page.more {
            Some(cursor) => after = Some(cursor),
            None => return Some(latest_first(rows)),
        }
    }
    None
}

/// The pages' rows, the latest merge first, each pull request once — a
/// page read a moment after the last can repeat one touched in between.
fn latest_first(mut rows: Vec<MergedPr>) -> Vec<MergedPr> {
    // RFC 3339 stamps in UTC sort as text.
    rows.sort_by(|a, b| b.merged_at.cmp(&a.merged_at));
    let mut seen = HashSet::new();
    rows.retain(|pr| seen.insert(pr.url.clone()));
    rows
}

// ---- gathering ----

/// The sources a review can be written from, in the new review's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Prs,
    Issues,
    Todos,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Prs, Source::Issues, Source::Todos];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Source::Prs => "Merged pull requests",
            Source::Issues => "Linear issues done",
            Source::Todos => "Todos ticked",
        }
    }
}

/// The projects `scope` covers, starting from the modal's own: `(id,
/// name, checkout)`.
fn projects_in(app: &App, project: &ProjectId, wher: Where) -> Vec<(ProjectId, String, PathBuf)> {
    app.tree
        .projects
        .iter()
        .filter(|p| wher == Where::All || &p.id == project)
        .map(|p| (p.id.clone(), p.name.clone(), p.repo_path.clone()))
        .collect()
}

/// What a review of `scope` would be written from right now: the fetched
/// pull requests, the done issues Linear's list holds — its own week of
/// them, however far back the range reaches — and the todos ticked, each
/// inside the range before `now` and each only when its source is ticked
/// in `picks`. An issue is done when Linear completed
/// it in the week or one of its pull requests merged in it — whatever a
/// team calls the state it waits in after that (`In Dev`, `In Staging`),
/// its work landed. `Just mine` keeps what is yours; `Everyone` keeps all
/// of it and leaves the todos out, which are nobody else's. An issue two
/// projects' lists both hold is taken once.
pub fn gather(
    app: &App,
    project: &ProjectId,
    scope: Scope,
    picks: [bool; 3],
    now: i64,
) -> Material {
    let days = scope.range.days();
    let since = now - days * DAY_SECS;
    let mine = scope.whose == Whose::Mine;
    let mut material = Material::default();
    let mut seen_issues = HashSet::new();
    let projects = projects_in(app, project, scope.wher);
    let fetched = |id: &ProjectId| {
        let window = (id.clone(), days);
        app.week_review
            .fetched
            .get(&window)
            .and_then(|f| f.prs.as_ref())
    };
    // Every project's, not just the issue's own: one Linear workspace can
    // hold the issues of several repos.
    let merged: HashMap<String, &str> = projects
        .iter()
        .flat_map(|(id, _, _)| fetched(id).into_iter().flatten())
        .filter_map(|pr| Some((pull_key(&pr.url)?, pr.merged_at.as_str())))
        .collect();
    for (id, _, dir) in projects {
        if picks[Source::Prs.index()] {
            let fetched = fetched(&id);
            material.prs.extend(
                fetched
                    .into_iter()
                    .flatten()
                    .filter(|pr| !mine || pr.mine)
                    .cloned(),
            );
        }
        if picks[Source::Issues.index()] {
            let issues = app.linear.get(&id).map(|l| l.list.as_slice());
            for issue in issues.into_iter().flatten() {
                let completed = issue.status_type == "completed"
                    && rfc3339_secs(&issue.completed_at).is_some_and(|at| at >= since);
                let done_at = if completed {
                    Some(issue.completed_at.as_str())
                } else {
                    issue
                        .prs
                        .iter()
                        .filter_map(|pr| merged.get(&pull_key(&pr.url)?).copied())
                        .max()
                };
                let Some(done_at) = done_at else {
                    continue;
                };
                if (!mine || issue.mine) && seen_issues.insert(issue.id.clone()) {
                    material.issues.push(DoneIssue {
                        identifier: issue.identifier.clone(),
                        title: issue.title.clone(),
                        assignee: issue.assignee.clone(),
                        done_at: done_at.to_string(),
                        description: issue.description.clone(),
                    });
                }
            }
        }
        if picks[Source::Todos.index()] && mine {
            let items = app.todos.get(&dir).map(|file| file.items.as_slice());
            push_ticked(&mut material, items.into_iter().flatten(), since);
        }
    }
    // Blue sky's ideas ticked: every one in a review of all projects, and
    // in one project's, those built there.
    if picks[Source::Todos.index()] && mine {
        let ideas = app
            .todos
            .get(&crate::todos::store::blue_sky_key())
            .map(|file| file.items.as_slice());
        let ours = |item: &&crate::todos::Item| {
            scope.wher == Where::All || item.target.as_deref() == Some(project.0.as_str())
        };
        push_ticked(
            &mut material,
            ideas.into_iter().flatten().filter(ours),
            since,
        );
    }
    material.prs.sort_by(|a, b| b.merged_at.cmp(&a.merged_at));
    material.issues.sort_by(|a, b| b.done_at.cmp(&a.done_at));
    material.todos.sort_by_key(|t| std::cmp::Reverse(t.done_at));
    material
}

/// A pull request's URL as both GitHub and a Linear attachment spell it:
/// the repo in lower case and the number, without a `/files` or `#…` after.
fn pull_key(url: &str) -> Option<String> {
    let (repo, rest) = url.split_once("/pull/")?;
    let number: u64 = rest.split(['/', '#', '?']).next()?.parse().ok()?;
    Some(format!("{}/pull/{number}", repo.to_ascii_lowercase()))
}

// ---- people ----

/// The parts a name or a login falls into: at spaces, `-`, `_`, `.`, and
/// where a capital follows a lower-case letter (`AdaLovelace`).
fn name_parts(text: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut previous = ' ';
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, '-' | '_' | '.') {
            parts.push(String::new());
        } else {
            if c.is_uppercase() && previous.is_lowercase() {
                parts.push(String::new());
            }
            if let Some(part) = parts.last_mut() {
                part.push(c);
            }
        }
        previous = c;
    }
    parts.retain(|p| p.chars().any(char::is_alphabetic));
    parts
}

/// A person's initials: the first letters of the first and last parts of
/// their GitHub name — or of their login, where they gave no name — and
/// the first two letters where there is only one part (`Grace` is `GR`).
fn initials_of(name: &str, login: &str) -> String {
    let source = if name.trim().is_empty() { login } else { name };
    let parts = name_parts(source);
    let letters: String = match parts.as_slice() {
        [] => "??".into(),
        [one] => one.chars().filter(|c| c.is_alphabetic()).take(2).collect(),
        [first, .., last] => first.chars().take(1).chain(last.chars().take(1)).collect(),
    };
    letters.to_uppercase()
}

/// Everyone who wrote a pull request in `prs`, as `(login, initials,
/// name)`, whoever merged the most first. Two people with the same
/// initials are told apart by a third letter, then by a number.
pub fn legend(prs: &[MergedPr]) -> Vec<(String, String, String)> {
    let mut merged: HashMap<&str, usize> = HashMap::new();
    for pr in prs {
        *merged.entry(pr.login.as_str()).or_default() += 1;
    }
    let mut people: Vec<(&str, &str)> = prs
        .iter()
        .map(|pr| (pr.login.as_str(), pr.name.as_str()))
        .collect();
    people.sort();
    people.dedup_by_key(|(login, _)| *login);
    people.sort_by_key(|(login, _)| std::cmp::Reverse(merged[login]));
    let mut taken = HashSet::new();
    people
        .into_iter()
        .map(|(login, name)| {
            let mut initials = initials_of(name, login);
            if taken.contains(&initials) {
                let source = if name.trim().is_empty() { login } else { name };
                let third: String = source
                    .chars()
                    .filter(|c| c.is_alphabetic())
                    .take(3)
                    .collect();
                initials = third.to_uppercase();
            }
            let stem = initials.clone();
            let mut n = 2;
            while taken.contains(&initials) {
                initials = format!("{stem}{n}");
                n += 1;
            }
            taken.insert(initials.clone());
            let shown = if name.trim().is_empty() { login } else { name };
            (login.to_string(), initials, shown.to_string())
        })
        .collect()
}

// ---- the prompt ----

const BRIEF_OPENING: &str = "\
# The week in review: what it is and how to write it

This is read aloud to the whole company in about five minutes, with the product on screen. The reader walks through it area by area and demos what is worth seeing. It is not a changelog: most of what was merged does not belong in it.
";

const BRIEF_EARNS: &str = "
## What earns a line

- A point is something a person OUTSIDE the team that built it would want to know or see. The test: would you stop the meeting to say this? If not, it is a smaller fix, however much work it was.
- By weight: (1) a redesign, or a capability that did not exist last week; (2) a measured gain in speed, memory or size; (3) a change to something people do every day. Those are points.
- Everything else is a smaller fix and gets no line: sizing, spacing, alignment, rounding, renaming, wording, placeholders, an edge case that now behaves, a glitch that is gone, a default that changed.
- Size is a signal. The handful of largest pull requests that are not marked RELEASE are where the week's biggest changes live: read those first, and make sure each one is either a point (or two or three, for a feature-branch merge) or is left out because no user would see it. A redesign that arrives as one huge merge gets an area of its own.
- A tweak to how an existing control behaves (how something toggles, snaps, aligns, resizes or is labelled) is a smaller fix, unless it removes something people fought with every day.
- 2 to 4 points an area, numbered from 1 again in each area. 12 to 18 points in the whole review, however many pull requests there were; fewer when the week was small. Fewer and stronger beats complete.
- 4 to 6 areas, fewer for a small week, each named in one to three words the way a user would name that part of the product. Three or more measured speed, memory or size gains get an area of their own called `Speed`, and only measured gains go in it. Infrastructure a user would feel goes last under `Under the hood`, at most 3 points; otherwise it is left out.
- Lead with the product's core, what people come to it to do, and put its edges (accounts, sign-in, admin, settings) after. Inside that, order areas and points by how much there is to show.
- When two candidates compete for a line, the one in the core of the product wins over the one at its edge.
- A pull request that merges a feature branch IS the feature, however thin its description: its list of commits is its description. Read the list and pull out the two or three changes a user would notice most; each is its own point, and that one number may then appear on up to three points. Only a pull request marked RELEASE, which moves one standing branch into another, is left out.

## Show and Say

- `Show` is the demo. Every point that can be seen on screen gets one: what the presenter does, then what appears. Most points have a Show. It names a place and an action (\"Press ⌘K, type a word: results land first, pages after\"), never \"see the new look\".
- If the description does not say enough to know what to click, write the point without a Show rather than guess.
- A new capability with no Show is a missed demo: read its description again for where it lives and what the user presses.
- `Say` is the fact the screen does not show: a number, before to after, or the consequence for the user. Only numbers the description states.
- At most 16 words for a Show, 14 for a Say. One of each at most.

## Writing a point

- ONE change, as the user meets it, in at most 9 words, present tense. Never two changes joined by a semicolon, \"and\" or a comma.
- Name the change. Never \"lands\", \"improvements\", \"updates\", \"various\".
- No \"fix:\"/\"feat:\" prefixes, file names, internal code names or ticket ids. Something behind a flag ends `(behind a flag)`.
- One point can cover up to 3 closely related pull requests, each written exactly as its entry below names it. Any other pull request appears once in the whole file.
- Use only what the descriptions say. Never invent a number, a name or a behaviour.
";

/// orion's own rules for what earns a line and how a point is written:
/// the **Week review** SYSTEM PRESET's text while it holds none of the
/// user's ([`crate::agent_presets::SystemPreset::builtin_text`]).
pub fn brief_rules() -> &'static str {
    BRIEF_EARNS.trim()
}

const BRIEF_CLOSING: &str = "
- Do not count or list what you leave out: the smaller fixes, releases, dependency bumps, CI, tests and tooling simply do not appear. The total left unlisted is added afterwards.
- No introduction, no summary, nothing after the last point.
";

const ISSUES_INTRO: &str = "
## Linear issues done in the period

Tickets, most of them the same work as a pull request above. Use them to understand what a change was for and to word its point. An issue gets no point of its own unless no pull request covers it, and an issue is never cited: the references are pull requests only.

";

const TODOS_INTRO: &str = "
## Todos ticked in the period

The asker's own notes, for context only: what was on their mind. Never a point by themselves.

";

/// What the model is asked, whole: the brief — `rules` its own rules for
/// what earns a line, the automation's preset's text and extra steer —
/// the shape, and every pull request, issue and todo of `material`.
/// `people` is [`legend`]'s, used in an `Everyone` review to name each
/// point's owner.
pub fn prompt(
    material: &Material,
    people: &[(String, String, String)],
    scope: Scope,
    project: &str,
    rules: &str,
    now: i64,
) -> String {
    let everyone = scope.whose == Whose::Everyone;
    let several = material.several_projects();
    let (from, to) = scope.period(now);
    let title = scope.range.title();
    let initials: HashMap<&str, &str> = people
        .iter()
        .map(|(login, initials, _)| (login.as_str(), initials.as_str()))
        .collect();
    let roster: Vec<String> = people
        .iter()
        .map(|(_, initials, name)| format!("{initials} {name}"))
        .collect();
    let roster = roster.join(" · ");
    let mut text = String::from(BRIEF_OPENING);
    text.push_str(&format!(
        "\nPeriod: {} {}. Project: {project}. Scope: {}.\n",
        period_label(from, to, true),
        to.format("%Y"),
        scope.whose_label(),
    ));
    if everyone {
        text.push_str(&format!("People, by initials: {roster}.\n"));
    }
    text.push_str(&format!("\n{}\n", rules.trim()));
    if everyone {
        text.push_str("- A point's owner is the author of the first pull request it lists. Exactly one owner a point, by the initials given.\n");
    }
    text.push_str("\n## The shape, exactly\n\n```\n");
    text.push_str(&format!(
        "# {title} · {}\n{project} · {} · {} pull requests merged\n\n",
        period_label(from, to, true),
        scope.whose_label(),
        material.prs.len(),
    ));
    let example = if several { "app#412" } else { "#412" };
    if everyone {
        text.push_str(&format!("{roster}\n\n## <Area> · <initials>, <initials>\n"));
        text.push_str(&format!("1. <what changed> · <owner> · {example}\n"));
    } else {
        text.push_str("## <Area>\n");
        text.push_str(&format!("1. <what changed> · {example}\n"));
    }
    text.push_str("   - Show: <what to do on screen, then what appears>\n   - Say: <the number or the consequence>\n");
    if everyone {
        text.push_str(&format!(
            "2. <what changed> · <owner> · {example}, {example}\n```\n\n"
        ));
        text.push_str("- The heading lists the initials of everyone who owns a point in the area, most points first, separated by a comma and a space.");
    } else {
        text.push_str(&format!("2. <what changed> · {example}, {example}\n```\n"));
    }
    text.push_str(BRIEF_CLOSING);
    text.push_str("\n## The pull requests merged in the period\n\nEach entry: reference · ");
    if everyone {
        text.push_str("author's initials · ");
    }
    text.push_str("size · branch · title, then the start of its description (more of it for a very large pull request).\n\n");
    for pr in &material.prs {
        let release = pr.is_release();
        let cut = if pr.additions >= BIG_ADDITIONS && !release {
            BIG_BODY_CUT
        } else {
            BODY_CUT
        };
        text.push_str(&pr.reference(several));
        if everyone {
            let owner = initials.get(pr.login.as_str()).copied().unwrap_or("??");
            text.push_str(&format!(" · {owner}"));
        }
        text.push_str(&format!(
            " · +{}/-{} in {} files · into {}{} · {}\n{}\n---\n",
            pr.additions,
            pr.deletions,
            pr.files,
            pr.base,
            if release { " · RELEASE" } else { "" },
            pr.title,
            clean(&pr.body, cut),
        ));
    }
    if !material.issues.is_empty() {
        text.push_str(ISSUES_INTRO);
        for issue in &material.issues {
            text.push_str(&format!("{} · {}", issue.identifier, issue.title));
            if everyone && !issue.assignee.is_empty() {
                text.push_str(&format!(" · {}", issue.assignee));
            }
            text.push_str(&format!(
                "\n{}\n---\n",
                clean(&issue.description, ISSUE_CUT)
            ));
        }
    }
    if !material.todos.is_empty() {
        text.push_str(TODOS_INTRO);
        for todo in &material.todos {
            text.push_str(&format!("- {}\n", todo.text.replace('\n', " ")));
        }
    }
    text.push_str(&format!(
        "\nWrite the review now: the Markdown and nothing else, starting with the `# {title}` line.\n"
    ));
    text
}

/// A description as the prompt carries it: bot footers and blank lines
/// gone, cut to `max` characters.
fn clean(body: &str, max: usize) -> String {
    let body = body.replace('\r', "");
    let end = ["<!--", "🤖 Generated with"]
        .iter()
        .filter_map(|mark| body.find(mark))
        .min()
        .unwrap_or(body.len());
    let lines: Vec<&str> = body[..end]
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines.join("\n").chars().take(max).collect()
}

/// The period a review of `days` written at `now` covers, as local days:
/// today and the days before it.
fn period(now: i64, days: i64) -> (NaiveDate, NaiveDate) {
    let to = Local
        .timestamp_opt(now, 0)
        .single()
        .map(|at| at.date_naive())
        .unwrap_or_default();
    (to - chrono::Duration::days(days - 1), to)
}

/// What a review of `range` to `to` is called on its row and its page:
/// `Week 45` — the week it ends in — `Weeks 44–45`, `Month to 5 Nov`.
fn review_name(range: Range, from: NaiveDate, to: NaiveDate) -> String {
    use chrono::Datelike;
    match range {
        Range::Week => format!("Week {}", to.iso_week().week()),
        Range::Fortnight => format!("Weeks {}–{}", from.iso_week().week(), to.iso_week().week()),
        Range::Month => format!("Month to {}", to.format("%-d %b")),
    }
}

/// `4 – 10 October`, or `27 September – 3 October` across a month's end;
/// the short form (`4 – 10 Oct`) for a row.
fn period_label(from: NaiveDate, to: NaiveDate, long: bool) -> String {
    let month = if long { "%B" } else { "%b" };
    if from.format("%Y-%m").to_string() == to.format("%Y-%m").to_string() {
        format!(
            "{} – {}",
            from.format("%-d"),
            to.format(&format!("%-d {month}"))
        )
    } else {
        format!(
            "{} – {}",
            from.format(&format!("%-d {month}")),
            to.format(&format!("%-d {month}"))
        )
    }
}

// ---- writing ----

/// The one line the model is given in the system prompt's place.
const SYSTEM: &str = "You are a careful writer. Output exactly what is asked for and nothing else: no preamble, no code fences, no closing remarks.";

/// The model and effort `preset` writes reviews at — sonnet · medium
/// where it names none.
fn launch_of(preset: &crate::agent_presets::AgentPreset) -> (String, String) {
    (
        preset.model.clone().unwrap_or_else(|| DEFAULT_MODEL.into()),
        preset
            .effort
            .clone()
            .unwrap_or_else(|| DEFAULT_EFFORT.into()),
    )
}

/// Send `prompt` to `model` at `effort` through `account`, as one
/// [`oneshot::run`] under the review's system prompt.
pub async fn write(
    prompt: String,
    model: String,
    effort: String,
    account: Account,
) -> Result<Reply, String> {
    oneshot::run(oneshot::Ask {
        prompt,
        system: SYSTEM.to_string(),
        account,
        model,
        effort,
        timeout: WRITE_TIMEOUT,
    })
    .await
}

// ---- checking ----

/// How many points one pull request may sit on: a feature-branch merge
/// is the one kind the brief lets repeat.
const POINTS_A_PR: usize = 3;

/// One numbered line of a review, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Point<'a> {
    number: &'a str,
    text: &'a str,
    owner: Option<&'a str>,
    refs: Vec<&'a str>,
}

/// Whether `word` reads as a pull request reference: `#412` or `app#412`.
fn is_reference(word: &str) -> bool {
    let Some((slug, number)) = word.split_once('#') else {
        return false;
    };
    !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Whether `word` reads as a person's initials: two or three capitals,
/// and a digit after them where two people shared a set.
fn is_initials(word: &str) -> bool {
    let letters = word.trim_end_matches(|c: char| c.is_ascii_digit());
    (2..=3).contains(&letters.chars().count()) && letters.chars().all(|c| c.is_uppercase())
}

/// `1. <what changed> · <owner> · #412, #436` taken apart; `None` for a
/// line that is not a point.
fn parse_point(line: &str) -> Option<Point<'_>> {
    let (number, rest) = line.split_once(". ")?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (rest, tail) = rest.rsplit_once(" · ")?;
    let refs: Vec<&str> = tail.split(", ").map(str::trim).collect();
    if !refs.iter().all(|r| is_reference(r)) {
        return None;
    }
    let (text, owner) = match rest.rsplit_once(" · ") {
        Some((text, owner)) if is_initials(owner) => (text, Some(owner)),
        _ => (rest, None),
    };
    Some(Point {
        number,
        text,
        owner,
        refs,
    })
}

/// `## <Area> · AL, GR` taken apart: the area, and who it names.
fn parse_heading(line: &str) -> Option<(&str, Vec<&str>)> {
    let rest = line.strip_prefix("## ")?;
    Some(match rest.rsplit_once(" · ") {
        Some((area, people)) if people.split(", ").all(is_initials) => {
            (area, people.split(", ").collect())
        }
        _ => (rest, Vec::new()),
    })
}

/// `   - Show: …` taken apart.
fn parse_sub(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_start().strip_prefix("- ")?;
    let (label, text) = rest.split_once(": ")?;
    matches!(label, "Show" | "Say").then_some((label, text))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `9 points`.
pub fn points_label(n: usize) -> String {
    plural(n, "point", "points")
}

/// Hold a reply to its shape, without a model. What comes back is one
/// sentence per kind of fault, for the line under the review's title:
/// a reference that is no pull request that was sent, or on more than
/// three points; an owner who is not the author of the point's first
/// pull request; a heading that names other people than its points do.
/// How long a line runs is the brief's to ask and the reader's to see: a
/// point a word over is no reason to doubt a review.
pub fn check(
    review: &str,
    material: &Material,
    people: &[(String, String, String)],
) -> Vec<String> {
    let several = material.several_projects();
    let authors: HashMap<String, &str> = material
        .prs
        .iter()
        .map(|pr| (pr.reference(several), pr.login.as_str()))
        .collect();
    let initials: HashMap<&str, &str> = people
        .iter()
        .map(|(login, initials, _)| (login.as_str(), initials.as_str()))
        .collect();
    let (mut unknown, mut wrong_owner, mut headings) = (0, 0, 0);
    let mut uses: HashMap<&str, usize> = HashMap::new();
    let mut area: Option<(Vec<&str>, HashSet<&str>)> = None;
    let mut close = |area: &mut Option<(Vec<&str>, HashSet<&str>)>| {
        if let Some((named, owners)) = area.take() {
            let named: HashSet<&str> = named.into_iter().collect();
            if !named.is_empty() && named != owners {
                headings += 1;
            }
        }
    };
    for line in review.lines() {
        if let Some((_, named)) = parse_heading(line) {
            close(&mut area);
            area = Some((named, HashSet::new()));
        } else if let Some(point) = parse_point(line) {
            for reference in &point.refs {
                *uses.entry(*reference).or_default() += 1;
                if !authors.contains_key(*reference) {
                    unknown += 1;
                }
            }
            if let Some(owner) = point.owner {
                let author = point.refs.first().and_then(|r| authors.get(*r));
                if author.is_some_and(|login| initials.get(login) != Some(&owner)) {
                    wrong_owner += 1;
                }
                if let Some((_, owners)) = &mut area {
                    owners.insert(owner);
                }
            }
        }
    }
    close(&mut area);
    let repeated = uses.values().filter(|n| **n > POINTS_A_PR).count();
    let mut problems = Vec::new();
    if unknown > 0 {
        problems.push(format!(
            "{} that {} not in the week",
            plural(unknown, "names a pull request", "name pull requests"),
            if unknown == 1 { "was" } else { "were" }
        ));
    }
    if repeated > 0 {
        problems.push(plural(
            repeated,
            "pull request is on more than three points",
            "pull requests are on more than three points",
        ));
    }
    if wrong_owner > 0 {
        problems.push(plural(
            wrong_owner,
            "point names the wrong owner",
            "points name the wrong owner",
        ));
    }
    if headings > 0 {
        problems.push(plural(
            headings,
            "area names people its points do not",
            "areas name people their points do not",
        ));
    }
    problems
}

/// A reply as it is kept: anything before the title and any code fence
/// the model wrapped it in gone, each area's points numbered from 1
/// whatever the model numbered them, and the line no model was asked to
/// count appended — how many pull requests were merged and are not
/// listed.
pub fn finish(reply: &str, material: &Material) -> String {
    let several = material.several_projects();
    let known: HashSet<String> = material
        .prs
        .iter()
        .map(|pr| pr.reference(several))
        .collect();
    // Whatever the model said before the title goes — unless it wrote no
    // title at all, when every line is kept rather than none.
    let titled = reply.lines().any(|line| line.starts_with("# "));
    let mut kept: Vec<&str> = reply
        .lines()
        .skip_while(|line| titled && !line.starts_with("# "))
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect();
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let listed: HashSet<&str> = kept
        .iter()
        .filter_map(|line| parse_point(line))
        .flat_map(|point| point.refs)
        .filter(|reference| known.contains(*reference))
        .collect();
    let mut number = 0;
    let lines: Vec<String> = kept
        .iter()
        .map(|line| {
            if line.starts_with("## ") {
                number = 0;
            }
            match (parse_point(line), line.split_once(". ")) {
                (Some(_), Some((_, rest))) => {
                    number += 1;
                    format!("{number}. {rest}")
                }
                // A sub-line sits under its point's text, however wide the
                // number the model gave it was.
                _ if parse_sub(line).is_some() => format!("   {}", line.trim_start()),
                _ => line.to_string(),
            }
        })
        .collect();
    let mut text = lines.join("\n");
    let unlisted = known.len().saturating_sub(listed.len());
    if unlisted > 0 {
        text.push_str(&format!(
            "\n\n+ {} merged, not listed",
            plural(unlisted, "other pull request", "other pull requests")
        ));
    }
    text.push('\n');
    text
}

// ---- what is kept ----

/// The run's own file in its folder, beside each project's Markdown.
const META: &str = "meta.json";

/// One project's review in a run, as the run's [`Meta`] keeps it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// The project's name: its tab's.
    pub project: String,
    /// Its Markdown's name in the run's folder, before `.md`.
    pub slug: String,
    /// How many points it makes.
    pub points: usize,
    /// [`check`]'s sentences, when the reply did not hold its shape.
    pub problems: Vec<String>,
    pub secs: u64,
    pub cost: Option<f64>,
    /// What it was written from: kept on disk, and never read back — a
    /// dozen runs' worth of it would ride every frame for nothing.
    #[serde(skip_deserializing)]
    pub material: Material,
}

/// A run, kept as `meta.json` in its folder
/// (`reviews/all/<to>/`) beside a `<project-slug>.md` for each project
/// that had changes: when and whose, what wrote it, and each review.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    /// The period's first and last days, `YYYY-MM-DD`.
    pub from: String,
    pub to: String,
    pub scope: Scope,
    /// Unix seconds.
    pub written_at: i64,
    /// The Claude account it was written through, as it was called then.
    pub account: String,
    pub model: String,
    pub effort: String,
    /// One a project, in the run's order.
    pub reviews: Vec<Kept>,
    /// The projects with changes whose review could not be written, and
    /// why: `(name, why)`.
    pub failed: Vec<(String, String)>,
    /// Shown once already: no longer `new`.
    pub read: bool,
}

impl Meta {
    fn days(&self) -> Option<(NaiveDate, NaiveDate)> {
        let day = |text: &str| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok();
        Some((day(&self.from)?, day(&self.to)?))
    }

    /// `4 – 10 Oct`.
    pub fn period(&self) -> String {
        match self.days() {
            Some((from, to)) => period_label(from, to, false),
            None => self.to.clone(),
        }
    }

    /// `Week 41` ([`review_name`]).
    pub fn name(&self) -> String {
        match self.days() {
            Some((from, to)) => review_name(self.scope.range, from, to),
            None => self.to.clone(),
        }
    }

    /// Every review's points, added up.
    pub fn points(&self) -> usize {
        self.reviews.iter().map(|r| r.points).sum()
    }
}

/// A run on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub dir: PathBuf,
    pub meta: Meta,
}

impl Saved {
    /// The Markdown of its `index`th review.
    fn path(&self, index: usize) -> Option<PathBuf> {
        let kept = self.meta.reviews.get(index)?;
        Some(self.dir.join(format!("{}.md", kept.slug)))
    }
}

/// The folder every run is kept in, one folder a run.
fn runs_dir(root: &Path) -> PathBuf {
    root.join("all")
}

/// Every run kept, newest first. What older releases kept — a review's
/// Markdown loose in `all/`, and the `p-<project>` folders — stays where
/// it is, and is not listed.
pub fn load_runs(root: &Path) -> Vec<Saved> {
    let Ok(entries) = std::fs::read_dir(runs_dir(root)) else {
        return Vec::new();
    };
    let mut runs: Vec<Saved> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| dir.is_dir())
        .filter_map(|dir| {
            let raw = std::fs::read_to_string(dir.join(META)).ok()?;
            let meta = serde_json::from_str::<Meta>(&raw).ok()?;
            Some(Saved { dir, meta })
        })
        .collect();
    runs.sort_by(|a, b| (&b.meta.to, b.meta.written_at).cmp(&(&a.meta.to, a.meta.written_at)));
    runs
}

/// The newest [`KEEP`] runs kept; the older ones' folders go.
fn prune(root: &Path) {
    for old in load_runs(root).into_iter().skip(KEEP) {
        let _ = std::fs::remove_dir_all(old.dir);
    }
}

/// How many numbered points a review makes.
fn count_points(text: &str) -> usize {
    text.lines().filter_map(parse_point).count()
}

// ---- the app's side ----

/// What one project's fetch came back with, and when.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub at: Instant,
    /// `None`: GitHub could not be asked.
    pub prs: Option<Vec<MergedPr>>,
}

/// What one fetch covers: a project, and how many days back.
pub type Window = (ProjectId, i64);

/// Where one project's review in the run being written is.
#[derive(Debug, Clone, PartialEq)]
pub enum PartState {
    Writing,
    Done { text: String, problems: Vec<String> },
    Failed(String),
}

/// One project's review in the run being written.
#[derive(Debug)]
pub struct Part {
    pub project: ProjectId,
    pub name: String,
    /// Its Markdown's name in the run's folder ([`project_slug`]).
    pub slug: String,
    pub material: Material,
    pub people: Vec<(String, String, String)>,
    pub state: PartState,
    pub secs: u64,
    pub cost: Option<f64>,
}

/// The run being written: one review a project that had changes.
#[derive(Debug)]
pub struct Run {
    /// Told apart from one stopped or started since.
    pub id: u64,
    pub scope: Scope,
    /// Unix seconds when it was asked: what dates it.
    pub asked: i64,
    pub started: Instant,
    /// GitHub is still being asked what merged: nothing is written yet.
    pub fetching: bool,
    /// Every project with changes, once GitHub has answered.
    pub parts: Vec<Part>,
    /// Who writes it, and at what.
    pub account: Account,
    pub model: String,
    pub effort: String,
    /// The preset's rules, the extra steer after them ([`crate::automations::compose`]).
    pub rules: String,
    /// The writes, for `⌘W` to stop.
    pub tasks: Vec<tokio::task::AbortHandle>,
    /// Its folder has been emptied of a run of the same day it replaces.
    cleared: bool,
}

impl Run {
    /// `Week 45` ([`review_name`]).
    pub fn name(&self) -> String {
        let (from, to) = self.scope.period(self.asked);
        review_name(self.scope.range, from, to)
    }

    /// How many of its reviews have come back, written or not, and of how
    /// many.
    pub fn progress(&self) -> (usize, usize) {
        let back = self
            .parts
            .iter()
            .filter(|p| p.state != PartState::Writing)
            .count();
        (back, self.parts.len())
    }

    /// Its folder: a second run to the same day takes the first's place.
    fn dir(&self, root: &Path) -> PathBuf {
        let (_, to) = self.scope.period(self.asked);
        runs_dir(root).join(to.format("%Y-%m-%d").to_string())
    }

    /// What its `meta.json` says, as far as it has got.
    fn meta(&self) -> Meta {
        let (from, to) = self.scope.period(self.asked);
        let mut meta = Meta {
            from: from.format("%Y-%m-%d").to_string(),
            to: to.format("%Y-%m-%d").to_string(),
            scope: self.scope,
            written_at: self.asked,
            account: self.account.label.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            ..Meta::default()
        };
        for part in &self.parts {
            match &part.state {
                PartState::Writing => {}
                PartState::Done { text, problems } => meta.reviews.push(Kept {
                    project: part.name.clone(),
                    slug: part.slug.clone(),
                    points: count_points(text),
                    problems: problems.clone(),
                    secs: part.secs,
                    cost: part.cost,
                    material: part.material.clone(),
                }),
                PartState::Failed(why) => meta.failed.push((part.name.clone(), why.clone())),
            }
        }
        meta
    }
}

/// An answer back on the loop.
#[derive(Debug)]
pub enum Answer {
    Fetched {
        window: Window,
        prs: Option<Vec<MergedPr>>,
    },
    Written {
        run: u64,
        part: usize,
        result: Result<Reply, String>,
        secs: u64,
    },
}

/// The WEEK IN REVIEW's state on the [`App`]: it outlives every screen,
/// so closing one loses neither a fetch nor a write.
#[derive(Debug, Default)]
pub struct State {
    /// `<DATA DIR>/reviews`. `None` — every unit test that sets none —
    /// keeps nothing.
    pub root: Option<PathBuf>,
    /// Where answers go. `None` — every unit test — starts nothing.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    pub fetched: HashMap<Window, Fetched>,
    pub fetching: HashSet<Window>,
    /// The run being written.
    pub run: Option<Run>,
    /// The runs kept, newest first, as of the last one kept ([`reload`]).
    pub kept: Vec<Saved>,
    next_id: u64,
}

/// The runs kept, read again.
pub fn reload(app: &mut App) {
    app.week_review.kept = app
        .week_review
        .root
        .as_deref()
        .map(load_runs)
        .unwrap_or_default();
}

/// Ask for `project`'s last `days` of merged pull requests, unless that
/// fetch is out or a fresh one is in hand.
fn request_fetch(app: &mut App, project: &ProjectId, name: &str, dir: &Path, days: i64) {
    let state = &mut app.week_review;
    let window = (project.clone(), days);
    let fresh = state
        .fetched
        .get(&window)
        .is_some_and(|f| f.prs.is_some() && f.at.elapsed() < FRESH);
    let Some(tx) = state.tx.clone() else {
        return;
    };
    if fresh || !state.fetching.insert(window.clone()) {
        return;
    }
    let (name, dir) = (name.to_string(), dir.to_path_buf());
    let now = orion_core::clock::now_secs() as i64;
    tokio::spawn(async move {
        let prs = fetch_merged(dir, name, now, days).await;
        let _ = tx.send(Answer::Fetched { window, prs });
    });
}

/// The todos in `items` ticked since `since` (secs), into `material`.
fn push_ticked<'a>(
    material: &mut Material,
    items: impl Iterator<Item = &'a crate::todos::Item>,
    since: i64,
) {
    for item in items {
        let Some(done_at) = item.done.map(|at| at.timestamp()) else {
            continue;
        };
        if done_at >= since {
            material.todos.push(DoneTodo {
                text: item.text.clone(),
                done_at,
            });
        }
    }
}

/// Ask for every project's week: its merged pull requests, and the lists
/// a review reads beside them — Linear's, where the project has a key, and
/// its todos. A fetch gone stale is let go first: descriptions are heavy.
fn request_all(app: &mut App) {
    app.week_review
        .fetched
        .retain(|_, fetched| fetched.at.elapsed() < FRESH);
    crate::todos::view::load_list(app, crate::todos::store::blue_sky_key());
    let projects = every_project(app);
    for (id, name, dir) in projects {
        request_fetch(app, &id, &name, &dir, DAYS);
        crate::todos::view::load_list(app, dir.clone());
        if !app.linear.contains_key(&id) && crate::linear::key_source(&dir).is_some() {
            crate::linear::request_list(app, id, dir, false);
        }
    }
}

/// Every project: `(id, name, checkout)`.
fn every_project(app: &App) -> Vec<(ProjectId, String, PathBuf)> {
    app.tree
        .projects
        .iter()
        .map(|p| (p.id.clone(), p.name.clone(), p.repo_path.clone()))
        .collect()
}

/// Whether GitHub is still being asked about any project's week.
fn still_fetching(app: &App) -> bool {
    app.tree
        .projects
        .iter()
        .any(|p| app.week_review.fetching.contains(&(p.id.clone(), DAYS)))
}

/// Every project that had changes in the week before `now`, each with
/// what its review is written from; a project with none is left out. A
/// project GitHub could not be asked about is not written up — a review
/// of part of a week would pass it off as the whole — and says so, unless
/// it had nothing else finished either.
pub fn split(app: &App, scope: Scope, now: i64) -> Vec<Part> {
    let mut slugs = HashSet::new();
    let mut parts = Vec::new();
    for (id, name, _) in every_project(app) {
        let asked = app
            .week_review
            .fetched
            .get(&(id.clone(), scope.range.days()))
            .is_some_and(|f| f.prs.is_some());
        let material = gather(app, &id, scope, [asked, true, true], now);
        if material.is_empty() {
            continue;
        }
        let stem = match project_slug(&name) {
            slug if slug.is_empty() => "project".to_string(),
            slug => slug,
        };
        let mut slug = stem.clone();
        let mut n = 2;
        while !slugs.insert(slug.clone()) {
            slug = format!("{stem}-{n}");
            n += 1;
        }
        let people = match scope.whose {
            Whose::Everyone => legend(&material.prs),
            Whose::Mine => Vec::new(),
        };
        let state = match asked {
            true => PartState::Writing,
            false => PartState::Failed("GitHub could not be asked what merged".into()),
        };
        parts.push(Part {
            project: id,
            name,
            slug,
            material,
            people,
            state,
            secs: 0,
            cost: None,
        });
    }
    parts
}

/// Start a run: every project's week asked for, then — as GitHub answers
/// ([`advance`]) — a review written for each one that had changes. The
/// **Week in review** automation's preset and extra steer say how, its
/// **Whose** whose. Why not, when it can't start.
pub fn run_now(app: &mut App) -> Result<(), String> {
    use crate::automations::{config::Whose as Whom, Automation};
    if app.week_review.run.is_some() {
        return Err("a week in review is already being written".into());
    }
    let which = Automation::WeekReview;
    let own = app.automations.settings.of(which);
    let preset = crate::automations::preset_of(&app.automations.settings, which);
    let cfg = crate::config::Config::load();
    let through = oneshot::account_for(&cfg, &preset);
    // A unit test starts nothing, so it needs no account.
    if app.week_review.tx.is_some() {
        if orion_core::env::non_empty(orion_core::env::AGENT_CMD).is_some() {
            return Err("off while ORION_AGENT_CMD stands in for the agents".into());
        }
        let Some(through) = &through else {
            return Err("no Claude account is switched on — Settings → Accounts, or Claude accounts in the palette".into());
        };
        if !crate::config::program_installed(&through.program) {
            return Err(format!("{} is not on your PATH", through.program));
        }
    }
    let scope = Scope {
        whose: match own.whose {
            Whom::JustMe => Whose::Mine,
            Whom::WholeTeam => Whose::Everyone,
        },
        ..Scope::default()
    };
    let (model, effort) = launch_of(&preset);
    request_all(app);
    app.week_review.next_id += 1;
    app.week_review.run = Some(Run {
        id: app.week_review.next_id,
        scope,
        asked: orion_core::clock::now_secs() as i64,
        started: Instant::now(),
        fetching: true,
        parts: Vec::new(),
        account: through.unwrap_or_default(),
        model,
        effort,
        rules: crate::automations::compose(&preset, "", &own.extra_steer),
        tasks: Vec::new(),
        cleared: false,
    });
    app.flash = Some(Flash::working("Week in review: writing…"));
    advance(app);
    app.dirty = true;
    Ok(())
}

/// The run moves on once GitHub has answered about every project: each
/// project with changes has its review written, [`AT_ONCE`] at a time.
fn advance(app: &mut App) {
    let waiting = app.week_review.run.as_ref().is_some_and(|r| r.fetching);
    if !waiting || still_fetching(app) {
        return;
    }
    let Some((scope, asked)) = app.week_review.run.as_ref().map(|r| (r.scope, r.asked)) else {
        return;
    };
    let parts = split(app, scope, asked);
    let tx = app.week_review.tx.clone();
    let Some(run) = app.week_review.run.as_mut() else {
        return;
    };
    run.fetching = false;
    run.parts = parts;
    if run.parts.is_empty() {
        app.week_review.run = None;
        app.flash = Some(Flash::note(format!(
            "Week in review: nothing finished in the last {DAYS} days to write up"
        )));
        return;
    }
    if let Some(tx) = tx {
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
        for (index, part) in run.parts.iter().enumerate() {
            if part.state != PartState::Writing {
                continue;
            }
            let text = prompt(
                &part.material,
                &part.people,
                run.scope,
                &part.name,
                &run.rules,
                run.asked,
            );
            let (model, effort, account) =
                (run.model.clone(), run.effort.clone(), run.account.clone());
            let (gate, tx, id) = (gate.clone(), tx.clone(), run.id);
            let task = tokio::spawn(async move {
                let _turn = gate.acquire_owned().await;
                let started = Instant::now();
                let result = write(text, model, effort, account).await;
                let secs = started.elapsed().as_secs();
                let _ = tx.send(Answer::Written {
                    run: id,
                    part: index,
                    result,
                    secs,
                });
            });
            run.tasks.push(task.abort_handle());
        }
    }
    // Every project with changes may have failed already.
    finish_run(app);
}

/// An answer landed on the loop.
pub fn land_answer(app: &mut App, answer: Answer) {
    match answer {
        Answer::Fetched { window, prs } => {
            app.week_review.fetching.remove(&window);
            app.week_review.fetched.insert(
                window,
                Fetched {
                    at: Instant::now(),
                    prs,
                },
            );
            advance(app);
        }
        Answer::Written {
            run,
            part,
            result,
            secs,
        } => land_written(app, run, part, result, secs),
    }
    app.dirty = true;
}

/// One project's review came back: checked, finished and kept — or why
/// not — and the run finished once it was the last.
pub fn land_written(
    app: &mut App,
    id: u64,
    index: usize,
    result: Result<Reply, String>,
    secs: u64,
) {
    let Some(run) = app.week_review.run.as_mut().filter(|r| r.id == id) else {
        return;
    };
    let Some(part) = run.parts.get_mut(index) else {
        return;
    };
    part.secs = secs;
    part.state = match result {
        Ok(reply) => {
            part.cost = reply.cost;
            PartState::Done {
                problems: check(&reply.text, &part.material, &part.people),
                text: finish(&reply.text, &part.material),
            }
        }
        Err(why) => PartState::Failed(why),
    };
    keep(app, index);
    finish_run(app);
}

/// The run's `index`th review on disk, and its `meta.json` as far as the
/// run has got: a run written over another of the same day empties that
/// one's folder first.
fn keep(app: &mut App, index: usize) {
    let Some(root) = app.week_review.root.clone() else {
        return;
    };
    let Some(run) = app.week_review.run.as_mut() else {
        return;
    };
    let dir = run.dir(&root);
    if !std::mem::replace(&mut run.cleared, true) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let kept = (|| {
        std::fs::create_dir_all(&dir)?;
        if let Some(Part {
            slug,
            state: PartState::Done { text, .. },
            ..
        }) = run.parts.get(index)
        {
            orion_core::settings::write_atomic(&dir.join(format!("{slug}.md")), text.as_bytes())?;
        }
        crate::pr_cache::write_json_atomic(&dir.join(META), &run.meta())
    })();
    if let Err(err) = kept {
        tracing::warn!("week in review not kept: {err}");
    }
}

/// Once every review of the run is back: the run is over, the oldest past
/// [`KEEP`] go, and the footer says how it went. A page on the run as it
/// was written turns to it as kept.
fn finish_run(app: &mut App) {
    let over = app
        .week_review
        .run
        .as_ref()
        .is_some_and(|r| !r.fetching && r.parts.iter().all(|p| p.state != PartState::Writing));
    if !over {
        return;
    }
    let Some(run) = app.week_review.run.take() else {
        return;
    };
    let failed: Vec<&str> = run
        .parts
        .iter()
        .filter_map(|p| match &p.state {
            PartState::Failed(why) => Some(why.as_str()),
            _ => None,
        })
        .collect();
    let written = run.parts.len() - failed.len();
    if let Some(root) = app.week_review.root.clone() {
        if written == 0 {
            let _ = std::fs::remove_dir_all(run.dir(&root));
        }
        prune(&root);
    }
    reload(app);
    app.flash = Some(match (written, failed.first()) {
        (0, Some(why)) => Flash::failed(format!("Week in review: {why}")),
        (_, None) => Flash::done("Week in review ready · ⌘J"),
        (_, Some(_)) => Flash::done(format!(
            "Week in review ready · {} couldn't be written",
            failed.len()
        )),
    });
    let live_page = matches!(&app.overlay, Some(Overlay::WeekReview(v)) if v.dir.is_none());
    if live_page {
        let (tab, back) = match &app.overlay {
            Some(Overlay::WeekReview(v)) => (v.tab, v.back),
            _ => (0, false),
        };
        let name = run.parts.get(tab).map(|p| p.name.clone());
        if written == 0 {
            close(app);
            return;
        }
        open(app, 0, back);
        if let Some(Overlay::WeekReview(view)) = &mut app.overlay {
            let at = view
                .meta
                .reviews
                .iter()
                .position(|r| Some(&r.project) == name.as_ref());
            view.tab = at.unwrap_or(0);
        }
    }
}

/// `⌘W` on the run being written: what is still being written is
/// stopped, and what came back is kept.
fn stop(app: &mut App) {
    let Some(run) = app.week_review.run.as_mut() else {
        return;
    };
    for task in run.tasks.drain(..) {
        task.abort();
    }
    run.fetching = false;
    for part in &mut run.parts {
        if part.state == PartState::Writing {
            part.state = PartState::Failed("stopped".into());
        }
    }
    if run.parts.is_empty() {
        app.week_review.run = None;
        app.flash = Some(Flash::note("Week in review stopped"));
        close(app);
        return;
    }
    finish_run(app);
    app.flash = Some(Flash::note("Week in review stopped"));
}

// ---- the page ----

/// A run on a page of its own, a tab per project it wrote up: the one
/// being written, or one kept. Opened from the Automations screen's
/// REVIEWS, it goes back there.
#[derive(Debug, Clone, Default)]
pub struct WeekReviewView {
    /// The kept run on the page, by its folder; `None` is the run being
    /// written.
    pub dir: Option<PathBuf>,
    /// The kept run's, and each of its reviews' Markdown in tab order.
    pub meta: Meta,
    pub texts: Vec<String>,
    /// The tab with the page.
    pub tab: usize,
    /// Top visible line of the page.
    pub scroll: u16,
    pub view_height: u16,
    pub body_lines: usize,
    /// Esc goes back to the Automations screen.
    pub back: bool,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// As of the last draw, for the mouse: the tabs' row, and each tab's
    /// cells along it.
    pub tab_row: u16,
    pub tab_hits: Vec<(u16, u16)>,
}

impl WeekReviewView {
    fn max_scroll(&self) -> u16 {
        crate::app::max_scroll(self.body_lines, self.view_height)
    }

    fn scroll_by(&mut self, delta: i32) {
        self.scroll = crate::app::scrolled_by(self.scroll, delta, self.max_scroll());
    }

    /// Whether a `Show` is on screen to sweep: what keeps the frame clock
    /// running while the page is up (`App::status_anim_active`).
    pub fn shimmers(&self) -> bool {
        self.texts
            .get(self.tab)
            .is_some_and(|text| text.contains("- Show: "))
    }
}

/// The REVIEWS list's `index`th run on its page — the one being written
/// first, while there is one, then the ones kept — on its first tab.
/// `back`: Esc goes back to the Automations screen.
pub fn open(app: &mut App, index: usize, back: bool) {
    let live = usize::from(app.week_review.run.is_some());
    let mut view = WeekReviewView {
        back,
        ..WeekReviewView::default()
    };
    if index >= live {
        let Some(saved) = app.week_review.kept.get(index - live) else {
            return;
        };
        view.texts = (0..saved.meta.reviews.len())
            .map(|i| {
                saved
                    .path(i)
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .unwrap_or_default()
            })
            .collect();
        view.dir = Some(saved.dir.clone());
        view.meta = saved.meta.clone();
    }
    app.overlay = Some(Overlay::WeekReview(Box::new(view)));
    mark_read(app);
    app.dirty = true;
}

/// The page closed: back to the Automations screen when it came from
/// there.
fn close(app: &mut App) {
    let back = matches!(&app.overlay, Some(Overlay::WeekReview(v)) if v.back);
    app.overlay = None;
    if back {
        crate::automations::view::open(app);
    }
}

/// The kept run on the page has been shown: it stops reading `new`, on
/// disk too.
fn mark_read(app: &mut App) {
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    let Some(dir) = view.dir.clone().filter(|_| !view.meta.read) else {
        return;
    };
    view.meta.read = true;
    if let Some(saved) = app.week_review.kept.iter_mut().find(|s| s.dir == dir) {
        saved.meta.read = true;
    }
    // Only the flag moves on disk: what the reviews were written from is
    // there and not in hand.
    let json = dir.join(META);
    let kept = std::fs::read_to_string(&json)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    if let Some(mut kept) = kept {
        kept["read"] = serde_json::Value::Bool(true);
        let _ = crate::pr_cache::write_json_atomic(&json, &kept);
    }
}

/// The page's tabs: the projects of the run on it.
fn tab_names(app: &App, view: &WeekReviewView) -> Vec<String> {
    match (&view.dir, &app.week_review.run) {
        (Some(_), _) => view
            .meta
            .reviews
            .iter()
            .map(|r| r.project.clone())
            .collect(),
        (None, Some(run)) => run.parts.iter().map(|p| p.name.clone()).collect(),
        (None, None) => Vec::new(),
    }
}

/// The tab `at`, the page on its top.
fn go_tab(app: &mut App, at: usize) {
    let n = match &app.overlay {
        Some(Overlay::WeekReview(view)) => tab_names(app, view).len(),
        _ => return,
    };
    if let Some(Overlay::WeekReview(view)) = &mut app.overlay {
        if at < n && at != view.tab {
            view.tab = at;
            view.scroll = 0;
        }
    }
}

/// The tab `delta` over, round the ends.
fn step_tab(app: &mut App, delta: i32) {
    let (n, tab) = match &app.overlay {
        Some(Overlay::WeekReview(view)) => (tab_names(app, view).len(), view.tab),
        _ => return,
    };
    if n > 0 {
        go_tab(app, (tab as i32 + delta).rem_euclid(n as i32) as usize);
    }
}

/// The page's own keys: one table [`handle_key`] matches and [`hints`]
/// spells.
pub(crate) mod keys {
    use crate::hints::Key;

    /// A project's tab and the next, as the TODOS MODAL walks its own.
    pub const TABS: Key = crate::nav::TABS.also(&["[", "]"]).doing("projects");
    pub const SCROLL: Key = crate::nav::ROWS.doing("scroll");
    /// The TODOS MODAL's copy.
    pub const COPY: Key = Key::new(&["cmd+c", "ctrl+y"], "copy as Markdown");
    pub const FILE: Key = Key::new(&["cmd+o", "ctrl+o"], "open the file");
    pub const STOP: Key = Key::new(&["cmd+w", "ctrl+w"], "stop");
    pub const BACK: Key = crate::nav::CLOSE.doing("back");
    #[cfg(test)]
    pub const ALL: &[Key] = &[TABS, SCROLL, COPY, FILE, STOP, BACK];
}

/// What the page's bottom border says its keys are.
fn hints(view: &WeekReviewView) -> Vec<Hint> {
    let mut hints = match view.dir {
        None => vec![keys::STOP.hint().kept()],
        Some(_) => vec![keys::COPY.hint().kept(), keys::FILE.hint()],
    };
    hints.extend([keys::TABS.hint(), keys::SCROLL.hint(), keys::BACK.hint()]);
    hints
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    let page = i32::from(view.view_height.max(1));
    match key.code {
        _ if keys::BACK.matches(&key) => close(app),
        _ if keys::TABS.matches(&key) => {
            let back = matches!(key.code, KeyCode::Left | KeyCode::Char('['));
            step_tab(app, if back { -1 } else { 1 });
        }
        KeyCode::Down => view.scroll_by(1),
        KeyCode::Up => view.scroll_by(-1),
        KeyCode::PageDown => view.scroll_by(page),
        KeyCode::PageUp => view.scroll_by(-page),
        KeyCode::Home => view.scroll = 0,
        KeyCode::End => view.scroll = view.max_scroll(),
        _ if keys::STOP.matches(&key) && view.dir.is_none() => stop(app),
        _ if keys::COPY.matches(&key) => {
            if let Some(text) = view.texts.get(view.tab).filter(|t| !t.is_empty()) {
                let text = text.clone();
                crate::event_loop::copy_and_flash(app, &text, "Copied the review as Markdown");
            }
        }
        _ if keys::FILE.matches(&key) => {
            let saved = app
                .week_review
                .kept
                .iter()
                .find(|s| Some(&s.dir) == view.dir.as_ref());
            let file = saved
                .and_then(|s| s.path(view.tab))
                .and_then(|p| p.file_name()?.to_str().map(str::to_string));
            if let (Some(dir), Some(file)) = (view.dir.clone(), file) {
                crate::event_loop::open_file_outside(app, &dir, &file, 0);
            }
        }
        _ => {}
    }
}

/// The wheel scrolls the page; a click on a tab is the keys landing on
/// it (INPUT PARITY). A click outside is Esc, as on every modal
/// (`overlay_close`).
pub fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollDown => view.scroll_by(3),
        MouseEventKind::ScrollUp => view.scroll_by(-3),
        MouseEventKind::Down(MouseButton::Left) if mouse.row == view.tab_row => {
            if let Some(at) = crate::ui::tab_hit(&view.tab_hits, mouse.column) {
                go_tab(app, at);
            }
        }
        _ => {}
    }
    app.dirty = true;
}

// ---- drawing ----

/// The gap between a review's columns.
const GAP: usize = 2;

/// `text` wrapped to `width`, never narrower than a few words.
fn wrap(text: &str, width: usize) -> Vec<String> {
    crate::pr_preview::wrap(text, width.max(8))
}

/// A review as its page draws it. A point's owner and its pull
/// requests sit in two columns at the right, each right-aligned; an
/// area's contributors END on the owner column, so one contributor sits
/// exactly over its points' owner and a longer list grows leftwards.
/// `Show` sweeps in green while `phase` runs (the animations setting).
fn review_lines(
    text: &str,
    problems: &[String],
    width: usize,
    phase: Option<usize>,
    th: Theme,
) -> Vec<Line<'static>> {
    let points: Vec<Point> = text.lines().filter_map(parse_point).collect();
    let owner_w = points
        .iter()
        .filter_map(|p| p.owner)
        .map(|o| o.chars().count())
        .max()
        .unwrap_or(0);
    let refs_w = points
        .iter()
        .map(|p| p.refs.join(", ").chars().count())
        .max()
        .unwrap_or(0);
    let owner_col = if owner_w > 0 { owner_w + GAP } else { 0 };
    // The columns give way before the text does on a narrow panel.
    let right = (owner_col + refs_w + GAP).min(width / 2);
    let text_w = width.saturating_sub(right);
    let pad = |n: usize| Span::raw(" ".repeat(n));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut seen_area = false;
    for raw in text.lines() {
        if let Some(title) = raw.strip_prefix("# ") {
            lines.push(Line::from(Span::styled(
                title.to_string(),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            )));
        } else if let Some((area, people)) = parse_heading(raw) {
            seen_area = true;
            let people = people.join(", ");
            let name = truncate(area, text_w.saturating_sub(people.chars().count() + 1));
            let used = name.chars().count() + people.chars().count();
            // The contributors' last letter lands where an owner's does.
            let ends_at = width.saturating_sub(refs_w + if refs_w > 0 { GAP } else { 0 });
            lines.push(Line::default());
            lines.push(Line::from(vec![
                Span::styled(
                    name,
                    Style::default().fg(th.text).add_modifier(Modifier::BOLD),
                ),
                pad(ends_at.saturating_sub(used)),
                Span::styled(people, Style::default().fg(th.muted)),
            ]));
            lines.push(Line::from(Span::styled(
                "─".repeat(width),
                Style::default().fg(th.edge),
            )));
        } else if let Some(point) = parse_point(raw) {
            let lead = format!("{}. ", point.number);
            let lead_w = lead.chars().count();
            let wrapped = wrap(point.text, text_w.saturating_sub(lead_w + 1));
            for (i, part) in wrapped.iter().enumerate() {
                let mut spans = if i == 0 {
                    vec![Span::styled(lead.clone(), Style::default().fg(th.muted))]
                } else {
                    vec![pad(lead_w)]
                };
                spans.push(Span::styled(part.clone(), Style::default().fg(th.text)));
                if i == 0 {
                    let used = lead_w + part.chars().count();
                    let refs = point.refs.join(", ");
                    let owner = point.owner.unwrap_or_default();
                    let tail = owner_col + refs.chars().count().max(refs_w);
                    spans.push(pad(width.saturating_sub(used + tail)));
                    if owner_w > 0 {
                        spans.push(Span::styled(
                            format!("{owner:>owner_w$}"),
                            Style::default().fg(th.muted),
                        ));
                        spans.push(pad(GAP));
                    }
                    spans.push(Span::styled(
                        format!("{refs:>refs_w$}"),
                        Style::default().fg(th.merged),
                    ));
                }
                lines.push(Line::from(spans));
            }
        } else if let Some((label, body)) = parse_sub(raw) {
            const INDENT: usize = 3;
            const LABEL_W: usize = 5;
            let label_spans = match (label, phase) {
                ("Show", Some(phase)) => crate::ui::sweep_spans(
                    "Show",
                    Style::default().add_modifier(Modifier::BOLD),
                    [th.ok, th.ok, th.text],
                    phase,
                ),
                ("Show", None) => vec![Span::styled(
                    "Show",
                    Style::default().fg(th.ok).add_modifier(Modifier::BOLD),
                )],
                _ => vec![Span::styled(label.to_string(), Style::default().fg(th.dim))],
            };
            let wrapped = wrap(body, width.saturating_sub(INDENT + LABEL_W));
            for (i, part) in wrapped.iter().enumerate() {
                let mut spans = vec![pad(INDENT)];
                if i == 0 {
                    spans.extend(label_spans.clone());
                    spans.push(pad(LABEL_W - label.chars().count()));
                } else {
                    spans.push(pad(LABEL_W));
                }
                spans.push(Span::styled(part.clone(), Style::default().fg(th.muted)));
                lines.push(Line::from(spans));
            }
        } else if raw.starts_with('+') {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(th.dim),
            )));
        } else if !raw.trim().is_empty() && !seen_area {
            // The tally and the legend, under the title.
            for part in wrap(raw, width) {
                lines.push(Line::from(Span::styled(
                    part,
                    Style::default().fg(th.muted),
                )));
            }
        }
    }
    if !problems.is_empty() {
        let at = lines.len().min(1);
        let note = format!("Check before you read it out: {}.", problems.join("; "));
        for (i, part) in wrap(&note, width).into_iter().enumerate() {
            lines.insert(
                at + i,
                Line::from(Span::styled(part, Style::default().fg(th.warn))),
            );
        }
    }
    lines
}

/// One row of what a review is written from: a glyph, a reference in a
/// column, the title, and at the right a word and how long ago.
#[allow(clippy::too_many_arguments)]
fn item_line(
    glyph: (&'static str, ratatui::style::Color),
    reference: &str,
    ref_w: usize,
    title: &str,
    word: (&str, ratatui::style::Color),
    age: &str,
    width: usize,
    th: Theme,
) -> Line<'static> {
    let right = format!("{}  {age:>3}", word.0);
    let right_w = right.chars().count();
    // Two cells in, under the rule's name.
    let lead = 2 + 2 + ref_w + if ref_w > 0 { 2 } else { 0 };
    let room = width.saturating_sub(lead + right_w + 2);
    let title = truncate(title, room);
    let used = lead + title.chars().count();
    let mut spans = vec![Span::styled(
        format!("  {} ", glyph.0),
        Style::default().fg(glyph.1),
    )];
    if ref_w > 0 {
        spans.push(Span::styled(
            format!("{reference:<ref_w$}  "),
            Style::default().fg(th.dim),
        ));
    }
    spans.push(Span::styled(title, Style::default().fg(th.text)));
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used + right_w))));
    spans.push(Span::styled(
        word.0.to_string(),
        Style::default().fg(word.1),
    ));
    spans.push(Span::styled(
        format!("  {age:>3}"),
        Style::default().fg(th.dim),
    ));
    Line::from(spans)
}

/// What a review is written from, source by source under its rule: every
/// item, not only the counts.
fn item_lines(material: &Material, now: i64, width: usize, th: Theme) -> Vec<Line<'static>> {
    let several = material.several_projects();
    let ago = |at: i64| crate::hosts::ago_short(now - at);
    let rule = |name: &str, count: usize| {
        crate::ui::section_rule(
            name,
            th.muted,
            vec![Span::styled(count.to_string(), Style::default().fg(th.dim))],
            width,
            th,
        )
    };
    let mut lines = Vec::new();
    if !material.prs.is_empty() {
        lines.push(rule(Source::Prs.label(), material.prs.len()));
        let refs: Vec<String> = material
            .prs
            .iter()
            .map(|pr| pr.reference(several))
            .collect();
        let ref_w = refs.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        for (pr, reference) in material.prs.iter().zip(&refs) {
            let age = rfc3339_secs(&pr.merged_at).map(ago).unwrap_or_default();
            lines.push(item_line(
                ("↗", th.merged),
                reference,
                ref_w,
                &pr.title,
                (pr.base.as_str(), th.merged),
                &age,
                width,
                th,
            ));
        }
    }
    if !material.issues.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(rule(Source::Issues.label(), material.issues.len()));
        let ref_w = material
            .issues
            .iter()
            .map(|i| i.identifier.chars().count())
            .max()
            .unwrap_or(0);
        for issue in &material.issues {
            let age = rfc3339_secs(&issue.done_at).map(ago).unwrap_or_default();
            lines.push(item_line(
                ("●", th.done),
                &issue.identifier,
                ref_w,
                &issue.title,
                ("done", th.done),
                &age,
                width,
                th,
            ));
        }
    }
    if !material.todos.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(rule(Source::Todos.label(), material.todos.len()));
        for todo in &material.todos {
            lines.push(item_line(
                ("✓", th.ok),
                "",
                0,
                &todo.text,
                ("", th.dim),
                &ago(todo.done_at),
                width,
                th,
            ));
        }
    }
    lines
}

/// The widest a review's page reads: past that its columns drift too
/// far from its points.
const PAGE_W: u16 = 96;

/// A run on its page: its name, period and whose on the frame, a tab per
/// project along the top, and the tab's review down a column no wider
/// than [`PAGE_W`] — or, while it is being written, what it is doing and
/// what went in.
pub fn draw(f: &mut Frame, app: &mut App, view: &WeekReviewView, th: Theme) {
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let now = orion_core::clock::now_secs() as i64;
    let run = app.week_review.run.as_ref().filter(|_| view.dir.is_none());
    let title = match run {
        Some(run) if run.fetching => format!(
            "{} · asking GitHub · {}",
            run.name(),
            run.scope.whose_label()
        ),
        Some(run) => {
            let (back, all) = run.progress();
            format!(
                "{} · writing {} of {all} · {}",
                run.name(),
                back + 1,
                run.scope.whose_label()
            )
        }
        None => format!(
            "{} · {} · {}",
            view.meta.name(),
            view.meta.period(),
            view.meta.scope.whose_label()
        ),
    };
    let mut block = panel_block(&title, true, th);
    let inner = block.inner(area);
    let names = tab_names(app, view);
    let tab = view.tab.min(names.len().saturating_sub(1));
    let strip_row = row_rect(inner, 0).unwrap_or_default();
    let (strip, tab_hits) = crate::ui::tab_strip(
        strip_row.x,
        strip_row.width,
        names.iter().map(String::as_str),
        tab,
        false,
        th,
    );
    f.render_widget(Paragraph::new(Line::from(strip)), strip_row);
    // Under the tabs and a row of air, in the middle, two cells clear of
    // either edge.
    let below = crate::ui::below_first_row(crate::ui::below_first_row(inner));
    let body = crate::ui::centered_rect(
        below,
        below.width.saturating_sub(4).min(PAGE_W),
        below.height,
    );
    let width = usize::from(body.width);
    let phase = app.animations.then(|| app.sweep_phase());
    let lines = match (run, view.texts.get(tab)) {
        (Some(run), _) => match run.parts.get(tab) {
            None => vec![waiting_line(
                app,
                "Asking GitHub what merged in each project",
                width,
                th,
            )],
            Some(part) => match &part.state {
                PartState::Done { text, problems } => {
                    review_lines(text, problems, width, phase, th)
                }
                PartState::Failed(why) => vec![Line::from(Span::styled(
                    format!("Couldn't write this one: {why}"),
                    Style::default().fg(th.warn),
                ))],
                PartState::Writing => {
                    let mut lines = writing_lines(app, run, part, width, th);
                    lines.push(crate::ui::section_rule(
                        "Included",
                        th.muted,
                        Vec::new(),
                        width,
                        th,
                    ));
                    lines.extend(item_lines(&part.material, now, width.saturating_sub(1), th));
                    lines
                }
            },
        },
        (None, Some(text)) => {
            let problems = view
                .meta
                .reviews
                .get(tab)
                .map(|r| r.problems.as_slice())
                .unwrap_or_default();
            review_lines(text, problems, width, phase, th)
        }
        (None, None) => Vec::new(),
    };
    let max_scroll = crate::app::max_scroll(lines.len(), body.height);
    let scroll = view.scroll.min(max_scroll);
    if max_scroll > 0 {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {}/{} ", scroll + 1, lines.len()),
                Style::default().fg(th.dim),
            ))
            .right_aligned(),
        );
    }
    f.render_widget(block, area);
    let total = lines.len();
    let shown: Vec<Line> = lines.into_iter().skip(usize::from(scroll)).collect();
    f.render_widget(Paragraph::new(shown), body);
    let reserve = if max_scroll > 0 { 12 } else { 0 };
    crate::hints::draw_on_border(f, area, &hints(view), reserve, th);

    if let Some(Overlay::WeekReview(live)) = &mut app.overlay {
        live.area = area;
        live.tab = tab;
        live.tab_row = strip_row.y;
        live.tab_hits = tab_hits;
        live.view_height = body.height;
        live.body_lines = total;
        live.scroll = scroll;
    }
}

/// A spinner, what is being waited on, and for how long at the right.
fn waiting_line(app: &App, what: &str, width: usize, th: Theme) -> Line<'static> {
    let glyph = crate::app::spinner_frame(app.spin_phase());
    let took = app
        .week_review
        .run
        .as_ref()
        .map(|run| format!("{}s", run.started.elapsed().as_secs()))
        .unwrap_or_default();
    Line::from(vec![
        Span::styled(format!("{glyph} "), Style::default().fg(th.warn)),
        Span::styled(what.to_string(), Style::default().fg(th.text)),
        Span::raw(
            " ".repeat(width.saturating_sub(2 + what.chars().count() + took.chars().count())),
        ),
        Span::styled(took, Style::default().fg(th.muted)),
    ])
}

/// A tab while its review is being written: what it is doing, for how
/// long, and that the window can be closed.
fn writing_lines(app: &App, run: &Run, part: &Part, width: usize, th: Theme) -> Vec<Line<'static>> {
    let glyph = crate::app::spinner_frame(app.spin_phase());
    let head = format!(
        "Writing {}'s review · {} · {}",
        part.name, run.model, run.effort
    );
    let through = match run.account.label.as_str() {
        "" => String::new(),
        name => format!("through {name}"),
    };
    let mut lines = vec![
        waiting_line(app, &head, width, th),
        Line::default(),
        Line::from(vec![
            Span::styled("  ✓ ", Style::default().fg(th.ok)),
            Span::styled(
                format!(
                    "read {}",
                    plural(
                        part.material.prs.len(),
                        "pull request description",
                        "pull request descriptions"
                    )
                ),
                Style::default().fg(th.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("  {glyph} "), Style::default().fg(th.warn)),
            Span::styled(
                "choosing what earns a line, and writing it",
                Style::default().fg(th.text),
            ),
        ]),
        Line::default(),
    ];
    let notes = [
        "You can close this. The footer says when the reviews are ready, and Automations (⌘J) lists them.",
        "Nothing runs in your checkout and no session is started: the descriptions go to the model, and text comes back.",
        through.as_str(),
    ];
    for note in notes.into_iter().filter(|note| !note.is_empty()) {
        for part in wrap(note, width) {
            lines.push(Line::from(Span::styled(
                part,
                Style::default().fg(th.muted),
            )));
        }
        lines.push(Line::default());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    const DAY: i64 = DAY_SECS;

    fn stamp(now: i64, days_ago: i64) -> String {
        orion_core::crashlog::format_timestamp((now - days_ago * DAY) as u64)
    }

    fn pr(number: u64, login: &str, name: &str, mine: bool, now: i64) -> MergedPr {
        MergedPr {
            project: "demo".into(),
            number,
            title: format!("Change {number}"),
            url: format!("https://github.com/o/r/pull/{number}"),
            login: login.into(),
            name: name.into(),
            mine,
            base: "main".into(),
            head: format!("branch-{number}"),
            additions: 10,
            deletions: 2,
            files: 1,
            merged_at: stamp(now, 1),
            body: format!("## Summary\nWhat change {number} does.").into(),
        }
    }

    /// An app with one project (`demo`) whose week of merges is fetched:
    /// two of yours, one of Grace's.
    fn app_with_week() -> (App, ProjectId, i64) {
        let mut app = App::new();
        let project = ProjectId("p1".into());
        app.tree.projects.push(orion_core::Project {
            id: project.clone(),
            name: "demo".into(),
            repo_path: "/nonexistent/orion-week-review".into(),
            sort_order: 0,
        });
        let now = orion_core::clock::now_secs() as i64;
        app.week_review.fetched.insert(
            (project.clone(), DAYS),
            Fetched {
                at: Instant::now(),
                prs: Some(vec![
                    pr(412, "ada", "Ada L", true, now),
                    pr(431, "ada", "Ada L", true, now),
                    pr(398, "grace", "Grace", false, now),
                ]),
            },
        );
        (app, project, now)
    }

    /// Config pinned to a temp file, so a test reads neither the dev's
    /// model nor their effort.
    fn pinned(f: impl FnOnce()) {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), f);
    }

    /// A Linear issue done `days_ago`.
    fn issue(id: &str, mine: bool, now: i64, days_ago: i64) -> crate::linear::LinearIssue {
        crate::linear::LinearIssue {
            id: id.into(),
            identifier: format!("ENG-{id}"),
            title: format!("Issue {id}"),
            url: format!("https://linear.app/x/issue/ENG-{id}"),
            description: String::new(),
            status: "Done".into(),
            status_type: "completed".into(),
            status_order: 0,
            team_id: String::new(),
            priority: 0,
            state_color: String::new(),
            labels: Vec::new(),
            project: None,
            assignee: "Ada".into(),
            assignee_id: String::new(),
            reporter: String::new(),
            mine,
            created_at: String::new(),
            updated_at: String::new(),
            completed_at: stamp(now, days_ago),
            prs: Vec::new(),
            comments: Vec::new(),
        }
    }

    fn view(app: &App) -> &WeekReviewView {
        match &app.overlay {
            Some(Overlay::WeekReview(v)) => v,
            other => panic!("expected the week in review, got {other:?}"),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let Some(Overlay::WeekReview(v)) = app.overlay.clone() else {
                panic!("no week in review");
            };
            draw(f, app, &v, app.theme);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A page keeps the week's merges and nobody's bots, and says where to
    /// read on from only while the week can run on.
    #[test]
    fn a_page_is_the_weeks_merges_without_bots() {
        let since = rfc3339_secs("2026-10-10T10:00:00Z").unwrap() - DAYS * DAY;
        let page = |next: bool, last_touched: &str| {
            format!(
                r#"{{"data":{{"repository":{{"pullRequests":{{
                  "pageInfo":{{"hasNextPage":{next},"endCursor":"c1"}},
                  "nodes":[
                    {{"number":40,"url":"https://github.com/o/r/pull/40","title":"New editor","body":"Does it.",
                      "additions":120,"deletions":4,"changedFiles":3,"baseRefName":"main","headRefName":"editor",
                      "mergedAt":"2026-10-09T10:00:00Z","updatedAt":"2026-10-09T11:00:00Z","viewerDidAuthor":true,
                      "author":{{"__typename":"User","login":"ada","name":"Ada L"}}}},
                    {{"number":41,"url":"https://github.com/o/r/pull/41","title":"Bump a crate",
                      "mergedAt":"2026-10-09T09:00:00Z","updatedAt":"2026-10-09T09:00:00Z",
                      "author":{{"__typename":"Bot","login":"dependabot"}}}},
                    {{"number":12,"url":"https://github.com/o/r/pull/12","title":"Old",
                      "mergedAt":"2026-08-01T10:00:00Z","updatedAt":"{last_touched}",
                      "author":{{"__typename":"User","login":"grace","name":null}}}}
                  ]}}}}}}}}"#
            )
        };
        let read = parse_page(&page(true, "2026-10-08T09:00:00Z"), "demo", since).expect("a page");
        assert_eq!(read.rows.len(), 1, "the bot's and the old one are left out");
        let pr = &read.rows[0];
        assert_eq!(
            (pr.number, pr.login.as_str(), pr.name.as_str()),
            (40, "ada", "Ada L")
        );
        assert!(pr.mine);
        assert_eq!((pr.additions, pr.files, pr.base.as_str()), (120, 3, "main"));
        assert_eq!(read.more.as_deref(), Some("c1"), "the week may run on");
        let last = parse_page(&page(true, "2026-09-30T09:00:00Z"), "demo", since).expect("a page");
        assert_eq!(last.more, None, "past the week");
        let end = parse_page(&page(false, "2026-10-08T09:00:00Z"), "demo", since).expect("a page");
        assert_eq!(end.more, None, "GitHub has no more");
        assert!(parse_page("{}", "demo", since).is_none());
    }

    /// Initials come from the name, else the login, and two people never
    /// share a set.
    #[test]
    fn initials_come_from_names_and_logins() {
        assert_eq!(initials_of("Ada L", "ada"), "AL");
        assert_eq!(initials_of("Grace", "grace-h"), "GR");
        assert_eq!(initials_of("", "linus-t"), "LT");
        assert_eq!(initials_of("", "AdaLovelace"), "AL");
        assert_eq!(initials_of("Ada King Lovelace", "ada"), "AL");
        let now = 0;
        let people = legend(&[
            pr(1, "alan", "Alan L", false, now),
            pr(2, "ada", "Ada L", false, now),
            pr(3, "ada", "Ada L", false, now),
        ]);
        let initials: Vec<&str> = people.iter().map(|(_, i, _)| i.as_str()).collect();
        assert_eq!(
            initials,
            ["AL", "ALA"],
            "the second AL takes a third letter"
        );
        assert_eq!(people[0].0, "ada", "whoever merged the most comes first");
    }

    /// `Just mine` takes what is yours; `Everyone` takes all of it and
    /// leaves your todos out; an unticked source is left out; an issue two
    /// projects both list is taken once.
    #[test]
    fn gather_follows_the_scope_and_the_ticks() {
        let (mut app, project, now) = app_with_week();
        app.linear.insert(
            project.clone(),
            crate::linear::LinearList {
                list: vec![
                    issue("1", true, now, 1),
                    issue("2", false, now, 2),
                    issue("3", true, now, 30),
                ],
                ..Default::default()
            },
        );
        let all = [true; 3];
        let mine = gather(&app, &project, Scope::default(), all, now);
        assert_eq!(mine.prs.len(), 2, "the two you opened");
        assert_eq!(mine.issues.len(), 1, "yours, inside the week");
        let everyone = Scope {
            whose: Whose::Everyone,
            ..Scope::default()
        };
        let team = gather(&app, &project, everyone, all, now);
        assert_eq!((team.prs.len(), team.issues.len()), (3, 2));
        assert!(team.todos.is_empty(), "todos are yours alone");
        let no_prs = gather(&app, &project, everyone, [false, true, true], now);
        assert!(no_prs.prs.is_empty());

        // A second project sharing the Linear workspace lists the same issue.
        let other = ProjectId("p2".into());
        app.tree.projects.push(orion_core::Project {
            id: other.clone(),
            name: "other".into(),
            repo_path: "/nonexistent/other".into(),
            sort_order: 1,
        });
        app.linear.insert(
            other,
            crate::linear::LinearList {
                list: vec![issue("1", true, now, 1)],
                ..Default::default()
            },
        );
        let both = Scope {
            wher: Where::All,
            ..Scope::default()
        };
        assert_eq!(gather(&app, &project, both, all, now).issues.len(), 1);
    }

    /// An issue Linear has not completed is done all the same when one of
    /// its pull requests merged in the week, whatever its state is called,
    /// and dated by that merge; one whose pull requests are all older or
    /// still open is not.
    #[test]
    fn an_issue_whose_pull_request_merged_is_done() {
        let (mut app, project, now) = app_with_week();
        let waiting = |id: &str, urls: &[&str]| {
            let mut issue = issue(id, true, now, 0);
            issue.status = "In Dev".into();
            issue.status_type = "started".into();
            issue.completed_at = String::new();
            issue.prs = urls
                .iter()
                .map(|url| crate::linear::IssuePr {
                    url: url.to_string(),
                    ..Default::default()
                })
                .collect();
            issue
        };
        app.linear.insert(
            project.clone(),
            crate::linear::LinearList {
                list: vec![
                    waiting("1", &["https://github.com/O/R/pull/412/files"]),
                    waiting("2", &["https://github.com/o/r/pull/7"]),
                    waiting("3", &[]),
                ],
                ..Default::default()
            },
        );
        let material = gather(&app, &project, Scope::default(), [true; 3], now);
        let ids: Vec<&str> = material
            .issues
            .iter()
            .map(|i| i.identifier.as_str())
            .collect();
        assert_eq!(ids, ["ENG-1"]);
        assert_eq!(
            material.issues[0].done_at,
            stamp(now, 1),
            "the merge's date"
        );
        let no_prs = gather(&app, &project, Scope::default(), [false, true, true], now);
        assert_eq!(no_prs.issues.len(), 1, "the merges count, listed or not");
    }

    /// The prompt names every item; an ordinary description is cut short
    /// and a very large pull request's is not; a standing branch moved
    /// into `main` is marked a release, a feature branch is not; the
    /// owner parts are an `Everyone` review's alone; the note goes in
    /// only when there is one.
    #[test]
    fn the_prompt_carries_the_week_and_the_brief() {
        let now = orion_core::clock::now_secs() as i64;
        let long = "word ".repeat(400);
        let mut big = pr(500, "ada", "Ada L", true, now);
        big.additions = 9000;
        big.body = long.as_str().into();
        let mut small = pr(501, "grace", "Grace", false, now);
        small.body = format!("{long}\n<!-- bot footer -->\nnever sent").into();
        let mut release = pr(502, "ada", "Ada L", true, now);
        release.head = "dev".into();
        release.additions = 90000;
        release.body = long.as_str().into();
        let material = Material {
            prs: vec![big, small, release],
            issues: vec![DoneIssue {
                identifier: "ENG-12".into(),
                title: "Faster search".into(),
                description: "Why it mattered.".into(),
                ..Default::default()
            }],
            todos: vec![DoneTodo {
                text: "check the editor".into(),
                done_at: now,
            }],
        };
        let people = legend(&material.prs);
        let everyone = Scope {
            whose: Whose::Everyone,
            ..Scope::default()
        };
        let text = prompt(&material, &people, everyone, "demo", brief_rules(), now);
        assert!(
            text.contains("People, by initials: AL Ada L · GR Grace."),
            "{text}"
        );
        assert!(text.contains("#500 · AL · +9000/-2 in 1 files · into main · Change 500"));
        assert!(text.contains("#501 · GR · "));
        assert!(text.contains("#502 · AL · +90000/-2 in 1 files · into main · RELEASE · "));
        assert!(text.contains("1. <what changed> · <owner> · #412"));
        assert!(text.contains("ENG-12 · Faster search"));
        assert!(text.contains("- check the editor"));
        assert!(!text.contains("never sent"), "a bot's footer is cut");
        let entry = |number: u64| {
            let from = text.find(&format!("#{number} · ")).unwrap();
            let rest = &text[from..];
            &rest[..rest.find("\n---\n").unwrap()]
        };
        assert!(entry(500).len() > 1500, "a very large one keeps more");
        assert!(entry(501).len() < 800, "an ordinary one is cut short");
        assert!(entry(502).len() < 800, "and so is a release, however large");

        let mine = prompt(
            &material,
            &[],
            Scope::default(),
            "demo",
            "Only the big ones.",
            now,
        );
        assert!(!mine.contains("People, by initials"));
        assert!(!mine.contains("<owner>"));
        assert!(mine.contains("1. <what changed> · #412"));
        assert!(mine.contains("#500 · +9000"));
        assert!(
            mine.contains("\nOnly the big ones.\n") && !mine.contains("## What earns a line"),
            "the preset's rules in orion's place"
        );
        assert!(text.contains("## What earns a line"), "orion's own rules");
    }

    const REVIEW: &str = "\
# Week in review · 4 – 10 October
demo · everyone · 3 pull requests merged

AL Ada L · GR Grace

## Editor · AL
1. Borders draw with a pen · AL · #412
   - Show: Pick a colour, then click a border: it paints.
   - Say: One click is one undo step.
2. Panels widen by dragging · AL · #431

## Speed · GR
1. Large documents open at once · GR · #398
";

    /// A reply in its shape passes; each way it can stray is one sentence.
    #[test]
    fn a_reply_is_held_to_its_shape() {
        let (app, project, now) = app_with_week();
        let everyone = Scope {
            whose: Whose::Everyone,
            ..Scope::default()
        };
        let material = gather(&app, &project, everyone, [true; 3], now);
        let people = legend(&material.prs);
        assert_eq!(check(REVIEW, &material, &people), Vec::<String>::new());

        let unknown = REVIEW.replace("#431", "#999");
        assert_eq!(
            check(&unknown, &material, &people),
            ["1 names a pull request that was not in the week"]
        );
        let wrong = REVIEW.replace("· GR · #398", "· AL · #398");
        let problems = check(&wrong, &material, &people);
        assert!(
            problems.contains(&"1 point names the wrong owner".to_string()),
            "{problems:?}"
        );
        assert!(
            problems.contains(&"1 area names people its points do not".to_string()),
            "{problems:?}"
        );
        let often = format!(
            "{REVIEW}2. Again · GR · #398\n3. And again · GR · #398\n4. And again · GR · #398\n"
        );
        assert_eq!(
            check(&often, &material, &people),
            ["1 pull request is on more than three points"]
        );
    }

    /// The last line is orion's count of what was merged and not listed,
    /// and a reply wrapped in a fence or led by a remark is unwrapped.
    #[test]
    fn the_unlisted_are_counted_by_orion() {
        let (app, project, now) = app_with_week();
        let everyone = Scope {
            whose: Whose::Everyone,
            ..Scope::default()
        };
        let material = gather(&app, &project, everyone, [true; 3], now);
        let two = REVIEW.replace(
            "\n## Speed · GR\n1. Large documents open at once · GR · #398\n",
            "",
        );
        let text = finish(&format!("Here it is:\n```\n{two}```\n"), &material);
        assert!(text.starts_with("# Week in review"), "{text}");
        assert!(!text.contains("```"));
        assert!(
            text.ends_with("+ 1 other pull request merged, not listed\n"),
            "{text}"
        );
        let all = finish(REVIEW, &material);
        assert!(!all.contains("not listed"), "nothing left to count: {all}");
        // A reply with no title keeps its points rather than losing them.
        let untitled = two.lines().skip(1).collect::<Vec<_>>().join("\n");
        let kept = finish(&untitled, &material);
        assert!(
            kept.contains("1. Borders draw with a pen · AL · #412"),
            "{kept}"
        );
        // A model that numbers straight through is put right: each area
        // starts at 1, and a sub-line keeps its place under its point.
        let through = REVIEW
            .replace("1. Large documents", "12. Large documents")
            .replace("2. Panels widen", "7. Panels widen")
            .replace("   - Say:", "    - Say:");
        assert_eq!(finish(&through, &material), all);
    }

    /// A review is written through a Claude account orion runs: the one
    /// the **Week review** preset runs on; with that one gone, the default
    /// agent's, with the config dir that pins it; and, where the default
    /// agent is not Claude at all, the first Claude account that is on. It
    /// runs at the preset's model and effort, sonnet · medium unset.
    #[test]
    fn a_review_is_written_through_one_of_orions_claude_accounts() {
        use crate::agent_presets::{AgentPreset, SystemPreset};
        let on = |kind: orion_core::AgentKind, custom: Option<&str>| AgentPreset {
            kind,
            custom_harness: custom.map(String::from),
            follow_box: Some(false),
            ..SystemPreset::WeekReview.default_preset()
        };
        let gone = on(orion_core::AgentKind::Custom, Some("gone"));
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let config = dir.path().join("config.json");
        std::fs::write(
            &config,
            format!(
                r#"{{"claude_enabled": true, "codex_enabled": true, "quick_prompt_kind": "claude-work",
                     "claude_accounts": [{{"id": "claude-work", "name": "Work", "config_dir": "{}"}}]}}"#,
                work.display()
            ),
        )
        .unwrap();
        crate::config::with_config_path(config, || {
            let pinned = |a: &Account| {
                a.env
                    .iter()
                    .find(|(name, _)| name == orion_core::env::CLAUDE_CONFIG_DIR)
                    .map(|(_, dir)| dir.clone())
            };
            let mut cfg = crate::config::Config::load();
            let through = oneshot::account_for(&cfg, &gone).expect("the default agent's account");
            assert_eq!(through.program, "claude");
            assert_eq!(pinned(&through), Some(work.display().to_string()));
            assert!(through.label.starts_with("Work"), "{}", through.label);

            let claude = on(orion_core::AgentKind::Claude, None);
            let through = oneshot::account_for(&cfg, &claude).expect("the one pinned");
            assert_eq!(pinned(&through), None, "built-in Claude, its own sign-in");
            let work_on = on(orion_core::AgentKind::Claude, Some("claude-work"));
            assert!(pinned(&oneshot::account_for(&cfg, &work_on).unwrap()).is_some());
            // Following the box, it is written through the default agent's.
            let follows = AgentPreset {
                follow_box: Some(true),
                ..claude.clone()
            };
            assert_eq!(
                pinned(&oneshot::account_for(&cfg, &follows).unwrap()),
                Some(work.display().to_string())
            );

            cfg.quick_prompt_kind = "codex".into();
            assert_eq!(
                oneshot::account_for(&cfg, &gone)
                    .expect("a Claude account that is on")
                    .program,
                "claude"
            );

            assert_eq!(
                launch_of(&claude),
                ("sonnet".to_string(), "medium".to_string())
            );
            let unset = AgentPreset {
                model: None,
                effort: None,
                ..claude
            };
            assert_eq!(launch_of(&unset), launch_of(&work_on), "the defaults");
        });
    }

    /// A release is one standing branch moved into another, never a
    /// feature branch merged into `main`.
    #[test]
    fn a_release_is_a_standing_branch_into_main() {
        let mut one = pr(1, "ada", "Ada L", true, 0);
        assert!(!one.is_release(), "a feature branch into main");
        one.head = "dev".into();
        assert!(one.is_release());
        one.base = "dev".into();
        one.head = "feature".into();
        assert!(!one.is_release());
    }
    /// The reply's columns: a point's owner and pull request right-aligned,
    /// and an area's one contributor exactly over its points' owner.
    #[test]
    fn one_contributor_sits_over_its_points_owner() {
        let th = App::new().theme;
        let lines = review_lines(REVIEW, &[], 70, None, th);
        let text =
            |line: &Line| -> String { line.spans.iter().map(|s| s.content.as_ref()).collect() };
        let rows: Vec<String> = lines.iter().map(text).collect();
        let heading = rows
            .iter()
            .find(|r| r.starts_with("Editor"))
            .expect("the area");
        let point = rows
            .iter()
            .find(|r| r.contains("Borders draw"))
            .expect("a point");
        let end = |row: &str, word: &str| row.rfind(word).map(|at| row[..at].chars().count() + 2);
        assert_eq!(end(heading, "AL"), end(point, "AL"), "\n{heading}\n{point}");
        assert!(point.ends_with("#412"), "{point:?}");
        assert_eq!(point.chars().count(), 70);
        let show = rows
            .iter()
            .find(|r| r.contains("Show"))
            .expect("a show line");
        assert!(show.starts_with("   Show Pick a colour"), "{show:?}");
        // A just-mine review has no owner column to keep.
        let mine = "# Week\n\n## Editor\n1. Borders draw with a pen · #412\n";
        let rows: Vec<String> = review_lines(mine, &[], 40, None, th)
            .iter()
            .map(text)
            .collect();
        let point = rows
            .iter()
            .find(|r| r.contains("Borders"))
            .expect("a point");
        assert!(point.ends_with("pen          #412"), "{point:?}");
        assert_eq!(point.chars().count(), 40);
    }

    /// A second project, `quiet`, with nothing finished in the week, and a
    /// third, `docs`, whose week is fetched with one merge of yours.
    fn with_two_more(app: &mut App, now: i64) {
        for (id, name, sort) in [("p2", "quiet", 1), ("p3", "docs", 2)] {
            app.tree.projects.push(orion_core::Project {
                id: ProjectId(id.into()),
                name: name.into(),
                repo_path: format!("/nonexistent/{name}").into(),
                sort_order: sort,
            });
        }
        app.week_review.fetched.insert(
            (ProjectId("p2".into()), DAYS),
            Fetched {
                at: Instant::now(),
                prs: Some(Vec::new()),
            },
        );
        let mut docs = pr(7, "ada", "Ada L", true, now);
        docs.project = "docs".into();
        app.week_review.fetched.insert(
            (ProjectId("p3".into()), DAYS),
            Fetched {
                at: Instant::now(),
                prs: Some(vec![docs]),
            },
        );
    }

    /// A run is one review a project that had changes: a project with
    /// none is left out, each one's material is its own, and one GitHub
    /// could not be asked about is not written from the rest.
    #[test]
    fn a_run_splits_by_project_and_skips_the_quiet_ones() {
        let (mut app, _, now) = app_with_week();
        with_two_more(&mut app, now);
        let parts = split(&app, Scope::default(), now);
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["demo", "docs"], "quiet had nothing");
        assert_eq!(parts[0].material.prs.len(), 2, "demo's two of yours");
        assert_eq!(parts[1].material.prs.len(), 1);
        assert!(parts.iter().all(|p| p.state == PartState::Writing));
        assert!(parts.iter().all(|p| p.people.is_empty()), "just me");
        let everyone = Scope {
            whose: Whose::Everyone,
            ..Scope::default()
        };
        let team = split(&app, everyone, now);
        assert_eq!(team[0].material.prs.len(), 3);
        assert_eq!(team[0].people.len(), 2, "each point names its owner");

        // GitHub couldn't be asked about docs: with nothing else finished
        // there it is left out; with an issue done, it says why.
        app.week_review
            .fetched
            .get_mut(&(ProjectId("p3".into()), DAYS))
            .unwrap()
            .prs = None;
        assert_eq!(split(&app, Scope::default(), now).len(), 1);
        app.linear.insert(
            ProjectId("p3".into()),
            crate::linear::LinearList {
                list: vec![issue("9", true, now, 1)],
                ..Default::default()
            },
        );
        let parts = split(&app, Scope::default(), now);
        assert!(
            matches!(&parts[1].state, PartState::Failed(why) if why.contains("GitHub")),
            "{:?}",
            parts[1].state
        );
        // Two projects of one name keep a file each.
        app.tree.projects[2].name = "demo".into();
        app.week_review
            .fetched
            .get_mut(&(ProjectId("p3".into()), DAYS))
            .unwrap()
            .prs = Some(Vec::new());
        let slugs: Vec<String> = split(&app, Scope::default(), now)
            .into_iter()
            .map(|p| p.slug)
            .collect();
        assert_eq!(slugs, ["demo", "demo-2"]);
    }

    /// The reply for `part` of the run being written, as the model sends
    /// it.
    fn land(app: &mut App, part: usize, reply: Result<&str, &str>) {
        let id = app.week_review.run.as_ref().expect("a run").id;
        let result = reply
            .map(|text| Reply {
                text: text.to_string(),
                cost: Some(0.01),
            })
            .map_err(str::to_string);
        land_written(app, id, part, result, 4);
    }

    const DOCS: &str = "# Week in review · 1 – 7 Nov\ndocs · just me · 1 pull requests merged\n\n## Guides\n1. A guide to the grid · #7\n";

    /// `r`: GitHub asked about every project, then each one with changes
    /// written up; each lands checked, finished and kept in the run's
    /// folder as `<project>.md` beside `meta.json`, and the run is listed,
    /// newest first, once the last is back. The page has a tab a project.
    #[test]
    fn a_run_is_kept_a_file_a_project_and_read_on_tabs() {
        pinned(|| {
            let (mut app, _, now) = app_with_week();
            with_two_more(&mut app, now);
            let dir = tempfile::tempdir().unwrap();
            app.week_review.root = Some(dir.path().to_path_buf());
            run_now(&mut app).unwrap();
            assert!(run_now(&mut app).is_err(), "one run at a time");
            let run = app.week_review.run.as_ref().unwrap();
            assert!(!run.fetching, "every week was in hand");
            assert_eq!(run.progress(), (0, 2));
            assert!(run.rules.contains("## What earns a line"), "the preset's");

            // The page on the run being written: a tab a project.
            open(&mut app, 0, false);
            let shot = screen(&mut app, 150, 40);
            assert!(shot.contains("· writing 1 of 2 · just me"), "{shot}");
            assert!(shot.contains(" demo ") && shot.contains(" docs "), "{shot}");
            assert!(shot.contains("Writing demo's review"), "{shot}");

            land(&mut app, 1, Ok(DOCS));
            let run = app.week_review.run.as_ref().expect("demo is still out");
            let to = run.scope.period(run.asked).1.format("%Y-%m-%d").to_string();
            let folder = dir.path().join("all").join(&to);
            assert!(folder.join("docs.md").is_file());
            assert!(folder.join("meta.json").is_file(), "kept as it goes");
            assert!(app.week_review.kept.is_empty(), "listed once it's over");

            handle_key(&mut app, key(KeyCode::Char(']')));
            assert_eq!(view(&app).tab, 1);
            assert!(screen(&mut app, 150, 40).contains("A guide to the grid"));

            land(&mut app, 0, Ok(REVIEW));
            assert!(app.week_review.run.is_none());
            assert_eq!(app.flash.as_deref(), Some("Week in review ready · ⌘J"));
            assert_eq!(app.week_review.kept.len(), 1);
            let kept = &app.week_review.kept[0];
            assert_eq!(kept.dir, folder);
            let names: Vec<&str> = kept
                .meta
                .reviews
                .iter()
                .map(|r| r.project.as_str())
                .collect();
            assert_eq!(names, ["demo", "docs"], "in the run's order");
            assert_eq!(kept.meta.points(), 4);
            assert!(kept.meta.read, "the page was on it as it landed");
            assert!(folder.join("demo.md").is_file());
            // The page turned to the run as kept, still on docs.
            let v = view(&app);
            assert_eq!(v.dir.as_ref(), Some(&folder));
            assert_eq!(v.tab, 1);
            assert!(v.texts[0].starts_with("# Week in review"));

            // `[` back to demo; Esc closes a page not opened from ⌘J.
            handle_key(&mut app, key(KeyCode::Char('[')));
            let shot = screen(&mut app, 150, 40);
            assert!(shot.contains("Borders draw with a pen"), "{shot}");
            assert!(shot.contains(&kept_name(&app)), "{shot}");
            handle_key(&mut app, key(KeyCode::Esc));
            assert!(app.overlay.is_none());

            // A second run the same day takes the first's place.
            run_now(&mut app).unwrap();
            land(&mut app, 0, Ok(REVIEW));
            land(&mut app, 1, Err("timed out"));
            assert_eq!(
                app.flash.as_deref(),
                Some("Week in review ready · 1 couldn't be written")
            );
            assert_eq!(app.week_review.kept.len(), 1);
            assert!(
                !folder.join("docs.md").exists(),
                "the old run's file is gone"
            );
            let meta = &app.week_review.kept[0].meta;
            assert_eq!(meta.failed, [("docs".to_string(), "timed out".to_string())]);
            assert!(!meta.read, "new until it's read");
        });
    }

    fn kept_name(app: &App) -> String {
        app.week_review.kept[0].meta.name()
    }

    /// Every review failing keeps nothing, and the footer says why.
    #[test]
    fn a_run_that_wrote_nothing_keeps_nothing() {
        pinned(|| {
            let (mut app, _, _) = app_with_week();
            let dir = tempfile::tempdir().unwrap();
            app.week_review.root = Some(dir.path().to_path_buf());
            run_now(&mut app).unwrap();
            land(&mut app, 0, Err("not signed in"));
            assert!(app.week_review.run.is_none());
            assert!(app.week_review.kept.is_empty());
            assert_eq!(app.flash.as_deref(), Some("Week in review: not signed in"));
            // Nothing finished anywhere: nothing to write.
            let mut quiet = App::new();
            quiet.week_review.root = Some(dir.path().to_path_buf());
            run_now(&mut quiet).unwrap();
            assert!(quiet.week_review.run.is_none());
            assert_eq!(
                quiet.flash.as_deref(),
                Some("Week in review: nothing finished in the last 7 days to write up")
            );
        });
    }

    /// The newest dozen runs are kept; what an older release kept is left
    /// where it is and not listed.
    #[test]
    fn the_newest_dozen_runs_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let all = dir.path().join("all");
        for day in 1..=14 {
            let to = format!("2026-10-{day:02}");
            let meta = Meta {
                to: to.clone(),
                ..Meta::default()
            };
            std::fs::create_dir_all(all.join(&to)).unwrap();
            crate::pr_cache::write_json_atomic(&all.join(&to).join(META), &meta).unwrap();
        }
        std::fs::write(all.join("2026-09-01-mine.md"), "# old").unwrap();
        prune(dir.path());
        let runs = load_runs(dir.path());
        assert_eq!(runs.len(), KEEP);
        assert_eq!(runs[0].meta.to, "2026-10-14", "newest first");
        assert!(!all.join("2026-10-02").exists());
        assert!(all.join("2026-09-01-mine.md").is_file());
    }

    /// INPUT PARITY: a click on a tab is the keys landing on it.
    #[test]
    fn a_click_on_a_tab_is_the_keys_landing_there() {
        pinned(|| {
            let landed = |by_click: bool| {
                let (mut app, _, now) = app_with_week();
                with_two_more(&mut app, now);
                run_now(&mut app).unwrap();
                open(&mut app, 0, false);
                screen(&mut app, 150, 40);
                if by_click {
                    let (x, row) = (view(&app).tab_hits[1].0, view(&app).tab_row);
                    handle_mouse(
                        &mut app,
                        MouseEvent {
                            kind: MouseEventKind::Down(MouseButton::Left),
                            column: x + 1,
                            row,
                            modifiers: KeyModifiers::NONE,
                        },
                    );
                } else {
                    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
                }
                let v = view(&app);
                (v.tab, v.scroll, app.overlay.is_some(), app.flash.clone())
            };
            assert_eq!(landed(true), landed(false));
            assert_eq!(landed(true).0, 1);
        });
    }

    #[test]
    fn every_key_parses() {
        assert!(keys::ALL.iter().all(crate::hints::Key::parses));
    }

    /// The real thing, through the runner's own path, against the real
    /// GitHub and model: a run over the checkout `ORION_WEEK_REVIEW_LIVE`
    /// names, kept under a scratch folder — never the data dir. Never run
    /// by the suite.
    #[tokio::test]
    #[ignore = "asks GitHub and a model: set ORION_WEEK_REVIEW_LIVE to a checkout"]
    async fn live_a_real_run_is_written_and_kept() {
        let Some(dir) = orion_core::env::non_empty("ORION_WEEK_REVIEW_LIVE") else {
            panic!("set ORION_WEEK_REVIEW_LIVE to a checkout");
        };
        let root = orion_core::env::non_empty("ORION_WEEK_REVIEW_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("orion-week-review-live"));
        let mut app = App::new();
        let name = Path::new(&dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("live")
            .to_string();
        app.tree.projects.push(orion_core::Project {
            id: ProjectId("live".into()),
            name,
            repo_path: PathBuf::from(&dir),
            sort_order: 0,
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.week_review.tx = Some(tx);
        app.week_review.root = Some(root.clone());
        if std::env::var("ORION_WEEK_REVIEW_SCOPE").as_deref() == Ok("everyone") {
            app.automations
                .settings
                .of_mut(crate::automations::Automation::WeekReview)
                .whose = crate::automations::config::Whose::WholeTeam;
        }
        let started = Instant::now();
        run_now(&mut app).expect("the run starts");
        while app.week_review.run.is_some() {
            let answer = rx.recv().await.expect("an answer");
            land_answer(&mut app, answer);
        }
        println!("{:?} in {:?}", app.flash, started.elapsed());
        let kept = app.week_review.kept.first().expect("a run kept");
        println!("kept in {}", kept.dir.display());
        for (i, review) in kept.meta.reviews.iter().enumerate() {
            let path = kept.path(i).unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            println!(
                "\n{} · {} points · problems {:?}\n{text}",
                path.display(),
                review.points,
                review.problems
            );
            assert!(review.problems.is_empty(), "{:?}", review.problems);
        }
    }
}
