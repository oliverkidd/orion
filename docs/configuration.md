# Configuration

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Settings live in one JSON file, `config.json`, in the DATA DIR beside the database:

- `~/Library/Application Support/dev.orion.orion/config.json`
- `ORION_CONFIG_FILE` moves that one file (into a dotfiles checkout, say); a save writes through a
  symlink rather than replacing it. `ORION_DATA_DIR` moves the whole data dir.
- `orion config path` prints where each settings file is.

`config.local.json`, beside it, has the same keys and wins over `config.json` one key at a time:
for what only makes sense on this machine. Create it by hand with the keys to pin; from then on a
save writes a key it holds back into it. `onboarded`, `seen_version` and `linear_assignee_email`
are always written there.

The SETTINGS OVERLAY (`s` or `⌘,`) writes the file, and hand edits are fine: the TUI and the DAEMON
both read it fresh on every use, so a change applies without a restart. No key is required. A
missing file is all defaults, a value this build can't read costs only that key (it takes its
default and stays in the file as written), and a malformed file is logged and ignored.

## Every setting

The overlay has six tabs: **General · Accounts · Projects · Presets · Connections · Keys**.
`Tab` / `⇧Tab`, `[` / `]` or `1`–`6` change tab; `Enter` or `Space` toggles a row, `←` / `→` cycle it, and a typed
row opens a prompt prefilled with its value (empty puts the default back). `⇧R` puts every setting
back to its default, after a confirm, and removes `config.local.json`. **Presets** is the
[presets screen](#presets), kept in its own file; **Keys** lists every action and its chords and
edits nothing (rebinding is [`keybindings`](#hand-edited)).

The DAEMON reads `session_idle_timeout`, `harnesses`, `claude_accounts`, `quick_prompt_kind`,
`claude_enabled` and four keys of each `projects` entry (`base_branch`, `setup_command`,
`run_command`, `worktree_containers`). The TUI reads the rest.

### General

| Key | Default | Row | What it does |
|---|---|---|---|
| `follow_new_session` | `true` | Launching → **Follow new session** | A QUICK PROMPT launch puts the cursor on the new session's row and the pane on it, without entering it. Off, the cursor stays where it was and the footer names where the session went. |
| `session_idle_timeout` | `"5m"` | **Idle session timeout** | How long a session in a worktree no client is viewing may sit idle before the IDLE REAPER kills its PTY. See [below](#idle-timeout). |
| `outside_editor` | `"auto"` | **Open files in** | The app `⌘O` opens a file or checkout in: `auto`, `cursor`, `vscode` or `sublime`. `auto` is the first installed of Cursor, VS Code and Sublime Text, else macOS's `open`. `i` on the row installs a missing app (`brew install --cask cursor`, `visual-studio-code`, `sublime-text`). Over ssh a file opens in the BUILT-IN EDITOR. |
| `done_sound` | `"Glass"` | Sounds → **Turn finished** | Rung when a turn you haven't seen finishes: `off`, `bell`, or a macOS system sound (`Glass`, `Ping`, `Pop`, `Hero`, …). `←` / `→` play each. While the window is in the background the finish is also a desktop notification; `off` silences both. |
| `feedback_sound` | `"Sosumi"` | Sounds → **Needs you** | The same, for a turn that stops to ask you or a CLI that dies mid-turn. Never rung for the pane you are typing in. |
| `theme` | `"default"` | Look → **Theme** | The accent: `default`, `ocean`, `forest`, `rose`, `amber`, `lavender`, `coral`, `slate`, `sand`, `mono`. Status colours are the same in every theme. |
| `animations` | `true` | Look → **Animations** | The unread shimmer, the one-shot sweep, the working spinner and the splash's motion. Off means fewer repaints. |
| `spotify` | `true` | Look → **Spotify in footer** | What Spotify is playing, with ⏮ ⏸ ⏭ buttons, in the footer. macOS only; the first ask raises macOS's Automation prompt. Off stops the polling. |

Sounds are played with `afplay` and fall back to the terminal bell over ssh. Desktop
notifications are posted by Orion.app itself (its name and icon, a click brings the window back);
outside the app, with `osascript`; never over ssh.

#### Idle timeout

The overlay cycles `off`, `1m`, `5m`, `15m`, `30m`, `1h`; by hand any `<n>s`, `<n>m` or `<n>h`
works, and `"off"` or `"0"` turns reaping off. A malformed value means `5m`, not off. The reaper
sweeps every 15 s and spares running agents, agents waiting on you, agents with a backgrounded tool
call still running, and terminals with a command running. A reaped session comes back on the next
attach, and an agent resumes its conversation. A worktree's dead sessions also boot as the cursor
rests on it, so attaching is instant; this timeout is what bounds that cost.

### Accounts

| Key | Default | Row | What it does |
|---|---|---|---|
| `quick_prompt_kind` | `"claude"` | **New agents on** | What `⌘N` starts: `claude`, `codex`, `cursor`, `grok`, `opencode`, or a Claude account's id (`claude-2`). A harness or account that is off is stepped around; one at its usage limit is too, and the box says so. |
| `claude_accounts` | `[]` | **Claude accounts** | Claude accounts beyond the default one: `[{"id": "claude-2", "config_dir": "~/.claude-2"}]`. See [Claude accounts](#claude-accounts). |
| `account_tags` | `{}` | `c` / `g` on an account | The ACCOUNT MARK each account's rows wear, and its chip in Settings and Account usage, by account id (`claude` for the default): `{"claude-2": {"icon": "★", "tint": 2}}`. `icon` replaces the letter; `tint` picks one of six tints. Without an entry an account gets the first letter of its name or email no other mark uses. Marks show only with more than one account. |
| `claude_enabled`, `codex_enabled`, `cursor_enabled`, `opencode_enabled` | `false` | **Enabled** | Whether the pickers offer that harness. Sessions that exist keep attaching and resuming. A harness whose CLI isn't installed is never offered. |
| `harnesses` | `{"grok": {"enabled": false}}` | **Enabled**, **Model**, **Effort**, **Open links in** | What is yours of each harness and each Claude account, by id. See [The harness registry](#the-harness-registry). |

The tab opens on **New agents on**, then **Claude accounts**, then one section per harness and
account with its **Enabled**, **Model** and **Effort** rows, and on each Claude account an **Open
links in** row. `i` on a harness whose CLI is missing installs it.

**Open links in** is the browser links from that account's sessions open in: a link ⌥- or ^-clicked
in its pane, and the pull request `⇧V` or a card's menu opens from its card. `←`/`→` step `default`,
`Safari`, `Chrome`, `Firefox`, `Arc`; `Enter` types it, with a profile after Chrome or Firefox
(`Chrome Profile 1` opens Chrome's `--profile-directory=Profile 1`, `Firefox work` its `-P work`;
any other app's name works too, with no profile). It is stored as `harnesses.<id>.browser`, `{"app":
"Chrome", "profile": "Profile 1"}`. Every other link goes to the default browser, as does a URL
⌘-clicked in the pane, which your terminal opens itself.

### Projects

One block per project in the tree, each row reading and writing that project's entry in
`projects`, keyed by its repo path:

```json
{ "projects": { "/Users/me/src/app": {
    "base_branch": "dev", "setup_command": "just setup-worktree",
    "run_command": "just start", "open_command": "open http://localhost:3000",
    "worktree_containers": "remove" } } }
```

| Key | Default | Row | What it does |
|---|---|---|---|
| `base_branch` | `""` (`auto`) | **Base branch** | Where the project's new worktrees start. Empty is `origin/HEAD`, freshly fetched; a name (`dev`) is origin's copy of that branch, else a local branch of that name, else `origin/HEAD` with a line in `daemon.log`. A leading `origin/` is dropped. The grid's ahead/behind counts, the commit list and the background base fetch measure against it, and a merge never deletes it. `orion worktree --base` wins over it. |
| `setup_command` | `""` | **Setup command** | Run once in each worktree orion creates for the project. See [Setup command](#setup-command). |
| `run_command` | `""` | **Run command** | What `⌘⇧S` (**Start stack**) runs. See [Run and open commands](#run-and-open-commands). |
| `open_command` | `""` | **Open command** | What `⌘O` → **Open command** runs. |
| `worktree_containers` | `""` (`off`) | **On worktree delete** | What deleting a worktree does to the docker compose projects started in it. See [Worktree containers](#worktree-containers). |
| `merge_method` | `""` (squash) | — | The method the project's last merge took; its next merge form opens on it. |
| — | — | **Linear key** | Not a setting: where the project's `LINEAR_API_KEY` was found. See [Linear](#linear). |
| `takeaways` | `true` | **Link to meeting takeaways** | Whether takeaways from your meetings can be placed in this project's todos. |
| `about` | `""` | **What it is** | One line saying what the project is. Meeting takeaways and tickets are routed by it. |

orion writes **What it is** when you add a project, from its README and its last 20 merged PRs. It
asks the default Claude account once, at haiku and low effort. A project added before this release
gets its line the first time something needs it. `Enter` edits the line and `^R` writes it again.
Edit it if takeaways land in the wrong place.

A project with no entry reads as the defaults, and an entry that only repeats them is dropped on
save. A key inside an entry this build doesn't know is kept.

### Connections

What orion reaches outside itself through, a section each (**Connections** in the command palette
opens Settings here): **AWAY AND PHONE**, **SLACK**,
**MICROSOFT 365** and **GITHUB AND LINEAR**. `←→` changes a row, `Enter` edits it, `Tab` goes on
to the next tab and `Esc` closes. SLACK and MICROSOFT 365 each have a status row: `c` (or `Enter`)
connects it and `o` disconnects it. GITHUB AND LINEAR is two switches (below). A section whose rows haven't arrived yet reads
`not connected`, and its keys below are read as they are written.

**AWAY AND PHONE · Slack DM from Orion** — when you count as away, and what reaches your phone then
(see [How it works](how-it-works.md#phone-bridge)):

| Row | Default | What it does |
|---|---|---|
| Away when the screen locks | `on` | The screen locking counts as away. |
| Away after no keys or mouse | `10m` | `never`, `5m`, `10m`, `15m`, `30m` or `60m` with no keys or mouse. |
| Away by hand | `⌘⇧A` | Says which key does it; `Enter` here does it too. Your next key, in any orion, clears it. |
| Send to your phone while away | `questions, plans, crashes` | `←→` steps through every mix of the three, down to `nothing`. |
| Quiet hours | `22:00 to 07:00` | Nothing goes to your phone between them. `Enter` types them (`22:00 to 07:00`, `22:00-07:00`); empty is `off`. |

**SLACK** — the **Workspace** row says which workspace orion is signed in to, `connecting…`, what
is wrong, or `not connected`. `c` on it asks for the three tokens of the Orion Slack app in turn —
the app-level token (`xapp-…`), the bot token (`xoxb-…`) and the user token (`xoxp-…`) — and saves
them in the Keychain as `dev.orion.slack` / `app`, `bot` and `user`; `o` forgets them. Make the
app once from `docs/2026-11-07-needs-you/slack-manifest.json` (api.slack.com/apps › Create New App ›
From a manifest), install it, and create its app-level token with `connections:write`. The rest of
the section is what counts as a Slack message for you in Needs you: **Direct messages** (`every
message` / `off`), **Mentions of you** (`every channel` / `off`), **Threads you're in** (`new
replies` / `off`), **Channels, every message** and **Keywords** (typed, comma-separated), and
**Banner and sound** (`DMs and mentions`, `every message`, `never`). Those messages are the SLACK
column's cards on the [Needs you board](keys.md#needs-you-k); turning a row off hides the cards it
made. A thread counts as yours once you post in it, or when you started it. Channels are matched
by name (`#eng`), which orion asks Slack for once per channel; a keyword matches anywhere in a
message, any case. **Banner and sound** rings the feedback sound and, while orion's window is in
the background, posts a desktop notification naming the sender. An app made from an older copy of
the manifest lacks the `channels:read` and `groups:read` user scopes that name channels: add them
under OAuth & Permissions and reinstall.

**MICROSOFT 365.**
- **Status:** the email you signed in as.
  - It reads `needs your organisation's app id · c to add it` until the Entra app's two IDs are
    set. `c` (or `Enter`) then asks for the **Application (client) ID** and then the **Directory
    (tenant) ID**, both on the app's Overview page in Entra (your admin can send them). Both
    pasted on one line, apart by a space, comma or line break, skip the second box. Each must be
    a GUID (`8-4-4-4-12` hex digits, either case); anything else keeps the box up and says so.
    `Esc` or an empty box saves nothing. After the second, both are saved to `config.json` as
    `microsoft.client_id` and `microsoft.tenant`, and the sign-in starts. Writing them into
    `config.json` by hand works too.
  - `c` asks Microsoft for a sign-in code, copies it, and opens microsoft.com/devicelogin in your
    default browser. Paste the code there and sign in. The row reads `enter <code> in the browser`
    until you do.
  - orion keeps the refresh token and your email in the Keychain, under `dev.orion.microsoft`.
  - `o` signs out, and stops a sign-in that is still waiting. It leaves the two IDs alone; `o`
    again, with nobody signed in, clears them from `config.json`, so `c` asks for them again.
- **Next meeting in the footer** (`microsoft.footer_meeting`): see
  [Your next meeting](sessions.md#your-next-meeting).
- **Takeaways from Teams transcripts** (`microsoft.takeaways`): off, [meeting takeaways](sessions.md#meeting-takeaways)
  never asks Microsoft for a transcript, whatever its own **On** says.

orion signs in with these delegated permissions: `User.Read`, `Calendars.Read`,
`OnlineMeetings.Read`, `OnlineMeetingTranscript.Read.All` and `offline_access`.

**GITHUB AND LINEAR** `· through gh and project keys` says what the [Needs you board](keys.md#needs-you-k)
lists, two switches `←`/`→` or `Enter` turn on and off:

- **Code reviews asked of you** (`inbox.code_reviews`): open pull requests that ask you to review
  them — your projects' own, as their open lists say, and every other repo's, through one
  `gh search prs --review-requested=@me` every five minutes.
- **Linear comments that mention you** (`inbox.linear_mentions`): your unread Linear notifications —
  a mention, a new comment on an issue you're in, an issue given to you — asked with each project's
  `LINEAR_API_KEY` (one ask per distinct key). Marking one done on the board marks it read in
  Linear too.

```json
{ "away": { "screen_lock": true, "idle_minutes": 10,
            "phone": ["question", "plan", "crash"], "quiet": ["22:00", "07:00"] },
  "slack": { "dms": true, "mentions": true, "threads": true,
             "channels": [], "keywords": [], "banner": "dms_and_mentions" },
  "microsoft": { "client_id": "", "tenant": "organizations",
                 "footer_meeting": true, "takeaways": true },
  "inbox": { "code_reviews": true, "linear_mentions": true } }
```

| Key | Default | What it does |
|---|---|---|
| `away.screen_lock` | `true` | The screen locking counts as away. |
| `away.idle_minutes` | `10` | Minutes with no keys or mouse before you count as away; `0` is never. |
| `away.phone` | `["question", "plan", "crash"]` | What goes to your phone while you are away. |
| `away.quiet` | `["22:00", "07:00"]` | Quiet hours: nothing goes to your phone between them. `null` is none. |
| `slack.dms`, `slack.mentions`, `slack.threads` | `true` | Direct messages, mentions of you, and new replies in threads you're in count as messages for you. |
| `slack.channels`, `slack.keywords` | `[]` | Channels whose every message counts, and words that make a message count anywhere. |
| `slack.banner` | `"dms_and_mentions"` | Which messages bring a banner and a sound: `every`, `dms_and_mentions` or `never`. |
| `microsoft.client_id`, `microsoft.tenant` | `""`, `"organizations"` | The Entra app orion signs in to Microsoft 365 through. Not secret, so a team can share them. |
| `microsoft.footer_meeting`, `microsoft.takeaways` | `true` | The next meeting in the footer, and takeaways from Teams transcripts. |
| `inbox.code_reviews`, `inbox.linear_mentions` | `true` | Code reviews asked of you, and Linear comments that mention you, show in Needs you (GITHUB AND LINEAR's two rows). |

A key left out of a section keeps its default, the others as written. Tokens are never in the
file: orion keeps them in the macOS Keychain, under services named `dev.orion.<service>`.

### Automations

Set on the AUTOMATIONS screen (`⌘J`, see [Automations](sessions.md#automations)), not in
Settings. Global, never per project.

```json
{ "automations": {
    "max_running": 5,
    "per": {
      "pr_fixer": { "on": true, "extra_steer": "" },
      "bug_fixer": { "on": false, "labels": ["Bug"], "preset": "Careful" },
      "week_review": { "when": { "day": "fri", "at": "08:30" }, "whose": "just_me" } } } }
```

| Key | Default | What it does |
|---|---|---|
| `automations.max_running` | `5` | No automation starts a session while this many automation sessions are running (at work or waiting on you). `←` / `→` on the screen change it, 1 to 20. |
| `automations.per.<id>.on` | `true` for `pr_fixer`, `meeting_takeaways` and `blue_sky`, else `false` | The automation on or off. Ids: `pr_fixer`, `bug_fixer`, `feature_builder`, `week_review`, `meeting_takeaways`, `blue_sky`. |
| `automations.per.<id>.extra_steer` | `""` | Added to every prompt the automation sends, after a blank line, as `Extra steer: …`. |
| `automations.per.<id>.preset` | none | The preset `⌘U` picked in place of the automation's own system preset, by name. A name no preset has falls back to the system preset. |
| `automations.per.<id>.labels` | `[]` | The Linear labels a ticket assigned to you needs for the bug fixer (`bug_fixer`) or the feature builder (`feature_builder`) to pick it up, matched in any case. Empty is each one's own: `Bug`, `Regression`, `Customer bug` for the bug fixer; `Feature`, `Improvement` for the feature builder. **Labels** on the screen picks them from your Linear workspace. |
| `automations.per.<id>.when`, `.whose` | none (Fridays 08:30), `"just_me"` | When week in review runs — a day and a time, `{"day": "fri", "at": "08:30"}` — and whose work it covers (`just_me` or `whole_team`). |

`pr_autofix` (**Auto-fix my PRs**, through 2.x) is retired. A config that has it keeps it for the
older builds that read it, and its value turns the PR fixer on (`auto`, `ask`) or off (`off`) once,
until `automations.per.pr_fixer.on` says otherwise.

### Hand-edited

These have no row.

| Key | Default | What it does |
|---|---|---|
| `keybindings` | `{}` | Key overrides by action id, each a comma-separated chord list: `{"git_diff": "cmd+g, g"}`. An empty string unbinds; an unknown id is ignored. `Ctrl+q` always unlocks a terminal, whatever is bound. |
| `claude_models` | `[]` | The Claude model rows the pickers offer, in place of the built-in aliases (`fable`, `opus`, `sonnet`, `haiku`): `["claude-sonnet-5", "us.anthropic.claude-opus-5-v1:0"]`, for an organization allowlist or a provider such as Bedrock. Empty follows Claude Code's own `availableModels` when one is set, else the aliases. |
| `hide_draft_prs` | `false` | Leaves draft pull requests out of the grid's PR count and `⌘K`'s rows. The pull requests modal lists them either way. |
| `linear_assignee_email` | `""` | Whose issues `⌘L` lists. Empty is the owner of the project's `LINEAR_API_KEY`. Kept in `config.local.json`. |
| `diff_tree_view` | `true` | Whether `⌘E` lists files as a tree. `⌘B` in the viewer flips it and writes it here. |
| `onboarded` | `false` | Whether setup has been seen. Kept in `config.local.json`; delete it, or run **Run setup** (palette) or `orion setup`, to see it again. |
| `seen_version` | `""` | The release this machine last showed **What's new** for. Kept in `config.local.json`. |

## Claude accounts

Claude Code keeps an account's login, settings and transcripts in one config dir
(`CLAUDE_CONFIG_DIR`, else `~/.claude`). A CLAUDE ACCOUNT is that same `claude` pointed at another
dir. The default account is `claude`; each `claude_accounts` entry adds one. Its name, model and
effort are its `harnesses.<id>` entry, the default account's under `harnesses.claude`:

```json
{
  "claude_accounts": [{ "id": "claude-work", "config_dir": "~/.claude-work" }],
  "harnesses": {
    "claude": { "label": "Personal", "model_default": "opus", "effort_default": "xhigh" },
    "claude-work": { "label": "Work", "model_default": "opus", "effort_default": "medium" }
  }
}
```

An account runs as Claude in every way (flags, hooks, resume, usage, title and model sync), with
`CLAUDE_CONFIG_DIR` set. Its `id` is what its sessions point back at, so it never changes:
lowercase letters, digits and hyphens. `"enabled": false` on an entry switches it off. Take an
entry out and its sessions refuse to start until it is back. Everywhere an account is listed it
reads `Work (you@example.com)`, the email read from the account's `.claude.json`.

On the Accounts tab's **Claude accounts** rows:

| Key | Action |
|---|---|
| `Enter` | sign in (`claude auth login`, an email to sign in as asked first), in the editor modal. On a Mac the login page opens in a private browser window, so it can't silently sign in as the account the browser already holds |
| `r` | rename it (`harnesses.<id>.label`); empty takes the name away |
| `o` | sign out, behind a confirm; the dir keeps its settings and transcripts |
| `c` / `g` | the chip's tint / its letter or glyph (`account_tags.<id>`) |
| `←` / `→` | switch it on or off |
| `⌫` | remove an added account, behind a confirm: `Enter` keeps its dir, `t` moves it to the Trash |

**Add account** asks for a name (`Work` makes `claude-work` in `~/.claude-work`; an existing dir is
adopted), then offers to link the default account's `CLAUDE.md`, `settings.json`, `skills`,
`agents`, `commands`, `plugins` and `keybindings.json` into the new dir. Two dirs signed in as one
login are flagged (`same as ~/.claude`). `⇧C` on a session moves it, conversation and all, to
another account ([Sessions](sessions.md#usage-limits-and-a-second-account)).

## The harness registry

Claude, Codex, Cursor, Grok Build and OpenCode are compiled in: how each CLI is driven is orion's.
`harnesses` holds what is yours of each, one entry per id (a built-in, a Claude account, or a new
CLI):

| Field | What it is |
|---|---|
| `enabled` | whether the pickers offer it |
| `label` | the name it goes by |
| `model_default` | the `--model` new sessions get; `"default"` passes none |
| `effort_default` | the effort new sessions get (Claude `--effort`, Codex `model_reasoning_effort`, Cursor a model suffix, Grok `--effort`); `"default"` passes none |
| `models` | the model rows its pickers offer: `[{"id": "big-1", "hint": "the big one"}]` |
| `env` | environment variables for its CLI, exported after your login shell's profile runs; a leading `~/` is expanded, `null` clears it |
| `browser` | a Claude account's **Open links in**: `{"app": "Chrome", "profile": "Profile 1"}`; absent is the default browser. Only orion's window reads it |

An id no built-in or account holds adds a CLI by the name of its program: `"agy": {}` launches
`agy` with `--model`, the first prompt as its last argument, and status from the process. A broken
entry refuses its own launches with the reason; every other harness keeps working. Like every
object key, a `harnesses` map in `config.local.json` replaces the whole map from `config.json`.

Grok Build reports working and done through hooks (`~/.grok/hooks/orion.json`) and resumes with
`--resume`, but has no hook for a permission prompt, so a Grok row never shows it is waiting on
you; its picker row says so.

## Presets

Settings → **Presets** manages the AGENT PRESETS, kept in `agent_presets.json` in the data dir. A
preset is a name, the account or harness it **Runs on** (the box's own by default, or one it
pins), a model and effort (Settings → Accounts' by default), and one text with `{task}` where the
task goes (after the text, past a blank line, when it names none). Its **Task** row is `ask` (an
optional task is asked for) or `skip` (it launches on its text alone). The editor shows a preview
of what the agent will get.

**Used by orion** lists the seven SYSTEM PRESETS, each tagged `orion`:

| Preset | Used for | Runs on |
|---|---|---|
| **PR autofix** | the text round what broke, for every agent the PR fixer sends | the PR's own agent (the session that worked on its branch), else your default |
| **Linear Ticket** | wraps every `⌘L` launch; `{issues}`, `{ids}` and `{first_id}` are filled in | the box's agent |
| **Week review** | the rules the week in review is written by | your default agent's account, at sonnet · medium |
| **Bug fixer** | what the bug fixer tells the agent it starts on a labelled Linear bug | the box's agent |
| **Feature builder** | what the feature builder tells the agent it starts on a labelled Linear feature | the box's agent |
| **Meeting takeaways** | how takeaways are pulled from a Teams meeting's transcript | your default agent's account, at sonnet · medium |
| **Blue sky builder** | what the blue sky builder tells the agent it starts on an idea | the box's agent |

A system preset always has text: empty, it shows orion's built-in text greyed and uses it. It can't
be renamed or deleted. On it `Enter` edits and `^R` puts orion's text back. On one of yours, under
**Yours**, `Enter` edits, `^N` makes a new one and `^D` deletes it.

Presets are picked, never managed, elsewhere: `⌘U` / `^X` in the QUICK PROMPT — the box `Enter` opens on
a Linear issue or a todo — **Agent presets** in the palette, `⌘U` over an agent's locked pane, which
pastes the text in at the caret, and `⌘U` on the AUTOMATIONS screen, which picks the preset an
automation is told by in place of its system preset.

## Setup command

A project's **Setup command** (`setup_command`, also on setup's **First project** page) runs once
in each worktree orion creates for it: `orion worktree`, **+ new worktree**, `⌘⇧N`, a branch picked
in the worktree picker, a pull request's checkout, a spare worktree.

- It runs on a **setup** row in the worktree's band, through your login shell (`$SHELL -l -i -c`),
  in the new checkout once its `.env` links are in. There is no time cap; sessions can start in the
  worktree meanwhile.
- The row reads `setting up…`. A clean exit removes it; anything else leaves a red
  `setup failed (1)`, whose output attaching replays.
- **Start stack** waits for it: the footer says the stack starts once setup finishes, and a second
  `⌘⇧S` (or **Cancel start**) calls the start off.

Worktrees made outside orion run nothing.

## Run and open commands

- **Run command**: `⌘⇧S`, or **Start stack** in a row's or a project tab's menu, starts it in the
  background, in a RUN TERMINAL through your login shell. Its run line shows in the band with the
  localhost ports it prints (`:3000 :5173`, each a link; `⌘O` → **Open app** opens the first).
  `⌘⇧S` again sends it `^C`; the line counts down `stopping… 1:52` and a further press, or two
  minutes, kills it. A stop you asked for removes the line; an exit nobody asked for leaves `done`
  or a red `exited 1`. **Restart stack** stops it and starts it again. One run per worktree.
- **Open command**: `⌘O` → **Open command** runs it from the TUI with `$SHELL -c`, output
  discarded, since what it opens belongs on the machine you sit at.

Both are read fresh at every press. With none set, the footer says so and names Settings → Projects.

## Worktree containers

**On worktree delete** decides what a deleted worktree's docker compose projects get:

| Value | What happens |
|---|---|
| `off` (default) | nothing |
| `stop` | `docker compose -p <project> stop` |
| `remove` | `docker compose -p <project> down --remove-orphans`; named volumes kept |
| `remove+volumes` | the same with `--volumes` |

A project is matched by the directory its containers were started in, never by name; one with
containers started elsewhere too is left alone, with a warning. It runs only after a delete that
succeeded, works with any docker engine (OrbStack, Docker Desktop, Colima), and only ever warns on
failure. While a checkout is there, its band shows a `⬡` while it has a stack: green running,
faint stopped. The DAEMON asks `docker ps` every 5 s, and only while a client is connected.
`orion doctor` lists compose projects whose checkout is gone.

## Spare worktrees

For each project used in the last 24 hours, the DAEMON keeps one SPARE WORKTREE: cut from the
project's **Base branch**, its `.env` links made and its **Setup command** run, with a `claude`
booted on the `⌘N` account at its default model and effort, waiting at its prompt. Its branch and
directory are `orion-spare-<id>`; the grid, worktree sync and the worktree picker leave it out.

A new-worktree launch (`⌘⇧N`, **+ new worktree**, a Linear launch) on that account, model and
effort, in edit mode, takes it: the branch is renamed to the launch's (the directory keeps its
name), the worktree appears, the task is typed in as the first turn, and a new spare is cut behind
it. Any other launch cuts a worktree as usual.

The DAEMON fetches origin for a spare every 5 minutes; when the base moves, an untaken spare is
reset to it and its CLI rebooted (setup is not run again). A finished spare survives a DAEMON
restart; a half-made one is removed. There is no switch: no spare is kept while `⌘N` starts another
harness or its account is off. Each costs one idle worktree and a 150–300 MB process, counted in the
footer's memory readout.

## Linear

Each project's `LINEAR_API_KEY` is read from its checkout's `.env.local`, then `.env`, then orion's
own environment. Only that variable is read, and it goes only to `api.linear.app`, on `curl`'s
standard input. The Projects tab's **Linear key** row says where it was found (`found in .env.local`,
`not found`); `Enter` asks Linear whose key it is (`✓ Jane Doe · jane@acme.dev` or `✗ <why>`). A
pull request a `⌘L` launch opens is attached to its issues as a closing link.

## Orion.app

On a Mac orion runs as an app of its own: **Orion** in the Dock, Spotlight and ⌘-Tab. `orion app`
opens it, as does a first install. It is a copy of the Ghostty.app on this Mac, kept at
`~/Applications/Orion.app` under orion's name and icon and signed ad hoc. It reads your Ghostty
config (font, theme) and then its own, which makes the window black and full-screen and clears
Ghostty's key table.

Nothing moves into it: it runs the installed `orion` against the same daemon and data dir. With
Ghostty installed, a bare `orion` in a terminal opens the app. orion stays in the terminal under
tmux, over ssh, without Ghostty, with `ORION_RUNTIME_DIR` or `ORION_DATA_DIR` set, for a build
from a checkout, or with `ORION_APP=off`. The app starts orion through your login shell, so it gets
your PATH. It is rebuilt when Ghostty updates or orion moves.

### Ghostty's keys in Orion.app

The app's Ghostty keeps only these; every other chord reaches orion as the key it is:

| Key | Stays Ghostty's |
|---|---|
| `⌘C` | copy, only while Ghostty has a selection of its own (a ⇧-drag); otherwise it reaches orion |
| `⌘V` | paste |
| `⌘Q` | quit (sessions keep running) |
| `⌘⇧W` | close the window |
| `⌘Enter`, `⌃⌘F` | full screen |
| `⌘=`, `⌘+`, `⌘-`, `⌘0` | text size |
| `⌘←`, `⌘→`, `⌥←`, `⌥→` | typed as `^A`, `^E`, `⎋b`, `⎋f` |

`⌘.` is sent as itself (macOS would make it Escape). Nothing is written into your own Ghostty
config, so in a Ghostty window of your own Ghostty keeps its ⌘ chords.

## Installing editors and agent CLIs

`install.sh` makes sure of git (it stops without it), fresh, gh and Ghostty; `--no-deps` or
`ORION_NO_DEPS=1` skips that. Inside orion, `i` on a row whose program is missing (setup's Claude
page, **Open files in**, a harness's **Enabled** row, the footer's note that fresh is missing) runs
its installer in the editor modal. `orion doctor` names the same commands without running them.

| Program | Command |
|---|---|
| fresh | `brew install fresh-editor` |
| Claude (`claude`) | `curl -fsSL https://claude.ai/install.sh \| bash` |
| Codex (`codex`) | `brew install --cask codex`, else `npm install -g @openai/codex` |
| Cursor (`cursor-agent`) | `curl https://cursor.com/install -fsS \| bash` |
| Grok Build (`grok`) | `curl -fsSL https://x.ai/cli/install.sh \| bash` |
| OpenCode (`opencode`) | `curl -fsSL https://opencode.ai/install \| bash` |
| Ghostty | `brew install --cask ghostty` |
| git / gh | `brew install git` / `brew install gh` |
| Open files in | `brew install --cask cursor` / `visual-studio-code` / `sublime-text` |

fresh runs off orion's own config, `<data dir>/fresh/config.json` (orion only adds settings it
lacks, so your edits stay), with orion's theme written to `~/.config/fresh/themes/orion.json` and
fresh's animations on. Your own `~/.config/fresh/config.json` is never used. `ORION_EDITOR` names
any other editor, opened as `<command> +<line> <file>`.

## ssh and other machines

`orion ssh <host> [dir]` opens orion on another machine over `ssh -t`, installing it there first
when it is missing. What you drive is the remote's own orion: its daemon, sessions and settings.
Nothing of this machine's travels with it. Each destination is remembered in `ssh_hosts.json`
(newest first, up to 20); **SSH hosts** in the palette lists them: `Enter` connects, `a` adds one,
`d` forgets one. Connecting quits this TUI and runs `orion ssh`; the local daemon keeps running.

To back up or move settings, copy `config.json` (it applies at once) and `agent_presets.json`.

## Logs

`daemon.log` and `tui.log` are in `<data dir>/state/`. `ORION_LOG=debug` logs more. A log past 5 MB
when its process starts is moved to `daemon.log.1` (or `tui.log.1`) and a new one begun. No
`daemon.log` at all means the DAEMON never started.

## What orion keeps

Everything orion writes under its data dir:

| File | What it holds | Kept |
|---|---|---|
| `orion.db` | projects, worktrees, sessions, terminals, the TUI's remembered state, pull request read marks | read marks: 180 days after you last read the PR |
| `config.json`, `config.local.json` | settings | — |
| `agent_presets.json` | presets | — |
| `ssh_hosts.json` | ssh destinations | the 20 newest |
| `todos/` | each project's todo list, and Blue sky's (`blue-sky.json`) | ticked items: 120 days |
| `reviews/` | week in review runs, `all/<last day>/<project>.md` and a `meta.json` each | the last 12 runs |
| `pr-cache/` | the last pull request lists and pages | 100 open per project, the last 7 days' merges, the last 30 days' closes; pages while their PR is on a row |
| `linear-cache/issues.json` | each project's last Linear lists and comments | rewritten on each fetch |
| `linear-links.json` | which branch was cut for which Linear issue | 90 days after the last link, once nothing is on the branch |
| `usage.json` | the last usage readings (no tokens) | the accounts still set up |
| `cursor_models.json` | Cursor's model list | refreshed after 24 h |
| `recent_files.json` | Go to file's recent files | 20 per checkout |
| `reviewed.json` | the diff viewer's reviewed marks | until the checkout's HEAD moves |
| `needs-you.json` | which Needs you cards are done or snoozed, and the Slack cards | a done card until no source offers it; a snooze for its hour |
| `automations/linear-builder.json` | every Linear ticket the bug fixer and feature builder have picked up, so none is picked up twice | — |
| `automations/meeting-takeaways.json` | each meeting's takeaways and where they stand | two weeks after the meeting |
| `quick_prompt_drafts.json` | unsent QUICK PROMPT text | until it is sent or cleared |
| `attachments/` | files dropped or pasted into a prompt | 7 days |
| `fresh/config.json` | fresh's config | — |
| `claude-sign-in-browser` | the private-window script sign-in uses | — |
| `state/` | `daemon.log`, `tui.log` | 5 MB each, and the 5 MB before |

## Environment variables

| Var | What it does |
|---|---|
| `ORION_LOG` | tracing filter for the DAEMON and the TUI (`debug`) |
| `ORION_EDITOR` | an editor to open files in instead of fresh |
| `ORION_APP` | `off` keeps a bare `orion` in the terminal it was typed in |
| `ORION_CONFIG_FILE` | where `config.json` is; after changing it, `orion kill` and relaunch |
| `ORION_DATA_DIR` | the data dir (tests, parallel instances) |
| `ORION_RUNTIME_DIR` | the dir holding the daemon socket and pidfile (default `$XDG_RUNTIME_DIR/orion`, else `/tmp/orion-<uid>`) |
| `ORION_DEV_INSTANCE` | set by `make dev`: the DAEMON leaves the seeded projects' checkouts alone, moving no worktree folder and cutting or sweeping no spare worktree, since they are the real daemon's |
| `ORION_AGENT_CMD` | one command line in place of every agent CLI (tests) |
| `ORION_INSTALL_URL` | the install script `orion upgrade` and `orion ssh` fetch |
| `ORION_USAGE` | `off` stops usage reads |
| `ORION_UPDATE_CHECK_SECS` | how often to check for a new release (default 3600; `0` off) |
| `ORION_SPOTIFY_POLL_SECS` | how often to ask Spotify (default 1; `0` off) |
| `ORION_MICROSOFT_POLL_SECS` | how often to read your Microsoft 365 calendar (default 300; `0` off) |
| `ORION_MEETING_TAKEAWAYS_POLL_SECS` | how often meeting takeaways looks for a finished Teams meeting's transcript, for up to an hour after it ends (default 120; `0` off) |
| `ORION_MICROSOFT_GRAPH_URL`, `ORION_MICROSOFT_TOKEN` | a Microsoft Graph base URL and access token used instead of `https://graph.microsoft.com/v1.0` and your sign-in (tests: a fake Graph) |
| `ORION_AWAY_POLL_MS` | how often the DAEMON reads whether you are away, from the screen lock and the time since your last key, while Slack is connected (default 5000; `0` off, so only `⌘⇧A` makes you away) |
| `ORION_SLACK_API_URL`, `ORION_SLACK_TOKENS` | a Slack Web API base URL used instead of `https://slack.com/api`, and the three tokens as `<app>,<bot>,<user>` used instead of the Keychain's (tests: a fake Slack) |
| `ORION_PROJECT_ABOUT` | `off` stops orion writing a project's **What it is** (tests) |
| `ORION_BLUE_SKY_DIR` | the folder the Blue sky builder builds ideas tied to no project in (default `~/Code/orion-blue-sky`) |
| `ORION_NEEDS_YOU_POLL_SECS` | how often Needs you asks Linear for your notifications and `gh` for the reviews asked of you (default 300; `0` off) |
| `ORION_LINEAR_BUILDER_POLL_SECS` | how often the bug fixer and feature builder look for labelled Linear tickets assigned to you (default 120; `0` off) |
| `ORION_WEEK_REVIEW_CHECK_SECS` | how often week in review looks whether its **When** has come round (default 60; `0` off, so it never runs on its own) |
| `ORION_IDLE_REAP_MS`, `ORION_WORKTREE_SYNC_MS`, `ORION_STACK_POLL_MS`, `ORION_SPARE_TICK_MS` | the DAEMON's reaper, worktree sync, docker poll and spare worktree periods (tests) |

The DAEMON sets `ORION_AGENT_ID`, `ORION_API_URL` and `ORION_API_TOKEN` on every agent so its hooks
can reach it; they are not for you to set. Empty and unset mean the same.

## Compatibility

A key this build doesn't know is left in the file as it is, and survives every save. (`⇧R`, reset
all, is the one thing that drops it.)
