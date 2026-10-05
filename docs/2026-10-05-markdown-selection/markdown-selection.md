# Select and copy text in rendered markdown

## Problem

Rendered markdown shows up in three places, and in none of them can you select or copy text:

| Surface | State | Drawn in | Mouse handled in |
|---|---|---|---|
| MARKDOWN PAGE (⌘P, ⌥click, find in files, tree browser on a `.md`) | `app.page: Option<MarkdownPage>`, `crates/orion-tui/src/markdown_view.rs` | `draw_page`, `crates/orion-tui/src/ui.rs:380` | `handle_mouse`, `crates/orion-tui/src/event_loop.rs:11550`: wheel only, a click outside closes, everything else is dropped |
| FILE TABS preview (`orion open x.md`, rendered unless `m` was pressed) | `Overlay::FileTabs(FileTabsView)`, `crates/orion-tui/src/file_tabs.rs` | `ui.rs` ~2633 (`view.renders_markdown()` branch) | `file_tabs::handle_mouse`, `file_tabs.rs:437`: tab clicks, wheel, click = `IntoPreview` |
| TREE BROWSER preview of a `.md` | `Overlay::Tree(TreeBrowser)`, `crates/orion-tui/src/tree_browser.rs` | `ui.rs` ~2786 | tree browser mouse code in `event_loop.rs` |

Orion turns on mouse capture, so the outer terminal's own selection never gets the drag. ⇧drag only gets through on some emulators, and Terminal.app has no bypass at all; see the comment on `TermSelection`, `crates/orion-tui/src/app.rs:3405`. The session pane gets around this with an app-side selection (`TermSelection` plus `selection_text`, `finish_selection`, `select_word_at` and `copy_text_and_flash` in `event_loop.rs` ~10975–11200). Rendered markdown has nothing like it.

## Goal

The rendered markdown views should select the way the session pane does:

- **Drag** selects a stream of text (whole rows between the two ends), highlighted in REVERSED. The text goes to the clipboard on mouse-up, with the existing `copied N chars` flash.
- **Double-click** selects the word under the pointer and copies it.
- The highlight stays after mouse-up. The next click or any key clears it. The wheel keeps it, because the selection is anchored to document rows, so it stays on its text while the page scrolls.
- **Dragging past the top or bottom edge** scrolls the page and keeps extending the selection.
- **⌘C** (and `^y` on the page) copies the selection when there is one. With no selection it does what it does now and copies the path. This matches the pane, where ⌘C copies the drag selection (`PANE_COPY`, `event_loop.rs:3844`).
- **Copied text reads as prose.** Rows that only wrapped because of the width are joined back with a space, and the decoration on wrapped rows (hanging indent, quote bar) is dropped. Done in step 5. Without it the copy is still correct, just with hard newlines at every wrap point.

Clicking outside the page still closes it. Clicking inside no longer does nothing: it arms a selection.

## Design

One shared module holds the selection over a flowed `Vec<Line<'static>>` (`markdown::Rendered::lines`). Each of the three views keeps an `Option<DocSelection>` and passes it mouse events and its geometry (text rect + scroll). Points are `(col, doc_row)`, where `doc_row` indexes `Rendered::lines`, not the screen, so scrolling doesn't move the selection. A view drops the selection whenever it drops `rendered` (reload, width change, tab change, `m`/`^r` toggle), because the rows it points at are gone.

## Steps

### 1. New module `crates/orion-tui/src/doc_select.rs`

