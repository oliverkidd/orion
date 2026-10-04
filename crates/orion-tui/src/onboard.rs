//! First-run ONBOARDING: a short setup over the empty grid — the agents to
//! turn on (and install), their Claude accounts, the editors, worktree
//! defaults, Linear and the outside terminal — ending on what was chosen
//! and the keys to press next. Opened once from `main_loop` while
//! `config.onboarded` is still false; Esc / a click outside skips and
//! stamps the flag so it does not come back.
//!
//! Every page is one modal laid out the way every modal is (docs/keys.md,
//! "How the screen is laid out"): the STEP STRIP across its top names each
//! step and lights this one, so no title is repeated over the page; then
//! the page's rows, labels in one column and values in words beside them;
//! the EXPLANATION of the row under the cursor, wrapped, right above the
//! bottom border; and on that border the keys, from the [`keys`] table the
//! handler matches. The modal is as tall as its page, within the screen.
//!
//! A program a row names that isn't on PATH — an agent's CLI, an editor —
//! says `install…`, and `i` there shows the command that installs it and
//! runs it on Enter, in the editor modal (`install`).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::claude_accounts::NewAccount;
use crate::config::{
    program_installed, AccountRow, Config, OutsideTerminal, SettingKind, SettingSpec,
    LINEAR_SETTINGS,
};
use crate::install::{Plan, Tools};
use crate::keymap::{Action, Keymap};
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
    /// The BUILT-IN EDITOR files open in, and the app `⌘O` hands them to.
    Editor,
    Worktrees,
    Linear,
    Terminal,
    Ready,
}

impl Page {
    /// Its name on the STEP STRIP.
    fn label(self) -> &'static str {
        match self {
            Page::Welcome => "Welcome",
            Page::Agents => "Agents",
            Page::Accounts => "Accounts",
            Page::Editor => "Editor",
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
        Page::Editor,
        Page::Worktrees,
        Page::Linear,
        Page::Terminal,
        Page::Ready,
    ]
    .into_iter()
    .filter(|page| *page != Page::Accounts || claude)
    .collect()
}

/// The Worktrees page's rows: Settings → General's, by kind.
const WORKTREE_ROWS: &[SettingKind] = &[SettingKind::WorktreeBaseBranch, SettingKind::LinkEnvFiles];

/// The Terminal page's rows: Settings → General's, by kind.
const TERMINAL_ROWS: &[SettingKind] = &[SettingKind::OutsideTerminal, SettingKind::GhosttyKeybinds];

/// The settings rows a page mirrors, in order — the overlay's own specs,
/// so each row's label and explanation are its tab's word for word. The
/// Linear page is the LINEAR TAB entire ([`LINEAR_SETTINGS`]).
fn setting_rows(page: Page) -> Vec<&'static SettingSpec> {
    let kinds = match page {
        Page::Worktrees => WORKTREE_ROWS,
        Page::Terminal => TERMINAL_ROWS,
        Page::Linear => return LINEAR_SETTINGS.iter().collect(),
        _ => return Vec::new(),
    };
    kinds
        .iter()
        .filter_map(|kind| crate::config::spec_for(*kind))
        .collect()
}

/// A row of the Editor page: a **File editor** choice, then an **Open in
/// app** one, by the word the setting stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorRow {
    Editor(&'static str),
    App(&'static str),
}

fn editor_rows() -> Vec<EditorRow> {
    crate::editor::EDITORS
        .iter()
        .map(|editor| EditorRow::Editor(editor))
        .chain(
            crate::outside_editor::CHOICES
                .iter()
                .map(|choice| EditorRow::App(choice)),
        )
        .collect()
}

/// The first-run wizard's live state. `area` is written back on draw so a
/// click outside can dismiss it the same way Esc does.
#[derive(Debug, Clone)]
pub struct OnboardView {
    pub page: usize,
    pub row: usize,
    pub area: Rect,
    /// A typed row being edited in place — the worktree base branch, the
    /// Linear page's account or task template — and its text so far.
    pub field: Option<(SettingKind, TextInput)>,
    /// What the CLAUDE ACCOUNTS page is asking, when it is asking.
    pub account: AccountStep,
    /// The install `i` asked about: its command, which Enter runs.
    pub install: Option<Plan>,
    /// The page's last word: an account added, a name refused, how an
    /// install went.
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
    pub fn new(_cfg: &Config) -> Self {
        Self {
            page: 0,
            row: 0,
            area: Rect::default(),
            field: None,
            account: AccountStep::Rows,
            install: None,
            note: None,
        }
    }

    /// Whether a field or a question has the keys, so Esc answers it
    /// rather than skipping the wizard.
    pub fn asking(&self) -> bool {
        self.field.is_some() || self.account != AccountStep::Rows || self.install.is_some()
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
    let Some(Overlay::Onboard(view)) = &mut app.overlay else {
        return;
    };
    if let Some((kind, input)) = &mut view.field {
        match key.code {
            KeyCode::Esc => view.field = None,
            // Enter saves; a multi-row field's ⇧Enter breaks a line.
            KeyCode::Enter if !input.takes_newline(&key) => {
                let (kind, value) = (*kind, input.as_str().to_string());
                view.field = None;
                persist(|cfg| {
                    cfg.set_text(kind, &value);
                });
            }
            _ => {
                input.handle_key(&key);
            }
        }
        app.dirty = true;
        return;
    }
    if let Some(plan) = view.install.clone() {
        match key.code {
            _ if keys::ENTER.matches(&key) && plan.runnable() => {
                view.install = None;
                crate::install::run(app, &plan);
            }
            KeyCode::Esc => view.install = None,
            _ => {}
        }
        app.dirty = true;
        return;
    }
    if view.account != AccountStep::Rows {
        account_step_key(app, key);
        app.dirty = true;
        return;
    }
    match key.code {
        _ if keys::SKIP.matches(&key) => dismiss(app),
        _ if keys::NEXT_PAGE.matches(&key) => next(app),
        _ if keys::PREV_PAGE.matches(&key) => prev(app),
        KeyCode::Down | KeyCode::Char('j') => move_row(app, 1),
        KeyCode::Up | KeyCode::Char('k') => move_row(app, -1),
        _ if keys::ENTER.matches(&key) => activate(app),
        _ if keys::TOGGLE.matches(&key) => toggle(app),
        _ if keys::INSTALL.matches(&key) => ask_install(app),
        _ => {}
    }
}

/// The wizard's own keys: one table [`handle_key`] matches and [`hints`]
/// spells.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const NEXT_PAGE: Key = Key::new(&["right", "tab", "l"], "next");
    pub const PREV_PAGE: Key = Key::new(&["left", "shift+tab", "h"], "back");
    pub const ENTER: Key = Key::new(&["enter"], "continue");
    pub const TOGGLE: Key = Key::new(&["space"], "toggle");
    /// A program a row names that isn't on PATH: its installer.
    pub const INSTALL: Key = Key::new(&["i"], "install");
    pub const SKIP: Key = Key::new(&["esc"], "skip");
    /// A multi-row field's line break: the task template's.
    pub const NEWLINE: Key = crate::ui::task_keys::NEWLINE;
    /// The account question's answers.
    pub const YES: Key = Key::new(&["enter", "y"], "share").show(2);
    pub const NO: Key = Key::new(&["n"], "start empty");
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        NEXT_PAGE, PREV_PAGE, ENTER, TOGGLE, INSTALL, SKIP, NEWLINE, YES, NO,
    ];
}

