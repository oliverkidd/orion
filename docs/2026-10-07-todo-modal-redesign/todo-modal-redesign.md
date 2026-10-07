# Todo modal redesign

## Context

The TODOS MODAL (`⌘I`, `crates/orion-tui/src/todos/view.rs`) needs four kinds of change:

- **Editing in place.** Renaming takes `⌘I`/`^E`, and deleting takes `⌘W`. Rows should behave like text fields instead.
- **Priority.** orion draws priority three ways today: signal bars on todos (`linear::priority_mark`), letters in the Linear list (`linear::priority_letter`), and bars again in Linear's reading pane. One mark should be used everywhere.
- **Layout.** Long items are cut off with `…`. Top-level groups and sub-groups look alike. Every header carries the full priority breakdown, and there is no sense of the day's or the week's progress.
- **Navigation.** Moving between groups means stepping through every row.

These options were picked in the prototype (claude.ai artifact "Todo Modal Redesign"):

| Decision | Choice |
| --- | --- |
| Priority mark | Letters: `U` bold `err`, `H` `warn`, `M` `muted`, `L` `dim`, `·` `faint` for none. The same in todos, group counts, the Linear list, the Linear reading pane and the priority filter |
| Group hierarchy | Top-level headers are UPPERCASE and bold, with a `─` rule in `edge` running to the counts and a blank row above (none above the first). Sub-groups are a `dim` fold glyph and a `muted` bold name, and show only `N open` |
| Priority breakdown | Only on top-level headers: `U1 H2 M1 · 6 open · ✓2` |
| Summary | Right-aligned on the tab row: `✓4 today · ✓16 this week ▅▇▄····` (a sparkline of ticks per day, Monday to Sunday, today in the accent, days to come as `·`) |
| First `⌫` on a row | Opens the row as a field with the caret at the end, and deletes the last character |
| `⌘↑` / `⌘↓` | Stops at every group header, sub-groups included |
| Typing on a group header | Edits the group's name |

## Keys after the change

| Action | Before | After |
| --- | --- | --- |
| Edit a row | `⌘I` / `^E` | `⌫` (also deletes the last character), or type a printable character to append it. In the field, `Enter` saves, `↑`/`↓` save and move, `Esc` reverts |
| Delete | `⌘W` / `^W` | `⌘⌫` / `^W`. An item goes at once. A group with items asks first, and `Enter`, `⌘⌫` or `^W` confirms |
| Filter | type anywhere | `⌘F` / `^F` (`list_filter::keys::FILTER`) opens the filter row. `Esc` clears it and closes it |
| Jump between groups | none | `⌘↑` / `⌘↓`, with `⌥↑` / `⌥↓` as the twins where ⌘ doesn't arrive |
| New item | `⌘N` | `⌘N`, or type on the `+ new item` row |

Unchanged: `space`, `Enter` (agent), `⇧Tab`, `⌘1`–`⌘4`, `←`/`→`, `⌘⇧N`, `⌘L`, `⌘R`, `⇧←`/`⇧→`.

## Steps

### 1. One priority mark: `crates/orion-tui/src/linear.rs`

- Make `priority_letter` `pub(crate)`. It already has the agreed colours.
- Delete `priority_mark` (around line 1892).
- In the reading pane's **Priority** row (around line 2094), use the letter, a space, then `priority_word`.
- Update the docs comments that mention the bars.

### 2. Keys table: `crates/orion-tui/src/todos/view.rs` (`mod keys`)

- Remove `RENAME`.
- Change `DELETE` to `Key::new(&["cmd+backspace", "ctrl+w"], "delete")`.
- Change `CONFIRM` to `Key::new(&["enter", "cmd+backspace", "ctrl+w"], "delete")`.
- Add `pub const JUMP: Key = Key::new(&["cmd+up", "cmd+down", "alt+up", "alt+down"], "groups").show(2);`
- Add `pub const FILTER: Key = crate::list_filter::keys::FILTER;`
- Add `pub const EDIT: Key = Key::new(&["backspace"], "edit");`
- Update `ALL`.
- In `hints()`, the Today list becomes: `DONE`, `AGENT`, `NEW`, `EDIT`, `LINEAR`, priority, `DELETE`, `FILTER`, `JUMP`, `FOLD`, `NEW_GROUP`, `PRESET`, `REFRESH`, `TABS`, `Esc`. The field's hints become `SAVE`, `↑↓ save & move`, `Esc revert`.
- `crates/orion-tui/src/ghostty_config.rs` `MODAL_KEYS`: remove `RENAME`, and add `JUMP` and `FILTER`.
- `crates/orion-tui/src/event_loop.rs` `modal_takes_cmd_w`: remove `Overlay::Todos(_)`. `⌘W` is no longer the modal's, and the guard then swallows it as it does over other modals.

### 3. Filter behind `⌘F`: `view.rs`

- Add `pub filtering: bool` to `TodoView`. The filter row is drawn only while it is true.
- `⌘F` sets it. While it is set, printable keys, space and `⌫` go to `query`, as typing does today.
- `Esc` with the filter open clears `query` and closes it. Without the filter open, `Esc` closes the modal.
- `paste`:
  - A multi-line list is still imported.
  - A single line goes to `query` while filtering.
  - Otherwise it is appended to the cursor's row, as typing would be.

