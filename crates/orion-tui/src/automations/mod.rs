//! AUTOMATIONS: the agents orion starts on its own — the PR fixer, the bug
//! fixer, the feature builder, week in review, meeting takeaways and the
//! blue sky builder — and the one engine they all plug into.
//!
//! * **The registry.** [`Automation`] names every one, with what the
//!   Automations screen (`⌘J`, [`view`]) says about it and the SYSTEM
//!   PRESET it is told by. Each one that does anything has a [`Runner`]
//!   in [`Engine`], which the git tick drives ([`tick`]).
//! * **Tagged sessions.** Every session an automation starts carries its
//!   id in `Agent::automation` ([`launch`], or a draft's own
//!   `AgentLaunchDraft::automation`), which the DAEMON keeps with the row:
//!   the grid's badge and Needs you's kind label read it ([`kind_of`]).
//! * **One cap.** No automation starts a session while
//!   `automations.max_running` of them are running ([`may_start`]).
//! * **One way to say it.** A runner's first prompt is its preset's text
//!   with its context at `{task}`, and the user's extra steer after it
//!   ([`compose`]).
//!
//! Settings are global, never per project: `automations` in `config.json`
//! ([`config`]).

pub mod blue_sky;
pub mod config;
pub mod linear_builder;
pub mod meeting_takeaways;
pub mod pr_fixer;
pub mod view;

use crate::agent_presets::{AgentPreset, SystemPreset};
use crate::app::{AgentLaunchDraft, App};
use orion_core::{Agent, AgentId, AgentStatus, ClientRequest};
use std::collections::HashSet;

/// Every automation, in the order the Automations screen lists them.
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

    /// The one line under its name on the screen.
    pub const fn description(self) -> &'static str {
        match self {
            Automation::PrFixer => "When one of your PRs hits conflicts or failing checks, fixes it and watches until it's green. Stops after 3 tries in a row that don't work.",
            Automation::BugFixer => "Picks up Linear tickets assigned to you that carry one of its labels, plans each one, waits for your approval, then builds to a draft PR.",
            Automation::FeatureBuilder => "Picks up Linear tickets assigned to you that carry one of its labels, plans each one, waits for your approval, then builds to a draft PR.",
            Automation::WeekReview => "Writes up the week from everything merged, done and ticked, so it's ready before the meeting. Also in ⌘⇧P as \"Run week in review now\".",
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
    /// fixer is, as **Auto-fix my PRs** was; the rest wait to be asked.
    pub const fn on_by_default(self) -> bool {
        matches!(self, Automation::PrFixer)
    }

    /// The SETTINGS rows its panel shows, in order (5.5).
    pub const fn settings(self) -> &'static [SettingRow] {
        use SettingRow::*;
        match self {
            Automation::BugFixer | Automation::FeatureBuilder => &[On, Labels, Preset, ExtraSteer],
            Automation::WeekReview => &[On, When, Whose, Preset, ExtraSteer],
            Automation::PrFixer | Automation::MeetingTakeaways | Automation::BlueSky => {
                &[On, Preset, ExtraSteer]
            }
        }
    }
}

/// The automation that started `agent`, if one did.
pub fn kind_of(agent: &Agent) -> Option<Automation> {
    agent.automation.as_deref().and_then(Automation::from_id)
}

/// A SETTINGS row on an automation's panel.
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
}

/// One of a panel's lists — YOUR OPEN PRS, RUNNING, DONE THIS WEEK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelList {
    pub title: &'static str,
    /// What it says with no rows.
    pub empty: &'static str,
    pub rows: Vec<PanelRow>,
}

/// What a runner shows: its row's dot and short state on the left, its
/// lists on the right. The name, description and SETTINGS come from the
/// registry and the config, not from here.
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
    /// Its SETTINGS rows.
    fn settings(&self) -> &'static [SettingRow] {
        self.automation().settings()
    }
    /// Enter on a SETTINGS row the screen has no editor for of its own
    /// ([`SettingRow::Labels`], [`SettingRow::When`],
    /// [`SettingRow::Whose`]). True when it did something.
    fn edit(&mut self, _app: &mut App, _row: SettingRow) -> bool {
        false
    }
}

