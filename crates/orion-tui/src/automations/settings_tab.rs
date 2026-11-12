//! SETTINGS › AUTOMATIONS: where every automation is set up. The overall
//! cap first — how many automation sessions run at once, all of them
//! together — then a section per automation, in [`Automation::ALL`]
//! order, holding its runner's rows ([`Automation::settings`]): on or
//! off, its own cap, its labels or its schedule, its preset and its extra
//! steer. Each section's heading says on its right what the automation is
//! doing now, from its panel ([`super::Engine::panel`]).
//!
//! The keys follow Settings': `↑`/`↓` walk the rows, `←`/`→` change a
//! value, Enter edits — a line of text is typed in a prompt that comes
//! back here, **Labels** opens the Linear labels pick, **Preset** opens
//! it in Settings › Presets — `Space` turns the section's automation on
//! or off and `⌘U` picks another preset for it. An edit goes through the
//! runner's own [`super::Runner::edit`], [`super::Runner::text_of`] and
//! [`super::Runner::set_text`], so a row reads and writes here what it
//! always has. INPUT PARITY: a second click on a row is Enter on it, as
//! on every tab (`event_loop::activate::settings_row_cmd`).

use super::config::cap_label;
use super::view::{change_settings, AutomationsView};
use super::{Automation, Engine, SettingRow};
use crate::app::{App, Overlay, PromptKind};
use crate::config::SettingsRow;
use crate::hints::Hint;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The tab's own keys, past Settings' Enter and arrows: one table
/// [`key_cmd`] matches and [`hints`] spells.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const TOGGLE: Key = Key::new(&["space"], "on / off");
    pub const PRESET: Key = Key::new(&["cmd+u", "ctrl+x"], "pick another preset");
    pub const CHANGE: Key = crate::ui::settings_keys::CYCLE.doing("change");
    #[cfg(test)]
    pub const ALL: &[Key] = &[TOGGLE, PRESET, CHANGE];
}

/// The first section's heading, over the overall cap.
pub const ALL_AUTOMATIONS: &str = "All automations";
/// The overall cap's row.
pub const SESSIONS_AT_ONCE: &str = "Sessions at once, all automations";
/// The local e2e row ([`super::config::LocalE2e`]).
pub const LOCAL_E2E: &str = "E2E tests on this machine";
/// What the line under the rows says on it.
const E2E_EXPLAINED: &str = "Whether automation agents run end-to-end tests here. Never leaves them to CI on the pull request, so several agents can't grind the machine down at once; the rule is added to every agent's prompt";
/// What the line under the rows says on it.
const CAP_EXPLAINED: &str = "The most automation sessions running at once, all automations together, 1 to 20. Waiting-on-you sessions hold a place too, so approving or closing them frees it";
/// The label column, in cells: the longest label and a gap.
const LABEL_W: usize = SESSIONS_AT_ONCE.len() + 2;

/// One selectable row of the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// **Sessions at once, all automations**.
    Cap,
    /// **E2E tests on this machine**.
    E2e,
    /// A row of `which`'s section.
    Row(Automation, SettingRow),
}

impl Entry {
    pub fn automation(self) -> Option<Automation> {
        match self {
            Entry::Cap | Entry::E2e => None,
            Entry::Row(which, _) => Some(which),
        }
    }
}

/// Every selectable row, top to bottom: the overall cap, then each
/// automation's rows in its [`Automation::settings`] order.
pub fn entries() -> Vec<Entry> {
    [Entry::Cap, Entry::E2e]
        .into_iter()
        .chain(Automation::ALL.into_iter().flat_map(|which| {
            which
                .settings()
                .iter()
                .map(move |row| Entry::Row(which, *row))
        }))
        .collect()
}

/// How many selectable rows the tab holds.
pub fn len() -> usize {
    entries().len()
}

/// The row at `at`.
pub fn entry(at: usize) -> Option<Entry> {
    entries().get(at).copied()
}

