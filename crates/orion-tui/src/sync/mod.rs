//! THE RESOLVER: the one place a query orion makes — to GitHub, Linear,
//! Microsoft 365, the usage endpoints, and a checkout's own git — is
//! decided, deduplicated and paced.
//!
//! Every query has a [`QueryKey`], scoped to what a screen shows — one
//! project's open pull requests, one checkout's branch lookup. Nothing
//! asks for a key on a timer of its own. A key is fetched because:
//!
//! * **you look at it.** After every turn of the event loop the screen
//!   says what it shows ([`scope::visible`]); a key that comes on screen,
//!   or every key on screen when the window takes focus back, is fetched
//!   when its last answer is older than its policy's `stale_after` — and
//!   not otherwise, so walking between projects reads the cache;
//! * **you are in it.** A key the user is in ([`Seen::Focused`]: the open
//!   PULL REQUESTS MODAL, a page whose checks run, the selected checkout)
//!   refetches every `live_every` while the window is focused and nobody
//!   is away;
//! * **an automation needs it** ([`background::needs`]), at the period it
//!   asks for, and only in the one orion that holds the machine's
//!   `sync.lock`;
//! * **something changed it** ([`Resolver::invalidate`]: a merge, a new
//!   pull request, a push, a file THE GIT WATCHER saw change) — refetched
//!   now when on screen, else marked stale for its next visit;
//! * **you asked** ([`Resolver::refresh`], `⌘R` / `Shift+R`) — the one
//!   trigger that cuts a failure's backoff short.
//!
//! One fetch per key runs at a time, whoever asks ([`crate::fetch::Flights`]);
//! a refresh during one is owed exactly one more. A provider has only a
//! few running at once ([`Provider::max_in_flight`]); the rest wait for
//! one to land. A failure backs off on
//! its policy's doubling backoff, which visits and focus never shorten. A
//! rate limit pauses the whole [`Provider`] until it resets ([`gate`]),
//! `⌘R` included.
//!
//! The logic here is pure: every method takes the time it runs at, so the
//! tests drive the clock. The event loop sleeps until [`Resolver::next_wake`],
//! the earliest moment anything here can come due, and spawns what
//! [`Resolver::due`] hands it ([`spawn`]).

pub mod background;
pub mod gate;
pub mod github;
pub mod key;
pub mod linear;
pub mod local;
pub mod policy;
pub mod scope;
pub mod services;
pub mod spawn;

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use crate::fetch::{Flights, Ticket};
pub use gate::FetchError;
pub use key::{Provider, QueryKey};
use policy::policy;

/// How a key is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Seen {
    /// Drawn: the sidebar's group, a checkout's badge.
    Shown,
    /// The thing the user is in: the open PULL REQUESTS MODAL, the page a
    /// cursor rests on while its checks run, the selected checkout. Wants
    /// a fresher answer on arrival (the policy's `focused_stale_after`),
    /// and keeps a live beat while the window is focused (its
    /// `live_every`).
    Focused,
}

/// What the resolver knows of one key.
#[derive(Debug, Clone, Default)]
struct Entry {
    /// When the last fetch started.
    asked: Option<Instant>,
    /// When the last success landed.
    ok: Option<Instant>,
    /// Failures in a row since the last success.
    fails: u32,
    /// Until when a failure holds the key off. Nothing but an explicit
    /// refresh shortens it.
    retry_at: Option<Instant>,
    /// Something changed it since its last success ([`Resolver::invalidate`]).
    stale: bool,
    /// A visit or a refresh wants a fetch that has not succeeded yet: it
    /// goes as soon as nothing holds it off, for as long as the key is on
    /// screen.
    wanted: bool,
    /// Why the last fetch failed, until one succeeds.
    error: Option<String>,
}

/// What a screen can say about a key ([`Resolver::status`]): `updated 4m
/// ago`, `refreshing…`, `couldn't refresh`, `GitHub limit, back at 22:11`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// A fetch is running.
    pub fetching: bool,
    /// When the last success landed, this run.
    pub updated_at: Option<Instant>,
    /// Why the last fetch failed, when it did and none has succeeded since.
    pub error: Option<String>,
    /// The key's provider is rate limited until then.
    pub paused_until: Option<Instant>,
    /// A failure holds the key off until then.
    pub retry_at: Option<Instant>,
}

/// The resolver's state: every key it has been asked about, what is on
/// screen, what the automations need, and which providers are paused.
#[derive(Debug, Default)]
pub struct Resolver {
    entries: HashMap<QueryKey, Entry>,
    flights: Flights<QueryKey>,
    /// What the screen showed at the last [`visit`](Self::visit).
    visible: HashMap<QueryKey, Seen>,
    /// What the automations need, and how often.
    background: HashMap<QueryKey, Duration>,
    /// Keys asked for once by something that is not a screen
    /// ([`ask`](Self::ask)): fetched once if stale, then let go.
    once: BTreeSet<QueryKey>,
    /// Providers paused by a rate limit, until when.
    paused: HashMap<Provider, Instant>,
    /// Somebody is looking: the window is focused and nobody is away.
    /// Live beats run only then.
    live: bool,
    /// Keys a watcher invalidates on every change ([`set_watched`](Self::set_watched)):
    /// their live beat is the policy's slower `watched_every`.
    watched: BTreeSet<QueryKey>,
}

