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

## Status (2026-10-03, work laptop)

An agent worked through this list on the work laptop. `[x]` means done or
dropped (the reason is given). Every open box needs Oliver at the keyboard: the
TUI, a Linear key, or a `/login`.

Orion keeps its data in `~/Library/Application Support/dev.orion.orion/` on
macOS (`orion config path`). `~/.orion/` is only the fallback.

## 1. First run on the work laptop

- [x] Clone and `cargo install --path crates/orion --locked`. Reinstalled after
      the worktree-filter commit below.
- [x] IT check for Rust. Dropped: Rust 1.99 was already installed.
- [x] `gh auth status`: logged in as `oliverkidd`, and the RiploData org
      answers (`gh pr list -R RiploData/riplo-os` works).
- [x] `orion --version` (0.42.0). `orion add` registered riplo-nightshift and
      riplo-os.
- [ ] Reload Ghostty config (`⌘⇧,`). Orion's keybind block is already in
      `~/.config/ghostty/config`.
- [x] Keep secrets out of `config.json`. It holds only the `claude-b` row.

## 2. Agent hook files

- [x] The three per-checkout paths are in `.git/info/exclude` for riplo-os and
      riplo-nightshift (riplo-os also ignores `.cursor/*` in `.gitignore`).
- [x] Back up `~/.codex/hooks.json`. Dropped: Codex isn't installed.
- [x] Model picker vs company allowlist: there's no `availableModels` in
      remote, managed or user settings and no MDM profile, so the stock
      aliases apply.
- [x] Strip "Don't mention the rename". Dropped: Oliver chose to keep it.

## 3. Verify worktrees

- [ ] In the TUI, `⌘N` a worktree on riplo-os and confirm `.env.local` is a
      symlink into the main checkout. riplo-os has no nested ignored `.env`
      files, only the root `.env.local`.
- [ ] Dirty delete shows "Unsaved work"; a clean delete is still one confirm.
- [x] Configurable worktree root. Dropped: Oliver keeps `<repo>-worktrees/`.
- [x] Fallow's audit caches: riplo-os had 59 of them as worktrees in the temp
      dir (33 already gone from disk). Pruned the dead ones. Orion now skips
      prunable worktrees and temp-dir worktrees of a repo outside temp
      (`crates/orion-daemon/src/git.rs`), so they never become bands or get
      `.env` symlinks.

## 4. Verify Linear (live API)

- [ ] Make a personal key (Linear → Settings → Security & access → Personal
      API keys) and put `LINEAR_API_KEY=lin_api_…` in
      `riplo-os/.env.local`. Neither work repo's code reads that name today.
      riplo-nightshift's design notes reserve it for the bot's service key,
      though, so don't put a personal key there.
- [x] API names: `User.assignedIssues` and
      `attachmentLinkGitHubPR(issueId: String!, url: String!)` both match
      Linear's published schema (`linear/linear` → `packages/sdk/src/schema.graphql`).
      A live call still waits on the key.
- [ ] Auto-link: team branches already carry the id (`feature/riplo-968-…`),
      and Orion's do too (`riplo-968-<slug>`). With the key, check whether
      RIPLO-968 already has its PR attached. If it does, Linear's GitHub
      integration links by itself, so turn **Link PRs to Linear** off.
- [ ] `⌘L`, mark two issues, launch, confirm the prompt and branch name. Then
      attach from `v` → `⌘L`.

## 5. Two Claude accounts (Option A prototype)

- [x] `~/bin/claude-b` wrapper (`CLAUDE_CONFIG_DIR=$HOME/.claude-b`).
- [x] `harnesses.claude-b` in `config.json`. `orion config harnesses` shows it
      matches the built-in Claude row apart from id, label and program.
- [ ] `CLAUDE_CONFIG_DIR=~/.claude-b claude`, then `/login` with the second
      account. `~/.claude-b` starts empty: no CLAUDE.md, skills or settings
      unless you link them in.
- [ ] Verify: separate login/usage; status dots and titles; resume after
      daemon restart; what the card does at a usage limit.
- [ ] If it works, decide later on an account picker. Don't start it unasked.

## 6. Use the product, then keybinds

- [ ] Leave keys alone until Oliver has used it for real work.
- [ ] Grid keys: Settings → Hotkeys. Modal keys: code change.
- [ ] Kill/unsure features on the review canvas: remove only when asked.

## 7. Later product work (not this laptop's first day)

- [ ] **Skills** picker at launch (design brief 3c).
- [ ] **Per-commit diff review** (design brief 3b).
- [ ] **Markdown editing**: parked. No WYSIWYG.
- [ ] **First release**: `git tag v0.1.0 && git push --tags`. Until then
      `install.sh` / `orion upgrade` fall back to `cargo install --git`.
- [x] **CI secrets**. Dropped: the repo is for local use, so both Claude
      workflows are deleted. `release.yml` stays (tags only, `GITHUB_TOKEN`).

## 8. Repo leftovers

- [x] `worktree_hooks::…hook_past_the_timeout_is_killed_with_what_it_started`:
      `sh` hadn't started within the timeout under load. Fixed; it now
      retries with longer timeouts.
- [x] `tui_drag_past_the_pane_top_autoscrolls_and_copies_the_run`: failed
      whenever `SSH_CONNECTION`/`SSH_TTY` were set. The harness now clears
      them for every test.
- [x] `orion_open_from_inside_a_session_raises_the_file_tabs`. Dropped:
      it never failed on this laptop (9 full runs, under load, with SSH
      vars). It may come from the other laptop's `~/.profile`, since the
      stub shell runs as a login shell.
- [x] Also fixed: a `config.rs` test swapped the process-wide `PATH`, so
      parallel tests sometimes failed to find `git`.
- [x] `chacha20` 0.10.1 → 0.10.2. `cargo audit` isn't installed
      (`cargo install cargo-audit --locked`, then `cargo audit`).
- [ ] `assets/*.png` still say "nebula". `make shot` when the UI settles.

---

## Cleanup (the last step)

When the boxes above are done or explicitly dropped:

1. Delete `DELETE-LATER.md`.
2. Search the repo for `DELETE-LATER` and remove any leftover mention.
3. Commit: `Remove the laptop handoff plan`.
