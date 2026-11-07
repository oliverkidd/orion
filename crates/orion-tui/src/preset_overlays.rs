//! The AGENT PRESETS overlays: the PICKER — the list that only picks a
//! preset, opened as a QUICK PROMPT's `⌘U` (the box's preset, written into
//! its text), over an agent's LOCKED PANE (`⌘U`, pasted at its caret), on
//! a pull request (a PR SESSION on it) and from the palette's **Agent
//! presets** (a launch into the checkout under the cursor) — and the
//! PRESET EDITOR, the form Settings → Presets (`presets_tab`) opens over
//! itself to make or change one. Their state, keys, mouse and drawing.
//! Presets are made, changed and deleted only in Settings: no picker
//! offers it. The presets themselves (and their file) are
//! `crate::agent_presets`; a launch goes through the QUICK PROMPT with the
//! preset on it — sent on at once, empty, for a `skip_task` preset
//! (`event_loop::submit_prompt_now`) — and the create it ends in goes
//! through `event_loop::create_agent` like every other session.

use crate::agent_presets::{AgentPreset, SystemPreset};
use crate::app::{clamp_selection, window_start, App, Focus, Overlay, PromptKind};
use crate::quick_prompt::QuickTarget;
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect, draw_multiline_input, draw_scroll_marks, empty_list_row, fuzzy_highlight_spans,
    input_spans, modal_block, render_row, row_rect, truncate, visible_positions,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use orion_core::{AgentKind, ClientRequest, WorktreeId};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

const AGENT_PRESETS_W: u16 = 72;
/// The PRESET EDITOR's width on a narrow screen (narrower still on one
/// under 80 columns), and the widest it grows to on a roomy one, where
/// the PREVIEW sits beside the text box.
const PRESET_EDITOR_W: u16 = 76;
const PRESET_EDITOR_WIDE: u16 = 118;
/// The inner width from which the text box and the PREVIEW sit side by
/// side; narrower, the preview goes under the box.
const SIDE_BY_SIDE: u16 = 96;
/// The tallest the editor draws — its rows, then the boxes, room for a
/// screenful of text — and the shortest.
const PRESET_EDITOR_H: u16 = 34;
const PRESET_EDITOR_MIN_H: u16 = 12;
/// The fewest rows a box is drawn in, its borders included.
const BOX_MIN_H: u16 = 3;
/// The text box's share of the rows when the PREVIEW goes under it.
const TEXT_SHARE: (u16, u16) = (3, 5);
/// The screen's rows and columns the editor leaves round it.
const SCREEN_MARGIN: (u16, u16) = (2, 4);
/// What the text box's border says of its marker.
const TEXT_TITLE: &str = " Text — {task} is where your task goes ";
const PREVIEW_TITLE: &str = " Preview — what the agent gets ";
/// Rows of the PREVIEW kept in sight above its first sample.
const PREVIEW_CONTEXT: usize = 3;
/// The Task row's two choices: ask for an (optional) task on launch, or
/// launch at once (`AgentPreset::skip_task`).
const TASK_ASK: &str = "ask";
const TASK_SKIP: &str = "skip";

/// The AGENT PRESETS PICKER: every saved preset, snapshot from the store
/// when the modal opens. Letters type ahead over the names (`filter`), as
/// in the MODEL / EFFORT submenus. Enter picks the selected one: launched
/// into `worktree` through its task box — at once for a `skip_task`
/// preset — or, as a QUICK PROMPT's picker, handed to that box. It only
/// picks: making, changing and deleting presets is Settings → Presets'.
#[derive(Debug, Clone)]
pub struct AgentPresetsView {
    pub presets: Vec<AgentPreset>,
    /// Cursor into `presets` — the stored index, whatever the filter shows,
    /// so the launch reads the right row.
    pub selected: usize,
    /// The type-ahead typed so far: the rows narrow to the names it fuzzy
    /// matches, best match first (`visible`). Empty shows every preset in
    /// its stored order.
    pub filter: String,
    /// The WORKTREE a launch lands in — the checkout under the cursor
    /// when the list opened, or the picker's context checkout.
    pub worktree: WorktreeId,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    /// Screen rect of the preset rows, written back during draw so clicks
    /// can hit-test rows.
    pub list_area: Rect,
    /// Set when the list was opened as a QUICK PROMPT picker (`⌘U` in the
    /// box or over a pane, on a pull request): the box to put back, with
    /// the text typed so far. In that mode Enter applies the row to that
    /// launch and Esc returns unchanged.
    pub quick: Option<crate::quick_prompt::QuickReturn>,
    /// Where the launch lands once `Tab` (or a click on the list's
    /// `[ ] new worktree` row) has flipped it: a fresh worktree, or back
    /// to a checkout. None is where the list was opened for — `worktree`
    /// in the launcher, the launch's own target in a picker — so Esc on a
    /// picker still hands its box back unchanged.
    pub aim: Option<QuickTarget>,
    /// Screen rect of the NEW WORKTREE row, written back during draw so a
    /// click there flips it as `Tab` does.
    pub toggle_area: Rect,
}

impl AgentPresetsView {
    /// The list over `presets`, each as it launches: a SYSTEM PRESET on
    /// orion's text carries that text.
    pub fn new(worktree: WorktreeId, presets: Vec<AgentPreset>) -> Self {
        Self {
            presets: presets
                .into_iter()
                .map(AgentPreset::with_builtin_text)
                .collect(),
            selected: 0,
            filter: String::new(),
            worktree,
            area: Rect::default(),
            list_area: Rect::default(),
            quick: None,
            aim: None,
            toggle_area: Rect::default(),
        }
    }

    /// Where Enter launches: the flipped `aim`, else the picker's launch
    /// target, else the launcher's `worktree`.
    pub fn target(&self) -> QuickTarget {
        self.aim
            .clone()
            .or_else(|| self.quick.as_ref().map(|q| q.launch.target.clone()))
            .unwrap_or_else(|| QuickTarget::Worktree(self.worktree.clone()))
    }

    /// Does Enter cut a fresh worktree first?
    pub fn is_new_worktree(&self) -> bool {
        matches!(self.target(), QuickTarget::NewWorktree { .. })
    }

    /// The picker's box with the flipped `aim` applied — what Enter hands
    /// over.
    pub fn aimed_quick(&self) -> Option<crate::quick_prompt::QuickReturn> {
        let mut back = self.quick.clone()?;
        if let Some(aim) = &self.aim {
            back.launch.target = aim.clone();
        }
        Some(back)
    }

    /// Is this list a QUICK PROMPT picker rather than the launcher — the
    /// palette's **Agent presets**, launching into `worktree`?
    pub fn is_picker(&self) -> bool {
        self.quick.is_some()
    }

    /// The box Esc puts back: only a picker the box itself opened has one
    /// (`QuickReturn::from_box`).
    pub fn box_behind(&self) -> Option<&crate::quick_prompt::QuickReturn> {
        self.quick.as_ref().filter(|q| q.from_box)
    }

    /// The agent whose prompt a pick is pasted into, when the list was
    /// opened from its LOCKED PANE (`QuickReturn::into_pane`).
    pub fn pane_target(&self) -> Option<&orion_core::SessionRef> {
        self.quick.as_ref()?.into_pane.as_ref()
    }

    /// Does the list carry its own NEW WORKTREE row and `Tab`? Not over an
    /// agent's pane, where nothing is launched, and not over the QUICK
    /// PROMPT, whose header names the checkout and whose WORKTREE PICKER
    /// (`⌘.`) has a fresh worktree as its first row: a second toggle for the
    /// same choice under the box was the clutter the box lost its own for.
    pub fn has_worktree_row(&self) -> bool {
        self.box_behind().is_none() && self.pane_target().is_none()
    }

    /// The rows `filter` leaves, top to bottom: indices into `presets`,
    /// each with the name's matched char positions (lit when drawn). Worked
    /// out afresh on every call rather than kept — a handful of names — so
    /// it can never go stale against `presets`.
    pub fn visible(&self) -> Vec<(usize, Vec<usize>)> {
        crate::fuzzy::rank(&self.filter, self.presets.iter().map(|p| p.name.as_str()))
    }

    /// Where the cursor sits among the visible rows: the top one when the
    /// filter hides `selected`, the last when an unfiltered `selected` has
    /// run past the end.
    fn cursor(&self, visible: &[(usize, Vec<usize>)]) -> usize {
        visible
            .iter()
            .position(|(i, _)| *i == self.selected)
            .unwrap_or(if self.filter.is_empty() {
                self.selected.min(visible.len().saturating_sub(1))
            } else {
                0
            })
    }

    /// ↑/↓: move `delta` rows through the visible ones.
    pub fn step(&mut self, delta: i64) {
        let visible = self.visible();
        let at = clamp_selection(self.cursor(&visible) as i64 + delta, visible.len());
        if let Some((index, _)) = visible.get(at) {
            self.selected = *index;
        }
    }

    /// The row `pos` rows down the visible list (a click, a wheel), as an
    /// index into `presets`.
    pub fn visible_row(&self, pos: usize) -> Option<usize> {
        self.visible().get(pos).map(|(index, _)| *index)
    }

    /// Narrow to `query`, the cursor on its best match. False — and
    /// nothing changes — when no name matches, so the list never empties
    /// under the user (as in the MODEL / EFFORT submenus). An empty query
    /// shows every row and leaves the cursor where it is: the preset just
    /// found stays selected once Esc clears the letters that found it.
    pub fn set_filter(&mut self, query: &str) -> bool {
        let ranked = crate::fuzzy::rank(query, self.presets.iter().map(|p| p.name.as_str()));
        let Some((best, _)) = ranked.first() else {
            return false;
        };
        if !query.trim().is_empty() {
            self.selected = *best;
        }
        self.filter = query.to_string();
        true
    }

    /// One typed character onto the filter; false when it would leave no
    /// rows (nothing changes).
    pub fn type_filter(&mut self, c: char) -> bool {
        self.set_filter(&format!("{}{c}", self.filter))
    }

    /// Backspace: drop the last character (widens, so it always succeeds).
    pub fn pop_filter(&mut self) {
        let mut query = self.filter.clone();
        query.pop();
        self.set_filter(&query);
    }

    /// First visible row of the list's stateless follow-window for a list of
    /// `height` rows.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.cursor(&self.visible()), height)
    }
}

/// One field of the PRESET EDITOR, in Tab order: the rows, then the text
/// box under them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetField {
    Name,
    Kind,
    Model,
    Effort,
    Task,
    /// The one text box: what the preset sends, `{task}` where the task
    /// goes.
    Text,
}

use crate::config::fits;

impl PresetField {
    pub const ALL: [PresetField; 6] = [
        PresetField::Name,
        PresetField::Kind,
        PresetField::Model,
        PresetField::Effort,
        PresetField::Task,
        PresetField::Text,
    ];

