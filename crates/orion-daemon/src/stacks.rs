//! STACK STATUS RESOLVER: every checkout's stack — up, starting, stopping
//! or down (`orion_core::stack`) — worked out every few seconds from what
//! is true right now, and broadcast whole as `StackStatuses` when it
//! changes. One place gathers the facts ([`resolve`]): orion's run in the
//! checkout, docker's compose containers started from it, and what
//! listens on a port from inside it (`listeners`). One pure function
//! turns them into the status (`stack::derive`). So the answer is right
//! however the stack was started or stopped — `⌘⇧S`, `orion run`, an
//! agent's own shell, a terminal outside orion, a restart mid-run.
//!
//! What should never last is repaired where it happens, not here: a run
//! that ends unasked takes its containers down with it, a stop finishes
//! what the run's trap left, and boot starts again the runs a restart
//! cut off (`registry`). A stop of a stack orion didn't start is
//! [`stop_outside`]'s.
//!
//! It looks while a client watches, a run is up, or a stack isn't down;
//! with none of those, docker and lsof are left alone, and the next to
//! subscribe pokes it for a fresh look (`server`). Even then it ticks only
//! while some checkout can have a stack ([`stacks_possible`]): orion's run
//! live there, containers running from it, ports its runs served before,
//! or a compose file. With none, it waits for a poke, asks docker then
//! and nothing else, and follows no events: a user with no stacks pays
//! nothing. What it asks, and how often ([`PollState::due`]), is kept to
//! what can have changed:
//!
//! - **Docker** is followed through one long-lived `docker events` stream
//!   ([`follow_docker`]), kept while a stack can be about: a container
//!   starting, dying or changing health asks for a fresh `docker ps`.
//!   Otherwise `docker ps` runs on a poke, while a stack is starting or
//!   stopping (a fresh container's age is read off the listing), and once
//!   a minute as a safety net; and every tick while the stream is down.
//!   It used to run every 3 s, which alone kept the docker VM from idling.
//! - **lsof** (`listeners`) runs on a poke and every tick while a stack
//!   is starting or stopping; otherwise every 30 s while a client is
//!   connected, every minute with none.
//!
//! Docker out of reach is unknown, never "down": the poller asks it less
//! often until it answers again, and reads the rest meanwhile.
//!
//! IDLE STACK STOP ([`stop_idle_stacks`]): a stack left up with nothing
//! happening in its checkout for `stacks.idle_stop` (an hour unless set;
//! `off` never) is stopped the way `⌘⇧S` stops it, logged, and every
//! client told. Happening means a client attached to one of the checkout's
//! sessions or leaving one, its selection resting there (the prewarm), an
//! agent there working or changing status, or a start or `orion run`
//! there ([`StackWatch::note_activity`]).

use crate::containers::{self, find_docker, PS_TIMEOUT};
use crate::listeners::{self, Listener};
use crate::registry::Daemon;
use orion_core::compose::{self, Stack};
use orion_core::protocol::ServerEvent;
use orion_core::stack::{self, ContainerFacts, RunFact, StackFacts, StackStatus};
use orion_core::{Worktree, WorktreeId};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the poller leaves docker alone after it failed to answer: an
/// engine that is stopped stays stopped for a while.
const UNREACHABLE_BACKOFF: Duration = Duration::from_secs(30);

/// The longest `docker ps` waits while the events stream is up: a safety
/// net for an event the stream dropped.
const DOCKER_SAFETY_EVERY: Duration = Duration::from_secs(60);

/// How often lsof looks when nothing is starting or stopping, with a
/// client connected to show the answer, and with none.
const LISTENERS_WATCHED_EVERY: Duration = Duration::from_secs(30);
const LISTENERS_UNWATCHED_EVERY: Duration = Duration::from_secs(60);

/// How long after a docker event the poller waits before it looks: a
/// compose start is a burst of them, and one `docker ps` covers the lot.
const DOCKER_EVENT_SETTLE: Duration = Duration::from_millis(500);

/// The `docker events` stream's restart pace: from this after it ends…
const EVENTS_RETRY_MIN: Duration = Duration::from_secs(2);
/// …doubling while it keeps ending soon after it starts, to at most this…
const EVENTS_RETRY_MAX: Duration = Duration::from_secs(5 * 60);
/// …and back to the start once it ran at least this long.
const EVENTS_STABLE_AFTER: Duration = Duration::from_secs(60);

/// The container changes the events stream follows. `health_status`
/// matches each `health_status: <state>` (docker matches it by prefix);
/// `exec_*` — every healthcheck run — is what this leaves out.
const DOCKER_EVENTS: &[&str] = &[
    "create",
    "start",
    "restart",
    "die",
    "stop",
    "destroy",
    "pause",
    "unpause",
    "health_status",
];

/// How often the IDLE STACK STOP looks.
const IDLE_CHECK_EVERY: Duration = Duration::from_secs(60);

/// How long a stop of a stack orion didn't start gives what listens for
/// it to go after a SIGTERM, before a SIGKILL.
const OUTSIDE_STOP_GRACE_MS: i64 = 15_000;

/// How long after its kill an outside stop still reads as stopping when
/// something of the stack won't go: then it is given up on, and the
/// status says what is left.
const OUTSIDE_STOP_GIVE_UP_MS: i64 = 30_000;

/// How many ports a checkout remembers its runs served.
const KNOWN_PORTS: usize = 8;

/// What the resolver last worked out, and the poke for a fresh look.
#[derive(Default)]
pub struct StackWatch {
    last: Mutex<Resolved>,
    poke: tokio::sync::Notify,
    /// Stops of stacks orion didn't start, under way: each checkout's
    /// kill time (epoch ms).
    outside_stops: Mutex<HashMap<WorktreeId, i64>>,
    /// How many clients are subscribed ([`StackWatch::watching`]). The
    /// event channel's own count can't say: the DAEMON's own listeners
    /// (the phone bridge) hold it for its whole life.
    watchers: Arc<AtomicUsize>,
    /// A client subscribed: what the WORKTREE SYNC wakes on
    /// ([`StackWatch::client_arrived`]).
    arrivals: tokio::sync::Notify,
    /// The `docker events` stream saw a container change (or restarted,
    /// and may have missed some): a `docker ps` is owed.
    docker_dirty: AtomicBool,
    /// …and the poller is woken for it.
    docker_changed: tokio::sync::Notify,
    /// The `docker events` stream is running.
    events_live: AtomicBool,
    /// When something last happened in each checkout, epoch ms
    /// ([`StackWatch::note_activity`]).
    activity: Mutex<HashMap<WorktreeId, i64>>,
    /// When each checkout's stack was first seen up, epoch ms: the IDLE
    /// STACK STOP's clock starts no earlier, so a stack up across a DAEMON
    /// restart gets its full span from the first look.
    seen_up: Mutex<HashMap<WorktreeId, i64>>,
}

