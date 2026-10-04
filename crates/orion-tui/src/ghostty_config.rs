//! GHOSTTY KEYBINDS: the lines orion keeps in the user's Ghostty config so
//! every ⌘ chord its keymap answers to reaches it. Ghostty claims dozens
//! of ⌘ chords for itself and never passes them to the program running
//! inside (`ghostty +list-keybinds --default`) — ⌘K clears the screen,
//! ⌘N opens a window, ⌘⇧P is its own palette — so the block releases
//! every ⌘ chord bound to an orion action, the user's rebinds included
//! ([`unbinds`]), plus the editing chords the built-in editor takes
//! ([`EDITOR_CHORDS`]), and nothing orion does not use. A few Ghostty chords are
//! never taken whatever the keymap says ([`NEVER_RELEASED`]): copy, paste,
//! quit, and the window and tab keys.
//!
//! The lines live in one marked block orion owns and rewrites in place.
//! Everything outside it is the user's and is never touched, and a block
//! already saying the right thing is not rewritten — so the file only
//! changes the first time, when a rebind adds or drops a ⌘ chord, or when
//! a newer orion ships a different keymap. Ghostty reads its config at
//! launch and on its own reload (⌘⇧,), so a change asks for one of those.

use crate::keymap::{KeyChord, Keymap};
use crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};

/// The Ghostty chords orion never releases, in Ghostty's spelling, even
/// with an action rebound onto one: copy, paste, quit, close, new tab, the
/// tab digits and the window keys stay Ghostty's.
pub const NEVER_RELEASED: &[&str] = &[
    "super+c",
    "super+v",
    "super+q",
    "super+w",
    "super+shift+w",
    "super+alt+w",
    "super+alt+shift+w",
    "super+t",
    "super+shift+t",
    "super+enter",
    "super+ctrl+f",
    "super+1",
    "super+2",
    "super+3",
    "super+4",
    "super+5",
    "super+6",
    "super+7",
    "super+8",
    "super+9",
];

/// First and last line of the block orion owns.
const BEGIN: &str =
    "# >>> orion keybinds (managed by orion; edits inside this block are replaced) >>>";
const END: &str = "# <<< orion keybinds <<<";

/// `chord` in Ghostty's trigger spelling — `super+shift+p`, `super+/`,
/// `super+arrow_up` — or None for a chord without ⌘, which Ghostty passes
/// on as it is. A shifted symbol is spelled by the key it is typed with
/// (`⌘?` is `super+shift+/`, the trigger Ghostty matches the press on).
pub fn trigger(chord: &KeyChord) -> Option<String> {
    if !chord.mods.contains(KeyModifiers::SUPER) {
        return None;
    }
    let mut shift = chord.mods.contains(KeyModifiers::SHIFT);
    let key = match chord.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => match crate::keymap::unshifted(c) {
            Some(base) => {
                shift = true;
                base.to_string()
            }
            None => c.to_lowercase().to_string(),
        },
        KeyCode::Up => "arrow_up".into(),
        KeyCode::Down => "arrow_down".into(),
        KeyCode::Left => "arrow_left".into(),
        KeyCode::Right => "arrow_right".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab | KeyCode::BackTab => "tab".into(),
        KeyCode::Esc => "escape".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "page_up".into(),
        KeyCode::PageDown => "page_down".into(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return None,
    };
    let mut out = String::from("super+");
    if chord.mods.contains(KeyModifiers::CONTROL) {
        out.push_str("ctrl+");
    }
    if chord.mods.contains(KeyModifiers::ALT) {
        out.push_str("alt+");
    }
    if shift {
        out.push_str("shift+");
    }
    out.push_str(&key);
    Some(out)
}

