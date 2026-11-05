//! MARKDOWN PAGE: a `.md` file opened from anywhere — Go to file (`⌘P`),
//! the TREE BROWSER, find in files (`⌘⇧F`), ⌥click — reads as its rendered
//! page, full width and wrapped, in a modal over everything else. `Enter`
//! (or `e`) edits it in the BUILT-IN EDITOR, which comes up over the page
//! and hands it back, re-read, when it quits; `⌘O` opens it in the OPEN IN
//! APP editor. A drag over the text selects it and copies it on release,
//! a double-click takes a word, and `⌘C` copies the selection when there
//! is one (`doc_select`).
//!
//! The page is read again when the file changes on disk. Its links are
//! live: a click on one, or `Tab` to it and `Enter`, follows it — another
//! markdown file in this page, `Esc` coming back; a `#heading` down the
//! page; an `http` link in the browser; any other file in the editor.
//! `Enter` with no link picked edits at the source line of the row at the
//! top of the page.
//!
//! Through 0.42 a markdown file opened as the MARKDOWN SPLIT instead: the
//! editor on the left and the page on the right, both at half width. A
//! document wants reading first, and the page is a key away from the
//! editor either way.

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::doc_select::DocSelect;
use crate::markdown::Rendered;

/// Rows a wheel notch scrolls the page — the FILE TABS' pace.
pub const WHEEL_LINES: i32 = 3;

/// Rows kept above a find-in-files hit the page opens on, for context.
const SEEK_CONTEXT: usize = 2;

#[derive(Debug, Clone)]
pub struct MarkdownPage {
    /// The checkout the file is in: the editor's working directory.
    pub root: PathBuf,
    /// The file as the editor takes it — relative to `root`, or absolute.
    pub file: String,
    /// The line the page was opened on — a find-in-files hit's, a followed
    /// `#heading`'s, else 1 — which Enter edits at while the page stays
    /// where that put it ([`MarkdownPage::edit_line`]).
    pub line: u64,
    /// The BUILT-IN EDITOR command Enter runs, captured at open.
    pub editor: String,
    /// `root` joined with `file`.
    path: PathBuf,
    /// The file's modification time when last read; a different one
    /// re-reads it.
    modified: Option<SystemTime>,
    /// Its text when last read (empty when it can't be read).
    pub text: String,
    /// The page flowed for the last drawn width, written back during draw.
    pub rendered: Option<Rendered>,
    /// Top visible page row.
    pub scroll: u16,
    /// The text's rect from the last draw, for paging and the wheel.
    pub area: Rect,
    /// The modal's whole rect from the last draw: a click outside it
    /// closes the page.
    pub frame: Rect,
    /// The source line to bring into view once the page is first flowed.
    seek: Option<u64>,
    /// The scroll that seek settled on: while the page stays there, Enter
    /// edits at `line` — the hit's — rather than at the top row's.
    seeked: Option<u16>,
    /// The DOC SELECTION a drag over the text makes.
    pub select: DocSelect,
    /// The link `Tab` is on, by its place among the page's
    /// (`markdown::LinkSpot::link`): Enter follows it.
    pub focus: Option<usize>,
    /// The pages a followed link left, the last one newest, each with its
    /// scroll: Esc goes back to it.
    back: Vec<(String, u16)>,
}

/// What a key on the page asks of the event loop, beyond the scrolling —
/// and the following of a link to another page — the page does itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageKey {
    /// Enter / `e`: the file in the BUILT-IN EDITOR.
    Edit,
    /// A link that leads off the page ([`Away`]).
    Follow(Away),
    /// `⌘O` / `^o`: the file in the OPEN IN APP editor.
    Outside,
    /// `⌘C` / `^y`: the file's path to the clipboard.
    CopyPath,
    /// Esc / `q`: the page closes.
    Close,
    /// Scrolled, or nothing to do.
    Done,
}

/// Where a followed link goes when it isn't another page of the checkout's
/// markdown, or a heading on this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Away {
    /// An address with a scheme — `https:`, `mailto:` — for the browser.
    Web(String),
    /// A file that isn't markdown, for the editor, at its `#L42` line.
    File { file: String, line: u64 },
    /// A link to nothing there: the address as written.
    Missing(String),
}

/// Where a link's address points, worked out against the page's file.
enum Target {
    Away(Away),
    /// A heading on this page, by its anchor.
    Anchor(String),
    /// A markdown file — relative to the checkout, or absolute — and the
    /// heading on it, if the address named one.
    Page(String, Option<String>),
}

