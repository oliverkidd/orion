//! POWER: whether the Mac is running on its battery, for what the DAEMON
//! holds back then — an idle agent's CLI is reaped sooner (`registry`'s
//! IDLE REAPER) and a SPARE WORKTREE's CLI is not started until you are in
//! its project (`spare`). Read in-process from IOKit's power sources
//! (`macos::on_battery`) and remembered for [`CACHE_FOR`]: the reaper and
//! the spare tick ask every half minute, and a power cable changes far
//! less often than that. Off macOS, or when the sources can't be read, the
//! Mac counts as on AC — nothing is held back on a guess.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long one reading stands.
const CACHE_FOR: Duration = Duration::from_secs(60);

/// Whether the Mac runs on its battery now (as of the last minute).
pub fn on_battery() -> bool {
    static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if let Some((at, battery)) = *last {
        if at.elapsed() < CACHE_FOR {
            return battery;
        }
    }
    let battery = read();
    *last = Some((Instant::now(), battery));
    battery
}

/// The daemon's own tests run on whatever powers the machine that runs
/// them, and are written for AC: under test this answers AC, and the
/// readings themselves are tested directly (`macos::on_battery`).
#[cfg(target_os = "macos")]
fn read() -> bool {
    !cfg!(test) && crate::macos::on_battery().unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn read() -> bool {
    false
}

#[cfg(test)]
mod tests {
    /// The real power sources answer, in-process (a desktop Mac has no
    /// battery and answers AC).
    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_says_what_powers_it() {
        assert!(crate::macos::on_battery().is_some(), "the sources read");
        assert!(!super::on_battery(), "tests run as on AC");
    }
}
