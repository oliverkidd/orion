//! The AUTOMATIONS screen (`⌘J`): every automation in a list on the left,
//! the one under the cursor on the right — its name, what it does, its
//! SETTINGS, and its lists (what it is running, what it has done). The
//! header says how many automation sessions are running against the cap,
//! and `←`/`→` move the cap.
//!
//! The list has the keys first; `Tab` (or Enter) takes them into the
//! panel, where Enter acts on the row: a setting is changed or edited,
//! a list row jumps to its session. Space turns the automation under the
//! cursor on or off from either side, and `⌘U` picks another preset for
//! it. INPUT PARITY: a click on a list row is the arrows landing there; a
//! click on a panel row is Enter on it ([`activate`]).

use super::{config::AutomationsSettings, Automation, Dot, Engine, Panel, SettingRow, Tone};
use crate::app::{App, Overlay};
use crate::hints::Key;
use crate::text_input::TextInput;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::{AgentId, ClientRequest};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub(crate) mod keys {
    use crate::hints::Key;

    pub const TOGGLE: Key = Key::new(&["space"], "on / off");
    pub const OPEN: Key = Key::new(&["enter"], "open");
    pub const PRESET: Key = Key::new(&["cmd+u", "ctrl+x"], "pick another preset");
    pub const CAP: Key = crate::nav::SIDEWAYS.doing("cap");
    pub const SIDE: Key = crate::nav::PANELS;
    pub const BACK: Key = crate::nav::CLOSE.doing("back");
    #[cfg(test)]
    pub const ALL: &[Key] = &[TOGGLE, OPEN, PRESET, CAP, SIDE, BACK];
}

/// What the dim line above the keys says.
const EXPLAIN: &str =
    "The preset says what the agent is told and what it runs on. Enter opens it in Settings › Presets.";

/// The widest the screen gets, and the list column's width.
const WIDEST: u16 = 112;
const LIST_W: u16 = 34;
/// The panel's label column (`Extra steer  `).
const LABEL_W: usize = 13;

/// A row of the panel the keys can stand on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Setting(SettingRow),
    Session(AgentId),
    /// A list row with no session, of a runner whose rows open
    /// ([`super::Runner::open_row`]): its index among those rows.
    Row(usize),
}

/// What a click lands on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hit {
    /// A row of the list, by [`Automation::ALL`] index.
    Automation(usize),
    /// A row of the panel, by its index among [`items`].
    Item(usize),
    /// A row of the labels pick.
    Label(usize),
}

#[derive(Debug, Clone, Default)]
pub struct AutomationsView {
    /// The automation under the cursor, an index into [`Automation::ALL`].
    pub selected: usize,
    /// The keys are on the panel, not the list.
    pub in_panel: bool,
    /// The panel row under the cursor, an index into [`items`].
    pub row: usize,
    /// Enter on **Extra steer**, or on a row the runner has written as a
    /// line ([`super::Runner::text_of`]): the line being written.
    pub steer: Option<TextInput>,
    /// Enter on **Labels**: the labels pick, in the panel's place.
    pub labels: Option<super::linear_builder::LabelPick>,
    /// The row [`Self::steer`] writes, when it is not **Extra steer**.
    pub editing: Option<SettingRow>,
    /// A word about the last thing tried, over the explanation.
    pub notice: Option<String>,
    /// The modal's rect and its rows', written back as it draws.
    pub area: Rect,
    hits: Vec<(Rect, Hit)>,
}

impl AutomationsView {
    pub fn automation(&self) -> Automation {
        Automation::ALL[self.selected.min(Automation::ALL.len() - 1)]
    }
}

/// Whether `which`'s runner opens its list rows that have no session.
fn opens_rows(app: &App, which: Automation) -> bool {
    app.automations
        .runner(which)
        .is_some_and(|r| r.opens_rows())
}

/// The panel rows the keys can stand on: the SETTINGS rows, then every
/// list row with a session to jump to — or, where the runner opens them,
/// with none.
pub fn items(app: &App, which: Automation, panel: &Panel) -> Vec<Item> {
    let mut items: Vec<Item> = app
        .automations
        .settings_of(which)
        .iter()
        .map(|row| Item::Setting(*row))
        .collect();
    let opens = opens_rows(app, which);
    let mut plain = 0;
    for row in panel.lists.iter().flat_map(|list| &list.rows) {
        match &row.session {
            Some(id) => items.push(Item::Session(id.clone())),
            None if opens => {
                items.push(Item::Row(plain));
                plain += 1;
            }
            None => {}
        }
    }
    items
}