    /// The field `delta` steps away in Tab order, wrapping, and skipping
    /// what the form has no row for: an Effort the (harness, model) pair
    /// has no choice for — a Cursor family without effort variants, or no
    /// Cursor model yet.
    pub fn step(self, editor: &AgentPresetEditor, delta: i32) -> PresetField {
        let n = Self::ALL.len() as i32;
        let mut pos = Self::ALL.iter().position(|f| *f == self).unwrap_or(0) as i32;
        for _ in 0..n {
            pos = (pos + delta).rem_euclid(n);
            let next = Self::ALL[pos as usize];
            if next.available(editor) {
                return next;
            }
        }
        self
    }

    /// Whether the form offers the field at all: Model and Effort follow
    /// the harness (with its model chosen); a SYSTEM PRESET's name is
    /// orion's, never renamed, and its task orion's to supply, so neither
    /// is a field there.
    pub fn available(self, editor: &AgentPresetEditor) -> bool {
        let (kind, custom) = (editor.kind, editor.custom.as_deref());
        match self {
            PresetField::Name | PresetField::Task => editor.system.is_none(),
            PresetField::Model => !crate::config::model_choices(kind, custom).is_empty(),
            PresetField::Effort => {
                !crate::config::effort_choices(kind, Some(&editor.model), custom).is_empty()
            }
            _ => true,
        }
    }

    /// Name and Text take typed text; the rest cycle a choice.
    pub fn is_typed(self) -> bool {
        matches!(self, PresetField::Name | PresetField::Text)
    }

    pub fn label(self) -> &'static str {
        match self {
            PresetField::Name => "Name",
            PresetField::Kind => "Runs on",
            PresetField::Model => "Model",
            PresetField::Effort => "Effort",
            PresetField::Task => "Task",
            PresetField::Text => "Text",
        }
    }
}

/// Why the last Enter in the PRESET EDITOR saved nothing: the message its
/// banner shows, and the field the banner points at — drawn in the error
/// color, with the caret moved onto it, until its text changes. Only the
/// Name row can fail today (`AgentPresetEditor::validate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormError {
    pub field: PresetField,
    pub message: String,
}

/// The PRESET EDITOR: one form for creating (`editing == None`) or editing
/// (`editing == Some(index)`) an AGENT PRESET, over Settings → Presets.
/// Model and effort hold a choice label from `config::model_choices` /
/// `effort_choices`, so `DEFAULT_CHOICE` means "follow Settings →
/// Accounts" and becomes None on save.
#[derive(Debug, Clone)]
pub struct AgentPresetEditor {
    /// The Settings → Presets row the form was opened from (`presets_tab`):
    /// Esc lands back on it, and a save on the saved preset's row.
    pub settings_row: usize,
    /// The SYSTEM PRESET being edited, if it is one: its name and task are
    /// orion's, and its empty text box shows orion's text greyed — what it
    /// sends while the box holds none of the user's.
    pub system: Option<SystemPreset>,
    /// Index into the stored list when editing; None when creating.
    pub editing: Option<usize>,
    pub name: TextInput,
    /// FOLLOW THE BOX (`AgentPreset::follow_box`): the Runs on row's
    /// first choice, and where every preset starts — the PR autofix
    /// preset's reads the PR's own agent. Off, `kind` and `custom` are the
    /// harness it pins.
    pub follow: bool,
    pub kind: AgentKind,
    /// The registry id beside `kind`: a Claude account's, a custom
    /// harness's.
    pub custom: Option<String>,
    pub model: String,
    pub effort: String,
    /// What the preset sends (`AgentPreset::text`): `{task}` marks where
    /// the task goes.
    pub text: TextInput,
    /// Launch without asking for a task (`AgentPreset::skip_task`); the
    /// Task row shows it as `ask` / `skip`.
    pub skip_task: bool,
    /// The field with the caret / cycle focus.
    pub field: PresetField,
    /// Type-ahead on the focused choice row (Harness / Model / Effort): the
    /// letters typed so far; the row jumps to the best fuzzy match and ←/→
    /// cycle the matches only. Cleared on leaving the row.
    pub filter: String,
    /// Why the last Enter was refused, while it still applies: the banner
    /// at the top of the form and the field it colors. Cleared by the
    /// first change to that field's text — the fix is under way, and the
    /// next Enter judges the new text afresh.
    pub error: Option<FormError>,
    /// Whole modal rect, written back during draw so a click outside can
    /// back out like Esc.
    pub area: Rect,
}

impl AgentPresetEditor {
    /// A blank form opened on Settings → Presets' row `from`: following
    /// the box at the configured defaults, the text box empty, caret on
    /// the name.
    pub fn new(from: usize) -> Self {
        Self {
            settings_row: from,
            system: None,
            editing: None,
            name: TextInput::new(),
            follow: true,
            kind: AgentKind::Claude,
            custom: None,
            model: crate::config::DEFAULT_CHOICE.into(),
            effort: crate::config::DEFAULT_CHOICE.into(),
            text: TextInput::multiline(),
            skip_task: false,
            field: PresetField::Name,
            area: Rect::default(),
            filter: String::new(),
            error: None,
        }
    }

    /// The form pre-filled from a stored preset. A SYSTEM PRESET on
    /// orion's text opens with its box empty, orion's text greyed in it,
    /// and the caret on its first field, the harness.
    pub fn from_preset(from: usize, index: usize, preset: &AgentPreset) -> Self {
        let choice = |v: &Option<String>| {
            v.clone()
                .unwrap_or_else(|| crate::config::DEFAULT_CHOICE.into())
        };
        Self {
            settings_row: from,
            system: preset.system,
            editing: Some(index),
            name: TextInput::with_text(preset.name.clone()),
            follow: preset.follows_box(),
            kind: preset.kind,
            custom: preset.custom_harness.clone(),
            model: choice(&preset.model),
            effort: choice(&preset.effort),
            text: TextInput::multiline_with_text(preset.text.clone()),
            skip_task: preset.skip_task,
            field: match preset.system {
                Some(_) => PresetField::Kind,
                None => PresetField::Name,
            },
            area: Rect::default(),
            filter: String::new(),
            error: None,
        }
    }

    /// orion's own text, while the form is a SYSTEM PRESET's.
    pub fn builtin_text(&self) -> Option<&'static str> {
        self.system.map(SystemPreset::builtin_text)
    }

    /// Whether `^R` has anything to do: a SYSTEM PRESET whose box holds
    /// other than orion's text — nothing, to start from it, or the user's,
    /// to put it back.
    pub fn can_restore(&self) -> bool {
        self.builtin_text()
            .is_some_and(|builtin| self.text.trim() != builtin.trim())
    }

    /// `^R`: orion's text into the box, the caret at its end — to edit
    /// from, or to send as it stands. Nothing is saved until Enter, and
    /// Esc leaves the preset as it was.
    pub fn restore_text(&mut self) {
        if let Some(builtin) = self.builtin_text().filter(|_| self.can_restore()) {
            self.text.set_text(builtin);
            self.field = PresetField::Text;
            self.filter.clear();
        }
    }

    pub fn is_edit(&self) -> bool {
        self.editing.is_some()
    }

    /// The Task row's value: `ask` or `skip`.
    fn task_choice(&self) -> &'static str {
        if self.skip_task {
            TASK_SKIP
        } else {
            TASK_ASK
        }
    }

    /// Switch harness, dropping a model / effort the new kind doesn't list
    /// back to the default so the form never holds a choice it can't show.
    pub fn set_kind(&mut self, kind: AgentKind, custom: Option<String>) {
        self.kind = kind;
        self.custom = custom;
        if !fits(
            &self.model,
            &crate::config::model_choices(kind, self.custom.as_deref()),
        ) {
            self.model = crate::config::DEFAULT_CHOICE.into();
        }
        self.fit_effort();
        if !self.field.available(self) {
            self.field = self.field.step(self, 1);
        }
    }

    /// Drop an effort the current (harness, model) pair doesn't list — a
    /// composing harness's list follows the family — to what "default"
    /// would launch: the family's fallback for a family with no bare id,
    /// else default.
    fn fit_effort(&mut self) {
        let choices =
            crate::config::effort_choices(self.kind, Some(&self.model), self.custom.as_deref());
        if !fits(&self.effort, &choices) {
            self.effort = crate::config::fit_effort(
                self.kind,
                Some(&self.model),
                None,
                self.custom.as_deref(),
            )
            .unwrap_or_else(|| crate::config::DEFAULT_CHOICE.into());
        }
    }

    /// The choices the focused row cycles: harness names, the kind's
    /// models, the (kind, model) pair's efforts, or `ask` / `skip`. Empty
    /// on a typed row and on an `n/a` row.
    fn row_choices(&self) -> Vec<String> {
        match self.field {
            // Every offered harness by id — built-ins and customs alike,
            // in registry order. Never a bare `custom`, which carries no
            // entry.
            PresetField::Kind => std::iter::once(self.follow_label().to_string())
                .chain(
                    crate::config::Config::load()
                        .offered_harnesses()
                        .into_iter()
                        .map(|(kind, custom)| {
                            orion_core::harness::registry_id(kind, custom.as_deref()).to_string()
                        }),
                )
                .collect(),
            PresetField::Model => crate::config::model_choices(self.kind, self.custom.as_deref()),
            PresetField::Effort => {
                crate::config::effort_choices(self.kind, Some(&self.model), self.custom.as_deref())
            }
            PresetField::Task => vec![TASK_ASK.to_string(), TASK_SKIP.to_string()],
            _ => Vec::new(),
        }
    }

    fn row_value(&self) -> String {
        match self.field {
            PresetField::Kind => self.harness_value(),
            PresetField::Model => self.model.clone(),
            PresetField::Effort => self.effort.clone(),
            PresetField::Task => self.task_choice().to_string(),
            _ => String::new(),
        }
    }

    /// What following means for this preset
    /// (`AgentPreset::follow_label`): the Runs on row's first choice.
    pub fn follow_label(&self) -> &'static str {
        crate::agent_presets::follow_label(self.system)
    }

    /// What the Runs on row says: [`Self::follow_label`], or the registry
    /// id of the harness the form pins.
    pub fn harness_value(&self) -> String {
        match self.follow {
            true => self.follow_label().to_string(),
            false => orion_core::harness::registry_id(self.kind, self.custom.as_deref()).into(),
        }
    }

    fn set_row_value(&mut self, value: &str) {
        match self.field {
            PresetField::Kind => {
                if value == self.follow_label() {
                    // Saved on the default Claude account, the harness an
                    // older release reads a following preset as.
                    self.follow = true;
                    self.set_kind(AgentKind::Claude, None);
                } else if let Some(entry) = crate::config::Config::load()
                    .harness_registry()
                    .iter()
                    .find(|entry| entry.id == value)
                {
                    let (kind, custom) = entry.session_kind();
                    self.follow = false;
                    self.set_kind(kind, custom);
                }
            }
            PresetField::Model => {
                self.model = value.to_string();
                self.fit_effort();
            }
            PresetField::Effort => self.effort = value.to_string(),
            PresetField::Task => self.skip_task = value == TASK_SKIP,
            _ => {}
        }
    }

    /// The row's choices narrowed to the filter, best match first; all of
    /// them, in list order, while nothing is typed.
    pub fn filtered_choices(&self) -> Vec<String> {
        let all = self.row_choices();
        crate::fuzzy::rank(&self.filter, all.iter().map(String::as_str))
            .into_iter()
            .map(|(i, _)| all[i].clone())
            .collect()
    }

    /// A typed letter on a choice row: extend the filter and jump to its
    /// best match. False — and nothing changes — when no choice would
    /// match, so the row always shows something the filter names.
    pub fn type_filter(&mut self, c: char) -> bool {
        let query = format!("{}{c}", self.filter);
        let all = self.row_choices();
        let Some((best, _)) = crate::fuzzy::rank(&query, all.iter().map(String::as_str))
            .into_iter()
            .next()
        else {
            return false;
        };
        self.filter = query;
        let best = all[best].clone();
        self.set_row_value(&best);
        true
    }

    /// Backspace on a choice row: shorten the filter; the value stays
    /// unless it no longer matches, then the best match takes over.
    pub fn pop_filter(&mut self) {
        self.filter.pop();
        let matches = self.filtered_choices();
        let current = self.row_value();
        if !matches.iter().any(|m| m.eq_ignore_ascii_case(&current)) {
            if let Some(first) = matches.first().cloned() {
                self.set_row_value(&first);
            }
        }
    }

    /// `←` / `→` on a choice field: rotate it by `delta` — through the
    /// filter's matches while one is typed, else the whole list.
    pub fn cycle(&mut self, delta: i32) {
        if self.field.is_typed() {
            return;
        }
        let choices = self.filtered_choices();
        if choices.is_empty() {
            return;
        }
        let next = crate::config::cycle_owned(&self.row_value(), &choices, delta);
        self.set_row_value(&next);
    }

    /// The text input under the caret, when the focused field is one.
    pub fn text_field(&self) -> Option<&TextInput> {
        match self.field {
            PresetField::Name => Some(&self.name),
            PresetField::Text => Some(&self.text),
            _ => None,
        }
    }

    /// The text input under the caret, when the focused field is one.
    pub fn text_field_mut(&mut self) -> Option<&mut TextInput> {
        match self.field {
            PresetField::Name => Some(&mut self.name),
            PresetField::Text => Some(&mut self.text),
            _ => None,
        }
    }

    /// A key on the focused text field. A change to the field the banner
    /// points at takes the banner down: the fix is under way.
    pub fn edit_text(&mut self, key: &KeyEvent) {
        let field = self.field;
        let Some(input) = self.text_field_mut() else {
            return;
        };
        if input.handle_key(key).changed() {
            self.text_changed(field);
        }
    }

    /// A bracketed paste into the focused text field — lines kept in the
    /// text box, flattened in the one-line name — clearing a banner
    /// that points at it, as a typed key does. False when a choice row has
    /// the caret and there is nowhere to paste.
    pub fn paste(&mut self, text: &str) -> bool {
        let field = self.field;
        let Some(input) = self.text_field_mut() else {
            return false;
        };
        input.insert_str(text);
        self.text_changed(field);
        true
    }

    /// Enter was refused: keep the reason for the banner, and put the
    /// caret on the field it names — wherever it was — so the next
    /// keystroke is the fix.
    pub fn reject(&mut self, error: FormError) {
        self.filter.clear();
        self.field = error.field;
        self.error = Some(error);
    }

    /// The text of `field` changed: a banner pointing at it comes down.
    /// One pointing elsewhere stays — that field is still the problem.
    fn text_changed(&mut self, field: PresetField) {
        if self.error.as_ref().is_some_and(|e| e.field == field) {
            self.error = None;
        }
    }

    /// The preset the form describes right now (name trimmed, default
    /// choices folded to None).
    pub fn to_preset(&self) -> AgentPreset {
        AgentPreset {
            name: self.name.trim().to_string(),
            kind: self.kind,
            custom_harness: self.custom.clone(),
            follow_box: Some(self.follow),
            model: crate::config::non_default(&self.model),
            effort: crate::config::non_default(&self.effort),
            text: self.text.as_str().to_string(),
            skip_task: self.skip_task,
            // The list's own, kept on save (`save_agent_preset_editor`).
            system: None,
        }
    }

    /// The preset to save, or why it can't be — pointing at the Name row:
    /// blank, or a name another preset (not the one being edited) already
    /// uses, case-insensitively.
    pub fn validate(&self, presets: &[AgentPreset]) -> Result<AgentPreset, FormError> {
        let preset = self.to_preset();
        let name_error = |message: String| FormError {
            field: PresetField::Name,
            message,
        };
        if preset.name.is_empty() {
            return Err(name_error("the preset needs a name".into()));
        }
        let taken = presets
            .iter()
            .enumerate()
            .any(|(i, p)| Some(i) != self.editing && p.name.eq_ignore_ascii_case(&preset.name));
        if taken {
            return Err(name_error(format!(
                "a preset named '{}' already exists",
                preset.name
            )));
        }
        Ok(preset)
    }
}

