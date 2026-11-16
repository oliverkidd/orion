//! THE FACTORY ACCOUNT: the Claude account everything orion runs on its
//! own goes through — the automations' sessions, the knowledge harvest and
//! backfill, and their one-shot model asks (a ticket's intake, the week in
//! review, the meeting takeaways). **Factory account** in Settings ›
//! Factory, `automations.account` in `config.json`: one of the Claude
//! accounts that are on, or none — the default agent, as before there was
//! a choice.
//!
//! - **Under the presets.** An automation whose preset pins an account or
//!   a harness keeps it; one that follows ([`AgentPreset::follows_box`])
//!   runs on the factory account ([`of_preset`], [`harness`]), and so does
//!   an ask through it (`oneshot::account_for`, `oneshot::factory_account`).
//! - **Sessions move together.** Changing it moves every live factory
//!   session — tagged with an automation, not archived — onto the new
//!   account, so they all stay on one (sessions in different config dirs
//!   can't see each other's work): each is carried over by the daemon's
//!   **Continue on** (`ClientRequest::ContinueAgentOn`), which copies its
//!   transcript into the new account's config dir and resumes the same
//!   session id there. A session mid-turn waits until its turn is over;
//!   no running turn is ever cut short ([`plan`]).
//! - **At the limit.** When the factory account reports a usage limit the
//!   factory pauses (`super::limit`), says so once, and offers to move the
//!   factory to the next account: one key sets this, which moves it.

use chrono::Local;
use orion_core::harness::HarnessDescriptor;
use orion_core::{Agent, AgentId, AgentKind, AgentStatus, ClientRequest};

use super::limit::{self, Pause};
use crate::agent_presets::AgentPreset;
use crate::app::{App, ConfirmDialog, Overlay, PendingAction};
use crate::config::Config;
use crate::flash::Flash;
use crate::oneshot::Account;
use crate::quick_prompt::{QuickLaunch, QuickTarget};

/// What the setting reads with no account chosen.
pub const DEFAULT_AGENT: &str = "Default agent";

/// The app's half of the factory account (`Engine::account`).
#[derive(Debug, Default)]
pub struct State {
    /// The move under way since the setting last changed.
    pub moving: Option<Move>,
    /// The pause that holds the factory back, as the last tick read it.
    pub paused: Option<Pause>,
    /// The reset of the pause the notice was given for: one notice a pause.
    pub noticed: Option<i64>,
}

/// The factory's sessions on their way to another account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    /// The account's registry id, and what the screens call it.
    pub to: String,
    pub label: String,
    /// The sessions still to move, each waiting for its turn to end.
    pub waiting: Vec<AgentId>,
    /// How many have been sent over.
    pub sent: usize,
}

// ---- the setting ----

/// The Claude account `automations.account` names, while it is one that
/// is on. None for the default agent — or an account switched off or gone
/// since, which falls back to it.
pub fn chosen(cfg: &Config) -> Option<HarnessDescriptor> {
    let id = cfg.automations.account.as_deref()?.trim();
    let all = cfg.harness_registry();
    orion_core::harness::usable(&all)
        .into_iter()
        .find(|entry| entry.id == id && entry.is_claude_account())
        .cloned()
}

/// What **Factory account** steps through: the default agent (None), then
/// every Claude account the pickers offer, by registry id.
pub fn choices(cfg: &Config) -> Vec<Option<String>> {
    let accounts = cfg
        .offered_harnesses()
        .into_iter()
        .map(|(kind, custom)| cfg.effective_harness(kind, custom.as_deref()))
        .filter(HarnessDescriptor::is_claude_account)
        .map(|entry| Some(entry.id));
    std::iter::once(None).chain(accounts).collect()
}

/// What the setting reads as: the account's label, or [`DEFAULT_AGENT`].
pub fn label(cfg: &Config) -> String {
    chosen(cfg).map_or_else(
        || DEFAULT_AGENT.to_string(),
        |entry| entry.display_label().to_string(),
    )
}

/// The setting a step along [`choices`] from where it is.
pub fn stepped(cfg: &Config, step: i8) -> Option<String> {
    let all = choices(cfg);
    let now = chosen(cfg).map(|entry| entry.id);
    let at = all.iter().position(|c| *c == now).unwrap_or(0);
    let len = all.len() as isize;
    let next = (at as isize + isize::from(step.signum())).rem_euclid(len.max(1));
    all.get(next as usize).cloned().flatten()
}

