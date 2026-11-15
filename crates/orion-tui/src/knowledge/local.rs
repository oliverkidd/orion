//! What stays on this machine: `<data_dir>/knowledge/<project id>/`, one
//! folder per project, never in the repo.
//!
//! - `sources/<pr>.md`: the condensed dossier a harvest read;
//! - `queue.json`: the merged pull requests waiting to be harvested;
//! - `state.json`: the cards' memory, and the knowledge pull request;
//! - `snapshot.json`: which sessions sat on which branch (`super::snapshot`);
//! - `repo/`: orion's private working clone, where the harvested pages are
//!   changed, committed and pushed (`super::share`).
//! - `lock`: held while anything works in `repo/`, by this process or
//!   another (the CLI's backfill).

use std::path::{Path, PathBuf};

/// A project's folder of local knowledge material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Local {
    /// The project's id, as the daemon keeps it.
    pub id: String,
    pub dir: PathBuf,
}

/// `<data_dir>/knowledge`.
pub(crate) fn root() -> PathBuf {
    orion_core::paths::data_dir().join("knowledge")
}

impl Local {
    /// Project `id`'s folder under the data dir.
    pub(crate) fn of(id: &str) -> Local {
        Local::at(id, &root().join(id))
    }

    /// Project `id`'s folder at `dir` (a test's).
    pub(crate) fn at(id: &str, dir: &Path) -> Local {
        Local {
            id: id.to_string(),
            dir: dir.to_path_buf(),
        }
    }

    /// Every project with a folder, by id.
    pub(crate) fn all() -> Vec<Local> {
        let mut all: Vec<Local> = std::fs::read_dir(root())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| Local::at(&e.file_name().to_string_lossy(), &e.path()))
            .collect();
        all.sort_by(|a, b| a.id.cmp(&b.id));
        all
    }

    /// orion's working clone of the repo.
    pub(crate) fn clone_dir(&self) -> PathBuf {
        self.dir.join("repo")
    }

    pub(crate) fn queue_path(&self) -> PathBuf {
        self.dir.join("queue.json")
    }

    pub(crate) fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub(crate) fn snapshot_path(&self) -> PathBuf {
        self.dir.join("snapshot.json")
    }

    pub(crate) fn source_path(&self, pr: u64) -> PathBuf {
        self.dir.join("sources").join(format!("{pr}.md"))
    }

    /// Keep the dossier pull request `pr`'s entries came from.
    pub(crate) fn write_source(&self, pr: u64, dossier: &str) -> Result<(), String> {
        let path = self.source_path(pr);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, dossier)
            .map_err(|e| format!("couldn't write {}: {e}", path.display()))
    }

    /// Whether this machine harvested pull request `pr`.
    pub(crate) fn has_source(&self, pr: u64) -> bool {
        self.source_path(pr).exists()
    }

    /// Hold the project's lock until the guard drops: one writer in the
    /// working clone at a time, across processes. Waits off the runtime's
    /// threads.
    pub(crate) async fn lock(&self) -> Result<Lock, String> {
        let path = self.dir.join("lock");
        tokio::task::spawn_blocking(move || Lock::take(&path))
            .await
            .map_err(|e| e.to_string())?
    }
}

/// An exclusive `flock` on a project's lock file, let go when dropped (or
/// when the process ends).
#[derive(Debug)]
pub(crate) struct Lock(#[allow(dead_code)] std::fs::File);

extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}
const LOCK_EX: i32 = 2;

impl Lock {
    fn take(path: &Path) -> Result<Lock, String> {
        use std::os::fd::AsRawFd as _;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| format!("couldn't open {}: {e}", path.display()))?;
        // SAFETY: flock only reads the descriptor, which `file` keeps open.
        loop {
            if unsafe { flock(file.as_raw_fd(), LOCK_EX) } == 0 {
                return Ok(Lock(file));
            }
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("couldn't lock {}: {err}", path.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_stay_in_the_projects_folder() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::at("01P", dir.path());
        assert!(!local.has_source(12));
        local.write_source(12, "dossier").unwrap();
        assert!(local.has_source(12));
        assert_eq!(local.source_path(12), dir.path().join("sources/12.md"));
        assert_eq!(local.clone_dir(), dir.path().join("repo"));
    }

    #[tokio::test]
    async fn the_lock_lets_one_holder_in_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::at("01P", dir.path());
        let held = local.lock().await.unwrap();
        let second = {
            let local = local.clone();
            tokio::spawn(async move { local.lock().await.map(|_| ()) })
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!second.is_finished(), "waits while the first is held");
        drop(held);
        second.await.unwrap().unwrap();
    }
}
