//! The **build-cache sweep**: orion deletes what Cargo left behind in each
//! checkout's `target/` and hasn't used in two weeks, so a machine that
//! builds Rust in many worktrees doesn't fill its disk.
//!
//! Cargo never deletes anything it built. Every change of features,
//! profile, `-p` selection or toolchain compiles a crate again under a new
//! hash (`deps/liborion_tui-<hash>.rlib`), and the old copy, with its
//! fingerprint, its build-script output and — on macOS, where a debug build
//! leaves its debug info in unpacked `.o` files — gigabytes of object files,
//! stays for good.
//!
//! What counts as "used" is the access time: Cargo reads a unit's
//! fingerprint every time it checks whether the unit is fresh, rustc reads
//! an `.rlib` it links against, and running a test binary reads that
//! binary. So a unit — every file sharing its 16-hex hash across
//! `.fingerprint/`, `build/` and `deps/` — whose newest access or
//! modification is older than the cutoff is deleted whole. An
//! `incremental/` session directory goes the same way on its own clock.
//! Getting it wrong costs a rebuild, never a broken build: Cargo treats a
//! missing output as stale and builds it again.
//!
//! Only a Cargo target directory is touched — `<checkout>/target` carrying
//! Cargo's `CACHEDIR.TAG` — and only its build directories (any with a
//! `.fingerprint/`). One Cargo is building in is skipped: the sweep takes
//! the build directory's `.cargo-lock` the way Cargo does, and a held lock
//! means "busy, next time". The top-level outputs Cargo copies next to
//! `deps/` (`debug/orion`) are never deleted.
//!
//! The sweep itself is plain blocking `std`; [`spawn`] runs it on the
//! blocking pool, which the event loop does a few minutes after start and
//! then once a day.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long a build output may go unused before it is deleted.
pub const MAX_IDLE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How often the sweep re-runs while the TUI is up.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after start the first sweep waits: the tree has arrived from
/// the daemon by then, and the disk walk stays out of the way of start-up.
pub const FIRST_DELAY: Duration = Duration::from_secs(5 * 60);

/// Seconds between sweeps; `0` turns the sweep off. Named here rather than
/// in `orion_core::env`, whose every edit is a daemon release.
pub const INTERVAL_VAR: &str = "ORION_BUILD_SWEEP_SECS";

/// The sweep's cadence: [`INTERVAL_VAR`] when set, `None` when it is `0`
/// (off), else [`DEFAULT_INTERVAL`].
pub fn interval() -> Option<Duration> {
    orion_core::env::secs_override(INTERVAL_VAR, DEFAULT_INTERVAL)
}

/// Run one sweep over these checkouts on the blocking pool. Nothing lands
/// back on the loop: what it freed is logged.
pub fn spawn(checkouts: Vec<PathBuf>) {
    tokio::task::spawn_blocking(move || {
        let report = sweep_checkouts(
            checkouts.iter().map(PathBuf::as_path),
            MAX_IDLE,
            SystemTime::now(),
        );
        if report.removed > 0 || report.busy > 0 {
            tracing::info!(
                "build-cache sweep: freed {} in {} stale builds; {} busy build directories skipped",
                human_bytes(report.freed),
                report.removed,
                report.busy,
            );
        }
    });
}

/// What one sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Bytes deleted.
    pub freed: u64,
    /// Units and incremental directories deleted.
    pub removed: usize,
    /// Build directories skipped because a Cargo held their lock.
    pub busy: usize,
}

impl Report {
    fn add(&mut self, other: Report) {
        self.freed += other.freed;
        self.removed += other.removed;
        self.busy += other.busy;
    }
}

/// Sweep the `target/` of every checkout, deleting what hasn't been used
/// since `now - max_idle`. A path given twice is swept once.
pub fn sweep_checkouts<'a>(
    checkouts: impl IntoIterator<Item = &'a Path>,
    max_idle: Duration,
    now: SystemTime,
) -> Report {
    let cutoff = now.checked_sub(max_idle).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut seen = std::collections::HashSet::new();
    let mut report = Report::default();
    for checkout in checkouts {
        if seen.insert(checkout.to_path_buf()) {
            report.add(sweep_target(&checkout.join("target"), cutoff));
        }
    }
    report
}

/// Sweep one Cargo target directory; anything else is left alone.
pub fn sweep_target(target: &Path, cutoff: SystemTime) -> Report {
    let mut report = Report::default();
    if !is_cargo_target(target) {
        return report;
    }
    for dir in build_dirs(target) {
        report.add(sweep_build_dir(&dir, cutoff));
    }
    report
}

/// Cargo writes a `CACHEDIR.TAG` naming itself into every target directory
/// it creates.
fn is_cargo_target(target: &Path) -> bool {
    fs::read_to_string(target.join("CACHEDIR.TAG"))
        .map(|tag| tag.contains("8a477f597d28d172789f06886806bc55") && tag.contains("cargo"))
        .unwrap_or(false)
}

