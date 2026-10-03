//! ENV LINKS: the `.env` files a repository keeps out of git, symlinked
//! from the main checkout into every other checkout of it, so a new
//! WORKTREE runs against the same secrets and local settings as the clone
//! it was cut from instead of starting without them.
//!
//! What is linked is what git itself ignores and does not track: every
//! `.env*` file at any depth of the main checkout (`.env`, `.env.local`,
//! `apps/web/.env.development`, …). A tracked one — a committed
//! `.env.example` — is already in the worktree as git checked it out, and
//! is left alone. Nothing under `node_modules` is touched. A path that
//! already exists in the worktree, as a file, a directory or a link, is
//! never replaced, so a worktree with its own `.env` keeps it.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// How long the ignored-file listing may take before the link pass gives
/// up. It walks the whole checkout, and runs under the DAEMON's worktree
/// lock, so a pathological tree must not hold every worktree op with it.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// The pathspec git matches: `.env` and every `.env.*` at any depth.
const ENV_PATHSPEC: &str = ":(glob)**/.env*";

/// Link every ignored `.env*` file of `main` into `worktree` at the same
/// relative path. Returns the paths linked, relative to the checkout.
pub async fn link(main: &Path, worktree: &Path) -> Result<Vec<PathBuf>> {
    if main == worktree {
        return Ok(Vec::new());
    }
    let mut linked = Vec::new();
    for rel in ignored_env_files(main).await? {
        let source = main.join(&rel);
        let target = worktree.join(&rel);
        if target.symlink_metadata().is_ok() || !source.is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::os::unix::fs::symlink(&source, &target)?;
        linked.push(rel);
    }
    Ok(linked)
}

/// `git ls-files --others --ignored` narrowed to `.env*` names: the files
/// git ignores and does not track, relative to `main`.
async fn ignored_env_files(main: &Path) -> Result<Vec<PathBuf>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(main)
        .args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
            ENV_PATHSPEC,
        ])
        .kill_on_drop(true);
    let output = match tokio::time::timeout(LIST_TIMEOUT, command.output()).await {
        Ok(output) => output?,
        Err(_) => bail!("listing .env files in {} took too long", main.display()),
    };
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(parse_listing(&String::from_utf8_lossy(&output.stdout)))
}

/// The NUL-separated listing as paths, keeping only names that start with
/// `.env` and nothing under a `node_modules` directory.
fn parse_listing(out: &str) -> Vec<PathBuf> {
    out.split('\0')
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .filter(|path| {
            let is_env = path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".env"));
            let in_node_modules = path.components().any(|c| c.as_os_str() == "node_modules");
            is_env && !in_node_modules
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A repo whose `.gitignore` ignores `.env*` except the example, with a
    /// root `.env`, a nested app's `.env.local`, a tracked `.env.example`,
    /// and an ignored `.env` inside `node_modules`.
    fn repo_with_env_files() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join(".gitignore"), ".env*\n!.env.example\nnode_modules/\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(root.join(".env.example"), "SECRET=\n").unwrap();
        std::fs::create_dir_all(root.join("apps/web")).unwrap();
        std::fs::write(root.join("apps/web/.env.local"), "PORT=3000\n").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/.env"), "X=1\n").unwrap();
        git(root, &["add", ".gitignore", ".env.example"]);
        git(
            root,
            &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"],
        );
        dir
    }

    #[tokio::test]
    async fn links_every_ignored_env_file_and_nothing_else() {
        let repo = repo_with_env_files();
        let worktree = tempfile::tempdir().unwrap();
        let mut linked = link(repo.path(), worktree.path()).await.unwrap();
        linked.sort();
        assert_eq!(
            linked,
            vec![PathBuf::from(".env"), PathBuf::from("apps/web/.env.local")]
        );
        let root_env = worktree.path().join(".env");
        assert_eq!(std::fs::read_link(&root_env).unwrap(), repo.path().join(".env"));
        assert_eq!(std::fs::read_to_string(root_env).unwrap(), "SECRET=1\n");
        assert!(worktree.path().join("apps/web/.env.local").is_file());
        assert!(!worktree.path().join(".env.example").exists(), "tracked: git's to check out");
        assert!(!worktree.path().join("node_modules").exists());
    }

    #[tokio::test]
    async fn an_existing_file_in_the_worktree_is_kept() {
        let repo = repo_with_env_files();
        let worktree = tempfile::tempdir().unwrap();
        std::fs::write(worktree.path().join(".env"), "MINE=1\n").unwrap();
        let linked = link(repo.path(), worktree.path()).await.unwrap();
        assert_eq!(linked, vec![PathBuf::from("apps/web/.env.local")]);
        assert_eq!(
            std::fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "MINE=1\n"
        );
        assert!(link(repo.path(), worktree.path()).await.unwrap().is_empty(), "idempotent");
    }

    #[tokio::test]
    async fn the_main_checkout_is_never_linked_into_itself() {
        let repo = repo_with_env_files();
        assert!(link(repo.path(), repo.path()).await.unwrap().is_empty());
    }

    #[test]
    fn the_listing_keeps_env_names_outside_node_modules() {
        let parsed = parse_listing(".env\0apps/a/.env.prod\0node_modules/x/.env\0docs/env.md\0\0");
        assert_eq!(
            parsed,
            vec![PathBuf::from(".env"), PathBuf::from("apps/a/.env.prod")]
        );
    }
}
