//! A CHECKOUT READ: what a checkout's badge and band print — its changed
//! files, the lines behind them, and how far it is from its base — read
//! as cheaply as what moved allows. Blocking: run off the loop
//! (`sync::local`).
//!
//! Every read runs `git status`: it is the question. The two answers that
//! follow it are kept from the read before (a [`Memo`] per checkout) and
//! asked again only when what they depend on moved:
//!
//! * **ahead and behind** (`git rev-list --left-right --count`) only when
//!   HEAD or the base names another commit — read from the ref files
//!   (`git_refs`), no process. An edit moves neither.
//! * **the lines** (`git diff HEAD --numstat` and the untracked files'
//!   own lines) only when the status output, HEAD, or the size or time of
//!   one of the changed files moved. The status alone would miss an edit
//!   to a file that was already modified; its stamp does not.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::git_diff::{DiffFile, LineChanges};

/// What a read found: the changed files, the lines behind them, and the
/// commits ahead of and behind the base. Each None when git couldn't say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Read {
    pub files: Option<Vec<DiffFile>>,
    pub lines: Option<LineChanges>,
    pub ahead: Option<(usize, usize)>,
}

/// What the last read of one checkout settled, and on what.
#[derive(Debug, Clone, Default, PartialEq)]
struct Memo {
    /// HEAD's sha, the base ref and its sha the counts were taken at.
    ahead_at: Option<(String, String, String)>,
    ahead: Option<(usize, usize)>,
    /// The fingerprint of the status, HEAD and the changed files' stamps
    /// the lines were counted at.
    lines_at: Option<u64>,
    lines: Option<LineChanges>,
}

/// Every checkout's [`Memo`], by path.
fn memos() -> &'static Mutex<HashMap<PathBuf, Memo>> {
    static MEMOS: std::sync::OnceLock<Mutex<HashMap<PathBuf, Memo>>> = std::sync::OnceLock::new();
    MEMOS.get_or_init(Default::default)
}

/// Read the checkout at `path`, measured against the **Base branch** of
/// `repo`, its project (THE CONFIG CACHE).
pub fn read(path: &Path, repo: Option<&Path>) -> Read {
    let files = crate::git_diff::changed_files(path).ok();
    let setting = crate::config::base_branch_of(repo);
    let dirs = crate::git_refs::dirs(path);
    let head = dirs.as_ref().and_then(crate::git_refs::head_sha);
    let base = crate::commit_list::resolve_base_cached(path, &setting);
    let base_at = match (&head, &base) {
        (Some(head), Some(base)) => {
            base_sha(path, dirs.as_ref(), base).map(|sha| (head.clone(), base.clone(), sha))
        }
        _ => None,
    };
    let lines_at = files
        .as_deref()
        .map(|files| fingerprint(path, head.as_deref(), files));
    let was = memos()
        .lock()
        .ok()
        .and_then(|m| m.get(path).cloned())
        .unwrap_or_default();
    let ahead = match &base_at {
        Some(at) if was.ahead_at.as_ref() == Some(at) => was.ahead,
        Some((_, base, _)) => crate::commit_list::ahead_behind_of(path, base),
        // HEAD or the base not read from the files: asked of git, as
        // before, and nothing kept.
        None => base
            .as_deref()
            .and_then(|base| crate::commit_list::ahead_behind_of(path, base)),
    };
    let lines = match (lines_at, files.as_deref()) {
        (Some(at), _) if was.lines_at == Some(at) && was.lines.is_some() => was.lines,
        (_, Some(files)) => crate::git_diff::line_changes(path, files),
        (_, None) => None,
    };
    let memo = Memo {
        ahead_at: base_at.filter(|_| ahead.is_some()),
        ahead,
        lines_at: lines_at.filter(|_| lines.is_some()),
        lines,
    };
    if let Ok(mut m) = memos().lock() {
        m.insert(path.to_path_buf(), memo);
    }
    Read {
        files,
        lines,
        ahead,
    }
}

