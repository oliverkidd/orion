//! Text field with the editing keys a terminal user expects — one line by
//! default, multi-row on request.
//!
//! Every typed field in the TUI — the prompt dialog, the fuzzy filters,
//! the grep query, the ssh destination, the task boxes, a preset's prefix
//! and postfix, an issue's description — is one of these, so the keys are
//! learned once and work everywhere: arrows and Home/End, word motion on
//! ⌥←/⌥→, the readline control chords (Ctrl+A/E/B/F/W/U/K), and word/line
//! deletes.
//!
//! A field holds a SELECTION too, the way a macOS text field does: an
//! anchor where it began and the caret where it ends. Shift on any motion
//! extends it — ⇧←/⇧→ by character, ⌥⇧←/⌥⇧→ by word, ⌘⇧←/⌘⇧→ to the
//! line's ends, ⇧Home/⇧End the same, and in a multi-row field ⇧↑/⇧↓ by
//! row (past the first or last, to the very start or end) and ⌘⇧↑/⌘⇧↓ to
//! the text's ends — and ⌘A takes the whole text. The same motion without
//! Shift lets it go: ←/→ land on its near edge and stop there, every other
//! motion sets out from that edge. Typing, a paste and a line break
//! replace it; ⌫, Delete and the word and line deletes remove just it.
//! ⌘C copies it and ⌘X cuts it, to the system clipboard ([`take_copied`]).
//! Every renderer draws it on the theme's selection background
//! (`ui::field_spans`).
//!
//! And the MOUSE works in every field the same way, as it does in any
//! macOS text field (and in Claude Code's own prompt): a click puts the
//! caret where it points, a drag selects, a double-click takes the word
//! and a triple-click the line, ⇧-click stretches the selection there, and
//! the wheel scrolls a box that runs past its rows. Each renderer records
//! where it drew its field ([`place_line`], [`place_rows`]); the event loop
//! hands the mouse to the field under it ([`TextInput::mouse`]).
//!
//! A [`TextInput::multiline`] field holds hard line breaks as well, and
//! takes them the way Claude Code's own prompt does: Shift+Enter,
//! Option+Enter — the `ESC` `CR` a terminal without the kitty protocol
//! sends for a mapped Shift+Enter, which is Alt+Enter to us — and Ctrl+J
//! all break the line; ↑/↓ walk the rows as drawn — a long paragraph's
//! wrapped rows too, keeping the column through a short one — and fall
//! through to the caller past the first or last, so a form can step to its
//! next field; ⌥↑/⌥↓ jump by paragraph, Cmd+↑/↓ (Ctrl+Home/End) to either
//! end, PageUp/PageDown a screenful; Home/End and the readline chords work
//! on the line under the caret; a paste keeps its newlines. A one-line
//! field flattens a paste and leaves the break keys to the caller, so
//! Enter — always the caller's — stays the only way out of it.
//!
//! On macOS the option-arrow combos are what actually reaches us as
//! `Alt+b` / `Alt+f`: both Terminal.app (its bundled keyMappings.plist maps
//! `~F702`/`~F703` to `ESC b` / `ESC f`) and iTerm2 send the readline word
//! sequences rather than a modified arrow, so those two chords matter more
//! than `Alt+Left`/`Alt+Right` — we accept both.
//!
//! The field never claims a key an overlay wants for itself: `handle_key`
//! returns [`Edit::Ignored`] for anything it doesn't recognize, and callers
//! run it last, after their own bindings have had first refusal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

thread_local! {
    /// What the last ⌘C or ⌘X in any field copied, until the event loop
    /// takes it for the clipboard ([`take_copied`]). A field has no route
    /// to the clipboard of its own — locally a process, over ssh an OSC 52
    /// request the main loop writes — and there are thirty-odd of them, so
    /// each hands its text here rather than through every caller.
    static COPIED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The text the last ⌘C / ⌘X in a field copied, once: the event loop puts
/// it on the clipboard after every key.
pub fn take_copied() -> Option<String> {
    COPIED.with(|c| c.borrow_mut().take())
}

/// Which field is which, across the clone a frame is drawn from: a field
/// and its clones share one, so where the clone was drawn is where the
/// live field is. Every new field takes the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FieldId(u64);

impl Default for FieldId {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// How a field was laid out where it was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// One row, scrolled sideways: the char in the first column.
    Line { first: usize },
    /// Wrapped into [`TextInput::rows`] the area's width: the row at the
    /// top.
    Rows { top: u16 },
}

/// Where a field was drawn: the cells its text sits in, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placed {
    area: Rect,
    shape: Shape,
}

/// Every field drawn in the frame on screen (`drawn`), and in the one
/// before it (`last`) — a one-line field scrolls on from where it was.
#[derive(Default)]
struct Placements {
    drawn: HashMap<FieldId, Placed>,
    last: HashMap<FieldId, Placed>,
}

thread_local! {
    /// Where each field was drawn, for the mouse. A draw works on a clone
    /// of the overlay and a field's renderer seldom knows which overlay it
    /// is in, so rather than every one of them writing its rect back, each
    /// renderer records it here under the field's [`FieldId`].
    static PLACED: RefCell<Placements> = RefCell::default();
}

/// A frame is starting: what was drawn becomes the last frame's, and each
/// field records afresh where it lands. A field not drawn this frame takes
/// no clicks.
pub fn new_frame() {
    PLACED.with(|p| {
        let p = &mut *p.borrow_mut();
        std::mem::swap(&mut p.last, &mut p.drawn);
        p.drawn.clear();
    });
}

fn place(input: &TextInput, area: Rect, shape: Shape) {
    PLACED.with(|p| {
        p.borrow_mut()
            .drawn
            .insert(input.id, Placed { area, shape })
    });
}

/// `input` was drawn one row high in `area`, from char `first` —
/// [`TextInput::line_first`]'s, for a field drawn live.
pub fn place_line(input: &TextInput, area: Rect, first: usize) {
    place(input, Rect { height: 1, ..area }, Shape::Line { first });
}

/// `input` was drawn wrapped into the rows `area` is wide
/// ([`TextInput::rows`]), from row `top`.
pub fn place_rows(input: &TextInput, area: Rect, top: u16) {
    place(input, area, Shape::Rows { top });
}

/// What a press selects, and a drag from it grows by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Line,
}

/// The left button, pressed in the field and not yet let go.
#[derive(Debug, Clone, Copy)]
struct Press {
    unit: Unit,
    /// What the press itself selected, as a byte range — empty for a
    /// single click — which a drag's selection always covers.
    origin: (usize, usize),
}

/// The last click, for counting a double- or triple-click.
#[derive(Debug, Clone, Copy)]
struct Click {
    at: Instant,
    cell: (u16, u16),
    count: u8,
}

/// What one key press did to the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Not an editing key — the caller still owns it.
    Ignored,
    /// The cursor moved (or hit an end); the text is unchanged.
    Moved,
    /// The text changed — re-run whatever this field feeds.
    Changed,
}

impl Edit {
    /// Did the key belong to the field at all?
    pub fn consumed(self) -> bool {
        !matches!(self, Edit::Ignored)
    }

    /// Does whatever this field drives (a filter, a search, a listing) need
    /// recomputing?
    pub fn changed(self) -> bool {
        matches!(self, Edit::Changed)
    }
}

/// Editable text plus a cursor into it: one line, or many.
#[derive(Clone, Default)]
pub struct TextInput {
    text: String,
    /// Byte offset into `text`; always on a char boundary, always ≤ len.
    cursor: usize,
    /// Where the SELECTION began, as a byte offset like `cursor`; it runs
    /// from here to the caret, either way round. None — or the caret's own
    /// offset, never kept — is no selection.
    anchor: Option<usize>,
    /// Hard line breaks allowed: the break keys insert one and a paste
    /// keeps its own. Off, the field is one line whatever comes in.
    multiline: bool,
    /// The column a run of ↑/↓ aims for, so passing through a short or
    /// empty row doesn't drag the caret to its end for good. Any other
    /// key lets it go.
    goal: Option<u16>,
    /// Where a multi-row field was last drawn (see [`TextView`]).
    view: TextView,
    /// Which field this is, for where it was drawn ([`Placements`]).
    id: FieldId,
    /// The left button held down in the field: the drag it selects by.
    press: Option<Press>,
    /// The last click, for a double- or triple-click.
    click: Option<Click>,
}

/// Where a multi-row field was last drawn: the columns its rows wrap at,
/// how many rows show, and the first one shown. The renderer hands it back
/// (draws work on a clone) so ↑/↓ walk the rows the user SEES — a long
/// paragraph wrapped over five rows is five rows, not one — PageUp/PageDown
/// move a screenful, and a click lands where it points. All zero before
/// the first draw, where the rows are the hard lines. In cells, as a
/// ratatui `Rect` counts them — every field carries one, so it stays small.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextView {
    pub width: u16,
    pub height: u16,
    pub top: u16,
}

/// A row or column count as a [`TextView`] keeps it, saturating.
fn cells(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// A field's value is its text, caret, SELECTION and shape — the range
/// selected, that is, the anchor being the caret's other end; the column
/// a run of ↑/↓ aims for, where it was last drawn and what the mouse is
/// doing in it are not part of it.
impl PartialEq for TextInput {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.cursor == other.cursor
            && self.selection() == other.selection()
            && self.multiline == other.multiline
    }
}

impl Eq for TextInput {}

/// Which field it is and what the mouse is doing in it are left out, as
/// they are of its value: two fields that read the same print the same.
impl fmt::Debug for TextInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextInput")
            .field("text", &self.text)
            .field("cursor", &self.cursor)
            .field("anchor", &self.anchor)
            .field("multiline", &self.multiline)
            .field("goal", &self.goal)
            .field("view", &self.view)
            .finish()
    }
}

/// Word characters for ⌥-arrow / Ctrl+W motion: a run of these is one word,
/// everything else (spaces, `/`, `-`, `.`) separates. Matches what readline
/// does in a shell, which is where the muscle memory comes from.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The char `col` chars into row `row` of a [`TextInput::rows`] layout —
/// or as far as that row lets a caret stand: a soft-wrapped row's last
/// spot is its final char (one step on is the next row's start), a hard
/// line's is its end.
fn spot_in_row(rows: &[(usize, usize)], row: usize, col: usize) -> usize {
    let (start, end) = rows[row];
    let last = if is_soft(rows, row) { end - 1 } else { end };
    (start + col).min(last)
}

