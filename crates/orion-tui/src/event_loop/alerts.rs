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

/// What one desktop notification says: its headline and the line under
/// it (empty for none).
#[derive(Debug, Clone)]
struct Note {
    summary: String,
    place: String,
}

impl Note {
    /// A session's alert: the edge as headline, `<project> · <branch>`
    /// under it.
    fn of(alert: &FeedbackAlert) -> Self {
        Self {
            summary: summary(alert),
            place: alert.place.clone(),
        }
    }
}

/// Post one desktop notification per alert. In Orion.app the app posts
/// them itself — its name, its icon, and a click that brings its window
/// back — asked through the terminal it is ([`host_escape`]): they are
/// queued here and written by the loop ([`write_host_notes`]). Anywhere
/// else a notifier is run, detached, on a helper thread that also reaps
/// it: `osascript` on macOS, `notify-send` elsewhere. One that is missing
/// or exits non-zero is logged at debug and otherwise ignored: the sound
/// already rang (or was folded into one that just did), and a box with no
/// desktop is not an error.
pub(super) fn notify_desktop(alerts: &[FeedbackAlert]) {
    post(alerts.iter().map(Note::of).collect());
}

/// A desktop notification that is not about a session — `summary` over
/// `body` — through the same notifier [`notify_desktop`] uses.
pub(crate) fn notify_text(summary: &str, body: &str) {
    post(vec![Note {
        summary: summary.to_string(),
        place: body.to_string(),
    }]);
}

/// Notifications waiting to be written to the terminal orion is drawn in.
static HOST_NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Write the escapes [`post`] has queued to the terminal orion is drawn
/// in, `backend` — the loop's own writer, between frames, never another
/// thread's mid-frame.
pub(super) fn write_host_notes<W: std::io::Write>(backend: &mut W) {
    let notes = HOST_NOTES
        .lock()
        .map(|mut notes| std::mem::take(&mut *notes))
        .unwrap_or_default();
    if notes.is_empty() {
        return;
    }
    for note in notes {
        let _ = backend.write_all(note.as_bytes());
    }
    let _ = backend.flush();
}

/// `note` as the escape that asks the terminal to post it (OSC 777
/// `notify`, which Ghostty takes): the summary as its title, the place as
/// its body. A `;` ends a field and a control character the escape, so a
/// name holding one loses it.
fn host_escape(note: &Note) -> String {
    let field = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_control())
            .map(|c| if c == ';' { ',' } else { c })
            .collect()
    };
    format!(
        "\x1b]777;notify;{};{}\x1b\\",
        field(&note.summary),
        field(&note.place)
    )
}

fn post(notes: Vec<Note>) {
    if crate::app_bundle::inside() {
        if let Ok(mut queued) = HOST_NOTES.lock() {
            queued.extend(notes.iter().map(host_escape));
        }
        return;
    }
    std::thread::spawn(move || {
        for note in &notes {
            let (program, args) = notifier_command(note, cfg!(target_os = "macos"));
            match Command::new(program)
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

/// The notifier for `note` outside Orion.app, as a program and its argv —
/// never a shell line, so the only quoting is AppleScript's own. macOS shows
/// *orion* / the summary / the place; `notify-send` gets the same as app
/// name, summary and body.
fn notifier_command(note: &Note, macos: bool) -> (&'static str, Vec<String>) {
    if macos {
        let script = if note.place.is_empty() {
            // No body to show: the subtitle carries the whole message.
            format!(
                "display notification {} with title \"orion\"",
                applescript_str(&note.summary)
            )
        } else {
            format!(
                "display notification {} with title \"orion\" subtitle {}",
                applescript_str(&note.place),
                applescript_str(&note.summary)
            )
        };
        ("osascript", vec!["-e".into(), script])
    } else {
        (
            "notify-send",
            vec![
                "--app-name=orion".into(),
                note.summary.clone(),
                note.place.clone(),
            ],
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
        let (program, args) = notifier_command(&Note::of(&alert("Fix Login", "demo · main")), true);
        assert_eq!(program, "osascript");
        assert_eq!(
            args,
            [
                "-e",
                r#"display notification "demo · main" with title "orion" subtitle "Fix Login needs feedback""#,
            ]
        );

        let (_, args) = notifier_command(&Note::of(&alert("agent-2", "")), true);
        assert_eq!(
            args[1],
            r#"display notification "agent-2 needs feedback" with title "orion""#
        );

        let (program, args) =
            notifier_command(&Note::of(&alert("Fix Login", "demo · main")), false);
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
        let (_, args) = notifier_command(&Note::of(&limited), false);
        assert_eq!(args[1], "Fix Login: limit reached");

        // A crash and an unseen finish say what happened, in the same
        // place; a limit still recorded on a row that left red says nothing.
        let crashed = FeedbackAlert {
            kind: AlertKind::Crashed,
            ..alert("Fix Login", "demo · main")
        };
        let (_, args) = notifier_command(&Note::of(&crashed), true);
        assert_eq!(
            args[1],
            r#"display notification "demo · main" with title "orion" subtitle "Fix Login stopped with an error""#
        );
        let finished = FeedbackAlert {
            kind: AlertKind::Finished,
            limit: Some("limit reached"),
            ..alert("Fix Login", "demo · main")
        };
        let (_, args) = notifier_command(&Note::of(&finished), false);
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

    /// In Orion.app a notification is one escape: what happened as the
    /// title, the place as the body, and nothing in a name able to end a
    /// field or the escape early.
    #[test]
    fn a_note_in_the_app_is_one_escape() {
        let finished = FeedbackAlert {
            kind: AlertKind::Finished,
            ..alert("Fix Login", "demo · main")
        };
        assert_eq!(
            host_escape(&Note::of(&finished)),
            "\x1b]777;notify;Fix Login finished;demo · main\x1b\\"
        );
        let odd = host_escape(&Note::of(&alert("a;b\x07c\x1b", "")));
        assert_eq!(odd, "\x1b]777;notify;a,bc needs feedback;\x1b\\");
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
