//! The LAUNCHER VIEW's drawing (`crate::launcher` is its model,
//! `event_loop::launcher` its keys): the GRID of session cards — each card
//! the session's name and status, gathered into a BAND per worktree whose
//! rule wears the branch and its pull request — beside the PANE that reads
//! the card under the cursor (`ui::draw` splits the body and fills that pane);
//! plus the view's own pieces of the QUICK PROMPT (its title and the
//! header of its fields, each with its key) and the PROJECT PICKER.

use super::{
    ago_badge, below_first_row, centered_rect, empty_list_row, fuzzy_highlight_spans,
    over_box_rect, render_modal_frame, render_row, row_rect, search_line, status_color, status_dot,
    status_name_spans, sweep_ramp, truncate, visible_positions, wants_you, NO_MATCHES,
    OVER_BOX_INSET, PENDING_SESSION_BADGE,
};
use crate::app::{App, Focus, HitTarget, Overlay};
use crate::keymap::Action;
use crate::launcher::{BoxField, Hidden, ProjectPicker, ProjectTab, Tally};
use crate::quick_prompt::QuickLaunch;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

/// Width of the view's QUICK PROMPT, and its height: wider and taller than
/// the panels' box, since here it is the front door — room for its two
/// header rows, the blank one under them and the question along the
/// bottom, with the editor keeping its full height.
pub(super) const BOX_SIZE: (u16, u16) = (92, 18);
/// The PROJECT PICKER's width, and the most rows it lists before scrolling.
const PICKER_W: u16 = 64;
const PICKER_ROWS: u16 = 14;
/// What a full-screen session's back button says — the word a click on
/// it goes back to.
const CRUMB: &str = "sessions";
/// Longest a project's name is drawn on its PROJECT TAB before it is
/// clipped, so one long name cannot push every other tab off the row.
const PROJECT_TAB_MAX: usize = 20;
/// The lit tab's name is never cut under this: at that point it gives
/// way whole, and the MORE CHIP counts it with the rest.
const TAB_NAME_MIN: usize = 3;
/// The air between the PROJECT TABS and the counts right of them.
const HEAD_GAP: usize = 2;
/// The button before the first PROJECT TAB, and what it says with no tab
/// beside it — the one time the header has room to say what it does.
/// A column of air either side is part of the button, so the pointer has
/// more than one cell to find.
const ADD: &str = "+";
const ADD_EMPTY: &str = "+ open a project";

/// The view's area, with the view on: the PROJECT TABS header, then the
/// GRID of the lit project's sessions under it. `body` is what
/// `crate::launcher::split` left over the PANE, which `ui::draw` fills
/// with the entry under the cursor; Enter on a session steps down into
/// that pane (`event_loop::launcher::enter_pane`);
/// on a body too short to draw one it full-screens the session over the
/// lot instead (`event_loop::launcher::open_session`, the `collapsed` arm
/// of `ui::draw`).
pub(super) fn draw(f: &mut Frame, app: &mut App, body: Rect) {
    app.body_area = body;
    app.settle_launcher_focus();
    // The project the grid is on gets its tab, whichever way it was
    // opened, before the header lays the tabs out.
    app.settle_project_tabs();
    let bands = crate::launcher::bands(app);
    let area = crate::launcher::grid_area(body);
    let cursor = wearing(app, crate::launcher::band_cursor(app, &bands));
    let count = HeadCount::of(&bands);
    if bands.is_empty() {
        draw_head(f, app, body, count, Hidden::default());
        // Nothing archived is not nothing at all: the hero's "type a
        // task" would be advice about the wrong list, so the ARCHIVED
        // VIEW gets the plain empty line and the way back out of it.
        if app.show_archived {
            draw_list_empty(f, app, area, NO_ARCHIVED);
        } else {
            draw_empty(f, app, area);
        }
        return;
    }
    // Every band its entries stacked a line apiece, so the whole panel
    // may run taller than the screen and scrolls as one list.
    let panel = app.panel_layout(&bands);
    let scroll = settle_panel_scroll(app, &panel, &bands, cursor);
    draw_head(f, app, body, count, panel.hidden(scroll));
    draw_bands(f, app, area, &panel, &bands, cursor, scroll);
    draw_marks(f, app, &bands);
}

/// What an entry picked out for the MULTI-SELECT wears at its left, one
/// cell in.
const MARKED_MARK: &str = "✓";
/// How much of the accent a picked-out entry's fill keeps: well over the
/// cursor's own wash ([`TINT_STILL`]), so a run of them reads as one
/// selection.
const MARKED_TINT: f32 = 0.28;

/// The MULTI-SELECT over what the bands drew: every picked-out entry and
/// drawer line washed in the accent, with a `✓` at its left — found by
/// the hit rect each was drawn into. Marks left on an entry no longer on
/// the grid are dropped here, as the frame is the one place that knows.
fn draw_marks(f: &mut Frame, app: &mut App, bands: &[crate::launcher::Band]) {
    if app.marked.is_empty() {
        return;
    }
    app.marked = crate::launcher::marked_on(app, bands);
    if app.marked.is_empty() {
        app.mark_anchor = None;
        return;
    }
    let th = app.theme;
    let fill = match dim_toward_black(th.accent, MARKED_TINT) {
        Color::Reset => th.sel_bg,
        c => c,
    };
    let buf = f.buffer_mut();
    for (rect, hit) in &app.hits {
        let sref = match *hit {
            HitTarget::LauncherCard(at) => {
                crate::launcher::card_at(bands, at).map(crate::launcher::Card::sref)
            }
            HitTarget::LauncherDrawerEntry(band, entry) => bands
                .get(band)
                .and_then(|b| b.drawer().get(entry))
                .map(|r| orion_core::SessionRef::Agent(r.agent.id.clone())),
            _ => None,
        };
        if !sref.is_some_and(|s| app.marked.contains(&s)) {
            continue;
        }
        let area = rect.intersection(buf.area);
        buf.set_style(area, Style::default().bg(fill));
        // One in: onto the air after an entry's cursor mark rather than
        // over it.
        if let Some(cell) = buf
            .cell_mut((area.x + 1, area.y))
            .filter(|_| area.width > 1)
        {
            cell.set_symbol(MARKED_MARK).set_fg(th.accent);
        }
    }
}

/// The scroll this frame draws the GRID at, settled from what the last
/// frame left and what has happened since — the rule
/// [`App::launcher_scroll`] spells out. A scroll the wheel set is kept unless a key
/// asked for the cursor's card ([`App::launcher_reveal`]) or the cursor
/// is on another entry than the one it was set under; any other scroll
/// keeps in step with the cursor — its entry with the line over it
/// ([`crate::launcher::PanelLayout::reveal`]). Whatever it lands on is
/// held within the panel, so a window that grew or a list that shrank
/// never scrolls past the last row.
fn settle_panel_scroll(
    app: &mut App,
    panel: &crate::launcher::PanelLayout,
    bands: &[crate::launcher::Band],
    cursor: Option<usize>,
) -> u16 {
    let mut scroll = panel.clamp(app.launcher_scroll);
    if let Some(index) = cursor {
        let band = &bands[index];
        let at = crate::launcher::card_cursor(app, band);
        // Off the cards, the cursor may be on a line of the band's
        // ARCHIVED DRAWER: that line is what to keep on screen.
        let drawer_at = at
            .is_none()
            .then(|| crate::launcher::drawer_cursor(app, band))
            .flatten();
        let on = match drawer_at {
            Some(e) => Some(orion_core::SessionRef::Agent(
                band.drawer()[e].agent.id.clone(),
            )),
            None => at.and_then(|i| band.cards.get(i)).map(|c| c.sref()),
        };
        if app.launcher_reveal || !app.launcher_scroll_held || app.launcher_scroll_on != on {
            scroll = match drawer_at {
                Some(e) => panel.reveal_drawer(scroll, index, e),
                None => panel.reveal(scroll, index, at),
            };
            app.launcher_scroll_held = false;
            app.launcher_scroll_on = on;
        }
    }
    app.launcher_reveal = false;
    app.launcher_scroll = scroll;
    scroll
}

/// What the header counts: the cards on the grid, by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HeadCount {
    sessions: usize,
    terminals: usize,
}

impl HeadCount {
    fn of(bands: &[crate::launcher::Band]) -> Self {
        Self {
            sessions: bands.iter().map(|b| b.sessions()).sum(),
            terminals: bands.iter().map(|b| b.terminals()).sum(),
        }
    }
}

/// What the ARCHIVED VIEW says with nothing in it.
const NO_ARCHIVED: &str = "nothing archived in this project — Esc back to the live sessions";

/// The header over the grid: the PROJECT TABS on the left, each with its
/// status dots, how many cards the grid holds on the right — and how many
/// of them it could not fit — a rule under both: the same three-row head
/// the panels' columns sit on.
///
/// The row is the project bar first: the tabs are laid out before
/// anything else and take what they need of it, and the counts on the
/// right get what they leave ([`head_count`]), giving way a piece at a
/// time — so a narrow window, or the PANE down the right of the cards,
/// costs the counts before it costs a project's name.
fn draw_head(f: &mut Frame, app: &mut App, body: Rect, count: HeadCount, hidden: Hidden) {
    let th = app.theme;
    if let Some(r) = row_rect(body, 1) {
        let r = pad_x(r);
        let tabs = head_tabs(app, r);
        let taken: usize = tabs.iter().map(|s| s.width()).sum();
        f.render_widget(Paragraph::new(Line::from(tabs)), r);
        let room = (r.width as usize).saturating_sub(taken + HEAD_GAP);
        let right = head_count(app, count, hidden, room, th);
        let used: usize = right.iter().map(|(s, _)| s.width()).sum();
        // The PR COUNT is a button, laid down where the right-aligned row
        // puts the word: a click opens that list for the project in front
        // of you, as `v` does.
        let mut x = r.x + (r.width as usize).saturating_sub(used) as u16;
        let mut spans = Vec::with_capacity(right.len());
        for (span, hit) in right {
            let width = span.width() as u16;
            if let Some(hit) = hit {
                app.hits.push((Rect { x, width, ..r }, hit));
            }
            x += width;
            spans.push(span);
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).alignment(ratatui::layout::Alignment::Right),
            r,
        );
    }
    draw_rule(f, body, 2, th.edge);
}

/// The header's PROJECT TABS: one tab per project opened since its tab
/// was last closed, behind a `+` for the rest, the most recently opened
/// next to it ([`crate::launcher::project_tabs`]) — where the `+` puts
/// the project it opens. The tab the grid is on is lit — a raised chip,
/// its name in the accent — and a click on any other opens it, as `[`
/// and `]` walking onto it do (`event_loop::launcher::open_tab`). The `×`
/// on a tab closes it
/// (`event_loop::launcher::close_tab`) — all but the last one, which is
/// the project on screen and has nowhere to hand the grid to; the `+`
/// drops the PROJECT DROPDOWN under itself, every project narrowed by
/// whatever you type plus a row that opens a folder, and the pick opens a
/// tab (`event_loop::launcher::open_project_menu`). A right-click on a
/// tab opens that project with its menu over it.
///
/// Each tab carries its project's STATUS MARKS after the name — waiting
/// on you, crashed, finished unread, working — each with its count and
/// no word ([`tab_dots`]), so a project that wants you says so from the
/// header, whichever project the grid is on. An unlit tab's name
/// shimmers while a finish there is unread, and sweeps red once when
/// something there starts needing you ([`tab_ramp`]), so the tab says it
/// in motion too; the lit tab holds still.
///
/// The hit rects are laid down as the spans are measured, so a click
/// lands on the tab itself and never on the air between two, and the
/// dropdown hangs off the `+`'s own rect.
///
/// With the keys up here (`k`,`k` off the top row of cards — see
/// [`App::launcher_tab_cursor`]) the header has a cursor of its own: the
/// tab it is on wears the accent as a solid block, the way a focused
/// title chip does, whichever tab is lit.
///
/// The tabs get the whole row ([`draw_head`]) and give way in steps, the
/// lit tab last of all ([`fit_tabs`]): first the `×` on every tab but the
/// lit one — the browser's own answer to a crowded tab strip, the lit tab
/// being the one a close is aimed at — then whole tabs, from the right,
/// into the MORE CHIP after the last one drawn (`2 more ▾`, [`more_chip`]),
/// which carries their STATUS DOTS and sweeps as a tab would, so a project
/// off the row that wants you still says so from the header. Its click
/// lists them (`event_loop::launcher::open_more_tabs_menu`). The lit tab
/// is always drawn, whole wherever there is room for it, so the header
/// always says which project the grid is on.
fn head_tabs(app: &mut App, r: Rect) -> Vec<Span<'static>> {
    let th = app.theme;
    let hover = app.hover_crumb.clone();
    let sweep = app.animations.then(|| app.sweep_phase());
    let tabs = crate::launcher::project_tabs(app);
    let room = r.width as usize;
    let add = if tabs.is_empty() { ADD_EMPTY } else { ADD };
    let add_w = add.chars().count() + 2;
    // The `+` is laid out first: it is the only way to a project with no
    // tab, so the tabs shrink around it rather than push it off the row.
    let budget = room.saturating_sub(add_w + 1);
    let chip =
        |tab: &ProjectTab, name_max: usize| project_chip(tab, name_max, hover.as_ref(), sweep, th);
    let sizes: Vec<TabSize> = tabs
        .iter()
        .map(|tab| {
            let [label, cross] = chip(tab, PROJECT_TAB_MAX);
            TabSize {
                label: label.width(),
                cross: cross.width(),
                name: tab.name.chars().count().min(PROJECT_TAB_MAX),
            }
        })
        .collect();
    let lit = tabs
        .iter()
        .position(|t| t.focused)
        .or_else(|| tabs.iter().position(|t| t.active));
    let fit = fit_tabs(&sizes, lit, budget, |more| {
        more_chip(more.len(), tally_of(&tabs, more), false, None, th).width()
    });
    app.launcher_tabs_more = fit.more.iter().map(|&i| tabs[i].id.clone()).collect();

    let mut tabs_row: Vec<PaneTab> = Vec::new();
    for (n, &i) in fit.shown.iter().enumerate() {
        if n > 0 {
            tabs_row.push(PaneTab::plain(vec![Span::raw(" ")]));
        }
        let is_lit = Some(i) == lit;
        let name_max = if is_lit {
            fit.lit_name
        } else {
            PROJECT_TAB_MAX
        };
        let [label, cross] = chip(&tabs[i], name_max);
        tabs_row.push(label);
        if fit.crosses || is_lit {
            tabs_row.push(cross);
        }
    }
    if !fit.more.is_empty() {
        if !tabs_row.is_empty() {
            tabs_row.push(PaneTab::plain(vec![Span::raw(" ")]));
        }
        let hovered = hover.as_ref() == Some(&HitTarget::LauncherTabMore);
        tabs_row.push(more_chip(
            fit.more.len(),
            tally_of(&tabs, &fit.more),
            hovered,
            sweep,
            th,
        ));
    }
    // The `+` follows the last tab (and the MORE CHIP), where a project
    // it opens lands: a new tab goes on the right end.
    let mut row = tabs_row;
    if add_w <= room {
        if !row.is_empty() {
            row.push(PaneTab::plain(vec![Span::raw(" ")]));
        }
        let mut style = Style::default().fg(th.muted);
        if hover.as_ref() == Some(&HitTarget::LauncherTabAdd) {
            style = Style::default()
                .fg(th.accent)
                .add_modifier(Modifier::UNDERLINED);
        }
        row.push(PaneTab {
            spans: vec![Span::raw(" "), Span::styled(add, style), Span::raw(" ")],
            hit: Some(HitTarget::LauncherTabAdd),
        });
    }

    let mut spans = Vec::new();
    let mut x = r.x;
    for tab in row {
        let width = u16::try_from(tab.width()).unwrap_or(u16::MAX);
        if let Some(hit) = tab.hit {
            app.hits.push((
                Rect {
                    x,
                    width,
                    height: 1,
                    ..r
                },
                hit,
            ));
        }
        x = x.saturating_add(width);
        spans.extend(tab.spans);
    }
    spans
}

/// The columns a PROJECT TAB takes, measured whole: its name and STATUS
/// DOTS (`label`), the `×` beside them (`cross`), and how much of the
/// label is the name itself (`name`) — what the lit tab can give up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TabSize {
    label: usize,
    cross: usize,
    name: usize,
}

/// How the PROJECT TABS share their row ([`fit_tabs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct TabFit {
    /// The tabs drawn, by index, in tab order.
    shown: Vec<usize>,
    /// Every tab drawn keeps its `×`, not just the lit one.
    crosses: bool,
    /// How much of the lit tab's name is drawn.
    lit_name: usize,
    /// The tabs the MORE CHIP stands in for, by index, in tab order.
    more: Vec<usize>,
}

/// Which PROJECT TABS the header draws in `budget` columns, each step
/// giving up less than the next: every tab with its `×`; every tab, the
/// `×` on the lit one only; the lit tab and as many of the rest as fit,
/// in tab order, with the MORE CHIP (`more_w` columns for the tabs it
/// holds) standing in for the others; the lit tab's name cut, down to
/// [`TAB_NAME_MIN`]; the chip alone, holding them all; and nothing. The
/// tabs keep their own order, so the lit tab drawn out of its place
/// sits where it falls among the ones kept, and the ones kept are the
/// first ones — the projects last worked in.
fn fit_tabs(
    sizes: &[TabSize],
    lit: Option<usize>,
    budget: usize,
    more_w: impl Fn(&[usize]) -> usize,
) -> TabFit {
    let all: Vec<usize> = (0..sizes.len()).collect();
    let lit_name = lit.map_or(0, |i| sizes[i].name);
    let row = |shown: &[usize], crosses: bool| -> usize {
        let tabs: usize = shown
            .iter()
            .map(|&i| {
                let cross = crosses || Some(i) == lit;
                sizes[i].label + if cross { sizes[i].cross } else { 0 }
            })
            .sum();
        tabs + shown.len().saturating_sub(1)
    };
    // The chip and the column of air before it; nothing with no chip.
    let chip = |more: &[usize]| {
        if more.is_empty() {
            0
        } else {
            1 + more_w(more)
        }
    };
    for crosses in [true, false] {
        if row(&all, crosses) <= budget {
            return TabFit {
                shown: all,
                crosses,
                lit_name,
                more: Vec::new(),
            };
        }
    }
    let others: Vec<usize> = all.iter().copied().filter(|&i| Some(i) != lit).collect();
    for kept in (0..others.len()).rev() {
        let mut shown = others[..kept].to_vec();
        shown.extend(lit);
        shown.sort_unstable();
        let more = others[kept..].to_vec();
        if row(&shown, false) + chip(&more) <= budget {
            return TabFit {
                shown,
                crosses: false,
                lit_name,
                more,
            };
        }
    }
    if let Some(i) = lit {
        let over = (row(&[i], false) + chip(&others)).saturating_sub(budget);
        if lit_name >= TAB_NAME_MIN + over {
            return TabFit {
                shown: vec![i],
                crosses: false,
                lit_name: lit_name - over,
                more: others,
            };
        }
    }
    let more = if !all.is_empty() && more_w(&all) <= budget {
        all
    } else {
        Vec::new()
    };
    TabFit {
        shown: Vec::new(),
        crosses: false,
        lit_name,
        more,
    }
}

/// The STATUS DOTS the tabs at `ids` would carry between them.
fn tally_of(tabs: &[ProjectTab], ids: &[usize]) -> Tally {
    ids.iter()
        .fold(Tally::default(), |sum, &i| sum.plus(tabs[i].tally))
}

/// What the MORE CHIP says it drops, after the count.
const MORE_CARET: &str = " ▾ ";

/// The MORE CHIP: ` 2 more ●1 ▾ ` — how many PROJECT TABS the row had no
/// room for, the STATUS MARKS they carry between them, and the caret that
/// says a click lists them. Its words sweep as a tab's name does
/// ([`tab_ramp`]), so a project off the row with a finish nobody has read
/// is still seen moving in the header. Underlined under the pointer, as
/// the tabs are.
fn more_chip(
    count: usize,
    tally: Tally,
    hovered: bool,
    sweep: Option<usize>,
    th: Theme,
) -> PaneTab {
    let mut words = Style::default().fg(th.muted);
    if hovered {
        words = words.add_modifier(Modifier::UNDERLINED);
    }
    let ramp = sweep.and_then(|_| tab_ramp(tally, th));
    let mut spans = vec![Span::raw(" ")];
    spans.extend(status_name_spans(
        format!("{count} more"),
        words,
        ramp,
        sweep.unwrap_or(0),
    ));
    spans.extend(tab_dots(tally, sweep, th));
    spans.push(Span::styled(MORE_CARET, Style::default().fg(th.dim)));
    PaneTab {
        spans,
        hit: Some(HitTarget::LauncherTabMore),
    }
}

/// One PROJECT TAB, as two hits side by side: the tab — its name and its
/// STATUS MARKS — and the `×` that closes it, a target of its own so a
/// click on the cross never reads as a click on the tab. The lit tab is a
/// raised chip, pads and all, with its name in the accent; the rest sit
/// flat — the name bright while something in the project wants you, dim
/// while nothing does. The name is cut to `name_max`. Every tab closes,
/// the last one included: that one closes to the splash.
///
/// `sweep` is the frame's sweep phase, `None` with the animations off:
/// with it, an unlit tab's name sweeps while its project has a finish
/// nobody has read, or something there just started needing you
/// ([`tab_ramp`]) — the tab is what says so from another project. The lit
/// tab never sweeps: its accent is what says where you are, and the rows
/// under it already say the rest. Nor does the header's cursor, so the
/// block that says where Enter goes stays legible.
fn project_chip(
    tab: &ProjectTab,
    name_max: usize,
    hover: Option<&HitTarget>,
    sweep: Option<usize>,
    th: Theme,
) -> [PaneTab; 2] {
    let fill = |style: Style| {
        if tab.active {
            style.bg(th.sel_bg)
        } else {
            style
        }
    };
    let wants = tab.tally.wants_you();
    let mut name = if tab.active {
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD)
    } else if wants {
        Style::default().fg(th.text)
    } else {
        Style::default().fg(th.dim)
    };
    if hover == Some(&HitTarget::LauncherTab(tab.id.clone())) {
        name = name.add_modifier(Modifier::UNDERLINED);
    }
    // The header's own cursor: the pad and the name as one accent block,
    // so where Enter would go reads apart from which tab is lit.
    let (pad, name) = if tab.focused {
        let cursor = Style::default().bg(th.accent).fg(th.on_accent);
        (cursor, cursor.add_modifier(Modifier::BOLD))
    } else {
        (fill(Style::default()), fill(name))
    };
    let (ramp, phase) = match sweep {
        Some(phase) if !tab.focused && !tab.active => (tab_ramp(tab.tally, th), phase),
        _ => (None, 0),
    };
    let mut label = vec![Span::styled(" ", pad)];
    label.extend(status_name_spans(
        truncate(&tab.name, name_max),
        name,
        ramp,
        phase,
    ));
    label.extend(
        tab_dots(tab.tally, sweep, th)
            .into_iter()
            .map(|dot| Span::styled(dot.content, fill(dot.style))),
    );
    let cross = if hover == Some(&HitTarget::LauncherTabClose(tab.id.clone())) {
        th.err
    } else if tab.active {
        th.muted
    } else {
        th.faint
    };
    [
        PaneTab {
            spans: label,
            hit: Some(HitTarget::LauncherTab(tab.id.clone())),
        },
        PaneTab {
            spans: vec![Span::styled(" × ", fill(Style::default().fg(cross)))],
            hit: Some(HitTarget::LauncherTabClose(tab.id.clone())),
        },
    ]
}

/// A PROJECT TAB's STATUS MARKS: one per state its sessions are in, each
/// the mark a row in that state wears, carrying the state's count and no
/// word at all, so the header is read at a glance rather than parsed. In
/// the attention order — waiting on you (`●`, crimson), crashed (`✕`),
/// finished unread (`●`, the done color), then working (the gold
/// spinner, turning with `sweep`). A state with nothing in it is left
/// out, so a quiet project is its bare name — and because the order is
/// fixed, the marks that are there never move as the work under them does.
fn tab_dots(tally: Tally, sweep: Option<usize>, th: Theme) -> Vec<Span<'static>> {
    use orion_core::AgentStatus;
    let spin = sweep.map(crate::app::spin_step);
    [
        (tally.needs_you, Some(AgentStatus::NeedsFeedback), false),
        (tally.failed, Some(AgentStatus::Terminated), false),
        (tally.done, Some(AgentStatus::Finished), true),
        (tally.running, Some(AgentStatus::Running), false),
    ]
    .into_iter()
    .filter(|(n, _, _)| *n > 0)
    .map(|(n, status, unseen)| {
        let mark = status_dot(status, unseen, spin, th);
        Span::styled(format!(" {}{n}", mark.content.trim_end()), mark.style)
    })
    .collect()
}

/// The ramp an unlit PROJECT TAB's name sweeps on: the red ONE-SHOT SWEEP
/// for the few seconds after something there starts needing you or
/// crashes, and otherwise the UNREAD SHIMMER for as long as a finish
/// there is left unread — the tab is how a finish in a project you are
/// not on gets noticed at all, so it keeps moving until you look. A
/// project with neither holds still, running or not: work in progress is
/// the spinner's to say.
fn tab_ramp(tally: Tally, th: Theme) -> Option<[Color; 3]> {
    if tally.alarm {
        Some(th.err_sweep)
    } else if tally.done > 0 {
        Some(th.done_sweep)
    } else {
        None
    }
}

