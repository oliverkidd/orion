# Keys

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Every binding below is a default, and every one of them is rebindable in the SETTINGS OVERLAY's
HOTKEYS TAB (`s` or `⌘,`) — see [Configuration](configuration.md). An action with no key at all is
still one keystroke away: the COMMAND PALETTE (`⌘⇧P`, or `:`) lists every action by name with
its key, and `Enter` runs it.

## How the screen is laid out

- **The FOOTER is the status bar.** Always, left to right: which orion this is (`orion v0.42.0` —
  a click on it goes HOME), where you are (`project ▸ branch ▸ session`, then `· archived`,
  `· full screen` or `· home` when you are somewhere other than the grid of live sessions), and at
  the right edge the live counts. Between them go KEY HINTS — but only while no modal is up, and
  then only about what the grid or the pane has under its cursor: on an empty band
  `⌘N new agent · t terminal · ⌫ delete worktree`, beside a pull request the pane can read
  `→ focus PR`. Wherever you are not on the grid, the first hint is the way back to it.
- **A modal's keys are on its own bottom border,** in one style everywhere —
  `╰ Enter launch · ⇧Enter newline · Esc cancel ─╯` — and the footer under it shows none. When a
  border is too narrow for every key, whole keys drop off its right end, the modal's main verb and
  its way out (`Esc`) last; a key is never cut in half.
- **A modal's explanation** of the row or setting under its cursor is a dim line inside the frame,
  right above the keys — in the SETTINGS OVERLAY, the COMMAND PALETTE, the onboarding wizard.
- **One spelling for every key.** A letter under a modifier is its capital — `⌘K`, `^K`, `⌥P`,
  `⇧A` — and a bare letter is the letter you type (`t`). A key that belongs to an action is always
  spelled from the live keymap, so a rebind shows up everywhere at once, and a terminal that never
  sends ⌘ sees the `^` twin instead; a modal's own keys come from the same table its key handler
  reads.
