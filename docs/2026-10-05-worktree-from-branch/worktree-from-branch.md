# Worktree from an existing branch: search any branch in the `⌘.` picker

Status: built. One addition to the design below: `MenuFilter` gained a `limit`, and the picker shows
at most 12 branch rows at once (`PICKER_BRANCH_ROWS`), because the menu does not scroll; typing reaches
the rest. A listing that lands while the picker is up rebuilds it through
`event_loop::launcher::refresh_worktree_picker`, keeping the query, the highlighted row and the
minted `+ new worktree` name.

## Goal

Make it easy to start work on a branch that **already exists**, such as a teammate's branch, local
or on GitHub. Today orion can only cut a **new** branch into a worktree.

There is one change, in one place. The quick prompt's WORKTREE PICKER (`⌘.` / `^T`) also lists
every branch that has no checkout yet: local branches, then the ones on origin (GitHub). Typing
filters them along with everything else. Picking one aims the launch at a fresh worktree **on that
branch**. `Enter` creates the worktree (env links and worktree hook, as for any new worktree) and
starts the agent there. Sending the box empty starts the agent bare.

```
 Worktree
 + new worktree  quiet-otter-runs
 main  (root)
 fix-login-redirect
 ⎇ teammate-feat                 ← local branch, no checkout
 ⎇ origin/alice/new-billing      ← only on GitHub
```

Nothing else changes: the branch switcher, the PR modal and the CLI stay as they are. PRs already
have their own route (`v` → pick → `Enter`), which handles forks.

This also fixes a bug. Typing the exact name of a branch that exists only on origin into **New
worktree** currently creates a *new, empty* branch with that name, cut from `origin/HEAD`. After
the fix, it checks out the remote branch and tracks it.

## How it works today (researched 2026-10-05)

| Path | What it does | Where |
|---|---|---|
| Quick prompt `⌘.` picker | "+ new worktree" (new branch), then the project's checkouts. No branches | `crates/orion-tui/src/event_loop/launcher.rs` `open_worktree_picker()` ~:2588 |
| Launch into a fresh worktree | `QuickTarget::NewWorktree { project, branch }` → `ClientRequest::CreateWorktree { branch, base: None }`; the agent follows on the Ack | `crates/orion-tui/src/quick_prompt.rs` ~:37, `rg "QuickTarget::NewWorktree" crates/orion-tui` |
| Daemon cut | `create_worktree()` → `git::add_worktree_off_default()` → `add_worktree_inner()`: `git worktree add -b <branch> <origin/HEAD>`. If `-b` fails because a **local** branch exists, it falls back to checking that branch out | `crates/orion-daemon/src/registry.rs` ~:762, `crates/orion-daemon/src/git.rs` ~:225, ~:286 |
| A branch that is **only on origin** | `-b` succeeds and makes a new branch off main with the same name. The teammate's commits are not in it | same |
| Branch listing + cache + throttled fetch | `git for-each-ref` into `Branch` rows: local, then remote with no local twin, each with `checked_out_at`. A per-checkout cache. A background `git fetch --all` at most once a minute | `crates/orion-tui/src/branch_switch.rs` `Branch` ~:109, `parse_refs()` ~:157, `list_branches()` ~:207, `refresh()` ~:1431 |

## Design

### 1. Daemon: check out an existing branch (`git.rs`, `registry.rs`)

Add `add_branch_worktree(repo, name) -> Result<(PathBuf, String)>` in `git.rs`. It returns the path
and the local branch name. `name` may be `feat-x` or `origin/feat-x`.

1. Call `fetch_origin_if_any(repo)`. Carry on even if the fetch fails.
2. Strip a leading `origin/` to get `local`.
3. If `refs/heads/<local>` exists: call `add_worktree(repo, local, None)`. Its fallback checks out
   the existing branch. Then, if `origin_branch(repo, local)` is `Some`, run `git merge --ff-only
   --quiet origin/<local>` in the new checkout and log a failure at info level. This is the same
   rule as the kept-branch case in `add_pr_worktree` (~:464): a branch with commits of its own is
   left as it is.
4. Else if `origin_branch(repo, local)` is `Some(remote)`: call `add_worktree_inner(repo, local,
   Some(&remote), true)`. Tracking is on, so `p` / `⇧P` / `git pull` talk to the teammate's branch.
5. Else bail with `no branch <name> locally or on origin`.

In `registry.rs`, change the signature to `create_worktree(project_id, branch, base, existing:
bool)`:

- **`existing` true**: under `worktree_ops`, if a row of this project already has `branch ==
  local`, return its id (the same reuse as `pr_worktree` ~:818). Otherwise call
  `add_branch_worktree`, then `register_worktree`, `link_env_files` and `run_worktree_hook(Create)`,
  as the current body does. `base` is ignored.
- **Bug fix**, when `existing` is false and `base` is `None`: if there is no `refs/heads/<branch>`
  but `origin/<branch>` exists, take the `existing` route. A name origin already has is someone's
  work. Slugified sentences and random `adj-noun-verb` names essentially never collide, so the
  normal flow is unaffected.

### 2. Protocol (`crates/orion-core/src/protocol.rs`, `crates/orion-daemon/src/server.rs`)

- `ClientRequest::CreateWorktree` gains `existing: bool`. Pass it through in `server.rs` ~:307.
- Bump `PROTOCOL_VERSION` from 46 to 47. The frames are positional msgpack, so this is a breaking
  change.
