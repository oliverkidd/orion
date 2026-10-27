//! CHECK OUT A PULL REQUEST: the project's checkout of a pull request's
//! branch, found where it is or cut by the DAEMON
//! (`ClientRequest::CreatePrWorktree`) — the very checkout a PR SESSION
//! runs in, a fork's under its owner's name (`OpenPr::head`). Every
//! surface that needs one asks here: **Check out** in a pull request row's
//! menu, `⌘B` in the PULL REQUESTS MODAL, and the CONFLICT RESOLVER on a
//! pull request nobody has checked out yet, which carries on into its
//! merge once the checkout lands ([`Then`]).

use orion_core::{ClientRequest, ProjectId, Worktree, WorktreeId};

use crate::app::{App, Overlay, PendingIntent};
use crate::flash::Flash;
use crate::pull_request::PrLaunch;

/// What a checkout is for, once it is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// Say where it is — and, off any modal, put the cursor on it.
    Show,
    /// Open the CONFLICT RESOLVER on the pull request at `url`, at `path`
    /// (`conflicts::open`).
    Resolve {
        url: String,
        label: String,
        path: Option<String>,
    },
}

/// The pull request at `url` as a launch knows it — its project and the
/// branch its checkout is on — from the open and merged lists.
pub(crate) fn of_url(app: &App, url: &str) -> Option<(ProjectId, PrLaunch)> {
    app.open_prs.iter().find_map(|(project, open)| {
        open.list
            .iter()
            .chain(&open.merged)
            .find(|pr| pr.url == url)
            .map(|pr| (project.clone(), PrLaunch::of(pr)))
    })
}

/// `project`'s checkout of `head`, when there is one on disk.
pub(crate) fn worktree_of<'a>(
    app: &'a App,
    project: &ProjectId,
    head: &str,
) -> Option<&'a Worktree> {
    app.tree.worktrees.iter().find(|w| {
        &w.project_id == project && w.branch == head && !app.is_placeholder_worktree(&w.id)
    })
}

/// Check `pr` out under `project`, then do `then`: at once when the
/// checkout is already there, else once the DAEMON has cut it.
pub(crate) fn check_out(
    app: &mut App,
    project: ProjectId,
    pr: PrLaunch,
    then: Then,
    out: &mut Vec<ClientRequest>,
) {
    if let Some(id) = worktree_of(app, &project, &pr.head).map(|w| w.id.clone()) {
        let already = then == Then::Show;
        finish(app, &id, then, already, out);
        return;
    }
    let on_its_way = app
        .pending
        .values()
        .any(|intent| matches!(intent, PendingIntent::CheckOutPr { .. }));
    if on_its_way || app.pr_checkout_waiting.is_some() {
        app.flash = Some(Flash::note("a checkout is already on its way"));
        return;
    }
    app.flash = Some(Flash::working(format!("checking out ⎇ {}…", pr.head)));
    let req_id = app.alloc_req_id(PendingIntent::CheckOutPr { then });
    out.push(ClientRequest::CreatePrWorktree {
        req_id,
        project,
        pr_url: pr.url,
        head: pr.head,
    });
}

/// The DAEMON's Ack for a checkout: on with `then` once its row is in the
/// tree — now, or on the upsert that brings it ([`seen`]).
pub(crate) fn landed(app: &mut App, id: WorktreeId, then: Then, out: &mut Vec<ClientRequest>) {
    if app.tree.worktrees.iter().any(|w| w.id == id) {
        finish(app, &id, then, false, out);
    } else {
        app.pr_checkout_waiting = Some((id, then));
    }
}

/// A row landed: the checkout an Ack beat here, if this is it.
pub(crate) fn seen(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some((id, _)) = &app.pr_checkout_waiting else {
        return;
    };
    if !app.tree.worktrees.iter().any(|w| &w.id == id) {
        return;
    }
    if let Some((id, then)) = app.pr_checkout_waiting.take() {
        finish(app, &id, then, false, out);
    }
}

