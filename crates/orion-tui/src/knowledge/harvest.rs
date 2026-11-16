//! HARVEST: one merged pull request into the harvested pages. Its dossier (the PR's
//! description and thread, and the condensed sessions that built it) is
//! read by Opus for entries ([`extract`]); the guards hold those to what
//! the owner said ([`super::guards`]); each topic they touch is checked
//! against the code and merged into its page by a second Opus call that
//! may read a checkout of the merge ([`check_topic`], [`CheckTree`]): a
//! temporary detached worktree, since the main checkout may not have the
//! merge yet. With no such checkout the entries are merged unchecked, at
//! low confidence, rather than lost. Then the pages, `entries.jsonl` and
//! `index.md` are written into the knowledge branch's checkout, and the
//! dossier kept on this machine; the caller commits and proposes them
//! (`super::share`). The repo's hand-written records are context to both
//! calls, and always win over a harvested rule.
//!
//! The model is a [`Model`]: the real one runs the Claude CLI
//! ([`ClaudeCli`]); a test hands in its own answers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::areas::Areas;
use super::condense::Line;
use super::guards;
use super::local::Local;
use super::share::Gh;
use super::store::{self, Entry, Store, ACTIVE, DROPPED, SECTIONS};
use super::{KnowledgeModels, Step};
use crate::oneshot::{self, Account};

/// The tools a check may read code with, and nothing else.
const READ_TOOLS: &str = "Read,Grep,Glob";
/// How long one ask may take: a big dossier, or a check reading code.
const ASK_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How much dossier one extraction reads, in chars.
pub(crate) const DOSSIER_CHARS: usize = 360_000;
/// A comment or description kept in a dossier, in chars.
const COMMENT_CUT: usize = 3_000;
const BODY_CUT: usize = 8_000;
/// A topic page's word budget.
pub(crate) const PAGE_WORDS: usize = 600;
/// How far past [`PAGE_WORDS`] a page runs before it is worth a warning.
const PAGE_SLACK: usize = PAGE_WORDS / 4;
/// A correction kept in a run record, in chars, and how many one keeps.
const CORRECTION_CUT: usize = 300;
const CORRECTIONS_MAX: usize = 20;

/// The extraction prompt: the plan's appendix, word for word.
pub(crate) const EXTRACT_SYSTEM: &str = r#"You are reading one merged pull request's dossier: its description, review comments and the condensed
transcripts of the agent sessions that built it. "OWNER" lines are the repo owner. "AGENT ASKED" / "OWNER
ANSWERED" are questions and answers. "AGENT PLAN" is a plan. Tool output was removed.
Extract only what a future agent working a different ticket in this repo would need, so it works the way
the owner wants without asking him again.

Streams. product: decision (a choice between behaviours the owner made, picked or approved), constraint (a
fact about the world that limits behaviour, with evidence), question (the owner hedged or left it open; never
turn a hedge into a decision). Write a product entry only if a user or client AI would notice it, a future
agent reading only the code could undo it, and the owner owned it (or an agent made a call he should be able
to overturn). engineering: gotcha (a trap that cost time, and what avoids it), tooling (how to reach access,
data, logs or environments, and what is allowed), convention (a rule the owner enforced that no lint, type or
test does), preference (how the owner wants agents to work with him, stated by him).

Never extract: progress, handovers; anything only true for this ticket; one-off authorisations ("use
--no-verify", "push it", "run the e2e", "just one PR here"): these are never precedent; mechanisms readable
from code; test choices or refactors with no behaviour choice; anything lint, types or tests enforce;
client names, emails or secrets.

by: owner (he said it; quote ≤2 lines verbatim from an OWNER line, required), owner-pick (he picked an
agent's option; quote the question and the picked label), plan (in a plan he approved; ≤ medium), review (a
human reviewer's comment acted on), agent (the agent decided; confidence low).

Topic: area/topic. Put each entry in one of the repo's listed topics unless none fits; several entries may share
one. Only then make a new topic: its area one of the listed areas, its name one to three kebab-case words, and
give new_topic: one line (under 15 words, no file paths) saying what it covers.
Each entry: stream, kind, topic, new_topic (only for a new topic), rule (≤30 words, true without the ticket),
applies_to, why (≤60 words or "Not recorded"), by, confidence, quote, options_rejected, confirm_by (a symbol,
test, command or file to check it still holds; never a line number), source_at.
Return JSON: {"entries":[...], "considered_and_skipped":["..."]}."#;

/// The check-and-merge call's rules, [`PAGE_WORDS`] said in them.
pub(super) fn check_system() -> String {
    format!(
        r#"You keep one topic page of a repository's knowledge store: the rules its owner set for the agents that work in it. You are in a checkout of the repository as merged. You may read the code (Read, Grep, Glob); change nothing.

You get the topic, its current page (or none yet) and new entries, each with an id. For each new entry:
- check its confirm_by, and the rule itself, against the code as it is now; drop it when it doesn't hold, saying why;
- when it repeats a rule already on the page, merge the two into that rule, citing every PR;
- mark an existing rule superseded (move it to Replaced, saying by what) only when a new entry clearly reverses it;
- otherwise, when they conflict, list the conflict under Open questions;
- when an entry or a rule contradicts one of the hand-written records you are given, drop it (its why names the record) or list it under Open questions citing the record's path: a record always wins, and you never change one;
- when a new entry belongs on one of the area's other topics you are given rather than this one, leave it off this page and list it under moved, with the topic it belongs on.

Write the whole page in Markdown, at most {PAGE_WORDS} words, in exactly this shape:
# <topic>
Scope: <what it covers, in under 15 words; no file paths>
## Rules in force
## Constraints
## Open questions
## Replaced

Each bullet is one rule of at most 30 words, then the PRs it came from, as (#123, #130), then an HTML comment with the ids of every entry it stands on: <!-- k123-1 k130-2 -->. Keep the id comment of every rule you keep. Name a file path in a rule only when the rule needs it. Rules in force: decisions, conventions, preferences, tooling, gotchas. Constraints: facts about the world that limit behaviour. A rule the agent decided, not the owner, ends with "(agent call)".

Your reply, after any reading, is one JSON object: {{"page": "<the whole page>", "dropped": [{{"id": "k…", "why": "…"}}], "moved": [{{"id": "k…", "to": "<area/topic>"}}]}}"#
    )
}

/// What both calls are told of the hand-written records.
pub(super) const RECORDS_RULE: &str = "The repo's hand-written decision records (in .orion/decisions/, outside harvested/) are authoritative and read-only. A record always wins: never record a rule that contradicts one; drop it, or list it under Open questions citing the record's path. Never restate a record as a rule.";

/// What the staleness pass adds: no new entries, every rule checked.
const STALE_TASK: &str = "There are no new entries. Check every rule and constraint on the page against the code as it is now. One that no longer holds moves to Replaced, saying why, its id comment kept; list its ids under dropped. Leave the rest as they are.";

/// What the check is told when there is no checkout to read.
pub(super) const UNCHECKED: &str = "This time the code can't be read and you have no tools. Don't check the new entries against the code and drop none of them for not holding: merge them into the page as the rules above say, and leave dropped empty.";

/// What both calls are told last, in their rules and at the end of the
/// prompt: the reply is parsed.
pub(super) const JSON_ONLY: &str = "Reply with the JSON object only: no prose, no code fence.";

/// One ask of the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelAsk {
    pub system: String,
    pub prompt: String,
    /// Where it runs; None runs it nowhere in particular.
    pub cwd: Option<PathBuf>,
    /// Read, Grep and Glob allowed — and nothing else.
    pub read_tools: bool,
    /// The step it is: which model and effort answer it
    /// (`knowledge.models`).
    pub step: Step,
}

/// The model a harvest asks: the Claude CLI, or a test's stand-in.
pub(crate) trait Model: Send + Sync {
    /// The model's reply text.
    fn ask(&self, ask: ModelAsk) -> BoxFuture<'_, Result<String, String>>;
}

/// The Claude CLI, in print mode, on one of the accounts orion runs, at
/// each step's model and effort.
pub(crate) struct ClaudeCli {
    pub account: Account,
    pub models: KnowledgeModels,
}

impl ClaudeCli {
    /// The factory's Claude account (`oneshot::factory_account`), when one
    /// is on and its CLI is installed — and never while `ORION_AGENT_CMD`
    /// stands in for the agents (orion's own tests).
    pub(crate) fn from_config() -> Result<Self, String> {
        if orion_core::env::non_empty(orion_core::env::AGENT_CMD).is_some() {
            return Err("off while ORION_AGENT_CMD stands in for the agents".into());
        }
        let cfg = crate::config::Config::load();
        let account = oneshot::factory_account(&cfg)
            .ok_or("no Claude account is switched on — Settings → Accounts")?;
        if !crate::config::program_installed(&account.program) {
            return Err(format!("{} is not on your PATH", account.program));
        }
        Ok(ClaudeCli {
            account,
            models: cfg.knowledge.models,
        })
    }

    /// `ask` as the CLI is asked it: on the account, at its step's model
    /// and effort.
    pub(crate) fn one_shot(&self, ask: &ModelAsk) -> oneshot::Ask {
        let choice = self.models.of(ask.step);
        oneshot::Ask {
            prompt: ask.prompt.clone(),
            system: ask.system.clone(),
            account: self.account.clone(),
            model: choice.model,
            effort: choice.effort,
            timeout: ASK_TIMEOUT,
        }
    }
}

impl Model for ClaudeCli {
    fn ask(&self, ask: ModelAsk) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            let tools = ask.read_tools.then_some(READ_TOOLS);
            let one = self.one_shot(&ask);
            Ok(oneshot::run_in(one, ask.cwd.as_deref(), tools).await?.text)
        })
    }
}

