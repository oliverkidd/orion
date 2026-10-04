//! FILE MENTIONS: `@` in a box whose text goes to an agent on this machine
//! — the QUICK PROMPT, an AGENT PRESET's task, the FOLLOW-UP MODAL — lists
//! the files of the checkout the agent runs in, narrowed fzf-style by what
//! is typed after the `@`, and Tab or Enter writes the picked path in as
//! `@path/to/file`, the form Claude Code, Cursor and Codex all read as a
//! reference to that file.
//!
//! The list is `git ls-files` (tracked and untracked, gitignore kept),
//! read off the loop the first time a box sees an `@` (`view_jobs`) and
//! kept for that box: a box reopened from one of its pickers is a fresh
//! box, and asks again. Esc puts the list away for that `@` alone; the
//! next one typed lists again.

use crate::app::{App, Overlay, PromptDialog, PromptKind};
use crate::text_input::{TextInput, TextView};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::path::PathBuf;
use std::sync::Arc;

/// Most rows the list shows at once.
const LIST_ROWS: usize = 8;
/// Most matches kept per keystroke: more than a list scrolled with ↓ ever
/// reaches, and few enough that ranking ten thousand paths stays cheap to
/// hold.
const MAX_HITS: usize = 200;
/// Widest the list is drawn.
const LIST_W: u16 = 64;

/// The checkout's files, as far as the box has got with them.
#[derive(Debug, Clone, Default)]
enum Files {
    /// No `@` typed yet: nothing read.
    #[default]
    NotAsked,
    /// `git ls-files` is running; its answer carries this ticket.
    Loading(u64),
    Ready(Arc<Vec<String>>),
    /// The read failed (not a repo, the checkout gone): the reason, shown
    /// in place of the list.
    Failed(String),
}

/// A box's FILE MENTION state. Lives on the [`PromptDialog`], and is
/// cloned with it every frame — the file list behind an `Arc`.
#[derive(Debug, Clone, Default)]
pub struct Mention {
    /// The checkout whose files are offered, resolved at the first `@`.
    root: Option<PathBuf>,
    files: Files,
    /// The `@` under the caret: its byte offset and what follows it.
    token: Option<(usize, String)>,
    /// The `@` Esc put the list away for.
    dismissed: Option<usize>,
    /// Ranked matches: an index into the files, and its matched chars.
    hits: Vec<(usize, Vec<usize>)>,
    hover: usize,
}

impl Mention {
    /// Is the list up — an `@` under the caret that Esc has not put away?
    pub fn is_open(&self) -> bool {
        matches!(&self.token, Some((at, _)) if self.dismissed != Some(*at))
            && !matches!(self.files, Files::NotAsked)
    }

    /// Re-aim at `token` after an edit: a new `@` (or none) forgets an
    /// Esc, and a new query ranks again from the top.
    fn set_token(&mut self, token: Option<(usize, String)>) {
        if token.as_ref().map(|t| t.0) != self.dismissed {
            self.dismissed = None;
        }
        if token != self.token {
            self.token = token;
            self.hover = 0;
            self.rank();
        }
    }

    fn rank(&mut self) {
        self.hits.clear();
        let (Some((_, query)), Files::Ready(files)) = (&self.token, &self.files) else {
            return;
        };
        self.hits = crate::fuzzy::rank(query, files.iter().map(String::as_str))
            .into_iter()
            .take(MAX_HITS)
            .collect();
    }

    fn path(&self, hit: usize) -> Option<&str> {
        let Files::Ready(files) = &self.files else {
            return None;
        };
        let (index, _) = self.hits.get(hit)?;
        files.get(*index).map(String::as_str)
    }

    /// `git ls-files` answered for `ticket`; true when it was this box's.
    pub fn land(&mut self, ticket: u64, result: Result<Vec<String>, String>) -> bool {
        if !matches!(self.files, Files::Loading(t) if t == ticket) {
            return false;
        }
        self.files = match result {
            Ok(files) => Files::Ready(Arc::new(files)),
            Err(msg) => Files::Failed(msg),
        };
        self.rank();
        true
    }
}

/// Does `kind`'s text reach an agent that can open a file in its checkout?
fn takes_mentions(kind: &PromptKind) -> bool {
    match kind {
        // A cloud box's task runs in the sandbox, not this checkout.
        PromptKind::QuickPrompt(launch) => !launch.cloud,
        other => other.reaches_local_agent(),
    }
}

