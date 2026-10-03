//! First-run onboarding: pick agents, worktree defaults, Linear, and the
//! outside terminal before the grid. Opened once from `main_loop` while
//! `config.onboarded` is still false; Esc / a click outside skips and
//! stamps the flag so it does not come back.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::config::{program_installed, Config, OutsideTerminal};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::centered_rect;

const PAGES: &[&str] = &[
    "Welcome",
    "Agents",
    "Worktrees",
    "Linear",
    "Terminal",
    "Ready",
];

/// The first-run wizard's live state. `area` is written back on draw so a
/// click outside can dismiss it the same way Esc does.
#[derive(Debug, Clone)]
pub struct OnboardView {
    pub page: usize,
    pub row: usize,
    pub area: Rect,
    pub email: TextInput,
    pub editing_email: bool,
}

impl OnboardView {
    pub fn new(cfg: &Config) -> Self {
        let mut email = TextInput::new();
        email.set_text(&cfg.linear_assignee_email);
        Self {
            page: 0,
            row: 0,
            area: Rect::default(),
            email,
            editing_email: false,
        }
    }
}

/// Open the wizard over the empty grid. Tests that construct an overlay
/// themselves never call this — only `main_loop` does, after `Config::load`.
pub fn open(app: &mut App, cfg: &Config) {
    app.overlay = Some(Overlay::Onboard(OnboardView::new(cfg)));
    app.dirty = true;
}

/// Leave the wizard and remember it was seen, so the next launch goes
/// straight to the grid. A test without a config path does not persist.
pub fn dismiss(app: &mut App) {
    app.overlay = None;
    app.dirty = true;
    persist(|cfg| cfg.onboarded = true);
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Onboard(_)) = &app.overlay else {
        return;
    };
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        if view.editing_email {
            match key.code {
                KeyCode::Esc => {
                    view.email.set_text(&Config::load().linear_assignee_email);
                    view.editing_email = false;
                }
                KeyCode::Enter => {
                    let value = view.email.as_str().trim().to_string();
                    view.editing_email = false;
                    persist(|cfg| cfg.linear_assignee_email = value);
                }
                _ => {
                    view.email.handle_key(&key);
                }
            }
            app.dirty = true;
            return;
        }
    }
    match key.code {
        KeyCode::Esc => dismiss(app),
        KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => next(app),
        KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => prev(app),
        KeyCode::Down | KeyCode::Char('j') => move_row(app, 1),
        KeyCode::Up | KeyCode::Char('k') => move_row(app, -1),
        KeyCode::Enter => activate(app),
        KeyCode::Char(' ') => toggle(app),
        _ => {}
    }
}

pub fn draw(f: &mut Frame, app: &mut App, view: &OnboardView, th: Theme) {
    let area = centered_rect(f.area(), 72, 20);
    let title = format!(
        " Orion  ·  {} ({}/{}) ",
        PAGES[view.page.min(PAGES.len() - 1)],
        view.page + 1,
        PAGES.len()
    );
    let inner = crate::ui::render_modal_frame(f, area, title, th);
    let cfg = Config::load();
    let lines = page_lines(&cfg, view, th);
    let budget = inner.height as usize;
    for (i, line) in lines.into_iter().take(budget).enumerate() {
        let Some(row) = crate::ui::row_rect(inner, i) else {
            break;
        };
        f.render_widget(Paragraph::new(line), row);
    }
    if let Some(Overlay::Onboard(live)) = &mut app.overlay {
        live.area = area;
    }
}

