# One shortcut to delete a worktree and everything in it

## Context

On the launcher grid (the homepage), each band is a worktree and each card in it is an agent or terminal. Clearing out a finished worktree takes many keystrokes today. You press ⌫ on every card, confirming each one, and then ⌫ again on the band once it's empty.

Two existing paths already delete a band with live cards: the band's right-click **Delete worktree** menu item, and `activate::delete_worktree`. Both confirm once, warning "N session(s) will be killed". The daemon then kills every session in the worktree before it removes the checkout (`crates/orion-daemon/src/registry.rs:873` `delete_worktree` → `kill_sessions_in`). The only thing missing is a key to reach that flow from any card.

Goal: **⌘⌫**, with **Shift+D** as its twin for terminals that never send ⌘, opens one confirm that lists the doomed sessions and then deletes the band's worktree with everything in it.

## Steps

0. Copy this plan to `docs/2026-10-05-delete-worktree-shortcut/delete-worktree-shortcut.md` so it lives with the repo's other plans.

1. **New action**: `crates/orion-tui/src/keymap.rs`
   - Add `Action::DeleteWorktree` next to `Action::DeleteAll` in the enum (around line 134).
   - Add an `ActionSpec` right after `Action::Delete`'s (around line 567):
     - `id: "delete_worktree"`
     - `label: "Delete worktree"`
     - `hint: "Delete the worktree under the cursor from disk, with every agent and terminal in it, behind one confirmation that lists them. The main checkout is never deleted"`
     - `group: "SESSIONS"`, `scope: Scope::Global`
     - `defaults: &["cmd+backspace", "shift+d"]`
   - Because `ghostty_config::keymap_unbinds` walks every action's ⌘ chords, Ghostty's managed block releases `super+backspace` automatically. `cmd+backspace` is already in `GHOSTTY_BINDS` and not in `NEVER_RELEASED`, so nothing else is needed there.

2. **Keep ⌘⌫ as "delete to line start" inside a locked pane**: `crates/orion-tui/src/event_loop.rs`, around line 3734
   - Today the locked-pane path sends every ⌘ chord orion binds to `dispatch_action`, because "no one types a ⌘ chord as text". ⌘⌫ is the exception: people use it to clear an agent's prompt.
   - Before the `if chord.mods.contains(KeyModifiers::SUPER)` block, skip the intercept when the chord is bound to `Action::DeleteWorktree` and its code is `KeyCode::Backspace`, so the key falls through to the PTY write. `crates/orion-tui/src/keys.rs` already encodes ⌘⌫; its tests are at lines 330 and 364.
   - Write it as a small helper next to `folds_launcher_pane`, for example `fn types_in_the_pane(app, &chord) -> bool`.
   - Text fields such as the new-agent box are overlays and handle keys before the keymap. They keep their own ⌘⌫ (`text_input.rs:589`).

3. **Which worktree the key acts on**: `crates/orion-tui/src/event_loop/launcher.rs`
   - Add `pub(super) fn band_worktree(app: &App) -> Option<WorktreeId>` next to `empty_band` (line 362).
   - It returns the worktree of the band under the grid's cursor, whether the cursor is on a card, a drawer line or the band itself. Build it with the same `view::bands(app)` and `view::band_cursor(app, &bands)` that `empty_band` uses.
   - It returns `None` when the grid is down.

4. **Dispatch**: `crates/orion-tui/src/event_loop.rs`
   - Next to `Action::Delete => open_delete_confirm(app)` (line 4157), add `Action::DeleteWorktree => open_delete_worktree_confirm(app)`.
   - Write `open_delete_worktree_confirm` beside `open_delete_confirm` (line 6187):
     - Find the target with `launcher::band_worktree(app)`. Fall back to `worktree_in_context(app)` (line 6177) for the panel layout.
     - If the target is the main checkout, show `app.flash = Some(Flash::note("The main checkout can't be deleted"))` instead of returning silently.
     - Otherwise call `activate::delete_worktree(app, &id)`.
   - Placeholder worktrees still return silently, as they do inside `activate::delete_worktree`.

