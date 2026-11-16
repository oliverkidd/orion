//! THE FACTORY AT ITS LIMIT: when the account the factory runs on
//! ([`super::account`]) reports a usage limit — a model ask refused with
//! `You've hit your weekly limit · resets Nov 20, 9am`, or a factory
//! session stopped with a `usage_limit` — the factory pauses until that
//! limit resets:
//!
//! - **Nothing new starts.** No automation starts a session
//!   (`super::may_start`) and no runner looks for work (`super::tick`);
//!   an ask on the paused account is refused at once with the pause's
//!   own line ([`refusal`], in `oneshot::run_in`), so the backfill and a
//!   queued harvest stop at the first limit rather than failing every
//!   pull request after it.
//! - **One notice.** The app says it once, in orion, never in Slack, and
//!   offers to move the factory to the next account (`super::account`).
//!   orion never switches accounts on its own.
//! - **It lifts itself.** Past the reset time the pause reads as none
//!   ([`current`]), and the factory carries on where it was.
//!
//! The pause lives in `<data_dir>/automations/factory-pause.json`, so the
//! `orion knowledge backfill` command — another process — and the app see
//! the same one.

use std::path::PathBuf;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use serde::{Deserialize, Serialize};

use crate::oneshot::Account;

/// How long the factory waits when a limit says no reset time it can
/// read: then it tries again, and pauses again if the limit still holds.
pub const UNREAD_RESET: Duration = Duration::hours(1);

/// The factory paused on one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    /// The account's registry id (`claude`, `claude-2`).
    pub account: String,
    /// What the screens call it: `Work (a@b.co)`.
    pub label: String,
    /// The account's CLI and its environment, as an ask on it runs them
    /// (`oneshot::Account`): what [`refusal`] knows it by.
    pub program: String,
    pub env: Vec<(String, String)>,
    /// When the limit resets, in ms since the epoch.
    pub until_ms: i64,
    /// Claude's own line about it.
    pub said: String,
}

impl Pause {
    /// The factory paused on the account registered as `account`, asked
    /// through `ask`, for what was `said` at `at`: the reset read from it
    /// as of then, else [`UNREAD_RESET`] after.
    pub fn of(account: &str, ask: &Account, said: &str, at: DateTime<Local>) -> Self {
        let until_ms = reset_at(said, at).unwrap_or_else(|| (at + UNREAD_RESET).timestamp_millis());
        Pause {
            account: account.to_string(),
            label: ask.label.clone(),
            program: ask.program.clone(),
            env: ask.env.clone(),
            until_ms,
            said: crate::ui::truncate(said.trim(), 200),
        }
    }

    /// Whether an ask through `ask` is one on this paused account.
    pub fn is_on(&self, ask: &Account) -> bool {
        self.program == ask.program && self.env == ask.env
    }

    /// `3:45pm`, `Tue 9:00am`, `Nov 20 9:00am`: when it resets.
    pub fn until(&self, now: DateTime<Local>) -> String {
        when(self.until_ms, now)
    }

    /// The pause's own line: what a refused ask says.
    pub fn line(&self, now: DateTime<Local>) -> String {
        format!(
            "{} is at its limit until {}, so the factory is paused",
            self.label,
            self.until(now)
        )
    }
}

/// Whether `why` — a refused ask's reason, a session's limit line — is a
/// usage limit: Claude's `You've hit your … limit · resets …`, `… limit
/// reached ∙ resets …`, `Claude AI usage limit reached|<epoch>`, running
/// out of extra usage — or the factory's own pause ([`Pause::line`]).
pub fn is_limit(why: &str) -> bool {
    let why = why.to_ascii_lowercase();
    if why.contains("out of extra usage") {
        return true;
    }
    why.contains("limit")
        && [
            "resets",
            "hit your",
            "limit reached",
            "usage limit",
            "at its limit",
        ]
        .iter()
        .any(|mark| why.contains(mark))
}

