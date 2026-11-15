//! BACKFILL: `orion knowledge backfill --project <name> --since <date>`,
//! the one-off harvest of the pull requests merged before orion kept a
//! snapshot. Each merged PR is joined to its transcripts by branch or by
//! the PR URLs they mention ([`super::transcripts::join`]) and harvested
//! oldest first, as at merge, into the knowledge branch — one commit a
//! batch of [`BATCH`], pushed and proposed in the same pull request as
//! the harvests at merge (`super::share`); then every topic gets a
//! staleness pass against the code as origin's base has it now (a
//! temporary checkout, `harvest::CheckTree`; with none, the pass is
//! skipped), committed the same way. Runs from the CLI, with no TUI.

use std::path::Path;

use serde_json::Value;

use super::condense;
use super::harvest::{self, Model, Outcome, Session, Target};
use super::records;
use super::share::{Gh, Published, Sharer};
use super::transcripts::{self, Found, Merged, Scan};

/// Branches everyone's work passes through: a session on one isn't a
/// pull request's own.
const BASES: &[&str] = &[
    "main", "master", "dev", "develop", "staging", "trunk", "HEAD",
];
/// How many pull requests one commit of a backfill holds.
pub(crate) const BATCH: usize = 10;

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
    /// The knowledge pull request the harvests went to, and the head
    /// pushed to it last.
    pub pr: Option<super::share::Pr>,
    pub head: String,
}

impl Summary {
    /// Where a publish left the pull request.
    fn published(&mut self, published: Published) {
        if let Some(pr) = published.pr {
            self.pr = Some(pr);
            self.head = published.head;
        }
    }
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
        if let Some(pr) = &self.pr {
            lines.push(format!("Proposed in #{}: {}", pr.number, pr.url));
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
    harvested: &dyn Fn(u64) -> bool,
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
        if harvested(pr.number) {
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
async fn repo_slug(gh: &dyn Gh, root: &Path) -> Result<String, String> {
    let url = gh
        .run(root, &["repo", "view", "--json", "url", "--jq", ".url"])
        .await
        .map_err(|why| format!("gh couldn't say which GitHub repo this is: {why}"))?;
    let url = url.trim();
    Ok(url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .to_string())
}

/// Backfill the project `share` writes for from the pull requests
/// merged since `since` (`YYYY-MM-DD`), saying how it goes on `say`.
pub(crate) async fn run(
    model: &dyn Model,
    share: &Sharer<'_>,
    since: &str,
    say: &mut (dyn FnMut(&str) + Send),
) -> Result<Summary, String> {
    let root = share.main.as_path();
    let gh = share.gh;
    let search = format!("merged:>={since}");
    let list = gh
        .run(
            root,
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
        )
        .await
        .map_err(|why| format!("gh couldn't list the merged pull requests: {why}"))?;
    let prs = read_list(&list);
    say(&format!(
        "{} pull requests merged since {since}.",
        prs.len()
    ));

    let slug = repo_slug(gh, root).await?;
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

    let (todo, skipped) = {
        let branch = share.open().await?;
        let store = branch.store();
        let harvested =
            |pr: u64| share.local.has_source(pr) || store.entries().iter().any(|e| e.pr == pr);
        plan(&harvested, &prs, &scans, &bases(root))
    };
    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    say(&format!("{} to harvest.", todo.len()));
    let mut done = 0;
    for batch in todo.chunks(BATCH) {
        let branch = share.open().await?;
        let store = branch.store();
        let records = records::for_check_in(&branch.clone);
        let to = Target {
            store: &store,
            local: &share.local,
            project: &share.project,
            records: &records,
        };
        let mut numbers = Vec::new();
        for (pr, found) in batch {
            done += 1;
            say(&format!(
                "[{done}/{}] #{} ({} transcripts)…",
                todo.len(),
                pr.number,
                found.len()
            ));
            let (info, comments) = match harvest::gather_pr(gh, root, pr.number).await {
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
                harvest::harvest_merge(model, &to, root, &info, &comments, &sessions).await;
            if let Some(why) = unchecked {
                say(&format!("  not checked against the code: {why}"));
            }
            match outcome {
                Ok(outcome) => {
                    say(&format!(
                        "  {} rules kept, {} dropped",
                        outcome.kept, outcome.dropped
                    ));
                    numbers.push(format!("#{}", outcome.pr));
                    summary.harvested.push(outcome);
                }
                Err(why) => {
                    say(&format!("  failed: {why}"));
                    summary.failed.push((pr.number, why));
                }
            }
        }
        if !numbers.is_empty() {
            let message = format!("knowledge: backfill {}", numbers.join(", "));
            let published = share.publish(&branch, &message).await?;
            say_published(say, &published);
            summary.published(published);
        }
    }
    say("Staleness pass over every topic…");
    match harvest::CheckTree::of_base(root).await {
        Ok(tree) => {
            let stale = async {
                let branch = share.open().await?;
                let store = branch.store();
                let records = records::for_check_in(&branch.clone);
                let to = Target {
                    store: &store,
                    local: &share.local,
                    project: &share.project,
                    records: &records,
                };
                let stale = harvest::staleness(model, &to, &tree.path).await?;
                let published = share.publish(&branch, "knowledge: staleness check").await?;
                Ok::<_, String>((stale, published))
            }
            .await;
            tree.remove().await;
            let (stale, published) = stale?;
            say_published(say, &published);
            summary.published(published);
            summary.stale = stale;
        }
        Err(why) => say(&format!("  skipped, no checkout to read: {why}")),
    }
    Ok(summary)
}

/// Where a batch went.
fn say_published(say: &mut (dyn FnMut(&str) + Send), published: &Published) {
    if let Some(pr) = &published.pr {
        say(&format!("  proposed in #{} {}", pr.number, pr.url));
    }
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
        let harvested = |pr: u64| pr == 4;
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
        let (todo, skipped) = plan(&harvested, &prs, &scans, &bases(repo.path()));
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
