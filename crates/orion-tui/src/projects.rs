//! A project's WHAT IT IS: one line saying what the project is, which
//! routing reads — meeting takeaways and Linear tickets go to the project
//! whose line fits them. [`about`] is the one way to read it.
//!
//! orion writes it when a project is added ([`added`]), and for a project
//! from before on the first [`about`] that finds none: one
//! [`crate::oneshot::run`] on the default Claude account, at haiku and low
//! effort, from the head of the README and the last 20 merged pull
//! request titles ([`prompt`]). It is kept in `projects.<repo>.about`, and
//! Settings › Projects › **What it is** shows it: Enter edits it, `^R`
//! writes it again ([`rewrite`]).
//!
//! The e2e tests set `ORION_PROJECT_ABOUT=off`, so adding a project there
//! runs no model.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::app::App;
use crate::config::Config;

/// The model and effort the line is written at: cheap, it is one line.
const MODEL: &str = "haiku";
const EFFORT: &str = "low";
/// How long the model may take.
const TIMEOUT: Duration = Duration::from_secs(90);
/// How much of the README goes in.
const README_LINES: usize = 60;
const README_CHARS: usize = 4000;
/// How many merged pull request titles go in.
const PR_TITLES: usize = 20;
/// The most of a reply kept.
const LINE_MAX: usize = 240;

/// The row's value while nothing is written.
pub const NOT_WRITTEN: &str = "not written yet";
/// …and while it is being written.
pub const WRITING: &str = "writing…";

const SYSTEM: &str = "You write one line saying what a software project is. Orion, a \
developer's workspace, reads it to decide which project a meeting takeaway or a ticket \
belongs to, so name what the product does, who it is for and its main parts. One plain \
sentence under 30 words: no preamble, no quotes, no markdown.";

/// What-it-is writes on the app.
#[derive(Debug, Default)]
pub struct State {
    /// Where the writes land: the loop's channel.
    pub tx: Option<UnboundedSender<Answer>>,
    /// Projects whose line is being written.
    pub writing: HashSet<PathBuf>,
    /// Projects [`about`] has already started a write for this run, so a
    /// write that failed isn't asked again on every read.
    tried: HashSet<PathBuf>,
}

/// A write, landed.
#[derive(Debug)]
pub struct Answer {
    pub repo: PathBuf,
    pub line: Result<String, String>,
    /// `^R` asked for it: its failure is said.
    pub by_hand: bool,
}

/// What the project at `repo` is, as one line: the line kept for it, or
/// empty while there is none — in which case one is written, once a run,
/// for the next read.
pub fn about(app: &mut App, repo: &Path) -> String {
    let line = Config::load().project(repo).about;
    if line.is_empty() && app.project_about.tried.insert(repo.to_path_buf()) {
        spawn(app, repo, false);
    }
    line
}

/// A project was just added: write its line, unless it has one.
pub fn added(app: &mut App, repo: &Path) {
    if Config::load().project(repo).about.is_empty() {
        app.project_about.tried.insert(repo.to_path_buf());
        spawn(app, repo, false);
    }
}

/// `^R` on **What it is**: write it again.
pub fn rewrite(app: &mut App, repo: &Path) -> &'static str {
    if off() {
        return "writing What it is is off here";
    }
    if app.project_about.writing.contains(repo) {
        return WRITING;
    }
    spawn(app, repo, true);
    WRITING
}

/// The **What it is** row's value.
pub fn label(app: &App, cfg: &Config, repo: &Path) -> String {
    if app.project_about.writing.contains(repo) {
        return WRITING.into();
    }
    match cfg.project(repo).about {
        line if line.is_empty() => NOT_WRITTEN.into(),
        line => line,
    }
}

/// `ORION_PROJECT_ABOUT=off`: no model runs for it.
fn off() -> bool {
    orion_core::env::non_empty(orion_core::env::PROJECT_ABOUT).as_deref() == Some("off")
}

fn spawn(app: &mut App, repo: &Path, by_hand: bool) {
    let Some(tx) = app.project_about.tx.clone() else {
        return;
    };
    if off() || !app.project_about.writing.insert(repo.to_path_buf()) {
        return;
    }
    app.dirty = true;
    let repo = repo.to_path_buf();
    tokio::spawn(async move {
        let line = write(&Config::load(), &repo).await;
        let _ = tx.send(Answer {
            repo,
            line,
            by_hand,
        });
    });
}

/// Write the line for the project at `repo` and hand it back, keeping
/// nothing: what [`spawn`] runs, and what a caller that must have the line
/// before it goes on (meeting takeaways) awaits.
pub async fn write(cfg: &Config, repo: &Path) -> Result<String, String> {
    let account = crate::oneshot::default_account(cfg)
        .ok_or_else(|| "no Claude account is switched on".to_string())?;
    let readme = readme_head(repo);
    let titles = merged_titles(repo).await;
    let name = orion_core::Project::folder_name(repo);
    let reply = crate::oneshot::run(crate::oneshot::Ask {
        prompt: prompt(&name, &readme, &titles),
        system: SYSTEM.into(),
        account,
        model: MODEL.into(),
        effort: EFFORT.into(),
        timeout: TIMEOUT,
    })
    .await?;
    one_line(&reply.text).ok_or_else(|| "the model wrote nothing".to_string())
}

