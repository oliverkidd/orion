//! CHORES: the loop's housekeeping, each on a deadline of its own — the
//! automations' turn, the knowledge beat, the factory's follow-ups, who
//! each Claude account is, the caches' writes to disk, and claiming
//! `sync.lock` from an orion that quit.
//!
//! None runs on a shared tick: each runs when it is due ([`period`]),
//! the loop sleeps until the earliest of them (and THE RESOLVER's
//! `next_wake`), and AWAY stretches the automations' own housekeeping six
//! to ten times over: what they wait on with nobody there — a merge, a
//! ticket, a meeting's end — comes through THE RESOLVER's background
//! reads, not through these.
//!
//! What a chore reacts to rather than waits for kicks it ([`Chores::kick`]):
//! a session's status moving, an answer landing — or, from a background
//! task with no channel of its own, [`wake`]: knowledge's review worked
//! out, or something it wants said. The chore then runs within a second
//! instead of on its next beat, never more than once a second however
//! many events arrive.
//!
//! | Chore | Looked at | Unfocused or away | Runs at all |
//! |---|---|---|---|
//! | `Automations` (PR bodies, every runner) | 5 s; 60 s with every automation off | away: 30 s; 10 min | always: cards close themselves even with the automations off |
//! | `Knowledge` | 30 s, and woken when its background has news | 60 s | unless offline |
//! | `FollowUps` | 10 min (a daily sweep inside) | 10 min | always |
//! | `Accounts` | 30 s | — | only while somebody is looking |
//! | `Flush` (`pr_cache`, `linear_cache`, usage) | 10 s | 60 s | only what changed is written |
//! | `Owner` (`sync.lock`) | 30 s | 30 s | only while another orion holds it |

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use orion_core::ClientRequest;

use crate::app::App;

/// One piece of the loop's housekeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Chore {
    /// `pr_body::tick` and `automations::tick`: every runner's turn.
    Automations,
    /// `knowledge::tick`.
    Knowledge,
    /// `factory::follow_ups::tick`.
    FollowUps,
    /// Who each CLAUDE ACCOUNT is signed in as.
    Accounts,
    /// What the answers changed, written to the caches off the loop.
    Flush,
    /// `sync.lock`, claimed from another orion that quit.
    Owner,
}

impl Chore {
    pub const ALL: [Chore; 6] = [
        Chore::Automations,
        Chore::Knowledge,
        Chore::FollowUps,
        Chore::Accounts,
        Chore::Flush,
        Chore::Owner,
    ];
}

/// The least time between two kicked runs of a chore: a burst of events
/// is one run.
pub const KICK_GAP: Duration = Duration::from_secs(1);

const fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// The chores a background task woke ([`wake`]), one bit each, for the
/// loop to kick ([`take_woken`]).
static WOKEN: AtomicU8 = AtomicU8::new(0);

/// What wakes the loop when a background task has news for a chore.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// A task off the loop has something for `chore` — knowledge's review
/// worked out, a word to flash: the loop wakes and kicks it, rather than
/// the chore polling for it on a short beat.
pub fn wake(chore: Chore) {
    WOKEN.fetch_or(1 << chore as u8, Ordering::SeqCst);
    WAKE.notify_one();
}

/// Resolves when a background task has woken a chore ([`wake`]).
pub async fn woken() {
    WAKE.notified().await;
}

/// The chores woken since the last take.
pub fn take_woken() -> Vec<Chore> {
    let bits = WOKEN.swap(0, Ordering::SeqCst);
    Chore::ALL
        .into_iter()
        .filter(|c| bits & (1 << *c as u8) != 0)
        .collect()
}

/// How often `chore` runs, as of now: None while it has nothing to do at
/// all — accounts while nobody is looking, the lock while this orion
/// holds it.
pub fn period(app: &App, chore: Chore) -> Option<Duration> {
    let away = app.away;
    let idle = app.idle();
    let on = crate::automations::Automation::ALL
        .into_iter()
        .any(|a| app.automations.is_on(a));
    match chore {
        Chore::Automations => Some(match (on, away) {
            (true, false) => secs(5),
            (true, true) => secs(30),
            (false, false) => secs(60),
            (false, true) => secs(600),
        }),
        // Its own work is on a 30 s beat (`knowledge::tick`); what its
        // background finds wakes it ([`wake`]).
        Chore::Knowledge => Some(if idle { secs(60) } else { secs(30) }),
        Chore::FollowUps => Some(secs(600)),
        Chore::Accounts => (!idle).then_some(secs(30)),
        Chore::Flush => Some(if idle { secs(60) } else { secs(10) }),
        Chore::Owner => app.sync_owner.is_none().then_some(secs(30)),
    }
}

/// When each chore next runs, and when it last did.
#[derive(Debug, Default)]
pub struct Chores {
    due: HashMap<Chore, Instant>,
    ran: HashMap<Chore, Instant>,
}