/// The header's right side: how many cards the grid holds, and how many
/// of them are off screen. Which of them want something is the STATUS
/// DOTS' business on the PROJECT TABS, told in dots rather than in a
/// second sentence.
///
/// It gets what the tabs leave of the row ([`draw_head`]), and what does
/// not fit in `width` gives way whole, least needed first: the count of
/// the cards (each band's rule says its own), then the PR COUNT (`v`
/// says it again), and the HIDDEN MARKER last — a
/// screenful of cards with more behind it looks exactly like a project
/// with that many sessions in it, and the PANE dragged up over the grid
/// is the usual way of getting there. The row never runs past `width`.
///
/// Each span comes with the button it is, if any: only the two counts are,
/// and [`draw_head`] lays their hit rects where the row lands them.
fn head_count(
    app: &App,
    count: HeadCount,
    hidden: Hidden,
    width: usize,
    th: Theme,
) -> Vec<(Span<'static>, Option<HitTarget>)> {
    type Part = Vec<(Span<'static>, Option<HitTarget>)>;
    let words: Part = vec![(
        Span::styled(count_words(app, count), Style::default().fg(th.dim)),
        None,
    )];
    // The PR COUNT the PROJECTS PANEL's rows used to carry: how many open
    // pull requests the project in front of you has, so the number is
    // read without opening `v` to find it — and a click on it opens that
    // list, the pointer's way to the same modal. A second column of air
    // sets it off from the count.
    let badge: Part = crate::ui::open_prs_badge(app.listed_open_prs().len(), th)
        .map(|parts| {
            let parts = parts.into_iter().map(|(text, mut style, hit)| {
                // Nothing about a word says it is a button, so the one
                // under the pointer is underlined, as the header's tabs
                // are.
                if hit.is_some() && app.hover_crumb == hit {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                (Span::styled(text, style), hit)
            });
            std::iter::once((Span::raw(" "), None))
                .chain(parts)
                .collect()
        })
        .unwrap_or_default();
    let mark: Part = hidden_mark(hidden, th)
        .into_iter()
        .map(|s| (s, None))
        .collect();
    // Once a piece has not fitted, nothing after it is tried: a narrower
    // piece slipping into the room a wider one left would swap what the
    // row says as the window narrows, rather than only take it away.
    let mut left = Some(width);
    let mut keep = |part: Part| -> Part {
        let w: usize = part.iter().map(|(s, _)| s.width()).sum();
        match left {
            Some(room) if w <= room => {
                left = Some(room - w);
                part
            }
            _ => {
                left = None;
                Vec::new()
            }
        }
    };
    let mark = keep(mark);
    let badge = keep(badge);
    let words = keep(words);
    words.into_iter().chain(badge).chain(mark).collect()
}

/// The header's count of what the grid holds: `3 sessions · 2 terminals`
/// — the terminals only when there are any, since most projects have
/// none — under the ARCHIVED VIEW's own word there, so the header says
/// which of the two lists is on screen without a second line to read.
fn count_words(app: &App, count: HeadCount) -> String {
    let noun = if app.show_archived {
        "archived session"
    } else {
        "session"
    };
    let mut out = format!("{} {noun}{}", count.sessions, plural(count.sessions));
    if count.terminals > 0 {
        out.push_str(&format!(
            " · {} terminal{}",
            count.terminals,
            plural(count.terminals)
        ));
    }
    out
}

/// The HIDDEN MARKER: how many bands — or, inside a worktree, cards — the
/// grid holds that are not on screen, with an arrow saying which way
/// they went — `↓` past the bottom edge, `↑` scrolled off the top, `↑↓`
/// both. Nothing at all when the window holds the lot, so a grid that
/// fits reads exactly as it does today.
///
/// This is the row's answer to a PANE dragged up over the cards: the
/// count beside it still says how many sessions the project has, and this
/// says how many of them the room left can show — so a card that went
/// missing reads as the pane taking its room rather than as the session
/// disappearing, and the arrow says whether the way back to it is a step
/// down through the grid or the pane's own edge dragged back.
fn hidden_mark(hidden: Hidden, th: Theme) -> Vec<Span<'static>> {
    let total = hidden.total();
    if total == 0 {
        return Vec::new();
    }
    let arrow = match (hidden.above > 0, hidden.below > 0) {
        (true, true) => "↑↓",
        (true, false) => "↑",
        _ => "↓",
    };
    vec![Span::styled(
        format!("  {arrow} {total} hidden"),
        Style::default().fg(th.muted),
    )]
}

pub(super) fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// The rule under a header row.
fn draw_rule(f: &mut Frame, area: Rect, row: usize, color: Color) {
    if let Some(r) = row_rect(area, row) {
        f.render_widget(
            Paragraph::new(Span::styled(
                "─".repeat(r.width as usize),
                Style::default().fg(color),
            )),
            r,
        );
    }
}

/// The header's own margin, so its text lines up with the cards' left
/// edge rather than hugging the screen.
fn pad_x(r: Rect) -> Rect {
    let pad = crate::launcher::PAD_X;
    Rect {
        x: r.x + pad,
        width: r.width.saturating_sub(pad * 2),
        ..r
    }
}

/// The GRID: one band per checkout, top to bottom — its rule and its
/// entries a line apiece under it — the whole panel scrolled by rows so
/// the cursor's entry is always drawn.
fn draw_bands(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    panel: &crate::launcher::PanelLayout,
    bands: &[crate::launcher::Band],
    cursor: Option<usize>,
    scroll: u16,
) {
    let th = app.theme;
    let window = panel.window();
    // One CONFIG.JSON read for the whole frame, and only if some entry on
    // it runs a CUSTOM harness whose label lives in there — a screenful
    // of sessions must not reload the file once per row.
    let mut cfg = None;
    // The keys are up on the PROJECT TABS, or down in the pane: the band
    // under the cursor keeps its bold branch — it still says which
    // checkout the pane reads — but the accent goes with the keys, so
    // only the one thing holding them is lit.
    let keys = app.focus != Focus::Terminal && app.launcher_tab_cursor.is_none();
    // The band the selection is on, aimed at or let go of: its lines
    // follow its remembered entry either way, so Esc scrolls nothing.
    let aimed = crate::launcher::band_cursor(app, bands);
    // Each band's whole rectangle, rule to last line: a click on the air
    // around its entries picks the band as a click on its rule does.
    // Pushed after every entry and rule so they keep their own targets
    // under `hit_at`'s first-match scan.
    let mut band_areas = Vec::with_capacity(bands.len());
    let facts: Vec<RuleFacts> = bands.iter().map(|b| rule_facts(app, b, th)).collect();
    let cols = RuleColumns::of(&facts);
    if let Some(y) = panel.divider_y {
        let line = Rect {
            y,
            height: 1,
            ..area
        };
        if let Some(placed) = crate::launcher::place(window, scroll, line) {
            draw_dormant_divider(f.buffer_mut(), placed.rect, th);
        }
    }
    for (index, band) in bands.iter().enumerate() {
        let pb = &panel.bands[index];
        let whole = Rect {
            y: pb.rule_y,
            height: pb.height,
            ..area
        };
        if let Some(placed) = crate::launcher::place(window, scroll, whole) {
            band_areas.push((placed.rect, HitTarget::LauncherBand(index)));
        }
        let on = cursor == Some(index);
        let at = (aimed == Some(index))
            .then(|| crate::launcher::card_cursor(app, band))
            .flatten();
        let rule = Rect {
            y: pb.rule_y,
            height: crate::launcher::BAND_RULE_H,
            ..area
        };
        // A one-row rule is on screen whole or not at all
        // ([`crate::launcher::place`]).
        if let Some(placed) = crate::launcher::place(window, scroll, rule) {
            let hits = draw_band_rule(
                f.buffer_mut(),
                app,
                placed.rect,
                band,
                &facts[index],
                BandRule {
                    index,
                    on,
                    lit: on && keys,
                    cols,
                },
            );
            app.hits.extend(hits);
        }
        // The ARCHIVED DRAWER under everything else the band draws: its
        // `▸ N archived` line, and its sessions a faint line apiece once
        // unfolded.
        draw_drawer(
            f,
            app,
            area,
            pb,
            DrawerBand {
                index,
                band,
                window,
                scroll,
                at: on
                    .then(|| crate::launcher::drawer_cursor(app, band))
                    .flatten(),
                lit: on && keys,
            },
            &mut cfg,
        );
        if band.cards.is_empty() {
            draw_empty_band(f, app, area, pb, (window, scroll), on && keys);
            continue;
        }
        // The entries under the rule: on the band the cursor is on, the
        // one it remembers — what the pane reads, and what the keys walk
        // — wears the cursor. A click on any lands the cursor on it
        // (`HitTarget::LauncherCard`).
        draw_list_band(
            f,
            app,
            pb,
            ListBand {
                index,
                band,
                window,
                scroll,
                on,
                at,
            },
            &mut cfg,
        );
    }
    draw_panel_edge_marks(f, panel, scroll, th);
    app.hits.extend(band_areas);
    // Last, so the bands themselves win `hit_at`'s first-match scan and
    // only the air between them falls through to the grid.
    app.hits.push((area, HitTarget::PanelBg(Focus::Sessions)));
}

/// What an EMPTY BAND says under its rule, before the ways to start
/// something there ([`empty_band_hint`]).
const EMPTY_BAND: &str = "nothing running";

/// What the DORMANT DIVIDER says, ahead of the dashes that fill its row.
const DORMANT_LABEL: &str = "┄┄ dormant ";

/// The DORMANT DIVIDER: one faint `┄┄ dormant ┄┄┄…` line across the grid,
/// over the checkouts whose sessions have all gone cold. Nothing to click.
fn draw_dormant_divider(buf: &mut Buffer, r: Rect, th: Theme) {
    let width = r.width as usize;
    if width < DORMANT_LABEL.chars().count() {
        return;
    }
    let text: String = DORMANT_LABEL
        .chars()
        .chain(std::iter::repeat('┄'))
        .take(width)
        .collect();
    buf.set_string(r.x, r.y, text, Style::default().fg(th.faint));
}

/// What an EMPTY BAND's line says in `width` columns: [`EMPTY_BAND`],
/// then the keys that start something there, from the live keymap —
/// `nothing running · ⌘N new agent · ⌘⇧N new agent in a new worktree · t
/// terminal` — an unbound one left out. Short of room the longest gives
/// way first, whole, so the line never ends mid-word.
fn empty_band_hint(app: &App, width: usize) -> String {
    let mut ways: Vec<String> = [
        (Action::QuickPrompt, "new agent"),
        (
            Action::QuickPromptNewWorktree,
            "new agent in a new worktree",
        ),
        (Action::NewTerminal, "terminal"),
    ]
    .iter()
    .filter_map(|(action, does)| {
        crate::hints::key(&app.keymap, *action).map(|key| format!("{key} {does}"))
    })
    .collect();
    loop {
        let line = std::iter::once(EMPTY_BAND.to_string())
            .chain(ways.iter().cloned())
            .collect::<Vec<_>>()
            .join(" · ");
        let longest = (0..ways.len()).max_by_key(|&i| ways[i].chars().count());
        match longest {
            Some(i) if line.chars().count() > width => {
                ways.remove(i);
            }
            _ => return truncate(&line, width),
        }
    }
}

/// The line under an EMPTY BAND's rule — a checkout with nothing running
/// in it — in place of the row
/// of cards it has none of: [`empty_band_hint`], dim, and in the accent
/// while the cursor is on the band with the keys on the grid. A click on
/// it is a click on the band (the band's whole area).
fn draw_empty_band(
    f: &mut Frame,
    app: &App,
    area: Rect,
    pb: &crate::launcher::PanelBand,
    (window, scroll): (Rect, u16),
    lit: bool,
) {
    let th = app.theme;
    let row = Rect {
        y: pb.rule_y + crate::launcher::BAND_RULE_H,
        height: crate::launcher::EMPTY_BAND_ROW_H,
        ..area
    };
    let Some(placed) = crate::launcher::place(window, scroll, row) else {
        return;
    };
    let fg = if lit { th.accent } else { th.dim };
    let indent = "  ";
    let room = usize::from(placed.rect.width).saturating_sub(indent.len());
    let hint = format!("{indent}{}", empty_band_hint(app, room));
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(fg)))),
        placed.rect,
    );
}

/// The EDGE MARKERS on a grid taller than its window: `↑ 2 more above`
/// on the header's row of air over the entries, `↓ 3 more below` on the
/// row kept under them ([`crate::launcher::PanelLayout::window`]) —
/// each where the eye looks for the rest, so the grid says it scrolls,
/// and which way, without a word painted over an entry. The header's
/// `↑↓ 5 hidden` says the same once more. A grid that fits has neither
/// row marked, and a panel scrolled to an end leaves that end's row as
/// plain air.
fn draw_panel_edge_marks(
    f: &mut Frame,
    panel: &crate::launcher::PanelLayout,
    scroll: u16,
    th: Theme,
) {
    if !panel.overflows() {
        return;
    }
    let hidden = panel.hidden(scroll);
    let mark = |f: &mut Frame, r: Rect, words: String| {
        f.render_widget(
            Paragraph::new(Span::styled(words, Style::default().fg(th.muted)))
                .alignment(ratatui::layout::Alignment::Center),
            r,
        );
    };
    if hidden.above > 0 {
        let r = Rect {
            y: panel.area.y.saturating_sub(1),
            height: 1,
            ..panel.area
        };
        let n = hidden.above;
        mark(f, r, format!("↑ {n} more above"));
    }
    if hidden.below > 0 {
        let window = panel.window();
        let r = Rect {
            y: window.y + window.height,
            height: crate::launcher::BELOW_MARK_H,
            ..panel.area
        };
        let n = hidden.below;
        mark(f, r, format!("↓ {n} more below"));
    }
}

/// One band's ARCHIVED DRAWER as [`draw_drawer`] draws it.
struct DrawerBand<'a> {
    index: usize,
    band: &'a crate::launcher::Band,
    window: Rect,
    scroll: u16,
    /// The drawer line the cursor is on, when it is on one.
    at: Option<usize>,
    /// The cursor is on the band, with the keys on the grid.
    lit: bool,
}

/// Columns an ARCHIVED DRAWER is set in from the band's edge, so its
/// lines read as filed under the band rather than as a band of their own.
const DRAWER_INDENT: u16 = 2;

/// A band's ARCHIVED DRAWER: its `▸ 3 archived` line — `▾` once
/// unfolded, naming the key that folds it while the band holds the
/// cursor — and, unfolded, each archived session on a line of its own,
/// drawn as the compact LIST draws an entry ([`draw_list_row`]) in an
/// archived session's quiet colors, so the drawer reads as put away
/// before a word of it is. A click on the first line folds or unfolds
/// it (`HitTarget::LauncherDrawer`); on a session's, lands the cursor
/// there (`HitTarget::LauncherDrawerEntry`).
fn draw_drawer(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    pb: &crate::launcher::PanelBand,
    d: DrawerBand<'_>,
    cfg: &mut Option<crate::config::Config>,
) {
    let DrawerBand {
        index,
        band,
        window,
        scroll,
        at,
        lit,
    } = d;
    if band.archived.is_empty() {
        return;
    }
    let th = app.theme;
    let inset = Rect {
        x: area.x + DRAWER_INDENT.min(area.width),
        width: area.width.saturating_sub(DRAWER_INDENT),
        ..area
    };
    if let Some(placed) = crate::launcher::place(window, scroll, pb.drawer_head(inset)) {
        let hit = HitTarget::LauncherDrawer(index);
        let hovered = app.hover_crumb.as_ref() == Some(&hit);
        let fold = crate::ui::fold_mark(band.drawer_open);
        let mut spans = vec![Span::styled(
            format!("{fold} {} archived", band.archived.len()),
            Style::default().fg(if hovered { th.accent } else { th.dim }),
        )];
        if lit {
            let key = crate::hints::key_or(&app.keymap, Action::ToggleArchivedDrawer, "");
            if !key.is_empty() {
                let does = if band.drawer_open { "hide" } else { "show" };
                spans.push(Span::styled(" · ", Style::default().fg(th.dim)));
                spans.push(Span::styled(
                    key,
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    format!(" {does}"),
                    Style::default().fg(th.dim),
                ));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), placed.rect);
        app.hits.push((placed.rect, hit));
    }
    let drawer = band.drawer();
    if drawer.is_empty() {
        return;
    }
    let cards: Vec<crate::launcher::Card> = drawer
        .iter()
        .cloned()
        .map(crate::launcher::Card::Session)
        .collect();
    let mut cols = (0, 0);
    for row in drawer {
        cols = (
            cols.0.max(row.agent.name.chars().count()),
            cols.1.max(list_runs_line(&row.agent, cfg).chars().count()),
        );
    }
    let cols = (cols.0.min(LIST_NAME_MAX), cols.1.min(LIST_RUNS_MAX));
    for (i, card) in cards.iter().enumerate() {
        let Some(placed) = crate::launcher::place(window, scroll, pb.drawer_row(inset, i)) else {
            continue;
        };
        let links = draw_list_row(
            f.buffer_mut(),
            app,
            placed.rect,
            card,
            at == Some(i),
            cols,
            cfg,
        );
        app.hits.extend(links);
        app.hits
            .push((placed.rect, HitTarget::LauncherDrawerEntry(index, i)));
    }
}

/// What the cursor's LIST entry wears in front of it.
const LIST_MARK: &str = "▌ ";
/// Columns a LIST entry spends before its name: the cursor mark and the
/// status dot (or a terminal's glyph), two apiece.
const LIST_LEAD: u16 = 4;
/// Longest a LIST's name column grows, and its runs-on column: past these
/// the entries truncate rather than push the prompt off the line.
const LIST_NAME_MAX: usize = 28;
const LIST_RUNS_MAX: usize = 24;
/// Air between a LIST entry's columns.
const LIST_GAP: usize = 2;

/// One band of the LIST as [`draw_list_band`] draws it.
struct ListBand<'a> {
    index: usize,
    band: &'a crate::launcher::Band,
    window: Rect,
    scroll: u16,
    /// The cursor is on the band, and `at` is its card.
    on: bool,
    at: Option<usize>,
}

/// A band in the LIST: under its rule, one line per entry
/// (`launcher::list_layout`), the names and what each runs on in columns
/// shared down the band. A click on a line lands the cursor on its entry
/// (`HitTarget::LauncherCard`).
fn draw_list_band(
    f: &mut Frame,
    app: &mut App,
    pb: &crate::launcher::PanelBand,
    list: ListBand<'_>,
    cfg: &mut Option<crate::config::Config>,
) {
    let ListBand {
        index,
        band,
        window,
        scroll,
        on,
        at,
    } = list;
    let content = &pb.content;
    let shown: Vec<usize> = content
        .rows
        .iter()
        .filter_map(|r| r.first().copied())
        .collect();
    let mut cols = (0, 0);
    for &i in &shown {
        let (name, runs) = match &band.cards[i] {
            crate::launcher::Card::Session(row) => (
                row.agent.name.chars().count(),
                list_runs_line(&row.agent, cfg).chars().count(),
            ),
            crate::launcher::Card::Terminal(t) => {
                (t.name.chars().count(), terminal_runs(t).chars().count())
            }
        };
        cols = (cols.0.max(name), cols.1.max(runs));
    }
    let cols = (cols.0.min(LIST_NAME_MAX), cols.1.min(LIST_RUNS_MAX));
    for &i in &shown {
        let Some(placed) = pb
            .cell(i)
            .and_then(|cell| crate::launcher::place(window, scroll, cell))
        else {
            continue;
        };
        let links = draw_list_row(
            f.buffer_mut(),
            app,
            placed.rect,
            &band.cards[i],
            on && at == Some(i),
            cols,
            cfg,
        );
        // A run line's ports and a session's ACCOUNT CHIP are links, ahead
        // of the line so they win.
        app.hits.extend(links);
        app.hits.push((
            placed.rect,
            HitTarget::LauncherCard(crate::launcher::CardRef {
                band: index,
                card: i,
            }),
        ));
    }
}

/// One entry of the LIST, on one line: the cursor mark, the status dot
/// and name, what it runs on, the last thing it was asked, and how long
/// since it moved at the right. A shell runs on `shell`; a RUN LINE
/// leaves that column blank, puts its command where the prompts go and
/// its status where the ages go ([`terminal_badge`]). `cols` are the
/// band's name and runs-on column widths, so the entries line up. The
/// cursor's entry (`selected`) wears the mark ([`LIST_MARK`]) and a wash
/// of its status color across the line ([`card_tint`]). A run line's
/// ports follow its command, underlined. The links on the line come back
/// with their cells — each port, and a session's ACCOUNT CHIP — for the
/// pointer and the clicks that open them.
fn draw_list_row(
    buf: &mut Buffer,
    app: &App,
    r: Rect,
    card: &crate::launcher::Card,
    selected: bool,
    (name_col, runs_col): (usize, usize),
    cfg: &mut Option<crate::config::Config>,
) -> Vec<(Rect, HitTarget)> {
    let th = app.theme;
    let width = usize::from(r.width);
    // A bar rather than the rule's `❯`: a shell's own glyph is `❯`, one
    // column over, and the two would read as one mark.
    let mark = if selected {
        Span::styled(LIST_MARK, Style::default().fg(th.accent))
    } else {
        Span::raw("  ")
    };
    struct Entry<'a> {
        lead: Span<'static>,
        /// The ACCOUNT CHIP, and the account it names.
        tag: Option<(Span<'static>, String)>,
        name: String,
        name_style: Style,
        ramp: Option<[Color; 3]>,
        runs: String,
        runs_style: Style,
        text_mark: &'static str,
        text: String,
        text_style: Style,
        /// A run line's ports, drawn after its command (`text`).
        ports: &'a [u16],
        /// The todo a session was sent at, `☐ text`, dim after its prompt.
        todo: Option<String>,
        badge: String,
        badge_style: Style,
        /// The color the cursor's wash takes: the status's while it
        /// wants you ([`card_edge`]), else none — the accent's.
        status: Option<Color>,
    }
    let e = match card {
        crate::launcher::Card::Session(row) => {
            let a = &row.agent;
            let look = session_look(app, a, selected, th);
            let quiet_or = |live: Color| if a.archived { look.quiet } else { live };
            // A session stopped on a usage limit says what Claude said
            // about it where the prompt goes — its badge already names it.
            let (text_mark, text) = match a.limit_reached().and_then(|l| l.message.as_deref()) {
                Some(message) => ("", message.to_string()),
                None => (
                    "› ",
                    crate::launcher::last_prompt(a)
                        .unwrap_or_default()
                        .to_string(),
                ),
            };
            Entry {
                lead: look.dot,
                tag: account_chip(a, look.quiet, th).map(|chip| {
                    let id = orion_core::harness::registry_id(a.kind, a.custom_harness.as_deref());
                    (chip, id.to_string())
                }),
                name: a.name.clone(),
                name_style: look.name_style,
                ramp: look.ramp,
                runs: list_runs_line(a, cfg),
                runs_style: Style::default().fg(quiet_or(th.dim)),
                text_mark,
                text,
                text_style: Style::default().fg(look.prompt),
                ports: &[],
                todo: crate::todos::of_agent(app, &a.id)
                    .map(|todo| format!("{} {}", crate::todos::mark(todo), todo.text)),
                badge: look.ago,
                badge_style: look.ago_style,
                status: card_edge(
                    a,
                    a.archived || app.is_placeholder_agent(&a.id) || crate::app::drawn_cold(a),
                    th,
                ),
            }
        }
        crate::launcher::Card::Terminal(t) => {
            let (badge, badge_style) = terminal_badge(t, crate::app::now_ms(), th);
            Entry {
                lead: terminal_mark(t, th),
                tag: None,
                name: t.name.clone(),
                name_style: Style::default().fg(th.text).add_modifier(Modifier::BOLD),
                ramp: None,
                runs: terminal_runs(t).into(),
                runs_style: Style::default().fg(th.dim),
                text_mark: if t.run_command.is_some() {
                    RUN_MARK
                } else {
                    ""
                },
                text: t.run_command.clone().unwrap_or_default(),
                text_style: Style::default().fg(th.muted),
                ports: &t.run.ports,
                todo: None,
                badge,
                badge_style,
                status: None,
            }
        }
    };
    let mut spans = vec![mark, e.lead];
    let mut room = width.saturating_sub(usize::from(LIST_LEAD));
    let mut links = Vec::new();
    // The ACCOUNT CHIP column, on every row while the machine has more
    // than one Claude account — blank on a row that runs on none — so
    // the names after it line up down the whole grid.
    if crate::claude_accounts::any_tags() && room > CHIP_COLUMN_W + 8 {
        match e.tag.clone() {
            Some((chip, id)) => {
                let x = r.x + LIST_LEAD;
                links.push((
                    Rect::new(x, r.y, CHIP_W as u16, 1),
                    HitTarget::AccountTag(id),
                ));
                spans.push(chip);
            }
            None => spans.push(Span::raw(" ".repeat(CHIP_W))),
        }
        spans.push(Span::raw(" ".repeat(CHIP_COLUMN_W - CHIP_W)));
        room -= CHIP_COLUMN_W;
    }
    // Where the name starts: every offset below (`used`) counts from it.
    let x0 = r.x + (width - room) as u16;
    // The badge at the right end, while the name keeps a few letters.
    let badge = e.badge.trim().to_string();
    let badge_w = badge.chars().count();
    let keep_badge = badge_w > 0 && room >= badge_w + 1 + 8;
    if keep_badge {
        room -= badge_w + 1;
    }
    let name_w = name_col.max(1).min(room);
    let name = truncate(&e.name, name_w);
    let mut used = name.chars().count();
    spans.extend(status_name_spans(
        name,
        e.name_style,
        e.ramp,
        app.sweep_phase(),
    ));
    // What it runs on, in the band's column, and the prompt after it —
    // each only where there is room for a few letters of it. A RUN LINE
    // has nothing in the first, and its command in the second.
    let runs_at = name_w + LIST_GAP;
    if room >= runs_at + 6 {
        let runs_w = runs_col.min(room - runs_at);
        if !e.runs.is_empty() {
            let runs = truncate(&e.runs, runs_w);
            spans.push(Span::raw(" ".repeat(runs_at - used)));
            used = runs_at + runs.chars().count();
            spans.push(Span::styled(runs, e.runs_style));
        }
        let text_at = runs_at + runs_w + LIST_GAP;
        let text_mark_w = e.text_mark.chars().count();
        if !e.text.is_empty() && room >= text_at + text_mark_w + 6 {
            // The ports keep their room — `  :3000 :5173`, two cells
            // before the first and one between — and the command gives way.
            let labels: Vec<String> = e.ports.iter().map(|p| format!(":{p}")).collect();
            let ports_w: usize = labels.iter().map(|l| l.chars().count() + 1).sum::<usize>() + 1;
            let ports_fit = !labels.is_empty() && room >= text_at + text_mark_w + 6 + ports_w;
            let text_room = room - text_at - text_mark_w - if ports_fit { ports_w } else { 0 };
            // The todo the session came from keeps up to half the prompt's
            // room, where that half holds a few letters of it.
            const TODO_MIN: usize = 6;
            let todo_w = e.todo.as_ref().map_or(0, |t| t.chars().count() + LIST_GAP);
            let keep = if text_room / 2 >= LIST_GAP + TODO_MIN {
                todo_w.min(text_room / 2)
            } else {
                0
            };
            let text = truncate(&e.text, text_room - keep);
            spans.push(Span::raw(" ".repeat(text_at - used)));
            spans.push(Span::styled(e.text_mark, Style::default().fg(th.dim)));
            used = text_at + text_mark_w + text.chars().count();
            let todo_room = text_room.saturating_sub(text.chars().count() + LIST_GAP);
            spans.push(Span::styled(text, e.text_style));
            if let Some(todo) = e.todo.as_ref().filter(|_| todo_room >= TODO_MIN) {
                let todo = truncate(todo, todo_room);
                spans.push(Span::raw(" ".repeat(LIST_GAP)));
                used += LIST_GAP + todo.chars().count();
                spans.push(Span::styled(todo, Style::default().fg(th.dim)));
            }
            // Underlined links, the one under the pointer lit.
            let hovered = match (&app.hover_crumb, card) {
                (Some(HitTarget::RunPort(id, port)), crate::launcher::Card::Terminal(t))
                    if *id == t.id =>
                {
                    Some(*port)
                }
                _ => None,
            };
            let shown: &[u16] = if ports_fit { e.ports } else { &[] };
            for (i, (&port, label)) in shown.iter().zip(labels).enumerate() {
                let gap = if i == 0 { 2 } else { 1 };
                spans.push(Span::raw(" ".repeat(gap)));
                used += gap;
                let w = label.chars().count();
                if let crate::launcher::Card::Terminal(t) = card {
                    links.push((
                        Rect::new(x0 + used as u16, r.y, w as u16, 1),
                        HitTarget::RunPort(t.id.clone(), port),
                    ));
                }
                let fg = if hovered == Some(port) {
                    th.accent
                } else {
                    th.text
                };
                let style = Style::default().fg(fg).add_modifier(Modifier::UNDERLINED);
                spans.push(Span::styled(label, style));
                used += w;
            }
        }
    }
    if keep_badge {
        spans.push(Span::raw(" ".repeat(room.saturating_sub(used) + 1)));
        spans.push(Span::styled(badge, e.badge_style));
    }
    let mut line = Paragraph::new(Line::from(spans));
    if selected {
        line = line.style(Style::default().bg(card_tint(app, e.status, th)));
    }
    line.render(r, buf);
    links
}

