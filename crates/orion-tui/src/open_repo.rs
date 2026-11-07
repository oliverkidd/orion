//! The OPEN REPO modal: `⌘O` on HOME (or the splash), the COMMAND
//! PALETTE's **Open a folder as a project**, the header's `+`. It lists the
//! git repositories in the places people keep code — `~/Desktop`,
//! `~/Documents`, `~/Code` and their like, and the folders the projects
//! already live in — each its folder's name over its path. Space ticks,
//! Enter adds the ticked ones (or the highlighted one), and Tab, or a
//! typed `/` or `~`, browses for a folder instead: the open-project prompt.
//!
//! The SCAN is a bounded directory walk, never a search of the whole disk:
//! a handful of roots, a few levels deep, stopping at every `.git` it
//! meets and never going into `node_modules`, `target` or a dot-folder. It
//! runs off the loop and lands on `App::open_repo.tx`. A linked worktree
//! (a `.git` FILE) is never listed: it belongs to the project its main
//! checkout is.
//!
//! The same walk, a little shallower, looks inside a folder the prompt is
//! given that is in no repository ([`nested`]): one folder too high, the
//! repository under it is the one meant.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use orion_core::protocol::ClientRequest;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::theme::Theme;

// ---- the scan ----

/// The folders under `~` the scan starts from, when they exist. Others
/// (case-insensitive twins like `~/code` beside `~/Code`) are deduped.
const ROOTS: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Developer",
    "Code",
    "code",
    "Projects",
    "projects",
    "dev",
    "src",
    "repos",
    "git",
    "workspace",
    "Sites",
];

/// How far under a root the scan looks.
pub const SCAN_DEPTH: usize = 4;
/// How far under a folder the prompt was given [`nested`] looks.
pub const NESTED_DEPTH: usize = 3;
/// The most folders one scan reads, so a huge `~/Documents` can't stall it.
const MAX_DIRS: usize = 30_000;

/// Folders never walked into.
fn skipped(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules"
                | "target"
                | "Library"
                | "vendor"
                | "venv"
                | "build"
                | "dist"
                | "Pods"
                | "DerivedData"
        )
        || name.ends_with(".app")
        || name.ends_with(".photoslibrary")
}

/// A repository the scan found: its main checkout, and when its `.git`
/// last changed (for newest-first).
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    pub touched: Option<SystemTime>,
}

/// Every repository at most `depth` folders under each of `roots`, newest
/// first. A `.git` directory marks one and ends the walk there; a `.git`
/// file (a linked worktree, a submodule) ends it without listing it.
pub fn scan(roots: &[PathBuf], depth: usize) -> Vec<Found> {
    use std::os::unix::fs::MetadataExt;
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    let mut budget = MAX_DIRS;
    let mut stack: Vec<(PathBuf, usize)> = Vec::new();
    for root in roots {
        stack.push((root.clone(), 0));
        while let Some((dir, level)) = stack.pop() {
            // Case-insensitive twins, and a folder reached twice.
            let Ok(meta) = std::fs::metadata(&dir) else {
                continue;
            };
            if !meta.is_dir() || !seen.insert((meta.dev(), meta.ino())) {
                continue;
            }
            if budget == 0 {
                break;
            }
            budget -= 1;
            let git = dir.join(".git");
            if let Ok(g) = std::fs::symlink_metadata(&git) {
                if g.is_dir() {
                    found.push(Found {
                        path: dir,
                        touched: g.modified().ok(),
                    });
                }
                continue;
            }
            if level >= depth {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                // Symlinks are not followed: no loops, no walking off the
                // roots.
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if !kind.is_dir() || skipped(&entry.file_name().to_string_lossy()) {
                    continue;
                }
                stack.push((entry.path(), level + 1));
            }
        }
    }
    found.sort_by(|a, b| b.touched.cmp(&a.touched).then_with(|| a.path.cmp(&b.path)));
    found
}

/// The repositories inside `dir` — itself in none — at most
/// [`NESTED_DEPTH`] down: what a folder one level too high was meant as.
pub fn nested(dir: &Path) -> Vec<PathBuf> {
    scan(&[dir.to_path_buf()], NESTED_DEPTH)
        .into_iter()
        .map(|f| f.path)
        .collect()
}

