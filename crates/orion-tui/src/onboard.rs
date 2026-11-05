//! First-run SETUP: four pages over the empty grid — Welcome; Claude, its
//! CLI and the accounts to sign in; the first project, with the branch
//! its worktrees start from and its run command; and Ready, the keys that
//! matter. Opened once from `main_loop` while `config.onboarded` is still
//! false, and again from the palette's **Run setup** and `orion setup`;
//! Esc / a click outside skips and stamps the flag so it does not come
//! back. WHAT'S NEW, after an upgrade, is a sheet of its own
//! (`crate::whats_new`), drawn in the same frame ([`draw_sheet`]).
//!
//! Every page is one modal laid out the way every modal is (docs/keys.md,
//! "How the screen is laid out"): the STEP STRIP across its top names each
//! step and lights this one, so no title is repeated over the page; then
//! the page's rows, labels in one column and values in words beside them;
//! the EXPLANATION of the row under the cursor, wrapped, right above the
//! bottom border; and on that border the keys, from the [`keys`] table the
//! handler matches. The modal is as tall as its page, within the screen.
//!
//! The Claude CLI, while it isn't on PATH, says `install…`, and `i` shows
//! the command that installs it and runs it on Enter, in the editor modal
//! (`install`).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::claude_accounts::NewAccount;
use crate::config::{program_installed, AccountRow, Config, SettingKind};
use crate::install::{Plan, Tools};
use crate::keymap::{Action, Keymap};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::centered_rect;
use orion_core::protocol::ClientRequest;

/// One page of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Welcome,
    /// CLAUDE: the CLI, then every account — sign each in, add one.
    Claude,
    /// FIRST PROJECT: the repo, its base branch and run command.
    Project,
    Ready,
}

/// The wizard's pages, in order.
const PAGES: [Page; 4] = [Page::Welcome, Page::Claude, Page::Project, Page::Ready];

impl Page {
    /// Its name on the STEP STRIP.
    fn label(self) -> &'static str {
        match self {
            Page::Welcome => "Welcome",
            Page::Claude => "Claude",
            Page::Project => "First project",
            Page::Ready => "Ready",
        }
    }
}

/// The CLI every Claude account runs.
const CLAUDE_CLI: &str = "claude";

/// A row of the Claude page: the CLI, then the CLAUDE ACCOUNTS rows of
/// Settings → Accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaudeRow {
    Cli,
    Account(AccountRow),
}

fn claude_rows(cfg: &Config) -> Vec<ClaudeRow> {
    std::iter::once(ClaudeRow::Cli)
        .chain(cfg.account_rows().into_iter().map(ClaudeRow::Account))
        .collect()
}

/// The First project page's rows under the project: its rows of
/// Settings → Projects, by kind.
const PROJECT_ROWS: &[SettingKind] = &[SettingKind::BaseBranch, SettingKind::RunCommand];

/// The first-run wizard's live state. `area` is written back on draw so a
/// click outside can dismiss it the same way Esc does.
#[derive(Debug, Clone, Default)]
pub struct OnboardView {
    pub page: usize,
    pub row: usize,
    pub area: Rect,
    /// A typed row being edited in place — the project's base branch or
    /// run command — and its text so far.
    pub field: Option<(SettingKind, TextInput)>,
    /// What the Claude page is asking, when it is asking.
    pub account: AccountStep,
    /// The install `i` asked about: its command, which Enter runs.
    pub install: Option<Plan>,
    /// The page's last word: an account added, a name refused, how an
    /// install went.
    pub note: Option<String>,
}

/// The Claude page's questions, one at a time — the same flow as
/// Settings → Accounts: an email to sign in as, or a new account's name
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
    /// Whether a field or a question has the keys, so Esc answers it
    /// rather than skipping the wizard.
    pub fn asking(&self) -> bool {
        self.field.is_some() || self.account != AccountStep::Rows || self.install.is_some()
    }

    /// The page the wizard stands on.
    fn current(&self) -> Page {
        PAGES[self.page.min(PAGES.len() - 1)]
    }
}

/// Open the wizard over whatever is up, on its first page: at startup
/// while `onboarded` is false, from the palette's **Run setup** and from
/// `orion setup`.
pub fn open(app: &mut App) {
    app.overlay = Some(Overlay::Onboard(OnboardView::default()));
    app.dirty = true;
}

/// Leave the wizard and remember it was seen — this build's release notes
/// with it, so the next launch goes straight to the grid. A test without
/// a config path does not persist.
pub fn dismiss(app: &mut App) {
    app.overlay = None;
    app.dirty = true;
    persist(|cfg| {
        cfg.onboarded = true;
        crate::whats_new::stamp_seen(cfg);
    });
}

