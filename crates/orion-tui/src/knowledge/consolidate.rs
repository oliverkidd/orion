//! CONSOLIDATION: the topics kept few, so the index stays short and an
//! agent finds the right page. At most about [`TOPIC_CAP`] topics in
//! all, and about [`AREA_CAP`] an area.
//!
//! One call an area ([`ask_area`], on Sonnet at low effort unless
//! `knowledge.models.consolidate` says otherwise) is given the area's topics and
//! their rules, and maps each old topic to a new one, merging near
//! duplicates. orion then rewrites the entries' topics, makes each merged
//! page through the check-and-merge (its rules deduplicated, citing every
//! PR, [`merge_pages`]), and deletes the old pages. A topic whose area
//! isn't a listed one moves to the nearest that is (`super::areas`). The
//! caller commits it all as one commit (`super::share`).
//!
//! It runs from `orion knowledge consolidate`, at the end of a backfill,
//! and after any harvest that takes the topics past [`TOPIC_CAP`].

use std::collections::{BTreeMap, BTreeSet};

use futures::future::BoxFuture;
use futures::stream::{self, StreamExt};
use serde_json::Value;

use super::areas::Areas;
use super::harvest::{self, Checked, Model, ModelAsk, Target};
use super::store::{self, Entry, ACTIVE, SECTIONS};
use super::Step;

/// The most topics there should be, in all.
pub(crate) const TOPIC_CAP: usize = 60;
/// The most topics an area should have.
pub(crate) const AREA_CAP: usize = 8;
/// How many rules of a topic an area's call reads, and how much of each.
const TOPIC_RULES: usize = 30;
const RULE_CHARS: usize = 220;
/// The commit a consolidation is.
pub(crate) const MESSAGE: &str = "knowledge: consolidate topics";
/// How many calls run at once.
const AT_ONCE: usize = 4;

/// The area call's rules.
pub(crate) const AREA_SYSTEM: &str = r#"You keep the topic list of one area of a repository's knowledge store: the rules its owner set for the agents that work in it, one page per topic. Too many small topics make the right page hard to find.

Merge topics that overlap, repeat each other or are too small to stand alone, so the area has at most the number of topics you are given; fewer is fine. Every topic is <area>/<name>: the area you are given, and a name of one to three kebab-case words saying what it covers. Keep a topic's name when it still fits. Map every topic you are given to exactly one topic. Give every resulting topic a scope: one line, under 15 words, no file paths.

Reply with one JSON object: {"topics": {"<old topic>": "<new topic>"}, "scopes": {"<new topic>": "<scope>"}}"#;

/// What a merged page's check is asked to do.
const MERGE_TASK: &str = "There are no new entries. The pages above become this one topic: write its one page. Keep every rule, constraint, open question and replaced line they have. Merge rules that say the same thing into one rule citing every PR and keeping every id. Drop nothing and move nothing; leave dropped and moved empty. The page may run past the word limit rather than lose a rule.";

/// What a consolidation did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Consolidated {
    /// Topics with a page, before and after.
    pub before: usize,
    pub after: usize,
    /// Each new topic made from more than one old one, or renamed: the
    /// old topics it took.
    pub merged: Vec<(String, Vec<String>)>,
}

impl Consolidated {
    pub(crate) fn line(&self) -> String {
        format!(
            "Topics consolidated: {} → {} ({} pages merged or moved).",
            self.before,
            self.after,
            self.merged.len()
        )
    }
}

/// Whether `store` has more topics than [`TOPIC_CAP`]: a harvest that
/// left it so consolidates.
pub(crate) fn over_cap(store: &store::Store) -> bool {
    store.topics().len() > TOPIC_CAP
}

/// One area, as its call is given it.
struct AreaAsk {
    area: String,
    scope: String,
    budget: usize,
    /// Each topic in it: name, scope, rules.
    topics: Vec<(String, String, Vec<String>)>,
}

