//! APPROVED PATHS: the protected policy files a factory session's approved
//! plan names, written where the repo's automation guard looks for them.
//!
//! A repo can guard its unattended sessions (riplo-os's
//! `scripts/automation-guard.mjs`): a session may edit a file on the
//! repo's policy list (`node scripts/automation-protected-paths.mjs
//! --json`, its `policy`) only when `<worktree>/.orion/approved-paths.json`
//! names it. The guard takes `{ "paths": [...] }` or a bare array; an entry
//! is an exact repo-relative path or a glob, a trailing `/` covers a
//! folder, and `<file>#<key>` (`package.json#scripts`) is one top-level
//! key of a JSON file. The session can't write that file itself, so
//! without orion it could never edit a policy file.
//!
//! Each time a plan is answered (Approving → Building, `stages`), orion
//! asks the session's transcript how ([`verdict`]): the result of its
//! latest `ExitPlanMode`. Approved, it reads the policy list in the
//! worktree and writes `{ "paths": [...] }`: the paths the plan names
//! ([`named`]) that a policy entry covers ([`approved`]). Sent back with
//! changes ("Suggest changes" runs the session again too), or with a
//! transcript that can't be read or doesn't say, it writes
//! `{ "paths": [] }`: the guard allows nothing until a real approval. The
//! file is rewritten every time — a plan approved again replaces the last
//! one's paths — and is empty when the plan names none. A repo without
//! the policy script has no guard, and gets no file.
//!
//! The file is never committed: the repo ignores `.orion/` but its
//! contract. One that doesn't ignore it is logged.

use std::path::{Path, PathBuf};
use std::time::Duration;

use orion_core::AgentId;
use serde_json::Value;

use crate::app::App;
use crate::pr_body::contract::glob_match;

/// Where the guard reads the approved paths, in the worktree.
pub(crate) const FILE: &str = ".orion/approved-paths.json";
/// The repo's policy list.
const SCRIPT: &str = "scripts/automation-protected-paths.mjs";
const NODE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the plan's answer is waited for in the transcript, a read
/// every [`RESULT_EVERY`]: the session can run again a moment before
/// Claude Code writes it down.
const RESULT_READS: u32 = 10;
const RESULT_EVERY: Duration = Duration::from_secs(1);

/// The start of Claude Code's `ExitPlanMode` result when you approve the
/// plan (Claude Code 2.1: "User has approved your plan. You can now start
/// coding. …").
const APPROVED: &str = "User has approved your plan";
/// The start of its result, an error, when you send the plan back ("The
/// user doesn't want to proceed with this tool use. The tool use was
/// rejected … the user said: …").
const REJECTED: &str = "The user doesn't want to proceed with this tool use";

/// How the session's latest plan was answered, by its transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Approved,
    /// Sent back with changes.
    Rejected,
    /// Its result not written down yet.
    Pending,
    /// No transcript, no plan in it, or a result that is neither.
    Unknown,
}

/// The plan just answered in the worktree at `dir` by `session`: its
/// approved paths written there, off the loop — or none, when the
/// transcript doesn't say it was approved. With no loop (the tests),
/// nothing.
pub(crate) fn approve(app: &App, dir: PathBuf, plan: String, session: &AgentId) {
    if app.automations.tx.is_none() {
        return;
    }
    let agent = app.tree.agents.iter().find(|a| &a.id == session).cloned();
    tokio::spawn(async move {
        if !dir.join(SCRIPT).is_file() {
            return;
        }
        let verdict = match agent {
            Some(agent) => answered(agent).await,
            None => Verdict::Unknown,
        };
        let policy = match verdict {
            Verdict::Approved => {
                let mut cmd = tokio::process::Command::new("node");
                cmd.args([SCRIPT, "--json"]).current_dir(&dir);
                crate::pull_request::run_piped(cmd, "", NODE_TIMEOUT)
                    .await
                    .and_then(|json| parse_policy(&json))
            }
            _ => Ok(Vec::new()),
        };
        let _ = tokio::task::spawn_blocking(move || write(&dir, &plan, verdict, policy)).await;
    });
}

