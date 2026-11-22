//! LINEAR through the RESOLVER: which Linear keys are on screen or needed
//! by an automation, the fetch that answers each, and where its answer
//! goes.
//!
//! | Key | Asked | Lands in |
//! |---|---|---|
//! | `LinearView(project)` | the selected project (a prefetch, 5 min), the open LINEAR VIEW (1 min) | `linear::land_list` |
//! | `LinearTeams(tag)` | the open LINEAR VIEW's key, once a launch | `linear::land_teams` |
//! | `LinearInbox(tag)` | each key while the NEEDS YOU board is open (1 min) | `needs_you::land` |
//! | `LinearIntake(tag)` | each key every 5 min while the bug fixer or the feature builder is on | `linear_builder::intake_landed` |
//!
//! A key's `tag` is [`crate::linear::key_tag`]: never the key itself.
//! Which key each project reads is worked out at most every
//! [`KEYS_TTL`] ([`Keys`]) — the scope is asked after every turn of the
//! loop, and must not stat every project's `.env` each keypress.
//!
//! Issue comments keep their own reads (`linear::schedule_comments`): one
//! per issue the cursor rests on, a minute fresh, one at a time — what
//! the resolver would do, already.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use orion_core::ProjectId;

use super::spawn::Landed;
use super::{gate, FetchError, QueryKey, Seen};
use crate::app::{App, Overlay};
use crate::fetch::Ticket;
use crate::linear::{LinearIssue, LinearList, Teams};

/// How long which key each project reads is trusted before the env files
/// are looked at again.
const KEYS_TTL: Duration = Duration::from_secs(30);

/// One Linear key in use: its tag, the key, and the projects that read it
/// (in project order; the first is where a key's NEEDS YOU answers are
/// kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyUse {
    pub tag: String,
    pub key: String,
    pub projects: Vec<ProjectId>,
}

/// Which Linear key each project reads, as of the last look (at most
/// every [`KEYS_TTL`], and again whenever the projects change).
#[derive(Debug, Default)]
pub struct Keys {
    memo: RefCell<Option<Memo>>,
}

#[derive(Debug)]
struct Memo {
    at: Instant,
    projects: Vec<(ProjectId, PathBuf)>,
    uses: Vec<KeyUse>,
}

impl Keys {
    /// Every key in use, as of `now`.
    pub fn uses(&self, app: &App, now: Instant) -> Vec<KeyUse> {
        let projects: Vec<(ProjectId, PathBuf)> = app
            .tree
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.repo_path.clone()))
            .collect();
        if let Some(memo) = self.memo.borrow().as_ref() {
            if memo.projects == projects && now.saturating_duration_since(memo.at) < KEYS_TTL {
                return memo.uses.clone();
            }
        }
        let mut uses: Vec<KeyUse> = Vec::new();
        for (id, dir) in &projects {
            let Some(key) = crate::linear::read_linear_key(dir) else {
                continue;
            };
            match uses.iter_mut().find(|u| u.key == key) {
                Some(u) => u.projects.push(id.clone()),
                None => uses.push(KeyUse {
                    tag: crate::linear::key_tag(&key),
                    key,
                    projects: vec![id.clone()],
                }),
            }
        }
        *self.memo.borrow_mut() = Some(Memo {
            at: now,
            projects,
            uses: uses.clone(),
        });
        uses
    }

    /// Look at the env files again on the next ask: a key was just
    /// added, changed or tested (`linear::test_connection`).
    pub fn forget(&self) {
        self.memo.borrow_mut().take();
    }
}

/// The tag of the key `project` reads, if it has one.
pub fn tag_of(app: &App, project: &ProjectId) -> Option<String> {
    app.linear_keys
        .uses(app, Instant::now())
        .into_iter()
        .find(|u| u.projects.contains(project))
        .map(|u| u.tag)
}

/// The projects that read the key tagged `tag`.
pub fn projects_of(app: &App, tag: &str) -> Vec<ProjectId> {
    use_of(app, tag).map(|u| u.projects).unwrap_or_default()
}

fn use_of(app: &App, tag: &str) -> Option<KeyUse> {
    app.linear_keys
        .uses(app, Instant::now())
        .into_iter()
        .find(|u| u.tag == tag)
}

