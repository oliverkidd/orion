//! The REPLY PANEL: `r` on a SLACK card answers it from orion, in place
//! of the board's right two columns — the conversation's last
//! [`SHOWN`] messages over a multi-row field, `Enter` sends, `⇧Enter`
//! breaks the line, `Esc` goes back to the board with the draft kept for
//! the next `r` on the same card.
//!
//! Both asks go to Slack's Web API with the user token, through
//! [`super::slack`]'s `call`, off the loop: `conversations.replies` for a
//! card in a thread or `conversations.history` for a conversation's top
//! level (neither needs a new scope), and `chat.postMessage`, so the
//! reply goes as you, with `thread_ts` for a thread. That one needs the
//! user scope `chat:write`; an install from before it was in the manifest
//! answers `missing_scope`, which says what to do ([`MISSING_SCOPE`]).
//! A reply sent hides its card at once (done until a newer message),
//! before Slack's echo of it clears the card again.

use std::collections::BTreeMap;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use serde_json::Value;

use super::model::{Card, CardKey, CardTarget, Section};
use super::slack::{self, Waiting};
use crate::app::{App, Overlay};
use crate::hints::Hint;
use crate::text_input::{TextInput, TextView};
use crate::theme::Theme;

/// The panel's keys, as its key arm matches them and the board's border
/// spells them.
pub mod keys {
    use crate::hints::Key;

    /// On a SLACK card, on the board: the panel.
    pub const REPLY: Key = Key::new(&["r"], "reply");
    pub const SEND: Key = Key::new(&["enter"], "send");
    pub const NEWLINE: Key = Key::new(&["shift+enter", "alt+enter", "ctrl+j"], "new line");
    pub const BACK: Key = crate::nav::CLOSE.doing("back");

    #[cfg(test)]
    #[test]
    fn every_reply_key_parses() {
        for key in [REPLY, SEND, NEWLINE, BACK] {
            assert!(key.parses(), "{:?}", key.chords);
        }
    }
}

/// How many of the conversation's messages the panel shows.
pub const SHOWN: usize = 6;

/// How far into a thread `conversations.replies` reads, oldest first, to
/// find its last [`SHOWN`].
const THREAD_LIMIT: usize = 200;

/// What a send says when the token can't post: an install from before
/// `chat:write` was in the manifest.
pub const MISSING_SCOPE: &str = "Reply needs Slack's chat:write. Reinstall Orion from the \
     manifest and paste the new user token in Settings › Connections.";

/// One message of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    /// Its sender's user id; empty for a bot's.
    pub user: String,
    pub text: String,
    pub ts: String,
}

/// The panel, while it is up: what it answers, what it read, the draft.
#[derive(Debug, Clone)]
pub struct Panel {
    /// The SLACK card it answers.
    pub card: Card,
    pub channel: String,
    /// The card's conversation ([`Waiting::root`]).
    pub root: Option<String>,
    /// The thread it sits in, which the reply goes to.
    pub thread: Option<String>,
    pub direct: bool,
    /// The newest message's sender: whom a DM is with.
    pub user: String,
    /// The conversation, once Slack answers (None while it is asked).
    pub said: Option<Result<Vec<Said>, String>>,
    pub input: TextInput,
    /// A send is out.
    pub sending: bool,
    /// Names by user id and channel names, as [`super::refresh`] last
    /// copied them from the store.
    pub users: BTreeMap<String, String>,
    pub channels: BTreeMap<String, String>,
    /// Where Send was drawn, for the pointer, and the field's rows.
    pub send_at: Option<Rect>,
    pub view: Option<TextView>,
}

impl Panel {
    /// `#eng · thread`, `DM · Ada`: where the reply goes.
    pub fn place(&self) -> String {
        if self.direct {
            let name = self.users.get(&self.user).map_or("Someone", String::as_str);
            return format!("DM · {name}");
        }
        let channel = self
            .channels
            .get(&self.channel)
            .map_or_else(|| "a channel".to_string(), |name| format!("#{name}"));
        match self.thread {
            Some(_) => format!("{channel} · thread"),
            None => channel,
        }
    }
}