/// How `agent`'s latest plan was answered, read off its transcript on the
/// blocking pool, [`RESULT_READS`] times while the answer is still to be
/// written down. Still pending then, [`Verdict::Pending`].
async fn answered(agent: orion_core::Agent) -> Verdict {
    let mut verdict = Verdict::Pending;
    for read in 0..RESULT_READS {
        if read > 0 {
            tokio::time::sleep(RESULT_EVERY).await;
        }
        let agent = agent.clone();
        verdict = tokio::task::spawn_blocking(move || {
            verdict_in(crate::automations::pr_fixer::transcript_of(&agent, None).as_deref())
        })
        .await
        .unwrap_or(Verdict::Unknown);
        if verdict != Verdict::Pending {
            break;
        }
    }
    verdict
}

/// [`verdict`] of the transcript at `path`; [`Verdict::Unknown`] with no
/// path or a file that can't be read.
fn verdict_in(path: Option<&Path>) -> Verdict {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map_or(Verdict::Unknown, |text| verdict(&text))
}

/// How the latest `ExitPlanMode` in a Claude Code `transcript` was
/// answered: its `tool_result` [`APPROVED`] (not an error) or
/// [`REJECTED`]. Subagents' lines aside.
fn verdict(transcript: &str) -> Verdict {
    let mut latest: Option<(String, Verdict)> = None;
    for line in transcript.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(content) = v.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        for item in content {
            let str_of = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("");
            match str_of("type") {
                "tool_use" if str_of("name") == orion_core::EXIT_PLAN_MODE_TOOL => {
                    latest = Some((str_of("id").to_string(), Verdict::Pending));
                }
                "tool_result" => {
                    if let Some((id, answer)) = latest.as_mut() {
                        if id.as_str() == str_of("tool_use_id") {
                            *answer = result_verdict(item);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    latest.map_or(Verdict::Unknown, |(_, answer)| answer)
}

/// One `ExitPlanMode` `tool_result`'s answer.
fn result_verdict(result: &Value) -> Verdict {
    let text = match result.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    };
    let text = text.trim_start();
    let error = result.get("is_error").and_then(Value::as_bool) == Some(true);
    if !error && text.starts_with(APPROVED) {
        Verdict::Approved
    } else if text.starts_with(REJECTED) {
        Verdict::Rejected
    } else {
        Verdict::Unknown
    }
}

/// `policy` from the script's `--json`.
fn parse_policy(json: &str) -> Result<Vec<String>, String> {
    #[derive(serde::Deserialize)]
    struct Lists {
        policy: Vec<String>,
    }
    serde_json::from_str::<Lists>(json)
        .map(|l| l.policy)
        .map_err(|err| format!("{SCRIPT} --json: {err}"))
}

/// Write the file. A plan not approved, or a policy list that couldn't
/// be read, approves nothing, rather than leave the last plan's paths
/// standing. `policy` is read only for an approved plan.
fn write(dir: &Path, plan: &str, verdict: Verdict, policy: Result<Vec<String>, String>) {
    let paths = match (verdict, policy) {
        (Verdict::Approved, Ok(policy)) => approved(&named(plan, dir), &policy, plan, |rel| {
            dir.join(rel).is_dir()
        }),
        (Verdict::Approved, Err(why)) => {
            tracing::warn!(
                "approved paths: reading the policy list in {}: {why}; approving none",
                dir.display()
            );
            Vec::new()
        }
        (verdict, _) => {
            tracing::info!(
                "approved paths: the plan in {} isn't approved ({verdict:?}); approving none",
                dir.display()
            );
            Vec::new()
        }
    };
    let file = dir.join(FILE);
    let json = serde_json::json!({ "paths": paths });
    let written = std::fs::create_dir_all(file.parent().unwrap_or(dir)).and_then(|()| {
        let part = file.with_extension("json.part");
        std::fs::write(&part, format!("{json:#}\n"))?;
        std::fs::rename(&part, &file)
    });
    match written {
        Ok(()) => tracing::info!(
            "approved paths: {} in {}: {}",
            FILE,
            dir.display(),
            if paths.is_empty() {
                "none".to_string()
            } else {
                paths.join(", ")
            }
        ),
        Err(err) => tracing::warn!("approved paths: writing {}: {err}", file.display()),
    }
    if crate::git_proc::read(dir, &["check-ignore", "-q", FILE]).is_err() {
        tracing::warn!(
            "approved paths: {} doesn't ignore {FILE}; it must never be committed",
            dir.display()
        );
    }
}

/// The repo-relative paths `plan` names, backticked or bare, in order and
/// once each: words with a `/`, `.` or `#` in them, `./` and the
/// worktree's own prefix taken off, a line (`:12`, `#L12`) and markdown's
/// `**` around a word dropped. URLs, flags and paths outside the worktree
/// aren't named.
pub(crate) fn named(plan: &str, worktree: &Path) -> Vec<String> {
    let root = format!("{}/", worktree.to_string_lossy().trim_end_matches('/'));
    let mut out: Vec<String> = Vec::new();
    let words = plan.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '`' | '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | ',' | ';' | '|'
            )
    });
    for word in words {
        if word.contains("://") || word.starts_with('-') {
            continue;
        }
        let mut w = word.trim_end_matches(['.', ':', '!', '?']);
        // Markdown bold around a word, not a glob's `**`.
        if let Some(rest) = w.strip_prefix("**").filter(|r| !r.starts_with('/')) {
            w = rest;
        }
        if let Some(rest) = w.strip_suffix("**").filter(|r| !r.ends_with('/')) {
            w = rest;
        }
        // A line: `a.ts:12`, `a.ts:12:3`, `a.ts#L12`.
        if let Some((path, line)) = w.split_once(':') {
            if line.chars().all(|c| c.is_ascii_digit() || c == ':') {
                w = path;
            }
        }
        if let Some((path, anchor)) = w.split_once("#L") {
            if anchor
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-' || c == 'L')
            {
                w = path;
            }
        }
        let w = w
            .strip_prefix(root.as_str())
            .filter(|_| root != "/")
            .unwrap_or(w);
        let w = w.trim_start_matches("./").to_string();
        let looks_like_a_path = w.contains(['/', '.', '#']);
        if !looks_like_a_path || w.starts_with('/') || w.starts_with('~') || w.contains("..") {
            continue;
        }
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

/// Whether the guard's policy `entry` covers the repo-relative `path`
/// (a folder as `dir/`). Policy globs are from the repo's root, as the
/// guard reads them: `CLAUDE.md` is the root one only.
fn covers(entry: &str, path: &str) -> bool {
    !entry.contains('#') && glob_match(&format!("/{}", entry.trim_start_matches('/')), path)
}

/// Of the paths a plan `named`, those a `policy` entry covers, as the
/// guard reads them — an unprotected file needs no approval:
///
/// - a policy entry named as written (`.github/workflows/**`);
/// - a file a policy entry covers (`CLAUDE.md`);
/// - a folder (a trailing `/`, or a folder in the worktree, by `is_dir`)
///   a policy entry covers, written with its `/` (`.githooks/`);
/// - the file of a `<file>#<key>` entry named, and its key named too as a
///   word of `plan`, becomes that entry (`package.json` and `scripts`:
///   `package.json#scripts`).
///
/// Sorted, once each.
pub(crate) fn approved(
    named: &[String],
    policy: &[String],
    plan: &str,
    is_dir: impl Fn(&str) -> bool,
) -> Vec<String> {
    let words: Vec<&str> = plan
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .collect();
    let mut out: Vec<String> = Vec::new();
    for path in named {
        if policy.contains(path) {
            out.push(path.clone());
            continue;
        }
        if path.contains('#') {
            continue;
        }
        for entry in policy {
            if let Some((file, key)) = entry.split_once('#') {
                if file == path && words.contains(&key) {
                    out.push(entry.clone());
                }
            }
        }
        let folder = path.ends_with('/') || is_dir(path);
        let path = match folder {
            true => format!("{}/", path.trim_end_matches('/')),
            false => path.clone(),
        };
        if policy.iter().any(|entry| covers(entry, &path)) {
            out.push(path);
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// riplo-os's policy list (`automation-protected-paths.mjs`, PR #2114).
    fn policy() -> Vec<String> {
        [
            "CLAUDE.md",
            "AGENTS.md",
            "agent_docs/automation.md",
            "agent_docs/test_policy.md",
            "scripts/enforce-repo-contract.mjs",
            "scripts/automation-*.mjs",
            ".githooks/**",
            ".github/workflows/**",
            ".fallowrc.json",
            ".claude/settings*.json",
            ".claude/skills/**",
            ".orion/pr-contract.toml",
            "**/vitest.config.*",
            "playwright*.config.ts",
            "package.json#scripts",
        ]
        .map(String::from)
        .to_vec()
    }

    const PLAN: &str = "## Plan\n\n\
        1. Add the flag to `apps/web/src/flags.ts:12` and read it in apps/web/src/app/page.tsx.\n\
        2. Note it in **CLAUDE.md** and in `agent_docs/test_policy.md#L4`.\n\
        3. Run it from `.github/workflows/e2e.yml` (see https://github.com/o/r/blob/dev/CLAUDE.md).\n\
        4. New skill under `.claude/skills/riplo-flags/`; touch `./scripts/automation-guard.mjs`.\n\
        5. `npm run test -- --run` and `pnpm vitest`; also apps/web/vitest.config.ts.\n";

    #[test]
    fn named_finds_backticked_and_bare_paths() {
        let found = named(PLAN, Path::new("/work/tree"));
        for want in [
            "apps/web/src/flags.ts",
            "apps/web/src/app/page.tsx",
            "CLAUDE.md",
            "agent_docs/test_policy.md",
            ".github/workflows/e2e.yml",
            ".claude/skills/riplo-flags/",
            "scripts/automation-guard.mjs",
            "apps/web/vitest.config.ts",
        ] {
            assert!(found.iter().any(|f| f == want), "{want} in {found:?}");
        }
        assert!(!found.iter().any(|f| f.contains("://")), "{found:?}");
        assert!(!found.iter().any(|f| f.starts_with('-')), "{found:?}");
    }

    #[test]
    fn named_takes_the_worktree_off_an_absolute_path() {
        let found = named(
            "Edit /work/tree/AGENTS.md, not /etc/hosts or ../x/CLAUDE.md.",
            Path::new("/work/tree/"),
        );
        assert_eq!(found, ["AGENTS.md"]);
    }

    #[test]
    fn approved_keeps_only_what_policy_covers() {
        let plan = PLAN;
        let found = named(plan, Path::new("/work/tree"));
        let dirs = [".claude/skills/riplo-flags", ".githooks"];
        let got = approved(&found, &policy(), plan, |p| {
            dirs.contains(&p.trim_end_matches('/'))
        });
        assert_eq!(
            got,
            [
                ".claude/skills/riplo-flags/",
                ".github/workflows/e2e.yml",
                "CLAUDE.md",
                "agent_docs/test_policy.md",
                "apps/web/vitest.config.ts",
                "scripts/automation-guard.mjs",
            ]
        );
    }

    #[test]
    fn policy_globs_are_from_the_root_and_folders_get_their_slash() {
        let named: Vec<String> = [
            "apps/web/CLAUDE.md",
            ".githooks",
            "agent_docs/",
            "AGENTS.md",
        ]
        .map(String::from)
        .to_vec();
        let got = approved(&named, &policy(), "", |p| p == ".githooks");
        // A nested CLAUDE.md, and a folder only some of whose files are
        // policy, aren't covered.
        assert_eq!(got, [".githooks/", "AGENTS.md"]);
    }

    #[test]
    fn a_json_key_entry_needs_its_file_and_its_key_named() {
        let named = vec!["package.json".to_string()];
        assert!(approved(
            &named,
            &policy(),
            "bump a dependency in `package.json`",
            |_| false
        )
        .is_empty());
        assert_eq!(
            approved(
                &named,
                &policy(),
                "add a `scripts` entry to `package.json`",
                |_| false
            ),
            ["package.json#scripts"]
        );
        let exact = vec!["package.json#scripts".to_string()];
        assert_eq!(
            approved(&exact, &policy(), "", |_| false),
            ["package.json#scripts"]
        );
        let entry = vec![".github/workflows/**".to_string()];
        assert_eq!(
            approved(&entry, &policy(), "", |_| false),
            [".github/workflows/**"]
        );
    }

    #[test]
    fn the_policy_list_is_read_from_the_script_s_json() {
        let json = r#"{"policy": ["CLAUDE.md", "package.json#scripts"], "reference": ["agent_docs/x.md"]}"#;
        assert_eq!(
            parse_policy(json).unwrap(),
            ["CLAUDE.md", "package.json#scripts"]
        );
        assert!(parse_policy("usage: ...").is_err());
    }

    #[test]
    fn writing_rewrites_the_file_with_the_plan_s_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".githooks")).unwrap();
        write(
            dir.path(),
            "Edit `CLAUDE.md` and `.githooks`.",
            Verdict::Approved,
            Ok(policy()),
        );
        let read = |p: &Path| -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(p.join(FILE)).unwrap()).unwrap()
        };
        assert_eq!(
            read(dir.path()),
            serde_json::json!({"paths": [".githooks/", "CLAUDE.md"]})
        );
        // Approved again after changes: the new plan's paths replace them.
        write(
            dir.path(),
            "Only `src/a.ts` now.",
            Verdict::Approved,
            Ok(policy()),
        );
        assert_eq!(read(dir.path()), serde_json::json!({"paths": []}));
        write(
            dir.path(),
            "Edit `CLAUDE.md`.",
            Verdict::Approved,
            Err("no node".into()),
        );
        assert_eq!(read(dir.path()), serde_json::json!({"paths": []}));
    }

    /// A transcript: the plan put up, then each of `results` its answer
    /// (`(is_error, text)`), the last one's id the latest plan's.
    fn transcript(results: &[(bool, &str)]) -> String {
        let mut lines = vec![r#"{"type":"user","message":{"content":"fix it"}}"#.to_string()];
        for (n, (error, text)) in results.iter().enumerate() {
            let id = format!("toolu_{n}");
            lines.push(
                serde_json::json!({"type": "assistant", "message": {"content": [
                    {"type": "tool_use", "id": id, "name": "ExitPlanMode",
                     "input": {"plan": "Edit `CLAUDE.md`."}}
                ]}})
                .to_string(),
            );
            lines.push(
                serde_json::json!({"type": "user", "message": {"content": [
                    {"type": "tool_result", "tool_use_id": id, "is_error": error,
                     "content": text}
                ]}})
                .to_string(),
            );
        }
        lines.join("\n") + "\n"
    }

    const YES: &str = "User has approved your plan. You can now start coding. Start with \
        updating your todo list if applicable\n\nYour plan has been saved to: /x.md";
    const NO: &str = "The user doesn't want to proceed with this tool use. The tool use was \
        rejected (eg. if it was a file edit, the new_string was NOT written to the file). \
        To tell you how to proceed, the user said:\nnot CLAUDE.md";

    #[test]
    fn the_verdict_is_the_latest_plan_s_answer() {
        assert_eq!(verdict(&transcript(&[(false, YES)])), Verdict::Approved);
        assert_eq!(verdict(&transcript(&[(true, NO)])), Verdict::Rejected);
        // Approved once, then a new plan sent back: not approved.
        assert_eq!(
            verdict(&transcript(&[(false, YES), (true, NO)])),
            Verdict::Rejected
        );
        assert_eq!(
            verdict(&transcript(&[(true, NO), (false, YES)])),
            Verdict::Approved
        );
        // Its answer as text parts.
        let parts = transcript(&[(false, YES)]).replace(
            &serde_json::to_string(YES).unwrap(),
            &serde_json::json!([{"type": "text", "text": YES}]).to_string(),
        );
        assert_eq!(verdict(&parts), Verdict::Approved);
        // The approval's words in an error aren't an approval.
        assert_eq!(verdict(&transcript(&[(true, YES)])), Verdict::Unknown);
        // A plan up with its answer not written down yet.
        let mut pending = transcript(&[(false, YES)]);
        pending.truncate(pending.trim_end().rfind('\n').unwrap() + 1);
        assert_eq!(verdict(&pending), Verdict::Pending);
        assert_eq!(verdict("{}\nnot json\n"), Verdict::Unknown);
    }

    #[test]
    fn an_unreadable_transcript_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(verdict_in(None), Verdict::Unknown);
        assert_eq!(
            verdict_in(Some(&dir.path().join("gone.jsonl"))),
            Verdict::Unknown
        );
        let file = dir.path().join("s.jsonl");
        std::fs::write(&file, transcript(&[(false, YES)])).unwrap();
        assert_eq!(verdict_in(Some(&file)), Verdict::Approved);
    }

    #[test]
    fn a_plan_not_approved_clears_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let read = || -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(FILE)).unwrap()).unwrap()
        };
        let plan = "Edit `CLAUDE.md`.";
        write(dir.path(), plan, Verdict::Approved, Ok(policy()));
        assert_eq!(read(), serde_json::json!({"paths": ["CLAUDE.md"]}));
        // Sent back: the earlier approval's paths go too.
        write(dir.path(), plan, Verdict::Rejected, Ok(policy()));
        assert_eq!(read(), serde_json::json!({"paths": []}));
        for not_known in [Verdict::Unknown, Verdict::Pending] {
            write(dir.path(), plan, Verdict::Approved, Ok(policy()));
            write(dir.path(), plan, not_known, Ok(policy()));
            assert_eq!(read(), serde_json::json!({"paths": []}), "{not_known:?}");
        }
    }
}
