//! The FOOTER: the bar along the bottom of every screen.
//!
//! Left to right it is always the STATUS — which orion this is, `v1.0.0`
//! (a button: a click goes HOME), the BREADCRUMB in the grid's own marks
//! (`orbit-api ⎇ feat/auth-tokens ◐ Token store`, each part a link back
//! to it), `· archived`, `· full screen` or `· home` when you are somewhere
//! other than the grid, and at the right edge the live counts, what
//! Spotify is playing just before them ([`crate::spotify`]). Between
//! the two go KEY HINTS, and only while no modal is up ([`hints`]): a
//! modal's keys are on its own bottom border (`crate::hints::modal_block`),
//! so the footer never repeats or contradicts them. With nothing up, the
//! hints are about what the grid or the pane has under its cursor — and
//! wherever that is not the grid, the first of them is the way back to it.
//! A FLASH takes the hints' place until the next key, in its kind's mark
//! and color ([`flash_spans`]).

use super::{truncate, App, ConnState, Focus, HitTarget};
use crate::app::{CrumbPart, SessionRow};
use crate::flash::{Flash, FlashKind};
use crate::hints::{act, acts, Hint};
use crate::keymap::Action;
use crate::launcher::pane_keys;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// The bottom bar, drawn under the splash and the collapsed view too,
/// with the KEY COMBO DISPLAY on the padding row above it.
pub(super) fn draw_footer(f: &mut Frame, app: &mut App, area: Rect) {
    draw_footer_bar(f, app, area);
    draw_key_combo(f, app, area);
}

/// The KEY COMBO DISPLAY: the last key press
/// and what it did — `j - Move down` — on the footer's padding row at the
/// far left, the one blank row on screen and right where vim keeps
/// `showcmd`. Each key sits in a keycap (the selected-row fill) so it
/// reads across a screen share; the label is plain text. Nothing is drawn
/// once the press has aged out (`key_combo::LINGER`; the loop clears it),
/// so the row stays the breathing space it was.
fn draw_key_combo(f: &mut Frame, app: &App, area: Rect) {
    let Some(combo) = &app.key_combo else {
        return;
    };
    if area.height < 2 || area.width == 0 {
        return;
    }
    let row = Rect {
        y: area.y,
        height: 1,
        ..area
    };
    let th = app.theme;
    let cap = crate::hints::key_style(th)
        .bg(th.sel_bg)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];
    for (i, key) in combo.keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!(" {} ", key.display()), cap));
    }
    if let Some(does) = &combo.does {
        spans.push(Span::styled(" - ", Style::default().fg(th.dim)));
        spans.push(Span::styled(does.as_str(), Style::default().fg(th.text)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), row);
}

/// One part of the BREADCRUMB: its spans, and the part of the grid a
/// click on it goes back to — none for the words between.
struct CrumbSeg {
    part: Option<CrumbPart>,
    spans: Vec<Span<'static>>,
}