fn page_lines(cfg: &Config, view: &OnboardView, th: Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(th.dim);
    let text = Style::default().fg(th.text);
    let accent = Style::default().fg(th.accent);
    match view.page {
        0 => vec![
            Line::from(Span::styled(
                " Welcome to Orion",
                accent.add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " A few choices so the grid only offers what you use.",
                text,
            )),
            Line::from(Span::styled(
                " Agents stay off until you turn them on here or in Settings.",
                dim,
            )),
            Line::from(""),
            Line::from(Span::styled(
                " Enter continue   Esc skip (you can change all of this later)",
                dim,
            )),
        ],
        1 => agent_lines(cfg, view.row, th),
        2 => toggle_page(
            view.row,
            th,
            &[
                (
                    "Link .env into worktrees",
                    cfg.link_env_files,
                    "symlink the main checkout's .env* into each worktree",
                ),
                (
                    "New agent in a new worktree",
                    cfg.quick_prompt_new_worktree,
                    "⌘N cuts a worktree first (off = current checkout)",
                ),
            ],
        ),
        3 => {
            let email = if view.editing_email {
                format!("{}▌", view.email.as_str())
            } else if cfg.linear_assignee_email.is_empty() {
                "owner of LINEAR_API_KEY".into()
            } else {
                cfg.linear_assignee_email.clone()
            };
            let mut lines = vec![
                Line::from(Span::styled(
                    " Linear",
                    accent.add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    " ⌘L lists issues assigned to you. Key comes from the project's .env.",
                    dim,
                )),
                Line::from(""),
            ];
            lines.push(setting_row(
                0,
                view.row,
                "Assignee",
                &email,
                th,
                view.editing_email && view.row == 0,
            ));
            lines.push(setting_row(
                1,
                view.row,
                "Auto-attach PRs",
                on_off(cfg.linear_auto_attach),
                th,
                false,
            ));
            lines
        }
        4 => {
            let term = cfg.outside_terminal();
            vec![
                Line::from(Span::styled(
                    " Terminal",
                    accent.add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                setting_row(
                    0,
                    view.row,
                    "Outside terminal",
                    term.as_str(),
                    th,
                    false,
                ),
                setting_row(
                    1,
                    view.row,
                    "Ghostty keybinds",
                    on_off(cfg.ghostty_keybinds),
                    th,
                    false,
                ),
                Line::from(""),
                Line::from(Span::styled(
                    " Ghostty keeps ⌘⇧P for itself unless Orion writes an unbind.",
                    dim,
                )),
                Line::from(Span::styled(
                    " Reload Ghostty (⌘⇧,) after this so the chords reach Orion.",
                    dim,
                )),
            ]
        }
        _ => vec![
            Line::from(Span::styled(
                " You're set",
                accent.add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " Settings (⌘,) changes any of this later.",
                text,
            )),
            Line::from(Span::styled(
                " Turn on an agent in Settings → Agents to start a session.",
                dim,
            )),
            Line::from(""),
            Line::from(Span::styled(" Enter start using Orion", dim)),
        ],
    }
}

fn agent_lines(cfg: &Config, selected: usize, th: Theme) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            " Agents",
            Style::default()
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Space toggles. Enter cycles the model. Off stays out of the picker.",
            Style::default().fg(th.dim),
        )),
        Line::from(""),
    ];
    for (i, entry) in cfg.harness_registry().into_iter().enumerate() {
        let found = program_installed(&entry.program);
        let status = if entry.enabled { "on " } else { "off" };
        let path = if found { "found" } else { "not on PATH" };
        let model = crate::config::model_row_label(&entry.model.default, entry.model.catalog);
        let value = format!("{status}  {model}  {path}");
        lines.push(setting_row(
            i,
            selected,
            &entry.display_label(),
            &value,
            th,
            false,
        ));
    }
    lines
}

fn toggle_page(selected: usize, th: Theme, rows: &[(&str, bool, &str)]) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            " Worktrees",
            Style::default()
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (i, (label, on, hint)) in rows.iter().enumerate() {
        lines.push(setting_row(i, selected, label, on_off(*on), th, false));
        if selected == i {
            lines.push(Line::from(Span::styled(
                format!("   {hint}"),
                Style::default().fg(th.dim),
            )));
        }
    }
    lines
}

fn setting_row(
    index: usize,
    selected: usize,
    label: &str,
    value: &str,
    th: Theme,
    caret: bool,
) -> Line<'static> {
    let mut style = Style::default();
    if index == selected {
        style = style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
    }
    let mark = if index == selected { " › " } else { "   " };
    let value = if caret {
        format!("{value}")
    } else {
        format!("[{value}]")
    };
    Line::from(vec![
        Span::styled(mark.to_string(), style),
        Span::styled(format!("{label:<28}"), style),
        Span::styled(value, style.fg(th.accent)),
    ])
}

fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

fn next(app: &mut App) {
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        if view.page + 1 >= PAGES.len() {
            dismiss(app);
            return;
        }
        view.page += 1;
        view.row = 0;
        app.dirty = true;
    }
}

fn prev(app: &mut App) {
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        if view.page == 0 {
            return;
        }
        view.page -= 1;
        view.row = 0;
        app.dirty = true;
    }
}

