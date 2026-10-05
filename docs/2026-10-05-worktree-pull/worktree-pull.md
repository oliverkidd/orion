# Worktree pull and push: keep a checkout in step with its remote from the grid

Status: built. Pull and push both live in `crates/orion-tui/src/git_sync.rs` (the plan below said
`pull.rs`; it was renamed when push joined it, and `App::pull` became `App::git_sync`). Push is
described in the "Push" section at the end.

## Goal

Add one keypress that pulls the remote's new commits into the selected checkout. It must never lose
work and never surprise an agent working there.

```
 ⌂ main                                    ↗ #41 merged          ⇣3
 ⎇ feat/ui-redesign in pawy                ↗ #42 ready   *2  +40 −3  ⇡4
──────────────────────────────────────────────────────────────────────────
 ✓ ⌂ main pulled 3 commits from origin/main          ← FLASH after `p`
```

- `p` on a band (or **Pull** in the band's right-click menu, or **Pull from remote** in the command
  palette) pulls the band's checkout. It fetches, then fast-forwards only.
- It works the same on the ROOT WORKTREE and on a linked worktree. What happens depends on the
  branch's upstream (see "Behaviour").

## How sync works today (researched 2026-10-05)

orion never fetches or pulls on its own. There is no timer and no `@{u}` check.

| What | Where | Touches the network? |
|---|---|---|
| Fetch, then cut a new worktree from `origin/HEAD` | `crates/orion-daemon/src/git.rs` `default_base()` / `fetch_origin_if_any()` | yes, only when a worktree is cut |
| PR worktree: `fetch origin <head>`, then `merge --ff-only` if the branch existed | `git.rs` `add_pr_worktree()` (~:464-519) | yes, only when a PR checkout is made |
| Branch switcher: `git fetch --all` when it opens (throttled 60s), or on `^R` | `crates/orion-tui/src/branch_switch.rs` `fetch()` :298, `request_fetch()` :1252 | yes; root only |
| `⇡N ⇣M` on the band rule | `commit_list::ahead_behind()` :264, read every 2s by `event_loop.rs` `read_checkout()` | **no**: it counts against whatever refs the last fetch left |
| Daemon worktree sync, every 2s | `crates/orion-daemon/src/lib.rs` :185-230 | no; it only reconciles `git worktree list` |

Consequences:
- `⇣` on a band only moves after something else fetches: cutting a worktree, opening the branch
  switcher, an agent's own git, or the user in a terminal.
- On the root, `⇡⇣` is measured against `origin/<branch>`, so it means unpushed and unpulled. On a
  linked worktree it is measured against the **base** (`worktree_base_branch`, usually
  `origin/main`), not the branch's own upstream.
- Linked worktrees are cut with `--no-track` (`git.rs` :300-302). A new worktree branch has **no
  upstream** until the New-PR form pushes it with `--set-upstream`
  (`crates/orion-tui/src/pr_actions.rs` `push_branch()` :629). After that it tracks
  `origin/<branch>`.
- All worktrees of a project share one object store and one set of `refs/remotes/*`. So a fetch is
  per repository, and refreshes the remote view of every band of that project at once.
- HOME (`⌘G`) is the splash (`crates/orion-tui/src/splash.rs`). It lists no checkouts. Per-worktree
  actions live on the LAUNCHER VIEW grid, one BAND per checkout, so that is where this goes.

## Behaviour

For the checkout `C` on branch `B`:

1. **Pick the target.** Use the band under the cursor: `app.selected_worktree()` when focus is
   `Worktrees | Sessions | Terminal`. Otherwise use the selected project's root
   (`app.root_worktree(&project)`). This mirrors `branch_switch::open_branch_switch` (:1118), minus
   its `is_main` filter.