/// The editing chords the BUILT-IN EDITOR takes — released whatever the
/// keymap says, so they reach micro, Edit or fresh instead of Ghostty:
/// the ones it takes as their Ctrl twins (`event_loop::cmd_as_ctrl`: ⌘S
/// save, ⌘Z undo, ⌘⇧Z redo, ⌘D next match, ⌘F find, ⌘A select all, ⌘X
/// cut), and the Mac editing chords it turns into the editor's own keys
/// (`editor::Kind::mac_key`): ⌘↑/⌘↓ and ⇧⌘↑/⇧⌘↓ to the file's ends —
/// Ghostty's jump to prompt — ⌥⌘↑/⌥⌘↓ a cursor above or below —
/// Ghostty's split up and down; ⌥⌘←/⌥⌘→ stay its split left and right —
/// ⌘L the line, ⌘⇧L every match, ⌘/ comment, ⌘⇧P the palette. ⌘←/⌘→
/// are not here: Ghostty types `^A`/`^E` for them, which the editor reads
/// as the line's ends, and which a shell outside orion still needs. Copy
/// and paste stay Ghostty's.
pub const EDITOR_CHORDS: &[&str] = &[
    "super+s",
    "super+z",
    "super+shift+z",
    "super+d",
    "super+f",
    "super+a",
    "super+x",
    "super+arrow_up",
    "super+arrow_down",
    "super+shift+arrow_up",
    "super+shift+arrow_down",
    "super+alt+arrow_up",
    "super+alt+arrow_down",
    "super+l",
    "super+shift+l",
    "super+/",
    "super+shift+p",
];

/// Whether the block releases `chord`: a ⌘ chord not on
/// [`NEVER_RELEASED`].
pub fn releases(chord: &KeyChord) -> bool {
    trigger(chord).is_some_and(|t| !NEVER_RELEASED.contains(&t.as_str()))
}

/// The triggers the block unbinds for `keymap`: every ⌘ chord any action
/// answers to, in the order the actions are declared, each once — a
/// digit twice, by its character and by its key (`super+0` and
/// `super+digit_0`), since Ghostty binds the tab digits both ways.
pub fn unbinds(keymap: &Keymap) -> Vec<String> {
    let mut out = keymap_unbinds(keymap);
    for chord in EDITOR_CHORDS {
        if !out.iter().any(|t| t == chord) {
            out.push((*chord).to_string());
        }
    }
    out
}

/// [`unbinds`]' keymap half: every ⌘ chord an action answers to.
fn keymap_unbinds(keymap: &Keymap) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for index in 0..crate::keymap::ACTIONS.len() {
        for chord in keymap.chords_at(index) {
            if !releases(chord) {
                continue;
            }
            let Some(trigger) = trigger(chord) else {
                continue;
            };
            let digit = match chord.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    Some(trigger.replace(&format!("+{c}"), &format!("+digit_{c}")))
                }
                _ => None,
            };
            for t in std::iter::once(trigger).chain(digit) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// The block as written for `keymap`: the markers around one
