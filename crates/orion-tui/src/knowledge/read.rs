//! The READ PATH: what agents get, read from their own checkout — the
//! harvested pages as the branch they work on has them, so a harvest is
//! seen once its pull request merges, not before. Every builder prompt
//! carries the index and lists the hand-written records
//! ([`prompt_context`]); `orion knowledge for <paths…>` prints the pages
//! for the files an agent is about to touch ([`pages_for`]). Paths are
//! repo-relative throughout.

use std::collections::BTreeSet;

use super::records::{self, Checkout};
use super::store::{self, Entry, Store, ACTIVE, DECISIONS, DIR};

/// What every builder prompt's knowledge ends on.
pub(crate) const RECORDS_WIN: &str =
    "Accepted records in .orion/decisions/ win over the harvested pages.";

/// The most topics `orion knowledge for` prints.
pub(crate) const FOR_TOPICS: usize = 4;
/// The most words it prints.
pub(crate) const FOR_WORDS: usize = 2_400;

/// Path parts too common to say anything about a topic.
const NOISE: &[&str] = &[
    "src",
    "lib",
    "app",
    "apps",
    "packages",
    "package",
    "crates",
    "index",
    "mod",
    "main",
    "test",
    "tests",
    "spec",
    "specs",
    "components",
    "component",
    "utils",
    "util",
    "common",
    "shared",
    "the",
    "and",
    "for",
    "tsx",
    "jsx",
    "rs",
    "ts",
    "js",
    "md",
    "json",
    "toml",
    "yaml",
    "yml",
    "css",
    "html",
    "general",
    "misc",
];

/// What a builder's prompt carries from `checkout` (its worktree): the
/// harvested index, where the pages are, and the hand-written records'
/// titles and paths. Empty when the checkout has neither.
pub(crate) fn prompt_context(checkout: &Checkout) -> String {
    let files = checkout.files(DECISIONS);
    let index_path = format!("{DIR}/index.md");
    let index = files
        .iter()
        .find(|(path, _)| *path == index_path)
        .map(|(_, text)| store::unmarked(text).trim().to_string())
        .filter(|i| !i.is_empty());
    let records = records::from_files(&files);
    if index.is_none() && records.is_empty() {
        return String::new();
    }
    let mut out = Vec::new();
    if let Some(index) = index {
        out.push(index);
        out.push(format!(
            "Before planning, read the topic pages for the areas you'll touch, in `{DIR}/` (`orion knowledge for <paths…>` prints them)."
        ));
    }
    if !records.is_empty() {
        out.push(format!(
            "Hand-written decision records:\n{}",
            records::listing(&records)
        ));
    }
    out.push(RECORDS_WIN.to_string());
    out.join("\n\n")
}

/// The areas and topics in `checkout` — its `areas.md` (else the seed)
/// and each page with its scope — as a builder is given them to note the
/// rules it learned on (`orion knowledge note`, `super::notes`).
pub(crate) fn topics_catalog(checkout: &Checkout) -> String {
    let files = checkout.files(DIR);
    let areas = files
        .iter()
        .find(|(path, _)| *path == format!("{DIR}/areas.md"))
        .map(|(_, text)| super::areas::parse(store::unmarked(text)))
        .filter(|a| !a.list.is_empty())
        .unwrap_or_default();
    let topics: Vec<(String, String)> = files
        .iter()
        .filter_map(|(path, text)| {
            let rest = path.strip_prefix(&format!("{DIR}/"))?.strip_suffix(".md")?;
            let (area, name) = rest.split_once('/')?;
            if name.contains('/') {
                return None;
            }
            let topic = format!("{area}/{name}");
            let scope = store::short_scope(&store::scope(store::unmarked(text)), &topic);
            Some((topic, scope))
        })
        .collect();
    super::harvest::catalog(&areas, &topics)
}

/// The words of a path or topic worth matching on.
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|w| w.len() >= 3 && !NOISE.contains(&w.as_str()))
        .collect()
}