/// The palette's **Agent presets**: the PICKER for the checkout under the
/// cursor — Enter launches the preset there — or, with the cursor on a
/// pull request, the picker for a PR SESSION on it
/// (`quick_prompt::open_preset_picker_for_pr`), since a pull request has
/// no checkout of its own to launch into until one is cut. Anywhere else
/// there is nothing to launch into, and it does nothing.
pub(crate) fn open_agent_presets(app: &mut App) {
    if app.selected_worktree_pr().is_some() {
        crate::quick_prompt::open_preset_picker_for_pr(app);
        return;
    }
    let worktree = match (app.focus, app.selected_worktree()) {
        (Focus::Sessions | Focus::Worktrees, Some(w)) => w.id.clone(),
        _ => return,
    };
    open_launcher(app, worktree);
}

/// The PICKER as the launcher for `worktree`, read from the store.
pub(crate) fn open_launcher(app: &mut App, worktree: WorktreeId) {
    let view = AgentPresetsView::new(worktree, crate::agent_presets::load());
    app.overlay = Some(Overlay::AgentPresets(view));
}

/// The PRESET EDITOR over Settings → Presets, opened on row `from`:
/// pre-filled from `editing` — a preset and its index in
/// `agent_presets::load_seeded`, as the tab reads it — or blank for a new
/// one.
pub(crate) fn open_editor_from_settings(
    app: &mut App,
    from: usize,
    editing: Option<(usize, &AgentPreset)>,
) {
    let editor = match editing {
        Some((index, preset)) => AgentPresetEditor::from_preset(from, index, preset),
        None => AgentPresetEditor::new(from),
    };
    app.overlay = Some(Overlay::AgentPresetEditor(editor));
}

/// Enter in the PRESET EDITOR: validate against the stored list, write the
/// row (in place when editing, appended when new), and land back on it in
/// Settings → Presets. A rejected form stays open, the reason on a banner
/// at the top of it and the caret on the field to fix
/// (`AgentPresetEditor::reject`) — not in the FOOTER, which is nowhere
/// near the form.
pub(crate) fn save_agent_preset_editor(app: &mut App, mut editor: AgentPresetEditor) {
    let mut presets = crate::agent_presets::load_seeded();
    let preset = match editor.validate(&presets) {
        Ok(preset) => preset,
        Err(error) => {
            editor.reject(error);
            app.overlay = Some(Overlay::AgentPresetEditor(editor));
            return;
        }
    };
    let index = match editor.editing {
        Some(index) if index < presets.len() => {
            // A SYSTEM PRESET stays one, under its own name: orion finds
            // it by what it is for, and the list by what it is called. Its
            // text, left as orion's or cleared, is kept as nothing, which
            // reads as orion's from then on.
            let mut preset = preset;
            if let Some(which) = presets[index].system {
                preset.system = Some(which);
                preset.name = presets[index].name.clone();
            }
            presets[index] = preset.without_builtin_text();
            index
        }
        _ => {
            presets.push(preset);
            presets.len() - 1
        }
    };
    crate::presets_tab::save(app, &presets);
    let at = crate::presets_tab::Screen::of(presets).row_of(index);
    crate::presets_tab::reopen(app, at.unwrap_or(0));
}

/// The list's Enter: ask for the optional task the preset's text will
/// carry — or, for a `skip_task` preset, launch on the text at once,
/// through the same submit an empty task box takes. A harness switched off
/// in Settings → Accounts is refused here, where the row is, rather than by a
/// failed spawn later.
pub(crate) fn open_agent_preset_task(
    app: &mut App,
    view: &AgentPresetsView,
    out: &mut Vec<ClientRequest>,
) {
    let Some(preset) = view.presets.get(view.selected).cloned() else {
        return;
    };
    if !crate::config::Config::load().preset_harness_usable(&preset) {
        app.flash = Some(turned_off(&preset));
        return;
    }
    let skip = preset.skip_task;
    // The QUICK PROMPT with the preset on it, aimed where the list was
    // opened for or where `Tab` flipped it — sent at once for a
    // `skip`-task preset, as Enter on that box would.
    let target = match view.aim.clone() {
        Some(target @ QuickTarget::NewWorktree { .. }) => target,
        _ => QuickTarget::Worktree(view.worktree.clone()),
    };
    let launch = crate::quick_prompt::QuickLaunch::of_preset(
        target,
        preset,
        None,
        &crate::config::Config::load(),
    );
    if skip {
        crate::event_loop::submit_prompt_now(app, PromptKind::QuickPrompt(launch), out);
    } else {
        let (launch, head, tail) = launch.inline_preset("", None);
        crate::quick_prompt::reopen_around(app, launch, &head, &tail);
    }
}

/// What the footer says of a preset whose harness can't launch.
fn turned_off(preset: &AgentPreset) -> crate::flash::Flash {
    crate::flash::Flash::setup(format!(
        "{} is turned off in Settings → Accounts",
        orion_core::harness::registry_id(preset.kind, preset.custom_harness.as_deref())
    ))
}

/// The picker's Enter: adopt the hovered AGENT PRESET — its harness,
/// MODEL / EFFORT and text — for the QUICK PROMPT that opened the list,
/// and hand the box back with that text written round its own (`QuickLaunch::inline_preset`) — or, for a `skip_task`
/// preset picked over an empty box, launch it as Enter on that box would;
/// typed text stays the user's to send. A harness switched off in
/// Settings → Accounts is refused here, where the row is, rather than by a
/// failed spawn later.
fn apply_preset_to_quick_prompt(
    app: &mut App,
    presets: &[AgentPreset],
    selected: usize,
    back: crate::quick_prompt::QuickReturn,
    out: &mut Vec<ClientRequest>,
) {
    let Some(preset) = presets.get(selected).cloned() else {
        return;
    };
    let cfg = crate::config::Config::load();
    if !cfg.preset_harness_usable(&preset) {
        app.flash = Some(turned_off(&preset));
        return;
    }
    if preset.skip_task && back.text.trim().is_empty() {
        let launch = preset_on_box(back, preset, &cfg);
        crate::event_loop::submit_prompt_now(app, PromptKind::QuickPrompt(launch), out);
    } else {
        reopen_box_with_preset(app, back, preset, &cfg);
    }
}

