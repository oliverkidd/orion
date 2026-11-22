//! GITHUB THROUGH THE RESOLVER: what each GitHub [`QueryKey`] asks, and
//! where its answer goes — and the one GitHub read that runs with nobody
//! looking, `MyPrs`.
//!
//! | Key | Asks | Lands in |
//! |---|---|---|
//! | `OpenPrs(project)` | the light list (`pull_request::list`, ~2 points) | `event_loop::note_open_prs_answer_at` |
//! | `PrExtras(project)` | labels, review requests, tallies (~4) | the rows' meta ([`land_extras`]) |
//! | `PrTails(project)` | merged 7 d + closed 30 d (~2) | `OpenPrs::merged` (`event_loop::land_pr_tails`) |
//! | `PrDetail(url)` | one page, one query (~1–2) | `event_loop::land_pr_detail` |
//! | `BranchPr(checkout)` | `gh pr view` on its branch (1) | `event_loop::land_branch_pr` |
//! | `MyPrs` | your open pull requests everywhere + recent merges (~2) | every project's rows of yours ([`land_my_prs`]) |
//! | `ReviewRequests` | `gh search prs --review-requested=@me` (1) | NEEDS YOU's CODE REVIEWS |
//!
//! `MyPrs` is the automations' clock. AUTOFIX and the factory's pull
//! request bodies act on your pull requests while you are elsewhere; they
//! used to keep timers of their own and read each page on them. Now one
//! cross-repo search every few minutes ([`needs`]) says which of your pull
//! requests moved — a verdict, a push, a comment, a review thread — and
//! only those pages are read again (`PrDetail`, invalidated by the
//! automation that cares). Its rows go into each project's open list, so
//! every surface and automation that reads the lists sees them, and its
//! merges into the same merge handling a list's are: the purple sweep, the
//! knowledge harvest, and the lists, tails and pages that merge makes
//! stale.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use orion_core::{ClientRequest, ProjectId};
use tokio::sync::mpsc::UnboundedSender;

use super::spawn::{Answer as Landing, Landed};
use super::{FetchError, QueryKey};
use crate::app::App;
use crate::automations::Automation;
use crate::fetch::Ticket;
use crate::pull_request::{Checks, Extras, MyPrsAnswer, OpenPr, PrDetail};

/// What a GitHub fetch beyond the open list and the branch lookup brought.
#[derive(Debug)]
pub enum Answer {
    Extras(Result<Extras, FetchError>),
    Tails(Result<Vec<OpenPr>, FetchError>),
    /// Boxed: a page is far bigger than every other answer.
    Detail(Result<Box<PrDetail>, FetchError>),
    MyPrs(Result<MyPrsAnswer, FetchError>),
    Reviews(Result<Vec<crate::needs_you::reviews::Found>, FetchError>),
}

impl Answer {
    /// Whether it answered.
    pub fn outcome(&self) -> Result<(), &FetchError> {
        match self {
            Answer::Extras(a) => a.as_ref().map(|_| ()),
            Answer::Tails(a) => a.as_ref().map(|_| ()),
            Answer::Detail(a) => a.as_ref().map(|_| ()),
            Answer::MyPrs(a) => a.as_ref().map(|_| ()),
            Answer::Reviews(a) => a.as_ref().map(|_| ()),
        }
    }
}

// ---- asking ----

