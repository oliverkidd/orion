//! True end-to-end TUI test: runs the real `orion` binary inside a PTY,
//! sends literal keystrokes, and parses the rendered frames with vt100 —
//! asserting what a user would actually see on screen, including which row
//! is highlighted and which panel has focus.
//!
//! Flow under test:
//!   add two projects → Tab-walk focus → create two worktrees →
//!   j/k selection between worktrees → Enter into the sessions panel →
//!   create an agent (auto-attach) → per-worktree session isolation →
//!   j/k toggling between projects updates the worktree panel.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const COLS: u16 = 120;
const ROWS: u16 = 36;
const WAIT: Duration = Duration::from_secs(20);
/// Sleep between polls of the screen or a child.
const POLL_STEP: Duration = Duration::from_millis(50);

// Raw key bytes as the PTY sees them — the name replaces a trailing comment.
const ENTER: &[u8] = b"\r";
const TAB: &[u8] = b"\t";
const ESC: &[u8] = &[0x1b];
const DOWN: &[u8] = b"\x1b[B";
const SHIFT_LEFT: &[u8] = b"\x1b[1;2D";
const SHIFT_RIGHT: &[u8] = b"\x1b[1;2C";
const CTRL_Q: &[u8] = &[0x11];
const CTRL_R: &[u8] = &[0x12];
const CTRL_U: &[u8] = &[0x15];
const CTRL_S: &[u8] = &[0x13];
const CTRL_A: &[u8] = &[0x01];
/// `⌘U`, `⌘N`, `⌘E` and `⌘F` as Ghostty's kitty protocol spells them.
const CMD_U: &[u8] = b"\x1b[117;9u";
const CMD_N: &[u8] = b"\x1b[110;9u";
const CMD_K: &[u8] = b"\x1b[107;9u";
/// ⌘⇧N as the kitty protocol spells it: `n`, modifiers 1 + shift 1 + super 8.
const CMD_SHIFT_N: &[u8] = b"\x1b[110;10u";
const CMD_E: &[u8] = b"\x1b[101;9u";
const CMD_F: &[u8] = b"\x1b[102;9u";
const CMD_SHIFT_R: &[u8] = b"\x1b[114;10u";
/// `⌘;`: codepoint 59, modifiers 1 + 8 (super).
const CMD_SEMICOLON: &[u8] = b"\x1b[59;9u";
const SHIFT_TAB: &[u8] = b"\x1b[Z";
const SPACE: &[u8] = b" ";
/// The COMMAND PALETTE's chord every terminal sends (⌘⇧P needs Ghostty).
const COMMANDS: &[u8] = b":";

/// A row only the PROJECT's own menu carries.
const PROJECT_MENU_ROW: &str = "Remove from list";
/// Terminal pane input-locked: keys forward to the PTY, and Esc is the
/// way back out to the grid's cards.
const FOOTER_TERMINAL_LOCKED: &str = "Esc back to the grid";

/// A data dir as a user who finished first-run setup leaves it: setup
/// already seen and this build's release notes with it (either would
/// cover the grid every test drives) and the agents it offers switched
/// on, which a fresh install leaves off.
fn seed_onboarded_config(data_dir: &std::path::Path, settings: &str) {
    std::fs::create_dir_all(data_dir).unwrap();
    std::fs::write(
        data_dir.join("config.local.json"),
        format!(
            r#"{{"onboarded": true, "seen_version": "{}"}}"#,
            orion_tui::whats_new::current()
        ),
    )
    .unwrap();
    std::fs::write(
        data_dir.join("config.json"),
        format!(
            r#"{{"claude_enabled": true, "codex_enabled": true, "cursor_enabled": true,
            "opencode_enabled": true{settings}}}"#
        ),
    )
    .unwrap();
}

struct TuiHarness {
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    output: Arc<Mutex<Vec<u8>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    runtime_dir: PathBuf,
    data_dir: PathBuf,
    _repos: tempfile::TempDir,
}

impl TuiHarness {
    fn spawn() -> Self {
        Self::spawn_with_env(&[])
    }

    /// `spawn`, plus environment overrides for the TUI process — used to put
    /// a stub `gh` on PATH so the pull-request row can be driven without a
    /// GitHub account.
    fn spawn_with_env(extra_env: &[(&str, String)]) -> Self {
        Self::spawn_with(extra_env, "")
    }

    /// `spawn`, with `settings` — `, "key": value` pairs — added to the
    /// seeded config.json.
    fn spawn_with_settings(settings: &str) -> Self {
        Self::spawn_with(&[], settings)
    }

