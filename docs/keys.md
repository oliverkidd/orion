# Keys

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Every binding below is a default, and every one of them is rebindable by hand in `config.json`'s
`keybindings`; the SETTINGS OVERLAY's HOTKEYS TAB (`s` or `⌘,`) lists every action and its chord —
see [Configuration](configuration.md). An action with no key at all is
still one keystroke away: the COMMAND PALETTE (`⌘⇧P`, or `:`) lists every action by name with
its key, and `Enter` runs it.

## How the screen is laid out

- **The FOOTER is the status bar.** Always, left to right: which orion this is (`v1.0.0` — a
  click on it goes HOME), where you are in the grid's own marks (`demo ⎇ feat ◐ fix-login`: the
  project, the checkout behind its SCOPE MARK, the session behind its STATUS MARK — each a link
  back to it on the grid), then `· archived`, `· full screen` or `· home` when you are somewhere
  other than the grid of live sessions, and at the right edge the live counts. Between them go KEY HINTS — but only while no modal is up, and
  then only about what the grid or the pane has under its cursor: on an empty band
  `⌘N new agent · t terminal · ⌫ delete worktree`, beside a pull request the pane can read
  `→ focus PR`. Wherever you are not on the grid, the first hint is the way back to it.
- **A modal's keys are on its own bottom border,** in one style everywhere —
  `╰ Enter launch · ⇧Enter newline · Esc cancel ─╯` — and the footer under it shows none. When a
  border is too narrow for every key, whole keys drop off its right end, the modal's main verb and
  its way out (`Esc`) last; a key is never cut in half.
- **A modal's explanation** of the row or setting under its cursor is a dim line inside the frame,
  right above the keys — in the SETTINGS OVERLAY, the COMMAND PALETTE, setup.
- **One spelling and one colour for every key.** A letter under a modifier is its capital — `⌘K`,
  `^K`, `⌥P`, `⇧A` — and a bare letter is the letter you type (`t`). Every key label is drawn in the
  theme's accent — in the footer, on a modal's border, in the new-agent box's header, the COMMAND
  PALETTE's key column, Help, the Keys tab — and what it does in dim grey. A key that belongs to an action is always
  spelled from the live keymap, so a rebind shows up everywhere at once, and its `⌘` chord is the
  one printed; a modal's own keys come from the same table its key handler reads.