/// The BREADCRUMB: where the selection is, in the marks the grid draws it
/// with — the project, the checkout behind its SCOPE MARK (`⌂` the root,
/// `⎇` a worktree, the merged `●`), and the session behind its STATUS MARK
/// in its card's own colors — so the bar and the grid read as one:
/// `orbit-api ⎇ feat/auth-tokens ◐ Token store on sqlite`. Each of the
/// three is a link back to it on the grid ([`CrumbPart`]). Then, when the
/// view is not the grid of live sessions, the word that says what Esc
/// climbs out of: `· archived`, `· full screen`, `· home`.
fn crumb(app: &App) -> Vec<CrumbSeg> {
    let th = app.theme;
    let mut segs = Vec::new();
    if app.splash_showing() && !app.home {
        return segs;
    }
    let Some(project) = app.selected_project() else {
        return segs;
    };
    let gap = || CrumbSeg {
        part: None,
        spans: vec![Span::raw(" ")],
    };
    segs.push(CrumbSeg {
        part: Some(CrumbPart::Project),
        spans: vec![Span::styled(
            truncate(&project.name, 20),
            Style::default().fg(th.muted),
        )],
    });
    if let Some(worktree) = app.selected_worktree() {
        segs.push(gap());
        segs.push(CrumbSeg {
            part: Some(CrumbPart::Worktree),
            spans: vec![
                super::launcher_view::scope_mark(app, &worktree.id, worktree.is_main),
                Span::styled(truncate(&worktree.branch, 24), Style::default().fg(th.text)),
            ],
        });
        if let Some(session) = app.selected_session_row() {
            // A link's crumb is its pull request's number, or its display
            // label — never the raw URL, which would eat the bar.
            let mut badge = None;
            let (mark, name, style) = match &session {
                SessionRow::Agent(a) => {
                    let (mark, style) = super::launcher_view::session_crumb(app, a);
                    (mark, a.name.clone(), style)
                }
                SessionRow::Terminal(t) => (
                    super::launcher_view::terminal_mark(t, th),
                    t.name.clone(),
                    Style::default().fg(th.text),
                ),
                SessionRow::Link(link) => match link.pull_request() {
                    // A pull request in `pr_row::look`'s colours, its
                    // state word after the number as the BAND has it.
                    Some(pr) => {
                        let status = app.prs.status_or_open(&pr.url);
                        let look = crate::pr_row::look(status.standing, status.trouble(), th);
                        badge = Some(Span::styled(
                            format!(" {}", status.word()),
                            Style::default().fg(look.badge),
                        ));
                        (
                            Span::styled("↗ ", Style::default().fg(look.glyph)),
                            format!("#{}", pr.number),
                            Style::default().fg(look.label),
                        )
                    }
                    None => (
                        Span::styled("↗ ", Style::default().fg(th.muted)),
                        link.label(),
                        Style::default().fg(th.text),
                    ),
                },
            };
            segs.push(gap());
            let mut spans = vec![mark, Span::styled(truncate(&name, 24), style)];
            spans.extend(badge);
            segs.push(CrumbSeg {
                part: Some(CrumbPart::Session),
                spans,
            });
        }
    }
    if let Some(place) = place(app) {
        segs.push(CrumbSeg {
            part: None,
            spans: vec![Span::styled(
                format!("  · {place}"),
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            )],
        });
    }
    segs
}

/// A FLASH as the bar draws it: its kind's mark, then the line, in the
/// mark and colors `crate::flash` lists for each kind.
fn flash_spans(app: &App, flash: &Flash) -> Vec<Span<'static>> {
    let th = app.theme;
    let spinner = crate::app::spinner_frame(app.spin_phase());
    let (mark, mark_color, text_color) = match flash.kind {
        FlashKind::Failed => ("✕", th.err, th.err),
        FlashKind::Setup => ("⚠", th.warn, th.warn),
        FlashKind::Done => ("✓", th.ok, th.ok),
        FlashKind::Working => (spinner, th.warn, th.muted),
        FlashKind::Note => ("·", th.muted, th.muted),
    };
    vec![
        Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
        Span::styled(flash.text.clone(), Style::default().fg(text_color)),
    ]
}

/// The view the screen is on, when it is not the grid of live sessions.
fn place(app: &App) -> Option<&'static str> {
    if app.home {
        Some("home")
    } else if app.launcher_active() && app.collapsed {
        Some("full screen")
    } else if app.launcher_grid() && app.show_archived {
        Some("archived")
    } else {
        None
    }
}

/// The footer's KEY HINTS for the screen as it stands — none at all while
/// a modal (or the editor) is up: those keys are on the modal's border.
pub(crate) fn hints(app: &App) -> Vec<Hint> {
    if app.overlay.is_some() || app.vim.is_some() || app.page.is_some() {
        return Vec::new();
    }
    let km = &app.keymap;
    let list: Vec<Option<Hint>> = if app.splash_showing() {
        // The splash spells the ways into a project in its own body; the
        // footer has the rest, and HOME's way back down.
        let mut list = Vec::new();
        if app.home {
            list.push(Some(Hint::new("Esc", "back to the grid").kept()));
        }
        list.extend([
            act(km, Action::CommandPalette, "commands"),
            act(km, Action::Settings, "settings"),
        ]);
        // HOME is where orion is restarted from: the daemon and every
        // session in it, behind a confirm.
        if app.home {
            list.push(act(km, Action::Restart, "restart orion"));
        }
        list.push(act(km, Action::Quit, "quit"));
        list
    } else if !app.launcher_grid() || app.focus == Focus::Terminal {
        pane_hints(app)
    } else if app.launcher_tab_cursor.is_some() {
        // The PROJECT TABS holding the keys (`↑`,`↑` off the top row of
        // cards): walking the header's cursor switches the grid as it
        // goes; Enter and Esc are the ways back down.
        vec![
            acts(
                km,
                &[Action::FocusLeft, Action::FocusRight],
                "switch project",
            ),
            act(km, Action::Activate, "into its cards"),
            act(km, Action::CloseProjectTab, "close tab"),
            Some(Hint::new("Esc", "back to the cards")),
        ]
    } else {
        grid_hints(app)
    };
    list.into_iter().flatten().collect()
}