/// A client's subscription, counted while it lives.
pub struct Watching(Arc<AtomicUsize>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Resolved {
    /// Every checkout whose stack is not down.
    statuses: BTreeMap<WorktreeId, StackStatus>,
    /// What listens for each checkout's stack: what an outside stop stops.
    listeners: HashMap<WorktreeId, Vec<Listener>>,
}

impl StackWatch {
    /// The statuses, as the event a new subscriber is sent.
    pub fn current(&self) -> ServerEvent {
        event(&self.last.lock().unwrap().statuses)
    }

    /// Checkout `worktree`'s stack, as last worked out.
    pub fn status_of(&self, worktree: &WorktreeId) -> StackStatus {
        self.last
            .lock()
            .unwrap()
            .statuses
            .get(worktree)
            .cloned()
            .unwrap_or(StackStatus::Down)
    }

    /// A client subscribed: counted until the returned guard drops, and
    /// the poller asked for a fresh look.
    pub fn watching(&self) -> Watching {
        self.watchers.fetch_add(1, Ordering::Relaxed);
        self.poke();
        self.arrivals.notify_one();
        Watching(Arc::clone(&self.watchers))
    }

    /// Whether any client is connected and subscribed now.
    pub(crate) fn watched(&self) -> bool {
        self.watchers.load(Ordering::Relaxed) > 0
    }

    /// Resolves when a client next subscribes (at once, if one did since
    /// the last wait).
    pub(crate) async fn client_arrived(&self) {
        self.arrivals.notified().await;
    }

    /// Something happened in checkout `worktree` just now: a client
    /// attached to one of its sessions or left one, its selection rested
    /// there, or a stack start was asked for. Holds off its IDLE STACK
    /// STOP.
    pub fn note_activity(&self, worktree: &WorktreeId) {
        self.activity
            .lock()
            .unwrap()
            .insert(worktree.clone(), orion_core::clock::now_ms());
    }

    /// When something was last noted in `worktree`, epoch ms.
    fn noted_activity(&self, worktree: &WorktreeId) -> Option<i64> {
        self.activity.lock().unwrap().get(worktree).copied()
    }

    /// A stack is starting or stopping somewhere — the moments its facts
    /// change by the second, so the poller looks every tick.
    fn in_transition(&self) -> bool {
        let moving = self.last.lock().unwrap().statuses.values().any(|s| {
            matches!(
                s,
                StackStatus::Starting { .. } | StackStatus::Stopping { .. }
            )
        });
        moving || !self.outside_stops.lock().unwrap().is_empty()
    }

    /// The checkouts whose stack is up, with when each was first seen up.
    fn up_since(&self) -> Vec<(WorktreeId, i64)> {
        let last = self.last.lock().unwrap();
        let seen = self.seen_up.lock().unwrap();
        last.statuses
            .iter()
            .filter(|(_, s)| matches!(s, StackStatus::Up { .. }))
            .filter_map(|(w, _)| Some((w.clone(), *seen.get(w)?)))
            .collect()
    }

    /// Ask the poller for a fresh look now: a client subscribed, or a run
    /// started, stopped or ended.
    pub fn poke(&self) {
        self.poke.notify_one();
    }

    fn listeners_of(&self, worktree: &WorktreeId) -> Vec<Listener> {
        self.last
            .lock()
            .unwrap()
            .listeners
            .get(worktree)
            .cloned()
            .unwrap_or_default()
    }

    /// Keep a resolution: the event to broadcast when the statuses
    /// changed, None when they didn't.
    fn store(
        &self,
        statuses: BTreeMap<WorktreeId, StackStatus>,
        listeners: HashMap<WorktreeId, Vec<Listener>>,
    ) -> Option<ServerEvent> {
        let mut last = self.last.lock().unwrap();
        last.listeners = listeners;
        {
            let now = orion_core::clock::now_ms();
            let mut seen = self.seen_up.lock().unwrap();
            seen.retain(|w, _| statuses.get(w).is_some_and(StackStatus::is_live));
            for (w, status) in &statuses {
                if status.is_live() {
                    seen.entry(w.clone()).or_insert(now);
                }
            }
        }
        if last.statuses == statuses {
            return None;
        }
        last.statuses = statuses;
        Some(event(&last.statuses))
    }

    fn anything_up(&self) -> bool {
        !self.last.lock().unwrap().statuses.is_empty()
            || !self.outside_stops.lock().unwrap().is_empty()
    }
}

fn event(statuses: &BTreeMap<WorktreeId, StackStatus>) -> ServerEvent {
    ServerEvent::StackStatuses {
        statuses: statuses
            .iter()
            .map(|(w, s)| (w.clone(), s.clone()))
            .collect(),
    }
}

/// What woke the poller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    Tick,
    Poke,
    DockerEvent,
}

/// What the poller knows when it decides what to ask this round.
#[derive(Debug, Clone, Copy, Default)]
struct Inputs {
    /// Asked for a fresh look: a client subscribed, a run started, stopped
    /// or ended.
    poked: bool,
    /// The events stream saw a container change since the last listing.
    docker_changed: bool,
    /// A stack is starting or stopping.
    transition: bool,
    /// A client is connected.
    watched: bool,
    /// The events stream is running, so a container change would be heard.
    events_live: bool,
    /// Some checkout can have a stack ([`stacks_possible`]). With none,
    /// lsof has nothing to find and docker is asked only when told to.
    stacks: bool,
}

/// What to ask this round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Due {
    docker: bool,
    listeners: bool,
}

/// When docker and lsof were last asked, and docker last failed.
#[derive(Debug, Clone, Copy, Default)]
struct PollState {
    docker_at: Option<Instant>,
    docker_failed_at: Option<Instant>,
    listeners_at: Option<Instant>,
}

impl PollState {
    /// What to ask at `now`. Pure: the cadence rules of the module docs.
    fn due(&self, now: Instant, i: Inputs) -> Due {
        let older = |at: Option<Instant>, span: Duration| {
            at.is_none_or(|at| now.saturating_duration_since(at) >= span)
        };
        let backing_off = !i.poked
            && self
                .docker_failed_at
                .is_some_and(|at| now.saturating_duration_since(at) < UNREACHABLE_BACKOFF);
        let docker = !backing_off
            && (i.poked
                || i.docker_changed
                || i.transition
                || (i.stacks && (!i.events_live || older(self.docker_at, DOCKER_SAFETY_EVERY))));
        let every = match i.watched {
            true => LISTENERS_WATCHED_EVERY,
            false => LISTENERS_UNWATCHED_EVERY,
        };
        let listeners = i.stacks && (i.poked || i.transition || older(self.listeners_at, every));
        Due { docker, listeners }
    }
}