impl MarkdownPage {
    pub fn open(root: PathBuf, file: String, line: u64, editor: String) -> Self {
        let path = root.join(&file);
        let mut page = Self {
            root,
            file,
            line,
            editor,
            path,
            modified: None,
            text: String::new(),
            rendered: None,
            scroll: 0,
            area: Rect::default(),
            frame: Rect::default(),
            seek: (line > 1).then_some(line),
            seeked: None,
            select: DocSelect::default(),
            focus: None,
            back: Vec::new(),
        };
        page.reload();
        page
    }

    fn modified_now(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
    }

    fn reload(&mut self) {
        self.modified = self.modified_now();
        self.text = std::fs::read_to_string(&self.path).unwrap_or_default();
        self.rendered = None;
        self.select.clear();
        self.focus = None;
        self.seeked = None;
    }

    /// Re-read the file when it changed on disk since the last read — what
    /// the editor closing over the page does, and the event loop's slow
    /// beat asks every couple of seconds. True when it had.
    pub fn refresh(&mut self) -> bool {
        if self.modified_now() == self.modified {
            return false;
        }
        self.reload();
        true
    }

    fn max_scroll(&self) -> u16 {
        let lines = self.rendered.as_ref().map_or(0, |r| r.lines.len());
        lines
            .saturating_sub(self.area.height as usize)
            .min(u16::MAX as usize) as u16
    }

    /// Scroll by `delta` rows, kept within the last flow.
    pub fn scroll_by(&mut self, delta: i32) {
        let max = i32::from(self.max_scroll());
        self.scroll = (i32::from(self.scroll) + delta).clamp(0, max) as u16;
    }

    /// Re-clamp the scroll against a fresh flow, and — the first time the
    /// page is flowed — bring the line it was opened on into view.
    pub fn settle(&mut self) {
        if let (Some(line), Some(rendered)) = (self.seek, &self.rendered) {
            self.seek = None;
            self.scroll =
                scroll_to_row(seek_row(&self.text, line, rendered)).min(self.max_scroll());
            self.seeked = Some(self.scroll);
        }
        self.scroll = self.scroll.min(self.max_scroll());
        self.let_hidden_focus_go();
    }

    /// The line Enter (and `⌘O`) opens the file at: the one the page was
    /// opened on while it is still where that put it, else the source line
    /// of the row at the top.
    pub fn edit_line(&self) -> u64 {
        match &self.rendered {
            Some(doc) if self.seeked != Some(self.scroll) => {
                source_line(&self.text, self.scroll as usize, doc)
            }
            _ => self.line,
        }
    }

    /// A link scrolled off the page — by a key, the wheel or a reflow — is
    /// no longer the one Enter follows.
    fn let_hidden_focus_go(&mut self) {
        let (top, height) = (self.scroll as usize, self.area.height as usize);
        let shown = |link: usize| {
            self.spots()
                .any(|s| s.link == link && s.row >= top && s.row < top + height)
        };
        if self.focus.is_some_and(|link| !shown(link)) {
            self.focus = None;
        }
    }

    /// Where the page's links landed, as last flowed.
    fn spots(&self) -> impl Iterator<Item = &crate::markdown::LinkSpot> {
        self.rendered.iter().flat_map(|doc| &doc.links)
    }

