# Orion feature cull and UI rework

A full pass over orion: cut what isn't used, bake defaults in, regroup settings and setup, fix the bugs the pass found, and build the UI changes that were signed off. The decisions come from two sign-off pages: *Orion Feature Cull* (https://claude.ai/artifact/VB7sCjhXKnorsLJRagBonu) and *Orion Cull Round 2* (https://claude.ai/artifact/N5H3Q88AA2FDKmUX3whtLB). Everything below is the result; nothing here needs those pages to be read.

The work lives on the branch `feature-cull` (worktree `orion-worktrees/feature-cull`). Each step is its own commit, made locally and never pushed: every push to `main` is a release (see `CLAUDE.md`).

**Release:** this drops things people relied on (tunnel/browser, five editors, Pi and Muse, cloud sessions, the GitHub issues modal, the cards layout), and it touches the daemon. So it is a **major** release. Step 7 raises `[workspace.package] version` in the root `Cargo.toml` to `3.0.0`, and its commit message says why.

## How to work a step

- Start from the tip of `feature-cull`. A builder in its own worktree runs `git reset --hard feature-cull` on its own branch first; the earlier waves are already merged there.
- Read the step and its listed files first. Line numbers are approximate (`~`); find code by the names given.
- Keep the step's scope. If a step needs something from another step that isn't there yet, stop and say so rather than building it.
- Update the docs the step affects in the same commit: `docs/keys.md`, `docs/sessions.md`, `docs/configuration.md`, `docs/commands.md`, `docs/how-it-works.md`, `README.md`, `ARCHITECTURE.md`. Step 7 rewrites `configuration.md` as a whole, so in earlier steps it's enough to delete or correct the rows and sections you touch.
- Retired settings are dropped, not kept for older builds (step 2A's compat rule). One rule stays: an unknown key in `config.json` survives a save (the overlay patches only keys it knows, `orion_core::settings`).
- When you add daemon code under a new path, add the path to `crates/orion-daemon/daemon-inputs.txt`.
- Verification, every step:
  ```sh
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test -p orion-core -p orion-daemon -p orion-tui -p orion-fuzzy
  cargo test -p orion --no-fail-fast --test e2e_tui --test help_cli --test config_cli --test install_sh
  ```
  Add `cargo test -p orion --test e2e_pty` when the step touches the daemon or the protocol. It spawns real daemons and is slow. Without `--no-fail-fast`, an e2e_tui failure stops the other suites from running at all. Run the suites once before you change anything, so a failure you didn't cause is known up front.
- A retired config key needs no list: the config tests (`a_retired_key_loads_as_nothing_and_survives_a_save`, `the_newest_releases_config_still_loads_every_key`) let any key the build no longer reads load as nothing, as long as a save leaves it in the file.
- Keep scratch scripts in a folder named after your step, not loose in a shared scratch folder.
- Before committing code (not docs or tests), run the `simplify` skill over the diff.
- Commit message style: one plain sentence saying what the user gets, the way `git log` already reads (`The run line says when it's stopping`), then a short body.

---

## Wave 1: cuts (four independent steps)

### 1A. Hosting, editors, keys

**Remove `orion tunnel` and `orion browser`; keep `orion ssh`.**
- Delete `crates/orion/src/tunnel.rs` and `crates/orion/src/browser.rs`, their clap entries and examples in `crates/orion/src/cli.rs`, and their tests `crates/orion/tests/tunnel_cli.rs` and `browser_cli.rs`. `tunnel` imports ssh's prelude and browser's port helpers: whatever `ssh.rs` still needs moves into `ssh.rs`.
- Remove `make browser` from the `Makefile`.
- Keep `orion ssh`, the SSH hosts picker (`crates/orion-tui/src/hosts.rs`, palette entry **SSH hosts**) and the remote-mode branches (`is_remote`).
- Docs: `docs/commands.md` "Other machines, other screens" keeps ssh only. Remove the Tunnel and Browser paragraphs in `ARCHITECTURE.md`.

**Remove settings bundles and settings sync over ssh** (`orion config export` / `import`, `ssh_sync_config`).
- Delete `crates/orion-tui/src/bundle.rs`: `for_remote`, `apply_forwarded`, `encode`/`decode`, `merge_hosts` and the bundle format.
- Remove the `ORION_IMPORT_BUNDLE` handling and the `apply_forwarded()` call in `crates/orion/src/main.rs` (~line 21), plus the `--no-sync-config` flag.
- Remove the `ssh_sync_config` key and its Settings row.
- Keep `orion config path` and `orion config harnesses`.
- Update `crates/orion/tests/config_cli.rs`.

**Remove the Linux build and paths.**
- Remove the musl targets in `.github/workflows/release.yml` (~lines 92–105).
- Remove `install.sh`'s Linux branches and the fresh Linux tarball path.
- Remove `notify-send` notifications and the wl-paste/xclip clipboard reads (`crates/orion-tui/src/clipboard_image.rs` ~233).
- Keep the `cfg(target_os)` gates only where the code no longer compiles without them.
- Docs: remove the Linux paths and instructions (README "macOS · Linux" becomes "macOS").
- Update `crates/orion/tests/install_sh.rs` to match.

**Remove the ⌃-twin shortcuts for plain terminals.**
- Delete the ~25 `ctrl+…` second defaults in `crates/orion-tui/src/keymap.rs` `ACTIONS`, keeping each action's ⌘ chord.
- Delete `cmd_shown`, `set_in_orion_app` and `shown_side` (~1448–1495) and the "twin" column in `docs/keys.md`.
- Keep the bare-letter fallbacks `:` (palette) and `s` (settings), and keep `Ctrl+q` unlocking a terminal.
- Hints and footers print ⌘ chords only.

**Every editor except fresh goes.**
- Remove from `crates/orion-tui/src/editor.rs`: `EDITORS` and `FALLBACKS`, `MICRO_*`, `ensure_micro_config`, `micro_dir`, `is_micro`, the `Kind` variants for micro, Edit, vim, nvim, hx and emacs and their arms (`keys_hint`, `startup_keys`, `mac_key`'s micro and Edit arms), and the fallback in `resolve`.
- Also remove: `note_editor_fallback` and `editor_fallback_noted`; the vim/hx `Ctrl+Q` force-close; `editor_hints`' per-kind arms (`ui.rs` ~368); the install rows for these editors (`install.rs`); the onboarding Editor page rows (`onboard.rs` `editor_rows`, `editor_blurb`); doctor's fallback text; and `config.rs` `editor_label` and the File editor row.
- Keep: fresh's PATH lookup and the `brew install fresh-editor` plan, `FRESH_CONFIG` and the theme write, fresh's `editor_args`, kitty flag tracking and DA1/DSR replies, fresh's `mac_key` and `find_step`, and `cmd_as_ctrl`.
- Keep `ORION_EDITOR` as an escape hatch: any other command, opened as `<cmd> +<line> <file>`.
- A missing fresh shows one footer flash naming the install command; `i` runs it.
- Rename `VimTerm`, `app.vim`, `vim_tx`, `VimEvent`, `close_vim` and `vim_term.rs` to `Editor…` / `editor_term.rs`.
- `install.sh`: `ensure_editor` installs fresh only.

**Terminal.app as the outside terminal goes.**
- Remove the `OutsideTerminal` enum (`config.rs` ~34–66), `OUTSIDE_TERMINALS`, the Settings row, `TERMINAL_APP` and the install plan row.
- ⌘O → **Terminal in the checkout** always opens a Ghostty tab.
- Remove the `outside_terminal` key.

**The legacy Ghostty keybind-block cleanup goes.**
- Remove `retire_block`, `without_block`, `config_path`, `BEGIN`/`END` and their tests in `crates/orion-tui/src/ghostty_config.rs`, and the call at `event_loop.rs` ~318.
- Remove `ORION_GHOSTTY_CONFIG` (`crates/orion-core/src/env.rs` ~59, `crates/orion/tests/e2e_tui.rs` ~137), the `ghostty_keybinds` field, and the notifier-folder cleanup at `event_loop.rs` ~328.

**Open in app offers Cursor, VS Code and Sublime Text.**
- Keep `auto` (the first installed of Cursor, VS Code, Sublime Text). Remove `zed` and `default` as choices; with none installed, the macOS `open` fallback is still used internally.
- A missing app installs in one press: `i` on the row runs its Homebrew cask (`cursor`, `visual-studio-code`, `sublime-text`) through the existing install modal (`install.rs`).

**The Hotkeys tab becomes a read-only key reference.**
- Remove the capture UI, conflict confirm and host-reachability tables from `keymap.rs` and the settings handlers: `host_warning`, `GHOSTTY_BINDS`, `CTRL_COLLISIONS`, `Reach`/`reach_at`, `bind`, `conflicts`, `shadowed_by`; the capture handling at `event_loop.rs` ~9113–9227 and its draw code in `ui.rs`.
- Keep `Keymap`, `from_overrides` and hand-edited `keybindings` in `config.json`.
- The tab lists every action and its chord, with nothing to edit.

### 1B. GitHub and git

**The GitHub issues modal and issue sessions go.**
- Delete `crates/orion-tui/src/issues.rs`, `Overlay::Issues` / `ModalUnder::Issues`, the jump-list issue rows (`app.rs` ~2875), the pane's issue page (`ui.rs` ~4243), the header issue count (`ui.rs` ~3703), quick-prompt `issue`, the field_mouse issue fields, `⇧I`, the background `gh issue list` sweep and the `card_issue_number` setting.
- Daemon: remove `issue_rule` / `validate_issue_url` (`crates/orion-daemon/src/pr_scope.rs` ~208) and the `issue_url` create parameter in the protocol (`crates/orion-core/src/protocol.rs`). Leave the `agents.issue_url` SQLite column in place, unused.
- First move the key constants other modals borrow from `issues::keys` (`REFRESH`, `BROWSER`, `PROMPT`, `COMMENT`, `READ`, `EDIT`) and `issues::FRESH` into `crates/orion-tui/src/hints.rs`.
- Docs: `docs/sessions.md` "The ISSUES MODAL and ISSUE SESSIONS" and the keys.md rows.

**The current PR review form goes** (`⌘⇧R`, `pr_actions.rs` `ReviewForm`, `ReviewVerdict`, `submit_review`, `draw_review`, `review_key`). Step 5B builds its replacement. Until then, `⌘⇧R` opens the PR's Files tab on github.com.

**The second diff path in ⌘E goes.**
- Remove the `gh pr diff` path: `DiffView.prefetched` / `pr_url` (`app.rs` ~1328–1337), `refresh_pr_diff_view`, `pr_diff_files`, `split_unified_diff` (`event_loop.rs` ~2678–2711, ~2846–3135) and `pr-cache/diffs`.
- A PR's diff fetches `refs/pull/N/head` the way `conflicts.rs` (~400) already does and goes through the git path.

**The Stacks modal and the `docker compose start` fallback go.**
- Delete the ⇧S modal (`crates/orion-tui/src/stacks.rs`), `StackAction`, and `stack_start_line` (`crates/orion-daemon/src/registry.rs` ~2295).
- With no run command set, **Start stack** says so in the footer.
- Keep the `⬡` mark.
- The daemon's docker poll (`crates/orion-daemon/src/stacks.rs`) runs only while at least one client is attached.
- `worktree_containers` stays; step 2A moves it to Projects.

**The branch switcher goes** (`c`, `crates/orion-tui/src/branch_switch.rs`). `spotify.rs` borrows `cells`/`fit` from it: move those to `ui.rs` first.

**The "one at a time" ticked-commits mode goes** (`⌘G` in ⌘E, the `diff_ticked` setting). In `commit_list.rs` and `ui/diff_view.rs`, ticked commits always read together; `⇧←`/`⇧→` with nothing ticked still steps commit by commit.

**Merged-list paging and the slim-query fallback go.**
- Remove `MERGED_PAGES_MAX`, `fold_merged`, `MergedPage` and the "99+" from the PR modal's Merged section; it shows the first page of 100.
- Remove the `LIST_QUERY_SLIM` fallback (`pull_request.rs` ~694, ~989).
- The week review keeps its own merged fetch (`week_review.rs`); check it doesn't depend on the removed pieces.

**The `.orion.json` project file goes.**
- Delete `crates/orion-core/src/project_file.rs` and the daemon and TUI lookups.
- Run and open commands come only from the Projects settings (`projects.<repo>.run_command` / `open_command`).
- The footer message for a missing command names Settings → Projects.

### 1C. Grid, sessions, harnesses

**The cards layout goes; the list is the only layout.**
- Delete card rendering, hit-testing and the terminal cards' shell tail (`launcher.rs` ~430 lines, `ui/launcher_view.rs` ~700, `event_loop/launcher.rs` ~150).
- Delete the `worktree_layout` and `hide_card_marks` keys.
- `App::default` sets `launcher_list: false` (`app.rs` ~4875); make the list the only path and rebase the ~195 tests in `event_loop/launcher.rs` and ~50 in `ui/launcher_view.rs` that run on cards.
- Port the cursor card's status wash to the list row: `card_tint` (`launcher_view.rs` ~2472) applied in place of the plain `focus_tint` (~1738). It is on always, and the `highlight_current_card` key goes.

**The accordion goes.**
- Remove `launcher_expanded`, `Tab` folding, the `▾ N more · Tab: see all` line and `LIST_RECENT`.
- Every band is always open. The `expand_all_worktrees` key goes.

**The session pane's side switch goes.**
- Remove the right/bottom `session_pane` setting and the ⬓/◨ header button.
- The pane is on the right, with the automatic move to the bottom when the window is too narrow.

**Dead code goes:**
- the `(new)` badges (`config.rs` `added_on`, `is_new`, `NEW_SETTING_PREFIX`, `today_days`, `days_from_civil`, ~659–753);
- `card_issue_label` / `card_issue_rect` / `card_issue_hit` / `HitTarget::LauncherCardIssue`;
- `KindPicker::pr_session` / `pr_session_menu_rows` (`agent_picker.rs` ~61, ~234) and `open_pr_agent_picker` (`event_loop.rs` ~7815).

**Cloud sessions go.**
- Delete `crates/orion-daemon/src/pty/cloud.rs` and the registry's cloud paths (~150 lines).
- In the protocol, remove `cloud_prompt` and `SendCloudMessage`.
- In the TUI, remove `PromptKind::ClaudeCloudTask`, `CloudMessage`, `CloudPreview`, the picker's Tab-cloud, `draw_cloud_session` and the cloud badge.
- Leave the `cloud_session_id` column unused.
- Docs: sessions.md ~36–55.

**The separate preset task box goes.** Every preset launch goes through the quick prompt, as the new-worktree branch already does (`preset_overlays.rs` ~926–938). Remove `PromptKind::AgentPresetTask`.

**"Saved on this machine" goes.**
- Delete the Agents-tab section listing unused `~/.claude-*` dirs (`claude_accounts.rs` ~1230–1335, `AccountRow::OnDisk`, `MACHINE_GROUP`).
- Keep **Remove account**'s choice of keeping the dir or moving it to the Trash.

**Remember harness goes; missing CLIs are always hidden.**
- Remove the `remember_harness` key and its code (`config.rs` ~3365), and the Experimental tab.
- `hide_uninstalled_harnesses` goes as a setting and becomes always on: the pickers list only enabled harnesses whose CLI is found (the daemon still re-checks at launch).

**Pi, Muse and legacy `custom_harnesses` go. Claude, Codex, Cursor, Grok and OpenCode stay.**
- Remove Pi's and Muse's built-in rows (`crates/orion-core/src/harness.rs`), Pi's extension (`pi_extension.rs` + `.ts`), their hook dialects, model lists, install rows and keys (`pi_*`, `muse_*`).
- Remove the legacy `custom_harnesses` array (`CustomHarness`, its write routing in `config.rs` ~3110–3150). Keep the `harnesses` map, which can still add a CLI.
- Stored session rows of a removed kind are archived by a one-time migration in `crates/orion-daemon/src/store.rs`, never silently re-read as Claude (~1159).

### 1D. Linear, todos and the rest

**Todo ↔ Linear goes.**
- Remove the todos view's Linear section (`todos/view.rs` ~2089–2510), `linked_from_linear`, `refresh_linked` and the chip.
- In `linear.rs`, remove `LinearMode::Link`, `LinkedIssue`, `LinkedAsks`, `fetch_linked`, `fetch_teams`, `pick_team` and `create_for_todo`, and the Answer variants `Linked`, `Teams` and `Created`. Keep `create_issue` for step 6C.
- Remove the `Item.linear`, `Item.linear_seen` and `TodoFile.linear_team` fields; serde ignores them in old files.
- This also removes the bug where a linked todo's launch sent a Linear URL as a GitHub `issue_url` (`todos/view.rs` ~2082).

**The skills browser goes** (`⌘S`, `crates/orion-tui/src/skills.rs`).
- First move `tilde`, `Places`, `Trash` and `move_folder_to_trash`, which `claude_accounts.rs` (~431, ~1312, ~1386) and `doctor.rs` (~358, ~393) use, into a small `crates/orion-tui/src/paths_util.rs`.
- Add a palette action **Open skills folder** that opens `~/.claude/skills` in the tree browser.
- `⌘S` is unbound.

**The todos Markdown import goes; cut, copy and paste stay.**
- Remove the indented-bullets import (`todos/import.rs`).
- Pasted lines still become one item each.
- `⌘X` / `⌘C` / `⌘V` of items, between projects too, are untouched.

**The memory modal goes.** Remove the modal (`ui.rs` ~1864–2153 and its event-loop arms) and keep the footer's memory readout.

**`DELETE-LATER.md` goes,** along with any mention of it.

**Todo groups go two levels deep exactly.** Set `MAX_DEPTH` to 2, and remove header promote/demote (`promote` / `demote`, `todos/mod.rs` ~586–623) and the keys.md paragraph that describes it.

**Ticked todos are kept for 120 days.** Prune done items older than 120 days when a list loads; configuration.md's "What orion keeps" table says so.

**Linear launches send one set of instructions.**
- Remove the `linear_task_template` setting and the multi-line settings machinery that exists only for it (`config.rs` `is_multiline_text`).
- The template's built-in text (`DEFAULT_LINEAR_TEMPLATE`, `config.rs` ~2197) becomes the **Linear Ticket** system preset's built-in text, as PR autofix already works.
- A launch sends the preset's text, with the issues filled in at `{issues}` / `{ids}` / `{first_id}`, and nothing else (`quick_prompt.rs` ~584).

**A Linear batch links every PR that names its issues.** In `attach_new_prs` (`event_loop.rs` ~1945), also attach a new open PR whose title or body names a waiting issue's identifier, not only a PR on the launch's branch (`linear.rs` ~2243–2279).

**Linear asks for team states once.** `ISSUE_FIELDS` (`linear.rs` ~4803) asks `team { states }` on every issue. Read the states once under `teams` and look them up by team id.

**One-time migrations instead of read-forever shims:**
- LinkStore's `attached` bool, bare-branch keys, undated links and multi-branch `branches_of` (`linear.rs` ~1153);
- `TodoFile::upgrade` (`todos/mod.rs` ~161);
- `pr_autofix: "ask"`, rewritten to `auto` on load.

---

## Wave 2: settings and setup (one step, after wave 1)

### 2A. Five settings tabs, defaults baked in, four setup pages

The tabs become **General · Accounts · Projects · Presets · Keys** (`SETTINGS_TABS`, `config.rs` ~769).

**General:**
- *Launching:* Follow new session (on/off), Idle session timeout, Auto-fix my PRs (on/off), Open files in (auto · Cursor / VS Code / Sublime Text).
- *Sounds:* Turn finished, Needs you.
- *Look:* Theme, Animations, Spotify in footer.

**Accounts:**
- The Claude accounts: sign in and out, rename, add, remove, and the tag's letter and colour (step 4D).
- Each account's Model and Effort.
- Then a section per other enabled harness (Codex, Cursor, Grok, OpenCode) with Enabled, Model and Effort.
- Which account `⌘N` uses.

**Projects,** per project:
- Base branch (moves from the global `worktree_base_branch`; the daemon reads `projects.<repo>.base_branch`, else `origin/HEAD`).
- Setup command (step 4B adds it), Run command, Open command.
- On worktree delete (moves from the global `worktree_containers`).
- Linear key status, with `Enter` to test the connection (moves from the Linear tab).

**Presets:** the preset manager (step 4C builds its screen; in this step the tab can open the existing list).

**Keys:** the read-only reference from 1A.

**Baked in, rows deleted:**
- *Always on:* link `.env` files, finder closes on open, prewarm dead sessions, Claude and Cursor usage, show all worktrees, black background, link PRs to Linear.
- *Fixed:* `⌘K` Enter just selects (`palette_enter_attaches` false), ⌘E starts on commits, base fetch every 5 minutes.
- *Into their forms:* merge method (remembered per repo from the last merge), delete merged branch (a box in the merge form, default on), new PRs as drafts (the create form's box, default off).
- *Remembered by the viewer:* files as a tree (⌘B flips it and the next ⌘E opens the same way).
- *Gone:* delete emptied worktree (a worktree's last card asks nothing about the worktree; the band stays, as `show_all_worktrees` on does today).
- *Hand-edit only:* Linear account email (`linear_assignee_email`), `keybindings`, `account_tags`, `claude_models`.
- `Focus` (`quick_prompt_focus`) goes; Follow new session is the one switch.
- The **New worktree** row (`quick_prompt_new_worktree`, `config.rs` ~493) goes, and launches start in the checkout under the cursor; `⌘⇧N` (step 4D) starts a new worktree.

**System presets carry the agents orion starts itself.**
- **PR autofix:** absorbs `autofix_model` / `autofix_effort`; "Runs on" defaults to the PR's own agent.
- **Linear ticket:** the template text, from 1D.
- **Week review:** absorbs `review_account` / `review_model` / `review_effort`; runs on sonnet · medium by default.
- A system preset always holds text. Clearing it restores orion's text, and it can't be deleted or renamed.
- Orion's built-in PR autofix text adopts these rules: fix only what is listed; resolve conflicts; reproduce each failing unit test locally and fix it; re-run only the e2e specs that failed, never the full suite, unless earlier attempts failed the same way; push; watch the PR until it is green and repeat.

**Status rows leave Settings.**
- git and gh are checked by `orion doctor`, and by a footer message where they fail.
- The Linear key row moves to Projects.
- Setup reopens from the palette (**Run setup**) and `orion setup` only.

**Drop the compat machinery.**
- Remove every retired key's field and doc row: `git_init_on_create`, `ghostty_keybinds`, `skip_session_naming`, `confirm_on_archive`, `focus_tint`, `show_workspaces`, `hide_card_prompt`, `card_line_changes`, `hide_root_worktree`, `hide_projects`, `hide_worktrees`, `hide_sessions`, `recent_prompts`, `recent_prompts_count`, `show_key_combos`, `pr_issue_counts`, `quick_prompt_new_worktree`, and every key the waves above removed.
- Keep one config fixture, for the newest release, in `crates/orion-core/fixtures/`, and delete the per-version ones.
- In configuration.md, the "Compatibility rules" section shrinks to the one unknown-keys rule.

**Setup is four pages:** Welcome · Claude · First project · Ready (`crates/orion-tui/src/onboard.rs`).
- *Claude:* is the CLI installed (`i` installs it); sign in each account; add one.
- *First project:* pick the repo; set its base branch, setup and run commands.
- *Ready:* the keys that matter (`⌘N` `⌘⇧N` `⌘K` `⌘E` `⌘L` `⌘I` `⌘,`).
- The App, Agents, Editor, Worktrees, Linear and Autofix pages go, and so does the setup-version replay of newly added steps (`SETUP_VERSION`, `setup_version`, `view_pages`' `since`).
- **What's new** stays as its own sheet, shown once after an upgrade (`whats_new.rs`, `seen_version`).

---

## Wave 3: accounts and harnesses

### 3A. A Claude account is Claude plus an account id

- Model accounts as `AgentKind::Claude` with an account id (reuse the `custom_harness` column for the id), not as `AgentKind::Custom` harnesses (`crates/orion-core/src/harness.rs`, `claude_account.rs`, `crates/orion-tui/src/claude_accounts.rs`).
- Migrate existing account rows once.
- **Model sync then covers both accounts:** `session_model.rs` ~240 stops skipping them.
- **One store per account for model, effort and label:** migrate the flat `claude_model` / `claude_effort` and `harnesses.claude.label` into the `harnesses` map on load, and delete the three write paths (`config.rs` ~2503, ~3110–3195).
- Cut `HarnessOverride` to `enabled`, `label`, `model_default`, `effort_default`, `models` and `env`, and remove `orion config harnesses`.
- **A preset follows the box's account unless it pins one.** Add "follow the box" as the default harness of a preset (`agent_presets.rs` ~26); `QuickLaunch::of_preset` (`quick_prompt.rs` ~556) keeps the box's account when it's set.
- Must keep working, with a test each:
  - **Continue on another account** (`crates/orion-daemon/src/continue_on.rs`) moves a session and its conversation between accounts.
  - Both accounts stay signed in at once.
  - Tab in the `⌘N` box switches between them.
  - Usage reads each account.
  - Resume after a daemon restart works for both.

### 3B. Grok and the other harnesses, brought up to date

- **Grok** runs with process-only status today. If the installed `grok` CLI exposes hooks and a session id (check `grok --help` and its docs at https://docs.x.ai/build/overview), wire them up as Codex's are: status, needs-you, session-id capture for resume, and title sync. If it doesn't, keep process-only status and say so in the picker row.
- Refresh the built-in model lists for Codex, Cursor and OpenCode to what their CLIs offer today.

---

## Wave 4: run line, setup command, presets, accounts on the grid

### 4A. The run line

- **Start leaves the cursor where it was.**
  - `StartRun` is sent as `AttachCreated{focus:false}` (`event_loop.rs` ~5990), and its Ack goes through `attach_created` → `show_created_terminal` / `select_when_seen` / `attach_now` (~14269–14314, `event_loop/launcher.rs` ~516).
  - Add a `RunStarted` intent whose Ack does none of that: the upsert alone puts the row in its band.
- **Stop shows `stopping… 1:52`** (a countdown to the 2-minute kill, `registry.rs` ~82).
  - The footer holds "stopping run · ⌘⇧S again kills it now" while it lasts.
  - The daemon broadcasts the state: add `stopping: bool` to `TerminalTab` (`crates/orion-core/src/entities.rs` ~383), set it from `stop_run` (`registry.rs` ~2318), and clear it on exit.
- **A stop you asked for removes the row.** In the exit handler (`registry.rs` ~3285–3293), an id in `stopping_runs` calls `close_terminal` instead of parking the row in `finished_runs`.
- **An exit nobody asked for keeps its row.**
  - Send the exit code on `TerminalTab`: 0 shows a faint `done`, non-zero a red `exited 1`.
  - The copper `th.exited` (`theme.rs` ~193) goes.
- **Ports.**
  - Read `http://(localhost|127.0.0.1|0.0.0.0):PORT` off the run's output into a sticky set per run, reset on restart. `links.rs` `visible_links` already finds URLs on a terminal's screen; startup lines scroll away, so keep what was seen.
  - Show the ports as clickable underlined `:3000 :5173`, the first one opening in the browser. Also add **Open app** (the first port) to the ⌘O menu.
- **Layout** (see 4D): the run line's command and ports sit in the description column the session rows use for their last prompt; its status (`running`, `stopping… 1:52`, `done`, `exited 1`) sits at the far right, where session rows show their age.
- **Restart.** Add a **Restart stack** verb (palette, the row's right-click menu): a stop, then a start once it exits.

### 4B. A per-project setup command, instead of worktree hooks

- Add **Setup command** to each project's Projects settings: `projects.<repo>.setup_command`, read fresh by the daemon.
- When a worktree is created, the daemon runs it in a visible **setup** terminal row in that worktree's band, with no time cap, through the login shell, as run commands are. Its row reads `setting up…`, then disappears on a clean exit or shows `setup failed (1)` in red.
- **Start stack** waits for a running setup to finish first, and the footer says so.
- Remove worktree hooks (`crates/orion-daemon/src/worktree_hooks.rs`, `orion.worktreeCreateHook` / `orion.worktreeDeleteHook`, `ORION_HOOK_TIMEOUT_MS`). Worktree containers keep running on delete, as before.
- Docs: replace configuration.md's "Worktree hooks" section with "Setup command". Note for the user: their git-config hook (`git config --get orion.worktreeCreateHook` in riplo-os) should become the setup command `just setup-worktree`.

### 4C. The presets screen

Built as Settings → **Presets** (`crates/orion-tui/src/preset_overlays.rs`, `agent_presets.rs`):

- **Two sections.** *Used by orion* comes first: PR autofix ("sent when one of your PRs breaks"), Linear ticket ("wraps every ⌘L launch"), Week review ("writes your week review"). Each has a dim `orion` tag and a line saying what it's for. *Yours* follows, then **+ New preset**.
- **Keys follow the selected row.**
  - On a system preset: `Enter` edits its text and `⌃R` puts back orion's text. Delete and rename are neither offered nor hinted.
  - On one of yours: `Enter` edits, `⌃N` new, `⌃D` delete.
- **An empty system preset** shows orion's built-in text greyed.
- **One text box with a `{task}` marker,** replacing prefix, postfix and both, and the `preset_text` setting. An old preset reads as its prefix, then `{task}`, then its postfix.
- **"Runs on"** defaults to following the box's account (3A); the PR autofix preset defaults to the PR's own agent.
- **The editor shows a preview** of what the agent will get.
- The picker on `⌘U` (in a pane) and `⇧Tab` (in the box) only picks; managing presets lives in Settings.
- Fix the stale module comments in `preset_overlays.rs` (they describe the retired Sessions and Worktrees panels).

### 4D. Account chips, the list grid, usage-aware launching

- **Account chip.** Draw it as a filled chip: the initial, black on the account's tint (`theme.rs` `ACCOUNT_TINTS`), the same width as today's tag (`ACCOUNT_TAG_W`).
  - Draw sites: `draw_list_row` (`launcher_view.rs` ~1689), `session_line` (~1753), the usage modal (`usage.rs` ~1179) and Settings → Accounts.
  - Add an optional `icon` character to `account_tags` that replaces the letter.
  - Archived rows get a dim chip; the `mono` theme draws it inverse.
- **Hovering the chip** puts the account, email, usage left and reset time in the footer. Add `HitTarget::AccountTag(id)` to the hover whitelist (`event_loop.rs` ~12503). A click opens the usage modal on that account.
- **The pane header names the account** (`pane_title`, `launcher_view.rs` ~3341): ` R  Riplo` in its tint.
- **Settings → Accounts:** `c` cycles the account's colour and `g` sets its letter or glyph (writing `account_tags.<id>`).
- **Grid list:**
  - Keep today's spacing between bands.
  - The selected row is washed in its status colour (1C).
  - The empty band's hint reads `nothing running · ⌘N new agent · ⌘⇧N new agent in a new worktree · t terminal`.
  - No Linear IDs or ports on the band's rule.
- **`⌘⇧N`** starts a new agent in a new worktree in one press: the quick prompt opens aimed at **+ new worktree**.
- **Usage-aware launching.**
  - The box's agent field and the picker rows show each account's headroom (`Riplo 41% · 2h`), from the usage readings.
  - When the default account has hit its limit (a session's `usage_limit`, or a 0% reading), the box starts on the other account and says so.
  - With only Claude accounts on, Tab in the box switches account in place.

---

## Wave 5: review, PRs and changes

### 5A. ⌘E knows the PR and its conflicts; Enter edits at the line

- **Top line:** `#1342 Fix PDF header · ✗ 44/45 checks · 3 files conflict with origin/dev`. It reads the checkout's PR from the PR store; nothing is shown without one.
- **Conflicts are worked out locally.** Run `merge_tree(origin/<base>, HEAD)` (`conflicts.rs` ~416) off the loop, keyed by the HEAD and base SHAs. It works before a PR exists and needs no network, because base sync keeps the base fresh.
- **Conflicting files are marked `U`** in the file list, reusing `pr_preview.rs` ~1166.
- **`Enter` on a `U` file** opens the conflict resolver (`conflicts::open`, ~669), or says `autofix-<n> is on it` when an autofix agent is (`autofix.rs` ~675).
- **`⌘U` in ⌘E** opens this branch's PR page in the PR modal, and `Esc` comes back. `⌘U` is already the pull-requests key everywhere.
- **`Enter` on a diff line** opens fresh at that line in that file (`spawn_editor_modal` at `line_on_screen()`, `app.rs` ~1586). The diff reloads when the editor closes, the way the conflict resolver does (`conflicts.rs` ~1334, `view.reload()`). The same works from a PR's diff when its head is checked out locally.
- **Conflict hunks are not drawn inside the diff.**

### 5B. PR review, rebuilt

Replaces the old form (removed in 1B). It opens from the PR modal (`⌘⇧R`) and the PR page.

- **Data:**
  - Fetch the PR's review threads with GraphQL (`reviewThreads { nodes { id isResolved path line originalLine comments { nodes { id author { login } body createdAt } } } }`) alongside the existing detail.
  - Comments are written as a *pending* review (`addPullRequestReview` with no event, then `addPullRequestReviewThread` / `addPullRequestReviewComment`).
  - Submit is `submitPullRequestReview` with `APPROVE`, `REQUEST_CHANGES` or `COMMENT` and an optional body.
  - Reply is `addPullRequestReviewThreadReply`; resolve is `resolveReviewThread`.
  - All of it goes through `gh api graphql`, off the loop.
- **Layout** (signed-off mockup):
  - A file column with each file's thread count, then the diff.
  - Each thread is a block under its line: theirs on a grey rule, yours on the accent.
  - `c` on a line writes a comment in place (`⇧↑↓` widens it to a range), held pending.
  - A bottom bar holds the verdict (Comment · Approve · Request changes, only what GitHub allows on your own PR) and an optional summary. `⌘⇧R` submits the review with everything pending.
  - `r` replies, `x` resolves, `]` jumps to the next thread, and `⌫` on your pending comment drops it.
  - Leaving keeps what's pending: it's GitHub's pending review, so it survives.
- The PR page's Reviews tab lists the same threads grouped by file.

### 5C. PR page "work" block; autofix reads cleanly

- **Work block.** Between the PR page's title line (`pr_preview.rs` `sentence`, ~612) and its tabs, add up to three lines, each only when it applies:
  - **AUTOFIX:** the live `autofix-<n>` session and what it's doing, or `✗ 1 failing check, 3 files conflict · ⌘G send autofix` when the PR is broken and nothing's on it.
  - **LINEAR:** the issues linked to the PR (from `linear-links.json` and `app.linear`), with their state.
  - **SESSIONS:** the agents on the head branch, with account chips.
  - A click on a session lands on it in the grid, and a click on an issue opens it in ⌘L.
  - `⌘G` works from the pane's PR page, not only the modal (`pr_modal.rs` ~1188).
- **Autofix:**
  - Fold unit, e2e and other into one **Failing checks** list grouped by workflow, removing `classify` (`autofix.rs` ~156–186) and the three form rows.
  - Name the conflicting files in the prompt, from 5A's merge-tree (today it says only "Merge conflicts with base", ~887).
  - Uses the PR autofix system preset's text (2A).

### 5D. New PR with a Promote mode; one place to edit a PR; Closed PRs

- **Create form** (`pr_actions.rs` `CreateForm`), `⌘N` in the PR modal, has two modes at its top:
  - **Feature branch:** today's form, re-laid out like the mockup, with the branch's commits listed.
  - **Promote:** From = the base branch (`dev`), Into = the repo's default branch (`main`). It lists every PR merged into `dev` since `main` last moved, with their Linear IDs and authors, and fills the title (`Release <date>: <top titles>`) and a description built from that list (`## Going out`, one line per PR). If a `dev → main` PR is already open, the form says so and `⌘↵` updates its title and description instead of opening a second one.
  - `⌘↵` creates.
- **Edit a PR in one place.** A free chord (check `keymap.rs` and the kept-keys list `ghostty_config::KEPT`; `⌘;` is a good candidate) opens an **Edit pull request** form on the PR under the cursor:
  - title, base branch (retarget), draft / ready, labels (from the repo's labels), reviewers (from collaborators) and assignees;
  - **Close** / **Reopen** with a comment and the branch's fate. The close part replaces today's `⌘W` form, and `⌘W` opens this form at its Close row.
  - It runs `gh pr edit` / `gh pr ready` / `gh pr close` / `gh pr reopen`.
- **Closed PRs.** The PR modal gets a **Closed** section after Merged: PRs closed without merging in the last 30 days, with why where GitHub says (a base branch deleted). `Enter` on one opens Edit at Reopen with the base field focused, so a PR GitHub auto-closed when its base was deleted can be reopened and pointed at a live branch in one form.

---

## Wave 6: todos, week review, editor links, Linear, instant agents

### 6A. Todos' Done panel; the week review screen

- **Todos (⌘I):** the lists on the left; the right column is always the **Done** panel.
  - Scoped to your ticked todos only: Today · Week · Month · Quarter, the day (or week) bars, and items grouped by project with the time or day ticked.
  - Clean and quiet, styled like the rest of the modal.
  - `Space` on a done item puts it back. `→` moves into the panel, `←` back.
  - Remove today's DONE page and the `✓N today` click target (`todos/history.rs`).
- **Week review (⌘⇧Y)** is its own screen:
  - The right panel lists past reviews (newest first, the last 12 per project); select one and `Enter` opens it.
  - **New review** shows the options: Whose (Just mine / Everyone), Where (this project / all projects), the range (Week by default), the sources (merged PRs, Linear done, todos ticked) and the Note.
  - `Enter` runs the review through the Week review system preset (2A) and opens the result on a fresh page: Show/Say points by area, numbered, with the merged-but-not-listed count.
  - The existing gather, prompt, check and finish code stays (`week_review.rs`); only the screen around it changes.
- **Todos and sessions:**
  - An item with an agent on it is ticked when that agent's PR merges (the item stores the agent, which leads to its worktree and PR).
  - The session's row shows the todo it came from, as a dim `☐ <text>` after its prompt.

### 6B. Editor and file links

- `orion open file:42` opens at line 42 (`file_tabs.rs` ~537 hardcodes line 1).
- `file:42` typed in ⌘P opens at the line (reuse `with_line`, `app.rs` ~1969).
- ⌘P gets a **Changed** section from the worktree's git status, above Recent.
- **The markdown page:**
  - reloads when the file changes on disk (an mtime check on the existing 2 s mode-reassert beat, `host_terminal.rs` ~34);
  - follows relative `.md` links, with `Esc` going back, and opens http links in the browser;
  - `Enter` edits at the source line of the top visible row (invert `seek_row`, `markdown_view.rs` ~178).
- ⌘O from the editor opens Cursor at the editor's cursor line, read off fresh's status bar (`editor.rs` ~109; read it the way `find_prompt_up` reads "Search:", `vim_term.rs` ~288).
- **Find in files:**
  - a context preview pane (`tree_browser::file_preview`, ~67) and the match highlighted;
  - hits grouped by file, and a regex toggle (today `-F` is hardcoded, `grep_search.rs` ~33);
  - the query prefilled from the pane's drag selection.
- The tree browser marks each row's git status and can reveal the file the editor or diff is on.
- **Send `path:line` to the agent.** A key in the editor inserts the location into the worktree's agent's prompt, quoted the way `clipboard_image::insertion` does (~78). Pick a chord with `ghostty_config::KEPT` in mind.

### 6C. Linear

- **No duplicate worktrees.** `Enter` on an issue that already has work (`work_of`, `linear.rs` ~1608, finds a branch or session) offers **Go to session** instead of cutting a second worktree (`confirm`, ~3594).
- **Comments.** Fetch `comments(first: 20)` when the cursor rests on an issue, show them in the reading pane, and add them to the launch prompt (`issue_sections`, ~434).
- **Faster to open.** Prefetch the list when a project is selected, and cache it on disk like `pr-cache` (today it loads only when ⌘L opens, ~1820).
- **The jump list.** Linear issues (from the cache) and open todos are rows in ⌘K (`palette.rs` `build_palette_items`, ~324).
- **New issue.** `⌘N` in the Linear view files a new issue into Triage, reusing `create_issue`.
- **Hints.** Trim the 16-key hint row (`linear.rs` ~2700–2717) to what fits.
- **On the grid.** Add **Open Linear issue** to the session's right-click menu (`event_loop.rs` ~7705).

### 6D. Instant new agents: a warm next worktree

This replaces the warm spare pool, which never serves a launch today (it warms the Personal account, never adopts for an account, and can't take a task).

- **Delete the pool:**
  - `prewarm_agent`, `take_prewarmed`, `reap_prewarmed` and the hook buffer (`registry.rs` ~27–42, ~1376–1520);
  - the `PrewarmAgent` request, `PrewarmInfo` and `fire_keepwarm`;
  - `default_claude_prewarm` (`event_loop.rs` ~11862);
  - the `prewarm_agents` key.
  - Keep `prewarm_sessions`, which boots a worktree's dead sessions as the cursor lands.
- **Add the spare:**
  - Per project used in the last 24 hours, the daemon keeps one **spare worktree**, cut from the project's base branch (2A), with its setup command run (4B) and a `claude` booted on the `⌘N` account at its default model and effort, sitting at its prompt.
  - The spare is hidden from the grid and from `git worktree` sync adoption until it's taken.
- **A new-worktree launch takes it** (`⌘⇧N`, a Linear launch, the box's **+ new worktree**):
  - the branch is renamed to the name the launch would have used (`git branch -m`; the directory keeps its name);
  - the worktree appears in the grid;
  - the task is typed into the booted CLI as its first turn (`send_turn`, `registry.rs` ~2492);
  - a new spare is cut behind it.
  - A launch on another account, model or effort, or with a PR rule, boots cold as before.
- **Refresh:** when the base branch moves and the spare is untouched, it's reset to the new base (`git reset --hard` is safe: nothing in it is anyone's) and its CLI restarted.
- **Cost:** one idle worktree and one 150–300 MB process per active project. The footer's memory readout counts the spares.

---

## Wave 7: docs and the version

### 7A. Docs, version, final gate

- **Rewrite `docs/configuration.md`** to the settings that remain: about 250 lines; one row per key; the Projects, Accounts and Presets sections; Setup command; ssh; logs; what orion keeps.
- Re-read `docs/keys.md`, `docs/sessions.md`, `docs/commands.md`, `docs/how-it-works.md`, `README.md` and `ARCHITECTURE.md` against the code.
- **Fix these known drifts:**
  - fresh uses orion's own `orion` theme written to `~/.config/fresh/themes`, and has animations on;
  - the hosts picker has no default key;
  - the notifier app paragraph;
  - `⇧Tab` vs `⌘U` for presets.
- Raise `[workspace.package] version` in `Cargo.toml` to `3.0.0`. The commit message lists what was dropped and why.
- Run the full gate: `make ci`, then `cargo test -p orion --test e2e_pty`.

---

## Not in this build

- **A real phone view:** a small web page served by the daemon (over Tailscale, behind a token) listing sessions with their status and last lines, a reply box, and approve/deny for permission prompts. It replaces `orion browser`, removed in 1A.

## Order and dependencies

```
1A  1B  1C  1D          (parallel)
        2A              (after wave 1)
   3A        3B         (after 2A; parallel)
4A  4B  4C  4D          (after 3A; 4D after 1C too)
5A  5B  5C  5D          (after wave 4; 5C after 5A)
6A  6B  6C  6D          (6D after 3A and 4B)
        7A
```

## Build log

**Wave 1 (done, merged into `feature-cull`).** 1A, 1B, 1C and 1D, then `cargo clippy -D warnings` made to pass (seven lints a newer clippy raises). Where the build differs from the steps above:
- `PROTOCOL_VERSION` is 50 (cloud sessions and the terminal shell tail gone, `issue_url` off `CreateAgent`). Store migrations 30 and 31 archive cloud rows and Pi/Muse rows.
- A pull request is attached to a waiting Linear issue when its **title** names it; the open-PR list doesn't fetch bodies.
- Open in app's install is `i` then Enter, like every other install row.
- The ⇧D / ⇧W / ⇧F shift-letter keys and the `^` chords inside modals stay; only the grid's ctrl twins went.
- A paste of several lines into the todos with no field open adds one item per line. Groups nested deeper than two levels are lifted into their top-level group on load.
- Moving a folder to the Trash is macOS-only (`paths_util::move_folder_to_trash`).
- Known flaky: `tui_autofix_sends_unasked_when_my_pr_conflicts` (passes alone). Known failing on `main` too: `tui_diff_steps_through_a_branch_commit_by_commit` (times out on "the alpha body").

