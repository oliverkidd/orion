# Project todos

## Goal

A per-project todo list inside orion, modelled on Capacities' daily todo.

- Items are grouped into collapsible, nestable sections.
- Each item has a priority and sorts by it within its group.
- Group headers total open items by priority and items done today.
- Ticking an item strikes it through for the rest of the day. After that it moves to a day-by-day log.
- Anything unfinished is simply still there tomorrow.

Each item can:
- **Dispatch an agent** with the item as its prompt.
- **Create a Linear issue** in the team's Triage.
- **Link an existing Linear issue.**

The list lives on this machine only. It is never written into the repo.

All paths below are relative to the repo root; `T = crates/orion-tui/src`.

## What it looks like

A centered modal (`ui::centered_rect_pct`, the same size as the Linear modal) with two tabs: **Today** and **Log**.

```
╭ Todos · riplo-os ─────────────── Today · Tue 6 Oct │ Log ─────────────────╮
│ filter…                                                                    │
│ ▾ Emails                                    ▆ 2  ▄ 3      5 open  ✓ 1 today │
│   ☐ ▆▄▂ run plan                                         ● agent  ◇ RIP-412 │
│   ☐ ▆▄▂ setup resend                                              ○ RIP-398 │
│   ☑ ▆▄▂ setup templates                       (struck through, dim)         │
│   ☐ ▆▄  change all emails over                                          2d │
│   ☐  ··· check lists created and hooks (local harness…)                    │
│ ▸ link sharing                                     ▄ 1        1 open        │
│ ▾ MCP fixes                               ‼ 1  ▆ 2  ▄ 3       9 open        │
│   ☐ ‼   MCP insert slides from previous decks                     ◑ RIP-377 │
│   ▾ Rethink templates                                 ▄ 2     4 open        │
│     ☐ ▆▄  get toby new closing page in default themes                      │
│ + new item                                                                 │
╰ space done · enter agent · ⌘N new · ⌘L Linear · ⌘1-4 priority · ⇧←/⇧→ tabs ╯
```

### One item row (one line, left to right)

- **Done/not done:**
  - `☐` in `th.dim` when open.
  - `☑` in `th.ok` when done today. The text is also dimmed and `Modifier::CROSSED_OUT`.
- **Priority:** Linear's own priority glyph, so the two lists read the same.
  - Reuse `priority_mark` (`T/linear.rs:1744`): `‼` urgent in `th.err`; 3, 2 or 1 lit bars of `▂▄▆` for high, medium and low; `···` for none.
  - The levels are Linear's: Urgent, High, Medium, Low, None. A created issue then carries the same priority.
- **Text:** one line, truncated with `…`.
- **Chips, right-aligned and built with `ui::fit_parts`** (`T/ui.rs:3823`) so they drop from the right when narrow:
  - **Agent** (if the item has a session that still exists in `app.tree.agents`):
    - `● agent`, coloured by `status_color` (`T/ui.rs:3732`), so running, needs-feedback and finished read the same as the session list.
    - The chip goes when the session is deleted.
  - **Linear** (if linked):
    - The issue's state glyph and colour from `state_mark` (`T/linear.rs:1728`): `◇` triage, `○` todo, `◑` in progress, `●` done.
    - Then the identifier `RIP-412`, then the state name when there is room.
  - **Age** (open items created before today): `2d`, in `th.faint`. This shows what has carried over.

### Group header (selectable, so it can collapse)

- `▾`/`▸`, then the name in bold.
- Then per-priority open counts, using the priority glyph and count, omitting zeros.
- Then `N open`, then `✓ N today` when non-zero.
- Counts include nested subgroups.
- A collapsed header still shows its counts.

### Sort order within a group

1. Open items by priority (Urgent → None), then by creation order.
2. Items done today, at the bottom of their group, in tick order.
3. Then subgroups, in their own order.

Groups keep the order they were created or imported in.

### Log tab

- Days, most recent first. Each day has a header like `Mon 5 Oct · 6 done`.
- Rows are the items done that day: `☑ priority text`, followed by a dim `Emails › setup`-style group path.
- Space on a log row un-ticks the item. It returns to Today as open.
- The tab badge on Today shows the open count.

## Keys

These follow the modal conventions: ⌘ chords with a same-letter Ctrl twin, and the same action on the same key everywhere.