/// The keys on the wizard's bottom border, for the page it is on and the
/// question, if one is being asked.
pub(crate) fn hints(cfg: &Config, view: &OnboardView) -> Vec<crate::hints::Hint> {
    use crate::hints::Hint;
    let back = Hint::new("Esc", "back");
    if let Some((kind, _)) = &view.field {
        let mut hints = vec![keys::ENTER.hint_as("save").kept()];
        if kind.is_multiline_text() {
            hints.push(keys::NEWLINE.hint());
        }
        hints.push(Hint::new("Esc", "cancel"));
        return hints;
    }
    if let Some(plan) = &view.install {
        let mut hints = Vec::new();
        if plan.runnable() {
            hints.push(keys::ENTER.hint_as("run it here").kept());
        }
        hints.push(back);
        return hints;
    }
    match &view.account {
        AccountStep::Rows => {}
        AccountStep::Email { .. } => {
            return vec![
                keys::ENTER.hint_as("run claude auth login here").kept(),
                back,
            ]
        }
        AccountStep::Name(_) => return vec![keys::ENTER.hint_as("next").kept(), back],
        AccountStep::Share(_) => return vec![keys::YES.hint().kept(), keys::NO.hint(), back],
    }
    let page = view.current(cfg);
    let mut hints = match page {
        Page::Welcome => vec![keys::ENTER.hint_as("start").kept()],
        Page::Agents => vec![
            keys::TOGGLE.hint_as("on/off").kept(),
            keys::ENTER.hint_as("model"),
        ],
        Page::Accounts => match cfg.account_rows().get(view.row) {
            Some(AccountRow::Add) => vec![keys::ENTER.hint_as("add an account").kept()],
            _ => vec![keys::ENTER.hint_as("sign in").kept()],
        },
        Page::Editor => vec![keys::ENTER.hint_as("choose").kept()],
        Page::Worktrees | Page::Linear | Page::Terminal => {
            match setting_rows(page).get(view.row).map(|spec| spec.kind) {
                Some(kind) if kind.is_status() => vec![keys::ENTER.hint_as("test").kept()],
                Some(kind) if kind.is_text() => vec![keys::ENTER.hint_as("type it").kept()],
                Some(kind) if is_switch(kind) => vec![keys::TOGGLE.hint().kept()],
                _ => vec![keys::ENTER.hint_as("change").kept()],
            }
        }
        Page::Ready => vec![keys::ENTER.hint_as("start using Orion").kept()],
    };
    if row_install(cfg, page, view.row).is_some() {
        hints.push(keys::INSTALL.hint());
    }
    if page != Page::Ready {
        hints.push(keys::NEXT_PAGE.hint());
    }
    if view.page > 0 {
        hints.push(keys::PREV_PAGE.hint());
    }
    hints.push(keys::SKIP.hint_as(if page == Page::Ready { "close" } else { "skip" }));
    hints
}

/// A row that is a plain on/off switch, which Space flips.
fn is_switch(kind: SettingKind) -> bool {
    matches!(
        kind,
        SettingKind::LinkEnvFiles | SettingKind::GhosttyKeybinds | SettingKind::LinearAutoAttach
    )
}

/// The program the row under the cursor names that isn't on PATH, and how
/// it gets there — an agent's CLI, a **File editor** choice. None on any
/// other row, or when the program is there.
fn row_install(cfg: &Config, page: Page, row: usize) -> Option<Plan> {
    let tools = Tools::here();
    match page {
        Page::Agents => {
            let program = cfg.harness_registry().get(row)?.program.clone();
            (!program_installed(&program))
                .then(|| tools.agent_plan(&program))
                .flatten()
        }
        Page::Editor => match editor_rows().get(row)? {
            EditorRow::Editor(editor) if !program_installed(editor) => tools.editor_plan(editor),
            _ => None,
        },
        _ => None,
    }
}

/// What the row under the cursor does, for the EXPLANATION line above the
/// keys: an agent's CLI and where it comes from, an editor's keys, the
/// settings rows' own explanations (the overlay's word for word).
fn explanation(cfg: &Config, view: &OnboardView, keymap: &Keymap) -> String {
    // The install question says it all, over the rows.
    if view.install.is_some() {
        return String::new();
    }
    let page = view.current(cfg);
    match page {
        Page::Welcome | Page::Ready => String::new(),
        Page::Agents => agent_explanation(cfg, view.row),
        Page::Accounts => cfg
            .account_rows()
            .get(view.row)
            .map(|row| cfg.account_hint(row))
            .unwrap_or_default(),
        Page::Editor => editor_explanation(cfg, view.row, keymap),
        Page::Worktrees | Page::Linear | Page::Terminal => setting_rows(page)
            .get(view.row)
            .map(|spec| crate::hints::expand(spec.hint, keymap))
            .unwrap_or_default(),
    }
}

fn agent_explanation(cfg: &Config, row: usize) -> String {
    let Some(entry) = cfg.harness_registry().into_iter().nth(row) else {
        return String::new();
    };
    let (label, program) = (entry.display_label(), entry.program.trim());
    if program_installed(program) {
        return format!(
            "{label} runs `{program}`. On, it is offered whenever you start an agent; {} steps its \
             default model",
            keys::ENTER.label()
        );
    }
    match Tools::here().agent_plan(program) {
        Some(plan) if plan.runnable() => format!(
            "`{program}` isn't on PATH — {} runs its installer: {}",
            keys::INSTALL.label(),
            plan.line
        ),
        Some(plan) => format!("`{program}` isn't on PATH — install it: {}", plan.link),
        None => format!("`{program}` isn't on PATH — install it, then turn {label} on"),
    }
}

fn editor_explanation(cfg: &Config, row: usize, keymap: &Keymap) -> String {
    match editor_rows().get(row) {
        Some(EditorRow::Editor(editor)) => {
            let mut text = editor_blurb(editor).to_string();
            if !program_installed(editor) {
                let chosen = cfg.editor_resolved();
                if chosen.missing.as_deref() == Some(*editor) {
                    text = format!(
                        "{editor} is your choice but isn't installed, so files open in {} — {text}",
                        chosen.command
                    );
                }
                match Tools::here().editor_plan(editor) {
                    Some(plan) if plan.runnable() => text.push_str(&format!(
                        ". {} installs it: {}",
                        keys::INSTALL.label(),
                        plan.line
                    )),
                    Some(plan) => text.push_str(&format!(". Install it: {}", plan.link)),
                    None => text.push_str(". Not installed"),
                }
            }
            if let Some(over) = orion_core::env::non_empty(orion_core::env::EDITOR) {
                text = format!("ORION_EDITOR={over} overrides this choice. {text}");
            }
            text
        }
        Some(EditorRow::App(word)) => {
            let places = crate::outside_editor::Places::here();
            let choice = crate::outside_editor::Choice::parse(word);
            let opens = crate::hints::key_or(keymap, Action::OpenOutside, "Open outside");
            match (choice, crate::outside_editor::resolve(choice, &places)) {
                (crate::outside_editor::Choice::Auto, Ok(target)) => format!(
                    "The first installed of Cursor, VS Code, Sublime Text and Zed — {opens} opens \
                     files and checkouts in {} here",
                    target.name
                ),
                (crate::outside_editor::Choice::System, _) => format!(
                    "{opens} hands a file to whatever the system opens that kind of file with"
                ),
                (_, Ok(target)) => format!("{opens} opens files and checkouts in {}", target.name),
                (_, Err(why)) => why,
            }
        }
        None => String::new(),
    }
}

/// What a **File editor** choice is, in a few words.
fn editor_blurb(editor: &str) -> &'static str {
    match editor {
        "fresh" => "fresh: VS Code's keys, a command palette on ^P, wraps lines",
        "micro" => "micro: VS Code's keys (^S save, ^Q quit, ^D next match), the mouse, soft wrap",
        "edit" => "Microsoft Edit: VS Code's keys, menus on F10, word wrap",
        "vim" => "vim: :w save, :q quit",
        "nvim" => "Neovim: vim's keys — :w save, :q quit",
        "hx" => "Helix: modal, selection first — :w save, :q quit",
        "emacs" => "Emacs: ^X ^S save, ^X ^C quit",
        _ => "",
    }
}

/// The modal's width: room for the STEP STRIP and a row's columns, never
/// wider than the screen.
const MODAL_W: u16 = 82;

