//! The two ways orion reaches a user who is not looking at it: the DONE
//! SOUND and FEEDBACK SOUND (`Config::done_sound` / `Config::feedback_sound`,
//! played here), and the desktop notification a session posts while the
//! terminal window is in the background — when it stops to ask, when its
//! CLI dies mid-turn, and when a turn nobody watched has finished.
//! `event_loop.rs` decides *when* — the status edges ([`edge_alert`]), the
//! DONE SOUND's settling and folding (`App::done_sounds`), the per-frame
//! drain of `App::pending_feedback`, the settings overlay's
//! `App::pending_sound_preview`, the focus and ssh gates — this module
//! only does the reaching, and fails soft: an `afplay` that won't start
//! falls back to the bell, a notifier that is missing or exits non-zero is
//! a debug line in tui.log.

use crate::app::{AlertKind, FeedbackAlert, Tree};
use crate::config::Sound;
use orion_core::{AgentId, AgentStatus};
use std::process::{Command, Stdio};

/// What a status flip from `from` to `to` has to say to a user who is not
/// looking, before any gate: the edge into NEEDS FEEDBACK asks
/// ([`AlertKind::NeedsFeedback`]); a live turn — RUNNING or NEEDS FEEDBACK
/// — landing on FINISHED is done ([`AlertKind::Finished`]), and on
/// TERMINATED crashed ([`AlertKind::Crashed`]). A re-stamp of the status a
/// row already wears says nothing, and neither does any other move: a
/// gray row booting, a dead one coming back, a daemon restart's
/// DISCONNECTED.
pub(super) fn edge_alert(from: AgentStatus, to: AgentStatus) -> Option<AlertKind> {
    let live = matches!(from, AgentStatus::Running | AgentStatus::NeedsFeedback);
    match to {
        AgentStatus::NeedsFeedback if from != AgentStatus::NeedsFeedback => {
            Some(AlertKind::NeedsFeedback)
        }
        AgentStatus::Finished if live => Some(AlertKind::Finished),
        AgentStatus::Terminated if live => Some(AlertKind::Crashed),
        _ => None,
    }
}

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

/// The `kind` alert for `agent`, as its row reads on screen: the session's
/// name and `<project> · <branch>`. `None` for a row the tree doesn't hold
/// (a status that raced a delete).
pub(super) fn alert_for(tree: &Tree, agent: &AgentId, kind: AlertKind) -> Option<FeedbackAlert> {
    let a = tree.agents.iter().find(|a| a.id == *agent)?;
    let worktree = tree.worktrees.iter().find(|w| w.id == a.worktree_id);
    let project = worktree.and_then(|w| tree.projects.iter().find(|p| p.id == w.project_id));
    let place = match (project, worktree) {
        (Some(p), Some(w)) => format!("{} · {}", p.name, w.branch),
        (None, Some(w)) => w.branch.clone(),
        _ => String::new(),
    };
    Some(FeedbackAlert {
        kind,
        session: a.name.clone(),
        place,
        limit: a.limit_reached().map(orion_core::UsageLimit::label),
    })
}

