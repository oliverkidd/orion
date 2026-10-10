//! WEEK IN REVIEW: what was finished in the last seven days — merged pull
//! requests, Linear issues done, todos ticked — written up as a short
//! review to read aloud with the product on screen: what changed, by
//! product area, with what to show and what to say.
//!
//! * **Gathering is orion's.** Opening the modal fetches every pull
//!   request merged in the week, description and all ([`fetch_merged`]):
//!   the PULL REQUESTS MODAL's merged tail carries titles, and a review
//!   is written from what a pull request says it does. Linear's done
//!   issues and the ticked todos come from the lists the app already
//!   holds ([`gather`]).
//! * **Writing is a model's, in one ask.** [`prompt`] puts everything in
//!   one text and [`write`] sends it through the print mode of a Claude
//!   account orion runs — the default agent's, or the **Review account**
//!   row's ([`account`]) — with no tools: nothing runs in a checkout, no
//!   session appears on the grid, and there is nothing to tidy up after. Measured on a week
//!   of 162 pull requests: under a minute, where an agent session that
//!   fetched for itself took over two.
//! * **Counting is orion's again.** The model is told to list what earns
//!   a line and nothing else; [`check`] holds the reply to its shape
//!   (every number a pull request that was sent, every owner its author)
//!   and [`finish`] appends how many were merged and not listed — exact,
//!   because nothing was asked to add it up.
//!
//! A review is plain Markdown, kept under the DATA DIR
//! (`reviews/<project>/<date>-<mine|everyone>.md`) with what it was
//! written from beside it, the last [`KEEP`] of them. Nothing here touches
//! the disk or starts a process unless the main loop installed a
//! directory and a channel ([`State`]): the unit tests never do.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use orion_core::ProjectId;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::hints::Hint;
use crate::pull_request::{bool_at, rfc3339_secs, str_at, u64_at};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{centered_rect_pct, panel_block, row_rect, truncate, SPLIT_MODAL_PCT};

/// How far back a review reaches: the PULL REQUESTS MODAL's week of
/// merges, and Linear's week of done issues.
pub const DAYS: i64 = crate::pull_request::MERGED_DAYS;
// Linear cuts its done issues to its own week on Linear's side: a review
// reaching further back than that would find none of them there.
const _: () = assert!(DAYS == crate::linear::DONE_DAYS as i64);
/// How many reviews a project keeps; the oldest go as a new one lands.
const KEEP: usize = 12;
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
/// How many pages [`fetch_merged`] reads before it gives up on a week:
/// the merged tail's own guard.
const PAGES_MAX: usize = crate::pull_request::MERGED_PAGES_MAX;
/// How long the write may take before it is given up on.
const WRITE_TIMEOUT: Duration = Duration::from_secs(300);

/// The **Review model** row's choices, the first the default: the best
/// blend of speed and judgement when the brief was tried on a real week.
pub const MODELS: &[&str] = &["sonnet", "opus", "haiku"];
/// The **Review effort** row's choices; `medium` is the default.
pub const EFFORTS: &[&str] = &["medium", "low", "high"];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Scope {
    pub whose: Whose,
    pub wher: Where,
}

impl Scope {
    /// `just mine · this project`.
    pub fn label(self) -> String {
        format!("{} · {}", self.whose_label(), self.where_label())
    }

