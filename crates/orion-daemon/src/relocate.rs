//! WORKTREE RELOCATION: orion used to cut a repo's worktrees into a folder
//! beside it, `<parent>/<repo>-worktrees/<branch>`. They now live in one
//! hidden folder per parent, `<parent>/.orion-worktrees/<repo>/<branch>`
//! (`git::worktree_dir`). This module moves the ones still in the old
//! place, once, at DAEMON boot — before any session respawns, so nothing
//! orion runs has its working directory moved out from under it.
//!
//! - **git does the move.** `git worktree move` carries the checkout and
//!   both halves of git's record of it. A locked checkout, or one with
//!   submodules, git refuses: it stays where it is.
//! - **The row follows.** Each worktree row (and a project rooted at the
//!   checkout) has its path rewritten in place, keeping its id, so its
//!   sessions, terminals and links stay on it, and the sync loop never
//!   sees a vanished checkout and a new one.
//! - **Nothing running is moved.** Docker compose projects running only
//!   from the checkout are stopped first (`compose stop`: containers kept)
//!   and named in the report, to be started again from the new folder. A
//!   checkout a running stack shares with another folder, or that some
//!   process outside orion still has as its working directory (a shell, a
//!   dev server, a background Claude session), is left where it is.
//! - **It runs on every boot**, and costs one `is_dir` per project once
//!   the old folders are gone: what was left in place moves on a later
//!   restart, after whatever held it lets go.
//! - **What is safe to move.** The `.env` links point at the main
//!   checkout, which does not move. A Python virtualenv records its own
//!   path and has to be recreated; a Rust `target/` rebuilds once.
//!
//! - **What a moved checkout leaves behind goes too.** A folder in the old
//!   place that git no longer lists and that holds nothing but orion's own
//!   hook files (`.claude/settings.local.json` and the like — a session
//!   that respawned on the old path wrote them after the move) is removed,
//!   unless some process still has it as its working directory. The old
//!   folder itself goes once nothing is left in it but Finder's
//!   `.DS_Store` and links that point nowhere.
//!
//! What it did and what it left goes to the first client to subscribe, as
//! one flash (`Daemon::boot_notes`), and to the log.

use crate::hooks::installer;
use crate::registry::canonical_or_raw;
use crate::store::Store;
use crate::{containers, git};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a move waits for processes still in a checkout — the last
/// daemon's sessions, killed moments ago — before leaving it in place.
const HOLDER_WAIT: Duration = Duration::from_secs(5);
const HOLDER_POLL: Duration = Duration::from_millis(500);

/// Move every worktree still in an old `<repo>-worktrees` folder into the
/// repo's `.orion-worktrees` folder. Returns the note for the clients,
/// None when there was nothing to move.
pub async fn relocate_legacy_worktrees(store: &Store) -> Option<String> {
    let Ok((projects, _, _, _)) = store.load_tree() else {
        return None;
    };
    let mut report = Report::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for project in &projects {
        let repo = &project.repo_path;
        let legacy = git::legacy_worktrees_home(repo);
        if !legacy.is_dir() {
            continue;
        }
        let legacy_canon = canonical_or_raw(&legacy);
        let linked = match git::linked_worktree_paths(repo).await {
            Ok(linked) => linked,
            Err(e) => {
                tracing::warn!(repo = %repo.display(), error = %e, "relocate: git worktree list failed");
                continue;
            }
        };
        let candidates: Vec<PathBuf> = linked
            .iter()
            .filter(|path| {
                path.parent().map(canonical_or_raw).as_deref() == Some(legacy_canon.as_path())
            })
            .filter(|path| seen.insert(canonical_or_raw(path)))
            .cloned()
            .collect();
        if !candidates.is_empty() {
            relocate_project(store, repo, &candidates, &mut report).await;
        }
        // The list from before the moves: a checkout that moved is gone
        // from the old folder, and one left in place is still on it.
        clear_leftovers(&legacy, &linked, &mut report).await;
        remove_if_empty(&legacy);
    }
    report.note()
}

/// What the moves did, for the clients' one flash.
#[derive(Default)]
struct Report {
    /// The folders worktrees moved into, each with how many.
    moved: Vec<(PathBuf, usize)>,
    stopped: Vec<String>,
    left: Vec<(PathBuf, String)>,
    venvs: Vec<PathBuf>,
    /// Folders a moved checkout left behind, removed.
    cleared: usize,
}