/// One session as a single line for a picker that lists checkouts with
/// their sessions under them (the Linear worktree picker): the status
/// mark, the ACCOUNT CHIP, the name as the grid draws it, what it runs on
/// and its last prompt — the LIST row's parts, without its columns or
/// age, cut to `width`.
pub(crate) fn session_line(app: &App, a: &orion_core::Agent, width: usize) -> Vec<Span<'static>> {
    let th = app.theme;
    let look = session_look(app, a, false, th);
    let mut spans = vec![look.dot];
    let mut used = 2;
    if let Some(chip) = account_chip(a, look.quiet, th) {
        used += CHIP_COLUMN_W;
        spans.extend([chip, Span::raw(" ")]);
    }
    let name = truncate(&a.name, width.saturating_sub(used).min(LIST_NAME_MAX));
    used += name.chars().count();
    spans.push(Span::styled(name, look.name_style));
    let runs = list_runs_line(a, &mut None);
    if !runs.is_empty() && width > used + LIST_GAP + 6 {
        let runs = truncate(&runs, (width - used - LIST_GAP).min(LIST_RUNS_MAX));
        used += LIST_GAP + runs.chars().count();
        spans.push(Span::raw(" ".repeat(LIST_GAP)));
        spans.push(Span::styled(runs, Style::default().fg(th.dim)));
    }
    if let Some(prompt) = crate::launcher::last_prompt(a).filter(|p| !p.trim().is_empty()) {
        if width > used + LIST_GAP + 2 + 6 {
            let prompt = truncate(prompt, width - used - LIST_GAP - 2);
            spans.push(Span::raw(" ".repeat(LIST_GAP)));
            spans.push(Span::styled("› ", Style::default().fg(th.dim)));
            spans.push(Span::styled(prompt, Style::default().fg(look.prompt)));
        }
    }
    spans
}

/// One session as a line that names several draws it (the PULL REQUEST
/// PAGE's SESSIONS, `pr_work`): the status mark, the ACCOUNT CHIP and the
/// name, as its grid row draws them.
pub(crate) fn session_chip(app: &App, a: &orion_core::Agent) -> Vec<Span<'static>> {
    let look = session_look(app, a, false, app.theme);
    let mut spans = vec![look.dot];
    if let Some(chip) = account_chip(a, look.quiet, app.theme) {
        spans.extend([chip, Span::raw(" ")]);
    }
    spans.push(Span::styled(a.name.clone(), look.name_style));
    spans
}

/// Cells the ACCOUNT CHIP fills: its mark, with a cell of the fill
/// either side (one before a two-cell glyph).
pub(crate) const CHIP_W: usize = 3;

/// Columns the ACCOUNT CHIP takes on a row: the chip and the air after
/// it, before the name.
pub(crate) const CHIP_COLUMN_W: usize = CHIP_W + 1;

/// The ACCOUNT CHIP `a` wears: [`chip`], dimmed to `quiet` on an
/// archived row, which says nothing in colour. None off a Claude
/// account, or with only one.
pub(crate) fn account_chip(
    a: &orion_core::Agent,
    quiet: Color,
    th: Theme,
) -> Option<Span<'static>> {
    let tag = crate::claude_accounts::account_tag(a.kind, a.custom_harness.as_deref())?;
    Some(chip(&tag, a.archived.then_some(quiet), th))
}

/// An ACCOUNT CHIP: the account's mark — its letter, or the glyph set
/// for it — black and bold, [`CHIP_W`] cells filled with the account's
/// tint; with `quiet` instead on an archived row, and in the `mono`
/// theme with its text gray, inverse, since that theme keeps colour for
/// the statuses.
pub(crate) fn chip(
    tag: &crate::claude_accounts::AccountTag,
    quiet: Option<Color>,
    th: Theme,
) -> Span<'static> {
    let fill = quiet.unwrap_or(if th.is_gray() { th.text } else { tag.color() });
    let mark = &tag.mark;
    let text = if Span::raw(mark.as_str()).width() > 1 {
        format!(" {mark}")
    } else {
        format!(" {mark} ")
    };
    Span::styled(
        text,
        Style::default()
            .fg(Color::Black)
            .bg(fill)
            .add_modifier(Modifier::BOLD),
    )
}

/// What a band's rule needs to know of the band beyond the band itself.
#[derive(Debug, Clone, Copy)]
struct BandRule {
    /// The band's place in `launcher::bands` — what its hit names.
    index: usize,
    /// The cursor is on the band: its branch goes bold.
    on: bool,
    /// The keys are on the band too — not up on the PROJECT TABS nor
    /// down in the pane: the rule takes the accent and the cursor mark.
    lit: bool,
    /// The widths the right-hand columns take on every rule of the grid.
    cols: RuleColumns,
}

/// How many git counts a rule's right end has a column for: uncommitted
/// files, lines added, lines removed, commits ahead, commits behind.
const GIT_COLUMNS: usize = 5;

/// The letters of a branch a rule keeps before anything at its right end
/// gives way.
const MIN_BRANCH_CELLS: usize = 8;

/// The gap between the parts of a rule's right end: the pull request and
/// the counts.
const COLUMN_GAP: usize = 2;

/// What a band's rule says at its right end ([`rule_facts`]): its pull
/// request, `↗ #57 ready`, and its checkout's git counts, `*3 +60 −28 ⇡4
/// ⇣1`, each in its own column.
struct RuleFacts {
    pr: Vec<Span<'static>>,
    git: [Option<Span<'static>>; GIT_COLUMNS],
}

/// The width of each right-hand column on the grid's rules: as wide as
/// the widest band's, so the pull requests and the counts line up down the
/// grid, and a count turning up on one band moves nothing on any other.
/// A column no band has anything in takes no room at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RuleColumns {
    pr: usize,
    git: [usize; GIT_COLUMNS],
}

impl RuleColumns {
    fn of<'a>(facts: impl IntoIterator<Item = &'a RuleFacts>) -> Self {
        let mut cols = Self::default();
        for f in facts {
            cols.pr = cols.pr.max(spans_width(&f.pr));
            for (w, count) in cols.git.iter_mut().zip(&f.git) {
                *w = (*w).max(count.as_ref().map_or(0, Span::width));
            }
        }
        cols
    }

    fn git_width(&self) -> usize {
        let shown = self.git.iter().filter(|w| **w > 0);
        shown.clone().sum::<usize>() + shown.count().saturating_sub(1)
    }

    /// The cells the columns take, with the two-cell gap between the pull
    /// request and the counts when both are there.
    fn width(&self) -> usize {
        let git = self.git_width();
        let gap = if self.pr > 0 && git > 0 {
            COLUMN_GAP
        } else {
            0
        };
        self.pr + gap + git
    }
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// The cells a rule's right end takes around a block `block` wide: ` `
/// before it and ` ──` after it — none with nothing there, where the rule
/// runs on unbroken.
fn rule_tail_width(block: usize) -> usize {
    if block > 0 {
        1 + block + 3
    } else {
        0
    }
}

/// `band`'s [`RuleFacts`]. The pull request in the PR rows' own colors
/// (`pr_row::look`) — its number and the state word, `ready`, `draft`,
/// `merged`, or the trouble (`conflicts`, `failing`) in the needs-you
/// crimson — and the counts as facts rather than statuses: the files in
/// the diff's modified sand, the lines in its pastels, the commits muted.
fn rule_facts(app: &App, band: &crate::launcher::Band, th: Theme) -> RuleFacts {
    let pr = band.pr.as_ref().map_or_else(Vec::new, |pr| {
        let look = crate::pr_row::look(pr.standing, pr.trouble, th);
        let badge = (format!(" {}", pr.badge()), look.badge);
        crate::pr_row::spans(look, &format!("#{}", pr.number), usize::MAX, Some(badge))
    });
    let files = app.worktree_changes(&band.worktree).filter(|n| *n > 0);
    let lines = files.and(app.worktree_lines(&band.worktree));
    let (ahead, behind) = app.worktree_ahead_behind(&band.worktree);
    let count = |n: u64, sign: &str, color: Color| {
        (n > 0).then(|| Span::styled(format!("{sign}{n}"), Style::default().fg(color)))
    };
    RuleFacts {
        pr,
        git: [
            files.and_then(|n| count(n as u64, "*", th.modified)),
            lines.and_then(|l| count(l.added, "+", th.added)),
            lines.and_then(|l| count(l.removed, "−", th.removed)),
            count(ahead as u64, "⇡", th.muted),
            count(behind as u64, "⇣", th.muted),
        ],
    }
}

/// A checkout's SCOPE MARK, the one glyph ahead of its branch wherever
/// the grid names it — a band's rule, the FOOTER's breadcrumb: `⌂` the
/// root, `⎇` a worktree, and the merged purple `●` once its pull request
/// has landed. Two cells each, so the branches start in one column.
pub(super) fn scope_mark(
    app: &App,
    worktree: &orion_core::WorktreeId,
    is_main: bool,
) -> Span<'static> {
    let th = app.theme;
    if app.worktree_wears_merge(worktree) {
        Span::styled("● ", Style::default().fg(th.merged))
    } else if is_main {
        Span::styled("⌂ ", Style::default().fg(th.muted))
    } else {
        Span::styled("⎇ ", Style::default().fg(th.dim))
    }
}

/// The checkout's folder, when it is not just its branch spelt as a
/// folder name (`/` as `-`): `feat/ui-redesign` checked out in `pawy`.
/// Never for the root, whose folder is the project's.
fn folder_if_differs(app: &App, band: &crate::launcher::Band) -> Option<String> {
    if band.is_main {
        return None;
    }
    let path = &app
        .tree
        .worktrees
        .iter()
        .find(|w| w.id == band.worktree)?
        .path;
    let dir = path.file_name()?.to_string_lossy().into_owned();
    (dir != band.branch.replace('/', "-")).then_some(dir)
}

/// A BAND's rule: the checkout on the left — its SCOPE MARK, the branch
/// and, after it, the folder when that says something the branch does not
/// (`feat/ui-redesign in pawy`) — then the rule, and at the right end the
/// pull request and the git counts in columns as wide on every band
/// ([`RuleColumns`]), so the eye reads them down the grid and nothing
/// shifts when one band's count changes.
///
/// On the band the cursor is on the branch is bold — this is what says
/// which checkout the pane reads, since no entry under it may wear the
/// cursor — and for as long as the keys are on it the rule wears the
/// accent and opens on the CURSOR MARK `❯` instead of `──`; gray and
/// unmarked like the rest once the keys are up on the PROJECT TABS or
/// down in the pane. What Tab does to it is the FOOTER's to say.
///
/// A checkout whose pull request has landed is the MERGED BAND: the merged
/// `●` for its mark and the branch in purple, so the one to delete stands
/// out from across the room. Its branch sweeps once, for the few seconds
/// after orion sees the merge land, then holds still.
///
/// A rule too narrow for all of it lets the folder go first, then the
/// pull request's column, then the counts', before the
/// branch shortens past eight letters.
/// Returns the rule's hits: the pull request ahead of the rule itself,
/// so a click on `#42` opens it and one anywhere else lands on the band.
fn draw_band_rule(
    buf: &mut Buffer,
    app: &App,
    r: Rect,
    band: &crate::launcher::Band,
    facts: &RuleFacts,
    rule: BandRule,
) -> Vec<(Rect, HitTarget)> {
    let th = app.theme;
    let width = usize::from(r.width);
    let BandRule {
        on, lit, mut cols, ..
    } = rule;
    let edge = if lit { th.accent } else { th.edge };
    let dash = |n: usize| Span::styled("─".repeat(n), Style::default().fg(edge));

    let merged = app.worktree_wears_merge(&band.worktree);
    let branch_color = if merged {
        th.merged
    } else if band.is_main || on {
        th.text
    } else {
        th.muted
    };
    let mut branch_style = Style::default().fg(branch_color);
    if on {
        branch_style = branch_style.add_modifier(Modifier::BOLD);
    }
    let ramp = (app.animations && app.merge_sweeping(&band.worktree)).then_some(th.merged_sweep);

    // The rule's left end: `──` like every other band's, or the CURSOR
    // MARK `❯` on the one the keys are on — the mark a prompt puts before
    // the line that takes the keys, so a rule that looks like a divider
    // reads as a row something can be done to. Two cells either way, then
    // the two-cell SCOPE MARK, so the branches start in one column.
    let lead = if lit {
        Span::styled(
            "❯ ",
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )
    } else {
        dash(2)
    };
    let lead_w = 2 + 1 + 2;
    // What the right end takes: the columns, with the tail around them;
    // and the words need ` ` and two dashes after them.
    let right_w = |cols: &RuleColumns| 1 + 2 + rule_tail_width(cols.width());
    let keep = band.branch.chars().count().min(MIN_BRANCH_CELLS);
    let crowded = |cols: &RuleColumns| lead_w + keep + right_w(cols) > width;
    if crowded(&cols) {
        cols.pr = 0;
    }
    if crowded(&cols) {
        cols.git = [0; GIT_COLUMNS];
    }
    let room = width.saturating_sub(lead_w + right_w(&cols)).max(1);
    let branch = truncate(&band.branch, room);
    let folder = folder_if_differs(app, band)
        .map(|dir| format!(" in {dir}"))
        .filter(|f| branch.chars().count() + f.chars().count() <= room);

    let mut left = vec![
        lead,
        Span::raw(" "),
        scope_mark(app, &band.worktree, band.is_main),
    ];
    left.extend(status_name_spans(
        branch,
        branch_style,
        ramp,
        app.sweep_phase(),
    ));
    if let Some(folder) = folder {
        left.push(Span::styled(folder, Style::default().fg(th.dim)));
    }

    // The right end: the columns.
    let mut right = Vec::new();
    let gap = || Span::raw(" ".repeat(COLUMN_GAP));
    let mut hits = Vec::new();
    let mut pr_at = None;
    if cols.pr > 0 {
        let mut pr = facts.pr.clone();
        if !pr.is_empty() {
            pr_at = Some((spans_width(&right), spans_width(&pr)));
            let hovered = app.hover_crumb == Some(HitTarget::LauncherBandPr(band.worktree.clone()));
            if let Some(label) = pr.get_mut(1).filter(|_| hovered) {
                label.style = label.style.add_modifier(Modifier::UNDERLINED);
            }
        }
        let pad = cols.pr.saturating_sub(spans_width(&pr));
        right.extend(pr);
        right.push(Span::raw(" ".repeat(pad)));
        if cols.git_width() > 0 {
            right.push(gap());
        }
    }
    let mut first = true;
    for (w, count) in cols.git.iter().zip(&facts.git) {
        if *w == 0 {
            continue;
        }
        if !first {
            right.push(Span::raw(" "));
        }
        first = false;
        let count_w = count.as_ref().map_or(0, Span::width);
        right.push(Span::raw(" ".repeat(w.saturating_sub(count_w))));
        if let Some(count) = count {
            right.push(count.clone());
        }
    }

    let left_w = spans_width(&left);
    let block_w = spans_width(&right);
    let fill = width
        .saturating_sub(left_w + 1 + rule_tail_width(block_w))
        .max(2);
    let mut spans = left;
    spans.push(Span::raw(" "));
    spans.push(dash(fill));
    let block_x = left_w + 1 + fill + 1;
    if block_w > 0 {
        spans.push(Span::raw(" "));
        spans.extend(right);
        spans.push(Span::raw(" "));
        spans.push(dash(2));
    }
    Paragraph::new(Line::from(spans)).render(r, buf);
    if let Some((at, pr_w)) = pr_at {
        let x = r.x + u16::try_from(block_x + at).unwrap_or(u16::MAX);
        if x < r.right() {
            hits.push((
                Rect {
                    x,
                    width: u16::try_from(pr_w).unwrap_or(u16::MAX).min(r.right() - x),
                    height: 1,
                    ..r
                },
                HitTarget::LauncherBandPr(band.worktree.clone()),
            ));
        }
    }
    paint_stack_mark(buf, r, app, &band.worktree);
    hits.push((r, HitTarget::LauncherBand(rule.index)));
    hits
}

/// STACK STATUS on a band's rule: the `⬡` over its last `─` — the tail's,
/// or the fill's when there is no right block — green while the
/// worktree's compose stack runs, faint while it is stopped, and nothing
/// without one. A running stack's STACK TRAIL runs into it down the
/// rule's dashes, one column a frame, and it flashes as the light lands;
/// the light skips text but keeps time across it, so its pace is even.
fn paint_stack_mark(buf: &mut Buffer, r: Rect, app: &App, worktree: &orion_core::WorktreeId) {
    let Some(stack) = app.stack_of(worktree) else {
        return;
    };
    let th = app.theme;
    // The rightmost dash: the rule's own last cell unless it was cut.
    let Some(h) = (r.x..r.right())
        .rev()
        .find(|&x| buf[(x, r.y)].symbol() == "─")
    else {
        return;
    };
    let running = stack.state() == orion_core::compose::StackState::Running;
    let mut mark = if running { th.ok } else { th.faint };
    if running && app.animations {
        let x0 = (r.x..h)
            .find(|&x| buf[(x, r.y)].symbol() == "─")
            .unwrap_or(h);
        let phase = app.sweep_phase() % crate::app::STACK_TRAIL_PERIOD;
        for x in x0..h {
            if buf[(x, r.y)].symbol() != "─" {
                continue;
            }
            if let Some(&c) = th.stack_sweep.get(phase.wrapping_sub(usize::from(x - x0))) {
                buf[(x, r.y)].set_fg(c);
            }
        }
        if let Some(&c) = th.stack_flash.get(phase.wrapping_sub(usize::from(h - x0))) {
            mark = c;
        }
    }
    buf[(h, r.y)].set_symbol(crate::stacks::MARK).set_fg(mark);
}

/// A terminal's mark, the counterpart of a session's STATUS MARK: `▶` for a
/// RUN or SETUP TERMINAL, `❯` for a plain shell, muted while its PTY lives and
/// faint once it has exited.
pub(super) fn terminal_mark(t: &orion_core::TerminalTab, th: Theme) -> Span<'static> {
    let glyph = if t.run_command.is_some() {
        "▶ "
    } else {
        "❯ "
    };
    Span::styled(
        glyph,
        Style::default().fg(if t.alive { th.muted } else { th.faint }),
    )
}

/// What a terminal's entry says it runs on, in the column a session's
/// model takes: `shell` for a shell. A RUN LINE's command goes where a
/// session's prompt does, so the column is blank on it.
fn terminal_runs(t: &orion_core::TerminalTab) -> &'static str {
    if t.run_command.is_some() {
        ""
    } else {
        "shell"
    }
}

/// What opens a RUN LINE's command, where a session's prompt opens on `›`.
const RUN_MARK: &str = "$ ";

/// What a terminal's entry says at its right end, where a session's age
/// goes, at `now` (epoch ms). A live run is a dim `running`; one winding
/// down from its stop counts down to its kill, `stopping… 1:52` (or
/// `restarting… 1:52`, a restart's), in the working gold. One that
/// ended with nobody stopping it says how: a faint `done` for a clean
/// exit, a red `exited 1` for any other; a run whose ending the DAEMON
/// never saw (it restarted since) is a faint `stopped`. A dead shell is a
/// faint `exited`. A stop that was asked for leaves no row to say any of
/// it. A SETUP TERMINAL says [`setup_badge`].
fn terminal_badge(t: &orion_core::TerminalTab, now: i64, th: Theme) -> (String, Style) {
    if t.setup {
        return setup_badge(t, th);
    }
    let faint = Style::default().fg(th.faint);
    if let Some(kill_at) = t.run.stopping_until {
        let doing = if t.run.restarting {
            "restarting"
        } else {
            "stopping"
        };
        return (
            format!("{doing}… {}", countdown(kill_at - now)),
            Style::default().fg(th.warn),
        );
    }
    match (t.run_command.is_some(), t.alive, t.run.exit_code) {
        (false, true, _) => (String::new(), Style::default()),
        (false, false, _) => ("exited".into(), faint),
        (true, true, _) => ("running".into(), Style::default().fg(th.dim)),
        (true, false, Some(0)) => ("done".into(), faint),
        (true, false, Some(code)) => (format!("exited {code}"), Style::default().fg(th.err)),
        (true, false, None) => ("stopped".into(), faint),
    }
}

/// A SETUP TERMINAL's right end: `setting up…` in the working gold while
/// it runs, then a red `setup failed (1)` — or `setup failed`, when the
/// DAEMON that saw it end has restarted since. A clean exit leaves no row
/// to say anything.
fn setup_badge(t: &orion_core::TerminalTab, th: Theme) -> (String, Style) {
    if t.alive {
        return ("setting up…".into(), Style::default().fg(th.warn));
    }
    let failed = match t.run.exit_code {
        Some(code) => format!("setup failed ({code})"),
        None => "setup failed".into(),
    };
    (failed, Style::default().fg(th.err))
}

