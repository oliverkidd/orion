//! NOTES: the rules a factory session hands over before it finishes, with
//! `orion knowledge note <file>` — entries in a harvest's own shape
//! (`super::harvest::ENTRY_FIELDS`), each on one of the repo's topics. They
//! wait under `<data_dir>/knowledge-notes/`, one file a pull request, until
//! it merges: its harvest then records them in place of an extraction,
//! held to the owner's words and checked against the code topic by topic
//! as any harvest's are (`super::harvest`), and the file goes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::areas::Areas;
use super::store::{topic_path, Entry};

/// Who a noted rule may be credited to: the harvest's own `by`s.
const BY: &[&str] = &["owner", "owner-pick", "plan", "review", "agent"];
/// The kinds of a product entry: all a note may be.
const KINDS: &[&str] = &["decision", "constraint", "question"];

/// One pull request's notes on disk.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    url: String,
    /// Each entry as the session wrote it: read as an [`Entry`] at merge,
    /// so a new topic's one-line scope (never kept in an entry) survives.
    entries: Vec<Value>,
}

/// `<data_dir>/knowledge-notes`.
pub(crate) fn dir() -> PathBuf {
    orion_core::paths::data_dir().join("knowledge-notes")
}

/// `text`, a session's notes — `{"entries": [...]}`, or the list alone —
/// checked against `areas`: each entry as written. Every problem is said,
/// a line each. An empty list is notes all the same: nothing lasting was
/// learned.
pub(crate) fn parse(text: &str, areas: &Areas) -> Result<Vec<Value>, Vec<String>> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| vec![format!("the notes aren't valid JSON: {e}")])?;
    let list = match &v {
        Value::Array(list) => list,
        Value::Object(o) => match o.get("entries") {
            Some(Value::Array(list)) => list,
            _ => return Err(vec!["the notes need an \"entries\" list".into()]),
        },
        _ => return Err(vec!["the notes need an \"entries\" list".into()]),
    };
    let mut problems = Vec::new();
    for (n, item) in list.iter().enumerate() {
        let n = n + 1;
        let Some(e) = Entry::from_model(item) else {
            problems.push(format!("entry {n} has no rule"));
            continue;
        };
        let raw = item
            .get("topic")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if raw.is_empty() {
            problems.push(format!("entry {n} ({}) has no topic", e.rule));
        } else {
            let area = topic_path(raw);
            let area = area.split('/').next().unwrap_or_default();
            if !areas.has(area) {
                problems.push(format!(
                    "entry {n}'s topic {raw} isn't in one of the areas: {}",
                    areas.names().join(", ")
                ));
            }
        }
        if e.is_engineering() {
            problems.push(format!(
                "entry {n} ({}) is about how agents work, not the product: orion records product rules only, and the repo's own agent docs hold the rest",
                e.rule
            ));
        }
        if !e.is_engineering() && !e.kind.is_empty() && !KINDS.contains(&e.kind.as_str()) {
            problems.push(format!(
                "entry {n}'s kind is {}, not one of {}",
                e.kind,
                KINDS.join(", ")
            ));
        }
        if !e.by.is_empty() && !BY.contains(&e.by.as_str()) {
            problems.push(format!(
                "entry {n}'s by is {}, not one of {}",
                e.by,
                BY.join(", ")
            ));
        }
        if matches!(e.by.as_str(), "owner" | "owner-pick") && e.quote.is_empty() {
            problems.push(format!(
                "entry {n} is credited to the owner but quotes nothing: quote his words, or credit it to the agent"
            ));
        }
    }
    match problems.is_empty() {
        true => Ok(list.clone()),
        false => Err(problems),
    }
}

fn read(dir: &Path, url: &str) -> Option<Stored> {
    let bytes = std::fs::read(crate::pr_body::facts::file_for(dir, url)).ok()?;
    let stored: Stored = serde_json::from_slice(&bytes).ok()?;
    (stored.url.trim() == url.trim()).then_some(stored)
}

/// Add `new` to the notes for pull request `url` under `dir`, a rule
/// noted before kept once: how many it has now.
pub(crate) fn add(dir: &Path, url: &str, new: Vec<Value>) -> std::io::Result<usize> {
    let mut stored = read(dir, url).unwrap_or_else(|| Stored {
        url: url.trim().to_string(),
        entries: Vec::new(),
    });
    let rule = |v: &Value| Entry::from_model(v).map(|e| e.rule.to_lowercase());
    for entry in new {
        let said = rule(&entry);
        if !stored.entries.iter().any(|old| rule(old) == said) {
            stored.entries.push(entry);
        }
    }
    let path = crate::pr_body::facts::file_for(dir, url);
    crate::pr_cache::write_json_atomic(&path, &stored)?;
    Ok(stored.entries.len())
}

