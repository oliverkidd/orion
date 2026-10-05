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
    use std::io::Read;
    let mut child = detached(root, args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    // Drained on a thread of its own, so a remote that says a lot can't
    // fill the pipe and stall git while this loop only polls.
    let (said_tx, said_rx) = std::sync::mpsc::channel();
    if let Some(mut pipe) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut said = String::new();
            let _ = pipe.read_to_string(&mut said);
            let _ = said_tx.send(said);
        });
    }
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
    let said = said_rx.recv_timeout(STDERR_WAIT).unwrap_or_default();
    match ended {
        Ok(true) => Ok(()),
        Ok(false) => Err(remote_error(&said)),
        Err(why) => Err(format!("{} {why}", args.first().copied().unwrap_or("git"))),
    }
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