/// Where `which`'s `row` sits — its section's first row when None, or
/// when it has no such row.
pub fn row_of(which: Automation, row: Option<SettingRow>) -> usize {
    let all = entries();
    let at = |want: &dyn Fn(&Entry) -> bool| all.iter().position(want);
    row.and_then(|row| at(&|e| *e == Entry::Row(which, row)))
        .or_else(|| at(&|e| e.automation() == Some(which)))
        .unwrap_or(0)
}

/// The rows as the settings overlay lays them out: each section under its
/// heading, a blank between them.
pub fn settings_rows() -> Vec<SettingsRow> {
    let mut out = vec![
        SettingsRow::Header(ALL_AUTOMATIONS.into()),
        SettingsRow::Setting(0),
        SettingsRow::Setting(1),
    ];
    let mut at = 2;
    for which in Automation::ALL {
        out.push(SettingsRow::Blank);
        out.push(SettingsRow::Header(which.name().into()));
        for _ in which.settings() {
            out.push(SettingsRow::Setting(at));
            at += 1;
        }
    }
    out
}

/// A row's value as it reads.
pub fn value(app: &App, entry: Entry) -> String {
    let settings = &app.automations.settings;
    let (which, row) = match entry {
        Entry::Cap => return settings.cap().to_string(),
        Entry::E2e => return settings.local_e2e.label().into(),
        Entry::Row(which, row) => (which, row),
    };
    let own = settings.of(which);
    match row {
        SettingRow::On => if app.automations.is_on(which) {
            "on"
        } else {
            "off"
        }
        .into(),
        SettingRow::Cap => cap_label(settings.cap_of(which)),
        SettingRow::Labels => super::linear_builder::labels_of(settings, which).join(" · "),
        SettingRow::When => own.when.map_or_else(
            || "Fri 08:30".into(),
            |w| format!("{} {}", capitalised(&w.day), w.at),
        ),
        SettingRow::Whose => own.whose.label().into(),
        SettingRow::Preset => {
            let preset = super::preset_of(settings, which);
            format!("{} · {}", preset.name, preset.spec_label())
        }
        SettingRow::ExtraSteer if own.extra_steer.trim().is_empty() => "none".into(),
        SettingRow::ExtraSteer => own.extra_steer,
    }
}

fn capitalised(word: &str) -> String {
    let mut chars = word.trim().chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// What a section's heading says on its right: what the automation is
/// doing, and how many of its sessions run against its own cap.
pub fn live_state(app: &App, which: Automation) -> String {
    let panel = Engine::panel(app, which);
    if !which.settings().contains(&SettingRow::Cap) || !app.automations.is_on(which) {
        return panel.state;
    }
    let running = super::running_of(app, which);
    match app.automations.settings.cap_of(which) {
        Some(cap) => format!("{} · running {running} of at most {cap}", panel.state),
        None => format!("{} · running {running}", panel.state),
    }
}

/// One of the tab's rows as drawn, `width` cells wide; `selected` is the
/// row under the cursor.
pub fn line(
    app: &App,
    row: &SettingsRow,
    selected: Option<usize>,
    width: u16,
    th: Theme,
) -> Line<'static> {
    let width = usize::from(width);
    match row {
        SettingsRow::Header(title) => {
            let state = match Automation::ALL.into_iter().find(|w| w.name() == title) {
                Some(which) => live_state(app, which),
                None => format!(
                    "{} of at most {} running",
                    super::running(app),
                    app.automations.settings.cap()
                ),
            };
            let left = vec![Span::styled(
                format!(" {title}"),
                Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
            )];
            let right = Span::styled(format!("{state} "), Style::default().fg(th.dim));
            super::view::spread(left, right, width)
        }
        SettingsRow::Setting(at) => {
            let Some(entry) = entry(*at) else {
                return Line::from("");
            };
            let label = match entry {
                Entry::Cap => SESSIONS_AT_ONCE,
                Entry::E2e => LOCAL_E2E,
                Entry::Row(_, row) => row.label(),
            };
            let value = value(app, entry);
            let mut label_style = Style::default();
            let mut value_style = match entry {
                Entry::Row(which, SettingRow::On) if !app.automations.is_on(which) => {
                    Style::default().fg(th.dim)
                }
                Entry::Row(_, SettingRow::ExtraSteer) if value == "none" => {
                    Style::default().fg(th.dim)
                }
                _ => Style::default().fg(th.accent),
            };
            if selected == Some(*at) {
                label_style = label_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
                value_style = value_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
            }
            let room = width.saturating_sub(3 + LABEL_W + 2);
            Line::from(vec![
                Span::styled("   ", label_style),
                Span::styled(format!("{label:<LABEL_W$}"), label_style),
                Span::styled(
                    format!("[{}]", crate::ui::truncate(&value, room)),
                    value_style,
                ),
            ])
        }
        _ => Line::from(""),
    }
}