/// The rules noted for pull request `url` under `dir`; None when its
/// session noted none (it is harvested whole).
pub(crate) fn read_for(dir: &Path, url: &str) -> Option<Vec<Entry>> {
    read(dir, url).map(|s| s.entries.iter().filter_map(Entry::from_model).collect())
}

/// Forget the notes for `url`: its harvest recorded them.
pub(crate) fn remove_for(dir: &Path, url: &str) {
    let _ = std::fs::remove_file(crate::pr_body::facts::file_for(dir, url));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notes_are_checked_against_the_areas_and_the_owners_words() {
        let areas = Areas::default();
        let good = json!({"entries": [
            {"stream": "product", "kind": "decision", "topic": "editor/text-boxes",
             "rule": "Text never shrinks below 8pt", "by": "owner", "quote": "never below 8pt"},
            {"stream": "product", "kind": "constraint", "topic": "editor/fonts",
             "new_topic": "Fonts the editor offers", "rule": "Webfonts load before first paint", "by": "agent"}
        ]})
        .to_string();
        assert_eq!(parse(&good, &areas).unwrap().len(), 2);
        let engineering = json!({"entries": [
            {"stream": "engineering", "kind": "gotcha", "topic": "editor/local-runs",
             "rule": "Probe ws first", "by": "agent"}
        ]})
        .to_string();
        let gotcha = json!({"entries": [
            {"stream": "product", "kind": "gotcha", "topic": "editor/x", "rule": "R", "by": "agent"}
        ]})
        .to_string();
        assert_eq!(
            parse(&gotcha, &areas).unwrap_err(),
            vec!["entry 1's kind is gotcha, not one of decision, constraint, question".to_string()]
        );
        assert_eq!(
            parse(&engineering, &areas).unwrap_err(),
            vec!["entry 1 (Probe ws first) is about how agents work, not the product: orion records product rules only, and the repo's own agent docs hold the rest".to_string()]
        );
        assert_eq!(
            parse("[]", &areas).unwrap().len(),
            0,
            "nothing learned is notes too"
        );

        let bad = json!([
            {"topic": "nowhere/thing", "rule": "A", "by": "owner"},
            {"topic": "editor/x", "rule": "B", "by": "boss"},
            {"topic": "editor/x"},
            {"rule": "C"}
        ])
        .to_string();
        let problems = parse(&bad, &areas).unwrap_err();
        assert!(
            problems[0]
                .starts_with("entry 1's topic nowhere/thing isn't in one of the areas: editor,"),
            "{problems:?}"
        );
        assert!(problems[1].starts_with("entry 1 is credited to the owner but quotes nothing"));
        assert_eq!(
            problems[2],
            "entry 2's by is boss, not one of owner, owner-pick, plan, review, agent"
        );
        assert_eq!(problems[3], "entry 3 has no rule");
        assert_eq!(problems[4], "entry 4 (C) has no topic");
        assert!(parse("{", &areas).is_err());
        assert!(parse(r#"{"rules": []}"#, &areas).is_err());
    }

    #[test]
    fn notes_wait_by_pull_request_until_its_harvest_takes_them() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://github.com/o/app/pull/12";
        assert_eq!(
            read_for(dir.path(), url),
            None,
            "nothing noted: harvested whole"
        );
        let one = json!({"topic": "editor/fonts", "new_topic": "Fonts on text boxes",
                         "rule": "Fonts fall back to Inter", "by": "agent"});
        let two = json!({"topic": "editor/fonts", "rule": "Fonts load once", "by": "agent"});
        assert_eq!(add(dir.path(), url, vec![one.clone()]).unwrap(), 1);
        assert_eq!(
            add(dir.path(), url, vec![one, two]).unwrap(),
            2,
            "a rule kept once"
        );
        let noted = read_for(dir.path(), url).unwrap();
        assert_eq!(noted[0].new_topic, "Fonts on text boxes");
        assert_eq!(
            read_for(dir.path(), "https://github.com/o/app/pull/13"),
            None
        );
        add(dir.path(), "https://github.com/o/app/pull/14", Vec::new()).unwrap();
        assert_eq!(
            read_for(dir.path(), "https://github.com/o/app/pull/14"),
            Some(Vec::new()),
            "noted as nothing: no extraction either"
        );
        remove_for(dir.path(), url);
        assert_eq!(read_for(dir.path(), url), None);
    }
}