    /// The page's keys: the scrolling ones it does itself, the rest it
    /// names for the event loop.
    pub fn key(&mut self, key: &KeyEvent) -> PageKey {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let cmd = key.modifiers.contains(KeyModifiers::SUPER);
        let page = i32::from(self.area.height.max(2)) - 1;
        match key.code {
            KeyCode::Enter if !ctrl && !cmd => {
                return match self.focus {
                    Some(link) => self.follow(link),
                    None => PageKey::Edit,
                }
            }
            KeyCode::Char('e') if !ctrl && !cmd => return PageKey::Edit,
            KeyCode::Char('o') if ctrl || cmd => return PageKey::Outside,
            _ if is_copy(key) => return PageKey::CopyPath,
            KeyCode::Tab => self.step_focus(true),
            KeyCode::BackTab => self.step_focus(false),
            KeyCode::Esc | KeyCode::Char('q') if !ctrl => {
                if self.focus.is_some() {
                    self.focus = None;
                } else if !self.go_back() {
                    return PageKey::Close;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(page),
            KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::Char('d') if ctrl => self.scroll_by(page / 2),
            KeyCode::Char('u') if ctrl => self.scroll_by(-page / 2),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll = self.max_scroll(),
            _ => {}
        }
        self.let_hidden_focus_go();
        PageKey::Done
    }

    /// The text the DOC SELECTION covers, when there is one.
    pub fn selected_text(&self) -> Option<String> {
        self.select.text(self.rendered.as_ref()?)
    }

    /// Whether Esc goes back to a page a link left, rather than closing.
    pub fn has_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// The links in the order `Tab` walks them — as they first appear down
    /// the page — each with the row it starts on.
    fn link_order(&self) -> Vec<(usize, usize)> {
        let mut seen = vec![false; self.rendered.as_ref().map_or(0, |doc| doc.dests.len())];
        let mut order = Vec::new();
        for spot in self.spots() {
            if let Some(seen) = seen.get_mut(spot.link).filter(|seen| !**seen) {
                *seen = true;
                order.push((spot.link, spot.row));
            }
        }
        order
    }

    /// `Tab` (`forward`) or `⇧Tab`: the next link, or the one before —
    /// from none, the first on screen, or the last — scrolled into view.
    fn step_focus(&mut self, forward: bool) {
        let order = self.link_order();
        let (Some(&first), Some(&last)) = (order.first(), order.last()) else {
            return;
        };
        let (top, height) = (self.scroll as usize, self.area.height as usize);
        let at = self
            .focus
            .and_then(|f| order.iter().position(|&(l, _)| l == f));
        let (link, row) = match at {
            Some(i) if forward => order[(i + 1) % order.len()],
            Some(i) => order[(i + order.len() - 1) % order.len()],
            None if forward => order
                .iter()
                .copied()
                .find(|&(_, row)| row >= top)
                .unwrap_or(first),
            None => order
                .iter()
                .rev()
                .copied()
                .find(|&(_, row)| row < top + height)
                .unwrap_or(last),
        };
        if row < top || row >= top + height {
            self.scroll = scroll_to_row(row).min(self.max_scroll());
        }
        self.focus = Some(link);
    }

    /// The link drawn at screen cell (`col`, `row`), if any.
    pub fn link_at(&self, col: u16, row: u16) -> Option<usize> {
        if !self.area.contains(ratatui::layout::Position::new(col, row)) {
            return None;
        }
        let (x, y) = (
            col - self.area.x,
            self.scroll as usize + (row - self.area.y) as usize,
        );
        self.spots()
            .find(|s| s.row == y && s.cols.contains(&x))
            .map(|s| s.link)
    }

    /// Follow `link`: another markdown page opens here, the one it leaves
    /// kept for Esc; a heading on this page is brought into view; anything
    /// else is the event loop's to open.
    pub fn follow(&mut self, link: usize) -> PageKey {
        let Some(dest) = self.rendered.as_ref().and_then(|doc| doc.dests.get(link)) else {
            return PageKey::Done;
        };
        let dest = dest.clone();
        self.focus = None;
        match self.target(&dest) {
            Target::Away(away) => return PageKey::Follow(away),
            Target::Anchor(anchor) => self.seek_anchor(&anchor),
            Target::Page(file, anchor) => {
                self.back.push((self.file.clone(), self.scroll));
                self.show(file, 0);
                if let Some(anchor) = anchor {
                    self.seek_anchor(&anchor);
                }
            }
        }
        PageKey::Done
    }

    /// Esc on a page a link opened: the one it left, scrolled where it was.
    /// False when there is none to go back to.
    fn go_back(&mut self) -> bool {
        let Some((file, scroll)) = self.back.pop() else {
            return false;
        };
        self.show(file, scroll);
        true
    }

    /// This page turned into `file`'s, at `scroll`, its history kept.
    fn show(&mut self, file: String, scroll: u16) {
        let mut next = MarkdownPage::open(self.root.clone(), file, 1, self.editor.clone());
        next.back = std::mem::take(&mut self.back);
        next.scroll = scroll;
        next.area = self.area;
        next.frame = self.frame;
        *self = next;
    }

    /// Bring the heading `anchor` names (GitHub's `#the-heading`) into
    /// view, and make it the line Enter edits at. A name no heading has
    /// leaves the page where it is.
    fn seek_anchor(&mut self, anchor: &str) {
        if let Some(line) = anchor_line(&self.text, anchor) {
            self.line = line;
            self.seek = Some(line);
        }
    }

    /// Where `dest`, a link on this page, points: a web address; a
    /// heading here; a markdown page; another file — the path read against
    /// this file's folder (or the checkout, for one starting `/`), its
    /// `%20`s decoded and any `?query` dropped — or nothing at all.
    fn target(&self, dest: &str) -> Target {
        let dest = dest.trim();
        if has_scheme(dest) {
            return Target::Away(Away::Web(dest.to_string()));
        }
        let (at, line) = crate::links::split_line_suffix(dest);
        let (path, anchor) = match at.split_once('#') {
            Some((path, anchor)) => (path, Some(anchor.to_string())),
            None => (at, None),
        };
        let path = path.split('?').next().unwrap_or(path);
        let path = crate::dropped_files::percent_decode(path).unwrap_or_else(|| path.to_string());
        if path.is_empty() {
            return match anchor {
                Some(anchor) => Target::Anchor(anchor),
                None => Target::Away(Away::Missing(dest.to_string())),
            };
        }
        let full = match path.strip_prefix('/') {
            Some(from_root) => self.root.join(from_root),
            None => self.path.parent().unwrap_or(&self.root).join(&path),
        };
        let full = lexically_normal(&full);
        if !full.is_file() {
            return Target::Away(Away::Missing(dest.to_string()));
        }
        if let Some(anchor) = anchor.clone().filter(|_| full == self.path) {
            return Target::Anchor(anchor);
        }
        let file = match full.strip_prefix(&self.root) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => full.to_string_lossy().into_owned(),
        };
        if crate::markdown::is_markdown_path(&file) {
            return Target::Page(file, anchor);
        }
        Target::Away(Away::File {
            file,
            line: line.unwrap_or(1),
        })
    }

    /// Where the page reads: `12/40`, the first row shown of all of them.
    pub fn position(&self) -> Option<String> {
        let lines = self.rendered.as_ref()?.lines.len();
        (lines > self.area.height as usize).then(|| format!("{}/{lines}", self.scroll + 1))
    }
}

/// The scroll that shows `row` a few rows down from the top, the rows
/// above it kept for context — where a page or preview opened at a line
/// starts.
pub(crate) fn scroll_to_row(row: usize) -> u16 {
    row.saturating_sub(SEEK_CONTEXT).min(u16::MAX as usize) as u16
}

/// `⌘C` / `^y`: the page's copy — the DOC SELECTION, else the path.
pub fn is_copy(key: &KeyEvent) -> bool {
    crate::hints::COPY_PATH.matches(key)
}

/// What a source line reads as on the page: its words, the markdown marks
/// in front of them and inside them stripped.
fn words_of(line: &str) -> String {
    let words: String = line
        .trim_start_matches(|c: char| c.is_whitespace() || "#>-*+".contains(c))
        .chars()
        .filter(|c| !"*_`".contains(*c))
        .collect();
    words.trim().to_string()
}

/// The page's rows as plain text.
fn row_texts(rendered: &Rendered) -> Vec<String> {
    rendered
        .lines
        .iter()
        .map(|row| crate::doc_select::row_text(row, 0, u16::MAX))
        .collect()
}

/// The first of `rows` from `from` on that holds the start of `words` —
/// its first two dozen characters, else its first dozen, where the page
/// broke a long line; None for words too short to tell apart.
fn find_words(rows: &[String], from: usize, words: &str) -> Option<usize> {
    for take in [24, 12] {
        let needle: String = words.chars().take(take).collect();
        if needle.chars().count() < 3 {
            return None;
        }
        if let Some(at) = rows.iter().skip(from).position(|row| row.contains(&needle)) {
            return Some(from + at);
        }
    }
    None
}

/// The rendered row that shows source line `line` (1-based) of `text`: the
/// first row holding the start of that line's words, markdown marks
/// stripped — else the same fraction of the way down the page.
pub(crate) fn seek_row(text: &str, line: u64, rendered: &Rendered) -> usize {
    let source: Vec<&str> = text.lines().collect();
    let index = (line.saturating_sub(1) as usize).min(source.len().saturating_sub(1));
    let words = words_of(source.get(index).copied().unwrap_or(""));
    let rows = row_texts(rendered);
    find_words(&rows, 0, &words).unwrap_or(rows.len() * index / source.len().max(1))
}

/// [`seek_row`] the other way: the source line (1-based) behind rendered
/// row `row` — the last line of `text` whose words start on that row or
/// above it. The lines are found down the page in order, so a line that
/// repeats is found where it stands, not at its first copy. 1 when none
/// is found.
pub(crate) fn source_line(text: &str, row: usize, rendered: &Rendered) -> u64 {
    let rows = row_texts(rendered);
    let (mut from, mut line) = (0, 1);
    for (i, source) in text.lines().enumerate() {
        let Some(at) = find_words(&rows, from, &words_of(source)) else {
            continue;
        };
        if at > row {
            break;
        }
        (from, line) = (at, i as u64 + 1);
    }
    line
}

/// The source line (1-based) of the heading `anchor` names, the way GitHub
/// names them: `## The needle, again` is `#the-needle-again`, and a second
/// heading of the same name `-1`. Headings inside fenced code are none.
fn anchor_line(text: &str, anchor: &str) -> Option<u64> {
    let want = crate::dropped_files::percent_decode(anchor)
        .unwrap_or_else(|| anchor.to_string())
        .to_lowercase();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut fenced = false;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_start();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        let Some(title) = line.strip_prefix('#').map(|t| t.trim_start_matches('#')) else {
            continue;
        };
        if fenced || !(title.is_empty() || title.starts_with(' ')) {
            continue;
        }
        let slug = slug(title);
        let copies = seen.entry(slug.clone()).or_insert(0);
        let named = match *copies {
            0 => slug,
            n => format!("{slug}-{n}"),
        };
        *copies += 1;
        if named == want {
            return Some(i as u64 + 1);
        }
    }
    None
}