    fn whose_label(self) -> &'static str {
        match self.whose {
            Whose::Mine => "just mine",
            Whose::Everyone => "everyone",
        }
    }

    fn where_label(self) -> &'static str {
        match self.wher {
            Where::Project => "this project",
            Where::All => "all projects",
        }
    }

    /// `mine` or `everyone`: the Reviews rows' word, and the file name's.
    fn slug(self) -> &'static str {
        match self.whose {
            Whose::Mine => "mine",
            Whose::Everyone => "everyone",
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
    /// The description. Sent to the model, never kept with the review.
    #[serde(skip)]
    pub body: String,
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
    /// RFC 3339.
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

    fn len(&self) -> usize {
        self.prs.len() + self.issues.len() + self.todos.len()
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

/// Parse one page. A pull request merged inside the window was last
/// touched inside it too, so a page that ends on one touched before it
/// is the last worth reading. Bots' pull requests (dependency bumps) are
/// left out here: no review has a line for one.
fn parse_page(json: &str, project: &str, now: i64) -> Option<Page> {
    let answer: serde_json::Value = serde_json::from_str(json).ok()?;
    let tail = answer.pointer("/data/repository/pullRequests")?;
    let nodes = tail.get("nodes")?.as_array()?;
    let since = now - DAYS * 24 * 60 * 60;
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
                body: str_at(v, "body"),
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

/// Every pull request merged in `dir`'s repo in the last [`DAYS`], the
/// latest merge first, descriptions and all. `None` when GitHub could not
/// be asked, or a page of the week could not be read: a review written
/// from part of a week would pass it off as the whole.
pub async fn fetch_merged(dir: PathBuf, project: String, now: i64) -> Option<Vec<MergedPr>> {
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
        let page = parse_page(&out, &project, now)?;
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

/// The sources a review can be written from, in the Compose panel's order.
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
/// pull requests, the done issues Linear's list holds and the todos
/// ticked, each inside the last [`DAYS`] before `now` and each only when
/// its source is ticked in `picks`. `Just mine` keeps what is yours;
/// `Everyone` keeps all of it and leaves the todos out, which are nobody
/// else's. An issue two projects' lists both hold is taken once.
pub fn gather(
    app: &App,
    project: &ProjectId,
    scope: Scope,
    picks: [bool; 3],
    now: i64,
) -> Material {
    let since = now - DAYS * 24 * 60 * 60;
    let mine = scope.whose == Whose::Mine;
    let mut material = Material::default();
    let mut seen_issues = HashSet::new();
    for (id, _, dir) in projects_in(app, project, scope.wher) {
        if picks[Source::Prs.index()] {
            let fetched = app
                .week_review
                .fetched
                .get(&id)
                .and_then(|f| f.prs.as_ref());
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
                let done = issue.status_type == "completed"
                    && rfc3339_secs(&issue.completed_at).is_some_and(|at| at >= since);
                if done && (!mine || issue.mine) && seen_issues.insert(issue.id.clone()) {
                    material.issues.push(DoneIssue {
                        identifier: issue.identifier.clone(),
                        title: issue.title.clone(),
                        assignee: issue.assignee.clone(),
                        done_at: issue.completed_at.clone(),
                        description: issue.description.clone(),
                    });
                }
            }
        }
        if picks[Source::Todos.index()] && mine {
            let items = app.todos.get(&dir).map(|file| file.items.as_slice());
            for item in items.into_iter().flatten() {
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
    }
    material.prs.sort_by(|a, b| b.merged_at.cmp(&a.merged_at));
    material.issues.sort_by(|a, b| b.done_at.cmp(&a.done_at));
    material.todos.sort_by_key(|t| std::cmp::Reverse(t.done_at));
    material
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

/// What the model is asked, whole: the brief, the shape, the asker's
/// note, and every pull request, issue and todo of `material`. `people`
/// is [`legend`]'s, used in an `Everyone` review to name each point's
/// owner.
pub fn prompt(
    material: &Material,
    people: &[(String, String, String)],
    scope: Scope,
    project: &str,
    note: &str,
    now: i64,
) -> String {
    let everyone = scope.whose == Whose::Everyone;
    let several = material.several_projects();
    let (from, to) = period(now);
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
    text.push_str(BRIEF_EARNS);
    if everyone {
        text.push_str("- A point's owner is the author of the first pull request it lists. Exactly one owner a point, by the initials given.\n");
    }
    text.push_str("\n## The shape, exactly\n\n```\n");
    text.push_str(&format!(
        "# Week in review · {}\n{project} · {} · {} pull requests merged\n\n",
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
    let note = note.trim();
    if !note.is_empty() {
        text.push_str(&format!("\n## Note from the person asking\n\n{note}\n"));
    }
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
    text.push_str("\nWrite the review now: the Markdown and nothing else, starting with the `# Week in review` line.\n");
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

/// The period a review written at `now` covers, as local days: today and
/// the six before it.
fn period(now: i64) -> (NaiveDate, NaiveDate) {
    let to = Local
        .timestamp_opt(now, 0)
        .single()
        .map(|at| at.date_naive())
        .unwrap_or_default();
    (to - chrono::Duration::days(DAYS - 1), to)
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

/// What [`write`] got back.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    /// What the ask cost, in US dollars, when the command said.
    pub cost: Option<f64>,
}

/// The Claude account a review is written through, as [`write`] runs it:
/// one of the accounts orion runs sessions on — its own program, with the
/// config dir that pins it — run directly, as the sign-in modal runs
/// `claude auth`. A shell alias for the program is not followed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Account {
    /// The account's own CLI: `claude`, or what its `harnesses` row names.
    pub program: String,
    /// Its environment on top of orion's — the `CLAUDE_CONFIG_DIR` that
    /// pins an extra account to its own sign-in.
    pub env: Vec<(String, String)>,
    /// `Work (a@b.co)`, or `Work (not signed in)`: what Compose and the
    /// Writing panel call it. Being signed out is said, never refused on:
    /// the record only knows a browser sign-in, and an account on an API
    /// key answers all the same. One that cannot answer says so itself.
    pub label: String,
}

/// The account `cfg` writes reviews through (`Config::review_account`);
/// None with no Claude account switched on.
pub fn account(cfg: &crate::config::Config) -> Option<Account> {
    let entry = cfg.review_account()?;
    Some(Account {
        program: entry.program.trim().to_string(),
        env: entry.launch_env(),
        label: entry.display_label().to_string(),
    })
}

/// Send `prompt` to `model` at `effort` through `account`'s CLI in print
/// mode, with no tools: text in, text out, under that account's sign-in.
/// The failure is the one line worth showing — the command missing, not
/// signed in, timed out.
pub async fn write(
    prompt: String,
    model: String,
    effort: String,
    account: Account,
) -> Result<Reply, String> {
    let mut cmd = tokio::process::Command::new(&account.program);
    cmd.envs(account.env.iter().map(|(name, value)| (name, value)));
    cmd.args([
        "-p",
        "--model",
        model.as_str(),
        "--effort",
        effort.as_str(),
        "--tools",
        "",
        "--system-prompt",
        SYSTEM,
        "--no-session-persistence",
        "--output-format",
        "json",
    ]);
    let out = crate::pull_request::run_piped(cmd, &prompt, WRITE_TIMEOUT).await?;
    parse_reply(&out)
}

/// `claude -p --output-format json`'s answer: the text under `result`,
/// unless it says it is an error.
fn parse_reply(json: &str) -> Result<Reply, String> {
    let answer: serde_json::Value =
        serde_json::from_str(json).map_err(|_| "claude answered something that was not JSON")?;
    let text = answer
        .get("result")
        .and_then(|r| r.as_str())
        .unwrap_or_default()
        .trim();
    if answer.get("is_error").and_then(|e| e.as_bool()) == Some(true) || text.is_empty() {
        let why = text.lines().next().unwrap_or("claude wrote nothing");
        return Err(truncate(why, WHY_CUT));
    }
    Ok(Reply {
        text: text.to_string(),
        cost: answer.get("total_cost_usd").and_then(|c| c.as_f64()),
    })
}

// ---- checking ----

/// How many points one pull request may sit on: a feature-branch merge
/// is the one kind the brief lets repeat.
const POINTS_A_PR: usize = 3;
/// How much of a failure's first line is worth showing.
const WHY_CUT: usize = 120;

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

/// What is kept beside a review's Markdown: when and what it covers, what
/// it was written from, and what the write cost.
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
    pub secs: u64,
    pub cost: Option<f64>,
    /// [`check`]'s sentences, when the reply did not hold its shape.
    pub problems: Vec<String>,
    /// Shown once already: no longer `new`.
    pub read: bool,
    /// What it was written from: kept on disk, and never read back — a
    /// dozen reviews' worth of it would ride every frame's copy of the
    /// modal for nothing.
    #[serde(skip_deserializing)]
    pub material: Material,
}

impl Meta {
    /// `4 – 10 Oct`.
    fn period(&self) -> String {
        let day = |text: &str| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok();
        match (day(&self.from), day(&self.to)) {
            (Some(from), Some(to)) => period_label(from, to, false),
            _ => self.to.clone(),
        }
    }
}

/// A review on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    /// The Markdown; the [`Meta`] is the same path with `.json`.
    pub path: PathBuf,
    pub meta: Meta,
}

/// The folder `project`'s reviews are kept in — or every project's, for
/// one that covers them all. A project's id is made safe as a file name.
fn folder(root: &Path, project: &ProjectId, wher: Where) -> PathBuf {
    match wher {
        Where::Project => {
            let name: String = project
                .0
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            root.join(format!("p-{name}"))
        }
        Where::All => root.join("all"),
    }
}

/// The reviews kept for `project` and for all projects, newest first.
fn load_reviews(root: &Path, project: &ProjectId) -> Vec<Saved> {
    let mut reviews = Vec::new();
    for wher in [Where::Project, Where::All] {
        let Ok(entries) = std::fs::read_dir(folder(root, project, wher)) else {
            continue;
        };
        for entry in entries.flatten() {
            let json = entry.path();
            if json.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let meta = std::fs::read_to_string(&json)
                .ok()
                .and_then(|raw| serde_json::from_str::<Meta>(&raw).ok());
            let path = json.with_extension("md");
            if let (Some(meta), true) = (meta, path.is_file()) {
                reviews.push(Saved { path, meta });
            }
        }
    }
    reviews.sort_by(|a, b| (&b.meta.to, b.meta.written_at).cmp(&(&a.meta.to, a.meta.written_at)));
    reviews
}

/// Keep `text` and `meta` as `project`'s review of its period and scope —
/// a second one of the same takes the first's place — and drop the
/// folder's oldest past [`KEEP`]. The Markdown's path.
fn save(root: &Path, project: &ProjectId, meta: &Meta, text: &str) -> std::io::Result<PathBuf> {
    let dir = folder(root, project, meta.scope.wher);
    let path = dir.join(format!("{}-{}.md", meta.to, meta.scope.slug()));
    orion_core::settings::write_atomic(&path, text.as_bytes())?;
    crate::pr_cache::write_json_atomic(&path.with_extension("json"), meta)?;
    let mut kept: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    kept.sort();
    let extra = kept.len().saturating_sub(KEEP);
    for old in kept.into_iter().take(extra) {
        let _ = std::fs::remove_file(old.with_extension("json"));
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// Where a project's standing note to the model is kept.
fn note_path(root: &Path, project: &ProjectId) -> PathBuf {
    folder(root, project, Where::Project).join("note.txt")
}

// ---- the app's side ----

/// What one project's fetch came back with, and when.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub at: Instant,
    /// `None`: GitHub could not be asked.
    pub prs: Option<Vec<MergedPr>>,
}

/// A review being written.
#[derive(Debug)]
pub struct Pending {
    /// Told apart from one stopped or started again since.
    pub id: u64,
    pub project: ProjectId,
    pub scope: Scope,
    pub material: Material,
    pub people: Vec<(String, String, String)>,
    /// The Claude account writing it, by the name Compose showed.
    pub account: String,
    pub model: String,
    pub effort: String,
    pub started: Instant,
    /// Unix seconds when it was asked: what dates the review.
    pub asked: i64,
    /// The write, for `⌘W` to stop.
    pub task: Option<tokio::task::AbortHandle>,
}

/// An answer back on the loop.
#[derive(Debug)]
pub enum Answer {
    Fetched {
        project: ProjectId,
        prs: Option<Vec<MergedPr>>,
    },
    Written {
        id: u64,
        result: Result<Reply, String>,
    },
}

/// The WEEK IN REVIEW's state on the [`App`]: it outlives the modal, so
/// closing it loses neither a fetch nor a write.
#[derive(Debug, Default)]
pub struct State {
    /// `<DATA DIR>/reviews`. `None` — every unit test — keeps nothing.
    pub root: Option<PathBuf>,
    /// Where answers go. `None` — every unit test — starts nothing.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    pub fetched: HashMap<ProjectId, Fetched>,
    pub fetching: HashSet<ProjectId>,
    pub pending: Option<Pending>,
    next_id: u64,
}

/// Ask for `project`'s week of merged pull requests, unless a fetch is out
/// or a fresh one is in hand.
fn request_fetch(app: &mut App, project: &ProjectId, name: &str, dir: &Path) {
    let state = &mut app.week_review;
    let fresh = state
        .fetched
        .get(project)
        .is_some_and(|f| f.prs.is_some() && f.at.elapsed() < FRESH);
    let Some(tx) = state.tx.clone() else {
        return;
    };
    if fresh || !state.fetching.insert(project.clone()) {
        return;
    }
    let (project, name, dir) = (project.clone(), name.to_string(), dir.to_path_buf());
    let now = orion_core::clock::now_secs() as i64;
    tokio::spawn(async move {
        let prs = fetch_merged(dir, name, now).await;
        let _ = tx.send(Answer::Fetched { project, prs });
    });
}

/// Ask for every project `scope` covers.
///
/// The lists a review reads beside the pull requests are asked for too:
/// Linear's, where the project has a key, and the project's todos, which
/// are in hand only once the TODOS MODAL has been opened. And a week
/// fetched for a project no longer covered is let go once it is stale:
/// descriptions are heavy to keep for a window that is closed.
fn request_scope(app: &mut App, project: &ProjectId, wher: Where) {
    let covered = projects_in(app, project, wher);
    app.week_review.fetched.retain(|id, fetched| {
        fetched.at.elapsed() < FRESH || covered.iter().any(|(covered, _, _)| covered == id)
    });
    for (id, name, dir) in covered {
        request_fetch(app, &id, &name, &dir);
        crate::todos::view::load_list(app, dir.clone());
        if !app.linear.contains_key(&id) && crate::linear::key_source(&dir).is_some() {
            crate::linear::request_list(app, id, dir, false);
        }
    }
}

/// Whether GitHub could not be asked about a project `scope` covers: its
/// week is not known, so no review of that scope is the whole week.
fn fetch_failed(app: &App, project: &ProjectId, wher: Where) -> bool {
    projects_in(app, project, wher).iter().any(|(id, _, _)| {
        app.week_review
            .fetched
            .get(id)
            .is_some_and(|f| f.prs.is_none())
    })
}

/// Whether any project `scope` covers is still being fetched.
fn fetching(app: &App, project: &ProjectId, wher: Where) -> bool {
    projects_in(app, project, wher)
        .iter()
        .any(|(id, _, _)| app.week_review.fetching.contains(id))
}

/// An answer landed on the loop.
pub fn land_answer(app: &mut App, answer: Answer) {
    match answer {
        Answer::Fetched { project, prs } => {
            app.week_review.fetching.remove(&project);
            app.week_review.fetched.insert(
                project,
                Fetched {
                    at: Instant::now(),
                    prs,
                },
            );
            // `Enter` was pressed before this landed: the modal that was
            // waiting, if it is still up and still on Compose, sends now.
            let waiting = match &mut app.overlay {
                Some(Overlay::WeekReview(view)) => std::mem::take(&mut view.send_when_fetched),
                _ => false,
            };
            if waiting {
                submit(app);
            }
        }
        Answer::Written { id, result } => land_written(app, id, result),
    }
    app.dirty = true;
}

/// The write came back. A reply is checked, finished, kept and shown; a
/// failure says why and puts Compose back as it was left.
fn land_written(app: &mut App, id: u64, result: Result<Reply, String>) {
    if app.week_review.pending.as_ref().map(|p| p.id) != Some(id) {
        return;
    }
    let Some(pending) = app.week_review.pending.take() else {
        return;
    };
    let reply = match result {
        Ok(reply) => reply,
        Err(why) => {
            app.flash = Some(Flash::failed(format!("Week in review: {why}")));
            if let Some(Overlay::WeekReview(view)) = &mut app.overlay {
                view.mode = Mode::Compose;
                view.notice = Some(why);
            }
            return;
        }
    };
    let (from, to) = period(pending.asked);
    let meta = Meta {
        from: from.format("%Y-%m-%d").to_string(),
        to: to.format("%Y-%m-%d").to_string(),
        scope: pending.scope,
        written_at: pending.asked,
        account: pending.account,
        model: pending.model,
        effort: pending.effort,
        secs: pending.started.elapsed().as_secs(),
        cost: reply.cost,
        problems: check(&reply.text, &pending.material, &pending.people),
        read: false,
        material: pending.material,
    };
    let text = finish(&reply.text, &meta.material);
    let saved = app.week_review.root.clone().and_then(|root| {
        save(&root, &pending.project, &meta, &text)
            .inspect_err(|err| tracing::warn!("week in review not kept: {err}"))
            .ok()
    });
    app.flash = Some(Flash::done("Week in review ready"));
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    if view.project != pending.project {
        return;
    }
    // On disk it is found again by its path; with nowhere to keep it (a
    // test), it is shown from memory.
    let path = saved.unwrap_or_default();
    view.reviews
        .retain(|r| r.path != path || path.as_os_str().is_empty());
    let meta = Meta {
        material: Material::default(),
        ..meta
    };
    view.reviews.insert(0, Saved { path, meta });
    view.mode = Mode::Reviews;
    view.selected = 0;
    view.scroll = 0;
    view.text = text;
    mark_read(app);
}

// ---- the modal ----

/// Which screen the modal is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Choosing the scope and the sources.
    #[default]
    Compose,
    /// The reviews: the one being written, and the ones kept.
    Reviews,
}

/// The Compose panel's rows the cursor rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Whose,
    Where,
    Source(Source),
    Note,
}

impl Row {
    const ALL: [Row; 6] = [
        Row::Whose,
        Row::Where,
        Row::Source(Source::Prs),
        Row::Source(Source::Issues),
        Row::Source(Source::Todos),
        Row::Note,
    ];

    fn step(self, forward: bool) -> Row {
        let at = Row::ALL.iter().position(|r| *r == self).unwrap_or(0);
        let next = if forward {
            (at + 1).min(Row::ALL.len() - 1)
        } else {
            at.saturating_sub(1)
        };
        Row::ALL[next]
    }
}

/// The modal's own state; what must outlive it is on the [`App`]
/// ([`State`]).
#[derive(Debug, Clone)]
pub struct WeekReviewView {
    pub project: ProjectId,
    pub project_name: String,
    pub mode: Mode,
    pub scope: Scope,
    /// Which sources are ticked, by [`Source::index`].
    pub picks: [bool; 3],
    pub note: TextInput,
    pub row: Row,
    /// What Compose has to say: nothing to send, the fetch still out, why
    /// the last write failed.
    pub notice: Option<String>,
    /// `Enter` was pressed before the fetch landed: send when it does.
    /// On the view, so closing the modal or changing what it would send
    /// calls it off.
    pub send_when_fetched: bool,
    /// The reviews kept, newest first.
    pub reviews: Vec<Saved>,
    /// Cursor into the Reviews list — the one being written first, when
    /// one is.
    pub selected: usize,
    /// The selected review's Markdown.
    pub text: String,
    /// Top visible line of the right panel.
    pub scroll: u16,
    pub view_height: u16,
    pub body_lines: usize,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Each Compose row's rect as of the last draw, for the mouse.
    pub rows: Vec<(Rect, Row)>,
}

impl WeekReviewView {
    fn new(project: ProjectId, project_name: String) -> Self {
        Self {
            project,
            project_name,
            mode: Mode::Compose,
            scope: Scope::default(),
            picks: [true; 3],
            note: TextInput::new(),
            row: Row::Whose,
            notice: None,
            send_when_fetched: false,
            reviews: Vec::new(),
            selected: 0,
            text: String::new(),
            scroll: 0,
            view_height: 0,
            body_lines: 0,
            area: Rect::default(),
            rows: Vec::new(),
        }
    }

    fn max_scroll(&self) -> u16 {
        crate::app::max_scroll(self.body_lines, self.view_height)
    }

    fn scroll_by(&mut self, delta: i32) {
        self.scroll = crate::app::scrolled_by(self.scroll, delta, self.max_scroll());
    }

    /// Whether a `Show` is on screen to sweep: what keeps the frame clock
    /// running while the modal is up (`App::status_anim_active`).
    pub fn shimmers(&self) -> bool {
        self.mode == Mode::Reviews && self.text.contains("- Show: ")
    }
}

/// Whether a review of the modal's project is being written.
fn writing(app: &App, view: &WeekReviewView) -> bool {
    app.week_review
        .pending
        .as_ref()
        .is_some_and(|p| p.project == view.project)
}

/// The kept review under the cursor: the Reviews list has the one being
/// written first, so the cursor is one past it while there is one.
fn selected_saved(app: &App, view: &WeekReviewView) -> Option<usize> {
    let offset = usize::from(writing(app, view));
    view.selected
        .checked_sub(offset)
        .filter(|i| *i < view.reviews.len())
}

/// The hotkey: the WEEK IN REVIEW for the selected PROJECT. It opens on
/// the review being written, else on this period's when there is one,
/// else on Compose — and asks GitHub for the week either way.
pub fn open(app: &mut App) {
    let Some(project) = app.selected_project().cloned() else {
        return;
    };
    let mut view = WeekReviewView::new(project.id.clone(), project.name);
    if let Some(root) = &app.week_review.root {
        view.reviews = load_reviews(root, &project.id);
        let note = std::fs::read_to_string(note_path(root, &project.id)).unwrap_or_default();
        view.note.insert_str(note.trim());
    }
    let today = period(orion_core::clock::now_secs() as i64)
        .1
        .format("%Y-%m-%d")
        .to_string();
    let this_week = view.reviews.first().is_some_and(|r| r.meta.to == today);
    let pending = app
        .week_review
        .pending
        .as_ref()
        .is_some_and(|p| p.project == project.id);
    if pending || this_week {
        view.mode = Mode::Reviews;
    }
    app.overlay = Some(Overlay::WeekReview(Box::new(view)));
    request_scope(app, &project.id, Where::Project);
    select(app, 0);
    app.dirty = true;
}

/// Put the Reviews cursor on `index`: the review's text is read, and one
/// shown is no longer new.
fn select(app: &mut App, index: usize) {
    let Some(Overlay::WeekReview(view)) = &app.overlay else {
        return;
    };
    let offset = usize::from(writing(app, view));
    let count = view.reviews.len() + offset;
    let index = index.min(count.saturating_sub(1));
    let path = index
        .checked_sub(offset)
        .and_then(|i| view.reviews.get(i))
        .map(|r| r.path.clone());
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    if view.selected != index || view.text.is_empty() {
        view.scroll = 0;
        match path {
            // One with nowhere to be kept is still in hand.
            Some(path) if path.as_os_str().is_empty() => {}
            Some(path) => view.text = std::fs::read_to_string(path).unwrap_or_default(),
            // The one being written has no text yet.
            None => view.text.clear(),
        }
    }
    view.selected = index;
    mark_read(app);
    app.dirty = true;
}

/// The review under the cursor has been shown: it stops reading `new`,
/// on disk too.
fn mark_read(app: &mut App) {
    let Some(Overlay::WeekReview(view)) = &app.overlay else {
        return;
    };
    if view.mode != Mode::Reviews {
        return;
    }
    let Some(index) = selected_saved(app, view) else {
        return;
    };
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    let saved = &mut view.reviews[index];
    if saved.meta.read {
        return;
    }
    saved.meta.read = true;
    if saved.path.as_os_str().is_empty() {
        return;
    }
    // Only the flag moves on disk: what the review was written from is
    // there and not in hand.
    let json = saved.path.with_extension("json");
    let kept = std::fs::read_to_string(&json)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    if let Some(mut kept) = kept {
        kept["read"] = serde_json::Value::Bool(true);
        let _ = crate::pr_cache::write_json_atomic(&json, &kept);
    }
}

/// `Enter` on Compose: write the review. With the fetch still out it is
/// sent as it lands; with nothing to write from, Compose says so.
fn submit(app: &mut App) {
    let Some(Overlay::WeekReview(view)) = &app.overlay else {
        return;
    };
    if view.mode != Mode::Compose {
        return;
    }
    let (project, name, scope, picks) = (
        view.project.clone(),
        view.project_name.clone(),
        view.scope,
        view.picks,
    );
    let note = view.note.as_str().trim().to_string();
    let notice = |app: &mut App, text: &str| {
        if let Some(Overlay::WeekReview(view)) = &mut app.overlay {
            view.notice = Some(text.to_string());
        }
        app.dirty = true;
    };
    if app.week_review.pending.is_some() {
        return notice(app, "a review is already being written");
    }
    if picks[Source::Prs.index()] && fetching(app, &project, scope.wher) {
        if let Some(Overlay::WeekReview(view)) = &mut app.overlay {
            view.send_when_fetched = true;
        }
        return notice(app, "asking GitHub for the week — it is sent as that lands");
    }
    if picks[Source::Prs.index()] && fetch_failed(app, &project, scope.wher) {
        request_scope(app, &project, scope.wher);
        return notice(
            app,
            "GitHub could not be asked for the week — Enter tries again, or untick the pull requests to write without them",
        );
    }
    // The account it is written through: one of orion's own. A unit test
    // starts nothing, so it needs none.
    let cfg = crate::config::Config::load();
    let through = account(&cfg);
    if app.week_review.tx.is_some() {
        if orion_core::env::non_empty(orion_core::env::AGENT_CMD).is_some() {
            return notice(app, "off while ORION_AGENT_CMD stands in for the agents");
        }
        let Some(through) = &through else {
            return notice(
                app,
                "no Claude account is switched on — Settings → Agents, or Claude accounts in the palette",
            );
        };
        if !crate::config::program_installed(&through.program) {
            return notice(app, &format!("{} is not on your PATH", through.program));
        }
    }
    let through = through.unwrap_or_default();
    let now = orion_core::clock::now_secs() as i64;
    let material = gather(app, &project, scope, picks, now);
    if material.is_empty() {
        return notice(
            app,
            &format!("nothing finished in the last {DAYS} days to write up"),
        );
    }
    if let Some(root) = &app.week_review.root {
        let _ = orion_core::settings::write_atomic(&note_path(root, &project), note.as_bytes());
    }
    let people = match scope.whose {
        Whose::Everyone => legend(&material.prs),
        Whose::Mine => Vec::new(),
    };
    let label = match scope.wher {
        Where::Project => name.clone(),
        Where::All => "all projects".to_string(),
    };
    let text = prompt(&material, &people, scope, &label, &note, now);
    let (model, effort) = cfg.review_launch();
    app.week_review.next_id += 1;
    let id = app.week_review.next_id;
    let task = app.week_review.tx.clone().map(|tx| {
        let (model, effort, through) = (model.clone(), effort.clone(), through.clone());
        tokio::spawn(async move {
            let result = write(text, model, effort, through).await;
            let _ = tx.send(Answer::Written { id, result });
        })
        .abort_handle()
    });
    app.week_review.pending = Some(Pending {
        id,
        project,
        scope,
        material,
        people,
        account: through.label,
        model,
        effort,
        started: Instant::now(),
        asked: now,
        task,
    });
    app.flash = Some(Flash::working("Week in review: writing…"));
    if let Some(Overlay::WeekReview(view)) = &mut app.overlay {
        view.mode = Mode::Reviews;
        view.selected = 0;
        view.scroll = 0;
        view.notice = None;
        view.text.clear();
    }
    app.dirty = true;
}

/// `⌘W` while one is being written: the write is stopped, and Compose
/// comes back as it was left.
fn stop(app: &mut App) {
    let ours = matches!(&app.overlay, Some(Overlay::WeekReview(view)) if writing(app, view));
    if !ours {
        return;
    }
    let Some(pending) = app.week_review.pending.take() else {
        return;
    };
    if let Some(task) = pending.task {
        task.abort();
    }
    app.flash = Some(Flash::note("Week in review stopped"));
    if let Some(Overlay::WeekReview(view)) = &mut app.overlay {
        view.mode = Mode::Compose;
        view.selected = 0;
    }
    app.dirty = true;
}

/// `⌘R`: write the review under the cursor again — its scope, today's
/// week.
fn rewrite(app: &mut App) {
    let Some(Overlay::WeekReview(view)) = &app.overlay else {
        return;
    };
    let scope = selected_saved(app, view).map(|i| view.reviews[i].meta.scope);
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    if let Some(scope) = scope {
        view.scope = scope;
    }
    view.mode = Mode::Compose;
    let (project, wher) = (view.project.clone(), view.scope.wher);
    request_scope(app, &project, wher);
    submit(app);
}

/// The modal's own keys: one table [`handle_key`] matches and [`hints`]
/// spells.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const SEND: Key = Key::new(&["enter"], "write it");
    pub const TICK: Key = Key::new(&["space"], "tick");
    /// The TODOS MODAL's copy.
    pub const COPY: Key = Key::new(&["cmd+c", "ctrl+y"], "copy as Markdown");
    pub const OPEN: Key = Key::new(&["cmd+o", "ctrl+o"], "open the file");
    pub const AGAIN: Key = Key::new(&["cmd+r", "ctrl+r"], "write it again");
    pub const NEW: Key = Key::new(&["cmd+n", "ctrl+n"], "a new one");
    pub const STOP: Key = Key::new(&["cmd+w", "ctrl+w"], "stop");
}

/// What the modal's bottom border says its keys are.
fn hints(app: &App, view: &WeekReviewView) -> Vec<Hint> {
    match view.mode {
        Mode::Compose => {
            let mut hints = vec![keys::SEND.hint().kept()];
            match view.row {
                Row::Whose | Row::Where => hints.push(Hint::new("←/→", "choose")),
                Row::Source(_) => hints.push(keys::TICK.hint()),
                Row::Note => {}
            }
            if !view.reviews.is_empty() || writing(app, view) {
                hints.push(Hint::new("Tab", "reviews"));
            }
            hints.push(crate::hints::ESC_CLOSE.hint());
            hints
        }
        Mode::Reviews if selected_saved(app, view).is_none() => vec![
            keys::STOP.hint().kept(),
            Hint::new("↑/↓", "reviews"),
            crate::hints::ESC_CLOSE.hint(),
        ],
        Mode::Reviews => vec![
            keys::COPY.hint().kept(),
            keys::OPEN.hint(),
            keys::AGAIN.hint(),
            keys::NEW.hint(),
            Hint::new("↑/↓", "reviews"),
            Hint::new("⇧↑/⇧↓", "scroll"),
            crate::hints::ESC_CLOSE.hint(),
        ],
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    if key.code == KeyCode::Esc {
        app.overlay = None;
        app.dirty = true;
        return;
    }
    app.dirty = true;
    match view.mode {
        Mode::Compose => compose_key(app, key),
        Mode::Reviews => reviews_key(app, key),
    }
}

fn compose_key(app: &mut App, key: KeyEvent) {
    // There is a list to go to: a review kept, or one being written.
    let has_reviews = matches!(
        &app.overlay,
        Some(Overlay::WeekReview(view)) if !view.reviews.is_empty() || writing(app, view)
    );
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    let on_note = view.row == Row::Note;
    match key.code {
        KeyCode::Enter => submit(app),
        KeyCode::Down => view.row = view.row.step(true),
        KeyCode::Up => view.row = view.row.step(false),
        KeyCode::Tab | KeyCode::BackTab if has_reviews => {
            view.mode = Mode::Reviews;
            view.text.clear();
            select(app, 0);
        }
        KeyCode::PageDown => view.scroll_by(i32::from(view.view_height.max(1))),
        KeyCode::PageUp => view.scroll_by(-i32::from(view.view_height.max(1))),
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if !on_note => {
            toggle(app, view_row(app));
        }
        _ if on_note && view.note.handle_key(&key).changed() => view.notice = None,
        _ => {}
    }
}

/// The Compose row under the cursor.
fn view_row(app: &App) -> Row {
    match &app.overlay {
        Some(Overlay::WeekReview(view)) => view.row,
        _ => Row::Whose,
    }
}

/// Flip `row`: a scope row to its other choice, a source in or out. A
/// wider `Where` asks GitHub about the projects it now covers.
fn toggle(app: &mut App, row: Row) {
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    view.notice = None;
    view.send_when_fetched = false;
    view.scroll = 0;
    match row {
        Row::Whose => {
            view.scope.whose = match view.scope.whose {
                Whose::Mine => Whose::Everyone,
                Whose::Everyone => Whose::Mine,
            }
        }
        Row::Where => {
            view.scope.wher = match view.scope.wher {
                Where::Project => Where::All,
                Where::All => Where::Project,
            };
            let (project, wher) = (view.project.clone(), view.scope.wher);
            request_scope(app, &project, wher);
        }
        Row::Source(source) => view.picks[source.index()] ^= true,
        Row::Note => {}
    }
    app.dirty = true;
}

fn reviews_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let page = i32::from(view.view_height.max(1));
    let at = view.selected;
    match key.code {
        KeyCode::Down if shift => view.scroll_by(1),
        KeyCode::Up if shift => view.scroll_by(-1),
        KeyCode::PageDown => view.scroll_by(page),
        KeyCode::PageUp => view.scroll_by(-page),
        KeyCode::Home => view.scroll = 0,
        KeyCode::End => view.scroll = view.max_scroll(),
        KeyCode::Down => select(app, at + 1),
        KeyCode::Up => select(app, at.saturating_sub(1)),
        _ if keys::STOP.matches(&key) => stop(app),
        _ if keys::NEW.matches(&key) => {
            view.mode = Mode::Compose;
            view.scroll = 0;
        }
        _ if keys::AGAIN.matches(&key) => rewrite(app),
        _ if keys::COPY.matches(&key) => {
            let text = view.text.clone();
            if !text.is_empty() {
                crate::event_loop::copy_and_flash(app, &text, "Copied the review as Markdown");
            }
        }
        _ if keys::OPEN.matches(&key) => {
            let path = selected_path(app);
            if let Some((dir, file)) = path.as_ref().and_then(|p| {
                Some((
                    p.parent()?.to_path_buf(),
                    p.file_name()?.to_str()?.to_string(),
                ))
            }) {
                crate::event_loop::open_file_outside(app, &dir, &file, 0);
            }
        }
        _ => {}
    }
}

/// The file of the review under the cursor, when it is kept.
fn selected_path(app: &App) -> Option<PathBuf> {
    let Some(Overlay::WeekReview(view)) = &app.overlay else {
        return None;
    };
    let path = &view.reviews[selected_saved(app, view)?].path;
    (!path.as_os_str().is_empty()).then(|| path.clone())
}

/// A paste lands in the note.
pub fn paste(app: &mut App, text: &str) {
    if let Some(Overlay::WeekReview(view)) = &mut app.overlay {
        if view.mode == Mode::Compose {
            view.row = Row::Note;
            view.note.insert_str(&text.replace('\n', " "));
            app.dirty = true;
        }
    }
}

/// The wheel scrolls the right panel; a click on a Compose row puts the
/// cursor there and flips it. (A click outside is Esc, as on every
/// modal: `overlay_close`.)
pub fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let Some(Overlay::WeekReview(view)) = &mut app.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollDown => view.scroll_by(3),
        MouseEventKind::ScrollUp => view.scroll_by(-3),
        MouseEventKind::Down(MouseButton::Left) if view.mode == Mode::Compose => {
            let at = Position::new(mouse.column, mouse.row);
            if let Some(&(_, row)) = view.rows.iter().find(|(r, _)| r.contains(at)) {
                view.row = row;
                toggle(app, row);
            }
        }
        _ => {}
    }
    app.dirty = true;
}

// ---- drawing ----

/// The left panel's share of the modal, and its floor.
const LIST_PCT: u16 = 36;
const MIN_LIST_W: u16 = 30;
/// The gap between a review's columns.
const GAP: usize = 2;

/// `text` wrapped to `width`, never narrower than a few words.
fn wrap(text: &str, width: usize) -> Vec<String> {
    crate::pr_preview::wrap(text, width.max(8))
}

/// A review as the right panel draws it. A point's owner and its pull
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

/// How many of `source` a review of `scope` would take, as Compose's
/// count column says it: the number, `asking…` while GitHub is out, or
/// why there is none to count.
fn count_label(app: &App, view: &WeekReviewView, source: Source, material: &Material) -> String {
    match source {
        Source::Prs if fetching(app, &view.project, view.scope.wher) => "asking…".into(),
        Source::Prs if fetch_failed(app, &view.project, view.scope.wher) => "couldn't ask".into(),
        Source::Todos if view.scope.whose == Whose::Everyone => "yours only".into(),
        Source::Prs => material.prs.len().to_string(),
        Source::Issues => material.issues.len().to_string(),
        Source::Todos => material.todos.len().to_string(),
    }
}

/// `(•) this   ( ) that`: a scope row's two choices.
fn choice_spans(first: &str, second: &str, on_first: bool, th: Theme) -> Vec<Span<'static>> {
    let option = |name: &str, on: bool| {
        let (mark, color) = if on {
            ("(•) ", th.accent)
        } else {
            ("( ) ", th.dim)
        };
        vec![
            Span::styled(mark, Style::default().fg(color)),
            Span::styled(
                name.to_string(),
                Style::default().fg(if on { th.text } else { th.muted }),
            ),
        ]
    };
    let mut spans = option(first, on_first);
    spans.push(Span::raw("   "));
    spans.extend(option(second, !on_first));
    spans
}

/// The WEEK IN REVIEW: scope, sources and note — or the reviews — down
/// the left, and on the right what would be sent, what is being written,
/// or the review itself.
pub fn draw(f: &mut Frame, app: &mut App, view: &WeekReviewView, th: Theme) {
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let list_w = (area.width * LIST_PCT / 100)
        .max(MIN_LIST_W)
        .min(area.width.saturating_sub(crate::ui::SPLIT_PANE_LAYOUT_MIN));
    let [left_a, right_a] = Layout::horizontal([
        Constraint::Length(list_w),
        Constraint::Min(crate::ui::SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);
    let now = orion_core::clock::now_secs() as i64;
    let title = format!("Week in review — {}", view.project_name);
    let compose = view.mode == Mode::Compose;
    let left_block = panel_block(&title, compose, th);
    let left = left_block.inner(left_a);
    f.render_widget(left_block, left_a);
    let mut rows = Vec::new();

    let (right_title, lines) = if compose {
        let material = gather(app, &view.project, view.scope, view.picks, now);
        draw_compose(f, app, view, &material, left, &mut rows, now, th);
        let right_w = usize::from(right_a.width.saturating_sub(4));
        let mut lines = item_lines(&material, now, right_w, th);
        if lines.is_empty() {
            let text = if fetching(app, &view.project, view.scope.wher) {
                "asking GitHub for the week…"
            } else {
                "nothing finished in the week"
            };
            lines.push(Line::from(Span::styled(text, Style::default().fg(th.dim))));
        }
        (
            format!(
                "What goes in · {} · {}",
                plural(material.len(), "item", "items"),
                view.scope.label()
            ),
            lines,
        )
    } else {
        draw_reviews(f, app, view, left, now, th);
        let right_w = usize::from(right_a.width.saturating_sub(4));
        match (selected_saved(app, view), &app.week_review.pending) {
            (Some(index), _) => {
                let meta = &view.reviews[index].meta;
                let phase = app.animations.then(|| app.sweep_phase());
                (
                    format!("{} · {}", meta.period(), meta.scope.label()),
                    review_lines(&view.text, &meta.problems, right_w, phase, th),
                )
            }
            (None, Some(pending)) => (
                format!("{} · writing", period_short(pending.asked)),
                writing_lines(app, pending, right_w, th),
            ),
            (None, None) => ("Week in review".to_string(), Vec::new()),
        }
    };

    let mut right_block = panel_block(&right_title, !compose, th);
    let right_inner = right_block.inner(right_a);
    let body = Rect {
        x: right_inner.x + 1,
        y: right_inner.y + 1.min(right_inner.height),
        width: right_inner.width.saturating_sub(2),
        height: right_inner.height.saturating_sub(1),
    };
    let max_scroll = (lines.len() as u16).saturating_sub(body.height.max(1));
    let scroll = view.scroll.min(max_scroll);
    if max_scroll > 0 {
        right_block = right_block.title_bottom(
            Line::from(Span::styled(
                format!(" {}/{} ", scroll + 1, lines.len()),
                Style::default().fg(th.dim),
            ))
            .right_aligned(),
        );
    }
    f.render_widget(right_block, right_a);
    let total = lines.len();
    let shown: Vec<Line> = lines.into_iter().skip(usize::from(scroll)).collect();
    f.render_widget(Paragraph::new(shown), body);
    let reserve = if max_scroll > 0 { 12 } else { 0 };
    crate::hints::draw_on_border(f, area, &hints(app, view), reserve, th);

    if let Some(Overlay::WeekReview(live)) = &mut app.overlay {
        live.area = area;
        live.rows = rows;
        live.view_height = body.height;
        live.body_lines = total;
        live.scroll = scroll;
    }
}

/// `4 – 10 Oct` for a review asked at `asked`.
fn period_short(asked: i64) -> String {
    let (from, to) = period(asked);
    period_label(from, to, false)
}

/// The Compose panel: the period, the scope's two rows, the sources with
/// their counts, the model, the note, and whatever there is to say.
#[allow(clippy::too_many_arguments)]
fn draw_compose(
    f: &mut Frame,
    app: &App,
    view: &WeekReviewView,
    material: &Material,
    area: Rect,
    rows: &mut Vec<(Rect, Row)>,
    now: i64,
    th: Theme,
) {
    let width = usize::from(area.width);
    let (from, to) = period(now);
    let rule = |name: &str| crate::ui::section_rule(name, th.muted, Vec::new(), width, th);
    let label = |name: &str| Span::styled(format!("{name:<7}"), Style::default().fg(th.muted));
    let cfg = crate::config::Config::load();
    let (model, effort) = cfg.review_launch();
    let mut out: Vec<(Option<Row>, Line<'static>)> = Vec::new();
    let period = period_label(from, to, false);
    let last = format!("Last {DAYS} days");
    out.push((
        None,
        Line::from(vec![
            Span::raw("  "),
            Span::styled(last.clone(), Style::default().fg(th.text)),
            Span::raw(" ".repeat(
                width.saturating_sub(2 + last.chars().count() + period.chars().count() + 1),
            )),
            Span::styled(period, Style::default().fg(th.muted)),
        ]),
    ));
    out.push((None, Line::default()));
    out.push((None, rule("Scope")));
    let mut whose = vec![Span::raw("  "), label("Whose")];
    whose.extend(choice_spans(
        "Just mine",
        "Everyone",
        view.scope.whose == Whose::Mine,
        th,
    ));
    out.push((Some(Row::Whose), Line::from(whose)));
    let mut wher = vec![Span::raw("  "), label("Where")];
    wher.extend(choice_spans(
        "This project",
        "All projects",
        view.scope.wher == Where::Project,
        th,
    ));
    out.push((Some(Row::Where), Line::from(wher)));
    out.push((None, Line::default()));
    out.push((None, rule("Sources")));
    for source in Source::ALL {
        let on = view.picks[source.index()];
        let count = count_label(app, view, source, material);
        let used = 2 + 4 + source.label().chars().count();
        out.push((
            Some(Row::Source(source)),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    if on { "[✓] " } else { "[ ] " },
                    Style::default().fg(if on { th.accent } else { th.dim }),
                ),
                Span::styled(
                    source.label(),
                    Style::default().fg(if on { th.text } else { th.muted }),
                ),
                Span::raw(" ".repeat(width.saturating_sub(used + count.chars().count() + 1))),
                Span::styled(
                    count,
                    Style::default().fg(if on { th.text } else { th.dim }),
                ),
            ]),
        ));
    }
    out.push((None, Line::default()));
    out.push((None, rule("Agent")));
    out.push((
        None,
        Line::from(vec![
            Span::raw("  "),
            label("Model"),
            Span::styled(model, Style::default().fg(th.text)),
            Span::styled(" · ", Style::default().fg(th.dim)),
            Span::styled(format!("{effort} effort"), Style::default().fg(th.text)),
        ]),
    ));
    let through = match account(&cfg) {
        Some(through) => Span::styled(
            truncate(&through.label, width.saturating_sub(2 + 7 + 1)),
            Style::default().fg(th.text),
        ),
        None => Span::styled("no Claude account is on", Style::default().fg(th.warn)),
    };
    out.push((
        None,
        Line::from(vec![Span::raw("  "), label("Via"), through]),
    ));
    out.push((None, Line::default()));
    out.push((Some(Row::Note), Line::default()));
    if let Some(notice) = &view.notice {
        out.push((None, Line::default()));
        for part in wrap(notice, width.saturating_sub(3)) {
            out.push((
                None,
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(part, Style::default().fg(th.warn)),
                ]),
            ));
        }
    }
    for (i, (row, line)) in out.into_iter().enumerate() {
        let Some(rect) = row_rect(area, i + 1) else {
            break;
        };
        let on = row.is_some_and(|r| r == view.row);
        // The field's own inset is one cell short of the rows above it.
        let field = Rect {
            x: rect.x + 1,
            width: rect.width.saturating_sub(1),
            ..rect
        };
        let line = match row {
            Some(Row::Note) => {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(crate::ui::form_field(
                    "Note",
                    &view.note,
                    "what matters here — kept for this project",
                    on,
                    field,
                    th,
                ));
                Line::from(spans)
            }
            _ => line,
        };
        let mut paragraph = Paragraph::new(line);
        if on {
            paragraph = paragraph.style(Style::default().bg(th.focus_tint));
        }
        f.render_widget(paragraph, rect);
        if on {
            f.render_widget(
                Paragraph::new(Span::styled("▌", Style::default().fg(th.accent))),
                Rect { width: 1, ..rect },
            );
        }
        if let Some(row) = row {
            rows.push((rect, row));
        }
    }
}

