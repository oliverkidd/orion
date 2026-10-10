//! Embedded editor modal: a local PTY child (micro, Edit, fresh, vim, …)
//! rendered inside the TUI.
//!
//! Unlike agent/terminal sessions (daemon-owned PTYs reached over IPC), the
//! editor is spawned in-process: it's a short-lived affordance of the
//! file overlays, needs no persistence or reattach, and dies with the
//! client. Output flows reader thread → mpsc → the main loop, which feeds
//! the vt100 parser here (the daemon's `PtySession` shape, minus the ring
//! buffer and broadcast) and answers the terminal queries in it the way
//! the daemon does for a session (`orion_core::kitty`): Microsoft Edit
//! waits on a DA1 reply and a cursor report before it draws anything, and
//! fresh pushes kitty keyboard flags its keys are then encoded for.

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use ratatui::layout::Rect;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

/// Reader-thread → main-loop messages. `generation` stamps which spawn they
/// belong to, so bytes from a closed editor can't bleed into a new one.
#[derive(Debug)]
pub enum VimEvent {
    Output { generation: u64, data: Vec<u8> },
    Exited { generation: u64 },
}

pub struct VimTerm {
    pub generation: u64,
    pub parser: vt100::Parser,
    /// Size the parser (and PTY) currently uses.
    pub cols: u16,
    pub rows: u16,
    /// "path:line" for the modal title.
    pub title: String,
    /// The checkout the editor runs in.
    pub cwd: PathBuf,
    /// The file and line it was opened on (empty / 0 for a bare command).
    pub file: String,
    pub line: u64,
    /// Rendered inside the open overlay's preview pane — the TREE BROWSER's,
    /// or the FILE TABS' body — instead of the centered modal (set by that
    /// overlay's Enter).
    pub embedded: bool,
    /// Inner rect from the last draw; `sync_vim_size` resizes to it.
    pub area: Rect,
    /// Which editor runs here, for its keys hint (`Kind::Other` for a bare
    /// command).
    pub kind: crate::editor::Kind,
    /// The editor quits on Ctrl+Q itself, asking to save first (micro,
    /// Edit, fresh), so Ctrl+Q goes to it rather than force-closing the
    /// modal.
    pub quits_itself: bool,
    /// The modal runs `claude auth …` for a CLAUDE ACCOUNT rather than an
    /// editor: there is no file to hand to an app, and its exit re-reads
    /// who the accounts are signed in as (`claude_accounts`).
    pub account_auth: bool,
    /// The modal runs an installer for this program rather than an editor
    /// (`install`): its exit keeps the modal up, saying whether the
    /// program is on PATH now, and closing it says so where the user is.
    pub install: Option<String>,
    /// The installer exited, and whether its program is on PATH since —
    /// the modal waits on Enter to close.
    pub finished: Option<bool>,
    /// Terminal queries in the output, answered here, and the kitty
    /// keyboard flags the child has pushed.
    queries: orion_core::kitty::KittyScanner,
    /// Keys to type once the editor has drawn ([`Kind::startup_keys`]).
    ///
    /// [`Kind::startup_keys`]: crate::editor::Kind::startup_keys
    startup_keys: Option<&'static [u8]>,
    /// A mouse press inside the editor is held: its drag and release are
    /// the editor's wherever the pointer goes.
    pub mouse_held: bool,
    /// The last key was the Enter that ran a find, or one that stepped to
    /// a match since: the next Enter steps too, where the editor's own
    /// would be a new line (`Kind::find_step`).
    pub find_stepping: bool,
    /// Output chunks seen, and whether one asked something that was
    /// answered: the first quiet chunk after that is its first screen.
    chunks: u64,
    answered: bool,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl VimTerm {
    /// Spawn `editor` on `file` at `line` in the checkout, told the line
    /// its own way — micro and fresh off orion's own config (`editor`).
    /// `Err` is a user-facing flash message.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_editor(
        editor: &str,
        root: &Path,
        file: &str,
        line: u64,
        cols: u16,
        rows: u16,
        generation: u64,
        tx: UnboundedSender<VimEvent>,
    ) -> Result<Self, String> {
        let title = format!("{file}:{line}");
        let kind = crate::editor::Kind::of(editor);
        let config_root = crate::editor::config_root();
        crate::editor::ensure_config(editor, &config_root).map_err(|e| {
            format!(
                "couldn't set up {}'s config in {}: {e}",
                crate::editor::program_name(editor),
                config_root.display()
            )
        })?;
        let mut term = Self::spawn_cmd(
            editor,
            &crate::editor::editor_args(editor, &config_root, file, line),
            root,
            title,
            cols,
            rows,
            generation,
            tx,
        )?;
        term.kind = kind;
        term.quits_itself = kind.quits_on_ctrl_q();
        term.startup_keys = kind.startup_keys();
        term.file = file.to_string();
        term.line = line;
        Ok(term)
    }