/// The checkout is there: do what it was for.
fn finish(app: &mut App, id: &WorktreeId, then: Then, already: bool, out: &mut Vec<ClientRequest>) {
    match then {
        Then::Resolve { url, label, path } => {
            crate::conflicts::open(app, &url, &label, path.as_deref(), out)
        }
        Then::Show => {
            let Some(w) = app.tree.worktrees.iter().find(|w| &w.id == id) else {
                return;
            };
            let (branch, path) = (w.branch.clone(), crate::claude_accounts::tilde(&w.path));
            if app.overlay.is_none() {
                crate::event_loop::select_worktree_by_id(app, id, out);
            }
            let said = if already {
                format!("⎇ {branch} is already checked out at {path}")
            } else {
                format!("checked out ⎇ {branch} at {path}")
            };
            app.flash = Some(Flash::done(said));
        }
    }
    app.dirty = true;
}

/// `⌘B` in the PULL REQUESTS MODAL, on the pull request under the cursor.
pub(crate) fn from_modal(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::PullRequests(view)) = &app.overlay else {
        return;
    };
    let project = view.project.clone();
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
        return;
    };
    check_out(app, project, PrLaunch::of(&pr), Then::Show, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{Entity, EntityId, ServerEvent};

    fn app() -> (App, ProjectId, PrLaunch) {
        let mut app = App::new();
        let project = ProjectId("p".into());
        app.tree.projects.push(orion_core::Project {
            id: project.clone(),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        });
        let pr = PrLaunch {
            url: "https://github.com/o/r/pull/7".into(),
            head: "someone/feat".into(),
            number: 7,
        };
        (app, project, pr)
    }

    fn worktree(project: &ProjectId) -> Worktree {
        Worktree {
            id: WorktreeId("w7".into()),
            project_id: project.clone(),
            path: "/tmp/demo-worktrees/feat".into(),
            branch: "someone/feat".into(),
            is_main: false,
            sort_order: 0,
        }
    }

    /// No checkout: the DAEMON is asked for the PR SESSION's, under the
    /// fork-aware branch name, and the Ack says where it landed.
    #[test]
    fn a_missing_checkout_is_cut_and_its_ack_says_where() {
        let (mut app, project, pr) = app();
        let mut out = Vec::new();
        check_out(&mut app, project.clone(), pr.clone(), Then::Show, &mut out);
        let [ClientRequest::CreatePrWorktree {
            req_id,
            project: asked,
            pr_url,
            head,
        }] = out.as_slice()
        else {
            panic!("{out:?}");
        };
        assert_eq!((asked, pr_url, head), (&project, &pr.url, &pr.head));
        let req_id = *req_id;
        check_out(&mut app, project.clone(), pr, Then::Show, &mut out);
        assert_eq!(out.len(), 1, "one at a time");

        // The Ack beats the row here: the flash waits for it.
        crate::event_loop::handle_server_event(
            &mut app,
            ServerEvent::Ack {
                req_id,
                created: Some(EntityId::Worktree(WorktreeId("w7".into()))),
            },
            &mut out,
        );
        assert!(app.pr_checkout_waiting.is_some());
        crate::event_loop::handle_server_event(
            &mut app,
            ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree(&project)),
            },
            &mut out,
        );
        assert!(app.pr_checkout_waiting.is_none());
        let said = app.flash.as_deref().unwrap_or_default();
        assert!(said.starts_with("checked out ⎇ someone/feat at"), "{said}");
    }

    /// A checkout already there is the answer, with nothing asked.
    #[test]
    fn an_existing_checkout_is_named_with_nothing_asked() {
        let (mut app, project, pr) = app();
        app.tree.worktrees.push(worktree(&project));
        let mut out = Vec::new();
        check_out(&mut app, project, pr, Then::Show, &mut out);
        assert!(out.is_empty(), "{out:?}");
        let said = app.flash.as_deref().unwrap_or_default();
        assert!(said.contains("already checked out"), "{said}");
    }
}