pub fn draw(f: &mut Frame, app: &mut App, view: &OnboardView, th: Theme) {
    let cfg = Config::load();
    let frame = f.area();
    let width = MODAL_W
        .min(frame.width.saturating_sub(2))
        .max(frame.width.min(24));
    let inner_w = width.saturating_sub(2);
    let body = page_body(app, &cfg, view, th, inner_w);
    let explain = explanation(&cfg, view, &app.keymap);
    let explain_rows = match explain.trim() {
        "" => 0,
        text => crate::hints::explain_lines(text, inner_w, 3).len() as u16 + 1,
    };
    // The strip and the blank under it, the body, and a blank over the
    // explanation; the borders.
    let want = 2 + body.lines.len() as u16 + explain_rows + 2;
    let height = want
        .min(frame.height.saturating_sub(2))
        .max(frame.height.min(8));
    let area = centered_rect(frame, width, height);
    f.render_widget(ratatui::widgets::Clear, area);
    let block = crate::hints::modal_block(
        crate::ui::modal_block(" Orion setup ", th),
        &hints(&cfg, view),
        area.width,
        th,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let all = pages(&cfg);
    let at = view.page.min(all.len() - 1);
    if inner.height > 0 {
        f.render_widget(
            Paragraph::new(step_strip(&all, at, inner.width, th)),
            Rect { height: 1, ..inner },
        );
    }
    let rest = Rect {
        y: inner.y + 2.min(inner.height),
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let (body_area, explain_row) = crate::hints::explain_area(&explain, rest, 3);
    crate::hints::draw_explain(f, explain_row, &explain, th);
    // A blank row between the body and the explanation when there is room.
    let room = usize::from(body_area.height)
        .saturating_sub(usize::from(explain_row.height > 0 && body_area.height > 4));
    // The row under the cursor — or a question being asked, at the end —
    // stays in sight on a screen too short for the whole page.
    let anchor = if view.asking() {
        body.lines.len().saturating_sub(1)
    } else {
        body.selected.unwrap_or(0)
    };
    let first = (anchor + 1).saturating_sub(room);
    let lines: Vec<Line> = body.lines.into_iter().skip(first).take(room).collect();
    f.render_widget(Paragraph::new(lines), body_area);
    if let Some(Overlay::Onboard(live)) = &mut app.overlay {
        live.area = area;
    }
}

/// The STEP STRIP: every step by name, the one the wizard is on lit, the
/// ones behind it a shade brighter than the ones ahead. Too narrow for the
/// names, it is `Step 3 of 8 · Accounts`.
fn step_strip(all: &[Page], at: usize, width: u16, th: Theme) -> Line<'static> {
    let full: usize = all
        .iter()
        .map(|page| page.label().chars().count() + 3)
        .sum();
    if full > usize::from(width) {
        return Line::from(vec![
            Span::styled(
                format!(" Step {} of {} · ", at + 1, all.len()),
                Style::default().fg(th.dim),
            ),
            Span::styled(
                all[at].label(),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
        ]);
    }
    let mut spans = Vec::new();
    for (i, page) in all.iter().enumerate() {
        spans.push(Span::raw(" "));
        let style = match i.cmp(&at) {
            std::cmp::Ordering::Less => Style::default().fg(th.muted),
            std::cmp::Ordering::Equal => Style::default()
                .fg(th.accent)
                .bg(th.sel_bg)
                .add_modifier(Modifier::BOLD),
            std::cmp::Ordering::Greater => Style::default().fg(th.dim),
        };
        spans.push(Span::styled(format!(" {} ", page.label()), style));
    }
    Line::from(spans)
}

/// A page's body as drawn: its lines at the modal's inner width, and which
/// of them is the row under the cursor.
struct Body {
    lines: Vec<Line<'static>>,
    selected: Option<usize>,
    /// The modal's inner width, which every line keeps a column short of.
    width: usize,
}

impl Body {
    fn new(width: u16) -> Self {
        Self {
            lines: Vec::new(),
            selected: None,
            width: usize::from(width),
        }
    }

    fn blank(&mut self) {
        self.lines.push(Line::from(""));
    }

    /// `text` wrapped to `width` with a column of margin each side.
    fn prose(&mut self, text: &str, width: u16, style: Style) {
        self.indented(text, 1, width, style);
    }

    /// [`Body::prose`] `indent` columns in, every wrapped row under the
    /// first — a command, a link.
    fn indented(&mut self, text: &str, indent: usize, width: u16, style: Style) {
        let room = usize::from(width).saturating_sub(indent + 1).max(1);
        let pad = " ".repeat(indent);
        for line in crate::pr_preview::wrap(text, room) {
            self.lines
                .push(Line::from(Span::styled(format!("{pad}{line}"), style)));
        }
    }

    /// A section's heading, in the overlay's header style.
    fn heading(&mut self, text: &str, th: Theme) {
        self.lines.push(Line::from(Span::styled(
            format!(" {text}"),
            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
        )));
    }

    /// One row of a table: the cursor's mark, then each cell padded to its
    /// column — the last cut to what is left of the line, a column of
    /// margin kept — the row under the cursor on the selection bar.
    fn row(&mut self, selected: bool, cells: Vec<(String, Style)>, widths: &[usize], th: Theme) {
        let bar = |style: Style| {
            if selected {
                style.bg(th.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                style
            }
        };
        let mark = if selected { " › " } else { "   " };
        let mut spans = vec![Span::styled(mark, bar(Style::default().fg(th.accent)))];
        let last = cells.len().saturating_sub(1);
        let mut used = 3;
        for (i, (text, style)) in cells.into_iter().enumerate() {
            let text = match widths.get(i) {
                Some(&w) if i < last => format!("{:<w$}  ", crate::ui::truncate(&text, w)),
                _ => crate::ui::truncate(&text, self.width.saturating_sub(used + 1).max(1)),
            };
            used += text.chars().count();
            spans.push(Span::styled(text, bar(style)));
        }
        if selected {
            self.selected = Some(self.lines.len());
        }
        self.lines.push(Line::from(spans));
    }
}

/// Each column's width over `rows`. The first — the labels — is cut to
/// what the columns between leave of `width`, room kept for a short last
/// one; the last is as long as its row lets it be ([`Body::row`]).
fn column_widths(rows: &[Vec<String>], width: u16) -> Vec<usize> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|row| row.get(c))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    if widths.len() > 1 {
        let last = widths[widths.len() - 1].min(16);
        let between: usize = widths[1..widths.len() - 1].iter().map(|w| w + 2).sum();
        let room = usize::from(width)
            .saturating_sub(3 + 2 + between + last + 1)
            .max(8);
        widths[0] = widths[0].min(room);
    }
    widths
}

fn page_body(app: &App, cfg: &Config, view: &OnboardView, th: Theme, width: u16) -> Body {
    let mut body = Body::new(width);
    let page = view.current(cfg);
    match page {
        Page::Welcome => welcome(&mut body, app, th, width),
        Page::Agents => agents(&mut body, cfg, view, th, width),
        Page::Accounts => accounts(&mut body, cfg, view, th, width),
        Page::Editor => editors(&mut body, cfg, view, th, width, &app.keymap),
        Page::Worktrees | Page::Linear | Page::Terminal => {
            settings_page(&mut body, app, cfg, view, page, th, width)
        }
        Page::Ready => ready(&mut body, app, cfg, th, width),
    }
    if let Some(plan) = &view.install {
        body.blank();
        install_question(&mut body, plan, th, width);
    }
    if let Some(note) = &view.note {
        body.blank();
        body.prose(note, width, Style::default().fg(th.muted));
    }
    body
}

fn welcome(body: &mut Body, app: &App, th: Theme, width: u16) {
    let text = Style::default().fg(th.text);
    let dim = Style::default().fg(th.dim);
    body.lines.push(Line::from(Span::styled(
        " Welcome to Orion",
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
    )));
    body.blank();
    body.prose(
        "Orion runs your coding agents — Claude Code, Codex, Cursor and the rest — in a grid \
         across your projects and their git worktrees, and keeps them running when you close it.",
        width,
        text,
    );
    body.blank();
    body.prose(
        "A minute of setup: the agents to turn on (and install), the editor files open in, \
         worktree defaults, Linear and the terminal outside Orion.",
        width,
        text,
    );
    body.blank();
    body.prose(
        &format!(
            "Nothing here is final — Settings ({}) changes all of it later — and Esc skips it.",
            crate::hints::key_or(&app.keymap, Action::Settings, "the command palette")
        ),
        width,
        dim,
    );
}

fn agents(body: &mut Body, cfg: &Config, view: &OnboardView, th: Theme, width: u16) {
    body.prose(
        "Turn on the agents you use; one left off stays out of every picker.",
        width,
        Style::default().fg(th.dim),
    );
    body.blank();
    let tools = Tools::here();
    let entries = cfg.harness_registry();
    let mut cells: Vec<Vec<String>> = vec![vec![
        "Agent".into(),
        "On".into(),
        "Default model".into(),
        "CLI".into(),
    ]];
    let mut styles = Vec::new();
    for entry in &entries {
        let installed = program_installed(&entry.program);
        let cli = match (installed, tools.agent_plan(&entry.program)) {
            (true, _) => ("installed", Style::default().fg(th.ok)),
            (false, Some(_)) => ("install…", Style::default().fg(th.accent)),
            (false, None) => ("not on PATH", Style::default().fg(th.warn)),
        };
        let on = if entry.enabled {
            ("on", Style::default().fg(th.ok))
        } else {
            ("off", Style::default().fg(th.dim))
        };
        cells.push(vec![
            entry.display_label().to_string(),
            on.0.into(),
            crate::config::model_row_label(&entry.model.default, entry.model.catalog),
            cli.0.into(),
        ]);
        styles.push((on.1, cli.1));
    }
    let widths = column_widths(&cells, width);
    let head = Style::default().fg(th.dim);
    let header = cells[0].iter().map(|c| (c.clone(), head)).collect();
    body.row(false, header, &widths, th);
    for (i, (row, (on, cli))) in cells.iter().skip(1).zip(styles).enumerate() {
        let text = Style::default().fg(th.text);
        let cells = vec![
            (row[0].clone(), text),
            (row[1].clone(), on),
            (row[2].clone(), text),
            (row[3].clone(), cli),
        ];
        body.row(i == view.row, cells, &widths, th);
    }
}

/// The CLAUDE ACCOUNTS page: every account, the default one first, with
/// its dir and who it is signed in as; **Add account**; the same-account
/// warning when two share an email; and the question being asked, if one
/// is.
fn accounts(body: &mut Body, cfg: &Config, view: &OnboardView, th: Theme, width: u16) {
    let dim = Style::default().fg(th.dim);
    let text = Style::default().fg(th.text);
    body.prose(
        "Each Claude account is a Claude Code config dir with a login of its own. Sign the \
         default one in, or add another to run sessions on a second subscription.",
        width,
        dim,
    );
    body.blank();
    let rows = cfg.account_rows();
    let mut cells: Vec<Vec<String>> = vec![vec![
        "Account".into(),
        "Config dir".into(),
        "Signed in".into(),
    ]];
    for row in &rows {
        let label = match row {
            AccountRow::Account(id) => cfg.effective_harness_by_id(id).display_label().to_string(),
            AccountRow::Add => "Add account".into(),
        };
        let (dir, state) = cfg.account_parts(row);
        cells.push(vec![label, dir, state]);
    }
    let widths = column_widths(&cells, width);
    body.row(
        false,
        cells[0].iter().map(|c| (c.clone(), dim)).collect(),
        &widths,
        th,
    );
    for (i, row) in cells.iter().skip(1).enumerate() {
        let state = if row[2] == "signed in" {
            Style::default().fg(th.ok)
        } else if row[2].starts_with("same as") {
            Style::default().fg(th.warn)
        } else {
            dim
        };
        let label = if matches!(rows.get(i), Some(AccountRow::Add)) {
            Style::default().fg(th.accent)
        } else {
            text
        };
        body.row(
            i == view.row,
            vec![
                (row[0].clone(), label),
                (row[1].clone(), text),
                (row[2].clone(), state),
            ],
            &widths,
            th,
        );
    }
    let notes = cfg.account_notes();
    if !notes.is_empty() {
        body.blank();
        for note in notes {
            body.prose(&note, width, Style::default().fg(th.warn));
        }
    }
    let field = |input: &TextInput| {
        Line::from(Span::styled(
            format!("   {}▌", input.as_str()),
            Style::default().fg(th.accent),
        ))
    };
    match &view.account {
        AccountStep::Rows => {}
        AccountStep::Email { id, input } => {
            body.blank();
            body.prose(
                &format!(
                    "Sign {} in as (empty = choose in the browser):",
                    cfg.effective_harness_by_id(id).display_label()
                ),
                width,
                text,
            );
            body.lines.push(field(input));
        }
        AccountStep::Name(input) => {
            let next = crate::claude_accounts::plan_new(cfg, "").map_or_else(
                |_| "~/.claude-2".into(),
                |new| crate::claude_accounts::tilde(&new.dir),
            );
            body.blank();
            body.prose(
                &format!("Name it — its dir is ~/.claude-<name> (empty = {next}):"),
                width,
                text,
            );
            body.lines.push(field(input));
        }
        AccountStep::Share(new) => {
            let from = crate::claude_accounts::default_dir(cfg)
                .map_or_else(|| "~/.claude".into(), |d| crate::claude_accounts::tilde(&d));
            body.blank();
            body.prose(
                &format!(
                    "Share {from}'s setup with {}, as links?",
                    crate::claude_accounts::tilde(&new.dir)
                ),
                width,
                text,
            );
            body.indented(
                &format!(
                    "{} — its login and history stay its own",
                    crate::claude_accounts::shareable(cfg).join(", ")
                ),
                3,
                width,
                dim,
            );
        }
    }
}

/// The Editor page: **File editor**'s choices, each saying whether it is
/// installed (or `install…`), then **Open in app**'s, each saying what it
/// opens on this machine. The chosen one of each wears the dot.
fn editors(
    body: &mut Body,
    cfg: &Config,
    view: &OnboardView,
    th: Theme,
    width: u16,
    keymap: &Keymap,
) {
    let dim = Style::default().fg(th.dim);
    let text = Style::default().fg(th.text);
    let tools = Tools::here();
    let rows = editor_rows();
    let chosen_editor = match crate::editor::program_name(&cfg.editor) {
        "" => crate::editor::DEFAULT_EDITOR,
        program => program,
    };
    let fallback = cfg.editor_resolved();
    let chosen_app = crate::outside_editor::Choice::parse(&cfg.outside_editor);
    let places = crate::outside_editor::Places::here();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut styles = Vec::new();
    for row in &rows {
        let (chosen, name, state, style) = match *row {
            EditorRow::Editor(editor) => {
                let chosen = editor == chosen_editor;
                let (state, style) = if program_installed(editor) {
                    ("installed".to_string(), Style::default().fg(th.ok))
                } else if chosen && fallback.missing.is_some() {
                    (
                        format!("not installed — opens {}", fallback.command),
                        Style::default().fg(th.warn),
                    )
                } else if tools.editor_plan(editor).is_some() {
                    ("install…".to_string(), Style::default().fg(th.accent))
                } else {
                    ("not installed".to_string(), dim)
                };
                (chosen, editor.to_string(), state, style)
            }
            EditorRow::App(word) => {
                let choice = crate::outside_editor::Choice::parse(word);
                let name = match choice {
                    crate::outside_editor::Choice::App(_) => crate::outside_editor::value_label(
                        word,
                        &crate::outside_editor::Places::default(),
                    )
                    .split(" — ")
                    .next()
                    .unwrap_or(word)
                    .to_string(),
                    _ => word.to_string(),
                };
                let (state, style) = match (choice, crate::outside_editor::resolve(choice, &places))
                {
                    (crate::outside_editor::Choice::Auto, Ok(target)) => {
                        (format!("opens {}", target.name), text)
                    }
                    (crate::outside_editor::Choice::System, _) => {
                        ("the system's own pick".to_string(), dim)
                    }
                    (_, Ok(_)) => ("installed".to_string(), Style::default().fg(th.ok)),
                    (_, Err(_)) => ("not installed".to_string(), dim),
                };
                (choice == chosen_app, name, state, style)
            }
        };
        cells.push(vec![
            format!("{} {name}", if chosen { "●" } else { "○" }),
            state,
        ]);
        styles.push(style);
    }
    let widths = column_widths(&cells, width);
    let editors_head = crate::hints::expand(
        "File editor — every file opens in it, inside Orion: {find_file}, {grep}, {tree_browser}",
        keymap,
    );
    let apps_head = crate::hints::expand(
        "Open in app — {open_outside} hands a file or a checkout to it",
        keymap,
    );
    for (i, (row, cell)) in rows.iter().zip(cells).enumerate() {
        match (i, row) {
            (0, _) => body.heading(&editors_head, th),
            (_, EditorRow::App(word)) if *word == crate::outside_editor::AUTO => {
                body.blank();
                body.heading(&apps_head, th);
            }
            _ => {}
        }
        let name_style = if cell[0].starts_with('●') {
            Style::default().fg(th.accent)
        } else {
            text
        };
        body.row(
            i == view.row,
            vec![(cell[0].clone(), name_style), (cell[1].clone(), styles[i])],
            &widths,
            th,
        );
    }
}

/// The Worktrees, Linear and Terminal pages: settings rows mirroring the
/// overlay's — their values read as the overlay reads them — and, while a
/// typed row is being edited, its text in place (a multi-row field's under
/// the rows, the caret's row in sight).
fn settings_page(
    body: &mut Body,
    app: &App,
    cfg: &Config,
    view: &OnboardView,
    page: Page,
    th: Theme,
    width: u16,
) {
    let dim = Style::default().fg(th.dim);
    let keymap = &app.keymap;
    match page {
        Page::Worktrees => {
            body.prose(
                "A worktree is a checkout of its own branch, so agents work side by side \
                 without touching each other's files.",
                width,
                dim,
            );
            body.blank();
        }
        Page::Linear => {
            body.prose(
                &format!(
                    "{} lists the Linear issues assigned to you; Settings → Linear has these rows \
                     too.",
                    crate::hints::key_or(keymap, Action::Linear, "The Linear view")
                ),
                width,
                dim,
            );
            body.blank();
        }
        Page::Terminal => {
            body.prose(
                "Where a terminal opens outside Orion. Ghostty keeps ⌘K, ⌘N, ⌘⇧P and more for \
                 itself unless they are unbound in its config — Orion keeps that block for you.",
                width,
                dim,
            );
            body.blank();
        }
        _ => {}
    }
    let specs = setting_rows(page);
    let editing = view.field.as_ref();
    let values: Vec<String> = specs
        .iter()
        .map(|spec| match editing {
            Some((kind, input)) if *kind == spec.kind && !kind.is_multiline_text() => {
                format!("{}▌", input.as_str())
            }
            Some((kind, _)) if *kind == spec.kind => "editing below".to_string(),
            _ => setting_value(app, cfg, spec.kind),
        })
        .collect();
    let cells: Vec<Vec<String>> = specs
        .iter()
        .zip(&values)
        .map(|(spec, value)| vec![spec.label.to_string(), value.clone()])
        .collect();
    let widths = column_widths(&cells, width);
    let mut group = "";
    for (i, (spec, value)) in specs.iter().zip(values).enumerate() {
        if spec.group != group {
            body.blank();
            body.heading(spec.group, th);
            group = spec.group;
        }
        let value_style = match value.as_str() {
            "on" => Style::default().fg(th.ok),
            "off" => dim,
            _ => Style::default().fg(th.accent),
        };
        body.row(
            i == view.row,
            vec![
                (spec.label.to_string(), Style::default().fg(th.text)),
                (value, value_style),
            ],
            &widths,
            th,
        );
    }
    if let Some((_, input)) = editing.filter(|(kind, _)| kind.is_multiline_text()) {
        body.blank();
        body.lines
            .extend(field_rows(input, width.saturating_sub(4), 6, th));
    }
    match page {
        Page::Worktrees => {
            body.blank();
            body.prose(
                &format!(
                    "A new agent runs in the checkout under the cursor. For a fresh worktree, {} \
                     in its box and pick + new worktree.",
                    crate::hints::key_or(keymap, Action::SelectLaunchWorktree, "Select worktree")
                ),
                width,
                dim,
            );
        }
        Page::Terminal if cfg.ghostty_keybinds => {
            body.blank();
            body.prose(
                "Reload Ghostty's config (⌘⇧,) after Orion's first launch so its ⌘ chords reach \
                 Orion.",
                width,
                dim,
            );
        }
        _ => {}
    }
}

/// A settings row's value in words: the LINEAR TAB's status rows as the
/// app tells them, the outside terminal by its app's name, the rest as the
/// overlay reads them.
fn setting_value(app: &App, cfg: &Config, kind: SettingKind) -> String {
    if let Some(status) = crate::linear::status_value(app, kind) {
        return status;
    }
    match kind {
        SettingKind::OutsideTerminal => terminal_name(cfg),
        SettingKind::WorktreeBaseBranch => match cfg.worktree_base_branch.trim() {
            "" => "auto — origin's default branch".into(),
            name => name.to_string(),
        },
        _ => cfg.value_label(kind),
    }
}

/// The outside terminal by name, and what stands in for Ghostty on a Mac
/// without it.
fn terminal_name(cfg: &Config) -> String {
    match cfg.outside_terminal() {
        OutsideTerminal::Ghostty
            if cfg!(target_os = "macos") && crate::event_loop::ghostty_app().is_none() =>
        {
            "Ghostty — not installed, Terminal.app opens".into()
        }
        OutsideTerminal::Ghostty => "Ghostty".into(),
        OutsideTerminal::Terminal => "Terminal.app".into(),
    }
}

/// What `i` asks before anything runs: the command, or — with nothing
/// orion can run — where to go instead.
fn install_question(body: &mut Body, plan: &Plan, th: Theme, width: u16) {
    let text = Style::default().fg(th.text);
    if plan.runnable() {
        body.prose(
            &format!(
                "Install {}? {} runs this here, in a terminal:",
                plan.program,
                keys::ENTER.label()
            ),
            width,
            text,
        );
        body.indented(&plan.line, 3, width, Style::default().fg(th.accent));
        body.indented(
            &format!("from {}", plan.link),
            3,
            width,
            Style::default().fg(th.dim),
        );
    } else {
        body.prose(
            &format!(
                "Homebrew isn't installed, so Orion can't install {} itself. Its install page:",
                plan.program
            ),
            width,
            text,
        );
        body.indented(plan.link, 3, width, Style::default().fg(th.accent));
    }
}

/// One line of the Ready page's summary: what it is about, what was
/// chosen, and whether it wants another look.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Chosen {
    what: &'static str,
    value: String,
    warn: bool,
}