/// Start the fetch a GitHub `ticket` asks for, off the loop, its answer
/// sent on `tx`. A key whose repo or checkout is gone lands at once as
/// [`FetchError::Missing`].
pub fn spawn(app: &App, ticket: Ticket<QueryKey>, tx: UnboundedSender<Landed>) {
    let send = move |ticket, answer| {
        let _ = tx.send(Landed { ticket, answer });
    };
    let github = |answer| Landing::GitHub(answer);
    match ticket.key.clone() {
        QueryKey::OpenPrs(project) => {
            let Some(dir) = project_dir(app, &project) else {
                return send(ticket, Landing::Unanswered);
            };
            if !dir.is_dir() {
                let answer = Err(FetchError::Missing);
                return send(ticket, Landing::OpenPrs { answer });
            }
            tokio::spawn(async move {
                let answer = crate::pull_request::list(&dir).await;
                send(ticket, Landing::OpenPrs { answer });
            });
        }
        QueryKey::PrExtras(project) => match project_dir(app, &project) {
            Some(dir) if dir.is_dir() => {
                tokio::spawn(async move {
                    let answer = crate::pull_request::extras(&dir).await;
                    send(ticket, github(Answer::Extras(answer)));
                });
            }
            _ => send(ticket, github(Answer::Extras(Err(FetchError::Missing)))),
        },
        QueryKey::PrTails(project) => match project_dir(app, &project) {
            Some(dir) if dir.is_dir() => {
                tokio::spawn(async move {
                    let answer = crate::pull_request::tails(&dir).await;
                    send(ticket, github(Answer::Tails(answer)));
                });
            }
            _ => send(ticket, github(Answer::Tails(Err(FetchError::Missing)))),
        },
        QueryKey::PrDetail(url) => {
            tokio::spawn(async move {
                let answer = crate::pull_request::detail(&url).await.map(Box::new);
                send(ticket, github(Answer::Detail(answer)));
            });
        }
        QueryKey::BranchPr(worktree) => {
            let Some((path, branch)) = app
                .tree
                .worktrees
                .iter()
                .find(|w| w.id == worktree)
                .map(|w| (w.path.clone(), w.branch.clone()))
            else {
                return send(ticket, Landing::Unanswered);
            };
            if !path.is_dir() {
                let answer = Err(FetchError::Missing);
                return send(ticket, Landing::BranchPr { branch, answer });
            }
            tokio::spawn(async move {
                let answer = crate::pull_request::lookup_checked(&path).await;
                send(ticket, Landing::BranchPr { branch, answer });
            });
        }
        QueryKey::MyPrs => {
            let since = merged_since(chrono::Local::now().date_naive());
            tokio::spawn(async move {
                let answer = crate::pull_request::my_prs(&since).await;
                send(ticket, github(Answer::MyPrs(answer)));
            });
        }
        QueryKey::ReviewRequests => {
            tokio::spawn(async move {
                let answer = crate::needs_you::reviews::search().await;
                send(ticket, github(Answer::Reviews(answer)));
            });
        }
        other => {
            tracing::debug!(key = ?other, "not a GitHub key");
            send(ticket, Landing::Unanswered);
        }
    }
}

/// The day `MyPrs` asks for merges from: a week before `today`, the same
/// window the PULL REQUESTS MODAL's `Merged` section reaches — so a merge
/// made while orion was closed for a weekend is still seen.
fn merged_since(today: chrono::NaiveDate) -> String {
    let days = chrono::Days::new(crate::pull_request::MERGED_DAYS as u64);
    today
        .checked_sub_days(days)
        .unwrap_or(today)
        .format("%Y-%m-%d")
        .to_string()
}

/// `project`'s repo, where `gh` asks from (and where its `.env` names
/// its Linear key).
pub(super) fn project_dir(app: &App, project: &ProjectId) -> Option<PathBuf> {
    app.tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map(|p| p.repo_path.clone())
}

// ---- which project a pull request is in ----

/// Each repo's web page (`remote::repo_url`), asked of git once a process:
/// a remote does not move under a running orion.
static REPO_PAGES: Mutex<Vec<(PathBuf, Option<String>)>> = Mutex::new(Vec::new());

/// The web page of the repo at `root`, from its remote — memoised.
fn repo_page(root: &std::path::Path) -> Option<String> {
    let mut pages = REPO_PAGES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, page)) = pages.iter().find(|(r, _)| r == root) {
        return page.clone();
    }
    let page = crate::remote::repo_url(root).ok();
    pages.push((root.to_path_buf(), page.clone()));
    page
}

/// The project pull request `url` is in: the one whose open list or tail
/// has it, else the one whose repo's web page it sits under
/// (`https://github.com/o/r` for `…/o/r/pull/42`). None for a repo no
/// project has.
pub fn project_of(app: &App, url: &str) -> Option<ProjectId> {
    let listed = app
        .open_prs
        .iter()
        .find(|(_, open)| open.list.iter().chain(&open.merged).any(|pr| pr.url == url));
    if let Some((project, _)) = listed {
        return Some(project.clone());
    }
    app.tree
        .projects
        .iter()
        .find(|p| under_page(url, repo_page(&p.repo_path).as_deref()))
        .map(|p| p.id.clone())
}