/// The checkout an `@` in `kind` lists: where the launch or the session
/// runs. A fresh worktree does not exist yet, so its project's root
/// checkout — the base it is cut from — stands in for it.
fn root_of(app: &App, kind: &PromptKind) -> Option<PathBuf> {
    let worktree_path = |id: &orion_core::WorktreeId| {
        app.tree
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .map(|w| w.path.clone())
    };
    match kind {
        PromptKind::QuickPrompt(launch) => match &launch.target {
            crate::quick_prompt::QuickTarget::Worktree(id) => worktree_path(id),
            crate::quick_prompt::QuickTarget::NewWorktree { project, .. } => app
                .tree
                .projects
                .iter()
                .find(|p| &p.id == project)
                .map(|p| p.repo_path.clone()),
        },
        PromptKind::AgentPresetTask { worktree, .. } => worktree_path(worktree),
        PromptKind::FollowUp { id } => app
            .tree
            .agents
            .iter()
            .find(|a| &a.id == id)
            .and_then(|a| worktree_path(&a.worktree_id)),
        _ => None,
    }
}

/// After any key or paste in a box: aim the list at the `@` under the
/// caret, reading the checkout's files the first time one is there.
pub fn sync(app: &mut App) {
    let Some(Overlay::Prompt(prompt)) = &app.overlay else {
        return;
    };
    if !takes_mentions(&prompt.kind) {
        return;
    }
    let token = token_at_caret(&prompt.input);
    let root = match (&token, &prompt.mention.root) {
        (Some(_), None) => root_of(app, &prompt.kind),
        _ => None,
    };
    let jobs = app.view_jobs.clone();
    let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
        return;
    };
    let mention = &mut prompt.mention;
    if root.is_some() {
        mention.root = root;
    }
    if token.is_some() && matches!(mention.files, Files::NotAsked) {
        if let Some(root) = mention.root.clone() {
            mention.files = match jobs {
                Some(jobs) => {
                    let ticket = crate::view_jobs::ticket();
                    jobs.run(move || {
                        Some(crate::view_jobs::Answer::Files {
                            ticket,
                            result: crate::git_diff::list_files(&root),
                        })
                    });
                    Files::Loading(ticket)
                }
                // No pool (unit tests): read inline.
                None => match crate::git_diff::list_files(&root) {
                    Ok(files) => Files::Ready(Arc::new(files)),
                    Err(msg) => Files::Failed(msg),
                },
            };
        }
    }
    mention.set_token(token);
}

/// `git ls-files` answered: hand it to the box that asked, if it is
/// still up. True when it was the box's.
pub fn land(app: &mut App, ticket: u64, result: &Result<Vec<String>, String>) -> bool {
    match &mut app.overlay {
        Some(Overlay::Prompt(prompt)) => prompt.mention.land(ticket, result.clone()),
        _ => false,
    }
}

/// The `@word` the caret sits at the end of — an `@` at the start of the
/// text or after whitespace, then no whitespace up to the caret — as the
/// `@`'s byte offset and the text after it.
fn token_at_caret(input: &TextInput) -> Option<(usize, String)> {
    let text = input.as_str();
    let caret = input.caret_byte();
    let before = &text[..caret];
    let at = before.rfind(|c: char| c.is_whitespace()).map_or(0, |i| {
        i + before[i..].chars().next().map_or(1, char::len_utf8)
    });
    let word = &before[at..];
    let query = word.strip_prefix('@')?;
    Some((at, query.to_string()))
}

