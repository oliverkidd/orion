//! THE PR CONTRACT: what a repo tells orion about its pull requests, in
//! `.orion/pr-contract.toml` on the PR's base branch, read through `gh api`
//! ([`fetch`]). Every field is optional; a repo without the file has no
//! risky paths, no visible paths, the default rule files and the default
//! size limits.
//!
//! ```toml
//! # Paths that always want a person's eyes: a PR touching one is Review.
//! risky_paths = ["supabase/migrations/**", "src/auth/**"]
//! # Paths users see: a PR touching one needs pictures (Skim), or is
//! # Blocked without them.
//! visible_paths = ["apps/web/src/**/*.tsx", "**/*.css"]
//! # The repo's rules for agents, listed on the PR when touched.
//! rule_paths = ["CLAUDE.md", "AGENTS.md", "agent_docs/**"]
//! # The workflow that takes the pictures, by file name. Without it orion
//! # looks for the one active workflow named for keyframes or pictures.
//! pictures_workflow = "pr-keyframes.yml"
//!
//! [limits.bug]
//! lines = 3000
//! files = 60
//!
//! [limits.feature]
//! lines = 5000
//! files = 70
//! ```
//!
//! Paths are globs from the repo's root: `*` stays within a directory,
//! `**` crosses any number of them, `?` is one character, a trailing `/`
//! is everything under it, and a pattern with no `/` matches a file name
//! at any depth unless it starts with one ([`glob_match`]).

use serde::Deserialize;
use std::time::Duration;

/// Where the contract lives in a repo.
pub const PATH: &str = ".orion/pr-contract.toml";

/// A size limit: changed lines and files, after the exclusions
/// (`super::verdict::counted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Limit {
    pub lines: u64,
    pub files: u64,
}

/// The limits by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub bug: Limit,
    pub feature: Limit,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            bug: Limit {
                lines: 3000,
                files: 60,
            },
            feature: Limit {
                lines: 5000,
                files: 70,
            },
        }
    }
}

/// A repo's contract.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Contract {
    pub risky_paths: Vec<String>,
    pub visible_paths: Vec<String>,
    pub rule_paths: Vec<String>,
    /// The pictures workflow's file name (`pr-keyframes.yml`), which
    /// orion runs again for a PR whose flow came after its push
    /// (`super::dispatch`). None: found by its name.
    pub pictures_workflow: Option<String>,
    pub limits: Limits,
}

impl Default for Contract {
    fn default() -> Self {
        Self {
            risky_paths: Vec::new(),
            visible_paths: Vec::new(),
            rule_paths: [
                "CLAUDE.md",
                "AGENTS.md",
                "agent_docs/",
                ".claude/",
                ".cursor/rules/",
            ]
            .map(String::from)
            .to_vec(),
            pictures_workflow: None,
            limits: Limits::default(),
        }
    }
}

/// What reading the contract gave: the contract (the default one when
/// the repo has none), or why the file there couldn't be read — which the
/// verdict sends to Review rather than guess.
pub type Read = Result<Contract, String>;

/// Parse the file's text.
pub fn parse(text: &str) -> Read {
    toml::from_str(text).map_err(|err| {
        let first = err.to_string();
        let line = first.lines().next().unwrap_or_default().to_string();
        format!("{PATH} doesn't parse: {line}")
    })
}

/// The patterns in `patterns` that `path` matches.
pub fn matches_any(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|p| glob_match(p, path))
}

/// Whether `path` (repo-relative, `/`-separated) matches the glob
/// `pattern` (see the module doc).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim().trim_start_matches("./");
    let anchored = pattern.starts_with('/');
    let pattern = pattern.trim_start_matches('/');
    if pattern.is_empty() {
        return false;
    }
    let owned;
    let pattern = if let Some(dir) = pattern.strip_suffix('/') {
        owned = format!("{dir}/**");
        owned.as_str()
    } else {
        pattern
    };
    if !anchored && !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return wild(pattern.as_bytes(), name.as_bytes());
    }
    wild(pattern.as_bytes(), path.as_bytes())
}

/// `*` within a segment, `**` across them, `?` one character.
fn wild(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') if p.get(1) == Some(&b'*') => {
            // `**/` also matches no directories at all.
            let rest = &p[2..];
            if let Some(after) = rest.strip_prefix(b"/") {
                if wild(after, s) {
                    return true;
                }
            }
            (0..=s.len()).any(|i| wild(rest, &s[i..]))
        }
        Some(b'*') => {
            let rest = &p[1..];
            for i in 0..=s.len() {
                if wild(rest, &s[i..]) {
                    return true;
                }
                if s.get(i) == Some(&b'/') {
                    break;
                }
            }
            false
        }
        Some(b'?') => s.first().is_some_and(|c| *c != b'/') && wild(&p[1..], &s[1..]),
        Some(c) => s.first() == Some(c) && wild(&p[1..], &s[1..]),
    }
}

const TIMEOUT: Duration = Duration::from_secs(20);

