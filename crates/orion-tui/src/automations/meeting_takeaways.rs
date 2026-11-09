//! MEETING TAKEAWAYS: a Teams meeting's transcript, read for what was
//! agreed and placed in your todos for you to check.
//!
//! * **Watching.** Every online meeting on the calendar (2D's poll,
//!   `App::microsoft.events`) is noted as it is seen; once one has ended it
//!   is kept ([`Kept`], `<data dir>/automations/meeting-takeaways.json`)
//!   and its transcript looked for every [`interval`] (2 minutes,
//!   `ORION_MEETING_TAKEAWAYS_POLL_SECS`) for up to [`WINDOW`] after it
//!   ended ([`crate::microsoft::transcripts`]). Nothing is asked while
//!   Microsoft 365 is not signed in or **Takeaways from Teams
//!   transcripts** is off.
//! * **Extracting.** One [`crate::oneshot::run_json`] ask, told by the
//!   **Meeting takeaways** preset plus its extra steer ([`super::compose`]),
//!   with the transcript, who you are, and every project with **Link to
//!   meeting takeaways** on — its **What it is**, its todo groups and its
//!   open Linear tickets — and Blue sky's groups ([`context`]). It answers
//!   [`RawTakeaway`]s; the ones that name you start accepted.
//! * **Placing.** Needs you's MEETINGS card and the panel's MEETINGS row
//!   open the TAKEAWAYS SCREEN (`crate::takeaways_view`), where Enter adds
//!   the accepted ones to their groups ([`add_to_todos`], [`place`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

use super::{Automation, Dot, Panel, PanelList, PanelRow, Runner, Tone};
use crate::app::App;
use crate::microsoft::calendar::Event;
use crate::todos::{store, TodoFile};
use orion_core::ClientRequest;

/// What the **Meeting takeaways** preset says while it holds none of the
/// user's.
pub const PRESET_TEXT: &str = "Read the meeting transcript below and pull out its takeaways: each thing someone agreed to do or follow up on, one short line each in the imperative, with who owns it. Leave out discussion that ended in nothing to do. Put each one where it belongs: the project whose description and groups fit it, in the group that fits, or a new group when none does. An idea for something new that belongs to no project goes to Blue sky.";

/// The shape the answer must take: orion reads it, so it is not the
/// preset's to change.
const SYSTEM: &str = "You turn a meeting transcript into takeaways for a developer's todo list. \
Answer {\"takeaways\": [...]}, each takeaway an object: \
\"text\" (the thing to do, one short line), \
\"owner_initials\" (who owns it, like \"AL\"; empty when nobody does), \
\"is_mine\" (true when it is owned by, or asked of, the person named as You), \
\"project\" (one of the listed project names, \"blue sky\", or null when none fits), \
\"group_path\" (one or two group names from that project's groups, outermost first; a new name when none fits; [] for none), \
\"priority\" (0 none, 1 urgent, 2 high, 3 medium, 4 low), \
\"ticket\" (the id of a listed open ticket it is about, else null). \
Use only names and ticket ids from the lists given.";

/// How long after a meeting ends its transcript is looked for.
pub const WINDOW: chrono::Duration = chrono::Duration::minutes(60);
/// How often, out of the box.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(2 * 60);
/// How long the model may take over one meeting.
const TIMEOUT: Duration = Duration::from_secs(240);
/// How long a meeting is kept.
const KEEP: chrono::Duration = chrono::Duration::days(14);
/// Of each project's open Linear tickets, how many the model is shown.
const TICKETS: usize = 40;
/// The most of a transcript sent: a long meeting's tail is cut.
const TRANSCRIPT_MAX: usize = 120_000;
/// The file the meetings are kept in, under the data dir.
const FILE: &str = "meeting-takeaways.json";

/// The poll's cadence: `ORION_MEETING_TAKEAWAYS_POLL_SECS` when set, `None`
/// when it is `0` (off: the e2e tests, the screenshots); else
/// [`DEFAULT_INTERVAL`].
pub fn interval() -> Option<Duration> {
    orion_core::env::secs_override(
        orion_core::env::MEETING_TAKEAWAYS_POLL_SECS,
        DEFAULT_INTERVAL,
    )
}

// ---- what is kept ----

/// Where a meeting is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "stage", content = "detail")]
pub enum Stage {
    /// Ended; its transcript is being looked for.
    Waiting,
    /// Its transcript is in, and the model is reading it.
    Reading,
    /// Read: takeaways wait for you to place them.
    ToPlace,
    /// Placed: this many went into your todos.
    Added(usize),
    /// Read: none of it was yours.
    NothingForYou,
    /// No transcript came within [`WINDOW`].
    NoTranscript,
    /// It couldn't be read, and why.
    Failed(String),
}

/// One takeaway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Takeaway {
    pub text: String,
    /// The owner's initials, `AL`; empty for nobody.
    #[serde(default)]
    pub owner: String,
    /// It names you.
    #[serde(default)]
    pub mine: bool,
    /// ✓: it goes into your todos.
    #[serde(default)]
    pub accepted: bool,
    /// The list it goes in: a project's checkout, or Blue sky's key
    /// ([`store::blue_sky_key`]). None: nowhere yet.
    #[serde(default)]
    pub dest: Option<PathBuf>,
    /// The group in that list, outermost first; empty is its Inbox.
    #[serde(default)]
    pub group_path: Vec<String>,
    /// Linear's scale: 0 none, 1 urgent … 4 low.
    #[serde(default)]
    pub priority: u8,
    /// An open Linear ticket it is about: `ENG-412`.
    #[serde(default)]
    pub ticket: Option<String>,
}

impl Takeaway {
    /// The text it goes into your todos as: the ticket's id after it.
    pub fn item_text(&self) -> String {
        match &self.ticket {
            Some(id) => format!("{} ({id})", self.text),
            None => self.text.clone(),
        }
    }
}

