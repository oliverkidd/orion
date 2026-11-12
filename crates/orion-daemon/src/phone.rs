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
//!     answer…`, and `or just reply here` under them;
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
//!   words (a steer, for a plan); a plain DM answers the most recent
//!   message still open. Only the installing user's replies count. Words
//!   on a several-question ask cancel its dialog and go in as the next
//!   turn, the questions quoted (`answer::quoted_turn`).
//! - **After an answer**: the message is updated in place, its buttons
//!   swapped for `✓ Sent: <what went in> · working again`. One that did
//!   not go in gets `✗ Not sent: …` in its thread.
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
/// What a modal submitted on an answered message shows.
pub const ALREADY_ANSWERED: &str = "This one was already answered";
/// What a modal question with neither a pick nor words shows.
const NOTHING_PICKED: &str = "Pick one, or write your own below";
/// The line under a question's buttons.
const REPLY_HERE: &str = "or just reply here";
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

/// The `block_id`s: the `or just reply here` line, and in a modal each
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
/// whether `or just reply here` goes under them, and its notification line.
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
    closed(blocks, format!("✓ {ANSWERED_IN_ORION}"))
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
    /// The most recent message still open.
    Latest,
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
/// thread reply to its message, a plain message to the latest open one.
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
        _ => Target::Latest,
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

impl Daemon {
    /// A status change: close the agent's open messages it answers, then
    /// post a new one when it is one to send.
    async fn phone_on_status(
        self: &Arc<Self>,
        agent: &AgentId,
        status: AgentStatus,
        reason: Option<WaitReason>,
    ) {
        let Some(session) = self.slack.session() else {
            return;
        };
        let open = self.store.open_phone_messages(agent).unwrap_or_default();
        for message in open.into_iter().filter(|m| status_of(m.kind) != status) {
            self.close_answered_in_orion(&session, message).await;
        }
        let Some(kind) = kind_of(status, reason.as_ref()) else {
            return;
        };
        let away = self.away.is_away();
        let settings = crate::config::Config::load().away;
        let minute = {
            use chrono::Timelike;
            let now = chrono::Local::now();
            now.hour() * 60 + now.minute()
        };
        if !away || settings.is_quiet(minute) || !settings.sends(kind) {
            return;
        }
        self.phone_post(&session, agent, kind, reason).await;
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

    /// `message`'s session moved on without the phone: say so on it.
    async fn close_answered_in_orion(&self, session: &Arc<Session>, message: PhoneMessage) {
        if let Err(e) = self
            .store
            .set_phone_answered(&message.channel, &message.ts, Some("orion"))
        {
            tracing::warn!(error = %e, "phone message not closed");
        }
        let blocks: Vec<Value> = serde_json::from_str(&message.blocks).unwrap_or_default();
        let blocks = answered(&blocks);
        let api = session.api.clone();
        slack("chat.update", move || {
            api.update(&message.channel, &message.ts, ANSWERED_IN_ORION, &blocks)
        })
        .await;
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

    /// A reply on the phone: open the modal a button asks for, or answer
    /// the session it is to and update its message to say what went in.
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

    pub(crate) async fn phone_reply(self: &Arc<Self>, session: &Arc<Session>, route: Route) {
        let message = match &route.target {
            Target::Message(ts) => self.phone_message_soon(&session.dm, ts).await,
            Target::Latest => self
                .store
                .latest_open_phone_message(&session.dm)
                .ok()
                .flatten(),
        };
        let say = |text: String, thread: Option<String>| {
            let (api, dm) = (session.api.clone(), session.dm.clone());
            slack("chat.postMessage", move || {
                api.post(&dm, &text, None, thread.as_deref()).map(drop)
            })
        };
        let Some((message, open)) = message else {
            if route.target == Target::Latest {
                say("Nothing is waiting for you.".into(), None).await;
            }
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
        match self.answer(&message.agent, answer).await {
            Ok(()) => {
                let blocks: Vec<Value> = serde_json::from_str(&message.blocks).unwrap_or_default();
                let blocks = answered_with(&blocks, &said);
                let api = session.api.clone();
                slack("chat.update", move || {
                    api.update(&message.channel, &message.ts, &sent_line(&said), &blocks)
                })
                .await;
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
                "or just reply here",
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
                "or just reply here",
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
            "the `or just reply here` line goes with the buttons"
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
    fn a_thread_reply_goes_to_its_message_and_a_plain_dm_to_the_latest() {
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
            words(Target::Latest)
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