/// The contract on `base` of `owner/repo`: `Ok(None)` is "couldn't ask"
/// (no network, no `gh`), which leaves the PR as it was this round.
pub async fn fetch(owner: &str, repo: &str, base: &str) -> Option<Read> {
    let endpoint = format!("repos/{owner}/{repo}/contents/{PATH}?ref={base}");
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args([
        "api",
        "-H",
        "Accept: application/vnd.github.raw",
        endpoint.as_str(),
    ]);
    match crate::pull_request::run_piped(cmd, "", TIMEOUT).await {
        Ok(text) => Some(parse(&text)),
        Err(why) if super::not_found(&why) => Some(Ok(Contract::default())),
        Err(why) => {
            tracing::info!("pr_body: reading {PATH} from {owner}/{repo}@{base}: {why}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_contract_parses() {
        let c = parse(
            r#"
risky_paths = ["supabase/migrations/**"]
visible_paths = ["apps/web/**/*.tsx"]
rule_paths = ["CLAUDE.md"]
[limits.bug]
lines = 100
files = 5
"#,
        )
        .unwrap();
        assert_eq!(c.risky_paths, ["supabase/migrations/**"]);
        assert_eq!(c.rule_paths, ["CLAUDE.md"]);
        assert_eq!(c.pictures_workflow, None);
        assert_eq!(
            c.limits.bug,
            Limit {
                lines: 100,
                files: 5
            }
        );
        assert_eq!(c.limits.feature, Limits::default().feature);
    }

    #[test]
    fn an_empty_contract_is_the_default() {
        assert_eq!(parse("").unwrap(), Contract::default());
        let d = Contract::default();
        assert!(d.risky_paths.is_empty());
        assert_eq!(
            d.limits.bug,
            Limit {
                lines: 3000,
                files: 60
            }
        );
        assert_eq!(
            d.limits.feature,
            Limit {
                lines: 5000,
                files: 70
            }
        );
    }

    #[test]
    fn a_broken_or_misspelt_contract_says_why() {
        assert!(parse("risky_paths = [").unwrap_err().contains(PATH));
        assert!(
            parse("risky_path = []").is_err(),
            "a typo is not silently ignored"
        );
    }

    /// riplo-os's own contract, as its PR #2114 commits it.
    #[test]
    fn riplo_os_s_contract_parses() {
        let c = parse(include_str!("../../fixtures/riplo-os-pr-contract.toml")).unwrap();
        assert_eq!(c.risky_paths.len(), 9);
        assert!(c.risky_paths.contains(&"**/package.json".to_string()));
        assert_eq!(
            c.visible_paths,
            [
                "apps/web/src/components/**",
                "apps/web/src/app/**",
                "packages/ui/**",
                "packages/deck/**"
            ]
        );
        assert!(c
            .rule_paths
            .contains(&"scripts/automation-*.mjs".to_string()));
        assert_eq!(
            c.limits.bug,
            Limit {
                lines: 3000,
                files: 60
            }
        );
        assert_eq!(
            c.limits.feature,
            Limit {
                lines: 5000,
                files: 70
            }
        );
        assert!(matches_any(&c.risky_paths, "package.json"));
        assert!(matches_any(&c.risky_paths, "apps/web/package.json"));
        assert!(matches_any(
            &c.visible_paths,
            "apps/web/src/components/deck/Slide.tsx"
        ));
        assert!(matches_any(&c.rule_paths, "scripts/automation-guard.mjs"));
        assert!(matches_any(&c.rule_paths, "apps/web/CLAUDE.md"));
        // It names no pictures workflow: orion finds `pr-keyframes.yml`.
        assert_eq!(c.pictures_workflow, None);
    }

    #[test]
    fn the_pictures_workflow_can_be_named() {
        let c = parse(r#"pictures_workflow = "pr-keyframes.yml""#).unwrap();
        assert_eq!(c.pictures_workflow.as_deref(), Some("pr-keyframes.yml"));
        assert!(
            parse("pictures_workflow = 3").is_err(),
            "a file name, not a number"
        );
    }

    #[test]
    fn globs_match_as_the_doc_says() {
        assert!(glob_match("src/auth/**", "src/auth/a/b.rs"));
        assert!(glob_match("src/auth/", "src/auth/x.rs"));
        assert!(!glob_match("src/auth/**", "src/authx/a.rs"));
        assert!(glob_match("apps/*/src/*.tsx", "apps/web/src/a.tsx"));
        assert!(!glob_match("apps/*/src/*.tsx", "apps/web/src/x/a.tsx"));
        assert!(glob_match("**/*.css", "a.css"));
        assert!(glob_match("**/*.css", "x/y/a.css"));
        assert!(glob_match("CLAUDE.md", "crates/x/CLAUDE.md"));
        assert!(glob_match("/CLAUDE.md", "CLAUDE.md"));
        assert!(!glob_match("/CLAUDE.md", "x/CLAUDE.md"));
        assert!(glob_match("migrations/0??_*.sql", "migrations/012_add.sql"));
        assert!(!glob_match("", "a"));
    }
}
