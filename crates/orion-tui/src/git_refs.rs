//! REFS FROM DISK: where a checkout's git keeps its HEAD, index and refs,
//! and the commit a ref names, read from the files themselves rather than
//! asked of a `git` process.
//!
//! A checkout read (`checkout_read`) runs on every change the GIT WATCHER
//! sees, and most of them move no ref at all: an edit, a save. Knowing
//! that HEAD and the base still name the commits they named last time is
//! what lets the read skip its `git rev-list` — and finding that out from
//! two small files costs no process. The same paths are what the watcher
//! watches: a linked worktree's HEAD and index live under the main repo's
//! `.git/worktrees/<name>`, and its refs in the repo's common `.git`.
//!
//! Anything these readers don't understand — a reftable repo, a ref
//! packed in a way they don't expect — reads as None, and the caller asks
//! `git` instead.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Where a checkout's git lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    /// The checkout's own git dir: `HEAD` and `index`. `<checkout>/.git`
    /// for the ROOT WORKTREE, `<repo>/.git/worktrees/<name>` for a linked
    /// one.
    pub gitdir: PathBuf,
    /// The dir every checkout of the repo shares: `refs`, `packed-refs`,
    /// `objects`. The same as `gitdir` for the ROOT WORKTREE.
    pub common: PathBuf,
}

/// The git dirs of the checkout at `checkout`: `.git` as a directory, or
/// as the `gitdir: …` file a linked worktree has, its `commondir` beside
/// the HEAD it points at. None for no checkout.
pub fn dirs(checkout: &Path) -> Option<GitDirs> {
    let dot_git = checkout.join(".git");
    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(GitDirs {
            gitdir: dot_git.clone(),
            common: dot_git,
        });
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let pointed = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = absolute(checkout, Path::new(pointed));
    let common = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(rel) => absolute(&gitdir, Path::new(rel.trim())),
        Err(_) => gitdir.clone(),
    };
    Some(GitDirs { gitdir, common })
}