/// `⌘J`: the screen, on the automation it was last left on.
pub fn open(app: &mut App) {
    app.overlay = Some(Overlay::Automations(AutomationsView {
        selected: app.automations.selected,
        ..AutomationsView::default()
    }));
    app.dirty = true;
}

/// The config, changed by `change` and saved, then mirrored into the app.
pub(crate) fn change_settings(app: &mut App, change: impl FnOnce(&mut AutomationsSettings)) {
    let mut cfg = crate::config::Config::load();
    change(&mut cfg.automations);
    if let Err(err) = cfg.try_save() {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "couldn't save the automations: {err}"
        )));
    }
    app.automations.settings = cfg.automations;
}

/// Space: the automation under the cursor on or off.
fn toggle(app: &mut App, which: Automation) {
    let on = !app.automations.is_on(which);
    change_settings(app, |s| s.of_mut(which).on = Some(on));
}

pub fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Automations(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    view.notice = None;
    let which = view.automation();
    // Writing the extra steer, or a runner's line: the field has the keys.
    if let Some(input) = &mut view.steer {
        if crate::nav::closes(&key) {
            view.steer = None;
        } else if key.code == KeyCode::Enter {
            let text = input.as_str().trim().to_string();
            view.steer = None;
            match view.editing.take() {
                None => change_settings(app, |s| s.of_mut(which).extra_steer = text),
                Some(row) => {
                    let kept = super::with_runner(app, which, |r, app| r.set_text(app, row, &text));
                    if let (Some(Err(why)), Some(Overlay::Automations(view))) =
                        (kept, &mut app.overlay)
                    {
                        view.notice = Some(why);
                    }
                }
            }
        } else {
            input.handle_key(&key);
        }
        return;
    }
    // Picking labels: the pick has the keys.
    if view.labels.is_some() {
        super::linear_builder::pick_key(app, key);
        return;
    }
    // The runner's own key (week in review's `r`).
    let own = app.automations.runner(which).and_then(|r| r.hotkey());
    if own.is_some_and(|k| k.matches(&key)) {
        let done = super::with_runner(app, which, |r, app| r.hotkey_pressed(app));
        if let (Some(Err(why)), Some(Overlay::Automations(view))) = (done, &mut app.overlay) {
            view.notice = Some(why);
        }
        return;
    }
    match key.code {
        _ if keys::BACK.matches(&key) => {
            if view.in_panel {
                view.in_panel = false;
            } else {
                close(app);
            }
        }
        _ if keys::TOGGLE.matches(&key) => toggle(app, which),
        _ if keys::PRESET.matches(&key) => pick_preset(app, which),
        _ if keys::SIDE.matches(&key) => {
            view.in_panel = !view.in_panel;
            view.row = 0;
        }
        _ if keys::CAP.matches(&key) => {
            let step = if key.code == KeyCode::Left { -1 } else { 1 };
            change_settings(app, |s| s.step_cap(step));
        }
        KeyCode::Up | KeyCode::Down => {
            let down = key.code == KeyCode::Down;
            if view.in_panel {
                let panel = Engine::panel(app, which);
                let n = items(app, which, &panel).len();
                if let Some(Overlay::Automations(view)) = &mut app.overlay {
                    view.row = step(view.row, down, n);
                }
            } else {
                view.selected = step(view.selected, down, Automation::ALL.len());
                app.automations.selected = view.selected;
            }
        }
        _ if keys::OPEN.matches(&key) => {
            if view.in_panel {
                let row = view.row;
                activate(app, row, out);
            } else {
                view.in_panel = true;
                view.row = 0;
            }
        }
        _ => {}
    }
}

/// `at` a step up or down a list of `n`, kept on it.
fn step(at: usize, down: bool, n: usize) -> usize {
    match down {
        true => (at + 1).min(n.saturating_sub(1)),
        false => at.saturating_sub(1),
    }
}

/// Close the screen, remembering where it was.
fn close(app: &mut App) {
    if let Some(Overlay::Automations(view)) = &app.overlay {
        app.automations.selected = view.selected;
    }
    app.overlay = None;
}

