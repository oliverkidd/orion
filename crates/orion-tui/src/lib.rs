pub mod agent_picker;
pub mod agent_presets;
pub mod app;
pub mod autofix;
pub mod branch_name;
pub mod branch_switch;
pub mod bundle;
pub mod claude_accounts;
pub mod claude_catalogue;
pub mod clipboard_image;
pub mod commit_list;
pub mod completion;
pub mod config;
pub mod cursor_catalogue;
pub mod diff_doc;
pub mod diff_tree;
pub mod doc_select;
pub mod doctor;
pub mod dropped_files;
pub mod editor;
pub mod event_loop;
pub mod file_tabs;
pub mod flash;
pub mod fuzzy;
pub mod ghostty_config;
pub mod git_diff;
pub(crate) mod git_proc;
pub mod git_sync;
pub mod grep_search;
pub mod hints;
pub mod hosts;
pub mod install;
pub mod ipc;
pub mod issues;
pub mod key_combo;
pub mod keymap;
pub mod keys;
pub mod launcher;
pub mod linear;
pub mod links;
pub(crate) mod list_hit;
pub mod markdown;
pub mod markdown_view;
pub mod mention;
pub mod onboard;
pub mod outside_editor;
pub mod overlay_close;
pub mod palette;
pub mod perf;
pub mod pr_actions;
pub mod pr_cache;
pub mod pr_modal;
pub mod pr_preview;
pub mod pr_row;
pub mod preset_overlays;
pub mod pull_request;
pub mod quick_prompt;
pub mod recent_files;
pub mod remote;
pub mod review;
pub mod saved_draft;
pub mod skills;
pub mod splash;
pub mod syntax;
pub mod terminal_tail;
pub mod text_input;
pub mod theme;
pub mod tree_browser;
pub mod ui;
pub mod update_check;
pub mod usage;
pub mod view_jobs;
pub mod vim_term;

use anyhow::Result;

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

pub use bundle::ConfigOp;

/// `orion config path | export | import`: locate, back up and restore this
/// machine's settings. See [`bundle`].
pub fn run_config(op: ConfigOp) -> Result<()> {
    bundle::run(op)
}

/// `orion doctor [--json]`: what this machine has of what orion leans on,
/// a line each with the fix for whatever is missing (see [`doctor`]).
/// False when something orion can't do without is missing.
pub fn run_doctor(json: bool) -> bool {
    doctor::run(json)
}

/// `orion setup`: forget that setup was seen, so the TUI about to start
/// opens every step of it again.
pub fn reset_setup() {
    let mut cfg = config::Config::load();
    cfg.onboarded = false;
    let _ = cfg.save();
}

pub use event_loop::Exit;

/// Entry point for the TUI client. Terminal setup/teardown lives here so the
/// binary crate stays a thin arg-parser. The [`Exit`] says what the caller
/// does next, the terminal already restored: nothing, exec `orion ssh` at
/// a recent host, or [`restart`].
pub fn run_tui() -> Result<Exit> {
    runtime()?.block_on(event_loop::run_app())
}

/// **Restart orion** (`⌘⇧R`), once the TUI has quit and restored the
/// terminal: stop the daemon and every session in it, as `orion kill`
/// does, then exec this binary again with the arguments it was started
/// with. The new client finds no daemon and spawns a fresh one from the
/// binary on disk — a rebuilt or upgraded orion included. Agents resume
/// their conversation on their next attach; terminals start a new shell.
/// Only returns when the exec fails.
pub fn restart() -> Result<()> {
    eprintln!("orion: restarting…");
    runtime()?.block_on(ipc::kill_daemon())?;
    relaunch()
}

/// Exec this binary again with the arguments it was started with, the
/// daemon left as it is. Only returns when the exec fails.
fn relaunch() -> Result<()> {
    use anyhow::Context as _;
    use std::os::unix::process::CommandExt as _;
    let mut args = std::env::args_os();
    // argv[0] over `current_exe`: a binary replaced since launch is the
    // one wanted, and Linux spells the old one `… (deleted)`.
    let program = match args.next() {
        Some(arg0) => std::path::PathBuf::from(arg0),
        None => std::env::current_exe().context("resolve current_exe")?,
    };
    let err = std::process::Command::new(&program).args(args).exec();
    Err(err).with_context(|| format!("restart {}", program.display()))
}

/// Post-upgrade daemon handoff: shut the daemon down only when it holds no
/// live sessions (see `ipc::shutdown_if_idle`).
pub fn shutdown_daemon_if_idle() -> Result<ipc::IdleShutdown> {
    runtime()?.block_on(ipc::shutdown_if_idle())
}

/// `orion rename` — agent-side session titling (see `ipc::rename_current_agent`).
/// `mode` is the CLI's `--force`, decided where the flag is parsed.
pub fn run_rename(title: String, mode: RenameMode) -> Result<()> {
    runtime()?.block_on(ipc::rename_current_agent(&title, mode))
}

/// `orion worktree [name] [--base <ref>]` — move the current agent session
/// into a worktree of its project (see `ipc::enter_worktree_for_current_agent`).
pub fn run_worktree(name: String, base: Option<String>) -> Result<()> {
    runtime()?.block_on(ipc::enter_worktree_for_current_agent(&name, base))
}

/// `orion open <file>…` — show files in every attached TUI's FILE TABS
/// (see `ipc::open_files_for_current_agent`).
pub fn run_open(files: Vec<String>) -> Result<()> {
    runtime()?.block_on(ipc::open_files_for_current_agent(&files))
}

/// `orion spawn "<task>" [--kind <kind>]` — start a new agent session
/// beside the current one (see `ipc::spawn_sibling_for_current_agent`).
/// `kind` is the CLI's `--kind`, already parsed where the flag is.
pub fn run_spawn(task: String, kind: Option<orion_core::AgentKind>) -> Result<()> {
    runtime()?.block_on(ipc::spawn_sibling_for_current_agent(&task, kind))
}

/// `orion add <dir>` / bare `orion <dir>` — register a directory as a
/// project (see `ipc::add_project`).
pub fn run_add_project(path: String) -> Result<()> {
    runtime()?.block_on(ipc::add_project(&path))
}

pub use ipc::RenameMode;

/// `orion kill`.
pub fn run_kill() -> Result<()> {
    runtime()?.block_on(async {
        if ipc::kill_daemon().await? {
            println!("orion daemon shut down");
        } else {
            println!("no orion daemon running");
        }
        Ok(())
    })
}
