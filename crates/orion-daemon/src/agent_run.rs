//! `orion run` from inside an agent session: the checkout's app started
//! in its RUN TERMINAL, as ⌘⇧S starts it — the run line in its band, the
//! ports it serves, **Open app**, and a ⌘⇧S that stops it — instead of in
//! the agent's own shell, where orion only sees it from outside.
//! `orion run --stop` stops it, `--restart` starts it again, `--status`
//! says whether it is up (`stacks`).

use anyhow::{bail, Context, Result};
use orion_core::protocol::AgentRunAction;
use orion_core::stack::StackStatus;
use orion_core::AgentId;
use std::sync::Arc;

use crate::registry::Daemon;

/// What orion appends to Claude's system prompt so an agent that starts,
/// stops or checks the checkout's app or stack does it through orion's
/// run line.
pub const CLAUDE_RUN_GUIDANCE: &str = "[orion] This checkout's app, dev server or stack (`just … \
start`, `docker compose up`, `npm run dev`, `make dev` and the like — anything long-running that \
serves ports) runs in orion's run line, which keeps its status. Never start, stop or restart it in \
your own shell, in the background or with `-d`; run these shell commands instead:\n\n  orion run \
<command>   start it, with the exact command you would have run\n  orion run --stop      stop it\n  \
orion run --restart   stop it and start it again\n  orion run --status    whether it is up, starting, \
stopping or down, and on which ports\n\nEach returns at once: the stack starts in the background, \
and the user sees it in its run line with its ports. A checkout has one stack: `orion run` while one \
is up leaves it running, so use `--restart` to restart it. Check `orion run --status` instead of \
probing ports, processes or docker yourself.";

impl Daemon {
    /// `orion run`, run by the agent inside its own session, on the
    /// caller's worktree's stack. `Ok(Some(status))` answers `Status`.
    pub fn agent_run(
        self: &Arc<Self>,
        id: &AgentId,
        action: &AgentRunAction,
    ) -> Result<Option<StackStatus>> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        let worktree = agent.worktree_id;
        // An agent at work on the stack is the checkout in use: its IDLE
        // STACK STOP waits.
        self.stacks.note_activity(&worktree);
        match action {
            AgentRunAction::Start(command) => self.start_run(&worktree, Some(command)).map(drop)?,
            AgentRunAction::Stop => self.stop_run(&worktree)?,
            AgentRunAction::Restart => {
                let has_run = !self.store.run_terminals_in(&worktree)?.is_empty();
                if !has_run {
                    bail!("no stack of orion's is running here to restart — `orion run <command>` starts one");
                }
                self.restart_run(&worktree)?;
            }
            AgentRunAction::Status => return Ok(Some(self.stacks.status_of(&worktree))),
        }
        Ok(None)
    }
}
