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
//!   3000-character sections), then the question or the plan, and its
//!   buttons — each option and `Something else…`, or `Approve` and a reply
//!   button.
//! - **Replies**: a button answers through `Daemon::answer`; `Something
//!   else…` and the reply button say "Reply in this thread". A reply in a
//!   message's thread answers that session in words (a steer, for a plan);
//!   a plain DM answers the most recent message still open. Only the
//!   installing user's replies count. Each answer gets `✓ Sent · working
//!   again` in its thread.
//! - **Answered at the desk**: a session that leaves the state its message
//!   was about any other way has the message updated to say "Answered in
//!   Orion", its buttons gone.
//!
//! Which message is which session's lives in the store
//! (`phone_messages`), so a reply finds its session after a restart.

use std::sync::Arc;

use orion_core::away::PhoneKind;
use orion_core::{AgentId, AgentStatus, Answer, ServerEvent, WaitReason};
use serde_json::{json, Value};

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

/// What the thread hears after an answer went in.
pub const SENT: &str = "✓ Sent · working again";
/// What an answered message gains.
pub const ANSWERED_IN_ORION: &str = "Answered in Orion";
/// The ephemeral hint `Something else…` and the reply button give.
pub const REPLY_HINT: &str = "Reply in this thread";
/// What a session no automation started is called on its context line;
/// one an automation started goes by the automation's name.
const AGENT_KIND: &str = "Agent";

/// The `action_id`s on a message's buttons.
const ACTION_OPTION: &str = "orion_option";
const ACTION_APPROVE: &str = "orion_approve";
const ACTION_REPLY: &str = "orion_reply";

/// One phone message, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneMessage {
    pub channel: String,
    pub ts: String,
    pub agent: AgentId,
    pub kind: PhoneKind,
    /// The blocks it was posted with (JSON), to rebuild it as answered.
    pub blocks: String,
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
    let (closing, actions, text) = match ask.reason {
        Some(WaitReason::Question {
            question, options, ..
        }) => {
            let mut buttons: Vec<Value> = options
                .iter()
                .enumerate()
                .map(|(i, label)| button(label, &format!("{ACTION_OPTION}_{i}"), &i.to_string()))
                .collect();
            buttons.push(button("Something else…", ACTION_REPLY, "reply"));
            (
                format!("*{}*", escape(question)),
                Some(buttons),
                format!("{}: {question}", ask.session),
            )
        }
        Some(WaitReason::Plan { plan }) => {
            body.extend(split(&mrkdwn(plan)));
            let mut approve = button("Approve", ACTION_APPROVE, "approve");
            approve["style"] = "primary".into();
            let reply = button(":leftwards_arrow_with_hook:", ACTION_REPLY, "reply");
            (
                "*Approve this plan?*".to_string(),
                Some(vec![approve, reply]),
                format!("{}: a plan waits for your approval", ask.session),
            )
        }
        Some(WaitReason::Permission { tool }) => (
            format!("*Allow {}?*", escape(tool)),
            Some(vec![
                button("Yes", &format!("{ACTION_OPTION}_0"), "0"),
                button("Something else…", ACTION_REPLY, "reply"),
            ]),
            format!("{}: allow {tool}?", ask.session),
        ),
        Some(WaitReason::Crash { exit_code }) => (
            match exit_code {
                Some(code) => format!("*Crashed* (exit {code})"),
                None => "*Crashed*".to_string(),
            },
            None,
            format!("{} crashed", ask.session),
        ),
        None if ask.kind == PhoneKind::Crash => {
            ("*Crashed*".into(), None, format!("{} crashed", ask.session))
        }
        None => (
            "*Waiting for you*".into(),
            Some(vec![button("Something else…", ACTION_REPLY, "reply")]),
            format!("{} is waiting for you", ask.session),
        ),
    };
    let tail = 1 + usize::from(actions.is_some());
    let room = MAX_BLOCKS - head.len() - tail;
    let overflow = match body.len() > room {
        true => body.split_off(room),
        false => Vec::new(),
    };
    head.extend(body.into_iter().map(section));
    head.push(section(closing));
    if let Some(buttons) = actions {
        head.push(json!({"type": "actions", "block_id": "orion_answer", "elements": buttons}));
    }
    Built {
        text,
        blocks: head,
        overflow,
    }
}