/// A Web API ask the panel makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The conversation's last messages: a thread's, or a top level's.
    Read {
        channel: String,
        thread: Option<String>,
    },
    /// The reply, as you, to the conversation `(channel, root)`.
    Post {
        channel: String,
        root: Option<String>,
        thread: Option<String>,
        text: String,
    },
}

impl Ask {
    /// The Web API method and its form.
    pub fn request(&self) -> (&'static str, Vec<(&'static str, String)>) {
        match self {
            Ask::Read {
                channel,
                thread: Some(ts),
            } => (
                "conversations.replies",
                vec![
                    ("channel", channel.clone()),
                    ("ts", ts.clone()),
                    ("limit", THREAD_LIMIT.to_string()),
                ],
            ),
            Ask::Read {
                channel,
                thread: None,
            } => (
                "conversations.history",
                vec![("channel", channel.clone()), ("limit", SHOWN.to_string())],
            ),
            Ask::Post {
                channel,
                thread,
                text,
                ..
            } => {
                let mut form = vec![("channel", channel.clone()), ("text", text.clone())];
                if let Some(ts) = thread {
                    form.push(("thread_ts", ts.clone()));
                }
                ("chat.postMessage", form)
            }
        }
    }

    /// Ask Slack. Blocks: run it under `spawn_blocking`.
    fn run(&self) -> Result<Got, String> {
        let (method, form) = self.request();
        let form: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let reply = slack::call(method, &form)?;
        Ok(match self {
            Ask::Read { .. } => Got::Read(messages(&reply)),
            Ask::Post { .. } => Got::Sent,
        })
    }
}

/// What an ask got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Got {
    Read(Vec<Said>),
    Sent,
}

/// An ask's answer, for the loop.
#[derive(Debug)]
pub struct Landed {
    pub ask: Ask,
    pub got: Result<Got, String>,
}

/// The last [`SHOWN`] messages of a `conversations.*` answer, oldest
/// first — `history` answers newest first, `replies` oldest first.
fn messages(reply: &Value) -> Vec<Said> {
    let text = |m: &Value, key: &str| m[key].as_str().unwrap_or_default().to_string();
    let mut said: Vec<Said> = reply["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| Said {
            user: text(m, "user"),
            text: text(m, "text"),
            ts: text(m, "ts"),
        })
        .filter(|m| !m.text.trim().is_empty())
        .collect();
    said.sort_by_key(|m| slack::ts_key(&m.ts));
    let skip = said.len().saturating_sub(SHOWN);
    said.split_off(skip)
}

/// The open board's panel.
fn panel_mut(app: &mut App) -> Option<&mut Panel> {
    match &mut app.overlay {
        Some(Overlay::Palette(palette)) => palette.board.reply.as_mut(),
        _ => None,
    }
}

/// Ask Slack off the loop.
fn ask(app: &mut App, ask: Ask) {
    #[cfg(test)]
    app.needs_you.reply_asks.push(ask.clone());
    let Some(tx) = app.needs_you.tx.clone() else {
        return;
    };
    tokio::spawn(async move {
        let asked = ask.clone();
        let got = tokio::task::spawn_blocking(move || asked.run())
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        let _ = tx.send(super::Answer::Reply(Landed { ask, got }));
    });
}

/// The conversation behind a SLACK card.
fn waiting_of<'a>(app: &'a App, card: &Card) -> Option<&'a Waiting> {
    let CardTarget::SlackMessage { channel, root, .. } = &card.target else {
        return None;
    };
    app.needs_you
        .store
        .slack
        .waiting
        .iter()
        .find(|w| &w.channel == channel && &w.root == root)
}

