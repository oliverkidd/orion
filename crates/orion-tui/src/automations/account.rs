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
//!   no running turn is ever cut short ([`plan`]). One waiting on you —
//!   a plan to approve, a question — is told once it is up on the new
//!   account to put it to you again ([`reprompt`]): the resume comes back
//!   at the CLI's input box, its dialog gone. Its factory card stays as
//!   it was. The move is kept on disk (`<data_dir>/automations/
//!   factory-move.json`), so a restart carries on with it.
//! - **At the limit.** When the factory account reports a usage limit the
//!   factory pauses (`super::limit`), says so once, and offers to move the
//!   factory to the next account: one key sets this, which moves it.

use chrono::Local;
use orion_core::harness::HarnessDescriptor;
use orion_core::{Agent, AgentId, AgentKind, AgentStatus, ClientRequest};
use serde::{Deserialize, Serialize};

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
    /// Whether the move a last run left on disk has been taken up.
    pub resumed: bool,
}

/// The factory's sessions on their way to another account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Move {
    /// The account's registry id, and what the screens call it.
    pub to: String,
    pub label: String,
    /// The sessions still to move, each waiting for its turn to end.
    pub waiting: Vec<AgentId>,
    /// How many have been sent over.
    pub sent: usize,
    /// Sessions sent over while waiting on you, each with the turn that
    /// puts its plan or question to you again once it is up on the new
    /// account ([`reprompt`]).
    #[serde(default)]
    pub reprompts: Vec<Reprompt>,
}

/// A moved session's turn to send once it runs on the new account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reprompt {
    pub id: AgentId,
    pub text: String,
    /// When it was sent over, in ms: one that never arrives is let go
    /// after [`REPROMPT_GIVE_UP_MS`].
    pub at_ms: i64,
}

/// How long a reprompt waits for its session to come up on the new
/// account — a move the daemon refused never does.
const REPROMPT_GIVE_UP_MS: i64 = 10 * 60 * 1000;

/// What a session moved while waiting on a plan's approval is told.
pub const PLAN_AGAIN: &str = "You were moved to another account. Your plan is still waiting for approval: present it again for approval exactly as before.";
/// What one moved while waiting on its question is told.
pub const QUESTION_AGAIN: &str = "You were moved to another account. Your question is still waiting for an answer: re-ask your question with AskUserQuestion exactly as before.";

/// The turn a session waiting on `reason` is sent after its move: a plan
/// presented again, a question asked again. None for anything else — a
/// permission prompt comes back as the session carries on.
pub fn reprompt(reason: Option<&orion_core::WaitReason>) -> Option<&'static str> {
    match reason? {
        orion_core::WaitReason::Plan { .. } => Some(PLAN_AGAIN),
        orion_core::WaitReason::Question { .. } => Some(QUESTION_AGAIN),
        _ => None,
    }
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
                reprompts: Vec::new(),
            })
        }
    };
    save_move(app.automations.account.moving.as_ref());
    app.dirty = true;
}

/// Send over every session of the move under way whose turn is over, and
/// the turn that restores its prompt to each one that has arrived.
fn advance(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(mut moving) = app.automations.account.moving.take() else {
        return;
    };
    let before = moving.clone();
    let (send, waiting) = plan(&app.tree.agents, &moving.waiting, &moving.to);
    let now = crate::app::now_ms();
    for id in &send {
        let text = reprompt(app.needs_you.reasons.get(id));
        if let (Some(text), Some(agent)) = (text, app.tree.agents.iter().find(|a| &a.id == id)) {
            if agent.status == AgentStatus::NeedsFeedback {
                moving.reprompts.push(Reprompt {
                    id: id.clone(),
                    text: text.to_string(),
                    at_ms: now,
                });
            }
        }
        let harness = moving.to.clone();
        let id = id.clone();
        crate::event_loop::send(app, out, |req_id| ClientRequest::ContinueAgentOn {
            req_id,
            id,
            harness,
        });
    }
    moving.sent += send.len();
    moving.waiting = waiting;
    let (due, left) = arrived(&app.tree.agents, moving.reprompts, &moving.to, now);
    moving.reprompts = left;
    for (id, text) in due {
        crate::event_loop::send_turn(app, &id, text, false, out);
    }
    if moving.waiting.is_empty() && moving.reprompts.is_empty() {
        if moving.sent > 0 {
            app.flash = Some(Flash::done(format!(
                "The factory runs on {} now: {} moved",
                moving.label,
                crate::flash::plural(moving.sent, "session")
            )));
        }
        save_move(None);
        app.dirty = true;
        return;
    }
    if moving != before {
        save_move(Some(&moving));
    }
    app.automations.account.moving = Some(moving);
}

