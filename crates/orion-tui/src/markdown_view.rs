//! MARKDOWN PAGE: a `.md` file opened from anywhere — Go to file (`⌘P`),
//! the TREE BROWSER, find in files (`⌘⇧F`), ⌥click — reads as its rendered
//! page, full width and wrapped, in a modal over everything else. `Enter`
//! (or `e`) edits it in the BUILT-IN EDITOR, which comes up over the page
//! and hands it back, re-read, when it quits; `⌘O` opens it in the OPEN IN
//! APP editor.
//!
//! Through 0.42 a markdown file opened as the MARKDOWN SPLIT instead: the
//! editor on the left and the page on the right, both at half width. A
//! document wants reading first, and the page is a key away from the
//! editor either way.

use std::path::PathBuf;
use std::time::SystemTime;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

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
    /// The line Enter opens the editor on: a find-in-files hit's, else 1.
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
}

/// What a key on the page asks of the event loop, beyond the scrolling the
/// page does itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKey {
    /// Enter / `e`: the file in the BUILT-IN EDITOR.
    Edit,
    /// `⌘O` / `^o`: the file in the OPEN IN APP editor.
    Outside,
    /// `⌘C` / `^y`: the file's path to the clipboard.
    CopyPath,
    /// Esc / `q`: the page closes.
    Close,
    /// Scrolled, or nothing to do.
    Done,
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
    }

    /// Re-read the file when it changed on disk since the last read — what
    /// the editor closing over the page does. True when it had.
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
            let row = seek_row(&self.text, line, rendered);
            self.scroll = row.saturating_sub(SEEK_CONTEXT).min(u16::MAX as usize) as u16;
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// The page's keys: the scrolling ones it does itself, the rest it
    /// names for the event loop.
    pub fn key(&mut self, key: &KeyEvent) -> PageKey {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let cmd = key.modifiers.contains(KeyModifiers::SUPER);
        let page = i32::from(self.area.height.max(2)) - 1;
        match key.code {
            KeyCode::Enter | KeyCode::Char('e') if !ctrl && !cmd => return PageKey::Edit,
            KeyCode::Char('o') if ctrl || cmd => return PageKey::Outside,
            KeyCode::Char('c') if cmd => return PageKey::CopyPath,
            KeyCode::Char('y') if ctrl => return PageKey::CopyPath,
            KeyCode::Esc | KeyCode::Char('q') if !ctrl => return PageKey::Close,
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
        PageKey::Done
    }

    /// Where the page reads: `12/40`, the first row shown of all of them.
    pub fn position(&self) -> Option<String> {
        let lines = self.rendered.as_ref()?.lines.len();
        (lines > self.area.height as usize).then(|| format!("{}/{lines}", self.scroll + 1))
    }
}

/// The rendered row that shows source line `line` (1-based) of `text`: the
/// first row holding the start of that line's words, markdown marks
/// stripped — else the same fraction of the way down the page.
fn seek_row(text: &str, line: u64, rendered: &Rendered) -> usize {
    let source: Vec<&str> = text.lines().collect();
    let index = (line.saturating_sub(1) as usize).min(source.len().saturating_sub(1));
    let words: String = source
        .get(index)
        .copied()
        .unwrap_or("")
        .trim_start_matches(|c: char| c.is_whitespace() || "#>-*+".contains(c))
        .chars()
        .filter(|c| !"*_`".contains(*c))
        .collect();
    let words = words.trim();
    let rows: Vec<String> = rendered
        .lines
        .iter()
        .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    for take in [24, 12] {
        let needle: String = words.chars().take(take).collect();
        if needle.chars().count() < 3 {
            break;
        }
        if let Some(row) = rows.iter().position(|row| row.contains(&needle)) {
            return row;
        }
    }
    rows.len() * index / source.len().max(1)
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
