//! Where this machine keeps things orion reads and writes outside a
//! checkout — the home directory, Claude Code's config dir and its
//! sign-in record, the Trash — and the two things done with them: a path
//! shown with `~` for the home directory, and a folder moved into the
//! Trash.
//!
//! A move into the Trash is a rename into `~/.Trash`, never a removal
//! (Finder has no Put Back for it — drag it out).

use orion_core::claude_account::Record;
use std::path::{Path, PathBuf};

/// This machine's folders, as orion finds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    /// The home dir: where a new Claude account's dir goes
    /// (`~/.claude-2`).
    pub home: Option<PathBuf>,
    /// Claude Code's config dir (`orion_core::paths::claude_config_dir`),
    /// `$CLAUDE_CONFIG_DIR` else `~/.claude`: the default Claude account's,
    /// which a new account shares its setup from.
    pub claude: Option<PathBuf>,
    /// Where Claude Code records who that default account is signed in
    /// as.
    pub claude_record: Option<Record>,
    /// `~/.Trash`, the folder Finder empties.
    pub trash: Option<PathBuf>,
}

impl Places {
    /// This machine's, from the environment. A unit test gets the ones
    /// [`with_places`] pinned, or none at all — never the developer's own
    /// folders or accounts, and never their Trash.
    pub fn of_this_machine() -> Self {
        if cfg!(test) {
            return pinned().unwrap_or_default();
        }
        let home = orion_core::env::home_dir();
        let trash = home.as_ref().map(|h| h.join(".Trash"));
        Self {
            claude: orion_core::paths::claude_config_dir(),
            claude_record: Record::of(None),
            home,
            trash,
        }
    }

    /// A test machine whose home is `home`: Claude Code's dir and record
    /// where they are with `CLAUDE_CONFIG_DIR` unset, and no Trash, so
    /// nothing is ever moved into one.
    #[cfg(test)]
    pub fn in_home(home: &Path) -> Self {
        Self {
            home: Some(home.to_path_buf()),
            claude: Some(home.join(".claude")),
            claude_record: Some(Record {
                file: home.join(".claude.json"),
                legacy: home.join(".claude/.config.json"),
            }),
            trash: None,
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
pub fn move_folder_to_trash(dir: &Path, trash: &Path) -> std::io::Result<PathBuf> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no folder name"))?;
    let candidate = std::iter::once(name.clone())
        .chain((2..).map(|n| format!("{name} {n}")))
        .find(|candidate| std::fs::symlink_metadata(trash.join(candidate)).is_err())
        .expect("an unbounded range always finds a free name");
    std::fs::create_dir_all(trash)?;
    let landed = trash.join(&candidate);
    std::fs::rename(dir, &landed)?;
    Ok(landed)
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
        let trash = home.path().join(".Trash");
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