- Every other constructor passes `existing: false`. Find them with `rg
  "ClientRequest::CreateWorktree" crates`; this includes the CLI `orion worktree` path in
  `crates/orion-tui/src/ipc.rs`.

### 3. TUI: the target (`quick_prompt.rs`)

- `QuickTarget::NewWorktree { project, branch }` gains `existing: bool`. Every current constructor
  passes `false`.
- Where the launch sends `CreateWorktree`, pass `existing` through. The stand-in row and Ack
  handling (`placeholder::stage_worktree`, `PendingIntent::SelectCreatedWorktree`) work unchanged.
- The box header's worktree field reads `checkout <branch>` instead of `new worktree <branch>` when
  `existing` is true. Find it where `new worktree` is rendered (`rg "new worktree"
  crates/orion-tui/src`).
- `QuickLaunch::is_new_worktree()` should stay true for both cases (the frame stays green). Check
  its callers.

### 4. TUI: the picker rows (`launcher.rs` `open_worktree_picker`)

- After the checkout rows, add one row per branch that has no checkout. Label local branches
  `⎇ <name>` and remote branches `⎇ origin/<name>`. Each row is a `MenuAction::PickLaunchWorktree`
  with target `QuickTarget::NewWorktree { project, branch: b.local_name().into(), existing: true }`.
  The ✓ logic works as-is, because the target equals the box's target when that branch was picked
  before.
- Skip a branch when it is `current`, when `checked_out_at` is `Some`, or when a row of
  `app.tree.worktrees` has that branch. Those already appear above as checkouts.
- **Data, never on the loop.** Expose the branch switcher's per-checkout cache as `pub(crate) fn
  cached_branches(app: &App, root: &WorktreeId) -> Option<Vec<Branch>>` in `branch_switch.rs`, keyed
  by the project's root worktree.
  - Open the picker with whatever is cached.
  - Kick the switcher's background list job, plus its throttled `git fetch --all` (once a minute at
    most), so new GitHub branches show up. Reuse the code behind `refresh()` / `open_for`; split out
    a `pub(crate) fn warm(app, root)` that starts both without opening the switcher.
  - When an answer lands and the open overlay is still this project's worktree picker
    (`menu.is_launch_worktree_picker()`), rebuild the rows in place. Rebuild the menu's
    `MenuFilter.all` and re-apply the current query, so typed text survives.
- Typing already filters rows through `MenuFilter`, so `alice` finds `⎇ origin/alice/new-billing`.
- The early return for a PR launch (`back.launch.pr.is_some()`) is unchanged.

### 5. Docs

- `docs/keys.md` ~:75 and ~:461: the `⌘.` picker lists every branch with no checkout, local and on
  origin; picking one checks it out into a new worktree for the launch.
- `docs/sessions.md` ~:331: the same sentence.

## Steps (in order)

1. `git.rs` `add_branch_worktree`. Put the tests next to `add_pr_worktree_*` (~:883), using
   `add_bare_origin`. They cover:
   - a remote-only branch gets a worktree that tracks `origin/<b>`, with HEAD at the origin tip;
   - a local branch behind origin is fast-forwarded;
   - a local branch with commits of its own is not moved;
   - the `origin/<b>` spelling is accepted;
   - an unknown name gives an error and leaves no directory behind.
2. `registry.rs`: the `existing` parameter, the row reuse and the bug fix. Model the tests on
   `create_worktree_runs_the_create_hook` (~:6491). They cover:
   - existing=true reuses a row;
   - existing=false with a remote-only name checks the branch out instead of cutting it off
     `origin/HEAD`.
3. Protocol + server + constructors + version bump. Run `cargo build --workspace` until it is
   clean.
4. `QuickTarget::NewWorktree.existing`, the header label and the launch pass-through.
5. `branch_switch::cached_branches` / `warm`, then the picker rows and the in-place rebuild. Put
   the tests near `the_worktree_picker_flips_the_launch_into_a_fresh_worktree`
   (`event_loop.rs` ~:34856). They cover:
   - with a seeded cache, the branch rows come after the checkouts;
   - branches that have checkouts are not listed;
   - picking a row and pressing `Enter` sends `CreateWorktree { existing: true, branch: "<local
     name>" }`, then the agent once the Ack lands;
   - a list that lands while the picker is open with a query keeps the query and the filtered rows.
6. Docs.

## Build and check

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Manual verification

Use a scratch repo with an origin. From a second clone, push `teammate-feat` with one commit.

1. Run `orion kill`, then start orion (the protocol was bumped).
2. Press `⌘N`, then `⌘.`, and type `teammate`. `⎇ origin/teammate-feat` is listed. If it was
   pushed seconds ago, it appears once the background fetch lands. Pick it, type a task, press
   `Enter`. A `teammate-feat` band appears with the agent in it. In that worktree:
   - `git log -1` shows the teammate's commit;
   - `git status -sb` shows `## teammate-feat...origin/teammate-feat`;
   - `.env` is linked, and the worktree hook ran.
3. Press `⌘.` again. `teammate-feat` is now listed as a checkout, not as a `⎇` branch row.
4. Project tab right-click → **New worktree** → type `teammate-feat2` (a branch that exists only on
   origin). It checks out the remote branch and does not cut a new one off main.

## Follow-ups (not in this change)

- A way to get the worktree with no agent at all. Today, sending the box empty starts the agent
  bare, which is close.
- `orion worktree --branch <existing>` for agents and the CLI.
