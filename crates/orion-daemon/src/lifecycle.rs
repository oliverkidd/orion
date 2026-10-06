//! Daemon lifecycle: pidfile + flock liveness, socket path hygiene,
//! auto-spawn from the client side.

use anyhow::{Context, Result};
use orion_core::paths;
use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How often a running daemon re-asserts its pidfile and buildstamp (see
/// [`PidfileLock::refresh`]). Far inside the three days macOS's cleaner
/// waits, and quick to repair a file something else deleted.
pub const RUNTIME_FILE_REFRESH: Duration = Duration::from_secs(60 * 60);

/// Guard holding the exclusive pidfile flock for the daemon's lifetime.
/// Lock possession — not file existence — is the liveness test.
pub struct PidfileLock {
    file: std::fs::File,
    path: PathBuf,
}

impl PidfileLock {
    /// Try to acquire the daemon lock. Returns None when another live daemon
    /// holds it.
    pub fn try_acquire() -> Result<Option<Self>> {
        ensure_runtime_dir()?;
        Self::acquire_at(&paths::pidfile_path())
    }

    fn acquire_at(path: &Path) -> Result<Option<Self>> {
        // No truncate: the flock decides ownership, content is informational.
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open pidfile {}", path.display()))?;
        let ret = libc_flock(file.as_raw_fd());
        if ret != 0 {
            return Ok(None);
        }
        // Informational only; liveness is the lock.
        let _ = fs::write(path, format!("{}\n", std::process::id()));
        Ok(Some(Self {
            file,
            path: path.to_path_buf(),
        }))
    }

    /// Keep the pidfile where clients look for it for as long as the daemon
    /// runs. The default runtime dir is under /tmp, and macOS's tmp_cleaner
    /// deletes regular files there untouched for three days — sparing the
    /// socket beside them — so a daemon left up over a long weekend would
    /// lose the file every liveness check and `orion kill` starts from
    /// (#68). A file still in place is touched so the cleaner passes it by;
    /// one already gone or replaced is re-created and locked again. False
    /// when another daemon has taken the path since: it owns the runtime
    /// files now, and this one must leave them alone.
    pub fn refresh(&mut self) -> bool {
        if self.still_at_path() {
            let _ = self.file.set_modified(SystemTime::now());
            return true;
        }
        match Self::acquire_at(&self.path) {
            Ok(Some(fresh)) => {
                *self = fresh;
                true
            }
            Ok(None) | Err(_) => false,
        }
    }

    /// Whether the file at the pidfile path is the one this guard locked.
    fn still_at_path(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(&self.path), self.file.metadata()) {
            (Ok(on_disk), Ok(held)) => on_disk.dev() == held.dev() && on_disk.ino() == held.ino(),
            _ => false,
        }
    }

    pub fn is_daemon_alive() -> bool {
        match Self::try_acquire() {
            // We got the lock: nobody holds it. Release immediately by drop.
            Ok(Some(_guard)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }
}

impl Drop for PidfileLock {
    fn drop(&mut self) {
        // flock released automatically on close; keep for explicitness.
        let _ = self.file.as_raw_fd();
    }
}

fn libc_flock(fd: i32) -> i32 {
    extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe { flock(fd, LOCK_EX | LOCK_NB) }
}

/// The DAEMON's build: a hash of the sources it is built from, baked in by
/// build.rs (see `daemon-inputs.txt`). Releases that leave the daemon's code
/// alone share it, so the daemon they find running is already theirs.
pub const BUILD_FINGERPRINT: &str = env!("ORION_DAEMON_FINGERPRINT");

/// Record this daemon's build so installers can tell whether the running
/// daemon is already on the build they just installed. Called by the daemon
/// at startup; best-effort (staleness checks treat a missing stamp as
/// "unknown build", which reads as stale). Returns the stamp for
/// [`rewrite_buildstamp`].
pub fn write_buildstamp() -> Option<String> {
    rewrite_buildstamp(BUILD_FINGERPRINT);
    Some(BUILD_FINGERPRINT.to_string())
}

/// Put the startup stamp back, on the pidfile's refresh tick and for the same
/// cleaner. Always the stamp this process started with: after an upgrade
/// the file at `current_exe` is the new build, and its stamp would pass a
/// stale daemon off as fresh.
pub fn rewrite_buildstamp(stamp: &str) {
    let _ = fs::write(paths::buildstamp_path(), stamp);
}

/// True when a live daemon is running different daemon code than this
/// binary — or predates source stamps, so its build is unknown.
pub fn daemon_is_stale() -> bool {
    PidfileLock::is_daemon_alive() && !daemon_runs(BUILD_FINGERPRINT)
}

/// Whether a live daemon is running the build stamped `fingerprint`. A
/// daemon from before source stamps recorded a hash of its whole binary,
/// which no fingerprint equals, so it never passes for current.
pub fn daemon_runs(fingerprint: &str) -> bool {
    PidfileLock::is_daemon_alive()
        && fs::read_to_string(paths::buildstamp_path())
            .is_ok_and(|recorded| recorded.trim() == fingerprint.trim())
}

/// Create the runtime dir with 0700 perms — this is the auth boundary.
pub fn ensure_runtime_dir() -> Result<()> {
    let dir = paths::runtime_dir();
    if !dir.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("create runtime dir {}", dir.display()))?;
    } else {
        let meta = fs::metadata(&dir)?;
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o700 {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Remove a socket file left behind by a dead daemon.
pub fn unlink_stale_socket(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    // #68: once the cleaner has deleted the pidfile, a client finds no
    // daemon at all. The refresh has to bring back a file that is both
    // readable and locked, or liveness checks still read "none running".
    #[test]
    fn refresh_recreates_and_relocks_a_deleted_pidfile() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("daemon.pid");
        let mut lock = PidfileLock::acquire_at(&path)
            .unwrap()
            .expect("lock is free");
        fs::remove_file(&path).unwrap();

        assert!(lock.refresh(), "still this daemon's pidfile");

        assert_eq!(
            fs::read_to_string(&path).unwrap().trim(),
            std::process::id().to_string()
        );
        assert!(
            PidfileLock::acquire_at(&path).unwrap().is_none(),
            "the re-created pidfile must be locked"
        );
    }

    #[test]
    fn refresh_touches_the_pidfile_it_still_holds() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("daemon.pid");
        let mut lock = PidfileLock::acquire_at(&path)
            .unwrap()
            .expect("lock is free");
        // Aged past the cleaner's three days.
        let week_ago = SystemTime::now() - Duration::from_secs(7 * 24 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(week_ago)
            .unwrap();
        let inode = fs::metadata(&path).unwrap().ino();

        assert!(lock.refresh());

        let meta = fs::metadata(&path).unwrap();
        assert!(meta.modified().unwrap() > week_ago + Duration::from_secs(24 * 60 * 60));
        assert_eq!(meta.ino(), inode, "touched in place, not re-created");
        assert!(PidfileLock::acquire_at(&path).unwrap().is_none());
    }

    // A second daemon that took the path after the first lost its file owns
    // the runtime files; the first must neither steal the pidfile back nor
    // overwrite that daemon's buildstamp with its own.
    #[test]
    fn refresh_yields_a_pidfile_another_daemon_holds() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("daemon.pid");
        let mut first = PidfileLock::acquire_at(&path)
            .unwrap()
            .expect("lock is free");
        fs::remove_file(&path).unwrap();
        let _second = PidfileLock::acquire_at(&path)
            .unwrap()
            .expect("path is free again");

        assert!(!first.refresh());
        assert!(PidfileLock::acquire_at(&path).unwrap().is_none());
    }
}
