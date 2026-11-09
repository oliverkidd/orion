//! The CALENDAR POLL: the signed-in user's meetings from now to twelve
//! hours on, read from Graph's `calendarView` every few minutes off the
//! loop ([`interval`]; the e2e tests turn it off). The footer shows the
//! next one ([`footer_text`]); meeting takeaways read the list
//! (`App::microsoft.events`) for online meetings that have ended.

use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;

use super::graph;
use crate::config::MicrosoftSettings;

/// How often the calendar is read.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// How far ahead a meeting shows in the footer.
pub const SOON: chrono::Duration = chrono::Duration::minutes(60);

/// How far ahead the poll reads.
const AHEAD: chrono::Duration = chrono::Duration::hours(12);

/// The most of a meeting's subject the footer spends cells on.
const SUBJECT_MAX: usize = 28;

/// One meeting on the calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Graph's id for the event.
    pub id: String,
    pub subject: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// A Teams (or other online) meeting.
    pub online: bool,
    /// Its join link, for an online meeting — what
    /// `/me/onlineMeetings?$filter=JoinWebUrl eq '…'` finds it by.
    pub join_url: Option<String>,
}

impl Event {
    /// Started and not yet over at `now`.
    pub fn running(&self, now: DateTime<Utc>) -> bool {
        self.start <= now && now < self.end
    }
}

/// The poll's cadence: `ORION_MICROSOFT_POLL_SECS` when set, `None` when
/// it is `0` (off — the e2e tests); else [`DEFAULT_INTERVAL`].
pub fn interval() -> Option<Duration> {
    orion_core::env::secs_override(orion_core::env::MICROSOFT_POLL_SECS, DEFAULT_INTERVAL)
}

/// The `calendarView` ask for `now` to twelve hours on.
pub fn path(now: DateTime<Utc>) -> String {
    let at = |t: DateTime<Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    format!(
        "/me/calendarView?startDateTime={}&endDateTime={}\
         &$select=id,subject,start,end,isAllDay,isCancelled,isOnlineMeeting,onlineMeeting\
         &$orderby=start/dateTime&$top=50",
        at(now),
        at(now + AHEAD),
    )
}

/// Read the calendar (blocking).
pub fn fetch(ms: &MicrosoftSettings, now: DateTime<Utc>) -> Result<Vec<Event>, String> {
    // Times in UTC, so they parse without a zone table.
    let headers = ["Prefer: outlook.timezone=\"UTC\"".to_string()];
    let body = graph::get(ms, &path(now), &headers)?;
    parse(&body)
}