/// Does row `i` of a [`TextInput::rows`] layout end at a soft wrap — the
/// next row picking up at the very char it stopped before — rather than
/// at a line break or the end of the text?
fn is_soft(rows: &[(usize, usize)], i: usize) -> bool {
    rows.get(i + 1).is_some_and(|next| next.0 == rows[i].1)
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// A field pre-filled with `text`, cursor parked at the end — the state
    /// you want when an edit starts from an existing value.
    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self {
            text,
            cursor,
            ..Self::default()
        }
    }

    /// An empty multi-row field: line breaks typed, pasted and walked.
    pub fn multiline() -> Self {
        Self {
            multiline: true,
            ..Self::default()
        }
    }

    /// A multi-row field pre-filled with `text`, cursor at the end.
    pub fn multiline_with_text(text: impl Into<String>) -> Self {
        let mut input = Self::with_text(text);
        input.multiline = true;
        input
    }

    /// Switch line breaks on or off for a field built before its shape was
    /// known — a prompt dialog decides by its kind.
    pub fn set_multiline(&mut self, multiline: bool) {
        self.multiline = multiline;
    }

    /// Is `key` one of the chords that break a line — Shift+Enter,
    /// Option (Alt)+Enter or Ctrl+J? The three ways the one intent reaches
    /// a terminal program: the kitty protocol delivers the shifted Enter
    /// as a key of its own; a mapped Shift+Enter (Claude Code's
    /// `/terminal-setup`, or Option+Enter with Option as Meta) arrives as
    /// `ESC` `CR`, which is Alt+Enter; and Ctrl+J is the line feed itself,
    /// which every terminal and tmux pass through. Claude Code's prompt
    /// takes all three, so its muscle memory works here.
    pub fn is_newline_key(key: &KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter => key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT),
            KeyCode::Char('j' | 'J') => key.modifiers.contains(KeyModifiers::CONTROL),
            _ => false,
        }
    }

    /// Will [`handle_key`](Self::handle_key) turn `key` into a line break
    /// here? Only in a multi-row field. A caller whose Enter submits guards
    /// that arm with this, so a shifted Enter is never a send in a box
    /// that breaks lines.
    pub fn takes_newline(&self, key: &KeyEvent) -> bool {
        self.multiline && Self::is_newline_key(key)
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Cursor as a char offset — the unit the renderer draws in.
    pub fn cursor_chars(&self) -> usize {
        self.text[..self.cursor].chars().count()
    }

    /// Cursor as a byte offset into [`as_str`](Self::as_str).
    pub fn caret_byte(&self) -> usize {
        self.cursor
    }

    /// Replace the text from byte `start` up to the caret with `with`, the
    /// caret landing after it — a completion written over the word it
    /// completes. A `start` past the caret, or off a char boundary,
    /// changes nothing.
    pub fn replace_to_caret(&mut self, start: usize, with: &str) {
        if start > self.cursor || !self.text.is_char_boundary(start) {
            return;
        }
        self.text.replace_range(start..self.cursor, with);
        self.cursor = start + with.len();
        self.goal = None;
    }

    /// Park the caret `at` chars in, clamped to the end of the text.
    fn set_cursor_chars(&mut self, at: usize) {
        self.cursor = self.byte_of(at);
    }

    /// The byte offset of char `at`, clamped to the end of the text.
    fn byte_of(&self, at: usize) -> usize {
        self.text
            .char_indices()
            .nth(at)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Caret to the very start of the text — ↑ on a task box's top row.
    /// Lets any SELECTION go.
    pub fn cursor_to_start(&mut self) {
        self.goal = None;
        self.anchor = None;
        self.cursor = 0;
    }

    /// Caret to the very end of the text — ↓ on a task box's bottom row.
    /// Lets any SELECTION go.
    pub fn cursor_to_end(&mut self) {
        self.goal = None;
        self.anchor = None;
        self.cursor = self.text.len();
    }

    /// Replace the whole value, cursor to the end, nothing selected.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.anchor = None;
        self.goal = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.anchor = None;
        self.goal = None;
    }

    /// The SELECTION as a byte range, start before end — None when nothing
    /// is selected.
    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor.filter(|a| *a != self.cursor)?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    /// The SELECTION as a char range, start before end — the unit the
    /// renderer draws in, as [`cursor_chars`](Self::cursor_chars) is.
    /// None when nothing is selected.
    pub fn selection_chars(&self) -> Option<(usize, usize)> {
        let (start, end) = self.selection()?;
        let start_chars = self.text[..start].chars().count();
        Some((
            start_chars,
            start_chars + self.text[start..end].chars().count(),
        ))
    }

    /// The selected text, when there is any.
    pub fn selected(&self) -> Option<&str> {
        self.selection().map(|(start, end)| &self.text[start..end])
    }

    /// The char just before where an insert lands — the caret, or the
    /// start of the SELECTION it would replace. None at the very start.
    pub fn char_before_insert(&self) -> Option<char> {
        let at = self.selection().map_or(self.cursor, |(start, _)| start);
        self.char_before(at)
    }

    /// ⌘A: the whole text selected, the caret at its end.
    pub fn select_all(&mut self) {
        self.goal = None;
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    /// Take the SELECTION out of the text, the caret where it began. False
    /// — the text untouched — when nothing is selected.
    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(start..end, "");
        self.cursor = start;
        self.anchor = None;
        true
    }

    /// The text as the rows a `width`-column box shows it in, as char
    /// ranges. A hard line break always ends a row (an empty line is an
    /// empty row); a line wider than the box breaks after the last space
    /// that fits — mid-word only for a word wider than the row — and the
    /// row keeps that space. The one layout the renderer draws and ↑/↓,
    /// PageUp/PageDown, the wheel and a click walk. Width 0 wraps nothing:
    /// the rows are the hard lines.
    pub fn rows(&self, width: usize) -> Vec<(usize, usize)> {
        let chars: Vec<char> = self.text.chars().collect();
        let width = if width == 0 { usize::MAX } else { width };
        let mut rows = Vec::new();
        let mut line_start = 0;
        loop {
            let line_end = chars[line_start..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(chars.len(), |i| line_start + i);
            if line_start == line_end {
                rows.push((line_start, line_end));
            }
            let mut start = line_start;
            while start < line_end {
                let hard_end = start.saturating_add(width).min(line_end);
                let end = if hard_end < line_end {
                    chars[start..hard_end]
                        .iter()
                        .rposition(|c| c.is_whitespace())
                        .map_or(hard_end, |i| start + i + 1)
                } else {
                    hard_end
                };
                rows.push((start, end));
                start = end;
            }
            if line_end == chars.len() {
                return rows;
            }
            line_start = line_end + 1;
        }
    }

    /// Which of `rows` the caret is drawn on. At a soft break it starts
    /// the next row; at a hard break, or the end of the text, it closes
    /// its own.
    pub fn caret_row(&self, rows: &[(usize, usize)]) -> usize {
        let caret = self.cursor_chars();
        rows.iter()
            .enumerate()
            .position(|(i, &(start, end))| {
                start <= caret && (caret < end || (caret == end && !is_soft(rows, i)))
            })
            .unwrap_or(rows.len().saturating_sub(1))
    }

    /// Record where the field was drawn — what [`view_for`](Self::view_for)
    /// returned — on the live field, the draw having worked on a clone.
    pub fn set_view(&mut self, view: TextView) {
        self.view = view;
    }

    /// The view to draw a `width` × `height` box with: the last draw's
    /// first row, moved only as far as brings the caret into sight, and
    /// never so far down that rows go unused below the text. So ↑/↓ inside
    /// the box leave it still, and the text doesn't jump about as it is
    /// typed.
    pub fn view_for(&self, width: u16, height: u16) -> TextView {
        let rows = self.rows(width.into());
        let caret = self.caret_row(&rows);
        let height = height.max(1);
        let shown = usize::from(height);
        let top = usize::from(self.view.top)
            .min(rows.len().saturating_sub(shown))
            .clamp(caret.saturating_sub(shown - 1), caret);
        TextView {
            width,
            height,
            top: cells(top),
        }
    }

    /// The wheel over the box: scroll it `delta` rows, bringing the caret
    /// along only when it would leave the rows in sight. [`Edit::Ignored`]
    /// with nowhere further to scroll.
    pub fn scroll_rows(&mut self, delta: isize) -> Edit {
        let rows = self.rows(self.view.width.into());
        let height = usize::from(self.view.height.max(1));
        let old = usize::from(self.view.top);
        let top = old
            .saturating_add_signed(delta)
            .min(rows.len().saturating_sub(height));
        if top == old {
            return Edit::Ignored;
        }
        self.view.top = cells(top);
        let row = self.caret_row(&rows);
        let into = row.clamp(top, top + height - 1);
        if into != row {
            self.anchor = None;
            let col = self
                .goal
                .map_or(self.cursor_chars() - rows[row].0, usize::from);
            self.land(&rows, into, col);
            self.goal = Some(cells(col));
        }
        Edit::Moved
    }

    /// The first char a one-line field `budget` cells wide shows: where
    /// it was last drawn from, moved only as far as keeps the caret in
    /// sight and off an edge's `…` — so the text holds still under the
    /// pointer and the caret, as a macOS text field's does — or, never
    /// drawn, the caret near the middle.
    pub fn line_first(&self, budget: usize) -> usize {
        let caret = self.cursor_chars();
        let len = self.text.chars().count();
        let total = self.line_cells();
        let budget = budget.max(1);
        if total <= budget {
            return 0;
        }
        // As far along as shows the end and the cell past it, the caret
        // there or not — so stepping off the end doesn't nudge the text.
        let latest = (len + 1).saturating_sub(budget);
        let centred = caret.saturating_sub(budget / 2);
        let last = PLACED.with(|p| {
            let p = p.borrow();
            match p.drawn.get(&self.id).or_else(|| p.last.get(&self.id)) {
                Some(Placed {
                    shape: Shape::Line { first },
                    ..
                }) => Some(*first),
                _ => None,
            }
        });
        let first = match last {
            // Never on a first cell an elided start covers, nor on the
            // last one, which an elided end does — unless too narrow for a
            // `…` either side of it.
            Some(first) if budget >= 3 => first
                .min(caret.saturating_sub(1))
                .max((caret + 2).saturating_sub(budget)),
            _ => centred,
        };
        first.min(latest)
    }

    /// The cells a one-line field's text takes: a char each, and one more
    /// for a caret past the last character to sit in.
    pub fn line_cells(&self) -> usize {
        let len = self.text.chars().count();
        len + usize::from(self.cursor_chars() >= len)
    }

    /// Where the field was drawn in the frame on screen, if it was.
    fn placed(&self) -> Option<Placed> {
        PLACED.with(|p| p.borrow().drawn.get(&self.id).copied())
    }

    /// Is `at` — a cell on screen — in the field as last drawn?
    pub fn under(&self, at: Position) -> bool {
        self.placed().is_some_and(|p| p.area.contains(at))
    }

    /// Is the left button held down in the field — a drag selecting?
    pub fn pressed(&self) -> bool {
        self.press.is_some()
    }

    /// Let a press go without its release — another took the mouse.
    pub fn release(&mut self) {
        self.press = None;
    }

    /// The mouse, in screen cells: the field's when it was drawn under the
    /// pointer (a press, the wheel over a box that runs past its rows) or
    /// the button was pressed in it (a drag, the release). [`Edit::Ignored`]
    /// — the caller's — otherwise.
    ///
    /// A click puts the caret where it points: past a row's end at that
    /// end, below the last row at the text's end. A second and a third
    /// click on the spot take the word and the line, and a drag from any
    /// of them selects on by chars, words or lines. ⇧-click stretches the
    /// SELECTION (or makes one from the caret) to the spot. A drag past an
    /// edge scrolls the field a step at a time.
    pub fn mouse(&mut self, event: &MouseEvent) -> Edit {
        self.mouse_at(event, Instant::now())
    }

    /// [`mouse`](Self::mouse), the clock read by the caller.
    pub fn mouse_at(&mut self, event: &MouseEvent, now: Instant) -> Edit {
        let at = Position::new(event.column, event.row);
        let Some(placed) = self.placed() else {
            self.press = None;
            return Edit::Ignored;
        };
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if placed.area.contains(at) => {
                self.press_at(
                    placed,
                    at,
                    event.modifiers.contains(KeyModifiers::SHIFT),
                    now,
                );
                Edit::Moved
            }
            MouseEventKind::Drag(MouseButton::Left) if self.press.is_some() => {
                self.drag_to(placed, at);
                Edit::Moved
            }
            MouseEventKind::Up(MouseButton::Left) if self.press.is_some() => {
                self.press = None;
                Edit::Moved
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if placed.area.contains(at) && matches!(placed.shape, Shape::Rows { .. }) =>
            {
                self.sync_view(placed);
                let step = if event.kind == MouseEventKind::ScrollUp {
                    -1
                } else {
                    1
                };
                self.scroll_rows(step * crate::pr_modal::WHEEL_LINES as isize)
            }
            _ => Edit::Ignored,
        }
    }

    /// A box drawn wrapped walks the rows it was drawn in — the view a
    /// renderer with no write-back never handed it.
    fn sync_view(&mut self, placed: Placed) {
        if let Shape::Rows { top } = placed.shape {
            self.view = TextView {
                width: placed.area.width,
                height: placed.area.height,
                top,
            };
        }
    }

    fn press_at(&mut self, placed: Placed, at: Position, shift: bool, now: Instant) {
        self.goal = None;
        self.sync_view(placed);
        let spot = self.point(placed, at);
        let cell = (at.x, at.y);
        let count = match self.click {
            Some(last)
                if last.cell == cell
                    && now.saturating_duration_since(last.at)
                        <= crate::event_loop::DOUBLE_CLICK =>
            {
                last.count % 3 + 1
            }
            _ => 1,
        };
        self.click = Some(Click {
            at: now,
            cell,
            count,
        });
        if shift {
            let anchor = self.anchor.unwrap_or(self.cursor);
            self.press = Some(Press {
                unit: Unit::Char,
                origin: (anchor, anchor),
            });
            self.extend_to(spot);
            return;
        }
        let unit = match count {
            1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let origin = self.unit_at(unit, spot);
        self.press = Some(Press { unit, origin });
        self.anchor = (origin.0 != origin.1).then_some(origin.0);
        self.cursor = origin.1;
    }

    fn drag_to(&mut self, placed: Placed, at: Position) {
        let area = placed.area;
        let (placed, at) = match placed.shape {
            // Past a box's top or bottom it scrolls a row, the caret
            // following onto its edge row.
            Shape::Rows { .. } => {
                let y = if at.y < area.y {
                    self.scroll_rows(-1);
                    area.y
                } else if at.y >= area.bottom() {
                    self.scroll_rows(1);
                    area.bottom().saturating_sub(1)
                } else {
                    at.y
                };
                let shape = Shape::Rows { top: self.view.top };
                (Placed { shape, ..placed }, Position::new(at.x, y))
            }
            Shape::Line { .. } => (placed, at),
        };
        let spot = self.point(placed, at);
        self.extend_to(spot);
    }

    /// The SELECTION from the press's origin out to whichever unit holds
    /// `spot`, the caret on its far end.
    fn extend_to(&mut self, spot: usize) {
        let Some(press) = self.press else { return };
        let (start, end) = self.unit_at(press.unit, spot);
        let (from, to) = press.origin;
        if start < from {
            self.anchor = Some(to);
            self.cursor = start;
        } else {
            self.anchor = Some(from);
            self.cursor = end.max(to);
        }
        if self.anchor == Some(self.cursor) {
            self.anchor = None;
        }
    }

    /// The span of `unit` at byte `at`: the spot itself, the word — or run
    /// of spaces, or of punctuation — it is in, or its line.
    fn unit_at(&self, unit: Unit, at: usize) -> (usize, usize) {
        match unit {
            Unit::Char => (at, at),
            Unit::Line => (self.line_start(at), self.line_end(at)),
            Unit::Word => {
                // The char at the spot decides the kind; at a line's end,
                // the one before it.
                let Some(c) = self
                    .char_at(at)
                    .filter(|c| *c != '\n')
                    .or_else(|| self.char_before(at).filter(|c| *c != '\n'))
                else {
                    return (at, at);
                };
                let kind = |c: char| (is_word(c), c.is_whitespace());
                let want = kind(c);
                let mut start = at;
                while self
                    .char_before(start)
                    .is_some_and(|b| b != '\n' && kind(b) == want)
                {
                    start = self.prev_boundary(start);
                }
                let mut end = at;
                while self
                    .char_at(end)
                    .is_some_and(|n| n != '\n' && kind(n) == want)
                {
                    end = self.next_boundary(end);
                }
                (start, end)
            }
        }
    }

    /// The byte offset a screen cell points at in the field as `placed`:
    /// the char under it — the end of a row clicked past its end, the
    /// text's end below the last row, and a step past either end of a
    /// one-line field's window clicked beside it.
    fn point(&self, placed: Placed, at: Position) -> usize {
        let area = placed.area;
        let col = usize::from(at.x.saturating_sub(area.x));
        let chars = match placed.shape {
            Shape::Line { first } if at.x < area.x => first.saturating_sub(1),
            Shape::Line { first } => (first + col).min(self.text.chars().count()),
            Shape::Rows { top } => {
                let rows = self.rows(area.width.into());
                let row = usize::from(top) + usize::from(at.y.saturating_sub(area.y));
                if row < rows.len() {
                    spot_in_row(&rows, row, col)
                } else {
                    self.text.chars().count()
                }
            }
        };
        self.byte_of(chars)
    }

    /// Type `c` at the caret — over the SELECTION, when there is one.
    pub fn insert_char(&mut self, c: char) {
        self.delete_selection();
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Insert a whole run at the cursor — a bracketed paste — over the
    /// SELECTION, when there is one. A multi-row field keeps its line
    /// breaks (`\r\n` and a bare `\r` become `\n`); a one-line field
    /// turns each into a space.
    pub fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
        let run = if self.multiline {
            normalized
        } else {
            normalized.replace('\n', " ")
        };
        self.text.insert_str(self.cursor, &run);
        self.cursor += run.len();
        self.goal = None;
    }

    /// [`insert_str`](Self::insert_str) with the caret left where it was,
    /// before the run: text that belongs after what is typed next.
    pub fn insert_after_caret(&mut self, s: &str) {
        self.delete_selection();
        let at = self.cursor;
        self.insert_str(s);
        self.cursor = at;
    }

    /// Apply one key press. Returns [`Edit::Ignored`] for anything that
    /// isn't an editing key, leaving it for the caller.
    pub fn handle_key(&mut self, key: &KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // Shift on a motion key extends the SELECTION rather than moving.
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // Cmd on macOS, when a terminal delivers it at all: line-wise.
        let cmd = key
            .modifiers
            .intersects(KeyModifiers::SUPER | KeyModifiers::META | KeyModifiers::HYPER);
        // A run of ↑/↓ (PageUp/PageDown) keeps aiming for the column it
        // set out from; any other key starts afresh — and so does a plain
        // ↑/↓ that lets a selection go, setting out from its edge.
        let goal = self.goal.take().filter(|_| shift || self.anchor.is_none());

        match key.code {
            // ---- line breaks (multi-row fields only) ----
            _ if self.takes_newline(key) => {
                self.insert_char('\n');
                Edit::Changed
            }

            // ---- selection ----
            // ⌘A, where the terminal hands ⌘ over (Ghostty, once the
            // GHOSTTY KEYBINDS block releases it from its select-all).
            // Never Ctrl+A: that is the line's start, and what Ghostty
            // types for ⌘←.
            KeyCode::Char('a' | 'A') if cmd && !ctrl && !alt => {
                self.select_all();
                Edit::Moved
            }
            // ⌘C copies the SELECTION and keeps it; ⌘X cuts it. With
            // nothing selected both stay the caller's — a finder's ⌘C
            // copies the path under its cursor.
            KeyCode::Char(c @ ('c' | 'C' | 'x' | 'X'))
                if cmd && !ctrl && !alt && self.selection().is_some() =>
            {
                let text = self.selected().map(str::to_string);
                COPIED.with(|copied| *copied.borrow_mut() = text);
                if c.eq_ignore_ascii_case(&'x') {
                    self.delete_selection();
                    Edit::Changed
                } else {
                    Edit::Moved
                }
            }

            // ---- motion ----
            // Line-wise keys work on the line under the caret — the whole
            // text, in a one-line field. Each extends the SELECTION with
            // Shift held and lets it go without.
            KeyCode::Left if cmd => self.motion(shift, true, |s| s.move_to(s.line_start(s.cursor))),
            KeyCode::Left if alt || ctrl => {
                self.motion(shift, true, |s| s.move_to(s.word_left(s.cursor)))
            }
            KeyCode::Left => self.step_char(shift, true),
            KeyCode::Right if cmd => self.motion(shift, false, |s| s.move_to(s.line_end(s.cursor))),
            KeyCode::Right if alt || ctrl => {
                self.motion(shift, false, |s| s.move_to(s.word_right(s.cursor)))
            }
            KeyCode::Right => self.step_char(shift, false),
            KeyCode::Home if ctrl || cmd => self.motion(shift, true, |s| s.move_to(0)),
            KeyCode::End if ctrl || cmd => self.motion(shift, false, |s| s.move_to(s.text.len())),
            KeyCode::Home => self.motion(shift, true, |s| s.move_to(s.line_start(s.cursor))),
            KeyCode::End => self.motion(shift, false, |s| s.move_to(s.line_end(s.cursor))),
            // ↑/↓ walk a multi-row field's rows as drawn — a paragraph
            // wrapped over five rows is five rows — keeping the column;
            // past the first or last row they are the caller's (a form
            // steps to its next field). ⌥↑/⌥↓ jump to the start or end of
            // the paragraph, then the one before or after, as a macOS text
            // view does; Cmd+↑/↓ (Ctrl+Home/End) to the text's very start
            // or end; PageUp/PageDown a screenful. A one-line field has no
            // second row: all of these stay the caller's there, shifted
            // or not.
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
                if !self.multiline =>
            {
                Edit::Ignored
            }
            KeyCode::Up if cmd => self.motion(shift, true, |s| s.move_to(0)),
            KeyCode::Down if cmd => self.motion(shift, false, |s| s.move_to(s.text.len())),
            KeyCode::Up if alt => self.motion(shift, true, Self::paragraph_up),
            KeyCode::Down if alt => self.motion(shift, false, Self::paragraph_down),
            // ⇧↑ on the first row (⇧↓ on the last) selects on to the very
            // start (end), as a macOS text view does, rather than handing
            // the key to the caller with half a selection made.
            KeyCode::Up if shift => self.motion(true, true, |s| match s.move_rows(-1, goal) {
                Edit::Ignored => s.move_to(0),
                moved => moved,
            }),
            KeyCode::Down if shift => self.motion(true, false, |s| match s.move_rows(1, goal) {
                Edit::Ignored => s.move_to(s.text.len()),
                moved => moved,
            }),
            KeyCode::Up => self.motion(false, true, |s| s.move_rows(-1, goal)),
            KeyCode::Down => self.motion(false, false, |s| s.move_rows(1, goal)),
            KeyCode::PageUp => self.motion(shift, true, |s| s.page(-1, goal)),
            KeyCode::PageDown => self.motion(shift, false, |s| s.page(1, goal)),

            // ---- deletion ----
            // Cmd+⌫ kills the line, ⌥⌫ / Ctrl+⌫ the previous word — and
            // every one of them just the SELECTION, when there is one.
            KeyCode::Backspace if cmd => self.kill(self.line_start(self.cursor), self.cursor),
            KeyCode::Backspace if alt || ctrl => {
                self.kill(self.word_left(self.cursor), self.cursor)
            }
            KeyCode::Backspace => self.kill(self.prev_boundary(self.cursor), self.cursor),
            KeyCode::Delete if cmd => self.kill(self.cursor, self.line_end(self.cursor)),
            KeyCode::Delete if alt || ctrl => self.kill(self.cursor, self.word_right(self.cursor)),
            KeyCode::Delete => self.kill(self.cursor, self.next_boundary(self.cursor)),

            // ---- readline chords ----
            // Cmd+key never means "type this" — leave it to the caller.
            KeyCode::Char(_) if cmd => Edit::Ignored,
            KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
                'a' => self.motion(false, true, |s| s.move_to(s.line_start(s.cursor))),
                'e' => self.motion(false, false, |s| s.move_to(s.line_end(s.cursor))),
                'b' => self.step_char(false, true),
                'f' => self.step_char(false, false),
                'd' => self.kill(self.cursor, self.next_boundary(self.cursor)),
                'w' => self.kill(self.word_left(self.cursor), self.cursor),
                'u' => self.kill(self.line_start(self.cursor), self.cursor),
                // To the end of the line — or, standing at its end, the
                // line break itself, as readline does.
                'k' => match self.line_end(self.cursor) {
                    end if end == self.cursor => {
                        self.kill(self.cursor, self.next_boundary(self.cursor))
                    }
                    end => self.kill(self.cursor, end),
                },
                _ => Edit::Ignored,
            },
            // ⌥b/⌥f are what macOS terminals send for ⌥←/⌥→ — shifted
            // (`ESC B` / `ESC F`), they select; ⌥d is readline's
            // kill-word-forward.
            KeyCode::Char(c) if alt => {
                let select = shift || c.is_ascii_uppercase();
                match c.to_ascii_lowercase() {
                    'b' => self.motion(select, true, |s| s.move_to(s.word_left(s.cursor))),
                    'f' => self.motion(select, false, |s| s.move_to(s.word_right(s.cursor))),
                    'd' => self.kill(self.cursor, self.word_right(self.cursor)),
                    // Some emulators send ⌥⌫ as ESC + DEL rather than a
                    // modified Backspace key.
                    '\u{7f}' | '\u{8}' => self.kill(self.word_left(self.cursor), self.cursor),
                    _ => Edit::Ignored,
                }
            }

            // ---- text ----
            // Plain (or shifted) printable keys, including the glyphs a Mac
            // makes from ⌥-letters when the profile isn't option-as-meta.
            KeyCode::Char(c) => {
                self.insert_char(c);
                Edit::Changed
            }
            _ => Edit::Ignored,
        }
    }

    // ---- internals ----

    fn move_to(&mut self, at: usize) -> Edit {
        self.cursor = at;
        Edit::Moved
    }

    /// One motion key, `step` being where it takes the caret. With
    /// `select` the SELECTION grows or shrinks with it, anchored where it
    /// began — or at the caret, starting one. Without, a selection is let
    /// go first, the caret setting out from its edge `toward_start` or the
    /// other; a key that then has nowhere to go still counts as Moved, the
    /// selection having gone.
    fn motion(
        &mut self,
        select: bool,
        toward_start: bool,
        step: impl FnOnce(&mut Self) -> Edit,
    ) -> Edit {
        if select {
            let anchor = self.anchor.unwrap_or(self.cursor);
            let edit = step(self);
            self.anchor = (anchor != self.cursor).then_some(anchor);
            return edit;
        }
        let collapsed = match self.selection() {
            Some((start, end)) => {
                self.cursor = if toward_start { start } else { end };
                true
            }
            None => false,
        };
        self.anchor = None;
        match step(self) {
            Edit::Ignored if collapsed => Edit::Moved,
            edit => edit,
        }
    }

    /// ←/→ (Ctrl+B/F): a character over, or with `select` the SELECTION
    /// one wider or narrower. Plain, over a selection, the caret lands on
    /// its near edge and goes no further, as in any macOS text field.
    fn step_char(&mut self, select: bool, back: bool) -> Edit {
        if !select {
            if let Some((start, end)) = self.selection() {
                self.anchor = None;
                return self.move_to(if back { start } else { end });
            }
        }
        self.motion(select, back, |s| {
            let to = if back {
                s.prev_boundary(s.cursor)
            } else {
                s.next_boundary(s.cursor)
            };
            s.move_to(to)
        })
    }

    /// A delete key's cut, `start..end` — or the SELECTION in its place,
    /// when there is one: ⌫, ⌥⌫ and ^U alike take just what is selected.
    fn kill(&mut self, start: usize, end: usize) -> Edit {
        if self.delete_selection() {
            return Edit::Changed;
        }
        self.delete(start, end)
    }

    fn delete(&mut self, start: usize, end: usize) -> Edit {
        self.anchor = None;
        if start >= end {
            // Backspace at column 0 is still the field's key — swallow it so
            // an overlay doesn't read it as "delete the selected row".
            return Edit::Moved;
        }
        self.text.replace_range(start..end, "");
        self.cursor = start;
        Edit::Changed
    }

    fn char_before(&self, at: usize) -> Option<char> {
        self.text[..at].chars().next_back()
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.text[at..].chars().next()
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.char_before(at).map_or(at, |c| at - c.len_utf8())
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.char_at(at).map_or(at, |c| at + c.len_utf8())
    }

    /// Start of the line `at` sits on: just past the previous line break,
    /// or 0 — always 0 in a one-line field.
    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }

    /// End of the line `at` sits on: its line break, or the end of the
    /// text — always the end in a one-line field.
    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i)
    }

    /// The caret `col` chars into `row` ([`spot_in_row`]).
    fn land(&mut self, rows: &[(usize, usize)], row: usize, col: usize) {
        self.set_cursor_chars(spot_in_row(rows, row, col));
    }

    /// The caret `delta` rows up (−) or down, as last drawn, at the goal
    /// column — clamped to the first and last rows, and [`Edit::Ignored`]
    /// when it already stands there, so the caller can act on the key.
    fn move_rows(&mut self, delta: isize, goal: Option<u16>) -> Edit {
        let rows = self.rows(self.view.width.into());
        let row = self.caret_row(&rows);
        let target = row.saturating_add_signed(delta).min(rows.len() - 1);
        if target == row {
            self.goal = goal;
            return Edit::Ignored;
        }
        let col = goal.map_or(self.cursor_chars() - rows[row].0, usize::from);
        self.land(&rows, target, col);
        self.goal = Some(cells(col));
        Edit::Moved
    }

    /// PageUp/PageDown: a screenful of rows at the goal column, keeping
    /// one row of the old screen in sight, the box scrolling with the
    /// caret; from the first or last row, the very start or end of the
    /// text.
    fn page(&mut self, dir: isize, goal: Option<u16>) -> Edit {
        let rows = self.rows(self.view.width.into());
        let before = self.caret_row(&rows) as isize;
        let step = self.view.height.saturating_sub(1).max(1) as isize;
        match self.move_rows(dir * step, goal) {
            Edit::Ignored => {
                self.goal = None;
                self.move_to(if dir < 0 { 0 } else { self.text.len() })
            }
            moved => {
                let after = self.caret_row(&rows) as isize;
                let top = usize::from(self.view.top).saturating_add_signed(after - before);
                self.view.top = cells(top);
                moved
            }
        }
    }

    /// ⌥↑: to the start of the paragraph — the hard line — the caret is
    /// in, or from its start to the previous paragraph's. [`Edit::Ignored`]
    /// at the very start.
    fn paragraph_up(&mut self) -> Edit {
        if self.cursor == 0 {
            return Edit::Ignored;
        }
        let start = self.line_start(self.cursor);
        if start < self.cursor {
            return self.move_to(start);
        }
        self.move_to(self.line_start(start - 1))
    }

    /// ⌥↓: to the end of the paragraph the caret is in, or from its end
    /// to the next paragraph's. [`Edit::Ignored`] at the very end.
    fn paragraph_down(&mut self) -> Edit {
        if self.cursor == self.text.len() {
            return Edit::Ignored;
        }
        let end = self.line_end(self.cursor);
        if end > self.cursor {
            return self.move_to(end);
        }
        self.move_to(self.line_end(end + 1))
    }

    /// Start of the word at or before `at`: skip back over separators, then
    /// over the word itself (readline's `backward-word`).
    fn word_left(&self, mut at: usize) -> usize {
        while self.char_before(at).is_some_and(|c| !is_word(c)) {
            at = self.prev_boundary(at);
        }
        while self.char_before(at).is_some_and(is_word) {
            at = self.prev_boundary(at);
        }
        at
    }

    /// End of the word at or after `at` (readline's `forward-word`).
    fn word_right(&self, mut at: usize) -> usize {
        while self.char_at(at).is_some_and(|c| !is_word(c)) {
            at = self.next_boundary(at);
        }
        while self.char_at(at).is_some_and(is_word) {
            at = self.next_boundary(at);
        }
        at
    }
}