/// The line under the rows for row `at`.
pub fn explain(at: usize) -> String {
    let (which, row) = match entry(at) {
        Some(Entry::Row(which, row)) => (which, row),
        Some(Entry::E2e) => return E2E_EXPLAINED.into(),
        _ => return CAP_EXPLAINED.into(),
    };
    let name = which.name();
    match row {
        SettingRow::On => which.description().into(),
        SettingRow::Cap => format!(
            "The most {name} sessions running at once, 1 to 10, or any: then only the overall cap applies. Waiting-on-you sessions hold a place too"
        ),
        SettingRow::Labels => "A ticket assigned to you with any of these labels is picked up. Enter picks them from your Linear workspace".into(),
        SettingRow::When => "When it runs: a day and a time, like Fri 08:30".into(),
        SettingRow::Whose => "Whose work it covers: just yours, or the whole team's".into(),
        SettingRow::Preset => "The preset says what the agent is told and what it runs on. Enter opens it in Settings › Presets".into(),
        SettingRow::ExtraSteer => format!(
            "A line added after every prompt {name} sends, as Extra steer: …"
        ),
    }
}

/// The keys row `at` answers to, for the frame: Enter's verb, then only
/// what works on that row.
pub fn hints(at: usize) -> Vec<Hint> {
    let enter = crate::ui::settings_keys::CHOOSE;
    let Some(Entry::Row(_, row)) = entry(at) else {
        return vec![keys::CHANGE.hint().kept()];
    };
    let verb = match row {
        SettingRow::On => "on / off",
        SettingRow::Cap => "more",
        SettingRow::Labels => "pick from Linear",
        SettingRow::Whose => "switch",
        SettingRow::Preset => "open in Presets",
        SettingRow::When | SettingRow::ExtraSteer => "edit",
    };
    let mut out = vec![enter.hint_as(verb).kept()];
    if matches!(row, SettingRow::Cap | SettingRow::On | SettingRow::Whose) {
        out.push(keys::CHANGE.hint());
    }
    if row != SettingRow::On {
        out.push(keys::TOGGLE.hint());
    }
    out.push(keys::PRESET.hint());
    out
}

/// What a key on the tab asks for. Each `usize` is a row of the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// Enter, or a second click: edit or change the row.
    Open(usize),
    /// `Space`: the row's automation on or off.
    Toggle(usize),
    /// `←` (-1) or `→` (1): the row's value a step.
    Step(usize, i8),
    /// `⌘U`: another preset for the row's automation.
    PickPreset(usize),
}

/// What `key` asks for on row `at`; None for a key the tab has no use for,
/// which then does nothing.
pub fn key_cmd(at: usize, key: &KeyEvent) -> Option<Cmd> {
    match key.code {
        _ if keys::TOGGLE.matches(key) => Some(Cmd::Toggle(at)),
        _ if keys::PRESET.matches(key) => Some(Cmd::PickPreset(at)),
        _ if crate::ui::settings_keys::CHOOSE.matches(key) => Some(Cmd::Open(at)),
        KeyCode::Left | KeyCode::Char('h') => Some(Cmd::Step(at, -1)),
        KeyCode::Right | KeyCode::Char('l') => Some(Cmd::Step(at, 1)),
        _ => None,
    }
}

