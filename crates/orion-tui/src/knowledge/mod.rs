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
//!   rebased on the base before each write, committed on this machine and
//!   pushed with a lease only when the owner signs off a rule review, one
//!   pull request kept open. Never a push to the base.
//! - **Local material** ([`local`]): the condensed dossiers, the queue,
//!   the cards' state and the session snapshot ([`snapshot`]) stay in
//!   `<data_dir>/knowledge/<project id>/`.
//! - **A harvest** ([`harvest`]) runs at merge, queued and one at a time
//!   in the background ([`runner`]): the PR's thread and its sessions'
//!   transcripts, condensed ([`condense`]), read by Opus in one call that
//!   extracts the entries, checks them against the code and the
//!   hand-written records, and writes the topic pages; held to the owner's
//!   words by code ([`guards`]) — then committed and proposed.
//! - **Notes** ([`notes`]): a factory session hands over the rules it
//!   learned with `orion knowledge note` before it finishes; its pull
//!   request's harvest records those instead of extracting, each topic
//!   they touch checked and merged by one call.
//! - **The rule review** ([`review`], drawn by `crate::rule_review`): at
//!   09:00, or the first launch after, every rule waiting opens over orion
//!   — kept, dropped, or noted to be rewritten — and a banner stays until
//!   it is signed off. The sign-off pushes the branch, keeps each drop and
//!   note as a lesson later harvests read, and lets orion merge the pull
//!   request ([`runner`]). Rules that hold repo-wide are offered weekly to
//!   move into the agent docs, through a pull request of their own.
//! - **The read path** ([`read`]): every builder prompt carries the index,
//!   the areas and topics to note rules on, and lists the records, read
//!   from the session's own checkout ([`prompt_context`]); `orion knowledge
//!   for <paths…>` prints the pages for the files an agent will touch.
//! - **The backfill** ([`backfill`]): `orion knowledge backfill`, the
//!   pull requests merged before the snapshot existed, joined to their
//!   transcripts by branch or by the PR links in them.
//! - **Areas and consolidation** ([`areas`], [`consolidate`]): every
//!   topic sits in one of a fixed set of areas, and the topics are merged
//!   down to about 60 by `orion knowledge consolidate`, at the end of a
//!   backfill, and whenever a harvest takes them past that.

pub(crate) mod areas;
pub(crate) mod backfill;
pub(crate) mod condense;
pub(crate) mod consolidate;
pub(crate) mod guards;
pub(crate) mod harvest;
pub(crate) mod local;
pub(crate) mod notes;
pub(crate) mod read;
pub(crate) mod records;
pub(crate) mod review;
pub(crate) mod runner;
pub(crate) mod share;
pub(crate) mod snapshot;
pub(crate) mod store;
pub(crate) mod transcripts;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};

use crate::oneshot::ModelChoice;

pub(crate) use read::prompt_context;
pub(crate) use records::Checkout;
pub(crate) use runner::{merge_landed, on_action, tick};

/// `knowledge` in `config.json`: what each background step asks.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct KnowledgeSettings {
    pub models: KnowledgeModels,
}

/// `knowledge.models`: the model and effort of each step that asks one
/// ([`Step`]). An unset part is the step's default ([`Step::default_model`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct KnowledgeModels {
    /// A merged pull request's harvest, and the check of a factory pull
    /// request's noted rules.
    pub harvest: ModelChoice,
    /// Merging topics down, and their pages into one.
    pub consolidate: ModelChoice,
    /// The weekly re-check of the topics whose files changed.
    pub staleness: ModelChoice,
}

/// A step of the knowledge work that asks a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Harvest,
    Consolidate,
    Staleness,
}

impl Step {
    /// What the step asks at out of the box. The harvest stays on Opus:
    /// cheaper models invent rules, or loosen the owner's.
    pub(crate) fn default_model(self) -> ModelChoice {
        match self {
            Step::Harvest | Step::Staleness => ModelChoice::new("opus", "high"),
            Step::Consolidate => ModelChoice::new("sonnet", "low"),
        }
    }
}

impl KnowledgeModels {
    /// The model and effort `step` asks at: as set, else its default.
    pub(crate) fn of(&self, step: Step) -> ModelChoice {
        let set = match step {
            Step::Harvest => &self.harvest,
            Step::Consolidate => &self.consolidate,
            Step::Staleness => &self.staleness,
        };
        set.or(&step.default_model())
    }
}

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

