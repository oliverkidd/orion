//! DOC SELECTION — drag-to-select over a rendered markdown document: the
//! MARKDOWN PAGE, the FILE TABS' rendered preview, the TREE BROWSER's.
//! Orion owns the mouse, so the terminal's own selection never sees the
//! drag (`app::TermSelection` says why); this is the session pane's
//! selection again, over the flowed lines of a [`Rendered`] page instead
//! of a terminal's history: a drag selects a stream of text and copies it
//! when the button comes up, a double-click takes a word, the highlight
//! stays until the next click or key.
//!
//! Points are `(display column, document row)`, the row an index into
//! `Rendered::lines`, so the selection stays on its text while the page
//! scrolls under it. A copy joins SOFT WRAPS back up (`markdown::Wrap`):
//! a paragraph comes out as the sentence it was, not the rows it was
//! drawn in.

use std::time::Instant;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

use crate::markdown::{Rendered, Wrap};

/// A point in a document: display column, document row.
pub type Point = (u16, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocSelection {
    pub anchor: Point,
    pub head: Point,
    /// Still being dragged (button down). Cleared on mouse-up.
    pub dragging: bool,
    /// A real selection, not just an armed click: set once a drag leaves
    /// its starting cell, or at once for a double-click's word.
    pub active: bool,
}

impl DocSelection {
    /// A press: the selection armed at `at`, real once the drag moves.
    pub fn arm(at: Point) -> Self {
        Self {
            anchor: at,
            head: at,
            dragging: true,
            active: false,
        }
    }

    /// Endpoints in reading order: (start, end), both inclusive.
    pub fn bounds(&self) -> (Point, Point) {
        let key = |p: Point| (p.1, p.0);
        if key(self.anchor) <= key(self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The head follows a drag to `to`; a head that has left the anchor
    /// makes the selection real.
    pub fn drag_to(&mut self, to: Point) {
        self.head = to;
        if self.head != self.anchor {
            self.active = true;
        }
    }

    /// The columns of document row `row` inside the selection, inclusive.
    fn columns(&self, row: usize) -> Option<(u16, u16)> {
        let ((start_col, start_row), (end_col, end_row)) = self.bounds();
        if row < start_row || row > end_row {
            return None;
        }
        let from = if row == start_row { start_col } else { 0 };
        let to = if row == end_row { end_col } else { u16::MAX };
        Some((from, to))
    }

    /// The selected text: rows joined by newlines, a SOFT WRAP by a space
    /// with its lead left out. None until the selection is real, or when
    /// it covers nothing but blanks.
    pub fn text(&self, lines: &[Line], wraps: &[Wrap]) -> Option<String> {
        if !self.active {
            return None;
        }
        let ((_, start_row), (_, end_row)) = self.bounds();
        let mut out = String::new();
        let picked = lines.iter().enumerate().take(end_row + 1).skip(start_row);
        for (row, line) in picked {
            let Some((from, to)) = self.columns(row) else {
                continue;
            };
            let wrap = wraps.get(row).copied().unwrap_or_default();
            let joins = wrap.continues && row > start_row;
            let from = if joins { from.max(wrap.lead) } else { from };
            let piece = row_text(line, from, to);
            if row > start_row {
                out.push(if joins { ' ' } else { '\n' });
            }
            out.push_str(&piece);
        }
        let out = out.trim_end().to_string();
        (!out.trim().is_empty()).then_some(out)
    }

    /// REVERSED over the selected cells on screen: the document in `area`,
    /// its top row `scroll`.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, scroll: u16) {
        if !self.active {
            return;
        }
        let reversed = Style::default().add_modifier(Modifier::REVERSED);
        let last_col = area.width.saturating_sub(1);
        for y in 0..area.height {
            let Some((from, to)) = self.columns(scroll as usize + y as usize) else {
                continue;
            };
            let to = to.min(last_col);
            if from > to {
                continue;
            }
            let cells = Rect::new(area.x + from, area.y + y, to - from + 1, 1).intersection(area);
            buf.set_style(cells, reversed);
        }
    }
}

/// The text of `line` between display columns `from` and `to`, both
/// inclusive: a wide character counts when it starts before `to` and ends
/// after `from`. Trailing blanks are dropped.
pub fn row_text(line: &Line, from: u16, to: u16) -> String {
    let (from, to) = (from as usize, to as usize);
    let mut out = String::new();
    let mut col = 0usize;
    for ch in line.spans.iter().flat_map(|s| s.content.chars()) {
        let w = ch.width().unwrap_or(0);
        if col > to {
            break;
        }
        if col + w.max(1) > from {
            out.push(ch);
        }
        col += w;
    }
    out.trim_end().to_string()
}

/// The display width of `line`'s cells, chars in order, with their start
/// columns.
fn cells(line: &Line) -> Vec<(usize, char)> {
    let mut col = 0usize;
    line.spans
        .iter()
        .flat_map(|s| s.content.chars())
        .map(|ch| {
            let at = col;
            col += ch.width().unwrap_or(0);
            (at, ch)
        })
        .collect()
}

/// The double-click word at `at`: the longest run of non-blank characters
/// around it (identifiers, paths and URLs alike — the session pane's
/// rule), selected and real. None on a blank.
pub fn word_at(lines: &[Line], at: Point) -> Option<DocSelection> {
    let line = lines.get(at.1)?;
    let cells = cells(line);
    let col = at.0 as usize;
    let i = cells
        .iter()
        .position(|&(c, ch)| col >= c && col < c + ch.width().unwrap_or(0).max(1))?;
    if cells[i].1.is_whitespace() {
        return None;
    }
    let mut start = i;
    while start > 0 && !cells[start - 1].1.is_whitespace() {
        start -= 1;
    }
    let mut end = i;
    while end + 1 < cells.len() && !cells[end + 1].1.is_whitespace() {
        end += 1;
    }
    let (end_at, end_ch) = cells[end];
    let end_col = end_at + end_ch.width().unwrap_or(1).max(1) - 1;
    let clamp = |c: usize| c.min(u16::MAX as usize) as u16;
    Some(DocSelection {
        anchor: (clamp(cells[start].0), at.1),
        head: (clamp(end_col), at.1),
        dragging: false,
        active: true,
    })
}

/// The document point under screen cell (`col`, `row`) of a document
/// drawn in `area` from row `scroll`, `rows` long. A pointer past an edge
/// lands on that edge — above the area the start of its top row, below it
/// the end of its bottom row — and never past the last row.
pub fn point_at(area: Rect, scroll: u16, rows: usize, col: u16, row: u16) -> Point {
    let last_row = rows.saturating_sub(1);
    let last_col = area.width.saturating_sub(1);
    let (col, y) = if row < area.y {
        (0, 0)
    } else if row >= area.y + area.height {
        (last_col, area.height.saturating_sub(1))
    } else {
        (col.clamp(area.x, area.x + last_col) - area.x, row - area.y)
    };
    let doc_row = (scroll as usize + y as usize).min(last_row);
    (col, doc_row)
}

/// How far past `area`'s top (negative) or bottom (positive) a pointer
/// row is, in rows; 0 inside.
fn overshoot(area: Rect, row: u16) -> i32 {
    if row < area.y {
        -i32::from(area.y - row)
    } else if row >= area.y + area.height {
        i32::from(row - (area.y + area.height)) + 1
    } else {
        0
    }
}

/// Most rows one drag report past an edge scrolls the document.
const EDGE_STEP_MAX: i32 = 3;

/// What a view holding a document selection keeps between events.
#[derive(Debug, Clone, Default)]
pub struct DocSelect {
    pub selection: Option<DocSelection>,
    /// The last press, for telling a double-click (`is_double_click`).
    pub last_click: Option<(Instant, Point)>,
}

/// What a mouse event did to a document selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not a selection event: the view handles it as before.
    Ignored,
    /// Handled; repaint. `scroll` rows to move the document first, when
    /// a drag went past an edge.
    Moved { scroll: i32 },
    /// The selection finished (mouse-up, double-click): this text goes to
    /// the clipboard.
    Copy(String),
}

impl DocSelect {
    /// Forget the selection — the document it pointed into changed.
    pub fn clear(&mut self) {
        self.selection = None;
    }

    pub fn is_active(&self) -> bool {
        self.selection.is_some_and(|s| s.active)
    }

    /// The selected text of `doc`, when there is a real selection.
    pub fn text(&self, doc: &Rendered) -> Option<String> {
        self.selection?.text(&doc.lines, &doc.wraps)
    }

    /// The left button over a document drawn in `area` from row `scroll`:
    /// a press inside arms a selection (or, the second of a double-click,
    /// takes the word), a drag extends it — scrolling when it goes past
    /// an edge — and the release copies it. A press outside `area` clears
    /// it and is the view's.
    pub fn mouse(
        &mut self,
        mouse: &MouseEvent,
        area: Rect,
        scroll: u16,
        doc: &Rendered,
    ) -> Outcome {
        let rows = doc.lines.len();
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let inside = area.contains(Position::new(mouse.column, mouse.row));
                if !inside || rows == 0 {
                    self.selection = None;
                    return Outcome::Ignored;
                }
                let at = point_at(area, scroll, rows, mouse.column, mouse.row);
                if crate::event_loop::is_double_click(&mut self.last_click, at) {
                    self.selection = word_at(&doc.lines, at);
                    return match self.text(doc) {
                        Some(text) => Outcome::Copy(text),
                        None => Outcome::Moved { scroll: 0 },
                    };
                }
                self.selection = Some(DocSelection::arm(at));
                Outcome::Moved { scroll: 0 }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(sel) = self.selection.as_mut().filter(|s| s.dragging) else {
                    return Outcome::Ignored;
                };
                let step = overshoot(area, mouse.row).clamp(-EDGE_STEP_MAX, EDGE_STEP_MAX);
                let max_scroll = rows.saturating_sub(area.height as usize) as i64;
                let scrolled = (i64::from(scroll) + i64::from(step)).clamp(0, max_scroll) as u16;
                sel.drag_to(point_at(area, scrolled, rows, mouse.column, mouse.row));
                Outcome::Moved {
                    scroll: i32::from(scrolled) - i32::from(scroll),
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(sel) = self.selection.as_mut().filter(|s| s.dragging) else {
                    return Outcome::Ignored;
                };
                if !sel.active {
                    self.selection = None;
                    return Outcome::Moved { scroll: 0 };
                }
                sel.dragging = false;
                match self.text(doc) {
                    Some(text) => Outcome::Copy(text),
                    None => Outcome::Moved { scroll: 0 },
                }
            }
            _ => Outcome::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::text::Span;

    fn lines(rows: &[&str]) -> Vec<Line<'static>> {
        rows.iter()
            .map(|r| Line::from(Span::raw(r.to_string())))
            .collect()
    }

    fn sel(anchor: Point, head: Point) -> DocSelection {
        DocSelection {
            anchor,
            head,
            dragging: false,
            active: true,
        }
    }

    #[test]
    fn a_backward_drag_has_the_same_bounds_as_a_forward_one() {
        assert_eq!(sel((2, 1), (4, 3)).bounds(), sel((4, 3), (2, 1)).bounds());
        assert_eq!(sel((5, 0), (1, 0)).bounds(), ((1, 0), (5, 0)));
    }

    #[test]
    fn a_selection_on_one_row_copies_exactly_its_cells() {
        let doc = lines(&["hello world"]);
        assert_eq!(
            sel((6, 0), (10, 0)).text(&doc, &[]).as_deref(),
            Some("world")
        );
        assert_eq!(sel((1, 0), (3, 0)).text(&doc, &[]).as_deref(), Some("ell"));
    }

    #[test]
    fn rows_between_the_ends_copy_whole_and_lose_their_trailing_blanks() {
        let doc = lines(&["first row   ", "middle   ", "last row"]);
        assert_eq!(
            sel((6, 0), (3, 2)).text(&doc, &[]).as_deref(),
            Some("row\nmiddle\nlast")
        );
    }

    #[test]
    fn a_soft_wrap_joins_the_row_above_without_its_lead() {
        let doc = lines(&["• a long", "  sentence", "• next"]);
        let wraps = [
            Wrap::default(),
            Wrap {
                continues: true,
                lead: 2,
            },
            Wrap::default(),
        ];
        assert_eq!(
            sel((2, 0), (u16::MAX, 2)).text(&doc, &wraps).as_deref(),
            Some("a long sentence\n• next")
        );
    }

    #[test]
    fn a_wide_character_on_the_edge_is_copied_once() {
        let doc = lines(&["ab漢字cd"]);
        // 漢 is columns 2–3, 字 4–5: a selection from 3 starts inside 漢.
        assert_eq!(sel((3, 0), (4, 0)).text(&doc, &[]).as_deref(), Some("漢字"));
    }

    #[test]
    fn a_double_click_takes_the_run_of_non_blanks() {
        let doc = lines(&["see docs/keys.md here"]);
        let word = word_at(&doc, (8, 0)).expect("a word");
        assert_eq!(word.text(&doc, &[]).as_deref(), Some("docs/keys.md"));
        assert!(word_at(&doc, (3, 0)).is_none(), "a blank is no word");
        assert!(word_at(&doc, (40, 0)).is_none(), "past the row's end");
    }

    #[test]
    fn a_pointer_past_an_edge_lands_on_that_edge() {
        let area = Rect::new(10, 5, 20, 4);
        assert_eq!(point_at(area, 3, 100, 15, 6), (5, 4));
        assert_eq!(
            point_at(area, 3, 100, 15, 0),
            (0, 3),
            "above: top row start"
        );
        assert_eq!(
            point_at(area, 3, 100, 15, 40),
            (19, 6),
            "below: bottom row end"
        );
        assert_eq!(
            point_at(area, 0, 2, 15, 8),
            (5, 1),
            "never past the last row"
        );
    }

    #[test]
    fn an_armed_click_copies_nothing() {
        let doc = lines(&["text"]);
        assert_eq!(DocSelection::arm((1, 0)).text(&doc, &[]), None);
    }

    fn event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn a_drag_copies_on_release_and_a_click_does_not() {
        let doc = Rendered {
            width: 20,
            lines: lines(&["alpha beta", "gamma"]),
            wraps: vec![Wrap::default(); 2],
        };
        let area = Rect::new(0, 0, 20, 5);
        let mut s = DocSelect::default();
        let left = MouseButton::Left;
        s.mouse(&event(MouseEventKind::Down(left), 6, 0), area, 0, &doc);
        s.mouse(&event(MouseEventKind::Drag(left), 2, 1), area, 0, &doc);
        assert!(s.is_active());
        assert_eq!(
            s.mouse(&event(MouseEventKind::Up(left), 2, 1), area, 0, &doc),
            Outcome::Copy("beta\ngam".into())
        );
        assert!(s.is_active(), "the highlight stays after the copy");

        s.mouse(&event(MouseEventKind::Down(left), 15, 3), area, 0, &doc);
        assert_eq!(
            s.mouse(&event(MouseEventKind::Up(left), 15, 3), area, 0, &doc),
            Outcome::Moved { scroll: 0 }
        );
        assert!(
            s.selection.is_none(),
            "a plain click leaves nothing selected"
        );
    }

    #[test]
    fn a_drag_past_the_bottom_scrolls_the_document() {
        let rows: Vec<String> = (0..20).map(|i| format!("row {i}")).collect();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let doc = Rendered {
            width: 20,
            lines: lines(&refs),
            wraps: vec![Wrap::default(); 20],
        };
        let area = Rect::new(0, 2, 20, 5);
        let mut s = DocSelect::default();
        let left = MouseButton::Left;
        s.mouse(&event(MouseEventKind::Down(left), 0, 2), area, 0, &doc);
        let out = s.mouse(&event(MouseEventKind::Drag(left), 3, 9), area, 0, &doc);
        assert_eq!(out, Outcome::Moved { scroll: 3 });
        assert_eq!(s.selection.unwrap().head, (19, 7), "the new bottom row");
        let out = s.mouse(&event(MouseEventKind::Drag(left), 3, 0), area, 0, &doc);
        assert_eq!(out, Outcome::Moved { scroll: 0 }, "the top stays the top");
    }
}