/// The GRID's hints: what the keys do to the card — or the EMPTY BAND —
/// under the cursor, and the way into the pane beside it.
fn grid_hints(app: &App) -> Vec<Option<Hint>> {
    let km = &app.keymap;
    let bands = crate::launcher::bands(app);
    let band = crate::launcher::band_cursor(app, &bands);
    let card = band.and_then(|b| {
        crate::launcher::card_cursor(app, &bands[b]).and_then(|c| bands[b].cards.get(c))
    });
    let tail = [
        act(km, Action::Palette, "jump"),
        act(km, Action::CommandPalette, "commands"),
        act(km, Action::Home, "home"),
    ];
    if app.show_archived {
        // The ARCHIVED VIEW is a different list with different verbs on
        // it: nothing to attach, prompt or archive there.
        let mut list = vec![
            Some(Hint::new("Esc", "back to the live sessions")),
            act(km, Action::Archive, "unarchive"),
            act(km, Action::Delete, "delete"),
        ];
        list.extend(tail);
        return list;
    }
    let Some(band_at) = band.filter(|_| !app.launcher_unaimed) else {
        let mut list = vec![
            acts(km, &[Action::MoveUp, Action::MoveDown], "pick a card"),
            act(km, Action::QuickPrompt, "new agent"),
            act(km, Action::NewTerminal, "terminal"),
        ];
        list.extend(tail);
        return list;
    };
    let mut list = Vec::new();
    if crate::launcher::drawer_cursor(app, &bands[band_at]).is_some() {
        // A line of the band's ARCHIVED DRAWER: a session put away, with
        // nothing to open or prompt until it is brought back.
        list.extend([
            act(km, Action::Unarchive, "unarchive"),
            act(km, Action::ToggleArchivedDrawer, "hide archived"),
            act(km, Action::Delete, "delete"),
        ]);
        list.extend(tail);
        return list;
    }
    match card {
        Some(crate::launcher::Card::Session(row)) => {
            list.push(act(km, Action::Activate, "open"));
            list.push(into_pane(app, &bands));
            // A session stopped on a usage limit: the way on is the first
            // thing worth naming after opening it.
            if row.agent.limit_reached().is_some() {
                let targets = crate::config::Config::load().continue_targets(&row.agent);
                if let Some(does) = crate::launcher::continue_does(&targets) {
                    list.push(act(km, Action::ContinueOn, &does));
                }
            }
            list.extend([
                act(km, Action::FollowUp, "follow-up"),
                act(km, Action::QuickPrompt, "new agent"),
                act(km, Action::Archive, "archive"),
                act(km, Action::Delete, "delete"),
                act(km, Action::GitDiff, "changes"),
            ]);
        }
        Some(crate::launcher::Card::Terminal(_)) => {
            list.extend([
                act(km, Action::Activate, "open"),
                into_pane(app, &bands),
                act(km, Action::PaneTabs, "next terminal"),
                act(km, Action::QuickPrompt, "new agent"),
                act(km, Action::Delete, "close"),
            ]);
        }
        None => {
            // An EMPTY BAND: a checkout with nothing running in it.
            list.extend([
                into_pane(app, &bands),
                act(km, Action::QuickPrompt, "new agent"),
                act(km, Action::NewTerminal, "terminal"),
            ]);
            if !bands[band_at].is_main {
                list.push(act(km, Action::Delete, "delete worktree"));
            }
        }
    }
    // `⌘⇧S` on the band's checkout: its run or its stack, started or
    // stopped — the same word the card's menu says.
    if !app.is_placeholder_worktree(&bands[band_at].worktree) {
        let does = crate::stacks::menu_label(app, &bands[band_at].worktree).to_lowercase();
        list.push(act(km, Action::ToggleStack, &does));
    }
    // On a card of a linked checkout, the whole worktree in one go —
    // `⌫` on an empty band already names it.
    if card.is_some() && !bands[band_at].is_main {
        list.push(act(km, Action::DeleteWorktree, "delete worktree"));
    }
    // The root's `⇡⇣` count against origin's copy of its own branch — what
    // is unpushed and unpulled — so there `p` and `⇧P` are what brings
    // them down. A linked worktree's count against the base it was cut
    // from, which neither moves.
    if bands[band_at].is_main {
        let (ahead, behind) = app.worktree_ahead_behind(&bands[band_at].worktree);
        if behind > 0 {
            list.push(act(km, Action::PullWorktree, "pull"));
        }
        if ahead > 0 {
            list.push(act(km, Action::PushWorktree, "push"));
        }
    }
    // Tab opens the band in place, or folds it back — the verb the rule
    // under the cursor used to spell at its right end.
    if !app.launcher_all_open && !bands[band_at].cards.is_empty() {
        let open = app.open_band(&bands) == Some(band_at);
        list.push(act(
            km,
            Action::FocusNext,
            if open { "collapse" } else { "expand" },
        ));
    }
    list.extend(tail);
    list
}

