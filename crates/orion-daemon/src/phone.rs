//! PHONE: while you are away, a session that starts waiting on you — a
//! question, a plan, a permission prompt — or crashes goes to your phone
//! at once, as a Slack DM from Orion's bot; the answer comes back the same
//! way.
//!
//! - **When**: a `StatusChanged` into `needs_feedback` or `terminated`
//!   while you are away (`away`), outside quiet hours, of a kind
//!   `away.phone` lists. Never delayed.
//! - **The message** (design 5.4): a context line `● Agent · web · 10:42`,
//!   the session's name, its whole last response (never summarised, in
//!   3000-character sections), then what it waits on and its buttons:
//!   - one single-select question: a button per option, `Type an
//!     answer…`, and `or reply in this thread` under them;
//!   - a multi-select question, or several in one ask: every question in
//!     bold with its options listed dim under it, and one `Answer…`;
//!   - a plan: `Approve` and `Suggest changes…`;
//!   - a permission prompt: `Yes` and `Type an answer…`.
//! - **Modals**: `Type an answer…`, `Answer…` and `Suggest changes…` open
//!   a Slack modal (`views.open`, with the press's `trigger_id`) — a text
//!   box; or, for `Answer…`, radio buttons or checkboxes per question,
//!   each with an optional "Or in your own words" box that wins over the
//!   picks. Its submit answers every question at once
//!   (`Answer::Answers`). A submit on a message already answered is
//!   refused inside the modal (`response_action: errors`).
//! - **Replies**: a reply in a message's thread answers that session in
//!   words (a steer, for a plan). Only the installing user's replies
//!   count. Words on a several-question ask cancel its dialog and go in
//!   as the next turn, the questions quoted (`answer::quoted_turn`).
//! - **A plain DM** answers nothing: it posts every message still waiting
//!   again, at the foot of the DM where you are looking — each old copy
//!   updated to `↓ Re-sent below`, its buttons gone, and a fresh one
//!   posted about what its session waits on now, oldest first. With
//!   nothing waiting it says so.
//! - **After an answer**: the message is updated in place, its buttons
//!   swapped for `✓ Sent: <what went in> · working again`. One that did
//!   not go in gets `✗ Not sent: …` in its thread. One typed into a
//!   dialog that still waits on the same thing [`ANSWER_CHECK`] later —
//!   red, over the same reason, its transcript unchanged — didn't take:
//!   the message gets its buttons back and [`DIDNT_TAKE`], and waits for
//!   an answer again.
//! - **A newer dialog**: a session still red that opens another (a
//!   steered plan going on to ask a question) says red again with the new
//!   reason (`Daemon::apply_hook_event_with`). Its open message about the
//!   old one is updated to [`MOVED_ON`], its buttons gone, and a new one
//!   is posted below. A message already up about the same dialog is never
//!   posted twice.
//! - **Answered at the desk**: a session that leaves the state its message
//!   was about any other way has the message updated to say "Answered in
//!   Orion", its buttons gone.
//!
//! Which message is which session's — and the `WaitReason` it was posted
//! about, so a modal can be built after a restart — lives in the store
//! (`phone_messages`).

use std::sync::Arc;

use orion_core::away::PhoneKind;
use orion_core::{AgentId, AgentStatus, Answer, Asked, Picked, ServerEvent, WaitReason};
use serde_json::{json, Value};

use crate::answer::picked_words;
use crate::registry::Daemon;
use crate::slack::Session;

/// Slack's cap on one section block's text.
pub const SECTION_CHARS: usize = 3000;
/// Slack's cap on blocks in one message.
const MAX_BLOCKS: usize = 50;
/// Slack's cap on a button's label.
const BUTTON_CHARS: usize = 75;
/// Slack's cap on a header block's text.
const HEADER_CHARS: usize = 150;
/// Slack's cap on a modal's title.
const TITLE_CHARS: usize = 24;
/// Slack's cap on an input block's label.
const LABEL_CHARS: usize = 2000;
/// Slack's cap on a radio button's or checkbox's text.
const OPTION_CHARS: usize = 75;
/// How much of an answer the updated message repeats.
const SUMMARY_CHARS: usize = 300;

/// What an answered message ends on, after what went in.
pub const WORKING_AGAIN: &str = "working again";
/// What an answered message gains.
pub const ANSWERED_IN_ORION: &str = "Answered in Orion";
/// What a message gains when its session opens a newer dialog.
pub const MOVED_ON: &str = "Moved on: a newer question is below";
/// What a message gains when a plain DM posts it again.
pub const RESENT_BELOW: &str = "Re-sent below";
/// What a message answered from the phone gains when the session still
/// waits on the same thing after [`ANSWER_CHECK`].
pub const DIDNT_TAKE: &str =
    "⚠ That didn't take: it's still waiting on this. Try again, or answer at your desk.";
/// What a plain DM gets with no message waiting.
pub const NOTHING_WAITING: &str = "Nothing is waiting for you.";
/// What a modal submitted on an answered message shows.
pub const ALREADY_ANSWERED: &str = "This one was already answered";
/// What a modal question with neither a pick nor words shows.
const NOTHING_PICKED: &str = "Pick one, or write your own below";
/// The line under a question's buttons.
const REPLY_HERE: &str = "or reply in this thread";

/// How long after an answer goes into a dialog the session is looked at
/// again, to see that it took ([`DIDNT_TAKE`]); `ORION_PHONE_ANSWER_CHECK_MS`
/// overrides it.
pub const ANSWER_CHECK: std::time::Duration = std::time::Duration::from_secs(20);
/// What a session no automation started is called on its context line;
/// one an automation started goes by the automation's name.
const AGENT_KIND: &str = "Agent";

/// How long a reply waits for its message to be recorded
/// ([`Daemon::phone_message_soon`]): this many looks, this far apart.
const RECORD_TRIES: usize = 20;
const RECORD_GAP: std::time::Duration = std::time::Duration::from_millis(100);

/// The `action_id`s on a message's buttons.
const ACTION_OPTION: &str = "orion_option";
const ACTION_APPROVE: &str = "orion_approve";
/// `Answer…`: the modal with every question.
pub const ACTION_ANSWER_MODAL: &str = "orion_answer_modal";
/// `Type an answer…`: the modal with one text box.
pub const ACTION_TYPE: &str = "orion_type";
/// `Suggest changes…`: the plan's steer modal.
pub const ACTION_STEER: &str = "orion_steer";

/// The `block_id`s: the `or reply in this thread` line, and in a modal each
/// question's picks (`orion_q_<n>`) and words (`orion_t_<n>`), and the
/// steer's box.
const BLOCK_REPLY_HERE: &str = "orion_reply_here";
const BLOCK_PICK: &str = "orion_q_";
const BLOCK_WORDS: &str = "orion_t_";
const BLOCK_STEER: &str = "orion_steer";
/// The `action_id`s inside a modal's input blocks.
const INPUT_PICK: &str = "pick";
const INPUT_TEXT: &str = "text";

/// One phone message, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneMessage {
    pub channel: String,
    pub ts: String,
    pub agent: AgentId,
    pub kind: PhoneKind,
    /// The blocks it was posted with (JSON), to rebuild it as answered.
    pub blocks: String,
    /// What it was about, to build its modals.
    pub reason: Option<WaitReason>,
}

/// Which kind of phone message a status change is, if any.
pub fn kind_of(status: AgentStatus, reason: Option<&WaitReason>) -> Option<PhoneKind> {
    match status {
        AgentStatus::NeedsFeedback => Some(match reason {
            Some(WaitReason::Plan { .. }) => PhoneKind::Plan,
            _ => PhoneKind::Question,
        }),
        AgentStatus::Terminated => Some(PhoneKind::Crash),
        _ => None,
    }
}

/// The status a message of `kind` is about: once the session is in any
/// other, the message has been answered.
fn status_of(kind: PhoneKind) -> AgentStatus {
    match kind {
        PhoneKind::Question | PhoneKind::Plan => AgentStatus::NeedsFeedback,
        PhoneKind::Crash => AgentStatus::Terminated,
    }
}

