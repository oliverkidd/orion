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
//!
//! The top's table holds at most about twelve words a cell, so the agent
//! is asked for the short labels its rows need — a call's in at most
//! [`CALL_LABEL_WORDS`], why it went off plan, the gaps and why there are
//! no pictures in at most [`LABEL_WORDS`] — and `set` refuses a document
//! that leaves one out or runs over ([`Facts::problems`]). The long words
//! go in the top's Notes fold.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The tags a call can carry: the agent asks the owner about the first
/// five; it takes the Recommended option on `copy` and `ux` and lists the
/// call. `security`, `dep` and `arch` send the PR to Review.
pub const TAGS: [&str; 7] = ["product", "arch", "security", "dep", "scope", "copy", "ux"];

/// The most words a call's label may have.
pub const CALL_LABEL_WORDS: usize = 4;
/// The most words any other label may have.
pub const LABEL_WORDS: usize = 8;
/// The most words `what_users_get` may have: one plain sentence.
pub const SENTENCE_WORDS: usize = 30;

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
    /// The call in at most [`CALL_LABEL_WORDS`] words, for the table
    /// (`sign-in button wording`).
    pub label: String,
}

/// Whether the build kept to the approved plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanFollowed {
    pub followed: bool,
    /// Where it went its own way, and why.
    pub deviations: Vec<String>,
    /// Why it went off plan in at most [`LABEL_WORDS`] words, for the
    /// table; empty when on plan.
    pub label: String,
}

impl PlanFollowed {
    /// Whether the build went its own way anywhere.
    pub fn off(&self) -> bool {
        !self.followed || self.deviations.iter().any(|d| !d.trim().is_empty())
    }
}

impl Default for PlanFollowed {
    fn default() -> Self {
        Self {
            followed: true,
            deviations: Vec::new(),
            label: String::new(),
        }
    }
}

/// The facts of one pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Facts {
    /// One plain sentence: what changes for the people who use it.
    pub what_users_get: String,
    /// How it was shown to work beyond what the PR's checks show: a manual
    /// run, a script. Not the tests or checks CI runs.
    pub proof: Vec<String>,
    /// What wasn't done or checked.
    pub gaps: Vec<String>,
    /// The gaps in at most [`LABEL_WORDS`] words, for the table.
    pub gaps_label: String,
    /// Why a change users can see has no pictures, in at most
    /// [`LABEL_WORDS`] words; empty when it has them or needs none.
    pub pictures_label: String,
    pub your_calls: Vec<Call>,
    /// The feature flag it is behind, if any.
    pub flag: Option<String>,
    /// Docs it changed.
    pub docs_touched: Vec<String>,
    pub plan_followed: PlanFollowed,
}

impl Facts {
    /// What is wrong with a filled-in document, so `set` refuses it:
    /// nothing said of users, or more than a sentence; a placeholder left
    /// in; a call without its tag; a label the table needs left out or
    /// too long.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.what_users_get.trim().is_empty() {
            out.push("what_users_get is empty".to_string());
        }
        let said = words(&self.what_users_get);
        if said > SENTENCE_WORDS {
            out.push(format!(
                "what_users_get has {said} words: say it in one plain sentence of at most \
                 {SENTENCE_WORDS}"
            ));
        }
        let mut texts: Vec<(&str, &str)> = vec![("what_users_get", &self.what_users_get)];
        texts.extend(self.proof.iter().map(|p| ("proof", p.as_str())));
        texts.extend(self.gaps.iter().map(|g| ("gaps", g.as_str())));
        texts.extend([
            ("gaps_label", self.gaps_label.as_str()),
            ("pictures_label", self.pictures_label.as_str()),
            ("plan_followed.label", self.plan_followed.label.as_str()),
        ]);
        let mut label = |field: &str, text: &str, needed: bool, at_most: usize| {
            let n = words(text);
            if needed && n == 0 {
                out.push(format!(
                    "{field} is empty: the table needs it, in at most {at_most} words"
                ));
            } else if n > at_most {
                out.push(format!("{field} has {n} words; at most {at_most}"));
            }
        };
        let has_gaps = self.gaps.iter().any(|g| !g.trim().is_empty());
        label("gaps_label", &self.gaps_label, has_gaps, LABEL_WORDS);
        label("pictures_label", &self.pictures_label, false, LABEL_WORDS);
        label(
            "plan_followed.label",
            &self.plan_followed.label,
            self.plan_followed.off(),
            LABEL_WORDS,
        );
        for call in &self.your_calls {
            label(
                &format!("your_calls: \"{}\" label", call.call),
                &call.label,
                true,
                CALL_LABEL_WORDS,
            );
        }
        for call in &self.your_calls {
            texts.extend([
                ("your_calls.call", call.call.as_str()),
                ("your_calls.picked", call.picked.as_str()),
                ("your_calls.alternative", call.alternative.as_str()),
                ("your_calls.label", call.label.as_str()),
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
        what_users_get: "<one plain sentence: what changes for the people who use it>".into(),
        proof: vec![
            "<only what the PR's checks don't show, such as a manual run; not the tests or \
             checks CI runs; leave the list empty if nothing>"
                .into(),
        ],
        gaps: vec![
            "<what you didn't do or couldn't check; leave the list empty if nothing>".into(),
        ],
        gaps_label: "<the gaps in at most 8 words; empty if there are none>".into(),
        pictures_label: "<why a change users can see has no pictures, in at most 8 words; \
                         empty otherwise>"
            .into(),
        your_calls: vec![Call {
            call: "<a choice you made for the owner>".into(),
            picked: "<what you picked>".into(),
            alternative: "<the option you didn't>".into(),
            tag: "copy".into(),
            label: "<the call in at most 4 words>".into(),
        }],
        flag: None,
        docs_touched: Vec::new(),
        plan_followed: PlanFollowed {
            label: "<why you went off plan, in at most 8 words; empty if on plan>".into(),
            ..PlanFollowed::default()
        },
    };
    serde_json::to_string_pretty(&facts).unwrap_or_default()
}

/// How many words `text` has.
pub fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// The proof the PR's checks don't already show: an item that speaks of
/// tests, types, lint, CI or one of `checks` by name is left out, unless
/// it says it was run by hand (`ran the export locally`).
pub fn beyond_checks<'a>(proof: &'a [String], checks: &[&str]) -> Vec<&'a str> {
    const BY_HAND: [&str; 6] = [
        "manual",
        "locally",
        "by hand",
        "clicked",
        "tried",
        "on the preview",
    ];
    const CI: [&str; 14] = [
        "test",
        "typecheck",
        "type check",
        "type-check",
        "tsc",
        "lint",
        "clippy",
        "fmt",
        "format",
        "ci",
        "check",
        "build",
        "e2e",
        "spec",
    ];
    proof
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let lower = p.to_ascii_lowercase();
            if BY_HAND.iter().any(|w| lower.contains(w)) {
                return true;
            }
            let restates = CI
                .iter()
                .copied()
                .chain(checks.iter().map(|c| c.trim()))
                .any(|w| speaks_of(&lower, w));
            !restates
        })
        .collect()
}

