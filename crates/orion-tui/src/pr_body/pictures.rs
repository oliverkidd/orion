//! PICTURES: whether a pull request changes what users see, and whether it
//! shows it.
//!
//! The repo's pictures job (riplo-os's `pr-keyframes.yml`) commits its
//! before/after grids to `pr-<n>/grids/` on the `pr-screenshots` branch and
//! writes what it found to `pr-<n>/pictures.json` there ([`Manifest`], read
//! with `gh api` by [`manifest`]). It reports in a check named `PR
//! pictures` that is never red, and that shows as skipped when the job
//! decided not to run.
//!
//! - **Visible** when a changed file matches the contract's
//!   `visible_paths`, or the `PR pictures` check ran on it (the job only
//!   runs, rather than skips, for a visible change). Skipped isn't ran.
//! - **Shown** when `pictures.json` names at least one grid and was taken
//!   at the PR's current head: pictures of an older push don't count.
//!   The grid in the body and on the card is the first one.
//! - **No difference** when they were taken at the head but every frame
//!   says `no visible difference`, or every frame's difference is 0
//!   ([`Manifest::no_difference`]): the flow never reached the state the
//!   change affects. Those don't count as shown either — the verdict
//!   treats them as missing, and the PR's agent is sent back once per head
//!   to stage that state (`super::NO_DIFFERENCE`).

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::pull_request::{CheckState, PrDetail};

/// The CI check that takes a PR's pictures.
pub const CHECK: &str = "PR pictures";
/// The branch pictures are kept on.
pub const BRANCH: &str = "pr-screenshots";

/// One grid of before/after pictures, as `pictures.json` lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Grid {
    /// Its file name in `pr-<n>/grids/` (`grid-01-ab12cd34.png`).
    pub file: String,
    /// Where a browser — and a PR body — shows it.
    pub url: String,
}

impl Grid {
    /// Its file name: `file`, else the last part of `url`.
    fn name(&self) -> &str {
        let name = self.file.trim();
        if !name.is_empty() {
            return name;
        }
        let path = self.url.split(['?', '#']).next().unwrap_or_default();
        path.rsplit('/').next().unwrap_or_default()
    }
}

/// One frame the job checked.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Frame {
    pub index: i64,
    pub label: String,
    /// `changed`, `no visible difference`, `new`, `missing` or `blank`.
    pub verdict: String,
    /// The share of pixels that differ, when both sides have the frame.
    pub difference: Option<f64>,
}

/// `pr-<n>/pictures.json` on [`BRANCH`]. A PR without one reads as the
/// default: no grids, so nothing shown.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Manifest {
    /// The commit the pictures were taken at.
    pub head_sha: String,
    /// The preview the "after" side was taken from.
    pub preview: String,
    /// The Actions run that took them.
    pub run: String,
    pub grids: Vec<Grid>,
    pub frames: Vec<Frame>,
    pub problems: Vec<String>,
    pub notes: Vec<String>,
}

/// What a frame that shows nothing changed says.
const NO_VISIBLE_DIFFERENCE: &str = "no visible difference";

impl Manifest {
    /// Whether its pictures show nothing: it has frames, and every one
    /// says `no visible difference`, or every one has a difference of 0.
    /// A frame that is `new` (no difference to measure) is a change.
    pub fn no_difference(&self) -> bool {
        let frames = &self.frames;
        !frames.is_empty()
            && (frames
                .iter()
                .all(|f| f.verdict.trim().eq_ignore_ascii_case(NO_VISIBLE_DIFFERENCE))
                || frames
                    .iter()
                    .all(|f| f.difference.is_some_and(|d| d <= 0.0)))
    }
}

/// The pictures signal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pictures {
    pub visible: bool,
    /// Pictures taken at the head that show the change.
    pub shown: bool,
    /// Pictures were taken at the head, but no frame shows a difference:
    /// not shown.
    pub no_difference: bool,
    /// The first grid, when shown.
    pub grid: Option<Grid>,
}

impl Pictures {
    pub fn grid(&self) -> Option<&Grid> {
        self.grid.as_ref()
    }
}

