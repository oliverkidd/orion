//! The SAVED DRAFT: what was typed into the QUICK PROMPT and not yet sent,
//! kept on disk so closing the terminal window does not lose it.
//!
//! A draft belongs to the place its box was aimed at ([`DraftPlace`]): the
//! WORKTREE it lands in, or — for a box that cuts a fresh one — the
//! PROJECT. A sentence half-typed for one checkout is not the next box's
//! in another, so every place keeps its own: a box opens on its place's
//! draft and nobody else's.
//!
//! The DRAFT slots (`App::quick_draft`, `quick_prompt::QuickDrafts`)
//! already hand an abandoned box's text to the next box at its place — but
//! only in memory, and closing the window kills orion outright: no clean
//! shutdown, no exit hook, the slots gone with the process. So the text is
//! written down as it is typed, to `quick_prompt_drafts.json` in the DATA
//! DIR (one entry per place), and a fresh box opened with nothing of its
//! own to show — after a restart, its slot empty — starts from its place's
//! entry, the caret at its end and `draft restored` on its explanation
//! line.
//!
//! The lifecycle, all of it driven from the event loop:
//!
//! * **Typed** — every key, paste or CLIPBOARD IMAGE that changes a QUICK
//!   PROMPT's text writes it under the box's place ([`note_edit`]). A box
//!   prefilled with text of its own — a refused launch handed back, a PR
//!   box with a parked prompt — writes nothing until the user edits it, so
//!   opening one never overwrites the draft. The write is skipped when the
//!   text is what the file already holds, and is one small file put down
//!   whole (temp file, then rename): cheap enough to do on every
//!   keystroke, which is the only way a window closed mid-sentence keeps
//!   the sentence.
//! * **Restored** — `quick_prompt::open_box` and `open_picked_box`, the
//!   doors every fresh box comes through, fall back to the place's entry
//!   when its DRAFT slot is empty ([`restore`]). Boxes that carry their own
//!   text (a picker's round trip, a refused launch, a preset's task, a
//!   follow-up) never come through those doors, so never take it.
//! * **Moved** — a box re-aimed by one of its pickers (the WORKTREE
//!   PICKER, `^P`) takes its draft along to the new place ([`followed`]).
//! * **Cleared** — when a QUICK PROMPT launches the text its place holds
//!   ([`launched`]), and when the box is emptied by hand (an empty text is
//!   no draft: the entry goes). A launch the DAEMON refuses hands the box
//!   back with its text, and puts it back on disk too ([`refused`]).
//!
//! Nothing here touches the disk unless an [`App`] carries a
//! [`SavedDraft`]: the main loop installs one at startup, the unit tests
//! only ever a temporary one, so no test reads or writes the real user's
//! drafts.

use crate::app::{App, Overlay, PromptKind};
use crate::quick_prompt::QuickTarget;
use crate::text_input::TextInput;
use orion_core::{ProjectId, WorktreeId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The drafts' file in the DATA DIR.
pub const FILE_NAME: &str = "quick_prompt_drafts.json";

/// Where a draft belongs: the WORKTREE a box lands in, or the PROJECT a
/// box cutting a fresh worktree lands in (its branch is minted when the
/// box opens, so no two such boxes would ever share one).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DraftPlace {
    Worktree(WorktreeId),
    Project(ProjectId),
}

impl DraftPlace {
    pub fn of(target: &QuickTarget) -> Self {
        match target {
            QuickTarget::Worktree(id) => Self::Worktree(id.clone()),
            QuickTarget::NewWorktree { project, .. } => Self::Project(project.clone()),
        }
    }

    /// The place's key in the file.
    fn key(&self) -> String {
        match self {
            Self::Worktree(id) => format!("worktree:{id}"),
            Self::Project(id) => format!("project:{id}"),
        }
    }
}

/// The drafts file and what it holds — read once at startup, then kept in
/// step with every write, so a write that would change nothing is never
/// made and a box opening never reads the disk.
#[derive(Debug, Clone)]
pub struct SavedDraft {
    path: PathBuf,
    drafts: BTreeMap<String, String>,
}

