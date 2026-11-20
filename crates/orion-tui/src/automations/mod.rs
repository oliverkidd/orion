//! AUTOMATIONS: the agents orion starts on its own — the PR fixer, the bug
//! fixer, the feature builder, week in review, meeting takeaways and the
//! blue sky builder — and the one engine they all plug into.
//!
//! * **The registry.** [`Automation`] names every one, with what
//!   Settings › Automations ([`settings_tab`]) and the running overview
//!   (`⌘J`, [`view`]) say about it and the SYSTEM PRESET it is told by. Each one that does anything has a [`Runner`]
//!   in [`Engine`], which the git tick drives ([`tick`]).
//! * **Tagged sessions.** Every session an automation starts carries its
//!   id in `Agent::automation` ([`launch`], or a draft's own
//!   `AgentLaunchDraft::automation`), which the DAEMON keeps with the row:
//!   the grid's badge and Needs you's kind label read it ([`kind_of`]).
//! * **One cap.** No automation starts a session while
//!   `automations.max_running` of them are at work — mid-turn or booting
//!   ([`may_start`]). A session waiting on you holds no place, and
//!   nothing caps how much may wait on you: a plan or a pull request left
//!   on Slack to pick up at the desk never holds the next ticket back.
//! * **One way to say it.** A runner's first prompt is its preset's text
//!   with its context at `{task}`, and the user's extra steer after it
//!   ([`compose`]).
//!
//! Settings are global, never per project: `automations` in `config.json`
//! ([`config`]).

pub mod account;
pub mod blue_sky;
pub mod config;
pub mod limit;
pub mod linear_builder;
pub mod meeting_takeaways;
pub mod pr_fixer;
pub mod settings_tab;
pub mod view;
pub mod week_review;

use crate::agent_presets::{AgentPreset, SystemPreset};
use crate::app::{AgentLaunchDraft, App};
use orion_core::{Agent, AgentId, AgentStatus, ClientRequest};
use std::collections::HashMap;

/// Every automation, in the order Settings › Automations and the overview
/// list them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Automation {
    PrFixer,
    BugFixer,
    FeatureBuilder,
    WeekReview,
    MeetingTakeaways,
    BlueSky,
}

impl Automation {
    pub const ALL: [Automation; 6] = [
        Automation::PrFixer,
        Automation::BugFixer,
        Automation::FeatureBuilder,
        Automation::WeekReview,
        Automation::MeetingTakeaways,
        Automation::BlueSky,
    ];

    /// Its key under `automations.per`, and the tag on every session it
    /// starts (`Agent::automation`): `orion_core::automation`'s table, in
    /// this enum's order.
    pub const fn id(self) -> &'static str {
        orion_core::automation::ALL[self as usize].0
    }

    /// The automation an id names: a session's tag read back.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id.trim())
    }

    /// Its name on the screen, and the kind a session it started is
    /// labelled with.
    pub const fn name(self) -> &'static str {
        orion_core::automation::ALL[self as usize].1
    }

    /// What it does: the line under **On** in Settings › Automations, its
    /// keys `{token}`s the keymap spells (`hints::expand`).
    pub const fn description(self) -> &'static str {
        match self {
            Automation::PrFixer => "When one of your PRs hits conflicts or failing checks, fixes it and watches until it's green. Stops after 3 tries in a row that don't work. Once it's green, has its agent work through Cubic's findings.",
            Automation::BugFixer => "Picks up Linear tickets assigned to you that carry one of its labels, plans each one, waits for your approval, then builds to a draft PR.",
            Automation::FeatureBuilder => "Picks up Linear tickets assigned to you that carry one of its labels, plans each one, waits for your approval, then builds to a draft PR.",
            Automation::WeekReview => "Writes up the week from everything merged, done and ticked, so it's ready before the meeting. Also in {command_palette} as \"Run week in review now\".",
            Automation::MeetingTakeaways => "When a Teams meeting's transcript lands, pulls out the takeaways, accepts the ones that name you and places them in your todos for you to check. Covers all projects.",
            Automation::BlueSky => "Builds an idea from Todos › Blue sky when you press Enter on it: writes a brief, waits for your approval, then builds a prototype to try.",
        }
    }

    /// The SYSTEM PRESET it is told by, unless `⌘U` picked another.
    pub const fn preset(self) -> SystemPreset {
        match self {
            Automation::PrFixer => SystemPreset::PrAutofix,
            Automation::BugFixer => SystemPreset::BugFixer,
            Automation::FeatureBuilder => SystemPreset::FeatureBuilder,
            Automation::WeekReview => SystemPreset::WeekReview,
            Automation::MeetingTakeaways => SystemPreset::MeetingTakeaways,
            Automation::BlueSky => SystemPreset::BlueSky,
        }
    }

    /// Whether it is on in a config that says nothing about it. The PR
    /// fixer is, as **Auto-fix my PRs** was; so are the blue sky builder,
    /// which starts nothing until Enter on an idea asks it to, and meeting
    /// takeaways, which asks nothing until Microsoft 365 is signed in and
    /// only ever puts takeaways in front of you. The rest wait to be asked.
    pub const fn on_by_default(self) -> bool {
        matches!(
            self,
            Automation::PrFixer | Automation::MeetingTakeaways | Automation::BlueSky
        )
    }

    /// Its rows in Settings › Factory, in order. None has a cap of its
    /// own: **Agents working at once** is the one cap, for all of them
    /// but the PR fixer, which stands outside it ([`uncapped`]).
    pub const fn settings(self) -> &'static [SettingRow] {
        use SettingRow::*;
        match self {
            Automation::BugFixer | Automation::FeatureBuilder => &[On, Labels, Preset, ExtraSteer],
            Automation::WeekReview => &[On, When, Whose, Preset, ExtraSteer],
            Automation::PrFixer | Automation::BlueSky | Automation::MeetingTakeaways => {
                &[On, Preset, ExtraSteer]
            }
        }
    }
}

