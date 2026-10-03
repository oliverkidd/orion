# Next steps (work laptop / main repo)

Work that needs your work laptop, your main repo, or your GitHub account, so it could not be finished in this
import. Each item says what to do, why it waits, and which files are involved. "Canvas" refers to the
Orion review canvas: the audit, features, shortcuts, design briefs, worktrees and Claude accounts tabs.

## 1. Publish the repo

- [x] Public repo: `https://github.com/oliverkidd/orion` (this machine's `origin`).
- [ ] The history is a fresh single commit, so GitHub will not show it as a fork of
      `AgentSystemLabs/nebula`. The README and LICENSE carry the attribution.
- [ ] Decide public or private. `install.sh`, `orion upgrade`, `orion ssh` (remote install) and the
      update check all fetch from `github.com/oliverkidd/orion`. They only work once the repo is public and
      has a release, because they use unauthenticated `curl`.
- [ ] To publish a release, push a `v*` tag; `.github/workflows/release.yml` builds the binaries.
      Until then the footer's update check finds nothing and `orion upgrade` fails harmlessly.
      Set `ORION_UPDATE_CHECK_SECS=0` to turn the check off.
- [ ] CI secrets: `.github/workflows/claude.yml` runs on any comment, issue or review containing
      `@claude`, and `claude-code-review.yml` runs on every same-repo PR. Both need a
      `CLAUDE_CODE_OAUTH_TOKEN` secret. If the repo is public, delete them or limit them to your account
      before adding the secret. Without the secret they just fail. (Canvas: audit)

## 2. Install on the work laptop

- [ ] Install Rust (`rustup`). Check with IT first if company policy needs it.
- [ ] Build from source, so no release is needed: `cargo install --path crates/orion --locked`
- [ ] Check `gh auth status`. The account must be able to read your org's repos, and org SSO must be
      authorised for the token. The PR and issue features use `gh`.
- [ ] Run `orion --version`, then `orion` in your main repo.
- [ ] Data lives in `~/.orion/` (`config.json`, `config.local.json`, SQLite, logs). Keep secrets out of
      `config.json` and presets, because `orion ssh` / `orion tunnel` copy them to the remote host.

## 3. Agent hook files Orion writes (check before the first session)

At each agent spawn Orion merges its managed hooks into these files. It never replaces your entries,
marks its own with `_orionManaged`, and the hooks do nothing outside Orion. (Canvas: audit)

| File | Scope |
|---|---|
| `<worktree>/.claude/settings.local.json` | per checkout |
| `<worktree>/.cursor/hooks.json` | per checkout |
| `<worktree>/.cursor/rules/orion-title.mdc` | per checkout (always-apply rule) |
| `$CODEX_HOME/hooks.json` (default `~/.codex/hooks.json`) | global |
| `~/.pi/agent/...` Orion extension | global |
| `~/.config/opencode/...` Orion plugin | global |

- [ ] Add the three per-checkout paths to the main repo's `.gitignore`, or to `.git/info/exclude` if you
      can't change the shared one, so they never get committed. The main repo may already commit a
      `.cursor/hooks.json` or `.claude/settings.local.json`: Orion would add its entries to them and the
      file would show as changed.
- [ ] Back up any existing global `~/.codex/hooks.json` first.
- [ ] Check for company-managed Claude settings (managed `settings.json` / `availableModels`). Orion reads
      the allowlist for its model picker; confirm the picker shows the right models.
- [ ] Optional: remove "Don't mention the rename to the user" from `AUTO_TITLE_INSTRUCTION` in
      `crates/orion-daemon/src/hooks/mod.rs` if you don't want hidden instructions in your sessions.

## 4. Worktrees vs your symlinked .env setup

Orion creates worktrees at `<repo>/../<repo>-worktrees/<branch>`. **Link .env files** is on by
default: ignored `.env*` files from the main checkout are symlinked into new and adopted worktrees
(existing files are kept). Delete now asks before force-removing a dirty checkout.

- [ ] On the work laptop, create a worktree and confirm nested `.env` files (monorepo apps) landed
      as symlinks into the main checkout.
- [ ] Confirm a dirty delete shows the "Unsaved work" prompt and that a clean delete still goes
      through one confirm.
- [ ] Compare the `<repo>-worktrees/` folder with where your existing worktrees live. A configurable
      root is still unbuilt if you need to match the company layout.

## 5. Two Claude accounts

Orion has no account support today. (Canvas: Claude accounts)

- [ ] Prototype option A. Create a `~/bin/claude-b` wrapper that runs
      `export CLAUDE_CONFIG_DIR="$HOME/.claude-b"; exec claude "$@"`, then add this to `~/.orion/config.json`:

      "harnesses": {
        "claude-b": {
          "program": "/Users/<you>/bin/claude-b",
          "hooks": "claude",
          "resume_flag": "--resume",
          "system_append_flag": "--append-system-prompt",
          "model_flag": "--model",
          "effort_flag": "--effort"
        }
      }

- [ ] Log the second account in once: `CLAUDE_CONFIG_DIR=~/.claude-b claude` then `/login`.
- [ ] Verify:
  - each account keeps its own login and usage;
  - status dots, titles and the model picker work for `claude-b`;
  - resuming a `claude-b` session after a daemon restart works;
  - what happens when a session hits its usage limit (the card state; Orion ignores quota
    notifications today).
- [ ] If the prototype works, decide whether to build the first-class version: an account picker,
      a "limit reached" card state, and "continue on the other account".

## 6. Linear (built; verify on the work laptop)

`⌘L` lists issues assigned to you, Space marks, Enter starts one agent on the marked set (one
worktree, one PR). From the pull requests modal, `⌘L` attaches the selected PR. Settings → Linear
account is an email (empty = owner of `LINEAR_API_KEY`, kept in `config.local.json`). Auto-attach
is on: a branch cut from ⌘L remembers its issues and attaches the first PR that appears on it.

- [ ] Confirm the env var is `LINEAR_API_KEY` and that it lives in `.env` or `.env.local`.
- [ ] Settings → Linear account: type your Linear email if the key is a shared/bot key, so
      "assigned to me" is actually you.
- [ ] Confirm `viewer.assignedIssues` and `attachmentLinkGitHubPR` against the live API. If Linear
      renamed the mutation, the attach flash will say so.
- [ ] Check whether Linear's GitHub integration already auto-links (branch names carry `ENG-12`).
      If it does, you can turn **Link PRs to Linear** off.
- [ ] Run ⌘L in the real repo, mark two issues, launch, and confirm the agent prompt and the branch
      name look right. Then attach from `v` → ⌘L.

## 7. Check on the real repo

- [ ] Performance with your repo's size: the grid, diff (`g`), file finder (`f`), find in files
      (`Shift+F`), PR/issue lists, and memory (`Shift+M`).
- [ ] Your real shortcut changes: send your choices from the canvas ("Send my decisions to chat") and
      apply them. Grid keys can be rebound in Settings (`s`) > Hotkeys. Modal keys need code changes.
- [ ] Features you marked kill or unsure in the canvas: remove or hide them.

## 8. Known leftovers in this repo

- [ ] Three tests also fail on untouched upstream; fix them or mark them flaky:
  - `orion_open_from_inside_a_session_raises_the_file_tabs` and
    `tui_drag_past_the_pane_top_autoscrolls_and_copies_the_run` (`crates/orion/tests/e2e_tui.rs`)
    time out waiting for the `^q: sessions` footer hint;
  - `worktree_hooks::tests::hook_past_the_timeout_is_killed_with_what_it_started` fails only under
    the full parallel run.
- [ ] `chacha20 0.10.1` (pulled in by `rand`) is yanked. There is no advisory against it. Run
      `cargo update -p chacha20`, then `cargo test` and `cargo audit`.
- [ ] The `assets/*.png` screenshots still show the old "nebula" name. Regenerate them with the repo's
      screenshot tooling once the UI changes settle.
- [ ] Design briefs 3b (per-commit diff review) and 3c (skills) are ready to build. 3a (markdown
      editing) is parked: read-only rendering already exists, and Settings → File editor / ⌘O
      opens Cursor or VS Code for edits. They don't need the work laptop, but test them on the
      real repo.
