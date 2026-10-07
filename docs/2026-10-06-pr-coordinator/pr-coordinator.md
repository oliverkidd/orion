# PR coordinator: base sync, conflict forecast, and coordinated autofix

## Problem

- Orion never fetches on a timer. Merging a PR from the app (`⌘X` in the PR modal) only re-reads the PR list (`pr_actions::land_done`, `crates/orion-tui/src/pr_actions.rs`). The local base branch is never moved and open branches are never checked against the new base.
- Autofix (`crates/orion-tui/src/autofix.rs`) launches one fresh `autofix-<n>` agent per broken PR (`autofix::dispatch`). When the base branch's CI breaks, every PR fails the same check and N agents fix the same thing in N branches. The agents that wrote each PR, who know its code best, are never told.

## Design

### Orion coordinates; agents only fix

The coordinator is a deterministic state machine inside orion, not an LLM session sitting on the base branch:

- Orion already polls every open PR (`event_loop::lookup_open_prs` / `sweep_open_prs`, which feed `autofix::note_list`). It already knows every agent's status (`AgentStatus` in `crates/orion-core/src/entities.rs`) and worktree, and can type into any session (`ClientRequest::Input`).
- An LLM coordinator would spend tokens just to wait and poll. Its plan would be lost on context compaction or a restart. Every message it sent would go through orion anyway.
- LLM agents are used only where judgement is needed: making a fix, and resolving conflicts.

The coordinator runs in the TUI, where autofix lives today, so it acts only while orion is open. It needs no daemon change, so every phase ships as a patch release. Moving it into the daemon is a later option.

### Incidents, not PRs

Each poll groups my broken PRs into **incidents**:

| Incident | Detected by | Owner |
|---|---|---|
| **Base broken** | A check failing on a PR also fails on the base branch's head commit | One new **base-fix** agent in a fresh worktree off `origin/<base>` |
| **Conflicts** | `mergeable == CONFLICTING`, or the local forecast (Phase 2) | The agents in that PR's worktree |
| **PR-only failure** | A failing check that passes on base | The agents in that PR's worktree |

A PR can sit in a base-broken incident and also have its own conflicts. The coordinator ignores the base-broken check on that PR and routes only the conflicts.

### Messaging the agents that did the work

A new primitive, **deliver**: paste a prompt into an agent's session as a bracketed paste (`PASTE_START`/`PASTE_END` in `crates/orion-tui/src/event_loop.rs`) followed by Enter.

- **When to deliver:** only once every agent in the worktree is idle (`Finished`, or `Fresh` with nothing in flight). This idle state is how orion "pauses" a worktree. The message waits in a per-worktree queue until then.
- **Why not interrupt:** stopping an agent mid-turn can leave half-written files and a dirty index right when the base is about to be merged in. Queued input also behaves differently in each harness.
- **Which agent:** the one most recently active in the worktree. Every other agent in that worktree gets a short note naming the agent that is doing the sync and asking them not to commit until it reports back.
- **An agent waiting on you** (`NeedsFeedback`): orion leaves it alone and raises a desktop notification.
- **No agent alive** in the worktree (`Terminated`, `Disconnected`, or none): fall back to today's `autofix-<n>` launch in that worktree (`create_pr_agent` reuses the PR's worktree).

### The base-fix agent

- It gets a worktree off `origin/<base>` and a prompt naming the failing check and its run URL.
- **It writes a brief first.** It investigates without changing anything, writes a brief, and ends its turn. The brief covers:
  - what is broken and the likely cause
  - the change it will make, and the files it will touch
  - how it will check the fix
  - that it will open a PR and auto-merge it into `<base>` once checks pass

  It writes the brief to a path orion gives it in the prompt, under orion's state directory (`briefs/<project>-<incident>.md`), so the brief is never committed.
- **You approve once.** When the agent is `Finished` and the brief file exists, orion sends a desktop notification. The Broken PRs modal then shows the brief in its right pane (rendered with `markdown_view`), with three choices:
  - **Approve:** orion delivers "Approved. Go ahead." The agent fixes, pushes, opens the PR and runs `gh pr merge --auto --squash`, and GitHub merges it once checks pass. Nothing has to watch it, and there is no second gate.
  - **Revise:** your note is delivered, and the agent writes a new brief.
  - **Cancel:** the incident is dropped, and the affected PRs fall back to per-PR routing.