/// The list's keys while it is up: ↑/↓ move, Tab or Enter writes the
/// path in, Esc puts the list away. True when the key was the list's;
/// every other key goes on to the box — Enter too while nothing matches,
/// so a stray `@` never holds up a send.
pub fn handle_key(app: &mut App, key: &KeyEvent) -> bool {
    let Some(Overlay::Prompt(prompt)) = &mut app.overlay else {
        return false;
    };
    if !prompt.mention.is_open() {
        return false;
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
    {
        return false;
    }
    let mention = &mut prompt.mention;
    let count = mention.hits.len();
    match key.code {
        KeyCode::Down if count > 0 => {
            mention.hover = (mention.hover + 1) % count;
            true
        }
        KeyCode::Up if count > 0 => {
            mention.hover = (mention.hover + count - 1) % count;
            true
        }
        KeyCode::Esc => {
            mention.dismissed = mention.token.as_ref().map(|t| t.0);
            true
        }
        // Tab is the list's even with nothing to write in: in the box it
        // would open the harness picker over the half-typed mention.
        KeyCode::Tab => {
            accept(prompt);
            true
        }
        KeyCode::Enter if !prompt.input.takes_newline(key) => accept(prompt),
        _ => false,
    }
}

/// Write the hovered match in over the `@word`, and a space after it so
/// the next word starts clear. False with nothing to write.
fn accept(prompt: &mut PromptDialog) -> bool {
    let mention = &prompt.mention;
    let Some((at, _)) = mention.token.clone() else {
        return false;
    };
    let Some(path) = mention.path(mention.hover).map(str::to_string) else {
        return false;
    };
    prompt.input.replace_to_caret(at, &format!("@{path} "));
    prompt.mention.token = None;
    prompt.mention.hits.clear();
    true
}

/// The first match the list shows: it scrolls to keep the hover in view.
fn first_shown(mention: &Mention) -> usize {
    mention.hover.saturating_sub(LIST_ROWS - 1)
}

/// Draw the list under the caret's row of `input`, drawn in `editor` with
/// `view` — over it when the screen has no room below.
pub fn draw(f: &mut Frame, prompt: &PromptDialog, editor: Rect, view: TextView, th: Theme) {
    let mention = &prompt.mention;
    if !mention.is_open() {
        return;
    }
    let input = &prompt.input;
    let rows = input.rows(view.width.into());
    let caret_row = input.caret_row(&rows);
    let shown_row = caret_row.saturating_sub(view.top.into()) as u16;
    let caret_y = editor.y + shown_row.min(editor.height.saturating_sub(1));
    // Hung from the `@` itself, so the list sits under the word it
    // completes; the caret's column when the word wrapped onto this row.
    let at_char = mention
        .token
        .as_ref()
        .map_or(input.cursor_chars(), |(at, _)| input.as_str()[..*at].chars().count());
    let col = rows.get(caret_row).map_or(0, |(start, _)| {
        let from = if at_char >= *start { at_char } else { input.cursor_chars() };
        from.saturating_sub(*start)
    }) as u16;

    let lines: Vec<Line> = match &mention.files {
        Files::NotAsked => return,
        Files::Loading(_) => vec![Line::from(Span::styled(
            "listing files…",
            Style::default().fg(th.dim),
        ))],
        Files::Failed(msg) => vec![Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(th.err),
        ))],
        Files::Ready(_) if mention.hits.is_empty() => vec![Line::from(Span::styled(
            "no file matches",
            Style::default().fg(th.dim),
        ))],
        Files::Ready(_) => {
            let first = first_shown(mention);
            (first..mention.hits.len().min(first + LIST_ROWS))
                .filter_map(|i| {
                    let path = mention.path(i)?;
                    let marks = &mention.hits[i].1;
                    let hovered = i == mention.hover;
                    let base = if hovered {
                        Style::default().fg(th.text).bg(th.sel_bg)
                    } else {
                        Style::default().fg(th.text)
                    };
                    let hit = base.fg(th.accent).add_modifier(Modifier::BOLD);
                    let spans: Vec<Span> = std::iter::once(Span::styled(" ", base))
                        .chain(path.chars().enumerate().map(|(n, c)| {
                            Span::styled(c.to_string(), if marks.contains(&n) { hit } else { base })
                        }))
                        .collect();
                    Some(Line::from(spans))
                })
                .collect()
        }
    };

    let frame = f.area();
    let width = LIST_W.min(frame.width);
    let height = (lines.len() as u16 + 2).min(frame.height);
    let x = (editor.x + col).min(frame.right().saturating_sub(width));
    let below = caret_y + 1;
    let y = if below + height <= frame.bottom() {
        below
    } else {
        caret_y.saturating_sub(height)
    };
    let area = Rect::new(x, y, width, height);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .title(Span::styled(" files ", Style::default().fg(th.accent)))
        .title_bottom(Line::from(Span::styled(
            " Tab insert · Esc close ",
            Style::default().fg(th.dim),
        )));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_end(text: &str) -> Option<(usize, String)> {
        token_at_caret(&TextInput::multiline_with_text(text))
    }

    #[test]
    fn the_token_is_an_at_word_ending_at_the_caret() {
        assert_eq!(at_end("@"), Some((0, String::new())));
        assert_eq!(at_end("look at @src/ma"), Some((8, "src/ma".into())));
        assert_eq!(at_end("line one\n@foo"), Some((9, "foo".into())));
        assert_eq!(at_end("mail me@example"), None, "mid-word is no mention");
        assert_eq!(at_end("@foo bar"), None, "the word ended");
        assert_eq!(at_end("plain"), None);
    }

    #[test]
    fn a_pick_replaces_the_word_and_esc_lasts_for_that_at_alone() {
        let mut prompt = PromptDialog::new(
            "t",
            "l",
            "",
            PromptKind::FollowUp {
                id: orion_core::AgentId::generate(),
            },
        );
        prompt.input.set_text("see @ma");
        prompt.mention.files =
            Files::Ready(Arc::new(vec!["README.md".into(), "src/main.rs".into()]));
        prompt.mention.set_token(token_at_caret(&prompt.input));
        assert!(prompt.mention.is_open());
        assert_eq!(prompt.mention.path(0), Some("src/main.rs"));
        assert!(accept(&mut prompt));
        assert_eq!(prompt.input.as_str(), "see @src/main.rs ");

        prompt.input.set_text("and @");
        prompt.mention.set_token(token_at_caret(&prompt.input));
        prompt.mention.dismissed = Some(4);
        assert!(!prompt.mention.is_open());
        prompt.input.set_text("and @x @");
        prompt.mention.set_token(token_at_caret(&prompt.input));
        assert!(prompt.mention.is_open(), "a new @ lists again");
    }
}