Register it in `crates/orion-tui/src/lib.rs` next to `markdown_view`. Give it a module doc header in the house style (DOC SELECTION — …) that cross-references `TermSelection`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocSelection {
    pub anchor: (u16, usize), // (display col, doc row)
    pub head: (u16, usize),
    pub dragging: bool,
    /// A real selection rather than an armed click (TermSelection's rule).
    pub active: bool,
}
```

Functions (all pure, so they can be unit tested without an `App`):

- `DocSelection::arm(cell) -> Self`: `dragging: true, active: false`.
- `bounds(&self) -> ((u16, usize), (u16, usize))`: normalised row-major. Copy `TermSelection::bounds`.
- `point_at(area: Rect, scroll: u16, rows: usize, col: u16, row: u16) -> (u16, usize)`: screen cell → doc point, clamped. A pointer above the area maps to column 0 of the top visible row, below it to the last column of the bottom visible row, and nothing goes past `rows - 1`. Same idea as `pane_cell` in `event_loop.rs`.
- `drag_to(&mut self, point)`: moves the head and sets `active` once the head leaves the anchor. Returns whether anything changed.
- `row_text(line: &Line, from: u16, to: u16) -> String`: walks the spans by display width (`unicode_width::UnicodeWidthChar`) and keeps chars whose start column is in `from..=to`. A wide char that straddles `from` counts as in. Trailing whitespace is trimmed.
- `text(&self, lines: &[Line]) -> Option<String>`: `None` unless `active`. First row runs from the start col to the end of the row, middle rows are whole, the last row runs from 0 to the end col. Rows are joined with `\n` (step 5 changes this to soft-wrap joining). Returns `None` when the result is empty.
- `word_at(lines: &[Line], point) -> Option<DocSelection>`: the longest run of non-blank cells around the point (`select_word_at`'s rule, `event_loop.rs` ~11160), returned `active: true, dragging: false`. `None` on a blank cell.
- `paint(&self, buf: &mut Buffer, area: Rect, scroll: u16)`: for each visible row inside the bounds, `buf.set_style(rect, Style::default().add_modifier(Modifier::REVERSED))`. The per-row `from`/`to` logic is the same as the pane's (`ui.rs` ~4230–4255), and every rect is intersected with `area`. Does nothing when `!active`.
- `edge_overshoot(area, row) -> Option<i32>`: signed rows past the top (negative) or bottom (positive). `event_loop.rs` has a private one for the pane; make it `pub(crate)` and reuse it, or move it here and have the pane call this one.

### 2. MARKDOWN PAGE (`markdown_view.rs`, `ui.rs::draw_page`, `event_loop.rs`)

1. Add `pub selection: Option<DocSelection>` and `pub last_click: Option<(Instant, (u16, usize))>` to `MarkdownPage`, initialised to `None` in `open`. Clear `selection` in `reload()`. Also clear it in `draw_page` when `Rendered::for_width` reflowed, i.e. when the cached width differed: check `page.rendered.as_ref().map(|r| r.width) != Some(text.width)` before the call.
2. Add `MarkdownPage::selected_text(&self) -> Option<String>` → `self.selection?.text(&self.rendered?.lines)`.
3. In `draw_page`, after `f.render_widget(Paragraph::new(rows), text)`, call `if let Some(sel) = page.selection { sel.paint(f.buffer_mut(), text, page.scroll) }`.
4. In `handle_mouse`'s page branch (`event_loop.rs:11550`), replace the match:
   - `Down(Left)` outside `page.frame` closes the page, as now.
   - `Down(Left)` inside `page.area`: work out the point. If `is_double_click(&mut page.last_click, point)`, set `page.selection = word_at(..)` and copy. Otherwise set `page.selection = Some(DocSelection::arm(point))`.
   - `Down(Left)` anywhere else inside the frame (border, hints): clear the selection.
   - `Drag(Left)` while `selection.dragging`: if the pointer is past an edge, `page.scroll_by(overshoot.clamp(-3, 3))` first. Then `drag_to(point_at(..))`.
   - `Up(Left)`: a non-`active` selection becomes `None`. An active one gets `dragging = false` and its `selected_text()` goes to `copy_text_and_flash` (make that `pub(crate)`).
   - Wheel: scroll as now and keep the selection.

   Collect the text to copy before calling `copy_text_and_flash(app, …)`, so the `&mut app.page` borrow has ended.
5. **Edge auto-scroll while the pointer is still.** Drag events only arrive while the mouse moves. For v1, scrolling on each drag event is enough. If that feels sticky, reuse the pane's beat: `app.next_drag_autoscroll` / `DRAG_AUTOSCROLL_TICK` (`drag_autoscroll_tick`, `event_loop.rs` ~11100), with a page branch that calls `scroll_by` and then `drag_to`. Keep this out of v1 unless it's needed.
6. Keys, in `handle_page_key` (`event_loop.rs:6117`): before `page.key(&key)`, if the chord is `⌘C`/`^y` and `page.selected_text()` is `Some`, copy it and return. For any other key, clear `page.selection` and carry on.
7. `update_pointer` (`event_loop.rs:11406`) needs no change. The pointer stays default over text.

### 3. FILE TABS (`file_tabs.rs`, `ui.rs` ~2633)

1. Add `pub selection: Option<DocSelection>` and `pub last_click: Option<(Instant, (u16, usize))>` to `FileTabsView`. Clear `selection` in `set_preview` and wherever `rendered` is set to `None` (the `m` toggle).
2. In the draw branch, after the preview `Paragraph`, paint when `rendered.is_some() && !editing`, using `body` and `scroll`. Clear the selection in the write-back when the flow width changed.
3. `file_tabs::handle_mouse` (`file_tabs.rs:437`) only selects when the tab renders markdown (`view.renders_markdown()` and no embedded editor). Source mode keeps today's behaviour.
   - `Down(Left)` in `body_area`: run `Cmd::IntoPreview` as now, then arm or word-select the same way the page does.
   - `Drag(Left)` and `Up(Left)`: same as the page, with `Cmd::Scroll(n)` for the edge.
   - Copy on Up.

   `handle_mouse` currently returns early (`_ => return`) for these kinds, so add arms for them.
4. Keys: find the FILE TABS key handler (`run` / the `Cmd` mapping in `file_tabs.rs` ~400–430). A selection-copy chord comes before the existing ones and any other key clears the selection. Check `file_tabs::hints` and the `⌘C` binding there so nothing collides.

### 4. TREE BROWSER preview (`tree_browser.rs`, `ui.rs` ~2786, tree mouse in `event_loop.rs`)

Same wiring as FILE TABS. The selection lives on `TreeBrowser` and is cleared when the selected node or `rendered` changes, and on `^r`. It is only active while the preview shows the rendered page. Look up the tree browser's preview rect write-back and its mouse branch: `grep -n "Overlay::Tree" crates/orion-tui/src/event_loop.rs`. Clicks in the tree panel and drags on the splitter (`files_drag`) must not change.

### 5. Prose-friendly copy: join soft wraps (`markdown.rs`)

Rows that `flow` broke only for width are continuations. `Rendered` should record that so `text()` can join them.

1. Change `Rendered` to `pub lines: Vec<Line<'static>>, pub wraps: Vec<Wrap>`, where `#[derive(Clone, Copy, Default)] pub struct Wrap { pub continues: bool, pub lead: u16 }`. `continues` means the row carries on the previous row's text after a width break. `lead` is the number of decoration columns at the row's start (hanging indent, `QUOTE_BAR`, `CODE_PAD`, list indent) that a copy should drop.
2. `flow` (`markdown.rs:915`) returns rows. Track which rows started because of the width test (`cur + 1 + w > avail` or the in-word char break) and which started because of `Kind::Break`. Return that flag with each `Row`. The `type Row` alias at `markdown.rs:143` grows a bool, and the table path (`markdown.rs:534`) can ignore it, since table cells always copy as rows.
3. `Renderer::emit` (find it near `flush_inline`, `markdown.rs:425`) is where the indent/bar prefix gets prepended. Record `Wrap { continues, lead: prefix_width }` alongside each line pushed. `Renderer::finish` returns both vectors. Keep `render()`'s signature for the issue/PR panes by adding `render_with_wraps()` and having `render()` drop the wraps. `Rendered::for_width` calls the new one.
4. In `DocSelection::text`, take `wraps: &[Wrap]`. For a row with `continues`, skip its first `lead` columns and join it to the previous row with a single space instead of `\n`. When the selection starts mid-row, the existing column bounds still apply.
5. Code blocks never reflow (they truncate or wrap per char), so their rows must report `continues: false`. Check this against the code-block path.

