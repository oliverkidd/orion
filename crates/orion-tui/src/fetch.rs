//! How an answer from a fetch becomes a fact on screen. orion asks GitHub,
//! Linear and git the same questions from several places and on several
//! beats, and the answers land in whatever order the network gives them.
//! Every store that keeps those facts follows the same three rules, and
//! this module is where they live:
//!
//! 1. **The newest *asked* answer wins.** Each fact carries when the
//!    question it answers was asked ([`Asked`]) — when the fetch started,
//!    not when it landed. An answer only replaces a fact asked no later
//!    than itself, so a slow fetch started before a fast one can't put
//!    back what the fast one already corrected. A fact read off disk
//!    ([`Asked::Cached`]) loses to any live answer.
//! 2. **Unknown never overwrites known.** A field the query didn't ask
//!    for, a value GitHub hasn't computed yet (`mergeable: UNKNOWN`), a
//!    failed fetch and a truncated page are all "no answer", observed as
//!    `None`, and the last known value stays ([`Known::observe`]).
//! 3. **Asking for fresh data is never dropped.** One fetch runs per key
//!    at a time ([`Flights`]). Asking for a fresh one while it runs marks
//!    it as owing another, which starts as soon as the running one lands,
//!    so an edit's "read it again" can't be swallowed by a fetch that was
//!    asked before the edit.
//!
//! A cancelled fetch's answer is dropped when it lands: the question it
//! asked — the branch a checkout was on, say — is no longer the one being
//! asked ([`Flights::cancel`]).
//!
//! Nothing here knows about `App` or any one kind of entity. Each store
//! keys its own facts (PR URL, issue id, worktree id) and holds a
//! [`Known`] per field and a [`Flights`] per kind of fetch.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The instant to stamp a question asked now with — what a fetch's ticket
/// carries and what an optimistic write observes with. Never the same
/// instant twice in one process: the clock can hand two questions asked
/// back to back the same reading, and a tie goes to the answer that lands
/// last ([`Known`]), which would let the slower of the two win. So the
/// only answers that ever tie are ones that share an ask on purpose — the
/// failing-checks recheck stamped with its list's time.
pub fn now() -> Instant {
    stamp(Instant::now())
}

/// `at`, or the next instant after the last stamp handed out when `at` is
/// not later than it. [`Flights::begin`] passes its `now` through here.
fn stamp(at: Instant) -> Instant {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let at = match *last {
        Some(prev) if at <= prev => prev + Duration::from_nanos(1),
        _ => at,
    };
    *last = Some(at);
    at
}

/// When the question an answer answers was asked. `Cached` came off disk
/// and loses to any live answer; `At` is the instant the fetch started
/// (not when it landed), taken from [`now`] or a [`Ticket`] so no two
/// questions share one. Ordered that way: `Cached` sorts before every
/// `At`, and `At`s by their instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Asked {
    Cached,
    At(Instant),
}

/// One fact and when the answer it holds was asked. Starts unknown.
///
/// [`observe`](Self::observe) is the only way in, and it keeps the
/// newest-asked known value: `None` never replaces a value, and an answer
/// asked before the stored one is dropped. A tie goes to the answer
/// observed last, so a follow-up that refines an answer under the same
/// asked time — the failing-checks recheck after the list it came from,
/// or a second row of the same list answer — takes over, and a second
/// hydration from disk replaces the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known<T> {
    value: Option<T>,
    asked: Option<Asked>,
}

// By hand: a derive would ask `T: Default` of a fact that starts unknown.
impl<T> Default for Known<T> {
    fn default() -> Self {
        Self {
            value: None,
            asked: None,
        }
    }
}

impl<T: PartialEq> Known<T> {
    /// Take an answer asked at `asked`. `None` (not asked, not computed,
    /// failed) leaves the fact as it is, its asked time included — so an
    /// older answer still in flight that does know the value can still
    /// land. An answer asked before the one stored is dropped. Returns
    /// whether the value changed; a newer answer saying the same thing
    /// moves the asked time on but returns false, so a caller can use it
    /// as its "repaint / write the cache" signal.
    pub fn observe(&mut self, value: Option<T>, asked: Asked) -> bool {
        let Some(value) = value else {
            return false;
        };
        if self.asked.is_some_and(|stored| asked < stored) {
            return false;
        }
        self.asked = Some(asked);
        if self.value.as_ref() == Some(&value) {
            return false;
        }
        self.value = Some(value);
        true
    }
}

impl<T> Known<T> {
    /// The newest-asked known value, or `None` while nothing has answered.
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// When the value [`get`](Self::get) returns was asked; `None` while
    /// unknown.
    pub fn asked(&self) -> Option<Asked> {
        self.asked
    }

    /// Whether anything has answered yet.
    pub fn is_known(&self) -> bool {
        self.value.is_some()
    }

