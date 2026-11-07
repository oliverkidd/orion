//! CLONE FROM GITHUB: the OPEN REPO modal's first row. Two steps in one
//! modal:
//!
//! 1. **Pick** — the GitHub repositories the user can reach (`gh api
//!    user/repos`, theirs, their collaborations and their organisations',
//!    most recently pushed first), filtered as they type. A URL, an `ssh`
//!    address or `owner/name` typed in is taken as it stands.
//! 2. **Where** — the folder to clone into, prefilled as the folder most
//!    projects already sit in plus the repository's name. The repository's
//!    files go straight into that folder: `git clone <url> <folder>`, so
//!    the folder's name is whatever was typed.
//!
//! Enter clones off the loop (`gh repo clone` for a GitHub name, so the
//! user's `gh` protocol and sign-in apply; `git clone` for any other
//! address) and, once it lands, adds the folder as a project and opens it.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use orion_core::protocol::ClientRequest;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::theme::Theme;

/// A repository `gh` listed.
#[derive(Debug, Clone, PartialEq)]
pub struct GhRepo {
    /// `owner/name`.
    pub full_name: String,
    pub description: String,
    pub private: bool,
}

/// What to clone: a GitHub `owner/name`, or any other address git takes.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Github(String),
    Url(String),
}

impl Source {
    /// What was typed, read: a full URL or `git@…` address is a URL —
    /// unless it is a plain github.com one, which is its `owner/name` —
    /// and `owner/name` is GitHub's. Anything else is not an address.
    pub fn parse(text: &str) -> Option<Source> {
        let t = text.trim();
        if t.is_empty() || t.contains(char::is_whitespace) {
            return None;
        }
        for prefix in ["https://github.com/", "http://github.com/", "github.com/"] {
            if let Some(rest) = t.strip_prefix(prefix) {
                let rest = rest.trim_end_matches('/').trim_end_matches(".git");
                let mut parts = rest.split('/');
                if let (Some(o), Some(n)) = (parts.next(), parts.next()) {
                    if !o.is_empty() && !n.is_empty() {
                        return Some(Source::Github(format!("{o}/{n}")));
                    }
                }
                return None;
            }
        }
        if t.contains("://") || t.starts_with("git@") {
            return Some(Source::Url(t.to_string()));
        }
        let mut parts = t.split('/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(o), Some(n), None) if !o.is_empty() && !n.is_empty() && !o.starts_with('~') => {
                Some(Source::Github(t.to_string()))
            }
            _ => None,
        }
    }

    /// The folder name a clone of it gets by default.
    pub fn name(&self) -> String {
        let s = match self {
            Source::Github(s) | Source::Url(s) => s,
        };
        let last = s
            .trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .unwrap_or(s);
        last.trim_end_matches(".git").to_string()
    }

    fn label(&self) -> &str {
        match self {
            Source::Github(s) | Source::Url(s) => s,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Pick,
    Where(Source),
}

#[derive(Debug, Clone)]
pub struct CloneView {
    pub stage: Stage,
    /// `None` while `gh` is listing.
    pub repos: Option<Vec<GhRepo>>,
    /// Why the list is empty, when `gh` said.
    pub error: Option<String>,
    /// Pick: the filter, or an address. Where: the folder.
    pub input: String,
    pub cursor: usize,
    pub first: usize,
    pub area: Rect,
}

impl CloneView {
    pub fn shown(&self) -> Vec<&GhRepo> {
        let needle = self.input.to_lowercase();
        self.repos
            .iter()
            .flatten()
            .filter(|r| {
                needle.is_empty()
                    || r.full_name.to_lowercase().contains(&needle)
                    || r.description.to_lowercase().contains(&needle)
            })
            .collect()
    }
}

/// What lands on `App::clone_repo.tx`.
#[derive(Debug)]
pub enum Answer {
    Listed(Result<Vec<GhRepo>, String>),
    Cloned {
        dest: PathBuf,
        result: Result<(), String>,
    },
}

#[derive(Default)]
pub struct Shared {
    /// `None` in the unit tests: nothing runs `gh` or `git` there.
    pub tx: Option<tokio::sync::mpsc::UnboundedSender<Answer>>,
}

pub(crate) mod keys {
    use crate::hints::Key;
    pub const NEXT: Key = Key::new(&["enter"], "next");
    pub const CLONE: Key = Key::new(&["enter"], "clone");
    pub const BACK: Key = Key::new(&["esc"], "back");
}

/// The folder most projects sit in — where a new clone likely belongs —
/// else `~/Code` when there is one, else the home folder.
pub fn suggested_parent(app: &App, home: Option<&Path>) -> PathBuf {
    let mut counts: std::collections::HashMap<&Path, usize> = Default::default();
    for p in &app.tree.projects {
        if let Some(parent) = p.repo_path.parent() {
            *counts.entry(parent).or_default() += 1;
        }
    }
    if let Some((best, _)) = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
    {
        return best.to_path_buf();
    }
    let home = home.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
    ["Code", "Developer", "Projects", "dev", "src"]
        .iter()
        .map(|d| home.join(d))
        .find(|d| d.is_dir())
        .unwrap_or(home)
}

fn tilde(path: &Path) -> String {
    match orion_core::env::home_dir().and_then(|h| path.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The OPEN REPO modal's first row: open the picker and list.
pub(crate) fn start(app: &mut App) {
    app.dirty = true;
    app.overlay = Some(Overlay::CloneRepo(CloneView {
        stage: Stage::Pick,
        repos: app.clone_repo.tx.is_none().then(Vec::new),
        error: None,
        input: String::new(),
        cursor: 0,
        first: 0,
        area: Rect::default(),
    }));
    let Some(tx) = app.clone_repo.tx.clone() else {
        return;
    };
    tokio::task::spawn_blocking(move || {
        let _ = tx.send(Answer::Listed(list()));
    });
}

/// `gh api user/repos`: every repository the signed-in user can reach.
fn list() -> Result<Vec<GhRepo>, String> {
    let out = std::process::Command::new("gh")
        .args([
            "api",
            "--paginate",
            "user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member",
            "--jq",
            r#".[] | [.full_name, (.private|tostring), (.description // "" | gsub("[\t\n]"; " "))] | @tsv"#,
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| "gh isn't installed — paste a URL instead".to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("gh failed");
        return Err(format!("{} — paste a URL instead", line.trim()));
    }
    Ok(parse_list(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_list(text: &str) -> Vec<GhRepo> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some(GhRepo {
                full_name: f.next()?.to_string(),
                private: f.next() == Some("true"),
                description: f.next().unwrap_or_default().to_string(),
            })
        })
        .filter(|r| !r.full_name.is_empty())
        .collect()
}

/// `source` cloned into `dest` — the repository's files straight in it.
fn clone(source: &Source, dest: &Path) -> Result<(), String> {
    if dest.exists()
        && std::fs::read_dir(dest).map_or(true, |mut d| d.next().is_some())
    {
        return Err(format!("{} already exists and isn't empty", tilde(dest)));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let dest_s = dest.to_string_lossy().to_string();
    let mut cmd = match source {
        Source::Github(name) => {
            let mut c = std::process::Command::new("gh");
            c.args(["repo", "clone", name, &dest_s]);
            c
        }
        Source::Url(url) => {
            let mut c = std::process::Command::new("git");
            c.args(["clone", url, &dest_s]);
            c
        }
    };
    {
        use std::os::unix::process::CommandExt;
        cmd.stdin(std::process::Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0");
        // SAFETY: setsid is async-signal-safe and touches nothing but the
        // child — no ssh prompt may find the TUI's terminal.
        unsafe {
            cmd.pre_exec(crate::ipc::own_session);
        }
    }
    let out = cmd.output().map_err(|e| format!("couldn't run the clone: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("clone failed")
            .trim()
            .trim_start_matches("fatal: ")
            .to_string())
    }
}

/// An answer landed: the list fills the picker; a clone adds its project.
pub(crate) fn land(app: &mut App, answer: Answer, out: &mut Vec<ClientRequest>) {
    app.dirty = true;
    match answer {
        Answer::Listed(result) => {
            let Some(Overlay::CloneRepo(view)) = &mut app.overlay else {
                return;
            };
            match result {
                Ok(repos) => view.repos = Some(repos),
                Err(why) => {
                    view.repos = Some(Vec::new());
                    view.error = Some(why);
                }
            }
        }
        Answer::Cloned { dest, result } => match result {
            Ok(()) => {
                app.flash = Some(Flash::done(format!("cloned into {}", tilde(&dest))));
                crate::event_loop::open_folder(app, dest, out);
            }
            Err(why) => app.flash = Some(Flash::failed(format!("clone failed: {why}"))),
        },
    }
}

/// Pick → Where, for `source`.
fn choose(app: &mut App, source: Source) {
    let parent = suggested_parent(app, orion_core::env::home_dir().as_deref());
    let Some(Overlay::CloneRepo(view)) = &mut app.overlay else {
        return;
    };
    view.input = format!("{}/{}", tilde(&parent), source.name());
    view.stage = Stage::Where(source);
}

/// Where's Enter: clone, off the loop.
fn go(app: &mut App, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::CloneRepo(view)) = &app.overlay else {
        return;
    };
    let Stage::Where(source) = view.stage.clone() else {
        return;
    };
    let typed = view.input.trim();
    if typed.is_empty() {
        return;
    }
    let dest = crate::event_loop::expand_home(typed);
    app.overlay = None;
    app.flash = Some(Flash::working(format!(
        "cloning {} into {}…",
        source.label(),
        tilde(&dest)
    )));
    match app.clone_repo.tx.clone() {
        Some(tx) => {
            tokio::task::spawn_blocking(move || {
                let result = clone(&source, &dest);
                let _ = tx.send(Answer::Cloned { dest, result });
            });
        }
        // The unit tests: nothing is cloned, the folder is added as given.
        None => land(app, Answer::Cloned { dest, result: Ok(()) }, out),
    }
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::CloneRepo(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match (&view.stage, key.code) {
        (Stage::Pick, KeyCode::Esc) => crate::open_repo::start(app),
        (Stage::Where(_), KeyCode::Esc) => {
            view.stage = Stage::Pick;
            view.input.clear();
        }
        (Stage::Pick, KeyCode::Down) => {
            view.cursor = crate::app::clamp_selection(view.cursor as i64 + 1, view.shown().len());
        }
        (Stage::Pick, KeyCode::Up) => {
            view.cursor = crate::app::clamp_selection(view.cursor as i64 - 1, view.shown().len());
        }
        (Stage::Pick, KeyCode::Enter) => {
            // A typed address wins over a filtered row.
            let source = Source::parse(&view.input).or_else(|| {
                view.shown()
                    .get(view.cursor)
                    .map(|r| Source::Github(r.full_name.clone()))
            });
            if let Some(source) = source {
                choose(app, source);
            }
        }
        (Stage::Where(_), KeyCode::Enter) => go(app, out),
        (_, KeyCode::Char('u')) if ctrl => {
            view.input.clear();
            view.cursor = 0;
        }
        (_, KeyCode::Backspace) => {
            view.input.pop();
            view.cursor = 0;
        }
        (_, KeyCode::Char(c)) if !ctrl && !key.modifiers.contains(KeyModifiers::SUPER) => {
            view.input.push(c);
            view.cursor = 0;
        }
        _ => {}
    }
}

const WIDEST: u16 = 84;

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &CloneView, th: Theme) {
    let screen = f.area();
    let width = screen.width.saturating_sub(4).min(WIDEST);
    let inner_w = usize::from(width.saturating_sub(2));
    let dim = Style::default().fg(th.dim);
    let text = Style::default().fg(th.text);
    let mut lines: Vec<Line> = Vec::new();
    let mut first = view.first;
    let mut cursor = view.cursor;
    let (title, hints) = match &view.stage {
        Stage::Pick => {
            lines.push(Line::from(vec![
                Span::styled(" ⌕ ", Style::default().fg(th.muted)),
                if view.input.is_empty() {
                    Span::styled("filter your repos, or paste a URL / git@… / owner/name", dim)
                } else {
                    Span::styled(view.input.clone(), text)
                },
            ]));
            lines.push(Line::from(""));
            let shown = view.shown();
            if let Some(src) = Source::parse(&view.input) {
                lines.push(Line::from(vec![
                    Span::styled(" Enter clones ", dim),
                    Span::styled(src.label().to_string(), text.add_modifier(Modifier::BOLD)),
                ]));
            } else if view.repos.is_none() {
                lines.push(Line::from(Span::styled(" Asking GitHub for your repositories…", dim)));
            } else if let Some(why) = &view.error {
                lines.push(Line::from(Span::styled(format!(" {why}"), Style::default().fg(th.warn))));
            } else if shown.is_empty() {
                lines.push(Line::from(Span::styled(" Nothing matches.", dim)));
            }
            let fits = usize::from(screen.height.saturating_sub(8).max(2)) / 2;
            cursor = cursor.min(shown.len().saturating_sub(1));
            first = first.min(cursor).max((cursor + 1).saturating_sub(fits));
            if Source::parse(&view.input).is_none() {
                for (n, r) in shown.iter().enumerate().skip(first).take(fits) {
                    let sel = |s: Style| {
                        if n == cursor {
                            s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
                        } else {
                            s
                        }
                    };
                    let tag = if r.private { "private" } else { "" };
                    let name = crate::ui::truncate(&r.full_name, inner_w.saturating_sub(3 + tag.len() + 1));
                    let pad = inner_w.saturating_sub(1 + name.chars().count() + tag.len());
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {name}"), sel(text)),
                        Span::styled(format!("{}{tag}", " ".repeat(pad)), sel(dim)),
                    ]));
                    let desc = crate::ui::truncate(&r.description, inner_w.saturating_sub(3));
                    let pad = inner_w.saturating_sub(3 + desc.chars().count());
                    lines.push(Line::from(Span::styled(
                        format!("   {desc}{}", " ".repeat(pad)),
                        sel(dim),
                    )));
                }
            }
            (
                " Clone from GitHub ".to_string(),
                vec![keys::NEXT.hint(), keys::BACK.hint()],
            )
        }
        Stage::Where(source) => {
            lines.push(Line::from(vec![
                Span::styled(" Cloning ", dim),
                Span::styled(source.label().to_string(), text.add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(" Into this folder (its files go straight in):", dim)));
            lines.push(Line::from(vec![
                Span::styled(" › ", Style::default().fg(th.muted)),
                Span::styled(view.input.clone(), text),
                Span::styled("▏", Style::default().fg(th.muted)),
            ]));
            (
                " Clone from GitHub · where ".to_string(),
                vec![keys::CLONE.hint().kept(), keys::BACK.hint()],
            )
        }
    };
    let height = (lines.len() as u16 + 2).min(screen.height.saturating_sub(2));
    let area = crate::ui::centered_rect(screen, width, height);
    let inner = crate::ui::render_modal_frame(f, area, title, &hints, th);
    f.render_widget(Paragraph::new(lines), inner);
    if let Some(Overlay::CloneRepo(v)) = &mut app.overlay {
        v.area = area;
        v.first = first;
        v.cursor = cursor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse() {
        let gh = |s: &str| Some(Source::Github(s.into()));
        assert_eq!(Source::parse("acme/widget"), gh("acme/widget"));
        assert_eq!(Source::parse("https://github.com/acme/widget.git"), gh("acme/widget"));
        assert_eq!(Source::parse("https://github.com/acme/widget/tree/main"), gh("acme/widget"));
        assert_eq!(
            Source::parse("git@github.com:acme/widget.git"),
            Some(Source::Url("git@github.com:acme/widget.git".into()))
        );
        assert_eq!(
            Source::parse("https://gitlab.com/a/b"),
            Some(Source::Url("https://gitlab.com/a/b".into()))
        );
        assert_eq!(Source::parse("widget"), None);
        assert_eq!(Source::parse("two words/x"), None);
        assert_eq!(Source::parse("~/Code"), None);
    }

    #[test]
    fn names_come_from_the_last_segment() {
        assert_eq!(Source::Github("acme/widget".into()).name(), "widget");
        assert_eq!(Source::Url("git@github.com:acme/w.git".into()).name(), "w");
        assert_eq!(Source::Url("https://x.org/a/b.git/".into()).name(), "b");
    }

    #[test]
    fn gh_tsv_parses() {
        let r = parse_list("acme/a\ttrue\tthe a\nacme/b\tfalse\t\n");
        assert_eq!(r.len(), 2);
        assert!(r[0].private && !r[1].private);
        assert_eq!(r[0].description, "the a");
    }

    #[test]
    fn the_parent_most_projects_share_is_suggested() {
        let mut app = App::new();
        for (i, p) in ["/w/a", "/w/b", "/z/c"].iter().enumerate() {
            app.tree.projects.push(orion_core::Project {
                id: orion_core::ProjectId(format!("p{i}")),
                name: format!("p{i}"),
                repo_path: PathBuf::from(p),
                sort_order: 0,
            });
        }
        assert_eq!(suggested_parent(&app, None), PathBuf::from("/w"));
    }

    #[test]
    fn a_pasted_url_goes_to_where_then_enter_adds_the_folder() {
        let mut app = App::new();
        start(&mut app);
        let mut out = Vec::new();
        let k = |c| KeyEvent::new(c, KeyModifiers::NONE);
        for c in "acme/widget".chars() {
            handle_key(&mut app, k(KeyCode::Char(c)), &mut out);
        }
        handle_key(&mut app, k(KeyCode::Enter), &mut out);
        let Some(Overlay::CloneRepo(v)) = &mut app.overlay else {
            panic!("{:?}", app.overlay);
        };
        assert_eq!(v.stage, Stage::Where(Source::Github("acme/widget".into())));
        assert!(v.input.ends_with("/widget"), "{}", v.input);
        // Somewhere that doesn't exist: the add asks to create it.
        v.input = "/nonexistent/orion-test/widget".into();
        handle_key(&mut app, k(KeyCode::Enter), &mut out);
        assert!(matches!(app.overlay, Some(Overlay::Confirm(_))), "{:?}", app.overlay);
    }
}