/// The poller: runs until the daemon shuts down. `interval` is its tick —
/// how often it reconsiders, not how often it asks anything — and it ticks
/// only while some checkout can have a stack ([`stacks_possible`]); else
/// it waits for a poke or a docker event.
pub async fn watch(daemon: Arc<Daemon>, mut interval: tokio::time::Interval) {
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let docker_cli = find_docker();
    let mut events = EventsStream::default();
    let mut state = PollState::default();
    // What docker and lsof last said, standing in between their looks.
    // None: unknown.
    let mut docker: Option<Vec<Stack>> = None;
    let mut scan: Option<Vec<Listener>> = None;
    let mut idle_checked_at: Option<Instant> = None;
    let mut stacks = false;
    loop {
        let wake = tokio::select! {
            _ = daemon.shutdown.cancelled() => break,
            _ = interval.tick(), if stacks => Wake::Tick,
            _ = daemon.stacks.poke.notified() => Wake::Poke,
            _ = daemon.stacks.docker_changed.notified() => Wake::DockerEvent,
        };
        if wake == Wake::DockerEvent {
            tokio::select! {
                _ = daemon.shutdown.cancelled() => break,
                _ = tokio::time::sleep(DOCKER_EVENT_SETTLE) => {}
            }
        }
        let active =
            daemon.stacks.watched() || daemon.stacks.anything_up() || daemon.any_run_live();
        stacks = active && stacks_possible(&daemon, docker.as_deref());
        if !stacks {
            // Asked again from scratch once a stack can be about.
            scan = None;
            state.listeners_at = None;
        }
        if active {
            let inputs = Inputs {
                poked: wake == Wake::Poke,
                docker_changed: daemon.stacks.docker_dirty.swap(false, Ordering::Relaxed),
                transition: daemon.stacks.in_transition(),
                watched: daemon.stacks.watched(),
                events_live: daemon.stacks.events_live.load(Ordering::Relaxed),
                stacks,
            };
            let due = state.due(Instant::now(), inputs);
            if due.docker {
                let listed = poll(find_docker().as_deref()).await;
                let at = Instant::now();
                state.docker_at = Some(at);
                state.docker_failed_at = listed.is_err().then_some(at);
                // A failure is unknown, and stays so until docker answers.
                docker = listed.ok();
                // Containers found running are a stack to follow from now.
                stacks = stacks || stacks_possible(&daemon, docker.as_deref());
            }
            if due.listeners {
                scan = listeners::scan().await;
                state.listeners_at = Some(Instant::now());
            }
            if due.docker || due.listeners || inputs.transition {
                resolve(&daemon, docker.as_deref(), scan.as_deref());
            }
            if stacks && idle_checked_at.is_none_or(|at| at.elapsed() >= IDLE_CHECK_EVERY) {
                idle_checked_at = Some(Instant::now());
                let limit = crate::config::Config::cached().stacks.idle_stop_after();
                stop_idle_stacks(&daemon, orion_core::clock::now_ms(), limit);
            }
        }
        events.keep(stacks, &daemon, docker_cli.as_deref());
    }
}

/// Whether any checkout can have a stack for the poller to follow: orion's
/// run is live, one is up or stopping, docker's last listing has
/// containers running from one, one's runs served a port before, or one
/// has a compose file. With none, lsof and docker's events have nothing to
/// find: a user with no stacks pays for neither.
fn stacks_possible(daemon: &Daemon, docker: Option<&[Stack]>) -> bool {
    if daemon.any_run_live() || daemon.stacks.anything_up() {
        return true;
    }
    let Ok(worktrees) = daemon.store.worktrees_with_ports() else {
        return false;
    };
    any_stack_in(
        &worktrees,
        docker,
        orion_core::run_targets::has_compose_file,
    )
}

/// [`stacks_possible`]'s look over the checkouts: any with known ports, a
/// compose file (`has_compose`), or containers running in `docker`.
fn any_stack_in(
    worktrees: &[(Worktree, Vec<u16>)],
    docker: Option<&[Stack]>,
    has_compose: impl Fn(&Path) -> bool,
) -> bool {
    let paths: Vec<&Path> = worktrees.iter().map(|(w, _)| w.path.as_path()).collect();
    let running = docker
        .unwrap_or_default()
        .iter()
        .any(|s| s.running > 0 && compose::owner_of(s, &paths).is_some());
    running
        || worktrees
            .iter()
            .any(|(w, known)| !known.is_empty() || has_compose(&w.path))
}

/// The `docker events` stream while it is wanted ([`follow_docker`]):
/// started when a stack can be about, and stopped — its process killed —
/// when none can, so docker is left alone.
#[derive(Default)]
struct EventsStream(Option<tokio::task::JoinHandle<()>>);

impl EventsStream {
    fn keep(&mut self, wanted: bool, daemon: &Arc<Daemon>, docker: Option<&Path>) {
        match (wanted, self.0.take()) {
            (true, None) => {
                self.0 = docker
                    .map(|docker| tokio::spawn(follow_docker(Arc::clone(daemon), docker.into())));
            }
            (false, Some(follow)) => {
                follow.abort();
                daemon.stacks.events_live.store(false, Ordering::Relaxed);
            }
            (_, kept) => self.0 = kept,
        }
    }
}