/// Hand the box `back` again with `preset` adopted and its text written
/// round the box's (`QuickLaunch::inline_preset`), the
/// caret where the task goes. The preset the box was on before, as saved,
/// has its text in the box already: a swap takes that out first.
fn reopen_box_with_preset(
    app: &mut App,
    back: crate::quick_prompt::QuickReturn,
    preset: AgentPreset,
    cfg: &crate::config::Config,
) {
    let replacing = back.launch.preset.as_ref().and_then(|on| {
        crate::agent_presets::load()
            .into_iter()
            .find(|saved| saved.name == on.name)
            .map(AgentPreset::with_builtin_text)
    });
    let text = back.text.clone();
    let (launch, head, tail) =
        preset_on_box(back, preset, cfg).inline_preset(&text, replacing.as_ref());
    crate::quick_prompt::reopen_around(app, launch, &head, &tail);
}

/// The box `back` with `preset` adopted — its harness, MODEL / EFFORT and
/// text — keeping where it lands, its pull request, Linear
/// batch, todo, mode and the modal it stands on.
fn preset_on_box(
    back: crate::quick_prompt::QuickReturn,
    preset: AgentPreset,
    cfg: &crate::config::Config,
) -> crate::quick_prompt::QuickLaunch {
    crate::quick_prompt::QuickLaunch::of_preset(
        back.launch.target.clone(),
        preset,
        Some(&back.launch),
        cfg,
    )
    .with_pr(back.launch.pr)
    .with_linear(back.launch.linear)
    .with_todo(back.launch.todo)
    .with_mode(back.launch.mode, cfg)
    .with_under(back.launch.under)
}

/// Keys in the AGENT PRESETS PICKER. Letters type ahead over the names,
/// ↑/↓ move and Enter picks, as in every filtered overlay; nothing else
/// — the presets are managed in Settings → Presets.
pub(crate) fn handle_list_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::AgentPresets(view)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        // Two-stage, like every type-ahead: the letters first.
        _ if crate::nav::closes(&key) && !view.filter.is_empty() => view.filter.clear(),
        // Backing out of the box's own picker is a return trip, not a
        // close: the box comes back exactly as it left, text and all. A
        // picker reached with no box up (one on a pull request) closes as
        // the launcher does — it used to put up an empty box nobody asked
        // for — unless it stands on a modal the box would have, which it
        // leaves on its row.
        _ if crate::nav::closes(&key) => match view.box_behind().cloned() {
            Some(back) => crate::quick_prompt::reopen(app, back.launch, &back.text),
            None => match view.quick.clone().and_then(|back| back.launch.under) {
                Some(under) => under.reopen(app),
                None => app.overlay = None,
            },
        },
        // Letters type ahead and Enter picks, so the NEW WORKTREE toggle
        // is the one key left free on the list — wherever it has the row.
        _ if keys::WORKTREE.matches(&key) && view.has_worktree_row() => toggle_new_worktree(app),
        KeyCode::Down => view.step(1),
        KeyCode::Up => view.step(-1),
        KeyCode::Char('u') if ctrl => view.filter.clear(),
        _ if keys::LAUNCH.matches(&key) => activate_selected(app, out),
        // Type-ahead: Backspace widens; a letter narrows the rows to the
        // names it fuzzy matches, the cursor on the best. A letter no name
        // matches is refused, so the list never empties under the user.
        KeyCode::Backspace => view.pop_filter(),
        // A leading space would narrow nothing; it waits for a word.
        KeyCode::Char(' ') if view.filter.is_empty() => {}
        KeyCode::Char(c) if !ctrl => {
            view.type_filter(c);
        }
        _ => {}
    }
}

/// The selected row is chosen — Enter, or a click on it. In a QUICK PROMPT
/// picker the row is handed to the box waiting behind the list
/// (`view.quick`), which keeps that box's target, its text and its pull
/// request; in the launcher it launches into the list's worktree,
/// through the preset's task box or past it; over an agent's pane it is
/// pasted at that agent's caret. The one function both input
/// handlers call: when the click ran a copy of the launcher half whatever
/// the mode, a click in the PR SESSION picker — whose worktree is the ROOT
/// WORKTREE, there only to name the project — started a plain session in
/// the main checkout.
fn activate_selected(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::AgentPresets(view)) = &app.overlay else {
        return;
    };
    let view = view.clone();
    if let Some(session) = view.pane_target() {
        if let Some(preset) = view.presets.get(view.selected) {
            insert_preset_into_pane(app, preset, session, out);
        }
        return;
    }
    match view.aimed_quick() {
        Some(back) => apply_preset_to_quick_prompt(app, &view.presets, view.selected, back, out),
        None => open_agent_preset_task(app, &view, out),
    }
}

/// Paste `preset`'s text at the caret of the agent in the pane and hand the
/// keys back to it: nothing is sent, so the task can still be typed. Only
/// the text travels — the session already has its harness, model and
/// effort. A preset with no text stays on the list to say so.
fn insert_preset_into_pane(
    app: &mut App,
    preset: &AgentPreset,
    session: &orion_core::SessionRef,
    out: &mut Vec<ClientRequest>,
) {
    let text = preset.standalone_text();
    if text.is_empty() {
        app.flash = Some(crate::flash::Flash::setup(format!(
            "{} has no text to insert",
            preset.name
        )));
        return;
    }
    app.overlay = None;
    if !crate::event_loop::paste_unsent(app, session, &text, out) {
        app.flash = Some(crate::flash::Flash::failed(
            "this session isn't taking pasted text right now",
        ));
    }
}

/// `Tab` on the AGENT PRESETS list, and a click on its `[ ] new worktree`
/// row: flip where the launch lands between a fresh worktree and a
/// checkout — the launcher's between the worktree it was opened on and a
/// fresh one of its project, a picker's through `launcher::flipped_target`,
/// so the box it hands over is already aimed. A picker opened from the box
/// has no row to flip ([`AgentPresetsView::has_worktree_row`]). A PR
/// SESSION has nothing to flip: its checkout is the pull request's own.
/// INPUT PARITY: the key and the click both call this.
pub(crate) fn toggle_new_worktree(app: &mut App) {
    let Some(Overlay::AgentPresets(view)) = &app.overlay else {
        return;
    };
    let home = QuickTarget::Worktree(view.worktree.clone());
    let flipped = match view.aimed_quick() {
        Some(back) => crate::launcher::flipped_target(app, &back.launch),
        None if view.is_new_worktree() => Some(home.clone()),
        None => app
            .tree
            .worktrees
            .iter()
            .find(|w| w.id == view.worktree)
            .map(|w| {
                let taken = app.project_branches(&w.project_id);
                QuickTarget::NewWorktree {
                    project: w.project_id.clone(),
                    branch: crate::branch_name::random_name(&taken),
                    existing: false,
                }
            }),
    };
    if let Some(target) = flipped {
        if let Some(Overlay::AgentPresets(view)) = &mut app.overlay {
            let own = view
                .quick
                .as_ref()
                .map_or(home, |q| q.launch.target.clone());
            view.aim = (target != own).then_some(target);
        }
    }
    app.dirty = true;
}

/// Keys in the PRESET EDITOR: Tab order between fields, choice cycling,
/// text editing, save and back.
pub(crate) fn handle_editor_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::AgentPresetEditor(editor)) = &mut app.overlay else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        // Esc first clears a choice row's type-ahead, then backs out.
        _ if crate::nav::closes(&key) && !editor.filter.is_empty() => editor.filter.clear(),
        _ if keys::RESTORE.matches(&key) => editor.restore_text(),
        // Back to the Settings → Presets row it came from, unsaved.
        _ if crate::nav::closes(&key) => {
            let at = editor.settings_row;
            crate::presets_tab::reopen(app, at);
        }
        // ↓/↑ inside the text box walk its lines first, as in
        // Claude Code's prompt; past the last (or first) line — and on
        // every other row — they step fields, as Tab does. Leaving a
        // choice row drops its type-ahead.
        KeyCode::Tab | KeyCode::Down => {
            let walked = key.code == KeyCode::Down
                && editor
                    .text_field_mut()
                    .is_some_and(|input| input.handle_key(&key).consumed());
            if !walked {
                editor.filter.clear();
                editor.field = editor.field.step(editor, 1);
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            let walked = key.code == KeyCode::Up
                && editor
                    .text_field_mut()
                    .is_some_and(|input| input.handle_key(&key).consumed());
            if !walked {
                editor.filter.clear();
                editor.field = editor.field.step(editor, -1);
            }
        }
        // A line break in the text box — Shift+Enter, Option+Enter
        // or Ctrl+J, as in the task editor — is the field's own (the `_`
        // arm below); the guard keeps the save off a shifted Enter.
        KeyCode::Enter
            if !editor
                .text_field()
                .is_some_and(|input| input.takes_newline(&key)) =>
        {
            let editor = editor.clone();
            save_agent_preset_editor(app, editor);
        }
        KeyCode::Left if !editor.field.is_typed() => editor.cycle(-1),
        KeyCode::Right | KeyCode::Char(' ') if !editor.field.is_typed() => editor.cycle(1),
        // Letters on a choice row type ahead: the row jumps to the best
        // match and ←/→ cycle the matches. A letter nothing matches is
        // refused, so the row never shows a choice the filter denies.
        KeyCode::Backspace if !editor.field.is_typed() => editor.pop_filter(),
        KeyCode::Char(c) if !editor.field.is_typed() && !ctrl => {
            editor.type_filter(c);
        }
        _ => editor.edit_text(&key),
    }
}

/// Mouse in the AGENT PRESETS PICKER: the wheel moves the selection, a
/// click on a row picks it (rows are actions, as in the hosts picker), a
/// click outside the modal closes; everything else is swallowed.
/// A click is Enter on that row, in both modes: in a QUICK PROMPT picker it
/// hands the row to the box waiting behind the list (`view.quick`), which
/// keeps that box's target, text and pull request. It used to launch
/// the row into `view.worktree` whatever the mode — for a PR SESSION picker
/// that is the ROOT WORKTREE, which only names the project — so a click on
/// a `skip`-task preset started a plain session in the main checkout, with
/// no pull request and no checkout cut.
pub(crate) fn handle_list_mouse(
    app: &mut App,
    mouse: MouseEvent,
    mouse_pos: Position,
    out: &mut Vec<ClientRequest>,
) {
    let Some(Overlay::AgentPresets(view)) = &mut app.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            view.step(-1);
            app.dirty = true;
        }
        MouseEventKind::ScrollDown => {
            view.step(1);
            app.dirty = true;
        }
        // Rows are the visible ones — whatever the type-ahead left.
        MouseEventKind::Down(MouseButton::Left) if view.toggle_area.contains(mouse_pos) => {
            toggle_new_worktree(app);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let list = view.list_area;
            let first = view.window_start(list.height as usize);
            let rows = view.visible().len();
            if let Some(index) = crate::list_hit::row_at(list, first, rows, mouse_pos)
                .and_then(|pos| view.visible_row(pos))
            {
                view.selected = index;
                activate_selected(app, out);
            }
            app.dirty = true;
        }
        _ => {}
    }
}