/// `→ focus PR`: the key that walks off the grid into the PANE beside
/// it from where the cursor is — only once it would, on the row's last
/// card, so the hint is never a key that just walks the row. With the
/// pane under the cards, Enter on a band that has only a pull request.
fn into_pane(app: &App, bands: &[crate::launcher::Band]) -> Option<Hint> {
    if app.launcher_pane_hidden || app.launcher_unaimed {
        return None;
    }
    app.launcher_split(app.launcher_body).1?;
    let what = if app.previewed_pr().is_some() {
        "focus PR"
    } else if app.previewed_issue().is_some() {
        "focus issue"
    } else if app.term.is_some() || app.previewed_cloud().is_some() {
        "focus pane"
    } else {
        return None;
    };
    let band = crate::launcher::band_cursor(app, bands)?;
    let at = crate::launcher::card_cursor(app, &bands[band]);
    // A pane along the bottom is reached by Enter on a band with nothing
    // on it but what the pane reads (and by a click); `↓` stays the
    // grid's.
    if !app.launcher_pane_side().beside() {
        return (at.is_none() && app.reading_url().is_some())
            .then(|| act(&app.keymap, Action::Activate, what))
            .flatten();
    }
    let stuck = match app.walked_band(bands) {
        Some((_, layout)) => layout.stepped(at, 1, 0).is_none_or(|n| Some(n) == at),
        None => {
            let len = bands[band].cards.len();
            crate::launcher::grid_stepped(at, 1, 0, len, len).is_none_or(|n| Some(n) == at)
        }
    };
    stuck
        .then(|| act(&app.keymap, Action::FocusRight, what))
        .flatten()
}

/// [`act`] for a key pressed from inside a LOCKED PANE, where only a ⌘
/// chord reaches orion — the `^` twins are the agent's there. None in a
/// terminal that never sends ⌘ ([`crate::keymap::cmd_shown`]), or for an
/// action with no ⌘ chord, so the footer never names a key the agent
/// would get instead.
fn from_lock(km: &crate::keymap::Keymap, action: Action, does: &str) -> Option<Hint> {
    let shown = km.shown_first(action)?;
    (crate::keymap::cmd_shown() && shown.mods.contains(crossterm::event::KeyModifiers::SUPER))
        .then(|| act(km, action, does))
        .flatten()
}