/// Whether `cited` (a path an entry names) and `path` are about the same
/// files: one is the other, or inside it.
pub(super) fn same_files(cited: &str, path: &str) -> bool {
    let cited = cited.trim().trim_start_matches("./").trim_end_matches('/');
    let path = path.trim_start_matches("./").trim_end_matches('/');
    if cited.len() < 3 || !cited.contains(['/', '.']) {
        return false;
    }
    path == cited
        || path.starts_with(&format!("{cited}/"))
        || cited.starts_with(&format!("{path}/"))
        || path.ends_with(&format!("/{cited}"))
}

/// How well `topic` fits `paths`: its name's words in theirs, and the
/// paths its entries cite.
pub(crate) fn score(topic: &str, entries: &[&Entry], paths: &[String]) -> usize {
    let topic_words = words(topic);
    let mut score = 0;
    for path in paths {
        let path_words = words(path);
        score += 2 * topic_words.intersection(&path_words).count();
        for e in entries {
            let cites = e.applies_to.iter().chain(std::iter::once(&e.confirm_by));
            if cites
                .flat_map(|c| c.split([',', ';', ' ']))
                .any(|c| same_files(c, path))
            {
                score += 3;
            }
        }
    }
    score
}

/// The topics for `paths` (repo-relative), best first: at most
/// [`FOR_TOPICS`], each fitting somehow.
pub(crate) fn topics_for(store: &Store, paths: &[String]) -> Vec<String> {
    let entries = store.entries();
    let mut scored: Vec<(usize, String)> = store
        .topics()
        .into_iter()
        .map(|topic| {
            let mine: Vec<&Entry> = entries
                .iter()
                .filter(|e| e.topic == topic && e.status == ACTIVE)
                .collect();
            (score(&topic, &mine, paths), topic)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(FOR_TOPICS)
        .map(|(_, t)| t)
        .collect()
}

/// What `orion knowledge for` prints: the pages for `paths`, at most
/// [`FOR_WORDS`] words in all; the index when no topic fits.
pub(crate) fn pages_for(store: &Store, paths: &[String]) -> String {
    let topics = topics_for(store, paths);
    if topics.is_empty() {
        return match store.index() {
            Some(index) => format!(
                "No topic page matches those paths. The index:\n\n{index}\nPages are in {DIR}/.\n"
            ),
            None => format!("No knowledge recorded yet in {DIR}/.\n"),
        };
    }
    let mut out = String::new();
    let mut left = FOR_WORDS;
    for topic in topics {
        let Some(page) = store.page(&topic) else {
            continue;
        };
        if left == 0 {
            break;
        }
        out.push_str(&format!("<!-- {DIR}/{} -->\n", store::page_file(&topic)));
        let n = page.split_whitespace().count();
        if n <= left {
            out.push_str(page.trim_end());
            left -= n;
        } else {
            let mut kept = 0;
            for line in page.lines() {
                let w = line.split_whitespace().count();
                if kept + w > left {
                    break;
                }
                out.push_str(line);
                out.push('\n');
                kept += w;
            }
            out.push_str("[… cut at the word limit]");
            left = 0;
        }
        out.push_str("\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(topic: &str, applies_to: &[&str], confirm_by: &str) -> Entry {
        Entry {
            topic: topic.into(),
            applies_to: applies_to.iter().map(|s| s.to_string()).collect(),
            confirm_by: confirm_by.into(),
            status: ACTIVE.into(),
            rule: "r".into(),
            ..Entry::default()
        }
    }

    fn store_with(pages: &[(&str, &str)], entries: &[Entry]) -> (tempfile::TempDir, Store) {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        std::fs::create_dir_all(&store.dir).unwrap();
        for (topic, page) in pages {
            store.write_page(topic, page).unwrap();
        }
        store.append(entries).unwrap();
        (repo, store)
    }

    #[test]
    fn paths_find_topics_by_name_and_by_what_entries_cite() {
        let (_repo, store) = store_with(
            &[
                ("auth/login", "# auth/login\n## Rules in force\n- a\n"),
                (
                    "billing/invoices",
                    "# billing/invoices\n## Rules in force\n- b\n",
                ),
                ("ui/forms", "# ui/forms\n## Rules in force\n- c\n"),
                ("ops/deploys", "# ops/deploys\n"),
            ],
            &[
                entry("ui/forms", &["apps/web/src/components/DatePicker.tsx"], ""),
                entry("billing/invoices", &[], "packages/billing/src/stripe.ts"),
            ],
        );
        assert_eq!(
            topics_for(&store, &["apps/web/src/auth/login.tsx".into()]),
            vec!["auth/login"]
        );
        assert_eq!(
            topics_for(&store, &["apps/web/src/components/DatePicker.tsx".into()]),
            vec!["ui/forms"]
        );
        assert_eq!(
            topics_for(&store, &["packages/billing".into(), "src/auth".into()]),
            vec!["billing/invoices", "auth/login"]
        );
        assert!(topics_for(&store, &["src/index.ts".into()]).is_empty());
    }

    #[test]
    fn pages_for_stays_inside_its_word_limit() {
        let long = format!(
            "# a/big\n## Rules in force\n{}",
            "- word word word word\n".repeat(1000)
        );
        let (_repo, store) = store_with(&[("a/big", &long)], &[]);
        let out = pages_for(&store, &["src/big/x.rs".into()]);
        assert!(out.split_whitespace().count() <= FOR_WORDS + 10);
        assert!(out.contains("[… cut at the word limit]"));
        let none = pages_for(&store, &["zzz".into()]);
        assert!(none.starts_with("No knowledge recorded yet"), "{none}");
    }

    #[test]
    fn the_prompt_reads_the_sessions_worktree_with_repo_relative_paths() {
        let wt = tempfile::tempdir().unwrap();
        let checkout = Checkout::Dir(wt.path().to_path_buf());
        assert_eq!(prompt_context(&checkout), "", "nothing recorded yet");
        crate::knowledge::records::tests::decisions(wt.path());
        let ctx = prompt_context(&checkout);
        assert!(ctx.starts_with("# Knowledge: app\n\nBefore planning, read the topic pages for the areas you'll touch, in `.orion/decisions/harvested/`"), "{ctx}");
        assert!(!ctx.contains("orion:harvested"), "no marker: {ctx}");
        assert!(ctx.contains("Hand-written decision records:\n- 0002-billing — `.orion/decisions/0002-billing.md`\n- Sessions last 30 days — `.orion/decisions/auth/0001-sessions.md`"), "{ctx}");
        assert!(ctx.ends_with(RECORDS_WIN));
        assert!(
            !ctx.contains(&wt.path().display().to_string()),
            "no absolute path: {ctx}"
        );

        // Records alone, before any harvest has merged.
        std::fs::remove_dir_all(wt.path().join(DIR)).unwrap();
        let ctx = prompt_context(&checkout);
        assert!(ctx.starts_with("Hand-written decision records:"), "{ctx}");
    }

    #[test]
    fn a_builder_is_given_the_areas_and_topics_to_note_rules_on() {
        let wt = tempfile::tempdir().unwrap();
        let checkout = Checkout::Dir(wt.path().to_path_buf());
        let none = topics_catalog(&checkout);
        assert!(
            none.starts_with(
                "The repo's knowledge areas (a topic's area is one of these):\n- editor — "
            ),
            "{none}"
        );
        assert!(none.ends_with("unless none fits):\n(none yet)"), "{none}");
        let store = Store::of(wt.path());
        store
            .write_page(
                "editor/tables",
                "# editor/tables\nScope: tables and their rows\n",
            )
            .unwrap();
        store.write_index("app").unwrap();
        let some = topics_catalog(&checkout);
        assert!(
            some.ends_with("unless none fits):\n- editor/tables — tables and their rows"),
            "{some}"
        );
    }

    #[test]
    fn pages_name_their_repo_relative_path() {
        let (_repo, store) = store_with(&[("auth/login", "# auth/login\n")], &[]);
        let out = pages_for(&store, &["src/auth/login.ts".into()]);
        assert!(
            out.starts_with("<!-- .orion/decisions/harvested/auth/login.md -->\n# auth/login"),
            "{out}"
        );
    }
}