/// The build directories under a target directory: `debug/`, `release/`,
/// a custom profile's, and those one level further down (a cross target's
/// `aarch64-apple-darwin/debug/`). One is any directory with a
/// `.fingerprint/`.
fn build_dirs(target: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for child in subdirs(target) {
        if child.join(".fingerprint").is_dir() {
            found.push(child);
        } else {
            found.extend(
                subdirs(&child)
                    .into_iter()
                    .filter(|d| d.join(".fingerprint").is_dir()),
            );
        }
    }
    found
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect()
}

/// Sweep one build directory, holding its `.cargo-lock` throughout so no
/// Cargo builds in it meanwhile (one that tries waits for the sweep).
fn sweep_build_dir(dir: &Path, cutoff: SystemTime) -> Report {
    let mut report = Report::default();
    // Held until the function returns; a missing lock file means no Cargo
    // has ever built here to contend with.
    let _lock = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join(".cargo-lock"))
    {
        Ok(file) => match file.try_lock() {
            Ok(()) => Some(file),
            Err(_) => {
                report.busy = 1;
                return report;
            }
        },
        Err(_) => None,
    };

    // Every file of a unit, by hash, with the unit's newest use.
    let mut units: HashMap<String, (SystemTime, Vec<PathBuf>)> = HashMap::new();
    for sub in [".fingerprint", "build", "deps"] {
        let Ok(entries) = fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(hash) = name.to_str().and_then(unit_hash) else {
                continue;
            };
            let path = entry.path();
            let used = last_used(&path);
            let unit = units
                .entry(hash.to_string())
                .or_insert((SystemTime::UNIX_EPOCH, Vec::new()));
            unit.0 = unit.0.max(used);
            unit.1.push(path);
        }
    }
    for (used, paths) in units.into_values() {
        if used < cutoff {
            for path in &paths {
                report.freed += remove(path);
            }
            report.removed += 1;
        }
    }

    // rustc's incremental state, one directory per crate, on its own clock.
    for crate_dir in subdirs(&dir.join("incremental")) {
        if last_used(&crate_dir) < cutoff {
            report.freed += remove(&crate_dir);
            report.removed += 1;
        }
    }
    report
}

/// The unit hash in a build file's name — the 16 hex digits after the
/// first `-` that are followed by the end of the name or a `.`:
/// `orion-tui-cda670e0e1fea6b5`, `liborion_tui-cda670e0e1fea6b5.rlib`,
/// `orion_tui-cda670e0e1fea6b5.orion_tui.4f2c-cgu.0.rcgu.o`.
pub fn unit_hash(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    for (i, _) in name.match_indices('-') {
        let start = i + 1;
        let end = start + 16;
        if end > bytes.len() {
            break;
        }
        let hex = &bytes[start..end];
        let boundary = end == bytes.len() || bytes[end] == b'.';
        if boundary
            && hex
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        {
            return Some(&name[start..end]);
        }
    }
    None
}

/// The newest access or modification of a path, and of everything under
/// it when it is a directory. Symlinks are not followed.
fn last_used(path: &Path) -> SystemTime {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return SystemTime::UNIX_EPOCH;
    };
    let mut newest = stamp(&meta);
    if meta.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                newest = newest.max(last_used(&entry.path()));
            }
        }
    }
    newest
}

/// A file's newest access or modification; a directory's modification
/// only — listing one (as this sweep does) bumps its access time, which
/// would otherwise make every directory look used the day it was swept.
fn stamp(meta: &fs::Metadata) -> SystemTime {
    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if meta.is_dir() {
        return modified;
    }
    let accessed = meta.accessed().unwrap_or(SystemTime::UNIX_EPOCH);
    modified.max(accessed)
}

/// Delete a file or directory tree, answering how many bytes it held.
fn remove(path: &Path) -> u64 {
    let size = size_of(path);
    let gone = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(_) => return 0,
    };
    if gone.is_ok() {
        size
    } else {
        0
    }
}

/// The disk a path's deletion gives back. A file with another hard link
/// gives back nothing — Cargo links its top-level outputs (`debug/orion`)
/// to their `deps/` copies, and the other link keeps the blocks.
fn size_of(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return if meta.nlink() > 1 {
            0
        } else {
            meta.blocks() * 512
        };
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| size_of(&e.path())).sum())
        .unwrap_or(0)
}

