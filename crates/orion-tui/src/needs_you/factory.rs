//! The board's FACTORY cards: every software factory card posted to you
//! and still open (`App::factory_cards`, `factory::cards`) — a plan, a
//! question, a verdict, the day's new rules, the week's fixes — in the
//! AGENTS column, marked `◆`, with the Slack card's own buttons. While
//! you are at the desk this is where a card is answered: it reaches
//! Slack only once you go away.
//!
//! - **Enter** on one opens its buttons as a menu, the card's words above
//!   them; there **1–9** presses the button of that number at once. On
//!   the board itself a digit types, as any key does: a search may start
//!   with one. A button that asks for words (`Reply…`) opens a box for
//!   them first, and a link button (`Open PR`) opens its link in the
//!   browser, the card left as it is.
//! - **A press here** is the owner's ([`press`]): it goes to the module
//!   that owns the card (`factory::cards::on_action`), exactly as a tap
//!   on Slack does, and the Slack card is then closed with the button's
//!   label (`factory::cards::close`) — unless that module already closed
//!   it, or posted it again to keep it open (a verdict's 👍 / 👎).
//! - **A press on Slack** marks the card acted, and it leaves the board.
//!
//! Only the cards that went to you are mirrored: a thin ticket's question
//! to its reporter is theirs to answer.

use chrono::{DateTime, Utc};

use super::model::{Card, CardSource, CardTarget, Section};
use crate::app::{App, ContextMenu, MenuAction, MenuItem, Overlay, PromptKind};
use crate::factory::cards::{self, CardState, Posted, Recipient};

/// Where a factory card sorts among the sessions: with the ones waiting
/// on you.
const URGENCY: u8 = 1;
/// The widest line of a card's words in its menu.
const MENU_TEXT_W: usize = 72;
/// The most lines of a card's words its menu shows.
const MENU_TEXT_LINES: usize = 10;

/// The open cards posted to you.
fn open(app: &App) -> impl Iterator<Item = &Posted> {
    app.factory_cards
        .iter()
        .filter(|p| p.state == CardState::Open && p.card.to == Recipient::Owner)
}

/// The open card under `key`.
fn posted<'a>(app: &'a App, key: &str) -> Option<&'a Posted> {
    open(app).find(|p| p.card.key == key)
}

/// The FACTORY cards' source.
pub struct Factory;

impl CardSource for Factory {
    fn cards(&self, app: &App) -> Vec<Card> {
        open(app)
            .map(|p| {
                let labels: Vec<&str> = p.card.buttons.iter().map(|b| b.label.as_str()).collect();
                let mut sub = p.card.context.clone();
                if !labels.is_empty() {
                    sub.push_str(&format!(" · {}", labels.join(" · ")));
                }
                Card::new(
                    Section::Agents,
                    CardTarget::Factory {
                        key: p.card.key.clone(),
                    },
                    p.card.title.clone(),
                    sub,
                    DateTime::from_timestamp_millis(p.at_ms).unwrap_or_else(Utc::now),
                    URGENCY,
                )
                .with_mark(("◆", app.theme.accent))
            })
            .collect()
    }
}

/// Enter on the card under `key`: its buttons as a menu, numbered as the
/// keys that press them, its words above them.
pub(crate) fn open_menu(app: &mut App, key: &str) {
    let Some(p) = posted(app, key) else {
        return;
    };
    let items: Vec<MenuItem> = p
        .card
        .buttons
        .iter()
        .enumerate()
        .map(|(n, b)| {
            let mut item = MenuItem::new(
                b.label.clone(),
                MenuAction::FactoryPress {
                    key: key.to_string(),
                    action: b.action.clone(),
                    label: b.label.clone(),
                    asks_text: b.asks_text,
                },
            );
            if n < 9 {
                item.hint = Some((n + 1).to_string());
            }
            item
        })
        .collect();
    if items.is_empty() {
        return;
    }
    let (title, words) = (p.card.title.clone(), words(&p.card.body));
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some(title),
        items,
        at: None,
        hover: 0,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter: None,
        message: (!words.is_empty()).then_some(words),
    }));
    app.dirty = true;
}

/// A card's body as its menu shows it: Slack's markup gone, wrapped, and
/// cut to [`MENU_TEXT_LINES`].
fn words(body: &[String]) -> String {
    let mut lines: Vec<String> = body
        .iter()
        .map(|line| plain(line))
        .filter(|line| !line.trim().is_empty())
        .flat_map(|line| crate::pr_preview::wrap(&line, MENU_TEXT_W))
        .collect();
    if lines.len() > MENU_TEXT_LINES {
        lines.truncate(MENU_TEXT_LINES);
        lines.push("…".into());
    }
    lines.join("\n")
}