/// Where the scan starts: the [`ROOTS`] under `home`, and the folder each
/// project's repository sits in (its siblings are likely repos too).
fn roots(app: &App, home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = home
        .map(|h| ROOTS.iter().map(|r| h.join(r)).collect())
        .unwrap_or_default();
    for p in &app.tree.projects {
        if let Some(parent) = p.repo_path.parent() {
            // Never the home folder itself, nor `/`: that is the whole disk.
            if Some(parent) != home && parent.parent().is_some() {
                roots.push(parent.to_path_buf());
            }
        }
    }
    roots
}

// ---- the modal ----

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    /// The folder's name.
    pub name: String,
    /// Already a project: shown, never ticked.
    pub added: bool,
    pub on: bool,
}

#[derive(Debug, Clone, Default)]
pub struct OpenRepoView {
    pub scanning: bool,
    /// Ticked from the start: [`nested`]'s repos, all of them meant.
    pub preset: bool,
    pub rows: Vec<Row>,
    /// Typed to narrow the list by name or path.
    pub filter: String,
    /// Index into [`Self::shown`].
    pub cursor: usize,
    pub first: usize,
    pub area: Rect,
    pub list_area: Rect,
}

impl OpenRepoView {
    /// The rows the filter keeps, as indexes into `rows`.
    pub fn shown(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                needle.is_empty()
                    || r.name.to_lowercase().contains(&needle)
                    || r.path.to_string_lossy().to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn chosen(&self) -> Vec<PathBuf> {
        self.rows
            .iter()
            .filter(|r| r.on && !r.added)
            .map(|r| r.path.clone())
            .collect()
    }

    /// The `rows` index the cursor is on: line 0 is CLONE FROM GITHUB,
    /// the repos follow.
    fn at_cursor(&self) -> Option<usize> {
        let line = self.cursor.checked_sub(1)?;
        self.shown().get(line).copied()
    }

    /// Back to the top of a fresh list: the first repo, or the clone row
    /// when there is none.
    fn home_cursor(&mut self) {
        self.cursor = usize::from(!self.shown().is_empty());
    }

    fn toggle_cursor(&mut self) {
        if let Some(i) = self.at_cursor() {
            let r = &mut self.rows[i];
            if !r.added {
                r.on = !r.on;
            }
        }
    }

    /// Tick every shown row that can be, or untick them all if they are.
    fn toggle_all(&mut self) {
        let shown = self.shown();
        let all = shown
            .iter()
            .filter(|&&i| !self.rows[i].added)
            .all(|&i| self.rows[i].on);
        for i in shown {
            if !self.rows[i].added {
                self.rows[i].on = !all;
            }
        }
    }
}

/// Fill `rows` from what was found: added projects marked, the rest
/// ticked when the modal was opened on them ([`open_on`]).
fn rows_of(app: &App, found: Vec<PathBuf>, on: bool) -> Vec<Row> {
    let mut rows: Vec<Row> = found
        .into_iter()
        .map(|path| {
            let added = app.tree.project_at_path(&path).is_some();
            Row {
                name: path
                    .file_name()
                    .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into()),
                added,
                on: on && !added,
                path,
            }
        })
        .collect();
    // The ones still to add first; the scan's newest-first order within.
    rows.sort_by_key(|r| r.added);
    rows
}

/// What lands on `App::open_repo.tx`.
#[derive(Debug)]
pub struct Answer {
    pub generation: u64,
    pub found: Vec<PathBuf>,
}

#[derive(Default)]
pub struct Shared {
    /// Installed at startup; `None` in the unit tests, where no scan runs
    /// (it would walk the real home folder).
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
    /// Bumped on every open, so a slow scan from an earlier one is dropped.
    pub generation: u64,
}

pub(crate) mod keys {
    use crate::hints::Key;

    pub const TICK: Key = Key::new(&["space"], "select");
    pub const ALL: Key = Key::new(&["ctrl+a"], "all");
    pub const ADD: Key = Key::new(&["enter"], "add");
    pub const BROWSE: Key = Key::new(&["tab"], "browse…");
    #[cfg(test)]
    pub const EVERY: &[Key] = &[TICK, ALL, ADD, BROWSE];
}

/// Open the modal and start the scan.
pub(crate) fn start(app: &mut App) {
    app.dirty = true;
    app.open_repo.generation += 1;
    let generation = app.open_repo.generation;
    app.overlay = Some(Overlay::OpenRepo(OpenRepoView {
        scanning: app.open_repo.tx.is_some(),
        ..OpenRepoView::default()
    }));
    let Some(tx) = app.open_repo.tx.clone() else {
        return;
    };
    let home = orion_core::env::home_dir();
    let roots = roots(app, home.as_deref());
    tokio::task::spawn_blocking(move || {
        let found = scan(&roots, SCAN_DEPTH).into_iter().map(|f| f.path).collect();
        let _ = tx.send(Answer { generation, found });
    });
}

