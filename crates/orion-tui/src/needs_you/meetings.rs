//! The MEETINGS section: a meeting whose takeaways wait for you to place
//! them (`automations::meeting_takeaways`), or whose transcript couldn't
//! be read. Enter opens its TAKEAWAYS SCREEN, or reads it again.

use super::model::{Card, CardSource, CardTarget, Section};
use crate::app::App;
use crate::automations::meeting_takeaways::{Meeting, Stage};

/// Meeting takeaways' meetings, as cards.
pub struct Meetings;

impl CardSource for Meetings {
    fn cards(&self, app: &App) -> Vec<Card> {
        app.automations
            .meetings
            .kept
            .meetings
            .iter()
            .filter_map(card)
            .collect()
    }
}

/// `meeting`'s card, while it waits on you.
pub fn card(meeting: &Meeting) -> Option<Card> {
    let (sub, urgency) = match &meeting.stage {
        Stage::ToPlace => match meeting.accepted() {
            1 => ("1 takeaway for you to place".to_string(), 0),
            n => (format!("{n} takeaways for you to place"), 0),
        },
        Stage::Failed(why) => (format!("couldn't read the takeaways · {why}"), 1),
        _ => return None,
    };
    Some(Card::new(
        Section::Meetings,
        CardTarget::Meeting {
            id: meeting.id.clone(),
        },
        meeting.subject.clone(),
        sub,
        meeting.end,
        urgency,
    ))
}