/// Follow docker's container events until aborted or shut down: one
/// `docker events` process, read line by line, each change marking a
/// `docker ps` owed and waking the poller. A stream that ends — the engine
/// stopped, or was never running — is started again after a wait that
/// doubles while it keeps ending soon after it starts
/// ([`events_retry_wait`]); each end owes a full listing, since changes
/// may have gone unheard.
async fn follow_docker(daemon: Arc<Daemon>, docker: PathBuf) {
    let mut fails = 0u32;
    loop {
        let started = Instant::now();
        let ended = tokio::select! {
            _ = daemon.shutdown.cancelled() => return,
            ended = stream_docker_events(&daemon, &docker) => ended,
        };
        daemon.stacks.events_live.store(false, Ordering::Relaxed);
        daemon.stacks.docker_dirty.store(true, Ordering::Relaxed);
        daemon.stacks.docker_changed.notify_one();
        fails = match started.elapsed() >= EVENTS_STABLE_AFTER {
            true => 0,
            false => fails.saturating_add(1),
        };
        let wait = events_retry_wait(fails);
        tracing::debug!(why = %ended, retry_secs = wait.as_secs(), "docker events stream ended");
        tokio::select! {
            _ = daemon.shutdown.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// The wait before the events stream's next start, after `fails` ends in a
/// row that came soon after a start: an end after a stable run (`0`) is
/// the first of a new row.
fn events_retry_wait(fails: u32) -> Duration {
    orion_core::backoff::doubling(EVENTS_RETRY_MIN, EVENTS_RETRY_MAX, fails.saturating_add(1))
}

/// The `docker events` arguments: container changes, one action a line.
fn docker_events_args() -> Vec<String> {
    let mut args: Vec<String> = ["events", "--filter", "type=container"]
        .map(String::from)
        .into();
    for event in DOCKER_EVENTS {
        args.push("--filter".into());
        args.push(format!("event={event}"));
    }
    args.extend(["--format".into(), "{{.Action}}".into()]);
    args
}

/// Whether one line of the stream says a container changed in a way the
/// listing shows. A healthcheck's `exec_*` never does; the filter keeps
/// them out already, and this keeps them out of an engine that ignores it.
fn is_container_change(action: &str) -> bool {
    let action = action.trim();
    !action.is_empty() && !action.starts_with("exec_")
}

/// One run of the events stream, until it ends; why it ended.
async fn stream_docker_events(daemon: &Daemon, docker: &Path) -> String {
    use tokio::io::AsyncBufReadExt;
    let mut cmd = tokio::process::Command::new(docker);
    cmd.args(docker_events_args())
        .env("PATH", containers::path_with(docker))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return format!("couldn't start: {e}"),
    };
    let Some(stdout) = child.stdout.take() else {
        return "no stdout".into();
    };
    daemon.stacks.events_live.store(true, Ordering::Relaxed);
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if is_container_change(&line) => {
                daemon.stacks.docker_dirty.store(true, Ordering::Relaxed);
                daemon.stacks.docker_changed.notify_one();
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => return format!("read: {e}"),
        }
    }
    match child.wait().await {
        Ok(status) => format!("exited {status}"),
        Err(e) => format!("wait: {e}"),
    }
}

/// When a stack up since `seen_up_ms` started sitting idle, epoch ms, with
/// the last thing happening in its checkout at `last_activity_ms`: the
/// later of the two — a stack first seen up after its checkout's last
/// activity has been idle only since then.
fn idle_since(seen_up_ms: i64, last_activity_ms: Option<i64>) -> i64 {
    last_activity_ms.map_or(seen_up_ms, |a| a.max(seen_up_ms))
}

/// How long a stack idle since `since_ms` ([`idle_since`]) has sat idle at
/// `now_ms`.
fn idle_for(now_ms: i64, since_ms: i64) -> Duration {
    Duration::from_millis(u64::try_from(now_ms.saturating_sub(since_ms)).unwrap_or(0))
}

/// Whether a stack up since `seen_up_ms`, with the last thing happening in
/// its checkout at `last_activity_ms`, has sat idle past `limit` at
/// `now_ms`.
fn idle_past(now_ms: i64, seen_up_ms: i64, last_activity_ms: Option<i64>, limit: Duration) -> bool {
    idle_for(now_ms, idle_since(seen_up_ms, last_activity_ms)) >= limit
}

/// IDLE STACK STOP: stop every stack that is up and has sat idle past
/// `limit` at `now_ms` ([`idle_past`]), the way `⌘⇧S` stops it
/// (`Daemon::stop_run`). Activity in a checkout is the latest of: a
/// client attached to one of its sessions (now), an agent there running
/// (now), any of its agents' last status change, and what was noted
/// ([`StackWatch::note_activity`]). Each stop is logged and every client
/// told.
pub(crate) fn stop_idle_stacks(daemon: &Arc<Daemon>, now_ms: i64, limit: Option<Duration>) {
    let Some(limit) = limit else {
        return;
    };
    let up = daemon.stacks.up_since();
    if up.is_empty() {
        return;
    }
    let attached = daemon.attached_worktrees();
    let Ok((_, worktrees, agents, _)) = daemon.store.load_tree() else {
        return;
    };
    for (id, seen_up_ms) in up {
        let here = agents.iter().filter(|a| a.worktree_id == id);
        let working = attached.contains(&id)
            || here
                .clone()
                .any(|a| a.status == orion_core::AgentStatus::Running);
        let last = match working {
            true => Some(now_ms),
            false => here
                .map(|a| a.status_changed_at)
                .chain(daemon.stacks.noted_activity(&id))
                .max(),
        };
        if !idle_past(now_ms, seen_up_ms, last, limit) {
            continue;
        }
        let name = worktrees
            .iter()
            .find(|w| w.id == id)
            .map_or_else(|| id.to_string(), |w| w.branch.clone());
        let idle_mins = idle_for(now_ms, idle_since(seen_up_ms, last)).as_secs() / 60;
        tracing::info!(worktree = %id, branch = %name, idle_mins, "stopping an idle stack");
        match daemon.stop_run(&id) {
            // News, not a failure: every TUI shows it as a neutral note.
            Ok(()) => daemon.broadcast(ServerEvent::Notice {
                text: format!("stopped idle stack for {name}"),
            }),
            Err(e) => tracing::warn!(worktree = %id, error = %e, "idle stack not stopped"),
        }
        // Its clock starts again: a stop that doesn't take is not asked
        // for every minute.
        daemon.stacks.note_activity(&id);
    }
}

/// Ask `docker` for every compose container and sum them into stacks; why
/// not, when it can't answer.
pub async fn poll(docker: Option<&Path>) -> Result<Vec<Stack>, String> {
    let Some(docker) = docker else {
        return Err(compose::NO_DOCKER_CLI.into());
    };
    let out = containers::run(docker, &compose::ps_state_args(), PS_TIMEOUT).await?;
    Ok(compose::stacks(&compose::parse_ps_state(&out)))
}

/// Work out every checkout's stack from docker's listing (`None`:
/// unknown) and what listens (`None`: unknown), keep it, and broadcast
/// it when it changed.
pub(crate) fn resolve(daemon: &Arc<Daemon>, docker: Option<&[Stack]>, scan: Option<&[Listener]>) {
    let Ok(worktrees) = daemon.store.worktrees_with_ports() else {
        return;
    };
    let now = orion_core::clock::now_ms();
    let paths: Vec<&Path> = worktrees.iter().map(|(w, _)| w.path.as_path()).collect();
    // orion's own processes are never a stack's: the daemon, and each
    // agent session's CLI — the session's own child, or the job its login
    // shell runs it as — which may listen for its own reasons. A run's or
    // a shell's child is left in: a server can be exactly that.
    let agents: HashSet<u32> = daemon
        .session_pids()
        .into_iter()
        .filter(|(sref, _, _)| matches!(sref, orion_core::SessionRef::Agent(_)))
        .map(|(_, pid, _)| pid)
        .collect();
    let daemon_pid = std::process::id();
    let ours =
        |l: &Listener| l.pid == daemon_pid || agents.contains(&l.pid) || agents.contains(&l.ppid);
    let mut by_checkout: HashMap<&Path, Vec<&Listener>> = HashMap::new();
    for l in scan.unwrap_or_default() {
        if ours(l) {
            continue;
        }
        if let Some(owner) = deepest_holding(&paths, &l.cwd) {
            by_checkout.entry(owner).or_default().push(l);
        }
    }
    let stops: HashMap<WorktreeId, i64> = daemon.stacks.outside_stops.lock().unwrap().clone();
    let mut statuses = BTreeMap::new();
    let mut listening = HashMap::new();
    let mut finished_stops = Vec::new();
    for (w, known) in &worktrees {
        let (run, printed) = daemon.run_fact(&w.id);
        let containers = docker.map(|all| containers_of(all, &w.path, &paths));
        let compose_running = containers.is_some_and(|c| c.running > 0);
        let run_alive = matches!(run, RunFact::Alive { .. });
        // A listener in the checkout is the stack's while orion's run is
        // up, while its containers run, or on a port its runs served.
        let mine: Vec<Listener> = by_checkout
            .get(w.path.as_path())
            .into_iter()
            .flatten()
            .filter(|l| run_alive || compose_running || known.contains(&l.port))
            .map(|l| (*l).clone())
            .collect();
        let ports = match scan {
            Some(_) => order_ports(&printed, &mine),
            // Nothing to look at: what orion's run printed stands in.
            None if run_alive => printed.clone(),
            None => Vec::new(),
        };
        let mut stopping_outside = stops.get(&w.id).copied();
        if let Some(kill_at) = stopping_outside {
            let gone = mine.is_empty() && containers.is_some_and(|c| c.running == 0);
            if gone || now > kill_at + OUTSIDE_STOP_GIVE_UP_MS {
                finished_stops.push(w.id.clone());
                stopping_outside = None;
            }
        }
        let compose_only = run == RunFact::None
            && ports.is_empty()
            && compose_running
            && orion_core::run_targets::compose_only(&w.path);
        let facts = StackFacts {
            run,
            stopping_outside,
            containers,
            listeners: ports,
            compose_only,
            now_ms: now,
        };
        if run_alive {
            learn_ports(daemon, &w.id, known, &facts.listeners);
        }
        let status = stack::derive(&facts);
        if !mine.is_empty() {
            listening.insert(w.id.clone(), mine);
        }
        if !status.is_down() {
            statuses.insert(w.id.clone(), status);
        }
    }
    {
        // Finished stops go, and so do those of checkouts deleted mid-stop.
        let mut stops = daemon.stacks.outside_stops.lock().unwrap();
        stops.retain(|id, _| {
            !finished_stops.contains(id) && worktrees.iter().any(|(w, _)| &w.id == id)
        });
    }
    // What happened in a checkout since deleted is nobody's any more.
    daemon
        .stacks
        .activity
        .lock()
        .unwrap()
        .retain(|id, _| worktrees.iter().any(|(w, _)| &w.id == id));
    if let Some(event) = daemon.stacks.store(statuses, listening) {
        daemon.broadcast(event);
    }
}

/// The checkout of `checkouts` that `path` is deepest inside.
fn deepest_holding<'a>(checkouts: &[&'a Path], path: &Path) -> Option<&'a Path> {
    checkouts
        .iter()
        .copied()
        .filter(|c| compose::is_within(path, c))
        .max_by_key(|c| c.components().count())
}

