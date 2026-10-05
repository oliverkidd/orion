//! WORKTREE CONTAINERS: the docker compose projects a checkout started,
//! found by the label compose stamps on every container it creates —
//! `com.docker.compose.project.working_dir`, the directory `up` ran from.
//! A worktree that runs `docker compose up` leaves a project behind when
//! orion deletes it; this is how the DAEMON knows which one to stop or
//! tear down (`worktree_containers`), and how `orion doctor` spots the
//! ones whose checkout is already gone.
//!
//! Plain docker CLI, so it serves any engine that speaks it — OrbStack,
//! Docker Desktop, Colima. Everything here is pure: what to run, and how
//! to read what it printed. Running it is each caller's (async in the
//! DAEMON, blocking in `orion doctor`).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// What happens to a deleted worktree's compose projects: the
/// `worktree_containers` SETTING.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorktreeContainers {
    /// Left as they are. The default: orion touches nothing it didn't make.
    #[default]
    Off,
    /// `docker compose stop`: nothing left running, everything kept.
    Stop,
    /// `docker compose down`: containers and networks gone, volumes kept.
    Remove,
    /// `docker compose down --volumes`: the project's data goes too.
    RemoveVolumes,
}

/// The **Worktree containers** choices, in the order the row cycles them,
/// the default first.
pub const WORKTREE_CONTAINERS: &[&str] = &[
    WorktreeContainers::Off.as_str(),
    WorktreeContainers::Stop.as_str(),
    WorktreeContainers::Remove.as_str(),
    WorktreeContainers::RemoveVolumes.as_str(),
];

impl WorktreeContainers {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Stop => "stop",
            Self::Remove => "remove",
            Self::RemoveVolumes => "remove+volumes",
        }
    }

    /// A stored word back to its policy; anything unknown is `off`, so a
    /// typo never deletes anything.
    pub fn parse(word: &str) -> Self {
        let word = word.trim();
        [Self::Stop, Self::Remove, Self::RemoveVolumes]
            .into_iter()
            .find(|p| word.eq_ignore_ascii_case(p.as_str()))
            .unwrap_or(Self::Off)
    }

    /// The `docker` arguments that apply this policy to compose `project`;
    /// None for `off`. They need no compose file: compose finds the
    /// project's containers, networks and volumes by their labels.
    pub fn compose_args(self, project: &str) -> Option<Vec<String>> {
        let tail: &[&str] = match self {
            Self::Off => return None,
            Self::Stop => &["stop"],
            Self::Remove => &["down", "--remove-orphans"],
            Self::RemoveVolumes => &["down", "--remove-orphans", "--volumes"],
        };
        let head = ["compose", "-p", project];
        Some(head.iter().chain(tail).map(|s| s.to_string()).collect())
    }

    /// What was done, for a log line: "stopped", "removed", …
    pub fn past_tense(self) -> &'static str {
        match self {
            Self::Off => "left",
            Self::Stop => "stopped",
            Self::Remove => "removed",
            Self::RemoveVolumes => "removed with its volumes",
        }
    }
}

/// `docker ps` arguments printing, a container a line, its compose
/// project and the directory it was started from — tab-separated, both
/// empty for a container compose didn't make.
pub fn ps_args() -> Vec<&'static str> {
    vec![
        "ps",
        "--all",
        "--format",
        "{{.Label \"com.docker.compose.project\"}}\t{{.Label \"com.docker.compose.project.working_dir\"}}",
    ]
}

/// One container compose made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub project: String,
    pub working_dir: PathBuf,
}

/// [`ps_args`]' output, without the containers compose didn't make.
pub fn parse_ps(out: &str) -> Vec<Container> {
    out.lines()
        .filter_map(|line| {
            let (project, dir) = line.split_once('\t')?;
            let (project, dir) = (project.trim(), dir.trim());
            (!project.is_empty() && !dir.is_empty()).then(|| Container {
                project: project.to_string(),
                working_dir: PathBuf::from(dir),
            })
        })
        .collect()
}

/// The compose projects started in `dir` or a directory under it — a
/// nested checkout's included, since deleting `dir` deletes that too.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StartedIn {
    /// Every container of the project started under `dir`: its to release.
    pub owned: Vec<String>,
    /// Some containers started under `dir`, some elsewhere — two checkouts
    /// sharing a project name. Left alone: tearing it down would take the
    /// other checkout's containers, and their data, with it.
    pub shared: Vec<String>,
}

/// Sort `containers`' projects by whether they live wholly under `dir`.
pub fn started_in(containers: &[Container], dir: &Path) -> StartedIn {
    let mut out = StartedIn::default();
    for (project, dirs) in by_project(containers) {
        let inside = dirs.iter().filter(|d| is_within(d, dir)).count();
        if inside == dirs.len() {
            out.owned.push(project);
        } else if inside > 0 {
            out.shared.push(project);
        }
    }
    out
}