/// `keybind = <trigger>=unbind` per [`unbinds`] entry.
pub fn block(keymap: &Keymap) -> String {
    let mut out = String::from(BEGIN);
    out.push('\n');
    for trigger in unbinds(keymap) {
        out.push_str(&format!("keybind = {trigger}=unbind\n"));
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// `existing` with orion's block for `keymap` in it: an old block
/// replaced where it stands, otherwise the block appended after a blank
/// line.
pub fn with_block(existing: &str, keymap: &Keymap) -> String {
    let wanted = block(keymap);
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

/// Where orion's block stands in the text of a Ghostty config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// There, and what `keymap` would write.
    Current,
    /// There, but written for other keys — an older orion's, or before a
    /// rebind.
    Stale,
    Missing,
}

/// Whether `existing` holds orion's block for `keymap` as it would be
/// written now — what `orion doctor` reports.
pub fn block_state(existing: &str, keymap: &Keymap) -> BlockState {
    if with_block(existing, keymap) == existing {
        BlockState::Current
    } else if existing.contains(BEGIN) {
        BlockState::Stale
    } else {
        BlockState::Missing
    }
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

/// Put orion's block for `keymap` in the file at `path`, creating it (and
/// its folder) when missing. True when the file changed.
pub fn ensure(path: &Path, keymap: &Keymap) -> std::io::Result<bool> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let updated = with_block(&existing, keymap);
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

/// The startup pass, and the one after every change to the keymap or the
/// Ghostty settings: with the `ghostty_keybinds` SETTING on, on a local
/// Mac, and Ghostty in the picture — orion running inside it, or it being
/// the **Outside terminal** — make sure the block for the config's keymap
/// (its `keybindings` over the defaults) is in Ghostty's config
/// (`ORION_GHOSTTY_CONFIG` names another file, or `off` none). The flash
/// to show when the file changed or could not be written; None when there
/// was nothing to do. Unit tests never get past the first check: they must
/// not write the machine's real config.
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
    match ensure(&path, &cfg.keymap()) {
        Ok(true) => Some(format!(
            "updated orion's keybinds in {} — reload Ghostty's config (⌘⇧,) to use ⌘ chords",
            path.display()
        )),
        Ok(false) => None,
        Err(e) => Some(format!(
            "couldn't update Ghostty's config {}: {e}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The editor's ⌘ shortcuts are released with an empty keymap too,
    /// once each, and none of Ghostty's own copy, paste or close keys.
    #[test]
    fn the_editors_chords_are_always_released() {
        let all = unbinds(&Keymap::default());
        for chord in EDITOR_CHORDS {
            assert_eq!(all.iter().filter(|t| t == chord).count(), 1, "{all:?}");
            assert!(!NEVER_RELEASED.contains(chord));
        }
    }

    /// Every Mac editing chord the editor turns into its own key is in the
    /// block, spelled as [`trigger`] spells it — and ⌘←/⌘→, which Ghostty
    /// types as `^A`/`^E`, and ⌥⌘←/⌥⌘→, its split keys, are left to it.
    #[test]
    fn the_mac_editing_chords_are_released() {
        let all = unbinds(&Keymap::default());
        for spec in [
            "cmd+up",
            "cmd+down",
            "shift+cmd+up",
            "shift+cmd+down",
            "alt+cmd+up",
            "alt+cmd+down",
            "cmd+l",
            "shift+cmd+l",
            "cmd+/",
            "shift+cmd+p",
        ] {
            let t = trigger(&KeyChord::parse(spec).unwrap()).unwrap();
            assert!(all.contains(&t), "{spec} ({t}) not released");
        }
        for kept in [
            "super+arrow_left",
            "super+arrow_right",
            "super+alt+arrow_left",
            "super+alt+arrow_right",
        ] {
            assert!(!all.iter().any(|t| t == kept), "{kept} released");
        }
    }

    /// The default keymap's ⌘ chords, in the order the actions are
    /// declared — and nothing without ⌘.
    #[test]
    fn the_block_unbinds_every_cmd_chord_the_keymap_binds() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap_unbinds(&keymap),
            [
                "super+k",
                "super+e",
                "super+r",
                "super+l",
                "super+i",
                "super+n",
                "super+/",
                "super+y",
                "super+shift+/",
                "super+.",
                "super+p",
                "super+shift+f",
                "super+b",
                "super+s",
                "super+j",
                "super+f",
                "super+g",
                "super+,",
                "super+shift+p",
                "super+o",
            ]
        );
        let block = block(&keymap);
        assert!(block.starts_with(&format!("{BEGIN}\nkeybind = super+k=unbind\n")));
        assert!(block.ends_with(&format!("keybind = super+shift+l=unbind\n{END}\n")));
        assert!(
            !block.contains("ctrl+"),
            "a ^ twin is never Ghostty's to give"
        );
    }

    /// Every ⌘ chord an action answers to is released — the derivation
    /// covers the registry, not a list kept beside it.
    #[test]
    fn every_registry_cmd_chord_is_released() {
        let keymap = Keymap::default();
        let released = unbinds(&keymap);
        for index in 0..crate::keymap::ACTIONS.len() {
            for chord in keymap.chords_at(index) {
                match trigger(chord) {
                    Some(t) => assert!(released.contains(&t), "{t} not released"),
                    None => assert!(!chord.mods.contains(KeyModifiers::SUPER)),
                }
            }
        }
    }

    /// A rebind is the block: a ⌘ chord bound in Settings → Hotkeys is
    /// released, the default it replaced is handed back to Ghostty, and
    /// copy, paste, quit and the window keys are never taken.
    #[test]
    fn a_rebind_moves_the_block_with_it() {
        let mut keymap = Keymap::default();
        let palette = crate::keymap::index_of(crate::keymap::Action::Palette).unwrap();
        keymap.bind(palette, KeyChord::parse("cmd+shift+k").unwrap(), false);
        let released = unbinds(&keymap);
        assert!(released.contains(&"super+shift+k".to_string()));
        assert!(!released.contains(&"super+k".to_string()));
        keymap.bind(palette, KeyChord::parse("cmd+q").unwrap(), true);
        keymap.bind(palette, KeyChord::parse("cmd+0").unwrap(), true);
        let released = unbinds(&keymap);
        assert!(
            !released.contains(&"super+q".to_string()),
            "⌘Q stays Ghostty's"
        );
        assert!(released.contains(&"super+0".to_string()));
        assert!(released.contains(&"super+digit_0".to_string()));
        assert_ne!(block(&keymap), block(&Keymap::default()));
    }

    #[test]
    fn triggers_are_spelled_the_way_ghostty_reads_them() {
        let t = |spec: &str| trigger(&KeyChord::parse(spec).unwrap());
        assert_eq!(t("cmd+shift+p").as_deref(), Some("super+shift+p"));
        assert_eq!(t("cmd+?").as_deref(), Some("super+shift+/"));
        assert_eq!(t("shift+cmd+/").as_deref(), Some("super+shift+/"));
        assert_eq!(t("cmd+up").as_deref(), Some("super+arrow_up"));
        assert_eq!(t("ctrl+cmd+pgdn").as_deref(), Some("super+ctrl+page_down"));
        assert_eq!(t("ctrl+k"), None);
        assert_eq!(t("k"), None);
    }

    #[test]
    fn the_block_is_appended_after_the_users_own_lines() {
        let keymap = Keymap::default();
        assert_eq!(with_block("", &keymap), block(&keymap));
        assert_eq!(
            with_block("font-size = 14", &keymap),
            format!("font-size = 14\n\n{}", block(&keymap))
        );
    }

    #[test]
    fn an_old_block_is_replaced_where_it_stands() {
        let keymap = Keymap::default();
        let old = format!("theme = x\n{BEGIN}\nkeybind = super+t=unbind\n{END}\nfont-size = 14\n");
        assert_eq!(
            with_block(&old, &keymap),
            format!("theme = x\n{}font-size = 14\n", block(&keymap))
        );
        let current = with_block(&old, &keymap);
        assert_eq!(
            with_block(&current, &keymap),
            current,
            "a right block stays put"
        );
    }

    #[test]
    fn ensure_writes_once_and_creates_the_folder() {
        let keymap = Keymap::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.ghostty");
        assert!(ensure(&path, &keymap).unwrap());
        assert!(!ensure(&path, &keymap).unwrap(), "already right: untouched");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), block(&keymap));
        // A rebind rewrites it.
        let mut rebound = keymap.clone();
        let index = crate::keymap::index_of(crate::keymap::Action::Skills).unwrap();
        rebound.bind(index, KeyChord::parse("cmd+u").unwrap(), false);
        assert!(ensure(&path, &rebound).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("keybind = super+u=unbind"));
        // ⌘S stays released all the same: it is the editor's save.
        assert!(text.contains("keybind = super+s=unbind"));
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
