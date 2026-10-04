//! The two ways orion reaches a user who is not looking at it: the DONE
//! SOUND and FEEDBACK SOUND (`Config::done_sound` / `Config::feedback_sound`,
//! played here), and the desktop notification a session that finishes or
//! stops to ask posts while the terminal window is in the background. `event_loop.rs`
//! decides *when* — the status edges, the per-frame drain of
//! `App::pending_ding`, `App::pending_done` and `App::pending_feedback`,
//! the settings overlay's `App::pending_sound_preview`, the focus and ssh
//! gates — this module only does the reaching, and fails soft: an `afplay`
//! that won't start falls back to the bell, a notifier that is missing or
//! exits non-zero is a debug line in tui.log.

use crate::app::{FeedbackAlert, Tree};
use crate::config::Sound;
use orion_core::AgentId;
use std::process::{Command, Stdio};

/// Ring `sound` once: a file goes to `afplay` (detached; a helper thread
/// reaps it so no zombie lingers), the bell — and a file whose `afplay`
/// won't start — is the BEL written through `backend`, so over ssh it
/// rings the terminal the user is sitting at.
pub(super) fn play_sound<W: std::io::Write>(backend: &mut W, sound: Sound) {
    if let Sound::File(path) = &sound {
        if let Ok(mut child) = Command::new("afplay")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return;
        }
    }
    let _ = backend.write_all(b"\x07");
    let _ = backend.flush();
}

/// The alert for `agent`, as its row reads on screen: the session's name
/// and `<project> · <branch>`. `None` for a row the tree doesn't hold (a
/// status that raced a delete).
pub(super) fn alert_for(tree: &Tree, agent: &AgentId) -> Option<FeedbackAlert> {
    let a = tree.agents.iter().find(|a| a.id == *agent)?;
    let worktree = tree.worktrees.iter().find(|w| w.id == a.worktree_id);
    let project = worktree.and_then(|w| tree.projects.iter().find(|p| p.id == w.project_id));
    let place = match (project, worktree) {
        (Some(p), Some(w)) => format!("{} · {}", p.name, w.branch),
        (None, Some(w)) => w.branch.clone(),
        _ => String::new(),
    };
    Some(FeedbackAlert {
        session: a.name.clone(),
        place,
        limit: a.limit_reached().map(orion_core::UsageLimit::label),
    })
}

/// Which status edge a desktop notification is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Edge {
    /// The turn finished (the DONE SOUND's edge).
    Done,
    /// The turn stopped to ask the user (the FEEDBACK SOUND's).
    Feedback,
}

/// Post one desktop notification per alert, detached, on a helper thread
/// that also reaps it. On macOS that is the NOTIFIER APP — orion's name and
/// logo, a click brings the terminal back (`notifier_app`) — or
/// `osascript` when it can't be built; elsewhere `notify-send`. A notifier
/// that is missing or exits non-zero is logged at debug and otherwise
/// ignored: the sound already rang, and a box with no desktop is not an
/// error.
pub(super) fn notify_desktop(alerts: &[FeedbackAlert], edge: Edge) {
    let alerts = alerts.to_vec();
    std::thread::spawn(move || {
        let macos = cfg!(target_os = "macos");
        let app = if macos {
            super::notifier_app::executable()
        } else {
            None
        };
        let activate = app.and_then(|_| super::notifier_app::terminal_bundle_id());
        for alert in &alerts {
            let (program, args) = match app {
                Some(exe) => (
                    exe.to_string_lossy().into_owned(),
                    notifier_app_args(alert, edge, activate.as_deref()),
                ),
                None => {
                    let (program, args) = notifier_command(alert, edge, macos);
                    (program.to_string(), args)
                }
            };
            match Command::new(&program)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Ok(status) if !status.success() => {
                    tracing::debug!(program, %status, "desktop notification not posted");
                }
                Err(err) => tracing::debug!(program, %err, "desktop notification not posted"),
                Ok(_) => {}
            }
        }
    });
}

/// The notification's headline: *`<session>` finished*, *`<session>` needs
/// feedback*, or *`<session>`: limit reached* for one stopped on a usage
/// limit.
fn summary(alert: &FeedbackAlert, edge: Edge) -> String {
    match (edge, alert.limit) {
        (Edge::Feedback, Some(limit)) => format!("{}: {limit}", alert.session),
        (Edge::Feedback, None) => format!("{} needs feedback", alert.session),
        (Edge::Done, _) => format!("{} finished", alert.session),
    }
}

/// `terminal-notifier` argv for the NOTIFIER APP: *orion* / the summary /
/// `<project> · <branch>`, grouped per session so a newer notification
/// for one replaces its last, and `-activate`-ing the terminal on click.
/// It reads the message from stdin when `-message` is empty, so a
/// placeless alert carries the summary as its message instead.
fn notifier_app_args(alert: &FeedbackAlert, edge: Edge, activate: Option<&str>) -> Vec<String> {
    let summary = summary(alert, edge);
    let mut args: Vec<String> = vec!["-title".into(), "orion".into()];
    if alert.place.is_empty() {
        args.extend(["-message".into(), summary]);
    } else {
        args.extend([
            "-subtitle".into(),
            summary,
            "-message".into(),
            alert.place.clone(),
        ]);
    }
    args.extend(["-group".into(), format!("orion:{}", alert.session)]);
    if let Some(id) = activate {
        args.extend(["-activate".into(), id.into()]);
    }
    args
}

