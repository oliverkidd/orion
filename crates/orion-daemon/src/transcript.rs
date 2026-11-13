//! TRANSCRIPT READER — what a Claude session last said, read off its
//! transcript (`<projects dir>/<slug>/<session id>.jsonl`).
//!
//! Claude writes one JSONL line per content block: an assistant message
//! that thinks, says something and calls a tool is three
//! `{"type":"assistant"}` lines sharing one `message.id`, each holding one
//! block. So "the last response" is the newest assistant message that has
//! any text, with every text block of that message joined — not merely the
//! last line, which is as often a bare `tool_use`. Subagent traffic
//! (`isSidechain`) is never the session's own words.
//!
//! Only the file's tail is read: the window starts at the title reader's
//! (`session_title::TAIL_BYTES`) and doubles up to [`MAX_TAIL_BYTES`] while
//! no text turns up in it.

use std::path::Path;

use orion_core::WaitReason;
use serde_json::Value;

use crate::session_title::{transcript_tail_within, TAIL_BYTES};

/// The most of a transcript's tail read for the last response. A single
/// response with a large tool result after it fits; a transcript whose
/// last megabyte has no assistant text has nothing worth showing.
pub const MAX_TAIL_BYTES: u64 = 1024 * 1024;

/// The text of the session's last assistant message that has any, its
/// text blocks joined by blank lines. None when the transcript can't be
/// read or its last [`MAX_TAIL_BYTES`] hold no assistant text.
pub fn last_assistant_text(path: &Path) -> Option<String> {
    let mut window = TAIL_BYTES;
    loop {
        let (tail, whole) = transcript_tail_within(path, window)?;
        if let Some(text) = last_text_in(&tail) {
            return Some(text);
        }
        if whole || window >= MAX_TAIL_BYTES {
            return None;
        }
        window = (window * 2).min(MAX_TAIL_BYTES);
    }
}

/// The dialog the transcript ends on, when it ends on one: the newest
/// conversation line is an assistant `AskUserQuestion` or `ExitPlanMode`
/// call with no result after it. How a daemon that restarted rebuilds why
/// a session is waiting, having lost the hook that said so.
pub fn pending_wait_reason(path: &Path) -> Option<WaitReason> {
    let (tail, _) = transcript_tail_within(path, TAIL_BYTES)?;
    let line = tail
        .lines()
        .rev()
        .filter_map(own_line)
        .find(|line| matches!(line_type(line), Some("assistant" | "user")))?;
    if line_type(&line) != Some("assistant") {
        return None;
    }
    let call = blocks(&line)
        .rev()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))?;
    let tool = call.get("name")?.as_str()?;
    crate::status::opens_dialog(Some(tool)).then(|| WaitReason::from_tool(tool, call.get("input")))
}

