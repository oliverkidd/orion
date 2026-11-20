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
//! It polls while a client watches, a run is up, or a stack isn't down;
//! with none of those, docker and lsof are left alone, and the next to
//! subscribe pokes it for a fresh look (`server`). Docker out of reach
//! is unknown, never "down": the poller asks it less often until it
//! answers again, and reads the rest meanwhile.

use crate::containers::{self, find_docker, PS_TIMEOUT};
use crate::listeners::{self, Listener};
use crate::registry::Daemon;
use orion_core::compose::{self, Stack};
use orion_core::protocol::ServerEvent;
use orion_core::stack::{self, ContainerFacts, RunFact, StackFacts, StackStatus};
use orion_core::{Worktree, WorktreeId};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the poller leaves docker alone after it failed to answer: an
/// engine that is stopped stays stopped for a while.
const UNREACHABLE_BACKOFF: Duration = Duration::from_secs(30);

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
    watchers: Arc<std::sync::atomic::AtomicUsize>,
}

/// A client's subscription, counted while it lives.
pub struct Watching(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
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
        self.watchers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.poke();
        Watching(Arc::clone(&self.watchers))
    }

    fn watched(&self) -> bool {
        self.watchers.load(std::sync::atomic::Ordering::Relaxed) > 0
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

/// The poller: runs until the daemon shuts down.
pub async fn watch(daemon: Arc<Daemon>, mut interval: tokio::time::Interval) {
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut docker_failed_at: Option<Instant> = None;
    loop {
        let poked = tokio::select! {
            _ = daemon.shutdown.cancelled() => break,
            _ = interval.tick() => false,
            _ = daemon.stacks.poke.notified() => true,
        };
        if !daemon.stacks.watched() && !daemon.stacks.anything_up() && !daemon.any_run_live() {
            continue;
        }
        let backing_off =
            !poked && docker_failed_at.is_some_and(|at| at.elapsed() < UNREACHABLE_BACKOFF);
        let docker = if backing_off {
            None
        } else {
            let listed = poll(find_docker().as_deref()).await;
            docker_failed_at = listed.is_err().then(Instant::now);
            listed.ok()
        };
        let scan = listeners::scan().await;
        resolve(&daemon, docker.as_deref(), scan.as_deref());
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