impl SavedDraft {
    /// The DATA DIR's drafts.
    pub fn default_location() -> Self {
        Self::at(orion_core::paths::data_dir().join(FILE_NAME))
    }

    /// The drafts kept at `path`, read now; a missing or unreadable file
    /// is no drafts, and a blank entry is no draft.
    pub fn at(path: PathBuf) -> Self {
        let mut drafts: BTreeMap<String, String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        drafts.retain(|_, text| !text.trim().is_empty());
        Self { path, drafts }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The text on disk for `place`; empty for none.
    pub fn text(&self, place: &DraftPlace) -> &str {
        self.drafts.get(&place.key()).map_or("", String::as_str)
    }

    /// Put `text` down as `place`'s draft — nothing at all, when it is
    /// what the file already holds. Text that is only whitespace is no
    /// draft: the entry goes, and the file with the last of them. Best
    /// effort: a write that fails is logged and the box carries on, the
    /// text still in front of the user.
    pub fn save(&mut self, place: &DraftPlace, text: &str) {
        let text = if text.trim().is_empty() { "" } else { text };
        if text == self.text(place) {
            return;
        }
        if text.is_empty() {
            self.drafts.remove(&place.key());
        } else {
            self.drafts.insert(place.key(), text.to_string());
        }
        self.write();
    }

    /// No draft any more at `place`.
    pub fn clear(&mut self, place: &DraftPlace) {
        self.save(place, "");
    }

    /// The draft holding exactly `text` is `to`'s now, wherever it was.
    /// Nothing when no place holds it, or `to` already does.
    pub fn moved(&mut self, text: &str, to: &DraftPlace) {
        let key = to.key();
        let Some(from) = self
            .drafts
            .iter()
            .find(|(k, t)| **k != key && t.as_str() == text)
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        if let Some(text) = self.drafts.remove(&from) {
            self.drafts.insert(key, text);
        }
        self.write();
    }

    fn write(&self) {
        let written = if self.drafts.is_empty() {
            match std::fs::remove_file(&self.path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            }
        } else {
            serde_json::to_vec_pretty(&self.drafts)
                .map_err(std::io::Error::other)
                .and_then(|bytes| orion_core::settings::write_atomic(&self.path, &bytes))
        };
        if let Err(err) = written {
            tracing::warn!(path = %self.path.display(), "quick prompt draft not saved: {err}");
        }
    }
}

/// The user changed the text of the box that is up: when it is a QUICK
/// PROMPT — the one box whose text is the SAVED DRAFT — write the text
/// down under its place, and let its `draft restored` cue go: the text is
/// simply what is being typed now.
pub(crate) fn note_edit(app: &mut App) {
    let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
        return;
    };
    let PromptKind::QuickPrompt(launch) = &prompt.kind else {
        return;
    };
    prompt.draft_restored = false;
    if let Some(saved) = &mut app.saved_draft {
        saved.save(&DraftPlace::of(&launch.target), prompt.input.as_str());
    }
}

/// The field a fresh box aimed at `target` opens on when its DRAFT slot is
/// empty: its place's SAVED DRAFT, the caret at its end. None with nothing
/// saved there (or no store).
pub(crate) fn restore(app: &App, target: &QuickTarget) -> Option<TextInput> {
    let text = app.saved_draft.as_ref()?.text(&DraftPlace::of(target));
    (!text.is_empty()).then(|| TextInput::multiline_with_text(text))
}

/// A box holding `text` was handed back aimed at `target` — a picker may
/// have re-aimed it: the draft it wrote down goes with it.
pub(crate) fn followed(app: &mut App, target: &QuickTarget, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(saved) = &mut app.saved_draft {
        saved.moved(text, &DraftPlace::of(target));
    }
}

