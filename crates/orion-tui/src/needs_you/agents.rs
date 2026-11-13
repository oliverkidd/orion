//! The board's AGENTS column: every session that wants you, of every
//! kind, in one list — crashed first, then waiting on you (the longest
//! first), then finished and unread.
//!
//! Which sessions count is the PALETTE's attention tiering
//! ([`crate::palette::session_tier`]) with the PROJECT TABS' rule for a
//! session whose PTY is gone: it no longer waits, but a crash stays one.
//! Each card reads `<kind> · <project> · <reason>`, the reason from
//! [`reason_text`].

use chrono::{DateTime, Utc};
use orion_core::{Agent, AgentStatus, WaitReason};

use super::model::{Card, CardSource, CardTarget, Section};
use crate::app::App;
use crate::palette::PaletteTier;

/// Urgency within the column: a crash over a question over a finish.
const CRASHED: u8 = 2;
const WAITING: u8 = 1;
const FINISHED: u8 = 0;

/// The reason half of a card's second line — what the session wants of
/// you: `asks a question`, `plan to approve`, `needs permission for
/// Bash`, `crashed, exit 1`, `draft PR #12 ready`, `done`. `reason` is
/// why the daemon last said it waits ([`reason_of`]); without one a
/// waiting session is `waiting for you`. `draft_pr` is the draft pull
/// request open on the session's checkout, what a finish leaves ready.
pub fn reason_text(agent: &Agent, reason: Option<&WaitReason>, draft_pr: Option<u64>) -> String {
    match (agent.status, reason) {
        (
            AgentStatus::Terminated,
            Some(WaitReason::Crash {
                exit_code: Some(code),
            }),
        ) => {
            format!("crashed, exit {code}")
        }
        (AgentStatus::Terminated, _) => "crashed".into(),
        (AgentStatus::NeedsFeedback, _) if agent.limit_reached().is_some() => {
            "hit its usage limit".into()
        }
        (AgentStatus::NeedsFeedback, Some(WaitReason::Question { .. })) => "asks a question".into(),
        // The blue sky builder's plan is the brief it wrote for the idea.
        (AgentStatus::NeedsFeedback, Some(WaitReason::Plan { .. }))
            if crate::automations::kind_of(agent)
                == Some(crate::automations::Automation::BlueSky) =>
        {
            "brief to approve".into()
        }
        (AgentStatus::NeedsFeedback, Some(WaitReason::Plan { .. })) => "plan to approve".into(),
        (AgentStatus::NeedsFeedback, Some(WaitReason::Permission { tool })) => {
            format!("needs permission for {tool}")
        }
        (AgentStatus::NeedsFeedback, _) => "waiting for you".into(),
        _ => match draft_pr {
            Some(number) => format!("draft PR #{number} ready"),
            None => "done".into(),
        },
    }
}

/// The kind half of a card's second line: the automation that started
/// the session (`PR fixer`, `Bug fixer` …), else `agent`.
pub fn kind_label(agent: &Agent) -> &'static str {
    orion_core::automation::kind_label(agent.automation.as_deref())
}

/// Why `agent` waits, as the daemon last said (`StatusChanged.reason`,
/// or the exit code its PTY reported).
pub fn reason_of<'a>(app: &'a App, agent: &Agent) -> Option<&'a WaitReason> {
    app.needs_you.reasons.get(&agent.id)
}

/// The draft pull request open on `agent`'s checkout, by number.
fn draft_pr(app: &App, agent: &Agent) -> Option<u64> {
    let pr = app.pull_requests.get(&agent.worktree_id)?.as_ref()?;
    app.prs
        .status_or_open(&pr.url)
        .is_draft()
        .then_some(pr.number)
}

/// Where `agent` sorts in the column, or None when it does not want you.
fn urgency(agent: &Agent) -> Option<u8> {
    match crate::palette::session_tier(agent) {
        PaletteTier::NeedsFeedback if agent.status == AgentStatus::Terminated => Some(CRASHED),
        // A question whose PTY is gone can't be answered: the row is gray.
        PaletteTier::NeedsFeedback if !crate::app::drawn_cold(agent) => Some(WAITING),
        PaletteTier::Unseen => Some(FINISHED),
        _ => None,
    }
}

