//! The TODOS MODAL: one project's list in two tabs — **Today**, the
//! groups with their open items and the ones ticked today struck through
//! at the bottom, and the **Log**, what was done on each day before. A
//! line per item — more where its text wraps: its box, its priority as
//! the one letter Linear's rows use too, the text, and on the right what
//! it is tied to — the agent sent at it, the Linear issue it is linked
//! to — and how many days it has carried over. A top-level group's
//! header is a section — its name in capitals, a rule across to what is
//! open in it at each priority, nested groups and all, and what was
//! ticked today; a group under it says only how many are open. Each
//! folds (`←`/`→`), and `⌘↑`/`⌘↓` jump from header to header. The tab row
//! says what was ticked today and this week, day by day.
//!
//! The list is the [`App`]'s (`App::todos`, by checkout), so it outlives
//! the modal; the view holds the cursor, the filter and the field being
//! typed into. Every change is saved at once (`store::save`). A row is
//! edited where it stands: typing on it adds to its end, `⌫` opens it
//! with the last character gone, and `⌘⌫` deletes it. `⌘F` opens the
//! filter, which keeps the headers over the items that match; a paste of
//! more than one line, with no field open, is read as an indented list
//! and added in (`import`).
//!
//! Each item reaches out two ways. `Enter` sends an agent at it — the
//! QUICK PROMPT over the modal, the item's text and group in the box —
//! and the session it starts is written onto the item (`TodoRef`), so a
//! second `Enter` jumps to it. `⌘L` ties it to Linear: **Create in
//! Triage** files it in the team's Triage with its priority, **Link
//! existing…** picks an issue in the LINEAR VIEW, and the chip then says
//! how the issue stands, read afresh each time the modal opens (`⌘R`
//! asks again) — an issue done or canceled in Linear ticks its todo. The
//! sync is one way: ticking a todo leaves its issue alone.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::{ClientRequest, ProjectId};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use super::{store, Item, TodoFile, TodoRef};
use crate::app::{App, Overlay};
use crate::keymap::KeyChord;
pub use crate::linear::LinkedIssue;
use crate::linear::{IssueDraft, LinearTeam, TeamChoice};
use crate::quick_prompt::{ModalUnder, QuickLaunch, QuickReturn, QuickTarget};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, fit_parts_at, fuzzy_highlight_styled, input_spans, panel_block, render_row,
    row_rect, search_line, truncate, visible_positions, SPLIT_MODAL_PCT,
};

/// The modal's two lists: what is on today, and what was done before.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TodoTab {
    #[default]
    Today,
    Log,
}

impl TodoTab {
    pub const ALL: [TodoTab; 2] = [TodoTab::Today, TodoTab::Log];

    fn other(self) -> Self {
        match self {
            TodoTab::Today => TodoTab::Log,
            TodoTab::Log => TodoTab::Today,
        }
    }
}

/// A chip on an item's line that a click acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    /// `● agent`: jump to the session.
    Agent,
    /// `◑ RIP-412`: open the issue in the browser.
    Linear,
}

/// A row of the `⌘L` menu: only the ones that apply to the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Create,
    Link,
    Browser,
    Unlink,
}

impl MenuAction {
    fn label(self, linked: Option<&str>) -> String {
        match self {
            MenuAction::Create => "Create in Triage".into(),
            MenuAction::Link => "Link existing…".into(),
            MenuAction::Browser => "Open in browser".into(),
            MenuAction::Unlink => format!("Unlink {}", linked.unwrap_or("the issue")),
        }
    }
}

/// What the small list over the modal picks: a `⌘L` menu row, or the
/// team **Create in Triage** files into when there are several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickKind {
    Menu(Vec<MenuAction>),
    Team(Vec<LinearTeam>),
}

/// The small list over the modal, for `item`: every key is its own while
/// it is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub item: u64,
    pub kind: PickKind,
    pub selected: usize,
    /// Where it was last drawn, its rows and the first one showing: what
    /// a click hits.
    pub area: Rect,
    pub rows: Rect,
    pub start: usize,
}

impl Pick {
    fn new(item: u64, kind: PickKind) -> Self {
        Self {
            item,
            kind,
            selected: 0,
            area: Rect::default(),
            rows: Rect::default(),
            start: 0,
        }
    }

    fn len(&self) -> usize {
        match &self.kind {
            PickKind::Menu(rows) => rows.len(),
            PickKind::Team(teams) => teams.len(),
        }
    }
}

/// A row the keys act on: a group's header, or an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Group(u64),
    Item(u64),
}

/// What the field in the list is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// A new item in this group — or, with none, in the `Inbox`.
    Item {
        group: Option<u64>,
    },
    /// A new group under `parent` (`None`: at the top).
    Group {
        parent: Option<u64>,
    },
    Rename(Target),
}

/// One row of the list as drawn, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// A group's header, `depth` groups in.
    Header {
        group: u64,
        depth: u16,
    },
    Item {
        id: u64,
        depth: u16,
    },
    /// The field being typed into, where what it makes will stand.
    Input {
        depth: u16,
    },
    /// `+ new item`, closing Today.
    AddRow,
    /// A day in the LOG, over what was done on it. Never the cursor's.
    Day {
        date: NaiveDate,
        count: usize,
    },
}

impl Entry {
    fn selectable(&self) -> bool {
        !matches!(self, Entry::Day { .. })
    }

    fn depth(&self) -> u16 {
        match self {
            Entry::Header { depth, .. } | Entry::Item { depth, .. } | Entry::Input { depth } => {
                *depth
            }
            Entry::AddRow | Entry::Day { .. } => 0,
        }
    }
}

/// The modal's own state. The list lives on [`App::todos`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoView {
    pub project: ProjectId,
    pub project_name: String,
    /// The project's checkout: whose list this is.
    pub dir: PathBuf,
    pub tab: TodoTab,
    /// The cursor's row, by index into the rows as last drawn; and the row
    /// itself, which it follows when the rows move under it (a priority
    /// re-sorts them). A tick lets go of the row, so the cursor stays put
    /// and the next one is under it.
    pub selected: usize,
    pub cursor: Option<Entry>,
    pub query: TextInput,
    /// The filter row is up (`⌘F`): typing is the filter's, not the row's.
    pub filtering: bool,
    /// The field in the list — a new item, a new group, a row being
    /// edited — while one is open: every key but the hatches is its own.
    pub input: Option<(InputKind, TextInput)>,
    /// A group with items in it that `⌘⌫` asked to delete: Enter (or `⌘⌫`
    /// again) does, any other key keeps it.
    pub confirm_delete: Option<u64>,
    /// What Linear said about the linked issues, by identifier.
    pub linked: HashMap<String, LinkedIssue>,
    /// The `⌘L` menu or the team pick, while one is up.
    pub pick: Option<Pick>,
    /// Each drawn chip that a click acts on, with its item.
    pub chip_hits: Vec<(Rect, u64, Chip)>,
    pub area: Rect,
    pub list_area: Rect,
    /// The first row drawn, as of the last draw (`ui::stacked_rows`).
    pub list_start: usize,
    /// Each drawn row's rect, by index into the rows: what a click
    /// hit-tests.
    pub row_rects: Vec<(usize, Rect)>,
    /// The tab strip's labels' screen x-ranges and its row, for the click.
    pub tab_hits: Vec<(u16, u16)>,
    pub tab_row: Rect,
}

impl TodoView {
    pub fn new(project: ProjectId, project_name: String, dir: PathBuf) -> Self {
        Self {
            project,
            project_name,
            dir,
            tab: TodoTab::Today,
            selected: 0,
            cursor: None,
            query: TextInput::new(),
            filtering: false,
            input: None,
            confirm_delete: None,
            linked: HashMap::new(),
            pick: None,
            chip_hits: Vec::new(),
            area: Rect::default(),
            list_area: Rect::default(),
            list_start: 0,
            row_rects: Vec::new(),
            tab_hits: Vec::new(),
            tab_row: Rect::default(),
        }
    }
}

/// The TODOS MODAL's own keys: one table [`handle_key`] matches and
/// [`hints`] spells. Each verb is the one every modal gives it — `⌘N`
/// new, `⌘F` filter — with its `^` twin; a row is edited by typing on it,
/// so there is no rename key.
pub(crate) mod keys {
    use crate::hints::Key;

