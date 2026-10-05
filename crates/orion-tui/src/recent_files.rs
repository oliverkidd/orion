//! RECENT FILES: the files last opened in orion's own viewer — from Go to
//! file (`⌘P`), find in files, the TREE BROWSER or a ⌘click on a path in
//! the pane — per checkout, newest first, for the FILE FINDER's `Recent`
//! section.
//!
//! `<data dir>/recent_files.json`, read and written whole on each open like
//! `reviewed.json` (`crate::review`), off the loop — pruning a checkout gone
//! from disk means a `stat` per checkout, which a sleeping network volume
//! can stall. Two TUIs writing at once can drop one's latest open, and a
//! failed write is logged, not surfaced: the list is a convenience, never
//! in the way.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// How many files each checkout remembers — more than the finder shows, so
/// one that left the listing (deleted, renamed) has others behind it.
const KEEP: usize = 20;

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    /// Checkout path → its files, relative to it, newest first.
    #[serde(default)]
    worktrees: HashMap<String, Vec<String>>,
}

/// Note `file` — as the viewer opened it, relative to `root` or absolute
/// inside it — as the one most recently opened there.
pub fn record(root: &Path, file: &str) {
    let Some(path) = store_path() else {
        return;
    };
    let Some(file) = listed_as(root, file) else {
        return;
    };
    let root = root.to_path_buf();
    #[cfg(not(test))]
    std::thread::spawn(move || save(&path, &root, file));
    #[cfg(test)]
    save(&path, &root, file);
}

/// `file` as the FILE FINDER lists it: relative to `root`, with no `./`.
/// None for a path outside the checkout, which has no row to lead.
fn listed_as(root: &Path, file: &str) -> Option<String> {
    let file = Path::new(file);
    let inside = if file.is_absolute() {
        file.strip_prefix(root).ok()?
    } else {
        file
    };
    let clean: PathBuf = inside
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    (!clean.as_os_str().is_empty()).then(|| clean.to_string_lossy().into_owned())
}

/// One write at a time from this process: they share a temporary file.
static SAVING: Mutex<()> = Mutex::new(());

fn save(path: &Path, root: &Path, file: String) {
    let _one = SAVING.lock();
    let mut store = read_store(path);
    store
        .worktrees
        .retain(|checkout, _| Path::new(checkout).is_dir());
    let list = store
        .worktrees
        .entry(root.to_string_lossy().into_owned())
        .or_default();
    list.retain(|f| *f != file);
    list.insert(0, file);
    list.truncate(KEEP);
    if let Err(err) = crate::pr_cache::write_json_atomic(path, &store) {
        tracing::warn!("failed to save {}: {err}", path.display());
    }
}

/// `root`'s recent files, newest first.
pub fn load(root: &Path) -> Vec<String> {
    let Some(path) = store_path() else {
        return Vec::new();
    };
    read_store(&path)
        .worktrees
        .remove(root.to_string_lossy().as_ref())
        .unwrap_or_default()
}

fn read_store(path: &Path) -> StoreFile {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return StoreFile::default();
    };
    serde_json::from_str(&raw).unwrap_or_else(|err| {
        tracing::warn!("ignoring malformed {}: {err}", path.display());
        StoreFile::default()
    })
}

/// Where the list lives. None in the unit tests unless one names a file
/// ([`with_store_path`]): the many that open a file must never write to
/// the real data dir.
fn store_path() -> Option<PathBuf> {
    #[cfg(test)]
    let path = STORE_PATH_OVERRIDE.with(|p| p.borrow().clone());
    #[cfg(not(test))]
    let path = Some(orion_core::paths::data_dir().join("recent_files.json"));
    path
}

#[cfg(test)]
thread_local! {
    static STORE_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn with_store_path<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    STORE_PATH_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(path));
        let out = f();
        slot.replace(prev);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_once_each_per_checkout_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        with_store_path(dir.path().join("recent_files.json"), || {
            record(&a, "src/one.rs");
            record(&a, "src/two.rs");
            record(&a, "src/one.rs");
            record(&b, "README.md");
            assert_eq!(load(&a), vec!["src/one.rs", "src/two.rs"], "once each");
            assert_eq!(load(&b), vec!["README.md"], "per checkout");

            for n in 0..KEEP + 5 {
                record(&a, &format!("f{n}.rs"));
            }
            let kept = load(&a);
            assert_eq!(kept.len(), KEEP);
            assert_eq!(kept[0], format!("f{}.rs", KEEP + 4));
        });
    }

    #[test]
    fn paths_are_kept_as_the_finder_lists_them_and_one_outside_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        with_store_path(dir.path().join("recent_files.json"), || {
            record(&root, root.join("src/in.rs").to_str().unwrap());
            record(&root, "/elsewhere/out.rs");
            record(&root, "./src/dot.rs");
            assert_eq!(load(&root), vec!["src/dot.rs", "src/in.rs"]);
        });
    }

    #[test]
    fn a_checkout_gone_from_disk_is_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let (kept, gone) = (dir.path().join("kept"), dir.path().join("gone"));
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::create_dir_all(&gone).unwrap();
        with_store_path(dir.path().join("recent_files.json"), || {
            record(&gone, "a.rs");
            std::fs::remove_dir_all(&gone).unwrap();
            record(&kept, "b.rs");
            assert!(load(&gone).is_empty());
            assert_eq!(load(&kept), vec!["b.rs"]);
        });
    }
}
