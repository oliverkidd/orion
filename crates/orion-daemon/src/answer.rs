//! ANSWER A WAITING SESSION — `ClientRequest::Answer` and
//! `ClientRequest::LastResponse`.
//!
//! A session red over a question or a plan has Claude Code's dialog on
//! screen, and the answer is the keystrokes that dialog takes, typed into
//! the live PTY as `ClientRequest::Input` would. Verified against Claude
//! Code 2.1.319 driven in a PTY:
//!
//! - **Question** (`AskUserQuestion`, single select): the options are
//!   numbered `1.`…`N.`, then `N+1. Type something.` and `N+2. Chat about
//!   this`. A digit picks its option at once. Your own words: the digit
//!   `N+1` focuses the text row, then the text, then Enter.
//! - **Question, multi select**: a digit toggles its option (the cursor
//!   stays put); → moves to the review tab, whose `1. Submit answers`
//!   sends them. Your own words: ↓ to the `N+1` row from the first (where
//!   a fresh dialog's cursor sits), the text (which ticks it), ↓ to its
//!   `Submit` row, Enter, then `1` on the review — → would only move the
//!   text's caret.
//! - **Plan** (`ExitPlanMode`): `1.` approves (Claude names the mode it
//!   carries on in, auto or manual), `2.` approves with edits approved by
//!   hand, and the last row, `Tell Claude what to change`, takes feedback.
//!   Its number depends on the options Claude offers, but ↑ from the first
//!   row wraps to it: ↑, the text, Enter.
//! - **Permission prompt**: a digit picks its row (`1. Yes`); your own
//!   words are Esc — which refuses the call and stops the turn — and then
//!   the words as the next prompt.
//!
//! Each step is its own write, a beat apart: Claude reads a burst as one
//! paste, which would type `3Use…` into the box instead of focusing row 3.
//!
//! With no dialog to answer — the session isn't running, the daemon
//! restarted since it asked, or it is red over something else — the answer
//! goes in as the session's next turn, in words (`Daemon::send_turn`).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use orion_core::{AgentId, AgentStatus, Answer, SessionRef, WaitReason};

use crate::pty::{DEFAULT_COLS, DEFAULT_ROWS};
use crate::registry::{resolve_harness, typed_bytes, Daemon};

/// The pause between one dialog keystroke and the next.
const KEY_GAP: Duration = Duration::from_millis(300);

const ENTER: &[u8] = b"\r";
const ESC: &[u8] = b"\x1b";
const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const RIGHT: &[u8] = b"\x1b[C";

/// The words a plan approval goes in as when there is no dialog to press.
pub const PLAN_APPROVED_TURN: &str = "Your plan is approved. Go ahead with it.";

/// The writes that answer `reason`'s dialog with `answer`, in order. An
/// error when the answer doesn't fit the dialog (an option it doesn't
/// have, a plan approval for a question); None when the reason is no
/// dialog at all (a crash).
pub fn dialog_keys(reason: &WaitReason, answer: &Answer) -> Result<Option<Vec<Vec<u8>>>> {
    let keys = match (reason, answer) {
        (WaitReason::Question { options, multi, .. }, Answer::Option(i)) => {
            if *i >= options.len() {
                bail!("the question has no option {}", i + 1);
            }
            let pick = digit(i + 1)?;
            match multi {
                true => vec![pick, RIGHT.to_vec(), b"1".to_vec()],
                false => vec![pick],
            }
        }
        (
            WaitReason::Question {
                options,
                multi: false,
                ..
            },
            Answer::Text(text) | Answer::Steer(text),
        ) => vec![digit(options.len() + 1)?, typed_bytes(text), ENTER.to_vec()],
        // A multi-select digit only toggles, and → inside the text row
        // moves its caret: walk down to the row, type (which ticks it),
        // then down to its `Submit` row and on to the review's submit.
        (
            WaitReason::Question {
                options,
                multi: true,
                ..
            },
            Answer::Text(text) | Answer::Steer(text),
        ) => {
            let mut keys = vec![DOWN.to_vec(); options.len()];
            keys.extend([
                typed_bytes(text),
                DOWN.to_vec(),
                ENTER.to_vec(),
                b"1".to_vec(),
            ]);
            keys
        }
        (WaitReason::Question { .. }, Answer::ApprovePlan) => {
            bail!("the session is asking a question, not waiting on a plan")
        }
        (WaitReason::Plan { .. }, Answer::ApprovePlan) => vec![b"1".to_vec()],
        (WaitReason::Plan { .. }, Answer::Option(i)) => vec![digit(i + 1)?],
        (WaitReason::Plan { .. }, Answer::Text(text) | Answer::Steer(text)) => {
            vec![UP.to_vec(), typed_bytes(text), ENTER.to_vec()]
        }
        (WaitReason::Permission { .. }, Answer::ApprovePlan) => vec![b"1".to_vec()],
        (WaitReason::Permission { .. }, Answer::Option(i)) => vec![digit(i + 1)?],
        (WaitReason::Permission { .. }, Answer::Text(text) | Answer::Steer(text)) => {
            vec![ESC.to_vec(), typed_bytes(text), ENTER.to_vec()]
        }
        (WaitReason::Crash { .. }, _) => return Ok(None),
    };
    Ok(Some(keys))
}