/// When the limit `said` resets, in ms since the epoch, as of `now`:
/// `resets 3:45pm` (today, or tomorrow once it has passed), `resets Mon
/// 9am`, `resets Nov 20, 9am (Europe/London)`, `resets in 2h 14m`, or the
/// epoch after a `|`. A time is read in this machine's zone. None when it
/// says nothing readable.
pub fn reset_at(said: &str, now: DateTime<Local>) -> Option<i64> {
    if let Some((_, epoch)) = said.rsplit_once('|') {
        if let Ok(secs) = epoch.trim().parse::<i64>() {
            return Some(secs * 1000);
        }
    }
    let lower = said.to_ascii_lowercase();
    let at = lower.find("resets")? + "resets".len();
    let rest = &lower[at..];
    let rest = rest.split(['(', '\n', '·', '∙', '|']).next()?;
    let rest = rest.trim().trim_start_matches(':').trim();
    let rest = rest
        .strip_prefix("at ")
        .unwrap_or(rest)
        .trim_end_matches('.');
    if let Some(span) = rest.strip_prefix("in ") {
        return span_of(span).map(|d| (now + d).timestamp_millis());
    }
    let words: Vec<&str> = rest
        .split([' ', ','])
        .map(str::trim)
        .filter(|w| !w.is_empty() && !matches!(*w, "at" | "on"))
        .collect();
    let mut time = None;
    let mut month = None;
    let mut day = None;
    let mut weekday = None;
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        // `9 am` as two words is `9am`.
        let joined = words
            .get(i + 1)
            .filter(|next| matches!(**next, "am" | "pm"))
            .map(|next| format!("{word}{next}"));
        if let Some(t) = joined.as_deref().and_then(time_of) {
            time = Some(t);
            i += 2;
            continue;
        }
        if let Some(t) = time_of(word) {
            time = Some(t);
        } else if let Some(m) = month_of(word) {
            month = Some(m);
        } else if let Some(w) = weekday_of(word) {
            weekday = Some(w);
        } else if let Ok(d) = word
            .trim_end_matches(['s', 't', 'n', 'd', 'r', 'h'])
            .parse::<u32>()
        {
            day = Some(d);
        }
        i += 1;
    }
    if time.is_none() && month.is_none() && weekday.is_none() {
        return None;
    }
    let time = time.unwrap_or(NaiveTime::MIN);
    let today = now.date_naive();
    let local = |date: NaiveDate| {
        Local
            .from_local_datetime(&date.and_time(time))
            .earliest()
            .map(|at| at.timestamp_millis())
    };
    let now_ms = now.timestamp_millis();
    let date = match (month, day, weekday) {
        (Some(month), Some(day), _) => {
            let this_year = NaiveDate::from_ymd_opt(today.year(), month, day)?;
            match local(this_year)? < now_ms {
                true => NaiveDate::from_ymd_opt(today.year() + 1, month, day)?,
                false => this_year,
            }
        }
        (_, _, Some(weekday)) => {
            let ahead =
                (7 + weekday.num_days_from_monday() - today.weekday().num_days_from_monday()) % 7;
            let date = today + Duration::days(i64::from(ahead));
            match local(date)? <= now_ms {
                true => date + Duration::days(7),
                false => date,
            }
        }
        _ => match local(today)? <= now_ms {
            true => today.succ_opt()?,
            false => today,
        },
    };
    local(date)
}

/// `3pm`, `3:45pm`, `3:45 pm` (joined), `15:00`.
fn time_of(word: &str) -> Option<NaiveTime> {
    let (clock, half) = match word {
        w if w.ends_with("am") => (&w[..w.len() - 2], Some(false)),
        w if w.ends_with("pm") => (&w[..w.len() - 2], Some(true)),
        w if w.contains(':') => (w, None),
        _ => return None,
    };
    let (h, m) = match clock.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (clock.parse::<u32>().ok()?, 0),
    };
    let h = match half {
        Some(pm) if (1..=12).contains(&h) => h % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

fn month_of(word: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let word = word.trim_end_matches('.');
    (word.len() >= 3)
        .then(|| MONTHS.iter().position(|m| word.starts_with(m)))
        .flatten()
        .map(|at| at as u32 + 1)
}

fn weekday_of(word: &str) -> Option<Weekday> {
    let word = word.trim_end_matches('.');
    (word.len() >= 3).then_some(())?;
    [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ]
    .into_iter()
    .find(|d| word.starts_with(&d.to_string().to_ascii_lowercase()))
}

/// `2h 14m`, `38m`, `3d 9h`, `1 hour 5 minutes`.
fn span_of(span: &str) -> Option<Duration> {
    let mut total = Duration::zero();
    let mut number: Option<i64> = None;
    let mut any = false;
    for word in span.split_whitespace() {
        let digits: String = word.chars().take_while(|c| c.is_ascii_digit()).collect();
        let unit = &word[digits.len()..];
        if !digits.is_empty() {
            number = digits.parse().ok();
        }
        let Some(n) = number.filter(|_| !unit.is_empty()) else {
            continue;
        };
        let part = match unit.chars().next() {
            Some('d') => Duration::days(n),
            Some('h') => Duration::hours(n),
            Some('m') => Duration::minutes(n),
            Some('s') => Duration::seconds(n),
            _ => continue,
        };
        total += part;
        number = None;
        any = true;
    }
    any.then_some(total)
}

/// `when_ms` as a time to read: `3:45pm` today, `Tue 9:00am` this week,
/// `Nov 20 9:00am` past that.
pub fn when(when_ms: i64, now: DateTime<Local>) -> String {
    let Some(at) = Local.timestamp_millis_opt(when_ms).single() else {
        return "later".into();
    };
    let days = (at.date_naive() - now.date_naive()).num_days();
    let clock = at.format("%-I:%M%P").to_string();
    match days {
        0 => clock,
        1..=6 => format!("{} {clock}", at.format("%a")),
        _ => format!("{} {clock}", at.format("%b %-d")),
    }
}

// ---- the pause on disk ----

#[cfg(test)]
thread_local! {
    static TEST_FILE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with the pause kept at `path`. Under test, with none named,
/// nothing is kept: a test never reads or writes the real one.
#[cfg(test)]
pub fn with_file<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    let before = TEST_FILE.with(|p| p.replace(Some(path)));
    let out = f();
    TEST_FILE.with(|p| *p.borrow_mut() = before);
    out
}

fn file() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_FILE.with(|p| p.borrow().clone())
    }
    #[cfg(not(test))]
    {
        Some(super::state_dir().join("factory-pause.json"))
    }
}

