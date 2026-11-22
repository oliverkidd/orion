//! WHAT THE SCREEN SHOWS, in [`QueryKey`]s: the set the RESOLVER is
//! handed after every turn of the event loop. Coarse on purpose — per
//! project and section, never per row — so a visit costs one fetch however
//! many rows it paints.
//!
//! | On screen | Key | How |
//! |---|---|---|
//! | the selected project (the PROJECT OPEN PRS GROUP, its checkouts' badges) | `OpenPrs(project)` | shown |
//! | the PULL REQUESTS MODAL | `OpenPrs`, `PrExtras` (its project) | focused |
//! | the PULL REQUESTS MODAL's `Merged` / `Closed` sections | `PrTails(its project)` | shown |
//! | the page a cursor rests on (modal or pane) | `PrDetail(url)` | focused while its checks run, else shown |
//! | the selected checkout, on a branch no open list has | `BranchPr(checkout)` | shown |
//! | the NEEDS YOU board's CODE REVIEWS | `ReviewRequests` | shown |
//! | the selected checkout's badge; every other band's rule (`sync::local`) | `GitStatus(checkout)` | focused; shown |
//! | the selected project (`sync::local`) | `BaseFetch(project)` | shown |
//!
//! What is asked because of the selection — the selected project's and
//! checkout's keys, here and in `sync::linear` and `sync::local` — waits
//! for the selection's REST (`git_watch::Rest`): walking the list asks
//! nothing for the rows passed.
//!
//! A checkout on its project's base or default branch — the ROOT WORKTREE
//! above all — never has a pull request of its own, and is never asked
//! about. One whose branch an open list has takes its pull request from
//! that list (`event_loop::derive_checkout_prs`).

use orion_core::Worktree;

use super::{QueryKey, Seen};
use crate::app::{App, Overlay};

/// Every key the screen shows right now, and how.
pub fn visible(app: &App) -> Vec<(QueryKey, Seen)> {
    let mut seen = Vec::new();
    if let Some(project) = rested_project(app) {
        seen.push((QueryKey::OpenPrs(project.id.clone()), Seen::Shown));
    }
    if let Some(Overlay::PullRequests(view)) = &app.overlay {
        seen.push((QueryKey::OpenPrs(view.project.clone()), Seen::Focused));
        seen.push((QueryKey::PrExtras(view.project.clone()), Seen::Focused));
        seen.push((QueryKey::PrTails(view.project.clone()), Seen::Shown));
    }
    if let Some(url) = &app.pr_detail_rest {
        seen.push((QueryKey::PrDetail(url.clone()), page_seen(app, url)));
    }
    if let Some(w) = rested_worktree(app) {
        if wants_branch_lookup(app, w) {
            seen.push((QueryKey::BranchPr(w.id.clone()), Seen::Shown));
        }
    }
    if matches!(app.overlay, Some(Overlay::Palette(_)))
        && app.needs_you.inbox.code_reviews
        && crate::needs_you::asks()
    {
        seen.push((QueryKey::ReviewRequests, Seen::Shown));
    }
    seen.extend(super::linear::visible(app));
    seen.extend(super::services::visible(app));
    seen.extend(super::local::visible(app));
    seen
}

/// The selected project, once the selection has rested on it.
pub(super) fn rested_project(app: &App) -> Option<&orion_core::Project> {
    app.selected_project()
        .filter(|_| app.git_watch.rest.rested())
}

/// The selected checkout, once the selection has rested on it.
pub(super) fn rested_worktree(app: &App) -> Option<&Worktree> {
    app.selected_worktree()
        .filter(|_| app.git_watch.rest.rested())
}

/// How the page a cursor rests on is seen: the thing the user is in —
/// read again on the live beat while the window is focused — only while
/// its checks are running, the one part of a page that moves on its own.
/// A settled page is read on arrival and then left.
fn page_seen(app: &App, url: &str) -> Seen {
    let status = app.prs.status_or_open(url);
    match status.is_open() && status.health.checks == crate::pull_request::Checks::Pending {
        true => Seen::Focused,
        false => Seen::Shown,
    }
}

/// The keys a visit to the Worktrees panel looks at again: what the panel
/// draws of GitHub.
pub fn worktrees_panel(app: &App) -> Vec<QueryKey> {
    visible(app)
        .into_iter()
        .map(|(key, _)| key)
        .filter(|key| matches!(key, QueryKey::OpenPrs(_) | QueryKey::BranchPr(_)))
        .collect()
}

/// Whether checkout `w`'s own pull request is asked about by branch: not
/// on the ROOT WORKTREE, a checkout still being cut, or a base branch,
/// and not when the project's open list already has its branch.
pub fn wants_branch_lookup(app: &App, w: &Worktree) -> bool {
    if w.is_main || w.branch.is_empty() || app.is_placeholder_worktree(&w.id) {
        return false;
    }
    let listed = app
        .open_prs
        .get(&w.project_id)
        .is_some_and(|open| open.list.iter().any(|pr| pr.head == w.branch));
    !listed && !is_base_branch(app, w)
}

/// Whether `w` is on its project's base or default branch: the branch its
/// ROOT WORKTREE is on, the **Base branch** Settings names, or `main` /
/// `master`.
fn is_base_branch(app: &App, w: &Worktree) -> bool {
    if matches!(w.branch.as_str(), "main" | "master") {
        return true;
    }
    let root = app
        .tree
        .worktrees
        .iter()
        .find(|r| r.project_id == w.project_id && r.is_main);
    if root.is_some_and(|r| r.branch == w.branch) {
        return true;
    }
    let Some(repo) = app
        .tree
        .projects
        .iter()
        .find(|p| p.id == w.project_id)
        .map(|p| p.repo_path.as_path())
    else {
        return false;
    };
    crate::config::base_branch_of(Some(repo)) == w.branch
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page a cursor rests on is the thing you are in — a live beat —
    /// only while its checks run; settled, it is read on arrival alone.
    #[test]
    fn a_rested_page_is_live_only_while_its_checks_run() {
        let mut app = App::new();
        let url = "https://github.com/o/r/pull/1".to_string();
        app.pr_detail_rest = Some(url.clone());
        let seen = |app: &App| {
            visible(app)
                .into_iter()
                .find(|(k, _)| k == &QueryKey::PrDetail(url.clone()))
                .map(|(_, how)| how)
        };
        assert_eq!(seen(&app), Some(Seen::Shown), "nothing known");
        let checks = |c| crate::pr_store::PrObservation {
            checks: Some(c),
            ..Default::default()
        };
        app.prs.observe(
            &url,
            checks(crate::pull_request::Checks::Pending),
            crate::fetch::Asked::At(crate::fetch::now()),
        );
        assert_eq!(seen(&app), Some(Seen::Focused), "checks running");
        app.prs.observe(
            &url,
            checks(crate::pull_request::Checks::Passing),
            crate::fetch::Asked::At(crate::fetch::now()),
        );
        assert_eq!(seen(&app), Some(Seen::Shown), "settled");
        app.pr_detail_rest = None;
        assert_eq!(seen(&app), None, "no rest, no page");
    }
}