impl Resolver {
    /// Whether somebody is looking (`!App::idle`): live beats run only
    /// while they are.
    pub fn set_live(&mut self, live: bool) {
        self.live = live;
    }

    /// Something now watches `keys` and invalidates each on every change
    /// (THE GIT WATCHER on the selected checkout): their live beat is only
    /// a backstop. Replaces the last set.
    pub fn set_watched(&mut self, keys: impl IntoIterator<Item = QueryKey>) {
        self.watched = keys.into_iter().collect();
    }

    /// The screen now shows `seen`. A key that was not on screen before —
    /// or was, but is now the one the user is in — is wanted when its last
    /// answer is older than its policy allows; a key that has left the
    /// screen stops being wanted.
    pub fn visit(&mut self, seen: Vec<(QueryKey, Seen)>, now: Instant) {
        let mut next: HashMap<QueryKey, Seen> = HashMap::with_capacity(seen.len());
        for (key, how) in seen {
            let how = next.get(&key).map_or(how, |was| (*was).max(how));
            next.insert(key, how);
        }
        for (key, how) in &next {
            if self.visible.get(key).is_none_or(|was| was < how) {
                self.consider(key, *how, now);
            }
        }
        for key in self.visible.keys() {
            if !next.contains_key(key) {
                if let Some(entry) = self.entries.get_mut(key) {
                    entry.wanted = false;
                }
            }
        }
        self.visible = next;
    }

    /// Look again at `keys`, already on screen, as if they had just come
    /// on: focus moving into the panel that shows them. Keys not on
    /// screen are ignored.
    pub fn touch(&mut self, keys: &[QueryKey], now: Instant) {
        for key in keys {
            if let Some(how) = self.visible.get(key).copied() {
                self.consider(key, how, now);
            }
        }
    }

    /// The window took focus back: every key on screen is looked at again.
    pub fn focus_gained(&mut self, now: Instant) {
        let keys: Vec<(QueryKey, Seen)> =
            self.visible.iter().map(|(k, s)| (k.clone(), *s)).collect();
        for (key, how) in keys {
            self.consider(&key, how, now);
        }
    }

    /// Want `key` when its answer is older than `how` allows, or stale.
    fn consider(&mut self, key: &QueryKey, how: Seen, now: Instant) {
        let p = policy(key);
        let stale_after = match how {
            Seen::Shown => p.stale_after,
            Seen::Focused => p.focused_stale_after,
        };
        let entry = self.entries.entry(key.clone()).or_default();
        let fresh = !entry.stale
            && entry
                .ok
                .is_some_and(|ok| now.saturating_duration_since(ok) < stale_after);
        if !fresh {
            entry.wanted = true;
        }
    }

    /// One look at `key` from something that is not a screen — a picker
    /// about to show what each account has left, the factory about to
    /// choose one: fetched once, if its answer is older than its policy's
    /// `stale_after` (and nothing holds it off), whether or not anything
    /// shows it. Never a beat: once it lands it is let go.
    pub fn ask(&mut self, key: QueryKey, now: Instant) {
        self.consider(&key, Seen::Shown, now);
        if self.entries.get(&key).is_some_and(|e| e.wanted || e.stale) {
            self.once.insert(key);
        }
    }

    /// `⌘R` / `Shift+R` on `key`: fetch it now, past its freshness and its
    /// failure backoff — but never past a provider's pause. One already
    /// running owes exactly one more. A key nobody is looking at is only
    /// marked stale, for its next visit.
    pub fn refresh(&mut self, key: &QueryKey) {
        if self.flights.want_fresh(key) {
            return;
        }
        let demanded = self.demanded(key);
        let entry = self.entries.entry(key.clone()).or_default();
        entry.retry_at = None;
        if demanded {
            entry.wanted = true;
        } else {
            entry.stale = true;
        }
    }

    /// Something changed `key` (a merge, a new pull request, a push): its
    /// answer is behind. On screen or needed by an automation it refetches
    /// now — a fetch already running owes one more, since it may have been
    /// asked before the change — and otherwise it waits, stale, for its
    /// next visit. A failure's backoff still stands.
    pub fn invalidate(&mut self, key: &QueryKey) {
        self.flights.want_fresh(key);
        self.entries.entry(key.clone()).or_default().stale = true;
    }

    /// What an earlier run knew of `key`: an answer that landed at `at`
    /// (the cache's stamp, `pr_cache`). It counts as this run's last
    /// answer and last ask while there is none — so a visit within its
    /// freshness, or a background beat within its period, asks nothing
    /// at launch — and is ignored once there is.
    pub fn seed(&mut self, key: &QueryKey, at: Instant) {
        let entry = self.entries.entry(key.clone()).or_default();
        if entry.ok.is_none() && entry.asked.is_none() {
            entry.ok = Some(at);
            entry.asked = Some(at);
        }
    }

    /// Every key with an answer this run (or a seed from the last), and
    /// when it landed: what the caches stamp their copies with.
    pub fn answered(&self) -> impl Iterator<Item = (&QueryKey, Instant)> {
        self.entries
            .iter()
            .filter_map(|(key, e)| Some((key, e.ok?)))
    }

    /// Whether this run has an answer for `key`, or an ask, or a seed.
    pub fn known(&self, key: &QueryKey) -> bool {
        self.entries
            .get(key)
            .is_some_and(|e| e.ok.is_some() || e.asked.is_some())
    }

