//! SETTINGS → PRESETS: where the AGENT PRESETS are managed. orion's own
//! — the SYSTEM PRESETS, the agents it starts itself — come first, under
//! *Used by orion*, each with a dim `orion` tag and a few words on what
//! it is for; the user's follow under *Yours*, then **+ New preset**.
//!
//! The keys follow the selected row, and so do the hints on the frame:
//! Enter edits any preset (and makes one on **+ New preset**); `^R` puts
//! orion's text back on a system preset that holds the user's; `^N` and
//! `^D` make and delete the user's own. A key that does nothing on a row
//! is neither offered nor hinted there — a system preset can't be deleted
//! or renamed, so its row has no delete. The PRESET EDITOR
//! (`preset_overlays`) opens over the tab and comes back to it, and the
//! confirms for a delete or a restore land back on it too.

use crate::agent_presets::{AgentPreset, SystemPreset};
use crate::app::{App, ConfirmDialog, Overlay, PendingAction};
use crate::config::SettingsRow;
use crate::hints::Hint;
use crate::keymap::Keymap;
use crate::theme::Theme;
use crate::ui::{cells, truncate, truncate_cells};
use crossterm::event::KeyEvent;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The sections' headings, top to bottom.
pub const USED_BY_ORION: &str = "Used by orion";
pub const YOURS: &str = "Yours";
/// The dim tag beside a system preset's name.
pub const ORION_TAG: &str = "orion";
const NEW_PRESET: &str = "+ New preset";
/// What the line under the rows says on **+ New preset**.
const NEW_PRESET_EXPLAINED: &str = "A launch you make often, saved: where it runs, its model and effort, and the text that goes with your task";
/// What `^D` on one of orion's presets says, there being nothing to delete.
pub const CANT_DELETE: &str = "orion's own presets can't be deleted";
/// The cells before a row's name, as every settings row is indented.
const ROW_INDENT: usize = 3;
/// The cells between a row's columns, and before what is pinned right.
const COLUMN_GAP: usize = 2;
/// The name column's narrowest and widest: room for the longest name and
/// a gap, never so wide the rest of the row has nothing left.
const NAME_MIN: usize = 14;
const NAME_MAX: usize = 26;
/// How many words of a preset's text the explanation under the rows
/// quotes before it is cut to [`EXCERPT`] characters.
const EXCERPT: usize = 120;

/// The tab's own keys, past Settings' Enter and arrows: one table
/// [`Screen::key_cmd`] matches and [`Screen::hints`] spells, both through
/// [`Screen::offers`].
pub(crate) mod keys {
    use crate::hints::Key;

    pub const NEW: Key = Key::new(&["ctrl+n"], "new");
    pub const DELETE: Key = Key::new(&["ctrl+d"], "delete");
    pub const RESTORE: Key = Key::new(&["ctrl+r"], "orion's text");
    #[cfg(test)]
    pub const ALL: &[Key] = &[NEW, DELETE, RESTORE];
}

/// One selectable row of the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The preset at this index of the list ([`Screen::presets`]).
    Preset(usize),
    /// **+ New preset**.
    New,
}

/// What a key on the tab asks for. Each `usize` is a row of the tab, not
/// an index into the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// Enter: edit the row's preset, or make one on **+ New preset**.
    Open(usize),
    /// `^N` on one of the user's: a new preset.
    New(usize),
    /// `^D` on one of the user's: delete it, once confirmed.
    Delete(usize),
    /// `^R` on a system preset holding the user's text: orion's back,
    /// once confirmed.
    Restore(usize),
    /// A key the row has no use for that deserves a word why.
    Note(&'static str),
}

/// What a row offers past Enter: the one rule [`Screen::hints`] spells
/// and [`Screen::key_cmd`] answers to, so a key that works is a key that
/// is hinted, and the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Offers {
    /// `^N` and `^D`: one of the user's.
    manage: bool,
    /// `^R`: a system preset holding the user's text.
    restore: bool,
}

/// The tab as of one read of the presets file: the presets, orion's made
/// sure of, and the rows they lay out as — orion's in [`SystemPreset::ALL`]
/// order, then the user's in theirs, then **+ New preset**.
#[derive(Debug, Clone)]
pub struct Screen {
    pub presets: Vec<AgentPreset>,
    pub rows: Vec<Row>,
    /// How many of `rows` are orion's: the *Used by orion* section.
    system_rows: usize,
    /// The name column, in cells: the longest name and a gap, within
    /// [`NAME_MIN`] and [`NAME_MAX`].
    name_w: usize,
}