/// The automation that started `agent`, if one did.
pub fn kind_of(agent: &Agent) -> Option<Automation> {
    agent.automation.as_deref().and_then(Automation::from_id)
}

/// A row of an automation's section in Settings › Automations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingRow {
    /// Space or Enter: on or off.
    On,
    /// The Linear labels a ticket needs (the bug fixer's, the feature
    /// builder's): Enter is the runner's ([`Runner::edit`]).
    Labels,
    /// When it runs (week in review's): Enter is the runner's.
    When,
    /// Whose work it covers (week in review's): Enter is the runner's.
    Whose,
    /// The preset it is told by: Enter opens it in Settings › Presets,
    /// `⌘U` picks another.
    Preset,
    /// Enter edits the line appended to every prompt it sends.
    ExtraSteer,
}

impl SettingRow {
    pub const fn label(self) -> &'static str {
        match self {
            SettingRow::On => "On",
            SettingRow::Labels => "Labels",
            SettingRow::When => "When",
            SettingRow::Whose => "Whose",
            SettingRow::Preset => "Preset",
            SettingRow::ExtraSteer => "Extra steer",
        }
    }
}

/// The dot before an automation's row: what it is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    Working,
    Waiting,
    Idle,
    Off,
}

/// How a list row's state reads: by meaning, as every screen colours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Yellow: at work.
    Working,
    /// Red: waiting on you, or given up.
    Waiting,
    /// Green: done.
    Done,
    /// Grey: nothing to do.
    Quiet,
}

/// One row of a panel's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelRow {
    pub text: String,
    pub state: String,
    pub tone: Tone,
    /// The session Enter on the row jumps to.
    pub session: Option<AgentId>,
    /// Where `⌘O` on the row opens: its pull request, else its ticket.
    pub url: Option<String>,
}

/// One of a panel's lists — YOUR OPEN PRS, RUNNING, PAST RUNS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelList {
    pub title: &'static str,
    /// What it says with no rows.
    pub empty: &'static str,
    pub rows: Vec<PanelRow>,
}

/// What a runner shows: its row's dot and short state, and its lists —
/// the page the overview (`⌘J`) shows beside it. The name,
/// description and settings come from the registry and the config, not
/// from here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub dot: Dot,
    /// "fixing #407", "1 plan waiting", "Fri 08:30" …
    pub state: String,
    pub lists: Vec<PanelList>,
}

impl Panel {
    /// A runner with nothing to show yet.
    pub fn idle() -> Self {
        Self {
            dot: Dot::Idle,
            state: "idle".into(),
            lists: Vec::new(),
        }
    }
}

