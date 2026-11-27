//! DICTATION's keys, clicks and helper lines (`crate::dictation`).
//!
//! Read ahead of everything else a key meets — the RELEASE WATCH, every
//! overlay, the locked pane — since `⌘D` has to start dictation in any
//! box, and while dictation runs Enter and Esc are its. `⌘D` pressed
//! starts it and arms a RELEASE WATCH, so a held key reports its repeats
//! and its release: held past `HOLD_AFTER`, letting go finishes it (HOLD
//! TO TALK); tapped, the next press does. Every other key goes where it
//! would have, so typing carries on under the ghost.
//!
//! The finish is a paste: what was heard goes in through the very path
//! ⌘V takes ([`super::dispatch_input`]), into the field it was said for —
//! or, gone from the screen by then, onto the clipboard.

use super::release_watch;
use crate::app::{App, Focus};
use crate::dictation::{self, Engine, Heard, Phase, Target};
use crate::flash::Flash;
use crate::keymap::{Action, KeyChord};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use orion_core::protocol::ClientRequest;
use std::time::Instant;

/// A locked pane with a live session holds the keys: the words go to it.
fn pane_takes_keys(app: &App) -> bool {
    app.focus == Focus::Terminal
        && app.term_locked
        && app.term.is_some()
        && app.overlay.is_none()
        && app.page.is_none()
        && !app.pane_shows_placeholder()
}

/// A key, before anything else sees it. True when it was dictation's.
pub(super) fn intercept(app: &mut App, key: &KeyEvent) -> bool {
    let chord = KeyChord::from_event(key);
    let now = Instant::now();
    if app.keymap.chords(Action::Dictate).contains(&chord)
        && app.editor.is_none()
        && dictation::available(app)
        && (dictation::key_free(app) || app.dictation.is_some())
    {
        match key.kind {
            KeyEventKind::Press => {
                let running = app.dictation.is_some();
                press(app, chord, now);
                let does = if running { "Stop dictating" } else { "Dictate" };
                crate::key_combo::note(app, &[chord], Some(does));
            }
            KeyEventKind::Repeat => {
                release_watch::take(&mut app.release_watch, key, now);
                if let Some(d) = app
                    .dictation
                    .as_mut()
                    .filter(|d| d.phase != Phase::Settling)
                {
                    d.held = true;
                }
            }
            KeyEventKind::Release => {
                release_watch::take(&mut app.release_watch, key, now);
                if let Some(d) = app
                    .dictation
                    .as_mut()
                    .filter(|d| d.phase != Phase::Settling)
                {
                    if d.held || now.duration_since(d.pressed) >= dictation::HOLD_AFTER {
                        d.stop(now);
                    }
                }
            }
        }
        app.dirty = true;
        return true;
    }
    if app.dictation.is_none() || key.kind != KeyEventKind::Press {
        return false;
    }
    if crate::nav::closes(key) {
        app.dictation = None;
        app.flash = Some(Flash::note("dictation thrown away"));
    } else if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::NONE {
        if let Some(d) = &mut app.dictation {
            d.stop(now);
        }
    } else {
        return false;
    }
    crate::key_combo::note(app, &[chord], None);
    app.dirty = true;
    true
}

/// `⌘D` with dictation off or out of reach here: the footer says why.
pub(super) fn unavailable(app: &mut App) {
    app.flash = Some(if !app.dictation_enabled {
        Flash::setup("dictation is off: Settings › General › Dictation")
    } else {
        Flash::note("dictation runs on a Mac with macOS 13 or later, not over ssh")
    });
}

/// `⌘D` down: start, or stop what is running.
fn press(app: &mut App, chord: KeyChord, now: Instant) {
    if let Some(d) = &mut app.dictation {
        d.stop(now);
        return;
    }
    if app.dictation_blocked {
        open_mic_settings(app);
        return;
    }
    if start(app, now) {
        // The held key's repeats and release come marked from here on.
        release_watch::arm(&mut app.release_watch, chord, now);
    }
}

/// The microphone was refused: its page of System Settings, and the
/// footer saying what to turn on there.
fn open_mic_settings(app: &mut App) {
    crate::open_url::run_open(&[dictation::MIC_SETTINGS.to_string()]);
    app.flash = Some(Flash::setup(dictation::error_line("mic-denied", "")));
}