/// Open the modal on `found` — the repos inside a folder that is in none —
/// every one ticked, no scan.
pub(crate) fn open_on(app: &mut App, found: Vec<PathBuf>) {
    app.dirty = true;
    app.open_repo.generation += 1;
    let rows = rows_of(app, found, true);
    app.overlay = Some(Overlay::OpenRepo(OpenRepoView {
        preset: true,
        rows,
        ..OpenRepoView::default()
    }));
}

/// A scan landed: fill the modal if this open asked for it.
pub(crate) fn land(app: &mut App, answer: Answer) {
    if answer.generation != app.open_repo.generation {
        return;
    }
    let rows = rows_of(app, answer.found, false);
    let Some(Overlay::OpenRepo(view)) = &mut app.overlay else {
        return;
    };
    if view.preset {
        return;
    }
    app.dirty = true;
    view.rows = rows;
    view.scanning = false;
    view.home_cursor();
}

/// Tab (or a typed `/`, `~`): the open-project prompt, holding `typed`.
fn browse(app: &mut App, typed: Option<char>) {
    crate::event_loop::open_prompt(app, crate::app::PromptKind::AddProject);
    if let (Some(c), Some(Overlay::Prompt(p))) = (typed, &mut app.overlay) {
        p.input.set_text(c.to_string());
        p.refresh_dirs();
    }
}

/// Enter: add the ticked repos, or the highlighted one with none ticked.
fn add(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::OpenRepo(view)) = &app.overlay else {
        return;
    };
    let mut paths = view.chosen();
    if paths.is_empty() {
        match view.at_cursor() {
            Some(i) => paths.push(view.rows[i].path.clone()),
            None if view.cursor == 0 => return crate::clone_repo::start(app),
            None => return,
        }
    }
    app.overlay = None;
    crate::event_loop::add_projects(app, paths, out);
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::OpenRepo(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    // The clone row, then the repos.
    let len = view.shown().len() + 1;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        _ if crate::nav::closes(&key) => app.overlay = None,
        KeyCode::Down => view.cursor = crate::app::clamp_selection(view.cursor as i64 + 1, len),
        KeyCode::Up => view.cursor = crate::app::clamp_selection(view.cursor as i64 - 1, len),
        _ if keys::ALL.matches(&key) => view.toggle_all(),
        _ if keys::TICK.matches(&key) => view.toggle_cursor(),
        _ if keys::ADD.matches(&key) => add(app, out),
        _ if keys::BROWSE.matches(&key) => browse(app, None),
        // A path being typed: that is browsing.
        KeyCode::Char(c @ ('/' | '~')) if view.filter.is_empty() && !ctrl => {
            browse(app, Some(c));
        }
        KeyCode::Char('u') if ctrl => {
            view.filter.clear();
            view.home_cursor();
        }
        KeyCode::Backspace => {
            view.filter.pop();
            view.home_cursor();
        }
        KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::SUPER) => {
            view.filter.push(c);
            view.home_cursor();
        }
        _ => {}
    }
}

/// A click on a row ticks it, as space there would.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) {
    let Some(Overlay::OpenRepo(view)) = &mut app.overlay else {
        return;
    };
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }
    app.dirty = true;
    // Two lines a row: the name, the path under it.
    let lines = (view.shown().len() + 1) * 2;
    if let Some(at) = crate::list_hit::row_at(view.list_area, view.first * 2, lines, pos) {
        view.cursor = at / 2;
        if view.cursor == 0 {
            return crate::clone_repo::start(app);
        }
        view.toggle_cursor();
    }
}

const WIDEST: u16 = 84;
/// Lines that aren't the list: the filter, a blank, the summary.
const HEAD_H: u16 = 2;
const FOOT_H: u16 = 2;