| Key | Action | Consistent with |
|---|---|---|
| `⌘I` (global) | Open the TODOS modal for the selected project | ⌘I is the one free ⌘ letter not kept by macOS/Ghostty. Ctrl twin `^Q`, because `^I` is Tab |
| `↑`/`↓`, PgUp/PgDn | Move | every list modal |
| `←`/`→` | Collapse/expand the group at the cursor (on an item: its group) | tree browser |
| `space` | Done ⇄ not done | Linear modal's Space |
| `enter` | Agent on this item. If its agent session still exists, jump to it instead | Linear modal's Enter, "agent on marked" |
| `shift+tab` | Agent with a preset | Linear modal's PRESET |
| `⌘N` | New item in the cursor's group (inline `+ ` input row) | PR modal's ⌘N "new PR" |
| `⌘⇧N` | New group, a sibling of the cursor's group | |
| `⌘R` | Rename the item or group at the cursor (inline edit) | |
| `⌘1`–`⌘4`, `⌘0` | Priority Urgent/High/Medium/Low, `⌘0` none | |
| `⌘L` | Linear menu: **Create in Triage** / **Link existing…** / **Open in browser** / **Unlink** | PR modal's ⌘L "Linear" |
| `⌘⌫` | Delete the item or group (group: confirm when it has items) | delete_worktree's ⌘⌫ |
| `⇧←`/`⇧→` | Today ⇄ Log | `pr_preview::keys::MODAL_TABS` |
| typing | Filter rows (matching items keep their group headers) | every list modal |
| `esc` | Clear the filter, then cancel the inline edit, then close | every list modal |

- **Ghostty:** every new modal ⌘ chord (`⌘I`, `⌘R`, `⌘1`–`⌘4`, `⌘0`, `⌘⇧N`) must be released in the Ghostty keybind block (`T/ghostty_config.rs`, `GHOSTTY_BINDS` around `T/keymap.rs:1352-1422`). `⌘1`–`⌘9` are Ghostty's tab keys, so check that `host_warning` stays quiet.
- **Ctrl twins:** pick each from what's free at build time and list them in `docs/keys.md`.
- **Mouse:**
  - Clicking a row moves the cursor there.
  - Clicking the checkbox ticks the item.
  - Clicking a header's `▾`/`▸` toggles it.
  - Clicking a Linear chip opens the issue in the browser.
  - Clicking an agent chip jumps to the session.
  - Wheel scrolls.

## Pasting a list in (import)

Paste is already routed to the open overlay (`T/event_loop.rs:3315`). When the TODOS modal receives a multi-line paste and no inline input is open, parse it as an indented Markdown list and append it.

Parse rules:
- Indentation is a tab or 4 spaces per level (2 spaces also accepted: infer the unit from the smallest non-zero indent).
- `- `, `* ` and `+ ` bullets are stripped.
- Surrounding `**…**` is stripped.
- A bullet with children becomes a **group**. A bullet without children becomes an **item**.
- Top-level items with no group go into a group named `Inbox`.
- Non-bullet lines (e.g. a leading `Todo`) are ignored.
- Items import open, with no priority.
- Existing groups with the same name at the same level are merged into, not duplicated.

Put the parser in `T/todos/import.rs` as a pure `fn parse(text: &str) -> Vec<Node>` with unit tests. Use the user's real list as a fixture: the Emails / link sharing / UI (with nested "Later stuff") / MCP fixes (four subgroups) / Side quests example in **Fixture** at the end of this file. It should produce 5 top-level groups with the right nesting and 29 items.

A single-line paste into the inline input is just text, as usual.

## Storage

- **Where:** `orion_core::paths::data_dir()/todos/<fnv64(repo_path) as hex>.json`.
  - On macOS that is `~/Library/Application Support/dev.orion.orion/todos/`.
  - Reuse the FNV-1a in `T/review.rs:39`; move it to a shared helper if it is private.
  - The file also records `repo_path`, so files can be identified by hand.
- **Writes:** with `orion_core::settings::write_atomic` (`crates/orion-core/src/settings.rs:158-180`), off the loop the way `T/recent_files.rs` saves: `std::thread::spawn` behind a static `Mutex`, with the path overridable in tests through a thread-local (see `recent_files.rs:107-117`).
- **When:** save after every mutation. The data is tiny, so there is no debouncing.

```rust
// T/todos/store.rs
#[derive(Serialize, Deserialize, Default)]
pub struct TodoFile {
    pub version: u32,                 // 1
    pub repo_path: PathBuf,
    pub groups: Vec<Group>,
    pub items: Vec<Item>,
    pub linear_team: Option<String>,  // team id remembered for "Create in Triage"
}
pub struct Group { pub id: u64, pub parent: Option<u64>, pub name: String, pub collapsed: bool }
pub struct Item {
    pub id: u64, pub group: u64, pub text: String,
    pub priority: u8,                 // Linear's scale: 0 none, 1 urgent, 2 high, 3 medium, 4 low
    pub created: NaiveDate,           // local date
    pub done: Option<DateTime<Local>>,// local timestamp when ticked
    pub linear: Option<String>,       // "RIP-412"
    pub agent: Option<String>,        // AgentId of the dispatched session
}
```