/// One automation's engine half. The state it keeps between ticks is its
/// own; what it starts goes through [`launch`].
pub trait Runner {
    fn automation(&self) -> Automation;
    /// The git tick (every few seconds): look for work, start what is due.
    fn tick(&mut self, _app: &mut App, _out: &mut Vec<ClientRequest>) {}
    /// Its dot, state and lists, as of now.
    fn panel(&self, app: &App) -> Panel;
    /// Enter on a row Settings › Automations has no editor for of its own
    /// ([`SettingRow::Labels`], [`SettingRow::When`],
    /// [`SettingRow::Whose`]). True when it did something.
    fn edit(&mut self, _app: &mut App, _row: SettingRow) -> bool {
        false
    }
    /// A row Enter edits as a line of text (week in review's
    /// **When**): the line as it stands. None for every other row.
    fn text_of(&self, _app: &App, _row: SettingRow) -> Option<String> {
        None
    }
    /// That line written: kept, or why not.
    fn set_text(&mut self, _app: &mut App, row: SettingRow, _text: &str) -> Result<(), String> {
        Err(format!("{} can't be written here", row.label()))
    }
    /// Whether Enter on one of its list rows with no session is its own
    /// ([`Self::open_row`]) — else that row's link opens in the browser.
    fn opens_rows(&self) -> bool {
        false
    }
    /// Enter on the `index`th of its list rows with no session, counted
    /// across its lists (meeting takeaways' MEETINGS, week in review's
    /// REVIEWS).
    fn open_row(&mut self, _app: &mut App, _index: usize) {}
    /// A key of its own on its page in the overview, shown in the keys
    /// while it is selected (week in review's `⌘G` run now).
    fn hotkey(&self) -> Option<crate::hints::Key> {
        None
    }
    /// That key pressed: done, or why not.
    fn hotkey_pressed(&mut self, _app: &mut App) -> Result<(), String> {
        Ok(())
    }
}

/// The app's automations (`App::automations`).
pub struct Engine {
    /// One per automation that has one, in [`Automation::ALL`] order.
    /// Taken out while one runs ([`with_runner`]).
    runners: Vec<Box<dyn Runner>>,
    /// `automations` in the config, mirrored (`event_loop::apply_config`).
    pub settings: config::AutomationsSettings,
    /// Creates sent for an automation whose Ack has not come, and the
    /// automation each is for: counted as running, so one tick can't
    /// start past the cap.
    pub launching: HashMap<u64, Automation>,
    /// The automation the overview (`⌘J`) was last left on, by
    /// [`Automation::ALL`] index, for its next open.
    pub selected: usize,
    /// Where the runners' asks off the loop land ([`land`]).
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    /// The bug fixer's and feature builder's shared half: the ledger of
    /// tickets picked up, and the groups waiting for a place under the cap.
    pub linear_builder: linear_builder::Shared,
    /// Meeting takeaways' meetings, and its looks and reads under way.
    pub meetings: meeting_takeaways::Shared,
    /// The factory account's move under way, and the pause at its limit.
    pub account: account::State,
}

impl Default for Engine {
    /// Every automation off until the config is applied as orion starts
    /// (`event_loop::apply_config`): an app nobody configured starts
    /// nothing on its own.
    fn default() -> Self {
        let mut settings = config::AutomationsSettings::default();
        for which in Automation::ALL {
            settings.of_mut(which).on = Some(false);
        }
        Self {
            runners: vec![
                Box::new(pr_fixer::PrFixer),
                Box::new(linear_builder::LinearBuilder(Automation::BugFixer)),
                Box::new(linear_builder::LinearBuilder(Automation::FeatureBuilder)),
                Box::new(week_review::WeekReview::default()),
                Box::new(blue_sky::BlueSkyBuilder::default()),
                Box::new(meeting_takeaways::MeetingTakeaways),
            ],
            settings,
            launching: HashMap::new(),
            selected: 0,
            tx: None,
            linear_builder: linear_builder::Shared::default(),
            meetings: meeting_takeaways::Shared::default(),
            account: account::State::default(),
        }
    }
}

impl Engine {
    /// Whether `which` is on, as the config last said.
    pub fn is_on(&self, which: Automation) -> bool {
        self.settings.is_on(which)
    }

    fn runner(&self, which: Automation) -> Option<&dyn Runner> {
        self.runners
            .iter()
            .find(|r| r.automation() == which)
            .map(|r| r.as_ref())
    }

    /// `which`'s panel: its runner's, or an idle one for an automation
    /// that has none yet. Off is off, whatever the runner says.
    pub fn panel(app: &App, which: Automation) -> Panel {
        let mut panel = app
            .automations
            .runner(which)
            .map_or_else(Panel::idle, |r| r.panel(app));
        if !app.automations.is_on(which) {
            panel.dot = Dot::Off;
            panel.state = "off".into();
        }
        panel
    }
}