/// The PANE's hints — the session in it, full-screen or beside the
/// cards, or the page it reads — led by the way back out.
fn pane_hints(app: &App) -> Vec<Option<Hint>> {
    let km = &app.keymap;
    let exited = app.term.as_ref().is_some_and(|t| t.exited);
    let full = app.launcher_active() && app.collapsed;
    if app.focus != Focus::Terminal {
        // Full screen over a session the keys are not in.
        return vec![
            act(km, Action::ToggleFullScreen, "normal size").map(Hint::kept),
            act(km, Action::Activate, "type into it"),
        ];
    }
    if exited {
        return vec![
            Some(Hint::new("Esc", "back to the grid").kept()),
            Some(Hint::new("Enter", "back to the grid")),
        ];
    }
    if app.term_locked {
        // The keys are the agent's: only the hatches reach orion.
        let back = if full {
            "normal size"
        } else {
            "back to the grid"
        };
        let drag = if app.child_mouse_mode().0 != vt100::MouseProtocolMode::None {
            "to the app (⇧drag: select)"
        } else {
            "select + copy"
        };
        return vec![
            act(km, Action::UnlockTerminal, back).map(Hint::kept),
            Some(pane_keys::AGENT_ESC.hint()),
            act(
                km,
                Action::ToggleFullScreen,
                if full { "normal size" } else { "full screen" },
            ),
            from_lock(km, Action::ClosePane, "close"),
            Some(Hint::new("drag", drag)),
            // ⌘ is read off the Mac's own keyboard, which a TUI on
            // another machine has none of.
            Some(Hint::new(
                if cfg!(target_os = "macos") && !app.is_remote {
                    "⌘click"
                } else {
                    "⌥click"
                },
                "open link",
            )),
        ];
    }
    // Unlocked in the pane: reading it.
    let mut list = vec![Some(pane_keys::BACK.hint().kept())];
    if app.previewed_cloud().is_some() && app.reading_url().is_none() {
        list.push(act(km, Action::Activate, "open in browser"));
        return list;
    }
    if app.previewed_pr().is_some() {
        // The PULL REQUEST PAGE: its tabs, then what Enter and ↑/↓ do on
        // the one showing — open and walk its rows, or open the pull
        // request and scroll its prose. Enter's verb before the arrows: a
        // narrow footer keeps what the row does over how to reach it.
        let tab = app.pr_tabs.tab;
        list.extend([
            Some(crate::pr_preview::keys::TABS.hint()),
            act(km, Action::Activate, tab.act_does()),
            Some(if tab.lists() {
                pane_keys::LINE.hint_as("pick")
            } else {
                pane_keys::LINE.hint()
            }),
            Some(pane_keys::PAGE.hint()),
            act(km, Action::GitDiff, "diff"),
        ]);
    } else if app.pane_reads_page() {
        list.extend([
            Some(pane_keys::LINE.hint()),
            Some(pane_keys::PAGE.hint()),
            act(km, Action::Activate, "open in browser"),
        ]);
    } else {
        list.extend([
            act(km, Action::Activate, "type into it"),
            Some(pane_keys::LINE.hint()),
            Some(pane_keys::PAGE.hint()),
        ]);
    }
    list.push(act(km, Action::CommandPalette, "commands"));
    list
}

