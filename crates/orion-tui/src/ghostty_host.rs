//! GHOSTTY HOST: moving orion itself into Ghostty. Terminal.app never sends
//! ⌘ to the program inside it, so there every ⌘ chord orion answers to
//! reaches it only as its `^` twin. Setup's first step offers to reopen
//! orion in Ghostty while the terminal it runs in can't send ⌘
//! ([`offered`]), and `install.sh` opens a first install there through
//! `orion _open-in-ghostty`. Nothing is lost by the move: the sessions live
//! in the daemon, and the new window attaches to it.
//!
//! The new window runs orion through `/bin/sh` with this one's PATH and
//! orion variables ([`KEPT_VARS`]), so its agents find the CLIs they find
//! here, and leaves the window to the user's login shell when orion quits
//! ([`open_args`]). Orion's keybind block goes into Ghostty's config first,
//! so the Ghostty about to start reads it at launch and needs no reload.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The variables the new window's orion keeps from this one besides PATH
/// ([`path_with`]): the ones that point orion at another daemon, config or
/// editor.
const KEPT_VARS: &[&str] = &[
    orion_core::env::RUNTIME_DIR,
    orion_core::env::DATA_DIR,
    orion_core::env::CONFIG_FILE,
    orion_core::env::EDITOR,
    orion_core::env::GHOSTTY_CONFIG,
    orion_core::env::LOG,
];

/// What the new window's `/bin/sh` runs: orion, through `env` with the
/// kept variables (`"$@"`), then the login shell named as `$0` in its place.
const RUN_THEN_SHELL: &str = r#"/usr/bin/env "$@"; exec "$0" -l"#;

/// Whether to offer moving into Ghostty: on a local Mac, in a terminal that
/// took no kitty keyboard protocol — so sends no ⌘ — and is neither
/// Ghostty, tmux (which never passes ⌘ on) nor `orion browser`'s page.
#[cfg(not(test))]
pub fn offered() -> bool {
    cfg!(target_os = "macos")
        && !orion_core::host::is_remote_session()
        && !crate::event_loop::host_sends_cmd()
        && !crate::ghostty_config::inside_ghostty()
        && std::env::var_os("TMUX").is_none()
        && std::env::var_os(orion_core::env::BROWSER).is_none()
}

/// [`offered`] under test: what [`with_offered`] set on this thread.
#[cfg(test)]
pub fn offered() -> bool {
    OFFERED.with(std::cell::Cell::get)
}

/// The `open` arguments that start a new Ghostty (`bundle`) in `dir`,
/// running orion at `exe` with `vars`, the window then left to `shell`.
pub fn open_args(
    bundle: &Path,
    exe: &Path,
    dir: &Path,
    shell: &OsStr,
    vars: &[(&str, OsString)],
) -> Vec<OsString> {
    let mut working_dir = OsString::from("--working-directory=");
    working_dir.push(dir);
    let mut args: Vec<OsString> = vec![
        "-na".into(),
        bundle.into(),
        "--args".into(),
        working_dir,
        "-e".into(),
        "/bin/sh".into(),
        "-c".into(),
        RUN_THEN_SHELL.into(),
        shell.into(),
    ];
    for (name, value) in vars {
        let mut pair = OsString::from(format!("{name}="));
        pair.push(value);
        args.push(pair);
    }
    args.push(exe.into());
    args
}

/// PATH as set here, with `bin` — the folder orion runs from — on the
/// front when it isn't on it, so the `orion` an agent's hook calls is found.
fn path_with(bin: Option<&Path>) -> Option<OsString> {
    let here = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&here).collect();
    if let Some(bin) = bin.filter(|bin| !dirs.iter().any(|d| d == bin)) {
        dirs.insert(0, bin.to_path_buf());
    }
    std::env::join_paths(dirs).ok()
}

/// PATH ([`path_with`]) and [`KEPT_VARS`], as set here.
fn kept_vars(bin: Option<&Path>) -> Vec<(&'static str, OsString)> {
    let path = path_with(bin).map(|v| ("PATH", v));
    let rest = KEPT_VARS.iter().filter_map(|name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(|v| (*name, v))
    });
    path.into_iter().chain(rest).collect()
}

/// Open orion in a new Ghostty window, in the folder this one runs in, its
/// keybind block written first. What went wrong, when something did.
pub fn open_orion() -> Result<(), String> {
    let bundle = crate::event_loop::ghostty_app().ok_or("Ghostty isn't installed")?;
    let exe = std::env::current_exe().map_err(|e| format!("couldn't find orion's binary: {e}"))?;
    let dir = std::env::current_dir()
        .ok()
        .or_else(orion_core::env::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"));
    let shell = OsString::from(orion_core::shell::user_shell());
    let vars = kept_vars(exe.parent());
    if let Some(flash) = crate::ghostty_config::ensure_before_launch(&crate::config::Config::load())
    {
        tracing::info!("ghostty keybinds before the move: {}", flash.text);
    }
    let status = std::process::Command::new("open")
        .args(open_args(&bundle, &exe, &dir, &shell, &vars))
        .status()
        .map_err(|e| format!("couldn't run open: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open {} failed ({status})", bundle.display()))
    }
}

#[cfg(test)]
thread_local! {
    static OFFERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with [`offered`] answering `on` on this test's thread — a
/// terminal that sends no ⌘, or one that does.
#[cfg(test)]
pub fn with_offered<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let was = OFFERED.with(|o| o.replace(on));
    let out = f();
    OFFERED.with(|o| o.set(was));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The new window: Ghostty opened fresh in the folder, `/bin/sh` running
    /// orion through `env` with the kept variables, then the login shell.
    #[test]
    fn the_new_window_runs_orion_then_the_login_shell() {
        let args = open_args(
            Path::new("/Applications/Ghostty.app"),
            Path::new("/Users/me/.local/bin/orion"),
            Path::new("/Users/me/code/app"),
            OsStr::new("/bin/zsh"),
            &[("PATH", "/Users/me/.local/bin:/usr/bin".into())],
        );
        assert_eq!(
            args,
            [
                "-na",
                "/Applications/Ghostty.app",
                "--args",
                "--working-directory=/Users/me/code/app",
                "-e",
                "/bin/sh",
                "-c",
                r#"/usr/bin/env "$@"; exec "$0" -l"#,
                "/bin/zsh",
                "PATH=/Users/me/.local/bin:/usr/bin",
                "/Users/me/.local/bin/orion",
            ]
            .map(OsString::from)
        );
    }

    /// orion's own folder goes on the front of PATH when it isn't on it, and
    /// is not added twice when it is.
    #[test]
    fn orions_folder_is_put_on_path_once() {
        let here = std::env::var_os("PATH").unwrap_or_default();
        let first = std::env::split_paths(&here).next().unwrap();
        assert_eq!(path_with(Some(&first)).unwrap(), here);
        let added = path_with(Some(Path::new("/nowhere/bin"))).unwrap();
        assert_eq!(
            std::env::split_paths(&added).next().unwrap(),
            Path::new("/nowhere/bin")
        );
    }
}