/// Projects every container of which was started in a directory that no
/// longer `exists` — what a checkout deleted without orion's cleanup left
/// behind — each with the directory it ran from.
pub fn orphaned(
    containers: &[Container],
    exists: impl Fn(&Path) -> bool,
) -> Vec<(String, PathBuf)> {
    by_project(containers)
        .into_iter()
        .filter(|(_, dirs)| dirs.iter().all(|d| !exists(d)))
        .map(|(project, mut dirs)| (project, dirs.swap_remove(0)))
        .collect()
}

fn by_project(containers: &[Container]) -> BTreeMap<String, Vec<PathBuf>> {
    let mut map: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for c in containers {
        map.entry(c.project.clone())
            .or_default()
            .push(c.working_dir.clone());
    }
    map
}

/// `path` is `dir` or under it, component-wise: `/a/wt` is not within
/// `/a/w`. A trailing slash on either side changes nothing.
fn is_within(path: &Path, dir: &Path) -> bool {
    !dir.as_os_str().is_empty() && path.starts_with(dir)
}

/// The `docker` CLI: the first on `path`, else where OrbStack, Homebrew
/// and Docker Desktop install one — the DAEMON runs under launchd's thin
/// PATH, which names none of them.
pub fn find_docker(path: &OsStr, home: Option<&Path>) -> Option<PathBuf> {
    let fixed = [
        home.map(|h| h.join(".orbstack/bin/docker")),
        Some(PathBuf::from("/usr/local/bin/docker")),
        Some(PathBuf::from("/opt/homebrew/bin/docker")),
        Some(PathBuf::from(
            "/Applications/Docker.app/Contents/Resources/bin/docker",
        )),
    ];
    std::env::split_paths(path)
        .map(|dir| dir.join("docker"))
        .chain(fixed.into_iter().flatten())
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(project: &str, dir: &str) -> Container {
        Container {
            project: project.into(),
            working_dir: dir.into(),
        }
    }

    #[test]
    fn policy_words_round_trip_and_unknown_is_off() {
        for word in WORKTREE_CONTAINERS {
            assert_eq!(WorktreeContainers::parse(word).as_str(), *word);
        }
        assert_eq!(
            WorktreeContainers::parse(" Remove+Volumes "),
            WorktreeContainers::RemoveVolumes
        );
        assert_eq!(WorktreeContainers::parse(""), WorktreeContainers::Off);
        assert_eq!(WorktreeContainers::parse("delete"), WorktreeContainers::Off);
    }

    #[test]
    fn compose_args_name_the_project_and_only_volumes_drop_data() {
        assert_eq!(WorktreeContainers::Off.compose_args("p"), None);
        assert_eq!(
            WorktreeContainers::Stop.compose_args("p").unwrap(),
            ["compose", "-p", "p", "stop"]
        );
        let remove = WorktreeContainers::Remove.compose_args("p").unwrap();
        assert!(!remove.contains(&"--volumes".to_string()));
        let purge = WorktreeContainers::RemoveVolumes.compose_args("p").unwrap();
        assert_eq!(purge[..4], ["compose", "-p", "p", "down"]);
        assert!(purge.contains(&"--volumes".to_string()));
    }

    #[test]
    fn ps_output_keeps_only_compose_containers() {
        let out = "app\t/src/wt\n\t\nbuildkit\t\n  \nother\t/src/x\n";
        assert_eq!(parse_ps(out), [c("app", "/src/wt"), c("other", "/src/x")]);
    }

    #[test]
    fn a_project_is_owned_when_every_container_started_under_the_dir() {
        let containers = [
            c("wt", "/src/wt"),
            c("wt", "/src/wt"),
            c("nested", "/src/wt/.claude/worktrees/x"),
            c("sibling", "/src/wt-2"),
            c("main", "/src/main"),
            c("shared", "/src/wt"),
            c("shared", "/src/main"),
        ];
        let found = started_in(&containers, Path::new("/src/wt/"));
        assert_eq!(found.owned, ["nested", "wt"]);
        assert_eq!(
            found.shared,
            ["shared"],
            "another checkout's containers are spared"
        );
        assert_eq!(started_in(&containers, Path::new("")), StartedIn::default());
    }

    #[test]
    fn orphans_are_projects_whose_every_dir_is_gone() {
        let containers = [
            c("gone", "/gone"),
            c("live", "/live"),
            c("half", "/gone"),
            c("half", "/live"),
        ];
        let found = orphaned(&containers, |d| d == Path::new("/live"));
        assert_eq!(found, [("gone".to_string(), PathBuf::from("/gone"))]);
    }

    #[test]
    fn docker_is_found_on_path_first_then_where_installers_put_it() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let orb = dir.path().join(".orbstack/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&orb).unwrap();
        std::fs::write(orb.join("docker"), "").unwrap();
        let path = std::env::join_paths([&bin]).unwrap();
        assert_eq!(
            find_docker(&path, Some(dir.path())),
            Some(orb.join("docker")),
            "OrbStack's, though launchd's PATH doesn't name it"
        );
        std::fs::write(bin.join("docker"), "").unwrap();
        assert_eq!(
            find_docker(&path, Some(dir.path())),
            Some(bin.join("docker"))
        );
    }
}