    fn spawn_with(extra_env: &[(&str, String)], settings: &str) -> Self {
        // Socket paths must stay under SUN_LEN (~104 bytes) — keep the
        // runtime dir short. Tests share one process, so a per-harness
        // sequence keeps each test on its own daemon.
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pid = std::process::id();
        let runtime_dir = PathBuf::from(format!("/tmp/oriontui-rt-{pid}-{seq}"));
        let data_dir = PathBuf::from(format!("/tmp/oriontui-data-{pid}-{seq}"));
        let _ = std::fs::remove_dir_all(&runtime_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
        seed_onboarded_config(&data_dir, settings);
        let repos = tempfile::tempdir().unwrap();

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_orion"));
        cmd.env(orion_core::env::RUNTIME_DIR, &runtime_dir);
        cmd.env(orion_core::env::DATA_DIR, &data_dir);
        cmd.env(orion_core::env::AGENT_CMD, "/bin/sh"); // stand-in for claude
        cmd.env(orion_core::env::WORKTREE_SYNC_MS, "100"); // fast external-change pickup
        cmd.env(orion_core::env::UPDATE_CHECK_SECS, "0"); // the footer must not depend on GitHub
        cmd.env(orion_core::env::SPOTIFY_POLL_SECS, "0"); // …nor on what's playing
        cmd.env(orion_core::env::NEEDS_YOU_POLL_SECS, "0"); // …nor on Linear and GitHub's inboxes
        cmd.env(orion_core::env::USAGE, "off"); // never read the machine's logins
        cmd.env(orion_core::env::MICROSOFT_POLL_SECS, "0"); // never call Microsoft Graph
        cmd.env(orion_core::env::PROJECT_ABOUT, "off"); // a new project runs no model
        cmd.env(orion_core::env::LOG, "debug");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("TERM", "xterm-256color");
        // Agent/CI shells often export NO_COLOR; crossterm then strips the
        // reverse/bold attrs wait_for_selected relies on.
        cmd.env_remove("NO_COLOR");
        cmd.env_remove("FORCE_COLOR");
        // Run from an ssh login, the TUI counts as remote: copies go out
        // as OSC 52 instead of the platform tool a test stubs, and the
        // host-terminal cwd reports stop.
        cmd.env_remove("SSH_CONNECTION");
        cmd.env_remove("SSH_TTY");
        // A project with a key has its Linear issues asked for as it is
        // selected: a test must never reach Linear with the machine's.
        cmd.env_remove("LINEAR_API_KEY");
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.cwd(repos.path());
        let child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);

        let mut reader = pty.master.try_clone_reader().unwrap();
        let writer = pty.master.take_writer().unwrap();
        // Keep the master alive for the whole test (dropping it hangs up the
        // TUI's tty); leak is fine in a test process.
        std::mem::forget(pty.master);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
        let output = Arc::new(Mutex::new(Vec::new()));
        {
            let parser = parser.clone();
            let output = output.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    output.lock().unwrap().extend_from_slice(&buf[..n]);
                    parser.lock().unwrap().process(&buf[..n]);
                }
            });
        }

        Self {
            writer,
            parser,
            output,
            child,
            runtime_dir,
            data_dir,
            _repos: repos,
        }
    }

    /// A committed git repo named `name` (fresh `git init` + one commit —
    /// worktrees need a HEAD to branch from).
    fn make_repo(&self, name: &str) -> PathBuf {
        let repo = self._repos.path().join(name);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| repo_git(&repo, args);
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@orion.dev"]);
        git(&["config", "user.name", "orion-test"]);
        std::fs::write(repo.join(".keep"), "").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "init"]);
        repo
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn type_str(&mut self, s: &str) {
        self.send(s.as_bytes());
    }

    fn screen_text(&self) -> String {
        let parser = self.parser.lock().unwrap();
        screen_to_text(parser.screen())
    }

    /// Poll the rendered screen until `pred` holds; panic with a full screen
    /// dump on timeout.
    fn wait_for(&self, what: &str, pred: impl Fn(&vt100::Screen) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            {
                let parser = self.parser.lock().unwrap();
                if pred(parser.screen()) {
                    return;
                }
            }
            if Instant::now() > deadline {
                let tui_log = std::fs::read_to_string(self.data_dir.join("state/tui.log"))
                    .unwrap_or_default();
                let tail: String = tui_log
                    .lines()
                    .rev()
                    .take(60)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                panic!(
                    "timed out waiting for: {what}\n--- screen ---\n{}\n--- tui.log tail ---\n{tail}",
                    self.screen_text()
                );
            }
            std::thread::sleep(POLL_STEP);
        }
    }

    fn wait_for_working_directory(&self, path: &Path) {
        let sequence = format!("\x1b]7;file://localhost{}\x1b\\", path.display());
        self.wait_for(&format!("working directory {path:?}"), |_| {
            self.output
                .lock()
                .unwrap()
                .windows(sequence.len())
                .any(|bytes| bytes == sequence.as_bytes())
        });
        self.output.lock().unwrap().clear();
    }

    fn wait_for_text(&self, needle: &str) {
        self.wait_for(&format!("text {needle:?}"), |s| {
            screen_to_text(s).contains(needle)
        });
    }

    /// Is `needle` on screen within `window`? Unlike `wait_for_text` this
    /// answers false instead of failing — for a flash that has to be let
    /// go of, and for asserting that something never comes up at all.
    fn try_wait_for_text(&self, needle: &str, window: Duration) -> bool {
        let deadline = Instant::now() + window;
        loop {
            if screen_to_text(self.parser.lock().unwrap().screen()).contains(needle) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(POLL_STEP);
        }
    }

    fn wait_for_gone(&self, needle: &str) {
        self.wait_for(&format!("text {needle:?} to disappear"), |s| {
            !screen_to_text(s).contains(needle)
        });
    }

    /// Wait until the row containing `needle` renders with the selection
    /// fill (the raised `sel_bg` / `sel_bg_dim` background bar).
    fn wait_for_selected(&self, needle: &str) {
        self.wait_for(&format!("row {needle:?} selected (filled)"), |s| {
            row_is_selected(s, needle)
        });
    }
}

impl Drop for TuiHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Stop the auto-spawned daemon and clean the short-lived dirs.
        let _ = std::process::Command::new(env!("CARGO_BIN_EXE_orion"))
            .arg("kill")
            .env(orion_core::env::RUNTIME_DIR, &self.runtime_dir)
            .env(orion_core::env::DATA_DIR, &self.data_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.runtime_dir);
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

fn screen_to_text(screen: &vt100::Screen) -> String {
    let (rows, cols) = screen.size();
    let mut out = String::new();
    for row in 0..rows {
        for col in 0..cols {
            match screen.cell(row, col) {
                Some(cell) => {
                    let contents = cell.contents();
                    if contents.is_empty() {
                        out.push(' ');
                    } else {
                        out.push_str(contents);
                    }
                }
                None => out.push(' '),
            }
        }
        out.push('\n');
    }
    out
}

fn row_is_selected(screen: &vt100::Screen, needle: &str) -> bool {
    let (rows, cols) = screen.size();
    for row in 0..rows {
        let mut line = String::new();
        for col in 0..cols {
            let contents = screen
                .cell(row, col)
                .map(|c| c.contents())
                .unwrap_or_default();
            if contents.is_empty() {
                line.push(' ');
            } else {
                line.push_str(contents);
            }
        }
        if let Some(at) = line.find(needle) {
            // Selection paints the row with the raised fill: indexed 236 in
            // the focused panel, 234 in unfocused ones (theme sel_bg /
            // sel_bg_dim). Only the needle's own panel band counts — the
            // panels share screen lines, and a selected pill in the next
            // column over used to pass this check for a row it had nothing
            // to do with.
            let at = line[..at].chars().count();
            let chars: Vec<char> = line.chars().collect();
            let band_start = chars[..at]
                .iter()
                .rposition(|&c| c == '│')
                .map_or(0, |i| i + 1);
            let band_end = chars[at..]
                .iter()
                .position(|&c| c == '│')
                .map_or(chars.len(), |i| at + i);
            let filled = (band_start..band_end).any(|col| {
                matches!(
                    screen.cell(row, col as u16).map(|c| c.bgcolor()),
                    Some(vt100::Color::Idx(236)) | Some(vt100::Color::Idx(234))
                )
            });
            if filled {
                return true;
            }
        }
    }
    false
}

fn add_project(tui: &mut TuiHarness, path: &Path, expect_name: &str) {
    run_command(tui, "open a folder");
    tui.wait_for_text("Open project");
    tui.type_str(&path.to_string_lossy());
    tui.send(ENTER);
    // The prompt must close before asserting grid content — otherwise the
    // overlay's own text can satisfy the wait (stale-frame race).
    tui.wait_for_gone("Open project");
    // Nothing opens over the grid — adding the first project no more than
    // any other. Callers land on the grid because nothing covered it.
    assert!(
        !tui.try_wait_for_text("what should the agent do?", Duration::from_millis(600)),
        "no modal may open on its own",
    );
    tui.wait_for_text(expect_name);
}

/// Pick `name` from the COMMAND PALETTE: `:`, its name typed, Enter.
fn run_command(tui: &mut TuiHarness, name: &str) {
    tui.send(COMMANDS);
    tui.wait_for_text("Command ⌕");
    tui.type_str(name);
    tui.send(ENTER);
}

