//! GHOSTTY KEYBINDS: the lines orion keeps in the user's Ghostty config so
//! the ⌘ chords its keymap ships reach it. Ghostty claims a handful of ⌘
//! chords for itself and never passes them to the program running inside
//! (`ghostty +list-keybinds --default`); [`UNBINDS`] releases exactly the
//! ones orion's defaults use, nothing else.
//!
//! The lines live in one marked block orion owns and rewrites in place.
//! Everything outside it is the user's and is never touched, and a block
//! already saying the right thing is not rewritten — so the file only
//! changes the first time, or when a newer orion needs a different set.
//! Ghostty reads its config at launch and on its own reload (⌘⇧,), so a
//! change asks for one of those.

use std::path::{Path, PathBuf};

/// The Ghostty default bindings orion's keymap needs released, in
/// Ghostty's own spelling: ⌘⇧P (orion's command palette, Ghostty's),
/// ⌘N (a new agent, Ghostty's new window) and ⌘, (orion's settings,
/// Ghostty's open-config).
pub const UNBINDS: &[&str] = &[
    "super+shift+p",
    "super+n",
    "super+,",
    "super+p",
    "super+o",
    "super+r",
    "super+f",
    "super+l",
];

/// First and last line of the block orion owns.
const BEGIN: &str = "# >>> orion keybinds (managed by orion; edits inside this block are replaced) >>>";
const END: &str = "# <<< orion keybinds <<<";

/// The block as written: the markers around one `keybind = <chord>=unbind`
/// per [`UNBINDS`] entry.
pub fn block() -> String {
    let mut out = String::from(BEGIN);
    out.push('\n');
    for chord in UNBINDS {
        out.push_str(&format!("keybind = {chord}=unbind\n"));
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// `existing` with orion's block in it: an old block replaced where it
/// stands, otherwise the block appended after a blank line.
pub fn with_block(existing: &str) -> String {
    let wanted = block();
    if let (Some(start), Some(end_at)) = (existing.find(BEGIN), existing.find(END)) {
        if end_at > start {
            let mut end = end_at + END.len();
            if existing[end..].starts_with('\n') {
                end += 1;
            }
            return format!("{}{wanted}{}", &existing[..start], &existing[end..]);
        }
    }
    let mut out = existing.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&wanted);
    out
}

/// The config file Ghostty reads on macOS, by its own rule: the first of
/// Application Support's `config.ghostty` / `config` and the XDG dir's
/// `config.ghostty` / `config` that exists with something in it, else
/// Application Support's `config.ghostty` (what Ghostty creates first).
pub fn config_path(home: &Path, xdg_config_home: Option<&Path>) -> PathBuf {
    let app_support = home.join("Library/Application Support/com.mitchellh.ghostty");
    let xdg = xdg_config_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".config"))
        .join("ghostty");
    let candidates = [
        app_support.join("config.ghostty"),
        app_support.join("config"),
        xdg.join("config.ghostty"),
        xdg.join("config"),
    ];
    candidates
        .iter()
        .find(|path| std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0))
        .cloned()
        .unwrap_or_else(|| app_support.join("config.ghostty"))
}

/// Put orion's block in the file at `path`, creating it (and its folder)
/// when missing. True when the file changed.
pub fn ensure(path: &Path) -> std::io::Result<bool> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let updated = with_block(&existing);
    if updated == existing {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, updated)?;
    Ok(true)
}

/// Whether orion is running inside Ghostty right now.
pub fn inside_ghostty() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|v| v.eq_ignore_ascii_case("ghostty"))
}