/// Whether `lower` (lowercased) names `word`: as a phrase when it has a
/// space, else as a word of its own or with `s`, `es`, `ed` or `ing` on.
fn speaks_of(lower: &str, word: &str) -> bool {
    let word = word.to_ascii_lowercase();
    if word.is_empty() {
        return false;
    }
    if word.contains(' ') {
        return lower.contains(&word);
    }
    lower
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter_map(|said| said.strip_prefix(word.as_str()))
        .any(|rest| matches!(rest, "" | "s" | "es" | "ed" | "ing"))
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
                label: "sign-in wording".into(),
            }],
            flag: Some("new-login".into()),
            docs_touched: vec!["docs/auth.md".into()],
            plan_followed: PlanFollowed {
                followed: false,
                deviations: vec!["kept the old cookie name".into()],
                label: "kept the old cookie name".into(),
            },
            ..Default::default()
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
    fn the_labels_the_table_needs_are_asked_for_and_kept_short() {
        let mut f = sample();
        f.your_calls[0].label = String::new();
        f.plan_followed.label = String::new();
        f.gaps = vec!["no second account to test sharing with".into()];
        let problems = f.problems();
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(
            problems[0].starts_with("gaps_label is empty"),
            "{problems:?}"
        );
        assert!(problems[1].starts_with("plan_followed.label is empty"));
        assert!(problems[2].contains("\"Button copy\" label is empty"));
        f.your_calls[0].label = "the words on the sign-in button".into();
        f.plan_followed.label = "kept the old cookie name".into();
        f.gaps_label = "sharing untested".into();
        f.pictures_label = "one two three four five six seven eight nine".into();
        let problems = f.problems();
        assert_eq!(
            problems,
            [
                "pictures_label has 9 words; at most 8",
                "your_calls: \"Button copy\" label has 6 words; at most 4",
            ]
        );
        // On plan needs no label; nor do no gaps.
        let mut f = sample();
        f.plan_followed = PlanFollowed::default();
        assert!(f.problems().is_empty());
        // One plain sentence.
        f.what_users_get = "word ".repeat(SENTENCE_WORDS + 1);
        assert!(f.problems()[0].contains("one plain sentence"));
    }

    #[test]
    fn proof_the_checks_already_show_is_left_out() {
        let proof: Vec<String> = [
            "typecheck passed",
            "Unit tests pass",
            "added a unit test for the redirect",
            "CI green",
            "storybook-a11y green",
            "Ran the export locally against a 10k-row table",
            "Opened the dialog on the preview and saved twice",
            "Measured the bundle: 2 KB smaller",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            beyond_checks(&proof, &["storybook-a11y", "unit"]),
            [
                "Ran the export locally against a 10k-row table",
                "Opened the dialog on the preview and saved twice",
                "Measured the bundle: 2 KB smaller",
            ]
        );
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