/// Run `cmd` on the tab.
pub fn run(app: &mut App, cmd: Cmd) {
    let at = match cmd {
        Cmd::Open(at) | Cmd::Toggle(at) | Cmd::Step(at, _) | Cmd::PickPreset(at) => at,
    };
    let Some(entry) = entry(at) else {
        return;
    };
    match (cmd, entry) {
        (Cmd::Open(_), Entry::Cap) => change_settings(app, |s| s.step_cap(1)),
        (Cmd::Step(_, step), Entry::Cap) => change_settings(app, |s| s.step_cap(step)),
        (Cmd::Open(_) | Cmd::Step(..), Entry::E2e) => {
            change_settings(app, |s| s.local_e2e = s.local_e2e.next())
        }
        (Cmd::Toggle(_) | Cmd::PickPreset(_), Entry::Cap | Entry::E2e) => {
            note(app, "That's for an automation: on one of its rows")
        }
        (Cmd::Toggle(_), Entry::Row(which, _)) => toggle(app, which),
        (Cmd::PickPreset(_), Entry::Row(which, _)) => {
            crate::preset_overlays::open_for_automation(app, which)
        }
        (Cmd::Open(_) | Cmd::Step(..), Entry::Row(which, SettingRow::On)) => toggle(app, which),
        (Cmd::Open(_), Entry::Row(which, SettingRow::Cap)) => step_own(app, which, 1),
        (Cmd::Step(_, step), Entry::Row(which, SettingRow::Cap)) => step_own(app, which, step),
        (Cmd::Step(..), Entry::Row(which, SettingRow::Whose)) => {
            edit(app, at, which, SettingRow::Whose)
        }
        (Cmd::Step(..), Entry::Row(..)) => note(app, "Enter changes it"),
        (Cmd::Open(_), Entry::Row(which, SettingRow::Preset)) => open_preset(app, which),
        (Cmd::Open(_), Entry::Row(which, SettingRow::ExtraSteer)) => {
            crate::event_loop::open_prompt(
                app,
                PromptKind::AutomationText {
                    which,
                    row: SettingRow::ExtraSteer,
                },
            )
        }
        (Cmd::Open(_), Entry::Row(which, row)) => {
            // A row the runner has as a line of text is typed in a prompt.
            if app
                .automations
                .runner(which)
                .is_some_and(|r| r.text_of(app, row).is_some())
            {
                return crate::event_loop::open_prompt(
                    app,
                    PromptKind::AutomationText { which, row },
                );
            }
            edit(app, at, which, row);
        }
    }
}

/// `Space`: `which` on or off.
pub(crate) fn toggle(app: &mut App, which: Automation) {
    let on = !app.automations.is_on(which);
    change_settings(app, |s| s.of_mut(which).on = Some(on));
}

fn step_own(app: &mut App, which: Automation, step: i8) {
    change_settings(app, |s| s.step_cap_of(which, step));
}

/// A word on the line under the rows.
fn note(app: &mut App, text: impl Into<String>) {
    if let Some(Overlay::Settings(view)) = &mut app.overlay {
        view.info(text);
    }
}

/// Enter on a row the runner edits itself ([`super::Runner::edit`]): the
/// labels pick comes up in the Automations modal, back here when it's
/// done; anything else is done at once, the tab staying up.
fn edit(app: &mut App, at: usize, which: Automation, row: SettingRow) {
    app.overlay = Some(Overlay::Automations(AutomationsView::editing_from(at)));
    let done = super::with_runner(app, which, |runner, app| runner.edit(app, row));
    if matches!(&app.overlay, Some(Overlay::Automations(v)) if v.labels.is_some()) {
        return;
    }
    reopen_at(app, at);
    if done != Some(true) {
        if let Some(Overlay::Settings(view)) = &mut app.overlay {
            view.warn(format!("{} can't be changed here yet", row.label()));
        }
    }
}