impl Screen {
    pub fn load() -> Self {
        Self::of(crate::agent_presets::load_seeded())
    }

    pub fn of(presets: Vec<AgentPreset>) -> Self {
        let mut rows: Vec<Row> = SystemPreset::ALL
            .iter()
            .filter_map(|which| presets.iter().position(|p| p.system == Some(*which)))
            .map(Row::Preset)
            .collect();
        let system_rows = rows.len();
        rows.extend(
            presets
                .iter()
                .enumerate()
                .filter(|(_, p)| p.system.is_none())
                .map(|(i, _)| Row::Preset(i)),
        );
        rows.push(Row::New);
        let longest = presets.iter().map(|p| cells(&p.name)).max().unwrap_or(0);
        Self {
            presets,
            rows,
            system_rows,
            name_w: (longest + COLUMN_GAP).clamp(NAME_MIN, NAME_MAX),
        }
    }

    pub fn row(&self, at: usize) -> Option<Row> {
        self.rows.get(at).copied()
    }

    /// The preset on row `at`, with its index in the list.
    pub fn preset(&self, at: usize) -> Option<(usize, &AgentPreset)> {
        match self.row(at)? {
            Row::Preset(index) => self.presets.get(index).map(|p| (index, p)),
            Row::New => None,
        }
    }

    /// The row the preset at `index` of the list sits on.
    pub fn row_of(&self, index: usize) -> Option<usize> {
        self.rows.iter().position(|row| *row == Row::Preset(index))
    }

    /// The rows as the settings overlay lays them out: each section under
    /// its heading, a blank between them.
    pub fn settings_rows(&self) -> Vec<SettingsRow> {
        let mut out = vec![SettingsRow::Header(USED_BY_ORION.into())];
        out.extend((0..self.system_rows).map(SettingsRow::Setting));
        out.push(SettingsRow::Blank);
        out.push(SettingsRow::Header(YOURS.into()));
        out.extend((self.system_rows..self.rows.len()).map(SettingsRow::Setting));
        out
    }

    fn offers(&self, at: usize) -> Offers {
        let preset = self.preset(at).map(|(_, p)| p);
        Offers {
            manage: preset.is_some_and(|p| p.system.is_none()),
            restore: preset.is_some_and(|p| p.system.is_some() && p.has_text()),
        }
    }

