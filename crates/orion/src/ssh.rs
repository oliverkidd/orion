//! `orion ssh HOST [PATH]`: open orion on a remote machine.
//!
//! Execs `ssh -t HOST <cmd>` so the remote TUI renders in this terminal. The
//! remote command installs orion via the published install script when it
//! isn't on the remote PATH, then launches it.
//!
//! Quoting: sshd hands the command string to the user's login shell, which
//! may be bash, zsh, or fish. The script below is a fixed constant with no
//! single quotes, backslashes, or newlines, wrapped once in '...'; user input
//! (install URL, start dir) is passed only as positional parameters, each
//! POSIX-single-quoted. csh/tcsh login shells are the one unsupported case.

use anyhow::{bail, Context, Result};
use orion_core::shell::single_quote;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Runs under `sh -c` on the remote: $1 = install URL, $2 = start dir
/// (optional; defaults to the remote $HOME). It first leaves a usable
/// `orion` on the remote PATH, installing it when there is none.
const REMOTE_SCRIPT: &str = concat!(
    // sshd hands a remote command a bare PATH — no login shell runs, so
    // nothing the user configured applies. Prepend install.sh's default
    // ORION_INSTALL_DIR, and append both Homebrew prefixes: on a macOS
    // remote that is the only place a brew-installed orion lives.
    "export PATH=\"$HOME/.local/bin:$PATH:/opt/homebrew/bin:/usr/local/bin\"; ",
    "if ! command -v orion >/dev/null 2>&1; then ",
    "command -v curl >/dev/null 2>&1 || { ",
    "echo \"orion: curl is required on the remote to install orion\" >&2; exit 127; }; ",
    "echo \"orion not found on remote; installing...\" >&2; ",
    "curl -fsSL \"$1\" | sh || exit 1; ",
    "fi; ",
    "cd -- \"${2:-$HOME}\" || exit 1; ",
    "exec orion"
);

pub fn run_ssh(host: &str, path: Option<&str>) -> Result<()> {
    // Remember the destination for the TUI's `Shift+H` picker. Before the exec on
    // purpose (there is no after); a host that fails to connect still lists,
    // and `d` can drop it.
    orion_tui::hosts::record(host, path);
    let cmd = remote_command(&crate::upgrade::install_url(), path);
    // exec: ssh owns the tty from here and its exit status propagates
    // natively. Only returns on failure.
    let err = Command::new("ssh").args(["-t", "--", host, &cmd]).exec();
    if err.kind() == std::io::ErrorKind::NotFound {
        bail!("ssh not found on PATH — orion ssh requires the OpenSSH client");
    }
    Err(err).context("failed to exec ssh")
}

fn remote_command(install_url: &str, path: Option<&str>) -> String {
    let mut cmd = format!(
        "sh -c '{}' orion-ssh {}",
        REMOTE_SCRIPT,
        single_quote(install_url)
    );
    if let Some(path) = path {
        cmd.push(' ');
        cmd.push_str(&single_quote(path));
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://example.com/install.sh";

    #[test]
    fn script_survives_single_quoting() {
        // The whole scheme rests on the script needing no escaping inside
        // '...' under any login shell.
        assert!(!REMOTE_SCRIPT.contains('\''));
        assert!(!REMOTE_SCRIPT.contains('\\'));
        assert!(!REMOTE_SCRIPT.contains('\n'));
    }

    #[test]
    fn no_path_defaults_to_remote_home() {
        let cmd = remote_command(URL, None);
        assert!(cmd.ends_with("orion-ssh 'https://example.com/install.sh'"));
        assert!(cmd.contains("${2:-$HOME}"));
    }

    #[test]
    fn path_is_quoted() {
        let cmd = remote_command(URL, Some("/srv/my repo"));
        assert!(cmd.ends_with("'/srv/my repo'"));
    }

    #[test]
    fn path_with_single_quote_is_escaped() {
        let cmd = remote_command(URL, Some("/tmp/it's here"));
        assert!(cmd.ends_with("'/tmp/it'\\''s here'"));
    }
}