    /// Tick or untick the item; fold a header.
    pub const DONE: Key = Key::new(&["space"], "done");
    /// The group at the cursor folded away or opened.
    pub const FOLD: Key = Key::new(&["left", "right"], "fold").show(2);
    /// The previous or next group's header, nested ones included. `⌥` is
    /// the twin where no ⌘ arrives: `^↑`/`^↓` are macOS's Mission Control.
    pub const JUMP: Key = Key::new(&["cmd+up", "cmd+down", "alt+up", "alt+down"], "groups").show(2);
    pub const NEW: Key = Key::new(&["cmd+n", "ctrl+n"], "new item");
    /// `^⇧N` arrives only where the KITTY PROTOCOL does: `^N` and `^⇧N`
    /// are one byte in a legacy terminal.
    pub const NEW_GROUP: Key = Key::new(&["cmd+shift+n", "ctrl+shift+n"], "new group");
    /// The row under the cursor opened where it stands, its last
    /// character gone; any other character typed on it is added to its
    /// end.
    pub const EDIT: Key = Key::new(&["backspace"], "edit");
    /// Linear's priorities, urgent to low — the item's own again takes
    /// it off. No `^` twins: a legacy terminal sends `^3` as Esc and `^4`
    /// as `^\`.
    pub const URGENT: Key = Key::new(&["cmd+1"], "urgent");
    pub const HIGH: Key = Key::new(&["cmd+2"], "high");
    pub const MEDIUM: Key = Key::new(&["cmd+3"], "medium");
    pub const LOW: Key = Key::new(&["cmd+4"], "low");
    /// The grid's delete-worktree key, on a row: the item, or the group
    /// and everything in it. `^W` where no ⌘ arrives.
    pub const DELETE: Key = Key::new(&["cmd+backspace", "ctrl+w"], "delete");
    /// The filter row, as every list modal opens its own.
    pub const FILTER: Key = crate::list_filter::keys::FILTER;
    /// Today ⇄ Log, as the PULL REQUESTS MODAL's page tabs.
    pub const TABS: Key = crate::pr_preview::keys::MODAL_TABS;
    /// The field's: the new item, group or edit in.
    pub const SAVE: Key = Key::new(&["enter"], "save");
    /// The field's other way out: saved, and the cursor a row on.
    pub const SAVE_MOVE: Key = Key::new(&["up", "down"], "save & move").show(2);
    pub const CONFIRM: Key = Key::new(&["enter", "cmd+backspace", "ctrl+w"], "delete");
    /// The priority keys, each with Linear's level for it.
    pub const PRIORITIES: [(Key, u8); 4] = [(URGENT, 1), (HIGH, 2), (MEDIUM, 3), (LOW, 4)];
    /// The LINEAR VIEW's `Enter` and `⇧Tab`: an agent on the item, or
    /// one of the AGENT PRESETS — and on an item whose session is still
    /// there, that session.
    pub const AGENT: Key = Key::new(&["enter"], "agent");
    pub const PRESET: Key = Key::new(&["shift+tab"], "preset");
    /// The PULL REQUESTS MODAL's way to Linear: the item's Linear menu.
    pub const LINEAR: Key = Key::new(&["cmd+l", "ctrl+l"], "Linear");
    /// Ask Linear again how the linked issues stand.
    pub const REFRESH: Key = crate::issues::keys::REFRESH;
    /// The `⌘L` menu's and the team pick's.
    pub const PICK: Key = Key::new(&["up", "down"], "pick").show(2);
    pub const CHOOSE: Key = Key::new(&["enter"], "choose");
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        DONE, FOLD, JUMP, NEW, NEW_GROUP, EDIT, URGENT, HIGH, MEDIUM, LOW, DELETE, FILTER, TABS,
        SAVE, SAVE_MOVE, CONFIRM, AGENT, PRESET, LINEAR, REFRESH, PICK, CHOOSE,
    ];
}

/// The priority keys as one hint: `⌘1-4 priority` — none where the
/// terminal sends no ⌘, as then they cannot be pressed.
fn priority_hint() -> Option<crate::hints::Hint> {
    crate::keymap::cmd_shown()
        .then(|| crate::hints::Hint::new(format!("{}-4", keys::URGENT.label()), "priority"))
}

/// The keys along the modal's bottom edge. Esc puts the filter away
/// first.
pub(crate) fn hints(view: &TodoView) -> Vec<crate::hints::Hint> {
    use crate::hints::Hint;
    if view.input.is_some() {
        return vec![
            keys::SAVE.hint().kept(),
            keys::SAVE_MOVE.hint(),
            Hint::new("Esc", "revert"),
        ];
    }
    if view.confirm_delete.is_some() {
        return vec![keys::CONFIRM.hint().kept(), Hint::new("Esc", "keep")];
    }
    if view.pick.is_some() {
        return vec![
            keys::CHOOSE.hint().kept(),
            keys::PICK.hint(),
            Hint::new("Esc", "cancel"),
        ];
    }
    if view.filtering {
        return vec![
            keys::AGENT.hint().kept(),
            keys::PICK.hint_as("move"),
            keys::DELETE.hint(),
            Hint::new("Esc", "clear"),
        ];
    }
    match view.tab {
        TodoTab::Today => {
            let mut hints = vec![
                keys::DONE.hint().kept(),
                keys::AGENT.hint().kept(),
                keys::NEW.hint(),
                keys::EDIT.hint(),
                keys::DELETE.hint(),
                keys::FILTER.hint(),
                keys::JUMP.hint(),
                keys::LINEAR.hint(),
            ];
            hints.extend(priority_hint());
            hints.extend([
                keys::FOLD.hint(),
                keys::NEW_GROUP.hint(),
                keys::PRESET.hint(),
                keys::REFRESH.hint(),
                keys::TABS.hint(),
                Hint::new("Esc", "close"),
            ]);
            hints
        }
        TodoTab::Log => vec![
            keys::DONE.hint_as("not done").kept(),
            keys::EDIT.hint(),
            keys::FILTER.hint(),
            keys::TABS.hint(),
            Hint::new("Esc", "close"),
        ],
    }
}

/// `⌘I` on the grid: the selected project's list.
pub(crate) fn open(app: &mut App) {
    let Some(project) = app.selected_project().cloned() else {
        return;
    };
    open_on(app, project.id, project.name, project.repo_path);
}

fn open_on(app: &mut App, project: ProjectId, name: String, dir: PathBuf) {
    if !app.todos.contains_key(&dir) {
        let loaded = store::load(&dir);
        if let Some(problem) = loaded.problem {
            app.flash = Some(crate::flash::Flash::failed(problem));
        }
        app.todos.insert(dir.clone(), loaded.file);
    }
    let mut view = TodoView::new(project, name, dir);
    view.linked = linked_from_linear(app, &view);
    app.overlay = Some(Overlay::Todos(view));
    app.dirty = true;
    refresh_linked(app);
}

/// Ask Linear how every linked issue stands now (`⌘R`, and on open).
fn refresh_linked(app: &mut App) {
    let Some(view) = view(app) else {
        return;
    };
    let dir = view.dir.clone();
    let ids: Vec<String> = app.todos.get(&dir).map_or_else(Vec::new, |file| {
        let mut ids: Vec<String> = file.items.iter().filter_map(|i| i.linear.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    });
    crate::linear::request_linked(app, dir, ids);
}

/// Put the modal back up as it was left.
pub(crate) fn reopen(app: &mut App, view: TodoView) {
    app.overlay = Some(Overlay::Todos(view));
    app.dirty = true;
}

/// The linked issues the LINEAR VIEW already has, so their chips draw
/// at once.
fn linked_from_linear(app: &App, view: &TodoView) -> HashMap<String, LinkedIssue> {
    let (Some(file), Some(list)) = (app.todos.get(&view.dir), app.linear.get(&view.project)) else {
        return HashMap::new();
    };
    file.items
        .iter()
        .filter_map(|i| i.linear.as_deref())
        .filter_map(|id| list.list.iter().find(|issue| issue.identifier == id))
        .map(|issue| (issue.identifier.clone(), LinkedIssue::of(issue)))
        .collect()
}

fn view(app: &App) -> Option<&TodoView> {
    match app.overlay.as_ref()? {
        Overlay::Todos(view) => Some(view),
        _ => None,
    }
}

fn view_mut(app: &mut App) -> Option<&mut TodoView> {
    match app.overlay.as_mut()? {
        Overlay::Todos(view) => Some(view),
        _ => None,
    }
}

/// The open modal's list.
fn list(app: &App) -> Option<&TodoFile> {
    app.todos.get(&view(app)?.dir)
}

/// Change the modal's list and save it.
fn edit<T>(app: &mut App, f: impl FnOnce(&mut TodoFile) -> T) -> Option<T> {
    let dir = view(app)?.dir.clone();
    let file = app.todos.get_mut(&dir)?;
    let out = f(file);
    store::save(file);
    app.dirty = true;
    Some(out)
}

/// The rows of `tab`, as `file`, the filter and the open field lay them
/// out on `today`.
pub(crate) fn entries(file: &TodoFile, view: &TodoView, today: NaiveDate) -> Vec<Entry> {
    let mut out = Vec::new();
    let query = view.query.trim();
    match view.tab {
        TodoTab::Today => {
            let walk = Walk {
                file,
                query,
                today,
                input: view.input.as_ref().map(|(kind, _)| *kind),
            };
            walk.groups(None, 0, false, &mut out);
            if matches!(walk.input, Some(InputKind::Item { group: None })) {
                out.push(Entry::Input { depth: 1 });
            }
            out.push(Entry::AddRow);
        }
        TodoTab::Log => {
            for (date, items) in file.log_days(today) {
                let items: Vec<&Item> = items
                    .into_iter()
                    .filter(|i| query.is_empty() || matches(query, &i.text))
                    .collect();
                if items.is_empty() {
                    continue;
                }
                out.push(Entry::Day {
                    date,
                    count: items.len(),
                });
                let rename = view.input.as_ref().map(|(kind, _)| *kind);
                for item in items {
                    out.push(match rename {
                        Some(InputKind::Rename(Target::Item(id))) if id == item.id => {
                            Entry::Input { depth: 0 }
                        }
                        _ => Entry::Item {
                            id: item.id,
                            depth: 0,
                        },
                    });
                }
            }
        }
    }
    out
}

/// Whether the filter's `query` finds `text`.
fn matches(query: &str, text: &str) -> bool {
    crate::fuzzy::fuzzy_match(query, text).is_some()
}

/// The Today tab's walk down the groups.
struct Walk<'a> {
    file: &'a TodoFile,
    query: &'a str,
    today: NaiveDate,
    input: Option<InputKind>,
}

impl Walk<'_> {
    /// The groups under `parent`, each header over its items and then its
    /// own groups — all of them when `all` (a filter matched a group they
    /// are in), else only what the filter finds, folded groups left
    /// folded while nothing is typed.
    fn groups(&self, parent: Option<u64>, depth: u16, all: bool, out: &mut Vec<Entry>) {
        if depth as usize > super::MAX_DEPTH {
            return;
        }
        let filtering = !self.query.is_empty();
        for group in self.file.subgroups(parent) {
            let all = all || (filtering && matches(self.query, &group.name));
            let start = out.len();
            out.push(match self.input {
                Some(InputKind::Rename(Target::Group(id))) if id == group.id => {
                    Entry::Input { depth }
                }
                _ => Entry::Header {
                    group: group.id,
                    depth,
                },
            });
            if group.collapsed && !filtering {
                continue;
            }
            let mut found = false;
            for item in self.file.today_items(group.id, self.today) {
                if filtering && !all && !matches(self.query, &item.text) {
                    continue;
                }
                found = true;
                out.push(match self.input {
                    Some(InputKind::Rename(Target::Item(id))) if id == item.id => {
                        Entry::Input { depth: depth + 1 }
                    }
                    _ => Entry::Item {
                        id: item.id,
                        depth: depth + 1,
                    },
                });
            }
            if self.input
                == Some(InputKind::Item {
                    group: Some(group.id),
                })
            {
                found = true;
                out.push(Entry::Input { depth: depth + 1 });
            }
            let before = out.len();
            self.groups(Some(group.id), depth + 1, all, out);
            found |= out.len() > before;
            // Filtering, a group with nothing found in it goes.
            if filtering && !all && !found {
                out.truncate(start);
            }
        }
        if self.input == Some(InputKind::Group { parent }) {
            out.push(Entry::Input { depth });
        }
    }
}

