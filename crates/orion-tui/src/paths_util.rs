//! Where this machine keeps things orion reads and writes outside a
//! checkout — the home directory, Claude Code's config dir, the Trash —
//! and the two things done with them: a path shown with `~` for the home
//! directory, and a folder moved into the Trash.
//!
//! A move into the Trash is a rename, never a removal: `~/.Trash` on
//! macOS (Finder has no Put Back for it — drag it out), the freedesktop.org
//! Trash (`~/.local/share/Trash`) elsewhere, recorded so a file manager
//! can restore it.

use std::path::{Path, PathBuf};

/// The Trash a folder is moved into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trash {
    /// macOS: `~/.Trash`, the folder Finder empties.
    Mac(PathBuf),
    /// Elsewhere: the freedesktop.org Trash (`$XDG_DATA_HOME/Trash`), each
    /// entry in its `files/` with an `info/<name>.trashinfo` saying where it
    /// came from, so a file manager can put it back.
    Freedesktop(PathBuf),
}

/// This machine's folders, as orion finds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    pub home: Option<PathBuf>,
    /// Claude Code's config dir (`orion_core::paths::claude_config_dir`).
    pub claude: Option<PathBuf>,
    pub trash: Option<Trash>,
}

impl Places {
    /// This machine's, from the environment. A unit test gets the ones
    /// [`with_places`] pinned, or none at all — never the developer's own
    /// folders, and never their Trash.
    pub fn of_this_machine() -> Self {
        if cfg!(test) {
            return pinned().unwrap_or_default();
        }
        let home = orion_core::env::home_dir();
        let trash = if cfg!(target_os = "macos") {
            home.as_ref().map(|h| Trash::Mac(h.join(".Trash")))
        } else {
            orion_core::env::non_empty("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join(".local/share")))
                .map(|data| Trash::Freedesktop(data.join("Trash")))
        };
        Self {
            claude: orion_core::paths::claude_config_dir(),
            home,
            trash,
        }
    }

    /// Claude Code's user skills folder: `~/.claude/skills`, or under
    /// `$CLAUDE_CONFIG_DIR` when that is set.
    pub fn user_skills(&self) -> Option<PathBuf> {
        self.claude.as_ref().map(|c| c.join("skills"))
    }
}

#[cfg(test)]
thread_local! {
    static PLACES_OVERRIDE: std::cell::RefCell<Option<Places>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn pinned() -> Option<Places> {
    PLACES_OVERRIDE.with(|p| p.borrow().clone())
}

#[cfg(not(test))]
fn pinned() -> Option<Places> {
    None
}

/// Pin [`Places::of_this_machine`] to `places` for the duration of `f`.
#[cfg(test)]
pub fn with_places<T>(places: Places, f: impl FnOnce() -> T) -> T {
    PLACES_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(places));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// Move the folder `dir` into `trash` — a removed Claude account's config
/// dir — under its own name or the first of `name 2`, `name 3`, … the
/// Trash does not hold yet, and return where it landed. A rename, never a
/// copy and a delete: across volumes it fails, and the folder stays where
/// it was.
pub fn move_folder_to_trash(dir: &Path, trash: &Trash) -> std::io::Result<PathBuf> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no folder name"))?;
    // macOS keeps the folder itself; freedesktop.org keeps it in `files/`
    // with its record in `info/`, and a name is free only in both.
    let (files, info) = match trash {
        Trash::Mac(files) => (files.clone(), None),
        Trash::Freedesktop(root) => (root.join("files"), Some(root.join("info"))),
    };
    let record = |candidate: &str| {
        info.as_ref()
            .map(|info| info.join(format!("{candidate}.trashinfo")))
    };
    let taken = |path: &Path| std::fs::symlink_metadata(path).is_ok();
    let candidate = std::iter::once(name.clone())
        .chain((2..).map(|n| format!("{name} {n}")))
        .find(|candidate| {
            !taken(&files.join(candidate)) && !record(candidate).is_some_and(|r| taken(&r))
        })
        .expect("an unbounded range always finds a free name");
    std::fs::create_dir_all(&files)?;
    let landed = files.join(&candidate);
    let Some(record) = record(&candidate) else {
        std::fs::rename(dir, &landed)?;
        return Ok(landed);
    };
    // The spec's order: claim the name with the record, then move — and
    // take the record back out when the move fails.
    std::fs::create_dir_all(record.parent().unwrap_or(&files))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&record)?;
    let moved = std::io::Write::write_all(&mut file, trash_info(dir).as_bytes())
        .and_then(|()| std::fs::rename(dir, &landed));
    if let Err(e) = moved {
        let _ = std::fs::remove_file(&record);
        return Err(e);
    }
    Ok(landed)
}