/// The pause as it stands, while its reset is still ahead of `now`.
pub fn current_at(now: DateTime<Local>) -> Option<Pause> {
    let text = std::fs::read_to_string(file()?).ok()?;
    let pause: Pause = serde_json::from_str(&text).ok()?;
    (pause.until_ms > now.timestamp_millis()).then_some(pause)
}

/// The pause as it stands now.
pub fn current() -> Option<Pause> {
    current_at(Local::now())
}

/// Keep `pause`.
pub fn set(pause: &Pause) {
    let Some(path) = file() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(pause) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                tracing::warn!("factory: couldn't keep the pause: {e}");
            }
        }
        Err(e) => tracing::warn!("factory: couldn't write the pause: {e}"),
    }
}

/// Lift the pause.
pub fn clear() {
    if let Some(path) = file() {
        let _ = std::fs::remove_file(path);
    }
}

/// An ask through `ask` refused with `why`: when that is a usage limit
/// and `ask` is the factory's account, the factory pauses. The pause, when
/// it did.
pub fn note_refusal(ask: &Account, why: &str) -> Option<Pause> {
    if !is_limit(why) {
        return None;
    }
    let cfg = crate::config::Config::load();
    let entry = super::account::ask_entry(&cfg)?;
    if crate::oneshot::Account::of(&entry) != *ask {
        return None;
    }
    let now = Local::now();
    if let Some(held) = current_at(now).filter(|p| p.account == entry.id) {
        return Some(held);
    }
    let pause = Pause::of(&entry.id, ask, why, now);
    tracing::warn!(
        "factory: {} is at its limit until {}, pausing: {why}",
        pause.label,
        pause.until(now)
    );
    set(&pause);
    Some(pause)
}