/// Ask the model to map `ask`'s topics onto at most its budget. The map
/// old → new, every old topic in it and every new one in the area; and
/// the new topics' scopes.
async fn ask_area(
    model: &dyn Model,
    ask: &AreaAsk,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>), String> {
    let mut prompt = format!(
        "Area: {} — {}\nAt most {} topics.\n\nIts topics, with their scope and rules:\n",
        ask.area, ask.scope, ask.budget
    );
    for (topic, scope, rules) in &ask.topics {
        prompt.push_str(&format!("\n### {topic}\nScope: {scope}\n"));
        for rule in rules.iter().take(TOPIC_RULES) {
            prompt.push_str(&format!("- {}\n", crate::ui::truncate(rule, RULE_CHARS)));
        }
        if rules.len() > TOPIC_RULES {
            prompt.push_str(&format!("- … and {} more\n", rules.len() - TOPIC_RULES));
        }
    }
    prompt.push_str(&format!("\n{}", harvest::JSON_ONLY));
    let v = harvest::ask_json(
        model,
        ModelAsk {
            system: format!("{AREA_SYSTEM}\n\n{}", harvest::JSON_ONLY),
            prompt,
            cwd: None,
            read_tools: false,
            step: Step::Consolidate,
        },
        |v| v.get("topics").is_some_and(Value::is_object),
    )
    .await?;
    let in_area = |raw: &str| {
        let path = store::topic_path(raw);
        let name = path.split_once('/').map_or(path.as_str(), |(_, n)| n);
        format!("{}/{name}", ask.area)
    };
    let said: BTreeMap<String, String> = v["topics"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(old, new)| Some((old.trim().to_string(), new.as_str()?.to_string())))
        .collect();
    let mut map = BTreeMap::new();
    for (topic, _, _) in &ask.topics {
        let new = said
            .get(topic)
            .map(|n| in_area(n))
            .unwrap_or_else(|| in_area(topic));
        map.insert(topic.clone(), new);
    }
    let scopes = v["scopes"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(new, scope)| Some((in_area(new), scope.as_str()?.trim().to_string())))
        .collect();
    Ok((map, scopes))
}

/// `pages` (old topic, page) made into `topic`'s one page by the
/// check-and-merge, unchecked: rules deduplicated, every PR cited, and
/// any id the model left out put back on the line it was on.
async fn merge_pages(
    model: &dyn Model,
    records: &str,
    topic: &str,
    scope: Option<&str>,
    pages: &[(String, String)],
) -> Result<String, String> {
    let mut merged = String::new();
    for (old, page) in pages {
        merged.push_str(&format!("\n### {old}\n{}\n", page.trim()));
    }
    let scope_line = match scope {
        Some(scope) => format!("Its scope line: {scope}\n"),
        None => String::new(),
    };
    let prompt = format!(
        "Topic: {topic}\n{scope_line}\n{}\n\nCurrent page:\n(no page yet)\n\nThe pages merged into it:\n{merged}\n{MERGE_TASK}\n\n{}",
        harvest::records_part(records),
        harvest::JSON_ONLY
    );
    let ask = ModelAsk {
        system: format!(
            "{}\n\n{}\n\n{}\n\n{}",
            harvest::check_system(),
            harvest::RECORDS_RULE,
            harvest::UNCHECKED,
            harvest::JSON_ONLY
        ),
        prompt,
        cwd: None,
        read_tools: false,
        step: Step::Consolidate,
    };
    let Checked { page, .. } = harvest::ask_page(model, ask, topic, scope).await?;
    Ok(keep_every_id(&page, pages))
}