/// The reprompts whose sessions are up on `to` now — `(id, text)` to send
/// — and the ones still on their way. One whose session is gone, or that
/// has waited past [`REPROMPT_GIVE_UP_MS`], is let go.
pub fn arrived(
    agents: &[Agent],
    reprompts: Vec<Reprompt>,
    to: &str,
    now_ms: i64,
) -> (Vec<(AgentId, String)>, Vec<Reprompt>) {
    let mut due = Vec::new();
    let mut left = Vec::new();
    for r in reprompts {
        let Some(agent) = agents.iter().find(|a| a.id == r.id && !a.archived) else {
            continue;
        };
        if account_of(agent) == to && agent.alive && agent.status != AgentStatus::Running {
            due.push((r.id, r.text));
        } else if now_ms - r.at_ms < REPROMPT_GIVE_UP_MS {
            left.push(r);
        }
    }
    (due, left)
}

// ---- the move on disk ----

#[cfg(test)]
thread_local! {
    static TEST_FILE: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with the move kept at `path`. Under test, with none named,
/// nothing is kept.
#[cfg(test)]
pub fn with_move_file<T>(path: std::path::PathBuf, f: impl FnOnce() -> T) -> T {
    let before = TEST_FILE.with(|p| p.replace(Some(path)));
    let out = f();
    TEST_FILE.with(|p| *p.borrow_mut() = before);
    out
}

fn move_file() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        TEST_FILE.with(|p| p.borrow().clone())
    }
    #[cfg(not(test))]
    {
        Some(super::state_dir().join("factory-move.json"))
    }
}

