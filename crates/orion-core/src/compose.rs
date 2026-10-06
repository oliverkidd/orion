//! WORKTREE CONTAINERS: the docker compose projects a checkout started,
//! found by the label compose stamps on every container it creates —
//! `com.docker.compose.project.working_dir`, the directory `up` ran from.
//! A worktree that runs `docker compose up` leaves a project behind when
//! orion deletes it; this is how the DAEMON knows which one to stop or
//! tear down (`worktree_containers`), how `orion doctor` spots the
//! ones whose checkout is already gone, and how STACK STATUS — the band's
//! `⬡` and the Stacks modal — knows which stacks run where.
//!
//! Plain docker CLI, so it serves any engine that speaks it — OrbStack,
//! Docker Desktop, Colima. Everything here is pure: what to run, and how
//! to read what it printed. Running it is each caller's (async in the
//! DAEMON, blocking in `orion doctor`).

use serde::{Deserialize, Serialize};
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
        let verb = match self {
            Self::Off => return None,
            Self::Stop => StackVerb::Stop,
            Self::Remove => StackVerb::Down,
            Self::RemoveVolumes => StackVerb::DownVolumes,
        };
        Some(verb.compose_args(project))
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

/// [`ps_args`] with a third column: the container's state (`running`,
/// `exited`, `created`, …) — what STACK STATUS reads.
pub fn ps_state_args() -> Vec<&'static str> {
    vec![
        "ps",
        "--all",
        "--format",
        "{{.Label \"com.docker.compose.project\"}}\t{{.Label \"com.docker.compose.project.working_dir\"}}\t{{.State}}",
    ]
}

/// [`ps_state_args`]' output: each compose container, and whether it is
/// running.
pub fn parse_ps_state(out: &str) -> Vec<(Container, bool)> {
    out.lines()
        .filter_map(|line| {
            let mut cols = line.split('\t').map(str::trim);
            let (project, dir) = (cols.next()?, cols.next()?);
            let running = cols.next() == Some("running");
            (!project.is_empty() && !dir.is_empty()).then(|| {
                (
                    Container {
                        project: project.to_string(),
                        working_dir: PathBuf::from(dir),
                    },
                    running,
                )
            })
        })
        .collect()
}

/// STACK STATUS: one compose project on the machine, whoever started it —
/// what the band's `⬡` and the Stacks modal read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    pub project: String,
    /// Every directory its containers were started from, sorted, once each.
    pub dirs: Vec<PathBuf>,
    /// How many of its containers are running …
    pub running: u16,
    /// … of how many it has.
    pub total: u16,
}

impl Stack {
    pub fn state(&self) -> StackState {
        if self.running > 0 {
            StackState::Running
        } else {
            StackState::Stopped
        }
    }
}

/// Whether anything in a stack is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackState {
    /// At least one container running.
    Running,
    /// Containers exist, none running.
    Stopped,
}

/// [`parse_ps_state`]'s containers summed per project, sorted by project.
pub fn stacks(rows: &[(Container, bool)]) -> Vec<Stack> {
    let mut map: BTreeMap<&str, Stack> = BTreeMap::new();
    for (c, running) in rows {
        let stack = map.entry(&c.project).or_insert_with(|| Stack {
            project: c.project.clone(),
            dirs: Vec::new(),
            running: 0,
            total: 0,
        });
        if !stack.dirs.contains(&c.working_dir) {
            stack.dirs.push(c.working_dir.clone());
        }
        stack.total = stack.total.saturating_add(1);
        if *running {
            stack.running = stack.running.saturating_add(1);
        }
    }
    map.into_values()
        .map(|mut s| {
            s.dirs.sort();
            s
        })
        .collect()
}

/// What a listing says when the machine has no docker CLI at all.
pub const NO_DOCKER_CLI: &str = "no docker CLI found";

/// The checkout of `checkouts` that `stack` belongs to: the deepest one
/// holding every directory it was started from — a nested checkout's
/// stack is its own, not its parent's. None when no one checkout holds
/// them all (two checkouts sharing a project name, or none orion knows).
pub fn owner_of<'a>(stack: &Stack, checkouts: &[&'a Path]) -> Option<&'a Path> {
    if stack.dirs.is_empty() {
        return None;
    }
    checkouts
        .iter()
        .copied()
        .filter(|c| stack.dirs.iter().all(|d| is_within(d, c)))
        .max_by_key(|c| c.components().count())
}

/// The stack checkout `dir` owns ([`owner_of`] over `checkouts`, `dir`
/// among them) — a running one first when it has several, then the first
/// by name.
pub fn stack_in<'a>(stacks: &'a [Stack], dir: &Path, checkouts: &[&Path]) -> Option<&'a Stack> {
    let mut mine = stacks
        .iter()
        .filter(|s| owner_of(s, checkouts) == Some(dir));
    let first = mine.next()?;
    if first.state() == StackState::Running {
        return Some(first);
    }
    mine.find(|s| s.state() == StackState::Running)
        .or(Some(first))
}

