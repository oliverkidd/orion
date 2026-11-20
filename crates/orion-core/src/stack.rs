//! STACK STATUS: whether a checkout's stack — the app its run serves and
//! the compose containers under it — is up, coming up, going down, or
//! down. One answer per checkout, the DAEMON's, derived afresh from what
//! is true right now ([`StackFacts`]) by [`derive`]: no timers, no flags,
//! no history, so the same facts always give the same answer however the
//! stack was started or stopped — `⌘⇧S`, `orion run`, an agent's own
//! shell, a terminal outside orion, a daemon restart mid-run.
//!
//! Every reader shows this and nothing else: the band's `⬡` (green `Up`,
//! amber `Starting`/`Stopping`, none `Down`), the run line (drawn only
//! while the stack is not down), `⌘⇧S`, `orion run --status`.

use serde::{Deserialize, Serialize};

/// A checkout's stack, as every client shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StackStatus {
    /// Nothing serving, nothing coming up.
    Down,
    /// Coming up: orion's run has not listened yet, or containers someone
    /// started are still starting. `outside` when orion has no run here.
    Starting { outside: bool },
    /// Serving `ports` — the app's, first the one its run printed first.
    /// `outside` when it was started without orion (an agent's shell,
    /// another terminal): there is no run terminal behind it.
    Up { ports: Vec<u16>, outside: bool },
    /// Going down: a stop is under way, killed at `kill_at` (epoch ms) if
    /// it hasn't finished by then.
    Stopping { kill_at: Option<i64> },
}

impl StackStatus {
    pub fn is_down(&self) -> bool {
        matches!(self, Self::Down)
    }

    /// Up or coming up: what a stop stops.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Starting { .. } | Self::Up { .. })
    }

    /// Started without orion: no run terminal behind it.
    pub fn is_outside(&self) -> bool {
        matches!(
            self,
            Self::Starting { outside: true } | Self::Up { outside: true, .. }
        )
    }

    pub fn ports(&self) -> &[u16] {
        match self {
            Self::Up { ports, .. } => ports,
            _ => &[],
        }
    }

    /// One word for a CLI line or a log: `down`, `starting`, `up`,
    /// `stopping`.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Starting { .. } => "starting",
            Self::Up { .. } => "up",
            Self::Stopping { .. } => "stopping",
        }
    }
}

/// What orion's own run in the checkout is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunFact {
    /// No run of orion's alive here.
    #[default]
    None,
    /// A start waits on the checkout's setup to finish.
    Queued,
    /// Its PTY is alive, spawned at `since_ms`.
    Alive { since_ms: i64 },
    /// A stop sent it its `^C`; it is killed at `kill_at` if still there.
    Stopping { kill_at: i64 },
}

/// The checkout's compose containers, from docker's listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContainerFacts {
    /// Containers running …
    pub running: u16,
    /// … of which this many are still starting (health `starting`, or
    /// `created` / `restarting`).
    pub starting: u16,
    /// How long ago the most recently started running container started,
    /// in seconds; None with none running.
    pub newest_up_secs: Option<u64>,
}

/// Everything [`derive`] reads about one checkout, gathered in one place
/// by the DAEMON.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackFacts {
    pub run: RunFact,
    /// A stop orion is carrying out on a stack it did not start, killed
    /// at this epoch ms.
    pub stopping_outside: Option<i64>,
    /// The checkout's containers. None when docker didn't answer: unknown
    /// is never read as down.
    pub containers: Option<ContainerFacts>,
    /// TCP ports listened on by a process that belongs to the stack (see
    /// the DAEMON's `listeners`), the app's first.
    pub listeners: Vec<u16>,
    /// The checkout's only run target is `docker compose up`: its
    /// containers are the app.
    pub compose_only: bool,
    pub now_ms: i64,
}

/// How long orion's run may go without listening before it counts as up
/// anyway: a worker, or an app whose port is inside a container.
pub const UP_WITHOUT_PORT_MS: i64 = 30_000;