/// What the wizard leaves set, a line a step: the agents on (and any whose
/// CLI is missing), the editors, worktrees, Linear, the terminal.
fn summary(app: &App, cfg: &Config) -> Vec<Chosen> {
    let on: Vec<_> = cfg
        .harness_registry()
        .into_iter()
        .filter(|entry| entry.enabled)
        .collect();
    let agents = if on.is_empty() {
        Chosen {
            what: "Agents",
            value: "none on — turn one on in Settings → Agents to start an agent".into(),
            warn: true,
        }
    } else {
        let missing = on.iter().any(|e| !program_installed(&e.program));
        Chosen {
            what: "Agents",
            value: on
                .iter()
                .map(|entry| {
                    let label = entry.display_label().to_string();
                    if program_installed(&entry.program) {
                        label
                    } else {
                        format!("{label} (not installed)")
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
            warn: missing,
        }
    };
    let editor = cfg.editor_resolved();
    let base = match cfg.worktree_base_branch.trim() {
        "" => "origin's default branch".to_string(),
        name => name.to_string(),
    };
    let key = crate::linear::status_value(app, SettingKind::LinearKey)
        .filter(|status| status.starts_with("found"))
        .map(|status| format!(" · LINEAR_API_KEY {status}"))
        .unwrap_or_default();
    vec![
        agents,
        Chosen {
            what: "File editor",
            value: cfg.value_label(SettingKind::Editor),
            warn: editor.missing.is_some(),
        },
        Chosen {
            what: "Open in app",
            value: cfg.value_label(SettingKind::OutsideEditor),
            warn: false,
        },
        Chosen {
            what: "Worktrees",
            value: format!(
                "{} · new ones branch from {base}",
                if cfg.link_env_files {
                    ".env files linked"
                } else {
                    ".env files not linked"
                }
            ),
            warn: false,
        },
        Chosen {
            what: "Linear",
            value: format!(
                "{}{key}",
                if cfg.linear_auto_attach {
                    "pull requests linked to their issues"
                } else {
                    "pull requests not linked"
                }
            ),
            warn: false,
        },
        Chosen {
            what: "Terminal",
            value: match cfg.outside_terminal() {
                OutsideTerminal::Ghostty if cfg.ghostty_keybinds => {
                    format!("{} · ⌘ chords unbound in its config", terminal_name(cfg))
                }
                _ => terminal_name(cfg),
            },
            warn: false,
        },
    ]
}

/// The keys to press first, from the live keymap — an unbound one left
/// out.
fn next_keys(keymap: &Keymap) -> Vec<(String, &'static str)> {
    [
        (Action::QuickPrompt, "start an agent: type the task, Enter"),
        (
            Action::NewTerminal,
            "open a terminal: a plain shell, no agent",
        ),
        (Action::Palette, "jump to any project, worktree or session"),
        (Action::Linear, "your Linear issues"),
        (Action::CommandPalette, "every action by name"),
        (Action::Settings, "Settings: change any of this"),
    ]
    .into_iter()
    .filter_map(|(action, does)| crate::hints::key(keymap, action).map(|key| (key, does)))
    .collect()
}

fn ready(body: &mut Body, app: &App, cfg: &Config, th: Theme, width: u16) {
    body.lines.push(Line::from(Span::styled(
        " You're set. This is what Orion will use:",
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
    )));
    body.blank();
    let chosen = summary(app, cfg);
    let cells: Vec<Vec<String>> = chosen
        .iter()
        .map(|c| vec![c.what.to_string(), c.value.clone()])
        .collect();
    let widths = column_widths(&cells, width);
    let room = usize::from(width)
        .saturating_sub(3 + widths[0] + 2 + 1)
        .max(8);
    for c in &chosen {
        let style = Style::default().fg(if c.warn { th.warn } else { th.text });
        // A long value — many agents — wraps under its own column.
        let mut wrapped = crate::pr_preview::wrap(&c.value, room).into_iter();
        let first = wrapped.next().unwrap_or_default();
        body.row(
            false,
            vec![
                (c.what.to_string(), Style::default().fg(th.muted)),
                (first, style),
            ],
            &widths,
            th,
        );
        for more in wrapped {
            body.lines.push(Line::from(Span::styled(
                format!("{}{more}", " ".repeat(3 + widths[0] + 2)),
                style,
            )));
        }
    }
    body.blank();
    body.heading("Next", th);
    let keys = next_keys(&app.keymap);
    let cells: Vec<Vec<String>> = keys
        .iter()
        .map(|(key, does)| vec![key.clone(), does.to_string()])
        .collect();
    let widths = column_widths(&cells, width);
    for (key, does) in keys {
        body.row(
            false,
            vec![
                (key, crate::hints::key_style(th)),
                (does.to_string(), Style::default().fg(th.text)),
            ],
            &widths,
            th,
        );
    }
}

/// A multi-row field drawn in place: its rows at `width`, at most `max`
/// of them — the window round the caret — the caret a `▌` where it is.
fn field_rows(input: &TextInput, width: u16, max: usize, th: Theme) -> Vec<Line<'static>> {
    let text: Vec<char> = input.as_str().chars().collect();
    let rows = input.rows(usize::from(width.max(1)));
    let caret_row = input.caret_row(&rows);
    let caret = input.cursor_chars();
    let first = (caret_row + 1).saturating_sub(max);
    rows.iter()
        .enumerate()
        .skip(first)
        .take(max)
        .map(|(i, &(start, end))| {
            let mut row: String = text[start..end].iter().collect();
            if i == caret_row {
                let at = caret.saturating_sub(start).min(row.chars().count());
                let byte = row.char_indices().nth(at).map_or(row.len(), |(b, _)| b);
                row.insert(byte, '▌');
            }
            Line::from(Span::styled(
                format!("   {}", row.trim_end_matches('\n')),
                Style::default().fg(th.accent),
            ))
        })
        .collect()
}

fn next(app: &mut App) {
    let cfg = Config::load();
    let all = pages(&cfg);
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        if view.page + 1 >= all.len() {
            dismiss(app);
            return;
        }
        view.page += 1;
        view.row = landing_row(all[view.page], &cfg);
        view.note = None;
        app.dirty = true;
    }
}

fn prev(app: &mut App) {
    let cfg = Config::load();
    let all = pages(&cfg);
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        if view.page == 0 {
            return;
        }
        view.page = (view.page - 1).min(all.len() - 1);
        view.row = landing_row(all[view.page], &cfg);
        view.note = None;
        app.dirty = true;
    }
}