/// What one phone message says.
#[derive(Debug, Clone)]
pub struct Ask<'a> {
    pub project: &'a str,
    /// Local `HH:MM`.
    pub time: &'a str,
    pub session: &'a str,
    /// The AUTOMATION that started the session (`Agent::automation`).
    pub automation: Option<&'a str>,
    /// The session's last response, whole.
    pub said: Option<&'a str>,
    pub reason: Option<&'a WaitReason>,
    pub kind: PhoneKind,
}

/// A message ready to post: its notification line, its blocks, and any
/// text past Slack's block cap, posted in its thread after it.
#[derive(Debug, Clone)]
pub struct Built {
    pub text: String,
    pub blocks: Vec<Value>,
    pub overflow: Vec<String>,
}

/// What a message closes on: the blocks under the response, its buttons,
/// whether `or reply in this thread` goes under them, and its notification line.
struct Closing {
    blocks: Vec<Value>,
    buttons: Option<Vec<Value>>,
    reply_here: bool,
    text: String,
}

/// The 5.4 layout for `ask`.
pub fn build(ask: &Ask) -> Built {
    let mut head = vec![
        json!({"type": "context", "elements": [{"type": "mrkdwn",
            "text": format!("● {} · {} · {}",
                ask.automation.and_then(orion_core::automation::name).unwrap_or(AGENT_KIND),
                escape(ask.project), ask.time)}]}),
        json!({"type": "header", "text": {"type": "plain_text", "emoji": true,
            "text": clip(ask.session, HEADER_CHARS)}}),
    ];
    let mut body: Vec<String> = ask
        .said
        .map(|said| split(&mrkdwn(said)))
        .unwrap_or_default();
    let type_an_answer = || button("Type an answer…", ACTION_TYPE, "type");
    let closing = match ask.reason {
        // One tap where one tap will do.
        Some(WaitReason::Question { questions }) if is_one_tap(questions) => {
            let asked = &questions[0];
            let mut buttons: Vec<Value> = asked
                .options
                .iter()
                .enumerate()
                .map(|(i, label)| button(label, &format!("{ACTION_OPTION}_{i}"), &i.to_string()))
                .collect();
            buttons.push(type_an_answer());
            Closing {
                blocks: vec![section(format!("*{}*", escape(&asked.question)))],
                buttons: Some(buttons),
                reply_here: true,
                text: format!("{}: {}", ask.session, asked.question),
            }
        }
        // Every question readable without opening anything; one modal
        // answers them all.
        Some(WaitReason::Question { questions }) if !questions.is_empty() => {
            let mut blocks = Vec::new();
            for asked in questions {
                let any = if asked.multi { " _(pick any)_" } else { "" };
                blocks.push(section(format!("*{}*{any}", escape(&asked.question))));
                if !asked.options.is_empty() {
                    let list: Vec<String> = asked
                        .options
                        .iter()
                        .map(|o| format!("• {}", escape(o)))
                        .collect();
                    blocks.push(context(&list.join("\n")));
                }
            }
            let mut answer = button("Answer…", ACTION_ANSWER_MODAL, "answer");
            answer["style"] = "primary".into();
            Closing {
                blocks,
                buttons: Some(vec![answer]),
                reply_here: true,
                text: match questions.len() {
                    1 => format!("{}: {}", ask.session, questions[0].question),
                    n => format!("{}: {n} questions", ask.session),
                },
            }
        }
        Some(WaitReason::Plan { plan }) => {
            body.extend(split(&mrkdwn(plan)));
            let mut approve = button("Approve", ACTION_APPROVE, "approve");
            approve["style"] = "primary".into();
            Closing {
                blocks: vec![section("*Approve this plan?*".into())],
                buttons: Some(vec![
                    approve,
                    button("Suggest changes…", ACTION_STEER, "steer"),
                ]),
                reply_here: false,
                text: format!("{}: a plan waits for your approval", ask.session),
            }
        }
        Some(WaitReason::Permission { tool }) => Closing {
            blocks: vec![section(format!("*Allow {}?*", escape(tool)))],
            buttons: Some(vec![
                button("Yes", &format!("{ACTION_OPTION}_0"), "0"),
                type_an_answer(),
            ]),
            reply_here: true,
            text: format!("{}: allow {tool}?", ask.session),
        },
        Some(WaitReason::Crash { exit_code }) => Closing {
            blocks: vec![section(match exit_code {
                Some(code) => format!("*Crashed* (exit {code})"),
                None => "*Crashed*".to_string(),
            })],
            buttons: None,
            reply_here: false,
            text: format!("{} crashed", ask.session),
        },
        None if ask.kind == PhoneKind::Crash => Closing {
            blocks: vec![section("*Crashed*".into())],
            buttons: None,
            reply_here: false,
            text: format!("{} crashed", ask.session),
        },
        Some(WaitReason::Question { .. }) | None => Closing {
            blocks: vec![section("*Waiting for you*".into())],
            buttons: Some(vec![type_an_answer()]),
            reply_here: true,
            text: format!("{} is waiting for you", ask.session),
        },
    };
    let tail = closing.blocks.len()
        + usize::from(closing.buttons.is_some())
        + usize::from(closing.reply_here);
    let room = MAX_BLOCKS - head.len() - tail;
    let overflow = match body.len() > room {
        true => body.split_off(room),
        false => Vec::new(),
    };
    head.extend(body.into_iter().map(section));
    head.extend(closing.blocks);
    if let Some(buttons) = closing.buttons {
        head.push(json!({"type": "actions", "block_id": "orion_answer", "elements": buttons}));
    }
    if closing.reply_here {
        let mut line = context(REPLY_HERE);
        line["block_id"] = BLOCK_REPLY_HERE.into();
        head.push(line);
    }
    Built {
        text: closing.text,
        blocks: head,
        overflow,
    }
}

/// A lone single-select question: a button per option answers it.
fn is_one_tap(questions: &[Asked]) -> bool {
    matches!(questions, [only] if !only.multi && !only.options.is_empty())
}

/// `blocks` as answered at the desk: its buttons gone, "Answered in
/// Orion" at the foot.
pub fn answered(blocks: &[Value]) -> Vec<Value> {
    closed_as(blocks, Close::AnsweredInOrion)
}

/// `blocks` with its buttons gone and why at the foot: answered at the
/// desk, moved on, or re-sent.
fn closed_as(blocks: &[Value], close: Close) -> Vec<Value> {
    closed(blocks, close.foot())
}

/// `blocks` as posted — its buttons back — with [`DIDNT_TAKE`] above them.
pub fn reopened(blocks: &[Value]) -> Vec<Value> {
    let mut out = blocks.to_vec();
    let at = out
        .iter()
        .position(|b| b["type"] == "actions" || b["block_id"] == BLOCK_REPLY_HERE)
        .unwrap_or(out.len());
    out.insert(at, section(format!("*{}*", escape(DIDNT_TAKE))));
    out
}

/// Why a message closes without the phone's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Close {
    /// Its session moved on at the desk.
    AnsweredInOrion,
    /// Its session opened a newer dialog, posted below.
    MovedOn,
    /// A plain DM posted it again below.
    Resent,
}

impl Close {
    /// The line its message ends on.
    fn foot(self) -> String {
        match self {
            Close::AnsweredInOrion => format!("✓ {ANSWERED_IN_ORION}"),
            Close::MovedOn => format!("↓ {MOVED_ON}"),
            Close::Resent => format!("↓ {RESENT_BELOW}"),
        }
    }

    /// Its updated message's notification line.
    fn text(self) -> &'static str {
        match self {
            Close::AnsweredInOrion => ANSWERED_IN_ORION,
            Close::MovedOn => MOVED_ON,
            Close::Resent => RESENT_BELOW,
        }
    }

    /// What the store marks it answered by.
    fn by(self) -> &'static str {
        match self {
            Close::AnsweredInOrion => "orion",
            Close::MovedOn => "moved",
            Close::Resent => "resent",
        }
    }
}

