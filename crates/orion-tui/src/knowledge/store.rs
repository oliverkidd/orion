//! The HARVESTED PAGES: `.orion/decisions/harvested/` in a checkout of
//! the repo, committed like any other file and changed only through
//! orion's knowledge pull request (`super::share`).
//!
//! - `index.md`: at most [`INDEX_LINES`] lines, one per area with its
//!   topics' short names and its rule count — what every builder prompt
//!   carries;
//! - `areas.md`: the areas a topic may sit in (`super::areas`);
//! - `<area>/<topic>.md`: a topic page in the fixed sections
//!   ([`SECTIONS`]), its scope line short and naming no files
//!   ([`short_scope`]), each rule ending in a hidden `<!-- k… -->` list
//!   of the entries it stands on, so a veto can take one out;
//! - `entries.jsonl`: every entry ever extracted, with its status;
//! - `checked.jsonl`: the commit and date each topic was last checked
//!   against the code, so the weekly staleness check one person's orion
//!   ran spares everyone else's ([`TopicCheck`]).
//!
//! Every file orion writes starts with [`MARKER`]. orion owns only
//! [`DIR`]: everything else under [`DECISIONS`] is a hand-written record,
//! read-only to orion ([`Store::write`] refuses any path outside). The
//! condensed dossiers stay on this machine (`super::local`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The repo's decision records, hand-written and harvested.
pub(crate) const DECISIONS: &str = ".orion/decisions";
/// The part of [`DECISIONS`] orion owns, and the only one it writes.
pub(crate) const DIR: &str = ".orion/decisions/harvested";
/// The first line of every file orion writes.
pub(crate) const MARKER: &str =
    "<!-- orion:harvested — written by orion; edit through a PR, it is regenerated -->";
/// The most lines `index.md` may have, its marker line among them.
pub(crate) const INDEX_LINES: usize = 40;
/// A topic page's sections, in order.
pub(crate) const SECTIONS: [&str; 4] = [
    "Rules in force",
    "Constraints",
    "Open questions",
    "Replaced",
];
/// The sections whose bullets count as rules.
const RULE_SECTIONS: [&str; 2] = ["Rules in force", "Constraints"];

/// An entry's standing in `entries.jsonl`.
pub(crate) const ACTIVE: &str = "active";
pub(crate) const DROPPED: &str = "dropped";
pub(crate) const VETOED: &str = "vetoed";

/// One extracted entry, as `entries.jsonl` keeps it: the extraction
/// spec's fields, plus where it came from and what became of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Entry {
    /// `k<pr>-<n>`: stable, and what a page's rule cites.
    pub id: String,
    pub pr: u64,
    pub pr_url: String,
    pub stream: String,
    pub kind: String,
    pub topic: String,
    pub rule: String,
    pub applies_to: Vec<String>,
    pub why: String,
    pub by: String,
    pub confidence: String,
    pub quote: String,
    pub options_rejected: Vec<String>,
    pub confirm_by: String,
    pub source_at: String,
    /// RFC 3339, when the harvest recorded it.
    pub recorded_at: String,
    /// [`ACTIVE`], [`DROPPED`] (the check found it doesn't hold) or
    /// [`VETOED`] (the owner said no).
    pub status: String,
    /// Credited to the owner by the model, but his words weren't found.
    pub downgraded: bool,
    /// Why it was dropped, when it was.
    pub note: String,
    /// What a new topic the extraction made covers, in one line; never
    /// kept in `entries.jsonl`.
    #[serde(skip)]
    pub new_topic: String,
}