/// `ms` to go as `m:ss`, rounded up to the second and never below `0:00`.
fn countdown(ms: i64) -> String {
    let secs = (ms.max(0) + 999) / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The color an entry's cursor wash takes when its status wants you:
/// crimson for a turn waiting on you or a crash, the done color for a
/// finish nobody has read — each in the color its STATUS MARK wears, and
/// no other. A working session has none (its spinner says it is busy, and
/// a busy session asks nothing of you), nor do a read finish, a fresh
/// session and a `quiet` one (pending or archived, or cold with nothing
/// left that wants you).
fn card_edge(a: &orion_core::Agent, quiet: bool, th: Theme) -> Option<Color> {
    let status = Some(a.status);
    (!quiet && wants_you(status, a.unseen)).then(|| status_color(status, a.unseen, th))
}

/// The cursor's entry's wash: its status color ([`card_edge`]) taken
/// nearly to black, so the line is only just washed in it. An entry that
/// wants you breathes, the wash rising and falling on the sweep's clock
/// ([`tint_level`]); a quiet one or a terminal holds a still wash of the
/// accent, as every entry does with the ANIMATIONS off. Fainter still
/// while the PROJECT TABS hold the keys.
fn card_tint(app: &App, status: Option<Color>, th: Theme) -> Color {
    let (color, level) = match status {
        Some(c) if app.animations => (c, tint_level(app.sweep_phase())),
        Some(c) => (c, TINT_PEAK),
        None => (th.accent, TINT_STILL),
    };
    let away = if app.launcher_tab_cursor.is_none() {
        1.0
    } else {
        0.6
    };
    dim_toward_black(color, level * away)
}

/// How much of its status color a breathing entry's wash keeps at its
/// brightest, and how much a still one keeps of the accent.
const TINT_PEAK: f32 = 0.20;
const TINT_STILL: f32 = 0.13;
/// Its dimmest: all but the grid's own black.
const TINT_FLOOR: f32 = 0.07;
/// Sweep frames ([`crate::app::SWEEP_FRAME`]) in one breath, ~1.6 s.
const TINT_BREATH: usize = 16;

/// Where a breathing fill is at sweep `phase`: a cosine from
/// [`TINT_FLOOR`] up to [`TINT_PEAK`] and back.
fn tint_level(phase: usize) -> f32 {
    let t = (phase % TINT_BREATH) as f32 / TINT_BREATH as f32;
    let wave = (1.0 - (t * std::f32::consts::TAU).cos()) / 2.0;
    TINT_FLOOR + (TINT_PEAK - TINT_FLOOR) * wave
}

/// `c` at `level` of its brightness, the rest black — truecolor, as
/// `focus_tint` already is, since the 256 palette has no dim shade of
/// most hues. A color with no fixed value (`Reset`) is returned as is.
fn dim_toward_black(c: Color, level: f32) -> Color {
    crate::theme::rgb(c).map_or(c, |(r, g, b)| {
        crate::theme::shade([r, g, b].map(f32::from), level)
    })
}

/// What an ARCHIVED session wears where a live one wears its STATUS DOT:
/// the round dot squared off. Two columns wide like the dot it stands in
/// for, so the name behind it starts in the same column on both grids.
const ARCHIVED_MARK: &str = "▪ ";

/// How a session shows its state wherever orion draws it — a LIST entry,
/// the pane's header, the full-screen breadcrumb — so they never
/// disagree.
struct SessionLook {
    /// The STATUS MARK, or an archived card's square.
    dot: Span<'static>,
    /// What an archived session's every part is drawn in, and a live
    /// one's age: dim, a step up on the cursor's entry.
    quiet: Color,
    /// Bold and bright for a session that wants you, plain text for one
    /// working, muted for one at rest — the way an inbox reads.
    name_style: Style,
    /// The last prompt's color: muted where the name is bright, dim where
    /// the session is at rest.
    prompt: Color,
    /// The status sweep across the name, while it animates.
    ramp: Option<[Color; 3]>,
    /// How long since it moved (`done`, `starting`, …), with its lead space.
    ago: String,
    ago_style: Style,
}

/// `a`'s [`SessionLook`], on the cursor's entry when `selected`.
///
/// A session whose PTY is gone (cold: reaped, or lost to a daemon restart)
/// is drawn by what is still true of it. A crash, or a finish nobody has
/// read, still wants you and keeps its mark; anything else it last did —
/// working, asking — stopped being true with the process, so its mark goes
/// faint and still, and its name sits back with the sessions at rest.
///
/// An ARCHIVED session is the same session put away, and it is drawn as
/// such: nothing on it is live, so nothing on it is colored. Every part of
/// it takes `quiet` — dim on its own, lifted to muted on the one the
/// cursor is on, so it reads a step brighter than its neighbours — its
/// name the plain `muted`, its STATUS DOT gives way to `ARCHIVED_MARK`,
/// and the colors off still leave its square mark. The colors say it at a
/// glance and the shapes say it again with the colors off, which is the
/// whole grid's answer to "which of the two lists am I looking at" without
/// a word repeated on every row - the header's `n archived sessions` says
/// that once.
fn session_look(app: &App, a: &orion_core::Agent, selected: bool, th: Theme) -> SessionLook {
    use orion_core::AgentStatus;
    let pending = app.is_placeholder_agent(&a.id);
    let archived = a.archived;
    let cold = crate::app::drawn_cold(a);
    let quiet = if selected { th.muted } else { th.dim };
    let dot = if archived {
        Span::styled(ARCHIVED_MARK, Style::default().fg(th.faint))
    } else if pending {
        status_dot(None, false, app.spin_phase(), th)
    } else if cold {
        Span {
            style: Style::default().fg(th.faint),
            ..status_dot(Some(a.status), false, None, th)
        }
    } else {
        status_dot(Some(a.status), a.unseen, app.spin_phase(), th)
    };
    let ramp = if pending || cold || archived {
        None
    } else {
        sweep_ramp(
            Some(a.status),
            a.unseen,
            app.agent_fresh_alarm(a),
            th,
            app.animations,
        )
    };
    let wants = !pending && !cold && wants_you(Some(a.status), a.unseen);
    let working = pending || (!cold && a.status == AgentStatus::Running);
    // An archived name is not the loud thing on the screen any more: it
    // gives up the bold with the rest of the card's weight and sits one
    // step above the quiet the rest of the card is in — muted over dim,
    // and text over muted on the card the cursor is on, the same one-step
    // lift `quiet` takes there. A live name is bold and bright only while
    // the session wants you; a working one is plain text, one at rest
    // muted — lifted to text under the cursor.
    let name_style = if archived {
        Style::default().fg(if selected { th.text } else { th.muted })
    } else if wants {
        Style::default().fg(th.text).add_modifier(Modifier::BOLD)
    } else if working || selected {
        Style::default().fg(th.text)
    } else {
        Style::default().fg(th.muted)
    };
    let prompt = if archived {
        quiet
    } else if wants || working {
        if selected {
            th.text
        } else {
            th.muted
        }
    } else {
        quiet
    };
    // A session stopped on a usage limit says so where its age goes, in
    // the red its frame is: the reason it is waiting on you.
    let limit = a.limit_reached().filter(|_| !pending);
    // How long ago it was filed, on an archived card, rather than when its
    // turn last moved: the status behind it stopped being news the moment
    // it was put away. A row archived before the stamp existed carries 0
    // and simply has no badge.
    let ago = if archived {
        ago_badge(a.archived_at)
    } else if pending {
        PENDING_SESSION_BADGE.to_string()
    } else if let Some(limit) = limit {
        format!(" {}", limit.label())
    } else if a.unseen {
        " done".to_string()
    } else {
        ago_badge(a.status_changed_at)
    };
    let ago_style = if limit.is_some() {
        Style::default().fg(th.err)
    } else if a.unseen && !pending && !archived {
        Style::default().fg(th.done)
    } else {
        Style::default().fg(quiet)
    };
    SessionLook {
        dot,
        quiet,
        name_style,
        prompt,
        ramp,
        ago,
        ago_style,
    }
}

/// A session's STATUS MARK as its entry draws it, and its name's style on
/// the entry the cursor is on ([`session_look`]): what the FOOTER's
/// breadcrumb names the selected session with, so the two agree.
pub(super) fn session_crumb(app: &App, a: &orion_core::Agent) -> (Span<'static>, Style) {
    let look = session_look(app, a, true, app.theme);
    (look.dot, look.name_style)
}

/// Which entry wears the cursor: the one the cursor is on, or none at all
/// once the aim has been let go of (`event_loop::launcher::clear_aim` —
/// a click on the air between the entries, or the first Esc). The grid
/// still knows where the cursor is; it simply draws no entry as selected,
/// which is what says the box `p` opens will ask where its session lands.
fn wearing(app: &App, cursor: Option<usize>) -> Option<usize> {
    (!app.launcher_unaimed).then_some(cursor).flatten()
}

/// What the ARCHIVED VIEW shows with nothing in it — one line where the
/// grid would be, rather than the live list's hero, which is about
/// starting a session and has nothing to say here.
fn draw_list_empty(f: &mut Frame, app: &mut App, area: Rect, what: &str) {
    app.hits.push((area, HitTarget::PanelBg(Focus::Sessions)));
    if app.overlay.is_some() {
        return;
    }
    let th = app.theme;
    if let Some(r) = row_rect(area, 1) {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                what.to_string(),
                Style::default().fg(th.dim),
            )))
            .alignment(ratatui::layout::Alignment::Center),
            r,
        );
    }
}

/// The harness (and model) a session runs on, as the PANE's breadcrumb
/// names it: `claude opus` — and a CLAUDE ACCOUNT, on a machine
/// with more than one, the email it is signed in as: `a@b.co opus`.
///
/// `cfg` is the frame's CONFIG.JSON slot, filled on the first entry that
/// needs it: only a row with a registry id — a custom harness, another
/// Claude account — reads the file, and an account's email is the last
/// read's, so a grid of built-ins never opens it at all.
fn harness_line(a: &orion_core::Agent, cfg: &mut Option<crate::config::Config>) -> String {
    let short = crate::claude_accounts::short_name(a.kind, a.custom_harness.as_deref());
    let mut out = match short {
        Some(name) => name,
        None if a.kind == orion_core::AgentKind::Custom => {
            let cfg = cfg.get_or_insert_with(crate::config::Config::load);
            crate::agent_picker::session_harness_badge_in(a, cfg)
        }
        None => orion_core::harness::registry_id(a.kind, a.custom_harness.as_deref()).to_string(),
    };
    if let Some(model) = a.model.as_deref().filter(|m| !m.is_empty()) {
        out.push(' ');
        out.push_str(model);
    }
    out
}

/// What a LIST row says a session runs on: the model and the effort —
/// `opus high`, `gpt-5.5 xhigh` — which is what tells one row from the
/// next. A CLAUDE ACCOUNT's name or email is never the row's to show:
/// on a line that narrow it pushed the model and effort out of sight (the
/// ACCOUNT CHIP says which account). A session with no model named says
/// its harness in the model's place (`claude high`), a Claude account's
/// as plain `claude`.
fn list_runs_line(a: &orion_core::Agent, cfg: &mut Option<crate::config::Config>) -> String {
    let mut out = match a.model.as_deref().filter(|m| !m.is_empty()) {
        Some(model) => model.to_string(),
        None if is_claude_account(a, cfg) => orion_core::AgentKind::Claude.as_str().into(),
        None => harness_line(a, cfg),
    };
    if let Some(effort) = a.effort.as_deref().filter(|e| !e.is_empty()) {
        out.push(' ');
        out.push_str(effort);
    }
    out
}

/// Whether `a` runs on a CLAUDE ACCOUNT other than the default one rather
/// than a harness of its own: one with a short name, a Claude row with
/// an account's id, or a custom harness entry that is an account.
fn is_claude_account(a: &orion_core::Agent, cfg: &mut Option<crate::config::Config>) -> bool {
    if crate::claude_accounts::short_name(a.kind, a.custom_harness.as_deref()).is_some() {
        return true;
    }
    match (a.kind, a.custom_harness.as_deref()) {
        (orion_core::AgentKind::Claude, Some(_)) => true,
        (orion_core::AgentKind::Custom, Some(id)) => cfg
            .get_or_insert_with(crate::config::Config::load)
            .effective_harness_by_id(id)
            .is_claude_account(),
        _ => false,
    }
}

/// Smallest GRID the welcome turns a orion in: under it the sky would be
/// a few smudges of dust, so the words stand alone, centered.
const SKY_MIN_W: u16 = 30;
const SKY_MIN_H: u16 = 10;

/// The grid with nothing in it: the splash's animated orion across the
/// space the cards would take, and one welcome under its core — the name,
/// and the one key that starts a session, as a key cap a click presses
/// too ([`HitTarget::LauncherWelcomePrompt`], through the same
/// `event_loop::launcher::open_box` the key runs). The key is whatever
/// the keymap binds, so a rebound prompt is never advertised as `p`.
fn draw_empty(f: &mut Frame, app: &mut App, area: Rect) {
    // Under the box (or any modal) the welcome would only peek out around
    // its edges in fragments; the box is saying the same thing.
    if app.overlay.is_some() {
        app.hits.push((area, HitTarget::PanelBg(Focus::Sessions)));
        return;
    }
    let th = app.theme;
    let t = crate::splash::scene_time(app, app.splash_epoch);
    let mut welcome = vec![Span::styled("Welcome to ", Style::default().fg(th.text))];
    welcome.extend(crate::splash::wordmark_word("orion", t, th));
    // The key line is a button, and marked as one under the pointer the
    // way the header's are.
    let mut words = Style::default().fg(th.muted);
    if app.hover_crumb == Some(HitTarget::LauncherWelcomePrompt) {
        words = words.add_modifier(Modifier::UNDERLINED);
    }
    let key = super::key_hint(app, crate::keymap::Action::QuickPrompt);
    let prompt = Line::from(vec![
        Span::styled("press ", words),
        Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(th.on_accent)
                .bg(th.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to start an agent", words),
    ]);
    let prompt_w = (prompt.width() as u16).min(area.width);
    let lines = vec![Line::from(welcome), Line::from(""), prompt];
    let w = (lines.iter().map(Line::width).max().unwrap_or(0) as u16).min(area.width);
    let h = (lines.len() as u16).min(area.height);
    // The words sit a little under the middle, leaving the galaxy the sky
    // above them to turn in; with no room for one they are just centered.
    let sky = area.width >= SKY_MIN_W && area.height >= SKY_MIN_H;
    let y = if sky {
        area.y + (area.height * 3 / 5).saturating_sub(h / 2)
    } else {
        area.y + area.height.saturating_sub(h) / 2
    };
    let text = Rect {
        x: area.x + (area.width - w) / 2,
        y,
        width: w,
        height: h,
    };
    if sky {
        // A wider berth than the splash's: here the arms curl down past
        // the words, where the splash's run out above them.
        let clear = Rect {
            x: text.x.saturating_sub(4),
            width: text.width + 8,
            ..text
        }
        .intersection(area);
        crate::splash::draw_sky(f.buffer_mut(), area, clear, t, th.accent);
    }
    // Even with no sky, the name's shine moves.
    app.welcome_on_screen = true;
    f.render_widget(Paragraph::new(lines).centered(), text);
    // Ahead of the background, so it wins `hit_at`'s first-match scan.
    let key_row = Rect {
        x: area.x + (area.width - prompt_w) / 2,
        y: text.y + 2,
        width: prompt_w,
        height: 1,
    }
    .intersection(area);
    app.hits.push((key_row, HitTarget::LauncherWelcomePrompt));
    app.hits.push((area, HitTarget::PanelBg(Focus::Sessions)));
}

/// One tab in the PANE's TAB STRIP, or in the header's PROJECT TABS: what
/// it draws, and what a click on it means. The gaps and the divider
/// between tabs are tabs with no hit of their own, so one walk both lays
/// the strip out and hit-tests it.
struct PaneTab {
    spans: Vec<Span<'static>>,
    hit: Option<HitTarget>,
}

impl PaneTab {
    fn plain(spans: Vec<Span<'static>>) -> Self {
        Self { spans, hit: None }
    }

    fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }
}

/// The PANE's CLOSE BUTTON, at the right end of its header: one click
/// folds the pane away, as `^~` does. A cell of air either side, so the
/// target is wider than the glyph.
const PANE_CLOSE: &str = " × ";
/// The PANE's FULL-SCREEN BUTTON, before the CLOSE BUTTON: one click gives
/// the session the whole screen, as `^F` does. A full-screen session's
/// header ends in the NORMAL-SIZE BUTTON, which brings it back down.
const PANE_FULL_SCREEN: &str = " ⤢ ";
const PANE_NORMAL_SIZE: &str = " ⤡ ";

/// A header button of `glyph` at `rect`, muted until the pointer is on
/// it and lit in `lit` then, registered as `hit`.
fn header_button(
    f: &mut Frame,
    app: &mut App,
    rect: Rect,
    glyph: &'static str,
    hit: HitTarget,
    lit: ratatui::style::Color,
) {
    let fg = if app.hover_crumb.as_ref() == Some(&hit) {
        lit
    } else {
        app.theme.muted
    };
    f.render_widget(
        Paragraph::new(Span::styled(glyph, Style::default().fg(fg))),
        rect,
    );
    app.hits.push((rect, hit));
}

/// The PANE's own header in the LAUNCHER VIEW: what the pane is reading
/// — the card under the cursor, session or terminal, and the checkout
/// it runs in — then the state tag (`scroll 4`, `exited`) right-aligned
/// before the FULL-SCREEN and CLOSE BUTTONS. A rule under it, and the rect the
/// PTY draws in returned, exactly as `ui::terminal_frame` does for the
/// panels' pane. Opening a terminal is `t`'s alone: the grid offers no
/// `+` for it.
pub(super) fn pane_frame(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    right: Option<Span<'static>>,
    focused: bool,
) -> Rect {
    let th = app.theme;
    if let Some(r) = row_rect(area, 1) {
        let r = pad_x(r);
        // The CLOSE BUTTON holds the right end of the row, the
        // FULL-SCREEN BUTTON stands before it, and the state tag
        // (`scroll 4`, exited) is right-aligned before those: the title
        // gets what they leave.
        let zoom_w = PANE_FULL_SCREEN.chars().count() as u16;
        let close_w = PANE_CLOSE.chars().count() as u16;
        let buttons_w = zoom_w + close_w;
        let taken = usize::from(buttons_w)
            + right
                .as_ref()
                .map_or(0, |tag| tag.content.chars().count() + 2);
        let tabs = pane_title(app, usize::from(r.width).saturating_sub(taken));
        let mut spans = Vec::new();
        let mut x = r.x;
        for tab in tabs {
            let width = u16::try_from(tab.width()).unwrap_or(u16::MAX);
            if let Some(hit) = tab.hit {
                app.hits.push((
                    Rect {
                        x,
                        y: r.y,
                        width,
                        height: 1,
                    },
                    hit,
                ));
            }
            x = x.saturating_add(width);
            spans.extend(tab.spans);
        }
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        let tag_r = Rect {
            width: r.width.saturating_sub(buttons_w),
            ..r
        };
        if let Some(tag) = right {
            f.render_widget(
                Paragraph::new(Line::from(vec![tag, Span::raw(" ")]))
                    .alignment(ratatui::layout::Alignment::Right),
                tag_r,
            );
        }
        if r.width >= buttons_w {
            let mut x = tag_r.x + tag_r.width;
            let zoom = Rect {
                x,
                width: zoom_w,
                ..r
            };
            header_button(
                f,
                app,
                zoom,
                PANE_FULL_SCREEN,
                HitTarget::LauncherPaneZoom,
                th.accent,
            );
            x += zoom_w;
            let close = Rect {
                x,
                width: close_w,
                ..r
            };
            // Marked under the pointer the way the PROJECT TABS' `×` is:
            // nothing about a cross says it is a button until then.
            let fg = if app.hover_crumb == Some(HitTarget::LauncherPaneClose) {
                th.err
            } else {
                th.muted
            };
            f.render_widget(
                Paragraph::new(Span::styled(PANE_CLOSE, Style::default().fg(fg))),
                close,
            );
            app.hits.push((close, HitTarget::LauncherPaneClose));
        }
    }
    draw_rule(f, area, 2, if focused { th.accent } else { th.edge });
    Rect {
        y: area.y + 3,
        height: area.height.saturating_sub(3),
        ..area
    }
}

/// The pane's title, laid out left to right in `room` columns: the card
/// the cursor is on — its STATUS MARK and name for a session, drawn by
/// the same [`session_look`] its row is, so a reaped or archived session
/// reads the same here as there; the `❯`/`▶` and name for a terminal —
/// bold, the name being the thing the pane reads; the checkout it runs in
/// after it, as the band's rule names it. With the pane on something the
/// grid no longer lists — a card just archived out from under it — the
/// attached session's own name, muted.
fn pane_title(app: &App, room: usize) -> Vec<PaneTab> {
    let th = app.theme;
    let bands = crate::launcher::bands(app);
    let at = crate::launcher::cursor(app, &bands);
    let card = at.and_then(|at| crate::launcher::card_at(&bands, at));
    let name_room = (room / 2).max(8);
    let lit = Style::default().fg(th.text).add_modifier(Modifier::BOLD);
    let mut tabs = Vec::new();
    match card {
        Some(crate::launcher::Card::Session(row)) => {
            let a = &row.agent;
            let look = session_look(app, a, false, th);
            tabs.push(PaneTab::plain(vec![
                look.dot,
                Span::styled(truncate(&a.name, name_room), lit),
            ]));
            tabs.extend(pane_account(a, look.quiet, room, th));
        }
        Some(crate::launcher::Card::Terminal(t)) => {
            tabs.push(PaneTab::plain(vec![
                terminal_mark(t, th),
                Span::styled(truncate(&t.name, name_room), lit),
            ]));
        }
        None => {
            if let Some(name) = app.term.as_ref().and_then(|t| attached_name(app, &t.sref)) {
                tabs.push(PaneTab::plain(vec![Span::styled(
                    truncate(&name, name_room),
                    Style::default().fg(th.muted),
                )]));
            }
        }
    }
    // The checkout everything in the pane belongs to, as the band's rule
    // names it — its SCOPE MARK and branch, and its pull request as
    // `↗ #42 ready` — so the pane and the grid say the same thing the same
    // way.
    let checkout = at
        .and_then(|at| bands.get(at.band))
        .map(|b| {
            (
                b.worktree.clone(),
                b.is_main,
                b.branch.clone(),
                b.pr.clone(),
            )
        })
        .or_else(|| {
            app.selected_worktree()
                .map(|w| (w.id.clone(), w.is_main, w.branch.clone(), None))
        });
    if let Some((worktree, is_main, branch, pr)) = checkout {
        if !tabs.is_empty() {
            tabs.push(PaneTab::plain(vec![Span::raw("  ")]));
        }
        let mut spans = vec![
            scope_mark(app, &worktree, is_main),
            Span::styled(
                truncate(&branch, (room / 4).max(8)),
                Style::default().fg(th.muted),
            ),
        ];
        if let Some(pr) = pr {
            let look = crate::pr_row::look(pr.standing, pr.trouble, th);
            spans.push(Span::raw("  "));
            spans.push(Span::styled("↗ ", Style::default().fg(look.glyph)));
            spans.push(Span::styled(
                format!("#{}", pr.number),
                Style::default().fg(look.label),
            ));
            // The word goes before the number does: only when it fits.
            let word = format!(" {}", pr.badge());
            let used: usize = tabs.iter().map(PaneTab::width).sum::<usize>()
                + spans
                    .iter()
                    .map(|s| s.content.chars().count())
                    .sum::<usize>();
            if used + word.chars().count() <= room {
                spans.push(Span::styled(word, Style::default().fg(look.badge)));
            }
        }
        tabs.push(PaneTab::plain(spans));
    }
    tabs
}

/// The account a session in the pane runs on, after its name in the
/// pane's header, on a machine with more than one: its ACCOUNT CHIP and
/// its name (else its email) in its tint, `  R  Riplo` — the pointer and
/// a click on it doing what they do on the row's chip. Nothing off a
/// Claude account, or with only one.
fn pane_account(a: &orion_core::Agent, quiet: Color, room: usize, th: Theme) -> Vec<PaneTab> {
    let Some(tag) = crate::claude_accounts::account_tag(a.kind, a.custom_harness.as_deref()) else {
        return Vec::new();
    };
    let id = orion_core::harness::registry_id(a.kind, a.custom_harness.as_deref());
    let name = crate::claude_accounts::short_name(a.kind, a.custom_harness.as_deref())
        .unwrap_or_else(|| id.to_string());
    let tint = if a.archived {
        quiet
    } else if th.is_gray() {
        th.text
    } else {
        tag.color()
    };
    vec![
        PaneTab::plain(vec![Span::raw("  ")]),
        PaneTab {
            spans: vec![
                chip(&tag, a.archived.then_some(quiet), th),
                Span::styled(
                    format!(" {}", truncate(&name, (room / 4).max(8))),
                    Style::default().fg(tint),
                ),
            ],
            hit: Some(HitTarget::AccountTag(id.to_string())),
        },
    ]
}

/// The name of the session `sref` attaches, whichever kind it is.
fn attached_name(app: &App, sref: &orion_core::SessionRef) -> Option<String> {
    match sref {
        orion_core::SessionRef::Agent(id) => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .map(|a| a.name.clone()),
        orion_core::SessionRef::Terminal(id) => app
            .tree
            .terminals
            .iter()
            .find(|t| &t.id == id)
            .map(|t| t.name.clone()),
    }
}

