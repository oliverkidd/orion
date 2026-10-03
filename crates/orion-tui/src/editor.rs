//! The BUILT-IN EDITOR every file opens in: which program that is, and how
//! each one is told the file and the line.
//!
//! The `editor` setting names it (`ORION_EDITOR` over it). Its choices are
//! the VS Code-style editors first — fresh, micro and Microsoft Edit
//! (`edit`), whose keys are the ones a GUI editor taught everyone (`^S`
//! save, `^Z` undo, `^C`/`^V`, `^F` find, `^Q` quit) — then vim, nvim,
//! Helix and emacs. A chosen editor that isn't installed is never swapped
//! for another in silence: [`resolve`] says which it fell back to — the
//! first installed of [`FALLBACKS`] — so the editor modal can say so once
//! and the settings row every time.
//!
//! micro runs off a config dir of orion's own (`<data dir>/micro`), so its
//! bindings here — `Ctrl+D` adds the next match as another cursor, as in
//! Cursor — and its soft wrap never touch the user's `~/.config/micro`, and
//! that config never changes how the modal behaves. The dir is made on
//! first use; after that its files are the user's to edit, and orion only
//! ever adds a setting they lack. fresh runs off a config file of orion's
//! own the same way (`<data dir>/fresh/config.json`, [`FRESH_CONFIG`]):
//! the one file opened, no menu, tab bar, file explorer or workspace dock,
//! a two-item status bar, and VS Code's Dark+ colours on orion's own
//! background.
//!
//! In micro, Edit and fresh a Mac's text-editing chords mean what they mean
//! in VS Code on a Mac — `⌘←` the line's start, `⌥→` the next word's end,
//! `⌘L` the line, `⌘⇧L` every match ([`Kind::mac_key`]): each is turned
//! into the key that editor binds the same action to.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};

/// The editor every fresh config names.
pub const DEFAULT_EDITOR: &str = "fresh";

/// The **File editor** row's choices, in the order it cycles them: the
/// VS Code-style editors first. Any other command passes through as typed.
pub const EDITORS: &[&str] = &[
    DEFAULT_EDITOR,
    "micro",
    "edit",
    "vim",
    "nvim",
    "hx",
    "emacs",
];

/// Where a chosen editor that isn't installed goes instead: the first of
/// these on PATH. vim is last — the one editor nobody wants by surprise.
pub const FALLBACKS: &[&str] = &[DEFAULT_EDITOR, "micro", "edit", "vim"];

/// The micro bindings orion's config dir holds, each added to a
/// `bindings.json` that lacks the key — one the user bound stays theirs:
/// `Ctrl+D` adds the next match as another cursor, as in Cursor, and the
/// rest are the keys [`Kind::mac_key`] turns a Mac chord into, pinned to
/// one action on every platform (micro's Linux defaults swap `Alt`'s and
/// `Ctrl`'s arrows): `Alt+L` selects the line, `Alt+:` opens micro's
/// command bar (`Ctrl+E` is `⌘→`'s line end here), `Ctrl+Shift+Home/End`
/// select to the file's ends.
pub const MICRO_BINDINGS: &[(&str, &str)] = &[
    ("Ctrl-d", "SpawnMultiCursor"),
    ("Alt-l", "SelectLine"),
    ("Alt-:", "CommandMode"),
    ("CtrlShiftHome", "SelectToStart"),
    ("CtrlShiftEnd", "SelectToEnd"),
    ("AltLeft", "WordLeft"),
    ("AltRight", "WordRight"),
    ("AltShiftLeft", "SelectWordLeft"),
    ("AltShiftRight", "SelectWordRight"),
    ("AltShiftUp", "SpawnMultiCursorUp"),
    ("AltShiftDown", "SpawnMultiCursorDown"),
];

