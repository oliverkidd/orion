//! The git that orion's TUI runs itself, as processes: a read that only
//! looks, a write or a call to a remote that runs DETACHED, and git's
//! complaint as the one line a FLASH or a modal shows. The BRANCH
//! SWITCHER, a PULL and a PUSH (`crate::git_sync`) and the pull request
//! forms' git (`crate::pr_actions`) all run through here.
//!
//! Git that writes and git that talks to a remote run in a session of
//! their own with stdin closed. The TUI owns a terminal, and an `ssh`
//! asking for a passphrase or a host key would otherwise open `/dev/tty`
//! and paint over the frame; detached, it fails instead. (A pinentry that
//! opens `$GPG_TTY` by path is beyond its reach.) A call to a remote is
//! also bounded — a stalled remote would otherwise hold it forever — and
//! stopped once the TUI is leaving, whose runtime's shutdown waits on it.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::git_diff::git_command;

/// How long a fetch may run. Generous — a fetch writes packs,
/// and one cut short starts over on the next open — but bounded, since a
/// stalled remote would otherwise hold it forever.
pub(crate) const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Between the SIGTERM that lets git remove its lock and temporary pack
/// files and the SIGKILL for a fetch or a push that lingers.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(2);
/// How often a running remote call is checked on, and a stopped one.
const POLL: Duration = Duration::from_millis(50);
const STOP_POLL: Duration = Duration::from_millis(20);
/// How long a finished call's stderr is waited for: anything git left
/// behind holding the pipe open (an `ssh` master gone to the background)
/// must not hold the answer.
const STDERR_WAIT: Duration = Duration::from_secs(1);
/// How much of a call's stderr is kept for its one-line complaint.
const SAID_KEPT: usize = 256 * 1024;
/// How long a push may run: a pre-push hook can run a test suite, and
/// the person who pressed the key is watching the footer say so.
pub(crate) const PUSH_TIMEOUT: Duration = Duration::from_secs(300);

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

/// git's complaint as one line: the first `error:` or `fatal:` it printed,
/// without the prefix, else the first thing it said at all.
fn git_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let line = lines
        .iter()
        .find(|l| l.starts_with("error: ") || l.starts_with("fatal: "))
        .or(lines.first())
        .copied()
        .unwrap_or("git failed");
    line.strip_prefix("error: ")
        .or_else(|| line.strip_prefix("fatal: "))
        .unwrap_or(line)
        .to_string()
}

/// Read-only `git -C root <args>`: stdout, or git's one-line complaint.
pub(crate) fn read(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = git_command(root);
    cmd.args(args).stdin(Stdio::null());
    finish(cmd)
}

/// `git -C root <args>` in a session of its own with stdin closed, for the
/// calls that write or reach a remote — see the module docs for why none
/// of them may find the TUI's terminal.
pub(crate) fn detached(root: &Path, args: &[&str]) -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = git_command(root);
    cmd.args(args)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    // SAFETY: setsid is async-signal-safe and touches nothing but the child.
    unsafe {
        cmd.pre_exec(crate::ipc::own_session);
    }
    cmd
}

/// [`detached`], run to completion: stdout on success, git's one-line
/// complaint otherwise.
pub(crate) fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    finish(detached(root, args))
}

/// `cmd` run to completion: stdout on success, git's one-line complaint
/// otherwise.
fn finish(mut cmd: Command) -> Result<String, String> {
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(git_error(&String::from_utf8_lossy(&out.stderr)))
    }
}

/// The branch HEAD is on; None when it is detached.
pub(crate) fn head_branch(root: &Path) -> Option<String> {
    read(root, &["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `git -C root <args>` for a call that reaches a remote — a fetch, a push
/// — stopped past `budget`, or once `quit` is raised:
/// `Err` is git's one-line complaint ([`remote_error`]), or why it was
/// stopped.
pub(crate) fn remote_git(
    root: &Path,
    args: &[&str],
    budget: Duration,
    quit: &AtomicBool,
) -> Result<(), String> {
    watched(root, args, budget, quit, None)
}

/// [`remote_git`] with everything git and its hooks say landing in `log`
/// as it is said, the command first — for a screen that shows the wait.
pub(crate) fn logged_git(
    root: &Path,
    args: &[&str],
    budget: Duration,
    quit: &AtomicBool,
    log: &GitLog,
) -> Result<(), String> {
    log.say(LogKind::Command, format!("$ git {}", args.join(" ")));
    watched(root, args, budget, quit, Some(log))
}

fn watched(
    root: &Path,
    args: &[&str],
    budget: Duration,
    quit: &AtomicBool,
    log: Option<&GitLog>,
) -> Result<(), String> {
    let mut child = detached(root, args)
        .stdout(if log.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    // Each pipe drained on a thread of its own, so a remote that says a lot
    // can't fill one and stall git while this loop only polls.
    let said_rx = child
        .stderr
        .take()
        .map(|pipe| drained(pipe, log, SAID_KEPT));
    let out_rx = child.stdout.take().map(|pipe| drained(pipe, log, 0));
    let deadline = Instant::now() + budget;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) if quit.load(Ordering::Relaxed) => break Err("cancelled"),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) => break Err("timed out"),
            Err(_) => break Err("failed"),
        }
    };
    if ended.is_err() {
        stop(&mut child);
    }
    let said = said_rx
        .and_then(|rx| rx.recv_timeout(STDERR_WAIT).ok())
        .unwrap_or_default();
    if let Some(rx) = out_rx {
        let _ = rx.recv_timeout(STDERR_WAIT);
    }
    match ended {
        Ok(true) => Ok(()),
        Ok(false) => Err(remote_error(&said)),
        Err(why) => Err(format!("{} {why}", args.first().copied().unwrap_or("git"))),
    }
}