/// `orion knowledge note <file>`: the rules the session that built this
/// branch's pull request (or `pr`) learned, checked against the areas in
/// the checkout it runs in and kept until the pull request merges
/// ([`notes`]). `-` reads them from stdin.
pub fn cli_note(file: String, pr: Option<String>) -> Result<()> {
    use std::io::Read as _;
    let text = match file.as_str() {
        "-" => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .context("reading the notes from stdin")?;
            text
        }
        path => std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?,
    };
    let cwd = std::env::current_dir().context("no current directory")?;
    let areas = match crate::git_proc::read(&cwd, &["rev-parse", "--show-toplevel"]) {
        Ok(top) => areas::Areas::load(&store::Store::of(Path::new(top.trim()))),
        Err(_) => areas::Areas::default(),
    };
    let entries = notes::parse(&text, &areas).map_err(|problems| {
        anyhow!(
            "the notes can't be recorded:\n  - {}",
            problems.join("\n  - ")
        )
    })?;
    let url = crate::pr_body::cli::pr_url(pr.as_deref())?;
    let noted = entries.len();
    let total = notes::add(&notes::dir(), &url, entries).context("saving the notes")?;
    match noted {
        0 => println!("nothing to record, noted for {url}"),
        n => println!(
            "{n} rule{} noted for {url} ({total} in all); orion records them when it merges",
            if n == 1 { "" } else { "s" }
        ),
    }
    Ok(())
}

/// The project `project_name` names, as orion writes its knowledge:
/// known to orion, its `.orion/decisions/` not ignored.
fn sharer_for(project_name: &str) -> Result<(share::Sharer<'static>, local::Local)> {
    let Known { root, name, id } = project(Some(project_name))?;
    let Some(id) = id else {
        bail!(
            "orion hasn't opened {name} ({}) yet — add it as a project in orion first",
            root.display()
        );
    };
    if store::ignored(&root) {
        bail!("{name} ignores .orion/decisions/, so harvested rules can't be shared — take it out of its .gitignore first");
    }
    let local = local::Local::of(&id);
    let share = share::Sharer {
        git: &share::SystemGit,
        gh: &share::GhCli,
        main: root.clone(),
        local: local.clone(),
        project: name,
        base: runtime()?.block_on(share::base_of(&root, &share::GhCli)),
    };
    Ok((share, local))
}

/// A runtime for a knowledge command.
fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

/// `orion knowledge consolidate --project <name>`: merge the project's
/// harvested topics down to about [`consolidate::TOPIC_CAP`], and propose
/// that in the knowledge pull request as one commit.
pub fn cli_consolidate(project_name: String) -> Result<()> {
    let (share, local) = sharer_for(&project_name)?;
    let model = harvest::ClaudeCli::from_config().map_err(|why| anyhow!(why))?;
    eprintln!(
        "Consolidating {}'s topics ({}), into a pull request to {}…",
        share.project,
        share.main.display(),
        share.base
    );
    let (done, published, index_lines, branch_name) = runtime()?
        .block_on(async {
            let branch = share.open().await?;
            let store = branch.store();
            let records = records::for_check_in(&branch.clone);
            let to = harvest::Target {
                store: &store,
                local: &share.local,
                project: &share.project,
                records: &records,
            };
            let done = consolidate::consolidate(&model, &to).await?;
            let lines = store
                .index()
                .map_or(0, |i| store::marked(&i).lines().count());
            let published = share.publish(&branch, consolidate::MESSAGE).await?;
            let name = branch.name.clone();
            drop(branch);
            Ok::<_, String>((done, published, lines, name))
        })
        .map_err(|why| anyhow!(why))?;
    println!("{}", done.line());
    println!("index.md: {index_lines} lines.");
    if let Some(pr) = &published.pr {
        println!("Proposed in #{}: {}", pr.number, pr.url);
    }
    runner::note_published(&local, &published);
    review::after_cli_push(&local, &share.base, &branch_name, &[], &published.head);
    Ok(())
}

/// `orion knowledge backfill --project <name> --since <date>`: harvest
/// the pull requests merged since `since` into the knowledge pull
/// request, then check every topic against the code. Needs no TUI; a
/// running one posts the summary card.
pub fn cli_backfill(project_name: String, since: String) -> Result<()> {
    chrono::NaiveDate::parse_from_str(&since, "%Y-%m-%d")
        .map_err(|_| anyhow!("--since takes a date as YYYY-MM-DD, not {since}"))?;
    let (share, local) = sharer_for(&project_name)?;
    let model = harvest::ClaudeCli::from_config().map_err(|why| anyhow!(why))?;
    eprintln!(
        "Backfilling {} ({}) from {since}, into a pull request to {}…",
        share.project,
        share.main.display(),
        share.base
    );
    let runtime = runtime()?;
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
        // A backfill's rules skip the review: you ran it.
        let prs: Vec<u64> = summary.harvested.iter().map(|o| o.pr).collect();
        if let Some(login) = runtime.block_on(runner::gh_login()) {
            let branch = share::branch_for(&login);
            review::after_cli_push(&local, &share.base, &branch, &prs, &summary.head);
        }
    }
    runner::notify(&local, runner::Notice::Backfilled { lines });
    Ok(())
}