/// What can be done to a stack from the Stacks modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StackVerb {
    /// `docker compose start`: its existing containers, back up.
    Start,
    /// `docker compose stop`: nothing left running, everything kept.
    Stop,
    /// `docker compose down`: containers and networks gone, volumes kept.
    Down,
    /// `docker compose down --volumes`: the project's data goes too.
    DownVolumes,
}

impl StackVerb {
    /// The `docker` arguments that apply this verb to compose `project`.
    /// They need no compose file: compose finds the project by its labels.
    pub fn compose_args(self, project: &str) -> Vec<String> {
        let tail: &[&str] = match self {
            Self::Start => &["start"],
            Self::Stop => &["stop"],
            Self::Down => &["down", "--remove-orphans"],
            Self::DownVolumes => &["down", "--remove-orphans", "--volumes"],
        };
        let head = ["compose", "-p", project];
        head.iter().chain(tail).map(|s| s.to_string()).collect()
    }

    /// The verb, for an error: "couldn't stop …".
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Down | Self::DownVolumes => "take down",
        }
    }

    /// What a row reads while the verb runs.
    pub fn progress(self) -> &'static str {
        match self {
            Self::Start => "starting…",
            Self::Stop => "stopping…",
            Self::Down | Self::DownVolumes => "taking down…",
        }
    }

    /// What was done, for a log line.
    pub fn past_tense(self) -> &'static str {
        match self {
            Self::Start => "started",
            Self::Stop => "stopped",
            Self::Down => "taken down",
            Self::DownVolumes => "taken down with its volumes",
        }
    }
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
    fn ps_state_reads_running_and_skips_foreign_containers() {
        let out = "app\t/src/wt\trunning\napp\t/src/wt\texited\n\t\trunning\nold\t/src/x\n";
        assert_eq!(
            parse_ps_state(out),
            [
                (c("app", "/src/wt"), true),
                (c("app", "/src/wt"), false),
                (c("old", "/src/x"), false),
            ]
        );
    }

    #[test]
    fn stacks_count_per_project_with_their_dirs_once_each() {
        let rows = [
            (c("b", "/src/b"), true),
            (c("a", "/src/a/sub"), false),
            (c("a", "/src/a"), true),
            (c("a", "/src/a"), false),
        ];
        let found = stacks(&rows);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].project, "a");
        assert_eq!(
            found[0].dirs,
            [PathBuf::from("/src/a"), PathBuf::from("/src/a/sub")]
        );
        assert_eq!((found[0].running, found[0].total), (1, 3));
        assert_eq!(found[0].state(), StackState::Running);
        assert_eq!((found[1].running, found[1].total), (1, 1));
    }

    fn stack(project: &str, dirs: &[&str], running: u16) -> Stack {
        Stack {
            project: project.into(),
            dirs: dirs.iter().map(PathBuf::from).collect(),
            running,
            total: 3,
        }
    }

    #[test]
    fn a_checkouts_stack_prefers_a_running_one_and_skips_shared_ones() {
        let all = [
            stack("a-old", &["/src/wt"], 0),
            stack("b-live", &["/src/wt/nested"], 2),
            stack("shared", &["/src/wt", "/src/main"], 3),
            stack("sibling", &["/src/wt-2"], 3),
        ];
        let (wt, nested, main) = (
            Path::new("/src/wt"),
            Path::new("/src/wt/nested"),
            Path::new("/src/main"),
        );
        assert_eq!(stack_in(&all, wt, &[wt]).unwrap().project, "b-live");
        assert_eq!(stack_in(&all[..1], wt, &[wt]).unwrap().project, "a-old");
        assert_eq!(
            stack_in(&all, main, &[wt, main]),
            None,
            "shared is nobody's"
        );
        assert_eq!(
            stack_in(&all, wt, &[]),
            None,
            "a checkout orion doesn't know"
        );
        assert_eq!(
            stack_in(&all, wt, &[wt, nested]).unwrap().project,
            "a-old",
            "the nested checkout's stack is its own"
        );
        assert_eq!(
            stack_in(&all, nested, &[wt, nested]).unwrap().project,
            "b-live"
        );
        assert_eq!(owner_of(&all[2], &[wt, main]), None);
        assert_eq!(owner_of(&all[1], &[wt, nested]), Some(nested));
    }

    #[test]
    fn stack_verbs_name_the_project_and_only_volumes_drop_data() {
        assert_eq!(
            StackVerb::Start.compose_args("p"),
            ["compose", "-p", "p", "start"]
        );
        assert_eq!(
            StackVerb::Stop.compose_args("p"),
            ["compose", "-p", "p", "stop"]
        );
        assert_eq!(
            StackVerb::Down.compose_args("p"),
            ["compose", "-p", "p", "down", "--remove-orphans"]
        );
        assert_eq!(
            StackVerb::DownVolumes.compose_args("p"),
            [
                "compose",
                "-p",
                "p",
                "down",
                "--remove-orphans",
                "--volumes"
            ]
        );
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