/// The cursor's row in `entries`: the open field's, else the row it was
/// on, else the nearest row to where it was that can hold it.
fn resolve(view: &TodoView, entries: &[Entry]) -> Option<usize> {
    if view.input.is_some() {
        if let Some(i) = entries
            .iter()
            .position(|e| matches!(e, Entry::Input { .. }))
        {
            return Some(i);
        }
    }
    if let Some(i) = view
        .cursor
        .and_then(|c| entries.iter().position(|e| *e == c))
    {
        return Some(i);
    }
    let at = view.selected.min(entries.len().checked_sub(1)?);
    (at..entries.len())
        .chain((0..at).rev())
        .find(|i| entries[*i].selectable())
}

/// The rows as they stand now, and the cursor's among them.
fn rows_now(app: &App) -> Option<(Vec<Entry>, Option<usize>)> {
    let view = view(app)?;
    let file = app.todos.get(&view.dir)?;
    let entries = entries(file, view, super::today());
    let at = resolve(view, &entries);
    Some((entries, at))
}

/// The row under the cursor.
fn current(app: &App) -> Option<Entry> {
    let (entries, at) = rows_now(app)?;
    entries.get(at?).copied()
}

/// What the cursor's row acts on.
fn target(app: &App) -> Option<Target> {
    match current(app)? {
        Entry::Header { group, .. } => Some(Target::Group(group)),
        Entry::Item { id, .. } => Some(Target::Item(id)),
        _ => None,
    }
}

/// The group the cursor is in: a header's own, an item's, or — on `+ new
/// item` — the last group drawn above it.
fn cursor_group(app: &App) -> Option<u64> {
    let (entries, at) = rows_now(app)?;
    let file = list(app)?;
    entries[..=at?].iter().rev().find_map(|e| match e {
        Entry::Header { group, .. } => Some(*group),
        Entry::Item { id, .. } => file.item(*id).map(|i| i.group),
        _ => None,
    })
}

fn put_cursor(app: &mut App, index: usize, entry: Option<Entry>) {
    if let Some(view) = view_mut(app) {
        view.selected = index;
        view.cursor = entry;
    }
}

/// Move the cursor `delta` rows, over the day headers.
fn step(app: &mut App, delta: i32) {
    let Some((entries, Some(at))) = rows_now(app) else {
        return;
    };
    let selectable: Vec<usize> = (0..entries.len())
        .filter(|i| entries[*i].selectable())
        .collect();
    let here = selectable.iter().position(|i| *i == at).unwrap_or(0);
    let next = (here as i32 + delta).clamp(0, selectable.len() as i32 - 1) as usize;
    let index = selectable[next];
    put_cursor(app, index, Some(entries[index]));
}

pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(view) = view_mut(app) else {
        return false;
    };
    // The menu, the team pick and the delete question take no text.
    if view.pick.is_some() || view.confirm_delete.is_some() {
        return true;
    }
    if let Some((_, input)) = &mut view.input {
        input.insert_str(&text.replace(['\r', '\n'], " "));
        return true;
    }
    if text.trim().contains('\n') {
        let nodes = super::import::parse(text);
        if !nodes.is_empty() {
            view.tab = TodoTab::Today;
            let count = edit(app, |file| file.import(&nodes, super::today())).unwrap_or(0);
            app.flash = Some(crate::flash::Flash::done(format!(
                "added {} from the pasted list",
                crate::bundle::plural(count, "item")
            )));
            return true;
        }
    }
    let line = text.replace(['\r', '\n'], " ");
    let Some(view) = view_mut(app) else {
        return false;
    };
    if view.filtering {
        view.query.insert_str(&line);
        query_changed(app);
    } else {
        // As typing would: onto the end of the row — or, with no row to
        // go on, into the filter.
        start_edit(app, Some(&line));
        if let Some(view) = view_mut(app).filter(|v| v.input.is_none()) {
            view.filtering = true;
            view.query.insert_str(&line);
            query_changed(app);
        }
    }
    true
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    app.dirty = true;
    if view(app).is_some_and(|v| v.input.is_some()) {
        input_key(app, key);
        return;
    }
    if view(app).is_some_and(|v| v.pick.is_some()) {
        pick_key(app, key, out);
        return;
    }
    if let Some(group) = view(app).and_then(|v| v.confirm_delete) {
        if let Some(view) = view_mut(app) {
            view.confirm_delete = None;
        }
        if keys::CONFIRM.matches(&key) {
            edit(app, |file| file.delete_group(group));
            if let Some(view) = view_mut(app) {
                view.cursor = None;
            }
        }
        return;
    }
    let Some(view) = view(app) else {
        return;
    };
    let today = view.tab == TodoTab::Today;
    let filtering = view.filtering;
    let page = view.list_area.height.max(1) as i32;
    let priority = keys::PRIORITIES
        .iter()
        .find(|(k, _)| k.matches(&key))
        .map(|(_, level)| *level);
    match key.code {
        KeyCode::Esc if filtering => close_filter(app),
        KeyCode::Esc => app.overlay = None,
        // ⇧←/⇧→ are the tabs', plain ←/→ the fold's.
        _ if keys::TABS.matches(&key) => {
            let other = view.tab.other();
            switch_tab(app, other);
        }
        _ if keys::JUMP.matches(&key) => jump(app, key.code == KeyCode::Down),
        KeyCode::Down => step(app, 1),
        KeyCode::Up => step(app, -1),
        KeyCode::PageDown => step(app, page),
        KeyCode::PageUp => step(app, -page),
        KeyCode::Home => step(app, i32::MIN / 2),
        KeyCode::End => step(app, i32::MAX / 2),
        KeyCode::Left if today => fold(app, true),
        KeyCode::Right if today => fold(app, false),
        KeyCode::Enter => match current(app) {
            Some(Entry::AddRow) => start_item(app),
            Some(Entry::Item { id, .. }) => agent(app, id, out),
            Some(Entry::Header { .. }) => done(app),
            _ => {}
        },
        _ if keys::PRESET.matches(&key) => preset(app),
        _ if keys::LINEAR.matches(&key) => open_menu(app),
        _ if keys::REFRESH.matches(&key) => refresh_linked(app),
        _ if keys::DELETE.matches(&key) => delete(app),
        _ if keys::FILTER.matches(&key) => {
            if let Some(view) = view_mut(app) {
                view.filtering = true;
            }
        }
        _ if keys::NEW.matches(&key) && today => start_item(app),
        _ if keys::NEW_GROUP.matches(&key) && today => start_group(app),
        _ if priority.is_some() => set_priority(app, priority.unwrap_or_default()),
        // With the filter up, what is typed is the filter's — a space
        // too.
        _ if filtering => {
            let changed = view_mut(app).is_some_and(|v| v.query.handle_key(&key).changed());
            if changed {
                query_changed(app);
            }
        }
        _ if keys::DONE.matches(&key) => done(app),
        _ if keys::EDIT.matches(&key) => start_edit(app, None),
        KeyCode::Char(c) if crate::key_combo::is_text_key(&KeyChord::from_event(&key)) => {
            start_edit(app, Some(&c.to_string()))
        }
        _ => {}
    }
}

/// `Esc` with the filter up: what was typed goes, and the filter with it.
fn close_filter(app: &mut App) {
    if let Some(view) = view_mut(app) {
        view.query.clear();
        view.filtering = false;
    }
    query_changed(app);
}

/// Show `tab`, the cursor on its first row.
fn switch_tab(app: &mut App, tab: TodoTab) {
    if let Some(view) = view_mut(app) {
        if view.tab != tab {
            view.tab = tab;
            view.selected = 0;
            view.cursor = None;
            view.list_start = 0;
        }
    }
}

/// A key while the field is open: Enter puts it in — a new item opens the
/// next one in the same place, for a run of them — `↑`/`↓` put it in and
/// move on, Esc lets it go.
fn input_key(app: &mut App, key: KeyEvent) {
    let Some(view) = view_mut(app) else {
        return;
    };
    let Some((kind, input)) = &mut view.input else {
        return;
    };
    let kind = *kind;
    match key.code {
        KeyCode::Esc => {
            view.input = None;
            view.cursor = None;
        }
        _ if keys::SAVE.matches(&key) => save_field(app, kind),
        // Saved as Enter would, then a row on — the next field a new item
        // would open stays shut.
        _ if keys::SAVE_MOVE.matches(&key) => {
            let next = beside_field(app, key.code == KeyCode::Up);
            save_field(app, kind);
            if let Some(view) = view_mut(app) {
                view.input = None;
            }
            match next {
                Some(next) => land_on(app, |e| *e == next),
                None => step(app, if key.code == KeyCode::Up { -1 } else { 1 }),
            }
        }
        _ => {
            input.handle_key(&key);
        }
    }
}

/// The row over (`up`) or under the open field, while the field still
/// stands in the rows: where `↑`/`↓` take the cursor once it is put in.
fn beside_field(app: &App, up: bool) -> Option<Entry> {
    let (entries, _) = rows_now(app)?;
    let at = entries
        .iter()
        .position(|e| matches!(e, Entry::Input { .. }))?;
    let found = if up {
        (0..at).rev().find(|i| entries[*i].selectable())
    } else {
        (at + 1..entries.len()).find(|i| entries[*i].selectable())
    };
    found.map(|i| entries[i])
}

/// The field shut and what was typed in it put in — nothing, when it was
/// left empty.
fn save_field(app: &mut App, kind: InputKind) {
    let Some((_, input)) = view_mut(app).and_then(|v| v.input.take()) else {
        return;
    };
    let text = input.trim().to_string();
    if !text.is_empty() {
        commit(app, kind, &text);
    }
}