/// Run `f` on `which`'s runner with the app free to borrow: the runner is
/// taken out of [`Engine`] for the call and put back after. None when it
/// has no runner.
pub fn with_runner<T>(
    app: &mut App,
    which: Automation,
    f: impl FnOnce(&mut dyn Runner, &mut App) -> T,
) -> Option<T> {
    let at = app
        .automations
        .runners
        .iter()
        .position(|r| r.automation() == which)?;
    let mut runner = app.automations.runners.remove(at);
    let out = f(runner.as_mut(), app);
    let at = at.min(app.automations.runners.len());
    app.automations.runners.insert(at, runner);
    Some(out)
}

/// The git tick: the factory account's turn first ([`account::tick`]),
/// then every runner of an automation that is on looks for work — none
/// while the factory is paused at its account's limit ([`limit`]). The
/// bug fixer and the feature builder share one ledger: it is brought up
/// to date once before they look, and the Linear writes they made are
/// sent once after.
pub fn tick(app: &mut App, out: &mut Vec<ClientRequest>) {
    account::tick(app, out);
    let builders = [Automation::BugFixer, Automation::FeatureBuilder]
        .into_iter()
        .any(|which| app.automations.is_on(which));
    if builders {
        linear_builder::stages::follow(app);
    }
    // Cards whose work moved on close themselves, here and on Slack.
    crate::factory::cards::reconcile(app);
    let paused = app.automations.account.paused.is_some();
    for which in Automation::ALL {
        if app.automations.is_on(which) && !paused {
            with_runner(app, which, |runner, app| runner.tick(app, out));
        }
    }
    if builders {
        linear_builder::owner::send_writes(app);
    }
}

/// Where the automations keep their state: `<data_dir>/automations`.
pub(crate) fn state_dir() -> std::path::PathBuf {
    orion_core::paths::data_dir().join("automations")
}

/// How many automation sessions hold a place under the cap: tagged, not
/// archived, at work — plus the creates still on their way. The PR
/// fixer's are never counted ([`uncapped`]).
pub fn running(app: &App) -> usize {
    let live = app
        .tree
        .agents
        .iter()
        .filter(|a| a.automation.is_some() && !kind_of(a).is_some_and(uncapped))
        .filter(|a| !a.archived && occupies(a))
        .count();
    let coming = app.automations.launching.values();
    live + coming.filter(|w| !uncapped(**w)).count()
}

/// Whether `which` stands outside the cap: the PR fixer mends pull
/// requests already open, so it never waits for a place and holds none.
/// Only the factory's pause at its account's limit ([`paused`]) holds it
/// back.
pub const fn uncapped(which: Automation) -> bool {
    matches!(which, Automation::PrFixer)
}

/// Whether the factory is paused at its account's usage limit
/// ([`limit`]): nothing starts, the PR fixer included.
pub fn paused(app: &App) -> bool {
    app.automations.account.paused.is_some()
}

/// How many of `which`'s own sessions are running, as [`running`] counts
/// them: its tagged live sessions and its creates on their way.
pub fn running_of(app: &App, which: Automation) -> usize {
    let live = app
        .tree
        .agents
        .iter()
        .filter(|a| kind_of(a) == Some(which) && !a.archived && occupies(a))
        .count();
    let coming = app.automations.launching.values();
    live + coming.filter(|w| **w == which).count()
}

/// Whether a session holds one of the cap's places: mid-turn, or
/// booting. One waiting on you — a plan, a question — holds none: the
/// cap is on agents at work, and what waits on you is never capped.
fn occupies(agent: &Agent) -> bool {
    match agent.status {
        AgentStatus::Running => true,
        AgentStatus::Fresh => agent.alive,
        _ => false,
    }
}

/// Whether a session is still on its task: at work ([`occupies`]), or
/// waiting on you. Neither is started over.
pub(crate) fn on_task(agent: &Agent) -> bool {
    occupies(agent) || agent.status == AgentStatus::NeedsFeedback
}

/// Whether one more capped automation's session may start: the factory
/// not paused at its account's limit ([`paused`]), and under the cap. The
/// PR fixer asks only [`paused`] ([`uncapped`]).
pub fn may_start(app: &App) -> bool {
    !paused(app) && running(app) < app.automations.settings.cap()
}

/// The automations whose work is the software factory's.
const FACTORY: [Automation; 3] = [
    Automation::BugFixer,
    Automation::FeatureBuilder,
    Automation::PrFixer,
];

