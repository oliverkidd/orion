# Sessions

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

Everything that can start an AGENT, and what each launch path does differently.

**Session, agent, terminal.** A SESSION is anything with a card on the grid, and it is one of two
things: an AGENT — an AI coding CLI (Claude, Codex, Cursor, …) working on a task, started with `⌘N` —
or a TERMINAL — a plain shell, started with `t`. orion says "agent" for what starts or concerns the
CLI, "terminal" for the shell, and "session" only for what is true of both: the cards, the attention
walk (`.` / `,`), archiving and deleting.

## The NEW AGENT PICKER

Run **New agent — choose harness** from the command palette (`⌘⇧P`, or `:`). A menu asks what to
run — **Claude**, **Codex**, **Cursor**, **Grok Build**, or **OpenCode** (a plain shell is `t` — see [Keys](keys.md)); a CLI you never use can be
switched off on the settings overlay's Accounts tab and drops out of the menu entirely. Claude is
listed by the account it is signed in as, `Claude (you@example.com)`, and every other CLAUDE
ACCOUNT you have added has a row of its own right after it ([below](#usage-limits-and-a-second-account)). The menu
lists only enabled harnesses whose CLI is found on PATH (the daemon still
checks through the login shell at launch). Your own CLIs join the menu too: add one to config.json
`harnesses` by the name of its program (see [Configuration](configuration.md#the-harness-registry))
and it appears after the built-ins under its own label, toggled in its own section on the Accounts
tab, with the session card wearing the entry's label as its badge. It launches with a `--model`
flag, boots fresh every time (no resume), and stays process-based: working while the PTY is live,
done when it ends, never needing you. `→` on any row drills
into model and reasoning-effort submenus (Cursor's model is a family such as `claude-opus-5-thinking`, and
its effort list follows the family, `-fast` variants included — `cursor-agent --list-models` bakes both
into the id, so orion launches `--model claude-opus-5-thinking-high-fast`; the list is a built-in seed
merged with `--list-models`, cached in `cursor_models.json` beside `config.json` and refreshed daily).
In those submenus you type to filter — `opus` narrows the rows to the Opus families, `↑`/`↓` move, `Backspace` widens, `Esc` clears — and the preset editor's Harness / Model / Effort rows take the same type-ahead.
`Enter` anywhere takes your configured defaults. `Enter` on a row opens the QUICK PROMPT set to
that harness, model and effort, aimed at the checkout under the cursor: type the task and `Enter`
launches it, or send the box empty and the CLI starts bare, and you type the agent's first prompt
there. Nothing asks for a name — the
session titles itself from that first prompt (AUTO-TITLE); `r` renames it whenever you like. `⌘N`
opens the same box straight away, on the Accounts tab's harness (see [Keys](keys.md)).

The picker opens on the Accounts tab's **New agents on** harness — the one `⌘N` launches — so
`Enter` at once is `⌘N`, whatever you picked last: a pick is one session's, and the Accounts tab is
where the defaults are set.

The rows do not run under the same permissions, and the picker is where you decide that. Claude
is spawned with no permission flag at all and keeps its normal prompts — it stops and asks before the
things it is configured to ask about. Codex is spawned with `--yolo` and Cursor with `--force`, so
**neither of those two ever stops to ask**: they edit files and run commands on their own judgment for
the life of the SESSION, and nothing in the picker or the settings overlay softens that. OpenCode keeps its own permission prompts: orion passes no `--auto`, so it
stops and asks like Claude does. Grok Build keeps its own prompts too — orion passes no `--always-approve` — but
unlike those two it has no hook that says it is asking (below). Pick the
harness with that in mind, especially in the ROOT WORKTREE.

The same choice reaches the STATUS DOT, because an AGENT can only report what its hook set can see.
Claude installs the full set — `UserPromptSubmit`, `Stop`, `SessionStart`, `PermissionRequest`,
`Notification` (which is where the idle prompt comes from), `PreToolUse` on `AskUserQuestion`, an
unmatched `PostToolUse` and `StopFailure` (a turn that ends on an API error, a usage limit among
them — see [below](#usage-limits-and-a-second-account)) — so a Claude row walks the whole range of
states, red NEEDS FEEDBACK included, and leaves red the moment you answer: a question's answer is its
own tool's `PostToolUse`, and an approved permission prompt shows up as the gated tool running,
whichever tool it was. Codex
has no `Notification` hook and no `AskUserQuestion` tool, but its native `PermissionRequest` is
installed, so the red state stays reachable there (and, with no `PostToolUse` to say you approved,
a Codex row stays red until the turn ends). Cursor has no `PermissionRequest` hook to install,
and since orion runs it with `--force` there is nothing left to wait on anyway: its hooks are
`sessionStart`, `beforeSubmitPrompt`, `stop`, `subagentStart` and `subagentStop`, which is busy versus
idle and nothing else. **A Cursor SESSION can never show the red NEEDS FEEDBACK dot** — if you are
watching one and waiting for it to ask you something, it is not going to. OpenCode
runs TypeScript plugins, so orion installs one managed plugin (`~/.config/opencode/plugins/orion.ts`,
inert outside orion) that posts OpenCode's `chat.message` as `UserPromptSubmit`, its `session.status`
/ `session.idle` as `Stop`, `permission.asked` / `permission.replied` as `PermissionRequest` and the
gated tool's `PostToolUse`, and its `question` tool's `question.asked` / `question.replied` as
`PreToolUse` / `PostToolUse` — so an OpenCode row is working, red while a permission or a question
waits on you (one raised from a subagent session included), and done when the turn ends, an aborted
one included. Grok Build reads hook files from its home, so orion keeps one there of its own
(`~/.grok/hooks/orion.json`, inert outside orion) with `SessionStart`, `UserPromptSubmit`, `Stop`,
`StopFailure`, `SubagentStart` and `SubagentStop` — a Grok row is working and done, and resumes its
conversation after a restart — but Grok has no permission-request hook, so **a Grok SESSION that
stops to ask you something stays working**. The picker says so on its row, dim at the right edge
(`can't show it's waiting on you`), as it does for any CLI of yours with no hook dialect
(`status from the process only`).

## AGENT PRESETS

If you keep starting the same kind of session with the same framing, save it as an **agent preset**:
**Agent presets** (the command palette, or `⇧Tab` in the quick prompt) lists them, and the launch lands in that card's checkout — its last row says which, and `Tab` flips it to a fresh worktree. Over the quick prompt the box's header says where instead, and its `⌘.` picks, so that row and its `Tab` are not drawn there. Type to find one by name: letters
narrow the list to the fuzzy matches, `↑`/`↓` move, `Backspace` widens and `Esc` clears — as in the
model and effort submenus. `Ctrl+a` opens a small form — name, harness, model, effort, **Text**
(which side of the task the preset's text goes: `prefix`, `postfix` or `prefix & postfix`, with a
box for each side it names) and **Task** (`ask` or `skip`) — and `Ctrl+e` / `Ctrl+d` edit or
delete; they are chords because plain letters type. A new preset starts on the side
`preset_text` in CONFIG.JSON names, by hand — `prefix` unless you change it: one
box, sent before your task — and cycling the row shows the other box or both; a side the row leaves
out saves blank. Editing a preset also shows any side that already holds text, so one saved with
both never loses either. `Enter` on a
preset opens the QUICK PROMPT on it, the prefix and postfix written round the caret, and `Enter` there
launches the CLI with `prefix + task + postfix` as its very first prompt. The task is optional:
send the box with nothing typed and the prefix and postfix go on their own (a preset with neither starts the CLI
with no first prompt). Set **Task** to `skip` for a preset that never needs one — a "commit and push" —
and `Enter` launches it at once, no box at all; the list marks those rows `no task`. A `skip` preset
picked with `Shift+Tab` in a quick prompt launches the same way when
the box is still empty, while text you already typed stays yours to send. The row it creates is an
ordinary session: it names itself on that first turn, resumes, and shows status like any other. Presets
live in `agent_presets.json` beside `config.json`. The form's Harness row lists custom registry entries
by id alongside the built-ins — a preset on one launches with the entry's program and defaults, and
refuses with the reason when its entry is switched off or gone.

Three presets are **system presets**, on every list and marked `system` — the agents orion starts
itself:

- **PR autofix** wraps what broke for the agent **Auto-fix my PRs** sends to a broken pull request.
  That agent runs on the PR's own agent — the session that did the work on its branch — and, with
  none, on your default agent at this preset's model and effort. orion's text: fix only what is
  listed; resolve conflicts; reproduce each failing unit test locally and fix it; re-run only the
  e2e specs that failed, never the full suite, unless earlier attempts failed the same way; push;
  watch the PR until it is green and go round again.
- **Linear Ticket** is what a Linear launch sends — with its harness, model and effort once its text
  is yours — unless `⇧Tab` picks another: `{issues}` becomes each issue's ID, title, link and
  description, `{ids}` the IDs comma-separated and `{first_id}` the first one, and text that names no
  `{issues}` gets them between its prefix and postfix. orion's text: one commit per issue, one pull
  request titled `"{ids}: …"` whose description starts `Fixes {ids}`.
- **Week review** holds the rules a WEEK IN REVIEW (`⌘⇧Y`) is written by — what earns a line, Show
  and Say, how a point is written — and the Claude account, model and effort that write it: sonnet ·
  medium unless you change them. An account that is off falls back to your default agent's.

A system preset always holds text: its editor opens on orion's text, and saving it unchanged, or
cleared, keeps nothing of yours, so it says orion's text from then on — including a better one a
later release ships. Its text, harness, model and effort can be edited like any preset's; it can't
be deleted or renamed. A list from before they existed keeps a preset you had already named `PR
autofix`, `Linear Ticket` or `Week review`, text and all, as that system preset. The settings older
releases kept for these agents in `config.json` — `autofix_model`, `autofix_effort`,
`review_account`, `review_model`, `review_effort` — move onto them once, the review's account as it
resolved then, and leave the file.

In the PULL REQUESTS MODAL (`v`), `Shift+Tab` on a pull request is a
picker instead of the manager: the preset you pick launches a PR SESSION on that pull request. The
box comes back with the preset applied and the PR in its title, and `Enter` starts the agent in the
project's worktree on the PR's head branch — reused when one is already checked out on it, cut by
the daemon otherwise — with the PR link and its work rule in the system prompt and the preset's
`prefix + task + postfix` as the first prompt, so the agent is already working on the PR when the
pane opens. A `skip`-task preset launches straight from the picker. `Enter` on the pull request is the same launch
without a preset: the box titled for the PR, your text alone as the first prompt, and the checkout's
row up under the pull request as soon as you press `Enter` — never a random-branch worktree the
session would have to move onto the pull request by hand.

A contributor's pull request from a fork gets a checkout named for the fork: `givemeurhats/main`,
`wende/feat/settings-hotkey`. A fork's branch shares nothing with yours but possibly a name — `main`
above all, which every fork has and your root checkout is on — so under its bare name a PR SESSION on
a fork's `main` ran in the root checkout, on your code, with nothing cut and nothing nested. Under the
fork's name it matches no branch of yours: the daemon seeds it from the pull request's own ref
(`refs/pull/N/head`) and points its upstream there, as `gh pr checkout` does, so `git pull` in the
checkout follows the contributor's pushes and the checkout's own PR ROW finds the pull request. A
checkout of a fork's pull request made before this, under the bare branch name, is a plain worktree
row now; the next PR SESSION cuts the fork-named one.

If the daemon refuses a PR SESSION — the fetch failed offline, the fork is gone — the `creating` rows
come down and the box you sent it from comes back with your text, as any refused quick prompt does.

## The FOLLOW-UP COMPOSER

The next turn for a session already running. Put the cursor on an agent's card and press `Space` — or
pick **Follow-up prompt** from its right-click menu — and a small MODAL opens over the grid, titled
`Follow-up · <session>`, four rows of typing.

`Enter` sends what you typed to the agent as its next turn and closes the box; `Shift+Enter`,
`Option+Enter` and `Ctrl+J` break a line, as in Claude Code's own prompt, and `Esc` closes the box without sending. The text goes straight down the session's
PTY — the same path your keystrokes take in the pane — so the CLI sees it as a prompt typed at it. A
prompt with line breaks in it crosses as one bracketed paste rather than as typing, so nothing
auto-indents it to mush.

While the box is open it owns the keyboard: the grid's own verbs include bare letters, so `t`, `r` and `s`
are letters in your prompt and not a terminal, a rename or Settings aimed at the session you are prompting.
A click outside the box closes it, as it closes any modal.

Nothing about the PANE moves when you use it. It is not unfolded, not swapped onto the card, not
attached to and not focused, and the turn goes down the session's PTY where it stands: prompting a
card is not opening it. So the loop is click a card, `Space`, type a line, `Enter`, move to the next
card — over a folded pane (`⌘J`) if you walk the cards with the keys, since a click on a card always
brings the pane back — never once stepping into a session or waiting for one to attach. A pane already open is left on whatever card it was reading, a terminal's chip included.

Only a live local agent takes one. An archived session's turn is over, a shell
terminal takes typing in the pane, and a pull request row is not a conversation — `Space` on any of
them opens no box. A session whose CLI is not up — reaped by the IDLE REAPER, or cold since the daemon
started — is booted first and the box left as it is, your text still in it: nothing is typed into a
process that is still starting, so press `Enter` again once it is up.

## Dropping a screenshot on a prompt box

Drag a file onto the QUICK PROMPT or the FOLLOW-UP box, and the terminal
pastes its path. For a macOS screenshot dragged from its floating thumbnail, that path points at a
temporary file (`…/TemporaryItems/NSIRD_screencaptureui_…/Screenshot 2026-09-21 at 11.13.58 PM.png`).
macOS deletes that file soon after the drop, and the space before `PM` is a U+202F that the agent types
back as a plain space. The agent could not find the file either way.

So the box copies the file as it lands, into `attachments/` in the DATA DIR under a plain name
(`Screenshot-2026-09-21-at-11.13.58-PM.png`), and writes that copy's path in place of the one dropped.
This happens to any dropped file in a `TemporaryItems` folder, and to any image whose path has spaces or
characters outside ASCII. Other dropped paths — a source file, an image with a tidy name — stay exactly
as pasted, so the agent works on the file itself. Copies older than a week are deleted the next time
something is dropped. A drop straight into a session's pane goes to the CLI untouched: Claude Code reads
a dropped image there itself.

An image on the clipboard — a screenshot taken with ⌃⇧⌘4, an image copied from a browser — goes in
with `^V` in the same boxes (the hint is on each box's bottom border). The terminal's own `⌘V` only
ever pastes text, so orion reads the clipboard itself: `pngpaste` when it is on PATH, otherwise
`osascript` asking for the clipboard as PNG (or as TIFF, converted by `sips`). The image is written
to the same `attachments/` folder as `clipboard-<hash>.png` — the same image pasted twice is one file
— and its path goes in at the caret, replacing any selection. With no image on the clipboard the
footer says so and nothing is pasted.

## The quick prompt's draft

The QUICK PROMPT's text is saved to `quick_prompt_drafts.json` in the DATA DIR as you type it, one draft
per place: the worktree the box lands in, or the project for a box that cuts a fresh worktree. Each
change rewritten whole through a temporary file, nothing written when nothing changed. Closing the
terminal window kills orion without a clean shutdown, so there is no exit hook to rely on; with the
text already on disk, the next box opened at the same place after a restart starts from it, caret at its end, its
explanation line reading `draft restored · …` until the first edit. Within one run the box you
closed comes back the same way, with its harness and preset (`Esc` parks it), but only into a box
at the same place. A box a picker re-aims takes its draft with it.

Sending the box deletes the draft, and so does emptying it by hand. A launch the daemon refuses
comes back with its text, which is the draft again. Boxes that carry their own text — a refused
launch, a preset's text, the FOLLOW-UP box — never open on the draft, and a box with text of its own
writes nothing until you edit it, so opening one never overwrites what you were drafting.

## RECENT PROMPTS

Every session card carries the last thing its session was asked to do, condensed to one line and
clipped to the card, with a dim `30m ago` pinned to the right. The DAEMON keeps the newest ten prompts
per session behind it.

The text is the prompt as you typed it, not a paraphrase. The DAEMON reads it off the
`UserPromptSubmit` hook payload every harness sends (Claude, Codex and Cursor name it `prompt`;
OpenCode's managed plugin posts the same field), collapses its whitespace and keeps the first 200 characters,
so a pasted file shows as its opening line. It costs the agent nothing — no extra turn, no tool call,
nothing added to its context — which is why it is the prompt and not a summary the model wrote.
Prompts orion composes itself, such as a PR SESSION's scope or the note a `orion worktree`
relocation reopens on, are left out, and so are blank ones. A click on the line lands on the card,
archived cards show none, and a session created before the feature simply has nothing to show until
its next prompt.

## Usage limits and a second account

A Claude session that hits its subscription's usage limit stops mid-task, and without help it would
look like any session that finished. Claude Code ends such a turn with a `StopFailure` hook instead
of a `Stop`, and orion installs one beside the others. When its error is a usage limit
(`rate_limit`), a billing stop (`billing_error`) or a held account (`account_on_hold`), the row goes
to red NEEDS FEEDBACK with the limit as its reason: the card says `limit reached` (`account on
hold` for a held account) where its age goes, Claude's own words where its last prompt goes — `You've
hit your session limit · resets 3:45pm` — and the list layout says the same in its columns. It counts
everywhere a question would: the project tab's red count, the `.` / `,` attention walk, the FEEDBACK
SOUND and the desktop notification (`Fix Login: limit reached`), and the idle reaper leaves it alone,
since Claude Code waits in the open session to carry the task on at the reset. Any other API error
(`overloaded`, a server error) ends the turn like a `Stop`.

The limit is the reason the row is red, so it lasts exactly as long as the red does. The next prompt
or tool call — yours, or the continuation Claude Code sends itself at the reset — moves the session
and takes the limit away with the red; so does the CLI ending. When the limit reset while the
machine slept and Claude waits for an `Enter`, the row stays red without the limit. A TUI restarted
meanwhile shows it as it was: the limit is kept on the session's row in the DAEMON. A DAEMON restart
ends the CLI, and Claude's wait for the reset with it, so the row comes back gray, without the limit.

To see a limit coming rather than hit it, `⇧U` opens ACCOUNT USAGE: the share left on each limit a
Claude account reports and the time until it resets, beside Cursor's month. An account with a
session stopped at its limit turns red there, and its emptiest limit says `limit reached`, at once,
before the next read.
See [Keys](keys.md#the-grid).

A second Claude account is one more Claude Code config dir with a login of its own: **Add
account** under **Claude accounts** on the settings overlay's Accounts tab (or **Claude accounts** in
the COMMAND PALETTE, or the step first-run onboarding has right after Agents) makes the dir, offers to
share your setup with it — `CLAUDE.md`, settings, skills, agents, commands, plugins, as links — and
`Enter` on its row signs it in with Claude Code's own `claude auth login`. Every account goes by the
name you gave it and the email it is signed in as, `Work (you@example.com)` — `Claude
(you@example.com)` until it has a name — and `r` on its row renames it, the default account too,
without moving its sessions or its dir. On a machine with more than one a card says which its
session runs on by that name, else that email (`Work opus high`), where it would say `claude`. A
removed account's dir stays, unless you send it to the Trash, for an account added by the same name
to take back. A session on any account is a Claude session — the same hooks, model sync, title sync
and resume — that keeps to its own account's config dir. See
[Configuration](configuration.md#claude-accounts) for the `claude_accounts` entry behind it,
signing in and out, renaming and removing one.

Two accounts signed in to the same claude.ai login are one subscription with one limit — the
browser sign-in approves whichever account the browser is signed in to, so it is easy to end up
there. The Accounts tab and onboarding flag it under the accounts, with the fix: sign one in again from
a private browser window, or with the other email.

With a second account set up, a Claude session can go on there: **Continue on Claude
(b@example.com)**, one row per account, in the card's right-click menu beside **Restart** — and
`⇧C` on the card or **Continue on another account** in the COMMAND PALETTE, which list the accounts
it can go to. An account signed in as the session's own email is listed as what it is, `Claude
(you@example.com) · same account`, and the footer says how to fix it: going there would only meet
the same limit.
A card stopped on a limit names the way under Claude's words: `⇧C: continue on Claude
(b@example.com)`. Going stops the session's CLI, copies its transcript —
`projects/<slug>/<id>.jsonl` in the config dir its harness uses, and the `<id>/` folder beside it
with its subagents' transcripts and its title — to the
same place under the other account's config dir, switches the card to that harness (name, worktree,
and the model and effort that harness offers, kept), and resumes the conversation there. The CLI
comes back at its input box: type `continue`, or whatever the next turn is. The source is never
deleted, and a file the destination already holds is never overwritten unless it is an older copy
of the same transcript — a session carried back to an account it ran on before. A session with no
conversation yet or a transcript orion can't find is refused with the reason in the footer, before
anything stops; an archived session or one off Claude has nowhere to go, and
`⇧C` on it lists nothing.

## The GRID

orion opens on a GRID of session cards — no modal over it, ever, on any launch. Starting a session
is one chord from there: `⌘N` opens the QUICK PROMPT, focused, so the first thing you type is the task.

- **The box** starts on the selected project and in the checkout under the cursor — the worktree
  whose band the cursor is on, open or collapsed; the root branch with nothing selected —
  launching the harness, model and effort the Accounts tab defaults name. Its header spells them out,
  each field beside the key that changes it, where the launch runs over what runs it:
  `project demo ⌘P   worktree main ⌘.` over
  `agent Claude (you@example.com) Tab   model opus · latest ⌘/   effort high ⌘Y`. The effort is
  always there — `default` until one is picked — for every harness that has one; an AGENT PRESET on
  the box adds its own `preset … ⇧Tab` field; and a fresh worktree reads `new worktree <branch>` in
  green, the frame turning green with it. The keys are the ones the box answers to, the worktree,
  model and effort ones spelled from your keymap (`⌘.`, `⌘/`, `⌘Y`);
  on a narrow screen the fields wrap onto more rows rather than lose a key. What `Enter` sends is
  the dim line along the bottom of the frame, right above its keys.
  `⌘P` puts the PROJECT PICKER over it — literally over it: the list floats inside the box, which
  stays on screen under it with its title, its header and the task already typed into it, so
  aiming the launch never costs you sight of what you are launching. Every project on the machine is
  in the list, the most recently worked in first, narrowed as you type; Enter aims the box there
  with your text kept. Aiming the box is not navigation — the grid behind it stays on the project
  you are working in, so a prompt fired at another project is a **background launch**: the session
  starts over there and nothing on screen moves — no tab is added, none is lit. The header lights
  the project when the box is aimed away, and the footer names it once Enter lands.
  `⌘.` drops the WORKTREE PICKER down from the branch the box names — **+ new worktree** first,
  then every checkout the project has, the one the box is aimed at ticked, then every branch with
  no checkout yet, local and on origin (`⎇`, a teammate's branch among them: picking one runs the
  launch in a new worktree that checks that branch out) — and picks where this
  one launch runs, never switching a checkout's branch; a fresh worktree is that first row, the
  box's own toggle for it gone. `⌘/` opens the harness's model list straight away (`→` on a model
  reaches its efforts), `⌘Y` steps the effort in place — `default`, then the harness's list, round
  again — and `Tab` opens the harness picker — the lists over the box, as the project picker is,
  so the task stays in front of you while you pick what will run it. The three keymap keys work
  with any of the box's pickers open over it, and none of them closes the box. `⇧Tab` takes a
  preset; a model or effort picked afterwards for the same harness keeps it. None of it needs the
  key: the header is a row of buttons, and a click on a field opens what the key printed beside it
  opens — on
  `effort`, the model's effort list — box and task still in front of you. Enter
  launches, and the cursor and the pane land on the new card as it goes up in its band, the grid
  scrolling to it and the keys still on the grid. Turn off **Follow new session** in
  Settings → General and nothing you were looking at moves instead: the cursor and the pane stay on
  the card you were on, and the footer names the branch the new session went to (`started a session
  in feat`). Esc leaves the box for the grid, keeping what you typed — `⌘N`
  opens on it again.
- **The grid** takes the top of the body: every unarchived session of
  the **selected project**, most recently touched first, as a wall
  of cards — up to four a row, fewer as the terminal narrows,
  under a header of **PROJECT TABS** — ` +   web ●1 ×   api ●2 ●1 ×` — one tab per project you
  have opened, the one you last worked in first, right after the `+`, and a count of the grid's cards on the
  right. The tab the grid is on is lit, a raised chip with its name in the accent. The tabs are how
  you move between projects — there is no level above the grid to walk out to. Opening a project
  that has no tab yet — from the `+`, a `⌘K` jump, a folder just opened — puts one first, next to the `+`,
  and so does working in one: launching a session there (a `^P` launch into another project included),
  opening a shell or a worktree there, sending a follow-up, or typing into one of its sessions in the
  pane moves its tab to the front, so the header reads from the project you last worked in, left to
  right. Only switching to a tab moves none of them, so `[` / `]` walk a row that holds still.
  A click on a tab, `[` / `]` for the tab to the left / right (stopping at either end), or a digit `1`–`9` for
  the Nth from the left opens that project's sessions, on the card you last left it on — its session
  back in the pane — or its first card on a first visit. A project with no sessions yet — a folder
  just opened, or any other — opens on the empty grid with the pane folded away, however it was
  opened (a tab, the `+`, a `⌘K` jump), so the orion has the whole body — or, with **Show all
  worktrees** on, the project's EMPTY BANDS do, the pane folded all the same; whatever the pane was
  reading, a session or a terminal, runs on in the project you left and is never shown here. A
  project with only a terminal in it opens on that terminal's card, the pane reading it.
  `` ^` `` brings the pane up empty — one press, since there was nothing on screen to fold — with
  nothing on its header to click — `t` opens a terminal on the project's root; the terminal comes
  up as a card inside its checkout's band with the keys in it, and `t` (or an EMPTY BAND's
  right-click **New terminal**) is the only way to open one: the grid has no `+` for it. From the keyboard the header can also be
  walked: `↑`,`↑` on the first line hands the keys up to the tabs, with a cursor of
  their own on the lit tab; `←`/`→` move it and the grid switches with it, each project
  shown on its last-focused card as the cursor passes, and `Enter` — or `↓`,`↓` back
  down, or `Esc` — hands the keys back to the cards of the project on screen. The `×` on a tab, or `Backspace` on the
  one under the header's cursor while it has the keys (behind a confirm), closes it and the grid
  moves to the tab that slides into its place. Closing a tab
  changes nothing about the project — its sessions run on. Closing the last tab puts orion back on
  the splash it opens on before there is any project, where `Enter` begins — it opens the repo orion
  was started in — `+` lists every project and `⌘K`'s last row opens another folder; the splash stays
  across a restart. The
  tabs are remembered across restarts. Each tab carries its project's STATUS MARKS right of the name — the
  mark a session in each state wears, carrying that state's count and no word at all: crimson `●`
  waiting on you, crimson `✕` crashed, `●` in the done color for an unread finish, the gold spinner
  mid-turn — in that fixed order and left out where a state is empty, so a quiet project is its bare
  name. An unlit tab's name is bright while something there wants you and dim while nothing does,
  and it shimmers: once, in red, for the seconds after a session there starts waiting on you or
  crashes, and in the done color for as long as a finish there is left unread — it stops the moment
  you read it. Work in progress never sweeps a tab; the spinner says that. The tab you are on never
  sweeps at all: its accent says where you are. The sweep recolors the name in place, so no tab
  moves; the animations setting turns it off. The `+` in front of the tabs — or the
  key `+` from the cards (`⌘P` is its silent alias inside the pane, where the terminal sends ⌘) —
  drops the PROJECT DROPDOWN: every project on the machine — the
  ones with a session waiting on you first, then the ones running, then the rest most recently
  worked in — the one in front of you ticked and each with how many sessions it holds, and a last
  row, `+ open a folder…`, for a folder that is not a project yet (the prompt **Open a folder as a project** opens). It takes
  **type-ahead** — letters narrow the rows to what they fuzzy-match, best first, `↑`/`↓` move,
  Backspace widens, Esc clears the query before it closes the list — so opening a project is its
  name and Enter. The row is the tabs' first: the counts right of them get only what the tabs
  leave. Squeezed, the tabs give way in steps — first the `×` on every tab but the lit one (a tab
  without one closes once it is lit), then whole tabs, from the right, into the MORE CHIP after the
  last one drawn: `2 more ▾`, carrying their STATUS DOTS between them and sweeping as a tab's name
  does, so a project the row had no room for still says it wants you. A click on the chip (either
  button) lists just those projects under it, and the pick opens one; `[` / `]`, the digits and the
  header's cursor reach them too, each drawn in the lit tab's place. The lit tab is always drawn,
  its name whole wherever there is room for it. A tab's name, its `×`, the chip and the `+` all mark
  themselves while the pointer rests on them. The cards are grouped
  into **BANDS**, one per checkout — the root first, then the rest most recently worked in first.
  A checkout with nothing running in it is an EMPTY BAND, its rule over a single line,
  `nothing running` — what can be done there is
  the footer's to say once the cursor is on it (`⌘N new agent · t terminal · ⌫ delete worktree`,
  and `→ focus PR` beside the pull request the pane reads). `↑`/`↓` walk onto it like any band, `⌘N`/`t` start work in
  that checkout, and `Backspace` (or **Delete worktree** in its right-click menu) deletes it behind the
  worktree's own confirm — and `⌘⌫` (`⇧D`) does the same from any card on a band, taking every
  session in it down with the checkout behind one confirm that names them; the root's band offers
  no delete, since the root is never deleted. Deleting a worktree's last card never asks about
  the worktree: the band stays, empty, until its own `Backspace`. Each band is a
  titled rule over its entries, a line apiece: the rule names the checkout by its SCOPE MARK (`⎇ feat`,
  `⌂ main` for the root, the purple `●` once its pull request has merged — the project is the
  grid's own scope, named once in the header; the root's branch is bright, a worktree's muted), and
  after the branch, dim, the folder it is checked out in when the branch's name doesn't spell it
  (`⎇ feat/ui-redesign in pawy`). Its right end holds columns that are as wide on every band, so
  they line up down the grid and nothing shifts when one band's count changes: the pull request —
  `↗ #42 ready`, muted, in the colors the PR rows wear (crimson `conflicts` or `failing`, purple
  once merged, faint for a draft or a closed one), a link while the pointer rests on it — then the
  checkout's git counts, each right-aligned in its own column: uncommitted files `*3` in the diff's
  sand, lines `+120 −45` in its sage and rose, and commits ahead of and behind the base it was cut
  from, `⇡4 ⇣1`, muted (the root's base is origin's copy of its branch, so there they are what is
  unpushed and unpulled). They count against what the last fetch left — orion fetches on no timer:
  `p` fetches and pulls the band's checkout, and `⇧P` pushes it ([Keys](keys.md#the-grid)). A band
  with nothing to say in a column leaves it blank; a narrow rule drops the folder, then the pull
  request's column, then the counts', before the branch shortens.
  The band the keys are on opens on `❯` where the rest open on `──`.
  The entries under it are the checkout's sessions, then its terminals, the names and what each
  runs on in columns down the band. A shell is `❯ shell-1` with `shell` for what it runs on, and a
  faint `exited` at the right once it is gone. A RUN TERMINAL is its RUN LINE, `▶ run`: its command
  on a `$` where a session's prompt goes, and its state at the right where a session's age is — a
  dim `running`, `stopping… 1:52` while a stop winds it down (the line goes once it has), or, for a
  run that ended with nobody stopping it, a faint `done` or a red `exited 1`. After the command come
  the localhost ports the run has printed — `http://localhost:3000`, `127.0.0.1` or `0.0.0.0` —
  underlined, `:3000 :5173`, kept after the lines that said them scroll away and cleared when the run
  starts again. A click on one opens it in the browser; the first is the app, and `⌘O` → **Open
  app** opens it too. The run line's right-click menu has **Stop stack** (or **Start stack**) and
  **Restart stack**: a stop, then a start in the same line once the run has gone, `restarting… 1:52`
  meanwhile. A session is its status dot and name,
  what it runs on (`opus high`: its model and the reasoning effort it was launched with — a session
  on the CLI's own default effort stops at the model), then the last thing it was asked to do, on a
  `›`, and how long ago at the right. The pull requests come from the same `gh` lookups
  the PULL REQUESTS MODAL makes, swept over every checkout the grid lists. The header counts the project's open pull requests beside the session count
  (`4 sessions  3 prs`), so what is waiting on the repo is read without opening `v` to find it —
  and a click on the count opens that list, as the key does. When the room left
  cannot hold every entry, the far right of that row says how many it could not draw and which way
  they went — `↓ 7 hidden` for entries under the fold, `↑ 4 hidden` for ones scrolled off the top,
  `↑↓ 5 hidden` for both. The count beside it still says how many sessions the project has, so an
  entry that is not on screen reads as something taking its room rather than as a session gone, and
  the arrow says whether the way back to it is `↓` down through the grid or the pane's edge dragged
  back down. On a row the tabs leave little of, the counts give way whole, least needed first: the
  session count (each band's rule says its own), then the pr count, and this one last.
  An open band's edges say it again where the eye looks for the rest: `↑ 2 more
  above` on the row of air under the PROJECT TABS once the top has scrolled off, `↓ 3 more below`
  on a row kept under the grid while there is more past the bottom — the row stays as air once the
  grid is scrolled to its end, and neither appears on a grid that fits.
- **Walking it.** `↑` and `↓` walk the lines as one column, off a band's last line onto the next
  band's first — the rule of the band under the cursor takes the accent and its branch goes bold,
  the entry under the cursor wears `▌` and a faint wash of its status color (the needs-you red, the
  done color while a finish is unread, breathing while the animations are on; the accent's, still,
  on one at rest), and the pane reads it; the accent goes with the keys, so with them up on the
  PROJECT TABS or down in the pane the rule is gray like the others and only the bold branch and the
  entry's mark still say what the pane reads. Every band lists all of its entries, so there is
  nothing to open or fold. The whole grid scrolls by rows to keep the cursor's entry on screen.
  `Enter` opens the entry under the cursor in the pane, and a jump that names a session — `⌘K`,
  the attention walk, a terminal just opened — lands on its line.
  The wheel never walks the cursor, so a trackpad cannot swap the pane out from under the entry you
  are reading: the grid scrolls the way a terminal's screen does — the wheel moves the bands three
  rows a notch under a cursor that stays put, held at the grid's ends. The next key that walks the
  cursor, or `↑`/`↓` against the grid's edge, scrolls just far enough to bring the cursor's entry
  back on screen — with the checkout's rule when it is the first — and a click on one does the
  same.
- **The pane** runs down the right side of the grid, full height and half the width — a window
  too narrow for it beside the grid lays it along the bottom until there is room — and reads
  whichever entry the cursor is on: `● polish-nav  ⎇ feat  ↗ #42` on its header, the card's name, its checkout and its pull
  request, and that session
  live under it, swapping as you walk the
  grid, so stepping across the bands reads each checkout's progress in turn. A terminal's chip
  puts its shell there the same way. It is the selected session, so
  it comes and goes with the selection: clicking a card opens the pane on it — reading only, the keys
  stay on the cards, so a clicked card stays a
  preview under its rule — and letting the card go (`Esc`, or `⌘J`) collapses
  the pane and gives the grid the whole body back. A click on the air between the cards does not: a
  miss with the pointer leaves the pane exactly where it is. It takes a second click, or Enter, to type into it. It is the same pane
  a full-screen session has — the same attach, the same scrollback and wheel, and the card it shows is marked read
  the moment it lands there, so a `done` badge comes down as you arrive rather than when you open it.
  A click into the pane types into that session where it stands, with the grid still up over it, and
  `` ^` `` hands the keys back to the cards — press it again there and the pane folds away. Its top edge is draggable — a short `━` grip marks it, and
  pulling it up or down trades rows between the cards and the session under them, stopping against
  the pane's own minimum one way and the header plus a few of the grid's rows the other — with the header's
  `↓ 7 hidden` counting whatever the cards lost the room for as you pull. The height you
  leave it at is remembered across restarts, and re-fitted to the window each frame. Beside the
  cards it is the edge facing them that drags, sideways, under a short `┃` grip, trading columns
  down to one column of cards — and that width is remembered apart from the height, so switching
  sides never turns one into the other. A double-click on either edge snaps it to the middle of the
  body, the cards and the pane sharing it evenly. On a terminal
  too short for the header, a few of the grid's rows and a pane worth the name, there is no pane and a session
  is only ever seen full-screen.
- **A follow-up** is `Space` on a card: a small modal opens over the grid and `Enter` sends what you
  type as that session's next turn, straight down its PTY. The pane is left exactly as it is — this is
  the way to hand a wall of sessions their next instructions one after another without opening any of
  them. See [the FOLLOW-UP COMPOSER](#the-follow-up-composer).
- **Stepping into one** is Enter on a card (or a double-click): the keys cross
  into the pane beside the cards, where that session is already running, with its input locked and
  the grid still up over it — the same place a click into the pane lands. `` ^` `` hands the keys
  back to the cards, and a second `` ^` `` folds the pane away.
- **Full-screen** is `⌘F` in the pane (or on a card), or the `⤢` in the pane's header: the grid and
  its header give way to the session, the keys still in it. `⌘F` again, `^q`, `` ^` `` or the `⤡` at
  the right end of the full-screen header brings it back down into the pane beside the cards, the
  keys still in it.
- **On a terminal too short to draw the pane**, stepping into a session gives it the whole screen
  instead — the grid gives way — with its input locked. Its header is a
  breadcrumb — `‹ sessions / ● Fix the login redirect loop`, with the harness, model and checkout
  right-aligned — and `^q`, or a click on `‹ sessions`, comes back to the grid with the cursor on
  the card you came from: there is no pane to come back down to. `⌘N` opens the box again, on the project under the cursor.

- **The project's own menu** — its verbs, which a session card has no room for — is a
  right-click on its PROJECT TAB: **New worktree**; **Start stack** / **Stop stack** and **Open** for the
  checkout the grid would launch into, and **Delete worktree** when that is a linked one; **Rename**, a label only —
  the folder on disk keeps its name, and an empty name goes back to it; and **Remove from list**,
  behind a confirm, which leaves the clone on disk alone. There is nothing above the bands to walk
  out to: `Esc` lets the card go, a second does nothing, and `↑` on the first band stays put.

Every other key acts on the session under the cursor — `⌘⇧A` (`^A`) archives, `Backspace` deletes, `⌘E` opens its
diff, `⌘K` jumps, `s` opens Settings. `⌘⇧A` asks first, always: a CONFIRM DIALOG names
the session, `Enter` or `y` archives it and `Esc` or `n` keeps it, so a letter aimed at an agent
that lands on the grid archives nothing — and saying yes is cheap, since `⌘⇧U` (`^U`) brings it back. The
cursor lands on the card after the one archived in its band — the one that slides up into its
place — or, when the band's last card went, on the one before it; a band's only card leaving leaves
the band, empty, under the cursor. `Backspace` lands the same way, and its confirm is the card's
own: deleting a worktree's last card never asks about the worktree, which stays as an empty band
until its own `Backspace`. A held `⌘⇧A` opens one dialog and
archives nothing by itself; a held `⌘⇧U` in the ARCHIVED VIEW unarchives one card, and the next needs
the key let go and pressed again (on a terminal with the kitty keyboard protocol, which is what
tells a held key's repeats from a fresh press; without it a long hold still walks the row).

**Every band carries an ARCHIVED DRAWER** under it once its checkout has an archived session: a
faint `▸ 3 archived` line, folded by default. `z` on the band — or a click on the line — unfolds it
into one line per archived session, most recently archived first, each drawn in an archived card's
quiet colors with how long ago it was filed on the right; the cursor lands on the newest, so `z`
then `⌘⇧U` brings back the session just archived. `↓` off the band's cards walks into the drawer and
on to the next band, `↑` the other way; `⌘⇧U` on a line (or a double-click) unarchives it, the cursor
staying on it as a live card, and `Backspace` deletes it. `z` again folds the drawer, the cursor
back on the band's cards. A checkout with only archived sessions has no band of its own — unless its
drawer is unfolded, so archiving a band's last card with the drawer open leaves the band standing.
Which drawers are open is remembered across restarts.

**`⇧A` is the ARCHIVED VIEW**: the same grid, of the project's archived sessions instead of its live
ones, with the header counting them under their own word (`3 archived sessions`). `⌘⇧U` unarchives the
card under the cursor where it stands and `Backspace` deletes it; `⇧A` again comes back to the live
sessions. The two lists never mix — there is no group to fold, only the other grid — and `Enter`
on an archived card says to unarchive it first, its session having been reaped when it was
archived.

**An archived card is drawn as the live card put away**, so which of the two grids is on screen
reads before the header is: its frame squares off (`┌`, where a live card is rounded `╭`), its
STATUS DOT gives way to a square `▪` — the session is filed, and the status it was filed in stopped
being a job the moment it was — and the color comes off the whole card: the name goes muted and
drops its bold, and the harness row goes with it. The ARCHIVED VIEW's bands are the checkouts the
filed sessions were in, with no terminals under them — a terminal is never archived. The badge on the right counts from when the session was archived (`2h ago`), not
from its last turn — a session archived before orion kept that stamp simply has no badge. Under the
cursor the whole card lifts a step, so the one you are about to unarchive stays legible on the
selection fill, and with color off entirely the two shapes still tell the grids apart. No card says
the word `archived`: the header says it once, for all of them.

## The PULL REQUESTS MODAL

`v` lists the selected PROJECT's open pull requests down the left of a modal — newest first with the drafts sunk below the
finished ones, and drafts listed even while `hide_draft_prs` keeps them out of the `⌘K` jump list — and the one
under the cursor read on the right as the pane reads a pull request, its [PULL REQUEST
PAGE](keys.md#the-pull-request-page): number and title, state, mergeability, author, branches and
size over the tabs — **Description** (the body rendered as markdown, then the conversation),
**Changes**, **Commits**, **Checks** and **Reviews**, each with its count or verdict. `⇧←`/`⇧→` walk
the tabs and `⇧↑`/`⇧↓` a listing's rows. A row reads
the way the pull request on a band's rule does — a draft dimmed with a `draft` badge, one GitHub says
cannot merge red end to end with `conflicts` or `failing` — and the modal opens on the pull request
the pane is reading (after a `⌘K` jump onto one), when it reads one.

Under the open rows sits **Merged**: the pull requests that landed in the last 7 days, the latest
merge first, each in the merged purple with how long ago it merged — so the list reads as a lineage,
not only as what is left to do. They ride the same query as the open list (the first page of 100
merged pull requests, most recently touched first), and only the modal lists them:
the grid's counts, the `⌘K` jump list and AUTOFIX see open pull requests alone. A merged row is read,
diffed, commented on and opened in the browser like any other; the verbs that need an open pull
request — `Enter`'s agent, merge, close, ready/draft, autofix — say it has merged and do nothing.
`is:merged` in the filter shows them alone.

Nothing new is asked of GitHub to paint it. The rows are the project's open list the OPEN PRS beat
already keeps warm (and remembers across launches), so the modal opens on them at once; a list older
than thirty seconds is asked for again underneath, and the cursor follows its pull request by URL
when the answer reorders the rows or retires one. The reading pane shares the pane's fetch: a pull
request read in one is read in the other, and resting on a row for a moment fetches its body (`gh pr
view` — one call for the body and every tab) the same way. `Ctrl+r` asks for the list and the row's
body again now.

The keys are every filtered list's, and the QUICK PROMPT box's: the list filters as you type, narrowing
the rows to the fuzzy matches of `#42 title`, `Esc` clears the filter before a second `Esc` closes,
and the verbs are chords. `Enter` opens the QUICK PROMPT for a
PR SESSION on the pull request — the box `⌘N` opens, titled `Quick prompt · PR #42 …`
— `Shift+Tab` launches one of your AGENT PRESETS on it, and `Tab` picks a harness and starts the session bare,
`→` drilling into the MODEL / EFFORT submenus. Every one of them is the same launch: a
`CreatePrAgent` that runs in the project's checkout of the pull request's head branch, reused when
one is there and cut by the DAEMON otherwise, its stand-in rows up under the pull request from the
moment you launch, and the PR's URL and work rule in the harness's context. The QUICK PROMPT stands
on the modal: the list stays under the box, `Esc` or a click outside puts
you back on the pull request you opened it on, and the launch closes the modal onto the grid, the
new session's card up in its band. `Ctrl+c` (or `Ctrl+y`) opens the
COMMENT BOX on the pull request and comes back to the modal on the row — after `Enter` posts, after
`Esc`, and after a post `gh` refused, with your text back in the box — `Ctrl+g` opens the pull request's
whole diff — on the Changes tab at the file under the cursor, on Commits that commit's own diff, and a
click on a file or a commit does the same — `Ctrl+o` — or a click on the `↗ open in browser` button pinned right on the reading pane's
frame — opens it in the browser and marks it read on the way out (on the Checks tab, and on a click on
a check, the check's own page), and `Esc` closes the
modal (twice, with a filter typed). The hotkey is
rebindable (`pull_requests`).

## The WEEK IN REVIEW

`⌘⇧Y` (`⇧W`) writes up what was finished in the selected PROJECT in the last 7 days — the pull
requests merged, the Linear issues done, the todos ticked — as a review to read aloud with the
product on screen: by product area, each point numbered, with what to **Show** and what to **Say**
under the ones worth a demo.

orion gathers; a model writes. Opening the modal fetches every pull request merged in the week with
its description (the PULL REQUESTS MODAL's merged list carries titles, and a review is written from
what a pull request says it does), a page of 100 at a time until the week is covered. The done
issues and ticked todos come from the lists already in hand. `Enter` then puts all of it in one
prompt and sends it through a Claude account's print mode with no tools: text in, text out. No
session starts, nothing runs in a checkout, and there is nothing to tidy up after.

The account is one orion runs sessions on, never whatever a bare `claude` would sign in as: the one
the **Week review** system preset runs on (see [Agent presets](#agent-presets); a new list puts it
on your default agent's account), else your default agent's when that is a Claude account, and
otherwise the first Claude account that is switched on. The model and effort are the preset's too,
sonnet · medium unless you change them. It runs as that account's own CLI with that account's config dir, so the ask lands on
that sign-in and that account's usage. Compose says which on its **Via** line, as Claude accounts
names it — `Work (a@b.co)`, or `Work (not signed in)` — and refuses to send, saying why, when no
Claude account is on or its CLI is not on your PATH. An account that reads as not signed in is still
tried: that record only knows a browser sign-in, and one on an API key answers all the same; one that
cannot answer says so itself, and Compose shows what it said. A very large
pull request — a feature branch merged in one go — sends far more of its description than an
ordinary one, since its commit list is all the description it has; one that only moves a standing
branch into `main` is marked a release and left out.

**Whose** is *Just mine* (the pull requests you opened, the issues assigned to you) or *Everyone*,
where each point names its owner by initials — from their GitHub name, else their login — and each
area lists who contributed to it. **Where** is this project or all of them.

The model is asked to list only what a person outside the team that built it would want to know or
see, and never to count. What it sends back is held to its shape without a model: every pull
request it names was in the week, every owner is the author of the point's first pull request. A
reply that strays is still shown, with a line under its title saying how. The last line — how many
were merged and not listed — is orion's own subtraction, so it is exact.

Reviews are kept as Markdown under the DATA DIR (`reviews/`), the last 12 per project, with what
each was written from beside it. The **Note** typed on Compose is kept per project, so what matters
there is said once.

## TODOS

`⌘I` (`^Q` where the terminal sends no ⌘) opens the TODOS MODAL: every open PROJECT's own todo
list, a tab each along the top in the order of the grid's project tabs, then any project whose
tab is closed, opened on the selected project's tab (`←`/`→`, `Tab` or a click for another,
`⌘1`–`⌘9` straight to one; each comes back as it was left while the modal is up). Each
list is modelled on a daily todo: groups that nest and fold, each item with Linear's priority, in
an order that is the list's own. The list is this machine's alone — one file per checkout under orion's data folder
(`~/Library/Application Support/dev.orion.orion/todos/` on macOS), named by a hash of the
checkout's path, the path written inside it — and is never written into the repo, so `git status`
stays clean. Every change is saved at once; a file that will not read is moved aside to
`<name>.corrupt-<time>.json`, the footer saying so, rather than written over.

The list is the groups. A top-level group is a section: its name in capitals, a rule across, and its
open items at each priority (`U`rgent, `H`igh, `M`edium, `L`ow — the letters the Linear issues modal
uses too), how many are open and how many were ticked today, nested groups and folded headers counted
in. A group under it says only how many are open. Under each header is a line per item — more when
its text is long enough to wrap: its box, its priority letter, the text, and on the right the agent
sent at it and how many days it has carried over (`2d`). A
nested group stands among its parent's items wherever it was put. `Space` ticks an item: it
strikes through and drops to the bottom of its group, the next item coming up under the cursor.
When everything in a group is ticked, the group is complete: its name struck through, `✓ done`, at
the bottom of the group it is in (or of the list). Ticked items and complete groups stay in sight
until the modal closes; the next time it opens they are gone from the list. Nothing unfinished goes
anywhere — it is simply still there tomorrow, a day older.

The project row's right end says what was ticked today and this week, across every project, with a
bar for each day from Monday. A click on `✓N today` or `✓N this week` opens the **DONE** page: what
was ticked in that span, a section per project in the tabs' order, each over the groups it was
ticked in and nested groups under theirs, with the time (today) or day (this week) it was ticked.
`←`/`→` turn between today and the week, `Space` puts an item ticked by mistake back on its list,
`Esc` goes back. Days are local: a tick at 11pm is that day's, and the week starts on Monday.

The order is yours: `⌥↑`/`⌥↓` move an item a place up or down the list as it is drawn, whatever
its priority — past the next item, into a nested group it meets and through it, out of the bottom
of the nested group to just under it, and past the end of a top-level group onto the start of the
next (or the end of the one before). On a header they move the group: a nested one the same way
(past what is beside it, never into another), and no further than the end of the last top-level
group (or the start of the first); a top-level one past the top-level group beside it. `⌘P` puts an item set out of order at the end of the run of
items at its new priority, so a list kept in priority order stays so. `⌥→` on one item makes it a
nested group of that name, where it stood and at its priority, with the field for its first item
open in it (an item an agent is on is kept as that first item, agent and all) — and
typed into a new item's field, `⌥→` at the end of the text does the same, so `⌘N`, a name, `⌥→`
starts a group. On a selection in one top-level group it puts them in a new group where they stood,
its name typed at once (`Esc` keeps **New group**). Groups go two levels deep: a top-level group
and the groups in it, and a group stays at its level (a list from before that nested deeper has its
deeper groups lifted into the top-level group they were under). `⌥←` takes items in a nested group
out to just under it, and on all of them flattens the group into its place.
(Ghostty sends `⌥←`/`⌥→` as `⌥B`/`⌥F`; both work.) `⇧↑`/`⇧↓` select a run of items, across groups
too, and ticking, `⌘P`, moving, grouping, deleting, `Enter` and cut/copy then act on them all.
`⌘X` cuts and `⌘C` copies: the clipboard gets their text, a line each, and that text pasted back
into the modal — any project's tab, or a new item's empty field — puts the items themselves right
after the cursor, priority kept.

A row is edited where it stands: typing on an item or a group header adds to its end, and `⌫` opens
it with its last character gone; `Enter` saves, `↑`/`↓` save and move on, `Esc` puts it back.
`⌘N` types a new item right under the cursor's (from a header or `+ new item`, at the group's
end) — `Enter` adds it and opens the next under it, `Esc` stops; the field wraps as you
type — `⌘⇧N` a new group: in a top-level group (on an item, or a nested group or its items) one
nested there right under the cursor's row, on a top-level header or `+ new item` a new top-level
group at the end, either way with its first item's field open in it — `⌘P` picks the priority (`1`–`4` urgent to low, `0` none) and
`⌘⌫` deletes, a group with items in it only after a second press.
`⌘↑`/`⌘↓` jump from group to group. `⌘F` opens the filter, which keeps the matching items under
their headers.

**Pasting lines in.** Paste several lines into the modal — a list copied out of a notes app — and
each one is an item, open and with no priority, right after the cursor's item; pasted into a new
item's field, they start where the field stands. A bullet (`- `, `* `, `+ `, `•`), a task box,
`**bold**` and a closing `;` come off each line, and blank lines are passed over.

**An agent on an item.** `Enter` opens the QUICK PROMPT over the modal, aimed at a fresh worktree,
the item's text in the box with `Context: todo in <group › subgroup>` under it. Edit it
and `Enter` sends it; `Esc` comes back to the list. `⇧Tab` picks one of your AGENT PRESETS
instead. The session it starts is written onto the item: its row shows `● agent`, in the session's
STATUS MARK colour, and from then on `Enter` — or a click on the chip — closes the modal and lands
on that session. When the session is deleted the chip goes, and `Enter` sends a new one.
