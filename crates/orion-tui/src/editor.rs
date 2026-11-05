//! The BUILT-IN EDITOR every file opens in: fresh, whose keys are the ones
//! a GUI editor taught everyone (`^S` save, `^Z` undo, `^C`/`^V`, `^F`
//! find, `^Q` quit), and how it is told the file and the line.
//! `ORION_EDITOR` names any other command instead ([`command`]), opened
//! as `<command> +<line> <file>`.
//!
//! fresh runs off a config file of orion's own (`<data dir>/fresh/config.json`,
//! [`FRESH_CONFIG`]), so it never touches the user's own fresh config, and
//! that config never changes how the modal behaves: the one file opened, no
//! menu, tab bar, file explorer or workspace dock, a two-item status bar, and
//! orion's own theme. The file is made on first use; after that it is the
//! user's to edit, and orion only ever adds a setting it lacks.
//!
//! In fresh a Mac's text-editing chords mean what they mean in VS Code on a
//! Mac — `⌘←` the line's start, `⌥→` the next word's end, `⌘L` the line,
//! `⌘⇧L` every match ([`Kind::mac_key`]): each is turned into the key fresh
//! binds the same action to.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};

/// The editor every file opens in, unless `ORION_EDITOR` names another.
pub const DEFAULT_EDITOR: &str = "fresh";

/// The settings in orion's own fresh config, merged into the file: a key
/// the user set — at any depth — stays as they set it. The one file asked
/// for and nothing around it: no menu bar, tab bar, scrollbar, `~` past
/// the end, whitespace dots or edge fade; no workspace dock (the
/// `orchestrator` plugin), no file explorer, no session restored around
/// the file; no update checks. fresh's animations, the cursor's jump among
/// them, stay on. The status bar keeps the cursor, the cursor count and
/// fresh's messages on the left and the language on the right. The theme
/// is orion's own ([`FRESH_THEME`]): Cursor's default Cursor Dark — its
/// background, its colours for keywords, strings, functions, types and
/// comments, VS Code's gold/orchid/blue bracket pairs — with VS Code's
/// selection blue and a current line barely lifted off the background, so
/// what is selected inside the cursor's line stands out. The `default`
/// keymap, not the `macos` one fresh picks on a Mac (it moves `Ctrl+A` to
/// the line start), is the one [`Kind::mac_key`]'s keys are read in.
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

/// How many `^D`s — fresh's "add the next match as a cursor" — `⌘⇧L` types
/// at once to put a cursor on every match. It stops at the last one ("All
/// matches are already selected"), so the rest are no-ops. Sent as the bare 0x04 byte whatever keyboard protocol
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

/// The editors orion knows how to drive, by their program's file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fresh,
    /// Anything else `ORION_EDITOR` names, launched as
    /// `<command> +<line> <file>`.
    Other,
}

impl Kind {
    pub fn of(editor: &str) -> Self {
        match program_name(editor) {
            "fresh" => Kind::Fresh,
            _ => Kind::Other,
        }
    }

    /// Ctrl+Q is the editor's own quit, which asks to save first — so the
    /// modal hands it over rather than force-closing on it.
    pub fn quits_on_ctrl_q(self) -> bool {
        self == Kind::Fresh
    }