/// Post one desktop notification per alert, detached, on a helper thread
/// that also reaps it. On macOS that is the NOTIFIER APP — orion's name and
/// logo, a click brings the terminal back (`notifier_app`) — or
/// `osascript` when it can't be built; elsewhere `notify-send`. A notifier
/// that is missing or exits non-zero is logged at debug and otherwise
/// ignored: the sound already rang (or was folded into one that just did),
/// and a box with no desktop is not an error.
pub(super) fn notify_desktop(alerts: &[FeedbackAlert]) {
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
                    notifier_app_args(alert, activate.as_deref()),
                ),
                None => {
                    let (program, args) = notifier_command(alert, macos);
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

/// The notification's headline: *`<session>` needs feedback* — or
/// *`<session>`: limit reached* for one stopped on a usage limit —
/// *`<session>` stopped with an error* for a crash, and *`<session>`
/// finished* for an unseen finish.
fn summary(alert: &FeedbackAlert) -> String {
    match (alert.kind, alert.limit) {
        (AlertKind::NeedsFeedback, Some(limit)) => format!("{}: {limit}", alert.session),
        (AlertKind::NeedsFeedback, None) => format!("{} needs feedback", alert.session),
        (AlertKind::Crashed, _) => format!("{} stopped with an error", alert.session),
        (AlertKind::Finished, _) => format!("{} finished", alert.session),
    }
}

/// `terminal-notifier` argv for the NOTIFIER APP: *orion* / the summary /
/// `<project> · <branch>`, grouped per session so a newer notification
/// for one replaces its last, and `-activate`-ing the terminal on click.
/// It reads the message from stdin when `-message` is empty, so a
/// placeless alert carries the summary as its message instead.
fn notifier_app_args(alert: &FeedbackAlert, activate: Option<&str>) -> Vec<String> {
    let summary = summary(alert);
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
fn notifier_command(alert: &FeedbackAlert, macos: bool) -> (&'static str, Vec<String>) {
    let summary = summary(alert);
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
            kind: AlertKind::NeedsFeedback,
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
        let (program, args) = notifier_command(&alert("Fix Login", "demo · main"), true);
        assert_eq!(program, "osascript");
        assert_eq!(
            args,
            [
                "-e",
                r#"display notification "demo · main" with title "orion" subtitle "Fix Login needs feedback""#,
            ]
        );

        let (_, args) = notifier_command(&alert("agent-2", ""), true);
        assert_eq!(
            args[1],
            r#"display notification "agent-2 needs feedback" with title "orion""#
        );

        let (program, args) = notifier_command(&alert("Fix Login", "demo · main"), false);
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
        let (_, args) = notifier_command(&limited, false);
        assert_eq!(args[1], "Fix Login: limit reached");

        // A crash and an unseen finish say what happened, in the same
        // place; a limit still recorded on a row that left red says nothing.
        let crashed = FeedbackAlert {
            kind: AlertKind::Crashed,
            ..alert("Fix Login", "demo · main")
        };
        let (_, args) = notifier_command(&crashed, true);
        assert_eq!(
            args[1],
            r#"display notification "demo · main" with title "orion" subtitle "Fix Login stopped with an error""#
        );
        let finished = FeedbackAlert {
            kind: AlertKind::Finished,
            limit: Some("limit reached"),
            ..alert("Fix Login", "demo · main")
        };
        let (_, args) = notifier_command(&finished, false);
        assert_eq!(
            args,
            ["--app-name=orion", "Fix Login finished", "demo · main"]
        );
    }

    /// The edges that reach a user who is not looking — into red, and a
    /// live turn landing on green or dying — and none of the moves around
    /// them.
    #[test]
    fn edge_alert_names_three_edges_and_nothing_else() {
        use AgentStatus::*;
        let edge = edge_alert;
        assert_eq!(edge(Running, NeedsFeedback), Some(AlertKind::NeedsFeedback));
        assert_eq!(
            edge(Finished, NeedsFeedback),
            Some(AlertKind::NeedsFeedback)
        );
        assert_eq!(
            edge(NeedsFeedback, NeedsFeedback),
            None,
            "a re-stamp of red"
        );

        assert_eq!(edge(Running, Finished), Some(AlertKind::Finished));
        assert_eq!(edge(NeedsFeedback, Finished), Some(AlertKind::Finished));
        assert_eq!(edge(Finished, Finished), None, "a re-stamp of green");
        assert_eq!(edge(Fresh, Finished), None, "no turn was live");

        assert_eq!(edge(Running, Terminated), Some(AlertKind::Crashed));
        assert_eq!(edge(NeedsFeedback, Terminated), Some(AlertKind::Crashed));
        assert_eq!(edge(Finished, Terminated), None, "no turn was live");
        assert_eq!(edge(Terminated, Terminated), None);

        assert_eq!(edge(Running, Disconnected), None, "a daemon restart");
        assert_eq!(edge(Finished, Running), None);
        assert_eq!(edge(Fresh, Running), None);
    }

    /// The NOTIFIER APP gets orion as the title, what happened as the
    /// subtitle and the place as the message, grouped per session and
    /// bringing the terminal back on click; with no place the summary is
    /// the message (an empty `-message` would read stdin).
    #[test]
    fn notifier_app_args_name_the_edge_and_bring_the_terminal_back() {
        let finished = FeedbackAlert {
            kind: AlertKind::Finished,
            ..alert("Fix Login", "demo · main")
        };
        let args = notifier_app_args(&finished, Some("com.mitchellh.ghostty"));
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

        let args = notifier_app_args(&alert("agent-2", ""), None);
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