/// Enter on **Preset**: Settings › Presets, on the preset it is told by.
fn open_preset(app: &mut App, which: Automation) {
    let preset = super::preset_of(&app.automations.settings, which);
    let presets = crate::agent_presets::load_seeded();
    let at = presets
        .iter()
        .position(|p| p.name == preset.name)
        .and_then(|index| crate::presets_tab::Screen::of(presets).row_of(index));
    crate::presets_tab::reopen(app, at.unwrap_or(0));
}

/// The AGENT PRESETS PICKER's answer for `which`: `name` is the preset it
/// is told by now — its own SYSTEM PRESET's name puts it back on that one,
/// None (Esc) leaves it — and the tab comes back on its **Preset** row.
pub fn picked(app: &mut App, which: Automation, name: Option<String>) {
    if let Some(name) = name {
        let own = name.trim().eq_ignore_ascii_case(which.preset().name());
        change_settings(app, |s| s.of_mut(which).preset = (!own).then_some(name));
    }
    reopen_at(app, row_of(which, Some(SettingRow::Preset)));
}

/// The prompt for `which`'s `row`: its title, what it asks for, and the
/// line as it stands.
pub fn prompt_text(app: &App, which: Automation, row: SettingRow) -> (String, String, String) {
    let title = format!("{} · {}", which.name(), row.label());
    match row {
        SettingRow::ExtraSteer => (
            title,
            "added after every prompt it sends (empty = none)".into(),
            app.automations.settings.of(which).extra_steer,
        ),
        _ => (
            title,
            "a day and a time, like Fri 08:30".into(),
            app.automations
                .runner(which)
                .and_then(|r| r.text_of(app, row))
                .unwrap_or_default(),
        ),
    }
}

/// That prompt's Enter: `text` written — the extra steer as it is, any
/// other row through its runner — and the tab back on the row, saying
/// why when it was refused.
pub fn write_text(app: &mut App, which: Automation, row: SettingRow, text: &str) {
    let text = text.trim().to_string();
    let kept = match row {
        SettingRow::ExtraSteer => {
            change_settings(app, |s| s.of_mut(which).extra_steer = text);
            Ok(())
        }
        _ => super::with_runner(app, which, |r, app| r.set_text(app, row, &text))
            .unwrap_or_else(|| Err(format!("{} can't be written here", row.label()))),
    };
    back_on(app, which, row);
    if let (Err(why), Some(Overlay::Settings(view))) = (kept, &mut app.overlay) {
        view.warn(why);
    }
}

/// The tab back up on `which`'s `row`: where a prompt and the labels pick
/// return.
pub fn back_on(app: &mut App, which: Automation, row: SettingRow) {
    reopen_at(app, row_of(which, Some(row)));
}

/// Settings › Automations, at `which`'s section — the top when None:
/// Enter on an automation in the overview (`⌘J`), and its `,`.
pub fn open_at(app: &mut App, which: Option<Automation>) {
    reopen_at(app, which.map_or(0, |which| row_of(which, None)));
}

