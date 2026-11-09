//! The board's SLACK section: the Slack conversations waiting on you,
//! made from the message events the daemon forwards unfiltered
//! (`ServerEvent::SlackMessage`, the user token's `message.im`,
//! `message.mpim`, `message.channels` and `message.groups`).
//!
//! A message is for you, by Settings › Connections › SLACK, when it is a
//! direct message, mentions you (`<@your id>`), replies in a thread you
//! have posted in (tracked from the same stream), sits in a channel
//! listed under **Channels, every message** (by name, from
//! `conversations.info`), or says a keyword. Messages that share a place
//! share a card ([`Waiting`]): a DM's top level is one conversation, a
//! thread is one, and a channel's top-level message is its own. A card
//! goes when you post there later — in the same thread, or at the top
//! of the same conversation — and `⌘D` hides it until a new message.
//!
//! The last [`KEEP_EVENTS`] messages are kept in memory; what they made
//! ([`Kept`]) is saved in the needs-you store, so the cards outlive a
//! restart. Sender names, channel names and permalinks come from Slack's
//! Web API (`users.info`, `conversations.info`, `chat.getPermalink`)
//! with the user token, through `orion_core::http` at
//! `ORION_SLACK_API_URL`, each asked once and cached. Those asks follow
//! Slack events, so there are none while Slack isn't connected.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use orion_core::ServerEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{Card, CardSource, CardTarget, Section};
use crate::app::App;
use crate::config::{BannerWhen, SlackSettings};

/// How many message events are kept in memory.
pub const KEEP_EVENTS: usize = 200;
/// How many waiting conversations are kept; the oldest go first.
const KEEP_WAITING: usize = 200;
/// How many threads you've posted in are remembered.
const KEEP_THREADS: usize = 500;

/// The subtypes that are someone saying something; every other
/// (`message_changed`, `channel_join`, `bot_message`, …) is not.
const SAID: &[&str] = &["thread_broadcast", "file_share", "me_message"];

/// A Slack message, as the daemon forwards it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub channel: String,
    /// `im`, `mpim`, `channel` or `group`.
    pub channel_type: String,
    pub user: Option<String>,
    pub text: String,
    pub ts: String,
    pub thread_ts: Option<String>,
    pub subtype: Option<String>,
}

impl Message {
    /// `ServerEvent::SlackMessage` as a message, and its whole event.
    pub fn of(event: ServerEvent) -> Option<(Message, String)> {
        let ServerEvent::SlackMessage {
            channel,
            channel_type,
            user,
            text,
            ts,
            thread_ts,
            subtype,
            event_json,
        } = event
        else {
            return None;
        };
        let message = Message {
            channel,
            channel_type,
            user,
            text,
            ts,
            thread_ts,
            subtype,
        };
        Some((message, event_json))
    }

    /// A direct or group direct message.
    fn direct(&self) -> bool {
        matches!(self.channel_type.as_str(), "im" | "mpim")
    }

    /// The place it shares a card with: its thread, its own `ts` for a
    /// channel's top-level message, None for a DM's top level.
    fn root(&self) -> Option<String> {
        match &self.thread_ts {
            Some(thread) => Some(thread.clone()),
            None if self.direct() => None,
            None => Some(self.ts.clone()),
        }
    }

    /// Someone saying something, outside the phone's DM.
    fn said(&self, who: &Who) -> bool {
        self.user.is_some()
            && who.phone.as_deref() != Some(self.channel.as_str())
            && self
                .subtype
                .as_deref()
                .is_none_or(|subtype| SAID.contains(&subtype))
    }
}

/// Why a message is for you, the strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    Direct,
    Mention,
    Thread,
    Channel,
    Keyword,
}

/// One conversation waiting on you: what a card is made of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    pub channel: String,
    /// [`Message::root`].
    #[serde(default)]
    pub root: Option<String>,
    /// In a thread, not at a conversation's top level.
    #[serde(default)]
    pub in_thread: bool,
    #[serde(default)]
    pub direct: bool,
    /// The newest message's sender, text and `ts`.
    pub user: String,
    pub text: String,
    pub ts: String,
    pub why: Why,
    /// The keyword it said, for [`Why::Keyword`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
}

