//! STACK STATUS in the TUI: each checkout's stack — up, starting,
//! stopping or down — as the DAEMON works it out
//! (`ServerEvent::StackStatuses`, `orion_core::stack`). The band's `⬡` is
//! green while it is up, amber while it starts or stops, and gone while
//! it is down; the run line is drawn only while it is not down, with the
//! ports it serves. Starting and stopping it is `⌘⇧S`'s
//! (`event_loop::toggle_stack_in`): an app found in the checkout
//! (`crate::run_targets`), in the background, on a run line in its band.
//! A click on a port, or ⌘O's **Open app**, opens the app.

use crate::app::{App, PendingIntent};
use orion_core::protocol::ClientRequest;
use orion_core::stack::StackStatus;
use orion_core::WorktreeId;

/// The mark a stack wears on its band.
pub const MARK: &str = "⬡";

/// `StackStatuses`: every checkout's stack that is not down.
pub(crate) fn listed(app: &mut App, statuses: Vec<(WorktreeId, StackStatus)>) {
    app.stack_status = statuses.into_iter().filter(|(_, s)| !s.is_down()).collect();
    app.dirty = true;
}

/// Whether `worktree`'s stack is up or coming up: what a stop stops.
pub(crate) fn live(app: &App, worktree: &WorktreeId) -> bool {
    app.stack_status(worktree).is_live()
}

/// A checkout's menu item for ⌘O's **Open app**: the first localhost
/// address its run printed, in the browser ([`open_app`]).
pub(crate) fn open_app_item() -> crate::app::MenuItem {
    crate::app::MenuItem::new(
        "Open app",
        crate::app::MenuAction::RunAction(crate::keymap::Action::OpenApp),
    )
}

/// A checkout's menu item for `⌘⇧S`: **Cancel start** while a start waits
/// on its setup, **Stop stack** while its stack is not down, else **Start
/// stack**.
pub(crate) fn menu_label(app: &App, worktree: &WorktreeId) -> &'static str {
    if run_queued(app, worktree) {
        "Cancel start"
    } else if !app.stack_status(worktree).is_down() {
        "Stop stack"
    } else {
        "Start stack"
    }
}

/// Whether a **Start stack** on `worktree` is waiting on its setup to
/// finish — the DAEMON's word, on the setup line (`RunState::run_queued`).
pub(crate) fn run_queued(app: &App, worktree: &WorktreeId) -> bool {
    app.tree
        .terminals
        .iter()
        .any(|t| &t.worktree_id == worktree && t.setup && t.run.run_queued)
}

/// **Restart stack**: `worktree`'s run stopped — a `^C`, so it winds down
/// — then the same app started again in its run line once it has gone,
/// in the background as a start is (`ClientRequest::RestartRun`); with
/// no run line yet, a start. On a run
/// already winding down the DAEMON kills it and starts it at once.
pub(crate) fn restart(app: &mut App, worktree: &WorktreeId, out: &mut Vec<ClientRequest>) {
    if app.is_placeholder_worktree(worktree)
        || !app.tree.worktrees.iter().any(|w| &w.id == worktree)
    {
        return;
    }
    // No run line yet: nothing to restart, so a start — the pick of the
    // checkout's apps, as `⌘⇧S`.
    let has_run = app
        .tree
        .terminals
        .iter()
        .any(|t| &t.worktree_id == worktree && t.run_command.is_some() && !t.setup);
    if !has_run {
        return crate::event_loop::toggle_stack_in(app, worktree, out);
    }
    app.flash = app
        .run_stopping(worktree)
        .map(|_| crate::flash::Flash::note("killing the run"));
    let req_id = app.alloc_req_id(PendingIntent::RunStarted);
    out.push(ClientRequest::RestartRun {
        req_id,
        worktree: worktree.clone(),
    });
}

/// **Stop all stacks**: every checkout's stack that is up or starting is
/// stopped — orion's runs sent their `^C`, so their traps wind them down
/// in view, and the rest stopped by the DAEMON.
pub(crate) fn stop_all(app: &mut App, out: &mut Vec<ClientRequest>) {
    let runs: Vec<WorktreeId> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| live(app, &w.id) && !run_queued(app, &w.id))
        .map(|w| w.id.clone())
        .collect();
    if runs.is_empty() {
        app.flash = Some(crate::flash::Flash::note("no stack is running"));
        return;
    }
    for id in &runs {
        crate::event_loop::toggle_stack_in(app, id, out);
    }
    // One line for the lot, not the last run's own.
    app.flash = Some(crate::flash::Flash::note(format!(
        "stopping {}",
        crate::flash::plural(runs.len(), "stack")
    )));
}

/// Where a run's port is served, as the browser is handed it.
pub(crate) fn port_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// `worktree`'s app: the first port its stack serves.
pub(crate) fn app_port(app: &App, worktree: &WorktreeId) -> Option<u16> {
    app.stack_status(worktree).ports().first().copied()
}

/// A click on a run line's `:3000`: that port, in the browser.
pub(crate) fn open_port(app: &mut App, port: u16) {
    let url = port_url(port);
    if !crate::open_url::open_url(&url, None) {
        app.flash = Some(crate::flash::Flash::failed(format!("couldn't open {url}")));
    }
}

/// ⌘O's **Open app**: the app the cursor's checkout's stack serves — the
/// first port its run printed — in the browser; with none yet, why.
pub(crate) fn open_app(app: &mut App) {
    let Some(worktree) = crate::git_sync::target(app) else {
        return;
    };
    if let Some(port) = app_port(app, &worktree) {
        open_port(app, port);
        return;
    }
    let note = if live(app, &worktree) {
        "the stack isn't serving a port yet".to_string()
    } else {
        let key = crate::hints::key_or(
            &app.keymap,
            crate::keymap::Action::ToggleStack,
            "Start stack",
        );
        format!("no stack running here — {key} starts it")
    };
    app.flash = Some(crate::flash::Flash::note(note));
}
