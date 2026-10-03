# Next steps (work laptop / main repo)

Work that needs your work laptop, your main repo, or your GitHub account, so it could not be finished in this
import. Each item says what to do, why it waits, and which files are involved. "Canvas" refers to the
Orion review canvas: the audit, features, shortcuts, design briefs, worktrees and Claude accounts tabs.

## 1. Publish the repo

- [ ] Create `github.com/oliverkidd/orion` and push this repo:
      `git remote add origin git@github.com:oliverkidd/orion.git && git push -u origin main`
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

Orion creates worktrees at `<repo>/../<repo>-worktrees/<branch>`. That folder is fixed, and nothing is
copied into new worktrees. (Canvas: worktrees)

- [ ] Compare this with where your main repo's existing worktrees live and how its `.env` symlinks are
      made: a script, a git hook, or by hand.
- [ ] Quick fix to test: a create hook. Save it as e.g. `~/bin/orion-link-env`, `chmod +x` it, and set it
      in the main repo:

      #!/bin/sh
      # $1 = main checkout, $2 = new worktree
      cd "$1" || exit 1
      for f in .env .env.local; do
        [ -e "$1/$f" ] && ln -sf "$1/$f" "$2/$f"
      done

      git config orion.worktreeCreateHook ~/bin/orion-link-env

  List every env file your repo really uses (including nested apps in a monorepo).
- [ ] The hook does not run for worktrees you create outside Orion. Those keep whatever your own
      tooling does.
- [ ] Delete in the TUI always force-removes the worktree, so uncommitted work is lost. Commit or
      push before deleting.
- [ ] Then decide which reworks from the canvas worktrees tab to build: a "files to link" setting,
      a configurable worktree folder, safe (non-forced) delete, delete-the-branch, hooks for adopted
      worktrees, auto-clean after merge.

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

## 6. Linear: attach my issues to a PR

Design is in the canvas design-briefs tab (item 4). Settle these before building:

- [ ] Find which env var the main repo uses (assumed `LINEAR_API_KEY`) and whether it lives in
      `.env` or `.env.local`.
- [ ] Find whose key it is. A personal key makes "assigned to me" work. A shared or bot key would list
      the bot's issues and attach links as the bot, so you would need a personal key instead.
- [ ] Confirm worktree `.env` files are symlinks into the main checkout. The design follows symlinks
      only inside the main checkout.
- [ ] Check whether Linear's GitHub integration is on for your workspace. If it is, PRs that mention an
      issue ID may already auto-link, and the feature should avoid double links.
- [ ] Confirm the GraphQL `viewer.assignedIssues` query and the `attachmentLinkGitHubPR` mutation
      against Linear's current API with a curl call using your key.
- [ ] Then build it: Ctrl+l in the pull requests modal, issues grouped by status, Space to select,
      Enter to attach. The key goes only to `api.linear.app` via curl stdin, and is never logged,
      stored or passed to agents.

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
- [ ] Design briefs 3a (markdown editing), 3b (per-commit diff review) and 3c (skills) are ready to
      build. They don't need the work laptop, but test them on the real repo.