    /// Undo an optimistic write: put `before` back, but only while the
    /// fact still holds the write made at `asked`. Anything observed
    /// since — a list asked after the action, a second action — is newer
    /// than the action being undone and stays. Take `before` as a clone
    /// of the fact just ahead of the optimistic `observe`, and `asked` as
    /// the stamp that `observe` used — a fresh [`now`], which no other
    /// answer shares, so a matching stamp means the write is still the one
    /// standing. Returns whether it put it back.
    pub fn undo(&mut self, asked: Asked, before: Known<T>) -> bool {
        if self.asked != Some(asked) {
            return false;
        }
        *self = before;
        true
    }
}

/// Fetches in flight, at most one per key, and whether each owes a fresh
/// one when it lands.
///
/// Moving from a `HashSet<K>` of keys in flight is one call per site:
///
/// | was | now |
/// |---|---|
/// | `if set.contains(&k) { return } set.insert(k)` | `let Some(ticket) = flights.begin(k, now) else { return }` |
/// | the same, for a refresh the user asked for | `let Some(ticket) = flights.begin_fresh(k, now) else { return }` |
/// | `set.remove(&k)` when the answer lands | `let Some(landed) = flights.land(&ticket) else { return }` |
/// | `set.contains(&k)` to draw "refreshing…" | `flights.in_flight(&k)` |
/// | `set.len()`, `set.clear()` | `flights.len()`, `flights.clear()` |
///
/// The [`Ticket`] rides the channel with the answer (it is `Send` and
/// `Clone` whenever the key is) and carries the asked time to observe the
/// answer with. When [`land`](Self::land) says a fresh fetch is owed,
/// start one straight away: the landed flight is gone, so `begin` hands
/// out a new ticket.
#[derive(Debug)]
pub struct Flights<K> {
    running: HashMap<K, Flight>,
    /// Bumped for every flight begun, so a ticket from a cancelled flight
    /// never matches one begun since under the same key.
    next: u64,
}

#[derive(Debug)]
struct Flight {
    generation: u64,
    owed: bool,
}

/// A fetch begun by [`Flights::begin`]: which key it asked about and when.
/// Send it along with the answer and hand it back to [`Flights::land`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket<K> {
    pub key: K,
    /// When the fetch started. Observe its answer with
    /// [`asked`](Self::asked).
    pub at: Instant,
    generation: u64,
}

impl<K> Ticket<K> {
    /// The stamp to observe this fetch's answer with.
    pub fn asked(&self) -> Asked {
        Asked::At(self.at)
    }
}

/// What [`Flights::land`] says about an answer it accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landed {
    /// A fresh fetch was asked for while this one ran: start one now.
    pub owed: bool,
}

// By hand: a derive would ask `K: Default` of the keys.
impl<K> Default for Flights<K> {
    fn default() -> Self {
        Self {
            running: HashMap::new(),
            next: 0,
        }
    }
}

impl<K: Eq + Hash + Clone> Flights<K> {
    /// Start a fetch for `key`, asked at `now`: a ticket to send with its
    /// answer, or `None` while one is already running (a polling beat then
    /// has nothing to do; a refresh someone asked for wants
    /// [`begin_fresh`](Self::begin_fresh)). The ticket's stamp is `now`
    /// made unique ([`now`](crate::fetch::now)), so it can come back a hair
    /// later than the `now` passed in.
    pub fn begin(&mut self, key: K, now: Instant) -> Option<Ticket<K>> {
        if self.running.contains_key(&key) {
            return None;
        }
        let generation = self.next;
        self.next = self.next.wrapping_add(1);
        self.running.insert(
            key.clone(),
            Flight {
                generation,
                owed: false,
            },
        );
        Some(Ticket {
            key,
            at: stamp(now),
            generation,
        })
    }

    /// [`begin`](Self::begin) for a refresh that must not be lost — after
    /// an edit, a ⌘R, a comment posted: a ticket when nothing is running,
    /// else the running fetch is marked as owing a fresh one and `None`
    /// comes back.
    pub fn begin_fresh(&mut self, key: K, now: Instant) -> Option<Ticket<K>> {
        if self.want_fresh(&key) {
            return None;
        }
        self.begin(key, now)
    }

    /// Mark the fetch running for `key` as owing a fresh one, asked after
    /// it — [`land`](Self::land) says so. True when one was running; false
    /// when none is, and the caller should start one itself.
    pub fn want_fresh(&mut self, key: &K) -> bool {
        match self.running.get_mut(key) {
            Some(flight) => {
                flight.owed = true;
                true
            }
            None => false,
        }
    }