/// What the model is given: the project's name, its README's head and the
/// titles of its latest merged pull requests.
pub fn prompt(name: &str, readme: &str, titles: &[String]) -> String {
    let mut out = format!("Project: {name}\n\n");
    match readme.trim() {
        "" => out.push_str("README: (none)\n"),
        readme => out.push_str(&format!("README (the start):\n{readme}\n")),
    }
    if titles.is_empty() {
        out.push_str("\nRecently merged pull requests: (none found)\n");
    } else {
        out.push_str("\nRecently merged pull requests:\n");
        for title in titles.iter().take(PR_TITLES) {
            out.push_str(&format!("- {}\n", title.trim()));
        }
    }
    out.push_str("\nWrite the one line.");
    out
}

/// A reply as the line kept: its first line with words, without the quotes
/// or bullet a model sometimes wraps it in, cut to a length a row can say.
fn one_line(reply: &str) -> Option<String> {
    let line = reply
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '#', ' ']))
        .map(|l| l.trim_matches(['"', '\'', '`', ' ']))
        .find(|l| !l.is_empty())?;
    Some(crate::ui::truncate(line, LINE_MAX))
}

/// The head of the project's README, whichever spelling it has.
fn readme_head(repo: &Path) -> String {
    let text = [
        "README.md",
        "README",
        "readme.md",
        "README.markdown",
        "README.txt",
    ]
    .iter()
    .find_map(|name| std::fs::read_to_string(repo.join(name)).ok())
    .unwrap_or_default();
    let head: String = text
        .lines()
        .take(README_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    head.chars().take(README_CHARS).collect()
}

/// The titles of the project's last merged pull requests, newest first;
/// none when `gh` can't say.
async fn merged_titles(repo: &Path) -> Vec<String> {
    let limit = PR_TITLES.to_string();
    let args = [
        "pr",
        "list",
        "--state",
        "merged",
        "--limit",
        &limit,
        "--json",
        "title",
        "--jq",
        ".[].title",
    ];
    crate::pull_request::gh(Some(repo), &args, Duration::from_secs(20))
        .await
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A write landed: keep the line, unless it was edited meanwhile by hand.
pub fn land(app: &mut App, answer: Answer) {
    app.project_about.writing.remove(&answer.repo);
    app.dirty = true;
    match answer.line {
        Ok(line) => {
            let mut cfg = Config::load();
            let mut settings = cfg.project(&answer.repo);
            // Typed in while the model was writing: the typed one stands.
            if !answer.by_hand && !settings.about.is_empty() {
                return;
            }
            settings.about = line;
            cfg.set_project(&answer.repo, settings);
            if let Err(err) = cfg.try_save() {
                tracing::warn!(%err, "couldn't save a project's What it is");
            }
        }
        Err(why) if answer.by_hand => {
            app.flash = Some(crate::flash::Flash::failed(format!(
                "couldn't write What it is: {why}"
            )));
        }
        Err(why) => tracing::debug!(%why, repo = %answer.repo.display(), "What it is not written"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_holds_the_readme_head_and_merged_titles() {
        let titles: Vec<String> = (1..=25).map(|n| format!("PR {n}")).collect();
        let prompt = prompt("orion", "# orion\n\nA terminal workspace.", &titles);
        assert!(prompt.starts_with("Project: orion\n"));
        assert!(prompt.contains("README (the start):\n# orion\n\nA terminal workspace.\n"));
        assert!(prompt.contains("- PR 1\n"));
        assert!(prompt.contains("- PR 20\n"));
        assert!(!prompt.contains("- PR 21\n"), "twenty at most");
        assert!(prompt.ends_with("Write the one line."));
    }

    #[test]
    fn the_prompt_says_when_there_is_nothing_to_go_on() {
        let prompt = prompt("blank", "  ", &[]);
        assert!(prompt.contains("README: (none)"));
        assert!(prompt.contains("Recently merged pull requests: (none found)"));
    }

    #[test]
    fn a_reply_is_kept_as_its_first_line() {
        assert_eq!(
            one_line("\n\"A terminal workspace for running coding agents.\"\n\nMore.").as_deref(),
            Some("A terminal workspace for running coding agents.")
        );
        assert_eq!(one_line("- A thing.").as_deref(), Some("A thing."));
        assert_eq!(one_line("  \n "), None);
        assert!(one_line(&"x".repeat(500)).unwrap().chars().count() <= LINE_MAX);
    }

    #[test]
    fn the_readme_head_is_cut() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(readme_head(dir.path()), "");
        let long: String = (0..200).map(|n| format!("line {n}\n")).collect();
        std::fs::write(dir.path().join("README.md"), long).unwrap();
        let head = readme_head(dir.path());
        assert_eq!(head.lines().count(), README_LINES);
        assert!(head.starts_with("line 0\n"));
    }

    /// Writes this repo's own line through the default Claude account,
    /// for real. By hand: `cargo test -p orion-tui projects::tests::writes_orions_own -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn writes_orions_own_line() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let repo = repo.canonicalize().unwrap();
        let line = write(&Config::load(), &repo).await.unwrap();
        eprintln!("What it is: {line}");
        assert!(!line.is_empty());
    }
}