/// `blocks` as answered from the phone with `summary`: its buttons gone,
/// what went in at the foot.
pub fn answered_with(blocks: &[Value], summary: &str) -> Vec<Value> {
    closed(blocks, escape(&sent_line(summary)))
}

/// The line an answered message ends on.
pub fn sent_line(summary: &str) -> String {
    format!("✓ Sent: {summary} · {WORKING_AGAIN}")
}

fn closed(blocks: &[Value], foot: String) -> Vec<Value> {
    let mut out: Vec<Value> = blocks
        .iter()
        .filter(|b| b["type"] != "actions" && b["block_id"] != BLOCK_REPLY_HERE)
        .cloned()
        .collect();
    out.push(context(&foot));
    out
}

/// What went in, in a few words: the options picked, your words, or the
/// approval. Several questions' answers are joined by `; `.
pub fn summary(reason: Option<&WaitReason>, answer: &Answer) -> String {
    let questions = match reason {
        Some(WaitReason::Question { questions }) => questions.as_slice(),
        _ => &[],
    };
    let text = match (answer, reason) {
        (Answer::ApprovePlan, _) | (Answer::Option(0), Some(WaitReason::Plan { .. })) => {
            "Approved".to_string()
        }
        (Answer::Option(0), Some(WaitReason::Permission { .. })) => "Yes".to_string(),
        (Answer::Option(i), _) => picked_words(questions.first(), &Picked::Options(vec![*i])),
        (Answer::Text(text) | Answer::Steer(text), _) => text.clone(),
        (Answer::Answers(picks), _) => picks
            .iter()
            .enumerate()
            .map(|(n, picked)| picked_words(questions.get(n), picked))
            .collect::<Vec<_>>()
            .join("; "),
    };
    clip(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        SUMMARY_CHARS,
    )
}

/// The modal `action` opens on `message`: one text box (`Type an
/// answer…`, `Suggest changes…`), or a block per question (`Answer…`) —
/// radio buttons for a single select, checkboxes for a multi-select, and
/// under each an optional box for your own words. Its `private_metadata`
/// is `<channel>:<ts>`, so the submit finds its message.
pub fn modal_for(message: &PhoneMessage, reason: Option<&WaitReason>, action: &str) -> Value {
    let questions = match reason {
        Some(WaitReason::Question { questions }) if !questions.is_empty() => Some(questions),
        _ => None,
    };
    let (callback, title, blocks) = match (action, questions) {
        (ACTION_STEER, _) => (
            ACTION_STEER,
            "Suggest changes",
            vec![input(
                BLOCK_STEER,
                "What should change in the plan?",
                text_box(true),
                false,
            )],
        ),
        (ACTION_ANSWER_MODAL, Some(questions)) => (
            ACTION_ANSWER_MODAL,
            "Answer",
            questions
                .iter()
                .enumerate()
                .flat_map(|(n, asked)| question_blocks(n, asked))
                .collect(),
        ),
        _ => {
            let label = match (questions, reason) {
                (Some(questions), _) => questions[0].question.clone(),
                (None, Some(WaitReason::Permission { tool })) => {
                    format!("Instead of allowing {tool}, Claude should…")
                }
                _ => "Your answer".to_string(),
            };
            (
                ACTION_TYPE,
                "Type an answer",
                vec![input(
                    &format!("{BLOCK_WORDS}0"),
                    &label,
                    text_box(true),
                    false,
                )],
            )
        }
    };
    json!({
        "type": "modal",
        "callback_id": callback,
        "private_metadata": format!("{}:{}", message.channel, message.ts),
        "title": plain(&clip(title, TITLE_CHARS)),
        "submit": plain("Send"),
        "close": plain("Cancel"),
        "blocks": blocks,
    })
}

/// Question `n`'s blocks in the `Answer…` modal.
fn question_blocks(n: usize, asked: &Asked) -> Vec<Value> {
    let words = format!("{BLOCK_WORDS}{n}");
    if asked.options.is_empty() {
        return vec![input(&words, &asked.question, text_box(false), false)];
    }
    let options: Vec<Value> = asked
        .options
        .iter()
        .enumerate()
        .map(
            |(i, label)| json!({"text": plain(&clip(label, OPTION_CHARS)), "value": i.to_string()}),
        )
        .collect();
    let picker = json!({
        "type": if asked.multi { "checkboxes" } else { "radio_buttons" },
        "action_id": INPUT_PICK,
        "options": options,
    });
    vec![
        input(&format!("{BLOCK_PICK}{n}"), &asked.question, picker, true),
        input(&words, "Or in your own words", text_box(false), true),
    ]
}

fn input(block_id: &str, label: &str, element: Value, optional: bool) -> Value {
    json!({"type": "input", "block_id": block_id, "optional": optional,
        "label": plain(&clip(label, LABEL_CHARS)), "element": element})
}

fn text_box(multiline: bool) -> Value {
    json!({"type": "plain_text_input", "action_id": INPUT_TEXT, "multiline": multiline})
}

fn plain(text: &str) -> Value {
    json!({"type": "plain_text", "emoji": true, "text": text})
}

fn section(text: String) -> Value {
    json!({"type": "section", "text": {"type": "mrkdwn", "text": text}})
}

fn context(text: &str) -> Value {
    json!({"type": "context", "elements": [{"type": "mrkdwn", "text": text}]})
}

fn button(label: &str, action_id: &str, value: &str) -> Value {
    json!({"type": "button", "action_id": action_id, "value": value,
        "text": {"type": "plain_text", "emoji": true, "text": clip(label, BUTTON_CHARS)}})
}

/// `text` cut to `max` characters, an ellipsis ending one that was cut.
fn clip(text: &str, max: usize) -> String {
    match text.chars().count() > max {
        true => format!("{}…", text.chars().take(max - 1).collect::<String>()),
        false => text.to_string(),
    }
}