/// The Reviews panel: the one being written, then the ones kept — each
/// with its period and whose it is — and, for one being written, what it
/// includes.
fn draw_reviews(f: &mut Frame, app: &App, view: &WeekReviewView, area: Rect, now: i64, th: Theme) {
    let width = usize::from(area.width);
    let mut lines: Vec<Line<'static>> = vec![Line::default()];
    let pending = app
        .week_review
        .pending
        .as_ref()
        .filter(|p| p.project == view.project);
    let row = |mark: Span<'static>, name: String, right: Span<'static>, on: bool| {
        let cursor = if on {
            Span::styled("▌ ", Style::default().fg(th.accent))
        } else {
            Span::raw("  ")
        };
        let used = 2 + 2 + name.chars().count() + right.content.chars().count() + 1;
        let line = Line::from(vec![
            cursor,
            mark,
            Span::raw(" "),
            Span::styled(
                name,
                Style::default().fg(if on { th.text } else { th.muted }),
            ),
            Span::raw(" ".repeat(width.saturating_sub(used))),
            right,
        ]);
        if on {
            line.style(Style::default().bg(th.focus_tint))
        } else {
            line
        }
    };
    let mut index = 0;
    if let Some(pending) = pending {
        let glyph = crate::app::spinner_frame(app.spin_phase());
        lines.push(row(
            Span::styled(glyph, Style::default().fg(th.warn)),
            format!("{} · {}", period_short(pending.asked), pending.scope.slug()),
            Span::styled("writing…", Style::default().fg(th.warn)),
            view.selected == index,
        ));
        index += 1;
    }
    for saved in &view.reviews {
        let age = crate::hosts::ago_short(now - saved.meta.written_at);
        let (mark, right) = if saved.meta.read {
            (th.dim, Span::styled(age, Style::default().fg(th.dim)))
        } else {
            (th.done, Span::styled("new", Style::default().fg(th.done)))
        };
        let all = match saved.meta.scope.wher {
            Where::All => " · all",
            Where::Project => "",
        };
        lines.push(row(
            Span::styled("●", Style::default().fg(mark)),
            format!("{} · {}{all}", saved.meta.period(), saved.meta.scope.slug()),
            right,
            view.selected == index,
        ));
        index += 1;
    }
    if let (Some(pending), None) = (pending, selected_saved(app, view)) {
        lines.push(Line::default());
        lines.push(crate::ui::section_rule(
            "Included",
            th.muted,
            Vec::new(),
            width,
            th,
        ));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(pending.scope.label(), Style::default().fg(th.text)),
        ]));
        lines.push(Line::default());
        lines.extend(item_lines(
            &pending.material,
            now,
            width.saturating_sub(1),
            th,
        ));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// The right panel while a review is being written: what it is doing,