/// The settings overlay up on this tab, the cursor on row `at`.
pub fn reopen_at(app: &mut App, at: usize) {
    let tab = crate::config::automations_tab();
    app.settings_tab = tab;
    app.remember_settings_row(tab, at);
    app.remember_settings_focus(false);
    crate::event_loop::reopen_settings(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn in_config<T>(f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("agent_presets.json"), || {
            crate::config::with_config_path(dir.path().join("config.json"), f)
        })
    }

    fn settings_at(app: &App) -> (usize, usize) {
        match &app.overlay {
            Some(Overlay::Settings(v)) => (v.tab, v.selected),
            other => panic!("not Settings: {other:?}"),
        }
    }

    #[test]
    fn every_key_parses() {
        assert!(keys::ALL.iter().all(crate::hints::Key::parses));
    }

    /// The overall cap, then each automation's rows in its `settings()`
    /// order — **At most at once** where it starts sessions of its own —
    /// under its own heading.
    #[test]
    fn the_tab_lists_each_automations_rows_in_order() {
        let all = entries();
        assert_eq!(all[0], Entry::Cap);
        for which in Automation::ALL {
            let rows: Vec<SettingRow> = all
                .iter()
                .filter_map(|e| match e {
                    Entry::Row(w, row) if *w == which => Some(*row),
                    _ => None,
                })
                .collect();
            assert_eq!(rows, which.settings(), "{}", which.name());
            assert_eq!(all[row_of(which, None)], Entry::Row(which, SettingRow::On));
        }
        assert_eq!(
            entry(row_of(Automation::BugFixer, Some(SettingRow::Cap))),
            Some(Entry::Row(Automation::BugFixer, SettingRow::Cap))
        );
        let rows = settings_rows();
        let selectable: Vec<usize> = rows.iter().filter_map(SettingsRow::index).collect();
        assert_eq!(selectable, (0..len()).collect::<Vec<_>>());
        let headers: Vec<String> = rows
            .iter()
            .filter_map(|r| match r {
                SettingsRow::Header(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        let mut want = vec![ALL_AUTOMATIONS.to_string()];
        want.extend(Automation::ALL.iter().map(|a| a.name().to_string()));
        assert_eq!(headers, want);
    }

    /// Settings › Automations as the overlay draws it: the caps and every
    /// section, its heading saying what it is doing.
    #[test]
    fn the_tab_draws_the_caps_and_every_section() {
        in_config(|| {
            let mut app = App::new();
            open_at(&mut app, None);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 60)).unwrap();
            terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let text: String = (0..buf.area.height)
                .map(|y| {
                    let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                    row + "\n"
                })
                .collect();
            assert!(text.contains(SESSIONS_AT_ONCE), "{text}");
            assert!(text.contains("[3]"), "{text}");
            assert!(text.contains("At most at once"), "{text}");
            assert!(text.contains("0 of at most 3 running"), "{text}");
        });
    }

    /// Arrows step both caps and save them; Space and Enter on **On** turn
    /// the section's automation on and off.
    #[test]
    fn arrows_step_the_caps_and_space_switches_the_automation() {
        in_config(|| {
            let mut app = App::new();
            open_at(&mut app, None);
            run(&mut app, Cmd::Step(0, 1));
            assert_eq!(crate::config::Config::load().automations.max_running, 4);
            let cap = row_of(Automation::BugFixer, Some(SettingRow::Cap));
            run(&mut app, Cmd::Step(cap, 1));
            let saved = crate::config::Config::load().automations;
            assert_eq!(saved.of(Automation::BugFixer).max_running, Some(2));
            assert!(!app.automations.is_on(Automation::BugFixer));
            run(&mut app, Cmd::Toggle(cap));
            assert!(app.automations.is_on(Automation::BugFixer));
            run(&mut app, Cmd::Open(row_of(Automation::BugFixer, None)));
            assert!(!app.automations.is_on(Automation::BugFixer));
            assert!(matches!(&app.overlay, Some(Overlay::Settings(_))));
        });
    }

    /// Each row writes the same key the old ⌘J row did: **Extra steer**
    /// and **When** typed in a prompt, **Whose** switched, **Labels**
    /// picked — each back on its row.
    #[test]
    fn editing_a_row_writes_its_automations_key() {
        in_config(|| {
            let mut app = App::new();
            let tab = crate::config::automations_tab();
            let steer = row_of(Automation::PrFixer, Some(SettingRow::ExtraSteer));
            open_at(&mut app, None);
            run(&mut app, Cmd::Open(steer));
            let Some(Overlay::Prompt(p)) = &app.overlay else {
                panic!("a prompt: {:?}", app.overlay)
            };
            assert_eq!(
                p.kind,
                PromptKind::AutomationText {
                    which: Automation::PrFixer,
                    row: SettingRow::ExtraSteer
                }
            );
            write_text(
                &mut app,
                Automation::PrFixer,
                SettingRow::ExtraSteer,
                " Be brief ",
            );
            assert_eq!(settings_at(&app), (tab, steer));
            assert_eq!(
                crate::config::Config::load()
                    .automations
                    .of(Automation::PrFixer)
                    .extra_steer,
                "Be brief"
            );

            let when = row_of(Automation::WeekReview, Some(SettingRow::When));
            run(&mut app, Cmd::Open(when));
            assert!(
                matches!(&app.overlay, Some(Overlay::Prompt(p)) if p.input.as_str() == "Fri 08:30")
            );
            write_text(
                &mut app,
                Automation::WeekReview,
                SettingRow::When,
                "mon 9:15",
            );
            let own = crate::config::Config::load()
                .automations
                .of(Automation::WeekReview);
            assert_eq!(
                own.when.map(|w| (w.day, w.at)),
                Some(("mon".into(), "09:15".into()))
            );
            write_text(
                &mut app,
                Automation::WeekReview,
                SettingRow::When,
                "someday",
            );
            assert_eq!(settings_at(&app), (tab, when));
            let Some(Overlay::Settings(v)) = &app.overlay else {
                unreachable!()
            };
            assert!(v
                .notice
                .as_ref()
                .is_some_and(|(n, _)| n.contains("isn't a day")));

            let whose = row_of(Automation::WeekReview, Some(SettingRow::Whose));
            run(&mut app, Cmd::Open(whose));
            assert_eq!(settings_at(&app), (tab, whose));
            assert_eq!(
                crate::config::Config::load()
                    .automations
                    .of(Automation::WeekReview)
                    .whose,
                super::super::config::Whose::WholeTeam
            );

            // Labels: the pick comes up, and its Enter lands back here.
            let labels = row_of(Automation::BugFixer, Some(SettingRow::Labels));
            run(&mut app, Cmd::Open(labels));
            assert!(matches!(&app.overlay, Some(Overlay::Automations(v)) if v.labels.is_some()));
            let press = |app: &mut App, code| {
                super::super::view::handle_key(
                    app,
                    KeyEvent::new(code, KeyModifiers::NONE),
                    &mut Vec::new(),
                )
            };
            // With no Linear key the rows are the ticked labels: untick
            // the first, keep the rest.
            press(&mut app, KeyCode::Char(' '));
            press(&mut app, KeyCode::Enter);
            assert_eq!(settings_at(&app), (tab, labels));
            let saved = crate::config::Config::load()
                .automations
                .of(Automation::BugFixer)
                .labels;
            assert_eq!(saved, ["Regression", "Customer bug"].map(String::from));
        });
    }

    /// `⌘U` picks another preset and comes back on **Preset**; Enter on it
    /// opens Settings › Presets.
    #[test]
    fn the_preset_row_opens_presets_and_command_u_picks_another() {
        in_config(|| {
            let mut app = App::new();
            let preset = row_of(Automation::PrFixer, Some(SettingRow::Preset));
            open_at(&mut app, None);
            run(&mut app, Cmd::Open(preset));
            assert_eq!(settings_at(&app).0, crate::config::presets_tab());
            open_at(&mut app, None);
            run(&mut app, Cmd::PickPreset(preset));
            assert!(matches!(&app.overlay, Some(Overlay::AgentPresets(_))));
            picked(&mut app, Automation::PrFixer, Some("Linear Ticket".into()));
            assert_eq!(
                settings_at(&app),
                (crate::config::automations_tab(), preset)
            );
            assert_eq!(
                app.automations
                    .settings
                    .of(Automation::PrFixer)
                    .preset
                    .as_deref(),
                Some("Linear Ticket")
            );
        });
    }
}