/// The newest assistant message with text in `tail`, joined.
fn last_text_in(tail: &str) -> Option<String> {
    let mut message: Option<Value> = None;
    let mut texts: Vec<String> = Vec::new();
    for line in tail.lines().rev().filter_map(own_line) {
        if line_type(&line) != Some("assistant") {
            continue;
        }
        let id = line.pointer("/message/id").cloned();
        match &message {
            // Older than the message being collected: it is complete.
            Some(collecting) if Some(collecting) != id.as_ref() => break,
            _ => {}
        }
        let mut line_texts: Vec<String> = blocks(&line)
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        if line_texts.is_empty() {
            continue;
        }
        if message.is_none() {
            message = Some(id.unwrap_or(Value::Null));
        }
        // Walking backwards: this line's blocks come before those already
        // collected.
        line_texts.append(&mut texts);
        texts = line_texts;
    }
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

/// A whole transcript line of the session's own conversation, parsed. The
/// tail's cut first line and subagent lines are None.
fn own_line(line: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(line).ok()?;
    let sidechain = value
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (!sidechain).then_some(value)
}

fn line_type(line: &Value) -> Option<&str> {
    line.get("type")?.as_str()
}

/// The content blocks of a line's message.
fn blocks(line: &Value) -> impl DoubleEndedIterator<Item = &Value> {
    line.pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(id: &str, block: Value) -> String {
        json!({"type": "assistant", "isSidechain": false,
               "message": {"id": id, "role": "assistant", "content": [block]}})
        .to_string()
    }

    fn text(t: &str) -> Value {
        json!({"type": "text", "text": t})
    }

    fn tool_use(name: &str, input: Value) -> Value {
        json!({"type": "tool_use", "id": "toolu_1", "name": name, "input": input})
    }

    fn user_result() -> String {
        json!({"type": "user", "message": {"role": "user",
               "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}})
        .to_string()
    }

    fn write(lines: &[String]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        (dir, path)
    }

    #[test]
    fn text_only_transcript_reads_its_last_message() {
        let (_d, path) = write(&[
            json!({"type": "user", "message": {"role": "user", "content": "hi"}}).to_string(),
            assistant("m1", text("First answer.")),
            json!({"type": "user", "message": {"role": "user", "content": "again"}}).to_string(),
            assistant("m2", json!({"type": "thinking", "thinking": "hmm"})),
            assistant("m2", text("Second answer.")),
            json!({"type": "system", "subtype": "turn_duration"}).to_string(),
        ]);
        assert_eq!(
            last_assistant_text(&path).as_deref(),
            Some("Second answer.")
        );
    }

    #[test]
    fn tool_only_transcript_has_no_last_response() {
        let (_d, path) = write(&[
            assistant("m1", tool_use("Bash", json!({"command": "ls"}))),
            user_result(),
            assistant("m2", tool_use("Read", json!({"file_path": "/x"}))),
        ]);
        assert_eq!(last_assistant_text(&path), None);
        assert_eq!(pending_wait_reason(&path), None);
    }

    #[test]
    fn mixed_transcript_joins_the_messages_text_and_skips_tool_lines() {
        let (_d, path) = write(&[
            assistant("m1", text("Old turn.")),
            assistant("m2", text("Looking at the tests.")),
            assistant("m2", tool_use("Bash", json!({"command": "cargo test"}))),
            user_result(),
            assistant("m3", text("Two failures.")),
            assistant("m3", text("Which should I fix?")),
            assistant(
                "m3",
                tool_use(
                    "AskUserQuestion",
                    json!({"questions": [{"question": "Fix which?", "multiSelect": false,
                       "options": [{"label": "Both"}, {"label": "Neither"}]}]}),
                ),
            ),
            // A subagent's words are not the session's.
            json!({"type": "assistant", "isSidechain": true,
                   "message": {"id": "sub", "content": [text("subagent chatter")]}})
            .to_string(),
        ]);
        assert_eq!(
            last_assistant_text(&path).as_deref(),
            Some("Two failures.\n\nWhich should I fix?")
        );
        assert_eq!(
            pending_wait_reason(&path),
            Some(WaitReason::Question {
                questions: vec![orion_core::Asked {
                    question: "Fix which?".into(),
                    header: String::new(),
                    options: vec!["Both".into(), "Neither".into()],
                    multi: false,
                    previews: false,
                }]
            })
        );
        // A tool-only newest message falls back to the last one with text.
        let (_d, path) = write(&[
            assistant("m1", text("Running the build.")),
            assistant("m1", tool_use("Bash", json!({"command": "make"}))),
            user_result(),
            assistant("m2", tool_use("Read", json!({"file_path": "/x"}))),
        ]);
        assert_eq!(
            last_assistant_text(&path).as_deref(),
            Some("Running the build.")
        );
    }

    #[test]
    fn an_answered_dialog_is_not_pending() {
        let (_d, path) = write(&[
            assistant("m1", tool_use("ExitPlanMode", json!({"plan": "Do it."}))),
            user_result(),
        ]);
        assert_eq!(pending_wait_reason(&path), None);
        let (_d, path) = write(&[assistant(
            "m1",
            tool_use("ExitPlanMode", json!({"plan": "Do it."})),
        )]);
        assert_eq!(
            pending_wait_reason(&path),
            Some(WaitReason::Plan {
                plan: "Do it.".into()
            })
        );
    }

    /// The window grows past a large tail (a big tool result) to find the
    /// text before it, up to the cap.
    #[test]
    fn the_window_grows_past_a_large_tool_result() {
        let big = "x".repeat(3 * TAIL_BYTES as usize);
        let (_d, path) = write(&[
            assistant("m1", text("Before the dump.")),
            json!({"type": "user", "message": {"role": "user",
                   "content": [{"type": "tool_result", "content": big}]}})
            .to_string(),
        ]);
        assert_eq!(
            last_assistant_text(&path).as_deref(),
            Some("Before the dump.")
        );
        let huge = "x".repeat(MAX_TAIL_BYTES as usize + 1);
        let (_d, path) = write(&[
            assistant("m1", text("Too far back.")),
            json!({"type": "user", "message": {"role": "user",
                   "content": [{"type": "tool_result", "content": huge}]}})
            .to_string(),
        ]);
        assert_eq!(last_assistant_text(&path), None);
    }
}