impl Entry {
    /// An entry as the extraction model wrote it: strings or lists
    /// where the spec has them, anything else left empty.
    pub(crate) fn from_model(v: &Value) -> Option<Entry> {
        let text = |key: &str| match v.get(key) {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|i| i.as_str().map_or_else(|| i.to_string(), str::to_string))
                .collect::<Vec<_>>()
                .join("; "),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let list = |key: &str| match v.get(key) {
            Some(Value::Array(items)) => items
                .iter()
                .map(|i| i.as_str().map_or_else(|| i.to_string(), str::to_string))
                .filter(|s| !s.trim().is_empty())
                .collect(),
            Some(Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
            _ => Vec::new(),
        };
        let entry = Entry {
            stream: text("stream"),
            kind: text("kind"),
            topic: topic_path(&text("topic")),
            rule: text("rule"),
            applies_to: list("applies_to"),
            why: text("why"),
            by: text("by").to_lowercase(),
            confidence: text("confidence").to_lowercase(),
            quote: text("quote"),
            options_rejected: list("options_rejected"),
            confirm_by: text("confirm_by"),
            source_at: text("source_at"),
            new_topic: text("new_topic"),
            status: ACTIVE.into(),
            ..Entry::default()
        };
        (!entry.rule.is_empty()).then_some(entry)
    }

    /// Whether the daily card lists it: the owner's own, his pick, or a
    /// plan he approved.
    pub(crate) fn is_owners(&self) -> bool {
        matches!(self.by.as_str(), "owner" | "owner-pick" | "plan")
    }
}

/// `topic` as a page path: `area/topic`, kebab-case, `general/…` when the
/// model gave only one part.
pub(crate) fn topic_path(topic: &str) -> String {
    let kebab = |part: &str| {
        let mut out = String::new();
        for c in part.trim().chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
        out.trim_matches('-').to_string()
    };
    let parts: Vec<String> = topic
        .split('/')
        .map(kebab)
        .filter(|p| !p.is_empty())
        .collect();
    match parts.as_slice() {
        [] => "general/misc".into(),
        [one] => format!("general/{one}"),
        [area, topic, ..] => format!("{area}/{topic}"),
    }
}

/// The harvested pages of one checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Store {
    /// The checkout: a session's worktree, or orion's working clone.
    pub repo: PathBuf,
    /// `<repo>/.orion/decisions/harvested`.
    pub dir: PathBuf,
}

impl Store {
    pub(crate) fn of(repo: &Path) -> Self {
        Store {
            repo: repo.to_path_buf(),
            dir: repo.join(DIR),
        }
    }

    pub(crate) fn entries_path(&self) -> PathBuf {
        self.dir.join("entries.jsonl")
    }

    /// Every entry recorded, oldest first. A line that doesn't read (the
    /// marker among them) is skipped.
    pub(crate) fn entries(&self) -> Vec<Entry> {
        read_entries(&std::fs::read_to_string(self.entries_path()).unwrap_or_default())
    }

    /// Add `new` to the end of `entries.jsonl`.
    pub(crate) fn append(&self, new: &[Entry]) -> Result<(), String> {
        let mut all = self.entries();
        all.extend(new.iter().cloned());
        self.save_entries(&all)
    }

    /// Write `all` as the whole of `entries.jsonl`.
    pub(crate) fn save_entries(&self, all: &[Entry]) -> Result<(), String> {
        self.write(&self.entries_path(), &render_entries(all)?)
    }

    pub(crate) fn checked_path(&self) -> PathBuf {
        self.dir.join(CHECKED_FILE)
    }

    /// When each topic was last checked against the code, by anyone.
    pub(crate) fn checked(&self) -> BTreeMap<String, TopicCheck> {
        read_checked(&std::fs::read_to_string(self.checked_path()).unwrap_or_default())
    }

    /// Write `all` as the whole of `checked.jsonl`, a topic a line.
    pub(crate) fn save_checked(&self, all: &BTreeMap<String, TopicCheck>) -> Result<(), String> {
        let mut text = String::new();
        for one in all.values() {
            text.push_str(&serde_json::to_string(one).map_err(|e| e.to_string())?);
            text.push('\n');
        }
        self.write(&self.checked_path(), &text)
    }

