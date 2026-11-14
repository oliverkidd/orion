//! HARVEST: one merged pull request into the store. Its dossier (the PR's
//! description and thread, and the condensed sessions that built it) is
//! read by Opus for entries ([`extract`]); the guards hold those to what
//! the owner said ([`super::guards`]); each topic they touch is checked
//! against the code and merged into its page by a second Opus call that
//! may read a checkout of the merge ([`check_topic`], [`CheckTree`]): a
//! temporary detached worktree, since the main checkout may not have the
//! merge yet. With no such checkout the entries are merged unchecked, at
//! low confidence, rather than lost. Then the pages,
//! `entries.jsonl`, `index.md` and the dossier are written and committed
//! as `harvest #<pr>`.
//!
//! The model is a [`Model`]: the real one runs the Claude CLI
//! ([`ClaudeCli`]); a test hands in its own answers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::condense::Line;
use super::guards;
use super::store::{Entry, Store, ACTIVE, DROPPED, SECTIONS};
use crate::oneshot::{self, Account};

/// The model a harvest asks, and how hard it thinks.
pub(crate) const MODEL: &str = "opus";
pub(crate) const EFFORT: &str = "high";
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

Topic: area/topic from the repo's existing index if given, else a short kebab-case area/topic.
Each entry: stream, kind, topic, rule (≤30 words, true without the ticket), applies_to, why (≤60 words or "Not
recorded"), by, confidence, quote, options_rejected, confirm_by (a symbol, test, command or file to check it
still holds; never a line number), source_at.
Return JSON: {"entries":[...], "considered_and_skipped":["..."]}."#;

/// The check-and-merge call's rules, [`PAGE_WORDS`] said in them.
fn check_system() -> String {
    format!(
        r#"You keep one topic page of a repository's knowledge store: the rules its owner set for the agents that work in it. You are in a checkout of the repository as merged. You may read the code (Read, Grep, Glob); change nothing.

You get the topic, its current page (or none yet) and new entries, each with an id. For each new entry:
- check its confirm_by, and the rule itself, against the code as it is now; drop it when it doesn't hold, saying why;
- when it repeats a rule already on the page, merge the two into that rule, citing every PR;
- mark an existing rule superseded (move it to Replaced, saying by what) only when a new entry clearly reverses it;
- otherwise, when they conflict, list the conflict under Open questions.

Write the whole page in Markdown, at most {PAGE_WORDS} words, in exactly this shape:
# <topic>
Scope: <the paths or features it covers, on one line>
## Rules in force
## Constraints
## Open questions
## Replaced

Each bullet is one rule of at most 30 words, then the PRs it came from, as (#123, #130), then an HTML comment with the ids of every entry it stands on: <!-- k123-1 k130-2 -->. Keep the id comment of every rule you keep. Rules in force: decisions, conventions, preferences, tooling, gotchas. Constraints: facts about the world that limit behaviour. A rule the agent decided, not the owner, ends with "(agent call)".

Reply with one JSON value and nothing else: {{"page": "<the whole page>", "dropped": [{{"id": "k…", "why": "…"}}]}}"#
    )
}

/// What the staleness pass adds: no new entries, every rule checked.
const STALE_TASK: &str = "There are no new entries. Check every rule and constraint on the page against the code as it is now. One that no longer holds moves to Replaced, saying why, its id comment kept; list its ids under dropped. Leave the rest as they are.";

/// What the check is told when there is no checkout to read.
const UNCHECKED: &str = "This time the code can't be read and you have no tools. Don't check the new entries against the code and drop none of them for not holding: merge them into the page as the rules above say, and leave dropped empty.";

/// What the extraction is told on top of the spec.
const JSON_ONLY: &str = "Reply with one JSON value and nothing else: no prose, no code fences.";

/// One ask of the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelAsk {
    pub system: String,
    pub prompt: String,
    /// Where it runs; None runs it nowhere in particular.
    pub cwd: Option<PathBuf>,
    /// Read, Grep and Glob allowed — and nothing else.
    pub read_tools: bool,
}

/// The model a harvest asks: the Claude CLI, or a test's stand-in.
pub(crate) trait Model: Send + Sync {
    /// The model's reply text.
    fn ask(&self, ask: ModelAsk) -> BoxFuture<'_, Result<String, String>>;
}