    /// What the editor's bottom row starts with while its find prompt is
    /// up: fresh's `Search: `, in the English it ships with (under another
    /// locale the prompt is never seen, and Enter stays the editor's own).
    /// None for an editor whose Enter after a find the modal leaves alone.
    pub fn find_prompt(self) -> Option<&'static str> {
        match self {
            Kind::Fresh => Some("Search:"),
            _ => None,
        }
    }

    /// The cursor's line, if `row` is the editor's status bar: fresh's
    /// starts with its `{cursor}` item, `Ln 42, Col 7` in the English it
    /// ships with (orion's own config puts it first, [`FRESH_CONFIG`]).
    /// None for any other row, and for an editor whose status bar orion
    /// can't read.
    pub fn cursor_line(self, row: &str) -> Option<u64> {
        if self != Kind::Fresh {
            return None;
        }
        let rest = row.trim_start().strip_prefix("Ln ")?;
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if !rest[digits..].starts_with(", Col ") {
            return None;
        }
        rest[..digits].parse().ok().filter(|&line| line > 0)
    }

    /// The key the editor binds its next match to — `back`, its previous
    /// one: fresh's `F3` and `⇧F3`. What Enter and `⇧Enter` are typed as
    /// straight after a find, where fresh's own Enter is a new line at the
    /// first match.
    pub fn find_step(self, back: bool) -> Option<KeyEvent> {
        let mods = if back {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        match self {
            Kind::Fresh => Some(KeyEvent::new(KeyCode::F(3), mods)),
            _ => None,
        }
    }

    /// `key` as a Mac's text-editing chord, in the key fresh binds the
    /// same action to — None for any other editor and any other key (it
    /// goes on as it came, a ⌘ letter as its Ctrl chord):
    ///
    /// | Chord | Means | fresh |
    /// |---|---|---|
    /// | `⌘←` `⌘→` (Ghostty's `^A` `^E`) | line start / end | `Home` `End` |
    /// | `⇧⌘←` `⇧⌘→` | select to it | `⇧Home` `⇧End` |
    /// | `⌘↑` `⌘↓` | file start / end | `^Home` `^End` |
    /// | `⇧⌘↑` `⇧⌘↓` | select to it | `^⇧Home` `^⇧End` |
    /// | `⌥←` `⌥→` (Ghostty's `⎋b` `⎋f`) | word | `^←` `^→` |
    /// | `⇧⌥←` `⇧⌥→` | select word | `^⇧←` `^⇧→` |
    /// | `⌥⌘↑` `⌥⌘↓` | cursor above / below | `^⌥↑` `^⌥↓` |
    /// | `⌘L` | select the line | `^L` |
    /// | `⌘/` | toggle comment | `^/` |
    /// | `⌘⇧L` | a cursor on every match | [`SELECT_ALL_MATCHES`] |
    /// | `⌘⇧P` | command palette | `^P` |
    ///
    /// Ghostty keeps `⌘←`/`⌘→` and `⌥←`/`⌥→` for itself and types the
    /// shell's keys for them, so those arrive as `^A`/`^E` and
    /// `Alt+B`/`Alt+F` — and a bare `^A`/`^E` here is the line's start and
    /// end, as in any Mac text field (`⌘A` stays select all).
    pub fn mac_key(self, key: &KeyEvent) -> Option<MacKey> {
        if self != Kind::Fresh {
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
        let word = |code: KeyCode, mods: KeyModifiers| to(code, KeyModifiers::CONTROL | mods);
        match key.code {
            KeyCode::Char(c) => match (c.to_ascii_lowercase(), cmd, alt, ctrl, shift) {
                ('a', false, false, true, false) => to(KeyCode::Home, KeyModifiers::NONE),
                ('e', false, false, true, false) => to(KeyCode::End, KeyModifiers::NONE),
                (c @ ('b' | 'f'), false, true, false, false) => word(left(c), KeyModifiers::NONE),
                ('l', true, false, false, false) => to(KeyCode::Char('l'), KeyModifiers::CONTROL),
                ('l', true, false, false, true) => Some(MacKey::Bytes(&SELECT_ALL_MATCHES)),
                ('p', true, false, false, true) => to(KeyCode::Char('p'), KeyModifiers::CONTROL),
                ('/', true, false, false, false) => to(KeyCode::Char('/'), KeyModifiers::CONTROL),
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
                    (true, true) if !shift => {
                        to(key.code, KeyModifiers::CONTROL | KeyModifiers::ALT)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// The program's file name: `/opt/homebrew/bin/fresh --log x` is `fresh`.
pub fn program_name(editor: &str) -> &str {
    let program = editor.split_whitespace().next().unwrap_or("");
    program.rsplit('/').next().unwrap_or(program)
}

/// What the BUILT-IN EDITOR runs: `ORION_EDITOR` when set, else fresh.
pub fn command() -> String {
    #[cfg(test)]
    if let Some(command) = TEST_COMMAND.with(|c| c.borrow().clone()) {
        return command;
    }
    orion_core::env::non_empty(orion_core::env::EDITOR)
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| DEFAULT_EDITOR.to_string())
}

#[cfg(test)]
thread_local! {
    static TEST_COMMAND: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with [`command`] answering `editor`, as `ORION_EDITOR` would —
/// a test's shell standing in for fresh — on this thread alone.
#[cfg(test)]
pub fn with_command<T>(editor: &str, f: impl FnOnce() -> T) -> T {
    let before = TEST_COMMAND.with(|c| c.replace(Some(editor.to_string())));
    let out = f();
    TEST_COMMAND.with(|c| *c.borrow_mut() = before);
    out
}

/// Where orion keeps fresh's own config: `<data dir>`, fresh's file under
/// it ([`fresh_config`]). Unit tests that open an editor get one under the
/// temp dir instead of the machine's data dir.
pub fn config_root() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join("orion-unit-tests-editors");
    }
    orion_core::paths::data_dir()
}

/// orion's own fresh config file under `root`.
pub fn fresh_config(root: &Path) -> PathBuf {
    root.join("fresh").join("config.json")
}

/// Make the config `editor` runs off under `root` — fresh's file and
/// theme — before it starts. Any other editor has none.
pub fn ensure_config(editor: &str, root: &Path) -> std::io::Result<()> {
    match Kind::of(editor) {
        Kind::Fresh => {
            if let Some(home) = orion_core::env::home_dir() {
                ensure_fresh_theme(&home)?;
            }
            ensure_fresh_config(&fresh_config(root))
        }
        Kind::Other => Ok(()),
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

/// The argv after the program for opening `file` at `line`:
///
/// - fresh: `--config <orion's> --no-upgrade-check --no-restore` and then
///   `<file>:<line>` — the bare file on line 1, so a name with a colon in
///   it is never read as a line
/// - anything else: `+<line> <file>`
///
/// `root` is [`config_root`], where fresh's file lives.
pub fn editor_args(editor: &str, root: &Path, file: &str, line: u64) -> Vec<String> {
    let at = format!("+{line}");
    let file_at = if line > 1 {
        format!("{file}:{line}")
    } else {
        file.to_string()
    };
    match Kind::of(editor) {
        Kind::Fresh => vec![
            "--config".into(),
            fresh_config(root).to_string_lossy().into_owned(),
            "--no-upgrade-check".into(),
            "--no-restore".into(),
            file_at,
        ],
        Kind::Other => vec![at, file.to_string()],
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
        assert_ne!(
            theme["editor"]["selection_bg"],
            theme["editor"]["current_line_bg"]
        );
        let config: serde_json::Value = serde_json::from_str(FRESH_CONFIG).unwrap();
        assert_eq!(config["theme"], "orion");
        assert_eq!(config["editor"]["cursor_jump_animation"], true);
    }

    #[test]
    fn fresh_is_known_by_its_program_name() {
        assert_eq!(Kind::of("fresh"), Kind::Fresh);
        assert_eq!(Kind::of("/opt/homebrew/bin/fresh --log x"), Kind::Fresh);
        assert_eq!(Kind::of("freshen"), Kind::Other);
        assert_eq!(Kind::of("vim"), Kind::Other, "no special handling");
        assert_eq!(Kind::of("code --wait"), Kind::Other);
        assert_eq!(Kind::of(""), Kind::Other);
    }

    /// fresh gets the line as `file:N` off orion's config file; any other
    /// command `ORION_EDITOR` names, `+N file`.
    #[test]
    fn each_editor_is_told_the_line_its_own_way() {
        let dir = Path::new("/data");
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
        assert_eq!(
            editor_args("fresh", dir, "notes:2.txt", 1).last().unwrap(),
            "notes:2.txt",
            "line 1 is the bare file: no colon to misread"
        );
        assert_eq!(editor_args("vim", dir, "src/a.rs", 3), ["+3", "src/a.rs"]);
        assert_eq!(editor_args("/bin/sh", dir, "a.rs", 1), ["+1", "a.rs"]);
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
        assert!(ensure_config("vim", home.path()).is_ok(), "nothing to make");
    }

    /// The Mac chords per editor, as [`Kind::mac_key`]'s table has them.
    #[test]
    fn mac_chords_become_each_editors_own_keys() {
        use KeyModifiers as M;
        let k = |code, mods| KeyEvent::new(code, mods);
        let key = |code, mods| Some(MacKey::Key(k(code, mods)));
        let (cmd, alt, shift, ctrl) = (M::SUPER, M::ALT, M::SHIFT, M::CONTROL);
        {
            let t = |code, mods| Kind::Fresh.mac_key(&k(code, mods));
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

        assert_eq!(Kind::Other.mac_key(&k(KeyCode::Left, cmd)), None);
        assert_eq!(Kind::Other.mac_key(&k(KeyCode::Char('a'), ctrl)), None);
        assert!(Kind::Fresh.quits_on_ctrl_q() && !Kind::Other.quits_on_ctrl_q());
    }

    /// fresh's status bar as it draws it (`Ln 42, Col 1`, then its message
    /// and the language) gives the cursor's line; any other row, or any
    /// other editor, none.
    #[test]
    fn the_cursor_line_is_read_off_freshs_status_bar() {
        let bar = " Ln 42, Col 1       Opened /tmp/sample.txt                 Text";
        assert_eq!(Kind::Fresh.cursor_line(bar), Some(42));
        assert_eq!(Kind::Fresh.cursor_line("Ln 7, Col 12"), Some(7));
        assert_eq!(Kind::Fresh.cursor_line("Search:"), None);
        assert_eq!(Kind::Fresh.cursor_line("  42 │ line number 42"), None);
        assert_eq!(Kind::Fresh.cursor_line(" Ln x, Col 1"), None);
        assert_eq!(Kind::Fresh.cursor_line(" Ln 4 Col 1"), None);
        assert_eq!(Kind::Other.cursor_line(bar), None);
    }

    /// `⌘⇧L` — however the terminal spells the shifted letter — is a burst
    /// of `^D` bytes, each the editor's next-match cursor, in one write
    /// small enough for the tty's input queue.
    #[test]
    fn cmd_shift_l_is_a_burst_of_next_match_cursors() {
        let sup_shift = KeyModifiers::SUPER | KeyModifiers::SHIFT;
        for code in [KeyCode::Char('l'), KeyCode::Char('L')] {
            {
                let Some(MacKey::Bytes(bytes)) =
                    Kind::Fresh.mac_key(&KeyEvent::new(code, sup_shift))
                else {
                    panic!("no burst");
                };
                assert!(bytes.iter().all(|&b| b == 0x04));
                assert_eq!(bytes.len(), 500);
                assert!(bytes.len() < 1024, "inside the tty's input queue");
            }
        }
    }
}
