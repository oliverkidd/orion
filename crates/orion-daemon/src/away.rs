//! AWAY DETECTION: whether you are at the desk, for the phone bridge
//! (`phone`). The rule is `orion_core::away`'s; this reads its signals off
//! the Mac every 5 s and keeps the hand flag (⌘⇧A, `ClientRequest::SetAway`).
//!
//! - **Idle**: `HIDIdleTime` from `ioreg -c IOHIDSystem -d 4`, nanoseconds
//!   since the last key or mouse movement.
//! - **Screen lock**: `ioreg -n Root -d1`'s `IOConsoleLocked`, and
//!   `CGSSessionScreenIsLocked` in its `IOConsoleUsers` when that is
//!   there. Checked on macOS 26.6.2: an unlocked screen lists
//!   `"IOConsoleLocked" = No` and no `CGSSessionScreenIsLocked` at all
//!   (older releases add that key only while locked), so `IOConsoleLocked`
//!   is the one relied on and either saying yes is locked.
//!
//! The reading runs only while Slack is connected — away means nothing
//! without somewhere to send to — and `ORION_AWAY_POLL_MS=0` turns it off
//! (the e2e tests: only the hand flag then).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orion_core::away::Signals;
use orion_core::ServerEvent;

use crate::registry::Daemon;

/// How often the signals are read.
const POLL: Duration = Duration::from_secs(5);
/// What one `ioreg` may take.
const IOREG_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct State {
    signals: Signals,
    by_hand: bool,
    away: bool,
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

    /// The state as `ServerEvent::AwayChanged`.
    pub fn current(&self) -> ServerEvent {
        let state = *self.state.lock().unwrap();
        ServerEvent::AwayChanged {
            away: state.away,
            by_hand: state.by_hand,
        }
    }

    /// Apply `change` and the rule; the event to send when what a TUI
    /// shows changed.
    fn update(&self, change: impl FnOnce(&mut State)) -> Option<ServerEvent> {
        let settings = crate::config::Config::load().away;
        let mut state = self.state.lock().unwrap();
        let before = *state;
        change(&mut state);
        state.away = settings.is_away(state.signals, state.by_hand);
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
    /// AWAY BY HAND on or off (`ClientRequest::SetAway`).
    pub fn set_away_by_hand(&self, on: bool) {
        if let Some(event) = self.away.update(|s| s.by_hand = on) {
            self.broadcast(event);
        }
    }

    fn note_away_signals(&self, signals: Signals) {
        if let Some(event) = self.away.update(|s| s.signals = signals) {
            self.broadcast(event);
        }
    }
}

/// Read the signals every `period` for the daemon's life (None: never —
/// the hand flag alone counts).
pub async fn watch(daemon: Arc<Daemon>, period: Option<Duration>) {
    let Some(period) = period else {
        return;
    };
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = daemon.shutdown.cancelled() => return,
            _ = interval.tick() => {}
        }
        let signals = match daemon.slack.connected() {
            true => tokio::task::spawn_blocking(read_signals)
                .await
                .unwrap_or_default(),
            false => Signals::default(),
        };
        daemon.note_away_signals(signals);
    }
}

/// The poll period: 5 s, or `ORION_AWAY_POLL_MS` (`0` is off).
pub fn period() -> Option<Duration> {
    match orion_core::env::non_empty(orion_core::env::AWAY_POLL_MS).map(|v| v.parse::<u64>()) {
        Some(Ok(0)) => None,
        Some(Ok(ms)) => Some(Duration::from_millis(ms.max(50))),
        _ => Some(POLL),
    }
}

/// The Mac's signals now. Each is None where `ioreg` is missing or said
/// nothing readable.
pub fn read_signals() -> Signals {
    Signals {
        locked: ioreg(&["-n", "Root", "-d1"]).and_then(|out| parse_locked(&out)),
        idle: ioreg(&["-c", "IOHIDSystem", "-d", "4", "-k", "HIDIdleTime"])
            .and_then(|out| parse_idle(&out)),
    }
}

/// `ioreg`'s output, read as it comes (the Root listing outgrows a pipe's
/// buffer), killed past [`IOREG_TIMEOUT`].
fn ioreg(args: &[&str]) -> Option<String> {
    let child = std::process::Command::new("/usr/sbin/ioreg")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let pid = nix::unistd::Pid::from_raw(child.id() as i32);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(IOREG_TIMEOUT) {
        Ok(Ok(out)) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        Ok(_) => None,
        Err(_) => {
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
            None
        }
    }
}

/// `"HIDIdleTime" = 6409087166` (nanoseconds), the first one.
pub fn parse_idle(text: &str) -> Option<Duration> {
    let value = value_of(text, "HIDIdleTime")?;
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u64>().ok().map(Duration::from_nanos)
}

/// Locked when `IOConsoleLocked` or `CGSSessionScreenIsLocked` says
/// `Yes`; None when neither is there.
pub fn parse_locked(text: &str) -> Option<bool> {
    let flags: Vec<bool> = ["IOConsoleLocked", "CGSSessionScreenIsLocked"]
        .iter()
        .filter_map(|key| value_of(text, key))
        .map(|value| value.starts_with("Yes"))
        .collect();
    (!flags.is_empty()).then(|| flags.contains(&true))
}

/// What follows `"key"` and its `=` in ioreg's text, spaced either way
/// (`"k" = v` at the top level, `"k"=v` inside a dict).
fn value_of<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let quoted = format!("\"{key}\"");
    let at = text.find(&quoted)? + quoted.len();
    let rest = text[at..].trim_start().strip_prefix('=')?;
    Some(rest.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_read_in_nanoseconds() {
        let out = "    | |     \"HIDIdleTime\" = 6409087166\n";
        assert_eq!(parse_idle(out), Some(Duration::from_nanos(6_409_087_166)));
        assert_eq!(parse_idle("nothing"), None);
    }

    #[test]
    fn the_lock_is_read_from_either_key() {
        // This Mac, unlocked (macOS 26.6.2).
        let unlocked = r#"  "IOConsoleLocked" = No
  "IOConsoleUsers" = ({"kCGSSessionOnConsoleKey"=Yes,"kSCSecuritySessionID"=100002})"#;
        assert_eq!(parse_locked(unlocked), Some(false));
        let locked = r#"  "IOConsoleLocked" = Yes"#;
        assert_eq!(parse_locked(locked), Some(true));
        let older = r#"  "IOConsoleUsers" = ({"CGSSessionScreenIsLocked"=Yes,"kCGSSessionOnConsoleKey"=Yes})"#;
        assert_eq!(parse_locked(older), Some(true));
        assert_eq!(parse_locked("{}"), None);
    }

    /// The real `ioreg` on this machine reads both signals. Skipped where
    /// there is none (Linux CI).
    #[test]
    fn this_mac_reads_its_signals() {
        if !std::path::Path::new("/usr/sbin/ioreg").exists() {
            return;
        }
        let signals = read_signals();
        assert!(signals.locked.is_some(), "IOConsoleLocked");
        assert!(signals.idle.is_some(), "HIDIdleTime");
    }

    #[test]
    fn the_hand_flag_alone_makes_you_away() {
        let watch = AwayWatch::default();
        assert!(watch.update(|s| s.by_hand = true).is_some());
        assert!(watch.is_away());
        assert!(watch.update(|s| s.by_hand = true).is_none(), "no change");
        watch.update(|s| s.by_hand = false);
        assert!(!watch.is_away());
    }
}