/// `r` on the board: the panel for the SLACK card under the cursor, its
/// draft back, and the conversation asked for.
pub fn open(app: &mut App) {
    let Some(card) = super::selected(app).filter(|c| c.section == Section::Slack) else {
        return;
    };
    let Some(w) = waiting_of(app, &card).cloned() else {
        return;
    };
    let draft = app.needs_you.reply_drafts.remove(&card.key);
    let mut input = TextInput::multiline();
    if let Some(draft) = draft {
        input.set_text(draft);
    }
    let thread = w.in_thread.then(|| w.root.clone()).flatten();
    let kept = &app.needs_you.store.slack;
    let panel = Panel {
        card,
        channel: w.channel.clone(),
        root: w.root.clone(),
        thread: thread.clone(),
        direct: w.direct,
        user: w.user.clone(),
        said: None,
        input,
        sending: false,
        users: kept.users.clone(),
        channels: kept.channels.clone(),
        send_at: None,
        view: None,
    };
    if let Some(Overlay::Palette(palette)) = &mut app.overlay {
        palette.board.reply = Some(panel);
    }
    ask(
        app,
        Ask::Read {
            channel: w.channel,
            thread,
        },
    );
    app.dirty = true;
}

/// A key while the panel is up: `Enter` sends, `⇧Enter` breaks the line,
/// `Esc` goes back; everything else edits the draft.
pub fn key(app: &mut App, key: &KeyEvent) {
    let Some(panel) = panel_mut(app) else {
        return;
    };
    if panel.input.takes_newline(key) {
        panel.input.handle_key(key);
    } else if keys::SEND.matches(key) {
        send(app);
    } else if keys::BACK.matches(key) {
        back(app);
    } else {
        panel.input.handle_key(key);
    }
    app.dirty = true;
}

/// The pointer while the panel is up: a click on Send is `Enter`, and
/// the field takes the pointer the way every field does.
pub fn mouse(app: &mut App, event: &MouseEvent) {
    let at = Position::new(event.column, event.row);
    let Some(panel) = panel_mut(app) else {
        return;
    };
    let on_send = panel.send_at.is_some_and(|r| r.contains(at));
    if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) && on_send {
        send(app);
    } else if panel.input.under(at) || panel.input.pressed() {
        panel.input.mouse(event);
    }
}

/// `Enter`, or a click on Send: the draft goes to Slack as you.
pub fn send(app: &mut App) {
    let Some(panel) = panel_mut(app) else {
        return;
    };
    let text = panel.input.as_str().trim_end().to_string();
    if panel.sending || text.trim().is_empty() {
        return;
    }
    panel.sending = true;
    let post = Ask::Post {
        channel: panel.channel.clone(),
        root: panel.root.clone(),
        thread: panel.thread.clone(),
        text,
    };
    let to = panel.place();
    app.flash = Some(crate::flash::Flash::working(format!("Sending to {to}")));
    ask(app, post);
}

/// `Esc`: back to the board, the draft kept for the card.
fn back(app: &mut App) {
    let Some(Overlay::Palette(palette)) = &mut app.overlay else {
        return;
    };
    let Some(panel) = palette.board.reply.take() else {
        return;
    };
    if !panel.input.trim().is_empty() {
        app.needs_you
            .reply_drafts
            .insert(panel.card.key, panel.input.as_str().to_string());
    }
}

/// The key of the SLACK card for conversation `(channel, root)`.
fn card_key(channel: &str, root: &Option<String>) -> CardKey {
    CardKey::of(&CardTarget::SlackMessage {
        channel: channel.to_string(),
        root: root.clone(),
        ts: String::new(),
    })
}

/// An ask's answer lands: the conversation on the panel, or the reply
/// sent — its card gone and the panel with it — or why it wasn't.
pub fn landed(app: &mut App, landed: Landed) {
    let Landed { ask, got } = landed;
    match (ask, got) {
        (Ask::Read { channel, thread }, got) => {
            let said = got.map(|got| match got {
                Got::Read(said) => said,
                Got::Sent => Vec::new(),
            });
            let unnamed: Vec<String> = said
                .iter()
                .flatten()
                .map(|m| m.user.clone())
                .filter(|u| !u.is_empty() && !app.needs_you.store.slack.users.contains_key(u))
                .collect();
            if let Some(panel) =
                panel_mut(app).filter(|p| p.channel == channel && p.thread == thread)
            {
                panel.said = Some(said);
            }
            for user in unnamed {
                slack::ask_name(app, user);
            }
        }
        (Ask::Post { channel, root, .. }, Ok(_)) => sent(app, &channel, &root),
        (Ask::Post { channel, root, .. }, Err(why)) => {
            if let Some(panel) = panel_mut(app).filter(|p| p.channel == channel && p.root == root) {
                panel.sending = false;
            }
            let say = match why.as_str() {
                "missing_scope" => MISSING_SCOPE.to_string(),
                _ => format!("Slack didn't send it: {why}"),
            };
            app.flash = Some(crate::flash::Flash::failed(say));
        }
    }
    app.dirty = true;
}