/// Whether `url` is a pull request of the repo whose web page is `page`.
fn under_page(url: &str, page: Option<&str>) -> bool {
    page.is_some_and(|page| {
        url.strip_prefix(page.trim_end_matches('/'))
            .is_some_and(|rest| rest.starts_with("/pull/"))
    })
}

/// A local checkout of the repo pull request `url` is in, for what reads
/// it through git (`conflicts::want`): its project's root.
pub fn repo_dir_of(app: &App, url: &str) -> Option<PathBuf> {
    project_of(app, url).and_then(|p| project_dir(app, &p))
}

// ---- background demand ----

/// How often `MyPrs` is asked while something of yours is moving: a pull
/// request's checks running, or AUTOFIX in the middle of one.
pub const MY_PRS_BUSY: Duration = Duration::from_secs(3 * 60);

/// How often `MyPrs` is asked once everything of yours has settled.
pub const MY_PRS_SETTLED: Duration = Duration::from_secs(15 * 60);

/// A need that never beats: a page an automation wants read once — when
/// it is invalidated, and again on its backoff while that fails — and not
/// on a clock. Long enough never to come round; short enough that
/// `asked + ON_CHANGE` never overflows an `Instant`.
pub const ON_CHANGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether an automation that acts on your pull requests is on: AUTOFIX,
/// or one of the factory's builders, whose pull requests get their bodies
/// written (`pr_body`).
pub fn automations_on(app: &App) -> bool {
    [
        Automation::PrFixer,
        Automation::BugFixer,
        Automation::FeatureBuilder,
    ]
    .into_iter()
    .any(|a| app.automations.is_on(a))
}

/// What GitHub the automations need while nobody looks: `MyPrs` on its
/// beat ([`my_prs_every`]), and each page AUTOFIX and the factory have
/// asked to have read (`pr_fixer::detail_needs`, `pr_body::detail_needs`).
/// Nothing while every such automation is off.
pub fn needs(app: &App) -> Vec<(QueryKey, Duration)> {
    if !automations_on(app) {
        return Vec::new();
    }
    let mut needs = vec![(QueryKey::MyPrs, my_prs_every(app))];
    let pages = crate::automations::pr_fixer::detail_needs(app)
        .into_iter()
        .chain(crate::pr_body::detail_needs(app));
    needs.extend(pages.map(|(url, every)| (QueryKey::PrDetail(url), every)));
    needs
}

/// `MyPrs`' beat: [`MY_PRS_BUSY`] while any of your pull requests has
/// checks running or AUTOFIX is mid-cycle on one, else
/// [`MY_PRS_SETTLED`].
pub fn my_prs_every(app: &App) -> Duration {
    let running = app.my_prs.open.iter().any(|pr| {
        let status = app.prs.status_or_open(&pr.url);
        status.is_open() && status.health.checks == Checks::Pending
    });
    if running || crate::automations::pr_fixer::mid_cycle(app) {
        MY_PRS_BUSY
    } else {
        MY_PRS_SETTLED
    }
}

/// What the last `MyPrs` answer said (`App::my_prs`): your open pull
/// requests. When it landed is THE RESOLVER's, kept across launches as a
/// stamp (`pr_cache`), so a relaunch waits out the beat rather than
/// asking at once.
#[derive(Debug, Clone, Default)]
pub struct Mine {
    pub open: Vec<OpenPr>,
}

// ---- landing ----