/// The harness a factory session launches on under a preset that
/// follows: the factory account's, else the default agent's.
pub fn harness(cfg: &Config) -> (AgentKind, Option<String>) {
    chosen(cfg).map_or_else(|| cfg.quick_prompt_harness(), |e| e.session_kind())
}

/// The Claude account the factory's asks go through: the factory
/// account, else the default agent's (`Config::review_account`).
pub fn ask_entry(cfg: &Config) -> Option<HarnessDescriptor> {
    chosen(cfg).or_else(|| cfg.review_account(""))
}

/// Where the factory's sessions move when the setting changes: the
/// factory account, else the default agent's while that is a Claude
/// account — so long as it takes a Claude session.
pub fn move_target(cfg: &Config) -> Option<HarnessDescriptor> {
    let target = chosen(cfg).or_else(|| {
        let (kind, custom) = cfg.quick_prompt_harness();
        Some(cfg.effective_harness(kind, custom.as_deref())).filter(|e| e.is_claude_account())
    })?;
    target.takes_claude_sessions().then_some(target)
}

/// The registry ids a pause holds the factory back on: the account its
/// asks go through, and the one its sessions launch on.
fn factory_ids(cfg: &Config) -> Vec<String> {
    let (kind, custom) = harness(cfg);
    let mut ids = vec![orion_core::harness::registry_id(kind, custom.as_deref()).to_string()];
    ids.extend(ask_entry(cfg).map(|e| e.id));
    ids
}

// ---- launches ----

/// `QuickLaunch::of_preset` for an automation: a preset that follows runs
/// on the factory account ([`chosen`]) in place of the default agent; one
/// that pins a harness keeps it.
pub fn of_preset(target: QuickTarget, preset: AgentPreset, cfg: &Config) -> QuickLaunch {
    match chosen(cfg).filter(|_| preset.follows_box()) {
        Some(entry) => {
            let (kind, custom) = entry.session_kind();
            let on = QuickLaunch::of_kind(target.clone(), kind, custom, None, None, cfg);
            QuickLaunch::of_preset(target, preset, Some(&on), cfg)
        }
        None => QuickLaunch::of_preset(target, preset, None, cfg),
    }
}

/// A launch `(kind, custom, model, effort)` carried onto the factory
/// account while one is chosen and the launch is on a Claude account —
/// the PR fixer following the session that built a branch — keeping its
/// model and effort where the account offers them. Anything else as it
/// is.
pub fn onto(
    cfg: &Config,
    launch: (AgentKind, Option<String>, Option<String>, Option<String>),
) -> (AgentKind, Option<String>, Option<String>, Option<String>) {
    let (kind, custom, model, effort) = launch;
    let Some(entry) = chosen(cfg) else {
        return (kind, custom, model, effort);
    };
    if !cfg
        .effective_harness(kind, custom.as_deref())
        .is_claude_account()
    {
        return (kind, custom, model, effort);
    }
    let (kind, custom) = entry.session_kind();
    let model = model.filter(|m| entry.offers_model(m));
    let effort = effort.filter(|e| entry.offers_effort(e));
    (kind, custom, model, effort)
}

// ---- the move ----

/// The registry id of the account or harness `agent` runs on.
fn account_of(agent: &Agent) -> &str {
    orion_core::harness::registry_id(agent.kind, agent.custom_harness.as_deref())
}

/// What a factory session waiting to move does on this look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Carry it over now: its turn is over, or it waits on you.
    Send,
    /// Mid-turn, or booting into its first: look again later.
    Wait,
    /// Nothing to move: gone, archived, already there, or never ran.
    Done,
}

/// What `agent` does in a move to the account `to`.
pub fn step(agent: Option<&Agent>, to: &str) -> Step {
    let Some(agent) = agent.filter(|a| !a.archived) else {
        return Step::Done;
    };
    if account_of(agent) == to {
        return Step::Done;
    }
    match agent.status {
        AgentStatus::Running => Step::Wait,
        AgentStatus::Fresh if agent.alive && agent.session_id.is_none() => Step::Wait,
        _ if agent.session_id.is_none() => Step::Done,
        _ => Step::Send,
    }
}