pub fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Onboard(view)) = &mut app.overlay else {
        return;
    };
    if let Some((kind, input)) = &mut view.field {
        match key.code {
            KeyCode::Esc => view.field = None,
            KeyCode::Enter => {
                let (kind, value) = (*kind, input.as_str().to_string());
                view.field = None;
                if let Some(repo) = app.selected_project().map(|p| p.repo_path.clone()) {
                    persist(|cfg| {
                        cfg.set_project_text(&repo, kind, &value);
                    });
                }
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
            _ if keys::ENTER.matches(&key) => {
                view.install = None;
                view.note = Some(if crate::event_loop::open_url(plan.link) {
                    format!("Opened {}'s install page in your browser", plan.program)
                } else {
                    format!("{}'s install page: {}", plan.program, plan.link)
                });
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
        _ if keys::NEXT_PAGE.matches(&key) => turn(app, 1),
        _ if keys::PREV_PAGE.matches(&key) => turn(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_row(app, 1),
        KeyCode::Up | KeyCode::Char('k') => move_row(app, -1),
        _ if keys::ENTER.matches(&key) => activate(app, out),
        _ if keys::TOGGLE.matches(&key) => toggle(app, out),
        _ if keys::INSTALL.matches(&key) && view.current() == Page::Claude => ask_install(app),
        _ => {}
    }
}

/// A click outside while a page asks something: the question goes, as Esc
/// takes it back, and the wizard stays — what was typed is not worth
/// skipping setup over.
pub fn back_out(app: &mut App) {
    set_view(app, |view| {
        view.field = None;
        view.account = AccountStep::Rows;
        view.install = None;
    });
}

/// The wizard's own keys: one table [`handle_key`] matches and [`hints`]
/// spells.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const NEXT_PAGE: Key = Key::new(&["right", "tab", "l"], "next");
    pub const PREV_PAGE: Key = Key::new(&["left", "shift+tab", "h"], "back");
    pub const ENTER: Key = Key::new(&["enter"], "continue");
    pub const TOGGLE: Key = Key::new(&["space"], "toggle");
    /// The Claude CLI while it isn't on PATH: its installer.
    pub const INSTALL: Key = Key::new(&["i"], "install");
    pub const SKIP: Key = Key::new(&["esc"], "skip");
    /// The account question's answers.
    pub const YES: Key = Key::new(&["enter", "y"], "share").show(2);
    pub const NO: Key = Key::new(&["n"], "start empty");
    #[cfg(test)]
    pub const ALL: &[Key] = &[NEXT_PAGE, PREV_PAGE, ENTER, TOGGLE, INSTALL, SKIP, YES, NO];
}

/// The keys on the wizard's bottom border, for the page it is on and the
/// question, if one is being asked.
pub(crate) fn hints(cfg: &Config, view: &OnboardView) -> Vec<crate::hints::Hint> {
    use crate::hints::Hint;
    let back = Hint::new("Esc", "back");
    if view.field.is_some() {
        return vec![
            keys::ENTER.hint_as("save").kept(),
            Hint::new("Esc", "cancel"),
        ];
    }
    if let Some(plan) = &view.install {
        let run = if plan.runnable() {
            "run it here"
        } else {
            "open the page"
        };
        return vec![keys::ENTER.hint_as(run).kept(), back];
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
    let page = view.current();
    let mut hints = match page {
        Page::Welcome => vec![keys::ENTER.hint_as("start").kept()],
        Page::Claude => match claude_rows(cfg).get(view.row) {
            Some(ClaudeRow::Cli) => vec![keys::ENTER.hint_as("check").kept()],
            Some(ClaudeRow::Account(AccountRow::Add)) => {
                vec![keys::ENTER.hint_as("add an account").kept()]
            }
            _ => vec![
                keys::ENTER.hint_as("sign in").kept(),
                keys::TOGGLE.hint_as("on/off"),
            ],
        },
        Page::Project => match view.row {
            0 => vec![
                keys::ENTER.hint_as("open a project").kept(),
                keys::TOGGLE.hint_as("next project"),
            ],
            _ => vec![keys::ENTER.hint_as("type it").kept()],
        },
        Page::Ready => vec![keys::ENTER.hint_as("start using Orion").kept()],
    };
    if page == Page::Claude && cli_plan().is_some() {
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

/// How the Claude CLI gets onto PATH, while it isn't there.
fn cli_plan() -> Option<Plan> {
    (!program_installed(CLAUDE_CLI))
        .then(|| Tools::here().agent_plan(CLAUDE_CLI))
        .flatten()
}

/// What the row under the cursor does, for the EXPLANATION line above the
/// keys.
fn explanation(app: &App, cfg: &Config, view: &OnboardView) -> String {
    // The install question says it all, over the rows.
    if view.install.is_some() {
        return String::new();
    }
    match view.current() {
        Page::Welcome | Page::Ready => String::new(),
        Page::Claude => match claude_rows(cfg).get(view.row) {
            Some(ClaudeRow::Cli) => cli_explanation(),
            Some(ClaudeRow::Account(row)) => cfg.account_hint(row),
            None => String::new(),
        },
        Page::Project => match view.row {
            0 => project_explanation(app),
            row => PROJECT_ROWS
                .get(row - 1)
                .and_then(|kind| crate::config::spec_for(*kind))
                .map(|spec| crate::hints::expand(spec.hint, &app.keymap))
                .unwrap_or_default(),
        },
    }
}

fn cli_explanation() -> String {
    if program_installed(CLAUDE_CLI) {
        return "Claude Code is installed: every Claude account below runs it, each signed in \
                with a login of its own"
            .into();
    }
    match Tools::here().agent_plan(CLAUDE_CLI) {
        Some(plan) if plan.runnable() => format!(
            "`{CLAUDE_CLI}` isn't on PATH — {} runs its installer: {}",
            keys::INSTALL.label(),
            plan.line
        ),
        Some(plan) => format!("`{CLAUDE_CLI}` isn't on PATH — install it: {}", plan.link),
        None => format!("`{CLAUDE_CLI}` isn't on PATH — install Claude Code, then come back"),
    }
}

fn project_explanation(app: &App) -> String {
    let enter = keys::ENTER.label();
    let space = keys::TOGGLE.label();
    let mut text = match app.launch_repo_name() {
        Some(name) => format!("{enter} opens {name}, the repo orion was started in, as a project"),
        None => format!(
            "{enter} opens a folder as a project — in setup's place; Run setup comes back here"
        ),
    };
    if app.tree.projects.len() > 1 {
        text.push_str(&format!("; {space} steps to the next project"));
    }
    text
}

/// The modal's width: room for the STEP STRIP and a row's columns, never
/// wider than the screen.
const MODAL_W: u16 = 92;

/// The width a sheet's lines are laid out at inside its border, on a
/// screen `frame` wide.
pub(crate) fn sheet_inner_width(frame: Rect) -> u16 {
    MODAL_W
        .min(frame.width.saturating_sub(2))
        .max(frame.width.min(24))
        .saturating_sub(2)
}

pub fn draw(f: &mut Frame, app: &mut App, view: &OnboardView, th: Theme) {
    let cfg = Config::load();
    let inner_w = sheet_inner_width(f.area());
    let body = page_body(app, &cfg, view, th, inner_w);
    let explain = explanation(app, &cfg, view);
    let strip = step_strip(view.page.min(PAGES.len() - 1), inner_w, th);
    let area = draw_sheet(
        f,
        " Orion setup ",
        Some(strip),
        body,
        &explain,
        &hints(&cfg, view),
        view.asking(),
        th,
    );
    if let Some(Overlay::Onboard(live)) = &mut app.overlay {
        live.area = area;
    }
}

/// One sheet over the grid — setup's pages and WHAT'S NEW alike: `strip`
/// across its top when it has one, then `body`, scrolled to keep the row
/// under the cursor — or, `asking`, the question at its end — in sight,
/// then `explain` over the keys on the bottom border. As tall as its
/// body, within the screen. Its area, for a click outside.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_sheet(
    f: &mut Frame,
    title: &str,
    strip: Option<Line<'static>>,
    body: Body,
    explain: &str,
    hints: &[crate::hints::Hint],
    asking: bool,
    th: Theme,
) -> Rect {
    let frame = f.area();
    let inner_w = sheet_inner_width(frame);
    let explain_rows = match explain.trim() {
        "" => 0,
        text => crate::hints::explain_lines(text, inner_w, 3).len() as u16 + 1,
    };
    // The strip and the blank under it, the body, and a blank over the
    // explanation; the borders.
    let strip_rows = if strip.is_some() { 2 } else { 0 };
    let want = strip_rows + body.lines.len() as u16 + explain_rows + 2;
    let height = want
        .min(frame.height.saturating_sub(2))
        .max(frame.height.min(8));
    let area = centered_rect(frame, inner_w + 2, height);
    f.render_widget(ratatui::widgets::Clear, area);
    let block = crate::hints::modal_block(crate::ui::modal_block(title, th), hints, area.width, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut rest = inner;
    if let Some(strip) = strip.filter(|_| inner.height > 0) {
        f.render_widget(Paragraph::new(strip), Rect { height: 1, ..inner });
        rest = Rect {
            y: inner.y + 2.min(inner.height),
            height: inner.height.saturating_sub(2),
            ..inner
        };
    }
    let (body_area, explain_row) = crate::hints::explain_area(explain, rest, 3);
    crate::hints::draw_explain(f, explain_row, explain, th);
    // A blank row between the body and the explanation when there is room.
    let room = usize::from(body_area.height)
        .saturating_sub(usize::from(explain_row.height > 0 && body_area.height > 4));
    let anchor = if asking {
        body.lines.len().saturating_sub(1)
    } else {
        body.selected.unwrap_or(0)
    };
    let first = (anchor + 1).saturating_sub(room);
    let lines: Vec<Line> = body.lines.into_iter().skip(first).take(room).collect();
    f.render_widget(Paragraph::new(lines), body_area);
    area
}

/// The STEP STRIP: every step by name, the one the wizard is on lit, the
/// ones behind it a shade brighter than the ones ahead. Too narrow for the
/// names, it is `Step 3 of 4 · First project`.
fn step_strip(at: usize, width: u16, th: Theme) -> Line<'static> {
    let full: usize = PAGES
        .iter()
        .map(|page| page.label().chars().count() + 3)
        .sum();
    if full > usize::from(width) {
        return Line::from(vec![
            Span::styled(
                format!(" Step {} of {} · ", at + 1, PAGES.len()),
                Style::default().fg(th.dim),
            ),
            Span::styled(
                PAGES[at].label(),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
        ]);
    }
    let mut spans = Vec::new();
    for (i, page) in PAGES.iter().enumerate() {
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

/// A sheet's body as drawn: its lines at the modal's inner width, and
/// which of them is the row under the cursor.
pub(crate) struct Body {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) selected: Option<usize>,
    /// The modal's inner width, which every line keeps a column short of.
    width: usize,
}

impl Body {
    pub(crate) fn new(width: u16) -> Self {
        Self {
            lines: Vec::new(),
            selected: None,
            width: usize::from(width),
        }
    }

    pub(crate) fn blank(&mut self) {
        self.lines.push(Line::from(""));
    }

    /// `text` wrapped to `width` with a column of margin each side.
    pub(crate) fn prose(&mut self, text: &str, width: u16, style: Style) {
        self.indented(text, 1, width, style);
    }

    /// [`Body::prose`] `indent` columns in, every wrapped row under the
    /// first — a command, a link.
    fn indented(&mut self, text: &str, indent: usize, width: u16, style: Style) {
        self.item("", text, indent, width, style);
    }

    /// `text` wrapped `indent` columns in behind `mark` (`• `), its wrapped
    /// rows hanging under the text rather than the mark — a list item.
    pub(crate) fn item(&mut self, mark: &str, text: &str, indent: usize, width: u16, style: Style) {
        let hang = indent + mark.chars().count();
        let room = usize::from(width).saturating_sub(hang + 1).max(1);
        for (i, line) in crate::pr_preview::wrap(text, room).into_iter().enumerate() {
            let lead = if i == 0 {
                format!("{}{mark}", " ".repeat(indent))
            } else {
                " ".repeat(hang)
            };
            self.lines
                .push(Line::from(Span::styled(format!("{lead}{line}"), style)));
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
    match view.current() {
        Page::Welcome => welcome(&mut body, app, th, width),
        Page::Claude => claude(&mut body, cfg, view, th, width),
        Page::Project => project(&mut body, app, cfg, view, th, width),
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
        "A minute of setup: Claude and its accounts, your first project, and the keys to know.",
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

/// The Claude page: the CLI `installed` or `install…`, then every account
/// — on or off, its dir and who it is signed in as — **Add account**, the
/// same-account warning when two share an email, and the question being
/// asked, if one is.
fn claude(body: &mut Body, cfg: &Config, view: &OnboardView, th: Theme, width: u16) {
    let dim = Style::default().fg(th.dim);
    let text = Style::default().fg(th.text);
    body.prose(
        "Orion's agents run on Claude Code. Each Claude account is a config dir with a login of \
         its own: sign the default one in, or add another to run sessions on a second \
         subscription.",
        width,
        dim,
    );
    body.blank();
    let rows = claude_rows(cfg);
    let installed = program_installed(CLAUDE_CLI);
    let mut cells: Vec<Vec<String>> = vec![vec![
        String::new(),
        "On".into(),
        "Config dir".into(),
        "Signed in".into(),
    ]];
    for row in &rows {
        cells.push(match row {
            ClaudeRow::Cli => vec![
                "Claude Code".into(),
                String::new(),
                CLAUDE_CLI.into(),
                if installed {
                    "installed".into()
                } else {
                    "install…".into()
                },
            ],
            ClaudeRow::Account(AccountRow::Add) => {
                let (dir, _) = cfg.account_parts(&AccountRow::Add);
                vec!["Add account".into(), String::new(), dir, String::new()]
            }
            ClaudeRow::Account(row @ AccountRow::Account(id)) => {
                let entry = cfg.effective_harness_by_id(id);
                let (dir, state) = cfg.account_parts(row);
                let on = if entry.enabled { "on" } else { "off" };
                vec![entry.display_label().to_string(), on.into(), dir, state]
            }
        });
    }
    let widths = column_widths(&cells, width);
    body.row(
        false,
        cells[0].iter().map(|c| (c.clone(), dim)).collect(),
        &widths,
        th,
    );
    for (i, (row, cell)) in rows.iter().zip(cells.iter().skip(1)).enumerate() {
        let state = match cell[3].as_str() {
            "signed in" | "installed" => Style::default().fg(th.ok),
            "install…" => Style::default().fg(th.accent),
            s if s.starts_with("same as") => Style::default().fg(th.warn),
            _ => dim,
        };
        let on = match cell[1].as_str() {
            "on" => Style::default().fg(th.ok),
            _ => dim,
        };
        let label = match row {
            ClaudeRow::Account(AccountRow::Add) => Style::default().fg(th.accent),
            _ => text,
        };
        body.row(
            i == view.row,
            vec![
                (cell[0].clone(), label),
                (cell[1].clone(), on),
                (cell[2].clone(), text),
                (cell[3].clone(), state),
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
                &format!(
                    "Name it — it goes by that name, its dir is ~/.claude-<name> (empty = \
                     {next}, no name):"
                ),
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

/// The First project page: the project the grid is on — or the repo
/// orion was started in, to open — then its **Base branch** and **Run
/// command**, the rows Settings → Projects has for it, typed in place.
fn project(body: &mut Body, app: &App, cfg: &Config, view: &OnboardView, th: Theme, width: u16) {
    let dim = Style::default().fg(th.dim);
    body.prose(
        "A project is a git repository. Its agents run in worktrees — a checkout of their own \
         branch each — cut from its base branch, so they work side by side without touching each \
         other's files.",
        width,
        dim,
    );
    body.blank();
    let selected = app.selected_project();
    let project = match (selected, app.launch_repo_name()) {
        (Some(p), _) => format!("{} · {}", p.name, p.repo_path.display()),
        (None, Some(name)) => format!("none yet — Enter opens {name}"),
        (None, None) => "none yet — Enter opens one".into(),
    };
    let settings = selected.map(|p| cfg.project(&p.repo_path));
    let editing = view.field.as_ref();
    let mut cells = vec![vec!["Project".to_string(), project]];
    for kind in PROJECT_ROWS {
        let label = crate::config::spec_for(*kind).map_or("", |s| s.label);
        let value = match (editing, &settings) {
            (Some((at, input)), _) if at == kind => format!("{}▌", input.as_str()),
            (_, Some(settings)) => settings.value_label(*kind),
            (_, None) => "open a project first".into(),
        };
        cells.push(vec![label.to_string(), value]);
    }
    let widths = column_widths(&cells, width);
    for (i, cell) in cells.into_iter().enumerate() {
        let value = if i == 0 && selected.is_none() {
            Style::default().fg(th.accent)
        } else {
            Style::default().fg(th.text)
        };
        body.row(
            i == view.row,
            vec![
                (cell[0].clone(), Style::default().fg(th.muted)),
                (cell[1].clone(), value),
            ],
            &widths,
            th,
        );
    }
}

/// What setup leaves set, a line a step: the Claude accounts that are on
/// (warned while none is, or the CLI is missing) and the project.
fn summary(app: &App, cfg: &Config) -> Vec<(&'static str, String, bool)> {
    let on: Vec<String> = cfg
        .harness_registry()
        .into_iter()
        .filter(|entry| entry.enabled && entry.is_claude_account())
        .map(|entry| entry.display_label().to_string())
        .collect();
    let claude = match (on.is_empty(), program_installed(CLAUDE_CLI)) {
        (true, _) => (
            "none on — turn one on in Settings → Accounts to start an agent".to_string(),
            true,
        ),
        (false, true) => (on.join(", "), false),
        (false, false) => (format!("{} (CLI not installed)", on.join(", ")), true),
    };
    let project = match app.selected_project() {
        Some(p) => {
            let base = match cfg.project(&p.repo_path).base_branch.trim() {
                "" => "origin's default branch".to_string(),
                name => name.to_string(),
            };
            (format!("{} · worktrees from {base}", p.name), false)
        }
        None => ("none yet — open one any time from the palette".into(), true),
    };
    vec![
        ("Claude", claude.0, claude.1),
        ("Project", project.0, project.1),
    ]
}

/// The keys that matter, from the live keymap — an unbound one left out.
fn next_keys(keymap: &Keymap) -> Vec<(String, &'static str)> {
    [
        (Action::QuickPrompt, "start an agent: type the task, Enter"),
        (Action::Palette, "jump to any project, worktree or session"),
        (Action::GitDiff, "the checkout's changes, commit by commit"),
        (Action::Linear, "your Linear issues"),
        (Action::Todos, "your todos, a list per project"),
        (Action::Settings, "Settings: change any of this"),
    ]
    .into_iter()
    .filter_map(|(action, does)| crate::hints::key(keymap, action).map(|key| (key, does)))
    .collect()
}

fn ready(body: &mut Body, app: &App, cfg: &Config, th: Theme, width: u16) {
    body.lines.push(Line::from(Span::styled(
        " You're set.",
        Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
    )));
    body.blank();
    let chosen = summary(app, cfg);
    let cells: Vec<Vec<String>> = chosen
        .iter()
        .map(|(what, value, _)| vec![what.to_string(), value.clone()])
        .collect();
    let widths = column_widths(&cells, width);
    for (what, value, warn) in &chosen {
        let style = Style::default().fg(if *warn { th.warn } else { th.text });
        body.row(
            false,
            vec![
                (what.to_string(), Style::default().fg(th.muted)),
                (value.clone(), style),
            ],
            &widths,
            th,
        );
    }
    body.blank();
    body.heading("The keys that matter", th);
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
                "Homebrew isn't installed, so Orion can't install {} itself. {} opens its install \
                 page:",
                plan.program,
                keys::ENTER.label()
            ),
            width,
            text,
        );
        body.indented(plan.link, 3, width, Style::default().fg(th.accent));
    }
}

/// Step a page on (`delta` 1) or back (-1); off the last page is done.
fn turn(app: &mut App, delta: i32) {
    let Some(Overlay::Onboard(view)) = &mut app.overlay else {
        return;
    };
    let next = view.page as i32 + delta;
    if next >= PAGES.len() as i32 {
        dismiss(app);
        return;
    }
    if next < 0 {
        return;
    }
    view.page = next as usize;
    view.row = 0;
    view.note = None;
    app.dirty = true;
}

fn page_rows(page: Page, cfg: &Config) -> usize {
    match page {
        Page::Claude => claude_rows(cfg).len(),
        Page::Project => 1 + PROJECT_ROWS.len(),
        Page::Welcome | Page::Ready => 0,
    }
}

fn move_row(app: &mut App, delta: i32) {
    let cfg = Config::load();
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        let n = page_rows(view.current(), &cfg);
        if n == 0 {
            return;
        }
        view.row = (view.row as i32 + delta).rem_euclid(n as i32) as usize;
        view.note = None;
        app.dirty = true;
    }
}

/// The page and row the wizard's cursor is on.
fn cursor(app: &App) -> Option<(Page, usize)> {
    match &app.overlay {
        Some(Overlay::Onboard(view)) => Some((view.current(), view.row)),
        _ => None,
    }
}

fn set_view(app: &mut App, edit: impl FnOnce(&mut OnboardView)) {
    if let Some(Overlay::Onboard(view)) = &mut app.overlay {
        edit(view);
    }
    app.dirty = true;
}

fn activate(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some((page, row)) = cursor(app) else {
        return;
    };
    match page {
        Page::Welcome | Page::Ready => turn(app, 1),
        Page::Claude => {
            let cfg = Config::load();
            let step = match claude_rows(&cfg).into_iter().nth(row) {
                Some(ClaudeRow::Cli) => return ask_install(app),
                Some(ClaudeRow::Account(AccountRow::Account(id))) => AccountStep::Email {
                    id,
                    input: TextInput::new(),
                },
                Some(ClaudeRow::Account(AccountRow::Add)) => AccountStep::Name(TextInput::new()),
                None => return,
            };
            set_view(app, |view| {
                view.account = step;
                view.note = None;
            });
        }
        Page::Project if row == 0 => open_project(app, out),
        Page::Project => {
            let Some(kind) = PROJECT_ROWS.get(row - 1).copied() else {
                return;
            };
            let Some(repo) = app.selected_project().map(|p| p.repo_path.clone()) else {
                return set_view(app, |view| {
                    view.note = Some("Open a project first: Enter on the Project row".into());
                });
            };
            let input = TextInput::with_text(Config::load().project_text_value(&repo, kind));
            set_view(app, |view| view.field = Some((kind, input)));
        }
    }
}

/// Enter on the Project row: the repo orion was started in, opened as a
/// project — the project the page is on once it lands — or, with none to
/// open, the open-project prompt in setup's place.
fn open_project(app: &mut App, out: &mut Vec<ClientRequest>) {
    match app
        .launch_repo
        .clone()
        .filter(|_| app.launch_repo_name().is_some())
    {
        Some(repo) => crate::event_loop::open_folder(app, repo, out),
        None => crate::event_loop::open_prompt(app, crate::app::PromptKind::AddProject),
    }
}

/// Space on the Project row: the next project the tree has, which the
/// grid moves to as well.
fn next_project(app: &mut App, out: &mut Vec<ClientRequest>) {
    let projects = &app.tree.projects;
    if projects.len() < 2 {
        return;
    }
    let at = app
        .selected_project()
        .and_then(|p| projects.iter().position(|q| q.id == p.id))
        .map_or(0, |i| (i + 1) % projects.len());
    let id = projects[at].id.clone();
    crate::event_loop::select_project(app, &id, out);
    app.dirty = true;
}

/// `i`: the Claude CLI's install, asked before it runs — or, when there
/// is nothing to install, why not.
fn ask_install(app: &mut App) {
    let plan = cli_plan();
    set_view(app, |view| match plan {
        Some(plan) => view.install = Some(plan),
        None if program_installed(CLAUDE_CLI) => {
            view.note = Some(format!("{CLAUDE_CLI} is already installed"));
        }
        None => view.note = Some(format!("Orion knows no installer for `{CLAUDE_CLI}`")),
    });
}

/// A key while the Claude page asks something: the field takes the
/// typing, Enter answers, Esc goes back to the rows. Signing an account
/// in switches it on: an account is signed in to be used.
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
            persist(|cfg| {
                if !cfg.effective_harness_by_id(&id).enabled {
                    cfg.cycle_agent_row(&id, crate::config::HarnessField::Enabled, 0);
                }
            });
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
    let row = claude_rows(&Config::load()).iter().position(
        |row| matches!(row, ClaudeRow::Account(AccountRow::Account(id)) if *id == new.id),
    );
    set_view(app, |view| {
        view.account = AccountStep::Rows;
        view.row = row.unwrap_or(view.row);
        view.note = Some(match result {
            Ok(note) => note,
            Err(why) => why,
        });
    });
    crate::claude_accounts::request_refresh(app, true);
}

/// Space: an account on or off; the next project.
fn toggle(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some((page, row)) = cursor(app) else {
        return;
    };
    match page {
        Page::Claude => {
            if let Some(ClaudeRow::Account(AccountRow::Account(id))) =
                claude_rows(&Config::load()).into_iter().nth(row)
            {
                persist(|cfg| cfg.cycle_agent_row(&id, crate::config::HarnessField::Enabled, 0));
            }
        }
        Page::Project if row == 0 => next_project(app, out),
        Page::Welcome | Page::Project | Page::Ready => {}
    }
    app.dirty = true;
}

pub(crate) fn persist(edit: impl FnOnce(&mut Config)) {
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
        handle_key(app, KeyEvent::from(code), &mut Vec::new());
    }

    fn view(app: &App) -> &OnboardView {
        match &app.overlay {
            Some(Overlay::Onboard(view)) => view,
            other => panic!("expected onboard, got {other:?}"),
        }
    }

    /// Walk the wizard onto `page`.
    fn to_page(app: &mut App, page: Page) {
        while view(app).current() != page {
            press(app, KeyCode::Right);
        }
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
        draw_at(app, 96, 28)
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

    /// Setup is four pages, in order, named once each on the step strip
    /// — which narrows to `Step n of m` when the names don't fit — and
    /// stepping off the last closes it.
    #[test]
    fn setup_is_four_pages_named_on_the_strip() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app);
            let shot = draw_text(&mut app);
            let strip = shot.lines().find(|l| l.contains("Welcome")).unwrap();
            for name in ["Welcome", "Claude", "First project", "Ready"] {
                assert_eq!(strip.matches(name).count(), 1, "{name}: {strip}");
            }
            for old in ["Agents", "Editor", "Worktrees", "Linear", "Autofix", "App"] {
                assert!(!strip.contains(old), "{old} is gone: {strip}");
            }
            let narrow = draw_at(&mut app, 40, 20);
            assert!(narrow.contains("Step 1 of 4"), "{narrow}");
            let mut seen = Vec::new();
            for _ in 0..PAGES.len() {
                seen.push(view(&app).current());
                press(&mut app, KeyCode::Right);
            }
            assert_eq!(seen, PAGES);
            assert!(app.overlay.is_none(), "off the last page is done");
            assert!(Config::load().onboarded);
        });
    }

    #[test]
    fn dismiss_stamps_onboarded_and_this_builds_notes_and_closes() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Esc);
            assert!(app.overlay.is_none());
            let cfg = Config::load();
            assert!(cfg.onboarded);
            assert_eq!(cfg.seen_version, crate::whats_new::current());
        });
    }

    #[test]
    fn enter_on_welcome_advances() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app);
            press(&mut app, KeyCode::Enter);
            assert_eq!(view(&app).current(), Page::Claude);
        });
    }

    /// The Claude page: the CLI first, `installed` or `install…` — `i`
    /// shows its installer, Esc backs out of the question, never the
    /// wizard — then every account and Add.
    #[test]
    fn the_claude_page_offers_the_cli_and_every_account() {
        with_temp_home(|_| {
            with_programs(&["brew"], || {
                let mut app = App::new();
                open(&mut app);
                to_page(&mut app, Page::Claude);
                let shot = draw_text(&mut app);
                let inner = words(&shot);
                for needle in [
                    "On Config dir Signed in",
                    "› Claude Code claude install…",
                    "Add account ~/.claude-2",
                ] {
                    assert!(inner.contains(needle), "{needle}:\n{shot}");
                }
                assert!(shot.contains("i install"), "{shot}");
                press(&mut app, KeyCode::Char('i'));
                let plan = view(&app)
                    .install
                    .clone()
                    .expect("the installer is asked about");
                assert_eq!(plan.program, "claude");
                assert!(view(&app).asking());
                press(&mut app, KeyCode::Esc);
                assert!(view(&app).install.is_none(), "the question, not the wizard");
                assert!(app.overlay.is_some());
            });
            with_programs(&["claude"], || {
                let mut app = App::new();
                open(&mut app);
                to_page(&mut app, Page::Claude);
                let inner = words(&draw_text(&mut app));
                assert!(inner.contains("Claude Code claude installed"), "{inner}");
            });
        });
    }

    /// Enter on an account asks for the email, then runs `claude auth
    /// login` over the wizard and switches the account on; Space flips it.
    #[test]
    fn enter_on_an_account_signs_it_in_and_switches_it_on() {
        with_temp_home(|_| {
            let mut app = App::new();
            open(&mut app);
            to_page(&mut app, Page::Claude);
            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Char(' '));
            assert!(!Config::load().effective_harness_by_id("claude").enabled);
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
            assert!(
                Config::load().effective_harness_by_id("claude").enabled,
                "on"
            );
        });
    }

    /// **Add account** on the wizard: a name, the share question, and the
    /// new account's row under the cursor, ready to sign in.
    #[test]
    fn add_account_on_the_wizard_names_and_shares() {
        with_temp_home(|root| {
            let mut app = App::new();
            open(&mut app);
            to_page(&mut app, Page::Claude);
            press(&mut app, KeyCode::Down);
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
            assert_eq!(view(&app).row, 2, "on the new account");
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

    /// Two accounts signed in as one email are drawn as such, with the
    /// same-account warning under them.
    #[test]
    fn the_claude_page_draws_who_each_account_is() {
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
            open(&mut app);
            to_page(&mut app, Page::Claude);
            press(&mut app, KeyCode::Down);
            let text = draw_text(&mut app);
            let inner = words(&text);
            for needle in [
                "› Claude (a@b.co) on ~/.claude same as ~/.claude-2",
                "Claude (a@b.co) on ~/.claude-2 same as ~/.claude",
                "Add account ~/.claude-3",
                "⚠ ~/.claude and ~/.claude-2 are signed in as one account,",
            ] {
                assert!(inner.contains(needle), "{needle}:\n{text}");
            }
            assert!(text.contains("Enter sign in"), "{text}");
            assert!(!text.contains('['), "no bracketed values:\n{text}");
        });
    }

    fn with_project<T>(app: &mut App, f: impl FnOnce(&mut App) -> T) -> T {
        app.tree.projects.push(orion_core::Project {
            id: orion_core::ProjectId("p1".into()),
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            sort_order: 0,
        });
        app.set_sel_project(0);
        f(app)
    }

    /// The First project page: the project the grid is on, then its base
    /// branch and run command, typed in place into that project's entry —
    /// the rows Settings → Projects has for it.
    #[test]
    fn the_first_project_page_sets_its_base_branch_and_run_command() {
        with_temp_config(|| {
            let mut app = App::new();
            open(&mut app);
            to_page(&mut app, Page::Project);
            let inner = words(&draw_text(&mut app));
            assert!(inner.contains("Project none yet — Enter opens"), "{inner}");
            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Enter);
            assert!(!view(&app).asking(), "nothing to type into yet");
            assert!(view(&app).note.is_some());

            with_project(&mut app, |app| {
                let inner = words(&draw_text(app));
                for needle in [
                    "Project demo · /tmp/demo",
                    "› Base branch auto",
                    "Run command not set",
                ] {
                    assert!(inner.contains(needle), "{needle}:\n{inner}");
                }
                press(app, KeyCode::Enter);
                assert!(view(app).asking());
                for c in "dev".chars() {
                    press(app, KeyCode::Char(c));
                }
                press(app, KeyCode::Enter);
                press(app, KeyCode::Down);
                press(app, KeyCode::Enter);
                for c in "just start".chars() {
                    press(app, KeyCode::Char(c));
                }
                press(app, KeyCode::Enter);
                let project = Config::load().project(std::path::Path::new("/tmp/demo"));
                assert_eq!(project.base_branch, "dev");
                assert_eq!(project.run_command, "just start");
                let inner = words(&draw_text(app));
                assert!(inner.contains("Base branch dev"), "{inner}");
            });
        });
    }

    /// Ready: what was set, and the keys that matter from the live keymap.
    #[test]
    fn the_ready_page_names_the_keys_that_matter() {
        with_temp_config(|| {
            with_programs(&["claude"], || {
                persist(|cfg| cfg.claude_enabled = false);
                let mut app = App::new();
                open(&mut app);
                to_page(&mut app, Page::Ready);
                let inner = words(&draw_text(&mut app));
                assert!(inner.contains("Claude none on"), "{inner}");
                for key in ["⌘N", "⌘K", "⌘E", "⌘L", "⌘I", "⌘,"] {
                    assert!(inner.contains(key), "{key}: {inner}");
                }
                press(&mut app, KeyCode::Enter);
                assert!(app.overlay.is_none(), "Enter starts using Orion");
            });
        });
    }

    /// Every page fits the screen, its prose wrapped a column short of
    /// the frame and its keys on the bottom border.
    #[test]
    fn every_page_fits_and_wraps() {
        with_temp_config(|| {
            with_programs(&["claude", "brew"], || {
                for (w, h) in [(150, 40), (90, 28)] {
                    let mut app = App::new();
                    open(&mut app);
                    for page in 0..PAGES.len() {
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
                        let right = rows[top].iter().position(|c| *c == '╮').unwrap();
                        for row in &rows[top + 1..bottom] {
                            assert_eq!(row[right], '│', "the frame:\n{shot}");
                            assert_eq!(row[right - 1], ' ', "a line cut at the frame:\n{shot}");
                        }
                        assert!(
                            bottom - top < usize::from(h) - 1,
                            "page {page} at {w}x{h} fits:\n{shot}"
                        );
                        if page + 1 < PAGES.len() {
                            press(&mut app, KeyCode::Right);
                        }
                    }
                }
            });
        });
    }
}