- **Dates are local, not UTC.** A late-evening tick must count as that day. Add `chrono = { version = "0.4", default-features = false, features = ["clock", "std", "serde"] }` to `crates/orion-tui/Cargo.toml`; there is no date crate today.
  - `config::today_days` (`T/config.rs:726`) is UTC, so don't use it here.
- **"Today":** an item counts as done today when `done.date_naive() == Local::now().date_naive()`. The day rolls over by itself at the next draw after midnight, with no stored day state.
- **Load:** on open. A missing file is an empty list. A corrupt file is renamed `*.corrupt-<ts>.json` and the user gets a flash; never overwrite it silently.

## Linear

The client is in `T/linear.rs`:
- The key comes from `read_linear_key(dir)` (`:2363`), where `dir` is the project's `repo_path`.
- Transport is `graphql()` (`:2108`), stubbed in tests through `with_graphql_stub`.
- Answers come back through `app.linear_tx` → `LinearAnswer` (`:380`) → `land_answer` (`:599`).
- Add new `LinearAnswer` variants for the todo flows, or give todos their own channel following the same pattern. Either is fine; prefer the existing channel.

### Live status of linked items

When the modal opens, fetch every linked identifier in one aliased query, with fields matching `ISSUE_FIELDS` (`:1942`):

```graphql
query { i0: issue(id: "RIP-412") { identifier url state { name type color } priority }
        i1: issue(id: "RIP-398") { … } }
```

- Keep the answers in memory on the view (`HashMap<identifier, LinkedIssue>`). Prefill from `app.linear[project].list` when an issue is already there, so the chip draws immediately.
- **Auto-tick:** when a linked issue's state `type` is `completed` or `canceled` and the item is open, tick it, with `done = now`, and save. Ticking an item in orion does **not** change the Linear issue; the sync is one-way.

### Create in Triage

1. **Team:**
   - Use `TodoFile.linear_team` if it is set.
   - Otherwise run `teams { nodes { id key name triageEnabled states { nodes { id type } } } }`. With exactly one team, use it and remember it. With several, show a small pick list (reuse the status-pick sub-panel style, `StatusPick` in `T/linear.rs`) and remember the choice.
2. **Mutation:**
   ```graphql
   mutation($input: IssueCreateInput!) {
     issueCreate(input: $input) { success issue { identifier url state { name type color } priority } }
   }
   ```
   with `input`:
   - `teamId`
   - `title`: the item text
   - `priority`: the item's priority
   - `description`: `From orion todos · <Group › Subgroup>`
   - `stateId`: the team's state with `type == "triage"` when `triageEnabled`. This is set explicitly, because API creates by a team member land in the default state otherwise. If the team has no triage state, omit it and flash `Linear: <team> has no Triage — created in its default state`.
3. On success, set `item.linear = identifier`, save, and flash `Created RIP-431 in Triage`. Linear's triage auto-tagging then runs on it as it does for issues from Slack's @Linear.
4. Check with `mutation_result(json, "issueCreate", …)` (`:2018`).

### Link existing…

- Add `LinearMode::Link { item: u64, back: Box<TodoView> }` beside `LinearMode::Attach` (`T/linear.rs:254`), and open the Linear modal in it.
- In that mode:
  - Enter takes the cursor issue (`picked(app)`, `:1286`), sets `item.linear`, saves, and reopens `back`.
  - Esc reopens `back` unchanged.
  - Follow `close` (`:1217`) and `confirm` (`:1303`) for the Attach round trip.
- The hints read `enter link to todo`.

### Open in browser / Unlink

- **Open in browser:** use the `url` from the status fetch, through the same opener the Linear modal's BROWSER key uses (`crate::issues::keys::BROWSER`).
- **Unlink:** clears `item.linear`.

## Dispatch an agent