#[derive(Deserialize)]
struct View {
    #[serde(default)]
    value: Vec<RawEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    #[serde(default)]
    id: String,
    #[serde(default)]
    subject: Option<String>,
    start: RawTime,
    end: RawTime,
    #[serde(default)]
    is_all_day: bool,
    #[serde(default)]
    is_cancelled: bool,
    #[serde(default)]
    is_online_meeting: bool,
    #[serde(default)]
    online_meeting: Option<RawOnline>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTime {
    date_time: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOnline {
    #[serde(default)]
    join_url: Option<String>,
}

/// A `calendarView` answer (times in UTC) as meetings, start order. All-day
/// and cancelled events are not meetings to show.
pub fn parse(body: &[u8]) -> Result<Vec<Event>, String> {
    let view: View =
        serde_json::from_slice(body).map_err(|e| format!("the calendar didn't read: {e}"))?;
    let mut events: Vec<Event> = view
        .value
        .into_iter()
        .filter(|e| !e.is_all_day && !e.is_cancelled)
        .filter_map(|e| {
            Some(Event {
                start: utc(&e.start.date_time)?,
                end: utc(&e.end.date_time)?,
                id: e.id,
                subject: e
                    .subject
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Meeting".into()),
                online: e.is_online_meeting,
                join_url: e.online_meeting.and_then(|o| o.join_url),
            })
        })
        .collect();
    events.sort_by_key(|e| e.start);
    Ok(events)
}

/// Graph's `2026-11-08T10:00:00.0000000`, already in UTC.
fn utc(text: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(text.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|t| t.and_utc())
}

/// The meeting the footer is about at `now`: one running, else the next to
/// start within the hour.
pub fn next(events: &[Event], now: DateTime<Utc>) -> Option<&Event> {
    events.iter().find(|e| e.running(now)).or_else(|| {
        events
            .iter()
            .find(|e| e.start > now && e.start - now <= SOON)
    })
}

/// The footer's meeting: `▸ Standup in 12m · Teams`, `▸ Standup now · Teams`
/// while it runs; nothing with no meeting within the hour.
pub fn footer_text(events: &[Event], now: DateTime<Utc>) -> Option<String> {
    let event = next(events, now)?;
    let when = if event.running(now) {
        "now".to_string()
    } else {
        // Whole minutes, rounded up: a meeting 30 s off is `in 1m`.
        let secs = (event.start - now).num_seconds().max(1);
        format!("in {}m", (secs + 59) / 60)
    };
    let subject = crate::ui::truncate(&event.subject, SUBJECT_MAX);
    let place = if event.online { " · Teams" } else { "" };
    Some(format!("▸ {subject} {when}{place}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 8, h, m, 0).unwrap()
    }

    /// A `calendarView` answer shaped as Graph documents it.
    const VIEW: &str = r#"{
        "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users('ada')/calendarView",
        "value": [
            {
                "id": "AAMkAGI2THVSAAA=",
                "subject": "Planning",
                "isAllDay": false,
                "isCancelled": false,
                "isOnlineMeeting": false,
                "onlineMeeting": null,
                "start": {"dateTime": "2026-11-08T14:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-11-08T15:00:00.0000000", "timeZone": "UTC"}
            },
            {
                "id": "AAMkAGI2THVTAAA=",
                "subject": "Standup",
                "isAllDay": false,
                "isCancelled": false,
                "isOnlineMeeting": true,
                "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc"},
                "start": {"dateTime": "2026-11-08T10:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-11-08T10:15:00.0000000", "timeZone": "UTC"}
            },
            {
                "id": "AAMkAGI2THVUAAA=",
                "subject": "Holiday",
                "isAllDay": true,
                "isCancelled": false,
                "isOnlineMeeting": false,
                "start": {"dateTime": "2026-11-08T00:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-11-09T00:00:00.0000000", "timeZone": "UTC"}
            },
            {
                "id": "AAMkAGI2THVVAAA=",
                "subject": "Cancelled sync",
                "isAllDay": false,
                "isCancelled": true,
                "isOnlineMeeting": true,
                "start": {"dateTime": "2026-11-08T11:00:00.0000000", "timeZone": "UTC"},
                "end": {"dateTime": "2026-11-08T11:30:00.0000000", "timeZone": "UTC"}
            }
        ]
    }"#;

    #[test]
    fn a_calendar_view_reads_as_meetings_in_start_order() {
        let events = parse(VIEW.as_bytes()).unwrap();
        let subjects: Vec<&str> = events.iter().map(|e| e.subject.as_str()).collect();
        assert_eq!(
            subjects,
            ["Standup", "Planning"],
            "no all-day, no cancelled"
        );
        let standup = &events[0];
        assert_eq!(standup.start, at(10, 0));
        assert_eq!(standup.end, at(10, 15));
        assert!(standup.online);
        assert_eq!(
            standup.join_url.as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc")
        );
        assert!(!events[1].online);
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn the_ask_covers_now_to_twelve_hours_on() {
        let path = path(at(9, 30));
        assert!(path.starts_with(
            "/me/calendarView?startDateTime=2026-11-08T09:30:00Z&endDateTime=2026-11-08T21:30:00Z"
        ));
        assert!(path.contains("$select=id,subject,start,end,"));
    }

    #[test]
    fn the_footer_counts_down_the_hour_then_says_now() {
        let events = parse(VIEW.as_bytes()).unwrap();
        assert_eq!(
            footer_text(&events, at(8, 30)),
            None,
            "more than an hour off"
        );
        assert_eq!(
            footer_text(&events, at(9, 48)).as_deref(),
            Some("▸ Standup in 12m · Teams")
        );
        let half_a_minute = at(9, 59) + chrono::Duration::seconds(30);
        assert_eq!(
            footer_text(&events, half_a_minute).as_deref(),
            Some("▸ Standup in 1m · Teams")
        );
        assert_eq!(
            footer_text(&events, at(10, 5)).as_deref(),
            Some("▸ Standup now · Teams")
        );
        // Over: the next one, which is no Teams meeting.
        assert_eq!(
            footer_text(&events, at(13, 15)).as_deref(),
            Some("▸ Planning in 45m")
        );
        assert_eq!(footer_text(&events, at(15, 0)), None, "the day is done");
    }

    #[test]
    fn a_long_subject_is_cut() {
        let events = vec![Event {
            id: "1".into(),
            subject: "Quarterly planning with the whole engineering org".into(),
            start: at(10, 0),
            end: at(11, 0),
            online: true,
            join_url: None,
        }];
        let text = footer_text(&events, at(9, 50)).unwrap();
        assert!(text.chars().count() < 50, "{text}");
        assert!(text.ends_with(" in 10m · Teams"), "{text}");
    }
}