/// The signal from the PR and its `pictures.json`.
pub fn of(detail: &PrDetail, visible_paths: &[String], manifest: &Manifest) -> Pictures {
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
    let current = !manifest.head_sha.trim().is_empty()
        && !detail.head_sha.trim().is_empty()
        && manifest.head_sha.trim() == detail.head_sha.trim();
    let taken = manifest
        .grids
        .iter()
        .find(|g| !g.url.trim().is_empty())
        .filter(|_| current)
        .cloned();
    let no_difference = taken.is_some() && manifest.no_difference();
    let grid = taken.filter(|_| !no_difference);
    Pictures {
        visible: touched || ran,
        shown: grid.is_some(),
        no_difference,
        grid,
    }
}

/// `pictures.json` from a `contents` answer: its base64 `content`
/// decoded. A manifest that can't be read is no manifest.
pub fn parse_contents(json: &str) -> Manifest {
    use base64::Engine as _;
    let content = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("content")?.as_str().map(str::to_string))
        .unwrap_or_default();
    let packed: String = content.split_whitespace().collect();
    base64::engine::general_purpose::STANDARD
        .decode(packed)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

const TIMEOUT: Duration = Duration::from_secs(20);

/// PR `number`'s `pictures.json` on [`BRANCH`]; the default when it has
/// none, None when `gh` couldn't ask.
pub async fn manifest(owner: &str, repo: &str, number: u64) -> Option<Manifest> {
    let endpoint = format!("repos/{owner}/{repo}/contents/pr-{number}/pictures.json?ref={BRANCH}");
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(["api", endpoint.as_str()]);
    match crate::pull_request::run_piped(cmd, "", TIMEOUT).await {
        Ok(json) => Some(parse_contents(&json)),
        Err(why) if super::not_found(&why) => Some(Manifest::default()),
        Err(why) => {
            tracing::info!("pr_body: reading the pictures of {owner}/{repo}#{number}: {why}");
            None
        }
    }
}

/// Where PR `number`'s grid at `head_sha` is kept for the card, under the
/// `pr-body` cache dir.
pub fn grid_file(cache: &Path, number: u64, head_sha: &str) -> PathBuf {
    let sha: String = head_sha
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    cache.join("grids").join(format!("{number}-{sha}.png"))
}