/// The merged pull request being harvested.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Pr {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
    pub branch: String,
    pub merged_at: String,
    /// The branch it merged into.
    pub base: String,
    /// The commit its merge made on the base; empty when GitHub didn't say.
    pub merge_commit: String,
}

/// Who wrote a comment on the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Commenter {
    Owner,
    Reviewer(String),
    Cubic,
}

/// One comment on the pull request: its thread, a review, or a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Comment {
    pub who: Commenter,
    pub body: String,
    /// `path:line` for a comment on a line.
    pub at: String,
}

/// One condensed session, under the heading the dossier gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Session {
    pub heading: String,
    pub lines: Vec<Line>,
}

/// What the extraction reads, and the owner's words the quote guard
/// holds it to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Dossier {
    pub text: String,
    pub owner_lines: Vec<String>,
}

/// The dossier of `pr`: its description, its thread, and each session.
/// Past [`DOSSIER_CHARS`] the agent's prose goes first, then the end.
pub(crate) fn dossier(pr: &Pr, comments: &[Comment], sessions: &[Session]) -> Dossier {
    let mut owner_lines = Vec::new();
    let mut head = format!("# PR #{}: {}\n{}", pr.number, pr.title, pr.url);
    if !pr.branch.is_empty() {
        head.push_str(&format!(" · branch `{}`", pr.branch));
    }
    if !pr.merged_at.is_empty() {
        head.push_str(&format!(" · merged {}", pr.merged_at));
    }
    head.push_str("\n\n## Description\n");
    head.push_str(&crate::ui::truncate(pr.body.trim(), BODY_CUT));
    head.push('\n');
    if !comments.is_empty() {
        head.push_str("\n## Review comments\n");
        for c in comments {
            let at = match c.at.as_str() {
                "" => String::new(),
                at => format!(" on {at}"),
            };
            let who = match &c.who {
                Commenter::Owner => {
                    owner_lines.push(c.body.clone());
                    format!("OWNER (PR comment{at})")
                }
                Commenter::Reviewer(login) => format!("REVIEWER {login}{at}"),
                Commenter::Cubic => format!("CUBIC{at}"),
            };
            let body = crate::ui::truncate(c.body.trim(), COMMENT_CUT);
            head.push_str(&format!("{who}: {body}\n"));
        }
    }
    for s in sessions {
        owner_lines.extend(
            s.lines
                .iter()
                .filter(|l| l.who.is_owner())
                .map(|l| l.text.clone()),
        );
    }
    let render = |keep_agent: bool| {
        let mut text = head.clone();
        for s in sessions {
            text.push_str(&format!("\n## {}\n", s.heading));
            for line in &s.lines {
                if keep_agent || line.who != super::condense::Who::Agent {
                    text.push_str(&line.render());
                    text.push('\n');
                }
            }
        }
        text
    };
    let mut text = render(true);
    if text.chars().count() > DOSSIER_CHARS {
        text = render(false);
    }
    if text.chars().count() > DOSSIER_CHARS {
        text = text.chars().take(DOSSIER_CHARS).collect();
        text.push_str("\n[… the rest of the dossier was cut]\n");
    }
    Dossier { text, owner_lines }
}

/// The owner's corrections in `sessions`, for a factory run's record
/// (`factory::runs`, which the weekly retro reads): what he typed to the
/// agents in his own words — not his answers to a question or a plan,
/// and not a session's first message, which is its prompt. Each cut to
/// [`CORRECTION_CUT`] chars, the first [`CORRECTIONS_MAX`] of them.
pub(crate) fn corrections(sessions: &[Session]) -> Vec<String> {
    sessions
        .iter()
        .flat_map(|s| {
            s.lines
                .iter()
                .filter(|l| l.who == super::condense::Who::Owner)
                .skip(1)
        })
        .map(|l| crate::ui::truncate(l.text.trim(), CORRECTION_CUT))
        .filter(|text| !text.is_empty())
        .take(CORRECTIONS_MAX)
        .collect()
}

/// What one extraction found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Extracted {
    pub entries: Vec<Entry>,
    pub skipped: Vec<String>,
}

/// The records as a prompt gives them.
pub(super) fn records_part(records: &str) -> String {
    match records.trim() {
        "" => "The repo's hand-written decision records: (none)".into(),
        records => {
            format!("The repo's hand-written decision records (read-only; they win):\n{records}")
        }
    }
}

/// The areas and the topics there are, as the extraction picks from
/// them: each topic `area/topic — scope`.
pub(crate) fn catalog(areas: &Areas, topics: &[(String, String)]) -> String {
    let topics = match topics.is_empty() {
        true => "(none yet)".to_string(),
        false => topics
            .iter()
            .map(|(topic, scope)| format!("- {topic} — {scope}"))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    format!(
        "The repo's knowledge areas (a topic's area is one of these):\n{}\n\nThe repo's topics (put each entry in one of these unless none fits):\n{topics}",
        areas.listing()
    )
}

/// Ask the model for `dossier`'s entries, with the store's `catalog`
/// ([`catalog`]) to pick topics from and the hand-written `records` to
/// defer to.
pub(crate) async fn extract(
    model: &dyn Model,
    dossier: &Dossier,
    catalog: &str,
    records: &str,
) -> Result<Extracted, String> {
    let prompt = format!(
        "{catalog}\n\n{}\n\nThe dossier:\n\n{}\n\n{JSON_ONLY}",
        records_part(records),
        dossier.text
    );
    let ask = ModelAsk {
        system: format!("{EXTRACT_SYSTEM}\n\n{RECORDS_RULE}\n\n{JSON_ONLY}"),
        prompt,
        cwd: None,
        read_tools: false,
        step: Step::Harvest,
    };
    let v = ask_json(model, ask, |v| {
        v.get("entries").is_some_and(Value::is_array) || v.get("considered_and_skipped").is_some()
    })
    .await?;
    let entries = v
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Entry::from_model)
        .collect();
    let skipped = v
        .get("considered_and_skipped")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect();
    Ok(Extracted { entries, skipped })
}

