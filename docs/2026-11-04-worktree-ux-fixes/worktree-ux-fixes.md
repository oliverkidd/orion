# Worktree UX fixes: focus, order, Linear picker, autofix, account tags

Five fixes to the orion TUI. Every change is in `crates/orion-tui`, and nothing in `crates/orion-daemon/daemon-inputs.txt` changes, so this ships as a **patch** release. The running daemon and its sessions are kept across the upgrade.

The UI changes (2, 3, 4 and 5) are drawn in the review artifact *Orion Worktree Fixes* (https://claude.ai/artifact/SWqtdKCrc411kFrb59YR6E). The user signed off on option A for each of them. The amendments are a roomier dormant divider (step 2) and the stand-down check autofix runs against any other live session on the branch (step 4d).

Suggested order of work: 1, then 2 (both touch cursor and ordering, and 1's regression test should pass against 2's new order), then 5, 3 and 4. Each step is its own commit.

Verification for every step:

```sh
cargo fmt --all
cargo clippy -p orion-tui --all-targets -- -D warnings
cargo test -p orion-tui
```

---

## 1. The cursor jumps to another worktree after a PR merges

### Cause

`App::sel_worktree` (`crates/orion-tui/src/app.rs:3907`) is an **index** into `App::worktree_rows()` (`app.rs:~6271`). A checkout sits nested under its PR's row (`Pr` then `PrCheckout`). When the PR merges, the PR row goes away and the checkout moves back among the plain checkouts at its recency position. Every row shifts, but the index stays the same, so it lands on a different worktree.

The full open-list answer `note_open_prs_answer_at` (`crates/orion-tui/src/event_loop.rs:~1813`) handles this correctly. It captures the PR under the cursor and the checkout id, then calls `reconcile_open_pr_cursor` and `follow_checkout` (`event_loop.rs:7097`).

`drop_retired_pr` (`event_loop.rs:2174`) does not. It captures only `selected_worktree_pr()`. `reconcile_open_pr_cursor` (`event_loop.rs:2089`) returns early when the cursor isn't on a PR row, and `follow_checkout` is never called. Two paths call it right after a merge, and whichever lands first retires the PR without moving the cursor:

- `land_pr_detail` (`event_loop.rs:~2381`), when the PR page re-read on modal close (`pr_modal::close` → `rearm_pane_detail`, `pr_modal.rs:~490`) answers `MERGED`.
- `land_lookup` (`event_loop.rs:~1578`), when a checkout's PR lookup triggered by base sync (`base_sync.rs:226`) answers merged.

### Steps

1. In `event_loop.rs`, add a helper that saves the cursor before any change to `app.open_prs` and puts it back afterwards:
   ```rust
   /// Runs `mutate` on the open-PR list and keeps the cursor on the same
   /// pull request, or failing that the same checkout, whichever rows move.
   fn keeping_worktree_cursor(
       app: &mut App,
       out: &mut Vec<ClientRequest>,
       mutate: impl FnOnce(&mut App),
   ) {
       let cursor = app.selected_worktree_pr().cloned();
       let checkout = app.selected_worktree().map(|w| w.id.clone());
       mutate(app);
       reconcile_open_pr_cursor(app, cursor, out);
       follow_checkout(app, checkout.as_ref());
   }
   ```
   Capture **before** `mutate`: `worktree_rows()` is memoised on `RowsKey` (`app.rs:~3833`), and that key does not include the open-PR list.
2. Rewrite `drop_retired_pr` to do its `retain` inside `keeping_worktree_cursor`.
3. Move `note_open_prs_answer_at`'s capture and re-seat onto the same helper, so the cursor fix lives in one place.
4. `grep -n "open_prs\.\(retain\|insert\|remove\|clear\)\|open_prs =" crates/orion-tui/src` and route any other change to the open-PR list through the helper too.
5. Check the home page (launcher bands) too. If its selected card is resolved from `sel_worktree` / `sel_session` by index, confirm it follows by id after the change. Manual check below.

### Tests (next to `a_detail_that_says_merged_retires_the_row_on_the_spot`, `event_loop.rs:~17580`)

- `the_cursor_stays_on_a_checkout_whose_pull_request_merges`: use `seed_tree` and `seed_feat_worktree(&mut app, "w2", "pr-7-head")`. Seed open PRs #7 (head `pr-7-head`) and #9, set `sel_worktree = app.worktree_row_of(&w2)`, call `drop_retired_pr(&mut app, &pr_url(7), &mut out)`, then assert `selected_worktree()` is still `w2`.
- The same scenario sent through `land_page` with a `MERGED` detail, followed by a list answer without #7. The cursor must still be on `w2`.

### Manual check

`cargo run -p orion`. Sit on a worktree with an open PR, open the PR modal, merge with ⌘X, then press Esc. The cursor and the pane stay on that worktree.

---

## 2. Stable worktree order, with only dormant worktrees sinking

### Today

`App::worktree_order` (`app.rs:6146`) sorts by `(Reverse(is_main), Reverse(interacted), Reverse(stamped))`. `last_interaction_ms` returns `i64::MAX` while a session is Running or NeedsFeedback, and `status_changed_at` moves on every status change. So every turn that starts or ends reorders the list.

### New rule

1. The root checkout comes first.
2. Then **live** worktrees, in creation order (oldest first, so new worktrees appear at the bottom of the live group).
3. Then **dormant** worktrees, in the same creation order.

A worktree is **dormant** when every unarchived session in it is `drawn_cold` (`app.rs:2991`): no live PTY, not a cloud session, no unread finish. A worktree with no sessions is dormant. Creation order is the ULID `WorktreeId`: it sorts by creation time, and `tree.worktrees` already arrives as `ORDER BY is_main DESC, sort_order, created_at` (`crates/orion-daemon/src/store.rs:1048`).

Only a worktree crossing between live and dormant ever moves. Status changes inside the live group never reorder it.

### Steps

1. In `app.rs`, add:
   ```rust
   /// Every unarchived session in the worktree is cold (or it has none):
   /// the one fact that moves a worktree, to the bottom.
   pub fn worktree_dormant(&self, wt: &WorktreeId) -> bool {
       self.tree.agents.iter()
           .filter(|a| &a.worktree_id == wt && !a.archived)
           .all(drawn_cold)
   }
   ```
2. Change the sort key in `worktree_order` to `(Reverse(w.is_main), self.worktree_dormant(&w.id), w.id.clone())`. Drop `worktree_recencies` from this sort, and drop the function if nothing else calls it.
3. Make sure `rows_key()` (`app.rs:~5766`) changes whenever any worktree's dormant flag changes, or the memo will serve a stale order. A cheap way is to fold a hash of the dormant worktree ids into the key.
4. Apply the same order to `show_worktree_picker` (`event_loop/launcher.rs:~2805`) by calling `worktree_order` instead of its own key, so every list of worktrees agrees.
5. Leave session order inside a band alone (`launcher::rows`, `launcher.rs:96-108`). It wasn't part of the request.
6. Draw a faint `┄┄ dormant ┄┄` divider before the first dormant band in `launcher_view.rs` (`draw_list_band` and the cards path). Leave **one blank row above and one below it** so it doesn't crowd the bands next to it. Skip the divider when there are no dormant worktrees, or when every worktree is dormant. Clicks on the divider and the blank rows do nothing, and the cursor steps over them.

### Tests

- Rewrite `worktrees_sort_by_last_interaction` (`event_loop.rs:26094`) as `worktrees_keep_creation_order_while_live`: a status change on `older` does not move it.
- Add `a_worktree_whose_sessions_all_go_cold_sinks_to_the_bottom`, then wake one of its sessions and assert it goes back to its creation slot.
- Keep `the_cursor_follows_its_pull_request_across_a_reorder` and `the_bands_row_follows_the_cursor` green.

---

## 3. The Linear "link to worktree" picker shows each worktree's agents

### Today

`WorktreePick { issues, branches: Vec<String>, selected }` (`crates/orion-tui/src/linear.rs:362`) holds only branch names. It is opened by `open_worktree_pick` (`linear.rs:3047`), handled by `handle_worktree_pick_key` (`:3084`) and drawn by `draw_worktree_pick` (`:4396`) through `draw_side_pick` (`:4433`), which draws one line per item.

### Target (see artifact, section 3)

```
  Attach ENG-412 to the PR opened from…
▌ ⎇ feat/billing-export          ENG-398 waiting
     ● billing-agent   opus high   › wire the CSV exporter
     ○ tests           sonnet      › add fixtures for…
  ⎇ fix/login-redirect
     ◌ claude          opus        › 2d
  ⎇ spike/search                    no sessions
```

### Steps

1. Change `WorktreePick` to `items: Vec<PickWorktree>`, where `struct PickWorktree { id: WorktreeId, branch: String }`. Keep the lookup by id in `link_worktree` (`:3106`).
2. In `open_worktree_pick`, use `app.worktree_order(&view.project)` (from step 2) so the picker lists worktrees in the home page's order. Keep the existing filter (`!is_main && !branch.is_empty()`).
3. In `ui/launcher_view.rs`, make `session_look` / `SessionLook` (`:2670`) and `list_runs_line` (`:2938`) `pub(crate)`, along with the account tag helper from step 5. In `ui.rs`, make `ago_badge` (`:3808`) `pub(crate)`.
4. Pass `app` to `draw_worktree_pick` (call site `linear.rs:~4041`). For each worktree draw:
   - the header line: `⎇ branch` plus the existing pending and gave-up notes;
   - one indented line per unarchived session, most recent first, capped at 3, with `+N more` after that: status dot, name, account tag (step 5), `list_runs_line`, and `› last prompt` truncated to fit;
   - `no sessions` in faint text when there are none.
5. Write `draw_grouped_pick` next to `draw_side_pick`. It takes `Vec<(header Vec<Span>, Vec<Vec<Span>>)>` plus `selected`, highlights the whole group (the `▌` rail on every line of the group), and scrolls so the selected group is fully in view. Selection still moves one *worktree* at a time, and Up/Down handling stays the same.

### Tests

- A unit test in `linear.rs` that builds `WorktreePick` from a seeded tree and asserts the items are in `worktree_order` with ids attached.
- A render test with `TestBackend` that draws a pick with two worktrees and asserts an agent's name appears under its branch.

---

## 4. Autofix starts on its own, as the worktree's agent, from a system preset

### Today

Autofix still works (`crates/orion-tui/src/autofix.rs`, 23 tests pass). It feels absent because the user's config has `"pr_autofix": "ask"`, and in `ask` mode:

- the ask modal opens only when `app.overlay.is_none() && !app.term_locked` (`autofix.rs:661-690`), and keys sit in an agent pane most of the time;
- one stray Esc or click outside dismisses that breakage for good (`autofix.rs:1008-1038`), which is what happened to #1484.

The launch also uses the default agent with the Autofix model and effort settings (`Config::autofix_launch`, `config.rs:3273`), not the worktree's agent. The preset is found by the name in `cfg.autofix_preset`.

### Behaviour after the change

- Setting **Auto-fix PRs**: on or off, **on by default**. When a PR you authored fails checks or has conflicts, and once the checks have settled, a fresh `autofix-<n>` session starts in that PR's worktree with nobody asked.
- It runs on the **same harness, account, model and effort** as the worktree's agent: the most recently active unarchived non-`autofix-*` session on the PR's head branch. If there isn't one, it falls back to the Autofix model and effort settings.
- Its prompt is the **PR autofix** system preset wrapped around the failure context.
- If another session on the branch is working or waiting on you, autofix starts anyway. Its first step is to read that session's conversation. If that session is already doing this PR's fix, autofix stands down and archives itself (4d).
- A flash says `autofix-412 started on #412 · checks failing`. When three sends haven't turned the PR green, it stops and posts a flash plus a desktop notification saying so.
- ⌘G in the PR modal still opens the manual autofix form.

### Steps

**4a. Setting**

1. In `autofix.rs:71-97`, keep the `pr_autofix` key and parse it into a `bool`: `"auto"` and `"ask"` mean on, `"off"` means off, and a missing key means on. Write it back as `"auto"` / `"off"` so older builds still read it.
2. In `config.rs:~1044`, replace the three-way "When a PR breaks" row with an `on_off` row named **Auto-fix PRs**, following the existing boolean `SettingSpec` pattern. Rename the Autofix model and effort rows to **Autofix fallback model** and **Autofix fallback effort**.
3. Delete the `ask` path: the queue, `tick`'s modal open, and the dismiss ledger write in `land_detail`. Keep the modal code that ⌘G uses (`pr_modal.rs:707`).
4. When `AUTO_ATTEMPTS` (3) runs out, raise a flash and a desktop notification instead of opening the modal.

**4b. Copy the worktree agent**

1. In `autofix.rs`, add:
   ```rust
   /// The session whose harness, account, model and effort autofix copies:
   /// the most recently active one on the PR's branch, autofix's own excluded.
   fn source_agent<'a>(app: &'a App, project: &ProjectId, head: &str) -> Option<&'a Agent>
   ```
   Filter `app.tree.agents` by `!archived`, worktree in `project` with `branch == head`, and name not starting with `autofix-`. Take the max by `last_interaction_ms`.
2. In `dispatch` (`autofix.rs:949`), take `kind`, `custom_harness`, `model` and `effort` from `source_agent`, falling back to `cfg.autofix_launch()`. `CreatePrAgent` (`orion-core` `protocol.rs:165`) already carries all four, so no protocol change is needed.
3. Busy rule:
   - Block while an `autofix-<n>` for this PR already exists unarchived. There is never a second autofix for one PR.
   - Any other session on the branch that is **Running** or **NeedsFeedback** doesn't block autofix. It triggers the stand-down check in 4d instead.

**4d. Check what the branch's other live sessions are doing, and stand down if one owns the fix**

This check runs whenever autofix fires and another unarchived, non-autofix session on the PR's branch is **Running** (mid-turn) or **NeedsFeedback** (waiting on the user). The new `autofix-<n>` session's first job is to work out whether one of those sessions is already doing work that will land on the same PR. If one is, autofix archives itself and leaves the fix to that session.

Autofix **reads** the other sessions' conversations and never types into their panes. A running session would take typed text as a new prompt mid-turn. A waiting session is stopped on a question or a permission prompt, so anything typed there becomes the answer, and could approve or deny a tool call. Reading the transcript tells autofix what it needs and leaves both kinds of session untouched.

1. In `autofix::prompt` (`autofix.rs:725`), when `live: Vec<(&Agent, PathBuf)>` is non-empty, put a **preamble** before the preset-wrapped task:
   ```text
   Before you change anything, check the other sessions in this worktree:
     - "<name>" (<working | waiting on the user>): <transcript path>
     - …
   Read the end of each conversation. If any of them is doing work that will land
   on this same PR (it is editing, committing or pushing on <head> to fix or extend
   this change, or asking the user something about it), run
   `orion autofix stand-down "<session name>: <one-line reason>"` and stop. Do not edit files.
   If none is, carry on with the task below. Before you push, read those
   conversations again, and stand down the same way if one has started on this fix.
   ```
2. Work out the transcript path in `autofix.rs` with a new `fn transcript_of(app, agent) -> Option<PathBuf>`. Take the harness's `CLAUDE_CONFIG_DIR` (from the account descriptor, else `~/.claude`), then glob `projects/*/<agent.session_id>.jsonl`. This is the lookup `crates/orion-daemon/src/continue_on.rs` does, copied into the TUI so the daemon stays untouched.
   - If any live session on the branch isn't Claude-dialect, has no session id, or has no transcript file, **don't fire**. Try again on the next poll, which is today's blocking behaviour. Flash `autofix held: <name> is busy on this branch` once per fingerprint.
3. Add `orion autofix stand-down <reason>` to `crates/orion/src/cli.rs` and `main.rs`. It only works inside a session: it reads the agent id from the session env, the way `orion rename` does. It first sends the existing `RenameAgent` to name the session `autofix-<n> · stood down: <reason>`, so the reason survives in the archived row. Then it sends the existing `ClientRequest::ArchiveAgent { id: self }`. The protocol doesn't change.
4. In the TUI, when an `autofix-*` agent is archived with a `stood down:` name, flash `autofix-412 stood down: <reason>`. Mark the breakage handled in the ledger under the current fingerprint, so autofix doesn't fire again until the next push or a new failure. If the PR is still red after that next push, autofix fires again and runs the same check.
5. Tests:
   - `a_running_session_adds_the_stand_down_preamble`
   - `a_waiting_session_adds_the_stand_down_preamble`
   - `a_live_session_without_a_transcript_holds_autofix`
   - `a_second_autofix_never_starts_for_one_pr`
   - `stand_down_marks_the_breakage_handled`
   - a CLI test that `orion autofix stand-down` errors outside a session
   - an e2e test in `crates/orion/tests/e2e_pty.rs`, next to the `continue_on` one: a fake agent calls stand-down, and its row ends up archived

**4c. System presets**

1. In `crates/orion-tui/src/agent_presets.rs:21`, add `#[serde(default, skip_serializing_if = "Option::is_none")] pub system: Option<SystemPreset>`, where `enum SystemPreset { PrAutofix, LinearTicket }` is serialized as `"pr_autofix"` and `"linear_ticket"`.
2. Have `load()` (`:167`) seed each missing system preset. On first run, **adopt** an existing user preset with a matching name instead (case-insensitive `"PR autofix"` and `"Linear Ticket"`) by setting its `system` field, so the user's text is kept. Otherwise insert a default written in `agent_presets.rs`, with the autofix default prefix built from the current `autofix::prompt` wording. Save after seeding.
3. In the presets overlay (`preset_overlays.rs`), system presets show a `system` tag and sort first. They can be edited (prefix, postfix, skip_task) but not deleted or renamed: the delete key flashes "System presets can't be deleted. Clear their text to turn them off." There is also a **Reset to default** action.
4. Autofix looks its preset up with `system == Some(PrAutofix)`. Drop `cfg.autofix_preset` and its settings row, but keep reading the key so an old config still parses.
5. The Linear launch (`linear.rs:3652-3690`) wraps the expanded `cfg.linear_template()` in the `LinearTicket` preset's prefix and postfix by default. Shift+Tab still opens the picker to choose another preset.

### Tests

- `autofix.rs`: `auto_mode_sends_without_a_modal`, `an_old_ask_config_reads_as_on`, `dispatch_copies_the_worktree_agents_harness_and_model`, `dispatch_falls_back_when_no_worktree_agent`, and `three_sends_without_green_notify_instead_of_asking`. The stand-down tests are listed in 4d.
- `agent_presets.rs`: `load_adopts_a_user_preset_by_name_as_system`, `load_seeds_missing_system_presets`, and `system_presets_round_trip_through_json`.
- `preset_overlays.rs`: `deleting_a_system_preset_is_refused`.

### Docs

Update `docs/configuration.md` (the setting, the fallback rows and system presets) and the autofix section of `ARCHITECTURE.md`.

---

## 5. A glanceable account tag on every session row

### Today

Accounts are harness ids. An extra account is `kind == Custom, custom_harness == Some("claude-2")`, and the default account is `Claude, None` (`crates/orion-core/src/claude_account.rs`, `crates/orion-tui/src/claude_accounts.rs:446` `short_name`). Cards show the full name on their second line. LIST rows leave it out on purpose (doc on `list_runs_line`, `launcher_view.rs:2938`) because a full name pushed the model out of sight.

### Target (see artifact, section 5)

A fixed **2-character tag** in its own column right after the status dot, tinted by account. It appears only when the machine has more than one Claude account, so single-account users see no change.

```
▌ ● ok  billing-agent   opus high  › wire the CSV exporter     2m
  ● rk  tests           sonnet     › add fixtures              5m
  ◌ ok  claude          opus       › …                         2d
```

### Steps

1. In `claude_accounts.rs`, add:
   ```rust
   /// Two letters that tell this session's account from the others on a
   /// glance, and a tint for them; None when only one account exists or
   /// the session isn't on a Claude account.
   pub fn account_tag(kind: AgentKind, custom: Option<&str>) -> Option<(String, Color)>
   ```
   - The letters are the account's name initials (first letter of the first two words, or the first two letters of a single word). With no name, use the first two letters of the email's local part, in lowercase. If two accounts would get the same tag, use the 1st and 3rd letters for the later one, then the 1st plus a digit.
   - The colour comes from `theme::ACCOUNT_TINTS`, a new fixed array of 6 ANSI-256 values that avoids the status colours (`err`, `warn`, `done`, `merged`). It is indexed by the account's position in `claude_accounts` config order, with the default account at 0, so a tag keeps its colour across restarts.
   - Reuse `short_name`'s gates for returning `None`.
2. In `launcher_view.rs`, add a `LIST_ACCT = 3` column (2 characters plus a gap) between the dot and the name in `draw_list_row` (`:1569`). Add it to the `cols` tuple in `draw_list_band` (`:1501-1515`) only when some row in the band has a tag, so bands with none keep their width.
3. On cards (`draw_card`, `:2432`), put the same tag before the name on line 1. Keep the full name on line 2.
4. Show the tag in the Linear picker's agent lines (step 3) and in the account list of the usage modal, so the user learns which letters mean which account.
5. **Settings → Accounts**: each account row shows its tag and lets the user override it (2 characters) and pick its tint. Store these in `claude_accounts[i].tag` / `.tint` in config.json. Unknown fields are kept, so older builds aren't affected.

### Tests

- `claude_accounts.rs`: `tag_uses_name_initials`, `tag_falls_back_to_email`, `tags_never_collide`, and `no_tag_with_one_account`.
- `launcher_view.rs` render test: a band with two accounts draws the tags, and a single-account band's columns are unchanged.

---

## Release

Patch release. Nothing under `crates/orion-core`, `crates/orion-daemon` or `vendor/vt100` changes. The new `orion autofix stand-down` command lives in `crates/orion` and reuses existing requests. Confirm this before pushing:

```sh
git diff --name-only origin/main... | grep -E '^(crates/orion-core|crates/orion-daemon|vendor/vt100|Cargo\.(toml|lock))' && echo "MINOR" || echo "patch"
```

---

## Status (2026-11-04)

All five are built, each in its own commit on `main`. Where the build differs from the steps above:

- **2.** Creation order comes from the tree's own order (the daemon loads worktrees by `created_at`), not from ULID ids: test fixtures and stand-in rows don't all carry ULIDs. A stand-in checkout (no path yet) and a stand-in session that is `Running` with no PTY yet count as live, so a new launch doesn't flash under the divider. The quick launcher's worktree picker uses the same order. The `rows_key` memo change wasn't needed: the memo only lives for one frame.
- **4.** System presets are added at the **end** of the list, so existing presets keep their positions (and ⌘U's first row). There's no **Reset to default** action: clearing a system preset's text has the same effect. The retired `autofix_preset` setting is removed. `orion autofix stand-down` is hidden from `orion --help`. No e2e test for stand-down: the unit tests cover the prompt, the hold, and the flash.
- **5.** Tag and tint overrides are set in `config.json` (`account_tags`), not edited from Settings. Settings → Agents and the usage modal show each account's tag.
- `tui_diff_steps_through_a_branch_one_commit_at_a_time` (e2e) also fails on `5438ae7`, before any of these changes.