/// The startup (and Settings) pass: with the `ghostty_keybinds` SETTING
/// on, on a local Mac, and Ghostty in the picture — orion running inside
/// it, or it being the **Outside terminal** — make sure the block is in
/// Ghostty's config (`ORION_GHOSTTY_CONFIG` names another file, or `off`
/// none). The flash to show when the file changed or could not be
/// written; None when there was nothing to do. Unit tests never get past
/// the first check: they must not write the machine's real config.
pub fn ensure_for(cfg: &crate::config::Config) -> Option<String> {
    if cfg!(test)
        || !cfg.ghostty_keybinds
        || !cfg!(target_os = "macos")
        || orion_core::host::is_remote_session()
    {
        return None;
    }
    let path = match std::env::var(orion_core::env::GHOSTTY_CONFIG) {
        Ok(v) if v.eq_ignore_ascii_case("off") => return None,
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v),
        _ => {
            let ghostty_in_use = inside_ghostty()
                || (cfg.outside_terminal() == crate::config::OutsideTerminal::Ghostty
                    && crate::event_loop::ghostty_app().is_some());
            if !ghostty_in_use {
                return None;
            }
            let home = PathBuf::from(std::env::var_os("HOME")?);
            let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
            config_path(&home, xdg.as_deref())
        }
    };
    match ensure(&path) {
        Ok(true) => Some(format!(
            "added orion's keybinds to {} — reload Ghostty's config (⌘⇧,) to use ⌘ chords",
            path.display()
        )),
        Ok(false) => None,
        Err(e) => Some(format!("couldn't update Ghostty's config {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_unbinds_every_chord_and_nothing_else() {
        assert_eq!(
            block(),
            format!(
                "{BEGIN}\nkeybind = super+shift+p=unbind\nkeybind = super+n=unbind\nkeybind = super+,=unbind\nkeybind = super+p=unbind\nkeybind = super+o=unbind\nkeybind = super+r=unbind\nkeybind = super+f=unbind\nkeybind = super+l=unbind\n{END}\n"
            )
        );
    }

    #[test]
    fn the_block_is_appended_after_the_users_own_lines() {
        assert_eq!(with_block(""), block());
        assert_eq!(
            with_block("font-size = 14"),
            format!("font-size = 14\n\n{}", block())
        );
    }

    #[test]
    fn an_old_block_is_replaced_where_it_stands() {
        let old = format!("theme = x\n{BEGIN}\nkeybind = super+t=unbind\n{END}\nfont-size = 14\n");
        assert_eq!(
            with_block(&old),
            format!("theme = x\n{}font-size = 14\n", block())
        );
        let current = with_block(&old);
        assert_eq!(with_block(&current), current, "a right block stays put");
    }

    #[test]
    fn ensure_writes_once_and_creates_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.ghostty");
        assert!(ensure(&path).unwrap());
        assert!(!ensure(&path).unwrap(), "already right: untouched");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), block());
    }

    #[test]
    fn the_path_is_the_first_non_empty_config_ghostty_reads() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let app_support = home.join("Library/Application Support/com.mitchellh.ghostty");
        let xdg = home.join(".config/ghostty");
        assert_eq!(
            config_path(home, None),
            app_support.join("config.ghostty"),
            "nothing yet: where Ghostty creates its own"
        );
        std::fs::create_dir_all(&xdg).unwrap();
        std::fs::write(xdg.join("config"), "font-size = 14\n").unwrap();
        assert_eq!(config_path(home, None), xdg.join("config"));
        std::fs::create_dir_all(&app_support).unwrap();
        std::fs::write(app_support.join("config.ghostty"), "").unwrap();
        assert_eq!(
            config_path(home, None),
            xdg.join("config"),
            "an empty file is passed over"
        );
        std::fs::write(app_support.join("config.ghostty"), "theme = x\n").unwrap();
        assert_eq!(config_path(home, None), app_support.join("config.ghostty"));
    }

    #[test]
    fn xdg_config_home_moves_the_xdg_candidates() {
        let home = tempfile::tempdir().unwrap();
        let xdg_home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(xdg_home.path().join("ghostty")).unwrap();
        let file = xdg_home.path().join("ghostty/config.ghostty");
        std::fs::write(&file, "theme = x\n").unwrap();
        assert_eq!(config_path(home.path(), Some(xdg_home.path())), file);
    }
}