/// A heading's anchor, as GitHub makes it: lower case, spaces as `-`, and
/// nothing but letters, digits, `-` and `_`.
fn slug(title: &str) -> String {
    title
        .trim()
        .trim_end_matches('#')
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// Does `dest` start with a URL scheme — `https:`, `mailto:`?
fn has_scheme(dest: &str) -> bool {
    dest.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    })
}

/// `path` with its `.` and `..` worked out, without asking the disk.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Breaks;

    fn flowed(page: &mut MarkdownPage, width: u16, height: u16) {
        page.area = Rect::new(0, 0, width, height);
        page.rendered = Some(Rendered::for_width(
            page.rendered.take(),
            &page.text,
            width,
            Breaks::Reflow,
            crate::theme::Theme::default(),
        ));
        page.settle();
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn a_save_on_disk_is_what_rereads_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "# one\n").unwrap();
        let mut page = MarkdownPage::open(dir.path().into(), "a.md".into(), 1, "micro".into());
        assert_eq!(page.text, "# one\n");
        assert!(!page.refresh(), "unchanged: nothing re-read");

        flowed(&mut page, 20, 5);
        std::fs::write(&path, "# two\n").unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(page.refresh());
        assert_eq!(page.text, "# two\n");
        assert!(page.rendered.is_none(), "the old flow is dropped");
    }

    /// The page wraps to its width rather than running off it, and the
    /// scrolling keys stay within the flow.
    #[test]
    fn the_page_wraps_and_scrolls_within_its_flow() {
        let dir = tempfile::tempdir().unwrap();
        let long = "word ".repeat(60);
        let text = format!("# Title\n\n{long}\n\n{}", "- item\n".repeat(30));
        std::fs::write(dir.path().join("a.md"), &text).unwrap();
        let mut page = MarkdownPage::open(dir.path().into(), "a.md".into(), 1, "micro".into());
        flowed(&mut page, 40, 10);
        let rows = page.rendered.as_ref().unwrap().lines.clone();
        assert!(
            rows.iter().all(|row| row.width() <= 40),
            "nothing past the edge"
        );
        assert!(
            rows.len() > 36,
            "the paragraph wrapped onto rows of its own"
        );

        let none = KeyModifiers::NONE;
        assert_eq!(page.key(&key(KeyCode::Down, none)), PageKey::Done);
        assert_eq!(page.scroll, 1);
        page.key(&key(KeyCode::PageDown, none));
        assert_eq!(page.scroll, 10, "a page less one row");
        page.key(&key(KeyCode::End, none));
        assert_eq!(page.scroll as usize, rows.len() - 10);
        page.key(&key(KeyCode::Down, none));
        assert_eq!(
            page.scroll as usize,
            rows.len() - 10,
            "the end stays the end"
        );
        page.key(&key(KeyCode::Home, none));
        assert_eq!(page.scroll, 0);
        page.key(&key(KeyCode::Up, none));
        assert_eq!(page.scroll, 0);
        assert_eq!(
            page.position().as_deref(),
            Some(&*format!("1/{}", rows.len()))
        );
    }

    #[test]
    fn the_keys_that_leave_the_page_are_named() {
        let dir = tempfile::tempdir().unwrap();
        let mut page = MarkdownPage::open(dir.path().into(), "a.md".into(), 1, "micro".into());
        let none = KeyModifiers::NONE;
        assert_eq!(page.key(&key(KeyCode::Enter, none)), PageKey::Edit);
        assert_eq!(page.key(&key(KeyCode::Char('e'), none)), PageKey::Edit);
        assert_eq!(
            page.key(&key(KeyCode::Char('o'), KeyModifiers::SUPER)),
            PageKey::Outside
        );
        assert_eq!(
            page.key(&key(KeyCode::Char('o'), KeyModifiers::CONTROL)),
            PageKey::Outside
        );
        assert_eq!(
            page.key(&key(KeyCode::Char('y'), KeyModifiers::CONTROL)),
            PageKey::CopyPath
        );
        assert_eq!(page.key(&key(KeyCode::Esc, none)), PageKey::Close);
        assert_eq!(page.key(&key(KeyCode::Char('q'), none)), PageKey::Close);
    }

    fn page_on(dir: &std::path::Path, file: &str, line: u64) -> MarkdownPage {
        MarkdownPage::open(dir.into(), file.into(), line, "fresh".into())
    }

    /// `Tab` walks the page's links from the first on screen, `⇧Tab` back,
    /// both round the ends; Enter on a relative `.md` link opens that page
    /// here, and Esc comes back to the one it left, scrolled as it was,
    /// before the next Esc closes. A link past the page's top, scrolled
    /// off, is let go.
    #[test]
    fn links_open_their_page_and_esc_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut text =
            String::from("# A\n\n[first](sub/b.md) and [second](sub/b.md#deep-part)\n\n");
        text.push_str(&"filler line\n\n".repeat(30));
        std::fs::write(dir.path().join("a.md"), &text).unwrap();
        let b = format!("# B\n\n{}## Deep part\n\nhere\n", "row\n\n".repeat(30));
        std::fs::write(dir.path().join("sub/b.md"), b).unwrap();
        let none = KeyModifiers::NONE;
        let mut page = page_on(dir.path(), "a.md", 1);
        flowed(&mut page, 60, 10);
        assert_eq!(page.key(&key(KeyCode::Tab, none)), PageKey::Done);
        assert_eq!(page.focus, Some(0));
        page.key(&key(KeyCode::Tab, none));
        assert_eq!(page.focus, Some(1));
        page.key(&key(KeyCode::Tab, none));
        assert_eq!(page.focus, Some(0), "round the end");
        page.key(&key(KeyCode::BackTab, none));
        assert_eq!(page.focus, Some(1));

        page.key(&key(KeyCode::BackTab, none));
        page.scroll = 3;
        assert_eq!(page.key(&key(KeyCode::Enter, none)), PageKey::Done);
        assert_eq!(page.file, "sub/b.md");
        assert!(page.has_back() && page.focus.is_none());
        assert!(page.text.starts_with("# B"));

        assert_eq!(page.key(&key(KeyCode::Esc, none)), PageKey::Done);
        assert_eq!((page.file.as_str(), page.scroll), ("a.md", 3));
        assert!(!page.has_back());
        flowed(&mut page, 60, 10);
        page.key(&key(KeyCode::Tab, none));
        page.key(&key(KeyCode::End, none));
        assert_eq!(page.focus, None, "scrolled off, let go");
        assert_eq!(page.key(&key(KeyCode::Esc, none)), PageKey::Close);

        // `#deep-part` lands on its heading, which is where Enter edits.
        let mut page = page_on(dir.path(), "a.md", 1);
        flowed(&mut page, 60, 10);
        assert_eq!(page.follow(1), PageKey::Done);
        flowed(&mut page, 60, 10);
        let rows = &page.rendered.as_ref().unwrap().lines;
        let shown: Vec<String> = rows[page.scroll as usize..]
            .iter()
            .take(10)
            .map(|r| r.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(shown.iter().any(|r| r.contains("Deep part")), "{shown:?}");
        assert_eq!(page.edit_line(), 63);
    }

    /// A link that leads off the page is the event loop's: the browser
    /// for an address with a scheme, the editor for a file that isn't
    /// markdown (at its `#L` line), the footer for one that isn't there;
    /// `/` is the checkout's top and `%20` a space.
    #[test]
    fn links_off_the_page_are_named_for_the_event_loop() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("my notes.md"), "# Notes\n").unwrap();
        std::fs::write(
            dir.path().join("docs/a.md"),
            "[web](https://example.com/x) [code](../main.rs#L3) [gone](nope.md) \
             [root](/my%20notes.md)\n",
        )
        .unwrap();
        let mut page = page_on(dir.path(), "docs/a.md", 1);
        flowed(&mut page, 80, 10);
        assert_eq!(
            page.follow(0),
            PageKey::Follow(Away::Web("https://example.com/x".into()))
        );
        assert_eq!(
            page.follow(1),
            PageKey::Follow(Away::File {
                file: "main.rs".into(),
                line: 3
            })
        );
        assert_eq!(
            page.follow(2),
            PageKey::Follow(Away::Missing("nope.md".into()))
        );
        assert_eq!(page.file, "docs/a.md", "none of those left the page");
        assert_eq!(page.follow(3), PageKey::Done);
        assert_eq!(page.file, "my notes.md");
    }

    /// Enter edits at the source line of the row at the top: a list item
    /// scrolled to the top is its own line, a wrapped paragraph's later
    /// row its paragraph's, and a line that repeats is found where it
    /// stands. A page opened on a hit edits at the hit until it moves.
    #[test]
    fn enter_edits_at_the_source_line_of_the_top_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut text = String::from("# Notes\n\n");
        for i in 0..20 {
            text.push_str(&format!("- entry number {i}\n"));
        }
        text.push_str(&format!("\n{}\n\n- entry number 3\n", "word ".repeat(40)));
        std::fs::write(dir.path().join("n.md"), &text).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let line_of = |needle: &str| lines.iter().rposition(|l| *l == needle).unwrap() as u64 + 1;

        let mut page = page_on(dir.path(), "n.md", 1);
        flowed(&mut page, 40, 6);
        assert_eq!(page.edit_line(), 1, "the heading");
        let entry = line_of("- entry number 5");
        let doc = page.rendered.clone().unwrap();
        page.scroll = seek_row(&text, entry, &doc) as u16;
        assert_eq!(page.edit_line(), entry);
        let wrapped = doc
            .lines
            .iter()
            .rposition(|r| r.spans.iter().any(|s| s.content.contains("word")))
            .unwrap();
        page.scroll = wrapped as u16;
        assert_eq!(
            page.edit_line(),
            line_of(&"word ".repeat(40)),
            "its paragraph"
        );
        page.scroll = doc.lines.len() as u16 - 1;
        assert_eq!(
            page.edit_line(),
            line_of("- entry number 3"),
            "the second copy"
        );

        let hit = line_of("- entry number 12");
        let mut page = page_on(dir.path(), "n.md", hit);
        flowed(&mut page, 40, 6);
        assert_eq!(page.edit_line(), hit, "the hit, untouched");
        page.key(&key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(page.edit_line(), hit - 1, "the top row's, once it moves");
    }

    /// Opened on a find-in-files hit, the page comes up with the hit's
    /// line in view, a couple of rows below the top.
    #[test]
    fn a_hit_opens_the_page_at_its_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut text = String::from("# Notes\n\n");
        for i in 0..40 {
            text.push_str(&format!("- entry number {i}\n"));
        }
        text.push_str("\n## The **needle** section\n\nmore\n");
        std::fs::write(dir.path().join("n.md"), &text).unwrap();
        let line = text.lines().position(|l| l.contains("needle")).unwrap() as u64 + 1;
        let mut page = MarkdownPage::open(dir.path().into(), "n.md".into(), line, "micro".into());
        flowed(&mut page, 60, 8);
        let rows = &page.rendered.as_ref().unwrap().lines;
        let shown: Vec<String> = rows
            .iter()
            .skip(page.scroll as usize)
            .take(8)
            .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            shown.iter().any(|row| row.contains("The needle section")),
            "{shown:?}"
        );
        assert_eq!(page.line, line, "Enter edits at the hit");
    }
}