/// The Linear keys the screen shows: the selected project's lists (a
/// prefetch, so `⌘L` and `⌘K`'s issue rows have them), the open LINEAR
/// VIEW's lists and its key's teams, and — while the NEEDS YOU board is
/// open with its Linear section on — each key's notifications.
pub fn visible(app: &App) -> Vec<(QueryKey, Seen)> {
    let now = Instant::now();
    let uses = app.linear_keys.uses(app, now);
    let keyed = |project: &ProjectId| uses.iter().find(|u| u.projects.contains(project));
    let mut seen = Vec::new();
    if let Some(project) = super::scope::rested_project(app) {
        if keyed(&project.id).is_some() {
            seen.push((QueryKey::LinearView(project.id.clone()), Seen::Shown));
        }
    }
    if let Some(view) = crate::linear::linear_view(&app.overlay) {
        seen.push((QueryKey::LinearView(view.project.clone()), Seen::Focused));
        if let Some(u) = keyed(&view.project) {
            seen.push((QueryKey::LinearTeams(u.tag.clone()), Seen::Shown));
        }
    }
    let board = matches!(&app.overlay, Some(Overlay::Palette(_)));
    if board && app.needs_you.inbox.linear_mentions && crate::needs_you::asks() {
        for u in &uses {
            seen.push((QueryKey::LinearInbox(u.tag.clone()), Seen::Shown));
        }
    }
    seen
}

/// What the automations need of Linear: each key's intake, while the bug
/// fixer or the feature builder is on, once its [`POLL`](crate::automations::linear_builder::POLL)
/// since the last look (this run's or, kept on disk, the last run's) is
/// up.
pub fn needs(app: &App) -> Vec<(QueryKey, Duration)> {
    let Some(every) = crate::automations::linear_builder::interval() else {
        return Vec::new();
    };
    let now_ms = crate::app::now_ms();
    app.linear_keys
        .uses(app, Instant::now())
        .into_iter()
        .filter(|u| crate::automations::linear_builder::intake_due(app, &u.tag, now_ms))
        .map(|u| (QueryKey::LinearIntake(u.tag), every))
        .collect()
}

/// What a Linear fetch brought.
#[derive(Debug)]
pub enum Answer {
    View(Result<LinearList, FetchError>),
    Teams(Result<Teams, FetchError>),
    /// The key's NEEDS YOU notifications and live tickets, kept under
    /// `project` (the first that reads the key).
    Inbox {
        project: ProjectId,
        heard: Result<crate::needs_you::linear::Heard, FetchError>,
    },
    /// The intake's tickets for the projects that read the key, asked at
    /// `asked_ms` (epoch ms), the whole list or what changed.
    Intake {
        projects: Vec<ProjectId>,
        asked_ms: i64,
        full: bool,
        issues: Result<Vec<LinearIssue>, FetchError>,
    },
}

impl Answer {
    /// Whether the fetch answered, for the resolver.
    pub fn outcome(&self) -> Result<(), &FetchError> {
        match self {
            Answer::View(r) => r.as_ref().map(|_| ()),
            Answer::Teams(r) => r.as_ref().map(|_| ()),
            Answer::Inbox { heard, .. } => heard.as_ref().map(|_| ()),
            Answer::Intake { issues, .. } => issues.as_ref().map(|_| ()),
        }
    }
}

/// A Linear call's error as the resolver takes it: a rate limit pauses
/// Linear (`gate::linear_failure`).
fn failure(why: String) -> FetchError {
    gate::linear_failure(why, Instant::now())
}