/// Slack mrkdwn as plain text: bold and italic marks gone, `<url|label>`
/// as its label, the escapes undone.
fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let link = &rest[open + 1..open + close];
        out.push_str(link.split_once('|').map_or(link, |(_, label)| label));
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out.replace(['*', '_'], "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The row digit `c` presses in `menu`, when it is a card's menu
/// ([`open_menu`]) with a button of that number.
pub(crate) fn numbered_row(menu: &ContextMenu, c: char) -> Option<usize> {
    let n = (c.to_digit(10)? as usize).checked_sub(1)?;
    menu.items
        .get(n)
        .filter(|item| matches!(item.action, MenuAction::FactoryPress { .. }))
        .map(|_| n)
}

/// A button chosen from a card's menu, or by its number: pressed, or —
/// one that asks for words — a box for them first, or — a link — its
/// link opened, as Slack does, with nothing handed back.
pub(crate) fn chosen(app: &mut App, key: String, action: String, label: String, asks_text: bool) {
    let url = posted(app, &key)
        .and_then(|p| p.card.buttons.iter().find(|b| b.action == action))
        .and_then(|b| b.url.clone());
    if let Some(url) = url {
        crate::open_url::open_url(&url, None);
        return;
    }
    match asks_text {
        true => crate::event_loop::open_prompt(app, PromptKind::FactoryText { key, action, label }),
        false => press(app, &key, &action, None),
    }
}

/// The FactoryText box's title and label.
pub(crate) fn prompt_text(app: &App, key: &str, label: &str) -> (String, String) {
    let title = posted(app, key).map_or("Factory card", |p| p.card.title.as_str());
    (
        format!("{} · {title}", label.trim_end_matches('…')),
        "sent as your answer on the card — Enter sends it".into(),
    )
}

/// Press `action` on the card under `key` as its owner, with `text` for a
/// button that asked for words: handed to the card's module as a Slack
/// tap is, then the Slack card closed with the button's label — unless
/// the module closed it, or posted it again, itself.
pub(crate) fn press(app: &mut App, key: &str, action: &str, text: Option<String>) {
    let label = posted(app, key)
        .and_then(|p| p.card.buttons.iter().find(|b| b.action == action))
        .map(|b| b.label.trim_end_matches('…').to_string())
        .unwrap_or_else(|| action.to_string());
    cards::on_action(app, key, action, text, true);
    let still_acted = matches!(cards::state_of(app, key), Some(CardState::Acted(_)));
    if still_acted {
        cards::close(app, key, &label);
    }
    super::refresh(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::cards::{Button, Card as SlackCard};
    use orion_core::ClientRequest;

    /// Enter on the card under `key`, then digit `n` in its menu, as the
    /// loop does. False when the menu has no such button.
    fn press_nth(app: &mut App, key: &str, n: u32) -> bool {
        open_menu(app, key);
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            return false;
        };
        let Some(row) = numbered_row(menu, char::from_digit(n, 10).unwrap()) else {
            app.overlay = None;
            return false;
        };
        let MenuAction::FactoryPress {
            key,
            action,
            label,
            asks_text,
        } = menu.items[row].action.clone()
        else {
            unreachable!("only a factory press is numbered");
        };
        app.overlay = None;
        chosen(app, key, action, label, asks_text);
        true
    }

    fn slack_card(key: &str) -> SlackCard {
        SlackCard {
            key: key.into(),
            to: Recipient::Owner,
            context: "Bug fixer · riplo-os".into(),
            title: "Fix the login loop".into(),
            body: vec!["*Merge*: nothing risky, see <https://x.y/7|#7>".into()],
            buttons: vec![
                Button::tap("approve", "Approve"),
                Button::text("reply", "Reply…"),
            ],
            image: None,
        }
    }

    #[test]
    fn every_open_card_to_you_is_mirrored_with_its_buttons() {
        let mut app = App::new();
        cards::post(&mut app, slack_card("issue:RIPLO-1"));
        let mut theirs = slack_card("issue:RIPLO-2");
        theirs.to = Recipient::Email("ann@example.com".into());
        cards::post(&mut app, theirs);
        cards::post(&mut app, slack_card("issue:RIPLO-3"));
        cards::close(&mut app, "issue:RIPLO-3", "Skipped");
        let mirrored = Factory.cards(&app);
        assert_eq!(mirrored.len(), 1);
        let card = &mirrored[0];
        assert_eq!(card.title, "Fix the login loop");
        assert_eq!(card.sub, "Bug fixer · riplo-os · Approve · Reply…");
        assert_eq!(card.section, Section::Agents);
        assert_eq!(card.key.0, "factory:issue:RIPLO-1");
        assert_eq!(card.mark.map(|m| m.0), Some("◆"));
    }

    #[test]
    fn enter_lists_the_buttons_numbered_under_the_cards_words() {
        let mut app = App::new();
        cards::post(&mut app, slack_card("issue:RIPLO-1"));
        open_menu(&mut app, "issue:RIPLO-1");
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("no menu: {:?}", app.overlay.is_some());
        };
        let rows: Vec<(&str, Option<&str>)> = menu
            .items
            .iter()
            .map(|i| (i.label.as_str(), i.hint.as_deref()))
            .collect();
        assert_eq!(rows, [("Approve", Some("1")), ("Reply…", Some("2"))]);
        assert_eq!(
            menu.message.as_deref(),
            Some("Merge: nothing risky, see #7")
        );
    }

    #[test]
    fn a_press_here_acts_and_closes_the_slack_card() {
        let mut app = App::new();
        cards::post(&mut app, slack_card("week:2026-11-14"));
        assert!(press_nth(&mut app, "week:2026-11-14", 1));
        let Some(ClientRequest::UpdateFactoryCard { key, card }) = app.outbox.last() else {
            panic!("{:?}", app.outbox);
        };
        assert_eq!(key, "week:2026-11-14");
        let blocks = card.blocks.as_array().unwrap();
        assert_eq!(blocks.last().unwrap()["elements"][0]["text"], "✓ Approve");
        assert!(Factory.cards(&app).is_empty(), "gone from the board");
        assert!(
            !press_nth(&mut app, "week:2026-11-14", 1),
            "and from its keys"
        );
    }

    #[test]
    fn a_button_that_asks_for_words_opens_a_box_first() {
        let mut app = App::new();
        cards::post(&mut app, slack_card("issue:RIPLO-1"));
        assert!(press_nth(&mut app, "issue:RIPLO-1", 2));
        let Some(Overlay::Prompt(prompt)) = &app.overlay else {
            panic!("no box");
        };
        assert!(matches!(
            &prompt.kind,
            PromptKind::FactoryText { key, action, .. } if key == "issue:RIPLO-1" && action == "reply"
        ));
        assert!(!press_nth(&mut app, "issue:RIPLO-1", 3), "no third button");
    }

    #[test]
    fn a_link_button_opens_its_link_and_leaves_the_card_up() {
        let mut app = App::new();
        let mut card = slack_card("pr:https://x.y/7:verdict");
        card.buttons
            .insert(0, Button::link("Open PR", "https://x.y/7"));
        cards::post(&mut app, card);
        crate::open_url::take_opened();
        let sent = app.outbox.len();
        assert!(press_nth(&mut app, "pr:https://x.y/7:verdict", 1));
        assert_eq!(crate::open_url::take_opened(), ["https://x.y/7"]);
        assert_eq!(app.outbox.len(), sent, "nothing dispatched or closed");
        assert_eq!(Factory.cards(&app).len(), 1, "still on the board");
    }

    #[test]
    fn digits_number_only_a_cards_buttons() {
        let mut app = App::new();
        cards::post(&mut app, slack_card("issue:RIPLO-1"));
        open_menu(&mut app, "issue:RIPLO-1");
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("no menu");
        };
        assert_eq!(numbered_row(menu, '1'), Some(0));
        assert_eq!(numbered_row(menu, '2'), Some(1));
        assert_eq!(numbered_row(menu, '3'), None, "no third button");
        let mut other = menu.clone();
        other.items = vec![MenuItem::new("Open PRs", MenuAction::ToggleOpenPrs)];
        assert_eq!(numbered_row(&other, '1'), None, "not a card's menu");
    }

    #[test]
    fn a_verdict_vote_here_leaves_the_card_up_with_its_buttons() {
        let mut app = App::new();
        let key = "pr:https://x.y/7:verdict";
        let mut card = slack_card(key);
        card.buttons = vec![
            Button::tap("merge", "Merge"),
            Button::tap("right", "👍"),
            Button::tap("wrong", "👎"),
        ];
        cards::post(&mut app, card);
        // pr_body puts a voted card back up as it was, the vote noted.
        crate::pr_body::remember_card_sent(&mut app, "https://x.y/7", "abc");
        press(&mut app, key, "right", None);
        assert!(
            !matches!(
                app.outbox.last(),
                Some(ClientRequest::UpdateFactoryCard { .. })
            ),
            "not closed: {:?}",
            app.outbox.last()
        );
        assert_eq!(Factory.cards(&app).len(), 1, "still on the board");
    }

    #[test]
    fn a_press_on_slack_takes_the_card_off_the_board() {
        let mut app = App::new();
        cards::post(&mut app, slack_card("issue:RIPLO-1"));
        cards::on_action(&mut app, "issue:RIPLO-1", "approve", None, true);
        assert!(Factory.cards(&app).is_empty());
    }
}