/// How long containers someone started outside orion count as a stack
/// coming up while no app listens yet: the app follows its containers.
pub const OUTSIDE_START_GRACE_SECS: u64 = 120;

/// The status `f` describes. Pure; the rules, in order:
///
/// 1. A stop under way is `Stopping`.
/// 2. A start waiting on setup is `Starting`.
/// 3. orion's run alive: `Up` once anything of the stack listens — or
///    once it has run [`UP_WITHOUT_PORT_MS`] with its containers started
///    — else `Starting`.
/// 4. No run: a listener of the stack's is `Up`, started outside orion;
///    a compose-only checkout's started containers are too.
/// 5. Containers that started within [`OUTSIDE_START_GRACE_SECS`], or are
///    still starting, with no app yet: `Starting`, outside orion.
/// 6. Anything else is `Down` — containers left running with no app
///    included: the stack is not up.
pub fn derive(f: &StackFacts) -> StackStatus {
    if let RunFact::Stopping { kill_at } = f.run {
        return StackStatus::Stopping {
            kill_at: Some(kill_at),
        };
    }
    if let Some(kill_at) = f.stopping_outside {
        return StackStatus::Stopping {
            kill_at: Some(kill_at),
        };
    }
    let starting = f.containers.is_some_and(|c| c.starting > 0);
    match f.run {
        RunFact::Queued => StackStatus::Starting { outside: false },
        RunFact::Alive { since_ms } => {
            if !f.listeners.is_empty() {
                StackStatus::Up {
                    ports: f.listeners.clone(),
                    outside: false,
                }
            } else if !starting && f.now_ms - since_ms >= UP_WITHOUT_PORT_MS {
                StackStatus::Up {
                    ports: Vec::new(),
                    outside: false,
                }
            } else {
                StackStatus::Starting { outside: false }
            }
        }
        RunFact::None | RunFact::Stopping { .. } => {
            if !f.listeners.is_empty() {
                return StackStatus::Up {
                    ports: f.listeners.clone(),
                    outside: true,
                };
            }
            let Some(c) = f.containers.filter(|c| c.running > 0) else {
                return StackStatus::Down;
            };
            if f.compose_only && c.starting == 0 {
                return StackStatus::Up {
                    ports: Vec::new(),
                    outside: true,
                };
            }
            let fresh = c
                .newest_up_secs
                .is_some_and(|s| s < OUTSIDE_START_GRACE_SECS);
            if c.starting > 0 || fresh {
                StackStatus::Starting { outside: true }
            } else {
                StackStatus::Down
            }
        }
    }
}

/// The id a stack started outside orion goes by on its run line: there is
/// no terminal behind it, so nothing attaches to it.
pub fn outside_line_id(worktree: &crate::WorktreeId) -> crate::TerminalId {
    crate::TerminalId(format!("{OUTSIDE_PREFIX}{}", worktree.0))
}

/// Whether `id` is an [`outside_line_id`].
pub fn is_outside_line(id: &crate::TerminalId) -> bool {
    id.0.starts_with(OUTSIDE_PREFIX)
}

const OUTSIDE_PREFIX: &str = "outside-stack:";