2. **Already pulling `C`:** show `Flash::note("already pulling ⎇ B")`. Start nothing.
3. **An agent is working in `C`:** if any unarchived agent of `C` has
   `AgentStatus::Running` (`orion-core/src/entities.rs` :7), ask first. Use a CONFIRM DIALOG:
   "An agent is working in ⎇ B. Pull anyway? Files may change under it." (`Enter`/`y` pulls,
   `Esc`/`n` doesn't). Otherwise pull straight away: a fast-forward cannot lose work, so it needs
   no confirm.
4. Show `Flash::working("pulling ⎇ B…")`. Then, off the loop:
   - `git rev-parse --abbrev-ref --symbolic-full-name @{u}`.
     - **No upstream, or detached HEAD:** run `git fetch --quiet origin` (skip it when there is no
       `origin`). Answer `NoUpstream { fetched }`. The fetch still pays off, because the base refs
       move and `⇣` on every band of the project is now current.
     - **Upstream `U` (e.g. `origin/B`):** run `git fetch --quiet` (git fetches `U`'s remote).
       Then `git rev-list --left-right --count HEAD...@{u}` gives `(ahead, behind)`:
       - `behind == 0`: answer `UpToDate { upstream, ahead }`.
       - `ahead > 0 && behind > 0`: answer `Diverged { upstream, ahead, behind }` and **don't
         merge**.
       - otherwise run `git merge --ff-only --quiet @{u}` and answer `Pulled { upstream, commits:
         behind }`. On failure answer `Failed(git_error(stderr))`. The likely case is "Your local
         changes to the following files would be overwritten". `HEAD` is untouched when it fails.
   - Fetch failure or timeout: answer `Failed("fetch failed: …")`.

   Doing an explicit fetch plus `merge --ff-only`, rather than `git pull`, means the user's
   `pull.rebase` / `pull.ff` config can't change what `p` does. It also lets fetch failures and
   fast-forward failures read differently. `add_pr_worktree` already does the same thing.
5. **Land it:**

   | Answer | FLASH |
   |---|---|
   | `Pulled` | `✓ ⎇ B pulled 3 commits from origin/B` |
   | `UpToDate` | `✓ ⎇ B is up to date with origin/B`, plus ` · ⇡2 to push` when ahead |
   | `Diverged` | `✕ ⎇ B and origin/B have both moved (⇡2 ⇣3): rebase or merge it in a terminal` |
   | `NoUpstream { fetched: true }` | `· fetched origin · ⎇ B tracks no remote branch yet` |
   | `NoUpstream { fetched: false }` | `· ⎇ B tracks no remote branch` |
   | `Failed(msg)` | `✕ pull ⎇ B: msg` |

   After landing, re-read every checkout of the project, so `⇡⇣`, `*N` and `+A −R` update now
   rather than on the sweep's next lap. Also mark the project as just fetched in
   `app.branch_switch.fetched`, so opening the branch switcher doesn't fetch again within
   `FETCH_GAP`.

The root and a tracked linked worktree go through the same path. There is no autostash, rebase or
merge commit: anything that isn't a fast-forward is left to the user, with the reason in the flash.

## Design inside orion

| Need | Copy from |
|---|---|
| Detached git with `setsid`, stdin closed, `GIT_TERMINAL_PROMPT=0` | `branch_switch.rs` `detached()` :256, `run()` :271 |
| Fetch with a timeout, SIGTERM then SIGKILL, stopped on quit | `branch_switch.rs` `fetch()` :298, `stop()` :324, `FETCH_TIMEOUT` :68 |
| git's stderr as one line | `branch_switch.rs` `git_error()` :221 |
| Async job: channel, `select!` arm, `land_*` | `branch_tx` / `branch_rx` in `event_loop.rs` (~:375, :752) → `branch_switch::land_answer` :1282 |
| Quit flag that stops a running fetch | `branch_switch::Shared.quit` + `Drop` :857-866 |
| Confirm dialog | `Overlay::Confirm(ConfirmDialog { …, action: PendingAction::… })` (`app.rs` :808, :687); `run_pending_action()` `event_loop.rs` :8922 |
| Band context menu | `worktree_menu_items()` `event_loop.rs` :6800; `MenuAction` `app.rs` :288; dispatch at `event_loop.rs` ~:9397 |
| Keymap entry | `ActionSpec` for `SwitchBranch`, `keymap.rs` :479-487 |
| Footer hint | `grid_hints()` `ui/footer.rs` :236, `act(km, Action::…, "…")` |
| Forcing a checkout re-read | selected: `app.git_changes` / `git_changes_stale()` (`app.rs` :5366); others: `changes_sweep_target()` `event_loop.rs` :1083 picks the entry missing from `app.worktree_changes` first |

## Steps

### 1. Share the remote-git helpers

In `crates/orion-tui/src/branch_switch.rs`:
- Make `detached`, `run`, `git_error` and `stop` `pub(crate)`.
- Generalise `fetch` into
  `pub(crate) fn fetch_with(root: &Path, args: &[&str], quit: &AtomicBool) -> Result<(), String>`:
  - Pipe stderr and read it on a helper thread (`std::thread::spawn` + `read_to_string`), so a
    chatty remote can't fill the pipe while `try_wait` polls.
  - Return `Err(git_error(..))` on a non-zero exit, `Err("fetch timed out")` past `FETCH_TIMEOUT`,
    and `Err("cancelled")` on quit.
  - Keep `pub fn fetch(root, quit) -> bool` as `fetch_with(root, &["fetch", "--all", "--quiet"],
    quit).is_ok()`, so the branch switcher doesn't change.

### 2. New module `crates/orion-tui/src/pull.rs`

Register it in `crates/orion-tui/src/main.rs` / `lib.rs`, next to `mod branch_switch;`. Write the
module doc in house style (SHOUTED terms: PULL, BAND, FLASH), covering the fast-forward-only rule
and why it fetches and merges instead of calling `git pull`.

```rust
pub enum Outcome {
    Pulled { upstream: String, commits: usize },
    UpToDate { upstream: String, ahead: usize },
    Diverged { upstream: String, ahead: usize, behind: usize },
    NoUpstream { fetched: bool },
    Failed(String),
}

pub struct Answer { pub worktree: WorktreeId, pub outcome: Outcome }

#[derive(Default)]
pub struct Shared {
    pub tx: Option<UnboundedSender<Answer>>,
    pub inflight: HashSet<WorktreeId>,
    pub quit: Arc<AtomicBool>, // raised in Drop, as branch_switch::Shared does
}

/// Blocking: the whole pull of one checkout (Behaviour step 4).
pub fn pull(root: &Path, quit: &AtomicBool) -> Outcome;

/// Target, in-flight check, running-agent confirm, flash, spawn (steps 1–4).
pub(crate) fn request(app: &mut App, worktree: WorktreeId);
/// Skip the confirm: called by request() and by the confirm's yes.
pub(crate) fn start(app: &mut App, worktree: WorktreeId);
/// FLASH, in-flight cleanup, re-read, branch_switch.fetched (step 5).
pub(crate) fn land(app: &mut App, answer: Answer);
/// The flash line for one outcome; pure, so it can be tested.
fn flash_for(branch: &str, outcome: &Outcome) -> Flash;
```

- `pull()` uses `branch_switch::run` for `rev-parse`, `rev-list` and `merge`, and
  `branch_switch::fetch_with` for the fetch.
- The "has origin" check is `git remote` output containing `origin`.
- `start()` skips spawning under `#[cfg(test)]`, as `request_fetch` does. Tests call `land`
  directly.

### 3. App state and the event loop

- `crates/orion-tui/src/app.rs`: add `pub pull: crate::pull::Shared` to `App`, next to
  `branch_switch`.
- `crates/orion-tui/src/event_loop.rs` `run_loop` (~:375): create `(pull_tx, mut pull_rx)` and set
  `app.pull.tx = Some(pull_tx)`.
- Add a `select!` arm next to `branch_rx` (:752):
  `answer = pull_rx.recv() => { if let Some(a) = answer { crate::pull::land(app, a); } }`.
- Re-read in `land`:
  - For the project's checkouts, remove their entries from `app.worktree_changes`, so
    `changes_sweep_target` takes them first.
  - If the pulled checkout is the selected one, set `app.git_changes = None`, so
    `git_changes_stale()` triggers a read on the next redraw.
  - Set `app.dirty = true`.

### 4. Action, key, menu, confirm

- `crates/orion-tui/src/keymap.rs`:
  - Add `Action::PullWorktree` to the enum (:44-212).
  - Add this `ActionSpec` after `SwitchBranch`:
    ```rust
    ActionSpec {
        action: Action::PullWorktree,
        id: "pull_worktree",
        label: "Pull from remote",
        hint: "Fetch and fast-forward the selected checkout to its upstream; never merges, rebases or touches uncommitted work",
        group: "PROJECTS & WORKTREES",
        scope: Scope::Global,
        defaults: &["p"],
    },
    ```
  - `p` is free: there are no `"p"` defaults in `ACTIONS`, and `event_loop/launcher.rs` only uses
    `^P` / `⌘P`. Pulling can't lose work, so a bare grid letter fits `docs/keys.md` ("destructive
    is never a bare letter"). It needs no Ghostty passthrough.
- `event_loop.rs` `dispatch_action` match (:3910-4202): add
  `Action::PullWorktree => crate::pull::request(app, target)`, with the target resolved as in
  Behaviour step 1 (factor the resolution into `pull::target(app) -> Option<WorktreeId>`).
- `app.rs`: add `MenuAction::PullWorktree(WorktreeId)` and `PendingAction::PullWorktree(WorktreeId)`.
- `event_loop.rs` `worktree_menu_items()` (:6800): add `MenuItem::new("Pull", MenuAction::PullWorktree(w.id.clone()))`
  after "Open", for root and linked worktrees alike. Dispatch it near :9397 to `pull::request`.
- `event_loop.rs` `run_pending_action()` (:8922): `PendingAction::PullWorktree(id) => crate::pull::start(app, id)`.

### 5. Footer hint

In `crates/orion-tui/src/ui/footer.rs` `grid_hints()`, add `act(km, Action::PullWorktree, "pull")`
only when the band under the cursor has `behind > 0` (`app.worktree_ahead_behind(&band.worktree)`).
Showing it only then keeps the footer short and points at the key when it matters.

### 6. Docs

- `docs/keys.md` "The grid" table (:91-127): add a `p` row after `c`:
  "**pull** the cursor's checkout: fetch, then fast-forward to its upstream. Never merges or
  rebases. A branch that diverged, or uncommitted changes in the way, says so in the FLASH and
  stays as it was. With an agent running there it asks first. A branch with no upstream yet (a new
  worktree before its PR is pushed) just fetches, which refreshes `⇣` across the project."
- `docs/sessions.md` band-rule paragraph (:415-427): after "unpushed and unpulled", add "`p` pulls
  them".
- `docs/how-it-works.md` fetch section (~:78-135): note that orion fetches when cutting a
  worktree, in the branch switcher, and on `p`, and never on a timer.
- `README.md` feature table (:40-50): add "pull a checkout (`p`)" to the git row if one exists.

## Tests

In `pull.rs` (`#[cfg(test)]`), use real repos in `tempfile::TempDir`: a bare `origin`, a clone,
and a linked worktree. Follow the `repo()` helper pattern in `branch_switch.rs` :2242.
- `behind_only_fast_forwards_and_counts`: push 3 commits to origin from a second clone. `pull()`
  returns `Pulled { commits: 3 }`, and `HEAD == origin/main`.
- `level_is_up_to_date_and_reports_ahead`: one local commit, nothing new upstream. Returns
  `UpToDate { ahead: 1 }`.
- `diverged_never_merges`: a commit on each side. Returns `Diverged { ahead: 1, behind: 1 }` and
  `HEAD` is unchanged.
- `local_changes_in_the_way_fail_and_keep_head`: upstream edits `f`, and the checkout has an
  uncommitted edit to `f`. Returns `Failed(msg)` with msg containing "would be overwritten", and
  `HEAD` plus the working file are unchanged.
- `no_upstream_fetches_origin`: a `--no-track` worktree branch, with origin's main moved on.
  Returns `NoUpstream { fetched: true }` and `origin/main` in the clone has advanced.
- `detached_head_is_no_upstream`.
- `flash_for` table test: one line per `Outcome`, matching the FLASH table above.

In `keymap.rs` tests:
- `p` maps to `Action::PullWorktree`.
- `stray_letters_quit_close_and_archive_nothing` (:1998) still passes.

In `event_loop.rs` tests (use the existing `App` test builders):
- `p` on a band whose agent is `Running` opens `Overlay::Confirm` with
  `PendingAction::PullWorktree`, and nothing is in flight.
- `p` on an idle band puts the worktree in `app.pull.inflight` and sets a `working` flash.
- A second `p` while in flight gives the "already pulling" note.
- `pull::land` clears in-flight, sets the flash, drops the project's `worktree_changes` entries,
  and makes `git_changes_stale()` true for the selected checkout.

## Build and check

```sh
cargo build -p orion-tui
cargo test -p orion-tui pull
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Manual verification

1. On a project whose root is behind origin (`git -C <root> reset --hard HEAD~2`), open orion. The
   root band shows `⇣2` only once something fetches. Press `p` on it. The flash reads
   `✓ ⌂ main pulled 2 commits from origin/main` and `⇣2` disappears within a frame or two.
2. Press `p` again: `up to date`.
3. On a fresh linked worktree (no upstream), press `p`: `fetched origin · ⎇ B tracks no remote
   branch yet`.
4. Push the branch through the New-PR form. Push a commit to it from GitHub's web editor. Press `p`
   on the band: `pulled 1 commit from origin/B`.
5. Make the branches diverge (a local commit plus a remote commit): the `Diverged` flash appears and
   `git log` is unchanged.
6. Start an agent turn in a worktree and press `p` while it is yellow: the confirm appears, and `n`
   leaves the checkout alone.
7. Right-click a band: **Pull** is in the menu. `⌘⇧P` → "Pull from remote" is in the command
   palette. Settings → Hotkeys lists it and can rebind it.
8. Unplug the network and press `p`: the flash shows `✕ pull ⎇ B: fetch failed: …` within
   `FETCH_TIMEOUT`, and the UI never blocks.

## Push (`⇧P`)

Added after pull, in the same module, sharing its one-sync-per-checkout guard
(`Shared::inflight: HashMap<WorktreeId, Op>`).

- **Key:** `⇧P` (`Action::PushWorktree`, id `push_worktree`). It's a modified chord, not a bare
  letter, because a push is seen by others. It's also **Push** in the band and card menus.
- **Tracking:** read from `branch.<b>.remote` / `.merge`, not only `@{u}`. A fork's pull request
  checkout tracks `refs/pull/N/head`, which no refspec stores, so `@{u}` doesn't resolve there.
  Pull fetches that ref and fast-forwards onto the fetched commit. Push refuses it.
- **No upstream:** `git push --set-upstream origin refs/heads/<b>:refs/heads/<b>` →
  `Published`.
- **Upstream:** fetch, then count `HEAD...@{u}`.
  - `ahead == 0` → `NothingToPush { behind }`.
  - Both sides moved → `Diverged`. It never forces.
  - The upstream is the base branch (`commit_list::resolve_base_cached` with the
    `worktree_base_branch` setting) and the push isn't confirmed → `ConfirmPush`. That arms
    `Shared::armed` and flashes "`⇧P` again to push". A second push request on that checkout within
    30 s runs with `confirmed: true`. It isn't a dialog, because the answer lands seconds after the
    key: a dialog would open over whatever was opened since, or catch a `y` typed into a pane.
  - Otherwise `git push <remote> refs/heads/<b>:<merge>` → `Pushed`.
- **Errors:** a refused push reports the line that explains it, a pre-push hook's last word or the
  `! [rejected]` row, via `branch_switch::remote_error`.
- **Budget:** `pr_actions::PUSH_TIMEOUT` (300 s), because hooks can run test suites.
- **After it lands:** `pr_refresh_requested = true`, the same re-read as a pull, and the flash
  counts any uncommitted files that weren't pushed.
- **Footer:** shows `pull` / `push` only on the root band, where `⇡⇣` mean unpulled / unpushed.

## Follow-ups (not in this change)

- **Background fetch.** Add a setting such as `auto_fetch_minutes` (default off), which fetches each
  open project once per interval through `fetch_with`. One fetch per repository is enough, because
  worktrees share `refs/remotes`. With it, `⇣` moves without a keypress.
- **Upstream counts on linked bands.** Once a worktree branch tracks `origin/B`, its rule's `⇡⇣`
  still measures against the base. A second, dim `⇣N` against `@{u}` when it differs would show
  that a teammate pushed to the PR branch.
- **Pull all.** A palette action that runs `pull::start` for every band of the project, each
  reporting through the same flash (or a summary flash once all land).