1. Build the launch the way the Linear modal's `launch_for` (`T/linear.rs:1376`) does: `QuickLaunch::from_config(QuickTarget::NewWorktree { project, branch: branch_name::random_name(&app.project_branches(&project)), existing: false }, &Config::load())`.
2. Prefill the prompt text with the item text, plus `\n\nContext: todo in <Group › Subgroup>`.
3. If the item is linked, add its Linear URL and set the launch's `issue_url`, as the Linear flow does.
4. Open it with `crate::quick_prompt::open_box(app, launch).with_under(ModalUnder::of(app.overlay.as_ref()))`, so the user can edit, Enter sends it, and Esc returns to the TODOS modal. Add a `ModalUnder::Todos` arm in `T/quick_prompt.rs:131-156`.
5. **Remembering the session:**
   - Add `todo: Option<TodoRef { repo_path: PathBuf, item: u64 }>` to `QuickLaunch` (`T/quick_prompt.rs:70`) and to `AgentLaunchDraft` (`T/app.rs:2457`), and carry it through `event_loop/quick_launch.rs` `draft` (`:246`).
   - In `create_agent` (`T/event_loop.rs:10743`), when `draft.todo` is set, record `app.todo_pending.insert(req_id, todo_ref)`.
   - In the `ServerEvent::Ack { req_id, created: Some(EntityId::Agent(id)) }` handler (`T/event_loop.rs:12905`), look up `todo_pending`, set `item.agent = id`, and save. On an error reply, drop the entry.
6. **Enter on an item whose `agent` is still in `app.tree.agents`:** select that session and close the modal. Reuse whatever the session list's jump-to-session path uses; the ⌘K palette's session entries in `T/palette.rs` are a good reference.

## Steps

1. **Store and model:** `T/todos/mod.rs` and `T/todos/store.rs`.
   - `TodoFile`, load/save, `today()`, sorting, and header counts.
   - Pure functions with unit tests: sort order, counts across nesting, done-today vs earlier, rollover (inject "now"), and the corrupt-file rename.
2. **Import:** `T/todos/import.rs` plus tests against the fixture below.
3. **View:** `T/todos/view.rs`.
   - `TodoView { project, project_name, dir, tab, selected, scroll, query: TextInput, input: Option<(InputKind, TextInput)>, row_rects, area, list_area, linked: HashMap<String, LinkedIssue>, menu: Option<LinearMenu> }`. The file itself lives on `App` as `app.todos: HashMap<PathBuf, TodoFile>`, so it survives the modal closing, like `app.linear`.
   - **Rows:** build a flat `Vec<Entry { Header{group, depth}, Item{id, depth}, AddRow }>` from the file, collapse state and filter. Collapsible headers are new; the existing `ui::sections` (`T/ui.rs:5024`) are not collapsible.
   - **Draw:** one-line rows with `ui::render_row_lines` (`T/ui.rs:3810`) and `fit_parts`, plus `list_header`-style header styling (`T/ui.rs:3853`). Use `tab_strip`, `search_line_lit`, and `hints::draw_on_border`, with a `todos::keys` table like `linear::keys` (`T/linear.rs:807`).
   - **Keys and mouse:** follow `linear::handle_key` (`:1002`) and `handle_mouse` (`:1048`). Hit-test rows with `ui::row_hit` (`T/ui.rs:5070`).
4. **Overlay wiring:** `Overlay::Todos(TodoView)` in `T/app.rs:2378`. Then each touchpoint:
   - draw: `T/ui.rs:2596`
   - keys: `T/event_loop.rs:7350`
   - mouse: `T/event_loop.rs:12119`
   - paste: `T/event_loop.rs:3315`
   - overlay area: `T/overlay_close.rs:44`
   - browser-button area: `T/ui.rs:3537`
   - overlay name: `T/event_loop.rs:37804`
   - `ModalUnder`: `T/quick_prompt.rs:131`
5. **Action:**
   - `Action::Todos` with an `ActionSpec` in `T/keymap.rs` (id `todos`, label `Todos`, hint `This project's todo list: tick off, prioritise, send an agent or a Linear issue from any item`, group `PROJECTS & WORKTREES`, defaults `["cmd+i", "ctrl+q"]`).
   - Dispatch it at `T/event_loop.rs:~4092` to `crate::todos::open(app)`.
   - It shows up in the ⌘⇧P command palette automatically.
   - Release ⌘I and the modal chords in `T/ghostty_config.rs`.
6. **Linear:**
   - New functions in `T/linear.rs`: `fetch_linked`, `teams`, `create_issue` and `LinearMode::Link`.
   - New `LinearAnswer` variants, landed into `app.todos` and the open `TodoView`.
7. **Agent dispatch:** the `TodoRef` plumbing and Ack handling described above.
8. **Docs:** add the keys to `docs/keys.md`, and a short **Todos** section to `docs/sessions.md` (or the closest user-facing doc) covering storage location, import-by-paste, and the Linear and agent actions.

