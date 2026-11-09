//! AWAY: when you count as away from the desk, and what reaches your phone
//! then. The settings are `away` in config.json — the TUI's Connections tab
//! edits them, the daemon's phone bridge reads them — so they live here,
//! with the rule both read them by.
//!
//! You are away when the screen is locked, after `idle_minutes` with no
//! keys or mouse, or by hand (⌘⇧A). Being in a meeting is never away.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// What may go to your phone while you are away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhoneKind {
    Question,
    Plan,
    Crash,
}

impl PhoneKind {
    pub const ALL: [PhoneKind; 3] = [PhoneKind::Question, PhoneKind::Plan, PhoneKind::Crash];

    /// The word a list of them is spelled with: `questions, plans, crashes`.
    pub fn plural(self) -> &'static str {
        match self {
            PhoneKind::Question => "questions",
            PhoneKind::Plan => "plans",
            PhoneKind::Crash => "crashes",
        }
    }
}

/// `away` in the config file: when you count as away, and what reaches
/// your phone then.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AwaySettings {
    /// The screen locking counts as away.
    pub screen_lock: bool,
    /// Minutes with no keys or mouse before you count as away; 0 is never.
    pub idle_minutes: u32,
    /// What goes to your phone while away.
    pub phone: Vec<PhoneKind>,
    /// `("22:00", "07:00")`: nothing goes to the phone between them.
    /// `null` is no quiet hours.
    pub quiet: Option<(String, String)>,
}

impl Default for AwaySettings {
    fn default() -> Self {
        Self {
            screen_lock: true,
            idle_minutes: 10,
            phone: PhoneKind::ALL.to_vec(),
            quiet: Some(("22:00".into(), "07:00".into())),
        }
    }
}

/// What the machine says about you: each None when it could not be read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Signals {
    /// The screen is locked.
    pub locked: Option<bool>,
    /// How long since the last key or mouse movement.
    pub idle: Option<Duration>,
}

impl AwaySettings {
    /// Away = (the screen lock counts and the screen is locked) or (idle
    /// at least `idle_minutes`) or the hand flag.
    pub fn is_away(&self, signals: Signals, by_hand: bool) -> bool {
        let locked = self.screen_lock && signals.locked == Some(true);
        let idle = self.idle_minutes > 0
            && signals
                .idle
                .is_some_and(|idle| idle >= Duration::from_secs(u64::from(self.idle_minutes) * 60));
        locked || idle || by_hand
    }

    /// Whether `minute` (of the local day, 0–1439) falls in the quiet
    /// hours. A window that runs past midnight (`22:00` to `07:00`) wraps;
    /// one that starts and ends at the same time, or doesn't parse, is no
    /// window.
    pub fn is_quiet(&self, minute: u32) -> bool {
        let Some((from, to)) = &self.quiet else {
            return false;
        };
        let (Some(from), Some(to)) = (parse_hhmm(from), parse_hhmm(to)) else {
            return false;
        };
        match from.cmp(&to) {
            std::cmp::Ordering::Less => (from..to).contains(&minute),
            std::cmp::Ordering::Greater => minute >= from || minute < to,
            std::cmp::Ordering::Equal => false,
        }
    }

    /// Whether `kind` goes to the phone at all.
    pub fn sends(&self, kind: PhoneKind) -> bool {
        self.phone.contains(&kind)
    }

    /// The quiet hours as the Connections tab shows them: `22:00 to
    /// 07:00`, or `off`.
    pub fn quiet_label(&self) -> String {
        match &self.quiet {
            Some((from, to)) => format!("{from} to {to}"),
            None => "off".into(),
        }
    }

    /// `phone` as the Connections tab shows it: `questions, plans,
    /// crashes`, or `nothing`.
    pub fn phone_label(&self) -> String {
        let words: Vec<&str> = PhoneKind::ALL
            .iter()
            .filter(|kind| self.sends(**kind))
            .map(|kind| kind.plural())
            .collect();
        match words.is_empty() {
            true => "nothing".into(),
            false => words.join(", "),
        }
    }
}