    /// Every topic with a page, sorted.
    pub(crate) fn topics(&self) -> Vec<String> {
        let mut topics = Vec::new();
        for area in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let name = area.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !area.path().is_dir() {
                continue;
            }
            for page in std::fs::read_dir(area.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let file = page.file_name().to_string_lossy().into_owned();
                if let Some(topic) = file.strip_suffix(".md") {
                    topics.push(format!("{name}/{topic}"));
                }
            }
        }
        topics.sort();
        topics
    }

    pub(crate) fn page_path(&self, topic: &str) -> PathBuf {
        self.dir.join(page_file(topic))
    }

    /// `topic`'s page as the checkout has it, [`MARKER`] left off.
    pub(crate) fn page(&self, topic: &str) -> Option<String> {
        std::fs::read_to_string(self.page_path(topic))
            .ok()
            .map(|t| unmarked(&t).to_string())
    }

    pub(crate) fn write_page(&self, topic: &str, text: &str) -> Result<(), String> {
        self.write(&self.page_path(topic), text)
    }

    pub(crate) fn index_path(&self) -> PathBuf {
        self.dir.join("index.md")
    }

    /// `index.md`, [`MARKER`] left off; None while there is none.
    pub(crate) fn index(&self) -> Option<String> {
        std::fs::read_to_string(self.index_path())
            .ok()
            .map(|t| unmarked(&t).to_string())
            .filter(|i| !i.trim().is_empty())
    }

    /// `index.md` written afresh from the pages, by area.
    pub(crate) fn write_index(&self, project: &str) -> Result<(), String> {
        let areas = super::areas::Areas::load(self);
        let order: Vec<String> = areas.names().into_iter().map(str::to_string).collect();
        self.write(
            &self.index_path(),
            &render_index(project, &order, &self.pages()),
        )
    }

    /// Every page, with its topic, sorted.
    pub(crate) fn pages(&self) -> Vec<(String, String)> {
        self.topics()
            .into_iter()
            .filter_map(|t| Some((t.clone(), self.page(&t)?)))
            .collect()
    }

    /// Every topic with a page, and its scope line.
    pub(crate) fn topic_scopes(&self) -> Vec<(String, String)> {
        self.pages()
            .into_iter()
            .map(|(topic, page)| {
                let scope = short_scope(&scope(&page), &topic);
                (topic, scope)
            })
            .collect()
    }

    /// Delete `topic`'s page, and its area's folder once empty.
    pub(crate) fn remove_page(&self, topic: &str) -> Result<(), String> {
        let path = self.page_path(topic);
        if !self.may_write(&path) {
            return Err(format!("orion removes only under {DIR}/"));
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("couldn't remove {}: {e}", path.display())),
        }
        if let Some(folder) = path.parent() {
            // Only when empty: remove_dir refuses otherwise.
            let _ = std::fs::remove_dir(folder);
        }
        Ok(())
    }

    /// Mark `id` vetoed, and take it out of its page and the index. The
    /// caller commits and pushes (`super::share`).
    pub(crate) fn veto(&self, id: &str, project: &str) -> Result<(), String> {
        let mut all = self.entries();
        let Some(entry) = all.iter_mut().find(|e| e.id == id) else {
            return Err(format!("no entry {id}"));
        };
        entry.status = VETOED.into();
        let topic = entry.topic.clone();
        self.save_entries(&all)?;
        if let Some(page) = self.page(&topic) {
            self.write_page(&topic, &without_ids(&page, &[id]))?;
        }
        self.write_index(project)?;
        Ok(())
    }

    /// Whether orion may write `path`: a file inside [`DIR`], with no `..`
    /// or other way out of it.
    pub(crate) fn may_write(&self, path: &Path) -> bool {
        use std::path::Component;
        path.strip_prefix(&self.dir).is_ok_and(|rest| {
            rest.components().count() > 0
                && rest.components().all(|c| matches!(c, Component::Normal(_)))
        })
    }

    /// Write `text` to `path`, its folders made and [`MARKER`] its first
    /// line. Refused anywhere outside [`DIR`]: the hand-written records
    /// beside it are never orion's to change.
    pub(crate) fn write(&self, path: &Path, text: &str) -> Result<(), String> {
        if !self.may_write(path) {
            return Err(format!(
                "orion writes only under {DIR}/, not {}",
                path.display()
            ));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, marked(text))
            .map_err(|e| format!("couldn't write {}: {e}", path.display()))
    }
}

/// `topic`'s page, relative to [`DIR`].
pub(crate) fn page_file(topic: &str) -> String {
    format!("{}.md", topic_path(topic))
}

/// `text` with [`MARKER`] as its first line, once.
pub(crate) fn marked(text: &str) -> String {
    format!("{MARKER}\n{}", unmarked(text))
}

/// `text` without its [`MARKER`] line.
pub(crate) fn unmarked(text: &str) -> &str {
    match text.strip_prefix(MARKER) {
        Some(rest) => rest.strip_prefix('\n').unwrap_or(rest),
        None => text,
    }
}