/// A GitHub answer beyond the list and the branch lookup landed, its
/// flight already settled with the resolver: `owed` when a fresh one was
/// asked for while it ran.
pub fn land(
    app: &mut App,
    key: QueryKey,
    answer: Answer,
    asked: crate::fetch::Asked,
    owed: bool,
    out: &mut Vec<ClientRequest>,
) {
    match (key, answer) {
        (QueryKey::PrExtras(project), Answer::Extras(Ok(extras))) => {
            land_extras(app, &project, &extras);
        }
        (QueryKey::PrTails(project), Answer::Tails(Ok(rows))) => {
            crate::event_loop::land_pr_tails(app, project, rows, asked, out);
        }
        (QueryKey::PrDetail(url), Answer::Detail(answer)) => {
            let detail = answer.ok().map(|d| *d);
            crate::event_loop::land_pr_detail(app, url.clone(), detail, asked, owed, out);
            if let Some(dir) = repo_dir_of(app, &url) {
                crate::conflicts::want(app, &url, &dir);
            }
        }
        (QueryKey::MyPrs, Answer::MyPrs(Ok(found))) => land_my_prs(app, found, asked, out),
        (QueryKey::ReviewRequests, Answer::Reviews(Ok(found))) => {
            crate::needs_you::land(app, crate::needs_you::Answer::Reviews(Some(found)));
        }
        _ => {}
    }
}

/// A project's extras landed: on its open rows, kept with them (and in
/// the cache) across the light lists that follow.
fn land_extras(app: &mut App, project: &ProjectId, extras: &Extras) {
    let Some(open) = app.open_prs.get_mut(project) else {
        return;
    };
    let before = open.list.clone();
    crate::pull_request::apply_extras(&mut open.list, extras);
    if open.list != before {
        app.pr_cache_dirty = true;
        app.dirty = true;
        crate::pr_modal::list_changed(app);
    }
}