/// Where the cursor lands on arriving at `page`: on the Editor page the
/// editor chosen now, so `i` there installs it; the first row elsewhere.
fn landing_row(page: Page, cfg: &Config) -> usize {
    if page != Page::Editor {
        return 0;
    }
    let chosen = match crate::editor::program_name(&cfg.editor) {
        "" => crate::editor::DEFAULT_EDITOR,
        program => program,
    };
    editor_rows()
        .iter()
        .position(|row| matches!(row, EditorRow::Editor(editor) if *editor == chosen))
        .unwrap_or(0)
}

fn page_rows(page: Page, cfg: &Config) -> usize {
    match page {
        Page::Agents => cfg.harness_registry().len(),
        Page::Accounts => cfg.account_rows().len(),
        Page::Editor => editor_rows().len(),
        Page::Worktrees | Page::Linear | Page::Terminal => setting_rows(page).len(),
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
        view.note = None;
        app.dirty = true;
    }
}

/// The page and row the wizard's cursor is on.
fn cursor(app: &App, cfg: &Config) -> Option<(Page, usize)> {
    match &app.overlay {
        Some(Overlay::Onboard(view)) => Some((view.current(cfg), view.row)),
        _ => None,
    }
}

fn activate(app: &mut App) {
    let cfg = Config::load();
    let Some((page, row)) = cursor(app, &cfg) else {
        return;
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
        Page::Editor => choose_editor_row(app, row),
        // The overlay's rows, doing what Enter does there: a typed row
        // opens for typing, a status row tests the connection, the rest
        // flip or step.
        Page::Worktrees | Page::Linear | Page::Terminal => {
            match setting_rows(page).get(row).map(|spec| spec.kind) {
                Some(kind) if kind.is_status() => crate::linear::test_connection(app),
                Some(kind) if kind.is_text() => {
                    let mut input = TextInput::with_text(cfg.text_value(kind));
                    input.set_multiline(kind.is_multiline_text());
                    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
                        view.field = Some((kind, input));
                    }
                    app.dirty = true;
                }
                Some(kind) => cycle_setting(app, kind),
                None => {}
            }
        }
    }
}

