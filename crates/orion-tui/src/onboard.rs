//! First-run onboarding: pick agents, sign in the Claude accounts, worktree
//! defaults, Linear, and the outside terminal before the grid. Opened once
//! from `main_loop` while `config.onboarded` is still false; Esc / a click
//! outside skips and stamps the flag so it does not come back.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::claude_accounts::NewAccount;
use crate::config::{program_installed, AccountRow, Config, OutsideTerminal};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::centered_rect;

/// One page of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Welcome,
    Agents,
    /// CLAUDE ACCOUNTS: sign the default account in, add another.
    Accounts,
    Worktrees,
    Linear,
    Terminal,
    Ready,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Welcome => "Welcome",
            Page::Agents => "Agents",
            Page::Accounts => "Claude accounts",
            Page::Worktrees => "Worktrees",
            Page::Linear => "Linear",
            Page::Terminal => "Terminal",
            Page::Ready => "Ready",
        }
    }
}

/// The wizard's pages, in order: the CLAUDE ACCOUNTS step right after
/// Agents while a Claude account is on there — with Claude off there is
/// nothing to sign in. Accounts are switched on and off on the Agents
/// page only, so the list never changes under the page the wizard is on.
fn pages(cfg: &Config) -> Vec<Page> {
    let claude = cfg
        .raw_harness_registry()
        .iter()
        .any(|entry| entry.is_claude_account() && entry.enabled);
    [
        Page::Welcome,
        Page::Agents,
        Page::Accounts,
        Page::Worktrees,
        Page::Linear,
        Page::Terminal,
        Page::Ready,
    ]
    .into_iter()
    .filter(|page| *page != Page::Accounts || claude)
    .collect()
}

/// The first-run wizard's live state. `area` is written back on draw so a
/// click outside can dismiss it the same way Esc does.
#[derive(Debug, Clone)]
pub struct OnboardView {
    pub page: usize,
    pub row: usize,
    pub area: Rect,
    pub email: TextInput,
    pub editing_email: bool,
    /// What the CLAUDE ACCOUNTS page is asking, when it is asking.
    pub account: AccountStep,
    /// That page's last word: an account added, a name refused.
    pub note: Option<String>,
}

