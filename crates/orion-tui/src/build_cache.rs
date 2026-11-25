//! The **build-cache sweep**: orion deletes the builds Cargo has left
//! behind in each checkout's `target/` once newer ones have replaced them,
//! so a machine that builds Rust in many worktrees doesn't fill its disk.
//!
//! Cargo never deletes anything it built. Every change of features,
//! profile, `-p` selection, toolchain or version compiles a crate again
//! under a new hash (`deps/liborion_tui-<hash>.rlib`), and the old copy,
//! with its fingerprint, its build-script output and — on macOS, where a
//! debug build leaves its debug info in unpacked `.o` files — gigabytes of
//! object files, stays for good. orion's own releases re-hash every
//! workspace crate, so a day of work leaves tens of GB of dead builds.
//!
//! A **unit** is every file sharing one 16-hex hash across `.fingerprint/`,
//! `build/` and `deps/`. Units are grouped by crate and by kind — a linked
//! library, a check's metadata, a binary or test binary, a build script, a
//! build script's run — so `cargo build`, `cargo test` and `cargo check`
//! never evict each other's live builds. In each group the [`KEEP`] most
//! recently used units stay and the rest are deleted whole, however new.
//! "Used" is the newest access or modification: Cargo reads a unit's
//! fingerprint every time it checks whether the unit is fresh, rustc reads
//! an `.rlib` it links against, and running a test binary reads that
//! binary. An `incremental/` session directory is ranked the same way
//! among its crate's, keeping [`KEEP_INCREMENTAL`]. Getting it wrong costs
//! a rebuild, never a broken build: Cargo treats a missing output as stale
//! and builds it again.
//!
//! Only a Cargo target directory is touched — `<checkout>/target` carrying
//! Cargo's `CACHEDIR.TAG` — and only its build directories (any with a
//! `.fingerprint/`). One Cargo is building in is skipped: the sweep takes
//! the build directory's `.cargo-lock` the way Cargo does, and a held lock
//! means "busy, next time". The top-level outputs Cargo copies next to
//! `deps/` (`debug/orion`) are never deleted.
//!
//! The sweep itself is plain blocking `std`; [`spawn`] runs it on the
//! blocking pool, which the event loop does a few minutes after start —
//! so every upgrade's reopen runs one — and then once a day.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How many builds of each crate, of each kind, the sweep keeps: the
/// current one and the one before it, so switching between two feature
/// sets or branches doesn't rebuild on every switch.
pub const KEEP: usize = 2;

/// How many `incremental/` session directories of each crate the sweep
/// keeps. A crate has one per build, test, check and clippy configuration,
/// and losing one only makes the next compile of that crate a full one.
pub const KEEP_INCREMENTAL: usize = 4;

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
        let report = sweep_checkouts(checkouts.iter().map(PathBuf::as_path));
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

/// Sweep the `target/` of every checkout, deleting every build a newer one
/// has replaced. A path given twice is swept once.
pub fn sweep_checkouts<'a>(checkouts: impl IntoIterator<Item = &'a Path>) -> Report {
    let mut seen = std::collections::HashSet::new();
    let mut report = Report::default();
    for checkout in checkouts {
        if seen.insert(checkout.to_path_buf()) {
            report.add(sweep_target(&checkout.join("target")));
        }
    }
    report
}