/// Step a settings row on, as the overlay does — and, for the terminal's
/// rows, keep Ghostty's config in step with them as the overlay does.
fn cycle_setting(app: &mut App, kind: SettingKind) {
    persist(|cfg| cfg.cycle_kind(kind, 1));
    if matches!(
        kind,
        SettingKind::OutsideTerminal | SettingKind::GhosttyKeybinds
    ) {
        let cfg = Config::load();
        if let (Some(note), Some(Overlay::Onboard(view))) =
            (crate::ghostty_config::ensure_for(&cfg), &mut app.overlay)
        {
            view.note = Some(note);
        }
        crate::keymap::set_ghostty_unbound(
            cfg.ghostty_keybinds && crate::ghostty_config::inside_ghostty() && !app.is_remote,
        );
    }
    app.dirty = true;
}

/// Enter or Space on an Editor page row: that editor, or that app, is the
/// one. A hand-typed command for the same editor (`micro -autosu true`)
/// is left as typed.
fn choose_editor_row(app: &mut App, row: usize) {
    match editor_rows().get(row).copied() {
        Some(EditorRow::Editor(editor)) => persist(|cfg| {
            if crate::editor::program_name(&cfg.editor) != editor {
                cfg.editor = editor.to_string();
            }
        }),
        Some(EditorRow::App(word)) => {
            persist(|cfg| cfg.outside_editor = word.to_string());
            crate::outside_editor::forget_hint_name();
        }
        None => {}
    }
    app.dirty = true;
}

/// `i`: the install for the row's program, asked before it runs — or, when
/// there is nothing to install, why not.
fn ask_install(app: &mut App) {
    let cfg = Config::load();
    let Some((page, row)) = cursor(app, &cfg) else {
        return;
    };
    let program = match page {
        Page::Agents => cfg.harness_registry().get(row).map(|e| e.program.clone()),
        Page::Editor => match editor_rows().get(row) {
            Some(EditorRow::Editor(editor)) => Some(editor.to_string()),
            _ => None,
        },
        _ => None,
    };
    let Some(program) = program else {
        return;
    };
    let plan = row_install(&cfg, page, row);
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        match plan {
            Some(plan) => view.install = Some(plan),
            None if program_installed(&program) => {
                view.note = Some(format!("{program} is already installed"));
            }
            None => view.note = Some(format!("Orion knows no installer for `{program}`")),
        }
    }
    app.dirty = true;
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
                _ if keys::YES.matches(&key) => true,
                _ if keys::NO.matches(&key) => false,
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

