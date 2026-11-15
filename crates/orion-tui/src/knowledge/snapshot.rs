//! The SESSION SNAPSHOT: `<data_dir>/knowledge/<project id>/snapshot.json`,
//! which sessions sat on which branch of the project. A harvest at merge
//! reads it, so a session's id outlives the worktree it ran in. It also
//! keeps the project's name and main checkout, which is how the CLI finds
//! a project with no TUI running ([`known`]).

use std::path::{Path, PathBuf};

use orion_core::clock::DAY_MS;
use serde::{Deserialize, Serialize};

use super::local::Local;

/// Where the snapshot of every project was kept before each project had
/// its own: read once, for a project that has none yet.
const OLD_FILE: &str = "sessions-by-branch.json";
/// How long a branch is kept after its last session was seen.
const KEEP_MS: i64 = 180 * DAY_MS;
/// How far a known session's last sighting may drift before it is worth
/// a write: a branch is kept for months ([`KEEP_MS`]), so a day is nothing.
const DRIFT_MS: i64 = DAY_MS;

/// One session seen on a branch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Session {
    /// The CLI's session id (what `--resume` takes).
    pub id: String,
    /// `claude`, `cursor`, `codex` …
    pub harness: String,
    pub cwd: PathBuf,
    /// Epoch ms it was last seen on the branch.
    pub seen_at: i64,
}

/// The sessions of one branch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Branch {
    pub branch: String,
    pub sessions: Vec<Session>,
}

/// One project's snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Snapshot {
    /// The project's name.
    pub name: String,
    /// Its main checkout.
    pub root: PathBuf,
    pub branches: Vec<Branch>,
}

/// One session sitting on a branch right now, as the tree shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sighting {
    pub branch: String,
    pub session: Session,
}

/// One project as the tree shows it, with its sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub sightings: Vec<Sighting>,
}

impl Snapshot {
    pub(crate) fn load_from(path: &Path) -> Option<Snapshot> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
    }

    /// `local`'s snapshot: its file, else what the old shared one kept of
    /// the project at `root`.
    pub(crate) fn load(local: &Local, root: &Path) -> Snapshot {
        Self::load_from(&local.snapshot_path()).unwrap_or_else(|| Snapshot {
            root: root.to_path_buf(),
            branches: old_branches(&crate::automations::state_dir().join(OLD_FILE), root),
            ..Snapshot::default()
        })
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), String> {
        crate::pr_cache::write_json_atomic(path, self).map_err(|e| e.to_string())
    }

    /// Take in what the tree shows now: the project's name and checkout,
    /// and each session on its branch — a new one added, a known one's
    /// time moved. Branches unseen for [`KEEP_MS`] go. Whether anything
    /// changed.
    pub(crate) fn note(&mut self, name: &str, root: &Path, seen: &[Sighting], now: i64) -> bool {
        let before = self.clone();
        self.name = name.to_string();
        self.root = root.to_path_buf();
        for s in seen {
            let at = match self.branches.iter().position(|b| b.branch == s.branch) {
                Some(at) => at,
                None => {
                    self.branches.push(Branch {
                        branch: s.branch.clone(),
                        sessions: Vec::new(),
                    });
                    self.branches.len() - 1
                }
            };
            let branch = &mut self.branches[at];
            match branch.sessions.iter_mut().find(|x| x.id == s.session.id) {
                Some(known) if now - known.seen_at < DRIFT_MS && known.cwd == s.session.cwd => {}
                Some(known) => *known = s.session.clone(),
                None => branch.sessions.push(s.session.clone()),
            }
        }
        self.branches.retain(|b| {
            b.sessions
                .iter()
                .map(|s| s.seen_at)
                .max()
                .is_some_and(|last| now - last < KEEP_MS)
        });
        *self != before
    }

    /// The sessions that sat on `branch`.
    pub(crate) fn sessions_on(&self, branch: &str) -> Vec<Session> {
        self.branches
            .iter()
            .filter(|b| b.branch == branch)
            .flat_map(|b| b.sessions.clone())
            .collect()
    }
}

