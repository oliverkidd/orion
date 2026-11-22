//! The other services through the RESOLVER: account usage and Microsoft
//! 365.
//!
//! | Key | Asked | Lands in |
//! |---|---|---|
//! | `Usage(account)` | the ACCOUNT USAGE modal (`⇧U`); once as an agent picker or a launch box opens, or the factory is about to choose an account ([`glance_usage`]); 10 min stale | `usage::land` |
//! | `Calendar` | every 30 min while MEETING TAKEAWAYS is on; else as the footer's next meeting comes on screen or the window takes focus back, 30 min stale | `microsoft::land_calendar` |
//! | `Transcript(meeting)` | at 2, 10 and 30 minutes after a meeting ends, while MEETING TAKEAWAYS is on | `meeting_takeaways::land` |
//!
//! A usage endpoint's 429 pauses every usage read for an hour
//! (`gate::USAGE_PAUSE`); Graph's pauses Microsoft for ten minutes
//! (`gate::GRAPH_PAUSE`).

use std::time::{Duration, Instant};

use super::spawn::Landed;
use super::{gate, FetchError, QueryKey, Seen};
use crate::app::{App, Overlay};
use crate::fetch::Ticket;

/// The period a transcript due a look is needed at. Its look's landing
/// moves it on to its next time (and out of the needs until then), so
/// this only paces one whose look is held off — a backoff, a pause.
const TRANSCRIPT_DUE: Duration = Duration::from_secs(60);

/// What the screen shows of these services: every account's usage while
/// the ACCOUNT USAGE modal is up, and the calendar while the footer's
/// next meeting is on and Microsoft 365 is signed in.
pub fn visible(app: &App) -> Vec<(QueryKey, Seen)> {
    let mut seen = Vec::new();
    if matches!(app.overlay, Some(Overlay::Usage(_))) && !crate::usage::usage_off() {
        for account in &app.usage.accounts {
            seen.push((QueryKey::Usage(account.id.clone()), Seen::Focused));
        }
    }
    let ms = &app.microsoft.settings;
    if ms.footer_meeting
        && app.microsoft.connected(ms)
        && crate::microsoft::calendar::interval().is_some()
    {
        seen.push((QueryKey::Calendar, Seen::Shown));
    }
    seen
}

/// What MEETING TAKEAWAYS needs while it is on: the calendar every half
/// hour, and each ended meeting's transcript when its next look is due.
pub fn needs(app: &App) -> Vec<(QueryKey, Duration)> {
    let mut needs = Vec::new();
    if !crate::automations::meeting_takeaways::watching(app) {
        return needs;
    }
    if let Some(every) = crate::microsoft::calendar::interval() {
        needs.push((QueryKey::Calendar, every));
    }
    for id in crate::automations::meeting_takeaways::transcripts_due(app, chrono::Utc::now()) {
        needs.push((QueryKey::Transcript(id), TRANSCRIPT_DUE));
    }
    needs
}

/// One look at every account's usage, as something about to show or use
/// what each has left opens: an agent picker, a launch box, the factory
/// choosing where to move. Read when older than 10 minutes, and not
/// otherwise. The account list is read again first when it is old.
pub fn glance_usage(app: &mut App) {
    if crate::usage::usage_off() {
        return;
    }
    crate::usage::relist_if_old(app);
    let now = Instant::now();
    let ids: Vec<String> = app.usage.accounts.iter().map(|a| a.id.clone()).collect();
    for id in ids {
        app.resolver.ask(QueryKey::Usage(id), now);
    }
}

/// What one of these fetches brought.
#[derive(Debug)]
pub enum Answer {
    /// An account's usage, and what the resolver makes of it: `None` for
    /// an answer — a reading, or a sign-in that says why there is none —
    /// else why not.
    Usage {
        result: Result<crate::usage::Reading, crate::usage::Problem>,
        error: Option<FetchError>,
    },
    Calendar(Result<Vec<crate::microsoft::calendar::Event>, FetchError>),
    /// A meeting's transcript looked for: the online meeting's id when it
    /// was found, and the transcript with your name, or none yet.
    Transcript {
        online_id: Option<String>,
        found: crate::automations::meeting_takeaways::Found,
        error: Option<FetchError>,
    },
}

impl Answer {
    /// Whether the fetch answered, for the resolver.
    pub fn outcome(&self) -> Result<(), &FetchError> {
        match self {
            Answer::Usage { error, .. } | Answer::Transcript { error, .. } => {
                error.as_ref().map_or(Ok(()), Err)
            }
            Answer::Calendar(events) => events.as_ref().map(|_| ()),
        }
    }
}