impl Chores {
    /// The earliest chore's deadline: the loop sleeps until then.
    pub fn next_wake(&self) -> Option<Instant> {
        self.due.values().min().copied()
    }

    /// Every chore due at `now`, taken off the schedule until it has run
    /// ([`ran`](Self::ran)). In a fixed order, so a tie runs the same way.
    pub fn take_due(&mut self, now: Instant) -> Vec<Chore> {
        let due: Vec<Chore> = Chore::ALL
            .into_iter()
            .filter(|c| self.due.get(c).is_some_and(|at| *at <= now))
            .collect();
        for c in &due {
            self.due.remove(c);
        }
        due
    }

    /// `chore` ran at `now`: next due a period on, if it has one.
    pub fn ran(&mut self, chore: Chore, every: Option<Duration>, now: Instant) {
        self.ran.insert(chore, now);
        match every {
            Some(every) => {
                self.due.insert(chore, now + every);
            }
            None => {
                self.due.remove(&chore);
            }
        }
    }

    /// Something `chore` reacts to just happened: run it now, or a
    /// [`KICK_GAP`] after its last run if that is later — never later
    /// than it was already due.
    pub fn kick(&mut self, chore: Chore, now: Instant) {
        let soonest = self
            .ran
            .get(&chore)
            .map_or(now, |at| (*at + KICK_GAP).max(now));
        let at = self
            .due
            .get(&chore)
            .map_or(soonest, |due| (*due).min(soonest));
        self.due.insert(chore, at);
    }

    /// Bring the schedule in line with what each chore's period is now: one
    /// with a period and no deadline (never run, or off until now) is due
    /// a period after its last run — at once if it never ran; one whose
    /// period has gone is dropped.
    pub fn rearm(&mut self, periods: impl Fn(Chore) -> Option<Duration>, now: Instant) {
        for chore in Chore::ALL {
            match periods(chore) {
                Some(every) => {
                    let ran = self.ran.get(&chore).copied();
                    self.due
                        .entry(chore)
                        .or_insert_with(|| ran.map_or(now, |at| (at + every).max(now)));
                }
                None => {
                    self.due.remove(&chore);
                }
            }
        }
    }

    /// Somebody is looking again: what IDLE held off runs at once.
    pub fn catch_up(&mut self, now: Instant) {
        for chore in [
            Chore::Accounts,
            Chore::Automations,
            Chore::Knowledge,
            Chore::Flush,
        ] {
            self.kick(chore, now);
        }
    }
}

/// Run whatever chore is due, and schedule each again.
pub fn tick(app: &mut App, out: &mut Vec<ClientRequest>) {
    let now = Instant::now();
    for chore in app.chores.take_due(now) {
        run(app, chore, out);
        let every = period(app, chore);
        app.chores.ran(chore, every, now);
    }
}

/// Every chore's schedule brought in line with the app as it is now
/// ([`Chores::rearm`]). Run after every turn of the loop.
pub fn rearm(app: &mut App) {
    let periods: HashMap<Chore, Option<Duration>> = Chore::ALL
        .into_iter()
        .map(|c| (c, period(app, c)))
        .collect();
    app.chores.rearm(|c| periods[&c], Instant::now());
}