/// One look at the sessions `waiting` to move to `to`: the ones to send
/// over now, and the ones that still wait.
pub fn plan(agents: &[Agent], waiting: &[AgentId], to: &str) -> (Vec<AgentId>, Vec<AgentId>) {
    let mut send = Vec::new();
    let mut wait = Vec::new();
    for id in waiting {
        match step(agents.iter().find(|a| &a.id == id), to) {
            Step::Send => send.push(id.clone()),
            Step::Wait => wait.push(id.clone()),
            Step::Done => {}
        }
    }
    (send, wait)
}

/// Every live factory session that would move to `to`: tagged with an
/// automation, not archived, a Claude session, and not there already.
pub fn to_move(cfg: &Config, agents: &[Agent], to: &str) -> Vec<AgentId> {
    agents
        .iter()
        .filter(|a| a.automation.is_some() && !a.archived)
        .filter(|a| {
            a.kind == AgentKind::Claude
                || cfg
                    .effective_harness(a.kind, a.custom_harness.as_deref())
                    .claude_like()
        })
        .filter(|a| account_of(a) != to)
        .map(|a| a.id.clone())
        .collect()
}

/// Set **Factory account** to `choice` — None for the default agent — and
/// start moving the factory's live sessions there.
pub fn set(app: &mut App, choice: Option<String>) {
    super::view::change_settings(app, |s| s.account = choice);
    let cfg = Config::load();
    refresh_pause(app, &cfg);
    let Some(target) = move_target(&cfg) else {
        app.automations.account.moving = None;
        return;
    };
    let waiting = to_move(&cfg, &app.tree.agents, &target.id);
    let label = target.display_label().to_string();
    app.automations.account.moving = match waiting.is_empty() {
        true => None,
        false => {
            app.flash = Some(Flash::working(format!(
                "Moving {} to {label}, each as its turn ends",
                crate::flash::plural(waiting.len(), "factory session")
            )));
            Some(Move {
                to: target.id,
                label,
                waiting,
                sent: 0,
            })
        }
    };
    app.dirty = true;
}

/// Send over every session of the move under way whose turn is over.
fn advance(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(moving) = app.automations.account.moving.take() else {
        return;
    };
    let (send, waiting) = plan(&app.tree.agents, &moving.waiting, &moving.to);
    for id in &send {
        let harness = moving.to.clone();
        let id = id.clone();
        crate::event_loop::send(app, out, |req_id| ClientRequest::ContinueAgentOn {
            req_id,
            id,
            harness,
        });
    }
    let sent = moving.sent + send.len();
    if waiting.is_empty() {
        if sent > 0 {
            app.flash = Some(Flash::done(format!(
                "The factory runs on {} now: {} moved",
                moving.label,
                crate::flash::plural(sent, "session")
            )));
        }
        app.dirty = true;
        return;
    }
    app.automations.account.moving = Some(Move {
        waiting,
        sent,
        ..moving
    });
}

// ---- the limit ----

/// The pause that holds the factory back now: the one on disk, while it
/// is on the account the factory runs on.
fn refresh_pause(app: &mut App, cfg: &Config) {
    let ids = factory_ids(cfg);
    app.automations.account.paused = limit::current().filter(|p| ids.contains(&p.account));
}

/// A factory session stopped on the factory account's usage limit pauses
/// the factory, as a refused ask does (`limit::note_refusal`) — until the
/// reset its line names, read as of when it stopped: a limit whose reset
/// has passed, its row still red, holds nothing back.
fn note_session_limits(app: &App, cfg: &Config) {
    let (kind, custom) = harness(cfg);
    let on = orion_core::harness::registry_id(kind, custom.as_deref()).to_string();
    let now = Local::now();
    if limit::current_at(now).is_some_and(|p| p.account == on) {
        return;
    }
    let ask = Account::of(&cfg.effective_harness(kind, custom.as_deref()));
    let Some(pause) = app
        .tree
        .agents
        .iter()
        .filter(|a| a.automation.is_some())
        .filter(|a| account_of(a) == on)
        .filter_map(|a| {
            let limit = a.limit_reached()?;
            let said = limit
                .message
                .clone()
                .unwrap_or_else(|| limit.label().to_string());
            let stopped = chrono::TimeZone::timestamp_millis_opt(&Local, a.status_changed_at)
                .single()
                .unwrap_or(now);
            Some(Pause::of(&on, &ask, &said, stopped))
        })
        .find(|pause| pause.until_ms > now.timestamp_millis())
    else {
        return;
    };
    tracing::warn!(
        "factory: a session on {} is at its limit until {}, pausing: {}",
        pause.label,
        pause.until(now),
        pause.said
    );
    limit::set(&pause);
}