/// Slack's three escapes for text it reads as mrkdwn.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Claude's Markdown as Slack's mrkdwn, near enough: escaped, `**bold**`
/// as `*bold*`, and a `#` heading as a bold line. Code blocks and lists
/// read the same in both.
pub fn mrkdwn(text: &str) -> String {
    escape(text)
        .lines()
        .map(|line| {
            let trimmed = line.trim_start_matches('#');
            let line = match trimmed.len() < line.len() && trimmed.starts_with(' ') {
                true => format!("**{}**", trimmed.trim()),
                false => line.to_string(),
            };
            line.replace("**", "*")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` in pieces of at most [`SECTION_CHARS`] characters, each cut at
/// the last line break (else space) that fits, never inside an `&…;`
/// escape. Nothing is dropped but the whitespace a cut lands on.
pub fn split(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let chars: Vec<(usize, char)> = rest.char_indices().collect();
        if chars.len() <= SECTION_CHARS {
            out.push(rest.to_string());
            break;
        }
        let limit = chars[SECTION_CHARS].0;
        let window = &rest[..limit];
        let mut cut = window
            .rfind('\n')
            .or_else(|| window.rfind(' '))
            .filter(|at| *at > 0)
            .unwrap_or(limit);
        // A hard cut never splits an escape.
        if let Some(amp) = window[..cut].rfind('&') {
            if !window[amp..cut].contains(';') && cut - amp < 6 {
                cut = amp;
            }
        }
        out.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start();
    }
    out
}

/// What a reply on the phone is to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The phone message with this `ts`.
    Message(String),
    /// No message: a plain DM, which posts every open one again.
    Dm,
}

/// What a reply on the phone says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// An option's button (0-based).
    Pick(usize),
    Approve,
    /// Words: typed in the thread, the DM, or a modal's one text box.
    Words(String),
    /// The `Answer…` modal's submit: one per question.
    Answers(Vec<Picked>),
    /// A button that opens a modal: its `action_id` and the press's
    /// `trigger_id`.
    Modal {
        action: String,
        trigger: String,
    },
}

/// One reply, routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub target: Target,
    pub reply: Reply,
}

/// A button press (`block_actions`) on a phone message by the installing
/// user, routed. Anyone else's press, or one on another message, is None.
pub fn route_action(payload: &Value, session: &Session) -> Option<Route> {
    route_action_for(payload, &session.me, &session.dm)
}

fn route_action_for(payload: &Value, me: &str, dm: &str) -> Option<Route> {
    if payload["type"] != "block_actions" || payload["user"]["id"] != me {
        return None;
    }
    let container = &payload["container"];
    let channel = container["channel_id"]
        .as_str()
        .or_else(|| payload["channel"]["id"].as_str())?;
    if channel != dm {
        return None;
    }
    let ts = container["message_ts"]
        .as_str()
        .or_else(|| payload["message"]["ts"].as_str())?;
    let action = payload["actions"].get(0)?;
    let id = action["action_id"].as_str()?;
    let reply = match id {
        ACTION_APPROVE => Reply::Approve,
        ACTION_ANSWER_MODAL | ACTION_TYPE | ACTION_STEER => Reply::Modal {
            action: id.to_string(),
            trigger: payload["trigger_id"].as_str()?.to_string(),
        },
        _ => Reply::Pick(
            id.strip_prefix(ACTION_OPTION)?
                .strip_prefix('_')?
                .parse()
                .ok()?,
        ),
    };
    Some(Route {
        target: Target::Message(ts.to_string()),
        reply,
    })
}

/// A modal's submit (`view_submission`) by the installing user, on one of
/// the phone's modals, routed: its text box as words, or the `Answer…`
/// modal's picks — a question's own words winning over its picks. A
/// question with neither is `Picked::Options(vec![])`, which
/// [`submission_errors`] refuses.
pub fn route_submission(payload: &Value, session: &Session) -> Option<Route> {
    route_submission_for(payload, &session.me, &session.dm)
}

fn route_submission_for(payload: &Value, me: &str, dm: &str) -> Option<Route> {
    if payload["type"] != "view_submission" || payload["user"]["id"] != me {
        return None;
    }
    let view = &payload["view"];
    let (channel, ts) = view["private_metadata"].as_str()?.split_once(':')?;
    if channel != dm {
        return None;
    }
    let values = &view["state"]["values"];
    let words = |block: &str| {
        values[block][INPUT_TEXT]["value"]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    let reply = match view["callback_id"].as_str()? {
        ACTION_STEER => Reply::Words(words(BLOCK_STEER)?),
        ACTION_TYPE => Reply::Words(words(&format!("{BLOCK_WORDS}0"))?),
        ACTION_ANSWER_MODAL => {
            let count = view["blocks"]
                .as_array()?
                .iter()
                .filter_map(|b| b["block_id"].as_str())
                .filter(|id| id.starts_with(BLOCK_WORDS))
                .count();
            Reply::Answers(
                (0..count)
                    .map(|n| match words(&format!("{BLOCK_WORDS}{n}")) {
                        Some(text) => Picked::Text(text),
                        None => {
                            Picked::Options(picked(&values[format!("{BLOCK_PICK}{n}")][INPUT_PICK]))
                        }
                    })
                    .collect(),
            )
        }
        _ => return None,
    };
    Some(Route {
        target: Target::Message(ts.to_string()),
        reply,
    })
}

/// The options a radio group (`selected_option`) or checkboxes
/// (`selected_options`) have picked, in order.
fn picked(state: &Value) -> Vec<usize> {
    let value = |o: &Value| o["value"].as_str().and_then(|v| v.parse().ok());
    let mut picks: Vec<usize> = match state["selected_options"].as_array() {
        Some(options) => options.iter().filter_map(value).collect(),
        None => value(&state["selected_option"]).into_iter().collect(),
    };
    picks.sort_unstable();
    picks.dedup();
    picks
}

/// The acknowledgement payload a modal's submit needs before it closes:
/// None to close it, else the errors it shows — "This one was already
/// answered" on a message that no longer waits (`open` false), or a
/// question left with neither a pick nor words.
pub fn submission_errors(view: &Value, reply: &Reply, open: bool) -> Option<Value> {
    let error = |block: &str, text: &str| {
        let mut errors = serde_json::Map::new();
        errors.insert(block.to_string(), text.into());
        Some(json!({"response_action": "errors", "errors": errors}))
    };
    if !open {
        let first = view["blocks"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["type"] == "input"))
            .and_then(|b| b["block_id"].as_str())
            .unwrap_or(BLOCK_STEER);
        return error(first, ALREADY_ANSWERED);
    }
    if let Reply::Answers(picks) = reply {
        let empty = picks
            .iter()
            .position(|p| matches!(p, Picked::Options(o) if o.is_empty()))?;
        return error(&format!("{BLOCK_PICK}{empty}"), NOTHING_PICKED);
    }
    None
}

/// A message event in the phone's DM from the installing user, routed: a
/// thread reply to its message, a plain message to the DM itself.
/// Anything else — another conversation, someone else, the bot itself, an
/// edit — is None.
pub fn route_message(event: &Value, session: &Session) -> Option<Route> {
    route_message_for(event, &session.me, &session.dm)
}

fn route_message_for(event: &Value, me: &str, dm: &str) -> Option<Route> {
    if event["channel"] != dm || event["user"] != me || !event["bot_id"].is_null() {
        return None;
    }
    // Edits, deletions and joins are not replies; a thread reply also
    // sent to the conversation is.
    if !matches!(event["subtype"].as_str(), None | Some("thread_broadcast")) {
        return None;
    }
    let text = event["text"].as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    let ts = event["ts"].as_str();
    let target = match event["thread_ts"].as_str() {
        Some(thread) if Some(thread) != ts => Target::Message(thread.to_string()),
        _ => Target::Dm,
    };
    Some(Route {
        target,
        reply: Reply::Words(text.to_string()),
    })
}

/// The answer a reply makes to a message of `kind`.
fn answer_for(kind: PhoneKind, reply: Reply) -> Option<Answer> {
    match reply {
        Reply::Pick(i) => Some(Answer::Option(i)),
        Reply::Approve => Some(Answer::ApprovePlan),
        Reply::Words(text) if kind == PhoneKind::Plan => Some(Answer::Steer(text)),
        Reply::Words(text) => Some(Answer::Text(text)),
        Reply::Answers(picks) => Some(Answer::Answers(picks)),
        Reply::Modal { .. } => None,
    }
}

/// Listen for status changes for the daemon's life, posting and closing
/// phone messages as they come.
pub async fn watch(daemon: Arc<Daemon>) {
    let mut rx = daemon.events.subscribe();
    loop {
        let event = tokio::select! {
            _ = daemon.shutdown.cancelled() => return,
            event = rx.recv() => event,
        };
        match event {
            Ok(ServerEvent::StatusChanged {
                agent,
                status,
                reason,
                ..
            }) => daemon.phone_on_status(&agent, status, reason).await,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "phone bridge fell behind the status events");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Run `call` against Slack off the async workers, logging a failure.
async fn slack<T: Send + 'static>(
    what: &'static str,
    call: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Option<T> {
    match tokio::task::spawn_blocking(call).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "slack {what} failed");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "slack {what} panicked");
            None
        }
    }
}

/// Whether a status change into `status` over `reason` closes open
/// `message`, and why: its session left the state it was about (answered
/// at the desk), or is red over a different dialog now — moved on, when
/// a message about that one is going out (`sends`). None leaves it up:
/// it is about this very wait. A red with no reason known (the daemon
/// restarted) is taken to be the same wait.
fn closes(
    message: &PhoneMessage,
    status: AgentStatus,
    reason: Option<&WaitReason>,
    sends: bool,
) -> Option<Close> {
    if status_of(message.kind) != status {
        return Some(Close::AnsweredInOrion);
    }
    let newer = status == AgentStatus::NeedsFeedback
        && reason.is_some()
        && message.reason.as_ref() != reason;
    match (newer, sends) {
        (false, _) => None,
        (true, true) => Some(Close::MovedOn),
        (true, false) => Some(Close::AnsweredInOrion),
    }
}

/// How long after a phone answer goes into a dialog it is checked
/// ([`ANSWER_CHECK`], or `ORION_PHONE_ANSWER_CHECK_MS`).
fn answer_check() -> std::time::Duration {
    orion_core::env::non_empty(orion_core::env::PHONE_ANSWER_CHECK_MS)
        .and_then(|ms| ms.parse().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or(ANSWER_CHECK)
}

/// What a session waiting on a dialog looked like as a phone answer went
/// in: still so after [`ANSWER_CHECK`], the answer didn't take.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Waiting {
    changed_at: i64,
    reason: Option<WaitReason>,
    /// Its transcript's length: an answer that took writes the dialog's
    /// result there, even when no hook turns the session yellow.
    transcript: Option<u64>,
}

impl Daemon {
    /// A status change: close the agent's open messages it answers — or,
    /// still red over a newer dialog, the ones about the old one — then
    /// post a new one when it is one to send and none is up about it yet.
    async fn phone_on_status(
        self: &Arc<Self>,
        agent: &AgentId,
        status: AgentStatus,
        reason: Option<WaitReason>,
    ) {
        let Some(session) = self.slack.session() else {
            return;
        };
        let kind = kind_of(status, reason.as_ref());
        let sends = kind.is_some_and(|kind| self.phone_sends(kind));
        let mut already_up = false;
        for message in self.store.open_phone_messages(agent).unwrap_or_default() {
            match closes(&message, status, reason.as_ref(), sends) {
                Some(close) => self.close_message(&session, message, close).await,
                None => already_up = true,
            }
        }
        if let (Some(kind), true, false) = (kind, sends, already_up) {
            self.phone_post(&session, agent, kind, reason).await;
        }
    }

    /// Whether a message of `kind` goes to the phone now: you are away,
    /// outside quiet hours, and `away.phone` lists it.
    fn phone_sends(&self, kind: PhoneKind) -> bool {
        let settings = crate::config::Config::load().away;
        let minute = {
            use chrono::Timelike;
            let now = chrono::Local::now();
            now.hour() * 60 + now.minute()
        };
        self.away.is_away() && !settings.is_quiet(minute) && settings.sends(kind)
    }

    /// Post `agent`'s phone message, at once.
    async fn phone_post(
        self: &Arc<Self>,
        session: &Arc<Session>,
        agent: &AgentId,
        kind: PhoneKind,
        reason: Option<WaitReason>,
    ) {
        let Ok(Some(row)) = self.store.get_agent(agent) else {
            return;
        };
        let project = self
            .store
            .get_worktree(&row.worktree_id)
            .ok()
            .flatten()
            .and_then(|w| self.store.get_project(&w.project_id).ok().flatten())
            .map(|p| p.name)
            .unwrap_or_default();
        let said = {
            let daemon = self.clone();
            let id = agent.clone();
            tokio::task::spawn_blocking(move || daemon.last_response(&id).0)
                .await
                .ok()
                .flatten()
        };
        let time = chrono::Local::now().format("%H:%M").to_string();
        let built = build(&Ask {
            project: &project,
            time: &time,
            session: &row.name,
            automation: row.automation.as_deref(),
            said: said.as_deref(),
            reason: reason.as_ref(),
            kind,
        });
        let blocks = Value::from(built.blocks.clone()).to_string();
        let (api, dm) = (session.api.clone(), session.dm.clone());
        let Some(ts) = slack("chat.postMessage", move || {
            let ts = api.post(&dm, &built.text, Some(&built.blocks), None)?;
            for more in &built.overflow {
                api.post(&dm, more, None, Some(&ts))?;
            }
            Ok(ts)
        })
        .await
        else {
            return;
        };
        tracing::info!(agent = %agent, ?kind, %ts, "sent to the phone");
        let message = PhoneMessage {
            channel: session.dm.clone(),
            ts,
            agent: agent.clone(),
            kind,
            blocks,
            reason,
        };
        if let Err(e) = self.store.insert_phone_message(&message) {
            tracing::warn!(error = %e, "phone message not recorded");
        }
    }

    /// Close `message` without the phone's answer, saying why on it.
    async fn close_message(&self, session: &Arc<Session>, message: PhoneMessage, close: Close) {
        if let Err(e) =
            self.store
                .set_phone_answered(&message.channel, &message.ts, Some(close.by()))
        {
            tracing::warn!(error = %e, "phone message not closed");
        }
        let blocks: Vec<Value> = serde_json::from_str(&message.blocks).unwrap_or_default();
        let blocks = closed_as(&blocks, close);
        let api = session.api.clone();
        slack("chat.update", move || {
            api.update(&message.channel, &message.ts, close.text(), &blocks)
        })
        .await;
    }

    /// A plain DM: every message still waiting, posted again at the foot
    /// of the DM, oldest first — each old copy closed as re-sent, and the
    /// new one about what its session waits on now. A message whose
    /// session no longer waits closes as answered in Orion; one session's
    /// second open message closes without a copy.
    async fn phone_resend(self: &Arc<Self>, session: &Arc<Session>) -> usize {
        let open = self
            .store
            .open_phone_messages_in(&session.dm)
            .unwrap_or_default();
        let mut posted: Vec<AgentId> = Vec::new();
        for message in open {
            let agent = message.agent.clone();
            let status = match self.store.get_agent(&agent) {
                Ok(Some(row)) => row.status,
                _ => continue,
            };
            if status != status_of(message.kind) {
                self.close_message(session, message, Close::AnsweredInOrion)
                    .await;
                continue;
            }
            let fallback = message.reason.clone();
            self.close_message(session, message, Close::Resent).await;
            if posted.contains(&agent) {
                continue;
            }
            let reason = match status {
                AgentStatus::NeedsFeedback => {
                    let daemon = self.clone();
                    let id = agent.clone();
                    tokio::task::spawn_blocking(move || daemon.wait_reason(&id))
                        .await
                        .ok()
                        .flatten()
                        .or(fallback)
                }
                _ => fallback,
            };
            if let Some(kind) = kind_of(status, reason.as_ref()) {
                self.phone_post(session, &agent, kind, reason).await;
                posted.push(agent);
            }
        }
        posted.len()
    }

    /// How agent `id` waits on a dialog now, if it does.
    async fn waiting(self: &Arc<Self>, id: &AgentId) -> Option<Waiting> {
        let agent = self.store.get_agent(id).ok()??;
        if agent.status != AgentStatus::NeedsFeedback {
            return None;
        }
        let reason = self.wait_reasons.lock().unwrap().get(id).cloned();
        let transcript = {
            let daemon = self.clone();
            let id = id.clone();
            tokio::task::spawn_blocking(move || {
                std::fs::metadata(daemon.transcript_path(&id)?)
                    .ok()
                    .map(|m| m.len())
            })
            .await
            .ok()
            .flatten()
        };
        Some(Waiting {
            changed_at: agent.status_changed_at,
            reason,
            transcript,
        })
    }

    /// [`ANSWER_CHECK`] after `message` was answered from the phone: a
    /// session still waiting just as it was (`before`) didn't take the
    /// answer, so the message gets its buttons back, says so, and waits
    /// for an answer again.
    fn check_answer_took(
        self: &Arc<Self>,
        session: &Arc<Session>,
        message: PhoneMessage,
        before: Waiting,
    ) {
        let (daemon, session) = (self.clone(), session.clone());
        tokio::spawn(async move {
            tokio::select! {
                _ = daemon.shutdown.cancelled() => return,
                _ = tokio::time::sleep(answer_check()) => {}
            }
            if daemon.waiting(&message.agent).await.as_ref() != Some(&before) {
                return;
            }
            // Closed some other way meanwhile, or already open again.
            match daemon.store.phone_message(&message.channel, &message.ts) {
                Ok(Some((_, false))) => {}
                _ => return,
            }
            tracing::warn!(agent = %message.agent, ts = %message.ts, "a phone answer didn't take");
            let _ = daemon
                .store
                .set_phone_answered(&message.channel, &message.ts, None);
            let blocks: Vec<Value> = serde_json::from_str(&message.blocks).unwrap_or_default();
            let blocks = reopened(&blocks);
            let api = session.api.clone();
            slack("chat.update", move || {
                api.update(&message.channel, &message.ts, DIDNT_TAKE, &blocks)
            })
            .await;
        });
    }

    /// What a modal's submit acks with (see [`submission_errors`]), read
    /// before the ack goes back: None for one that isn't the phone's,
    /// which then closes as any submit does.
    pub(crate) fn phone_submission_ack(&self, session: &Session, payload: &Value) -> Option<Value> {
        let route = route_submission(payload, session)?;
        let Target::Message(ts) = &route.target else {
            return None;
        };
        let open = matches!(
            self.store.phone_message(&session.dm, ts),
            Ok(Some((_, true)))
        );
        submission_errors(&payload["view"], &route.reply, open)
    }

    /// The phone message `ts` in `channel`, and whether it is open. A
    /// reply can beat its own message into the store: Slack hands out the
    /// `ts` (and the phone can show the message) before `phone_post` has
    /// recorded it, so one not found yet is looked for again, briefly.
    async fn phone_message_soon(&self, channel: &str, ts: &str) -> Option<(PhoneMessage, bool)> {
        for _ in 0..RECORD_TRIES {
            if let Some(found) = self.store.phone_message(channel, ts).ok().flatten() {
                return Some(found);
            }
            tokio::time::sleep(RECORD_GAP).await;
        }
        None
    }

    /// A reply on the phone: a plain DM posts what waits again; anything
    /// on a message opens the modal a button asks for, or answers the
    /// session it is to and updates its message to say what went in.
    pub(crate) async fn phone_reply(self: &Arc<Self>, session: &Arc<Session>, route: Route) {
        let say = |text: String, thread: Option<String>| {
            let (api, dm) = (session.api.clone(), session.dm.clone());
            slack("chat.postMessage", move || {
                api.post(&dm, &text, None, thread.as_deref()).map(drop)
            })
        };
        let Target::Message(ts) = &route.target else {
            if self.phone_resend(session).await == 0 {
                say(NOTHING_WAITING.into(), None).await;
            }
            return;
        };
        let Some((message, open)) = self.phone_message_soon(&session.dm, ts).await else {
            return;
        };
        let reason = message.reason.clone().or_else(|| {
            self.wait_reasons
                .lock()
                .unwrap()
                .get(&message.agent)
                .cloned()
        });
        // The trigger lasts three seconds: nothing else waits first.
        if let Reply::Modal { action, trigger } = &route.reply {
            let view = modal_for(&message, reason.as_ref(), action);
            let (api, trigger) = (session.api.clone(), trigger.clone());
            slack("views.open", move || api.views_open(&trigger, &view)).await;
            return;
        }
        let thread = Some(message.ts.clone());
        if !open {
            say(format!("{ALREADY_ANSWERED}."), thread).await;
            return;
        }
        let Some(answer) = answer_for(message.kind, route.reply) else {
            return;
        };
        // Marked first: the status change the answer causes must not
        // read as an answer at the desk.
        let _ = self
            .store
            .set_phone_answered(&message.channel, &message.ts, Some("phone"));
        tracing::info!(agent = %message.agent, ?answer, "answered from the phone");
        let said = summary(reason.as_ref(), &answer);
        let before = self.waiting(&message.agent).await;
        match self.answer(&message.agent, answer).await {
            Ok(()) => {
                let blocks: Vec<Value> = serde_json::from_str(&message.blocks).unwrap_or_default();
                let blocks = answered_with(&blocks, &said);
                let (api, sent) = (session.api.clone(), message.clone());
                slack("chat.update", move || {
                    api.update(&sent.channel, &sent.ts, &sent_line(&said), &blocks)
                })
                .await;
                if let Some(before) = before {
                    self.check_answer_took(session, message, before);
                }
            }
            Err(e) => {
                let _ = self
                    .store
                    .set_phone_answered(&message.channel, &message.ts, None);
                say(format!("✗ Not sent: {e:#}"), thread).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(question: &str, options: &[&str], multi: bool) -> Asked {
        Asked {
            question: question.into(),
            header: String::new(),
            options: options.iter().map(|o| o.to_string()).collect(),
            multi,
            previews: false,
        }
    }

    fn question() -> WaitReason {
        WaitReason::Question {
            questions: vec![asked("Which color?", &["Red", "Green"], false)],
        }
    }

    fn several() -> WaitReason {
        WaitReason::Question {
            questions: vec![
                asked("Which color?", &["Red", "Green"], false),
                asked("Which fruits?", &["Apple", "Pear", "Mango"], true),
            ],
        }
    }

    fn ask<'a>(said: Option<&'a str>, reason: Option<&'a WaitReason>) -> Ask<'a> {
        Ask {
            project: "web",
            time: "10:42",
            session: "Fix the login redirect",
            automation: None,
            said,
            reason,
            kind: kind_of(AgentStatus::NeedsFeedback, reason).unwrap(),
        }
    }

    fn message(reason: Option<WaitReason>) -> PhoneMessage {
        PhoneMessage {
            channel: "D1".into(),
            ts: "100.1".into(),
            agent: AgentId("a1".into()),
            kind: PhoneKind::Question,
            blocks: "[]".into(),
            reason,
        }
    }

    fn texts(blocks: &[Value]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b["type"].as_str().unwrap() {
                "actions" => format!(
                    "[{}]",
                    b["elements"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|e| e["text"]["text"].as_str().unwrap())
                        .collect::<Vec<_>>()
                        .join("|")
                ),
                "context" => b["elements"][0]["text"].as_str().unwrap().to_string(),
                _ => b["text"]["text"].as_str().unwrap().to_string(),
            })
            .collect()
    }

    #[test]
    fn an_automations_session_goes_by_the_automations_name() {
        let reason = question();
        let tagged = Ask {
            automation: Some("bug_fixer"),
            ..ask(None, Some(&reason))
        };
        assert_eq!(
            texts(&build(&tagged).blocks)[0],
            "● Bug fixer · web · 10:42"
        );
    }

    #[test]
    fn a_single_question_keeps_one_tap_buttons_and_a_type_an_answer() {
        let reason = question();
        let built = build(&ask(Some("I looked at **both**.\n# Next"), Some(&reason)));
        assert_eq!(
            texts(&built.blocks),
            [
                "● Agent · web · 10:42",
                "Fix the login redirect",
                "I looked at *both*.\n*Next*",
                "*Which color?*",
                "[Red|Green|Type an answer…]",
                "or reply in this thread",
            ]
        );
        assert_eq!(built.text, "Fix the login redirect: Which color?");
        assert!(built.overflow.is_empty());
        let buttons = &built.blocks[4]["elements"];
        assert_eq!(buttons[1]["action_id"], "orion_option_1");
        assert_eq!(buttons[2]["action_id"], "orion_type");
    }

    #[test]
    fn several_questions_show_every_one_and_answer_in_one_modal() {
        let reason = several();
        let built = build(&ask(None, Some(&reason)));
        assert_eq!(
            texts(&built.blocks)[2..],
            [
                "*Which color?*",
                "• Red\n• Green",
                "*Which fruits?* _(pick any)_",
                "• Apple\n• Pear\n• Mango",
                "[Answer…]",
                "or reply in this thread",
            ]
        );
        assert_eq!(built.text, "Fix the login redirect: 2 questions");
        let answer = &built.blocks[6]["elements"][0];
        assert_eq!(
            (&answer["action_id"], &answer["style"]),
            (&json!("orion_answer_modal"), &json!("primary"))
        );
        // A lone multi-select question is the same shape.
        let multi = WaitReason::Question {
            questions: vec![asked("Which fruits?", &["Apple"], true)],
        };
        assert_eq!(
            texts(&build(&ask(None, Some(&multi))).blocks)[4],
            "[Answer…]"
        );
    }

    #[test]
    fn a_plan_shows_whole_with_approve_and_suggest_changes() {
        let plan = WaitReason::Plan {
            plan: "1. Back up\n2. Migrate".into(),
        };
        let built = build(&ask(None, Some(&plan)));
        assert_eq!(
            texts(&built.blocks)[2..],
            [
                "1. Back up\n2. Migrate",
                "*Approve this plan?*",
                "[Approve|Suggest changes…]"
            ]
        );
        assert_eq!(built.blocks[4]["elements"][0]["style"], "primary");
        assert_eq!(built.blocks[4]["elements"][1]["action_id"], "orion_steer");
    }

    #[test]
    fn a_long_response_is_split_never_cut_short() {
        let line = "word ".repeat(100);
        let said = vec![line.trim(); 40].join("\n");
        let reason = question();
        let built = build(&ask(Some(&said), Some(&reason)));
        let sections: Vec<String> = texts(&built.blocks)[2..built.blocks.len() - 3].to_vec();
        assert!(sections.len() > 1);
        assert!(sections.iter().all(|s| s.chars().count() <= SECTION_CHARS));
        assert_eq!(sections.join("\n"), said, "nothing lost");
    }

    #[test]
    fn past_slacks_block_cap_the_rest_goes_in_the_thread() {
        let said = "x".repeat(SECTION_CHARS * 60);
        let reason = question();
        let built = build(&ask(Some(&said), Some(&reason)));
        assert_eq!(built.blocks.len(), MAX_BLOCKS);
        assert_eq!(built.blocks.len() - 5 + built.overflow.len(), 60);
        assert_eq!(
            texts(&built.blocks)[MAX_BLOCKS - 2],
            "[Red|Green|Type an answer…]"
        );
    }

    #[test]
    fn a_split_never_breaks_an_escape() {
        let text = format!("{}&amp;tail", "x".repeat(SECTION_CHARS - 2));
        let pieces = split(&text);
        assert_eq!(pieces[0].len(), SECTION_CHARS - 2);
        assert_eq!(pieces[1], "&amp;tail");
        assert_eq!(mrkdwn("a < b && c"), "a &lt; b &amp;&amp; c");
    }

    #[test]
    fn answered_loses_its_buttons_and_says_where_or_what() {
        let reason = question();
        let built = build(&ask(None, Some(&reason)));
        let after = answered(&built.blocks);
        assert!(after.iter().all(|b| b["type"] != "actions"));
        assert_eq!(texts(&after).last().unwrap(), "✓ Answered in Orion");
        let sent = answered_with(&built.blocks, "Red <b>");
        assert!(sent.iter().all(|b| b["type"] != "actions"));
        assert_eq!(
            texts(&sent)[sent.len() - 2..],
            ["*Which color?*", "✓ Sent: Red &lt;b&gt; · working again"],
            "the `or reply in this thread` line goes with the buttons"
        );
    }

    #[test]
    fn moved_on_and_re_sent_lose_their_buttons_and_a_lost_answer_gets_them_back() {
        let reason = question();
        let built = build(&ask(None, Some(&reason)));
        let moved = closed_as(&built.blocks, Close::MovedOn);
        assert!(moved.iter().all(|b| b["type"] != "actions"));
        assert_eq!(
            texts(&moved).last().unwrap(),
            "↓ Moved on: a newer question is below"
        );
        let resent = closed_as(&built.blocks, Close::Resent);
        assert_eq!(texts(&resent).last().unwrap(), "↓ Re-sent below");
        // As posted, the warning above its buttons.
        let again = reopened(&built.blocks);
        assert_eq!(
            texts(&again)[again.len() - 4..],
            [
                "*Which color?*",
                "*⚠ That didn't take: it's still waiting on this. Try again, or answer at your desk.*",
                "[Red|Green|Type an answer…]",
                "or reply in this thread",
            ]
        );
        assert_eq!(
            again[again.len() - 2]["elements"][0]["action_id"],
            "orion_option_0"
        );
    }

    /// A status change closes the messages it answers; a red over a newer
    /// dialog closes the one about the old dialog — moved on when the new
    /// one goes out — and leaves the one about this very wait up.
    #[test]
    fn a_newer_dialog_moves_an_open_message_on() {
        let plan = WaitReason::Plan { plan: "p".into() };
        let asked = question();
        let on_plan = PhoneMessage {
            kind: PhoneKind::Plan,
            ..message(Some(plan.clone()))
        };
        let red = AgentStatus::NeedsFeedback;
        assert_eq!(
            closes(&on_plan, AgentStatus::Running, None, true),
            Some(Close::AnsweredInOrion)
        );
        assert_eq!(
            closes(&on_plan, red, Some(&asked), true),
            Some(Close::MovedOn)
        );
        assert_eq!(
            closes(&on_plan, red, Some(&asked), false),
            Some(Close::AnsweredInOrion),
            "nothing is posted below it at the desk"
        );
        assert_eq!(
            closes(&on_plan, red, Some(&plan), true),
            None,
            "the same plan"
        );
        assert_eq!(closes(&on_plan, red, None, true), None, "no reason known");
        let crash = PhoneMessage {
            kind: PhoneKind::Crash,
            ..message(None)
        };
        assert_eq!(closes(&crash, AgentStatus::Terminated, None, true), None);
        assert_eq!(
            closes(&crash, AgentStatus::Running, None, true),
            Some(Close::AnsweredInOrion)
        );
    }

    #[test]
    fn the_summary_says_what_went_in() {
        let reason = several();
        assert_eq!(
            summary(
                Some(&reason),
                &Answer::Answers(vec![Picked::Options(vec![1]), Picked::Options(vec![0, 2])])
            ),
            "Green; Apple, Mango"
        );
        assert_eq!(summary(Some(&question()), &Answer::Option(0)), "Red");
        assert_eq!(
            summary(None, &Answer::Text("  go\n  left ".into())),
            "go left"
        );
        let plan = WaitReason::Plan { plan: "p".into() };
        assert_eq!(summary(Some(&plan), &Answer::ApprovePlan), "Approved");
    }

    #[test]
    fn kinds_follow_the_status_and_reason() {
        let plan = WaitReason::Plan { plan: "p".into() };
        assert_eq!(
            kind_of(AgentStatus::NeedsFeedback, Some(&plan)),
            Some(PhoneKind::Plan)
        );
        assert_eq!(
            kind_of(AgentStatus::NeedsFeedback, None),
            Some(PhoneKind::Question)
        );
        assert_eq!(
            kind_of(AgentStatus::Terminated, None),
            Some(PhoneKind::Crash)
        );
        assert_eq!(kind_of(AgentStatus::Running, None), None);
    }

    #[test]
    fn the_answer_modal_has_radios_checkboxes_and_words_per_question() {
        let reason = several();
        let view = modal_for(
            &message(Some(reason.clone())),
            Some(&reason),
            ACTION_ANSWER_MODAL,
        );
        assert_eq!(view["type"], "modal");
        assert_eq!(view["callback_id"], "orion_answer_modal");
        assert_eq!(view["private_metadata"], "D1:100.1");
        let blocks = view["blocks"].as_array().unwrap();
        let shape: Vec<(&str, &str, &str)> = blocks
            .iter()
            .map(|b| {
                (
                    b["block_id"].as_str().unwrap(),
                    b["element"]["type"].as_str().unwrap(),
                    b["label"]["text"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("orion_q_0", "radio_buttons", "Which color?"),
                ("orion_t_0", "plain_text_input", "Or in your own words"),
                ("orion_q_1", "checkboxes", "Which fruits?"),
                ("orion_t_1", "plain_text_input", "Or in your own words"),
            ]
        );
        assert!(blocks.iter().all(|b| b["optional"] == true));
        assert_eq!(
            blocks[2]["element"]["options"][2],
            json!({"text": {"type": "plain_text", "emoji": true, "text": "Mango"}, "value": "2"})
        );
    }

    #[test]
    fn type_an_answer_and_suggest_changes_are_one_box() {
        let reason = question();
        let view = modal_for(&message(Some(reason.clone())), Some(&reason), ACTION_TYPE);
        assert_eq!(view["callback_id"], "orion_type");
        assert_eq!(view["blocks"].as_array().unwrap().len(), 1);
        let block = &view["blocks"][0];
        assert_eq!(
            (
                &block["block_id"],
                &block["label"]["text"],
                &block["optional"]
            ),
            (&json!("orion_t_0"), &json!("Which color?"), &json!(false))
        );
        assert_eq!(block["element"]["multiline"], true);
        let plan = WaitReason::Plan { plan: "p".into() };
        let view = modal_for(&message(Some(plan.clone())), Some(&plan), ACTION_STEER);
        assert_eq!(view["callback_id"], "orion_steer");
        assert_eq!(view["blocks"][0]["block_id"], "orion_steer");
        assert!(view["title"]["text"].as_str().unwrap().chars().count() <= TITLE_CHARS);
    }

    fn submit(callback: &str, blocks: &[&str], values: Value) -> Value {
        let blocks: Vec<Value> = blocks
            .iter()
            .map(|id| json!({"type": "input", "block_id": id}))
            .collect();
        json!({"type": "view_submission", "user": {"id": "U1"},
            "view": {"callback_id": callback, "private_metadata": "D1:100.1",
                "blocks": blocks, "state": {"values": values}}})
    }

    #[test]
    fn a_submit_reads_radios_checkboxes_and_words_which_win() {
        let all = ["orion_q_0", "orion_t_0", "orion_q_1", "orion_t_1"];
        let payload = submit(
            ACTION_ANSWER_MODAL,
            &all,
            json!({
                "orion_q_0": {"pick": {"type": "radio_buttons", "selected_option": {"value": "1"}}},
                "orion_t_0": {"text": {"type": "plain_text_input", "value": null}},
                "orion_q_1": {"pick": {"type": "checkboxes",
                    "selected_options": [{"value": "2"}, {"value": "0"}]}},
                "orion_t_1": {"text": {"type": "plain_text_input", "value": ""}},
            }),
        );
        let route = route_submission_for(&payload, "U1", "D1").unwrap();
        assert_eq!(route.target, Target::Message("100.1".into()));
        assert_eq!(
            route.reply,
            Reply::Answers(vec![Picked::Options(vec![1]), Picked::Options(vec![0, 2])])
        );
        // Words win over the picks beside them.
        let payload = submit(
            ACTION_ANSWER_MODAL,
            &all,
            json!({
                "orion_q_0": {"pick": {"selected_option": {"value": "1"}}},
                "orion_t_0": {"text": {"value": " teal "}},
                "orion_q_1": {"pick": {"selected_options": []}},
                "orion_t_1": {"text": {"value": null}},
            }),
        );
        let route = route_submission_for(&payload, "U1", "D1").unwrap();
        assert_eq!(
            route.reply,
            Reply::Answers(vec![Picked::Text("teal".into()), Picked::Options(vec![])])
        );
        // ...and a question with neither is refused inside the modal.
        assert_eq!(
            submission_errors(&payload["view"], &route.reply, true),
            Some(json!({"response_action": "errors",
                "errors": {"orion_q_1": "Pick one, or write your own below"}}))
        );
        // One box: words, a steer's or an answer's.
        let steer = submit(
            ACTION_STEER,
            &["orion_steer"],
            json!({"orion_steer": {"text": {"value": "back up first"}}}),
        );
        assert_eq!(
            route_submission_for(&steer, "U1", "D1").unwrap().reply,
            Reply::Words("back up first".into())
        );
        // Someone else's, another conversation's, another app's.
        assert_eq!(route_submission_for(&steer, "U9", "D1"), None);
        assert_eq!(route_submission_for(&steer, "U1", "D2"), None);
        let other = submit("other_app", &[], json!({}));
        assert_eq!(route_submission_for(&other, "U1", "D1"), None);
    }

    #[test]
    fn a_stale_submit_is_refused_inside_the_modal() {
        let payload = submit(
            ACTION_TYPE,
            &["orion_t_0"],
            json!({"orion_t_0": {"text": {"value": "red"}}}),
        );
        let reply = Reply::Words("red".into());
        assert_eq!(
            submission_errors(&payload["view"], &reply, false),
            Some(json!({"response_action": "errors",
                "errors": {"orion_t_0": "This one was already answered"}}))
        );
        assert_eq!(submission_errors(&payload["view"], &reply, true), None);
    }

    fn press(user: &str, action: &str) -> Value {
        json!({"type": "block_actions", "user": {"id": user}, "trigger_id": "T1",
            "container": {"type": "message", "message_ts": "100.1", "channel_id": "D1"},
            "actions": [{"action_id": action, "value": "x"}]})
    }

    #[test]
    fn buttons_route_to_their_message() {
        let route = |p: &Value| route_action_for(p, "U1", "D1");
        let to = |reply| {
            Some(Route {
                target: Target::Message("100.1".into()),
                reply,
            })
        };
        assert_eq!(route(&press("U1", "orion_option_1")), to(Reply::Pick(1)));
        assert_eq!(route(&press("U1", "orion_approve")), to(Reply::Approve));
        for modal in [ACTION_TYPE, ACTION_STEER, ACTION_ANSWER_MODAL] {
            assert_eq!(
                route(&press("U1", modal)),
                to(Reply::Modal {
                    action: modal.into(),
                    trigger: "T1".into()
                })
            );
        }
        assert_eq!(route(&press("U9", "orion_approve")), None, "someone else");
        assert_eq!(route(&press("U1", "other_app")), None);
    }

    #[test]
    fn a_thread_reply_goes_to_its_message_and_a_plain_dm_to_the_dm() {
        let route = |e: Value| route_message_for(&e, "U1", "D1");
        let words = |target| {
            Some(Route {
                target,
                reply: Reply::Words("purple".into()),
            })
        };
        assert_eq!(
            route(
                json!({"channel": "D1", "user": "U1", "text": "purple", "ts": "200.1",
                "thread_ts": "100.1"})
            ),
            words(Target::Message("100.1".into()))
        );
        assert_eq!(
            route(json!({"channel": "D1", "user": "U1", "text": " purple ", "ts": "200.1"})),
            words(Target::Dm)
        );
        // Not the installing user, the bot, an edit, another conversation.
        assert_eq!(
            route(json!({"channel": "D1", "user": "U9", "text": "x", "ts": "2"})),
            None
        );
        assert_eq!(
            route(json!({"channel": "D1", "user": "U1", "bot_id": "B1", "text": "x", "ts": "2"})),
            None
        );
        assert_eq!(
            route(
                json!({"channel": "D1", "user": "U1", "subtype": "message_changed",
                "text": "x", "ts": "2"})
            ),
            None
        );
        assert_eq!(
            route(json!({"channel": "C1", "user": "U1", "text": "x", "ts": "2"})),
            None
        );
    }

    #[test]
    fn words_steer_a_plan_and_answer_a_question() {
        assert_eq!(
            answer_for(PhoneKind::Plan, Reply::Words("w".into())),
            Some(Answer::Steer("w".into()))
        );
        assert_eq!(
            answer_for(PhoneKind::Question, Reply::Words("w".into())),
            Some(Answer::Text("w".into()))
        );
        assert_eq!(
            answer_for(
                PhoneKind::Question,
                Reply::Answers(vec![Picked::Options(vec![0])])
            ),
            Some(Answer::Answers(vec![Picked::Options(vec![0])]))
        );
        let modal = Reply::Modal {
            action: ACTION_TYPE.into(),
            trigger: "T".into(),
        };
        assert_eq!(answer_for(PhoneKind::Plan, modal), None);
    }
}