/// `page` with every rule of `olds` it lost put back: a line of an old
/// page citing an id `page` doesn't, under the same section.
pub(crate) fn keep_every_id(page: &str, olds: &[(String, String)]) -> String {
    let have: BTreeSet<&str> = page.lines().flat_map(store::line_ids).collect();
    let mut lost: Vec<(String, String)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (_, old) in olds {
        let mut section = SECTIONS[0].to_string();
        for line in old.lines() {
            if let Some(head) = line.strip_prefix("## ") {
                section = head.trim().to_string();
                continue;
            }
            let ids = store::line_ids(line);
            let missing: Vec<&str> = ids
                .iter()
                .copied()
                .filter(|id| !have.contains(id) && !seen.contains(*id))
                .collect();
            if missing.is_empty() {
                continue;
            }
            seen.extend(missing.iter().map(|s| s.to_string()));
            let others: Vec<&str> = ids.into_iter().filter(|i| !missing.contains(i)).collect();
            let line = match others.is_empty() {
                true => line.to_string(),
                false => store::without_ids(line, &others),
            };
            lost.push((section.clone(), line));
        }
    }
    if lost.is_empty() {
        return page.to_string();
    }
    let mut out: Vec<String> = page.lines().map(str::to_string).collect();
    for (section, line) in lost {
        let head = format!("## {section}");
        let start = out.iter().position(|l| l.trim() == head);
        let at = match start {
            Some(start) => out[start + 1..]
                .iter()
                .position(|l| l.starts_with("## "))
                .map_or(out.len(), |n| start + 1 + n),
            None => out.len(),
        };
        // Before the blank lines that end the section.
        let mut at = at;
        while at > 0 && out[at - 1].trim().is_empty() {
            at -= 1;
        }
        out.insert(at, line);
    }
    out.join("\n") + "\n"
}

/// An area's answer: old topic → new, and the new topics' scopes.
type Answer = (BTreeMap<String, String>, BTreeMap<String, String>);

/// `ask`'s answer: the model's, when there is more than one topic to
/// map; else — or when the call fails — each topic kept, moved into the
/// area as `placed` says.
async fn area_answer(
    model: &dyn Model,
    ask: &AreaAsk,
    placed: &BTreeMap<String, String>,
) -> Answer {
    let keep = || {
        let map = ask
            .topics
            .iter()
            .map(|(t, _, _)| {
                (
                    t.clone(),
                    placed.get(t).cloned().unwrap_or_else(|| t.clone()),
                )
            })
            .collect();
        (map, BTreeMap::new())
    };
    if ask.topics.len() < 2 {
        return keep();
    }
    match ask_area(model, ask).await {
        Ok(answer) => answer,
        Err(why) => {
            tracing::warn!("knowledge: consolidating {} failed: {why}", ask.area);
            keep()
        }
    }
}

/// A new topic's page, to make from the old topics it takes.
struct NewPage {
    topic: String,
    scope: Option<String>,
    /// The old topics' pages.
    olds: Vec<(String, String)>,
    /// The old topics' entries.
    entries: Vec<Entry>,
}

impl NewPage {
    /// The page from the entries alone ([`store::render_page`]); None
    /// with none in force.
    fn made_of_entries(&self) -> Option<String> {
        let refs: Vec<&Entry> = self.entries.iter().collect();
        refs.iter().any(|e| e.status == ACTIVE).then(|| {
            let page = store::render_page(&self.topic, &refs);
            store::tidy_page(&self.topic, &page, self.scope.as_deref())
        })
    }

    /// `(topic, page)`: one old page renamed, several merged by the
    /// model — or, should that fail, made from the entries.
    async fn make(self, model: &dyn Model, records: &str) -> Option<(String, String)> {
        let scope = self.scope.as_deref();
        let page = match self.olds.as_slice() {
            [] => self.made_of_entries()?,
            [(_, one)] => store::tidy_page(&self.topic, one, scope),
            olds => match merge_pages(model, records, &self.topic, scope, olds).await {
                Ok(page) => page,
                Err(why) => {
                    tracing::warn!(
                        "knowledge: merging into {} failed, made from its entries: {why}",
                        self.topic
                    );
                    self.made_of_entries()?
                }
            },
        };
        Some((self.topic, page))
    }
}