/// One meeting that ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meeting {
    /// The calendar event's id.
    pub id: String,
    pub subject: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub join_url: String,
    #[serde(flatten)]
    pub stage: Stage,
    #[serde(default)]
    pub takeaways: Vec<Takeaway>,
    /// The transcript as text, once in: what a retry reads again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub transcript: String,
    /// Your name as Microsoft 365 has it.
    #[serde(default)]
    pub me: String,
    /// The last look's failure, while looking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Meeting {
    /// A meeting the calendar showed, ended.
    pub fn ended(event: &Event) -> Self {
        Self {
            id: event.id.clone(),
            subject: event.subject.clone(),
            start: event.start,
            end: event.end,
            join_url: event.join_url.clone().unwrap_or_default(),
            stage: Stage::Waiting,
            takeaways: Vec::new(),
            transcript: String::new(),
            me: String::new(),
            last_error: None,
        }
    }

    /// The accepted takeaways: what Enter adds.
    pub fn accepted(&self) -> usize {
        self.takeaways.iter().filter(|t| t.accepted).count()
    }

    /// The takeaways that name you.
    pub fn mine(&self) -> usize {
        self.takeaways.iter().filter(|t| t.mine).count()
    }

    /// `Thu 10:00 to 10:32`, local.
    pub fn when(&self) -> String {
        let (start, end) = (
            self.start.with_timezone(&Local),
            self.end.with_timezone(&Local),
        );
        format!("{} to {}", start.format("%a %H:%M"), end.format("%H:%M"))
    }

    /// The panel's state and its colour.
    pub fn state(&self) -> (String, Tone) {
        match &self.stage {
            Stage::Waiting => ("waiting for the transcript".into(), Tone::Working),
            Stage::Reading => ("reading".into(), Tone::Working),
            Stage::ToPlace => (format!("{} to place", self.accepted()), Tone::Waiting),
            Stage::Added(n) => (format!("{n} added"), Tone::Done),
            Stage::NothingForYou => ("nothing for you".into(), Tone::Quiet),
            Stage::NoTranscript => ("no transcript".into(), Tone::Quiet),
            Stage::Failed(_) => ("couldn't read it".into(), Tone::Waiting),
        }
    }
}

/// `meeting-takeaways.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    #[serde(default)]
    pub meetings: Vec<Meeting>,
}

/// What the engine keeps for meeting takeaways (`Engine::meetings`).
#[derive(Debug, Default)]
pub struct Shared {
    pub kept: Kept,
    /// Where [`Kept`] is saved; None in the unit tests.
    path: Option<PathBuf>,
    /// The online meetings the calendar has shown, by id, so one that ends
    /// after the calendar stopped listing it is still caught.
    watching: HashMap<String, Event>,
    /// Meetings with a look or a read out.
    in_flight: HashSet<String>,
    /// When each waiting meeting is looked at next.
    next_look: HashMap<String, DateTime<Utc>>,
}

impl Shared {
    pub fn meeting(&self, id: &str) -> Option<&Meeting> {
        self.kept.meetings.iter().find(|m| m.id == id)
    }

    pub fn meeting_mut(&mut self, id: &str) -> Option<&mut Meeting> {
        self.kept.meetings.iter_mut().find(|m| m.id == id)
    }

    /// Write [`Kept`] to its file.
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let written = serde_json::to_vec_pretty(&self.kept)
            .map_err(std::io::Error::other)
            .and_then(|bytes| orion_core::settings::write_atomic(path, &bytes));
        if let Err(err) = written {
            tracing::warn!("meeting takeaways: writing {}: {err}", path.display());
        }
    }
}

/// The meetings from the last run, as orion starts. A read cut off by the
/// quit is asked again.
pub fn load(app: &mut App) {
    let path = orion_core::paths::data_dir().join("automations").join(FILE);
    load_from(app, path);
}

fn load_from(app: &mut App, path: PathBuf) {
    let kept: Kept = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let shared = &mut app.automations.meetings;
    shared.kept = kept;
    shared.path = Some(path);
}

/// The meetings as the panel lists them: the latest first.
pub fn listed(app: &App) -> Vec<&Meeting> {
    let mut meetings: Vec<&Meeting> = app.automations.meetings.kept.meetings.iter().collect();
    meetings.sort_by_key(|m| std::cmp::Reverse(m.end));
    meetings
}

// ---- the runner ----

/// Meeting takeaways' runner. What it keeps lives on the engine
/// ([`Shared`]), where the answers land.
pub struct MeetingTakeaways;

/// Whether the takeaways may ask Microsoft anything: signed in, and
/// **Takeaways from Teams transcripts** on.
fn connected(app: &App) -> bool {
    let ms = crate::config::Config::load().microsoft;
    ms.takeaways && app.microsoft.connected(&ms)
}

impl Runner for MeetingTakeaways {
    fn automation(&self) -> Automation {
        Automation::MeetingTakeaways
    }

    /// Note the online meetings the calendar shows; keep each one that
    /// ended; look for its transcript when due; read one that came in.
    fn tick(&mut self, app: &mut App, _out: &mut Vec<ClientRequest>) {
        let Some(every) = interval() else {
            return;
        };
        if !connected(app) {
            return;
        }
        let now = Utc::now();
        watch(app, now);
        let ids: Vec<String> = app
            .automations
            .meetings
            .kept
            .meetings
            .iter()
            .filter(|m| matches!(m.stage, Stage::Waiting | Stage::Reading))
            .map(|m| m.id.clone())
            .collect();
        for id in ids {
            let shared = &mut app.automations.meetings;
            if shared.in_flight.contains(&id) {
                continue;
            }
            let due = shared.next_look.get(&id).is_none_or(|at| *at <= now);
            let Some(meeting) = shared.meeting_mut(&id) else {
                continue;
            };
            if meeting.stage == Stage::Reading {
                extract(app, &id);
            } else if now > meeting.end + WINDOW {
                meeting.stage = match meeting.last_error.take() {
                    Some(why) => Stage::Failed(why),
                    None => Stage::NoTranscript,
                };
                shared.save();
            } else if due {
                let next = now + chrono::Duration::from_std(every).unwrap_or(WINDOW);
                shared.next_look.insert(id.clone(), next);
                look(app, &id);
            }
        }
    }