/// Put the field's `text` in.
fn commit(app: &mut App, kind: InputKind, text: &str) {
    let today = super::today();
    match kind {
        InputKind::Item { group } => {
            let made = edit(app, |file| {
                let group = group
                    .filter(|g| file.group(*g).is_some())
                    .unwrap_or_else(|| file.inbox());
                file.add_item(group, text, today);
                group
            });
            if let (Some(group), Some(view)) = (made, view_mut(app)) {
                view.input = Some((InputKind::Item { group: Some(group) }, TextInput::new()));
            }
        }
        InputKind::Group { parent } => {
            let id = edit(app, |file| file.add_group(parent, text));
            land_on_group(app, id);
        }
        InputKind::Rename(Target::Group(id)) => {
            edit(app, |file| {
                if let Some(group) = file.group_mut(id) {
                    group.name = text.to_string();
                }
            });
            land_on_group(app, Some(id));
        }
        InputKind::Rename(Target::Item(id)) => {
            edit(app, |file| {
                if let Some(item) = file.item_mut(id) {
                    item.text = text.to_string();
                }
            });
            land_on(app, |e| matches!(e, Entry::Item { id: x, .. } if *x == id));
        }
    }
}

/// The cursor onto the first row `hit` picks, wherever it is drawn.
fn land_on(app: &mut App, hit: impl Fn(&Entry) -> bool) {
    let Some((entries, _)) = rows_now(app) else {
        return;
    };
    if let Some(i) = entries.iter().position(hit) {
        put_cursor(app, i, Some(entries[i]));
    }
}

/// The cursor onto `group`'s header.
fn land_on_group(app: &mut App, group: Option<u64>) {
    land_on(
        app,
        |e| matches!(e, Entry::Header { group: g, .. } if Some(*g) == group),
    );
}

fn open_input(app: &mut App, kind: InputKind, text: &str) {
    if let Some(view) = view_mut(app) {
        view.tab = match kind {
            InputKind::Rename(_) => view.tab,
            _ => TodoTab::Today,
        };
        view.input = Some((kind, TextInput::with_text(text)));
    }
}

/// `⌘N`: a new item in the cursor's group.
fn start_item(app: &mut App) {
    let group = cursor_group(app);
    open_input(app, InputKind::Item { group }, "");
}

/// `⌘⇧N`: a new group beside the cursor's.
fn start_group(app: &mut App) {
    let parent = cursor_group(app).and_then(|g| {
        let file = app.todos.get(&view(app)?.dir)?;
        file.group(g)?.parent
    });
    open_input(app, InputKind::Group { parent }, "");
}

/// Typing on a row: the row opened where it stands, `append` added to its
/// end — or, for `⌫` (`None`), its last character gone. On `+ new item`
/// it is a new item, in the group above.
fn start_edit(app: &mut App, append: Option<&str>) {
    let Some(entry) = current(app) else {
        return;
    };
    let Some(file) = list(app) else {
        return;
    };
    let (kind, text) = match entry {
        Entry::Header { group, .. } => match file.group(group) {
            Some(g) => (InputKind::Rename(Target::Group(group)), g.name.clone()),
            None => return,
        },
        Entry::Item { id, .. } => match file.item(id) {
            Some(i) => (InputKind::Rename(Target::Item(id)), i.text.clone()),
            None => return,
        },
        Entry::AddRow => (
            InputKind::Item {
                group: cursor_group(app),
            },
            String::new(),
        ),
        Entry::Input { .. } | Entry::Day { .. } => return,
    };
    open_input(app, kind, &text);
    let Some((_, input)) = view_mut(app).and_then(|v| v.input.as_mut()) else {
        return;
    };
    match append {
        Some(text) => input.insert_str(text),
        None => {
            input.handle_key(&KeyEvent::from(KeyCode::Backspace));
        }
    }
}

/// `⌘↓`/`⌘↑`: the cursor onto the next (or previous) group's header,
/// nested groups' included — on the Log, the first item of the next (or
/// previous) day.
fn jump(app: &mut App, down: bool) {
    let Some((entries, Some(at))) = rows_now(app) else {
        return;
    };
    let header = |i: &usize| match entries[*i] {
        Entry::Header { .. } => true,
        Entry::Item { .. } => i
            .checked_sub(1)
            .is_some_and(|p| matches!(entries[p], Entry::Day { .. })),
        _ => false,
    };
    let found = if down {
        (at + 1..entries.len()).find(header)
    } else {
        (0..at).rev().find(header)
    };
    if let Some(i) = found {
        put_cursor(app, i, Some(entries[i]));
    }
}

/// `space`: tick or untick the item — the cursor staying on its row, so
/// the next one comes under it — or fold the header.
fn done(app: &mut App) {
    match target(app) {
        Some(Target::Item(id)) => {
            edit(app, |file| file.toggle(id, super::now()));
            if let Some(view) = view_mut(app) {
                view.cursor = None;
            }
        }
        Some(Target::Group(_)) => {
            let folded = list(app)
                .zip(cursor_group(app))
                .and_then(|(file, g)| file.group(g))
                .is_some_and(|g| g.collapsed);
            fold(app, !folded);
        }
        None => {}
    }
}

/// `←` folds the group at the cursor — an item's own, the cursor going
/// up to its header — and on a folded header goes up to the group it is
/// in; `→` opens it.
fn fold(app: &mut App, collapse: bool) {
    let Some(entry) = current(app) else {
        return;
    };
    let Some(file) = list(app) else {
        return;
    };
    let (group, on_header) = match entry {
        Entry::Header { group, .. } => (group, true),
        Entry::Item { id, .. } => match file.item(id) {
            Some(item) => (item.group, false),
            None => return,
        },
        _ => return,
    };
    let Some(state) = file.group(group) else {
        return;
    };
    if collapse && on_header && state.collapsed {
        let parent = state.parent;
        land_on_group(app, parent);
        return;
    }
    if state.collapsed != collapse {
        edit(app, |file| {
            if let Some(g) = file.group_mut(group) {
                g.collapsed = collapse;
            }
        });
    }
    if !on_header {
        land_on_group(app, Some(group));
    }
}

/// `⌘1`–`⌘4`: the item's priority — or, pressed on the one it has, no
/// priority. It re-sorts, the cursor going with it.
fn set_priority(app: &mut App, level: u8) {
    if let Some(Target::Item(id)) = target(app) {
        edit(app, |file| {
            if let Some(item) = file.item_mut(id) {
                item.priority = if item.priority == level { 0 } else { level };
            }
        });
    }
}

/// `⌘⌫`: the item goes; a group goes at once when it is empty, else
/// after a second press says so.
fn delete(app: &mut App) {
    match target(app) {
        Some(Target::Item(id)) => {
            edit(app, |file| file.delete_item(id));
        }
        Some(Target::Group(id)) => {
            let size = list(app).map_or(0, |f| f.size(id));
            if size > 0 {
                if let Some(view) = view_mut(app) {
                    view.confirm_delete = Some(id);
                }
                return;
            }
            edit(app, |file| file.delete_group(id));
        }
        None => return,
    }
    if let Some(view) = view_mut(app) {
        view.cursor = None;
    }
}

fn query_changed(app: &mut App) {
    if let Some(view) = view_mut(app) {
        view.list_start = 0;
    }
    // The cursor onto the first match when its row went.
    let Some((entries, _)) = rows_now(app) else {
        return;
    };
    let Some(view) = view(app) else {
        return;
    };
    let kept = view.cursor.is_some_and(|c| entries.contains(&c));
    if !kept {
        let first = entries
            .iter()
            .position(|e| matches!(e, Entry::Item { .. }))
            .or_else(|| entries.iter().position(Entry::selectable));
        if let Some(i) = first {
            put_cursor(app, i, Some(entries[i]));
        }
    }
}

// ---- the agent ----

/// `Enter` on an item: the session sent at it, while there is one — the
/// modal goes and the panels land on it — else the QUICK PROMPT over the
/// modal, the item's text and where it sits in the box, in a fresh
/// worktree.
fn agent(app: &mut App, item: u64, out: &mut Vec<ClientRequest>) {
    let live = list(app)
        .and_then(|file| file.item(item))
        .and_then(|i| live_agent(app, i))
        .map(|a| a.id.clone());
    if let Some(id) = live {
        crate::event_loop::jump_to_session(app, id, out);
        return;
    }
    let Some((launch, task)) = launch_for(app, item) else {
        return;
    };
    let launch = launch.with_under(ModalUnder::of(app.overlay.as_ref()));
    // A draft parked from a box for this very todo comes back as it was
    // left; one for anything else stays parked, and the box opens on the
    // item's task — never on words meant for another.
    let parked = app.quick_draft.take(&launch.target);
    let resumed = parked
        .as_ref()
        .is_some_and(|d| d.launch.todo_item() == launch.todo_item());
    if resumed {
        app.quick_draft
            .park(parked.expect("resumed is a parked draft"));
        crate::quick_prompt::open_box(app, launch);
        return;
    }
    crate::quick_prompt::open_picked_box(app, launch);
    if let Some(draft) = parked {
        app.quick_draft.park(draft);
    }
    if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
        prompt.input = TextInput::multiline_with_text(task);
        prompt.draft_restored = false;
    }
}

/// `⇧Tab` on an item: an AGENT PRESET for it, the box then opening with
/// the item's text, as the LINEAR VIEW's does.
fn preset(app: &mut App) {
    let Some(Target::Item(item)) = target(app) else {
        return;
    };
    let Some((launch, task)) = launch_for(app, item) else {
        return;
    };
    let under = ModalUnder::of(app.overlay.as_ref());
    let mut back = QuickReturn::fresh(launch.with_under(under));
    back.text = task;
    crate::quick_prompt::open_preset_picker(app, back);
}

