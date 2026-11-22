//! How fresh each kind of answer has to be: the one table the RESOLVER
//! reads its timings from. A visit refetches only past `stale_after`; a
//! key the user is in refetches every `live_every` while the window is
//! focused; a failure waits out a doubling backoff. Nothing here runs on
//! a timer by itself — a key with no one looking at it and no automation
//! asking for it is never fetched, whatever its policy says.

use std::time::Duration;

use super::key::QueryKey;

/// One kind of key's timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// How old an answer may be when the key comes on screen
    /// ([`Seen::Shown`](super::Seen::Shown)) and still stand: a visit
    /// within this of the last success fetches nothing.
    pub stale_after: Duration,
    /// The same, for a key the user is in ([`Seen::Focused`](super::Seen::Focused)):
    /// opening the PULL REQUESTS MODAL wants a fresher list than glancing at
    /// the sidebar does.
    pub focused_stale_after: Duration,
    /// How often a key the user is in refetches while the window is
    /// focused and nobody is away. `None` for almost every kind: displayed
    /// data refreshes when you look at it, not on a beat.
    pub live_every: Option<Duration>,
    /// The live beat instead, while something watches the key and
    /// invalidates it on every change ([`Resolver::set_watched`](super::Resolver::set_watched)):
    /// the selected checkout under THE GIT WATCHER, whose beat is only a
    /// backstop for a change the watcher missed. `None` where nothing
    /// watches.
    pub watched_every: Option<Duration>,
    /// A failure's backoff: the first retry waits `base`, each further
    /// failure doubles it, up to `cap`. Visits and focus never shorten it;
    /// only an explicit refresh (`⌘R`, `Shift+R`) does.
    pub backoff: (Duration, Duration),
}

const fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

