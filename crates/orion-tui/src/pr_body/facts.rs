//! FACTS: what the agent that built a pull request says about it, in one
//! JSON document — the only words of the PR's top that are the agent's.
//! Everything else on it orion works out (`super::verdict`).
//!
//! The agent prints a skeleton with `orion pr facts init`, fills it in,
//! and hands it over with `orion pr facts set <file>`, which checks it and
//! writes it to `<data_dir>/pr-facts/<url hash>.json` ([`file_for`]). The
//! TUI's poller picks it up from there, and keeps it in the PR's body as a
//! hidden block, `<!-- orion:facts {json} -->` ([`block`]), so a PR
//! carries its facts wherever it is read from.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The tags a call can carry: the agent asks the owner about the first
/// five; it takes the Recommended option on `copy` and `ux` and lists the
/// call. `security`, `dep` and `arch` send the PR to Review.
pub const TAGS: [&str; 7] = ["product", "arch", "security", "dep", "scope", "copy", "ux"];

/// A call the agent made on the owner's behalf.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Call {
    /// The question, in a few words.
    pub call: String,
    /// What it picked.
    pub picked: String,
    /// The option it didn't.
    pub alternative: String,
    /// One of [`TAGS`].
    pub tag: String,
}

/// Whether the build kept to the approved plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanFollowed {
    pub followed: bool,
    /// Where it went its own way, and why.
    pub deviations: Vec<String>,
}

impl Default for PlanFollowed {
    fn default() -> Self {
        Self {
            followed: true,
            deviations: Vec::new(),
        }
    }
}

/// The facts of one pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Facts {
    /// At most two sentences: what changes for the people who use it.
    pub what_users_get: String,
    /// How it was shown to work: tests added, checks run, pictures.
    pub proof: Vec<String>,
    /// What wasn't done or checked.
    pub gaps: Vec<String>,
    pub your_calls: Vec<Call>,
    /// The feature flag it is behind, if any.
    pub flag: Option<String>,
    /// Docs it changed.
    pub docs_touched: Vec<String>,
    pub plan_followed: PlanFollowed,
}

impl Facts {
    /// What is wrong with a filled-in document, so `set` refuses it:
    /// nothing said of users, a placeholder left in, a call without its
    /// tag.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.what_users_get.trim().is_empty() {
            out.push("what_users_get is empty".to_string());
        }
        let mut texts: Vec<(&str, &str)> = vec![("what_users_get", &self.what_users_get)];
        texts.extend(self.proof.iter().map(|p| ("proof", p.as_str())));
        texts.extend(self.gaps.iter().map(|g| ("gaps", g.as_str())));
        for call in &self.your_calls {
            texts.extend([
                ("your_calls.call", call.call.as_str()),
                ("your_calls.picked", call.picked.as_str()),
                ("your_calls.alternative", call.alternative.as_str()),
            ]);
            if !TAGS.contains(&call.tag.as_str()) {
                out.push(format!(
                    "your_calls: \"{}\" has tag \"{}\"; use one of {}",
                    call.call,
                    call.tag,
                    TAGS.join(", ")
                ));
            }
        }
        for (field, text) in texts {
            if text.trim_start().starts_with('<') && text.trim_end().ends_with('>') {
                out.push(format!("{field} still holds the placeholder {text}"));
            }
        }
        out
    }
}

/// What `orion pr facts init` prints: every field, each with what goes
/// in it.
pub fn skeleton() -> String {
    let facts = Facts {
        what_users_get: "<at most two sentences: what changes for the people who use it>".into(),
        proof: vec!["<how you showed it works: a test, a check, a picture>".into()],
        gaps: vec![
            "<what you didn't do or couldn't check; leave the list empty if nothing>".into(),
        ],
        your_calls: vec![Call {
            call: "<a choice you made for the owner>".into(),
            picked: "<what you picked>".into(),
            alternative: "<the option you didn't>".into(),
            tag: "copy".into(),
        }],
        flag: None,
        docs_touched: Vec::new(),
        plan_followed: PlanFollowed::default(),
    };
    serde_json::to_string_pretty(&facts).unwrap_or_default()
}

const OPEN: &str = "<!-- orion:facts ";
const CLOSE: &str = " -->";

/// The hidden block that carries `facts` in a PR body. The JSON's `<` and
/// `>` are written as `<` / `>` (they only ever occur inside
/// its strings), so nothing in it can end the comment early.
pub fn block(facts: &Facts) -> String {
    let json = serde_json::to_string(facts).unwrap_or_default();
    format!(
        "{OPEN}{}{CLOSE}",
        json.replace('<', "\\u003c").replace('>', "\\u003e")
    )
}

