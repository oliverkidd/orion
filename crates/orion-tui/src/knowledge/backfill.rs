//! BACKFILL: `orion knowledge backfill --project <name> --since <date>`,
//! the one-off harvest of the pull requests merged before orion kept a
//! snapshot. Each merged PR is joined to its transcripts by branch or by
//! the PR URLs they mention ([`super::transcripts::join`]) and harvested
//! oldest first, as at merge; then every topic gets a staleness pass
//! against the code as origin's base has it now (a temporary checkout,
//! `harvest::CheckTree`; with none, the pass is skipped). Runs from the
//! CLI, with no TUI.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::condense;
use super::harvest::{self, Model, Outcome, Session};
use super::store::Store;
use super::transcripts::{self, Found, Merged, Scan};

/// Branches everyone's work passes through: a session on one isn't a
/// pull request's own.
const BASES: &[&str] = &[
    "main", "master", "dev", "develop", "staging", "trunk", "HEAD",
];
const GH_TIMEOUT: Duration = Duration::from_secs(120);

/// A pull request to harvest, and its transcripts.
pub(crate) type Todo = (Listed, Vec<Found>);

/// A merged pull request, as `gh pr list` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    pub number: u64,
    pub branch: String,
    pub merged_at: String,
}

/// What a backfill did, for its summary.
#[derive(Debug, Default)]
pub(crate) struct Summary {
    pub harvested: Vec<Outcome>,
    /// `(pr, why)`: harvested before, or nothing to read.
    pub skipped: Vec<(u64, String)>,
    pub failed: Vec<(u64, String)>,
    /// Topics the staleness pass changed.
    pub stale: Vec<String>,
}

impl Summary {
    /// The summary, a line each.
    pub(crate) fn lines(&self) -> Vec<String> {
        let kept: usize = self.harvested.iter().map(|o| o.kept).sum();
        let mut lines = vec![format!(
            "{} pull requests harvested, {kept} rules recorded; {} skipped, {} failed.",
            self.harvested.len(),
            self.skipped.len(),
            self.failed.len()
        )];
        for (pr, why) in &self.failed {
            lines.push(format!("#{pr} failed: {why}"));
        }
        if !self.stale.is_empty() {
            lines.push(format!("Staleness pass changed: {}", self.stale.join(", ")));
        }
        lines
    }
}

/// `gh pr list`'s merged pull requests as [`Listed`], oldest first.
pub(crate) fn read_list(json: &str) -> Vec<Listed> {
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    let mut prs: Vec<Listed> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Listed {
            number: crate::pull_request::u64_at(p, "number"),
            branch: crate::pull_request::str_at(p, "headRefName"),
            merged_at: crate::pull_request::str_at(p, "mergedAt"),
        })
        .filter(|p| p.number > 0)
        .collect();
    prs.sort_by(|a, b| a.merged_at.cmp(&b.merged_at).then(a.number.cmp(&b.number)));
    prs
}

/// The pull requests to harvest, oldest first, each with its transcripts:
/// every listed one not harvested yet that has any.
pub(crate) fn plan(
    store: &Store,
    prs: &[Listed],
    scans: &[Scan],
    bases: &[String],
) -> (Vec<Todo>, Vec<(u64, String)>) {
    let merged: Vec<Merged> = prs
        .iter()
        .map(|p| Merged {
            number: p.number,
            branch: p.branch.clone(),
        })
        .collect();
    let mut joined = transcripts::join(&merged, scans, bases);
    let mut todo = Vec::new();
    let mut skipped = Vec::new();
    for pr in prs {
        if store.harvested(pr.number) {
            skipped.push((pr.number, "harvested before".into()));
            continue;
        }
        match joined.remove(&pr.number) {
            Some(found) => todo.push((pr.clone(), found)),
            None => skipped.push((pr.number, "no transcripts".into())),
        }
    }
    (todo, skipped)
}

/// `found`, read and condensed, under its heading.
pub(crate) fn session_of(found: &Found) -> Option<Session> {
    let text = std::fs::read_to_string(&found.path).ok()?;
    let lines = condense::condense(found.shape, &text);
    let shape = match found.shape {
        condense::Shape::Claude => "claude",
        condense::Shape::Cursor => "cursor",
    };
    (!lines.is_empty()).then(|| Session {
        heading: format!("Session {} ({shape})", found.session()),
        lines,
    })
}

/// The base branches of the repo at `root`: the usual names and origin's
/// default.
pub(crate) fn bases(root: &Path) -> Vec<String> {
    let mut bases: Vec<String> = BASES.iter().map(|b| b.to_string()).collect();
    if let Ok(head) = crate::git_proc::read(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(name) = head.trim().strip_prefix("origin/") {
            bases.push(name.to_string());
        }
    }
    bases
}

/// `github.com/owner/name` for the repo at `root`.
async fn repo_slug(root: &Path) -> Result<String, String> {
    let url = crate::pull_request::gh(
        Some(root),
        &["repo", "view", "--json", "url", "--jq", ".url"],
        GH_TIMEOUT,
    )
    .await
    .ok_or("gh couldn't say which GitHub repo this is")?;
    let url = url.trim();
    Ok(url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .to_string())
}