    /// Drop the fetch running for `key`: what it asked is no longer the
    /// question (the checkout switched branch). Its answer is dropped
    /// when it lands.
    pub fn cancel(&mut self, key: &QueryKey) {
        self.flights.cancel(key);
    }

    /// Forget `key` altogether: the thing it asked about is gone.
    pub fn forget(&mut self, key: &QueryKey) {
        self.flights.cancel(key);
        self.entries.remove(key);
        self.once.remove(key);
    }

    /// Forget what nothing needs any more: a key nothing asks for, not
    /// running, not marked behind, whose answer is past every freshness
    /// its policy has and whose backoff is out — a visit would fetch it
    /// anyway, so nothing is lost. What would otherwise pile up for good:
    /// each page a cursor ever rested on, each meeting's transcript, each
    /// account and Linear key since removed.
    pub fn prune(&mut self, now: Instant) {
        let gone: Vec<QueryKey> = self
            .entries
            .iter()
            .filter(|(key, e)| {
                let p = policy(key);
                let fresh_for = p.stale_after.max(p.focused_stale_after);
                let spent =
                    e.ok.is_none_or(|ok| now.saturating_duration_since(ok) >= fresh_for);
                !self.demanded(key)
                    && !self.flights.in_flight(key)
                    && !e.stale
                    && spent
                    && e.retry_at.is_none_or(|at| at <= now)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            self.forget(&key);
        }
    }

    /// Forget every key `keep` says no to.
    pub fn retain(&mut self, mut keep: impl FnMut(&QueryKey) -> bool) {
        let gone: Vec<QueryKey> = self
            .entries
            .keys()
            .filter(|key| !keep(key))
            .cloned()
            .collect();
        for key in gone {
            self.forget(&key);
        }
    }

    /// What the automations need now (from [`background::needs`]):
    /// each key and how often. Replaces the last set.
    pub fn set_background(&mut self, needs: Vec<(QueryKey, Duration)>) {
        self.background.clear();
        for (key, every) in needs {
            let every = self.background.get(&key).map_or(every, |e| (*e).min(every));
            self.background.insert(key, every);
        }
    }

    /// A provider was found rate limited until `until` (by a call outside
    /// the resolver, through the [`gate`]). A pause only ever extends.
    pub fn observe_pause(&mut self, provider: Provider, until: Option<Instant>) {
        if let Some(until) = until {
            let at = self.paused.entry(provider).or_insert(until);
            *at = (*at).max(until);
        }
    }

    /// Until when `provider` is paused at `now`, if it is.
    pub fn paused_until(&self, provider: Provider, now: Instant) -> Option<Instant> {
        self.paused
            .get(&provider)
            .copied()
            .filter(|until| *until > now)
    }

    /// Every key something asks for, best first: what the user is in,
    /// what is drawn, a one-off look, then what the automations need — so
    /// when a provider's cap holds some back, it is the background's.
    fn demanded_keys(&self) -> Vec<QueryKey> {
        let mut ranked: Vec<(u8, &QueryKey)> = self
            .visible
            .iter()
            .map(|(key, how)| (u8::from(*how == Seen::Shown), key))
            .chain(self.once.iter().map(|key| (2, key)))
            .chain(self.background.keys().map(|key| (3, key)))
            .collect();
        ranked.sort();
        let mut seen = BTreeSet::new();
        ranked
            .into_iter()
            .filter(|(_, key)| seen.insert(*key))
            .map(|(_, key)| key.clone())
            .collect()
    }

    /// Whether anything asks for `key`: it is on screen, an automation
    /// needs it, or something looked once.
    fn demanded(&self, key: &QueryKey) -> bool {
        self.visible.contains_key(key)
            || self.background.contains_key(key)
            || self.once.contains(key)
    }

    /// Whether `provider` has as many keys fetching as it may at once.
    fn full(&self, provider: Provider) -> bool {
        let running = self
            .flights
            .keys()
            .filter(|key| key.provider() == provider)
            .count();
        running >= provider.max_in_flight()
    }

    /// How often `key` refetches with nobody asking anew: the shorter of
    /// its live beat (as the thing the user is in, while somebody is
    /// looking: `live_every`, or `watched_every` under a watcher) and
    /// what the automations need. A key only drawn has no beat.
    fn period(&self, key: &QueryKey) -> Option<Duration> {
        let live = match self.visible.get(key).filter(|_| self.live) {
            Some(Seen::Focused) if self.watched.contains(key) => policy(key).watched_every,
            Some(Seen::Focused) => policy(key).live_every,
            Some(Seen::Shown) | None => None,
        };
        let background = self.background.get(key).copied();
        match (live, background) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// When `key` may next be fetched for any reason, or `None` while
    /// nothing will ask for it. Not counting a fetch already running.
    fn due_at(&self, key: &QueryKey, now: Instant) -> Option<Instant> {
        if !self.demanded(key) || self.flights.in_flight(key) {
            return None;
        }
        let entry = self.entries.get(key);
        let wanted = entry.is_some_and(|e| e.wanted || e.stale);
        let asked = entry.and_then(|e| e.asked);
        let at = if wanted {
            now
        } else {
            let every = self.period(key)?;
            asked.map_or(now, |asked| asked + every)
        };
        let held = [
            entry.and_then(|e| e.retry_at),
            self.paused_until(key.provider(), now),
        ]
        .into_iter()
        .flatten()
        .max();
        Some(held.map_or(at, |held| at.max(held)))
    }

    /// Every key that is due at `now`, each started: one ticket per fetch
    /// to spawn, to be handed back to [`land`](Self::land) with its answer.
    /// No more than its provider's [`max_in_flight`](Provider::max_in_flight)
    /// run at once; the rest stay due for a step after one lands. Best
    /// first ([`demanded_keys`](Self::demanded_keys)), in key order within
    /// a rank, so a tie always goes the same way.
    pub fn due(&mut self, now: Instant) -> Vec<Ticket<QueryKey>> {
        let mut tickets = Vec::new();
        for key in self.demanded_keys() {
            if self.due_at(&key, now).is_none_or(|at| at > now) {
                continue;
            }
            if self.full(key.provider()) {
                continue;
            }
            let Some(ticket) = self.flights.begin(key.clone(), now) else {
                continue;
            };
            self.entries.entry(key).or_default().asked = Some(now);
            tickets.push(ticket);
        }
        tickets
    }

    /// The earliest moment anything here can come due — a live or
    /// background beat, the end of a backoff or of a pause holding back a
    /// wanted key — or `None` when nothing will. The event loop sleeps
    /// until then. A key due now that its provider's cap holds back is
    /// no deadline: a landing frees its place, and wakes the loop itself.
    pub fn next_wake(&self, now: Instant) -> Option<Instant> {
        self.demanded_keys()
            .iter()
            .filter_map(|key| {
                let at = self.due_at(key, now)?;
                (at > now || !self.full(key.provider())).then_some(at)
            })
            .min()
    }

    /// A fetch landed at `now`: `answer` is `Ok` when it brought an answer
    /// (the caller then observes it), else why not. `None` for a ticket
    /// whose flight was cancelled — drop the answer. A refresh owed by the
    /// flight is wanted now.
    pub fn land(
        &mut self,
        ticket: &Ticket<QueryKey>,
        answer: Result<(), &FetchError>,
        now: Instant,
    ) -> Option<crate::fetch::Landed> {
        let landed = self.flights.land(ticket)?;
        let key = &ticket.key;
        // A one-off ask is one fetch, whatever it brought.
        self.once.remove(key);
        let p = policy(key);
        let mut paused = None;
        let entry = self.entries.entry(key.clone()).or_default();
        match answer {
            Ok(()) => {
                entry.ok = Some(now);
                entry.fails = 0;
                entry.retry_at = None;
                entry.stale = false;
                entry.wanted = false;
                entry.error = None;
            }
            Err(FetchError::RateLimited { until }) => {
                // Not the key's failure: it goes again once the pause lifts.
                entry.error = Some(format!("{} rate limit", key.provider().name()));
                entry.wanted = true;
                paused = Some(*until);
            }
            Err(e) => {
                entry.fails += 1;
                entry.retry_at = Some(now + p.backoff_after(entry.fails));
                entry.error = Some(e.summary());
                entry.wanted = true;
            }
        }
        // Asked for afresh while this one ran: it may predate what it was
        // asked after.
        if landed.owed {
            entry.stale = true;
        }
        if let Some(until) = paused {
            self.observe_pause(key.provider(), Some(until));
        }
        Some(landed)
    }

    /// What a screen can say about `key` at `now`.
    pub fn status(&self, key: &QueryKey, now: Instant) -> Status {
        let entry = self.entries.get(key);
        Status {
            fetching: self.flights.in_flight(key),
            updated_at: entry.and_then(|e| e.ok),
            error: entry.and_then(|e| e.error.clone()),
            paused_until: self.paused_until(key.provider(), now),
            retry_at: entry.and_then(|e| e.retry_at).filter(|at| *at > now),
        }
    }

    /// Whether a fetch of `key` is running.
    pub fn in_flight(&self, key: &QueryKey) -> bool {
        self.flights.in_flight(key)
    }

    /// Whether an answer for `key` is on its way at `now`: a fetch is
    /// running, or one asked for ([`ask`](Self::ask)) starts on the next
    /// step — not one a backoff or a pause holds off, which is no answer
    /// worth waiting for.
    pub fn coming(&self, key: &QueryKey, now: Instant) -> bool {
        self.flights.in_flight(key)
            || (self.once.contains(key) && self.due_at(key, now).is_some_and(|at| at <= now))
    }

    /// Whether `key` is on screen as of the last visit: a status line may
    /// say how its last fetch went.
    pub fn is_visible(&self, key: &QueryKey) -> bool {
        self.visible.contains_key(key)
    }

    /// Every key on screen as of the last visit — what `Shift+R`
    /// refreshes.
    pub fn visible_keys(&self) -> Vec<QueryKey> {
        let mut keys: Vec<QueryKey> = self.visible.keys().cloned().collect();
        keys.sort();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{ProjectId, WorktreeId};

    fn list(p: &str) -> QueryKey {
        QueryKey::OpenPrs(ProjectId(p.into()))
    }

    fn branch(w: &str) -> QueryKey {
        QueryKey::BranchPr(WorktreeId(w.into()))
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// A resolver with somebody looking.
    fn live() -> Resolver {
        let mut r = Resolver::default();
        r.set_live(true);
        r
    }

    fn keys(tickets: &[Ticket<QueryKey>]) -> Vec<QueryKey> {
        tickets.iter().map(|t| t.key.clone()).collect()
    }

    /// Fetch `key` at `at` and land it as a success at the same instant.
    fn answered(r: &mut Resolver, key: &QueryKey, at: Instant) {
        r.visit(vec![(key.clone(), Seen::Shown)], at);
        let tickets = r.due(at);
        let ticket = tickets.iter().find(|t| &t.key == key).expect("fetched");
        r.land(ticket, Ok(()), at).expect("landed");
    }

    #[test]
    fn a_key_coming_on_screen_is_fetched_once() {
        let t0 = Instant::now();
        let mut r = live();
        r.visit(vec![(list("p"), Seen::Shown)], t0);
        assert_eq!(keys(&r.due(t0)), vec![list("p")]);
        assert!(r.in_flight(&list("p")));
        assert!(r.due(t0).is_empty(), "nothing more to start");
    }

    /// The sidebar and the modal asking for the same list share one fetch.
    #[test]
    fn two_visits_to_one_key_in_flight_start_one_fetch() {
        let t0 = Instant::now();
        let mut r = live();
        r.visit(vec![(list("p"), Seen::Shown)], t0);
        let first = r.due(t0);
        assert_eq!(first.len(), 1);
        r.visit(
            vec![(list("p"), Seen::Shown), (list("p"), Seen::Focused)],
            t0 + secs(1),
        );
        r.focus_gained(t0 + secs(2));
        assert!(
            r.due(t0 + secs(2)).is_empty(),
            "the running one serves both"
        );
    }

    #[test]
    fn a_refresh_during_a_flight_starts_exactly_one_more() {
        let t0 = Instant::now();
        let mut r = live();
        r.visit(vec![(list("p"), Seen::Shown)], t0);
        let ticket = r.due(t0).remove(0);
        r.refresh(&list("p"));
        r.refresh(&list("p"));
        assert!(r.due(t0).is_empty(), "never two at once");
        let landed = r.land(&ticket, Ok(()), t0 + secs(1)).unwrap();
        assert!(landed.owed);
        let again = r.due(t0 + secs(1));
        assert_eq!(keys(&again), vec![list("p")], "one more");
        r.land(&again[0], Ok(()), t0 + secs(2)).unwrap();
        assert!(r.due(t0 + secs(2)).is_empty(), "and no chain");
        assert_eq!(r.next_wake(t0 + secs(2)), None);
    }

    #[test]
    fn a_visit_inside_stale_after_does_nothing() {
        let t0 = Instant::now();
        let mut r = live();
        answered(&mut r, &list("p"), t0);
        // Off screen and back within the minute: the cache stands.
        r.visit(vec![], t0 + secs(10));
        r.visit(vec![(list("p"), Seen::Shown)], t0 + secs(20));
        r.focus_gained(t0 + secs(30));
        assert!(r.due(t0 + secs(30)).is_empty());
        // Past it, the next visit fetches.
        r.visit(vec![], t0 + secs(61));
        r.visit(vec![(list("p"), Seen::Shown)], t0 + secs(62));
        assert_eq!(keys(&r.due(t0 + secs(62))), vec![list("p")]);
    }

    /// Opening the modal on a list the sidebar read 40 s ago fetches it:
    /// the thing you are in wants a fresher answer than a glance does.
    #[test]
    fn becoming_the_focused_key_is_a_visit_of_its_own() {
        let t0 = Instant::now();
        let mut r = live();
        answered(&mut r, &list("p"), t0);
        r.visit(vec![(list("p"), Seen::Focused)], t0 + secs(20));
        assert!(r.due(t0 + secs(20)).is_empty(), "20 s old: fresh enough");
        r.visit(vec![(list("p"), Seen::Shown)], t0 + secs(30));
        r.visit(vec![(list("p"), Seen::Focused)], t0 + secs(40));
        assert_eq!(keys(&r.due(t0 + secs(40))), vec![list("p")]);
    }

    #[test]
    fn a_failures_backoff_survives_visits_and_focus_but_not_a_refresh() {
        let t0 = Instant::now();
        let mut r = live();
        r.visit(vec![(list("p"), Seen::Shown)], t0);
        let ticket = r.due(t0).remove(0);
        let err = FetchError::Failed("HTTP 502".into());
        r.land(&ticket, Err(&err), t0).unwrap();
        let backoff = policy(&list("p")).backoff_after(1);
        assert_eq!(r.status(&list("p"), t0).error.as_deref(), Some("HTTP 502"));
        // Leave, come back, refocus: still held off.
        r.visit(vec![], t0 + secs(1));
        r.visit(vec![(list("p"), Seen::Focused)], t0 + secs(2));
        r.focus_gained(t0 + secs(3));
        r.touch(&[list("p")], t0 + secs(4));
        assert!(r.due(t0 + secs(4)).is_empty(), "the backoff stands");
        assert_eq!(r.next_wake(t0 + secs(4)), Some(t0 + backoff));
        // It retries on its own once the backoff is out, while on screen.
        let retried = r.due(t0 + backoff);
        assert_eq!(keys(&retried), vec![list("p")]);
        r.land(&retried[0], Err(&err), t0 + backoff).unwrap();
        let second = policy(&list("p")).backoff_after(2);
        assert_eq!(
            r.status(&list("p"), t0 + backoff).retry_at,
            Some(t0 + backoff + second),
            "and doubles"
        );
        // ⌘R does not wait.
        r.refresh(&list("p"));
        assert_eq!(keys(&r.due(t0 + backoff + secs(1))), vec![list("p")]);
    }

    #[test]
    fn a_rate_limit_pauses_every_key_of_its_provider_refresh_included() {
        let t0 = Instant::now();
        let reset = t0 + secs(600);
        let mut r = live();
        r.visit(
            vec![(list("p"), Seen::Shown), (branch("w"), Seen::Shown)],
            t0,
        );
        let mut tickets = r.due(t0);
        assert_eq!(tickets.len(), 2);
        let limited = FetchError::RateLimited { until: reset };
        r.land(&tickets.remove(0), Err(&limited), t0).unwrap();
        r.land(&tickets.remove(0), Ok(()), t0).unwrap();
        assert_eq!(r.status(&branch("w"), t0).paused_until, Some(reset));
        // A new GitHub key, ⌘R on both: nothing goes.
        r.visit(
            vec![
                (list("p"), Seen::Shown),
                (branch("w"), Seen::Shown),
                (list("q"), Seen::Shown),
            ],
            t0 + secs(1),
        );
        r.refresh(&list("p"));
        r.refresh(&branch("w"));
        assert!(r.due(t0 + secs(300)).is_empty(), "paused, ⌘R included");
        assert_eq!(r.next_wake(t0 + secs(300)), Some(reset));
        // The pause lifts at the reset, and what was asked for goes.
        let after = r.due(reset);
        assert_eq!(keys(&after), vec![list("p"), list("q"), branch("w")]);
    }

    /// A pause another `gh` call found pauses the resolver's keys too.
    #[test]
    fn a_pause_seen_by_the_gate_holds_the_resolver_back() {
        let t0 = Instant::now();
        let mut r = live();
        r.observe_pause(Provider::GitHub, Some(t0 + secs(60)));
        r.visit(vec![(list("p"), Seen::Shown)], t0);
        assert!(r.due(t0).is_empty());
        assert_eq!(keys(&r.due(t0 + secs(60))), vec![list("p")]);
    }

    #[test]
    fn a_key_with_no_demand_is_never_due() {
        let t0 = Instant::now();
        let mut r = live();
        answered(&mut r, &list("p"), t0);
        r.visit(vec![], t0 + secs(1));
        r.invalidate(&list("p"));
        r.refresh(&list("p"));
        for later in [secs(2), secs(600), secs(86_400)] {
            assert!(r.due(t0 + later).is_empty());
        }
        assert_eq!(r.next_wake(t0), None);
    }

    /// The sidebar's list has no beat: on screen for an hour, it is asked
    /// once. The modal's has a minute's, and only while somebody looks.
    #[test]
    fn only_the_focused_key_has_a_live_beat_and_only_while_looked_at() {
        let t0 = Instant::now();
        let mut r = live();
        answered(&mut r, &list("p"), t0);
        assert_eq!(r.next_wake(t0), None, "shown: no beat");
        assert!(r.due(t0 + secs(3600)).is_empty());
        r.visit(vec![(list("p"), Seen::Focused)], t0 + secs(3600));
        let ticket = r.due(t0 + secs(3600)).remove(0);
        r.land(&ticket, Ok(()), t0 + secs(3600)).unwrap();
        let beat = t0 + secs(3660);
        assert_eq!(r.next_wake(t0 + secs(3600)), Some(beat));
        r.set_live(false);
        assert_eq!(r.next_wake(t0 + secs(3600)), None, "nobody looking");
        assert!(r.due(beat).is_empty());
        r.set_live(true);
        assert_eq!(keys(&r.due(beat)), vec![list("p")]);
    }

    /// The selected checkout's git status is read on every selection and
    /// beats every 15 s unwatched, every 5 min under the watcher; a band
    /// is read as it comes on screen and never beats — and nothing beats
    /// with nobody looking. A change the watcher (or an agent's turn
    /// ending) says of is read at once, looked at or not.
    #[test]
    fn a_checkouts_read_beats_only_while_looked_at() {
        let t0 = Instant::now();
        let git = |w: &str| QueryKey::GitStatus(WorktreeId(w.into()));
        let mut r = live();
        let seen = vec![(git("sel"), Seen::Focused), (git("band"), Seen::Shown)];
        r.visit(seen.clone(), t0);
        let tickets = r.due(t0);
        assert_eq!(keys(&tickets), vec![git("sel"), git("band")], "yours first");
        for t in &tickets {
            r.land(t, Ok(()), t0).unwrap();
        }
        assert_eq!(r.next_wake(t0), Some(t0 + secs(15)), "unwatched: 15 s");
        let beat = r.due(t0 + secs(15));
        assert_eq!(keys(&beat), vec![git("sel")], "the band has no beat");
        r.land(&beat[0], Ok(()), t0 + secs(15)).unwrap();
        r.set_watched([git("sel")]);
        assert_eq!(r.next_wake(t0 + secs(15)), Some(t0 + secs(315)), "watched");
        // Back on the selection a second later: read again, fresh or not.
        r.visit(vec![(git("band"), Seen::Shown)], t0 + secs(61));
        r.visit(seen.clone(), t0 + secs(62));
        assert!(keys(&r.due(t0 + secs(62))).contains(&git("sel")));
        // Nobody looking: no beat at all, but a change still reads.
        r.set_live(false);
        let later = t0 + secs(3600);
        assert!(r.due(later).is_empty(), "no beat while idle");
        r.invalidate(&git("band"));
        assert_eq!(keys(&r.due(later)), vec![git("band")]);
    }

    /// Walking past a dozen projects asks GitHub four at a time: the rest
    /// wait, with no deadline of their own, for a landing to free a place.
    #[test]
    fn a_burst_goes_a_few_at_a_time_per_provider() {
        let t0 = Instant::now();
        let mut r = live();
        let lists: Vec<QueryKey> = (0..10).map(|n| list(&format!("p{n:02}"))).collect();
        let mut seen: Vec<(QueryKey, Seen)> =
            lists.iter().map(|k| (k.clone(), Seen::Shown)).collect();
        let git = QueryKey::GitStatus(WorktreeId("w".into()));
        seen.push((git.clone(), Seen::Focused));
        r.visit(seen, t0);
        let first = r.due(t0);
        let github = Provider::GitHub.max_in_flight();
        assert_eq!(first.len(), github + 1, "four lists and the local read");
        assert!(keys(&first).contains(&git), "another provider's cap");
        assert!(r.due(t0).is_empty(), "capped");
        assert_eq!(r.next_wake(t0), None, "no spin while capped");
        let list = first.iter().find(|t| t.key == lists[0]).expect("the first");
        r.land(list, Ok(()), t0).unwrap();
        assert_eq!(keys(&r.due(t0)), vec![lists[github].clone()], "one freed");
    }

    /// The thing you are in goes before what is only drawn, and both
    /// before what an automation wants, when a cap holds some back.
    #[test]
    fn a_cap_holds_back_the_background_first() {
        let t0 = Instant::now();
        let mut r = live();
        let pages: Vec<(QueryKey, Duration)> = (0..6)
            .map(|n| (QueryKey::PrDetail(format!("u{n}")), secs(60)))
            .collect();
        r.set_background(pages);
        r.visit(vec![(list("z"), Seen::Focused)], t0);
        let first = keys(&r.due(t0));
        assert_eq!(first[0], list("z"));
        assert_eq!(first.len(), Provider::GitHub.max_in_flight());
    }

    #[test]
    fn next_wake_is_the_earliest_deadline() {
        let t0 = Instant::now();
        let mut r = live();
        // A background need every 5 minutes, asked at t0.
        r.set_background(vec![(QueryKey::MyPrs, secs(300))]);
        // A focused list asked at t0: its beat is a minute.
        r.visit(vec![(list("p"), Seen::Focused)], t0);
        let tickets = r.due(t0);
        assert_eq!(tickets.len(), 2);
        for t in &tickets {
            r.land(t, Ok(()), t0).unwrap();
        }
        assert_eq!(r.next_wake(t0), Some(t0 + secs(60)));
        // A branch that failed: retried at its backoff, sooner still.
        r.visit(
            vec![(list("p"), Seen::Focused), (branch("w"), Seen::Shown)],
            t0,
        );
        let ticket = r.due(t0).remove(0);
        r.land(&ticket, Err(&FetchError::Missing), t0).unwrap();
        let retry = t0 + policy(&branch("w")).backoff_after(1);
        assert_eq!(r.next_wake(t0), Some(retry.min(t0 + secs(60))));
        // A pause past all of them holds every GitHub key to its end.
        r.observe_pause(Provider::GitHub, Some(t0 + secs(900)));
        assert_eq!(r.next_wake(t0), Some(t0 + secs(900)));
    }

    #[test]
    fn invalidate_on_a_hidden_key_marks_it_stale_without_fetching() {
        let t0 = Instant::now();
        let mut r = live();
        answered(&mut r, &list("p"), t0);
        r.visit(vec![], t0 + secs(1));
        r.invalidate(&list("p"));
        assert!(r.due(t0 + secs(2)).is_empty());
        assert_eq!(r.next_wake(t0 + secs(2)), None);
        // Seconds later, back on screen: fetched, fresh or not.
        r.visit(vec![(list("p"), Seen::Shown)], t0 + secs(3));
        assert_eq!(keys(&r.due(t0 + secs(3))), vec![list("p")]);
    }

    #[test]
    fn invalidate_on_a_visible_key_fetches_now_and_owes_a_running_one() {
        let t0 = Instant::now();
        let mut r = live();
        answered(&mut r, &list("p"), t0);
        r.invalidate(&list("p"));
        let ticket = r.due(t0 + secs(1)).remove(0);
        r.invalidate(&list("p"));
        let landed = r.land(&ticket, Ok(()), t0 + secs(2)).unwrap();
        assert!(landed.owed, "asked before the change");
        assert_eq!(keys(&r.due(t0 + secs(2))), vec![list("p")]);
    }

    #[test]
    fn a_cancelled_flight_drops_its_answer() {
        let t0 = Instant::now();
        let mut r = live();
        r.visit(vec![(branch("w"), Seen::Shown)], t0);
        let ticket = r.due(t0).remove(0);
        r.cancel(&branch("w"));
        assert!(r.land(&ticket, Ok(()), t0).is_none());
        assert_eq!(r.status(&branch("w"), t0).updated_at, None);
        // Still wanted: a fresh one goes for the question now asked.
        assert_eq!(keys(&r.due(t0)), vec![branch("w")]);
    }

    #[test]
    fn a_background_need_beats_with_nobody_looking() {
        let t0 = Instant::now();
        let mut r = Resolver::default();
        r.set_background(vec![(QueryKey::MyPrs, secs(180))]);
        let ticket = r.due(t0).remove(0);
        r.land(&ticket, Ok(()), t0).unwrap();
        assert_eq!(r.next_wake(t0), Some(t0 + secs(180)));
        assert_eq!(keys(&r.due(t0 + secs(180))), vec![QueryKey::MyPrs]);
        r.set_background(vec![]);
        assert_eq!(r.next_wake(t0 + secs(180)), None, "the automation went off");
    }

    #[test]
    fn status_says_when_it_last_worked_and_why_it_failed() {
        let t0 = Instant::now();
        let mut r = live();
        answered(&mut r, &list("p"), t0);
        r.refresh(&list("p"));
        let ticket = r.due(t0 + secs(240)).remove(0);
        assert!(r.status(&list("p"), t0 + secs(240)).fetching);
        r.land(&ticket, Err(&FetchError::Missing), t0 + secs(240))
            .unwrap();
        let status = r.status(&list("p"), t0 + secs(240));
        assert_eq!(status.updated_at, Some(t0));
        assert_eq!(status.error.as_deref(), Some("couldn't ask"));
        assert!(!status.fetching);
    }

    /// A stamp the cache kept stands as the last answer: a visit within
    /// its freshness and a background beat within its period ask nothing
    /// at launch. A live answer replaces it.
    #[test]
    fn a_seeded_answer_counts_until_a_live_one_lands() {
        let t0 = Instant::now();
        let mut r = live();
        let tails = QueryKey::PrTails(ProjectId("p".into()));
        r.seed(&tails, t0);
        r.visit(vec![(tails.clone(), Seen::Shown)], t0 + secs(60));
        assert!(r.due(t0 + secs(60)).is_empty(), "ten minutes' worth");
        r.seed(&QueryKey::MyPrs, t0);
        r.set_background(vec![(QueryKey::MyPrs, secs(900))]);
        assert!(r.due(t0 + secs(60)).is_empty());
        assert_eq!(r.next_wake(t0 + secs(60)), Some(t0 + secs(900)));
        assert_eq!(keys(&r.due(t0 + secs(900))), vec![QueryKey::MyPrs]);
    }

    /// A one-off ask fetches a key nothing shows, once, when it is stale
    /// — never on a beat, and not at all when it is fresh. An answer on
    /// its way is `coming`; one a backoff holds off is not.
    #[test]
    fn a_one_off_ask_fetches_once_when_stale() {
        let t0 = Instant::now();
        let mut r = live();
        let key = QueryKey::Usage("claude".into());
        r.ask(key.clone(), t0);
        assert!(r.coming(&key, t0), "starts on the next step");
        let ticket = r.due(t0).pop().expect("asked once");
        assert!(r.coming(&key, t0), "running");
        r.land(&ticket, Ok(()), t0);
        assert!(!r.coming(&key, t0));
        assert!(r.due(t0 + secs(3600)).is_empty(), "no beat after");
        r.ask(key.clone(), t0 + secs(60));
        assert!(r.due(t0 + secs(60)).is_empty(), "fresh: nothing");
        let later = t0 + secs(11 * 60);
        r.ask(key.clone(), later);
        let ticket = r.due(later).pop().expect("stale: again");
        r.land(&ticket, Err(&FetchError::Failed("x".into())), later);
        r.ask(key.clone(), later);
        assert!(
            !r.coming(&key, later),
            "a backoff holds it: nothing to wait for"
        );
    }

    /// A key nobody asks for is dropped once its answer is past use; one
    /// still fresh, behind, or on screen is kept.
    #[test]
    fn prune_drops_only_what_a_visit_would_fetch_anyway() {
        let t0 = Instant::now();
        let mut r = live();
        let page = |n: &str| QueryKey::PrDetail(n.into());
        for n in ["old", "behind", "shown"] {
            answered(&mut r, &page(n), t0);
        }
        answered(&mut r, &page("recent"), t0 + secs(100));
        r.visit(vec![(page("shown"), Seen::Shown)], t0 + secs(100));
        r.invalidate(&page("behind"));
        r.prune(t0 + secs(120));
        let kept = |r: &Resolver, n: &str| r.known(&page(n));
        assert!(!kept(&r, "old"), "a minute old and off screen");
        assert!(kept(&r, "behind"), "marked behind: its next visit reads it");
        assert!(kept(&r, "shown"), "on screen");
        assert!(kept(&r, "recent"), "still fresh");
    }

    #[test]
    fn forgetting_a_key_drops_what_it_knew() {
        let t0 = Instant::now();
        let mut r = live();
        answered(&mut r, &branch("w1"), t0);
        answered(&mut r, &branch("w2"), t0);
        r.retain(|k| k != &branch("w1"));
        assert_eq!(r.status(&branch("w1"), t0).updated_at, None);
        assert_eq!(r.status(&branch("w2"), t0).updated_at, Some(t0));
    }
}
