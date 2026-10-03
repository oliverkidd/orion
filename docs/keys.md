# Keys

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Every binding below is a default, and every one of them is rebindable in the SETTINGS OVERLAY's
HOTKEYS TAB (`s` or `⌘,`) — see [Configuration](configuration.md). An action with no key at all is
still one keystroke away: the COMMAND PALETTE (`⌘⇧P`, or `:`) lists every action by name with
its key, and `Enter` runs it.

## How the keys are laid out

- **⌘ for the entry points, as in Cursor.** The things you open from anywhere — a new agent, the
  jump list, go to file, changes, the pane — are `⌘` chords, and each has a `Ctrl` twin (`⌘P` and
  `^P`) for a terminal that never sends ⌘. Ghostty and kitty send ⌘ (the KITTY PROTOCOL);
  Terminal.app, tmux and `orion browser` never do, so there only the twin arrives. Settings →
  Hotkeys shows the chords this terminal can actually press.
- **⌘ reaches orion from inside a pane.** No one types a ⌘ chord as text, so `⌘K`, `⌘P`, `⌘E`,
  `⌘⇧P` and the rest leave a locked pane and run, the agent losing nothing. The `Ctrl` twins stay
  the agent's while its pane is locked — `^P` there is the agent's `^P`.
- **Esc closes.** It leaves a locked pane (`⇧Esc` sends the agent its Esc), and it closes any list
  outright — the jump list, the file finder, the diff viewer, a menu, help — whatever you typed into
  its filter. Forms keep a staged Esc of their own: a prompt, a confirm, the preset editor and the
  issue and pull request modals back out one step at a time, so a half-typed comment is never lost
  to one press. `Ctrl+q` still force-closes any OVERLAY in one press.
- **The grid's own keys are few and bare.** Arrows walk the cards; a handful of letters act on the
  card under the cursor (below). Everything else lives behind `⌘O` (open outside orion), the
  right-click menu, or the COMMAND PALETTE.