/// The facts a body carries, from its last hidden block.
pub fn from_body(body: &str) -> Option<Facts> {
    let start = body.rfind(OPEN)? + OPEN.len();
    let len = body[start..].find(CLOSE)?;
    serde_json::from_str(&body[start..start + len]).ok()
}

/// `body` with every hidden facts block taken out.
pub fn strip_blocks(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find(OPEN) {
        out.push_str(&rest[..at]);
        match rest[at..].find(CLOSE) {
            Some(len) => rest = &rest[at + len + CLOSE.len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// What `set` writes: the PR it is for, and its facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub url: String,
    pub facts: Facts,
}

/// Where the facts handed over for the PR at `url` wait, under `dir`
/// (`<data_dir>/pr-facts`).
pub fn file_for(dir: &Path, url: &str) -> PathBuf {
    let hash = Sha256::digest(url.trim().as_bytes());
    let name: String = hash[..8].iter().map(|b| format!("{b:02x}")).collect();
    dir.join(format!("{name}.json"))
}

/// The facts waiting for `url` under `dir`, if any.
pub fn read_for(dir: &Path, url: &str) -> Option<Facts> {
    let bytes = std::fs::read(file_for(dir, url)).ok()?;
    let stored: Stored = serde_json::from_slice(&bytes).ok()?;
    (stored.url.trim() == url.trim()).then_some(stored.facts)
}

/// When the facts for `url` were last handed over: the poller reads the
/// PR again when this moves.
pub fn stamp_for(dir: &Path, url: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(file_for(dir, url))
        .and_then(|m| m.modified())
        .ok()
}

/// Write `facts` for `url` under `dir`.
pub fn write_for(dir: &Path, url: &str, facts: &Facts) -> std::io::Result<PathBuf> {
    let path = file_for(dir, url);
    let stored = Stored {
        url: url.trim().to_string(),
        facts: facts.clone(),
    };
    crate::pr_cache::write_json_atomic(&path, &stored)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Facts {
        Facts {
            what_users_get: "Login no longer loops. Users land on <the dashboard> --> fast.".into(),
            proof: vec!["unit test `login_redirects_once`".into()],
            gaps: vec![],
            your_calls: vec![Call {
                call: "Button copy".into(),
                picked: "Sign in".into(),
                alternative: "Log in".into(),
                tag: "copy".into(),
            }],
            flag: Some("new-login".into()),
            docs_touched: vec!["docs/auth.md".into()],
            plan_followed: PlanFollowed {
                followed: false,
                deviations: vec!["kept the old cookie name".into()],
            },
        }
    }

    #[test]
    fn facts_round_trip_through_the_hidden_block() {
        let facts = sample();
        let block = block(&facts);
        assert!(block.starts_with("<!-- orion:facts {"));
        // Only the block's own close ends the comment.
        assert_eq!(block.matches("-->").count(), 1);
        let body = format!("Some notes\n\n{block}\n\nmore");
        assert_eq!(from_body(&body), Some(facts));
        assert_eq!(strip_blocks(&body), "Some notes\n\n\n\nmore");
    }

    #[test]
    fn a_body_without_a_block_has_no_facts() {
        assert_eq!(from_body("just words"), None);
        assert_eq!(from_body("<!-- orion:facts not json -->"), None);
    }

    #[test]
    fn the_skeleton_parses_and_is_refused_until_filled() {
        let facts: Facts = serde_json::from_str(&skeleton()).unwrap();
        assert!(!facts.problems().is_empty());
        assert!(sample().problems().is_empty());
        let mut bad = sample();
        bad.your_calls[0].tag = "vibes".into();
        assert_eq!(bad.problems().len(), 1);
    }

    #[test]
    fn a_partial_document_fills_in_defaults() {
        let facts: Facts = serde_json::from_str(r#"{"what_users_get":"x"}"#).unwrap();
        assert!(facts.plan_followed.followed);
        assert!(facts.proof.is_empty());
    }

    #[test]
    fn facts_files_are_keyed_by_url() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://github.com/o/r/pull/7";
        write_for(dir.path(), url, &sample()).unwrap();
        assert_eq!(read_for(dir.path(), url), Some(sample()));
        assert_eq!(read_for(dir.path(), "https://github.com/o/r/pull/8"), None);
        assert!(stamp_for(dir.path(), url).is_some());
    }
}