- Auto-merge is on by default (`autofix_base_merge = auto`). Set it to `off` and the agent opens the PR for you to merge.
- While the incident is open, the base-broken check is marked as covered on every affected PR. The `autofix::Fingerprint` ledger gets a `covered_by: Option<u64>` naming the fix PR, so no agent is sent to fix it.
- Agents in the affected worktrees get one note: "`<check>` is broken on `<base>`; #N fixes it. Don't fix it here; carry on."

### Every merge into base triggers a sync pass

Triggers:
- an in-app merge landing (`land_done`)
- the PR poll seeing the base branch's head SHA move

Steps:
1. `git fetch origin` in the project's root checkout. If the root checkout is on the base branch and clean, `git merge --ff-only origin/<base>` (reuse `git_sync::pull`, `crates/orion-tui/src/git_sync.rs`).
2. **Forecast** each of my open PRs: `git merge-tree --write-tree origin/<base> origin/<head>` gives conflicts locally and instantly, without waiting for GitHub to recompute `mergeable`. Exit status 1 means it conflicts.
3. Re-run incident grouping. If the merged PR was a base-fix, close its incident and queue for each affected worktree: "`<base>` is fixed (#N). Merge `origin/<base>`, re-run checks, push."
4. PRs that would conflict: queue "merge `origin/<base>` and resolve conflicts" to the owning worktree. PRs that are only behind are left alone unless the setting `sync_behind_prs` is on.

### One modal: Broken PRs

This replaces the per-PR `Overlay::Autofix` form.

- One list. Base-broken incidents come first, each with the PRs it affects; then per-PR rows with tickboxes for Conflicts, Unit, E2E and Other (reuse `Issue` and `Picks` from autofix.rs, and the `marked` set pattern from `crates/orion-tui/src/linear.rs:1411`).
- Each row shows where its work will go: `→ claude in silent-cactus (idle)`, `→ queued: 2 agents busy`, `→ new autofix agent`.
- Enter dispatches everything ticked. `⌘G` in the PR modal opens it with the cursor's PR ticked.
- `pr_autofix = ask` opens this modal (through the existing `autofix::tick` gate) instead of one form per PR. `auto` dispatches the defaults.

## Retiring today's autofix

Autofix shipped earlier today in v1.0.21 (commit 7e58a18, plan in `docs/2026-10-06-pr-autofix/pr-autofix.md`). The coordinator replaces how it acts. How it detects problems stays.

| Part of `crates/orion-tui/src/autofix.rs` | Fate |
|---|---|
| Detection: `note_list`, `due_fetches`, `land_detail`, `diagnose`, `classify`, settle and recheck timers | **Keep.** They feed incident grouping. |
| `Fingerprint` / `Record` ledger, persisted through `pr_cache.rs` | **Keep**, adding `covered_by: Option<u64>`. |
| Prompt builders `prompt`, `context`, `instructions` | **Keep.** They become the fallback fixer's prompt; their conflict and check text is reused in delivered messages. |
| `AutofixForm`, `Overlay::Autofix`, its `draw`, keys and mouse handling, and `open_for` | **Retire.** Replaced by the Broken PRs modal. |
| `dispatch`: a fresh `autofix-<n>` per PR | **Demote** to the fallback when no agent is alive in the PR's worktree. |
| `ask`, `tick` (one form per PR, queued) | **Retire.** `tick` opens the Broken PRs modal once, with everything waiting. |
| `⌘G` in the PR modal | **Keep the key.** It opens the Broken PRs modal with the cursor's PR ticked. |
| e2e test `tui_autofix_asks_when_my_pr_conflicts` (`crates/orion/tests/e2e_tui.rs:1206`) | **Rewrite** for the Broken PRs modal. |

### Settings

The settings stay in Settings → Review, in a group renamed from "Autofix" to "Broken PRs". The onboarding wizard's Autofix page (`crates/orion-tui/src/onboard.rs`, `AUTOFIX_ROWS`) gets the same rows. Existing config keys keep their names, so nobody's `config.json` needs migrating.

| Key | Label | Values (default first) | Change |
|---|---|---|---|
| `pr_autofix` | When a PR breaks | `off`, `ask`, `auto` | Same values; new hint. `ask` opens the Broken PRs modal and `auto` runs the coordinator. |
| `base_fetch` | Fetch base branch | `5m`, `15m`, `off` | New (Phase 1). |
| `autofix_base_merge` | Auto-merge base fixes | `auto`, `off` | New (Phase 4). |
| `autofix_brief` | Brief before fixing | `base fixes`, `every fix`, `never` | New (Phase 4). With `every fix`, owner-agent deliveries and fallback fixers also brief first. |
| `sync_behind_prs` | Update PRs behind base | `off`, `on` | New (Phase 4). |
| `autofix_preset`, `autofix_model`, `autofix_effort` | Fix instructions, model, effort | unchanged | Now apply to base-fix agents and fallback fixers. |