/// The project's own menu, where its verbs live: a right-click on its
/// PROJECT TAB, which reads `tab` — the header is the top of the screen,
/// so the first place it appears is the tab.
fn open_project_menu(tui: &mut TuiHarness, tab: &str) {
    let at = find_text(tui.parser.lock().unwrap().screen(), tab);
    let (row, col) = at.unwrap_or_else(|| panic!("no {tab:?} tab:\n{}", tui.screen_text()));
    tui.send(&sgr_mouse(2, col, row, false));
    tui.send(&sgr_mouse(2, col, row, true));
    tui.wait_for_text(PROJECT_MENU_ROW);
}

/// Walk the open menu down to the row that reads `label`, and pick it.
fn choose_menu_row(tui: &mut TuiHarness, label: &str) {
    for _ in 0..12 {
        let deadline = Instant::now() + Duration::from_millis(400);
        while Instant::now() < deadline {
            if row_is_selected(tui.parser.lock().unwrap().screen(), label) {
                tui.send(ENTER);
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        tui.send(DOWN);
    }
    panic!(
        "menu row {label:?} never came under the cursor:\n{}",
        tui.screen_text()
    );
}

/// A session in the selected checkout, started from the box the way the
/// grid starts one, then stepped into so the pane has the keys. `task` is
/// typed into the box — the grid's box will not launch on an empty one.
fn start_session(tui: &mut TuiHarness, task: &str) {
    tui.send(CMD_N);
    tui.wait_for_text("what should the agent do?");
    tui.type_str(task);
    tui.send(ENTER);
    tui.wait_for_gone("what should the agent do?");
    // The launch lands the card without taking the pane; Enter steps into
    // it, which is where the keys have to be to type at the agent.
    tui.wait_for_text("agent-1");
    tui.send(ENTER);
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED);
}

/// What `git -C repo <args>` prints, trimmed.
fn git_stdout(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo_git(repo: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed in {}", repo.display());
}

/// Host-terminal links follow the visible checkout, not the directory where
/// orion was launched. On exit, the shell gets its original directory back.
#[test]
fn host_working_directory_follows_project_switches_and_restores_on_exit() {
    let mut tui = TuiHarness::spawn();
    let first = tui.make_repo("cwd-first").canonicalize().unwrap();
    let second = tui.make_repo("cwd-second").canonicalize().unwrap();
    tui.wait_for_text("to begin");
    add_project(&mut tui, &first, "cwd-first");
    tui.wait_for_working_directory(&first);
    add_project(&mut tui, &second, "cwd-second");
    tui.wait_for_working_directory(&second);

    tui.send(&[0x03]); // ^C: quit has no bare key
    tui.wait_for_text("Quit orion");
    tui.send(ENTER);
    tui.wait_for_working_directory(&tui._repos.path().canonicalize().unwrap());
}

/// `⌘K` with nothing typed is the NEEDS YOU board — its five sections,
/// each saying it is empty in the same words — and typing turns it into
/// the jump list, which Esc closes.
#[test]
fn tui_cmd_k_is_the_needs_you_board_until_you_type() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("board-demo");
    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "board-demo");

    tui.send(CMD_K);
    tui.wait_for_text("Needs you");
    for empty in [
        "No agents waiting for you",
        "No Linear issues waiting for you",
        "No code reviews waiting for you",
        "No Slack messages waiting for you",
        "No meetings waiting for you",
    ] {
        tui.wait_for_text(empty);
    }

    tui.type_str("board");
    tui.wait_for_text("Jump to (");
    tui.wait_for_gone("No agents waiting for you");

    tui.send(ESC);
    tui.wait_for_gone("Jump to (");
}

#[test]
fn tui_help_modal_grouped_keymap() {
    let mut tui = TuiHarness::spawn();
    tui.wait_for_text("to begin");

    // The grouped two-column keymap: every section header on screen at
    // once (the old single list clipped its tail on short terminals).
    run_command(&mut tui, "keyboard shortcuts");
    tui.wait_for_text("NAVIGATE & SEARCH");
    tui.wait_for_text("CHECKOUTS & GITHUB");
    tui.wait_for_text("SESSIONS");
    tui.wait_for_text("TERMINAL & MOUSE");
    tui.wait_for_text("GENERAL");

    tui.send(ESC); // closes
    tui.wait_for_gone("NAVIGATE & SEARCH");
}

/// `orion open` typed into a live session's shell — the stand-in agent is
/// `/bin/sh`, holding the AGENT ENV a real CLI would — raises the FILE
/// TABS in the TUI attached to that daemon: one tab per file, the first
/// previewed. Ctrl+Q from the strip closes it and hands the pane back.
#[test]
fn orion_open_from_inside_a_session_raises_the_file_tabs() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("open-proj");
    let alpha = repo.join("alpha.md");
    let beta = repo.join("beta.rs");
    std::fs::write(&alpha, "# alpha opened from the session\n").unwrap();
    std::fs::write(&beta, "fn beta() {}\n").unwrap();

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "open-proj");

    // ---- an agent (the stand-in shell), stepped into and locked ----
    start_session(&mut tui, "hello");

    // ---- what the model runs, typed at the shell inside the session ----
    tui.type_str(&format!(
        "{} open {} {}",
        env!("CARGO_BIN_EXE_orion"),
        alpha.display(),
        beta.display()
    ));
    tui.send(ENTER);
    tui.wait_for_text("Open files (2)");
    // The preview is the file itself — text the shell never echoed.
    tui.wait_for_text("alpha opened from the session");
    tui.wait_for_text("Enter edit in");

    // ---- → moves to the next tab and its preview ----
    tui.send(b"\x1b[C");
    tui.wait_for_text("fn beta() {}");

    // ---- Ctrl+Q from the strip closes; the locked pane is back ----
    tui.send(CTRL_Q);
    tui.wait_for_gone("Open files (2)");
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED);
}

/// Renaming a project is a label change, and an empty name undoes it.
/// Drives the real binary: the row picks up the new label with the folder
/// name hanging off a `└` underneath, then clearing the field puts the row
/// back on the folder's own name with nothing under it.
#[test]
fn tui_project_rename_shows_the_folder_and_empty_undoes_it() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("acme-repo");

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "acme-repo");

    // ---- rename: the tab takes the label ----
    open_project_menu(&mut tui, "acme-repo");
    choose_menu_row(&mut tui, "Rename");
    tui.wait_for_text("Rename project");
    // The field is prefilled with the current name; clear it first.
    tui.send(b"\x15"); // ^u
    tui.type_str("Acme API");
    tui.send(b"\r");
    tui.wait_for_gone("Rename project");
    tui.wait_for_text("Acme API");

    // ---- undo: an empty name puts the tab back on the folder name ----
    open_project_menu(&mut tui, "Acme API");
    choose_menu_row(&mut tui, "Rename");
    tui.wait_for_text("Rename project");
    tui.send(b"\x15"); // ^u clears the prefill
    tui.send(b"\r");
    tui.wait_for_gone("Rename project");
    // The chosen label is gone — the folder name is the card again,
    // exactly as a freshly added project renders.
    tui.wait_for_gone("Acme API");
    tui.wait_for_text("acme-repo");
}