const fn mins(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

/// The usual backoff for a network fetch: half a minute, doubling to ten.
const NETWORK_BACKOFF: (Duration, Duration) = (secs(30), mins(10));

/// The policy for `key`'s kind.
pub fn policy(key: &QueryKey) -> Policy {
    use QueryKey::*;
    let shown = |stale: Duration| Policy {
        stale_after: stale,
        focused_stale_after: stale,
        live_every: None,
        watched_every: None,
        backoff: NETWORK_BACKOFF,
    };
    match key {
        // One project's list: the sidebar's group at a minute, the modal's
        // at half one, and a minute's beat while the modal is open and
        // looked at — what keeps its checks moving while CI runs.
        OpenPrs(_) => Policy {
            stale_after: secs(60),
            focused_stale_after: secs(30),
            live_every: Some(secs(60)),
            ..shown(secs(60))
        },
        PrExtras(_) => Policy {
            live_every: Some(secs(60)),
            ..shown(secs(30))
        },
        // Merged and closed rows never change once they are there.
        PrTails(_) => shown(mins(10)),
        // One page: read on arrival when older than a minute, every half
        // minute while its checks run and you are on it (`scope`), and an
        // automation's page that keeps failing backs off to half an hour.
        PrDetail(_) => Policy {
            live_every: Some(secs(30)),
            backoff: (secs(60), mins(30)),
            ..shown(secs(60))
        },
        // A branch no list has: one ask says merged, closed or none, and
        // that holds for a while.
        BranchPr(_) => Policy {
            backoff: (secs(60), mins(30)),
            ..shown(mins(5))
        },
        MyPrs | ReviewRequests => shown(secs(60)),
        // A project's Linear lists — the heaviest Linear query: the
        // selected project's prefetch at five minutes, the open view's at
        // one. Edits read their own issue again, not the lists.
        LinearView(_) => Policy {
            focused_stale_after: secs(60),
            ..shown(mins(5))
        },
        LinearInbox(_) => shown(secs(60)),
        // The builders' intake: its beat is the background need's, and a
        // failure waits that beat out anyway (`linear_builder::intake_due`).
        LinearIntake(_) => shown(secs(60)),
        // Teams, members and states: once a launch.
        LinearTeams(_) => shown(Duration::MAX),
        // Undocumented endpoints that 429 a client that keeps asking:
        // ten minutes fresh, and a slow backoff.
        Usage(_) => Policy {
            backoff: (mins(5), mins(60)),
            ..shown(crate::usage::STALE)
        },
        Calendar => shown(crate::microsoft::calendar::DEFAULT_INTERVAL),
        // Each look is scheduled by its meeting (`LOOKS_AFTER`), and a
        // failed one counts as a look.
        Transcript(_) => shown(mins(2)),
        // A checkout's read. The selected one is read every time it is
        // selected and whenever the GIT WATCHER says it changed; while
        // somebody looks, a backstop beat reads it every 5 min under the
        // watcher, and every 15 s where the watcher failed to start. A
        // band is read as it comes on screen when its last read is a
        // minute old, and again when an agent's turn in it ends
        // (`event_loop`'s `StatusChanged`) — never on a beat. Nothing here
        // beats while IDLE.
        GitStatus(_) => Policy {
            stale_after: secs(60),
            focused_stale_after: Duration::ZERO,
            live_every: Some(secs(15)),
            watched_every: Some(mins(5)),
            backoff: (secs(2), mins(1)),
        },
        // A project's fetch of origin: on a visit once `base_fetch` old.
        BaseFetch(_) => shown(crate::base_sync::fresh_for().unwrap_or(Duration::MAX)),
    }
}

impl Policy {
    /// How long a failure that is the `fails`-th in a row holds the key
    /// off: `base` after the first, doubling each time, never past `cap`.
    pub fn backoff_after(&self, fails: u32) -> Duration {
        let (base, cap) = self.backoff;
        orion_core::backoff::doubling(base, cap, fails)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{ProjectId, WorktreeId};

    #[test]
    fn a_backoff_starts_at_its_base_and_doubles_to_its_cap() {
        let p = policy(&QueryKey::OpenPrs(ProjectId("p".into())));
        assert_eq!(p.backoff_after(1), secs(30));
        assert_eq!(p.backoff_after(2), secs(60));
        assert_eq!(p.backoff_after(3), secs(120));
        assert_eq!(p.backoff_after(10), mins(10), "capped");
        assert_eq!(p.backoff_after(u32::MAX), mins(10), "never overflows");
    }

    /// The sidebar's list and a branch lookup refresh on a visit only;
    /// the modal's list refreshes on a beat while it is looked at.
    #[test]
    fn displayed_github_data_has_no_beat_outside_the_modal() {
        let list = policy(&QueryKey::OpenPrs(ProjectId("p".into())));
        assert_eq!(list.stale_after, secs(60));
        assert_eq!(list.focused_stale_after, secs(30));
        assert_eq!(list.live_every, Some(secs(60)));
        let branch = policy(&QueryKey::BranchPr(WorktreeId("w".into())));
        assert_eq!(branch.stale_after, mins(5));
        assert_eq!(branch.live_every, None);
        assert_eq!(branch.watched_every, None);
    }

    /// The selected checkout is read on every visit, and on a beat while
    /// looked at: 15 s unwatched, 5 min under the watcher. A band is read
    /// as it comes on screen when a minute old, and never on a beat.
    #[test]
    fn a_checkouts_read_beats_only_while_looked_at() {
        let git = policy(&QueryKey::GitStatus(WorktreeId("w".into())));
        assert_eq!(git.focused_stale_after, Duration::ZERO);
        assert_eq!(git.live_every, Some(secs(15)));
        assert_eq!(git.watched_every, Some(mins(5)));
        assert_eq!(git.stale_after, secs(60));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        crate::config::with_config_path(path.clone(), || {
            let base = policy(&QueryKey::BaseFetch(ProjectId("p".into())));
            assert_eq!(base.stale_after, mins(30), "base_fetch's default");
            assert_eq!(base.live_every, None);
            std::fs::write(&path, r#"{"base_fetch": "2h"}"#).unwrap();
            let base = policy(&QueryKey::BaseFetch(ProjectId("p".into())));
            assert_eq!(base.stale_after, mins(120), "as the setting says");
        });
    }
}
