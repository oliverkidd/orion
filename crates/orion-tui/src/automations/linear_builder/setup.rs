//! The factory's SETUP CHECK. orion works on any repo, so what the
//! builder prompt points at is read from the repo, never assumed:
//!
//! - **The workflow doc**, [`super::WORKFLOW_DOC`]. Without it the bug
//!   fixer and the feature builder start nothing in that project, and say
//!   so once a run on a card ([`missing_workflow`]).
//! - **The tools list**, a "Tools" heading in `AGENTS.md`, where agents
//!   look before asking for access, data or logs.
//!
//! Switching either runner on checks every project with a Linear key, and
//! a project missing either gets a one-time card naming what's missing
//! ([`on_enable`]). The card's **Open template** writes orion's template
//! ([`TEMPLATE`], `docs/factory-template/automation.md` in orion's repo)
//! to `<data_dir>/factory-template/automation.md` and opens it in the
//! editor **Open files in** names. Its key
//! is `setup:<project id>`.

use std::path::{Path, PathBuf};

use orion_core::ProjectId;

use super::{Automation, WORKFLOW_DOC};
use crate::app::App;
use crate::factory::cards::{self, Button, Card, Press, Recipient};

/// orion's template for the workflow doc.
pub const TEMPLATE: &str = include_str!("../../../../../docs/factory-template/automation.md");

/// Whether `repo` has the factory workflow doc.
pub(crate) fn has_workflow_doc(repo: &Path) -> bool {
    repo.join(WORKFLOW_DOC).is_file()
}

/// Whether `repo`'s `AGENTS.md` has a "Tools" heading.
pub(crate) fn has_tools_list(repo: &Path) -> bool {
    std::fs::read_to_string(repo.join("AGENTS.md")).is_ok_and(|text| {
        text.lines().any(|line| {
            let line = line.trim();
            let heading = line.trim_start_matches('#');
            line.starts_with('#')
                && heading
                    .trim()
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .is_some_and(|w| w.eq_ignore_ascii_case("tools"))
        })
    })
}

/// What `repo` is missing, as the card names it.
pub(crate) fn missing(repo: &Path) -> Vec<&'static str> {
    let mut out = Vec::new();
    if !has_workflow_doc(repo) {
        out.push("factory workflow doc");
    }
    if !has_tools_list(repo) {
        out.push("tools list");
    }
    out
}

fn repo_of(app: &App, project: &ProjectId) -> Option<(PathBuf, String)> {
    app.tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map(|p| (p.repo_path.clone(), p.name.clone()))
}

/// Whether `project`'s checkout has the workflow doc: what a launch there
/// waits on.
pub(crate) fn workflow_doc_in(app: &App, project: &ProjectId) -> bool {
    repo_of(app, project).is_some_and(|(repo, _)| has_workflow_doc(&repo))
}

/// A launch in `project` held for its missing workflow doc: said once a
/// run.
pub(crate) fn missing_workflow(app: &mut App, project: &ProjectId) {
    let Some((repo, name)) = repo_of(app, project) else {
        return;
    };
    post(app, project, &name, missing(&repo));
}

/// `which` switched on: every project with a Linear key checked, and one
/// missing a piece told once.
pub(crate) fn on_enable(app: &mut App, which: Automation) {
    if !matches!(which, Automation::BugFixer | Automation::FeatureBuilder) {
        return;
    }
    let projects: Vec<(ProjectId, PathBuf, String)> = app
        .tree
        .projects
        .iter()
        .filter(|p| crate::linear::read_linear_key(&p.repo_path).is_some())
        .map(|p| (p.id.clone(), p.repo_path.clone(), p.name.clone()))
        .collect();
    for (id, repo, name) in projects {
        post(app, &id, &name, missing(&repo));
    }
}

/// Where the template is written for **Open template**.
pub(crate) fn template_path() -> PathBuf {
    orion_core::paths::data_dir()
        .join("factory-template")
        .join("automation.md")
}

/// The card's body: what's missing, and where the template is.
pub(crate) fn body(name: &str, missing: &[&str]) -> Vec<String> {
    let mut body = vec![format!("*{name}* has no {}.", missing.join(" or "))];
    if missing.contains(&"factory workflow doc") {
        body.push(format!(
            "The bug fixer and the feature builder start nothing there until `{WORKFLOW_DOC}` exists. orion's template: `{}`",
            template_path().display()
        ));
    }
    if missing.contains(&"tools list") {
        body.push(
            "Add a *Tools* section to `AGENTS.md`: where agents find access, data and logs before asking you."
                .into(),
        );
    }
    body
}

/// The card, once a run per project and what it misses.
fn post(app: &mut App, project: &ProjectId, name: &str, missing: Vec<&'static str>) {
    if missing.is_empty() {
        return;
    }
    let once = format!("{project}:{}", missing.join("+"));
    if !app.automations.linear_builder.setup_carded.insert(once) {
        return;
    }
    let card = Card {
        key: format!("setup:{project}"),
        to: Recipient::Owner,
        context: format!("Factory · {name}"),
        title: "Factory setup missing".into(),
        body: body(name, &missing),
        buttons: vec![Button::tap("template", "Open template")],
        image: None,
    };
    cards::post(app, card);
}

/// A press on a `setup:` card: **Open template** writes the template
/// and opens it.
pub(crate) fn on_action(app: &mut App, press: Press) {
    if press.action != "template" {
        tracing::warn!("factory: {} on {} does nothing", press.action, press.key);
        return;
    }
    let path = template_path();
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| std::fs::write(&path, TEMPLATE));
    if let Err(why) = written {
        tracing::warn!("factory: writing the template: {why}");
        return;
    }
    // In the editor **Open files in** names.
    let opened = crate::outside_editor::configured().and_then(|editor| {
        let dir = path.parent().unwrap_or(&path);
        let (program, args) = editor.file_command(dir, "automation.md", 1);
        match cfg!(test) {
            true => Ok(()),
            false => crate::outside_editor::launch(&program, &args),
        }
    });
    if let Err(why) = opened {
        tracing::warn!("factory: opening the template: {why}");
    }
    cards::close(
        app,
        press.key,
        &format!("Template at {}: copy it to {WORKFLOW_DOC}", path.display()),
    );
}