/// Ask `model`, and read its reply as a JSON object that `fits`
/// ([`oneshot::parse_json_where`]). One that doesn't is asked once more,
/// told so; only then is it a failure (the PR's harvest fails).
pub(super) async fn ask_json(
    model: &dyn Model,
    ask: ModelAsk,
    fits: fn(&Value) -> bool,
) -> Result<Value, String> {
    let why = match oneshot::parse_json_where(&model.ask(ask.clone()).await?, fits) {
        Ok(v) => return Ok(v),
        Err(why) => why,
    };
    tracing::info!("knowledge: asking again, {why}");
    let again = ModelAsk {
        prompt: oneshot::asked_again(&ask.prompt, oneshot::NOT_JSON_OBJECT),
        ..ask
    };
    oneshot::parse_json_where(&model.ask(again).await?, fits)
}

/// What a check-and-merge returned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Checked {
    pub page: String,
    /// `(id, why)` of each entry or rule that doesn't hold.
    pub dropped: Vec<(String, String)>,
    /// `(id, topic)` of each new entry that belongs on another topic.
    pub moved: Vec<(String, String)>,
}

/// One topic as a check-and-merge is given it.
pub(crate) struct Topic<'a> {
    pub name: &'a str,
    /// Its page as it stands; None while it has none.
    pub page: Option<&'a str>,
    /// The other topics of its area, and their scopes: where a new entry
    /// may be moved to.
    pub others: &'a [(String, String)],
}

/// Check `new` against the code at `code` (a [`CheckTree`]) and merge
/// them into `topic`'s page — or, with `new` empty, check the page's
/// rules alone (the staleness pass). With no code to read (`Err`), `new`
/// is merged unchecked, and nothing is dropped.
pub(crate) async fn check_topic(
    model: &dyn Model,
    records: &str,
    topic: &Topic<'_>,
    new: &[Entry],
    code: Result<&Path, &str>,
) -> Result<Checked, String> {
    let task = match new.is_empty() {
        true => STALE_TASK.to_string(),
        false => {
            let list: Vec<Value> = new
                .iter()
                .map(|e| {
                    let mut v = json!({"id": e.id, "pr": e.pr, "stream": e.stream, "kind": e.kind,
                        "rule": e.rule, "applies_to": e.applies_to, "why": e.why, "by": e.by,
                        "confidence": e.confidence, "quote": e.quote,
                        "options_rejected": e.options_rejected, "confirm_by": e.confirm_by});
                    if !e.new_topic.is_empty() {
                        v["new_topic"] = json!(e.new_topic);
                    }
                    v
                })
                .collect();
            format!(
                "New entries:\n{}",
                serde_json::to_string_pretty(&list).unwrap_or_default()
            )
        }
    };
    let others = match (new.is_empty(), topic.others.is_empty()) {
        (false, false) => format!(
            "The area's other topics (move a new entry there only when it belongs there, not here):\n{}\n\n",
            topic
                .others
                .iter()
                .map(|(t, scope)| format!("- {t} — {scope}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        _ => String::new(),
    };
    let scope_hint = new.iter().find(|e| !e.new_topic.is_empty());
    let prompt = format!(
        "Topic: {}\n\n{}\n\n{others}Current page:\n{}\n\n{task}\n\n{JSON_ONLY}",
        topic.name,
        records_part(records),
        topic.page.unwrap_or("(no page yet)"),
    );
    let step = match new.is_empty() {
        true => Step::Staleness,
        false => Step::Harvest,
    };
    let ask = match code {
        Ok(dir) => ModelAsk {
            system: format!("{}\n\n{RECORDS_RULE}\n\n{JSON_ONLY}", check_system()),
            prompt,
            cwd: Some(dir.to_path_buf()),
            read_tools: true,
            step,
        },
        Err(_) => ModelAsk {
            system: format!(
                "{}\n\n{RECORDS_RULE}\n\n{UNCHECKED}\n\n{JSON_ONLY}",
                check_system()
            ),
            prompt,
            cwd: None,
            read_tools: false,
            step,
        },
    };
    let hint = match topic.page {
        None => scope_hint.map(|e| e.new_topic.as_str()),
        Some(_) => None,
    };
    let mut checked = ask_page(model, ask, topic.name, hint).await?;
    // Unchecked, nothing was found not to hold.
    if code.is_err() {
        checked.dropped.clear();
    }
    Ok(checked)
}

/// Ask for a topic's page ([`check_system`]'s reply), and read it: every
/// section there, its title the topic and its scope line short
/// ([`store::tidy_page`]) — `scope` when given.
pub(crate) async fn ask_page(
    model: &dyn Model,
    ask: ModelAsk,
    topic: &str,
    scope: Option<&str>,
) -> Result<Checked, String> {
    let v = ask_json(model, ask, |v| v.get("page").is_some_and(Value::is_string)).await?;
    let mut page = v
        .get("page")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if !page.contains("## Rules in force") {
        return Err(format!("the check of {topic} returned no page"));
    }
    // Every page has every section, an empty one included.
    for section in SECTIONS {
        if !page.lines().any(|l| l.trim() == format!("## {section}")) {
            page.push_str(&format!("\n\n## {section}"));
        }
    }
    let page = store::tidy_page(topic, &page, scope);
    let words = page.split_whitespace().count();
    if words > PAGE_WORDS + PAGE_SLACK {
        tracing::warn!("knowledge: {topic}'s page runs {words} words");
    }
    let pairs = |key: &str, field: &str| -> Vec<(String, String)> {
        v.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|d| {
                let id = d.get("id")?.as_str()?.to_string();
                let other = d.get(field).and_then(Value::as_str).unwrap_or_default();
                Some((id, other.to_string()))
            })
            .collect()
    };
    Ok(Checked {
        page,
        dropped: pairs("dropped", "why"),
        moved: pairs("moved", "to"),
    })
}

/// What one harvest did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub pr: u64,
    /// Entries recorded and in force.
    pub kept: usize,
    /// Dropped by the guards or the check.
    pub dropped: usize,
    pub topics: Vec<String>,
}

/// Where a harvest writes: the knowledge branch's checkout, this
/// machine's folder for the dossiers, and what it reads besides.
pub(crate) struct Target<'a> {
    /// The harvested pages, in the working clone.
    pub store: &'a Store,
    pub local: &'a Local,
    pub project: &'a str,
    /// The hand-written records, as the calls read them
    /// (`super::records::for_check`).
    pub records: &'a str,
}

impl Target<'_> {
    /// Whether pull request `pr` was harvested: here, or by anyone whose
    /// harvest the branch has.
    pub(crate) fn harvested(&self, pr: u64) -> bool {
        self.local.has_source(pr) || self.store.entries().iter().any(|e| e.pr == pr)
    }
}

/// Harvest the merged `pr` of the repo at `root` into `to`: its dossier
/// from `comments` and `sessions`, checked against a checkout of the
/// merge ([`CheckTree::of_merge`], removed after). The outcome, and why
/// the entries went unchecked when there was no checkout to read.
pub(crate) async fn harvest_merge(
    model: &dyn Model,
    to: &Target<'_>,
    root: &Path,
    pr: &Pr,
    comments: &[Comment],
    sessions: &[Session],
) -> (Result<Outcome, String>, Option<String>) {
    let dossier = dossier(pr, comments, sessions);
    let tree = CheckTree::of_merge(root, pr).await;
    let outcome = harvest(model, to, pr, &dossier, code_at(&tree)).await;
    match tree {
        Ok(tree) => {
            tree.remove().await;
            (outcome, None)
        }
        Err(why) => (outcome, Some(why)),
    }
}