/// `checked.jsonl`, beside the pages.
pub(crate) const CHECKED_FILE: &str = "checked.jsonl";

/// When a topic's rules were last checked against the code: the base
/// commit they were checked at, and the local date (`YYYY-MM-DD`) of the
/// check. Kept in the repo, not on one machine, so every orion on the
/// project sees a check any of them made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TopicCheck {
    pub topic: String,
    pub commit: String,
    pub on: String,
}

/// `checked.jsonl`'s text, by topic: a line that doesn't read is skipped,
/// and of two lines for one topic the later check wins.
pub(crate) fn read_checked(text: &str) -> BTreeMap<String, TopicCheck> {
    let mut all = BTreeMap::new();
    merge_checked(
        &mut all,
        text.lines().filter_map(|l| serde_json::from_str(l).ok()),
    );
    all
}

/// Fold `more` into `into`, keeping each topic's latest check.
pub(crate) fn merge_checked(
    into: &mut BTreeMap<String, TopicCheck>,
    more: impl IntoIterator<Item = TopicCheck>,
) {
    for one in more {
        match into.get(&one.topic) {
            Some(held) if held.on >= one.on => {}
            _ => {
                into.insert(one.topic.clone(), one);
            }
        }
    }
}

/// `entries.jsonl`'s text as entries: a line that doesn't read is skipped.
pub(crate) fn read_entries(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// `all` as `entries.jsonl`'s text, one entry a line ([`MARKER`] goes on
/// when it is written).
pub(crate) fn render_entries(all: &[Entry]) -> Result<String, String> {
    let mut text = String::new();
    for entry in all {
        text.push_str(&serde_json::to_string(entry).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    Ok(text)
}

/// A page made from its entries alone, with no model: what a rebuild
/// writes for a topic both sides changed (`super::share`). Rules,
/// constraints and questions in force; vetoed and dropped entries left
/// out. Its scope is the topic's name: no paths.
pub(crate) fn render_page(topic: &str, entries: &[&Entry]) -> String {
    let mut sections: [Vec<String>; 3] = Default::default();
    for e in entries.iter().filter(|e| e.status == ACTIVE) {
        let call = if e.by == "agent" { " (agent call)" } else { "" };
        let at = match e.kind.as_str() {
            "constraint" => 1,
            "question" => 2,
            _ => 0,
        };
        sections[at].push(format!("- {} (#{}){call} <!-- {} -->", e.rule, e.pr, e.id));
    }
    let mut out = format!("# {topic}\nScope: {}\n", short_scope("", topic));
    for (i, name) in SECTIONS.iter().enumerate() {
        out.push_str(&format!("\n## {name}\n"));
        for line in sections.get(i).into_iter().flatten() {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The most words a page's scope line holds.
pub(crate) const SCOPE_WORDS: usize = 14;

/// Whether `word` names a file or folder: a path, a dotfile, or a name
/// with an extension.
fn is_path(word: &str) -> bool {
    let w =
        word.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '(' | ')' | '"' | '\'' | '`'));
    if w.contains('/') || w.contains('\\') || (w.starts_with('.') && w.len() > 1) {
        return true;
    }
    match w.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// `text` as a page's scope line: no code, no parentheses, no file
/// paths, at most [`SCOPE_WORDS`] words — cut back to a whole item of a
/// list where it can be. With nothing left, the topic's name in words.
pub(crate) fn short_scope(text: &str, topic: &str) -> String {
    let mut plain = String::new();
    let (mut code, mut depth) = (false, 0usize);
    for c in text.chars() {
        match c {
            '`' => code = !code,
            '(' if !code => depth += 1,
            ')' if !code => depth = depth.saturating_sub(1),
            _ if code || depth > 0 => {}
            c => plain.push(c),
        }
    }
    // A list's comma left on its own, the code before it gone, goes back
    // on the word before.
    let mut words: Vec<String> = Vec::new();
    for w in plain.split_whitespace().filter(|w| !is_path(w)) {
        if w.chars().any(|c| c.is_alphanumeric()) {
            words.push(w.to_string());
        } else if let Some(last) = words.last_mut() {
            if w.contains([',', ';']) && !last.ends_with([',', ';']) {
                last.push(',');
            }
        }
    }
    let mut kept: Vec<String> = words.iter().take(SCOPE_WORDS).cloned().collect();
    if words.len() > SCOPE_WORDS {
        if let Some(cut) = kept
            .iter()
            .rposition(|w| w.ends_with(',') || w.ends_with(';'))
        {
            if cut >= 3 {
                kept.truncate(cut + 1);
            }
        }
    }
    let line = kept
        .join(" ")
        .trim_end_matches([',', ';', ':', '—', '-', ' '])
        .to_string();
    match line.is_empty() {
        true => {
            let name = topic.rsplit('/').next().unwrap_or(topic);
            name.replace('-', " ")
        }
        false => line,
    }
}

/// `page` as `topic`'s: its title the topic, and its scope line `scope`
/// (else its own) made short ([`short_scope`]).
pub(crate) fn tidy_page(topic: &str, page: &str, scope_hint: Option<&str>) -> String {
    let own = scope(page);
    let wanted = match scope_hint.map(str::trim).filter(|s| !s.is_empty()) {
        Some(hint) => hint.to_string(),
        None => own,
    };
    let scope_line = format!("Scope: {}", short_scope(&wanted, topic));
    let mut out: Vec<String> = Vec::new();
    let (mut titled, mut scoped) = (false, false);
    for line in page.lines() {
        if !titled && line.starts_with("# ") {
            out.push(format!("# {topic}"));
            titled = true;
        } else if !scoped && line.trim().starts_with("Scope:") {
            out.push(scope_line.clone());
            scoped = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !titled {
        out.insert(0, format!("# {topic}"));
    }
    if !scoped {
        out.insert(1, scope_line);
    }
    out.join("\n") + "\n"
}

/// The ids a page line cites, in its `<!-- … -->`.
pub(crate) fn line_ids(line: &str) -> Vec<&str> {
    let Some(start) = line.rfind("<!--") else {
        return Vec::new();
    };
    let rest = &line[start + 4..];
    let rest = rest.find("-->").map_or(rest, |end| &rest[..end]);
    rest.split([' ', ','])
        .filter(|w| w.starts_with('k') && w.contains('-'))
        .collect()
}

/// `page` with `ids` taken off every rule, and a rule left citing none of
/// its entries gone.
pub(crate) fn without_ids(page: &str, ids: &[&str]) -> String {
    let mut out = Vec::new();
    for line in page.lines() {
        let cited = line_ids(line);
        if cited.is_empty() || !cited.iter().any(|c| ids.contains(c)) {
            out.push(line.to_string());
            continue;
        }
        let left: Vec<&str> = cited.into_iter().filter(|c| !ids.contains(c)).collect();
        if left.is_empty() {
            continue;
        }
        let start = line.rfind("<!--").unwrap_or(line.len());
        out.push(format!("{}<!-- {} -->", &line[..start], left.join(" ")));
    }
    let mut text = out.join("\n");
    if page.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// How many rules a page holds: the bullets under its rule sections.
pub(crate) fn rule_count(page: &str) -> usize {
    let mut counting = false;
    let mut n = 0;
    for line in page.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            counting = RULE_SECTIONS.contains(&head.trim());
        } else if counting && (line.starts_with("- ") || line.starts_with("* ")) {
            n += 1;
        }
    }
    n
}

/// A page's `Scope:` line, without the label.
pub(crate) fn scope(page: &str) -> String {
    page.lines()
        .find_map(|l| l.trim().strip_prefix("Scope:"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// What the index's last line says.
pub(crate) const POINTER: &str = "Topic pages: .orion/decisions/harvested/<area>/<topic>.md";
/// The most topic names an area's line lists.
const LINE_TOPICS: usize = 16;

/// `index.md` for `pages` (topic, text): one line per area — `order`'s
/// first, then any other a page sits in — naming its topics, short, and
/// counting its rules; then where the pages are. At most [`INDEX_LINES`]
/// lines, however many topics there are.
pub(crate) fn render_index(project: &str, order: &[String], pages: &[(String, String)]) -> String {
    let mut areas: Vec<(String, Vec<String>, usize)> =
        order.iter().map(|a| (a.clone(), Vec::new(), 0)).collect();
    for (topic, page) in pages {
        let (area, name) = topic.split_once('/').unwrap_or(("general", topic));
        let at = match areas.iter().position(|(a, _, _)| a == area) {
            Some(at) => at,
            None => {
                areas.push((area.to_string(), Vec::new(), 0));
                areas.len() - 1
            }
        };
        areas[at].1.push(name.to_string());
        areas[at].2 += rule_count(page);
    }
    areas.retain(|(_, topics, _)| !topics.is_empty());
    let mut lines = vec![
        format!("# Knowledge: {project}"),
        format!(
            "{} topics in {} areas, recorded from merged pull requests. One line per area: its topics, then its rules.",
            pages.len(),
            areas.len()
        ),
        String::new(),
    ];
    // Room for the areas: the file's marker comes first, a blank line
    // and the pointer last.
    let room = INDEX_LINES - 1 - lines.len() - 2;
    let shown = match areas.len() > room {
        true => room - 1,
        false => areas.len(),
    };
    for (area, topics, rules) in &areas[..shown] {
        let mut names = topics.clone();
        names.sort();
        let more = names.len().saturating_sub(LINE_TOPICS);
        names.truncate(LINE_TOPICS);
        let mut list = names.join(", ");
        if more > 0 {
            list.push_str(&format!(" and {more} more"));
        }
        let noun = if *rules == 1 { "rule" } else { "rules" };
        lines.push(format!("- {area}: {list} — {rules} {noun}"));
    }
    if shown < areas.len() {
        lines.push(format!("- … and {} more areas", areas.len() - shown));
    }
    lines.push(String::new());
    lines.push(POINTER.to_string());
    lines.join("\n") + "\n"
}

/// Whether the repo at `repo` ignores [`DECISIONS`] — or the harvested
/// pages in it — so harvested rules can't be committed and shared.
pub(crate) fn ignored(repo: &Path) -> bool {
    let index = format!("{DIR}/index.md");
    crate::git_proc::read(
        repo,
        &["check-ignore", "--no-index", ".orion/decisions/", &index],
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const PAGE: &str = "# auth/login\nScope: src/auth, login flow\n\n## Rules in force\n- Login returns to the page the user came from (#12) <!-- k12-1 k30-2 -->\n- Sessions last 30 days (#14) <!-- k14-1 -->\n\n## Constraints\n- The SSO provider strips fragments (#12) <!-- k12-3 -->\n\n## Open questions\n- Remember me? <!-- k15-1 -->\n\n## Replaced\n";

    #[test]
    fn topics_become_two_part_kebab_paths() {
        assert_eq!(topic_path("Auth/Login Redirect"), "auth/login-redirect");
        assert_eq!(topic_path("billing"), "general/billing");
        assert_eq!(topic_path("a/b/c"), "a/b");
        assert_eq!(topic_path(""), "general/misc");
    }

    #[test]
    fn the_index_is_one_line_an_area_with_its_topics_and_rules() {
        let pages = vec![
            ("access-and-accounts/login".to_string(), PAGE.to_string()),
            (
                "editor/forms".to_string(),
                "# editor/forms\nScope: forms in src/ui/forms.tsx\n## Rules in force\n- one <!-- k1-1 -->\n".to_string(),
            ),
            (
                "editor/tables".to_string(),
                "# editor/tables\n## Rules in force\n- two <!-- k2-1 -->\n".to_string(),
            ),
            (
                "legacy/old".to_string(),
                "# legacy/old\n## Rules in force\n".to_string(),
            ),
        ];
        let order: Vec<String> = ["editor", "charts", "access-and-accounts"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            render_index("riplo-os", &order, &pages),
            "# Knowledge: riplo-os\n4 topics in 3 areas, recorded from merged pull requests. One line per area: its topics, then its rules.\n\n- editor: forms, tables — 2 rules\n- access-and-accounts: login — 3 rules\n- legacy: old — 0 rules\n\nTopic pages: .orion/decisions/harvested/<area>/<topic>.md\n"
        );
    }

    #[test]
    fn the_index_never_runs_past_its_lines() {
        let order: Vec<String> = crate::knowledge::areas::SEED
            .iter()
            .map(|(name, _, _)| name.to_string())
            .collect();
        // 300 topics: in five listed areas, and in 40 areas of their own.
        let pages: Vec<(String, String)> = (0..300)
            .map(|i| {
                let area = match i % 2 {
                    0 => order[i % order.len()].clone(),
                    _ => format!("stray-{}", i % 80),
                };
                let page = format!(
                    "# t\nScope: src/a/b{i}.ts and more\n## Rules in force\n- r <!-- k{i}-1 -->\n"
                );
                (format!("{area}/topic-number-{i:03}"), page)
            })
            .collect();
        let index = render_index("p", &order, &pages);
        assert_eq!(
            index.lines().count(),
            INDEX_LINES - 1,
            "the marker makes 40: {index}"
        );
        assert!(
            index.starts_with("# Knowledge: p\n300 topics in 45 areas"),
            "{index}"
        );
        assert!(
            index.ends_with(&format!("- … and 12 more areas\n\n{POINTER}\n")),
            "{index}"
        );
        assert!(!index.contains("src/"), "no paths: {index}");
        assert!(!index.contains("Scope"), "no scope text: {index}");
        assert!(index.contains(" and 14 more — 30 rules"), "{index}");

        // A store's own index stays inside its lines too.
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        for (topic, page) in &pages {
            store.write_page(topic, page).unwrap();
        }
        store.write_index("p").unwrap();
        let file = std::fs::read_to_string(store.index_path()).unwrap();
        assert_eq!(file.lines().count(), INDEX_LINES, "{file}");
    }

    #[test]
    fn scope_lines_are_short_and_name_no_files() {
        let long = "local stack commands (`just status`, `just start`/`stop`), `.next`/`.next-e2e`, stack health, offline auth, local seeding for visual checks, stack needs of local e2e and the pre-commit hook";
        let short = short_scope(long, "engineering-tooling/local-stack");
        assert_eq!(
            short,
            "local stack commands, stack health, offline auth, local seeding for visual checks"
        );
        for scope in [
            long,
            "src/lib/presentations/deck-logo.ts, apps/os/SaveNoticeToast.tsx and the toolbar",
            "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen",
        ] {
            let short = short_scope(scope, "a/b");
            assert!(short.split_whitespace().count() < 15, "{short}");
            assert!(!short.split_whitespace().any(is_path), "{short}");
        }
        assert_eq!(short_scope("`src/x.ts`", "editor/text-boxes"), "text boxes");

        let page = tidy_page("editor/text", PAGE, None);
        assert!(
            page.starts_with("# editor/text\nScope: login flow\n\n## Rules in force\n"),
            "{page}"
        );
        let page = tidy_page(
            "editor/text",
            "## Rules in force\n- a <!-- k1-1 -->\n",
            Some("Text boxes and fonts"),
        );
        assert_eq!(
            page,
            "# editor/text\nScope: Text boxes and fonts\n## Rules in force\n- a <!-- k1-1 -->\n"
        );
    }

    #[test]
    fn a_veto_takes_its_entry_off_the_page() {
        let page = without_ids(PAGE, &["k12-1", "k14-1"]);
        assert!(
            page.contains("- Login returns to the page the user came from (#12) <!-- k30-2 -->")
        );
        assert!(!page.contains("Sessions last 30 days"));
        assert!(page.contains("strips fragments"));
        assert_eq!(rule_count(&page), 2);
    }

    fn entry(id: &str, topic: &str, kind: &str, by: &str) -> Entry {
        Entry {
            id: id.into(),
            pr: 12,
            topic: topic.into(),
            kind: kind.into(),
            rule: format!("rule {id}"),
            by: by.into(),
            status: ACTIVE.into(),
            ..Entry::default()
        }
    }

    #[test]
    fn every_file_orion_writes_is_marked_and_reads_back_without_it() {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        store
            .append(&[entry("k12-1", "auth/login", "decision", "owner")])
            .unwrap();
        store.write_page("auth/login", PAGE).unwrap();
        store.write_index("demo").unwrap();
        for file in ["entries.jsonl", "auth/login.md", "index.md"] {
            let text = std::fs::read_to_string(store.dir.join(file)).unwrap();
            assert!(text.starts_with(&format!("{MARKER}\n")), "{file}: {text}");
            assert_eq!(text.matches(MARKER).count(), 1, "{file}");
        }
        assert_eq!(store.page("auth/login").unwrap(), PAGE);
        assert!(store.index().unwrap().starts_with("# Knowledge: demo\n"));
        assert_eq!(store.entries().len(), 1, "the marker line is skipped");
        assert_eq!(store.topics(), vec!["auth/login"]);
        // Written twice, still one marker.
        store.write_page("auth/login", &marked(PAGE)).unwrap();
        assert_eq!(store.page("auth/login").unwrap(), PAGE);

        store.veto("k12-1", "demo").unwrap();
        assert_eq!(store.entries()[0].status, VETOED);
        assert!(store.page("auth/login").unwrap().contains("<!-- k30-2 -->"));
    }

    #[test]
    fn the_writer_refuses_anything_outside_the_harvested_folder() {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        let record = repo.path().join(DECISIONS).join("0001-auth.md");
        std::fs::create_dir_all(record.parent().unwrap()).unwrap();
        std::fs::write(&record, "# Auth\nHand-written.\n").unwrap();
        for outside in [
            record.clone(),
            repo.path().join(DECISIONS).join("harvested-not/x.md"),
            store.dir.join("../0001-auth.md"),
            store.dir.join("auth/../../0001-auth.md"),
            store.dir.clone(),
            repo.path().join("README.md"),
            PathBuf::from("/tmp/x.md"),
        ] {
            let refused = store.write(&outside, "over").unwrap_err();
            assert!(refused.starts_with("orion writes only under"), "{refused}");
        }
        assert_eq!(
            std::fs::read_to_string(&record).unwrap(),
            "# Auth\nHand-written.\n",
            "the record is untouched"
        );
        // A topic can't climb out either: its path is made of kebab parts.
        assert_eq!(
            store.page_path("../../0001-auth"),
            store.dir.join("general/0001-auth.md")
        );
        assert!(store.write(&store.dir.join("a/b.md"), "fine").is_ok());
    }

    #[test]
    fn a_rebuilt_page_is_made_from_its_entries() {
        let mut gone = entry("k12-4", "auth/login", "decision", "owner");
        gone.status = VETOED.into();
        let mut scoped = entry("k12-1", "auth/login", "decision", "owner");
        scoped.applies_to = vec!["src/auth".into()];
        let entries = [
            scoped,
            entry("k12-2", "auth/login", "constraint", "owner"),
            entry("k12-3", "auth/login", "decision", "agent"),
            gone,
        ];
        let refs: Vec<&Entry> = entries.iter().collect();
        let page = render_page("auth/login", &refs);
        assert_eq!(
            page,
            "# auth/login\nScope: login\n\n## Rules in force\n- rule k12-1 (#12) <!-- k12-1 -->\n- rule k12-3 (#12) (agent call) <!-- k12-3 -->\n\n## Constraints\n- rule k12-2 (#12) <!-- k12-2 -->\n\n## Open questions\n\n## Replaced\n"
        );
        assert_eq!(rule_count(&page), 3);
    }

    #[test]
    fn the_gitignore_check_asks_about_the_decisions_folder() {
        let repo = tempfile::tempdir().unwrap();
        crate::git_proc::run(repo.path(), &["init", "-q"]).unwrap();
        assert!(!ignored(repo.path()));
        std::fs::write(repo.path().join(".gitignore"), ".orion/pr-contract.toml\n").unwrap();
        assert!(!ignored(repo.path()));
        std::fs::write(repo.path().join(".gitignore"), ".orion/\n").unwrap();
        assert!(ignored(repo.path()));
        std::fs::write(repo.path().join(".gitignore"), "harvested/\n").unwrap();
        assert!(ignored(repo.path()), "the harvested pages alone");
    }

    #[test]
    fn model_entries_read_strings_or_lists() {
        let v = serde_json::json!({
            "stream": "product", "kind": "decision", "topic": "Auth/Login",
            "rule": "Login returns to the page the user came from",
            "applies_to": "src/auth", "by": "Owner", "options_rejected": ["dashboard"],
            "confidence": "High", "confirm_by": null
        });
        let e = Entry::from_model(&v).unwrap();
        assert_eq!(e.topic, "auth/login");
        assert_eq!(e.applies_to, vec!["src/auth"]);
        assert_eq!((e.by.as_str(), e.confidence.as_str()), ("owner", "high"));
        assert_eq!(e.status, ACTIVE);
        assert!(Entry::from_model(&serde_json::json!({"rule": ""})).is_none());
    }
}