/// A QUICK PROMPT aimed at `target` holding `text` just launched: when
/// that is its place's draft, it is spent. A box sent with other text — a
/// `skip_task` preset's empty one, a PR box's own — leaves the draft for
/// the next box.
pub(crate) fn launched(app: &mut App, target: &QuickTarget, text: &str) {
    let place = DraftPlace::of(target);
    if let Some(saved) = app.saved_draft.as_mut().filter(|s| s.text(&place) == text) {
        saved.clear(&place);
    }
}

/// A QUICK PROMPT launch aimed at `target` the DAEMON refused came back
/// with `text`: when the launch spent its place's draft, `text` is the
/// draft again.
pub(crate) fn refused(app: &mut App, target: &QuickTarget, text: &str) {
    let place = DraftPlace::of(target);
    if let Some(saved) = app
        .saved_draft
        .as_mut()
        .filter(|s| s.text(&place).is_empty())
    {
        saved.save(&place, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(id: &str) -> DraftPlace {
        DraftPlace::Worktree(WorktreeId(id.into()))
    }

    #[test]
    fn a_draft_round_trips_and_an_unchanged_text_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut saved = SavedDraft::at(path.clone());
        assert_eq!(saved.text(&wt("w1")), "");
        saved.save(&wt("w1"), "fix the login\nredirect");
        assert_eq!(
            SavedDraft::at(path.clone()).text(&wt("w1")),
            "fix the login\nredirect"
        );
        // The same text again is no write: a file swapped in behind the
        // store's back is left alone.
        std::fs::write(&path, "elsewhere").unwrap();
        saved.save(&wt("w1"), "fix the login\nredirect");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "elsewhere");
        // No temp file left beside it.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(FILE_NAME)]);
    }

    /// Every place keeps its own draft: a worktree's is not its sibling's,
    /// nor its project's fresh-worktree box's.
    #[test]
    fn each_place_keeps_its_own_draft() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let project = DraftPlace::Project(ProjectId("p1".into()));
        let mut saved = SavedDraft::at(path.clone());
        saved.save(&wt("w1"), "for main");
        saved.save(&wt("w2"), "for feat");
        saved.save(&project, "for a fresh one");
        let back = SavedDraft::at(path.clone());
        assert_eq!(back.text(&wt("w1")), "for main");
        assert_eq!(back.text(&wt("w2")), "for feat");
        assert_eq!(back.text(&project), "for a fresh one");
        assert_eq!(back.text(&wt("w3")), "");

        saved.clear(&wt("w2"));
        assert_eq!(SavedDraft::at(path.clone()).text(&wt("w1")), "for main");
        assert_eq!(SavedDraft::at(path).text(&wt("w2")), "");
    }

    /// A re-aimed box's draft moves with it; a text nobody holds moves
    /// nothing.
    #[test]
    fn a_draft_moves_to_where_its_box_went() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = SavedDraft::at(dir.path().join(FILE_NAME));
        saved.save(&wt("w1"), "typed on main");
        saved.save(&wt("w3"), "other");
        saved.moved("typed on main", &wt("w2"));
        assert_eq!(saved.text(&wt("w1")), "");
        assert_eq!(saved.text(&wt("w2")), "typed on main");
        saved.moved("nobody's", &wt("w1"));
        assert_eq!(saved.text(&wt("w1")), "");
        assert_eq!(saved.text(&wt("w3")), "other");
    }

    #[test]
    fn an_empty_or_blank_text_removes_the_entry_and_the_last_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut saved = SavedDraft::at(path.clone());
        saved.save(&wt("w1"), "draft");
        saved.save(&wt("w1"), "  \n ");
        assert!(!path.exists());
        assert_eq!(saved.text(&wt("w1")), "");
        saved.clear(&wt("w1"));
        std::fs::write(&path, r#"{"worktree:w1": " \n"}"#).unwrap();
        assert_eq!(
            SavedDraft::at(path.clone()).text(&wt("w1")),
            "",
            "a blank entry is no draft"
        );
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(SavedDraft::at(path).text(&wt("w1")), "", "nor is junk");
    }
}