    /// An answer landed. `None` for a ticket whose flight was cancelled or
    /// begun again since: drop the answer, it answers a question no longer
    /// asked. Otherwise the flight is over, and [`Landed::owed`] says
    /// whether a fresh fetch was asked for meanwhile.
    pub fn land(&mut self, ticket: &Ticket<K>) -> Option<Landed> {
        let flight = self.running.get(&ticket.key)?;
        if flight.generation != ticket.generation {
            return None;
        }
        let owed = flight.owed;
        self.running.remove(&ticket.key);
        Some(Landed { owed })
    }

    /// Forget the fetch running for `key`, owed refresh and all, so its
    /// answer is dropped when it lands — the branch it asked about has been
    /// switched away, the checkout is gone. A fetch can begin again for the
    /// key straight away.
    pub fn cancel(&mut self, key: &K) {
        self.running.remove(key);
    }

    /// Whether a fetch is running for `key` — the "refreshing…" a screen
    /// draws.
    pub fn in_flight(&self, key: &K) -> bool {
        self.running.contains_key(key)
    }

    /// How many fetches are running, for a cap on how many go at once.
    pub fn len(&self) -> usize {
        self.running.len()
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// Cancel every fetch running.
    pub fn clear(&mut self) {
        self.running.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Three instants in order, a second apart, for stamps that must sort.
    fn instants() -> (Instant, Instant, Instant) {
        let t0 = Instant::now();
        (t0, t0 + Duration::from_secs(1), t0 + Duration::from_secs(2))
    }

    #[test]
    fn unknown_never_replaces_a_known_value_or_its_stamp() {
        let (t0, t1, _) = instants();
        let mut fact = Known::default();
        assert!(!fact.observe(None, Asked::At(t0)), "nothing to replace");
        assert!(!fact.is_known());
        assert_eq!(fact.asked(), None, "an unknown answer stamps nothing");

        assert!(fact.observe(Some("ready"), Asked::At(t0)));
        assert!(!fact.observe(None, Asked::At(t1)));
        assert_eq!(fact.get(), Some(&"ready"));
        assert_eq!(fact.asked(), Some(Asked::At(t0)), "the stamp stays too");
    }

    #[test]
    fn an_answer_asked_before_the_stored_one_is_dropped() {
        let (t0, t1, _) = instants();
        let mut fact = Known::default();
        assert!(fact.observe(Some("conflicts"), Asked::At(t1)));
        assert!(!fact.observe(Some("clean"), Asked::At(t0)));
        assert_eq!(fact.get(), Some(&"conflicts"));
        assert_eq!(fact.asked(), Some(Asked::At(t1)));
    }

    /// A tie goes to the answer observed last: the recheck that refines a
    /// list's checks is stamped with the list's asked time.
    #[test]
    fn an_answer_asked_at_the_same_time_replaces_the_stored_one() {
        let (t0, _, _) = instants();
        let mut fact = Known::default();
        assert!(fact.observe(Some("pending"), Asked::At(t0)));
        assert!(fact.observe(Some("failing"), Asked::At(t0)));
        assert_eq!(fact.get(), Some(&"failing"));

        let mut cached = Known::default();
        assert!(cached.observe(Some(1), Asked::Cached));
        assert!(
            cached.observe(Some(2), Asked::Cached),
            "a second hydration wins"
        );
        assert_eq!(cached.get(), Some(&2));
    }

    #[test]
    fn a_cached_value_loses_to_any_live_answer() {
        let (t0, _, _) = instants();
        assert!(Asked::Cached < Asked::At(t0));

        let mut fact = Known::default();
        assert!(fact.observe(Some("open"), Asked::Cached));
        assert!(fact.observe(Some("merged"), Asked::At(t0)));
        assert!(
            !fact.observe(Some("open"), Asked::Cached),
            "the disk never takes back a live answer"
        );
        assert_eq!(fact.get(), Some(&"merged"));
    }

    /// The return is the caller's "repaint / write the cache" signal, so a
    /// newer answer saying the same thing is no change — but it still
    /// moves the stamp, so an answer asked between the two is dropped.
    #[test]
    fn the_same_value_asked_later_moves_the_stamp_but_reports_no_change() {
        let (t0, t1, t2) = instants();
        let mut fact = Known::default();
        assert!(fact.observe(Some("ready"), Asked::At(t0)));
        assert!(!fact.observe(Some("ready"), Asked::At(t2)));
        assert_eq!(fact.asked(), Some(Asked::At(t2)));
        assert!(!fact.observe(Some("draft"), Asked::At(t1)));
        assert_eq!(fact.get(), Some(&"ready"));
    }

    /// The fetch asked second lands first; the slow one asked before it
    /// must not put back what it corrected.
    #[test]
    fn a_slow_older_fetch_never_overwrites_a_fast_newer_one() {
        let (t0, t1, t2) = instants();
        let mut fact = Known::default();
        fact.observe(Some("clean"), Asked::At(t0));
        assert!(fact.observe(Some("conflicts"), Asked::At(t2)));
        assert!(!fact.observe(Some("clean"), Asked::At(t1)));
        assert_eq!(fact.get(), Some(&"conflicts"));
    }

    /// A newer answer that doesn't know (`mergeable: UNKNOWN`) stamps
    /// nothing, so the older fetch that does know still lands over the
    /// value asked before both.
    #[test]
    fn a_newer_unknown_lets_an_older_known_answer_land() {
        let (t0, t1, t2) = instants();
        let mut fact = Known::default();
        fact.observe(Some("clean"), Asked::At(t0));
        assert!(!fact.observe(None, Asked::At(t2)));
        assert!(fact.observe(Some("conflicts"), Asked::At(t1)));
        assert_eq!(fact.get(), Some(&"conflicts"));
        assert_eq!(fact.asked(), Some(Asked::At(t1)));
    }

    #[test]
    fn undo_puts_back_only_while_the_optimistic_write_still_stands() {
        let (t0, t1, t2) = instants();
        let mut fact = Known::default();
        fact.observe(Some("Todo"), Asked::At(t0));

        let before = fact.clone();
        fact.observe(Some("In Progress"), Asked::At(t1));
        assert!(fact.undo(Asked::At(t1), before.clone()));
        assert_eq!(fact, before, "the whole fact, stamp included, goes back");

        fact.observe(Some("In Progress"), Asked::At(t1));
        fact.observe(Some("Done"), Asked::At(t2));
        assert!(
            !fact.undo(Asked::At(t1), before),
            "something newer replaced the write: leave it"
        );
        assert_eq!(fact.get(), Some(&"Done"));
        assert_eq!(fact.asked(), Some(Asked::At(t2)));
    }

    #[test]
    fn one_fetch_runs_per_key_until_it_lands() {
        let (t0, t1, _) = instants();
        let mut flights = Flights::default();
        let ticket = flights.begin("p1", t0).expect("nothing running");
        assert!(
            ticket.asked() >= Asked::At(t0),
            "stamped no earlier than asked"
        );
        assert!(flights.in_flight(&"p1"));
        assert!(flights.begin("p1", t1).is_none(), "one is running");
        assert!(flights.begin("p2", t1).is_some(), "keys run apart");
        assert_eq!(flights.len(), 2);

        assert_eq!(flights.land(&ticket), Some(Landed { owed: false }));
        assert!(!flights.in_flight(&"p1"));
        assert!(flights.begin("p1", t1).is_some(), "free to ask again");
    }

    #[test]
    fn a_fresh_fetch_asked_mid_flight_is_owed_when_it_lands() {
        let (t0, t1, t2) = instants();
        let mut flights = Flights::default();
        assert!(
            !flights.want_fresh(&"p1"),
            "nothing running: start one yourself"
        );

        let ticket = flights.begin("p1", t0).unwrap();
        assert!(flights.want_fresh(&"p1"));
        assert_eq!(flights.land(&ticket), Some(Landed { owed: true }));
        let again = flights.begin("p1", t1).expect("the landed flight is gone");

        assert!(
            flights.begin_fresh("p1", t2).is_none(),
            "running: marked owed instead"
        );
        assert_eq!(flights.land(&again), Some(Landed { owed: true }));
        assert!(flights.begin_fresh("p1", t2).is_some(), "idle: begins");
    }

    /// A branch switch cancels the lookup for the old branch; its answer,
    /// and any refresh it owed, must not reach the new one.
    #[test]
    fn a_cancelled_flights_answer_is_dropped_even_after_a_new_one_begins() {
        let (t0, t1, _) = instants();
        let mut flights = Flights::default();
        let old = flights.begin("wt", t0).unwrap();
        flights.want_fresh(&"wt");
        flights.cancel(&"wt");
        assert!(!flights.in_flight(&"wt"));
        assert_eq!(flights.land(&old), None, "cancelled: drop it");

        let old = flights.begin("wt", t0).unwrap();
        flights.cancel(&"wt");
        let new = flights
            .begin("wt", t1)
            .expect("free straight after a cancel");
        assert_eq!(flights.land(&old), None, "begun again since: drop it");
        assert!(
            flights.in_flight(&"wt"),
            "a stale ticket leaves the new flight alone"
        );
        assert_eq!(
            flights.land(&new),
            Some(Landed { owed: false }),
            "the owed refresh went with the cancel"
        );
        assert!(flights.is_empty());
    }

    /// Two questions asked within one clock reading still sort apart, so
    /// the slower answer can never win the tie a shared stamp would give.
    #[test]
    fn two_questions_asked_at_once_never_share_a_stamp() {
        let at = Instant::now();
        let mut flights = Flights::default();
        let first = flights.begin("a", at).unwrap();
        let second = flights.begin("b", at).unwrap();
        assert!(first.asked() < second.asked());
        assert!(second.asked() < Asked::At(now()));
    }
}
