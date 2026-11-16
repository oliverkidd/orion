//! The AUTOMATIONS modal (`⌘J`): the automations on the left — a dot
//! each (yellow working, red waiting on you, grey idle, hollow off) and a
//! short state — and on the right the selected one's page: its runner's
//! lists ([`super::Runner::panel`]), what it is doing now (YOUR OPEN PRS,
//! RUNNING) and what it did before (PAST RUNS, REVIEWS, MEETINGS, IDEAS),
//! newest first and however long ago. The header says how many automation
//! sessions are running against the overall cap. Everything they are set
//! up by lives in Settings › Factory ([`super::settings_tab`]).
//!
//! Keys, as the pull requests modal has them, one an action: `↑`/`↓` move
//! in the side with the keys, `Tab`/`⇧Tab` move the keys between the list
//! and the page, Enter on an automation goes to its page and on a page row
//! opens it ([`enter_on`]): its session, else the runner's own (a review,
//! a meeting's takeaways), else its pull request or ticket in the browser,
//! which `⌘O` opens on any row with one. `⌘G` runs the automation now
//! where it can be (week in review); `⌘,` opens its settings. On the list
//! `Space` turns it on or off; on the page it is part of the filter.
//! Typing filters the page, every word in a row's text, state or list
//! title; Esc clears the filter, then steps back to the list, then
//! closes. The page's cursor stays on its row as rows come and go
//! ([`AutomationsView::pinned`]). INPUT PARITY: a click on a row puts the
//! cursor there, and a click on the row it is on is Enter on it.
//!
//! The same modal hosts the Linear labels pick for **Labels** in
//! Settings › Factory, which comes back there when it is done.

use super::config::AutomationsSettings;
use super::{Automation, Dot, Engine, Panel, PanelRow, Tone};
use crate::app::{App, Overlay};
use crate::hints::{Hint, Key};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use orion_core::{AgentId, ClientRequest};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub(crate) mod keys {
    use crate::hints::Key;

    pub const ROWS: Key = crate::nav::ROWS;
    pub const SIDE: Key = crate::nav::PANELS.doing("list / page");
    pub const OPEN: Key = crate::nav::OPEN;
    pub const BROWSER: Key = crate::hints::BROWSER;
    pub const TOGGLE: Key = Key::new(&["space"], "on / off");
    pub const SETTINGS: Key = Key::new(&["cmd+,", "ctrl+,"], "settings");
    pub const BACK: Key = crate::nav::CLOSE.doing("back");
    #[cfg(test)]
    pub const ALL: &[Key] = &[ROWS, SIDE, OPEN, BROWSER, TOGGLE, SETTINGS, BACK];
}

/// What the dim line above the keys says.
const EXPLAIN: &str =
    "What the automations are doing and what they did. Their caps, labels, presets and steer are in Settings › Factory (⌘,).";
/// What the filter line says with nothing typed.
const FILTER_HINT: &str = "type to filter";

/// The widest the screen gets.
const WIDEST: u16 = 124;
/// The tallest.
const TALLEST: u16 = 40;
/// The fewest rows it shrinks to.
const SHORTEST: u16 = 12;
/// The automations' column, and the gap after it.
const LEFT_W: u16 = 28;
const GAP: u16 = 2;
/// An automation on the left: its name, then its state.
const LINES_EACH: usize = 2;
/// What the modal spends besides the body: its borders, the header and
/// the gap under it, the explanation and the gap over it.
const CHROME: usize = 7;
/// The page's own top: its title and the filter line, never scrolled.
const PAGE_TOP: u16 = 2;

/// Which side of the modal has the keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Side {
    /// The automations.
    #[default]
    List,
    /// The selected automation's page.
    Page,
}

/// What a click lands on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hit {
    /// An automation, by its [`Automation::ALL`] index.
    Automation(usize),
    /// A row of the page, by its index among [`page_rows`].
    Row(usize),
    /// A row of the labels pick.
    Label(usize),
}

#[derive(Debug, Clone, Default)]
pub struct AutomationsView {
    /// The automation under the cursor, by its [`Automation::ALL`] index.
    pub selected: usize,
    /// The side with the keys.
    pub side: Side,
    /// The page row under the cursor, an index into [`page_rows`].
    pub row: usize,
    /// The row under the cursor as the last frame had it ([`row_key`]):
    /// where the cursor goes back to when rows above it come or go.
    pub pinned: Option<String>,
    /// What the page is filtered by.
    pub filter: TextInput,
    /// **Labels** in Settings › Factory: the labels pick, the whole
    /// modal, back on [`Self::back`] once it is done.
    pub labels: Option<super::linear_builder::LabelPick>,
    /// The Settings › Factory row the labels pick goes back to.
    pub back: usize,
    /// A word about the last thing tried, over the explanation.
    pub notice: Option<String>,
    /// The modal's rect and its rows', written back as it draws.
    pub area: Rect,
    hits: Vec<(Rect, Hit)>,
}