/// `~/…` for a path under the home folder.
fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &OpenRepoView, th: Theme) {
    let screen = f.area();
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let inner_w = usize::from(width.saturating_sub(2));
    let dim = Style::default().fg(th.dim);
    let home = orion_core::env::home_dir();
    let shown = view.shown();

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(" ⌕ ", Style::default().fg(th.muted)),
            if view.filter.is_empty() {
                Span::styled("type to filter · / or ~ to type a path", dim)
            } else {
                Span::styled(view.filter.clone(), Style::default().fg(th.text))
            },
        ]),
        Line::from(""),
    ];
    if view.scanning {
        lines.push(Line::from(Span::styled(
            " Looking for git repositories…",
            dim,
        )));
    } else if shown.is_empty() {
        lines.push(Line::from(Span::styled(
            if view.rows.is_empty() {
                " No git repositories found — Tab to browse for a folder."
            } else {
                " Nothing matches."
            },
            dim,
        )));
    }
    let fits = usize::from(screen.height.saturating_sub(4 + HEAD_H + FOOT_H).max(2)) / 2;
    let cursor = view.cursor.min(shown.len());
    let first = view
        .first
        .min(cursor)
        .max((cursor + 1).saturating_sub(fits));
    let list_at = lines.len();
    for n in (first..=shown.len()).take(fits) {
        let sel = |s: Style| {
            if n == cursor {
                s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                s
            }
        };
        let Some(&i) = n.checked_sub(1).and_then(|k| shown.get(k)) else {
            // Line 0: CLONE FROM GITHUB.
            let a = "  +  Clone from GitHub…";
            let b = "     a repo you can reach, or paste a URL — cloned and added in one go";
            for (t, st) in [(a, Style::default().fg(th.accent)), (b, dim)] {
                let t = crate::ui::truncate(t, inner_w);
                let pad = inner_w.saturating_sub(t.chars().count());
                lines.push(Line::from(Span::styled(format!("{t}{}", " ".repeat(pad)), sel(st))));
            }
            continue;
        };
        let r = &view.rows[i];
        let sel = |s: Style| {
            if n == cursor {
                s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                s
            }
        };
        let tick = match (r.added, r.on) {
            (true, _) => "     ",
            (false, true) => " [x] ",
            (false, false) => " [ ] ",
        };
        let tag = if r.added { "added ✓" } else { "" };
        let name_w = inner_w.saturating_sub(5 + tag.chars().count() + 1);
        let name = crate::ui::truncate(&r.name, name_w);
        let pad = inner_w.saturating_sub(5 + name.chars().count() + tag.chars().count());
        lines.push(Line::from(vec![
            Span::styled(
                tick,
                sel(Style::default().fg(if r.on { th.ok } else { th.muted })),
            ),
            Span::styled(
                name,
                sel(Style::default().fg(if r.added { th.dim } else { th.text })),
            ),
            Span::styled(format!("{}{tag}", " ".repeat(pad)), sel(dim)),
        ]));
        let path = crate::ui::truncate(&tilde(&r.path, home.as_deref()), inner_w.saturating_sub(5));
        let pad = inner_w.saturating_sub(5 + path.chars().count());
        lines.push(Line::from(vec![
            Span::styled("     ", sel(dim)),
            Span::styled(format!("{path}{}", " ".repeat(pad)), sel(dim)),
        ]));
    }
    let list_h = lines.len() - list_at;
    let chosen = view.chosen().len();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        match chosen {
            0 => " Enter adds the highlighted repo.".to_string(),
            n => format!(" Adds {}.", crate::flash::plural(n, "project")),
        },
        Style::default().fg(th.muted),
    )));

    let hints = vec![
        keys::TICK.hint(),
        keys::ALL.hint(),
        if chosen == 0 {
            keys::ADD.hint()
        } else {
            keys::ADD.hint_as(format!("add {chosen}")).kept()
        },
        keys::BROWSE.hint(),
        crate::hints::ESC_CLOSE.hint(),
    ];
    let height = (lines.len() as u16 + 2).min(screen.height.saturating_sub(2));
    let area = crate::ui::centered_rect(screen, width, height);
    let title = match view.rows.iter().filter(|r| !r.added).count() {
        0 => " Open project ".to_string(),
        n => format!(" Open project · {} ", crate::flash::plural(n, "repo")),
    };
    let inner = crate::ui::render_modal_frame(f, area, title, &hints, th);
    f.render_widget(Paragraph::new(lines), inner);
    if let Some(Overlay::OpenRepo(v)) = &mut app.overlay {
        v.area = area;
        v.first = first;
        v.cursor = cursor;
        v.list_area = Rect {
            y: inner.y + list_at as u16,
            height: (list_h as u16).min(inner.height.saturating_sub(list_at as u16)),
            ..inner
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn repo(at: &Path) {
        std::fs::create_dir_all(at.join(".git")).unwrap();
    }

    #[test]
    fn every_key_parses() {
        for k in keys::EVERY {
            assert!(k.parses(), "{:?}", k.chords);
        }
    }

    #[test]
    fn scan_finds_repos_and_stops_at_each() {
        let t = tmp();
        let root = t.path();
        repo(&root.join("a"));
        repo(&root.join("group/b"));
        // Inside a repo: not walked.
        repo(&root.join("a/inner"));
        // Skipped folders.
        repo(&root.join("node_modules/c"));
        repo(&root.join(".hidden/d"));
        // A linked worktree: a .git file.
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: /x").unwrap();
        // Too deep.
        repo(&root.join("1/2/3/4/5/deep"));
        let mut found: Vec<PathBuf> = scan(&[root.to_path_buf()], SCAN_DEPTH)
            .into_iter()
            .map(|f| f.path)
            .collect();
        found.sort();
        assert_eq!(found, vec![root.join("a"), root.join("group/b")]);
    }

    #[test]
    fn a_root_listed_twice_is_walked_once() {
        let t = tmp();
        repo(&t.path().join("a"));
        let r = t.path().to_path_buf();
        assert_eq!(scan(&[r.clone(), r], 2).len(), 1);
    }

    #[test]
    fn nested_finds_the_repo_one_folder_down() {
        let t = tmp();
        repo(&t.path().join("proj"));
        assert_eq!(nested(t.path()), vec![t.path().join("proj")]);
    }

    fn opened(paths: &[&str]) -> App {
        let mut app = App::new();
        start(&mut app);
        let found = paths.iter().map(PathBuf::from).collect();
        let generation = app.open_repo.generation;
        // No channel in tests: land the answer by hand.
        if let Some(Overlay::OpenRepo(v)) = &mut app.overlay {
            v.scanning = true;
        }
        land(&mut app, Answer { generation, found });
        app
    }

    fn view(app: &App) -> &OpenRepoView {
        match &app.overlay {
            Some(Overlay::OpenRepo(v)) => v,
            other => panic!("{other:?}"),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn filter_narrows_and_enter_adds_the_highlighted() {
        let t = tmp();
        let beta = t.path().join("beta");
        repo(&beta);
        let mut app = opened(&["/x/alpha", &beta.to_string_lossy()]);
        let mut out = Vec::new();
        for c in "bet".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)), &mut out);
        }
        assert_eq!(view(&app).shown().len(), 1);
        handle_key(&mut app, key(KeyCode::Enter), &mut out);
        assert!(app.overlay.is_none());
        assert!(matches!(
            &out[..],
            [ClientRequest::AddProject { path, .. }] if path == &beta
        ));
    }

    #[test]
    fn space_ticks_several_and_enter_adds_them_all() {
        let mut app = opened(&["/x/a", "/x/b", "/x/c"]);
        let mut out = Vec::new();
        handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
        handle_key(&mut app, key(KeyCode::Down), &mut out);
        handle_key(&mut app, key(KeyCode::Down), &mut out);
        handle_key(&mut app, key(KeyCode::Char(' ')), &mut out);
        handle_key(&mut app, key(KeyCode::Enter), &mut out);
        let paths: Vec<_> = out
            .iter()
            .filter_map(|r| match r {
                ClientRequest::AddProject { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(paths, vec![PathBuf::from("/x/a"), PathBuf::from("/x/c")]);
    }

    #[test]
    fn a_slash_switches_to_browsing() {
        let mut app = opened(&["/x/a"]);
        let mut out = Vec::new();
        handle_key(&mut app, key(KeyCode::Char('/')), &mut out);
        let Some(Overlay::OpenRepo(_)) = &app.overlay else {
            let Some(Overlay::Prompt(p)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert_eq!(p.input.as_str(), "/");
            return;
        };
        panic!("still the picker");
    }

    #[test]
    fn the_top_row_clones_from_github() {
        let mut app = opened(&["/x/a"]);
        let mut out = Vec::new();
        handle_key(&mut app, key(KeyCode::Up), &mut out);
        handle_key(&mut app, key(KeyCode::Enter), &mut out);
        assert!(matches!(app.overlay, Some(Overlay::CloneRepo(_))), "{:?}", app.overlay);
        // Esc from its list comes back here.
        crate::clone_repo::handle_key(&mut app, key(KeyCode::Esc), &mut out);
        assert!(matches!(app.overlay, Some(Overlay::OpenRepo(_))));
    }

    #[test]
    fn a_stale_scan_is_dropped() {
        let mut app = opened(&[]);
        let stale = app.open_repo.generation;
        start(&mut app);
        land(
            &mut app,
            Answer {
                generation: stale,
                found: vec![PathBuf::from("/x/a")],
            },
        );
        assert!(view(&app).rows.is_empty());
    }
}