/// The account the notice offers: the first Claude account that takes a
/// session, other than the paused one, with room left at the last
/// reading of its usage.
fn next_account(app: &App, cfg: &Config, paused: &str) -> Option<HarnessDescriptor> {
    choices(cfg)
        .into_iter()
        .flatten()
        .filter(|id| id != paused)
        .map(|id| cfg.effective_harness_by_id(&id))
        .filter(HarnessDescriptor::takes_claude_sessions)
        .find(|entry| !crate::usage::headroom(app, &entry.id).is_some_and(|room| room.spent()))
}

/// The notice for `pause`, offering `next`: its Enter sets the setting
/// to `next`, which moves the factory there.
pub fn notice(
    pause: &Pause,
    next: &HarnessDescriptor,
    now: chrono::DateTime<Local>,
) -> ConfirmDialog {
    ConfirmDialog {
        title: "Factory paused at its limit".into(),
        message: format!(
            "Factory account {} is at its limit\nuntil {}.\n\nMove the factory to {}?",
            pause.label,
            pause.until(now),
            next.display_label()
        ),
        action: PendingAction::MoveFactory {
            to: next.id.clone(),
        },
        area: ratatui::layout::Rect::default(),
    }
}

/// Say the pause once: the dialog offering the next account when there is
/// one — held until nothing else is up on screen — else a footer line.
fn give_notice(app: &mut App, cfg: &Config, pause: &Pause) {
    if app.automations.account.noticed == Some(pause.until_ms) {
        return;
    }
    let now = Local::now();
    match next_account(app, cfg, &pause.account) {
        Some(next) if app.overlay.is_none() => {
            app.overlay = Some(Overlay::Confirm(notice(pause, &next, now)));
        }
        Some(_) => return,
        None => {
            app.flash = Some(Flash::setup(format!(
                "Factory account {} is at its limit until {}: the factory is paused until then",
                pause.label,
                pause.until(now)
            )));
        }
    }
    app.automations.account.noticed = Some(pause.until_ms);
    app.dirty = true;
}

/// The tick's first turn, before any runner's: a factory session at the
/// factory account's limit pauses it; the pause is read, said once when
/// it is new and lifted on its own past its reset; and the move under way
/// sends over every session whose turn has ended.
pub fn tick(app: &mut App, out: &mut Vec<ClientRequest>) {
    let limited = app
        .tree
        .agents
        .iter()
        .any(|a| a.automation.is_some() && a.limit_reached().is_some());
    let cfg = Config::load();
    if limited {
        note_session_limits(app, &cfg);
    }
    let was = app.automations.account.paused.take();
    refresh_pause(app, &cfg);
    match (was, app.automations.account.paused.clone()) {
        (_, Some(pause)) => give_notice(app, &cfg, &pause),
        (Some(was), None) => {
            if app.automations.account.moving.is_none() {
                app.flash = Some(Flash::note(format!(
                    "{} is past its limit: the factory carries on",
                    was.label
                )));
            }
            app.dirty = true;
        }
        (None, None) => {}
    }
    advance(app, out);
}

