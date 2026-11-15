//! The LINK SHIM: what a session's CLI opens a link with. Claude Code opens
//! a newly published Artifact, an OAuth sign-in and the like by running
//! `$BROWSER` with the URL as its one argument — a program, never a shell
//! line, so `BROWSER` can't be `orion open-link` itself. The daemon writes
//! this two-line script instead, at [`paths::link_shim_path`], and points
//! every agent PTY's `BROWSER` at it (`Daemon::session_env`). It runs the
//! orion this daemon was started from as `orion open-link <url>`, which
//! opens the link in the session's account's **Open links in**, as a click
//! in its pane does.

use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use orion_core::paths;

/// The shim's text for the orion binary at `orion`: `exec` hands the
/// process over, so the CLI hears `open-link`'s own exit status. The path
/// is double-quoted, with what a shell reads inside double quotes escaped.
pub fn content(orion: &Path) -> String {
    let mut quoted = String::new();
    for c in orion.to_string_lossy().chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    format!("#!/bin/sh\nexec \"{quoted}\" open-link \"$@\"\n")
}

/// Write the shim for `orion` at `at`, mode 0755 — only when what is there
/// differs, so a daemon start that changes nothing touches nothing. True
/// when it was (re)written. The new text goes in beside it and is renamed
/// over, so a session running the shim at that moment reads the old one
/// or the new, never half.
pub fn write(at: &Path, orion: &Path) -> io::Result<bool> {
    let want = content(orion);
    let current = std::fs::read_to_string(at).ok();
    let executable = std::fs::metadata(at).is_ok_and(|m| m.permissions().mode() & 0o777 == 0o755);
    if current.as_deref() == Some(want.as_str()) && executable {
        return Ok(false);
    }
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = at.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, want)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, at).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(true)
}

/// At daemon start: write the shim for the orion this daemon runs as —
/// the binary the TUI spawned it from (`current_exe`). The path sessions'
/// `BROWSER` gets, or None when it couldn't be written: then they keep
/// whatever `BROWSER` they had, and their links the default browser.
pub fn install() -> Option<PathBuf> {
    let orion = match std::env::current_exe() {
        Ok(orion) => orion,
        Err(err) => {
            tracing::warn!(%err, "link shim not written: orion's own binary not found");
            return None;
        }
    };
    let at = paths::link_shim_path();
    match write(&at, &orion) {
        Ok(rewritten) => {
            if rewritten {
                tracing::info!(shim = %at.display(), orion = %orion.display(), "link shim written");
            }
            Some(at)
        }
        Err(err) => {
            tracing::warn!(%err, shim = %at.display(), "link shim not written");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shim_execs_orion_open_link_with_the_url() {
        assert_eq!(
            content(Path::new("/opt/orion/bin/orion")),
            "#!/bin/sh\nexec \"/opt/orion/bin/orion\" open-link \"$@\"\n"
        );
        // A path a shell would read into inside double quotes stays whole.
        assert_eq!(
            content(Path::new("/Users/me/My \"$odd\" `dir`/orion")),
            "#!/bin/sh\nexec \"/Users/me/My \\\"\\$odd\\\" \\`dir\\`/orion\" open-link \"$@\"\n"
        );
    }

    /// The shim really runs: `sh` hands the URL, untouched, to what it
    /// names — here a stand-in that prints its arguments.
    #[test]
    fn the_shim_passes_the_url_through() {
        let dir = tempfile::tempdir().unwrap();
        let orion = dir.path().join("fake orion");
        std::fs::write(&orion, "#!/bin/sh\nprintf '%s|' \"$@\"\n").unwrap();
        std::fs::set_permissions(&orion, std::fs::Permissions::from_mode(0o755)).unwrap();
        let shim = dir.path().join("bin/orion-open-link");
        write(&shim, &orion).unwrap();
        let out = std::process::Command::new(&shim)
            .arg("https://claude.ai/a?b=c&d")
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "open-link|https://claude.ai/a?b=c&d|"
        );
    }

    #[test]
    fn the_shim_is_rewritten_only_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("bin/orion-open-link");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        assert!(write(&shim, Path::new("/a/orion")).unwrap(), "created");
        assert_eq!(mode(&shim), 0o755);
        assert_eq!(
            std::fs::read_to_string(&shim).unwrap(),
            content(Path::new("/a/orion"))
        );
        let inode = |p: &Path| std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(p).unwrap());
        let before = inode(&shim);
        assert!(!write(&shim, Path::new("/a/orion")).unwrap(), "unchanged");
        assert_eq!(inode(&shim), before, "left alone");

        assert!(
            write(&shim, Path::new("/b/orion")).unwrap(),
            "another orion"
        );
        assert_eq!(
            std::fs::read_to_string(&shim).unwrap(),
            content(Path::new("/b/orion"))
        );
        // The right text but not executable is put right too.
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(write(&shim, Path::new("/b/orion")).unwrap());
        assert_eq!(mode(&shim), 0o755);
        let left: Vec<_> = std::fs::read_dir(shim.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["orion-open-link"], "no temp file left behind");
    }
}