/// ⌘⇧N, as Orion.app's Ghostty sends it, opens the new-agent box aimed
/// at a fresh worktree, in one press.
#[test]
fn tui_cmd_shift_n_opens_the_box_on_a_new_worktree() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("fresh-proj");

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "fresh-proj");

    tui.send(CMD_SHIFT_N);
    tui.wait_for_text("what should the agent do?");
    // The box's header names the branch it will cut — a line of its own,
    // not the empty band's hint naming the key.
    let screen = tui.screen_text();
    assert!(
        screen
            .lines()
            .any(|l| l.contains("new worktree ") && !l.contains("in a new worktree")),
        "{screen}"
    );
}

/// Manual LINK creation is intentionally absent: Shift+L is unbound and
/// the HELP OVERLAY offers no attach-link action.
#[test]
fn tui_manual_link_add_is_unavailable() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("link-proj");

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "link-proj");

    tui.send(b"L");
    run_command(&mut tui, "keyboard shortcuts");
    // If Shift+L still opened a prompt, this `:` would type into it instead
    // of opening HELP OVERLAY, so this heading proves the key was a no-op.
    tui.wait_for_text("NAVIGATE & SEARCH");
    tui.wait_for_gone("attach a link");
}

/// The DIFF VIEWER's flat file list, the way this walk reads it: the
/// list flat, and the keys handed from the commits to the files.
#[test]
fn tui_git_diff_modal() {
    let mut tui = TuiHarness::spawn_with_settings(r#", "diff_tree_view": false"#);
    let repo = tui.make_repo("diff-proj");

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "diff-proj");
    // The root worktree row must exist before g has anything to diff.

    // Dirty the checkout: one tracked modification, one untracked file.
    std::fs::write(repo.join(".keep"), "tracked change\n").unwrap();
    std::fs::write(repo.join("hello.txt"), "hello world\n").unwrap();

    // No wait on a changed-file count: it rides the session cards, and
    // this project has none. ^e reads the checkout for itself.

    // ---- open the modal; the selected file's diff renders ----
    tui.send(CMD_E);
    tui.wait_for_text("Files (2)");
    // It opens on the commits; Tab hands the keys to the files. Status
    // is path-ordered, so .keep (modified) is selected first.
    tui.send(TAB);
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+ tracked change");

    // ---- Ctrl+r marks .keep reviewed: it sinks below hello.txt and the
    // selection auto-advances to the next file, loading its diff ----
    tui.send(CTRL_R);
    tui.wait_for_text("· 1✓"); // files-panel title counts the mark
    tui.wait_for_selected("hello.txt");
    tui.wait_for_text("+ hello world");

    // ---- Down reaches the reviewed zone; Ctrl+r unmarks .keep, which
    // pops back to the top of the list and stays selected ----
    tui.send(DOWN);
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+ tracked change");
    tui.send(CTRL_R);
    tui.wait_for_gone("· 1✓");

    // ---- arrow to the untracked file ----
    tui.send(DOWN);
    tui.wait_for_selected("hello.txt");
    tui.wait_for_text("+ hello world");

    // ---- type-to-filter narrows the list and reselects the top match ----
    tui.type_str("kee");
    tui.wait_for_text("Files (1/2)");
    tui.wait_for_selected(".keep");
    tui.wait_for_text("+ tracked change");
    tui.send(CTRL_U); // clears the filter, not the modal
    tui.wait_for_text("Files (2)");

    // ---- the modal blocks other interaction ----
    // ^n would open the box on the grid; inside the modal it is the
    // filter's (verified after close — stale-frame convention).
    tui.send(b"n");
    tui.wait_for_text("no matches");
    tui.send(ESC); // closes the modal, filter and all
    tui.wait_for_gone("Files (");
    assert!(
        !tui.screen_text().contains("what should the agent do?"),
        "modal swallowed n\n--- screen ---\n{}",
        tui.screen_text()
    );

    // ---- a clean tree opens nothing ----
    repo_git(&repo, &["add", "."]);
    repo_git(&repo, &["commit", "-m", "wip"]);
    tui.send(CMD_E);
    // Opened ahead of `git status` off a badge that has not caught up yet,
    // the modal closes again once git says there is nothing to show.
    if tui.try_wait_for_text("Files (", Duration::from_secs(2)) {
        tui.wait_for_gone("Files (");
    }
    assert!(
        !tui.screen_text().contains("Files ("),
        "a clean tree opens no modal\n--- screen ---\n{}",
        tui.screen_text()
    );
}

/// The DIFF VIEWER's COMMIT LIST end to end, on the defaults
/// — the keys on the commits as it opens: a clean checkout with commits
/// of its own opens on the whole branch, every commit ticked, instead of
/// saying "no changes"; `^A` unticks them and the cursor's commit shows
/// exactly its files, the reading pane's border naming it; `⇧←`/`⇧→` step
/// older and newer from the files; ticked commits read together; and a
/// dirty checkout opens on its uncommitted changes, as it always did.
#[test]
fn tui_diff_steps_through_a_branch_commit_by_commit() {
    let mut tui = TuiHarness::spawn();
    let repo = tui.make_repo("commits-proj");
    // `origin/HEAD` on the first commit: what a worktree the DAEMON cut
    // from origin sees.
    repo_git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo_git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    std::fs::write(repo.join("alpha.txt"), "alpha line\n").unwrap();
    repo_git(&repo, &["add", "."]);
    repo_git(&repo, &["commit", "-m", "add alpha"]);
    std::fs::write(repo.join("beta.txt"), "beta line\n").unwrap();
    repo_git(&repo, &["add", "."]);
    repo_git(&repo, &["commit", "-m", "add beta"]);

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "commits-proj");
    // The reading pane's top border, naming the commit it reads: the
    // message itself is not drawn over the diff.
    let reading = |subject: &str| format!("╭ {subject} ─");

    // ---- nothing uncommitted: the whole branch, every commit ticked ----
    tui.send(CMD_E);
    tui.wait_for_text("Commits · 2 since origin/main");
    tui.wait_for_text("all ticked · together");
    tui.wait_for_text("Files (2)");
    tui.wait_for_text("+ alpha line");

    // ---- ^A, the keys already on the commits: nothing ticked, the
    // newest commit, exactly its files ----
    tui.send(CTRL_A);
    tui.wait_for_text("Files (1)");
    tui.wait_for_text(&reading("add beta"));
    tui.wait_for_text("+ beta line");
    tui.wait_for_gone("+ alpha line");

    // ---- Enter hands the keys on; ⇧← is the commit before, the border
    // naming it over its diff; ⇧→ the one after ----
    tui.send(ENTER);
    tui.send(SHIFT_LEFT);
    tui.wait_for_text(&reading("add alpha"));
    tui.wait_for_text("+ alpha line");
    tui.wait_for_gone("+ beta line");
    tui.send(SHIFT_RIGHT);
    tui.wait_for_text(&reading("add beta"));
    tui.wait_for_text("+ beta line");
    tui.wait_for_gone("+ alpha line");

    // ---- tick both: read together, both commits' files listed, the
    // reading pane on one file at a time ----
    tui.send(SHIFT_TAB);
    tui.send(SPACE);
    tui.send(DOWN);
    tui.send(SPACE);
    tui.wait_for_text("all ticked · together");
    tui.wait_for_text(&reading("The whole branch since origin/main"));
    tui.wait_for_text("Files (2)");
    tui.wait_for_text("+ alpha line");
    tui.send(ENTER);
    tui.send(DOWN);
    tui.wait_for_text("+ beta line");
    tui.wait_for_gone("+ alpha line");
    tui.send(ESC);
    tui.wait_for_gone("Commits ·");

    // ---- something uncommitted: the view it always was, on top ----
    std::fs::write(repo.join("wip.txt"), "wip line\n").unwrap();
    tui.send(CMD_E);
    tui.wait_for_selected("Uncommitted changes");
    tui.wait_for_text("+ wip line");
    tui.send(ESC);
    tui.wait_for_gone("Commits ·");
}

