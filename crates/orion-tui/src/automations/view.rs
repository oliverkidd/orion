//! The AUTOMATIONS overview (`⌘J`): what the automations are doing. A row
//! per automation — its dot (yellow working, red waiting on you, grey
//! idle, hollow off) and a short state — and under each its live sessions,
//! with their project and how long they have been at it. The header says
//! how many automation sessions are running against the overall cap.
//! Everything they are set up by lives in Settings › Automations
//! ([`super::settings_tab`]).
//!
//! Enter on a session row jumps to it; on an automation row it opens
//! Settings › Automations at that automation's section — except week in
//! review, whose Enter opens its latest review (`r` runs it now), and
//! meeting takeaways, whose Enter unfolds its MEETINGS, Enter on one
//! opening its takeaways. `Space` turns the automation under the cursor
//! on or off, and `,` opens its settings. INPUT PARITY: a click on a row
//! puts the cursor there, and a click on the row it is on is Enter on it
//! ([`activate`]).
//!
//! The same modal hosts the Linear labels pick for **Labels** in
//! Settings › Automations, which comes back there when it is done.

use super::config::AutomationsSettings;
use super::{Automation, Dot, Engine, Tone};
use crate::app::{App, Overlay};
use crate::hints::Key;
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
    pub const SETTINGS: Key = Key::new(&[","], "settings");
    pub const BACK: Key = crate::nav::CLOSE.doing("back");
    #[cfg(test)]
    pub const ALL: &[Key] = &[TOGGLE, OPEN, SETTINGS, BACK];
}

/// What the dim line above the keys says.
const EXPLAIN: &str =
    "What the automations are doing. Their caps, labels, presets and steer are in Settings › Automations.";

/// The widest the screen gets.
const WIDEST: u16 = 100;
/// The name column of an automation's row (`Meeting takeaways  `).
const NAME_W: usize = 19;
/// A session row's project column, and its age column.
const PROJECT_W: usize = 16;
const AGE_W: usize = 5;

/// A row of the overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Automation(Automation),
    /// One of its live sessions.
    Session(Automation, AgentId),
    /// Meeting takeaways' MEETINGS, unfolded: the `n`th meeting.
    Meeting(usize),
}

impl Item {
    pub fn automation(&self) -> Automation {
        match self {
            Item::Automation(which) | Item::Session(which, _) => *which,
            Item::Meeting(_) => Automation::MeetingTakeaways,
        }
    }
}

/// What a click lands on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hit {
    /// A row of the overview, by its index among [`items`].
    Item(usize),
    /// A row of the labels pick.
    Label(usize),
}

#[derive(Debug, Clone, Default)]
pub struct AutomationsView {
    /// The row under the cursor, an index into [`items`].
    pub selected: usize,
    /// Meeting takeaways' MEETINGS are unfolded under its row.
    pub meetings: bool,
    /// **Labels** in Settings › Automations: the labels pick, the whole
    /// modal, back on [`Self::back`] once it is done.
    pub labels: Option<super::linear_builder::LabelPick>,
    /// The Settings › Automations row the labels pick goes back to.
    pub back: usize,
    /// A word about the last thing tried, over the explanation.
    pub notice: Option<String>,
    /// The modal's rect and its rows', written back as it draws.
    pub area: Rect,
    hits: Vec<(Rect, Hit)>,
}

impl AutomationsView {
    /// The modal standing in for Settings › Automations while a runner's
    /// own editor (the labels pick) is up, back on row `at` after.
    pub fn editing_from(at: usize) -> Self {
        Self {
            back: at,
            ..Self::default()
        }
    }
}

/// Whether `id` is a live session: not archived, at work, waiting on you
/// or booting.
fn live(app: &App, id: &AgentId) -> bool {
    app.tree
        .agents
        .iter()
        .find(|a| &a.id == id)
        .is_some_and(|a| !a.archived && super::occupies(a))
}