/// The launch for an agent at `item`, and the task the box opens with:
/// its text, the group it is in, and its Linear issue when it has one —
/// the issue's URL the session's context too.
fn launch_for(app: &App, item: u64) -> Option<(QuickLaunch, String)> {
    let view = view(app)?;
    let file = app.todos.get(&view.dir)?;
    let todo = file.item(item)?;
    let issue = todo
        .linear
        .as_ref()
        .and_then(|id| view.linked.get(id))
        .filter(|i| !i.url.is_empty());
    let mut task = format!(
        "{}\n\nContext: todo in {}",
        todo.text,
        file.path_label(todo.group)
    );
    if let Some(issue) = issue {
        task.push_str(&format!(
            "\nLinear issue: {} · {}",
            issue.identifier, issue.url
        ));
    }
    let project = view.project.clone();
    let target = QuickTarget::NewWorktree {
        branch: crate::branch_name::random_name(&app.project_branches(&project)),
        project,
        existing: false,
    };
    let todo = TodoRef {
        repo_path: view.dir.clone(),
        item,
        issue_url: issue.map(|i| i.url.clone()),
    };
    let launch =
        QuickLaunch::from_config(target, &crate::config::Config::load()).with_todo(Some(todo));
    Some((launch, task))
}

// ---- Linear ----

/// `⌘L` on an item: its Linear menu — **Create in Triage** and **Link
/// existing…** for one not linked, **Open in browser** and **Unlink** for
/// one that is.
fn open_menu(app: &mut App) {
    let Some(Target::Item(item)) = target(app) else {
        return;
    };
    let Some(view) = view(app) else {
        return;
    };
    let linked = list(app)
        .and_then(|f| f.item(item))
        .is_some_and(|i| i.linear.is_some());
    // While its issue is being made, there is no making it again.
    let creating = app.todo_creates.contains(&(view.dir.clone(), item));
    let actions = match (linked, creating) {
        (true, _) => vec![MenuAction::Browser, MenuAction::Unlink],
        (false, true) => vec![MenuAction::Link],
        (false, false) => vec![MenuAction::Create, MenuAction::Link],
    };
    if let Some(view) = view_mut(app) {
        view.pick = Some(Pick::new(item, PickKind::Menu(actions)));
    }
}

/// A key while the menu or the team pick is up.
fn pick_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(view) = view_mut(app) else {
        return;
    };
    let Some(pick) = &mut view.pick else {
        return;
    };
    let len = pick.len();
    match key.code {
        KeyCode::Esc => view.pick = None,
        KeyCode::Down => pick.selected = crate::app::clamp_selection(pick.selected as i64 + 1, len),
        KeyCode::Up => pick.selected = crate::app::clamp_selection(pick.selected as i64 - 1, len),
        _ if keys::CHOOSE.matches(&key) => choose(app, out),
        _ => {}
    }
}

/// Enter in the menu or the team pick: what its row says.
fn choose(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(pick) = view_mut(app).and_then(|v| v.pick.take()) else {
        return;
    };
    let item = pick.item;
    match pick.kind {
        PickKind::Menu(actions) => match actions.get(pick.selected) {
            Some(MenuAction::Create) => create(app, item, None),
            Some(MenuAction::Link) => {
                let Some(Overlay::Todos(view)) = app.overlay.take() else {
                    return;
                };
                crate::linear::open_link(app, item, view);
            }
            Some(MenuAction::Browser) => open_issue(app, item, out),
            Some(MenuAction::Unlink) => {
                let gone = edit(app, |file| {
                    let todo = file.item_mut(item)?;
                    todo.linear_seen = None;
                    todo.linear.take()
                })
                .flatten();
                if let (Some(id), Some(view)) = (gone, view_mut(app)) {
                    view.linked.remove(&id);
                }
            }
            None => {}
        },
        PickKind::Team(teams) => {
            if let Some(team) = teams.get(pick.selected).cloned() {
                // The pick is remembered: the next issue goes there too.
                edit(app, |file| file.linear_team = Some(team.id.clone()));
                create(app, item, Some(team));
            }
        }
    }
}

/// **Create in Triage**: the item filed in the team picked, else the one
/// the list remembers — or the only one.
fn create(app: &mut App, item: u64, team: Option<LinearTeam>) {
    let Some(view) = view(app) else {
        return;
    };
    let dir = view.dir.clone();
    let Some(file) = app.todos.get(&dir) else {
        return;
    };
    let Some(todo) = file.item(item) else {
        return;
    };
    let draft = IssueDraft {
        title: todo.text.clone(),
        priority: todo.priority,
        description: format!("From orion todos · {}", file.path_label(todo.group)),
    };
    let choice = match team {
        Some(team) => TeamChoice::Picked(team),
        None => TeamChoice::Remembered(file.linear_team.clone()),
    };
    crate::linear::create_for_todo(app, dir, item, draft, choice);
}

/// The item's issue in the browser — once Linear has said where it is.
fn open_issue(app: &mut App, item: u64, out: &mut Vec<ClientRequest>) {
    let Some(view) = view(app) else {
        return;
    };
    let Some(id) = app
        .todos
        .get(&view.dir)
        .and_then(|f| f.item(item))
        .and_then(|i| i.linear.clone())
    else {
        return;
    };
    match view
        .linked
        .get(&id)
        .map(|i| i.url.clone())
        .filter(|u| !u.is_empty())
    {
        Some(url) => crate::event_loop::open_link(app, &url, out),
        None => {
            app.flash = Some(crate::flash::Flash::note(format!(
                "Linear hasn't said where {id} is yet — {} asks again",
                keys::REFRESH.label()
            )))
        }
    }
}

/// The modal on `dir`'s list wherever it is kept: up, standing under the
/// QUICK PROMPT an `Enter` opened over it, or waiting for the LINEAR VIEW
/// that **Link existing…** opened to come back to it. An answer landing
/// meanwhile is not lost on the way back.
fn view_on<'a>(app: &'a mut App, dir: &std::path::Path) -> Option<&'a mut TodoView> {
    use crate::app::PromptKind;
    let view = match app.overlay.as_mut()? {
        Overlay::Todos(view) => view,
        Overlay::Linear(linear) => match &mut linear.mode {
            crate::linear::LinearMode::Link { back, .. } => back.as_mut(),
            _ => return None,
        },
        Overlay::Prompt(prompt) => match &mut prompt.kind {
            PromptKind::QuickPrompt(launch) => match &mut launch.under {
                Some(ModalUnder::Todos(view)) => view.as_mut(),
                _ => return None,
            },
            _ => return None,
        },
        _ => return None,
    };
    (view.dir == dir).then_some(view)
}

/// Linear's answer on the linked issues: their chips say how they stand,
/// and an issue gone to done or canceled since it was last seen ticks its
/// todo, if it is still open.
pub(crate) fn land_linked(app: &mut App, dir: PathBuf, result: Result<Vec<LinkedIssue>, String>) {
    app.dirty = true;
    let issues = match result {
        Ok(issues) => issues,
        Err(err) => {
            app.flash = Some(crate::flash::Flash::failed(format!("Linear: {err}")));
            return;
        }
    };
    let now = super::now();
    let mut ticked = Vec::new();
    if let Some(file) = app.todos.get_mut(&dir) {
        let mut changed = false;
        for item in file.items.iter_mut() {
            let Some(issue) = item
                .linear
                .as_ref()
                .and_then(|id| issues.iter().find(|i| &i.identifier == id))
            else {
                continue;
            };
            // Only the way into done ticks: an issue that was already
            // done when last seen leaves an unticked item alone.
            let was_finished = item
                .linear_seen
                .as_deref()
                .is_some_and(crate::linear::finished_kind);
            if issue.finished() && !was_finished && item.done.is_none() {
                item.done = Some(now);
                ticked.push(issue.identifier.clone());
            }
            if item.linear_seen.as_deref() != Some(issue.state_type.as_str()) {
                item.linear_seen = Some(issue.state_type.clone());
                changed = true;
            }
        }
        if changed {
            store::save(file);
        }
    }
    if !ticked.is_empty() {
        app.flash = Some(crate::flash::Flash::done(format!(
            "ticked {} — done in Linear",
            ticked.join(", ")
        )));
    }
    if let Some(view) = view_on(app, &dir) {
        for issue in issues {
            view.linked.insert(issue.identifier.clone(), issue);
        }
    }
}

/// Several teams and none remembered: which one **Create in Triage**
/// files `item` in is the user's to pick.
pub(crate) fn land_teams(
    app: &mut App,
    dir: PathBuf,
    item: u64,
    result: Result<Vec<LinearTeam>, String>,
) {
    app.dirty = true;
    app.todo_creates.remove(&(dir.clone(), item));
    match result {
        Err(err) => {
            app.flash = Some(crate::flash::Flash::failed(format!(
                "couldn't create the issue: {err}"
            )))
        }
        Ok(teams) if teams.is_empty() => {
            app.flash = Some(crate::flash::Flash::failed(
                "couldn't create the issue: Linear lists no team for this key",
            ))
        }
        Ok(teams) => match view_on(app, &dir) {
            Some(view) => {
                view.pick = Some(Pick::new(item, PickKind::Team(teams)));
                // The wait is over; the pick says what comes next.
                if app
                    .flash
                    .as_deref()
                    .is_some_and(|f| f == crate::linear::CREATING)
                {
                    app.flash = None;
                }
            }
            None => {
                let again = keys::LINEAR.label();
                let text = format!(
                    "Linear has several teams — pick one: open the todos and {again} Create in Triage again"
                );
                app.flash = Some(crate::flash::Flash::setup(text));
            }
        },
    }
}

/// The issue **Create in Triage** made: the item linked to it, the team
/// remembered, and the footer saying where it went.
pub(crate) fn land_created(
    app: &mut App,
    dir: PathBuf,
    item: u64,
    team: LinearTeam,
    result: Result<LinkedIssue, String>,
) {
    app.dirty = true;
    app.todo_creates.remove(&(dir.clone(), item));
    let issue = match result {
        Ok(issue) => issue,
        Err(err) => {
            app.flash = Some(crate::flash::Flash::failed(format!(
                "couldn't create the issue: {err}"
            )));
            return;
        }
    };
    if let Some(file) = app.todos.get_mut(&dir) {
        if let Some(todo) = file.item_mut(item) {
            link_to(todo, &issue);
        }
        file.linear_team = Some(team.id.clone());
        store::save(file);
    }
    app.flash = Some(if team.triage_state.is_some() {
        crate::flash::Flash::done(format!("Created {} in Triage", issue.identifier))
    } else {
        crate::flash::Flash::setup(format!(
            "Linear: {} has no Triage — created {} in its default state",
            team.name, issue.identifier
        ))
    });
    if let Some(view) = view_on(app, &dir) {
        view.linked.insert(issue.identifier.clone(), issue);
    }
}