/// Sweep one Cargo target directory; anything else is left alone.
pub fn sweep_target(target: &Path) -> Report {
    let mut report = Report::default();
    if !is_cargo_target(target) {
        return report;
    }
    for dir in build_dirs(target) {
        report.add(sweep_build_dir(&dir));
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
fn sweep_build_dir(dir: &Path) -> Report {
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

    // Every file of a unit, by hash.
    let mut units: HashMap<String, Unit> = HashMap::new();
    for sub in [".fingerprint", "build", "deps"] {
        let Ok(entries) = fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(hash) = unit_hash(name) else {
                continue;
            };
            let path = entry.path();
            let unit = units.entry(hash.to_string()).or_default();
            unit.used = unit.used.max(last_used(&path));
            unit.note(sub, name, hash, &path);
            unit.paths.push(path);
        }
    }
    let mut groups: HashMap<(String, Kind), Builds<Vec<PathBuf>>> = HashMap::new();
    for unit in units.into_values() {
        groups
            .entry((unit.crate_name, unit.kind))
            .or_default()
            .push((unit.used, unit.paths));
    }
    for builds in groups.into_values() {
        for paths in superseded(builds, KEEP) {
            for path in &paths {
                report.freed += remove(path);
            }
            report.removed += 1;
        }
    }

    // rustc's incremental state, one directory per crate and configuration
    // (`orion_tui-<13 base-36 digits>`), ranked among its crate's.
    let mut sessions: HashMap<String, Builds<PathBuf>> = HashMap::new();
    for crate_dir in subdirs(&dir.join("incremental")) {
        let Some(name) = crate_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((crate_name, _)) = name.rsplit_once('-') else {
            continue;
        };
        sessions
            .entry(crate_name.to_string())
            .or_default()
            .push((last_used(&crate_dir), crate_dir));
    }
    for builds in sessions.into_values() {
        for crate_dir in superseded(builds, KEEP_INCREMENTAL) {
            report.freed += remove(&crate_dir);
            report.removed += 1;
        }
    }
    report
}

/// One group's builds, each with its newest use.
type Builds<T> = Vec<(SystemTime, T)>;

/// All but the `keep` most recently used of one group's builds.
fn superseded<T>(mut builds: Builds<T>, keep: usize) -> Vec<T> {
    builds.sort_by_key(|b| std::cmp::Reverse(b.0));
    builds.into_iter().skip(keep).map(|(_, b)| b).collect()
}

/// What one build of one crate is, read off its files — the sweep keeps
/// [`KEEP`] of each, so one kind's builds never push out another's.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Kind {
    /// Only a fingerprint (and maybe a dep-info file): nothing else known.
    #[default]
    Other,
    /// `cargo check`'s or clippy's `.rmeta`, with no library beside it.
    Metadata,
    /// A build script's compiled program (`build/<crate>-<hash>/build-script-build`).
    BuildScript,
    /// A build script's run: its `output` and `out/` (`build/<crate>-<hash>/output`).
    BuildScriptRun,
    /// A binary or test binary (`deps/<crate>-<hash>`, no extension).
    Binary,
    /// A library to link against: `.rlib`, `.dylib`, `.so` or `.a`.
    Library,
}

/// One unit's files, its newest use, and what it is.
#[derive(Debug)]
struct Unit {
    used: SystemTime,
    paths: Vec<PathBuf>,
    crate_name: String,
    kind: Kind,
}

impl Default for Unit {
    fn default() -> Self {
        Unit {
            used: SystemTime::UNIX_EPOCH,
            paths: Vec::new(),
            crate_name: String::new(),
            kind: Kind::default(),
        }
    }
}

impl Unit {
    /// Learn the unit's crate and kind from one of its files, `name` in
    /// `sub`. The crate comes from the fingerprint directory when there is
    /// one (`orion-tui-<hash>` → `orion_tui`), else from the first file seen
    /// with its `lib` prefix off; the kind is the strongest any file shows.
    fn note(&mut self, sub: &str, name: &str, hash: &str, path: &Path) {
        // `unit_hash` found `-<hash>` followed by the end or a `.`.
        let Some(at) = name.find(&format!("-{hash}")) else {
            return;
        };
        let stem = &name[..at];
        let ext = name[at + 1 + hash.len()..].strip_prefix('.').unwrap_or("");
        let kind = match sub {
            "build" if path.join("output").exists() => Kind::BuildScriptRun,
            "build" => Kind::BuildScript,
            "deps" => match ext {
                "rlib" | "dylib" | "so" | "a" => Kind::Library,
                "rmeta" => Kind::Metadata,
                "" if path.is_file() => Kind::Binary,
                _ => Kind::Other,
            },
            _ => Kind::Other,
        };
        self.kind = self.kind.max(kind);
        let stem = if matches!(kind, Kind::Library | Kind::Metadata) {
            stem.strip_prefix("lib").unwrap_or(stem)
        } else {
            stem
        };
        if sub == ".fingerprint" || self.crate_name.is_empty() {
            self.crate_name = stem.replace('-', "_");
        }
    }
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

