# Richer PR and Linear lists: sections, two-line rows, filters

## Context

Both list modals show one plain line per item:

- **Pull requests modal** (`⌘U`, or the sidebar's `N prs` badge): `#42 Fix login        failing`.
- **Linear view** (`⌘L`): `◑ ENG-12 Title        In Progress`. It lists only issues assigned to you.

Neither shows who wrote the item, how old it is, its checks, its review state, its priority or its labels. Nothing can be filtered except by fuzzy text.

GitHub's list (title, then `#1295 · author opened 8m ago · ● 28/29` with a comment count) and Linear's cards (identifier, status glyph, priority bars, coloured label chips, created date) are the references. The changes:

- **Pull requests modal:** sections `Yours` / `Review requested` / `Others`, and two-line rows carrying author, age, checks count, review decision, comments and labels.
- **Linear view:** two tabs, `My issues` (assigned to me) and `Other issues` (active issues not assigned to me), each grouped by status. Two-line rows carry priority bars, labels, project, assignee and date.
- **Both:** filtering by typed tokens (`label:bug p:high status:todo`), plus a `⌘F` facet picker that writes those tokens for you.

Save a copy of this plan to `docs/2026-10-06-pr-linear-lists/pr-linear-lists.md` before starting. The repo keeps plans there.

All paths below are relative to `crates/orion-tui/src/` unless stated otherwise.

## Design

### Pull requests list (left pane of the modal)

```
Pull requests — riplo-os (14)
⌕ type to filter…
 YOURS 2
▌● #1295 feat(analytics): carry UTM tags through /si…   failing
   8m ago · ✗ 27/29 · ○ review · 5 comments · analytics
 ● #1290 Fix login redirect
   2d ago · ✓ 29/29 · ✓ approved
 REVIEW REQUESTED 1
 ● #1288 Speed up the grid
   jacobrvl · 1d ago · ◐ 12/29 · 1 comment
 OTHERS 11
 ○ #1281 WIP: new onboarding                             draft
   sam · 6d ago · ✓ 29/29
```

**Line 1**
- A state glyph:
  - `●` for open and ready, coloured `th.ok`.
  - `○` for a draft, coloured `th.faint`.
  - In `th.err` when the row is in trouble.
- `#N` and the title, as `row_spans` does today, with fuzzy highlights.
- The right-aligned badge, unchanged: `failing` / `conflicts` / `draft`.

**Line 2** is dim meta, joined with ` · `:
1. The author. Left out in the `Yours` section.
2. The age, from `createdAt` via `hosts::ago_label`.
3. Checks as `passed/total`:
   - `✓` in `th.ok` when all pass.
   - `◐` in `th.warn` while any are pending.
   - `✗` in `th.err` when any fail.
   - Left out when there are no checks.
4. The review decision:
   - `✓ approved` in `th.ok`.
   - `✗ changes` in `th.err`.
   - `○ review` in `th.muted`.
   - Left out when there is none.
5. `N comment(s)`, when N > 0.
6. Labels, each name in its GitHub colour.

When the width runs out, the meta items drop from the right: labels first, then comments, then the review decision.

**Sections**
- `Yours` is `viewerDidAuthor`.
- `Review requested` is the viewer among the user review requests.
- `Others` is everything else.
- Headers are ` NAME count` in `th.muted` bold, the style of `FinderRow::Header` (`ui.rs:2378`). The cursor never lands on a header.
- Empty sections are hidden.
- Within a section, drafts sink to the bottom (the `drafts_last` rule) and newest come first.
- With free text in the filter, rows within each section order by fuzzy score.

### Linear view (left pane)

```
Linear — riplo-os
 My issues 12 │ Other issues 48                 (tab strip, ⇧←/⇧→ or click)
⌕ p:high label:bug▏
 ◑ IN PROGRESS 2
▌◑ RIPLO-1018 Toggle PDF export slide selection with…
   ▂▄▆ High · ● Export PDF · ● bug · Oct 5
 ○ TODO 5
 ○ RIPLO-1002 Fix slide thumbnails
   ‼ Urgent · ● Export PDF · Sep 28           (Other issues adds "· Sam")
```

**Tabs.** `My issues` / `Other issues`. Each tab label carries its count after filtering. Switch with `⇧←`/`⇧→`, the key the pull requests modal already uses for tabs, or by clicking a tab.

**Sections** follow the status order:
- The existing `status_rank` order (`started`, `unstarted`, `backlog`, other), then the state's `position`.
- Each header is the state glyph plus the uppercased name and a count. The glyph is drawn in Linear's state colour.

**Line 1** is the mark tick (`✓ ` when marked), the state glyph in the state colour, `IDENT` in dim, and the title with fuzzy highlights.

**Line 2** is joined with ` · `:
1. Priority:
   - `‼ Urgent` in `th.err`.
   - `▂▄▆ High`, `▂▄ Medium` and `▂ Low` as filled bars in `th.muted`, with the unfilled bar slots in `th.faint`.
   - `··· No priority` in `th.faint`, written as ``.
2. The project as `● name` in the project's colour.
3. Labels as `● name` in each label's colour.
4. The assignee's display name. Only on `Other issues`, and `unassigned` when there is none.
5. The created date as `Oct 5`.

Within a status section, rows sort by priority (Urgent → Low, then none), then by `updatedAt` descending.

### Filtering (both modals)

The query line holds free text plus `key:value` tokens. It stays the one source of truth.

**Token rules**
- Keys and values are case-insensitive and match as a prefix: `p:hi` matches High.
- Values with spaces are written either quoted (`status:"in progress"`) or hyphenated (`status:in-progress`).
- Several values of the same key OR together. Different keys AND together.
- A leading `-` negates: `-label:wip`.
- Unknown keys are treated as free text.
- Everything that isn't a token goes to the fuzzy matcher, as today.

**Pull request keys**

| Key | Values |
|---|---|
| `author:` | any login |
| `label:` | any label |
| `review:` | `approved`, `changes`, `required` |
| `checks:` | `passing`, `failing`, `pending` |
| `is:` | `draft`, `ready`, `conflicts` |

**Linear keys**

| Key | Values |
|---|---|
| `status:` | any state name |
| `p:` / `priority:` | `urgent`, `high`, `medium`, `low`, `none` |
| `label:` | any label |
| `project:` | any project |
| `assignee:` | any assignee (useful on `Other issues`) |

**`⌘F` filter picker**
- It takes over the reading pane, the way `StatusPick` does (`linear.rs:86`, `draw_status_pick` `:1368`).
- Facets sit on the left. The selected facet's values sit on the right, each with a count taken over the current tab or rows.
- Active values show a `✓`.
- Keys inside the picker:

| Key | Action |
|---|---|
| `↑`/`↓` | pick a value |
| `←`/`→` | switch facet |
| `space` | toggle the value, which adds or removes the token in the query |
| `Esc` / `Enter` | close the picker |

- The list re-filters live behind it.
- Recognised tokens in the search line are drawn in `th.accent`, so active filters read like chips.

`⌘F` is the global full-screen chord (`keymap.rs:787`), but the modals consume their own chords first, as `⌘E` diff in the pull requests modal already does. Confirm this during implementation. Ghostty already releases `super+f` (`ghostty_config.rs:130`).

## Implementation steps

### 1. Shared building blocks (new code, no UI change yet)

1. **`list_filter.rs`** (new module, registered in `lib.rs`/`main.rs` next to `issues`):
   - `pub struct Token { key: String, value: String, negated: bool }`.
   - `pub fn parse(query: &str, keys: &[&str]) -> (String /* free text */, Vec<Token>)`. It handles quotes, `-` negation and unknown keys.
   - `pub fn matches(tokens: &[Token], key_values: impl Fn(&str) -> Vec<String>) -> bool`. Same key ORs, different keys AND, prefix match, case-insensitive.
   - `pub fn toggle(query: &mut TextInput, key: &str, value: &str)`. It adds or removes a token and hyphenates values that contain spaces.
   - Picker state: `pub struct FilterPick { facet: usize, value: usize }`.
   - `pub fn draw_pick(f, area, facets: &[Facet], pick, th)`, where `Facet { key, title, values: Vec<(String, usize /*count*/, bool /*active*/, Option<Color>)> }`.
   - Unit tests for `parse`, `matches` and `toggle`.
2. **Hex colour helper** in `theme.rs`:
   - `pub fn hex(s: &str) -> Option<Color>`. It accepts `#rrggbb` or `rrggbb` and returns `Color::Rgb`.
   - Under the `mono` theme the callers fall back to `th.muted`.
   - This replaces the ad-hoc `u8::from_str_radix` uses only where convenient. Don't churn `clipboard_image.rs` or `usage.rs`.
3. **Stacked-row layout** in `ui.rs`, next to `row_rect` (`:4883`):
   - `pub(crate) fn stacked_rows(heights: &[u16], cursor: usize, prev_start: usize, area: Rect) -> (usize /*start*/, Vec<(usize, Rect)>)`. It keeps the cursor's whole entry on screen and moves `start` only as far as it needs to.
   - Model it on the commit list's `entry_heights` and render loop (`ui/diff_view.rs:334`, `~:400-438`).
   - Each modal writes the returned rects back, so the mouse hit-tests by rect instead of by `cursor_row` arithmetic.
4. **Styled search line:** add a `search_line_styled(&TextInput, placeholder, area, th, accent_ranges)` beside `search_line` (`ui.rs:4867`) to colour token ranges.
5. **Unit tests** for `stacked_rows`: the cursor at the top, the bottom, beyond the window, and a header before the cursor.

### 2. Pull request data (`pull_request.rs`)

1. Extend `LIST_QUERY` (`:537`):
   - Add a top-level `viewer { login }` next to `repository`, so it costs no extra request.
   - Add these node fields:
     ```
     createdAt viewerDidAuthor reviewDecision author { login }
     comments { totalCount }
     labels(first: 10) { nodes { name color } }
     reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } } } }
     commits(last: 1) { nodes { commit { statusCheckRollup { state
       contexts(first: 1) { checkRunCountsByState { state count }
                            statusContextCountsByState { state count } } } } } }
     ```
   - The `*CountsByState` fields are aggregates GitHub computes itself. They don't enumerate contexts, which was the cause of the #106 504s.
2. **Fallback:**
   - Keep the current query as `LIST_QUERY_SLIM`.
   - If the rich query fails (`None`), `list()` retries once with the slim one, so the list never stops refreshing.
   - Rows parsed from the slim query just have empty meta.
3. Add these fields to `OpenPr` (`:546`), all `#[serde(default)]`:
   - `author: String`
   - `created_at: String`
   - `mine: bool`
   - `review_requested: bool` (viewer login ∈ requested user logins)
   - `review: Review { None, Approved, Changes, Required }`
   - `comments: u32`
   - `labels: Vec<Label { name, color }>`
   - `check_counts: Option<CheckTally { passed, failed, pending, total }>`
4. Fill them in `parse_list` (`:723`):
   - `parse_list` now also reads `/data/viewer/login`.
   - Map `CheckConclusionState` and `StatusState` counts into the tally:
     - SUCCESS/NEUTRAL/SKIPPED are passed.
     - FAILURE/ERROR/TIMED_OUT/CANCELLED/ACTION_REQUIRED/STARTUP_FAILURE are failed.
     - Everything else is pending.
   - Reuse the existing `rollup_state` (`:459`) for `health`.
5. Add `pub fn section(&self) -> PrSection { Yours, ReviewRequested, Others }` on `OpenPr`.
6. Bump `pr_cache.rs` `VERSION` (`:64`) from 3 to 4.
7. Update every `OpenPr { … }` literal; `rg "OpenPr \{"` finds them.
8. Update the `pr()` fixture (`pr_modal.rs:1320`) and the `list_answer` fixtures.
9. Tests:
   - Extend the parse tests (`:1560+`) with a node carrying all new fields, and with a slim-shaped node.
   - Keep the `:1664` assertion that `statusCheckRollup { state` stays in the query.

### 3. Pull request modal UI (`pr_modal.rs`)

1. **Visible model.** Replace `visible_rows` (`:376`) with `fn visible_entries(query, rows) -> Vec<Entry>`, where `Entry` is `Header(PrSection, usize)` or `Pr(index, positions)`:
   - Parse with `list_filter::parse`.
   - Apply the token predicate.
   - Fuzzy-rank the free text.
   - Bucket the result by `section()`, keeping stable order within each bucket.
2. **Cursor movement.** `cursor_index`, `step` and `select` (`:386`, `:524`, `:502`) work over `Pr` entries only, so headers are skipped. `list_changed` (`:470`) still follows the cursor by URL.
3. **Rows.**
   - `row_spans` (`:1050`) becomes line 1, with the state glyph added.
   - Add `meta_spans(pr, section, budget, now, th)` for line 2, with the drop-from-the-right fitting.
   - Draw entries with `stacked_rows`: headers are height 1, rows height 2.
   - Draw each row with `render_row`'s two-line sibling. Generalise `render_button` (`ui.rs:3816`) or add `render_row_lines` so the `▌` bar and `sel_bg` span both lines.
4. **Write-back.** Store `row_rects: Vec<(usize, Rect)>` on `PullRequestsView` and use it in `handle_mouse` (`:844`) for clicks and double-clicks.
5. **Filter picker.**
   - Add `keys::FILTER = Key::new(&["cmd+f", "ctrl+f"], "filter")` to `mod keys` (`:924`), `ALL` and `hints()` (`:975`).
   - Add `filter_pick: Option<FilterPick>` to the view.
   - When it is set, draw `list_filter::draw_pick` in the page's place, as forms do (`:1206`), and route keys to it in `handle_key` (`:754`).
   - The PR facets are Author, Label, Review, Checks and State, with counts over the project's rows.
6. **Title count.** `matches/all` stays as is (`:1134`).
7. **Tests** (`pr_modal.rs` tests, `screen()` helper `:1401`):
   - Sections render with counts, and empty ones hide.
   - Line 2 shows author, age, `27/29` and `approved`.
   - `↓` skips headers.
   - `label:bug` and `checks:failing` narrow the rows.
   - `⌘F` then `space` writes the token into the query.
   - A click on the second line of a row selects that row.
   - Update `it_draws_the_rows_and_reads_the_one_under_the_cursor` (`:1989`).
   - Add a no-overflow test like `issues.rs:2430`.

### 4. Linear data (`linear.rs`)

1. Extend `ISSUE_FIELDS` (`:1432`):
   ```
   priority createdAt updatedAt state { name type color }
   labels { nodes { name color } } project { name color }
   assignee { displayName isMe }
   ```
2. Replace `fetch_assigned` (`:1435`) with `fetch_lists`. It makes one request with two aliased connections, each `first: 100, orderBy: updatedAt`, with the active-state filter (`nin: completed, canceled`):
   - `mine: issues(filter: { assignee: { isMe: { eq: true } } … })`. When `linear_assignee_email` is set, it uses `{ email: { eq: $email } }` instead.
   - `others: issues(filter: { team: { members: { some: { isMe: { eq: true } } } }, or: [{ assignee: { null: true } }, { assignee: { isMe: { eq: false } } }] … })`. This means active issues in my teams that are unassigned or someone else's. With an email set, it uses `email: { neq: $email }`.
   - Ask for `pageInfo { hasNextPage }` on `others`. When it is true, show `showing 100 most recent` as a dim note row under the search line, in the slot the pull requests modal uses for its stale note (`pr_modal.rs:1167`).
   - Confirm the exact filter names against Linear's schema before relying on them: run an introspection query, or try the query in the Linear API explorer.
3. Add these fields to `LinearIssue` (`:57`), all `#[serde(default)]`:
   - `priority: u8` (0 none, 1 urgent … 4 low)
   - `state_color`
   - `labels: Vec<LinearTag { name, color }>`
   - `project: Option<LinearTag>`
   - `assignee: String`
   - `mine: bool`
   - `created_at`
   - `updated_at`
4. Fill them in `issue_from` (`:1693`):
   - `mine` comes from which alias the node arrived under.
   - `parse_issues` / `issue_nodes` read both aliases.
   - `parse_states` merges team states from both.
   - Sort by `status_rank`, state position, priority (0 last), then `updated_at` descending.
5. `LinearList` (`:232`) keeps one `list`. Tabs filter on `mine`.
6. Update the `cfg(test)` GraphQL stub fixtures and `parse_viewer_list` (`:2542`). Add parse tests for the new fields and both aliases.

### 5. Linear view UI (`linear.rs`)

1. Add `tab: LinearTab { Mine, Others }` to `LinearView` (`:176`). It defaults to `Mine`. The attach mode (`LinearMode::Attach`) keeps both tabs.
2. **Tab strip.**
   - Row 0 of the list pane is the tab strip. Make `ui::tab_strip` (`ui.rs:2933`) `pub(crate)` and reuse it; it returns x-ranges for click hit-testing.
   - Row 1 is the search line.
   - The rows start at row 2, with the optional "showing 100" note above them.
3. **Tab keys.** `⇧←`/`⇧→` switch tabs: add `keys::TABS` to `mod keys` (`:694`) and `ALL`.
   - On a switch, the cursor goes to the first issue of the new tab.
   - Marks persist across tabs, since the set is keyed by id.
4. **Visible model.** `visible_rows` (`:1161`) becomes `visible_entries`: filter by tab, then the tokens, then the free text, then bucket by status, giving `Header(status, count)` and `Issue(index, positions)` entries.
5. **Rows.** Rewrite the row loop (`:1252-1303`) as line 1 and line 2 per the design, using `stacked_rows` and `theme::hex` for state, label and project colours.
   - Add a `priority_spans(priority, th)` helper.
   - `row_under` (`:951`) uses the written-back rects.
6. **Filter picker.**
   - Add `keys::FILTER` (`⌘F`) and `filter_pick: Option<FilterPick>` to the view.
   - The picker draws in the reading pane, like `draw_status_pick`. Only one of the status picker and the filter picker is open at a time.
   - The facets are Status, Priority, Label, Project, plus Assignee on `Other issues`. Counts are over the current tab.
7. **Empty states** (`:1239`): `"no open issues assigned to you"` stays for `Mine`. Add `"no other open issues in your teams"` for `Others`.
8. **Tests:**
   - The first Linear row-render tests, using a `screen()` like `issues.rs:3105`.
   - Tabs switch with `⇧→` and by click.
   - Status headers render, and `↓` skips them.
   - Priority bars and labels show on line 2.
   - `p:high` and `label:bug` filter.
   - The `⌘F` picker toggles a token.
   - No line overflows the pane.
   - `every_overlay()` (`event_loop.rs:37237`) still passes.

### 6. Docs

- `docs/keys.md:253-277`: describe the sections, tabs, `⌘F` filter and the token syntax for both modals.
- Add a short "Filtering" note to `README.md` only if it already describes these modals. It currently doesn't, so skip it.

## Suggested PR split

1. Shared blocks: `list_filter`, `theme::hex`, `stacked_rows`, the styled search line.
2. Pull request data and UI (steps 2 and 3).
3. Linear data and UI (steps 4 and 5), plus the docs.

Each part is shippable on its own.

## Verification

1. Run the workspace checks:
   ```
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test -p orion-tui
   cargo test -p orion --test e2e_tui
   ```
2. Check the GitHub query cost before shipping step 2:
   ```
   gh api graphql -F owner=<owner> -F repo=<busy repo> -F limit=100 -f query="$(rich query)"
   ```
   Run it against the busiest repo you use (riplo-os). It must answer well within `TIMEOUT`, and spot-check `checkRunCountsByState` against `gh pr checks <n>`.
3. Check the Linear query: run the new `fetch_lists` query with your key, `curl -H "Authorization: $LINEAR_API_KEY" https://api.linear.app/graphql -d @query.json`. Confirm that `mine` and `others` don't overlap and that `others` excludes completed and canceled issues.
4. Manual check in the running app (`make run` or the `run` skill):
   - Open `⌘U` on a project with your own PRs, review requests and others' PRs.
   - Check the sections, the line 2 meta, and that `↓` skips headers.
   - Type `checks:failing`.
   - Use `⌘F` → Label → `space`.
   - Narrow the terminal and confirm the meta drops from the right without overflowing.
   - Open `⌘L`: switch tabs with `⇧→` and by click, check the priority bars and label colours, filter `p:urgent`, and mark issues on both tabs, then `Enter`.
   - Check the `mono` theme and one colour preset.
