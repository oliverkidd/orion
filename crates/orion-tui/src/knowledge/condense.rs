//! CONDENSING a transcript: what of an agent session a harvest reads.
//! The owner's own words, the questions put to him and his answers, the
//! plans, the agent's prose, the commits and PR commands it ran and what
//! it wrote to docs. Tool output is dropped. A port of the reference
//! script `kcondense.py` (the plan's appendix), for Claude Code's JSONL
//! and Cursor's agent transcripts.

use std::collections::HashMap;

use serde_json::Value;

use crate::ui::truncate;

/// How much of one owner message is kept.
pub(crate) const OWNER_CUT: usize = 6_000;
/// How much of one piece of agent prose, command or doc write is kept.
pub(crate) const AGENT_CUT: usize = 1_800;

/// User text that is a wrapper Claude Code or the harness put there, not
/// something the owner typed.
const NOT_OWNER: &[&str] = &[
    "<task-notification",
    "<local-command",
    "<command-name",
    "<system-reminder",
    "Caveat:",
    "[SYSTEM",
    "<bash-",
    "<user-memory-input",
];

/// Paths a write to counts as a write to docs.
const DOC_PATHS: &[&str] = &["docs/", "agent_docs/", "CLAUDE.md", "AGENTS.md", "memory/"];

/// Who said a condensed line, and so the label it is read under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Who {
    Owner,
    /// His answer to an AskUserQuestion (`question`) or an ExitPlanMode
    /// (`plan approval`).
    OwnerAnswered(&'static str),
    OwnerRejected,
    Agent,
    AgentAsked,
    AgentPlan,
    AgentRan,
    AgentWrote(String),
}

impl Who {
    /// The label the line is written under in the dossier.
    pub(crate) fn label(&self) -> String {
        match self {
            Who::Owner => "OWNER".into(),
            Who::OwnerAnswered(what) => format!("OWNER ANSWERED ({what})"),
            Who::OwnerRejected => "OWNER REJECTED A TOOL CALL".into(),
            Who::Agent => "AGENT".into(),
            Who::AgentAsked => "AGENT ASKED".into(),
            Who::AgentPlan => "AGENT PLAN".into(),
            Who::AgentRan => "AGENT RAN".into(),
            Who::AgentWrote(path) => format!("AGENT WROTE {path}"),
        }
    }

    /// Whether the owner said it — what a quote credited to him must
    /// come from.
    pub(crate) fn is_owner(&self) -> bool {
        matches!(
            self,
            Who::Owner | Who::OwnerAnswered(_) | Who::OwnerRejected
        )
    }
}

/// One kept line of a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub who: Who,
    pub text: String,
}

impl Line {
    fn new(who: Who, text: &str, cut: usize) -> Option<Self> {
        let text = text.trim();
        (!text.is_empty()).then(|| Line {
            who,
            text: truncate(text, cut),
        })
    }

    pub(crate) fn render(&self) -> String {
        format!("{}: {}", self.who.label(), self.text)
    }
}

/// Which CLI wrote a transcript.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Shape {
    Claude,
    Cursor,
}

/// `jsonl`, condensed by its shape.
pub(crate) fn condense(shape: Shape, jsonl: &str) -> Vec<Line> {
    match shape {
        Shape::Claude => claude(jsonl),
        Shape::Cursor => cursor(jsonl),
    }
}

/// A Claude Code transcript: one JSON record a line, the side chains
/// (subagents) left out.
pub(crate) fn claude(jsonl: &str) -> Vec<Line> {
    let mut out = Vec::new();
    // The tool uses whose results are the owner's answers, by id.
    let mut answers: HashMap<String, &'static str> = HashMap::new();
    for record in records(jsonl) {
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || record.get("isMeta").and_then(Value::as_bool) == Some(true)
            || record.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let content = &record["message"]["content"];
        match record.get("type").and_then(Value::as_str) {
            Some("user") => claude_user(content, &answers, &mut out),
            Some("assistant") => claude_assistant(content, &mut answers, &mut out),
            _ => {}
        }
    }
    out
}

fn claude_user(content: &Value, answers: &HashMap<String, &'static str>, out: &mut Vec<Line>) {
    if let Some(text) = content.as_str() {
        out.extend(owner_text(text));
        return;
    }
    for item in content.as_array().into_iter().flatten() {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => out.extend(owner_text(str_of(item, "text"))),
            Some("tool_result") => {
                let id = str_of(item, "tool_use_id");
                let said = result_text(&item["content"]);
                if let Some(what) = answers.get(id) {
                    out.extend(Line::new(Who::OwnerAnswered(what), &said, OWNER_CUT));
                } else if item.get("is_error").and_then(Value::as_bool) == Some(true)
                    && said.contains("rejected")
                {
                    out.extend(Line::new(Who::OwnerRejected, &said, OWNER_CUT));
                }
            }
            _ => {}
        }
    }
}