/// `path` as it stands, or under `base` when it is relative — with `..`
/// folded away, so the watcher's prefix tests see the path the events
/// will carry.
fn absolute(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// When the repo last fetched: the newest `FETCH_HEAD` of the checkout's
/// own git dir and the repo's (a fetch writes it where it ran, and a
/// linked worktree's is its own). None when neither has fetched.
pub fn fetched_at(dirs: &GitDirs) -> Option<SystemTime> {
    [&dirs.gitdir, &dirs.common]
        .into_iter()
        .filter_map(|dir| std::fs::metadata(dir.join("FETCH_HEAD")).ok())
        .filter_map(|meta| meta.modified().ok())
        .max()
}

/// The commit HEAD names, read from its file: a detached HEAD's own sha,
/// or the sha of the branch it points at. None when HEAD is unborn or
/// the files don't say.
pub fn head_sha(dirs: &GitDirs) -> Option<String> {
    ref_sha(dirs, "HEAD")
}

/// The branch HEAD is on, by its short name (`main`), read from its file.
/// None for a detached HEAD.
pub fn head_branch(dirs: &GitDirs) -> Option<String> {
    let head = std::fs::read_to_string(dirs.gitdir.join("HEAD")).ok()?;
    let name = head.trim().strip_prefix("ref:")?.trim();
    name.strip_prefix("refs/heads/").map(str::to_string)
}

/// The sha a short branch name reads as, the way `git rev-parse` reads
/// one: a local branch first, then a remote-tracking one — `main`,
/// `origin/main`. None when neither file says.
pub fn branch_sha(dirs: &GitDirs, name: &str) -> Option<String> {
    ["refs/heads/", "refs/remotes/"]
        .iter()
        .find_map(|prefix| ref_sha(dirs, &format!("{prefix}{name}")))
}

/// How many `ref:` links a ref may go through before it reads as None:
/// git's own symrefs are one deep, and a loop must end.
const MAX_SYMREF_DEPTH: u8 = 5;

/// The sha the full ref `name` (`refs/heads/main`, or `HEAD`) names: its
/// loose file, else its line in `packed-refs`. `HEAD` and a per-checkout
/// ref (`refs/bisect/…`, `refs/worktree/…`) are the checkout's own; every
/// other ref is the repo's.
pub fn ref_sha(dirs: &GitDirs, name: &str) -> Option<String> {
    ref_sha_at(dirs, name, 0)
}

/// [`ref_sha`], `depth` links into a chain of `ref:`s.
fn ref_sha_at(dirs: &GitDirs, name: &str, depth: u8) -> Option<String> {
    let own = name == "HEAD"
        || ["refs/bisect/", "refs/worktree/", "refs/rewritten/"]
            .iter()
            .any(|p| name.starts_with(p));
    let dir = if own { &dirs.gitdir } else { &dirs.common };
    match std::fs::read_to_string(dir.join(name)) {
        Ok(text) => follow(dirs, text.trim(), depth),
        Err(_) => packed(&dirs.common, name),
    }
}

/// What a ref file's `text` names: a sha as it stands, or the ref a
/// `ref: <name>` points at, read where that ref lives ([`ref_sha`]) — but
/// never more than [`MAX_SYMREF_DEPTH`] links deep.
fn follow(dirs: &GitDirs, text: &str, depth: u8) -> Option<String> {
    if let Some(target) = text.strip_prefix("ref:") {
        if depth >= MAX_SYMREF_DEPTH {
            return None;
        }
        return ref_sha_at(dirs, target.trim(), depth + 1);
    }
    is_sha(text).then(|| text.to_string())
}

/// One repo's `packed-refs`, parsed, as of the file's mtime and length.
struct Packed {
    stamp: (SystemTime, u64),
    refs: Arc<HashMap<String, String>>,
}

/// Every repo's `packed-refs`, parsed once per change: a checkout read
/// asks it for HEAD's branch and the base on every change the GIT
/// WATCHER sees, and a busy repo's file runs to thousands of lines.
static PACKED: Mutex<Option<HashMap<PathBuf, Packed>>> = Mutex::new(None);

/// `name`'s sha in `<common>/packed-refs`: a `<sha> <name>` line.
fn packed(common: &Path, name: &str) -> Option<String> {
    packed_refs(common)?.get(name).cloned()
}

/// `<common>/packed-refs` as name → sha, parsed again only when its mtime
/// or length moved. None when there is no such file.
fn packed_refs(common: &Path) -> Option<Arc<HashMap<String, String>>> {
    let path = common.join("packed-refs");
    let meta = std::fs::metadata(&path).ok()?;
    let stamp = (meta.modified().ok()?, meta.len());
    let mut memo = PACKED.lock().unwrap_or_else(|e| e.into_inner());
    let memo = memo.get_or_insert_with(HashMap::new);
    if let Some(known) = memo.get(&path).filter(|p| p.stamp == stamp) {
        return Some(known.refs.clone());
    }
    let refs = Arc::new(parse_packed(&std::fs::read_to_string(&path).ok()?));
    memo.insert(
        path,
        Packed {
            stamp,
            refs: refs.clone(),
        },
    );
    Some(refs)
}

/// A `packed-refs` file's `<sha> <name>` lines, by name; its header and
/// peeled (`^…`) lines skipped.
fn parse_packed(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
        .filter_map(|l| {
            let (sha, name) = l.split_once(' ')?;
            is_sha(sha).then(|| (name.trim().to_string(), sha.to_string()))
        })
        .collect()
}

/// A full object name: 40 hex digits (SHA-1) or 64 (SHA-256).
fn is_sha(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo with one commit on `main` and a linked worktree on `feat`.
    fn repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.email", "t@t"]);
        git(&root, &["config", "user.name", "t"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("a"), "a").unwrap();
        git(&root, &["add", "a"]);
        git(&root, &["commit", "-q", "-m", "a"]);
        let linked = dir.path().join("linked");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat",
                linked.to_str().unwrap(),
            ],
        );
        (dir, root, linked)
    }

    #[test]
    fn a_linked_worktree_keeps_its_head_apart_and_shares_the_refs() {
        let (_dir, root, linked) = repo();
        let root = root.canonicalize().unwrap();
        let d = dirs(&linked).expect("linked worktree");
        let gitdir = d.gitdir.canonicalize().unwrap();
        assert_eq!(gitdir, root.join(".git/worktrees/linked"));
        assert_eq!(d.common.canonicalize().unwrap(), root.join(".git"));
        let main = dirs(&root).unwrap();
        assert_eq!(main.gitdir, main.common);
    }

    /// HEAD, a branch and a remote-tracking branch read from the files
    /// say what `git rev-parse` says — loose or packed.
    #[test]
    fn refs_read_from_disk_match_git() {
        let (_dir, root, linked) = repo();
        let d = dirs(&linked).unwrap();
        assert_eq!(head_sha(&d), Some(git(&linked, &["rev-parse", "HEAD"])));
        std::fs::write(linked.join("b"), "b").unwrap();
        git(&linked, &["add", "b"]);
        git(&linked, &["commit", "-q", "-m", "b"]);
        assert_eq!(head_sha(&d), Some(git(&linked, &["rev-parse", "HEAD"])));
        let main = git(&root, &["rev-parse", "main"]);
        assert_eq!(branch_sha(&d, "main"), Some(main.clone()));
        git(&root, &["update-ref", "refs/remotes/origin/main", &main]);
        git(&root, &["pack-refs", "--all"]);
        assert_eq!(branch_sha(&d, "origin/main"), Some(main.clone()));
        assert_eq!(branch_sha(&d, "main"), Some(main), "packed too");
        assert_eq!(branch_sha(&d, "nope"), None);
    }

    /// A symref anywhere but HEAD is read where its target lives: a
    /// linked worktree's per-checkout ref pointing at a branch, a remote's
    /// HEAD pointing at its default. A loop reads as None.
    #[test]
    fn symrefs_resolve_where_their_target_lives() {
        let (_dir, root, linked) = repo();
        let d = dirs(&linked).unwrap();
        let main = git(&root, &["rev-parse", "main"]);
        git(&root, &["update-ref", "refs/remotes/origin/main", &main]);
        git(&root, &["pack-refs", "--all"]);
        git(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        assert_eq!(branch_sha(&d, "origin/HEAD"), Some(main.clone()));
        let own = d.gitdir.join("refs/worktree");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("pin"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(ref_sha(&d, "refs/worktree/pin"), Some(main));
        std::fs::write(own.join("a"), "ref: refs/worktree/b\n").unwrap();
        std::fs::write(own.join("b"), "ref: refs/worktree/a\n").unwrap();
        assert_eq!(ref_sha(&d, "refs/worktree/a"), None, "a loop ends");
    }

    /// A packed ref rewritten is read anew: the memo follows the file.
    #[test]
    fn packed_refs_are_read_again_when_the_file_changes() {
        let (_dir, root, linked) = repo();
        let d = dirs(&linked).unwrap();
        let first = git(&root, &["rev-parse", "main"]);
        git(&root, &["update-ref", "refs/remotes/origin/main", &first]);
        git(&root, &["pack-refs", "--all"]);
        assert_eq!(branch_sha(&d, "origin/main"), Some(first));
        std::fs::write(root.join("c"), "c").unwrap();
        git(&root, &["add", "c"]);
        git(&root, &["commit", "-q", "-m", "c"]);
        let second = git(&root, &["rev-parse", "main"]);
        git(&root, &["update-ref", "refs/remotes/origin/main", &second]);
        git(&root, &["pack-refs", "--all"]);
        assert_eq!(branch_sha(&d, "origin/main"), Some(second));
    }

    /// A fetch's stamp is the newest `FETCH_HEAD` of the checkout and its
    /// repo: none before any fetch.
    #[test]
    fn the_last_fetch_is_read_off_fetch_head() {
        let (_dir, root, linked) = repo();
        let d = dirs(&linked).unwrap();
        assert_eq!(fetched_at(&d), None);
        std::fs::write(root.join(".git/FETCH_HEAD"), "").unwrap();
        let at = fetched_at(&d).expect("the repo's");
        assert!(at <= SystemTime::now());
        assert!(fetched_at(&dirs(&root).unwrap()).is_some());
    }

    #[test]
    fn an_unborn_head_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        let d = dirs(dir.path()).unwrap();
        assert_eq!(head_sha(&d), None);
        assert_eq!(dirs(&dir.path().join("nothing")), None);
    }
}