impl Waiting {
    /// Whether `mine`, your own message, answers it: later, in the same
    /// conversation, in its thread or at the top level it sits at.
    fn answered_by(&self, mine: &Message) -> bool {
        self.channel == mine.channel
            && later(&mine.ts, &self.ts)
            && match &mine.thread_ts {
                None => !self.in_thread,
                Some(thread) => self.root.as_deref() == Some(thread.as_str()),
            }
    }
}

/// What the messages made, as the needs-you store keeps it: the
/// conversations waiting on you, the threads you've posted in, and the
/// names Slack gave.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    pub waiting: Vec<Waiting>,
    /// `(channel, thread ts)`, the newest last.
    pub threads: Vec<(String, String)>,
    /// Display names by user id.
    pub users: BTreeMap<String, String>,
    /// Channel names (no `#`) by channel id.
    pub channels: BTreeMap<String, String>,
}

impl Kept {
    fn in_thread(&self, channel: &str, thread: &str) -> bool {
        self.threads
            .iter()
            .any(|(c, t)| c == channel && t == thread)
    }

    fn remember_thread(&mut self, channel: &str, thread: &str) {
        if self.in_thread(channel, thread) {
            return;
        }
        self.threads.push((channel.to_string(), thread.to_string()));
        if self.threads.len() > KEEP_THREADS {
            self.threads.remove(0);
        }
    }

    /// Whether `channel` is listed under **Channels, every message**.
    fn listed(&self, settings: &SlackSettings, channel: &str) -> bool {
        self.channels.get(channel).is_some_and(|name| {
            settings
                .channels
                .iter()
                .any(|c| c.trim_start_matches('#').eq_ignore_ascii_case(name))
        })
    }
}

/// Who you are, and what counts, for one message.
#[derive(Debug, Clone, Default)]
pub struct Who {
    /// Your Slack user id (`SlackStatus.user_id`).
    pub me: Option<String>,
    /// The phone's DM (`SlackStatus.phone_channel`), never a card.
    pub phone: Option<String>,
    pub settings: SlackSettings,
}

/// Whether `text` mentions `me`: `<@U1>` or `<@U1|name>`.
fn mentions(text: &str, me: &str) -> bool {
    text.contains(&format!("<@{me}>")) || text.contains(&format!("<@{me}|"))
}

/// Why `m` is for you, and the keyword it said; None when it isn't.
fn why(kept: &Kept, m: &Message, who: &Who) -> Option<(Why, Option<String>)> {
    let s = &who.settings;
    if m.direct() && s.dms {
        return Some((Why::Direct, None));
    }
    if s.mentions && who.me.as_deref().is_some_and(|me| mentions(&m.text, me)) {
        return Some((Why::Mention, None));
    }
    if s.threads
        && m.thread_ts
            .as_deref()
            .is_some_and(|t| kept.in_thread(&m.channel, t))
    {
        return Some((Why::Thread, None));
    }
    if !m.direct() && kept.listed(s, &m.channel) {
        return Some((Why::Channel, None));
    }
    let text = m.text.to_lowercase();
    s.keywords
        .iter()
        .find(|k| !k.is_empty() && text.contains(&k.to_lowercase()))
        .map(|k| (Why::Keyword, Some(k.clone())))
}

/// Whether `w`'s rule is still on in Settings.
fn wanted(kept: &Kept, w: &Waiting, s: &SlackSettings) -> bool {
    match w.why {
        Why::Direct => s.dms,
        Why::Mention => s.mentions,
        Why::Thread => s.threads,
        Why::Channel => kept.listed(s, &w.channel),
        Why::Keyword => w
            .keyword
            .as_deref()
            .is_some_and(|k| s.keywords.iter().any(|kw| kw.eq_ignore_ascii_case(k))),
    }
}