/// What the resolver makes of a usage ask's [`Problem`](crate::usage::Problem):
/// a 429 pauses every usage read for [`gate::USAGE_PAUSE`]; a refused or
/// missing login is an answer — nothing to ask again until it is stale;
/// anything else is a failure, backed off.
pub fn usage_error(
    result: &Result<crate::usage::Reading, crate::usage::Problem>,
    now: Instant,
) -> Option<FetchError> {
    use crate::usage::Problem;
    match result {
        Ok(_) | Err(Problem::SignedOut | Problem::Expired) => None,
        Err(Problem::RateLimited) => Some(FetchError::RateLimited {
            until: now + gate::USAGE_PAUSE,
        }),
        Err(Problem::Failed(why)) => Some(FetchError::Failed(why.clone())),
    }
}

/// Start the fetch `ticket` asks for, off the loop, its answer sent on
/// `tx`. A key whose account or meeting is gone lands at once as
/// [`FetchError::Missing`].
pub fn spawn(app: &App, ticket: Ticket<QueryKey>, tx: tokio::sync::mpsc::UnboundedSender<Landed>) {
    let unanswered = tx.clone();
    let send = move |ticket, answer| {
        let _ = tx.send(Landed {
            ticket,
            answer: super::spawn::Answer::Service(answer),
        });
    };
    match ticket.key.clone() {
        QueryKey::Usage(id) => {
            let Some(account) = app.usage.accounts.iter().find(|a| a.id == id).cloned() else {
                let result = Err(crate::usage::Problem::Failed("not an account".into()));
                let error = Some(FetchError::Missing);
                return send(ticket, Answer::Usage { result, error });
            };
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || crate::usage::fetch(&account))
                    .await
                    .unwrap_or_else(|e| Err(crate::usage::Problem::Failed(e.to_string())));
                let error = usage_error(&result, Instant::now());
                send(ticket, Answer::Usage { result, error });
            });
        }
        QueryKey::Calendar => {
            let ms = app.microsoft.settings.clone();
            tokio::spawn(async move {
                let events = tokio::task::spawn_blocking(move || {
                    crate::microsoft::calendar::fetch(&ms, chrono::Utc::now())
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
                .map_err(graph_failure);
                send(ticket, Answer::Calendar(events));
            });
        }
        QueryKey::Transcript(id) => {
            let Some(meeting) = app.automations.meetings.meeting(&id).cloned() else {
                let error = Some(FetchError::Missing);
                let found = Err("the meeting is gone".into());
                return send(
                    ticket,
                    Answer::Transcript {
                        online_id: None,
                        found,
                        error,
                    },
                );
            };
            let ms = app.microsoft.settings.clone();
            tokio::spawn(async move {
                let (online_id, found) = tokio::task::spawn_blocking(move || {
                    crate::automations::meeting_takeaways::look_blocking(
                        &ms,
                        &meeting.join_url,
                        meeting.online_id.as_deref(),
                        meeting.start,
                    )
                })
                .await
                .unwrap_or_else(|e| (None, Err(e.to_string())));
                // Not there yet is an answer: the meeting's own schedule
                // says when to look again.
                let error = found.as_ref().err().map(|why| graph_failure(why.clone()));
                send(
                    ticket,
                    Answer::Transcript {
                        online_id,
                        found,
                        error,
                    },
                );
            });
        }
        other => {
            tracing::debug!(key = ?other, "no service fetch answers this key");
            let answer = super::spawn::Answer::Unanswered;
            let _ = unanswered.send(Landed { ticket, answer });
        }
    }
}

/// A Graph call's error as the resolver takes it: a 429 pauses Microsoft.
fn graph_failure(why: String) -> FetchError {
    gate::failure(why, gate::GRAPH_PAUSE, Instant::now())
}