/// Consolidate `to`'s topics: one call an area, the entries' topics
/// rewritten, merged pages made, old pages deleted, the areas and the
/// index written. Not committed (the caller does).
pub(crate) async fn consolidate(
    model: &dyn Model,
    to: &Target<'_>,
) -> Result<Consolidated, String> {
    let store = to.store;
    let areas = Areas::load(store);
    let mut entries = store.entries();
    let pages: BTreeMap<String, String> = store.pages().into_iter().collect();
    let before = pages.len();

    let mut topics: BTreeSet<String> = pages.keys().cloned().collect();
    topics.extend(
        entries
            .iter()
            .filter(|e| e.status == ACTIVE)
            .map(|e| e.topic.clone()),
    );
    let placed: BTreeMap<String, String> =
        topics.iter().map(|t| (t.clone(), areas.topic(t))).collect();
    let rules_of = |topic: &str| -> Vec<String> {
        entries
            .iter()
            .filter(|e| e.topic == topic && e.status == ACTIVE)
            .map(|e| e.rule.clone())
            .collect()
    };
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (old, new) in &placed {
        let area = new.split('/').next().unwrap_or_default().to_string();
        groups.entry(area).or_default().push(old.clone());
    }
    let total: usize = entries.iter().filter(|e| e.status == ACTIVE).count();
    let asks: Vec<AreaAsk> = groups
        .iter()
        .map(|(area, olds)| {
            let rules: usize = olds.iter().map(|t| rules_of(t).len()).sum();
            let budget = (TOPIC_CAP * rules / total.max(1)).clamp(1, AREA_CAP);
            AreaAsk {
                area: area.clone(),
                scope: areas
                    .list
                    .iter()
                    .find(|a| a.name == *area)
                    .map(|a| a.scope.clone())
                    .unwrap_or_default(),
                budget,
                topics: olds
                    .iter()
                    .map(|t| {
                        let scope = pages
                            .get(t)
                            .map(|p| store::short_scope(&store::scope(p), t))
                            .unwrap_or_default();
                        (t.clone(), scope, rules_of(t))
                    })
                    .collect(),
            }
        })
        .collect();

    // One call an area with more than one topic.
    let calls: Vec<BoxFuture<'_, Answer>> = asks
        .iter()
        .map(|ask| Box::pin(area_answer(model, ask, &placed)) as BoxFuture<'_, Answer>)
        .collect();
    let answers: Vec<Answer> = stream::iter(calls).buffered(AT_ONCE).collect().await;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let mut scopes: BTreeMap<String, String> = BTreeMap::new();
    for (m, s) in answers {
        map.extend(m);
        scopes.extend(s);
    }
    let mut made: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (old, new) in &map {
        made.entry(new.clone()).or_default().push(old.clone());
    }

    // Each new topic's page: kept, renamed, or merged by the model.
    let jobs: Vec<BoxFuture<'_, Option<(String, String)>>> = made
        .iter()
        .map(|(new, olds)| {
            let olds_pages: Vec<(String, String)> = olds
                .iter()
                .filter_map(|o| Some((o.clone(), pages.get(o)?.clone())))
                .collect();
            let of_topic: Vec<Entry> = entries
                .iter()
                .filter(|e| olds.contains(&e.topic))
                .cloned()
                .collect();
            let job = NewPage {
                topic: new.clone(),
                scope: scopes.get(new).cloned(),
                olds: olds_pages,
                entries: of_topic,
            };
            Box::pin(job.make(model, to.records)) as BoxFuture<'_, _>
        })
        .collect();
    let written: Vec<Option<(String, String)>> =
        stream::iter(jobs).buffered(AT_ONCE).collect().await;
    let new_pages: Vec<(String, String)> = written.into_iter().flatten().collect();

    // Everything asked: now written.
    let keeping: BTreeSet<&str> = new_pages.iter().map(|(t, _)| t.as_str()).collect();
    for (topic, page) in &new_pages {
        store.write_page(topic, page)?;
    }
    for old in pages.keys() {
        if !keeping.contains(old.as_str()) {
            store.remove_page(old)?;
        }
    }
    for e in entries.iter_mut() {
        e.topic = match map.get(&e.topic) {
            Some(new) => new.clone(),
            None => areas.topic(&e.topic),
        };
    }
    store.save_entries(&entries)?;
    store.keep_product()?;
    areas.save(store)?;
    store.write_index(to.project)?;
    let merged = made
        .into_iter()
        .filter(|(new, olds)| olds.len() > 1 || olds.first() != Some(new))
        .collect();
    Ok(Consolidated {
        before,
        after: store.topics().len(),
        merged,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::harvest::tests::Bench;
    use serde_json::json;
    use std::sync::Mutex;

    /// A model that answers by what it is asked, and remembers the asks.
    struct ByPrompt {
        answer: fn(&ModelAsk) -> String,
        asked: Mutex<Vec<ModelAsk>>,
    }

    impl Model for ByPrompt {
        fn ask(&self, ask: ModelAsk) -> BoxFuture<'_, Result<String, String>> {
            let reply = (self.answer)(&ask);
            self.asked.lock().unwrap().push(ask);
            Box::pin(async move { Ok(reply) })
        }
    }

    fn entry(id: &str, topic: &str, rule: &str) -> Entry {
        Entry {
            id: id.into(),
            pr: id[1..].split('-').next().unwrap().parse().unwrap(),
            topic: topic.into(),
            stream: "product".into(),
            kind: "decision".into(),
            rule: rule.into(),
            by: "owner".into(),
            status: ACTIVE.into(),
            ..Entry::default()
        }
    }

    fn page(topic: &str, scope: &str, lines: &[&str]) -> String {
        format!(
            "# {topic}\nScope: {scope}\n\n## Rules in force\n{}\n\n## Constraints\n\n## Open questions\n\n## Replaced\n",
            lines.join("\n")
        )
    }

    fn answer(ask: &ModelAsk) -> String {
        if ask.system.starts_with("You keep the topic list") {
            assert!(ask.prompt.starts_with("Area: editor"), "{}", ask.prompt);
            return json!({
                "topics": {
                    "canvas/snapping": "editor/snapping",
                    "snap/guides": "snapping",
                    "shape/handles": "editor/handles"
                },
                "scopes": {"editor/snapping": "Snapping shapes: grid, guides, `src/x.ts` and modifiers"}
            })
            .to_string();
        }
        // The merge: two pages into one, k2-1 left out by the model.
        assert!(
            ask.prompt.starts_with("Topic: editor/snapping\n"),
            "{}",
            ask.prompt
        );
        assert!(ask.prompt.contains("### canvas/snapping\n"));
        assert!(ask.prompt.contains("### snap/guides\n"));
        json!({"page": "# whatever\nScope: x\n\n## Rules in force\n- Shapes snap to the grid and guides (#1, #3) <!-- k1-1 k3-1 -->\n\n## Constraints\n\n## Open questions\n\n## Replaced\n"}).to_string()
    }

    #[tokio::test]
    async fn a_consolidation_merges_topics_by_the_models_map() {
        let into = Bench::new();
        let store = &into.store;
        let entries = vec![
            entry("k1-1", "canvas/snapping", "Shapes snap to the grid"),
            entry("k2-1", "canvas/snapping", "Alt turns snapping off"),
            entry("k3-1", "snap/guides", "Shapes snap to guides"),
            entry("k4-1", "shape/handles", "Handles stay 8px"),
            entry("k5-1", "billing/invoices", "Invoices show VAT"),
        ];
        store.append(&entries).unwrap();
        store
            .write_page(
                "canvas/snapping",
                &page(
                    "canvas/snapping",
                    "snapping (`snap.ts`), grid",
                    &[
                        "- Shapes snap to the grid (#1) <!-- k1-1 -->",
                        "- Alt turns snapping off (#2) <!-- k2-1 -->",
                    ],
                ),
            )
            .unwrap();
        store
            .write_page(
                "snap/guides",
                &page(
                    "snap/guides",
                    "guides",
                    &["- Shapes snap to guides (#3) <!-- k3-1 -->"],
                ),
            )
            .unwrap();
        store
            .write_page(
                "shape/handles",
                &page(
                    "shape/handles",
                    "handles",
                    &["- Handles stay 8px (#4) <!-- k4-1 -->"],
                ),
            )
            .unwrap();
        store
            .write_page(
                "billing/invoices",
                &page(
                    "billing/invoices",
                    "invoices in `src/billing.ts`",
                    &["- Invoices show VAT (#5) <!-- k5-1 -->"],
                ),
            )
            .unwrap();

        let model = ByPrompt {
            answer,
            asked: Mutex::new(Vec::new()),
        };
        let done = consolidate(&model, &into.target("")).await.unwrap();
        assert_eq!((done.before, done.after), (4, 3));
        assert_eq!(
            model.asked.lock().unwrap().len(),
            2,
            "one call for the area with three topics, one merge; none for access-and-accounts' one"
        );
        assert!(
            model
                .asked
                .lock()
                .unwrap()
                .iter()
                .all(|ask| ask.step == Step::Consolidate),
            "both on the consolidation's model"
        );

        // The entries' topics are rewritten.
        let topics: Vec<(String, String)> = store
            .entries()
            .into_iter()
            .map(|e| (e.id, e.topic))
            .collect();
        assert_eq!(
            topics,
            vec![
                ("k1-1".into(), "editor/snapping".into()),
                ("k2-1".into(), "editor/snapping".into()),
                ("k3-1".into(), "editor/snapping".into()),
                ("k4-1".into(), "editor/handles".into()),
                ("k5-1".into(), "access-and-accounts/invoices".into()),
            ]
        );
        // The old pages are gone; the new ones are there.
        assert_eq!(
            store.topics(),
            vec![
                "access-and-accounts/invoices",
                "editor/handles",
                "editor/snapping"
            ]
        );
        assert!(!store.dir.join("canvas").exists());
        // The merged page cites every PR and every id, the lost one put back.
        let merged = store.page("editor/snapping").unwrap();
        assert!(
            merged.starts_with(
                "# editor/snapping\nScope: Snapping shapes: grid, guides, and modifiers\n"
            ),
            "{merged}"
        );
        assert!(merged.contains("<!-- k1-1 k3-1 -->"), "{merged}");
        assert!(
            merged.contains("- Alt turns snapping off (#2) <!-- k2-1 -->\n\n## Constraints"),
            "{merged}"
        );
        let moved = store.page("access-and-accounts/invoices").unwrap();
        assert!(
            moved.starts_with("# access-and-accounts/invoices\nScope: invoices in\n"),
            "{moved}"
        );
        // The index is by area, and areas.md is written.
        let index = store.index().unwrap();
        assert!(
            index.contains("- editor: handles, snapping — 3 rules"),
            "{index}"
        );
        assert!(store.dir.join("areas.md").exists());
        assert_eq!(done.merged.len(), 3);
    }

    #[test]
    fn a_lost_id_goes_back_under_its_section() {
        let olds = vec![(
            "a/b".to_string(),
            "# a/b\n## Rules in force\n- one <!-- k1-1 k1-2 -->\n## Constraints\n- fact <!-- k2-1 -->\n".to_string(),
        )];
        let page = "# a/c\nScope: c\n\n## Rules in force\n- one <!-- k1-1 -->\n\n## Constraints\n\n## Open questions\n\n## Replaced\n";
        assert_eq!(
            keep_every_id(page, &olds),
            "# a/c\nScope: c\n\n## Rules in force\n- one <!-- k1-1 -->\n- one <!-- k1-2 -->\n\n## Constraints\n- fact <!-- k2-1 -->\n\n## Open questions\n\n## Replaced\n"
        );
        assert_eq!(keep_every_id(&olds[0].1, &olds), olds[0].1);
    }
}