/// The sessions the move under way still waits on, for the setting's row.
pub fn waiting(app: &App) -> usize {
    app.automations
        .account
        .moving
        .as_ref()
        .map_or(0, |m| m.waiting.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{UsageLimit, WorktreeId};
    use std::collections::HashSet;

    /// The ids of every Claude account `cfg` turns on.
    fn ids(cfg: &Config) -> HashSet<String> {
        choices(cfg).into_iter().flatten().collect()
    }

    /// A config with two Claude accounts beside the default one, in
    /// `dir`: `claude-work` and `claude-personal`.
    fn two_accounts(dir: &std::path::Path) -> String {
        let work = dir.join("work");
        let personal = dir.join("personal");
        serde_json::json!({
            "claude_accounts": [
                {"id": "claude-work", "config_dir": work.to_str().unwrap(), "name": "Work"},
                {"id": "claude-personal", "config_dir": personal.to_str().unwrap(), "name": "Personal"},
            ]
        })
        .to_string()
    }

    fn in_config<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, two_accounts(dir.path())).unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("agent_presets.json"), || {
            crate::config::with_config_path(path, || {
                limit::with_file(dir.path().join("pause.json"), || f(dir.path()))
            })
        })
    }

    pub(crate) fn session(id: &str, status: AgentStatus, on: Option<&str>) -> Agent {
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
            custom_harness: on.map(str::to_string),
            model: None,
            effort: None,
            session_id: Some(format!("sid-{id}")),
            sort_order: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: Some("bug_fixer".into()),
        }
    }

    fn save_choice(choice: Option<&str>) {
        let mut cfg = Config::load();
        cfg.automations.account = choice.map(str::to_string);
        cfg.try_save().unwrap();
    }

    #[test]
    fn the_setting_steps_through_the_default_agent_and_every_account() {
        in_config(|_| {
            let cfg = Config::load();
            assert_eq!(
                ids(&cfg),
                ["claude", "claude-work", "claude-personal"]
                    .map(String::from)
                    .into()
            );
            assert_eq!(choices(&cfg)[0], None);
            assert_eq!(label(&cfg), DEFAULT_AGENT);
            assert_eq!(chosen(&cfg), None);
            let next = stepped(&cfg, 1);
            assert!(next.is_some());
            assert_eq!(stepped(&cfg, -1), choices(&cfg).last().cloned().flatten());
            save_choice(Some("claude-work"));
            let cfg = Config::load();
            assert_eq!(chosen(&cfg).map(|e| e.id).as_deref(), Some("claude-work"));
            assert!(label(&cfg).starts_with("Work"), "{}", label(&cfg));
            // An account that is gone falls back to the default agent.
            save_choice(Some("claude-gone"));
            assert_eq!(chosen(&Config::load()), None);
        });
    }

    /// The setting decides the account of every launch and ask under a
    /// preset that follows; a preset that pins an account keeps it.
    #[test]
    fn the_setting_decides_the_account_for_launches_and_asks() {
        in_config(|dir| {
            let follows = crate::agent_presets::system(Automation::BugFixer.preset());
            assert!(follows.follows_box());
            let target = QuickTarget::Worktree(WorktreeId("w".into()));
            let cfg = Config::load();
            let before = of_preset(target.clone(), follows.clone(), &cfg);
            assert_eq!(
                (before.kind, before.custom.clone()),
                cfg.quick_prompt_harness()
            );
            let asked = crate::oneshot::factory_account(&cfg).unwrap();
            assert_eq!(asked, crate::oneshot::default_account(&cfg).unwrap());

            save_choice(Some("claude-work"));
            let cfg = Config::load();
            let work_dir = dir.join("work").display().to_string();
            // A session launch.
            let launch = of_preset(target.clone(), follows.clone(), &cfg);
            assert_eq!(launch.kind, AgentKind::Claude);
            assert_eq!(launch.custom.as_deref(), Some("claude-work"));
            assert_eq!(
                harness(&cfg),
                (AgentKind::Claude, Some("claude-work".into()))
            );
            // The factory's asks: intake, the harvest, the week in review.
            let asked = crate::oneshot::factory_account(&cfg).unwrap();
            assert!(
                asked
                    .env
                    .iter()
                    .any(|(k, v)| k == "CLAUDE_CONFIG_DIR" && *v == work_dir),
                "{asked:?}"
            );
            assert_eq!(
                crate::oneshot::account_for(&cfg, &follows),
                Some(asked.clone())
            );
            assert_ne!(crate::oneshot::default_account(&cfg), Some(asked));
            // The PR fixer following a Claude session moves onto it; one on
            // Codex stays.
            let followed = onto(&cfg, (AgentKind::Claude, None, Some("opus".into()), None));
            assert_eq!(
                (followed.0, followed.1.as_deref(), followed.2.as_deref()),
                (AgentKind::Claude, Some("claude-work"), Some("opus"))
            );
            let codex = (AgentKind::Codex, None, None, None);
            assert_eq!(onto(&cfg, codex.clone()), codex);

            // A preset that pins another account keeps it.
            let pinned = AgentPreset {
                kind: AgentKind::Claude,
                custom_harness: Some("claude-personal".into()),
                follow_box: Some(false),
                ..follows.clone()
            };
            let launch = of_preset(target, pinned.clone(), &cfg);
            assert_eq!(launch.custom.as_deref(), Some("claude-personal"));
            let asked = crate::oneshot::account_for(&cfg, &pinned).unwrap();
            assert!(
                asked.env.iter().any(|(_, v)| v.contains("personal")),
                "{asked:?}"
            );
        });
    }

    /// A session mid-turn waits; the ones at rest or waiting on you are
    /// sent over, each to the new account; the rest have nothing to move.
    #[test]
    fn a_move_sends_the_sessions_at_rest_and_waits_for_one_mid_turn() {
        in_config(|_| {
            let mut app = App::new();
            let mut limited = session("limited", AgentStatus::NeedsFeedback, Some("claude-work"));
            limited.usage_limit = Some(UsageLimit {
                reason: orion_core::LimitReason::RateLimit,
                message: Some("You've hit your weekly limit · resets Thu 9am".into()),
            });
            let mut untagged = session("mine", AgentStatus::Finished, Some("claude-work"));
            untagged.automation = None;
            let mut archived = session("old", AgentStatus::Finished, Some("claude-work"));
            archived.archived = true;
            app.tree.agents = vec![
                session("busy", AgentStatus::Running, Some("claude-work")),
                session("idle", AgentStatus::Finished, Some("claude-work")),
                limited,
                untagged,
                archived,
                session("there", AgentStatus::Finished, Some("claude-personal")),
            ];
            set(&mut app, Some("claude-personal".into()));
            assert_eq!(
                Config::load().automations.account.as_deref(),
                Some("claude-personal")
            );
            let moving = app.automations.account.moving.clone().unwrap();
            assert_eq!(moving.to, "claude-personal");
            let ids = |v: &[AgentId]| v.iter().map(|a| a.0.clone()).collect::<Vec<_>>();
            assert_eq!(ids(&moving.waiting), ["busy", "idle", "limited"]);

            let mut out = Vec::new();
            advance(&mut app, &mut out);
            let sent: Vec<(String, String)> = out
                .iter()
                .map(|req| match req {
                    ClientRequest::ContinueAgentOn { id, harness, .. } => {
                        (id.0.clone(), harness.clone())
                    }
                    other => panic!("only moves: {other:?}"),
                })
                .collect();
            assert_eq!(
                sent,
                [
                    ("idle".to_string(), "claude-personal".to_string()),
                    ("limited".to_string(), "claude-personal".to_string()),
                ]
            );
            assert_eq!(waiting(&app), 1, "the session mid-turn waits");

            // Still mid-turn: nothing more is sent, nothing is cut short.
            out.clear();
            advance(&mut app, &mut out);
            assert!(out.is_empty());
            // Its turn over, it goes too, and the move is done.
            app.tree.agents[0].status = AgentStatus::Finished;
            advance(&mut app, &mut out);
            assert!(matches!(
                out.as_slice(),
                [ClientRequest::ContinueAgentOn { id, harness, .. }]
                    if id.0 == "busy" && harness == "claude-personal"
            ));
            assert_eq!(app.automations.account.moving, None);
            assert!(app
                .flash
                .as_deref()
                .is_some_and(|f| f.contains("3 sessions moved")));
        });
    }

    #[test]
    fn each_session_steps_by_where_its_turn_is() {
        let to = "claude-personal";
        let step_of = |a: &Agent| step(Some(a), to);
        assert_eq!(step(None, to), Step::Done);
        assert_eq!(
            step_of(&session("a", AgentStatus::Running, Some("claude-work"))),
            Step::Wait
        );
        assert_eq!(
            step_of(&session(
                "a",
                AgentStatus::NeedsFeedback,
                Some("claude-work")
            )),
            Step::Send
        );
        assert_eq!(
            step_of(&session("a", AgentStatus::Finished, Some(to))),
            Step::Done
        );
        let mut booting = session("a", AgentStatus::Fresh, None);
        booting.session_id = None;
        assert_eq!(step_of(&booting), Step::Wait);
        booting.alive = false;
        assert_eq!(step_of(&booting), Step::Done, "never ran: nothing to carry");
    }

    /// A factory session stopped at the factory account's limit pauses
    /// the factory: no launches, one notice offering the next account,
    /// whose key sets the setting and starts the move. Past the reset the
    /// factory carries on by itself.
    #[test]
    fn a_session_at_the_limit_pauses_the_factory_and_offers_the_next_account() {
        in_config(|_| {
            save_choice(Some("claude-work"));
            let mut app = App::new();
            app.automations.settings = Config::load().automations;
            let mut limited = session("limited", AgentStatus::NeedsFeedback, Some("claude-work"));
            limited.usage_limit = Some(UsageLimit {
                reason: orion_core::LimitReason::RateLimit,
                message: Some("You've hit your weekly limit · resets Thu 9am".into()),
            });
            limited.status_changed_at = Local::now().timestamp_millis();
            app.tree.agents = vec![limited.clone()];
            // One that stopped a week ago is past its reset: nothing held.
            let mut stale = limited.clone();
            stale.status_changed_at -= 8 * orion_core::clock::DAY_MS;
            app.tree.agents = vec![stale];
            let mut out = Vec::new();
            tick(&mut app, &mut out);
            assert_eq!(app.automations.account.paused, None);
            app.tree.agents = vec![limited];
            tick(&mut app, &mut out);
            let pause = app.automations.account.paused.clone().expect("paused");
            assert_eq!(pause.account, "claude-work");
            assert!(!super::super::may_start(&app, Automation::BlueSky));
            let Some(Overlay::Confirm(dialog)) = &app.overlay else {
                panic!("the notice: {:?}", app.overlay)
            };
            assert!(
                dialog.message.starts_with("Factory account Work"),
                "{}",
                dialog.message
            );
            let until = pause.until(Local::now());
            assert!(
                dialog
                    .message
                    .contains(&format!("is at its limit\nuntil {until}.")),
                "{}",
                dialog.message
            );
            assert!(
                dialog.message.contains("Move the factory to"),
                "{}",
                dialog.message
            );
            let PendingAction::MoveFactory { to } = dialog.action.clone() else {
                panic!("{:?}", dialog.action)
            };
            assert_ne!(to, "claude-work");

            // Said once.
            app.overlay = None;
            tick(&mut app, &mut out);
            assert!(app.overlay.is_none());

            // The key: the setting set, the move started.
            set(&mut app, Some(to.clone()));
            assert_eq!(Config::load().automations.account, Some(to.clone()));
            assert_eq!(
                app.automations.account.paused, None,
                "the new account isn't paused"
            );
            assert!(super::super::may_start(&app, Automation::BlueSky));
            assert_eq!(
                app.automations
                    .account
                    .moving
                    .as_ref()
                    .map(|m| m.to.clone()),
                Some(to)
            );

            // Past the reset, the old account's pause is gone of itself.
            let mut gone = limit::current().unwrap();
            gone.until_ms = Local::now().timestamp_millis() - 1;
            limit::set(&gone);
            assert_eq!(limit::current(), None);
        });
    }

    /// A refused ask on the factory account with a limit for its reason
    /// pauses the factory; on any other account, or for another reason, it
    /// doesn't. A paused account's asks are refused with the pause's line.
    #[test]
    fn a_refused_ask_at_the_limit_pauses_the_factory() {
        in_config(|_| {
            save_choice(Some("claude-work"));
            let cfg = Config::load();
            let work = crate::oneshot::factory_account(&cfg).unwrap();
            let personal = Account::of(&cfg.effective_harness_by_id("claude-personal"));
            assert_eq!(limit::note_refusal(&work, "Not logged in"), None);
            assert_eq!(
                limit::note_refusal(&personal, "You've hit your limit · resets 3pm"),
                None,
                "not the factory's account"
            );
            assert_eq!(limit::refusal(&work), None);
            let pause = limit::note_refusal(&work, "You've hit your weekly limit · resets Thu 9am")
                .expect("paused");
            assert_eq!(pause.account, "claude-work");
            let refused = limit::refusal(&work).unwrap();
            let until = pause.until(Local::now());
            assert!(
                refused.contains(&format!("is at its limit until {until}")),
                "{refused}"
            );
            assert_eq!(limit::refusal(&personal), None);
            let mut app = App::new();
            refresh_pause(&mut app, &cfg);
            assert_eq!(app.automations.account.paused, Some(pause));
        });
    }

    use super::super::Automation;
}