fn page_rows(page: usize, cfg: &Config) -> usize {
    match page {
        1 => cfg.harness_registry().len(),
        2 | 3 | 4 => 2,
        _ => 0,
    }
}

fn move_row(app: &mut App, delta: i32) {
    let cfg = Config::load();
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        let n = page_rows(view.page, &cfg);
        if n == 0 {
            return;
        }
        let next = (view.row as i32 + delta).rem_euclid(n as i32) as usize;
        view.row = next;
        app.dirty = true;
    }
}

fn activate(app: &mut App) {
    let page = match &app.overlay {
        Some(Overlay::Onboard(view)) => view.page,
        _ => return,
    };
    match page {
        0 | 5 => next(app),
        1 => cycle_selected_model(app),
        3 => {
            if let Some(Overlay::Onboard(view)) = &mut app.overlay {
                if view.row == 0 {
                    view.editing_email = true;
                    app.dirty = true;
                    return;
                }
            }
            toggle(app);
        }
        4 => {
            if let Some(Overlay::Onboard(view)) = &app.overlay {
                if view.row == 0 {
                    persist(|cfg| {
                        cfg.outside_terminal = match cfg.outside_terminal() {
                            OutsideTerminal::Ghostty => OutsideTerminal::Terminal.as_str().into(),
                            OutsideTerminal::Terminal => OutsideTerminal::Ghostty.as_str().into(),
                        };
                    });
                    app.dirty = true;
                    return;
                }
            }
            toggle(app);
        }
        _ => toggle(app),
    }
}

fn cycle_selected_model(app: &mut App) {
    let row = match &app.overlay {
        Some(Overlay::Onboard(view)) => view.row,
        _ => return,
    };
    persist(|cfg| {
        if let Some(entry) = cfg.harness_registry().get(row) {
            let id = entry.id.clone();
            cfg.cycle_agent_row(&id, crate::config::HarnessField::Model, 1);
        }
    });
    app.dirty = true;
}

fn toggle(app: &mut App) {
    let (page, row) = match &app.overlay {
        Some(Overlay::Onboard(view)) => (view.page, view.row),
        _ => return,
    };
    match page {
        1 => persist(|cfg| {
            if let Some(entry) = cfg.harness_registry().get(row) {
                let id = entry.id.clone();
                cfg.cycle_agent_row(&id, crate::config::HarnessField::Enabled, 0);
            }
        }),
        2 => persist(|cfg| match row {
            0 => cfg.link_env_files = !cfg.link_env_files,
            1 => cfg.quick_prompt_new_worktree = !cfg.quick_prompt_new_worktree,
            _ => {}
        }),
        3 if row == 1 => persist(|cfg| cfg.linear_auto_attach = !cfg.linear_auto_attach),
        4 if row == 1 => persist(|cfg| cfg.ghostty_keybinds = !cfg.ghostty_keybinds),
        _ => {}
    }
    app.dirty = true;
}

fn persist(edit: impl FnOnce(&mut Config)) {
    let mut cfg = Config::load();
    edit(&mut cfg);
    let _ = cfg.try_save();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::with_config_path;

    fn with_temp_config<T>(f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        with_config_path(dir.path().join("config.json"), f)
    }

    #[test]
    fn dismiss_stamps_onboarded_and_closes() {
        with_temp_config(|| {
            let mut app = App::new();
            let cfg = Config::load();
            open(&mut app, &cfg);
            assert!(matches!(app.overlay, Some(Overlay::Onboard(_))));
            dismiss(&mut app);
            assert!(app.overlay.is_none());
            assert!(Config::load().onboarded);
        });
    }

    #[test]
    fn enter_on_welcome_advances() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app, &Config::load());
            handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
            match &app.overlay {
                Some(Overlay::Onboard(view)) => assert_eq!(view.page, 1),
                other => panic!("expected onboard, got {other:?}"),
            }
        });
    }

    #[test]
    fn enter_on_an_agent_cycles_its_model() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app, &Config::load());
            handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
            let before = Config::load().claude_model.clone();
            handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
            let after = Config::load().claude_model.clone();
            assert_ne!(after, before, "Enter on the first agent steps its model");
            match &app.overlay {
                Some(Overlay::Onboard(view)) => assert_eq!(view.page, 1),
                other => panic!("expected onboard agents, got {other:?}"),
            }
        });
    }
}