/// The digit that picks dialog row `n` (1-based). Claude's dialogs number
/// their rows 1–9 (a question has at most four options).
fn digit(n: usize) -> Result<Vec<u8>> {
    match n {
        1..=9 => Ok(n.to_string().into_bytes()),
        _ => bail!("dialog row {n} has no digit to pick it"),
    }
}

/// The answer in words, for a session with no dialog to press: a picked
/// option by its label when the reason names it.
pub fn turn_text(reason: Option<&WaitReason>, answer: &Answer) -> String {
    match answer {
        Answer::Text(text) | Answer::Steer(text) => text.clone(),
        Answer::ApprovePlan => PLAN_APPROVED_TURN.to_string(),
        Answer::Option(i) => match reason {
            Some(WaitReason::Question { options, .. }) if *i < options.len() => {
                format!("Answer to your question: {}", options[*i])
            }
            Some(WaitReason::Plan { .. }) if *i == 0 => PLAN_APPROVED_TURN.to_string(),
            _ => format!("Answer to your question: option {}", i + 1),
        },
    }
}

impl Daemon {
    /// Answer agent `id`, waiting on you (`ClientRequest::Answer`): the
    /// dialog's keystrokes while it is live and red over a question, plan
    /// or permission prompt this daemon saw open; otherwise the answer as
    /// its next turn, in words.
    pub async fn answer(self: &Arc<Self>, id: &AgentId, answer: Answer) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        let open_dialog = (agent.status == AgentStatus::NeedsFeedback)
            .then(|| self.wait_reasons.lock().unwrap().get(id).cloned())
            .flatten();
        let session = self.session(&SessionRef::Agent(id.clone()));
        if let (Some(session), Some(reason)) = (session, &open_dialog) {
            if let Some(keys) = dialog_keys(reason, &answer)? {
                tracing::info!(agent = %id, ?answer, "answering the open dialog");
                for (n, key) in keys.iter().enumerate() {
                    if n > 0 {
                        tokio::time::sleep(KEY_GAP).await;
                    }
                    session.write_input(key)?;
                }
                return Ok(());
            }
        }
        let reason = match open_dialog {
            Some(reason) => Some(reason),
            None => self.wait_reason(id),
        };
        let text = turn_text(reason.as_ref(), &answer);
        self.send_turn(id, &text, DEFAULT_COLS, DEFAULT_ROWS)
    }

    /// What agent `id` last said, and why it waits on you if it does
    /// (`ClientRequest::LastResponse`). Reads its transcript, so callers
    /// keep it off the async workers.
    pub fn last_response(&self, id: &AgentId) -> (Option<String>, Option<WaitReason>) {
        let text = self
            .transcript_path(id)
            .and_then(|path| crate::transcript::last_assistant_text(&path));
        (text, self.wait_reason(id))
    }

    /// Why agent `id` waits on you: the reason the hook that turned it red
    /// gave, or — the daemon having restarted since — the dialog its
    /// transcript ends on, while the row still says it waits (red, or cut
    /// off mid-wait by the restart).
    pub fn wait_reason(&self, id: &AgentId) -> Option<WaitReason> {
        if let Some(reason) = self.wait_reasons.lock().unwrap().get(id) {
            return Some(reason.clone());
        }
        let agent = self.store.get_agent(id).ok()??;
        if !matches!(
            agent.status,
            AgentStatus::NeedsFeedback | AgentStatus::Disconnected
        ) {
            return None;
        }
        crate::transcript::pending_wait_reason(&self.transcript_path(id)?)
    }

    /// Where agent `id`'s Claude transcript is: as its hooks last reported
    /// it, else found by its session id where its harness keeps them.
    pub(crate) fn transcript_path(&self, id: &AgentId) -> Option<std::path::PathBuf> {
        if let Some(transcript) = self.transcripts.lock().unwrap().get(id) {
            return Some(transcript.transcript_path.clone());
        }
        let agent = self.store.get_agent(id).ok()??;
        let session_id = agent.session_id.as_deref()?;
        let harness = resolve_harness(agent.kind, agent.custom_harness.as_deref()).ok()?;
        let roots = self.claude_projects_roots(&agent, &harness)?;
        crate::continue_on::find_transcript(&roots, session_id).map(|(_, file)| file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(multi: bool) -> WaitReason {
        WaitReason::Question {
            question: "Which color?".into(),
            options: vec!["Red".into(), "Green".into(), "Blue".into()],
            multi,
        }
    }

    fn keys(reason: &WaitReason, answer: Answer) -> Vec<Vec<u8>> {
        dialog_keys(reason, &answer).unwrap().unwrap()
    }

    #[test]
    fn a_question_takes_digits_and_its_type_something_row() {
        assert_eq!(
            keys(&question(false), Answer::Option(1)),
            vec![b"2".to_vec()]
        );
        assert_eq!(
            keys(&question(false), Answer::Text("purple".into())),
            vec![b"4".to_vec(), b"purple".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(
            keys(&question(true), Answer::Option(2)),
            vec![b"3".to_vec(), b"\x1b[C".to_vec(), b"1".to_vec()]
        );
        assert_eq!(
            keys(&question(true), Answer::Text("mango".into())),
            vec![
                b"\x1b[B".to_vec(),
                b"\x1b[B".to_vec(),
                b"\x1b[B".to_vec(),
                b"mango".to_vec(),
                b"\x1b[B".to_vec(),
                b"\r".to_vec(),
                b"1".to_vec(),
            ]
        );
        assert!(dialog_keys(&question(false), &Answer::Option(3)).is_err());
        assert!(dialog_keys(&question(false), &Answer::ApprovePlan).is_err());
    }

    #[test]
    fn a_plan_approves_with_one_and_steers_from_its_last_row() {
        let plan = WaitReason::Plan {
            plan: "Do it.".into(),
        };
        assert_eq!(keys(&plan, Answer::ApprovePlan), vec![b"1".to_vec()]);
        assert_eq!(
            keys(&plan, Answer::Steer("also back up".into())),
            vec![b"\x1b[A".to_vec(), b"also back up".to_vec(), b"\r".to_vec()]
        );
        // A multi-line steer goes in as one paste.
        let lines = keys(&plan, Answer::Steer("a\nb".into()));
        assert_eq!(lines[1], b"\x1b[200~a\nb\x1b[201~".to_vec());
    }

    #[test]
    fn a_permission_prompt_refuses_with_esc_before_words() {
        let bash = WaitReason::Permission {
            tool: "Bash".into(),
        };
        assert_eq!(keys(&bash, Answer::Option(0)), vec![b"1".to_vec()]);
        assert_eq!(
            keys(&bash, Answer::Text("use make".into())),
            vec![b"\x1b".to_vec(), b"use make".to_vec(), b"\r".to_vec()]
        );
        assert_eq!(
            dialog_keys(
                &WaitReason::Crash { exit_code: Some(1) },
                &Answer::Option(0)
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn with_no_dialog_the_answer_is_a_turn_in_words() {
        assert_eq!(
            turn_text(Some(&question(false)), &Answer::Option(1)),
            "Answer to your question: Green"
        );
        assert_eq!(
            turn_text(None, &Answer::Option(1)),
            "Answer to your question: option 2"
        );
        assert_eq!(turn_text(None, &Answer::ApprovePlan), PLAN_APPROVED_TURN);
        assert_eq!(turn_text(None, &Answer::Steer("go".into())), "go");
    }
}
