//! The SAVED DRAFT: what was typed into the QUICK PROMPT and not yet sent,
//! kept on disk so closing the terminal window does not lose it.
//!
//! The DRAFT slot (`App::quick_draft`, `quick_prompt::QuickDraft`) already
//! hands an abandoned box's text to the next box — but only in memory, and
//! closing the window kills orion outright: no clean shutdown, no exit
//! hook, the slot gone with the process. So the text is written down as it
//! is typed, to `quick_prompt_draft.txt` in the DATA DIR, and a fresh box
//! opened with nothing of its own to show — after a restart, the slot
//! empty — starts from it, the caret at its end and `draft restored` on its
//! explanation line.
//!
//! The lifecycle, all of it driven from the event loop:
//!
//! * **Typed** — every key, paste or CLIPBOARD IMAGE that changes a QUICK
//!   PROMPT's text writes it ([`note_edit`]). A box prefilled with text of
//!   its own — a refused launch handed back, a PR box with a parked prompt
//!   — writes nothing until the user edits it, so opening one never
//!   overwrites the draft. The write is skipped when the text is what the
//!   file already holds, and is one small file put down whole (temp file,
//!   then rename): cheap enough to do on every keystroke, which is the
//!   only way a window closed mid-sentence keeps the sentence.
//! * **Restored** — `quick_prompt::open_box` and `open_picked_box`, the
//!   doors every fresh box comes through, fall back to it when the DRAFT
//!   slot is empty ([`restore`]). Boxes that carry their own text (a
//!   picker's round trip, a refused launch, a preset's task, a follow-up)
//!   never come through those doors, so never take it.
//! * **Cleared** — when a QUICK PROMPT launches the text the file holds
//!   ([`launched`]), and when the box is emptied by hand (an empty text is
//!   no draft: the file is removed). A launch the DAEMON refuses hands the
//!   box back with its text, and puts it back on disk too ([`refused`]).
//!
//! Nothing here touches the disk unless an [`App`] carries a
//! [`SavedDraft`]: the main loop installs one at startup, the unit tests
//! only ever a temporary one, so no test reads or writes the real user's
//! draft.

use crate::app::{App, Overlay, PromptKind};
use crate::text_input::TextInput;
use std::path::{Path, PathBuf};

/// The draft's file in the DATA DIR.
pub const FILE_NAME: &str = "quick_prompt_draft.txt";

/// The draft file and what it holds — read once at startup, then kept in
/// step with every write, so a write that would change nothing is never
/// made and a box opening never reads the disk.
#[derive(Debug, Clone)]
pub struct SavedDraft {
    path: PathBuf,
    text: String,
}

impl SavedDraft {
    /// The DATA DIR's draft.
    pub fn default_location() -> Self {
        Self::at(orion_core::paths::data_dir().join(FILE_NAME))
    }

    /// The draft kept at `path`, read now; a missing or unreadable file is
    /// no draft.
    pub fn at(path: PathBuf) -> Self {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let text = if text.trim().is_empty() {
            String::new()
        } else {
            text
        };
        Self { path, text }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The text on disk; empty for none.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Put `text` down as the draft — nothing at all, when it is what the
    /// file already holds. Text that is only whitespace is no draft: the
    /// file goes. Best effort: a write that fails is logged and the box
    /// carries on, the text still in front of the user.
    pub fn save(&mut self, text: &str) {
        let text = if text.trim().is_empty() { "" } else { text };
        if text == self.text {
            return;
        }
        let written = if text.is_empty() {
            match std::fs::remove_file(&self.path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            }
        } else {
            orion_core::settings::write_atomic(&self.path, text.as_bytes())
        };
        if let Err(err) = written {
            tracing::warn!(path = %self.path.display(), "quick prompt draft not saved: {err}");
        }
        self.text = text.to_string();
    }

    /// No draft any more: the file removed.
    pub fn clear(&mut self) {
        self.save("");
    }
}

/// The user changed the text of the box that is up: when it is a QUICK
/// PROMPT — the one box whose text is the SAVED DRAFT — write the text
/// down, and let its `draft restored` cue go: the text is simply what is
/// being typed now.
pub(crate) fn note_edit(app: &mut App) {
    let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
        return;
    };
    if !matches!(prompt.kind, PromptKind::QuickPrompt(_)) {
        return;
    }
    prompt.draft_restored = false;
    if let Some(saved) = &mut app.saved_draft {
        saved.save(prompt.input.as_str());
    }
}

/// The field a fresh box opens on when the DRAFT slot is empty: the SAVED
/// DRAFT, the caret at its end. None with nothing saved (or no store).
pub(crate) fn restore(app: &App) -> Option<TextInput> {
    let text = app.saved_draft.as_ref()?.text();
    (!text.is_empty()).then(|| TextInput::multiline_with_text(text))
}

/// A QUICK PROMPT holding `text` just launched: when that is the draft,
/// it is spent. A box sent with other text — a `skip_task` preset's empty
/// one, a PR box's own — leaves the draft for the next box.
pub(crate) fn launched(app: &mut App, text: &str) {
    if let Some(saved) = app.saved_draft.as_mut().filter(|s| s.text() == text) {
        saved.clear();
    }
}

/// A QUICK PROMPT launch the DAEMON refused came back with `text`: when
/// the launch spent the draft, `text` is the draft again.
pub(crate) fn refused(app: &mut App, text: &str) {
    if let Some(saved) = app.saved_draft.as_mut().filter(|s| s.text().is_empty()) {
        saved.save(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_round_trips_and_an_unchanged_text_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut saved = SavedDraft::at(path.clone());
        assert_eq!(saved.text(), "");
        saved.save("fix the login\nredirect");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fix the login\nredirect"
        );
        assert_eq!(
            SavedDraft::at(path.clone()).text(),
            "fix the login\nredirect"
        );
        // The same text again is no write: a file swapped in behind the
        // store's back is left alone.
        std::fs::write(&path, "elsewhere").unwrap();
        saved.save("fix the login\nredirect");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "elsewhere");
        // No temp file left beside it.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(FILE_NAME)]);
    }

    #[test]
    fn an_empty_or_blank_text_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut saved = SavedDraft::at(path.clone());
        saved.save("draft");
        saved.save("  \n ");
        assert!(!path.exists());
        assert_eq!(saved.text(), "");
        saved.clear();
        std::fs::write(&path, " \n").unwrap();
        assert_eq!(SavedDraft::at(path).text(), "", "a blank file is no draft");
    }
}