/// Download PR `number`'s `grid` to `to`, for the verdict card. False when
/// it couldn't.
pub async fn download(owner: &str, repo: &str, number: u64, grid: &Grid, to: &Path) -> bool {
    let name = grid.name();
    if name.is_empty() || name.contains("..") {
        return false;
    }
    let endpoint = format!("repos/{owner}/{repo}/contents/pr-{number}/grids/{name}?ref={BRANCH}");
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
    // Written whole, then moved in: a card never uploads half a picture.
    let part = to.with_extension("png.part");
    tokio::fs::write(&part, &out.stdout).await.is_ok() && tokio::fs::rename(&part, to).await.is_ok()
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

    /// What riplo-os's pictures job writes (`pr-keyframes.yml`, publish).
    const MANIFEST: &str = r#"{
        "pr": 7,
        "head_sha": "abc123",
        "preview": "https://editor-git-x.vercel.app",
        "run": "https://github.com/o/r/actions/runs/1",
        "grids": [
            {"file": "grid-01-ab12cd34.png",
             "rows": [{"index": 1, "label": "home", "verdict": "changed"}],
             "url": "https://github.com/o/r/blob/pr-screenshots/pr-7/grids/grid-01-ab12cd34.png?raw=true"},
            {"file": "grid-02-ef56ab78.png", "rows": [],
             "url": "https://github.com/o/r/blob/pr-screenshots/pr-7/grids/grid-02-ef56ab78.png?raw=true"}
        ],
        "frames": [
            {"index": 1, "label": "home", "before": "1-home.png", "after": "1-home.png",
             "difference": 0.12, "verdict": "changed"},
            {"index": 2, "label": "menu", "before": null, "after": "2-menu.png",
             "difference": null, "verdict": "new"}
        ],
        "problems": ["Frame 3 menu: the picture of dev is blank."],
        "notes": []
    }"#;

    fn contents(json: &str) -> String {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        // GitHub wraps the content at 60 columns.
        let wrapped: Vec<String> = encoded
            .as_bytes()
            .chunks(60)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        serde_json::json!({"type": "file", "encoding": "base64", "content": wrapped.join("\n")})
            .to_string()
    }

    fn at(head: &str) -> PrDetail {
        PrDetail {
            head_sha: head.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_manifest_is_read_from_its_base64_contents() {
        let m = parse_contents(&contents(MANIFEST));
        assert_eq!(m.head_sha, "abc123");
        assert_eq!(m.run, "https://github.com/o/r/actions/runs/1");
        assert_eq!(m.grids.len(), 2);
        assert_eq!(m.grids[0].file, "grid-01-ab12cd34.png");
        assert_eq!(m.frames[1].verdict, "new");
        assert_eq!(m.frames[0].difference, Some(0.12));
        assert_eq!(m.problems.len(), 1);
        // The job writes `{}` when it can't build the manifest.
        assert_eq!(parse_contents(&contents("{}")), Manifest::default());
        assert_eq!(parse_contents("not json"), Manifest::default());
    }

    #[test]
    fn shown_takes_grids_taken_at_the_current_head_and_the_first_is_the_grid() {
        let m = parse_contents(&contents(MANIFEST));
        let p = of(&at("abc123"), &[], &m);
        assert!(p.shown && !p.visible);
        assert_eq!(
            p.grid().unwrap().url,
            "https://github.com/o/r/blob/pr-screenshots/pr-7/grids/grid-01-ab12cd34.png?raw=true"
        );
        // Pictures of an older push don't count.
        let stale = of(&at("def456"), &[], &m);
        assert!(!stale.shown && stale.grid().is_none());
        // Nor does a manifest without grids.
        let empty = Manifest {
            head_sha: "abc123".into(),
            ..Default::default()
        };
        assert!(!of(&at("abc123"), &[], &empty).shown);
        assert!(!of(&at(""), &[], &Manifest::default()).shown);
    }

    fn frame(verdict: &str, difference: Option<f64>) -> Frame {
        Frame {
            verdict: verdict.into(),
            difference,
            ..Default::default()
        }
    }

    #[test]
    fn pictures_whose_every_frame_shows_no_difference_are_not_shown() {
        let mut m = parse_contents(&contents(MANIFEST));
        assert!(!m.no_difference(), "a changed frame and a new one");
        m.frames = vec![
            frame("no visible difference", Some(0.0)),
            frame("No visible difference", Some(0.001)),
        ];
        assert!(m.no_difference(), "every verdict says so");
        let p = of(&at("abc123"), &[], &m);
        assert!(!p.shown && p.no_difference && p.grid().is_none(), "{p:?}");
        m.frames = vec![frame("changed", Some(0.0)), frame("blank", Some(0.0))];
        assert!(m.no_difference(), "every difference is 0");
        m.frames = vec![
            frame("no visible difference", Some(0.0)),
            frame("new", None),
        ];
        assert!(!m.no_difference(), "a new frame is a change");
        m.frames = vec![];
        assert!(!m.no_difference(), "no frames to judge");
        assert!(of(&at("abc123"), &[], &m).shown);
        // An older push's pictures are neither.
        m.frames = vec![frame("no visible difference", Some(0.0))];
        let stale = of(&at("def456"), &[], &m);
        assert!(!stale.shown && !stale.no_difference);
    }

    #[test]
    fn visible_by_path_or_by_the_check_running() {
        let none = Manifest::default();
        let mut detail = PrDetail {
            files: vec![file("apps/web/a.tsx")],
            ..Default::default()
        };
        let visible = ["apps/web/**".to_string()];
        let p = of(&detail, &visible, &none);
        assert!(p.visible && !p.shown);
        detail.files = vec![file("server/a.rs")];
        assert!(!of(&detail, &visible, &none).visible);
        detail.checks = vec![check(CheckState::Skipped)];
        assert!(!of(&detail, &visible, &none).visible);
        detail.checks = vec![check(CheckState::Passed)];
        let p = of(&detail, &visible, &none);
        // A green check alone isn't pictures: it is never red.
        assert!(p.visible && !p.shown);
        detail.checks = vec![check(CheckState::Running)];
        assert!(of(&detail, &visible, &none).visible);
    }

    #[test]
    fn the_grid_is_cached_by_pr_and_head_and_named_from_its_file() {
        let to = grid_file(Path::new("/data/pr-body"), 7, "abc123");
        assert_eq!(to, Path::new("/data/pr-body/grids/7-abc123.png"));
        let g = Grid {
            file: String::new(),
            url: "https://github.com/o/r/blob/pr-screenshots/pr-7/grids/grid-01-ab.png?raw=true"
                .into(),
        };
        assert_eq!(g.name(), "grid-01-ab.png");
    }
}