/// [`drain`] on a thread of its own, its answer on the channel once `pipe`
/// closes.
fn drained(
    pipe: impl std::io::Read + Send + 'static,
    log: Option<&GitLog>,
    keep: usize,
) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let log = log.cloned();
    std::thread::spawn(move || {
        let _ = tx.send(drain(pipe, log, keep));
    });
    rx
}

/// Read `pipe` until it closes, feeding `log` as it comes. Returns the last
/// `keep` or so bytes: however much a hook prints, only git's complaint at
/// the end is wanted.
fn drain(mut pipe: impl std::io::Read, log: Option<GitLog>, keep: usize) -> String {
    let mut tail = Vec::new();
    let mut feed = log.map(LogFeed::new);
    let mut chunk = [0u8; 4096];
    while let Ok(n) = pipe.read(&mut chunk) {
        if n == 0 {
            break;
        }
        if keep > 0 {
            tail.extend_from_slice(&chunk[..n]);
            if tail.len() > 2 * keep {
                tail.drain(..tail.len() - keep);
            }
        }
        if let Some(feed) = &mut feed {
            feed.push(&chunk[..n]);
        }
    }
    if let Some(feed) = &mut feed {
        feed.end();
    }
    String::from_utf8_lossy(&tail).into_owned()
}

/// What a line of a [`GitLog`] is, which sets how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// The command about to run.
    Command,
    /// What git or a hook said.
    Output,
    /// orion saying something went well.
    Good,
    /// orion saying where things stand.
    Note,
    /// orion saying it stopped, and why.
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub kind: LogKind,
    pub text: String,
}

/// The newest lines a [`GitLog`] keeps.
const LOG_LINES: usize = 2000;

/// git's output on its way to the screen: written off the loop as it
/// arrives, read by each draw, shared by every clone.
#[derive(Debug, Clone, Default)]
pub struct GitLog(Arc<Mutex<Vec<LogLine>>>);

impl GitLog {
    pub fn say(&self, kind: LogKind, text: impl Into<String>) {
        let mut lines = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if lines.len() >= LOG_LINES {
            lines.remove(0);
        }
        lines.push(LogLine {
            kind,
            text: text.into(),
        });
    }

    /// The last line said again: a progress bar's next frame.
    fn say_over(&self, text: String) {
        let mut lines = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match lines.last_mut() {
            Some(last) => last.text = text,
            None => lines.push(LogLine {
                kind: LogKind::Output,
                text,
            }),
        }
    }

    pub fn lines(&self) -> Vec<LogLine> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Bytes off a pipe cut into a [`GitLog`]'s lines. A line ended by a bare
/// `\r` is one a program means to draw over, so the next takes its place:
/// `git push --progress` reads as one line that counts up.
struct LogFeed {
    log: GitLog,
    line: Vec<u8>,
    /// The last line ended in `\r`.
    over: bool,
}

impl LogFeed {
    fn new(log: GitLog) -> Self {
        Self {
            log,
            line: Vec::new(),
            over: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match b {
                b'\n' if self.line.is_empty() && self.over => self.over = false,
                b'\n' => {
                    self.flush();
                    self.over = false;
                }
                b'\r' => {
                    self.flush();
                    self.over = true;
                }
                _ => self.line.push(b),
            }
        }
    }

    fn flush(&mut self) {
        let text = plain(&String::from_utf8_lossy(&self.line));
        self.line.clear();
        if self.over {
            self.log.say_over(text);
        } else {
            self.log.say(LogKind::Output, text);
        }
    }

