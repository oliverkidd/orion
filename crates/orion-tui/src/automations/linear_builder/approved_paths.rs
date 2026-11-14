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
//! Each time a plan is approved (Approving → Building, `stages`), orion
//! reads the policy list in the worktree and writes `{ "paths": [...] }`:
//! the paths the plan names ([`named`]) that a policy entry covers
//! ([`approved`]). The file is rewritten on every approval — a plan sent
//! back and approved again replaces the last one's paths — and is empty
//! when the plan names none. A repo without the policy script has no
//! guard, and gets no file.
//!
//! The file is never committed: the repo ignores `.orion/` but its
//! contract. One that doesn't ignore it is logged.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::app::App;
use crate::pr_body::contract::glob_match;

/// Where the guard reads the approved paths, in the worktree.
pub(crate) const FILE: &str = ".orion/approved-paths.json";
/// The repo's policy list.
const SCRIPT: &str = "scripts/automation-protected-paths.mjs";
const NODE_TIMEOUT: Duration = Duration::from_secs(30);

/// The plan just approved in the worktree at `dir`: its approved paths
/// written there, off the loop. With no loop (the tests), nothing.
pub(crate) fn approve(app: &App, dir: PathBuf, plan: String) {
    if app.automations.tx.is_none() {
        return;
    }
    tokio::spawn(async move {
        if !dir.join(SCRIPT).is_file() {
            return;
        }
        let mut cmd = tokio::process::Command::new("node");
        cmd.args([SCRIPT, "--json"]).current_dir(&dir);
        let policy = crate::pull_request::run_piped(cmd, "", NODE_TIMEOUT)
            .await
            .and_then(|json| parse_policy(&json));
        let _ = tokio::task::spawn_blocking(move || write(&dir, &plan, policy)).await;
    });
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

/// Write the file. A policy list that couldn't be read approves nothing,
/// rather than leave the last plan's paths standing.
fn write(dir: &Path, plan: &str, policy: Result<Vec<String>, String>) {
    let paths = match policy {
        Ok(policy) => approved(&named(plan, dir), &policy, plan, |rel| {
            dir.join(rel).is_dir()
        }),
        Err(why) => {
            tracing::warn!(
                "approved paths: reading the policy list in {}: {why}; approving none",
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
        write(dir.path(), "Only `src/a.ts` now.", Ok(policy()));
        assert_eq!(read(dir.path()), serde_json::json!({"paths": []}));
        write(dir.path(), "Edit `CLAUDE.md`.", Err("no node".into()));
        assert_eq!(read(dir.path()), serde_json::json!({"paths": []}));
    }
}