/// Backfill project `project` (main checkout `root`) from the pull
/// requests merged since `since` (`YYYY-MM-DD`), saying how it goes on
/// `say`.
pub(crate) async fn run(
    model: &dyn Model,
    root: &Path,
    project: &str,
    since: &str,
    say: &mut (dyn FnMut(&str) + Send),
) -> Result<Summary, String> {
    let store = Store::of(root);
    store.init()?;
    let search = format!("merged:>={since}");
    let list = crate::pull_request::gh(
        Some(root),
        &[
            "pr",
            "list",
            "--state",
            "merged",
            "--search",
            &search,
            "--limit",
            "1000",
            "--json",
            "number,headRefName,mergedAt",
        ],
        GH_TIMEOUT,
    )
    .await
    .ok_or("gh couldn't list the merged pull requests")?;
    let prs = read_list(&list);
    say(&format!(
        "{} pull requests merged since {since}.",
        prs.len()
    ));

    let slug = repo_slug(root).await?;
    let folder = orion_core::Project::folder_name(root);
    let found = transcripts::list(
        &transcripts::claude_roots(),
        transcripts::cursor_root().as_deref(),
        &folder,
    );
    say(&format!(
        "Reading {} transcripts for their branches and PR links…",
        found.len()
    ));
    let scans = tokio::task::spawn_blocking(move || {
        found
            .iter()
            .map(|f| transcripts::scan(f, &slug))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| e.to_string())?;

    let (todo, skipped) = plan(&store, &prs, &scans, &bases(root));
    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    say(&format!("{} to harvest.", todo.len()));
    for (n, (pr, found)) in todo.iter().enumerate() {
        say(&format!(
            "[{}/{}] #{} ({} transcripts)…",
            n + 1,
            todo.len(),
            pr.number,
            found.len()
        ));
        let gathered = harvest::gather_pr(root, pr.number).await;
        let (info, comments) = match gathered {
            Ok(g) => g,
            Err(why) => {
                summary.failed.push((pr.number, why));
                continue;
            }
        };
        let found = found.clone();
        let sessions = tokio::task::spawn_blocking(move || {
            found.iter().filter_map(session_of).collect::<Vec<_>>()
        })
        .await
        .map_err(|e| e.to_string())?;
        let (outcome, unchecked) =
            harvest::harvest_merge(model, &store, project, root, &info, &comments, &sessions).await;
        if let Some(why) = unchecked {
            say(&format!("  not checked against the code: {why}"));
        }
        match outcome {
            Ok(outcome) => {
                say(&format!(
                    "  {} rules kept, {} dropped",
                    outcome.kept, outcome.dropped
                ));
                summary.harvested.push(outcome);
            }
            Err(why) => {
                say(&format!("  failed: {why}"));
                summary.failed.push((pr.number, why));
            }
        }
    }
    say("Staleness pass over every topic…");
    match harvest::CheckTree::of_base(root).await {
        Ok(tree) => {
            let stale = harvest::staleness(model, &store, project, &tree.path).await;
            tree.remove().await;
            summary.stale = stale?;
        }
        Err(why) => say(&format!("  skipped, no checkout to read: {why}")),
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::condense::Shape;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    fn scan(path: &str, shape: Shape, branches: &[&str], mentions: &[(u64, usize)]) -> Scan {
        Scan {
            found: Found {
                shape,
                path: PathBuf::from(path),
            },
            branches: branches
                .iter()
                .map(|b| b.to_string())
                .collect::<BTreeSet<_>>(),
            mentions: mentions.iter().copied().collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn the_list_is_read_oldest_first() {
        let json = r#"[{"number": 9, "headRefName": "b", "mergedAt": "2026-07-02T00:00:00Z"},
                       {"number": 4, "headRefName": "a", "mergedAt": "2026-06-30T00:00:00Z"}]"#;
        let prs = read_list(json);
        assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), vec![4, 9]);
    }

    #[test]
    fn the_plan_skips_what_was_harvested_and_what_has_no_transcripts() {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        store.init().unwrap();
        store.write_source(4, "done").unwrap();
        let prs = read_list(
            r#"[{"number": 4, "headRefName": "a", "mergedAt": "1"},
                {"number": 5, "headRefName": "b", "mergedAt": "2"},
                {"number": 6, "headRefName": "dev", "mergedAt": "3"},
                {"number": 7, "headRefName": "c", "mergedAt": "4"}]"#,
        );
        let scans = vec![
            scan("/t/s1.jsonl", Shape::Claude, &["a", "b"], &[]),
            scan("/t/s2.jsonl", Shape::Claude, &["dev"], &[(7, 3)]),
            scan("/t/c1.jsonl", Shape::Cursor, &[], &[(5, 2), (7, 1)]),
        ];
        let (todo, skipped) = plan(&store, &prs, &scans, &bases(repo.path()));
        let todo: Vec<(u64, Vec<String>)> = todo
            .iter()
            .map(|(p, f)| (p.number, f.iter().map(Found::session).collect()))
            .collect();
        assert_eq!(
            todo,
            vec![(5, vec!["s1".into(), "c1".into()]), (7, vec!["s2".into()])]
        );
        assert_eq!(
            skipped,
            vec![(4, "harvested before".into()), (6, "no transcripts".into())]
        );
    }

    #[test]
    fn a_found_transcript_condenses_by_its_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c9.jsonl");
        std::fs::write(&path, condense::tests::CURSOR).unwrap();
        let s = session_of(&Found {
            shape: Shape::Cursor,
            path,
        })
        .unwrap();
        assert_eq!(s.heading, "Session c9 (cursor)");
        assert!(s.lines[0]
            .render()
            .starts_with("OWNER: Use the shared date picker"));
    }
}