    fn end(&mut self) {
        if !self.line.is_empty() {
            self.flush();
        }
    }
}

/// `line` as a pane can draw it: a hook's colors and other control
/// sequences gone, tabs spread.
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// [`git_error`], but a refused push says why rather than only that it
/// was: git ends one with `failed to push some refs`, after the line that
/// matters — a pre-push hook's own last word, or the `! [rejected]` row.
fn remote_error(stderr: &str) -> String {
    let line = git_error(stderr);
    if !line.starts_with("failed to push") {
        return line;
    }
    let why = stderr
        .lines()
        .map(str::trim)
        .take_while(|l| !l.starts_with("error: "))
        .filter(|l| !l.is_empty() && !l.starts_with("To ") && !l.starts_with("hint:"))
        .last();
    match why {
        Some(why) => format!(
            "rejected: {}",
            why.split_whitespace().collect::<Vec<_>>().join(" ")
        ),
        None => line,
    }
}

/// End a fetch or a push and the `ssh` it may have started, which share its session
/// and so its process group: SIGTERM first, so git cleans up its lock and
/// temporary pack files, and SIGKILL only for one that lingers.
fn stop(child: &mut std::process::Child) {
    let group = -(child.id() as i32);
    // SAFETY: plain syscalls on a process group this process started and
    // has not yet reaped the leader of, so the id cannot have been reused.
    unsafe { kill(group, SIGTERM) };
    let grace = Instant::now() + STOP_GRACE;
    while Instant::now() < grace {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(STOP_POLL);
    }
    unsafe { kill(group, SIGKILL) };
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fed(chunks: &[&[u8]]) -> Vec<String> {
        let log = GitLog::default();
        let mut feed = LogFeed::new(log.clone());
        for chunk in chunks {
            feed.push(chunk);
        }
        feed.end();
        log.lines().into_iter().map(|l| l.text).collect()
    }

    /// Lines survive being cut across reads; a line ended by `\r` is drawn
    /// over by the next, so progress reads as one line; CRLF is one ending.
    #[test]
    fn a_log_keeps_lines_and_draws_progress_over_itself() {
        assert_eq!(
            fed(&[b"one\ntw", b"o\n", b"three"]),
            ["one", "two", "three"]
        );
        assert_eq!(
            fed(&[
                b"Writing: 10%\rWriting: 60%\r",
                b"Writing: 100%, done.\nTo origin\n"
            ]),
            ["Writing: 100%, done.", "To origin"]
        );
        assert_eq!(fed(&[b"one\r\ntwo\r\n"]), ["one", "two"]);
    }

    /// A hook's colors don't reach the pane.
    #[test]
    fn a_log_line_is_plain_text() {
        assert_eq!(
            plain("\u{1b}[1;31merror\u{1b}[0m:\tbad\u{7}"),
            "error:    bad"
        );
    }

    /// What git and its hooks say lands as it is said, the command first,
    /// and a failure still answers with the one line.
    #[test]
    fn logged_git_reports_output_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        let log = GitLog::default();
        let quit = AtomicBool::new(false);
        let budget = Duration::from_secs(30);
        logged_git(dir.path(), &["init", "-b", "main"], budget, &quit, &log).unwrap();
        let lines = log.lines();
        assert_eq!(lines[0].text, "$ git init -b main");
        assert_eq!(lines[0].kind, LogKind::Command);
        assert!(
            lines.iter().any(|l| l.text.contains("Initialized")),
            "{lines:?}"
        );
        let why = logged_git(dir.path(), &["rev-parse", "nope"], budget, &quit, &log).unwrap_err();
        assert!(!why.is_empty());
    }

    /// With no log there is no stdout to wait for: a call answers as soon
    /// as git ends, not after the wait for a pipe that never was.
    #[test]
    fn remote_git_answers_as_soon_as_git_ends() {
        let dir = tempfile::tempdir().unwrap();
        let quit = AtomicBool::new(false);
        let at = Instant::now();
        remote_git(dir.path(), &["--version"], Duration::from_secs(30), &quit).unwrap();
        assert!(at.elapsed() < STDERR_WAIT, "{:?}", at.elapsed());
    }

    /// A pipe that says a lot keeps its tail, and only that.
    #[test]
    fn drain_keeps_only_the_tail() {
        let said = vec![b'x'; 10_000];
        let kept = drain(&said[..], None, 100);
        assert!(kept.len() <= 200 + 4096, "{}", kept.len());
        assert!(drain(&said[..], None, 0).is_empty());
    }

    #[test]
    fn git_errors_read_as_the_error_line_without_its_prefix() {
        assert_eq!(
            git_error("error: pathspec 'x' did not match\nhint: whatever\n"),
            "pathspec 'x' did not match"
        );
        assert_eq!(
            git_error("\nfatal: not a git repository\n"),
            "not a git repository"
        );
        assert_eq!(
            git_error("Switched to branch 'feature'\nfatal: hook said no\n"),
            "hook said no",
            "the error, not the chatter before it"
        );
        assert_eq!(git_error("something odd\n"), "something odd");
        assert_eq!(git_error(""), "git failed");
    }

    #[test]
    fn a_refused_push_says_why_rather_than_that_it_was() {
        let hook = "tests failed\nerror: failed to push some refs to '/o.git'\n";
        assert_eq!(remote_error(hook), "rejected: tests failed");
        let raced = "To /o.git\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to '/o.git'\nhint: Updates were rejected\n";
        assert_eq!(
            remote_error(raced),
            "rejected: ! [rejected] main -> main (fetch first)"
        );
        assert_eq!(
            remote_error("fatal: Could not read from remote repository.\n"),
            "Could not read from remote repository."
        );
    }
}