/// Enter in the LINEAR VIEW opened by **Link existing…**: `item` linked to
/// `issue`, and the modal back as it was left.
pub(crate) fn link_issue(app: &mut App, mut back: TodoView, item: u64, issue: LinkedIssue) {
    if let Some(file) = app.todos.get_mut(&back.dir) {
        if let Some(todo) = file.item_mut(item) {
            link_to(todo, &issue);
        }
        store::save(file);
    }
    back.linked.insert(issue.identifier.clone(), issue);
    reopen(app, back);
}

/// `todo` linked to `issue`, in the state it is in now: only a move to
/// done from here on ticks it.
fn link_to(todo: &mut Item, issue: &LinkedIssue) {
    todo.linear = Some(issue.identifier.clone());
    todo.linear_seen = Some(issue.state_type.clone());
}

pub(crate) fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    pos: Position,
    out: &mut Vec<ClientRequest>,
) {
    app.dirty = true;
    let Some(view) = view(app) else {
        return;
    };
    let click = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
    if view.pick.is_some() {
        if click {
            pick_click(app, pos, out);
        }
        return;
    }
    let chip = click
        .then(|| view.chip_hits.iter().find(|(r, ..)| r.contains(pos)))
        .flatten()
        .map(|(_, item, chip)| (*item, *chip));
    if let Some((item, chip)) = chip {
        match chip {
            Chip::Agent => agent(app, item, out),
            Chip::Linear => open_issue(app, item, out),
        }
        return;
    }
    let list = view.list_area;
    match mouse.kind {
        MouseEventKind::ScrollDown if list.contains(pos) => step(app, 1),
        MouseEventKind::ScrollUp if list.contains(pos) => step(app, -1),
        MouseEventKind::Down(MouseButton::Left) if view.tab_row.contains(pos) => {
            let tab = crate::ui::tab_hit(&view.tab_hits, pos.x).and_then(|i| TodoTab::ALL.get(i));
            if let Some(tab) = tab {
                switch_tab(app, *tab);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if list.contains(pos) => click_row(app, pos),
        _ => {}
    }
}

/// A click while the menu or the team pick is up: on a row it chooses
/// it, anywhere else it puts the pick away.
fn pick_click(app: &mut App, pos: Position, out: &mut Vec<ClientRequest>) {
    let Some(pick) = view_mut(app).and_then(|v| v.pick.as_mut()) else {
        return;
    };
    match crate::list_hit::row_at(pick.rows, pick.start, pick.len(), pos) {
        Some(row) => {
            pick.selected = row;
            choose(app, out);
        }
        None => {
            if let Some(view) = view_mut(app) {
                view.pick = None;
            }
        }
    }
}

/// A click in the list: the cursor to the row — and on an item's box it
/// ticks, on a header's `▾`/`▸` it folds, on `+ new item` it opens the
/// field.
fn click_row(app: &mut App, pos: Position) {
    let Some(view) = view(app) else {
        return;
    };
    if view.input.is_some() {
        return;
    }
    let Some(index) = crate::ui::row_hit(&view.row_rects, pos) else {
        return;
    };
    let row = view
        .row_rects
        .iter()
        .find(|(i, _)| *i == index)
        .map_or(Rect::default(), |(_, r)| *r);
    let Some((entries, _)) = rows_now(app) else {
        return;
    };
    let Some(entry) = entries.get(index).copied().filter(Entry::selectable) else {
        return;
    };
    put_cursor(app, index, Some(entry));
    // The box or the fold glyph, right after the gutter and the indent —
    // on the first line: under it a wrapped item's text runs on.
    let mark_x = row.x + GUTTER_W + INDENT_W * entry.depth();
    let on_mark = pos.y == row.y && (mark_x..mark_x + MARK_W).contains(&pos.x);
    match entry {
        Entry::Item { .. } if on_mark => done(app),
        Entry::Header { .. } if on_mark => done(app),
        Entry::AddRow => start_item(app),
        _ => {}
    }
}

/// `Tue 6 Oct`.
fn day_label(date: NaiveDate) -> String {
    date.format("%a %-d %b").to_string()
}

fn width_of(spans: &[Span]) -> usize {
    use unicode_width::UnicodeWidthStr;
    spans.iter().map(|s| s.content.width()).sum()
}

/// `left`, then `rest` cut to fit, then `right` against the edge of
/// `budget` columns.
fn justify(
    mut left: Vec<Span<'static>>,
    text: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    budget: usize,
) -> Vec<Span<'static>> {
    let used = width_of(&left) + width_of(&text) + width_of(&right);
    left.extend(text);
    if used < budget {
        left.push(Span::raw(" ".repeat(budget - used)));
    }
    left.extend(right);
    left
}

/// The fewest columns an item's text keeps before its chips drop.
const MIN_TEXT_W: usize = 16;
/// The columns a row's text sits right of: `render_row`'s gutter.
const GUTTER_W: u16 = 1;
/// The columns each level of nesting indents a row by.
const INDENT_W: u16 = 2;
/// An item's box or a header's fold glyph, with the space after it.
const MARK_W: u16 = 2;

/// `depth` levels of indent.
fn indent(depth: u16) -> Span<'static> {
    Span::raw(" ".repeat((INDENT_W * depth) as usize))
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &TodoView, th: Theme, backdrop: bool) {
    let focused = !backdrop;
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let today = super::today();
    let empty = TodoFile::new(&view.dir);
    let file = app.todos.get(&view.dir).unwrap_or(&empty);
    let title = format!("Todos · {} · {}", view.project_name, day_label(today));
    let block = panel_block(&title, focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // The tabs on the first line — Today with what is open — and against
    // its right edge the week so far, when there is room for it.
    let tab_row = row_rect(inner, 0).unwrap_or_default();
    let labels = [format!("Today {}", file.open_count()), "Log".to_string()];
    let active = TodoTab::ALL
        .iter()
        .position(|t| *t == view.tab)
        .unwrap_or(0);
    let (strip, tab_hits) = crate::ui::tab_strip(
        tab_row.x,
        tab_row.width,
        labels.iter().map(String::as_str),
        active,
        false,
        th,
    );
    let strip_w = width_of(&strip);
    f.render_widget(Paragraph::new(Line::from(strip)), tab_row);
    let summary = week_summary(file, today, th);
    // A gap after the tabs, and the frame's margin on the right.
    const GAP: usize = 2;
    const MARGIN: u16 = 1;
    if strip_w + GAP + width_of(&summary) + usize::from(MARGIN) <= tab_row.width as usize {
        let right = Rect {
            width: tab_row.width.saturating_sub(MARGIN),
            ..tab_row
        };
        let line = Line::from(summary).alignment(ratatui::layout::Alignment::Right);
        f.render_widget(Paragraph::new(line), right);
    }
    // Under the tabs the filter while `⌘F` has it up, else a blank row.
    let below_tabs = crate::ui::below_first_row(inner);
    if let Some(query_area) = row_rect(below_tabs, 0).filter(|_| view.filtering) {
        let line = search_line(&view.query, "type to filter…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let rows_area = crate::ui::below_first_row(below_tabs);

    let entries = entries(file, view, today);
    let cursor = resolve(view, &entries);
    if entries.is_empty() {
        let text = if view.query.is_empty() {
            "nothing done before today yet"
        } else {
            "nothing matches"
        };
        crate::ui::empty_list_row(f, rows_area, text, th);
    }
    let budget = (rows_area.width as usize).saturating_sub(2);
    let row = Row {
        app,
        file,
        view,
        today,
        budget,
        th,
    };
    let drawn_lines: Vec<_> = entries.iter().map(|e| row.lines(*e)).collect();
    // A section, a day or `+ new item` past the first row stands a blank
    // line below what is over it.
    let gaps: Vec<bool> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| i > 0 && spaced(*e, view.tab))
        .collect();
    let heights: Vec<u16> = drawn_lines
        .iter()
        .zip(&gaps)
        .map(|((lines, _), gap)| lines.len().max(1) as u16 + u16::from(*gap))
        .collect();
    // The cursor's row on screen — and the day over it, on the Log.
    let at = cursor.unwrap_or(0);
    let first = match at.checked_sub(1).map(|i| entries[i]) {
        Some(Entry::Day { .. }) => at - 1,
        _ => at,
    };
    let (list_start, drawn) =
        crate::ui::stacked_rows(&heights, first, at, view.list_start, rows_area);
    let mut row_rects = Vec::with_capacity(drawn.len());
    let mut chip_hits = Vec::new();
    let mut drawn_lines = drawn_lines;
    for (i, rect) in drawn {
        let entry = entries[i];
        let (lines, spots) = std::mem::take(&mut drawn_lines[i]);
        let rect = if gaps[i] {
            Rect {
                y: rect.y + 1,
                height: rect.height.saturating_sub(1),
                ..rect
            }
        } else {
            rect
        };
        if rect.height == 0 {
            continue;
        }
        if let Entry::Item { id, .. } = entry {
            for (chip, x, w) in spots {
                let x = rect.x + GUTTER_W + x as u16;
                chip_hits.push((Rect::new(x, rect.y, w as u16, 1), id, chip));
            }
        }
        if entry.selectable() {
            crate::ui::render_row_lines(
                f,
                rect,
                lines,
                cursor == Some(i),
                focused && view.input.is_none(),
                th,
            );
        } else {
            f.render_widget(
                Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()),
                rect,
            );
        }
        row_rects.push((i, rect));
    }

    let mut pick = view.pick.clone();
    if let Some(pick) = &mut pick {
        draw_pick(f, area, file, pick, th);
    }
    if !backdrop {
        crate::hints::draw_on_border(f, area, &hints(view), 0, th);
    }

    if let Some(Overlay::Todos(v)) = &mut app.overlay {
        v.area = area;
        v.chip_hits = chip_hits;
        v.pick = pick;
        v.list_area = rows_area;
        v.list_start = list_start;
        v.row_rects = row_rects;
        v.tab_hits = tab_hits;
        v.tab_row = tab_row;
        if let Some(at) = cursor {
            v.selected = at;
            v.cursor = entries.get(at).copied();
        }
    }
}

/// Whether `entry` stands a blank line below the row over it: a
/// top-level group's header (or the field naming one), a day in the Log,
/// and `+ new item`.
fn spaced(entry: Entry, tab: TodoTab) -> bool {
    match entry {
        Entry::Header { depth: 0, .. } | Entry::AddRow | Entry::Day { .. } => true,
        Entry::Input { depth: 0 } => tab == TodoTab::Today,
        _ => false,
    }
}

/// The tab row's right end: what was ticked today and this week, then
/// the week a bar a day from Monday — today's brightest, the days to
/// come a faint `·`.
fn week_summary(file: &TodoFile, today: NaiveDate, th: Theme) -> Vec<Span<'static>> {
    use chrono::Datelike;
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let days = file.week_ticks(today);
    let at = today.weekday().num_days_from_monday() as usize;
    let max = days.iter().copied().max().unwrap_or(0).max(1);
    // The busiest day reaches the tallest bar.
    const TOP: usize = BARS.len() - 1;
    let mut spans = vec![
        Span::styled(format!("✓{}", days[at]), Style::default().fg(th.ok)),
        Span::styled(" today · ", Style::default().fg(th.dim)),
        Span::styled(
            format!("✓{}", days.iter().sum::<usize>()),
            Style::default().fg(th.ok),
        ),
        Span::styled(" this week ", Style::default().fg(th.dim)),
    ];
    for (i, n) in days.iter().enumerate() {
        let (bar, color) = match (i.cmp(&at), *n) {
            (std::cmp::Ordering::Greater, _) => ('·', th.faint),
            (_, 0) => (BARS[0], th.faint),
            (std::cmp::Ordering::Equal, n) => (BARS[n * TOP / max], th.text),
            (_, n) => (BARS[n * TOP / max], th.muted),
        };
        spans.push(Span::styled(bar.to_string(), Style::default().fg(color)));
    }
    spans
}