/// for how long, and that the window can be closed.
fn writing_lines(app: &App, pending: &Pending, width: usize, th: Theme) -> Vec<Line<'static>> {
    let glyph = crate::app::spinner_frame(app.spin_phase());
    let took = format!("{}s", pending.started.elapsed().as_secs());
    let head = format!("Writing your week · {} · {}", pending.model, pending.effort);
    let through = match pending.account.as_str() {
        "" => String::new(),
        name => format!("through {name}"),
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{glyph} "), Style::default().fg(th.warn)),
            Span::styled(head.clone(), Style::default().fg(th.text)),
            Span::raw(
                " ".repeat(width.saturating_sub(2 + head.chars().count() + took.chars().count())),
            ),
            Span::styled(took, Style::default().fg(th.muted)),
        ]),
        Line::default(),
        Line::from(vec![
            Span::styled("  ✓ ", Style::default().fg(th.ok)),
            Span::styled(
                format!(
                    "read {}",
                    plural(
                        pending.material.prs.len(),
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
        "You can close this. The footer says when the review is ready, and this window opens on it.",
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

    const DAY: i64 = 24 * 60 * 60;

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
            body: format!("## Summary\nWhat change {number} does."),
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
            project.clone(),
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
        let now = rfc3339_secs("2026-10-10T10:00:00Z").unwrap();
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
        let read = parse_page(&page(true, "2026-10-08T09:00:00Z"), "demo", now).expect("a page");
        assert_eq!(read.rows.len(), 1, "the bot's and the old one are left out");
        let pr = &read.rows[0];
        assert_eq!(
            (pr.number, pr.login.as_str(), pr.name.as_str()),
            (40, "ada", "Ada L")
        );
        assert!(pr.mine);
        assert_eq!((pr.additions, pr.files, pr.base.as_str()), (120, 3, "main"));
        assert_eq!(read.more.as_deref(), Some("c1"), "the week may run on");
        let last = parse_page(&page(true, "2026-09-30T09:00:00Z"), "demo", now).expect("a page");
        assert_eq!(last.more, None, "past the week");
        let end = parse_page(&page(false, "2026-10-08T09:00:00Z"), "demo", now).expect("a page");
        assert_eq!(end.more, None, "GitHub has no more");
        assert!(parse_page("{}", "demo", now).is_none());
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
        big.body = long.clone();
        let mut small = pr(501, "grace", "Grace", false, now);
        small.body = format!("{long}\n<!-- bot footer -->\nnever sent");
        let mut release = pr(502, "ada", "Ada L", true, now);
        release.head = "dev".into();
        release.additions = 90000;
        release.body = long.clone();
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
        let text = prompt(
            &material,
            &people,
            everyone,
            "demo",
            "lead with search",
            now,
        );
        assert!(
            text.contains("People, by initials: AL Ada L · GR Grace."),
            "{text}"
        );
        assert!(text.contains("#500 · AL · +9000/-2 in 1 files · into main · Change 500"));
        assert!(text.contains("#501 · GR · "));
        assert!(text.contains("#502 · AL · +90000/-2 in 1 files · into main · RELEASE · "));
        assert!(text.contains("1. <what changed> · <owner> · #412"));
        assert!(text.contains("## Note from the person asking\n\nlead with search"));
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

        let mine = prompt(&material, &[], Scope::default(), "demo", "", now);
        assert!(!mine.contains("People, by initials"));
        assert!(!mine.contains("<owner>"));
        assert!(mine.contains("1. <what changed> · #412"));
        assert!(!mine.contains("Note from the person asking"));
        assert!(mine.contains("#500 · +9000"));
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

    /// A review is written through a Claude account orion runs: the
    /// default agent's, with the config dir that pins it; the **Review
    /// account** row's when one is picked; and, where the default agent is
    /// not Claude at all, the first Claude account that is on.
    #[test]
    fn a_review_is_written_through_one_of_orions_claude_accounts() {
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
            let through = account(&cfg).expect("the default agent's account");
            assert_eq!(through.program, "claude");
            assert_eq!(pinned(&through), Some(work.display().to_string()));
            assert!(through.label.starts_with("Work"), "{}", through.label);

            cfg.review_account = "claude".into();
            let through = account(&cfg).expect("the one picked");
            assert_eq!(pinned(&through), None, "built-in Claude, its own sign-in");

            cfg.review_account = "gone".into();
            assert!(
                pinned(&account(&cfg).unwrap()).is_some(),
                "back to the default agent's"
            );

            cfg.review_account.clear();
            cfg.quick_prompt_kind = "codex".into();
            assert_eq!(
                account(&cfg).expect("a Claude account that is on").program,
                "claude"
            );
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

    /// Compose: the scope rows flip with ←/→ and the sources tick with
    /// Space, the right panel following; Enter with nothing ticked sends
    /// nothing and says why.
    #[test]
    fn compose_sets_the_scope_and_the_sources() {
        pinned(|| {
            let (mut app, _, _) = app_with_week();
            open(&mut app);
            assert_eq!(view(&app).mode, Mode::Compose);
            let shot = screen(&mut app, 150, 40);
            assert!(shot.contains("Week in review — demo"), "{shot}");
            assert!(
                shot.contains("What goes in · 2 items · just mine · this project"),
                "{shot}"
            );
            assert!(shot.contains("↗ #412  Change 412"), "{shot}");
            assert!(!shot.contains("#398"), "Grace's is not yours: {shot}");

            handle_key(&mut app, key(KeyCode::Right));
            assert_eq!(view(&app).scope.whose, Whose::Everyone);
            let shot = screen(&mut app, 150, 40);
            assert!(shot.contains("What goes in · 3 items · everyone"), "{shot}");
            assert!(
                shot.contains("yours only"),
                "the todos row says why: {shot}"
            );

            for _ in 0..2 {
                handle_key(&mut app, key(KeyCode::Down));
            }
            assert_eq!(view(&app).row, Row::Source(Source::Prs));
            handle_key(&mut app, key(KeyCode::Char(' ')));
            assert!(!view(&app).picks[0]);
            handle_key(&mut app, key(KeyCode::Enter));
            assert!(app.week_review.pending.is_none(), "nothing to write from");
            assert_eq!(
                view(&app).notice.as_deref(),
                Some("nothing finished in the last 7 days to write up")
            );
        });
    }

    /// Enter writes: the modal turns to the Reviews with this one first,
    /// what it includes listed under it. The reply lands checked, with
    /// orion's count, kept on disk, and shown — and the file is found
    /// again the next time the modal opens.
    #[test]
    fn a_review_is_written_kept_and_shown() {
        pinned(|| {
            let (mut app, project, _) = app_with_week();
            let dir = tempfile::tempdir().unwrap();
            app.week_review.root = Some(dir.path().to_path_buf());
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Right));
            handle_key(&mut app, key(KeyCode::Enter));
            let pending = app.week_review.pending.as_ref().expect("being written");
            assert_eq!(pending.material.prs.len(), 3);
            assert_eq!(pending.people.len(), 2);
            let id = pending.id;
            assert_eq!(view(&app).mode, Mode::Reviews);
            let shot = screen(&mut app, 150, 40);
            assert!(shot.contains("writing…"), "{shot}");
            assert!(shot.contains("Included"), "{shot}");
            assert!(shot.contains("everyone · this project"), "{shot}");
            assert!(
                shot.contains("#398  Change 398"),
                "the items, not a count: {shot}"
            );
            assert!(shot.contains("read 3 pull request descriptions"), "{shot}");

            // An answer to a write stopped since changes nothing.
            land_answer(
                &mut app,
                Answer::Written {
                    id: id + 7,
                    result: Err("stale".into()),
                },
            );
            assert!(app.week_review.pending.is_some());

            let two = REVIEW.replace(
                "\n## Speed · GR\n1. Large documents open at once · GR · #398\n",
                "",
            );
            land_answer(
                &mut app,
                Answer::Written {
                    id,
                    result: Ok(Reply {
                        text: two,
                        cost: Some(0.28),
                    }),
                },
            );
            assert!(app.week_review.pending.is_none());
            assert_eq!(app.flash.as_deref(), Some("Week in review ready"));
            let shown = view(&app);
            assert_eq!(shown.reviews.len(), 1);
            assert!(shown.reviews[0].meta.problems.is_empty());
            assert!(shown
                .text
                .ends_with("+ 1 other pull request merged, not listed\n"));
            let shot = screen(&mut app, 150, 40);
            assert!(shot.contains("Borders draw with a pen"), "{shot}");
            assert!(
                !shot.contains("Included"),
                "that panel is for the wait: {shot}"
            );

            app.overlay = None;
            open(&mut app);
            let again = view(&app);
            assert_eq!(again.mode, Mode::Reviews, "this week's is there to read");
            assert_eq!(again.reviews.len(), 1);
            assert!(again.reviews[0].meta.read, "shown once, no longer new");
            assert!(again.text.contains("Borders draw with a pen"));
            assert_eq!(again.reviews[0].meta.scope.whose, Whose::Everyone);
            assert!(folder(dir.path(), &project, Where::Project)
                .join("note.txt")
                .is_file());
        });
    }

    /// A write that fails says why and gives Compose back as it was left.
    #[test]
    fn a_failed_write_says_why() {
        pinned(|| {
            let (mut app, _, _) = app_with_week();
            open(&mut app);
            handle_key(&mut app, key(KeyCode::Enter));
            let id = app.week_review.pending.as_ref().unwrap().id;
            land_answer(
                &mut app,
                Answer::Written {
                    id,
                    result: Err("claude timed out".into()),
                },
            );
            assert!(app.week_review.pending.is_none());
            assert_eq!(view(&app).mode, Mode::Compose);
            assert_eq!(view(&app).notice.as_deref(), Some("claude timed out"));
        });
    }

    /// A week GitHub could not be asked about is never written around: a
    /// review of part of a week would pass it off as the whole. Unticking
    /// the pull requests is the way to write without them.
    #[test]
    fn a_week_that_could_not_be_fetched_is_not_written_up() {
        pinned(|| {
            let (mut app, project, now) = app_with_week();
            app.week_review.fetched.get_mut(&project).unwrap().prs = None;
            app.linear.insert(
                project.clone(),
                crate::linear::LinearList {
                    list: vec![issue("1", true, now, 1)],
                    ..Default::default()
                },
            );
            open(&mut app);
            let shot = screen(&mut app, 150, 40);
            assert!(shot.contains("couldn't ask"), "{shot}");
            handle_key(&mut app, key(KeyCode::Enter));
            assert!(app.week_review.pending.is_none());
            assert!(view(&app)
                .notice
                .as_deref()
                .unwrap()
                .contains("could not be asked"));
            // ⌘W here stops nothing: there is nothing of this project's to stop.
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            );
            for _ in 0..2 {
                handle_key(&mut app, key(KeyCode::Down));
            }
            handle_key(&mut app, key(KeyCode::Char(' ')));
            handle_key(&mut app, key(KeyCode::Enter));
            let pending = app
                .week_review
                .pending
                .as_ref()
                .expect("written from the issue");
            assert!(pending.material.prs.is_empty());
            assert_eq!(pending.material.issues.len(), 1);
            // Tab on Compose goes back to the one being written.
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            );
            assert_eq!(view(&app).mode, Mode::Compose);
            handle_key(&mut app, key(KeyCode::Tab));
            assert_eq!(view(&app).mode, Mode::Reviews);
        });
    }

    /// Enter before the fetch has landed sends the moment it does.
    #[test]
    fn enter_before_the_fetch_sends_as_it_lands() {
        pinned(|| {
            let (mut app, project, now) = app_with_week();
            let prs = app.week_review.fetched.remove(&project).unwrap().prs;
            app.week_review.fetching.insert(project.clone());
            open(&mut app);
            let shot = screen(&mut app, 150, 40);
            assert!(shot.contains("asking…"), "{shot}");
            handle_key(&mut app, key(KeyCode::Enter));
            assert!(app.week_review.pending.is_none());
            assert!(view(&app).notice.is_some());
            land_answer(&mut app, Answer::Fetched { project, prs });
            assert!(app.week_review.pending.is_some(), "sent as the week landed");
            let _ = now;
        });
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

    /// End to end against the real GitHub and the real model, for a
    /// checkout named by `ORION_WEEK_REVIEW_LIVE`: fetch its week, write
    /// the review, hold it to its shape. Never run by the suite.
    #[tokio::test]
    #[ignore = "asks GitHub and a model: set ORION_WEEK_REVIEW_LIVE to a checkout"]
    async fn live_a_real_week_is_written_and_holds_its_shape() {
        let Some(dir) = orion_core::env::non_empty("ORION_WEEK_REVIEW_LIVE") else {
            panic!("set ORION_WEEK_REVIEW_LIVE to a checkout");
        };
        let everyone = std::env::var("ORION_WEEK_REVIEW_SCOPE").as_deref() != Ok("mine");
        let now = orion_core::clock::now_secs() as i64;
        let started = Instant::now();
        let prs = fetch_merged(PathBuf::from(&dir), "live".into(), now)
            .await
            .expect("GitHub answered");
        let fetched = started.elapsed();
        let scope = Scope {
            whose: if everyone {
                Whose::Everyone
            } else {
                Whose::Mine
            },
            ..Scope::default()
        };
        let material = Material {
            prs: prs.into_iter().filter(|pr| everyone || pr.mine).collect(),
            ..Default::default()
        };
        let people = if everyone {
            legend(&material.prs)
        } else {
            Vec::new()
        };
        let text = prompt(&material, &people, scope, "live", "", now);
        let started = Instant::now();
        let mut cfg = crate::config::Config::load();
        if let Some(id) = orion_core::env::non_empty("ORION_WEEK_REVIEW_ACCOUNT") {
            cfg.review_account = id;
        }
        let through = account(&cfg).expect("a Claude account is on");
        println!("through {} ({})", through.label, through.program);
        let reply = write(text.clone(), MODELS[0].into(), EFFORTS[0].into(), through)
            .await
            .expect("the model answered");
        let review = finish(&reply.text, &material);
        println!(
            "{} pull requests, prompt {} bytes, fetched in {:?}, written in {:?}, cost {:?}\n\n{review}",
            material.prs.len(),
            text.len(),
            fetched,
            started.elapsed(),
            reply.cost
        );
        let problems = check(&reply.text, &material, &people);
        assert!(problems.is_empty(), "{problems:?}");
    }
}