/// A factory card open longer than this no longer counts toward
/// [`attention`].
const CARD_COUNTS_MS: i64 = 7 * orion_core::clock::DAY_MS;

/// How many factory items wait on you, as the Urgent card says it:
/// factory sessions waiting on you (a plan, a question), and the factory
/// cards you haven't acted on — a pull request's verdict, a ticket held
/// for you, a fix given up on — in the last [`CARD_COUNTS_MS`]. A
/// no-change card is its session's plan, counted once. It holds nothing
/// back.
pub fn attention(app: &App) -> usize {
    let sessions = app
        .tree
        .agents
        .iter()
        .filter(|a| !a.archived && a.status == AgentStatus::NeedsFeedback)
        .filter(|a| kind_of(a).is_some_and(|k| FACTORY.contains(&k)))
        .count();
    let counts = |key: &str| {
        ["issue:", "pr:", "fix:"].iter().any(|p| key.starts_with(p)) && !key.ends_with(":nochange")
    };
    let now = crate::app::now_ms();
    let cards = crate::factory::cards::open_to_you(app)
        .filter(|p| counts(&p.card.key))
        .filter(|p| now - p.at_ms <= CARD_COUNTS_MS)
        .count();
    sessions + cards
}

/// Start `which`'s session: tagged with it, and refused — false — at
/// the cap, for the runner to try again on a later tick.
pub fn launch(
    app: &mut App,
    which: Automation,
    draft: AgentLaunchDraft,
    out: &mut Vec<ClientRequest>,
) -> bool {
    if !may_start(app) {
        return false;
    }
    let draft = AgentLaunchDraft {
        automation: Some(which),
        ..draft
    };
    crate::event_loop::create_agent(app, draft, out);
    true
}

/// Start `which`'s session in a fresh worktree — or in the checkout
/// `launch` names — as the QUICK PROMPT's Enter does with `launch` and
/// `text` typed, tagged with it, moving nothing on screen; refused, false,
/// at the cap, for the runner to try again on a later tick. The checkout
/// being cut holds its place under it.
pub fn launch_in_new_worktree(
    app: &mut App,
    which: Automation,
    launch: crate::quick_prompt::QuickLaunch,
    text: String,
    out: &mut Vec<ClientRequest>,
) -> bool {
    if !may_start(app) {
        return false;
    }
    let launch = crate::quick_prompt::QuickLaunch {
        automation: Some(which),
        ..launch
    };
    crate::event_loop::submit_unattended(app, launch, text, out);
    true
}

/// What a runner's ask off the loop brings back.
#[derive(Debug)]
pub enum Answer {
    LinearBuilder(linear_builder::Answer),
    MeetingTakeaways(meeting_takeaways::Answer),
}

/// An answer lands on the runner it is for — whether or not it is on, so a
/// screen waiting on it (the labels pick) still fills.
pub fn land(app: &mut App, answer: Answer, out: &mut Vec<ClientRequest>) {
    match answer {
        Answer::LinearBuilder(answer) => linear_builder::land(app, answer, out),
        Answer::MeetingTakeaways(answer) => meeting_takeaways::land(app, answer),
    }
    app.dirty = true;
}

/// The most PAST RUNS a page lists.
pub const PAST_MAX: usize = 50;

/// A PAST RUNS row's state: how it ended, and how long ago (`merged ·
/// 2d ago`) when `at_ms` is known.
pub fn past_state(word: &str, at_ms: i64) -> String {
    match at_ms {
        0 => word.to_string(),
        _ => format!(
            "{word} · {}",
            crate::hosts::ago_label(crate::app::now_ms() - at_ms)
        ),
    }
}

/// PAST RUNS: `which`'s sessions that came to rest — finished, or
/// archived — newest first and however long ago, at most [`PAST_MAX`],
/// each a jump to its transcript.
pub fn past_sessions(app: &App, which: Automation) -> PanelList {
    let mut done: Vec<&Agent> = app
        .tree
        .agents
        .iter()
        .filter(|a| kind_of(a) == Some(which))
        .filter(|a| a.archived || a.status == AgentStatus::Finished)
        .collect();
    done.sort_by_key(|a| std::cmp::Reverse(a.status_changed_at));
    PanelList {
        title: "PAST RUNS",
        empty: "Nothing finished yet",
        rows: done
            .into_iter()
            .take(PAST_MAX)
            .map(|a| {
                let word = if a.archived { "archived" } else { "done" };
                PanelRow {
                    text: a.name.clone(),
                    state: past_state(word, a.status_changed_at),
                    tone: Tone::Done,
                    session: Some(a.id.clone()),
                    url: None,
                }
            })
            .collect(),
    }
}