    /// Row `at` as drawn, `width` cells wide: the name, then — for one of
    /// orion's — its tag and what it is for, `your text` at the right
    /// while it holds the user's; for one of the user's, where it runs,
    /// `no task` at the right when it never asks for one.
    pub fn line(
        &self,
        at: usize,
        selected: bool,
        width: u16,
        keymap: &Keymap,
        th: Theme,
    ) -> Line<'static> {
        let base = if selected {
            Style::default().bg(th.sel_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let dim = base.fg(th.dim);
        let (left, right) = match self.preset(at) {
            None => (vec![Span::styled(NEW_PRESET, base.fg(th.accent))], ""),
            Some((_, preset)) => {
                let name = Span::styled(self.name_cell(&preset.name), base);
                match preset.system {
                    Some(which) => system_cells(name, preset, which, keymap, dim),
                    None => user_cells(name, preset, dim),
                }
            }
        };
        fit_row(left, (right, dim), base, usize::from(width))
    }

    /// `name` cut and padded to the name column.
    fn name_cell(&self, name: &str) -> String {
        let name = truncate_cells(name, self.name_w - COLUMN_GAP);
        let pad = self.name_w.saturating_sub(cells(&name));
        format!("{name}{}", " ".repeat(pad))
    }

    /// The line under the rows for row `at`: what the preset is for, what
    /// it runs on and whose text it holds — or, for one of the user's, the
    /// start of what it sends.
    pub fn explain(&self, at: usize) -> String {
        let Some((_, preset)) = self.preset(at) else {
            return NEW_PRESET_EXPLAINED.into();
        };
        let runs = format!("Runs on {}", preset.spec_label());
        match preset.system {
            Some(which) => {
                let text = if preset.has_text() {
                    format!(
                        "Its text is yours; {} puts back orion's",
                        keys::RESTORE.label()
                    )
                } else {
                    "Its text is orion's".to_string()
                };
                format!(
                    "{}. {runs}. {text}. It can't be deleted or renamed",
                    explain_purpose(which)
                )
            }
            None => {
                let ask = if preset.skip_task {
                    ", never asking for a task"
                } else {
                    ""
                };
                let sends = match preset.has_text() {
                    true => excerpt(&preset.text),
                    false => "your task alone".to_string(),
                };
                format!("{runs}{ask}. Sends: {sends}")
            }
        }
    }

    /// The keys row `at` answers to, for the frame: Enter's verb, then
    /// only what works on that row.
    pub fn hints(&self, at: usize) -> Vec<Hint> {
        let enter = crate::ui::settings_keys::CHOOSE;
        let verb = if self.preset(at).is_some() {
            "edit"
        } else {
            "new preset"
        };
        let offers = self.offers(at);
        let mut hints = vec![enter.hint_as(verb).kept()];
        if offers.restore {
            hints.push(keys::RESTORE.hint());
        }
        if offers.manage {
            hints.extend([keys::NEW.hint(), keys::DELETE.hint()]);
        }
        hints
    }

    /// What `key` asks for on row `at`, past Enter — Settings' own, which
    /// comes to [`Cmd::Open`] as a second click does; None for a key the
    /// row has no use for, which then does nothing.
    pub fn key_cmd(&self, at: usize, key: &KeyEvent) -> Option<Cmd> {
        let offers = self.offers(at);
        if keys::NEW.matches(key) && offers.manage {
            Some(Cmd::New(at))
        } else if keys::DELETE.matches(key) && offers.manage {
            Some(Cmd::Delete(at))
        } else if keys::DELETE.matches(key) && self.preset(at).is_some() {
            Some(Cmd::Note(CANT_DELETE))
        } else if keys::RESTORE.matches(key) && offers.restore {
            Some(Cmd::Restore(at))
        } else {
            None
        }
    }
}

/// A system preset's row, past its name: the tag and what it is for, and
/// `your text` while it holds the user's.
fn system_cells(
    name: Span<'static>,
    preset: &AgentPreset,
    which: SystemPreset,
    keymap: &Keymap,
    dim: Style,
) -> (Vec<Span<'static>>, &'static str) {
    let gap = " ".repeat(COLUMN_GAP);
    let purpose = crate::hints::expand(which.purpose(), keymap);
    let left = vec![
        name,
        Span::styled(ORION_TAG, dim),
        Span::styled(format!("{gap}{purpose}"), dim),
    ];
    let right = if preset.has_text() { "your text" } else { "" };
    (left, right)
}

/// One of the user's rows, past its name: where it runs, and `no task`
/// when it never asks for one.
fn user_cells(
    name: Span<'static>,
    preset: &AgentPreset,
    dim: Style,
) -> (Vec<Span<'static>>, &'static str) {
    let left = vec![name, Span::styled(preset.spec_label(), dim)];
    (left, if preset.skip_task { "no task" } else { "" })
}

/// What the explanation says a system preset is for.
fn explain_purpose(which: SystemPreset) -> &'static str {
    match which {
        SystemPreset::PrAutofix => "What orion's autofix agent is told when a PR of yours breaks",
        SystemPreset::LinearTicket => "What every {linear} launch sends, its issues filled in",
        SystemPreset::WeekReview => "The rules your week in review ({automations}) is written by",
        SystemPreset::BugFixer => {
            "What the bug fixer tells the agent it starts on a labelled Linear bug"
        }
        SystemPreset::FeatureBuilder => {
            "What the feature builder tells the agent it starts on a labelled Linear feature"
        }
        SystemPreset::MeetingTakeaways => {
            "How meeting takeaways are pulled from a Teams transcript"
        }
        SystemPreset::BlueSky => "What the blue sky builder tells the agent it starts on an idea",
    }
}

/// The first [`EXCERPT`] words of `text` on one line, cut to as many
/// characters: however long the text, the work stops there. Its `{task}`
/// stays, saying where the task goes.
fn excerpt(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(EXCERPT).collect();
    truncate(&words.join(" "), EXCERPT)
}