    fn panel(&self, app: &App) -> Panel {
        let meetings = listed(app);
        let rows: Vec<PanelRow> = meetings
            .iter()
            .map(|m| {
                let (state, tone) = m.state();
                PanelRow {
                    text: format!("{} · {}", m.subject, m.when()),
                    state,
                    tone,
                    session: None,
                }
            })
            .collect();
        let to_place: usize = meetings
            .iter()
            .filter(|m| m.stage == Stage::ToPlace)
            .map(|m| m.accepted())
            .sum();
        let busy = meetings
            .iter()
            .filter(|m| matches!(m.stage, Stage::Waiting | Stage::Reading))
            .count();
        let (dot, state) = if to_place > 0 {
            (Dot::Waiting, format!("{to_place} to place"))
        } else if busy > 0 {
            (Dot::Working, format!("reading {busy}"))
        } else if app.microsoft.email.is_none() {
            (Dot::Idle, "not connected".into())
        } else {
            (Dot::Idle, "idle".into())
        };
        Panel {
            dot,
            state,
            lists: vec![PanelList {
                title: "MEETINGS",
                empty: "No meetings yet · your Teams meetings show here as they end",
                rows,
            }],
        }
    }

    fn opens_rows(&self) -> bool {
        true
    }

    /// Enter on a MEETINGS row: its takeaways, or — when it couldn't be
    /// read — a read again.
    fn open_row(&mut self, app: &mut App, index: usize) {
        let Some(id) = listed(app).get(index).map(|m| m.id.clone()) else {
            return;
        };
        open(app, &id, true);
    }
}

/// Enter on a meeting — its Needs you card, its row on the panel: the
/// TAKEAWAYS SCREEN; one that couldn't be read is read again.
pub fn open(app: &mut App, id: &str, back: bool) {
    let Some(meeting) = app.automations.meetings.meeting(id) else {
        return;
    };
    match &meeting.stage {
        Stage::Failed(_) => retry(app, id),
        _ if meeting.takeaways.is_empty() => {
            let (state, _) = meeting.state();
            app.flash = Some(crate::flash::Flash::done(format!(
                "{}: {state}",
                meeting.subject
            )));
        }
        _ => crate::takeaways_view::open(app, id, back),
    }
}

/// A meeting that couldn't be read, asked again: read once more when its
/// transcript is in, else looked for anew.
pub fn retry(app: &mut App, id: &str) {
    let shared = &mut app.automations.meetings;
    let Some(meeting) = shared.meeting_mut(id) else {
        return;
    };
    let reading = !meeting.transcript.is_empty();
    meeting.stage = if reading {
        Stage::Reading
    } else {
        Stage::Waiting
    };
    meeting.last_error = None;
    // A look that ran out of time gets the window again.
    if !reading && Utc::now() > meeting.end + WINDOW {
        meeting.end = Utc::now();
    }
    let subject = meeting.subject.clone();
    shared.next_look.remove(id);
    shared.save();
    app.flash = Some(crate::flash::Flash::done(format!(
        "Meeting takeaways: reading {subject} again"
    )));
    if reading {
        extract(app, id);
    }
    app.dirty = true;
}

/// The calendar's online meetings noted; each one that has ended kept.
fn watch(app: &mut App, now: DateTime<Utc>) {
    let shared = &mut app.automations.meetings;
    for event in &app.microsoft.events {
        if event.online && event.join_url.is_some() {
            shared
                .watching
                .entry(event.id.clone())
                .or_insert_with(|| event.clone());
        }
    }
    let ended: Vec<Event> = shared
        .watching
        .values()
        .filter(|e| e.end <= now)
        .cloned()
        .collect();
    if ended.is_empty() {
        return;
    }
    for event in ended {
        shared.watching.remove(&event.id);
        if shared.meeting(&event.id).is_none() {
            shared.kept.meetings.push(Meeting::ended(&event));
        }
    }
    shared.kept.meetings.retain(|m| now - m.end < KEEP);
    shared.save();
}

/// What lands back on the loop.
#[derive(Debug)]
pub enum Answer {
    /// A look for a transcript: the WebVTT and your name, None while there
    /// is none yet.
    Looked {
        id: String,
        found: Result<Option<(String, String)>, String>,
    },
    /// A read: the takeaways, mapped onto `places`.
    Read {
        id: String,
        reply: Result<Vec<RawTakeaway>, String>,
        places: Vec<Place>,
    },
}

/// Look for `id`'s transcript off the loop.
fn look(app: &mut App, id: &str) {
    let Some(tx) = app.automations.tx.clone() else {
        return;
    };
    let shared = &mut app.automations.meetings;
    let Some(meeting) = shared.meeting(id) else {
        return;
    };
    let (join_url, start) = (meeting.join_url.clone(), meeting.start);
    shared.in_flight.insert(id.to_string());
    let ms = crate::config::Config::load().microsoft;
    let id = id.to_string();
    tokio::spawn(async move {
        let found = tokio::task::spawn_blocking(move || {
            let vtt = crate::microsoft::transcripts::look(&ms, &join_url, start)?;
            Ok(vtt.map(|vtt| {
                let me = crate::microsoft::graph::display_name(&ms).unwrap_or_default();
                (vtt, me)
            }))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let _ = tx.send(super::Answer::MeetingTakeaways(Answer::Looked {
            id,
            found,
        }));
    });
}

/// An answer, landed.
pub fn land(app: &mut App, answer: Answer) {
    let shared = &mut app.automations.meetings;
    match answer {
        Answer::Looked { id, found } => {
            shared.in_flight.remove(&id);
            let Some(meeting) = shared.meeting_mut(&id) else {
                return;
            };
            match found {
                Ok(None) => meeting.last_error = None,
                Err(why) => meeting.last_error = Some(why),
                Ok(Some((vtt, me))) => {
                    meeting.transcript = crate::microsoft::transcripts::vtt_to_text(&vtt);
                    meeting.me = me;
                    meeting.last_error = None;
                    meeting.stage = Stage::Reading;
                    shared.save();
                    extract(app, &id);
                }
            }
        }
        Answer::Read { id, reply, places } => {
            shared.in_flight.remove(&id);
            let Some(meeting) = shared.meeting_mut(&id) else {
                return;
            };
            read(meeting, reply, &places);
            shared.save();
            crate::needs_you::refresh(app);
        }
    }
}

/// A read landed on its meeting: its takeaways, or why there are none.
pub fn read(meeting: &mut Meeting, reply: Result<Vec<RawTakeaway>, String>, places: &[Place]) {
    match reply {
        Ok(raw) => {
            meeting.takeaways = takeaways_from(raw, places);
            meeting.stage = match meeting.mine() {
                0 => Stage::NothingForYou,
                _ => Stage::ToPlace,
            };
        }
        Err(why) => meeting.stage = Stage::Failed(why),
    }
}

// ---- the read ----

/// One list a takeaway can go in, as the model is told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub name: String,
    /// The list's key: a checkout, or Blue sky's.
    pub key: PathBuf,
    /// **What it is**; empty for Blue sky.
    pub about: String,
    /// Its groups, `Outer › Inner`.
    pub groups: Vec<String>,
    /// Its open Linear tickets: id and title.
    pub tickets: Vec<(String, String)>,
}

/// One takeaway as the model answers it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RawTakeaway {
    pub text: String,
    #[serde(default)]
    pub owner_initials: String,
    #[serde(default)]
    pub is_mine: bool,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub group_path: Vec<String>,
    #[serde(default)]
    pub priority: u8,
    #[serde(default)]
    pub ticket: Option<String>,
}