/// `topics`' topics in `topic`'s area but `topic` itself.
fn area_others(topics: &[(String, String)], topic: &str) -> Vec<(String, String)> {
    let area = topic.split('/').next().unwrap_or_default();
    topics
        .iter()
        .filter(|(t, _)| t != topic && t.split('/').next() == Some(area))
        .cloned()
        .collect()
}

/// The topic `e` goes on, its area made a listed one ([`Areas::topic`]).
/// A topic there's no page for, that the model didn't say what it covers,
/// goes on the page in its area whose name shares a word with it, when
/// one does.
fn place(areas: &Areas, topics: &[(String, String)], e: &Entry) -> String {
    let topic = areas.topic(&e.topic, &e.stream);
    if !e.new_topic.is_empty() || topics.iter().any(|(t, _)| *t == topic) {
        return topic;
    }
    let (area, name) = topic.split_once('/').unwrap_or(("", &topic));
    let words: Vec<&str> = name.split('-').filter(|w| w.len() >= 3).collect();
    topics
        .iter()
        .map(|(t, _)| t)
        .find(|t| {
            t.split_once('/')
                .is_some_and(|(a, n)| a == area && n.split('-').any(|w| words.contains(&w)))
        })
        .cloned()
        .unwrap_or(topic)
}

/// Harvest `pr` from `dossier` into `to`, the entries checked against
/// the code at `code` ([`CheckTree::of_merge`]) — or, with none (`Err`,
/// why), kept unchecked at low confidence with a note saying so. Each
/// entry goes on a listed topic unless none fits ([`place`]), and the
/// check may move it to another of its area's. Nothing is written unless
/// every step worked; nothing is committed (the caller does,
/// `super::share`).
pub(crate) async fn harvest(
    model: &dyn Model,
    to: &Target<'_>,
    pr: &Pr,
    dossier: &Dossier,
    code: Result<&Path, &str>,
) -> Result<Outcome, String> {
    let store = to.store;
    let areas = Areas::load(store);
    let topics = store.topic_scopes();
    let extracted = extract(model, dossier, &catalog(&areas, &topics), to.records).await?;
    let guarded = guards::apply(extracted.entries, &dossier.owner_lines);
    let mut dropped = guarded.dropped.len();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let first = store.entries().iter().filter(|e| e.pr == pr.number).count() + 1;
    let mut entries: Vec<Entry> = guarded
        .kept
        .into_iter()
        .enumerate()
        .map(|(n, mut e)| {
            e.id = format!("k{}-{}", pr.number, first + n);
            e.pr = pr.number;
            e.pr_url = pr.url.clone();
            e.recorded_at = now.clone();
            e.status = ACTIVE.into();
            e.topic = place(&areas, &topics, &e);
            if let Err(why) = code {
                e.confidence = "low".into();
                e.note = format!("not checked against the code: {why}");
            }
            e
        })
        .collect();

    // Where an entry may be moved: a topic with a page, or one this
    // harvest makes.
    let mut targets = topics.clone();
    let mut queue: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for e in &entries {
        queue.entry(e.topic.clone()).or_default().push(e.clone());
        if !targets.iter().any(|(t, _)| *t == e.topic) {
            let scope = store::short_scope(&e.new_topic, &e.topic);
            targets.push((e.topic.clone(), scope));
        }
    }
    let mut pages: BTreeMap<String, String> = BTreeMap::new();
    let mut moved_once: Vec<String> = Vec::new();
    while let Some((topic, new)) = queue.pop_first() {
        let page = pages.get(&topic).cloned().or_else(|| store.page(&topic));
        let others = area_others(&targets, &topic);
        let at = Topic {
            name: &topic,
            page: page.as_deref(),
            others: &others,
        };
        let checked = check_topic(model, to.records, &at, &new, code).await?;
        let mut page_text = checked.page;
        let mut kept_here = new.len();
        for (id, to_topic) in &checked.moved {
            let to_topic = areas.topic(to_topic, "");
            let Some(e) = new.iter().find(|e| &e.id == id) else {
                continue;
            };
            let movable = to_topic != topic
                && !moved_once.contains(id)
                && others.iter().any(|(t, _)| *t == to_topic);
            if !movable {
                continue;
            }
            moved_once.push(id.clone());
            page_text = store::without_ids(&page_text, &[id.as_str()]);
            kept_here -= 1;
            let mut e = e.clone();
            e.topic = to_topic.clone();
            e.new_topic.clear();
            if let Some(entry) = entries.iter_mut().find(|x| &x.id == id) {
                entry.topic = to_topic.clone();
            }
            queue.entry(to_topic).or_default().push(e);
        }
        for (id, why) in &checked.dropped {
            if let Some(e) = entries.iter_mut().find(|e| &e.id == id) {
                e.status = DROPPED.into();
                e.note = why.clone();
            }
        }
        // A topic made for entries that all went elsewhere isn't made.
        if page.is_some() || kept_here > 0 {
            pages.insert(topic, page_text);
        }
    }
    dropped += entries.iter().filter(|e| e.status == DROPPED).count();

    for (topic, page) in &pages {
        store.write_page(topic, page)?;
    }
    let mut source = dossier.text.clone();
    if !extracted.skipped.is_empty() {
        source.push_str("\n## Considered and skipped\n");
        for s in &extracted.skipped {
            source.push_str(&format!("- {s}\n"));
        }
    }
    store.append(&entries)?;
    areas.save(store)?;
    store.write_index(to.project)?;
    to.local.write_source(pr.number, &source)?;
    Ok(Outcome {
        pr: pr.number,
        kept: entries.iter().filter(|e| e.status == ACTIVE).count(),
        dropped,
        topics: pages.into_keys().collect(),
    })
}

/// The staleness pass: every topic's rules checked against the code at
/// `code` (the base as origin has it, [`CheckTree::of_base`]), the ones
/// that no longer hold moved to Replaced. Not committed (the caller
/// does). The topics changed.
pub(crate) async fn staleness(
    model: &dyn Model,
    to: &Target<'_>,
    code: &Path,
) -> Result<Vec<String>, String> {
    let store = to.store;
    let mut changed = Vec::new();
    let mut entries = store.entries();
    for topic in store.topics() {
        let page = store.page(&topic);
        let at = Topic {
            name: &topic,
            page: page.as_deref(),
            others: &[],
        };
        let checked = match check_topic(model, to.records, &at, &[], Ok(code)).await {
            Ok(checked) => checked,
            Err(why) => {
                tracing::warn!("knowledge: staleness check of {topic} failed: {why}");
                continue;
            }
        };
        for (id, why) in &checked.dropped {
            if let Some(e) = entries.iter_mut().find(|e| &e.id == id) {
                e.status = DROPPED.into();
                e.note = format!("no longer holds: {why}");
            }
        }
        if page.as_deref() != Some(checked.page.as_str()) {
            store.write_page(&topic, &checked.page)?;
            changed.push(topic);
        }
    }
    store.save_entries(&entries)?;
    store.write_index(to.project)?;
    Ok(changed)
}

// ---- the checkout a check reads ----

/// How long the fetch before a check may take.
const CHECK_FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// A temporary detached worktree of the code a check reads — what a pull
/// request merged, or the base as origin has it — removed with
/// [`CheckTree::remove`] once the check is done, whether or not it
/// worked. The main checkout may not have the merge yet.
///
/// It lives under the system temp dir (`orion-knowledge-check/<project>/
/// <pr>`), not orion's data dir: the daemon leaves a project's linked
/// checkouts in temp dirs out of the grid, and would otherwise adopt this
/// one as a worktree row for as long as the check runs.
#[derive(Debug)]
pub(crate) struct CheckTree {
    repo: PathBuf,
    pub path: PathBuf,
}