/// `left` after the row's indent, `right` pinned to the far end with a
/// cell to spare, the gaps in `base` so a selected row is lit end to
/// end. Widths are in cells, so a wide glyph in a name can't push the
/// right off the row. The left is cut short to keep the right — it says
/// whose text a preset holds — unless that would leave it narrower than
/// the widest name column.
fn fit_row(
    left: Vec<Span<'static>>,
    (right, right_style): (&str, Style),
    base: Style,
    width: usize,
) -> Line<'static> {
    let right_w = cells(right);
    let room = width.saturating_sub(ROW_INDENT + 1);
    let show_right = right_w > 0 && room >= NAME_MAX + COLUMN_GAP + right_w;
    let mut budget = match show_right {
        true => room - right_w - COLUMN_GAP,
        false => room,
    };
    let mut spans = vec![Span::styled(" ".repeat(ROW_INDENT), base)];
    let mut used = 0;
    for span in left {
        if budget == 0 {
            break;
        }
        let text = truncate_cells(&span.content, budget);
        let w = cells(&text).min(budget);
        budget -= w;
        used += w;
        spans.push(Span::styled(text, span.style));
    }
    let tail = room.saturating_sub(used + if show_right { right_w } else { 0 });
    spans.push(Span::styled(" ".repeat(tail), base));
    if show_right {
        spans.push(Span::styled(right.to_string(), right_style));
    }
    spans.push(Span::styled(" ", base));
    Line::from(spans)
}

/// Run `cmd` from the tab: open the editor over it, or ask before a
/// delete or a restore — each comes back to the tab ([`reopen`]).
pub fn run(app: &mut App, cmd: Cmd) {
    match cmd {
        Cmd::Open(at) => {
            let screen = Screen::load();
            crate::preset_overlays::open_editor_from_settings(app, at, screen.preset(at));
        }
        Cmd::New(at) => crate::preset_overlays::open_editor_from_settings(app, at, None),
        Cmd::Delete(at) => {
            let screen = Screen::load();
            let Some((index, preset)) = screen.preset(at) else {
                return;
            };
            app.overlay = Some(confirm(
                "Delete preset",
                format!("Delete preset '{}'?\nIts text goes with it.", preset.name),
                PendingAction::DeletePreset { index },
            ));
        }
        Cmd::Restore(at) => {
            let screen = Screen::load();
            let Some((index, preset)) = screen.preset(at) else {
                return;
            };
            app.overlay = Some(confirm(
                "Put back orion's text",
                format!(
                    "'{}' goes back to orion's own text,\nand the text you gave it is gone.",
                    preset.name
                ),
                PendingAction::RestorePresetText { index },
            ));
        }
        Cmd::Note(text) => {
            if let Some(Overlay::Settings(view)) = &mut app.overlay {
                view.info(text);
            }
        }
    }
}

fn confirm(title: &str, message: String, action: PendingAction) -> Overlay {
    Overlay::Confirm(ConfirmDialog {
        title: title.into(),
        message,
        action,
        area: ratatui::layout::Rect::default(),
    })
}

/// The confirmed delete of the user's preset at `index`, then the tab
/// again, the cursor on the row that took its place.
pub fn delete(app: &mut App, index: usize) {
    change_and_reopen(app, index, |presets| {
        if presets.get(index).is_some_and(|p| p.system.is_none()) {
            presets.remove(index);
            return true;
        }
        false
    });
}

/// The confirmed restore of the system preset at `index`: it holds no
/// text of the user's, so it says orion's from now on.
pub fn restore(app: &mut App, index: usize) {
    let mut name = None;
    change_and_reopen(app, index, |presets| {
        let Some(preset) = presets.get_mut(index).filter(|p| p.system.is_some()) else {
            return false;
        };
        preset.text.clear();
        name = Some(preset.name.clone());
        true
    });
    if let (Some(name), Some(Overlay::Settings(view))) = (name, &mut app.overlay) {
        view.info(format!("{name} says orion's text again"));
    }
}

/// `change` the list, save it when it says it changed, and bring the tab
/// back on the row the preset at `index` was on.
fn change_and_reopen(
    app: &mut App,
    index: usize,
    change: impl FnOnce(&mut Vec<AgentPreset>) -> bool,
) {
    let screen = Screen::load();
    let at = screen.row_of(index).unwrap_or(0);
    let mut presets = screen.presets;
    if change(&mut presets) {
        save(app, &presets);
    }
    reopen(app, at);
}

/// Write the list, the footer saying so when it couldn't.
pub(crate) fn save(app: &mut App, presets: &[AgentPreset]) {
    if let Err(err) = crate::agent_presets::save(presets) {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "could not save agent presets: {err}"
        )));
    }
}