/// A freedesktop.org `.trashinfo` for `path`: where it came from, percent-
/// encoded, and when it went — in UTC, where the spec asks for local
/// time, so a file manager's "deleted" column is off by the zone at most.
fn trash_info(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut encoded = String::new();
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    let when = orion_core::crashlog::format_timestamp(orion_core::clock::now_secs());
    format!(
        "[Trash Info]\nPath={encoded}\nDeletionDate={}\n",
        when.trim_end_matches('Z')
    )
}

/// `path` with the home directory spelled `~`.
pub(crate) fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(root: &Path, name: &str, file: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), file).unwrap();
        dir
    }

    #[test]
    fn the_trash_takes_the_folder_under_a_free_name() {
        let home = tempfile::tempdir().unwrap();
        let trash = Trash::Mac(home.path().join(".Trash"));
        let first = folder(home.path(), "notes", "kept");
        let landed = move_folder_to_trash(&first, &trash).unwrap();
        assert_eq!(landed, home.path().join(".Trash/notes"));
        assert!(!first.exists(), "moved, not copied");
        assert_eq!(
            std::fs::read_to_string(landed.join("notes.md")).unwrap(),
            "kept"
        );

        let second = folder(home.path(), "notes", "again");
        assert_eq!(
            move_folder_to_trash(&second, &trash).unwrap(),
            home.path().join(".Trash/notes 2"),
            "a name the Trash holds already gets a suffix"
        );
        assert!(
            home.path().join(".Trash/notes/notes.md").exists(),
            "the first one is untouched"
        );
    }

    #[test]
    fn the_freedesktop_trash_records_where_each_folder_came_from() {
        let home = tempfile::tempdir().unwrap();
        let trash = Trash::Freedesktop(home.path().join("Trash"));
        let dir = folder(&home.path().join("my things"), "notes", "x");
        let landed = move_folder_to_trash(&dir, &trash).unwrap();
        assert_eq!(landed, home.path().join("Trash/files/notes"));
        let info = std::fs::read_to_string(home.path().join("Trash/info/notes.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="), "{info}");
        assert!(info.contains("/my%20things/notes\n"), "{info}");
        assert!(info.contains("DeletionDate="), "{info}");

        let again = folder(&home.path().join("my things"), "notes", "y");
        assert_eq!(
            move_folder_to_trash(&again, &trash).unwrap(),
            home.path().join("Trash/files/notes 2")
        );
        assert!(home.path().join("Trash/info/notes 2.trashinfo").exists());
    }

    #[test]
    fn a_path_under_home_reads_with_a_tilde() {
        let home = Path::new("/Users/me");
        assert_eq!(tilde(Path::new("/Users/me"), Some(home)), "~");
        assert_eq!(
            tilde(Path::new("/Users/me/.claude-2"), Some(home)),
            "~/.claude-2"
        );
        assert_eq!(tilde(Path::new("/opt/x"), Some(home)), "/opt/x");
        assert_eq!(tilde(Path::new("/Users/me/x"), None), "/Users/me/x");
    }
}