/// Draw the bar.
fn draw_footer_bar(f: &mut Frame, app: &mut App, area: Rect) {
    // `area` includes the blank padding row; the bar itself is its last row.
    let area = Rect {
        y: area.y + area.height.saturating_sub(1),
        height: area.height.min(1),
        ..area
    };
    let th = app.theme;
    // Quiet footer: context on the left, live stats on the right. The
    // hostname only earns a slot when it's a remote session, and the
    // connection state only when something is wrong.
    let mut status = vec![Span::raw(" ")];
    if app.is_remote {
        status.push(Span::styled(
            truncate(&app.hostname, 24),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ));
        status.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
    }
    if matches!(app.conn, ConnState::Disconnected) {
        status.push(Span::styled("✗ disconnected", Style::default().fg(th.err)));
        status.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
    }
    let crumbs = crumb(app);
    // Right edge: live session/process counts and orion's total memory
    // footprint, fed by the footer metrics poll. The hints clip before the
    // readout does.
    let usage = footer_usage(app);
    let right_w = usage
        .as_ref()
        .map(|s| s.chars().count() as u16 + 2)
        .unwrap_or(0)
        .min(area.width);
    // Beside it, what Spotify is playing (`crate::spotify`), in at most a
    // third of the bar — and never so much the readout can't keep its
    // room. It gives way before the counts do, and the hints before it.
    let spotify_hover = match app.hover_crumb {
        Some(HitTarget::FooterSpotify(button)) => Some(button),
        _ => None,
    };
    let spot_max = (area.width / 3).min(area.width.saturating_sub(right_w + 2));
    let spot_sweep = app.animations.then(|| app.sweep_phase());
    let spot = app.spotify.as_ref().and_then(|np| {
        crate::spotify::readout(np, usize::from(spot_max), th, spotify_hover, spot_sweep)
    });
    let spot_w = spot.as_ref().map_or(0, |r| r.width);
    let left = Rect {
        width: area
            .width
            .saturating_sub(right_w)
            .saturating_sub(if spot.is_some() { spot_w + 2 } else { 0 }),
        ..area
    };
    // Which orion this is, at the far left: the one thing on the bar that
    // never moves with the cursor, so it reads as a nameplate — and a
    // button, HOME's (`⌘G`), lit under the pointer. A newer published
    // release rides it as `⇡ v0.22.0`, in the heads-up color. A clipped
    // *flash* loses the end of a sentence, so the nameplate steps aside
    // for one that would not otherwise fit.
    let plate = format!("v{}", env!("CARGO_PKG_VERSION"));
    let update = app.update_available.as_ref().map(|v| format!(" ⇡ v{v}"));
    let plate_w = plate.chars().count()
        + update.as_ref().map_or(0, |u| u.chars().count())
        + "  ·  ".chars().count();
    let status_w: usize = status.iter().map(|s| s.width()).sum::<usize>()
        + crumbs
            .iter()
            .flat_map(|seg| &seg.spans)
            .map(|s| s.width())
            .sum::<usize>();
    // The flash, behind the four cells the crumb leaves before it.
    let flash_w = app.flash.as_ref().map_or(0, |f| {
        flash_spans(app, f).iter().map(|s| s.width()).sum::<usize>() + 4
    });
    let show_plate = app.flash.is_none() || status_w + flash_w + plate_w <= left.width as usize;
    let mut spans = status;
    if show_plate {
        let x = left.x + spans.iter().map(|s| s.width() as u16).sum::<u16>();
        let width = plate.chars().count() as u16;
        let hovered = app.hover_crumb == Some(HitTarget::FooterHome);
        let style = footer_button_style(th, hovered);
        push_hit(app, left, x, width, HitTarget::FooterHome);
        spans.push(Span::styled(plate, style));
        if let Some(update) = update {
            // A button too: **Upgrade orion**, shimmering green so a new
            // release is hard to miss.
            let x = left.x + spans.iter().map(|s| s.width() as u16).sum::<u16>();
            let width = update.chars().count() as u16;
            push_hit(app, left, x, width, HitTarget::FooterUpgrade);
            let mut base = Style::default().add_modifier(Modifier::BOLD);
            if app.hover_crumb == Some(HitTarget::FooterUpgrade) {
                base = base.add_modifier(Modifier::UNDERLINED);
            }
            spans.extend(upgrade_spans(app, &update, base));
        }
        spans.push(Span::styled("  ·  ", Style::default().fg(th.dim)));
    }
    if !crumbs.is_empty() {
        let mut x = left.x + spans.iter().map(|s| s.width() as u16).sum::<u16>();
        for seg in crumbs {
            let width: u16 = seg.spans.iter().map(|s| s.width() as u16).sum();
            let mut seg_spans = seg.spans;
            if let Some(part) = seg.part {
                let target = HitTarget::FooterCrumb(part);
                // Under the pointer the words underline — the mark stays
                // as it is — the way every other button on the bar lifts.
                if app.hover_crumb.as_ref() == Some(&target) {
                    if let Some(words) = seg_spans.last_mut() {
                        words.style = words.style.add_modifier(Modifier::UNDERLINED);
                    }
                }
                push_hit(app, left, x, width, target);
            }
            spans.extend(seg_spans);
            x += width;
        }
        spans.push(Span::raw("    "));
    }
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let room = usize::from(left.width).saturating_sub(used + 1);
    match &app.flash {
        Some(flash) => spans.extend(flash_spans(app, flash)),
        None => spans.extend(crate::hints::spans(&hints(app), room, th)),
    }
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    if let Some(spot) = spot {
        let rect = Rect {
            x: area.x + area.width.saturating_sub(right_w + spot_w),
            width: spot_w,
            ..area
        };
        // Each glyph is a button, its target the glyph and the space after
        // it, inside the readout's own cells; the title is only words.
        for (button, col) in spot.buttons {
            let width = col.end - col.start + 1;
            push_hit(
                app,
                rect,
                rect.x + col.start,
                width,
                HitTarget::FooterSpotify(button),
            );
        }
        f.render_widget(Paragraph::new(Line::from(spot.spans)), rect);
    }
    if let Some(usage) = usage {
        let right = Rect {
            x: area.x + area.width.saturating_sub(right_w),
            width: right_w,
            ..area
        };
        // The readout is a button — a click opens the memory modal, as
        // `⇧M` does — and nothing about a dim figure says so, so the
        // pointer on it lifts it to full text and underlines it, as the
        // header's buttons are. Only the words are the target, laid where
        // the right alignment puts them, not the padding beside them.
        let span = Span::styled(
            usage,
            footer_button_style(th, app.hover_crumb == Some(HitTarget::FooterUsage)),
        );
        let width = (span.width() as u16).min(right.width);
        app.hits.push((
            Rect {
                x: right.x + right.width - width,
                width,
                ..right
            },
            HitTarget::FooterUsage,
        ));
        f.render_widget(
            Paragraph::new(Line::from(span)).alignment(ratatui::layout::Alignment::Right),
            right,
        );
    }
}