/// Why an ask through `ask` doesn't run: the factory is paused on that
/// very account. None when it may run.
pub fn refusal(ask: &Account) -> Option<String> {
    let now = Local::now();
    current_at(now)
        .filter(|p| p.is_on(ask))
        .map(|p| p.line(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monday 16 November 2026, 10:00 here.
    fn monday_ten() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, 16, 10, 0, 0)
            .single()
            .unwrap()
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn a_limit_is_told_from_any_other_failure() {
        for said in [
            "You've hit your weekly limit · resets Nov 20, 9am (Europe/London)",
            "You've hit your limit · resets 3pm",
            "5-hour limit reached ∙ resets 3pm",
            "Claude AI usage limit reached|1763650800",
            "You're out of extra usage · resets 4pm",
            "Work (a@b.co) is at its limit until 3:00pm, so the factory is paused",
        ] {
            assert!(is_limit(said), "{said}");
        }
        for said in [
            "Not logged in · Please run /login",
            "claude timed out after 600s",
            "the model's JSON did not read: expected value",
            "a rule about rate limits in the codebase",
        ] {
            assert!(!is_limit(said), "{said}");
        }
    }

    #[test]
    fn the_reset_time_is_read_in_every_shape_claude_says_it() {
        let now = monday_ten();
        let reset = |said: &str| reset_at(said, now);
        assert_eq!(
            reset("You've hit your session limit · resets 3:45pm"),
            Some(at(2026, 11, 16, 15, 45))
        );
        assert_eq!(
            reset("You've hit your limit · resets 9am"),
            Some(at(2026, 11, 17, 9, 0)),
            "a time already past today is tomorrow's"
        );
        assert_eq!(
            reset("You've hit your weekly limit · resets Nov 20, 9am (Europe/London)"),
            Some(at(2026, 11, 20, 9, 0))
        );
        assert_eq!(
            reset("weekly limit reached ∙ resets Nov 20 at 9:30 am"),
            Some(at(2026, 11, 20, 9, 30))
        );
        assert_eq!(
            reset("You've hit your limit · resets Jan 2, 5pm"),
            Some(at(2027, 1, 2, 17, 0)),
            "a date already past this year is next year's"
        );
        assert_eq!(
            reset("Weekly limit reached ∙ resets Thu 9am"),
            Some(at(2026, 11, 19, 9, 0))
        );
        assert_eq!(
            reset("Weekly limit reached ∙ resets Monday at 8am"),
            Some(at(2026, 11, 23, 8, 0)),
            "today's weekday already past is next week's"
        );
        assert_eq!(
            reset("limit reached · resets 18:30"),
            Some(at(2026, 11, 16, 18, 30))
        );
        assert_eq!(
            reset("limit reached · resets in 2h 14m"),
            Some(at(2026, 11, 16, 12, 14))
        );
        assert_eq!(
            reset("Claude AI usage limit reached|1763650800"),
            Some(1_763_650_800_000)
        );
        assert_eq!(reset("You've hit your limit"), None);
        assert_eq!(reset("resets soon"), None);
    }

    #[test]
    fn a_reset_reads_as_a_time_of_day_a_weekday_or_a_date() {
        let now = monday_ten();
        assert_eq!(when(at(2026, 11, 16, 15, 45), now), "3:45pm");
        assert_eq!(when(at(2026, 11, 19, 9, 0), now), "Thu 9:00am");
        assert_eq!(when(at(2026, 12, 1, 9, 0), now), "Dec 1 9:00am");
    }

    fn account(label: &str, dir: &str) -> Account {
        Account {
            program: "claude".into(),
            env: vec![("CLAUDE_CONFIG_DIR".into(), dir.into())],
            label: label.into(),
        }
    }

    /// A pause read from a limit's line holds until its reset, refuses
    /// asks on its own account only, and lifts itself after.
    #[test]
    fn the_pause_holds_until_the_reset_and_lifts_itself() {
        let dir = tempfile::tempdir().unwrap();
        with_file(dir.path().join("pause.json"), || {
            let now = monday_ten();
            let work = account("Work", "/w");
            let said = "You've hit your weekly limit · resets 3pm";
            let pause = Pause::of("claude-work", &work, said, now);
            assert_eq!(pause.until_ms, at(2026, 11, 16, 15, 0));
            set(&pause);
            assert_eq!(current_at(now), Some(pause.clone()));
            assert!(pause.is_on(&work));
            assert!(!pause.is_on(&account("Personal", "/p")));
            assert_eq!(
                pause.line(now),
                "Work is at its limit until 3:00pm, so the factory is paused"
            );
            assert!(
                is_limit(&pause.line(now)),
                "its refusal reads as a limit too"
            );
            let after = Local
                .with_ymd_and_hms(2026, 11, 16, 15, 0, 1)
                .single()
                .unwrap();
            assert_eq!(current_at(after), None, "resumed after the reset");
            clear();
            assert_eq!(current_at(now), None);
        });
    }

    #[test]
    fn a_limit_with_no_reset_waits_an_hour() {
        let now = monday_ten();
        let pause = Pause::of(
            "claude",
            &account("Work", "/w"),
            "You've hit your limit",
            now,
        );
        assert_eq!(pause.until_ms, (now + UNREAD_RESET).timestamp_millis());
    }

    #[test]
    fn with_no_file_named_a_test_keeps_no_pause() {
        let now = monday_ten();
        set(&Pause::of(
            "claude",
            &account("Work", "/w"),
            "resets 3pm",
            now,
        ));
        assert_eq!(current_at(now), None);
    }
}
