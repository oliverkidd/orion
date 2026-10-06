# Stack status: see which compose stacks are running, and stop them from orion

## Goal

Compose stacks started inside worktrees (riplo-os runs five containers for each checkout on
OrbStack) currently have to be checked and stopped in OrbStack. Orion should show them itself:

1. **A mark on each worktree's band rule.** It is an outline hexagon `⬡` that replaces the last
   `─` at the right end of the rule. Green means the worktree's stack is running and grey means it
   exists but is stopped. A worktree with no stack has no mark. When the stack is running, a faint
   green light travels along the rule's dashes into the hexagon every 8 seconds, and the hexagon
   flashes when the light arrives.
2. **A Stacks modal** (`⇧S`, and **Stacks** in the command palette). It lists every compose stack
   on the machine, including stacks orion did not start (for example a Cursor worktree's), and
   names the worktree each one belongs to. Each stack can be started, stopped or taken down.

It must work for any repo and any engine that speaks the `docker` CLI (OrbStack, Docker Desktop,
Colima), not just riplo-os.

## Background: what already exists

- `crates/orion-core/src/compose.rs` holds the pure docker helpers:
  - `ps_args()` lists every container with its compose project and its
    `com.docker.compose.project.working_dir` label. It has no state column yet.
  - `parse_ps`, `Container` and `by_project`.
  - `started_in(containers, dir)` matches projects to a checkout by directory, and keeps
    `owned` and `shared` projects apart.
  - `find_docker(PATH, home)` finds the CLI even under launchd's thin PATH.
  - `WorktreeContainers::compose_args(project)` builds `compose -p <p> stop|down …`.
- `crates/orion-daemon/src/containers.rs` runs docker from the daemon:
  - `run(docker, args, timeout)` returns stdout, or the last stderr line as the error.
  - `path_with(docker)` builds the PATH to run under.
  - `PS_TIMEOUT` is 15s and `COMPOSE_TIMEOUT` is 60s.
  - The module stops or tears down a deleted worktree's stacks according to the
    `worktree_containers` setting.
- `orion doctor` lists orphaned stacks (`crates/orion-tui/src/doctor.rs:489`).
- Mockups the user signed off on (2026-10-06): https://claude.ai/artifact/YUJXuVAZjnEFm5yPQNTEYT
  (they chose position "Right end", trail "Into it" and the "Outline" hexagon).

Today no fact is polled by the daemon per worktree and pushed to clients. The TUI polls git
itself. Stacks are the first, because docker must run from the daemon: one poll should serve
every client, and the TUI may be closed.

## The look

### Band rule

```
── ⌂ main ─────────────────────────────────────────── ↑0 ↓3 ─⬡     (grey: stopped)
── ⎇ curious-otter-glides ───────────────────── #812  +48 −12 ─⬡   (green, trail runs in)
❯  ⎇ electric-zebra-drifts ──────────────────── #806 +210 −37 ─⬡   (green)
── ● cosmic-gadget-sails ──────────────────────────────── #799 ─⬡  (grey)
── ⎇ velvet-cactus-sprints ─────────────────────────────────────── (no stack: unchanged)
```

- **Position:** the hexagon takes the last cell of the trailing `──` when the rule has a right
  block (PR or git columns). When it has no right block, it takes the last cell of the fill dash.
  The rule's width never changes, so every band's hexagon sits in the same column.
- **Colours:**
  - Running (at least one container up): `th.ok`, which is green, `Indexed(71)`.
  - Stopped (containers exist, none running): `th.faint`, `Indexed(240)`.
- **Trail ("Into it"):**
  - The light covers every `─` cell to the left of the hexagon: the lead `──`, the fill dash and
    the first dash of the tail. It does not cover the `❯` cursor mark or any text.
  - It moves one cell per animation frame (`SWEEP_FRAME`, 100 ms), starting at the leftmost
    dash. The cycle is `STACK_TRAIL_PERIOD = 80` frames.
  - A dash at column `x`, with the hexagon at column `h` and the leftmost dash at `x0`, is reached
    at frame `t = x - x0`. On a frame `p = sweep_phase % 80`, set `k = p - t`. When `k` is 0, 1 or
    2, the cell takes `stack_sweep[k]`. Otherwise it keeps the colour it was drawn in.
  - Because `t` counts columns, not dashes, the light keeps an even speed while it passes behind
    text.
  - The hexagon flashes `stack_flash[k]` for `k = p - (h - x0)` in 0..3.
  - Every band reads the same `sweep_phase`, so all the lights move together. That is calmer than
    each band running out of step.
- **Theme ramps** go in `Theme::with_accent` (`crates/orion-tui/src/theme.rs`, next to
  `merged_sweep`). They are part of the fixed status set, so presets do not override them:
  - `stack_sweep: [Indexed(114), Indexed(71), Indexed(65)]`: the head, then the fading tail.
  - `stack_flash: [Indexed(114), Indexed(157), Indexed(114)]`.
- **When it is still:** a stopped stack never animates. When `animations` is off, a running stack
  shows a steady green `⬡` with no trail.

### Stacks modal

```
╭─ Stacks ──────────────────────────────────────────────────────────────────────╮
│   STACK                                     WORKTREE                  STATE   │
│ ⬡ riplo-electric-zebra-drifts-f2e9483b      electric-zebra-drifts     5/5 up  │
│ ⬡ riplo-curious-otter-glides-69eaa911       curious-otter-glides      5/5 up  │
│ ⬡ riplo-fix-riplo-984-indicator-…-4fb7bc91  fix-riplo-984-indicat…    5/5 up  │
│ ⬡ riplo-pawy-8287fa6e                       ~/.cursor/worktrees/…/pawy 5/5 up │
│ ⬡ riplo-main                                main · riplo-os           0/8     │
│ ⬡ riplo-cosmic-gadget-sails-285355c4        cosmic-gadget-sails       0/5     │
│                                                                               │
│ enter start/stop · ⌘W take down · s stop all (3) · esc close            │
╰───────────────────────────────────────────────────────────────────────────────╯
```

- **Rows:** one per compose project. The `⬡` is coloured the same way as on the band. There is no
  trail in the modal.
- **Order:**
  - Stacks of the selected project's worktrees come first, then other orion projects' stacks, then
    stacks orion knows nothing about.
  - Within each group, running stacks come first, then stacks sorted by name.
  - The cursor opens on the selected worktree's stack when it has one.
- **WORKTREE column:**
  - The worktree's branch when orion knows the checkout.
  - `main · <project>` for a root checkout.
  - The directory, shortened with `~`, for a stack orion doesn't know about.
  - `several` for a project whose containers started in different checkouts (`started_in`'s
    `shared` case).
- **STATE column:** `<running>/<total> up`, or `<running>/<total>` while none are running. It shows
  `starting…`, `stopping…` or `taking down…` while an action runs.
- **Keys** (following the modal shortcut rules: one key per action, the same key as the matching
  global action):
  - `enter`: start the stack when nothing in it is running, otherwise stop it.
  - `⌘W` (twin `ctrl+w`): take the stack down — the key every other modal deletes with
    (todos, skills, presets, the PR modal's close). It asks first, under the list:
    `take down riplo-x? enter keeps its volumes · ⌘W deletes them too · esc cancels`. Volumes are
    only removed on the second `⌘W`. (Built 2026-10-06: ⌘⌫ was planned, ⌘W matches the modals.)
  - `s`: stop every running stack. Also a palette action, **Stop all stacks**.
  - `esc` closes the modal. `↑`/`↓` move, and so do `j`/`k` if other list modals take them.
- **Mouse:**
  - Clicking a row selects it. Clicking the `⬡` toggles that stack.
  - Clicking the `⬡` on a worktree's band opens the modal on that worktree's stack.
- **Docker unavailable:**
  - With no docker CLI, the modal body reads `No docker CLI found — orion looks on PATH, in
    ~/.orbstack/bin, /usr/local/bin, /opt/homebrew/bin and Docker Desktop's app bundle.`
  - With the engine not running, it reads `Docker isn't running` followed by the last error line.
    The band marks disappear (no stale state).

## Design

### Data flow

```
daemon poller (every 5s) ── docker ps ──▶ Vec<Stack> ──changed?──▶ broadcast StacksChanged
        ▲                                                         (also sent after each Snapshot)
        └── poked right after any StackAction finishes
TUI: App.stacks ──▶ per-worktree StackState (by directory) ──▶ band mark + Stacks modal
TUI ── ClientRequest::StackAction{project, verb} ──▶ daemon runs `docker compose -p …` ──▶ Ack/Error
```

The daemon sends raw stacks with their directories. The TUI maps them to worktrees with a pure
core helper. The TUI already has every worktree's path, and the modal needs the unmatched stacks
too.

## Steps

### 1. Core: stack listing and verbs (`crates/orion-core/src/compose.rs`)

1. Add a third column to the listing and keep the old one intact for `doctor` and `release`:
   - `pub fn ps_state_args() -> Vec<&'static str>` is the same as `ps_args` with
     `\t{{.State}}` appended to the format.
   - `pub fn parse_ps_state(out: &str) -> Vec<(Container, bool)>`, where the bool is
     `state == "running"`. Lines that compose didn't make are skipped, as `parse_ps` does.
2. Add a serialisable summary (it goes over the wire, so derive
   `Serialize, Deserialize, Debug, Clone, PartialEq, Eq`):

   ```rust
   pub struct Stack {
       pub project: String,
       /// Every distinct directory its containers were started from, sorted.
       pub dirs: Vec<PathBuf>,
       pub running: u16,
       pub total: u16,
   }
   pub fn stacks(rows: &[(Container, bool)]) -> Vec<Stack> // sorted by project
   ```

3. Map stacks to worktrees:

   ```rust
   pub enum StackState { Running, Stopped }
   /// The stack started wholly inside `dir` (a nested checkout's included),
   /// preferring a running one when a checkout has several.
   pub fn stack_in<'a>(stacks: &'a [Stack], dir: &Path) -> Option<&'a Stack>
   ```

   Reuse `is_within`, and treat `Stack.dirs` the same way `started_in` treats `owned`: every dir
   must be within `dir`. When several stacks qualify, pick a running one first, then the first by
   name.
4. Add the verbs:

   ```rust
   pub enum StackVerb { Start, Stop, Down, DownVolumes }
   impl StackVerb { pub fn compose_args(self, project: &str) -> Vec<String>; pub fn progress(self) -> &'static str; pub fn past_tense(self) -> &'static str }
   ```

   - `Start` → `compose -p P start`, which restarts existing containers without needing a compose
     file.
   - `Stop` → `stop`.
   - `Down` → `down --remove-orphans`.
   - `DownVolumes` → `down --remove-orphans --volumes`.
   - Make `WorktreeContainers::compose_args` delegate to it (Stop/Remove/RemoveVolumes map onto
     Stop/Down/DownVolumes) so the argument lists live in one place.
5. Tests in the existing `mod tests`:
   - Parsing the state column.
   - `stacks` counts running and total, and dedupes and sorts dirs.
   - `stack_in` prefers a running stack and ignores a shared one.
   - Every verb's args.
   - The `WorktreeContainers` args stay unchanged.

### 2. Protocol (`crates/orion-core/src/protocol.rs`)

1. Bump `PROTOCOL_VERSION` from 47 to 48. Frames are positional msgpack, so new variants break
   compatibility.
2. Add `ClientRequest::StackAction { req_id: u64, project: String, verb: StackVerb }`.
3. Add `ServerEvent::StacksChanged { stacks: Option<Vec<Stack>>, error: Option<String> }`:
   - `stacks: None` means docker is unavailable, and `error` says why: `no docker CLI`, or the
     last line from `docker ps`.
   - `Some(vec![])` means docker answered and no stacks exist.
4. Add round-trip tests next to the existing ones.

### 3. Daemon: poller and actions

1. Create a new module, `crates/orion-daemon/src/stacks.rs` (register it in `lib.rs`):
   - Make `run` and `path_with` in `containers.rs` `pub(crate)`, and reuse them, together with
     `PS_TIMEOUT` and `COMPOSE_TIMEOUT`.
   - `pub async fn poll(docker: Option<&Path>) -> (Option<Vec<Stack>>, Option<String>)` runs
     `ps_state_args` and turns the output into stacks with `stacks()`.
   - `pub async fn act(project: &str, verb: StackVerb) -> Result<(), String>` finds docker, runs
     `verb.compose_args(project)` with `COMPOSE_TIMEOUT`, and logs `compose project {p}
     {past_tense}`.
2. On `Daemon` (`crates/orion-daemon/src/registry.rs`), add:
   - `stacks: std::sync::Mutex<ServerEvent>`, the last `StacksChanged`, initially
     `{stacks: None, error: None}`.
   - `stacks_poke: tokio::sync::Notify`.
3. Add the poller loop in `crates/orion-daemon/src/lib.rs`. Copy the worktree sync loop
   (`lib.rs:208-250`):
   - Add an env var `STACK_POLL_MS = "ORION_STACK_POLL_MS"` in `crates/orion-core/src/env.rs`,
     next to `WORKTREE_SYNC_MS`, and use `env_interval(env::STACK_POLL_MS, 5_000)`.
   - Run `tokio::select!` over `shutdown.cancelled()`, `interval.tick()` and
     `stacks_poke.notified()`.
   - Find docker once per tick with `compose::find_docker`, which is cheap (a few stats).
   - When docker is missing or `ps` fails, back off: skip ticks until 30s have passed since the
     last failure. That way a stopped engine isn't asked every 5s.
   - Broadcast `StacksChanged` only when the result differs from the stored one, then store it.
4. In `crates/orion-daemon/src/server.rs`:
   - In the subscribe path (`server.rs:124-150`), send the stored `StacksChanged` right after
     `Snapshot`, so a new client has the state immediately.
   - Handle `StackAction`: clone `daemon` and `out_tx`, then `tokio::spawn` the work, as
     `DeleteWorktree` does at `server.rs:332-347`. Run `stacks::act`, then
     `daemon.stacks_poke.notify_one()`, then `reply_done(&out_tx, req_id, result)`.
5. Tests:
   - `stacks.rs`: reuse the `Stub` docker pattern from `containers.rs` tests (move `Stub` into a
     shared `#[cfg(test)] mod test_docker` if both modules need it). Cover: `ps` with mixed states
     produces the right stacks, an unreachable engine produces `(None, Some(error))`, and `act`
     runs the right args and surfaces stderr.
   - `server.rs`, if there is an existing request test harness: a `StackAction` with a failing
     stub replies `Error{req_id}`.

### 4. TUI state (`crates/orion-tui/src/app.rs`, `event_loop.rs`)

1. On `App`, add `stacks: Option<Vec<Stack>>` and `stacks_error: Option<String>`, next to
   `worktree_ahead` (~`app.rs:4358`). Add a `stack_pending: HashMap<String, StackVerb>` for the
   modal's in-flight labels.
2. In `handle_server_event` (`event_loop.rs:12833`), store `StacksChanged`, drop pending entries
   whose result has arrived, and set `dirty`.
3. Add `App::stack_of(&self, worktree: &WorktreeId) -> Option<(&Stack, StackState)>`, which looks
   up the worktree's path in `self.tree.worktrees` and calls `compose::stack_in`.
4. In `App::status_anim_active` (`app.rs:5101`), add a clause that keeps the clock running while
   any visible worktree has a running stack:

   ```rust
   || self.visible_worktrees().iter().any(|w| matches!(self.stack_of(&w.id), Some((_, StackState::Running))))
   ```

   This keeps orion repainting at 10 fps while any stack is up. Ratatui only writes changed cells,
   so terminal output stays small, but measure it (see Verification). If it costs too much, drop
   the trail's frame rate by redrawing only on even phases instead of removing the trail.

### 5. Band mark (`crates/orion-tui/src/ui/launcher_view.rs`)

1. Leave span building in `draw_band_rule` (`launcher_view.rs:1890-2056`) as it is. After
   `Paragraph::new(Line::from(spans)).render(r, buf)`, call a new
   `paint_stack_mark(buf, r, app, &band.worktree)`:
   - Return early when `app.stack_of` is `None`.
   - `h` is the last column of the rule: `r.x + r.width - 1`, clamped to the rendered width. Both
     layouts end in a `─` there, because the tail `dash(2)` or the fill dash is last. Assert this
     in debug builds with `debug_assert_eq!(buf[(h, r.y)].symbol(), "─")`.
   - Set that cell to `⬡`, with fg `th.ok` for running and `th.faint` for stopped.
   - When running and `app.animations` is on:
     - `x0` is the first column left of `h` whose symbol is `─`.
     - For every column `x` in `x0..h` whose symbol is `─`, apply the trail formula from "The
       look" using `app.sweep_phase()`.
     - Then apply the hexagon flash.
     - Put `STACK_TRAIL_PERIOD` next to `ONE_SHOT_SWEEP` in `app.rs`.
2. Hit target: add `HitTarget::LauncherBandStack(WorktreeId)` covering the `⬡` cell. Push it
   before the band's `LauncherBand(index)` hit so it wins, following how `LauncherBandPr` is
   ordered at `launcher_view.rs:2040-2055`. A click opens the Stacks modal on that stack.
3. The list layout (`draw_list_band`) uses the same rule function. Check that the mark appears
   there too, and add it if the list band builds its own rule.
4. Tests, in the band-rule tests (~`launcher_view.rs:5600-5820`, using the `rule_row`,
   `rule_row_as`, `painted` and `row_string` helpers):
   - `a_running_stack_marks_the_rules_right_end`: the text ends in `─⬡` with the git columns
     present, the `⬡` is painted `th.ok`, and the row width is unchanged.
   - `a_stack_mark_without_columns_takes_the_last_fill_dash`.
   - `a_stopped_stack_is_a_faint_mark`.
   - `no_stack_leaves_the_rule_unchanged`: compare against the existing expected text.
   - `the_trail_lights_dashes_never_text`: at a phase where the head would sit over the branch
     name, no text cell changes colour, and at the next dash phase the dash is
     `stack_sweep[0]`.
   - `animations_off_keeps_the_mark_still`.
   - Extend the `status_anim_active` tests (`event_loop.rs` ~15300 and ~17915): a running stack
     on a visible worktree keeps the clock running, a stopped one does not, and animations off
     wins.

### 6. Stacks modal (`crates/orion-tui/src/stacks.rs`, new)

Model it on the Account usage modal (`crates/orion-tui/src/usage.rs`): `UsageView` (:851),
`mod keys` (:880), `open` (:892), `handle_key` (:906), `handle_mouse` (:933) and `draw` (:1012),
using `ui::centered_rect` and `ui::render_modal_frame`.

1. `StacksView { selected: usize, confirm_down: Option<String>, area, list_area }`.
   - Rows are rebuilt from `app.stacks` on every draw, in the order described above. Track the
     selection by project name, not index, so it survives a refresh.
2. `mod keys`:
   - `TOGGLE = Key::new(&["enter"], "start/stop")`.
   - `DOWN = Key::new(&["cmd+w","ctrl+w"], "take down")`, `STOP_ALL = Key::new(&["s"], "stop all")`.
   - Use the shared close and nav keys.
   - Add `DOWN` to `MODAL_KEYS` in `crates/orion-tui/src/ghostty_config.rs`, `Overlay::Stacks`
     to `modal_takes_cmd_w` (event_loop.rs) so ⌘W reaches the modal instead of closing the pane,
     and stage Esc in `closes_on_esc` while a take-down is waiting for its answer.
3. Actions send `ClientRequest::StackAction` through the client used by other request senders,
   and record `stack_pending`. A reply of `Error` shows the usual warning toast:
   `couldn't stop riplo-x: <reason>`.
4. Opening the modal:
   - Add `Overlay::Stacks` (`app.rs` ~2398).
   - Add `Action::Stacks` and `Action::StopAllStacks` (`keymap.rs` ~202), with `ActionSpec`
     entries next to Usage (`keymap.rs:857-865`):
     - `Stacks`: id `stacks`, label `Stacks`, hint `Every docker compose stack on this machine
       and which worktree it belongs to — start, stop or take one down`, group `GENERAL`, scope
       `Global`, defaults `["shift+s"]`.
     - `StopAllStacks`: id `stop-all-stacks`, label `Stop all stacks`, no defaults.
     - Check that `⇧S` is free in every scope where Global applies. Grep `keymap.rs` for
       `"shift+s"`; it was free on 2026-10-06.
   - Wire them through the same places as Usage:
     - `event_loop.rs:4099`, plus the action list at :4990.
     - The overlay list at :3745.
     - Key routing at :7396 and mouse routing at :12218.
     - Draw at `ui.rs:2604`.
     - `overlay_close.rs:38` and :105.
     - The perf label at `perf.rs:163` and the test label at `event_loop.rs:38182`.
5. Tests:
   - Row ordering: current project's stacks, then other orion projects', then unknown ones, with
     running first in each group.
   - The WORKTREE column labels, including the `several` case.
   - `enter` sends Start for a stopped stack and Stop for a running one.
   - `⌘W` arms the confirm. `enter` then sends Down, and a second `⌘W` sends DownVolumes.
   - Stop all sends Stop only for running stacks.
   - The empty states for no docker and for docker not running.
   - Clicking a band's `⬡` opens the modal on that stack.

### 7. Docs

- `docs/keys.md`: add `⇧S Stacks` under "Views" (:244), and the modal's verbs under "Modal verbs"
  (:261).
- `docs/commands.md`: add the **Stacks** and **Stop all stacks** palette entries.
- `docs/configuration.md`, in the "Worktree containers" section (:655): add a paragraph saying
  that the band's `⬡` and the Stacks modal show live state, and that `ORION_STACK_POLL_MS` changes
  the 5s poll.
- There is no CHANGELOG. The commit message becomes the release note (`whats_new.rs`), so write
  it for users.

## Out of scope (possible follow-ups)

- A stacks count chip in the footer or on project tabs.
- Per-container detail (ports, logs) in the modal. OrbStack already does this well.
- Pausing the poller while no client is attached. A 5s `docker ps` is cheap, and the band needs
  fresh state the moment a client attaches. Revisit if the daemon's CPU shows up in
  `orion doctor` metrics.

## Verification

```sh
cargo fmt --all
cargo test -p orion-core compose
cargo test -p orion-core protocol
cargo test -p orion-daemon stacks
cargo test -p orion-tui stack
cargo test -p orion-tui status_anim_active
make lint STRICT=1
make test
```

Manual checks in the isolated dev instance (`make dev`):

1. With riplo-os worktrees running (check with `docker compose ls --all`), each one's band shows
   a green `⬡` at the right end, and the trail runs into it about every 8s. Stopped ones (for
   example `riplo-main`) show a grey `⬡`. Worktrees without a stack are unchanged.
2. Press `⇧S`. Every stack from `docker compose ls --all` is listed, including the Cursor `pawy`
   one, which is labelled with its `~/.cursor/…` path.
3. Pressing `enter` on a running stack shows `stopping…`. Within about 5s the row and the band
   turn grey, and `docker compose ls` agrees. Pressing `enter` again starts it.
4. Press `⌘W`, then `esc`: nothing happens. Press `⌘W`, then `enter`: the stack is gone from
   `docker compose ls --all` and its volumes remain (`docker volume ls`).
5. Quit OrbStack. Within 30s the marks disappear and the modal says Docker isn't running. Start
   OrbStack again and the marks return.
6. Turn off `animations` in Settings. The marks stay and stop moving.
7. Measure the idle cost: compare `make perf` and the daemon/TUI CPU in Activity Monitor, with
   and without a running stack, for about a minute. The TUI should stay under 2% CPU at 10 fps.