impl CheckTree {
    /// The code `pr` merged in the repo at `repo`: its base fetched from
    /// origin, and its merge commit — else `origin/<base>` — checked out.
    pub(crate) async fn of_merge(repo: &Path, pr: &Pr) -> Result<CheckTree, String> {
        let repo = repo.to_path_buf();
        let pr = pr.clone();
        tokio::task::spawn_blocking(move || {
            let base = match pr.base.trim() {
                "" => default_base(&repo),
                base => base.to_string(),
            };
            let rev = match pr.merge_commit.trim() {
                "" => format!("origin/{base}"),
                commit => commit.to_string(),
            };
            CheckTree::make(&repo, &base, &rev, &pr.number.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// The base of the repo at `repo` as origin has it now: what the
    /// staleness pass reads.
    pub(crate) async fn of_base(repo: &Path) -> Result<CheckTree, String> {
        let repo = repo.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let base = default_base(&repo);
            CheckTree::make(&repo, &base, &format!("origin/{base}"), "staleness")
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// `git fetch origin <base>`, then `git worktree add --detach` of
    /// `rev` at the check's folder `leaf` — a leftover there from a run cut
    /// off removed first.
    fn make(repo: &Path, base: &str, rev: &str, leaf: &str) -> Result<CheckTree, String> {
        let quit = std::sync::atomic::AtomicBool::new(false);
        crate::git_proc::remote_git(
            repo,
            &["fetch", "--quiet", "origin", base],
            CHECK_FETCH_TIMEOUT,
            &quit,
        )
        .map_err(|why| format!("couldn't fetch origin/{base}: {why}"))?;
        let path = std::env::temp_dir()
            .join("orion-knowledge-check")
            .join(orion_core::Project::folder_name(repo))
            .join(leaf);
        if path.exists() {
            remove_tree(repo, &path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let at = path.to_string_lossy().into_owned();
        crate::git_proc::run(repo, &["worktree", "add", "--detach", &at, rev])
            .map_err(|why| format!("couldn't check out {rev}: {why}"))?;
        Ok(CheckTree {
            repo: repo.to_path_buf(),
            path,
        })
    }

    /// Remove the worktree, and git's record of it.
    pub(crate) async fn remove(self) {
        let _ = tokio::task::spawn_blocking(move || remove_tree(&self.repo, &self.path)).await;
    }
}

/// Where a [`CheckTree`] stands, as the check takes it: its folder, or
/// why there is none.
pub(crate) fn code_at(tree: &Result<CheckTree, String>) -> Result<&Path, &str> {
    match tree {
        Ok(tree) => Ok(tree.path.as_path()),
        Err(why) => Err(why.as_str()),
    }
}

/// `git worktree remove --force` of `path`, its folder deleted should git
/// not have, then `git worktree prune`.
fn remove_tree(repo: &Path, path: &Path) {
    let at = path.to_string_lossy().into_owned();
    if let Err(why) = crate::git_proc::run(repo, &["worktree", "remove", "--force", &at]) {
        tracing::info!("knowledge: removing the check's checkout {at}: {why}");
    }
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
    let _ = crate::git_proc::run(repo, &["worktree", "prune"]);
}

/// The repo's base branch: origin's default, else `main`.
pub(crate) fn default_base(repo: &Path) -> String {
    crate::git_proc::read(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .and_then(|head| head.trim().strip_prefix("origin/").map(str::to_string))
    .filter(|b| !b.is_empty())
    .unwrap_or_else(|| "main".into())
}

// ---- gathering, from GitHub ----

/// Pull request `number` of the repo at `root`, with its thread: the
/// conversation, the reviews and the comments on lines.
pub(crate) async fn gather_pr(
    gh: &dyn Gh,
    root: &Path,
    number: u64,
) -> Result<(Pr, Vec<Comment>), String> {
    let n = number.to_string();
    let view = gh
        .run(
            root,
            &[
                "pr",
                "view",
                &n,
                "--json",
                "number,url,title,body,headRefName,baseRefName,mergeCommit,mergedAt,comments,reviews",
            ],
        )
        .await
        .map_err(|why| format!("gh couldn't read PR #{number}: {why}"))?;
    let view: Value = serde_json::from_str(&view).map_err(|e| e.to_string())?;
    let path = format!("repos/{{owner}}/{{repo}}/pulls/{number}/comments");
    let lines = gh
        .run(root, &["api", &path, "--paginate", "--jq", ".[]"])
        .await
        .unwrap_or_default();
    // The gh user: who "OWNER" is on the thread.
    let me = gh.login().await.unwrap_or_default();
    Ok(read_pr(&view, &lines, &me))
}

/// `gh pr view`'s JSON and the line comments (one JSON object a line)
/// as a [`Pr`] and its thread, `me` being the owner.
pub(crate) fn read_pr(view: &Value, line_comments: &str, me: &str) -> (Pr, Vec<Comment>) {
    use crate::pull_request::str_at;
    let pr = Pr {
        number: crate::pull_request::u64_at(view, "number"),
        url: str_at(view, "url"),
        title: str_at(view, "title"),
        body: str_at(view, "body"),
        branch: str_at(view, "headRefName"),
        merged_at: str_at(view, "mergedAt"),
        base: str_at(view, "baseRefName"),
        merge_commit: view["mergeCommit"]["oid"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    };
    let who = |login: &str| -> Option<Commenter> {
        let lower = login.to_lowercase();
        if lower.contains("cubic") {
            Some(Commenter::Cubic)
        } else if !me.is_empty() && login == me {
            Some(Commenter::Owner)
        } else if lower.ends_with("[bot]") || lower.ends_with("-bot") || login.is_empty() {
            None
        } else {
            Some(Commenter::Reviewer(login.to_string()))
        }
    };
    let mut comments = Vec::new();
    for key in ["comments", "reviews"] {
        for c in view
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let body = str_at(c, "body");
            let login = c["author"]["login"].as_str().unwrap_or_default();
            if let (false, Some(who)) = (body.trim().is_empty(), who(login)) {
                comments.push(Comment {
                    who,
                    body,
                    at: String::new(),
                });
            }
        }
    }
    for line in line_comments.lines() {
        let Ok(c) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let body = str_at(&c, "body");
        let login = c["user"]["login"].as_str().unwrap_or_default();
        if let (false, Some(who)) = (body.trim().is_empty(), who(login)) {
            let line_no = c.get("line").and_then(Value::as_u64).unwrap_or(0);
            let at = match line_no {
                0 => str_at(&c, "path"),
                n => format!("{}:{n}", str_at(&c, "path")),
            };
            comments.push(Comment { who, body, at });
        }
    }
    (pr, comments)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::knowledge::condense::{self, Shape};
    use std::sync::Mutex;

    #[test]
    fn the_check_asks_for_a_page_within_page_words() {
        let system = check_system();
        assert!(system.contains(&format!("at most {PAGE_WORDS} words")));
        assert!(system.contains(r#"{"page": "<the whole page>", "dropped": [{"id""#));
    }

    #[test]
    fn each_step_asks_at_the_model_its_setting_names() {
        let cfg: crate::config::Config = serde_json::from_value(json!({
            "knowledge": {"models": {
                "consolidate": {"model": "haiku"},
                "staleness": {"model": "sonnet", "effort": "medium"}
            }},
            "automations": {"intake_model": {"effort": "medium"}}
        }))
        .unwrap();
        let cli = ClaudeCli {
            account: Account::default(),
            models: cfg.knowledge.models.clone(),
        };
        let at = |step: Step| {
            let one = cli.one_shot(&ModelAsk {
                system: String::new(),
                prompt: String::new(),
                cwd: None,
                read_tools: false,
                step,
            });
            (one.model, one.effort)
        };
        assert_eq!(
            at(Step::Harvest),
            ("opus".into(), "high".into()),
            "unset: the default"
        );
        assert_eq!(
            at(Step::Consolidate),
            ("haiku".into(), "low".into()),
            "the model set, the effort its default"
        );
        assert_eq!(at(Step::Staleness), ("sonnet".into(), "medium".into()));
        let intake = cfg.automations.intake_model();
        assert_eq!(
            (intake.model.as_str(), intake.effort.as_str()),
            ("haiku", "medium")
        );

        let defaults = crate::config::Config::default();
        assert_eq!(
            defaults.knowledge.models.of(Step::Consolidate),
            oneshot::ModelChoice::new("sonnet", "low")
        );
        assert_eq!(
            defaults.automations.intake_model(),
            oneshot::ModelChoice::new("haiku", "low")
        );
    }

    /// A model that answers from a script, and remembers what it was asked.
    pub(crate) struct Scripted {
        pub replies: Mutex<Vec<String>>,
        pub asked: Mutex<Vec<ModelAsk>>,
    }

    impl Scripted {
        pub(crate) fn new(replies: Vec<String>) -> Self {
            Scripted {
                replies: Mutex::new(replies),
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    impl Model for Scripted {
        fn ask(&self, ask: ModelAsk) -> BoxFuture<'_, Result<String, String>> {
            self.asked.lock().unwrap().push(ask);
            let mut replies = self.replies.lock().unwrap();
            let reply = match replies.is_empty() {
                true => Err("no more replies".to_string()),
                false => Ok(replies.remove(0)),
            };
            Box::pin(async move { reply })
        }
    }

    pub(crate) fn pr() -> Pr {
        Pr {
            number: 12,
            url: "https://github.com/o/app/pull/12".into(),
            title: "Keep the return path on login".into(),
            body: "Login sends you back where you were.".into(),
            branch: "fix-login".into(),
            merged_at: "2026-11-10T10:00:00Z".into(),
            base: "main".into(),
            merge_commit: "abc123".into(),
        }
    }

    pub(crate) fn sessions() -> Vec<Session> {
        vec![Session {
            heading: "Session s1 (claude)".into(),
            lines: condense::condense(Shape::Claude, condense::tests::CLAUDE),
        }]
    }

    pub(crate) fn extraction() -> String {
        json!({"entries": [
            {"stream": "product", "kind": "decision", "topic": "auth/login",
             "rule": "Login returns users to the page they came from, never /home",
             "applies_to": ["src/auth"], "why": "Users lost their place", "by": "owner",
             "confidence": "high", "quote": "Never send users to /home after login, always back where they were.",
             "confirm_by": "src/auth.ts returnPath", "source_at": "session s1"},
            {"stream": "product", "kind": "decision", "topic": "auth/login",
             "rule": "Login keeps the query string", "by": "owner", "confidence": "high",
             "quote": "keep the query string, obviously", "confirm_by": "returnPath"},
            {"stream": "engineering", "kind": "convention", "topic": "git/hooks",
             "rule": "Use --no-verify when the hook is slow", "by": "owner", "quote": "use --no-verify"}
        ], "considered_and_skipped": ["the redirect refactor"]})
        .to_string()
    }

    pub(crate) fn checked_page() -> String {
        json!({"page": "# auth/login\nScope: src/auth\n\n## Rules in force\n- Login returns users to the page they came from (#12) <!-- k12-1 -->\n\n## Constraints\n\n## Open questions\n\n## Replaced\n",
               "dropped": [{"id": "k12-2", "why": "returnPath drops the query"}]})
        .to_string()
    }

    #[test]
    fn corrections_are_the_owners_own_words_after_each_prompt() {
        use condense::{Line, Who};
        let line = |who: Who, text: &str| Line {
            who,
            text: text.into(),
        };
        let long = "y".repeat(400);
        let first = Session {
            heading: "Session s1 (claude)".into(),
            lines: vec![
                line(Who::Owner, "Fix RIPLO-7: the login loop."),
                line(Who::AgentAsked, "Keep the query string?"),
                line(Who::OwnerAnswered("question"), "Keep it"),
                line(Who::AgentPlan, "1. Store the path"),
                line(Who::OwnerAnswered("plan approval"), "Approved"),
                line(Who::Owner, "No, keep the old redirect for admins."),
                line(Who::OwnerRejected, "rejected"),
                line(Who::Owner, &long),
            ],
        };
        let handover = Session {
            heading: "Session s2 (claude)".into(),
            lines: vec![
                line(Who::Owner, "You are taking over pull request #12."),
                line(Who::Owner, "Don't touch the migrations."),
            ],
        };
        let got = corrections(&[first, handover]);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], "No, keep the old redirect for admins.");
        assert_eq!(got[1].chars().count(), 300);
        assert!(got[1].ends_with('…'));
        assert_eq!(got[2], "Don't touch the migrations.");

        let chatty = Session {
            heading: "Session s3 (claude)".into(),
            lines: (0..30)
                .map(|n| line(Who::Owner, &format!("say {n}")))
                .collect(),
        };
        let got = corrections(&[chatty]);
        assert_eq!(got.len(), 20, "at most twenty");
        assert_eq!(got[0], "say 1", "the prompt isn't one");
        assert!(
            corrections(&sessions()).is_empty(),
            "a prompt and answers only"
        );
    }

    #[test]
    fn a_dossier_labels_the_thread_and_sessions() {
        let comments = vec![
            Comment {
                who: Commenter::Owner,
                body: "Keep the copy short.".into(),
                at: String::new(),
            },
            Comment {
                who: Commenter::Cubic,
                body: "P1: null check".into(),
                at: "src/a.ts:3".into(),
            },
            Comment {
                who: Commenter::Reviewer("ann".into()),
                body: "lgtm".into(),
                at: String::new(),
            },
        ];
        let d = dossier(&pr(), &comments, &sessions());
        assert!(d.text.starts_with("# PR #12: Keep the return path on login\nhttps://github.com/o/app/pull/12 · branch `fix-login`"));
        assert!(d.text.contains("OWNER (PR comment): Keep the copy short.\nCUBIC on src/a.ts:3: P1: null check\nREVIEWER ann: lgtm\n"));
        assert!(d
            .text
            .contains("\n## Session s1 (claude)\nOWNER: Fix the login redirect."));
        assert_eq!(d.owner_lines[0], "Keep the copy short.");
        assert!(d
            .owner_lines
            .iter()
            .any(|l| l.starts_with("Fix the login redirect")));
    }

    #[test]
    fn an_oversized_dossier_drops_agent_prose_first() {
        let mut big = sessions();
        big[0].lines.push(Line {
            who: condense::Who::Agent,
            text: "y".repeat(DOSSIER_CHARS),
        });
        let d = dossier(&pr(), &[], &big);
        assert!(d.text.contains("OWNER: Fix the login redirect"));
        assert!(!d.text.contains("AGENT: I'll look"));
        assert!(d.text.len() < DOSSIER_CHARS);
    }

    /// A checkout to harvest into, and a folder for the dossiers.
    pub(crate) struct Bench {
        pub repo: tempfile::TempDir,
        pub store: Store,
        pub local: Local,
    }

    impl Bench {
        pub(crate) fn new() -> Self {
            let repo = tempfile::tempdir().unwrap();
            let store = Store::of(repo.path());
            let local = Local::at("01P", &repo.path().join("local"));
            Bench { repo, store, local }
        }

        pub(crate) fn target<'a>(&'a self, records: &'a str) -> Target<'a> {
            Target {
                store: &self.store,
                local: &self.local,
                project: "app",
                records,
            }
        }
    }

    #[tokio::test]
    async fn a_harvest_guards_checks_and_writes() {
        let into = Bench::new();
        let (repo, store) = (&into.repo, &into.store);
        let model = Scripted::new(vec![extraction(), checked_page()]);
        let d = dossier(&pr(), &[], &sessions());
        let records =
            "### Sessions last 30 days (.orion/decisions/0001-sessions.md)\nDecided with legal.";
        let out = harvest(&model, &into.target(records), &pr(), &d, Ok(repo.path()))
            .await
            .unwrap();
        assert_eq!(
            out,
            Outcome {
                pr: 12,
                kept: 1,
                dropped: 2,
                topics: vec!["access-and-accounts/login".into()]
            }
        );

        let asked = model.asked.lock().unwrap();
        assert_eq!(
            asked.len(),
            2,
            "one extraction, one check: git/hooks was dropped by the guard"
        );
        assert!(asked[0]
            .system
            .starts_with("You are reading one merged pull request's dossier"));
        assert!(!asked[0].read_tools && asked[0].cwd.is_none());
        assert!(asked[1].read_tools);
        assert_eq!(asked[1].cwd.as_deref(), Some(repo.path()));
        assert!(asked[1].prompt.contains("\"id\": \"k12-1\""));

        let entries = store.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            (
                entries[0].id.as_str(),
                entries[0].by.as_str(),
                entries[0].status.as_str()
            ),
            ("k12-1", "owner", ACTIVE)
        );
        // Not his words: downgraded, then dropped by the check.
        assert_eq!(
            (entries[1].by.as_str(), entries[1].status.as_str()),
            ("agent", DROPPED)
        );
        assert!(entries[1].downgraded);
        let page = store.page("access-and-accounts/login").unwrap();
        assert!(page.contains("<!-- k12-1 -->"));
        assert!(
            page.starts_with("# access-and-accounts/login\nScope: login\n"),
            "the area is a listed one, the scope names no path: {page}"
        );
        assert!(store.dir.join("areas.md").exists());
        assert!(store
            .index()
            .unwrap()
            .contains("- access-and-accounts: login — 1 rule"));
        let source = std::fs::read_to_string(into.local.source_path(12)).unwrap();
        assert!(source.contains("## Considered and skipped\n- the redirect refactor"));
        assert!(
            !store.dir.join("sources").exists(),
            "no dossier in the repo"
        );
        assert!(into.target("").harvested(12) && !into.target("").harvested(13));
        // The records are context to both calls, and win.
        for ask in asked.iter() {
            assert!(ask.prompt.contains(records), "{}", ask.prompt);
            assert!(ask.system.contains(RECORDS_RULE));
        }
    }

    #[tokio::test]
    async fn entries_go_on_the_topics_there_are_unless_none_fits() {
        let into = Bench::new();
        let store = &into.store;
        store
            .write_page(
                "editor/text-boxes",
                "# editor/text-boxes\nScope: text boxes, `src/editor/TextBox.tsx` and fonts\n\n## Rules in force\n- Text grows the box (#3) <!-- k3-1 -->\n",
            )
            .unwrap();
        store
            .write_page(
                "editor/tables",
                "# editor/tables\nScope: tables\n\n## Rules in force\n- Rows keep height (#4) <!-- k4-1 -->\n",
            )
            .unwrap();
        let extraction = json!({"entries": [
            // A new name, with nothing said of it: put on the page it shares a word with.
            {"stream": "product", "kind": "decision", "topic": "editor/text",
             "rule": "Text never shrinks below 8pt", "by": "agent"},
            // A new topic that says what it covers; the check moves it.
            {"stream": "product", "kind": "decision", "topic": "editor/fonts",
             "new_topic": "Fonts on text boxes", "rule": "Fonts fall back to Inter", "by": "agent"},
            // An area that isn't one: moved to the nearest listed.
            {"stream": "engineering", "kind": "gotcha", "topic": "e2e/local-runs",
             "new_topic": "Running e2e locally", "rule": "Probe ws before a local e2e run", "by": "agent"}
        ]})
        .to_string();
        let fonts = json!({"page": "# editor/fonts\nScope: fonts\n\n## Rules in force\n- Fonts fall back to Inter (#12) <!-- k12-2 -->\n",
            "moved": [{"id": "k12-2", "to": "editor/text-boxes"}]})
        .to_string();
        let text_boxes = json!({"page": "# editor/text-boxes\nScope: text boxes and fonts\n\n## Rules in force\n- Text grows the box (#3) <!-- k3-1 -->\n- Text never shrinks below 8pt (#12) <!-- k12-1 -->\n- Fonts fall back to Inter (#12) <!-- k12-2 -->\n"}).to_string();
        let e2e = json!({"page": "# testing-and-ci/local-runs\nScope: Running e2e locally\n\n## Rules in force\n- Probe ws before a local e2e run (#12) (agent call) <!-- k12-3 -->\n"}).to_string();
        let model = Scripted::new(vec![extraction, fonts, text_boxes, e2e]);
        let d = dossier(&pr(), &[], &sessions());
        let out = harvest(&model, &into.target(""), &pr(), &d, Ok(into.repo.path()))
            .await
            .unwrap();
        assert_eq!(
            out.topics,
            vec!["editor/text-boxes", "testing-and-ci/local-runs"],
            "no page for fonts: its one entry moved"
        );

        let asked = model.asked.lock().unwrap();
        // The extraction is given the areas and the topics there are.
        let extract = &asked[0];
        assert!(extract
            .system
            .contains("Put each entry in one of the repo's listed topics unless none fits"));
        assert!(extract.prompt.contains("The repo's knowledge areas (a topic's area is one of these):\n- editor — The slide editor"));
        assert!(extract.prompt.contains("The repo's topics (put each entry in one of these unless none fits):\n- editor/tables — tables\n- editor/text-boxes — text boxes, and fonts\n"), "{}", extract.prompt);
        // The check is given the area's other topics, and may move an entry.
        assert!(asked[1].prompt.starts_with("Topic: editor/fonts\n"));
        assert!(asked[1].prompt.contains("The area's other topics (move a new entry there only when it belongs there, not here):\n- editor/tables — tables\n- editor/text-boxes — text boxes, and fonts\n"), "{}", asked[1].prompt);
        assert!(asked[1]
            .prompt
            .contains("\"new_topic\": \"Fonts on text boxes\""));
        // Moved onto a topic still to be checked: checked there with its own.
        assert!(asked[2].prompt.starts_with("Topic: editor/text-boxes\n"));
        assert!(asked[2].prompt.contains("\"id\": \"k12-1\""));
        assert!(asked[2].prompt.contains("\"id\": \"k12-2\""));
        assert!(asked[3]
            .prompt
            .starts_with("Topic: testing-and-ci/local-runs\n"));
        assert_eq!(asked.len(), 4);

        let topics: Vec<(String, String)> = store
            .entries()
            .into_iter()
            .map(|e| (e.id, e.topic))
            .collect();
        assert_eq!(
            topics,
            vec![
                ("k12-1".into(), "editor/text-boxes".into()),
                ("k12-2".into(), "editor/text-boxes".into()),
                ("k12-3".into(), "testing-and-ci/local-runs".into()),
            ]
        );
        assert_eq!(store.page("editor/fonts"), None);
        assert!(store
            .page("editor/text-boxes")
            .unwrap()
            .contains("<!-- k12-2 -->"));
        assert_eq!(
            store.topics(),
            vec![
                "editor/tables",
                "editor/text-boxes",
                "testing-and-ci/local-runs"
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_check_writes_nothing() {
        let into = Bench::new();
        let model = Scripted::new(vec![extraction(), r#"{"page": "nonsense"}"#.into()]);
        let d = dossier(&pr(), &[], &sessions());
        assert!(
            harvest(&model, &into.target(""), &pr(), &d, Ok(into.repo.path()))
                .await
                .is_err()
        );
        assert!(into.store.entries().is_empty());
        assert!(!into.target("").harvested(12));
    }

    #[tokio::test]
    async fn a_reply_that_isnt_json_is_asked_for_once_more() {
        let into = Bench::new();
        let model = Scripted::new(vec![
            "I read the dossier and found three rules.".into(),
            extraction(),
            format!(
                "Checked against the code:\n```json\n{}\n```\nAll done.",
                checked_page()
            ),
        ]);
        let d = dossier(&pr(), &[], &sessions());
        let out = harvest(&model, &into.target(""), &pr(), &d, Ok(into.repo.path()))
            .await
            .unwrap();
        assert_eq!((out.kept, out.dropped), (1, 2));
        let asked = model.asked.lock().unwrap();
        assert_eq!(asked.len(), 3, "the extraction twice, the check once");
        assert!(asked[0].prompt.ends_with(JSON_ONLY));
        assert_eq!(
            asked[1].prompt,
            oneshot::asked_again(&asked[0].prompt, oneshot::NOT_JSON_OBJECT)
        );
        assert_eq!(asked[1].system, asked[0].system);
        assert!(
            asked[2].prompt.ends_with(JSON_ONLY),
            "the check read first time"
        );
    }

    #[tokio::test]
    async fn a_reply_that_isnt_json_twice_fails_the_harvest() {
        let into = Bench::new();
        let model = Scripted::new(vec![
            extraction(),
            "Sorry, I couldn't read the code.".into(),
            r#"{"dropped": []}"#.into(),
        ]);
        let d = dossier(&pr(), &[], &sessions());
        let why = harvest(&model, &into.target(""), &pr(), &d, Ok(into.repo.path()))
            .await
            .unwrap_err();
        assert_eq!(
            why,
            "the model's JSON was not the shape asked for; it replied: {\"dropped\": []}"
        );
        assert_eq!(
            model.asked.lock().unwrap().len(),
            3,
            "asked again once, no more"
        );
        assert!(into.store.entries().is_empty());
        assert!(!into.target("").harvested(12));
    }

    #[tokio::test]
    async fn with_no_checkout_the_entries_are_kept_unchecked_at_low_confidence() {
        let into = Bench::new();
        let store = &into.store;
        let model = Scripted::new(vec![extraction(), checked_page()]);
        let d = dossier(&pr(), &[], &sessions());
        let out = harvest(&model, &into.target(""), &pr(), &d, Err("no origin"))
            .await
            .unwrap();
        assert_eq!(
            (out.kept, out.dropped),
            (2, 1),
            "only the guard dropped one"
        );
        let asked = model.asked.lock().unwrap();
        assert!(!asked[1].read_tools && asked[1].cwd.is_none());
        assert!(asked[1]
            .system
            .ends_with(&format!("{UNCHECKED}\n\n{JSON_ONLY}")));
        let entries = store.entries();
        assert!(entries
            .iter()
            .all(|e| e.status == ACTIVE && e.confidence == "low"));
        assert_eq!(entries[0].note, "not checked against the code: no origin");
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A clone of a repo whose merge, `number`, landed on origin after the
    /// clone was pulled: the clone, and the merged pull request.
    fn merged_after_clone(tmp: &Path, number: u64) -> (PathBuf, Pr) {
        let origin = tmp.join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        std::fs::write(origin.join("a.txt"), "old").unwrap();
        git(&origin, &["add", "."]);
        let commit = |args: &[&str]| {
            let mut all = vec!["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q"];
            all.extend_from_slice(args);
            git(&origin, &all);
        };
        commit(&["-m", "one"]);
        let clone = tmp.join("clone");
        git(tmp, &["clone", "-q", &origin.to_string_lossy(), "clone"]);
        // The merge lands on origin after the main checkout was pulled.
        std::fs::write(origin.join("a.txt"), "merged").unwrap();
        commit(&["-am", "two"]);
        let merge = git(&origin, &["rev-parse", "HEAD"]).trim().to_string();
        let merged = Pr {
            number,
            base: "main".into(),
            merge_commit: merge,
            ..pr()
        };
        (clone, merged)
    }

    #[tokio::test]
    async fn the_check_reads_a_checkout_of_the_merge_removed_after() {
        let tmp = tempfile::tempdir().unwrap();
        let (clone, merged) = merged_after_clone(tmp.path(), 4242);
        let tree = CheckTree::of_merge(&clone, &merged).await.unwrap();
        let path = tree.path.clone();
        assert_eq!(
            std::fs::read_to_string(path.join("a.txt")).unwrap(),
            "merged"
        );
        assert_eq!(std::fs::read_to_string(clone.join("a.txt")).unwrap(), "old");
        tree.remove().await;
        assert!(!path.exists());
        let list = git(&clone, &["worktree", "list", "--porcelain"]);
        assert_eq!(list.matches("worktree ").count(), 1, "{list}");

        let gone = Pr {
            merge_commit: "0000000000000000000000000000000000000000".into(),
            ..merged
        };
        assert!(CheckTree::of_merge(&clone, &gone).await.is_err());
    }

    #[tokio::test]
    async fn a_merge_is_harvested_against_its_checkout_or_said_unchecked() {
        let tmp = tempfile::tempdir().unwrap();
        let (clone, merged) = merged_after_clone(tmp.path(), 4343);
        let into = Bench::new();
        let model = Scripted::new(vec![extraction(), checked_page()]);
        let (out, unchecked) =
            harvest_merge(&model, &into.target(""), &clone, &merged, &[], &sessions()).await;
        assert_eq!(out.unwrap().pr, 4343);
        assert_eq!(unchecked, None);
        let asked = std::mem::take(&mut *model.asked.lock().unwrap());
        assert!(asked[0].prompt.contains("Keep the return path on login"));
        let cwd = asked[1].cwd.clone().expect("the check ran in the checkout");
        assert!(asked[1].read_tools && cwd.ends_with("4343"));
        assert!(!cwd.exists(), "the checkout is removed after");

        // Nothing to fetch from: harvested all the same, saying why.
        let bare = tempfile::tempdir().unwrap();
        let into = Bench::new();
        let model = Scripted::new(vec![extraction(), checked_page()]);
        let (out, unchecked) = harvest_merge(
            &model,
            &into.target(""),
            bare.path(),
            &pr(),
            &[],
            &sessions(),
        )
        .await;
        assert_eq!(out.unwrap().kept, 2);
        assert!(unchecked.is_some_and(|why| why.contains("couldn't fetch")));
        assert!(model.asked.lock().unwrap()[1].cwd.is_none());
    }

    #[test]
    fn the_thread_is_told_apart_by_author() {
        let view = json!({"number": 12, "url": "u", "title": "t", "body": "b", "headRefName": "fix",
            "comments": [{"author": {"login": "me"}, "body": "Keep it short."},
                         {"author": {"login": "github-actions[bot]"}, "body": "deployed"}],
            "reviews": [{"author": {"login": "ann"}, "body": ""}, {"author": {"login": "ann"}, "body": "nice"}]});
        let lines = format!(
            "{}\n{}\n",
            json!({"user": {"login": "cubic-dev-ai[bot]"}, "body": "P1 bug", "path": "a.rs", "line": 4}),
            json!({"user": {"login": "me"}, "body": "rename this", "path": "b.rs"})
        );
        let (pr, comments) = read_pr(&view, &lines, "me");
        assert_eq!((pr.number, pr.branch.as_str()), (12, "fix"));
        let said: Vec<(Commenter, &str, &str)> = comments
            .iter()
            .map(|c| (c.who.clone(), c.body.as_str(), c.at.as_str()))
            .collect();
        assert_eq!(
            said,
            vec![
                (Commenter::Owner, "Keep it short.", ""),
                (Commenter::Reviewer("ann".into()), "nice", ""),
                (Commenter::Cubic, "P1 bug", "a.rs:4"),
                (Commenter::Owner, "rename this", "b.rs"),
            ]
        );
    }
}