impl AutomationsView {
    /// The modal standing in for Settings › Factory while a runner's own
    /// editor (the labels pick) is up, back on row `at` after.
    pub fn editing_from(at: usize) -> Self {
        Self {
            back: at,
            ..Self::default()
        }
    }

    /// The automation under the cursor.
    pub fn automation(&self) -> Automation {
        Automation::ALL[self.selected.min(Automation::ALL.len() - 1)]
    }

    /// The cursor on page row `at`, by index: the next frame pins it.
    fn put_row(&mut self, at: usize) {
        self.row = at;
        self.pinned = None;
    }
}

/// A row of an automation's page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRow {
    /// Its list, by index among the panel's.
    pub list: usize,
    pub row: PanelRow,
    /// Among the rows with no session, counted across the lists unfiltered:
    /// what [`super::Runner::open_row`] is told.
    pub runner_index: Option<usize>,
}

/// What Enter does on a page row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enter {
    /// Blue sky's idea, at its session.
    Idea(AgentId),
    /// Its session.
    Jump(AgentId),
    /// The runner's own: the `n`th row with no session.
    Runner(usize),
    /// Its pull request or ticket in the browser.
    Browser(String),
    Nothing,
}

impl Enter {
    /// What the keys call it.
    fn label(&self) -> &'static str {
        match self {
            Enter::Idea(_) => "idea",
            Enter::Jump(_) => "jump",
            Enter::Runner(_) => "open",
            Enter::Browser(_) => "browser",
            Enter::Nothing => "open",
        }
    }
}

/// What Enter does on `row` of `which`'s page: a session jumps to it (blue
/// sky's to its idea), a runner that opens its own rows opens it, a link
/// opens in the browser.
pub fn enter_on(app: &App, which: Automation, row: &PageRow) -> Enter {
    let own = app
        .automations
        .runner(which)
        .is_some_and(|r| r.opens_rows());
    match (&row.row.session, row.runner_index, &row.row.url) {
        (Some(id), _, _) if which == Automation::BlueSky => Enter::Idea(id.clone()),
        (Some(id), _, _) => Enter::Jump(id.clone()),
        (None, Some(n), _) if own => Enter::Runner(n),
        (None, _, Some(url)) => Enter::Browser(url.clone()),
        _ => Enter::Nothing,
    }
}

/// What the cursor holds on to across frames: the row's session, else
/// its link, else its text.
fn row_key(row: &PageRow) -> String {
    match (&row.row.session, &row.row.url) {
        (Some(id), _) => format!("s:{}", id.0),
        (None, Some(url)) => format!("u:{url}"),
        (None, None) => format!("t:{}", row.row.text),
    }
}

/// Whether `row` of the list titled `title` has every word of `words`
/// (lowercased) in its text, its state or the title.
fn kept_by_filter(words: &[String], title: &str, row: &PanelRow) -> bool {
    if words.is_empty() {
        return true;
    }
    let hay = format!("{} {} {}", row.text, row.state, title).to_lowercase();
    words.iter().all(|word| hay.contains(word.as_str()))
}

/// `panel`'s rows that `filter` keeps, in its lists' order.
pub fn page_rows(panel: &Panel, filter: &str) -> Vec<PageRow> {
    let words: Vec<String> = filter
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let mut runner_index = 0;
    let mut rows = Vec::new();
    for (list, l) in panel.lists.iter().enumerate() {
        for row in &l.rows {
            let index = match row.session {
                Some(_) => None,
                None => {
                    runner_index += 1;
                    Some(runner_index - 1)
                }
            };
            if kept_by_filter(&words, l.title, row) {
                rows.push(PageRow {
                    list,
                    row: row.clone(),
                    runner_index: index,
                });
            }
        }
    }
    rows
}

