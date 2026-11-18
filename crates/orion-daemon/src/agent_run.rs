//! `orion run <command>` from inside an agent session: the checkout's app
//! started in its RUN TERMINAL, as ⌘⇧S starts it — the run line in its
//! band, the ports it prints, **Open app**, and a ⌘⇧S that stops it —
//! instead of in the agent's own shell, where orion sees none of that.
//! `orion run --stop` stops it again.

use anyhow::{Context, Result};
use orion_core::AgentId;
use std::sync::Arc;

use crate::registry::Daemon;

/// What orion appends to Claude's system prompt so an agent that starts
/// the checkout's app or stack does it through orion's run line.
pub const CLAUDE_RUN_GUIDANCE: &str = "[orion] When you need to start this checkout's app, dev \
server or stack (`just … start`, `docker compose up`, `npm run dev`, `make dev` and the like — \
anything long-running that serves ports), do not run it in your own shell. Run this shell command \
instead:\n\n  orion run <command>\n\nwith the exact command you would have run. orion starts it in \
this checkout's run terminal, where the user sees it as a run line with its ports and can stop it \
with ⌘⇧S; the command returns at once. To stop it, run `orion run --stop`. A checkout has one \
run: `orion run` while one is up leaves that one running, so stop it first to restart it.";

impl Daemon {
    /// `orion run`, run by the agent inside its own session: `command`
    /// started in the caller's worktree's run line, or with None, that run
    /// stopped.
    pub fn agent_run(self: &Arc<Self>, id: &AgentId, command: Option<&str>) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        let worktree = agent.worktree_id;
        match command {
            None => self.stop_run(&worktree),
            Some(command) => self.start_run(&worktree, Some(command)).map(drop),
        }
    }
}