5. **Itemise the confirm**: `crates/orion-tui/src/event_loop/activate.rs:338` `delete_worktree`
   - When `live_here > 0`, append the session names below the existing sentence, the way the bulk dialogs do. Reuse `bulk_confirm_listing` (`event_loop.rs:6389`, made `pub(super)`).
   - Collect names from `app.tree.agents` (live ones only) and `app.tree.terminals` for that worktree.
   - Keep the sentence "Delete worktree '{branch}' from disk? {n} session(s) will be killed." unchanged. The test at `event_loop.rs:35693` asserts on it.
   - The right-click menu goes through the same function and gains the listing too, so both paths stay consistent.
   - The worktree's confirm is already destructive, so its border is red. Enter or `y` confirms and Esc or `n` cancels.
   - Unsaved changes are already handled: the daemon replies `WorktreeHasChanges`, and the existing "Unsaved work" force dialog takes over (`event_loop.rs:12745`).

6. **Make it discoverable**
   - Footer, `crates/orion-tui/src/ui/footer.rs:290-310`: on the `Card::Session` and `Card::Terminal` arms, push `act(km, Action::DeleteWorktree, "delete worktree")` when `!bands[band_at].is_main`.
   - The empty-band arm keeps ⌫ "delete worktree".
   - Help overlay, `crates/orion-tui/src/ui.rs` around line 1394: add `(Act(&[DeleteWorktree]), "delete worktree + all its sessions")` under the `Delete` line.

7. **Stale comments**: the docs on `PendingAction::DeleteAllWorktrees` and `DeleteAllSessions` (`app.rs:715,722`) and on `open_delete_all_confirm` (`event_loop.rs:6404`) still say "Shift+D". `DeleteAll` is unbound, so name the action instead of the key.

## Tests

Add these to `crates/orion-tui/src/event_loop/launcher.rs` tests, modelled on `d_and_the_menu_on_an_empty_band_ask_to_delete_the_worktree` (line 4705). Reuse the fixtures `with_empty_band`, `draw_tall`, `keys`, `key` and the pattern `PendingAction::DeleteWorktree(..)`.

- `cmd_backspace_on_a_card_asks_to_delete_its_whole_worktree`:
  - With the cursor on an agent card in a non-main band, ⌘⌫ (`KeyModifiers::SUPER`) opens `Confirm` with `PendingAction::DeleteWorktree(<that band's id>)`.
  - The message lists the card's name, and nothing is sent before the answer.
  - Shift+D does the same.
- `confirming_deletes_the_worktree_once`: Enter sends exactly one `ClientRequest::DeleteWorktree` for that id and no per-agent deletes. Reuse the `delete_worktree_requests` helper (`event_loop.rs:37104`).
- `cmd_backspace_on_the_main_checkout_only_flashes`: no overlay, and the flash note is set.
- `cmd_backspace_in_a_locked_pane_goes_to_the_agent`: with the pane input-locked, ⌘⌫ opens no overlay. Model this on the existing locked-pane ⌘ tests near `event_loop.rs:15306`.
- Keymap: assert that `cmd+backspace` and `shift+d` look up `Action::DeleteWorktree` in `Scope::Global`. If a keymap test checks for duplicate default chords, it must still pass.

## Verification

```sh
cargo fmt --all
cargo clippy -p orion-tui --all-targets -- -D warnings
cargo test -p orion-tui delete_worktree
cargo test -p orion-tui keymap
cargo test -p orion-tui ghostty
cargo test -p orion-tui
```

Then by hand, using the `run` skill or `cargo run`:
1. Make a throwaway worktree and start two agents and a terminal in it.
2. On any of its cards, press ⌘⌫. The red confirm should list all three. Press Enter, and the band and the checkout should disappear in one step.
3. Check that ⌘⌫ on the main band only flashes.
4. Check that ⌘⌫ inside a locked agent pane still clears the prompt line.
5. Make sure orion's Ghostty keybind block has been rewritten to include `keybind = super+backspace=unbind`, then reload Ghostty (⌘⇧,).

## Known trade-off

Ghostty's managed block applies to every Ghostty window. Releasing `super+backspace` changes ⌘⌫ in plain shells outside orion: it stops deleting the line and Ghostty passes the raw key instead. Users who mind can rebind **Delete worktree** in the Hotkeys tab, and Shift+D still works.
