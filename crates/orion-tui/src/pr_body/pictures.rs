//! PICTURES: whether a pull request changes what users see, and whether it
//! shows it.
//!
//! - **Visible** when a changed file matches the contract's
//!   `visible_paths`, or the repo's `PR pictures` check ran on it (the CI
//!   job only runs, rather than skips, for a visible change).
//! - **Shown** when that check passed, or the `pr-screenshots` branch
//!   holds pictures under `pr-<number>/` (the manifest, read with `gh api`
//!   by [`listing`]). The grid is the one named `grid…`, else the first.

use serde::Deserialize;
use std::time::Duration;

use crate::pull_request::{CheckState, PrDetail};

/// The CI check that takes a PR's pictures.
pub const CHECK: &str = "PR pictures";
/// The branch pictures are kept on.
pub const BRANCH: &str = "pr-screenshots";

/// One picture on [`BRANCH`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// Its path on the branch (`pr-7/grid-ab12cd34.png`).
    pub path: String,
    /// Where a browser — and a PR body — shows it.
    pub url: String,
}

/// The pictures signal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pictures {
    pub visible: bool,
    pub shown: bool,
    /// The grid first, then the rest.
    pub pictures: Vec<Picture>,
}

impl Pictures {
    pub fn grid(&self) -> Option<&Picture> {
        self.pictures.first()
    }
}

/// The signal from what the PR's body and the manifest say.
pub fn of(detail: &PrDetail, visible_paths: &[String], mut pictures: Vec<Picture>) -> Pictures {
    let check = detail
        .checks
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(CHECK))
        .map(|c| c.state);
    let touched = detail
        .files
        .iter()
        .any(|f| super::contract::matches_any(visible_paths, &f.path));
    let ran = matches!(
        check,
        Some(CheckState::Passed | CheckState::Failed | CheckState::Running)
    );
    pictures.sort_by_key(|p| {
        let name = p.path.rsplit('/').next().unwrap_or_default();
        (!name.starts_with("grid"), p.path.clone())
    });
    Pictures {
        visible: touched || ran,
        shown: check == Some(CheckState::Passed) || !pictures.is_empty(),
        pictures,
    }
}

#[derive(Deserialize)]
struct Entry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

const IMAGE: [&str; 5] = [".png", ".jpg", ".jpeg", ".gif", ".webp"];

/// The pictures in a `contents` listing of `pr-<n>/` on [`BRANCH`].
pub fn parse_listing(owner: &str, repo: &str, json: &str) -> Vec<Picture> {
    let entries: Vec<Entry> = serde_json::from_str(json).unwrap_or_default();
    entries
        .into_iter()
        .filter(|e| e.kind == "file")
        .filter(|e| {
            let lower = e.path.to_ascii_lowercase();
            IMAGE.iter().any(|x| lower.ends_with(x))
        })
        .map(|e| Picture {
            url: format!(
                "https://github.com/{owner}/{repo}/blob/{BRANCH}/{}?raw=true",
                e.path
            ),
            path: e.path,
        })
        .collect()
}

const TIMEOUT: Duration = Duration::from_secs(20);

/// The pictures kept for PR `number` of `owner/repo`; an empty list when
/// there are none, None when `gh` couldn't ask.
pub async fn listing(owner: &str, repo: &str, number: u64) -> Option<Vec<Picture>> {
    let endpoint = format!("repos/{owner}/{repo}/contents/pr-{number}?ref={BRANCH}");
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(["api", endpoint.as_str()]);
    match crate::pull_request::run_piped(cmd, "", TIMEOUT).await {
        Ok(json) => Some(parse_listing(owner, repo, &json)),
        Err(why) if super::not_found(&why) => Some(Vec::new()),
        Err(why) => {
            tracing::info!("pr_body: listing pictures of {owner}/{repo}#{number}: {why}");
            None
        }
    }
}

/// Download `picture` to `to`, for the verdict card. False when it
/// couldn't.
pub async fn download(owner: &str, repo: &str, picture: &Picture, to: &std::path::Path) -> bool {
    let endpoint = format!(
        "repos/{owner}/{repo}/contents/{}?ref={BRANCH}",
        picture.path
    );
    let run = tokio::process::Command::new("gh")
        .args(["api", "-H", "Accept: application/vnd.github.raw", &endpoint])
        .kill_on_drop(true)
        .output();
    let Ok(Ok(out)) = tokio::time::timeout(TIMEOUT, run).await else {
        return false;
    };
    if !out.status.success() || out.stdout.is_empty() {
        return false;
    }
    if let Some(dir) = to.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    tokio::fs::write(to, &out.stdout).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::{PrCheck, PrFile};

    fn check(state: CheckState) -> PrCheck {
        PrCheck {
            name: CHECK.into(),
            workflow: String::new(),
            state,
            word: String::new(),
            started: String::new(),
            completed: String::new(),
            url: String::new(),
        }
    }

    fn file(path: &str) -> PrFile {
        PrFile {
            path: path.into(),
            additions: 1,
            deletions: 0,
            change: String::new(),
        }
    }

    #[test]
    fn the_listing_keeps_pictures_and_puts_the_grid_first() {
        let json = r#"[
            {"path":"pr-7/web-1.png","type":"file"},
            {"path":"pr-7/manifest.json","type":"file"},
            {"path":"pr-7/grid-ab.png","type":"file"},
            {"path":"pr-7/old","type":"dir"}
        ]"#;
        let found = parse_listing("o", "r", json);
        assert_eq!(found.len(), 2);
        let detail = PrDetail::default();
        let p = of(&detail, &[], found);
        assert_eq!(p.grid().unwrap().path, "pr-7/grid-ab.png");
        assert_eq!(
            p.grid().unwrap().url,
            "https://github.com/o/r/blob/pr-screenshots/pr-7/grid-ab.png?raw=true"
        );
        assert!(p.shown && !p.visible);
    }

    #[test]
    fn visible_by_path_or_by_the_check_running() {
        let mut detail = PrDetail {
            files: vec![file("apps/web/a.tsx")],
            ..Default::default()
        };
        let visible = ["apps/web/**".to_string()];
        let p = of(&detail, &visible, vec![]);
        assert!(p.visible && !p.shown);
        detail.files = vec![file("server/a.rs")];
        assert!(!of(&detail, &visible, vec![]).visible);
        detail.checks = vec![check(CheckState::Skipped)];
        assert!(!of(&detail, &visible, vec![]).visible);
        detail.checks = vec![check(CheckState::Passed)];
        let p = of(&detail, &visible, vec![]);
        assert!(p.visible && p.shown);
        detail.checks = vec![check(CheckState::Failed)];
        let p = of(&detail, &visible, vec![]);
        assert!(p.visible && !p.shown);
    }
}