/// The sha `base` names in the checkout at `path`: from the ref files
/// when they say, else one `git rev-parse`.
fn base_sha(path: &Path, dirs: Option<&crate::git_refs::GitDirs>, base: &str) -> Option<String> {
    dirs.and_then(|d| crate::git_refs::branch_sha(d, base))
        .or_else(|| crate::git_diff::rev_parse(path, base))
}

/// What the line counts of `files` depend on, as one number: HEAD, the
/// status itself, and each changed file's size and modification time —
/// so an edit to a file already listed as modified still counts again.
fn fingerprint(root: &Path, head: Option<&str>, files: &[DiffFile]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    head.hash(&mut h);
    for f in files {
        f.path.hash(&mut h);
        f.orig_path.hash(&mut h);
        f.xy.hash(&mut h);
        let stamp = std::fs::symlink_metadata(root.join(&f.path))
            .ok()
            .map(|m| (m.len(), m.modified().ok()));
        stamp.hash(&mut h);
    }
    h.finish()
}

/// Forget what reads of the checkout at `path` kept: the next one asks
/// git for everything.
pub fn forget(path: &Path) {
    if let Ok(mut m) = memos().lock() {
        m.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{:?}", out);
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "t@t"]);
        git(root, &["config", "user.name", "t"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("a"), "one\n").unwrap();
        git(root, &["add", "a"]);
        git(root, &["commit", "-q", "-m", "a"]);
        git(root, &["checkout", "-q", "-b", "feat"]);
        dir
    }

    /// An edit to a file already modified moves its lines, though the
    /// status says the same `M a` both times; a commit moves the counts.
    #[test]
    fn a_read_counts_again_only_what_moved() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("a"), "one\ntwo\n").unwrap();
        let first = read(root, None);
        assert_eq!(first.files.as_ref().map(Vec::len), Some(1));
        assert_eq!(first.lines.map(|l| l.added), Some(1));
        // The memo answers an unchanged read with what it kept.
        let kept = memos().lock().unwrap().get(root).cloned().unwrap();
        assert!(kept.lines_at.is_some());
        assert_eq!(read(root, None), first);
        // Same status, a longer file: counted again.
        std::fs::write(root.join("a"), "one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(read(root, None).lines.map(|l| l.added), Some(3));
        git(root, &["commit", "-qam", "more"]);
        let after = read(root, None);
        assert_eq!(after.files.as_ref().map(Vec::len), Some(0));
        assert_eq!(after.lines, Some(LineChanges::default()));
        forget(root);
        assert!(memos().lock().unwrap().get(root).is_none());
    }

    /// The counts against the base are kept while neither HEAD nor the
    /// base moves, and asked again the moment one does.
    #[test]
    fn ahead_and_behind_follow_head_and_the_base() {
        let dir = repo();
        let root = dir.path();
        git(root, &["checkout", "-q", "main"]);
        let linked = root.join("linked");
        git(
            root,
            &["worktree", "add", "-q", linked.to_str().unwrap(), "feat"],
        );
        // Measured against the ROOT WORKTREE's branch, `main`.
        assert_eq!(read(&linked, None).ahead, Some((0, 0)));
        let kept = |p: &Path| memos().lock().unwrap().get(p).cloned().unwrap();
        let (head, base, _) = kept(&linked).ahead_at.expect("read from the files");
        assert_eq!(base, "main");
        std::fs::write(linked.join("b"), "b\n").unwrap();
        git(&linked, &["add", "b"]);
        git(&linked, &["commit", "-qm", "b"]);
        assert_eq!(read(&linked, None).ahead, Some((1, 0)), "HEAD moved");
        assert_ne!(kept(&linked).ahead_at.unwrap().0, head);
        std::fs::write(root.join("c"), "c\n").unwrap();
        git(root, &["add", "c"]);
        git(root, &["commit", "-qm", "c"]);
        assert_eq!(read(&linked, None).ahead, Some((1, 1)), "the base moved");
    }
}