orion writes three `unbind` lines into Ghostty's config, inside an orion-managed block it keeps in
place, so `⌘⇧P`, `⌘N` and `⌘,` reach it rather than Ghostty — see
[Configuration](configuration.md#outside-terminal-and-ghostty-keybinds). Ghostty reads them at launch or on its own
reload (`⌘⇧,`).

## Entry points

| Action | Key | Twin | What it opens |
|---|---|---|---|
| New agent | `⌘N`, `⌘I` | `^N` | the QUICK PROMPT, aimed at the checkout under the cursor (below) |
| Jump to… | `⌘K` | `^K` | fuzzy jump across every project, worktree, session and open pull request (below); its last row opens a folder as a project |
| Go to file | `⌘P` | `^P` | the FILE FINDER for the selected worktree |
| Find in files | `⌘⇧F` | `^⇧F`, `⇧F` | `git grep` into the same modal (`^⇧F` needs the kitty protocol, so `⇧F` is bound for every other terminal) |
| File tree browser | `⌘B` | `^B` | the TREE BROWSER |
| Changes | `⌘E` | `^E` | the DIFF VIEWER for the selected worktree, or the diff of the pull request the pane is reading |
| Command palette | `⌘⇧P` | `^⇧P`, `:` | every action by name, with its key; `:` is vim's command line, the key a stock terminal delivers |
| Open outside orion | `⌘O` | `^O` | the OPEN MENU: the repo, pull request or issue on GitHub, the checkout in Cursor, a terminal in the checkout, the checkout's open command |
| Reload from GitHub | `⌘R` | `^R` | ask `gh` again now for the project's open pull requests and issues, the selected worktree's pull request and the one the pane is reading |
| Toggle pane | `⌘J` | `^J` | fold the pane beside the cards away or bring it back; `⌘J` works from inside the pane, `^J` from the cards only (in the pane it is the agent's newline) |
| Full-screen session | `^F` | | give the session in the pane the whole screen, or bring it back down |
| Settings | `⌘,` | `s` | the SETTINGS OVERLAY |
| Quit | `q`, `^C` | | leave the TUI, behind a confirm; sessions keep running in the DAEMON |

## The grid

orion's whole screen is a GRID of cards (see [Sessions](sessions.md#the-grid)): the sessions and
terminals of the project whose PROJECT TAB is lit in the header, grouped into a BAND per checkout —
a titled rule over one row of cards — with a pane beside them that reads the card under the
cursor. `↑`/`↓` walk the bands, `Tab` opens one in place (its cards walked with `←`/`→`) and `Tab`
or `Esc` folds it back.

| Key | Action |
|---|---|
| `↑` / `↓` | walk the BANDS — one checkout at a time, the pane swapping onto that checkout's remembered card as you pass; on the band opened with `Tab`, walk the rows of its cards and off its last row on to the next band. On the first band `↑` has nowhere to go: one press stays put and says so, and a second within 400 ms walks up into the PROJECT TABS. Walking reads what the pane lands on — a `done` badge comes down as you arrive |
| `←` / `→` | walk the cards of the band under the cursor, stopping at either end, the pane following onto each. A `❮` before the row or a `❯` after its last card says the rest went that way, and a click on either is the same step |
| `Tab` | **open the worktree in place**: every card of the band wrapped into rows under its rule, its terminals under a `terminals` rule. One band is open at a time; `Tab` on the open band folds it back. Remembered per project and across restarts. With Settings → Appearance → **Expand all worktrees** on, every band stays open and `Tab` does nothing |
| `Enter`, a double-click | into the pane on the card under the cursor, its input locked. On a terminal too short to draw the pane the session takes the whole screen instead |
| `Esc` | close the open band first; with every band collapsed, let the band under the cursor go — the pane collapses. What was under the cursor is only let go of: the next arrow takes the aim back |
| `Space` | the FOLLOW-UP MODAL: a small box over the grid for the selected session's next turn. `Enter` sends it down the PTY, `⇧Enter` / `⌥Enter` / `^J` break a line, `Esc` closes without sending. Nothing about the pane moves. Only a live local agent takes one; a session whose CLI is not up is booted first and the box kept |
| `t` | a new shell terminal inside orion, in the cursor's checkout — a chip on the grid with the pane on it and the keys in it. `⌘O` → **Terminal in the checkout** opens one outside orion |
| `` ` `` | the next TERMINAL chip of the cursor's checkout, round to the first |
| `r` | rename the session under the cursor |
| `a` | archive the session, behind a CONFIRM DIALOG (`Enter` or `y` archives, `Esc` or `n` keeps it); on an archived card, unarchive it. A held `a` opens one dialog and archives nothing by itself |
| `⇧A` | **the ARCHIVED VIEW**: the same grid, of the project's archived sessions. `a` unarchives the card under the cursor and `Backspace` deletes it; `⇧A` again comes back |
| `Backspace` | delete the card under the cursor, behind a confirm. On the last live card of a linked worktree the confirm asks about the checkout too — `y` deletes both, `n` the card only, `Esc` keeps the card. A checkout with uncommitted or untracked work asks once more before it goes, counting the files it would lose (commits on its branch are kept), so nothing is lost by accident. On an EMPTY BAND it asks to delete the worktree itself; on a terminal's chip it closes the terminal and kills its shell |
| `c` | the BRANCH SWITCHER for the project's root checkout (below) |
| `i` | the ISSUES MODAL for the selected project (below) |
| `v` | the PULL REQUESTS MODAL for the selected project (below) |
| `.` / `,` | next / previous session in attention order — waiting on you first, then running, then the unread finishes, then the rest by last interaction — wrapping at both ends, in any project. Landing on an UNSEEN finish reads it |
| `[` / `]` | the project tab to the left / right, stopping at the ends |
| `1`–`9` | that PROJECT TAB, counting from the left; rebindable per slot as `project_tab_1` … `project_tab_9` |
| `x` | close the project tab the grid is on (the project and its sessions are untouched; `⌘K` opens it again) |
| right-click | a card's, a band's or a project tab's context menu |

The actions with no key of their own — **New session** (harness picker first), **Agent presets**,
**Duplicate session**, **Comment on pull request**, **Delete all sessions**, **SSH hosts**,
**Memory usage**, **Keyboard shortcuts**, **Open a folder as a project** — are in the COMMAND
PALETTE and, where they apply to a card, its right-click menu. Bind any of them in Settings →
Hotkeys.

### The locked pane

| Key | Action |
|---|---|
| anything | forwarded raw to the PTY — until the session's process exits, when `Esc`, `Enter` and `q` leave the lock and every other key falls through to the grid |
| `Esc` | leave the pane, back to the card it reads |
| `⇧Esc` | the agent's Esc: a bare Esc down the PTY |
| `^q`, `^]`, `^⇧H` | also leave the pane; `^q` is the HARDWIRED UNLOCK, which no rebind can take away |
| any ⌘ chord orion binds | leave the pane and run it — `⌘K`, `⌘P`, `⌘E`, `⌘J`, `⌘⇧P` … |
| `^F` | full-screen the session, or bring it back down; never forwarded to the agent. `Esc` and `^q` also bring a full-screen session back down |
| mouse wheel | a child that asked for the mouse (Claude Code's alt-screen UI, `vim` with `mouse=a`, `htop`) gets the notch as a real wheel report; an alternate-screen app that wants no mouse (`less`, plain `vim`) gets `↑`/`↓`; anything else scrolls orion's own scrollback, and the header counts how far back you are as `scroll N` |

### The project tabs

| Key | Action |
|---|---|
| `↑`,`↑` on the top row of cards | **the PROJECT TABS take the keys.** A cursor of its own lands on the lit tab; the card you left stays selected with the pane still on it |
| `←` / `→`, `[` / `]` | walk the header's cursor along the tabs — **the grid switches with it**, each project on the card you last left it on |
| `Enter`, `↓`,`↓` | hand the keys back to the cards of the project on screen |
| `x` / `Backspace` | close the tab under the cursor — `x` at once, `Backspace` behind a confirm |
| `Esc`, a click, any other key | back down to the cards; any other key then means what it means on the grid |

A click on a tab opens it, a tab's `×` closes it, and the `+` in front of the tabs drops the
PROJECT DROPDOWN — every project on the machine, the ones waiting on you first, type to narrow.
Working in a project moves its tab to the far left; switching never reorders the tabs. A
right-click on a tab is the project's own menu: **New worktree**, **Run** / **Stop run**, **Open**,
**Delete worktree** for a linked one, **Rename**, **Remove from list**.

## The jump list (`⌘K`)

Fuzzy jump across every live project, worktree and session on the machine. `↑`/`↓` (or `^N`/`^P`)
move, `Enter` jumps, `^F` only lands the selection there. Until you type it is a recent sessions
list in attention order — the ones waiting on you first — so `⌘K` `Enter` is the fastest way back
to what needs you. `Enter` on a session that NEEDS FEEDBACK attaches and focuses its pane whatever
`palette_enter_attaches` says. Typing reaches every row, projects and worktrees too; the query runs
over each row's full `project/branch/session` path. Every open pull request orion has listed is a
row too, `↗ project/#42 title`, badged `draft`, `ready for review`, or `merge conflicts` /
`checks failing` in red; `Enter` on one reads it in the pane, and `^O` opens it in the browser.
Archived sessions are never rows here — their PTY is released, so there is nothing to jump to. The
last row opens a folder as a project.

With the pane reading a pull request, `⌘E` shows its diff, `⌘R` fetches it afresh, the COMMAND
PALETTE's **Comment on pull request** comments on it, and `PgUp`/`PgDn`, `Home`/`End` scroll it.

## Views

| View | Keys |
|---|---|
| **Changes** (`⌘E`) | Changed files down the left, the diff on the right, with a live fuzzy filter. `↑`/`↓` walk the files, `⇧↑`/`⇧↓`, `PgUp`/`PgDn`, `Home`/`End` scroll the diff, and `^D`/`^U` move half the file list (`^U` kills the typed filter first). `^R` marks a file reviewed ✓ and sinks it to the bottom — orion-side bookkeeping only, cleared when HEAD moves or the file changes again. `^T` folds the file list into a directory tree and back (`←`/`→` fold, `Enter` flips a directory), and the choice is remembered. With the pane reading a pull request it shows that pull request's diff, fetched with `gh pr diff`. `Esc` closes |
| **Go to file** (`⌘P`) | Fuzzy finder over the worktree. `Enter` opens the file in the BUILT-IN EDITOR (below), `⌘C` (or `^Y`) copies the path — ready to paste into an agent — and `⌘O` opens the file in Cursor at its line |
| **Find in files** (`⌘⇧F`) | `git grep` into the same modal; `Enter` opens the hit at its line, `⌘C`/`^Y` and `⌘O` as in Go to file |
| **File tree** (`⌘B`) | Tree on the left, syntax-highlighted preview on the right (a markdown file as a rendered page; `^R` flips it to the source and back), and an always-live filter. `→` expands a directory and `←` collapses it; `Enter` folds a directory, and on a file loads it into the preview. `⇧↑`/`⇧↓`, `PgUp`/`PgDn`, `Home`/`End` scroll the preview. `⌘C`/`^Y` copies the selected path, `⌘O` opens it in Cursor, and dragging the tree/preview border resizes the tree |
| **GitHub issues** (`i`) | The project's open issues, newest first, the one under the cursor read on the right with its comments. Type to filter by `#15 title`; `↑`/`↓` (or `^N`/`^P`) walk the matches; `Esc` clears the filter before a second `Esc` closes. `PgUp`/`PgDn` and `⇧↑`/`⇧↓` read; `^O` — or the pane's `↗ open in browser` button — opens it in the browser; `^R` asks GitHub again. `^C` (or `^Y`) comments, `^E` edits the title and description in place. `Enter` opens the QUICK PROMPT for an ISSUE SESSION on it (sent empty, the task is `Fix GitHub issue #15: <title>`), `⇧Tab` picks an AGENT PRESET for it, and `^N` in the box cuts a fresh `issue-15-<title-slug>` worktree |
| **GitHub pull requests** (`v`) | The project's open pull requests, newest first with the drafts below, the one under the cursor read on the right. Filters as the issues modal does, with the same two-stage `Esc`. `^R` refreshes, `^C`/`^Y` comments, `^G` opens its diff, `^O` (or a double-click) opens it in the browser. `⌘L` flips to Linear issues to attach this PR. `Enter` opens the QUICK PROMPT for a PR SESSION on it, `⇧Tab` launches an AGENT PRESET, `Tab` picks a harness and starts one bare — all in the project's checkout of the PR's head branch |
| **Linear issues** (`⌘L`) | Open Linear issues assigned to you (Settings → Linear account; empty = the owner of the project's `LINEAR_API_KEY`). Type to filter; `Space` marks; `Enter` starts one agent on the marked set in one worktree, with a task that asks for one PR; `⇧Tab` picks a preset. From a pull request, `⌘L` attaches the PR to the marked issues |
| **Branch switcher** (`c`) | Moves the project's ROOT WORKTREE onto another branch: the current branch first, then the local branches newest first, then the remote ones. Type to filter; `↑`/`↓` move; `Enter` switches (a remote branch becomes a local one tracking it); `^R` fetches again. With nothing matching, `Enter` creates the typed branch. A checkout with uncommitted changes asks how they travel: `s` stashes, `b` brings them along, `c` commits everything first, `d` discards tracked changes on a second `d`. `Esc` on the list closes it; past the list it backs out one step at a time, and while git works it hides the modal (`c` brings it back) |
| **File tabs** *(an agent opens it)* | What `orion open <file>…` raises when a session runs it. One tab per file: `←`/`→`, `Tab`/`⇧Tab` and `1`-`9` switch tabs; the focused file is previewed underneath — a markdown file as a rendered page (`m` flips it to the source). `Enter` edits the file in that pane. `↓` from the strip drops into the preview, where `↑`/`↓`, `PgUp`/`PgDn` and `Home`/`End` scroll and `↑` off the top returns to the strip. `Esc` and `^q` step back to the strip first, and close from the strip |

### The built-in editor

Every file opens to be edited, in a modal over everything else: **micro** by default (falling back
to vim when micro isn't installed), or whatever the `editor` setting or `ORION_EDITOR` names. micro
runs off orion's own config dir (`<data dir>/micro`), so its Cursor-like keys — `^S` save, `^Z`
undo, `^C`/`^V` copy and paste, `^F` find, `^D` add the next match as another cursor — never touch
your `~/.config/micro`. The modal is handed every key raw, with two exceptions:

- `^q` — micro's own quit, which asks to save first. With an editor that has no quit of its own
  (vim), `^q` force-closes the modal instead.
- `⌘O` — the same file in Cursor, at the line it opened on; the editor stays up.

A **markdown file** opens as the MARKDOWN SPLIT: the editor on the left, its rendered page on the
right, the page re-read each time the file is saved. `^T` hides the page; on a modal narrower than
100 columns the two show one at a time and `^T` swaps them, the arrows and `PgUp`/`PgDn` scrolling
the page while it is the one showing.

## The quick prompt

`⌘N` opens a wrapped, multi-row task box that starts a new agent in the checkout under the cursor
with what you typed as its first prompt. Which CLI it launches is the `Agent` row under **Quick
prompt** in Settings → Agents. `Enter` launches; `Enter` on the box empty starts the session bare.
`Esc`, a click outside it and `^q` park what you typed, and the next `⌘N` opens on it.

| Key | Action |
|---|---|
| `⇧Enter`, `⌥Enter`, `^J` | insert a line |
| `Tab` | pick a different harness for this one launch (`→` drills into its model and effort); in Claude's list `Tab` toggles Claude Cloud |
| `⇧Tab` | pick one of your saved AGENT PRESETS, adopting its harness, model, effort and prefix/postfix |
| `^P` | the PROJECT PICKER, over the box: aim it at any project on the machine, the text kept. A launch into another project runs in the background, and the footer names where it went |
| `^T` | the WORKTREE PICKER: a fresh worktree, or any checkout of the project |
| `^O` | the harness's model list |
| `^N` | flip between a fresh worktree and the project's root branch for this one launch; `New worktree` under **Quick prompt** sets which one every box starts on |
| a click on `project …`, `worktree …`, `harness …` or `model …` | the picker the chord beside it opens |

A new worktree's branch is your sentence slugified (`fix login redirect` → `fix-login-redirect`),
or a random `<adj>-<noun>-<verb>` when empty, started at the freshly fetched `origin/HEAD` — or
the **Worktree base branch** setting. The project's ignored `.env` files are linked into it from
the main checkout (**Link .env files**, Settings → General).

## Menus and lists

The COMMAND PALETTE, the OPEN MENU, a right-click menu, help, the memory modal, the ssh hosts list
and the project pickers are all lists: type to filter where they filter, `↑`/`↓` move, `Enter` or
a click picks, `→` / `←` step into and back out of a submenu, and `Esc` or a click outside closes
the whole thing.

In the SETTINGS OVERLAY, `Tab`/`⇧Tab`, `[`/`]` and the digits move between tabs, `↑`/`↓` move down
the rows, and `←`/`→` cycle the selected row's value. On a HOTKEYS TAB row, `Enter` or `Space`
captures a chord, `a` or `+` captures a *second* chord beside it, `Backspace`/`Delete` puts the
default back, `x` unbinds it, and `Esc` cancels a capture. A chord another action already owns is
not taken silently: the row names who has it and a second `Enter` moves it. A ⚠ on a row means the
chord can't reach orion from this terminal; `R` resets every binding (with a confirmation).

## Typed fields

| Where | Key | Action |
|---|---|---|
| Any typed field | `←→`/`⌥←→`, `^A`/`^E`, `⌥⌫`, `^U`/`^K` | every prompt, filter and query is the same line editor: move by character / word, jump to ends, delete word, kill line |
| Any multi-row box | `↑`/`↓`, `⌥↑`/`⌥↓`, `^Home`/`^End`, `PgUp`/`PgDn`, wheel, click | the quick prompt, the task and comment boxes, a preset's prefix and postfix, an issue's description: `↑`/`↓` move a row and keep the column; past the top or bottom a form steps to its next field. `⌥↑`/`⌥↓` jump by paragraph, `^Home`/`^End` (`⌘↑`/`⌘↓` where the terminal passes ⌘ on) to the start or end of the text, `PgUp`/`PgDn` a boxful. `↑ 3 more` / `↓ 5 more` on the border say what is out of sight |

## Chips and readouts

The TERMINAL PANE's header shows one chip at a time — the first of these that applies:

| Chip | What it is telling you |
|---|---|
| `exited` (red) | the session's process is gone. `Esc`, `Enter` and `q` leave the lock and everything else falls through to the grid; the next attach respawns it |
| `scroll N` (yellow) | you are N lines back in the scrollback. Typing anything, or scrolling back down, clears it |
| `starting…` (dim) | nothing has come off the PTY yet: the session's CLI is booting |
| `INPUT` (accent) | the pane is locked and every key is going to the PTY; `Esc` leaves |

The FOOTER carries the rest, left to right:

| Readout | What it is telling you |
|---|---|
| `orion vX.Y.Z` at the far left | which orion this is — the string `orion --version` prints |
| `⇡ v0.22.0` after it, in the warning color | a newer orion is published on GitHub — run `orion upgrade`. `ORION_UPDATE_CHECK_SECS=0` turns the check off |
| `✗ disconnected` | this client has lost the DAEMON. A healthy connection says nothing |
| key hints | what the keys do where you are, built from your live keymap — a rebind shows up here, and an unbound action drops out |
| `2 agents · 1 term · 3 warm · 412 MB` at the right edge | live counts and orion's whole memory footprint, re-read every 5 seconds; a click opens the memory modal |
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
  list, and `3 prs` / `2 issues` open the PULL REQUESTS and ISSUES modals.
- **A band's rule**: a click aims the band, a second click opens it, and a click on its
  `↗ #42 title` opens the pull request in the browser.
- **The pane's edge** drags to resize (a double-click snaps it to the middle); the `×` on the
  pane's header folds it, the button before it moves it between the right and the bottom, and `⤢`
  full-screens the session.
- **Inside the pane**: double-click selects a word, `⌥`-click opens the URL or `file:line` under
  the pointer, and `⇧`-drag selects through the terminal (as tmux does). orion's own drag-select
  ends only when the button comes up; drag past the pane's top or bottom edge and the history
  scrolls under the pointer, so a copy can run longer than the pane is tall.
- **The wheel** scrolls whatever the pointer is over. Over the grid it scrolls the bands without
  moving the cursor, so a trackpad never swaps the pane out from under the card you are reading.

A program that asked for the mouse itself — Claude Code's fullscreen renderer, vim with `mouse=a`,
htop, tmux with its mouse on — gets the left button the way a plain terminal hands it over, and its
own selection does the copying. A copy it sends as an OSC 52 write is passed on to the terminal you
are sitting at. orion asks for the mouse (and bracketed paste, focus reports and the kitty keyboard
flags) again every two seconds, so a host terminal that forgot it — iTerm2's Session ▸ Reset — gets
it back on its own.
