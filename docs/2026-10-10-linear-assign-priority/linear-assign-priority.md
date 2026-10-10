# Linear view: reporter, priority and assignee

## Goal

In the LINEAR VIEW (`⌘L`, `crates/orion-tui/src/linear.rs`) an issue today shows its status,
priority, assignee, project, labels and dates, and only its status can be changed (`⌘S`). Add:

1. **Reporter** — who created the issue, in the reading pane's properties and as a filter.
2. **Priority** — `⌘P` (`^P`) picks one of Linear's five priorities for the issue under the cursor.
3. **Assignee** — `⌘I` (`^G`) assigns the issue under the cursor: to you, to nobody, or to a teammate.

Both edits behave exactly as `⌘S` does now: a picker in the reading pane's place, the row changed
at once, `issueUpdate` sent off the loop, the old value put back if Linear refuses, and a list
asked before Linear took the edit never putting the old value back.

Everything is in `orion-tui`; nothing in `crates/orion-daemon/daemon-inputs.txt` changes, so this
is a patch release.

## Decisions

- **Separate keys, not one "change properties" menu.** Status is already `⌘S`, and the TODOS MODAL
  already sets priority on `⌘P`; the same action stays on the same key, and each edit is two
  keystrokes instead of three.
- **Assign is `⌘I`**, Linear's own letter for "assign to me". `⌘A` is every text field's
  select-all, the filter line's included, so it is left alone. The no-⌘ twin is `^G`: `^I` is Tab
  in a terminal that sends no ⌘, and `^A` is what Ghostty types for `⌘←`.
- **The assignee picker opens on `Me`**, so `⌘I` `Enter` is "assign to me". Under it `No assignee`,
  then the issue's team's members by name.
- **The issue under the cursor only**, as `⌘S` is. Marks are not acted on.
- **A changed row keeps its place** until the next list lands (as a `⌘S` row does); an issue
  assigned to you moves to `My issues` at once, one taken off you to `Other issues`.

## What Linear is asked

All in `crates/orion-tui/src/linear.rs`.

`ISSUE_FIELDS` gains the reporter and the assignee's id:

```
assignee { id displayName } creator { displayName } externalUserCreator { name }
```

`creator` is null for an issue filed through an integration (Slack, email intake); then
`externalUserCreator` names who asked. Before coding, check the three names against Linear's
schema (`Issue.creator`, `Issue.externalUserCreator`, `ExternalUser.name`): a wrong field fails the
whole list.

`lists_query` gains two top-level fields beside `mine` and `others`, so the members are read once
per team rather than once per issue:

```
# Settings → Linear account empty:
me: viewer { id displayName }
# …or naming someone:
me: users(first: 1, filter: { email: { eq: $email } }) { nodes { id displayName } }

teams(first: 25, filter: { members: { some: { <me> } } }) {
  nodes { id members(first: 100) { nodes { id displayName } } }
}
```

`<me>` is the same `isMe: { eq: true }` / `email: { eq: $email }` fragment the query already
builds. `members` leaves deactivated users out by default. An issue in a team you are not a
member of has no member list; its picker still offers `Me` and `No assignee`.

The one mutation, replacing `update_state`:

```rust
/// Set one property of issue `issue_id`.
async fn update_issue(dir: &Path, issue_id: &str, change: &Change) -> Result<(), String>
```

It builds `mutation($id: String!, <decl>) { issueUpdate(id: $id, input: { <field> }) { success } }`
with flat variables, so the tests' `moves_sent()` (which reads `stateId` off the variables) keeps
working:

| Change | `<decl>` | `<field>` | variables |
| --- | --- | --- | --- |
| `Status(state)` | `$stateId: String!` | `stateId: $stateId` | `{ id, stateId }` |
| `Priority(n)` | `$priority: Int!` | `priority: $priority` | `{ id, priority }` |
| `Assignee(user)` | `$assigneeId: String` | `assigneeId: $assigneeId` | `{ id, assigneeId }` — `null` for nobody |

The answer still goes through `mutation_result(&json, "issueUpdate", …)`.

## Steps

### 1. Reporter (read-only, lands on its own)