/// A dim button on the bar — the nameplate, the memory readout, the
/// Spotify glyphs — as it is drawn: lifted to full text and underlined
/// while the pointer is on it (`hovered`), since nothing about a dim word
/// says it can be clicked.
pub(crate) fn footer_button_style(th: crate::theme::Theme, hovered: bool) -> Style {
    if hovered {
        Style::default()
            .fg(th.text)
            .add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().fg(th.dim)
    }
}

/// `target`'s hit rect on the bar: `width` cells from `x`, clipped to
/// `within`, and none at all once `x` is past it — so a button the bar
/// cut short can't take a click meant for its neighbour.
fn push_hit(app: &mut App, within: Rect, x: u16, width: u16, target: HitTarget) {
    if x < within.right() {
        app.hits.push((
            Rect {
                x,
                width: width.min(within.right() - x),
                ..within
            },
            target,
        ));
    }
}

/// The footer's right-edge readout: the agents running and orion's total
/// memory footprint (TUI + daemon + every session's process subtree), `8
/// agents · 44 MB`. Terminals and the prewarm pool's spares count toward
/// the memory but not the agents — the memory modal it opens breaks it
/// all down. None until the first metrics reply arrives.
fn footer_usage(app: &App) -> Option<String> {
    let m = app.last_metrics.as_ref()?;
    let agents = m
        .sessions
        .iter()
        .filter(|s| matches!(s.session, orion_core::SessionRef::Agent(_)) && s.prewarm.is_none())
        .count();
    let total = m.daemon_rss_bytes
        + app.client_rss_bytes
        + m.sessions.iter().map(|s| s.rss_bytes).sum::<u64>();
    Some(format!(
        "{agents} agent{} · {}",
        super::launcher_view::plural(agents),
        super::fmt_mem(total)
    ))
}

/// The green ramp a waiting upgrade shimmers on: the theme's `ok` at rest.
pub(crate) fn upgrade_ramp(th: crate::theme::Theme) -> [ratatui::style::Color; 3] {
    use ratatui::style::Color;
    [th.ok, Color::Indexed(114), Color::Indexed(157)]
}

/// `text` in the upgrade green, sweeping while animations are on.
pub(crate) fn upgrade_spans(app: &App, text: &str, base: Style) -> Vec<Span<'static>> {
    let ramp = upgrade_ramp(app.theme);
    if app.animations {
        crate::ui::sweep_spans(text, base, ramp, app.sweep_phase())
    } else {
        vec![Span::styled(text.to_string(), base.fg(ramp[0]))]
    }
}
