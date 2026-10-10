mod browser;
mod cli;
mod ssh;
mod tunnel;
mod upgrade;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, ConfigCommand};
use std::path::Path;

fn main() -> Result<()> {
    let cli = Cli::parse();
    // A `orion ssh` / `orion tunnel` from another machine may have sent its
    // settings along. Merge them before anything reads a setting, and before
    // a thread or a child exists to inherit the variable.
    orion_tui::bundle::apply_forwarded();
    match cli.command {
        Some(Command::Daemon { foreground }) => {
            init_daemon_logging(foreground)?;
            log_fatal(
                orion_daemon::run_daemon(),
                &orion_core::paths::daemon_log_path(),
            )
        }
        Some(Command::Add { path }) => orion_tui::run_add_project(path),
        Some(Command::Config { command }) => orion_tui::run_config(match command {
            ConfigCommand::Path => orion_tui::ConfigOp::Path,
            ConfigCommand::Export { path } => orion_tui::ConfigOp::Export { path },
            ConfigCommand::Import { source } => orion_tui::ConfigOp::Import { source },
            ConfigCommand::Harnesses => orion_tui::ConfigOp::Harnesses,
        }),
        Some(Command::Kill) => orion_tui::run_kill(),
        Some(Command::Rename { title, force }) => {
            let mode = if force {
                orion_tui::RenameMode::Force
            } else {
                orion_tui::RenameMode::Auto
            };
            orion_tui::run_rename(title.join(" "), mode)
        }
        Some(Command::Worktree { name, base }) => orion_tui::run_worktree(name.join(" "), base),
        Some(Command::Spawn { task, kind }) => orion_tui::run_spawn(task.join(" "), kind),
        Some(Command::Open { files }) => orion_tui::run_open(files),
        Some(Command::Browser {
            port,
            bind,
            public,
            credential,
            no_open,
        }) => browser::run_browser(browser::BrowserOpts {
            port,
            // --public is --bind 0.0.0.0 with a name; clap keeps the two
            // from being given at once.
            bind: bind.unwrap_or(if public {
                browser::PUBLIC_BIND
            } else {
                browser::DEFAULT_BIND
            }),
            credential,
            open: !no_open,
        }),
        Some(Command::Ssh {
            host,
            path,
            no_sync_config,
        }) => ssh::run_ssh(&host, path.as_deref(), !no_sync_config),
        Some(Command::Tunnel {
            host,
            path,
            port,
            remote_port,
            no_sync_config,
        }) => tunnel::run_tunnel(tunnel::TunnelOpts {
            host,
            path,
            port,
            remote_port,
            sync_config: !no_sync_config,
        }),
        Some(Command::Setup) => {
            orion_tui::reset_setup();
            run_tui()
        }
        Some(Command::Doctor { json }) => {
            if !orion_tui::run_doctor(json) {
                std::process::exit(1);
            }
            Ok(())
        }
        Some(Command::Upgrade { force }) => upgrade::run_upgrade(force),
        Some(Command::StaleDaemonNote) => {
            if orion_daemon::lifecycle::daemon_is_stale() {
                println!("note: the running daemon was built from older code.");
                println!("{}", upgrade::KILL_HINT);
            }
            Ok(())
        }
        Some(Command::ProtocolVersion) => {
            println!("{}", orion_core::PROTOCOL_VERSION);
            Ok(())
        }
        Some(Command::DaemonFingerprint) => {
            println!("{}", orion_daemon::lifecycle::BUILD_FINGERPRINT);
            Ok(())
        }
        Some(Command::OpenInGhostty) => orion_tui::open_in_ghostty(),
        None => match cli.dir {
            Some(dir) => orion_tui::run_add_project(dir),
            None => run_tui(),
        },
    }
}

/// Record a fatal top-level error in the log file before it goes to stderr —
/// the TUI's stderr disappears with the terminal, the daemon's is /dev/null.
fn log_fatal<T>(result: Result<T>, log_path: &Path) -> Result<T> {
    if let Err(err) = &result {
        orion_core::crashlog::append(log_path, &format!("FATAL {err:#}"));
    }
    result
}

fn log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_env(orion_core::env::LOG)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