/// The Claude CLI, in print mode, on one of the accounts orion runs.
pub(crate) struct ClaudeCli {
    pub account: Account,
}

impl ClaudeCli {
    /// The default agent's Claude account, when one is on and its CLI is
    /// installed — and never while `ORION_AGENT_CMD` stands in for the
    /// agents (orion's own tests).
    pub(crate) fn from_config() -> Result<Self, String> {
        if orion_core::env::non_empty(orion_core::env::AGENT_CMD).is_some() {
            return Err("off while ORION_AGENT_CMD stands in for the agents".into());
        }
        let cfg = crate::config::Config::load();
        let account = oneshot::default_account(&cfg)
            .ok_or("no Claude account is switched on — Settings → Accounts")?;
        if !crate::config::program_installed(&account.program) {
            return Err(format!("{} is not on your PATH", account.program));
        }
        Ok(ClaudeCli { account })
    }
}

impl Model for ClaudeCli {
    fn ask(&self, ask: ModelAsk) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            let tools = ask.read_tools.then_some(READ_TOOLS);
            let one = oneshot::Ask {
                prompt: ask.prompt,
                system: ask.system,
                account: self.account.clone(),
                model: MODEL.into(),
                effort: EFFORT.into(),
                timeout: ASK_TIMEOUT,
            };
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

/// Ask the model for `dossier`'s entries, with the store's `index` to
/// pick topics from.
pub(crate) async fn extract(
    model: &dyn Model,
    dossier: &Dossier,
    index: Option<&str>,
) -> Result<Extracted, String> {
    let index = index.unwrap_or("(none yet)");
    let prompt = format!(
        "The repo's existing knowledge index:\n{index}\n\nThe dossier:\n\n{}",
        dossier.text
    );
    let reply = model
        .ask(ModelAsk {
            system: format!("{EXTRACT_SYSTEM}\n\n{JSON_ONLY}"),
            prompt,
            cwd: None,
            read_tools: false,
        })
        .await?;
    let v: Value = oneshot::parse_json(&reply)?;
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

/// What a check-and-merge returned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Checked {
    pub page: String,
    /// `(id, why)` of each entry or rule that doesn't hold.
    pub dropped: Vec<(String, String)>,
}

/// Check `new` against the code at `code` (a [`CheckTree`]) and merge
/// them into `topic`'s page — or, with `new` empty, check the page's
/// rules alone (the staleness pass). With no code to read (`Err`), `new`
/// is merged unchecked, and nothing is dropped.
pub(crate) async fn check_topic(
    model: &dyn Model,
    store: &Store,
    topic: &str,
    new: &[Entry],
    code: Result<&Path, &str>,
) -> Result<Checked, String> {
    let page = store.page(topic).unwrap_or_else(|| "(no page yet)".into());
    let task = match new.is_empty() {
        true => STALE_TASK.to_string(),
        false => {
            let list: Vec<Value> = new
                .iter()
                .map(|e| {
                    json!({"id": e.id, "pr": e.pr, "stream": e.stream, "kind": e.kind,
                        "rule": e.rule, "applies_to": e.applies_to, "why": e.why, "by": e.by,
                        "confidence": e.confidence, "quote": e.quote,
                        "options_rejected": e.options_rejected, "confirm_by": e.confirm_by})
                })
                .collect();
            format!(
                "New entries:\n{}",
                serde_json::to_string_pretty(&list).unwrap_or_default()
            )
        }
    };
    let prompt = format!("Topic: {topic}\n\nCurrent page:\n{page}\n\n{task}");
    let ask = match code {
        Ok(dir) => ModelAsk {
            system: check_system(),
            prompt,
            cwd: Some(dir.to_path_buf()),
            read_tools: true,
        },
        Err(_) => ModelAsk {
            system: format!("{}\n\n{UNCHECKED}", check_system()),
            prompt,
            cwd: None,
            read_tools: false,
        },
    };
    let reply = model.ask(ask).await?;
    let v: Value = oneshot::parse_json(&reply)?;
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
    let words = page.split_whitespace().count();
    if words > PAGE_WORDS + PAGE_SLACK {
        tracing::warn!("knowledge: {topic}'s page runs {words} words");
    }
    let dropped = v
        .get("dropped")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let id = d.get("id")?.as_str()?.to_string();
            let why = d.get("why").and_then(Value::as_str).unwrap_or_default();
            Some((id, why.to_string()))
        })
        // Unchecked, nothing was found not to hold.
        .filter(|_| code.is_ok())
        .collect();
    Ok(Checked {
        page: page + "\n",
        dropped,
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

/// Harvest the merged `pr` of the repo at `root` into `store`: its
/// dossier from `comments` and `sessions`, checked against a checkout of
/// the merge ([`CheckTree::of_merge`], removed after). The outcome, and
/// why the entries went unchecked when there was no checkout to read.
pub(crate) async fn harvest_merge(
    model: &dyn Model,
    store: &Store,
    project: &str,
    root: &Path,
    pr: &Pr,
    comments: &[Comment],
    sessions: &[Session],
) -> (Result<Outcome, String>, Option<String>) {
    let dossier = dossier(pr, comments, sessions);
    let tree = CheckTree::of_merge(root, pr).await;
    let outcome = harvest(model, store, project, pr, &dossier, code_at(&tree)).await;
    match tree {
        Ok(tree) => {
            tree.remove().await;
            (outcome, None)
        }
        Err(why) => (outcome, Some(why)),
    }
}

/// Harvest `pr` from `dossier` into `store` (made if it isn't yet), and
/// commit it as `harvest #<pr>`, the entries checked against the code at
/// `code` ([`CheckTree::of_merge`]) — or, with none (`Err`, why), kept
/// unchecked at low confidence with a note saying so. Nothing is written
/// unless every step worked.
pub(crate) async fn harvest(
    model: &dyn Model,
    store: &Store,
    project: &str,
    pr: &Pr,
    dossier: &Dossier,
    code: Result<&Path, &str>,
) -> Result<Outcome, String> {
    store.init()?;
    let index = store.index();
    let extracted = extract(model, dossier, index.as_deref()).await?;
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
            if let Err(why) = code {
                e.confidence = "low".into();
                e.note = format!("not checked against the code: {why}");
            }
            e
        })
        .collect();

    let mut by_topic: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for e in &entries {
        by_topic.entry(e.topic.clone()).or_default().push(e.clone());
    }
    let mut pages = Vec::new();
    for (topic, new) in &by_topic {
        let checked = check_topic(model, store, topic, new, code).await?;
        for (id, why) in &checked.dropped {
            if let Some(e) = entries.iter_mut().find(|e| &e.id == id) {
                e.status = DROPPED.into();
                e.note = why.clone();
            }
        }
        pages.push((topic.clone(), checked.page));
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
    store.write_source(pr.number, &source)?;
    store.append(&entries)?;
    store.write_index(project)?;
    store.commit(&format!("harvest #{}", pr.number))?;
    Ok(Outcome {
        pr: pr.number,
        kept: entries.iter().filter(|e| e.status == ACTIVE).count(),
        dropped,
        topics: by_topic.into_keys().collect(),
    })
}

/// The staleness pass: every topic's rules checked against the code at
/// `code` (the base as origin has it, [`CheckTree::of_base`]), the ones
/// that no longer hold moved to Replaced. Committed as `staleness
/// check`. The topics changed.
pub(crate) async fn staleness(
    model: &dyn Model,
    store: &Store,
    project: &str,
    code: &Path,
) -> Result<Vec<String>, String> {
    let mut changed = Vec::new();
    let mut entries = store.entries();
    for topic in store.topics() {
        let checked = match check_topic(model, store, &topic, &[], Ok(code)).await {
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
        if store.page(&topic).as_deref() != Some(checked.page.as_str()) {
            store.write_page(&topic, &checked.page)?;
            changed.push(topic);
        }
    }
    store.save_entries(&entries)?;
    store.write_index(project)?;
    store.commit("staleness check")?;
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
fn default_base(repo: &Path) -> String {
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

const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// Pull request `number` of the repo at `root`, with its thread: the
/// conversation, the reviews and the comments on lines.
pub(crate) async fn gather_pr(root: &Path, number: u64) -> Result<(Pr, Vec<Comment>), String> {
    let n = number.to_string();
    let view = crate::pull_request::gh(
        Some(root),
        &[
            "pr",
            "view",
            &n,
            "--json",
            "number,url,title,body,headRefName,baseRefName,mergeCommit,mergedAt,comments,reviews",
        ],
        GH_TIMEOUT,
    )
    .await
    .ok_or_else(|| format!("gh couldn't read PR #{number}"))?;
    let view: Value = serde_json::from_str(&view).map_err(|e| e.to_string())?;
    let path = format!("repos/{{owner}}/{{repo}}/pulls/{number}/comments");
    let lines = crate::pull_request::gh(
        Some(root),
        &["api", &path, "--paginate", "--jq", ".[]"],
        GH_TIMEOUT,
    )
    .await
    .unwrap_or_default();
    // The gh user: who "OWNER" is on the thread.
    let me = crate::pull_request::viewer_login()
        .await
        .unwrap_or_default();
    Ok(read_pr(&view, &lines, me))
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

    #[tokio::test]
    async fn a_harvest_guards_checks_writes_and_commits() {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        let model = Scripted::new(vec![extraction(), checked_page()]);
        let d = dossier(&pr(), &[], &sessions());
        let out = harvest(&model, &store, "app", &pr(), &d, Ok(repo.path()))
            .await
            .unwrap();
        assert_eq!(
            out,
            Outcome {
                pr: 12,
                kept: 1,
                dropped: 2,
                topics: vec!["auth/login".into()]
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
        assert!(store.page("auth/login").unwrap().contains("<!-- k12-1 -->"));
        assert!(store
            .index()
            .unwrap()
            .contains("- `auth/login` — src/auth — 1 rule"));
        let source = std::fs::read_to_string(store.source_path(12)).unwrap();
        assert!(source.contains("## Considered and skipped\n- the redirect refactor"));
        let log = crate::git_proc::read(&store.dir, &["log", "--format=%s"]).unwrap();
        assert_eq!(log.trim(), "harvest #12");
    }

    #[tokio::test]
    async fn a_failed_check_writes_nothing() {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        let model = Scripted::new(vec![extraction(), r#"{"page": "nonsense"}"#.into()]);
        let d = dossier(&pr(), &[], &sessions());
        assert!(harvest(&model, &store, "app", &pr(), &d, Ok(repo.path()))
            .await
            .is_err());
        assert!(store.entries().is_empty());
        assert!(!store.harvested(12));
    }

    #[tokio::test]
    async fn with_no_checkout_the_entries_are_kept_unchecked_at_low_confidence() {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        let model = Scripted::new(vec![extraction(), checked_page()]);
        let d = dossier(&pr(), &[], &sessions());
        let out = harvest(&model, &store, "app", &pr(), &d, Err("no origin"))
            .await
            .unwrap();
        assert_eq!(
            (out.kept, out.dropped),
            (2, 1),
            "only the guard dropped one"
        );
        let asked = model.asked.lock().unwrap();
        assert!(!asked[1].read_tools && asked[1].cwd.is_none());
        assert!(asked[1].system.ends_with(UNCHECKED));
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
        let store = Store::of(&clone);
        let model = Scripted::new(vec![extraction(), checked_page()]);
        let (out, unchecked) =
            harvest_merge(&model, &store, "app", &clone, &merged, &[], &sessions()).await;
        assert_eq!(out.unwrap().pr, 4343);
        assert_eq!(unchecked, None);
        let asked = std::mem::take(&mut *model.asked.lock().unwrap());
        assert!(asked[0].prompt.contains("Keep the return path on login"));
        let cwd = asked[1].cwd.clone().expect("the check ran in the checkout");
        assert!(asked[1].read_tools && cwd.ends_with("4343"));
        assert!(!cwd.exists(), "the checkout is removed after");

        // Nothing to fetch from: harvested all the same, saying why.
        let bare = tempfile::tempdir().unwrap();
        let store = Store::of(bare.path());
        let model = Scripted::new(vec![extraction(), checked_page()]);
        let (out, unchecked) =
            harvest_merge(&model, &store, "app", bare.path(), &pr(), &[], &sessions()).await;
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