/// The answer: `{"takeaways": […]}`, or the list bare.
#[derive(Deserialize)]
#[serde(untagged)]
enum Reply {
    Wrapped { takeaways: Vec<RawTakeaway> },
    Bare(Vec<RawTakeaway>),
}

impl Reply {
    fn takeaways(self) -> Vec<RawTakeaway> {
        match self {
            Reply::Wrapped { takeaways } | Reply::Bare(takeaways) => takeaways,
        }
    }
}

/// The projects with **Link to meeting takeaways** on, in the tree's order:
/// their name and checkout.
pub fn linked(app: &App) -> Vec<(String, PathBuf)> {
    let cfg = crate::config::Config::load();
    app.tree
        .projects
        .iter()
        .filter(|p| cfg.project(&p.repo_path).takeaways)
        .map(|p| (p.name.clone(), p.repo_path.clone()))
        .collect()
}

/// A list's groups as `Outer › Inner`, in the order they stand.
pub fn group_paths(file: &TodoFile) -> Vec<String> {
    let mut out = Vec::new();
    for top in file.subgroups(None) {
        out.push(top.name.clone());
        for inner in file.subgroups(Some(top.id)) {
            out.push(format!("{} › {}", top.name, inner.name));
        }
    }
    out
}

/// Every place a takeaway can go: the linked projects, then Blue sky —
/// each with its groups read in, its line and its open tickets.
pub fn places(app: &mut App) -> Vec<Place> {
    let mut places = Vec::new();
    for (name, repo) in linked(app) {
        let about = crate::projects::about(app, &repo);
        let project = app
            .tree
            .projects
            .iter()
            .find(|p| p.repo_path == repo)
            .map(|p| p.id.clone());
        let tickets = project
            .and_then(|id| app.linear.get(&id))
            .map(|list| {
                list.list
                    .iter()
                    .filter(|i| !matches!(i.status_type.as_str(), "completed" | "canceled"))
                    .take(TICKETS)
                    .map(|i| (i.identifier.clone(), i.title.clone()))
                    .collect()
            })
            .unwrap_or_default();
        places.push(Place {
            name,
            about,
            groups: groups_of(app, &repo),
            tickets,
            key: repo,
        });
    }
    let key = store::blue_sky_key();
    places.push(Place {
        name: store::BLUE_SKY_KEY.into(),
        about: String::new(),
        groups: groups_of(app, &key),
        tickets: Vec::new(),
        key,
    });
    places
}

fn groups_of(app: &mut App, key: &Path) -> Vec<String> {
    crate::todos::view::load_list(app, key.to_path_buf());
    app.todos.get(key).map(group_paths).unwrap_or_default()
}

/// What the model is given at the preset's `{task}`.
pub fn context(transcript: &str, me: &str, email: &str, places: &[Place]) -> String {
    let mut out = String::new();
    out.push_str(&match (me.trim(), email.trim()) {
        ("", email) => format!("You: {email}\n"),
        (me, "") => format!("You: {me}\n"),
        (me, email) => format!("You: {me} <{email}>\n"),
    });
    out.push_str("\nWhere takeaways can go:\n");
    for place in places {
        match store::is_blue_sky(&place.key) {
            true => out.push_str(&format!(
                "- {}: ideas for something new that belong to no project\n",
                place.name
            )),
            false => match place.about.trim() {
                "" => out.push_str(&format!("- {}\n", place.name)),
                about => out.push_str(&format!("- {}: {about}\n", place.name)),
            },
        }
        let groups = match place.groups.is_empty() {
            true => "(none yet)".to_string(),
            false => place.groups.join("; "),
        };
        out.push_str(&format!("  Groups: {groups}\n"));
        if !place.tickets.is_empty() {
            let tickets: Vec<String> = place
                .tickets
                .iter()
                .map(|(id, title)| format!("{id} {title}"))
                .collect();
            out.push_str(&format!("  Open tickets: {}\n", tickets.join("; ")));
        }
    }
    let mut transcript = transcript.trim();
    if transcript.len() > TRANSCRIPT_MAX {
        let mut cut = TRANSCRIPT_MAX;
        while !transcript.is_char_boundary(cut) {
            cut -= 1;
        }
        transcript = &transcript[..cut];
    }
    out.push_str(&format!("\nTranscript:\n{transcript}\n"));
    out
}

/// What the model answered, made takeaways: a project named by a place's
/// name (any case), a group path of at most two names, a priority on
/// Linear's scale, a ticket only if the place listed it. The ones that
/// name you start accepted.
pub fn takeaways_from(raw: Vec<RawTakeaway>, places: &[Place]) -> Vec<Takeaway> {
    raw.into_iter()
        .filter(|t| !t.text.trim().is_empty())
        .map(|t| {
            let place = t.project.as_deref().and_then(|name| {
                places
                    .iter()
                    .find(|p| p.name.trim().eq_ignore_ascii_case(name.trim()))
            });
            let ticket = t.ticket.as_deref().map(str::trim).and_then(|id| {
                place?
                    .tickets
                    .iter()
                    .find(|(known, _)| known.eq_ignore_ascii_case(id))
                    .map(|(known, _)| known.clone())
            });
            Takeaway {
                text: t.text.trim().to_string(),
                owner: t.owner_initials.trim().to_uppercase(),
                mine: t.is_mine,
                accepted: t.is_mine,
                dest: place.map(|p| p.key.clone()),
                group_path: match place {
                    Some(_) => clean_path(&t.group_path),
                    None => Vec::new(),
                },
                priority: if t.priority <= 4 { t.priority } else { 0 },
                ticket,
            }
        })
        .collect()
}