/// A row's lines, and where the chips a click acts on sit on its first:
/// each chip with its column from the line's start and its width.
type RowLines = (Vec<Vec<Span<'static>>>, Vec<(Chip, usize, usize)>);

/// What every row of one draw is drawn from.
struct Row<'a> {
    app: &'a App,
    file: &'a TodoFile,
    view: &'a TodoView,
    today: NaiveDate,
    budget: usize,
    th: Theme,
}

impl Row<'_> {
    /// `entry`'s lines — an item's text wraps onto more — and, on an item,
    /// where its clickable chips are on the first.
    fn lines(&self, entry: Entry) -> RowLines {
        let (file, view, th, budget) = (self.file, self.view, self.th, self.budget);
        let line = match entry {
            Entry::Header { group, depth } => {
                header_spans(file, view, group, depth, self.today, budget, th)
            }
            Entry::Item { id, depth } => {
                return match file.item(id) {
                    Some(item) => item_lines(self, item, depth),
                    None => (vec![Vec::new()], Vec::new()),
                }
            }
            Entry::Input { depth } => input_row(view, depth, budget, th),
            Entry::AddRow => {
                let words = if file.groups.is_empty() {
                    "+ new item — or paste an indented list"
                } else {
                    "+ new item"
                };
                vec![
                    Span::styled(words, Style::default().fg(th.dim)),
                    Span::styled(
                        format!("  {} · or type here", keys::NEW.label()),
                        Style::default().fg(th.faint),
                    ),
                ]
            }
            Entry::Day { date, count } => vec![
                Span::styled(
                    day_label(date),
                    Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" · {count} done"), Style::default().fg(th.dim)),
            ],
        };
        (vec![line], Vec::new())
    }
}

/// The `⌘L` menu or the team pick over the modal: a small box in its
/// middle, a row per choice, the cursor's lit — scrolled to keep it in
/// sight when the teams outnumber the rows.
fn draw_pick(f: &mut Frame, area: Rect, file: &TodoFile, pick: &mut Pick, th: Theme) {
    let linked = file.item(pick.item).and_then(|i| i.linear.clone());
    let (title, rows): (String, Vec<String>) = match &pick.kind {
        PickKind::Menu(actions) => (
            format!("Linear · {}", linked.as_deref().unwrap_or("not linked")),
            actions.iter().map(|a| a.label(linked.as_deref())).collect(),
        ),
        PickKind::Team(teams) => (
            "Create in which team?".into(),
            teams
                .iter()
                .map(|t| {
                    let triage = if t.triage_state.is_some() {
                        ""
                    } else {
                        "  no Triage"
                    };
                    format!("{}  {}{triage}", t.key, t.name)
                })
                .collect(),
        ),
    };
    let widest = rows
        .iter()
        .chain(std::iter::once(&title))
        .map(|r| r.chars().count())
        .max()
        .unwrap_or(0);
    let w = (widest as u16 + 6).min(area.width);
    let h = (rows.len() as u16 + 2).min(area.height);
    let rect = crate::ui::centered_rect(area, w, h);
    f.render_widget(Clear, rect);
    let block = panel_block(&title, true, th);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let start = crate::app::window_start(pick.selected, inner.height as usize);
    for (i, row) in rows.into_iter().enumerate().skip(start) {
        let Some(line) = row_rect(inner, i - start) else {
            break;
        };
        render_row(f, line, vec![Span::raw(row)], i == pick.selected, true, th);
    }
    pick.area = rect;
    pick.rows = inner;
    pick.start = start;
}

/// A group's header. A top-level one is a section: the fold glyph, the
/// name in capitals, a rule across, and against the right edge its open
/// items at each priority, how many are open and how many were ticked
/// today — nested groups counted in, folded or not. A group under it says
/// only how many are open.
fn header_spans(
    file: &TodoFile,
    view: &TodoView,
    group: u64,
    depth: u16,
    today: NaiveDate,
    budget: usize,
    th: Theme,
) -> Vec<Span<'static>> {
    let Some(g) = file.group(group) else {
        return Vec::new();
    };
    if view.confirm_delete == Some(group) {
        return vec![
            indent(depth),
            Span::styled(
                format!(
                    "delete {} and its {}?",
                    g.name,
                    crate::bundle::plural(file.size(group), "item")
                ),
                Style::default().fg(th.err).add_modifier(Modifier::BOLD),
            ),
        ];
    }
    let top = depth == 0;
    let counts = file.counts(group, today);
    let dim = Style::default().fg(th.dim);
    let open = Span::styled(format!("{} open", counts.open_total()), dim);
    let right = if top {
        let mut right: Vec<Span<'static>> = Vec::new();
        // The levels with something open — no priority is in the total.
        let levels = crate::linear::PRIORITY_ORDER.iter().zip(counts.open.iter());
        for (priority, n) in levels.take(super::PRIORITIES - 1) {
            if *n > 0 {
                right.push(crate::linear::priority_mark(*priority, th));
                right.push(Span::styled(format!("{n} "), dim));
            }
        }
        right.push(Span::styled("· ", Style::default().fg(th.faint)));
        right.push(open);
        if counts.done_today > 0 {
            right.push(Span::styled(" · ", Style::default().fg(th.faint)));
            right.push(Span::styled(
                format!("✓{}", counts.done_today),
                Style::default().fg(th.ok),
            ));
        }
        right
    } else {
        vec![open]
    };
    let fold = if g.collapsed { "▸ " } else { "▾ " };
    let left = vec![
        indent(depth),
        Span::styled(
            fold,
            Style::default().fg(if top { th.muted } else { th.dim }),
        ),
    ];
    let full = if top {
        g.name.to_uppercase()
    } else {
        g.name.clone()
    };
    // A space either side of the rule, and at least a cell of it.
    const AROUND_RULE: usize = 2;
    let room = budget
        .saturating_sub(width_of(&left) + width_of(&right) + AROUND_RULE + 1)
        .max(4);
    let name = truncate(&full, room);
    let positions = crate::fuzzy::fuzzy_match(view.query.trim(), &full)
        .map(|m| visible_positions(&m.positions, &name, &full).to_vec())
        .unwrap_or_default();
    let style = Style::default()
        .fg(if top { th.text } else { th.muted })
        .add_modifier(Modifier::BOLD);
    let name = fuzzy_highlight_styled(&name, &positions, style, th);
    if !top {
        return justify(left, name, right, budget);
    }
    let rule = budget
        .saturating_sub(width_of(&left) + width_of(&name) + width_of(&right) + AROUND_RULE)
        .max(1);
    let mut spans = left;
    spans.extend(name);
    spans.push(Span::raw(" "));
    spans.push(Span::styled("─".repeat(rule), Style::default().fg(th.edge)));
    spans.push(Span::raw(" "));
    spans.extend(right);
    spans
}

/// `text` broken at spaces into lines of at most `first` columns, then
/// `rest` — each line with the char it starts at, so the filter's
/// highlights land on the line they are in. A word longer than a line is
/// cut across lines. Its own rather than `pr_preview::wrap`, which has
/// neither the narrower first line (the chips' room) nor the offsets;
/// columns are display columns, so a wide character takes two.
fn wrap_at(text: &str, first: usize, rest: usize) -> Vec<(usize, String)> {
    use unicode_width::UnicodeWidthChar;
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut width = first.max(1);
    while start < chars.len() {
        // As many chars as fit `width` columns — at least one.
        let mut end = start;
        let mut used = 0;
        while let Some(c) = chars.get(end) {
            let w = c.width().unwrap_or(0);
            if used + w > width && end > start {
                break;
            }
            used += w;
            end += 1;
        }
        let cut = if end == chars.len() {
            end
        } else {
            (start + 1..=end)
                .rev()
                .find(|&i| chars[i] == ' ')
                .unwrap_or(end)
        };
        out.push((start, chars[start..cut].iter().collect()));
        start = cut;
        while chars.get(start) == Some(&' ') {
            start += 1;
        }
        width = rest.max(1);
    }
    if out.is_empty() {
        out.push((0, String::new()));
    }
    out
}