/// The settings overlay back up on the Presets tab, the cursor on row
/// `at` (clamped) — where the editor and the confirms return.
pub fn reopen(app: &mut App, at: usize) {
    let tab = crate::config::presets_tab();
    app.settings_tab = tab;
    app.remember_settings_row(tab, at);
    app.remember_settings_focus(false);
    crate::event_loop::reopen_settings(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SettingsView;
    use crossterm::event::{KeyCode, KeyModifiers};
    use orion_core::AgentKind;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn mine(name: &str) -> AgentPreset {
        AgentPreset {
            name: name.into(),
            kind: AgentKind::Claude,
            custom_harness: None,
            follow_box: Some(true),
            model: None,
            effort: None,
            text: "Please review it.".into(),
            skip_task: false,
            system: None,
        }
    }

    /// The user's own list, as it stands: Linear Ticket and PR autofix
    /// first — adopted by name, with text of their own — then Subagent
    /// Delegation; Week review added by orion at the end.
    fn users_list() -> Vec<AgentPreset> {
        let mut presets = vec![
            AgentPreset {
                text: "Plan the issues first.".into(),
                ..mine("Linear Ticket")
            },
            AgentPreset {
                text: "Fix the failing checks.".into(),
                skip_task: true,
                ..mine("PR autofix")
            },
            mine("Subagent Delegation"),
        ];
        crate::agent_presets::ensure_system(&mut presets);
        presets
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn table() -> Vec<crate::hints::Key> {
        let mut table = keys::ALL.to_vec();
        table.push(crate::ui::settings_keys::CHOOSE);
        table
    }

    /// The first row of the user's own: after every one of orion's.
    const MINE: usize = SystemPreset::ALL.len();

    /// orion's presets lead in their own order whatever the file's, the
    /// user's follow in theirs, and + New preset closes the list.
    #[test]
    fn orions_presets_come_first_then_yours_then_new() {
        let screen = Screen::of(users_list());
        let names: Vec<&str> = (0..screen.rows.len())
            .map(|at| {
                screen
                    .preset(at)
                    .map_or(NEW_PRESET, |(_, p)| p.name.as_str())
            })
            .collect();
        assert_eq!(
            names,
            [
                "PR autofix",
                "Linear Ticket",
                "Week review",
                "Bug fixer",
                "Feature builder",
                "Meeting takeaways",
                "Blue sky builder",
                "Subagent Delegation",
                NEW_PRESET
            ]
        );
        assert_eq!(
            screen.settings_rows(),
            [
                SettingsRow::Header(USED_BY_ORION.into()),
                SettingsRow::Setting(0),
                SettingsRow::Setting(1),
                SettingsRow::Setting(2),
                SettingsRow::Setting(3),
                SettingsRow::Setting(4),
                SettingsRow::Setting(5),
                SettingsRow::Setting(6),
                SettingsRow::Blank,
                SettingsRow::Header(YOURS.into()),
                SettingsRow::Setting(MINE),
                SettingsRow::Setting(MINE + 1),
            ]
        );
    }

    /// A system row offers no delete, no new and no rename, and hints
    /// none: Enter edits, and `^R` is there only while its text is the
    /// user's. One of the user's edits, makes and deletes; + New preset
    /// only makes one. Every hint names a key the tab answers to.
    #[test]
    fn keys_and_hints_follow_the_selected_row() {
        let screen = Screen::of(users_list());
        let words = |at: usize| -> Vec<String> {
            screen
                .hints(at)
                .iter()
                .map(|h| format!("{} {}", h.key, h.does))
                .collect()
        };
        for at in 0..MINE {
            crate::hints::assert_hints_from(&screen.hints(at), &table());
            let text = words(at).join(" · ");
            assert!(text.starts_with("Enter edit"), "{text}");
            for absent in ["delete", "new", "rename"] {
                assert!(!text.contains(absent), "row {at}: {text}");
            }
            assert_eq!(screen.key_cmd(at, &ctrl('n')), None, "row {at}");
            assert_eq!(
                screen.key_cmd(at, &ctrl('d')),
                Some(Cmd::Note(CANT_DELETE)),
                "row {at}"
            );
        }
        // PR autofix and Linear Ticket hold the user's text; Week review
        // holds orion's, where ^R has nothing to put back.
        assert_eq!(words(0), ["Enter edit", "^R orion's text"]);
        assert_eq!(screen.key_cmd(0, &ctrl('r')), Some(Cmd::Restore(0)));
        assert_eq!(words(2), ["Enter edit"]);
        assert_eq!(screen.key_cmd(2, &ctrl('r')), None);

        crate::hints::assert_hints_from(&screen.hints(MINE), &table());
        assert_eq!(words(MINE), ["Enter edit", "^N new", "^D delete"]);
        assert_eq!(screen.key_cmd(MINE, &ctrl('n')), Some(Cmd::New(MINE)));
        assert_eq!(screen.key_cmd(MINE, &ctrl('d')), Some(Cmd::Delete(MINE)));
        assert_eq!(screen.key_cmd(MINE, &ctrl('r')), None);

        let new = MINE + 1;
        assert_eq!(words(new), ["Enter new preset"]);
        assert_eq!(screen.key_cmd(new, &ctrl('d')), None);
        assert_eq!(screen.key_cmd(new, &ctrl('n')), None, "Enter is its new");
    }

    /// What the line under the rows says: what a system preset is for,
    /// and whose text it holds; the start of what one of the user's sends.
    #[test]
    fn the_explanation_says_what_the_row_is() {
        let screen = Screen::of(users_list());
        let autofix = screen.explain(0);
        assert!(autofix.contains("autofix agent"), "{autofix}");
        assert!(autofix.contains("Its text is yours"), "{autofix}");
        assert!(autofix.contains("can't be deleted or renamed"), "{autofix}");
        assert!(screen.explain(2).contains("Its text is orion's"));
        let mine = screen.explain(MINE);
        assert!(mine.contains("Sends: Please review it."), "{mine}");
        assert!(screen.explain(MINE + 1).contains("A launch you make often"));
    }

    /// The settings overlay on the Presets tab, `width` cells wide, the
    /// cursor on row `selected`, as plain text.
    fn draw(width: u16, selected: usize) -> String {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("agent_presets.json");
        crate::config::with_config_path(dir.path().join("config.json"), || {
            crate::agent_presets::with_presets_path(store, || {
                crate::agent_presets::save(&users_list()).unwrap();
                let mut app = App::new();
                let tab = crate::config::presets_tab();
                app.overlay = Some(Overlay::Settings(SettingsView::new(tab, selected, false)));
                let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
                terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
                let buffer = terminal.backend().buffer();
                let mut out = String::new();
                for y in 0..buffer.area.height {
                    for x in 0..buffer.area.width {
                        out.push_str(buffer[(x, y)].symbol());
                    }
                    out.push('\n');
                }
                out
            })
        })
    }

    /// The tab at a roomy width and a narrow one: the two sections under
    /// their headings, orion's first, each of orion's tagged, the user's
    /// text marked, + New preset last — and the frame's keys those of the
    /// row the cursor is on.
    #[test]
    fn the_tab_draws_two_sections_at_two_widths() {
        for width in [100, 72] {
            let text = draw(width, 0);
            let at = |needle: &str| {
                text.find(needle)
                    .unwrap_or_else(|| panic!("{needle} at {width}:\n{text}"))
            };
            assert!(at("Used by orion") < at("PR autofix"), "{text}");
            assert!(at("PR autofix") < at("Linear Ticket"));
            assert!(at("Week review") < at("Yours"));
            assert!(at("Yours") < at("Subagent Delegation"));
            assert!(at("Subagent Delegation") < at(NEW_PRESET));
            let line = |name: &str| text.lines().find(|l| l.contains(name)).unwrap().to_string();
            let autofix = line("PR autofix");
            assert!(autofix.contains(ORION_TAG), "{autofix}");
            assert!(autofix.contains("your text"), "{autofix}");
            let review = line("Week review");
            assert!(review.contains(ORION_TAG), "{review}");
            assert!(!review.contains("your text"), "{review}");
            assert!(text.contains("Enter edit"), "{text}");
            assert!(
                !text.contains("^D delete"),
                "no delete on a system row:\n{text}"
            );
        }
        let wide = draw(100, 0);
        assert!(wide.contains("sent when one of your PRs breaks"), "{wide}");
        assert!(wide.contains("writes your week review"), "{wide}");
        let on_mine = draw(100, MINE);
        assert!(on_mine.contains("^D delete"), "{on_mine}");
        assert!(on_mine.contains("^N new"), "{on_mine}");
    }
}
