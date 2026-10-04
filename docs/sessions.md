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
run — **Claude**, **Codex**, **Cursor**, **Pi**, **Muse**, **Grok Build**, or **OpenCode** (a plain shell is `t` — see [Keys](keys.md)); a CLI you never use can be
switched off on the settings overlay's Agents tab and drops out of the menu entirely. Claude is
listed by the account it is signed in as, `Claude (you@example.com)`, and every other CLAUDE
ACCOUNT you have added has a row of its own right after it ([below](#usage-limits-and-a-second-account)). Turn on `Hide missing CLIs`
on the Agents tab and the menu lists only enabled harnesses whose CLI is found on PATH (the daemon still
checks through the login shell at launch). Your own CLIs join the menu too: add them to config.json
`custom_harnesses` (see [Configuration](configuration.md)) and they appear after the built-ins under their
own labels, each toggled in its own section on the Agents tab, with the session card wearing the
entry's label as its badge. A custom entry launches with its program and model flag, boots fresh every
time (no resume mapping), and — unless it names a built-in hook dialect — stays process-based: yellow
while the PTY is live, green when it ends, never red. `→` on any row drills
into model and reasoning-effort submenus (Cursor's model is a family such as `claude-opus-5-thinking`, and
its effort list follows the family, `-fast` variants included — `cursor-agent --list-models` bakes both
into the id, so orion launches `--model claude-opus-5-thinking-high-fast`; the list is a built-in seed
merged with `--list-models`, cached in `cursor_models.json` beside `config.json` and refreshed daily;
Pi's model is a fuzzy `--model` pattern — `opus`, `sonnet`, or a `provider/id` you set in `config.json` —
and its effort is the `--thinking` level, `off` through `max`).
In those submenus you type to filter — `opus` narrows the rows to the Opus families, `↑`/`↓` move, `Backspace` widens, `Esc` clears — and the preset editor's Harness / Model / Effort rows take the same type-ahead.
`Enter` anywhere takes your configured defaults. On the
Claude row — and on any row of the Claude model and effort lists under it, whose title then reads
`Claude model · cloud` — `Tab` toggles Cloud mode, the menu's bottom border naming the key and the
state it is in (`Tab cloud off`): enter the task in the wrapped editor
(`Shift+Enter`, `Option+Enter` or `Ctrl+J` adds a line) and orion launches `claude --cloud=<task>` — the value binds
with `=` and never a space, because `--cloud` takes an *optional* value, so a separate argv item starting
with `--` would be read as another Claude flag instead. The CLI creates the session, prints its URL and
exits — orion reads the session id off that output, and that is where the local side ends. The agent
runs in Claude's cloud sandbox, not in a terminal here, so the row wears a `cloud` badge and its pane is
the **CLOUD SESSION PANEL** instead of a terminal: a line saying so, and the session's
`https://claude.ai/code/session_…` link, underlined. The card reads like any other card's: the task
you typed is its first prompt (the `›` line, and every message you later **Send to cloud session**
joins the RECENT PROMPTS behind it), and the row takes the title Claude Cloud gives the session —
the `Created cloud session:` line of the same output — as its name, since no hook ever reaches
orion from the sandbox to let the session title itself; a name you typed stands, and `r` renames
it whenever you like. `Enter` on the row — or a click on the link — opens
the page in the browser. Nothing is attached, teleported or re-homed on your behalf, the checkout never
switches branch, and **Attach** and **Restart** are not offered: there is no local session behind the
row, and the daemon refuses to boot a bare `claude` in its name. To steer the cloud agent without a
browser, pick **Send to cloud session** from the card's right-click menu — the same wrapped editor — and orion
runs `claude -p <message> --cloud=<id>`; the reply lands on the session's page, the CLI never returns
one. Otherwise `Enter` on a row opens the QUICK PROMPT set to that harness, model and effort, aimed
at the checkout under the cursor: type the task and `Enter` launches it, or send the box empty and
the CLI starts bare, and you type the agent's first prompt there. Nothing asks for a name — the
session titles itself from that first prompt (AUTO-TITLE); `r` renames it whenever you like. `⌘N`
opens the same box straight away, on the Agents tab's harness (see [Keys](keys.md)).

The picker opens on the Agents tab's **Quick prompt › Agent** harness — the one `⌘N` launches — so
`Enter` at once is `⌘N`, whatever you picked last, unless **Remember harness**
is on (Settings → Experimental, `remember_harness` in CONFIG.JSON). Then every launch you walk
through this picker, the PR SESSION picker or the QUICK PROMPT's `Tab` picker writes its harness into
that row, and a model or effort you drilled into through the
submenus into that harness's own **Model** / **Effort** rows: the next **New agent — choose harness** opens on that harness with
its ✓ on that model, the box `Enter` opens is set to it, and `⌘N` launches it too. The rows are the ordinary settings,
so the Agents tab always shows what the next launch will be, and you can still change them there. An
AGENT PRESET launch leaves them alone — its harness is the preset's, not a change of mind.

The rows do not run under the same permissions, and the picker is where you decide that. Claude
is spawned with no permission flag at all and keeps its normal prompts — it stops and asks before the
things it is configured to ask about. Codex is spawned with `--yolo` and Cursor with `--force`, so
**neither of those two ever stops to ask**: they edit files and run commands on their own judgment for
the life of the SESSION, and nothing in the picker or the settings overlay softens that. Pi has no
permission gate to begin with — orion passes no flag, and it runs its tools as it sees fit. Muse is
the same: no flag mapped yet. OpenCode keeps its own permission prompts: orion passes no `--auto`, so it
stops and asks like Claude does, and a `permissions_flag` of `--auto` in its `harnesses` entry is how you
opt out of that. Pick the
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
watching one and waiting for it to ask you something, it is not going to. Pi has no shell hooks at all:
orion installs one managed extension (`~/.pi/agent/extensions/orion.ts`, inert outside orion) that
posts pi's `session_start`, `before_agent_start`, `agent_end` and `ask_question` tool events as
`SessionStart`, `UserPromptSubmit`, `Stop` and `PreToolUse` / `PostToolUse`, and any blocking prompt an
extension raises mid-run as `PermissionRequest` — so a Pi row goes yellow, red while its `ask_question`
tool waits on you, and green when the run ends, a cancelled run included. Muse has no hooks at all yet:
its row is yellow while the PTY is live and green when the process ends, and it never goes red. OpenCode
runs TypeScript plugins, so orion installs one managed plugin (`~/.config/opencode/plugins/orion.ts`,
inert outside orion) that posts OpenCode's `chat.message` as `UserPromptSubmit`, its `session.status`
/ `session.idle` as `Stop`, `permission.asked` / `permission.replied` as `PermissionRequest` and the
gated tool's `PostToolUse`, and its `question` tool's `question.asked` / `question.replied` as
`PreToolUse` / `PostToolUse` — so an OpenCode row goes yellow, red while a permission or a question
waits on you (one raised from a subagent session included), and green when the turn ends, an aborted
one included.

## AGENT PRESETS

If you keep starting the same kind of session with the same framing, save it as an **agent preset**:
**Agent presets** (the command palette, or `⇧Tab` in the quick prompt) lists them, and the launch lands in that card's checkout — its last row says which, and `Tab` flips it to a fresh worktree. Over the quick prompt the box's header says where instead, and its `⌘.` picks, so that row and its `Tab` are not drawn there. Type to find one by name: letters
narrow the list to the fuzzy matches, `↑`/`↓` move, `Backspace` widens and `Esc` clears — as in the
model and effort submenus. `Ctrl+a` opens a small form — name, harness, model, effort, **Text**
(which side of the task the preset's text goes: `prefix`, `postfix` or `prefix & postfix`, with a
box for each side it names) and **Task** (`ask` or `skip`) — and `Ctrl+e` / `Ctrl+d` edit or
delete; they are chords because plain letters type. A new preset starts on the side **Preset text**
in Settings → Sessions names (`preset_text` in CONFIG.JSON) — `prefix` unless you change it: one
box, sent before your task — and cycling the row shows the other box or both; a side the row leaves
out saves blank. Editing a preset also shows any side that already holds text, so one saved with
both never loses either. `Enter` on a
preset asks for the task in the same wrapped editor, then launches the CLI with `prefix + task + postfix`
as its very first prompt, so the agent is already working when the pane opens. The task is optional:
send the box empty and the prefix and postfix go on their own (a preset with neither starts the CLI
with no first prompt). Set **Task** to `skip` for a preset that never needs one — a "commit and push" —
and `Enter` launches it at once, no box at all; the list marks those rows `no task`. A `skip` preset
picked with `Shift+Tab` in a quick prompt or in the issues modal launches the same way when
the box is still empty, while text you already typed stays yours to send. The row it creates is an
ordinary session: it names itself on that first turn, resumes, and shows status like any other. Presets
live in `agent_presets.json` beside `config.json`. The form's Harness row lists custom registry entries
by id alongside the built-ins — a preset on one launches with the entry's program and defaults, and
refuses with the reason when its entry is switched off or gone.

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

`Enter` sends what you typed to the agent as its next turn and closes the box, with
`sent to <session>` in the footer; `Shift+Enter`, `Option+Enter` and `Ctrl+J` break a line, as in Claude
Code's own prompt, and `Esc` closes the box without sending. The text goes straight down the session's
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

Only a live local agent takes one. An archived session's turn is over, a Claude Cloud row's
agent runs in a sandbox with a message queue of its own (**Send to cloud session** in its menu), a shell
terminal takes typing in the pane, and a pull request row is not a conversation — each says so if you
ask. A session whose CLI is not up — reaped by the IDLE REAPER, or cold since the daemon started — is
booted first and the box left as it is with `starting <session>` in the footer: nothing is typed into a
process that is still starting, so press `Enter` again once it is up.

## Dropping a screenshot on a prompt box

Drag a file onto the QUICK PROMPT, an AGENT PRESET's task box or the FOLLOW-UP box, and the terminal
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

The QUICK PROMPT's text is saved to `quick_prompt_draft.txt` in the DATA DIR as you type it — each
change rewritten whole through a temporary file, nothing written when nothing changed. Closing the
terminal window kills orion without a clean shutdown, so there is no exit hook to rely on; with the
text already on disk, the next box opened after a restart starts from it, caret at its end, its
explanation line reading `draft restored · …` until the first edit. Within one run the box you
closed comes back the same way, with its harness and preset (`Esc` parks it).

Sending the box deletes the draft, and so does emptying it by hand. A launch the daemon refuses
comes back with its text, which is the draft again. Boxes that carry their own text — a refused
launch, a preset's task, the FOLLOW-UP box — never open on the draft, and a box with text of its own
writes nothing until you edit it, so opening one never overwrites what you were drafting.

## RECENT PROMPTS

Every session card carries the last thing its session was asked to do, condensed to one line and
clipped to the card, with a dim `30m ago` pinned to the right. The DAEMON keeps the newest ten prompts
per session behind it.

The text is the prompt as you typed it, not a paraphrase. The DAEMON reads it off the
`UserPromptSubmit` hook payload every harness sends (Claude, Codex and Cursor name it `prompt`; Pi's
managed extension and OpenCode's managed plugin post the same field), collapses its whitespace and keeps the first 200 characters,
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

A second Claude account is one more Claude Code config dir with a login of its own: **Add
account** under **Claude accounts** on the settings overlay's Agents tab (or **Claude accounts** in
the COMMAND PALETTE, or the step first-run onboarding has right after Agents) makes the dir, offers to
share your setup with it — `CLAUDE.md`, settings, skills, agents, commands, plugins, as links — and
`Enter` on its row signs it in with Claude Code's own `claude auth login`. Every account goes by the
name you gave it and the email it is signed in as, `Work (you@example.com)` — `Claude
(you@example.com)` until it has a name — and `r` on its row renames it, the default account too,
without moving its sessions or its dir. On a machine with more than one a card says which its
session runs on by that name, else that email (`Work opus high`), where it would say `claude`. A
removed account's dir stays, listed under **Saved on this machine** with who it is signed in as, to
add back or move to the Trash. See [Configuration](configuration.md#claude-accounts) for the
`claude_accounts` entry behind it, signing in and out, renaming and removing one.

Two accounts signed in to the same claude.ai login are one subscription with one limit — the
browser sign-in approves whichever account the browser is signed in to, so it is easy to end up
there. The Agents tab and onboarding flag it under the accounts, with the fix: sign one in again from
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
conversation yet, a transcript orion can't find, a Claude Cloud session or an archived one is
refused with the reason in the footer, before anything stops.

## The GRID

orion opens on a GRID of session cards — no modal over it, ever, on any launch. Starting a session
is one chord from there: `⌘N` opens the QUICK PROMPT, focused, so the first thing you type is the task.

- **The box** starts on the selected project and in the checkout under the cursor — the worktree
  whose band the cursor is on, open or collapsed; the root branch with nothing selected —
  launching the harness, model and effort the Agents tab defaults name. Its header spells them out,
  each field beside the key that changes it, where the launch runs over what runs it:
  `project demo ⌘P   worktree main ⌘.` over
  `agent Claude (you@example.com) Tab   model opus · latest ⌘/   effort high ⌘Y`. The effort is
  always there — `default` until one is picked — for every harness that has one; an AGENT PRESET on
  the box adds its own `preset … ⇧Tab` field; and a fresh worktree reads `new worktree <branch>` in
  green, the frame turning green with it. The keys are the ones the box answers to, the worktree,
  model and effort ones spelled from your keymap (`^T`, `^/`, `^Y` where the terminal never sends ⌘);
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
  then every checkout the project has, the one the box is aimed at ticked — and picks where this
  one launch runs, never switching a checkout's branch; a fresh worktree is that first row, the
  box's own toggle for it gone. `⌘/` opens the harness's model list straight away (`→` on a model
  reaches its efforts), `⌘Y` steps the effort in place — `default`, then the harness's list, round
  again — and `Tab` opens the harness picker — the lists over the box, as the project picker is,
  so the task stays in front of you while you pick what will run it. The three keymap keys work
  with any of the box's pickers open over it, and none of them closes the box. `Tab` again on the
  picker's Claude row — or in the Claude model list `⌘/` opens — is the new-session picker's Claude
  Cloud toggle: the box comes back as a cloud one (`agent Claude · cloud`) and Enter sends your text
  as the cloud task — not offered in a box for an issue or a pull request. `⇧Tab` takes a preset; a
  model or effort picked afterwards for the same harness keeps it. None of it needs the key: the
  header is a row of buttons, and a click on a field opens what the key printed beside it opens — on
  `effort`, the model's effort list — box and task still in front of you. Enter
  launches, and the cursor and the pane land on the new card as it goes up in its band, the grid
  scrolling to it and the keys still on the grid. Turn off **Follow new** under **Quick prompt** in
  Settings → Agents and nothing you were looking at moves instead: the cursor and the pane stay on
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
  walked: `↑`,`↑` on the top row of cards hands the keys up to the tabs, with a cursor of
  their own on the lit tab; `←`/`→` move it and the grid switches with it, each project
  shown on its last-focused card as the cursor passes, and `Enter` — or `↓`,`↓` back
  down, or `Esc` — hands the keys back to the cards of the project on screen. The `×` on a tab, or `Backspace` on the
  one under the header's cursor while it has the keys (behind a confirm), closes it and the grid
  moves to the tab that slides into its place. Closing a tab
  changes nothing about the project — its sessions run on. Closing the last tab puts orion back on
  the splash it opens on before there is any project, where `+` lists every project, `Enter` opens
  the repo orion was started in and `⌘K`'s last row another folder; the splash stays across a restart. The
  tabs are remembered across restarts. Each tab carries its project's STATUS DOTS right of the name — one
  per state its sessions are in, carrying that state's count and no word at all: red waiting on
  you, blue an unread finish, yellow mid-turn, in that fixed order and left out where a state is
  empty, so a quiet project is its bare name. The tab's name sweeps too, on the loudest of them:
  red while a session waits on you, else yellow while one is mid-turn, else blue while a finish is
  left unread — and holds still once the project is quiet. The sweep recolors the name in place,
  so no tab moves; the animations setting turns it off. The `+` in front of the tabs — or the
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
  into **BANDS**, one per checkout that has something running in it — the root first, then the
  rest most recently worked in first; a checkout with nothing running has no band — unless
  **Show all worktrees** (Settings → Sessions, on by default) is on, when every checkout gets
  one: an EMPTY BAND, its rule over a single line, `nothing running` — what can be done there is
  the footer's to say once the cursor is on it (`⌘N new agent · t terminal · ⌫ delete worktree`,
  and `→ focus PR` beside the pull request the pane reads). `↑`/`↓` walk onto it like any band, `⌘N`/`t` start work in
  that checkout, and `Backspace` (or **Delete worktree** in its right-click menu) deletes it behind the
  worktree's own confirm; the root's band offers no delete, since the root is never deleted. With the
  setting on, deleting a worktree's last card never asks about the worktree: the band stays,
  empty, until its own `Backspace` — unless **Delete emptied worktree** is on, which still deletes the
  worktree with its last card. Each band is a
  titled rule over one row of cards: the rule names the checkout in its scope color (`↳ feat`,
  `⌂ main` for the root — the project is the grid's own scope, named once in the header), with
  that checkout's uncommitted changes right behind the branch in the warning color (`↳ feat +3
  files`, just `+3` on a narrow rule, nothing when it is clean; the lines behind it follow in
  green and red, `+3 files +120 -45`), then its pull request —
  `↗ #42 Polish the nav  ready` in the colors the PR rows wear (red for conflicts or failing checks,
  purple once merged), a link while the pointer rests on it — and, at its right end, how many
  sessions and terminals are under it and how many cards the row had no room for (`▸ 2 more`).
  The band the keys are on opens on `❯` where the rest open on `──`, and the FOOTER says what
  Tab does there — `Tab expand`, or `Tab collapse` on the one band open as an accordion, every card
  of it wrapped into rows under the rule.
  A collapsed band whose row left cards off its edges says so on the row of air under the cards,
  centered — `▾ 2 more · see all 8` — where the eye leaves the row; a click on it opens the band as
  Tab does.
  The cards under it are the checkout's sessions, then its terminals as cards two columns wide,
  gap included, so their output has room — a one-column grid gives them the one (`❯ shell-1`
  with what runs in it, then the last lines its shell printed, asked of the daemon
  once a second while the card is on screen, so a glance down the grid says what each shell is up
  to; `▶` on a RUN TERMINAL, `exited` at the name's right once its shell is gone — the lines it
  went out on stay, so the card says what it was doing when it went). A session
  card is the session's name with its status dot and how long ago, what it runs on (`claude opus high`:
  the harness, its model and the reasoning effort it was launched with — a session on the CLI's own
  default effort stops at the model),
  then the last thing it was asked to do, on a `›`, over four rows so a sentence reads as one
  (what still does not fit ends in an ellipsis). The pull requests come from the same `gh` lookups
  the PULL REQUESTS MODAL makes, swept over every checkout the grid lists. The header counts the project's open pull requests and issues beside the session count
  (`4 sessions  3 prs · 2 issues`), so what is waiting on the repo is read without opening `v` or
  `i` to find it — and a click on either count opens that list, as the key does. When the room left
  cannot hold every card, the far right of that row says how many it could not draw and which way
  they went — `↓ 7 hidden` for cards under the fold, `↑ 4 hidden` for ones scrolled off the top,
  `↑↓ 5 hidden` for both. The count beside it still says how many sessions the project has, so a
  card that is not on screen reads as something taking its room rather than as a session gone, and
  the arrow says whether the way back to it is `↓` down through the grid or the pane's edge dragged
  back down. On a row the tabs leave little of, the counts give way whole, least needed first: the
  session count (each band's rule says its own), then the pr and issue counts, and this one last.
  An open band's edges say it again where the eye looks for the rest: `↑ 2 more
  above` on the row of air under the PROJECT TABS once the top has scrolled off, `↓ 3 more below`
  on a row kept under the cards while there is more past the bottom — the row stays as air once the
  grid is scrolled to its end, and neither appears on a grid that fits.
- **Walking it.** `↑` and `↓` walk the bands — the rule of the band under the cursor takes the
  accent and its branch goes bold, its remembered card (the session it was last left on, else its
  first) is raised out of the row — a heavy accent frame (`┏━┓`, a weight no status frame takes) over
  a gray fill — and the pane reads it; the accent goes with the keys, so with them up on the
  PROJECT TABS or down in the pane the rule is gray like the others, the card's fill drops a shade,
  and only the bold branch still says which checkout the pane reads. `←` and `→` walk the cards along the band's
  row, which scrolls under the cursor. `Tab` opens the band in place, like an accordion: every one
  of its cards wrapped into rows under its own rule — the sessions first, the terminals under a
  `terminals` rule — pushing the bands under it down, one band open at a time. On the open band
  the arrows walk the cards — `←` and `→` along a row, `↑` and `↓` down the
  rows, on from the last row of sessions into the terminals, and off the band's last row onto the
  next band — and `Tab` or `Esc` folds it back up with the cursor where it was. A collapsed band
  with more cards than its row can hold counts the rest on its rule rather than wrapping, so a step
  down is always a step onto the next checkout; open, there is room to wrap, and the whole grid
  scrolls by rows to keep the cursor's card on screen. `Enter` opens the card under the cursor in
  the pane, open band or collapsed. Which band is open is remembered per project — a trip to another
  project's tab and back finds it still open — and across restarts, and a jump that
  names a session — `⌘K`, the attention walk, a terminal just opened — lands inside its checkout.
  The wheel never walks the cursor, so a trackpad cannot swap the pane out from under the card you
  are reading: the grid scrolls the way a terminal's screen does — the wheel moves the bands three
  rows a notch under a cursor that stays put, held at the grid's ends, and a card the window's edge cuts is drawn to the edge rather than
  left out, so the window is full to its edges and no card-sized hole opens over the cards. The
  next key that walks the cursor, or `↑`/`↓` against the grid's edge, scrolls just far enough to
  bring the cursor's card whole back on screen — with the checkout's rule when it is on the first
  row — and a click on a cut card does the same.
- **The compact list.** Settings → Appearance → **Worktree layout** → `list` (`worktree_layout`)
  swaps each band's row of cards for a list: every session one line under the band's rule —
  its status dot and name, what it runs on, its last prompt — then its terminals, the shell's last
  line where a session has its prompt, the names and harnesses in columns down the band. Each band
  starts collapsed on its 3 most recent sessions (the one the cursor is on stays listed too,
  wherever it sits), with `▾ 2 more · see all 5` on the line under them; `Tab`, or a click on
  that line, opens the band to every entry and `Tab` or `Esc` folds it back — one band open at a
  time, as with the cards. A band that already lists everything has nothing to open, and `Tab`
  there says so. `↑` and `↓` walk the lines as one column, off a band's last line onto the next
  band's first; `Enter` and a click work on a line as on a card.
- **Every worktree open.** Settings → Appearance → **Expand all worktrees** (`expand_all_worktrees`,
  off by default) lays every band out open at once — each worktree's sessions and terminals wrapped
  into rows under its rule, or every line of the compact list — so there is no accordion: `Tab`, and
  a second click on a rule, open and fold nothing (the footer says so), the footer offers no `Tab` verb,
  and `Esc` has no band to close. `←`/`→` walk a row, `↑`/`↓` walk down every worktree's rows as one
  column — off a band's last row onto the next band's first, in the column the cursor was in. Which
  band `Tab` last opened is kept, and comes back open when the setting is switched off.
- **The pane** runs down the right side of the cards, full height and half the width — or along
  the bottom, under them, with the `⬓` button just before the `×` on its header (`◨` there moves
  it back) or Settings → Appearance → **Session pane** (`session_pane`, which the button writes; a
  window too narrow for it beside a column of cards lays it along the bottom until there is
  room) — and reads whichever card the cursor is on — on a collapsed band, its remembered
  card: `● polish-nav  ↳ feat` on its header, the card's name and its checkout, and that session
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
  the pane's own minimum one way and the header plus one row of cards the other — with the header's
  `↓ 7 hidden` counting whatever the cards lost the room for as you pull. The height you
  leave it at is remembered across restarts, and re-fitted to the window each frame. Beside the
  cards it is the edge facing them that drags, sideways, under a short `┃` grip, trading columns
  down to one column of cards — and that width is remembered apart from the height, so switching
  sides never turns one into the other. A double-click on either edge snaps it to the middle of the
  body, the cards and the pane sharing it evenly. On a terminal
  too short for the header, a row of cards and a pane worth the name, there is no pane and a session
  is only ever seen full-screen.
- **A follow-up** is `Space` on a card: a small modal opens over the grid and `Enter` sends what you
  type as that session's next turn, straight down its PTY. The pane is left exactly as it is — this is
  the way to hand a wall of sessions their next instructions one after another without opening any of
  them. See [the FOLLOW-UP COMPOSER](#the-follow-up-composer).
- **Stepping into one** is Enter on a card (or a double-click): the keys cross
  into the pane beside the cards, where that session is already running, with its input locked and
  the grid still up over it — the same place a click into the pane lands. `` ^` `` hands the keys
  back to the cards, and a second `` ^` `` folds the pane away.
- **Full-screen** is `^F` in the pane (or on a card), or the `⤢` in the pane's header: the grid and
  its header give way to the session, the keys still in it. `^F` again, `^q`, `` ^` `` or the `⤡` at
  the right end of the full-screen header brings it back down into the pane beside the cards, the
  keys still in it.
- **On a terminal too short to draw the pane**, stepping into a session gives it the whole screen
  instead — the grid gives way — with its input locked. Its header is a
  breadcrumb — `‹ sessions / ● Fix the login redirect loop`, with the harness, model and checkout
  right-aligned — and `^q`, or a click on `‹ sessions`, comes back to the grid with the cursor on
  the card you came from: there is no pane to come back down to. `⌘N` opens the box again, on the project under the cursor.

- **The project's own menu** — its verbs, which a session card has no room for — is a
  right-click on its PROJECT TAB: **New worktree**; **Run** / **Stop run** and **Open** for the
  checkout the grid would launch into, and **Delete worktree** when that is a linked one; **Rename**, a label only —
  the folder on disk keeps its name, and an empty name goes back to it; and **Remove from list**,
  behind a confirm, which leaves the clone on disk alone. There is nothing above the bands to walk
  out to: `Esc` lets the card go, a second does nothing, and `↑` on the first band stays put.

Every other key acts on the session under the cursor — `⌘⇧A` (`^A`) archives, `Backspace` deletes, `⌘E` opens its
diff, `⌘K` jumps, `s` opens Settings. `⌘⇧A` asks first, always: a CONFIRM DIALOG names
the session, `Enter` or `y` archives it and `Esc` or `n` keeps it, so a letter aimed at an agent
that lands on the grid archives nothing — and saying yes is cheap, since `⌘⇧U` (`^U`) brings it back. The
cursor lands on the card after the one archived in its band — the one that slides up into its
place — or, when the band's last card went, on the one before it; a band's only card leaving takes
the band with it, and the cursor lands on the one that slid up into its slot. `Backspace` lands
the same way — and when the card is the last one in a linked worktree (a lone terminal counts too,
and so does a **Delete all sessions** that takes every row), its confirm asks about the checkout in the same breath,
before anything is deleted: `Delete agent 'x'? Its session and history go away.` and under it
`Nothing else is left in worktree 'feature': delete it from disk too?`, with three answers. `Enter`
or `y` deletes the card and then the worktree, the way `Backspace` on its band would; `n` deletes the card
and keeps the checkout standing empty; `Esc` cancels, and the card stays alive. The ROOT WORKTREE
is never offered, and an archive (`⌘⇧A`) never asks: an archived card is still filed under its
checkout, so the dialog on a worktree that still holds archived sessions counts them, since the
delete takes their history with it. **Delete emptied worktree** (Settings → Sessions, off by
default) skips the question: the card's ordinary confirm says the worktree goes with it and `Enter`
deletes both — except when archived sessions are still filed under it, which always get the
three-way question. **Show all worktrees** takes the question away, not the setting: with both on,
the worktree still goes with its last card. A held `⌘⇧A` opens one dialog and
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

## The ISSUES MODAL and ISSUE SESSIONS

`i` lists the selected PROJECT's open GitHub issues — `gh issue list`, newest first,
pull requests left out — down the left of a modal, and reads the one under the cursor on the right:
number and title, who opened it and when, its labels, the description rendered as markdown (a newline
is a line break, as GitHub shows a comment), and,
once the cursor has rested on the row for a moment, its comments (`gh issue view`, one call per issue
you actually stop on, remembered for the session). `Ctrl+o` — or a click on the `↗ open in browser`
button pinned right on the reading pane's frame — opens the issue in the browser and `Ctrl+r` asks
GitHub again; a machine with no `gh`, or one that is not logged in, gets a line saying so in the pane
rather than an empty modal. The list is asked for before you press `i`: once the cursor has rested
on a project for a moment its open issues are fetched in the background, and re-fetched every couple
of minutes while the project stays selected (backing off when the repo has none, or `gh` can't
answer), so the modal opens on rows instead of an empty pane. A list that landed in the last thirty
seconds is what you see; an older one paints while the fresh list lands underneath — and the cursor
stays on the issue it was on, by URL, when a refresh retires a row above it.

The list filters as you type, from the moment the modal is up — the FILE FINDER's and the DIFF
VIEWER's way, no key to press first: letters narrow the rows to the fuzzy matches of `#15 title`,
best first, the cursor on the best with its comments asked for as any move asks for them, the matched
letters lit in each row and the title's count reading `2/14`. `↑`/`↓` (or `Ctrl+n`/`Ctrl+p`) walk the
matches, and `Enter`, `Shift+Tab`, `Ctrl+e`, `Ctrl+c` (or `Ctrl+y`) and `Ctrl+o` act on the issue you found. `Esc`
clears the filter, the cursor staying on that row, and a second `Esc` closes the modal, as in every
fuzzy overlay; a filter nothing matches says `no issues match` and leaves the cursor where it was for
the next letter or `Backspace` to decide. A refresh that retires the row under the cursor lands it on
the filter's next match, never on a row the filter hides. Because the letters are the filter's, the
verbs are chords — the diff viewer's rule — and `Ctrl+u` kills the typed filter before it scrolls the
pane.

`Ctrl+c` leaves a comment on the issue under the cursor without leaving the modal for long: a multi-row
box (the task prompts' shape, `Shift+Enter` for a newline) whose `Enter` posts the text as you —
`gh issue comment`, so it appears under your GitHub login — and puts the modal back on the row at
once, the pane saying the comment is on its way until GitHub answers; then the conversation is read
again with it in. `Esc`, or an empty box, puts the modal back without posting, and a post `gh`
refused (not logged in, no network) brings the box back with your text so nothing is lost.

`Ctrl+e` edits the issue itself without leaving the modal at all: the reading pane becomes a form on
the row's title and description — `Tab`, `↑`/`↓` or a click move between the two fields, and the
description takes `Shift+Enter` (or `Option+Enter`, or `Ctrl+J`) for a line break and `↑`/`↓` to walk
its lines, as every multi-row box does — the preset editor's prefix and postfix included.
`Enter` sends both to GitHub as one `gh issue edit` (the title on the command line, the description
on its stdin) and holds the form, its foot saying `saving…`, until GitHub answers: the row and the
pane then carry the new text at once, the list is asked for again underneath, and the footer says
`issue #15 updated`. `Esc` drops the draft and puts the reading pane back. An unchanged form closes
without a call, a blank title is refused on the spot, and a save GitHub refuses — not logged in, no
push access to the repo — keeps the form up with `gh`'s own reason on its frame and your text
intact, so nothing typed is lost. Labels, assignees and milestones stay GitHub's to edit.

Two keys put an agent on the issue. `Enter` opens the QUICK PROMPT for it — the same box
`⌘N` opens anywhere, titled `Quick prompt · issue #15 (claude · opus)`, launching the `Agent` row's
harness from Settings → Agents on the PROJECT's ROOT WORKTREE, whatever card the cursor is on. `Shift+Tab` opens the AGENT PRESETS list as a picker instead — the box's own key for it — over
the modal, which stays up under it (`Esc` goes back to the issue), and `Enter` on a preset hands the same box back with that preset's harness, model, effort and
prefix/postfix applied. Inside the box `Tab` and `Shift+Tab` still switch the harness or the preset,
and the WORKTREE PICKER's first row (`⌘.`) is a fresh worktree — named `issue-15-fix-login-redirect`
here, the number first and the title slugified, rather than a random name — and the issue survives
every one of those round trips. Send the box empty and the task is `Fix GitHub issue #15: <title> (<url>)`. The box
goes up over the modal rather than in its place — the list and the issue you were reading stay on
screen under it. `Esc`, or a click outside the box, puts you back in the modal on the same row; the
launch closes the modal as well, back onto the grid with the new session's card up in its band —
the cursor on the new card, or still on the card you were on with **Follow new** off.

Either way the launch is an ISSUE SESSION. The create carries the issue's URL
(`CreateAgent::issue_url`); the DAEMON validates it, keeps it with the AGENT row beside a PR
SESSION's URL, refuses to hand the launch to a PREWARM POOL spare (which booted without it), and on
every cold spawn and RESUME composes an issue-context rule naming the URL, the checkout and its
branch — Claude and Pi receive it through `--append-system-prompt`, Grok Build through `--rules`, and Codex, Cursor, Muse and OpenCode as the opening
of their first prompt, exactly as the PR rule travels. The harness therefore knows which issue the
session exists for before it reads your task, is told to read the issue with `gh issue view` first,
and to reference it in commits and close it from the pull request. The row it creates is an
ordinary agent from then on: auto-title, hooks, status, resume.

## The PULL REQUESTS MODAL

`v` is the ISSUES MODAL for pull requests: the selected PROJECT's open pull requests
down the left of a modal — newest first with the drafts sunk below the
finished ones, and drafts listed even while `hide_draft_prs` keeps them out of the `⌘K` jump list — and the one
under the cursor read on the right as the pane reads a pull request, its [PULL REQUEST
PAGE](keys.md#the-pull-request-page): number and title, state, mergeability, author, branches and
size over the tabs — **Description** (the body rendered as markdown, then the conversation),
**Changes**, **Commits**, **Checks** and **Reviews**, each with its count or verdict. `⇧←`/`⇧→` walk
the tabs and `⇧↑`/`⇧↓` a listing's rows. A row reads
the way the pull request on a band's rule does — a draft dimmed with a `draft` badge, one GitHub says
cannot merge red end to end with `conflicts` or `failing` — and the modal opens on the pull request
the pane is reading (after a `⌘K` jump onto one), when it reads one.

Nothing new is asked of GitHub to paint it. The rows are the project's open list the OPEN PRS beat
already keeps warm (and remembers across launches), so the modal opens on them at once; a list older
than thirty seconds is asked for again underneath, and the cursor follows its pull request by URL
when the answer reorders the rows or retires one. The reading pane shares the pane's fetch: a pull
request read in one is read in the other, and resting on a row for a moment fetches its body (`gh pr
view` — one call for the body and every tab) the same way. `Ctrl+r` asks for the list and the row's
body again now.

The keys are the ISSUES MODAL's, and the QUICK PROMPT box's: the list filters as you type, narrowing
the rows to the fuzzy matches of `#42 title`, `Esc` clears the filter before a second `Esc` closes,
and the verbs are chords. `Enter` opens the QUICK PROMPT for a
PR SESSION on the pull request — the box `⌘N` opens, titled `Quick prompt · PR #42 …`
— `Shift+Tab` launches one of your AGENT PRESETS on it, and `Tab` picks a harness and starts the session bare,
`→` drilling into the MODEL / EFFORT submenus. Every one of them is the same launch: a
`CreatePrAgent` that runs in the project's checkout of the pull request's head branch, reused when
one is there and cut by the DAEMON otherwise, its stand-in rows up under the pull request from the
moment you launch, and the PR's URL and work rule in the harness's context. The QUICK PROMPT stands
on the modal as the ISSUES MODAL's does: the list stays under the box, `Esc` or a click outside puts
you back on the pull request you opened it on, and the launch closes the modal onto the grid, the
new session's card up in its band as the ISSUES MODAL's launch leaves it. `Ctrl+c` (or `Ctrl+y`) opens the
COMMENT BOX on the pull request and comes back to the modal on the row — after `Enter` posts, after
`Esc`, and after a post `gh` refused, with your text back in the box — `Ctrl+g` opens the pull request's
whole diff — on the Changes tab at the file under the cursor, on Commits that commit's own diff, and a
click on a file or a commit does the same — `Ctrl+o` — or a click on the `↗ open in browser` button pinned right on the reading pane's
frame — opens it in the browser and marks it read on the way out (on the Checks tab, and on a click on
a check, the check's own page), and `Esc` closes the
modal (twice, with a filter typed). The hotkey is
rebindable (`pull_requests`).