/// The overview's rows: each automation, then its live sessions from its
/// panel's lists — and, unfolded, meeting takeaways' meetings.
pub fn items(app: &App, meetings: bool) -> Vec<Item> {
    let mut items = Vec::new();
    for which in Automation::ALL {
        items.push(Item::Automation(which));
        let panel = Engine::panel(app, which);
        let mut seen: Vec<&AgentId> = Vec::new();
        let rows = panel.lists.iter().flat_map(|list| &list.rows);
        for id in rows.clone().filter_map(|row| row.session.as_ref()) {
            if live(app, id) && !seen.contains(&id) {
                seen.push(id);
                items.push(Item::Session(which, id.clone()));
            }
        }
        if which == Automation::MeetingTakeaways && meetings {
            let n = rows.filter(|row| row.session.is_none()).count();
            items.extend((0..n).map(Item::Meeting));
        }
    }
    items
}

/// The row of `which` among `items`.
fn row_of(items: &[Item], which: Automation) -> usize {
    items
        .iter()
        .position(|item| *item == Item::Automation(which))
        .unwrap_or(0)
}

/// `⌘J`: the overview, on the automation it was last left on.
pub fn open(app: &mut App) {
    let which = Automation::ALL[app.automations.selected.min(Automation::ALL.len() - 1)];
    app.overlay = Some(Overlay::Automations(AutomationsView {
        selected: row_of(&items(app, false), which),
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

/// The row under the cursor.
fn current(app: &App, view: &AutomationsView) -> Option<Item> {
    items(app, view.meetings).into_iter().nth(view.selected)
}

pub fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Automations(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    view.notice = None;
    // Picking labels: the pick has the keys, and its Enter or Esc goes
    // back to Settings › Automations.
    if view.labels.is_some() {
        let back = view.back;
        super::linear_builder::pick_key(app, key);
        if matches!(&app.overlay, Some(Overlay::Automations(v)) if v.labels.is_none()) {
            super::settings_tab::reopen_at(app, back);
        }
        return;
    }
    let view = view.clone();
    let item = current(app, &view);
    let which = item.as_ref().map_or(Automation::PrFixer, Item::automation);
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
        _ if keys::BACK.matches(&key) => close(app),
        _ if keys::TOGGLE.matches(&key) => super::settings_tab::toggle(app, which),
        _ if keys::SETTINGS.matches(&key) => {
            close(app);
            super::settings_tab::open_at(app, Some(which));
        }
        KeyCode::Up | KeyCode::Down => {
            let n = items(app, view.meetings).len();
            let at = step(view.selected, key.code == KeyCode::Down, n);
            select(app, at);
        }
        _ if keys::OPEN.matches(&key) => activate(app, view.selected, out),
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

/// The cursor on row `at`, its automation remembered for the next open.
fn select(app: &mut App, at: usize) {
    let Some(Overlay::Automations(view)) = &app.overlay else {
        return;
    };
    let which = items(app, view.meetings).get(at).map(Item::automation);
    if let Some(which) = which {
        app.automations.selected = Automation::ALL
            .iter()
            .position(|a| *a == which)
            .unwrap_or_default();
    }
    if let Some(Overlay::Automations(view)) = &mut app.overlay {
        view.selected = at;
    }
}

/// Close the screen.
fn close(app: &mut App) {
    app.overlay = None;
}

/// Row `index` chosen — Enter on it, or a click on the row the cursor is
/// on: a session jumps to it, an automation opens its settings, week in
/// review its latest review, meeting takeaways its meetings.
pub fn activate(app: &mut App, index: usize, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Automations(view)) = &app.overlay else {
        return;
    };
    let meetings = view.meetings;
    select(app, index);
    let Some(item) = items(app, meetings).into_iter().nth(index) else {
        return;
    };
    match item {
        Item::Automation(Automation::WeekReview) => {
            let any = app.week_review.run.is_some() || !app.week_review.kept.is_empty();
            if !any {
                if let Some(Overlay::Automations(view)) = &mut app.overlay {
                    view.notice = Some("No reviews yet · r runs one now".into());
                }
                return;
            }
            close(app);
            // The run being written, else the newest kept.
            crate::week_review::open(app, 0, true);
        }
        Item::Automation(Automation::MeetingTakeaways) => {
            if let Some(Overlay::Automations(view)) = &mut app.overlay {
                view.meetings = !view.meetings;
            }
        }
        Item::Automation(which) => {
            close(app);
            super::settings_tab::open_at(app, Some(which));
        }
        // Blue sky's IDEAS jump to the idea, whose row has its session.
        Item::Session(Automation::BlueSky, id) => {
            close(app);
            super::blue_sky::open_idea(app, &id);
        }
        Item::Session(_, id) => {
            close(app);
            crate::event_loop::jump_to_session(app, id, out);
        }
        Item::Meeting(n) => {
            close(app);
            super::with_runner(app, Automation::MeetingTakeaways, |r, app| {
                r.open_row(app, n)
            });
        }
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
    let (selected, meetings) = (view.selected, view.meetings);
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if view.labels.is_none() => {
            let down = matches!(mouse.kind, MouseEventKind::ScrollDown);
            let n = items(app, meetings).len();
            select(app, step(selected, down, n));
        }
        MouseEventKind::Down(MouseButton::Left) => match hit {
            Some(Hit::Item(i)) if i == selected => activate(app, i, out),
            Some(Hit::Item(i)) => select(app, i),
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

/// What Enter does on `item`.
fn enter_does(item: Option<&Item>) -> &'static str {
    match item {
        Some(Item::Automation(Automation::WeekReview)) => "latest review",
        Some(Item::Automation(Automation::MeetingTakeaways)) => "meetings",
        Some(Item::Automation(_)) => "settings",
        Some(Item::Session(..)) => "jump",
        Some(Item::Meeting(_)) => "takeaways",
        None => "open",
    }
}

pub fn hints(app: &App, view: &AutomationsView) -> Vec<crate::hints::Hint> {
    if view.labels.is_some() {
        return vec![
            Key::new(&["space"], "tick").hint().kept(),
            Key::new(&["enter"], "save").hint().kept(),
            crate::nav::CLOSE.doing("cancel").hint().kept(),
        ];
    }
    let item = current(app, view);
    let mut hints = vec![keys::OPEN.hint_as(enter_does(item.as_ref())).kept()];
    if let Some(own) = item
        .as_ref()
        .and_then(|i| app.automations.runner(i.automation()))
        .and_then(|r| r.hotkey())
    {
        hints.push(own.hint());
    }
    hints.extend([
        keys::TOGGLE.hint(),
        keys::SETTINGS.hint(),
        keys::BACK.hint().kept(),
    ]);
    hints
}

/// A session's project and how long it has been at its state.
fn session_meta(app: &App, id: &AgentId) -> (String, String) {
    let Some(agent) = app.tree.agents.iter().find(|a| &a.id == id) else {
        return Default::default();
    };
    let project = app
        .tree
        .worktrees
        .iter()
        .find(|w| w.id == agent.worktree_id)
        .and_then(|w| app.tree.projects.iter().find(|p| p.id == w.project_id))
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let secs = (crate::app::now_ms() - agent.status_changed_at) / 1000;
    let age = match agent.status_changed_at {
        0 => String::new(),
        _ => crate::hosts::ago_short(secs),
    };
    (project, age)
}

/// `text` cut and padded to `w` cells.
fn cell(text: &str, w: usize) -> String {
    let text = crate::ui::truncate(text, w.saturating_sub(1));
    format!("{text:<w$}")
}

pub fn draw(f: &mut Frame, app: &mut App, view: &AutomationsView, th: Theme) {
    let screen = f.area();
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let rows = items(app, view.meetings);
    let want = rows.len() as u16 + 7;
    let height = want.min(screen.height.saturating_sub(2)).max(10);
    let area = crate::ui::centered_rect(screen, width, height);
    let title = match &view.labels {
        Some(_) => " Labels ",
        None => " Automations ",
    };
    let inner = crate::ui::render_modal_frame(f, area, title, &hints(app, view), th);
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
        if let Some(Overlay::Automations(live)) = &mut app.overlay {
            live.area = area;
            live.hits = hits;
        }
        return;
    }

    let dim = Style::default().fg(th.dim);
    let w = usize::from(body.width);
    // The header: what is running against the overall cap.
    let header = format!(
        "{} of at most {} running · caps in Settings",
        super::running(app),
        app.automations.settings.cap()
    );
    if let Some(row) = crate::ui::row_rect(body, 0) {
        let line = spread(
            Vec::new(),
            Span::styled(header, Style::default().fg(th.muted)),
            w,
        );
        f.render_widget(Paragraph::new(line), row);
    }
    let list = Rect {
        y: body.y + 2,
        height: body.height.saturating_sub(2),
        ..body
    };
    let selected = view.selected.min(rows.len().saturating_sub(1));
    let fits = usize::from(list.height).max(1);
    let first = (selected + 1).saturating_sub(fits);
    // Each automation's panel, once a frame, by `Automation as usize`.
    let panels = Automation::ALL.map(|which| Engine::panel(app, which));
    let meeting_rows: Vec<&super::PanelRow> = panels[Automation::MeetingTakeaways as usize]
        .lists
        .iter()
        .flat_map(|l| &l.rows)
        .filter(|r| r.session.is_none())
        .collect();
    for (n, (i, item)) in rows.iter().enumerate().skip(first).take(fits).enumerate() {
        let Some(rect) = crate::ui::row_rect(list, n) else {
            break;
        };
        let line = match item {
            Item::Automation(which) => {
                let panel = &panels[*which as usize];
                let name = Span::styled(cell(which.name(), NAME_W), Style::default().fg(th.text));
                let state = Span::styled(panel.state.clone(), dim);
                Line::from(vec![Span::raw(" "), dot(panel.dot, th), name, state])
            }
            Item::Session(which, id) => {
                let row = panels[*which as usize]
                    .lists
                    .iter()
                    .flat_map(|l| &l.rows)
                    .find(|r| r.session.as_ref() == Some(id));
                let (text, state, row_tone) = row.map_or_else(
                    || (id.0.clone(), String::new(), Tone::Quiet),
                    |r| (r.text.clone(), r.state.clone(), r.tone),
                );
                let (project, age) = session_meta(app, id);
                let right = format!("{}{age:>AGE_W$} ", cell(&project, PROJECT_W));
                let room = w.saturating_sub(4 + right.chars().count() + 1);
                let state_w = state.chars().count().min(room / 2);
                let text_w = room.saturating_sub(state_w + 2);
                let left = vec![
                    Span::raw("    "),
                    Span::styled(cell(&text, text_w), Style::default().fg(th.text)),
                    Span::styled(crate::ui::truncate(&state, state_w), tone(row_tone, th)),
                ];
                spread(left, Span::styled(right, dim), w)
            }
            Item::Meeting(n) => {
                let (text, state, row_tone) = meeting_rows
                    .get(*n)
                    .map_or((String::new(), String::new(), Tone::Quiet), |r| {
                        (r.text.clone(), r.state.clone(), r.tone)
                    });
                spread(
                    vec![
                        Span::raw("    "),
                        Span::styled(text, Style::default().fg(th.text)),
                    ],
                    Span::styled(format!("{state} "), tone(row_tone, th)),
                    w,
                )
            }
        };
        let lit = i == selected;
        let line = Line::from(
            line.spans
                .into_iter()
                .map(|s| match lit {
                    true => Span::styled(
                        s.content,
                        s.style.bg(th.sel_bg).add_modifier(Modifier::BOLD),
                    ),
                    false => s,
                })
                .collect::<Vec<_>>(),
        );
        f.render_widget(Paragraph::new(line), rect);
        hits.push((rect, Hit::Item(i)));
    }

    if let Some(Overlay::Automations(live)) = &mut app.overlay {
        live.area = area;
        live.hits = hits;
        live.selected = selected;
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
    use orion_core::{Agent, AgentKind, AgentStatus, WorktreeId};
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
            },
        );
        id
    }

    #[test]
    fn every_key_parses() {
        assert!(keys::ALL.iter().all(Key::parses));
    }

    #[test]
    fn the_overview_lists_every_automation_and_the_running_count() {
        in_config(|| {
            let mut app = App::new();
            open(&mut app);
            let text = screen(&mut app);
            for which in Automation::ALL {
                assert!(text.contains(which.name()), "{}:\n{text}", which.name());
            }
            assert!(
                text.contains("0 of at most 3 running · caps in Settings"),
                "{text}"
            );
            assert!(!text.contains("SETTINGS"), "no settings here:\n{text}");
        });
    }

    #[test]
    fn space_turns_the_automation_on_and_off_and_saves_it() {
        in_config(|| {
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            assert_eq!(
                current(&app, view(&app)),
                Some(Item::Automation(Automation::BugFixer))
            );
            assert!(!app.automations.is_on(Automation::BugFixer));
            press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            assert!(app.automations.is_on(Automation::BugFixer));
            let saved = crate::config::Config::load();
            assert!(saved.automations.is_on(Automation::BugFixer), "saved");
            press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            assert!(!app.automations.is_on(Automation::BugFixer));
        });
    }

    /// Enter on an automation row opens Settings › Automations at its
    /// section, `,` too; on a session row it jumps to the session.
    #[test]
    fn enter_opens_an_automations_settings_or_jumps_to_its_session() {
        in_config(|| {
            let mut app = App::new();
            app.automations.settings.of_mut(Automation::BugFixer).on = Some(true);
            let id = bug_fixer_session(&mut app);
            open(&mut app);
            let rows = items(&app, false);
            assert_eq!(rows[1], Item::Automation(Automation::BugFixer));
            assert_eq!(rows[2], Item::Session(Automation::BugFixer, id.clone()));
            assert!(screen(&mut app).contains("ENG-412"));

            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
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

            // Back on the bug fixer: `,` is the same.
            open(&mut app);
            press(&mut app, KeyCode::Char(','), KeyModifiers::NONE);
            assert!(matches!(&app.overlay, Some(Overlay::Settings(_))));

            open(&mut app);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            assert_eq!(
                current(&app, view(&app)),
                Some(Item::Session(Automation::BugFixer, id))
            );
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(app.overlay.is_none(), "jumped to the session");
        });
    }

    /// Week in review's Enter says there's no review yet; meeting
    /// takeaways' unfolds its meetings and folds them again.
    #[test]
    fn week_review_and_meeting_takeaways_open_their_own() {
        in_config(|| {
            let mut app = App::new();
            app.automations.selected = 3;
            open(&mut app);
            assert_eq!(
                current(&app, view(&app)),
                Some(Item::Automation(Automation::WeekReview))
            );
            assert!(hints(&app, view(&app)).iter().any(|h| h.does == "run now"));
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(view(&app)
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("No reviews yet")));
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(view(&app).meetings);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(!view(&app).meetings);
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.overlay.is_none());
        });
    }

    /// INPUT PARITY: a click on a row is the arrows landing there; a click
    /// on the row the cursor is on is Enter on it.
    #[test]
    fn a_click_is_the_keys_choice() {
        let click = |app: &mut App, i: usize| {
            screen(app);
            let rect = view(app)
                .hits
                .iter()
                .find(|(_, h)| *h == Hit::Item(i))
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
            Some(Overlay::Automations(v)) => format!("automations {}", v.selected),
            Some(Overlay::Settings(v)) => format!("settings {} {}", v.tab, v.selected),
            other => format!("{other:?}"),
        };
        let by_key = in_config(|| {
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Down, KeyModifiers::NONE);
            let moved = landed(&app);
            press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            (moved, landed(&app))
        });
        let by_click = in_config(|| {
            let mut app = App::new();
            open(&mut app);
            click(&mut app, 2);
            let moved = landed(&app);
            click(&mut app, 2);
            (moved, landed(&app))
        });
        assert_eq!(by_key, by_click);
        assert!(by_key.1.starts_with("settings"), "{by_key:?}");
    }
}