/// `blocks` as answered at the desk: its buttons gone, "Answered in
/// Orion" at the foot.
pub fn answered(blocks: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = blocks
        .iter()
        .filter(|b| b["type"] != "actions")
        .cloned()
        .collect();
    out.push(json!({"type": "context", "elements": [{"type": "mrkdwn",
        "text": format!("✓ {ANSWERED_IN_ORION}")}]}));
    out
}

fn section(text: String) -> Value {
    json!({"type": "section", "text": {"type": "mrkdwn", "text": text}})
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
    /// `Something else…` or the reply button: the hint to reply in the
    /// thread.
    Hint,
    /// Words, typed in the thread or the DM.
    Words(String),
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
        ACTION_REPLY => Reply::Hint,
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
        Reply::Hint => None,
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

    /// A reply on the phone: answer the session it is to, and say so in
    /// the message's thread.
    pub(crate) async fn phone_reply(self: &Arc<Self>, session: &Arc<Session>, route: Route) {
        let message = match &route.target {
            Target::Message(ts) => self.store.phone_message(&session.dm, ts).ok().flatten(),
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
        let thread = Some(message.ts.clone());
        if !open {
            say("This one was already answered.".into(), thread).await;
            return;
        }
        let Some(answer) = answer_for(message.kind, route.reply) else {
            let (api, dm, me) = (session.api.clone(), session.dm.clone(), session.me.clone());
            slack("chat.postEphemeral", move || {
                api.post_ephemeral(&dm, &me, REPLY_HINT, thread.as_deref())
            })
            .await;
            return;
        };
        // Marked first: the status change the answer causes must not
        // read as an answer at the desk.
        let _ = self
            .store
            .set_phone_answered(&message.channel, &message.ts, Some("phone"));
        tracing::info!(agent = %message.agent, ?answer, "answered from the phone");
        match self.answer(&message.agent, answer).await {
            Ok(()) => {
                say(SENT.into(), thread).await;
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

    fn question() -> WaitReason {
        WaitReason::Question {
            question: "Which color?".into(),
            options: vec!["Red".into(), "Green".into()],
            multi: false,
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
    fn a_question_reads_top_to_bottom_as_designed() {
        let reason = question();
        let built = build(&ask(Some("I looked at **both**.\n# Next"), Some(&reason)));
        assert_eq!(
            texts(&built.blocks),
            [
                "● Agent · web · 10:42",
                "Fix the login redirect",
                "I looked at *both*.\n*Next*",
                "*Which color?*",
                "[Red|Green|Something else…]",
            ]
        );
        assert_eq!(built.text, "Fix the login redirect: Which color?");
        assert!(built.overflow.is_empty());
        let buttons = &built.blocks[4]["elements"];
        assert_eq!(buttons[1]["action_id"], "orion_option_1");
        assert_eq!(buttons[2]["action_id"], "orion_reply");
    }

    #[test]
    fn a_plan_shows_whole_with_approve_and_reply() {
        let plan = WaitReason::Plan {
            plan: "1. Back up\n2. Migrate".into(),
        };
        let built = build(&ask(None, Some(&plan)));
        assert_eq!(
            texts(&built.blocks)[2..],
            [
                "1. Back up\n2. Migrate",
                "*Approve this plan?*",
                "[Approve|:leftwards_arrow_with_hook:]"
            ]
        );
        assert_eq!(built.blocks[4]["elements"][0]["style"], "primary");
    }

    #[test]
    fn a_long_response_is_split_never_cut_short() {
        let line = "word ".repeat(100);
        let said = vec![line.trim(); 40].join("\n");
        let reason = question();
        let built = build(&ask(Some(&said), Some(&reason)));
        let sections: Vec<String> = texts(&built.blocks)[2..built.blocks.len() - 2].to_vec();
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
        assert_eq!(built.blocks.len() - 4 + built.overflow.len(), 60);
        assert_eq!(
            texts(&built.blocks).last().unwrap(),
            "[Red|Green|Something else…]"
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
    fn answered_loses_its_buttons_and_says_where() {
        let reason = question();
        let built = build(&ask(None, Some(&reason)));
        let after = answered(&built.blocks);
        assert!(after.iter().all(|b| b["type"] != "actions"));
        assert_eq!(texts(&after).last().unwrap(), "✓ Answered in Orion");
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

    fn press(user: &str, action: &str) -> Value {
        json!({"type": "block_actions", "user": {"id": user},
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
        assert_eq!(route(&press("U1", "orion_reply")), to(Reply::Hint));
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
        assert_eq!(answer_for(PhoneKind::Plan, Reply::Hint), None);
    }
}