/// `⌘J`: the modal, on the automation it was last left on.
pub fn open(app: &mut App) {
    app.overlay = Some(Overlay::Automations(AutomationsView {
        selected: app.automations.selected.min(Automation::ALL.len() - 1),
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

/// The view, changed by `change`, when the modal is up.
fn with_view(app: &mut App, change: impl FnOnce(&mut AutomationsView)) {
    if let Some(Overlay::Automations(view)) = &mut app.overlay {
        change(view);
    }
}

/// Say `why` over the explanation.
fn notice(app: &mut App, why: impl Into<String>) {
    let why = why.into();
    with_view(app, |v| v.notice = Some(why));
}

/// The rows of `view`'s page as they stand.
fn rows_of(app: &App, view: &AutomationsView) -> Vec<PageRow> {
    page_rows(&Engine::panel(app, view.automation()), view.filter.as_str())
}

/// A key that starts a filter from the list: a character with no
/// modifier but shift.
fn starts_filter(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_)) && (key.modifiers - KeyModifiers::SHIFT).is_empty()
}

pub fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Automations(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    view.notice = None;
    // Picking labels: the pick has the keys, and its Enter or Esc goes
    // back to Settings › Factory.
    if view.labels.is_some() {
        let back = view.back;
        super::linear_builder::pick_key(app, key);
        if matches!(&app.overlay, Some(Overlay::Automations(v)) if v.labels.is_none()) {
            super::settings_tab::reopen_at(app, back);
        }
        return;
    }
    let (which, side, selected, row) = (view.automation(), view.side, view.selected, view.row);
    // The runner's own key (week in review's `⌘G`).
    let own = app.automations.runner(which).and_then(|r| r.hotkey());
    if own.is_some_and(|k| k.matches(&key)) {
        if let Some(Err(why)) = super::with_runner(app, which, |r, app| r.hotkey_pressed(app)) {
            notice(app, why);
        }
        return;
    }
    if keys::BACK.matches(&key) {
        back(app);
        return;
    }
    if keys::SETTINGS.matches(&key) {
        close(app);
        super::settings_tab::open_at(app, Some(which));
        return;
    }
    if crate::nav::panel(&key).is_some() {
        with_view(app, |v| {
            v.side = match side {
                Side::List => Side::Page,
                Side::Page => Side::List,
            }
        });
        return;
    }
    match side {
        Side::List => match key.code {
            _ if keys::TOGGLE.matches(&key) => super::settings_tab::toggle(app, which),
            _ if keys::ROWS.matches(&key) => {
                let down = key.code == KeyCode::Down;
                select(app, step(selected, down, Automation::ALL.len()));
            }
            _ if keys::OPEN.matches(&key) => to_page(app),
            _ if starts_filter(&key) => with_view(app, |v| {
                v.side = Side::Page;
                v.filter.handle_key(&key);
                v.put_row(0);
            }),
            _ => {}
        },
        Side::Page => match key.code {
            _ if keys::ROWS.matches(&key) => {
                let n = with_rows(app, |rows| rows.len());
                with_view(app, |v| v.put_row(step(row, key.code == KeyCode::Down, n)));
            }
            _ if keys::OPEN.matches(&key) => activate(app, row, out),
            _ if keys::BROWSER.matches(&key) => {
                let url = with_rows(app, |rows| rows.get(row).and_then(|r| r.row.url.clone()));
                browse(app, url, out);
            }
            _ => with_view(app, |v| {
                if v.filter.handle_key(&key).changed() {
                    v.put_row(0);
                }
            }),
        },
    }
}

/// `f` over the modal's page rows as they stand.
fn with_rows<T>(app: &App, f: impl FnOnce(&[PageRow]) -> T) -> T {
    let rows = match &app.overlay {
        Some(Overlay::Automations(view)) => rows_of(app, view),
        _ => Vec::new(),
    };
    f(&rows)
}

/// Esc: the filter cleared, else back to the list, else closed.
fn back(app: &mut App) {
    let Some(Overlay::Automations(view)) = &app.overlay else {
        return;
    };
    match (view.side, view.filter.is_empty()) {
        (Side::Page, false) => with_view(app, |v| {
            v.filter.clear();
            v.put_row(0);
        }),
        (Side::Page, true) => with_view(app, |v| v.side = Side::List),
        (Side::List, _) => close(app),
    }
}

/// Enter on an automation: its page has the keys, when there is anything
/// on it — filtered or not.
fn to_page(app: &mut App) {
    let Some(Overlay::Automations(view)) = &app.overlay else {
        return;
    };
    let which = view.automation();
    let empty = page_rows(&Engine::panel(app, which), "").is_empty();
    if empty {
        let run = app
            .automations
            .runner(which)
            .and_then(|r| r.hotkey())
            .map(|k| format!(" · {} runs it now", k.label()))
            .unwrap_or_default();
        notice(app, format!("Nothing on {}'s page yet{run}", which.name()));
        return;
    }
    with_view(app, |v| {
        v.side = Side::Page;
        v.put_row(0);
    });
}

/// `url` — a row's pull request or ticket — in the browser, a pull request
/// marked read ([`crate::event_loop::open_link`]); without one, say so.
fn browse(app: &mut App, url: Option<String>, out: &mut Vec<ClientRequest>) {
    match url {
        Some(url) => crate::event_loop::open_link(app, &url, None, out),
        None => notice(app, "Nothing on this row to open in the browser"),
    }
}

/// `at` a step up or down a list of `n`, kept on it.
fn step(at: usize, down: bool, n: usize) -> usize {
    match down {
        true => (at + 1).min(n.saturating_sub(1)),
        false => at.saturating_sub(1),
    }
}

/// The cursor on automation `at`, remembered for the next open; its page
/// from the top, unfiltered.
fn select(app: &mut App, at: usize) {
    let at = at.min(Automation::ALL.len() - 1);
    app.automations.selected = at;
    with_view(app, |v| {
        if v.selected != at {
            v.filter.clear();
        }
        v.selected = at;
        v.put_row(0);
    });
}

/// Close the screen.
fn close(app: &mut App) {
    app.overlay = None;
}

/// Page row `index` chosen — Enter on it, or a click on the row the
/// cursor is on — and done as [`enter_on`] says.
pub fn activate(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Automations(view)) = &app.overlay else {
        return;
    };
    let which = view.automation();
    let Some(enter) = with_rows(app, |rows| rows.get(index).map(|r| enter_on(app, which, r)))
    else {
        return;
    };
    with_view(app, |v| v.put_row(index));
    match enter {
        Enter::Idea(id) => {
            close(app);
            super::blue_sky::open_idea(app, &id);
        }
        Enter::Jump(id) => {
            close(app);
            crate::event_loop::jump_to_session(app, id, out);
        }
        Enter::Runner(n) => {
            close(app);
            super::with_runner(app, which, |r, app| r.open_row(app, n));
        }
        Enter::Browser(url) => browse(app, Some(url), out),
        Enter::Nothing => notice(app, "Nothing to open on this row"),
    }
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
    let (side, selected, row, labels) = (view.side, view.selected, view.row, view.labels.is_some());
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if !labels => {
            let down = matches!(mouse.kind, MouseEventKind::ScrollDown);
            match side {
                Side::List => select(app, step(selected, down, Automation::ALL.len())),
                Side::Page => {
                    let n = with_rows(app, |rows| rows.len());
                    with_view(app, |v| v.put_row(step(row, down, n)));
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left) => match hit {
            Some(Hit::Automation(i)) if i == selected && side == Side::List => to_page(app),
            Some(Hit::Automation(i)) => {
                select(app, i);
                with_view(app, |v| v.side = Side::List);
            }
            Some(Hit::Row(i)) if i == row && side == Side::Page => activate(app, i, out),
            Some(Hit::Row(i)) => with_view(app, |v| {
                v.side = Side::Page;
                v.put_row(i);
            }),
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
pub(crate) fn dot(dot: Dot, th: Theme) -> Span<'static> {
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

pub fn hints(app: &App, view: &AutomationsView) -> Vec<Hint> {
    let rows = match view.side {
        Side::Page => rows_of(app, view),
        Side::List => Vec::new(),
    };
    hints_for(app, view, &rows)
}

/// The keys, over the page's `rows` as they stand.
fn hints_for(app: &App, view: &AutomationsView, rows: &[PageRow]) -> Vec<Hint> {
    if view.labels.is_some() {
        return vec![
            Key::new(&["space"], "tick").hint().kept(),
            Key::new(&["enter"], "save").hint().kept(),
            crate::nav::CLOSE.doing("cancel").hint().kept(),
        ];
    }
    let which = view.automation();
    let own = app.automations.runner(which).and_then(|r| r.hotkey());
    let mut hints = Vec::new();
    match view.side {
        Side::List => {
            hints.push(keys::OPEN.hint_as("page").kept());
            hints.extend(own.map(|k| k.hint()));
            hints.extend([
                keys::TOGGLE.hint(),
                keys::SIDE.hint(),
                keys::SETTINGS.hint(),
                keys::BACK.hint_as("close").kept(),
            ]);
        }
        Side::Page => {
            let row = rows.get(view.row.min(rows.len().saturating_sub(1)));
            let enter = row.map_or(Enter::Nothing, |r| enter_on(app, which, r));
            hints.push(keys::OPEN.hint_as(enter.label()).kept());
            let link = row.is_some_and(|r| r.row.url.is_some());
            if link && !matches!(enter, Enter::Browser(_)) {
                hints.push(keys::BROWSER.hint());
            }
            hints.extend(own.map(|k| k.hint()));
            let esc = match view.filter.is_empty() {
                true => "list",
                false => "clear",
            };
            hints.extend([
                keys::SIDE.hint(),
                Hint::new("type", "filter"),
                keys::SETTINGS.hint(),
                keys::BACK.hint_as(esc).kept(),
            ]);
        }
    }
    hints
}

/// `text` cut and padded to `w` cells.
fn cell(text: &str, w: usize) -> String {
    let text = crate::ui::truncate(text, w.saturating_sub(1));
    format!("{text:<w$}")
}

/// How many lines `panel`'s page wants: its top, and each list's gap,
/// title and rows (one for an empty list's line).
fn page_lines(panel: &Panel) -> usize {
    usize::from(PAGE_TOP)
        + panel
            .lists
            .iter()
            .map(|l| 2 + l.rows.len().max(1))
            .sum::<usize>()
}

pub fn draw(f: &mut Frame, app: &mut App, view: &AutomationsView, th: Theme) {
    let screen = f.area();
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let panels = Automation::ALL.map(|which| Engine::panel(app, which));
    // Tall enough for the list and the longest page, so moving down the
    // list doesn't make the modal jump.
    let tallest_page = panels.iter().map(page_lines).max().unwrap_or(0);
    let want = CHROME + tallest_page.max(Automation::ALL.len() * LINES_EACH);
    let height = u16::try_from(want)
        .unwrap_or(u16::MAX)
        .min(TALLEST)
        .min(screen.height.saturating_sub(2))
        .max(SHORTEST);
    let area = crate::ui::centered_rect(screen, width, height);
    let title = match &view.labels {
        Some(_) => " Labels ",
        None => " Automations ",
    };
    let panel = &panels[view.automation() as usize];
    let rows = page_rows(panel, view.filter.as_str());
    // The cursor back on the row it was on, wherever that row is now.
    let row = view
        .pinned
        .as_ref()
        .and_then(|key| rows.iter().position(|r| &row_key(r) == key))
        .unwrap_or_else(|| view.row.min(rows.len().saturating_sub(1)));
    let view = &AutomationsView {
        row,
        ..view.clone()
    };
    let inner = crate::ui::render_modal_frame(f, area, title, &hints_for(app, view, &rows), th);
    let explain = view.notice.as_deref().unwrap_or(EXPLAIN);
    let (body, explain_row) = crate::hints::explain_area(explain, inner, 2);
    match &view.notice {
        Some(_) => {
            crate::hints::draw_explain_as(f, explain_row, explain, Style::default().fg(th.warn))
        }
        None => crate::hints::draw_explain(f, explain_row, explain, th),
    }
    let mut hits = Vec::new();

    // The labels pick, the whole body.
    if let Some(pick) = &view.labels {
        draw_labels(f, body, pick, th, &mut hits);
        with_view(app, |live| {
            live.area = area;
            live.hits = hits;
        });
        return;
    }

    // The header: what is running against the overall cap.
    let header = format!(
        "{} of at most {} running · caps in ⌘,",
        super::running(app),
        app.automations.settings.cap()
    );
    if let Some(at) = crate::ui::row_rect(body, 0) {
        let line = spread(
            Vec::new(),
            Span::styled(header, Style::default().fg(th.muted)),
            usize::from(body.width),
        );
        f.render_widget(Paragraph::new(line), at);
    }
    let rest = Rect {
        y: body.y + 2,
        height: body.height.saturating_sub(2),
        ..body
    };
    let left_w = LEFT_W.min(rest.width / 2);
    let left = Rect {
        width: left_w,
        ..rest
    };
    let right = Rect {
        x: rest.x + left_w + GAP,
        width: rest.width.saturating_sub(left_w + GAP),
        ..rest
    };
    draw_list(f, left, view, &panels, th, &mut hits);
    draw_page(f, right, view, panel, &rows, th, &mut hits);

    let pinned = rows.get(row).map(row_key);
    with_view(app, |live| {
        live.area = area;
        live.hits = hits;
        live.row = row;
        live.pinned = pinned;
    });
}

/// The automations, [`LINES_EACH`] lines each — the dot and the name, then
/// the state — the selected one kept in view and lit as every list row is.
fn draw_list(
    f: &mut Frame,
    area: Rect,
    view: &AutomationsView,
    panels: &[Panel],
    th: Theme,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let w = usize::from(area.width).saturating_sub(1);
    let fits = (usize::from(area.height) / LINES_EACH).max(1);
    let first = (view.selected + 1).saturating_sub(fits);
    let each = LINES_EACH as u16;
    for (n, (i, (which, panel))) in Automation::ALL
        .iter()
        .zip(panels)
        .enumerate()
        .skip(first)
        .take(fits)
        .enumerate()
    {
        let y = area.y + n as u16 * each;
        if y + each > area.y + area.height {
            break;
        }
        let rect = Rect {
            y,
            height: each,
            ..area
        };
        let name = vec![
            dot(panel.dot, th),
            Span::styled(
                cell(which.name(), w.saturating_sub(2)),
                Style::default().fg(th.text),
            ),
        ];
        let state = vec![
            Span::raw("  "),
            Span::styled(
                cell(&panel.state, w.saturating_sub(2)),
                Style::default().fg(th.dim),
            ),
        ];
        crate::ui::render_row_lines(
            f,
            rect,
            vec![name, state],
            i == view.selected,
            view.side == Side::List,
            th,
        );
        hits.push((rect, Hit::Automation(i)));
    }
}

/// The selected automation's page: its name and state and the filter line
/// on top, then each list under its title — the cursor's row lit and kept
/// in view.
fn draw_page(
    f: &mut Frame,
    area: Rect,
    view: &AutomationsView,
    panel: &Panel,
    rows: &[PageRow],
    th: Theme,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let w = usize::from(area.width);
    let dim = Style::default().fg(th.dim);
    let keys = view.side == Side::Page;
    let cursor = view.row;
    if let Some(at) = crate::ui::row_rect(area, 0) {
        let title = spread(
            vec![Span::styled(
                view.automation().name().to_string(),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            )],
            Span::styled(panel.state.clone(), dim),
            w,
        );
        f.render_widget(Paragraph::new(title), at);
    }
    if let Some(at) = crate::ui::row_rect(area, 1) {
        let line = match keys {
            true => crate::ui::search_line(&view.filter, FILTER_HINT, at, th),
            false => crate::ui::search_line_idle(&view.filter, FILTER_HINT, at, th),
        };
        f.render_widget(Paragraph::new(line), at);
    }
    let lists = Rect {
        y: area.y + PAGE_TOP,
        height: area.height.saturating_sub(PAGE_TOP),
        ..area
    };
    let filtered = !view.filter.is_empty();
    // Each line: a title or a gap (None), or row `i` (its spans).
    let mut lines: Vec<(Option<usize>, Vec<Span<'static>>)> = Vec::new();
    for (list, l) in panel.lists.iter().enumerate() {
        let of_list: Vec<(usize, &PageRow)> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.list == list)
            .collect();
        if filtered && of_list.is_empty() {
            continue;
        }
        lines.push((None, Vec::new()));
        lines.push((
            None,
            spread(
                vec![Span::styled(l.title.to_string(), dim)],
                Span::styled(of_list.len().to_string(), dim),
                w,
            )
            .spans,
        ));
        if of_list.is_empty() {
            lines.push((None, vec![Span::styled(format!("  {}", l.empty), dim)]));
        }
        for (i, r) in of_list {
            // One cell of the row's gutter, one in, one gap, one after.
            let room = w.saturating_sub(4);
            let state_w = r.row.state.chars().count().min(room / 2);
            let state = crate::ui::truncate(&r.row.state, state_w);
            let text_w = room.saturating_sub(state.chars().count());
            let spans = spread(
                vec![
                    Span::raw(" "),
                    Span::styled(cell(&r.row.text, text_w), Style::default().fg(th.text)),
                ],
                Span::styled(format!("{state} "), tone(r.row.tone, th)),
                w.saturating_sub(1),
            )
            .spans;
            lines.push((Some(i), spans));
        }
    }
    if filtered && rows.is_empty() {
        lines.push((None, Vec::new()));
        lines.push((None, vec![Span::styled("  Nothing matches", dim)]));
    }
    let fits = usize::from(lists.height).max(1);
    let cursor_line = lines
        .iter()
        .position(|(at, _)| *at == Some(cursor))
        .unwrap_or(0);
    let first = (cursor_line + 1).saturating_sub(fits);
    for (n, (at, spans)) in lines.into_iter().skip(first).take(fits).enumerate() {
        let Some(rect) = crate::ui::row_rect(lists, n) else {
            break;
        };
        match at {
            Some(i) => {
                crate::ui::render_row(f, rect, spans, i == cursor, keys, th);
                hits.push((rect, Hit::Row(i)));
            }
            None => f.render_widget(Paragraph::new(Line::from(spans)), rect),
        }
    }
}

/// The labels pick: a tick box per label, the cursor's row lit; each row a
/// click target.
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
    use orion_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
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

    /// A bug fixer session planning ENG-412, as its ledger and RUNNING
    /// list have it.
    fn bug_fixer_session(app: &mut App) -> AgentId {
        let id = AgentId("s1".into());
        app.tree.agents.push(Agent {
            id: id.clone(),
            worktree_id: WorktreeId("w".into()),
            name: "fix".into(),
            status: AgentStatus::Running,
            archived: false,
            archived_at: 0,
            unseen: false,
            status_changed_at: 0,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: Some("bug_fixer".into()),
        });
        app.automations.linear_builder.ledger.issues.insert(
            "i1".into(),
            super::super::linear_builder::Entry {
                automation: "bug_fixer".into(),
                identifier: "ENG-412".into(),
                title: "Crash on save".into(),
                project: orion_core::ProjectId("p".into()),
                branch: "eng-412".into(),
                session: Some(id.clone()),
                stage: super::super::linear_builder::Stage::Planning,
                at: 0,
                ..Default::default()
            },
        );
        id
    }

    /// A bug fixer ticket that merged `days` ago, as the ledger keeps it.
    fn merged_ticket(app: &mut App, id: &str, days: i64) {
        app.automations.linear_builder.ledger.issues.insert(
            id.into(),
            super::super::linear_builder::Entry {
                automation: "bug_fixer".into(),
                identifier: id.into(),
                title: "Old crash".into(),
                project: orion_core::ProjectId("p".into()),
                branch: id.to_lowercase(),
                stage: super::super::linear_builder::Stage::Merged,
                at: crate::app::now_ms() - days * orion_core::clock::DAY_MS,
                pr_url: Some(format!("https://github.com/o/app/pull/{days}")),
                ..Default::default()
            },
        );
    }

    #[test]
    fn every_key_parses() {
        assert!(keys::ALL.iter().all(Key::parses));
    }

    #[test]
    fn the_modal_lists_every_automation_and_the_running_count() {
        in_config(|| {
            let mut app = App::new();
            open(&mut app);
            let text = screen(&mut app);
            for which in Automation::ALL {
                assert!(text.contains(which.name()), "{}:\n{text}", which.name());
            }
            assert!(
                text.contains("0 of at most 3 running · caps in ⌘,"),
                "{text}"
            );
            // The PR fixer's page beside the list.
            assert!(
                text.contains("YOUR OPEN PRS") && text.contains("PAST RUNS"),
                "{text}"
            );
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

    /// Enter on an automation hands the keys to its page; Enter on a row
    /// with a session jumps to it; `⌘,` opens its settings.
    #[test]
    fn enter_goes_to_the_page_and_a_session_row_jumps() {
        in_config(|| {
            let mut app = App::new();
            app.automations.settings.of_mut(Automation::BugFixer).on = Some(true);
            let id = bug_fixer_session(&mut app);
            open(&mut app);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            let text = screen(&mut app);
            assert!(
                text.contains("RUNNING") && text.contains("ENG-412"),
                "{text}"
            );
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(view(&app).side, Side::Page);
            let rows = rows_of(&app, view(&app));
            assert_eq!(rows[0].row.session.as_ref(), Some(&id));
            assert!(hints(&app, view(&app)).iter().any(|h| h.does == "jump"));
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "jumped to the session");

            open(&mut app);
            press(&mut app, KeyCode::Char(','), KeyModifiers::SUPER);
            let Some(Overlay::Settings(s)) = &app.overlay else {
                panic!("Settings: {:?}", app.overlay)
            };
            assert_eq!(s.tab, crate::config::automations_tab());
            assert_eq!(
                super::super::settings_tab::entry(s.selected),
                Some(super::super::settings_tab::Entry::Row(
                    Automation::BugFixer,
                    super::super::SettingRow::On
                ))
            );
        });
    }

    /// PAST RUNS reach back past a week, each with when it ended and its
    /// pull request a `⌘O` away.
    #[test]
    fn past_runs_reach_back_past_a_week() {
        in_config(|| {
            let mut app = App::new();
            merged_ticket(&mut app, "ENG-300", 30);
            app.automations.selected = 1;
            open(&mut app);
            let text = screen(&mut app);
            assert!(text.contains("ENG-300 Old crash"), "{text}");
            assert!(text.contains("merged ·"), "{text}");
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_eq!(view(&app).side, Side::Page);
            assert!(hints(&app, view(&app)).iter().any(|h| h.does == "browser"));
            crate::open_url::take_opened();
            press(&mut app, KeyCode::Char('o'), KeyModifiers::SUPER);
            assert_eq!(
                crate::open_url::take_opened(),
                vec!["https://github.com/o/app/pull/30".to_string()]
            );
            // Its session long gone, Enter opens the pull request too.
            let row = &rows_of(&app, view(&app))[0];
            assert_eq!(
                enter_on(&app, Automation::BugFixer, row),
                Enter::Browser("https://github.com/o/app/pull/30".into())
            );
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(crate::open_url::take_opened().len(), 1);
        });
    }

    /// The page's cursor stays on its row when a newer run lands above it.
    #[test]
    fn the_cursor_keeps_its_row_as_rows_come() {
        in_config(|| {
            let mut app = App::new();
            merged_ticket(&mut app, "ENG-300", 30);
            merged_ticket(&mut app, "ENG-301", 2);
            app.automations.selected = 1;
            open(&mut app);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            screen(&mut app);
            let on = |app: &App| rows_of(app, view(app))[view(app).row].row.text.clone();
            assert!(on(&app).starts_with("ENG-300"));
            merged_ticket(&mut app, "ENG-302", 1);
            screen(&mut app);
            assert!(on(&app).starts_with("ENG-300"), "{}", on(&app));
        });
    }

    /// Typing filters the page from either side; Esc clears the filter,
    /// then steps back to the list, then closes.
    #[test]
    fn typing_filters_the_page_and_esc_steps_back() {
        in_config(|| {
            let mut app = App::new();
            merged_ticket(&mut app, "ENG-300", 30);
            merged_ticket(&mut app, "ENG-301", 2);
            app.automations.selected = 1;
            open(&mut app);
            for c in "301".chars() {
                press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
            }
            assert_eq!(view(&app).side, Side::Page);
            assert_eq!(view(&app).filter.as_str(), "301");
            let rows = rows_of(&app, view(&app));
            assert_eq!(rows.len(), 1);
            assert!(rows[0].row.text.starts_with("ENG-301"));
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(view(&app).filter.as_str(), "");
            assert_eq!(rows_of(&app, view(&app)).len(), 2);
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(view(&app).side, Side::List);
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.overlay.is_none());
        });
    }

    /// Week in review runs on `⌘G`, and its empty page says so.
    #[test]
    fn week_review_runs_on_cmd_g_and_says_when_its_page_is_empty() {
        in_config(|| {
            let mut app = App::new();
            app.automations.selected = 3;
            open(&mut app);
            assert_eq!(view(&app).automation(), Automation::WeekReview);
            assert!(hints(&app, view(&app)).iter().any(|h| h.does == "run now"));
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(
                view(&app)
                    .notice
                    .as_deref()
                    .is_some_and(|n| n.contains("Nothing on Week in review's page yet · ⌘G")),
                "{:?}",
                view(&app).notice
            );
            assert_eq!(view(&app).side, Side::List);
        });
    }

    /// INPUT PARITY: a click on an automation is the arrows landing there;
    /// a click on the one the cursor is on is Enter on it.
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
        let landed = |app: &App| match &app.overlay {
            Some(Overlay::Automations(v)) => {
                format!("{} {:?} {} {:?}", v.selected, v.side, v.row, v.notice)
            }
            other => format!("{other:?}"),
        };
        let setup = |app: &mut App| {
            merged_ticket(app, "ENG-300", 30);
            merged_ticket(app, "ENG-301", 2);
            open(app);
        };
        let by_key = in_config(|| {
            let mut app = App::new();
            setup(&mut app);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            let moved = landed(&app);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            let paged = landed(&app);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            (moved, paged, landed(&app))
        });
        let by_click = in_config(|| {
            let mut app = App::new();
            setup(&mut app);
            click(&mut app, Hit::Automation(1));
            let moved = landed(&app);
            click(&mut app, Hit::Automation(1));
            let paged = landed(&app);
            click(&mut app, Hit::Row(1));
            (moved, paged, landed(&app))
        });
        assert_eq!(by_key, by_click);
        assert!(by_key.1.contains("Page"), "{by_key:?}");
    }
}