/// One of these fetches landed, the resolver told already: what it
/// brought goes to the store that keeps it.
pub fn land(app: &mut App, key: QueryKey, answer: Answer) {
    match (key, answer) {
        (QueryKey::Usage(id), Answer::Usage { result, .. }) => {
            crate::usage::land(app, crate::usage::Answer { id, result });
        }
        (QueryKey::Calendar, Answer::Calendar(events)) => {
            let events = events.map_err(|e| e.summary());
            crate::microsoft::land_calendar(app, events);
        }
        (
            QueryKey::Transcript(id),
            Answer::Transcript {
                online_id, found, ..
            },
        ) => {
            use crate::automations::meeting_takeaways::{land, Answer};
            land(
                app,
                Answer::Looked {
                    id,
                    online_id,
                    found,
                },
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Problem;

    /// A 429 pauses usage reads for an hour; a missing or expired login
    /// is an answer, not a failure to retry.
    #[test]
    fn a_usage_429_pauses_usage_for_an_hour_and_a_sign_in_is_an_answer() {
        let now = Instant::now();
        assert_eq!(
            usage_error(&Err(Problem::RateLimited), now),
            Some(FetchError::RateLimited {
                until: now + Duration::from_secs(3600)
            })
        );
        assert_eq!(usage_error(&Err(Problem::SignedOut), now), None);
        assert_eq!(usage_error(&Err(Problem::Expired), now), None);
        assert_eq!(
            usage_error(&Err(Problem::Failed("x".into())), now),
            Some(FetchError::Failed("x".into()))
        );
    }

    /// The paused usage provider holds every account back, `r` included,
    /// until the hour is up.
    #[test]
    fn a_429_holds_every_account_back_until_the_pause_lifts() {
        let t0 = Instant::now();
        let mut r = super::super::Resolver::default();
        let (a, b) = (
            QueryKey::Usage("claude".into()),
            QueryKey::Usage("cursor".into()),
        );
        r.visit(vec![(a.clone(), Seen::Focused)], t0);
        let ticket = r.due(t0).pop().unwrap();
        let error = usage_error(&Err(Problem::RateLimited), t0).unwrap();
        r.land(&ticket, Err(&error), t0);
        r.visit(
            vec![(a.clone(), Seen::Focused), (b.clone(), Seen::Focused)],
            t0,
        );
        r.refresh(&b);
        assert!(r.due(t0 + Duration::from_secs(60)).is_empty(), "paused");
        let later = t0 + Duration::from_secs(3601);
        assert_eq!(r.due(later).len(), 2, "both once it lifts");
    }

    fn signed_in() -> App {
        let mut app = App::new();
        app.microsoft.email = Some("ada@riplo.ai".into());
        app.microsoft.settings.client_id = "3f1c2a7e-9b4d-4e21-8c55-1a2b3c4d5e6f".into();
        app
    }

    /// The calendar is on screen only with the footer's meeting on and
    /// someone signed in; it is read in the background only while meeting
    /// takeaways is on.
    #[test]
    fn the_calendar_follows_the_footer_and_the_automation() {
        use crate::automations::Automation;
        let mut app = App::new();
        app.microsoft.settings.footer_meeting = true;
        assert!(visible(&app).is_empty(), "nobody signed in");
        let mut app = signed_in();
        app.microsoft.settings.footer_meeting = true;
        app.microsoft.settings.takeaways = true;
        assert_eq!(visible(&app), [(QueryKey::Calendar, Seen::Shown)]);
        app.microsoft.settings.footer_meeting = false;
        assert!(visible(&app).is_empty(), "the footer's meeting off");
        app.automations
            .settings
            .of_mut(Automation::MeetingTakeaways)
            .on = Some(false);
        assert!(needs(&app).is_empty(), "the automation off: no beat");
        app.automations
            .settings
            .of_mut(Automation::MeetingTakeaways)
            .on = Some(true);
        assert_eq!(
            needs(&app),
            [(
                QueryKey::Calendar,
                crate::microsoft::calendar::DEFAULT_INTERVAL
            )]
        );
        app.microsoft.settings.takeaways = false;
        assert!(needs(&app).is_empty(), "transcripts off in Connections");
    }

    /// A meeting's transcript is looked for at 2, 10 and 30 minutes after
    /// it ends, then given up on.
    #[test]
    fn a_transcript_is_looked_for_three_times_after_the_meeting() {
        use crate::automations::meeting_takeaways::{Meeting, Stage};
        use crate::automations::Automation;
        let mut app = signed_in();
        app.microsoft.settings.takeaways = true;
        app.automations
            .settings
            .of_mut(Automation::MeetingTakeaways)
            .on = Some(true);
        let end = chrono::Utc::now() - chrono::Duration::minutes(1);
        let event = crate::microsoft::calendar::Event {
            id: "evt".into(),
            subject: "Standup".into(),
            start: end - chrono::Duration::minutes(15),
            end,
            online: true,
            join_url: Some("https://teams/x".into()),
        };
        app.automations
            .meetings
            .kept
            .meetings
            .push(Meeting::ended(&event));
        let due = |app: &App, after_end: i64| {
            crate::automations::meeting_takeaways::transcripts_due(
                app,
                end + chrono::Duration::minutes(after_end),
            )
        };
        assert!(due(&app, 1).is_empty(), "not before two minutes");
        assert_eq!(due(&app, 2), ["evt"]);
        let look = |app: &mut App| {
            land(
                app,
                QueryKey::Transcript("evt".into()),
                Answer::Transcript {
                    online_id: Some("MS1".into()),
                    found: Ok(None),
                    error: None,
                },
            )
        };
        look(&mut app);
        let m = app.automations.meetings.meeting("evt").unwrap();
        assert_eq!((m.looks, m.online_id.as_deref()), (1, Some("MS1")));
        assert!(due(&app, 9).is_empty(), "the next at ten");
        assert_eq!(due(&app, 10), ["evt"]);
        look(&mut app);
        assert!(due(&app, 29).is_empty(), "the last at thirty");
        assert_eq!(due(&app, 30), ["evt"]);
        look(&mut app);
        assert!(due(&app, 90).is_empty(), "three looks, then none");
        assert_eq!(
            app.automations.meetings.meeting("evt").unwrap().stage,
            Stage::Waiting,
            "the tick gives up on it"
        );
    }
}
