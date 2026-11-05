//! STACK STATUS in the TUI: the DAEMON's listing of every docker compose
//! stack on the machine (`ServerEvent::StacksChanged`), which the band's
//! `⬡` reads through [`App::stack_of`]. Starting and stopping a checkout's
//! stack is `⌘⇧S`'s (`event_loop::toggle_stack_in`): its RUN COMMAND, in
//! the background, on a run line in its band. The ports that run prints
//! are its app: a click on one, or ⌘O's **Open app**, opens it.

use crate::app::{App, PendingIntent};
use orion_core::compose::Stack;
use orion_core::protocol::ClientRequest;
use orion_core::WorktreeId;

/// The mark a stack wears on its band.
pub const MARK: &str = "⬡";

/// `StacksChanged`: the new listing.
pub(crate) fn listed(app: &mut App, stacks: Option<Vec<Stack>>) {
    app.stacks = stacks;
    app.dirty = true;
}

/// A checkout's menu item for `⌘⇧S`: **Stop stack** while its run is up,
/// **Cancel start** while a start waits on its setup, else **Start stack**.
pub(crate) fn menu_label(app: &App, worktree: &WorktreeId) -> &'static str {
    if app.worktree_running(worktree) {
        "Stop stack"
    } else if run_queued(app, worktree) {
        "Cancel start"
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
/// — then started again in its run line once it has gone, in the
/// background as a start is (`ClientRequest::RestartRun`). On a run
/// already winding down the DAEMON kills it and starts it at once.
pub(crate) fn restart(app: &mut App, worktree: &WorktreeId, out: &mut Vec<ClientRequest>) {
    if app.is_placeholder_worktree(worktree)
        || !app.tree.worktrees.iter().any(|w| &w.id == worktree)
    {
        return;
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

/// **Stop all stacks**: every checkout's run sent its `^C`, so its trap
/// winds its stack down in view.
pub(crate) fn stop_all(app: &mut App, out: &mut Vec<ClientRequest>) {
    let runs: Vec<WorktreeId> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| app.worktree_running(&w.id) && app.run_stopping(&w.id).is_none())
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

/// `worktree`'s app: the first localhost port its live run printed.
pub(crate) fn app_port(app: &App, worktree: &WorktreeId) -> Option<u16> {
    app.run_terminal(worktree)?.run.ports.first().copied()
}

/// A click on a run line's `:3000`: that port, in the browser.
pub(crate) fn open_port(app: &mut App, port: u16) {
    let url = port_url(port);
    if !crate::event_loop::open_url(&url) {
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
    let note = if app.worktree_running(&worktree) {
        "the run hasn't printed a localhost address yet".to_string()
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
