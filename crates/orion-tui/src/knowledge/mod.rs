//! KNOWLEDGE: the decisions the owner made while agents built his pull
//! requests, recorded at merge so later agents work his way without
//! asking him again.
//!
//! - **The store** ([`store`]) is `<project main checkout>/.orion/knowledge/`,
//!   a git repository of its own that only orion writes: an index, one
//!   page per topic, every entry in `entries.jsonl`, and the dossier each
//!   pull request's entries came from.
//! - **The snapshot** ([`snapshot`]) keeps which sessions sat on which
//!   branch, so their transcripts can be found after the worktree goes.
//! - **A harvest** ([`harvest`]) runs at merge, queued and one at a time
//!   in the background ([`runner`]): the PR's thread and its sessions'
//!   transcripts, condensed ([`condense`]), read by Opus for entries, held
//!   to the owner's words by code ([`guards`]), checked against the code
//!   and merged into topic pages, then committed.
//! - **The daily card** at 18:30 lists the rules recorded as the owner's
//!   that day, each with a veto ([`runner`]).
//! - **The read path** ([`read`]): every builder prompt carries the index
//!   ([`prompt_context`]); `orion knowledge for <paths…>` prints the pages
//!   for the files an agent will touch.
//! - **The backfill** ([`backfill`]): `orion knowledge backfill`, the
//!   pull requests merged before the snapshot existed, joined to their
//!   transcripts by branch or by the PR links in them.

pub(crate) mod backfill;
pub(crate) mod condense;
pub(crate) mod guards;
pub(crate) mod harvest;
pub(crate) mod read;
pub(crate) mod runner;
pub(crate) mod snapshot;
pub(crate) mod store;
pub(crate) mod transcripts;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};

pub(crate) use read::prompt_context;
pub(crate) use runner::{merge_landed, on_action, tick};

/// The main checkout of the repo `dir` is in (a worktree's included).
fn main_checkout(dir: &Path) -> Result<PathBuf> {
    let common = runner::common_dir(dir)
        .map_err(|why| anyhow!("{} is not in a git repository: {why}", dir.display()))?;
    Ok(match common.file_name().is_some_and(|n| n == ".git") {
        true => common.parent().unwrap_or(&common).to_path_buf(),
        false => common,
    })
}

/// The project `name` names — a project orion knows, or a directory —
/// else the one the current directory is in: its main checkout, and the
/// name its store is indexed under.
fn project(name: Option<&str>) -> Result<(PathBuf, String)> {
    let snapshot = snapshot::Snapshot::load();
    let named = |root: &Path| {
        snapshot
            .projects
            .iter()
            .find(|(_, r)| r.as_path() == root)
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| orion_core::Project::folder_name(root))
    };
    let dir = match name {
        None => std::env::current_dir().context("no current directory")?,
        Some(name) => {
            if let Some(root) = snapshot.root_of(name) {
                return Ok((root, name.to_string()));
            }
            let cfg = crate::config::Config::load();
            if let Some(root) = cfg
                .projects
                .keys()
                .find(|root| orion_core::Project::folder_name(root) == name)
            {
                return Ok((root.clone(), name.to_string()));
            }
            let dir = PathBuf::from(name);
            if !dir.is_dir() {
                let mut known: Vec<String> = snapshot.projects.keys().cloned().collect();
                known.extend(
                    cfg.projects
                        .keys()
                        .map(|r| orion_core::Project::folder_name(r)),
                );
                known.sort();
                known.dedup();
                match known.is_empty() {
                    true => bail!("no project called {name} — pass its directory"),
                    false => bail!(
                        "no project called {name} — pass one of {}, or a directory",
                        known.join(", ")
                    ),
                }
            }
            dir
        }
    };
    let root = main_checkout(&dir)?;
    let name = named(&root);
    Ok((root, name))
}

/// `orion knowledge for <paths…>`: the topic pages for the files at
/// `paths` (relative to the current directory, or the repo), from the
/// store of the project they are in.
pub fn cli_for(paths: Vec<String>, project_name: Option<String>) -> Result<()> {
    let (root, _) = project(project_name.as_deref())?;
    let cwd = std::env::current_dir().context("no current directory")?;
    let top = crate::git_proc::read(&cwd, &["rev-parse", "--show-toplevel"])
        .map(|t| PathBuf::from(t.trim()))
        .unwrap_or_else(|_| root.clone());
    let relative: Vec<String> = paths
        .iter()
        .map(|p| {
            let abs = cwd.join(p);
            let abs = abs.canonicalize().unwrap_or(abs);
            let top = top.canonicalize().unwrap_or_else(|_| top.clone());
            abs.strip_prefix(&top)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.clone())
        })
        .collect();
    print!("{}", read::pages_for(&store::Store::of(&root), &relative));
    Ok(())
}

/// `orion knowledge backfill --project <name> --since <date>`: harvest
/// the pull requests merged since `since`, then check every topic
/// against the code. Needs no TUI; a running one posts the summary card.
pub fn cli_backfill(project_name: String, since: String) -> Result<()> {
    chrono::NaiveDate::parse_from_str(&since, "%Y-%m-%d")
        .map_err(|_| anyhow!("--since takes a date as YYYY-MM-DD, not {since}"))?;
    let (root, name) = project(Some(&project_name))?;
    let model = harvest::ClaudeCli::from_config().map_err(|why| anyhow!(why))?;
    eprintln!("Backfilling {name} ({}) from {since}…", root.display());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut say = |line: &str| eprintln!("{line}");
    let summary = runtime
        .block_on(backfill::run(&model, &root, &name, &since, &mut say))
        .map_err(|why| anyhow!(why))?;
    let lines = summary.lines();
    for line in &lines {
        println!("{line}");
    }
    println!("Store: {}", store::Store::of(&root).dir.display());
    if !store::ignored(&root) {
        println!(
            "Note: {} doesn't ignore .orion/knowledge/ — add it to .gitignore.",
            root.display()
        );
    }
    runner::notify(runner::Notice::Backfilled {
        project: name,
        lines,
    });
    Ok(())
}