impl Report {
    fn note(&self) -> Option<String> {
        let total: usize = self.moved.iter().map(|(_, n)| n).sum();
        if total == 0 && self.left.is_empty() && self.cleared == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if total > 0 {
            let homes: Vec<String> = self
                .moved
                .iter()
                .map(|(home, _)| home.display().to_string())
                .collect();
            parts.push(format!(
                "moved {total} worktree{} into {}",
                if total == 1 { "" } else { "s" },
                homes.join(", ")
            ));
        }
        if self.cleared > 0 {
            parts.push(format!(
                "removed {} folder{} a moved worktree left behind",
                self.cleared,
                if self.cleared == 1 { "" } else { "s" }
            ));
        }
        if !self.stopped.is_empty() {
            parts.push(format!(
                "stopped compose {} (start again from the new folder)",
                self.stopped.join(", ")
            ));
        }
        if !self.venvs.is_empty() {
            let names: Vec<String> = self.venvs.iter().map(|p| display_name(p)).collect();
            parts.push(format!("recreate the Python venv in {}", names.join(", ")));
        }
        for (path, why) in &self.left {
            parts.push(format!(
                "left {} in place ({why}); it moves on a later restart",
                path.display()
            ));
        }
        let mut note = parts.join(" · ");
        if let Some(first) = note.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        Some(note)
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

async fn relocate_project(store: &Store, repo: &Path, candidates: &[PathBuf], report: &mut Report) {
    let docker = containers::find_docker();
    let holders = wait_for_holders(candidates).await;
    let mut moved = 0;
    for from in candidates {
        let Some(name) = from.file_name() else {
            continue;
        };
        let to = git::worktrees_home(repo).join(name);
        if to.symlink_metadata().is_ok() {
            report
                .left
                .push((from.clone(), format!("{} already exists", to.display())));
            continue;
        }
        let from_canon = canonical_or_raw(from);
        if let Some(cmds) = holders.get(&from_canon) {
            report
                .left
                .push((from.clone(), format!("still in use by {}", cmds.join(", "))));
            continue;
        }
        if let Some(docker) = &docker {
            if let Some(running) = containers::running_in(docker, from).await {
                if let Some(project) = running.shared.first() {
                    report.left.push((
                        from.clone(),
                        format!("compose {project} also runs from another folder"),
                    ));
                    continue;
                }
                let mut failed = None;
                for project in &running.owned {
                    match containers::stop_project(docker, project).await {
                        Ok(()) => {
                            tracing::info!(project, worktree = %from.display(), "relocate: stopped compose project");
                            report.stopped.push(project.clone());
                        }
                        Err(e) => {
                            failed = Some(format!("couldn't stop compose {project}: {e}"));
                            break;
                        }
                    }
                }
                if let Some(why) = failed {
                    report.left.push((from.clone(), why));
                    continue;
                }
            }
        }
        if let Err(e) = git::move_worktree(repo, from, &to).await {
            tracing::warn!(from = %from.display(), to = %to.display(), error = %e, "relocate: git worktree move failed");
            report.left.push((from.clone(), format!("git: {e}")));
            continue;
        }
        tracing::info!(from = %from.display(), to = %to.display(), "relocate: moved worktree");
        repoint_rows(store, from, &from_canon, &to);
        if has_venv(&to) {
            report.venvs.push(to.clone());
        }
        moved += 1;
    }
    if moved > 0 {
        report.moved.push((git::worktrees_home(repo), moved));
    }
}

/// Rewrite every row that named the checkout at `from` — spelled as
/// stored, or resolved to the same place — to `to`.
fn repoint_rows(store: &Store, from: &Path, from_canon: &Path, to: &Path) {
    let Ok((projects, worktrees, _, _)) = store.load_tree() else {
        return;
    };
    let named = |path: &Path| path == from || path == from_canon;
    for w in worktrees.iter().filter(|w| named(&w.path)) {
        if let Err(e) = store.update_worktree_path(&w.id, to) {
            tracing::warn!(worktree = %w.id, error = %e, "relocate: worktree row not repointed");
        }
    }
    for p in projects.iter().filter(|p| named(&p.repo_path)) {
        if let Err(e) = store.update_project_repo_path(&p.id, to) {
            tracing::warn!(project = %p.id, error = %e, "relocate: project row not repointed");
        }
    }
}

/// A Python virtualenv at the checkout's top: it records its own path, so
/// a moved one has to be made again.
fn has_venv(checkout: &Path) -> bool {
    [".venv", "venv"]
        .iter()
        .any(|dir| checkout.join(dir).join("pyvenv.cfg").is_file())
}

/// The processes outside orion whose working directory is in one of
/// `candidates`, keyed by the candidate's resolved path. Asked again for
/// up to [`HOLDER_WAIT`] while any are left: the last daemon's sessions
/// die moments before this one boots.
async fn wait_for_holders(
    candidates: &[PathBuf],
) -> std::collections::HashMap<PathBuf, Vec<String>> {
    let canon: Vec<PathBuf> = candidates.iter().map(|p| canonical_or_raw(p)).collect();
    let deadline = tokio::time::Instant::now() + HOLDER_WAIT;
    loop {
        let holders = match process_cwds().await {
            Some(cwds) => holders_of(&cwds, &canon),
            // No lsof to ask: nothing is known to hold anything.
            None => return Default::default(),
        };
        if holders.is_empty() || tokio::time::Instant::now() >= deadline {
            return holders;
        }
        tokio::time::sleep(HOLDER_POLL).await;
    }
}

/// `(command, cwd)` of every process this user can see, from `lsof`.
async fn process_cwds() -> Option<Vec<(String, PathBuf)>> {
    let output = tokio::process::Command::new("lsof")
        .args(["-n", "-P", "-w", "-d", "cwd", "-F", "pcn"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    // lsof exits 1 when some process could not be read; what it printed
    // is still good.
    let output = tokio::time::timeout(Duration::from_secs(10), output)
        .await
        .ok()?
        .ok()?;
    Some(parse_lsof(
        &String::from_utf8_lossy(&output.stdout),
        std::process::id(),
    ))
}

/// lsof's `-F pcn` fields — `p<pid>`, `c<command>`, `f<fd>`, `n<path>`,
/// a line each — as `(command, cwd)`, leaving out `own_pid` and orion's
/// own processes (a TUI started from inside a checkout is fine: its
/// working directory goes with the move).
fn parse_lsof(out: &str, own_pid: u32) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let (mut pid, mut command) = (0u32, String::new());
    for line in out.lines() {
        let (tag, value) = match line.char_indices().nth(1) {
            Some((at, _)) => line.split_at(at),
            None => continue,
        };
        match tag {
            "p" => {
                pid = value.parse().unwrap_or(0);
                command.clear();
            }
            "c" => command = value.to_string(),
            "n" if pid != own_pid && command != "orion" => {
                found.push((command.clone(), PathBuf::from(value)));
            }
            _ => {}
        }
    }
    found
}

fn holders_of(
    cwds: &[(String, PathBuf)],
    candidates: &[PathBuf],
) -> std::collections::HashMap<PathBuf, Vec<String>> {
    let mut holders: std::collections::HashMap<PathBuf, Vec<String>> = Default::default();
    for (command, cwd) in cwds {
        if let Some(dir) = candidates.iter().find(|dir| cwd.starts_with(dir)) {
            let names = holders.entry(dir.clone()).or_default();
            if !names.contains(command) {
                names.push(command.clone());
            }
        }
    }
    holders
}

/// Remove what moved checkouts left in the old folder `legacy`: each
/// folder in it that git no longer lists among `checkouts` and that holds
/// only orion's own files ([`only_orion_leftovers`]). One still some
/// process's working directory — a session of the last daemon not quite
/// dead, a shell left there — stays, for a later restart.
async fn clear_leftovers(legacy: &Path, checkouts: &[PathBuf], report: &mut Report) {
    let Ok(entries) = std::fs::read_dir(legacy) else {
        return;
    };
    let checkouts: HashSet<PathBuf> = checkouts.iter().map(|p| canonical_or_raw(p)).collect();
    let leftovers: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.symlink_metadata().is_ok_and(|m| m.is_dir()))
        .filter(|p| !checkouts.contains(&canonical_or_raw(p)))
        .filter(|p| only_orion_leftovers(p))
        .collect();
    if leftovers.is_empty() {
        return;
    }
    let holders = wait_for_holders(&leftovers).await;
    for dir in leftovers {
        if let Some(cmds) = holders.get(&canonical_or_raw(&dir)) {
            tracing::info!(dir = %dir.display(), held_by = ?cmds, "relocate: leftover folder still in use; it goes on a later restart");
            continue;
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {
                tracing::info!(dir = %dir.display(), "relocate: removed a folder a moved worktree left behind");
                report.cleared += 1;
            }
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "relocate: leftover folder not removed")
            }
        }
    }
}