If step 5 grows past what `flow`/`emit` can carry cleanly, ship steps 1–4 first. The selection is useful without it.

### 6. Docs and help

- In `ui.rs` ~1400, the help screen's "TERMINAL & MOUSE" group, add or extend an entry to say that drag selects and copies in markdown pages too, or add a line under the markdown page's own hints if it has a help entry.
- `docs/keys.md`: document drag/double-click/⌘C on the markdown page, file tabs and tree preview.
- The `markdown_view.rs` module doc: one sentence on selection.

## Tests

Add them beside the existing ones, in the house style (descriptive `snake_case` sentences).

- `doc_select.rs` unit tests, against hand-built `Vec<Line>`:
  - a forward and a backward drag give the same `bounds`;
  - a single-row partial selection copies the exact substring;
  - a multi-row selection copies first row from col, middle rows whole, last row to col, with trailing spaces trimmed;
  - a wide char (CJK or emoji) at the boundary is included once;
  - `word_at` on a blank cell is `None`, and on `foo/bar.md` it selects the whole path;
  - `point_at` clamps above, below and past the last row;
  - an armed, inactive selection gives `text() == None`.
- `markdown.rs`: a long paragraph at width 20 marks its later rows `continues`, and a `- item` list's wrapped row has `lead == 2` (or whatever the bullet indent is). Copying a whole wrapped paragraph returns the original sentence on one line.
- `event_loop.rs` tests, next to `a click outside closes the page` (~29518): open a page, `flowed`, send `Down`/`Drag`/`Up` `MouseEvent`s inside `page.area`, and assert that `page.selection` is active and the flash says `copied N chars`. A double-click selects a word. A key clears the selection. A click outside still closes the page. A wheel notch keeps the selection.
- FILE TABS test, next to the existing `file_tabs` tests in `event_loop.rs` (~37130): drag inside the rendered preview copies, and does nothing in source mode (`m`).

## Verification

```sh
cargo fmt --all
cargo clippy -p orion-tui --all-targets -- -D warnings
cargo test -p orion-tui doc_select
cargo test -p orion-tui markdown
cargo test -p orion-tui page
cargo test -p orion-tui file_tabs
cargo test -p orion-tui
```

By hand, in a real orion build (`cargo run -p orion`, or the project's Makefile target):

1. Open `README.md` with ⌘P. Drag across part of a paragraph: it highlights, and the flash says `copied N chars`. Paste somewhere and check the text has no hard wraps (after step 5).
2. Double-click a word: it alone gets copied.
3. Drag down past the bottom border: the page scrolls and the selection grows.
4. Scroll with the wheel: the highlight stays on its text. Press `j`: the highlight clears.
5. With a selection, ⌘C copies it. Without one, ⌘C copies the path.
6. Click outside the modal: it still closes.
7. `orion open README.md` from a session: the same behaviour in the FILE TABS preview. Press `m` for source and check that drag no longer selects there.
8. Tree browser on a `.md`: drag in the preview selects, and the tree panel and splitter work as before.
9. Try it in Terminal.app and in Ghostty/kitty.