    /// A Cargo target directory with one `debug/` build directory in it.
    fn cargo_target(root: &Path) -> PathBuf {
        let target = root.join("target");
        let debug = target.join("debug");
        fs::create_dir_all(debug.join(".fingerprint")).unwrap();
        fs::write(target.join("CACHEDIR.TAG"), TAG).unwrap();
        fs::write(debug.join(".cargo-lock"), b"").unwrap();
        debug
    }

    /// One library build of `orion_tui` under `hash`: fingerprint, rlib,
    /// rmeta and an object file, all last used at `when`.
    fn library(debug: &Path, hash: &str, when: SystemTime) {
        let fingerprint = debug.join(format!(".fingerprint/orion-tui-{hash}"));
        touch(&fingerprint.join("lib-orion_tui"), when);
        set_dir_time(&fingerprint, when);
        touch(&debug.join(format!("deps/liborion_tui-{hash}.rlib")), when);
        touch(&debug.join(format!("deps/liborion_tui-{hash}.rmeta")), when);
        touch(
            &debug.join(format!("deps/orion_tui-{hash}.orion_tui.x-cgu.0.rcgu.o")),
            when,
        );
    }

    fn ago(hours: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(hours * 60 * 60)
    }

    #[test]
    fn every_build_but_the_newest_two_of_a_crate_goes_whole_however_recent() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = cargo_target(tmp.path());
        // Four builds within the last hours: none idle, two superseded.
        library(&debug, "aaaaaaaaaaaaaaaa", ago(4));
        library(&debug, "bbbbbbbbbbbbbbbb", ago(3));
        library(&debug, "cccccccccccccccc", ago(2));
        library(&debug, "dddddddddddddddd", ago(1));
        // The top-level copy, never touched.
        touch(&debug.join("liborion_tui.rlib"), ago(4));

        let report = sweep_checkouts([tmp.path()]);