/// The AGENT PRESETS PICKER's and the PRESET EDITOR's own keys: one table
/// their key handlers ([`handle_list_key`], [`handle_editor_key`]) match
/// and their bottom borders spell.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const LAUNCH: Key = Key::new(&["enter"], "launch");
    /// The NEW WORKTREE toggle under the rows.
    pub const WORKTREE: Key = Key::new(&["tab"], "new worktree");
    /// The editor: the next field, a choice row's value, save.
    pub const FIELD: Key = Key::new(&["tab", "down"], "field");
    pub const CHOOSE: Key = crate::nav::SIDEWAYS.doing("choose");
    pub const SAVE: Key = Key::new(&["enter"], "save");
    /// A SYSTEM PRESET's: orion's text into the box.
    pub const RESTORE: Key = Key::new(&["ctrl+r"], "orion's text");
    #[cfg(test)]
    pub const ALL: &[Key] = &[LAUNCH, WORKTREE, FIELD, CHOOSE, SAVE, RESTORE];
}

/// The keys on the list's frame, by mode: Enter's verb, the NEW WORKTREE
/// toggle where the list has it, and where Esc goes — the one thing that
/// differs between the box's picker and one opened on a pull request,
/// until letters are typed, when Esc clears them.
pub(crate) fn list_hints(view: &AgentPresetsView) -> Vec<crate::hints::Hint> {
    let enter = if view.pane_target().is_some() {
        "insert"
    } else if view.is_picker() {
        "use"
    } else {
        "launch"
    };
    let esc = if !view.filter.is_empty() {
        "clear"
    } else if view.box_behind().is_some() {
        "back to the prompt"
    } else {
        "close"
    };
    let mut hints = vec![keys::LAUNCH.hint_as(enter).kept()];
    if view.has_worktree_row() {
        hints.push(keys::WORKTREE.hint());
    }
    hints.push(crate::hints::Hint::new("Esc", esc));
    hints
}

/// The keys on the PRESET EDITOR's frame: `^R` only on a SYSTEM PRESET
/// whose box holds other than orion's text — "start from" it while the box
/// is empty.
pub(crate) fn editor_hints(editor: &AgentPresetEditor) -> Vec<crate::hints::Hint> {
    let mut hints = vec![
        keys::FIELD.hint(),
        keys::CHOOSE.hint(),
        crate::ui::task_keys::NEWLINE.hint(),
    ];
    // The way back to orion's text is one a narrow frame keeps.
    if editor.can_restore() {
        hints.push(match editor.text.trim().is_empty() {
            true => keys::RESTORE.hint_as("start from orion's text").kept(),
            false => keys::RESTORE.hint().kept(),
        });
    }
    hints.push(keys::SAVE.hint().kept());
    hints.push(crate::hints::Hint::new("Esc", "back to Settings"));
    hints
}

/// The AGENT PRESETS list modal.
pub(crate) fn draw_list(f: &mut Frame, app: &mut App, view: &AgentPresetsView, th: Theme) {
    let total = view.presets.len();
    let visible = view.visible();
    let cursor = view.cursor(&visible);
    let selected = visible.get(cursor).map_or(0, |(index, _)| *index);
    // Sized for every preset, not the filtered few, so the frame holds
    // still under the typing — plus the NEW WORKTREE row under them, where
    // the list has one.
    let row = u16::from(view.has_worktree_row());
    let height = (total.max(1) as u16)
        .saturating_add(2 + row)
        .clamp(5 + row, f.area().height.max(5 + row));
    let area = centered_rect(f.area(), AGENT_PRESETS_W, height);
    f.render_widget(Clear, area);
    // In QUICK PROMPT picker mode Enter applies the row to the launch
    // behind it — the box, which Esc gives back, or the pull request the
    // list was opened on, where Esc just closes.
    // The type-ahead shows in the title, as a submenu's does: `Agent
    // presets ⌕ rev`, the bare ⌕ while nothing is typed yet.
    let title = if view.pane_target().is_some() {
        "Insert preset"
    } else if view.is_picker() {
        "Use preset"
    } else {
        "Agent presets"
    };
    let title = match view.filter.as_str() {
        "" => format!(" {title} ⌕ "),
        query => format!(" {title} ⌕ {query} "),
    };
    let block =
        crate::hints::modal_block(modal_block(&title, th), &list_hints(view), area.width, th);
    let frame_inner = block.inner(area);
    f.render_widget(block, area);
    // The last row inside the frame is where the launch lands; the
    // presets fill the rest. Over the box, the box's header says that.
    let toggle_area = if row == 1 {
        Rect {
            y: frame_inner.bottom().saturating_sub(1),
            height: frame_inner.height.min(1),
            ..frame_inner
        }
    } else {
        Rect::default()
    };
    let inner = Rect {
        height: frame_inner.height.saturating_sub(row),
        ..frame_inner
    };
    if row == 1 {
        f.render_widget(
            Paragraph::new(worktree_line(app, view, toggle_area.width, th)),
            toggle_area,
        );
    }

    if total == 0 {
        empty_list_row(
            f,
            inner,
            "no presets yet — make one in Settings → Presets",
            th,
        );
    }
    let start = window_start(cursor, inner.height as usize);
    for (pos, (i, lit)) in visible.iter().enumerate().skip(start) {
        let preset = &view.presets[*i];
        let Some(row_area) = row_rect(inner, pos - start) else {
            break;
        };
        let budget = (inner.width as usize).saturating_sub(2);
        // "name  claude · opus · high" left, a dim "no task orion" pinned
        // right for what the preset does on a launch and whose it is.
        let mut marks = Vec::new();
        if preset.skip_task {
            marks.push("no task");
        }
        // One of orion's own (Settings → Presets' Used by orion).
        if preset.system.is_some() {
            marks.push(crate::presets_tab::ORION_TAG);
        }
        let marks = marks.join(" ");
        let marks_w = marks.chars().count();
        let text_budget = budget.saturating_sub(if marks_w > 0 { marks_w + 2 } else { 0 });
        // The letters the type-ahead matched are lit in the name.
        let name_txt = truncate(&preset.name, text_budget);
        let mut used = name_txt.chars().count();
        let lit = visible_positions(lit, &name_txt, &preset.name);
        let mut spans = fuzzy_highlight_spans(&name_txt, lit, th);
        let spec = preset.spec_label();
        if used + 2 < text_budget {
            let spec = truncate(&format!("  {spec}"), text_budget - used);
            used += spec.chars().count();
            spans.push(Span::styled(spec, Style::default().fg(th.dim)));
        }
        if marks_w > 0 && used + marks_w < budget {
            spans.push(Span::raw(" ".repeat(budget - used - marks_w)));
            spans.push(Span::styled(marks, Style::default().fg(th.dim)));
        }
        render_row(f, row_area, spans, pos == cursor, true, th);
    }

    // Write-back (draw works on a clone): rects for mouse
    // hit-testing, plus the clamped cursor.
    if let Some(Overlay::AgentPresets(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = inner;
        v.toggle_area = toggle_area;
        v.selected = selected;
    }
}

/// The list's bottom row: where Enter launches, and the `[ ] new worktree
/// Tab` toggle, drawn as the QUICK PROMPT's target row is — quiet on a
/// checkout, a green NEW WORKTREE chip and the branch it will cut once
/// flipped. A PR SESSION names the pull request's head branch, the
/// checkout the DAEMON reuses or cuts, with nothing to flip. The right
/// half is dropped whole before the left is cut short.
fn worktree_line(app: &App, view: &AgentPresetsView, width: u16, th: Theme) -> Line<'static> {
    let dim = Style::default().fg(th.dim);
    let target = view.target();
    let branch = match &target {
        QuickTarget::NewWorktree { branch, .. } => branch.clone(),
        QuickTarget::Worktree(id) => app
            .tree
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .map_or_else(|| "(worktree gone)".into(), |w| w.branch.clone()),
    };
    let pr = view.quick.as_ref().and_then(|q| q.launch.pr.as_ref());
    let (left, right) = if let Some(pr) = pr {
        (
            vec![
                Span::styled(format!(" PR #{} · worktree: ", pr.number), dim),
                Span::styled(pr.head.clone(), Style::default().fg(th.muted)),
            ],
            vec![Span::styled("reused or cut on Enter ", dim)],
        )
    } else if view.is_new_worktree() {
        let chip = Style::default()
            .fg(th.on_accent)
            .bg(th.ok)
            .add_modifier(Modifier::BOLD);
        let on = Style::default().fg(th.ok).add_modifier(Modifier::BOLD);
        (
            vec![
                Span::raw(" "),
                Span::styled(" NEW WORKTREE ", chip),
                Span::styled(format!(" {branch}"), on),
            ],
            vec![
                Span::styled("[✓] new worktree", on),
                Span::styled(" Tab ", Style::default().fg(th.ok)),
            ],
        )
    } else {
        (
            vec![
                Span::styled(" worktree: ", dim),
                Span::styled(branch, Style::default().fg(th.muted)),
            ],
            vec![
                Span::styled("[ ] new worktree", dim),
                Span::styled(" Tab ", dim),
            ],
        )
    };
    let left_w: usize = left.iter().map(|s| s.width()).sum();
    let right_w: usize = right.iter().map(|s| s.width()).sum();
    let mut spans = left;
    if usize::from(width) > left_w + right_w {
        spans.push(Span::raw(" ".repeat(usize::from(width) - left_w - right_w)));
        spans.extend(right);
    }
    Line::from(spans)
}