/// Start a dictation for wherever the keys are going. False, with a
/// footer line saying why, when there is nowhere or it can't start.
fn start(app: &mut App, now: Instant) -> bool {
    let target = if let Some(field) = crate::text_input::caret_field() {
        Target::Field(field.id)
    } else if pane_takes_keys(app) {
        match &app.term {
            Some(term) => Target::Pane(term.sref.clone()),
            None => return false,
        }
    } else {
        let key = crate::hints::key_or(&app.keymap, Action::Dictate, "Dictate");
        app.flash = Some(Flash::note(format!(
            "{key} types where the caret is: open a box, or a session's pane, first"
        )));
        return false;
    };
    let Some(tx) = app.dictation_tx.clone() else {
        return false;
    };
    let id = app.dictation_next;
    app.dictation_next += 1;
    match dictation::start(id, target, &app.dictation_words, tx, now) {
        Ok(d) => {
            app.dictation = Some(d);
            app.flash = None;
            true
        }
        Err(e) => {
            app.flash = Some(Flash::failed(format!("dictation couldn't start: {e}")));
            false
        }
    }
}

/// A click: on the MIC CHIP it is `⌘D`. True when it was.
pub(super) fn click(app: &mut App, mouse: &MouseEvent) -> bool {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return false;
    }
    let at = ratatui::layout::Position::new(mouse.column, mouse.row);
    if !app.dictation_chip.is_some_and(|chip| chip.contains(at)) || !dictation::available(app) {
        return false;
    }
    let now = Instant::now();
    if let Some(d) = &mut app.dictation {
        d.stop(now);
    } else if app.dictation_blocked {
        open_mic_settings(app);
    } else {
        start(app, now);
    }
    app.dirty = true;
    true
}

/// A line from dictation `id`'s helper.
pub(super) fn heard(app: &mut App, id: u64, heard: Heard, out: &mut Vec<ClientRequest>) {
    let Some(d) = app.dictation.as_mut().filter(|d| d.id == id) else {
        return;
    };
    let ends = d.hear(&heard);
    let settling = d.phase == Phase::Settling;
    app.dirty = true;
    match heard {
        Heard::Engine { name } => {
            app.dictation_engine = Some(if name == "basic" {
                Engine::Basic
            } else {
                Engine::Analyzer
            });
        }
        Heard::Listening => app.dictation_blocked = false,
        Heard::Error { kind, message } => {
            app.dictation = None;
            let line = dictation::error_line(&kind, &message);
            if kind == "mic-denied" || kind == "speech-denied" {
                app.dictation_blocked = kind == "mic-denied";
                app.flash = Some(Flash::setup(line));
            } else {
                app.flash = Some(Flash::failed(line));
            }
        }
        Heard::Done { .. } => finish(app, out),
        Heard::Gone if settling => finish(app, out),
        Heard::Gone => {
            app.dictation = None;
            app.flash = Some(Flash::failed("dictation stopped: its helper quit"));
        }
        _ if ends => finish(app, out),
        _ => {}
    }
}

/// When a finish stops waiting for the last words.
pub(super) fn settle_deadline(app: &App) -> Option<Instant> {
    app.dictation.as_ref().and_then(|d| d.settle_by)
}

/// The finish waited out [`settle_deadline`]: what was heard goes in.
pub(super) fn settle_timed_out(app: &mut App, out: &mut Vec<ClientRequest>) {
    if settle_deadline(app).is_some_and(|by| Instant::now() >= by) {
        finish(app, out);
    }
}

/// What was heard goes in where it was said for, as a paste.
fn finish(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(d) = app.dictation.take() else {
        return;
    };
    let text = d.text();
    let target = d.target.clone();
    drop(d);
    app.dirty = true;
    if text.is_empty() {
        app.flash = Some(Flash::note("dictation heard nothing"));
        return;
    }
    let landed = match target {
        Target::Field(id) => match crate::text_input::caret_field() {
            Some(field) if field.id == id => Some(spaced(&text, field.char_before)),
            _ => None,
        },
        Target::Pane(sref) => {
            let same = app.term.as_ref().is_some_and(|t| t.sref == sref);
            (pane_takes_keys(app) && same).then_some(text.clone())
        }
    };
    match landed {
        Some(text) => super::dispatch_input(app, Event::Paste(text), out),
        None => {
            super::copy_text_and_flash(app, &text);
            app.flash = Some(Flash::note(
                "its box closed while you talked, so what you said is on the clipboard",
            ));
        }
    }
}

/// `text` as it goes in after `before`: a space between it and a word it
/// would otherwise run on from.
fn spaced(text: &str, before: Option<char>) -> String {
    let runs_on = before.is_some_and(|c| !c.is_whitespace());
    let opens_with_punctuation = text.starts_with(['.', ',', '!', '?', ';', ':', ')']);
    if runs_on && !opens_with_punctuation {
        format!(" {text}")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictated_text_gets_a_space_only_after_a_word() {
        assert_eq!(spaced("then run it", Some('d')), " then run it");
        assert_eq!(spaced("then run it", Some(' ')), "then run it");
        assert_eq!(spaced("then run it", None), "then run it");
        assert_eq!(spaced(", then", Some('d')), ", then");
    }
}
