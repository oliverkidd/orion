//! AWAY DETECTION: whether you are at the desk, for the phone bridge
//! (`phone`: sessions that start waiting while you are away, and those
//! already waiting as you go) and the factory's cards (`factory`, held at
//! the desk and sent to Slack as you go). The rule is `orion_core::away`'s;
//! this reads its signals off the Mac and keeps the hand flag (⌘⇧A,
//! `ClientRequest::SetAway`).
//!
//! Both signals are read in-process from the I/O Registry (`macos`),
//! where they used to come from two spawned `ioreg` runs every 5 s:
//!
//! - **Idle**: `IOHIDSystem`'s `HIDIdleTime`, nanoseconds since the last
//!   key or mouse movement.
//! - **Screen lock**: the registry root's `IOConsoleLocked`, and
//!   `CGSSessionScreenIsLocked` in its `IOConsoleUsers` when that is
//!   there. Checked on macOS 26.6.2: an unlocked screen lists
//!   `IOConsoleLocked = No` and no `CGSSessionScreenIsLocked` at all
//!   (older releases add that key only while locked), so `IOConsoleLocked`
//!   is the one relied on and either saying yes is locked.
//!
//! The reading paces itself ([`next_wait`]): at the desk it sleeps until
//! the moment the idle threshold could first be crossed — never more than
//! 5 s, which is also how late a screen lock can be noticed — and while
//! away it looks every 10 s, so coming back is seen quickly. The
//! settings come from the mtime-keyed [`Config::cached`], not a fresh
//! parse each look.
//!
//! The reading runs only while Slack is connected — away means nothing
//! without somewhere to send to: with it signed out, the loop sleeps until
//! it signs in. `ORION_AWAY_POLL_MS` pins a fixed period instead (`0`
//! turns it off: the e2e tests, only the hand flag then).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orion_core::away::{AwaySettings, Signals};
use orion_core::ServerEvent;

use crate::config::Config;
use crate::registry::Daemon;

/// The longest the reading sleeps while you are at the desk: how late a
/// screen lock may be noticed — and a session already waiting on you goes
/// to your phone as you lock up and leave. Each reading is a few in-process registry reads, so
/// looking this often costs next to nothing.
const PRESENT_MAX: Duration = Duration::from_secs(5);
/// The pace while you are away: coming back is noticed within this.
const AWAY_EVERY: Duration = Duration::from_secs(10);
/// The shortest sleep, so a threshold a moment off is not spun on.
const MIN_WAIT: Duration = Duration::from_secs(1);
/// Past the predicted crossing, so the look after it lands on the far side.
const CROSSING_MARGIN: Duration = Duration::from_millis(500);

/// How the signals are paced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// Every this long, whatever happens (`ORION_AWAY_POLL_MS`).
    Fixed(Duration),
    /// As [`next_wait`] says.
    Adaptive,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct State {
    signals: Signals,
    by_hand: bool,
    away: bool,
    /// How many times you have left the desk, and the last of those the
    /// phone re-sent what still waits for ([`AwayWatch::take_departure`]).
    left: u64,
    resent_for: u64,
}

/// Whether you are away, on the daemon.
#[derive(Default)]
pub struct AwayWatch {
    state: Mutex<State>,
}

impl AwayWatch {
    /// You count as away now.
    pub fn is_away(&self) -> bool {
        self.state.lock().unwrap().away
    }

    /// Whether you are away on a departure the phone has not yet re-sent
    /// its open messages for; true once per departure.
    pub fn take_departure(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        let fresh = state.away && state.left != state.resent_for;
        state.resent_for = state.left;
        fresh
    }

    /// The state as `ServerEvent::AwayChanged`.
    pub fn current(&self) -> ServerEvent {
        let state = *self.state.lock().unwrap();
        ServerEvent::AwayChanged {
            away: state.away,
            by_hand: state.by_hand,
        }
    }

    /// Apply `change` and the rule, as `settings` say; the event to send
    /// when what a TUI shows changed.
    fn update(
        &self,
        settings: &AwaySettings,
        change: impl FnOnce(&mut State),
    ) -> Option<ServerEvent> {
        let mut state = self.state.lock().unwrap();
        let before = *state;
        change(&mut state);
        state.away = settings.is_away(state.signals, state.by_hand);
        if state.away && !before.away {
            state.left += 1;
        }
        let changed = (before.away, before.by_hand) != (state.away, state.by_hand);
        if changed && before.away != state.away {
            tracing::info!(away = state.away, by_hand = state.by_hand, "away changed");
        }
        changed.then(|| ServerEvent::AwayChanged {
            away: state.away,
            by_hand: state.by_hand,
        })
    }
}

impl Daemon {
    /// AWAY BY HAND on or off (`ClientRequest::SetAway`); away now, the
    /// factory cards held and the sessions already waiting go to Slack.
    pub fn set_away_by_hand(self: &Arc<Self>, on: bool) {
        let settings = &Config::cached().away;
        if let Some(event) = self.away.update(settings, |s| s.by_hand = on) {
            self.broadcast(event);
        }
        let daemon = self.clone();
        tokio::spawn(async move {
            daemon.factory_flush().await;
            daemon.phone_flush().await;
        });
    }

    fn note_away_signals(&self, settings: &AwaySettings, signals: Signals) {
        if let Some(event) = self.away.update(settings, |s| s.signals = signals) {
            self.broadcast(event);
        }
    }
}

