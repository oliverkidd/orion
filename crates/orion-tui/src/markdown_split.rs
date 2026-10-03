//! MARKDOWN SPLIT: a `.md` file in the editor modal is the editor on the
//! left and its rendered page on the right, the page re-read whenever the
//! file on disk changes — so a save is what refreshes it. A terminal can't
//! edit the rendered page in place; this is the nearest thing.
//!
//! Below [`SPLIT_MIN_COLS`] the two don't fit side by side and the modal
//! shows one at a time; `Ctrl+T` swaps them there, and hides or shows the
//! page in the wide layout.

use std::path::PathBuf;
use std::time::SystemTime;

use ratatui::layout::Rect;

use crate::markdown::Rendered;

/// Narrowest editor-modal interior that lays the editor and page side by
/// side: two 50-column halves.
pub const SPLIT_MIN_COLS: u16 = 100;

/// Rows a wheel notch scrolls the page — the FILE TABS' pace.
pub const WHEEL_LINES: i32 = 3;

/// What the modal shows for a markdown file at the drawn width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Split,
    EditorOnly,
    PageOnly,
}

#[derive(Debug, Clone)]
pub struct MarkdownSide {
    /// The file, absolute.
    pub path: PathBuf,
    /// Its modification time when last read; a different one re-reads it.
    modified: Option<SystemTime>,
    /// Its text when last read (empty when it can't be read).
    pub text: String,
    /// The page flowed for the last drawn width, written back during draw.
    pub rendered: Option<Rendered>,
    /// Top visible page row.
    pub scroll: u16,
    /// `Ctrl+T` was pressed an odd number of times: the wide layout hides
    /// the page, the narrow one shows it instead of the editor.
    pub swapped: bool,
    /// The page's rect from the last draw (empty when it isn't drawn), for
    /// the wheel.
    pub area: Rect,
    /// Width of the modal's interior at the last draw: what the keys
    /// decide the [`Layout`] by.
    pub interior: u16,
}

impl MarkdownSide {
    pub fn load(path: PathBuf) -> Self {
        let mut side = Self {
            path,
            modified: None,
            text: String::new(),
            rendered: None,
            scroll: 0,
            swapped: false,
            area: Rect::default(),
            interior: 0,
        };
        side.reload();
        side
    }

    fn modified_now(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path).and_then(|m| m.modified()).ok()
    }

    fn reload(&mut self) {
        self.modified = self.modified_now();
        self.text = std::fs::read_to_string(&self.path).unwrap_or_default();
        self.rendered = None;
    }

    /// Re-read the file when it changed on disk since the last read. True
    /// when it did.
    pub fn refresh(&mut self) -> bool {
        if self.modified_now() == self.modified {
            return false;
        }
        self.reload();
        true
    }

    /// What to draw in an interior `width` columns wide.
    pub fn layout(&self, width: u16) -> Layout {
        match (width >= SPLIT_MIN_COLS, self.swapped) {
            (true, false) => Layout::Split,
            (true, true) | (false, false) => Layout::EditorOnly,
            (false, true) => Layout::PageOnly,
        }
    }

    pub fn toggle(&mut self) {
        self.swapped = !self.swapped;
    }

    /// Scroll the page by `delta` rows, kept within its last flow.
    pub fn scroll_by(&mut self, delta: i32) {
        let lines = self.rendered.as_ref().map_or(0, |r| r.lines.len());
        let max = lines.saturating_sub(self.area.height as usize) as i32;
        self.scroll = (self.scroll as i32 + delta).clamp(0, max.max(0)) as u16;
    }
}

/// `inner` cut into the editor's rect and the page's (a one-column rule
/// between them is the page's left edge) for `layout`; an empty rect for
/// the side not shown.
pub fn split(inner: Rect, layout: Layout) -> (Rect, Rect) {
    match layout {
        Layout::EditorOnly => (inner, Rect::default()),
        Layout::PageOnly => (Rect::default(), inner),
        Layout::Split => {
            let left = inner.width / 2;
            let editor = Rect { width: left, ..inner };
            let page = Rect {
                x: inner.x + left,
                width: inner.width - left,
                ..inner
            };
            (editor, page)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_splits_narrow_shows_one_and_ctrl_t_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let mut side = MarkdownSide::load(dir.path().join("a.md"));
        assert_eq!(side.layout(120), Layout::Split);
        assert_eq!(side.layout(80), Layout::EditorOnly);
        side.toggle();
        assert_eq!(side.layout(120), Layout::EditorOnly, "the page hidden");
        assert_eq!(side.layout(80), Layout::PageOnly, "the page instead");
    }

    #[test]
    fn a_save_on_disk_is_what_rereads_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "# one\n").unwrap();
        let mut side = MarkdownSide::load(path.clone());
        assert_eq!(side.text, "# one\n");
        assert!(!side.refresh(), "unchanged: nothing re-read");

        side.rendered = Some(Rendered {
            width: 10,
            lines: Vec::new(),
        });
        std::fs::write(&path, "# two\n").unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(side.refresh());
        assert_eq!(side.text, "# two\n");
        assert!(side.rendered.is_none(), "the old flow is dropped");
    }

    #[test]
    fn the_split_halves_the_interior() {
        let inner = Rect::new(2, 1, 101, 30);
        let (editor, page) = split(inner, Layout::Split);
        assert_eq!(editor, Rect::new(2, 1, 50, 30));
        assert_eq!(page, Rect::new(52, 1, 51, 30));
        assert_eq!(split(inner, Layout::EditorOnly), (inner, Rect::default()));
        assert_eq!(split(inner, Layout::PageOnly), (Rect::default(), inner));
    }

    #[test]
    fn the_page_scrolls_within_its_flow() {
        let dir = tempfile::tempdir().unwrap();
        let mut side = MarkdownSide::load(dir.path().join("a.md"));
        side.rendered = Some(Rendered {
            width: 40,
            lines: vec![ratatui::text::Line::raw("x"); 25],
        });
        side.area = Rect::new(0, 0, 40, 10);
        side.scroll_by(100);
        assert_eq!(side.scroll, 15);
        side.scroll_by(-100);
        assert_eq!(side.scroll, 0);
    }
}