Build it in this order. Each step compiles and is testable on its own. Steps 1–5 are a usable list; 6 and 7 add the integrations.

## Tests

- **Unit** (`T/todos/*`):
  - sorting
  - counts
  - rollover with an injected clock
  - import fixture
  - store round trip
  - corrupt-file handling
- **Overlay**, in `T/event_loop.rs`'s test module, modelled on the Linear modal tests (`App::new()` + `seed_tree`, `TestBackend`, `buffer_text`):
  - ⌘I opens TODOS for the selected project.
  - Space ticks an item: it draws `☑` and crossed out, and the header shows `✓ 1 today`.
  - Advancing the injected clock a day removes it from Today, and it appears under that day in Log.
  - ←/→ collapse and expand, and a collapsed header still shows its counts.
  - Pasting the fixture creates the groups.
  - Typing filters items but keeps their headers.
- **Linear**, through `with_graphql_stub`:
  - Create sends `issueCreate` with the triage `stateId` and the item's priority, and links the returned identifier.
  - A team without triage omits `stateId` and flashes.
  - A linked issue answering `type: "completed"` auto-ticks the item.
  - Link mode: Enter returns to TODOS with the item linked.
- **Agent:**
  - Enter opens the quick-prompt box prefilled with the item text and the TODOS modal under it.
  - A simulated `Ack { created: Some(Agent(id)) }` sets `item.agent`, and the row shows the `● agent` chip.

## Verification

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p orion-tui todos
cargo test --workspace
```

Manual check (`cargo run`):
1. Press ⌘I and paste the fixture list. Five groups appear.
2. Set some priorities with ⌘1–⌘4. Items re-sort and the header counts update.
3. Tick two items. They strike through, and the headers show `✓ 2 today`. Close and reopen: the state persists. The file exists under `~/Library/Application Support/dev.orion.orion/todos/`, and `git status` in the project is clean.
4. Press ⌘L → Create in Triage. The issue appears in Linear's Triage with the right priority, and the row shows `◇ RIP-…`.
5. Press ⌘L → Link existing… on another item, pick an issue, and the chip appears. Move that issue to Done in Linear, then reopen TODOS: the item is ticked.
6. Press Enter on an item. The quick-prompt box opens prefilled; send it, reopen TODOS, and the row shows `● agent`. Press Enter again to jump to the session.
7. To test rollover, temporarily set the system date forward a day or use the test clock. Ticked items move to Log and open items show `1d`.

## Fixture

```
Todo
- Emails
    - run plan
    - setup resend
    - setup templates
    - change all emails over
    - check lists created and hooks (ideally with local harness to check emails without having to deploy (even if just logs)
- **link sharing**
    - run desctructive migration to fix old links: docs/2026-09-29-share-link-doors/contract-follow-up.md [MAYBE NOT NEEDED]
- **UI**
    - get Zack to s
    - table panel fix to be small chips
    - workOS login pages etc
    - Later stuff
        - seeem to be reading the deck an awful lot. should we not have a read slide tool for just this slide requests?
        - insert from previous decks ui is a bit confusing could move to grid of just matches asdiscussed with toby
        - file history modal a bit too big / same vein as above could cleanup
        - learn about riplo ombaording card is not env scoped it should be
        - output design style to figma
- **MCP fixes**
    - MCP insert lsides from previoius decks and deck search etc
    - Rethink templates in scope of MCP creating custo m teampltes (links to deck review)
        - get toby new closing page in default themes
        - not able to delete stuff on later templates from master properly (block enable/disable format)
        - Not able to make custom layoyuts
        - import from ppt detection (theme too)
    - storyline prompt
        - review when its actually used
        - use toby message .md file
    - low desnity component liek claude
        - Review toby outbound decks for componentsi
        - Compare to AutoPresent slide library to see if any missing
    - Long tail improve components
        - server render best of bad bunch vs returning nothing
        - get toby messages in slack and quirk fixes message in
        - look at bhavna deck comments
- **Side quests**
    - chat to benj about setting up a meeting with small 3 man team to get them on platform
    - simplify design system
    - start workign on params thoughts?]
```

Expected: 5 top-level groups (Emails, link sharing, UI, MCP fixes, Side quests). UI has one subgroup (Later stuff). MCP fixes has four subgroups (Rethink templates…, storyline prompt, low desnity component liek claude, Long tail improve components). `Todo` is ignored. That gives 29 items in total: Emails 5, link sharing 1, UI 8, MCP fixes 12, Side quests 3.
