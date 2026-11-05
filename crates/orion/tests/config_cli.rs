//! `orion config` against throwaway data dirs.

use orion_core::env::{CONFIG_FILE, DATA_DIR};
use std::path::Path;
use std::process::{Command, Output};

fn orion(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_orion"))
        .args(args)
        .env(DATA_DIR, data)
        .env_remove(CONFIG_FILE)
        .output()
        .expect("failed to run orion")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn path_names_every_settings_file() {
    let data = tempfile::tempdir().unwrap();
    let out = orion(data.path(), &["config", "path"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    for file in [
        "config.json",
        "config.local.json",
        "agent_presets.json",
        "ssh_hosts.json",
    ] {
        let path = data.path().join(file).display().to_string();
        assert!(stdout.contains(&path), "{file} missing from:\n{stdout}");
    }
}

/// The settings backup is gone — `export` and `import` are no commands —
/// and so is `harnesses`: what a harness runs is orion's, and what is the
/// user's is in config.json to read.
#[test]
fn there_is_no_settings_export_import_or_harnesses() {
    let data = tempfile::tempdir().unwrap();
    for command in ["export", "import", "harnesses"] {
        let out = orion(data.path(), &["config", command]);
        assert!(!out.status.success(), "`orion config {command}` ran");
        assert!(
            text(&out.stderr).contains("unrecognized subcommand"),
            "{}",
            text(&out.stderr)
        );
    }
}