/// An item's lines: its box, its priority letter, its text — struck
/// through when it was ticked today, and wrapped under its own first
/// character when it is long — and its chips against the right edge of
/// the first line, dropped from the right when the line is short: the
/// agent sent at it, its Linear issue, how many days it has carried over.
/// With the lines, the columns (from the line's start) of the chips a
/// click acts on.
fn item_lines(row: &Row, item: &Item, depth: u16) -> RowLines {
    let Row {
        app,
        file,
        view,
        today,
        budget,
        th,
    } = *row;
    let done = item.done.is_some();
    let mut left = vec![indent(depth)];
    left.push(if done {
        Span::styled("☑ ", Style::default().fg(th.ok))
    } else {
        Span::styled("☐ ", Style::default().fg(th.dim))
    });
    left.push(crate::linear::priority_mark(item.priority, th));
    left.push(Span::raw(" "));

    let mut parts: Vec<(Option<Chip>, Vec<Span<'static>>)> = Vec::new();
    if let Some(agent) = live_agent(app, item) {
        let color = crate::ui::status_color(Some(agent.status), agent.unseen, th);
        parts.push((
            Some(Chip::Agent),
            vec![Span::styled("● agent", Style::default().fg(color))],
        ));
    }
    if let Some(id) = &item.linear {
        parts.push((Some(Chip::Linear), linear_chip(id, view.linked.get(id), th)));
    }
    if view.tab == TodoTab::Log {
        parts.push((
            None,
            vec![Span::styled(
                file.path_label(item.group),
                Style::default().fg(th.dim),
            )],
        ));
    } else if !done && item.age(today) > 0 {
        parts.push((
            None,
            vec![Span::styled(
                format!("{}d", item.age(today)),
                Style::default().fg(th.faint),
            )],
        ));
    }
    // As many as fit, with where each landed — the one cut the line and
    // the click both read.
    let (kinds, parts): (Vec<Option<Chip>>, Vec<_>) = parts.into_iter().unzip();
    let lead = width_of(&left);
    let chip_budget = budget.saturating_sub(lead + MIN_TEXT_W);
    let (chips, at) = fit_parts_at(parts, chip_budget, th);
    let positions = if view.query.trim().is_empty() {
        Vec::new()
    } else {
        crate::fuzzy::fuzzy_match(view.query.trim(), &item.text)
            .map(|m| m.positions)
            .unwrap_or_default()
    };
    let base = if done && view.tab == TodoTab::Today {
        Style::default()
            .fg(th.dim)
            .add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::default().fg(th.text)
    };
    let first = budget.saturating_sub(lead + width_of(&chips) + 1);
    let rest = budget.saturating_sub(lead);
    let mut lines = Vec::new();
    let mut spots = Vec::new();
    for (n, (start, text)) in wrap_at(&item.text, first, rest).into_iter().enumerate() {
        let count = text.chars().count();
        let here: Vec<usize> = positions
            .iter()
            .filter(|p| (start..start + count).contains(p))
            .map(|p| p - start)
            .collect();
        let text = fuzzy_highlight_styled(&text, &here, base, th);
        if n > 0 {
            let mut line = vec![Span::raw(" ".repeat(lead))];
            line.extend(text);
            lines.push(line);
            continue;
        }
        let before = lead + width_of(&text);
        let from = budget.saturating_sub(width_of(&chips)).max(before);
        spots = kinds
            .iter()
            .zip(&at)
            .filter_map(|(kind, (x, w))| Some(((*kind)?, from + x, *w)))
            .collect();
        lines.push(justify(left.clone(), text, chips.clone(), budget));
    }
    (lines, spots)
}

/// The session sent at `item`, while it is still there.
fn live_agent<'a>(app: &'a App, item: &Item) -> Option<&'a orion_core::Agent> {
    let id = item.agent.as_deref()?;
    app.tree.agents.iter().find(|a| a.id.0 == id)
}

/// A linked issue's chip: its state's glyph in its colour, `RIP-412`,
/// and the state's name — or a dim `·` before Linear has said.
fn linear_chip(id: &str, issue: Option<&LinkedIssue>, th: Theme) -> Vec<Span<'static>> {
    let (glyph, color) = match issue {
        Some(issue) => crate::linear::state_glyph(&issue.state_type, &issue.state_color, th),
        None => ("·", th.dim),
    };
    let mut chip = vec![
        Span::styled(format!("{glyph} "), Style::default().fg(color)),
        Span::styled(id.to_string(), Style::default().fg(th.muted)),
    ];
    if let Some(issue) = issue.filter(|i| !i.state.is_empty()) {
        chip.push(Span::styled(
            format!(" {}", issue.state),
            Style::default().fg(th.dim),
        ));
    }
    chip
}

/// The open field, where what it makes will stand: a box or a fold glyph
/// for what it is, then the text and the caret — a dim word for it while
/// it is empty.
fn input_row(view: &TodoView, depth: u16, budget: usize, th: Theme) -> Vec<Span<'static>> {
    let Some((kind, input)) = &view.input else {
        return Vec::new();
    };
    let (mark, word) = match kind {
        InputKind::Item { .. } => ("☐ ", "new item"),
        InputKind::Group { .. } => ("▾ ", "new group"),
        InputKind::Rename(Target::Group(_)) => ("▾ ", "name"),
        InputKind::Rename(Target::Item(_)) => ("☐ ", "item"),
    };
    let mut spans = vec![
        indent(depth),
        Span::styled(mark, Style::default().fg(th.accent)),
    ];
    let room = budget.saturating_sub(width_of(&spans));
    spans.extend(input_spans(input, room, th.accent, th));
    if input.is_empty() {
        spans.push(Span::styled(word, Style::default().fg(th.dim)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    fn view() -> TodoView {
        TodoView::new(ProjectId("p1".into()), "demo".into(), PathBuf::from("/r"))
    }

    /// Every hint the modal draws names a key in its table — the priority
    /// keys drawn as one, `⌘1-4`.
    #[test]
    fn the_hints_come_from_the_table() {
        let check = |view: &TodoView| {
            let hints: Vec<_> = hints(view)
                .into_iter()
                .filter(|h| h.does != "priority")
                .collect();
            crate::hints::assert_hints_from(&hints, keys::ALL);
        };
        let mut v = view();
        check(&v);
        v.tab = TodoTab::Log;
        check(&v);
        v.input = Some((InputKind::Item { group: None }, TextInput::new()));
        check(&v);
        v.input = None;
        v.confirm_delete = Some(1);
        check(&v);
        v.confirm_delete = None;
        v.filtering = true;
        check(&v);
    }

    /// Lines break at spaces, the first narrower than the rest, each
    /// with the char it starts at; a wide character takes two columns,
    /// and a word too long for a line is cut.
    #[test]
    fn items_wrap_by_display_width() {
        let lines = |text, first, rest| -> Vec<(usize, String)> { wrap_at(text, first, rest) };
        assert_eq!(
            lines("one two three four", 7, 10),
            [(0, "one two".into()), (8, "three four".into())]
        );
        assert_eq!(lines("", 5, 5), [(0, String::new())]);
        assert_eq!(lines("abcdefgh", 3, 3).len(), 3);
        // Four wide characters are eight columns: two to a six-column line.
        let wide = lines("日本語版", 6, 6);
        assert_eq!(wide, [(0, "日本語".into()), (3, "版".into())]);
    }

    /// Folded groups show their header only; a filter opens them, keeps
    /// the headers over what it finds and drops the groups it finds
    /// nothing in.
    #[test]
    fn the_rows_fold_and_filter() {
        let mut file = TodoFile::new(Path::new("/r"));
        let a = file.add_group(None, "Emails");
        let b = file.add_group(Some(a), "Later");
        let c = file.add_group(None, "UI");
        file.add_item(a, "run plan", day(6));
        file.add_item(b, "setup resend", day(6));
        file.add_item(c, "chips", day(6));
        file.group_mut(a).unwrap().collapsed = true;
        let mut v = view();
        let rows = entries(&file, &v, day(6));
        assert_eq!(
            rows,
            [
                Entry::Header { group: a, depth: 0 },
                Entry::Header { group: c, depth: 0 },
                Entry::Item { id: 6, depth: 1 },
                Entry::AddRow,
            ]
        );
        v.query.insert_str("resend");
        let rows = entries(&file, &v, day(6));
        assert_eq!(
            rows,
            [
                Entry::Header { group: a, depth: 0 },
                Entry::Header { group: b, depth: 1 },
                Entry::Item { id: 5, depth: 2 },
                Entry::AddRow,
            ]
        );
    }

    /// The field stands where what it makes will: a new item after its
    /// group's items, a new group after its siblings, a rename in the
    /// row's own place.
    #[test]
    fn the_field_stands_where_its_row_will() {
        let mut file = TodoFile::new(Path::new("/r"));
        let a = file.add_group(None, "A");
        let sub = file.add_group(Some(a), "Sub");
        let item = file.add_item(a, "x", day(6));
        let mut v = view();
        v.input = Some((InputKind::Item { group: Some(a) }, TextInput::new()));
        assert_eq!(
            entries(&file, &v, day(6))[2],
            Entry::Input { depth: 1 },
            "after x, before Sub"
        );
        v.input = Some((InputKind::Group { parent: Some(a) }, TextInput::new()));
        assert_eq!(entries(&file, &v, day(6))[3], Entry::Input { depth: 1 });
        v.input = Some((InputKind::Rename(Target::Item(item)), TextInput::new()));
        assert_eq!(entries(&file, &v, day(6))[1], Entry::Input { depth: 1 });
        assert_eq!(resolve(&v, &entries(&file, &v, day(6))), Some(1));
        let _ = sub;
    }
}
