//! ANSWER A WAITING SESSION — `ClientRequest::Answer` and
//! `ClientRequest::LastResponse`.
//!
//! A session red over a question or a plan has Claude Code's dialog on
//! screen, and the answer is the keystrokes that dialog takes, typed into
//! the live PTY as `ClientRequest::Input` would. Verified against Claude
//! Code 2.1.319, and again on 2.1.322 (with the several-question dialog),
//! driven in a PTY:
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
//! - **Several questions in one ask**: a tab per question (`← ☐ Color
//!   ☐ Fruits ✔ Submit →`), each answered as above but moving on instead
//!   of submitting — a single-select digit, or its own words' Enter, moves
//!   to the next tab; on a multi-select tab → does, or Enter on the row
//!   under its own words (`Next`, `Submit` on the last). The cursor sits
//!   on row 1 of every tab it arrives at. After the last tab, the review's
//!   `1. Submit answers` sends them all. A lone single-select question has
//!   no tabs and no review; a lone multi-select one has both.
//! - **An answer that leaves a question out** (a plain DM on a three-part
//!   ask): Esc on any tab cancels the whole dialog ("Interrupted · What
//!   should Claude do instead?"), and the words go in as the next prompt
//!   with the questions quoted above them (`quoted_turn`).
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
use orion_core::{AgentId, AgentStatus, Answer, Asked, Picked, SessionRef, WaitReason};

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
        (WaitReason::Question { .. }, Answer::ApprovePlan) => {
            bail!("the session is asking a question, not waiting on a plan")
        }
        (WaitReason::Question { questions }, answer) => question_keys(questions, answer)?,
        (_, Answer::Answers(_)) => bail!("the session isn't asking a question"),
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

/// The writes that answer an `AskUserQuestion` dialog: every question in
/// turn, then the review tab's `1. Submit answers` when there is one. An
/// answer that leaves a question out cancels the dialog with Esc instead
/// and sends what it says as the next turn, the questions quoted — never
/// a dialog left half-answered on a later tab.
fn question_keys(questions: &[Asked], answer: &Answer) -> Result<Vec<Vec<u8>>> {
    let picks: Vec<Picked> = match answer {
        Answer::Answers(picks) if picks.len() == questions.len() => picks.clone(),
        Answer::Option(i) if questions.len() == 1 => vec![Picked::Options(vec![*i])],
        Answer::Text(text) | Answer::Steer(text) if questions.len() == 1 => {
            vec![Picked::Text(text.clone())]
        }
        partial => {
            let words = answer_words(questions, partial);
            return Ok(vec![
                ESC.to_vec(),
                typed_bytes(&quoted_turn(questions, &words)),
                ENTER.to_vec(),
            ]);
        }
    };
    let mut keys = Vec::new();
    for (n, (asked, picked)) in questions.iter().zip(&picks).enumerate() {
        keys.extend(tab_keys(asked, picked).with_context(|| format!("question {}", n + 1))?);
    }
    // A lone single-select question submits on its pick; any other dialog
    // has tabs, and a review tab after them.
    if questions.len() > 1 || questions[0].multi {
        keys.push(b"1".to_vec());
    }
    Ok(keys)
}

/// The writes that answer one question's tab and move on from it.
fn tab_keys(asked: &Asked, picked: &Picked) -> Result<Vec<Vec<u8>>> {
    let rows = asked.options.len();
    Ok(match (asked.multi, picked) {
        (_, Picked::Text(text)) if text.trim().is_empty() => bail!("no answer"),
        (_, Picked::Options(picks)) if picks.is_empty() => bail!("no option picked"),
        (_, Picked::Options(picks)) if picks.iter().any(|i| *i >= rows) => {
            bail!("no option {}", picks.iter().max().unwrap_or(&0) + 1)
        }
        // A digit picks its option and moves on.
        (false, Picked::Options(picks)) if picks.len() == 1 => vec![digit(picks[0] + 1)?],
        (false, Picked::Options(_)) => bail!("one option only"),
        // Each digit toggles its option; → moves on.
        (true, Picked::Options(picks)) => {
            let mut picks = picks.clone();
            picks.sort_unstable();
            picks.dedup();
            let mut keys = picks
                .iter()
                .map(|i| digit(i + 1))
                .collect::<Result<Vec<_>>>()?;
            keys.push(RIGHT.to_vec());
            keys
        }
        // The `N+1` digit focuses the text row; Enter moves on.
        (false, Picked::Text(text)) => vec![digit(rows + 1)?, typed_bytes(text), ENTER.to_vec()],
        // A digit only toggles, and → inside the text row moves its caret:
        // walk down to the row from the first (where the cursor sits on
        // arrival), type (which ticks it), then down to its `Next` or
        // `Submit` row and Enter.
        (true, Picked::Text(text)) => {
            let mut keys = vec![DOWN.to_vec(); rows];
            keys.extend([typed_bytes(text), DOWN.to_vec(), ENTER.to_vec()]);
            keys
        }
    })
}