/// Panel row `index` chosen — Enter on it, or a click: a setting changes
/// or opens, a list row jumps to its session.
pub fn activate(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Automations(view)) = &mut app.overlay else {
        return;
    };
    let which = view.automation();
    view.in_panel = true;
    view.row = index;
    let panel = Engine::panel(app, which);
    let Some(item) = items(app, which, &panel).into_iter().nth(index) else {
        return;
    };
    match item {
        Item::Setting(SettingRow::On) => toggle(app, which),
        Item::Setting(SettingRow::Preset) => open_preset(app, which),
        Item::Setting(SettingRow::ExtraSteer) => {
            let text = app.automations.settings.of(which).extra_steer;
            if let Some(Overlay::Automations(view)) = &mut app.overlay {
                view.steer = Some(TextInput::from(text.as_str()));
                view.editing = None;
            }
        }
        Item::Setting(row) => {
            // A row the runner has as a line of text is written in place.
            if let Some(text) = app
                .automations
                .runner(which)
                .and_then(|r| r.text_of(app, row))
            {
                if let Some(Overlay::Automations(view)) = &mut app.overlay {
                    view.steer = Some(TextInput::from(text.as_str()));
                    view.editing = Some(row);
                }
                return;
            }
            let done = super::with_runner(app, which, |runner, app| runner.edit(app, row));
            if done != Some(true) {
                if let Some(Overlay::Automations(view)) = &mut app.overlay {
                    view.notice = Some(format!("{} can't be changed here yet", row.label()));
                }
            }
        }
        // Blue sky's IDEAS jump to the idea, whose row has its session.
        Item::Session(id) if which == Automation::BlueSky => {
            close(app);
            super::blue_sky::open_idea(app, &id);
        }
        Item::Session(id) => {
            close(app);
            crate::event_loop::jump_to_session(app, id, out);
        }
        Item::Row(index) => {
            close(app);
            super::with_runner(app, which, |r, app| r.open_row(app, index));
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
    close(app);
    crate::presets_tab::reopen(app, at.unwrap_or(0));
}

/// `⌘U`: the AGENT PRESETS PICKER, its pick becoming the preset `which`
/// is told by; Esc there comes back here unchanged.
fn pick_preset(app: &mut App, which: Automation) {
    close(app);
    crate::preset_overlays::open_for_automation(app, which);
}

/// The picker's Enter: `name` is the preset `which` is told by now — its
/// own SYSTEM PRESET's name puts it back on that one — and the screen
/// comes back on it.
pub fn picked(app: &mut App, which: Automation, name: Option<String>) {
    if let Some(name) = name {
        let own = name.trim().eq_ignore_ascii_case(which.preset().name());
        change_settings(app, |s| s.of_mut(which).preset = (!own).then_some(name));
    }
    open(app);
}

pub fn handle_mouse(app: &mut App, mouse: MouseEvent, at: Position, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Automations(view)) = &mut app.overlay else {
        return;
    };
    let hit = view
        .hits
        .iter()
        .find(|(rect, _)| rect.contains(at))
        .map(|(_, hit)| hit.clone());
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let down = matches!(mouse.kind, MouseEventKind::ScrollDown);
            view.selected = step(view.selected, down, Automation::ALL.len());
            app.automations.selected = view.selected;
        }
        MouseEventKind::Down(MouseButton::Left) => match hit {
            Some(Hit::Automation(i)) => {
                view.selected = i;
                view.in_panel = false;
                view.steer = None;
                view.labels = None;
                view.editing = None;
                app.automations.selected = i;
            }
            Some(Hit::Item(i)) => activate(app, i, out),
            Some(Hit::Label(i)) => super::linear_builder::pick_click(app, i),
            None => {}
        },
        _ => return,
    }
    app.dirty = true;
}

// ---- drawing ----

/// `● ` in the dot's colour: red waiting on you, yellow working, grey
/// idle, a hollow `○` off.
fn dot(dot: Dot, th: Theme) -> Span<'static> {
    let (glyph, color) = match dot {
        Dot::Working => ("● ", th.warn),
        Dot::Waiting => ("● ", th.err),
        Dot::Idle => ("● ", th.dim),
        Dot::Off => ("○ ", th.faint),
    };
    Span::styled(glyph, Style::default().fg(color))
}

pub(crate) fn tone(tone: Tone, th: Theme) -> Style {
    Style::default().fg(match tone {
        Tone::Working => th.warn,
        Tone::Waiting => th.err,
        Tone::Done => th.ok,
        Tone::Quiet => th.dim,
    })
}

