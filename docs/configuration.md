# Configuration

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

CONFIG.JSON is the settings file, in the DATA DIR beside the SQLITE STORE — hand-editable, and
what the `s` SETTINGS OVERLAY writes:

- **macOS**: `~/Library/Application Support/dev.orion.orion/config.json`
- **Linux**: `~/.local/share/orion/config.json`
- `ORION_CONFIG_FILE` moves that one file — into a dotfiles checkout, say — and leaves everything
  else where it is. A save writes through a symlink instead of replacing it, so linking `config.json`
  into a repo works as well.
- `ORION_DATA_DIR` moves the whole directory, config included (tests, parallel instances).
- `orion config path` prints where each settings file is.

CONFIG.LOCAL.JSON, beside it in the DATA DIR, has the same keys and wins over `config.json` one key at
a time. It is for what only makes sense on this machine — an editor a remote box lacks,
`prewarm_agents: false` on a small one — and it is never exported, never sent over `orion ssh`, and
never written by an import. Create it by hand with just the keys to pin; after that the overlay
writes a key it holds back into it, so changing that setting here keeps it local, and every other key
into `config.json`:

```json
{ "editor": "vim", "prewarm_agents": false }
```

Both halves of orion read the two files. The TUI owns most keys; the DAEMON owns
`worktree_base_branch`, `worktree_containers`, `session_idle_timeout`, `prewarm_agents` and `prewarm_sessions`, reads
`custom_harnesses`, `harnesses` and `claude_accounts` beside the TUI (spawn and resume go through
them), and reads one key out of each `projects` entry, `run_command`. Each side
deserializes only its own fields and ignores the rest, and both load
them fresh on every use — so a hand edit applies without restarting either. No key is required: a
missing file is all defaults, an unknown field is skipped, a value this build can't read costs only
that key (it takes its default and stays as stored in the file), and a malformed file is logged and
ignored rather than failing the operation that read it. The overlay patches only the keys it knows
and leaves everything else in the JSON untouched, so hand-written fields survive a save. See
[Compatibility rules](#compatibility-rules) for why.

Files beside it in the DATA DIR: `orion.db` (the SQLITE STORE), `config.local.json` (above),
`agent_presets.json` (AGENT PRESETS), `cursor_models.json` (the cached `cursor-agent --list-models` answer, refreshed after 24h),
`ssh_hosts.json` (the SSH HOSTS FILE), `reviewed.json` (REVIEWED MARKS). All but the database are
convenience stores: missing or malformed reads as empty.

## Every setting

Seventy-five keys. **Overlay** is the SETTINGS OVERLAY tab whose row edits the key; `—` means the key
exists only in the file, so it is hand-edit-only. Most rows toggle or cycle on `Enter` / `←` / `→`; a
*typed* row (`worktree_base_branch`, the Project tab's **Run command**) opens a one-line prompt on
`Enter` instead, pre-filled with the stored value, and an empty answer puts its default back — the
Linear tab's **Task template**, which runs over lines, a multi-row box (`⇧Enter` breaks a line). The Agents tab groups its rows under **Quick
prompt**, **Claude accounts** and — while a removed account's dir is still there — **Saved on this
machine** (see [Claude accounts](#claude-accounts)), then one header per harness —
**Claude (you@example.com)**, **Codex**, **Cursor**, **Pi**, **Muse**, **Grok Build**, **OpenCode** — so a harness's rows read `Enabled` / `Model` /
`Effort` under its name rather than repeating it. The **Project** tab is the one tab whose rows are
not orion's but one project's: the project the grid is scoped to, named with its path on
the tab's first line, and each row there reads and writes that project's own entry under `projects`
— so the same row shows a different value on the next project over. The **Linear** tab gathers every
Linear option in one place ([Linear](#linear)). The **Experimental** tab holds
behaviors that change how the tree is worked; every switch there is off by default.

| Key | Type | Default | Overlay | What it does |
|---|---|---|---|---|
| `palette_enter_attaches` | bool | `true` | General | `Enter` on a session in the jump list (`⌘K`) attaches and focuses the TERMINAL PANE. Off, `Enter` only lands on the card and previews it — except on a session that NEEDS FEEDBACK (the red row), which attaches either way, since the only thing to do with a jump to a question is answer it. `Ctrl+o` / `⌘F` still pick open / focus explicitly either way. The `.` / `,` attention jump lands the same way this setting says, red rows included. |
| `git_init_on_create` | bool | `true` | — (retired) | Through 0.37, **git init new projects** (Settings → General): DAEMON-owned, off left a directory the ADD PROJECT BROWSER (`o`) created without a repository, and the add then failed. Every project is a git repository now: a folder the browser creates is always `git init`ed, and opening an existing folder in no repository asks in a CONFIRM DIALOG whether to `git init` it — `y` inits it and opens it, `n` leaves it alone. So nothing reads the key and no tab edits it; the TUI still loads and writes it back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `worktree_base_branch` | string | `""` | General | DAEMON-owned WORKTREE BASE BRANCH: where every new WORKTREE nobody named a base for starts — a bare `orion worktree`, the QUICK PROMPT's auto-created one (`orion worktree --base` always wins). Empty, shown as `auto` in the overlay, is origin's own default branch: `origin/HEAD` freshly fetched, normally `origin/main`. A name — `master`, `develop` — is resolved the way `--base` resolves one: origin is fetched and origin's copy of that branch (`origin/master`) is the start point, untracked, never the checkout's local branch of that name, which is only as new as its last pull; a branch origin lacks that the checkout has locally is used as named. The setting is one name for every project, so a repo with no branch of that name at all does not fail the launch: it falls back to `origin/HEAD` as if the key were empty, and `daemon.log` says which repo ignored it. A leading `origin/` is dropped (`origin/master` means `master`); a tag or SHA is not a branch and falls back too — name those with `--base`. Typed, not cycled: `Enter` on the row opens a prompt, an empty answer puts `auto` back. |
| `editor` | string | `"fresh"` | Tools | **File editor**: the BUILT-IN EDITOR every file opens in — Go to file (`⌘P`), the TREE BROWSER (`⌘B`), find in files (`⌘⇧F`), ⌥click, a MARKDOWN PAGE's `Enter`. The overlay cycles the VS Code-style editors first — `fresh`, `micro`, `edit` (Microsoft Edit) — then `vim`, `nvim`, `hx`, `emacs`, its hint naming the ones installed; any command passes through verbatim, so a hand edit can name one the picker doesn't. Each is told the line its own way: micro `<file> +<line>`, Edit, fresh and Helix `<file>:<line>`, the rest `+<line> <file>`. micro, Edit and fresh quit on their own `Ctrl+Q` after asking to save; `Ctrl+\` force-closes any of them ([Keys](keys.md#the-built-in-editor)). Text wraps: `micro` runs off orion's own config dir (`<data dir>/micro`, never your `~/.config/micro`), made on first use with `Ctrl+D` bound to add the next match as another cursor, the keys the Mac editing chords become ([Keys](keys.md#the-built-in-editor)) and `softwrap`/`wordwrap` on — a binding or setting already in its `bindings.json`/`settings.json` is left as it is — and orion presses Edit's `Alt+Z` (its word wrap, which no setting turns on) once Edit has drawn; fresh, vim and emacs wrap by default. `fresh` runs off orion's own config file too (`fresh --config <data dir>/fresh/config.json --no-upgrade-check --no-restore`, never your `~/.config/fresh/config.json`): just the one file — no menu bar, tab bar, scrollbar, file explorer, workspace dock (its `orchestrator` plugin off) or restored session, no whitespace dots, `~` lines, edge fade or animation, no update checks — a status bar of the cursor, the cursor count and fresh's messages on the left and the language on the right, the `default` keymap, and fresh's `dark` theme (VS Code's Dark+ colours) on orion's own background (`use_terminal_bg`). fresh loads a theme file only from `~/.config/fresh/themes`, which orion never writes to, so a theme of orion's own palette isn't possible; set `theme` in that file to pick another built-in. As with micro, a key you set there — at any depth — is left as it is. A chosen editor that isn't installed is never swapped in silence: the first installed of `fresh`, `micro`, `edit` and `vim` opens instead, the footer says so the first time, and the row reads `nvim — not installed, opens fresh`; `i` on the row installs it ([Installing editors and agent CLIs](#installing-editors-and-agent-clis)), and `install.sh` puts fresh on a machine that has none of fresh, micro and Edit. A `.md` file opens as its rendered MARKDOWN PAGE, `Enter` there editing it ([Keys](keys.md#the-markdown-page)). `ORION_EDITOR` overrides it for the process. |
| `outside_editor` | string | `"auto"` | Tools | **Open in app**: the GUI editor `⌘O` hands a file to — from Go to file, find in files, the TREE BROWSER, the skills browser, a MARKDOWN PAGE and the BUILT-IN EDITOR — and the OPEN MENU's **Checkout in …** row opens the checkout in: `cursor`, `vscode`, `sublime`, `zed`, or `default` (macOS `open`, whatever the system opens that kind of file with). `auto` is the first of Cursor, VS Code, Sublime Text and Zed installed, else the system default; the row shows which (`auto · Cursor`) and its hint lists the ones installed. An app is launched through the command-line tool inside its own bundle in `/Applications` or `~/Applications` (`Cursor.app/Contents/Resources/app/bin/cursor`, `Visual Studio Code.app/…/bin/code`, `Sublime Text.app/Contents/SharedSupport/bin/subl`, `Zed.app/Contents/MacOS/cli`), then the one on `PATH` — never Cursor's agent CLI shim, the `~/.local/bin/cursor` the `cursor-agent` installer writes, which only forwards to another `cursor` and otherwise fails — then `open -a`. A tool goes to the file's line (`cursor`/`code` with `--goto <file>:<line>` in the checkout's window, `subl` and `zed` with `<file>:<line>`); `open -a` and the system default open the file at its top. The hints name the app (`⌘O: VS Code`). A named app that isn't installed, or a tool that fails, says why in the footer instead; over ssh the file opens in the BUILT-IN EDITOR. |
| `link_env_files` | bool | `true` | General | DAEMON-owned ENV LINKS (**Link .env files**): every new WORKTREE, one an agent moves into, and one made outside orion that the sync adopts gets the main checkout's git-ignored `.env*` files (`.env`, `.env.local`, `apps/web/.env.development`, …) as symlinks at the same paths, so each checkout runs against the clone's secrets and local settings. A tracked file (a committed `.env.example`) and anything under `node_modules` are left alone, and a path that already exists in the worktree is never replaced. Off links nothing new; links already made stay. |
| `worktree_containers` | string | `"off"` | General | DAEMON-owned WORKTREE CONTAINERS (**Worktree containers**): what deleting a worktree does to the docker compose projects started in it — `off` leaves them, `stop` stops them, `remove` runs `docker compose down` (volumes kept), `remove+volumes` removes their volumes too. Matched by the directory compose recorded on each container, never by name; a value this build doesn't know is `off`. See [Worktree containers](#worktree-containers). |
| `outside_terminal` | string | `"ghostty"` | Tools | **Outside terminal**: the app `⌘O` → **Terminal in the checkout** opens, in the selected worktree's directory — `ghostty` (a new Ghostty tab) or `terminal` (a Terminal.app window). Ghostty not installed in `/Applications` or `~/Applications` opens Terminal.app instead. Off macOS or over ssh nothing opens. See [Outside terminal and Ghostty keybinds](#outside-terminal-and-ghostty-keybinds). |
| `ghostty_keybinds` | bool | `true` | Tools | GHOSTTY KEYBINDS: keep a marked block in Ghostty's config that releases every ⌘ chord orion's keymap uses — your rebinds included — so none of them is swallowed by Ghostty. See [Outside terminal and Ghostty keybinds](#outside-terminal-and-ghostty-keybinds). |
| `onboarded` | bool | `false` | — (config.local.json) | Whether the ONBOARDING wizard — **Orion setup** — has been seen. While it is false, orion opens it over the grid at startup, a step at a time under a STEP STRIP. In a terminal that sends no ⌘ — Terminal.app on a Mac, outside tmux and ssh — it starts on the GHOSTTY STEP: `Enter` reopens orion in a new Ghostty window in the same folder, its keybinds already in Ghostty's config, and setup carries on there (with Ghostty missing, `Enter` installs it first, or opens its download page without Homebrew); `→` stays with the `^` twins. Then: which agents to turn on, each with its default model and its CLI `installed` or `install…`; the Claude accounts (while Claude is on); the editors — the **File editor** choices, each installed or `install…`, and the **Open in app** choices with the app each opens here; the worktree defaults (**Worktree base branch**, **Link .env files**); Linear (the Linear tab's rows, the same values in the same order); autofix; then what was chosen, and the keys to press next. The outside terminal and the Ghostty keybinds are on Settings → Tools. `i` installs a missing CLI or editor ([Installing editors and agent CLIs](#installing-editors-and-agent-clis)). `Esc` or a click outside skips it; either way the key is set and it does not come back. It lives in `config.local.json`, so a remote reached over `orion ssh` asks on its own first run. Delete the key to see the wizard again. |
| `setup_version` | number | `0` | — (config.local.json) | The SETUP VERSION this machine last went through. A release that adds a step to **Orion setup** bumps it; on the next launch an onboarded machine on an older version sees only the new steps that have something to offer it (a terminal that sends no ⌘ gets the Ghostty step), then Ready — and with nothing to offer, the version is stamped quietly. 0 on a config from before the key, read as 1. Setup opens again in full from the palette's **Run setup**, Settings → Tools → **Setup**, or `orion setup`. |
| `seen_version` | string | `""` | — (config.local.json) | The orion version this machine last saw **What's new** for. On the first launch of a newer build — after `orion upgrade` or the footer's update — setup opens as **What's new in Orion**: every release since this one, stacked newest first however many the upgrade jumped, each with its date and what its commits said; then the setup steps added since (`setup_version`) and Ready, or with none missed, the notes alone. Closing it stamps this build's version; it is never lowered, so an older or dev build in between shows nothing twice. A first run stamps it without showing notes. Empty on a config from before the key: read as the release that brought its `setup_version` (2 → 1.0.17, 3 → 1.0.21), or as nothing seen. The notes are baked in at build time from git — each release is the commits between its `v*` tag and the one before (`crates/orion-tui/build.rs`) — so a build without git or tags has none, and setup carries on. |
| `close_finder_on_open` | bool | `true` | General | Opening a file closes the FILE FINDER behind the editor modal (or the MARKDOWN PAGE), so quitting the editor is one Esc instead of two. Off leaves the results underneath. Never touches the TREE BROWSER (its editor is its own preview pane) or ⌥click. |
| `ssh_sync_config` | bool | `true` | General | SETTINGS SYNC: `orion ssh` and `orion tunnel` send this machine's `config.json` and AGENT PRESETS along, and the remote orion merges them into its own settings before it starts — so a remote is set up the way this machine is on every connect, without reconfiguring it. Its `config.local.json` still wins there, and its projects, sessions and SSH HOSTS FILE stay its own. `--no-sync-config` leaves the settings behind for one connection. See [Backup, restore and other machines](#backup-restore-and-other-machines). |
| `linear_auto_attach` | bool | `true` | Linear | **Link PRs to Linear**: a branch a `⌘L` launch cut is remembered, and when the project's open list first shows a pull request on it, orion attaches the pull request to each of its Linear issues through Linear's API (`attachmentLinkGitHubPR`). The link is orion's, so it holds when the branch name carries no issue ID — the case Linear's own GitHub integration misses — and it is a closing link (`linkKind: closes`), the kind Linear's pull request automations act on: the issue moves when the pull request opens and when it merges, as it would had the branch named it. Linear keeps one attachment per pull request, so an issue that is linked twice still shows it once. An issue has one branch at a time: sent to another worktree (a `⌘L` launch, `⌘.`, or attached by hand to another pull request), it leaves the branch it was on, and orion deletes the attachment it made for that branch's pull request, so the old pull request's merge no longer moves the issue. A link Linear made itself is left alone, and a pull request can still close several issues. Off, nothing is remembered or attached. |
| `linear_assignee_email` | string | `""` | Linear (config.local.json) | **Linear account**: whose open issues `⌘L` lists. Empty, shown as `the key's owner`, is the owner of the project's `LINEAR_API_KEY`; an email names another member of the workspace, for a key a team shares. A typed row. Always written to `config.local.json`, so `orion ssh` never carries it to a remote. |
| `linear_task_template` | string | `""` | Linear | **Task template**: the task `⌘L`'s `Enter` fills the QUICK PROMPT with for the issues picked. `{issues}` becomes each issue's ID, title, link and description, `{ids}` the IDs comma-separated, `{first_id}` the first one. Empty, shown as `default`, is the built-in template — one commit per issue, one pull request titled `"{ids}: …"` whose description starts `Fixes {ids}`. `Enter` on the row opens a multi-row box on the template the box would get, the default spelled out; sending the default back unchanged keeps it the default. |
| `skip_session_naming` | bool | `false` | — (retired) | Through 0.30, **Skip starting prompt**: on, new AGENTS launched straight from the NEW AGENT PICKER instead of stopping at a task box first. Every launch now goes through the QUICK PROMPT (`⌘N`, or **New agent — choose harness** after its harness pick), whose `Enter` on an empty box starts the CLI bare, so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `confirm_on_archive` | bool | `false` | — (retired) | Through 0.34, **Confirm on archive**: on, `a` and the row menu's **Archive** asked in a CONFIRM DIALOG before archiving a session; off, the default, archived at once. Every archive asks now, whatever the key says — the dialog names the session and says `u` brings it back — so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `session_idle_timeout` | string | `"5m"` | Sessions | DAEMON-owned IDLE TIMEOUT: how long a session in a WORKTREE no client is viewing goes unwatched before the IDLE REAPER kills its PTY. See the values below. |
| `done_sound` | string | `"Glass"` | Sessions | The DONE SOUND rung when a turn you haven't seen reaches FINISHED: `off`, `bell` (the terminal BEL — silent in Ghostty unless its `bell-features` include `audio`), or a macOS system sound from `/System/Library/Sounds` played with `afplay` (`Glass`, `Ping`, `Pop`, `Hero`, …). Over `orion ssh` and off macOS it is always the bell. Stepping the row in the settings overlay plays each sound. It rings only for news: a turn that finishes in the pane on screen was watched and rings nothing. It waits 3 s for the finish to settle, and stays quiet if by then the session is working again, has been looked at, or was archived or deleted. It rings once per unseen stretch — the follow-up turns a session starts on its own (a task notification, a queued message) ring nothing more until you have looked at it, and then its next finish rings again. And it never rings within 10 s of the last sound of either kind, so a burst of finishes is one ding. The one switch for that finish's DESKTOP NOTIFICATION too: while the terminal window is in the background, each finish that settles is named in one (`Fix Login finished`, under its project and branch) — even when the 10 s rule kept the sound quiet — on the same terms as `feedback_sound`'s. `off` silences the sound and the notification together. |
| `feedback_sound` | string | `"Sosumi"` | Sessions | The FEEDBACK SOUND rung when a turn stops at NEEDS FEEDBACK — a permission prompt or a question — or when a session's CLI dies with an error in the middle of a turn, with the same values and fallbacks as `done_sound`, and a different default so red and green sound different from the next room. It never rings for the session whose pane you are locked into typing at while the terminal window has focus: that prompt, or that error, is already under your hands. The one switch for their DESKTOP NOTIFICATIONS too: while the terminal window is in the background (from the focus reports orion asks the terminal for — tmux needs `focus-events on`), each session that goes red is also named in a desktop notification (`Fix Login needs feedback`), and so is each that crashes (`Fix Login stopped with an error`). On macOS they come from orion's own NOTIFIER APP — a copy of Homebrew's `terminal-notifier` re-badged with orion's name and logo, built on first use under the data dir, whose click brings the terminal back; `osascript` when `terminal-notifier` is not installed — and from `notify-send` on Linux; never over `orion ssh`, where the desktop is the wrong machine's; a notifier that is missing or fails is a debug line, not an error. `off` silences the sound and the notifications together. |
| `preset_text` | string | `"prefix"` | Sessions | PRESET TEXT: which side of the task a new AGENT PRESET's text goes — `prefix` (one box, sent before the task), `postfix` (one box, sent after it) or `prefix & postfix` (both, the form every preset had through 0.32). The PRESET EDITOR opens a new preset with the box(es) named here, and its **Text** row changes one preset — a side the row leaves out saves blank; editing a stored preset also shows any side that already holds text, so nothing saved is ever hidden. Prefix alone by default: the framing most people reach for, and one box to fill. A hand-edited `both` reads as `prefix & postfix`; anything else as the default. See [Sessions](sessions.md#agent-presets). |
| `delete_empty_worktree` | bool | `false` | Sessions | DELETE EMPTIED WORKTREE: what the delete of a linked WORKTREE's last live card asks — the last session's `Backspace`, the last terminal's close, or a **Delete all sessions** that takes them all. Off, the default, that card's own CONFIRM DIALOG carries the question too, before anything is deleted (`Delete agent 'x'? Its session and history go away.` then `Nothing else is left in worktree 'feature': delete it from disk too?`), with three answers: `Enter` or `y` deletes the card and then the worktree, the same forced delete the band's own `Backspace` runs; `n` deletes the card and keeps the empty checkout; `Esc` cancels and keeps the card alive. On, the question is not asked: the dialog is the card's ordinary two-way confirm, its message saying the worktree goes with it, and `Enter` deletes both — unless archived sessions are still filed under the checkout, whose history the delete would take: those always get the three-way question, which says how many go with it. The ROOT WORKTREE is never offered either way, and archiving a session (`a`) never counts as emptying: an archived card is still filed under its checkout. With `show_all_worktrees` on, off asks nothing — the emptied worktree keeps its band — and on still deletes the worktree with its last card. See [Sessions](sessions.md#the-grid). |
| `show_all_worktrees` | bool | `true` | Sessions | SHOW ALL WORKTREES: every checkout of the project gets a BAND on the grid, one with nothing running in it too. It is on by default. Off, the grid is only what is running: a checkout with no session or terminal has no band. On, such a checkout is an EMPTY BAND — its rule over one line, `nothing running · ⌘N: new agent · t: terminal · ⌫: delete worktree` — which `↑`/`↓` walk onto, where `⌘N`/`t` start work, and where `Backspace` (or **Delete worktree** in its right-click menu) deletes the worktree behind its own confirm (`Delete worktree 'feature' from disk?`); the ROOT WORKTREE's band offers no delete. On also means deleting a worktree's last card never asks about the worktree, so the emptied band stays until its own `Backspace` — unless `delete_empty_worktree` is on, which still deletes the worktree with its last card. The ARCHIVED VIEW never shows an empty band. See [Sessions](sessions.md#the-grid). |
| `theme` | string | `"default"` | Appearance | The THEME: `default`, `ocean`, `forest`, `rose`, `amber`, `lavender`, `coral`, `slate`, `sand`, `mono`. An unknown name falls back to `default`. A theme is the accent — the focus chrome: the selection rail, the cursor's frame, the lit tab and rule — and the focus tint. Every status color is fixed and the same in every theme (crimson needs you or crashed, a gold spinner working, gray at rest, purple merged), as are the grays, so a terminal's palette can't move them; the one status that follows the theme is DONE, NOT SEEN, which takes the accent itself — sky blue instead where the accent is warm (`rose`, `amber`, `coral`, `sand`), gray (`mono`) or too near the merged purple (`lavender`). |
| `animations` | bool | `true` | Appearance | Master switch for the motion: the UNREAD SHIMMER (a finish nobody has read, on its row and its project's tab, until it is read), the ONE-SHOT SWEEP (a session that just started needing you or crashed, a just-merged checkout, for about five seconds), the WORKING SPINNER a running session's dot turns into (a still `◐` with this off) and the SPLASH's motion. Off trades them for fewer repaints on a constrained machine. |
| `spotify` | bool | `true` | Appearance | **Spotify in footer**: what the Spotify desktop app is playing, `♪ Midnight City · M83  ⏮ ⏸ ⏭`, in the FOOTER just left of the memory readout — the title in text (dimmed too while paused), the artist dim, and the three glyphs buttons: back a track (or to the start of this one, a few seconds in), play/pause, next. macOS only, asked over AppleScript once a second while a track is up and every five seconds while Spotify is closed or stopped; it never launches Spotify, and nothing is drawn while there is nothing to show. There are no keys: the Mac's media keys already reach Spotify. The first ask raises macOS's AUTOMATION prompt for your terminal; denied, orion stops asking for the rest of the run and says once where to allow it (System Settings → Privacy & Security → Automation). On a narrow bar the artist shortens first, then the title, and then the readout goes. Off stops the polling too. |
| `focus_tint` | bool | `true` | — (retired) | Through 0.34, **Focused panel tint**: whether the FOCUSED PANEL TINT — the faint accent wash behind whatever keys land in, the card under the cursor while the grid has them and the session pane while it does — was painted at all. It always is now: the wash is the one cue that says which surface keys land in (the cursor's card itself now wears a heavy accent frame over the selection fill instead — brighter while the grid has the keys, a shade darker while the pane does — so the grid still names the session the pane reads), so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `show_workspaces` | bool | `true` | — (retired) | Through 0.33, **Workspaces bar** (Settings → Appearance): whether the bar of WORKSPACE tabs was drawn across the top. Workspaces are gone — every project is in the one list the PROJECT TABS open from — so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `black_background` | bool | `true` | Appearance | BLACK BACKGROUND: paint the whole window pure black — grid, cards, session pane, overlays — instead of leaving it on the terminal's own background (a dark gray in a stock Ghostty). Only cells nothing else colored change: selection fills, the FOCUSED PANEL TINT and the colors a session draws itself sit on top as before. Off keeps the terminal's background, transparency or image included. |
| `hide_card_marks` | bool | `false` | Appearance | **Card marks** (`shown` / `hidden`): leave the `▶` (a RUN TERMINAL) and the `❯` (a plain shell) off the front of each terminal card's name on the GRID, and the `›` off the front of each session card's last prompt, so the name or prompt starts where the mark was. The pane's title still shows the terminal glyph. Off keeps them. A config written under the old key, `hide_terminal_glyphs`, still reads. |
| `highlight_current_card` | bool | `true` | Appearance | **Highlight current card** (`on` / `off`): the card under the cursor on the GRID, the one the pane reads, trades its gray fill for a very faint wash of the colour its frame would have unselected (crimson asking or crashed, the done color finished and unread). The wash breathes slowly while the card has something going on; a quiet card or a terminal gets a still, faint accent wash, as every card does with the animations off. It stays lit while you type in the pane and fades further when the PROJECT TABS have the keys. Off keeps the plain gray fill, dimmed whenever the keys leave the grid. |
| `session_pane` | string | `"right"` | Appearance | Where the PANE that reads the card under the cursor sits: `right` (down the right of the cards, full height, half the width until its edge is dragged) or `bottom` (under the GRID, full width). The SIDE BUTTON just before the `×` on the pane's header flips it in one click — `⬓` down the right, `◨` along the bottom — and writes this same key. Its edge facing the cards is dragged the same way on both sides — a `┃` grip beside the cards, a `━` grip under them — and the width and the height are remembered apart, so switching sides never turns one into the other. A window too narrow for the pane and a column of cards side by side lays it out along the bottom until there is room (and draws no side button). Anything off the list — `left` from older builds included — reads as `right`. |
| `worktree_layout` | string | `"cards"` | Appearance | How the GRID lays out each worktree's band: `cards` (a row of cards under the band's rule) or `list` (a compact list — every session and terminal one line under the rule, stacked: its status dot and name, what it runs on, and its last prompt or the shell's last line, with how long since it moved at the right). A band in the list starts collapsed, showing only its 3 most recent sessions — plus the one the cursor is on, wherever it sits — and a `▾ 2 more · Tab: see all 5` line under them; `Tab` (or a click on that line) opens the band to every entry, and `Tab` or `Esc` folds it back. `↑`/`↓` walk the lines as one column across the bands. Anything off the list reads as `cards`. |
| `expand_all_worktrees` | bool | `false` | Appearance | **Expand all worktrees** (`on` / `off`): lay every band on the GRID out open at once — each worktree's sessions and terminals wrapped into rows under its rule (in the `list` layout, every entry listed) — instead of one band opened at a time with `Tab`. With it on there is no accordion: `Tab` and a second click on a band's rule open and fold nothing, and `↑`/`↓` walk down every worktree's rows as one column. The band `Tab` last opened is kept and is open again once this is off. |
| `hide_card_prompt` | bool | `false` | — (retired) | Through 0.40, **Card prompt** (Settings → Appearance, `shown` / `hidden`): `hidden` left the last prompt off every session card on the GRID. Every card shows it now, so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `card_issue_number` | bool | `true` | Appearance | Show the `#15` of the GitHub issue a session was started from (an issue session, launched out of the issues modal) at the right end of its card on the GRID (**Card issue number**, `on` / `off`). The number is a link: a click lands the cursor on the card and opens the issue in the browser, as `⇧I` does. Cards not started from an issue are unchanged. |
| `hide_draft_prs` | bool | `false` | Appearance | Leave draft pull requests out of the grid's PR & ISSUE COUNTS and the jump list's (`⌘K`) pull-request rows, so browsing what's open shows only the rows asking for a reviewer; the PULL REQUESTS MODAL lists drafts either way. A view filter, not a fetch filter: the open-list lookup still fetches the drafts and the PR CACHE still holds them, so `shown` brings them back at once and a draft marked ready joins the rows on the refresh that says so. Sessions and the pull request under their cards are never hidden. |
| `diff_tree_view` | bool | `true` | Review | **Files as a tree**: the DIFF VIEWER (`⌘E`, and a pull request's diff) lists its files as a directory tree rather than flat paths. `⌘B` flips one open viewer; this is how the next one opens. Replaces the tree state the UI-state blob carried through 0.42, which no longer decides it. |
| `diff_start` | string | `"commits"` | Review | **Start on**: which DIFF VIEWER panel has the keys as it opens — `commits` (the COMMIT LIST, so the ticks come first; a viewer with no commit list starts on its files), `files` or `diff`. `Tab` moves on from there. Anything off the list reads as `commits`. |
| `diff_ticked` | string | `"together"` | Review | **Ticked commits**: how two or more ticked commits read as the viewer opens — `together` as one diff, or `one at a time` from the oldest (`⌘G` flips it). |
| `pr_merge_method` | string | `"squash"` | Review | **Merge method**: how the PULL REQUESTS MODAL's merge (`⌘X`) opens — `squash`, `merge` (a merge commit) or `rebase`. A repo that refuses the method opens on the first one it allows. Anything off the list reads as `squash`. |
| `pr_delete_branch` | bool | `true` | Review | **Delete merged branch**: the merge opens with the pull request's branch to be deleted from GitHub once it lands — never a fork's, never the local branch or its worktree, and not after an auto-merge. A repo that deletes merged branches itself says so instead. |
| `pr_draft` | bool | `false` | Review | **New PRs as drafts**: the new pull request form (`⌘N` in the PULL REQUESTS MODAL) opens with its Draft box ticked. |
| `base_fetch` | string | `"5m"` | Review | **Fetch base branch**: how often each project's ROOT WORKTREE fetches origin in the background — `5m`, `15m` or `off` — one project at a time, every project once soon after the TUI starts. The fetch brings every worktree of the project current (they share its remote-tracking refs). When the root sits on the base branch (the `worktree_base_branch` setting, else `origin/HEAD`) and is simply behind origin, it is fast-forwarded too and a FLASH names the commits; a root on another branch, ahead of origin, with uncommitted changes in the way, or with an agent working in it is only fetched. A failed fetch (offline, no origin) is never flashed. A merge from the PULL REQUESTS MODAL (`⌘X`) syncs the project at once, even while this is `off`. Anything off the list reads as `5m`. |
| `pr_autofix` | string | `"off"` | Review | **When a PR breaks**: what orion does when one of *your* open pull requests (`gh`'s signed-in user opened it) hits merge conflicts or failing checks — `off`, `ask` (the AUTOFIX MODAL: the PR, what broke, and a checklist of **Merge conflicts / Unit tests / E2E tests / Other checks** with the detected ones ticked, plus a note; `Enter` sends, `Esc` is not now) or `auto` (sends at once with everything detected, then asks after 3 tries in a row that didn't turn the PR green). Failing checks are acted on once every check has finished (or after 20 minutes); conflicts alone at once. Each breakage — head commit, conflicts, failing checks — is asked about once, across restarts, and never while an `autofix-<n>` session is still at it. Unit vs e2e is read off each check's name. The agent is a PR SESSION in the PR's worktree on the default agent; it fixes what was ticked, runs the tests locally until they pass, and commits and pushes to the PR. Seen while the TUI runs: within about 15s for the selected project, 5 minutes for the others. `⌘G` in the PULL REQUESTS MODAL opens the same form for any PR, whatever this says. Anything off the list reads as `off`. Also on the setup wizard's Autofix page. |
| `autofix_preset` | string | `""` | Review | **Autofix instructions**: empty (`built-in`) tells the agent orion's own instructions for each ticked issue; an AGENT PRESET's name sends that preset's prefix and postfix around the pull request, its branches and what failed instead (its agent, model and effort are not used — the rows below are). A preset deleted since falls back to the built-in instructions, and the footer says so. |
| `autofix_model` | string | `""` | Review | **Autofix model**: the autofix agent's model, from the default agent's list (the Agents tab's **Agent**); empty, or a model that agent no longer offers, is its own default. |
| `autofix_effort` | string | `""` | Review | **Autofix effort**: the same for its effort, fitted to the model. |
| `review_account` | string | `""` | Review | **Review account**: the Claude account My week in review (`⌘⇧Y`) is written through, by its id (`claude`, or an extra account's, as Claude accounts lists them). Empty is your default agent's when that is a Claude account, else the first Claude account that is on. An id that is off or gone reads as empty. The row cycles through the accounts that are on. |
| `review_model` | string | `""` | Review | **Review model**: the model that writes My week in review (`⌘⇧Y`) — `sonnet`, `opus` or `haiku`, the names the `claude` command takes. Empty is `sonnet`: measured on a week of 162 pull requests it wrote the best review in 40 to 50 seconds, where Opus took longer for no better and Haiku was slower and less accurate. Anything off the list reads as `sonnet`. |
| `review_effort` | string | `""` | Review | **Review effort**: `medium`, `low` or `high`. Empty is `medium`; `low` is about a third quicker and drops some of the **Say** lines. |
| `card_line_changes` | bool | `false` | — (retired) | Through 0.37, **Card line counts** (Settings → Appearance): on, each GRID card followed its checkout's changed-file count with the lines behind it. Every card does now — `↳ feat +3 files +120 -45`, the added in the DIFF VIEWER's green and the removed in its red, counted as the DIFF VIEWER shows them (tracked files against HEAD, staged or not, and every line of an untracked file as added; a binary file, or an untracked one over 1 MiB, adds nothing) by one `git diff --numstat` beside each `git status` the file count already runs; on a narrow card the word `files` goes first, then the lines, before the branch gives up a letter — so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `projects` | object | `{}` | Project | PROJECT SETTINGS: one entry per project set up differently from the rest, keyed by the project's repo path exactly as the DAEMON stores it, holding that project's rows from the **Project** tab — `{"projects": {"/Users/me/src/app": {"run_command": "npm run dev", "open_command": "open http://localhost:3000"}}}`. Two rows: **Run command** (`run_command`, string, default `""`) is the RUN COMMAND `⌘⇧S` (**Start stack** in a card's or the project tab's right-click menu) starts in *that project's* worktrees — the same shell line a `.orion.json` `run` would carry, and the way to set one without committing a file; while it is set, **Start stack** runs it and never opens the file, and empty (shown as `.orion.json`) hands the decision back to the checkout's PROJECT FILE, so a project that has one needs nothing here. Typed, not cycled: `Enter` opens a prompt titled with the project, an empty answer puts `.orion.json` back. The DAEMON reads it fresh at each start. **Open command** (`open_command`, string, default `""`) is its twin for the OPEN COMMAND `⌘O` → **Open command** fires on that project's worktrees — `open http://localhost:3000`, say — with the same precedence over the file's `open` and the same prompt; the TUI reads it fresh at each press, since it runs on the machine you are sitting at. The tab edits the selected project and names it on its first line; with no project in the tree its rows read `n/a`. A project with no entry reads as the defaults (an empty command in each row), and an entry that only repeats them is dropped on save, so the map names only the projects that differ; an empty `run_command` or `open_command` is left out of an entry rather than written; a key inside an entry this build doesn't know — the retired `hide_root_worktree` an older build wrote among them — is carried through a save. To the file's rules the map is one key: a value in it this build can't read costs the whole map, not one project. |
| `hide_root_worktree` | bool | `false` | — (retired) | Through 0.27 one switch for every project (**Hide root worktree**, Settings → Experimental), then through 0.35 the fallback for a project whose `projects` entry had no **Hide root worktree** row of its own: on, that project's ROOT WORKTREE was left out of everything the grid launched into. The root is always listed now — a launch that must not land in the shared checkout cuts a fresh worktree instead (**+ new worktree** in the QUICK PROMPT's WORKTREE PICKER, `⌘.`) — so this build never reads the key, here or inside an entry, and no tab edits it; both are still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `hide_projects` | bool | `false` | — (retired) | Through 0.37, **Projects panel** (Settings → Appearance): on, the Projects panel of the old three-panel layout started collapsed to its rail. The GRID has no panels, so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `hide_worktrees` | bool | `false` | — (retired) | Through 0.37, **Worktrees panel**: the same switch for the Worktrees panel. Never read, no row, loaded and written back as stored. |
| `hide_sessions` | bool | `false` | — (retired) | Through 0.37, **Sessions panel**: the same switch for the Sessions panel. Never read, no row, loaded and written back as stored. |
| `recent_prompts` | bool | `false` | — (retired) | Through 0.37, **Recent prompts** (Settings → Experimental): on, a session's last prompts were listed under its row. Every card carries its session's newest prompt now, whatever the key says, so this build never reads it and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `recent_prompts_count` | integer | `3` | — (retired) | Through 0.37, **Recent prompts shown**: how many of those prompts that build listed, `1` to `5`. Never read, no row, loaded and written back as stored. |
| `show_key_combos` | bool | `false` | — (retired) | Through 0.37, **Key combo display** (Settings → Experimental): on, each key pressed on the grid was spelled at the bottom left of the screen with what it did. The KEY COMBO DISPLAY is always on now, whatever the key says — every press shows, `j - Move down`, and clears itself three seconds on ([Keys](keys.md#chips-and-readouts)) — so this build never reads it and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `remember_harness` | bool | `false` | Experimental | REMEMBER HARNESS: a launch walked through the NEW AGENT PICKER (**New agent — choose harness**), the PR SESSION picker or the QUICK PROMPT's `Tab` picker makes its harness the default the next launch starts on — the picker opens on that row and `⌘N` launches it — and a model or effort drilled into through the submenus becomes that harness's own Model / Effort default. It writes the Agents tab's own rows (`quick_prompt_kind`, `<kind>_model`, `<kind>_effort`), so the tab always shows what the next launch will be; a pick that already is the default writes nothing, and an AGENT PRESET launch changes nothing, its harness being the preset's. Off, a pick is one session's: the NEW AGENT PICKER keeps opening on the `quick_prompt_kind` harness and the PR SESSION picker on its first row. See [Sessions](sessions.md#the-new-agent-picker). |
| `pr_issue_counts` | bool | `true` | — (retired) | Through 0.37, **PR & issue counts** (Settings → Experimental): off, the GRID's header dropped the `3 prs · 2 issues` beside the session count and the other projects' issues were never swept. The header always counts now — what is waiting on a repo is read off it without opening `v` or `i`, a click on either count opens that list, a count is left out until its list has landed, and a list cut off at the fetch cap counts `100+` — so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `quick_prompt_kind` | string | `"claude"` | Agents | Which AGENT KIND the QUICK PROMPT (`⌘N`) launches: `claude`, `codex`, `cursor`, `pi`, `muse`, `grok` or `opencode` — or another CLAUDE ACCOUNT's id (`claude-2`), which the **Agent** row steps through right after `claude` and shows by name, `Claude (you@example.com)`. Its model and effort come from that harness's own defaults below, so this is one name, not a third pair. A kind or an account switched off here is stepped around, and an older orion reading an account's id falls back to Claude. The NEW AGENT PICKER opens on it too, and with **Remember harness** on (Experimental) every picker-walked launch rewrites it. |
| `quick_prompt_focus` | bool | `false` | Agents | QUICK PROMPT FOCUS: whether a QUICK PROMPT launch enters and locks the new session's TERMINAL PANE. Off, the keys stay on the grid, and where the cursor goes is `follow_new_session`'s. On, it outranks that key: a launch that enters the new session's pane has to go there. Only the QUICK PROMPT reads it — every other launch takes the pane. |
| `follow_new_session` | bool | `true` | Agents | FOLLOW NEW SESSION (**Follow new**): a QUICK PROMPT launch lands the cursor on the new session's card — the grid scrolls to keep it on screen and the pane shows it, the keys still on the grid — so a run of launches can be watched going up. It selects the card and no more: entering its terminal is `quick_prompt_focus`'s. It covers every QUICK PROMPT: `⌘N`, **New agent — choose harness**'s box, **Duplicate session** on a card, and the boxes the ISSUES MODAL and the PULL REQUESTS MODAL open. Off, the cursor, the pane and the keys stay on the card you were on while the new session's card goes up in its band — first in it, stand-in rows first for a launch that cuts a fresh worktree — and the footer names the branch it went to (`started a session in feat`): the BACKGROUND LAUNCH's stillness, in the project on screen. A launch with no card under the cursor — the aim let go with `Esc`, an empty band, an empty grid — has nothing to keep and lands on its new card either way; so does every launch while `quick_prompt_focus` is on. Terminals (`t`) always come up in the pane. |
| `quick_prompt_new_worktree` | bool | `false` | — (retired) | Through 0.42, **New worktree** under **Quick prompt** (Settings → Agents, and the onboarding wizard's Worktrees page): on, every QUICK PROMPT started aimed at a fresh worktree, and the box's `[ ] new worktree` toggle (`^N`) flipped one launch. Every box starts in the checkout under the grid's cursor now — the project's root branch with nothing selected — and a fresh worktree is the first row of the box's WORKTREE PICKER (`⌘.` / `^T`, **+ new worktree**), so this build never reads the key and no tab edits it; it is still loaded and written back as stored for an older orion sharing the file ([Compatibility rules](#compatibility-rules)). |
| `claude_enabled` | bool | `false` | Agents | HARNESS TOGGLE. Off leaves Claude — the default CLAUDE ACCOUNT — out of the NEW AGENT PICKER and the PR SESSION picker, and skips the standing PREWARM POOL slot; existing sessions keep attaching and resuming. Every harness starts off — a file that never names the key reads as off too — until the ONBOARDING wizard or this tab turns it on, so the picker only offers what you use. Its other accounts keep switches of their own (see [Claude accounts](#claude-accounts)). |
| `codex_enabled` | bool | `false` | Agents | HARNESS TOGGLE for Codex, same rules. |
| `cursor_enabled` | bool | `false` | Agents | HARNESS TOGGLE for Cursor, same rules. |
| `pi_enabled` | bool | `false` | Agents | HARNESS TOGGLE for Pi, same rules. |
| `muse_enabled` | bool | `false` | Agents | HARNESS TOGGLE for Muse, same rules. Muse has no managed hooks yet, so its status stays process-based (running while the PTY is live, no waiting-on-you detection). |
| `opencode_enabled` | bool | `false` | Agents | HARNESS TOGGLE for OpenCode, same rules. |
| `hide_uninstalled_harnesses` | bool | `false` | Agents | When on, the NEW AGENT PICKER lists only enabled harnesses whose CLI is found on PATH. Off by default: a login shell can see CLIs a plain PATH lookup misses, and the daemon re-checks through the login shell at launch anyway. |
| `claude_model` | string | `"default"` | Agents | Default `--model` for new Claude sessions. The literal `"default"` is the sentinel meaning *don't pass the flag, let the CLI pick* — it is what you see in a fresh file, not a missing value. Overlay list: `fable`, `opus`, `sonnet`, `haiku` — unless `claude_models` below or Claude Code's own `availableModels` allowlist replaces it; any other string is passed through verbatim. |
| `claude_models` | array of strings | `[]` | — (hand-edited) | The Claude model rows every picker offers (the NEW AGENT PICKER and QUICK PROMPT submenus, the AGENTS TAB, the PRESET EDITOR) in place of the built-in aliases, verbatim, `"default"` always first: `["claude-sonnet-5", "us.anthropic.claude-opus-5-v1:0"]`. For an organization that restricts models (Claude Code refuses `--model sonnet` with *Model "sonnet" is restricted by your organization's settings. Using claude-sonnet-5 instead.*) or a provider whose ids the aliases don't reach (Bedrock, Vertex, a gateway; on Bedrock `sonnet` even means Sonnet 4.5). Empty, the list follows Claude Code's `availableModels` when one is on disk — `~/.claude/remote-settings.json` (server-managed cache), the macOS MDM profile, `managed-settings.json` and `managed-settings.d/` in the system directory, then `~/.claude/settings.json`, read once at TUI start — else the aliases. A hand edit here applies without a restart. |
| `claude_effort` | string | `"default"` | Agents | Default reasoning effort (`--effort`) for new Claude sessions: `low`, `medium`, `high`, `xhigh`, `max`, or the `"default"` sentinel. |
| `codex_model` | string | `"default"` | Agents | Default `--model` for new Codex sessions (Codex spells the flag the same way Claude does): `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`, or the `"default"` sentinel. |
| `codex_effort` | string | `"default"` | Agents | Default `-c model_reasoning_effort=` for new Codex sessions: `minimal`, `low`, `medium`, `high`, `xhigh`, or the `"default"` sentinel. |
| `cursor_model` | string | `"default"` | Agents | Default model *family* for new Cursor sessions, from the cached catalogue (`cursor_models.json`), or the `"default"` sentinel. |
| `cursor_effort` | string | `"default"` | Agents | The effort suffix the DAEMON joins onto `cursor_model` into one flat `--model <family>-<effort>` id. The choices follow the family, so the overlay row reads `n/a` while the model is unset or has no effort variants. |
| `pi_model` | string | `"default"` | Agents | Default `--model` for new Pi sessions. Pi takes a fuzzy pattern across every provider it has credentials for, so the overlay lists families (`opus`, `sonnet`, `haiku`, `gpt-5.5`); a hand-edited `provider/id` such as `anthropic/claude-sonnet-5` passes through verbatim. |
| `pi_effort` | string | `"default"` | Agents | Default `--thinking` level for new Pi sessions: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, or the `"default"` sentinel. |
| `muse_model` | string | `"default"` | Agents | Default `--model` for new Muse sessions. Any model id passes through verbatim; `"default"` means don't pass the flag. |
| `muse_effort` | string | `"default"` | Agents | Reserved until the `muse` CLI documents a reasoning flag. Stored, never sent. |
| `opencode_model` | string | `"default"` | Agents | Default `--model` for new OpenCode sessions: a `provider/model` id passed verbatim (`opencode models` lists what your machine has credentials for). The overlay lists a few well-known ids (`opencode/big-pickle`, `anthropic/claude-sonnet-5`, …); a hand-edited one passes through. `"default"` means don't pass the flag, so OpenCode opens on its own last-picked model. There is no `opencode_effort`: OpenCode has no effort flag (reasoning is a per-model variant picked inside its TUI), so its Agents section has no Effort row. |
| `custom_harnesses` | array | `[]` | Agents | Extra CLIs the NEW AGENT PICKER offers after the built-ins, each with its own Agents tab section (Enabled and Model rows). Each entry is `{id, program}` plus options: `label` (picker text, defaults to the id), `enabled` (default `true`), `model` (default `"default"` = the CLI's pick, else passed verbatim), `model_flag` (default `"--model"`), and `hooks` (a built-in dialect the program speaks: `claude`, `codex`, `cursor`, `pi` or `opencode` — with one set the sessions report status, prompts and permission waits exactly like that harness, including title sync and auto-title for `claude`; without one they stay process-based, running while the PTY is live and never waiting-on-you). Ids use lowercase letters, digits and hyphens and must not collide with a built-in. Legacy: new harnesses belong in `harnesses`, where they also gain resume, effort, system-prompt and hook-dialect rows. Invalid entries never launch — the picker hides them and the daemon refuses them with the reason. |
| `harnesses` | object | `{"grok": {"enabled": false}}` | Agents | The harness registry: per-harness deltas over the compiled-in known harnesses (Claude, Codex, Cursor, Pi, Muse, Grok Build, OpenCode), and whole new third-party CLIs. The Agents tab grows one section per entry — Enabled, Model, and Effort rows while the harness offers effort — and the NEW AGENT PICKER, AGENT PRESETS, spawn, resume and hooks all read the merged rows. An entry keyed by a `claude_accounts` id is deltas over that account's row (its own model default, say). A hand edit that breaks one entry refuses its launches with the reason, never the whole file. Run `orion config harnesses` to print the effective rows to copy from. Written even when empty, so removing its last entry sticks. |
| `claude_accounts` | array | `[]` | Agents | CLAUDE ACCOUNTS beyond the default one: `[{"id": "claude-2", "config_dir": "~/.claude-2"}]`. Each is a Claude Code config dir with a login of its own, launched as built-in Claude's row — program, flags, hooks, resume — with `CLAUDE_CONFIG_DIR` pointing there, and named after the email it is signed in as, after its `name` when it has one (`"name": "Work"`, written only while set — `Work (you@example.com)`). `enabled: false` switches one off (written only while off). The Agents tab's **Claude accounts** section and first-run onboarding add, rename, sign in, sign out and remove them. Like `harnesses`, never sent over `orion ssh`. See [Claude accounts](#claude-accounts). |
| `keybindings` | object | `{}` | Hotkeys | KEYMAP overrides, keyed by action id, valued with a comma-separated chord list: `{"git_diff": "ctrl+g, g"}`. An empty string deliberately unbinds; unknown ids are ignored. Only rows that differ from the defaults are written. |
| `prewarm_agents` | bool | `true` | Sessions | DAEMON-owned PREWARM POOL: keep one booted agent CLI standing by in the selected WORKTREE, so creating a session there adopts it and feels instant. **Costs one idle CLI process per warm slot** (150–300 MB each, up to 15 minutes), and that spare is a real session as far as the CLI is concerned — Claude's own `/list-agents` lists it beside the sessions you made, named after the directory (`my-repo-3f`), and the memory modal (**Memory usage**) groups it under **warm spares**. Off drains the pool on the DAEMON's next sweep (within 30 s). |
| `usage_claude` | bool | `true` | Sessions | ACCOUNT USAGE (`⇧U`) reads each Claude account's session and weekly limits every 15 minutes, from the endpoint Claude Code's own `/usage` reads, with that account's Claude Code login (on a Mac the Keychain item Claude Code keeps it in, elsewhere `.credentials.json` in its config dir). The token is only read, never refreshed. Off: no Claude rows, and nothing is asked. |
| `usage_cursor` | bool | `true` | Sessions | ACCOUNT USAGE reads the `cursor-agent` login's monthly allowance every 15 minutes, from Cursor's dashboard endpoint. Shown while `cursor-agent` is installed and the Cursor harness is on. On a Mac the first read may ask to let `security` read the `cursor-access-token` Keychain item. |
| `prewarm_sessions` | bool | `true` | Sessions | DAEMON-owned SESSION PREWARM: boot a WORKTREE's dead sessions when your selection rests on it, so attaching shows an already-booted screen instead of a booting shell. **Costs idle shell/CLI processes for sessions you may never open.** Off — for a machine with less memory to spare — landing on a worktree boots nothing: a session forks only when your cursor lands on its row or you attach to it, one at a time; sessions already up stay until the IDLE REAPER takes them. |

### Outside terminal and Ghostty keybinds

Ghostty claims a lot of ⌘ chords for itself and never passes them to the program inside it — `⌘K`
clears the screen, `⌘N` opens a window, `⌘⇧P` is its own palette, `⌘F` searches, `⌘J` scrolls to the
selection. So with `ghostty_keybinds` on (the default), orion keeps a block in Ghostty's config that
releases **every ⌘ chord its keymap answers to** — derived from the keymap itself, your
rebinds in `keybindings` included, so a chord orion uses can never be swallowed by Ghostty. A rebind
in Settings → Hotkeys rewrites the block at once: the new ⌘ chord is released and the one it replaced
goes back to Ghostty. A few chords are never taken whatever the keymap says: `⌘C`, `⌘V`, `⌘Q`, `⌘W`,
`⌘T`, `⌘⇧T`, `⌘Enter` and the tab and window keys (`⌘1`–`⌘9`, `⌘⇧W`, …) stay Ghostty's. `⌘C` is
Ghostty's only while it has a selection of its own (a ⇧-drag): the block binds it
`performable:super+c=copy_to_clipboard:mixed`, so with nothing selected in Ghostty it reaches orion,
where a typed field copies its selection, the editor its own and a session pane its drag selection. The
BUILT-IN EDITOR's chords are released whatever the keymap says ([Keys](keys.md#the-built-in-editor)):
`⌘Z`, `⌘⇧Z`, `⌘D`, `⌘A`, `⌘X`, and the Mac editing chords — `⌘↑`/`⌘↓` and `⇧⌘↑`/`⇧⌘↓`, which
were Ghostty's jump to prompt, `⌥⌘↑`/`⌥⌘↓`, which were its split up and down (`⌥⌘←`/`⌥⌘→` stay its
split left and right), and `⌘⇧L`. `⌘←`/`⌘→` are not taken: Ghostty types `^A`/`^E` for them, which a
shell outside orion still needs and the editor reads as the line's ends. Every typed field takes three
of them too: `⌘A` selects its text, `⌘X` cuts the selection and `⇧⌘↑`/`⇧⌘↓` select to its ends
([Keys](keys.md#typed-fields)).
Its other selection chords need no line — Ghostty binds no `⇧⌘←`/`⇧⌘→` or `⌥⇧←`/`⌥⇧→`, and its
`⇧←`/`⇧→`/`⇧↑`/`⇧↓`, `⇧Home`/`⇧End` binds only act on a selection made in Ghostty itself, so they
reach orion as they are. With the default keymap the block reads:

```
# >>> orion keybinds (managed by orion; edits inside this block are replaced) >>>
keybind = super+k=unbind
keybind = super+e=unbind
keybind = super+r=unbind
keybind = super+l=unbind
keybind = super+n=unbind
keybind = super+/=unbind
keybind = super+y=unbind
keybind = super+shift+/=unbind
keybind = super+.=csi:46;9u
keybind = super+p=unbind
keybind = super+shift+f=unbind
keybind = super+b=unbind
keybind = super+s=unbind
keybind = super+j=unbind
keybind = super+f=unbind
keybind = super+g=unbind
keybind = super+,=unbind
keybind = super+shift+p=unbind
keybind = super+o=unbind
keybind = super+z=unbind
keybind = super+shift+z=unbind
keybind = super+d=unbind
keybind = super+a=unbind
keybind = super+x=unbind
keybind = super+arrow_up=unbind
keybind = super+arrow_down=unbind
keybind = super+shift+arrow_up=unbind
keybind = super+shift+arrow_down=unbind
keybind = super+alt+arrow_up=unbind
keybind = super+alt+arrow_down=unbind
keybind = super+shift+l=unbind
# <<< orion keybinds <<<
```

— one line per ⌘ chord, in the order the actions are listed and the editor's after them, Ghostty's spelling (`⌘?` is the key it is
typed with, `super+shift+/`). A chord macOS keeps for itself never reaches Ghostty to be released at
all: `⇧⌘/` opens every app's Help menu, which is why **Cycle effort** (`cycle_effort`) leads with `⌘Y`
(`^Y` its twin) and keeps `⇧⌘/` only behind it. `⌘.` is the one chord bound rather than released: macOS
reads it as Cancel — the command Escape sends — in every app, so a bare `unbind` let it through as an
Escape and **Select worktree** closed the new-agent box it was pressed in. `csi:46;9u` makes Ghostty
send the KITTY PROTOCOL's own spelling of `⌘.` instead (`ESC [46;9u`: the `.`, with ⌘), and orion reads
an Escape still carrying ⌘ as `⌘.` too, for a terminal without the line. It is written at startup, whenever the Ghostty rows in Settings change
and whenever the keymap does, only on a local Mac and only while Ghostty is in use — orion running
inside it, or `outside_terminal` set to `ghostty` with Ghostty installed. The file is the first
non-empty one of `~/Library/Application Support/com.mitchellh.ghostty/config.ghostty`, `…/config`,
`$XDG_CONFIG_HOME/ghostty/config.ghostty` and `…/config` (`~/.config` when `XDG_CONFIG_HOME` is
unset), else the first. Everything outside the block is yours and is never touched, and a block that
already says the right thing is not rewritten. Ghostty reads its config at launch and on its own
reload (`⌘⇧,`); when orion changes the block from inside Ghostty 1.2 or later it sends that Ghostty
`SIGUSR2`, which reloads it, and otherwise the footer says a reload is needed. `ORION_GHOSTTY_CONFIG` names another file, or
`off` to never write one. Turning the setting off stops orion writing; the block already there stays
until you delete it.

### Installing editors and agent CLIs

orion runs programs it does not ship: the editor behind the BUILT-IN EDITOR, and a CLI per harness.
`install.sh` makes sure of three after orion itself: git, which it stops without; an editor with VS
Code's keys — fresh, unless fresh, micro or Microsoft Edit is already there (`brew install
fresh-editor`; without Homebrew, on Linux, the quick-install script fresh's README links, its
universal build linked into the install dir, and on a Mac — or should that fail — the one micro's
README links, run in the install dir); and gh, which pull
requests and issues are read with (`brew install gh`, else a pointer to its install page); and on a
Mac whose terminal sends no ⌘, Ghostty (`brew install --cask ghostty`, else a pointer to
ghostty.org/download). It says nothing about what is already there, and `--no-deps` (`curl … | sh -s
-- --no-deps`) or `ORION_NO_DEPS=1` skips the step.

A first install on a Mac, from a terminal that sends no ⌘ (anything but Ghostty and kitty), then
opens orion in a new Ghostty window, so setup starts where the ⌘ shortcuts work: orion's keybinds go
into Ghostty's config first, so no reload is needed, and orion runs with the installer's PATH, the
window falling back to your login shell when orion quits. The terminal the installer ran in can be
closed. An update, `orion upgrade`, ssh or tmux, `--no-launch` or `ORION_NO_LAUNCH=1`, or no
Ghostty, skips it.

Inside orion, `i` on a row whose program isn't on PATH — first-run setup's Agents and Editor steps,
every program row on Settings → Tools (**Outside terminal** for Ghostty, **File editor**, **Open in app**, git and gh), a
Claude account's row or a harness's **Enabled** row on the Agents tab — shows the command that
installs it, and `Enter` runs it in the editor modal, the installer's own output on screen (`Ctrl+Q`
stops it), the footer reading `installing …` meanwhile. When it exits the modal stays, its title and
the footer saying whether the program is here now, until `Enter`; the footer and the page or tab
under the modal then keep the result. Ghostty counts as here once its app is in `/Applications` or
`~/Applications`, and installing it from setup — `Enter` on the Ghostty step — makes it the outside
terminal again. The rows read
PATH as they draw. An
installer that puts its program somewhere new and adds that to your shell profile leaves orion's
own PATH as it was: start orion again from a new shell. Nothing is installed unasked, and `orion
doctor` ([Commands](commands.md#checking-the-machine)) names the same commands without running
them.

| Program | Command | Documented at |
|---|---|---|
| fresh | `brew install fresh-editor` | [github.com/sinelaw/fresh](https://github.com/sinelaw/fresh#installation) |
| micro | `brew install micro` | [github.com/zyedidia/micro](https://github.com/zyedidia/micro#installation) |
| Microsoft Edit (`edit`) | `brew install msedit` | [github.com/microsoft/edit](https://github.com/microsoft/edit) |
| nvim | `brew install neovim` | [neovim INSTALL.md](https://github.com/neovim/neovim/blob/master/INSTALL.md) |
| hx | `brew install helix` | [docs.helix-editor.com](https://docs.helix-editor.com/install.html) |
| emacs | `brew install emacs` | [gnu.org/software/emacs](https://www.gnu.org/software/emacs/download.html) |
| Claude (`claude`) | `curl -fsSL https://claude.ai/install.sh \| bash` | [code.claude.com](https://code.claude.com/docs/en/setup) |
| Codex (`codex`) | `brew install --cask codex`, or without Homebrew `npm install -g @openai/codex` | [github.com/openai/codex](https://github.com/openai/codex) |
| Cursor (`cursor-agent`) | `curl https://cursor.com/install -fsS \| bash` | [cursor.com/docs/cli](https://cursor.com/docs/cli/installation) |
| Pi (`pi`) | `curl -fsSL https://pi.dev/install.sh \| sh` | [pi.dev](https://pi.dev) |
| Muse (`muse`) | `curl -fsSL https://dev.meta.ai/install.sh \| bash` | [dev.meta.ai](https://dev.meta.ai) |
| Grok Build (`grok`) | `curl -fsSL https://x.ai/cli/install.sh \| bash` | [docs.x.ai](https://docs.x.ai/build/overview) |
| OpenCode (`opencode`) | `curl -fsSL https://opencode.ai/install \| bash` | [opencode.ai/docs](https://opencode.ai/docs/) |
| Ghostty (macOS) | `brew install --cask ghostty` | [ghostty.org/download](https://ghostty.org/download) |
| git | `brew install git` | [git-scm.com](https://git-scm.com/downloads) |
| gh | `brew install gh` | [cli.github.com](https://cli.github.com) |
| Open in app (macOS) | `brew install --cask cursor` / `visual-studio-code` / `sublime-text` / `zed` | each app's download page |

Without Homebrew an editor's or Ghostty's row names its install page instead and runs nothing;
in first-run setup `Enter` opens that page in the browser. vim has no
installer here: macOS ships it, and it is the last fallback.

### Prewarming

`prewarm_agents` and `prewarm_sessions` are the two settings that cost real processes you never
asked for, so they are worth knowing about on a laptop or a small remote box — and worth knowing
about if your sessions talk to each other. A warm spare is a bare `claude` sitting at its prompt in
the selected worktree: Claude's `/list-agents` shows it as a peer named after the directory, the
same way it names any session you have not titled yet, so a spare beside a fresh untitled session
reads as two copies of one session (`my-repo-3f`, `my-repo-a1`), one of them forever idle. That is
the spare, not a duplicate — **Memory usage** (the command palette) lists it under **warm spares** with its PID. Both rows live on
the SETTINGS OVERLAY's Sessions tab (`Warm spare agent`, `Prewarm dead sessions`); the same keys in
CONFIG.JSON work by hand:

```json
{ "prewarm_agents": false, "prewarm_sessions": false }
```

Turning the pool off takes the standing spares away on the DAEMON's next sweep. `session_idle_timeout`
is what bounds the cost of both when they are left on.

### The harness registry

The seven known harnesses ship compiled in, and `harnesses` edits them per field — or adds a new
CLI outright. Only your deltas go in the file; `orion config harnesses` prints the effective rows
to copy from. Disabling one is one line, and adding a CLI is one block: picker, presets, spawn,
resume, hooks and the Agents tab section all follow, with no rebuild.

```json
{
  "harnesses": {
    "cursor": { "enabled": false },
    "agy": {
      "program": "agy",
      "model_default": "big-1",
      "resume_flag": "--resume",
      "hooks": "claude"
    }
  }
}
```

Nullable rows (`program`, `model_flag`, `permissions_flag`, `hooks`, `env`, …) clear with `null` —
`"claude": {"hooks": null}` runs Claude with process-based status and no title sync. A row that
stops making sense (an empty program, a resume flag plus a resume subcommand, an unknown dialect,
an `env` name a shell can't export) refuses its launches with the reason while every other harness
keeps working. The per-harness keys
the Agents tab edits (`claude_model`, `codex_enabled`, …) keep working as a fallback wherever the
map stays silent. Omit the map entirely and you get every built-in, enabled, with its defaults —
including ones a later orion adds. Like every other object key, a `harnesses` map in
`config.local.json` replaces the whole map from `config.json` rather than merging per harness,
so keep machine-specific overrides in one layer.

`env` gives a harness's CLI environment variables of its own, on top of yours: a map of names to
values, a leading `~/` expanded. They are set on the session's process and exported again after
your login shell's profile has run, so an rc file that exports the same name cannot undo them. A
map replaces the row's whole `env`; `null` clears it. A name a shell can't export, or one of
orion's own `ORION_*` session variables, refuses the harness's launches with the reason.

A second Claude account needs none of this: it is one `claude_accounts` entry
([below](#claude-accounts)), which is built-in Claude's row with `CLAUDE_CONFIG_DIR` set for you. A
hand-written entry whose `env` pins `CLAUDE_CONFIG_DIR` still works and counts as an account — it
is named after its email (`Claude B (b@example.com)` when it has a label), listed and signed in from
the Agents tab, and offered by **Continue on** — but every flag Claude's row carries is yours to
copy, and to keep in step. A `program` that is a wrapper script exporting the variable itself still
launches and resumes as it always has, but orion only knows a harness's config dir through `env`:
such an entry is no account, gets none of a Claude session's transcript checks, and is never a
**Continue on** target.

### Claude accounts

Claude Code keeps everything an account is — its login, its settings, every transcript — in one
config dir: `CLAUDE_CONFIG_DIR`, `~/.claude` when unset. A second subscription is the same `claude`
pointed at another dir, so that is all a CLAUDE ACCOUNT is here:

```json
{
  "claude_accounts": [
    { "id": "claude-2", "config_dir": "~/.claude-2" },
    { "id": "claude-work", "config_dir": "~/.claude-work", "name": "Work" }
  ]
}
```

Built-in Claude is the DEFAULT ACCOUNT, in Claude Code's own default dir (orion's
`$CLAUDE_CONFIG_DIR`, else `~/.claude`); each entry adds one more. An entry becomes a harness that
*is* Claude's row — the program, model and effort flags, `--resume`, the system prompt, the hooks,
the model list, whatever `harnesses.claude` changes — plus `CLAUDE_CONFIG_DIR`, so a later change to
Claude's row reaches every account and nothing is copied by hand. It also takes Claude's model and
effort defaults until its own Model / Effort rows on the Agents tab are changed, which write
`harnesses.<id>`. The `id` is what its sessions point back at, so it stays put; lowercase letters,
digits and hyphens, never a built-in's. `name` is the one it goes by, as typed — `Work Laptop` —
and the only thing a rename changes: never the `id`, never the dir. `enabled: false` switches one off like any harness. The
list is this machine's logins: an export carries it, but `orion ssh` and `orion tunnel` leave it
behind, as they leave `harnesses`, so a remote keeps its own accounts. An entry an older orion reads
is an unknown key it leaves alone: its sessions refuse to start there ("custom harness `claude-2` is
no longer defined") and nothing is lost.

**Named after who they are.** Everywhere an account is listed — the NEW AGENT PICKER, the QUICK
PROMPT's `Tab` list, the Agents tab, onboarding, the preset editor, **Continue on** and the card a
usage limit stops — it reads its name and who it is signed in as: `Work (you@example.com)`,
`Work (not signed in)` before its first login, and `Claude (you@example.com)` while it has no name
of its own. The name is an entry's `name`; for the default account it is `harnesses.claude.label`,
and a hand-written entry's is its `label` (`Claude B (b@example.com)`). Short of room — a card, a
list row, the quick prompt's `harness` field, a preset's line — it is the name alone, else the
email, and only while the machine has more than one account; with one, `claude` says it all. The email is
Claude Code's own record, `oauthAccount.emailAddress` in `.claude.json`: inside the config dir for an
account (as Claude Code keeps it whenever `CLAUDE_CONFIG_DIR` is set, `~/.claude` named that way
included), in the home dir — `~/.claude.json` — for the default account. orion reads that one field
and nothing else from the file, off the TUI's loop: a file's size and mtime are checked every few
seconds and it is parsed again only when they move, and at once after a sign-in from orion.

**The Agents tab** (`s`, then the Agents tab, or **Claude accounts** in the COMMAND PALETTE) lists
every account under **Claude accounts**, the default first: its name, `on` or `off`, its dir, and
`signed in`, `not signed in` or `same as ~/.claude`. On an account's row:

| Key | Action |
|---|---|
| `Enter` | sign it in, or in again: asks for the email to sign in as — it fills Claude's login page; empty leaves the choice to the browser — then runs Claude Code's own `claude auth login [--email …]` with the account's `CLAUDE_CONFIG_DIR`, in the editor modal over the overlay. On a Mac the login page opens in a private window (see below). The browser finishes it; the row says who it is once the modal closes. When it can't — the page shows a code instead — paste the code into the modal and press `Enter`. `Ctrl+Q` closes it early |
| `r` | rename it: a prompt prefilled with its name, `Enter` keeps what is typed and an empty one takes the name away (back to `Claude (you@example.com)`), `Esc` leaves it. Only the name moves — the id its sessions point back at and the dir its login lives in stay. The default account's name is written as `harnesses.claude.label`, a hand-written entry's as its `harnesses` label |
| `o` | sign it out, behind a confirm: `claude auth logout` in the same modal. Its dir keeps its settings and transcripts |
| `←` / `→` | switch it on or off — the same switch as its section's **Enabled** row further down |
| `⌫` | remove an added account, behind a confirm: its entry (and any `harnesses.<id>` deltas, and the `⌘N` default when it named it) leaves config.json. `Enter` keeps its dir on disk — listed from then on under **Saved on this machine**, below — `t` moves it to the Trash; a dir another account also runs in is never moved. The default account, and a hand-written `harnesses` entry, are not orion's to remove: switch the one off, edit the file for the other |

**Add account** asks for a name — `Work` makes `claude-work` in `~/.claude-work`, going by `Work`
as typed; nothing makes the next `claude-2`, `claude-3` … whose dir is not there yet, with no name;
a name whose dir already exists adopts it, login and all, and the question and the notice say so
(`~/.claude-work is already on this machine, signed in as …`) — then, when the default account has
any of it, asks whether to share its setup: `CLAUDE.md`, `settings.json`, `skills`, `agents`, `commands`, `plugins` and
`keybindings.json` are linked from the default dir into the new one (`Enter` / `y`; `n` starts it
empty). Only those, only the ones that exist, and never over anything the new dir already holds —
never its login (`.claude.json`, the credentials), `projects`, history, sessions or caches, which are
what make it another account. Being links, an edit in either account is an edit in both. Then
`Enter` on its row signs it in. First-run onboarding has the same step right after Agents (while
Claude is on there): the same rows, `Enter` on an account to sign it in, `Enter` on **Add account**
for the same two questions, and the same warning.

**Saved on this machine.** Removing an account keeps its dir — its login, settings and transcripts
— and a later **Add account** whose name comes to the same id (`Work` after `work`) takes it back
whole, so it never sits there unseen: under the accounts, **Saved on this machine** lists every
`~/.claude-*` folder that looks like a Claude Code config dir (it holds `.claude.json`,
`.credentials.json`, `projects` or `settings.json`; a real folder, not a symlink) and that no account
runs in, with who it is signed in as — `~/.claude-work [not in orion · signed in as you@work.com]`.
The home dir is scanned with the records, off the TUI's loop. On one of those rows:

| Key | Action |
|---|---|
| `Enter` | add it back: asks for the name it goes by, prefilled from its folder (`~/.claude-work` → `work`; a number is no name), then adds it as it is — under the id its folder makes, or the next free `claude-work-2` — sharing nothing into it, since it has its own setup |
| `⌫` | move it to the Trash, behind a confirm that names the login going with it. Never a dir an account runs in, never `~/.claude` |

First-run onboarding lists only the accounts.

**One login, twice.** claude.ai's browser sign-in approves whichever account the browser is
signed in to, so a second dir signed in from the same browser silently becomes the first account
again — one subscription, one usage limit, and **Continue on** gaining nothing. The Agents tab,
onboarding and **Continue on** flag it: `same as ~/.claude` on the row and a warning under the
accounts (`⚠ ~/.claude and ~/.claude-2 are signed in as one account`), and `· same account` on a
**Continue on** row. To fix it, `Enter` on one and sign it in again as the other account, typing
its email if you like (`claude auth login --email <other>`).

On a Mac, orion sidesteps the trap: its sign-in sets Claude Code's `BROWSER` to a small script
(`claude-sign-in-browser` in orion's data dir) that opens the login page in a private window — the
default browser's when it has one (Chrome, Firefox, Brave, Edge, Vivaldi, Chromium, Opera), else the
first of those installed (Safari has no private window to open from the command line), else the
page opens as before. A private window is signed in to nobody, so claude.ai asks who is signing in,
and its redirect back to `claude auth login` finishes the sign-in with no code to paste. Elsewhere,
sign in from a private browser window yourself, or sign out of claude.ai first.

**From a hand-written entry.** A second account set up as a `harnesses` entry — a wrapper script
in `program` that exports `CLAUDE_CONFIG_DIR`, all of Claude's flags copied beside it — becomes one
line. Keep its id, so its sessions follow it, and delete the entry:

```json
{
  "claude_accounts": [{ "id": "claude-b", "config_dir": "~/.claude-b" }]
}
```

with `"claude-b": {…}` gone from `harnesses` (the wrapper can go too). orion does not rewrite a
hand-written entry for you: **Add account** refuses an id `harnesses` already defines and points
here.

Grok Build launches `grok` and starts off: a config with no `harnesses` map at all — `{}`, or a
file from before the map — reads as `"grok": {"enabled": false}`, and its Enabled toggle is stored
under `harnesses.grok.enabled`. A `harnesses` map that is there is read as written. Set `harnesses.grok.model_default` and `effort_default` in config.json
to pass `--model` and `--reasoning-effort`; unset values use the CLI defaults. The picker offers
only `default` unless you configure `models` and `efforts` lists in that same block.
`--rules` carries additional system guidance and `--resume` accepts a stored session ID, but
automatic session-ID capture and managed hooks are not yet supported. Status is process-based.

### Linear

Settings → **Linear** is every Linear option in one place, and the onboarding wizard's Linear page
draws the same rows:

| Row | Default | What it is |
|---|---|---|
| **Link PRs to Linear** | on | `linear_auto_attach` — the pull request a `⌘L` launch opens is attached to its issues through the API, as a closing link, so Linear's pull request automations move the issue when it opens and merges. Linear keeps one attachment per pull request, so it never duplicates, and the link does not depend on the branch name carrying an issue ID. An issue sent to another worktree leaves the pull request orion had attached it to |
| **Linear account** | the key's owner | `linear_assignee_email` — whose issues `⌘L` lists; `Enter` types an email |
| **Task template** | default | `linear_task_template` — the task a `⌘L` launch starts with; `Enter` edits it in a multi-row box |
| **API key** | — | read-only: where the selected project's `LINEAR_API_KEY` was found — `found in .env.local · demo`, `found in .env · demo`, `found in orion's environment · demo`, or `not found for demo`. The lookup is the one every Linear call makes: the checkout's `.env.local`, then its `.env`, then orion's own environment. The value is never shown, logged or stored |
| **Test connection** | not tested | `Enter` sends Linear's `viewer` query with that key and the row reads whose it is — `✓ Jane Doe · jane@acme.dev` — or why not — `✗ Authentication required`; the explanation line under the rows says it in full. A key that moves or changes since reads `not tested` again |

Only `LINEAR_API_KEY` is read from an env file. It goes to `api.linear.app` alone, on `curl`'s
standard input (`--config -`), never on a command line.

### What `session_idle_timeout` accepts

The overlay cycles `off`, `1m`, `5m`, `15m`, `30m`, `1h`, but the DAEMON parses more than that: any
`<n>s` / `<n>m` / `<n>h` works, and **`"off"` (or `"0"`) disables reaping entirely**. A malformed
value falls back to the 5m default, *not* to off — a typo makes reaping ordinary, not absent.

The IDLE REAPER sweeps every 15s and only takes sessions in WORKTREES no client is viewing. RUNNING
and NEEDS FEEDBACK agents, agents whose backgrounded tool call is still running (a Claude
`run_in_background` Bash call or Monitor watch, a Codex shell command — the idle clock restarts when
that job ends), and terminals with a command running, are spared. A reaped session revives on the
next ATTACH or prewarm, and an agent RESUMES its conversation there.

## What the settings overlay owns

- **Settings live in one JSON file** (`config.json`, beside the database, with `config.local.json` over
  it), read fresh on each use by both the daemon and the TUI, so hand edits apply without a restart. `s` opens the settings overlay over the
  same file: color theme, animations, which side of the cards the session pane sits on
  (`session_pane`), whether each worktree is a row of cards or a compact list (`worktree_layout`), whether every worktree is open at once (`expand_all_worktrees`),
  editor, the branch new worktrees start from (`worktree_base_branch`: `auto` for origin's default
  branch, or a name such as `master`, typed into a prompt that `Enter` opens on the row), what
  deleting a worktree does to its docker compose projects (`worktree_containers`), which
  agent CLIs the new-session menu offers (at least one stays on) and their default model
  and reasoning effort, the selected project's own settings on the **Project** tab (`projects`,
  keyed by repo path — its run and open commands), the idle timeout, whether a warm spare and a worktree's dead sessions are
  pre-booted (`prewarm_agents`, `prewarm_sessions`), the done sound (`done_sound`: a ding
  when a turn you haven't seen finishes, 3 s after it settles, once until you look at the session,
  and never within 10 s of another sound — a macOS system sound such as `Glass`, the default; `bell`
  for the terminal bell, which Ghostty keeps silent unless its `bell-features` include `audio`; or
  `off`. Over `orion ssh` and off macOS it is always the bell. While the terminal window is in the
  background the finish is also named in a desktop notification; `off` silences both), the feedback
  sound (`feedback_sound`: the same choices, `Sosumi` by default, rung when a turn stops to ask you
  or crashes mid-turn — and, while the terminal window is in the background, a desktop notification
  naming the session and its worktree; `off` silences both). Stepping either sound row with ←/→
  plays the sound it lands on, so you can pick by ear. And which side of the task a new AGENT PRESET's text goes (`preset_text`: `prefix`, the
  default, `postfix` or `prefix & postfix`). `R` inside the overlay puts every setting —
  hotkeys included — back to its default, after a confirmation, and removes `config.local.json`.
  A row that shipped in the last seven days reads `(new)` before its label.
- **Every grid key is rebindable.** The overlay's Hotkeys tab lists every action and what it answers to,
  and writes overrides into the same file (`"keybindings": {"git_diff": "ctrl+g, g"}`); an empty value
  unbinds. Because orion is always a guest inside Terminal.app / Ghostty / tmux, the tab says at bind
  time when a chord probably won't survive the trip — `⌘` anything, `^⇧` without the kitty protocol,
  `^←` on stock macOS. `Ctrl+q` is the one exception to all of it: it unlocks a terminal no matter what
  you bind, since unbinding your way out would trap you in the session.

## Backup, restore and other machines

```sh
orion config path                  # where each settings file is
orion config export ~/backups      # write ~/backups/orion-settings.json
orion config import ~/backups      # merge it back in, here or on a new machine
```

A SETTINGS BUNDLE is one JSON file holding `config.json`, `agent_presets.json` and `ssh_hosts.json`,
each exactly as the file has it, under a marker key:

```json
{
  "orion_bundle": 1,
  "exported_by": "0.27.0",
  "config": { "theme": "ocean", "keybindings": { "git_diff": "ctrl+g" } },
  "agent_presets": [{ "name": "reviewer", "kind": "claude", "prefix": "Be strict." }],
  "ssh_hosts": [{ "host": "me@box", "last_used_ms": 1789400000000 }]
}
```

- **Export** writes to stdout, to a file, or to `orion-settings.json` inside a folder you name. It
  never carries `config.local.json`, the database, the logs or the PR and model caches.
- **Import** takes an export, a bare `config.json`, `agent_presets.json` or `ssh_hosts.json`, a folder
  holding any of them (a dotfiles folder, a copy of an old data dir), or `-` for stdin. It merges
  rather than replaces: keys the file sets replace this machine's and keys it lacks are left alone,
  presets merge by name, and hosts merge by destination, keeping the most recent. `config.local.json`
  is never written, and the summary names any imported key it still overrides. A settings file that
  exists but can't be read fails the import before anything is written. Changes apply without a
  restart.
- **Copying the file works too.** Both halves re-read `config.json` on every use, so a backup copied
  over it — or to wherever `ORION_CONFIG_FILE` points — applies at once, with no import and no
  restart.
- **Over ssh** (`ssh_sync_config`, on by default), `orion ssh` and `orion tunnel` export
  `config.json` and the presets into the one ssh command they already run, and the remote orion
  imports them before it starts, saying what changed on stderr. Sync is one way: the machine you
  connect from wins key by key on each connect, so a setting the remote should keep goes in the
  remote's `config.local.json`. A remote orion too old to know about the bundle ignores it and starts
  as before. The bundle travels base64-encoded in the remote command, so that machine's process list
  shows it while ssh starts. Settings hold no credentials, but a preset's prefix text travels too.
  A bundle over 64 KiB is not sent (a real one is a few KiB), and the connection goes ahead without
  it.

### Compatibility rules

Two orion versions share these files all the time: a remote a few releases behind the machine that
connects to it, a backup restored into a newer build. Every setting follows these rules so neither
version breaks the other:

1. **Keys are only ever added.** A released key keeps its name, its type and its meaning. A new
   meaning gets a new key, and orion keeps reading and writing the old one for older builds. A
   key renamed with its row (`hide_terminal_glyphs` became `hide_card_marks` in 0.40) is read under
   both names and written back under the new one alone, so a build older than the rename sees that
   one setting at its default.
2. **Missing means default; unknown is kept.** A key a file lacks is its default. A key a build
   doesn't know is ignored when read and written back untouched on save; the overlay's `R` reset is
   the one thing that drops it.
3. **One bad value costs one key.** A value a build can't read takes its default in that build, is
   logged, and stays as stored unless that setting is changed on that machine. Preset and host
   entries work the same way: one this build can't read, such as a preset for a harness a newer
   orion added, is kept through a save rather than deleted.
4. **Choices fall back when used, not when read.** Values like `theme` and `quick_prompt_kind` stay
   plain strings, and a name a build doesn't know falls back to the default where it is used.
5. **Old files are tested.** `crates/orion-core/fixtures/config-<version>.json` holds every key a
   release wrote, each set to something other than its default, and the test suite checks that every
   one still loads as written and survives a save. Each release adds its own fixture.

`config.json` carries no version number. An older build could only refuse a newer file or ignore the
number, and the keys a file holds already tell a newer build everything it needs. The bundle's
`orion_bundle` marks the file and never gates it: sections are only ever added, and a reader leaves
one it doesn't know alone.

## The project file (`.orion.json`)

The PROJECT FILE tells orion how a repository is run. Commit it at the repo root, beside
`package.json` or `Cargo.toml`, and every checkout of the project carries it:

```json
{
  "run": "npm run dev",
  "open": "open http://localhost:3000"
}
```

Both keys are optional shell command lines, and both run in the selected worktree's checkout:

- **`run`** — the RUN COMMAND: how a worktree's stack starts. `⌘⇧S` on the grid, or **Start stack** in
  a card's right-click menu (or the project tab's), starts it and moves the pane onto it; the same key,
  or the row now **Stop stack**, stops it. The DAEMON runs it in a RUN TERMINAL: your login shell
  (`$SHELL -l -i -c`, the wrapper an agent launch uses, so `npm`, `bun` or `mise` resolve the way they
  do typed) running that line and nothing else. Its process is the worktree's RUNNING state — it is a
  terminal card in the worktree's band, its `▶` lit while the process lives, showing the run's last
  lines like any terminal card. Like every session it outlives
  the TUI and every client sees it, and the idle reaper never takes it. Stopping sends the run a `^C`,
  as you would at a terminal, so its own trap winds down what it started — a compose stack, a dev
  server — in the pane; a second stop, or two minutes without it exiting, kills the whole process tree
  and removes the row. A command that exits on its own, or on its `^C`, leaves its row behind, dimmed;
  attaching it replays how the run ended, and nothing but **Start stack** runs it again — not an
  attach, not the prewarm sweep. There is one run per worktree: a second client's start on a worktree
  already running never starts another. With no run command anywhere, a worktree whose checkout has a
  compose stack runs `docker compose start` on it in the same terminal instead.
- **`open`** — the OPEN COMMAND. `⌘O` → **Open command** on a worktree fires it once, from the TUI
  rather than the DAEMON, since what it opens — a browser tab, an editor — belongs on the machine you
  are sitting at. It runs through `$SHELL -c` with its output discarded, and orion does not wait for
  it. It answers from any card: the worktree is the one whose band the cursor is on. A card's
  right-click menu has **Open** too, and **Open checkout in editor** can be given a key of its own in
  Settings → Hotkeys.

The file is read fresh at every press, so an edit applies on the next **Start stack** or **Open command**. Orion looks
in the worktree's own checkout first, so a branch can carry commands of its own, and falls back to the
project's main checkout, so a worktree cut before the file was committed still runs. The first file found
is the whole answer: a worktree's file with no `open` does not borrow the main checkout's. A missing
file, a missing key, or JSON that doesn't parse is a one-line footer message saying what to fix; unknown
keys are ignored.

Both commands have a second home: **Run command** and **Open command** on the SETTINGS OVERLAY's
Project tab (`s`), which keep them in `config.json` under the project's `projects` entry instead of in
the repository — for a project you would rather not commit a file to, or one whose commands are yours
alone. Set, the row is what **Start stack** runs, or `Shift+Enter` opens, in every worktree of that project, and the
file is not consulted for that command; empty, the file decides as above. They are one project's
settings, so each project can run and open its own way, and the footer names both places when neither
has the command. See `projects` in [Every setting](#every-setting).

Unlike the WORKTREE HOOKS below, which orion runs on its own and so never takes from a checkout,
nothing in the PROJECT FILE runs until you press its key on that worktree — the same trust as typing the
command into a shell there.

## Worktree containers

A checkout that runs `docker compose up` — a database, a cache, an object store per worktree —
leaves that compose project behind when orion deletes the checkout: running, or stopped but holding
its volumes. **Worktree containers** (Settings → General, `worktree_containers`) has the DAEMON deal
with them as part of the delete:

| Value | What a deleted worktree's compose projects get |
|---|---|
| `off` (default) | Nothing; they stay as they are. |
| `stop` | `docker compose -p <project> stop`: nothing left running, everything kept. |
| `remove` | `docker compose -p <project> down --remove-orphans`: containers and networks gone, named volumes kept. |
| `remove+volumes` | The same with `--volumes`: the project's data goes too. |

How a project is matched, and when it is left alone:

- **By where it was started, never by name.** Compose stamps every container with the directory
  `up` ran from (`com.docker.compose.project.working_dir`). A project goes when every one of its
  containers was started in the deleted checkout or a directory under it — a nested checkout's
  included. A project with containers started elsewhere too (two checkouts sharing a project name)
  is left alone, with a warning naming it, since tearing it down would take the other checkout's
  data with it. No compose file is needed: compose finds what to stop by its labels.
- **Any docker engine.** It drives the `docker` CLI, so OrbStack, Docker Desktop and Colima all
  work. The DAEMON looks for `docker` on its own `PATH`, then in `~/.orbstack/bin`, `/usr/local/bin`,
  `/opt/homebrew/bin` and Docker Desktop's app bundle, since launchd's `PATH` names none of them. A
  machine with no docker CLI has nothing to clean up.
- **Only after a delete that succeeded, and before the delete hook.** It runs when the WORKTREE
  HOOKS below would — after `git worktree remove` and the row drop, under the same lock, skipped
  while the directory is still on disk — and ahead of `orion.worktreeDeleteHook`, so a hook that
  releases ports finds them already free. Worktrees removed outside orion run nothing.
- **It only reports.** Docker not answering, a compose command failing, or one running past 60 s
  (`docker ps` gets 15 s) is a one-line warning in every client; the worktree stays deleted. What it
  stopped or removed is logged in `daemon.log`.

While a checkout is still there, its stack's state is on the grid: the DAEMON asks `docker ps` every
5 seconds (`ORION_STACK_POLL_MS` changes that; it waits 30 seconds after docker stops answering) and
every client draws a `⬡` at the right end of the band of a worktree with a stack — green while the
stack runs, faint while it is stopped. `⇧S` opens STACKS, every compose stack on the machine with the
worktree it belongs to, to start, stop or take one down ([Keys](keys.md)).

`orion doctor` lists the compose projects whose every container was started in a directory that is
gone — a checkout deleted while this was `off`, or deleted outside orion — with the
`docker compose -p <project> down --volumes` that removes each ([Commands](commands.md#checking-the-machine)).

## Worktree hooks

A checkout often owns things outside its own directory — a dev-server port, a Caddy or nginx route, a
docker compose project, a database — that orion knows nothing about. WORKTREE HOOKS are two
executables of yours the DAEMON runs after it creates or deletes a worktree, so a project can provision
and release those itself. They are the one setting that is not in CONFIG.JSON: a hook is per
repository, so it lives in git config, read fresh at each use:

```sh
git config orion.worktreeCreateHook /absolute/path/to/worktree-setup
git config orion.worktreeDeleteHook /absolute/path/to/worktree-cleanup
```

Git resolves the key the usual way, so `git config --global` sets one script for every project and a
repo's own `.git/config` overrides it. Orion never reads a hook from a file inside the checkout — a
committed hook would run whatever a clone brought with it, which is why git refuses working-tree hooks
too. Keep the executable outside the worktrees it serves; the delete hook runs after its checkout is
gone.

What a hook gets:

- **Two arguments**, both absolute: the main repository path, then the created or deleted worktree
  path. The value is spawned directly as an executable — no shell — so a space in either path arrives
  intact.
- **The main checkout as its working directory**, since the deleted directory no longer exists.
- **Environment**: `ORION_HOOK` (`worktree-create` or `worktree-delete`, so one script can serve
  both keys), `ORION_WORKTREE_BRANCH` and `ORION_WORKTREE_ID`.

When they run, and what a failure means:

- **Only after a orion operation that succeeded.** The create hook fires once the checkout exists and
  its row is in every client — `orion worktree`, the QUICK PROMPT's fresh
  worktree, a PR SESSION's checkout. The delete hook fires once `git worktree remove` (forced or not)
  and the row drop went through, including a checkout you had already `rm -rf`'d by hand. A delete
  that fails or is cancelled runs nothing. Worktrees created or removed outside orion, which WORKTREE
  SYNC merely notices, run nothing either.
- **Skipped while the directory is still there.** When git had already stopped tracking a checkout
  and orion leaves the untracked directory alone, the delete hook does not run against live files;
  the warning says so.
- **Hooks never overlap.** They run under the DAEMON's worktree lock, so a create of a path waits
  for the delete hook still releasing it, and **Delete all sessions**' batch runs its hooks one after another. A
  stuck hook holds the next worktree operation for at most the timeout; keystrokes never wait on it.
- **A hook only reports.** It exits non-zero, cannot start, or runs past the timeout (30 s; then it
  and every process it started are killed) — orion shows a one-line warning naming the hook and the
  last line it wrote to stderr, and logs the tail of its output in `daemon.log`. The worktree stays
  created or deleted, because it already was. Nothing is retried.
- **It may start something that outlives it.** A hook that launches a dev server in the background
  and exits 0 is a success the moment it exits — its output goes to a file, not a pipe, so a child
  holding it open never stalls the wait — and the server is left running. Only a timeout takes down
  what the hook started.
- **The DAEMON's environment is not a login shell.** On macOS a launchd-started daemon has a thin
  `PATH`; a script that calls `caddy` or `docker` sets its own.

A cleanup script in the spirit of the request that introduced this — release a routing entry and a
development slot keyed by the deleted path:

```sh
#!/bin/sh
# $1 = main repo, $2 = deleted worktree
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
name=$(basename "$2")
[ -e "$HOME/.config/dev-slots/$name" ] || exit 0
rm -f "$HOME/.config/dev-slots/$name" "/etc/caddy/sites/$name.caddy"
caddy reload --config /etc/caddy/Caddyfile
```

## Logs

`daemon.log` and `tui.log` live in the state dir, which is not the DATA DIR on Linux and *is* on
macOS — the `directories` crate has no state dir there, so orion falls back to `<DATA DIR>/state`:

- **macOS**: `~/Library/Application Support/dev.orion.orion/state/`
- **Linux**: `~/.local/state/orion/`
- With `ORION_DATA_DIR` set, always `$ORION_DATA_DIR/state/`, so a test or a parallel instance keeps
  its logs beside its own data.

`ORION_LOG=debug` for more. No `daemon.log` at all means the DAEMON never started.

Neither grows without end: a log past 5 MB when its process starts is set aside as `daemon.log.1`
(or `tui.log.1`), replacing the one set aside before, and a new one begun. A DAEMON that stays up
keeps writing the file it opened, so the cap is checked at each start, not while it runs.

## What orion keeps, and for how long

Everything orion fetches is cut at the ask, and everything it stores is pruned to what is still on
screen, so a year of use reads and writes what a week does:

| What | Kept |
|---|---|
| Open pull requests | the 100 newest per project |
| Merged pull requests | every one merged in the last 7 days, read a page of 100 at a time until the week is covered |
| Linear issues | open ones — 100 of yours, 250 of your teams' — and the ones done in the last 7 days, 50 of each |
| GitHub issues | the 100 newest open per project |
| Pull request pages and diffs (`pr-cache/details/`, `pr-cache/diffs/`) | one file each, while the pull request is on a row; written only when read again |
| Linear branch links (`linear-links.json`) | until 90 days after they were last linked, once no checkout is on the branch and no open pull request is from it |
| Week in review (`reviews/`) | the last 12 per project, each with what it was written from; a second one for the same week and scope takes the first's place |
| Pull request read marks (the DAEMON's `pr_seen`) | 180 days after you last read the pull request; pruned when the DAEMON starts |
| Files dropped or pasted into a prompt (`attachments/`) | 7 days |
| Logs | 5 MB, and the 5 MB before it |

## Environment variables

Knobs worth reaching for by hand:

| Var | Default | What it does |
|---|---|---|
| `ORION_LOG` | — | `RUST_LOG`-style tracing filter for both the DAEMON and the TUI. |
| `ORION_EDITOR` | — | Editor command the file modals open, ahead of the `editor` setting. |
| `ORION_GHOSTTY_CONFIG` | Ghostty's own config file | The Ghostty config file orion keeps its GHOSTTY KEYBINDS block in, or `off` to never write one. |
| `ORION_CONFIG_FILE` | `<DATA DIR>/config.json` | Moves `config.json` alone — into a dotfiles checkout, say — leaving the database, logs and `config.local.json` in the DATA DIR. Give an absolute or `~/` path. The DAEMON reads it from its own environment, so after changing it run `orion kill` and relaunch. |

Overrides for tests and parallel instances — real, but not things a normal install needs:

| Var | Default | What it does |
|---|---|---|
| `ORION_RUNTIME_DIR` | `$XDG_RUNTIME_DIR/orion`, else `/tmp/orion-<uid>` | The RUNTIME DIR holding the DAEMON SOCKET and pidfile. |
| `ORION_DATA_DIR` | the platform app-support dir | The DATA DIR holding the database, config and logs. |
| `ORION_AGENT_CMD` | — | Replaces every agent CLI with one command line, taken verbatim (tests stand in `/bin/sh` or a stub script). |
| `ORION_INSTALL_URL` | the published install script | The URL `orion upgrade` / `orion ssh` fetch. |
| `ORION_USAGE` | unset | `off` stops ACCOUNT USAGE asking any provider, background reads included — what the e2e tests set so a run never reads the machine's logins. |
| `ORION_UPDATE_CHECK_SECS` | `3600` | How often the TUI asks GitHub whether a newer release is published, for the FOOTER's `⇡ vX.Y.Z` indicator (one `curl` to the release page's redirect, no `gh` token); `0` turns it off. See [Keys](keys.md#chips-and-readouts). |
| `ORION_SPOTIFY_POLL_SECS` | `1` | How often the TUI asks Spotify what it is playing while a track is up, for the FOOTER's Spotify readout (one `osascript`; every five seconds while Spotify is closed or stopped); `0` turns it off, as the e2e tests do. macOS only. See the `spotify` setting. |
| `ORION_IDLE_REAP_MS` | `15000` | IDLE REAPER sweep period in ms. This is how often it looks, not how long a session may idle — that is `session_idle_timeout`. |
| `ORION_WORKTREE_SYNC_MS` | `2000` | WORKTREE SYNC probe period in ms: how often the DAEMON reconciles `git worktree list` so worktrees made outside orion appear. |
| `ORION_HOOK_TIMEOUT_MS` | `30000` | How long a WORKTREE HOOK may run before the DAEMON kills it and warns; tests shorten it. |

`ORION_AGENT_ID`, `ORION_API_URL` and `ORION_API_TOKEN` are set *by* the DAEMON on every agent
PTY (and scrubbed from plain terminals) so hooks can reach the HOOK RECEIVER — never something you
set yourself. `ORION_IMPORT_BUNDLE` is likewise set *by* `orion ssh` and `orion tunnel` in the
remote command: the SETTINGS BUNDLE the remote orion merges at startup and removes from its
environment before it starts anything. For all of these, empty and unset mean the same thing: use the default.