/// The digit that picks dialog row `n` (1-based). Claude's dialogs number
/// their rows 1–9 (a question has at most four options).
fn digit(n: usize) -> Result<Vec<u8>> {
    match n {
        1..=9 => Ok(n.to_string().into_bytes()),
        _ => bail!("dialog row {n} has no digit to pick it"),
    }
}

/// What one question's answer says, in words: the labels picked, joined,
/// or your own.
pub fn picked_words(asked: Option<&Asked>, picked: &Picked) -> String {
    match picked {
        Picked::Text(text) => text.trim().to_string(),
        Picked::Options(picks) => picks
            .iter()
            .map(|i| match asked.and_then(|a| a.options.get(*i)) {
                Some(label) => label.clone(),
                None => format!("option {}", i + 1),
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// What `answer` says to `questions`, in words, all on one line.
fn answer_words(questions: &[Asked], answer: &Answer) -> String {
    match answer {
        Answer::Text(text) | Answer::Steer(text) => text.clone(),
        Answer::ApprovePlan => PLAN_APPROVED_TURN.to_string(),
        Answer::Option(i) => picked_words(questions.first(), &Picked::Options(vec![*i])),
        Answer::Answers(picks) => picks
            .iter()
            .enumerate()
            .map(|(n, p)| picked_words(questions.get(n), p))
            .collect::<Vec<_>>()
            .join("; "),
    }
}

/// `words` as the next turn, after the questions they answer:
///
/// ```text
/// You asked:
/// 1. <q>
/// 2. <q>
/// My answer: <words>
/// ```
pub fn quoted_turn(questions: &[Asked], words: &str) -> String {
    format!("{}\nMy answer: {words}", asked_lines(questions))
}

/// `You asked:` and the questions, numbered, one per line.
fn asked_lines(questions: &[Asked]) -> String {
    let mut lines = vec!["You asked:".to_string()];
    lines.extend(
        questions
            .iter()
            .enumerate()
            .map(|(n, asked)| format!("{}. {}", n + 1, asked.question)),
    );
    lines.join("\n")
}

/// The answer in words, for a session with no dialog to press: a picked
/// option by its label when the reason names it, and a several-question
/// ask's answers after the questions they answer.
pub fn turn_text(reason: Option<&WaitReason>, answer: &Answer) -> String {
    let questions = match reason {
        Some(WaitReason::Question { questions }) => questions.as_slice(),
        _ => &[],
    };
    match answer {
        Answer::Text(text) | Answer::Steer(text) if questions.len() <= 1 => text.clone(),
        Answer::ApprovePlan => PLAN_APPROVED_TURN.to_string(),
        Answer::Option(0) if matches!(reason, Some(WaitReason::Plan { .. })) => {
            PLAN_APPROVED_TURN.to_string()
        }
        Answer::Option(_) if questions.len() <= 1 => {
            format!(
                "Answer to your question: {}",
                answer_words(questions, answer)
            )
        }
        Answer::Answers(picks) if picks.len() == questions.len() && questions.len() > 1 => {
            let answers: Vec<String> = questions
                .iter()
                .zip(picks)
                .enumerate()
                .map(|(n, (asked, picked))| {
                    format!("{}. {}", n + 1, picked_words(Some(asked), picked))
                })
                .collect();
            format!(
                "{}\nMy answers:\n{}",
                asked_lines(questions),
                answers.join("\n")
            )
        }
        Answer::Answers(_) if questions.len() <= 1 => {
            format!(
                "Answer to your question: {}",
                answer_words(questions, answer)
            )
        }
        _ => quoted_turn(questions, &answer_words(questions, answer)),
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

    fn asked(question: &str, options: &[&str], multi: bool) -> Asked {
        Asked {
            question: question.into(),
            header: String::new(),
            options: options.iter().map(|o| o.to_string()).collect(),
            multi,
        }
    }

    fn question(multi: bool) -> WaitReason {
        WaitReason::Question {
            questions: vec![asked("Which color?", &["Red", "Green", "Blue"], multi)],
        }
    }

    /// The two- and three-question asks driven on Claude Code 2.1.322.
    fn several() -> WaitReason {
        WaitReason::Question {
            questions: vec![
                asked("Which color do you like?", &["Red", "Green", "Blue"], false),
                asked(
                    "Which fruits do you like?",
                    &["Apple", "Pear", "Mango"],
                    true,
                ),
            ],
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

    fn bytes(keys: &[&str]) -> Vec<Vec<u8>> {
        keys.iter().map(|k| k.as_bytes().to_vec()).collect()
    }

    #[test]
    fn several_questions_answer_tab_by_tab_then_submit_on_the_review() {
        // Green, then Apple and Mango ticked and → on, then the review.
        assert_eq!(
            keys(
                &several(),
                Answer::Answers(vec![
                    Picked::Options(vec![1]),
                    Picked::Options(vec![2, 0, 2])
                ])
            ),
            bytes(&["2", "1", "3", "\x1b[C", "1"])
        );
        // Own words on both: the single tab's `Type something.` row by its
        // digit, the multi tab's by ↓ from row 1, then ↓ to `Submit`.
        assert_eq!(
            keys(
                &several(),
                Answer::Answers(vec![
                    Picked::Text("purple".into()),
                    Picked::Text("kiwi".into())
                ])
            ),
            bytes(&[
                "4", "purple", "\r", "\x1b[B", "\x1b[B", "\x1b[B", "kiwi", "\x1b[B", "\r", "1"
            ])
        );
        // A lone multi-select question has its review too; a lone single
        // select doesn't.
        assert_eq!(
            keys(
                &question(true),
                Answer::Answers(vec![Picked::Options(vec![0, 1])])
            ),
            bytes(&["1", "2", "\x1b[C", "1"])
        );
        assert_eq!(
            keys(
                &question(false),
                Answer::Answers(vec![Picked::Options(vec![2])])
            ),
            bytes(&["3"])
        );
        for wrong in [
            vec![Picked::Options(vec![3]), Picked::Options(vec![0])],
            vec![Picked::Options(vec![0, 1]), Picked::Options(vec![0])],
            vec![Picked::Options(vec![0]), Picked::Options(vec![])],
            vec![Picked::Options(vec![0]), Picked::Text(" ".into())],
        ] {
            assert!(dialog_keys(&several(), &Answer::Answers(wrong)).is_err());
        }
        assert!(dialog_keys(&question(false), &Answer::ApprovePlan).is_err());
    }

    /// An answer that leaves a question out never types into the dialog:
    /// Esc cancels it and the words go in as the next turn, the questions
    /// quoted.
    #[test]
    fn a_partial_answer_cancels_the_dialog_and_quotes_the_questions() {
        let quoted = "You asked:\n1. Which color do you like?\n2. Which fruits do you like?\n\
                      My answer: green, and pears";
        assert_eq!(
            keys(&several(), Answer::Text("green, and pears".into())),
            vec![
                b"\x1b".to_vec(),
                format!("\x1b[200~{quoted}\x1b[201~").into_bytes(),
                b"\r".to_vec()
            ]
        );
        let picked = keys(&several(), Answer::Option(2));
        assert_eq!(picked[0], b"\x1b".to_vec());
        assert!(String::from_utf8_lossy(&picked[1]).ends_with("My answer: Blue\x1b[201~"));
        let short = keys(&several(), Answer::Answers(vec![Picked::Options(vec![0])]));
        assert!(String::from_utf8_lossy(&short[1]).ends_with("My answer: Red\x1b[201~"));
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
        assert_eq!(
            turn_text(
                Some(&several()),
                &Answer::Answers(vec![Picked::Options(vec![1]), Picked::Options(vec![0, 2])])
            ),
            "You asked:\n1. Which color do you like?\n2. Which fruits do you like?\n\
             My answers:\n1. Green\n2. Apple, Mango"
        );
        assert_eq!(
            turn_text(Some(&several()), &Answer::Text("both red".into())),
            "You asked:\n1. Which color do you like?\n2. Which fruits do you like?\n\
             My answer: both red"
        );
        assert_eq!(
            turn_text(
                Some(&question(true)),
                &Answer::Answers(vec![Picked::Options(vec![0, 2])])
            ),
            "Answer to your question: Red, Blue"
        );
    }
}