/// The reply went: its card hidden until a newer message and out of the
/// waiting, its draft and panel gone, and the flash says where it went.
fn sent(app: &mut App, channel: &str, root: &Option<String>) {
    let key = card_key(channel, root);
    app.needs_you.reply_drafts.remove(&key);
    let to = panel_place(app, channel, root);
    let kept = &app.needs_you.store.slack;
    if let Some(w) = kept
        .waiting
        .iter()
        .find(|w| w.channel == channel && &w.root == root)
    {
        let card = slack::card(kept, w);
        app.needs_you.store.done(&card);
    }
    if let Some(Overlay::Palette(palette)) = &mut app.overlay {
        if palette
            .board
            .reply
            .as_ref()
            .is_some_and(|p| p.card.key == key)
        {
            palette.board.reply = None;
        }
    }
    let mut next = app.needs_you.store.slack.clone();
    next.waiting
        .retain(|w| !(w.channel == channel && &w.root == root));
    slack::commit(app, next);
    app.flash = Some(crate::flash::Flash::done(format!("Sent to {to}")));
}

/// Where a reply to `(channel, root)` went: the open panel's words for
/// it, else the channel's name.
fn panel_place(app: &App, channel: &str, root: &Option<String>) -> String {
    if let Some(Overlay::Palette(palette)) = &app.overlay {
        if let Some(panel) = palette
            .board
            .reply
            .as_ref()
            .filter(|p| p.channel == channel && &p.root == root)
        {
            return panel.place();
        }
    }
    match app.needs_you.store.slack.channels.get(channel) {
        Some(name) => format!("#{name}"),
        None => "Slack".into(),
    }
}

/// The panel's keys line, on the board's bottom border.
pub fn hints() -> Vec<Hint> {
    vec![
        keys::SEND.hint().kept(),
        keys::NEWLINE.hint(),
        keys::BACK.hint().kept(),
    ]
}

/// The rows the field may take, its box's borders aside.
const FIELD_MAX: u16 = 8;

/// Draw the panel over `area`: `REPLY  #eng · thread`, the conversation,
/// the field in its box and Send under it. Writes back where Send and
/// the field's rows were drawn.
pub fn draw(f: &mut Frame, panel: &mut Panel, area: Rect, th: Theme) {
    if area.height < 6 || area.width < 12 {
        return;
    }
    let heading = Line::from(vec![
        Span::styled(
            "REPLY",
            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", panel.place()), Style::default().fg(th.text)),
    ]);
    f.render_widget(Paragraph::new(heading), Rect { height: 1, ..area });

    // The field: as many rows as its text, between two and FIELD_MAX.
    let width = area.width.saturating_sub(2);
    let rows = panel.input.rows(usize::from(width.max(1))).len() as u16;
    let field_h = rows.clamp(2, FIELD_MAX) + 2;
    let send_y = area.bottom() - 1;
    let field = Rect {
        y: send_y.saturating_sub(field_h),
        height: field_h,
        ..area
    };
    let color = if panel.sending { th.dim } else { th.accent };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(" Reply as you ", Style::default().fg(color)));
    let inner = block.inner(field);
    f.render_widget(block, field);
    let (view, _) = crate::ui::draw_multiline_input(f, &panel.input, inner, th);
    panel.view = Some(view);

    // Send, on the right under the box: a button, Enter's twin.
    let label = if panel.sending {
        " Sending… "
    } else {
        " Send ↵ "
    };
    let send_w = label.chars().count() as u16;
    let send = Rect {
        x: area.right().saturating_sub(send_w),
        y: send_y,
        width: send_w.min(area.width),
        height: 1,
    };
    let style = if panel.sending {
        Style::default().fg(th.dim)
    } else {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    };
    f.render_widget(Paragraph::new(Span::styled(label, style)), send);
    panel.send_at = Some(send);

    // The conversation, its newest message just over the box.
    let talk = Rect {
        y: area.y + 2,
        height: field.y.saturating_sub(area.y + 3),
        ..area
    };
    let lines = conversation(panel, usize::from(talk.width), th);
    let skip = lines.len().saturating_sub(usize::from(talk.height));
    f.render_widget(
        Paragraph::new(lines.into_iter().skip(skip).collect::<Vec<_>>()),
        talk,
    );
}