/// Whether everything under `dir` is orion's own: the hook files its
/// installers write into a checkout ([`installer::EXCLUDED`]), Finder's
/// `.DS_Store`, and the folders holding them. Anything else — a source
/// file, a symlink, a file of the user's — and the folder is not ours to
/// remove.
fn only_orion_leftovers(dir: &Path) -> bool {
    fn walk(root: &Path, dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return false;
            };
            let path = entry.path();
            let Ok(meta) = path.symlink_metadata() else {
                return false;
            };
            if meta.is_dir() {
                if !walk(root, &path) {
                    return false;
                }
                continue;
            }
            if entry.file_name() == ".DS_Store" {
                continue;
            }
            let managed = path.strip_prefix(root).is_ok_and(|rel| {
                installer::EXCLUDED
                    .iter()
                    .any(|file| rel == Path::new(file))
            });
            if !meta.is_file() || !managed {
                return false;
            }
        }
        true
    }
    walk(dir, dir)
}

/// Remove the old folder once nothing is left in it but Finder's
/// `.DS_Store` and symlinks that point nowhere (a shared `node_modules`
/// link to a checkout that is gone).
fn remove_if_empty(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<_> = entries.flatten().collect();
    let dangling = |e: &std::fs::DirEntry| {
        e.path().symlink_metadata().is_ok_and(|m| m.is_symlink()) && !e.path().exists()
    };
    if entries
        .iter()
        .all(|e| e.file_name() == ".DS_Store" || dangling(e))
    {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            tracing::warn!(dir = %dir.display(), error = %e, "relocate: old worktrees folder not removed");
        } else {
            tracing::info!(dir = %dir.display(), "relocate: removed the empty old worktrees folder");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_in(repo: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn lsof_fields_become_command_and_cwd_without_orion_or_us() {
        let out = "p10\nczsh\nfcwd\nn/w/repo-worktrees/feat\n\
                   p11\ncorion\nfcwd\nn/w/repo-worktrees/feat\n\
                   p12\ncnode\nfcwd\nn/w/repo-worktrees/feat/web\n\
                   p99\ncwhatever\nfcwd\nn/w/repo-worktrees/feat\n";
        assert_eq!(
            parse_lsof(out, 99),
            vec![
                ("zsh".to_string(), PathBuf::from("/w/repo-worktrees/feat")),
                (
                    "node".to_string(),
                    PathBuf::from("/w/repo-worktrees/feat/web")
                ),
            ]
        );
    }

    #[test]
    fn a_holder_is_a_process_in_the_checkout_or_under_it_only() {
        let cwds = vec![
            ("zsh".to_string(), PathBuf::from("/w/repo-worktrees/feat")),
            (
                "node".to_string(),
                PathBuf::from("/w/repo-worktrees/feat/web"),
            ),
            (
                "zsh".to_string(),
                PathBuf::from("/w/repo-worktrees/feature"),
            ),
            ("vim".to_string(), PathBuf::from("/w/repo")),
        ];
        let holders = holders_of(&cwds, &[PathBuf::from("/w/repo-worktrees/feat")]);
        assert_eq!(holders.len(), 1);
        assert_eq!(
            holders[Path::new("/w/repo-worktrees/feat")],
            vec!["zsh".to_string(), "node".to_string()]
        );
    }

    #[test]
    fn the_note_says_what_moved_stopped_and_stayed() {
        let report = Report {
            moved: vec![(PathBuf::from("/w/.orion-worktrees/repo"), 2)],
            stopped: vec!["feat".into()],
            left: vec![(
                PathBuf::from("/w/repo-worktrees/busy"),
                "still in use by zsh".into(),
            )],
            venvs: vec![PathBuf::from("/w/.orion-worktrees/repo/py")],
            cleared: 0,
        };
        assert_eq!(
            report.note().unwrap(),
            "Moved 2 worktrees into /w/.orion-worktrees/repo · stopped compose feat (start again \
             from the new folder) · recreate the Python venv in py · left /w/repo-worktrees/busy \
             in place (still in use by zsh); it moves on a later restart"
        );
        assert_eq!(Report::default().note(), None);
    }

    /// The whole move against a real repo: the checkout lands in the new
    /// folder, git still knows it there, its row keeps its id with the new
    /// path, and the empty old folder is gone.
    #[tokio::test]
    async fn an_old_worktree_moves_into_the_hidden_folder_and_its_row_follows() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let old = root.join("repo-worktrees").join("feat");
        git_in(
            &repo,
            &["worktree", "add", &old.to_string_lossy(), "-b", "feat"],
        );

        let store = Store::open(&root.join("orion.db"), &[]).unwrap();
        let project = orion_core::Project {
            id: orion_core::ProjectId::generate(),
            name: "repo".into(),
            repo_path: repo.clone(),
            sort_order: 0,
        };
        store.insert_project(&project).unwrap();
        let row = orion_core::Worktree {
            id: orion_core::WorktreeId::generate(),
            project_id: project.id.clone(),
            path: old.clone(),
            branch: "feat".into(),
            is_main: false,
            sort_order: 0,
        };
        store.insert_worktree(&row).unwrap();

        let note = relocate_legacy_worktrees(&store).await.unwrap();

        let new = root.join(".orion-worktrees").join("repo").join("feat");
        assert!(note.starts_with("Moved 1 worktree into"), "{note}");
        assert!(new.join(".git").is_file(), "the checkout moved");
        assert!(
            !root.join("repo-worktrees").exists(),
            "the empty old folder went"
        );
        let listed = git::linked_worktree_paths(&repo).await.unwrap();
        assert_eq!(listed, vec![new.clone()], "git knows it at the new path");
        let moved = store.get_worktree(&row.id).unwrap().unwrap();
        assert_eq!(moved.path, new, "the row kept its id and took the new path");

        assert_eq!(
            relocate_legacy_worktrees(&store).await,
            None,
            "nothing left to move"
        );
    }

    /// A repo with an old `repo-worktrees` folder, and a store with its
    /// project, for the leftover sweeps.
    fn repo_with_old_folder(root: &Path) -> (PathBuf, PathBuf, Store) {
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-b", "main"]);
        git_in(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let legacy = root.join("repo-worktrees");
        std::fs::create_dir(&legacy).unwrap();
        let store = Store::open(&root.join("orion.db"), &[]).unwrap();
        store
            .insert_project(&orion_core::Project {
                id: orion_core::ProjectId::generate(),
                name: "repo".into(),
                repo_path: repo.clone(),
                sort_order: 0,
            })
            .unwrap();
        (repo, legacy, store)
    }

    /// A folder holding only the hook file a session wrote after its
    /// checkout moved away.
    fn leftover(legacy: &Path, name: &str) -> PathBuf {
        let dir = legacy.join(name);
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join(".claude/settings.local.json"), "{}").unwrap();
        std::fs::write(dir.join(".DS_Store"), "").unwrap();
        dir
    }

    /// What a moved checkout left — a folder of orion's hook files and a
    /// `node_modules` link to a checkout that is gone — goes, and the old
    /// folder with it.
    #[tokio::test]
    async fn a_moved_checkouts_leftovers_go_and_the_old_folder_with_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (_repo, legacy, store) = repo_with_old_folder(&root);
        leftover(&legacy, "feat");
        std::os::unix::fs::symlink(root.join("gone/node_modules"), legacy.join("node_modules"))
            .unwrap();

        let note = relocate_legacy_worktrees(&store).await.unwrap();

        assert_eq!(note, "Removed 1 folder a moved worktree left behind");
        assert!(!legacy.exists(), "the old folder went");
    }

    /// Anything that isn't orion's keeps its folder: a file of the user's
    /// in a leftover, a checkout git still lists, a loose file.
    #[tokio::test]
    async fn what_isnt_orions_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (repo, legacy, store) = repo_with_old_folder(&root);
        let ours = leftover(&legacy, "ours");
        let theirs = leftover(&legacy, "theirs");
        std::fs::write(theirs.join("notes.txt"), "mine").unwrap();
        let log = legacy.join("hook.log");
        std::fs::write(&log, "a log").unwrap();
        // A checkout still in the old place (it would move, but the new
        // place is taken): git lists it, so it is never a leftover.
        let held = legacy.join("held");
        git_in(
            &repo,
            &["worktree", "add", &held.to_string_lossy(), "-b", "held"],
        );
        std::fs::create_dir_all(root.join(".orion-worktrees/repo/held")).unwrap();

        relocate_legacy_worktrees(&store).await;

        assert!(!ours.exists(), "orion's leftover went");
        assert!(theirs.join("notes.txt").is_file(), "the user's file stayed");
        assert!(log.is_file(), "the loose file stayed");
        assert!(held.join(".git").is_file(), "the checkout stayed");
    }

    #[test]
    fn a_folder_is_orions_only_when_every_file_in_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(only_orion_leftovers(&empty));
        let hooks = leftover(tmp.path(), "hooks");
        assert!(only_orion_leftovers(&hooks));
        std::fs::create_dir_all(hooks.join(".cursor/rules")).unwrap();
        std::fs::write(hooks.join(".cursor/rules/orion-title.mdc"), "").unwrap();
        assert!(only_orion_leftovers(&hooks));
        std::fs::write(hooks.join(".cursor/rules/mine.mdc"), "").unwrap();
        assert!(!only_orion_leftovers(&hooks));
        let linked = leftover(tmp.path(), "linked");
        let file = linked.join(".claude/settings.local.json");
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink("/nowhere", &file).unwrap();
        assert!(!only_orion_leftovers(&linked), "a symlink is never ours");
    }
}