/// Take `m` into `kept`: your own message answers what it answers and
/// marks its thread yours; anyone else's that is for you opens or
/// updates its conversation's card. The card's reason when it did.
pub fn apply(kept: &mut Kept, m: &Message, who: &Who) -> Option<Why> {
    if !m.said(who) {
        return None;
    }
    // Until Slack has said who you are, no message can be told yours.
    let (user, me) = (m.user.as_deref()?, who.me.as_deref()?);
    if me == user {
        let thread = m.thread_ts.as_deref().unwrap_or(&m.ts);
        kept.remember_thread(&m.channel, thread);
        kept.waiting.retain(|w| !w.answered_by(m));
        return None;
    }
    let (why, keyword) = why(kept, m, who)?;
    let root = m.root();
    match kept
        .waiting
        .iter_mut()
        .find(|w| w.channel == m.channel && w.root == root)
    {
        Some(w) => {
            if !later(&m.ts, &w.ts) {
                return None;
            }
            w.user = user.to_string();
            w.text = m.text.clone();
            w.ts = m.ts.clone();
            if why < w.why {
                (w.why, w.keyword) = (why, keyword);
            }
        }
        None => {
            kept.waiting.push(Waiting {
                channel: m.channel.clone(),
                root,
                in_thread: m.thread_ts.is_some(),
                direct: m.direct(),
                user: user.to_string(),
                text: m.text.clone(),
                ts: m.ts.clone(),
                why,
                keyword,
            });
            if kept.waiting.len() > KEEP_WAITING {
                if let Some(oldest) =
                    (0..kept.waiting.len()).min_by_key(|&i| ts_key(&kept.waiting[i].ts))
                {
                    kept.waiting.remove(oldest);
                }
            }
        }
    }
    Some(why)
}

/// A Slack `ts` (`1355517523.000005`) as something that sorts.
fn ts_key(ts: &str) -> (u64, u64) {
    let (secs, frac) = ts.split_once('.').unwrap_or((ts, "0"));
    (secs.parse().unwrap_or(0), frac.parse().unwrap_or(0))
}

fn later(a: &str, b: &str) -> bool {
    ts_key(a) > ts_key(b)
}

/// A Slack `ts` as a time.
fn time_of(ts: &str) -> DateTime<Utc> {
    let (secs, micros) = ts_key(ts);
    DateTime::from_timestamp(secs as i64, (micros.min(999_999) * 1000) as u32).unwrap_or_default()
}

/// Slack's markup as plain text: `<@U1>` as `@name`, `<#C1|eng>` as
/// `#eng`, a link as its label, `&amp;` as `&`.
pub fn plain(text: &str, users: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            rest = &rest[open..];
            break;
        };
        let inner = &rest[open + 1..open + close];
        let (target, label) = match inner.split_once('|') {
            Some((target, label)) => (target, Some(label)),
            None => (inner, None),
        };
        let shown = match (target.chars().next(), label) {
            (Some('@'), _) => {
                let name = users.get(&target[1..]).map(String::as_str);
                format!("@{}", name.or(label).unwrap_or("someone"))
            }
            (Some('#'), Some(label)) => format!("#{label}"),
            (Some('#'), None) => "#channel".into(),
            (Some('!'), Some(label)) => label.to_string(),
            (Some('!'), None) => format!("@{}", target[1..].split('^').next().unwrap_or("")),
            (_, Some(label)) => label.to_string(),
            (_, None) => target.to_string(),
        };
        out.push_str(&shown);
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    crate::markdown::decode_entities(&out)
}

/// The first line worth showing of a message.
fn first_line(text: &str, users: &BTreeMap<String, String>) -> String {
    let text = plain(text, users);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

/// A sender's name, while Slack hasn't said it yet.
const SOMEONE: &str = "Someone";

fn sender(kept: &Kept, user: &str) -> String {
    kept.users
        .get(user)
        .cloned()
        .unwrap_or_else(|| SOMEONE.into())
}

/// A card's second line: `direct message`, `#eng · mentions you`.
fn place(kept: &Kept, w: &Waiting) -> String {
    if w.direct {
        return "direct message".into();
    }
    let channel = match kept.channels.get(&w.channel) {
        Some(name) => format!("#{name}"),
        None => "a channel".into(),
    };
    match (w.why, &w.keyword) {
        (Why::Mention, _) => format!("{channel} · mentions you"),
        (Why::Thread, _) => format!("{channel} · reply in your thread"),
        (Why::Keyword, Some(k)) => format!("{channel} · says {k}"),
        _ => channel,
    }
}

fn card(kept: &Kept, w: &Waiting) -> Card {
    let line = first_line(&w.text, &kept.users);
    let urgency = u8::from(matches!(w.why, Why::Direct | Why::Mention));
    Card::new(
        Section::Slack,
        CardTarget::SlackMessage {
            channel: w.channel.clone(),
            root: w.root.clone(),
            ts: w.ts.clone(),
        },
        format!("{}: {line}", sender(kept, &w.user)),
        place(kept, w),
        time_of(&w.ts),
        urgency,
    )
}

/// The SLACK section's source.
pub struct Inbox;

impl CardSource for Inbox {
    fn cards(&self, app: &App) -> Vec<Card> {
        let kept = &app.needs_you.store.slack;
        let settings = &app.needs_you.slack.settings;
        kept.waiting
            .iter()
            .filter(|w| wanted(kept, w, settings))
            .map(|w| card(kept, w))
            .collect()
    }
}

/// A banner and sound owed for a message — its conversation as the
/// message left it — once its sender and channel have names.
type Banner = Waiting;

/// A Web API ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// `users.info`: a sender's display name.
    User(String),
    /// `conversations.info`: a channel's name.
    Channel(String),
    /// `chat.getPermalink` of one message, opened when it lands.
    Permalink { channel: String, ts: String },
}