/// The full-screen session's own header, in place of the pane's
/// `TERMINAL · name`: a breadcrumb back to the grid — `‹ sessions` is a
/// button, the session's name the crumb it leads out of — with the
/// harness it runs on and where, right-aligned before the NORMAL-SIZE
/// BUTTON. Returns the rect the PTY
/// draws in, as `terminal_frame` does.
pub(super) fn crumb_frame(f: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let th = app.theme;
    if let Some(r) = row_rect(area, 1) {
        let r = pad_x(r);
        // The NORMAL-SIZE BUTTON holds the right end of the row, where the
        // pane's header keeps its buttons; the rest is laid out in what it
        // leaves.
        let size_w = PANE_NORMAL_SIZE.chars().count() as u16;
        if r.width > size_w {
            let size = Rect {
                x: r.x + r.width - size_w,
                width: size_w,
                ..r
            };
            header_button(
                f,
                app,
                size,
                PANE_NORMAL_SIZE,
                HitTarget::LauncherPaneZoom,
                th.accent,
            );
        }
        let r = Rect {
            width: r.width.saturating_sub(size_w),
            ..r
        };
        let back = format!("‹ {CRUMB}");
        // The hatch out of a full-screen session is a button too, and
        // wears the same underline while the pointer is on it.
        let mut style = Style::default().fg(th.accent).add_modifier(Modifier::BOLD);
        if app.hover_crumb.as_ref() == Some(&HitTarget::LauncherCrumb) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        let mut spans = vec![Span::styled(back.clone(), style)];
        // Only the crumb itself is the button — a click anywhere else on
        // the header row belongs to the pane.
        app.hits.push((
            Rect {
                width: back.chars().count() as u16,
                height: 1,
                ..r
            },
            HitTarget::LauncherCrumb,
        ));
        let row = app
            .term
            .as_ref()
            .map(|t| t.sref.clone())
            .and_then(|sref| match sref {
                orion_core::SessionRef::Agent(id) => crate::launcher::row(app, &id),
                _ => None,
            });
        if let Some(row) = &row {
            let a = &row.agent;
            spans.push(Span::styled(" / ", Style::default().fg(th.dim)));
            spans.push(session_look(app, a, false, th).dot);
            spans.push(Span::styled(
                truncate(&a.name, usize::from(r.width / 2).max(8)),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        if let Some(row) = &row {
            let a = &row.agent;
            let mut right = vec![Span::styled(
                harness_line(a, &mut None),
                Style::default().fg(th.muted),
            )];
            if let Some(effort) = a.effort.as_deref().filter(|e| !e.is_empty()) {
                right.push(Span::styled(
                    format!(" {effort}"),
                    Style::default().fg(th.dim),
                ));
            }
            // Where it runs, as the footer's breadcrumb names it: the
            // project, then the checkout behind its SCOPE MARK.
            let is_main = app
                .tree
                .worktrees
                .iter()
                .any(|w| w.id == a.worktree_id && w.is_main);
            right.push(Span::styled(" · ", Style::default().fg(th.dim)));
            right.push(Span::styled(
                format!("{} ", truncate(&row.project, 20)),
                Style::default().fg(th.muted),
            ));
            right.push(scope_mark(app, &a.worktree_id, is_main));
            right.push(Span::styled(
                truncate(&row.branch, 24),
                Style::default().fg(th.muted),
            ));
            f.render_widget(
                Paragraph::new(Line::from(right)).alignment(ratatui::layout::Alignment::Right),
                r,
            );
        }
    }
    let focused = app.focus == Focus::Terminal;
    draw_rule(f, area, 2, if focused { th.accent } else { th.edge });
    Rect {
        y: area.y + 3,
        height: area.height.saturating_sub(3),
        ..area
    }
}

/// The view's box header: everything the launch is made of, each field a
/// dim word, its value in bold and the key that changes it in the accent
/// — `project demo ⌘P`, `effort high ⌘Y` — so what Enter is about to
/// start reads off the box before a key is pressed, and every key on it
/// is one the box answers to. Two rows: where the session runs — the
/// project, the checkout in it, the AGENT PRESET wrapping the task when
/// one is on — over what runs there — the harness (a CLAUDE ACCOUNT as
/// `Claude (you@example.com)`), its model and its effort. The effort is
/// always there, `default` while nothing is picked, for every harness
/// that has one (OpenCode has none, so its box has no effort field).
///
/// A key comes from where its handler reads it: Select worktree, Select
/// model and Cycle effort from the live keymap — a rebind shows at once,
/// an unbound action shows no key, and a terminal that never sends ⌘
/// sees the `^` twin — and the project, harness and preset keys from the
/// box's own table ([`super::task_keys`]).
///
/// A row too narrow for its fields wraps them onto the next, whole; a
/// field too wide for a row of its own has its value cut — never its key
/// — and then gives up its word, and a field that cannot keep even a few
/// letters of its value is left off. The first field of each row has its
/// word padded to the widest of them, so the values start in one column.
///
/// A fresh worktree's field reads `new worktree <branch>`, in the green
/// the box's frame turns while Enter will cut it. A box aimed away from
/// the grid behind it fires a BACKGROUND LAUNCH: the project is the half
/// that changed, and nothing on screen will move when Enter lands, so the
/// project is lit. A PR SESSION's head branch wears no key and is no
/// button: its checkout is the DAEMON's to pick.
///
/// Returns the rows, each field's own cells — the word, the value and the
/// key together, never the air between two fields — so a click there can
/// be given the same picker the key opens
/// ([`crate::app::PromptDialog::detail_areas`]), and the branch's own, for
/// the WORKTREE PICKER to hang from
/// ([`crate::app::PromptDialog::branch_area`]). A field left off for room
/// hands back no cells: what is not drawn is not a button.
pub(super) fn box_header(
    app: &App,
    launch: &QuickLaunch,
    cfg: &crate::config::Config,
    width: u16,
    th: Theme,
) -> BoxHeader {
    lay_out(&header_fields(app, launch, cfg, th), usize::from(width), th)
}

/// [`box_header`]'s answer: the rows, each drawn field's `(field, row,
/// first column, width)` within them, and the branch's `(row, first
/// column, width)` — the value and its key — None when it was left off
/// for room or names a checkout nobody picks.
#[derive(Debug, Default)]
pub(super) struct BoxHeader {
    pub lines: Vec<Line<'static>>,
    pub fields: Vec<(BoxField, u16, u16, u16)>,
    pub branch: Option<(u16, u16, u16)>,
}

/// The air between two fields on a row — wide enough that `demo ⌘P` and
/// `worktree main` never read as one phrase.
const FIELD_GAP: usize = 3;

/// Widest the `agent` field's value is drawn: a CLAUDE ACCOUNT's label
/// runs to an email address, which took the row the mode needs.
const AGENT_W: usize = 10;

/// Fewest columns a value is cut to — `ma…` — before its field gives up
/// its word, and then the whole field, instead.
const MIN_VALUE: usize = 3;

/// One field of [`box_header`], before a row decides where it goes.
struct HeaderField {
    field: BoxField,
    label: &'static str,
    label_style: Style,
    value: String,
    value_style: Style,
    /// The key that changes it, already spelled; None for a field nothing
    /// changes or an action left unbound.
    key: Option<String>,
    /// A click on it opens its picker — every field but a PR SESSION's
    /// checkout, which is the DAEMON's to pick.
    button: bool,
    /// Said after the value, the first thing to go short of room: the
    /// agent's headroom, `41% · 2h`.
    detail: Option<Span<'static>>,
}

/// The two rows of fields [`box_header`] lays out: where, then what.
fn header_fields(
    app: &App,
    launch: &QuickLaunch,
    cfg: &crate::config::Config,
    th: Theme,
) -> [Vec<HeaderField>; 2] {
    use super::task_keys::{AGENT, MODE, PRESET, PROJECT};
    let dim = Style::default().fg(th.dim);
    let bold = |fg| Style::default().fg(fg).add_modifier(Modifier::BOLD);
    let action_key = |action| crate::hints::key(&app.keymap, action);
    let field = |field, label, value: String, value_style, key| HeaderField {
        field,
        label,
        label_style: dim,
        value,
        value_style,
        key,
        button: true,
        detail: None,
    };

    let background = crate::launcher::project_of(app, &launch.target)
        .is_some_and(|project| crate::launcher::is_background(app, &project));
    let project = field(
        BoxField::Project,
        "project",
        launch_project(app, launch),
        bold(if background { th.accent } else { th.text }),
        Some(PROJECT.label()),
    );
    let worktree = match &launch.pr {
        Some(pr) => HeaderField {
            button: false,
            ..field(
                BoxField::Worktree,
                "worktree",
                pr.head.clone(),
                bold(th.text),
                None,
            )
        },
        None => {
            let branch = crate::quick_prompt::target_branch(app, launch)
                .unwrap_or_else(|| "(worktree gone)".into());
            let key = action_key(Action::SelectLaunchWorktree);
            if launch.is_new_worktree() {
                HeaderField {
                    label_style: bold(th.ok),
                    ..field(
                        BoxField::Worktree,
                        launch.new_worktree_label(),
                        branch,
                        bold(th.ok),
                        key,
                    )
                }
            } else {
                field(BoxField::Worktree, "worktree", branch, bold(th.text), key)
            }
        }
    };
    let mut place = vec![project, worktree];
    if let Some(preset) = &launch.preset {
        place.push(field(
            BoxField::Preset,
            "preset",
            preset.name.clone(),
            bold(th.text),
            Some(PRESET.label()),
        ));
    }

    let harness = cfg.effective_harness(launch.kind, launch.custom.as_deref());
    // A CLAUDE ACCOUNT by its name, else its email, with more than one —
    // cut to a fixed width, so a long one leaves the row room for the
    // mode, the model and the effort beside it.
    let agent = crate::claude_accounts::short_name(launch.kind, launch.custom.as_deref())
        .unwrap_or_else(|| harness.display_label().to_string());
    let agent = super::truncate(&agent, AGENT_W);
    // What is left on it, as the usage readings have it: `41% · 2h`.
    let now = orion_core::clock::now_secs() as i64;
    let headroom = crate::usage::headroom(app, &harness.id)
        .map(|room| (room.short(now), room.color(th)))
        .filter(|(said, _)| !said.is_empty())
        .map(|(said, fg)| Span::styled(said, Style::default().fg(fg)));
    let model = crate::config::model_row_label(
        launch
            .model
            .as_deref()
            .unwrap_or(crate::config::DEFAULT_CHOICE),
        harness.model.catalog,
    );
    let mut runs = vec![
        HeaderField {
            detail: headroom,
            ..field(
                BoxField::Agent,
                "agent",
                agent,
                bold(th.text),
                Some(AGENT.label()),
            )
        },
        field(
            BoxField::Model,
            "model",
            model,
            bold(th.text),
            action_key(Action::SelectModel),
        ),
    ];
    // Beside the agent, since what it can be is the agent's: plan and ask
    // read in colour, so a launch that will not edit is never mistaken for
    // one that will.
    if launch.modes(cfg).len() > 1 {
        let colour = match launch.mode {
            orion_core::AgentMode::Edit => th.text,
            orion_core::AgentMode::Plan => th.warn,
            orion_core::AgentMode::Ask => th.special,
        };
        runs.insert(
            1,
            field(
                BoxField::Mode,
                "mode",
                launch.mode.as_str().to_string(),
                bold(colour),
                Some(MODE.label()),
            ),
        );
    }
    if !crate::config::effort_choices_in(&harness, launch.model.as_deref()).is_empty() {
        runs.push(field(
            BoxField::Effort,
            "effort",
            launch
                .effort
                .clone()
                .unwrap_or_else(|| crate::config::DEFAULT_CHOICE.into()),
            bold(th.text),
            action_key(Action::CycleEffort),
        ));
    }
    [place, runs]
}

/// [`box_header`]'s layout: each group of fields from a fresh row,
/// wrapping onto the next whenever the row is full.
fn lay_out(groups: &[Vec<HeaderField>], width: usize, th: Theme) -> BoxHeader {
    let label_w = groups
        .iter()
        .filter_map(|group| group.first())
        .map(|f| f.label.chars().count())
        .max()
        .unwrap_or(0);
    let key_style = crate::hints::key_style(th);
    let mut out = BoxHeader::default();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut x = 0usize;
    let flush = |out: &mut BoxHeader, row: &mut Vec<Span<'static>>, x: &mut usize| {
        if !row.is_empty() {
            out.lines.push(Line::from(std::mem::take(row)));
        }
        *x = 0;
    };
    for group in groups {
        flush(&mut out, &mut row, &mut x);
        for field in group {
            let key_w = field
                .key
                .as_ref()
                .map_or(0, |k| 1 + Span::raw(k.as_str()).width());
            let detail_w = field.detail.as_ref().map_or(0, |d| 1 + d.width());
            let value_w = Span::raw(field.value.as_str()).width();
            let label_at = |x: usize| {
                let own = field.label.chars().count();
                if x == 0 {
                    own.max(label_w)
                } else {
                    own
                }
            };
            // Whole on this row, or whole on a row of its own.
            if x > 0 && x + FIELD_GAP + label_at(x) + 1 + value_w + detail_w + key_w > width {
                flush(&mut out, &mut row, &mut x);
            }
            let gap = if x > 0 { FIELD_GAP } else { 0 };
            let room = width.saturating_sub(x + gap);
            // The detail goes first, whole, before the value is cut.
            let detail = field
                .detail
                .clone()
                .filter(|_| label_at(x) + 1 + value_w + detail_w + key_w <= room);
            // The value is cut before the word goes, and the word before
            // the field does; the key is never cut.
            let labelled = label_at(x) + 1;
            let (label, value) = if labelled + value_w + key_w <= room {
                (Some(labelled), field.value.clone())
            } else if room >= labelled + MIN_VALUE + key_w {
                (
                    Some(labelled),
                    truncate(&field.value, room - labelled - key_w),
                )
            } else if room >= MIN_VALUE.min(value_w) + key_w {
                (None, truncate(&field.value, room - key_w))
            } else {
                continue;
            };
            if gap > 0 {
                row.push(Span::raw(" ".repeat(gap)));
                x += gap;
            }
            let start = x;
            if let Some(w) = label {
                let text = format!("{:<w$}", field.label, w = w);
                x += text.chars().count();
                row.push(Span::styled(text, field.label_style));
            }
            let value_at = x;
            x += Span::raw(value.as_str()).width();
            row.push(Span::styled(value, field.value_style));
            if let Some(detail) = detail {
                x += 1 + detail.width();
                row.extend([Span::raw(" "), detail]);
            }
            if let Some(key) = &field.key {
                let text = format!(" {key}");
                x += Span::raw(text.as_str()).width();
                row.push(Span::styled(text, key_style));
            }
            if !field.button {
                continue;
            }
            let at = out.lines.len() as u16;
            if field.field == BoxField::Worktree {
                out.branch = Some((at, value_at as u16, (x - value_at) as u16));
            }
            out.fields
                .push((field.field, at, start as u16, (x - start) as u16));
        }
    }
    flush(&mut out, &mut row, &mut x);
    out
}

/// The PROJECT a launch is aimed at, by name.
fn launch_project(app: &App, launch: &QuickLaunch) -> String {
    crate::launcher::project_of(app, &launch.target)
        .and_then(|id| crate::launcher::project_name(app, &id))
        .unwrap_or_else(|| "(project gone)".into())
}

/// The view's box title: what Enter starts — always a new AGENT (a
/// TERMINAL is `t`, never this box). The harness, the model, the
/// effort and an AGENT PRESET are in [`box_header`] under it, beside the
/// keys that change them; what is left here is what the box is *for* — a
/// pull request, a Linear batch.
pub(super) fn box_title(launch: &QuickLaunch) -> String {
    let mut head = vec!["New agent".to_string()];
    if let Some(pr) = &launch.pr {
        head.push(format!("PR #{}", pr.number));
    }
    if let Some(linear) = &launch.linear {
        head.push(linear.title());
    }
    head.join(" · ")
}

/// Where the view's QUICK PROMPT is drawn — one place, so the PROJECT
/// PICKER can float over exactly the rect the box is in.
pub(super) fn box_rect(frame: Rect) -> Rect {
    centered_rect(frame, BOX_SIZE.0, BOX_SIZE.1)
}

/// Does the PROJECT PICKER float over the box, rather than stand on its
/// own in the middle of the screen? Only when a box was up to come back
/// to.
pub(super) fn picker_over_box(_app: &App, picker: &ProjectPicker) -> bool {
    picker.back.from_box
}

/// Where the PROJECT PICKER goes, sized to the list it has to show.
/// Opened from the box, it floats over it: inset inside the box's rect,
/// so the box's frame, its title and its header stay on screen
/// around the list. The list gives up the rows that costs — the box
/// behind is worth more than four more projects. With no box under it,
/// it is centered on the screen as any other modal.
fn picker_rect(frame: Rect, over: Option<Rect>, matches: usize) -> Rect {
    // Unlike the menus that float over the box, the picker shrinks to fit
    // inside it rather than spilling back out to the middle of the
    // screen: a list has rows to give up, and the box behind is worth
    // more than four more projects.
    let max_rows = over
        .map(|b| b.height.saturating_sub(OVER_BOX_INSET + 4))
        .unwrap_or(PICKER_ROWS)
        .clamp(1, PICKER_ROWS);
    let rows = (matches as u16).clamp(1, max_rows);
    let width = over.map_or(PICKER_W, |b| {
        PICKER_W.min(b.width.saturating_sub(OVER_BOX_INSET))
    });
    over_box_rect(frame, over, width, rows + 4)
}

/// The PROJECT PICKER: the query on top, the projects under it — the name
/// with the typed letters lit, the path dim after it. Drawn over the box it was opened from
/// (`ui::draw_overlay` puts that box down first), inset inside it.
pub(super) fn draw_project_picker(f: &mut Frame, app: &mut App, picker: &ProjectPicker) {
    let th = app.theme;
    let over = picker_over_box(app, picker).then(|| box_rect(f.area()));
    let area = picker_rect(f.area(), over, picker.matches.len());
    let title = if picker.query.is_empty() {
        " Project ".to_string()
    } else {
        format!(
            " Project ({}/{}) ",
            picker.matches.len(),
            picker.projects.len()
        )
    };
    let hints = [
        crate::hints::Hint::new("Enter", "aim the box there").kept(),
        crate::hints::Hint::new("Esc", "back to the box"),
    ];
    let inner = render_modal_frame(f, area, title, &hints, th);
    if let Some(query_area) = row_rect(inner, 0) {
        let line = search_line(&picker.query, "type a project name…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let list = below_first_row(inner);
    if picker.matches.is_empty() {
        empty_list_row(f, list, NO_MATCHES, th);
    }
    let selected = picker.selected.min(picker.matches.len().saturating_sub(1));
    let start = crate::app::window_start(selected, list.height as usize);
    for (row, (i, (index, positions))) in picker.matches.iter().enumerate().skip(start).enumerate()
    {
        let Some(row_area) = row_rect(list, row) else {
            break;
        };
        let project = &picker.projects[*index];
        let budget = (list.width as usize).saturating_sub(4);
        let name = truncate(&project.name, budget);
        let lit = visible_positions(positions, &name, &project.name);
        let mut spans = vec![Span::styled("▪ ", Style::default().fg(th.accent))];
        spans.extend(fuzzy_highlight_spans(&name, lit, th));
        let used = name.chars().count();
        let after = format!("  {}", project.path);
        if used + 2 < budget {
            spans.push(Span::styled(
                truncate(&after, budget - used),
                Style::default().fg(th.dim),
            ));
        }
        render_row(f, row_area, spans, i == selected, true, th);
    }
    if let Some(Overlay::ProjectPicker(p)) = &mut app.overlay {
        p.area = area;
        p.list_area = list;
        p.selected = selected;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::LauncherRow;

    /// [`draw_list_row`] for the tests that draw one entry alone, its
    /// columns 12 wide.
    fn draw_one(app: &App, area: Rect, row: &LauncherRow, selected: bool) -> Buffer {
        let mut buf = Buffer::empty(area);
        let card = crate::launcher::Card::Session(row.clone());
        draw_list_row(&mut buf, app, area, &card, selected, (12, 12), &mut None);
        buf
    }

    /// `^P` opens the PROJECT PICKER *over* the box, so the rect it takes
    /// is strictly inside the box's on all four sides — the frame, the
    /// title and the header above the list stay on screen however
    /// long the project list is. With no box under it the picker is
    /// centered on the screen, as it always was.
    #[test]
    fn the_picker_floats_inside_the_box_it_was_opened_from() {
        let frame = Rect::new(0, 0, 130, 34);
        let boxed = box_rect(frame);
        for matches in [0usize, 1, 3, 200] {
            let area = picker_rect(frame, Some(boxed), matches);
            assert!(
                area.x > boxed.x && area.right() < boxed.right(),
                "{matches}: {area:?} is not inside {boxed:?} sideways"
            );
            assert!(
                area.y > boxed.y && area.bottom() < boxed.bottom(),
                "{matches}: {area:?} is not inside {boxed:?} top to bottom"
            );
        }
        // Long list, no box: the screen's middle and the full height.
        let loose = picker_rect(frame, None, 200);
        assert_eq!(loose.height, PICKER_ROWS + 4);
        assert_eq!(loose, centered_rect(frame, PICKER_W, PICKER_ROWS + 4));
    }

    /// A terminal too small for the box to float anything inside still
    /// gets a picker — clamped, never a zero-sized or off-screen rect.
    #[test]
    fn a_tiny_screen_still_draws_the_picker() {
        for (w, h) in [(20u16, 6u16), (30, 9), (60, 12)] {
            let frame = Rect::new(0, 0, w, h);
            let area = picker_rect(frame, Some(box_rect(frame)), 50);
            assert!(area.width > 0 && area.height > 0, "{w}x{h}: {area:?}");
            assert!(
                area.right() <= frame.right() && area.bottom() <= frame.bottom(),
                "{w}x{h}: {area:?} runs off {frame:?}"
            );
        }
    }

    fn a_launch() -> QuickLaunch {
        let cfg = crate::config::Config::default();
        let target = crate::quick_prompt::QuickTarget::Worktree(orion_core::WorktreeId("w".into()));
        QuickLaunch::of_kind(
            target,
            orion_core::AgentKind::Claude,
            None,
            None,
            None,
            &cfg,
        )
    }

    fn text_of(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The title is what the box is *for*; the harness, the model, the
    /// effort and a preset are in the header, beside the keys that change
    /// them.
    #[test]
    fn the_box_title_is_what_the_box_is_for() {
        let mut launch = a_launch();
        launch.model = None;
        launch.effort = None;
        assert_eq!(box_title(&launch), "New agent");
        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        launch.preset = Some(Box::new(a_preset("reviewer")));
        assert_eq!(box_title(&launch), "New agent");
    }

    fn a_preset(name: &str) -> crate::agent_presets::AgentPreset {
        crate::agent_presets::AgentPreset {
            name: name.into(),
            kind: orion_core::AgentKind::Claude,
            custom_harness: None,
            follow_box: None,
            model: None,
            effort: None,
            text: String::new(),
            skip_task: false,
            system: None,
        }
    }

    /// The header at `width`, under the stock config.
    fn header(app: &App, launch: &QuickLaunch, width: u16) -> BoxHeader {
        box_header(
            app,
            launch,
            &crate::config::Config::default(),
            width,
            Theme::default(),
        )
    }

    /// Its rows as plain text.
    fn rows_of(header: &BoxHeader) -> Vec<String> {
        header.lines.iter().map(text_of).collect()
    }

    /// The text `field` was drawn in, read back out of its own cells.
    fn cells_of(header: &BoxHeader, field: BoxField) -> Option<String> {
        let rows = rows_of(header);
        header
            .fields
            .iter()
            .find(|(f, ..)| *f == field)
            .map(|(_, row, x, w)| {
                rows[usize::from(*row)]
                    .chars()
                    .skip(usize::from(*x))
                    .take(usize::from(*w))
                    .collect()
            })
    }

    /// The box's header names every field of the launch on two rows —
    /// where it runs over what runs it — each with the key that changes
    /// it, and fits the box it is drawn in. The effort is there with
    /// nothing picked, as `default`.
    #[test]
    fn the_header_names_every_field_with_its_key() {
        let app = App::new();
        let mut launch = a_launch();
        launch.model = None;
        launch.effort = None;
        let inner = BOX_SIZE.0 - 4;
        let head = header(&app, &launch, inner);
        let rows = rows_of(&head);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows[0].starts_with("project "), "{rows:?}");
        assert!(rows[1].starts_with("agent "), "{rows:?}");
        for (field, want) in [
            (BoxField::Project, "project (project gone) ⌘P"),
            (BoxField::Worktree, "worktree (worktree gone) ⌘."),
            (BoxField::Agent, "Claude Tab"),
            (BoxField::Model, "model default ⌘/"),
            (BoxField::Effort, "effort default ⌘Y"),
        ] {
            let cells = cells_of(&head, field).unwrap_or_else(|| panic!("{field:?}: {rows:?}"));
            assert!(cells.ends_with(want), "{field:?} is {cells:?}");
        }
        assert!(
            cells_of(&head, BoxField::Preset).is_none(),
            "no preset, no field"
        );
        for line in &head.lines {
            assert!(line.width() <= usize::from(inner), "{rows:?}");
        }

        // A picked effort and a Claude alias read as the pickers say them.
        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        let head = header(&app, &launch, inner);
        assert!(
            cells_of(&head, BoxField::Model).is_some_and(|c| c.ends_with("opus · latest ⌘/")),
            "{:?}",
            rows_of(&head)
        );
        assert!(
            cells_of(&head, BoxField::Effort).is_some_and(|c| c.ends_with("effort high ⌘Y")),
            "{:?}",
            rows_of(&head)
        );
    }

    /// Every key on the header is one the box answers to: the worktree,
    /// model and effort keys are spelled from the live keymap — a rebind
    /// shows at once, an unbound action shows no key at all — and the
    /// project, harness and preset keys from the box's own table, the one
    /// its key arm matches.
    #[test]
    fn the_header_keys_come_from_the_keymap_and_the_box_table() {
        let mut app = App::new();
        let mut launch = a_launch();
        launch.preset = Some(Box::new(a_preset("reviewer")));
        let head = header(&app, &launch, 200);
        let key = |head: &BoxHeader, field| cells_of(head, field).unwrap_or_default();
        assert!(key(&head, BoxField::Project)
            .ends_with(&format!(" {}", super::super::task_keys::PROJECT.label())));
        assert!(key(&head, BoxField::Agent)
            .ends_with(&format!(" {}", super::super::task_keys::AGENT.label())));
        assert!(key(&head, BoxField::Preset)
            .ends_with(&format!(" {}", super::super::task_keys::PRESET.label())));
        for (field, action) in [
            (BoxField::Worktree, Action::SelectLaunchWorktree),
            (BoxField::Model, Action::SelectModel),
            (BoxField::Effort, Action::CycleEffort),
        ] {
            let want = crate::hints::key(&app.keymap, action).expect("bound by default");
            assert!(
                key(&head, field).ends_with(&format!(" {want}")),
                "{field:?}: {:?}",
                key(&head, field)
            );
        }

        // Rebound: the header follows. Unbound: no key, still a field.
        app.keymap = crate::keymap::Keymap::from_overrides(&std::collections::BTreeMap::from([
            ("cycle_effort".into(), "f7".into()),
            ("select_model".into(), String::new()),
        ]));
        let head = header(&app, &launch, 200);
        assert!(
            key(&head, BoxField::Effort).ends_with(" F7"),
            "{:?}",
            rows_of(&head)
        );
        assert_eq!(
            key(&head, BoxField::Model),
            "model default",
            "{:?}",
            rows_of(&head)
        );
    }

    /// The effort is shown for every harness that has one — `default`
    /// while nothing is picked — and left off for one that has none, which
    /// OpenCode is: no field, no button, no key promising a step.
    #[test]
    fn the_effort_is_always_shown_where_the_harness_has_one() {
        let app = App::new();
        let cfg = crate::config::Config::default();
        let target = crate::quick_prompt::QuickTarget::Worktree(orion_core::WorktreeId("w".into()));
        for kind in [
            orion_core::AgentKind::Claude,
            orion_core::AgentKind::Codex,
            orion_core::AgentKind::Grok,
        ] {
            let launch = QuickLaunch::of_kind(target.clone(), kind, None, None, None, &cfg);
            let effort = cells_of(&header(&app, &launch, 200), BoxField::Effort);
            assert!(
                effort
                    .as_deref()
                    .is_some_and(|e| e.starts_with("effort default")),
                "{kind:?}: {effort:?}"
            );
        }
        let open = QuickLaunch::of_kind(
            target,
            orion_core::AgentKind::OpenCode,
            None,
            None,
            None,
            &cfg,
        );
        let head = header(&app, &open, 200);
        assert!(
            cells_of(&head, BoxField::Effort).is_none(),
            "{:?}",
            rows_of(&head)
        );
        assert!(
            !rows_of(&head).concat().contains("effort"),
            "{:?}",
            rows_of(&head)
        );
    }

    /// A CLAUDE ACCOUNT is named in the box's `agent` field as its chip's
    /// rows name it — its name, else the email it is signed in as — and,
    /// once its usage has been read, what is left on it beside: the share
    /// on its emptiest limit and the time to that limit's reset, in the
    /// usage modal's colours — red once it is at its limit.
    #[test]
    fn the_boxs_agent_field_names_the_account_and_its_headroom() {
        with_two_accounts("b@b.co", || {
            let cfg = crate::config::Config::load();
            let target =
                crate::quick_prompt::QuickTarget::Worktree(orion_core::WorktreeId("w".into()));
            let launch = QuickLaunch::of_kind(
                target,
                orion_core::AgentKind::Claude,
                Some("claude-2".into()),
                None,
                None,
                &cfg,
            );
            let mut app = App::new();
            let head = box_header(&app, &launch, &cfg, 200, Theme::default());
            let agent = cells_of(&head, BoxField::Agent).unwrap_or_default();
            assert_eq!(agent, "agent   b@b.co Tab", "{:?}", rows_of(&head));

            let now = orion_core::clock::now_secs() as i64;
            let reading = |used_pct: f64| crate::usage::Reading {
                plan: None,
                main: crate::usage::Row {
                    label: String::new(),
                    cells: [
                        Some(crate::usage::Cell {
                            used_pct,
                            resets_at: Some(now + 2 * 3600 + 600),
                        }),
                        None,
                        None,
                        None,
                    ],
                },
                sub: Vec::new(),
                extra: None,
                fetched_at: now,
            };
            app.usage.readings.insert("claude-2".into(), reading(59.0));
            let head = box_header(&app, &launch, &cfg, 200, Theme::default());
            let agent = cells_of(&head, BoxField::Agent).unwrap_or_default();
            assert_eq!(agent, "agent   b@b.co 41% · 2h Tab", "{:?}", rows_of(&head));
            let left = head.lines[1]
                .spans
                .iter()
                .find(|span| span.content == "41% · 2h")
                .expect("the headroom");
            assert_eq!(left.style.fg, Some(Theme::default().ok));

            app.usage.readings.insert("claude-2".into(), reading(100.0));
            let head = box_header(&app, &launch, &cfg, 200, Theme::default());
            let agent = cells_of(&head, BoxField::Agent).unwrap_or_default();
            assert_eq!(agent, "agent   b@b.co at limit · 2h Tab");

            // Short of room the headroom goes, whole, before the name is cut.
            let head = box_header(&app, &launch, &cfg, 24, Theme::default());
            let agent = cells_of(&head, BoxField::Agent).unwrap_or_default();
            assert_eq!(agent, "agent   b@b.co Tab", "{:?}", rows_of(&head));
        });
    }

    /// Every field the header draws hands back the cells it was drawn in,
    /// and those cells hold exactly that field's own text — so a click on
    /// `agent Claude Tab` cannot open the model list — and whatever the
    /// width, no row overruns it and no key is ever cut: a field gives up
    /// its value's letters, then its word, then its place, before that.
    #[test]
    fn every_field_hands_back_its_own_cells_at_every_width() {
        let app = App::new();
        let mut launch = a_launch();
        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        launch.preset = Some(Box::new(a_preset("reviewer")));
        for width in 8..=140u16 {
            let head = header(&app, &launch, width);
            let rows = rows_of(&head);
            for line in &head.lines {
                assert!(line.width() <= usize::from(width), "{width}: {rows:?}");
            }
            for (field, ..) in &head.fields {
                let cells = cells_of(&head, *field).unwrap();
                let key = match field {
                    BoxField::Project => "⌘P",
                    BoxField::Worktree => "⌘.",
                    BoxField::Agent => "Tab",
                    BoxField::Model => "⌘/",
                    BoxField::Effort => "⌘Y",
                    BoxField::Mode => "⇧Tab",
                    BoxField::Preset => "⌘U",
                };
                assert!(
                    cells.ends_with(&format!(" {key}")),
                    "{width}: {field:?} is {cells:?}, which does not end in its whole key"
                );
                assert!(
                    !cells.starts_with(' '),
                    "{width}: {field:?} is {cells:?}, which reaches into the air beside it"
                );
            }
            if width >= 60 {
                // Claude has plan, so its box draws the mode field too.
                assert_eq!(head.fields.len(), 7, "{width}: every field fits: {rows:?}");
                assert!(
                    !rows.concat().contains('…'),
                    "{width}: nothing cut: {rows:?}"
                );
            }
        }
    }

    /// Two projects, one session in each: `api` with a `feat` checkout,
    /// `web` with one of its own. Both sessions start idle, so nothing
    /// sweeps until a test says it does.
    fn a_tree() -> App {
        use orion_core::{
            Agent, AgentId, AgentKind, AgentStatus, Project, ProjectId, Worktree, WorktreeId,
        };
        let mut app = App::new();
        app.tree.projects = ["api", "web"]
            .iter()
            .enumerate()
            .map(|(i, name)| Project {
                id: ProjectId(format!("p{i}")),
                name: (*name).into(),
                repo_path: format!("/tmp/{name}").into(),
                sort_order: 0,
            })
            .collect();
        app.tree.worktrees = (0..2)
            .map(|i| Worktree {
                id: WorktreeId(format!("w{i}")),
                project_id: ProjectId(format!("p{i}")),
                path: format!("/tmp/w{i}").into(),
                branch: "feat".into(),
                is_main: false,
                sort_order: 0,
            })
            .collect();
        app.tree.agents = (0..2)
            .map(|i| Agent {
                id: AgentId(format!("a{i}")),
                worktree_id: WorktreeId(format!("w{i}")),
                name: format!("s{i}"),
                status: AgentStatus::Finished,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: true,
                recent_prompts: Vec::new(),
                usage_limit: None,
            })
            .collect();
        app
    }

    /// `a_tree` with `count` sessions in `api` instead of one, so a grid
    /// can be given more cards than the body has room for.
    fn a_crowded_tree(count: usize) -> App {
        use orion_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        let mut app = a_tree();
        let one = app.tree.agents[0].clone();
        app.tree.agents = (0..count)
            .map(|i| Agent {
                id: AgentId(format!("api{i}")),
                worktree_id: WorktreeId("w0".into()),
                name: format!("session {i}"),
                status: AgentStatus::Finished,
                kind: AgentKind::Claude,
                sort_order: i as i64,
                ..one.clone()
            })
            .collect();
        app
    }

    /// Aim the SESSIONS cursor at the session with this id, whichever
    /// row of the panels' list it happens to be on.
    fn aim_at(app: &mut App, id: &str) {
        app.set_sel_session(
            app.visible_session_rows()
                .iter()
                .position(|row| matches!(row, crate::app::SessionRow::Agent(a) if a.id.0 == id))
                .expect("a row for the session"),
        );
    }

    /// The whole view: a body too short for every entry says so in the
    /// header, and says it
    /// about exactly the entries the grid left off. This is what a PANE
    /// dragged up over the grid looks like — the entries that lost their
    /// room are counted rather than simply gone — and the grid's own edge
    /// says it again where the eye looks for the rest.
    #[test]
    fn a_body_too_short_for_its_entries_counts_them_in_the_header() {
        use crate::launcher::{BAND_RULE_H, BELOW_MARK_H, HEAD_H, LIST_ROW_H};
        let mut app = a_crowded_tree(9);
        select(&mut app, "api");
        // The cursor on the grid's first card, the way a project opens:
        // everything missing is under the fold.
        let first = crate::launcher::rows(&app)[0].agent.id.0.clone();
        aim_at(&mut app, &first);
        // The band's rule, room for two entries and the row under them
        // the `↓` marker takes: seven of the nine are under the fold.
        let body = Rect::new(
            0,
            0,
            60,
            HEAD_H + BAND_RULE_H + LIST_ROW_H * 2 + BELOW_MARK_H,
        );
        let lines = drawn_lines(&mut app, body);
        let head = &lines[1];
        assert!(head.contains("9 sessions"), "{head:?}");
        assert!(head.contains("↓ 7 hidden"), "{head:?}");

        // And the grid really did draw two of them: the header is not
        // guessing at a number the drawing disagrees with.
        let drawn = lines
            .iter()
            .filter(|line| line.contains("session "))
            .count();
        assert_eq!(drawn, 2, "two entries on screen, seven hidden: {lines:#?}");

        // The EDGE MARKERS: the row under the entries says what is below;
        // nothing is scrolled off the top, so the row of air over them
        // says nothing.
        let last = lines.last().unwrap();
        assert!(last.contains("↓ 7 more below"), "{last:?}");
        let air = &lines[usize::from(HEAD_H) - 1];
        assert!(air.trim().is_empty(), "nothing above: {air:?}");
    }

    /// Nine sessions and three terminals, the cursor walked down onto the
    /// first terminal in a body too short for the lot: the window scrolls
    /// the least that shows the cursor's entry, the row of air over the
    /// grid says what is above, the row kept under it what is below, and
    /// the header counts both.
    #[test]
    fn the_window_scrolls_the_least_that_shows_the_cursors_entry() {
        use crate::launcher::HEAD_H;
        use orion_core::{TerminalId, TerminalTab, WorktreeId};
        let mut app = a_crowded_tree(9);
        select(&mut app, "api");
        for i in 1..=3 {
            app.tree.terminals.push(TerminalTab {
                id: TerminalId(format!("t{i}")),
                worktree_id: WorktreeId("w0".into()),
                name: format!("term-{i}"),
                sort_order: i,
                alive: true,
                run_command: None,
                setup: false,
                run: Default::default(),
            });
        }
        app.set_sel_session(
            app.visible_session_rows()
                .iter()
                .position(
                    |row| matches!(row, crate::app::SessionRow::Terminal(t) if t.id.0 == "t1"),
                )
                .expect("a row for the terminal"),
        );
        // Ten rows of grid: nine in the window, the last for the marker.
        let body = Rect::new(0, 0, 100, HEAD_H + 10);
        let lines = drawn_lines(&mut app, body);
        assert_eq!(
            app.launcher_scroll, 2,
            "two rows, the least that shows the cursor's entry"
        );
        let head = &lines[1];
        assert!(head.contains("↑↓ 3 hidden"), "{head:?}");
        let air = &lines[usize::from(HEAD_H) - 1];
        assert!(air.contains("↑ 1 more above"), "{air:?}");
        let last = lines.last().unwrap();
        assert!(last.contains("↓ 2 more below"), "{last:?}");
        // The cursor's entry is on the window's last row, just over the
        // marker.
        let bottom = &lines[lines.len() - 2];
        assert!(bottom.contains("term-1"), "{lines:#?}");
    }

    /// A grid with room for every entry keeps the rule it has today —
    /// the marker is a thing that appears, not a thing that is always
    /// there saying zero.
    #[test]
    fn a_grid_that_fits_says_nothing_about_hiding() {
        use crate::launcher::{BAND_RULE_H, HEAD_H, LIST_ROW_H};
        let mut app = a_crowded_tree(2);
        select(&mut app, "api");
        let body = Rect::new(0, 0, 60, HEAD_H + BAND_RULE_H + LIST_ROW_H * 2);
        let lines = drawn_lines(&mut app, body);
        let head = &lines[1];
        assert!(head.contains("2 sessions"), "{head:?}");
        assert!(!head.contains("hidden"), "{head:?}");
    }

    /// A terminal's line opens on its mark — `❯` for a shell, `▶` for a
    /// RUN TERMINAL — and names what runs in it, and a session's prompt
    /// opens on `›`; once a shell is gone `exited` sits at the right.
    #[test]
    fn a_terminal_line_says_what_runs_in_it_and_when_it_has_exited() {
        use orion_core::{TerminalId, TerminalTab, WorktreeId};
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.agents[0].recent_prompts = vec![orion_core::PromptEntry {
            text: "fix the login redirect".into(),
            submitted_at: 0,
        }];
        for (id, name, run) in [
            ("t1", "shell-1", None),
            ("t2", "dev-srv", Some("npm run dev")),
        ] {
            app.tree.terminals.push(TerminalTab {
                id: TerminalId(id.into()),
                worktree_id: WorktreeId("w0".into()),
                name: name.into(),
                sort_order: 0,
                alive: true,
                run_command: run.map(Into::into),
                setup: false,
                run: Default::default(),
            });
        }
        let body = Rect::new(0, 0, 100, 30);
        let row_of = |lines: &[String], name: &str| -> String {
            lines
                .iter()
                .find(|l| l.contains(name))
                .unwrap_or_else(|| panic!("{name}'s line: {lines:?}"))
                .clone()
        };
        let lines = drawn_lines(&mut app, body);
        let shell = row_of(&lines, "shell-1");
        assert!(shell.contains("❯ shell-1"), "{shell:?}");
        assert!(!shell.contains("exited"), "alive: {shell:?}");
        let run = row_of(&lines, "dev-srv");
        assert!(run.contains("▶ dev-srv"), "{run:?}");
        assert!(run.contains("npm run dev"), "{run:?}");
        let prompt = row_of(&lines, "fix the login redirect");
        assert!(prompt.contains("› fix the login redirect"), "{prompt:?}");

        app.tree.terminals[0].alive = false;
        let lines = drawn_lines(&mut app, body);
        assert!(row_of(&lines, "shell-1").contains("exited"), "{lines:#?}");
    }

    /// A RUN TERMINAL in the `api` checkout running `npm run dev`.
    fn a_run(alive: bool) -> orion_core::TerminalTab {
        orion_core::TerminalTab {
            id: orion_core::TerminalId("t1".into()),
            worktree_id: orion_core::WorktreeId("w0".into()),
            name: "run".into(),
            sort_order: 0,
            alive,
            run_command: Some("npm run dev".into()),
            setup: false,
            run: Default::default(),
        }
    }

    /// A run winding down from its stop says so at the right end of its
    /// line, counting down to the kill in the working gold.
    #[test]
    fn a_stopping_run_counts_down_to_its_kill() {
        let th = Theme::default();
        let mut run = a_run(true);
        assert_eq!(
            terminal_badge(&run, 0, th),
            ("running".into(), Style::default().fg(th.dim))
        );
        run.run.stopping_until = Some(112_000);
        let (badge, style) = terminal_badge(&run, 0, th);
        assert_eq!(badge, "stopping… 1:52");
        assert_eq!(style.fg, Some(th.warn));
        assert_eq!(terminal_badge(&run, 111_500, th).0, "stopping… 0:01");
        assert_eq!(terminal_badge(&run, 200_000, th).0, "stopping… 0:00");

        let mut app = a_tree();
        select(&mut app, "api");
        run.run.stopping_until = Some(crate::app::now_ms() + 60_000);
        app.tree.terminals.push(run);
        let lines = drawn_lines(&mut app, Rect::new(0, 0, 100, 30));
        let line = lines.iter().find(|l| l.contains("▶ run")).unwrap();
        assert!(
            line.trim_end().ends_with("stopping… 1:00")
                || line.trim_end().ends_with("stopping… 0:59"),
            "{line:?}"
        );
    }

    /// The RUN LINE fits the list's columns: its command sits where a
    /// session's prompt does — not under the model — and its status at
    /// the far right, where the session's age is.
    #[test]
    fn the_run_line_puts_its_command_with_the_prompts_and_its_status_at_the_right() {
        let mut app = a_tree();
        select(&mut app, "api");
        let a = &mut app.tree.agents[0];
        a.model = Some("opus".into());
        a.status_changed_at = crate::app::now_ms() - 5 * 60_000;
        a.recent_prompts = vec![orion_core::PromptEntry {
            text: "fix the login redirect".into(),
            submitted_at: 0,
        }];
        app.tree.terminals.push(a_run(true));
        let lines = drawn_lines(&mut app, Rect::new(0, 0, 100, 30));
        let line_of = |needle: &str| {
            lines
                .iter()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle}: {lines:#?}"))
                .clone()
        };
        let col = |line: &str, needle: &str| line[..line.find(needle).unwrap()].chars().count();
        let session = line_of("fix the login redirect");
        let run = line_of("▶ run");
        assert_eq!(
            col(&run, "$ npm run dev"),
            col(&session, "› fix the login redirect"),
            "the command in the prompts' column:\n{session}\n{run}"
        );
        assert!(!run[..run.find("$ npm").unwrap()].contains("npm"), "{run}");
        assert!(run.trim_end().ends_with("running"), "{run:?}");
        assert_eq!(
            run.trim_end().chars().count(),
            session.trim_end().chars().count(),
            "the status ends where the age does:\n{session}\n{run}"
        );
    }

    /// A run line's ports keep their room on a narrow line: the command
    /// is what gives way.
    #[test]
    fn a_long_command_gives_way_to_the_ports() {
        let mut app = a_tree();
        select(&mut app, "api");
        let mut run = a_run(true);
        run.run_command = Some("docker compose up --build --remove-orphans api web worker".into());
        run.run.ports = vec![3000, 5173];
        app.tree.terminals.push(run);
        let lines = drawn_lines(&mut app, Rect::new(0, 0, 60, 30));
        let line = lines.iter().find(|l| l.contains("▶ run")).unwrap();
        assert!(line.contains("…  :3000 :5173"), "{line:?}");
        assert!(line.trim_end().ends_with("running"), "{line:?}");
    }

    /// A run that ended with nobody stopping it says how at the right
    /// end of its line: a faint `done` for a clean exit, a red `exited N`
    /// for any other, a faint `stopped` when the DAEMON never saw it end.
    /// Nothing is copper any more, a dead shell's `exited` included.
    #[test]
    fn a_run_that_ended_on_its_own_says_how() {
        let th = Theme::default();
        let mut run = a_run(false);
        for (code, said, color) in [
            (Some(0), "done", th.faint),
            (Some(1), "exited 1", th.err),
            (Some(137), "exited 137", th.err),
            (None, "stopped", th.faint),
        ] {
            run.run.exit_code = code;
            let (badge, style) = terminal_badge(&run, 0, th);
            assert_eq!((badge.as_str(), style.fg), (said, Some(color)), "{code:?}");
        }
        let shell = orion_core::TerminalTab {
            run_command: None,
            ..a_run(false)
        };
        assert_eq!(
            terminal_badge(&shell, 0, th),
            ("exited".into(), Style::default().fg(th.faint))
        );

        let mut app = a_tree();
        select(&mut app, "api");
        run.run.exit_code = Some(2);
        app.tree.terminals.push(run);
        let lines = drawn_lines(&mut app, Rect::new(0, 0, 100, 30));
        let line = lines.iter().find(|l| l.contains("▶ run")).unwrap();
        assert!(line.trim_end().ends_with("exited 2"), "{line:?}");
    }

    /// A SETUP LINE looks like a run line — its command with the prompts,
    /// its status at the far right — and says `setting up…` in the
    /// working gold while it runs, then a red `setup failed (1)`.
    #[test]
    fn a_setup_line_says_setting_up_then_how_it_failed() {
        let th = Theme::default();
        let mut setup = orion_core::TerminalTab {
            name: "setup".into(),
            run_command: Some("just setup-worktree".into()),
            setup: true,
            ..a_run(true)
        };
        assert_eq!(
            terminal_badge(&setup, 0, th),
            ("setting up…".into(), Style::default().fg(th.warn))
        );
        setup.alive = false;
        for (code, said) in [(Some(1), "setup failed (1)"), (None, "setup failed")] {
            setup.run.exit_code = code;
            let (badge, style) = terminal_badge(&setup, 0, th);
            assert_eq!((badge.as_str(), style.fg), (said, Some(th.err)), "{code:?}");
        }

        let mut app = a_tree();
        select(&mut app, "api");
        setup.alive = true;
        app.tree.terminals.push(setup);
        let lines = drawn_lines(&mut app, Rect::new(0, 0, 100, 30));
        let line = lines.iter().find(|l| l.contains("▶ setup")).unwrap();
        assert!(line.contains("$ just setup-worktree"), "{line:?}");
        assert!(line.trim_end().ends_with("setting up…"), "{line:?}");
    }

    /// A session's line says what it runs on — the model and the
    /// reasoning effort it was launched with, `opus high` — and a session
    /// on the CLI's own default effort ends at the model.
    #[test]
    fn a_session_line_names_its_effort_beside_the_model() {
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.agents[0].model = Some("opus".into());
        app.tree.agents[0].effort = Some("high".into());
        let body = Rect::new(0, 0, 100, 30);
        let line = |app: &mut App| {
            drawn_lines(app, body)
                .into_iter()
                .find(|l| l.contains("s0"))
                .expect("the session's line")
        };
        let runs = line(&mut app);
        assert!(runs.contains("opus high"), "what it runs on: {runs:?}");

        app.tree.agents[0].effort = None;
        let runs = line(&mut app);
        assert!(
            runs.contains("opus") && !runs.contains("high"),
            "the CLI's default effort is unnamed: {runs:?}"
        );
    }

    /// Every session's line carries its last prompt after what it runs on
    /// — there is no setting to leave it off any more.
    #[test]
    fn every_line_shows_its_last_prompt() {
        let mut app = a_tree();
        select(&mut app, "api");
        app.tree.agents[0].recent_prompts = vec![orion_core::PromptEntry {
            text: "fix the login redirect".into(),
            submitted_at: 0,
        }];
        let body = Rect::new(0, 0, 100, 30);
        let shows = |lines: &[String]| lines.iter().any(|l| l.contains("fix the login redirect"));

        let lines = drawn_lines(&mut app, body);
        assert!(shows(&lines), "{lines:#?}");
        let line = lines
            .iter()
            .find(|l| l.contains("s0"))
            .expect("the session's line");
        assert!(line.contains("› fix the login redirect"), "{line:?}");
    }

    /// A session sent at a todo names it after its prompt, a dim `☐` — a
    /// `☑` once it is ticked — and a long prompt leaves the todo its
    /// share of the line rather than crowding it out. A session sent at
    /// none shows only its prompt.
    #[test]
    fn a_session_line_names_the_todo_it_came_from() {
        let mut app = a_tree();
        select(&mut app, "api");
        let prompt = |text: &str| orion_core::PromptEntry {
            text: text.into(),
            submitted_at: 0,
        };
        app.tree.agents[0].recent_prompts = vec![prompt("fix the login redirect")];
        let mut file = crate::todos::TodoFile::new(std::path::Path::new("/tmp/api"));
        let inbox = file.inbox();
        let item = file.add_item(inbox, "redirects lose the query", crate::todos::today());
        file.item_mut(item).unwrap().agent = Some(app.tree.agents[0].id.0.clone());
        app.todos.insert("/tmp/api".into(), file);
        let body = Rect::new(0, 0, 120, 30);
        let line = |app: &mut App| -> String {
            drawn_lines(app, body)
                .into_iter()
                .find(|l| l.contains("s0"))
                .expect("the session's line")
        };

        let row = line(&mut app);
        let (prompt_at, todo_at) = (
            row.find("› fix the login redirect").expect(&row),
            row.find("☐ redirects lose the query").expect(&row),
        );
        assert!(prompt_at < todo_at, "after its prompt: {row:?}");

        app.tree.agents[0].recent_prompts = vec![prompt(&"a very long prompt ".repeat(12))];
        let row = line(&mut app);
        assert!(row.contains("☐ redirects lose"), "its share kept: {row:?}");

        let file = app.todos.get_mut(std::path::Path::new("/tmp/api")).unwrap();
        file.toggle(item, crate::todos::now());
        assert!(line(&mut app).contains("☑ redirects"));

        app.tree.agents[0].id = orion_core::AgentId("someone-else".into());
        assert!(!line(&mut app).contains("redirects"), "sent at nothing");
    }

    /// A session stopped on a usage limit says so on its line: `limit
    /// reached` where its age goes, in the needs-you red, and Claude's own
    /// words where its prompt goes. A limit the row still records once it
    /// has left red says nothing.
    #[test]
    fn a_session_at_a_usage_limit_names_it_and_the_way_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"harnesses": {"claude-b": {"label": "Claude B", "program": "claude",
                "hooks": "claude", "resume_flag": "--resume",
                "env": {"CLAUDE_CONFIG_DIR": "~/.claude-b"}}}}"#,
        )
        .unwrap();
        crate::config::with_config_path(path, || {
            let mut app = a_tree();
            select(&mut app, "api");
            let th = app.theme;
            app.tree.agents[0].status = orion_core::AgentStatus::NeedsFeedback;
            app.tree.agents[0].recent_prompts = vec![orion_core::PromptEntry {
                text: "fix the login redirect".into(),
                submitted_at: 0,
            }];
            app.tree.agents[0].usage_limit = Some(orion_core::UsageLimit {
                reason: orion_core::LimitReason::RateLimit,
                message: Some("session limit · resets 3:45pm".into()),
            });
            let body = Rect::new(0, 0, 100, 30);
            let lines = drawn_lines(&mut app, body);
            let row = lines
                .iter()
                .position(|l| l.contains("s0"))
                .expect("the session's line");
            assert!(lines[row].contains("limit reached"), "{:?}", lines[row]);
            assert!(
                lines[row].contains("session limit · resets 3:45pm"),
                "{:?}",
                lines[row]
            );
            assert!(
                !lines[row].contains("fix the login redirect"),
                "Claude's words stand where the prompt was: {:?}",
                lines[row]
            );
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| draw(f, &mut app, body)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let x = lines[row].find("limit reached").unwrap();
            let x = lines[row][..x].chars().count() as u16;
            assert_eq!(
                buf.cell((x, row as u16)).unwrap().fg,
                th.err,
                "the label is the needs-you red"
            );

            app.tree.agents[0].status = orion_core::AgentStatus::Running;
            let lines = drawn_lines(&mut app, body);
            assert!(!lines.iter().any(|l| l.contains("limit reached")));
            assert!(lines.iter().any(|l| l.contains("fix the login redirect")));
        });
    }

    /// A machine with two Claude accounts — the default one, unnamed and
    /// signed in as nobody yet, and `claude-2`, signed in as `email` — and
    /// `f` run with the tags read, so the default account's chip is `C`
    /// and the other's the first letter of `email`.
    pub(crate) fn with_two_accounts<T>(email: &str, f: impl FnOnce() -> T) -> T {
        let home = tempfile::tempdir().unwrap();
        let two = home.path().join(".claude-2");
        std::fs::create_dir_all(&two).unwrap();
        std::fs::write(
            two.join(".claude.json"),
            serde_json::json!({"oauthAccount": {"emailAddress": email, "accountUuid": "u"}})
                .to_string(),
        )
        .unwrap();
        let config = home.path().join("config.json");
        std::fs::write(
            &config,
            serde_json::json!({"claude_accounts": [
                {"id": "claude-2", "config_dir": two.display().to_string()}
            ]})
            .to_string(),
        )
        .unwrap();
        let places = crate::paths_util::Places::in_home(home.path());
        crate::paths_util::with_places(places, || {
            crate::config::with_config_path(config, || {
                crate::claude_accounts::refresh_now();
                f()
            })
        })
    }

    /// A LIST row says the model and effort a session runs on and never
    /// the CLAUDE ACCOUNT it runs as — the pane's breadcrumb keeps the
    /// account — and with no model named says the harness, an account's
    /// as `claude`.
    #[test]
    fn a_list_row_names_the_model_and_effort_not_the_account() {
        with_two_accounts("b@b.co", || {
            let mut a = a_tree().tree.agents[0].clone();
            a.kind = orion_core::AgentKind::Claude;
            a.custom_harness = Some("claude-2".into());
            a.model = Some("opus".into());
            a.effort = Some("high".into());
            assert!(harness_line(&a, &mut None).contains("b@b.co"));
            assert_eq!(list_runs_line(&a, &mut None), "opus high");
            a.model = None;
            assert_eq!(list_runs_line(&a, &mut None), "claude high");
        });
    }

    /// With two Claude accounts every LIST row wears its account's chip
    /// right after the status dot — its letter in a filled chip, then air
    /// before the name; with one, no row does.
    #[test]
    fn rows_wear_their_accounts_chip_with_two_accounts() {
        with_two_accounts("rk@b.co", || {
            let mut app = a_tree();
            select(&mut app, "api");
            app.tree.agents[0].kind = orion_core::AgentKind::Claude;
            app.tree.agents[0].custom_harness = Some("claude-2".into());
            let body = Rect::new(0, 0, 100, 30);
            let lines = drawn_lines(&mut app, body);
            let line = lines
                .iter()
                .find(|l| l.contains("s0"))
                .expect("the session's line");
            assert!(line.contains(" R  s0"), "{line:?}");
        });
        let mut app = a_tree();
        select(&mut app, "api");
        let lines = drawn_lines(&mut app, Rect::new(0, 0, 100, 30));
        let line = lines.iter().find(|l| l.contains("s0")).unwrap();
        assert!(!line.contains(" R "), "one account, no chip: {line:?}");
    }

    /// The chip is the account's letter, black and bold, on three cells of
    /// its tint, in every theme but `mono`, which keeps colour for the
    /// statuses and draws it inverse — black on its text gray; an
    /// archived session's chip is dimmed to the quiet it is drawn in.
    #[test]
    fn the_chip_is_black_on_its_tint_in_every_theme_and_inverse_in_mono() {
        with_two_accounts("rk@b.co", || {
            let body = Rect::new(0, 0, 100, 30);
            for name in crate::theme::THEMES {
                let mut app = a_tree();
                app.theme = Theme::by_name(name);
                select(&mut app, "api");
                app.tree.agents[0].kind = orion_core::AgentKind::Claude;
                app.tree.agents[0].custom_harness = Some("claude-2".into());
                let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
                    body.width,
                    body.height,
                ))
                .unwrap();
                terminal.draw(|f| draw(f, &mut app, body)).unwrap();
                let buf = terminal.backend().buffer().clone();
                let lines = drawn_lines(&mut app, body);
                let row = lines.iter().position(|l| l.contains(" R  s0")).unwrap();
                let at = lines[row].find(" R  s0").unwrap();
                let x = lines[row][..at].chars().count() as u16;
                let th = app.theme;
                let fill = if *name == "mono" {
                    th.text
                } else {
                    crate::theme::ACCOUNT_TINTS[1]
                };
                for dx in 0..CHIP_W as u16 {
                    let cell = buf.cell((x + dx, row as u16)).unwrap();
                    assert_eq!(cell.bg, fill, "{name}: cell {dx} of the chip");
                }
                let mark = buf.cell((x + 1, row as u16)).unwrap();
                assert_eq!(mark.fg, Color::Black, "{name}");
                assert!(mark.modifier.contains(Modifier::BOLD), "{name}");
                let after = buf.cell((x + CHIP_W as u16, row as u16)).unwrap();
                assert_ne!(after.bg, fill, "{name}: air between the chip and the name");

                let mut archived = app.tree.agents[0].clone();
                archived.archived = true;
                let spans = session_chip(&app, &archived);
                let chip = spans.iter().find(|s| s.content == " R ").unwrap();
                assert_eq!(
                    chip.style.bg,
                    Some(th.dim),
                    "{name}: an archived chip is dim"
                );
            }
        });
    }

    /// Every row of `body` as drawn with the view on it.
    fn drawn_lines(app: &mut App, body: Rect) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(body.width, body.height))
                .unwrap();
        terminal.draw(|f| draw(f, app, body)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..body.height)
            .map(|y| {
                (0..body.width)
                    .filter_map(|x| buf.cell((x, y)))
                    .map(|c| c.symbol().to_string())
                    .collect()
            })
            .collect()
    }

    /// `a_tree` with `count` checkouts in `api` instead of one, a session
    /// in each, so the grid can be given more bands than the body has
    /// room for.
    fn a_tree_with_worktrees(count: usize) -> App {
        use orion_core::{Agent, AgentId, AgentKind, AgentStatus, Worktree, WorktreeId};
        let mut app = a_tree();
        let one = app.tree.agents[0].clone();
        app.tree.worktrees = (0..count)
            .map(|i| Worktree {
                id: WorktreeId(format!("wt{i}")),
                project_id: orion_core::ProjectId("p0".into()),
                path: format!("/tmp/wt{i}").into(),
                branch: format!("feat-{i}"),
                is_main: false,
                sort_order: i as i64,
            })
            .collect();
        app.tree.agents = (0..count)
            .map(|i| Agent {
                id: AgentId(format!("api{i}")),
                worktree_id: WorktreeId(format!("wt{i}")),
                name: format!("session {i}"),
                status: AgentStatus::Finished,
                kind: AgentKind::Claude,
                sort_order: i as i64,
                ..one.clone()
            })
            .collect();
        app
    }

    /// Put the cursor on the band of checkout `id`, at the band level.
    fn aim_at_band(app: &mut App, id: &str) {
        app.set_sel_worktree(
            app.worktree_row_of(&orion_core::WorktreeId(id.into()))
                .expect("a row for the checkout"),
        );
    }

    /// The air under the last whole band says how many more are down
    /// there — in that air, under the bands, not over them — and once
    /// the walk down has brought the last one on screen the cue is the
    /// header's row of air pointing back up at the ones scrolled off.
    #[test]
    fn the_air_under_the_bands_says_how_many_more_are_below() {
        use crate::launcher::{BAND_RULE_H, GAP_Y, HEAD_H, LIST_ROW_H};
        let mut app = a_tree_with_worktrees(9);
        select(&mut app, "api");
        aim_at_band(&mut app, "wt0");
        // Room for two bands of a line each, the gap after them, and the
        // marker's row.
        let band_h = BAND_RULE_H + LIST_ROW_H;
        let bands_end = HEAD_H + band_h * 2 + GAP_Y;
        let body = Rect::new(0, 0, 60, bands_end + 2);
        let lines = drawn_lines(&mut app, body);
        let cue: Vec<usize> = (0..lines.len())
            .filter(|&y| lines[y].contains("more below"))
            .collect();
        assert_eq!(cue.len(), 1, "{lines:#?}");
        // Seven bands, an entry each, are past the two on screen.
        assert!(
            lines[cue[0]].contains("↓ 7 more below"),
            "{:?}",
            lines[cue[0]]
        );
        assert!(cue[0] >= bands_end as usize, "under the bands: {lines:#?}");

        // Walked to the last band, nothing is left below to point at: the
        // seven scrolled off the top are counted on the row of air under
        // the tabs instead.
        aim_at_band(&mut app, "wt8");
        let lines = drawn_lines(&mut app, body);
        assert!(
            lines.iter().all(|line| !line.contains("more below")),
            "{lines:#?}"
        );
        assert!(
            lines[usize::from(HEAD_H) - 1].contains("↑ 7 more above"),
            "{lines:#?}"
        );
    }

    /// A grid with every band on screen leaves its air empty, however
    /// much of it there is.
    #[test]
    fn a_grid_that_fits_leaves_its_air_empty() {
        use crate::launcher::{BAND_RULE_H, GAP_Y, HEAD_H, LIST_ROW_H};
        let mut app = a_tree_with_worktrees(2);
        select(&mut app, "api");
        aim_at_band(&mut app, "wt0");
        let body = Rect::new(
            0,
            0,
            60,
            HEAD_H + (BAND_RULE_H + LIST_ROW_H) * 2 + GAP_Y + 6,
        );
        let lines = drawn_lines(&mut app, body);
        assert!(
            lines.iter().all(|line| !line.contains("more below")),
            "{lines:#?}"
        );
    }

    /// Put the PROJECTS cursor on the named project. The cursor is a row
    /// index and a turn starting anywhere reorders the rows, so a test
    /// that starts one has to say again which project the grid is on.
    fn select(app: &mut App, name: &str) {
        app.set_sel_project(
            app.project_rows()
                .iter()
                .position(|i| app.tree.projects[*i].name == name)
                .expect("a row for the project"),
        );
    }

    /// The HIDDEN MARKER: a grid that holds every card says nothing, and
    /// one that does not says how many it left off and which way they
    /// went — so a PANE dragged up over the cards reads as the pane
    /// taking their room rather than as sessions going missing.
    #[test]
    fn the_header_says_how_many_cards_it_could_not_fit() {
        let app = App::new();
        let th = app.theme;
        let mark = |hidden| row_text(&count_spans(&app, 9, hidden, 80, th));

        assert_eq!(mark(Hidden::default()), "9 sessions", "nothing is missing");
        assert_eq!(
            mark(Hidden { above: 0, below: 5 }),
            "9 sessions  ↓ 5 hidden",
            "under the fold: step down to them"
        );
        assert_eq!(
            mark(Hidden { above: 4, below: 0 }),
            "9 sessions  ↑ 4 hidden"
        );
        assert_eq!(
            mark(Hidden { above: 2, below: 3 }),
            "9 sessions  ↑↓ 5 hidden",
            "both ways at once, counted together"
        );
    }

    /// The counts get what the tabs leave them and give way whole, least
    /// needed first — the count of the cards, then the PR COUNT — with the
    /// HIDDEN MARKER last, and never run past their room.
    #[test]
    fn the_counts_give_way_a_piece_at_a_time() {
        let mut app = a_tree();
        select(&mut app, "api");
        let id = app.selected_project().expect("api").id.clone();
        let now = std::time::Instant::now();
        app.open_prs.insert(
            id,
            crate::app::OpenPrs {
                list: vec![crate::pull_request::OpenPr {
                    number: 7,
                    title: "Tabs vanish on a narrow window".into(),
                    url: "https://github.com/o/r/pull/7".into(),
                    answered_draft: false,
                    answered: Default::default(),
                    head: "tabs".into(),
                    mine: false,
                    head_sha: String::new(),
                    meta: Default::default(),
                }],
                merged: Vec::new(),
                at: now,
                due: now,
                step: std::time::Duration::ZERO,
            },
        );
        let th = app.theme;
        let hidden = Hidden { above: 0, below: 5 };
        let mark_w = "  ↓ 5 hidden".chars().count();
        for width in 0..=120usize {
            let spans = count_spans(&app, 9, hidden, width, th);
            let text = row_text(&spans);
            let used: usize = spans.iter().map(|s| s.width()).sum();
            assert!(used <= width, "{width}: {text:?} overruns");
            let words = text.contains("9 sessions");
            let badge = text.contains("1 pr");
            let mark = text.contains("5 hidden");
            assert_eq!(mark, width >= mark_w, "{width}: {text:?}");
            assert!(!badge || mark, "{width}: the marker went first: {text:?}");
            assert!(!words || badge, "{width}: the counts went first: {text:?}");
        }
        assert_eq!(
            row_text(&count_spans(&app, 9, hidden, 120, th)),
            "9 sessions  1 pr  ↓ 5 hidden",
            "a column of air more between the count and the badge"
        );
    }

    /// [`head_count`]'s spans without the buttons they carry.
    fn count_spans(
        app: &App,
        count: usize,
        hidden: Hidden,
        width: usize,
        th: Theme,
    ) -> Vec<Span<'static>> {
        let count = HeadCount {
            sessions: count,
            terminals: 0,
        };
        head_count(app, count, hidden, width, th)
            .into_iter()
            .map(|(span, _)| span)
            .collect()
    }

    /// The row as one string, the way it lands on screen.
    fn row_text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// `a_tree` on `api`, with both projects open: `web` opened last, so
    /// its tab leads.
    fn a_tabbed_tree() -> App {
        let mut app = a_tree();
        select(&mut app, "api");
        app.launcher_tabs = vec![ProjectId("p1".into()), ProjectId("p0".into())];
        app
    }

    use orion_core::ProjectId;

    /// The header's buttons, in the order they were laid down.
    fn head_hits(app: &App) -> Vec<HitTarget> {
        app.hits.iter().map(|(_, h)| h.clone()).collect()
    }

    /// The header is a `+` and then the open projects as tabs, the newest
    /// opened next to it — where the `+` puts the next one — each with its
    /// own `×`: no `orion`, no trail. The tab the grid is on is lit: a raised
    /// chip with its name in the accent, where the rest sit flat.
    #[test]
    fn the_header_is_the_open_projects_as_tabs() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        let th = app.theme;
        app.hits.clear();
        let spans = head_tabs(&mut app, r);
        let text = row_text(&spans);
        assert_eq!(text, " web ×   api ×   + ");
        let (web, api) = (ProjectId("p1".into()), ProjectId("p0".into()));
        assert_eq!(
            head_hits(&app),
            vec![
                HitTarget::LauncherTab(web.clone()),
                HitTarget::LauncherTabClose(web),
                HitTarget::LauncherTab(api.clone()),
                HitTarget::LauncherTabClose(api.clone()),
                HitTarget::LauncherTabAdd,
            ]
        );
        // Each tab's rect is its own name and never the `×` beside it.
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, h)| *h == HitTarget::LauncherTab(api.clone()))
            .expect("api's tab");
        let cells: Vec<char> = text.chars().collect();
        let tab: String = cells[rect.x as usize..(rect.x + rect.width) as usize]
            .iter()
            .collect();
        assert_eq!(tab, " api", "the name, and never the × beside it");

        let lit = spans.iter().find(|s| s.content == "api").expect("api");
        assert_eq!(lit.style.fg, Some(th.accent));
        assert_eq!(lit.style.bg, Some(th.sel_bg), "the lit tab is a chip");
        // A tab with nothing in it that wants you sits back, dim.
        let flat = spans.iter().find(|s| s.content == "web").expect("web");
        assert_eq!(flat.style.fg, Some(th.dim));
        assert_eq!(flat.style.bg, None);

        // With nothing open, the `+` says what it does.
        app.launcher_tabs.clear();
        app.hits.clear();
        let spans = head_tabs(&mut app, r);
        assert_eq!(row_text(&spans), " + open a project ");
        assert_eq!(head_hits(&app), vec![HitTarget::LauncherTabAdd]);
    }

    /// A tab's STATUS MARKS: the mark and a count per state its project's
    /// sessions are in, right of the name, in the one order — needs-you
    /// crimson `●`, done `●`, working gold spinner (still, with the
    /// animations off) — with no word of its own. A quiet project is its
    /// bare name.
    #[test]
    fn a_tab_carries_its_projects_status_dots() {
        use orion_core::{AgentId, AgentStatus};
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        let th = app.theme;
        // api: one asking, one mid-turn; web: one finished unread.
        let mut asking = app.tree.agents[0].clone();
        asking.id = AgentId("a9".into());
        asking.status = AgentStatus::NeedsFeedback;
        app.tree.agents.push(asking);
        app.tree.agents[0].status = AgentStatus::Running;
        app.tree.agents[1].unseen = true;
        app.animations = false;
        select(&mut app, "api");
        app.hits.clear();
        let spans = head_tabs(&mut app, r);
        assert_eq!(row_text(&spans), " web ●1 ×   api ●1 ◐1 ×   + ");
        let dots: Vec<(String, Option<Color>)> = spans
            .iter()
            .filter(|s| s.content.contains(['●', '◐', '✕']))
            .map(|s| (s.content.to_string(), s.style.fg))
            .collect();
        assert_eq!(
            dots,
            vec![
                (" ●1".to_string(), Some(th.done)),
                (" ●1".to_string(), Some(th.err)),
                (" ◐1".to_string(), Some(th.warn)),
            ]
        );
    }

    /// A lone tab carries its `×` like any other: closing it goes back
    /// to the splash (`event_loop::launcher::close_tab`).
    #[test]
    fn a_lone_tab_has_a_cross() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        app.launcher_tabs = vec![ProjectId("p0".into())];
        let spans = head_tabs(&mut app, r);
        assert_eq!(row_text(&spans), " api ×   + ");
        assert!(head_hits(&app)
            .iter()
            .any(|h| matches!(h, HitTarget::LauncherTabClose(_))));

        let mut app = a_tabbed_tree();
        app.hits.clear();
        let spans = head_tabs(&mut app, r);
        assert_eq!(row_text(&spans), " web ×   api ×   + ");
    }

    /// The tabs sweep in place: whatever the work under them is doing, the
    /// names spell the same names in the same columns — a project with an
    /// unread finish has its name recolored a cell at a time
    /// ([`tab_ramp`]), never moved.
    #[test]
    fn the_tabs_sweep_in_place_whatever_is_unread() {
        use orion_core::AgentStatus;
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        app.hits.clear();
        head_tabs(&mut app, r);
        let quiet = std::mem::take(&mut app.hits);
        for a in &mut app.tree.agents {
            a.status = AgentStatus::Finished;
            a.unseen = true;
        }
        select(&mut app, "api");
        let busy = head_tabs(&mut app, r);
        assert!(row_text(&busy).contains(" web"), "{:?}", row_text(&busy));
        assert!(
            !busy.iter().any(|s| s.content == "web"),
            "swept a cell at a time"
        );
        let busy_hits = std::mem::take(&mut app.hits);
        let first_tab = |hits: &[(Rect, HitTarget)]| {
            hits.iter()
                .find(|(_, h)| matches!(h, HitTarget::LauncherTab(_)))
                .map(|(r, h)| (r.x, h.clone()))
        };
        assert_eq!(
            first_tab(&quiet),
            first_tab(&busy_hits),
            "the first tab starts where it did"
        );
    }

    /// A tab sweeps red only for the seconds after something there starts
    /// needing you (the one-shot), and otherwise shimmers in the done color
    /// for as long as a finish there is unread; work in progress, and a
    /// question already announced, hold still — the marks say those.
    #[test]
    fn a_tab_sweeps_for_a_fresh_alarm_or_an_unread_finish_only() {
        let th = Theme::default();
        let tally = |needs_you, running, done, alarm| Tally {
            needs_you,
            done,
            running,
            alarm,
            ..Tally::default()
        };
        assert_eq!(tab_ramp(tally(1, 3, 2, true), th), Some(th.err_sweep));
        assert_eq!(tab_ramp(tally(1, 3, 2, false), th), Some(th.done_sweep));
        assert_eq!(
            tab_ramp(tally(1, 3, 0, false), th),
            None,
            "announced already"
        );
        assert_eq!(
            tab_ramp(tally(0, 1, 0, false), th),
            None,
            "working is the spinner"
        );
        assert_eq!(tab_ramp(tally(0, 0, 0, false), th), None);
    }

    /// The pointer marks the button it rests on: a tab's name underlines,
    /// its `×` turns red, the `+` underlines — and nothing else does.
    #[test]
    fn the_pointer_marks_the_tab_it_rests_on() {
        let r = Rect::new(0, 0, 80, 1);
        let mut app = a_tabbed_tree();
        let th = app.theme;
        let web = ProjectId("p1".into());
        let underlined = |spans: &[Span<'static>]| -> Vec<String> {
            spans
                .iter()
                .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
                .map(|s| s.content.to_string())
                .collect()
        };
        assert!(underlined(&head_tabs(&mut app, r)).is_empty());

        app.hover_crumb = Some(HitTarget::LauncherTab(web.clone()));
        assert_eq!(underlined(&head_tabs(&mut app, r)), ["web"]);

        app.hover_crumb = Some(HitTarget::LauncherTabAdd);
        assert_eq!(underlined(&head_tabs(&mut app, r)), ["+"]);

        app.hover_crumb = Some(HitTarget::LauncherTabClose(web));
        let spans = head_tabs(&mut app, r);
        assert!(underlined(&spans).is_empty());
        let crosses: Vec<Option<Color>> = spans
            .iter()
            .filter(|s| s.content.contains('×'))
            .map(|s| s.style.fg)
            .collect();
        assert_eq!(crosses, [Some(th.err), Some(th.muted)], "web's is red");
    }

    /// More tabs than the row holds: the ones it has no room for go into
    /// the MORE CHIP, which counts them, rather than being drawn half;
    /// every tab is either drawn or in the chip, the lit tab is always
    /// drawn — whole wherever there is room — the `+` is never pushed
    /// off, and nothing overruns the row at any width.
    #[test]
    fn tabs_that_do_not_fit_go_into_the_more_chip() {
        use orion_core::Project;
        let mut app = a_tree();
        for i in 2..9 {
            app.tree.projects.push(Project {
                id: ProjectId(format!("p{i}")),
                name: format!("project-number-{i}"),
                repo_path: format!("/tmp/p{i}").into(),
                sort_order: 0,
            });
        }
        // Every project open, `api` — the oldest opened — last.
        app.launcher_tabs = (0..9).rev().map(|i| ProjectId(format!("p{i}"))).collect();
        select(&mut app, "api");
        for width in 5..=160u16 {
            let r = Rect::new(0, 0, width, 1);
            app.hits.clear();
            let spans = head_tabs(&mut app, r);
            let text = row_text(&spans);
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(used <= width as usize, "{width}: {text:?} overruns");
            assert!(text.contains('+'), "{width}: the + went: {text:?}");
            let drawn = head_hits(&app)
                .iter()
                .filter(|h| matches!(h, HitTarget::LauncherTab(_)))
                .count();
            let more = app.launcher_tabs_more.len();
            if drawn + more > 0 {
                assert_eq!(drawn + more, 9, "{width}: a tab went missing: {text:?}");
            }
            if more > 0 {
                assert!(
                    text.contains(&format!("{more} more ▾")),
                    "{width}: {text:?}"
                );
                assert!(head_hits(&app).contains(&HitTarget::LauncherTabMore));
            }
            if width >= 24 {
                assert!(
                    text.contains(" api × "),
                    "{width}: the lit tab went: {text:?}"
                );
            }
        }
        // Wide enough for all of them, no chip.
        let r = Rect::new(0, 0, 400, 1);
        let text = row_text(&head_tabs(&mut app, r));
        assert!(!text.contains("more"), "{text:?}");
        assert!(app.launcher_tabs_more.is_empty());
    }

    /// The tabs give way in steps, each giving up less than the next: the
    /// `×` off every tab but the lit one, then tabs from the right into the
    /// MORE CHIP, then the lit tab's name, down to [`TAB_NAME_MIN`]; then
    /// the chip alone, holding every tab, and at last nothing. The lit tab
    /// keeps its place among the tabs kept.
    #[test]
    fn the_tabs_give_way_in_steps() {
        let size = |name: usize| TabSize {
            label: 1 + name,
            cross: 3,
            name,
        };
        // Four tabs, the third lit; a chip is ten columns, whatever it holds.
        let sizes = [size(5), size(5), size(10), size(5)];
        let fit = |budget| fit_tabs(&sizes, Some(2), budget, |_| 10);
        let tabs = |shown: &[usize], crosses, lit_name, more: &[usize]| TabFit {
            shown: shown.to_vec(),
            crosses,
            lit_name,
            more: more.to_vec(),
        };
        // 29 of labels, 12 of crosses, 3 of air.
        assert_eq!(fit(44), tabs(&[0, 1, 2, 3], true, 10, &[]));
        assert_eq!(fit(43), tabs(&[0, 1, 2, 3], false, 10, &[]));
        assert_eq!(fit(35), tabs(&[0, 1, 2, 3], false, 10, &[]));
        assert_eq!(fit(34), tabs(&[0, 2], false, 10, &[1, 3]));
        assert_eq!(fit(25), tabs(&[2], false, 10, &[0, 1, 3]));
        assert_eq!(fit(24), tabs(&[2], false, 9, &[0, 1, 3]));
        assert_eq!(fit(18), tabs(&[2], false, 3, &[0, 1, 3]));
        assert_eq!(fit(17), tabs(&[], false, 10, &[0, 1, 2, 3]));
        assert_eq!(fit(9), tabs(&[], false, 10, &[]));
        // With no tab lit, the first ones are the ones kept.
        let unlit = fit_tabs(&sizes, None, 31, |_| 10);
        assert_eq!(unlit, tabs(&[0, 1], false, 0, &[2, 3]));
    }

    /// The MORE CHIP carries the STATUS DOTS of the tabs it stands in for,
    /// so a project the row had no room for still says it wants you — and
    /// the header remembers which projects those are, for the chip's list.
    #[test]
    fn the_more_chip_carries_the_dots_of_the_tabs_it_holds() {
        use orion_core::AgentStatus;
        let mut app = a_tabbed_tree();
        let th = app.theme;
        let web = ProjectId("p1".into());
        for p in &mut app.tree.projects {
            if p.id == web {
                p.name = "website-frontend".into();
            }
        }
        // web: one waiting on you.
        app.tree.agents[1].status = AgentStatus::NeedsFeedback;
        select(&mut app, "api");
        app.hits.clear();
        let spans = head_tabs(&mut app, Rect::new(0, 0, 30, 1));
        let text = row_text(&spans);
        assert!(!text.contains("website"), "{text:?}");
        assert!(text.contains(" api × "), "the lit tab, whole: {text:?}");
        assert!(text.contains("1 more ●1 ▾"), "{text:?}");
        let dot = spans.iter().find(|s| s.content == " ●1").expect("a dot");
        assert_eq!(dot.style.fg, Some(th.err), "web's red, on the chip");
        assert_eq!(app.launcher_tabs_more, vec![web]);
        assert!(head_hits(&app).contains(&HitTarget::LauncherTabMore));
    }

    /// Squeezed, the tabs that are not lit give up their `×` before any
    /// tab gives way: the lit one keeps its own.
    #[test]
    fn a_crowded_row_keeps_only_the_lit_tabs_cross() {
        let mut app = a_tabbed_tree();
        let web = ProjectId("p1".into());
        for p in &mut app.tree.projects {
            if p.id == web {
                p.name = "website-frontend".into();
            }
        }
        app.hits.clear();
        let text = row_text(&head_tabs(&mut app, Rect::new(0, 0, 30, 1)));
        assert_eq!(text, " website-frontend  api ×   + ");
        assert!(!head_hits(&app).contains(&HitTarget::LauncherTabClose(web)));
        assert!(app.launcher_tabs_more.is_empty());
    }

    /// The worktree field is the WORKTREE PICKER's button, and its value
    /// and key are what the picker hangs from: the cells handed back are
    /// exactly the branch and its key, inside the field's own. A PR
    /// SESSION's branch is no button at all and wears no key: its checkout
    /// is the DAEMON's to pick.
    #[test]
    fn the_worktree_field_hands_back_the_branch_button() {
        let app = App::new();
        for width in 12..=120u16 {
            let head = header(&app, &a_launch(), width);
            let rows = rows_of(&head);
            let (row, x, w) = head.branch.unwrap_or_else(|| panic!("{width}: {rows:?}"));
            let under: String = rows[usize::from(row)]
                .chars()
                .skip(usize::from(x))
                .take(usize::from(w))
                .collect();
            assert!(under.ends_with(" ⌘."), "{width}: {under:?}");
            assert!(under.starts_with("(w"), "{width}: {under:?}");
            let (_, frow, fx, fw) = head
                .fields
                .iter()
                .find(|(f, ..)| *f == BoxField::Worktree)
                .copied()
                .expect("a drawn branch is a field");
            assert!(
                frow == row && fx <= x && x + w <= fx + fw,
                "{width}: the branch sits inside its field"
            );
        }

        let pr = a_launch().with_pr(Some(crate::pull_request::PrLaunch {
            url: "https://github.com/o/r/pull/7".into(),
            head: "fix-nav".into(),
            number: 7,
        }));
        let head = header(&app, &pr, 120);
        let rows = rows_of(&head);
        assert!(rows[0].contains("worktree fix-nav"), "{rows:?}");
        assert!(!rows[0].contains("^T"), "{rows:?}");
        assert_eq!(head.branch, None);
        assert!(cells_of(&head, BoxField::Worktree).is_none(), "{rows:?}");
    }

    /// A launch into a fresh worktree reads `new worktree <branch>` in the
    /// green the box's frame turns — the word and the branch both — and
    /// the box's own toggle for it is gone: the picker's first row is the
    /// way to one.
    #[test]
    fn a_fresh_worktree_reads_new_worktree_in_green() {
        let th = Theme::default();
        let app = App::new();
        let mut launch = a_launch();
        launch.target = crate::quick_prompt::QuickTarget::NewWorktree {
            project: orion_core::ProjectId("p".into()),
            branch: "yellow-fox-jumps".into(),
            existing: false,
        };
        let head = header(&app, &launch, 200);
        let rows = rows_of(&head);
        assert!(
            rows[0].contains("new worktree yellow-fox-jumps ⌘."),
            "{rows:?}"
        );
        assert!(
            !rows.concat().contains("[ ]") && !rows.concat().contains("^N"),
            "{rows:?}"
        );
        let span = |text: &str| {
            head.lines[0]
                .spans
                .iter()
                .find(|s| s.content.trim_end() == text)
                .unwrap_or_else(|| panic!("no {text:?} in {rows:?}"))
                .style
                .fg
        };
        assert_eq!(span("new worktree"), Some(th.ok));
        assert_eq!(span("yellow-fox-jumps"), Some(th.ok));
    }

    /// An AGENT PRESET on the launch has a field of its own, on the row of
    /// where the launch runs, with the `⇧Tab` that picks another — and the
    /// bottom border drops its own `⇧Tab`, the header carrying it now.
    #[test]
    fn a_preset_has_a_field_of_its_own() {
        let app = App::new();
        let mut launch = a_launch();
        let border = |launch: &QuickLaunch| {
            crate::hints::text(
                &super::super::task_hints(&crate::app::PromptKind::QuickPrompt(launch.clone())),
                200,
            )
        };
        assert!(border(&launch).contains("⌘U preset"), "{}", border(&launch));
        launch.preset = Some(Box::new(a_preset("reviewer")));
        let head = header(&app, &launch, BOX_SIZE.0 - 4);
        assert_eq!(
            cells_of(&head, BoxField::Preset).as_deref(),
            Some("preset reviewer ⌘U"),
            "{:?}",
            rows_of(&head)
        );
        assert_eq!(
            head.fields
                .iter()
                .find(|(f, ..)| *f == BoxField::Preset)
                .map(|(_, row, ..)| *row),
            Some(0),
            "on the row of where it runs"
        );
        assert!(
            !border(&launch).contains(&super::super::task_keys::PRESET.label()),
            "{}",
            border(&launch)
        );
    }

    /// One checkout's BAND, with one session in it, for the rule tests.
    fn a_band(is_main: bool, branch: &str) -> crate::launcher::Band {
        use orion_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        crate::launcher::Band {
            worktree: WorktreeId("w1".into()),
            branch: branch.into(),
            is_main,
            pr: None,
            cards: vec![crate::launcher::Card::Session(LauncherRow {
                agent: Agent {
                    id: AgentId("a1".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "fix login".into(),
                    status: AgentStatus::Finished,
                    archived: false,
                    archived_at: 0,
                    unseen: false,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: 0,
                    alive: true,
                    recent_prompts: Vec::new(),
                    usage_limit: None,
                },
                project: "orion".into(),
                branch: branch.into(),
                pr: None,
            })],
            archived: Vec::new(),
            drawer_open: false,
            dormant: false,
        }
    }

    /// A band's rule drawn on one row `width` wide, as the buffer: a
    /// band the cursor is not on, with every card on its row.
    fn rule_row(app: &App, band: &crate::launcher::Band, width: u16) -> ratatui::buffer::Buffer {
        rule_row_as(
            app,
            band,
            width,
            BandRule {
                index: 0,
                on: false,
                lit: false,
                cols: RuleColumns::default(),
            },
        )
    }

    /// [`rule_row`] for a band drawn as `rule` says.
    fn rule_row_as(
        app: &App,
        band: &crate::launcher::Band,
        width: u16,
        rule: BandRule,
    ) -> ratatui::buffer::Buffer {
        let area = Rect::new(0, 0, width, 1);
        let facts = rule_facts(app, band, app.theme);
        let rule = BandRule {
            cols: RuleColumns::of([&facts]),
            ..rule
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|f| {
                draw_band_rule(f.buffer_mut(), app, area, band, &facts, rule);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// The cells of a buffer's row `y` painted in `want`, as text.
    fn painted(buf: &ratatui::buffer::Buffer, y: u16, want: Color) -> String {
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, y)))
            .filter(|c| c.fg == want)
            .map(|c| c.symbol().to_string())
            .collect()
    }

    /// A buffer's row `y` as text.
    fn row_string(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, y)))
            .map(|c| c.symbol().to_string())
            .collect()
    }
    /// A band's rule names its checkout by its SCOPE MARK — `⌂` the root
    /// in muted, `⎇` a worktree in dim — with no color of its own: the mark
    /// sorts the two scopes, so neither spends a hue the statuses need. The
    /// root's branch is bright, being the branch itself; a worktree's
    /// muted. The cards under it say how many run there, so the rule does
    /// not count them.
    #[test]
    fn a_band_names_its_checkout_by_its_scope_mark() {
        let app = App::new();
        let th = app.theme;
        let root = rule_row(&app, &a_band(true, "main"), 44);
        let worktree = rule_row(&app, &a_band(false, "feat-x"), 44);
        assert_eq!(painted(&root, 0, th.muted).trim(), "⌂");
        assert_eq!(painted(&root, 0, th.text), "main");
        assert_eq!(painted(&worktree, 0, th.dim).trim(), "⎇");
        assert_eq!(painted(&worktree, 0, th.muted), "feat-x");
        let text = row_string(&worktree, 0);
        assert!(text.starts_with("── ⎇ feat-x ─"), "{text:?}");
        assert!(!text.contains("session"), "{text:?}");
    }

    /// A worktree checked out in a folder its branch's name doesn't spell
    /// says so after the branch, dim — `feat/ui in pawy` — so the branches
    /// still start in one column; one whose folder is just its branch (`/`
    /// as `-`) says nothing more.
    #[test]
    fn a_band_names_its_folder_when_the_branch_does_not() {
        use orion_core::{ProjectId, Worktree, WorktreeId};
        let mut app = App::new();
        let th = app.theme;
        app.tree.worktrees.push(Worktree {
            id: WorktreeId("w1".into()),
            project_id: ProjectId("p1".into()),
            path: "/repo/pawy".into(),
            branch: "feat/ui".into(),
            is_main: false,
            sort_order: 0,
        });
        let band = a_band(false, "feat/ui");
        let buf = rule_row(&app, &band, 60);
        let text = row_string(&buf, 0);
        assert!(text.starts_with("── ⎇ feat/ui in pawy ─"), "{text:?}");
        assert!(painted(&buf, 0, th.dim).contains(" in pawy"));

        app.tree.worktrees[0].path = "/repo/feat-ui".into();
        let text = row_string(&rule_row(&app, &band, 60), 0);
        assert!(!text.contains(" in "), "{text:?}");
    }

    /// The checkout's git counts sit at the rule's right end, each in its
    /// own color — the files in the diff's modified sand, the lines in its
    /// pastels, the commits ahead and behind muted, never the working gold
    /// — and a rule too narrow for them lets them go before a short branch
    /// gives up a letter.
    #[test]
    fn a_band_counts_its_checkouts_changes_at_its_right_end() {
        use orion_core::WorktreeId;
        let mut app = App::new();
        let th = app.theme;
        let w1 = WorktreeId("w1".into());
        app.worktree_changes
            .insert(w1.clone(), (Some(3), std::time::Instant::now()));
        app.worktree_lines.insert(
            w1.clone(),
            crate::git_diff::LineChanges {
                added: 120,
                removed: 45,
            },
        );
        app.worktree_ahead.insert(w1, (4, 1));
        let band = a_band(false, "feat-x");

        let buf = rule_row(&app, &band, 60);
        let text = row_string(&buf, 0);
        assert!(text.ends_with(" *3 +120 −45 ⇡4 ⇣1 ──"), "{text:?}");
        assert_eq!(painted(&buf, 0, th.modified), "*3");
        assert_eq!(painted(&buf, 0, th.added), "+120");
        assert_eq!(painted(&buf, 0, th.removed), "−45");
        assert_eq!(painted(&buf, 0, th.warn), "", "never the working gold");

        let buf = rule_row(&app, &band, 24);
        let text = row_string(&buf, 0);
        assert!(text.starts_with("── ⎇ feat-x ─"), "narrow: {text:?}");
        assert!(!text.contains('*'), "the counts go first: {text:?}");
    }

    /// An app whose checkout `w1` (at `/src/feat-x`) has a compose stack with
    /// `running` of its 5 containers up, at STACK TRAIL frame `phase`.
    fn with_stack(running: u16, phase: usize) -> App {
        use orion_core::{ProjectId, Worktree, WorktreeId};
        let mut app = App::new();
        app.tree.worktrees.push(Worktree {
            id: WorktreeId("w1".into()),
            project_id: ProjectId("p1".into()),
            path: "/src/feat-x".into(),
            branch: "feat-x".into(),
            is_main: false,
            sort_order: 0,
        });
        app.stacks = Some(vec![orion_core::compose::Stack {
            project: "riplo-wt".into(),
            dirs: vec!["/src/feat-x".into()],
            running,
            total: 5,
        }]);
        let frame = crate::app::SWEEP_FRAME.as_millis() as u64;
        app.splash_epoch = std::time::Instant::now()
            - std::time::Duration::from_millis(phase as u64 * frame + frame / 2);
        app
    }

    /// A running stack's `⬡` takes the rule's last dash — the tail's, past
    /// the counts — in green, and the rule keeps its width.
    #[test]
    fn a_running_stack_marks_the_rules_right_end() {
        let mut app = with_stack(5, 79);
        app.animations = false;
        app.worktree_ahead
            .insert(orion_core::WorktreeId("w1".into()), (4, 0));
        let th = app.theme;
        let band = a_band(false, "feat-x");
        let buf = rule_row(&app, &band, 50);
        let text = row_string(&buf, 0);
        assert!(text.ends_with(" ⇡4 ─⬡"), "{text:?}");
        assert_eq!(text.chars().count(), 50);
        assert_eq!(painted(&buf, 0, th.ok), "⬡");
    }

    /// With no right block the `⬡` takes the fill's last dash; a stopped
    /// stack's is faint.
    #[test]
    fn a_stopped_stack_marks_the_last_fill_dash_faintly() {
        let app = with_stack(0, 0);
        let th = app.theme;
        let buf = rule_row(&app, &a_band(false, "feat-x"), 40);
        let text = row_string(&buf, 0);
        assert!(text.ends_with("──⬡"), "{text:?}");
        assert!(painted(&buf, 0, th.faint).contains('⬡'));
        assert_eq!(painted(&buf, 0, th.ok), "");
    }

    /// No stack, no mark: the rule is the one it always was.
    #[test]
    fn no_stack_leaves_the_rule_alone() {
        let mut app = with_stack(5, 0);
        let band = a_band(false, "feat-x");
        let marked = row_string(&rule_row(&app, &band, 40), 0);
        app.stacks = Some(Vec::new());
        let plain = row_string(&rule_row(&app, &band, 40), 0);
        assert!(!plain.contains('⬡'), "{plain:?}");
        assert!(plain.ends_with("───"));
        assert_eq!(marked.chars().count(), plain.chars().count());
    }

    /// The STACK TRAIL lights dashes only: at the frame its head is over
    /// the branch name nothing there changes colour, and a frame that
    /// puts it on a dash paints that dash the head's shade. The `⬡`
    /// flashes as the light lands, and stays still with animations off.
    #[test]
    fn the_stack_trail_runs_on_dashes_into_the_mark() {
        let band = a_band(false, "feat-x");
        let head = |app: &App| app.theme.stack_sweep[0];
        // `── ⎇ feat-x ───…`: columns 0–1 dashes, `feat-x` at 5–10.
        let app = with_stack(5, 0);
        let buf = rule_row(&app, &band, 40);
        assert_eq!(
            buf[(0, 0)].fg,
            head(&app),
            "the light starts at the left end"
        );
        let app = with_stack(5, 7);
        let buf = rule_row(&app, &band, 40);
        assert_eq!(painted(&buf, 0, head(&app)), "", "over text: nothing lit");
        assert_eq!(buf[(7, 0)].symbol(), "a");
        let app = with_stack(5, 14);
        let buf = rule_row(&app, &band, 40);
        assert_eq!(buf[(14, 0)].fg, head(&app));
        assert_eq!(buf[(14, 0)].symbol(), "─");
        // The `⬡` sits at column 39; the light reaches it at frame 39.
        let app = with_stack(5, 40);
        let buf = rule_row(&app, &band, 40);
        assert_eq!(buf[(39, 0)].symbol(), "⬡");
        assert_eq!(
            buf[(39, 0)].fg,
            app.theme.stack_flash[1],
            "peak of the flash"
        );
        let mut app = with_stack(5, 40);
        app.animations = false;
        let buf = rule_row(&app, &band, 40);
        assert_eq!(buf[(39, 0)].fg, app.theme.ok);
        assert_eq!(painted(&buf, 0, app.theme.stack_sweep[0]), "");
    }

    /// Every rule on the grid gives its pull request and its counts the
    /// same columns ([`RuleColumns`]): a band with no pull request, or a
    /// count the others have and it lacks, leaves those cells blank — so
    /// the numbers line up down the grid, right-aligned, and the rule's
    /// dashes end at the same column on every band.
    #[test]
    fn the_rules_line_their_columns_up_down_the_grid() {
        use orion_core::WorktreeId;
        let mut app = App::new();
        let th = app.theme;
        let now = std::time::Instant::now();
        let mut root = a_band(true, "main");
        root.worktree = WorktreeId("w0".into());
        let mut feat = a_band(false, "feat/auth-tokens");
        feat.pr = Some(crate::launcher::RowPr {
            number: 57,
            title: "Move the token store".into(),
            url: String::new(),
            standing: crate::pull_request::Standing::Open,
            trouble: None,
        });
        app.worktree_changes
            .insert(WorktreeId("w0".into()), (Some(12), now));
        app.worktree_ahead.insert(WorktreeId("w0".into()), (0, 5));
        app.worktree_changes
            .insert(WorktreeId("w1".into()), (Some(3), now));
        app.worktree_lines.insert(
            WorktreeId("w1".into()),
            crate::git_diff::LineChanges {
                added: 60,
                removed: 28,
            },
        );
        let facts = [rule_facts(&app, &root, th), rule_facts(&app, &feat, th)];
        let cols = RuleColumns::of(&facts);
        let draw = |band: &crate::launcher::Band, facts: &RuleFacts| -> Vec<char> {
            let area = Rect::new(0, 0, 80, 1);
            let mut buf = Buffer::empty(area);
            let rule = BandRule {
                index: 0,
                on: false,
                lit: false,
                cols,
            };
            draw_band_rule(&mut buf, &app, area, band, facts, rule);
            row_string(&buf, 0).chars().collect()
        };
        let root_row = draw(&root, &facts[0]);
        let feat_row = draw(&feat, &facts[1]);
        let at = |row: &[char], needle: &str| -> usize {
            let n: Vec<char> = needle.chars().collect();
            row.windows(n.len())
                .position(|w| w == n.as_slice())
                .unwrap()
        };
        let text = |row: &[char]| row.iter().collect::<String>();
        assert!(
            text(&feat_row).contains("↗ #57 ready"),
            "{}",
            text(&feat_row)
        );
        assert!(!text(&root_row).contains('↗'), "{}", text(&root_row));
        // The file counts end in the same column, right-aligned.
        assert_eq!(at(&root_row, "*12") + 3, at(&feat_row, "*3") + 2);
        // The pull request's column is blank on the root, not closed up.
        let pr_at = at(&feat_row, "↗");
        assert_eq!(root_row[pr_at], ' ', "{}", text(&root_row));
        // The dashes stop at the same column on both.
        let dashes_end = |row: &[char]| {
            let start = at(row, "──");
            start + row[start..].iter().take_while(|c| **c == '─').count()
        };
        assert_eq!(dashes_end(&root_row), dashes_end(&feat_row));
    }

    /// The band the keys are on says so: its rule opens on the CURSOR
    /// MARK `❯` in the accent where every other band's opens on `──`. It
    /// spells no key. With the cursor on the band but the keys elsewhere
    /// (up on the PROJECT TABS, down in the pane) the rule is the plain
    /// gray one again.
    #[test]
    fn the_band_the_keys_are_on_marks_itself_and_spells_no_key() {
        let app = App::new();
        let th = app.theme;
        let key = crate::hints::key(&app.keymap, crate::keymap::Action::FocusNext).unwrap();
        let mut band = a_band(true, "main");
        let card = band.cards[0].clone();
        band.cards.extend(std::iter::repeat_n(card, 7));
        let lit = BandRule {
            index: 0,
            on: true,
            lit: true,
            cols: RuleColumns::default(),
        };

        let buf = rule_row_as(&app, &band, 96, lit);
        let text = row_string(&buf, 0);
        assert!(text.starts_with("❯  ⌂ main"), "{text:?}");
        assert!(!text.contains(&key) && !text.contains("expand"), "{text:?}");
        // The mark in the accent, the checkout as on any band.
        let accent = painted(&buf, 0, th.accent);
        assert!(accent.starts_with('❯'), "{accent:?}");
        assert_eq!(painted(&buf, 0, th.text), "main");

        // The cursor's band with the keys elsewhere: bold branch, and
        // that is all — no mark.
        let buf = rule_row_as(
            &app,
            &band,
            96,
            BandRule {
                index: 0,
                on: true,
                lit: false,
                cols: RuleColumns::default(),
            },
        );
        let text = row_string(&buf, 0);
        assert!(text.starts_with("── ⌂ main"), "keys elsewhere: {text:?}");
        assert_eq!(painted(&buf, 0, th.accent), "", "{text:?}");
    }

    /// A narrow rule keeps the cursor mark before the branch gives way.
    #[test]
    fn a_narrow_rule_keeps_the_cursor_mark() {
        let app = App::new();
        let mut band = a_band(false, "feat-x");
        let card = band.cards[0].clone();
        band.cards.extend(std::iter::repeat_n(card, 2));
        let rule = BandRule {
            index: 0,
            on: true,
            lit: true,
            cols: RuleColumns::default(),
        };

        let buf = rule_row_as(&app, &band, 40, rule);
        let text = row_string(&buf, 0);
        assert!(text.starts_with("❯  ⎇ feat-x"), "{text:?}");
    }

    /// The cursor's wash takes its status's color while it wants you —
    /// red asking, the done color unread, red for a crash, cold or not —
    /// and none otherwise: a working, read, fresh, cold or archived
    /// session washes in the accent.
    #[test]
    fn the_cursors_wash_takes_the_status_that_wants_you() {
        use orion_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        let base = Agent {
            id: AgentId("a1".into()),
            worktree_id: WorktreeId("w1".into()),
            name: "fix login".into(),
            status: AgentStatus::Finished,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
        };
        let th = Theme::by_name("amber");
        let wash = |a: Agent| card_edge(&a, a.archived || crate::app::drawn_cold(&a), th);
        let with = |status: AgentStatus, unseen: bool| Agent {
            status,
            unseen,
            ..base.clone()
        };
        assert_eq!(wash(with(AgentStatus::NeedsFeedback, false)), Some(th.err));
        assert_eq!(wash(with(AgentStatus::Finished, true)), Some(th.done));
        assert_eq!(
            wash(with(AgentStatus::Running, false)),
            None,
            "working asks nothing of you: the spinner says it"
        );
        assert_eq!(
            wash(with(AgentStatus::Terminated, false)),
            Some(th.err),
            "a crash wants you"
        );
        assert_eq!(wash(with(AgentStatus::Finished, false)), None);
        assert_eq!(wash(with(AgentStatus::Fresh, false)), None);
        let cold = Agent {
            alive: false,
            ..with(AgentStatus::Running, false)
        };
        assert_eq!(wash(cold), None);
        // A crash's PTY is gone too, but the crash is still true.
        let crashed = Agent {
            alive: false,
            ..with(AgentStatus::Terminated, false)
        };
        assert_eq!(wash(crashed), Some(th.err), "a cold crash still wants you");
        let archived = Agent {
            archived: true,
            ..with(AgentStatus::NeedsFeedback, false)
        };
        assert_eq!(wash(archived), None);
    }

    /// The cursor's entry is washed, very faintly, in its status color —
    /// breathing while it wants you, still in the accent once nothing is
    /// going on — and keeps the wash while the pane has the keys. Every
    /// other entry keeps the terminal's background.
    #[test]
    fn the_cursors_entry_is_washed_faintly_in_its_status_color() {
        use orion_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};
        let row = LauncherRow {
            agent: Agent {
                id: AgentId("a1".into()),
                worktree_id: WorktreeId("w1".into()),
                name: "fix login".into(),
                status: AgentStatus::NeedsFeedback,
                archived: false,
                archived_at: 0,
                unseen: false,
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                session_id: None,
                sort_order: 0,
                status_changed_at: 0,
                alive: true,
                recent_prompts: Vec::new(),
                usage_limit: None,
            },
            project: "orion".into(),
            branch: "feat-x".into(),
            pr: None,
        };
        let th = Theme::by_name("coral");
        let mut app = App::new();
        app.theme = th;
        let area = Rect::new(0, 0, 60, 1);
        let bg = |app: &App, row: &LauncherRow, selected: bool| {
            draw_one(app, area, row, selected).cell((40, 0)).unwrap().bg
        };

        assert_eq!(bg(&app, &row, false), Color::Reset, "only the cursor's");

        // A faint wash of the needs-you crimson, the whole line, while the
        // pane has the keys.
        app.focus = Focus::Terminal;
        let fill = bg(&app, &row, true);
        assert_eq!(
            draw_one(&app, area, &row, true).cell((0, 0)).unwrap().bg,
            fill
        );
        let Color::Rgb(r, g, b) = fill else {
            panic!("{fill:?}")
        };
        assert!(r.max(g).max(b) <= 45, "faint: {fill:?}");
        assert!(r > g && r > b, "reddish: {fill:?}");

        // It breathes between the floor and the peak.
        let levels: Vec<f32> = (0..TINT_BREATH).map(tint_level).collect();
        assert!(levels
            .iter()
            .all(|l| *l >= TINT_FLOOR - 1e-4 && *l <= TINT_PEAK + 1e-4));
        assert!(levels[TINT_BREATH / 2] > levels[0] + 0.1);
        assert_eq!(
            dim_toward_black(Color::Indexed(209), 0.5),
            Color::Rgb(128, 68, 48)
        );

        // A quiet entry holds a still wash of the accent; the animations
        // off hold a live one still at its peak.
        let mut idle = row.clone();
        idle.agent.status = AgentStatus::Fresh;
        assert_eq!(
            bg(&app, &idle, true),
            dim_toward_black(th.accent, TINT_STILL)
        );
        app.animations = false;
        assert_eq!(bg(&app, &row, true), dim_toward_black(th.err, TINT_PEAK));

        // The PROJECT TABS holding the keys fade it further.
        app.launcher_tab_cursor = Some(orion_core::ProjectId("p1".into()));
        assert_eq!(
            bg(&app, &row, true),
            dim_toward_black(th.err, TINT_PEAK * 0.6)
        );
    }

    /// An ARCHIVED session's line is the live one put away rather than the
    /// live one with a word added: the round STATUS DOT gives way to the
    /// square ARCHIVED MARK, the name drops to muted, the color goes with
    /// the rest of it, and the badge counts from when it was filed rather
    /// than from its last turn.
    #[test]
    fn an_archived_line_is_the_live_line_put_away() {
        use orion_core::{Agent, AgentId, AgentKind, AgentStatus, WorktreeId};

        fn card(archived: bool) -> LauncherRow {
            LauncherRow {
                agent: Agent {
                    id: AgentId("a1".into()),
                    worktree_id: WorktreeId("w1".into()),
                    name: "fix login".into(),
                    status: AgentStatus::Finished,
                    archived,
                    // Filed two hours ago, last turn a minute ago: the two
                    // badges cannot be mistaken for one another.
                    archived_at: crate::app::now_ms() - 2 * 3_600_000,
                    unseen: false,
                    kind: AgentKind::Claude,
                    custom_harness: None,
                    model: None,
                    effort: None,
                    session_id: None,
                    sort_order: 0,
                    status_changed_at: crate::app::now_ms() - 60_000,
                    alive: true,
                    recent_prompts: Vec::new(),
                    usage_limit: None,
                },
                project: "orion".into(),
                branch: "main".into(),
                pr: None,
            }
        }

        let th = Theme::default();
        let app = App::new();
        let area = Rect::new(0, 0, 44, 1);
        let drawn = |row: &LauncherRow| draw_one(&app, area, row, false);
        // The line as text, its air trimmed off both ends.
        let line = |buf: &ratatui::buffer::Buffer, y: u16| -> String {
            let row: String = (0..area.width)
                .filter_map(|x| buf.cell((x, y)))
                .map(|c| c.symbol().to_string())
                .collect();
            row.trim_matches(|c| c == '\u{2502}' || c == ' ')
                .to_string()
        };
        // And one row's cells in a single color, for the rows whose whole
        // point is which color they are in.
        let colored = |buf: &ratatui::buffer::Buffer, y: u16, want: Color| -> String {
            (0..area.width)
                .filter_map(|x| buf.cell((x, y)))
                .filter(|c| c.fg == want)
                .map(|c| c.symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        };

        let live = drawn(&card(false));
        let gone = drawn(&card(true));

        // The mark stands where the dot stood — the one part of this a
        // terminal with no color at all still says — the name behind it
        // starting in the same column on both.
        assert!(
            line(&live, 0).starts_with("\u{25cf} fix login"),
            "{:?}",
            line(&live, 0)
        );
        assert!(
            line(&gone, 0).starts_with("\u{25aa} fix login"),
            "{:?}",
            line(&gone, 0)
        );

        // The archived name is muted, never the live line's text color.
        assert_eq!(colored(&gone, 0, th.muted), "fix login");
        assert_eq!(colored(&gone, 0, th.text), "");

        // The badge counts from the archiving, not from the last turn.
        assert!(line(&live, 0).ends_with("1m ago"), "{:?}", line(&live, 0));
        assert!(line(&gone, 0).ends_with("2h ago"), "{:?}", line(&gone, 0));

        // And what it runs on is quiet on both.
        assert!(colored(&live, 0, th.dim).contains("claude"));
        assert!(colored(&gone, 0, th.dim).contains("claude"));
    }
}