/// The fallback notifier for `alert`, as a program and its argv — never a
/// shell line, so the only quoting is AppleScript's own. macOS shows
/// *orion* / the summary / *`<project> · <branch>`*; `notify-send` gets
/// the same as app name, summary and body.
fn notifier_command(alert: &FeedbackAlert, edge: Edge, macos: bool) -> (&'static str, Vec<String>) {
    let summary = summary(alert, edge);
    if macos {
        let mut script = format!(
            "display notification {} with title \"orion\" subtitle {}",
            applescript_str(&alert.place),
            applescript_str(&summary)
        );
        if alert.place.is_empty() {
            // No body to show: the subtitle carries the whole message.
            script = format!(
                "display notification {} with title \"orion\"",
                applescript_str(&summary)
            );
        }
        ("osascript", vec!["-e".into(), script])
    } else {
        (
            "notify-send",
            vec!["--app-name=orion".into(), summary, alert.place.clone()],
        )
    }
}

/// `s` as an AppleScript string literal: backslashes and double quotes
/// escaped, control characters dropped — a session name is one line.
fn applescript_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(session: &str, place: &str) -> FeedbackAlert {
        FeedbackAlert {
            session: session.into(),
            place: place.into(),
            limit: None,
        }
    }

    /// A session name is user text — a quote or a backslash in it must
    /// not break out of the AppleScript literal, and a newline can't make
    /// the script two statements.
    #[test]
    fn applescript_literals_are_escaped() {
        assert_eq!(applescript_str("agent-2"), r#""agent-2""#);
        assert_eq!(
            applescript_str(r#"say "hi" \ now"#),
            r#""say \"hi\" \\ now""#
        );
        assert_eq!(applescript_str("one\ntwo\ttab"), r#""onetwotab""#);
        assert_eq!(applescript_str("demo · main"), r#""demo · main""#);
    }

    /// The macOS notifier is `osascript -e <one display notification>`,
    /// orion as the title, the session as the subtitle, the place as the
    /// body — and just the session when there is no place to name. Linux
    /// gets `notify-send` with the same three under its own names.
    #[test]
    fn notifier_command_names_the_session_and_its_place() {
        let (program, args) =
            notifier_command(&alert("Fix Login", "demo · main"), Edge::Feedback, true);
        assert_eq!(program, "osascript");
        assert_eq!(
            args,
            [
                "-e",
                r#"display notification "demo · main" with title "orion" subtitle "Fix Login needs feedback""#,
            ]
        );

        let (_, args) = notifier_command(&alert("agent-2", ""), Edge::Feedback, true);
        assert_eq!(
            args[1],
            r#"display notification "agent-2 needs feedback" with title "orion""#
        );

        let (program, args) =
            notifier_command(&alert("Fix Login", "demo · main"), Edge::Feedback, false);
        assert_eq!(program, "notify-send");
        assert_eq!(
            args,
            [
                "--app-name=orion",
                "Fix Login needs feedback",
                "demo · main"
            ]
        );

        // A session stopped on a usage limit says so instead.
        let limited = FeedbackAlert {
            limit: Some("limit reached"),
            ..alert("Fix Login", "demo · main")
        };
        let (_, args) = notifier_command(&limited, Edge::Feedback, false);
        assert_eq!(args[1], "Fix Login: limit reached");
    }

    /// The NOTIFIER APP gets orion as the title, what happened as the
    /// subtitle and the place as the message, grouped per session and
    /// bringing the terminal back on click; with no place the summary is
    /// the message (an empty `-message` would read stdin).
    #[test]
    fn notifier_app_args_name_the_edge_and_bring_the_terminal_back() {
        let args = notifier_app_args(
            &alert("Fix Login", "demo · main"),
            Edge::Done,
            Some("com.mitchellh.ghostty"),
        );
        assert_eq!(
            args,
            [
                "-title",
                "orion",
                "-subtitle",
                "Fix Login finished",
                "-message",
                "demo · main",
                "-group",
                "orion:Fix Login",
                "-activate",
                "com.mitchellh.ghostty",
            ]
        );

        let args = notifier_app_args(&alert("agent-2", ""), Edge::Feedback, None);
        assert_eq!(
            args,
            [
                "-title",
                "orion",
                "-message",
                "agent-2 needs feedback",
                "-group",
                "orion:agent-2",
            ]
        );
    }

    /// The bell path writes exactly one BEL through the backend; `off` is
    /// the caller's business (a `None` never reaches here).
    #[test]
    fn the_bell_is_one_bel_byte() {
        let mut out = Vec::new();
        play_sound(&mut out, Sound::Bell);
        assert_eq!(out, b"\x07");
    }
}
