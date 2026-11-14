//! The READ PATH: what agents get from the store. Every builder prompt
//! carries the index and where the pages are ([`prompt_context`]); `orion
//! knowledge for <paths…>` prints the pages for the files an agent is
//! about to touch ([`pages_for`]).

use std::collections::BTreeSet;
use std::path::Path;

use super::store::{Entry, Store, ACTIVE};

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

/// The store's index for a builder's prompt, with where its topic pages
/// are; empty when `project` (its main checkout) has no store yet.
pub(crate) fn prompt_context(project: &Path) -> String {
    let store = Store::of(project);
    let Some(index) = store.index() else {
        return String::new();
    };
    format!(
        "{}\n\nBefore planning, read the topic pages for the areas you'll touch at {}. Accepted records in docs/decisions/ win over them.",
        index.trim_end(),
        store.dir.display()
    )
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
fn same_files(cited: &str, path: &str) -> bool {
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
                "No topic page matches those paths. The index:\n\n{index}\nPages are at {}.\n",
                store.dir.display()
            ),
            None => format!("No knowledge recorded yet at {}.\n", store.dir.display()),
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
        out.push_str(&format!("<!-- {} -->\n", store.page_path(&topic).display()));
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
    fn the_prompt_carries_the_index_and_the_pages_path() {
        let (repo, store) = store_with(&[("auth/login", "# auth/login\n")], &[]);
        assert_eq!(prompt_context(repo.path()), "");
        store.write_index("app").unwrap();
        let ctx = prompt_context(repo.path());
        assert!(ctx.starts_with("# Knowledge: app\n"));
        assert!(ctx.ends_with(&format!(
            "read the topic pages for the areas you'll touch at {}. Accepted records in docs/decisions/ win over them.",
            store.dir.display()
        )));
    }
}
