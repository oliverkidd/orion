//! The SESSION SNAPSHOT: `<data_dir>/automations/sessions-by-branch.json`,
//! which sessions sat on which branch of which project. A harvest at merge
//! reads it, so a session's id outlives the worktree it ran in. It also
//! remembers each project's main checkout by name, which is how the CLI
//! finds a project with no TUI running.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use orion_core::clock::DAY_MS;
use serde::{Deserialize, Serialize};

const FILE: &str = "sessions-by-branch.json";
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

/// The sessions of one branch of one project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Branch {
    pub project: String,
    /// The project's main checkout.
    pub root: PathBuf,
    pub branch: String,
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Snapshot {
    /// Project name → its main checkout.
    pub projects: BTreeMap<String, PathBuf>,
    pub branches: Vec<Branch>,
}

/// One session sitting on a branch right now, as the tree shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sighting {
    pub project: String,
    pub root: PathBuf,
    pub branch: String,
    pub session: Session,
}

impl Snapshot {
    pub(crate) fn path() -> PathBuf {
        crate::automations::state_dir().join(FILE)
    }

    pub(crate) fn load_from(path: &Path) -> Snapshot {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub(crate) fn load() -> Snapshot {
        Self::load_from(&Self::path())
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), String> {
        crate::pr_cache::write_json_atomic(path, self).map_err(|e| e.to_string())
    }

    /// Take in what the tree shows now: each project's checkout, and each
    /// session on its branch — a new one added, a known one's time moved.
    /// Branches unseen for [`KEEP_MS`] go. Whether anything changed.
    pub(crate) fn note(
        &mut self,
        projects: &[(String, PathBuf)],
        seen: &[Sighting],
        now: i64,
    ) -> bool {
        let before = self.clone();
        for (name, root) in projects {
            self.projects.insert(name.clone(), root.clone());
        }
        for s in seen {
            let at = match self
                .branches
                .iter()
                .position(|b| b.root == s.root && b.branch == s.branch)
            {
                Some(at) => at,
                None => {
                    self.branches.push(Branch {
                        project: s.project.clone(),
                        root: s.root.clone(),
                        branch: s.branch.clone(),
                        sessions: Vec::new(),
                    });
                    self.branches.len() - 1
                }
            };
            let branch = &mut self.branches[at];
            branch.project = s.project.clone();
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

    /// The sessions that sat on `branch` of the project at `root`.
    pub(crate) fn sessions_on(&self, root: &Path, branch: &str) -> Vec<Session> {
        self.branches
            .iter()
            .filter(|b| b.root == root && b.branch == branch)
            .flat_map(|b| b.sessions.clone())
            .collect()
    }

    /// The main checkout of the project called `name`.
    pub(crate) fn root_of(&self, name: &str) -> Option<PathBuf> {
        self.projects.get(name).cloned()
    }
}

/// What the tree shows: each project, and each session with an id on a
/// worktree's branch — an archived one left out, as it sits nowhere now.
pub(crate) fn sightings(
    tree: &crate::app::Tree,
    now: i64,
) -> (Vec<(String, PathBuf)>, Vec<Sighting>) {
    let projects = tree
        .projects
        .iter()
        .map(|p| (p.name.clone(), p.repo_path.clone()))
        .collect();
    let mut seen = Vec::new();
    for agent in tree.agents.iter().filter(|a| !a.archived) {
        let Some(id) = agent.session_id.as_deref().filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some(worktree) = tree.worktrees.iter().find(|w| w.id == agent.worktree_id) else {
            continue;
        };
        let Some(project) = tree.projects.iter().find(|p| p.id == worktree.project_id) else {
            continue;
        };
        if worktree.branch.is_empty() {
            continue;
        }
        seen.push(Sighting {
            project: project.name.clone(),
            root: project.repo_path.clone(),
            branch: worktree.branch.clone(),
            session: Session {
                id: id.to_string(),
                harness: agent.kind.as_str().to_string(),
                cwd: worktree.path.clone(),
                seen_at: now,
            },
        });
    }
    (projects, seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sighting(branch: &str, id: &str, at: i64) -> Sighting {
        Sighting {
            project: "app".into(),
            root: "/src/app".into(),
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
        let path = dir.path().join(FILE);
        let mut snap = Snapshot::default();
        let projects = vec![("app".to_string(), PathBuf::from("/src/app"))];
        assert!(snap.note(&projects, &[sighting("fix-a", "s1", 1_000)], 1_000));
        assert!(snap.note(&projects, &[sighting("fix-a", "s2", 2_000)], 2_000));
        let drifted = 2_000 + DRIFT_MS;
        assert!(
            !snap.note(
                &projects,
                &[sighting("fix-a", "s2", drifted - 1)],
                drifted - 1
            ),
            "less than a day's drift"
        );
        assert!(
            snap.note(&projects, &[sighting("fix-a", "s2", drifted)], drifted),
            "a day's drift"
        );
        snap.save_to(&path).unwrap();

        // The worktree is gone: nothing sits on the branch any more.
        let mut back = Snapshot::load_from(&path);
        assert!(!back.note(&projects, &[], 5_000));
        let ids: Vec<String> = back
            .sessions_on(Path::new("/src/app"), "fix-a")
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["s1", "s2"]);
        assert_eq!(back.root_of("app"), Some(PathBuf::from("/src/app")));

        assert!(back.note(&projects, &[], drifted + KEEP_MS));
        assert!(back.branches.is_empty(), "long gone branches go");
    }
}
