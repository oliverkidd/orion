//! STACK STATUS in the TUI: the DAEMON's listing of every docker compose
//! stack on the machine (`ServerEvent::StacksChanged`), which the band's
//! `⬡` reads through [`App::stack_of`]. Starting and stopping a checkout's
//! stack is `⌘⇧S`'s (`event_loop::toggle_stack_in`): its RUN COMMAND, in
//! the pane.

use crate::app::App;
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
/// else **Start stack**.
pub(crate) fn menu_label(app: &App, worktree: &WorktreeId) -> &'static str {
    if app.worktree_running(worktree) {
        "Stop stack"
    } else {
        "Start stack"
    }
}

/// **Stop all stacks**: every checkout's run sent its `^C`, so its trap
/// winds its stack down in view.
pub(crate) fn stop_all(app: &mut App, out: &mut Vec<ClientRequest>) {
    let runs: Vec<WorktreeId> = app
        .tree
        .worktrees
        .iter()
        .filter(|w| app.worktree_running(&w.id) && !app.runs_stopping.contains(&w.id))
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
        crate::bundle::plural(runs.len(), "stack")
    )));
}
