//! The editors a file opens in: the BUILT-IN EDITOR orion runs in its
//! editor modal (micro by default, any `editor` setting otherwise), and
//! Cursor, which ⌘O hands a file to outside orion.
//!
//! micro runs off a config dir of orion's own (`<data dir>/micro`), so its
//! bindings here — `Ctrl+D` adds the next match as another cursor, as in
//! Cursor — never touch the user's `~/.config/micro`, and that config never
//! changes how the modal behaves. The dir is made on first use with a
//! `bindings.json`; after that the file is the user's to edit.

use std::path::{Path, PathBuf};

/// The editor every fresh config names.
pub const DEFAULT_EDITOR: &str = "micro";

/// Where the default goes when micro isn't installed.
pub const FALLBACK_EDITOR: &str = "vim";

/// The `bindings.json` written into orion's micro config dir on first use.
pub const MICRO_BINDINGS: &str = "{\n    \"Ctrl-d\": \"SpawnMultiCursor\"\n}\n";

/// Whether `editor` is micro, by its program's file name.
pub fn is_micro(editor: &str) -> bool {
    program_name(editor) == "micro"
}

fn program_name(editor: &str) -> &str {
    let program = editor.split_whitespace().next().unwrap_or("");
    program.rsplit('/').next().unwrap_or(program)
}

/// orion's own micro config dir. Unit tests that open the default editor
/// get one under the temp dir instead of the machine's data dir.
pub fn micro_config_dir() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join("orion-unit-tests-micro");
    }
    orion_core::paths::data_dir().join("micro")
}

/// Make `dir` a micro config dir: created when missing, with
/// [`MICRO_BINDINGS`] as its `bindings.json` unless one is already there.
pub fn ensure_micro_config(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let bindings = dir.join("bindings.json");
    if !bindings.exists() {
        std::fs::write(bindings, MICRO_BINDINGS)?;
    }
    Ok(())
}

/// The argv after the program for opening `file` at `line`: `+<line>
/// <file>`, which every other editor in the picker takes (nano only in
/// that order), or micro's own `-config-dir <dir> <file> +<line>`.
pub fn editor_args(editor: &str, micro_dir: &Path, file: &str, line: u64) -> Vec<String> {
    let at = format!("+{line}");
    if is_micro(editor) {
        return vec![
            "-config-dir".into(),
            micro_dir.to_string_lossy().into_owned(),
            file.to_string(),
            at,
        ];
    }
    vec![at, file.to_string()]
}

/// The Cursor CLI: `cursor` on PATH (Cursor's "Install 'cursor' command"),
/// else the one inside Cursor.app, which the app ships whether or not that
/// was ever run.
pub fn cursor_cli() -> Option<PathBuf> {
    if crate::config::program_installed("cursor") {
        return Some(PathBuf::from("cursor"));
    }
    let mut roots = vec![PathBuf::from("/")];
    roots.extend(orion_core::env::home_dir());
    cursor_cli_in(&roots)
}

fn cursor_cli_in(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .map(|root| root.join("Applications/Cursor.app/Contents/Resources/app/bin/cursor"))
        .find(|cli| cli.is_file())
}

/// `cursor <root> --goto <file>:<line>`'s argv after the program: the
/// checkout's window (reused when it is already open) with the file at
/// that line. `file` relative to `root` is made absolute.
pub fn cursor_args(root: &Path, file: &str, line: u64) -> Vec<String> {
    let full = root.join(file);
    vec![
        root.to_string_lossy().into_owned(),
        "--goto".into(),
        format!("{}:{line}", full.display()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micro_is_known_by_its_program_name() {
        assert!(is_micro("micro"));
        assert!(is_micro("/opt/homebrew/bin/micro"));
        assert!(is_micro("micro -autosu true"));
        assert!(!is_micro("vim"));
        assert!(!is_micro("micromamba"));
        assert!(!is_micro(""));
    }

    #[test]
    fn micro_gets_orions_config_dir_and_others_do_not() {
        let dir = Path::new("/data/micro");
        assert_eq!(
            editor_args("micro", dir, "src/a.rs", 12),
            ["-config-dir", "/data/micro", "src/a.rs", "+12"]
        );
        assert_eq!(editor_args("vim", dir, "src/a.rs", 3), ["+3", "src/a.rs"]);
    }

    #[test]
    fn the_micro_config_is_written_once_and_then_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("micro");
        ensure_micro_config(&dir).unwrap();
        let bindings = dir.join("bindings.json");
        assert_eq!(std::fs::read_to_string(&bindings).unwrap(), MICRO_BINDINGS);
        let parsed: serde_json::Value = serde_json::from_str(MICRO_BINDINGS).unwrap();
        assert_eq!(parsed["Ctrl-d"], "SpawnMultiCursor");

        std::fs::write(&bindings, "{}").unwrap();
        ensure_micro_config(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&bindings).unwrap(), "{}", "the user's edit stays");
    }

    #[test]
    fn cursor_opens_the_checkout_at_the_files_line() {
        assert_eq!(
            cursor_args(Path::new("/repo"), "src/a.rs", 7),
            ["/repo", "--goto", "/repo/src/a.rs:7"]
        );
        assert_eq!(
            cursor_args(Path::new("/repo"), "/abs/b.md", 1),
            ["/repo", "--goto", "/abs/b.md:1"]
        );
    }

    #[test]
    fn cursor_cli_is_found_inside_either_applications_folder() {
        let system = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = [system.path().to_path_buf(), home.path().to_path_buf()];
        assert_eq!(cursor_cli_in(&roots), None);
        let bin = home.path().join("Applications/Cursor.app/Contents/Resources/app/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("cursor"), "").unwrap();
        assert_eq!(cursor_cli_in(&roots), Some(bin.join("cursor")));
    }
}