/// Start the Linear fetch `ticket` asks for, off the loop, its answer sent
/// on `tx`. A key no project reads any more lands at once: as
/// [`FetchError::Missing`] where its answer has somewhere to go, else
/// unanswered.
pub fn spawn(app: &App, ticket: Ticket<QueryKey>, tx: tokio::sync::mpsc::UnboundedSender<Landed>) {
    let unanswered = tx.clone();
    let send = move |ticket, answer| {
        let _ = tx.send(Landed {
            ticket,
            answer: super::spawn::Answer::Linear(answer),
        });
    };
    let email = || {
        crate::config::Config::cached()
            .linear_assignee_email
            .trim()
            .to_string()
    };
    match ticket.key.clone() {
        QueryKey::LinearView(project) => {
            let Some(dir) = super::github::project_dir(app, &project) else {
                return send(ticket, Answer::View(Err(FetchError::Missing)));
            };
            let email = email();
            tokio::spawn(async move {
                let list = crate::linear::fetch_lists(&dir, &email)
                    .await
                    .map_err(failure);
                send(ticket, Answer::View(list));
            });
        }
        QueryKey::LinearTeams(tag) => {
            let Some(u) = use_of(app, &tag) else {
                return send(ticket, Answer::Teams(Err(FetchError::Missing)));
            };
            let email = email();
            tokio::spawn(async move {
                let teams = crate::linear::fetch_teams(&u.key, &email)
                    .await
                    .map_err(failure);
                send(ticket, Answer::Teams(teams));
            });
        }
        QueryKey::LinearInbox(tag) => {
            // A key no project reads any more has no project to keep
            // its answer under.
            let Some(u) = use_of(app, &tag) else {
                let answer = super::spawn::Answer::Unanswered;
                let _ = unanswered.send(Landed { ticket, answer });
                return;
            };
            let project = u.projects[0].clone();
            tokio::spawn(async move {
                let heard = crate::needs_you::linear::fetch(u.key)
                    .await
                    .map_err(failure);
                send(ticket, Answer::Inbox { project, heard });
            });
        }
        QueryKey::LinearIntake(tag) => {
            let asked_ms = crate::app::now_ms();
            let asked = use_of(app, &tag).zip(crate::automations::linear_builder::intake_query(
                app, &tag, asked_ms,
            ));
            let Some((u, (query, variables, full))) = asked else {
                let answer = Answer::Intake {
                    projects: Vec::new(),
                    asked_ms,
                    full: false,
                    issues: Err(FetchError::Missing),
                };
                return send(ticket, answer);
            };
            tokio::spawn(async move {
                let issues = crate::linear::graphql(&u.key, &query, variables)
                    .await
                    .and_then(|json| {
                        crate::linear::parse_my_issues(
                            &json,
                            &crate::automations::linear_builder::PICK_ORDER,
                        )
                    })
                    .map_err(failure);
                let answer = Answer::Intake {
                    projects: u.projects,
                    asked_ms,
                    full,
                    issues,
                };
                send(ticket, answer);
            });
        }
        other => {
            tracing::debug!(key = ?other, "no Linear fetch answers this key");
            let answer = super::spawn::Answer::Unanswered;
            let _ = unanswered.send(Landed { ticket, answer });
        }
    }
}

/// A Linear fetch landed, the resolver told already: what it brought goes
/// to the store that keeps it. `asked` is when it was asked.
pub fn land(app: &mut App, key: QueryKey, answer: Answer, asked: Instant) {
    match (key, answer) {
        (QueryKey::LinearView(project), Answer::View(list)) => {
            let list = list.map_err(|e| said(&e));
            crate::linear::land_list(app, project, list, asked);
        }
        (QueryKey::LinearTeams(tag), Answer::Teams(teams)) => match teams {
            Ok(teams) => crate::linear::land_teams(app, &tag, teams),
            Err(e) => tracing::warn!("linear: reading the teams: {}", said(&e)),
        },
        (QueryKey::LinearInbox(_), Answer::Inbox { project, heard }) => {
            let heard = heard.ok();
            crate::needs_you::land(app, crate::needs_you::Answer::Linear { project, heard });
        }
        (
            QueryKey::LinearIntake(tag),
            Answer::Intake {
                projects,
                asked_ms,
                full,
                issues,
            },
        ) => {
            let issues = issues.map_err(|e| said(&e));
            crate::automations::linear_builder::intake_landed(
                app, &tag, projects, asked_ms, full, issues,
            );
        }
        _ => {}
    }
}

/// What a failed Linear fetch says on screen.
fn said(e: &FetchError) -> String {
    match e {
        FetchError::Failed(why) => why.clone(),
        FetchError::RateLimited { .. } => "Linear's rate limit: waiting for it to reset".into(),
        FetchError::Missing => crate::linear::NO_KEY.into(),
    }
}