/// At most two group names, trimmed, blanks gone.
pub fn clean_path(path: &[String]) -> Vec<String> {
    path.iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .take(2)
        .collect()
}

/// Read meeting `id`'s transcript off the loop.
pub fn extract(app: &mut App, id: &str) {
    let Some(tx) = app.automations.tx.clone() else {
        return;
    };
    let Some(meeting) = app.automations.meetings.meeting(id) else {
        return;
    };
    let (transcript, me) = (meeting.transcript.clone(), meeting.me.clone());
    let email = app.microsoft.email.clone().unwrap_or_default();
    let places = places(app);
    let settings = &app.automations.settings;
    let preset = super::preset_of(settings, Automation::MeetingTakeaways);
    let extra = settings.of(Automation::MeetingTakeaways).extra_steer;
    let cfg = crate::config::Config::load();
    let account = crate::oneshot::account_for(&cfg, &preset);
    app.automations.meetings.in_flight.insert(id.to_string());
    let id = id.to_string();
    tokio::spawn(async move {
        // A project with no **What it is** yet has it written for this read.
        let mut places = places;
        for place in places.iter_mut() {
            if place.about.is_empty() && !store::is_blue_sky(&place.key) {
                place.about = crate::projects::write(&cfg, &place.key)
                    .await
                    .unwrap_or_default();
            }
        }
        let prompt = super::compose(&preset, &context(&transcript, &me, &email, &places), &extra);
        let reply = match account {
            None => Err("no Claude account is switched on".to_string()),
            Some(account) => crate::oneshot::run_json::<Reply>(crate::oneshot::Ask {
                prompt,
                system: SYSTEM.into(),
                account,
                model: preset
                    .model
                    .clone()
                    .unwrap_or_else(|| crate::week_review::DEFAULT_MODEL.into()),
                effort: preset
                    .effort
                    .clone()
                    .unwrap_or_else(|| crate::week_review::DEFAULT_EFFORT.into()),
                timeout: TIMEOUT,
            })
            .await
            .map(Reply::takeaways),
        };
        let _ = tx.send(super::Answer::MeetingTakeaways(Answer::Read {
            id,
            reply,
            places,
        }));
    });
}

// ---- placing ----

/// `name` among `parent`'s groups, in any case.
fn named(file: &TodoFile, parent: Option<u64>, name: &str) -> Option<u64> {
    file.subgroups(parent)
        .find(|g| g.name.trim().eq_ignore_ascii_case(name))
        .map(|g| g.id)
}

/// The group at `path` in `file` when it is there: the Inbox for no path.
pub fn find_group(file: &TodoFile, path: &[String]) -> Option<u64> {
    let path = clean_path(path);
    if path.is_empty() {
        return named(file, None, crate::todos::INBOX);
    }
    path.iter()
        .try_fold(None, |parent, name| named(file, parent, name).map(Some))
        .flatten()
}

/// The group at `path` in `file`, made where it is missing: the Inbox for
/// no path.
pub fn group_at(file: &mut TodoFile, path: &[String]) -> u64 {
    let mut parent = None;
    for name in clean_path(path) {
        let found = named(file, parent, &name);
        parent = Some(found.unwrap_or_else(|| file.add_group(parent, &name)));
    }
    parent.unwrap_or_else(|| file.inbox())
}

/// `takeaway` into `file`: at the end of its group, at its priority.
pub fn place(file: &mut TodoFile, takeaway: &Takeaway, today: chrono::NaiveDate) -> u64 {
    let group = group_at(file, &takeaway.group_path);
    let item = file.add_item(group, &takeaway.item_text(), today);
    if takeaway.priority != 0 {
        file.set_priority(item, takeaway.priority);
    }
    file.reveal(group);
    item
}