/// The app's automations (`App::automations`).
pub struct Engine {
    /// One per automation that has one, in [`Automation::ALL`] order.
    /// Taken out while one runs ([`with_runner`]).
    runners: Vec<Box<dyn Runner>>,
    /// `automations` in the config, mirrored (`event_loop::apply_config`).
    pub settings: config::AutomationsSettings,
    /// Creates sent for an automation whose Ack has not come: counted as
    /// running, so one tick can't start past the cap.
    pub launching: HashSet<u64>,
    /// The Automations screen's row, kept for its next open.
    pub selected: usize,
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
            runners: vec![Box::new(pr_fixer::PrFixer)],
            settings,
            launching: HashSet::new(),
            selected: 0,
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

    /// `which`'s SETTINGS rows.
    pub fn settings_of(&self, which: Automation) -> &'static [SettingRow] {
        self.runner(which)
            .map_or_else(|| which.settings(), |r| r.settings())
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

/// The git tick: every runner of an automation that is on looks for work.
pub fn tick(app: &mut App, out: &mut Vec<ClientRequest>) {
    for which in Automation::ALL {
        if app.automations.is_on(which) {
            with_runner(app, which, |runner, app| runner.tick(app, out));
        }
    }
}

/// How many automation sessions are running: tagged, not archived, at
/// work or waiting on you — plus the creates still on their way.
pub fn running(app: &App) -> usize {
    let live = app
        .tree
        .agents
        .iter()
        .filter(|a| a.automation.is_some() && !a.archived && occupies(a))
        .count();
    live + app.automations.launching.len()
}

/// Whether a session holds one of the cap's places: mid-turn, waiting on
/// you, or booting.
fn occupies(agent: &Agent) -> bool {
    match agent.status {
        AgentStatus::Running | AgentStatus::NeedsFeedback => true,
        AgentStatus::Fresh => agent.alive,
        _ => false,
    }
}

/// Whether one more automation session may start under the cap.
pub fn may_start(app: &App) -> bool {
    running(app) < app.automations.settings.cap()
}

/// Start `which`'s session: tagged with it, and refused — false — at the
/// cap, for the runner to try again on a later tick.
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

/// How far back DONE THIS WEEK reaches.
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// DONE THIS WEEK: `which`'s sessions that came to rest in the last seven
/// days — finished, or archived — newest first, each a jump to it.
pub fn done_this_week(app: &App, which: Automation) -> PanelList {
    let since = crate::app::now_ms() - WEEK_MS;
    let mut done: Vec<&Agent> = app
        .tree
        .agents
        .iter()
        .filter(|a| kind_of(a) == Some(which) && a.status_changed_at >= since)
        .filter(|a| a.archived || a.status == AgentStatus::Finished)
        .collect();
    done.sort_by_key(|a| std::cmp::Reverse(a.status_changed_at));
    PanelList {
        title: "DONE THIS WEEK",
        empty: "Nothing done this week",
        rows: done
            .into_iter()
            .map(|a| PanelRow {
                text: a.name.clone(),
                state: if a.archived { "archived" } else { "done" }.into(),
                tone: Tone::Done,
                session: Some(a.id.clone()),
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
    let text = preset.clone().with_builtin_text().compose(context);
    match extra.trim() {
        "" => text,
        extra => format!("{text}\n\nExtra steer: {extra}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{AgentKind, WorktreeId};

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
    fn the_running_cap_counts_live_tagged_sessions_and_creates_on_their_way() {
        let mut app = App::new();
        app.tree.agents = vec![
            tagged("a", AgentStatus::Running, Some("pr_fixer")),
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
            2,
            "finished, untagged and archived don't count"
        );
        assert!(may_start(&app));
        app.automations.launching.insert(9);
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

    #[test]
    fn the_cap_steps_within_its_range() {
        let mut s = config::AutomationsSettings::default();
        assert_eq!(s.max_running, 5);
        s.step_cap(1);
        assert_eq!(s.max_running, 6);
        s.max_running = 1;
        s.step_cap(-1);
        assert_eq!(s.max_running, 1);
        s.max_running = 20;
        s.step_cap(1);
        assert_eq!(s.max_running, 20);
    }

    #[test]
    fn only_the_pr_fixer_is_on_out_of_the_box() {
        let s = config::AutomationsSettings::default();
        let on: Vec<Automation> = Automation::ALL
            .into_iter()
            .filter(|a| s.is_on(*a))
            .collect();
        assert_eq!(on, [Automation::PrFixer]);
    }
}