/// A setting's value as its row shows it.
fn value(app: &App, which: Automation, row: SettingRow) -> String {
    let own = app.automations.settings.of(which);
    match row {
        SettingRow::On => if app.automations.is_on(which) {
            "on"
        } else {
            "off"
        }
        .into(),
        SettingRow::Labels => format!(
            "{} · Enter to pick from Linear",
            super::linear_builder::labels_of(&app.automations.settings, which).join(" · ")
        ),
        SettingRow::When => own.when.map_or_else(
            || "Fri 08:30".into(),
            |w| format!("{} {}", capitalised(&w.day), w.at),
        ),
        SettingRow::Whose => own.whose.label().into(),
        SettingRow::Preset => {
            let preset = super::preset_of(&app.automations.settings, which);
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

/// `left` and `right` across `width` cells, `right` against the edge.
pub(crate) fn spread(
    left: Vec<Span<'static>>,
    right: Span<'static>,
    width: usize,
) -> Line<'static> {
    let used: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let room = width.saturating_sub(used + 1);
    let right_text = crate::ui::truncate(&right.content, room);
    let pad = width.saturating_sub(used + right_text.chars().count());
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right_text, right.style));
    Line::from(spans)
}

pub fn hints(view: &AutomationsView) -> Vec<crate::hints::Hint> {
    if view.labels.is_some() {
        return vec![
            Key::new(&["space"], "tick").hint().kept(),
            Key::new(&["enter"], "save").hint().kept(),
            crate::nav::CLOSE.doing("cancel").hint().kept(),
        ];
    }
    if view.steer.is_some() {
        return vec![
            Key::new(&["enter"], "save").hint().kept(),
            crate::nav::CLOSE.doing("cancel").hint().kept(),
        ];
    }
    vec![
        keys::TOGGLE.hint(),
        keys::OPEN.hint().kept(),
        keys::PRESET.hint(),
        keys::BACK.hint().kept(),
    ]
}