/// Space: an agent on or off, an editor chosen, a switch flipped.
fn toggle(app: &mut App) {
    let cfg = Config::load();
    let Some((page, row)) = cursor(app, &cfg) else {
        return;
    };
    match page {
        Page::Agents => persist(|cfg| {
            if let Some(entry) = cfg.harness_registry().get(row) {
                let id = entry.id.clone();
                cfg.cycle_agent_row(&id, crate::config::HarnessField::Enabled, 0);
            }
        }),
        Page::Editor => choose_editor_row(app, row),
        Page::Worktrees | Page::Linear | Page::Terminal => {
            match setting_rows(page).get(row).map(|spec| spec.kind) {
                Some(kind) if !kind.is_text() && !kind.is_status() => cycle_setting(app, kind),
                _ => {}
            }
        }
        Page::Welcome | Page::Accounts | Page::Ready => {}
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

    /// A PATH of one temp dir holding a stub of each of `programs`, for
    /// `f` on this test's thread — what is "installed" while it runs.
    fn with_programs<T>(programs: &[&str], f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        for program in programs {
            std::fs::write(dir.path().join(program), "#!/bin/sh\nexit 0\n").unwrap();
        }
        let path = std::env::join_paths([dir.path()]).unwrap();
        crate::config::with_search_path(path, f)
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

    /// The steps in order: the CLAUDE ACCOUNTS step follows Agents while
    /// Claude is on there, and is skipped with it off; the Editor step
    /// comes before Worktrees, Ready is last.
    #[test]
    fn the_steps_run_in_order_and_accounts_only_with_claude_on() {
        with_temp_config(|| {
            let cfg = Config::load();
            assert_eq!(
                pages(&cfg),
                [
                    Page::Welcome,
                    Page::Agents,
                    Page::Accounts,
                    Page::Editor,
                    Page::Worktrees,
                    Page::Linear,
                    Page::Terminal,
                    Page::Ready,
                ]
            );
            let off: Config = serde_json::from_str(r#"{"claude_enabled": false}"#).unwrap();
            assert_eq!(
                pages(&off),
                [
                    Page::Welcome,
                    Page::Agents,
                    Page::Editor,
                    Page::Worktrees,
                    Page::Linear,
                    Page::Terminal,
                    Page::Ready,
                ]
            );
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

    /// Walk the wizard onto `page`.
    fn to_page(app: &mut App, page: Page) {
        let all = pages(&Config::load());
        let at = all.iter().position(|p| *p == page).unwrap();
        while view(app).page < at {
            press(app, KeyCode::Right);
        }
        assert_eq!(view(app).current(&Config::load()), page);
    }

    fn draw_at(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    fn draw_text(app: &mut App) -> String {
        draw_at(app, 90, 28)
    }

    /// The text inside the modal's frame, its words run together with
    /// single spaces — what a row or a wrapped sentence says, wherever it
    /// broke.
    fn words(shot: &str) -> String {
        let rows: Vec<&str> = shot.lines().collect();
        let top = rows.iter().position(|row| row.contains('╭')).unwrap_or(0);
        let left = rows[top].chars().position(|c| c == '╭').unwrap_or(0);
        let right = rows[top].chars().position(|c| c == '╮').unwrap_or(0);
        rows.iter()
            .skip(top + 1)
            .take_while(|row| !row.contains('╰'))
            .map(|row| {
                row.chars()
                    .skip(left + 1)
                    .take(right.saturating_sub(left + 1))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The step strip names every step once — no page title repeated
    /// over the page — and narrows to `Step n of m` when the names don't
    /// fit.
    #[test]
    fn the_step_strip_names_every_step_once() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app, &Config::load());
            to_page(&mut app, Page::Agents);
            let shot = draw_text(&mut app);
            assert!(
                shot.contains(
                    " Welcome   Agents   Accounts   Editor   Worktrees   Linear   Terminal   Ready "
                ),
                "{shot}"
            );
            assert_eq!(shot.matches("Agents").count(), 1, "{shot}");
            assert!(shot.contains("Orion setup"), "{shot}");
            let narrow = draw_at(&mut app, 60, 28);
            assert!(narrow.contains("Step 2 of 8 · Agents"), "{narrow}");
        });
    }

    /// The Worktrees page mirrors Settings → General's rows — the base
    /// branch and linking `.env` files — and says where a fresh worktree
    /// comes from: the box's own toggle is gone, and the retired
    /// `quick_prompt_new_worktree` is neither shown nor flipped here.
    #[test]
    fn the_worktrees_page_offers_its_settings_and_points_at_the_picker() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app, &Config::load());
            to_page(&mut app, Page::Worktrees);
            let cfg = Config::load();
            assert_eq!(page_rows(Page::Worktrees, &cfg), 2);
            let keymap = crate::keymap::Keymap::default();
            let shot = draw_text(&mut app);
            let text = words(&shot);
            assert!(
                text.contains("› Worktree base branch auto — origin's default branch"),
                "{text}"
            );
            assert!(text.contains("Link .env files on"), "{text}");
            let key = crate::hints::key(&keymap, crate::keymap::Action::SelectLaunchWorktree)
                .expect("bound by default");
            assert!(
                text.contains(&format!("{key} in its box and pick + new worktree")),
                "{text}"
            );
            assert!(
                text.contains("Branch new worktrees start from"),
                "the row's own explanation: {text}"
            );
            assert!(shot.contains("Enter type it"), "{shot}");

            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Char(' '));
            let cfg = Config::load();
            assert!(!cfg.link_env_files, "row 1 is the .env link");
            assert!(
                !cfg.quick_prompt_new_worktree,
                "the retired key is left alone"
            );
            press(&mut app, KeyCode::Up);
            press(&mut app, KeyCode::Enter);
            for c in "develop".chars() {
                press(&mut app, KeyCode::Char(c));
            }
            press(&mut app, KeyCode::Enter);
            assert_eq!(Config::load().worktree_base_branch, "develop");
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

    /// The page as drawn: every account by its email, its dir and who it
    /// is signed in as in columns, Add, and the same-account warning when
    /// two share one.
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
            let text = draw_text(&mut app);
            let inner = words(&text);
            for needle in [
                "Account Config dir Signed in",
                "› Claude (a@b.co) ~/.claude same as ~/.claude-2",
                "Claude (a@b.co) ~/.claude-2 same as ~/.claude",
                "Add account ~/.claude-3",
                "⚠ ~/.claude and ~/.claude-2 are signed in as one account,",
            ] {
                assert!(inner.contains(needle), "{needle}:\n{text}");
            }
            assert!(text.contains("Enter sign in"), "{text}");
            assert!(!text.contains('['), "no bracketed values:\n{text}");
        });
    }

    /// The Linear page mirrors Settings → Linear: the same rows, in the
    /// same order, with the same values and the tab's own explanation of
    /// the row under the cursor. Space flips **Link PRs to Linear**, Enter
    /// types the account in place, and the task template opens over lines
    /// — ⇧Enter breaks one, Enter saves.
    #[test]
    fn the_linear_page_mirrors_the_linear_tab() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app, &Config::load());
            to_page(&mut app, Page::Linear);
            let shot = draw_text(&mut app);
            let mut at = 0;
            for spec in LINEAR_SETTINGS {
                let found = shot[at..]
                    .find(spec.label)
                    .unwrap_or_else(|| panic!("{} in order:\n{shot}", spec.label));
                at += found;
            }
            let inner = words(&shot);
            for needle in [
                "Link PRs to Linear on",
                "Linear account the key's owner",
                "Task template default",
                "Connection",
                "API key no project selected",
            ] {
                assert!(inner.contains(needle), "{needle}:\n{shot}");
            }
            assert!(
                shot.contains("Attach the PR a"),
                "the tab's explanation:\n{shot}"
            );
            assert!(shot.contains("Space toggle"), "{shot}");

            press(&mut app, KeyCode::Char(' '));
            assert!(!Config::load().linear_auto_attach);
            press(&mut app, KeyCode::Char(' '));
            assert!(Config::load().linear_auto_attach);

            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Enter);
            assert!(view(&app).asking());
            for c in "a@b.co".chars() {
                press(&mut app, KeyCode::Char(c));
            }
            press(&mut app, KeyCode::Enter);
            assert_eq!(Config::load().linear_assignee_email, "a@b.co");

            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Enter);
            assert!(
                matches!(&view(&app).field, Some((SettingKind::LinearTaskTemplate, input))
                    if input.as_str() == crate::config::DEFAULT_LINEAR_TEMPLATE),
                "the edit starts from the template the box would get"
            );
            let keys_line = format!(
                "Enter save · {} newline · Esc cancel",
                keys::NEWLINE.label()
            );
            let shot = draw_text(&mut app);
            assert!(shot.contains(&keys_line), "{keys_line}:\n{shot}");
            if let Some(Overlay::Onboard(view)) = &mut app.overlay {
                if let Some((_, input)) = &mut view.field {
                    input.set_text("Fix {ids}");
                }
            }
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('j'), crossterm::event::KeyModifiers::CONTROL),
            );
            for c in "{issues}".chars() {
                press(&mut app, KeyCode::Char(c));
            }
            let editing = draw_text(&mut app);
            assert!(editing.contains("Fix {ids}"), "{editing}");
            assert!(editing.contains("{issues}▌"), "{editing}");
            press(&mut app, KeyCode::Enter);
            assert!(!view(&app).asking());
            assert_eq!(Config::load().linear_task_template, "Fix {ids}\n{issues}");
            assert!(draw_text(&mut app).contains("custom: Fix {ids}"));
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
                other => panic!("expected onboard, got {other:?}"),
            }
        });
    }

    /// The Agents page reads as a table — on or off, the model, and the
    /// CLI `installed` or `install…` — with every label whole. `i` on a
    /// missing CLI shows its installer and Enter runs it in the modal;
    /// Esc backs out of the question, never the wizard.
    #[test]
    fn a_missing_agent_cli_installs_from_the_agents_page() {
        with_temp_config(|| {
            with_programs(&["claude", "brew"], || {
                let mut app = App::new();
                open(&mut app, &Config::load());
                to_page(&mut app, Page::Agents);
                let shot = draw_text(&mut app);
                let inner = words(&shot);
                for needle in [
                    "Agent On Default model CLI",
                    "› Claude on default installed",
                    "Codex on default install…",
                    "Grok Build on default install…",
                ] {
                    assert!(inner.contains(needle), "{needle}:\n{shot}");
                }
                assert!(!shot.contains('['), "no bracketed values:\n{shot}");
                assert!(!shot.contains(" i install"), "claude is there:\n{shot}");

                press(&mut app, KeyCode::Char('i'));
                assert!(view(&app).install.is_none());
                assert_eq!(
                    view(&app).note.as_deref(),
                    Some("claude is already installed")
                );

                let codex = Config::load()
                    .harness_registry()
                    .iter()
                    .position(|entry| entry.id == "codex")
                    .unwrap();
                for _ in 0..codex {
                    press(&mut app, KeyCode::Down);
                }
                let shot = draw_text(&mut app);
                assert!(shot.contains("i install"), "{shot}");
                assert!(
                    words(&shot).contains(
                        "`codex` isn't on PATH — i runs its installer: brew install --cask codex"
                    ),
                    "{shot}"
                );
                press(&mut app, KeyCode::Char('i'));
                assert!(view(&app).asking());
                let shot = draw_text(&mut app);
                assert!(
                    words(&shot).contains("Install codex? Enter runs this here"),
                    "{shot}"
                );
                assert!(shot.contains("Enter run it here · Esc back"), "{shot}");
                press(&mut app, KeyCode::Esc);
                assert!(!view(&app).asking());
                assert!(matches!(app.overlay, Some(Overlay::Onboard(_))));
                assert!(crate::install::take_ran().is_empty());

                press(&mut app, KeyCode::Char('i'));
                press(&mut app, KeyCode::Enter);
                let ran = crate::install::take_ran();
                assert_eq!(ran.len(), 1);
                assert_eq!(ran[0].args, ["install", "--cask", "codex"]);
                assert!(!view(&app).asking());
            });
        });
    }

    /// The Editor page: every **File editor** choice with whether it is
    /// installed, the chosen one dotted — and, chosen but missing, what
    /// opens instead — then **Open in app**'s choices. `i` on a missing
    /// editor runs `brew install <formula>`; Enter chooses.
    #[test]
    fn the_editor_page_installs_and_chooses() {
        with_temp_config(|| {
            with_programs(&["vim", "edit", "brew"], || {
                let mut app = App::new();
                open(&mut app, &Config::load());
                to_page(&mut app, Page::Editor);
                let shot = draw_at(&mut app, 150, 40);
                let inner = words(&shot);
                for needle in [
                    "File editor — every file opens in it",
                    "› ● fresh not installed — opens edit",
                    "○ micro install…",
                    "○ edit installed",
                    "Open in app — ",
                    "● auto opens",
                ] {
                    assert!(inner.contains(needle), "{needle}:\n{shot}");
                }
                assert!(
                    inner.contains(
                        "fresh is your choice but isn't installed, so files open in edit"
                    ),
                    "{shot}"
                );
                press(&mut app, KeyCode::Char('i'));
                press(&mut app, KeyCode::Enter);
                assert_eq!(
                    crate::install::take_ran()[0].args,
                    ["install", "fresh-editor"],
                    "the formula"
                );
                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Char('i'));
                press(&mut app, KeyCode::Enter);
                assert_eq!(crate::install::take_ran()[0].args, ["install", "micro"]);

                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Enter);
                assert_eq!(Config::load().editor, "vim");
                let rows = editor_rows();
                let zed = rows
                    .iter()
                    .position(|r| *r == EditorRow::App("zed"))
                    .unwrap();
                for _ in view(&app).row..zed {
                    press(&mut app, KeyCode::Down);
                }
                press(&mut app, KeyCode::Char(' '));
                assert_eq!(Config::load().outside_editor, "zed");
            });
        });
    }

    /// The Editor page opens on the editor chosen now — a missing one's
    /// `i` right there.
    #[test]
    fn the_editor_page_opens_on_the_chosen_editor() {
        with_temp_config(|| {
            persist(|cfg| cfg.editor = "hx".into());
            let mut app = App::new();
            open(&mut app, &Config::load());
            to_page(&mut app, Page::Editor);
            assert_eq!(
                editor_rows()[view(&app).row],
                EditorRow::Editor("hx"),
                "on the choice"
            );
        });
    }

    /// Without Homebrew, `i` on an editor names its install page and Enter
    /// runs nothing.
    #[test]
    fn without_homebrew_the_editor_page_points_at_the_page() {
        with_temp_config(|| {
            with_programs(&["vim"], || {
                let mut app = App::new();
                open(&mut app, &Config::load());
                to_page(&mut app, Page::Editor);
                press(&mut app, KeyCode::Char('i'));
                let shot = draw_text(&mut app);
                assert!(words(&shot).contains("Homebrew isn't installed"), "{shot}");
                assert!(shot.contains("github.com/sinelaw/fresh"), "{shot}");
                assert!(!shot.contains("run it here"), "{shot}");
                press(&mut app, KeyCode::Enter);
                assert!(crate::install::take_ran().is_empty());
                assert!(view(&app).asking(), "Esc is the way back");
            });
        });
    }

    /// The Ready page sums up what was chosen and names the next keys from
    /// the keymap; with no agent on it says so instead of naming none.
    #[test]
    fn the_ready_page_sums_up_what_was_chosen() {
        with_temp_config(|| {
            with_programs(&["claude", "fresh"], || {
                // Claude on, every other agent off.
                persist(|cfg| {
                    for entry in cfg.harness_registry() {
                        if entry.enabled != (entry.id == "claude") {
                            cfg.cycle_agent_row(&entry.id, crate::config::HarnessField::Enabled, 0);
                        }
                    }
                });
                let app = App::new();
                let cfg = Config::load();
                let chosen = summary(&app, &cfg);
                assert_eq!(chosen[0].what, "Agents");
                assert_eq!(chosen[0].value, "Claude");
                assert!(!chosen[0].warn);
                assert_eq!(chosen[1].value, "fresh");
                assert!(chosen[3].value.starts_with(".env files linked"));

                let mut app = App::new();
                open(&mut app, &cfg);
                to_page(&mut app, Page::Ready);
                let shot = draw_text(&mut app);
                let inner = words(&shot);
                for (key, does) in next_keys(&app.keymap) {
                    assert!(
                        inner.contains(&format!("{key} {does}")),
                        "{key} {does}:\n{shot}"
                    );
                }
                let new_agent = crate::hints::key(&app.keymap, Action::QuickPrompt).unwrap();
                assert!(
                    inner.contains(&format!("{new_agent} start an agent")),
                    "{shot}"
                );
                assert!(inner.contains("Agents Claude File editor fresh"), "{shot}");
                assert!(!shot.contains("Turn on an agent"), "{shot}");
                assert!(shot.contains("Enter start using Orion"), "{shot}");

                persist(|cfg| cfg.claude_enabled = false);
                let chosen = summary(&app, &Config::load());
                assert!(chosen[0].value.starts_with("none on"), "{:?}", chosen[0]);
                assert!(chosen[0].warn);
            });
        });
    }

    /// Every page fits the screen — 150×40 and 90×28 — with its keys on
    /// the bottom border and nothing cut at the frame: the modal is as tall
    /// as the page, and every line of prose wraps.
    #[test]
    fn every_page_fits_and_wraps() {
        with_temp_config(|| {
            with_programs(&["claude", "fresh", "brew"], || {
                for (w, h) in [(150, 40), (90, 28)] {
                    let mut app = App::new();
                    open(&mut app, &Config::load());
                    let count = pages(&Config::load()).len();
                    for page in 0..count {
                        let shot = draw_at(&mut app, w, h);
                        let rows: Vec<Vec<char>> =
                            shot.lines().map(|row| row.chars().collect()).collect();
                        let top = rows.iter().position(|row| row.contains(&'╭')).unwrap();
                        let bottom = rows
                            .iter()
                            .rposition(|row| row.contains(&'╰'))
                            .unwrap_or_else(|| panic!("a frame:\n{shot}"));
                        let border: String = rows[bottom].iter().collect();
                        assert!(border.contains("Esc"), "keys on the border:\n{shot}");
                        // Nothing runs into the right border: prose wraps
                        // a column short of it, and a row's value is cut
                        // to fit.
                        let right = rows[top].iter().position(|c| *c == '╮').unwrap();
                        for row in &rows[top + 1..bottom] {
                            assert_eq!(row[right], '│', "the frame:\n{shot}");
                            assert_eq!(row[right - 1], ' ', "a line cut at the frame:\n{shot}");
                        }
                        assert!(
                            bottom - top < usize::from(h) - 1,
                            "page {page} at {w}x{h} fits:\n{shot}"
                        );
                        if page + 1 < count {
                            press(&mut app, KeyCode::Right);
                        }
                    }
                }
            });
        });
    }
}