/// Read access is just `&str`, so every `is_empty()` / `chars()` / `trim()`
/// call site — and every `&str` argument — keeps working unchanged.
impl Deref for TextInput {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for TextInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<String> for TextInput {
    fn from(text: String) -> Self {
        Self::with_text(text)
    }
}

impl From<&str> for TextInput {
    fn from(text: &str) -> Self {
        Self::with_text(text)
    }
}

impl PartialEq<str> for TextInput {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

impl PartialEq<&str> for TextInput {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl PartialEq<String> for TextInput {
    fn eq(&self, other: &String) -> bool {
        &self.text == other
    }
}

/// The line editor's keys as Help spells them ("TYPING IN A FIELD"):
/// [`TextInput::handle_key`]'s own arms answer every one, which the test
/// beside them presses to prove.
pub mod keys {
    use crate::hints::Key;

    pub const WORD: Key = Key::new(&["alt+left", "alt+right"], "move by word").show(2);
    pub const LINE_ENDS: Key = Key::new(&["ctrl+a", "ctrl+e"], "start / end of line").show(2);
    pub const DELETE_WORD: Key = Key::new(&["alt+backspace"], "delete a word");
    pub const KILL: Key = Key::new(&["ctrl+u", "ctrl+k"], "kill to start / end").show(2);
    /// The SELECTION, a character at a time. ⇧↑/⇧↓ select by row too, but
    /// only in a multi-row box — a one-line field leaves them to the list
    /// under it — so Help, which speaks for every field, names just these.
    pub const SELECT: Key = Key::new(&["shift+left", "shift+right"], "select").show(2);
    /// By word: what Ghostty sends for ⌥⇧←/⌥⇧→, which it binds to nothing.
    pub const SELECT_WORD: Key =
        Key::new(&["alt+shift+left", "alt+shift+right"], "select by word").show(2);
    /// To the line's ends: ⌘⇧←/⌘⇧→ where ⌘ arrives (Ghostty binds neither,
    /// unlike plain ⌘←/⌘→), ⇧Home/⇧End everywhere.
    pub const SELECT_LINE: Key = Key::new(
        &[
            "cmd+shift+left",
            "cmd+shift+right",
            "shift+home",
            "shift+end",
        ],
        "select to start / end",
    )
    .show(2);
    /// ⌘A — never `^A`, which is the line's start and what Ghostty types
    /// for ⌘←. Ghostty keeps ⌘A for its own select-all until the GHOSTTY
    /// KEYBINDS block releases it (`ghostty_config::EDITOR_CHORDS`).
    pub const SELECT_ALL: Key = Key::new(&["cmd+a"], "select all");
    /// ⌘C / ⌘X on the SELECTION. Ghostty answers ⌘C itself only while
    /// its own (mouse) selection exists — the GHOSTTY KEYBINDS block makes
    /// its copy `performable` — and releases ⌘X outright.
    pub const COPY_CUT: Key = Key::new(&["cmd+c", "cmd+x"], "copy / cut").show(2);
    pub const ALL: &[Key] = &[
        WORD,
        LINE_ENDS,
        DELETE_WORD,
        KILL,
        SELECT,
        SELECT_WORD,
        SELECT_LINE,
        SELECT_ALL,
        COPY_CUT,
    ];
}

/// Test-only accessors: nothing in the app reads these any more.
#[cfg(test)]
impl TextInput {
    /// Where the field was last drawn.
    pub fn view(&self) -> TextView {
        self.view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chord Help names for the line editor is one it answers:
    /// pressed mid-word in a one-line field, each moves the caret, selects,
    /// deletes or — over a selection — copies.
    #[test]
    fn every_key_help_names_is_the_editors() {
        for key in keys::ALL {
            assert!(key.parses(), "{:?}", key.chords);
            for chord in key.chords() {
                let mut input = TextInput::new();
                input.set_text("one two three");
                input.handle_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
                input.handle_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
                if key.chords == keys::COPY_CUT.chords {
                    input.handle_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
                }
                let state = |input: &TextInput| {
                    (
                        input.as_str().to_string(),
                        input.cursor_chars(),
                        input.selection_chars(),
                    )
                };
                let before = state(&input);
                let edit = input.handle_key(&KeyEvent::new(chord.code, chord.mods));
                let copied = take_copied().is_some();
                assert!(
                    edit.consumed() && (before != state(&input) || copied),
                    "{chord} did nothing"
                );
            }
        }
    }

    /// ⌘C copies the SELECTION and keeps it; ⌘X cuts it, the caret where
    /// it began. With nothing selected both are the caller's, nothing
    /// copied.
    #[test]
    fn cmd_c_copies_and_cmd_x_cuts_the_selection() {
        let cmd = KeyModifiers::SUPER;
        let mut input = typed("keep this cut");
        assert_eq!(press(&mut input, KeyCode::Char('c'), cmd), Edit::Ignored);
        assert_eq!(press(&mut input, KeyCode::Char('x'), cmd), Edit::Ignored);
        assert_eq!(take_copied(), None);

        for _ in 0.."cut".len() {
            press(&mut input, KeyCode::Left, KeyModifiers::SHIFT);
        }
        assert_eq!(press(&mut input, KeyCode::Char('c'), cmd), Edit::Moved);
        assert_eq!(take_copied().as_deref(), Some("cut"));
        assert_eq!(take_copied(), None, "taken once");
        assert_eq!(input.selected(), Some("cut"), "a copy keeps the selection");

        assert_eq!(press(&mut input, KeyCode::Char('x'), cmd), Edit::Changed);
        assert_eq!(take_copied().as_deref(), Some("cut"));
        assert_eq!(input.as_str(), "keep this ");
        assert_eq!(input.cursor_chars(), "keep this ".len());
        assert_eq!(input.selected(), None);

        // ⌘A then ⌘X empties a multi-row box, its line breaks and all.
        let mut many = TextInput::multiline_with_text("one\ntwo");
        press(&mut many, KeyCode::Char('a'), cmd);
        press(&mut many, KeyCode::Char('x'), cmd);
        assert_eq!(take_copied().as_deref(), Some("one\ntwo"));
        assert!(many.as_str().is_empty());
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// Type `s` a character at a time, as the event loop would.
    fn typed(s: &str) -> TextInput {
        let mut input = TextInput::new();
        for c in s.chars() {
            input.handle_key(&key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        input
    }

    fn press(input: &mut TextInput, code: KeyCode, mods: KeyModifiers) -> Edit {
        input.handle_key(&key(code, mods))
    }

    #[test]
    fn typing_appends_and_tracks_the_cursor() {
        let input = typed("hello");
        assert_eq!(input.as_str(), "hello");
        assert_eq!(input.cursor_chars(), 5);
    }

    #[test]
    fn arrows_move_and_typing_inserts_at_the_cursor() {
        let mut input = typed("hello");
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), 3);
        assert_eq!(
            press(&mut input, KeyCode::Char('X'), KeyModifiers::NONE),
            Edit::Changed
        );
        assert_eq!(input.as_str(), "helXlo");
        assert_eq!(input.cursor_chars(), 4);
    }

    #[test]
    fn backspace_deletes_before_the_cursor_only() {
        let mut input = typed("hello");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        press(&mut input, KeyCode::Right, KeyModifiers::NONE);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "ello");
        assert_eq!(input.cursor_chars(), 0);
        // At column 0 it is still the field's key — consumed, not passed on.
        assert_eq!(
            press(&mut input, KeyCode::Backspace, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(input.as_str(), "ello");
    }

    #[test]
    fn delete_removes_forward() {
        let mut input = typed("hello");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        press(&mut input, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "ello");
        press(&mut input, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "llo");
    }

    /// What ⌥← actually sends on macOS: ESC b, i.e. crossterm's Alt+b.
    #[test]
    fn option_arrows_arrive_as_alt_b_and_alt_f() {
        let mut input = typed("fix the login redirect");
        assert_eq!(
            press(&mut input, KeyCode::Char('b'), KeyModifiers::ALT),
            Edit::Moved
        );
        assert_eq!(input.cursor_chars(), "fix the login ".len());
        press(&mut input, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "fix the ".len());
        press(&mut input, KeyCode::Char('f'), KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "fix the login".len());
    }

    #[test]
    fn alt_and_ctrl_arrows_move_by_word_too() {
        let mut input = typed("one two three");
        press(&mut input, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "one two ".len());
        press(&mut input, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), "one ".len());
        press(&mut input, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "one two".len());
    }

    #[test]
    fn word_motion_treats_punctuation_as_a_separator() {
        let mut input = typed("~/src/orion-tui/app.rs");
        press(&mut input, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "~/src/orion-tui/app.".len());
        press(&mut input, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(input.cursor_chars(), "~/src/orion-tui/".len());
    }

    #[test]
    fn word_and_line_deletes() {
        let mut input = typed("one two three");
        assert_eq!(
            press(&mut input, KeyCode::Char('w'), KeyModifiers::CONTROL),
            Edit::Changed
        );
        assert_eq!(input.as_str(), "one two ");
        press(&mut input, KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(input.as_str(), "one ");
        press(&mut input, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "");
    }

    #[test]
    fn ctrl_k_kills_to_the_end_and_ctrl_a_e_jump() {
        let mut input = typed("keep this cut this");
        press(&mut input, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), 0);
        for _ in 0.."keep this ".len() {
            press(&mut input, KeyCode::Char('f'), KeyModifiers::CONTROL);
        }
        press(&mut input, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "keep this ");
        press(&mut input, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), 10);
    }

    #[test]
    fn alt_d_kills_the_word_ahead() {
        let mut input = typed("alpha beta");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        press(&mut input, KeyCode::Char('d'), KeyModifiers::ALT);
        assert_eq!(input.as_str(), " beta");
    }

    #[test]
    fn multibyte_text_moves_by_whole_characters() {
        let mut input = typed("héllo→");
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "héll→");
        assert_eq!(input.cursor_chars(), 4);
    }

    #[test]
    fn unknown_keys_are_left_to_the_caller() {
        let mut input = typed("x");
        assert_eq!(
            press(&mut input, KeyCode::Enter, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut input, KeyCode::Esc, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut input, KeyCode::Tab, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut input, KeyCode::Char('n'), KeyModifiers::CONTROL),
            Edit::Ignored
        );
        assert_eq!(input.as_str(), "x");
    }

    /// One paste, two fields: the one-line field spaces out every kind of
    /// line break, the multi-row field keeps them all as `\n`.
    #[test]
    fn a_paste_keeps_its_lines_only_in_a_multi_row_field() {
        let mut one = typed("ab");
        press(&mut one, KeyCode::Left, KeyModifiers::NONE);
        one.insert_str("one\r\ntwo\rthree");
        assert_eq!(one.as_str(), "aone two threeb");
        assert_eq!(one.cursor_chars(), 14);

        let mut many = TextInput::multiline_with_text("ab");
        press(&mut many, KeyCode::Left, KeyModifiers::NONE);
        many.insert_str("one\r\ntwo\rthree");
        assert_eq!(many.as_str(), "aone\ntwo\nthreeb");
        assert_eq!(many.cursor_chars(), 14);
    }

    /// The three chords Claude Code's prompt breaks a line on — the kitty
    /// protocol's Shift+Enter, the `ESC` `CR` (Alt+Enter) a mapped
    /// Shift+Enter or Option+Enter sends, and Ctrl+J — all break one
    /// here; a plain Enter is still the caller's.
    #[test]
    fn a_multi_row_field_breaks_lines_three_ways() {
        let mut input = TextInput::multiline();
        for c in "one".chars() {
            press(&mut input, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(
            press(&mut input, KeyCode::Enter, KeyModifiers::SHIFT),
            Edit::Changed
        );
        for c in "two".chars() {
            press(&mut input, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(
            press(&mut input, KeyCode::Enter, KeyModifiers::ALT),
            Edit::Changed
        );
        for c in "three".chars() {
            press(&mut input, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(
            press(&mut input, KeyCode::Char('j'), KeyModifiers::CONTROL),
            Edit::Changed
        );
        assert_eq!(input.as_str(), "one\ntwo\nthree\n");
        assert_eq!(
            press(&mut input, KeyCode::Enter, KeyModifiers::NONE),
            Edit::Ignored,
            "Enter is the caller's send or save"
        );
        assert!(input.takes_newline(&key(KeyCode::Enter, KeyModifiers::SHIFT)));
        assert!(!input.takes_newline(&key(KeyCode::Enter, KeyModifiers::NONE)));
    }

    /// A one-line field has no line to break: the chords are recognized
    /// (a form can still act on them) but left to the caller untouched.
    #[test]
    fn a_one_line_field_leaves_the_break_keys_to_the_caller() {
        let mut input = typed("one");
        for (code, mods) in [
            (KeyCode::Enter, KeyModifiers::SHIFT),
            (KeyCode::Enter, KeyModifiers::ALT),
            (KeyCode::Char('j'), KeyModifiers::CONTROL),
        ] {
            assert!(TextInput::is_newline_key(&key(code, mods)), "{code:?}");
            assert!(!input.takes_newline(&key(code, mods)), "{code:?}");
            assert_eq!(press(&mut input, code, mods), Edit::Ignored, "{code:?}");
        }
        assert_eq!(input.as_str(), "one");
        assert!(!TextInput::is_newline_key(&key(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
    }

    /// ↑/↓ walk the lines keeping the column — clamped on a shorter line,
    /// and back to it past one — and past the first or last line they are
    /// Ignored, so a form can step to its next field on the very same key.
    #[test]
    fn arrows_walk_the_lines_and_fall_through_at_the_ends() {
        let mut input = TextInput::multiline_with_text("first line\nhi\nthird");
        // From the end of "third" (column 5): "hi" is shorter, so its end.
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(input.cursor_chars(), "first line\nhi".len());
        // Still aiming for column 5 on the first line, not hi's 2.
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!(input.cursor_chars(), "first".len());
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored,
            "no line above the first"
        );
        assert_eq!(input.cursor_chars(), "first".len(), "the caret stays put");
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "first line\nhi\nthird".len());
        assert_eq!(
            press(&mut input, KeyCode::Down, KeyModifiers::NONE),
            Edit::Ignored,
            "no line below the last"
        );
        // Any other key lets the column go: from "fi|rst", ↓ lands at 2.
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        press(&mut input, KeyCode::Right, KeyModifiers::NONE);
        press(&mut input, KeyCode::Right, KeyModifiers::NONE);
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        press(&mut input, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "first line\nhi\nth".len());
        // A one-line field never walks: still the caller's keys.
        let mut one = typed("solo");
        assert_eq!(
            press(&mut one, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored
        );
        assert_eq!(
            press(&mut one, KeyCode::Down, KeyModifiers::NONE),
            Edit::Ignored
        );
    }

    /// Home/End, Ctrl+A/E and the line kills act on the line under the
    /// caret, and Ctrl+K at a line's end joins it to the next.
    #[test]
    fn line_keys_work_on_the_line_under_the_caret() {
        let mut input = TextInput::multiline_with_text("keep this\ncut here");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "keep this\n".len());
        press(&mut input, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), "keep this\ncut here".len());
        press(&mut input, KeyCode::Char('a'), KeyModifiers::CONTROL);
        press(&mut input, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(
            input.as_str(),
            "keep this\n",
            "^K kills the second line only"
        );
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        press(&mut input, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), "keep this".len());
        press(&mut input, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "keep this", "^K at the end eats the break");
        let mut input = TextInput::multiline_with_text("one\ntwo three");
        press(&mut input, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(input.as_str(), "one\n", "^U stops at the line's start");
        assert_eq!(input.cursor_chars(), 4);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "one", "⌫ at a line's start joins it");
    }

    /// A multi-row field as a `width` × `height` box last drew it, caret
    /// at the end.
    fn drawn(text: &str, width: u16, height: u16) -> TextInput {
        let mut input = TextInput::multiline_with_text(text);
        let view = input.view_for(width, height);
        input.set_view(view);
        input
    }

    /// The row the caret is drawn on, in the layout the box last used.
    fn row_of(input: &TextInput) -> usize {
        input.caret_row(&input.rows(input.view().width.into()))
    }

    const FOX: &str = "the quick brown fox jumps over the lazy dog";

    /// Ten columns wide the fox wraps into `the quick |brown fox |jumps
    /// |over the |lazy dog`.
    #[test]
    fn rows_wrap_after_the_last_space_that_fits() {
        let input = TextInput::multiline_with_text(FOX);
        assert_eq!(
            input.rows(10),
            vec![(0, 10), (10, 20), (20, 26), (26, 35), (35, 43)]
        );
        assert_eq!(input.rows(0), vec![(0, 43)], "width 0 wraps nothing");
        let hard = TextInput::multiline_with_text("one\n\nabcdefghij");
        assert_eq!(
            hard.rows(4),
            vec![(0, 3), (4, 4), (5, 9), (9, 13), (13, 15)],
            "a word wider than the row breaks mid-word"
        );
    }

    /// The prompt nobody pressed Shift+Enter in is ONE line: ↑/↓ must walk
    /// the rows it wraps into as drawn, keeping the column, or there is no
    /// way up it but ←.
    #[test]
    fn arrows_walk_the_rows_a_long_paragraph_wraps_into() {
        let mut input = drawn(FOX, 10, 5);
        assert_eq!(row_of(&input), 4);
        // Column 8 all the way up; "jumps " stops at its space.
        let ups = [34, 25, 18, 8];
        for want in ups {
            assert_eq!(
                press(&mut input, KeyCode::Up, KeyModifiers::NONE),
                Edit::Moved
            );
            assert_eq!(input.cursor_chars(), want);
        }
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored,
            "the top row is the caller's"
        );
        for want in [18, 25, 34, 43] {
            press(&mut input, KeyCode::Down, KeyModifiers::NONE);
            assert_eq!(input.cursor_chars(), want);
        }
        // Never drawn, the paragraph is one row, as it always was.
        let mut undrawn = TextInput::multiline_with_text(FOX);
        assert_eq!(
            press(&mut undrawn, KeyCode::Up, KeyModifiers::NONE),
            Edit::Ignored
        );
    }

    /// ⌥↑/⌥↓ — what the user reached for — jump by paragraph as a macOS
    /// text view does: its start (end), then the one before (after).
    #[test]
    fn option_arrows_jump_by_paragraph() {
        let text = "alpha beta\ngamma\n\ndelta";
        let mut input = drawn(text, 80, 5);
        for want in [18, 17, 11, 0] {
            assert_eq!(
                press(&mut input, KeyCode::Up, KeyModifiers::ALT),
                Edit::Moved
            );
            assert_eq!(input.cursor_chars(), want);
        }
        assert_eq!(
            press(&mut input, KeyCode::Up, KeyModifiers::ALT),
            Edit::Ignored
        );
        for want in [10, 16, 17, 23] {
            press(&mut input, KeyCode::Down, KeyModifiers::ALT);
            assert_eq!(input.cursor_chars(), want);
        }
        assert_eq!(
            press(&mut input, KeyCode::Down, KeyModifiers::ALT),
            Edit::Ignored
        );
        let mut one = typed("solo");
        assert_eq!(
            press(&mut one, KeyCode::Up, KeyModifiers::ALT),
            Edit::Ignored,
            "a one-line field leaves it to the caller"
        );
    }

    #[test]
    fn cmd_arrows_and_ctrl_home_end_reach_the_ends_of_the_text() {
        let mut input = drawn("one\ntwo\nthree", 80, 5);
        press(&mut input, KeyCode::Home, KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), 0);
        press(&mut input, KeyCode::End, KeyModifiers::CONTROL);
        assert_eq!(input.cursor_chars(), 13);
        press(&mut input, KeyCode::Up, KeyModifiers::SUPER);
        assert_eq!(input.cursor_chars(), 0);
        press(&mut input, KeyCode::Down, KeyModifiers::SUPER);
        assert_eq!(input.cursor_chars(), 13);
    }

    fn twenty_lines() -> String {
        (0..20)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// PageUp/PageDown move a screenful less one row, the box going with
    /// the caret, and from the first or last row to the text's very end.
    #[test]
    fn page_keys_move_a_screenful() {
        let mut input = drawn(&twenty_lines(), 20, 5);
        assert_eq!((row_of(&input), input.view().top), (19, 15));
        for (row, top) in [(15, 11), (11, 7), (7, 3), (3, 0), (0, 0)] {
            assert_eq!(
                press(&mut input, KeyCode::PageUp, KeyModifiers::NONE),
                Edit::Moved
            );
            let view = input.view_for(20, 5);
            input.set_view(view);
            assert_eq!((row_of(&input), view.top), (row, top));
        }
        assert_eq!(input.cursor_chars(), 2, "column 3 clamped on l0");
        press(&mut input, KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(input.cursor_chars(), 0, "the first row's page is the start");
        press(&mut input, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(
            input.cursor_chars(),
            "l0\nl1\nl2\nl3\n".len(),
            "l4, column 0"
        );
    }

    /// The box scrolls only when the caret would leave it — ↑/↓ inside it
    /// leave it still, rather than recentring on every press.
    #[test]
    fn the_view_moves_only_to_keep_the_caret_in_sight() {
        let mut input = drawn(&twenty_lines(), 20, 5);
        for _ in 0..4 {
            press(&mut input, KeyCode::Up, KeyModifiers::NONE);
            let view = input.view_for(20, 5);
            assert_eq!(view.top, 15, "caret on row {}", row_of(&input));
            input.set_view(view);
        }
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(input.view_for(20, 5).top, 14);
        // A box taller than the text shows it all, from the top.
        assert_eq!(input.view_for(20, 30).top, 0);
    }

    /// The wheel scrolls the box, pulling the caret along only when it
    /// would drop out of sight.
    #[test]
    fn the_wheel_scrolls_and_drags_the_caret_into_view() {
        let mut input = drawn(&twenty_lines(), 20, 5);
        assert_eq!(input.scroll_rows(-3), Edit::Moved);
        assert_eq!((input.view().top, row_of(&input)), (12, 16));
        assert_eq!(input.scroll_rows(1), Edit::Moved);
        assert_eq!(
            (input.view().top, row_of(&input)),
            (13, 16),
            "still in sight: the caret stays"
        );
        input.scroll_rows(-100);
        assert_eq!((input.view().top, row_of(&input)), (0, 4));
        assert_eq!(input.scroll_rows(-1), Edit::Ignored);
    }

    /// A multi-row field drawn into the `width` × `height` box at the
    /// screen's top left, as a renderer records it.
    fn placed(text: &str, width: u16, height: u16) -> TextInput {
        let input = drawn(text, width, height);
        place_rows(&input, Rect::new(0, 0, width, height), input.view().top);
        input
    }

    /// A one-line field drawn `width` cells wide from column 2 of row 5,
    /// from where it would scroll to.
    fn placed_line(text: &str, width: u16) -> TextInput {
        let input = TextInput::with_text(text);
        place_line(
            &input,
            Rect::new(2, 5, width, 1),
            input.line_first(width.into()),
        );
        input
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    const LEFT: MouseButton = MouseButton::Left;

    /// A click — press and release — at a screen cell, `at` that instant.
    fn click_at(input: &mut TextInput, column: u16, row: u16, at: Instant) -> Edit {
        let edit = input.mouse_at(&mouse(MouseEventKind::Down(LEFT), column, row), at);
        input.mouse_at(&mouse(MouseEventKind::Up(LEFT), column, row), at);
        edit
    }

    fn click(input: &mut TextInput, column: u16, row: u16) -> Edit {
        click_at(input, column, row, Instant::now())
    }

    fn drag(input: &mut TextInput, column: u16, row: u16) -> Edit {
        input.mouse(&mouse(MouseEventKind::Drag(LEFT), column, row))
    }

    /// A click puts the caret where it points in the rows as drawn — past
    /// a row's end at that end, below the last row at the text's end.
    #[test]
    fn a_click_lands_the_caret_where_it_points() {
        let mut input = placed(FOX, 10, 3);
        assert_eq!(input.view().top, 2, "rows 2-4 in sight");
        click(&mut input, 2, 0);
        assert_eq!(input.cursor_chars(), 22, "ju|mps");
        click(&mut input, 9, 0);
        assert_eq!(input.cursor_chars(), 25, "the end of `jumps `");
        // Below the box is not the field's; its last row's end is.
        assert_eq!(click(&mut input, 0, 9), Edit::Ignored);
        click(&mut input, 9, 2);
        assert_eq!(input.cursor_chars(), 43);
        let mut top = placed(FOX, 10, 5);
        click(&mut top, 0, 1);
        assert_eq!(top.cursor_chars(), 10);
        assert_eq!(row_of(&top), 1, "a soft break's caret starts the next row");
    }

    /// A one-line field's click counts from the char its window starts
    /// on, wherever on screen it was drawn; past the text, its end.
    #[test]
    fn a_click_in_a_one_line_field_counts_from_where_it_was_drawn() {
        let mut input = placed_line("hello world", 20);
        assert_eq!(click(&mut input, 2 + 4, 5), Edit::Moved);
        assert_eq!(input.cursor_chars(), 4, "hell|o");
        click(&mut input, 2 + 15, 5);
        assert_eq!(input.cursor_chars(), 11);
        assert_eq!(click(&mut input, 1, 5), Edit::Ignored, "left of the field");
        assert_eq!(click(&mut input, 4, 6), Edit::Ignored, "the row below");
        // Never drawn, a field takes no clicks at all.
        let mut hidden = TextInput::with_text("hello");
        assert_eq!(click(&mut hidden, 0, 0), Edit::Ignored);
    }

    /// A drag selects from the press to the pointer, either way round —
    /// across rows in a box — and a click lets it go.
    #[test]
    fn a_drag_selects_from_where_it_was_pressed() {
        let mut input = placed_line("hello world", 20);
        input.mouse(&mouse(MouseEventKind::Down(LEFT), 2 + 6, 5));
        assert!(input.pressed());
        drag(&mut input, 2 + 11, 5);
        assert_eq!(sel(&input), "world");
        assert_eq!(input.cursor_chars(), 11, "the caret on the far end");
        drag(&mut input, 2, 5);
        assert_eq!(sel(&input), "hello ");
        assert_eq!(input.cursor_chars(), 0);
        input.mouse(&mouse(MouseEventKind::Up(LEFT), 2, 5));
        assert!(!input.pressed());
        assert_eq!(drag(&mut input, 2 + 6, 5), Edit::Ignored, "released");
        assert_eq!(sel(&input), "hello ", "kept past the release");
        click(&mut input, 2 + 3, 5);
        assert_eq!(input.selected(), None);

        let mut many = placed(FOX, 10, 5);
        many.mouse(&mouse(MouseEventKind::Down(LEFT), 4, 0));
        drag(&mut many, 5, 1);
        assert_eq!(sel(&many), "quick brown");
        // Dragged off the box, the rows' end.
        drag(&mut many, 30, 4);
        assert_eq!(sel(&many), "quick brown fox jumps over the lazy dog");
    }

    /// A second click on the spot takes the word — or the run of spaces
    /// or punctuation — a third the line, a fourth starts over; a drag
    /// from a double-click grows by words. Clicks apart in time or place
    /// count afresh.
    #[test]
    fn double_and_triple_clicks_take_the_word_and_the_line() {
        use std::time::Duration;
        let mut input = TextInput::multiline_with_text("one two-three\nfour");
        place_rows(&input, Rect::new(0, 0, 40, 4), 0);
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        click_at(&mut input, 5, 0, ms(0));
        assert_eq!(input.selected(), None);
        click_at(&mut input, 5, 0, ms(200));
        assert_eq!(sel(&input), "two");
        click_at(&mut input, 5, 0, ms(400));
        assert_eq!(sel(&input), "one two-three", "the line, not its break");
        click_at(&mut input, 5, 0, ms(600));
        assert_eq!(input.selected(), None, "a fourth starts over");
        click_at(&mut input, 7, 0, ms(800));
        click_at(&mut input, 7, 0, ms(900));
        assert_eq!(sel(&input), "-", "punctuation is a run of its own");
        click_at(&mut input, 3, 0, ms(2000));
        click_at(&mut input, 3, 0, ms(2100));
        assert_eq!(sel(&input), " ");
        click_at(&mut input, 1, 0, ms(5000));
        click_at(&mut input, 2, 0, ms(5100));
        assert_eq!(input.selected(), None, "a different cell is a new click");
        click_at(&mut input, 1, 0, ms(9000));
        click_at(&mut input, 1, 0, ms(9600));
        assert_eq!(input.selected(), None, "too slow to be a double-click");

        // Double-click `two` and drag: whole words either way.
        click_at(&mut input, 5, 0, ms(20_000));
        input.mouse_at(&mouse(MouseEventKind::Down(LEFT), 5, 0), ms(20_100));
        drag(&mut input, 10, 0);
        assert_eq!(sel(&input), "two-three");
        drag(&mut input, 1, 0);
        assert_eq!(sel(&input), "one two");
        drag(&mut input, 1, 1);
        assert_eq!(sel(&input), "two-three\nfour");
    }

    /// ⇧-click stretches the selection — or makes one from the caret — to
    /// the spot, and a drag on from it keeps the same anchor.
    #[test]
    fn shift_click_extends_the_selection() {
        let mut input = placed_line("hello world", 20);
        click(&mut input, 2 + 2, 5);
        let shifted = MouseEvent {
            modifiers: KeyModifiers::SHIFT,
            ..mouse(MouseEventKind::Down(LEFT), 2 + 8, 5)
        };
        input.mouse(&shifted);
        assert_eq!(sel(&input), "llo wo");
        drag(&mut input, 2 + 11, 5);
        assert_eq!(sel(&input), "llo world");
        input.mouse(&mouse(MouseEventKind::Up(LEFT), 2 + 11, 5));
        input.mouse(&MouseEvent {
            modifiers: KeyModifiers::SHIFT,
            ..mouse(MouseEventKind::Down(LEFT), 2, 5)
        });
        assert_eq!(sel(&input), "he", "from the same anchor");
    }

    /// The wheel over a box scrolls it; a one-line field leaves the wheel
    /// to the list under it. A drag past a box's edge scrolls it a row a
    /// step, the selection growing with it.
    #[test]
    fn the_wheel_and_a_drag_past_the_edge_scroll_a_box() {
        let mut input = placed(&twenty_lines(), 20, 5);
        assert_eq!(input.view().top, 15);
        assert_eq!(
            input.mouse(&mouse(MouseEventKind::ScrollUp, 3, 2)),
            Edit::Moved
        );
        assert_eq!(input.view().top, 12);
        assert_eq!(
            input.mouse(&mouse(MouseEventKind::ScrollUp, 30, 2)),
            Edit::Ignored,
            "not over the box"
        );
        let mut line = placed_line("hello", 20);
        assert_eq!(
            line.mouse(&mouse(MouseEventKind::ScrollUp, 3, 5)),
            Edit::Ignored
        );

        // A box drawn from row 3, `l15` on its top row.
        let mut up = drawn(&twenty_lines(), 20, 5);
        place_rows(&up, Rect::new(0, 3, 20, 5), up.view().top);
        up.mouse(&mouse(MouseEventKind::Down(LEFT), 0, 3));
        drag(&mut up, 0, 1);
        assert_eq!(up.view().top, 14);
        drag(&mut up, 0, 1);
        assert_eq!(up.view().top, 13);
        assert_eq!(sel(&up), "l13\nl14\n");
        // Below the bottom: down again, to the end at most.
        for _ in 0..9 {
            drag(&mut up, 0, 20);
        }
        assert_eq!(up.view().top, 15);
        assert_eq!(sel(&up), "l15\nl16\nl17\nl18\n");
    }

    /// A one-line field too long for its window scrolls only as far as
    /// keeps the caret in sight — not recentring on every step, so the
    /// text holds still under a drag.
    #[test]
    fn a_long_one_line_field_scrolls_only_as_far_as_it_must() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        let mut input = TextInput::with_text(text);
        assert_eq!(input.line_first(10), 17, "never drawn: the text's end");
        place_line(&input, Rect::new(0, 0, 10, 1), 17);
        assert_eq!(input.line_first(10), 17, "at the end, the window stays");
        for _ in 0..5 {
            press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        }
        assert_eq!(input.line_first(10), 17, "caret 21 still in sight");
        for _ in 0..4 {
            press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        }
        assert_eq!(input.line_first(10), 16, "caret 17 one in from the `…`");
        press(&mut input, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(input.line_first(10), 0);
        // Short text never scrolls.
        assert_eq!(TextInput::with_text("abc").line_first(10), 0);
    }

    #[test]
    fn with_text_parks_the_cursor_at_the_end() {
        let mut input = TextInput::with_text("note");
        assert_eq!(input.cursor_chars(), 4);
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(input.as_str(), "not");
    }

    // ---- the SELECTION ----

    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    fn mods(list: &[KeyModifiers]) -> KeyModifiers {
        list.iter().fold(KeyModifiers::NONE, |all, m| all | *m)
    }

    /// The selected text, or "" with none.
    fn sel(input: &TextInput) -> &str {
        input.selected().unwrap_or("")
    }

    /// ⇧←/⇧→ grow and shrink the selection a character at a time from
    /// where it began; plain ←/→ then land on its near edge and stop, as
    /// in a macOS text field.
    #[test]
    fn shift_arrows_select_by_character_and_plain_arrows_collapse_to_an_edge() {
        let mut input = typed("hello world");
        for _ in 0..5 {
            assert_eq!(press(&mut input, KeyCode::Left, SHIFT), Edit::Moved);
        }
        assert_eq!(sel(&input), "world");
        assert_eq!(input.selection_chars(), Some((6, 11)));
        press(&mut input, KeyCode::Right, SHIFT);
        assert_eq!(sel(&input), "orld", "shrinks back toward the anchor");
        // ← lands on the start and goes no further.
        assert_eq!(
            press(&mut input, KeyCode::Left, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!((input.cursor_chars(), input.selected()), (7, None));
        // → from a selection made leftward lands on its end.
        press(&mut input, KeyCode::Left, SHIFT);
        press(&mut input, KeyCode::Left, SHIFT);
        press(&mut input, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!((input.cursor_chars(), input.selected()), (7, None));
        // Back over the anchor, the selection flips sides.
        press(&mut input, KeyCode::Right, SHIFT);
        press(&mut input, KeyCode::Left, SHIFT);
        press(&mut input, KeyCode::Left, SHIFT);
        assert_eq!(sel(&input), "w");
        assert_eq!(input.cursor_chars(), 6);
    }

    /// ⌥⇧←/⌥⇧→ by word — the modified arrows Ghostty sends, and the
    /// `ESC B` / `ESC F` a readline-minded terminal would — and ^⇧←/^⇧→
    /// the same.
    #[test]
    fn option_shift_arrows_select_by_word() {
        let mut input = typed("fix the login redirect");
        let alt_shift = mods(&[KeyModifiers::ALT, SHIFT]);
        press(&mut input, KeyCode::Left, alt_shift);
        assert_eq!(sel(&input), "redirect");
        press(&mut input, KeyCode::Left, alt_shift);
        assert_eq!(sel(&input), "login redirect");
        press(&mut input, KeyCode::Right, alt_shift);
        assert_eq!(sel(&input), " redirect");
        let mut esc = typed("fix the login");
        press(&mut esc, KeyCode::Char('B'), alt_shift);
        assert_eq!(sel(&esc), "login");
        press(&mut esc, KeyCode::Char('B'), KeyModifiers::ALT);
        assert_eq!(sel(&esc), "the login", "an uppercase B is a shifted one");
        press(&mut esc, KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!((esc.selected(), esc.cursor_chars()), (None, 0));
        let mut ctrl = typed("one two");
        press(
            &mut ctrl,
            KeyCode::Left,
            mods(&[KeyModifiers::CONTROL, SHIFT]),
        );
        assert_eq!(sel(&ctrl), "two");
    }

    /// ⌘⇧←/⌘⇧→ and ⇧Home/⇧End select to the ends of the line under the
    /// caret; ⌘⇧↑/⌘⇧↓ and ^⇧Home/^⇧End to the ends of the text.
    #[test]
    fn line_and_text_ends_extend_the_selection() {
        let cmd_shift = mods(&[KeyModifiers::SUPER, SHIFT]);
        let mut input = TextInput::multiline_with_text("first line\nsecond line");
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        press(&mut input, KeyCode::Left, cmd_shift);
        assert_eq!(sel(&input), "second li");
        press(&mut input, KeyCode::Right, cmd_shift);
        assert_eq!(sel(&input), "ne", "back over the anchor to the line's end");
        press(&mut input, KeyCode::Home, SHIFT);
        assert_eq!(sel(&input), "second li");
        press(&mut input, KeyCode::End, SHIFT);
        assert_eq!(sel(&input), "ne");
        press(&mut input, KeyCode::Up, cmd_shift);
        assert_eq!(sel(&input), "first line\nsecond li");
        press(&mut input, KeyCode::Down, cmd_shift);
        assert_eq!(sel(&input), "ne");
        press(
            &mut input,
            KeyCode::Home,
            mods(&[KeyModifiers::CONTROL, SHIFT]),
        );
        assert_eq!(sel(&input), "first line\nsecond li");
        // A one-line field takes the line's ends the same way.
        let mut one = typed("solo");
        press(&mut one, KeyCode::Home, SHIFT);
        assert_eq!(sel(&one), "solo");
    }

    /// ⇧↑/⇧↓ select by row as drawn, keeping the column; past the first
    /// or last row they select on to the very start or end. A one-line
    /// field leaves them to its caller, as it does plain ↑/↓.
    #[test]
    fn shift_up_and_down_select_by_row() {
        let mut input = drawn("one two\nthree four\nfive", 80, 5);
        press(&mut input, KeyCode::Up, SHIFT);
        assert_eq!(sel(&input), "e four\nfive", "column 4 on the row above");
        press(&mut input, KeyCode::Up, SHIFT);
        assert_eq!(sel(&input), "two\nthree four\nfive");
        assert_eq!(press(&mut input, KeyCode::Up, SHIFT), Edit::Moved);
        assert_eq!(
            sel(&input),
            "one two\nthree four\nfive",
            "the top row: to the start"
        );
        // Plain ↓ lets it go from its end — the text's end — and has
        // nowhere further to go, but the key was still the field's.
        assert_eq!(
            press(&mut input, KeyCode::Down, KeyModifiers::NONE),
            Edit::Moved
        );
        assert_eq!((input.selected(), input.cursor_chars()), (None, 23));
        let mut one = typed("solo");
        assert_eq!(press(&mut one, KeyCode::Up, SHIFT), Edit::Ignored);
        assert_eq!(press(&mut one, KeyCode::Down, SHIFT), Edit::Ignored);
        assert_eq!(one.selected(), None);
    }

    /// Plain ↑ over a selection sets out from its start, ↓ from its end.
    #[test]
    fn plain_rows_set_out_from_the_selections_edge() {
        let mut input = drawn("alpha\nbravo\ncharlie", 80, 5);
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        press(&mut input, KeyCode::Left, SHIFT);
        press(&mut input, KeyCode::Left, SHIFT);
        assert_eq!(sel(&input), "av");
        press(&mut input, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            (input.selected(), input.cursor_chars()),
            (None, 2),
            "al|pha"
        );
    }

    /// ⌘A takes the whole text, multi-row or not; ^A is still the line's
    /// start — what Ghostty types for ⌘← — and never selects.
    #[test]
    fn cmd_a_selects_everything_and_ctrl_a_does_not() {
        let mut input = TextInput::multiline_with_text("one\ntwo");
        press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(
            press(&mut input, KeyCode::Char('a'), KeyModifiers::SUPER),
            Edit::Moved
        );
        assert_eq!(sel(&input), "one\ntwo");
        assert_eq!(input.cursor_chars(), 7);
        press(&mut input, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!((input.selected(), input.cursor_chars()), (None, 0));
        // Other ⌘ letters stay the caller's.
        assert_eq!(
            press(&mut input, KeyCode::Char('p'), KeyModifiers::SUPER),
            Edit::Ignored
        );
    }

    /// Typing, a paste and a line break all replace the selection.
    #[test]
    fn typing_pasting_and_breaking_a_line_replace_the_selection() {
        let mut input = TextInput::multiline_with_text("fix the bug now");
        for _ in 0.."bug now".len() {
            press(&mut input, KeyCode::Left, KeyModifiers::NONE);
        }
        for _ in 0.."bug".len() {
            press(&mut input, KeyCode::Right, SHIFT);
        }
        assert_eq!(
            press(&mut input, KeyCode::Char('X'), SHIFT),
            Edit::Changed,
            "a shifted letter types"
        );
        assert_eq!(input.as_str(), "fix the X now");
        assert_eq!(input.selected(), None);
        press(&mut input, KeyCode::Char('a'), KeyModifiers::SUPER);
        input.insert_str("pasted");
        assert_eq!((input.as_str(), input.cursor_chars()), ("pasted", 6));
        press(&mut input, KeyCode::Left, SHIFT);
        press(&mut input, KeyCode::Enter, SHIFT);
        assert_eq!(input.as_str(), "paste\n");
    }

    /// ⌫, Delete and every word or line delete remove just the selection.
    #[test]
    fn deletes_take_only_the_selection() {
        let select_two = |input: &mut TextInput| {
            input.set_text("one two three");
            for _ in 0.." three".len() {
                press(input, KeyCode::Left, KeyModifiers::NONE);
            }
            press(input, KeyCode::Left, mods(&[KeyModifiers::ALT, SHIFT]));
            assert_eq!(sel(input), "two");
        };
        let mut input = TextInput::new();
        for (code, m) in [
            (KeyCode::Backspace, KeyModifiers::NONE),
            (KeyCode::Delete, KeyModifiers::NONE),
            (KeyCode::Backspace, KeyModifiers::ALT),
            (KeyCode::Backspace, KeyModifiers::SUPER),
            (KeyCode::Delete, KeyModifiers::ALT),
            (KeyCode::Char('w'), KeyModifiers::CONTROL),
            (KeyCode::Char('u'), KeyModifiers::CONTROL),
            (KeyCode::Char('k'), KeyModifiers::CONTROL),
            (KeyCode::Char('d'), KeyModifiers::CONTROL),
            (KeyCode::Char('d'), KeyModifiers::ALT),
        ] {
            select_two(&mut input);
            assert_eq!(press(&mut input, code, m), Edit::Changed, "{code:?} {m:?}");
            assert_eq!(input.as_str(), "one  three", "{code:?} {m:?}");
            assert_eq!(
                (input.cursor_chars(), input.selected()),
                (4, None),
                "{code:?} {m:?}"
            );
        }
    }

    /// A selection over multi-byte characters moves, draws and deletes by
    /// whole characters, in chars for the renderer.
    #[test]
    fn a_selection_over_multibyte_text_is_whole_characters() {
        let mut input = typed("naïve → café");
        press(&mut input, KeyCode::Left, mods(&[KeyModifiers::ALT, SHIFT]));
        assert_eq!(sel(&input), "café");
        assert_eq!(input.selection_chars(), Some((8, 12)));
        press(&mut input, KeyCode::Left, SHIFT);
        press(&mut input, KeyCode::Left, SHIFT);
        assert_eq!(sel(&input), "→ café");
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!((input.as_str(), input.cursor_chars()), ("naïve ", 6));
    }

    /// Two fields that differ only in their selection are not equal — and
    /// only in which way round it was made, they are. Replacing the text,
    /// or a click, lets it go.
    #[test]
    fn the_selection_is_part_of_a_fields_value() {
        let plain = typed("abc");
        let mut left = typed("abc");
        press(&mut left, KeyCode::Home, SHIFT);
        assert_ne!(left, plain);
        let mut right = typed("abc");
        press(&mut right, KeyCode::Home, KeyModifiers::NONE);
        press(&mut right, KeyCode::End, SHIFT);
        assert_eq!(sel(&right), sel(&left));
        assert_eq!(right.selection_chars(), left.selection_chars());
        left.set_text("abc");
        assert_eq!(left, plain);
        press(&mut right, KeyCode::Home, SHIFT);
        place_line(&right, Rect::new(0, 0, 10, 1), 0);
        click(&mut right, 1, 0);
        assert_eq!(right.selected(), None);
    }
}
