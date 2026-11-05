//! Embedded editor modal: a local PTY child (fresh, or the command
//! `ORION_EDITOR` names) rendered inside the TUI.
//!
//! Unlike agent/terminal sessions (daemon-owned PTYs reached over IPC), the
//! editor is spawned in-process: it's a short-lived affordance of the
//! file overlays, needs no persistence or reattach, and dies with the
//! client. Output flows reader thread → mpsc → the main loop, which feeds
//! the vt100 parser here (the daemon's `PtySession` shape, minus the ring
//! buffer and broadcast) and answers the terminal queries in it the way
//! the daemon does for a session (`orion_core::kitty`): an editor may wait
//! on a DA1 reply and a cursor report before it draws anything, and fresh
//! pushes kitty keyboard flags its keys are then encoded for.

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use ratatui::layout::Rect;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

/// How many rows up from the bottom the editor's status bar may sit: its
/// own row, or above fresh's find prompt and that prompt's options.
const STATUS_BAR_REACH: usize = 3;

/// Reader-thread → main-loop messages. `generation` stamps which spawn they
/// belong to, so bytes from a closed editor can't bleed into a new one.
#[derive(Debug)]
pub enum EditorEvent {
    Output { generation: u64, data: Vec<u8> },
    Exited { generation: u64 },
}

pub struct EditorTerm {
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
    /// Inner rect from the last draw; `sync_editor_size` resizes to it.
    pub area: Rect,
    /// Which editor runs here, for its keys hint (`Kind::Other` for a bare
    /// command).
    pub kind: crate::editor::Kind,
    /// The editor quits on Ctrl+Q itself, asking to save first (fresh), so
    /// Ctrl+Q goes to it rather than force-closing the modal.
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
    /// A mouse press inside the editor is held: its drag and release are
    /// the editor's wherever the pointer goes.
    pub mouse_held: bool,
    /// The last key was the Enter that ran a find, or one that stepped to
    /// a match since: the next Enter steps too, where the editor's own
    /// would be a new line (`Kind::find_step`).
    pub find_stepping: bool,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl EditorTerm {
    /// Spawn `editor` on `file` at `line` in the checkout, told the line
    /// its own way — fresh off orion's own config (`editor`).
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
        tx: UnboundedSender<EditorEvent>,
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
        tx: UnboundedSender<EditorEvent>,
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
        tx: UnboundedSender<EditorEvent>,
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
                        if tx.send(EditorEvent::Output { generation, data }).is_err() {
                            break; // main loop gone
                        }
                    }
                }
            }
            let _ = child.wait(); // reap
            let _ = tx.send(EditorEvent::Exited { generation });
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
            mouse_held: false,
            find_stepping: false,
            master: pair.master,
            writer,
            killer,
        })
    }

    /// Feed reader-thread output into the emulator, answering the terminal
    /// queries in it — a cursor report off the screen as it stood just
    /// past its query.
    pub fn process(&mut self, data: &[u8]) {
        use orion_core::kitty::Reply;
        let actions = self.queries.feed(data);
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
        self.bottom_rows_up(1)
            .next()
            .is_some_and(|row| row.starts_with(prompt))
    }

    /// The screen's last `n` rows as text, the bottom one first.
    fn bottom_rows_up(&self, n: usize) -> impl Iterator<Item = String> {
        let skip = usize::from(self.parser.screen().size().0).saturating_sub(n);
        let rows: Vec<String> = self.parser.screen().rows(0, self.cols).skip(skip).collect();
        rows.into_iter().rev()
    }

    /// The line the editor's cursor is on, read off its status bar
    /// ([`Kind::cursor_line`]): the bottom row, or a few above it while a
    /// prompt (fresh's `Search:` and its options) sits under the bar. None
    /// until the bar is drawn, and for an editor without one orion reads.
    ///
    /// [`Kind::cursor_line`]: crate::editor::Kind::cursor_line
    pub fn cursor_line(&self) -> Option<u64> {
        self.bottom_rows_up(STATUS_BAR_REACH)
            .find_map(|row| self.kind.cursor_line(&row))
    }

    /// The file as the checkout names it: its path under [`Self::cwd`] —
    /// the FILE TABS open absolute paths — else as it was opened.
    pub fn rel_file(&self) -> String {
        match Path::new(&self.file).strip_prefix(&self.cwd) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => self.file.clone(),
        }
    }

    /// The line the file is at now, for `⌘O` and `⌘⇧I`'s `path:line`:
    /// fresh's cursor line — line 1 when its status bar can't be read,
    /// rather than a guess — and for any other editor the line it was
    /// opened on, the one line orion knows.
    pub fn line_now(&self) -> u64 {
        match self.kind {
            crate::editor::Kind::Fresh => self.cursor_line().unwrap_or(1),
            crate::editor::Kind::Other => self.line.max(1),
        }
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
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<EditorEvent>,
        term: &mut EditorTerm,
        mut done: impl FnMut(&EditorTerm, &EditorEvent) -> bool,
    ) {
        let ok = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(ev) = rx.recv().await {
                if let EditorEvent::Output { data, .. } = &ev {
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
        let mut term = EditorTerm::spawn_cmd(
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
            matches!(ev, EditorEvent::Exited { generation: 1 })
        })
        .await;
    }

    #[tokio::test]
    async fn env_reaches_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut term = EditorTerm::spawn_cmd_env(
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
        let mut term = EditorTerm::spawn_cmd(
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
            matches!(ev, EditorEvent::Exited { generation: 7 })
        })
        .await;
    }

    /// Terminal queries are answered from the modal's own screen: DA1, a
    /// kitty flags query, and a cursor report of where the child's output
    /// had put the cursor when it asked — what an editor may wait on
    /// before it draws. Pushed kitty flags are tracked for the keys.
    #[tokio::test]
    async fn terminal_queries_are_answered_from_the_modals_screen() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let script = "stty raw -echo; printf 'abc\\033[6n\\033[c\\033[>5u'; \
                      dd bs=1 count=11 2>/dev/null | od -An -c | tr -d ' \\n'; sleep 30";
        let mut term = EditorTerm::spawn_cmd(
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

    /// The line now is fresh's cursor line, off the status bar at the
    /// bottom or a few rows up under a prompt; line 1 while the bar can't
    /// be read; and for any other editor the line it was opened on.
    #[tokio::test]
    async fn the_line_now_is_read_off_the_status_bar() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut term = EditorTerm::spawn_cmd(
            "/bin/sh",
            &["-c".into(), "sleep 30".into()],
            dir.path(),
            "test".into(),
            40,
            10,
            3,
            tx,
        )
        .unwrap();
        term.line = 5;
        assert_eq!(term.line_now(), 5, "not fresh: the line it opened on");
        term.kind = crate::editor::Kind::Fresh;
        assert_eq!(term.line_now(), 1, "no bar drawn: line 1");
        term.process(b"\x1b[10;1H Ln 42, Col 3   Text");
        assert_eq!(term.line_now(), 42);
        term.process(b"\x1b[8;1H Ln 7, Col 1\x1b[9;1H\x1b[2K[ ] Regex\x1b[10;1H\x1b[2KSearch:");
        assert_eq!(term.cursor_line(), Some(7), "the bar above a prompt");
        term.kill();
    }

    #[tokio::test]
    async fn spawn_failure_is_a_message_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let err = EditorTerm::spawn_cmd(
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