/// The conversation as lines `width` wide: each message's sender and age,
/// then its text wrapped under them, a blank line between two.
fn conversation(panel: &Panel, width: usize, th: Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(th.dim);
    let said = match &panel.said {
        None => return vec![Line::from(Span::styled("Reading the conversation…", dim))],
        Some(Err(why)) => {
            return vec![Line::from(Span::styled(
                crate::ui::truncate(&format!("Couldn't read the conversation: {why}"), width),
                dim,
            ))]
        }
        Some(Ok(said)) if said.is_empty() => {
            return vec![Line::from(Span::styled("No messages to show", dim))]
        }
        Some(Ok(said)) => said,
    };
    let now = chrono::Utc::now();
    let mut lines = Vec::new();
    for (i, m) in said.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        let name = panel.users.get(&m.user).map_or("Someone", String::as_str);
        let age = crate::hosts::ago_short((now - slack::time_of(&m.ts)).num_seconds());
        lines.push(Line::from(vec![
            Span::styled(
                crate::ui::truncate(name, width.saturating_sub(age.chars().count() + 2)),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {age}"), dim),
        ]));
        let text = slack::plain(&m.text, &panel.users);
        for row in crate::pr_preview::wrap(&text, width.saturating_sub(2)) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(row, Style::default().fg(th.muted)),
            ]));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_is_read_with_replies_and_a_top_level_with_history() {
        let thread = Ask::Read {
            channel: "C1".into(),
            thread: Some("10.000000".into()),
        };
        assert_eq!(
            thread.request(),
            (
                "conversations.replies",
                vec![
                    ("channel", "C1".to_string()),
                    ("ts", "10.000000".to_string()),
                    ("limit", "200".to_string())
                ]
            )
        );
        let top = Ask::Read {
            channel: "D1".into(),
            thread: None,
        };
        assert_eq!(top.request().0, "conversations.history");
        assert!(top.request().1.contains(&("limit", "6".to_string())));
    }

    #[test]
    fn a_reply_in_a_thread_names_it_and_one_at_the_top_does_not() {
        let post = |thread: Option<&str>| Ask::Post {
            channel: "C1".into(),
            root: Some("10.000000".into()),
            thread: thread.map(str::to_string),
            text: "on it".into(),
        };
        let (method, form) = post(Some("10.000000")).request();
        assert_eq!(method, "chat.postMessage");
        assert_eq!(
            form,
            vec![
                ("channel", "C1".to_string()),
                ("text", "on it".to_string()),
                ("thread_ts", "10.000000".to_string())
            ]
        );
        assert!(!post(None)
            .request()
            .1
            .iter()
            .any(|(k, _)| *k == "thread_ts"));
    }

    #[test]
    fn the_last_six_messages_are_kept_oldest_first() {
        let history: Vec<Value> = (0..9)
            .rev()
            .map(|i| serde_json::json!({ "user": "U1", "text": format!("m{i}"), "ts": format!("{i}.000000") }))
            .collect();
        let said = messages(&serde_json::json!({ "ok": true, "messages": history }));
        let texts: Vec<&str> = said.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["m3", "m4", "m5", "m6", "m7", "m8"]);
    }
}