impl Lookup {
    /// The id it is asked under, so it is asked once at a time.
    fn id(&self) -> String {
        match self {
            Lookup::User(id) | Lookup::Channel(id) => id.clone(),
            Lookup::Permalink { channel, ts } => format!("{channel}/{ts}"),
        }
    }

    /// Ask Slack. Blocks: run it under `spawn_blocking`.
    fn run(&self) -> Result<String, String> {
        let text = |v: &Value, path: &str| {
            v.pointer(path)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        match self {
            Lookup::User(id) => {
                let reply = call("users.info", &[("user", id)])?;
                user_name(&reply["user"]).ok_or_else(|| "users.info gave no name".into())
            }
            Lookup::Channel(id) => {
                let reply = call("conversations.info", &[("channel", id)])?;
                text(&reply, "/channel/name")
                    .ok_or_else(|| "conversations.info gave no name".into())
            }
            Lookup::Permalink { channel, ts } => {
                let reply = call(
                    "chat.getPermalink",
                    &[("channel", channel), ("message_ts", ts)],
                )?;
                text(&reply, "/permalink").ok_or_else(|| "Slack gave no link".into())
            }
        }
    }
}

/// An ask's answer, for the loop.
#[derive(Debug)]
pub struct Looked {
    pub lookup: Lookup,
    pub found: Result<String, String>,
}

/// A Slack user object's name: the display name, else the real name,
/// else the handle.
fn user_name(user: &Value) -> Option<String> {
    [
        "/profile/display_name",
        "/profile/real_name",
        "/real_name",
        "/name",
    ]
    .iter()
    .find_map(|path| user.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(str::to_string)
}

/// The token every ask goes out with: the installing user's.
const TOKEN: &str = orion_core::secrets::SLACK_ACCOUNTS[2];

/// POST `method` with form `params` as the user. Slack's `error` code
/// is the error, as is a status that isn't JSON.
fn call(method: &str, params: &[(&str, &str)]) -> Result<Value, String> {
    let token = orion_core::secrets::slack_token(TOKEN).ok_or("Slack isn't connected")?;
    let url = format!("{}/{method}", orion_core::http::slack_api_url());
    let body = params
        .iter()
        .map(|(k, v)| format!("{k}={}", form_escape(v)))
        .collect::<Vec<_>>()
        .join("&");
    let headers = ["Content-Type: application/x-www-form-urlencoded".to_string()];
    let response = orion_core::http::request(&orion_core::http::Request {
        method: "POST",
        url: &url,
        headers: &headers,
        bearer: Some(&token),
        body: Some(body.as_bytes()),
        timeout_secs: 15,
    })?;
    let json: Value = serde_json::from_slice(&response.body)
        .map_err(|_| format!("{method}: Slack answered {}", response.status))?;
    match json["ok"].as_bool() {
        Some(true) => Ok(json),
        _ => Err(json["error"]
            .as_str()
            .unwrap_or("Slack refused")
            .to_string()),
    }
}

/// `value` for a form body: everything but letters, digits and `-._~`
/// percent-encoded.
fn form_escape(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// What the app keeps for the section between frames.
#[derive(Debug, Default)]
pub struct Slack {
    /// Settings › Connections › SLACK, as last read.
    pub settings: SlackSettings,
    /// What the store held at start, and every message since pushed out
    /// of `events`, taken in: `events` folded over it is the store's.
    base: Kept,
    /// The last [`KEEP_EVENTS`] messages.
    events: VecDeque<Message>,
    /// `chat.getPermalink` answers, by `(channel, ts)`.
    permalinks: HashMap<(String, String), String>,
    /// Asks out now, by [`Lookup::id`].
    asking: HashSet<String>,
    banners: Vec<Banner>,
}

/// Who you are, as the daemon last said.
fn who(app: &App) -> Who {
    Who {
        me: app.slack_status.user_id.clone(),
        phone: app.slack_status.phone_channel.clone(),
        settings: app.needs_you.slack.settings.clone(),
    }
}

/// The store was loaded: what it kept is where the messages start from.
pub fn loaded(app: &mut App) {
    app.needs_you.slack.base = app.needs_you.store.slack.clone();
}

/// A name for `user` from a message event's own `user_profile`, which
/// Slack sometimes sends with it.
fn profile_name(event_json: &str) -> Option<String> {
    let event: Value = serde_json::from_str(event_json).ok()?;
    user_name(&event["user_profile"])
}

/// `ServerEvent::SlackMessage`: taken in once, the cards brought up to
/// date, its banner owed, and any name it needs asked for.
pub fn heard(app: &mut App, m: Message, event_json: &str) {
    let heard_before = app
        .needs_you
        .slack
        .events
        .iter()
        .any(|e| e.channel == m.channel && e.ts == m.ts);
    if heard_before {
        return;
    }
    let who = who(app);
    let slack = &mut app.needs_you.slack;
    let mut next = app.needs_you.store.slack.clone();
    if let (Some(user), Some(name)) = (&m.user, profile_name(event_json)) {
        slack.base.users.entry(user.clone()).or_insert(name.clone());
        next.users.entry(user.clone()).or_insert(name);
    }
    slack.events.push_back(m.clone());
    if slack.events.len() > KEEP_EVENTS {
        if let Some(old) = slack.events.pop_front() {
            apply(&mut slack.base, &old, &who);
        }
    }
    let why = apply(&mut next, &m, &who);
    let want_channel = !m.direct()
        && !next.channels.contains_key(&m.channel)
        && (why.is_some() || !who.settings.channels.is_empty());
    if let Some(why) = why {
        let w = next
            .waiting
            .iter()
            .find(|w| w.channel == m.channel && w.ts == m.ts);
        let banner = match who.settings.banner {
            BannerWhen::Every => true,
            BannerWhen::DmsAndMentions => matches!(why, Why::Direct | Why::Mention),
            BannerWhen::Never => false,
        };
        if let (true, Some(w)) = (banner, w) {
            app.needs_you.slack.banners.push(w.clone());
        }
        if let Some(user) = m.user.as_ref().filter(|u| !next.users.contains_key(*u)) {
            ask(app, Lookup::User(user.clone()));
        }
    }
    if want_channel {
        ask(app, Lookup::Channel(m.channel.clone()));
    }
    commit(app, next);
}

/// Make `next` what the store keeps, saving it when it changed, and
/// bring an open board up to date.
fn commit(app: &mut App, next: Kept) {
    if next != app.needs_you.store.slack {
        app.needs_you.store.slack = next;
        super::save(app);
    }
    super::refresh(app);
    app.dirty = true;
}

/// Ask Slack off the loop, once at a time per thing.
fn ask(app: &mut App, lookup: Lookup) {
    let Some(tx) = app.needs_you.tx.clone() else {
        return;
    };
    if !app.needs_you.slack.asking.insert(lookup.id()) {
        return;
    }
    tokio::spawn(async move {
        let asked = lookup.clone();
        let found = tokio::task::spawn_blocking(move || asked.run())
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        let _ = tx.send(super::Answer::Slack(Looked { lookup, found }));
    });
}

/// An ask's answer lands: a name cached (and the cards made again from
/// the kept messages, since a channel's name can make one count), or a
/// link opened.
pub fn landed(app: &mut App, looked: Looked) {
    let Looked { lookup, found } = looked;
    app.needs_you.slack.asking.remove(&lookup.id());
    match (lookup, found) {
        (Lookup::Permalink { channel, ts }, Ok(url)) => {
            app.needs_you
                .slack
                .permalinks
                .insert((channel, ts), url.clone());
            if !crate::open_url::open_url(&url, None) {
                app.flash = Some(crate::flash::Flash::failed(format!("couldn't open {url}")));
            }
        }
        (Lookup::Permalink { .. }, Err(why)) => {
            app.flash = Some(crate::flash::Flash::failed(format!(
                "Slack gave no link: {why}"
            )));
        }
        (Lookup::User(id), Ok(name)) => {
            app.needs_you
                .slack
                .base
                .users
                .insert(id.clone(), name.clone());
            let mut next = app.needs_you.store.slack.clone();
            next.users.insert(id, name);
            commit(app, next);
        }
        (Lookup::Channel(id), Ok(name)) => {
            app.needs_you.slack.base.channels.insert(id, name);
            let who = who(app);
            let slack = &app.needs_you.slack;
            let mut next = slack.base.clone();
            for m in &slack.events {
                apply(&mut next, m, &who);
            }
            commit(app, next);
        }
        (lookup, Err(why)) => {
            tracing::debug!("needs you: Slack {lookup:?}: {why}");
        }
    }
}

/// Enter on a Slack card: its newest message's link, when Slack already
/// gave it — else it is asked for, and opens when it lands.
pub fn permalink(app: &mut App, channel: &str, ts: &str) -> Option<String> {
    let key = (channel.to_string(), ts.to_string());
    if let Some(url) = app.needs_you.slack.permalinks.get(&key) {
        return Some(url.clone());
    }
    ask(
        app,
        Lookup::Permalink {
            channel: key.0,
            ts: key.1,
        },
    );
    None
}

/// The banners owed with no name still asked for: `(summary, body)` for
/// `alerts::notify_text` — `Ada · direct message` over the first line.
pub fn due_banners(app: &mut App) -> Vec<(String, String)> {
    let slack = &mut app.needs_you.slack;
    if slack.banners.is_empty() {
        return Vec::new();
    }
    let kept = &app.needs_you.store.slack;
    let (due, owed): (Vec<Banner>, Vec<Banner>) = std::mem::take(&mut slack.banners)
        .into_iter()
        .partition(|b| !slack.asking.contains(&b.user) && !slack.asking.contains(&b.channel));
    slack.banners = owed;
    due.iter()
        .map(|b| {
            let summary = format!("{} · {}", sender(kept, &b.user), place(kept, b));
            (summary, first_line(&b.text, &kept.users))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "U0ME";

    fn who() -> Who {
        Who {
            me: Some(ME.into()),
            phone: Some("D0PHONE".into()),
            settings: SlackSettings::default(),
        }
    }

    fn msg(channel: &str, kind: &str, user: &str, text: &str, ts: &str) -> Message {
        Message {
            channel: channel.into(),
            channel_type: kind.into(),
            user: Some(user.into()),
            text: text.into(),
            ts: ts.into(),
            thread_ts: None,
            subtype: None,
        }
    }

    fn reply(m: Message, thread: &str) -> Message {
        Message {
            thread_ts: Some(thread.into()),
            ..m
        }
    }

    fn whys(kept: &Kept) -> Vec<Why> {
        kept.waiting.iter().map(|w| w.why).collect()
    }

    /// The daemon's `inbox_event`, as the TUI receives it.
    fn forwarded(event: &Value) -> ServerEvent {
        let text = |key: &str| event[key].as_str().map(str::to_string);
        ServerEvent::SlackMessage {
            channel: text("channel").unwrap(),
            channel_type: text("channel_type").unwrap_or_default(),
            user: text("user"),
            text: text("text").unwrap_or_default(),
            ts: text("ts").unwrap(),
            thread_ts: text("thread_ts"),
            subtype: text("subtype"),
            event_json: event.to_string(),
        }
    }

    /// Slack's own `message.channels` example
    /// (api.slack.com/events/message.channels), with a mention of you,
    /// all the way through to a card on the board.
    #[test]
    fn a_real_message_event_becomes_a_card() {
        let mut app = App::new();
        app.slack_status.user_id = Some(ME.into());
        let event = serde_json::json!({
            "type": "message",
            "channel": "C024BE91L",
            "user": "U2147483697",
            "text": "<@U0ME> Live long and prospect.\nSecond line",
            "ts": "1355517523.000005",
            "event_ts": "1355517523.000005",
            "channel_type": "channel"
        });
        let (m, json) = Message::of(forwarded(&event)).unwrap();
        heard(&mut app, m, &json);
        app.needs_you
            .store
            .slack
            .users
            .insert("U2147483697".into(), "Spock".into());
        app.needs_you
            .store
            .slack
            .channels
            .insert("C024BE91L".into(), "bridge".into());
        let cards = super::super::cards(&app);
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert_eq!(card.section, Section::Slack);
        assert_eq!(card.title, "Spock: @someone Live long and prospect.");
        assert_eq!(card.sub, "#bridge · mentions you");
        assert_eq!(card.since, time_of("1355517523.000005"));
        assert_eq!(
            card.target,
            CardTarget::SlackMessage {
                channel: "C024BE91L".into(),
                root: Some("1355517523.000005".into()),
                ts: "1355517523.000005".into(),
            }
        );
        // Its banner is due at once: no ask is out for the name.
        assert_eq!(
            due_banners(&mut app),
            [(
                "Spock · #bridge · mentions you".to_string(),
                "@someone Live long and prospect.".to_string()
            )]
        );
    }

    #[test]
    fn each_rule_makes_a_card() {
        let who = Who {
            settings: SlackSettings {
                channels: vec!["#deploys".into()],
                keywords: vec!["Outage".into()],
                ..SlackSettings::default()
            },
            ..who()
        };
        let mut kept = Kept::default();
        kept.channels.insert("C2".into(), "deploys".into());
        kept.threads.push(("C1".into(), "50.000000".into()));
        let cases = [
            (msg("D1", "im", "U1", "hi", "100.000001"), Why::Direct),
            (msg("G1", "mpim", "U1", "hi all", "100.000002"), Why::Direct),
            (
                msg("C1", "channel", "U1", "ping <@U0ME|me>", "100.000003"),
                Why::Mention,
            ),
            (
                reply(
                    msg("C1", "channel", "U1", "and?", "100.000004"),
                    "50.000000",
                ),
                Why::Thread,
            ),
            (
                msg("C2", "channel", "U1", "shipped", "100.000005"),
                Why::Channel,
            ),
            (
                msg("C3", "group", "U1", "an OUTAGE upstream", "100.000006"),
                Why::Keyword,
            ),
        ];
        for (m, expect) in cases {
            assert_eq!(apply(&mut kept, &m, &who), Some(expect), "{m:?}");
        }
        assert_eq!(kept.waiting.len(), 6);
        assert_eq!(kept.waiting[5].keyword.as_deref(), Some("Outage"));
        // Nothing for you: another channel, the phone's DM, an edit, a bot.
        let nothing = [
            msg("C9", "channel", "U1", "lunch?", "101.000000"),
            msg("D0PHONE", "im", "U1", "phone", "101.000001"),
            Message {
                subtype: Some("message_changed".into()),
                ..msg("D1", "im", "U1", "edit", "101.000002")
            },
            Message {
                user: None,
                ..msg("D1", "im", "U1", "bot", "101.000003")
            },
        ];
        for m in nothing {
            assert_eq!(apply(&mut kept, &m, &who), None, "{m:?}");
        }
    }

    #[test]
    fn a_rule_turned_off_hides_its_cards() {
        let mut kept = Kept::default();
        apply(&mut kept, &msg("D1", "im", "U1", "hi", "1.0"), &who());
        let off = SlackSettings {
            dms: false,
            ..SlackSettings::default()
        };
        assert!(wanted(&kept, &kept.waiting[0], &who().settings));
        assert!(!wanted(&kept, &kept.waiting[0], &off));
    }

    #[test]
    fn your_reply_clears_the_card_and_joins_you_to_the_thread() {
        let who = who();
        let mut kept = Kept::default();
        // A DM: your later top-level reply clears it.
        apply(&mut kept, &msg("D1", "im", "U1", "hi", "10.000000"), &who);
        apply(&mut kept, &msg("D1", "im", ME, "older", "9.000000"), &who);
        assert_eq!(kept.waiting.len(), 1, "an earlier message answers nothing");
        apply(&mut kept, &msg("D1", "im", ME, "hey", "11.000000"), &who);
        assert!(kept.waiting.is_empty());
        // A mention: your reply in its thread clears it, and later replies
        // there are a thread you're in.
        let mention = msg("C1", "channel", "U1", "<@U0ME> look", "20.000000");
        apply(&mut kept, &mention, &who);
        apply(
            &mut kept,
            &reply(msg("C1", "channel", ME, "on it", "21.000000"), "20.000000"),
            &who,
        );
        assert!(kept.waiting.is_empty());
        let back = reply(
            msg("C1", "channel", "U2", "thanks", "22.000000"),
            "20.000000",
        );
        assert_eq!(apply(&mut kept, &back, &who), Some(Why::Thread));
        // A top-level post of yours doesn't answer a thread.
        apply(
            &mut kept,
            &msg("C1", "channel", ME, "news", "23.000000"),
            &who,
        );
        assert_eq!(whys(&kept), [Why::Thread]);
        // A reply in the thread does.
        apply(
            &mut kept,
            &reply(msg("C1", "channel", ME, "np", "24.000000"), "20.000000"),
            &who,
        );
        assert!(kept.waiting.is_empty());
    }

    #[test]
    fn a_message_heard_twice_is_one_card_and_one_banner() {
        let mut app = App::new();
        app.slack_status.user_id = Some(ME.into());
        let m = msg("D1", "im", "U1", "hi", "1.000001");
        heard(&mut app, m.clone(), "{}");
        heard(&mut app, m, "{}");
        assert_eq!(app.needs_you.store.slack.waiting.len(), 1);
        assert_eq!(due_banners(&mut app).len(), 1);
        // A later message in the same DM moves the one card on.
        heard(
            &mut app,
            msg("D1", "im", "U1", "still there?", "2.000000"),
            "{}",
        );
        let kept = &app.needs_you.store.slack;
        assert_eq!(kept.waiting.len(), 1);
        assert_eq!(kept.waiting[0].text, "still there?");
        // The same message again, as a store kept since a restart has it.
        let mut kept = kept.clone();
        let again = msg("D1", "im", "U1", "still there?", "2.000000");
        assert_eq!(apply(&mut kept, &again, &who()), None);
    }

    #[test]
    fn a_name_in_the_event_is_used_and_the_banner_follows_the_setting() {
        let mut app = App::new();
        app.slack_status.user_id = Some(ME.into());
        app.needs_you.slack.settings.banner = BannerWhen::DmsAndMentions;
        let json = r#"{"user_profile":{"display_name":"","real_name":"Ada Lovelace"}}"#;
        heard(&mut app, msg("D1", "im", "U1", "hi &amp; bye", "1.0"), json);
        app.needs_you.slack.settings.keywords.push("deploy".into());
        heard(
            &mut app,
            msg("C1", "channel", "U2", "deploy done", "2.0"),
            "{}",
        );
        let cards = super::super::cards(&app);
        let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Ada Lovelace: hi & bye", "Someone: deploy done"]);
        assert_eq!(cards[1].sub, "a channel · says deploy");
        let banners = due_banners(&mut app);
        assert_eq!(
            banners,
            [(
                "Ada Lovelace · direct message".to_string(),
                "hi & bye".to_string()
            )]
        );
    }

    #[test]
    fn the_store_keeps_the_cards_across_a_restart() {
        let mut kept = Kept::default();
        apply(&mut kept, &msg("D1", "im", "U1", "hi", "1.0"), &who());
        kept.users.insert("U1".into(), "Ada".into());
        let mut store = super::super::model::Store::default();
        store.slack = kept;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("needs-you.json");
        store.save(&path).unwrap();
        let back = super::super::model::Store::load(&path);
        assert_eq!(back, store);
        store.slack = Kept::default();
        assert_ne!(back, store);
    }

    #[test]
    fn slack_markup_reads_as_plain_text() {
        let mut users = BTreeMap::new();
        users.insert("U1".into(), "ada".into());
        assert_eq!(
            plain(
                "<@U1> see <#C1|eng>, <https://x.dev|the doc> &amp; <https://y.dev> <!here>",
                &users
            ),
            "@ada see #eng, the doc & https://y.dev @here"
        );
        assert_eq!(first_line("\n  \n second\nthird", &users), "second");
        assert_eq!(form_escape("1.0 a&b"), "1.0%20a%26b");
    }
}