/// An epoch-ms stamp as a time; 0 (unknown) is the epoch, the longest
/// waiting of all.
fn from_ms(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

/// The AGENTS column's source.
pub struct Agents;

impl CardSource for Agents {
    fn cards(&self, app: &App) -> Vec<Card> {
        let mut cards = Vec::new();
        for project in &app.tree.projects {
            for agent in crate::launcher::project_agents(app, &project.id) {
                let Some(urgency) = urgency(agent) else {
                    continue;
                };
                let reason = reason_text(agent, reason_of(app, agent), draft_pr(app, agent));
                cards.push(Card::new(
                    Section::Agents,
                    CardTarget::Session(agent.id.clone()),
                    agent.name.clone(),
                    format!("{} · {} · {reason}", kind_label(agent), project.name),
                    from_ms(agent.status_changed_at),
                    urgency,
                ));
            }
        }
        cards
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{AgentId, AgentKind, WorktreeId};

    fn agent(status: AgentStatus, unseen: bool) -> Agent {
        Agent {
            id: AgentId("a".into()),
            worktree_id: WorktreeId("w".into()),
            name: "fix login".into(),
            status,
            archived: false,
            archived_at: 0,
            unseen,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: None,
        }
    }

    #[test]
    fn the_kind_is_the_automations_name_or_agent() {
        let mut a = agent(AgentStatus::NeedsFeedback, false);
        assert_eq!(kind_label(&a), "agent");
        a.automation = Some("pr_fixer".into());
        assert_eq!(kind_label(&a), "PR fixer");
    }

    #[test]
    fn the_reason_says_what_the_session_wants_of_you() {
        let crashed = agent(AgentStatus::Terminated, false);
        assert_eq!(reason_text(&crashed, None, None), "crashed");
        let crash = WaitReason::Crash {
            exit_code: Some(137),
        };
        assert_eq!(
            reason_text(&crashed, Some(&crash), None),
            "crashed, exit 137"
        );
        let waiting = agent(AgentStatus::NeedsFeedback, false);
        assert_eq!(reason_text(&waiting, None, None), "waiting for you");
        let question = WaitReason::Question {
            questions: vec![orion_core::Asked {
                question: "Which?".into(),
                header: String::new(),
                options: vec!["A".into(), "B".into()],
                multi: false,
                previews: false,
            }],
        };
        assert_eq!(
            reason_text(&waiting, Some(&question), None),
            "asks a question"
        );
        let plan = WaitReason::Plan {
            plan: "1. …".into(),
        };
        assert_eq!(reason_text(&waiting, Some(&plan), None), "plan to approve");
        let blue_sky = Agent {
            automation: Some("blue_sky".into()),
            ..waiting.clone()
        };
        assert_eq!(
            reason_text(&blue_sky, Some(&plan), None),
            "brief to approve",
            "a blue sky idea's plan is its brief"
        );
        let permission = WaitReason::Permission {
            tool: "Bash".into(),
        };
        assert_eq!(
            reason_text(&waiting, Some(&permission), None),
            "needs permission for Bash"
        );
        let done = agent(AgentStatus::Finished, true);
        assert_eq!(reason_text(&done, None, None), "done");
        assert_eq!(reason_text(&done, None, Some(12)), "draft PR #12 ready");
    }

    /// The column's order: crashed, then waiting — the longest first —
    /// then finished; a session at rest is no card.
    #[test]
    fn the_column_runs_crashed_then_waiting_longest_first_then_finished() {
        use orion_core::{Project, ProjectId, Worktree};
        let mut app = App::new();
        app.tree.projects.push(Project {
            id: ProjectId("p".into()),
            name: "web".into(),
            repo_path: "/tmp/web".into(),
            sort_order: 0,
        });
        app.tree.worktrees.push(Worktree {
            id: WorktreeId("w".into()),
            project_id: ProjectId("p".into()),
            path: "/tmp/web".into(),
            branch: "main".into(),
            is_main: true,
            sort_order: 0,
        });
        let at = |name: &str, status, unseen, stamp| Agent {
            id: AgentId(name.into()),
            name: name.into(),
            status_changed_at: stamp,
            ..agent(status, unseen)
        };
        app.tree.agents = vec![
            at("finished", AgentStatus::Finished, true, 1),
            at("asked later", AgentStatus::NeedsFeedback, false, 300),
            at("resting", AgentStatus::Finished, false, 2),
            at("asked first", AgentStatus::NeedsFeedback, false, 100),
            at("crashed", AgentStatus::Terminated, false, 500),
        ];
        app.needs_you.reasons.insert(
            AgentId("crashed".into()),
            WaitReason::Crash { exit_code: Some(1) },
        );
        app.needs_you.reasons.insert(
            AgentId("asked later".into()),
            WaitReason::Plan {
                plan: "1. …".into(),
            },
        );
        let cards = app.needs_you.store.visible(Agents.cards(&app), Utc::now());
        let order: Vec<(&str, &str)> = cards
            .iter()
            .map(|c| (c.title.as_str(), c.sub.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                ("crashed", "agent · web · crashed, exit 1"),
                ("asked first", "agent · web · waiting for you"),
                ("asked later", "agent · web · plan to approve"),
                ("finished", "agent · web · done"),
            ]
        );
    }

    #[test]
    fn only_sessions_that_want_you_are_cards_crashes_first() {
        assert_eq!(
            urgency(&agent(AgentStatus::Terminated, false)),
            Some(CRASHED)
        );
        assert_eq!(
            urgency(&agent(AgentStatus::NeedsFeedback, false)),
            Some(WAITING)
        );
        assert_eq!(urgency(&agent(AgentStatus::Finished, true)), Some(FINISHED));
        assert_eq!(urgency(&agent(AgentStatus::Finished, false)), None);
        assert_eq!(urgency(&agent(AgentStatus::Running, false)), None);
        let mut gone = agent(AgentStatus::NeedsFeedback, false);
        gone.alive = false;
        assert_eq!(urgency(&gone), None, "a question nobody can answer");
    }
}