/// `MyPrs` landed, asked at `asked`. Each row is observed into `prs` — a
/// verdict GitHub's rollup calls failing only where nothing better is
/// known, as a list's unchecked row is (`PrStore::observe_list_row`) — and
/// each merge too. Then every project with a pull request of yours has its
/// rows of yours replaced by these (`event_loop::adopt_my_rows`): a new
/// one added, one that left dropped, the rest of the list as it was —
/// which runs AUTOFIX and the factory over them, as a list landing does.
/// A merge seen here makes that project's list, tail and the pull
/// request's page stale. An answer that says what the last one did
/// writes and paints nothing of its own.
pub fn land_my_prs(
    app: &mut App,
    found: MyPrsAnswer,
    asked: crate::fetch::Asked,
    out: &mut Vec<ClientRequest>,
) {
    let merged_now: Vec<(String, Option<ProjectId>)> = found
        .merged
        .iter()
        .filter(|pr| {
            app.prs
                .status(&pr.url)
                .is_none_or(|s| s.standing != crate::pull_request::Standing::Merged)
        })
        .map(|pr| (pr.url.clone(), project_of(app, &pr.url)))
        .collect();
    let changed = !merged_now.is_empty() || app.my_prs.open != found.open;
    let before: Vec<(orion_core::WorktreeId, bool)> = app
        .tree
        .projects
        .iter()
        .flat_map(|p| crate::event_loop::checkout_merges(app, &p.id))
        .collect();
    for row in &found.open {
        app.prs.observe_list_row(row, row.says_failing(), asked);
    }
    for row in &found.merged {
        let seen = crate::pr_store::PrObservation::of_tail_row(row);
        app.prs.observe(&row.url, seen, asked);
    }
    let complete = found.open.len() < crate::pull_request::LIST_LIMIT;
    let mut by_project: HashMap<ProjectId, Vec<OpenPr>> = HashMap::new();
    for row in &found.open {
        if let Some(project) = project_of(app, &row.url) {
            by_project.entry(project).or_default().push(row.clone());
        }
    }
    // A project whose list has rows of yours MyPrs no longer has: they
    // left — merged or closed — and go too.
    let projects: HashSet<ProjectId> = app
        .open_prs
        .iter()
        .filter(|(_, open)| open.list.iter().any(|pr| pr.mine))
        .map(|(p, _)| p.clone())
        .chain(by_project.keys().cloned())
        .collect();
    for project in projects {
        let mine = by_project.remove(&project).unwrap_or_default();
        crate::event_loop::adopt_my_rows(app, project, mine, complete, out);
    }
    for (url, project) in merged_now {
        app.resolver.invalidate(&QueryKey::PrDetail(url));
        if let Some(project) = project {
            app.resolver.invalidate(&QueryKey::OpenPrs(project.clone()));
            app.resolver.invalidate(&QueryKey::PrTails(project));
        }
    }
    crate::event_loop::note_merges_seen(app, before);
    app.my_prs = Mine { open: found.open };
    app.pr_cache_dirty |= changed;
    app.dirty |= changed;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_are_asked_for_a_week_back() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        assert_eq!(merged_since(today), "2026-11-15");
    }

    #[test]
    fn a_pull_request_is_in_the_repo_whose_page_it_sits_under() {
        let page = Some("https://github.com/o/app");
        assert!(under_page("https://github.com/o/app/pull/4", page));
        assert!(!under_page("https://github.com/o/app-two/pull/4", page));
        assert!(!under_page("https://github.com/o/app/issues/4", page));
        assert!(!under_page("https://github.com/o/app/pull/4", None));
    }

    fn row(n: u64, mine: bool, sha: &str) -> OpenPr {
        OpenPr {
            number: n,
            title: format!("PR {n}"),
            url: format!("https://github.com/o/r/pull/{n}"),
            answered_draft: false,
            answered: Default::default(),
            head: format!("b{n}"),
            mine,
            head_sha: sha.into(),
            meta: Default::default(),
        }
    }

    /// `MyPrs` keeps a project's rows of yours current without asking the
    /// project's list: a row of yours moved on, one that left dropped,
    /// other people's rows left as they were — and a merge it saw makes
    /// the project's tail stale.
    #[test]
    fn my_prs_replace_my_rows_and_see_merges() {
        let mut app = App::new();
        let p = ProjectId("p".into());
        let now = std::time::Instant::now();
        app.open_prs.insert(
            p.clone(),
            crate::app::OpenPrs {
                list: vec![row(1, true, "a"), row(2, true, "b"), row(3, false, "c")],
                merged: Vec::new(),
                at: now,
            },
        );
        let tails = QueryKey::PrTails(p.clone());
        app.resolver.seed(&tails, now);
        let mut merged = row(2, true, "b");
        merged.meta.merged_at = "2026-11-21T10:00:00Z".into();
        let found = MyPrsAnswer {
            open: vec![row(1, true, "a2")],
            merged: vec![merged],
        };
        let mut out = Vec::new();
        land_my_prs(
            &mut app,
            found,
            crate::fetch::Asked::At(crate::fetch::now()),
            &mut out,
        );
        let list = &app.open_prs[&p].list;
        let numbers: Vec<u64> = list.iter().map(|pr| pr.number).collect();
        assert_eq!(numbers, vec![1, 3], "#2 merged and left; theirs stays");
        assert_eq!(list[0].head_sha, "a2", "yours moved on");
        assert_eq!(
            app.prs.status_or_open(&row(2, true, "").url).standing,
            crate::pull_request::Standing::Merged
        );
        assert_eq!(app.my_prs.open.len(), 1);
        // The tail read a moment ago is behind now: on the modal's next
        // look it is read again.
        app.resolver
            .visit(vec![(tails.clone(), super::super::Seen::Shown)], now);
        assert!(app.resolver.due(now).iter().any(|t| t.key == tails));
    }

    /// The automations' read runs only while one that acts on your pull
    /// requests is on, at three minutes while something moves and a
    /// quarter of an hour once it has settled.
    #[test]
    fn my_prs_runs_only_for_the_automations_and_slows_once_settled() {
        let mut app = App::new();
        for a in Automation::ALL {
            app.automations.settings.of_mut(a).on = Some(false);
        }
        assert!(needs(&app).is_empty(), "every automation off");
        app.automations.settings.of_mut(Automation::PrFixer).on = Some(true);
        assert_eq!(needs(&app), vec![(QueryKey::MyPrs, MY_PRS_SETTLED)]);
        let pr = OpenPr {
            number: 1,
            title: "t".into(),
            url: "https://github.com/o/r/pull/1".into(),
            answered_draft: false,
            answered: crate::pull_request::Answered {
                conflicts: Some(false),
                checks: Some(Checks::Pending),
            },
            head: "b".into(),
            mine: true,
            head_sha: "s".into(),
            meta: Default::default(),
        };
        app.prs.observe(
            &pr.url,
            crate::pr_store::PrObservation::of_list_row(&pr),
            crate::fetch::Asked::Cached,
        );
        app.my_prs.open = vec![pr];
        assert_eq!(my_prs_every(&app), MY_PRS_BUSY, "checks running");
    }
}
