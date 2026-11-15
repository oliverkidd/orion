//! KNOWLEDGE: the decisions the owner made while agents built his pull
//! requests, recorded at merge so later agents work his way without
//! asking him again — committed in the repo, beside the records people
//! write by hand.
//!
//! - **The harvested pages** ([`store`]) are `.orion/decisions/harvested/`
//!   in the repo: an index, one page per topic, and every entry in
//!   `entries.jsonl`. orion writes nothing else; every other file under
//!   `.orion/decisions/` is a hand-written record ([`records`]),
//!   authoritative and read-only to orion.
//! - **Sharing** ([`share`]): orion writes through a pull request — its
//!   own working clone, one `orion/knowledge-<login>` branch a person,
//!   rebased on the base before each write, pushed with a lease, one pull
//!   request kept open. Never a push to the base.
//! - **Local material** ([`local`]): the condensed dossiers, the queue,
//!   the cards' state and the session snapshot ([`snapshot`]) stay in
//!   `<data_dir>/knowledge/<project id>/`.
//! - **A harvest** ([`harvest`]) runs at merge, queued and one at a time
//!   in the background ([`runner`]): the PR's thread and its sessions'
//!   transcripts, condensed ([`condense`]), read by Opus for entries, held
//!   to the owner's words by code ([`guards`]), checked against the code
//!   and the hand-written records, and merged into topic pages — then
//!   committed and proposed.
//! - **The daily card** at 18:30 lists the rules recorded as the owner's
//!   that day, each with a veto, and links the pull request; after its
//!   veto window orion merges it ([`runner`]).
//! - **The read path** ([`read`]): every builder prompt carries the index
//!   and lists the records, read from the session's own checkout
//!   ([`prompt_context`]); `orion knowledge for <paths…>` prints the pages
//!   for the files an agent will touch.
//! - **The backfill** ([`backfill`]): `orion knowledge backfill`, the
//!   pull requests merged before the snapshot existed, joined to their
//!   transcripts by branch or by the PR links in them.

pub(crate) mod backfill;
pub(crate) mod condense;
pub(crate) mod guards;
pub(crate) mod harvest;
pub(crate) mod local;
pub(crate) mod read;
pub(crate) mod records;
pub(crate) mod runner;
pub(crate) mod share;
pub(crate) mod snapshot;
pub(crate) mod store;
pub(crate) mod transcripts;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};

pub(crate) use read::prompt_context;
pub(crate) use records::Checkout;
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

/// A project orion has seen: its main checkout, name and id.
struct Known {
    root: PathBuf,
    name: String,
    id: Option<String>,
}

/// The project `name` names — a project orion knows, or a directory —
/// else the one the current directory is in.
fn project(name: Option<&str>) -> Result<Known> {
    let known = snapshot::known();
    let by_root = |root: &Path| {
        known
            .iter()
            .find(|(_, s)| s.root.as_path() == root)
            .map(|(local, s)| Known {
                root: root.to_path_buf(),
                name: s.name.clone(),
                id: Some(local.id.clone()),
            })
            .unwrap_or_else(|| Known {
                root: root.to_path_buf(),
                name: orion_core::Project::folder_name(root),
                id: None,
            })
    };
    let dir = match name {
        None => std::env::current_dir().context("no current directory")?,
        Some(name) => {
            if let Some((local, s)) = known.iter().find(|(_, s)| s.name == name) {
                return Ok(Known {
                    root: s.root.clone(),
                    name: name.to_string(),
                    id: Some(local.id.clone()),
                });
            }
            let cfg = crate::config::Config::load();
            if let Some(root) = cfg
                .projects
                .keys()
                .find(|root| orion_core::Project::folder_name(root) == name)
            {
                return Ok(by_root(root));
            }
            let dir = PathBuf::from(name);
            if !dir.is_dir() {
                let mut names: Vec<String> = known.iter().map(|(_, s)| s.name.clone()).collect();
                names.extend(
                    cfg.projects
                        .keys()
                        .map(|r| orion_core::Project::folder_name(r)),
                );
                names.sort();
                names.dedup();
                match names.is_empty() {
                    true => bail!("no project called {name} — pass its directory"),
                    false => bail!(
                        "no project called {name} — pass one of {}, or a directory",
                        names.join(", ")
                    ),
                }
            }
            dir
        }
    };
    Ok(by_root(&main_checkout(&dir)?))
}

/// `orion knowledge for <paths…>`: the harvested pages for the files at
/// `paths` (relative to the current directory, or the repo), read from
/// the checkout the command runs in — or, named, the project's main
/// checkout.
pub fn cli_for(paths: Vec<String>, project_name: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir().context("no current directory")?;
    let here = crate::git_proc::read(&cwd, &["rev-parse", "--show-toplevel"])
        .map(|t| PathBuf::from(t.trim()));
    let top = match (&project_name, here) {
        (None, Ok(top)) => top,
        (None, Err(_)) => bail!("{} is not in a git repository", cwd.display()),
        (Some(name), _) => project(Some(name))?.root,
    };
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
    print!("{}", read::pages_for(&store::Store::of(&top), &relative));
    Ok(())
}

/// `orion knowledge backfill --project <name> --since <date>`: harvest
/// the pull requests merged since `since` into the knowledge pull
/// request, then check every topic against the code. Needs no TUI; a
/// running one posts the summary card.
pub fn cli_backfill(project_name: String, since: String) -> Result<()> {
    chrono::NaiveDate::parse_from_str(&since, "%Y-%m-%d")
        .map_err(|_| anyhow!("--since takes a date as YYYY-MM-DD, not {since}"))?;
    let Known { root, name, id } = project(Some(&project_name))?;
    let Some(id) = id else {
        bail!(
            "orion hasn't opened {name} ({}) yet — add it as a project in orion first",
            root.display()
        );
    };
    if store::ignored(&root) {
        bail!("{name} ignores .orion/decisions/, so harvested rules can't be shared — take it out of its .gitignore first");
    }
    let model = harvest::ClaudeCli::from_config().map_err(|why| anyhow!(why))?;
    let local = local::Local::of(&id);
    let share = share::Sharer {
        git: &share::SystemGit,
        gh: &share::GhCli,
        main: root.clone(),
        local: local.clone(),
        project: name.clone(),
        base: share::base_of(&root),
    };
    eprintln!(
        "Backfilling {name} ({}) from {since}, into a pull request to {}…",
        root.display(),
        share.base
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut say = |line: &str| eprintln!("{line}");
    let summary = runtime
        .block_on(backfill::run(&model, &share, &since, &mut say))
        .map_err(|why| anyhow!(why))?;
    let lines = summary.lines();
    for line in &lines {
        println!("{line}");
    }
    if let Some(pr) = &summary.pr {
        runner::note_published(
            &local,
            &share::Published {
                pr: Some(pr.clone()),
                head: summary.head.clone(),
                pushed: true,
            },
        );
    }
    runner::notify(&local, runner::Notice::Backfilled { lines });
    Ok(())
}