pub fn draw(f: &mut Frame, app: &mut App, view: &AutomationsView, th: Theme) {
    let screen = f.area();
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let height = screen.height.saturating_sub(2).min(32);
    let area = crate::ui::centered_rect(screen, width, height);
    let inner = crate::ui::render_modal_frame(f, area, " Automations ", &hints(view), th);
    let explain = view.notice.as_deref().unwrap_or(EXPLAIN);
    let (body, explain_row) = crate::hints::explain_area(explain, inner, 2);
    match &view.notice {
        Some(_) => {
            crate::hints::draw_explain_as(f, explain_row, explain, Style::default().fg(th.warn))
        }
        None => crate::hints::draw_explain(f, explain_row, explain, th),
    }
    let mut hits = Vec::new();
    let dim = Style::default().fg(th.dim);

    // The header: what is running against the cap.
    let header = format!(
        "{} of at most {} agents running ",
        super::running(app),
        app.automations.settings.cap()
    );
    if let Some(row) = crate::ui::row_rect(body, 0) {
        let line = spread(
            Vec::new(),
            Span::styled(header, Style::default().fg(th.muted)),
            usize::from(row.width).saturating_sub(3),
        );
        let mut spans = line.spans;
        spans.push(Span::styled("←→", Style::default().fg(th.accent)));
        f.render_widget(Paragraph::new(Line::from(spans)), row);
    }
    let cols = Rect {
        y: body.y + 2,
        height: body.height.saturating_sub(2),
        ..body
    };
    let list_w = LIST_W.min(cols.width / 2);
    let list = Rect {
        width: list_w,
        ..cols
    };
    let panel_area = Rect {
        x: cols.x + list_w + 2,
        width: cols.width.saturating_sub(list_w + 2),
        ..cols
    };
    // The rule between them.
    for y in cols.y..cols.y + cols.height {
        f.render_widget(
            Paragraph::new(Span::styled("│", Style::default().fg(th.edge))),
            Rect::new(cols.x + list_w, y, 1, 1),
        );
    }

    // The list: one row per automation, its dot, name and state.
    let selected = view.selected.min(Automation::ALL.len() - 1);
    for (i, which) in Automation::ALL.into_iter().enumerate() {
        let Some(row) = crate::ui::row_rect(list, i) else {
            break;
        };
        let panel = Engine::panel(app, which);
        let lit = i == selected;
        let fill = |s: Style| match (lit, view.in_panel) {
            (true, false) => s.bg(th.sel_bg).add_modifier(Modifier::BOLD),
            (true, true) => s.bg(th.sel_bg_dim),
            _ => s,
        };
        let mut left = vec![Span::raw(" "), dot(panel.dot, th)];
        left.push(Span::styled(which.name(), Style::default().fg(th.text)));
        let mut line = spread(
            left,
            Span::styled(panel.state, dim),
            usize::from(row.width) - 1,
        );
        line.spans.push(Span::raw(" "));
        let line = Line::from(
            line.spans
                .into_iter()
                .map(|s| {
                    let style = fill(s.style);
                    Span::styled(s.content, style)
                })
                .collect::<Vec<_>>(),
        );
        f.render_widget(Paragraph::new(line), row);
        hits.push((row, Hit::Automation(i)));
    }

    // The labels pick, in the panel's place.
    if let Some(pick) = &view.labels {
        draw_labels(f, panel_area, pick, th, &mut hits);
        if let Some(Overlay::Automations(live)) = &mut app.overlay {
            live.area = area;
            live.hits = hits;
        }
        return;
    }

    // The panel: name, description, SETTINGS, then its lists.
    let which = Automation::ALL[selected];
    let panel = Engine::panel(app, which);
    let item_rows = items(app, which, &panel);
    let row_cursor = view.row.min(item_rows.len().saturating_sub(1));
    let pw = usize::from(panel_area.width);
    let name = Span::styled(
        which.name(),
        Style::default().fg(th.text).add_modifier(Modifier::BOLD),
    );
    // The runner's own key, at the right of its name.
    let name = match app.automations.runner(which).and_then(|r| r.hotkey()) {
        Some(own) => {
            let hint = own.hint();
            let used = which.name().chars().count() + hint.key.chars().count() + 1;
            Line::from(vec![
                name,
                Span::raw(" ".repeat(pw.saturating_sub(used + hint.does.chars().count()))),
                Span::styled(hint.key, Style::default().fg(th.accent)),
                Span::raw(" "),
                Span::styled(hint.does, dim),
            ])
        }
        None => Line::from(name),
    };
    let mut lines: Vec<(Line<'static>, Option<usize>)> = vec![(name, None)];
    for l in crate::pr_preview::wrap(which.description(), pw.max(1)) {
        lines.push((
            Line::from(Span::styled(l, Style::default().fg(th.muted))),
            None,
        ));
    }
    lines.push((Line::from(""), None));
    lines.push((Line::from(Span::styled("SETTINGS", dim)), None));
    let mut index = 0;
    let lit = |i: usize, s: Style| {
        if view.in_panel && i == row_cursor {
            s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
        } else {
            s
        }
    };
    let mut steer_at = None;
    for row in app.automations.settings_of(which) {
        let label = format!("  {:<LABEL_W$}", row.label());
        let value = value(app, which, *row);
        let value_style = match row {
            SettingRow::On if app.automations.is_on(which) => Style::default().fg(th.ok),
            SettingRow::ExtraSteer if value == "none" => dim,
            _ => Style::default().fg(th.text),
        };
        let budget = pw.saturating_sub(LABEL_W + 2);
        if *row == view.editing.unwrap_or(SettingRow::ExtraSteer) && view.steer.is_some() {
            steer_at = Some(lines.len());
        }
        lines.push((
            Line::from(vec![
                Span::styled(label, lit(index, Style::default().fg(th.muted))),
                Span::styled(crate::ui::truncate(&value, budget), lit(index, value_style)),
            ]),
            Some(index),
        ));
        index += 1;
    }
    let opens = opens_rows(app, which);
    for list in &panel.lists {
        lines.push((Line::from(""), None));
        lines.push((Line::from(Span::styled(list.title, dim)), None));
        if list.rows.is_empty() {
            lines.push((
                Line::from(Span::styled(format!("  {}", list.empty), dim)),
                None,
            ));
        }
        for row in &list.rows {
            let at = (row.session.is_some() || opens).then(|| {
                index += 1;
                index - 1
            });
            let style = |s: Style| at.map_or(s, |i| lit(i, s));
            let line = spread(
                vec![Span::styled(
                    format!("  {}", row.text),
                    style(Style::default().fg(th.text)),
                )],
                Span::styled(row.state.clone(), style(tone(row.tone, th))),
                pw,
            );
            lines.push((line, at));
        }
    }
    // Keep the cursor's row in view.
    let fits = usize::from(panel_area.height).max(1);
    let cursor_line = lines
        .iter()
        .position(|(_, at)| view.in_panel && *at == Some(row_cursor))
        .unwrap_or(0);
    let first = (cursor_line + 1).saturating_sub(fits);
    for (n, (line, at)) in lines.into_iter().skip(first).take(fits).enumerate() {
        let Some(rect) = crate::ui::row_rect(panel_area, n) else {
            break;
        };
        let line = match (steer_at, &view.steer) {
            (Some(s), Some(input)) if s == n + first => {
                let label = Span::styled(
                    format!(
                        "  {:<LABEL_W$}",
                        view.editing.unwrap_or(SettingRow::ExtraSteer).label()
                    ),
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                );
                let field = crate::ui::field_after(rect, LABEL_W + 2);
                let mut spans = vec![label];
                spans.extend(crate::ui::input_spans(input, field, th.accent, th));
                Line::from(spans)
            }
            _ => line,
        };
        f.render_widget(Paragraph::new(line), rect);
        if let Some(i) = at {
            hits.push((rect, Hit::Item(i)));
        }
    }

    if let Some(Overlay::Automations(live)) = &mut app.overlay {
        live.area = area;
        live.hits = hits;
        live.row = row_cursor;
    }
}

/// The labels pick: a tick box per label, the cursor's row lit, in the
/// panel's place; each row a click target.
fn draw_labels(
    f: &mut Frame,
    area: Rect,
    pick: &super::linear_builder::LabelPick,
    th: Theme,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let dim = Style::default().fg(th.dim);
    let mut lines: Vec<(Line<'static>, Option<usize>)> = vec![
        (
            Line::from(Span::styled(
                format!("{} · Labels", pick.which.name()),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            )),
            None,
        ),
        (
            Line::from(Span::styled(
                "A ticket assigned to you with any of these is picked up.",
                Style::default().fg(th.muted),
            )),
            None,
        ),
        (Line::from(""), None),
    ];
    if let Some(notice) = pick.notice {
        lines.push((
            Line::from(Span::styled(notice, Style::default().fg(th.warn))),
            None,
        ));
    }
    if let Some(status) = pick.status() {
        lines.push((Line::from(Span::styled(status, dim)), None));
    }
    for (i, name) in pick.rows().into_iter().enumerate() {
        let lit = |s: Style| match i == pick.cursor {
            true => s.bg(th.sel_bg).add_modifier(Modifier::BOLD),
            false => s,
        };
        let (mark, color) = match pick.is_ticked(&name) {
            true => ("  [x] ", th.ok),
            false => ("  [ ] ", th.dim),
        };
        lines.push((
            Line::from(vec![
                Span::styled(mark, lit(Style::default().fg(color))),
                Span::styled(name, lit(Style::default().fg(th.text))),
            ]),
            Some(i),
        ));
    }
    let fits = usize::from(area.height).max(1);
    let cursor_line = lines
        .iter()
        .position(|(_, at)| *at == Some(pick.cursor))
        .unwrap_or(0);
    let first = (cursor_line + 1).saturating_sub(fits);
    for (n, (line, at)) in lines.into_iter().skip(first).take(fits).enumerate() {
        let Some(rect) = crate::ui::row_rect(area, n) else {
            break;
        };
        f.render_widget(Paragraph::new(line), rect);
        if let Some(i) = at {
            hits.push((rect, Hit::Label(i)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn press(app: &mut App, code: KeyCode, mods: KeyModifiers) -> Vec<ClientRequest> {
        let mut out = Vec::new();
        handle_key(app, KeyEvent::new(code, mods), &mut out);
        out
    }

    fn view(app: &App) -> &AutomationsView {
        match &app.overlay {
            Some(Overlay::Automations(v)) => v,
            other => panic!("not the automations screen: {other:?}"),
        }
    }

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal
            .draw(|f| {
                let Some(Overlay::Automations(v)) = app.overlay.clone() else {
                    unreachable!()
                };
                draw(f, app, &v, crate::theme::Theme::default());
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn in_config<T>(f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        crate::agent_presets::with_presets_path(dir.path().join("agent_presets.json"), || {
            crate::config::with_config_path(dir.path().join("config.json"), f)
        })
    }

    #[test]
    fn every_key_parses() {
        assert!(keys::ALL.iter().all(Key::parses));
    }

    #[test]
    fn the_screen_lists_every_automation_and_the_selected_ones_panel() {
        in_config(|| {
            let mut app = App::new();
            open(&mut app);
            let text = screen(&mut app);
            for which in Automation::ALL {
                assert!(text.contains(which.name()), "{}:\n{text}", which.name());
            }
            assert!(text.contains("0 of at most 5 agents running"), "{text}");
            assert!(text.contains("SETTINGS"));
            assert!(text.contains("PR autofix · the PR's own agent"), "{text}");
            assert!(text.contains("YOUR OPEN PRS"));
            assert!(text.contains("DONE THIS WEEK"));
            assert!(text.contains("Enter opens it in Settings › Presets"));
        });
    }

    #[test]
    fn space_turns_the_automation_on_and_off_and_saves_it() {
        in_config(|| {
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            assert_eq!(view(&app).automation(), Automation::BugFixer);
            assert!(!app.automations.is_on(Automation::BugFixer));
            press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            assert!(app.automations.is_on(Automation::BugFixer));
            let saved = crate::config::Config::load();
            assert!(saved.automations.is_on(Automation::BugFixer), "saved");
            press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            assert!(!app.automations.is_on(Automation::BugFixer));
        });
    }

    #[test]
    fn arrows_sideways_move_the_cap() {
        in_config(|| {
            let mut app = App::new();
            app.automations.settings.max_running = 5;
            open(&mut app);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE);
            assert_eq!(app.automations.settings.max_running, 6);
            press(&mut app, KeyCode::Left, KeyModifiers::NONE);
            press(&mut app, KeyCode::Left, KeyModifiers::NONE);
            assert_eq!(crate::config::Config::load().automations.max_running, 4);
        });
    }

    #[test]
    fn the_extra_steer_is_written_on_its_row() {
        in_config(|| {
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(view(&app).in_panel, "Enter takes the keys to the panel");
            // On, Preset, Extra steer.
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(view(&app).steer.is_some());
            for c in "Be brief".chars() {
                press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(view(&app).steer.is_none());
            assert_eq!(
                app.automations.settings.of(Automation::PrFixer).extra_steer,
                "Be brief"
            );
            assert!(screen(&mut app).contains("Be brief"));
            // Esc while writing drops the edit, not the screen.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(view(&app).steer.is_none());
            assert_eq!(
                app.automations.settings.of(Automation::PrFixer).extra_steer,
                "Be brief"
            );
            // Esc on the panel is back to the list; on the list it closes.
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(!view(&app).in_panel);
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.overlay.is_none());
        });
    }

    #[test]
    fn enter_on_preset_opens_it_in_settings_presets() {
        in_config(|| {
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            match &app.overlay {
                Some(Overlay::Settings(v)) => {
                    assert_eq!(v.tab, crate::config::presets_tab())
                }
                other => panic!("expected Settings › Presets, got {other:?}"),
            }
        });
    }

    #[test]
    fn command_u_picks_another_preset_and_comes_back() {
        in_config(|| {
            let mut presets = crate::agent_presets::load_seeded();
            presets.insert(
                0,
                crate::agent_presets::AgentPreset {
                    name: "Careful".into(),
                    text: "Go slowly.\n\n{task}".into(),
                    system: None,
                    ..crate::agent_presets::SystemPreset::PrAutofix.default_preset()
                },
            );
            crate::agent_presets::save(&presets).unwrap();
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Char('u'), KeyModifiers::SUPER);
            assert!(matches!(&app.overlay, Some(Overlay::AgentPresets(_))));
            // Esc there comes back unchanged.
            crate::preset_overlays::handle_list_key(
                &mut app,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                &mut Vec::new(),
            );
            assert!(matches!(&app.overlay, Some(Overlay::Automations(_))));
            assert_eq!(
                app.automations.settings.of(Automation::PrFixer).preset,
                None
            );
            // The picker opens on the preset it is told by now; Enter on
            // "Careful" makes that the PR fixer's.
            press(&mut app, KeyCode::Char('u'), KeyModifiers::SUPER);
            let Some(Overlay::AgentPresets(picker)) = &app.overlay else {
                unreachable!()
            };
            assert_eq!(picker.presets[picker.selected].name, "PR autofix");
            for code in [KeyCode::Char('c'), KeyCode::Char('a'), KeyCode::Enter] {
                crate::preset_overlays::handle_list_key(
                    &mut app,
                    KeyEvent::new(code, KeyModifiers::NONE),
                    &mut Vec::new(),
                );
            }
            assert!(matches!(&app.overlay, Some(Overlay::Automations(_))));
            assert_eq!(
                app.automations
                    .settings
                    .of(Automation::PrFixer)
                    .preset
                    .as_deref(),
                Some("Careful")
            );
            let preset = super::super::preset_of(&app.automations.settings, Automation::PrFixer);
            assert_eq!(preset.name, "Careful");
            assert!(screen(&mut app).contains("Careful · "));
        });
    }

    /// Week in review's panel: `r run now` at its top right, Whose
    /// switched by Enter, When written as a line, and Enter on a REVIEWS
    /// row opens that run, Esc coming back here.
    #[test]
    fn week_in_review_runs_now_and_opens_its_reviews() {
        in_config(|| {
            let dir = tempfile::tempdir().unwrap();
            let run = dir.path().join("all").join("2026-11-06");
            std::fs::create_dir_all(&run).unwrap();
            let meta = crate::week_review::Meta {
                from: "2026-10-31".into(),
                to: "2026-11-06".into(),
                reviews: vec![crate::week_review::Kept {
                    project: "demo".into(),
                    slug: "demo".into(),
                    points: 1,
                    ..Default::default()
                }],
                ..Default::default()
            };
            crate::pr_cache::write_json_atomic(&run.join("meta.json"), &meta).unwrap();
            std::fs::write(
                run.join("demo.md"),
                "# Week in review\n\n## Grid\n1. Rows fold · #4\n",
            )
            .unwrap();
            let mut app = App::new();
            app.week_review.root = Some(dir.path().to_path_buf());
            crate::week_review::reload(&mut app);
            app.automations.selected = 3;
            open(&mut app);
            assert_eq!(view(&app).automation(), Automation::WeekReview);
            let text = screen(&mut app);
            assert!(text.contains("r run now"), "{text}");
            assert!(text.contains("REVIEWS"), "{text}");
            assert!(text.contains("Week 45"), "{text}");
            assert!(text.contains("1 point · new"), "{text}");

            // On, When, Whose, Preset, Extra steer, then the review.
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(view(&app).editing, Some(SettingRow::When));
            for _ in 0..9 {
                press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
            }
            for c in "mon 9:15".chars() {
                press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let own = app.automations.settings.of(Automation::WeekReview);
            assert_eq!(
                own.when.map(|w| (w.day, w.at)),
                Some(("mon".into(), "09:15".into()))
            );
            assert!(screen(&mut app).contains("Mon 09:15"));
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(view(&app)
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("isn't a day")));
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(
                app.automations.settings.of(Automation::WeekReview).whose,
                crate::automations::config::Whose::WholeTeam
            );

            for _ in 0..3 {
                press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let Some(Overlay::WeekReview(page)) = &app.overlay else {
                panic!("the run's page: {:?}", app.overlay)
            };
            assert_eq!(page.dir.as_ref(), Some(&run));
            assert!(page.texts[0].contains("Rows fold"));
            assert!(app.week_review.kept[0].meta.read);
            crate::week_review::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            );
            assert_eq!(
                view(&app).automation(),
                Automation::WeekReview,
                "back on ⌘J"
            );

            // `r`: nothing finished anywhere, so the run ends at once.
            press(&mut app, KeyCode::Char('r'), KeyModifiers::NONE);
            assert!(app.week_review.run.is_none());
            assert!(app
                .flash
                .as_deref()
                .is_some_and(|f| f.contains("nothing finished")));
        });
    }

    /// INPUT PARITY: a click on a list row is the arrows landing there; a
    /// click on a panel row is Enter on it.
    #[test]
    fn a_click_is_the_keys_choice() {
        let click = |app: &mut App, hit: Hit| {
            screen(app);
            let rect = view(app)
                .hits
                .iter()
                .find(|(_, h)| *h == hit)
                .map(|(r, _)| *r)
                .unwrap();
            let at = Position::new(rect.x + 2, rect.y);
            let mouse = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: at.x,
                row: at.y,
                modifiers: KeyModifiers::NONE,
            };
            handle_mouse(app, mouse, at, &mut Vec::new());
        };
        // Where each lands: the row, which side has the keys, and whether
        // the feature builder is on.
        let landed = |app: &App| {
            let v = view(app);
            let on = app.automations.is_on(Automation::FeatureBuilder);
            (v.selected, v.in_panel, v.row, on)
        };
        let by_key = in_config(|| {
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            let moved = landed(&app);
            // Into the panel, then Enter on its On row.
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            (moved, landed(&app))
        });
        let by_click = in_config(|| {
            let mut app = App::new();
            open(&mut app);
            click(&mut app, Hit::Automation(2));
            let moved = landed(&app);
            click(&mut app, Hit::Item(0));
            (moved, landed(&app))
        });
        assert_eq!(by_key, by_click);
        assert!(by_key.1 .3, "the feature builder is on");
    }
}