/// Read the signals for the daemon's life at `pace` (None: never — the
/// hand flag alone counts).
pub async fn watch(daemon: Arc<Daemon>, pace: Option<Pace>) {
    let Some(pace) = pace else {
        return;
    };
    loop {
        let connected = daemon.slack.connected();
        let signals = match connected {
            true => read_signals(),
            false => Signals::default(),
        };
        let config = Config::cached();
        daemon.note_away_signals(&config.away, signals);
        daemon.factory_flush().await;
        daemon.phone_flush().await;
        if !connected {
            // Nothing to read for: the next look is when Slack signs in.
            tokio::select! {
                _ = daemon.shutdown.cancelled() => return,
                _ = daemon.slack.until_connected() => {}
            }
            continue;
        }
        let wait = match pace {
            Pace::Fixed(period) => period,
            Pace::Adaptive => next_wait(&config.away, signals, daemon.away.is_away()),
        };
        tokio::select! {
            _ = daemon.shutdown.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// How long to sleep before the next look, given what this one read.
/// Away: [`AWAY_EVERY`]. At the desk: until the idle threshold could first
/// be crossed (the idle clock only runs forward until you touch something,
/// and touching something only moves the crossing later), capped at
/// [`PRESENT_MAX`] — the lock can come any time and is only seen by
/// looking. No threshold, or no idle reading: the cap.
pub fn next_wait(settings: &AwaySettings, signals: Signals, away: bool) -> Duration {
    if away {
        return AWAY_EVERY;
    }
    let threshold = Duration::from_secs(u64::from(settings.idle_minutes) * 60);
    match signals.idle {
        Some(idle) if settings.idle_minutes > 0 => threshold
            .saturating_sub(idle)
            .saturating_add(CROSSING_MARGIN)
            .clamp(MIN_WAIT, PRESENT_MAX),
        _ => PRESENT_MAX,
    }
}

/// The minute of the local day now (0–1439), for the quiet hours.
pub fn local_minute() -> u32 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() * 60 + now.minute()
}

/// The pace: adaptive, or the fixed period `ORION_AWAY_POLL_MS` names
/// (`0` is off).
pub fn pace() -> Option<Pace> {
    match orion_core::env::non_empty(orion_core::env::AWAY_POLL_MS).map(|v| v.parse::<u64>()) {
        Some(Ok(0)) => None,
        Some(Ok(ms)) => Some(Pace::Fixed(Duration::from_millis(ms.max(50)))),
        _ => Some(Pace::Adaptive),
    }
}

/// The Mac's signals now, read in-process. Each is None where the
/// registry said nothing readable — and both are, off macOS.
pub fn read_signals() -> Signals {
    #[cfg(target_os = "macos")]
    {
        Signals {
            locked: crate::macos::screen_locked(),
            idle: crate::macos::hid_idle(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Signals::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_desk(idle_secs: u64) -> Signals {
        Signals {
            locked: Some(false),
            idle: Some(Duration::from_secs(idle_secs)),
        }
    }

    /// At the desk the reading sleeps until the idle threshold could be
    /// crossed, never longer than 5 s (a lock is seen within that); away
    /// it looks every 10 s.
    #[test]
    fn the_wait_runs_to_the_threshold_capped_at_five_seconds() {
        let ten = AwaySettings::default();
        assert_eq!(ten.idle_minutes, 10);
        assert_eq!(PRESENT_MAX, Duration::from_secs(5));
        assert_eq!(next_wait(&ten, at_desk(0), false), PRESENT_MAX, "far off");
        assert_eq!(
            next_wait(&ten, at_desk(600 - 3), false),
            Duration::from_secs(3) + CROSSING_MARGIN,
            "three seconds to go"
        );
        assert_eq!(
            next_wait(&ten, at_desk(600), false),
            MIN_WAIT,
            "on the line: look again soon"
        );
        assert_eq!(next_wait(&ten, at_desk(30), true), AWAY_EVERY, "away");
        assert_eq!(
            next_wait(&ten, Signals::default(), false),
            PRESENT_MAX,
            "nothing read"
        );
        let never = AwaySettings {
            idle_minutes: 0,
            ..AwaySettings::default()
        };
        assert_eq!(next_wait(&never, at_desk(5_000), false), PRESENT_MAX);
    }

    /// The real registry on this machine reads both signals, in-process.
    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_reads_its_signals() {
        let signals = read_signals();
        assert!(signals.locked.is_some(), "IOConsoleLocked");
        assert!(signals.idle.is_some(), "HIDIdleTime");
    }

    #[test]
    fn the_hand_flag_alone_makes_you_away() {
        let watch = AwayWatch::default();
        let settings = AwaySettings::default();
        assert!(watch.update(&settings, |s| s.by_hand = true).is_some());
        assert!(watch.is_away());
        assert!(
            watch.update(&settings, |s| s.by_hand = true).is_none(),
            "no change"
        );
        watch.update(&settings, |s| s.by_hand = false);
        assert!(!watch.is_away());
    }

    /// Each time you leave the desk counts once, however often it is
    /// asked about, and never while you are at it.
    #[test]
    fn each_departure_is_taken_once() {
        let watch = AwayWatch::default();
        let settings = AwaySettings::default();
        assert!(!watch.take_departure(), "at the desk");
        watch.update(&settings, |s| s.by_hand = true);
        assert!(watch.take_departure());
        assert!(!watch.take_departure(), "the same departure");
        watch.update(&settings, |s| s.by_hand = false);
        assert!(!watch.take_departure(), "back");
        watch.update(&settings, |s| s.by_hand = true);
        assert!(watch.take_departure(), "gone again");
    }
}