/// `HH:MM` as minutes of the day.
pub fn parse_hhmm(text: &str) -> Option<u32> {
    let (h, m) = text.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Quiet hours typed as one line — `22:00 to 07:00`, `22:00-07:00`, or
/// empty / `off` for none. Err with the line to show when it doesn't read.
pub fn parse_quiet(text: &str) -> Result<Option<(String, String)>, String> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let (from, to) = text
        .split_once(" to ")
        .or_else(|| text.split_once('-'))
        .ok_or_else(|| "quiet hours read like 22:00 to 07:00".to_string())?;
    let canon = |t: &str| {
        parse_hhmm(t)
            .map(|m| format!("{:02}:{:02}", m / 60, m % 60))
            .ok_or_else(|| format!("{} is not a time like 07:00", t.trim()))
    };
    Ok(Some((canon(from)?, canon(to)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mins(m: u64) -> Option<Duration> {
        Some(Duration::from_secs(m * 60))
    }

    #[test]
    fn the_screen_lock_idle_and_the_hand_flag_each_make_you_away() {
        let away = AwaySettings::default();
        let here = Signals {
            locked: Some(false),
            idle: mins(1),
        };
        assert!(!away.is_away(here, false));
        assert!(away.is_away(here, true), "by hand");
        let locked = Signals {
            locked: Some(true),
            ..here
        };
        assert!(away.is_away(locked, false), "locked");
        let no_lock = AwaySettings {
            screen_lock: false,
            ..AwaySettings::default()
        };
        assert!(!no_lock.is_away(locked, false), "the lock switched off");
        let idle = Signals {
            locked: Some(false),
            idle: mins(10),
        };
        assert!(away.is_away(idle, false), "ten minutes idle");
        assert!(!away.is_away(
            Signals {
                idle: mins(9),
                ..idle
            },
            false
        ));
        let never = AwaySettings {
            idle_minutes: 0,
            ..AwaySettings::default()
        };
        assert!(!never.is_away(idle, false), "idle 0 is never");
        assert!(!away.is_away(Signals::default(), false), "nothing read");
    }

    #[test]
    fn quiet_hours_wrap_past_midnight() {
        let away = AwaySettings::default();
        let at = |h: u32, m: u32| h * 60 + m;
        assert!(away.is_quiet(at(22, 0)));
        assert!(away.is_quiet(at(3, 30)));
        assert!(away.is_quiet(at(6, 59)));
        assert!(!away.is_quiet(at(7, 0)));
        assert!(!away.is_quiet(at(12, 0)));
        let day = AwaySettings {
            quiet: Some(("12:00".into(), "13:30".into())),
            ..AwaySettings::default()
        };
        assert!(day.is_quiet(at(12, 45)));
        assert!(!day.is_quiet(at(13, 30)));
        let none = AwaySettings {
            quiet: None,
            ..AwaySettings::default()
        };
        assert!(!none.is_quiet(at(23, 0)));
        let broken = AwaySettings {
            quiet: Some(("late".into(), "07:00".into())),
            ..AwaySettings::default()
        };
        assert!(!broken.is_quiet(at(23, 0)));
    }

    #[test]
    fn quiet_hours_are_typed_as_one_line() {
        assert_eq!(
            parse_quiet("22:00 to 07:00"),
            Ok(Some(("22:00".into(), "07:00".into())))
        );
        assert_eq!(
            parse_quiet("9:5-17:00"),
            Ok(Some(("09:05".into(), "17:00".into())))
        );
        assert_eq!(parse_quiet(" off "), Ok(None));
        assert_eq!(parse_quiet(""), Ok(None));
        assert!(parse_quiet("25:00 to 07:00").is_err());
        assert!(parse_quiet("tonight").is_err());
    }

    #[test]
    fn the_phone_kinds_read_as_a_list() {
        let mut away = AwaySettings::default();
        assert_eq!(away.phone_label(), "questions, plans, crashes");
        away.phone = vec![PhoneKind::Crash, PhoneKind::Question];
        assert_eq!(away.phone_label(), "questions, crashes");
        away.phone.clear();
        assert_eq!(away.phone_label(), "nothing");
        assert_eq!(AwaySettings::default().quiet_label(), "22:00 to 07:00");
    }
}