/// The branches the old shared snapshot at `path` kept for the project
/// at `root`.
fn old_branches(path: &Path, root: &Path) -> Vec<Branch> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct OldBranch {
        root: PathBuf,
        branch: String,
        sessions: Vec<Session>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Old {
        branches: Vec<OldBranch>,
    }
    let old: Old = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    old.branches
        .into_iter()
        .filter(|b| b.root == root)
        .map(|b| Branch {
            branch: b.branch,
            sessions: b.sessions,
        })
        .collect()
}

/// Every project orion has a snapshot of: its folder and snapshot.
pub(crate) fn known() -> Vec<(Local, Snapshot)> {
    Local::all()
        .into_iter()
        .filter_map(|local| {
            let snap = Snapshot::load_from(&local.snapshot_path())?;
            Some((local, snap))
        })
        .collect()
}

/// What the tree shows: each project, with each session with an id on
/// one of its worktrees' branches — an archived one left out, as it sits
/// nowhere now.
pub(crate) fn sightings(tree: &crate::app::Tree, now: i64) -> Vec<Seen> {
    let mut seen: Vec<Seen> = tree
        .projects
        .iter()
        .map(|p| Seen {
            id: p.id.to_string(),
            name: p.name.clone(),
            root: p.repo_path.clone(),
            sightings: Vec::new(),
        })
        .collect();
    for agent in tree.agents.iter().filter(|a| !a.archived) {
        let Some(id) = agent.session_id.as_deref().filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some(worktree) = tree.worktrees.iter().find(|w| w.id == agent.worktree_id) else {
            continue;
        };
        if worktree.branch.is_empty() {
            continue;
        }
        let Some(project) = seen
            .iter_mut()
            .find(|p| p.id == worktree.project_id.as_str())
        else {
            continue;
        };
        project.sightings.push(Sighting {
            branch: worktree.branch.clone(),
            session: Session {
                id: id.to_string(),
                harness: agent.kind.as_str().to_string(),
                cwd: worktree.path.clone(),
                seen_at: now,
            },
        });
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sighting(branch: &str, id: &str, at: i64) -> Sighting {
        Sighting {
            branch: branch.into(),
            session: Session {
                id: id.into(),
                harness: "claude".into(),
                cwd: format!("/wt/{branch}").into(),
                seen_at: at,
            },
        }
    }

    #[test]
    fn sessions_are_kept_by_branch_past_their_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::at("01P", dir.path());
        let root = Path::new("/src/app");
        let mut snap = Snapshot::default();
        assert!(snap.note("app", root, &[sighting("fix-a", "s1", 1_000)], 1_000));
        assert!(snap.note("app", root, &[sighting("fix-a", "s2", 2_000)], 2_000));
        let drifted = 2_000 + DRIFT_MS;
        assert!(
            !snap.note(
                "app",
                root,
                &[sighting("fix-a", "s2", drifted - 1)],
                drifted - 1
            ),
            "less than a day's drift"
        );
        assert!(
            snap.note("app", root, &[sighting("fix-a", "s2", drifted)], drifted),
            "a day's drift"
        );
        snap.save_to(&local.snapshot_path()).unwrap();

        // The worktree is gone: nothing sits on the branch any more.
        let mut back = Snapshot::load(&local, root);
        assert!(!back.note("app", root, &[], 5_000));
        let ids: Vec<String> = back
            .sessions_on("fix-a")
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["s1", "s2"]);
        assert_eq!((back.name.as_str(), back.root.as_path()), ("app", root));

        assert!(back.note("app", root, &[], drifted + KEEP_MS));
        assert!(back.branches.is_empty(), "long gone branches go");
    }

    #[test]
    fn the_old_shared_snapshot_seeds_each_project_once() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(OLD_FILE);
        std::fs::write(
            &old,
            r#"{"projects": {"app": "/src/app"}, "branches": [
                {"project": "app", "root": "/src/app", "branch": "fix-a", "sessions": [{"id": "s1", "harness": "claude", "cwd": "/wt/fix-a", "seen_at": 1}]},
                {"project": "web", "root": "/src/web", "branch": "fix-b", "sessions": [{"id": "s9"}]}
            ]}"#,
        )
        .unwrap();
        let branches = old_branches(&old, Path::new("/src/app"));
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].branch, "fix-a");
        assert_eq!(branches[0].sessions[0].id, "s1");
    }
}
