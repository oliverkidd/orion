//! The daemon's world: persisted entity tree + live PTY sessions, and the
//! operations the IPC surface exposes over them.

use crate::claude_bg;
use crate::git;
use crate::hooks::{self, HookEnv};
use crate::pty::{PtyEvent, PtySession, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
use crate::status::{AgentStatusMachine, Effect, HookEvent};
use crate::store::Store;
use anyhow::{bail, Context, Result};
use orion_core::env;
use orion_core::harness::HarnessDescriptor;
use orion_core::{
    Agent, AgentId, AgentKind, AgentStatus, EnterOutcome, Entity, EntityId, LinkId, Project,
    ProjectId, ServerEvent, SessionRef, TerminalId, TerminalTab, WaitReason, Worktree, WorktreeId,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Gap between the boots of a worktree prewarm sweep. A worktree with five
/// agents must not fork five agent CLIs at once: they would all contend for
/// the CPU with the one session the user is actually waiting to see, which
/// is the whole reason the sweep exists. Nothing is watching these, so
/// warming them slowly costs the user nothing.
const PREWARM_STAGGER: Duration = Duration::from_millis(1500);
/// A resumed agent CLI that exits with an error this soon after its spawn
/// is taken to have not found the session it was told to resume. Generous
/// on purpose: the login shell alone takes most of a second, and a worktree
/// prewarm sweep boots CLIs back to back — the 2 s this used to be let slow
/// failures through, leaving the pane on the CLI's error.
const RESUME_FAIL_WINDOW: Duration = Duration::from_secs(10);

/// A resumed spawn, watched for the fast failure of a missing session.
struct ResumeWatch {
    /// The PTY it booted, so a later respawn's exit is never mistaken for it.
    session: std::sync::Weak<PtySession>,
    spawned_at: Instant,
    cols: u16,
    rows: u16,
}
/// `$SHELL -l -i -c <cmd>`: a login *and* interactive shell, so zsh sources
/// ~/.zprofile and ~/.zshrc both and the child sees the PATH the user's
/// terminal has. The CLI probe and the spawn wrapper share it so they can
/// never disagree about what "on the user's PATH" means.
const LOGIN_SHELL_ARGS: [&str; 3] = ["-l", "-i", "-c"];
/// Cap on one CLI probe. A heavy rc file costs ~1s; a hung one must not
/// stall a create forever, so on timeout the CLI is assumed present and
/// the spawn itself gets to report.
const CLI_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// A RUN TERMINAL whose command exited on its own, or a SETUP TERMINAL
/// whose setup failed: the PTY, kept for the replay, and the exit code
/// the OS reported.
type FinishedRun = (Arc<PtySession>, Option<i32>);
/// What a RUN TERMINAL's row is called in the Sessions panel.
const RUN_TERMINAL_NAME: &str = "run";
/// `⌘⇧S` on a worktree with nothing to run: the footer line naming where
/// a RUN COMMAND is set.
const NO_RUN_COMMAND: &str =
    "no run command for this project — set one in Settings → Projects (Run command)";
/// Why a RUN TERMINAL with nothing left to replay will not attach — its
/// run ended before a DAEMON restart, typically.
const RUN_NOT_RUNNING: &str = "this run has stopped — ⌘⇧S on its worktree starts it again";
/// What a SETUP TERMINAL's row is called in the Sessions panel.
const SETUP_TERMINAL_NAME: &str = "setup";
/// Why a SETUP TERMINAL with nothing left to replay will not attach: the
/// DAEMON that ran it has restarted since, and its output went with it.
const SETUP_NOT_RUNNING: &str =
    "this setup ran under a daemon that has restarted since, and its output went with it";
/// How long a RUN COMMAND has to wind down after its `^C` before it is
/// killed outright: a compose stack's own stop waits on every container.
const RUN_STOP_GRACE: Duration = Duration::from_secs(120);

/// A stop under way on a RUN TERMINAL: sent its `^C`, not yet gone.
struct Stop {
    /// The epoch ms its kill lands at (`RunState::stopping_until`).
    kill_at: i64,
    /// A restart's stop: the config to start the run again with once it
    /// has gone. None for a plain stop, whose row goes with the run.
    restart: Option<crate::config::Config>,
}

pub(crate) struct CreateAgentSpec {
    pub worktree: WorktreeId,
    pub name: String,
    pub kind: AgentKind,
    /// The registry id beside `kind`: the Claude account, or the custom
    /// harness ([`orion_core::harness::registry_id`]).
    pub custom_harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub auto_title: bool,
    /// The CLI's positional first prompt (an AGENT PRESET launch). Request-only.
    pub starting_prompt: Option<String>,
    pub pr_url: Option<String>,
    /// The mode the CLI starts in (plan, ask, edit). Request-only.
    pub mode: orion_core::harness::AgentMode,
    /// The AUTOMATION starting the session (`Agent::automation`).
    pub automation: Option<String>,
}

/// What [`Daemon::delete_worktree`] came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeDelete {
    Deleted,
    /// Unforced, and this many files in the checkout have uncommitted or
    /// untracked changes: nothing was killed or removed.
    HasChanges(usize),
}

pub struct Daemon {
    sessions: Mutex<HashMap<SessionRef, Arc<PtySession>>>,
    pub(crate) status_machines: Mutex<HashMap<AgentId, AgentStatusMachine>>,
    pub hook_env: HookEnv,
    /// Shared with the hook HTTP server, which reads agent rows to decide
    /// auto-title injection.
    pub store: Arc<Store>,
    /// Entity/status deltas fanned out to every subscribed client.
    pub events: broadcast::Sender<ServerEvent>,
    /// Every session the registry installs — a spawn, a respawn, a prewarm
    /// adoption — by ref, the moment it is in `sessions`. A client's forward
    /// task (`attach.rs`) listens so a kill-and-respawn behind an attached
    /// pane rebinds it to the new PTY instead of leaving it on a dead one.
    pub session_installs: broadcast::Sender<SessionRef>,
    pub shutdown: tokio_util::sync::CancellationToken,
    /// Serializes worktree create/delete with the background auto-sync so
    /// a checkout is never adopted twice while its row is mid-insert.
    pub(crate) worktree_ops: tokio::sync::Mutex<()>,
    /// Cached `command -v` results per CLI so a missing binary doesn't get
    /// re-probed (login shell spawn) on every create.
    cli_probes: Mutex<HashMap<String, (bool, Instant)>>,
    /// How many client connections are attached per session — a session
    /// with attachments (and its whole worktree) is "in view" and exempt
    /// from idle reaping.
    attach_counts: Mutex<HashMap<SessionRef, usize>>,
    /// When each live session was last "looked at": spawned, prewarmed,
    /// attached, or covered by the in-view sweep refresh. The idle reaper
    /// kills sessions whose stamp ages past `session_idle_timeout`.
    session_interest: Mutex<HashMap<SessionRef, Instant>>,
    /// Last hook-reported cwd per agent, recorded only for payloads that
    /// passed the foreign-session gate. An agent that walks into a checkout
    /// orion hasn't adopted yet leaves its cwd here, so the worktree sync
    /// can finish the re-home once the row exists.
    last_cwd: Mutex<HashMap<AgentId, PathBuf>>,
    /// Where each Claude agent's transcript (and beside it, the session
    /// title `/rename` persists) lives, from its hook payloads. Read by
    /// the CLAUDE TITLE SYNC (`session_title.rs`) when the PTY's window
    /// title changes, which no hook reports.
    pub(crate) transcripts: Mutex<HashMap<AgentId, crate::session_title::TranscriptRef>>,
    /// Why each red (or crashed) agent is waiting on you, from the hook
    /// that turned it red (`WaitReason`). Memory only: a restarted daemon
    /// rebuilds a red row's from its transcript (`Daemon::wait_reason`).
    /// Dropped as soon as the agent leaves red.
    pub(crate) wait_reasons: Mutex<HashMap<AgentId, WaitReason>>,
    /// Agents that ran `orion worktree` and are waiting for their turn to
    /// end: the row already sits under the target worktree while the PTY
    /// still runs in the old checkout. Drained by `complete_pending_move`
    /// on the turn-end hook (kill + respawn resumed in the target), cleared
    /// by any other spawn of the agent, and consulted by the cwd reparent so
    /// the old checkout's cwd can't drag the row back in the meantime.
    pending_moves: Mutex<HashMap<AgentId, Worktree>>,
    /// Serializes the check-and-spawn inside [`Daemon::ensure_session`].
    /// Attach (the request loop) and the worktree prewarm sweep (its own
    /// task) can both reach for the same dead session; without this they
    /// would both miss the registry and fork two CLIs, orphaning one.
    pub(crate) spawn_gate: Mutex<()>,
    /// What this daemon did at boot that the user should hear of (the
    /// WORKTREE RELOCATION's report): flashed to the first client to
    /// subscribe, then gone.
    pub boot_notes: Mutex<Vec<String>>,
    /// The worktree prewarm sweep currently running, so a newer one can
    /// cancel it. Walking the project tabs fires a sweep per step, and
    /// only the project the cursor rests on is worth warming.
    prewarm_sweep: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Resumed agent spawns, by agent, until their PTY exits: a resume that
    /// dies inside [`RESUME_FAIL_WINDOW`] could not find its session, and
    /// `watch_for_exit` reads that off this record.
    resumes: Mutex<HashMap<AgentId, ResumeWatch>>,
    /// RUN TERMINALS whose command exited on its own, nobody having asked
    /// it to, with its exit code (`RunState::exit_code`). The PTY is kept
    /// — outside `sessions`, so the row reads as not alive — so an attach
    /// replays how the run ended instead of running it again. Dropped
    /// when the run starts again, is stopped, or its row goes. SETUP
    /// TERMINALS that failed are kept here the same way.
    finished_runs: Mutex<HashMap<TerminalId, FinishedRun>>,
    /// RUN TERMINALS sent a `^C` by a stop or a restart and not yet gone:
    /// a second press kills them outright.
    stopping_runs: Mutex<HashMap<TerminalId, Stop>>,
    /// SETUP TERMINALS still running that a **Start stack** is waiting
    /// on: the config to start the worktree's run under once each exits
    /// cleanly (`RunState::run_queued`).
    queued_runs: Mutex<HashMap<TerminalId, crate::config::Config>>,
    /// STACK STATUS: the last docker compose listing, and the poke that
    /// asks for a fresh one now.
    pub stacks: crate::stacks::StackWatch,
    /// SPARE WORKTREES: the warm next worktree of each project in use
    /// (`spare`).
    pub(crate) spares: crate::spare::SpareBook,
    /// AWAY: whether you are at the desk, for the phone bridge (`away`).
    pub away: crate::away::AwayWatch,
    /// The SLACK BRIDGE's connection and status (`slack`).
    pub slack: crate::slack::SlackBridge,
}

impl Daemon {
    pub fn new(store: Arc<Store>, hook_env: HookEnv) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        let (session_installs, _) = broadcast::channel(64);
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            status_machines: Mutex::new(HashMap::new()),
            hook_env,
            store,
            events,
            session_installs,
            shutdown: tokio_util::sync::CancellationToken::new(),
            worktree_ops: tokio::sync::Mutex::new(()),
            cli_probes: Mutex::new(HashMap::new()),
            attach_counts: Mutex::new(HashMap::new()),
            session_interest: Mutex::new(HashMap::new()),
            last_cwd: Mutex::new(HashMap::new()),
            transcripts: Mutex::new(HashMap::new()),
            wait_reasons: Mutex::new(HashMap::new()),
            pending_moves: Mutex::new(HashMap::new()),
            spawn_gate: Mutex::new(()),
            boot_notes: Mutex::new(Vec::new()),
            prewarm_sweep: Mutex::new(None),
            resumes: Mutex::new(HashMap::new()),
            finished_runs: Mutex::new(HashMap::new()),
            stopping_runs: Mutex::new(HashMap::new()),
            queued_runs: Mutex::new(HashMap::new()),
            stacks: crate::stacks::StackWatch::default(),
            spares: Default::default(),
            away: Default::default(),
            slack: Default::default(),
        })
    }

    // ---- status machine plumbing ----

    /// Feed one hook (or synthetic) event through the agent's status machine
    /// and apply the resulting effects (persist + broadcast).
    pub fn apply_hook_event(
        &self,
        agent_id: &AgentId,
        event: HookEvent,
        session_id: Option<String>,
    ) {
        self.apply_hook_event_with(agent_id, event, session_id, None);
    }

    /// [`Self::apply_hook_event`] for an event that may say why the turn
    /// waits on you (`HookDelivery::reason`). The reason is recorded only
    /// when the event is this agent's own and leaves it red, so the
    /// `StatusChanged` into red carries it. The PTY dying with an error
    /// is its own reason: a crash.
    pub fn apply_hook_event_with(
        &self,
        agent_id: &AgentId,
        event: HookEvent,
        session_id: Option<String>,
        reason: Option<WaitReason>,
    ) {
        let reason = match &event {
            HookEvent::SessionEnded { exit_code } => Some(WaitReason::Crash {
                exit_code: *exit_code,
            }),
            _ => reason,
        };
        // A `orion worktree` relocation waiting on this agent's turn end
        // holds that end at `running` (`AgentStatusMachine::set_relocating`;
        // `complete_pending_move` drains it). Read off the daemon's own
        // record before every event, so the machine's copy cannot go stale
        // — and before the machines lock, so the two are never nested.
        let relocating = self.pending_moves.lock().unwrap().contains_key(agent_id);
        // A SPARE WORKTREE's CLI is up once it says so, row or none.
        if matches!(event, HookEvent::SessionStart { .. }) {
            self.spares.note_boot(agent_id);
        }
        let handled = {
            let mut machines = self.status_machines.lock().unwrap();
            let machine = match machines.entry(agent_id.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => Some(e.into_mut()),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    // Lazily seed from the persisted row.
                    match self.store.get_agent(agent_id) {
                        Ok(Some(agent)) => {
                            let limited = agent.limit_reached().is_some();
                            Some(
                                slot.insert(
                                    AgentStatusMachine::new(agent.status, agent.session_id)
                                        .at_limit(limited),
                                ),
                            )
                        }
                        // No row: a stale env's id, or an agent deleted
                        // since — nothing to drive.
                        _ => None,
                    }
                }
            };
            machine.map(|machine| {
                machine.set_relocating(relocating);
                let foreign = machine.is_foreign(&event, session_id.as_deref());
                let effects = machine.handle(event, session_id.as_deref(), Instant::now());
                (effects, (!foreign).then(|| machine.status()))
            })
        };
        if let Some((effects, own_status)) = handled {
            let replaced = match (own_status, reason) {
                (Some(status), Some(reason)) => self.note_wait_reason(agent_id, status, reason),
                _ => None,
            };
            let status_set = effects.iter().any(|e| matches!(e, Effect::SetStatus(_)));
            self.apply_status_effects(agent_id, effects);
            // Still red, over a new dialog: no status change says so, so
            // the red one is said again with the new reason — the phone
            // bridge posts its message, the NEEDS YOU card reads anew. A
            // steered plan that goes on to ask a question gets no hook in
            // between to turn it yellow.
            if let (Some(reason), false) = (replaced, status_set) {
                self.rebroadcast_wait(agent_id, reason);
            }
        }
    }

    /// Record why `id` waits on you, when `status` (the machine's, after
    /// the event that carried `reason`) is the one it explains: a dialog's
    /// reason while red, a crash once terminated. The newest wins — one
    /// dialog shows at a time. Returns the reason when it replaced a
    /// different one while red: a new dialog over the last.
    fn note_wait_reason(
        &self,
        id: &AgentId,
        status: AgentStatus,
        reason: WaitReason,
    ) -> Option<WaitReason> {
        let explains = match reason {
            WaitReason::Crash { .. } => status == AgentStatus::Terminated,
            _ => status == AgentStatus::NeedsFeedback,
        };
        if !explains {
            return None;
        }
        let before = self
            .wait_reasons
            .lock()
            .unwrap()
            .insert(id.clone(), reason.clone());
        let new_dialog =
            status == AgentStatus::NeedsFeedback && before.is_some_and(|before| before != reason);
        new_dialog.then_some(reason)
    }

    /// Say again that `id` is red, now over `reason`: the row's own stamp
    /// and unseen flag, so nothing re-sorts.
    fn rebroadcast_wait(&self, id: &AgentId, reason: WaitReason) {
        let Ok(Some(agent)) = self.store.get_agent(id) else {
            return;
        };
        if agent.status != AgentStatus::NeedsFeedback {
            return;
        }
        tracing::info!(agent = %id, "a new dialog while already waiting");
        self.broadcast(ServerEvent::StatusChanged {
            agent: id.clone(),
            status: AgentStatus::NeedsFeedback,
            changed_at: agent.status_changed_at,
            unseen: agent.unseen,
            reason: Some(reason),
        });
    }

    /// The reason a `StatusChanged` into `status` carries: the recorded
    /// one while red or terminated; leaving those drops it.
    fn reason_for_status(&self, id: &AgentId, status: AgentStatus) -> Option<WaitReason> {
        let mut reasons = self.wait_reasons.lock().unwrap();
        match status {
            AgentStatus::NeedsFeedback | AgentStatus::Terminated => reasons.get(id).cloned(),
            _ => {
                reasons.remove(id);
                None
            }
        }
    }

    /// Deferred-finish recheck across all machines (runs on a timer).
    pub fn tick_status_machines(&self) {
        let now = Instant::now();
        let ticked: Vec<(AgentId, Vec<Effect>)> = {
            let mut machines = self.status_machines.lock().unwrap();
            machines
                .iter_mut()
                .map(|(id, m)| (id.clone(), m.tick(now)))
                .collect()
        };
        for (id, effects) in ticked {
            self.apply_status_effects(&id, effects);
        }
    }

    fn apply_status_effects(&self, agent_id: &AgentId, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::SetStatus(status) => {
                    let (changed_at, unseen) = match self.store.set_agent_status(agent_id, status) {
                        Ok(stamped) => stamped,
                        Err(e) => {
                            tracing::warn!(error = %e, "persist status failed");
                            (orion_core::clock::now_ms(), false)
                        }
                    };
                    self.broadcast(ServerEvent::StatusChanged {
                        agent: agent_id.clone(),
                        status,
                        changed_at,
                        unseen,
                        reason: self.reason_for_status(agent_id, status),
                    });
                }
                Effect::SaveSessionId(sid) => {
                    if let Err(e) = self.store.set_agent_session_id(agent_id, Some(&sid)) {
                        tracing::warn!(error = %e, "persist session id failed");
                    }
                }
                // The reason the row is red rides the row itself, so an
                // upsert carries it — ahead of the StatusChanged that
                // turns it red, which a client rings on.
                Effect::SetUsageLimit(limit) => {
                    match self.store.set_agent_usage_limit(agent_id, limit.as_ref()) {
                        Ok(true) => {
                            if let Some(limit) = &limit {
                                tracing::info!(agent = %agent_id, reason = limit.reason.as_str(), "session stopped on a usage limit");
                            }
                            self.try_broadcast_agent(agent_id);
                        }
                        Ok(false) => {}
                        Err(e) => tracing::warn!(error = %e, "persist usage limit failed"),
                    }
                }
            }
        }
    }

    pub fn broadcast(&self, ev: ServerEvent) {
        let _ = self.events.send(ev);
    }

    pub fn session(&self, sref: &SessionRef) -> Option<Arc<PtySession>> {
        self.sessions.lock().unwrap().get(sref).cloned()
    }

    pub fn is_alive(&self, sref: &SessionRef) -> bool {
        self.sessions.lock().unwrap().contains_key(sref)
    }

    /// Whether `session` is still the PTY registered for `sref` — not one
    /// a kill (restart, move, relocation) has since replaced.
    fn owns_session(&self, sref: &SessionRef, session: &Arc<PtySession>) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(sref)
            .is_some_and(|current| Arc::ptr_eq(current, session))
    }

    /// (session, child pid, a SPARE WORKTREE's?) for every live PTY — the
    /// metrics reading's input.
    pub fn session_pids(&self) -> Vec<(SessionRef, u32, bool)> {
        let live: Vec<(SessionRef, u32)> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(sref, s)| Some((sref.clone(), s.child_pid?)))
            .collect();
        // Asked after the sessions lock is let go: the spare book is never
        // taken inside it.
        let spares = self.spares.sessions();
        live.into_iter()
            .map(|(sref, pid)| {
                let spare = spares.contains(&sref);
                (sref, pid, spare)
            })
            .collect()
    }

    pub fn remove_session(&self, sref: &SessionRef) -> Option<Arc<PtySession>> {
        self.session_interest.lock().unwrap().remove(sref);
        self.sessions.lock().unwrap().remove(sref)
    }

    pub fn kill_session(&self, sref: &SessionRef) {
        if let Some(s) = self.remove_session(sref) {
            s.kill();
        }
    }

    pub fn kill_all(&self) {
        for (_, s) in self.sessions.lock().unwrap().drain() {
            s.kill();
        }
    }

    // ---- attach tracking & idle reaping ----

    /// A client attached to `sref` (the server dedupes re-attaches per
    /// connection). While any attachment exists, the session — and its
    /// whole worktree — counts as "in view".
    pub fn note_attached(&self, sref: &SessionRef) {
        if let Some(worktree) = self.session_worktree(sref) {
            self.note_used_in(&worktree);
        }
        *self
            .attach_counts
            .lock()
            .unwrap()
            .entry(sref.clone())
            .or_insert(0) += 1;
        self.touch_session(sref);
    }

    /// A client detached from `sref` (or its connection dropped). Restamps
    /// the session so the idle clock starts at "stopped looking", not at
    /// spawn time.
    pub fn note_detached(&self, sref: &SessionRef) {
        let mut counts = self.attach_counts.lock().unwrap();
        if let Some(n) = counts.get_mut(sref) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                counts.remove(sref);
            }
        }
        drop(counts);
        self.touch_session(sref);
    }

    /// Worktree `id`'s project was just used, which keeps it a SPARE
    /// WORKTREE.
    fn note_used_in(&self, id: &WorktreeId) {
        if let Ok(Some(worktree)) = self.store.get_worktree(id) {
            self.spares.note_used(&worktree.project_id);
        }
    }

    /// Stamp `sref` as just-looked-at for the idle reaper.
    fn touch_session(&self, sref: &SessionRef) {
        self.session_interest
            .lock()
            .unwrap()
            .insert(sref.clone(), Instant::now());
    }

    /// Kill idle sessions in worktrees no client is looking at, per
    /// `session_idle_timeout` — this bounds what prewarming and
    /// walked-away-from sessions cost. "In view" = the worktree holding any
    /// attached session; in-view sessions get their stamps refreshed
    /// instead, so the full timeout starts only when the user leaves.
    /// Spared regardless of age: agents that are running or waiting on
    /// feedback, agents with a backgrounded tool call still running (a job
    /// detached from their terminal — see `pty::detached_job_under`),
    /// terminals with a command running, and sessions with no row. A reaped
    /// session revives on the next attach or prewarm; agents resume their
    /// conversation.
    pub fn reap_idle_sessions(self: &Arc<Self>) {
        let Some(timeout) = crate::config::Config::load().session_idle_timeout() else {
            return;
        };
        let sessions: Vec<(SessionRef, Arc<PtySession>)> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let attached: std::collections::HashSet<SessionRef> =
            self.attach_counts.lock().unwrap().keys().cloned().collect();
        let viewed_worktrees: std::collections::HashSet<WorktreeId> = attached
            .iter()
            .filter_map(|sref| self.session_worktree(sref))
            .collect();
        let now = Instant::now();
        for (sref, session) in sessions {
            // No store row: deleted mid-sweep, its delete kills the PTY.
            let Some(worktree_id) = self.session_worktree(&sref) else {
                continue;
            };
            if attached.contains(&sref) || viewed_worktrees.contains(&worktree_id) {
                self.touch_session(&sref);
                continue;
            }
            let age = {
                let mut interest = self.session_interest.lock().unwrap();
                // A missing stamp (session predating the map) starts aging now.
                now.duration_since(*interest.entry(sref.clone()).or_insert(now))
            };
            if age < timeout {
                continue;
            }
            let spared = match &sref {
                SessionRef::Agent(id) => match self.store.get_agent(id).ok().flatten() {
                    Some(agent)
                        if matches!(
                            agent.status,
                            AgentStatus::Running | AgentStatus::NeedsFeedback
                        ) =>
                    {
                        true
                    }
                    // A backgrounded tool call — Claude's `run_in_background`
                    // Bash, its Monitor watch, a Codex shell command —
                    // outlives the turn that started it, and the hook-fed
                    // status machine read that turn's Stop as Finished. The
                    // process tree still knows (#78). Restamped rather than
                    // just skipped: the job ending is the moment the agent
                    // wakes to read its result, so the clock starts there.
                    Some(_) => {
                        let busy = agent_has_detached_job(&session);
                        if busy {
                            self.touch_session(&sref);
                        }
                        busy
                    }
                    // Row vanished mid-sweep: its delete kills the PTY anyway.
                    None => true,
                },
                // A RUN TERMINAL is the server `r` started: sitting quiet
                // is what it is for, and only `r` again stops it. A SETUP
                // TERMINAL's install may be as quiet, and must finish.
                SessionRef::Terminal(id) => {
                    self.command_terminal(id).is_some() || shell_has_children(&session)
                }
            };
            if spared {
                continue;
            }
            tracing::info!(session = ?sref, idle_secs = age.as_secs(), "reaping idle session");
            self.kill_session(&sref);
            let upsert = match &sref {
                SessionRef::Agent(id) => self.agent_entity(id).map(Entity::Agent),
                SessionRef::Terminal(id) => self.terminal_entity(id).map(Entity::Terminal),
            };
            if let Ok(entity) = upsert {
                self.broadcast(ServerEvent::EntityUpserted { entity });
            }
        }
    }

    /// The worktree a session's row lives under; None when the row is gone.
    fn session_worktree(&self, sref: &SessionRef) -> Option<WorktreeId> {
        match sref {
            SessionRef::Agent(id) => self
                .store
                .get_agent(id)
                .ok()
                .flatten()
                .map(|a| a.worktree_id),
            SessionRef::Terminal(id) => self
                .store
                .get_terminal(id)
                .ok()
                .flatten()
                .map(|t| t.worktree_id),
        }
    }

    // ---- snapshot ----

    pub fn snapshot(&self) -> Result<ServerEvent> {
        let (projects, worktrees, mut agents, mut terminals) = self.store.load_tree()?;
        {
            let sessions = self.sessions.lock().unwrap();
            for a in &mut agents {
                a.alive = sessions.contains_key(&SessionRef::Agent(a.id.clone()));
            }
            for t in &mut terminals {
                t.alive = sessions.contains_key(&SessionRef::Terminal(t.id.clone()));
            }
        }
        for t in &mut terminals {
            t.run = self.run_state(&t.id);
        }
        Ok(ServerEvent::Snapshot {
            projects,
            worktrees,
            agents,
            terminals,
            links: self.store.load_links()?,
            pr_seen: self.store.load_pr_seen()?,
            ui_state: self.store.load_ui_state()?,
        })
    }

    fn agent_entity(&self, id: &AgentId) -> Result<Agent> {
        let mut agent = self.store.get_agent(id)?.context("agent not found")?;
        agent.alive = self.is_alive(&SessionRef::Agent(id.clone()));
        Ok(agent)
    }

    /// Push the agent's current row — liveness included —
    /// to every subscriber. The tail of every mutation that changes how
    /// the row renders; fails only when the row is gone.
    fn broadcast_agent(&self, id: &AgentId) -> Result<()> {
        let agent = self.agent_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(agent),
        });
        Ok(())
    }

    /// [`Self::broadcast_agent`] for the best-effort sites — background
    /// tasks and post-respawn refreshes — where a row deleted meanwhile is
    /// not an error: nothing to show, so nothing to say.
    pub(crate) fn try_broadcast_agent(&self, id: &AgentId) {
        let _ = self.broadcast_agent(id);
    }

    fn terminal_entity(&self, id: &TerminalId) -> Result<TerminalTab> {
        let mut term = self.store.get_terminal(id)?.context("terminal not found")?;
        term.alive = self.is_alive(&SessionRef::Terminal(id.clone()));
        term.run = self.run_state(id);
        Ok(term)
    }

    /// [`TerminalTab::run`] for terminal `id`, from what the DAEMON holds
    /// of its run. Takes no lock a caller of it may hold.
    fn run_state(&self, id: &TerminalId) -> orion_core::RunState {
        let sref = SessionRef::Terminal(id.clone());
        let (stopping_until, restarting) = self
            .stopping_runs
            .lock()
            .unwrap()
            .get(id)
            .map_or((None, false), |stop| {
                (Some(stop.kill_at), stop.restart.is_some())
            });
        orion_core::RunState {
            ports: self.session(&sref).map(|s| s.ports()).unwrap_or_default(),
            stopping_until,
            restarting,
            exit_code: self
                .finished_run_exit(&SessionRef::Terminal(id.clone()))
                .flatten(),
            run_queued: self.queued_runs.lock().unwrap().contains_key(id),
        }
    }

    /// Push terminal `id`'s current row to every subscriber.
    fn broadcast_terminal(&self, id: &TerminalId) -> Result<()> {
        let term = self.terminal_entity(id)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Terminal(term),
        });
        Ok(())
    }

    // ---- projects ----

    /// Register a repo as a project. One repo is one project: a path that
    /// resolves to a repo already registered — its root or any checkout of
    /// it — is refused.
    pub async fn add_project(
        self: &Arc<Self>,
        path: &Path,
        name: Option<String>,
        create_missing: bool,
    ) -> Result<EntityId> {
        if create_missing {
            if !path.exists() {
                tokio::fs::create_dir_all(path)
                    .await
                    .with_context(|| format!("create {}", path.display()))?;
            }
            // A project is a repository, so the confirmed folder becomes
            // one — unless it already sits inside one, or git is missing
            // and the check below should say so.
            match git::repo_toplevel(path).await {
                Err(e) if !git::is_missing(&e) => git::init(path).await?,
                _ => {}
            }
        }
        // "not a git repository" is the right explanation only when git ran and
        // said no — if git itself is missing, that message blames the wrong
        // thing, so let git.rs's own diagnosis through untouched.
        let toplevel = git::repo_toplevel(path).await.map_err(|e| {
            if git::is_missing(&e) {
                e
            } else {
                e.context(format!("{} is not a git repository", path.display()))
            }
        })?;
        // `--show-toplevel` answers with the checkout it was run in, so inside a
        // linked worktree it names the worktree rather than the repo. A project
        // is the repo: root it at the main checkout, which `git worktree list`
        // always puts first. Adding from inside a worktree used to name the
        // project after that worktree and leave its ⌂ root row pointing at a
        // directory the project did not own.
        let entries = git::list_worktrees(&toplevel)
            .await
            .with_context(|| format!("list checkouts of {}", toplevel.display()))?;
        let repo_path = match entries.first() {
            Some(main) => main.path.clone(),
            // git listing no checkout at all for a path it just called a work
            // tree would leave the root unknowable; refuse rather than seed a
            // project with no rows, which is how a project loses its root row.
            None => bail!("git listed no checkout for {}", toplevel.display()),
        };
        if self.store.project_by_path(&repo_path)?.is_some() {
            bail!("project already added: {}", repo_path.display());
        }
        let name = name.unwrap_or_else(|| Project::folder_name(&repo_path));
        let project = Project {
            id: ProjectId::generate(),
            name,
            repo_path: repo_path.clone(),
            sort_order: self.store.next_project_sort_order()?,
        };
        self.store.insert_project(&project)?;
        self.spares.note_used(&project.id);
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project.clone()),
        });

        // Main checkout is modeled as a worktree row; adopt pre-existing
        // worktrees too so `orion` matches reality on day one. Root-ness is
        // the path test the reconcile uses, not insert order — the two agreeing
        // is what keeps `repo_path` and the ⌂ root row the same directory.
        for entry in entries {
            let worktree = Worktree {
                id: WorktreeId::generate(),
                project_id: project.id.clone(),
                is_main: entry.path == repo_path,
                path: entry.path.clone(),
                branch: entry.branch,
                sort_order: 0,
            };
            self.store.insert_worktree(&worktree)?;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        Ok(EntityId::Project(project.id))
    }

    /// Retitle a project's row. Cosmetic only — the checkout on disk is never
    /// renamed, and every worktree under the project keeps its own path. An
    /// empty name resets the row to the folder's name, which is the only way
    /// back once a project has been renamed.
    pub fn rename_project(self: &Arc<Self>, id: &ProjectId, name: &str) -> Result<()> {
        let mut project = self.store.get_project(id)?.context("project not found")?;
        let name = name.trim();
        project.name = if name.is_empty() {
            Project::folder_name(&project.repo_path)
        } else {
            name.to_string()
        };
        self.store.rename_project(id, &project.name)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Project(project),
        });
        Ok(())
    }

    pub fn remove_project(self: &Arc<Self>, id: &ProjectId) -> Result<()> {
        // Kill any live sessions under this project first.
        let (_, worktrees, agents, terminals) = self.store.load_tree()?;
        let wt_ids: Vec<WorktreeId> = worktrees
            .into_iter()
            .filter(|w| &w.project_id == id)
            .map(|w| w.id)
            .collect();
        self.kill_sessions_in(&wt_ids, &agents, &terminals);
        // Removing a project only forgets it in orion — never touches disk,
        // but for its SPARE WORKTREE, which was orion's own.
        self.store.delete_project(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Project(id.clone()),
        });
        let daemon = self.clone();
        let id = id.clone();
        tokio::spawn(async move {
            daemon.remove_spare(&id, false).await;
            daemon.spares.forget_project(&id);
        });
        Ok(())
    }

    // ---- worktrees ----

    pub async fn create_worktree(
        self: &Arc<Self>,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
        existing: bool,
    ) -> Result<EntityId> {
        let config = crate::config::Config::load();
        self.create_worktree_with(project_id, branch, base, existing, &config)
            .await
    }

    /// [`Daemon::create_worktree`] against a given config, for the tests
    /// that can't pin the settings file.
    pub async fn create_worktree_with(
        self: &Arc<Self>,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
        existing: bool,
        config: &crate::config::Config,
    ) -> Result<EntityId> {
        if branch.trim().is_empty() {
            bail!("branch name is empty");
        }
        self.spares.note_used(project_id);
        let ops = self.worktree_ops.lock().await;
        let project = self
            .store
            .get_project(project_id)?
            .context("project not found")?;
        // A branch that already exists — the WORKTREE PICKER's branch rows,
        // a teammate's work — is checked out rather than cut: its own
        // commits, its own upstream (`git::add_branch_worktree`). One
        // checkout per branch, as git keeps it, so a project's row already
        // on it is the answer.
        if existing {
            let local = git::local_branch_name(branch.trim());
            let (_, worktrees, _, _) = self.store.load_tree()?;
            if let Some(w) = worktrees
                .into_iter()
                .find(|w| &w.project_id == project_id && w.branch == local)
            {
                return Ok(EntityId::Worktree(w.id));
            }
            let (path, local) = git::add_branch_worktree(&project.repo_path, branch).await?;
            let worktree = self.register_worktree(project_id, path, &local)?;
            self.provision_worktree(&project.repo_path, &worktree, config)
                .await;
            drop(ops);
            return Ok(EntityId::Worktree(worktree.id));
        }
        // A base the caller named (`orion worktree --base`) is resolved
        // against the fetched origin — `main` means `origin/main`, never
        // this checkout's local branch; every other new WORKTREE — `n` in
        // the WORKTREES PANEL, a bare `orion worktree`, the QUICK PROMPT's
        // auto-created one — starts at the project's **Base branch** when
        // one is set (`dev`, resolved the same way), else at the fetched
        // `origin/HEAD`; never at this checkout's HEAD.
        let path = match base {
            Some(base) => git::add_worktree_off_ref(&project.repo_path, branch, base).await?,
            None => match config.base_branch(&project.repo_path) {
                Some(configured) => {
                    git::add_worktree_off_configured(&project.repo_path, branch, configured).await?
                }
                None => git::add_worktree_off_default(&project.repo_path, branch).await?,
            },
        };
        let worktree = self.register_worktree(project_id, path, branch)?;
        self.provision_worktree(&project.repo_path, &worktree, config)
            .await;
        drop(ops);
        Ok(EntityId::Worktree(worktree.id))
    }

    /// What a checkout orion just cut for the project at `repo` gets once
    /// its row is out: its ENV LINKS, then its SETUP COMMAND started — the
    /// setup runs on past the Ack, for as long as it takes.
    async fn provision_worktree(
        self: &Arc<Self>,
        repo: &Path,
        worktree: &Worktree,
        config: &crate::config::Config,
    ) {
        self.link_env_files(repo, &worktree.path).await;
        self.setup_worktree(repo, worktree, config);
    }

    /// The checkout every PR SESSION for pull request `number` runs in: the
    /// PROJECT's worktree already on its head branch `head` (the ROOT
    /// WORKTREE only when the branch is checked out there — git allows a
    /// branch in one checkout at a time), or a new one under the WORKTREE
    /// DIR with the branch fetched from `origin` (`git::add_pr_worktree`).
    /// `head` is the checkout's branch as the client names it: a fork's
    /// arrives under its owner's name (`givemeurhats/main`), so a
    /// contributor's `main` never matches the ROOT WORKTREE on ours.
    /// Serialized with the other worktree ops, so two PR SESSIONS launched
    /// together get one checkout, not a race to create it.
    pub(crate) async fn pr_worktree(
        self: &Arc<Self>,
        project_id: &ProjectId,
        number: u64,
        head: &str,
    ) -> Result<Worktree> {
        let ops = self.worktree_ops.lock().await;
        let project = self
            .store
            .get_project(project_id)?
            .context("project not found")?;
        let (_, worktrees, _, _) = self.store.load_tree()?;
        if let Some(existing) = worktrees
            .into_iter()
            .find(|w| &w.project_id == project_id && w.branch == head)
        {
            return Ok(existing);
        }
        let path = git::add_pr_worktree(&project.repo_path, number, head).await?;
        let worktree = self.register_worktree(project_id, path, head)?;
        let config = crate::config::Config::load();
        self.provision_worktree(&project.repo_path, &worktree, &config)
            .await;
        drop(ops);
        Ok(worktree)
    }

    /// Record a checkout git just made as a worktree row and tell every
    /// client. Callers hold `worktree_ops`.
    fn register_worktree(
        &self,
        project_id: &ProjectId,
        path: PathBuf,
        branch: &str,
    ) -> Result<Worktree> {
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project_id.clone(),
            path,
            branch: branch.to_string(),
            is_main: false,
            sort_order: 0,
        };
        self.store.insert_worktree(&worktree)?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Worktree(worktree.clone()),
        });
        Ok(worktree)
    }

    /// Delete a WORKTREE: its sessions, its checkout, its row. Unforced, a
    /// checkout with uncommitted or untracked changes is left exactly as
    /// it was — no session killed — and the answer says how many files,
    /// so the client can ask before a forced resend loses them. The
    /// branch survives either way, and with it every commit on it.
    pub async fn delete_worktree(
        self: &Arc<Self>,
        id: &WorktreeId,
        force: bool,
    ) -> Result<WorktreeDelete> {
        // Answer a changed checkout before queueing for `worktree_ops`: a
        // run of deletes holds it through git and containers, and the
        // question shouldn't wait for all of that to be asked.
        if !force {
            if let Some(files) = self.uncommitted_in(id).await? {
                return Ok(WorktreeDelete::HasChanges(files));
            }
        }
        let ops = self.worktree_ops.lock().await;
        let worktree = self.store.get_worktree(id)?.context("worktree not found")?;
        if worktree.is_main {
            bail!("cannot delete the main checkout — remove the project instead");
        }
        let project = self
            .store
            .get_project(&worktree.project_id)?
            .context("project not found")?;
        // Asked again under the lock: the checkout's sessions kept running
        // while this waited for it, and may have written since.
        if !force {
            if let Some(files) = self.uncommitted_in(id).await? {
                return Ok(WorktreeDelete::HasChanges(files));
            }
        }

        // Kill sessions living in this worktree.
        let (_, _, agents, terminals) = self.store.load_tree()?;
        self.kill_sessions_in(std::slice::from_ref(id), &agents, &terminals);

        // Forced either way: the changes check above is the only one the
        // user is asked about, and git's own refuses a clean checkout
        // that holds a submodule.
        git::remove_worktree(&project.repo_path, &worktree.path, true).await?;
        self.store.delete_worktree(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Worktree(id.clone()),
        });
        // The delete has happened as far as git and every client are
        // concerned; WORKTREE CONTAINERS only release what the checkout
        // started elsewhere, so they go after, and a failure is a warning
        // — never an Error for this request, which would put the rows
        // back in the TUI. Still under the lock: a create of the same path
        // waits until they are released, and their "still on disk" check
        // sees the delete's result, not a recreate's.
        self.release_containers(&project.repo_path, &worktree.path)
            .await;
        drop(ops);
        Ok(WorktreeDelete::Deleted)
    }

    /// How many files worktree `id` has with uncommitted or untracked
    /// changes, when it has any. The main checkout — never deleted — and
    /// a checkout that is gone or git can't read report none; the removal
    /// says what is wrong with those.
    async fn uncommitted_in(&self, id: &WorktreeId) -> Result<Option<usize>> {
        let worktree = self.store.get_worktree(id)?.context("worktree not found")?;
        if worktree.is_main || !worktree.path.exists() {
            return Ok(None);
        }
        let changed = git::changed_files(&worktree.path).await.unwrap_or(0);
        Ok((changed > 0).then_some(changed))
    }

    /// ENV LINKS for a checkout just added to the project cloned at
    /// `repo`: the main checkout — git's first worktree, which is `repo`
    /// itself unless the project was added from a linked one — lends its
    /// ignored `.env*` files to `worktree`, always; a failure is a client
    /// warning, never a failed create.
    pub(crate) async fn link_env_files(&self, repo: &Path, worktree: &Path) {
        let main = git::list_worktrees(repo)
            .await
            .ok()
            .and_then(|entries| entries.into_iter().next())
            .map_or_else(|| repo.to_path_buf(), |main| main.path);
        self.link_env_files_from(&main, worktree).await;
    }

    /// [`Daemon::link_env_files`] with the main checkout already known.
    async fn link_env_files_from(&self, main: &Path, worktree: &Path) {
        match crate::env_links::link(main, worktree).await {
            Ok(linked) if !linked.is_empty() => tracing::info!(
                worktree = %worktree.display(),
                files = ?linked,
                "linked .env files from the main checkout"
            ),
            Ok(_) => {}
            Err(e) => self.warn_clients(format!(
                "couldn't link .env files into {}: {e:#}",
                worktree.display()
            )),
        }
    }

    /// Stop or tear down the compose projects started in a just-deleted
    /// checkout of the project at `repo`, as its **On worktree delete**
    /// says; whatever goes wrong is a client warning.
    pub(crate) async fn release_containers(&self, repo: &Path, worktree: &Path) {
        let policy = crate::config::Config::load().worktree_containers(repo);
        for warning in crate::containers::release(worktree, policy).await {
            self.warn_clients(warning);
        }
    }

    /// Tell every client about something that went wrong after a request
    /// had already succeeded. Rides `ServerEvent::Error` with no `req_id`,
    /// which the TUI shows as a flash and ties to no pending intent.
    pub(crate) fn warn_clients(&self, message: String) {
        tracing::warn!("{message}");
        self.broadcast(ServerEvent::Error {
            req_id: None,
            message,
        });
    }

    /// Reconcile a project's worktree rows with `git worktree list` so
    /// checkouts made outside orion (an agent running `git worktree add`,
    /// manual CLI use) appear without a restart. Adopts unknown checkouts;
    /// refreshes the branch on known rows after an in-place checkout;
    /// drops rows whose checkout vanished — except the main row and rows
    /// that still have sessions, which the user must delete deliberately.
    pub async fn sync_project_worktrees(self: &Arc<Self>, project: &Project) -> Result<()> {
        let adopted = {
            let _ops = self.worktree_ops.lock().await;
            self.reconcile_project_worktrees(project).await?
        };
        // Outside the ops lock: the replay only touches agent rows, and a
        // just-adopted checkout is exactly where a session that ran
        // `git worktree add` itself already lives.
        if adopted {
            self.reparent_agents_by_last_cwd(project);
        }
        Ok(())
    }

    /// The reconcile half of `sync_project_worktrees`. Returns whether any
    /// checkout was newly adopted.
    async fn reconcile_project_worktrees(self: &Arc<Self>, project: &Project) -> Result<bool> {
        let mut adopted = false;
        let entries = git::list_worktrees(&project.repo_path).await?;
        // git lists the main checkout first, and that — not the order rows
        // happened to be inserted in — is what makes a row the ⌂ root row.
        // Deriving it here every pass repairs a project whose rows were seeded
        // before the root was known, and keeps root-ness following the repo
        // when the checkouts underneath it change.
        let main_path = entries.first().map(|e| e.path.clone());
        let is_root = |path: &Path| main_path.as_deref() == Some(path);
        let (_, worktrees, agents, terminals) = self.store.load_tree()?;
        let links = self.store.load_links()?;
        let ours: Vec<&Worktree> = worktrees
            .iter()
            .filter(|w| w.project_id == project.id)
            .collect();
        let occupied = |id: &WorktreeId| {
            agents.iter().any(|a| &a.worktree_id == id)
                || terminals.iter().any(|t| &t.worktree_id == id)
        };
        // Rows a MOVED CHECKOUT took over, already standing on a path in
        // git's list: the vanished-row sweep below leaves them be.
        let mut rehomed: HashSet<WorktreeId> = HashSet::new();
        for entry in &entries {
            if let Some(known) = ours.iter().find(|w| w.path == entry.path) {
                // A MOVED CHECKOUT this reconcile adopted as a row of its
                // own before it knew better: the old row keeps the sessions
                // and the new one only the path. The old row takes the path
                // and the new one goes, so there is one row again, with the
                // sessions on the checkout they work in.
                let bare = !occupied(&known.id) && !links.iter().any(|l| l.worktree_id == known.id);
                if bare && !is_root(&entry.path) {
                    if let Some(stale) = moved_from(&ours, &entries, &entry.branch, &rehomed)
                        .filter(|stale| stale.id != known.id && occupied(&stale.id))
                    {
                        self.store.delete_worktree(&known.id)?;
                        self.broadcast(ServerEvent::EntityRemoved {
                            id: EntityId::Worktree(known.id.clone()),
                        });
                        self.rehome_moved_row(stale, entry)?;
                        rehomed.insert(stale.id.clone());
                        continue;
                    }
                }
                // Branch switched in place (checkout on the root or inside a
                // linked worktree): refresh the stored name so the row tracks
                // reality instead of the branch at adoption time.
                let root = is_root(&entry.path);
                if known.branch != entry.branch || known.is_main != root {
                    if known.branch != entry.branch {
                        self.store
                            .update_worktree_branch(&known.id, &entry.branch)?;
                    }
                    if known.is_main != root {
                        self.store.set_worktree_main(&known.id, root)?;
                    }
                    let mut updated = (*known).clone();
                    updated.branch = entry.branch.clone();
                    updated.is_main = root;
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Worktree(updated),
                    });
                }
                continue;
            }
            // A checkout git lists where no row stands, on the branch of a
            // row whose checkout git no longer lists: the same checkout,
            // moved (`git worktree move`, or another orion relocating it).
            // The row follows it, keeping its id and so its sessions.
            if !is_root(&entry.path) {
                if let Some(stale) = moved_from(&ours, &entries, &entry.branch, &rehomed) {
                    self.rehome_moved_row(stale, entry)?;
                    rehomed.insert(stale.id.clone());
                    continue;
                }
            }
            let worktree = Worktree {
                id: WorktreeId::generate(),
                project_id: project.id.clone(),
                is_main: is_root(&entry.path),
                path: entry.path.clone(),
                branch: entry.branch.clone(),
                sort_order: 0,
            };
            self.store.insert_worktree(&worktree)?;
            adopted = true;
            if let (false, Some(main)) = (worktree.is_main, main_path.as_deref()) {
                self.link_env_files_from(main, &worktree.path).await;
            }
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Worktree(worktree),
            });
        }
        for w in ours {
            // The main checkout is always somewhere in git's list, so a row
            // that isn't there is a linked checkout that went away — including
            // one still carrying an `is_main` from before root-ness was
            // derived, which no longer earns the row a reprieve.
            if entries.iter().any(|e| e.path == w.path) || rehomed.contains(&w.id) {
                continue;
            }
            if occupied(&w.id) {
                continue;
            }
            self.store.delete_worktree(&w.id)?;
            self.broadcast(ServerEvent::EntityRemoved {
                id: EntityId::Worktree(w.id.clone()),
            });
        }
        Ok(adopted)
    }

    /// Row `stale` stands on `entry`'s path from here on: its checkout moved
    /// there ([`moved_from`]).
    fn rehome_moved_row(&self, stale: &Worktree, entry: &git::WorktreeEntry) -> Result<()> {
        tracing::info!(
            worktree = %stale.id,
            from = %stale.path.display(),
            to = %entry.path.display(),
            "worktree row follows its moved checkout"
        );
        self.store.update_worktree_path(&stale.id, &entry.path)?;
        let mut moved = stale.clone();
        moved.path = entry.path.clone();
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Worktree(moved),
        });
        Ok(())
    }

    // ---- agents ----

    pub(crate) async fn create_agent(self: &Arc<Self>, spec: CreateAgentSpec) -> Result<EntityId> {
        let CreateAgentSpec {
            worktree: worktree_id,
            name,
            kind,
            custom_harness,
            model,
            effort,
            auto_title,
            starting_prompt,
            pr_url,
            mode,
            automation,
        } = spec;
        let starting_prompt = match starting_prompt {
            Some(prompt) => Some(validate_starting_prompt(&prompt)?),
            None => None,
        };
        let pr_url = match pr_url {
            Some(url) => Some(crate::pr_scope::validate_pr_url(&url)?),
            None => None,
        };
        // Every harness resolves against the current registry before
        // anything spawns: a missing or broken entry refuses the create
        // with its reason, and a deleted entry breaks respawns the same
        // way at boot.
        let harness = resolve_harness(kind, custom_harness.as_deref())?;
        // The row is stored as what its harness is — a Claude account as
        // Claude with the account's id — whatever the request called it.
        let (kind, custom_harness) = harness.session_kind();
        let program = harness.program.trim().to_string();
        // A launch that hands the CLI a first prompt is working from the
        // moment it spawns, so the row says so now instead of staying gray
        // until the CLI has booted and its first hook has landed — seconds
        // in which the session the user just started looked idle and sorted
        // under every session mid-turn.
        let optimistic_run = Self::launch_submits_first_prompt(
            &harness,
            starting_prompt.as_deref(),
            pr_url.is_some(),
        );
        let worktree = self
            .store
            .get_worktree(&worktree_id)?
            .context("worktree not found")?;
        self.spares.note_used(&worktree.project_id);
        // A SPARE WORKTREE a launch just took holds the CLI it booted for
        // this very launch: the row takes that CLI's id, and the task goes
        // in as its first turn. A PR rule rides a CLI's argv, so a launch
        // with one never adopts.
        let adopted = pr_url
            .is_none()
            .then(|| {
                let launch = orion_core::LaunchSpec {
                    kind,
                    custom_harness: custom_harness.clone(),
                    model: model.clone(),
                    effort: effort.clone(),
                    mode,
                };
                self.take_held_cli(&worktree_id, &launch)
            })
            .flatten();
        // Only the cold path needs asking: an adopted CLI is proof it runs.
        // Without this, a missing CLI still "succeeds" — the login shell
        // prints `command not found` into a PTY that dies at once, leaving
        // a dead row that looks identical to a fresh one.
        if adopted.is_none() && !self.cli_available_for_create(&program).await {
            bail!("{}", cli_missing_message(&program));
        }
        let agent = Agent {
            id: adopted
                .as_ref()
                .map_or_else(AgentId::generate, |cli| cli.agent.clone()),
            worktree_id,
            name: if name.trim().is_empty() {
                "agent".into()
            } else {
                name.trim().to_string()
            },
            status: if optimistic_run {
                AgentStatus::Running
            } else {
                AgentStatus::Fresh
            },
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            custom_harness,
            model,
            effort,
            session_id: None,
            sort_order: 0,
            status_changed_at: orion_core::clock::now_ms(),
            alive: false,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation,
        };
        if let Err(e) =
            self.store
                .insert_agent_with_launch_context(&agent, auto_title, pr_url.as_deref())
        {
            // An adopted CLI with no row would run on, unseen, to the end.
            if adopted.is_some() {
                self.spares.forget_boot(&agent.id);
                self.kill_session(&SessionRef::Agent(agent.id.clone()));
            }
            return Err(e);
        }
        if optimistic_run {
            // Seeded by hand, ahead of the spawn, so the CLI's own startup
            // progress-clear cannot green the row out before its turn has
            // begun — and so nothing seeds a plain `running` machine from
            // the row first.
            self.status_machines
                .lock()
                .unwrap()
                .insert(agent.id.clone(), AgentStatusMachine::launching());
        }
        if adopted.is_none() {
            let spawned = self.spawn_agent_session_with(
                &agent,
                &worktree,
                DEFAULT_COLS,
                DEFAULT_ROWS,
                starting_prompt.as_deref(),
                mode,
            );
            self.rollback_agent_on_spawn_error(&agent.id, spawned)?;
        }
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        if let Some(cli) = adopted {
            self.send_first_turn(agent.id.clone(), cli.boot, starting_prompt);
        }
        Ok(EntityId::Agent(agent.id))
    }

    /// Does a cold spawn of this launch hand the CLI a first prompt — the
    /// turn that makes a created row `running` before a single hook has
    /// fired? Asked of the two places that decide it, so it cannot drift
    /// from them: [`crate::pr_scope::launch_prompts`], which folds a launch
    /// rule (a PR SESSION's) into the first prompt on a
    /// harness with no system-prompt flag, and the argv builder's prepend
    /// shape, which puts orion's guidance there
    /// ([`agent_spawn_command_with`]). The rule's text is built per spawn
    /// from the checkout; only whether there *is* one matters here, so a
    /// stand-in stands in for it.
    fn launch_submits_first_prompt(
        harness: &orion_core::harness::HarnessDescriptor,
        starting_prompt: Option<&str>,
        rule: bool,
    ) -> bool {
        harness.system.prepend_to_first_prompt
            || crate::pr_scope::launch_prompts(
                harness.system.append_flag.is_some(),
                // A row being created has no session to resume.
                false,
                rule.then_some("<rule>"),
                starting_prompt,
            )
            .initial
            .is_some()
    }

    fn rollback_agent_on_spawn_error<T>(&self, id: &AgentId, result: Result<T>) -> Result<T> {
        match result {
            Ok(value) => Ok(value),
            Err(spawn_error) => {
                self.status_machines.lock().unwrap().remove(id);
                if let Err(rollback_error) = self.store.delete_agent(id) {
                    return Err(spawn_error.context(format!(
                        "agent spawn failed and its database rollback also failed: {rollback_error:#}"
                    )));
                }
                Err(spawn_error)
            }
        }
    }

    /// Kill every live agent and terminal PTY homed in these worktrees —
    /// the prelude to dropping their rows (worktree delete, project
    /// remove).
    fn kill_sessions_in(
        &self,
        worktree_ids: &[WorktreeId],
        agents: &[Agent],
        terminals: &[TerminalTab],
    ) {
        for a in agents
            .iter()
            .filter(|a| worktree_ids.contains(&a.worktree_id))
        {
            self.kill_session(&SessionRef::Agent(a.id.clone()));
        }
        for t in terminals
            .iter()
            .filter(|t| worktree_ids.contains(&t.worktree_id))
        {
            self.kill_session(&SessionRef::Terminal(t.id.clone()));
            self.forget_command(&t.id);
        }
        self.drop_held_in(worktree_ids);
    }

    /// Is the harness's CLI on the user's PATH (as their login shell sees
    /// it)? Cached by program: hits for an hour, misses for a minute so a
    /// just-installed CLI gets picked up quickly. Probe trouble (timeout,
    /// spawn error) fails open — the spawn itself then gets to report.
    /// Custom harnesses pass their entry's program; built-ins their CLI.
    pub(crate) async fn cli_available(&self, program: &str) -> bool {
        if std::env::var(env::AGENT_CMD).is_ok() {
            return true; // test override is spawned verbatim
        }
        const OK_TTL: Duration = Duration::from_secs(3600);
        const FAIL_TTL: Duration = Duration::from_secs(60);
        {
            let probes = self.cli_probes.lock().unwrap();
            if let Some((ok, at)) = probes.get(program) {
                if at.elapsed() < if *ok { OK_TTL } else { FAIL_TTL } {
                    return *ok;
                }
            }
        }
        self.probe_cli(program).await
    }

    /// Fill the availability cache for every harness at boot, off the
    /// request loop. Without it the first CreateAgent of a session pays a
    /// full login-shell probe (~1s with a heavy ~/.zshrc) before it can
    /// answer. Custom entries resolve against the current config; a bare
    /// `Custom` kind never reaches the probe — it has no program.
    pub async fn warm_cli_probes(self: &Arc<Self>) {
        // One probe per launchable program in the effective registry, so
        // a repointed program warms the binary that actually launches.
        let mut programs = Vec::new();
        for entry in harness_registry() {
            if entry.problem().is_none() && !programs.contains(&entry.program) {
                programs.push(entry.program.clone());
            }
        }
        for program in programs {
            self.cli_available(program.trim()).await;
        }
    }

    /// Same question, asked on behalf of a create the user just triggered.
    /// A cached *hit* is trusted; a cached *miss* is re-probed, so someone who
    /// installs the CLI and immediately retries isn't told for another minute
    /// that it's missing. Misses are rare, so this costs nothing in practice.
    async fn cli_available_for_create(&self, program: &str) -> bool {
        self.cli_available(program).await || self.probe_cli(program).await
    }

    /// Uncached `command -v` through the user's login shell; caches the answer.
    async fn probe_cli(&self, program: &str) -> bool {
        let check = cli_probe_line(program);
        let mut probe = tokio::process::Command::new(orion_core::shell::user_shell());
        probe
            .args(LOGIN_SHELL_ARGS)
            .arg(&check)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // A timed-out probe must die with the dropped future, not linger.
            .kill_on_drop(true);
        // Own session: the interactive shell must not reach the daemon's
        // controlling terminal (--foreground runs have one). zsh's job-control
        // init opens /dev/tty and makes itself the foreground process group,
        // SIGTTIN-stopping whatever TUI owns that terminal.
        unsafe {
            probe.pre_exec(own_session);
        }
        let status = tokio::time::timeout(CLI_PROBE_TIMEOUT, probe.status()).await;
        match status {
            Ok(Ok(status)) => {
                let ok = status.success();
                self.cli_probes
                    .lock()
                    .unwrap()
                    .insert(program.to_string(), (ok, Instant::now()));
                ok
            }
            _ => true,
        }
    }

    pub fn rename_agent(self: &Arc<Self>, id: &AgentId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("name is empty");
        }
        self.store.rename_agent(id, name.trim())?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// Agent-initiated one-shot title (`orion rename` inside the session's
    /// CLI). Applies only while the auto-title is still pending; afterwards
    /// it reports the standing title as an error so the CLI (and the model
    /// reading its output) knows nothing changed.
    pub fn auto_rename_agent(self: &Arc<Self>, id: &AgentId, name: &str) -> Result<()> {
        let title = sanitize_title(name);
        if title.is_empty() {
            bail!("title is empty");
        }
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if !self.store.rename_agent_if_auto_pending(id, &title)? {
            bail!(
                "session already has a title ({:?}); leaving it unchanged — a user-set \
                 title is only replaced with `orion rename --force`",
                agent.name
            );
        }
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// The project's worktree on `branch`, created from `base` when it has
    /// none — what `orion worktree` moves into and `orion spawn --worktree`
    /// starts a session in.
    pub(crate) async fn worktree_for_branch(
        self: &Arc<Self>,
        project: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<Worktree> {
        let branch = branch.trim();
        if branch.is_empty() {
            bail!("branch name is empty");
        }
        let (_, worktrees, _, _) = self.store.load_tree()?;
        if let Some(w) = worktrees
            .into_iter()
            .find(|w| &w.project_id == project && w.branch == branch)
        {
            return Ok(w);
        }
        let created = self.create_worktree(project, branch, base, false).await?;
        let EntityId::Worktree(new_id) = created else {
            bail!("worktree creation returned a non-worktree entity");
        };
        self.store
            .get_worktree(&new_id)?
            .context("worktree not found")
    }

    /// `orion worktree <branch>`, run by the agent inside its own session.
    /// The row moves under `branch`'s worktree of the same project now —
    /// created when the project has no checkout for that branch yet — and
    /// a live PTY follows once its turn ends (`complete_pending_move`),
    /// because the CLI running this command *is* that PTY's foreground
    /// tool call: killing it here would cut the turn off mid-answer.
    pub async fn enter_worktree(
        self: &Arc<Self>,
        id: &AgentId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<(Worktree, EnterOutcome)> {
        let branch = branch.trim();
        if branch.is_empty() {
            bail!("branch name is empty");
        }
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived");
        }
        let current = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        let target = self
            .worktree_for_branch(&current.project_id, branch, base)
            .await?;
        if target.id == current.id {
            return Ok((target, EnterOutcome::AlreadyThere));
        }
        let alive = self.session(&SessionRef::Agent(id.clone())).is_some();
        // Same invalidation as `relocate_into`: every cwd this process
        // reports until it respawns is the old checkout's.
        self.last_cwd.lock().unwrap().remove(id);
        if alive {
            self.pending_moves
                .lock()
                .unwrap()
                .insert(id.clone(), target.clone());
        }
        self.store.set_agent_worktree(id, &target.id)?;
        self.broadcast_agent(id)?;
        let outcome = if alive {
            EnterOutcome::Relocating
        } else {
            EnterOutcome::NextLaunch
        };
        Ok((target, outcome))
    }

    /// The turn an agent ran `orion worktree` in has ended: make the
    /// process match its row. Kill it and respawn it resumed in the target,
    /// with a prompt naming the checkout it now runs in so the conversation
    /// carries straight on (Claude and codex take that prompt as an
    /// argument; cursor resumes silent and waits for the user — see
    /// `relocation_prompt`). Gated on the turn-end signals — Stop, the idle
    /// notification a Stop-less end still fires, and the progress clear
    /// that is the only word of a cancelled turn — so a Bash hook from the
    /// same turn never triggers it.
    ///
    /// The status machine held that turn end at `running` for the
    /// relocation (`AgentStatusMachine::set_relocating`), so the card never
    /// drops to the bottom of the grid for the seconds until the respawned
    /// CLI's first hook: a respawn that opens on the notice is seeded as a
    /// launch, the way a create with a task is, and any other outcome — a
    /// silent respawn, a failed one, nothing left to respawn — lets the
    /// held end finish the turn after all.
    pub fn complete_pending_move(self: &Arc<Self>, id: &AgentId, event: &HookEvent) {
        let turn_over = match event {
            // A turn stopped on a usage limit is over too: the respawn
            // meets the limit again, and says so, from the target.
            HookEvent::Stop | HookEvent::UsageLimit { .. } => true,
            HookEvent::Notification { notification_type } => {
                notification_type.as_deref() == Some("idle_prompt")
            }
            HookEvent::Progress { busy } => !busy,
            _ => false,
        };
        if !turn_over {
            return;
        }
        let Some(target) = self.pending_moves.lock().unwrap().remove(id) else {
            return;
        };
        if self.relocate_into(id, &target) {
            // Working from the moment it boots, on the notice: seeded with
            // the launch reprieve so its startup progress-clear cannot
            // green it out before that turn begins (see `create_agent`).
            self.status_machines
                .lock()
                .unwrap()
                .insert(id.clone(), AgentStatusMachine::launching());
        } else {
            self.release_relocation_hold(id);
        }
    }

    /// The kill-and-respawn of [`Self::complete_pending_move`]. True when
    /// the respawn opened on the relocation notice — the one outcome that
    /// carries on the turn the status machine held.
    fn relocate_into(self: &Arc<Self>, id: &AgentId, target: &Worktree) -> bool {
        let agent = match self.store.get_agent(id) {
            Ok(Some(agent)) if !agent.archived && agent.worktree_id == target.id => agent,
            // Archived, deleted, or moved elsewhere by hand since: the
            // row's current home wins, nothing to relocate into.
            _ => return false,
        };
        let sref = SessionRef::Agent(id.clone());
        if self.session(&sref).is_none() {
            // Died since (or the user closed it): the next launch boots in
            // the target on its own, only without the relocation notice.
            return false;
        }
        tracing::info!(agent = %id, to = %target.branch, "relocating session into its worktree");
        self.kill_session(&sref);
        self.last_cwd.lock().unwrap().remove(id);
        // A row whose entry went missing since still relocates; the boot
        // itself refuses with the entry's reason, so the notice degrades
        // to none rather than failing the move.
        let prompt = resolve_harness(agent.kind, agent.custom_harness.as_deref())
            .map(|harness| relocation_prompt(harness.relocation_prompt, target))
            .unwrap_or(None);
        let spawned = self.spawn_agent_session_with(
            &agent,
            target,
            DEFAULT_COLS,
            DEFAULT_ROWS,
            prompt.as_deref(),
            orion_core::harness::AgentMode::Edit,
        );
        let continued = match spawned {
            Ok(_) => prompt.is_some(),
            Err(e) => {
                tracing::warn!(agent = %id, error = %e, "respawn after worktree relocation failed");
                false
            }
        };
        self.try_broadcast_agent(id);
        continued
    }

    /// No respawn is carrying the held turn on: the end the status machine
    /// held for the relocation lands now, as the Stop it was. A machine not
    /// mid-turn held nothing — the row finished or died the ordinary way —
    /// and is left alone.
    fn release_relocation_hold(&self, id: &AgentId) {
        let effects = {
            let mut machines = self.status_machines.lock().unwrap();
            let Some(machine) = machines.get_mut(id) else {
                return;
            };
            if !matches!(
                machine.status(),
                AgentStatus::Running | AgentStatus::NeedsFeedback
            ) {
                return;
            }
            machine.set_relocating(false);
            machine.handle(HookEvent::Stop, None, Instant::now())
        };
        self.apply_status_effects(id, effects);
    }

    /// Whether `id` is between `enter_worktree` and its respawn.
    #[cfg(test)]
    fn relocation_pending(&self, id: &AgentId) -> bool {
        self.pending_moves.lock().unwrap().contains_key(id)
    }

    /// Row-only re-home: store update plus broadcast, never the PTY. The
    /// hook-cwd reparent uses this — there the process already runs in the
    /// target checkout and only the row is stale, so killing it would
    /// interrupt a live conversation for nothing.
    fn move_agent_row(self: &Arc<Self>, id: &AgentId, worktree_id: &WorktreeId) -> Result<()> {
        self.store.set_agent_worktree(id, worktree_id)?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// A hook payload reported the agent CLI's working directory. When that
    /// directory sits inside a *different* worktree of the same project (the
    /// session entered a worktree it created mid-conversation), re-home the
    /// agent row so the tree reflects where the work actually happens.
    /// Fail-soft: any error leaves the row where it is.
    pub fn reparent_agent_by_cwd(
        self: &Arc<Self>,
        agent_id: &AgentId,
        cwd: &str,
        payload_session_id: Option<&str>,
        captures_session: bool,
    ) {
        if let Err(e) =
            self.try_reparent_agent_by_cwd(agent_id, cwd, payload_session_id, captures_session)
        {
            tracing::warn!(agent = %agent_id, error = %e, "cwd reparent failed");
        }
    }

    fn try_reparent_agent_by_cwd(
        self: &Arc<Self>,
        agent_id: &AgentId,
        cwd: &str,
        payload_session_id: Option<&str>,
        captures_session: bool,
    ) -> Result<()> {
        let Some(agent) = self.store.get_agent(agent_id)? else {
            self.last_cwd.lock().unwrap().remove(agent_id);
            return Ok(());
        };
        if agent.archived {
            self.last_cwd.lock().unwrap().remove(agent_id);
            return Ok(());
        }
        // Mid-relocation the row already sits under the target while the
        // process still reports the old checkout — ignore it until the
        // respawn lands there.
        if self.pending_moves.lock().unwrap().contains_key(agent_id) {
            return Ok(());
        }
        // Same foreign-session rule as the status machine: a payload from a
        // different CLI session only counts when the event (re)establishes
        // session ownership (UserPromptSubmit / SessionStart).
        if !captures_session {
            if let (Some(mine), Some(theirs)) = (agent.session_id.as_deref(), payload_session_id) {
                if mine != theirs {
                    return Ok(());
                }
            }
        }
        let cwd = canonical_or_raw(Path::new(cwd));
        // Remembered even when it resolves to nothing: an agent that just ran
        // `git worktree add` and stepped into the result reports a cwd orion
        // has no row for yet, and the worktree sync replays this to finish the
        // re-home the moment that row is adopted.
        self.last_cwd
            .lock()
            .unwrap()
            .insert(agent_id.clone(), cwd.clone());
        self.reparent_agent_to_cwd(&agent, &cwd)
    }

    /// Move `agent`'s row under the worktree owning `cwd` when that is a
    /// different worktree of the same project. `cwd` must already be
    /// canonicalized.
    fn reparent_agent_to_cwd(self: &Arc<Self>, agent: &Agent, cwd: &Path) -> Result<()> {
        let Some(current) = self.store.get_worktree(&agent.worktree_id)? else {
            return Ok(());
        };
        let (_, worktrees, _, _) = self.store.load_tree()?;
        // Deepest worktree of the same project containing cwd — nested
        // layouts (checkouts under the repo root) must not resolve to the
        // root row just because the root path is also a prefix.
        let target = worktrees
            .into_iter()
            .filter(|w| w.project_id == current.project_id)
            .map(|w| {
                let canonical = canonical_or_raw(&w.path);
                (w, canonical)
            })
            .filter(|(_, canonical)| cwd.starts_with(canonical))
            .max_by_key(|(_, canonical)| canonical.components().count());
        if let Some((worktree, _)) = target {
            if worktree.id != agent.worktree_id {
                tracing::info!(
                    agent = %agent.id,
                    from = %current.branch,
                    to = %worktree.branch,
                    "agent re-homed by hook cwd"
                );
                self.move_agent_row(&agent.id, &worktree.id)?;
            }
        }
        Ok(())
    }

    /// Replay remembered hook cwds for `project`'s agents. Runs after the
    /// worktree sync adopts checkouts: a session that creates a worktree and
    /// enters it reports the new cwd (often on the very next `Stop`) before
    /// the row exists, and without this replay its row would sit under the
    /// old checkout until the user's next prompt.
    fn reparent_agents_by_last_cwd(self: &Arc<Self>, project: &Project) {
        let known: Vec<(AgentId, PathBuf)> = {
            let map = self.last_cwd.lock().unwrap();
            map.iter().map(|(id, p)| (id.clone(), p.clone())).collect()
        };
        for (agent_id, cwd) in known {
            let agent = match self.store.get_agent(&agent_id) {
                Ok(Some(agent)) => agent,
                Ok(None) => {
                    self.last_cwd.lock().unwrap().remove(&agent_id);
                    continue;
                }
                Err(e) => {
                    tracing::warn!(agent = %agent_id, error = %e, "cwd replay lookup failed");
                    continue;
                }
            };
            if agent.archived {
                continue;
            }
            let in_project = matches!(
                self.store.get_worktree(&agent.worktree_id),
                Ok(Some(w)) if w.project_id == project.id
            );
            if !in_project {
                continue;
            }
            if let Err(e) = self.reparent_agent_to_cwd(&agent, &cwd) {
                tracing::warn!(agent = %agent_id, error = %e, "cwd replay reparent failed");
            }
        }
    }

    pub fn archive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.store.set_agent_archived(id, true)?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    pub fn unarchive_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.store.set_agent_archived(id, false)?;
        self.broadcast_agent(id)?;
        Ok(())
    }

    /// A client put this agent's session on screen: its unseen-finish flag
    /// (`Agent::unseen`) is cleared, and every subscriber gets the row so
    /// their counts drop together. Nothing is sent when the flag was
    /// already clear — re-attaching to a session you've read is free.
    pub fn mark_agent_seen(&self, id: &AgentId) -> Result<()> {
        if self.store.mark_agent_seen(id)? {
            self.broadcast_agent(id)?;
        }
        Ok(())
    }

    pub fn delete_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.last_cwd.lock().unwrap().remove(id);
        self.pending_moves.lock().unwrap().remove(id);
        self.store.delete_agent(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Agent(id.clone()),
        });
        Ok(())
    }

    pub async fn restart_agent(self: &Arc<Self>, id: &AgentId) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived — unarchive it first");
        }
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        self.kill_session(&SessionRef::Agent(id.clone()));
        self.spawn_agent_session(&agent, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        let mut broadcast_agent = agent.clone();
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        Ok(())
    }

    // ---- terminals ----

    pub fn create_terminal(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        name: Option<String>,
    ) -> Result<EntityId> {
        let worktree = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            let n = self.store.count_terminals(worktree_id).unwrap_or(0);
            format!("term-{}", n + 1)
        });
        let terminal = TerminalTab {
            id: TerminalId::generate(),
            worktree_id: worktree_id.clone(),
            name,
            sort_order: 0,
            alive: false,
            run_command: None,
            setup: false,
            run: Default::default(),
        };
        self.store.insert_terminal(&terminal)?;
        self.spawn_terminal_session(&terminal, &worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        let mut broadcast_term = terminal.clone();
        broadcast_term.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Terminal(broadcast_term),
        });
        Ok(EntityId::Terminal(terminal.id))
    }

    pub fn rename_terminal(self: &Arc<Self>, id: &TerminalId, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("name is empty");
        }
        self.store.rename_terminal(id, name.trim())?;
        self.broadcast_terminal(id)
    }

    pub fn close_terminal(self: &Arc<Self>, id: &TerminalId) -> Result<()> {
        self.kill_session(&SessionRef::Terminal(id.clone()));
        self.forget_command(id);
        self.store.delete_terminal(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Terminal(id.clone()),
        });
        Ok(())
    }

    /// Drop what the DAEMON holds about terminal `id`'s last command — an
    /// ended run's PTY, a stop under way, a start waiting on it — as its
    /// row goes, or its command starts again.
    fn forget_command(&self, id: &TerminalId) {
        self.finished_runs.lock().unwrap().remove(id);
        self.stopping_runs.lock().unwrap().remove(id);
        self.queued_runs.lock().unwrap().remove(id);
    }

    // ---- setup terminals ----

    /// A worktree orion just created: [`Daemon::start_setup`], with
    /// whatever stops it starting a warning, never a failed create.
    fn setup_worktree(
        self: &Arc<Self>,
        repo: &Path,
        worktree: &Worktree,
        config: &crate::config::Config,
    ) {
        if let Err(e) = self.start_setup(repo, worktree, config) {
            self.warn_clients(format!(
                "setup didn't start in {}: {e:#}",
                worktree.path.display()
            ));
        }
    }

    /// Run the SETUP COMMAND of the project cloned at `repo` (Settings →
    /// Projects, read from `config`) in `worktree`, in a SETUP TERMINAL in
    /// its band: through the login shell, as a run is, for as long as it
    /// takes. Its end is [`Daemon::setup_ended`]'s. The row's id, or None
    /// when the project has no setup command.
    pub(crate) fn start_setup(
        self: &Arc<Self>,
        repo: &Path,
        worktree: &Worktree,
        config: &crate::config::Config,
    ) -> Result<Option<TerminalId>> {
        let Some(command) = config.setup_command(repo) else {
            return Ok(None);
        };
        let term = setup_terminal_row(worktree, command);
        let id = term.id.clone();
        // Held across the insert and the spawn, as a run's start holds it:
        // an attach in between must find the PTY, not a row without one.
        let _gate = self.spawn_gate.lock().unwrap();
        self.store.insert_terminal(&term)?;
        let spawned = self.spawn_command(term, worktree);
        tracing::info!(worktree = %worktree.id, command, ok = spawned.is_ok(), "setup started");
        if let Err(e) = spawned {
            // Nothing ran, so there is nothing for the row to show: the
            // warning says why.
            self.close_terminal(&id)?;
            return Err(e);
        }
        Ok(Some(id))
    }

    /// SETUP TERMINAL `setup`'s PTY, `session`, exited with `exit_code`. A
    /// clean exit takes the row with it and starts the run a **Start
    /// stack** left waiting on it; any other keeps the row, as `setup
    /// failed (1)`, its PTY kept for the attach that wants to read why, and
    /// starts nothing — the warning says so, and a press now starts it.
    fn setup_ended(
        self: &Arc<Self>,
        setup: &TerminalTab,
        session: &Arc<PtySession>,
        exit_code: Option<i32>,
    ) {
        let id = &setup.id;
        let queued = self.queued_runs.lock().unwrap().remove(id);
        if exit_code != Some(0) {
            self.keep_finished(id, session, exit_code);
            if queued.is_some() {
                let branch = self.branch_of(&setup.worktree_id);
                self.warn_clients(format!(
                    "setup failed on {branch}, so its stack didn't start — Start stack there starts it anyway"
                ));
            }
            return;
        }
        if let Err(e) = self.close_terminal(id) {
            tracing::warn!(terminal = %id.0, error = %e, "finished setup's row not removed");
        }
        if let Some(config) = queued {
            tracing::info!(worktree = %setup.worktree_id, "setup finished; starting its run");
            if let Err(e) = self.start_run_with(&setup.worktree_id, &config) {
                let branch = self.branch_of(&setup.worktree_id);
                self.warn_clients(format!(
                    "setup finished on {branch}, but its stack didn't start: {e:#}"
                ));
            }
        }
    }

    /// Worktree `id`'s branch, for a warning that names it.
    fn branch_of(&self, id: &WorktreeId) -> String {
        self.store
            .get_worktree(id)
            .ok()
            .flatten()
            .map_or_else(|| "its worktree".into(), |w| w.branch)
    }

    /// A **Start stack** on a worktree still setting up: queued on its
    /// running SETUP TERMINAL under `config` — the config of the press, as
    /// a restart keeps its own — to start once that exits cleanly
    /// ([`Daemon::setup_ended`]). The first press sends the row again, so
    /// every client's footer says it is waiting. The setup's id, or None
    /// when the worktree has no setup running.
    fn queue_run_behind_setup(
        &self,
        worktree_id: &WorktreeId,
        config: &crate::config::Config,
    ) -> Result<Option<TerminalId>> {
        let setups = self.store.setup_terminals_in(worktree_id)?;
        let (id, first) = {
            // Checked under the queue's lock: a setup's exit leaves
            // `sessions` before it takes its queue, so one already ending
            // reads as gone here and nothing waits on it.
            let mut queue = self.queued_runs.lock().unwrap();
            let Some(setup) = setups
                .into_iter()
                .find(|s| self.is_alive(&SessionRef::Terminal(s.id.clone())))
            else {
                return Ok(None);
            };
            let first = queue.insert(setup.id.clone(), config.clone()).is_none();
            (setup.id, first)
        };
        if first {
            // A row gone meanwhile took its queue with it; nothing to say.
            let _ = self.broadcast_terminal(&id);
        }
        Ok(Some(id))
    }

    /// A stop on `worktree_id` while a start waits on its setup: the
    /// start is called off, and the setup's row sent again to say so.
    /// Whether there was one to call off.
    fn cancel_queued_run(&self, worktree_id: &WorktreeId) -> Result<bool> {
        let mut cancelled = false;
        for setup in self.store.setup_terminals_in(worktree_id)? {
            if self.queued_runs.lock().unwrap().remove(&setup.id).is_some() {
                cancelled = true;
                let _ = self.broadcast_terminal(&setup.id);
            }
        }
        Ok(cancelled)
    }

    /// Spawn command terminal `term`'s PTY — a run's or a setup's — in
    /// `worktree`, and send its row to every client as up. Sent under the
    /// sessions lock, and only while the PTY is still there: one that has
    /// already exited was dropped from the map by its exit, whose own word
    /// on the row — the row gone, or how it ended — must come last, never
    /// followed by a stale "up". A spawn that failed sends the row as it
    /// is, not alive.
    fn spawn_command(self: &Arc<Self>, mut term: TerminalTab, worktree: &Worktree) -> Result<()> {
        let spawned = self.spawn_terminal_session(&term, worktree, DEFAULT_COLS, DEFAULT_ROWS);
        let sessions = self.sessions.lock().unwrap();
        term.alive = sessions.contains_key(&SessionRef::Terminal(term.id.clone()));
        if term.alive || spawned.is_err() {
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Terminal(term),
            });
        }
        spawned.map(drop)
    }

    /// Keep command terminal `id`'s ended PTY, `session`, and the code it
    /// exited with, for the attach that replays how it ended and the row
    /// that says it.
    fn keep_finished(&self, id: &TerminalId, session: &Arc<PtySession>, exit_code: Option<i32>) {
        self.finished_runs
            .lock()
            .unwrap()
            .insert(id.clone(), (session.clone(), exit_code));
    }

    // ---- run terminals ----

    /// `⌘⇧S` on a worktree: start its RUN COMMAND — the project's
    /// `run_command` setting (Settings → Projects), read fresh — in the
    /// worktree's RUN TERMINAL; with none, the refusal says so. A run that already
    /// exited lends its row; one still going is the answer as it stands,
    /// so a second client's press never starts a second server. A worktree
    /// still running its setup starts nothing yet: the start waits for the
    /// setup ([`Daemon::queue_run_behind_setup`]), and the answer is its row.
    pub fn start_run(self: &Arc<Self>, worktree_id: &WorktreeId) -> Result<EntityId> {
        self.start_run_with(worktree_id, &crate::config::Config::load())
    }

    /// [`Daemon::start_run`] against a given config, for the tests that
    /// can't pin the settings file.
    pub fn start_run_with(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        config: &crate::config::Config,
    ) -> Result<EntityId> {
        let worktree = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        // Held across the check and the spawn, like `ensure_session`: two
        // presses racing must produce one run, not two.
        let _gate = self.spawn_gate.lock().unwrap();
        let existing = self.store.run_terminals_in(worktree_id)?.into_iter().next();
        if let Some(term) = &existing {
            if self.is_alive(&SessionRef::Terminal(term.id.clone())) {
                return Ok(EntityId::Terminal(term.id.clone()));
            }
        }
        let command = self.run_command_in(&worktree, config)?;
        if let Some(setup) = self.queue_run_behind_setup(worktree_id, config)? {
            return Ok(EntityId::Terminal(setup));
        }
        let term = match existing {
            Some(mut term) => {
                self.store.set_terminal_run_command(&term.id, &command)?;
                term.run_command = Some(command.clone());
                term
            }
            None => {
                let term = TerminalTab {
                    id: TerminalId::generate(),
                    worktree_id: worktree_id.clone(),
                    name: RUN_TERMINAL_NAME.into(),
                    sort_order: 0,
                    alive: false,
                    run_command: Some(command.clone()),
                    setup: false,
                    run: Default::default(),
                };
                self.store.insert_terminal(&term)?;
                term
            }
        };
        self.forget_command(&term.id);
        let id = term.id.clone();
        let spawned = self.spawn_command(term, &worktree);
        tracing::info!(worktree = %worktree_id, %command, ok = spawned.is_ok(), "run started");
        spawned?;
        Ok(EntityId::Terminal(id))
    }

    /// `worktree`'s RUN COMMAND under `config`: its project's setting, or
    /// the refusal that says where to set one.
    fn run_command_in(
        &self,
        worktree: &Worktree,
        config: &crate::config::Config,
    ) -> Result<String> {
        let main = self
            .store
            .get_project(&worktree.project_id)?
            .map_or_else(|| worktree.path.clone(), |p| p.repo_path);
        match config.run_command(&main) {
            Some(command) => Ok(command.to_string()),
            None => bail!(NO_RUN_COMMAND),
        }
    }

    /// `RestartRun`: the worktree's run stopped as [`Daemon::stop_run`]
    /// stops it, then started again in its row once it has gone — the
    /// command read fresh now. A run that isn't up just starts; a second
    /// restart, like a second stop, kills it at once and starts it.
    pub fn restart_run(self: &Arc<Self>, worktree_id: &WorktreeId) -> Result<()> {
        self.restart_run_with(worktree_id, &crate::config::Config::load())
    }

    /// [`Daemon::restart_run`] against a given config, for the tests.
    pub fn restart_run_with(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        config: &crate::config::Config,
    ) -> Result<()> {
        let worktree = self
            .store
            .get_worktree(worktree_id)?
            .context("worktree not found")?;
        // Nothing to start again: said before anything is stopped.
        self.run_command_in(&worktree, config)?;
        if self.store.run_terminals_in(worktree_id)?.is_empty() {
            return self.start_run_with(worktree_id, config).map(drop);
        }
        self.stop_run_then(worktree_id, Some(config.clone()))
    }

    /// Stop a worktree's run: a `^C` into its RUN TERMINAL, as a person at
    /// the terminal would, so the run's own trap tears down what it
    /// started (a compose stack, a dev server), and ends as a run that
    /// exited on its own. Meanwhile its row says it is stopping, counting
    /// down to the kill ([`RunState::stopping_until`]): one that ignores
    /// the `^C` is killed after [`RUN_STOP_GRACE`]; a second stop kills it
    /// at once. A run that already ended is dropped, row and all. Nothing
    /// to stop is not an error — two clients may both have pressed it. A
    /// start still waiting on the worktree's setup is called off instead.
    ///
    /// [`RunState::stopping_until`]: orion_core::RunState::stopping_until
    pub fn stop_run(self: &Arc<Self>, worktree_id: &WorktreeId) -> Result<()> {
        if self.cancel_queued_run(worktree_id)? {
            return Ok(());
        }
        self.stop_run_then(worktree_id, None)
    }

    /// [`Daemon::stop_run`], with what follows the run's end: its row
    /// dropped, or with `restart`, the run started again in it under that
    /// config. A press on a run already stopping hurries it — killed now —
    /// and keeps a restart either press asked for.
    fn stop_run_then(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        restart: Option<crate::config::Config>,
    ) -> Result<()> {
        let kill_at = orion_core::clock::now_ms() + RUN_STOP_GRACE.as_millis() as i64;
        for term in self.store.run_terminals_in(worktree_id)? {
            let sref = SessionRef::Terminal(term.id.clone());
            let Some(session) = self.session(&sref) else {
                // Ended already: nothing to wind down.
                self.stopping_runs.lock().unwrap().remove(&term.id);
                self.stop_done(&term.id, restart.clone())?;
                continue;
            };
            let hurried = {
                let mut stops = self.stopping_runs.lock().unwrap();
                match stops.remove(&term.id) {
                    Some(stop) => Some(restart.clone().or(stop.restart)),
                    None => {
                        let restart = restart.clone();
                        stops.insert(term.id.clone(), Stop { kill_at, restart });
                        None
                    }
                }
            };
            if let Some(then) = hurried {
                tracing::info!(worktree = %worktree_id, "run killed");
                self.kill_session(&sref);
                self.stop_done(&term.id, then)?;
                continue;
            }
            tracing::info!(worktree = %worktree_id, "run interrupted");
            // Said before the `^C` goes in: a run that dies of it at once
            // must be heard ending after it was heard stopping, never the
            // other way round.
            self.kill_run_after(term.id.clone(), session.clone(), RUN_STOP_GRACE);
            self.broadcast_terminal(&term.id)?;
            session.write_input(b"\x03")?;
        }
        Ok(())
    }

    /// RUN TERMINAL `id`'s stop is over — its run wound down, or was
    /// killed. A plain stop takes the row with it; a restart's starts the
    /// run again in the row, under the config it was asked with.
    fn stop_done(
        self: &Arc<Self>,
        id: &TerminalId,
        restart: Option<crate::config::Config>,
    ) -> Result<()> {
        let Some(config) = restart else {
            return self.close_terminal(id);
        };
        let term = self.store.get_terminal(id)?.context("terminal not found")?;
        tracing::info!(worktree = %term.worktree_id, "run restarting");
        self.start_run_with(&term.worktree_id, &config).map(drop)
    }

    /// Kill RUN TERMINAL `id` after `grace` if `session` is still its
    /// live PTY — the run never answered its `^C` — and finish its stop.
    fn kill_run_after(self: &Arc<Self>, id: TerminalId, session: Arc<PtySession>, grace: Duration) {
        let daemon = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            let sref = SessionRef::Terminal(id.clone());
            if !daemon.owns_session(&sref, &session) {
                return;
            }
            // Whoever takes the stop finishes it: the run's own end may
            // be racing this.
            let stop = daemon.stopping_runs.lock().unwrap().remove(&id);
            let Some(stop) = stop else {
                return;
            };
            tracing::warn!(terminal = %id.0, "run ignored its ^C; killing it");
            daemon.kill_session(&sref);
            if let Err(e) = daemon.stop_done(&id, stop.restart) {
                tracing::warn!(terminal = %id.0, error = %e, "killed run not finished");
            }
        });
    }

    /// RUN TERMINAL `id`'s PTY, `session`, died on its own. A stop asked
    /// for that: the row goes, as the stop said it would — or a restart's
    /// run starts in it again. Nobody asked: the row stays, its PTY kept
    /// for the attach that wants to read how it ended and its exit code
    /// for the run line (`done`, `exited 1`).
    fn run_ended(
        self: &Arc<Self>,
        id: &TerminalId,
        session: &Arc<PtySession>,
        exit_code: Option<i32>,
    ) {
        let stop = self.stopping_runs.lock().unwrap().remove(id);
        if let Some(stop) = stop {
            if let Err(e) = self.stop_done(id, stop.restart) {
                tracing::warn!(terminal = %id.0, error = %e, "stopped run not finished");
            }
            return;
        }
        self.keep_finished(id, session, exit_code);
    }

    /// Resend RUN TERMINAL `session`'s row when its run has printed ports
    /// since the row last went out with `sent` of them — and only while
    /// it is still that terminal's live PTY.
    fn send_new_ports(&self, session: &Arc<PtySession>, sent: &mut usize) {
        let SessionRef::Terminal(id) = &session.sref else {
            return;
        };
        let seen = session.ports().len();
        if seen != *sent && self.owns_session(&session.sref, session) {
            *sent = seen;
            let _ = self.broadcast_terminal(id);
        }
    }

    /// Terminal `id`'s row when it runs a command — a RUN TERMINAL's, or a
    /// SETUP TERMINAL's — rather than a shell.
    fn command_terminal(&self, id: &TerminalId) -> Option<TerminalTab> {
        self.store
            .get_terminal(id)
            .ok()
            .flatten()
            .filter(|t| t.run_command.is_some())
    }

    /// A RUN TERMINAL that exited on its own, or a SETUP TERMINAL that
    /// failed: its exit code (None when the OS reported none), for the
    /// attach replaying it. None for a live session or anything else.
    pub fn finished_run_exit(&self, sref: &SessionRef) -> Option<Option<i32>> {
        let SessionRef::Terminal(id) = sref else {
            return None;
        };
        self.finished_runs
            .lock()
            .unwrap()
            .get(id)
            .map(|(_, code)| *code)
    }

    // ---- links ----

    pub fn update_link(self: &Arc<Self>, id: &LinkId, url: &str) -> Result<()> {
        let url = normalize_url(url)?;
        self.store.set_link_url(id, &url)?;
        let link = self.store.get_link(id)?.context("link not found")?;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Link(link),
        });
        Ok(())
    }

    pub fn delete_link(self: &Arc<Self>, id: &LinkId) -> Result<()> {
        self.store.delete_link(id)?;
        self.broadcast(ServerEvent::EntityRemoved {
            id: EntityId::Link(id.clone()),
        });
        Ok(())
    }

    // ---- attach / spawn ----

    /// Get the live session for an entity, lazily (re)spawning its PTY when
    /// none is running (restored agents, closed shells).
    pub fn ensure_session(
        self: &Arc<Self>,
        sref: &SessionRef,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        if let Some(s) = self.session(sref) {
            return Ok(s);
        }
        // Hold the gate across the whole check-and-install: an Attach and the
        // prewarm sweep racing the same dead session must produce one CLI,
        // not two. Re-check under it — the winner installed while we waited.
        let _gate = self.spawn_gate.lock().unwrap();
        if let Some(s) = self.session(sref) {
            return Ok(s);
        }
        match sref {
            SessionRef::Agent(id) => self.boot_agent(id, cols, rows, None),
            SessionRef::Terminal(id) => {
                let term = self.store.get_terminal(id)?.context("terminal not found")?;
                // A RUN TERMINAL is only ever started by `r`, and a SETUP
                // TERMINAL by its worktree's create: an attach, or the
                // prewarm sweep walking past, must never run a command
                // that exited again. What it can show is how the command
                // ended, while the DAEMON still holds that PTY.
                if term.run_command.is_some() {
                    if let Some((session, _)) = self.finished_runs.lock().unwrap().get(id) {
                        return Ok(session.clone());
                    }
                    if term.setup {
                        bail!("{SETUP_NOT_RUNNING}");
                    }
                    bail!("{RUN_NOT_RUNNING}");
                }
                let worktree = self
                    .store
                    .get_worktree(&term.worktree_id)?
                    .context("worktree not found")?;
                let session = self.spawn_terminal_session(&term, &worktree, cols, rows)?;
                let mut broadcast_term = term;
                broadcast_term.alive = true;
                self.broadcast(ServerEvent::EntityUpserted {
                    entity: Entity::Terminal(broadcast_term),
                });
                Ok(session)
            }
        }
    }

    /// Start agent `id`'s CLI — resuming its stored session — with
    /// `initial_prompt` as a first turn it submits on its own. The caller
    /// holds `spawn_gate` and has seen no live session.
    fn boot_agent(
        self: &Arc<Self>,
        id: &AgentId,
        cols: u16,
        rows: u16,
        initial_prompt: Option<&str>,
    ) -> Result<Arc<PtySession>> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.archived {
            bail!("agent is archived — unarchive it first");
        }
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        let session = self.spawn_agent_session_with(
            &agent,
            &worktree,
            cols,
            rows,
            initial_prompt,
            orion_core::harness::AgentMode::Edit,
        )?;
        let mut broadcast_agent = agent;
        broadcast_agent.alive = true;
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Agent(broadcast_agent),
        });
        Ok(session)
    }

    /// `text` as agent `id`'s next turn (`ClientRequest::SendTurn`). A live
    /// CLI has it typed and submitted: the text, then the carriage return
    /// as a write of its own, so the child reads the prompt before the
    /// Enter — a BRACKETED PASTE when it has line breaks, so the CLI takes
    /// it as one block, plain bytes otherwise, which keeps a one-liner out
    /// of the "[Pasted text]" placeholder. A dead one — the IDLE REAPER's,
    /// or cold since the daemon started — boots with `text` as its first
    /// prompt, since a CLI still starting up drops what is typed at it.
    pub fn send_turn(
        self: &Arc<Self>,
        id: &AgentId,
        text: &str,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        let sref = SessionRef::Agent(id.clone());
        let live = match self.session(&sref) {
            Some(session) => Some(session),
            None => {
                let _gate = self.spawn_gate.lock().unwrap();
                match self.session(&sref) {
                    Some(session) => Some(session),
                    None => {
                        tracing::info!(agent = %id, "booting a stopped session on its follow-up");
                        self.boot_agent(id, cols, rows, Some(text))?;
                        None
                    }
                }
            }
        };
        if let Some(session) = live {
            session.write_input(&typed_bytes(text))?;
            session.write_input(b"\r")?;
        }
        Ok(())
    }

    /// Boot every dead, non-archived session under `worktree_id` (agents and
    /// terminals) so a later Attach replays an already-running screen.
    /// Already-alive sessions pass through ensure_session untouched; one
    /// session failing to spawn (missing CLI, deleted checkout) is logged
    /// and doesn't stop the rest.
    pub fn prewarm_worktree_sessions(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        cols: u16,
        rows: u16,
    ) {
        let daemon = self.clone();
        let worktree_id = worktree_id.clone();
        let handle = tokio::spawn(async move {
            daemon.run_worktree_prewarm(&worktree_id, cols, rows).await;
        });
        // Supersede whatever sweep was still warming the worktree the user
        // has now left; its remaining boots are wasted work.
        if let Some(old) = self.prewarm_sweep.lock().unwrap().replace(handle) {
            old.abort();
        }
    }

    /// The sweep itself: boot the worktree's dead sessions one at a time,
    /// [`PREWARM_STAGGER`] apart. Deliberately off the connection's request
    /// loop — it used to run inline, which stalled that client's Input and
    /// Attach frames for as long as the whole burst of forks took.
    async fn run_worktree_prewarm(
        self: &Arc<Self>,
        worktree_id: &WorktreeId,
        cols: u16,
        rows: u16,
    ) {
        let Ok((_, _, agents, terminals)) = self.store.load_tree() else {
            return;
        };
        let srefs: Vec<SessionRef> = agents
            .iter()
            .filter(|a| &a.worktree_id == worktree_id && !a.archived)
            .map(|a| SessionRef::Agent(a.id.clone()))
            .chain(
                terminals
                    .iter()
                    // A RUN or SETUP TERMINAL never boots from a sweep (see
                    // `ensure_session`).
                    .filter(|t| &t.worktree_id == worktree_id && t.run_command.is_none())
                    .map(|t| SessionRef::Terminal(t.id.clone())),
            )
            .collect();
        for sref in srefs {
            // The prewarm doubles as a "user is looking here" signal for
            // the idle reaper, for alive sessions as much as fresh spawns.
            self.touch_session(&sref);
            // Already warm — most importantly the one the user just
            // attached to, which Attach spawned a moment ago.
            if self.is_alive(&sref) {
                continue;
            }
            let daemon = self.clone();
            let target = sref.clone();
            // fork/exec blocks; keep it off the async worker threads.
            let spawned = tokio::task::spawn_blocking(move || {
                daemon.ensure_session(&target, cols, rows).map(|_| ())
            })
            .await;
            match spawned {
                Ok(Err(e)) => {
                    tracing::debug!(session = ?sref, error = %e, "session prewarm failed")
                }
                Err(e) => tracing::debug!(session = ?sref, error = %e, "session prewarm panicked"),
                Ok(Ok(())) => {}
            }
            tokio::time::sleep(PREWARM_STAGGER).await;
        }
    }

    /// Bring back the agents the boot sweep found cut off mid-turn (`ids`,
    /// from `Store::sweep_disconnected`): a daemon restart — an upgrade,
    /// `orion kill` — took their PTYs, and they would otherwise sit dead
    /// until someone attached and typed "continue". Each is resumed on
    /// its stored session id, opening on [`RESUME_INTERRUPTED_PROMPT`]
    /// where its CLI takes one, [`PREWARM_STAGGER`] apart like the
    /// prewarm sweep. Off the caller's task: every boot is a fork/exec.
    pub fn resume_interrupted(self: &Arc<Self>, ids: Vec<AgentId>) {
        if ids.is_empty() {
            return;
        }
        let daemon = self.clone();
        tokio::spawn(async move {
            for id in ids {
                if daemon.shutdown.is_cancelled() {
                    return;
                }
                let worker = daemon.clone();
                let target = id.clone();
                let resumed =
                    tokio::task::spawn_blocking(move || worker.resume_interrupted_agent(&target))
                        .await;
                match resumed {
                    Ok(Ok(true)) => {
                        tracing::info!(agent = %id, "resumed a session the restart interrupted")
                    }
                    Ok(Ok(false)) => continue,
                    Ok(Err(e)) => {
                        tracing::warn!(agent = %id, error = %e, "resume after restart failed")
                    }
                    Err(e) => {
                        tracing::warn!(agent = %id, error = %e, "resume after restart panicked")
                    }
                }
                tokio::time::sleep(PREWARM_STAGGER).await;
            }
        });
    }

    /// One agent of [`Self::resume_interrupted`]. False when there is
    /// nothing to bring back: archived, no session id to resume, or
    /// already running (an Attach got there first).
    fn resume_interrupted_agent(self: &Arc<Self>, id: &AgentId) -> Result<bool> {
        // `ensure_session`'s gate: an Attach racing this boot must not
        // fork a second CLI for the same row.
        let _gate = self.spawn_gate.lock().unwrap();
        if self.is_alive(&SessionRef::Agent(id.clone())) {
            return Ok(false);
        }
        let Some(agent) = self.store.get_agent(id)? else {
            return Ok(false);
        };
        if agent.archived || agent.session_id.is_none() {
            return Ok(false);
        }
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;
        let harness = resolve_harness(agent.kind, agent.custom_harness.as_deref())?;
        // A Claude transcript gone from disk boots the spawn fresh, and a
        // fresh session has no request to continue: leave it for an attach.
        if agent
            .session_id
            .as_deref()
            .is_some_and(|sid| self.claude_transcript_missing(&agent, &harness, sid))
        {
            return Ok(false);
        }
        // Only CLIs verified to open a resumed session on a trailing prompt
        // get one (the relocation notice's flag); the rest come back at
        // their input box, still `disconnected` until their next turn.
        let prompt = harness
            .relocation_prompt
            .then_some(RESUME_INTERRUPTED_PROMPT);
        if prompt.is_some() {
            // Working from the moment it boots, as a launch with a task is
            // (see `create_agent`): persisted first, so a spawn failure can
            // put the row back, and seeded with the launch reprieve.
            self.apply_status_effects(id, vec![Effect::SetStatus(AgentStatus::Running)]);
            self.status_machines
                .lock()
                .unwrap()
                .insert(id.clone(), AgentStatusMachine::launching());
        }
        let spawned = self.spawn_agent_session_with(
            &agent,
            &worktree,
            DEFAULT_COLS,
            DEFAULT_ROWS,
            prompt,
            orion_core::harness::AgentMode::Edit,
        );
        if let Err(e) = spawned {
            if prompt.is_some() {
                self.status_machines.lock().unwrap().remove(id);
                self.apply_status_effects(id, vec![Effect::SetStatus(AgentStatus::Disconnected)]);
            }
            return Err(e);
        }
        self.try_broadcast_agent(id);
        Ok(true)
    }

    /// Whether `sid` is a Claude session with no transcript behind it, so
    /// a resume of it would find "No conversation found". Looked for where
    /// the harness keeps its transcripts, a second account's own config
    /// dir included (`claude_projects_roots`). An override (tests) never
    /// resumes, so it skips the look.
    fn claude_transcript_missing(
        &self,
        agent: &Agent,
        harness: &HarnessDescriptor,
        sid: &str,
    ) -> bool {
        std::env::var_os(env::AGENT_CMD).is_none()
            && self
                .claude_projects_roots(agent, harness)
                .is_some_and(|roots| claude_transcript_exists(&roots, sid) == Some(false))
    }

    pub(crate) fn spawn_agent_session(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        self.spawn_agent_session_with(
            agent,
            worktree,
            cols,
            rows,
            None,
            orion_core::harness::AgentMode::Edit,
        )
    }

    /// The general spawn: `initial_prompt` a first turn the CLI submits on
    /// its own (the relocation notice a `orion worktree` respawn opens with,
    /// or the prefix + task + postfix an AGENT PRESET launch composes). It
    /// is intentionally transient: later restarts/resumes follow the
    /// persisted Agent fields.
    fn spawn_agent_session_with(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
        initial_prompt: Option<&str>,
        mode: orion_core::harness::AgentMode,
    ) -> Result<Arc<PtySession>> {
        // A session the user sent to Claude's background (`/background`)
        // can't be resumed, only attached to — see `claude_bg`. The probe
        // costs a login shell, so it hides behind the one-`stat` hint. A
        // row whose harness no longer resolves looks for nothing: the
        // spawn below refuses it with the reason.
        let attach = match resolve_harness(agent.kind, agent.custom_harness.as_deref()) {
            Ok(harness) if self.claude_job_hint(agent, &harness) => {
                self.claude_background_id(agent, &harness)
            }
            _ => None,
        };
        self.spawn_agent_pty(
            agent,
            worktree,
            cols,
            rows,
            initial_prompt,
            attach.as_deref(),
            mode,
        )
    }

    /// [`Self::spawn_agent_session_with`] past its look for a backgrounded
    /// Claude session: `attach` is the id `claude attach` takes, and wins
    /// over a resume of the stored session id — and over `initial_prompt`,
    /// which `attach` has no way to submit.
    #[allow(clippy::too_many_arguments)]
    fn spawn_agent_pty(
        self: &Arc<Self>,
        agent: &Agent,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
        initial_prompt: Option<&str>,
        attach: Option<&str>,
        mode: orion_core::harness::AgentMode,
    ) -> Result<Arc<PtySession>> {
        // Whatever spawns this agent, it runs in `worktree` from here: a
        // relocation still pending for it has been overtaken.
        self.pending_moves.lock().unwrap().remove(&agent.id);
        // Every row resolves its registry descriptor once, up front: a row
        // whose entry was deleted or broken since refuses the boot with
        // its reason rather than launching the wrong CLI, and the same
        // descriptor picks the hook dialect below.
        let harness = resolve_harness(agent.kind, agent.custom_harness.as_deref())?;
        // Managed status hooks; a failure here degrades to "no status
        // updates", never blocks the spawn. The dialect is data: a custom
        // harness naming one reports status, prompts and permission waits
        // exactly like that harness, and a harness with none runs
        // hookless (process-based status until a dialect is mapped).
        let install_result = match harness.hook_dialect() {
            Some(AgentKind::Claude) => hooks::installer::install_claude_hooks(&worktree.path),
            // Codex's hooks live in its home, not the worktree, so one
            // trust approval covers every worktree (see installer docs);
            // any per-worktree copy an older orion left is pruned.
            Some(AgentKind::Codex) => {
                hooks::installer::install_codex_hooks(&hooks::installer::codex_home())
                    .and_then(|()| hooks::installer::prune_codex_worktree_hooks(&worktree.path))
            }
            // Cursor also gets the managed auto-title project rule — its
            // hook dialect has no context-injection channel.
            Some(AgentKind::Cursor) => hooks::installer::install_cursor_hooks(&worktree.path)
                .and_then(|()| hooks::installer::install_cursor_title_rule(&worktree.path)),
            // Grok reads personal hook files from its home with no trust
            // prompt, so one file of orion's there serves every worktree;
            // its auto-title rides `--rules` (`launch_system_prompt`).
            Some(AgentKind::Grok) => {
                hooks::installer::install_grok_hooks(&hooks::installer::grok_home())
            }
            // OpenCode runs TypeScript plugins, not shell hooks: one
            // managed plugin in its global config dir (globbed at startup,
            // no trust prompt) serves every worktree.
            Some(AgentKind::OpenCode) => {
                hooks::opencode_plugin::install(&hooks::opencode_plugin::opencode_config_dir())
            }
            _ => Ok(()),
        };
        if let Err(e) = install_result {
            tracing::warn!(error = %e, cwd = %worktree.path.display(), "hook install failed");
        }
        // The two dialects that write into the checkout keep those files out
        // of the project's `git status`.
        if matches!(
            harness.hook_dialect(),
            Some(AgentKind::Claude | AgentKind::Cursor)
        ) {
            if let Err(e) = hooks::installer::exclude_managed_files(&worktree.path) {
                tracing::warn!(error = %e, cwd = %worktree.path.display(), "info/exclude not updated");
            }
        }

        // ORION_AGENT_CMD overrides for tests; default is the kind's CLI.
        let cmd_override = std::env::var(env::AGENT_CMD).ok();
        // A Claude session id with no transcript behind it — a CLI nobody
        // sent a prompt, or a session Claude's cleanup has deleted — resumes
        // into "No conversation found" and a dead pane: boot fresh instead.
        let unresumable;
        let agent = match agent.session_id.as_deref() {
            Some(sid)
                if attach.is_none() && self.claude_transcript_missing(agent, &harness, sid) =>
            {
                tracing::info!(agent = %agent.id, session = %sid, "no Claude transcript for the session — spawning fresh");
                if let Err(e) = self.store.set_agent_session_id(&agent.id, None) {
                    tracing::warn!(agent = %agent.id, error = %e, "clear session id failed");
                }
                let mut fresh = agent.clone();
                fresh.session_id = None;
                unresumable = fresh;
                &unresumable
            }
            _ => agent,
        };
        // A PR SESSION's rule rides Claude's system prompt, or opens a
        // Codex / Cursor cold spawn as its first prompt (see `pr_scope`).
        // Rebuilt from the row's *current* worktree on every spawn, so a
        // relocated session is told where it now works.
        let pr_url = self.store.agent_pr_url(&agent.id)?;
        let root = match &pr_url {
            Some(_) if !worktree.is_main => self
                .store
                .get_project(&worktree.project_id)?
                .map(|p| p.repo_path),
            _ => None,
        };
        let scope = pr_url.as_deref().map(|url| crate::pr_scope::PrScope {
            url,
            worktree: &worktree.path,
            branch: &worktree.branch,
            root: root.as_deref(),
        });
        let rule = scope.as_ref().map(crate::pr_scope::rule);
        let prompts = crate::pr_scope::launch_prompts(
            harness.system.append_flag.is_some(),
            agent.session_id.is_some(),
            rule.as_deref(),
            initial_prompt,
        );
        let system = launch_system_prompt(
            &harness,
            agent.session_id.is_some(),
            prompts.system.as_deref(),
        );
        let (program, args, resumed) = match attach {
            // Never a watched resume: an attach that dies at once keeps the
            // row's session id, and the pane keeps the CLI's reason.
            Some(id) => {
                if prompts.initial.is_some() {
                    tracing::warn!(agent = %agent.id, "backgrounded Claude session — `claude attach` takes no prompt, dropping the initial one");
                }
                (
                    harness.program.trim().to_string(),
                    claude_bg::attach_args(id),
                    false,
                )
            }
            None => {
                let (program, mut args, resumed) = agent_spawn_command_with(
                    &harness,
                    agent.session_id.as_deref(),
                    Some(&worktree.path),
                    agent.model.as_deref(),
                    agent.effort.as_deref(),
                    cmd_override.as_deref(),
                    prompts.initial.as_deref(),
                    system.as_deref(),
                    true,
                );
                if !resumed && cmd_override.is_none() {
                    push_mode(&harness, mode, &mut args);
                }
                (program, args, resumed)
            }
        };
        // Run the agent through the user's login+interactive shell so it sees
        // the same env as a Terminal.app tab (~/.zprofile, ~/.zshrc,
        // path_helper) instead of the daemon's inherited-at-boot env, and
        // resolves the CLI the way a typed command would — an alias or
        // function in those files wins over the binary on PATH. The
        // harness's own `env` goes on the PTY and is restated after the
        // profile, so an rc file exporting the same name cannot undo it.
        // Overrides (tests) stay verbatim, the `env` on the PTY alone.
        let launch_env = harness.launch_env();
        let (program, args) = if cmd_override.is_some() {
            (program, args)
        } else {
            login_shell_wrap(
                &orion_core::shell::user_shell(),
                &launch_env,
                &program,
                &args,
            )
        };

        let mut session_env = launch_env;
        session_env.extend([
            (env::AGENT_ID.into(), agent.id.to_string()),
            (
                env::API_URL.into(),
                format!("http://127.0.0.1:{}", self.hook_env.port),
            ),
            (env::API_TOKEN.into(), self.hook_env.token.clone()),
        ]);
        let spec = SpawnSpec {
            program,
            args,
            cwd: worktree.path.clone(),
            env: session_env,
            scrub_env: env::AGENT_SESSION_VARS,
            cols,
            rows,
            scan_ports: false,
        };
        let sref = SessionRef::Agent(agent.id.clone());
        let session = PtySession::spawn(sref, spec)?;
        // Recorded before the install, so a CLI that dies at once still
        // finds its watch when `watch_for_exit` sees it go.
        {
            let mut resumes = self.resumes.lock().unwrap();
            if resumed {
                resumes.insert(
                    agent.id.clone(),
                    ResumeWatch {
                        session: Arc::downgrade(&session),
                        spawned_at: Instant::now(),
                        cols,
                        rows,
                    },
                );
            } else {
                resumes.remove(&agent.id);
            }
        }
        self.install_session(session.clone());
        Ok(session)
    }

    /// A resumed session (`claude --resume` / `codex resume` /
    /// `cursor-agent --resume`) that died inside [`RESUME_FAIL_WINDOW`]
    /// could not find its session: clear the id and boot fresh instead of
    /// leaving a dead pane. Reached from `watch_for_exit` on a natural death
    /// only, so a restart or relocation that killed the PTY on purpose (and
    /// has respawned it already) never gets a second CLI.
    fn respawn_failed_resume(self: &Arc<Self>, id: &AgentId, cols: u16, rows: u16) {
        // `ensure_session`'s gate: an Attach reaching for the dead session
        // right now must not fork a CLI beside this one.
        let _gate = self.spawn_gate.lock().unwrap();
        if self.is_alive(&SessionRef::Agent(id.clone())) {
            return; // respawned already, and that spawn is watched itself
        }
        // Archived or deleted inside the window: never resurrect those.
        let Ok(Some(mut agent)) = self.store.get_agent(id) else {
            return;
        };
        if agent.archived {
            return;
        }
        let Some(sid) = agent.session_id.take() else {
            return;
        };
        // A Claude transcript still on disk — where the row's harness keeps
        // them — says the id is good and the CLI quit over something else,
        // a bad flag, a login: keep the id for the next attach, and the
        // pane keeps the CLI's reason.
        let harness = resolve_harness(agent.kind, agent.custom_harness.as_deref()).ok();
        let transcript_kept = harness.as_ref().is_some_and(|harness| {
            self.claude_projects_roots(&agent, harness)
                .is_some_and(|roots| claude_transcript_exists(&roots, &sid) == Some(true))
        });
        if let (true, Some(harness)) = (transcript_kept, &harness) {
            // One reason Claude refuses a good id: the session runs in its
            // background daemon now, and only `claude attach` opens it. The
            // spawn's own look missed it — its job-dir hint is Claude's
            // private layout, free to move — so this one asks outright.
            agent.session_id = Some(sid);
            if let Some(attach) = self.claude_background_id(&agent, harness) {
                let Ok(Some(worktree)) = self.store.get_worktree(&agent.worktree_id) else {
                    return;
                };
                tracing::info!(agent = %id, attach = %attach, "resume refused for a backgrounded session — attaching");
                if self
                    .spawn_agent_pty(
                        &agent,
                        &worktree,
                        cols,
                        rows,
                        None,
                        Some(&attach),
                        orion_core::harness::AgentMode::Edit,
                    )
                    .is_ok()
                {
                    agent.alive = true;
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Agent(agent),
                    });
                }
                return;
            }
            tracing::info!(agent = %id, "resume failed fast with its transcript intact — keeping the session id");
            return;
        }
        let Ok(Some(worktree)) = self.store.get_worktree(&agent.worktree_id) else {
            return;
        };
        tracing::info!(agent = %id, "resume failed fast — respawning fresh");
        if let Err(e) = self.store.set_agent_session_id(id, None) {
            tracing::warn!(agent = %id, error = %e, "clear session id failed");
        }
        if self
            .spawn_agent_session(&agent, &worktree, cols, rows)
            .is_ok()
        {
            agent.alive = true;
            self.broadcast(ServerEvent::EntityUpserted {
                entity: Entity::Agent(agent),
            });
        }
    }

    /// The resume watch `session` was spawned under, while it is still the
    /// agent's latest — taken, so each is judged once.
    fn take_resume_watch(&self, id: &AgentId, session: &Arc<PtySession>) -> Option<ResumeWatch> {
        let mut resumes = self.resumes.lock().unwrap();
        match resumes.get(id) {
            Some(watch) if std::ptr::eq(watch.session.as_ptr(), Arc::as_ptr(session)) => {
                resumes.remove(id)
            }
            _ => None,
        }
    }

    /// Whether Claude keeps a background job under `agent`'s session id —
    /// the cheap look that earns [`Self::claude_background_id`] its probe.
    /// The job dirs sit beside the projects dirs, in the same config dir.
    fn claude_job_hint(&self, agent: &Agent, harness: &HarnessDescriptor) -> bool {
        let Some(sid) = claude_resumable_session(agent) else {
            return false;
        };
        self.claude_projects_roots(agent, harness)
            .unwrap_or_default()
            .iter()
            .filter_map(|projects| projects.parent())
            .any(|config| claude_bg::job_hint(config, sid))
    }

    /// The id `claude attach` takes for `agent`'s session, when `claude
    /// agents --json` lists it as a background session. The listing runs
    /// through the login shell the agent itself would: the harness's own
    /// CLI, under its own `env` — the same `CLAUDE_CONFIG_DIR`.
    fn claude_background_id(&self, agent: &Agent, harness: &HarnessDescriptor) -> Option<String> {
        let sid = claude_resumable_session(agent)?;
        self.claude_projects_roots(agent, harness)?;
        let listing = ["agents".to_string(), "--json".to_string()];
        let (program, args) = login_shell_wrap(
            &orion_core::shell::user_shell(),
            &harness.launch_env(),
            harness.program.trim(),
            &listing,
        );
        let id = claude_bg::probe(&program, &args, sid)?;
        tracing::info!(agent = %agent.id, session = %sid, attach = %id, "Claude session runs in the background");
        Some(id)
    }

    /// Where `agent`'s Claude transcripts may sit, when its harness lets
    /// orion know — the roots the resume safeguards look in: the transcript
    /// checked before `--resume`, the transcript a failed resume is judged
    /// by, the background job a resume would be refused over. One dir when
    /// the harness's `env` pins `CLAUDE_CONFIG_DIR` (a second account, or
    /// built-in Claude itself), since the spawn restates that after the
    /// profile; built-in Claude otherwise looks wherever its hooks have
    /// reported and in this process's default
    /// ([`Self::claude_projects_dirs`]). None for every other harness.
    pub(crate) fn claude_projects_roots(
        &self,
        agent: &Agent,
        harness: &HarnessDescriptor,
    ) -> Option<Vec<PathBuf>> {
        let builtin = agent.kind == AgentKind::Claude;
        if !builtin && !harness.claude_like() {
            return None;
        }
        match harness.pinned_claude_config_dir() {
            Some(dir) => Some(vec![dir.join("projects")]),
            None if builtin => Some(self.claude_projects_dirs()),
            None => None,
        }
    }

    /// Every Claude projects dir a transcript may sit in: the ones this
    /// daemon's Claude hooks reported (wherever the agent's shell pointed
    /// `CLAUDE_CONFIG_DIR`), then the default.
    fn claude_projects_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self
            .transcripts
            .lock()
            .unwrap()
            .values()
            .filter_map(|t| Some(t.transcript_path.parent()?.parent()?.to_path_buf()))
            .collect();
        dirs.extend(orion_core::paths::claude_config_dir().map(|dir| dir.join("projects")));
        dirs.sort();
        dirs.dedup();
        dirs
    }

    pub(crate) fn spawn_terminal_session(
        self: &Arc<Self>,
        terminal: &TerminalTab,
        worktree: &Worktree,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<PtySession>> {
        let (program, args) = match &terminal.run_command {
            // A RUN TERMINAL (or a SETUP TERMINAL) runs its command line
            // through the login + interactive shell an agent launch uses,
            // so `npm` or `bun` resolve the way they do typed. The PTY
            // lives exactly as long as the command, which is what makes a
            // run's the RUNNING state.
            Some(command) => login_shell_line(&orion_core::shell::user_shell(), command),
            // `-l` makes it a login shell, matching Terminal.app: zsh then
            // sources /etc/zprofile (path_helper), ~/.zprofile, and ~/.zshrc.
            None => (orion_core::shell::user_shell(), vec!["-l".into()]),
        };
        let spec = SpawnSpec {
            program,
            args,
            cwd: worktree.path.clone(),
            env: vec![],
            scrub_env: env::AGENT_SESSION_VARS,
            cols,
            rows,
            // A run's dev server says where it listens; a setup's install
            // and a shell's output are not for reading.
            scan_ports: terminal.is_run(),
        };
        let sref = SessionRef::Terminal(terminal.id.clone());
        let session = PtySession::spawn(sref, spec)?;
        self.install_session(session.clone());
        Ok(session)
    }

    fn install_session(self: &Arc<Self>, session: Arc<PtySession>) {
        self.touch_session(&session.sref);
        self.sessions
            .lock()
            .unwrap()
            .insert(session.sref.clone(), session.clone());
        // After the insert: a forward task woken by this looks the ref up.
        let _ = self.session_installs.send(session.sref.clone());
        self.watch_for_exit(session);
    }

    /// Once the child dies: drop it from the registry, feed the status
    /// machine (agents), and tell subscribers the entity is no longer alive.
    fn watch_for_exit(self: &Arc<Self>, session: Arc<PtySession>) {
        let daemon = self.clone();
        let mut rx = session.events.subscribe();
        let sref = session.sref.clone();
        tokio::spawn(async move {
            // How many of a run's ports its row was last sent with: the
            // set only grows, so a count says whether the row is behind.
            let mut ports_sent = 0;
            loop {
                match rx.recv().await {
                    Ok(PtyEvent::Exited { exit_code }) => {
                        // Deliberate kills (archive/restart/delete) remove the
                        // entry first — only a *natural* death of the still-
                        // registered session drives status, so a restart never
                        // flags the fresh PTY's agent as terminated.
                        let was_registered = {
                            let mut sessions = daemon.sessions.lock().unwrap();
                            match sessions.get(&sref) {
                                Some(current) if Arc::ptr_eq(current, &session) => {
                                    sessions.remove(&sref);
                                    true
                                }
                                _ => false,
                            }
                        };
                        if was_registered {
                            daemon.session_interest.lock().unwrap().remove(&sref);
                        }
                        // Taken on any exit, so a killed resume leaves no
                        // record behind; acted on for a natural death only.
                        let resume = match &sref {
                            SessionRef::Agent(id) => daemon.take_resume_watch(id, &session),
                            SessionRef::Terminal(_) => None,
                        };
                        if !was_registered {
                            break;
                        }
                        tracing::info!(session = ?sref, exit_code, "session exited");
                        // A SPARE WORKTREE's setup has no row: its spare
                        // hears how it went. Asked first, so a take that
                        // puts its row in after this exit finds it ended.
                        if let SessionRef::Terminal(id) = &sref {
                            if !daemon.spares.setup_ended(id, exit_code) {
                                match daemon.command_terminal(id) {
                                    Some(t) if t.setup => {
                                        daemon.setup_ended(&t, &session, exit_code)
                                    }
                                    Some(_) => daemon.run_ended(id, &session, exit_code),
                                    None => {}
                                }
                            }
                        }
                        if let SessionRef::Agent(id) = &sref {
                            daemon.apply_hook_event(
                                id,
                                HookEvent::SessionEnded { exit_code },
                                None,
                            );
                        }
                        let upsert = match &sref {
                            SessionRef::Agent(id) => daemon.agent_entity(id).map(Entity::Agent),
                            SessionRef::Terminal(id) => {
                                daemon.terminal_entity(id).map(Entity::Terminal)
                            }
                        };
                        if let Ok(entity) = upsert {
                            daemon.broadcast(ServerEvent::EntityUpserted { entity });
                        }
                        if let (SessionRef::Agent(id), Some(watch)) = (&sref, resume) {
                            if exit_code.unwrap_or(1) != 0
                                && watch.spawned_at.elapsed() < RESUME_FAIL_WINDOW
                            {
                                daemon.respawn_failed_resume(id, watch.cols, watch.rows);
                            }
                        }
                        break;
                    }
                    // The CLI's own busy/idle bit, read off its output. It is
                    // the only end-of-turn news after a user cancel: Claude
                    // Code fires no Stop for an interrupted turn, and
                    // suppresses the idle notification because the user just
                    // pressed a key. See `pty::progress`. Only while this PTY
                    // is still the session's: a killed CLI clears its bar as
                    // it dies, and that must not speak for the replacement a
                    // restart or relocation has installed by then.
                    Ok(PtyEvent::Progress { busy }) => {
                        if let SessionRef::Agent(id) = &sref {
                            if daemon.owns_session(&sref, &session) {
                                daemon.apply_hook_event(id, HookEvent::Progress { busy }, None);
                                // A cancelled turn's only word: a relocation
                                // waiting on this turn's end goes now, not
                                // at the end of the next one.
                                daemon.complete_pending_move(id, &HookEvent::Progress { busy });
                            }
                        }
                    }
                    // The window title carries Claude's session name, and
                    // `/rename` fires no hook — this is the cue to read the
                    // title it persisted (see `session_title`).
                    Ok(PtyEvent::Title { title }) => {
                        if let SessionRef::Agent(id) = &sref {
                            daemon.on_pty_title(id, &title);
                        }
                    }
                    // A run printed where it listens: its line shows it.
                    Ok(PtyEvent::Ports) => {
                        daemon.send_new_ports(&session, &mut ports_sent);
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // A fire-hosing child can push progress edges off the
                        // broadcast queue. The scanner itself never lags, so
                        // reconcile from its current reading rather than
                        // leaving the status stuck on a dropped edge.
                        if let (SessionRef::Agent(id), Some(busy)) =
                            (&sref, session.progress_busy())
                        {
                            if daemon.owns_session(&sref, &session) {
                                daemon.apply_hook_event(id, HookEvent::Progress { busy }, None);
                                daemon.complete_pending_move(id, &HookEvent::Progress { busy });
                            }
                        }
                        // A dropped Ports with them: the scanner never
                        // lags either.
                        daemon.send_new_ports(&session, &mut ports_sent);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// `text` as keystrokes a CLI's input box takes as typed: a BRACKETED
/// PASTE when it has line breaks, so the CLI takes it as one block, plain
/// bytes otherwise, which keeps a one-liner out of the "[Pasted text]"
/// placeholder. The Enter that submits it is the caller's, as a write of
/// its own.
pub(crate) fn typed_bytes(text: &str) -> Vec<u8> {
    if text.contains('\n') {
        let mut data = b"\x1b[200~".to_vec();
        data.extend_from_slice(text.as_bytes());
        data.extend_from_slice(b"\x1b[201~");
        data
    } else {
        text.as_bytes().to_vec()
    }
}

/// The session id a spawn of `agent` would hand `--resume` — a Claude one
/// whenever [`Daemon::claude_projects_roots`] names somewhere to look for
/// it, which is what the callers ask first. None under the
/// `ORION_AGENT_CMD` override (tests), which never resumes.
/// The row a MOVED CHECKOUT on `branch` came from: the one linked row of
/// `ours` on that branch whose path git no longer lists, and which no other
/// checkout has taken over already (`rehomed`). None when there is none,
/// or more than one to choose between. git checks a branch out in one
/// place at a time, so a vanished row and a new checkout on the same
/// branch are the one checkout, moved.
fn moved_from<'a>(
    ours: &[&'a Worktree],
    entries: &[git::WorktreeEntry],
    branch: &str,
    rehomed: &HashSet<WorktreeId>,
) -> Option<&'a Worktree> {
    if branch.is_empty() || git::is_detached_label(branch) {
        return None;
    }
    let mut vanished = ours.iter().copied().filter(|w| {
        !w.is_main
            && w.branch == branch
            && !rehomed.contains(&w.id)
            && !entries.iter().any(|e| e.path == w.path)
    });
    let row = vanished.next()?;
    vanished.next().is_none().then_some(row)
}

fn claude_resumable_session(agent: &Agent) -> Option<&str> {
    if std::env::var(env::AGENT_CMD).is_ok() {
        return None;
    }
    agent.session_id.as_deref()
}

/// Whether Claude Code still holds the transcript `claude --resume <id>`
/// reads — `<projects>/<cwd slug>/<id>.jsonl` under any of `roots`, in any
/// slug: a session `orion worktree` relocated keeps its transcript where
/// it began, and resumes from there. `None` when no root could be read at
/// all, which is no verdict.
fn claude_transcript_exists(roots: &[PathBuf], session_id: &str) -> Option<bool> {
    let file = format!("{session_id}.jsonl");
    // An id that isn't one plain file name names no transcript.
    if Path::new(&file).file_name() != Some(std::ffi::OsStr::new(&file)) {
        return Some(false);
    }
    let mut read_any = false;
    for root in roots {
        let Ok(slugs) = std::fs::read_dir(root) else {
            continue;
        };
        read_any = true;
        if slugs
            .flatten()
            .any(|slug| slug.path().join(&file).is_file())
        {
            return Some(true);
        }
    }
    read_any.then_some(false)
}

/// Program + args for an agent PTY. An override (tests) is used verbatim —
/// no resume args. Otherwise the kind picks the CLI and its resume shape:
/// `claude --resume <sid>` and `cursor-agent --resume <sid>` (flag) vs
/// `codex resume <sid> --cd <cwd>` (subcommand, so resume args must lead;
/// the `--cd` because codex reopens a resumed session in the directory its
/// transcript recorded — or asks which to use — unless told one, and a
/// relocated session must land in its new worktree, not the old checkout).
/// `cwd` is the checkout every local spawn boots in. Codex and cursor
/// always get their skip-permissions flag (`--yolo` / `--force`), appended
/// after the resume args — same convention as Mission Control.
/// Model/effort choices follow: `claude --model m --effort e`,
/// `codex -m m -c model_reasoning_effort=e`, and for cursor one flat id
/// joined from the two — `cursor-agent --model m-e` (`--model m` when
/// effort is None; the CLI's catalogue bakes the effort into the id and
/// rejects the `m[effort=e]` form its `--help` advertises).
/// Claude then gets orion's worktree guidance appended to the system
/// prompt, any persisted PR scope is composed into that same system-prompt
/// argument, and an `initial_prompt` — the relocation notice a `orion
/// worktree` respawn opens with, or the starting prompt an AGENT PRESET
/// launch composes — goes last, as the CLI's trailing positional prompt
/// (`claude [prompt]`, `codex [PROMPT]`, `cursor-agent [prompt...]`).
///
/// The plain shape, as every restart/resume spawns it: booted in
/// `TEST_CWD`, no initial prompt, guidance on. Tests assert against this;
/// the daemon calls the full form. The descriptor resolves from a pinned
/// registry so tests never touch the user's config.
#[cfg(test)]
fn agent_spawn_command(
    kind: AgentKind,
    session_id: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    cmd_override: Option<&str>,
) -> (String, Vec<String>, bool) {
    let all = test_registry();
    let harness = test_harness(&all, kind);
    agent_spawn_command_with(
        &harness,
        session_id,
        Some(Path::new(TEST_CWD)),
        model,
        effort,
        cmd_override,
        None,
        None,
        true,
    )
}

/// A pinned registry for spawn tests: the compiled-in rows, no overrides
/// — what a fresh install launches.
#[cfg(test)]
fn test_registry() -> Vec<orion_core::harness::HarnessDescriptor> {
    harness_registry_in(&std::collections::BTreeMap::new(), &[])
}

/// The pinned descriptor `kind` launches as in spawn tests.
#[cfg(test)]
fn test_harness(
    all: &[orion_core::harness::HarnessDescriptor],
    kind: AgentKind,
) -> orion_core::harness::HarnessDescriptor {
    resolve_harness_in(kind, None, all).expect("built-ins resolve from a pinned registry")
}

/// The pinned descriptor for a `harnesses` map entry in spawn tests.
#[cfg(test)]
fn test_custom_harness(
    all: &[orion_core::harness::HarnessDescriptor],
    id: &str,
) -> orion_core::harness::HarnessDescriptor {
    resolve_harness_in(AgentKind::Custom, Some(id), all).expect("the pinned entry resolves")
}

/// What orion appends to Claude's system prompt: how to take a "do this
/// in a worktree" request through orion (`Daemon::enter_worktree`) instead
/// of Claude's own EnterWorktree tool, whose checkout lands under
/// `<repo>/.claude/worktrees/` on a `worktree-*` branch — a layout the
/// worktree list only adopts after the fact, and not where a orion user
/// keeps their worktrees. Claude takes it through `--append-system-prompt`;
/// codex and cursor have no system-prompt flag.
pub const CLAUDE_WORKTREE_GUIDANCE: &str = "[orion] This session runs inside orion, which \
manages this project's git worktrees. When the user asks you to work in a worktree (\"do this in a \
worktree\", \"in a new worktree\", \"branch this off in its own checkout\"), do not use the \
EnterWorktree tool and do not run `git worktree add` yourself. Run this shell command instead, \
exactly once:\n\n  orion worktree <name>\n\nwhere <name> is the branch name the user gave, or a \
short kebab-case name for the task (`orion worktree` with no name invents one; `--base <ref>` picks \
the start point). orion creates the worktree, associates this session with it, and relocates the \
session into it once your current turn ends. So when the command succeeds, end your turn at once: \
tell the user in one line that the session is moving into the worktree, and make no further tool \
calls or edits — you will be resumed inside the worktree with a prompt to carry on there. If the \
command fails, report the error and carry on in the current checkout.";

/// The first prompt of a session [`Daemon::resume_interrupted`] brings back.
const RESUME_INTERRUPTED_PROMPT: &str =
    "[orion] orion restarted (an upgrade or `orion kill`) and stopped this session mid-turn. \
     Continue the user's most recent request from where you left off.";

/// The prompt a relocated session is resumed with: it names the checkout
/// the process now runs in and asks for the work to pick back up there, so
/// the user never has to type "continue". Only for the CLIs verified to
/// open a resumed session on a trailing prompt — `claude --resume <sid>
/// "<prompt>"`, `codex resume <sid> [PROMPT]` (the positional its `--help`
/// documents; submitted as the next turn, verified live on codex 0.153.4).
/// Whether `cursor-agent --resume
/// <id> [prompt...]` submits one is not, so a relocated cursor session
/// resumes silent and waits for the user.
fn relocation_prompt(relocate: bool, worktree: &Worktree) -> Option<String> {
    relocate.then(|| {
        format!(
            "[orion] This session now runs inside the worktree `{}` at {} — your working \
             directory is that checkout. Continue the user's most recent request there.",
            worktree.branch,
            worktree.path.display()
        )
    })
}

/// The checkout the test wrapper boots every spawn in.
#[cfg(test)]
const TEST_CWD: &str = "/orion-test/p-feat";

// Nine positional knobs are two over clippy's line; the callers are the
// two thin wrappers above and the tests, so a builder would only add
// ceremony. Every behavior comes off `harness` — the registry descriptor
// the launch resolved — never off the kind: a repointed program, a renamed
// flag or a whole new CLI flows through here with no new arms.
#[allow(clippy::too_many_arguments)]
fn agent_spawn_command_with(
    harness: &orion_core::harness::HarnessDescriptor,
    session_id: Option<&str>,
    cwd: Option<&Path>,
    model: Option<&str>,
    effort: Option<&str>,
    cmd_override: Option<&str>,
    initial_prompt: Option<&str>,
    additional_system_prompt: Option<&str>,
    guidance: bool,
) -> (String, Vec<String>, bool) {
    if let Some(cmd) = cmd_override {
        let mut parts = cmd.split_whitespace().map(String::from).collect::<Vec<_>>();
        if parts.is_empty() {
            parts.push(harness.program.trim().to_string());
        }
        let program = parts.remove(0);
        return (program, parts, false);
    }
    let program = harness.program.trim().to_string();
    // Resume: a flag (`--resume <id>`), a positional subcommand
    // (`resume <id>`, with `--cd`), or nothing — a stored id with no
    // resume mapping is ignored and the CLI boots fresh.
    let (mut args, resumed) = match (&harness.resume.flag, &harness.resume.subcommand, session_id) {
        (Some(flag), _, Some(sid)) => (vec![flag.clone(), sid.to_string()], true),
        (None, Some(subcommand), Some(sid)) => {
            let mut args = vec![subcommand.clone(), sid.to_string()];
            if harness.resume.cd {
                if let Some(cwd) = cwd {
                    args.extend(["--cd".to_string(), cwd.to_string_lossy().into_owned()]);
                }
            }
            (args, true)
        }
        _ => (Vec::new(), false),
    };
    if let Some(flag) = harness.permissions_flag.as_deref() {
        args.push(flag.to_string());
    }
    // The model rides its flag — composed with the effort into one id for
    // Cursor's family-suffix shape (`claude-opus-5` + `high`: the TUI only
    // sends an effort the family ships; an effort without a family has
    // nothing to hang off and is dropped).
    if let (Some(m), Some(flag)) = (model, harness.model.flag.as_deref()) {
        let id = match (harness.compose_model_effort, effort) {
            (true, Some(e)) => format!("{m}-{e}"),
            _ => m.to_string(),
        };
        args.extend([flag.to_string(), id]);
    }
    // The effort rides its flag, Codex's `-c key=value` pair, or nothing
    // (composed above, or unmapped and dropped like before).
    if let Some(e) = effort {
        if !harness.compose_model_effort {
            if let Some(flag) = harness.effort.flag.as_deref() {
                args.extend([flag.to_string(), e.to_string()]);
            } else if let (Some(flag), Some(key)) = (
                harness.effort.config_flag.as_deref(),
                harness.effort.config_key.as_deref(),
            ) {
                args.extend([flag.to_string(), format!("{key}={e}")]);
            }
        }
    }
    // Guidance (worktree rules, then the PR scope) rides the
    // system-prompt flag where one is mapped, folds into the first prompt
    // where prepend is, and is dropped where neither is.
    let mut initial_prompt = initial_prompt.map(str::to_string);
    if let Some(flag) = harness.system.append_flag.as_deref() {
        push_system_prompt(&mut args, flag, guidance, additional_system_prompt);
    } else if harness.system.prepend_to_first_prompt {
        let mut first = Vec::new();
        if guidance {
            first.push(CLAUDE_WORKTREE_GUIDANCE.to_string());
            first.push(crate::sibling::CLAUDE_SPAWN_GUIDANCE.to_string());
            first.push(crate::open_files::CLAUDE_OPEN_GUIDANCE.to_string());
        }
        if let Some(prompt) = additional_system_prompt {
            first.push(prompt.to_string());
        }
        if !first.is_empty() {
            let head = first.join("\n\n");
            initial_prompt = Some(match initial_prompt {
                Some(task) => format!("{head}\n\n{task}"),
                None => head,
            });
        }
    }
    // The starting prompt rides trailing, like every CLI's positional —
    // or its own flag where the positional is something else (OpenCode's
    // `--prompt`: its positional is the project path).
    if let Some(p) = initial_prompt {
        if let Some(flag) = harness.prompt_flag.as_deref() {
            args.push(flag.to_string());
        }
        args.push(p);
    }
    (program, args, resumed)
}

/// Start a fresh CLI in `mode`: its flag and value lead the args — a fresh
/// launch has no subcommand for them to follow, and every positional
/// (the first prompt) comes after. Nothing for edit, the mode every CLI
/// starts in, nor for a mode the harness does not have.
fn push_mode(
    harness: &orion_core::harness::HarnessDescriptor,
    mode: orion_core::harness::AgentMode,
    args: &mut Vec<String>,
) {
    if let Some(pair) = harness.mode.args(mode) {
        args.splice(0..0, pair.map(str::to_string));
    }
}

/// What a spawn appends to the CLI's system prompt after orion's guidance:
/// on a fresh launch of a Grok-dialect harness, the AUTO-TITLE rule — Grok
/// reads no passive hook's stdout, so the instruction the other dialects
/// get back from a hook can only ride its `--rules`; a resumed
/// conversation was told the first time — then `launch`'s own text (a PR
/// SESSION's rule). Firing on a session already named is harmless: the
/// DAEMON takes one auto-title per session, as for Cursor's always-on rule.
fn launch_system_prompt(
    harness: &orion_core::harness::HarnessDescriptor,
    resuming: bool,
    launch: Option<&str>,
) -> Option<String> {
    let title = (!resuming
        && harness.system.append_flag.is_some()
        && harness.hook_dialect() == Some(AgentKind::Grok))
    .then(hooks::auto_title_rule);
    match (title, launch) {
        (Some(title), Some(launch)) => Some(format!("{title}\n\n{launch}")),
        (title, launch) => title.or_else(|| launch.map(str::to_string)),
    }
}

/// One system-prompt flag carrying orion's guidance (worktree, spawn,
/// then open) and whatever else the launch adds (the PR scope).
fn push_system_prompt(
    args: &mut Vec<String>,
    flag: &str,
    guidance: bool,
    additional: Option<&str>,
) {
    let mut system_prompt = Vec::new();
    if guidance {
        system_prompt.push(CLAUDE_WORKTREE_GUIDANCE);
        system_prompt.push(crate::sibling::CLAUDE_SPAWN_GUIDANCE);
        system_prompt.push(crate::open_files::CLAUDE_OPEN_GUIDANCE);
    }
    if let Some(prompt) = additional {
        system_prompt.push(prompt);
    }
    if !system_prompt.is_empty() {
        args.extend([flag.to_string(), system_prompt.join("\n\n")]);
    }
}

/// The row of a SETUP TERMINAL running `command` in `worktree`: a new
/// worktree's, or a SPARE WORKTREE's, which becomes its worktree's when a
/// launch takes it.
pub(crate) fn setup_terminal_row(worktree: &Worktree, command: &str) -> TerminalTab {
    TerminalTab {
        id: TerminalId::generate(),
        worktree_id: worktree.id.clone(),
        name: SETUP_TERMINAL_NAME.into(),
        sort_order: 0,
        alive: false,
        run_command: Some(command.to_string()),
        setup: true,
        run: Default::default(),
    }
}

/// Validate an AGENT PRESET's composed starting prompt before it becomes the
/// CLI's positional argument. It rides the login shell's `-c` string as
/// well as the CLI's argv: quoting stops injection, but a NUL would
/// truncate the command, so it is rejected here rather than at the shell.
/// Length is the OS's call alone (`ARG_MAX`, about 1 MiB on macOS): a
/// launch over it is refused at the spawn with [`crate::pty::ARGV_TOO_LONG`].
pub(crate) fn validate_starting_prompt(raw: &str) -> Result<String> {
    let text = raw.trim().to_string();
    if text.is_empty() {
        bail!("starting prompt is empty");
    }
    if text.contains('\0') {
        bail!("starting prompt cannot contain NUL bytes");
    }
    Ok(text)
}

/// Normalize an agent-supplied title: control characters become spaces,
/// whitespace collapses, and over-long titles are cut — models occasionally
/// hand over a whole sentence no matter what the instruction says.
pub(crate) fn sanitize_title(raw: &str) -> String {
    const MAX_CHARS: usize = 60;
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut title = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > MAX_CHARS {
        title = title.chars().take(MAX_CHARS).collect();
        title.truncate(title.trim_end().len());
    }
    title
}

/// Canonicalize for path containment tests, falling back to the raw path
/// when it doesn't resolve (deleted checkout, not-yet-created dir). macOS
/// symlinks (`/tmp` → `/private/tmp`) otherwise break `starts_with`.
pub(crate) fn canonical_or_raw(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Does this agent still have a backgrounded tool call running — a job it
/// cut loose from its terminal, which the turn that started it has already
/// left behind? An unknown child pid counts as busy, as for terminals.
fn agent_has_detached_job(session: &PtySession) -> bool {
    match session.child_pid {
        Some(pid) => crate::pty::detached_job_under(pid),
        None => true,
    }
}

/// Does this terminal's shell have any child processes (a command or job
/// still running)? An unknown child pid or a failed probe counts as busy —
/// never kill what can't be inspected.
fn shell_has_children(session: &PtySession) -> bool {
    let Some(pid) = session.child_pid else {
        return true;
    };
    !matches!(
        std::process::Command::new("pgrep")
            .arg("-P")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
        Ok(status) if !status.success()
    )
}

/// Canonical form of a user-typed link. Pasting a URL out of a browser is
/// the common case, but people also type `github.com/o/r/pull/7`, so a
/// scheme-less value gets https://. Anything else — another scheme, or no
/// host at all — is refused rather than stored: the TUI hands these to
/// `open(1)`, and only http(s) may ever reach it.
pub(crate) fn normalize_url(url: &str) -> Result<String> {
    let url = url.trim();
    if url.is_empty() {
        bail!("link URL is empty");
    }
    if url.contains(char::is_whitespace) {
        bail!("link URL contains whitespace");
    }
    let normalized = match url.split_once("://") {
        Some(("http" | "https", _)) => url.to_string(),
        Some((scheme, _)) => bail!("only http(s) links are supported (got {scheme}://)"),
        // Scheme-less: a bare host is a URL people type; a bare word is not.
        None => {
            let host = url.split(['/', '?', '#']).next().unwrap_or_default();
            if !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
                bail!("not a URL: {url}");
            }
            format!("https://{url}")
        }
    };
    // Reject "https://" and friends: a scheme with nothing behind it.
    if normalized
        .split_once("://")
        .is_none_or(|(_, rest)| rest.is_empty())
    {
        bail!("not a URL: {url}");
    }
    Ok(normalized)
}

/// `pre_exec` hook putting the child in a session of its own (`setsid`):
/// an interactive shell there cannot reach the daemon's controlling
/// terminal, and the child leads a group a timeout can sweep.
pub(crate) fn own_session() -> std::io::Result<()> {
    nix::unistd::setsid()
        .map(drop)
        .map_err(|errno| std::io::Error::from_raw_os_error(errno as i32))
}

/// Why a create was refused when the agent CLI isn't installed. One line —
/// the TUI shows it in the footer flash, which truncates. Unlike git (which
/// the daemon runs with its own inherited PATH), agent CLIs are spawned
/// through the user's login shell, so a fresh install is picked up on the
/// next try with no daemon restart.
fn cli_missing_message(program: &str) -> String {
    format!("{program} was not found on your PATH — install it, then try again.")
}

/// The effective harness registry from the current config: the
/// compiled-in known harnesses with the `harnesses` map applied — Claude's
/// extra accounts (`claude_accounts`) beside it — then map-only new ids.
/// Every launch, resume and
/// hook install resolves through here, so a config edit (not a rebuild) is
/// what adds a CLI.
pub(crate) fn harness_registry() -> Vec<orion_core::harness::HarnessDescriptor> {
    let config = crate::config::Config::load();
    harness_registry_in(&config.harnesses, &config.claude_accounts)
}

/// [`harness_registry`] against an explicit config, so tests can pin the
/// registry without touching the user's files.
fn harness_registry_in(
    overrides: &std::collections::BTreeMap<String, orion_core::harness::HarnessOverride>,
    accounts: &[orion_core::claude_account::ClaudeAccount],
) -> Vec<orion_core::harness::HarnessDescriptor> {
    orion_core::harness::registry(overrides, accounts)
}

/// The descriptor a launch or row runs as: built-ins by kind, customs by
/// registry id. A missing id, or a broken entry, refuses the caller with
/// its reason before anything spawns. An `enabled` switch gates the
/// picker, never an existing row — a harness switched off after its
/// sessions were created keeps running them.
pub(crate) fn resolve_harness(
    kind: AgentKind,
    id: Option<&str>,
) -> Result<orion_core::harness::HarnessDescriptor> {
    resolve_harness_in(kind, id, &harness_registry())
}

/// [`resolve_harness`] against an explicit registry, so tests can pin
/// entries without touching the user's config.
fn resolve_harness_in(
    kind: AgentKind,
    id: Option<&str>,
    all: &[orion_core::harness::HarnessDescriptor],
) -> Result<orion_core::harness::HarnessDescriptor> {
    if orion_core::harness::registry_id(kind, id).is_empty() {
        bail!("custom harness launch is missing its registry id");
    }
    orion_core::harness::resolve(all, kind, id)
        .cloned()
        .map_err(anyhow::Error::msg)
}

/// Wrap `program args…` in a login + interactive shell (`$SHELL -l -i -c
/// 'unset …; export …; prog args'`) so the child gets the user's real
/// environment — ~/.zprofile and ~/.zshrc on zsh — rather than the daemon's.
///
/// The command word goes in bare, so the shell resolves it the way a typed
/// command line would: an alias or function from the rc files wins over the
/// binary on PATH. That is where a work setup reroutes `claude` through a
/// wrapper (another backend, another login), and the earlier `exec env …
/// 'claude'` form skipped it — `env` looks the name up on PATH, and neither
/// zsh nor bash expands the word after an `exec` either — so every session
/// landed on the raw CLI. Without an `exec` the shell stays in charge of the
/// launch: zsh execs a plain last command itself, so the agent is still the
/// PTY's direct child there; bash, and any command that resolves to a
/// function, run it as a job under the shell instead, in a process group of
/// its own — which is why `PtySession::kill` sweeps the whole tree rather
/// than one group.
///
/// The prelude restates what the pane is *after* those files have run:
/// `TERM` and `COLORTERM` name orion's own grid — 24-bit colour whatever
/// the host terminal — and `NO_COLOR` / `FORCE_COLOR` are dropped. A
/// login-only profile that exports `NO_COLOR` reaches a session here and
/// nowhere else (foot and Ghostty on Linux start non-login shells), and
/// Claude Code takes it as "no colour": its whole UI in the default
/// foreground while the TUI around it stays coloured (#37). The spawn sets
/// the same three against the daemon's inherited environment; this covers
/// the profile's.
///
/// `env` is the CLI's own environment — a harness's `env`, single-quoted
/// into one `export` after the prelude, so after the profile too: a
/// `CLAUDE_CONFIG_DIR` an rc file exports is replaced by the harness's,
/// never the other way round. The names were checked by
/// [`orion_core::harness::HarnessDescriptor::problem`] before any spawn.
fn login_shell_wrap(
    shell: &str,
    env: &[(String, String)],
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let mut line = String::new();
    if !env.is_empty() {
        line.push_str("export");
        for (name, value) in env {
            line.push(' ');
            line.push_str(name);
            line.push('=');
            line.push_str(&orion_core::shell::single_quote(value));
        }
        line.push_str("; ");
    }
    line.push_str(&command_word(program));
    for arg in args {
        line.push(' ');
        line.push_str(&orion_core::shell::single_quote(arg));
    }
    login_shell_line(shell, &line)
}

/// [`login_shell_wrap`] for a line that is already shell syntax — a RUN
/// TERMINAL's run command, pipes and `&&` and all — behind the
/// same prelude.
fn login_shell_line(shell: &str, line: &str) -> (String, Vec<String>) {
    let mut cmdline = String::from("unset");
    for name in env::PANE_COLOR_OVERRIDES {
        cmdline.push(' ');
        cmdline.push_str(name);
    }
    cmdline.push_str("; export TERM=");
    cmdline.push_str(env::PANE_TERM);
    cmdline.push_str(" COLORTERM=");
    cmdline.push_str(env::PANE_COLORTERM);
    cmdline.push_str("; ");
    cmdline.push_str(line);
    let args = LOGIN_SHELL_ARGS
        .iter()
        .map(|s| s.to_string())
        .chain([cmdline])
        .collect();
    (shell.to_string(), args)
}

/// `program` as the command word of a shell line: bare when it is a plain
/// name (letters, digits, `-`, `_`, `.`, `/`), since a quoted word is exempt
/// from alias expansion in every shell; single-quoted otherwise.
fn command_word(program: &str) -> String {
    let plain = !program.is_empty()
        && program
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b));
    if plain {
        program.to_string()
    } else {
        orion_core::shell::single_quote(program)
    }
}

/// The login-shell line [`Daemon::probe_cli`] runs to ask whether
/// `program` resolves. The word is single-quoted
/// ([`orion_core::shell::single_quote`]):
/// a `harnesses` entry in config.json can name any string, and pasted in
/// bare a quote would close the word and run the rest as a command — at
/// daemon boot, since [`Daemon::warm_cli_probes`] asks for every entry.
/// Built-in names come out exactly as they always did (`'claude'`).
fn cli_probe_line(program: &str) -> String {
    format!(
        "command -v {} >/dev/null 2>&1",
        orion_core::shell::single_quote(program)
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Claude argv: `args`, then orion's appended guidance (worktree and
    /// spawn, one `--append-system-prompt`).
    fn guided(flag: &str, args: &[&str]) -> Vec<String> {
        args.iter()
            .map(|s| s.to_string())
            .chain([
                flag.to_string(),
                [
                    CLAUDE_WORKTREE_GUIDANCE,
                    crate::sibling::CLAUDE_SPAWN_GUIDANCE,
                    crate::open_files::CLAUDE_OPEN_GUIDANCE,
                ]
                .join("\n\n"),
            ])
            .collect()
    }

    #[test]
    fn claude_transcript_lookup_searches_every_project_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        for slug in ["-w-issue-68", "-w-root-branch-switcher"] {
            std::fs::create_dir_all(projects.join(slug)).unwrap();
        }
        std::fs::write(projects.join("-w-issue-68").join("sid-1.jsonl"), "{}\n").unwrap();
        let missing = tmp.path().join("no-such-config").join("projects");
        let roots = [missing.clone(), projects];
        // Found under the slug the session began in, whichever checkout
        // resumes it (a relocated session).
        assert_eq!(claude_transcript_exists(&roots, "sid-1"), Some(true));
        // A CLI nobody prompted: an id, and no file behind it.
        assert_eq!(claude_transcript_exists(&roots, "sid-2"), Some(false));
        // An id that tries to be a path names nothing.
        assert_eq!(
            claude_transcript_exists(&roots, "../-w-issue-68/sid-1"),
            Some(false)
        );
        // Nothing readable: no verdict, so the resume is still tried.
        assert_eq!(claude_transcript_exists(&[missing], "sid-1"), None);
    }

    #[test]
    fn spawn_command_per_kind_resume_shapes() {
        // Fresh sessions: bare CLI (Claude plus its system-prompt guidance).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, None, None, None),
            (
                "claude".into(),
                guided("--append-system-prompt", &[]),
                false
            )
        );
        // Codex/cursor always run in skip-permissions mode.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, None, None, None, None),
            ("codex".into(), vec!["--yolo".to_string()], false)
        );
        // Cursor's agent CLI is `cursor-agent`, not `cursor` (the editor).
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, None, None, None, None),
            ("cursor-agent".into(), vec!["--force".to_string()], false)
        );
        // OpenCode boots bare like Codex (no system-prompt flag) and with
        // no permission flag (its prompts drive NEEDS FEEDBACK through the
        // managed plugin); a stored id resumes by `--session`.
        assert_eq!(
            agent_spawn_command(AgentKind::OpenCode, None, None, None, None),
            ("opencode".into(), vec![], false)
        );
        assert_eq!(
            agent_spawn_command(AgentKind::OpenCode, Some("ses_9"), None, None, None),
            (
                "opencode".into(),
                vec!["--session".to_string(), "ses_9".to_string()],
                true
            )
        );
        // Claude resumes with a flag; codex with a subcommand (order matters).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, Some("sid-1"), None, None, None),
            (
                "claude".into(),
                guided("--append-system-prompt", &["--resume", "sid-1"]),
                true
            )
        );
        // Skip-permissions flags trail the resume args. A codex resume is
        // told its checkout (`--cd`): without it codex reopens the session
        // in the directory its transcript recorded, which for a relocated
        // session is the old one.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, Some("sid-2"), None, None, None),
            (
                "codex".into(),
                vec![
                    "resume".to_string(),
                    "sid-2".to_string(),
                    "--cd".to_string(),
                    TEST_CWD.to_string(),
                    "--yolo".to_string()
                ],
                true
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, Some("sid-3"), None, None, None),
            (
                "cursor-agent".into(),
                vec![
                    "--resume".to_string(),
                    "sid-3".to_string(),
                    "--force".to_string()
                ],
                true
            )
        );
        // Override wins for both kinds and never gets resume args.
        assert_eq!(
            agent_spawn_command(
                AgentKind::Claude,
                Some("sid"),
                None,
                None,
                Some("/bin/sh -i")
            ),
            ("/bin/sh".into(), vec!["-i".to_string()], false)
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, Some("sid"), None, None, Some("/bin/sh")),
            ("/bin/sh".into(), vec![], false)
        );
    }

    #[test]
    fn spawn_command_model_and_effort_flags() {
        // Claude gets --model/--effort; either alone works.
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, Some("opus"), Some("high"), None),
            (
                "claude".into(),
                guided(
                    "--append-system-prompt",
                    &["--model", "opus", "--effort", "high"]
                ),
                false
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, None, Some("max"), None),
            (
                "claude".into(),
                guided("--append-system-prompt", &["--effort", "max"]),
                false
            )
        );
        // Codex takes --model plus a config override for effort, after --yolo.
        assert_eq!(
            agent_spawn_command(AgentKind::Codex, None, Some("gpt-5.5"), Some("high"), None),
            (
                "codex".into(),
                vec![
                    "--yolo".to_string(),
                    "--model".to_string(),
                    "gpt-5.5".to_string(),
                    "-c".to_string(),
                    "model_reasoning_effort=high".to_string()
                ],
                false
            )
        );
        // OpenCode takes a `provider/model` id verbatim and has no effort
        // flag: effort is dropped, never sent.
        assert_eq!(
            agent_spawn_command(
                AgentKind::OpenCode,
                None,
                Some("anthropic/claude-sonnet-5"),
                Some("high"),
                None
            ),
            (
                "opencode".into(),
                vec![
                    "--model".to_string(),
                    "anthropic/claude-sonnet-5".to_string()
                ],
                false
            )
        );
        // Resume keeps the model/effort flags (a fallback fresh spawn needs
        // them, and the CLIs accept them alongside resume).
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, Some("sid"), Some("sonnet"), None, None),
            (
                "claude".into(),
                guided(
                    "--append-system-prompt",
                    &["--resume", "sid", "--model", "sonnet"]
                ),
                true
            )
        );
        // Cursor joins family and effort into the CLI's one flat id; a
        // family alone is passed bare, an effort alone has nothing to
        // join and is dropped.
        assert_eq!(
            agent_spawn_command(
                AgentKind::Cursor,
                None,
                Some("claude-opus-5-thinking"),
                Some("high"),
                None
            ),
            (
                "cursor-agent".into(),
                vec![
                    "--force".to_string(),
                    "--model".to_string(),
                    "claude-opus-5-thinking-high".to_string()
                ],
                false
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, Some("sid"), Some("auto"), None, None),
            (
                "cursor-agent".into(),
                vec![
                    "--resume".to_string(),
                    "sid".to_string(),
                    "--force".to_string(),
                    "--model".to_string(),
                    "auto".to_string()
                ],
                true
            )
        );
        assert_eq!(
            agent_spawn_command(AgentKind::Cursor, None, None, Some("high"), None),
            ("cursor-agent".into(), vec!["--force".to_string()], false)
        );
        // Override still wins over everything.
        assert_eq!(
            agent_spawn_command(AgentKind::Claude, None, Some("opus"), None, Some("/bin/sh")),
            ("/bin/sh".into(), vec![], false)
        );
    }

    /// A `harnesses` map entry for an id no built-in holds — `agy`, a CLI
    /// by the name of its program — carrying `over`.
    fn agy(
        over: orion_core::harness::HarnessOverride,
    ) -> std::collections::BTreeMap<String, orion_core::harness::HarnessOverride> {
        std::collections::BTreeMap::from([("agy".to_string(), over)])
    }

    #[test]
    fn custom_spawn_runs_the_entrys_program_with_a_model_flag() {
        // A map-only entry launches with its own program and model flag; a
        // stored session id is ignored (fresh boot) and effort is dropped.
        let all = harness_registry_in(&agy(Default::default()), &[]);
        let harness = test_custom_harness(&all, "agy");
        let (program, args, resumed) = agent_spawn_command_with(
            &harness,
            Some("sid-1"),
            Some(Path::new(TEST_CWD)),
            Some("big-1"),
            Some("high"),
            None,
            Some("do it"),
            None,
            true,
        );
        assert_eq!(program, "agy");
        assert_eq!(args, vec!["--model", "big-1", "do it"]);
        assert!(!resumed);
    }

    #[test]
    fn harness_resolve_names_missing_unknown_and_broken_entries() {
        use orion_core::harness::{Clearable, HarnessOverride};
        let all = harness_registry_in(&agy(Default::default()), &[]);
        // Built-ins resolve by kind, whatever the id says.
        assert_eq!(
            resolve_harness_in(AgentKind::Claude, None, &all)
                .unwrap()
                .program,
            "claude"
        );
        // Custom without an id, or with an unknown one, refuses.
        let err = resolve_harness_in(AgentKind::Custom, None, &all).unwrap_err();
        assert!(err.to_string().contains("registry id"), "{err}");
        let err = resolve_harness_in(AgentKind::Custom, Some("gone"), &all).unwrap_err();
        assert!(err.to_string().contains("no longer defined"), "{err}");
        // A broken entry refuses with its reason, never launches.
        let broken = agy(HarnessOverride {
            env: Clearable::Set(std::collections::BTreeMap::from([(
                "MY-VAR".to_string(),
                "x".to_string(),
            )])),
            ..HarnessOverride::default()
        });
        let all = harness_registry_in(&broken, &[]);
        let err = resolve_harness_in(AgentKind::Custom, Some("agy"), &all).unwrap_err();
        assert!(err.to_string().contains("MY-VAR"), "{err}");
        // The usable entry resolves, disabled or not: a harness switched
        // off after its sessions were created keeps running them.
        let off = agy(HarnessOverride {
            enabled: Some(false),
            ..HarnessOverride::default()
        });
        let all = harness_registry_in(&off, &[]);
        assert_eq!(
            resolve_harness_in(AgentKind::Custom, Some("agy"), &all)
                .unwrap()
                .program,
            "agy"
        );
    }

    /// A `claude_accounts` entry launches and resumes exactly as built-in
    /// Claude does — its own `--resume`, its model and effort flags — in
    /// its own config dir, which the spawn exports after the profile.
    #[test]
    fn a_claude_account_launches_as_claude_in_its_own_dir() {
        let account = orion_core::claude_account::ClaudeAccount {
            id: "claude-2".into(),
            config_dir: "/home/me/.claude-2".into(),
            name: String::new(),
            enabled: true,
        };
        let all = harness_registry_in(&std::collections::BTreeMap::new(), &[account]);
        let harness = resolve_harness_in(AgentKind::Claude, Some("claude-2"), &all).unwrap();
        assert_eq!(harness.id, "claude-2");
        assert_eq!(harness.program, "claude");
        assert_eq!(
            harness.launch_env(),
            [(
                "CLAUDE_CONFIG_DIR".to_string(),
                "/home/me/.claude-2".to_string()
            )]
        );
        let (program, args, resumed) = agent_spawn_command_with(
            &harness,
            Some("sid-2"),
            Some(Path::new(TEST_CWD)),
            Some("opus"),
            Some("high"),
            None,
            None,
            None,
            false,
        );
        assert_eq!(program, "claude");
        assert!(resumed);
        assert_eq!(
            args,
            ["--resume", "sid-2", "--model", "opus", "--effort", "high"]
        );
    }

    #[test]
    fn grok_spawn_uses_verified_flags_and_preserves_prompt_boundaries() {
        let grok = orion_core::harness::builtin("grok").unwrap();
        assert_eq!(
            agent_spawn_command_with(&grok, None, None, None, None, None, None, None, false),
            ("grok".into(), vec![], false)
        );
        let (program, args, resumed) = agent_spawn_command_with(
            &grok,
            Some("session-id"),
            Some(Path::new(TEST_CWD)),
            Some("model-id"),
            Some("high"),
            None,
            Some("fix the bug; keep this as one argument"),
            Some("Review only PR #42"),
            false,
        );
        assert_eq!(program, "grok");
        assert!(resumed);
        assert_eq!(
            args,
            vec![
                "--resume",
                "session-id",
                "--model",
                "model-id",
                "--effort",
                "high",
                "--rules",
                "Review only PR #42",
                "fix the bug; keep this as one argument",
            ]
        );
        assert_eq!(grok.hook_dialect(), Some(AgentKind::Grok));
        assert_eq!(grok.permissions_flag, None);
    }

    /// Grok's AUTO-TITLE rides `--rules`: on a fresh launch the rule goes
    /// ahead of the launch's own text (a PR SESSION's), a resume carries
    /// only the latter, and no other dialect gets it — theirs comes back
    /// from a hook, a project rule or a plugin.
    #[test]
    fn grok_fresh_launches_carry_the_title_rule_in_rules() {
        let grok = orion_core::harness::builtin("grok").unwrap();
        let title = hooks::auto_title_rule();
        assert_eq!(
            launch_system_prompt(&grok, false, None),
            Some(title.clone())
        );
        assert_eq!(
            launch_system_prompt(&grok, false, Some("Review only PR #42")),
            Some(format!("{title}\n\nReview only PR #42"))
        );
        assert_eq!(launch_system_prompt(&grok, true, None), None);
        assert_eq!(
            launch_system_prompt(&grok, true, Some("Review only PR #42")).as_deref(),
            Some("Review only PR #42")
        );
        for id in ["claude", "codex", "cursor", "opencode"] {
            let other = orion_core::harness::builtin(id).unwrap();
            assert_eq!(launch_system_prompt(&other, false, None), None, "{id}");
        }
        // The rule reaches the argv as part of the one `--rules` value.
        let (_, args, _) = agent_spawn_command_with(
            &grok,
            None,
            Some(Path::new(TEST_CWD)),
            None,
            None,
            None,
            None,
            launch_system_prompt(&grok, false, None).as_deref(),
            true,
        );
        assert_eq!(args[0], "--rules");
        assert!(args[1].contains("orion rename"), "{}", args[1]);
        assert!(args[1].ends_with(&title), "after the guidance: {}", args[1]);
    }

    #[test]
    fn spawn_command_initial_prompt_is_the_trailing_positional_argument() {
        let all = test_registry();
        let claude = test_harness(&all, AgentKind::Claude);
        let codex = test_harness(&all, AgentKind::Codex);
        let cursor = test_harness(&all, AgentKind::Cursor);
        // The relocation notice trails everything, guidance included.
        let (_, args, resumed) = agent_spawn_command_with(
            &claude,
            Some("sid"),
            Some(Path::new(TEST_CWD)),
            Some("opus"),
            None,
            None,
            Some("carry on"),
            None,
            true,
        );
        assert!(resumed);
        let mut expected = guided(
            "--append-system-prompt",
            &["--resume", "sid", "--model", "opus"],
        );
        expected.push("carry on".into());
        assert_eq!(args, expected);
        // Codex and cursor take it as their trailing positional too.
        assert_eq!(
            agent_spawn_command_with(
                &codex,
                Some("sid"),
                Some(Path::new(TEST_CWD)),
                None,
                None,
                None,
                Some("carry on"),
                None,
                true,
            )
            .1,
            vec!["resume", "sid", "--cd", TEST_CWD, "--yolo", "carry on"]
        );
        assert_eq!(
            agent_spawn_command_with(
                &cursor,
                Some("sid"),
                Some(Path::new(TEST_CWD)),
                None,
                None,
                None,
                Some("carry on"),
                None,
                true,
            )
            .1,
            vec!["--resume", "sid", "--force", "carry on"]
        );
        // A fresh spawn with a starting prompt (an AGENT PRESET launch):
        // model, effort and system prompt all precede it.
        let mut expected = guided(
            "--append-system-prompt",
            &["--model", "opus", "--effort", "high"],
        );
        expected.push("fix auth".into());
        assert_eq!(
            agent_spawn_command_with(
                &claude,
                None,
                Some(Path::new(TEST_CWD)),
                Some("opus"),
                Some("high"),
                None,
                Some("fix auth"),
                None,
                true,
            )
            .1,
            expected
        );
        assert_eq!(
            agent_spawn_command_with(
                &codex,
                None,
                Some(Path::new(TEST_CWD)),
                Some("gpt-5.5"),
                Some("high"),
                None,
                Some("fix auth"),
                None,
                true,
            )
            .1,
            vec![
                "--yolo",
                "--model",
                "gpt-5.5",
                "-c",
                "model_reasoning_effort=high",
                "fix auth"
            ]
        );
        assert_eq!(
            agent_spawn_command_with(
                &cursor,
                None,
                Some(Path::new(TEST_CWD)),
                None,
                None,
                None,
                Some("fix auth"),
                None,
                true,
            )
            .1,
            vec!["--force", "fix auth"]
        );
        // An override is verbatim: no guidance, no prompt.
        assert_eq!(
            agent_spawn_command_with(
                &claude,
                None,
                Some(Path::new(TEST_CWD)),
                None,
                None,
                Some("/bin/sh -i"),
                Some("carry on"),
                None,
                true,
            ),
            ("/bin/sh".into(), vec!["-i".to_string()], false)
        );
    }

    /// OpenCode's positional is the project path, so a starting prompt —
    /// an AGENT PRESET's task, a PR SESSION's rule — rides `--prompt`
    /// after the model flag, where every other CLI takes a trailing
    /// positional; the field changes nothing for them.
    #[test]
    fn opencode_first_prompt_rides_its_flag() {
        let all = test_registry();
        let opencode = test_harness(&all, AgentKind::OpenCode);
        let (program, args, resumed) = agent_spawn_command_with(
            &opencode,
            None,
            Some(Path::new(TEST_CWD)),
            Some("opencode/big-pickle"),
            None,
            None,
            Some("Fix auth"),
            None,
            true,
        );
        assert_eq!(program, "opencode");
        assert_eq!(
            args,
            ["--model", "opencode/big-pickle", "--prompt", "Fix auth"]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert!(!resumed);
        let claude = test_harness(&all, AgentKind::Claude);
        let (_, args, _) = agent_spawn_command_with(
            &claude,
            None,
            Some(Path::new(TEST_CWD)),
            None,
            None,
            None,
            Some("Fix auth"),
            None,
            false,
        );
        assert_eq!(args, vec!["Fix auth".to_string()]);
    }

    /// The relocation notice reaches the CLIs whose resume submits a
    /// trailing prompt — Claude and codex — and names the checkout;
    /// cursor's is unverified and OpenCode's `--session <id> --prompt` is
    /// not submitted (verified on opencode 1.18.32), so their relocated
    /// sessions reopen silent. (Codex was gated out until #39: `codex
    /// resume <id> --yolo` sat at Ready in the worktree until the user
    /// typed "continue".)
    #[test]
    fn relocation_prompt_reaches_every_kind_but_cursor_and_opencode() {
        let feat = Worktree {
            id: WorktreeId("feat".into()),
            project_id: ProjectId("p".into()),
            path: "/orion-test/p-feat".into(),
            branch: "feat".into(),
            is_main: false,
            sort_order: 0,
        };
        let all = test_registry();
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            let harness = test_harness(&all, kind);
            assert!(harness.relocation_prompt, "{kind:?} maps the notice");
            let prompt = relocation_prompt(harness.relocation_prompt, &feat)
                .unwrap_or_else(|| panic!("{kind:?}"));
            assert!(prompt.contains("`feat`"), "{kind:?}: {prompt}");
            assert!(prompt.contains("/orion-test/p-feat"), "{kind:?}: {prompt}");
            assert!(prompt.contains("Continue the user's most recent request"));
        }
        let cursor = test_harness(&all, AgentKind::Cursor);
        assert_eq!(relocation_prompt(cursor.relocation_prompt, &feat), None);
        let opencode = test_harness(&all, AgentKind::OpenCode);
        assert_eq!(relocation_prompt(opencode.relocation_prompt, &feat), None);

        // And the codex respawn it feeds: resumed, re-rooted in the
        // worktree, and opening on the notice.
        let codex = test_harness(&all, AgentKind::Codex);
        let notice = relocation_prompt(codex.relocation_prompt, &feat).unwrap();
        let (program, args, resumed) = agent_spawn_command_with(
            &codex,
            Some("sid"),
            Some(&feat.path),
            None,
            None,
            None,
            Some(&notice),
            None,
            true,
        );
        assert_eq!(program, "codex");
        assert!(resumed);
        assert_eq!(
            args,
            vec![
                "resume",
                "sid",
                "--cd",
                "/orion-test/p-feat",
                "--yolo",
                notice.as_str()
            ]
        );
    }

    #[test]
    fn spawn_command_keeps_pr_scope_and_url_in_claudes_system_prompt() {
        let pr_url = "https://github.com/oliverkidd/orion/pull/42";
        let pr_prompt = crate::pr_scope::rule(&crate::pr_scope::PrScope {
            url: pr_url,
            worktree: Path::new("/w/orion-worktrees/fix"),
            branch: "fix",
            root: Some(Path::new("/w/orion")),
        });
        let all = test_registry();
        let claude = test_harness(&all, AgentKind::Claude);
        let (_, args, resumed) = agent_spawn_command_with(
            &claude,
            Some("sid"),
            Some(Path::new(TEST_CWD)),
            None,
            None,
            None,
            None,
            Some(&pr_prompt),
            true,
        );
        assert!(resumed);
        let prompts = args
            .windows(2)
            .filter(|pair| pair[0] == "--append-system-prompt")
            .map(|pair| pair[1].as_str())
            .collect::<Vec<_>>();
        assert_eq!(prompts.len(), 1, "Claude gets one composed system prompt");
        assert!(prompts[0].contains(CLAUDE_WORKTREE_GUIDANCE));
        assert!(prompts[0].contains(crate::sibling::CLAUDE_SPAWN_GUIDANCE));
        assert!(prompts[0].contains(crate::open_files::CLAUDE_OPEN_GUIDANCE));
        assert!(prompts[0].contains("All work in this session must be scoped"));
        assert!(prompts[0].contains(pr_url));
        assert!(prompts[0].contains("/w/orion-worktrees/fix"));
    }

    #[test]
    fn starting_prompt_is_trimmed_and_checked() {
        assert_eq!(
            validate_starting_prompt("  fix auth  ").unwrap(),
            "fix auth"
        );
        // Newlines are part of a multi-row task; only the ends go.
        assert_eq!(
            validate_starting_prompt("\nline one\nline two\n").unwrap(),
            "line one\nline two"
        );
        for bad in ["", "   ", "\n"] {
            assert!(validate_starting_prompt(bad).is_err(), "{bad:?}");
        }
        // A NUL would truncate the login shell's -c string.
        assert!(validate_starting_prompt("fix\0auth").is_err());
        // No length cap of orion's own: the OS's ARG_MAX is the only one.
        assert!(validate_starting_prompt(&"x".repeat(512 * 1024)).is_ok());
    }

    /// What every agent launch runs once the login shell's files have run,
    /// ahead of the command itself.
    const PANE_ENV: &str =
        "unset NO_COLOR FORCE_COLOR; export TERM=xterm-256color COLORTERM=truecolor;";

    #[test]
    fn cli_probe_line_looks_the_program_up_verbatim() {
        // Built-ins read exactly as before the registry.
        assert_eq!(
            cli_probe_line("claude"),
            "command -v 'claude' >/dev/null 2>&1"
        );
        assert_eq!(
            cli_probe_line("cursor-agent"),
            "command -v 'cursor-agent' >/dev/null 2>&1"
        );
        // A config-named program is any string. A quote in it stays
        // inside the word instead of closing it and opening a command.
        let hostile = "x'; echo INJECTED; echo '";
        let line = cli_probe_line(hostile);
        assert_eq!(
            line,
            "command -v 'x'\\''; echo INJECTED; echo '\\''' >/dev/null 2>&1"
        );
        // And a real shell agrees: the lookup fails quietly, nothing runs.
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &line])
            .output()
            .expect("/bin/sh");
        assert!(!out.status.success(), "no such program");
        assert!(
            out.stdout.is_empty(),
            "{:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    #[test]
    fn login_shell_wrap_quotes_args_and_leaves_the_command_word_bare() {
        let (program, args) = login_shell_wrap(
            "/bin/zsh",
            &[],
            "claude",
            &["--resume".to_string(), "sid-1".to_string()],
        );
        assert_eq!(program, "/bin/zsh");
        assert_eq!(
            args,
            vec![
                "-l",
                "-i",
                "-c",
                &format!("{PANE_ENV} claude '--resume' 'sid-1'")
            ]
        );
        // Single quotes in an arg survive the wrapping.
        let (_, args) = login_shell_wrap("/bin/zsh", &[], "echo", &["it's".to_string()]);
        assert_eq!(args[3], format!(r"{PANE_ENV} echo 'it'\''s'"));
        // A command word that isn't a plain name is quoted like an argument.
        let (_, args) = login_shell_wrap("/bin/zsh", &[], "my tool", &[]);
        assert_eq!(args[3], format!("{PANE_ENV} 'my tool'"));
    }

    /// The command word resolves through the shell, so an alias or function
    /// from the rc files takes precedence over the binary on PATH — the
    /// reason a launch goes through the login shell at all. A function
    /// stands in for the alias: every POSIX sh honours one in a `-c` string,
    /// and the old `exec env` form bypassed both alike.
    #[test]
    fn login_shell_wrap_lets_the_shell_resolve_the_command() {
        let (_, args) = login_shell_wrap(
            "/bin/sh",
            &[],
            "claude",
            &["--resume".to_string(), "sid-1".to_string()],
        );
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "claude() {{ printf 'routed %s' \"$*\"; }}; {}",
                args[3]
            ))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "routed --resume sid-1",
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The shape of the report itself: zsh, `-l -i -c`, and an alias in
    /// `.zshrc` that reroutes `claude`. Skipped where there is no zsh.
    #[test]
    fn login_shell_wrap_honours_a_zshrc_alias() {
        use std::os::unix::process::CommandExt;
        if std::process::Command::new("zsh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".zshrc"), "alias claude='echo routed'\n").unwrap();
        let (program, args) = login_shell_wrap(
            "zsh",
            &[],
            "claude",
            &["--resume".to_string(), "sid-1".to_string()],
        );
        let mut cmd = std::process::Command::new(program);
        cmd.args(&args)
            .env("ZDOTDIR", home.path())
            .stdin(std::process::Stdio::null());
        // Own session, as the daemon's CLI probe does: an interactive zsh
        // must not make itself the foreground of the terminal running the
        // tests.
        unsafe {
            cmd.pre_exec(own_session);
        }
        let out = cmd.output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            "routed --resume sid-1",
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The command line's `env` really does undo a profile's colour
    /// overrides and restate the pane: run it through a plain `sh -c` with
    /// `NO_COLOR` and a foreign `TERM` already exported, as a `.profile`
    /// would leave them, and read back what the program sees.
    #[test]
    fn login_shell_wrap_restates_the_pane_after_the_profile() {
        let (_, args) = login_shell_wrap(
            "/bin/sh",
            &[],
            "sh",
            &[
                "-c".to_string(),
                r#"printf '%s|%s|%s|%s' "$TERM" "$COLORTERM" "${NO_COLOR-unset}" "${FORCE_COLOR-unset}""#
                    .to_string(),
            ],
        );
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&args[3])
            .env("NO_COLOR", "1")
            .env("FORCE_COLOR", "0")
            .env("TERM", "foot")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "xterm-256color|truecolor|unset|unset",
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub(crate) fn test_daemon() -> Arc<Daemon> {
        let store = Arc::new(Store::open_in_memory().unwrap());
        Daemon::new(
            store,
            HookEnv {
                port: 0,
                token: String::new(),
            },
        )
    }

    #[test]
    fn claude_projects_dirs_follow_the_transcripts_hooks_reported() {
        // An agent's shell may point CLAUDE_CONFIG_DIR somewhere the daemon's
        // own env never heard of; its hooks name the real transcript.
        let daemon = test_daemon();
        let reported = crate::session_title::TranscriptRef::from_payload(
            Some("/cfg/alt/projects/-w-feat/sid-9.jsonl"),
            Some("sid-9"),
        )
        .unwrap();
        daemon
            .transcripts
            .lock()
            .unwrap()
            .insert(AgentId("a1".into()), reported);
        let dirs = daemon.claude_projects_dirs();
        assert!(
            dirs.contains(&PathBuf::from("/cfg/alt/projects")),
            "{dirs:?}"
        );
    }

    /// A harness's `env` is exported after the prelude — after the
    /// profile — so a `CLAUDE_CONFIG_DIR` the user's rc files set cannot
    /// undo the one a second account's harness pins, and a value with a
    /// quote in it arrives whole.
    #[test]
    fn login_shell_wrap_exports_the_harness_env_after_the_profile() {
        let (_, args) = login_shell_wrap(
            "/bin/sh",
            &[
                ("CLAUDE_CONFIG_DIR".into(), "/home/me/.claude-b".into()),
                ("ORION_TEST_QUOTED".into(), "it's here".into()),
            ],
            "sh",
            &[
                "-c".to_string(),
                r#"printf '%s|%s|%s' "$CLAUDE_CONFIG_DIR" "$ORION_TEST_QUOTED" "$TERM""#
                    .to_string(),
            ],
        );
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&args[3])
            .env("CLAUDE_CONFIG_DIR", "/home/me/.claude")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "/home/me/.claude-b|it's here|xterm-256color",
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // No env, no export: the line is the CLI's alone.
        let (_, args) = login_shell_wrap("/bin/sh", &[], "claude", &["--resume".into()]);
        assert_eq!(args[3], format!("{PANE_ENV} claude '--resume'"));
    }

    /// The resume safeguards look for a Claude session's transcript where
    /// its harness keeps them: a second account's own config dir when its
    /// `env` pins one, built-in Claude's usual places otherwise — and
    /// nowhere for a wrapper whose config dir only it knows, or a harness
    /// off Claude's dialect.
    #[test]
    fn claude_projects_roots_follow_the_harness_env() {
        let daemon = test_daemon();
        let agent = |kind: AgentKind, custom: Option<&str>| Agent {
            id: AgentId("a1".into()),
            worktree_id: WorktreeId("w".into()),
            name: "a".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            custom_harness: custom.map(str::to_string),
            model: None,
            effort: None,
            session_id: Some("sid".into()),
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: None,
        };
        let claude = orion_core::harness::builtin("claude").unwrap();
        let builtin = agent(AgentKind::Claude, None);
        assert_eq!(
            daemon.claude_projects_roots(&builtin, &claude),
            Some(daemon.claude_projects_dirs())
        );
        let mut wrapper = claude.clone();
        wrapper.id = "claude-b".into();
        wrapper.kind = AgentKind::Custom;
        wrapper.program = "/home/me/bin/claude-b".into();
        let custom = agent(AgentKind::Custom, Some("claude-b"));
        assert_eq!(daemon.claude_projects_roots(&custom, &wrapper), None);
        let mut pinned = wrapper.clone();
        pinned
            .env
            .insert("CLAUDE_CONFIG_DIR".into(), "/home/me/.claude-b".into());
        assert_eq!(
            daemon.claude_projects_roots(&custom, &pinned),
            Some(vec![PathBuf::from("/home/me/.claude-b/projects")])
        );
        // A Claude account: Claude with its id, in its own dir alone.
        let account = orion_core::claude_account::ClaudeAccount {
            id: "claude-b".into(),
            config_dir: "/home/me/.claude-b".into(),
            name: String::new(),
            enabled: true,
        }
        .descriptor(&claude);
        assert_eq!(
            daemon.claude_projects_roots(&agent(AgentKind::Claude, Some("claude-b")), &account),
            Some(vec![PathBuf::from("/home/me/.claude-b/projects")])
        );
        let codex = orion_core::harness::builtin("codex").unwrap();
        assert_eq!(
            daemon.claude_projects_roots(&agent(AgentKind::Codex, None), &codex),
            None
        );
    }

    /// A project whose main checkout is a fresh directory, registered with
    /// `daemon`, for the RUN TERMINAL tests.
    fn run_worktree(daemon: &Daemon) -> (tempfile::TempDir, Worktree) {
        let dir = tempfile::tempdir().unwrap();
        let project = Project {
            id: ProjectId::generate(),
            name: "demo".into(),
            repo_path: dir.path().to_path_buf(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id,
            path: dir.path().to_path_buf(),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        };
        daemon.store.insert_worktree(&worktree).unwrap();
        (dir, worktree)
    }

    /// A config whose project at `repo` has `run` for its RUN COMMAND.
    fn run_config(repo: &Path, run: &str) -> crate::config::Config {
        project_config(repo, serde_json::json!({ "run_command": run }))
    }

    /// A config whose project at `repo` has `setup` for its SETUP COMMAND.
    fn setup_config(repo: &Path, setup: &str) -> crate::config::Config {
        project_config(repo, serde_json::json!({ "setup_command": setup }))
    }

    /// A config whose project at `repo` has these Projects-tab rows.
    fn project_config(repo: &Path, rows: serde_json::Value) -> crate::config::Config {
        let json = serde_json::json!({ "projects": { repo.display().to_string(): rows } });
        serde_json::from_value(json).unwrap()
    }

    /// A daemon restart (an upgrade, `orion kill`) takes every PTY: the
    /// rows the boot sweep finds cut off mid-turn come back on their own,
    /// working, instead of waiting for an attach and a typed "continue".
    #[tokio::test]
    async fn a_restart_resumes_the_agents_it_cut_off_mid_turn() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        // `/bin/cat` stands in for the CLI: spawned verbatim, it blocks on
        // the PTY instead of running anything.
        let _cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        let row = |name: &str, status, session_id: Option<&str>, archived| Agent {
            id: AgentId(format!("agent-{name}")),
            worktree_id: worktree.id.clone(),
            name: name.into(),
            status,
            archived,
            archived_at: 0,
            unseen: false,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: session_id.map(String::from),
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: None,
        };
        for agent in [
            row("working", AgentStatus::Running, Some("sid-1"), false),
            row("asking", AgentStatus::NeedsFeedback, Some("sid-2"), false),
            row("done", AgentStatus::Finished, Some("sid-3"), false),
            row("never-prompted", AgentStatus::Running, None, false),
            row("archived", AgentStatus::Running, Some("sid-4"), true),
        ] {
            daemon.store.insert_agent(&agent).unwrap();
        }
        let interrupted = daemon.store.sweep_disconnected().unwrap();

        for id in &interrupted {
            let worker = daemon.clone();
            let id = id.clone();
            tokio::task::spawn_blocking(move || worker.resume_interrupted_agent(&id).unwrap())
                .await
                .unwrap();
        }
        let alive =
            |name: &str| daemon.is_alive(&SessionRef::Agent(AgentId(format!("agent-{name}"))));
        let status = |name: &str| {
            daemon
                .store
                .get_agent(&AgentId(format!("agent-{name}")))
                .unwrap()
                .unwrap()
                .status
        };
        for name in ["working", "asking"] {
            assert!(alive(name), "{name} is resumed");
            assert_eq!(status(name), AgentStatus::Running, "{name} carries on");
        }
        // Seeded with the launch reprieve: the resumed CLI's startup
        // progress-clear does not green it out before the turn begins.
        let working = AgentId("agent-working".into());
        daemon.apply_hook_event(&working, HookEvent::Progress { busy: false }, None);
        assert_eq!(status("working"), AgentStatus::Running);

        // Idle at the restart: resumes on its next attach, as before.
        assert!(!alive("done"));
        assert_eq!(status("done"), AgentStatus::Finished);
        // No session to resume, or put away: nothing to bring back.
        assert!(!alive("never-prompted"));
        assert!(!alive("archived"));

        // An Attach that got there first is never doubled.
        assert!(!daemon.resume_interrupted_agent(&working).unwrap());
        daemon.kill_all();
        drop(dir);
    }

    /// A follow-up always lands: a live CLI has it typed and submitted, and
    /// one the IDLE REAPER put down is booted on it rather than left for a
    /// second Enter. An archived row is refused, as its attach is.
    #[tokio::test]
    async fn a_turn_is_typed_into_a_live_session_and_boots_a_stopped_one() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        // `/bin/cat` stands in for the CLI: what is submitted comes back.
        let _cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        seed_agent(&daemon, "follow", &worktree.id.0, Some("sid-1"));
        let id = AgentId("follow".into());
        let sref = SessionRef::Agent(id.clone());

        daemon.send_turn(&id, "first turn", 80, 24).unwrap();
        assert!(
            daemon.is_alive(&sref),
            "a stopped session is booted on its turn"
        );

        daemon.send_turn(&id, "second turn", 80, 24).unwrap();
        let session = daemon.session(&sref).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        // Typed (the tty's echo), then submitted (cat's line back).
        while String::from_utf8_lossy(&session.snapshot(None).1)
            .matches("second turn")
            .count()
            < 2
        {
            assert!(Instant::now() < deadline, "the turn never went in");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        daemon.kill_all();
        daemon.store.set_agent_archived(&id, true).unwrap();
        let err = daemon.send_turn(&id, "too late", 80, 24).unwrap_err();
        assert!(err.to_string().contains("archived"), "{err}");
        drop(dir);
    }

    #[tokio::test]
    async fn run_needs_a_run_command_starts_once_and_stops() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);

        let missing = daemon
            .start_run_with(&worktree.id, &crate::config::Config::default())
            .unwrap_err();
        assert!(
            missing.to_string().contains("Settings → Projects"),
            "names where to set one: {missing}"
        );
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());

        let cfg = run_config(dir.path(), "sleep 30");
        let EntityId::Terminal(id) = daemon.start_run_with(&worktree.id, &cfg).unwrap() else {
            panic!("a run lives in a terminal");
        };
        let sref = SessionRef::Terminal(id.clone());
        assert!(daemon.is_alive(&sref));
        let row = daemon.store.get_terminal(&id).unwrap().unwrap();
        assert_eq!(row.run_command.as_deref(), Some("sleep 30"));

        // A second press while it runs starts nothing new.
        assert_eq!(
            daemon.start_run_with(&worktree.id, &cfg).unwrap(),
            EntityId::Terminal(id.clone())
        );
        assert_eq!(
            daemon.store.run_terminals_in(&worktree.id).unwrap().len(),
            1
        );

        // A stop is a ^C, a second one the kill.
        daemon.stop_run(&worktree.id).unwrap();
        daemon.stop_run(&worktree.id).unwrap();
        assert!(!daemon.is_alive(&sref));
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());
        // Nothing left to stop is not an error.
        daemon.stop_run(&worktree.id).unwrap();
    }

    /// Start `run` as `worktree`'s RUN COMMAND and wait for it to print
    /// `ready`: its terminal's id.
    async fn start_run_until(
        daemon: &Arc<Daemon>,
        worktree: &WorktreeId,
        cfg: &crate::config::Config,
        ready: &str,
    ) -> TerminalId {
        let EntityId::Terminal(id) = daemon.start_run_with(worktree, cfg).unwrap() else {
            panic!("a run lives in a terminal");
        };
        let session = daemon.session(&SessionRef::Terminal(id.clone())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !String::from_utf8_lossy(&session.snapshot(None).1).contains(ready) {
            assert!(Instant::now() < deadline, "the run never printed {ready:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        id
    }

    /// The next broadcast about terminal `id`: its upserted row, or None
    /// for its removal.
    async fn next_terminal_event(
        events: &mut broadcast::Receiver<ServerEvent>,
        id: &TerminalId,
    ) -> Option<TerminalTab> {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match events.recv().await.unwrap() {
                    ServerEvent::EntityUpserted {
                        entity: Entity::Terminal(t),
                    } if &t.id == id => return Some(t),
                    ServerEvent::EntityRemoved {
                        id: EntityId::Terminal(gone),
                    } if &gone == id => return None,
                    _ => {}
                }
            }
        })
        .await
        .expect("no word about the run terminal")
    }

    /// A stop's `^C` is news to every client: the run's row carries the
    /// moment its kill lands, in the snapshot too, until the run is gone —
    /// and a stop that was asked for takes the row with it.
    #[tokio::test]
    async fn a_stopping_run_says_so_until_it_is_gone() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let cfg = run_config(
            dir.path(),
            "trap 'sleep 0.3; exit 0' INT; echo started; while :; do sleep 0.1; done",
        );
        let id = start_run_until(&daemon, &worktree.id, &cfg, "started").await;
        let mut events = daemon.events.subscribe();
        let before = orion_core::clock::now_ms();
        daemon.stop_run(&worktree.id).unwrap();

        let row = next_terminal_event(&mut events, &id).await.unwrap();
        assert!(row.alive, "still winding down");
        let until = row.run.stopping_until.expect("the stop is broadcast");
        let grace = RUN_STOP_GRACE.as_millis() as i64;
        assert!(
            (before + grace..=orion_core::clock::now_ms() + grace).contains(&until),
            "the kill is two minutes out: {until}"
        );
        let ServerEvent::Snapshot { terminals, .. } = daemon.snapshot().unwrap() else {
            panic!("a snapshot");
        };
        let listed = terminals.iter().find(|t| t.id == id).unwrap();
        assert_eq!(listed.run.stopping_until, Some(until));

        assert!(
            next_terminal_event(&mut events, &id).await.is_none(),
            "the row goes once the run has wound down"
        );
        assert!(daemon.terminal_entity(&id).is_err());
    }

    /// A stop sends the run a ^C, so its own trap winds down what it
    /// started — and once it has, the run line goes: a stop asked for
    /// leaves nothing behind to read as an exit.
    #[tokio::test]
    async fn a_stop_you_asked_for_removes_the_run_line() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let marker = dir.path().join("wound-down");
        let cfg = run_config(
            dir.path(),
            &format!(
                "trap 'touch {}; exit 0' INT; echo started; while :; do sleep 0.1; done",
                marker.display()
            ),
        );
        let id = start_run_until(&daemon, &worktree.id, &cfg, "started").await;
        let mut events = daemon.events.subscribe();
        daemon.stop_run(&worktree.id).unwrap();
        while next_terminal_event(&mut events, &id).await.is_some() {}
        assert!(marker.exists(), "its trap ran first");
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());
        let sref = SessionRef::Terminal(id);
        assert_eq!(daemon.finished_run_exit(&sref), None, "nothing kept");
        // Nothing left to stop is not an error.
        daemon.stop_run(&worktree.id).unwrap();
    }

    /// The next row about terminal `id` that `pred` takes.
    async fn terminal_row_until(
        events: &mut broadcast::Receiver<ServerEvent>,
        id: &TerminalId,
        pred: impl Fn(&TerminalTab) -> bool,
    ) -> TerminalTab {
        loop {
            let row = next_terminal_event(events, id)
                .await
                .expect("the line stays");
            if pred(&row) {
                return row;
            }
        }
    }

    /// **Restart stack** is a stop, then a start once the run has gone:
    /// the run winds down under its `^C` as a stop's does, its row saying
    /// it is restarting, and the run comes back in the same row with the
    /// command read at the press. A second restart doesn't wait: it kills
    /// the run and starts it at once.
    #[tokio::test]
    async fn a_restart_stops_the_run_then_starts_it_in_its_row() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let winds_down = "trap 'sleep 0.3; exit 0' INT; echo first; while :; do sleep 0.1; done";
        let id = start_run_until(
            &daemon,
            &worktree.id,
            &run_config(dir.path(), winds_down),
            "first",
        )
        .await;
        let old = daemon.session(&SessionRef::Terminal(id.clone())).unwrap();
        let mut events = daemon.events.subscribe();
        let second = run_config(
            dir.path(),
            "trap '' INT; echo second; while :; do sleep 0.1; done",
        );
        daemon.restart_run_with(&worktree.id, &second).unwrap();

        let row = next_terminal_event(&mut events, &id).await.unwrap();
        assert!(row.alive && row.run.restarting, "{row:?}");
        assert!(row.run.stopping_until.is_some());
        let back = terminal_row_until(&mut events, &id, |t| {
            t.alive && t.run.stopping_until.is_none()
        })
        .await;
        assert!(!back.run.restarting);
        assert_eq!(
            back.run_command.as_deref(),
            Some(second.run_command(dir.path()).unwrap())
        );
        let new = daemon.session(&SessionRef::Terminal(id.clone())).unwrap();
        assert!(!Arc::ptr_eq(&old, &new), "a new run");
        assert_eq!(
            daemon.store.run_terminals_in(&worktree.id).unwrap().len(),
            1
        );

        // This one ignores its ^C: a second restart kills it and starts.
        daemon.restart_run_with(&worktree.id, &second).unwrap();
        daemon.restart_run_with(&worktree.id, &second).unwrap();
        let again = daemon.session(&SessionRef::Terminal(id.clone())).unwrap();
        assert!(!Arc::ptr_eq(&again, &new), "killed and started at once");
        assert_eq!(
            daemon.terminal_entity(&id).unwrap().run.stopping_until,
            None
        );

        daemon.stop_run(&worktree.id).unwrap();
        daemon.stop_run(&worktree.id).unwrap();
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());
    }

    /// A restart with nothing running is a start; with no run command it
    /// is the start's refusal, and a run that is up is left running.
    #[tokio::test]
    async fn a_restart_of_nothing_starts_and_one_with_no_command_stops_nothing() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let cfg = run_config(dir.path(), "echo up; while :; do sleep 0.1; done");
        daemon.restart_run_with(&worktree.id, &cfg).unwrap();
        let runs = daemon.store.run_terminals_in(&worktree.id).unwrap();
        assert_eq!(runs.len(), 1);
        let sref = SessionRef::Terminal(runs[0].id.clone());
        assert!(daemon.is_alive(&sref));

        let err = daemon
            .restart_run_with(&worktree.id, &crate::config::Config::default())
            .unwrap_err();
        assert!(err.to_string().contains("Settings → Projects"), "{err}");
        assert!(daemon.is_alive(&sref), "still running");
        assert_eq!(
            daemon
                .terminal_entity(&runs[0].id)
                .unwrap()
                .run
                .stopping_until,
            None
        );
        daemon.stop_run(&worktree.id).unwrap();
        daemon.stop_run(&worktree.id).unwrap();
    }

    /// The localhost ports a run prints ride its row — every client's run
    /// line — kept as the output scrolls on, only while it runs; a start
    /// of the run begins again from none. A plain shell is never read.
    #[tokio::test]
    async fn a_runs_ports_ride_its_row_until_it_starts_again() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let mut events = daemon.events.subscribe();
        let first = run_config(
            dir.path(),
            "echo '- Local: http://localhost:3000'; echo 'api http://127.0.0.1:4000/'; \
             seq 1 300; sleep 1",
        );
        let EntityId::Terminal(id) = daemon.start_run_with(&worktree.id, &first).unwrap() else {
            panic!("a run lives in a terminal");
        };
        let row = terminal_row_until(&mut events, &id, |t| t.run.ports.len() == 2).await;
        assert!(row.alive);
        assert_eq!(row.run.ports, vec![3000, 4000], "first seen first");
        let ended = terminal_row_until(&mut events, &id, |t| !t.alive).await;
        assert!(ended.run.ports.is_empty(), "nothing listens once it ended");

        let again = run_config(
            dir.path(),
            "echo 'http://localhost:5173/'; echo started; while :; do sleep 0.1; done",
        );
        assert_eq!(
            daemon.start_run_with(&worktree.id, &again).unwrap(),
            EntityId::Terminal(id.clone()),
            "the same line"
        );
        let row = terminal_row_until(&mut events, &id, |t| !t.run.ports.is_empty()).await;
        assert_eq!(row.run.ports, vec![5173], "the last run's ports are gone");
        daemon.stop_run(&worktree.id).unwrap();
        daemon.stop_run(&worktree.id).unwrap();

        let EntityId::Terminal(shell) = daemon.create_terminal(&worktree.id, None).unwrap() else {
            panic!("a terminal");
        };
        let session = daemon.session(&SessionRef::Terminal(shell)).unwrap();
        assert!(!session.scans_ports());
    }

    /// A run that ends with nobody stopping it keeps its line, and the
    /// line carries the code it exited with: 0 for `done`, anything else
    /// for `exited N`. Its next start clears it.
    #[tokio::test]
    async fn a_run_that_ends_on_its_own_says_how() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        for (line, code) in [("exit 0", 0), ("echo failing; exit 3", 3)] {
            let cfg = run_config(dir.path(), line);
            let mut events = daemon.events.subscribe();
            let EntityId::Terminal(id) = daemon.start_run_with(&worktree.id, &cfg).unwrap() else {
                panic!("a run lives in a terminal");
            };
            // A run this quick can be gone before its start is sent: the
            // row that carries the code is the word on how it ended.
            let ended = loop {
                let row = next_terminal_event(&mut events, &id)
                    .await
                    .expect("the line stays");
                if row.run.exit_code.is_some() {
                    break row;
                }
            };
            assert!(!ended.alive);
            assert_eq!(ended.run.exit_code, Some(code), "{line}");
            assert_eq!(ended.run.stopping_until, None);
            let ServerEvent::Snapshot { terminals, .. } = daemon.snapshot().unwrap() else {
                panic!("a snapshot");
            };
            assert_eq!(
                terminals.iter().find(|t| t.id == id).unwrap().run.exit_code,
                Some(code)
            );
        }
        assert_eq!(
            daemon.store.run_terminals_in(&worktree.id).unwrap().len(),
            1,
            "each start reused the line"
        );
    }

    /// The project's `run_command` setting (Settings → Projects) is what
    /// `r` runs; blank, or another project's, is no run command at all.
    #[tokio::test]
    async fn the_project_setting_is_the_run_command() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let EntityId::Terminal(id) = daemon
            .start_run_with(&worktree.id, &run_config(dir.path(), "sleep 31"))
            .unwrap()
        else {
            panic!("a run lives in a terminal");
        };
        let command = daemon.store.get_terminal(&id).unwrap().unwrap().run_command;
        assert_eq!(command.as_deref(), Some("sleep 31"));
        // The ^C, then the kill: gone before the next start.
        daemon.stop_run(&worktree.id).unwrap();
        daemon.stop_run(&worktree.id).unwrap();

        for cfg in [
            run_config(dir.path(), "   "),
            run_config(Path::new("/tmp/other"), "sleep 31"),
        ] {
            let err = daemon.start_run_with(&worktree.id, &cfg).unwrap_err();
            assert!(err.to_string().contains("Settings → Projects"), "{err}");
        }
    }

    /// A run that ends on its own keeps its output for the attach that
    /// comes to read it, and never runs again on that attach — only `r`
    /// starts a RUN COMMAND.
    #[tokio::test]
    async fn an_exited_run_replays_instead_of_running_again() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let cfg = run_config(dir.path(), "echo run-finished");
        let EntityId::Terminal(id) = daemon.start_run_with(&worktree.id, &cfg).unwrap() else {
            panic!("a run lives in a terminal");
        };
        let sref = SessionRef::Terminal(id.clone());
        let deadline = Instant::now() + Duration::from_secs(20);
        while daemon.finished_run_exit(&sref).is_none() {
            assert!(Instant::now() < deadline, "the run never exited");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!daemon.is_alive(&sref));

        let session = daemon.ensure_session(&sref, 80, 24).unwrap();
        assert!(!daemon.is_alive(&sref), "an attach spawned nothing");
        let (_, bytes) = session.snapshot(None);
        assert!(
            String::from_utf8_lossy(&bytes).contains("run-finished"),
            "the replay is the run's output"
        );

        // `r` again reuses the row.
        assert_eq!(
            daemon.start_run_with(&worktree.id, &cfg).unwrap(),
            EntityId::Terminal(id)
        );
        assert_eq!(
            daemon.store.run_terminals_in(&worktree.id).unwrap().len(),
            1
        );
        daemon.stop_run(&worktree.id).unwrap();
    }

    /// A SETUP COMMAND runs in the checkout in a `setup` row of its own —
    /// out to every client alive, never the worktree's run — and a clean
    /// exit takes the row with it.
    #[tokio::test]
    async fn a_setup_runs_in_its_own_row_and_a_clean_exit_removes_it() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let mut events = daemon.events.subscribe();
        let line = "sleep 0.3; echo ok > set-up";
        let id = daemon
            .start_setup(dir.path(), &worktree, &setup_config(dir.path(), line))
            .unwrap()
            .expect("a setup command makes a row");

        let row = daemon.store.get_terminal(&id).unwrap().unwrap();
        assert!(row.setup && !row.is_run());
        assert_eq!(row.name, "setup");
        assert_eq!(row.run_command.as_deref(), Some(line));
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());
        let sent = next_terminal_event(&mut events, &id)
            .await
            .expect("the row goes out");
        assert!(sent.alive && sent.setup, "setting up…");
        assert!(
            next_terminal_event(&mut events, &id).await.is_none(),
            "a clean exit takes the row"
        );
        assert!(daemon.store.get_terminal(&id).unwrap().is_none());
        assert!(dir.path().join("set-up").exists(), "it ran in the checkout");
    }

    /// A setup that fails keeps its row, carrying the code it exited with
    /// (`setup failed (3)`), and an attach replays it rather than running
    /// it again.
    #[tokio::test]
    async fn a_failed_setup_keeps_its_row_with_the_exit_code() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let mut events = daemon.events.subscribe();
        let cfg = setup_config(dir.path(), "echo no-npm; exit 3");
        let id = daemon
            .start_setup(dir.path(), &worktree, &cfg)
            .unwrap()
            .unwrap();
        let failed = terminal_row_until(&mut events, &id, |row| !row.alive).await;
        assert_eq!(failed.run.exit_code, Some(3));
        let sref = SessionRef::Terminal(id.clone());
        let session = daemon.ensure_session(&sref, 80, 24).unwrap();
        assert!(!daemon.is_alive(&sref), "an attach ran nothing");
        let (_, bytes) = session.snapshot(None);
        assert!(String::from_utf8_lossy(&bytes).contains("no-npm"));

        // Under a later DAEMON its output is gone, and the attach says so.
        daemon.finished_runs.lock().unwrap().clear();
        let Err(err) = daemon.ensure_session(&sref, 80, 24) else {
            panic!("an attach ran the setup again");
        };
        assert!(err.to_string().contains("setup"), "{err}");
    }

    /// A project with no setup command gets no row.
    #[tokio::test]
    async fn no_setup_command_is_no_setup_row() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        for cfg in [
            crate::config::Config::default(),
            setup_config(dir.path(), "  "),
            run_config(dir.path(), "just start"),
        ] {
            let started = daemon.start_setup(dir.path(), &worktree, &cfg).unwrap();
            assert_eq!(started, None);
        }
        assert_eq!(daemon.store.count_terminals(&worktree.id).unwrap(), 0);
    }

    /// A config whose project at `repo` has both a SETUP COMMAND and a
    /// RUN COMMAND.
    fn setup_and_run_config(repo: &Path, setup: &str, run: &str) -> crate::config::Config {
        project_config(
            repo,
            serde_json::json!({ "setup_command": setup, "run_command": run }),
        )
    }

    /// The next row any client is sent for a live RUN TERMINAL.
    async fn next_live_run(events: &mut broadcast::Receiver<ServerEvent>) -> TerminalTab {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let ServerEvent::EntityUpserted {
                    entity: Entity::Terminal(t),
                } = events.recv().await.unwrap()
                {
                    if t.is_run() && t.alive {
                        return t;
                    }
                }
            }
        })
        .await
        .expect("no run started")
    }

    /// **Start stack** on a worktree still setting up starts nothing yet:
    /// it waits on the setup's row, which says so to every client, and the
    /// run starts once the setup exits cleanly and its row has gone. A
    /// second press meanwhile waits too, and never starts a second run.
    #[tokio::test]
    async fn a_start_during_setup_waits_for_it_then_runs() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let cfg = setup_and_run_config(dir.path(), "sleep 0.5", "sleep 30");
        let mut events = daemon.events.subscribe();
        let setup = daemon
            .start_setup(dir.path(), &worktree, &cfg)
            .unwrap()
            .unwrap();

        for _ in 0..2 {
            assert_eq!(
                daemon.start_run_with(&worktree.id, &cfg).unwrap(),
                EntityId::Terminal(setup.clone()),
                "the answer is the setup it waits on"
            );
        }
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());
        let waiting = terminal_row_until(&mut events, &setup, |row| row.run.run_queued).await;
        assert!(waiting.alive, "setting up, with a start waiting");

        let run = next_live_run(&mut events).await;
        assert_eq!(run.run_command.as_deref(), Some("sleep 30"));
        assert!(
            daemon.store.get_terminal(&setup).unwrap().is_none(),
            "the setup's row went first"
        );
        assert_eq!(
            daemon.store.run_terminals_in(&worktree.id).unwrap().len(),
            1
        );
        daemon.kill_all();
    }

    /// A stop while a start waits on the setup calls the start off — the
    /// row says so — and leaves the setup running.
    #[tokio::test]
    async fn a_stop_calls_off_a_start_waiting_on_setup() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let cfg = setup_and_run_config(dir.path(), "sleep 30", "sleep 30");
        let mut events = daemon.events.subscribe();
        let setup = daemon
            .start_setup(dir.path(), &worktree, &cfg)
            .unwrap()
            .unwrap();
        daemon.start_run_with(&worktree.id, &cfg).unwrap();
        terminal_row_until(&mut events, &setup, |row| row.run.run_queued).await;

        daemon.stop_run(&worktree.id).unwrap();
        let called_off = terminal_row_until(&mut events, &setup, |row| !row.run.run_queued).await;
        assert!(called_off.alive, "the setup carries on");
        assert!(daemon.queued_runs.lock().unwrap().is_empty());
        daemon.kill_all();
    }

    /// A setup that fails starts no run that was waiting on it, and a
    /// warning says so; a press after that starts the run at once.
    #[tokio::test]
    async fn a_failed_setup_starts_no_waiting_run() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        let cfg = setup_and_run_config(dir.path(), "sleep 0.3; exit 2", "sleep 30");
        let mut events = daemon.events.subscribe();
        let mut warnings = daemon.events.subscribe();
        let setup = daemon
            .start_setup(dir.path(), &worktree, &cfg)
            .unwrap()
            .unwrap();
        daemon.start_run_with(&worktree.id, &cfg).unwrap();

        let failed = terminal_row_until(&mut events, &setup, |row| !row.alive).await;
        assert_eq!(failed.run.exit_code, Some(2));
        assert!(!failed.run.run_queued, "nothing waits on it now");
        assert!(daemon
            .store
            .run_terminals_in(&worktree.id)
            .unwrap()
            .is_empty());
        let said = drain_warnings(&mut warnings);
        assert!(
            said.iter().any(|w| w.contains("stack didn't start")),
            "{said:?}"
        );

        let EntityId::Terminal(run) = daemon.start_run_with(&worktree.id, &cfg).unwrap() else {
            panic!("a run lives in a terminal");
        };
        assert_ne!(run, setup);
        assert!(daemon.is_alive(&SessionRef::Terminal(run)));
        daemon.kill_all();
    }

    #[tokio::test]
    async fn pr_launch_context_is_accepted_for_every_kind() {
        let daemon = test_daemon();
        let spec = |kind: AgentKind| CreateAgentSpec {
            automation: None,
            mode: Default::default(),
            worktree: WorktreeId("unused".into()),
            name: "pr".into(),
            kind,
            custom_harness: None,
            model: None,
            effort: None,
            auto_title: false,
            starting_prompt: None,
            pr_url: Some("https://github.com/o/r/pull/7".into()),
        };
        for kind in AgentKind::ALL {
            if kind == AgentKind::Custom {
                // Without a registry id the custom refusal comes first.
                let err = daemon.create_agent(spec(kind)).await.unwrap_err();
                assert!(err.to_string().contains("registry id"), "{err}");
                continue;
            }
            // Validation passes for every harness; the missing worktree is
            // what stops this spec, one check later.
            let err = daemon.create_agent(spec(kind)).await.unwrap_err();
            assert!(
                err.to_string().contains("worktree not found"),
                "{kind:?}: {err}"
            );
        }
        let not_a_pr = CreateAgentSpec {
            mode: Default::default(),
            pr_url: Some("https://github.com/o/r/issues/7".into()),
            ..spec(AgentKind::Codex)
        };
        let err = daemon.create_agent(not_a_pr).await.unwrap_err();
        assert!(err.to_string().contains("not a pull request URL"), "{err}");
    }

    /// A PR SESSION's checkout is made once for the PR's head branch —
    /// fetched from `origin`, under the WORKTREE DIR, never the ROOT
    /// WORKTREE — and every later launch for that PR finds the same row.
    /// Only a PR whose branch the root itself has checked out runs there.
    #[tokio::test]
    async fn pr_worktree_is_created_once_and_shared_by_later_launches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let origin = root.join("origin.git");
        std::fs::create_dir(&origin).unwrap();
        git_in(&origin, &["init", "--bare", "-b", "main"]);
        git_in(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_in(&repo, &["push", "-u", "origin", "main"]);
        git_in(&repo, &["branch", "feat-x", "main"]);
        git_in(&repo, &["push", "origin", "feat-x"]);
        git_in(&repo, &["branch", "-D", "feat-x"]);

        let daemon = test_daemon();
        let EntityId::Project(project) = daemon.add_project(&repo, None, false).await.unwrap()
        else {
            panic!("expected a project id");
        };
        let mut events = daemon.events.subscribe();

        let first = daemon.pr_worktree(&project, 7, "feat-x").await.unwrap();
        assert!(!first.is_main, "never the ROOT WORKTREE");
        assert_eq!(first.branch, "feat-x");
        assert_eq!(first.path, git::worktree_dir(&repo, "feat-x"));
        assert!(first.path.join(".git").exists(), "a real checkout");
        assert!(
            matches!(
                events.try_recv(),
                Ok(ServerEvent::EntityUpserted {
                    entity: Entity::Worktree(w)
                }) if w.id == first.id
            ),
            "clients hear about the new row before the agent lands in it"
        );

        let again = daemon.pr_worktree(&project, 7, "feat-x").await.unwrap();
        assert_eq!(
            again.id, first.id,
            "one checkout per PR, shared by every launch"
        );
        assert!(events.try_recv().is_err(), "nothing new to broadcast");

        let on_main = daemon.pr_worktree(&project, 8, "main").await.unwrap();
        assert!(
            on_main.is_main,
            "a PR whose branch the root has checked out is already there"
        );

        // A fork's `main` is not that branch: the client names its checkout
        // for the fork's owner, so the root is no match — the contributor's
        // commit gets a checkout of its own, shared like any other.
        git_in(&repo, &["commit", "--allow-empty", "-m", "fork work"]);
        git_in(&repo, &["push", "origin", "HEAD:refs/pull/129/head"]);
        git_in(&repo, &["reset", "--hard", "origin/main"]);
        let fork = daemon
            .pr_worktree(&project, 129, "someone/main")
            .await
            .unwrap();
        assert!(!fork.is_main, "a fork's main is never the ROOT WORKTREE");
        assert_eq!(fork.branch, "someone/main");
        assert_eq!(fork.path, git::worktree_dir(&repo, "someone/main"));
        let again = daemon
            .pr_worktree(&project, 129, "someone/main")
            .await
            .unwrap();
        assert_eq!(again.id, fork.id, "and later launches share it");

        // CHECK OUT A PULL REQUEST finds the same checkout a PR SESSION
        // runs in — and refuses a bad URL or head before any git.
        let checked_out = daemon
            .create_pr_worktree(&project, "https://github.com/o/r/pull/7", "feat-x")
            .await
            .unwrap();
        assert_eq!(checked_out, EntityId::Worktree(first.id.clone()));
        for (url, head) in [
            ("https://github.com/o/r/issues/7", "feat-x"),
            ("https://github.com/o/r/pull/7", "--force"),
        ] {
            assert!(daemon
                .create_pr_worktree(&project, url, head)
                .await
                .is_err());
        }

        // A bad head never reaches git — it is refused ahead of the lookup.
        let err = daemon
            .create_pr_agent(crate::pr_scope::CreatePrAgentSpec {
                mode: Default::default(),
                project: project.clone(),
                name: "pr".into(),
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                auto_title: false,
                pr_url: "https://github.com/o/r/pull/7".into(),
                head: "--force".into(),
                starting_prompt: None,
                automation: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a branch name"), "{err}");

        // A preset's composed prompt rides the same create, and is held to
        // `CreateAgent`'s rules once the checkout is found: it reaches the
        // agent create, where the NUL is refused.
        let err = daemon
            .create_pr_agent(crate::pr_scope::CreatePrAgentSpec {
                mode: Default::default(),
                project: project.clone(),
                name: "pr".into(),
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                auto_title: true,
                pr_url: "https://github.com/o/r/pull/7".into(),
                head: "feat-x".into(),
                starting_prompt: Some("fix\0auth".into()),
                automation: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("NUL"), "{err}");
    }

    #[tokio::test]
    async fn starting_prompt_is_validated() {
        let daemon = test_daemon();
        let spec = |kind: AgentKind, starting: Option<&str>| CreateAgentSpec {
            mode: Default::default(),
            worktree: WorktreeId("unused".into()),
            name: "preset".into(),
            kind,
            custom_harness: None,
            model: None,
            effort: None,
            auto_title: true,
            starting_prompt: starting.map(String::from),
            pr_url: None,
            automation: None,
        };
        // Validation runs before the worktree lookup, so an unknown
        // worktree is fine here and every failure is the prompt's own.
        for (kind, starting, needle) in [
            (AgentKind::Claude, " \n ", "is empty"),
            (AgentKind::Codex, "fix\0auth", "NUL"),
        ] {
            let err = daemon
                .create_agent(spec(kind, Some(starting)))
                .await
                .unwrap_err();
            assert!(err.to_string().contains(needle), "{kind:?}: {err}");
        }
    }

    /// Which launches boot straight into a turn — the question the
    /// optimistic `running` hangs on. Claude and Grok take a launch rule on
    /// their system-prompt flag, so only a task makes them work at once;
    /// Codex, Cursor and OpenCode have no such flag, so the rule itself
    /// opens their first prompt.
    #[test]
    fn a_launch_submits_a_first_prompt_when_it_carries_a_task_or_an_unflagged_rule() {
        for kind in AgentKind::ALL {
            if kind == AgentKind::Custom {
                continue; // no descriptor without a registry entry
            }
            let harness = resolve_harness(kind, None).unwrap();
            let flagged = harness.system.append_flag.is_some();
            assert!(
                Daemon::launch_submits_first_prompt(&harness, Some("Fix auth"), false),
                "{kind:?}: a task is always the first prompt"
            );
            assert!(
                Daemon::launch_submits_first_prompt(&harness, Some("Fix auth"), true),
                "{kind:?}: a task beside a rule too"
            );
            assert_eq!(
                Daemon::launch_submits_first_prompt(&harness, None, true),
                !flagged,
                "{kind:?}: a bare rule opens the first prompt only without the flag"
            );
            assert!(
                !Daemon::launch_submits_first_prompt(&harness, None, false),
                "{kind:?}: a bare launch parks at the CLI's own input"
            );
        }
    }

    /// The row the clients are told about is already `running` when the
    /// launch carries a task: the CLI submits it as it boots, and the
    /// session must not sit gray — and sort under everything mid-turn —
    /// for the seconds until its first hook lands.
    #[tokio::test]
    async fn a_create_with_a_task_broadcasts_a_running_row() {
        let daemon = test_daemon();
        let (dir, worktree) = run_worktree(&daemon);
        // `/bin/cat` stands in for the CLI: spawned verbatim, it blocks on
        // the PTY instead of running anything.
        let _cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        let spec = |name: &str, task: Option<&str>| CreateAgentSpec {
            mode: Default::default(),
            worktree: worktree.id.clone(),
            name: name.into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            auto_title: false,
            starting_prompt: task.map(String::from),
            pr_url: None,
            automation: None,
        };

        let created = |mut events: broadcast::Receiver<ServerEvent>| {
            std::iter::from_fn(|| events.try_recv().ok())
                .find_map(|e| match e {
                    ServerEvent::EntityUpserted {
                        entity: Entity::Agent(a),
                    } => Some(a),
                    _ => None,
                })
                .expect("the create broadcasts its row")
        };

        let events = daemon.events.subscribe();
        let EntityId::Agent(with_task) = daemon
            .create_agent(spec("task", Some("Fix auth")))
            .await
            .unwrap()
        else {
            panic!("a create makes an agent");
        };
        assert_eq!(created(events).status, AgentStatus::Running);
        assert_eq!(
            daemon.store.get_agent(&with_task).unwrap().unwrap().status,
            AgentStatus::Running,
            "and that is what a restart reads back"
        );
        // Seeded with its reprieve, so the CLI's startup progress-clear
        // cannot green it out before the turn begins.
        daemon.apply_hook_event(&with_task, HookEvent::Progress { busy: false }, None);
        assert_eq!(
            daemon.store.get_agent(&with_task).unwrap().unwrap().status,
            AgentStatus::Running
        );

        // A launch with nothing to do still parks at the CLI's input box.
        let events = daemon.events.subscribe();
        daemon.create_agent(spec("bare", None)).await.unwrap();
        assert_eq!(created(events).status, AgentStatus::Fresh);
        drop(dir);
    }

    /// Held by every [`EnvGuard`] while it lives: tests run in parallel,
    /// and one test's restore would otherwise unset the var under another
    /// that set it too.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Save/restore around a process-wide env var a test has to set.
    pub(crate) struct EnvGuard {
        key: &'static str,
        was: Option<String>,
        _held: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        pub(crate) fn set(key: &'static str, value: &str) -> Self {
            let held = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let was = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self {
                key,
                was,
                _held: held,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.was.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn failed_agent_spawn_rolls_back_the_persisted_row() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "w", "/tmp", true);
        seed_agent(&daemon, "a", "w", None);
        let id = AgentId("a".into());

        let error = daemon
            .rollback_agent_on_spawn_error(&id, Err::<(), _>(anyhow::anyhow!("spawn failed")))
            .unwrap_err();

        assert!(error.to_string().contains("spawn failed"));
        assert!(daemon.store.get_agent(&id).unwrap().is_none());
    }

    fn seed_projects(daemon: &Daemon, names: &[&str]) {
        for (i, name) in names.iter().enumerate() {
            daemon
                .store
                .insert_project(&Project {
                    id: ProjectId((*name).into()),
                    name: (*name).into(),
                    repo_path: format!("/tmp/{name}").into(),
                    sort_order: i as i64,
                })
                .unwrap();
        }
    }

    pub(crate) fn seed_worktree(
        daemon: &Daemon,
        project: &str,
        id: &str,
        path: &str,
        is_main: bool,
    ) {
        daemon
            .store
            .insert_worktree(&Worktree {
                id: WorktreeId(id.into()),
                project_id: ProjectId(project.into()),
                path: path.into(),
                branch: id.into(),
                is_main,
                sort_order: 0,
            })
            .unwrap();
    }

    fn seed_agent(daemon: &Daemon, id: &str, worktree: &str, session_id: Option<&str>) {
        daemon
            .store
            .insert_agent(&Agent {
                id: AgentId(id.into()),
                worktree_id: WorktreeId(worktree.into()),
                name: id.into(),
                status: AgentStatus::Running,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: session_id.map(str::to_string),
                sort_order: 0,
                status_changed_at: 0,
                alive: false,
                recent_prompts: Vec::new(),
                usage_limit: None,
                automation: None,
            })
            .unwrap();
    }

    fn agent_worktree(daemon: &Daemon, id: &str) -> String {
        daemon
            .store
            .get_agent(&AgentId(id.into()))
            .unwrap()
            .unwrap()
            .worktree_id
            .to_string()
    }

    #[tokio::test]
    async fn enter_worktree_takes_an_existing_branch_and_moves_the_row_now() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/orion-test/p-feat", false);
        seed_agent(&daemon, "a1", "root", Some("s1"));
        let a1 = AgentId("a1".into());
        let mut rx = daemon.events.subscribe();

        let (target, outcome) = daemon.enter_worktree(&a1, "feat", None).await.unwrap();
        assert_eq!(target.id.to_string(), "feat");
        // No PTY runs here, so nothing waits on a turn end.
        assert_eq!(outcome, EnterOutcome::NextLaunch);
        assert!(!daemon.relocation_pending(&a1));
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");
        match rx.try_recv().unwrap() {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } => assert_eq!(a.worktree_id.to_string(), "feat"),
            other => panic!("expected agent upsert, got {other:?}"),
        }

        // Already there: a settled answer, no broadcast.
        let (again, outcome) = daemon.enter_worktree(&a1, "feat", None).await.unwrap();
        assert_eq!(again.id, target.id);
        assert_eq!(outcome, EnterOutcome::AlreadyThere);
        assert!(rx.try_recv().is_err(), "no broadcast for a no-op enter");

        // Blank names are refused before anything is touched.
        assert!(daemon.enter_worktree(&a1, "  ", None).await.is_err());
    }

    /// Between `orion worktree` and the turn's Stop the row already sits
    /// under the target while the process still reports the old checkout:
    /// that cwd must not drag it back, and only a turn-end hook drains the
    /// pending relocation.
    #[test]
    fn pending_relocation_ignores_the_old_cwd_until_the_turn_ends() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/orion-test/p-feat", false);
        seed_agent(&daemon, "a1", "feat", Some("s1"));
        let a1 = AgentId("a1".into());
        let feat = daemon
            .store
            .get_worktree(&WorktreeId("feat".into()))
            .unwrap()
            .unwrap();
        daemon
            .pending_moves
            .lock()
            .unwrap()
            .insert(a1.clone(), feat);

        daemon.reparent_agent_by_cwd(&a1, "/orion-test/p", Some("s1"), false);
        assert_eq!(
            agent_worktree(&daemon, "a1"),
            "feat",
            "the old checkout's cwd is ignored mid-relocation"
        );

        daemon.complete_pending_move(
            &a1,
            &HookEvent::PostToolUse {
                tool_name: Some("Bash".into()),
                subagent_id: None,
            },
        );
        assert!(
            daemon.relocation_pending(&a1),
            "a tool hook is not a turn end"
        );
        daemon.complete_pending_move(&a1, &HookEvent::Stop);
        assert!(!daemon.relocation_pending(&a1));

        // Drained, the reparent is live again.
        daemon.reparent_agent_by_cwd(&a1, "/orion-test/p", Some("s1"), false);
        assert_eq!(agent_worktree(&daemon, "a1"), "root");
    }

    /// `orion worktree` from a live session: the turn end that triggers
    /// the relocation must not finish the row for the seconds until the
    /// respawned CLI's first hook — the card would drop to the bottom of
    /// the grid and climb back — so the Stop is held at `running`, the
    /// respawn is seeded as a launch (its startup progress-clear swallowed),
    /// and only the relocated turn's own end finishes it.
    #[tokio::test]
    async fn a_relocation_keeps_the_row_running_through_the_respawn() {
        let daemon = test_daemon();
        let (dir, main) = run_worktree(&daemon);
        let feat_dir = tempfile::tempdir().unwrap();
        let feat = Worktree {
            id: WorktreeId::generate(),
            project_id: main.project_id.clone(),
            path: feat_dir.path().to_path_buf(),
            branch: "feat".into(),
            is_main: false,
            sort_order: 1,
        };
        daemon.store.insert_worktree(&feat).unwrap();
        // `/bin/cat` stands in for the CLI, on the first boot and the
        // respawn alike (an override takes no argv, notice included).
        let _cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        let EntityId::Agent(id) = daemon
            .create_agent(CreateAgentSpec {
                mode: Default::default(),
                worktree: main.id.clone(),
                name: "a".into(),
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                auto_title: false,
                starting_prompt: None,
                pr_url: None,
                automation: None,
            })
            .await
            .unwrap()
        else {
            panic!("a create makes an agent");
        };
        let sref = SessionRef::Agent(id.clone());
        let status = |id: &AgentId| daemon.store.get_agent(id).unwrap().unwrap().status;
        daemon.apply_hook_event(&id, HookEvent::UserPromptSubmit, Some("s1".into()));
        assert_eq!(status(&id), AgentStatus::Running);

        // The turn runs `orion worktree feat`: the row moves now, the PTY
        // waits for the turn's end.
        let (_, outcome) = daemon.enter_worktree(&id, "feat", None).await.unwrap();
        assert_eq!(outcome, EnterOutcome::Relocating);
        let first = daemon.session(&sref).expect("still the first PTY");
        let mut rx = daemon.events.subscribe();

        // The turn's Stop lands as the hook loop delivers it: through the
        // status machine first, then the relocation.
        daemon.apply_hook_event(&id, HookEvent::Stop, Some("s1".into()));
        assert_eq!(
            status(&id),
            AgentStatus::Running,
            "the Stop is held for the relocation"
        );
        daemon.complete_pending_move(&id, &HookEvent::Stop);
        assert!(!daemon.relocation_pending(&id));
        let second = daemon.session(&sref).expect("respawned in the worktree");
        assert!(!Arc::ptr_eq(&first, &second), "a new PTY");
        assert_eq!(status(&id), AgentStatus::Running);
        // The respawned CLI's startup progress-clear is its boot, not a
        // turn ending.
        daemon.apply_hook_event(&id, HookEvent::Progress { busy: false }, None);
        assert_eq!(status(&id), AgentStatus::Running);
        while let Ok(ev) = rx.try_recv() {
            assert!(
                !matches!(
                    ev,
                    ServerEvent::StatusChanged {
                        status: AgentStatus::Finished,
                        ..
                    }
                ),
                "nothing in between said finished: {ev:?}"
            );
        }
        // The relocated turn's own end does.
        daemon.apply_hook_event(&id, HookEvent::Stop, Some("s1".into()));
        assert_eq!(status(&id), AgentStatus::Finished);
        drop(dir);
    }

    /// The same relocation when the respawn carries no notice (cursor
    /// resumes silent and waits for the user) or nothing is left to
    /// respawn: the held turn end finishes the row after all, since no
    /// prompt is carrying the work on.
    #[test]
    fn a_relocation_with_no_respawn_lets_the_held_turn_end_finish() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/orion-test/p-feat", false);
        seed_agent(&daemon, "a1", "feat", Some("s1"));
        let a1 = AgentId("a1".into());
        let feat = daemon
            .store
            .get_worktree(&WorktreeId("feat".into()))
            .unwrap()
            .unwrap();
        daemon
            .pending_moves
            .lock()
            .unwrap()
            .insert(a1.clone(), feat);
        let status = |id: &AgentId| daemon.store.get_agent(id).unwrap().unwrap().status;

        daemon.apply_hook_event(&a1, HookEvent::Stop, Some("s1".into()));
        assert_eq!(status(&a1), AgentStatus::Running, "held while pending");
        // No PTY to relocate: the hold is released and the Stop finishes it.
        daemon.complete_pending_move(&a1, &HookEvent::Stop);
        assert!(!daemon.relocation_pending(&a1));
        assert_eq!(status(&a1), AgentStatus::Finished);
    }

    #[tokio::test]
    async fn enter_worktree_creates_the_checkout_in_orions_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let daemon = test_daemon();
        daemon
            .store
            .insert_project(&Project {
                id: ProjectId("p".into()),
                name: "p".into(),
                repo_path: repo.clone(),
                sort_order: 0,
            })
            .unwrap();
        seed_worktree(&daemon, "p", "root", &repo.to_string_lossy(), true);
        seed_agent(&daemon, "a1", "root", Some("s1"));
        let a1 = AgentId("a1".into());
        let mut rx = daemon.events.subscribe();

        let (target, _) = daemon.enter_worktree(&a1, "feat", None).await.unwrap();
        assert_eq!(target.branch, "feat");
        assert_eq!(
            target.path,
            root.join(".orion-worktrees").join("repo").join("feat")
        );
        assert!(target.path.join(".git").exists(), "a real checkout");
        assert_eq!(agent_worktree(&daemon, "a1"), target.id.to_string());
        // The worktree's upsert lands first, then the agent's.
        assert!(matches!(
            rx.try_recv().unwrap(),
            ServerEvent::EntityUpserted { entity: Entity::Worktree(w) } if w.id == target.id
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            ServerEvent::EntityUpserted { entity: Entity::Agent(a) } if a.worktree_id == target.id
        ));
    }

    /// `orion spawn --worktree`'s lookup: the branch's row when the
    /// project has one, a new checkout in orion's layout when it has not —
    /// and a refused spawn (a blank task) makes no checkout at all.
    #[tokio::test]
    async fn spawn_into_a_worktree_takes_or_creates_the_branch_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let daemon = test_daemon();
        daemon
            .store
            .insert_project(&Project {
                id: ProjectId("p".into()),
                name: "p".into(),
                repo_path: repo.clone(),
                sort_order: 0,
            })
            .unwrap();
        seed_worktree(&daemon, "p", "root", &repo.to_string_lossy(), true);
        seed_agent(&daemon, "a1", "root", Some("s1"));
        let a1 = AgentId("a1".into());

        let err = daemon
            .spawn_sibling_agent(&a1, None, "  ", Some("feat"), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("is empty"), "{err}");
        assert!(
            !root
                .join(".orion-worktrees")
                .join("repo")
                .join("feat")
                .exists(),
            "a refused spawn leaves no checkout behind"
        );

        let project = ProjectId("p".into());
        let made = daemon
            .worktree_for_branch(&project, "feat", None)
            .await
            .unwrap();
        assert_eq!(
            made.path,
            root.join(".orion-worktrees").join("repo").join("feat")
        );
        let again = daemon
            .worktree_for_branch(&project, " feat ", None)
            .await
            .unwrap();
        assert_eq!(again.id, made.id, "an existing branch row is reused");
        let spec = daemon
            .sibling_spec(&a1, None, "Fix fonts", Some(made.id.clone()))
            .unwrap();
        assert_eq!(spec.worktree, made.id);
        assert_eq!(spec.name, "agent-1", "names count in the target worktree");
        assert!(daemon
            .worktree_for_branch(&project, " ", None)
            .await
            .is_err());
    }

    fn seed_pending_agent(daemon: &Daemon, id: &str, worktree: &str) {
        daemon
            .store
            .insert_agent_with_auto_title(
                &Agent {
                    id: AgentId(id.into()),
                    worktree_id: WorktreeId(worktree.into()),
                    name: format!("{id}-default"),
                    status: AgentStatus::Fresh,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: false,
                    recent_prompts: Vec::new(),
                    usage_limit: None,
                    automation: None,
                },
                true,
            )
            .unwrap();
    }

    #[test]
    fn auto_rename_applies_once_and_defers_to_user_titles() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_pending_agent(&daemon, "a1", "root");
        let mut rx = daemon.events.subscribe();

        // First agent attempt lands, sanitized, and is broadcast.
        daemon
            .auto_rename_agent(&AgentId("a1".into()), "  Fix   Login\tRedirect  ")
            .unwrap();
        match rx.try_recv().unwrap() {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } => assert_eq!(a.name, "Fix Login Redirect"),
            other => panic!("expected agent upsert, got {other:?}"),
        }

        // A second attempt is declined with a settled, informative error.
        let err = daemon
            .auto_rename_agent(&AgentId("a1".into()), "Another Title")
            .unwrap_err();
        assert!(err.to_string().contains("already has a title"), "{err}");
        assert_eq!(
            daemon
                .store
                .get_agent(&AgentId("a1".into()))
                .unwrap()
                .unwrap()
                .name,
            "Fix Login Redirect"
        );

        // A user rename beats a pending auto-title: the CLI's later attempt
        // must not clobber it.
        seed_pending_agent(&daemon, "a2", "root");
        daemon
            .rename_agent(&AgentId("a2".into()), "my session")
            .unwrap();
        let err = daemon
            .auto_rename_agent(&AgentId("a2".into()), "Model Title")
            .unwrap_err();
        assert!(err.to_string().contains("already has a title"), "{err}");

        // Garbage titles are rejected outright.
        assert!(daemon
            .auto_rename_agent(&AgentId("a1".into()), " \u{7}\n ")
            .is_err());
        // Unknown agents report cleanly.
        let err = daemon
            .auto_rename_agent(&AgentId("ghost".into()), "Some Title")
            .unwrap_err();
        assert!(err.to_string().contains("agent not found"), "{err}");
    }

    #[test]
    fn sanitize_title_collapses_and_caps() {
        assert_eq!(
            sanitize_title(" Fix   Login\u{7}Redirect \n"),
            "Fix Login Redirect"
        );
        assert_eq!(sanitize_title("\u{1b}[31m"), "[31m");
        assert_eq!(sanitize_title("   "), "");
        let long = "word ".repeat(30);
        assert!(sanitize_title(&long).chars().count() <= 60);
        assert!(!sanitize_title(&long).ends_with(' '));
    }

    #[test]
    fn reparent_by_cwd_picks_deepest_matching_worktree() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        // Nested layout: the linked checkout lives under the repo root, so
        // both paths are prefixes of a cwd inside it — deepest must win.
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/orion-test/p/.wt/feat", false);
        seed_agent(&daemon, "a1", "root", None);

        // cwd inside the root checkout (but outside the nested worktree)
        // keeps the agent where it is.
        daemon.reparent_agent_by_cwd(&AgentId("a1".into()), "/orion-test/p/src", None, false);
        assert_eq!(agent_worktree(&daemon, "a1"), "root");

        // cwd inside the nested worktree re-homes it there.
        daemon.reparent_agent_by_cwd(
            &AgentId("a1".into()),
            "/orion-test/p/.wt/feat/src",
            None,
            false,
        );
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");

        // cwd outside every worktree is ignored.
        daemon.reparent_agent_by_cwd(&AgentId("a1".into()), "/elsewhere", None, false);
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");
    }

    /// Regression: a session that creates a worktree and steps into it
    /// reports the new cwd *before* the sync has adopted a row for it (the
    /// `Stop` hook fires long before the next 2s sync tick). The cwd must be
    /// remembered and replayed on adoption, or the row sits under the old
    /// checkout until the user's next prompt.
    #[tokio::test]
    async fn worktree_sync_replays_a_cwd_reported_before_adoption() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);

        let daemon = test_daemon();
        let project = Project {
            id: ProjectId("p".into()),
            name: "p".into(),
            repo_path: repo.clone(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        seed_worktree(&daemon, "p", "root", &repo.to_string_lossy(), true);
        seed_agent(&daemon, "a1", "root", Some("s1"));

        // The agent creates a sibling worktree and walks into it. The hook
        // lands first: no row exists yet, so nothing moves.
        let feat = root.join(".orion-worktrees").join("repo").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &feat.to_string_lossy(), "-b", "feat"],
        );
        daemon.reparent_agent_by_cwd(
            &AgentId("a1".into()),
            &feat.to_string_lossy(),
            Some("s1"),
            false,
        );
        assert_eq!(agent_worktree(&daemon, "a1"), "root");

        // The sync adopts the checkout and replays the remembered cwd.
        daemon.sync_project_worktrees(&project).await.unwrap();
        let (_, worktrees, _, _) = daemon.store.load_tree().unwrap();
        let adopted = worktrees
            .iter()
            .find(|w| w.branch == "feat")
            .expect("feat worktree adopted");
        assert_eq!(agent_worktree(&daemon, "a1"), adopted.id.to_string());
    }

    /// The replay is scoped to the synced project and skips archived rows.
    #[test]
    fn cwd_replay_skips_other_projects_and_archived_agents() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p", "q"]);
        seed_worktree(&daemon, "p", "p-root", "/orion-test/p", true);
        seed_worktree(&daemon, "q", "q-root", "/orion-test/q", true);
        seed_worktree(&daemon, "q", "q-feat", "/orion-test/q-feat", false);
        seed_agent(&daemon, "a1", "q-root", None);
        seed_agent(&daemon, "a2", "q-root", None);

        // Both agents report a cwd inside q-feat before it exists...
        daemon
            .store
            .delete_worktree(&WorktreeId("q-feat".into()))
            .unwrap();
        daemon.reparent_agent_by_cwd(&AgentId("a1".into()), "/orion-test/q-feat", None, false);
        daemon.reparent_agent_by_cwd(&AgentId("a2".into()), "/orion-test/q-feat", None, false);
        seed_worktree(&daemon, "q", "q-feat", "/orion-test/q-feat", false);

        // ...but a replay for project p touches neither.
        let p = daemon
            .store
            .get_project(&ProjectId("p".into()))
            .unwrap()
            .unwrap();
        daemon.reparent_agents_by_last_cwd(&p);
        assert_eq!(agent_worktree(&daemon, "a1"), "q-root");

        // Archived agents stay put; live ones re-home.
        daemon
            .store
            .set_agent_archived(&AgentId("a2".into()), true)
            .unwrap();
        let q = daemon
            .store
            .get_project(&ProjectId("q".into()))
            .unwrap()
            .unwrap();
        daemon.reparent_agents_by_last_cwd(&q);
        assert_eq!(agent_worktree(&daemon, "a1"), "q-feat");
        assert_eq!(agent_worktree(&daemon, "a2"), "q-root");
    }

    /// Renaming a project relabels its row and nothing else: the checkout on
    /// disk keeps its name, `repo_path` keeps pointing at it, and an empty
    /// name puts the row back on the folder's own name — the only way back
    /// once a project has been renamed.
    #[tokio::test]
    async fn rename_project_relabels_the_row_and_leaves_the_folder_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("acme-api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);

        let daemon = test_daemon();
        let id = match daemon.add_project(&repo, None, false).await.unwrap() {
            EntityId::Project(id) => id,
            other => panic!("expected a project id, got {other:?}"),
        };
        let named = |daemon: &Arc<Daemon>| daemon.store.get_project(&id).unwrap().unwrap();
        assert_eq!(named(&daemon).name, "acme-api", "named after the folder");

        daemon.rename_project(&id, "  Acme API  ").unwrap();
        let project = named(&daemon);
        assert_eq!(project.name, "Acme API", "trimmed and stored");
        assert_eq!(project.repo_path, repo, "the folder is untouched");
        assert!(repo.exists(), "and still on disk under its own name");
        assert_eq!(
            project.folder_subtitle().as_deref(),
            Some("acme-api"),
            "a renamed row still shows where it lives"
        );

        daemon.rename_project(&id, "   ").unwrap();
        let project = named(&daemon);
        assert_eq!(project.name, "acme-api", "empty resets to the folder name");
        assert_eq!(project.folder_subtitle(), None, "nothing left to show");
    }

    /// `git rev-parse --show-toplevel` answers with the checkout it ran in, so
    /// `orion add .` from inside a linked worktree used to make the worktree
    /// the project: named after the branch directory, `repo_path` pointing at
    /// it, and a ⌂ root row for a directory the project did not own. The repo
    /// is the project no matter which of its checkouts you add it from.
    #[tokio::test]
    async fn add_project_from_inside_a_worktree_roots_at_the_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let feat = root
            .join(".orion-worktrees")
            .join("repo")
            .join("gentle-narwhal-files");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                &feat.to_string_lossy(),
                "-b",
                "gentle-narwhal-files",
            ],
        );

        let daemon = test_daemon();
        daemon.add_project(&feat, None, false).await.unwrap();

        let (projects, worktrees, _, _) = daemon.store.load_tree().unwrap();
        let project = projects.first().expect("project added");
        assert_eq!(project.repo_path, repo, "project is rooted at the repo");
        assert_eq!(project.name, "repo", "named after the repo, not the branch");

        let main: Vec<&Worktree> = worktrees.iter().filter(|w| w.is_main).collect();
        assert_eq!(main.len(), 1, "exactly one root row: {worktrees:#?}");
        assert_eq!(
            main[0].path, repo,
            "the ⌂ root row is the project's own dir"
        );
        assert_eq!(main[0].branch, "main");
        let linked = worktrees
            .iter()
            .find(|w| !w.is_main)
            .expect("the worktree we added from is a plain row");
        assert_eq!(linked.path, feat);
        assert_eq!(linked.branch, "gentle-narwhal-files");
    }

    /// Adding the repo from a worktree of one already registered is the
    /// same repo, so it collides instead of arriving as a second project.
    #[tokio::test]
    async fn adding_a_worktree_of_a_known_repo_is_a_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let feat = root.join(".orion-worktrees").join("repo").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &feat.to_string_lossy(), "-b", "feat"],
        );

        let daemon = test_daemon();
        daemon.add_project(&repo, None, false).await.unwrap();
        let err = daemon.add_project(&feat, None, false).await.unwrap_err();
        assert!(
            err.to_string().contains("already added"),
            "expected a duplicate error, got: {err}"
        );
    }

    /// Root-ness is derived from git's checkout list on every pass, not frozen
    /// at insert time: a project whose rows were seeded before the root was
    /// known (or seeded wrong) has its ⌂ root row repaired in place, and the
    /// stale one loses the reprieve that kept it undeletable.
    #[tokio::test]
    async fn reconcile_moves_root_ness_onto_the_checkout_git_lists_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let feat = root.join(".orion-worktrees").join("repo").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &feat.to_string_lossy(), "-b", "feat"],
        );

        let daemon = test_daemon();
        let project = Project {
            id: ProjectId("p".into()),
            name: "p".into(),
            repo_path: repo.clone(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        // The wrong way round: the linked checkout wears the root badge and
        // the repo's own checkout is a plain row.
        seed_worktree(&daemon, "p", "wt", &feat.to_string_lossy(), true);
        seed_worktree(&daemon, "p", "rt", &repo.to_string_lossy(), false);

        daemon.sync_project_worktrees(&project).await.unwrap();

        let (_, worktrees, _, _) = daemon.store.load_tree().unwrap();
        let by = |id: &str| worktrees.iter().find(|w| w.id.as_str() == id).unwrap();
        assert!(by("rt").is_main, "the repo's checkout is the root row");
        assert!(!by("wt").is_main, "the linked checkout gave the badge back");
        assert_eq!(by("rt").branch, "main");
        assert_eq!(by("wt").branch, "feat");
    }

    /// A row still carrying a stale `is_main` no longer survives its checkout
    /// going away — the real root is always in git's list, so anything missing
    /// from it is a linked checkout, whatever flag it happens to hold.
    #[tokio::test]
    async fn reconcile_drops_a_vanished_row_that_still_claims_to_be_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);

        let daemon = test_daemon();
        let project = Project {
            id: ProjectId("p".into()),
            name: "p".into(),
            repo_path: repo.clone(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        seed_worktree(&daemon, "p", "rt", &repo.to_string_lossy(), true);
        seed_worktree(
            &daemon,
            "p",
            "ghost",
            &root
                .join(".orion-worktrees")
                .join("repo")
                .join("gone")
                .to_string_lossy(),
            true,
        );

        daemon.sync_project_worktrees(&project).await.unwrap();

        let (_, worktrees, _, _) = daemon.store.load_tree().unwrap();
        assert!(
            worktrees.iter().all(|w| w.id.as_str() != "ghost"),
            "the ghost row is gone: {worktrees:#?}"
        );
        let rt = worktrees.iter().find(|w| w.id.as_str() == "rt").unwrap();
        assert!(rt.is_main, "the surviving root row keeps the badge");
    }

    /// A repo with a linked checkout on `feat` that has been moved, by
    /// `git worktree move`, from `<root>/old/feat` to `<root>/new/feat`, and
    /// the project with its root row and the `feat` row (holding session
    /// `a`) still on the old path, as a daemon that missed the move has it.
    fn moved_checkout() -> (tempfile::TempDir, Arc<Daemon>, Project, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = init_repo(&root);
        let old = root.join("old").join("feat");
        let new = root.join("new").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &old.to_string_lossy(), "-b", "feat"],
        );
        let daemon = test_daemon();
        let project = Project {
            id: ProjectId("p".into()),
            name: "p".into(),
            repo_path: repo.clone(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        seed_worktree(&daemon, "p", "main", &repo.to_string_lossy(), true);
        seed_worktree(&daemon, "p", "feat", &old.to_string_lossy(), false);
        seed_agent(&daemon, "a", "feat", None);
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        git_in(
            &repo,
            &[
                "worktree",
                "move",
                &old.to_string_lossy(),
                &new.to_string_lossy(),
            ],
        );
        (tmp, daemon, project, new)
    }

    fn rows_of(daemon: &Daemon) -> Vec<Worktree> {
        let (_, worktrees, _, _) = daemon.store.load_tree().unwrap();
        worktrees
    }

    /// A checkout moved on disk while its session sat on it is the same
    /// checkout: its row takes the new path, keeping its id and so the
    /// session, rather than a second row being adopted for the new path
    /// while the session stays on one for a folder that is gone.
    #[tokio::test]
    async fn a_moved_checkout_takes_its_row_with_it() {
        let (_tmp, daemon, project, new) = moved_checkout();

        daemon.sync_project_worktrees(&project).await.unwrap();

        let rows = rows_of(&daemon);
        let feat: Vec<&Worktree> = rows.iter().filter(|w| w.branch == "feat").collect();
        assert_eq!(feat.len(), 1, "one row for the checkout: {rows:#?}");
        assert_eq!(feat[0].id.as_str(), "feat", "the row kept its id");
        assert_eq!(feat[0].path, new);
        let (_, _, agents, _) = daemon.store.load_tree().unwrap();
        assert_eq!(
            agents[0].worktree_id.as_str(),
            "feat",
            "the session stayed on it"
        );
    }

    /// A daemon that already adopted the moved checkout as a row of its own
    /// heals the pair: the old row, which holds the session, takes the path,
    /// and the bare new row goes.
    #[tokio::test]
    async fn a_moved_checkout_adopted_twice_is_one_row_again() {
        let (_tmp, daemon, project, new) = moved_checkout();
        daemon
            .store
            .insert_worktree(&Worktree {
                id: WorktreeId("dup".into()),
                project_id: ProjectId("p".into()),
                path: new.clone(),
                branch: "feat".into(),
                is_main: false,
                sort_order: 0,
            })
            .unwrap();

        daemon.sync_project_worktrees(&project).await.unwrap();

        let rows = rows_of(&daemon);
        assert!(rows.iter().all(|w| w.id.as_str() != "dup"), "{rows:#?}");
        let feat = rows.iter().find(|w| w.id.as_str() == "feat").unwrap();
        assert_eq!(feat.path, new);
        // And it stays so on the next pass.
        daemon.sync_project_worktrees(&project).await.unwrap();
        assert_eq!(rows_of(&daemon).len(), 2, "the root row and feat's");
    }

    /// When the new row has a session of its own, both rows hold work and
    /// neither is the user's to lose on a guess: the pair is left for them.
    #[tokio::test]
    async fn a_moved_checkout_with_sessions_on_both_rows_is_left_alone() {
        let (_tmp, daemon, project, new) = moved_checkout();
        daemon
            .store
            .insert_worktree(&Worktree {
                id: WorktreeId("dup".into()),
                project_id: ProjectId("p".into()),
                path: new.clone(),
                branch: "feat".into(),
                is_main: false,
                sort_order: 0,
            })
            .unwrap();
        seed_agent(&daemon, "b", "dup", None);

        daemon.sync_project_worktrees(&project).await.unwrap();

        let rows = rows_of(&daemon);
        let dup = rows.iter().find(|w| w.id.as_str() == "dup").unwrap();
        assert_eq!(dup.path, new);
        let feat = rows.iter().find(|w| w.id.as_str() == "feat").unwrap();
        assert_ne!(feat.path, new, "the old row keeps its own path");
    }

    /// A fresh repo with one commit, at a canonical path (the macOS
    /// tempdir is a symlink, and git reports worktrees canonically).
    pub(crate) fn init_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        repo
    }

    fn hook_script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("hook.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    pub(crate) fn project_at(daemon: &Daemon, repo: &Path) -> Project {
        let project = Project {
            id: ProjectId("p".into()),
            name: "p".into(),
            repo_path: repo.to_path_buf(),
            sort_order: 0,
        };
        daemon.store.insert_project(&project).unwrap();
        seed_worktree(daemon, "p", "rt", &repo.to_string_lossy(), true);
        project
    }

    /// Every `Error` a daemon broadcast with no `req_id` — the warnings it
    /// raises after a request already succeeded.
    fn drain_warnings(events: &mut broadcast::Receiver<ServerEvent>) -> Vec<String> {
        let mut warnings = Vec::new();
        while let Ok(ev) = events.try_recv() {
            if let ServerEvent::Error {
                req_id: None,
                message,
            } = ev
            {
                warnings.push(message);
            }
        }
        warnings
    }

    /// WORKTREE HOOKS are gone: a repository that still names them in git
    /// config — `orion.worktreeCreateHook`, `orion.worktreeDeleteHook` —
    /// runs neither on a create or a delete, and nobody is warned.
    #[tokio::test]
    async fn the_old_worktree_hooks_in_git_config_run_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = init_repo(&root);
        let log = root.join("hook.log");
        let hook = hook_script(&root, &format!("echo ran >> '{}'", log.display()));
        for key in ["orion.worktreeCreateHook", "orion.worktreeDeleteHook"] {
            git_in(&repo, &["config", key, &hook.to_string_lossy()]);
        }
        let daemon = test_daemon();
        let project = project_at(&daemon, &repo);
        let mut events = daemon.events.subscribe();

        let EntityId::Worktree(id) = daemon
            .create_worktree_with(
                &project.id,
                "feat",
                None,
                false,
                &crate::config::Config::default(),
            )
            .await
            .unwrap()
        else {
            panic!("a worktree id");
        };
        daemon.delete_worktree(&id, false).await.unwrap();

        assert!(!log.exists(), "{}", std::fs::read_to_string(&log).unwrap());
        assert!(drain_warnings(&mut events).is_empty());
    }

    /// A new worktree runs its project's SETUP COMMAND in its own band,
    /// after its `.env` links are in, and the create's answer doesn't wait
    /// for it. A new worktree of a project without one gets no row.
    #[tokio::test]
    async fn create_worktree_runs_the_setup_command_in_the_new_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = init_repo(&root);
        std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
        std::fs::write(repo.join(".env"), "TOKEN=1\n").unwrap();
        let daemon = test_daemon();
        let project = project_at(&daemon, &repo);
        let cfg = setup_config(&repo, "sleep 0.3; cat .env > set-up");
        let mut events = daemon.events.subscribe();

        let EntityId::Worktree(id) = daemon
            .create_worktree_with(&project.id, "feat", None, false, &cfg)
            .await
            .unwrap()
        else {
            panic!("a worktree id");
        };
        let worktree = daemon.store.get_worktree(&id).unwrap().unwrap();
        let setups = daemon.store.setup_terminals_in(&id).unwrap();
        assert_eq!(setups.len(), 1, "{setups:#?}");
        assert!(
            daemon.is_alive(&SessionRef::Terminal(setups[0].id.clone())),
            "still setting up when the create answers"
        );
        let setup = terminal_row_until(&mut events, &setups[0].id, |row| row.alive).await;
        assert!(setup.setup, "out as setting up");
        assert!(
            next_terminal_event(&mut events, &setup.id).await.is_none(),
            "the setup finished and took its row"
        );
        let marker = worktree.path.join("set-up");
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "TOKEN=1\n");

        let EntityId::Worktree(bare) = daemon
            .create_worktree_with(
                &project.id,
                "bare",
                None,
                false,
                &crate::config::Config::default(),
            )
            .await
            .unwrap()
        else {
            panic!("a worktree id");
        };
        assert_eq!(daemon.store.count_terminals(&bare).unwrap(), 0);
    }

    /// ENV LINKS: a new worktree gets the main checkout's ignored `.env`
    /// as a link back to it, before the setup command runs.
    #[tokio::test]
    async fn create_worktree_links_the_main_checkouts_env_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = init_repo(&root);
        std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
        std::fs::write(repo.join(".env"), "TOKEN=1\n").unwrap();
        let daemon = test_daemon();
        let project = project_at(&daemon, &repo);

        let created = daemon
            .create_worktree(&project.id, "feat", None, false)
            .await
            .unwrap();
        let EntityId::Worktree(id) = created else {
            panic!("a worktree id: {created:?}");
        };
        let worktree = daemon.store.get_worktree(&id).unwrap().unwrap();
        let linked = worktree.path.join(".env");
        assert_eq!(std::fs::read_link(&linked).unwrap(), repo.join(".env"));
        assert_eq!(std::fs::read_to_string(linked).unwrap(), "TOKEN=1\n");
    }

    /// An existing branch is checked out under its local name, and asking
    /// for it again — either spelling — is the row already on it, not a
    /// second checkout git would refuse.
    #[tokio::test]
    async fn create_worktree_of_an_existing_branch_reuses_its_row() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = init_repo(&root);
        git_in(&repo, &["branch", "teammate"]);
        let daemon = test_daemon();
        let project = project_at(&daemon, &repo);

        let created = daemon
            .create_worktree(&project.id, "teammate", None, true)
            .await
            .unwrap();
        let EntityId::Worktree(id) = &created else {
            panic!("a worktree id: {created:?}");
        };
        let worktree = daemon.store.get_worktree(id).unwrap().unwrap();
        assert_eq!(worktree.branch, "teammate");
        assert_eq!(
            worktree.path,
            root.join(".orion-worktrees").join("repo").join("teammate")
        );

        let again = daemon
            .create_worktree(&project.id, "origin/teammate", None, true)
            .await
            .unwrap();
        assert_eq!(again, created);

        let err = daemon
            .create_worktree(&project.id, "nowhere", None, true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no branch nowhere"), "{err}");
    }

    /// Unforced, a checkout with an untracked file and an edited one is
    /// answered with its change count and left on disk with its row;
    /// forced, it goes. An ignored file is not a change.
    #[tokio::test]
    async fn an_unforced_delete_of_a_changed_worktree_keeps_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = init_repo(&root);
        std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
        git_in(&repo, &["add", ".gitignore"]);
        git_in(&repo, &["commit", "-qm", "ignore"]);
        let wt = root.join(".orion-worktrees").join("repo").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &wt.to_string_lossy(), "-b", "feat"],
        );
        let daemon = test_daemon();
        project_at(&daemon, &repo);
        seed_worktree(&daemon, "p", "feat", &wt.to_string_lossy(), false);
        let id = WorktreeId("feat".into());

        std::fs::write(wt.join(".env"), "IGNORED=1\n").unwrap();
        assert_eq!(
            daemon.delete_worktree(&id, false).await.unwrap(),
            WorktreeDelete::Deleted,
            "an ignored file alone is no reason to ask"
        );

        git_in(&repo, &["worktree", "add", &wt.to_string_lossy(), "feat"]);
        seed_worktree(&daemon, "p", "feat", &wt.to_string_lossy(), false);
        std::fs::write(wt.join("notes.txt"), "draft").unwrap();
        std::fs::write(wt.join(".gitignore"), ".env\nedited\n").unwrap();
        assert_eq!(
            daemon.delete_worktree(&id, false).await.unwrap(),
            WorktreeDelete::HasChanges(2)
        );
        assert!(wt.join("notes.txt").exists(), "nothing removed");
        assert!(
            daemon.store.get_worktree(&id).unwrap().is_some(),
            "row kept"
        );

        assert_eq!(
            daemon.delete_worktree(&id, true).await.unwrap(),
            WorktreeDelete::Deleted
        );
        assert!(!wt.exists());
    }

    /// A changed checkout's answer doesn't queue behind other worktree
    /// work: with `worktree_ops` held — as a run of deletes holds it
    /// through git and containers — an unforced delete still says
    /// at once that there are changes, and leaves the checkout be.
    #[tokio::test]
    async fn a_changed_worktree_is_answered_without_waiting_for_other_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = init_repo(&root);
        let wt = root.join(".orion-worktrees").join("repo").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &wt.to_string_lossy(), "-b", "feat"],
        );
        let daemon = test_daemon();
        project_at(&daemon, &repo);
        seed_worktree(&daemon, "p", "feat", &wt.to_string_lossy(), false);
        let id = WorktreeId("feat".into());
        std::fs::write(wt.join("notes.txt"), "draft").unwrap();

        let _busy = daemon.worktree_ops.lock().await;
        let answer = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            daemon.delete_worktree(&id, false),
        )
        .await
        .expect("answered without waiting for the lock")
        .unwrap();
        assert_eq!(answer, WorktreeDelete::HasChanges(1));
        assert!(wt.join("notes.txt").exists(), "nothing removed");
    }

    pub(crate) fn git_in(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn reparent_by_cwd_ignores_foreign_sessions_unless_capturing() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/orion-test/p-feat", false);
        seed_agent(&daemon, "a1", "root", Some("s1"));

        // A different session id on a non-capturing event (a nested claude
        // launched inside the agent's PTY) must not move the row.
        daemon.reparent_agent_by_cwd(
            &AgentId("a1".into()),
            "/orion-test/p-feat",
            Some("s2"),
            false,
        );
        assert_eq!(agent_worktree(&daemon, "a1"), "root");

        // A capturing event (re)establishes ownership, so it may move it.
        daemon.reparent_agent_by_cwd(
            &AgentId("a1".into()),
            "/orion-test/p-feat",
            Some("s2"),
            true,
        );
        assert_eq!(agent_worktree(&daemon, "a1"), "feat");
    }

    #[test]
    fn normalize_url_adds_https_and_refuses_non_links() {
        // Pasted URLs pass through untouched.
        assert_eq!(
            normalize_url("https://github.com/o/r/pull/7").unwrap(),
            "https://github.com/o/r/pull/7"
        );
        assert_eq!(normalize_url("  http://x.dev  ").unwrap(), "http://x.dev");
        // Typed hosts gain the scheme.
        assert_eq!(
            normalize_url("github.com/o/r/pull/7").unwrap(),
            "https://github.com/o/r/pull/7"
        );
        // Anything that isn't an http(s) URL is refused, so `open(1)` can
        // never be handed a scheme the user didn't intend.
        for bad in [
            "",
            "   ",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
            "just a note",
            "notaurl",
        ] {
            assert!(normalize_url(bad).is_err(), "expected refusal: {bad:?}");
        }
    }

    #[test]
    fn cli_missing_message_names_the_binary_not_the_kind() {
        // Cursor ships its agent as `cursor-agent`; naming the kind would
        // send the user off to install the wrong thing.
        assert!(cli_missing_message(AgentKind::Cursor.cli_program())
            .starts_with("cursor-agent was not found"));
        assert!(cli_missing_message(AgentKind::Claude.cli_program())
            .starts_with("claude was not found"));
        assert!(
            cli_missing_message(AgentKind::Codex.cli_program()).starts_with("codex was not found")
        );
        assert!(cli_missing_message("agy").starts_with("agy was not found"));
        // No "restart orion": agent CLIs are spawned through the user's
        // login shell, so a fresh install is picked up on the next try.
        for kind in AgentKind::ALL {
            if kind == AgentKind::Custom {
                continue; // no static program; resolved through the registry
            }
            let msg = cli_missing_message(kind.cli_program());
            assert!(msg.contains("try again"), "{msg}");
            assert!(!msg.contains("restart"), "{msg}");
        }
    }

    #[test]
    fn reparent_by_cwd_skips_archived_agents() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_worktree(&daemon, "p", "feat", "/orion-test/p-feat", false);
        seed_agent(&daemon, "a1", "root", None);
        daemon
            .store
            .set_agent_archived(&AgentId("a1".into()), true)
            .unwrap();

        daemon.reparent_agent_by_cwd(&AgentId("a1".into()), "/orion-test/p-feat", None, false);
        assert_eq!(agent_worktree(&daemon, "a1"), "root");
    }

    /// The status broadcast carries the flag it persisted: a live turn
    /// landing on finished says `unseen`, the next prompt says not.
    #[test]
    fn status_broadcast_carries_the_unseen_flag() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_agent(&daemon, "a1", "root", None); // running
        let id = AgentId("a1".into());
        let mut rx = daemon.events.subscribe();

        daemon.apply_status_effects(&id, vec![Effect::SetStatus(AgentStatus::Finished)]);
        match rx.try_recv().unwrap() {
            ServerEvent::StatusChanged { status, unseen, .. } => {
                assert_eq!(status, AgentStatus::Finished);
                assert!(unseen, "yellow → green with nobody told otherwise");
            }
            other => panic!("expected a status change, got {other:?}"),
        }
        daemon.apply_status_effects(&id, vec![Effect::SetStatus(AgentStatus::Running)]);
        match rx.try_recv().unwrap() {
            ServerEvent::StatusChanged { unseen, .. } => {
                assert!(!unseen, "a new turn: nothing finished to read")
            }
            other => panic!("expected a status change, got {other:?}"),
        }
    }

    /// The reason a hook gives rides the `StatusChanged` into red, a
    /// foreign session's never does, and leaving red drops it; the PTY
    /// dying with an error is a crash.
    #[test]
    fn status_broadcast_carries_why_the_agent_waits() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_agent(&daemon, "a1", "root", None);
        let id = AgentId("a1".into());
        let mut rx = daemon.events.subscribe();
        let reason_of_next = |rx: &mut broadcast::Receiver<ServerEvent>| loop {
            if let ServerEvent::StatusChanged { reason, .. } = rx.try_recv().unwrap() {
                return reason;
            }
        };
        let plan = WaitReason::Plan {
            plan: "Do it.".into(),
        };
        let pre_plan = || HookEvent::PreToolUse {
            tool_name: Some(orion_core::EXIT_PLAN_MODE_TOOL.into()),
            subagent_id: None,
        };
        // Seeded running: the prompt only claims the session.
        daemon.apply_hook_event(&id, HookEvent::UserPromptSubmit, Some("s1".into()));
        // Another Claude in the same checkout asks nothing of this row.
        daemon.apply_hook_event_with(&id, pre_plan(), Some("other".into()), Some(plan.clone()));
        assert!(daemon.wait_reasons.lock().unwrap().is_empty());
        daemon.apply_hook_event_with(&id, pre_plan(), Some("s1".into()), Some(plan.clone()));
        assert_eq!(reason_of_next(&mut rx), Some(plan.clone()));
        assert_eq!(daemon.wait_reason(&id), Some(plan));
        let approved = HookEvent::PostToolUse {
            tool_name: Some(orion_core::EXIT_PLAN_MODE_TOOL.into()),
            subagent_id: None,
        };
        daemon.apply_hook_event(&id, approved, Some("s1".into()));
        assert_eq!(reason_of_next(&mut rx), None);
        assert_eq!(daemon.wait_reason(&id), None);
        daemon.apply_hook_event(&id, HookEvent::SessionEnded { exit_code: Some(2) }, None);
        assert_eq!(
            reason_of_next(&mut rx),
            Some(WaitReason::Crash { exit_code: Some(2) })
        );
    }

    /// A plan steered with feedback leaves the session red, and the
    /// question it goes on to ask turns nothing red: the red status is
    /// said again with the new reason, so the phone posts its message. The
    /// same dialog's second hook says nothing new.
    #[test]
    fn a_new_dialog_while_red_says_red_again_with_its_reason() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_agent(&daemon, "a1", "root", None);
        let id = AgentId("a1".into());
        let pre = |tool: &str| HookEvent::PreToolUse {
            tool_name: Some(tool.into()),
            subagent_id: None,
        };
        let plan = WaitReason::Plan {
            plan: "Do it.".into(),
        };
        let question = WaitReason::from_tool(
            orion_core::ASK_USER_QUESTION_TOOL,
            Some(&serde_json::json!({"questions": [{"question": "Which?",
                "options": [{"label": "A"}, {"label": "B"}]}]})),
        );
        daemon.apply_hook_event(&id, HookEvent::UserPromptSubmit, Some("s1".into()));
        daemon.apply_hook_event_with(
            &id,
            pre(orion_core::EXIT_PLAN_MODE_TOOL),
            Some("s1".into()),
            Some(plan.clone()),
        );
        let mut rx = daemon.events.subscribe();
        let red_changes = |rx: &mut broadcast::Receiver<ServerEvent>| {
            let mut out = Vec::new();
            while let Ok(event) = rx.try_recv() {
                if let ServerEvent::StatusChanged { status, reason, .. } = event {
                    out.push((status, reason));
                }
            }
            out
        };
        // The same plan's permission prompt: nothing new.
        daemon.apply_hook_event_with(
            &id,
            HookEvent::PermissionRequest { subagent_id: None },
            Some("s1".into()),
            Some(plan.clone()),
        );
        assert_eq!(red_changes(&mut rx), []);
        // Steered, it asks a question: red again, over the question.
        daemon.apply_hook_event_with(
            &id,
            pre(orion_core::ASK_USER_QUESTION_TOOL),
            Some("s1".into()),
            Some(question.clone()),
        );
        assert_eq!(
            red_changes(&mut rx),
            [(AgentStatus::NeedsFeedback, Some(question.clone()))]
        );
        assert_eq!(daemon.wait_reason(&id), Some(question));
    }

    /// `mark_agent_seen` clears the flag and hands every subscriber the row
    /// — once. Marking a row already read sends nothing.
    #[test]
    fn mark_agent_seen_broadcasts_only_a_flip() {
        let daemon = test_daemon();
        seed_projects(&daemon, &["p"]);
        seed_worktree(&daemon, "p", "root", "/orion-test/p", true);
        seed_agent(&daemon, "a1", "root", None);
        let id = AgentId("a1".into());
        daemon
            .store
            .set_agent_status(&id, AgentStatus::Finished)
            .unwrap();
        let mut rx = daemon.events.subscribe();

        daemon.mark_agent_seen(&id).unwrap();
        match rx.try_recv().unwrap() {
            ServerEvent::EntityUpserted {
                entity: Entity::Agent(a),
            } => assert!(!a.unseen),
            other => panic!("expected agent upsert, got {other:?}"),
        }
        daemon.mark_agent_seen(&id).unwrap();
        assert!(rx.try_recv().is_err(), "nothing to say twice");
    }

    /// A plan or ask launch leads the fresh CLI's args with its mode flag,
    /// ahead of the first prompt; edit, and a mode the harness lacks, add
    /// nothing.
    #[test]
    fn a_mode_leads_a_fresh_launchs_args() {
        use orion_core::harness::AgentMode;
        let all = test_registry();
        let launch = |kind, mode| {
            let harness = test_harness(&all, kind);
            let (_, mut args, _) = agent_spawn_command_with(
                &harness,
                None,
                Some(Path::new(TEST_CWD)),
                None,
                None,
                None,
                Some("look around"),
                None,
                false,
            );
            push_mode(&harness, mode, &mut args);
            args
        };
        assert_eq!(
            launch(AgentKind::Claude, AgentMode::Plan),
            ["--permission-mode", "plan", "look around"]
        );
        assert_eq!(
            launch(AgentKind::Cursor, AgentMode::Ask),
            ["--mode", "ask", "--force", "look around"]
        );
        assert_eq!(launch(AgentKind::Claude, AgentMode::Edit), ["look around"]);
        assert_eq!(launch(AgentKind::Claude, AgentMode::Ask), ["look around"]);
        assert_eq!(
            launch(AgentKind::Codex, AgentMode::Plan),
            ["--yolo", "look around"]
        );
    }
}