    /// Editor-agnostic spawn (tests use a shell here).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_cmd(
        program: &str,
        args: &[String],
        cwd: &Path,
        title: String,
        cols: u16,
        rows: u16,
        generation: u64,
        tx: UnboundedSender<VimEvent>,
    ) -> Result<Self, String> {
        Self::spawn_cmd_env(program, args, &[], cwd, title, cols, rows, generation, tx)
    }

    /// [`Self::spawn_cmd`] with `env` set on the child, on top of this
    /// process's own — a Claude account's `CLAUDE_CONFIG_DIR` for its
    /// `claude auth login`.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_cmd_env(
        program: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: &Path,
        title: String,
        cols: u16,
        rows: u16,
        generation: u64,
        tx: UnboundedSender<VimEvent>,
    ) -> Result<Self, String> {
        let cols = cols.max(2);
        let rows = rows.max(2);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("openpty failed: {e}"))?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        // The modal is drawn on orion's grid, the same truecolor terminal
        // every session pane is (see `orion_core::env::PANE_TERM`).
        cmd.env("TERM", orion_core::env::PANE_TERM);
        cmd.env("COLORTERM", orion_core::env::PANE_COLORTERM);
        for (name, value) in env {
            cmd.env(name, value);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("failed to launch {program}: {e}"))?;
        drop(pair.slave);

        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("pty reader: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("pty writer: {e}"))?;

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        if tx.send(VimEvent::Output { generation, data }).is_err() {
                            break; // main loop gone
                        }
                    }
                }
            }
            let _ = child.wait(); // reap
            let _ = tx.send(VimEvent::Exited { generation });
        });

        Ok(Self {
            generation,
            parser: vt100::Parser::new(rows, cols, 0),
            cols,
            rows,
            title,
            cwd: cwd.to_path_buf(),
            file: String::new(),
            line: 0,
            embedded: false,
            area: Rect::default(),
            kind: crate::editor::Kind::Other,
            quits_itself: false,
            account_auth: false,
            install: None,
            finished: None,
            queries: orion_core::kitty::KittyScanner::new(),
            startup_keys: None,
            mouse_held: false,
            find_stepping: false,
            chunks: 0,
            answered: false,
            master: pair.master,
            writer,
            killer,
        })
    }

    /// Feed reader-thread output into the emulator, answering the terminal
    /// queries in it — a cursor report off the screen as it stood just
    /// past its query — and typing the startup keys once the editor has
    /// drawn.
    pub fn process(&mut self, data: &[u8]) {
        use orion_core::kitty::Reply;
        let actions = self.queries.feed(data);
        let asked = !actions.replies.is_empty();
        let mut reply = Vec::new();
        let mut fed = 0;
        for answer in actions.replies {
            match answer {
                Reply::Bytes(bytes) => reply.extend_from_slice(&bytes),
                Reply::CursorPosition { at } => {
                    self.parser.process(&data[fed..at]);
                    fed = at;
                    let (row, col) = self.parser.screen().cursor_position();
                    reply.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
                }
            }
        }
        self.parser.process(&data[fed..]);
        if !reply.is_empty() {
            self.input(&reply);
        }
        self.chunks += 1;
        if asked {
            self.answered = true;
        } else if self.answered || self.chunks > 1 {
            if let Some(keys) = self.startup_keys.take() {
                self.input(keys);
            }
        }
    }

    /// The kitty keyboard flags the editor has pushed (0: legacy keys).
    pub fn kitty_flags(&self) -> u8 {
        self.queries.flags()
    }

    /// The editor's find prompt is on its bottom row
    /// ([`Kind::find_prompt`]), waiting on the Enter that runs the find.
    ///
    /// [`Kind::find_prompt`]: crate::editor::Kind::find_prompt
    pub fn find_prompt_up(&self) -> bool {
        let Some(prompt) = self.kind.find_prompt() else {
            return false;
        };
        let bottom = self.parser.screen().rows(0, self.cols).last();
        bottom.is_some_and(|row| row.starts_with(prompt))
    }

    /// The mouse protocol the editor asked for, and whether in SGR
    /// coordinates.
    pub fn mouse_mode(&self) -> (vt100::MouseProtocolMode, bool) {
        let screen = self.parser.screen();
        (
            screen.mouse_protocol_mode(),
            screen.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr,
        )
    }

    /// Encoded keystrokes (and bracketed pastes) go straight to the child.
    pub fn input(&mut self, data: &[u8]) {
        // A write error means the child died; the Exited event closes us.
        let _ = self
            .writer
            .write_all(data)
            .and_then(|_| self.writer.flush());
    }

    /// Keep the PTY and parser sized to the drawn modal.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        if (self.cols, self.rows) == (cols, rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.parser.screen_mut().set_size(rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Force-close (the `Ctrl+\` hatch, and Ctrl+Q for an editor that
    /// doesn't quit on it); the reader thread reaps the child.
    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn recv_until(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<VimEvent>,
        term: &mut VimTerm,
        mut done: impl FnMut(&VimTerm, &VimEvent) -> bool,
    ) {
        let ok = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(ev) = rx.recv().await {
                if let VimEvent::Output { data, .. } = &ev {
                    term.process(data);
                }
                if done(term, &ev) {
                    return;
                }
            }
            panic!("event channel closed early");
        })
        .await;
        assert!(
            ok.is_ok(),
            "timed out; screen:\n{}",
            term.parser.screen().contents()
        );
    }

    #[tokio::test]
    async fn output_reaches_parser_and_kill_exits() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut term = VimTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), "printf 'VIM_MODAL_TEST'; sleep 30".into()],
            dir.path(),
            "test".into(),
            80,
            24,
            1,
            tx,
        )
        .unwrap();

        recv_until(&mut rx, &mut term, |t, _| {
            t.parser.screen().contents().contains("VIM_MODAL_TEST")
        })
        .await;

        term.kill();
        recv_until(&mut rx, &mut term, |_, ev| {
            matches!(ev, VimEvent::Exited { generation: 1 })
        })
        .await;
    }

    #[tokio::test]
    async fn env_reaches_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut term = VimTerm::spawn_cmd_env(
            "/bin/sh",
            &[
                "-c".into(),
                "printf \"DIR:$CLAUDE_CONFIG_DIR\"; sleep 30".into(),
            ],
            &[("CLAUDE_CONFIG_DIR".into(), "/tmp/claude-two".into())],
            dir.path(),
            "test".into(),
            80,
            24,
            5,
            tx,
        )
        .unwrap();
        recv_until(&mut rx, &mut term, |t, _| {
            t.parser.screen().contents().contains("DIR:/tmp/claude-two")
        })
        .await;
        term.kill();
    }

    #[tokio::test]
    async fn input_reaches_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut term = VimTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), "read line; printf \"GOT:$line\"".into()],
            dir.path(),
            "test".into(),
            80,
            24,
            7,
            tx,
        )
        .unwrap();

        term.input(b"hello\r");
        recv_until(&mut rx, &mut term, |t, _| {
            t.parser.screen().contents().contains("GOT:hello")
        })
        .await;
        // The script ends after one line: the exit must be reported with the
        // spawn's generation stamp.
        recv_until(&mut rx, &mut term, |_, ev| {
            matches!(ev, VimEvent::Exited { generation: 7 })
        })
        .await;
    }

    /// Terminal queries are answered from the modal's own screen: DA1, a
    /// kitty flags query, and a cursor report of where the child's output
    /// had put the cursor when it asked — what Microsoft Edit waits on
    /// before it draws. Pushed kitty flags are tracked for the keys.
    #[tokio::test]
    async fn terminal_queries_are_answered_from_the_modals_screen() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let script = "stty raw -echo; printf 'abc\\033[6n\\033[c\\033[>5u'; \
                      dd bs=1 count=11 2>/dev/null | od -An -c | tr -d ' \\n'; sleep 30";
        let mut term = VimTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), script.into()],
            dir.path(),
            "test".into(),
            80,
            24,
            9,
            tx,
        )
        .unwrap();
        recv_until(&mut rx, &mut term, |t, _| {
            t.parser.screen().contents().contains("[1;4R")
        })
        .await;
        let shown = term.parser.screen().contents();
        assert!(shown.contains("033[1;4R033[?6c"), "{shown}");
        assert_eq!(term.kitty_flags(), 5, "fresh's push");
        term.kill();
    }

    /// Microsoft Edit's word wrap goes on with the keys it is typed once
    /// its first screen is up: after the queries it waited on, never
    /// mixed into their answers.
    #[tokio::test]
    async fn startup_keys_follow_the_first_screen() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let script = "stty raw -echo; printf '\\033[c'; dd bs=1 count=5 2>/dev/null >/dev/null; \
                      printf 'SCREEN'; dd bs=1 count=2 2>/dev/null | od -An -c | tr -d ' \\n'; \
                      sleep 30";
        let mut term = VimTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), script.into()],
            dir.path(),
            "test".into(),
            80,
            24,
            4,
            tx,
        )
        .unwrap();
        term.startup_keys = crate::editor::Kind::Edit.startup_keys();
        recv_until(&mut rx, &mut term, |t, _| {
            t.parser.screen().contents().contains("SCREEN033z")
        })
        .await;
        term.kill();
    }

    /// The real micro, where installed: it opens on the asked line off
    /// orion's config dir, and quits on its own Ctrl+Q.
    #[tokio::test]
    async fn micro_opens_on_the_line_and_quits_on_ctrl_q() {
        if !crate::config::program_installed("micro") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        crate::editor::ensure_config("micro", dir.path()).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut term = VimTerm::spawn_cmd(
            "micro",
            &crate::editor::editor_args("micro", dir.path(), "a.txt", 3),
            dir.path(),
            "test".into(),
            80,
            24,
            3,
            tx,
        )
        .unwrap();
        recv_until(&mut rx, &mut term, |t, _| {
            t.parser.screen().contents().contains("(3,1)")
        })
        .await;
        term.input(&[0x11]);
        recv_until(&mut rx, &mut term, |_, ev| {
            matches!(ev, VimEvent::Exited { generation: 3 })
        })
        .await;
    }

    #[tokio::test]
    async fn spawn_failure_is_a_message_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = VimTerm::spawn_cmd(
            "/nonexistent-editor-binary",
            &[],
            dir.path(),
            "test".into(),
            80,
            24,
            1,
            tx,
        )
        .map(|_| ())
        .unwrap_err();
        assert!(err.contains("failed to launch"), "{err}");
    }
}