Each new key needs a field, a default and a `SettingKind`. Wire it up the way `PrAutofix` is in `crates/orion-tui/src/config.rs` (the spec around line 1019, the field at 1638, the default at 2007, the value at 3385 and the cycle at 3586). Update `docs/configuration.md` and `docs/keys.md`.

## Phases

Each phase ships on its own.

### Phase 1: base sync

1. Add a `BASE_FETCH` beat (5 min) to the git-poll loop in `crates/orion-tui/src/event_loop.rs`, next to `OPEN_PRS_REFRESH`. It fetches the selected project's root checkout through `git_proc`, at most one fetch per project in flight.
2. After the fetch, fast-forward the root checkout if it is on the base branch, is clean, and has an upstream (same rules as `git_sync::pull`).
3. In `pr_actions::land_done`, on `Answer::Merged`, run the fetch and fast-forward at once.
4. Base branch = the daemon's `worktree_base_branch` setting, else `origin/HEAD`. Mirror the lookup the daemon uses in `crates/orion-daemon/src/git.rs` (`add_worktree_off_configured` / `origin_head`).
5. Tests: fake-remote fixtures as in `git_sync`'s tests (fetch moves `origin/<base>`; the fast-forward is skipped on a dirty tree or another branch).

### Phase 2: conflict forecast

1. A `forecast(root, base, head) -> Forecast { Clean, Behind(n), Conflicts(Vec<PathBuf>) }` in a new `crates/orion-tui/src/forecast.rs`, using `git merge-tree --write-tree --name-only`.
2. Run it for my open PRs after each base move (Phase 1) and when a PR's head SHA changes.
3. Show it on PR modal rows and worktree rows: a `⚠ conflicts` badge before GitHub says so.

### Phase 3: deliver

1. `crates/orion-tui/src/deliver.rs`: a per-worktree queue of `Delivery { text, kind }`, flushed when every agent in the worktree is idle, by `ClientRequest::Input` with a bracketed paste and `\r`.
2. Agent choice, the hold note to the other agents, and the fallbacks described in the design above.
3. Tests: a fake tree with agents in each status; assert what is sent, held, or falls back.

### Phase 4: incidents, briefs and the Broken PRs modal

1. Base CI status: `gh api repos/{owner}/{repo}/commits/<base>/check-runs`, polled with the PR list and only while one of my PRs is failing.
2. Grouping in autofix.rs, with `covered_by` added to the ledger (persisted through `crates/orion-tui/src/pr_cache.rs`).
3. The modal, replacing `AutofixForm`, following "Retiring today's autofix" above. Keep the prompt builders (`prompt`, `context`, `instructions`), and split the conflict text out for delivered messages.
4. The base-fix launch with its brief gate: a brief-first prompt, a watch for `Finished` plus the brief file, the brief pane, and Approve / Revise / Cancel. Approve and Revise go through Phase 3's deliver.
5. The new settings (`autofix_base_merge`, `autofix_brief`, `sync_behind_prs`) and the regrouped onboarding page.
6. The sync pass after a merge (steps 3–4 under "Every merge into base triggers a sync pass").
7. Rewrite the e2e test, and delete the retired form code.

### Phase 5: auto mode

`pr_autofix = auto` runs the whole loop without the modal. Keep the `AUTO_ATTEMPTS` cap per incident, and fall back to the modal when it is hit.

## Verification

- `cargo fmt --all && cargo clippy -p orion-tui --all-targets && cargo test -p orion-tui`
- Manual, on a scratch repo with three of my PRs:
  - Break a test on the base branch. Expect one base-fix agent whose brief appears before it changes anything, notes in the other three worktrees, and no per-PR fixers.
  - Approve the brief. Expect a PR with auto-merge on, merged once green, with no further prompt.
  - Merge the fix. Expect each worktree to be told to merge base.
  - Make one PR conflict with another, merge that other PR from the app. Expect a forecast badge within seconds and the conflict routed to the PR's own agent once it is idle.

## Decisions

1. **Coordinator:** orion's own state machine, not an LLM agent on the base branch.
2. **Base-fix self-merge:** allowed (`autofix_base_merge = auto`), gated by one brief you approve before the agent starts work.
3. **Pausing:** orion waits for the worktree to go idle; it never interrupts an agent mid-turn.