/// Every compose stack checkout `dir` owns, summed.
fn containers_of(all: &[Stack], dir: &Path, checkouts: &[&Path]) -> ContainerFacts {
    compose::owned_by(all, dir, checkouts).fold(ContainerFacts::default(), |mut sum, s| {
        sum.running = sum.running.saturating_add(s.running);
        sum.starting = sum.starting.saturating_add(s.starting);
        if let Some(up) = s.newest_up_secs {
            sum.newest_up_secs = Some(sum.newest_up_secs.map_or(up, |n| n.min(up)));
        }
        sum
    })
}

/// A stack's ports, the app's first: those its run printed that listen,
/// in the order printed, then the rest that listen, lowest first.
fn order_ports(printed: &[u16], listening: &[Listener]) -> Vec<u16> {
    let mut rest: Vec<u16> = listening.iter().map(|l| l.port).collect();
    rest.sort_unstable();
    rest.dedup();
    let mut out: Vec<u16> = printed
        .iter()
        .copied()
        .filter(|p| rest.contains(p))
        .collect();
    out.dedup();
    out.extend(rest.into_iter().filter(|p| !printed.contains(p)));
    out
}

/// Remember `serving` among the ports checkout `worktree`'s runs served,
/// most recent first: how its stack is known when something outside orion
/// starts it next.
fn learn_ports(daemon: &Daemon, worktree: &WorktreeId, known: &[u16], serving: &[u16]) {
    if serving.is_empty() {
        return;
    }
    let mut ports: Vec<u16> = serving.to_vec();
    ports.extend(known.iter().filter(|p| !serving.contains(p)));
    ports.truncate(KNOWN_PORTS);
    if ports != known {
        if let Err(e) = daemon.store.set_known_ports(worktree, &ports) {
            tracing::warn!(worktree = %worktree, error = %e, "couldn't remember the stack's ports");
        }
    }
}

/// Stop `worktree`'s stack, started without orion: what listens for it
/// gets a SIGTERM — its whole process group, unless that group is one of
/// orion's sessions (an agent's own CLI) — and its compose projects stop;
/// anything still listening after [`OUTSIDE_STOP_GRACE_MS`] is killed.
/// The stack reads as stopping meanwhile, until nothing of it is left.
pub(crate) fn stop_outside(daemon: &Arc<Daemon>, worktree: &Worktree) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let kill_at = orion_core::clock::now_ms() + OUTSIDE_STOP_GRACE_MS;
    daemon
        .stacks
        .outside_stops
        .lock()
        .unwrap()
        .insert(worktree.id.clone(), kill_at);
    tracing::info!(worktree = %worktree.id, "stopping a stack started outside orion");
    daemon.stacks.poke();
    daemon.stop_stacks_in(worktree.path.clone());
    let daemon = Arc::clone(daemon);
    let id = worktree.id.clone();
    rt.spawn(async move {
        signal_listeners(&daemon, &id, nix::sys::signal::Signal::SIGTERM).await;
        daemon.stacks.poke();
        let wait = (kill_at - orion_core::clock::now_ms()).max(0) as u64;
        tokio::time::sleep(Duration::from_millis(wait)).await;
        if daemon
            .stacks
            .outside_stops
            .lock()
            .unwrap()
            .contains_key(&id)
        {
            signal_listeners(&daemon, &id, nix::sys::signal::Signal::SIGKILL).await;
        }
        daemon.stacks.poke();
    });
}