/// An SGR mouse report as the terminal would send it: `button` (0 = left,
/// 32 = left held while moving) at 0-based `col`,`row`.
fn sgr_mouse(button: u16, col: u16, row: u16, release: bool) -> Vec<u8> {
    format!(
        "\x1b[<{button};{};{}{}",
        col + 1,
        row + 1,
        if release { 'm' } else { 'M' }
    )
    .into_bytes()
}

/// Where `needle` first appears on screen: (row, col), in cells.
fn find_text(screen: &vt100::Screen, needle: &str) -> Option<(u16, u16)> {
    screen_to_text(screen)
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            let at = line.find(needle)?;
            Some((row as u16, line[..at].chars().count() as u16))
        })
}

/// How far back the pane says it is scrolled — the `scroll N` tag in the
/// TERMINAL header — or 0 at the live tail.
fn scrolled(screen: &vt100::Screen) -> usize {
    let text = screen_to_text(screen);
    text.match_indices("scroll ")
        .filter_map(|(at, _)| {
            let digits: String = text[at + 7..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
        .max()
        .unwrap_or(0)
}

/// A drag-select past the pane's top edge scrolls the history under the
/// pointer on the event loop's own beat — with no further mouse report —
/// and the release copies rows that were never on screen together. A stub
/// `pbcopy` on PATH catches the copy.
#[test]
fn tui_drag_past_the_pane_top_autoscrolls_and_copies_the_run() {
    use std::os::unix::fs::PermissionsExt;
    let stub_bin = tempfile::tempdir().unwrap();
    let copied = stub_bin.path().join("copied");
    let stub = stub_bin.path().join("pbcopy");
    std::fs::write(&stub, format!("#!/bin/sh\ncat > {}\n", copied.display())).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        stub_bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut tui = TuiHarness::spawn_with_env(&[("PATH", path)]);
    let repo = tui.make_repo("drag-proj");

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "drag-proj");

    start_session(&mut tui, "hello");

    // Sixty numbered rows out of the stand-in shell: twice the pane's height.
    tui.type_str("i=1; while [ $i -le 60 ]; do echo \"row $i\"; i=$((i+1)); done");
    tui.send(ENTER);
    tui.wait_for_text("row 60");

    // The pane's first content row is two below its header;
    // `row 58` sits near the bottom of the pane.
    let (header_row, content_top, row58, col58) = {
        let parser = tui.parser.lock().unwrap();
        let screen = parser.screen();
        // The session's mark is the WORKING SPINNER while it runs, on
        // whichever frame the screen caught.
        let (header_row, _) = ["✻", "✽", "✶", "✳", "✢", "·", "●"]
            .iter()
            .find_map(|mark| find_text(screen, &format!("{mark} agent-1  ⌂")))
            .unwrap_or_else(|| panic!("the pane header:\n{}", screen_to_text(screen)));
        let (row58, col58) = find_text(screen, "row 58").expect("row 58 on screen");
        (header_row, header_row + 2, row58, col58)
    };
    // The PANE is a third of the body under the GRID, so "well inside it"
    // is a handful of rows rather than half a screen.
    assert!(
        row58 > content_top + 2,
        "row 58 is well inside the pane (header {header_row})"
    );
    // Rows on screen above `row 58` at the press; the top one is
    // `row {58 - visible_above}`.
    let visible_above = usize::from(row58 - content_top);
    let off_screen_row = format!("row {}", 58 - visible_above - 3);
    assert!(!tui.screen_text().contains(&off_screen_row));

    // Press on the last character of `row 58`, drag up onto the pane's
    // top row, then one row further — onto the rule above it — and rest.
    tui.send(&sgr_mouse(0, col58 + 5, row58, false));
    tui.send(&sgr_mouse(32, col58, content_top, false));
    tui.send(&sgr_mouse(32, col58, content_top - 1, false));
    // The loop's beat scrolls the history under the resting pointer; the
    // header counts the lines.
    tui.wait_for("the pane to scroll back under the held drag", |s| {
        scrolled(s) >= 5
    });
    tui.wait_for_text(&off_screen_row);

    // Release there: the copy runs from rows above anything that was on
    // screen at the press down to `row 58`.
    tui.send(&sgr_mouse(0, col58, content_top - 1, true));
    tui.wait_for_text("copied");
    let deadline = Instant::now() + WAIT;
    while !std::fs::read_to_string(&copied).is_ok_and(|t| t.ends_with("row 58")) {
        assert!(
            Instant::now() < deadline,
            "the copy never reached the stub clipboard"
        );
        std::thread::sleep(POLL_STEP);
    }
    let text = std::fs::read_to_string(&copied).unwrap();
    let rows: Vec<&str> = text.lines().collect();
    assert!(
        rows.len() >= visible_above + 6,
        "rows above the screen at the press are in the copy: {} rows, {visible_above} visible above row 58\n{text}",
        rows.len()
    );
    assert!(text.contains(&off_screen_row), "{text}");
    // Every row is one the shell printed, in order.
    let first: usize = rows[0]
        .strip_prefix("row ")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("a numbered row first: {:?}", rows[0]));
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(*row, format!("row {}", first + i), "{text}");
    }
}