### 4. Editing in place: `view.rs`

- Add a `start_edit(app, append: Option<&str>)` function that dispatches on the cursor's row:
  - **Item:** `InputKind::Rename(Target::Item)`, opened with the item's text.
  - **Group header:** `InputKind::Rename(Target::Group)`, opened with the group's name.
  - **`+ new item`:** `InputKind::Item { group: cursor_group }`, starting empty.
  - With `append`, insert that text. Without it (`⌫`), send one `Backspace` to the `TextInput`, so the first press deletes the last character.
- `handle_key`, when no field, menu or confirm is open:
  - `KeyCode::Backspace` with no modifiers calls `start_edit(app, None)`.
  - `KeyCode::Char(c)` with no CONTROL, ALT or SUPER, and `c != ' '`, calls `start_edit(app, Some(c))`.
  - Both only apply on the Today tab, or for a Log item, which can be renamed as today.
- `input_key`: `Up`/`Down` commit the field (an empty new-item field just closes), close any follow-on new-item field `commit` opened, then `step` by ±1.
- Remove `start_rename`.

### 5. Delete and jump: `view.rs`

- `delete()` is unchanged apart from its key. `confirm_delete` matches the new `CONFIRM`.
- Add `jump(app, delta)`, which moves the cursor to the next or previous `Entry::Header` in `entries`.

### 6. Drawing: `view.rs`

- **Wrapping.** Each item's height is the number of lines its text wraps to. The first line's width is the room left after the prefix and the chips. Later lines are as wide as the room after the prefix, and start under the text's first character. Pass these heights to `ui::stacked_rows`, and draw items with `render_row_lines`.
- **Top-level headers.** These are `Entry::Header` with depth 0 that aren't the first row. Give them height 2 and draw them in the lower line, leaving the upper line blank and outside the selection. `row_rects` records the lower line, so clicks still hit it.
- **Header spans.** Replace `rank_glyph` with letters.
  - Top-level: the fold glyph in `muted`, the NAME uppercased and bold in `text`, a `─` rule in `edge` filling the room, then `U1 H2 M1 · 6 open · ✓2`.
  - Sub-group: indent, the fold glyph in `dim`, the name in `muted` bold, and `N open` in `dim` on the right.
- **Priority column.** Item rows use `priority_letter` and a space (`PRIORITY_W` = 1).
- **Tab row summary.** Right-align `✓N today · ✓M this week` plus the sparkline in the tab row, when it fits after the strip. Week counts come from a new `TodoFile::week_ticks(today) -> [usize; 7]`, which counts items whose `done_on()` falls Monday to Sunday of `today`'s week.
- **Filter row.** Drawn only while `filtering`. Otherwise the list starts on the row after the tabs. The `+ new item` row reads `+ new item  ⌘N · or type here`.

### 7. Docs

- `docs/keys.md` line 257, the Todos row: rewrite for letters, editing in place, `⌘⌫`, `⌘F`, `⌘↑`/`⌘↓`, the tab-row summary and wrapping.
- `docs/keys.md` lines 271–284, the shared modal keys table: drop todos from `⌘I` and `⌘W`, and add `⌘⌫` delete and `⌘F` filter for todos.
- `docs/keys.md` line 256 already describes the Linear letters.

## Tests (in `view.rs` and `mod.rs`)

- `week_ticks`: ticks on Monday, on today and the week before. Only this week counts, by weekday.
- `⌫` on an item opens `Rename(Item)` with the last character gone. `Enter` saves it.
- A typed letter on an item appends it. On a header, it appends to the name. On `+ new item`, it opens a new item.
- `↓` in a field saves and moves the cursor down one row.
- `⌘⌫` on an item deletes it. On a group with items, the first press asks and the second deletes.
- `⌘↓` and `⌘↑` stop at each header, sub-groups included.
- With the filter closed, typing doesn't filter. `⌘F` then typing does, and `Esc` clears and closes it.
- `the_hints_come_from_the_table` still passes, with the new keys in `ALL`.
- A long item drawn into a narrow modal takes two rows and no `…`.
- Fix the existing tests that expect the bars (Linear pane tests, if any) or `⌘W`/`⌘I` in todos (`linear.rs` todo tests).

## Verification

```sh
cargo fmt --all
cargo clippy -p orion-tui --all-targets -- -D warnings
cargo test -p orion-tui todos
cargo test -p orion-tui linear
cargo test -p orion-tui ghostty
cargo test -p orion-tui
```

Manual check: run orion, press `⌘I`, and confirm each of the following.

- A long item wraps.
- `⌫` and typing edit the row in place.
- `⌘⌫` deletes.
- `⌘↓` and `⌘↑` jump between groups.
- `⌘F` filters.
- The tab row shows today's and this week's ticks.
- Linear's list and reading pane show the same letters.

None of this touches `crates/orion-daemon/daemon-inputs.txt` paths, so the release is a patch.