/// Do `chore`.
fn run(app: &mut App, chore: Chore, out: &mut Vec<ClientRequest>) {
    match chore {
        Chore::Automations => {
            // PR BODY: each factory pull request whose facts were just
            // handed over, or whose card is up after it left its list,
            // asked to be read (THE RESOLVER reads it: its background
            // demand, `pr_body::detail_needs`).
            crate::pr_body::tick(app);
            // AUTOMATIONS: each runner's turn — the PR fixer's queued
            // asks among them.
            crate::automations::tick(app, out);
        }
        // KNOWLEDGE: the session snapshot, harvests left queued, and the
        // daily card of new rules.
        Chore::Knowledge => crate::knowledge::tick(app),
        // FACTORY: once a day, the bugs that followed each run's merge.
        Chore::FollowUps => crate::factory::follow_ups::tick(app),
        // Who each CLAUDE ACCOUNT is signed in as: a `/login` typed in a
        // session's pane renames its account here.
        Chore::Accounts => crate::claude_accounts::request_refresh(app, false),
        // Whatever the answers changed since the last flush goes to disk,
        // off the loop; the next launch paints from it.
        Chore::Flush => {
            if let Some((path, readings)) = crate::usage::take_flush(app) {
                tokio::task::spawn_blocking(move || crate::usage::write_cache(&path, &readings));
            }
            if let Some((cache, store, live)) = crate::pr_cache::take_flush(app) {
                tokio::task::spawn_blocking(move || {
                    crate::pr_cache::write_all(&cache, &store, &live)
                });
            }
            if let Some((cache, store)) = crate::linear_cache::take_flush(app) {
                tokio::task::spawn_blocking(move || crate::linear_cache::write(&cache, &store));
            }
            // After the stamps went out with the flush: THE RESOLVER lets
            // go of what nothing needs any more.
            app.resolver.prune(Instant::now());
        }
        // Another orion that held `sync.lock` may have quit: the
        // automations' background reads move here if so.
        Chore::Owner => {
            app.sync_owner = crate::sync::background::Owner::claim(
                &crate::sync::background::Owner::default_location(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every(n: u64) -> impl Fn(Chore) -> Option<Duration> {
        move |_| Some(secs(n))
    }

    #[test]
    fn a_chore_never_run_is_due_at_once_then_a_period_on() {
        let t0 = Instant::now();
        let mut c = Chores::default();
        c.rearm(every(5), t0);
        assert_eq!(c.next_wake(), Some(t0));
        let due = c.take_due(t0);
        assert_eq!(due.len(), Chore::ALL.len());
        assert_eq!(c.next_wake(), None, "taken until run");
        for chore in due {
            c.ran(chore, Some(secs(5)), t0);
        }
        assert_eq!(c.next_wake(), Some(t0 + secs(5)));
        assert!(c.take_due(t0 + secs(4)).is_empty());
        assert_eq!(c.take_due(t0 + secs(5)).len(), Chore::ALL.len());
    }

    /// A burst of kicks is one run, a gap after the last; a kick never
    /// puts a chore off.
    #[test]
    fn kicks_run_a_chore_soon_but_never_twice_a_second() {
        let t0 = Instant::now();
        let mut c = Chores::default();
        c.ran(Chore::Automations, Some(secs(60)), t0);
        c.kick(Chore::Automations, t0 + Duration::from_millis(100));
        assert_eq!(c.next_wake(), Some(t0 + KICK_GAP));
        c.kick(Chore::Automations, t0 + Duration::from_millis(200));
        assert_eq!(c.next_wake(), Some(t0 + KICK_GAP), "one run for the burst");
        c.kick(Chore::Automations, t0 + secs(10));
        assert_eq!(c.next_wake(), Some(t0 + KICK_GAP), "never later");
        let due = c.take_due(t0 + KICK_GAP);
        assert_eq!(due, vec![Chore::Automations]);
        c.ran(Chore::Automations, Some(secs(60)), t0 + KICK_GAP);
        c.kick(Chore::Automations, t0 + secs(30));
        assert_eq!(c.next_wake(), Some(t0 + secs(30)), "long after: at once");
    }

    /// A chore whose period goes (nobody looking at the accounts) is
    /// dropped; when it comes back it is due a period after it last ran.
    #[test]
    fn a_chore_off_for_a_while_comes_back_on_its_period() {
        let t0 = Instant::now();
        let mut c = Chores::default();
        let only = |on: bool| move |chore| (on && chore == Chore::Accounts).then_some(secs(30));
        c.rearm(only(true), t0);
        let due = c.take_due(t0);
        assert_eq!(due, vec![Chore::Accounts]);
        c.ran(Chore::Accounts, Some(secs(30)), t0);
        c.rearm(only(false), t0 + secs(1));
        assert_eq!(c.next_wake(), None, "idle: not at all");
        c.rearm(only(true), t0 + secs(10));
        assert_eq!(c.next_wake(), Some(t0 + secs(30)));
        c.rearm(only(true), t0 + secs(90));
        assert_eq!(c.next_wake(), Some(t0 + secs(30)), "already armed");
    }

    /// Away, the automations' housekeeping stretches six times over, and
    /// with every automation off it all but stops; the accounts aren't
    /// read with nobody looking.
    #[test]
    fn away_stretches_the_automations_housekeeping() {
        let mut app = App::new();
        app.window_focused = true;
        for a in crate::automations::Automation::ALL {
            app.automations.settings.of_mut(a).on = Some(false);
        }
        assert_eq!(period(&app, Chore::Automations), Some(secs(60)));
        app.automations
            .settings
            .of_mut(crate::automations::Automation::PrFixer)
            .on = Some(true);
        assert_eq!(period(&app, Chore::Automations), Some(secs(5)));
        assert_eq!(period(&app, Chore::Accounts), Some(secs(30)));
        assert_eq!(period(&app, Chore::Knowledge), Some(secs(30)));
        app.window_focused = false;
        assert_eq!(period(&app, Chore::Knowledge), Some(secs(60)), "unfocused");
        assert_eq!(period(&app, Chore::Flush), Some(secs(60)));
        app.window_focused = true;
        app.away = true;
        assert_eq!(period(&app, Chore::Automations), Some(secs(30)));
        assert_eq!(period(&app, Chore::Knowledge), Some(secs(60)));
        assert_eq!(period(&app, Chore::Accounts), None);
    }

    /// A background task's wake reaches the loop once, for the chores it
    /// named.
    #[test]
    fn a_wake_names_its_chores_once() {
        // `Owner`: no background task wakes it, so no other test races.
        wake(Chore::Owner);
        wake(Chore::Owner);
        assert!(take_woken().contains(&Chore::Owner));
        assert!(!take_woken().contains(&Chore::Owner));
    }
}
