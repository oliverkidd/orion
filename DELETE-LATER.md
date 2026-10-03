# DELETE LATER

This file is a **handoff plan**, not product docs. It exists so an agent on the
other laptop can finish the leftover work without re-deriving it from chat.

**When every item below is done or dropped, delete this file** and commit that
cleanup. Do not leave a stub. Do not move the leftovers into the README.

Repo: https://github.com/oliverkidd/orion
Branch: `main`

---

## Already built (do not redo)

These shipped in `f00da21`. Verify on the work laptop; do not rebuild.

- **Linear `⌘L`** — browse issues assigned to you, Space marks, Enter launches
  one agent on the set (one worktree, one PR). From the PR modal, `⌘L` attaches
  the selected PR. Settings → Linear account (email; empty = owner of
  `LINEAR_API_KEY`). Auto-attach remembers the branch and links the first PR.
  Key: project `.env` / `.env.local` only, then process env. Never logged.
- **Ghostty** default outside terminal; Settings picker Ghostty vs Terminal.app.
- **Worktrees** symlink ignored `.env*` from the main checkout (existing files
  kept). Delete asks on dirty work ("Unsaved work").
- **Esc** closes any overlay / leaves the pane. `⇧Esc` sends Esc to the agent.
- **Command chords** throughout (Ctrl twins still work). Stolen: `⌘N` `⌘,`
  `⌘⇧P` `⌘P` `⌘O` `⌘R` `⌘F`. Left alone: `⌘Q` (quit) and `⌘C` (copy). Ghostty
  unbinds those so they reach Orion — reload config with `⌘⇧,`.
- **Markdown** is read-only in-app. Settings → File editor / `⌘O` opens Cursor
  or VS Code. No WYSIWYG. Brief 3a is parked.
- **Two Claude accounts** — Option A accepted (wrapper + second harness). Not
  first-class UI. Prototype below.
- **Keybinds** — leave the rest until Oliver has used the product; then rebind
  in Settings → Hotkeys. Modal keys still need code.

---

## 1. First run on the work laptop

- [ ] `git clone https://github.com/oliverkidd/orion.git` (or `git pull` if
      already cloned) then `cargo install --path crates/orion --locked`.
      Updating later: same two commands; see README → Updating.
- [ ] Check with IT before installing Rust if company policy needs it.
- [ ] `gh auth status` — org SSO authorised. PR and issue features use `gh`.
- [ ] `orion --version`, then `orion` inside the real work repo. Enter on the
      splash to add it.
- [ ] Reload Ghostty config (`⌘⇧,`) so Command chords work.
- [ ] Data lives in `~/.orion/`. Keep secrets out of `config.json` and presets
      (`orion ssh` copies that file).

## 2. Agent hook files (before the first session)

Orion merges managed hooks at spawn, tagged `_orionManaged`. It never replaces
your entries. Hooks do nothing outside Orion.

| File | Scope |
|---|---|
| `<worktree>/.claude/settings.local.json` | per checkout |
| `<worktree>/.cursor/hooks.json` | per checkout |
| `<worktree>/.cursor/rules/orion-title.mdc` | per checkout |
| `~/.codex/hooks.json` | global |
| `~/.pi/agent/...` Orion extension | global |
| `~/.config/opencode/...` Orion plugin | global |

- [ ] Add the three per-checkout paths to the work repo `.gitignore` or
      `.git/info/exclude`.
- [ ] Back up `~/.codex/hooks.json` if it exists.
- [ ] Confirm the model picker matches any company-managed Claude allowlist.
- [ ] Optional: strip "Don't mention the rename to the user" from
      `AUTO_TITLE_INSTRUCTION` in `crates/orion-daemon/src/hooks/mod.rs`.

## 3. Verify worktrees

- [ ] Create a worktree and confirm nested `.env` files (monorepo apps) landed
      as symlinks into the main checkout.
- [ ] Dirty delete shows "Unsaved work"; a clean delete is still one confirm.
- [ ] If `<repo>-worktrees/` does not match the company layout, a configurable
      root is still unbuilt — ask before adding it.

## 4. Verify Linear (live API)

- [ ] Confirm the env var is `LINEAR_API_KEY` in the work repo `.env` /
      `.env.local`.
- [ ] Settings → Linear account: type the Linear email if the key is shared or
      a bot, so "assigned to me" is actually Oliver.
- [ ] Confirm `viewer.assignedIssues` and `attachmentLinkGitHubPR` against the
      live API. If the mutation was renamed, the attach flash will say so —
      fix `crates/orion-tui/src/linear.rs`.
- [ ] Check whether Linear's GitHub integration already auto-links from
      `ENG-12` in the branch name. If yes, turn **Link PRs to Linear** off.
- [ ] `⌘L`, mark two issues, launch, confirm the prompt and branch name. Then
      attach from `v` → `⌘L`.

## 5. Two Claude accounts (Option A prototype)

Not product code. A wrapper plus a harness row.

- [ ] `~/bin/claude-b`:
      `export CLAUDE_CONFIG_DIR="$HOME/.claude-b"; exec claude "$@"`
- [ ] Add `harnesses.claude-b` to `~/.orion/config.json` (`program` = that
      wrapper, `hooks` = `claude`, same flags as the built-in Claude row).
- [ ] `CLAUDE_CONFIG_DIR=~/.claude-b claude` then `/login`.
- [ ] Verify: separate login/usage; status dots and titles; resume after
      daemon restart; what the card does at a usage limit (Orion ignores quota
      notifications today).
- [ ] If it works, decide later whether to build an account picker / "continue
      on the other account". Do not start that unless asked.

## 6. Use the product, then keybinds

- [ ] Leave keys alone until Oliver has used it for real work.
- [ ] Grid keys: Settings → Hotkeys. Modal keys: code change.
- [ ] Features marked kill/unsure on the review canvas: remove or hide only
      when asked.

## 7. Later product work (not this laptop's first day)

- [ ] **Skills** — show / pick installed skills when launching an agent
      (design brief 3c). Linear multi-select launch may grow a skill that
      groups issues; the product surface (`⌘L`) is already there.
- [ ] **Per-commit diff review** (design brief 3b).
- [ ] **Markdown editing** — parked. Do not build a WYSIWYG.
- [ ] **First release** — `git tag v0.1.0 && git push --tags`. Until then
      `install.sh` / `orion upgrade` fall back to `cargo install --git`.
- [ ] **CI secrets** — `.github/workflows/claude.yml` and
      `claude-code-review.yml` need `CLAUDE_CODE_OAUTH_TOKEN`. On a public
      repo, delete the workflows or lock them to this account before adding
      the secret. Without it they just fail.

## 8. Repo leftovers (any machine)

- [ ] Three tests also fail on untouched upstream; fix or mark flaky:
      `orion_open_from_inside_a_session_raises_the_file_tabs`,
      `tui_drag_past_the_pane_top_autoscrolls_and_copies_the_run`
      (`crates/orion/tests/e2e_tui.rs`);
      `worktree_hooks::tests::hook_past_the_timeout_is_killed_with_what_it_started`
      (fails only under the full parallel run).
- [ ] `chacha20 0.10.1` (via `rand`) is yanked. No advisory. `cargo update -p
      chacha20`, then `cargo test` and `cargo audit`.
- [ ] `assets/*.png` still say "nebula". Regenerate with `make shot` when the
      UI settles.

---

## Cleanup (the last step)

When the boxes above are done or explicitly dropped:

1. Delete `DELETE-LATER.md`.
2. Search the repo for `DELETE-LATER` and remove any leftover mention.
3. Commit: `Remove the laptop handoff plan`.