fn claude_assistant(
    content: &Value,
    answers: &mut HashMap<String, &'static str>,
    out: &mut Vec<Line>,
) {
    for item in content.as_array().into_iter().flatten() {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => out.extend(Line::new(Who::Agent, str_of(item, "text"), AGENT_CUT)),
            Some("tool_use") => {
                let input = &item["input"];
                let id = str_of(item, "id").to_string();
                match str_of(item, "name") {
                    "AskUserQuestion" => {
                        answers.insert(id, "question");
                        out.extend(Line::new(Who::AgentAsked, &questions(input), OWNER_CUT));
                    }
                    "ExitPlanMode" => {
                        answers.insert(id, "plan approval");
                        out.extend(Line::new(Who::AgentPlan, str_of(input, "plan"), OWNER_CUT));
                    }
                    "Bash" => {
                        let command = str_of(input, "command");
                        if is_kept_command(command) {
                            out.extend(Line::new(Who::AgentRan, command, AGENT_CUT));
                        }
                    }
                    "Write" | "Edit" | "MultiEdit" => {
                        let path = str_of(input, "file_path");
                        if is_doc_path(path) {
                            let body = match str_of(input, "content") {
                                "" => str_of(input, "new_string"),
                                body => body,
                            };
                            out.extend(Line::new(Who::AgentWrote(path.into()), body, AGENT_CUT));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// A Cursor agent transcript: `role` and `message.content[]`, the owner's
/// words inside `<user_query>`.
pub(crate) fn cursor(jsonl: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for record in records(jsonl) {
        let role = record.get("role").and_then(Value::as_str);
        let content = &record["message"]["content"];
        let items: Vec<&Value> = match content {
            Value::Array(items) => items.iter().collect(),
            _ => vec![content],
        };
        for item in items {
            let text = item
                .as_str()
                .or_else(|| (str_of(item, "type") == "text").then(|| str_of(item, "text")));
            match (role, text) {
                (Some("user"), Some(text)) => {
                    out.extend(user_query(text).and_then(|q| Line::new(Who::Owner, q, OWNER_CUT)))
                }
                (Some("assistant"), Some(text)) => {
                    out.extend(Line::new(Who::Agent, text, AGENT_CUT))
                }
                (Some("assistant"), None) if str_of(item, "type") == "tool_use" => {
                    let input = &item["input"];
                    match str_of(item, "name") {
                        "AskQuestion" => {
                            out.extend(Line::new(Who::AgentAsked, &questions(input), OWNER_CUT))
                        }
                        "CreatePlan" => {
                            let plan = match str_of(input, "plan") {
                                "" => input.to_string(),
                                plan => plan.to_string(),
                            };
                            out.extend(Line::new(Who::AgentPlan, &plan, OWNER_CUT))
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// The text inside `<user_query>…</user_query>`.
fn user_query(text: &str) -> Option<&str> {
    let start = text.find("<user_query>")? + "<user_query>".len();
    let rest = &text[start..];
    Some(rest.find("</user_query>").map_or(rest, |end| &rest[..end]))
}

/// The owner's line, unless `text` is a wrapper.
fn owner_text(text: &str) -> Option<Line> {
    let trimmed = text.trim_start();
    if NOT_OWNER.iter().any(|p| trimmed.starts_with(p)) {
        return None;
    }
    Line::new(Who::Owner, text, OWNER_CUT)
}

/// An AskUserQuestion's (or AskQuestion's) questions with their options,
/// one per line.
fn questions(input: &Value) -> String {
    let Some(list) = input.get("questions").and_then(Value::as_array) else {
        return match str_of(input, "question") {
            "" => input.to_string(),
            q => q.to_string(),
        };
    };
    list.iter()
        .map(|q| {
            let options: Vec<&str> = q
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|o| o.as_str().unwrap_or_else(|| str_of(o, "label")))
                .collect();
            let asked = match str_of(q, "question") {
                "" => str_of(q, "prompt"),
                asked => asked,
            };
            format!("{asked} [{}]", options.join(" | "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool result's text, whether one string or a list of text blocks.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A commit or a `gh pr create|edit`.
fn is_kept_command(command: &str) -> bool {
    command.contains("git commit")
        || command.contains("gh pr create")
        || command.contains("gh pr edit")
}

/// Whether `path` is a doc a write to is kept.
pub(crate) fn is_doc_path(path: &str) -> bool {
    DOC_PATHS.iter().any(|doc| {
        path.starts_with(doc)
            || path.contains(&format!("/{doc}"))
            || (!doc.ends_with('/') && path.ends_with(doc))
    })
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Each line of `jsonl` that is JSON; the rest are skipped.
fn records(jsonl: &str) -> impl Iterator<Item = Value> + '_ {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A Claude Code session: a prompt, a wrapper, a question and its
    /// answer, a plan and its approval, prose, a commit, a doc write, a
    /// rejected call, a side chain and a tool result that is only output.
    pub(crate) const CLAUDE: &str = r#"{"type":"user","gitBranch":"fix-login","message":{"role":"user","content":"Fix the login redirect. Never send users to /home after login, always back where they were."}}
{"type":"user","message":{"role":"user","content":"<system-reminder>ignore me</system-reminder>"}}
{"type":"user","isMeta":true,"message":{"role":"user","content":"meta text"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"I'll look at the auth flow."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"rg redirect src"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"src/auth.ts:12 redirect('/home')"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"AskUserQuestion","input":{"questions":[{"question":"Keep the query string?","options":[{"label":"Keep it (Recommended)"},{"label":"Drop it"}]}]}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"User answered: Keep it (Recommended)"}]}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"ExitPlanMode","input":{"plan":"1. Store the return path\n2. Redirect to it"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t3","content":"User has approved your plan."}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t4","name":"Bash","input":{"command":"git commit -m 'fix: keep return path'"}},{"type":"tool_use","id":"t5","name":"Write","input":{"file_path":"/repo/docs/auth.md","content":"Login returns to the page you came from."}},{"type":"tool_use","id":"t6","name":"Edit","input":{"file_path":"/repo/src/auth.ts","new_string":"x"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t7","is_error":true,"content":"The user doesn't want to proceed with this tool use. The tool use was rejected."}]}}
{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent chatter"}]}}
not json at all
"#;

    pub(crate) const CURSOR: &str = r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_info>os</user_info>\n<user_query>\nUse the shared date picker everywhere, never the native one.\n</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"Switching the forms to the shared picker."},{"type":"tool_use","name":"AskQuestion","input":{"questions":[{"prompt":"Also the admin pages?","options":[{"label":"Yes"},{"label":"No"}]}]}},{"type":"tool_use","name":"CreatePlan","input":{"plan":"Replace every input[type=date]"}},{"type":"tool_use","name":"Read","input":{"path":"x"}}]}}
{"role":"user","message":{"content":[{"type":"text","text":"no query tag here"}]}}
"#;

    fn rendered(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::render).collect()
    }

    #[test]
    fn a_claude_transcript_keeps_what_a_harvest_reads() {
        assert_eq!(
            rendered(&claude(CLAUDE)),
            vec![
                "OWNER: Fix the login redirect. Never send users to /home after login, always back where they were.",
                "AGENT: I'll look at the auth flow.",
                "AGENT ASKED: Keep the query string? [Keep it (Recommended) | Drop it]",
                "OWNER ANSWERED (question): User answered: Keep it (Recommended)",
                "AGENT PLAN: 1. Store the return path\n2. Redirect to it",
                "OWNER ANSWERED (plan approval): User has approved your plan.",
                "AGENT RAN: git commit -m 'fix: keep return path'",
                "AGENT WROTE /repo/docs/auth.md: Login returns to the page you came from.",
                "OWNER REJECTED A TOOL CALL: The user doesn't want to proceed with this tool use. The tool use was rejected.",
            ]
        );
    }

    #[test]
    fn a_cursor_transcript_keeps_the_queries_prose_questions_and_plans() {
        assert_eq!(
            rendered(&cursor(CURSOR)),
            vec![
                "OWNER: Use the shared date picker everywhere, never the native one.",
                "AGENT: Switching the forms to the shared picker.",
                "AGENT ASKED: Also the admin pages? [Yes | No]",
                "AGENT PLAN: Replace every input[type=date]",
            ]
        );
    }

    #[test]
    fn long_messages_are_cut() {
        let long = "x".repeat(10_000);
        let jsonl = format!(
            "{}\n{}",
            serde_json::json!({"type":"user","message":{"content": long}}),
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text": long}]}})
        );
        let lines = claude(&jsonl);
        assert_eq!(lines[0].text.chars().count(), OWNER_CUT);
        assert_eq!(lines[1].text.chars().count(), AGENT_CUT);
    }

    #[test]
    fn doc_paths_are_the_docs_and_agent_files() {
        for doc in [
            "docs/a.md",
            "/r/agent_docs/x.md",
            "/r/CLAUDE.md",
            "AGENTS.md",
            "/r/memory/m.md",
        ] {
            assert!(is_doc_path(doc), "{doc}");
        }
        for not in ["/r/src/docs.rs", "README.md"] {
            assert!(!is_doc_path(not), "{not}");
        }
    }
}