- **Focus is visible.** With the screen split — the cards and the pane beside or under them — the
  side holding the keys wears the accent: the cursor's band and card on the grid, or the pane's
  title, the rule under it and its edge. See [The pane](#the-pane-focus-and-scroll).

## How the keys are laid out

- **⌘ for the entry points, as in Cursor.** The things you open from anywhere — a new agent, the
  jump list, go to file, changes, the pane — are `⌘` chords, and each has a `Ctrl` twin (`⌘P` and
  `^P`) for a terminal that never sends ⌘. Ghostty and kitty send ⌘ (the KITTY PROTOCOL);
  Terminal.app, tmux and `orion browser` never do, so there only the twin arrives. Settings →
  Hotkeys shows the chords this terminal can actually press.
- **⌘ reaches orion from inside a pane.** No one types a ⌘ chord as text, so `⌘K`, `⌘P`, `⌘E`,
  `⌘⇧P` and the rest leave a locked pane and run, the agent losing nothing. The `Ctrl` twins stay
  the agent's while its pane is locked — `^P` there is the agent's `^P`.
- **Esc climbs one level.** Modal → pane → grid, everywhere: it leaves a locked pane (`⇧Esc` sends
  the agent its Esc), brings a full-screen session back down to its pane, hands the keys back from
  the pane to the grid, folds the open band, leaves the ARCHIVED VIEW and HOME, and closes any list
  outright — the jump list, the file finder, the diff viewer, a menu, help — whatever you typed into
  its filter. Forms keep a staged Esc of their own: a prompt, a confirm, the preset editor and the
  issue and pull request modals back out one step at a time, so a half-typed comment is never lost
  to one press. `^Q` still force-closes any OVERLAY in one press. See
  [Back to the grid](#back-to-the-grid).
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
| Select model | `⌘/` | `^/` | a searchable model list for the QUICK PROMPT, opening the box first when it isn't up; type to narrow, `Enter` picks |
| Cycle effort | `⌘Y`, `⌘?` (`⇧⌘/`) | `^Y` | step the effort (`default`, low, high, …) of the model the QUICK PROMPT is set to, shown in its header at once, and the Agents tab default with it; with no box up, that default alone. macOS keeps `⇧⌘/` for every app's Help menu, so in Ghostty it opens Help and only `⌘Y` arrives; it stays bound for a terminal that lets it through, however that spells the press — `/` with ⇧, `?` with or without it |
| Select worktree | `⌘.` | `^T` | which checkout the next agent runs in, opening the box first; type to narrow, or **+ new worktree** |
| Jump to… | `⌘K` | `^K` | fuzzy jump across every project, worktree, session and open pull request (below); its last row opens a folder as a project |
| Go to file | `⌘P` | `^P` | the FILE FINDER for the selected worktree |
| Find in files | `⌘⇧F` | `^⇧F`, `⇧F` | `git grep` into the same modal (`^⇧F` needs the kitty protocol, so `⇧F` is bound for every other terminal) |
| File tree browser | `⌘B` | `^B` | the TREE BROWSER |
| Skills | `⌘S` | `^S` | the SKILLS BROWSER: every agent skill on the machine, to read, edit or trash (below) |
| Changes | `⌘E` | `^E` | the DIFF VIEWER for the selected worktree — its uncommitted changes, the whole branch, or the commits you tick, together or one at a time — or the diff of the pull request the pane is reading ([below](#the-diff-viewer)) |
| Command palette | `⌘⇧P` | `^⇧P`, `:` | every action by name, with its key; `:` is vim's command line, the key a stock terminal delivers |
| Open outside orion | `⌘O` | `^O` | the OPEN MENU: the repo, pull request or issue on GitHub, the checkout in your editor app (**Open in app**: Cursor, VS Code, …), a terminal in the checkout, the checkout's open command |
| Reload from GitHub | `⌘R` | `^R` | ask `gh` again now for the project's open pull requests and issues, the selected worktree's pull request and the one the pane is reading |
| Toggle pane | `⌘J` | `^J` | fold the pane beside the cards away or bring it back; `⌘J` works from inside the pane, `^J` from the cards only (in the pane it is the agent's newline) |
| Full-screen session | `^F` | | give the session in the pane the whole screen, or bring it back down |
| Settings | `⌘,` | `s` | the SETTINGS OVERLAY |
| Home | `⌘G` | `^G` | HOME: orion's animation over the grid, with the ways into a project; `Esc`, `Enter` or an arrow comes back to the grid exactly as it was. A click on the footer's `orion vX.Y.Z` does the same |
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
| `←` / `→` | walk the cards of the band under the cursor, stopping at either end, the pane following onto each. A `❮` before the row or a `❯` after its last card says the rest went that way, and a click on either is the same step. With the pane beside the cards, `→` off the row's last card — or on a band with no cards — goes on into the pane, to read it ([below](#the-pane-focus-and-scroll)) |
| `Tab` | **open the worktree in place**: every card of the band wrapped into rows under its rule, its terminals under a `terminals` rule. One band is open at a time; `Tab` on the open band folds it back. Remembered per project and across restarts. With Settings → Appearance → **Expand all worktrees** on, every band stays open and `Tab` does nothing |
| `Enter`, a double-click | into the pane on the card under the cursor, its input locked. On a band with nothing on it but its pull request, into the pane to read it. On a terminal too short to draw the pane the session takes the whole screen instead |
| `Esc` | close the open band first; in the ARCHIVED VIEW, back to the live sessions; with every band collapsed, let the band under the cursor go — the pane collapses. What was under the cursor is only let go of: the next arrow takes the aim back |
| `Space` | the FOLLOW-UP MODAL: a small box over the grid for the selected session's next turn. `Enter` sends it down the PTY, `⇧Enter` / `⌥Enter` / `^J` break a line, `Esc` closes without sending. Nothing about the pane moves. Only a live local agent takes one; a session whose CLI is not up is booted first and the box kept |
| `t` | a new shell terminal inside orion, in the cursor's checkout — a chip on the grid with the pane on it and the keys in it. `⌘O` → **Terminal in the checkout** opens one outside orion |
| `` ` `` | the next TERMINAL chip of the cursor's checkout, round to the first |
| `r` | rename the session under the cursor |
| `a` | archive the session, behind a CONFIRM DIALOG (`Enter` or `y` archives, `Esc` or `n` keeps it); on an archived card, unarchive it. A held `a` opens one dialog and archives nothing by itself |
| `⇧A` | **the ARCHIVED VIEW**: the same grid, of the project's archived sessions. `a` unarchives the card under the cursor and `Backspace` deletes it; `⇧A` again, or `Esc`, comes back |
| `Backspace` | delete the card under the cursor, behind a confirm. On the last live card of a linked worktree the confirm asks about the checkout too — `y` deletes both, `n` the card only, `Esc` keeps the card. A checkout with uncommitted or untracked work asks once more before it goes, counting the files it would lose (commits on its branch are kept), so nothing is lost by accident. On an EMPTY BAND it asks to delete the worktree itself; on a terminal's chip it closes the terminal and kills its shell |
| `c` | the BRANCH SWITCHER for the project's root checkout (below) |
| `⇧C` | **continue on another account**: the Claude session under the cursor, conversation and all, carried onto another Claude account and resumed there — a list of the accounts it can go to, `Enter` goes. For a session stopped at a usage limit (see [Sessions](sessions.md#usage-limits-and-a-second-account)) |
| `i` | the ISSUES MODAL for the selected project (below) |
| `v` | the PULL REQUESTS MODAL for the selected project (below) |
| `.` / `,` | next / previous session in attention order — waiting on you first, then running, then the unread finishes, then the rest by last interaction — wrapping at both ends, in any project. Landing on an UNSEEN finish reads it |
| `[` / `]` | the project tab to the left / right, stopping at the ends |
| `1`–`9` | that PROJECT TAB, counting from the left; rebindable per slot as `project_tab_1` … `project_tab_9` |
| `x` | close the project tab the grid is on (the project and its sessions are untouched; `⌘K` opens it again) |
| right-click | a card's, a band's or a project tab's context menu |

The actions with no key of their own — **New session** (harness picker first), **Agent presets**,
**Duplicate session**, **Comment on pull request**, **Delete all sessions**, **SSH hosts**,
**Memory usage**, **Keyboard shortcuts**, **Open a folder as a project**, **Claude accounts** — are
in the COMMAND PALETTE and, where they apply to a card, its right-click menu. Bind any of them in
Settings → Hotkeys.

### The locked pane

| Key | Action |
|---|---|
| anything | forwarded raw to the PTY — until the session's process exits, when `Esc`, `Enter` and `q` leave the lock and every other key falls through to the grid |
| `Esc` | leave the pane, back to the card it reads |
| `⇧Esc` | the agent's Esc: a bare Esc down the PTY |
| `^Q`, `^]`, `^⇧H` | also leave the pane; `^Q` is the HARDWIRED UNLOCK, which no rebind can take away |
| any ⌘ chord orion binds | leave the pane and run it — `⌘K`, `⌘P`, `⌘E`, `⌘J`, `⌘⇧P` … |
| `^F` | full-screen the session, or bring it back down; never forwarded to the agent. `Esc` and `^Q` also bring a full-screen session back down |
| mouse wheel | a child that asked for the mouse (Claude Code's alt-screen UI, `vim` with `mouse=a`, `htop`) gets the notch as a real wheel report; an alternate-screen app that wants no mouse (`less`, plain `vim`) gets `↑`/`↓`; anything else scrolls orion's own scrollback, and the header counts how far back you are as `scroll N` |

### The pane: focus and scroll

The pane beside (or under) the cards reads the card under the cursor — a session's terminal, a pull
request, an issue. It can take the keys without typing into anything, to be read and scrolled; the
side that has the keys wears the accent (the pane's title, the rule under it and its edge), and the
footer says so, leading with the way back.

| Key | Action |
|---|---|
| `→` off a row's last card, Enter on a band with only a pull request, a click on the page | the pane takes the keys — unlocked: nothing typed goes to a session from here. A pull request still loading takes them too |
| `↑` / `↓` | a line of the page, or of the session's scrollback (a program that took the mouse, like Claude Code, gets the wheel's notches); on a pull request's Changes, Commits or Checks tab, the row cursor |
| `PgUp` / `PgDn`, `Home` / `End` | a page; the top and the end — on a tab that lists things, its rows a page at a time and the first and last |
| `Tab` / `⇧Tab` | a pull request's next and previous tab ([below](#the-pull-request-page)) |
| `Enter` | into what it shows: a session takes the input lock, an issue opens in the browser; on a pull request, what its tab's row is — the DIFF VIEWER at the file, the commit's diff, the check's page — and the pull request in the browser on Description and Reviews |
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

The head stays put and the tab under it scrolls. All of it is one `gh pr view`, read when the cursor
rests on the pull request and remembered across launches; until it lands the tabs are up with `…`
for their counts, and the keys work already. The tab stays as you walk from one pull request to the
next — reading the checks of one after another is the common walk — and each tab's rows start again
at the top.

| Tab | What it lists | Enter (`^G` / `^O` in the modal), a click |
|---|---|---|
| **Description** | the body as markdown, then the conversation — comments and reviews, oldest first | the pull request in the browser |
| **Changes** `11` | every file: its status (`M`, `A`, `D`, `R`) and its `+`/`−`. GitHub hands back the first hundred; past that the tab says so and `⌘E` reads the whole diff | the DIFF VIEWER on the pull request, at that file |
| **Commits** `4` | newest first: short sha, subject, author and age | the DIFF VIEWER on that commit's own diff |
| **Checks** | failed first, then running, passed and skipped: `✗` / `●` / `✓` / `–`, the name, its workflow and how long it ran. The tab reads `✗ Checks 44/45` with one failed (44 of 45 fine), `● Checks 3/5` while some still run, `✓ Checks` when every one passed | the check's page — its log — in the browser |
| **Reviews** | GitHub's review decision (`✓ Approved`, `✗ Changes requested`, `● Review required`), each reviewer's latest word, who has been asked and not answered, then the reviews themselves. The tab wears the decision's mark | the pull request in the browser |

In the pane the tabs are `Tab` / `⇧Tab` and a click on a label; in the modal, whose `Tab` and
`⇧Tab` launch and whose letters type into its filter, they are `⇧←` / `⇧→`, a listing's rows are
`⇧↑` / `⇧↓`, and `^G` and `^O` reach the row — the border names each by what it does there.

### Back to the grid

The grid is the top of the view, and `Esc` climbs back to it one level at a time from anywhere —
the footer (or the modal's border) always names the way:

| Where | The way back |
|---|---|
| a modal | `Esc` (a form backs out a step at a time); `^Q` closes it outright |
| a locked pane | `Esc` — `⇧Esc` is the agent's Esc |
| a full-screen session (`⤢`, `^F`) | `Esc` to its pane, `Esc` again to the grid; the `‹ sessions` crumb in its header |
| the pane, unlocked | `Esc`, or `←` with the pane beside the cards |
| the ARCHIVED VIEW | `Esc`, or `⇧A` |
| the PROJECT TABS | `Esc`, `Enter`, or `↓`,`↓` |
| HOME (`⌘G`) | `Esc`, `Enter`, an arrow, or a click anywhere |
| every tab closed | `Enter` opens the repo orion was started in; `⌘K` lists every project |

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

With the pane reading a pull request — its [page](#the-pull-request-page) — `⌘E` shows its diff,
`⌘R` fetches it afresh, the COMMAND PALETTE's **Comment on pull request** comments on it, and
`PgUp`/`PgDn`, `Home`/`End` scroll it.

## Views

| View | Keys |
|---|---|
| **Changes** (`⌘E`) | The DIFF VIEWER: the branch's own commits over its changed files down the left, the selected file's diff on the right, wrapped — see [The diff viewer](#the-diff-viewer). `Esc` closes |
| **Go to file** (`⌘P`) | Fuzzy finder over the worktree. `Enter` opens the file in the BUILT-IN EDITOR (below) — a markdown file as its MARKDOWN PAGE, to read — `⌘C` (or `^Y`) copies the path — ready to paste into an agent — and `⌘O` opens the file in the **Open in app** editor (Cursor, VS Code, Sublime Text, Zed) at its line |
| **Find in files** (`⌘⇧F`) | `git grep` into the same modal; `Enter` opens the hit at its line — a markdown file's page with the hit in view — `⌘C`/`^Y` and `⌘O` as in Go to file |
| **File tree** (`⌘B`) | Tree on the left, syntax-highlighted preview on the right, long lines wrapped (a markdown file as a rendered page; `^R` flips it to the source and back), and an always-live filter. `→` expands a directory and `←` collapses it; `Enter` folds a directory, and on a file edits it in the BUILT-IN EDITOR in the preview's place — a markdown file opens as its MARKDOWN PAGE over the tree. `⇧↑`/`⇧↓`, `PgUp`/`PgDn`, `Home`/`End` scroll the preview. `⌘C`/`^Y` copies the selected path, `⌘O` opens it in the **Open in app** editor, and dragging the tree/preview border resizes the tree |
| **Skills** (`⌘S`) | Every agent skill on the machine in one list, nothing picked for a launch — agents find their skills themselves. Yours (`~/.claude/skills`, or `$CLAUDE_CONFIG_DIR/skills`), the selected checkout's (its `.claude/skills`, `.cursor/skills`, `.codex/skills`, `.agents/skills`), `~/.cursor/skills`, `~/.codex/skills` (`$CODEX_HOME`) and `~/.agents/skills`, and installed Claude Code plugins' — each row badged `user`, `project`, `cursor`, `codex`, `agents` or `plugin`. A folder reached twice (`~/.claude/skills` a symlink to `~/.cursor/skills`) is listed once. Type to filter by name and description; `↑`/`↓` (or `^N`/`^P`) walk the matches, and the right pane reads the one under the cursor: its frontmatter on a line or two, the SKILL.md as a rendered page, its other files. `⇧↑`/`⇧↓`, `PgUp`/`PgDn`, `Home`/`End` scroll it. `Enter` (or a click on the row the cursor is on) edits the SKILL.md in the BUILT-IN EDITOR, `⌘O` opens the skill's folder in the **Open in app** editor, and `⌘C`/`^Y` copies the SKILL.md's path. `^A` names a new skill and opens `~/.claude/skills/<name>/SKILL.md`, written from a stub. `^D` moves the skill's folder — its symlinks resolved — to the Trash behind a confirm that names it: `~/.Trash` on macOS, the freedesktop.org Trash elsewhere, never a delete. A plugin's skills are read-only and refuse it. `^R` reads the folders again, as closing the editor does. `Esc` closes |
| **GitHub issues** (`i`) | The project's open issues, newest first, the one under the cursor read on the right with its comments. Type to filter by `#15 title`; `↑`/`↓` (or `^N`/`^P`) walk the matches; `Esc` clears the filter before a second `Esc` closes. `PgUp`/`PgDn` and `⇧↑`/`⇧↓` read; `^O` — or the pane's `↗ open in browser` button — opens it in the browser; `^R` asks GitHub again. `^C` (or `^Y`) comments, `^E` edits the title and description in place. `Enter` opens the QUICK PROMPT for an ISSUE SESSION on it (sent empty, the task is `Fix GitHub issue #15: <title>`), `⇧Tab` picks an AGENT PRESET for it, and `⌘.` in the box offers a fresh `issue-15-<title-slug>` worktree as its first row |
| **GitHub pull requests** (`v`) | The project's open pull requests, newest first with the drafts below, the one under the cursor read on the right as its [pull request page](#the-pull-request-page) — `⇧←`/`⇧→` (or a click on a label) walk its tabs, `⇧↑`/`⇧↓` the rows of Changes, Commits and Checks. Filters as the issues modal does, with the same two-stage `Esc`. `^R` refreshes, `^C`/`^Y` comments, `^G` opens its diff — at the file under the cursor on Changes, that commit's on Commits — and `^O` (or a double-click) opens it in the browser, or on Checks the check under the cursor. `⌘L` flips to Linear issues to attach this PR. `Enter` opens the QUICK PROMPT for a PR SESSION on it, `⇧Tab` launches an AGENT PRESET, `Tab` picks a harness and starts one bare — all in the project's checkout of the PR's head branch |
| **Linear issues** (`⌘L`) | Open Linear issues assigned to you (Settings → Linear → **Linear account**; empty = the owner of the project's `LINEAR_API_KEY`). Type to filter; `Space` marks; `Enter` starts one agent on the marked set in one worktree, with a task that asks for one PR; `⇧Tab` picks a preset. From a pull request, `⌘L` attaches the PR to the marked issues |
| **Branch switcher** (`c`) | Moves the project's ROOT WORKTREE onto another branch: the current branch first, then the local branches newest first, then the remote ones. Type to filter; `↑`/`↓` move; `Enter` switches (a remote branch becomes a local one tracking it); `^R` fetches again. With nothing matching, `Enter` creates the typed branch. A checkout with uncommitted changes asks how they travel: `s` stashes, `b` brings them along, `c` commits everything first, `d` discards tracked changes on a second `d`. `Esc` on the list closes it; past the list it backs out one step at a time, and while git works it hides the modal (`c` brings it back) |
| **File tabs** *(an agent opens it)* | What `orion open <file>…` raises when a session runs it. One tab per file: `←`/`→`, `Tab`/`⇧Tab` and `1`-`9` switch tabs; the focused file is previewed underneath — a markdown file as a rendered page (`m` flips it to the source). `Enter` edits the file in that pane. `↓` from the strip drops into the preview, where `↑`/`↓`, `PgUp`/`PgDn` and `Home`/`End` scroll and `↑` off the top returns to the strip. `Esc` and `^Q` step back to the strip first, and close from the strip |

### The built-in editor

Every file opens to be edited, in a modal over everything else, in the editor Settings → General
**File editor** names (`editor`, or `ORION_EDITOR`): **fresh** by default, micro or Microsoft Edit
(`edit`) — the three whose keys are VS Code's: `^S` save, `^Z` undo, `^C`/`^V` copy and paste,
`^F` find, `^Q` quit after asking to save — then vim, nvim, Helix and emacs. One that isn't
installed opens the first installed of fresh, micro, Edit and vim instead, and the first time
it does the footer says so ([Configuration](configuration.md)); `i` on the **File editor** row, or on
the editor's row in first-run setup, installs it ([Installing editors and agent
CLIs](configuration.md#installing-editors-and-agent-clis)). Long lines wrap: orion turns micro's
and Edit's wrap on, and fresh, vim and emacs wrap on their own.

The modal hands the editor every key, and the mouse — a click places the cursor, a drag selects,
the wheel scrolls — with three exceptions:

- `^\` — force-closes the modal, from any editor in any state, without saving.
- `^q` — fresh's, micro's and Edit's own quit, which asks to save first. With an editor that has no
  quit there (vim, Helix), `^q` force-closes the modal instead.
- `⌘O` — the same file in the **Open in app** editor, at the line it opened on; the editor stays
  up.

In micro, Edit and fresh a `⌘` chord that reaches orion is the editor's `^` chord, as VS Code has
it on a Mac: `⌘S` saves, `⌘Z` undoes, `⌘⇧Z` redoes, `⌘F` finds, `⌘A` selects all, `⌘D` adds the
next match as another cursor. orion's own `⌘` keys — `⌘L`, `⌘/`, `⌘P`, `⌘K` — are the editor's
while it is up. A Mac's text-editing chords mean what they mean in VS Code, each typed as the key
the editor binds that action to:

| Key | Action | fresh | micro | Edit |
|---|---|---|---|---|
| `⌘←` `⌘→` (and `^A` `^E`) | line start / end | ✓ | ✓ | ✓ |
| `⇧⌘←` `⇧⌘→` | select to the line's start / end | ✓ | ✓ | ✓ |
| `⌘↑` `⌘↓` | file start / end | ✓ | ✓ | ✓ |
| `⇧⌘↑` `⇧⌘↓` | select to the file's start / end | ✓ | ✓ | ✓ |
| `⌥←` `⌥→` | word left / right | ✓ | ✓ | ✓ |
| `⇧⌥←` `⇧⌥→` | select a word left / right | ✓ | ✓ | ✓ |
| `⌥⌘↑` `⌥⌘↓` | add a cursor above / below | ✓ | ✓ | — |
| `⌘D` | add the next match as a cursor | ✓ | ✓ | — |
| `⌘⇧L` | a cursor on every match | ✓ | ✓ | — |
| `⌘L` | select the line | ✓ | ✓ | — |
| `⌘/` | toggle comment | ✓ | ✓ | — |
| `⌘⇧P` | command palette | ✓ | its command bar | — |
| `⌥↑` `⌥↓` | move the line up / down | ✓ | ✓ | — |
| `Esc` | back to one cursor | ✓ | ✓ | — |

- **`⌘⇧L` is `⌘D` pressed 500 times at once.** Neither editor has a select-all-matches of its own;
  both stop adding cursors at the last match, so the presses past it do nothing. The first press
  takes the word under the cursor when nothing is selected, as `⌘D` does.
- **Ghostty keeps `⌘←`/`⌘→` and `⌥←`/`⌥→`** and types the shell's keys for them — `^A`/`^E`, `⎋b`/`⎋f`
  — so in the editor a bare `^A` and `^E` are the line's start and end, as in any Mac text field.
  micro's command bar, `^E` elsewhere, is `⌘⇧P` (`⌥:`) here.
- **Released from Ghostty** ([Configuration](configuration.md#outside-terminal-and-ghostty-keybinds)):
  `⌘↑`/`⌘↓` and `⇧⌘↑`/`⇧⌘↓` — Ghostty's jump to prompt — and `⌥⌘↑`/`⌥⌘↓` — its split up and down,
  in every Ghostty window; `⌥⌘←`/`⌥⌘→` stay its split left and right.
- **Not there:** Edit has no multiple cursors, comment toggle or line select; vim, Helix and emacs
  get every key as it is typed.

micro runs off orion's own config dir (`<data dir>/micro`), so its keys here and its wrap never
touch your `~/.config/micro`; fresh off orion's own config file (`<data dir>/fresh/config.json`) —
the one file, no menu, tabs, explorer or dock, a two-item status bar, VS Code's Dark+ colours on
orion's background — never your `~/.config/fresh/config.json` ([Configuration](configuration.md)).

### The markdown page

A **markdown file** opened from Go to file, find in files, the tree or an ⌥click opens as its
MARKDOWN PAGE: the file rendered, full width and wrapped, in a modal of its own.

| Key | Action |
|---|---|
| `↑`/`↓`, `PgUp`/`PgDn`, `Space`, `Home`/`End`, the wheel | scroll |
| `Enter`, `e` | edit it in the BUILT-IN EDITOR, at the line it was opened on (a find-in-files hit's); quitting the editor lands back on the page, re-read |
| `⌘O` (`^o`) | open it in the **Open in app** editor |
| `⌘C` (`^y`) | copy its path |
| `Esc`, `q`, a click outside it | close it |

The FILE TABS and the skills browser show a markdown file's page in their own preview already, so
their `Enter` edits it.

### The diff viewer

`⌘E` reads the checkout under the cursor in one modal: the COMMIT LIST over the changed files down
the left, the selected file's diff on the right at the modal's full height. The panel with the keys
wears the accent, and `Tab` walks them in reading order — commits, files, diff.

- **The commits are what the branch added.** Its own commits since the base it was cut from, newest
  first — `git log --first-parent --no-merges <merge-base>..HEAD` — so a merge of `main` into it,
  and every commit that came in with that merge, never show. **Uncommitted changes** sits on top
  while the checkout is dirty. Each row has a box, `[✓]` ticked and `[ ]` not; its subject wraps
  under the box, its short sha, age and `+added −removed` on the line below.
- **What the diff shows.** With nothing ticked, the row under the cursor. Tick rows and they read
  TOGETHER, as one diff — or, `^G`, ONE AT A TIME, oldest first, `commit 2 of 3 · <subject>` over
  each. The rows on screen are the bold ones, and the panel's foot says `3 ticked · together`. A
  clean checkout opens with every commit ticked — the whole branch, what its pull request shows; a
  dirty one on its uncommitted changes, nothing ticked. Nothing carries over to the next `⌘E`.
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
| `↑` / `↓` | the commits, the files | walk the list; with nothing ticked each commit goes up as the cursor lands on it, one at a time the step follows it onto a ticked row |
| `Space` | the commits | tick the row under the cursor, or untick it |
| `^A` | the commits, or the files with nothing typed | tick every commit — or, when they all are, none |
| `^G` | two or more ticked | read them together, or one at a time |
| `⇧←` / `⇧→` | anywhere | older / newer: the cursor's commit with nothing ticked; the step before or after one at a time; from together, the newest or the oldest step |
| `Enter` | the commits / a file / a tree directory | on to the files / the diff takes the keys / fold it |
| `↑` / `↓`, `Space`, `←` | the diff | a row, a page, back to the files |
| `⇧↑` / `⇧↓`, `PgUp` / `PgDn`, `Home` / `End` | anywhere | scroll the diff |
| typing | anywhere | the files' fuzzy filter, which takes the keys; `^U` kills it |
| `^R` | the files, the diff | mark the file reviewed ✓, which sinks it to the bottom — orion-side bookkeeping only: on the uncommitted changes it is stored until HEAD moves or the file changes again, on anything else it lasts while the modal is open |
| `^T` | the files | fold the file list into a directory tree and back (`←`/`→` fold), remembered |
| `⌘O` (`^O`) | the files, the diff | the file in the **Open in app** editor, at the line at the top of the diff |
| the wheel | over any panel | scrolls that panel; no cursor moves |
| a click | | a commit's row aims the cursor there and its box ticks it, a file is selected, the diff takes the keys; a second click on the cursor's row is `Enter` |
| `Esc` | | close |

With the pane reading a pull request, `⌘E` — and the PULL REQUESTS MODAL's `^G` — open the same
viewer on that pull request's diff, fetched whole with `gh pr diff`: its files in the column, its
title heading each, no commit list.

## The quick prompt

`⌘N` opens a wrapped, multi-row task box that starts a new agent in the checkout under the cursor
with what you typed as its first prompt. Which CLI it launches is the `Agent` row under **Quick
prompt** in Settings → Agents. `Enter` launches; `Enter` on the box empty starts the session bare.
`Esc`, a click outside it and `^Q` park what you typed, and the next `⌘N` opens on it.

Its header names everything the launch is made of, each field beside the key that changes it — where
it runs over what runs it:

```
project demo ⌘P   worktree main ⌘.
agent   Claude (you@example.com) Tab   model opus · latest ⌘/   effort high ⌘Y
```

The effort is always there — `default` until one is picked — for every harness that has one
(OpenCode has none, so its box has no effort field); an AGENT PRESET on the launch adds a
`preset reviewer ⇧Tab` field, and a fresh worktree reads `new worktree <branch>`, in the green the
frame turns. The worktree, model and effort keys are spelled from the live keymap, so a rebind shows
there at once; on a narrow screen the fields wrap onto more rows rather than lose a key. What `Enter`
sends is the dim line along the bottom of the frame.

| Key | Action |
|---|---|
| `⇧Enter`, `⌥Enter`, `^J` | insert a line |
| `Tab` | pick a different harness for this one launch (`→` drills into its model and effort); in Claude's list `Tab` toggles Claude Cloud |
| `⇧Tab` | pick one of your saved AGENT PRESETS, adopting its harness, model, effort and prefix/postfix |
| `⌘P` (`^P`) | the PROJECT PICKER, over the box: aim it at any project on the machine, the text kept. A launch into another project runs in the background, and the footer names where it went |
| `⌘.` (`^T`) | the WORKTREE PICKER: **+ new worktree** first, then every checkout of the project |
| `⌘/` (`^/`) | the harness's model list |
| `⌘Y` (`^Y`) | step the effort, the header showing the step at once — `⇧⌘/` too, where macOS lets it through |
| a click on a header field | the picker its key opens; on `effort`, the model's effort list |

`⌘.`, `⌘/` and `⌘Y` work the same with one of the box's pickers open over it — the model list,
the worktree list, the harness or preset picker — and none of them closes the box: the model and
worktree keys swap the picker in front, the effort key hands the box back stepped.

A new worktree's branch is your sentence slugified (`fix login redirect` → `fix-login-redirect`),
or a random `<adj>-<noun>-<verb>` when empty, started at the freshly fetched `origin/HEAD` — or
the **Worktree base branch** setting. The project's ignored `.env` files are linked into it from
the main checkout (**Link .env files**, Settings → General).

## Menus and lists

The COMMAND PALETTE, the OPEN MENU, a right-click menu, help, the memory modal, the ssh hosts list
and the project pickers are all lists: type to filter where they filter, `↑`/`↓` move, `Enter` or
a click picks, `→` / `←` step into and back out of a submenu, and `Esc` or a click outside closes
the whole thing. Each list's keys are on its bottom border — on a session picker's Claude row
`Tab cloud off`, on a harness `? settings` — and the COMMAND PALETTE says what the row under its
cursor does on the line above them.

In the SETTINGS OVERLAY, `Tab`/`⇧Tab`, `[`/`]` and the digits move between tabs, `↑`/`↓` move down
the rows, and `←`/`→` cycle the selected row's value. On a HOTKEYS TAB row, `Enter` or `Space`
captures a chord, `a` or `+` captures a *second* chord beside it, `Backspace`/`Delete` puts the
default back, `x` unbinds it, and `Esc` cancels a capture. A chord another action already owns is
not taken silently: the row names who has it and a second `Enter` moves it. A ⚠ on a row means the
chord can't reach orion from this terminal; `R` resets every binding (with a confirmation). On the
Agents tab's **Claude accounts** rows, `Enter` signs the account in (asking for the email first),
`r` renames it, `o` signs it out, `←`/`→` switch it on or off and `⌫` removes an added one; `Enter`
on **Add account** names a new one; on a **Saved on this machine** row `Enter` adds the dir back
under a name and `⌫` moves it to the Trash ([Configuration](configuration.md#claude-accounts)). On a row whose
program isn't on PATH — General's **File editor**, a Claude account, a harness's **Enabled** row —
`i` puts the command that installs it where the row's explanation was, `Enter` runs it in the editor
modal and anything else leaves it unrun ([Installing editors and agent
CLIs](configuration.md#installing-editors-and-agent-clis)). The installer's own output is the
modal: `^Q` stops it, and when it exits the title says whether the program is on PATH now and
`Enter` closes it.

FIRST-RUN SETUP (**Orion setup**, while `onboarded` is unset) is the same kind of modal, a step at
a time — the STEP STRIP across its top lights the one it is on. `→`/`Tab` go to the next step,
`←`/`⇧Tab` back, `↑`/`↓` move down a step's rows; `Space` flips the row (an agent on or off, an
editor chosen, a switch), `Enter` does its verb (an agent's default model, an account's sign-in, a
typed row opened, the connection tested), `i` installs a missing agent CLI or editor, and `Esc`
skips the rest — or, while the step asks something, backs out of the question.

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
| `orion vX.Y.Z` at the far left | which orion this is — the string `orion --version` prints. A button: a click goes HOME, and from HOME back to the grid |
| `⇡ v0.22.0` after it, in the warning color | a newer orion is published on GitHub — run `orion upgrade`. `ORION_UPDATE_CHECK_SECS=0` turns the check off |
| `✗ disconnected` | this client has lost the DAEMON. A healthy connection says nothing |
| key hints | what the keys do where you are, built from your live keymap — a rebind shows up here, and an unbound action drops out. None while a modal is up: its keys are on its own border |
| `demo ▸ feat ▸ fix-login  · archived` after it | where you are: the project, the checkout and the card under the cursor, then `· archived`, `· full screen` or `· home` when the view is not the grid of live sessions |
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