/// The settings in orion's own fresh config, merged into the file like
/// [`MICRO_SETTINGS`]: a key the user set — at any depth — stays as they
/// set it. The one file asked for and nothing around it: no menu bar, tab
/// bar, scrollbar, `~` past the end, whitespace dots, edge fade or
/// animation; no workspace dock (the `orchestrator` plugin), no file
/// explorer, no session restored around the file; no update checks. The
/// status bar keeps the cursor, the cursor count and fresh's messages on
/// the left and the language on the right. The theme is orion's own
/// ([`FRESH_THEME`]): Cursor's default Cursor Dark — its background, its
/// colours for keywords, strings, functions, types and comments, VS Code's
/// gold/orchid/blue bracket pairs — with VS Code's selection blue and a
/// current line barely lifted off the background, so what is selected
/// inside the cursor's line stands out.
/// fresh's cursor animations stay on. The `default` keymap, not the `macos` one fresh picks
/// on a Mac (it moves `Ctrl+A` to the line start), is the one
/// [`Kind::mac_key`]'s keys are read in.
pub const FRESH_CONFIG: &str = r#"{
  "theme": "orion",
  "check_for_updates": false,
  "self_update": false,
  "orchestrator_mode": false,
  "active_keybinding_map": "default",
  "plugins": { "orchestrator": { "enabled": false } },
  "file_explorer": { "auto_open_on_last_buffer_close": false },
  "editor": {
    "show_menu_bar": false,
    "show_tab_bar": false,
    "show_vertical_scrollbar": false,
    "show_tilde": false,
    "whitespace_show": false,
    "viewport_edge_fade": false,
    "animations": true,
    "cursor_jump_animation": true,
    "hide_current_line_on_selection": true,
    "restore_previous_session": false,
    "use_terminal_bg": true,
    "line_wrap": true,
    "status_bar": {
      "left": ["{cursor}", "{cursor_count}", "{messages}"],
      "right": ["{language}"],
      "separator": "  "
    }
  }
}
"#;

/// How many `^D`s — each micro's and fresh's "add the next match as a
/// cursor" — `⌘⇧L` types at once to put a cursor on every match. Both stop
/// at the last one (fresh: "All matches are already selected"), so the
/// rest are no-ops. Sent as the bare 0x04 byte whatever keyboard protocol
/// the editor pushed — every key parser reads it as `Ctrl+D` — so the
/// burst stays well inside the tty's 1 KiB input queue and the write never
/// waits on the editor.
pub const SELECT_ALL_MATCHES: [u8; 500] = [0x04; 500];

/// What a Mac editing chord becomes in the editor ([`Kind::mac_key`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacKey {
    /// The key the editor binds the chord's action to.
    Key(KeyEvent),
    /// Bytes written to the editor as they are: `⌘⇧L`'s burst.
    Bytes(&'static [u8]),
}

/// The micro settings orion's config dir holds: soft wrap at word
/// boundaries, so a long line never runs off the modal. Each one is added
/// to a `settings.json` that lacks it; one the user set stays as they set
/// it.
pub const MICRO_SETTINGS: &[(&str, bool)] = &[("softwrap", true), ("wordwrap", true)];

/// The editors orion knows how to drive, by their program's file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Micro,
    /// Microsoft Edit.
    Edit,
    Fresh,
    /// vim, nvim, vi.
    Vim,
    Helix,
    Emacs,
    /// Anything else, launched as `<command> +<line> <file>`.
    Other,
}

impl Kind {
    pub fn of(editor: &str) -> Self {
        match program_name(editor) {
            "micro" => Kind::Micro,
            "edit" | "msedit" => Kind::Edit,
            "fresh" => Kind::Fresh,
            "vim" | "nvim" | "vi" => Kind::Vim,
            "hx" | "helix" => Kind::Helix,
            "emacs" => Kind::Emacs,
            _ => Kind::Other,
        }
    }

    /// Ctrl+Q is the editor's own quit, which asks to save first — so the
    /// modal hands it over rather than force-closing on it.
    pub fn quits_on_ctrl_q(self) -> bool {
        matches!(self, Kind::Micro | Kind::Edit | Kind::Fresh)
    }