/// How big a log may be when a process opens it. Past this the file is
/// set aside as `<name>.1` — the one before it dropped — and a new one
/// started, so the two together never hold more than this and one run's
/// worth: a daemon log grows by a few megabytes a month, and nothing else
/// ever trims it.
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Set `log_path` aside as `<name>.1` once it is past `max` bytes. Only
/// ever at open: a process already writing it keeps its handle, and its
/// lines land in the file set aside. Best effort — a log that could not
/// be moved is appended to as before.
fn rotate_log(log_path: &Path, max: u64) {
    let past = std::fs::metadata(log_path).is_ok_and(|meta| meta.len() > max);
    if past {
        let mut aside = log_path.as_os_str().to_owned();
        aside.push(".1");
        let _ = std::fs::rename(log_path, aside);
    }
}

/// Route tracing to `log_path` (created on demand, appended, no ANSI) —
/// neither binary can log to the terminal: the TUI owns it and the daemon
/// has no stderr. A log past [`LOG_MAX_BYTES`] is set aside first
/// ([`rotate_log`]).
fn init_file_logging(log_path: &Path) -> Result<()> {
    std::fs::create_dir_all(orion_core::paths::log_dir())?;
    rotate_log(log_path, LOG_MAX_BYTES);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    tracing_subscriber::fmt()
        .with_env_filter(log_filter())
        .with_writer(file)
        .with_ansi(false)
        .init();
    Ok(())
}

fn init_daemon_logging(foreground: bool) -> Result<()> {
    // The daemon runs detached with stderr on /dev/null — without this hook a
    // panic (on any thread, tokio workers included) leaves no trace.
    let log_path = orion_core::paths::daemon_log_path();
    orion_core::crashlog::install_panic_hook(log_path.clone());
    if foreground {
        tracing_subscriber::fmt()
            .with_env_filter(log_filter())
            .init();
        return Ok(());
    }
    init_file_logging(&log_path)
}

fn init_tui_logging() -> Result<()> {
    // Panic output to stderr dies with the alternate screen — capture it to
    // the log file. The TUI later wraps this hook with its terminal-restore,
    // so the chain on panic is: restore terminal → log to file → stderr.
    let log_path = orion_core::paths::tui_log_path();
    orion_core::crashlog::install_panic_hook(log_path.clone());
    // stdout belongs to the UI — log to file only.
    init_file_logging(&log_path)
}

/// The TUI, and what its exit asks for next.
fn run_tui() -> Result<()> {
    init_tui_logging()?;
    let exit = log_fatal(orion_tui::run_tui(), &orion_core::paths::tui_log_path())?;
    match exit {
        // Hosts-picker handoff: the TUI quit and restored the
        // terminal so a fresh `orion ssh` can exec over us (the
        // local daemon and its sessions stay up).
        orion_tui::Exit::Ssh(entry) => {
            eprintln!("orion: connecting to {}…", entry.host);
            ssh::run_ssh(&entry.host, entry.path.as_deref(), true)
        }
        // **Restart orion**: the daemon goes, and a fresh
        // `orion` execs over us.
        orion_tui::Exit::Restart => {
            log_fatal(orion_tui::restart(), &orion_core::paths::tui_log_path())
        }
        // **Upgrade orion**: install, then reopen the TUI on the new
        // binary — restarting the daemon too, unless the new build carries
        // the very daemon code that is running.
        orion_tui::Exit::Upgrade => {
            upgrade::install_only()?;
            if upgrade::daemon_carries_over() {
                eprintln!("orion: the daemon is unchanged — reopening, sessions kept…");
                orion_tui::relaunch()
            } else {
                orion_tui::restart()
            }
        }
        // Setup's Ghostty step: orion opens in a new Ghostty window, the
        // daemon and its sessions left up for it. Failing that, this
        // terminal is left as it is, with the reason.
        orion_tui::Exit::Ghostty => {
            if let Err(why) = orion_tui::open_in_ghostty() {
                eprintln!("orion: {why:#} — run orion again to carry on here");
            }
            Ok(())
        }
        orion_tui::Exit::Quit => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log past its cap is set aside as `.1` when it is next opened,
    /// taking the place of the one set aside before; one under it is left
    /// to be appended to.
    #[test]
    fn a_log_past_its_cap_is_set_aside_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("daemon.log");
        let aside = dir.path().join("daemon.log.1");
        std::fs::write(&log, "short").unwrap();
        rotate_log(&log, 10);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "short");
        assert!(!aside.exists());

        std::fs::write(&aside, "the one before").unwrap();
        std::fs::write(&log, "longer than ten bytes").unwrap();
        rotate_log(&log, 10);
        assert!(!log.exists(), "a new one starts");
        assert_eq!(
            std::fs::read_to_string(&aside).unwrap(),
            "longer than ten bytes"
        );
        rotate_log(&log, 10);
        assert!(aside.exists(), "no log is nothing to move");
    }
}
