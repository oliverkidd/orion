//! BACKGROUND DEMAND: what an orion process fetches with nobody looking,
//! because an automation acts on it without you — and nothing else.
//!
//! Displayed data refreshes when you look at it (`scope`); a key appears
//! here only while an automation that reads it is on, at the period that
//! automation needs, and only in the one orion on the machine that holds
//! `sync.lock` ([`Owner`]) — a second `orion` in another terminal fetches
//! what it shows and leaves the automations' reads to the first.

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use super::QueryKey;
use crate::app::App;

/// What the automations need fetched, each with how often: GitHub's
/// (`MyPrs` and the pages AUTOFIX and the factory asked for, while one of
/// them is on — `github::needs`).
pub fn needs(app: &App) -> Vec<(QueryKey, Duration)> {
    let mut needs = Vec::new();
    needs.extend(super::github::needs(app));
    needs.extend(super::linear::needs(app));
    needs.extend(super::services::needs(app));
    needs
}

/// The lock that makes this process the machine's one runner of
/// background demand: an exclusive `flock` on `<data dir>/sync.lock`,
/// held for as long as the process lives (the kernel lets go of it when
/// the process ends, however it ends).
#[derive(Debug)]
pub struct Owner {
    _lock: File,
}

impl Owner {
    /// Take the lock at `path` if no other process holds it. `None` when
    /// another does, or the file can't be opened — then this process
    /// fetches only what it shows, and may try again later.
    pub fn claim(path: &Path) -> Option<Owner> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .ok()?;
        file.try_lock().ok()?;
        Some(Owner { _lock: file })
    }

    /// Where the lock lives.
    pub fn default_location() -> std::path::PathBuf {
        orion_core::paths::data_dir().join("sync.lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One holder at a time; once it lets go, the next claim gets it.
    #[test]
    fn one_process_owns_background_demand() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("sync.lock");
        let first = Owner::claim(&path).expect("free: claimed");
        assert!(Owner::claim(&path).is_none(), "held: another must wait");
        drop(first);
        // A test running beside this one may fork a `git` at the moment
        // the lock was held: until that child execs, its copy of the
        // descriptor keeps the lock. Gone within milliseconds.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let again = loop {
            match Owner::claim(&path) {
                Some(owner) => break Some(owner),
                None if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                None => break None,
            }
        };
        assert!(again.is_some(), "let go: claimable again");
    }

    /// With every automation off, nothing is fetched with nobody looking.
    #[test]
    fn nothing_is_fetched_in_the_background_with_the_automations_off() {
        let mut app = App::new();
        for a in crate::automations::Automation::ALL {
            app.automations.settings.of_mut(a).on = Some(false);
        }
        assert!(needs(&app).is_empty());
    }
}