        assert_eq!(report.removed, 2);
        assert_eq!(report.busy, 0);
        assert!(report.freed > 0);
        for gone in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"] {
            assert!(!debug
                .join(format!(".fingerprint/orion-tui-{gone}"))
                .exists());
            assert!(!debug
                .join(format!("deps/liborion_tui-{gone}.rlib"))
                .exists());
            assert!(!debug
                .join(format!("deps/liborion_tui-{gone}.rmeta"))
                .exists());
            assert!(!debug
                .join(format!("deps/orion_tui-{gone}.orion_tui.x-cgu.0.rcgu.o"))
                .exists());
        }
        for kept in ["cccccccccccccccc", "dddddddddddddddd"] {
            assert!(debug
                .join(format!("deps/liborion_tui-{kept}.rlib"))
                .exists());
        }
        assert!(debug.join("liborion_tui.rlib").exists());
    }

    #[test]
    fn a_unit_ranks_by_its_newest_use_so_a_build_cargo_just_checked_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = cargo_target(tmp.path());
        library(&debug, "aaaaaaaaaaaaaaaa", ago(30 * 24));
        library(&debug, "bbbbbbbbbbbbbbbb", ago(3));
        library(&debug, "cccccccccccccccc", ago(2));
        // The oldest build's fingerprint, read by Cargo a moment ago.
        touch(
            &debug.join(".fingerprint/orion-tui-aaaaaaaaaaaaaaaa/lib-orion_tui"),
            ago(0),
        );

        let report = sweep_checkouts([tmp.path()]);

        assert_eq!(report.removed, 1);
        assert!(debug
            .join("deps/liborion_tui-aaaaaaaaaaaaaaaa.rlib")
            .exists());
        assert!(!debug
            .join("deps/liborion_tui-bbbbbbbbbbbbbbbb.rlib")
            .exists());
    }

    #[test]
    fn build_test_and_check_builds_of_a_crate_never_push_each_other_out() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = cargo_target(tmp.path());
        library(&debug, "aaaaaaaaaaaaaaaa", ago(3));
        library(&debug, "bbbbbbbbbbbbbbbb", ago(2));
        // `cargo check`'s metadata and `cargo test`'s binary, both older
        // than the two library builds.
        touch(
            &debug.join("deps/liborion_tui-1111111111111111.rmeta"),
            ago(10),
        );
        touch(
            &debug.join(".fingerprint/orion-tui-1111111111111111/lib-orion_tui"),
            ago(10),
        );
        touch(&debug.join("deps/orion_tui-2222222222222222"), ago(10));
        touch(&debug.join("deps/orion_tui-2222222222222222.d"), ago(10));
        // A build script and its run, of the same crate.
        touch(
            &debug.join("build/orion-tui-3333333333333333/build-script-build"),
            ago(10),
        );
        touch(
            &debug.join("build/orion-tui-4444444444444444/output"),
            ago(10),
        );

        let report = sweep_checkouts([tmp.path()]);

        assert_eq!(
            report,
            Report {
                removed: 0,
                ..report.clone()
            }
        );
        assert!(debug
            .join("deps/liborion_tui-1111111111111111.rmeta")
            .exists());
        assert!(debug.join("deps/orion_tui-2222222222222222").exists());
        assert!(debug.join("build/orion-tui-3333333333333333").exists());
        assert!(debug.join("build/orion-tui-4444444444444444").exists());
    }

    #[test]
    fn incremental_sessions_beyond_a_crates_newest_four_go() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = cargo_target(tmp.path());
        for (i, name) in [
            "0aaaaaaaaaaaa",
            "1bbbbbbbbbbbb",
            "2cccccccccccc",
            "3dddddddddddd",
            "4eeeeeeeeeeee",
        ]
        .iter()
        .enumerate()
        {
            let session = debug.join(format!("incremental/orion_tui-{name}"));
            touch(&session.join("s-1/dep-graph.bin"), ago(10 - i as u64));
            set_dir_time(&session.join("s-1"), ago(10 - i as u64));
            set_dir_time(&session, ago(10 - i as u64));
        }
        touch(
            &debug.join("incremental/orion_core-0aaaaaaaaaaaa/s-1/dep-graph.bin"),
            ago(100),
        );

        let report = sweep_checkouts([tmp.path()]);

        assert_eq!(report.removed, 1);
        assert!(!debug.join("incremental/orion_tui-0aaaaaaaaaaaa").exists());
        assert!(debug.join("incremental/orion_tui-1bbbbbbbbbbbb").exists());
        assert!(debug.join("incremental/orion_core-0aaaaaaaaaaaa").exists());
    }

    #[test]
    fn a_directory_without_cargos_tag_is_never_touched() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = tmp.path().join("target/debug");
        for (i, hash) in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc"]
            .iter()
            .enumerate()
        {
            library(&debug, hash, ago(3 - i as u64));
        }

        let report = sweep_checkouts([tmp.path()]);

        assert_eq!(report, Report::default());
        assert!(debug
            .join("deps/liborion_tui-aaaaaaaaaaaaaaaa.rlib")
            .exists());
    }

    #[test]
    fn a_build_directory_cargo_is_building_in_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = cargo_target(tmp.path());
        for (i, hash) in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc"]
            .iter()
            .enumerate()
        {
            library(&debug, hash, ago(3 - i as u64));
        }
        let held = fs::File::options()
            .read(true)
            .write(true)
            .open(debug.join(".cargo-lock"))
            .unwrap();
        held.lock().unwrap();

        let report = sweep_checkouts([tmp.path()]);

        assert_eq!(report.busy, 1);
        assert_eq!(report.removed, 0);
        assert!(debug
            .join("deps/liborion_tui-aaaaaaaaaaaaaaaa.rlib")
            .exists());
    }
}
