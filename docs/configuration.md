# Configuration

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

CONFIG.JSON is the settings file, in the DATA DIR beside the SQLITE STORE — hand-editable, and
what the `s` SETTINGS OVERLAY writes:

- `~/Library/Application Support/dev.orion.orion/config.json`
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
`session_idle_timeout` and `prewarm_agents`, reads
`harnesses` and `claude_accounts` beside the TUI (spawn and resume go through
them), and reads three keys out of each `projects` entry: `base_branch`, `run_command` and
`worktree_containers`. Each side
deserializes only its own fields and ignores the rest, and both load
them fresh on every use — so a hand edit applies without restarting either. No key is required: a
missing file is all defaults, an unknown field is skipped, a value this build can't read costs only
that key (it takes its default and stays as stored in the file), and a malformed file is logged and
ignored rather than failing the operation that read it. The overlay patches only the keys it knows
and leaves everything else in the JSON untouched, so hand-written fields survive a save — the
[compatibility rule](#compatibility-rules) that lets two orion versions share the file.

Files beside it in the DATA DIR: `orion.db` (the SQLITE STORE), `config.local.json` (above),
`agent_presets.json` (AGENT PRESETS), `cursor_models.json` (the cached `cursor-agent --list-models` answer, refreshed after 24h),
`ssh_hosts.json` (the SSH HOSTS FILE), `reviewed.json` (REVIEWED MARKS). All but the database are
convenience stores: missing or malformed reads as empty.

## Every setting

Thirty-four keys. **Overlay** is the SETTINGS OVERLAY tab whose row edits the key; `—` means the key
exists only in the file, so it is hand-edit-only. The overlay has five tabs: **General ·
Accounts · Projects · Presets · Keys**. Most rows toggle or cycle on `Enter` / `←` / `→`; a
*typed* row (the Projects tab's **Base branch**, **Run command** and **Open command**) opens a
one-line prompt on `Enter` instead, pre-filled with the stored value, and an empty answer puts its
default back. **General** groups its rows under **Launching**, **Sounds** and **Look**.
**Accounts** opens on **New agents on** (which account or harness `⌘N` starts on), then **Claude
accounts** (see [Claude accounts](#claude-accounts)), then one header per harness — **Claude
(you@example.com)**, **Codex**, **Cursor**, **Grok Build**, **OpenCode** — so a harness's rows read
`Enabled` / `Model` / `Effort` under its name rather than repeating it. The **Projects** tab is the
one tab whose rows are not orion's but each project's: every project in turn, named with its path,
and each row under it reads and writes that project's own entry under `projects` — so the same row
shows a different value on the next project over; its **Linear key** row is where that project's
key was found ([Linear](#linear)). **Presets**' one row opens the AGENT PRESETS list, and **Keys**
lists every action and its chords, read-only (rebinding is `keybindings`, below).

Some things are no longer settings at all, because they are how orion works: `.env` files are
linked into every new worktree, a file opened from Go to file or find in files closes the finder,
dead sessions boot when the selection rests on their worktree, Claude and Cursor usage are read,
every checkout has a band on the grid, the window is painted black, a pull request is linked to the
Linear ticket its branch was cut for, `⌘K` Enter selects a row rather than attaching it, the DIFF
VIEWER opens on its commits, and each project's base branch is fetched every five minutes. A merge
form opens on the method the project last merged by with its branch to be deleted, and a new pull
request form opens with Draft off. Those keys, if a file still holds them, are left as they are and
read as nothing.

| Key | Type | Default | Overlay | What it does |
|---|---|---|---|---|
| `outside_editor` | string | `"auto"` | General | **Open files in**: the GUI editor `⌘O` hands a file to — from Go to file, find in files, the TREE BROWSER, a MARKDOWN PAGE and the BUILT-IN EDITOR — and the OPEN MENU's **Checkout in …** row opens the checkout in: `cursor`, `vscode` or `sublime`. `auto` is the first of Cursor, VS Code and Sublime Text installed, else whatever macOS's `open` hands the file to; the row shows which (`auto · Cursor`) and its hint lists the ones installed. A stored `zed` or `default` reads as `auto`. With a named app missing, `i` on the row installs it — `brew install --cask cursor`, `visual-studio-code` or `sublime-text` — in the install modal. An app is launched through the command-line tool inside its own bundle in `/Applications` or `~/Applications` (`Cursor.app/Contents/Resources/app/bin/cursor`, `Visual Studio Code.app/…/bin/code`, `Sublime Text.app/Contents/SharedSupport/bin/subl`), then the one on `PATH` — never Cursor's agent CLI shim, the `~/.local/bin/cursor` the `cursor-agent` installer writes, which only forwards to another `cursor` and otherwise fails — then `open -a`. A tool goes to the file's line (`cursor`/`code` with `--goto <file>:<line>` in the checkout's window, `subl` with `<file>:<line>`); `open -a` and `open` open the file at its top. The hints name the app (`⌘O: VS Code`). A named app that isn't installed, or a tool that fails, says why in the footer instead; over ssh the file opens in the BUILT-IN EDITOR. |
| `onboarded` | bool | `false` | — (config.local.json) | Whether **Orion setup** has been seen. While it is false, orion opens it over the grid at startup: four pages under a STEP STRIP — Welcome; **Claude**, its CLI `installed` or `install…` (`i` installs it, [Installing editors and agent CLIs](#installing-editors-and-agent-clis)) and every Claude account, each signed in (which switches it on) or added; **First project**, the repo orion was started in opened as a project, its **Base branch** and **Run command**; and **Ready**, the keys that matter. `Esc` or a click outside skips it; either way the key is set and it does not come back. It lives in `config.local.json`, so a remote reached over `orion ssh` asks on its own first run. Delete the key, or **Run setup** in the palette, or `orion setup`, to see it again. |
| `seen_version` | string | `""` | — (config.local.json) | The orion version this machine last saw **What's new** for. On the first launch of a newer build — after `orion upgrade` or the footer's update — the **What's new in Orion** sheet is up over the grid: every release since this one, stacked newest first however many the upgrade jumped, each with its date and what its commits said. Closing it — or setup — stamps this build's version; it is never lowered, so an older or dev build in between shows nothing twice. A first run stamps it without showing notes; a config from before the key shows the newest releases. The notes are baked in at build time from git — each release is the commits between its `v*` tag and the one before (`crates/orion-tui/build.rs`) — so a build without git or tags has none. |
| `linear_assignee_email` | string | `""` | — (config.local.json) | Whose open issues `⌘L` lists. Empty is the owner of the project's `LINEAR_API_KEY`; an email names another member of the workspace, for a key a team shares. Set by hand; a save keeps it in `config.local.json`, so `orion ssh` never carries it to a remote. |
| `session_idle_timeout` | string | `"5m"` | General | DAEMON-owned IDLE TIMEOUT: how long a session in a WORKTREE no client is viewing goes unwatched before the IDLE REAPER kills its PTY. See the values below. |
| `done_sound` | string | `"Glass"` | General | The DONE SOUND rung when a turn you haven't seen reaches FINISHED: `off`, `bell` (the terminal BEL — silent in Ghostty unless its `bell-features` include `audio`), or a macOS system sound from `/System/Library/Sounds` played with `afplay` (`Glass`, `Ping`, `Pop`, `Hero`, …). Over `orion ssh` and off macOS it is always the bell. Stepping the row in the settings overlay plays each sound. It rings only for news: a turn that finishes in the pane on screen was watched and rings nothing. It waits 3 s for the finish to settle, and stays quiet if by then the session is working again, has been looked at, or was archived or deleted. It rings once per unseen stretch — the follow-up turns a session starts on its own (a task notification, a queued message) ring nothing more until you have looked at it, and then its next finish rings again. And it never rings within 10 s of the last sound of either kind, so a burst of finishes is one ding. The one switch for that finish's DESKTOP NOTIFICATION too: while the terminal window is in the background, each finish that settles is named in one (`Fix Login finished`, under its project and branch) — even when the 10 s rule kept the sound quiet — on the same terms as `feedback_sound`'s. `off` silences the sound and the notification together. |
| `feedback_sound` | string | `"Sosumi"` | General | The FEEDBACK SOUND rung when a turn stops at NEEDS FEEDBACK — a permission prompt or a question — or when a session's CLI dies with an error in the middle of a turn, with the same values and fallbacks as `done_sound`, and a different default so red and green sound different from the next room. It never rings for the session whose pane you are locked into typing at while the terminal window has focus: that prompt, or that error, is already under your hands. The one switch for their DESKTOP NOTIFICATIONS too: while the terminal window is in the background (from the focus reports orion asks the terminal for — tmux needs `focus-events on`), each session that goes red is also named in a desktop notification (`Fix Login needs feedback`), and so is each that crashes (`Fix Login stopped with an error`). On macOS they come from orion's own NOTIFIER APP — a copy of Homebrew's `terminal-notifier` re-badged with orion's name and logo, built on first use under the data dir, whose click brings the terminal back; `osascript` when `terminal-notifier` is not installed; never over `orion ssh`, where the desktop is the wrong machine's; a notifier that is missing or fails is a debug line, not an error. `off` silences the sound and the notifications together. |
| `preset_text` | string | `"prefix"` | — (hand-edited) | PRESET TEXT: which side of the task a new AGENT PRESET's text goes — `prefix` (one box, sent before the task), `postfix` (one box, sent after it) or `prefix & postfix` (both, the form every preset had through 0.32). The PRESET EDITOR opens a new preset with the box(es) named here, and its **Text** row changes one preset — a side the row leaves out saves blank; editing a stored preset also shows any side that already holds text, so nothing saved is ever hidden. Prefix alone by default: the framing most people reach for, and one box to fill. A hand-edited `both` reads as `prefix & postfix`; anything else as the default. See [Sessions](sessions.md#agent-presets). |
| `theme` | string | `"default"` | General | The THEME: `default`, `ocean`, `forest`, `rose`, `amber`, `lavender`, `coral`, `slate`, `sand`, `mono`. An unknown name falls back to `default`. A theme is the accent — the focus chrome: the selection rail, the cursor's frame, the lit tab and rule — and the focus tint. Every status color is fixed and the same in every theme (crimson needs you or crashed, a gold spinner working, gray at rest, purple merged), as are the grays, so a terminal's palette can't move them; the one status that follows the theme is DONE, NOT SEEN, which takes the accent itself — sky blue instead where the accent is warm (`rose`, `amber`, `coral`, `sand`), gray (`mono`) or too near the merged purple (`lavender`). |
| `animations` | bool | `true` | General | Master switch for the motion: the UNREAD SHIMMER (a finish nobody has read, on its row and its project's tab, until it is read), the ONE-SHOT SWEEP (a session that just started needing you or crashed, a just-merged checkout, for about five seconds), the WORKING SPINNER a running session's dot turns into (a still `◐` with this off) and the SPLASH's motion. Off trades them for fewer repaints on a constrained machine. |
| `spotify` | bool | `true` | General | **Spotify in footer**: what the Spotify desktop app is playing, `♪ Midnight City · M83  ⏮ ⏸ ⏭`, in the FOOTER just left of the memory readout — the title in text (dimmed too while paused), the artist dim, and the three glyphs buttons: back a track (or to the start of this one, a few seconds in), play/pause, next. macOS only, asked over AppleScript once a second while a track is up and every five seconds while Spotify is closed or stopped; it never launches Spotify, and nothing is drawn while there is nothing to show. There are no keys: the Mac's media keys already reach Spotify. The first ask raises macOS's AUTOMATION prompt for your terminal; denied, orion stops asking for the rest of the run and says once where to allow it (System Settings → Privacy & Security → Automation). On a narrow bar the artist shortens first, then the title, and then the readout goes. Off stops the polling too. |
| `hide_draft_prs` | bool | `false` | — (hand-edited) | Leave draft pull requests out of the grid's PR COUNT and the jump list's (`⌘K`) pull-request rows, so browsing what's open shows only the rows asking for a reviewer; the PULL REQUESTS MODAL lists drafts either way. A view filter, not a fetch filter: the open-list lookup still fetches the drafts and the PR CACHE still holds them, so `false` brings them back at once and a draft marked ready joins the rows on the refresh that says so. Sessions and the pull request under their cards are never hidden. |
| `diff_tree_view` | bool | `true` | — (⌘B) | Whether the DIFF VIEWER (`⌘E`, and a pull request's diff) lists its files as a directory tree rather than flat paths. `⌘B` flips the open viewer and writes the answer here, so the next one — after a restart too — opens the way the last was left. Replaces the tree state the UI-state blob carried through 0.42, which no longer decides it. |
| `pr_autofix` | string | `"auto"` | General | **Auto-fix my PRs**: on (`auto`) or `off`. When one of *your* open pull requests (`gh`'s signed-in user opened it) hits merge conflicts or failing checks, a fresh `autofix-<n>` session starts on it, nothing to confirm, and the footer says so. It runs on the harness, Claude account, model and effort of the session that did the work on the PR's branch (the most recently active one there), and with none on your default agent at the **PR autofix** system preset's model and effort (as the PR's own agent is what it runs on, the preset's harness is not used). Its prompt is the PR and what broke, wrapped in that preset's text (see [Sessions](sessions.md#agent-presets)): orion's own rules unless you changed them — fix only what is listed; resolve conflicts; reproduce each failing unit test locally and fix it; re-run only the e2e specs that failed, never the full suite, unless earlier attempts failed the same way; push; watch the PR until it is green and go round again. Failing checks are acted on once every check has finished (or after 20 minutes); conflicts alone at once. Each breakage — head commit, conflicts, failing checks — is sent once, across restarts; there is never a second `autofix-<n>` on a pull request while one is starting, working or waiting on you. Another session live on the branch, working or waiting on you, does not hold it back: the agent is told where that session's conversation is and reads it first, and when that session is already on this fix it runs `orion autofix stand-down "<why>"`, which archives it, and the footer shows why. A live session whose conversation can't be read (not a Claude session, or no transcript yet) holds it back instead, and the footer says so once. After 3 sends in a row that didn't turn the PR green it stops, with a footer message and a desktop notification. Unit vs e2e is read off each check's name. Seen while the TUI runs: within about 15s for the selected project, 5 minutes for the others; a breakage from while it was closed is picked up on the next launch. `⌘G` in the PULL REQUESTS MODAL still opens the AUTOFIX MODAL for any PR, whatever this says. An older orion's `ask` reads as on. |
| `projects` | object | `{}` | Projects | PROJECT SETTINGS: one entry per project set up differently from the rest, keyed by the project's repo path exactly as the DAEMON stores it, holding that project's rows from the **Projects** tab — `{"projects": {"/Users/me/src/app": {"base_branch": "dev", "run_command": "npm run dev", "open_command": "open http://localhost:3000", "worktree_containers": "remove"}}}`. **Base branch** (`base_branch`, string, default `""`) is where the project's new WORKTREES start when nobody named a base — a bare `orion worktree`, the QUICK PROMPT's **+ new worktree** (`orion worktree --base` always wins). Empty, shown as `auto`, is origin's own default branch: `origin/HEAD` freshly fetched. A name — `dev`, `master` — is resolved the way `--base` resolves one: origin is fetched and origin's copy of that branch is the start point, untracked, never the checkout's local branch of that name; a branch origin lacks that the checkout has locally is used as named, and a name the repo has no branch of at all falls back to `origin/HEAD` with a line in `daemon.log`. A leading `origin/` is dropped. Typed, like the commands. The same branch is what the grid's ahead/behind counts, the COMMIT LIST and the background base fetch measure against, and what a merge never deletes. **Run command** (`run_command`, string, default `""`) is the RUN COMMAND `⌘⇧S` (**Start stack** in a card's or the project tab's right-click menu) starts in *that project's* worktrees; empty (shown as `not set`) is none, and **Start stack** says where to set one. Typed, not cycled: `Enter` opens a prompt titled with the project, an empty answer unsets it. The DAEMON reads it fresh at each start. **Open command** (`open_command`, string, default `""`) is its twin for the OPEN COMMAND `⌘O` → **Open command** fires on that project's worktrees — `open http://localhost:3000`, say — with the same prompt; the TUI reads it fresh at each press, since it runs on the machine you are sitting at. **On worktree delete** (`worktree_containers`, string, default `""` shown as `off`) is what deleting one of the project's worktrees does to the docker compose projects started in it — see [Worktree containers](#worktree-containers). **Linear key** is no setting: it says where the project's `LINEAR_API_KEY` was found, and `Enter` tests it ([Linear](#linear)). `merge_method` (string, default `""`, squash) has no row: it is the method the project's last merge took (`squash`, `merge` or `rebase`), which its next merge form opens on. Releases before these keys kept **Base branch**, **On worktree delete** and **Merge method** once for every project (`worktree_base_branch`, `worktree_containers`, `pr_merge_method`): the first time the grid lists the projects, each project with no value of its own takes the old one, and the old key leaves the file on the next save. The tab lists every project under its name and path; with no project in the tree it says so. A project with no entry reads as the defaults, and an entry that only repeats them is dropped on save, so the map names only the projects that differ; an empty row is left out of an entry rather than written; a key inside an entry this build doesn't know — the retired `hide_root_worktree` an older build wrote among them — is carried through a save. To the file's rules the map is one key: a value in it this build can't read costs the whole map, not one project. |
| `quick_prompt_kind` | string | `"claude"` | Accounts | Which AGENT KIND the QUICK PROMPT (`⌘N`) launches: `claude`, `codex`, `cursor`, `grok` or `opencode` — or another CLAUDE ACCOUNT's id (`claude-2`), which the **New agents on** row steps through right after `claude` and shows by name, `Claude (you@example.com)`. Its model and effort come from that harness's own defaults below, so this is one name, not a third pair. A kind or an account switched off here is stepped around, and an older orion reading an account's id falls back to Claude. The NEW AGENT PICKER opens on it too. |
| `follow_new_session` | bool | `true` | General | FOLLOW NEW SESSION (**Follow new session**): a QUICK PROMPT launch lands the cursor on the new session's card — the grid scrolls to keep it on screen and the pane shows it, the keys still on the grid — so a run of launches can be watched going up. It selects the card and no more: a QUICK PROMPT never enters its session's terminal. It covers every QUICK PROMPT: `⌘N`, **New agent — choose harness**'s box, **Duplicate session** on a card, and the box the PULL REQUESTS MODAL opens. Off, the cursor, the pane and the keys stay on the card you were on while the new session's card goes up in its band — first in it, stand-in rows first for a launch that cuts a fresh worktree — and the footer names the branch it went to (`started a session in feat`): the BACKGROUND LAUNCH's stillness, in the project on screen. A launch with no card under the cursor — the aim let go with `Esc`, an empty band, an empty grid — has nothing to keep and lands on its new card either way. Terminals (`t`) always come up in the pane. |
| `claude_enabled` | bool | `false` | Accounts | HARNESS TOGGLE. Off leaves Claude — the default CLAUDE ACCOUNT — out of the NEW AGENT PICKER, and skips the standing PREWARM POOL slot; existing sessions keep attaching and resuming. Every harness starts off — a file that never names the key reads as off too — until setup or the Accounts tab turns it on, so the picker only offers what you use. Its other accounts keep switches of their own (see [Claude accounts](#claude-accounts)). |
| `codex_enabled` | bool | `false` | Accounts | HARNESS TOGGLE for Codex, same rules. |
| `cursor_enabled` | bool | `false` | Accounts | HARNESS TOGGLE for Cursor, same rules. |
| `opencode_enabled` | bool | `false` | Accounts | HARNESS TOGGLE for OpenCode, same rules. |
| `claude_model` | string | `"default"` | Accounts | Default `--model` for new Claude sessions. The literal `"default"` is the sentinel meaning *don't pass the flag, let the CLI pick* — it is what you see in a fresh file, not a missing value. Overlay list: `fable`, `opus`, `sonnet`, `haiku` — unless `claude_models` below or Claude Code's own `availableModels` allowlist replaces it; any other string is passed through verbatim. |
| `claude_models` | array of strings | `[]` | — (hand-edited) | The Claude model rows every picker offers (the NEW AGENT PICKER and QUICK PROMPT submenus, the AGENTS TAB, the PRESET EDITOR) in place of the built-in aliases, verbatim, `"default"` always first: `["claude-sonnet-5", "us.anthropic.claude-opus-5-v1:0"]`. For an organization that restricts models (Claude Code refuses `--model sonnet` with *Model "sonnet" is restricted by your organization's settings. Using claude-sonnet-5 instead.*) or a provider whose ids the aliases don't reach (Bedrock, Vertex, a gateway; on Bedrock `sonnet` even means Sonnet 4.5). Empty, the list follows Claude Code's `availableModels` when one is on disk — `~/.claude/remote-settings.json` (server-managed cache), the macOS MDM profile, `managed-settings.json` and `managed-settings.d/` in the system directory, then `~/.claude/settings.json`, read once at TUI start — else the aliases. A hand edit here applies without a restart. |
| `claude_effort` | string | `"default"` | Accounts | Default reasoning effort (`--effort`) for new Claude sessions: `low`, `medium`, `high`, `xhigh`, `max`, or the `"default"` sentinel. |
| `codex_model` | string | `"default"` | Accounts | Default `--model` for new Codex sessions (Codex spells the flag the same way Claude does): `gpt-6-astra`, `gpt-6.1-sol`, `gpt-6.1-terra`, `gpt-6-luna`, or the `"default"` sentinel. |
| `codex_effort` | string | `"default"` | Accounts | Default `-c model_reasoning_effort=` for new Codex sessions: `low`, `medium`, `high`, `xhigh`, `max`, `ultra` (which models take which is Codex's call — Luna stops at `max`), or the `"default"` sentinel. |
| `cursor_model` | string | `"default"` | Accounts | Default model *family* for new Cursor sessions, from the cached catalogue (`cursor_models.json`), or the `"default"` sentinel. |
| `cursor_effort` | string | `"default"` | Accounts | The effort suffix the DAEMON joins onto `cursor_model` into one flat `--model <family>-<effort>` id. The choices follow the family, so the overlay row reads `n/a` while the model is unset or has no effort variants. |
| `opencode_model` | string | `"default"` | Accounts | Default `--model` for new OpenCode sessions: a `provider/model` id passed verbatim (`opencode models` lists what your machine has credentials for). The overlay lists a few well-known ids (`opencode/big-pickle`, `anthropic/claude-sonnet-5`, …); a hand-edited one passes through. `"default"` means don't pass the flag, so OpenCode opens on its own last-picked model. There is no `opencode_effort`: OpenCode has no effort flag (reasoning is a per-model variant picked inside its TUI), so its Agents section has no Effort row. |
| `harnesses` | object | `{"grok": {"enabled": false}}` | Accounts | The harness registry: per-harness deltas over the compiled-in known harnesses (Claude, Codex, Cursor, Grok Build, OpenCode), and whole new third-party CLIs. The Accounts tab grows one section per entry — Enabled, Model, and Effort rows while the harness offers effort — and the NEW AGENT PICKER, AGENT PRESETS, spawn, resume and hooks all read the merged rows. An entry keyed by a `claude_accounts` id is deltas over that account's row (its own model default, say). A hand edit that breaks one entry refuses its launches with the reason, never the whole file. Run `orion config harnesses` to print the effective rows to copy from. Written even when empty, so removing its last entry sticks. |
| `claude_accounts` | array | `[]` | Accounts | CLAUDE ACCOUNTS beyond the default one: `[{"id": "claude-2", "config_dir": "~/.claude-2"}]`. Each is a Claude Code config dir with a login of its own, launched as built-in Claude's row — program, flags, hooks, resume — with `CLAUDE_CONFIG_DIR` pointing there, and named after the email it is signed in as, after its `name` when it has one (`"name": "Work"`, written only while set — `Work (you@example.com)`). `enabled: false` switches one off (written only while off). The Accounts tab's **Claude accounts** section and first-run onboarding add, rename, sign in, sign out and remove them. Like `harnesses`, never sent over `orion ssh`. See [Claude accounts](#claude-accounts). |
| `account_tags` | object | `{}` | — | The ACCOUNT TAG each Claude account's sessions wear on the grid, set by hand: by account id (`claude` for the default one), `{"claude-2": {"tag": "wk", "tint": 2}}`, either half alone. With more than one Claude account, every list row and card shows two letters right after its status dot, tinted per account, so you can see which account each session runs on at a glance; Settings → Accounts and the usage modal show each account's tag. An account with no entry gets two letters from its name, else its email, never the same as another's, and the next of six tints, none of them a status color. Written only while set. |
| `keybindings` | object | `{}` | — (hand-edited) | KEYMAP overrides, keyed by action id, valued with a comma-separated chord list: `{"git_diff": "cmd+g, g"}`. An empty string deliberately unbinds; unknown ids are ignored. The Keys tab lists the result and edits nothing. |
| `prewarm_agents` | bool | `true` | — (hand-edited) | DAEMON-owned PREWARM POOL: keep one booted agent CLI standing by in the selected WORKTREE, so creating a session there adopts it and feels instant. **Costs one idle CLI process per warm slot** (150–300 MB each, up to 15 minutes), and that spare is a real session as far as the CLI is concerned — Claude's own `/list-agents` lists it beside the sessions you made, named after the directory (`my-repo-3f`). Off drains the pool on the DAEMON's next sweep (within 30 s). |

### Ghostty's keys in Orion.app

Ghostty claims a lot of ⌘ chords for itself and never passes them to the program inside it — `⌘K`
clears the screen, `⌘N` opens a window, `⌘T` a tab, `⌘⇧P` is its own palette, `⌘F` searches. In
[Orion.app](#orionapp) the program inside is always orion, so the app's own config clears Ghostty's
whole key table (`keybind = clear`) and binds back only what a Mac app should still answer to
itself:

| Key | Stays the app's |
|---|---|
| `⌘C` | copy — only while Ghostty has a selection of its own (a ⇧-drag); with none it reaches orion, and a text field copies its SELECTION, the editor its own, the session pane its drag selection |
| `⌘V` | paste |
| `⌘Q` | quit the app (sessions keep running in the daemon) |
| `⌘⇧W` | close the window |
| `⌘Enter`, `⌃⌘F` | full screen |
| `⌘=`, `⌘+`, `⌘-`, `⌘0` | text size |
| `⌘←`, `⌘→`, `⌥←`, `⌥→` | typed as the keys a shell reads, as Ghostty types them anywhere: the line's start and end (`^A`, `^E`), a word back and on (`⎋b`, `⎋f`) — what orion's fields, its editor and every agent's prompt expect |

Every other chord reaches orion as the key it is, whatever your keymap or a rebind says — `⌘T`,
`⌘N`, `⌘D`, `⌘W`, `⌘1`–`⌘9` and the rest of Ghostty's tab, split and window keys included. `⌘.` is
the one chord macOS takes
before a terminal can send it (it is Cancel, Escape's twin, in every Mac app), so the app's config
binds it to the bytes the kitty protocol spells it with (`csi:46;9u`); orion also reads an Escape
still carrying ⌘ as `⌘.`, for a terminal without that line.

Nothing is written into your own Ghostty config. So in a Ghostty window of your own — or in
Terminal.app, which never sends ⌘ — orion's ⌘ shortcuts don't reach it.

### Installing editors and agent CLIs

orion runs programs it does not ship: the editor behind the BUILT-IN EDITOR, and a CLI per harness.
`install.sh` makes sure of three after orion itself: git, which it stops without; fresh, the editor
with VS Code's keys every file opens in (`brew install fresh-editor`; without Homebrew, a pointer to
its install page); and gh, which pull
requests are read with (`brew install gh`, else a pointer to its install page); and on a
Mac, Ghostty, which Orion.app is made from (`brew install --cask ghostty`; without Homebrew, the
newest disk image Ghostty's own update feed lists, copied into `/Applications` — or `~/Applications`
when that can't be written — once the app inside is seen to be signed by Ghostty's team). It says nothing about what is already there, and `--no-deps` (`curl … | sh -s
-- --no-deps`) or `ORION_NO_DEPS=1` skips the step.

A first install on a Mac then opens orion as [Orion.app](#orionapp), so setup starts where the ⌘
shortcuts work and orion is in the Dock from then on: orion's keybinds go into Ghostty's config
first, so no reload is needed. The terminal the installer ran in can be closed. An update, `orion
upgrade`, ssh or tmux, `--no-launch` or `ORION_NO_LAUNCH=1`, or no Ghostty, skips it — an orion
that was here already moves with `orion app`.

### Orion.app

On a Mac orion can run as an app of its own: **Orion** in the Dock, Launchpad, Spotlight and ⌘-Tab
under its constellation icon, one click to open it, and a second click that brings the window back
rather than opening another. `orion app` opens it, and so does the installer on a first install.

orion is drawn in a terminal, so the app is one: a copy of the Ghostty.app on this Mac, kept at
`~/Applications/Orion.app` under orion's name, icon and bundle id (`dev.orion.app`) and signed ad
hoc — made the first time it is opened, in about a second. It needs Ghostty installed to copy, and
it reads your Ghostty config first, so your font and theme carry over, and then its own
(`Orion.app/Contents/Resources/orion.conf`), which wins: the window is black to the edge and opaque,
opens filling the screen, and shows the three window buttons and no title, and Ghostty's key table
is cleared down to a few ([Ghostty's keys in Orion.app](#ghosttys-keys-in-orionapp)).

**Nothing moves into the app.** It is a window onto the orion already here: it runs the installed
`orion` binary, attaches to the same daemon and reads the same data dir. Sessions keep running
through the move, and projects, settings, accounts and history are where they were. Deleting
`~/Applications/Orion.app` removes the app and nothing else.

**The app is where orion runs on a Mac.** With Ghostty installed, a bare `orion` typed in a terminal
opens Orion.app and leaves the terminal, saying so. An upgrade moves an orion kept in a terminal
over the same way: it reopens orion, and the reopening opens the app, before anything of the new
release is drawn — the release notes are read there. With the app already open it is brought to
the front instead. orion stays where it was started under tmux, over ssh,
on a Mac without Ghostty, when `ORION_RUNTIME_DIR` or `ORION_DATA_DIR` points it at a daemon
or data of its own, when it is a build from a checkout, and when `ORION_APP=off` says to — the way
back into a terminal should the app ever not come up. If the app can't be made, orion carries on in
the terminal and says why in its footer. The other commands (`orion add`, `orion doctor`, …) run
where they are typed.

The app starts orion through your login shell (`$SHELL -l -i`), so the PATH your shell profile sets
— the one your agents' CLIs are on — is the one orion gets; a click in the Dock has no other. From
the Dock it starts in your home folder, from `orion app` or setup in the folder you were in.
Quitting orion closes the app.

Desktop notifications come from the app itself — its name, its icon, and a click that brings its
window back — so macOS asks once whether Orion may send them. Outside the app they are posted with
`osascript`.

The copy is rebuilt when the Ghostty it came from is updated, when `orion` is installed somewhere
else, or when a newer orion gives it a different config or layout: by `orion app` when the app isn't running, or
as orion quits inside it. Ghostty's own updater is off in the copy. macOS treats it as a new app,
so whatever it once asked you to allow Ghostty — a folder such as Documents, notifications — it may
ask again for "Orion", and once more after a rebuild.

Inside orion, `i` on a row whose program isn't on PATH — setup's **Claude** page, **Open files in**
on Settings → General, a
Claude account's row or a harness's **Enabled** row on the Accounts tab, and the footer's word that
fresh is missing when a file was to open — shows the command that
installs it, and `Enter` runs it in the editor modal, the installer's own output on screen (`Ctrl+Q`
stops it), the footer reading `installing …` meanwhile. When it exits the modal stays, its title and
the footer saying whether the program is here now, until `Enter`; the footer and the page or tab
under the modal then keep the result. Ghostty counts as here once its app is in `/Applications` or
`~/Applications`. The rows read
PATH as they draw. An
installer that puts its program somewhere new and adds that to your shell profile leaves orion's
own PATH as it was: start orion again from a new shell. Nothing is installed unasked, and `orion
doctor` ([Commands](commands.md#checking-the-machine)) names the same commands without running
them.

| Program | Command | Documented at |
|---|---|---|
| fresh | `brew install fresh-editor` | [github.com/sinelaw/fresh](https://github.com/sinelaw/fresh#installation) |
| Claude (`claude`) | `curl -fsSL https://claude.ai/install.sh \| bash` | [code.claude.com](https://code.claude.com/docs/en/setup) |
| Codex (`codex`) | `brew install --cask codex`, or without Homebrew `npm install -g @openai/codex` | [github.com/openai/codex](https://github.com/openai/codex) |
| Cursor (`cursor-agent`) | `curl https://cursor.com/install -fsS \| bash` | [cursor.com/docs/cli](https://cursor.com/docs/cli/installation) |
| Grok Build (`grok`) | `curl -fsSL https://x.ai/cli/install.sh \| bash` | [docs.x.ai](https://docs.x.ai/build/overview) |
| OpenCode (`opencode`) | `curl -fsSL https://opencode.ai/install \| bash` | [opencode.ai/docs](https://opencode.ai/docs/) |
| Ghostty (macOS) | `brew install --cask ghostty` | [ghostty.org/download](https://ghostty.org/download) |
| git | `brew install git` | [git-scm.com](https://git-scm.com/downloads) |
| gh | `brew install gh` | [cli.github.com](https://cli.github.com) |
| Open files in (macOS) | `brew install --cask cursor` / `visual-studio-code` / `sublime-text` | each app's download page |

Without Homebrew an editor's or Ghostty's row names its install page instead and runs nothing;
in first-run setup `Enter` opens that page in the browser. vim has no
installer here: macOS ships it, and it is the last fallback.

### Prewarming

Orion boots processes you never asked for in two ways, worth knowing about on a laptop or a small
remote box — and worth knowing about if your sessions talk to each other. A worktree's dead sessions
boot when your selection rests on it, so attaching is instant; that is how orion works, with no
switch. And `prewarm_agents` keeps a warm spare: a bare `claude` sitting at its prompt in the
selected worktree. Claude's `/list-agents` shows it as a peer named after the directory, the same
way it names any session you have not titled yet, so a spare beside a fresh untitled session reads
as two copies of one session (`my-repo-3f`, `my-repo-a1`), one of them forever idle. That is the
spare, not a duplicate. The pool has no row; turn it off by hand, in `config.local.json` on a small
machine:

```json
{ "prewarm_agents": false }
```

Turning the pool off takes the standing spares away on the DAEMON's next sweep. `session_idle_timeout`
is what bounds the cost of both.

### The harness registry

The seven known harnesses ship compiled in, and `harnesses` edits them per field — or adds a new
CLI outright. Only your deltas go in the file; `orion config harnesses` prints the effective rows
to copy from. Disabling one is one line, and adding a CLI is one block: picker, presets, spawn,
resume, hooks and the Accounts tab section all follow, with no rebuild.

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
`"claude": {"hooks": null}` runs Claude with process-based status and no title sync (`hooks` names
one of `claude`, `codex`, `cursor`, `grok` or `opencode`). A row that
stops making sense (an empty program, a resume flag plus a resume subcommand, an unknown dialect,
an `env` name a shell can't export) refuses its launches with the reason while every other harness
keeps working. The per-harness keys
the Accounts tab edits (`claude_model`, `codex_enabled`, …) keep working as a fallback wherever the
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
the Accounts tab, and offered by **Continue on** — but every flag Claude's row carries is yours to
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
effort defaults until its own Model / Effort rows on the Accounts tab are changed, which write
`harnesses.<id>`. The `id` is what its sessions point back at, so it stays put; lowercase letters,
digits and hyphens, never a built-in's. `name` is the one it goes by, as typed — `Work Laptop` —
and the only thing a rename changes: never the `id`, never the dir. `enabled: false` switches one off like any harness. The
list is this machine's logins: a remote reached over `orion ssh` keeps its own. A session on an
account is a Claude session with the account's id beside it, so everything orion does for Claude
— model and title sync, usage, resume, the transcript checks — it does on every account. Take an
entry out and its sessions refuse to start ("Claude account `claude-2` is no longer set up") until
it is back; nothing is lost. Sessions an older orion started on an account are moved onto Claude
and that account once, when the daemon first starts after the upgrade (store migration 32).

**Named after who they are.** Everywhere an account is listed — the NEW AGENT PICKER, the QUICK
PROMPT's `Tab` list, the Accounts tab, onboarding, the preset editor, **Continue on** and the card a
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

**The Accounts tab** (`s`, then the Accounts tab, or **Claude accounts** in the COMMAND PALETTE) lists
every account under **Claude accounts**, the default first: its name, `on` or `off`, its dir, and
`signed in`, `not signed in` or `same as ~/.claude`. On an account's row:

| Key | Action |
|---|---|
| `Enter` | sign it in, or in again: asks for the email to sign in as — it fills Claude's login page; empty leaves the choice to the browser — then runs Claude Code's own `claude auth login [--email …]` with the account's `CLAUDE_CONFIG_DIR`, in the editor modal over the overlay. On a Mac the login page opens in a private window (see below). The browser finishes it; the row says who it is once the modal closes. When it can't — the page shows a code instead — paste the code into the modal and press `Enter`. `Ctrl+Q` closes it early |
| `r` | rename it: a prompt prefilled with its name, `Enter` keeps what is typed and an empty one takes the name away (back to `Claude (you@example.com)`), `Esc` leaves it. Only the name moves — the id its sessions point back at and the dir its login lives in stay. The default account's name is written as `harnesses.claude.label`, a hand-written entry's as its `harnesses` label |
| `o` | sign it out, behind a confirm: `claude auth logout` in the same modal. Its dir keeps its settings and transcripts |
| `←` / `→` | switch it on or off — the same switch as its section's **Enabled** row further down |
| `⌫` | remove an added account, behind a confirm: its entry (and any `harnesses.<id>` deltas, and the `⌘N` default when it named it) leaves config.json. `Enter` keeps its dir on disk, for a later **Add account** by the same name to take back whole — `t` moves it to the Trash; a dir another account also runs in is never moved. The default account, and a hand-written `harnesses` entry, are not orion's to remove: switch the one off, edit the file for the other |

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

**One login, twice.** claude.ai's browser sign-in approves whichever account the browser is
signed in to, so a second dir signed in from the same browser silently becomes the first account
again — one subscription, one usage limit, and **Continue on** gaining nothing. The Accounts tab,
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
to pass `--model` and `--effort`; unset values use the CLI defaults. The picker offers
only `default` unless you configure `models` and `efforts` lists in that same block.
`--rules` carries orion's guidance (and, on a fresh launch, the AUTO-TITLE instruction), and
orion installs its hooks as one file of its own, `~/.grok/hooks/orion.json` (`$GROK_HOME/hooks/`
when that is set), which Grok reads with no trust prompt. They report `SessionStart`,
`UserPromptSubmit`, `Stop`, `StopFailure` and the subagent events, so a Grok row shows working and
done, and its `sessionId` is saved for `grok --resume` after a restart. Grok has no hook for a
permission prompt, and orion passes no `--always-approve`, so a Grok row never shows that it is
waiting on you — the picker row says so. A `permissions_flag` of `--always-approve` in its
`harnesses` entry leaves it nothing to ask. Its first-turn `orion rename` asks for permission like
any other command until you allow it (`Always allow` at the prompt, or `allow =
["Bash(orion rename *)"]` under `[permission]` in `~/.grok/config.toml`).

### Linear

A pull request a `⌘L` launch opens is always attached to its issues through the API, as a closing
link, so Linear's pull request automations move the issue when it opens and merges. Linear keeps
one attachment per pull request, so it never duplicates, and the link does not depend on the branch
name carrying an issue ID. An issue sent to another worktree leaves the pull request orion had
attached it to. Whose issues `⌘L` lists is `linear_assignee_email`, set by hand (empty: the key's
owner). Each project's key is on Settings → **Projects**, under that project:

| Row | Default | What it is |
|---|---|---|
| **Linear key** | — | read-only: where that project's `LINEAR_API_KEY` was found — `found in .env.local`, `found in .env`, `found in orion's environment`, or `not found`. The lookup is the one every Linear call makes: the checkout's `.env.local`, then its `.env`, then orion's own environment. The value is never shown, logged or stored. `Enter` sends Linear's `viewer` query with that key and the row adds whose it is — `✓ Jane Doe · jane@acme.dev` — or why not — `✗ Authentication required`; the explanation line under the rows says it in full. A key that moves or changes since drops the answer again |

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
  it), read fresh on each use by both the daemon and the TUI, so hand edits apply without a restart.
  `s` opens the settings overlay over the same file, in five tabs. **General**: whether a new
  agent's card comes under the cursor, the idle timeout, auto-fixing your pull requests and the app
  files open in; the done sound (`done_sound`: a ding when a turn you haven't seen finishes, 3 s
  after it settles, once until you look at the session, and never within 10 s of another sound — a
  macOS system sound such as `Glass`, the default; `bell` for the terminal bell, which Ghostty keeps
  silent unless its `bell-features` include `audio`; or `off`. Over `orion ssh` and off macOS it is
  always the bell. While the terminal window is in the background the finish is also named in a
  desktop notification; `off` silences both) and the feedback sound (`feedback_sound`: the same
  choices, `Sosumi` by default, rung when a turn stops to ask you or crashes mid-turn — and, while
  the terminal window is in the background, a desktop notification naming the session and its
  worktree; `off` silences both) — stepping either sound row with ←/→ plays the sound it lands on,
  so you can pick by ear; and the theme, animations and Spotify in the footer. **Accounts**: which
  account `⌘N` starts on, the Claude accounts, and which agent CLIs the new-session menu offers (at
  least one stays on) with their default model and reasoning effort. **Projects**: each project's
  own settings (`projects`, keyed by repo path — the branch its new worktrees start from, `auto`
  for origin's default or a name such as `dev`; its run and open commands; what deleting one of its
  worktrees does to its docker compose projects; and its Linear key). **Presets** opens the AGENT
  PRESETS, and **Keys** lists the keys. `R` inside the overlay puts every setting — hotkeys
  included — back to its default, after a confirmation, and removes `config.local.json`.
- **Every grid key is rebindable by hand.** The overlay's Keys tab lists every action and what it
  answers to, and nothing on it edits: overrides go into the same file by hand
  (`"keybindings": {"git_diff": "cmd+g, g"}`), and an empty value unbinds. `Ctrl+q` is the one
  exception: it unlocks a terminal no matter what you bind, since unbinding your way out would trap
  you in the session.

## Backup, restore and other machines

```sh
orion config path                  # where each settings file is
```

- **Copy the file.** Both halves re-read `config.json` on every use, so a backup copied over it — or
  to wherever `ORION_CONFIG_FILE` points — applies at once, with no restart. `agent_presets.json`
  and `ssh_hosts.json` sit beside it and copy the same way; `config.local.json` is this machine's.
- **Over ssh**, `orion ssh` runs the remote's own orion on the remote's own settings: nothing of
  this machine's travels with it.

### Compatibility rules

Two orion versions share these files all the time: a remote a few releases behind the machine that
connects to it, a backup restored into a newer build. One rule keeps either from breaking the other:
**a key a build doesn't know is ignored when read and written back untouched on save** — a key a
newer orion added, or one an older orion wrote that this one has retired. The overlay's `R` reset is
the one thing that drops it. `crates/orion-core/fixtures/` holds the config file the newest release
wrote, every key off its default, and the test suite checks that it still loads and survives a save.

`config.json` carries no version number. An older build could only refuse a newer file or ignore the
number, and the keys a file holds already tell a newer build everything it needs.

## Run and open commands

Each project can carry two shell command lines, **Run command** and **Open command** on the SETTINGS
OVERLAY's Projects tab (`s`), kept in `config.json` under the project's `projects` entry. Both run in the
selected worktree's checkout:

- **Run command** — the RUN COMMAND: how a worktree's stack starts, `npm run dev` say. `⌘⇧S` on the
  grid, or **Start stack** in a card's right-click menu (or the project tab's), starts it in the
  background — its run line comes up in the band, and the cursor and the pane stay where they were;
  the same key, or the row now **Stop stack**, stops it. The DAEMON runs it in a RUN
  TERMINAL: your login shell (`$SHELL -l -i -c`, the wrapper an agent launch uses, so `npm`, `bun` or
  `mise` resolve the way they do typed) running that line and nothing else. Its process is the
  worktree's RUNNING state — it is a terminal in the worktree's band, its `▶` lit while the process
  lives. Like every session it outlives
  the TUI and every client sees it, and the idle reaper never takes it. Stopping sends the run a `^C`,
  as you would at a terminal, so its own trap winds down what it started — a compose stack, a dev
  server. Meanwhile its run line counts down, `stopping… 1:52`, and the footer says the key again kills
  it; a second stop, or two minutes without it exiting, kills the whole process tree and removes the
  row. A stop you asked for takes the run line with it once the run has wound down. A command that
  exits on its own, with nobody stopping it, leaves its line behind saying how it ended — a faint
  `done` for a clean exit, a red `exited 1` for any other; attaching it replays how the run ended,
  and nothing but **Start stack** runs it again — not an attach, not the prewarm sweep. The DAEMON
  reads the run's output for the localhost addresses it prints (`http://localhost:3000`,
  `http://127.0.0.1:…`, `http://0.0.0.0:…`) and every client shows their ports after the command,
  `:3000 :5173`, each a link to it on localhost; the first is what `⌘O` → **Open app** opens.
  **Restart stack** (the command palette, or the run line's right-click menu) stops the run as a stop
  does and starts it again in the same line once it has gone, with the command as it is set at the
  press; a run that isn't up just starts. There is one run per worktree: a second client's start on a worktree
  already running never starts another.
- **Open command** — the OPEN COMMAND, `open http://localhost:3000` say. `⌘O` → **Open command** on a
  worktree fires it once, from the TUI rather than the DAEMON, since what it opens — a browser tab, an
  editor — belongs on the machine you are sitting at. It runs through `$SHELL -c` with its output
  discarded, and orion does not wait for it. It answers from any card: the worktree is the one whose
  band the cursor is on. A card's right-click menu has **Open** too, and **Open checkout in editor**
  can be given a key of its own in `keybindings` (Settings → Keys lists every action and its keys).

Both are read fresh at every press, so an edit applies on the next **Start stack** or **Open command**.
They are one project's settings, so each project can run and open its own way; with none set, the
footer says so and names Settings → Projects. See `projects` in [Every setting](#every-setting).

## Worktree containers

A checkout that runs `docker compose up` — a database, a cache, an object store per worktree —
leaves that compose project behind when orion deletes the checkout: running, or stopped but holding
its volumes. **On worktree delete** (Settings → Projects, each project's `worktree_containers`) has the DAEMON deal
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
stack runs, faint while it is stopped. The DAEMON asks only while a client is connected: with none,
docker is left alone.

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

`daemon.log` and `tui.log` live in the state dir, under the DATA DIR — the `directories` crate has
no state dir on macOS, so orion uses `<DATA DIR>/state`:

- `~/Library/Application Support/dev.orion.orion/state/`
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
| Merged pull requests | the ones merged in the last 7 days among the 100 most recently touched |
| Linear issues | open ones — 100 of yours, 250 of your teams' — and the ones done in the last 7 days, 50 of each |
| Linear lists (`linear-cache/issues.json`) | each project's last answer, with the comments read on its issues — what `⌘L` and `⌘K` show before the next ask lands; rewritten whole when one lands |
| Pull request pages (`pr-cache/details/`) | one file each, while the pull request is on a row; written only when read again |
| Linear branch links (`linear-links.json`) | until 90 days after they were last linked, once no checkout is on the branch and no open pull request is from it |
| Ticked todos (`todos/`) | 120 days after they were ticked, pruned when the list loads; a group left with nothing in it goes with them. Open todos stay until you tick or delete them |
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
| `ORION_APP` | unset | `off` keeps a bare `orion` in the terminal it was typed in, on a Mac where it would otherwise open [Orion.app](#orionapp). The app's launcher sets it to `1` for the orion it starts; orion takes it out of its environment again, so nothing it starts inherits it. |
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
set yourself. For all of these, empty and unset mean the same thing: use the default.