/// The preset `which` is told by: the one `⌘U` picked while the list
/// still has it, else its SYSTEM PRESET — as stored, its text blank while
/// it is orion's ([`compose`] fills it in).
pub fn preset_of(settings: &config::AutomationsSettings, which: Automation) -> AgentPreset {
    let picked = settings.of(which).preset;
    picked
        .and_then(|name| {
            crate::agent_presets::load_seeded()
                .into_iter()
                .find(|p| p.name.trim().eq_ignore_ascii_case(name.trim()))
        })
        .unwrap_or_else(|| crate::agent_presets::system(which.preset()))
}

/// What an automation's agent is told: `preset`'s text — orion's while it
/// holds none of the user's — with `context` at its `{task}`, and the
/// user's `extra` steer after a blank line as `Extra steer: …` when there
/// is any. Every runner's prompt goes through here.
pub fn compose(preset: &AgentPreset, context: &str, extra: &str) -> String {
    compose_with(preset, context, "", extra)
}

/// [`compose`] for an agent that writes code: `rule` (the testing rule,
/// [`config::LocalE2e::rule`]) between the preset's text and the extra
/// steer, so the user's steer still has the last word.
pub fn compose_with(preset: &AgentPreset, context: &str, rule: &str, extra: &str) -> String {
    let mut parts = vec![preset.clone().with_builtin_text().compose(context)];
    if !rule.trim().is_empty() {
        parts.push(rule.trim().to_string());
    }
    if !extra.trim().is_empty() {
        parts.push(format!("Extra steer: {}", extra.trim()));
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{AgentKind, WorktreeId};
    use std::collections::HashSet;

    fn tagged(id: &str, status: AgentStatus, automation: Option<&str>) -> Agent {
        Agent {
            id: AgentId(id.into()),
            worktree_id: WorktreeId("w".into()),
            name: id.into(),
            status,
            archived: false,
            archived_at: 0,
            unseen: false,
            status_changed_at: 0,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: automation.map(str::to_string),
        }
    }

    #[test]
    fn ids_round_trip_and_name_every_automation_once() {
        for which in Automation::ALL {
            assert_eq!(Automation::from_id(which.id()), Some(which));
        }
        assert_eq!(Automation::from_id("lab"), None);
        assert_eq!(Automation::ALL.len(), orion_core::automation::ALL.len());
        let names: HashSet<&str> = Automation::ALL.iter().map(|a| a.name()).collect();
        assert_eq!(names.len(), Automation::ALL.len());
    }

    #[test]
    fn compose_appends_extra_steer_only_when_there_is_some() {
        let preset = AgentPreset {
            text: "Fix it.\n\n{task}\n\nThen push.".into(),
            ..SystemPreset::PrAutofix.default_preset()
        };
        let plain = compose(&preset, "PR #7", "");
        assert_eq!(plain, "Fix it.\n\nPR #7\n\nThen push.");
        assert_eq!(compose(&preset, "PR #7", "   "), plain, "blank is none");
        assert_eq!(
            compose(&preset, "PR #7", " Run the e2e specs too. "),
            "Fix it.\n\nPR #7\n\nThen push.\n\nExtra steer: Run the e2e specs too."
        );
        // A system preset on orion's own text says orion's text.
        let builtin = compose(&SystemPreset::PrAutofix.default_preset(), "PR #7", "");
        assert!(builtin.starts_with("You are orion's PR autofixer"));
        assert!(builtin.contains("PR #7"));
    }

    #[test]
    fn the_testing_rule_sits_between_the_preset_and_the_extra_steer() {
        use config::{AutomationsSettings, LocalE2e};
        let preset = AgentPreset {
            text: "Fix it.\n\n{task}".into(),
            ..SystemPreset::PrAutofix.default_preset()
        };
        // Out of the box, agents leave e2e to CI.
        let never = AutomationsSettings::default().local_e2e;
        assert_eq!(never, LocalE2e::Never);
        let text = compose_with(&preset, "PR #7", never.rule(), "Be quick.");
        assert_eq!(
            text,
            format!(
                "Fix it.\n\nPR #7\n\n{}\n\nExtra steer: Be quick.",
                never.rule()
            )
        );
        assert!(never.rule().contains("never run end-to-end"));
        assert!(LocalE2e::Covering.rule().contains("only the e2e specs"));
        assert_eq!(never.next(), LocalE2e::Covering);
        // A saved choice reads back, and a config without one is never.
        let saved: AutomationsSettings =
            serde_json::from_str(r#"{"local_e2e": "covering"}"#).unwrap();
        assert_eq!(saved.local_e2e, LocalE2e::Covering);
        let none: AutomationsSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(none.local_e2e, LocalE2e::Never);
        // orion's own coding presets leave e2e to CI unless the logs
        // aren't enough, and never run a full suite; the builders carry
        // their machine rules in their own layers.
        for text in [
            pr_fixer::PRESET_TEXT,
            linear_builder::BUG_FIXER_TEXT,
            linear_builder::FEATURE_BUILDER_TEXT,
        ] {
            assert!(text.contains("logs aren't enough"), "{text}");
            assert!(!text.contains("full suite once"), "{text}");
        }
        for text in [
            linear_builder::BUG_FIXER_TEXT,
            linear_builder::FEATURE_BUILDER_TEXT,
        ] {
            assert!(!text.contains("testing rule below"), "{text}");
        }
    }

    #[test]
    fn the_running_cap_counts_live_tagged_sessions_and_creates_on_their_way() {
        let mut app = App::new();
        app.tree.agents = vec![
            tagged("a", AgentStatus::Running, Some("feature_builder")),
            tagged("b", AgentStatus::NeedsFeedback, Some("bug_fixer")),
            tagged("c", AgentStatus::Finished, Some("bug_fixer")),
            tagged("d", AgentStatus::Running, None),
            {
                let mut gone = tagged("e", AgentStatus::Running, Some("blue_sky"));
                gone.archived = true;
                gone
            },
        ];
        app.automations.settings.max_running = 3;
        assert_eq!(
            running(&app),
            1,
            "waiting on you, finished, untagged and archived don't count"
        );
        assert!(may_start(&app));
        app.automations.launching.insert(8, Automation::BugFixer);
        app.automations.launching.insert(9, Automation::BlueSky);
        assert_eq!(running(&app), 3);
        assert!(!may_start(&app), "a create on its way takes a place");

        // A refused launch sends nothing.
        let mut out = Vec::new();
        let draft = AgentLaunchDraft::new(WorktreeId("w".into()), AgentKind::Claude, None, None);
        assert!(!launch(&mut app, Automation::BugFixer, draft, &mut out));
        assert!(out.is_empty());

        // A hand-edited 0 is still one.
        app.automations.launching.clear();
        app.tree.agents.clear();
        app.automations.settings.max_running = 0;
        assert!(may_start(&app));
    }

    /// The one cap is on agents at work, all automations together: no
    /// runner has a cap of its own, and sessions waiting on you — however
    /// many — hold no place.
    #[test]
    fn only_agents_at_work_count_against_the_one_cap() {
        let mut app = App::new();
        app.automations.settings.max_running = 2;
        app.tree.agents = (0..6)
            .map(|n| {
                tagged(
                    &format!("w{n}"),
                    AgentStatus::NeedsFeedback,
                    Some("bug_fixer"),
                )
            })
            .collect();
        assert!(may_start(&app), "six waiting on you");
        app.tree
            .agents
            .push(tagged("a", AgentStatus::Running, Some("bug_fixer")));
        assert!(may_start(&app), "no cap of its own");
        app.tree
            .agents
            .push(tagged("b", AgentStatus::Running, Some("feature_builder")));
        assert!(!may_start(&app), "two at work");
        for which in Automation::ALL {
            assert!(!which.settings().is_empty(), "{}", which.name());
        }
    }

    /// The PR fixer stands outside the cap: its sessions at work, and its
    /// creates on their way, hold no place — and only the factory's pause
    /// at its account's limit holds it back.
    #[test]
    fn the_pr_fixer_neither_waits_for_nor_holds_a_place() {
        let mut app = App::new();
        app.automations.settings.max_running = 1;
        app.tree.agents = (0..4)
            .map(|n| tagged(&format!("f{n}"), AgentStatus::Running, Some("pr_fixer")))
            .collect();
        app.automations.launching.insert(7, Automation::PrFixer);
        assert_eq!(running(&app), 0, "four fixers at work, one on its way");
        assert!(may_start(&app), "a bug fixer still finds its place");
        app.tree
            .agents
            .push(tagged("b", AgentStatus::Running, Some("bug_fixer")));
        assert!(!may_start(&app), "the cap is full");
        assert!(
            Automation::ALL
                .into_iter()
                .all(|w| uncapped(w) == (w == Automation::PrFixer)),
            "the PR fixer alone stands outside the cap"
        );
    }

    /// A saved cap of 8 (stepped up before the default came down) is kept
    /// through a load and a save; a config without one reads as 3.
    #[test]
    fn a_saved_cap_survives_and_a_missing_one_is_three() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        crate::config::with_config_path(path.clone(), || {
            std::fs::write(&path, r#"{"automations": {"max_running": 8}}"#).unwrap();
            let cfg = crate::config::Config::load();
            assert_eq!(cfg.automations.max_running, 8);
            cfg.try_save().unwrap();
            assert_eq!(crate::config::Config::load().automations.max_running, 8);
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(saved["automations"]["max_running"], 8);

            std::fs::write(
                &path,
                r#"{"automations": {"per": {"bug_fixer": {"on": true}}}}"#,
            )
            .unwrap();
            let cfg = crate::config::Config::load();
            assert_eq!(cfg.automations.max_running, 3);
            assert_eq!(cfg.automations.cap(), 3);
        });
    }

    /// A config from before the one cap — a cap per runner, **Things
    /// waiting on you** — still loads, and a save drops what is gone.
    #[test]
    fn an_old_config_with_the_retired_caps_still_loads() {
        let old: config::AutomationsSettings = serde_json::from_str(
            r#"{"max_running": 8, "attention_cap": 4,
                "per": {"bug_fixer": {"on": true, "max_running": 4}}}"#,
        )
        .unwrap();
        assert_eq!(old.cap(), 8);
        assert!(old.is_on(Automation::BugFixer));
        let saved = serde_json::to_value(&old).unwrap();
        assert!(saved.get("attention_cap").is_none());
        assert!(saved["per"]["bug_fixer"].get("max_running").is_none());
    }

    /// Factory sessions waiting on you and the factory cards you haven't
    /// acted on count; other sessions, other cards and a no-change card
    /// (its session's plan, already counted) don't.
    #[test]
    fn attention_counts_factory_items_waiting_on_you() {
        use crate::factory::cards::{post, Card, Recipient};
        let mut app = App::new();
        app.tree.agents = vec![
            tagged("a", AgentStatus::NeedsFeedback, Some("bug_fixer")),
            tagged("b", AgentStatus::NeedsFeedback, Some("pr_fixer")),
            tagged("c", AgentStatus::NeedsFeedback, Some("blue_sky")),
            tagged("d", AgentStatus::NeedsFeedback, None),
            tagged("e", AgentStatus::Finished, Some("feature_builder")),
        ];
        let card = |key: &str, to: Recipient| Card {
            key: key.into(),
            to,
            context: String::new(),
            title: String::new(),
            body: Vec::new(),
            buttons: Vec::new(),
            image: None,
        };
        post(&mut app, card("pr:u:verdict", Recipient::Owner));
        post(&mut app, card("issue:i:urgent", Recipient::Owner));
        post(&mut app, card("issue:i:nochange", Recipient::Owner));
        post(
            &mut app,
            card("issue:j:thin", Recipient::Email("a@x.co".into())),
        );
        post(&mut app, card("kb:backfill", Recipient::Owner));
        assert_eq!(attention(&app), 4);
        // It holds nothing back.
        app.automations.settings.max_running = 20;
        assert!(may_start(&app));
        // A card open over a week no longer counts.
        app.factory_cards
            .iter_mut()
            .find(|p| p.card.key == "issue:i:urgent")
            .unwrap()
            .at_ms -= CARD_COUNTS_MS + 1;
        assert_eq!(attention(&app), 3);
    }

    #[test]
    fn the_cap_steps_within_its_range() {
        let mut s = config::AutomationsSettings::default();
        assert_eq!(s.max_running, 3);
        s.step_cap(1);
        assert_eq!(s.max_running, 4);
        s.max_running = 1;
        s.step_cap(-1);
        assert_eq!(s.max_running, 1);
        s.max_running = 20;
        s.step_cap(1);
        assert_eq!(s.max_running, 20);
    }

    #[test]
    fn only_the_pr_fixer_meeting_takeaways_and_blue_sky_are_on_out_of_the_box() {
        let s = config::AutomationsSettings::default();
        let on: Vec<Automation> = Automation::ALL
            .into_iter()
            .filter(|a| s.is_on(*a))
            .collect();
        assert_eq!(
            on,
            [
                Automation::PrFixer,
                Automation::MeetingTakeaways,
                Automation::BlueSky
            ]
        );
    }
}