/// The CLAUDE ACCOUNTS page's questions, one at a time — the same flow
/// as Settings → Agents: an email to sign in as, or a new account's name
/// and then whether it shares the default account's setup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AccountStep {
    /// The rows: nothing asked.
    #[default]
    Rows,
    /// The email to sign account `id` in as; empty leaves it to the
    /// browser.
    Email { id: String, input: TextInput },
    /// A new account's short name; empty for the next `claude-N`.
    Name(TextInput),
    /// Whether the new account shares the default one's setup.
    Share(NewAccount),
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
            account: AccountStep::Rows,
            note: None,
        }
    }

    /// Whether a field or a question has the keys, so Esc answers it
    /// rather than skipping the wizard.
    pub fn asking(&self) -> bool {
        self.editing_email || self.account != AccountStep::Rows
    }

    /// The page the wizard stands on.
    fn current(&self, cfg: &Config) -> Page {
        let all = pages(cfg);
        all[self.page.min(all.len() - 1)]
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
        if view.account != AccountStep::Rows {
            account_step_key(app, key);
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
    let cfg = Config::load();
    let all = pages(&cfg);
    let title = format!(
        " Orion  ·  {} ({}/{}) ",
        view.current(&cfg).title(),
        view.page.min(all.len() - 1) + 1,
        all.len()
    );
    let inner = crate::ui::render_modal_frame(f, area, title, th);
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
    match view.current(cfg) {
        Page::Welcome => vec![
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
        Page::Agents => agent_lines(cfg, view.row, th),
        Page::Accounts => account_lines(cfg, view, th),
        Page::Worktrees => toggle_page(
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
        Page::Linear => {
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
        Page::Terminal => {
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
        Page::Ready => vec![
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
            &crate::ui::truncate(entry.display_label(), LABEL_W - 1),
            &value,
            th,
            false,
        ));
    }
    lines
}

/// The CLAUDE ACCOUNTS page: every account, the default one first, with
/// its dir and who it is signed in as; **Add account**; the same-account
/// warning when two share an email; and the question being asked, if one
/// is.
fn account_lines(cfg: &Config, view: &OnboardView, th: Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(th.dim);
    let mut lines = vec![
        Line::from(Span::styled(
            " Claude accounts",
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " Each is a Claude Code config dir with a login of its own.",
            dim,
        )),
        Line::from(""),
    ];
    for (i, row) in cfg.account_rows().iter().enumerate() {
        let label = match row {
            AccountRow::Account(id) => cfg.effective_harness_by_id(id).display_label().to_string(),
            AccountRow::Add => "Add account".into(),
        };
        lines.push(setting_row(
            i,
            view.row,
            &crate::ui::truncate(&label, LABEL_W - 1),
            &cfg.account_brief(row),
            th,
            false,
        ));
    }
    let notes = cfg.account_notes();
    if !notes.is_empty() {
        lines.push(Line::from(""));
        lines.extend(notes.into_iter().map(|note| {
            Line::from(Span::styled(
                format!(" {note}"),
                Style::default().fg(th.warn),
            ))
        }));
    }
    lines.push(Line::from(""));
    let field = |input: &TextInput| format!("   {}▌", input.as_str());
    match &view.account {
        AccountStep::Rows => {
            lines.push(Line::from(Span::styled(
                " Enter on an account signs it in; on Add, names a new one.",
                dim,
            )));
            lines.push(Line::from(Span::styled(
                " Space on Agents switches one on or off.",
                dim,
            )));
        }
        AccountStep::Email { id, input } => {
            lines.push(Line::from(Span::styled(
                format!(
                    " Sign {} in as (empty = choose in the browser):",
                    cfg.effective_harness_by_id(id).display_label()
                ),
                Style::default().fg(th.text),
            )));
            lines.push(Line::from(Span::styled(
                field(input),
                Style::default().fg(th.accent),
            )));
            lines.push(Line::from(Span::styled(
                " Enter runs claude auth login here   Esc back",
                dim,
            )));
        }
        AccountStep::Name(input) => {
            let next = crate::claude_accounts::plan_new(cfg, "").map_or_else(
                |_| "~/.claude-2".into(),
                |new| crate::claude_accounts::tilde(&new.dir),
            );
            lines.push(Line::from(Span::styled(
                format!(" Name it — its dir is ~/.claude-<name> (empty = {next}):"),
                Style::default().fg(th.text),
            )));
            lines.push(Line::from(Span::styled(
                field(input),
                Style::default().fg(th.accent),
            )));
            lines.push(Line::from(Span::styled(" Enter next   Esc back", dim)));
        }
        AccountStep::Share(new) => {
            let from = crate::claude_accounts::default_dir(cfg)
                .map_or_else(|| "~/.claude".into(), |d| crate::claude_accounts::tilde(&d));
            lines.push(Line::from(Span::styled(
                format!(
                    " Share {from}'s setup with {}, as links?",
                    crate::claude_accounts::tilde(&new.dir)
                ),
                Style::default().fg(th.text),
            )));
            lines.push(Line::from(Span::styled(
                format!(
                    "   {} — its login and history stay its own",
                    crate::claude_accounts::shareable(cfg).join(", ")
                ),
                dim,
            )));
            lines.push(Line::from(Span::styled(
                " Enter/y share   n start empty   Esc back",
                dim,
            )));
        }
    }
    if let Some(note) = &view.note {
        lines.push(Line::from(Span::styled(
            format!(" {note}"),
            Style::default().fg(th.muted),
        )));
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

/// How wide a row's label is padded: a longer one — an account's email —
/// is cut to fit.
const LABEL_W: usize = 28;

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
        value.to_string()
    } else {
        format!("[{value}]")
    };
    Line::from(vec![
        Span::styled(mark.to_string(), style),
        Span::styled(format!("{label:<LABEL_W$}"), style),
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
    let count = pages(&Config::load()).len();
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        if view.page + 1 >= count {
            dismiss(app);
            return;
        }
        view.page += 1;
        view.row = 0;
        view.note = None;
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
        view.note = None;
        app.dirty = true;
    }
}

fn page_rows(page: Page, cfg: &Config) -> usize {
    match page {
        Page::Agents => cfg.harness_registry().len(),
        Page::Accounts => cfg.account_rows().len(),
        Page::Worktrees | Page::Linear | Page::Terminal => 2,
        Page::Welcome | Page::Ready => 0,
    }
}

fn move_row(app: &mut App, delta: i32) {
    let cfg = Config::load();
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        let n = page_rows(view.current(&cfg), &cfg);
        if n == 0 {
            return;
        }
        let next = (view.row as i32 + delta).rem_euclid(n as i32) as usize;
        view.row = next;
        app.dirty = true;
    }
}

fn activate(app: &mut App) {
    let cfg = Config::load();
    let (page, row) = match &app.overlay {
        Some(Overlay::Onboard(view)) => (view.current(&cfg), view.row),
        _ => return,
    };
    match page {
        Page::Welcome | Page::Ready => next(app),
        Page::Agents => cycle_selected_model(app),
        Page::Accounts => {
            let step = match cfg.account_rows().into_iter().nth(row) {
                Some(AccountRow::Account(id)) => AccountStep::Email {
                    id,
                    input: TextInput::new(),
                },
                Some(AccountRow::Add) => AccountStep::Name(TextInput::new()),
                None => return,
            };
            if let Some(Overlay::Onboard(view)) = &mut app.overlay {
                view.account = step;
                view.note = None;
                app.dirty = true;
            }
        }
        Page::Linear => {
            if let Some(Overlay::Onboard(view)) = &mut app.overlay {
                if view.row == 0 {
                    view.editing_email = true;
                    app.dirty = true;
                    return;
                }
            }
            toggle(app);
        }
        Page::Terminal => {
            if row == 0 {
                persist(|cfg| {
                    cfg.outside_terminal = match cfg.outside_terminal() {
                        OutsideTerminal::Ghostty => OutsideTerminal::Terminal.as_str().into(),
                        OutsideTerminal::Terminal => OutsideTerminal::Ghostty.as_str().into(),
                    };
                });
                app.dirty = true;
                return;
            }
            toggle(app);
        }
        Page::Worktrees => toggle(app),
    }
}

/// A key while the CLAUDE ACCOUNTS page asks something: the field takes
/// the typing, Enter answers, Esc goes back to the rows.
fn account_step_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Onboard(view)) = &mut app.overlay else {
        return;
    };
    if key.code == KeyCode::Esc {
        view.account = AccountStep::Rows;
        return;
    }
    match &mut view.account {
        AccountStep::Rows => {}
        AccountStep::Email { id, input } => {
            if key.code != KeyCode::Enter {
                input.handle_key(&key);
                return;
            }
            let (id, email) = (id.clone(), input.as_str().trim().to_string());
            view.account = AccountStep::Rows;
            crate::claude_accounts::sign_in(app, &id, Some(&email));
        }
        AccountStep::Name(input) => {
            if key.code != KeyCode::Enter {
                input.handle_key(&key);
                return;
            }
            let cfg = Config::load();
            match crate::claude_accounts::plan_new(&cfg, input.as_str()) {
                Err(why) => view.note = Some(why),
                Ok(new) if crate::claude_accounts::shareable(&cfg).is_empty() => {
                    add_account(app, new, false);
                }
                Ok(new) => view.account = AccountStep::Share(new),
            }
        }
        AccountStep::Share(new) => {
            let share = match key.code {
                KeyCode::Enter | KeyCode::Char('y') => true,
                KeyCode::Char('n') => false,
                _ => return,
            };
            let new = new.clone();
            add_account(app, new, share);
        }
    }
}

/// Add `new` from the wizard and land on its row, ready to sign in.
fn add_account(app: &mut App, new: NewAccount, share: bool) {
    let result = crate::claude_accounts::add(&new, share);
    let row = Config::load()
        .account_rows()
        .iter()
        .position(|row| matches!(row, AccountRow::Account(id) if *id == new.id));
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        view.account = AccountStep::Rows;
        view.row = row.unwrap_or(view.row);
        view.note = Some(match result {
            Ok(note) => note,
            Err(why) => why,
        });
    }
    crate::claude_accounts::request_refresh(app, true);
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
    let cfg = Config::load();
    let (page, row) = match &app.overlay {
        Some(Overlay::Onboard(view)) => (view.current(&cfg), view.row),
        _ => return,
    };
    match page {
        Page::Agents => persist(|cfg| {
            if let Some(entry) = cfg.harness_registry().get(row) {
                let id = entry.id.clone();
                cfg.cycle_agent_row(&id, crate::config::HarnessField::Enabled, 0);
            }
        }),
        Page::Worktrees => persist(|cfg| match row {
            0 => cfg.link_env_files = !cfg.link_env_files,
            1 => cfg.quick_prompt_new_worktree = !cfg.quick_prompt_new_worktree,
            _ => {}
        }),
        Page::Linear if row == 1 => persist(|cfg| cfg.linear_auto_attach = !cfg.linear_auto_attach),
        Page::Terminal if row == 1 => persist(|cfg| cfg.ghostty_keybinds = !cfg.ghostty_keybinds),
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

    /// A temp home with the default account's dir (holding a CLAUDE.md
    /// to share) and the config pinned beside it.
    fn with_temp_home<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
        let home = tempfile::tempdir().unwrap();
        let root = home.path();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude/CLAUDE.md"), "be terse").unwrap();
        let places = crate::claude_accounts::Places {
            home: Some(root.to_path_buf()),
            default_dir: Some(root.join(".claude")),
            default_record: Some(orion_core::claude_account::Record {
                file: root.join(".claude.json"),
                legacy: root.join(".claude/.config.json"),
            }),
        };
        crate::claude_accounts::with_places(places, || {
            with_config_path(root.join("config.json"), || f(root))
        })
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::from(code));
    }

    fn view(app: &App) -> &OnboardView {
        match &app.overlay {
            Some(Overlay::Onboard(view)) => view,
            other => panic!("expected onboard, got {other:?}"),
        }
    }

    /// The CLAUDE ACCOUNTS step follows Agents while Claude is on there,
    /// and is skipped with it off.
    #[test]
    fn the_accounts_step_follows_agents_while_claude_is_on() {
        with_temp_config(|| {
            let cfg = Config::load();
            assert_eq!(pages(&cfg)[1..3], [Page::Agents, Page::Accounts]);
            let off: Config = serde_json::from_str(r#"{"claude_enabled": false}"#).unwrap();
            assert_eq!(pages(&off)[1..3], [Page::Agents, Page::Worktrees]);
        });
    }

    /// Enter on an account asks for the email, then runs `claude auth
    /// login` over the wizard; Esc on the question answers it, never
    /// skipping the wizard.
    #[test]
    fn enter_on_an_account_signs_it_in() {
        with_temp_home(|_| {
            let mut app = App::new();
            open(&mut app, &Config::load());
            press(&mut app, KeyCode::Enter);
            press(&mut app, KeyCode::Right);
            assert_eq!(view(&app).current(&Config::load()), Page::Accounts);
            press(&mut app, KeyCode::Enter);
            assert!(matches!(view(&app).account, AccountStep::Email { .. }));
            press(&mut app, KeyCode::Esc);
            assert_eq!(
                view(&app).account,
                AccountStep::Rows,
                "the question, not the wizard"
            );
            press(&mut app, KeyCode::Enter);
            for c in "a@b.co".chars() {
                press(&mut app, KeyCode::Char(c));
            }
            press(&mut app, KeyCode::Enter);
            let ran = crate::claude_accounts::take_ran();
            assert_eq!(ran.len(), 1);
            assert_eq!(ran[0].args, ["auth", "login", "--email", "a@b.co"]);
            assert_eq!(view(&app).account, AccountStep::Rows);
        });
    }

    /// **Add account** on the wizard: a name, the share question, and the
    /// new account's row under the cursor, ready to sign in.
    #[test]
    fn add_account_on_the_wizard_names_and_shares() {
        with_temp_home(|root| {
            let mut app = App::new();
            open(&mut app, &Config::load());
            press(&mut app, KeyCode::Enter);
            press(&mut app, KeyCode::Right);
            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Enter);
            assert!(matches!(view(&app).account, AccountStep::Name(_)));
            press(&mut app, KeyCode::Enter);
            assert!(
                matches!(&view(&app).account, AccountStep::Share(new) if new.id == "claude-2"),
                "empty is the next number"
            );
            press(&mut app, KeyCode::Char('y'));
            assert_eq!(view(&app).account, AccountStep::Rows);
            assert_eq!(view(&app).row, 1, "on the new account");
            assert!(view(&app)
                .note
                .as_deref()
                .is_some_and(|n| n.contains("sharing CLAUDE.md")));
            assert_eq!(
                std::fs::read_link(root.join(".claude-2/CLAUDE.md")).unwrap(),
                root.join(".claude/CLAUDE.md")
            );
            assert_eq!(Config::load().claude_accounts[0].id, "claude-2");
        });
    }

    /// The page as drawn: every account by its email and dir, Add, and
    /// the same-account warning when two share one.
    #[test]
    fn the_accounts_page_draws_who_each_account_is() {
        with_temp_home(|root| {
            std::fs::create_dir_all(root.join(".claude-2")).unwrap();
            std::fs::write(
                root.join("config.json"),
                serde_json::json!({"claude_accounts": [
                    {"id": "claude-2", "config_dir": root.join(".claude-2").display().to_string()}
                ]})
                .to_string(),
            )
            .unwrap();
            for record in [".claude.json", ".claude-2/.claude.json"] {
                std::fs::write(
                    root.join(record),
                    r#"{"oauthAccount": {"emailAddress": "a@b.co"}}"#,
                )
                .unwrap();
            }
            crate::claude_accounts::refresh_now();
            let mut app = App::new();
            open(&mut app, &Config::load());
            press(&mut app, KeyCode::Enter);
            press(&mut app, KeyCode::Right);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 26)).unwrap();
            terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            let text: String = (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol().to_string())
                        .collect::<String>()
                        + "\n"
                })
                .collect();
            for needle in [
                "Claude accounts (3/7)",
                "Claude (a@b.co)",
                "[~/.claude · same as ~/.claude-2]",
                "[~/.claude-2 · same as ~/.claude]",
                "Add account",
                "⚠ ~/.claude and ~/.claude-2 are signed in as one account,",
                "Enter on an account signs it in",
            ] {
                assert!(text.contains(needle), "{needle}:\n{text}");
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