/// Keep `moving` for a restart to take up; None forgets it.
fn save_move(moving: Option<&Move>) {
    let Some(path) = move_file() else {
        return;
    };
    let Some(moving) = moving else {
        let _ = std::fs::remove_file(path);
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let written = serde_json::to_string_pretty(moving)
        .map_err(|e| e.to_string())
        .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
    if let Err(e) = written {
        tracing::warn!("factory: couldn't keep the move: {e}");
    }
}

/// The move a last run left on disk, if any.
fn load_move() -> Option<Move> {
    let text = std::fs::read_to_string(move_file()?).ok()?;
    serde_json::from_str(&text).ok()
}

/// Take up the move a last run left, once, as the first tick runs.
fn resume_move(app: &mut App) {
    if std::mem::replace(&mut app.automations.account.resumed, true) {
        return;
    }
    if app.automations.account.moving.is_none() {
        app.automations.account.moving = load_move();
    }
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
    resume_move(app);
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
                limit::with_file(dir.path().join("pause.json"), || {
                    with_move_file(dir.path().join("move.json"), || f(dir.path()))
                })
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

    /// A session moved while waiting on its plan or its question is told,
    /// once it is up on the new account, to put it to you again; one at
    /// rest is told nothing.
    #[test]
    fn a_session_waiting_on_you_gets_its_prompt_back_after_the_move() {
        in_config(|_| {
            let mut app = App::new();
            app.tree.agents = vec![
                session("planner", AgentStatus::NeedsFeedback, Some("claude-work")),
                session("asker", AgentStatus::NeedsFeedback, Some("claude-work")),
                session("idle", AgentStatus::Finished, Some("claude-work")),
            ];
            app.needs_you.reasons.insert(
                AgentId("planner".into()),
                orion_core::WaitReason::Plan {
                    plan: "Do it.".into(),
                },
            );
            app.needs_you.reasons.insert(
                AgentId("asker".into()),
                orion_core::WaitReason::Question {
                    questions: Vec::new(),
                },
            );
            set(&mut app, Some("claude-personal".into()));
            let mut out = Vec::new();
            advance(&mut app, &mut out);
            assert_eq!(out.len(), 3, "three moves, no turn yet: {out:?}");
            let moving = app.automations.account.moving.clone().unwrap();
            let texts: Vec<(&str, &str)> = moving
                .reprompts
                .iter()
                .map(|r| (r.id.0.as_str(), r.text.as_str()))
                .collect();
            assert_eq!(texts, [("planner", PLAN_AGAIN), ("asker", QUESTION_AGAIN)]);
            assert!(PLAN_AGAIN.contains("present it again for approval exactly as before"));
            assert!(QUESTION_AGAIN.contains("re-ask your question with AskUserQuestion"));

            // Not there yet: nothing typed.
            out.clear();
            advance(&mut app, &mut out);
            assert!(out.is_empty());
            // The daemon's upserts: each on the new account, at rest.
            for agent in &mut app.tree.agents {
                agent.custom_harness = Some("claude-personal".into());
                agent.status = AgentStatus::Finished;
            }
            advance(&mut app, &mut out);
            let turns: Vec<(String, String)> = out
                .iter()
                .map(|req| match req {
                    ClientRequest::SendTurn { id, text, .. } => (id.0.clone(), text.clone()),
                    other => panic!("only turns: {other:?}"),
                })
                .collect();
            assert_eq!(
                turns,
                [
                    ("planner".to_string(), PLAN_AGAIN.to_string()),
                    ("asker".to_string(), QUESTION_AGAIN.to_string()),
                ]
            );
            assert_eq!(app.automations.account.moving, None, "done");
        });
    }

    #[test]
    fn a_reprompt_waits_for_its_session_and_gives_up_on_one_that_never_comes() {
        let to = "claude-personal";
        let r = |id: &str| Reprompt {
            id: AgentId(id.into()),
            text: PLAN_AGAIN.into(),
            at_ms: 0,
        };
        let agents = vec![
            session("there", AgentStatus::Finished, Some(to)),
            session("busy", AgentStatus::Running, Some(to)),
            session("stuck", AgentStatus::Finished, Some("claude-work")),
        ];
        let reprompts = vec![r("there"), r("busy"), r("stuck"), r("gone")];
        let (due, left) = arrived(&agents, reprompts.clone(), to, 1);
        assert_eq!(due, [(AgentId("there".into()), PLAN_AGAIN.to_string())]);
        let ids: Vec<&str> = left.iter().map(|r| r.id.0.as_str()).collect();
        assert_eq!(ids, ["busy", "stuck"]);
        let (_, left) = arrived(&agents, reprompts, to, REPROMPT_GIVE_UP_MS);
        assert!(left.is_empty(), "let go after the wait");
        assert_eq!(reprompt(None), None);
        assert_eq!(
            reprompt(Some(&orion_core::WaitReason::Permission {
                tool: "Bash".into()
            })),
            None
        );
    }

    /// The move is kept on disk as it goes: a restart — a new app —
    /// takes it up on its first tick and finishes it.
    #[test]
    fn a_move_survives_a_restart() {
        in_config(|_| {
            let mut app = App::new();
            app.tree.agents = vec![
                session("busy", AgentStatus::Running, Some("claude-work")),
                session("idle", AgentStatus::Finished, Some("claude-work")),
            ];
            set(&mut app, Some("claude-personal".into()));
            let mut out = Vec::new();
            advance(&mut app, &mut out);
            assert_eq!(out.len(), 1);
            let kept = load_move().expect("kept on disk");
            assert_eq!(kept.waiting, [AgentId("busy".into())]);
            assert_eq!(kept.sent, 1);

            // orion restarts: the busy session's turn has ended meanwhile.
            let mut again = App::new();
            again.tree.agents = vec![
                session("busy", AgentStatus::Finished, Some("claude-work")),
                session("idle", AgentStatus::Finished, Some("claude-personal")),
            ];
            out.clear();
            tick(&mut again, &mut out);
            assert!(matches!(
                out.as_slice(),
                [ClientRequest::ContinueAgentOn { id, harness, .. }]
                    if id.0 == "busy" && harness == "claude-personal"
            ));
            assert_eq!(again.automations.account.moving, None);
            assert_eq!(load_move(), None, "done: forgotten");
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
            assert!(!super::super::may_start(&app));
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
            assert!(super::super::may_start(&app));
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