/// AUTOFIX, end to end: with **Auto-fix my PRs** on — an older orion's
/// `ask` reads as on — a pull request of the user's that `gh` says
/// conflicts sends an `autofix-<n>` agent at it on its own, with no modal
/// to answer. A stub `gh` on PATH answers the open list (GraphQL) and the
/// PR's body.
#[test]
fn tui_autofix_sends_unasked_when_my_pr_conflicts() {
    let stub_bin = tempfile::tempdir().unwrap();
    let gh = stub_bin.path().join("gh");
    std::fs::write(
        &gh,
        concat!(
            "#!/bin/sh\n",
            "case \"$1 $2 $3\" in\n",
            "  'api graphql'*) printf '%s' '{\"data\":{\"repository\":{\"pullRequests\":{\"nodes\":[",
            "{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",\"title\":\"Add things\",",
            "\"isDraft\":false,\"headRefName\":\"feat\",\"isCrossRepository\":false,",
            "\"headRepositoryOwner\":{\"login\":\"o\"},\"mergeable\":\"CONFLICTING\",",
            "\"viewerDidAuthor\":true,\"headRefOid\":\"abc123\",",
            "\"commits\":{\"nodes\":[{\"commit\":{\"statusCheckRollup\":{\"state\":\"SUCCESS\"}}}]}}",
            "]}}}}' ;;\n",
            "  'pr view 7') printf '%s' '{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",",
            "\"title\":\"Add things\",\"state\":\"OPEN\",\"isDraft\":false,\"mergeable\":\"CONFLICTING\",",
            "\"statusCheckRollup\":[],\"author\":{\"login\":\"me\"},\"baseRefName\":\"main\",",
            "\"headRefName\":\"feat\",\"headRefOid\":\"abc123\",\"comments\":[],\"reviews\":[]}' ;;\n",
            "  'pr view'*) echo 'no pull requests found for branch \"main\"' >&2; exit 1 ;;\n",
            "  *) exit 1 ;;\n",
            "esac\n",
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        stub_bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut tui = TuiHarness::spawn_with(&[("PATH", path)], r#", "pr_autofix": "ask""#);
    let repo = tui.make_repo("autofix-proj");
    // The pull request's head branch, already here: with no origin to
    // fetch it from, the DAEMON checks this one out for the agent
    // (`git::add_pr_worktree`). Without it the create is refused at once,
    // and `autofix-7` is on screen only between the send and the refusal.
    repo_git(&repo, &["branch", "feat"]);
    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "autofix-proj");

    // The agent running in the DAEMON — the footer counts it off the
    // daemon's own metrics — not only the `autofix-7 started` flash and
    // the stand-in row, which go up the moment the create is sent.
    tui.wait_for("autofix-7 running", |s| {
        let text = screen_to_text(s);
        text.contains("autofix-7") && text.contains("1 agent ·")
    });
    assert!(
        !tui.screen_text().contains("[x] Merge conflicts"),
        "no modal to answer:\n{}",
        tui.screen_text()
    );
}

/// The CONFLICT RESOLVER, end to end against a real remote: Enter on a
/// conflicted file opens it, taking a side hands the pane to the log, `^S`
/// shows a hook's output while the hook is still running, and the push
/// landing puts the pull request's Checks tab up, the merge on the remote.
#[test]
fn tui_conflict_resolver_runs_through_to_the_pull_request() {
    let repos = tempfile::tempdir().unwrap();
    let repo = repos.path().join("merge-proj");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| repo_git(&repo, args);
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@orion.dev"]);
    git(&["config", "user.name", "orion-test"]);
    git(&["config", "commit.gpgsign", "false"]);
    let origin = repos.path().join("merge-origin.git");
    let ok = std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&origin)
        .status()
        .unwrap()
        .success();
    assert!(ok, "bare origin");
    git(&["remote", "add", "origin", &origin.to_string_lossy()]);
    std::fs::write(repo.join("a.txt"), "one\nshared\nthree\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "base"]);
    git(&["switch", "-q", "-c", "feat"]);
    std::fs::write(repo.join("a.txt"), "one\nfeature\nthree\n").unwrap();
    git(&["commit", "-q", "-am", "feature"]);
    git(&["switch", "-q", "main"]);
    std::fs::write(repo.join("a.txt"), "one\nupstream\nthree\n").unwrap();
    git(&["commit", "-q", "-am", "upstream"]);
    git(&["push", "-q", "origin", "main"]);
    git(&["switch", "-q", "feat"]);
    git(&["push", "-q", "-u", "origin", "feat"]);
    let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    // A hook that is slow enough to watch, and says so first.
    let hook = repo.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho hook-is-running\nsleep 2\n").unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let stub_bin = tempfile::tempdir().unwrap();
    let gh = stub_bin.path().join("gh");
    let body = format!(
        concat!(
            "{{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",\"title\":\"Add things\",",
            "\"state\":\"OPEN\",\"isDraft\":false,\"mergeable\":\"CONFLICTING\",",
            "\"statusCheckRollup\":[],\"author\":{{\"login\":\"me\"}},\"baseRefName\":\"main\",",
            "\"headRefName\":\"feat\",\"headRefOid\":\"{head}\",\"comments\":[],\"reviews\":[],",
            "\"files\":[{{\"path\":\"a.txt\",\"additions\":1,\"deletions\":1,",
            "\"changeType\":\"MODIFIED\"}}]}}"
        ),
        head = head
    );
    let list = format!(
        concat!(
            "{{\"data\":{{\"repository\":{{\"pullRequests\":{{\"nodes\":[",
            "{{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",\"title\":\"Add things\",",
            "\"isDraft\":false,\"headRefName\":\"feat\",\"isCrossRepository\":false,",
            "\"headRepositoryOwner\":{{\"login\":\"o\"}},\"mergeable\":\"CONFLICTING\",",
            "\"viewerDidAuthor\":true,\"headRefOid\":\"{head}\",",
            "\"commits\":{{\"nodes\":[{{\"commit\":{{\"statusCheckRollup\":",
            "{{\"state\":\"SUCCESS\"}}}}}}]}}}}]}}}}}}}}"
        ),
        head = head
    );
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\ncase \"$1 $2 $3\" in\n  'api graphql'*) printf '%s' '{list}' ;;\n  'pr view 7') printf '%s' '{body}' ;;\n  *) exit 1 ;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        stub_bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // The pull request conflicts: autofix, on by default, would send an
    // agent at it and the resolver would wait for that agent to stop.
    let mut tui = TuiHarness::spawn_with(&[("PATH", path)], r#", "pr_autofix": "off""#);
    tui.wait_for_text("to begin");
    // On the grid with the project in it: `⌘U` sent the moment the prompt
    // closes can reach the TUI before the daemon has added the project,
    // and opens nothing.
    add_project(&mut tui, &repo, "merge-proj");

    tui.send(CMD_U);
    tui.wait_for_text("Add things");
    tui.send(SHIFT_RIGHT);
    tui.wait_for_text("a.txt");
    tui.wait_for_text("✗ 1");
    tui.send(TAB);
    tui.send(ENTER);

    tui.wait_for_text("Merge — #7");
    tui.wait_for_text("● Resolve conflicts  1 left");
    // `l` reads what the merge said, and goes back to the file.
    tui.send(b"l");
    tui.wait_for_text("CONFLICT (content)");
    tui.send(b"l");
    tui.wait_for_text("accept incoming ·");
    tui.send(b"i");
    tui.wait_for_text("Nothing is committed yet.");
    tui.wait_for_text("● Commit the merge");

    tui.send(CTRL_S);
    tui.wait_for_text("Log · committing");
    tui.wait_for_text("hook-is-running");
    assert!(
        tui.screen_text().contains("Log · committing"),
        "the hook's words while it still runs:\n{}",
        tui.screen_text()
    );
    tui.wait_for_text("Checks");
    tui.wait_for_gone("Merge — #7");

    let on_origin = git_stdout(&origin, &["rev-list", "--count", "--merges", "feat"]);
    assert_eq!(on_origin, "1", "the merge reached the remote");
    let merged = git_stdout(&origin, &["show", "feat:a.txt"]);
    assert_eq!(merged, "one\nupstream\nthree", "the side taken");
}

/// The PR REVIEW, end to end against a stub `gh`: `⌘⇧R` in the PULL
/// REQUESTS MODAL reads the pull request's diff from the repo and its
/// threads from GitHub, ada's under the line she commented on; `c` on a
/// line writes a comment that GitHub holds in a pending review the comment
/// starts, and `⌘⇧R` sends the review with it. The stub writes down every
/// request body it is handed, which is what GitHub would have been sent.
#[test]
fn tui_pr_review_comments_on_a_line_and_sends_the_review() {
    let stub_bin = tempfile::tempdir().unwrap();
    let sent = stub_bin.path().join("sent");
    let mut tui = TuiHarness::spawn_with(
        &[(
            "PATH",
            format!(
                "{}:{}",
                stub_bin.path().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )],
        r#", "pr_autofix": "off""#,
    );
    let repo = tui.make_repo("review-proj");
    std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    repo_git(&repo, &["add", "."]);
    repo_git(&repo, &["commit", "-q", "-m", "base"]);
    repo_git(&repo, &["switch", "-q", "-c", "feat"]);
    std::fs::write(repo.join("a.txt"), "one\ndeux\nthree\n").unwrap();
    repo_git(&repo, &["commit", "-q", "-am", "feature"]);
    let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    repo_git(&repo, &["switch", "-q", "main"]);

    let list = format!(
        concat!(
            "{{\"data\":{{\"repository\":{{\"pullRequests\":{{\"nodes\":[",
            "{{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",\"title\":\"Add things\",",
            "\"isDraft\":false,\"headRefName\":\"feat\",\"isCrossRepository\":false,",
            "\"headRepositoryOwner\":{{\"login\":\"o\"}},\"mergeable\":\"MERGEABLE\",",
            "\"viewerDidAuthor\":false,\"headRefOid\":\"{head}\",",
            "\"commits\":{{\"nodes\":[{{\"commit\":{{\"statusCheckRollup\":",
            "{{\"state\":\"SUCCESS\"}}}}}}]}}}}]}}}}}}}}"
        ),
        head = head
    );
    let detail = format!(
        concat!(
            "{{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",\"title\":\"Add things\",",
            "\"state\":\"OPEN\",\"isDraft\":false,\"mergeable\":\"MERGEABLE\",",
            "\"statusCheckRollup\":[],\"author\":{{\"login\":\"ada\"}},\"baseRefName\":\"main\",",
            "\"headRefName\":\"feat\",\"headRefOid\":\"{head}\",\"comments\":[],\"reviews\":[],",
            "\"files\":[{{\"path\":\"a.txt\",\"additions\":1,\"deletions\":1,",
            "\"changeType\":\"MODIFIED\"}}]}}"
        ),
        head = head
    );
    let comment = |id: &str, who: &str, body: &str, mine: bool, state: &str| {
        format!(
            "{{\"id\":\"{id}\",\"author\":{{\"login\":\"{who}\"}},\"body\":\"{body}\",\
             \"createdAt\":\"2026-11-05T10:00:00Z\",\"viewerDidAuthor\":{mine},\"state\":\"{state}\"}}"
        )
    };
    let thread = |id: &str, line: u32, comment: String| {
        format!(
            "{{\"id\":\"{id}\",\"isResolved\":false,\"isOutdated\":false,\"path\":\"a.txt\",\
             \"line\":{line},\"startLine\":null,\"originalLine\":{line},\"diffSide\":\"RIGHT\",\
             \"comments\":{{\"nodes\":[{comment}]}}}}"
        )
    };
    let threads = format!(
        "{{\"data\":{{\"repository\":{{\"pullRequest\":{{\"id\":\"PR_7\",\"viewerDidAuthor\":false,\
         \"reviews\":{{\"nodes\":[]}},\"reviewThreads\":{{\"nodes\":[{}]}}}}}}}}}}",
        thread("T_ada", 2, comment("C_1", "ada", "Looks off", false, "SUBMITTED"))
    );
    let added = format!(
        "{{\"data\":{{\"addPullRequestReviewThread\":{{\"thread\":{}}}}}}}",
        thread("T_me", 1, comment("C_2", "me", "nit: one", true, "PENDING"))
    );
    let started =
        "{\"data\":{\"addPullRequestReview\":{\"pullRequestReview\":{\"id\":\"PRR_1\"}}}}";
    let submitted = "{\"data\":{\"submitPullRequestReview\":{\"pullRequestReview\":{\"id\":\"PRR_1\",\"state\":\"COMMENTED\"}}}}";
    let gh = stub_bin.path().join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n\
             \x20 *'--input -'*)\n\
             \x20   body=$(cat)\n\
             \x20   printf '%s\\n' \"$body\" >> '{sent}'\n\
             \x20   case \"$body\" in\n\
             \x20     *addPullRequestReviewThread*) printf '%s' '{added}' ;;\n\
             \x20     *submitPullRequestReview*) printf '%s' '{submitted}' ;;\n\
             \x20     *addPullRequestReview*) printf '%s' '{started}' ;;\n\
             \x20     *) exit 1 ;;\n\
             \x20   esac ;;\n\
             \x20 *reviewThreads*) printf '%s' '{threads}' ;;\n\
             \x20 'api graphql'*) printf '%s' '{list}' ;;\n\
             \x20 'pr view 7'*) printf '%s' '{detail}' ;;\n\
             \x20 *) exit 1 ;;\n\
             esac\n",
            sent = sent.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "review-proj");
    tui.send(CMD_U);
    tui.wait_for_text("Add things");
    // The page is in — the Changes tab lists its file — so the review has
    // a head and a base to read the diff between.
    tui.send(SHIFT_RIGHT);
    tui.wait_for_text("a.txt");
    tui.send(CMD_SHIFT_R);
    tui.wait_for_text("Your review");
    tui.wait_for_text("Looks off");
    tui.wait_for("the diff, ada's thread under deux", |s| {
        let text = screen_to_text(s);
        let at = |needle: &str| text.find(needle);
        matches!((at("deux"), at("Looks off")), (Some(a), Some(b)) if a < b)
    });

    // `c` on the first line, a comment, Enter: into a pending review.
    tui.send(b"c");
    tui.wait_for_text("Comment on line 1");
    tui.type_str("nit: one");
    tui.send(ENTER);
    tui.wait_for_text("me · pending");
    tui.wait_for_text("1 pending comment");

    tui.send(CMD_SHIFT_R);
    tui.wait_for_text("reviewed #7");
    tui.wait_for_gone("Your review");

    let bodies = std::fs::read_to_string(&sent).unwrap();
    let bodies: Vec<&str> = bodies.lines().collect();
    assert_eq!(bodies.len(), 3, "{bodies:?}");
    assert!(
        bodies[0].contains("addPullRequestReview(input: {pullRequestId: $pr})")
            && bodies[0].contains("\"pr\":\"PR_7\""),
        "the comment starts the pending review: {}",
        bodies[0]
    );
    assert!(
        bodies[1].contains("addPullRequestReviewThread")
            && bodies[1].contains("\"review\":\"PRR_1\"")
            && bodies[1].contains("\"line\":1")
            && bodies[1].contains("\"body\":\"nit: one\""),
        "the comment, on line 1: {}",
        bodies[1]
    );
    assert!(
        bodies[2].contains("submitPullRequestReview")
            && bodies[2].contains("\"review\":\"PRR_1\"")
            && bodies[2].contains("\"event\":\"COMMENT\""),
        "the review sent with it: {}",
        bodies[2]
    );
}

/// `⌘;` in the PULL REQUESTS MODAL, as Ghostty's kitty protocol spells it
/// (`ESC [59;9u`): the EDIT form comes up on the row and fills from
/// GitHub's one read — and opening it, and leaving, write nothing. A stub
/// `gh` on PATH answers and writes down every call.
#[test]
fn tui_cmd_semicolon_opens_the_edit_form_and_only_reads() {
    let stub_bin = tempfile::tempdir().unwrap();
    let calls = stub_bin.path().join("calls");
    let mut tui = TuiHarness::spawn_with(
        &[(
            "PATH",
            format!(
                "{}:{}",
                stub_bin.path().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )],
        r#", "pr_autofix": "off""#,
    );
    let repo = tui.make_repo("edit-proj");
    let list = concat!(
        "{\"data\":{\"repository\":{\"pullRequests\":{\"nodes\":[",
        "{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",\"title\":\"Add things\",",
        "\"isDraft\":false,\"headRefName\":\"feat\",\"isCrossRepository\":false,",
        "\"headRepositoryOwner\":{\"login\":\"o\"},\"mergeable\":\"MERGEABLE\",",
        "\"viewerDidAuthor\":false,\"headRefOid\":\"abc123\",",
        "\"commits\":{\"nodes\":[{\"commit\":{\"statusCheckRollup\":{\"state\":\"SUCCESS\"}}}]}}",
        "]}}}}"
    );
    let facts = concat!(
        "{\"data\":{\"repository\":{\"pullRequest\":{\"title\":\"Add things\",\"state\":\"OPEN\",",
        "\"isDraft\":false,\"baseRefName\":\"main\",\"baseRefOid\":\"aaa111\",",
        "\"headRefName\":\"feat\",\"headRefOid\":\"abc123\",\"isCrossRepository\":false,",
        "\"author\":{\"login\":\"ada\"},\"baseRef\":{\"name\":\"main\"},\"headRef\":{\"name\":\"feat\"},",
        "\"labels\":{\"nodes\":[{\"name\":\"bug\"}]},\"reviewRequests\":{\"nodes\":[]},",
        "\"assignees\":{\"nodes\":[]}},",
        "\"labels\":{\"nodes\":[{\"name\":\"bug\",\"color\":\"d73a4a\"}]},",
        "\"assignableUsers\":{\"nodes\":[{\"login\":\"ada\"}]}}}}"
    );
    let gh = stub_bin.path().join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{calls}'\ncase \"$*\" in\n\
             \x20 *assignableUsers*) printf '%s' '{facts}' ;;\n\
             \x20 'api graphql'*) printf '%s' '{list}' ;;\n\
             \x20 *) exit 1 ;;\n\
             esac\n",
            calls = calls.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    tui.wait_for_text("to begin");
    add_project(&mut tui, &repo, "edit-proj");
    tui.send(CMD_U);
    tui.wait_for_text("Add things");
    tui.send(CMD_SEMICOLON);
    tui.wait_for_text("Edit #7");
    // GitHub's answer is in: the form says where it stands and what it has.
    tui.wait_for_text("feat → main · open");
    tui.wait_for_text("Labels    bug");
    tui.send(ESC);
    tui.wait_for_gone("Edit #7");

    let calls = std::fs::read_to_string(&calls).unwrap_or_default();
    let writes: Vec<&str> = calls
        .lines()
        .filter(|c| {
            ["pr edit", "pr close", "pr reopen", "pr ready", "api -X"]
                .iter()
                .any(|w| c.starts_with(w))
        })
        .collect();
    assert!(writes.is_empty(), "nothing written: {writes:?}");
}

/// `⌘U` in an agent's locked pane, as Ghostty's kitty protocol spells it:
/// the preset list comes up over the running session, Enter pastes the
/// preset into the agent's prompt as a bracketed paste, and the keys are
/// still in the pane — no new box, no clipboard, nothing sent. The stand-in
/// agent asks for bracketed paste as Claude Code does, and the terminal's
/// echo shows the markers (`^[[200~`) it was handed.
#[test]
fn tui_cmd_u_pastes_a_preset_into_the_running_agent() {
    let scripts = tempfile::tempdir().unwrap();
    let agent = scripts.path().join("agent.sh");
    std::fs::write(
        &agent,
        "#!/bin/sh\nprintf '\\033[?2004hagent-ready\\n'\nexec cat\n",
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let mut tui =
        TuiHarness::spawn_with_env(&[(orion_core::env::AGENT_CMD, agent.display().to_string())]);
    std::fs::write(
        tui.data_dir.join(orion_core::paths::PRESETS_FILE_NAME),
        r#"[{"name": "reviewer", "kind": "claude", "prefix": "Be strict."}]"#,
    )
    .unwrap();
    let repo = tui.make_repo("alpha");
    add_project(&mut tui, &repo, "alpha");
    start_session(&mut tui, "go");
    // A session still starting up refuses the paste: wait for its prompt.
    tui.wait_for_text("agent-ready");

    tui.send(CMD_U);
    tui.wait_for_text("Insert preset");
    tui.wait_for_text("reviewer");
    tui.send(ENTER);
    tui.wait_for_gone("Insert preset");
    tui.wait_for_text("^[[200~Be strict.");
    tui.wait_for_text(FOOTER_TERMINAL_LOCKED);

    // The next key is the agent's, where the task goes.
    tui.type_str("the task");
    tui.wait_for_text("the task");

    // Over a full-screen session too, and Esc lands back in it.
    tui.send(CMD_F);
    tui.wait_for_text("normal size");
    tui.send(CMD_U);
    tui.wait_for_text("Insert preset");
    tui.send(ESC);
    tui.wait_for_gone("Insert preset");
    tui.wait_for_text("normal size");
    tui.type_str("still here");
    tui.wait_for_text("still here");
}