1. `LinearIssue`: add `#[serde(default)] pub reporter: String` (the creator's display name, else
   the external creator's name, else empty) and `#[serde(default)] pub assignee_id: String`.
2. `ISSUE_FIELDS`: the fields above. `issue_from`: `assignee_id: text("/assignee/id")`, and
   `reporter` from `/creator/displayName`, falling back to `/externalUserCreator/name`.
3. `properties()`: a `Reporter` cell built like `assignee` (dim `unknown` when empty). Rows become
   `pair(status, priority)`, `pair(assignee, reporter)`, then `project` on a row of its own, the
   labels, the dates.
4. `FACETS`: add `FacetKey::new("reporter", "Reporter")`; `facet_values`: `"reporter"` gives the
   name (nothing when empty). `reporter:sam` then works, a typed word matches a reporter as it
   does an assignee, and the FILTER PICK lists reporters on both tabs.
5. Update the test fixture `tests::issue()` and `rich()` for the two new fields.

### 2. One edit path for all three properties

Generalise the `⌘S` machinery rather than copying it twice. Rename as you go; behaviour for
status must not change, and the existing `⌘S` tests are the safety net.

1. Add the types:

   ```rust
   /// Someone an issue can be assigned to.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct LinearUser { pub id: String, pub name: String }

   /// A property `issueUpdate` sets from here.
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
   pub enum Prop { Status, Priority, Assignee }

   /// A property's value: what a picker row sets, and what a refusal puts back.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub enum Change {
       Status(LinearState),
       Priority(u8),
       /// `None` is nobody.
       Assignee(Option<LinearUser>),
   }
   ```

   `impl Change`:
   - `prop(&self) -> Prop`
   - `of(issue: &LinearIssue, prop: Prop) -> Change` — what the row says now. For `Status` the
     `LinearState`'s `id` is left empty: this value is only ever put back, never sent.
   - `put_on(&self, issue: &mut LinearIssue, me: Option<&str>)` — status as `RowState::put_on`
     does today; priority sets `priority`; assignee sets `assignee`, `assignee_id` and
     `mine = me == Some(id)` (false for nobody).
   - `refused(&self, identifier: &str, why: &str) -> String` — `couldn't move ENG-1: {why}`
     (unchanged), `couldn't set ENG-1's priority: {why}`, `couldn't assign ENG-1: {why}`.

   `RowState` goes; `Change` replaces it.
2. `LinearList`: add `pub me: Option<LinearUser>` and `pub members: HashMap<String, Vec<LinearUser>>`
   (by team id, sorted by name). `parse_lists` fills them: `me` from `/data/me` or
   `/data/me/nodes/0`, members from `/data/teams/nodes`. A list without them (older fixtures)
   parses to `None` and empty.
3. `LocalEdits.moves` becomes `HashMap<(String, Prop), Move>`: `StatusMove` renamed `Move`, with
   `want: Change` and `confirmed: Change`. Each property of an issue queues on its own, so a `⌘P`
   never waits behind a `⌘S`. `lay_over` takes the landing list's `me` id and calls
   `mv.want.put_on(issue, me)`.
4. `LinearAnswer::Status { state, .. }` becomes `LinearAnswer::Edited { change: Change, .. }`;
   `land_status` → `land_edit`, `send_move` → `send_edit(app, issue_id, prop)`, and the body of
   `set_status` becomes `set_prop(app)` — same steps, keyed by `(issue_id, prop)`, with
   `change.stands_on(issue)` for "already there, nothing sent".
   In `land_edit`'s `Ok` arm only a `Status` tells the todos' chips, exactly as before
   (`crate::todos::view::note_linked`): a chip draws an issue's state, not its priority or whose
   it is.
5. `update_state` → `update_issue` as above.
6. `StatusPick` becomes one picker for all three:

   ```rust
   pub struct PropPick {
       pub issue_id: String,
       pub identifier: String,
       pub rows: Vec<Change>,
       pub selected: usize,
       pub prop: Prop,
       /// The row the issue stands on, marked `current`.
       pub current: Option<usize>,
   }
   ```

   `LinearView.status_pick` → `prop_pick: Option<PropPick>`. `handle_pick_key`, `hints()`, and
   `draw()`'s `side_up` follow the rename. `draw_status_pick` → `draw_prop_pick`, a heading and a
   row per `Change` through the existing `draw_side_pick`:
   - status: `Move ENG-1 to…`; the state's name and its kind dim (as now).
   - priority: `Set ENG-1's priority…`; `priority_mark` then `priority_word`, with the digit dim.
   - assignee: `Assign ENG-1 to…`; `Me` with the name dim, `No assignee`, then the names.
   - the `current` row ends with a dim `current`.

Run the `⌘S` tests (`ctrl_s_moves_an_issue_to_another_state`,
`a_ctrl_s_outlasts_lists_asked_before_linear_took_it`,
`two_quick_ctrl_s_go_out_in_order_past_a_refused_first`,
`a_lone_refused_ctrl_s_puts_back_what_linear_has`) before going on: only their constructors
(`LinearAnswer::Edited`, `prop_pick`, `pick.rows`) should have needed touching.

### 3. Priority (`⌘P`)

1. `keys`: `pub const PRIORITY: Key = crate::todos::view::keys::PRIORITY;` (`cmd+p`, `ctrl+p`) and
   `pub const LEVEL: Key = crate::todos::view::keys::LEVEL;` (`1`–`4`, `0`). Add both to
   `keys::ALL`.
2. `open_priority_pick(app)`: rows are `PRIORITY_ORDER` as `Change::Priority`, `selected` and
   `current` on the issue's own.
3. `handle_key`: `_ if keys::PRIORITY.matches(&key) => open_priority_pick(app)`, beside `STATUS`.
4. `handle_pick_key`: in a priority pick a digit chooses at once, as the todos' pick does
   (`todos/view.rs` `pick_key`): find the row for that level, select it, apply.
5. `hints()`: `keys::PRIORITY.hint()` after `keys::STATUS.hint()` in the Browse and Attach lists;
   while a priority pick is up, add `Hint::new("1-4/0", keys::LEVEL.does)`. `keys::SET`'s word
   becomes `set` (it now sets more than a status).

### 4. Assignee (`⌘I`)

1. `keys`: `pub const ASSIGN: Key = Key::new(&["cmd+i", "ctrl+g"], "assign");`, in `keys::ALL`.
2. `open_assign_pick(app)`: rows are `Assignee(Some(me))` when the list has `me`, `Assignee(None)`,
   then `members[issue.team_id]` without `me`. `selected` is the first row; `current` is the row
   whose id is `issue.assignee_id` (the `None` row for an unassigned issue). With no `me` and no
   members there is only `No assignee`: flash `Linear didn't say who {id} can go to — ⌘R asks
   again` and open nothing, as `open_status_pick` does for missing states.
3. `handle_key`: `_ if keys::ASSIGN.matches(&key) => open_assign_pick(app)`, beside `PRIORITY`.
4. `hints()`: `keys::ASSIGN.hint()` after `PRIORITY`. In the Browse list the three property hints
   go before `ATTACH` and `WORKTREE`: a narrow border cuts hints from the right.
5. `crates/orion-tui/src/ghostty_config.rs`: add `crate::linear::keys::PRIORITY` and
   `crate::linear::keys::ASSIGN` to `MODAL_KEYS`, next to `linear::keys::STATUS`.

### 5. Docs

- `crates/orion-tui/src/linear.rs` module comment: the reporter in the reading pane, and `⌘P` /
  `⌘I` beside the `⌘S` paragraph.
- `docs/keys.md`: the **Linear issues** row (reporter among the properties, the `reporter:` token,
  `⌘P` and `⌘I` after the `⌘S` sentence), and in the shared-chords table `⌘P | priority | todos,
  Linear` and `⌘I | assign | Linear` beside `⌘S | status | Linear`.
- `README.md` needs nothing.

## Tests

In `linear.rs`'s `tests`, reusing `moving()`, `land_list`, `ask_list`, `with_graphql_stub`,
`graphql_sent` and `refuse_the_first_move`:

- **parsing**: `reporter` from `creator`, from `externalUserCreator` when `creator` is null, empty
  with neither; `assignee_id`; `me` in both shapes; members by team, sorted. Extend
  `the_lists_query_asks_for_mine_and_the_teams_others` for `me:` and `teams(` in both the `isMe`
  and the `$email` form.
- **reading pane**: `the_reading_pane_heads_the_text_with_the_properties` shows `Reporter`.
- **filter**: `reporter:sam` narrows the list (beside `tokens_narrow_the_issues_by_facet`).
- **`⌘P`**: opens on the issue's priority; `2` sets High at once and sends `priority: 2`; the row
  says so before the answer; a refusal puts the old one back and flashes; a list asked before
  Linear took it leaves the new one.
- **`⌘I`**: opens on `Me`; `Enter` sends `assigneeId`, the issue shows on `My issues` at once;
  `No assignee` sends `assigneeId: null` and it shows on `Other issues`; a refusal puts both the
  name and the tab back.
- **independent queues**: a `⌘S` and a `⌘P` on one issue both go out without waiting on each other.
- **hints**: `assert_hints_from(&hints(view), keys::ALL)` with each picker up.

## Verification

```sh
cargo test -p orion-tui linear -- --nocapture
cargo test -p orion-tui todos            # the chips still follow a ⌘S and now a ⌘P
cargo test -p orion-tui ghostty_config   # every_modal_cmd_chord_is_released
make fmt && make lint
make dev
```

In `make dev`, on a project with a `LINEAR_API_KEY`: `⌘L`, check `Reporter` in the reading pane;
`⌘P` `1` and confirm Urgent in Linear; on `Other issues`, `⌘I` `Enter` and confirm the issue jumps
to `My issues` and is yours in Linear; `⌘I`, `No assignee`, and confirm it goes back.