/// Enter on the TAKEAWAYS SCREEN: every accepted takeaway into its list,
/// each list saved, and the meeting marked added. An accepted one with
/// nowhere to go stops it: `Err` is its index, for the screen to ask where.
pub fn add_to_todos(app: &mut App, id: &str) -> Result<usize, usize> {
    let Some(meeting) = app.automations.meetings.meeting(id) else {
        return Ok(0);
    };
    if let Some(at) = meeting
        .takeaways
        .iter()
        .position(|t| t.accepted && t.dest.is_none())
    {
        return Err(at);
    }
    let accepted: Vec<Takeaway> = meeting
        .takeaways
        .iter()
        .filter(|t| t.accepted)
        .cloned()
        .collect();
    let today = crate::todos::today();
    let mut touched: Vec<PathBuf> = Vec::new();
    for takeaway in &accepted {
        let Some(key) = takeaway.dest.clone() else {
            continue;
        };
        crate::todos::view::load_list(app, key.clone());
        if let Some(file) = app.todos.get_mut(&key) {
            place(file, takeaway, today);
        }
        if !touched.contains(&key) {
            touched.push(key);
        }
    }
    for key in &touched {
        if let Some(file) = app.todos.get(key) {
            store::save(file);
        }
    }
    let shared = &mut app.automations.meetings;
    if let Some(meeting) = shared.meeting_mut(id) {
        meeting.stage = Stage::Added(accepted.len());
    }
    shared.save();
    crate::needs_you::refresh(app);
    Ok(accepted.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn place_of(name: &str, key: &str, groups: &[&str], tickets: &[(&str, &str)]) -> Place {
        Place {
            name: name.into(),
            key: PathBuf::from(key),
            about: String::new(),
            groups: groups.iter().map(|g| g.to_string()).collect(),
            tickets: tickets
                .iter()
                .map(|(id, t)| (id.to_string(), t.to_string()))
                .collect(),
        }
    }

    fn places() -> Vec<Place> {
        vec![
            place_of(
                "web",
                "/code/web",
                &["Onboarding", "Onboarding › Emails"],
                &[("ENG-412", "Welcome email copy")],
            ),
            place_of("billing-api", "/code/billing", &["Bugs"], &[]),
            place_of("blue sky", "blue sky", &["Product"], &[]),
        ]
    }

    fn meeting() -> Meeting {
        Meeting::ended(&Event {
            id: "evt-1".into(),
            subject: "Standup".into(),
            start: Utc.with_ymd_and_hms(2026, 11, 8, 10, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 11, 8, 10, 32, 0).unwrap(),
            online: true,
            join_url: Some("https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc".into()),
        })
    }

    #[test]
    fn a_reply_maps_onto_the_places_and_yours_start_accepted() {
        let reply = r#"{"takeaways": [
            {"text": " Write the welcome email ", "owner_initials": "al", "is_mine": true,
             "project": "Web", "group_path": ["Onboarding", "Emails", "Deeper"], "priority": 2,
             "ticket": "eng-412"},
            {"text": "Look at the 500", "owner_initials": "GH", "is_mine": false,
             "project": "billing-api", "group_path": ["Bugs"], "priority": 9, "ticket": "ENG-999"},
            {"text": "A habit tracker", "is_mine": true, "project": "blue sky", "group_path": []},
            {"text": "Somewhere unknown", "is_mine": true, "project": "mobile", "group_path": ["X"]},
            {"text": "   "}
        ]}"#;
        let raw: Reply = serde_json::from_str(reply).unwrap();
        let got = takeaways_from(raw.takeaways(), &places());
        assert_eq!(got.len(), 4, "the blank one goes");
        assert_eq!(got[0].text, "Write the welcome email");
        assert_eq!(got[0].owner, "AL");
        assert!(got[0].mine && got[0].accepted);
        assert_eq!(got[0].dest, Some(PathBuf::from("/code/web")));
        assert_eq!(
            got[0].group_path,
            ["Onboarding", "Emails"],
            "two deep at most"
        );
        assert_eq!(got[0].priority, 2);
        assert_eq!(got[0].ticket.as_deref(), Some("ENG-412"));
        assert_eq!(got[0].item_text(), "Write the welcome email (ENG-412)");
        assert!(!got[1].accepted, "not yours, not accepted");
        assert_eq!(got[1].priority, 0, "off the scale is none");
        assert_eq!(got[1].ticket, None, "a ticket it wasn't shown is dropped");
        assert_eq!(got[2].dest, Some(store::blue_sky_key()));
        assert_eq!(got[3].dest, None, "a project it wasn't given is nowhere");
        assert!(got[3].group_path.is_empty());

        let mut m = meeting();
        read(&mut m, Ok(raw_bare()), &places());
        assert_eq!(m.stage, Stage::NothingForYou);
    }

    fn raw_bare() -> Vec<RawTakeaway> {
        serde_json::from_str::<Reply>(r#"[{"text": "Theirs", "is_mine": false}]"#)
            .unwrap()
            .takeaways()
    }

    /// A reply that isn't the shape asked for is an error the board shows:
    /// a MEETINGS card saying it couldn't be read, Enter on which reads it
    /// again.
    #[test]
    fn a_bad_reply_is_an_error_card() {
        let bad: Result<Reply, String> =
            serde_json::from_str("{\"takeaways\": \"none\"}").map_err(|e| e.to_string());
        assert!(bad.is_err());
        let mut app = App::new();
        let mut m = meeting();
        read(
            &mut m,
            Err("the model's JSON did not read: expected a sequence".into()),
            &places(),
        );
        assert!(matches!(m.stage, Stage::Failed(_)));
        app.automations.meetings.kept.meetings.push(m);
        let cards = crate::needs_you::meetings::Meetings.cards(&app);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Standup");
        assert!(
            cards[0]
                .sub
                .starts_with("couldn't read the takeaways · the model's JSON"),
            "{}",
            cards[0].sub
        );
        assert_eq!(
            app.automations.meetings.meeting("evt-1").unwrap().state().0,
            "couldn't read it"
        );
    }

    use crate::needs_you::model::CardSource;

    #[test]
    fn placing_goes_into_an_existing_group_and_makes_a_new_one() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let mut file = TodoFile::new(Path::new("/code/web"));
        let onboarding = file.add_group(None, "Onboarding");
        let emails = file.add_group(Some(onboarding), "Emails");
        file.add_item(emails, "Old one", today);
        let take = |path: &[&str], text: &str, priority: u8| Takeaway {
            text: text.into(),
            owner: "AL".into(),
            mine: true,
            accepted: true,
            dest: Some(PathBuf::from("/code/web")),
            group_path: path.iter().map(|s| s.to_string()).collect(),
            priority,
            ticket: None,
        };
        let a = place(
            &mut file,
            &take(&["onboarding", "EMAILS"], "Welcome copy", 2),
            today,
        );
        assert_eq!(
            file.item(a).unwrap().group,
            emails,
            "found by name, any case"
        );
        assert_eq!(file.item(a).unwrap().priority, 2);
        let groups = file.groups.len();
        let b = place(
            &mut file,
            &take(&["Launch", "Docs"], "Write the guide", 0),
            today,
        );
        assert_eq!(file.groups.len(), groups + 2, "both levels made");
        assert_eq!(
            file.path_label(file.item(b).unwrap().group),
            "Launch › Docs"
        );
        let c = place(&mut file, &take(&[], "Loose end", 0), today);
        assert_eq!(
            file.path_label(file.item(c).unwrap().group),
            crate::todos::INBOX
        );
    }

    #[test]
    fn enter_adds_the_accepted_and_stops_at_one_with_nowhere_to_go() {
        let dir = tempfile::tempdir().unwrap();
        store::with_dir(dir.path().to_path_buf(), || {
            let mut app = App::new();
            let mut m = meeting();
            m.stage = Stage::ToPlace;
            m.takeaways = takeaways_from(
                serde_json::from_str::<Reply>(
                    r#"[{"text": "Welcome copy", "is_mine": true, "project": "web", "group_path": ["Onboarding"]},
                        {"text": "Theirs", "is_mine": false, "project": "web"},
                        {"text": "An idea", "is_mine": true, "project": "blue sky", "group_path": ["Product"]}]"#,
                )
                .unwrap()
                .takeaways(),
                &places(),
            );
            m.takeaways.push(Takeaway {
                dest: None,
                ..m.takeaways[0].clone()
            });
            app.automations.meetings.kept.meetings.push(m);
            assert_eq!(add_to_todos(&mut app, "evt-1"), Err(3));
            app.automations
                .meetings
                .meeting_mut("evt-1")
                .unwrap()
                .takeaways[3]
                .accepted = false;
            assert_eq!(add_to_todos(&mut app, "evt-1"), Ok(2));
            let web = &app.todos[Path::new("/code/web")];
            assert_eq!(web.items.len(), 1);
            assert_eq!(web.path_label(web.items[0].group), "Onboarding");
            let sky = &app.todos[&store::blue_sky_key()];
            assert_eq!(sky.items[0].text, "An idea");
            assert_eq!(
                app.automations.meetings.meeting("evt-1").unwrap().stage,
                Stage::Added(2)
            );
            // Saved, to be read back.
            let back = store::load(&store::blue_sky_key());
            assert_eq!(back.file.items.len(), 1);
        });
    }

    #[test]
    fn the_context_names_you_every_place_and_the_transcript() {
        let text = context(
            "Ada: I'll do it.",
            "Ada Lovelace",
            "ada@riplo.ai",
            &places(),
        );
        assert!(
            text.starts_with("You: Ada Lovelace <ada@riplo.ai>\n"),
            "{text}"
        );
        assert!(text.contains("- web\n  Groups: Onboarding; Onboarding › Emails\n  Open tickets: ENG-412 Welcome email copy\n"), "{text}");
        assert!(
            text.contains("- blue sky: ideas for something new"),
            "{text}"
        );
        assert!(text.ends_with("Transcript:\nAda: I'll do it.\n"), "{text}");
    }

    #[test]
    fn a_meeting_round_trips_through_its_file() {
        let mut m = meeting();
        m.stage = Stage::Added(2);
        let kept = Kept {
            meetings: vec![m, {
                let mut f = meeting();
                f.id = "evt-2".into();
                f.stage = Stage::Failed("403".into());
                f
            }],
        };
        let json = serde_json::to_string(&kept).unwrap();
        assert!(json.contains(r#""stage":"added","detail":2"#), "{json}");
        assert_eq!(serde_json::from_str::<Kept>(&json).unwrap(), kept);
    }

    /// Signed out of Microsoft 365, the left list's state is short enough
    /// to sit whole beside the name, like `off` and `idle`.
    #[test]
    fn signed_out_reads_not_connected() {
        let app = App::new();
        assert!(app.microsoft.email.is_none());
        let panel = MeetingTakeaways.panel(&app);
        assert_eq!(
            (panel.dot, panel.state.as_str()),
            (Dot::Idle, "not connected")
        );
    }

    /// The panel's MEETINGS rows say where each meeting is, the latest
    /// first, and the row's state is the one 5.5 names.
    #[test]
    fn the_panel_lists_meetings_with_their_state() {
        let mut app = App::new();
        let states = [
            Stage::ToPlace,
            Stage::Added(2),
            Stage::NothingForYou,
            Stage::NoTranscript,
        ];
        for (i, stage) in states.into_iter().enumerate() {
            let mut m = meeting();
            m.id = format!("evt-{i}");
            m.end += chrono::Duration::hours(i as i64);
            if stage == Stage::ToPlace {
                m.takeaways = takeaways_from(
                    serde_json::from_str::<Reply>(
                        r#"[{"text":"a","is_mine":true,"project":"web"},{"text":"b","is_mine":true,"project":"web"},{"text":"c","is_mine":true,"project":"web"}]"#,
                    )
                    .unwrap()
                    .takeaways(),
                    &places(),
                );
            }
            m.stage = stage;
            app.automations.meetings.kept.meetings.push(m);
        }
        let panel = MeetingTakeaways.panel(&app);
        let states: Vec<&str> = panel.lists[0]
            .rows
            .iter()
            .map(|r| r.state.as_str())
            .collect();
        assert_eq!(
            states,
            ["no transcript", "nothing for you", "2 added", "3 to place"]
        );
        assert_eq!(
            (panel.dot, panel.state.as_str()),
            (Dot::Waiting, "3 to place")
        );
    }

    /// A fake Microsoft Graph on a local port, answering each GET whose
    /// path starts with one of `routes`' prefixes with its body; anything
    /// else is a 404 in Graph's error shape. It serves until the test ends.
    fn fake_graph(routes: Vec<(&'static str, String)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                let _ = reader.read_line(&mut first);
                let mut header = String::new();
                let mut authorized = false;
                while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                    authorized |= header.trim() == "Authorization: Bearer fake-token";
                    header.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or_default();
                let found = routes.iter().find(|(prefix, _)| path.starts_with(prefix));
                let (status, body) = match (authorized, found) {
                    (true, Some((_, body))) => ("200 OK", body.clone()),
                    _ => (
                        "404 Not Found",
                        format!(
                            r#"{{"error":{{"code":"NotFound","message":"no route for {path}"}}}}"#
                        ),
                    ),
                };
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://127.0.0.1:{port}/v1.0")
    }

    /// End to end, with a fake Graph and the model's answer canned: a
    /// meeting that ended is found by its join link, its sitting's
    /// transcript fetched as VTT and read down to text; the read lands on
    /// the runner, the card shows, and Enter puts your takeaways into an
    /// existing group of a project and a new group of Blue sky.
    #[test]
    fn a_meeting_goes_from_graph_to_placed_todos() {
        let join = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc";
        let filter = crate::microsoft::transcripts::meeting_path(join);
        let routes =
            vec![
            (
                // The join link, encoded as Graph wants it, finds the meeting.
                Box::leak(format!("/v1.0{filter}").into_boxed_str()) as &'static str,
                r#"{"value": [{"id": "MSo1N2Y5", "joinWebUrl": "x"}]}"#.to_string(),
            ),
            (
                "/v1.0/me/onlineMeetings/MSo1N2Y5/transcripts/MSMjMCMj/content?$format=text/vtt",
                crate::microsoft::transcripts::tests::VTT.to_string(),
            ),
            (
                "/v1.0/me/onlineMeetings/MSo1N2Y5/transcripts",
                r#"{"value": [
                    {"id": "OLD", "createdDateTime": "2026-11-01T10:31:00Z"},
                    {"id": "MSMjMCMj", "createdDateTime": "2026-11-08T10:32:41.1234567Z"}
                ]}"#
                .to_string(),
            ),
            ("/v1.0/me?$select=displayName", r#"{"displayName": "Ada Lovelace"}"#.to_string()),
        ];
        let root = fake_graph(routes);
        std::env::set_var(orion_core::env::MICROSOFT_GRAPH_URL, &root);
        std::env::set_var(orion_core::env::MICROSOFT_TOKEN, "fake-token");
        let ms = crate::config::MicrosoftSettings::default();
        let m = meeting();
        let vtt = crate::microsoft::transcripts::look(&ms, join, m.start)
            .unwrap()
            .expect("today's transcript");
        let me = crate::microsoft::graph::display_name(&ms).unwrap();
        assert_eq!(me, "Ada Lovelace");
        // A join link Graph doesn't know is said.
        assert!(crate::microsoft::transcripts::look(&ms, "https://nope", m.start).is_err());

        let dir = tempfile::tempdir().unwrap();
        store::with_dir(dir.path().to_path_buf(), || {
            let mut app = App::new();
            let mut web = TodoFile::new(Path::new("/code/web"));
            let onboarding = web.add_group(None, "Onboarding");
            web.add_item(onboarding, "Old one", crate::todos::today());
            app.todos.insert(PathBuf::from("/code/web"), web);
            app.automations.meetings.kept.meetings.push(m);
            land(
                &mut app,
                Answer::Looked {
                    id: "evt-1".into(),
                    found: Ok(Some((vtt, me))),
                },
            );
            let read_in = app.automations.meetings.meeting("evt-1").unwrap();
            assert_eq!(read_in.stage, Stage::Reading);
            assert!(
                read_in.transcript.starts_with("Ada Lovelace: Morning."),
                "{}",
                read_in.transcript
            );
            assert!(read_in
                .transcript
                .contains("\nGrace Hopper: Thanks & I'll write"));

            // The model's answer, canned.
            let reply: Reply = serde_json::from_str(
                r#"{"takeaways": [
                    {"text": "Fix the empty address 500", "owner_initials": "AL", "is_mine": true,
                     "project": "web", "group_path": ["Onboarding"], "priority": 2, "ticket": "ENG-412"},
                    {"text": "Write the onboarding email", "owner_initials": "GH", "is_mine": false,
                     "project": "web", "group_path": ["Onboarding"]},
                    {"text": "A habit tracker", "owner_initials": "AL", "is_mine": true,
                     "project": "blue sky", "group_path": ["Someday"]}
                ]}"#,
            )
            .unwrap();
            land(
                &mut app,
                Answer::Read {
                    id: "evt-1".into(),
                    reply: Ok(reply.takeaways()),
                    places: places(),
                },
            );
            let cards = crate::needs_you::meetings::Meetings.cards(&app);
            assert_eq!(cards[0].sub, "2 takeaways for you to place");
            assert_eq!(add_to_todos(&mut app, "evt-1"), Ok(2));
            let web = &app.todos[Path::new("/code/web")];
            let texts: Vec<&str> = web.items.iter().map(|i| i.text.as_str()).collect();
            // High priority ranks above the unprioritised, as in Todos.
            assert_eq!(texts, ["Fix the empty address 500 (ENG-412)", "Old one"]);
            assert_eq!(web.items[0].group, onboarding);
            assert_eq!(web.items[0].priority, 2);
            let sky = &app.todos[&store::blue_sky_key()];
            assert_eq!(sky.path_label(sky.items[0].group), "Someday");
            assert!(crate::needs_you::meetings::Meetings.cards(&app).is_empty());
        });
    }

    /// The read for real, through the default Claude account: a short
    /// made-up standup and two fake projects with their groups. Run by
    /// hand: `cargo test -p orion-tui meeting_takeaways::tests::
    /// extracts_for_real -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn extracts_for_real() {
        let vtt = "WEBVTT\n\n\
            1\n00:00:01.000 --> 00:00:06.000\n<v Ada Lovelace>Morning. The invoices endpoint is throwing 500s when a customer has no billing address. I'll take that today.</v>\n\n\
            2\n00:00:06.500 --> 00:00:12.000\n<v Grace Hopper>Thanks. On mobile, the HEIC avatar crash is still there. Ada, can you look at it after the invoices fix? It's urgent for the release.</v>\n\n\
            3\n00:00:12.500 --> 00:00:18.000\n<v Ada Lovelace>Sure. Also, wild idea: a little habit tracker that nags you to stretch. Not for any of our apps, just fun.</v>\n\n\
            4\n00:00:18.500 --> 00:00:23.000\n<v Grace Hopper>Ha. I'll update the release notes for the mobile build myself.</v>\n";
        let transcript = crate::microsoft::transcripts::vtt_to_text(vtt);
        let mut places = vec![
            Place {
                about:
                    "A React Native mobile app for customers to manage their profile and orders."
                        .into(),
                ..place_of(
                    "mobile",
                    "/code/mobile",
                    &["Crashes", "Release", "Release › Notes"],
                    &[("MOB-77", "HEIC avatar crash on save")],
                )
            },
            Place {
                about: "The Node billing backend: invoices, payments and the REST API behind them."
                    .into(),
                ..place_of("billing-api", "/code/billing", &["Bugs", "Payments"], &[])
            },
        ];
        places.push(place_of("blue sky", "blue sky", &["Product"], &[]));
        let preset = crate::agent_presets::SystemPreset::MeetingTakeaways.default_preset();
        let prompt = super::super::compose(
            &preset,
            &context(&transcript, "Ada Lovelace", "ada@riplo.ai", &places),
            "",
        );
        println!("--- prompt ---\n{prompt}\n");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let started = std::time::Instant::now();
        let reply = rt
            .block_on(crate::oneshot::run_json::<Reply>(crate::oneshot::Ask {
                prompt,
                system: SYSTEM.into(),
                account: crate::oneshot::Account {
                    program: "claude".into(),
                    env: Vec::new(),
                    label: "default".into(),
                },
                model: crate::week_review::DEFAULT_MODEL.into(),
                effort: crate::week_review::DEFAULT_EFFORT.into(),
                timeout: TIMEOUT,
            }))
            .expect("the model answered in shape");
        let got = takeaways_from(reply.takeaways(), &places);
        println!("--- in {:?} ---", started.elapsed());
        for t in &got {
            println!(
                "{} {:<45} {:<3} {:?} {:?} p{} {:?}",
                if t.accepted { "✓" } else { "✗" },
                t.text,
                t.owner,
                t.dest,
                t.group_path,
                t.priority,
                t.ticket
            );
        }
        assert!(got.iter().any(|t| t.mine), "{got:#?}");
    }
}