    /// The editor's own keys the modal names: how to save and leave it.
    pub fn keys_hint(self) -> &'static str {
        match self {
            Kind::Micro => "Ctrl+S: save  Ctrl+Q: quit  Ctrl+D: next match",
            Kind::Edit => "Ctrl+S: save  Ctrl+Q: quit  F10: menu",
            Kind::Fresh => "Ctrl+S: save  Ctrl+Q: quit  Ctrl+P: palette",
            Kind::Vim | Kind::Helix => ":w save  :q quit",
            Kind::Emacs => "Ctrl+X Ctrl+S: save  Ctrl+X Ctrl+C: quit",
            Kind::Other => "",
        }
    }

    /// Keys the modal types into the editor once it has drawn its first
    /// screen: Microsoft Edit's `Alt+Z`, its word wrap, which nothing in
    /// its settings turns on. micro wraps by its settings
    /// ([`MICRO_SETTINGS`]), fresh, vim and emacs out of the box.
    pub fn startup_keys(self) -> Option<&'static [u8]> {
        match self {
            Kind::Edit => Some(b"\x1bz"),
            _ => None,
        }
    }

    /// `key` as a Mac's text-editing chord, in the key this editor binds
    /// the same action to — micro, Edit and fresh only, None for any other
    /// key (it goes on as it came, a ⌘ letter as its Ctrl twin):
    ///
    /// | Chord | Means | fresh | micro | Edit |
    /// |---|---|---|---|---|
    /// | `⌘←` `⌘→` (Ghostty's `^A` `^E`) | line start / end | `Home` `End` | same | same |
    /// | `⇧⌘←` `⇧⌘→` | select to it | `⇧Home` `⇧End` | same | same |
    /// | `⌘↑` `⌘↓` | file start / end | `^Home` `^End` | same | same |
    /// | `⇧⌘↑` `⇧⌘↓` | select to it | `^⇧Home` `^⇧End` | same | same |
    /// | `⌥←` `⌥→` (Ghostty's `⎋b` `⎋f`) | word | `^←` `^→` | `⌥←` `⌥→` | `^←` `^→` |
    /// | `⇧⌥←` `⇧⌥→` | select word | `^⇧←` `^⇧→` | `⌥⇧←` `⌥⇧→` | `^⇧←` `^⇧→` |
    /// | `⌥⌘↑` `⌥⌘↓` | cursor above / below | `^⌥↑` `^⌥↓` | `⌥⇧↑` `⌥⇧↓` | — |
    /// | `⌘L` | select the line | `^L` | `⌥L` | — |
    /// | `⌘/` | toggle comment | `^/` | `⌥/` | — |
    /// | `⌘⇧L` | a cursor on every match | [`SELECT_ALL_MATCHES`] | same | — |
    /// | `⌘⇧P` | command palette | `^P` | `⌥:` (its command bar) | — |
    ///
    /// Ghostty keeps `⌘←`/`⌘→` and `⌥←`/`⌥→` for itself and types the
    /// shell's keys for them, so those arrive as `^A`/`^E` and
    /// `Alt+B`/`Alt+F` — and a bare `^A`/`^E` here is the line's start and
    /// end, as in any Mac text field (`⌘A` stays select all).
    pub fn mac_key(self, key: &KeyEvent) -> Option<MacKey> {
        if !matches!(self, Kind::Micro | Kind::Edit | Kind::Fresh) {
            return None;
        }
        let m = key.modifiers;
        let (cmd, alt, ctrl) = (
            m.contains(KeyModifiers::SUPER),
            m.contains(KeyModifiers::ALT),
            m.contains(KeyModifiers::CONTROL),
        );
        let upper = matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase());
        let shift = m.contains(KeyModifiers::SHIFT) || upper;
        let sel = if shift {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        let to = |code, mods| Some(MacKey::Key(KeyEvent::new(code, mods)));
        let left = |c: char| {
            if c == 'b' {
                KeyCode::Left
            } else {
                KeyCode::Right
            }
        };
        // A word left or right: micro's pinned Alt arrows, the others' Ctrl.
        let word = |code: KeyCode, mods: KeyModifiers| match self {
            Kind::Micro => to(code, KeyModifiers::ALT | mods),
            _ => to(code, KeyModifiers::CONTROL | mods),
        };
        match key.code {
            KeyCode::Char(c) => match (c.to_ascii_lowercase(), cmd, alt, ctrl, shift) {
                ('a', false, false, true, false) => to(KeyCode::Home, KeyModifiers::NONE),
                ('e', false, false, true, false) => to(KeyCode::End, KeyModifiers::NONE),
                (c @ ('b' | 'f'), false, true, false, false) => word(left(c), KeyModifiers::NONE),
                ('l', true, false, false, false) => match self {
                    Kind::Fresh => to(KeyCode::Char('l'), KeyModifiers::CONTROL),
                    Kind::Micro => to(KeyCode::Char('l'), KeyModifiers::ALT),
                    _ => None,
                },
                ('l', true, false, false, true) => match self {
                    Kind::Edit => None,
                    _ => Some(MacKey::Bytes(&SELECT_ALL_MATCHES)),
                },
                ('p', true, false, false, true) => match self {
                    Kind::Fresh => to(KeyCode::Char('p'), KeyModifiers::CONTROL),
                    Kind::Micro => to(KeyCode::Char(':'), KeyModifiers::ALT),
                    _ => None,
                },
                ('/', true, false, false, false) => match self {
                    Kind::Fresh => to(KeyCode::Char('/'), KeyModifiers::CONTROL),
                    Kind::Micro => to(KeyCode::Char('/'), KeyModifiers::ALT),
                    _ => None,
                },
                _ => None,
            },
            KeyCode::Left | KeyCode::Right if !ctrl => match (cmd, alt) {
                (true, false) => {
                    let end = key.code == KeyCode::Right;
                    to(if end { KeyCode::End } else { KeyCode::Home }, sel)
                }
                (false, true) => word(key.code, sel),
                _ => None,
            },
            KeyCode::Up | KeyCode::Down if !ctrl => {
                let down = key.code == KeyCode::Down;
                match (cmd, alt) {
                    (true, false) => to(
                        if down { KeyCode::End } else { KeyCode::Home },
                        KeyModifiers::CONTROL | sel,
                    ),
                    (true, true) if !shift => match self {
                        Kind::Fresh => to(key.code, KeyModifiers::CONTROL | KeyModifiers::ALT),
                        Kind::Micro => to(key.code, KeyModifiers::ALT | KeyModifiers::SHIFT),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Whether `editor` is micro, by its program's file name.
pub fn is_micro(editor: &str) -> bool {
    Kind::of(editor) == Kind::Micro
}

/// The program's file name: `/opt/homebrew/bin/micro -autosu true` is
/// `micro`.
pub fn program_name(editor: &str) -> &str {
    let program = editor.split_whitespace().next().unwrap_or("");
    program.rsplit('/').next().unwrap_or(program)
}

/// What the BUILT-IN EDITOR runs, and — when the editor asked for isn't
/// installed — that one's program, for the modal and the settings row to
/// say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub command: String,
    /// The program asked for, when it isn't installed and `command` is the
    /// fallback standing in for it.
    pub missing: Option<String>,
}

/// The first non-blank of the env override and the setting, else fresh. A
/// program `installed` doesn't know is swapped for the first of
/// [`FALLBACKS`] it does, and named in [`Resolved::missing`]. With none of
/// them installed the chosen command stands, and its spawn says why it
/// failed.
pub fn resolve(env: Option<&str>, configured: &str, installed: impl Fn(&str) -> bool) -> Resolved {
    let chosen = [env.unwrap_or(""), configured]
        .into_iter()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or(DEFAULT_EDITOR);
    let program = chosen.split_whitespace().next().unwrap_or(chosen);
    let as_chosen = Resolved {
        command: chosen.to_string(),
        missing: None,
    };
    if installed(program) {
        return as_chosen;
    }
    match FALLBACKS.iter().find(|fallback| installed(fallback)) {
        Some(fallback) => Resolved {
            command: fallback.to_string(),
            missing: Some(program.to_string()),
        },
        None => as_chosen,
    }
}

/// Where orion keeps its editors' own config: `<data dir>`, micro's dir and
/// fresh's file under it ([`micro_dir`], [`fresh_config`]). Unit tests that
/// open an editor get one under the temp dir instead of the machine's data
/// dir.
pub fn config_root() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join("orion-unit-tests-editors");
    }
    orion_core::paths::data_dir()
}

/// orion's own micro config dir under `root`.
pub fn micro_dir(root: &Path) -> PathBuf {
    root.join("micro")
}

/// orion's own fresh config file under `root`.
pub fn fresh_config(root: &Path) -> PathBuf {
    root.join("fresh").join("config.json")
}

/// Make the config `editor` runs off under `root` — micro's dir, fresh's
/// file and theme — before it starts. Any other editor has none.
pub fn ensure_config(editor: &str, root: &Path) -> std::io::Result<()> {
    match Kind::of(editor) {
        Kind::Micro => ensure_micro_config(&micro_dir(root)),
        Kind::Fresh => {
            if let Some(home) = orion_core::env::home_dir() {
                ensure_fresh_theme(&home)?;
            }
            ensure_fresh_config(&fresh_config(root))
        }
        _ => Ok(()),
    }
}

/// orion's fresh theme, named `orion` ([`FRESH_CONFIG`]'s `theme`).
pub const FRESH_THEME: &str = include_str!("fresh_theme.json");

/// Keep [`FRESH_THEME`] at `<home>/.config/fresh/themes/orion.json`, the one
/// folder fresh reads theme files from. The file is orion's — its name is
/// the theme's — so it is rewritten whenever it differs.
pub fn ensure_fresh_theme(home: &Path) -> std::io::Result<()> {
    let dir = home.join(".config/fresh/themes");
    let path = dir.join("orion.json");
    if std::fs::read_to_string(&path).is_ok_and(|text| text == FRESH_THEME) {
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(path, FRESH_THEME)
}

/// Make `dir` a micro config dir: created when missing, with every one of
/// [`MICRO_BINDINGS`] its `bindings.json` lacks and every one of
/// [`MICRO_SETTINGS`] its `settings.json` lacks. A file that isn't a plain
/// JSON object — micro reads comments too — is left exactly as it is.
pub fn ensure_micro_config(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let bindings = MICRO_BINDINGS
        .iter()
        .map(|(key, action)| ((*key).to_string(), serde_json::json!(action)));
    merge_into_file(&dir.join("bindings.json"), &bindings.collect())?;
    let settings = MICRO_SETTINGS
        .iter()
        .map(|(key, on)| ((*key).to_string(), serde_json::json!(on)));
    merge_into_file(&dir.join("settings.json"), &settings.collect())
}

/// Make `path` orion's fresh config: [`FRESH_CONFIG`] merged into what is
/// there, as [`merge_into_file`] does.
pub fn ensure_fresh_config(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let wanted: serde_json::Value =
        serde_json::from_str(FRESH_CONFIG).map_err(std::io::Error::other)?;
    let serde_json::Value::Object(wanted) = wanted else {
        return Ok(());
    };
    merge_into_file(path, &wanted)
}

/// Add every key of `wanted` the JSON object in `path` lacks — into nested
/// objects too, so a user's own `editor.tab_size` keeps company with
/// orion's `editor.show_tab_bar` — writing the file only when something
/// was added. A key already there keeps its value. A missing file starts
/// empty; one that isn't a plain JSON object is left exactly as it is.
fn merge_into_file(
    path: &Path,
    wanted: &serde_json::Map<String, serde_json::Value>,
) -> std::io::Result<()> {
    let mut object = match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(object)) => object,
            _ => return Ok(()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => return Err(e),
    };
    if merge_missing(&mut object, wanted) {
        let mut text = serde_json::to_string_pretty(&object).map_err(std::io::Error::other)?;
        text.push('\n');
        std::fs::write(path, text)?;
    }
    Ok(())
}

/// [`merge_into_file`]'s merge: true when anything was added.
fn merge_missing(
    have: &mut serde_json::Map<String, serde_json::Value>,
    wanted: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let mut added = false;
    for (key, value) in wanted {
        match (have.get_mut(key), value) {
            (None, _) => {
                have.insert(key.clone(), value.clone());
                added = true;
            }
            (Some(serde_json::Value::Object(mine)), serde_json::Value::Object(theirs)) => {
                added |= merge_missing(mine, theirs);
            }
            _ => {}
        }
    }
    added
}

/// The argv after the program for opening `file` at `line`, the way each
/// editor's `--help` says it takes one:
///
/// - micro: `-config-dir <orion's> <file> +<line>`
/// - fresh: `--config <orion's> --no-upgrade-check --no-restore` and then,
///   as Microsoft Edit and Helix take it, `<file>:<line>` — the bare file
///   on line 1, so a name with a colon in it is never read as a line
/// - vim, nvim, emacs and anything else: `+<line> <file>`
///
/// `root` is [`config_root`], where micro's dir and fresh's file live.
pub fn editor_args(editor: &str, root: &Path, file: &str, line: u64) -> Vec<String> {
    let at = format!("+{line}");
    let file_at = if line > 1 {
        format!("{file}:{line}")
    } else {
        file.to_string()
    };
    match Kind::of(editor) {
        Kind::Micro => vec![
            "-config-dir".into(),
            micro_dir(root).to_string_lossy().into_owned(),
            file.to_string(),
            at,
        ],
        Kind::Fresh => vec![
            "--config".into(),
            fresh_config(root).to_string_lossy().into_owned(),
            "--no-upgrade-check".into(),
            "--no-restore".into(),
            file_at,
        ],
        Kind::Edit | Kind::Helix => vec![file_at],
        Kind::Vim | Kind::Emacs | Kind::Other => vec![at, file.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The theme lands where fresh reads themes, named for the config's
    /// `theme`, selection clearly apart from the current line.
    #[test]
    fn the_fresh_theme_is_written_once_where_fresh_reads_it() {
        let home = tempfile::tempdir().unwrap();
        ensure_fresh_theme(home.path()).unwrap();
        let path = home.path().join(".config/fresh/themes/orion.json");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FRESH_THEME);
        let theme: serde_json::Value = serde_json::from_str(FRESH_THEME).unwrap();
        assert_eq!(theme["name"], "orion");
        assert_ne!(theme["editor"]["selection_bg"], theme["editor"]["current_line_bg"]);
        let config: serde_json::Value = serde_json::from_str(FRESH_CONFIG).unwrap();
        assert_eq!(config["theme"], "orion");
        assert_eq!(config["editor"]["cursor_jump_animation"], true);
    }

    #[test]
    fn micro_is_known_by_its_program_name() {
        assert!(is_micro("micro"));
        assert!(is_micro("/opt/homebrew/bin/micro"));
        assert!(is_micro("micro -autosu true"));
        assert!(!is_micro("vim"));
        assert!(!is_micro("micromamba"));
        assert!(!is_micro(""));
        assert_eq!(Kind::of("/usr/bin/vim"), Kind::Vim);
        assert_eq!(Kind::of("nano"), Kind::Other, "no special handling");
        assert_eq!(Kind::of("nvim"), Kind::Vim);
        assert_eq!(Kind::of("edit"), Kind::Edit);
        assert_eq!(Kind::of("code --wait"), Kind::Other);
    }

    /// Each editor gets the line the way its `--help` documents: micro's
    /// `+N` after the file off orion's config dir, fresh's `file:N` off
    /// orion's config file, Edit's `file:N`, `+N file` for the rest.
    #[test]
    fn each_editor_is_told_the_line_its_own_way() {
        let dir = Path::new("/data");
        assert_eq!(
            editor_args("micro", dir, "src/a.rs", 12),
            ["-config-dir", "/data/micro", "src/a.rs", "+12"]
        );
        assert_eq!(editor_args("edit", dir, "src/a.rs", 40), ["src/a.rs:40"]);
        assert_eq!(
            editor_args("fresh", dir, "src/a.rs", 40),
            [
                "--config",
                "/data/fresh/config.json",
                "--no-upgrade-check",
                "--no-restore",
                "src/a.rs:40"
            ]
        );
        assert_eq!(editor_args("fresh", dir, "a.rs", 1).last().unwrap(), "a.rs");
        assert_eq!(editor_args("hx", dir, "src/a.rs", 2), ["src/a.rs:2"]);
        assert_eq!(
            editor_args("edit", dir, "notes:2.txt", 1),
            ["notes:2.txt"],
            "line 1 is the bare file: no colon to misread"
        );
        assert_eq!(editor_args("vim", dir, "src/a.rs", 3), ["+3", "src/a.rs"]);
        assert_eq!(editor_args("nvim", dir, "src/a.rs", 3), ["+3", "src/a.rs"]);
        assert_eq!(editor_args("emacs", dir, "a.rs", 9), ["+9", "a.rs"]);
        assert_eq!(editor_args("/bin/sh", dir, "a.rs", 1), ["+1", "a.rs"]);
    }

    #[test]
    fn only_the_vs_code_style_editors_quit_on_ctrl_q() {
        for editor in ["micro", "edit", "fresh"] {
            assert!(Kind::of(editor).quits_on_ctrl_q(), "{editor}");
        }
        for editor in ["vim", "nvim", "hx", "emacs", "/bin/sh"] {
            assert!(!Kind::of(editor).quits_on_ctrl_q(), "{editor}");
        }
        assert_eq!(
            Kind::Edit.startup_keys(),
            Some(&b"\x1bz"[..]),
            "Edit's Alt+Z wrap"
        );
        assert_eq!(Kind::Micro.startup_keys(), None);
    }

    #[test]
    fn resolution_prefers_env_then_setting_then_fresh() {
        let all = |_: &str| true;
        let chosen = |command: &str| Resolved {
            command: command.into(),
            missing: None,
        };
        assert_eq!(resolve(Some("hx"), "nvim", all), chosen("hx"));
        assert_eq!(resolve(Some("  "), "nvim", all), chosen("nvim"));
        assert_eq!(resolve(None, " nvim ", all), chosen("nvim"));
        assert_eq!(resolve(None, "", all), chosen("fresh"));
        assert_eq!(
            resolve(None, "micro -autosu true", all),
            chosen("micro -autosu true"),
            "a hand-typed command passes through"
        );
    }

    /// A chosen editor that isn't installed goes to the first installed of
    /// fresh → micro → edit → vim, and says which one it was.
    #[test]
    fn a_missing_editor_falls_back_in_order_and_says_so() {
        let only = |have: &'static [&'static str]| move |program: &str| have.contains(&program);
        let fell = |command: &str, missing: &str| Resolved {
            command: command.into(),
            missing: Some(missing.into()),
        };
        assert_eq!(
            resolve(None, "fresh", only(&["vim", "edit", "micro"])),
            fell("micro", "fresh")
        );
        assert_eq!(
            resolve(None, "nvim", only(&["vim", "edit"])),
            fell("edit", "nvim")
        );
        assert_eq!(
            resolve(None, "", only(&["vim", "edit"])),
            fell("edit", "fresh"),
            "edit before vim"
        );
        assert_eq!(resolve(None, "hx", only(&["vim"])), fell("vim", "hx"));
        assert_eq!(
            resolve(Some("/opt/bin/kak -n"), "micro", only(&["micro"])),
            fell("micro", "/opt/bin/kak")
        );
        assert_eq!(
            resolve(None, "nvim", only(&[])),
            Resolved {
                command: "nvim".into(),
                missing: None
            },
            "nothing to fall back to: the spawn says why"
        );
    }

    #[test]
    fn micro_bindings_are_added_never_over_the_users() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("micro");
        ensure_micro_config(&dir).unwrap();
        let bindings = dir.join("bindings.json");
        let read = || -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(&bindings).unwrap()).unwrap()
        };
        assert_eq!(read()["Ctrl-d"], "SpawnMultiCursor");
        assert_eq!(read()["Alt-l"], "SelectLine");
        assert_eq!(read().as_object().unwrap().len(), MICRO_BINDINGS.len());

        std::fs::write(&bindings, r#"{"Ctrl-d": "Duplicate"}"#).unwrap();
        ensure_micro_config(&dir).unwrap();
        assert_eq!(read()["Ctrl-d"], "Duplicate", "the user's binding stays");
        assert_eq!(
            read()["AltShiftUp"],
            "SpawnMultiCursorUp",
            "a missing one is added"
        );

        let commented = "{\n  // mine\n}\n";
        std::fs::write(&bindings, commented).unwrap();
        ensure_micro_config(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&bindings).unwrap(), commented);
    }

    /// orion's fresh config: the whole of [`FRESH_CONFIG`] at first, then
    /// only what the file lacks — a nested key the user changed stays, its
    /// siblings are filled in.
    #[test]
    fn the_fresh_config_is_merged_never_over_the_users() {
        let home = tempfile::tempdir().unwrap();
        let path = fresh_config(home.path());
        ensure_config("fresh", home.path()).unwrap();
        let read = || -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
        };
        let wanted: serde_json::Value = serde_json::from_str(FRESH_CONFIG).unwrap();
        assert_eq!(read(), wanted);
        assert_eq!(read()["editor"]["show_tab_bar"], false);
        assert_eq!(read()["plugins"]["orchestrator"]["enabled"], false);

        std::fs::write(
            &path,
            r#"{"theme": "builtin://nord", "editor": {"show_tab_bar": true, "tab_size": 2}}"#,
        )
        .unwrap();
        ensure_fresh_config(&path).unwrap();
        assert_eq!(read()["theme"], "builtin://nord", "the user's theme stays");
        assert_eq!(read()["editor"]["show_tab_bar"], true);
        assert_eq!(read()["editor"]["tab_size"], 2);
        assert_eq!(
            read()["editor"]["show_menu_bar"],
            false,
            "a missing one is added"
        );
        assert_eq!(read()["active_keybinding_map"], "default");

        let before = std::fs::read_to_string(&path).unwrap();
        ensure_fresh_config(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "nothing to add"
        );
        assert!(ensure_config("vim", home.path()).is_ok());
    }

    /// The Mac chords per editor, as [`Kind::mac_key`]'s table has them.
    #[test]
    fn mac_chords_become_each_editors_own_keys() {
        use KeyModifiers as M;
        let k = |code, mods| KeyEvent::new(code, mods);
        let key = |code, mods| Some(MacKey::Key(k(code, mods)));
        let (cmd, alt, shift, ctrl) = (M::SUPER, M::ALT, M::SHIFT, M::CONTROL);
        for kind in [Kind::Fresh, Kind::Micro, Kind::Edit] {
            let t = |code, mods| kind.mac_key(&k(code, mods));
            // Line and file ends, the same key in all three.
            assert_eq!(t(KeyCode::Left, cmd), key(KeyCode::Home, M::NONE));
            assert_eq!(t(KeyCode::Right, cmd), key(KeyCode::End, M::NONE));
            assert_eq!(t(KeyCode::Char('a'), ctrl), key(KeyCode::Home, M::NONE));
            assert_eq!(t(KeyCode::Char('e'), ctrl), key(KeyCode::End, M::NONE));
            assert_eq!(t(KeyCode::Left, cmd | shift), key(KeyCode::Home, shift));
            assert_eq!(t(KeyCode::Right, cmd | shift), key(KeyCode::End, shift));
            assert_eq!(t(KeyCode::Up, cmd), key(KeyCode::Home, ctrl));
            assert_eq!(t(KeyCode::Down, cmd), key(KeyCode::End, ctrl));
            assert_eq!(
                t(KeyCode::Up, cmd | shift),
                key(KeyCode::Home, ctrl | shift)
            );
            assert_eq!(
                t(KeyCode::Down, cmd | shift),
                key(KeyCode::End, ctrl | shift)
            );
            // Not Mac chords: left alone.
            assert_eq!(t(KeyCode::Char('a'), cmd), None, "⌘A: Ctrl+A, select all");
            assert_eq!(t(KeyCode::Char('s'), cmd), None);
            assert_eq!(t(KeyCode::Up, alt), None, "⌥↑: the editor's move line");
            assert_eq!(t(KeyCode::Left, M::NONE), None);
            assert_eq!(t(KeyCode::Char('b'), M::NONE), None);
        }
        let fresh = |code, mods| Kind::Fresh.mac_key(&k(code, mods));
        assert_eq!(fresh(KeyCode::Left, alt), key(KeyCode::Left, ctrl));
        assert_eq!(fresh(KeyCode::Char('f'), alt), key(KeyCode::Right, ctrl));
        assert_eq!(
            fresh(KeyCode::Right, alt | shift),
            key(KeyCode::Right, ctrl | shift)
        );
        assert_eq!(fresh(KeyCode::Up, cmd | alt), key(KeyCode::Up, ctrl | alt));
        assert_eq!(
            fresh(KeyCode::Char('l'), cmd),
            key(KeyCode::Char('l'), ctrl)
        );
        assert_eq!(
            fresh(KeyCode::Char('/'), cmd),
            key(KeyCode::Char('/'), ctrl)
        );
        assert_eq!(
            fresh(KeyCode::Char('P'), cmd | shift),
            key(KeyCode::Char('p'), ctrl)
        );

        let micro = |code, mods| Kind::Micro.mac_key(&k(code, mods));
        assert_eq!(micro(KeyCode::Char('b'), alt), key(KeyCode::Left, alt));
        assert_eq!(micro(KeyCode::Right, alt), key(KeyCode::Right, alt));
        assert_eq!(
            micro(KeyCode::Left, alt | shift),
            key(KeyCode::Left, alt | shift)
        );
        assert_eq!(
            micro(KeyCode::Down, cmd | alt),
            key(KeyCode::Down, alt | shift)
        );
        assert_eq!(micro(KeyCode::Char('l'), cmd), key(KeyCode::Char('l'), alt));
        assert_eq!(micro(KeyCode::Char('/'), cmd), key(KeyCode::Char('/'), alt));
        assert_eq!(
            micro(KeyCode::Char('p'), cmd | shift),
            key(KeyCode::Char(':'), alt)
        );

        let edit = |code, mods| Kind::Edit.mac_key(&k(code, mods));
        assert_eq!(edit(KeyCode::Char('b'), alt), key(KeyCode::Left, ctrl));
        assert_eq!(edit(KeyCode::Up, cmd | alt), None, "no multiple cursors");
        assert_eq!(edit(KeyCode::Char('l'), cmd), None);
        assert_eq!(edit(KeyCode::Char('L'), cmd | shift), None);

        for kind in [Kind::Vim, Kind::Emacs, Kind::Helix, Kind::Other] {
            assert_eq!(kind.mac_key(&k(KeyCode::Left, cmd)), None, "{kind:?}");
            assert_eq!(kind.mac_key(&k(KeyCode::Char('a'), ctrl)), None, "{kind:?}");
        }
    }

    /// `⌘⇧L` — however the terminal spells the shifted letter — is a burst
    /// of `^D` bytes, each the editor's next-match cursor, in one write
    /// small enough for the tty's input queue.
    #[test]
    fn cmd_shift_l_is_a_burst_of_next_match_cursors() {
        let sup_shift = KeyModifiers::SUPER | KeyModifiers::SHIFT;
        for code in [KeyCode::Char('l'), KeyCode::Char('L')] {
            for kind in [Kind::Fresh, Kind::Micro] {
                let Some(MacKey::Bytes(bytes)) = kind.mac_key(&KeyEvent::new(code, sup_shift))
                else {
                    panic!("{kind:?}: no burst");
                };
                assert!(bytes.iter().all(|&b| b == 0x04));
                assert_eq!(bytes.len(), 500);
                assert!(bytes.len() < 1024, "inside the tty's input queue");
            }
        }
    }

    /// Soft wrap is on in orion's micro dir: added where the settings lack
    /// it, never over a value the user set, never into a file micro's own
    /// looser JSON holds.
    #[test]
    fn micro_soft_wraps_unless_told_otherwise() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("micro");
        ensure_micro_config(&dir).unwrap();
        let settings = dir.join("settings.json");
        let read = || -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap()
        };
        assert_eq!(read()["softwrap"], true);
        assert_eq!(read()["wordwrap"], true);

        std::fs::write(&settings, r#"{"softwrap": false, "tabsize": 2}"#).unwrap();
        ensure_micro_config(&dir).unwrap();
        assert_eq!(read()["softwrap"], false, "the user's choice stands");
        assert_eq!(read()["wordwrap"], true, "the missing one is added");
        assert_eq!(read()["tabsize"], 2);

        let commented = "{\n  // mine\n  \"softwrap\": false,\n}\n";
        std::fs::write(&settings, commented).unwrap();
        ensure_micro_config(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), commented);
    }
}