/// What a stack started outside orion's run line says where a run's
/// command goes.
pub const OUTSIDE_LINE_TEXT: &str = "started outside orion";

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn facts() -> StackFacts {
        StackFacts {
            containers: Some(ContainerFacts::default()),
            now_ms: NOW,
            ..Default::default()
        }
    }

    fn up(ports: &[u16], outside: bool) -> StackStatus {
        StackStatus::Up {
            ports: ports.to_vec(),
            outside,
        }
    }

    fn containers(running: u16, starting: u16, newest_up_secs: u64) -> Option<ContainerFacts> {
        Some(ContainerFacts {
            running,
            starting,
            newest_up_secs: Some(newest_up_secs),
        })
    }

    #[test]
    fn nothing_is_down() {
        assert_eq!(derive(&facts()), StackStatus::Down);
        assert_eq!(
            derive(&StackFacts {
                containers: None,
                ..facts()
            }),
            StackStatus::Down
        );
    }

    #[test]
    fn a_stop_under_way_is_stopping_whatever_else_is_true() {
        let f = StackFacts {
            run: RunFact::Stopping { kill_at: 5 },
            listeners: vec![3000],
            containers: containers(3, 0, 999),
            ..facts()
        };
        assert_eq!(derive(&f), StackStatus::Stopping { kill_at: Some(5) });
        let f = StackFacts {
            stopping_outside: Some(7),
            listeners: vec![3000],
            ..facts()
        };
        assert_eq!(derive(&f), StackStatus::Stopping { kill_at: Some(7) });
    }

    #[test]
    fn a_start_behind_setup_is_starting() {
        let f = StackFacts {
            run: RunFact::Queued,
            ..facts()
        };
        assert_eq!(derive(&f), StackStatus::Starting { outside: false });
    }

    #[test]
    fn orions_run_is_starting_until_it_listens() {
        let alive = |since_ms| StackFacts {
            run: RunFact::Alive { since_ms },
            ..facts()
        };
        assert_eq!(
            derive(&alive(NOW - 1_000)),
            StackStatus::Starting { outside: false }
        );
        let listening = StackFacts {
            listeners: vec![3000, 3001],
            ..alive(NOW - 1_000)
        };
        assert_eq!(derive(&listening), up(&[3000, 3001], false));
    }

    #[test]
    fn a_run_that_never_listens_is_up_after_a_while_unless_containers_still_start() {
        let f = StackFacts {
            run: RunFact::Alive {
                since_ms: NOW - UP_WITHOUT_PORT_MS,
            },
            ..facts()
        };
        assert_eq!(derive(&f), up(&[], false));
        let booting = StackFacts {
            containers: containers(3, 1, 10),
            ..f
        };
        assert_eq!(derive(&booting), StackStatus::Starting { outside: false });
    }

    #[test]
    fn a_listener_with_no_run_is_up_outside_orion() {
        let f = StackFacts {
            listeners: vec![3000],
            ..facts()
        };
        assert_eq!(derive(&f), up(&[3000], true));
        // Docker unreachable changes nothing about what listens.
        let f = StackFacts {
            containers: None,
            ..f
        };
        assert_eq!(derive(&f), up(&[3000], true));
    }

    #[test]
    fn containers_alone_are_starting_while_fresh_then_down() {
        let fresh = StackFacts {
            containers: containers(5, 0, 10),
            ..facts()
        };
        assert_eq!(derive(&fresh), StackStatus::Starting { outside: true });
        let booting = StackFacts {
            containers: containers(5, 2, 9_999),
            ..facts()
        };
        assert_eq!(derive(&booting), StackStatus::Starting { outside: true });
        let left = StackFacts {
            containers: containers(5, 0, OUTSIDE_START_GRACE_SECS),
            ..facts()
        };
        assert_eq!(
            derive(&left),
            StackStatus::Down,
            "containers with no app are not a stack"
        );
    }

    #[test]
    fn a_compose_only_checkouts_containers_are_its_app() {
        let f = StackFacts {
            compose_only: true,
            containers: containers(2, 0, 9_999),
            ..facts()
        };
        assert_eq!(derive(&f), up(&[], true));
        let booting = StackFacts {
            containers: containers(2, 1, 1),
            ..f
        };
        assert_eq!(derive(&booting), StackStatus::Starting { outside: true });
    }

    #[test]
    fn outside_line_ids_round_trip() {
        let w = crate::WorktreeId("w1".into());
        assert!(is_outside_line(&outside_line_id(&w)));
        assert!(!is_outside_line(&crate::TerminalId("t1".into())));
    }

    #[test]
    fn status_round_trips_through_the_wire() {
        for status in [
            StackStatus::Down,
            StackStatus::Starting { outside: true },
            up(&[3000], false),
            StackStatus::Stopping { kill_at: Some(9) },
        ] {
            let bytes = rmp_serde::to_vec(&status).unwrap();
            let back: StackStatus = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(back, status);
        }
    }
}