/// `12.3 GB`, `450 MB`: a byte count for the sweep's log line.
pub fn human_bytes(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

    fn touch(path: &Path, when: SystemTime) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_accessed(when).set_modified(when))
            .unwrap();
    }

    fn set_dir_time(path: &Path, when: SystemTime) {
        let dir = fs::File::open(path).unwrap();
        dir.set_times(fs::FileTimes::new().set_accessed(when).set_modified(when))
            .unwrap();
    }

    #[test]
    fn hashes_are_read_off_every_kind_of_build_file() {
        assert_eq!(
            unit_hash("orion-tui-cda670e0e1fea6b5"),
            Some("cda670e0e1fea6b5")
        );
        assert_eq!(
            unit_hash("liborion_tui-cda670e0e1fea6b5.rlib"),
            Some("cda670e0e1fea6b5")
        );
        assert_eq!(
            unit_hash("orion_tui-cda670e0e1fea6b5.orion_tui.41c8fd17ac5fe4e2-cgu.0.rcgu.o"),
            Some("cda670e0e1fea6b5")
        );
        assert_eq!(
            unit_hash("orion-62927a4f3a1381ff"),
            Some("62927a4f3a1381ff")
        );
        assert_eq!(unit_hash("liborion_tui.rlib"), None);
        assert_eq!(unit_hash("orion"), None);
        assert_eq!(unit_hash("foo-cda670e0e1fea6b5x"), None);
        assert_eq!(unit_hash("foo-CDA670E0E1FEA6B5"), None);
    }

    #[test]
    fn a_unit_unused_past_the_cutoff_goes_whole_and_a_used_one_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let debug = target.join("debug");
        fs::create_dir_all(&debug).unwrap();
        fs::write(target.join("CACHEDIR.TAG"), TAG).unwrap();
        fs::write(debug.join(".cargo-lock"), b"").unwrap();

        let now = SystemTime::now();
        let old = now - Duration::from_secs(30 * 24 * 60 * 60);
        let recent = now - Duration::from_secs(60 * 60);

        // A stale unit: fingerprint, rlib and object files.
        touch(
            &debug.join(".fingerprint/orion-tui-aaaaaaaaaaaaaaaa/lib-orion_tui"),
            old,
        );
        touch(&debug.join("deps/liborion_tui-aaaaaaaaaaaaaaaa.rlib"), old);
        touch(
            &debug.join("deps/orion_tui-aaaaaaaaaaaaaaaa.orion_tui.x-cgu.0.rcgu.o"),
            old,
        );
        // A unit whose files are old but whose fingerprint Cargo just read.
        touch(
            &debug.join(".fingerprint/orion-tui-bbbbbbbbbbbbbbbb/lib-orion_tui"),
            recent,
        );
        touch(&debug.join("deps/liborion_tui-bbbbbbbbbbbbbbbb.rlib"), old);
        // The top-level copy, never touched.
        touch(&debug.join("liborion_tui.rlib"), old);
        // Incremental state, one stale and one fresh.
        touch(
            &debug.join("incremental/orion_tui-0stale/s-1/dep-graph.bin"),
            old,
        );
        touch(
            &debug.join("incremental/orion_tui-1fresh/s-1/dep-graph.bin"),
            recent,
        );
        for dir in [
            ".fingerprint/orion-tui-aaaaaaaaaaaaaaaa",
            ".fingerprint/orion-tui-bbbbbbbbbbbbbbbb",
            "incremental/orion_tui-0stale/s-1",
            "incremental/orion_tui-0stale",
        ] {
            set_dir_time(&debug.join(dir), old);
        }

        let report = sweep_checkouts([tmp.path()], MAX_IDLE, now);

        assert_eq!(report.removed, 2);
        assert_eq!(report.busy, 0);
        assert!(report.freed > 0);
        assert!(!debug
            .join(".fingerprint/orion-tui-aaaaaaaaaaaaaaaa")
            .exists());
        assert!(!debug
            .join("deps/liborion_tui-aaaaaaaaaaaaaaaa.rlib")
            .exists());
        assert!(!debug
            .join("deps/orion_tui-aaaaaaaaaaaaaaaa.orion_tui.x-cgu.0.rcgu.o")
            .exists());
        assert!(debug
            .join("deps/liborion_tui-bbbbbbbbbbbbbbbb.rlib")
            .exists());
        assert!(debug.join("liborion_tui.rlib").exists());
        assert!(!debug.join("incremental/orion_tui-0stale").exists());
        assert!(debug.join("incremental/orion_tui-1fresh").exists());
    }

    #[test]
    fn a_directory_without_cargos_tag_is_never_touched() {
        let tmp = tempfile::tempdir().unwrap();
        let old = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
        let rlib = tmp
            .path()
            .join("target/debug/deps/libx-aaaaaaaaaaaaaaaa.rlib");
        touch(
            &tmp.path()
                .join("target/debug/.fingerprint/x-aaaaaaaaaaaaaaaa/lib-x"),
            old,
        );
        touch(&rlib, old);

        let report = sweep_checkouts([tmp.path()], MAX_IDLE, SystemTime::now());

        assert_eq!(report, Report::default());
        assert!(rlib.exists());
    }

    #[test]
    fn a_build_directory_cargo_is_building_in_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let debug = target.join("debug");
        let old = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
        let rlib = debug.join("deps/libx-aaaaaaaaaaaaaaaa.rlib");
        touch(&debug.join(".fingerprint/x-aaaaaaaaaaaaaaaa/lib-x"), old);
        touch(&rlib, old);
        fs::write(target.join("CACHEDIR.TAG"), TAG).unwrap();
        fs::write(debug.join(".cargo-lock"), b"").unwrap();
        let held = fs::File::options()
            .read(true)
            .write(true)
            .open(debug.join(".cargo-lock"))
            .unwrap();
        held.lock().unwrap();

        let report = sweep_checkouts([tmp.path()], MAX_IDLE, SystemTime::now());

        assert_eq!(report.busy, 1);
        assert_eq!(report.removed, 0);
        assert!(rlib.exists());
    }
}