/// The PRESET EDITOR form — under a banner saying why the last Enter saved
/// nothing, while that still applies, with the field it names in the error
/// color.
pub(crate) fn draw_editor(f: &mut Frame, app: &mut App, editor: &AgentPresetEditor, th: Theme) {
    // The single rows and a blank, then the text box and the PREVIEW —
    // all one row down while the banner is up. As tall as the screen
    // allows, within PRESET_EDITOR_H, and as wide: side by side the boxes
    // want the room.
    let banner = u16::from(editor.error.is_some());
    let screen = f.area();
    let height = screen
        .height
        .saturating_sub(SCREEN_MARGIN.0)
        .min(PRESET_EDITOR_H + banner)
        .max(PRESET_EDITOR_MIN_H);
    let width = screen
        .width
        .saturating_sub(SCREEN_MARGIN.1)
        .clamp(PRESET_EDITOR_W, PRESET_EDITOR_WIDE);
    let area = centered_rect(screen, width, height);
    f.render_widget(Clear, area);
    let title = if editor.is_edit() {
        format!(" Edit preset — {} ", editor.name.trim())
    } else {
        " New preset ".to_string()
    };
    let block = crate::hints::modal_block(
        modal_block(title, th),
        &editor_hints(editor),
        area.width,
        th,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    // The banner: the refusal in the error color on the first row, right
    // above the Name row it points at.
    if let Some(error) = &editor.error {
        if let Some(row_area) = row_rect(inner, 0) {
            let budget = (inner.width as usize).saturating_sub(2);
            let spans = vec![Span::styled(
                truncate(&format!("✗ {}", error.message), budget),
                Style::default().fg(th.err).add_modifier(Modifier::BOLD),
            )];
            render_row(f, row_area, spans, false, true, th);
        }
    }

    let label_w = 9usize;
    let top = usize::from(banner);
    // The rows above the box. A SYSTEM PRESET's task is orion's: no row.
    let single: Vec<(PresetField, usize)> = [
        PresetField::Name,
        PresetField::Kind,
        PresetField::Model,
        PresetField::Effort,
        PresetField::Task,
    ]
    .into_iter()
    .filter(|field| editor.system.is_none() || *field != PresetField::Task)
    .enumerate()
    .map(|(row, field)| (field, top + row))
    .collect();
    for (field, row) in single.iter() {
        let Some(row_area) = row_rect(inner, *row) else {
            break;
        };
        let focused = editor.field == *field;
        let available = field.available(editor);
        // The field the banner points at: label, value and caret in the
        // error color, focused or not, until its text changes.
        let bad = editor.error.as_ref().is_some_and(|e| e.field == *field);
        let label_style = if bad {
            Style::default().fg(th.err).add_modifier(Modifier::BOLD)
        } else if focused {
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
        } else if available {
            Style::default().fg(th.muted)
        } else {
            Style::default().fg(th.dim)
        };
        let mut spans = vec![Span::styled(
            format!("{:>label_w$}  ", field.label()),
            label_style,
        )];
        let budget = (inner.width as usize).saturating_sub(2 + label_w + 2);
        match field {
            // orion's own name, and the word that it stays so.
            PresetField::Name if editor.system.is_some() => {
                spans.push(Span::raw(truncate(editor.name.as_str(), budget)));
                spans.push(Span::styled(
                    format!("  {} — can't be renamed", crate::presets_tab::ORION_TAG),
                    Style::default().fg(th.dim),
                ));
            }
            PresetField::Name => {
                // Past the row's mark and the label.
                let at = crate::ui::field_after(row_area, 1 + label_w + 2);
                if focused {
                    let caret = if bad { th.err } else { th.accent };
                    let mut value = input_spans(&editor.name, at, caret, th);
                    if bad {
                        for span in &mut value {
                            if span.style.fg == Some(th.text) {
                                span.style.fg = Some(th.err);
                            }
                        }
                    }
                    spans.extend(value);
                } else {
                    crate::text_input::place_line(&editor.name, at, 0);
                    spans.push(if editor.name.trim().is_empty() {
                        let hint = if bad { th.err } else { th.dim };
                        Span::styled("(required)", Style::default().fg(hint))
                    } else if bad {
                        Span::styled(
                            truncate(editor.name.as_str(), budget),
                            Style::default().fg(th.err),
                        )
                    } else {
                        Span::raw(truncate(editor.name.as_str(), budget))
                    });
                }
            }
            _ => {
                let value = match field {
                    PresetField::Kind => editor.harness_value(),
                    PresetField::Model => editor.model.clone(),
                    PresetField::Effort => editor.effort.clone(),
                    PresetField::Task => editor.task_choice().to_string(),
                    _ => String::new(),
                };
                if !available {
                    spans.push(Span::styled("n/a", Style::default().fg(th.dim)));
                } else if focused {
                    spans.push(Span::styled("◂ ", Style::default().fg(th.accent)));
                    spans.push(Span::styled(
                        value,
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(" ▸", Style::default().fg(th.accent)));
                    if !editor.filter.is_empty() {
                        spans.push(Span::styled(
                            format!("  ⌕ {}", editor.filter),
                            Style::default().fg(th.accent),
                        ));
                        spans.push(Span::styled(
                            format!(" ({})", editor.filtered_choices().len()),
                            Style::default().fg(th.dim),
                        ));
                    }
                } else {
                    spans.push(Span::raw(value));
                }
                // A CLAUDE ACCOUNT's id says which dir, its email whose
                // login: with more than one account, the row says both.
                if *field == PresetField::Kind && !editor.follow {
                    if let Some(email) =
                        crate::claude_accounts::short_name(editor.kind, editor.custom.as_deref())
                    {
                        spans.push(Span::styled(
                            format!("  {email}"),
                            Style::default().fg(th.dim),
                        ));
                    }
                }
                if *field == PresetField::Task {
                    let what = if editor.skip_task {
                        "  Enter launches at once on the text alone"
                    } else {
                        "  Enter asks for a task (optional)"
                    };
                    spans.push(Span::styled(what, Style::default().fg(th.dim)));
                }
            }
        }
        render_row(f, row_area, spans, focused, true, th);
    }

    // Under a blank row, to the frame's foot: the text box, and the
    // PREVIEW beside it — or under it, on a narrow screen.
    let above = banner + single.len() as u16 + 1;
    let boxes = Rect {
        y: inner.y.saturating_add(above),
        height: inner.height.saturating_sub(above),
        ..inner
    };
    let (text_area, preview_area) = split_boxes(boxes);
    let focused_view = draw_text_box(f, editor, text_area, th);
    if let Some(area) = preview_area {
        draw_preview_box(f, editor, area, th);
    }

    // Write-back (draw works on a clone): the rect a click outside
    // of backs out from, and the view the focused box's rows walk by.
    if let Some(Overlay::AgentPresetEditor(e)) = &mut app.overlay {
        e.area = area;
        if let (Some(view), Some(input)) = (focused_view, e.text_field_mut()) {
            input.set_view(view);
        }
    }
}

/// The text box's rect and the PREVIEW's in `boxes`: halves side by side
/// from [`SIDE_BY_SIDE`] cells wide, else the box over the preview, the
/// box the larger ([`TEXT_SHARE`]) — it is where the typing is; no preview
/// when either would have under [`BOX_MIN_H`] rows.
fn split_boxes(boxes: Rect) -> (Rect, Option<Rect>) {
    if boxes.height < BOX_MIN_H {
        return (boxes, None);
    }
    if boxes.width >= SIDE_BY_SIDE {
        let left = boxes.width / 2;
        let text = Rect {
            width: left,
            ..boxes
        };
        let preview = Rect {
            x: boxes.x + left,
            width: boxes.width - left,
            ..boxes
        };
        return (text, Some(preview));
    }
    let (num, den) = TEXT_SHARE;
    let text_h = (boxes.height * num / den).max(BOX_MIN_H);
    if boxes.height.saturating_sub(text_h) < BOX_MIN_H {
        return (boxes, None);
    }
    let text = Rect {
        height: text_h,
        ..boxes
    };
    let preview = Rect {
        y: boxes.y + text_h,
        height: boxes.height - text_h,
        ..boxes
    };
    (text, Some(preview))
}

/// A rounded box titled `title`, border and title in `color`: the text
/// box's and the PREVIEW's.
fn titled_box(title: &'static str, color: ratatui::style::Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title, Style::default().fg(color)))
}

/// The text box in `area`, focused or idle — orion's text greyed in an
/// empty SYSTEM PRESET's. The view its rows walk by, when it has the caret.
fn draw_text_box(
    f: &mut Frame,
    editor: &AgentPresetEditor,
    area: Rect,
    th: Theme,
) -> Option<crate::text_input::TextView> {
    if area.height < BOX_MIN_H {
        return None;
    }
    let focused = editor.field == PresetField::Text;
    let block = titled_box(
        text_title(editor.system),
        if focused { th.accent } else { th.dim },
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    // An empty SYSTEM PRESET says orion's text, greyed: what it sends.
    if let Some(builtin) = editor
        .builtin_text()
        .filter(|_| editor.text.trim().is_empty())
    {
        let greyed = [PreviewPart::own(builtin)];
        let lines: Vec<Line> = styled_rows(&greyed, inner.width, |_| Style::default().fg(th.dim))
            .into_iter()
            .take(inner.height.into())
            .map(|row| row.line)
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
    if focused {
        let (view, rows) = draw_multiline_input(f, &editor.text, inner, th);
        draw_scroll_marks(f, area, view, rows, th.dim);
        return Some(view);
    }
    let empty = match editor.system {
        Some(_) => "",
        None => "sent with your task, or alone (optional)",
    };
    crate::ui::draw_multiline_idle(f, &editor.text, inner, empty, th);
    None
}

/// The PREVIEW's box in `area`: what the agent gets ([`preview`]).
fn draw_preview_box(f: &mut Frame, editor: &AgentPresetEditor, area: Rect, th: Theme) {
    let block = titled_box(PREVIEW_TITLE, th.dim);
    let inner = block.inner(area);
    f.render_widget(block, area);
    draw_preview(f, &preview(editor), inner, th);
}

/// The task the PREVIEW fills a preset of the user's in with.
const SAMPLE_TASK: &str = "fix the login redirect loop";

/// One run of the PREVIEW: text the preset sends as it stands, or what
/// orion puts in its place — a sample task, sample issues, what broke, a
/// note of what goes there — drawn apart so the eye finds where the task
/// lands.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewPart {
    text: String,
    sample: bool,
}

impl PreviewPart {
    fn own(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            sample: false,
        }
    }

    fn sample(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            sample: true,
        }
    }
}

/// What the agent will get from the preset `editor` describes, as it
/// stands — the box's text unsaved, orion's for an empty SYSTEM PRESET:
/// a preset of the user's with [`SAMPLE_TASK`] at its `{task}` (none
/// for one that never asks); Linear Ticket with two sample issues where
/// they go; PR autofix with what broke on a sample pull request; the week
/// review's rules between what orion writes round them.
fn preview(editor: &AgentPresetEditor) -> Vec<PreviewPart> {
    let mut preset = editor.to_preset();
    preset.system = editor.system;
    let preset = preset.with_builtin_text();
    match editor.system {
        Some(SystemPreset::LinearTicket) => linear_preview(&preset),
        Some(SystemPreset::WeekReview) => vec![
            PreviewPart::sample("… the period, the project and whose work …"),
            PreviewPart::own(format!("\n\n{}\n\n", preset.compose(""))),
            PreviewPart::sample(
                "… the shape to write in, then the period's pull requests, issues and todos …",
            ),
        ],
        Some(SystemPreset::PrAutofix) => with_sample(&preset, &crate::autofix::sample_task()),
        None if preset.skip_task && !preset.has_text() => {
            vec![PreviewPart::sample(
                "nothing: the CLI starts with no first prompt",
            )]
        }
        None if preset.skip_task => vec![PreviewPart::own(preset.compose(""))],
        None => with_sample(&preset, SAMPLE_TASK),
    }
}

/// `preset` with `sample` where its task goes, the sample marked: what
/// `compose(sample)` sends, in its parts.
fn with_sample(preset: &AgentPreset, sample: &str) -> Vec<PreviewPart> {
    let (before, after) = preset.halves();
    [
        PreviewPart::own(before.trim_start()),
        PreviewPart::sample(sample.trim()),
        PreviewPart::own(after.trim_end()),
    ]
    .into_iter()
    .filter(|part| !part.text.is_empty())
    .collect()
}

/// The Linear Ticket text as a launch on two sample issues fills it in
/// (`linear::ticket_template`), each placeholder's fill marked.
fn linear_preview(preset: &AgentPreset) -> Vec<PreviewPart> {
    let fills = crate::linear::template_fills(&crate::linear::sample_issues());
    let template = crate::linear::ticket_template(preset);
    let mut rest = template.as_str();
    let mut parts = Vec::new();
    while let Some((at, token, fill)) = fills
        .iter()
        .filter_map(|(token, fill)| rest.find(token).map(|at| (at, *token, fill)))
        .min_by_key(|(at, ..)| *at)
    {
        if at > 0 {
            parts.push(PreviewPart::own(&rest[..at]));
        }
        parts.push(PreviewPart::sample(fill.clone()));
        rest = &rest[at + token.len()..];
    }
    if !rest.is_empty() {
        parts.push(PreviewPart::own(rest));
    }
    parts
}

/// The PREVIEW drawn into `area`, wrapped as the text box wraps
/// ([`styled_rows`]): the samples in the accent, the rest as the agent
/// will read it, scrolled so the first sample's row is in sight with
/// [`PREVIEW_CONTEXT`] rows above it — a long text the user wrote round a
/// task near its end still shows where the task lands.
fn draw_preview(f: &mut Frame, parts: &[PreviewPart], area: Rect, th: Theme) {
    let text = Style::default().fg(th.text);
    let accent = Style::default().fg(th.accent);
    let mut lines = styled_rows(parts, area.width, |part| match part.sample {
        true => accent,
        false => text,
    });
    let first_sample: usize = parts
        .iter()
        .take_while(|part| !part.sample)
        .map(|part| part.text.chars().count())
        .sum();
    let lead = match parts.iter().any(|part| part.sample) {
        true => lines
            .iter()
            .rposition(|line| line.start <= first_sample)
            .unwrap_or(0),
        false => 0,
    };
    let height = usize::from(area.height);
    let top = lead
        .saturating_sub(PREVIEW_CONTEXT)
        .min(lines.len().saturating_sub(height));
    let shown: Vec<Line> = lines
        .drain(..)
        .skip(top)
        .take(height)
        .map(|row| row.line)
        .collect();
    f.render_widget(Paragraph::new(shown), area);
}

/// One row of [`styled_rows`]: the line, and the char offset into the
/// whole text it starts at.
struct StyledRow {
    start: usize,
    line: Line<'static>,
}

/// `parts` wrapped `width` cells wide as a text box wraps its text
/// (`TextInput::rows`), each row cut into spans where a part ends and
/// each span in `style_of` its part.
fn styled_rows(
    parts: &[PreviewPart],
    width: u16,
    style_of: impl Fn(&PreviewPart) -> Style,
) -> Vec<StyledRow> {
    let text: String = parts.iter().map(|part| part.text.as_str()).collect();
    let chars: Vec<char> = text.chars().collect();
    let mut bounds = Vec::with_capacity(parts.len());
    let mut at = 0;
    for part in parts {
        let end = at + part.text.chars().count();
        bounds.push((at, end, style_of(part)));
        at = end;
    }
    TextInput::multiline_with_text(text)
        .rows(width.max(1).into())
        .into_iter()
        .map(|(start, end)| {
            let spans = bounds
                .iter()
                .filter(|(from, to, _)| *from < end && *to > start)
                .map(|(from, to, style)| {
                    let piece: String = chars[start.max(*from)..end.min(*to)].iter().collect();
                    Span::styled(piece, *style)
                })
                .collect::<Vec<_>>();
            StyledRow {
                start,
                line: Line::from(spans),
            }
        })
        .collect()
}

/// What the text box's border says of its marker: where a task goes —
/// for orion's own presets, what orion puts there.
fn text_title(system: Option<SystemPreset>) -> &'static str {
    match system {
        None => TEXT_TITLE,
        Some(SystemPreset::PrAutofix) => " Text — {task} is where what broke goes ",
        Some(SystemPreset::LinearTicket) => " Text — {issues} is where the issues go ",
        Some(SystemPreset::WeekReview) => " Text — the rules your review is written by ",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::WorktreeId;

    /// The PICKER only picks: in every mode, ⌘N / ⌘I / ⌘W and their
    /// `^` twins — the manage chords it used to answer to — do nothing,
    /// and its frame names none of them; orion's presets carry the
    /// `orion` tag Settings → Presets gives them.
    #[test]
    fn the_picker_only_picks() {
        let presets = vec![
            crate::agent_presets::SystemPreset::PrAutofix.default_preset(),
            AgentPreset {
                name: "mine".into(),
                system: None,
                ..crate::agent_presets::SystemPreset::PrAutofix.default_preset()
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("presets.json"), || {
            pinned("{}", || {
                let mut app = App::new();
                let w = WorktreeId("w".into());
                let launch = crate::quick_prompt::QuickLaunch::from_config(
                    QuickTarget::Worktree(w.clone()),
                    &crate::config::Config::load(),
                );
                let fresh = crate::quick_prompt::QuickReturn::fresh(launch.clone());
                let in_box = crate::quick_prompt::QuickReturn {
                    from_box: true,
                    ..fresh.clone()
                };
                let on_pr = crate::quick_prompt::QuickReturn::fresh(launch.with_pr(Some(
                    crate::pull_request::PrLaunch {
                        url: "https://github.com/o/r/pull/7".into(),
                        head: "feat".into(),
                        number: 7,
                    },
                )));
                let over_pane = crate::quick_prompt::QuickReturn {
                    into_pane: Some(orion_core::SessionRef::Agent(orion_core::AgentId(
                        "a1".into(),
                    ))),
                    ..fresh
                };
                // The launcher, the box's picker, a pull request's, a pane's.
                for quick in [None, Some(in_box), Some(on_pr), Some(over_pane)] {
                    let mut view = AgentPresetsView::new(w.clone(), presets.clone());
                    view.quick = quick;
                    crate::hints::assert_hints_from(&list_hints(&view), keys::ALL);
                    for (selected, chord) in [0, 1].into_iter().flat_map(|row| {
                        [
                            "cmd+n", "ctrl+n", "cmd+i", "ctrl+e", "cmd+w", "ctrl+w", "ctrl+a",
                            "ctrl+d",
                        ]
                        .map(|chord| (row, chord))
                    }) {
                        let key = crate::keymap::KeyChord::parse(chord).unwrap();
                        view.selected = selected;
                        app.overlay = Some(Overlay::AgentPresets(view.clone()));
                        let mut out = Vec::new();
                        handle_list_key(&mut app, KeyEvent::new(key.code, key.mods), &mut out);
                        assert!(
                            matches!(&app.overlay, Some(Overlay::AgentPresets(v)) if v.presets.len() == 2),
                            "{chord} on row {selected}: {:?}",
                            app.overlay.as_ref().map(|_| ())
                        );
                        assert!(out.is_empty(), "{chord}");
                    }
                }
                app.overlay = Some(Overlay::AgentPresets(AgentPresetsView::new(w, presets)));
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
                terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
                let buffer = terminal.backend().buffer();
                let text: String = (0..buffer.area.height)
                    .map(|y| {
                        (0..buffer.area.width)
                            .map(|x| buffer[(x, y)].symbol())
                            .collect::<String>()
                            + "\n"
                    })
                    .collect();
                let autofix = text.lines().find(|l| l.contains("PR autofix")).unwrap();
                assert!(autofix.contains(crate::presets_tab::ORION_TAG), "{text}");
                for verb in ["new", "edit", "delete"] {
                    assert!(!text.contains(&format!(" {verb} ·")), "{verb}:\n{text}");
                }
            })
        });
    }

    /// A system preset on orion's text opens with its box empty — orion's
    /// text greyed in it — and the caret on its harness: its name and task
    /// are orion's, no fields at all, so Tab never lands on them. `^R`
    /// puts orion's text in the box to edit from; saving that unchanged,
    /// or the box empty, keeps nothing, and a rename is not kept.
    #[test]
    fn a_system_preset_opens_empty_on_orions_text_and_keeps_it_as_none() {
        use crate::agent_presets::SystemPreset;
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("agent_presets.json");
        pinned("{}", || {
            crate::agent_presets::with_presets_path(store, || {
                crate::agent_presets::seed_system();
                let presets = crate::agent_presets::load();
                let at = presets
                    .iter()
                    .position(|p| p.system == Some(SystemPreset::LinearTicket))
                    .unwrap();
                let mut editor = AgentPresetEditor::from_preset(0, at, &presets[at]);
                assert_eq!(
                    editor.text.as_str(),
                    "",
                    "orion's text is greyed, not typed"
                );
                assert_eq!(editor.builtin_text(), Some(crate::linear::TICKET_TEXT));
                assert_eq!(editor.field, PresetField::Kind);
                let mut field = editor.field;
                for _ in 0..PresetField::ALL.len() {
                    field = field.step(&editor, 1);
                    assert!(
                        !matches!(field, PresetField::Name | PresetField::Task),
                        "{field:?}"
                    );
                }
                assert!(editor.can_restore());
                editor.restore_text();
                assert_eq!(editor.text.as_str(), crate::linear::TICKET_TEXT);
                assert_eq!(editor.field, PresetField::Text);
                assert!(!editor.can_restore(), "nothing left to put back");
                editor.name = TextInput::with_text("Renamed".to_string());
                let mut app = App::new();
                save_agent_preset_editor(&mut app, editor);
                let saved = &crate::agent_presets::load()[at];
                assert_eq!(saved.name, "Linear Ticket", "not renamed");
                assert!(!saved.has_text(), "orion's text kept as none");
                assert_eq!(
                    crate::agent_presets::system(SystemPreset::LinearTicket)
                        .with_builtin_text()
                        .text,
                    crate::linear::TICKET_TEXT
                );
            })
        });
    }

    fn screen(app: &mut App, width: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    /// The editor of one of orion's presets: its name with the word that
    /// it stays, no Task row, the box's border naming what orion puts at
    /// its marker, orion's text greyed in the empty box, and `^R` on the
    /// frame — "start from" it while the box is empty, "orion's text" over
    /// the user's own, and nowhere on a preset of the user's.
    #[test]
    fn an_empty_system_preset_shows_orions_text_greyed() {
        use crate::agent_presets::SystemPreset;
        pinned("{}", || {
            let autofix = SystemPreset::PrAutofix.default_preset();
            let mut app = App::new();
            app.overlay = Some(Overlay::AgentPresetEditor(AgentPresetEditor::from_preset(
                0, 0, &autofix,
            )));
            let text = screen(&mut app, 100);
            assert!(
                text.contains("PR autofix  orion — can't be renamed"),
                "{text}"
            );
            assert!(!text.contains("Task "), "no task row:\n{text}");
            assert!(
                text.contains("Runs on  ◂ the PR's own agent ▸"),
                "PR autofix runs on the PR's own agent by default:\n{text}"
            );
            assert!(text.contains("{task} is where what broke goes"), "{text}");
            assert!(
                text.contains("You are orion's PR autofixer."),
                "greyed:\n{text}"
            );
            assert!(text.contains("^R start from orion's text"), "{text}");
            // The greyed text is dim, not the text colour.
            let th = app.theme;
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let at = (0..buffer.area.height).find_map(|y| {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                row.find("You are orion")
                    .map(|byte| (row[..byte].chars().count() as u16, y))
            });
            let (x, y) = at.expect("orion's text on screen");
            assert_eq!(buffer[(x, y)].fg, th.dim);

            let Some(Overlay::AgentPresetEditor(editor)) = &mut app.overlay else {
                unreachable!()
            };
            editor.text = TextInput::multiline_with_text("Mine, {task}.");
            let text = screen(&mut app, 100);
            assert!(!text.contains("You are orion's PR autofixer."), "{text}");
            assert!(text.contains("^R orion's text"), "{text}");

            app.overlay = Some(Overlay::AgentPresetEditor(AgentPresetEditor::new(0)));
            let text = screen(&mut app, 100);
            assert!(
                !text.contains("^R"),
                "a preset of the user's has none:\n{text}"
            );
            assert!(text.contains("Task "), "{text}");
        });
    }

    /// The PREVIEW's text, the samples in brackets.
    fn previewed(editor: &AgentPresetEditor) -> String {
        preview(editor)
            .into_iter()
            .map(|part| match part.sample {
                true => format!("[{}]", part.text),
                false => part.text,
            })
            .collect()
    }

    /// What the PREVIEW shows: a preset of the user's with the sample task
    /// at its marker — after its text with none, nothing for a `skip` one;
    /// Linear Ticket with sample issues where they go, after the user's
    /// text when it names no `{issues}` (as the user's own does), ids
    /// filled; PR autofix with what broke at its `{task}`; the week
    /// review's rules between what orion writes round them. Unsaved text
    /// counts, and an empty system preset previews orion's.
    #[test]
    fn the_preview_fills_each_preset_with_samples() {
        use crate::agent_presets::SystemPreset;
        let mut mine = AgentPresetEditor::new(0);
        mine.text = TextInput::multiline_with_text("Be strict.\n\n{task}\n\nRun the tests.");
        assert_eq!(
            previewed(&mine),
            format!("Be strict.\n\n[{SAMPLE_TASK}]\n\nRun the tests.")
        );
        mine.text = TextInput::multiline_with_text("Plan first.");
        assert_eq!(previewed(&mine), format!("Plan first.\n\n[{SAMPLE_TASK}]"));
        mine.skip_task = true;
        assert_eq!(previewed(&mine), "Plan first.", "no task asked, none shown");

        let ticket = SystemPreset::LinearTicket.default_preset();
        let mut editor = AgentPresetEditor::from_preset(0, 0, &ticket);
        let orions = previewed(&editor);
        assert!(
            orions.starts_with("Fix these Linear issues together"),
            "{orions}"
        );
        assert!(
            orions.contains("[### ENG-12: Login redirect loops"),
            "{orions}"
        );
        assert!(
            orions.contains("\"[ENG-12, ENG-15]: <short summary>\""),
            "{orions}"
        );
        // The user's own text: a prefix that names no `{issues}`, migrated.
        editor.text = TextInput::multiline_with_text("Investigate each upfront.\n\n{task}");
        let theirs = previewed(&editor);
        assert!(
            theirs.starts_with("Investigate each upfront.\n\n[### ENG-12"),
            "{theirs}"
        );
        assert!(
            !theirs.contains("{task}") && !theirs.contains("{issues}"),
            "{theirs}"
        );

        let autofix =
            AgentPresetEditor::from_preset(0, 0, &SystemPreset::PrAutofix.default_preset());
        let text = previewed(&autofix);
        assert!(text.starts_with("You are orion's PR autofixer."), "{text}");
        assert!(
            text.contains("[Pull request: https://github.com/you/app/pull/42"),
            "{text}"
        );
        assert!(text.contains("unit-tests"), "{text}");
        assert!(
            text.ends_with("anything you could not fix and why."),
            "{text}"
        );

        let review =
            AgentPresetEditor::from_preset(0, 0, &SystemPreset::WeekReview.default_preset());
        let text = previewed(&review);
        assert!(text.starts_with("[… the period"), "{text}");
        assert!(text.contains(crate::week_review::brief_rules()), "{text}");
    }

    /// The editor at a roomy width puts the PREVIEW beside the text box,
    /// at a narrow one under it; either way the preview of a long text
    /// whose task sits at its end scrolls to the sample, the sample in
    /// the accent.
    #[test]
    fn the_preview_sits_beside_the_box_or_under_it_and_shows_where_the_task_lands() {
        use crate::agent_presets::SystemPreset;
        pinned("{}", || {
            let long = format!("{}\n\n{{task}}", "Read every ticket first. ".repeat(40));
            let ticket = AgentPreset {
                text: long,
                ..SystemPreset::LinearTicket.default_preset()
            };
            for (width, beside) in [(130, true), (84, false)] {
                let mut app = App::new();
                app.overlay = Some(Overlay::AgentPresetEditor(AgentPresetEditor::from_preset(
                    0, 0, &ticket,
                )));
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 40)).unwrap();
                terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
                let buffer = terminal.backend().buffer().clone();
                let rows: Vec<String> = (0..buffer.area.height)
                    .map(|y| {
                        (0..buffer.area.width)
                            .map(|x| buffer[(x, y)].symbol())
                            .collect()
                    })
                    .collect();
                let find = |needle: &str| {
                    rows.iter().enumerate().find_map(|(y, row)| {
                        row.find(needle)
                            .map(|byte| (row[..byte].chars().count(), y))
                    })
                };
                let text = rows.join("\n");
                let (tx, ty) = find("Text — {issues}").unwrap_or_else(|| panic!("{text}"));
                let (px, py) =
                    find("Preview — what the agent gets").unwrap_or_else(|| panic!("{text}"));
                if beside {
                    assert_eq!(ty, py, "side by side:\n{text}");
                    assert!(px > tx, "{text}");
                } else {
                    assert!(py > ty, "under the box:\n{text}");
                }
                let (sx, sy) =
                    find("### ENG-12").unwrap_or_else(|| panic!("the sample in sight:\n{text}"));
                assert!(sy > py, "{text}");
                assert_eq!(
                    buffer[(sx as u16, sy as u16)].fg,
                    app.theme.accent,
                    "{text}"
                );
            }
        });
    }

    fn pinned<T>(json: &str, f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, json).unwrap();
        crate::config::with_config_path(path, f)
    }

    fn editor() -> AgentPresetEditor {
        AgentPresetEditor::new(0)
    }

    fn stored(text: &str) -> AgentPreset {
        AgentPreset {
            name: "reviewer".into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            follow_box: None,
            model: None,
            effort: None,
            text: text.into(),
            skip_task: false,
            system: None,
        }
    }

    /// One text box, last in Tab order after the Task row; what it holds
    /// is what saves, `{task}` and all, and a stored preset opens on it.
    #[test]
    fn one_text_box_holds_the_text_and_its_task_marker() {
        let mut e = editor();
        assert_eq!(PresetField::Effort.step(&e, 1), PresetField::Task);
        assert_eq!(PresetField::Task.step(&e, 1), PresetField::Text);
        assert_eq!(PresetField::Text.step(&e, 1), PresetField::Name, "wraps");
        assert!(PresetField::Text.is_typed());
        e.name = TextInput::with_text("p");
        e.text = TextInput::multiline_with_text("Be strict.\n\n{task}\n\nRun the tests.");
        let saved = e.to_preset();
        assert_eq!(saved.text, "Be strict.\n\n{task}\n\nRun the tests.");
        assert_eq!(
            saved.compose("Fix auth"),
            "Be strict.\n\nFix auth\n\nRun the tests."
        );
        let reopened = AgentPresetEditor::from_preset(0, 0, &saved);
        assert_eq!(reopened.text.as_str(), saved.text, "nothing saved is lost");
        assert_eq!(stored("").text, "");
    }

    /// The Harness row lists custom entries by id after the built-ins —
    /// never a bare `custom` — and picking one stores the registry id.
    #[test]
    fn kind_choices_list_custom_entries_by_id() {
        pinned(r#"{"harnesses": {"agy": {"program": "agy"}}}"#, || {
            let mut editor = editor();
            editor.field = PresetField::Kind;
            let choices = editor.row_choices();
            assert!(choices.contains(&"claude".to_string()), "{choices:?}");
            assert!(choices.contains(&"agy".to_string()), "{choices:?}");
            assert!(!choices.contains(&"custom".to_string()), "{choices:?}");

            editor.set_row_value("agy");
            assert_eq!(editor.kind, AgentKind::Custom);
            assert_eq!(editor.custom.as_deref(), Some("agy"));
            assert_eq!(editor.row_value(), "agy");
            let preset = editor.to_preset();
            assert_eq!(preset.custom_harness.as_deref(), Some("agy"));

            editor.set_row_value("codex");
            assert_eq!(editor.kind, AgentKind::Codex);
            assert_eq!(editor.custom, None);
        });
    }

    /// A refused save moves the caret onto the field the error names,
    /// wherever it was; the first change to that field's text takes the
    /// error down, a change elsewhere leaves it — the name is still the
    /// problem. Pastes land like typed keys: lines kept in the text box,
    /// flattened in the name, nowhere on a choice row.
    #[test]
    fn reject_points_at_the_field_and_its_first_edit_clears() {
        let mut editor = AgentPresetEditor::new(0);
        let error = editor.validate(&[]).unwrap_err();
        assert_eq!(
            error,
            FormError {
                field: PresetField::Name,
                message: "the preset needs a name".into(),
            }
        );

        editor.field = PresetField::Text;
        editor.filter = "x".into();
        editor.reject(error.clone());
        assert_eq!(
            editor.field,
            PresetField::Name,
            "the caret jumps to the fix"
        );
        assert_eq!(editor.filter, "", "and no type-ahead comes along");
        assert_eq!(editor.error.as_ref(), Some(&error));

        // Typing into another field leaves the banner up.
        editor.field = PresetField::Text;
        assert!(editor.paste("one\ntwo"));
        assert_eq!(editor.text.as_str(), "one\ntwo", "lines kept in the box");
        assert_eq!(editor.error.as_ref(), Some(&error));

        // A choice row has nowhere to paste, and changes nothing.
        editor.field = PresetField::Kind;
        assert!(!editor.paste("codex"));
        assert_eq!(editor.row_value(), crate::agent_presets::FOLLOW_BOX);
        assert_eq!(editor.error.as_ref(), Some(&error));

        // The first change to the name is the fix.
        editor.field = PresetField::Name;
        assert!(editor.paste("re\nviewer"));
        assert_eq!(
            editor.name.as_str(),
            "re viewer",
            "flattened in the one-line field"
        );
        assert_eq!(editor.error, None);
        assert!(editor.validate(&[]).is_ok());

        // Likewise a typed key — but only one that changes the text: an
        // arrow leaves the banner where it is.
        editor.reject(error.clone());
        editor.edit_text(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(editor.error.as_ref(), Some(&error));
        editor.edit_text(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(editor.error, None);
    }
}
