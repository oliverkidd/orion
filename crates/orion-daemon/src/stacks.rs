//! STACK STATUS: every docker compose project on the machine — running or
//! stopped, started from a worktree orion knows or not — polled every few
//! seconds and broadcast as `StacksChanged` when it changes, for the band's
//! `⬡` and the Stacks modal. The modal's start / stop / take down run here
//! too (`ClientRequest::StackAction`).
//!
//! One poll serves every client, and goes on while none is attached so the
//! first frame of the next one is current. Docker out of reach — no CLI,
//! the engine stopped — is a state like any other, sent once; the poller
//! then asks less often until it answers again.

use crate::containers::{self, find_docker, COMPOSE_TIMEOUT, PS_TIMEOUT};
use crate::registry::Daemon;
use orion_core::compose::{self, Stack, StackVerb};
use orion_core::protocol::ServerEvent;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the poller leaves docker alone after it failed to answer: an
/// engine that is stopped stays stopped for a while.
const UNREACHABLE_BACKOFF: Duration = Duration::from_secs(30);

/// What the poller last saw, and the poke for a fresh look.
#[derive(Default)]
pub struct StackWatch {
    last: Mutex<Listing>,
    poke: tokio::sync::Notify,
}

/// One poll's answer: the stacks, or why there are none to read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub stacks: Option<Vec<Stack>>,
    pub error: Option<String>,
}

impl Listing {
    pub fn event(&self) -> ServerEvent {
        ServerEvent::StacksChanged {
            stacks: self.stacks.clone(),
            error: self.error.clone(),
        }
    }
}

impl StackWatch {
    /// The last listing, as the event a new subscriber is sent.
    pub fn current(&self) -> ServerEvent {
        self.last.lock().unwrap().event()
    }

    /// The stacks the last poll found; None while docker isn't answering.
    pub fn stacks(&self) -> Option<Vec<Stack>> {
        self.last.lock().unwrap().stacks.clone()
    }

    /// Ask the poller for a fresh listing now — after a verb changed one.
    pub fn poke(&self) {
        self.poke.notify_one();
    }

    /// Store `listing`: the event to broadcast when it differs from the
    /// last, None when nothing changed.
    fn store(&self, listing: Listing) -> Option<ServerEvent> {
        let mut last = self.last.lock().unwrap();
        if *last == listing {
            return None;
        }
        *last = listing;
        Some(last.event())
    }
}

/// The poller: runs until the daemon shuts down.
pub async fn watch(daemon: Arc<Daemon>, mut interval: tokio::time::Interval) {
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut failed_at: Option<Instant> = None;
    loop {
        let poked = tokio::select! {
            _ = daemon.shutdown.cancelled() => break,
            _ = interval.tick() => false,
            _ = daemon.stacks.poke.notified() => true,
        };
        if !poked && failed_at.is_some_and(|at| at.elapsed() < UNREACHABLE_BACKOFF) {
            continue;
        }
        let listing = poll(find_docker().as_deref()).await;
        failed_at = listing.stacks.is_none().then(Instant::now);
        if let Some(event) = daemon.stacks.store(listing) {
            daemon.broadcast(event);
        }
    }
}

/// Ask `docker` for every compose container and sum them into stacks.
pub async fn poll(docker: Option<&Path>) -> Listing {
    let Some(docker) = docker else {
        return Listing {
            stacks: None,
            error: Some(compose::NO_DOCKER_CLI.into()),
        };
    };
    match containers::run(docker, &compose::ps_state_args(), PS_TIMEOUT).await {
        Ok(out) => Listing {
            stacks: Some(compose::stacks(&compose::parse_ps_state(&out))),
            error: None,
        },
        Err(e) => Listing {
            stacks: None,
            error: Some(e),
        },
    }
}

/// Apply `verb` to compose project `project`, then have the poller look
/// again so every client sees the result.
pub async fn act(daemon: &Daemon, project: &str, verb: StackVerb) -> anyhow::Result<()> {
    let result = match find_docker() {
        Some(docker) => act_with(&docker, project, verb).await,
        None => Err(compose::NO_DOCKER_CLI.to_string()),
    };
    daemon.stacks.poke();
    result.map_err(|e| anyhow::anyhow!("couldn't {} {project}: {e}", verb.as_str()))
}

async fn act_with(docker: &Path, project: &str, verb: StackVerb) -> Result<(), String> {
    containers::run(docker, &verb.compose_args(project), COMPOSE_TIMEOUT).await?;
    tracing::info!(project, "compose project {}", verb.past_tense());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containers::tests::Stub;

    #[tokio::test]
    async fn a_listing_sums_each_project() {
        let stub = Stub::new(
            "printf 'a\\t/src/a\\trunning\\na\\t/src/a\\texited\\nb\\t/src/b\\texited\\n\\t\\trunning\\n'",
        );
        let listing = poll(Some(&stub.docker())).await;
        let stacks = listing.stacks.expect("docker answered");
        assert_eq!(listing.error, None);
        assert_eq!(stacks.len(), 2);
        assert_eq!((stacks[0].running, stacks[0].total), (1, 2));
        assert_eq!((stacks[1].running, stacks[1].total), (0, 1));
    }

    #[tokio::test]
    async fn docker_out_of_reach_is_no_stacks_and_why() {
        let stub = Stub::new("echo 'Cannot connect to the Docker daemon' >&2\nexit 1");
        let listing = poll(Some(&stub.docker())).await;
        assert_eq!(listing.stacks, None);
        assert_eq!(
            listing.error.as_deref(),
            Some("Cannot connect to the Docker daemon")
        );
        assert_eq!(
            poll(None).await.error.as_deref(),
            Some(compose::NO_DOCKER_CLI)
        );
    }

    #[tokio::test]
    async fn a_verb_runs_compose_and_surfaces_its_error() {
        let stub = Stub::new(
            "echo \"$*\" >> \"$(dirname \"$0\")/calls\"\n\
             [ \"$4\" = stop ] && { echo 'no such project' >&2; exit 1; }\nexit 0",
        );
        act_with(&stub.docker(), "p", StackVerb::Start)
            .await
            .unwrap();
        let err = act_with(&stub.docker(), "p", StackVerb::Stop)
            .await
            .unwrap_err();
        assert_eq!(err, "no such project");
        assert_eq!(stub.calls(), ["compose -p p start", "compose -p p stop"]);
    }

    #[test]
    fn only_a_changed_listing_is_news() {
        let watch = StackWatch::default();
        let listing = Listing {
            stacks: Some(vec![]),
            error: None,
        };
        assert!(
            watch.store(listing.clone()).is_some(),
            "first answer differs from unknown"
        );
        assert!(watch.store(listing).is_none());
        assert!(matches!(
            watch.current(),
            ServerEvent::StacksChanged { stacks: Some(s), error: None } if s.is_empty()
        ));
    }
}