/// Send `sig` to what listens for checkout `id`'s stack: each listener's
/// whole process group (its `npm` parent, its workers) when that group is
/// safe to signal, else the listener alone. A group is not safe when it is
/// orion's own, one of orion's sessions', or led by an agent's CLI — a
/// server an agent started in its own shell can share the agent's group,
/// and the agent must live through the stop.
async fn signal_listeners(daemon: &Daemon, id: &WorktreeId, sig: nix::sys::signal::Signal) {
    let pids: Vec<u32> = daemon
        .stacks
        .listeners_of(id)
        .iter()
        .map(|l| l.pid)
        .collect();
    let info = listeners::process_info(&pids).await;
    let leaders: Vec<u32> = info.values().map(|&(_, pgid)| pgid).collect();
    let leader_info = listeners::process_info(&leaders).await;
    let sessions: HashSet<u32> = daemon
        .session_pids()
        .into_iter()
        .map(|(_, pid, _)| pid)
        .chain([std::process::id()])
        .collect();
    let own_group = nix::unistd::getpgrp().as_raw() as u32;
    let safe_group = |pgid: u32| {
        pgid > 1
            && pgid != own_group
            && !sessions.contains(&pgid)
            && leader_info
                .get(&pgid)
                .is_some_and(|&(ppid, _)| !sessions.contains(&ppid))
    };
    for pid in pids {
        match info.get(&pid) {
            Some(&(_, pgid)) if safe_group(pgid) => {
                listeners::signal(pgid, true, sig);
            }
            _ => {
                listeners::signal(pid, false, sig);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(pid: u32, port: u16) -> Listener {
        Listener {
            pid,
            ppid: 1,
            port,
            cwd: "/src/wt".into(),
        }
    }

    fn quiet() -> Inputs {
        Inputs {
            events_live: true,
            watched: true,
            stacks: true,
            ..Inputs::default()
        }
    }

    /// With the events stream up and nothing moving, docker is asked once
    /// a minute and lsof every 30 s with a client (a minute without); a
    /// poke, an event or a stack starting or stopping asks at once.
    #[test]
    fn docker_and_lsof_are_asked_only_when_something_can_have_changed() {
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let fresh = PollState {
            docker_at: Some(t0),
            docker_failed_at: None,
            listeners_at: Some(t0),
        };
        assert_eq!(fresh.due(at(3), quiet()), Due::default(), "nothing changed");
        assert_eq!(
            fresh.due(at(30), quiet()),
            Due {
                docker: false,
                listeners: true
            },
            "lsof at 30 s with a client"
        );
        let alone = Inputs {
            watched: false,
            ..quiet()
        };
        assert!(!fresh.due(at(30), alone).listeners, "a minute with none");
        assert!(fresh.due(at(60), alone).listeners);
        assert!(fresh.due(at(60), quiet()).docker, "the safety net");
        let event = Inputs {
            docker_changed: true,
            ..quiet()
        };
        assert_eq!(
            fresh.due(at(3), event),
            Due {
                docker: true,
                listeners: false
            }
        );
        let poke = Inputs {
            poked: true,
            ..quiet()
        };
        assert_eq!(
            fresh.due(at(1), poke),
            Due {
                docker: true,
                listeners: true
            }
        );
        let moving = Inputs {
            transition: true,
            ..quiet()
        };
        assert_eq!(
            fresh.due(at(3), moving),
            Due {
                docker: true,
                listeners: true
            }
        );
        let no_stream = Inputs {
            events_live: false,
            ..quiet()
        };
        assert!(fresh.due(at(3), no_stream).docker, "polled while unheard");
        assert_eq!(
            PollState::default().due(t0, quiet()),
            Due {
                docker: true,
                listeners: true
            },
            "the first look asks both"
        );
    }

    /// With no checkout that can have a stack, lsof is never asked and
    /// docker only when poked: no safety net, nothing for the stream's
    /// absence to make up for.
    #[test]
    fn with_no_stacks_lsof_is_never_asked_and_docker_only_when_poked() {
        let t0 = Instant::now();
        let none = Inputs {
            stacks: false,
            events_live: false,
            ..quiet()
        };
        let stale = PollState::default();
        assert_eq!(stale.due(t0, none), Due::default(), "nothing to look for");
        let poke = Inputs {
            poked: true,
            ..none
        };
        assert_eq!(
            stale.due(t0, poke),
            Due {
                docker: true,
                listeners: false
            }
        );
    }

    /// A checkout can have a stack when its runs served a port, it has a
    /// compose file, or docker lists containers running from it.
    #[test]
    fn a_stack_can_be_about_only_with_ports_a_compose_file_or_containers() {
        let wt = |path: &str| Worktree {
            id: WorktreeId(path.into()),
            project_id: orion_core::ProjectId("p".into()),
            path: path.into(),
            branch: "main".into(),
            is_main: false,
            sort_order: 0,
        };
        let plain = [(wt("/src/a"), vec![]), (wt("/src/b"), vec![])];
        let no_compose = |_: &Path| false;
        assert!(!any_stack_in(&plain, None, no_compose));
        assert!(!any_stack_in(
            &[],
            Some(&compose(Path::new("/src/a"), 2, 9)),
            no_compose
        ));
        assert!(any_stack_in(
            &plain,
            Some(&compose(Path::new("/src/a"), 2, 9)),
            no_compose
        ));
        assert!(
            !any_stack_in(
                &plain,
                Some(&compose(Path::new("/src/a"), 0, 9)),
                no_compose
            ),
            "stopped containers"
        );
        assert!(
            !any_stack_in(
                &plain,
                Some(&compose(Path::new("/elsewhere"), 2, 9)),
                no_compose
            ),
            "nobody's containers"
        );
        let served = [(wt("/src/a"), vec![3000])];
        assert!(any_stack_in(&served, None, no_compose));
        assert!(any_stack_in(&plain, None, |p: &Path| p == Path::new("/src/b")));
    }

    /// Docker failing is left alone for a while, unless poked.
    #[test]
    fn a_failed_docker_backs_off_but_a_poke_asks_anyway() {
        let t0 = Instant::now();
        let failed = PollState {
            docker_at: Some(t0),
            docker_failed_at: Some(t0),
            listeners_at: Some(t0),
        };
        let no_stream = Inputs {
            events_live: false,
            ..quiet()
        };
        assert!(!failed.due(t0 + Duration::from_secs(10), no_stream).docker);
        assert!(failed.due(t0 + UNREACHABLE_BACKOFF, no_stream).docker);
        let poke = Inputs {
            poked: true,
            ..no_stream
        };
        assert!(failed.due(t0 + Duration::from_secs(1), poke).docker);
    }

    #[test]
    fn the_events_stream_restarts_doubling_to_five_minutes() {
        assert_eq!(events_retry_wait(0), EVENTS_RETRY_MIN);
        assert_eq!(events_retry_wait(1), Duration::from_secs(4));
        assert_eq!(events_retry_wait(3), Duration::from_secs(16));
        assert_eq!(events_retry_wait(10), EVENTS_RETRY_MAX);
        assert_eq!(events_retry_wait(u32::MAX), EVENTS_RETRY_MAX);
    }

    #[test]
    fn the_events_stream_follows_container_changes_but_no_healthcheck_runs() {
        let args = docker_events_args();
        assert_eq!(args[..3], ["events", "--filter", "type=container"]);
        assert!(args.contains(&"event=die".to_string()));
        assert!(args.contains(&"event=health_status".to_string()));
        assert!(!args.iter().any(|a| a.contains("exec_")));
        assert_eq!(args[args.len() - 2..], ["--format", "{{.Action}}"]);
        assert!(is_container_change("start"));
        assert!(is_container_change("health_status: healthy"));
        assert!(!is_container_change("exec_start: /bin/sh -c curl"));
        assert!(!is_container_change("  "));
    }

    /// The idle clock runs from the later of the stack first seen up and
    /// the checkout's last activity.
    #[test]
    fn a_stack_is_idle_an_hour_after_the_last_thing_happened() {
        let hour = Duration::from_secs(3600);
        let min = 60_000;
        let now = 1_000 * min;
        assert!(
            !idle_past(now, now - 30 * min, None, hour),
            "up half an hour"
        );
        assert!(
            idle_past(now, now - 60 * min, None, hour),
            "up an hour, nothing since"
        );
        assert!(
            !idle_past(now, now - 300 * min, Some(now - 10 * min), hour),
            "used ten minutes ago"
        );
        assert!(idle_past(now, now - 300 * min, Some(now - 61 * min), hour));
        assert!(
            !idle_past(now, now - 5 * min, Some(now - 300 * min), hour),
            "activity before it came up doesn't count against it"
        );
        assert_eq!(
            idle_for(now, idle_since(now - 300 * min, Some(now - 61 * min))),
            Duration::from_secs(61 * 60)
        );
    }

    /// A stack up past the idle span with nothing happening is stopped as
    /// `⌘⇧S` stops it, and every client hears it; one in use is left up.
    #[tokio::test]
    async fn an_idle_stack_is_stopped_and_one_in_use_is_left_up() {
        let daemon = test_daemon();
        let (dir, w) = run_worktree(&daemon);
        let d = dir.path();
        let gone_pid = u32::MAX;
        let up = |daemon: &Arc<Daemon>| {
            resolve(
                daemon,
                Some(&compose(d, 3, 9_999)),
                Some(&[at(d, gone_pid, 3000)]),
            );
        };
        up(&daemon);
        assert!(matches!(
            daemon.stacks.status_of(&w.id),
            StackStatus::Up { .. }
        ));
        let hour = Some(Duration::from_secs(3600));
        let now = orion_core::clock::now_ms();
        let mut events = daemon.events.subscribe();

        // Just used: left up half an hour on.
        daemon.stacks.note_activity(&w.id);
        stop_idle_stacks(&daemon, now + 30 * 60_000, hour);
        up(&daemon);
        assert!(matches!(
            daemon.stacks.status_of(&w.id),
            StackStatus::Up { .. }
        ));
        // Off: never.
        stop_idle_stacks(&daemon, now + 10 * 3_600_000, None);
        up(&daemon);
        assert!(matches!(
            daemon.stacks.status_of(&w.id),
            StackStatus::Up { .. }
        ));

        // An hour and more with nothing: stopped, and said.
        stop_idle_stacks(&daemon, now + 61 * 60_000, hour);
        up(&daemon);
        assert!(
            matches!(daemon.stacks.status_of(&w.id), StackStatus::Stopping { .. }),
            "{:?}",
            daemon.stacks.status_of(&w.id)
        );
        let mut said = Vec::new();
        while let Ok(ev) = events.try_recv() {
            if let ServerEvent::Notice { text } = ev {
                said.push(text);
            }
        }
        assert!(
            said.iter()
                .any(|m| m.starts_with("stopped idle stack for ")),
            "{said:?}"
        );
    }

    #[test]
    fn ports_put_the_printed_ones_first_and_drop_what_doesnt_listen() {
        let listening = [l(1, 5173), l(1, 3001), l(2, 3000)];
        assert_eq!(order_ports(&[3000, 9999], &listening), [3000, 3001, 5173]);
        assert_eq!(order_ports(&[], &listening), [3000, 3001, 5173]);
        assert!(order_ports(&[3000], &[]).is_empty());
    }

    #[test]
    fn a_path_belongs_to_the_deepest_checkout_holding_it() {
        let (wt, nested, sibling) = (
            Path::new("/src/wt"),
            Path::new("/src/wt/.claude/worktrees/x"),
            Path::new("/src/wt-2"),
        );
        let all = [wt, nested, sibling];
        assert_eq!(
            deepest_holding(&all, Path::new("/src/wt/apps/os")),
            Some(wt)
        );
        assert_eq!(
            deepest_holding(&all, Path::new("/src/wt/.claude/worktrees/x/a")),
            Some(nested)
        );
        assert_eq!(deepest_holding(&all, Path::new("/src/wt-2")), Some(sibling));
        assert_eq!(deepest_holding(&all, Path::new("/elsewhere")), None);
    }

    #[test]
    fn a_checkouts_containers_sum_over_its_projects() {
        let stack = |project: &str, dir: &str, running, starting, up| Stack {
            project: project.into(),
            dirs: vec![dir.into()],
            running,
            total: 3,
            starting,
            newest_up_secs: up,
        };
        let all = [
            stack("a", "/src/wt", 2, 0, Some(500)),
            stack("b", "/src/wt/sub", 1, 1, Some(5)),
            stack("c", "/src/other", 3, 0, Some(1)),
        ];
        let wt = Path::new("/src/wt");
        let sum = containers_of(&all, wt, &[wt, Path::new("/src/other")]);
        assert_eq!(
            sum,
            ContainerFacts {
                running: 3,
                starting: 1,
                newest_up_secs: Some(5)
            }
        );
    }

    use crate::registry::tests::{run_config, run_worktree, start_run_until, test_daemon};

    fn at(dir: &Path, pid: u32, port: u16) -> Listener {
        Listener {
            pid,
            ppid: 1,
            port,
            cwd: dir.join("apps/web"),
        }
    }

    fn compose(dir: &Path, running: u16, up_secs: u64) -> Vec<Stack> {
        vec![Stack {
            project: "demo".into(),
            dirs: vec![dir.to_path_buf()],
            running,
            total: 3,
            starting: 0,
            newest_up_secs: Some(up_secs),
        }]
    }

    /// A stack started outside orion: a listener in the checkout is its
    /// app while its containers run, or on a port its runs served before;
    /// a stray listener with neither is nobody's stack. Containers alone
    /// are starting while fresh, then down. Docker unknown is not down.
    #[tokio::test]
    async fn a_stack_started_outside_orion_is_read_from_what_runs() {
        let daemon = test_daemon();
        let (dir, w) = run_worktree(&daemon);
        let d = dir.path();
        let status = |docker: Option<&[Stack]>, scan: &[Listener]| {
            resolve(&daemon, docker, Some(scan));
            daemon.stacks.status_of(&w.id)
        };
        let up = |ports: &[u16]| StackStatus::Up {
            ports: ports.to_vec(),
            outside: true,
        };
        assert_eq!(status(Some(&[]), &[]), StackStatus::Down);
        assert_eq!(
            status(Some(&[]), &[at(d, 9, 51204)]),
            StackStatus::Down,
            "a stray listener"
        );
        assert_eq!(
            status(Some(&compose(d, 3, 9_999)), &[at(d, 9, 3000)]),
            up(&[3000])
        );
        assert_eq!(
            status(None, &[at(d, 9, 3000)]),
            StackStatus::Down,
            "unknown containers, no known port"
        );
        daemon.store.set_known_ports(&w.id, &[3000]).unwrap();
        assert_eq!(
            status(None, &[at(d, 9, 3000)]),
            up(&[3000]),
            "a port its runs served"
        );
        assert_eq!(
            status(Some(&compose(d, 3, 10)), &[]),
            StackStatus::Starting { outside: true }
        );
        assert_eq!(status(Some(&compose(d, 3, 9_999)), &[]), StackStatus::Down);
        // What the subscriber hears: only checkouts that are not down.
        resolve(
            &daemon,
            Some(&compose(d, 3, 9_999)),
            Some(&[at(d, 9, 3000)]),
        );
        let ServerEvent::StackStatuses { statuses } = daemon.stacks.current() else {
            panic!("statuses");
        };
        assert_eq!(statuses, vec![(w.id.clone(), up(&[3000]))]);
    }

    /// orion's own run is starting until something of it listens, then up
    /// with its ports — which the checkout remembers — and stopping once
    /// a stop is asked for.
    #[tokio::test]
    async fn orions_run_is_starting_then_up_then_stopping() {
        let daemon = test_daemon();
        let (dir, w) = run_worktree(&daemon);
        let d = dir.path();
        let cfg = run_config(
            d,
            "trap 'sleep 5' INT; echo 'http://localhost:3000'; echo started; while :; do sleep 0.1; done",
        );
        start_run_until(&daemon, &w.id, &cfg, "started").await;
        resolve(&daemon, Some(&[]), Some(&[]));
        assert_eq!(
            daemon.stacks.status_of(&w.id),
            StackStatus::Starting { outside: false }
        );
        resolve(&daemon, Some(&[]), Some(&[at(d, 9, 5173), at(d, 9, 3000)]));
        assert_eq!(
            daemon.stacks.status_of(&w.id),
            StackStatus::Up {
                ports: vec![3000, 5173],
                outside: false
            },
            "the printed port first"
        );
        assert_eq!(daemon.store.known_ports(&w.id).unwrap(), vec![3000, 5173]);
        // Docker unknown and lsof unknown: what the run printed stands in.
        resolve(&daemon, None, None);
        assert_eq!(daemon.stacks.status_of(&w.id).ports(), [3000]);
        daemon.stop_run(&w.id).unwrap();
        resolve(&daemon, Some(&[]), Some(&[]));
        assert!(matches!(
            daemon.stacks.status_of(&w.id),
            StackStatus::Stopping { kill_at: Some(_) }
        ));
        daemon.stop_run(&w.id).unwrap();
        resolve(&daemon, Some(&[]), Some(&[]));
        assert_eq!(daemon.stacks.status_of(&w.id), StackStatus::Down);
    }

    /// A stop of a stack orion didn't start reads as stopping until
    /// nothing of it is left, then down.
    #[tokio::test]
    async fn an_outside_stop_is_stopping_until_nothing_is_left() {
        let daemon = test_daemon();
        let (dir, w) = run_worktree(&daemon);
        let d = dir.path();
        // A pid no process can have: `signal` refuses what it can't name,
        // so the stop signals nothing real.
        let gone_pid = u32::MAX;
        resolve(
            &daemon,
            Some(&compose(d, 3, 9_999)),
            Some(&[at(d, gone_pid, 3000)]),
        );
        assert!(daemon.stacks.status_of(&w.id).is_outside());
        daemon.stop_run(&w.id).unwrap();
        resolve(
            &daemon,
            Some(&compose(d, 3, 9_999)),
            Some(&[at(d, gone_pid, 3000)]),
        );
        assert!(matches!(
            daemon.stacks.status_of(&w.id),
            StackStatus::Stopping { .. }
        ));
        resolve(&daemon, Some(&compose(d, 0, 9_999)), Some(&[]));
        assert_eq!(daemon.stacks.status_of(&w.id), StackStatus::Down);
        assert!(daemon.stacks.outside_stops.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_listing_sums_each_project() {
        let stub = crate::containers::tests::Stub::new(
            "printf 'a\\t/src/a\\trunning\\tUp 5 seconds\\na\\t/src/a\\texited\\tExited (0)\\nb\\t/src/b\\texited\\tExited (0)\\n\\t\\trunning\\n'",
        );
        let stacks = poll(Some(&stub.docker())).await.expect("docker answered");
        assert_eq!(stacks.len(), 2);
        assert_eq!((stacks[0].running, stacks[0].total), (1, 2));
        assert_eq!(stacks[0].newest_up_secs, Some(5));
        assert_eq!((stacks[1].running, stacks[1].total), (0, 1));
    }

    #[tokio::test]
    async fn docker_out_of_reach_is_unknown_and_why() {
        let stub = crate::containers::tests::Stub::new(
            "echo 'Cannot connect to the Docker daemon' >&2\nexit 1",
        );
        assert_eq!(
            poll(Some(&stub.docker())).await,
            Err("Cannot connect to the Docker daemon".to_string())
        );
        assert_eq!(poll(None).await, Err(compose::NO_DOCKER_CLI.to_string()));
    }
}