- **Focus is visible.** With the screen split — the cards and the pane beside or under them — the
  side holding the keys wears the accent: the cursor's band and card on the grid, or the pane's
  title, the rule under it and its edge. See [The pane](#the-pane-focus-and-scroll).

## How the keys are laid out

- **A session is an agent or a terminal.** `⌘N` starts an AGENT — an AI coding CLI on a task — and
  `t` a TERMINAL — a plain shell. Both are SESSIONS, the word for what applies to either card: the
  attention walk, archive, delete ([Sessions](sessions.md)).
- **⌘ for the entry points, as in Cursor.** The things you open from anywhere — a new agent, the
  jump list, go to file, changes, the pane — are `⌘` chords. They work in Orion.app, orion's own
  app on a Mac (`orion app`), which hands orion every one but copy, paste, quit, close window, full
  screen and the text size; a Ghostty window of your own keeps `⌘K`, `⌘N` and the rest for itself,
  and Terminal.app and tmux never send ⌘. Setup starts by offering to reopen orion in its app.
- **⌘ reaches orion from inside a pane.** No one types a ⌘ chord as text, so `⌘K`, `⌘P`, `⌘E`,
  `⌘⇧P` and the rest leave a locked pane and run, the agent losing nothing; every `Ctrl` chord stays
  the agent's while its pane is locked — `^P` there is the agent's `^P`. One ⌘ chord stays in the
  pane: over an agent, `⌘U` pastes a preset into its prompt.
- **Esc climbs one level.** Modal → pane → grid, everywhere: it leaves a locked pane (`⇧Esc` sends
  the agent its Esc), brings a full-screen session back down to its pane, hands the keys back from
  the pane to the grid, folds the open band, leaves the ARCHIVED VIEW and HOME, and closes any list
  outright — the jump list, the file finder, the diff viewer, a menu, help — whatever you typed into
  its filter. Forms keep a staged Esc of their own: a prompt, a confirm, the preset editor and the
  pull request modal back out one step at a time, so a half-typed comment is never lost
  to one press. `^Q` still force-closes any OVERLAY in one press. See
  [Back to the grid](#back-to-the-grid).
- **The grid's own keys are few and bare.** Arrows walk the cards; a handful of letters act on the
  card under the cursor (below). Everything else lives behind `⌘O` (open outside orion), the
  right-click menu, or the COMMAND PALETTE.

Orion.app's Ghostty is started with a key table of orion's own, so `⌘⇧P`, `⌘N`, `⌘,`, `⌘T` and the
rest reach orion rather than Ghostty — and `⌘.`, which macOS turns into Escape, arrives as `⌘.` —
see [Configuration](configuration.md#ghosttys-keys-in-orionapp). Nothing is written into your own
Ghostty config.

## Entry points

| Action | Key | What it opens |
|---|---|---|
| New agent | `⌘N` | the QUICK PROMPT, aimed at the checkout under the cursor (below) |
| Select model | `⌘/` | a searchable model list for the QUICK PROMPT, opening the box first when it isn't up; type to narrow, `Enter` picks |
| Cycle effort | `⌘Y`, `⌘?` (`⇧⌘/`) | step the effort (`default`, low, high, …) of the model the QUICK PROMPT is set to, shown in its header at once, and the Accounts tab default with it; with no box up, that default alone. macOS keeps `⇧⌘/` for every app's Help menu, so in Ghostty it opens Help and only `⌘Y` arrives; it stays bound for a terminal that lets it through, however that spells the press — `/` with ⇧, `?` with or without it |
| Select worktree | `⌘.` | which checkout the next agent runs in, opening the box first; type to narrow, or **+ new worktree**, or any branch with no checkout yet — a teammate's included |
| Jump to… | `⌘K` | fuzzy jump across every project, worktree, session, open pull request, open Linear issue and open todo (below); its last row opens a folder as a project |
| Go to file | `⌘P` | the FILE FINDER for the selected worktree |
| Find in files | `⌘⇧F`, `⇧F` | `git grep` into the same modal |
| File tree browser | `⌘B` | the TREE BROWSER |
| Todos | `⌘I` | the TODOS MODAL: every project's own todo list, a tab each, opened on the selected project's (below) |
| My week in review | `⌘⇧Y`, `⇧W` | the WEEK IN REVIEW: what was merged and done in the last 7 days (or 2 weeks, or a month), written up by product area with what to show and what to say, and the reviews kept (below). `⌘⇧W` would read better, and Ghostty keeps it |
| Changes | `⌘E` | the DIFF VIEWER for the selected worktree — its uncommitted changes, the whole branch, or the commits you tick, read together — or the diff of the pull request the pane is reading ([below](#the-diff-viewer)) |
| Command palette | `⌘⇧P`, `:` | every action by name, with its key; `:` is vim's command line |
| Open outside orion | `⌘O` | the OPEN MENU: the repo or pull request on GitHub, the checkout in your editor app (**Open files in**: Cursor, VS Code, …), a terminal in the checkout, the checkout's open command, and **Open app** — the first localhost port the checkout's run printed (`localhost:3000` on its row), in the browser |
| Reload from GitHub | `⌘R` | ask `gh` again now for the project's open pull requests, the selected worktree's pull request and the one the pane is reading |
| Toggle pane | `⌘J` | fold the pane beside the cards away or bring it back, from the cards or from inside the pane |
| Full-screen session | `⌘F` | give the session in the pane the whole screen, or bring it back down |
| Settings | `⌘,`, `s` | the SETTINGS OVERLAY |
| Home | `⌘G` | HOME: orion's animation over the grid — the hunter, the name small in his chest, and one line under the sky, `Enter to begin`; `Esc`, `Enter` or an arrow comes back to the grid exactly as it was. With a newer release waiting that line reads `⇡ vX.Y.Z available · Enter to upgrade` on the upgrade's green, and `Enter` asks to install it — once: turned down, it begins again. A click on the footer's `vX.Y.Z` does the same |
| Quit | `^C` | leave the TUI, behind a confirm; sessions keep running in the DAEMON |

## The grid

orion's whole screen is a GRID (see [Sessions](sessions.md#the-grid)): the sessions and
terminals of the project whose PROJECT TAB is lit in the header, a line apiece, grouped into a
BAND per checkout under a titled rule — with a pane beside them that reads the entry under the
cursor. `↑`/`↓` walk the lines, every band listing all of its entries.

| Key | Action |
|---|---|
| `↑` / `↓` | walk the lines — down a band's sessions and terminals and off its last line onto the next band's first, the pane following onto each. On the first line `↑` has nowhere to go: one press stays put, and a second within 400 ms walks up into the PROJECT TABS. Walking reads what the pane lands on — a `done` badge comes down as you arrive |
| `→` | with the pane beside the grid, into the pane, to read it ([below](#the-pane-focus-and-scroll)) |
| `Enter`, a double-click | into the pane on the card under the cursor, its input locked. On a band with nothing on it but its pull request, into the pane to read it. On a terminal too short to draw the pane the session takes the whole screen instead |
| `Esc` | in the ARCHIVED VIEW, back to the live sessions; else let the band under the cursor go — the pane collapses. What was under the cursor is only let go of: the next arrow takes the aim back |
| `Space` | the FOLLOW-UP MODAL: a small box over the grid for the selected session's next turn. `Enter` sends it down the PTY, `⇧Enter` / `⌥Enter` / `^J` break a line, `Esc` closes without sending. Nothing about the pane moves. Only a live local agent takes one; a session whose CLI is not up is booted first and the box kept |
| `t` | a new shell terminal inside orion, in the cursor's checkout — a chip on the grid with the pane on it and the keys in it. `⌘O` → **Terminal in the checkout** opens one outside orion |
| `` ` `` | the next TERMINAL chip of the cursor's checkout, round to the first |
| `r` | rename the session under the cursor |
| `⌘⇧A` | archive the session, behind a CONFIRM DIALOG (`Enter` or `y` archives, `Esc` or `n` keeps it); on an archived card, unarchive it. Never a bare letter, so a stray keypress can't file a session away. Held, it opens one dialog and archives nothing by itself |
| `⌘⇧U` | unarchive the archived session under the cursor — a line of an ARCHIVED DRAWER, or a card in the ARCHIVED VIEW. The cursor stays on it, a live card again. Held, it unarchives one |
| `z` | **the ARCHIVED DRAWER**: fold or unfold the `▸ 3 archived` line under the band the cursor is on — that checkout's archived sessions, a faint line apiece, most recently archived first. Unfolding puts the cursor on the newest, so `z` then `⌘⇧U` brings back the session just archived; `↑` / `↓` walk through the drawer between the band's cards and the next band. A click on the line is `z`, a double-click on a session's line unarchives it. Remembered across restarts |
| `⇧A` | **the ARCHIVED VIEW**: the same grid, of the project's archived sessions. `⌘⇧U` (or `⌘⇧A`) unarchives the card under the cursor and `Backspace` deletes it; `⇧A` again, or `Esc`, comes back |
| `Backspace` | delete the card under the cursor, behind a confirm. On the last live card of a linked worktree the confirm asks about the checkout too — `y` deletes both, `n` the card only, `Esc` keeps the card. A checkout with uncommitted or untracked work asks once more before it goes, counting the files it would lose (commits on its branch are kept), so nothing is lost by accident. On an EMPTY BAND it asks to delete the worktree itself; on a terminal's chip it closes the terminal and kills its shell |
| `⌘⌫` (`⇧D`) | **Delete worktree**: the checkout of the band under the cursor — from any card on it — deleted from disk with every agent and terminal in it, behind one confirm that names them all. A checkout with uncommitted work asks once more, as `Backspace` does. The root checkout is never deleted; on its band the key only says so. In a locked pane `⌘⌫` stays the agent's: it goes down as `^U`, kill-line |
| `⇧U` | **ACCOUNT USAGE**: how much is left on each linked account: a heading per Claude account and one for the `cursor-agent` login, and under it a line per limit its provider reports — 5-hour, Weekly, model-scoped weeks such as Opus, Monthly (Cursor's billing cycle). A line is a bar that starts full and empties as the limit is used, the share left and the time until it resets, `72% left`, `resets in 3d 9h`: green above 25% left, yellow down to 5%, red below that. A limit the provider doesn't report isn't drawn. When a session on the account has stopped at its limit, the account's name turns red and the limit with least left says `limit reached`. Accounts signed in as one login share one heading. Read every 15 minutes in the background and kept across restarts, so the modal opens on the last reading; `r` reads again now (at most once a minute per account). A failed read keeps the last numbers, dimmed, and says why. `Enter` on a signed-out Claude account signs it in. `⇧U`, `q` or `Esc` closes it |
| `⌘⇧S` | **start or stop the stack** of the cursor's checkout. Starting runs its RUN COMMAND — the project's **Run command** (Settings → Projects), say `just start --no-open` — in its RUN TERMINAL, in the background: the run line comes up in the checkout's band, and the cursor and the pane stay where they were; with none set the footer says so. On a checkout still running its **Setup command** the start waits for the setup to finish, the footer holding `setting up · the stack starts once setup finishes · ⌘⇧S again cancels`; a second press (or **Cancel start** in the menu) calls it off. On a checkout whose run is up it sends the run a `^C`, as you would at a terminal, so the run's own trap winds its stack down, and once it has, its run line goes. Until then the line says `stopping… 1:52`, counting down to the kill, and the footer holds `stopping run · ⌘⇧S again kills it now`: a second press kills it at once, and one that ignores the `^C` is killed after two minutes. **Restart stack** (the palette, or the run line's right-click menu) is a stop then a start: the line says `restarting… 1:52` while the run winds down, then the run comes back in the same line; pressed again it kills the run and starts it at once. A stack up with no run of orion's behind it — started in another terminal — is only its band's `⬡`: orion leaves it to whoever started it. Also **Start stack** / **Stop stack** in a card's, a band's, a project tab's or the run line's menu. A worktree with a compose stack has a `⬡` at the right end of its band's rule, green with a light running into it every eight seconds while the stack runs, faint while it is stopped |
| `p` | **pull** the cursor's checkout — the root or a linked worktree: fetch, then fast-forward onto the branch it tracks, never a merge or a rebase. A branch with commits of its own and new ones upstream, or uncommitted changes in the way, stays as it was and the FLASH says why. With an agent running in the checkout it asks first. A branch that tracks nothing yet (a new worktree before its pull request is pushed) only fetches origin — which brings `⇣` current on every band of the project. Also **Pull** in a band's or a card's menu; the FOOTER offers it on the root's band when it is behind |
| `⇧P` | **push** the cursor's checkout: fetch, then send its new commits to the branch it tracks, never a force. A branch the remote has moved on from too stays as it was, and the FLASH says why; a push onto the base branch itself (`origin/main`) is held, the FLASH asking for `⇧P` again — a second press within 30 s, not a dialog, so nothing opens under keys typed meanwhile. That press sends the commit it was held at; if HEAD moved since, it is held again with the new count. A branch that tracks nothing yet — a new worktree's — is published to origin under its own name and tracks it from then on. Only commits go: the FLASH counts the uncommitted files left behind. A fork's pull request checkout is refused (push to the fork from a terminal). Also **Push** in a band's or a card's menu; the FOOTER offers it on the root's band when it has commits to push |
| `⇧C` | **continue on another account**: the Claude session under the cursor, conversation and all, carried onto another Claude account and resumed there — a list of the accounts it can go to, `Enter` goes. For a session stopped at a usage limit (see [Sessions](sessions.md#usage-limits-and-a-second-account)) |
| `v` | the PULL REQUESTS MODAL for the selected project (below) |
| `.` / `,` | next / previous session in attention order — waiting on you first, then running, then the unread finishes, then the rest by last interaction — wrapping at both ends, in any project. Landing on an UNSEEN finish reads it |
| `[` / `]` | the project tab to the left / right, stopping at the ends |
| `1`–`9` | that PROJECT TAB, counting from the left; rebindable per slot as `project_tab_1` … `project_tab_9` |
| right-click | a card's, a band's or a project tab's context menu; a card in a checkout that works on a Linear issue (linked to it, or named in its branch) has **Open Linear issue** there, opening the Linear issues modal on it |

The actions with no key of their own — **New agent — choose harness** (the NEW AGENT PICKER), **Agent presets**,
**Duplicate session**, **Comment on pull request**, **Delete all sessions**, **SSH hosts**,
**Keyboard shortcuts**, **Open a folder as a project**, **Claude accounts**,
**Open skills folder** (your Claude Code skills, `~/.claude/skills`, in the TREE BROWSER) — are
in the COMMAND PALETTE and, where they apply to a card, its right-click menu. Bind any of them in
`config.json`'s `keybindings`.

### The locked pane

| Key | Action |
|---|---|
| anything | forwarded raw to the PTY — until the session's process exits, when `Esc`, `Enter` and `q` leave the lock and every other key falls through to the grid |
| `Esc` | leave the pane, back to the card it reads |
| `⇧Esc` | the agent's Esc: a bare Esc down the PTY |
| `^Q`, `^]`, `^⇧H` | also leave the pane; `^Q` is the HARDWIRED UNLOCK, which no rebind can take away |
| `⌘U` | over an agent: pick one of your saved AGENT PRESETS and its text is pasted at the caret, unsent, the pane still locked. A prefix ends on a blank line for the task, a postfix starts on one so it follows a task already typed, and a preset with both lands whole with the caret after it. Only the text travels, not the preset's harness or model. A long preset shows as the harness shows any long paste — Claude Code's `[Pasted text #1 +4 lines]` — and is sent in full. `⌘N` makes a new one from the list. Over a shell, `⌘U` is the pull requests modal as anywhere else |
| any ⌘ chord orion binds | leave the pane and run it — `⌘K`, `⌘P`, `⌘E`, `⌘J`, `⌘⇧P` … |
| `⌘F` | full-screen the session, or bring it back down; never forwarded to the agent. `Esc` and `^Q` also bring a full-screen session back down |
| mouse wheel | a child that asked for the mouse (Claude Code's alt-screen UI, `vim` with `mouse=a`, `htop`) gets the notch as a real wheel report; an alternate-screen app that wants no mouse (`less`, plain `vim`) gets `↑`/`↓`; anything else scrolls orion's own scrollback, and the header counts how far back you are as `scroll N` |

### The pane: focus and scroll

The pane beside (or under) the cards reads the card under the cursor — a session's terminal or a
pull request. It can take the keys without typing into anything, to be read and scrolled; the
side that has the keys wears the accent (the pane's title, the rule under it and its edge), and the
footer says so, leading with the way back.

| Key | Action |
|---|---|
| `→` off a row's last card, Enter on a band with only a pull request, a click on the page | the pane takes the keys — unlocked: nothing typed goes to a session from here. A pull request still loading takes them too |
| `↑` / `↓` | a line of the page, or of the session's scrollback (a program that took the mouse, like Claude Code, gets the wheel's notches); on a pull request's Changes, Commits or Checks tab, the row cursor |
| `PgUp` / `PgDn`, `Home` / `End` | a page; the top and the end — on a tab that lists things, its rows a page at a time and the first and last |
| `Tab` / `⇧Tab` | a pull request's next and previous tab ([below](#the-pull-request-page)) |
| `Enter` | into what it shows: a session takes the input lock; on a pull request, what its tab's row is — the DIFF VIEWER at the file, the commit's diff, the check's page — the [PR review](#the-pr-review) on Reviews, and the pull request in the browser on Description |
| `⌘G` | on a pull request, the AUTOFIX MODAL for it, as in the pull requests modal; `⌘G` is HOME everywhere else |
| `Esc`, `←` (the pane beside the cards), a click on a card | back to the grid |
| mouse wheel | scrolls whatever is under the pointer — the pane or the grid — wherever the keys are |

### The pull request page

A pull request reads as one page, in the pane beside the cards and in the PULL REQUESTS MODAL's
right half alike, laid out the way Cursor lays one out: its number and title, where it stands —
`ready for review · kate · main ← feat/links · +106 −4`, a merge conflict in red — and a row of
TABS, the active one lit, each with its count, the checks and the reviews marked with their verdict:

```
 Description   Changes 11   Commits 4   ✗ Checks 44/45   ✓ Reviews
```

Between that first line and the tabs, the **work block** says what is at work on the pull request, a
line each, and only the lines that have something to say:

```
 AUTOFIX   ✗ 1 failing check, 3 files conflict · ⌘G send autofix
 LINEAR    ◑ ENG-12 In Progress   ○ ENG-15 Todo
 SESSIONS  ● Ri billing   ◐ Pe docs
```

- **AUTOFIX** is the `autofix-<n>` at work on it and what it is doing (`working · 4m`, `waiting on
  you`) — or, while it is open and broken with none on it, what broke and the key that sends one, a button too.
- **LINEAR** is the issues linked to it — the ones its branch was linked to (`⌘.`, a `⌘L` launch) and
  the ones Linear has it attached to — each with its state.
- **SESSIONS** is the agents on its head branch, each with its account's tag as its grid row wears it.

A click on a session lands on it in the grid; a click on an issue opens it in `⌘L`; `⌘G` opens
the AUTOFIX MODAL on the page in the pane as in the modal.

The head stays put and the tab under it scrolls. All of it is one `gh pr view`, read when the cursor
rests on the pull request and remembered across launches; until it lands the tabs are up with `…`
for their counts, and the keys work already. The tab stays as you walk from one pull request to the
next — reading the checks of one after another is the common walk — and each tab's rows start again
at the top.

| Tab | What it lists | Enter (`⌘E` / `⌘O` in the modal), a click |
|---|---|---|
| **Description** | the body as markdown, then the conversation — comments and reviews, oldest first | the pull request in the browser |
| **Changes** `11` | every file: its status (`M`, `A`, `D`, `R`) and its `+`/`−`. GitHub hands back the first hundred; past that the tab says so and `⌘E` reads every file | the DIFF VIEWER on the pull request, at that file |
| **Commits** `4` | newest first: short sha, subject, author and age, and how big it is — `+12 −3 · 4 files` | the DIFF VIEWER on that commit's own diff |
| **Checks** | failed first, then running, passed and skipped: `✗` / `●` / `✓` / `–`, the name, its workflow and how long it ran. The tab reads `✗ Checks 44/45` with one failed (44 of 45 fine), `● Checks 3/5` while some still run, `✓ Checks` when every one passed | the check's page — its log — in the browser |
| **Reviews** | GitHub's review decision (`✓ Approved`, `✗ Changes requested`, `● Review required`), each reviewer's latest word, who has been asked and not answered, then the reviews themselves, then the review **Threads** file by file — each one's line and how it stands (`outdated`, `✓ resolved`, `2 pending`), who said what, a resolved one only its first words. The tab wears the decision's mark | the [PR review](#the-pr-review) |

In the pane the tabs are `Tab` / `⇧Tab` and a click on a label. In the modal `Tab` / `⇧Tab` hand
the keys between the list and the page, as the DIFF VIEWER's panels do: with the page holding them
`←` / `→` walk its tabs, `↑` / `↓` a listing's rows (or scroll prose), and `Enter` acts on the row;
from the list, whose letters type into its filter, the tabs are `⇧←` / `⇧→` and a listing's rows
`⇧↑` / `⇧↓`. `⌘E` and `⌘O` reach the row from either — the border names each by what it does there.

### Back to the grid

The grid is the top of the view, and `Esc` climbs back to it one level at a time from anywhere —
the footer (or the modal's border) always names the way:

| Where | The way back |
|---|---|
| a modal | `Esc` (a form backs out a step at a time); `^Q` closes it outright |
| a locked pane | `Esc` — `⇧Esc` is the agent's Esc |
| a full-screen session (`⤢`, `⌘F`) | `Esc` to its pane, `Esc` again to the grid; the `‹ sessions` crumb in its header |
| the pane, unlocked | `Esc`, or `←` with the pane beside the cards |
| the ARCHIVED VIEW | `Esc`, or `⇧A` |
| the PROJECT TABS | `Esc`, `Enter`, or `↓`,`↓` |
| HOME (`⌘G`) | `Esc`, `Enter`, an arrow, or a click anywhere |
| every tab closed | `Enter` opens the repo orion was started in; `⌘K` lists every project |

### The project tabs

| Key | Action |
|---|---|
| `↑`,`↑` on the first line | **the PROJECT TABS take the keys.** A cursor of its own lands on the lit tab; the card you left stays selected with the pane still on it |
| `←` / `→`, `[` / `]` | walk the header's cursor along the tabs — **the grid switches with it**, each project on the card you last left it on |
| `Enter`, `↓`,`↓` | hand the keys back to the cards of the project on screen |
| `Backspace` | close the tab under the cursor, behind a confirm (or click its `×`; **Close project tab** has no key by default — bind one in `config.json`'s `keybindings`) |
| `Esc`, a click, any other key | back down to the cards; any other key then means what it means on the grid |

A click on a tab opens it, a tab's `×` closes it, and the `+` in front of the tabs drops the
PROJECT DROPDOWN — every project on the machine, the ones waiting on you first, type to narrow.
Working in a project moves its tab to the far left; switching never reorders the tabs. A
right-click on a tab is the project's own menu: **New worktree**, **Start stack** / **Stop stack**, **Open**,
**Delete worktree** for a linked one, **Rename**, **Remove from list**.

## The jump list (`⌘K`)

Fuzzy jump across every live project, worktree and session on the machine. `↑`/`↓`
move, `Enter` jumps — the cursor on the row and the pane reading it — and `^O` opens a session
(attached, its pane focused). Until you type it is a recent sessions list in attention order — the
ones waiting on you first — so `⌘K` `Enter` is the fastest way back to what needs you. `Enter` on a
session that NEEDS FEEDBACK attaches and focuses its pane too. Typing reaches every row, projects
and worktrees too; the query runs
over each row's full `project/branch/session` path. Every open pull request orion has listed is a
row too, `↗ project/#42 title`, badged `draft`, `ready for review`, or `merge conflicts` /
`checks failing` in red; `Enter` on one reads it in the pane, and `^O` opens it in the browser.
So is each project's open Linear issues, `◇ project/ENG-12 title` — the last list read, or the one
kept from the last run — where `Enter` opens the Linear issues modal on it, and its open todos,
`☐ project/text`, where `Enter` opens the todos modal on it. Archived sessions are never rows here — their PTY is released, so there is nothing to jump to. The
last row opens a folder as a project.

With the pane reading a pull request — its [page](#the-pull-request-page) — `⌘E` shows its diff,
`⌘R` fetches it afresh, the COMMAND PALETTE's **Comment on pull request** comments on it, and
`PgUp`/`PgDn`, `Home`/`End` scroll it.

## Views

| View | Keys |
|---|---|
| **Changes** (`⌘E`) | The DIFF VIEWER: the branch's own commits over its changed files down the left, the selected file's diff on the right, wrapped — see [The diff viewer](#the-diff-viewer). `Esc` closes |
| **Go to file** (`⌘P`) | Fuzzy finder over the worktree. Until you type, a **Changed** section leads it — the checkout's changed files as its `git status` lists them, up to ten — then a **Recent** section: the last three files opened in that checkout — from here, find in files, the tree or a `⌘`-click in the pane — newest first, so `⌘P` `Enter` reopens the last one when nothing is changed; typing hides both. A line after the path — `app.rs:42` (or `:42:7`, `#L42`) — matches on the path and opens the file at that line. `Enter` opens the file in the BUILT-IN EDITOR (below) — a markdown file as its MARKDOWN PAGE, to read — `⌘C` (or `^Y`) copies the path — ready to paste into an agent — and `⌘O` opens the file in the **Open files in** editor (Cursor, VS Code, Sublime Text) at its line |
| **Find in files** (`⌘⇧F`) | `git grep` into the same modal, the hits under a header per file — its path and how many — each with its line number. Beside them, when the modal is wide enough, the selected hit's file is previewed around its line: numbered, highlighted, the line lifted and every match lit. The query is the text itself (smart case: an upper-case letter makes it match case) until `⌘R` (`^R`) reads it as a regex (`git grep -E`; the title says `(regex)`), and back. Text dragged over in the pane is the query it opens on, its first line, searched at once. `Enter` opens the hit at its line — a markdown file's page with the hit in view — a click on a file's header picks its first hit, and `⌘C`/`^Y` and `⌘O` work as in Go to file |
| **File tree** (`⌘B`) | Tree on the left, syntax-highlighted preview on the right, long lines wrapped (a markdown file as a rendered page; `⌘R` flips it to the source and back), and an always-live filter. `→` expands a directory and `←` collapses it; `Enter` folds a directory, and on a file edits it in the BUILT-IN EDITOR in the preview's place — a markdown file opens as its MARKDOWN PAGE over the tree. `⇧↑`/`⇧↓`, `PgUp`/`PgDn`, `Home`/`End` scroll the preview. `⌘C`/`^Y` copies the selected path, `⌘O` opens it in the **Open files in** editor, and dragging the tree/preview border resizes the tree. A changed file wears its `git status` letter at the row's end (`M`, `A`, `D`, `R`; `U` for one git doesn't track yet, `!` for one a merge left unresolved), and a folder with a change under it a dot — from the checkout's last two-second poll. `⌘⇧B` in the editor or the diff viewer opens the tree on the file it is on, its folders unfolded to it — the editor moves into the tree's preview pane, unsaved work and all — and `Esc` there goes back to what it was opened over |
| **GitHub pull requests** (`v`) | The project's open pull requests, newest first with the drafts below, in sections when there is more than one to show — **Yours**, **Review requested** (you were asked by name), **Others** — then **Merged**: the ones that landed in the last 7 days, the latest merge first, each with how long ago it merged; then **Closed**: the ones closed without merging in the last 30 days, the latest first, each saying why where GitHub does (`its base feat was deleted`, `closed by sam`). A merged or closed row is read, diffed, commented on and opened like any other; `Enter`, `⌘X`, `⌘;`, `⌘W`, `⌘D` and `⌘G` on a merged one only say it has merged, and `⌘X`, `⌘D` and `⌘G` on a closed one say it is closed — while `Enter` on a closed one opens **Edit pull request** at its reopen, the caret in its base, so one GitHub closed when its base branch was deleted is pointed at a live branch and reopened in one form. The title's count is the open ones'. The one under the cursor is read on the right as its [pull request page](#the-pull-request-page). Each row reads its meta under its title: who opened it (not on your own), how long ago, its checks as `passed/total` (`✓` all passed, `◐` running, `✗` failing), the review decision, its comments and its labels. The filter takes `key:value` tokens beside its words (see [Filtering a list](#filtering-a-list)): `author:`, `label:`, `review:` (`approved`, `changes`, `required`), `checks:` (`passing`, `failing`, `pending`) and `is:` (`draft`, `ready`, `conflicts`, `merged`, `closed`). `Tab` / `⇧Tab` move the keys between the list and the page: on the page `←`/`→` walk its tabs, `↑`/`↓` the rows of Changes, Commits and Checks, and `Enter` opens the row; from the list `⇧←`/`⇧→` and `⇧↑`/`⇧↓` do the same. Filters as you type — a letter typed on the page hands the keys back to the list — and `Esc` steps back off the page, then clears the filter, then closes. `⌘R` refreshes, `⌘Y` comments, `⌘E` opens its diff — at the file under the cursor on Changes, that commit's on Commits — and `⌘O` (or a double-click) opens it in the browser, or on Checks the check under the cursor. The DIFF VIEWER opened from here is a level inside the modal: `#42 ›` leads its title and `Esc` comes back to the pull request on the same tab. `⌘N` opens a **new pull request** in the page's place, in one of two modes along its top (`←`/`→` on that row, or a click): **Feature branch** — from the branch the cursor's checkout is on, into the project's base, each a list of branches to pick from as you type, the branch's commits listed, the title and description filled from them, a Draft box (`Space`) — where `Enter` pushes the branch and runs `gh pr create`; and **Promote** (`dev → main`) — the project's base into the repo's default branch, listing every pull request merged into the base that the default doesn't have yet, each with its Linear issues and author, and writing the title (`Release 5 Nov: …`, the first few titles) and a `## Going out` description, a line per pull request, from them — where `Enter` opens it from origin's branches without pushing anything, or, when a `dev → main` pull request is already open (the form says so), updates that one's title and description instead. Opened from the base branch itself — the root's band on `dev` — it starts in Promote. `⌘X` **merges** the one under the cursor: squash, merge commit or rebase (`←`/`→`, only what the repo allows), the branch deleted from GitHub once it lands, or auto-merge once its checks and reviews are in, or **Bypass** (`--admin`) to merge now past the branch's rules — the review you can't give your own PR included, for an admin on the ruleset's bypass list — with what stands in the way (a draft, conflicts, checks, a review owed) spelled out first; the method last merged with in this project is the one it opens on, and its delete-branch box starts ticked. `⌘;` opens **Edit pull request** in the page's place — all of it in one form: its title, its base (a retarget — the branches on origin listed to pick from as you type), draft or ready (`Space`), its labels (the repo's), its reviewers and its assignees (the people the repo can assign) — `Space` ticks one, typing narrows the list — and its state: `←`/`→` on **State** close it, with a comment and its branch deleted from GitHub if that is ticked (never a long-lived one, nor one with pull requests stacked on it), or reopen a closed one, with a comment. The **Saves** row spells out what `Enter` will send, and only that goes (`gh pr edit`, `gh pr ready`, `gh pr close`, `gh pr reopen`); refused, the form says where it stopped, reads the pull request again, and a second `Enter` sends what is left. GitHub won't retarget a closed pull request or reopen one whose base was deleted, so a reopen goes first, and a deleted base is put back on GitHub at the commit it last pointed at — and deleted again once the form has moved the pull request onto another base. `⌘W` **closes** it: the same form, open at its State row set to close. `⌘D` marks a draft ready for review, or a ready one a draft again. `⌘⇧R` (and `Enter` on the Reviews tab) opens it in the [PR review](#the-pr-review), a level inside the modal. `⌘G` opens the AUTOFIX MODAL for it — an agent sent at its merge conflicts and failing checks, the detected ones ticked (with Settings → General → **Auto-fix my PRs** on, the default, one is sent unasked instead). `⌘L` flips to Linear issues to attach this PR; opened from Linear's `⌘U` instead, the title names the marked issues (`Pull requests → ENG-12, ENG-15`), `Enter` attaches the PR under the cursor to them and goes back to Linear, and `Esc` goes back with the marks kept. Otherwise `Enter` on the list opens the QUICK PROMPT for a PR SESSION on it — the box's own `Tab` / `⇧Tab` pick a harness or an AGENT PRESET — in the project's checkout of the PR's head branch |
| **Linear issues** (`⌘L`) | Open Linear issues — asked for as the project is selected and kept from the last run, so the list is up at once and brought up to date underneath — in two tabs, `←`/`→` (or `⇧←`/`⇧→`, or a click) between them: **My issues**, assigned to you (`linear_assignee_email`, by hand; empty = the owner of the project's `LINEAR_API_KEY`), and **Other issues**, the rest of your teams' open ones — someone else's or nobody's, the 250 most recently updated. Under the open ones each tab lists what was **done in the last 7 days** — completed, not canceled, the latest finished first, up to 50 — so a week's work reads off the same list (`status:done` shows it alone). Each tab is grouped by status, a line per issue: its state, its priority as a letter (`U`rgent, `H`igh, `M`edium, `L`ow), identifier and title. The reading pane sets the issue out as Linear does — the title, then its status, priority, assignee, reporter (who filed it, or who asked for it through Slack or email), project, labels and dates in a column on the right (under the title when the pane is narrow), then the description, then its latest 20 comments, oldest first — read when the cursor rests on the issue, again on a rest a minute later, and given to an agent sent at it with the rest of the issue. Type to filter — a word matches a status, priority, label, project, assignee or reporter as well as the title (`high bug`) — with `status:`, `p:`/`priority:` (`urgent`, `high`, `medium`, `low`, `none`), `label:`, `project:`, `assignee:` and `reporter:` tokens (see [Filtering a list](#filtering-a-list)); `Space` marks; `Enter` starts one agent on the marked set in one worktree, with a task that asks for one PR; `⇧Tab` picks a preset. When a worktree is on one of them already — linked to it, named in its branch, or on its pull request's branch — `Enter` (and `⇧Tab`) offers that worktree instead, its sessions under it: `Enter` on it is **Go to session**, landing on its session as `⌘K` does, and only **New worktree anyway**, under them, cuts a second one. `⌘S` lists the issue's team states in the reading pane — `↑`/`↓`, `Enter` moves the issue there, the row saying so at once (a list already on its way can't put it back), moves on one issue going to Linear one at a time with the latest winning, and put back to what Linear last took if it refuses. `⌘P` (`^P`) sets its priority the same way — `1`–`4` urgent to low, `0` none, or `↑`/`↓` and `Enter` — and `⌘I` (`^G`) whose it is: the list opens on **Me**, so `⌘I` `Enter` assigns it to you and it moves to **My issues** at once, then **No assignee**, then the rest of its team. `⌘N` (`^N`) files a new issue: its title typed in the reading pane's place, into the team of the issue under the cursor (`Tab` for the next of your teams), and `Enter` files it in that team's **Triage** — its default state for a team with Triage off — the new issue on the list at once with the cursor on it. `⌘U` (`^V`) the other way: the pull requests modal opens to pick one, and `Enter` there attaches it to the marked issues (or the one under the cursor) — the footer says when Linear has — while `Esc` comes back with the marks kept. From a pull request, `⌘L` attaches the PR to the marked issues. The bottom border names only what fits a 120-column window — `Space` (or `↓` from the search line), `Enter`, `⌘S`, `⌘P`, `⌘I`, `⌘N` and `Esc`; the rest are here: `←`/`→` the tabs, `⌘F` the filter pick, `⇧↑`/`⇧↓` mark a run, `⌘↑`/`⌘↓` jump from status to status, `⌘C` (`^Y`) copies the marked issues as an agent is given them, `⌘.` (`^T`) links them to a worktree whose pull request will close them, `⌘U`, `⇧Tab`, `⌘O` the browser and `⌘R` refresh |
| **Todos** (`⌘I`) | Every open project's own todo list, kept on this machine and never in the repo (see [Sessions](sessions.md#todos)), a tab each along the top with how many are open in it, in the order of the grid's project tabs and then any project whose tab is closed: it opens on the selected project's tab, and `⇧←`/`⇧→` (`Tab`, or a click) walk them round the ends — `⌘1`–`⌘9` go straight to one, as on the grid — each coming back as it was left (cursor, filter) while the modal is up. Down the right is the **DONE** panel: your ticked todos across every project over **Today**, the **Week** (the one it opens on), the **Month** or the **Quarter** — the count, a bar a day (a bar a week for the quarter), then a section per project, the latest first, each with when. `→` puts the keys on it and `←` (or `Esc`) back; there `↑`/`↓` walk it, `Tab`/`⇧Tab` (or a click) turn the span and `Space` puts an item back on its list. The list is the groups, nested and folded with `Space` (or `Enter`) on a header. A top-level group is a section — its name in capitals, a rule across, and its open items at each priority (`U`rgent, `H`igh, `M`edium, `L`ow, the letters the Linear issues modal uses), how many are open and `✓N` ticked today, nested groups counted in; a group under it says only how many are open. `⌘↑`/`⌘↓` jump from group header to group header. Under each header a row per item — its box, its priority letter, the text, wrapped onto more lines when it is long, and on the right the agent sent at it and how many days it has carried over (`2d`). The order is the list's own; a nested group stands among its parent's items. `Space` ticks one: it strikes through and drops to the bottom of its group, and a group with everything ticked is complete and drops to the bottom of where it stands; once the modal closes, both are gone from the list (the DONE panel still has them). `⇧↑`/`⇧↓` select a run of items, across groups too, and the keys below act on all of them; `Esc` lets the selection go. `⌥↑`/`⌥↓` move the item (or selection) a place through the list as drawn, whatever its priority — into a nested group and through it, out of its bottom to just under it, then onto the start of the next top-level group (or the end of the one above). On a group's header `⌥↑`/`⌥↓` move the group — a nested one the same way, never into another, and no further than the end of the last top-level group (or the start of the first); a top-level one past the one beside it. Groups go two levels deep: a top-level group and the groups in it. `⌥→` on one item makes it a nested group of that name where it stood, its first item's field open in it (one an agent is on is kept as that first item), and in a new item's field `⌥→` at the end of the text does the same; on a selection it puts them in a new group where they stood, named at once (one deep: not inside a nested group). `⌥←` takes them out of a nested group to just under it, and on all its items flattens the group; on a group's header `⌥←`/`⌥→` leave the group at its level (Ghostty sends `⌥←`/`⌥→` as `⌥B`/`⌥F`; both work). `⌘X` (`^X`) cuts and `⌘C` (`^Y`) copies them — their text on the clipboard, a line each — and pasting that text back into the modal, in any project's tab, puts the items themselves right after the cursor — or where a new item's empty field is — priority kept (a cut's first paste moves them; any paste after is copies). `Enter` opens the QUICK PROMPT over the modal for an agent on the item — its text and group in the box, a fresh worktree; on a selection, one agent with every item listed — and `⇧Tab` an AGENT PRESET; once its session is up the row shows `● agent`, and `Enter` (or a click on the chip) jumps to it; the session's own row on the grid names the todo, a dim `☐ text` after its prompt, and when its pull request is seen to merge the item is ticked. A row is edited where it stands: typing on an item or a group header adds to its end, `⌫` opens it with its last character gone, and in the field — which wraps as you type — `Enter` saves, `↑`/`↓` save and move on, `Esc` puts it back; typed on `+ new item` it starts a new item. `⌘N` (`^N`) types a new item right under the cursor's, or at the end of the cursor's group from a header — `Enter` adds it and opens the next under it, `Esc` stops — `⌘⇧N` (`^⇧N`) a new group — nested right under the cursor's row when it is in a top-level group, a new top-level group from a top-level header or `+ new item`, its first item's field open either way — `⌘P` (`^P`) picks the priority — `1`–`4` urgent to low, `0` none, or `↑`/`↓` and `Enter` — and `⌘⌫` (`^W`) deletes the item, the selection, or a group, a group with items in it after a second `⌘⌫` or `Enter`. `⌘F` (`^F`) opens the filter: the items that match stay, under their headers, and `Esc` clears it and puts it away. Paste several lines and each is an item, bullet and task box taken off, right after the cursor's item (or, in a new item's field, from where the field stands). A single line pasted goes on the end of the row, or into the filter while it is up. `Esc` closes |
| **My week in review** (`⌘⇧Y`) | Finished work in the selected project, written up to read aloud with the product on screen, on a screen of its own. **New review**, down the left: `←`/`→` on **Whose** picks *Just mine* or *Everyone* (each point then names its owner by initials), on **Where** *This project* or *All projects*, on **Range** a *Week* (the default), *2 weeks* or a *Month* back from today; `Space` ticks a source in or out — merged pull requests, Linear issues done (Linear's list holds a week of them, which its row says on a longer range), todos ticked (yours alone, so left out of *Everyone*); the **Note** is sent as typed and kept for the project; under them, exactly what would be sent. `Enter` writes it: one ask to the model the **Week review** system preset names (sonnet · medium unless you change it), on the rules that preset holds, with no tools, through a Claude account orion runs — the preset's, else your default agent's, named on the **Via** line — so no session starts and nothing runs in a checkout. It turns to a fresh page that shows it being written and what went in; usually under a minute later the review is on that page, and the window can be closed meanwhile (`⌘W` stops it; the next open lands on the review once it is in). **Reviews**, down the right: the one being written, then the ones kept, newest first (the last 12 per project, and 12 of all projects) — each named for the week it ends in (`Week 45 · mine`, `Weeks 43–45`, `Month to 5 Nov`), `new` until read. `Tab` moves between the two sides; on the list `↑`/`↓` pick one and `Enter` (or a click) opens it on its page. The page is the review by product area, each point numbered with its owner and pull request at the right and a green **Show** (what to demo) and a **Say** (the fact worth saying) under the points that have one; `↑`/`↓` and `PgUp`/`PgDn` scroll it, `⌘C` copies it as Markdown, `⌘O` opens the file, `⌘R` writes it again, `⌘N` goes to a new review and `Esc` back to the screen. The last line counts what was merged and not listed, and a line under the title says so when the reply named a pull request that was not in the range or a wrong owner |
| **Autofix** *(`⌘G` on a pull request, in the modal or the pane)* | A pull request with merge conflicts or failing checks: the PR, what broke, and a checklist — **Merge conflicts**, naming the files that conflict as the repo merges the base in, and **Failing checks**, listed under the workflow each ran in — the detected ones ticked, then a **Note** line. The prompt lists the checks the same way, and which is a unit test, an e2e spec or a lint is the agent's to read off their logs, under the PR autofix preset's rules for each. `↑`/`↓` walk the rows, `Space` ticks, `Enter` sends an `autofix-<n>` session to the PR's worktree, and `Esc` is not now — it remembers nothing, so **Auto-fix my PRs** still sends one for that breakage. Opened over the pull requests modal, `Esc` and `Enter` both go back to it; from the pane's pull request page, back to the page |
| **File tabs** *(an agent opens it)* | What `orion open <file>…` raises when a session runs it. One tab per file, at the line `orion open file:42` gave it — its label says `:42`, the preview opens with the line in view, and `Enter` and `⌘O` open the file there: `←`/`→`, `Tab`/`⇧Tab` and `1`-`9` switch tabs; the focused file is previewed underneath — a markdown file as a rendered page (`m` flips it to the source). `Enter` edits the file in that pane. `↓` from the strip drops into the preview, where `↑`/`↓`, `PgUp`/`PgDn` and `Home`/`End` scroll and `↑` off the top returns to the strip. `Esc` and `^Q` step back to the strip first, and close from the strip |

### Modal verbs

A list modal's letters type into its filter, so its verbs are ⌘ chords — one per verb, the same
in every modal that has it, and the grid's own letter wherever the grid does the same thing. Each
also answers to the same letter's `^` chord; the border shows the ⌘ one. `↑`/`↓` walk the rows and `PgUp`/`PgDn` read — no `^N`/`^P` or `^D`/`^U`.

| Key | Verb | Where |
|---|---|---|
| `⌘N` | new | pull requests (a new pull request), agent presets, todos (an item; `⌘⇧N` a group) |
| `⌘E` | changes | pull requests (its diff) — the grid's Changes |
| `⌘I` | edit (`^E` with no ⌘ — `^I` is Tab there) | agent presets |
| `⌘R` | ask again; the query as a regex | pull requests, Linear; find in files |
| `⌘O` | open outside orion | pull requests (the browser), Linear and the finders (**Open files in**) |
| `⌘⇧B` | the file in the **File tree** | the editor, the diff viewer |
| `⌘⇧I` | `path:line` into the checkout's agent's prompt | the editor |
| `⌘Y` | comment | pull requests — the grid's `y` |
| `⌘W` | close / remove, behind a confirm | pull requests (close without merging — the edit form, at its State row), agent presets (delete) |
| `⌘;` | edit, every field in one form | pull requests (title, base, draft, labels, reviewers, assignees, close or reopen) |
| `⌘X` | merge (a selection in the filter cuts first); cut | pull requests; todos (the item or selection) |
| `⌘D` | ready for review ⇄ draft | pull requests |
| `⌘G` | autofix: an agent at its conflicts and failing checks | pull requests, and the pane's pull request page |
| `⌘L` / `⌘U` | Linear / pull requests, to attach one to the other | pull requests / Linear |
| `⌘S` | status | Linear |
| `⌘I` | assign (`^G` with no ⌘ — `^I` is Tab there) | Linear |
| `⌘F` | filter — the FILTER PICK | pull requests, Linear; todos (the filter row) |
| `⌘⌫` | delete, a group with items behind a second press (`^W` with no ⌘) | todos — the grid's delete-worktree key |
| `⌘↑` / `⌘↓` | the previous / next group | todos |
| `⌘1`–`⌘9` | project tab 1–9 — the grid's own | todos |
| `⌘P` | priority, in a pick (`1`–`4`, `0` none) | todos, Linear |
| `⌥↑` / `⌥↓` | move the item, selection or group through the list, into and out of nested groups | todos |
| `⌥→` / `⌥←` | group the items (one item: it becomes the group) / take them out of their group; on a header, nest the group / lift it out to the top level | todos |
| `⇧↑` / `⇧↓` | select a run of items | todos |

### Filtering a list

The pull requests and Linear lists filter by facet as well as by fuzzy text, in the same filter
line. A `key:value` token narrows the rows to those carrying that value — `label:bug`, `p:high`,
`checks:failing` — and is lit in the line. Keys and values ignore case, a value matches the start
of the row's value or of any word in it (`p:hi` is High, `label:pdf` is `Export PDF`), and `-`
stands for a space (`status:in-progress`, or quote it: `status:"in progress"`). Two tokens of one
key are either/or, tokens of different keys must all hold, and a leading `-` leaves rows out
(`-label:wip`). Everything else in the line is fuzzy text, and a word that names one of the row's values — its
status, priority, a label — finds it too, so `high bug` needs no keys.

`⌘F` opens the **FILTER PICK** in the reading pane's place: one column, each facet's name over its
values with how many rows carry each. `↑`/`↓` walk every value, `←`/`→` jump a facet, and `Space`
adds the value's token to the filter line — or takes it out — the rows narrowing behind it. `Enter` or `Esc` puts
it away with the filter kept.

The DIFF VIEWER keeps its own: `⌘A` all, `⌘R` reviewed, `⌘B` tree
([The diff viewer](#the-diff-viewer)). orion releases every one of these from Ghostty along with
the grid's ⌘ chords. `⌘V` stays Ghostty's, which is what pastes the clipboard into a field.

### The built-in editor

Every file opens to be edited, in a modal over everything else, in **fresh**, whose keys are VS
Code's: `^S` save, `^Z` undo, `^C`/`^V` copy and paste, `^F` find, `^Q` quit after asking to save.
`ORION_EDITOR` names any other command instead, opened as `<command> +<line> <file>`. With fresh
not installed the footer says so and names its install, and `i` runs it (`brew install
fresh-editor`; [Installing editors and agent CLIs](configuration.md#installing-editors-and-agent-clis)).
Long lines wrap.

The modal hands the editor every key, and the mouse — a click places the cursor, a drag selects,
the wheel scrolls — with these exceptions:

- `^\` — force-closes the modal, in any state, without saving.
- `^q` — fresh's own quit, which asks to save first. With a command `ORION_EDITOR` names, `^q`
  force-closes the modal instead.
- `⌘⇧B` — the file in the **File tree**, the editor moved into its preview pane.
- `⌘⇧I` — where the cursor is, `src/app.rs:42`, typed into the prompt of the checkout's agent
  (the one in the pane when it is this checkout's, else its running one the list puts first),
  unsent, with a space either side; the editor stays up and the footer says where it went. Never
  into a permission question or a dialog, where the digits would answer it.
- `⌘O` — the same file in the **Open files in** editor, at the line fresh's cursor is on (read off
  its status bar's `Ln 42, Col 7`; line 1 if the bar can't be read — with a command `ORION_EDITOR`
  names, the line it opened on); the editor stays up.

In fresh a `⌘` chord that reaches orion is fresh's `^` chord, as VS Code has it on a Mac: `⌘S`
saves, `⌘Z` undoes, `⌘⇧Z` redoes, `⌘F` finds, `⌘A` selects all, `⌘D` adds the next match as another
cursor. orion's own `⌘` keys — `⌘L`, `⌘/`, `⌘P`, `⌘K` — are fresh's while it is up. A Mac's
text-editing chords mean what they mean in VS Code, each typed as the key fresh binds that action
to:

| Key | Action |
|---|---|
| `⌘←` `⌘→` (and `^A` `^E`) | line start / end |
| `⇧⌘←` `⇧⌘→` | select to the line's start / end |
| `⌘↑` `⌘↓` | file start / end |
| `⇧⌘↑` `⇧⌘↓` | select to the file's start / end |
| `⌥←` `⌥→` | word left / right |
| `⇧⌥←` `⇧⌥→` | select a word left / right |
| `⌥⌘↑` `⌥⌘↓` | add a cursor above / below |
| `⌘D` | add the next match as a cursor |
| `⌘⇧L` | a cursor on every match |
| `⌘L` | select the line |
| `⌘/` | toggle comment |
| `⌘⇧P` | command palette |
| `⌥↑` `⌥↓` | move the line up / down |
| `Esc` | back to one cursor |

- **`⌘⇧L` is `⌘D` pressed 500 times at once.** fresh has no select-all-matches of its own; it stops
  adding cursors at the last match, so the presses past it do nothing. The first press takes the
  word under the cursor when nothing is selected, as `⌘D` does.
- **`Enter` after a find steps through the matches.** The `Enter` that runs the find puts the
  cursor on the first match; each `Enter` straight after it moves to the next one and `⇧Enter` to
  the one before (fresh's `F3` and `⇧F3`), nothing edited. Any other key, a click or a paste ends
  that, and `Enter` is a new line again. It reads fresh's `Search:` prompt off the screen, so under
  another fresh locale `Enter` stays fresh's own.
- **Ghostty keeps `⌘←`/`⌘→` and `⌥←`/`⌥→`** and types the shell's keys for them — `^A`/`^E`, `⎋b`/`⎋f`
  — so in the editor a bare `^A` and `^E` are the line's start and end, as in any Mac text field.
- **Not Ghostty's in Orion.app** ([Configuration](configuration.md#ghosttys-keys-in-orionapp)):
  `⌘↑`/`⌘↓` and `⇧⌘↑`/`⇧⌘↓` — Ghostty's jump to prompt — and `⌥⌘↑`/`⌥⌘↓`, `⌥⌘←`/`⌥⌘→` — its
  splits — all reach the editor there.
- **Not there:** a command `ORION_EDITOR` names gets every key as it is typed.

fresh runs off orion's own config file (`<data dir>/fresh/config.json`) — the one file, no menu,
tabs, explorer or dock, a two-item status bar, orion's own theme — never your
`~/.config/fresh/config.json` ([Configuration](configuration.md)).

### The markdown page

A **markdown file** opened from Go to file, find in files, the tree or a `⌘`-click opens as its
MARKDOWN PAGE: the file rendered, full width and wrapped, in a modal of its own. It is read again
when the file changes on disk (a look every two seconds), so an agent's edit shows while you read.

| Key | Action |
|---|---|
| `↑`/`↓`, `PgUp`/`PgDn`, `Space`, `Home`/`End`, the wheel | scroll |
| `Enter`, `e` | edit it in the BUILT-IN EDITOR at the source line of the row at the top of the page — at the line it was opened on (a find-in-files hit's) until it scrolls; quitting the editor lands back on the page, re-read |
| `Tab`/`⇧Tab` | the next or previous link, highlighted, scrolled into view; `Enter` then follows it (`e` still edits) |
| a click on a link | follow it: a relative `.md` link opens that page here (`#heading` brings the heading into view; `/` is the checkout's top), an `http` link opens in the browser, any other file in the BUILT-IN EDITOR at its `#L42` line |
| `⌘O` (`^o`) | open it in the **Open files in** editor, at the line `Enter` would edit |
| drag | select text and copy it on release; past the top or bottom edge the page scrolls under the pointer |
| double-click | select and copy the word under the pointer |
| `⌘C` (`^y`) | copy the selection, or its path when nothing is selected |
| `Esc`, `q` | let go of the link `Tab` is on; else back to the page a link left, scrolled where it was; else close |
| a click outside it | close it |

A selection stays highlighted until the next click or key, and the wheel keeps it. A copied
paragraph reads as the sentence it is: rows the page only wrapped for width are joined back up
with a space, and their indent or quote bar is left out.

The FILE TABS show a markdown file's page in their own preview already, so
their `Enter` edits it. The FILE TABS' and the file tree's rendered previews select the same way:
drag or double-click, and `⌘C` copies the selection.

### The diff viewer

`⌘E` reads the checkout under the cursor in one modal: the COMMIT LIST over the changed files down
the left, the selected file's diff on the right at the modal's full height. The panel with the keys
wears the accent, and `Tab` walks them in reading order — commits, files, diff.

- **The pull request heads it.** When the branch has a pull request, a row over the panels names
  it and where it stands: `#1342 Fix PDF header · ✗ 44/45 checks · 3 files conflict with
  origin/dev` — the checks' tally once its page has been read (`◐` while one runs, `✓` all passed),
  the one word GitHub's list gave before that, and `merged` or `closed` once it is past taking work.
  A branch with no pull request has no row unless something conflicts, when the row says only that.
- **Conflicts are worked out here.** As the viewer opens, git merges the branch with its base in
  memory (`git merge-tree`, nothing touched, nothing fetched — base sync keeps the base fresh), so
  it works before there is a pull request. The files that would conflict wear git's red `U` in the
  file list, and `Enter` on one opens the CONFLICT RESOLVER on the checkout, at that file — or, with
  an `autofix-<n>` already on the pull request, the footer says `autofix-<n> is on it` and leaves it
  to that agent. `Esc` in the resolver comes back here.

- **The commits are what the branch added.** Its own commits since the base it was cut from, newest
  first — `git log --first-parent --no-merges <merge-base>..HEAD` — so a merge of `main` into it,
  and every commit that came in with that merge, never show. **Uncommitted changes** sits on top
  while the checkout is dirty. Each row has a box, `[✓]` ticked and `[ ]` not; its subject wraps
  under the box, its short sha, age and `+added −removed` on the line below.
- **What the diff shows.** With nothing ticked, the row under the cursor. Tick rows and they read
  TOGETHER, as one diff. The rows on screen are the bold ones, and the panel's foot says `3 ticked · together`. A
  clean checkout opens with every commit ticked — the whole branch, what its pull request shows; a
  dirty one on its uncommitted changes, nothing ticked. Nothing carries over to the next `⌘E`.
- **How it opens:** the keys on the commits, so the ticks come first and `Tab` moves on to the
  files, then the diff; and the files flat or as a directory tree, the way `⌘B` last left them.
- **Ticked with a gap.** Ticked rows side by side on the branch read as one range. An unticked
  commit, or a merge, between two ticked ones starts another range, and a file both ranges touch
  shows each range's diff in turn under its `── 1ec007c..dde6cc9` label — never a diff across the
  gap, which would put the left-out commit back in. A run reaching down to the branch's first
  commit stays one range across the merges inside it.
- **The diff reads like a review, not a pager.** git's headers become the pane's top edge — the
  path, `added`, `renamed from …`, `+20 −3` — and each hunk a `line 12  fn main()` marker. Every
  line keeps its old and new numbers and its `+`/`−` in a gutter of its own, its code in the file
  language's colours on a green or red tint, and wraps at the pane's edge — a long line continues
  under its code, on a whole word where one fits. Nothing scrolls sideways. A commit's message heads
  the first file it opens on; later files open past it, a scroll up. One dim line at the diff's foot
  says what is on screen.

| Key | Where | Action |
|---|---|---|
| `Tab` / `⇧Tab` | anywhere | the next panel / the one before |
| `↑` / `↓` | the commits, the files | walk the list; with nothing ticked each commit goes up as the cursor lands on it |
| `Space` | the commits | tick the row under the cursor, or untick it |
| `⌘A` | the commits, or the files with nothing typed | tick every commit — or, when they all are, none |
| `⇧←` / `⇧→` | anywhere | older / newer: the cursor's commit, with nothing ticked |
| `Enter` | the commits / a file / a `U` file / a tree directory | on to the files / the diff takes the keys / the CONFLICT RESOLVER at that file / fold it |
| `↑` / `↓`, `Space`, `←` | the diff | a row, a page, back to the files |
| `Enter` | the diff | edit the file in the built-in editor (fresh) at the line at the top of the diff — a pull request's in the project's checkout of its branch, when there is one. When the editor closes the diff is read again, your place in it kept |
| `⇧↑` / `⇧↓`, `PgUp` / `PgDn`, `Home` / `End` | anywhere | scroll the diff |
| typing | anywhere | the files' fuzzy filter, which takes the keys; `^U` kills it |
| `⌘R` | the files, the diff | mark the file reviewed ✓, which sinks it to the bottom — orion-side bookkeeping only: on the uncommitted changes it is stored until HEAD moves or the file changes again, on anything else it lasts while the modal is open |
| `⌘B` | the files | fold the file list into a directory tree and back (`←`/`→` fold) — and the next viewer opens the same way |
| `⌘O` (`^O`) | the files, the diff | the file in the **Open files in** editor, at the line at the top of the diff |
| `⌘⇧B` | the files, the diff | the file in the **File tree**, its folders unfolded to it; `Esc` there comes back here |
| `⌘U` | anywhere | the PULL REQUESTS MODAL on this branch's pull request page (as the hotkey opens it, when there is none); its last `Esc` comes back here. On a pull request's viewer the modal opened, back up to the modal |
| the wheel | over any panel | scrolls that panel; no cursor moves |
| a click | | a commit's row aims the cursor there and its box ticks it, a file is selected, the diff takes the keys; a second click on the cursor's row is `Enter` |
| `Esc` | | close — or, opened from the PULL REQUESTS MODAL, back to the pull request |

With the pane reading a pull request, `⌘E` — and the PULL REQUESTS MODAL's `⌘E` and its Changes and
Commits rows — open the same viewer on that pull request, read from git: its commits over the
files, every one ticked — the whole pull request — to read a commit or a few together like a
checkout's, opened on the file or the commit you picked. A head the repo lacks (someone else's
branch, a fork's) is fetched by its pull ref first.

### The PR review

`⌘⇧R` in the PULL REQUESTS MODAL — or `Enter` on a pull request page's Reviews tab, in the modal or
the pane — reviews the pull request in orion; opened from the modal it is a level inside it. On the left, the files it changes, each with how
many review threads are on it (`✓2` when all of them are resolved, a `●` while one holds a pending
comment of yours); on the right, the selected file's diff, read from the repo as the diff viewer
reads a pull request, with every thread a block under the line it is on: the other reviewers'
comments on a grey rule, yours on the accent. A resolved thread folds to one line until the cursor
rests on it. A thread GitHub no longer places — a push moved its code, or it is on the whole file —
sits at the top of its file, named by the line it was written on and marked `outdated`; a file a
thread names that the pull request no longer changes keeps a row of its own.

What you write goes into GitHub's own pending review: a comment or a reply shows at once, marked
`pending`, and nobody else sees it until the review is submitted. Leaving the screen keeps it — the
pending review is GitHub's, there on the next visit here or on github.com. A comment GitHub refused
stays in its box with what you typed, the footer saying why.

The bar along the bottom, **Your review**, holds the verdict — **Comment**, **Approve** or
**Request changes**; on a pull request you opened yourself GitHub takes only a comment, and the other
two are greyed out — how many comments are pending, and an optional **Summary** (carried on from
the pending review's own, when one was begun on github.com). `⌘⇧R` from anywhere — or `Enter` on the
bar — sends the review with everything pending in it; with nothing pending it is sent at once as a
review of its own. A comment or a change request with nothing pending needs a summary, and the bar
says so rather than sending it. Once GitHub takes it the footer says `approved #42` (or
`requested changes on #42`, `reviewed #42`) and the screen gives way to the pull request's Reviews
tab, its page read again; refused, the bar says why and everything stays pending.

| Key | Where | Action |
|---|---|---|
| `Tab` / `⇧Tab` | anywhere | the files, the diff, the bar, round |
| `↑` / `↓`, `Home` / `End` | the files | pick a file; its diff beside them |
| `Enter`, `→` | the files | the diff takes the keys |
| `↑` / `↓`, `PgUp` / `PgDn`, `Home` / `End` | the diff | the cursor, from line to line and thread to thread |
| `←` | the diff | back to the files |
| `c` | a line | a comment on it, written in place under the line (after the threads already there) |
| `⇧↑` / `⇧↓` | a line | mark a run of lines, inside one hunk, for the next `c`; `Esc` lets them go |
| `Enter`, `⇧Enter` | the comment | add it to the pending review; a new line |
| `Esc` | the comment | put it away |
| `r` | a thread | a reply at its foot, held pending the same way (`c` on a thread replies too) |
| `x` | a thread | resolve it, or open a resolved one again — at once, not held |
| `⌫` | a thread | drop your last pending comment in it; the thread goes with it when it was all there was |
| `]` / `[` | the files, the diff | the next thread / the one before, file to file, round either end |
| `←` / `→`, `Space` | the bar's verdict | the next verdict GitHub takes; `↓` (or typing) to the summary |
| typing, `⇧Enter` | the bar's summary | the summary; a new line; `↑` off its top back to the verdict |
| `⌘⇧R`; `Enter` on the bar | anywhere | submit the review |
| `⌘R` | anywhere | ask GitHub for the threads again |
| the wheel, a click | | the wheel scrolls the diff; a click picks a file, puts the cursor on the line or thread under it, or picks a verdict |
| `Esc` | | from the bar, back to the diff; else back to the pull request (or, opened from the pane, the grid) |

## The quick prompt

`⌘N` opens a wrapped, multi-row task box that starts a new agent in the checkout under the cursor
with what you typed as its first prompt. Which CLI it launches is **New agents on** in
Settings → Accounts. `Enter` launches; `Enter` on the box empty starts the session bare.
`Esc`, a click outside it and `^Q` park what you typed, and the next `⌘N` at the same place opens
on it. Each worktree keeps its own draft, and so does each project's fresh-worktree box, so a draft
you typed for one checkout never turns up in another's box.

What you type is also saved as you type it, per place, to `quick_prompt_drafts.json` in the DATA
DIR, so closing the terminal window with the box up loses nothing: the next `⌘N` there after a
restart opens on the draft, caret at its end, its dim explanation line starting `draft restored ·` until you edit it.
Launching it, or emptying the box, deletes that place's draft. A box that brings its own text — a launch
the daemon refused, handed back — never takes the draft or overwrites it.

Its header names everything the launch is made of, each field beside the key that changes it — where
it runs over what runs it:

```
project demo ⌘P   worktree main ⌘.
agent   Claude (y… Tab   mode plan ⇧Tab   model opus · latest ⌘/   effort high ⌘Y
```

The agent's name is cut to ten columns, so a Claude account's long label leaves the row room. The
`mode` field is there for a harness that can start somewhere other than edit: Claude offers edit and
plan (`--permission-mode plan`), Cursor edit, plan and ask (`--mode plan` / `--mode ask`). Every box
opens on edit; plan and ask read in colour, so a launch that will not touch the code is
never mistaken for one that will. The mode is where the session starts — the CLI's own toggle moves it
from there, and a resume never forces it back.

The effort is always there — `default` until one is picked — for every harness that has one
(OpenCode has none, so its box has no effort field); an AGENT PRESET on the launch adds a
`preset reviewer ⌘U` field, and a fresh worktree reads `new worktree <branch>`, in the green the
frame turns. The worktree, model and effort keys are spelled from the live keymap, so a rebind shows
there at once; on a narrow screen the fields wrap onto more rows rather than lose a key. What `Enter`
sends is the dim line along the bottom of the frame.

| Key | Action |
|---|---|
| `⇧Enter`, `⌥Enter`, `^J` | insert a line |
| `^V` | paste the image on the clipboard (a ⌃⇧⌘4 screenshot, a copied image): it is kept in the DATA DIR's `attachments/` and its path goes in at the caret — [Dropping a screenshot on a prompt box](sessions.md#dropping-a-screenshot-on-a-prompt-box). `⌘V` is still Ghostty's text paste |
| `Tab` | pick a different harness for this one launch (`→` drills into its model and effort) |
| `⇧Tab` | step the mode — edit, plan, ask — among the ones the harness has, as in Claude Code and Cursor; for this box only |
| `⌘U` (`^X`) | pick one of your saved AGENT PRESETS, adopting its harness, model, effort and prefix/postfix |
| `⌘P` (`^P`) | the PROJECT PICKER, over the box: aim it at any project on the machine, the text kept. A launch into another project runs in the background, and the footer names where it went |
| `⌘.` (`^T`) | the WORKTREE PICKER: **+ new worktree** first, then every checkout of the project, then every branch with no checkout yet (`⎇`) — local ones, then origin's — narrowed as you type; picking one launches in a new worktree on that branch |
| `⌘/` (`^/`) | the harness's model list |
| `⌘Y` (`^Y`) | step the effort, the header showing the step at once — `⇧⌘/` too, where macOS lets it through |
| `@` | list the checkout's files under the caret, narrowed by what follows the `@`: `↑`/`↓` move, `Tab` or `Enter` writes `@path/to/file` in, `Esc` puts the list away for that `@` |
| a click on a header field | the picker its key opens; on `effort`, the model's effort list; on `mode`, the next mode |

`⌘.`, `⌘/` and `⌘Y` work the same with one of the box's pickers open over it — the model list,
the worktree list, the harness or preset picker — and none of them closes the box: the model and
worktree keys swap the picker in front, the effort key hands the box back stepped.

A new worktree's branch is your sentence slugified (`fix login redirect` → `fix-login-redirect`),
or a random `<adj>-<noun>-<verb>` when empty, started at the freshly fetched `origin/HEAD` — or
the project's **Base branch** (Settings → Projects). The project's ignored `.env` files are linked into it from
the main checkout.

A `⎇` row checks out a branch that already exists instead — someone else's work, or yours from
before. The header reads `checkout <branch>`. `Enter` fetches origin first, so a branch pushed a
minute ago is found. A local branch is checked out as it is, fast-forwarded to origin's copy when it
is only behind. A branch only origin has becomes a local branch tracking it, so `p` and `⇧P` talk to
it. The list is the project's root's last listing of its branches. Opening the picker
lists the branches again and, at most once a minute, fetches every remote in the background. Each
answer that lands while the picker is up rebuilds it, keeping what you typed. Only the first dozen
branches show at once; type to reach the rest. A **New worktree** you type the exact name of a
branch only origin has into is that branch, too, never a fresh one cut over it.

## Menus and lists

The COMMAND PALETTE, the OPEN MENU, a right-click menu, help, the ssh hosts list
and the project pickers are all lists: type to filter where they filter, `↑`/`↓` move, `Enter` or
a click picks, `→` / `←` step into and back out of a submenu, and `Esc` or a click outside closes
the whole thing. Each list's keys are on its bottom border — on a harness `? settings` — and the
COMMAND PALETTE says what the row under its
cursor does on the line above them.

In the SETTINGS OVERLAY, `Tab`/`⇧Tab`, `[`/`]` and the digits move between tabs, `↑`/`↓` move down
the rows, and `←`/`→` cycle the selected row's value. The HOTKEYS TAB lists every action and its
chord, with nothing to edit: a key on one of its rows says that a binding changes in `config.json`'s
`keybindings`. `R` resets every setting, the bindings included (with a confirmation). On the
Accounts tab's **Claude accounts** rows, `Enter` signs the account in (asking for the email first),
`r` renames it, `o` signs it out, `←`/`→` switch it on or off and `⌫` removes an added one; `Enter`
on **Add account** names a new one ([Configuration](configuration.md#claude-accounts)). On a row whose
program isn't on PATH — a Claude account, a harness's **Enabled** row —
`i` puts the command that installs it where the row's explanation was, `Enter` runs it in the editor
modal and anything else leaves it unrun ([Installing editors and agent
CLIs](configuration.md#installing-editors-and-agent-clis)). The installer's own output is the
modal: `^Q` stops it, and when it exits the title says whether the program is on PATH now and
`Enter` closes it.

FIRST-RUN SETUP (**Orion setup**, while `onboarded` is unset, and from **Run setup** or `orion
setup`) is the same kind of modal, four pages — Welcome · Claude · First project · Ready — the STEP
STRIP across its top lighting the one it is on. `→`/`Tab` go to the next page, `←`/`⇧Tab` back,
`↑`/`↓` move down a page's rows. On **Claude**, `i` installs the Claude CLI while it is missing,
`Enter` signs an account in (and switches it on) or adds one, and `Space` switches an account on or
off. On **First project**, `Enter` on the Project row opens the repo orion was started in (or the
open-project prompt), `Space` steps to the next project, and `Enter` on **Base branch**, **Setup
command** or **Run command** types it in place. **Ready** lists the keys that matter. `Esc` skips the rest — or, while
a page asks something, backs out of the question.

WHAT'S NEW is a sheet of its own, up once on the first launch after an upgrade: every release since
the one this machine last ran, newest first. `↑`/`↓` walk the releases; `Enter` or `Esc` closes it.

## Typed fields

| Where | Key | Action |
|---|---|---|
| Any typed field | `←→`/`⌥←→`, `^A`/`^E`, `⌥⌫`, `^U`/`^K` | every prompt, filter and query is the same line editor: move by character / word, jump to ends, delete word, kill line |
| Any typed field | `⇧←`/`⇧→`, `⌥⇧←`/`⌥⇧→`, `⌘⇧←`/`⌘⇧→` (`⇧Home`/`⇧End`), `⌘A` | select, as a macOS text field does: by character, by word, to the line's start or end, everything. The same key without `⇧` lets the selection go — `←`/`→` land on its edge — and typing, a paste, a line break, `⌫` or any delete replace or remove just the selection. It draws on the theme's selection background. `⌘C` copies the selection and `⌘X` cuts it, to the system clipboard (over `orion ssh`, the near terminal's, by OSC 52); with nothing selected they are the modal's own — Go to file's `⌘C` still copies the path. `⌘A` reaches orion in [Orion.app](configuration.md#ghosttys-keys-in-orionapp), which clears Ghostty's own select-all; `^A` stays the line's start (Ghostty types it for `⌘←`) |
| Any multi-row box | `↑`/`↓`, `⌥↑`/`⌥↓`, `^Home`/`^End`, `PgUp`/`PgDn`, wheel, click | the quick prompt, the task and comment boxes, a preset's prefix and postfix: `↑`/`↓` move a row and keep the column; past the top or bottom a form steps to its next field. `⌥↑`/`⌥↓` jump by paragraph, `^Home`/`^End` (`⌘↑`/`⌘↓` where the terminal passes ⌘ on) to the start or end of the text, `PgUp`/`PgDn` a boxful. `↑ 3 more` / `↓ 5 more` on the border say what is out of sight. With `⇧` they select: `⇧↑`/`⇧↓` by row (on the first or last row, on to the start or end), `⌘⇧↑`/`⌘⇧↓` to the ends of the text, `⇧PgUp`/`⇧PgDn` a boxful |

## Chips and readouts

The TERMINAL PANE's header shows one chip at a time — the first of these that applies:

| Chip | What it is telling you |
|---|---|
| `exited` (dim) | the session's process is gone. `Esc`, `Enter` and `q` leave the lock and everything else falls through to the grid; the next attach respawns it |
| `scroll N` (muted) | you are N lines back in the scrollback. Typing anything, or scrolling back down, clears it |
| `starting…` (dim) | nothing has come off the PTY yet: the session's CLI is booting |
| `INPUT` (accent) | the pane is locked and every key is going to the PTY; `Esc` leaves |

The FOOTER carries the rest, left to right:

| Readout | What it is telling you |
|---|---|
| `vX.Y.Z` at the far left | which orion this is — the version `orion --version` prints. A button: a click goes HOME, and from HOME back to the grid |
| `⇡ v0.22.0` after it, in the warning color | a newer orion is published on GitHub — run `orion upgrade`. `ORION_UPDATE_CHECK_SECS=0` turns the check off |
| `✗ disconnected` | this client has lost the DAEMON. A healthy connection says nothing |
| key hints | what the keys do where you are, built from your live keymap — a rebind shows up here, and an unbound action drops out. None while a modal is up: its keys are on its own border |
| `demo ⎇ feat ◐ fix-login  · archived` after it | where you are, in the marks the grid draws it with: the project, the checkout behind its SCOPE MARK (`⌂` the root, `⎇` a worktree), and the card under the cursor behind its STATUS MARK, then `· archived`, `· full screen` or `· home` when the view is not the grid of live sessions. Each part is a link: a click goes back down onto the grid there — the project's whole grid, the checkout's band, the session's card |
| a message in place of the hints | a FLASH, until the next key, led by what kind it is: `✕` a failure in crimson, `⚠` a setting to change in gold, `✓` a result nothing else shows in green, the gold spinner over muted words for a wait, `·` a heads-up in muted |
| `♪ Midnight City · M83  ⏮ ⏸ ⏭` before it | what Spotify is playing (macOS, the `spotify` setting), dimmed while paused. Each glyph is a button: back, play/pause, next. Gone while Spotify is closed or stopped |
| `2 agents · 412 MB` at the right edge | the agents running and orion's whole memory footprint (this UI, the DAEMON and every session's processes), re-read every 5 seconds |
| `↓ - Move down` on the row above the bar | the KEY COMBO DISPLAY: the key you just pressed and what it did, for anyone watching a screen share. Keys typed into a LOCKED PANE never show |

## Mouse

The mouse is another way to press the keys, never a second set of behaviors: a click on a row is
the arrow keys landing there, a click on a row of a picking list is `Enter` on that row, a second
click on a row you are already on is `Enter` on it, a right-click is a click that then opens that
row's context menu, and a click into the terminal pane is `Enter` on the pane.

- **A click on a card** aims at it: the pane opens reading that session — even folded away — and
  the keys stay on the cards. A click in the air between cards does nothing but take the focus.
- **A click outside any modal** dismisses it and lands on what it hit, without moving the cursor.
  For a list that is the same as `Esc`; a form's own `Esc` (a confirm, a prompt, the preset editor)
  runs its own staged back-out instead.
- **The header**: a click on a project tab opens it, its `×` closes it, the `+` drops the project
  list, and `3 prs` opens the PULL REQUESTS modal.
- **A band's rule**: a click aims the band, a second click opens it, and a click on its
  `↗ #42 title` opens the pull request in the browser.
- **The pane's edge** drags to resize (a double-click snaps it to the middle); the `×` on the
  pane's header folds it, the button before it moves it between the right and the bottom, and `⤢`
  full-screens the session.
- **Inside the pane**: double-click selects a word, and `⌘`-click opens the `file:line` under the
  pointer in orion's own editor modal at that line — or as its page, for markdown — as Go to file
  opens it. `⌥`- or `^`-click opens that and a URL too, in the browser (a URL's `⌘`-click is left to
  the host terminal, which opens it itself). `⌘` never rides on a mouse report, so orion asks macOS
  whether it is held as the click lands; over `orion ssh` use `⌥` or `^`. `⇧`-drag selects
  through the terminal (as tmux does). orion's own drag-select ends only when the button comes up;
  drag past the pane's top or bottom edge and the history scrolls under the pointer, so a copy can
  run longer than the pane is tall.
- **The wheel** scrolls whatever the pointer is over. Over the grid it scrolls the bands without
  moving the cursor, so a trackpad never swaps the pane out from under the card you are reading.

A program that asked for the mouse itself — Claude Code's fullscreen renderer, vim with `mouse=a`,
htop, tmux with its mouse on — gets the left button the way a plain terminal hands it over, and its
own selection does the copying. A copy it sends as an OSC 52 write is passed on to the terminal you
are sitting at. orion asks for the mouse (and bracketed paste, focus reports and the kitty keyboard
flags) again every two seconds, so a host terminal that forgot it — iTerm2's Session ▸ Reset — gets
it back on its own.
