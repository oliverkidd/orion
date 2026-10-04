# How it works

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

- **Relative links in the host terminal.** On local sessions, orion reports the displayed
  agent or shell's checkout root through OSC 7, so the host terminal can resolve relative
  file links. With no attached session it reports the selected checkout; on exit it restores
  orion's launch directory. This follows checkout changes, not `cd` inside a child shell.
  SSH sessions leave the host's directory unchanged rather than label remote paths as local.
- **Detached daemon (tmux-style).** A background `orion` daemon owns every PTY, so agents keep running
  when the TUI closes. The TUI is a client that attaches over a unix socket (`$XDG_RUNTIME_DIR/orion/`
  or `/tmp/orion-<uid>/`, mode 0700). Quit the TUI, relaunch later, and your sessions are still alive
  with scrollback replayed. When the daemon swaps the process under a session you are looking at — a
  restart, or the `orion worktree` relocation at the end of a turn — the pane is rebound to the new
  one on its own. Moving the cursor onto a live session — a card in the grid, or a worktree or
  project switch that brings one back — attaches it on the keypress; only a session the
  idle reaper took waits a moment, so that walking past its row doesn't boot a CLI. The screens of the
  last six sessions shown are kept, so returning to one paints on the same frame and fetches only the
  bytes it missed instead of replaying the whole ring. Between them they may hold about 12 MB of grid;
  past that the oldest give up their scrollback and keep only the screen (a fiftieth of the size), and
  scrolling up in a pane that came back that way replays its ring once to get the history back.
- **A key never waits on git, the disk or the DAEMON.** Everything a keypress can start that takes
  longer than a frame runs off the event loop and lands when it is done. The DIFF VIEWER (`⌘E`), the
  FILE FINDER (`⌘P`), its grep view (`⌘⇧F`) and the TREE BROWSER (`⌘B`) open on the keypress and fill in
  when `git status` / `git ls-files` answer — what is typed meanwhile is kept and applied — and a
  file's diff, a search and a preview are read on the blocking pool: the pane keeps what it showed
  for up to 60 ms, which is longer than a read takes, and says `loading…` past that. The DIFF VIEWER
  opens on the list the changed-files badge's last `git status` found (two seconds old at most; its own
  `git status` still runs, and the reader keeps their place when it lands), so the first diff is being
  read while the list is checked rather than after — on a ten-thousand-file checkout `⌘E` went from
  260 ms of frozen UI to a list in 2 ms and a diff in 60. It reads the row after the cursor ahead and
  keeps what it has read (2 MB at most, gone with the modal), so `↓` paints the next diff on the
  keypress and re-reads it behind. List filters rank with `orion-fuzzy`, a crate of its own only so
  that a dev build compiles it optimised: 27 ms a keystroke over ten thousand paths became 5. `git grep` waits 40 ms for the
  next character, streams, and is killed at 200 hits or when the query moves on. A browser `open` and
  a clipboard `pbcopy` are started and left to finish. Rename, archive, unarchive, delete and close
  are OPTIMISTIC UPDATES: the row changes on the keypress, by way of the same upsert or removal the
  DAEMON is about to broadcast, and an Error puts it back and says why. FRAME PACING is a token
  bucket rather than a fixed 16 ms tick — three frames may go out 2 ms apart, a token comes back
  every 16 ms — so a key's frame and its answer's follow each other, while sustained PTY output still
  paints at 60 fps; a key that only goes to the PTY paints nothing of its own; and the DAEMON flushes
  PTY output that breaks a silence at once instead of holding it 5 ms to coalesce. A typed character
  echoes in 2 ms in a release build (3.5 ms in a debug one), where it was 20 (25).
- **…and that is measured, not felt.** `ORION_PERF_LOG=<file>` turns on the INPUT LATENCY PROBE: one
  JSON line per input (how long its handler held the loop), per frame (draw time, what it showed, and
  how long each input waited for it) and per DAEMON event. `make perf` drives the real TUI through
  every view, modal and verb inside a private tmux — isolated daemon, a clone of this repository as
  the checkout, a stand-in agent with a full 1 MB ring — and prints handler / paint / settle / echo
  per step plus the peak RSS of the TUI and the DAEMON; `python3 scripts/perf/report.py BEFORE AFTER`
  compares two runs. A change to anything on a key path is judged by that table.
- **Every pane is the same truecolor terminal.** A session paints orion's own grid, not the terminal
  orion runs in, so the daemon tells each child `TERM=xterm-256color` and `COLORTERM=truecolor` and
  drops any `NO_COLOR` / `FORCE_COLOR` it inherited. An agent launch runs through your login shell
  (`$SHELL -l -i -c 'unset …; export …; claude …'`) so it sees your real PATH — and your aliases and
  functions: a `claude` that `.zshrc` reroutes through a wrapper launches exactly as it does when typed
  — and restates all three after your profile has run, along with the harness's own `env` (a second
  Claude account's `CLAUDE_CONFIG_DIR`), so a profile can't undo it. Claude Code takes a stray `NO_COLOR` —
  exported by the agent shell the daemon was first started from, or by a login-only profile no
  interactive terminal sources — as "no colour" and paints its whole UI in the default foreground while
  the TUI around it stays coloured; the TUI still honours its own `NO_COLOR` on the way out, so a user
  who wants none keeps none.
- **Client and DAEMON must agree on the PROTOCOL VERSION.** IPC frames are positional msgpack, so any
  change to the shared types bumps `PROTOCOL_VERSION` (`crates/orion-core/src/protocol.rs`) and the
  handshake refuses a mismatched pair — the DAEMON answers `Incompatible`, the TUI bails, and the
  VERSION SKEW message names both binaries. Which side is stale decides the fix, and getting it
  backwards costs an afternoon: when the DAEMON is the *older* build, `orion kill` and relaunch is the
  whole remedy (it stops every live session on the way). A DAEMON that can't take the `Shutdown`
  request gets SIGTERM instead, at the pid the kernel reports on the other end of the socket or, when
  it can't say, at the pid in its pidfile — which can't come first, because macOS's tmp cleaner deletes
  regular files in `/tmp` after three idle days but spares sockets, so the DAEMON touches its pidfile
  hourly and puts it back if it vanishes. When the DAEMON is *ahead* of the `orion` you
  just ran, `orion kill` does nothing for you — a live instance respawns its DAEMON from its own
  binary, so the skew survives every restart, and the fix is to install the DAEMON's build over yours
  (`make install` from that checkout) instead. The usual shape in a checkout is a `make dev` DAEMON out
  of `target/debug` while your PATH still finds an older `orion` from the last `make install`.
- **RECENCY ORDER stamps every row.** A session is stamped when it last did anything, a worktree carries
  the newest stamp of its sessions, and a project the newest of its worktrees — which is why the lists
  sort themselves most-recent-first. The one fixed seat is the ROOT WORKTREE, always the first worktree
  row.
- **Projects → worktrees → sessions.** All work happens in the main checkout or a git worktree.
  Worktrees are real (`git worktree add/remove`), created under
  `<repo>/../<repo-name>-worktrees/<branch>` and branched from the freshly fetched `origin/HEAD`
  unless `orion worktree --base` names another start — a branch origin has means origin's fetched
  copy, so `--base main` is `origin/main` and never the checkout's local `main` (no `origin`, or a
  fetch that fails: the checkout's HEAD). The `worktree_base_branch` SETTING (Settings → General)
  is a standing `--base` for every worktree nobody names one for — `master` for a repo whose
  default branch is not what origin says, or that has no origin — resolved the same way, and
  falling back to `origin/HEAD` in a repo that has no branch of that name.
- **A branch reads as what it added — the COMMIT LIST.** The DIFF VIEWER (`⌘E`) lists the
  checkout's own commits, newest first: `git log --first-parent --no-merges <merge-base>..HEAD`,
  measured against the base worktrees are cut from — the `worktree_base_branch` SETTING's branch
  (origin's copy first), else `origin/HEAD`, else the branch the ROOT WORKTREE is on — as the
  checkout last fetched it, since the list never fetches. First parents walk the line the branch was
  committed on and never into a merged branch's history; `--no-merges` drops the merges themselves:
  a branch that merged `main` in to keep up lists only its own work. HEAD is all it needs, so a
  detached checkout lists the same way. Over the commits sits **Uncommitted changes** while the
  checkout is dirty, the view `⌘E` always opened on and still does then; a clean checkout opens with
  every commit ticked. What is on screen is a scope: the cursor's row with nothing ticked — a
  commit against its first parent, its message heading each diff — or the ticked rows, one at a
  time or TOGETHER. Together, rows side by side on the branch are one range, `git diff <first
  parent of the oldest> <newest>` (the working tree, for the uncommitted row); an unticked commit or
  a merge between two starts another range, and a file two ranges touch reads as each range's diff
  in turn, so a commit left out never comes back in a diff that runs across it. A range down to the
  branch's first commit is taken from the merge-base at its newest end instead, which keeps it one
  diff across merges of the base — every commit ticked is exactly the branch's diff. Each scope's
  file list is read before it goes up, and every diff walked is taken against it. The list is read
  in the same background job as the viewer's `git status`, and a branch past 50 commits is read 50
  at a time, the next page when the cursor reaches `… N older commits` — `git log` diffs each
  commit it lists to count its lines, so three hundred commits are never diffed to show six. A pull
  request's diff has no commit list: `gh pr diff` hands it over whole. One of its commits opens on
  its own (`event_loop::open_pr_review`) when the project's repo has it.
- **The diff pane is read, not paged — `diff_doc`.** A file's `git diff` is read once, as it lands,
  into lines that say what they are: its headers into the facts on the pane's edge (added, renamed
  from, `+20 −3`), each `@@` into the line it starts on, each line of code into its old and new
  numbers and its syntax runs. Drawing wraps every line at the pane's width — code at a word where
  that leaves the row half full — and the pane scrolls by rows, counted once per width and cached,
  so a 20 000-line diff scrolls as cheaply as a short one.
- **Worktrees made outside orion show up anyway — WORKTREE SYNC.** Every 2 s the DAEMON mtime-probes
  the git files a worktree operation touches — the repo's shared `.git/HEAD`, the `.git/worktrees`
  directory, and each linked checkout's own `HEAD` — and only when the newest of those stamps has moved
  does it spend a `git worktree list` and reconcile the rows. So an agent that runs `git worktree add`
  itself, a `git checkout` you did in another terminal, or a worktree someone removed lands on the
  grid within a couple of seconds without a restart, while an idle repo costs nothing but a few
  `stat` calls (`ORION_WORKTREE_SYNC_MS` overrides the 2 s beat; the e2e tests turn it down to
  100 ms). This structural sync is the *only* git polling the DAEMON does — the pull request lookups
  further down are the TUI's own.
- **The root checkout changes branch in place — the BRANCH SWITCHER.** `c` on a root-branch card (or
  with no card selected) lists the repo's branches with one `git for-each-ref` and switches with
  `git switch`, asking first when the checkout has uncommitted changes whether to stash, bring along,
  commit or discard them. That git is the TUI's, like the diff viewer's: the writes and the background
  `git fetch --all` run in a session of their own with stdin closed, so an `ssh` passphrase prompt fails
  instead of painting over the screen, and the TUI renames the root row the moment git says yes. The
  WORKTREE SYNC above then sees the moved `.git/HEAD` and confirms the branch from the DAEMON's side.
  Linked worktrees don't switch: each is named after the branch it was cut for.
- **A worktree's outside resources are yours to hook — WORKTREE HOOKS.** `git config
  orion.worktreeCreateHook` / `orion.worktreeDeleteHook` name an executable the DAEMON runs after it
  creates or removes a checkout, from the main repository, with the repo path and the worktree path as
  its two arguments — so a project can claim a dev-server port or a Caddy route on create and release
  it on delete. The hook runs after the git operation and the row change have gone through, still
  under the worktree lock — so hooks never overlap and a create of a path waits for the delete hook
  releasing it — and under a 30 s timeout that kills the hook and everything it started; a failure is
  a warning in every client, never a rolled-back create or delete. Per repo in git config rather than in CONFIG.JSON, and never a file inside the
  checkout. See [Configuration](configuration.md#worktree-hooks).
- **A worktree runs its own project — the PROJECT FILE.** A committed `.orion.json` names a `run` and
  an `open` command; the Project tab of the SETTINGS OVERLAY can hold either instead, per project in
  `config.json`, and wins over the file while it is set. **Run** in a card's or the project tab's
  right-click menu has the DAEMON start `run` in a RUN TERMINAL — a terminal row
  that carries its command and spawns `$SHELL -l -i -c '<run>'` instead of an interactive shell — so the
  PTY's life is the worktree's RUNNING state, broadcast as that terminal's `alive` and drawn as its
  card's green `▶`; **Stop run** kills the process tree and drops the row. The idle reaper and the prewarm
  sweep leave that terminal alone, and a run that exits on its own keeps its PTY, so an attach replays
  the ending instead of respawning — a command starts only when you pick **Run**. `⌘O` → **Open command** runs `open`
  once, from the TUI. See [Configuration](configuration.md#the-project-file-orionjson).
- **Agents boot `claude`, `codex`, `cursor-agent`, `pi`, `muse`, `grok`, `opencode`, or a custom registry program.** Creating an agent through **New agent — choose harness** first asks which CLI to
  run, then opens the QUICK PROMPT set to it, and the launch spawns it in the worktree. Claude's picker can also dispatch a one-shot Cloud task as
  `claude --cloud=<task>`; because Claude accepts that description as a process argument, don't put
  secrets in the Cloud task. That CLI prints the new session's id and exits, and the DAEMON reads the
  id off its output — the agent then runs in Claude's cloud, so the row's pane is the CLOUD SESSION
  PANEL linking to it, and nothing is ever attached, teleported or restarted locally in its name.
  Restored agents resume with `claude --resume <session-id>` /
  `codex resume <session-id>` / `cursor-agent --resume <session-id>` (falling back to a fresh session
  when the old one is gone) / `pi --session-id <session-id>` (which creates a missing id instead of
  dying) / `opencode --session <session-id>` / `grok --resume <session-id>` (orion does not capture
  Grok's id yet, so there is rarely one to resume); `muse` always boots fresh (no resume flag mapped yet). A session's id is saved only once a turn has run in it — the CLI writes the transcript a
  resume reads on the first prompt — so a CLI booted and never used resumes as nothing. Claude
  ids are checked against the transcripts on disk before the spawn — in the config dir a harness's
  `env` pins, for a second Claude account — and one with none boots fresh;
  any resume that exits with an error within 10 s of its spawn is respawned fresh, unless its Claude
  transcript is still there (then the id is kept, and the pane shows why the CLI quit). A Claude
  session sent to Claude's own background (`/background`) is the exception to resuming: its worker
  keeps the pane's `ORION_*` env, so hooks — and the forked session id — still land on the same
  row, but `claude --resume` refuses a session that runs in the background. When Claude holds a job
  dir for the id (`~/.claude/jobs/<first 8 of the id>/`), the DAEMON asks `claude agents --json`
  before the spawn, and an id listed as a `background` session opens as `claude attach <id>`
  instead — the live conversation, which detaching (Ctrl+Z) or archiving the row leaves running. A
  refused resume the job-dir look missed is asked about the same way and re-opened attached. An AGENT created from the PULL REQUESTS MODAL also receives the PR URL and a PR-only
  work rule — Claude and Pi through `--append-system-prompt` and Grok Build through `--rules` on every spawn, Codex, Cursor, Muse and OpenCode as the first prompt of
  their cold spawn (their transcripts carry it through a resume); orion persists that URL. An AGENT
  launched from the ISSUES MODAL (`i`) carries the GitHub issue's URL the same way — persisted with
  the row, rebuilt into an issue-context rule on every spawn and resume — so the harness knows which
  issue the session is for (see [Sessions](sessions.md#the-issues-modal-and-issue-sessions)).
- **Status via agent-CLI hooks, not MCP.** At agent spawn, orion merges managed hooks into the
  worktree's `.claude/settings.local.json` (Claude Code) or `.cursor/hooks.json` (Cursor CLI), and into
  `~/.codex/hooks.json` (Codex — codex records hook approvals against the hook file's path, so a
  per-worktree file would re-prompt forever; from its home, you approve orion's hooks once at codex's
  "Hooks need review" prompt and every later worktree is silent). Groups are tagged `_orionManaged`,
  user hooks preserved, rebuilt each spawn. Each hook is a fail-soft curl to the daemon's loopback HTTP
  endpoint, authenticated with a per-boot bearer token injected into the agent's environment only.
  The checkout's own files (`.claude/settings.local.json`, `.cursor/hooks.json`,
  `.cursor/rules/orion-title.mdc`) are listed in the clone's `.git/info/exclude` as orion writes them —
  local to the clone, never committed — so they never show in `git status` or the CHANGES view.
- **…plus the progress bar, for the cancel no hook reports.** Escaping out of a turn fires no `Stop` and
  suppresses the idle notification that normally un-sticks one, so orion also reads the CLI's terminal
  progress-bar escapes (OSC 9;4) straight off the PTY. That signal survives a cancel, and it stays busy
  while a permission prompt is open — so it can't mark an agent done while it is actually waiting on you.
- **…and the IDLE PROMPT, which is a hold rather than a finish.** Claude posts a
  `Notification{idle_prompt}` after roughly 60 s parked at the input box with nobody touching the
  keyboard, and that is the notification which un-sticks a turn that ended without a `Stop` — a
  rejected prompt, an escape mid-turn. Since Claude Code 2.1 the Agent tool runs subagents in the
  *background*, so it also fires while workers are still going: the foreground turn ended and the input
  box came back, but the session is anything but done. So orion treats an IDLE PROMPT as a hold —
  exactly the hold a gated `Stop` gets — whenever any subagent is still tracked, and only a set that
  has gone quiet is ever presumed orphaned and finished on the strength of it.
- **…and the USAGE LIMIT, which is a stop that waits on you.** A Claude turn that ends on an API
  error fires `StopFailure` instead of `Stop`. When the error is the account's — a usage limit, a
  billing stop, a held account — the row goes red with the limit recorded on it as the reason,
  ahead of the status edge, so the FEEDBACK SOUND's desktop notification can name it; any other
  error ends the turn like a `Stop`. The stopped turn's own progress clear and the idle
  notification a minute later leave the row red; the next prompt or foreground tool call — Claude
  Code's own continuation at the reset among them — takes it out of red, and the limit with it.
  **Continue on** carries the session to another Claude account: its transcript is copied into
  that harness's config dir and resumed there. See [Sessions](sessions.md#usage-limits-and-a-second-account).
- **The STOP GATE's four graces, on a 30 s tick.** A `Stop` (or an IDLE PROMPT) is held while
  `SubagentStart`s outnumber `SubagentStop`s, and a recheck every 30 s — fixed in the DAEMON, with no
  knob to turn it down — decides what becomes of the hold. Once the set drains and stays empty for
  180 s the session is finished; a `SubagentStart` that lands within 30 s of a finish instead heals it
  back to running, on the reading that the `Stop` raced that subagent's own POST. When the set never
  drains, a subagent that has shown no sign of life for 30 min — no `SubagentStart`/`SubagentStop`, no
  subagent tool traffic — is presumed killed and the turn finishes anyway. That last grace is why a
  session whose worker died can sit yellow far longer than you expect, and it is generous on purpose:
  one silent `cargo test` can run for many minutes, and a wrong green is the bug it exists to prevent.
  An individually tracked subagent older than 2 h is dropped from the set outright.
- **Answering is a hook too — just not its own.** Approving a permission prompt fires nothing: the
  gated tool simply runs, and its `PostToolUse` is the first word that you said yes. So the
  `PostToolUse` group is unmatched — every tool's end reaches orion — and a tool event from the same
  origin as the open dialog (the foreground turn, or the one subagent whose prompt it was) moves the
  row from red back to yellow; another subagent's traffic says nothing about a dialog it did not
  raise. An `AskUserQuestion` is answered only by that tool's own `PostToolUse`: Claude runs a question
  alongside the other calls of the response that asked it, so a read-only `Bash` or `Read` batched
  beside it finishes with the question still on screen, and its tool events leave the row red. The
  one signal orion treats with suspicion is Claude's `permission_prompt` notification: Claude sends
  it from a timer once a dialog has sat 6 s with no keystroke, detached from the turn, and its
  question dialog sends the same type — so one can land just *after* the answer that closed the
  dialog. Inside 5 s of the row leaving red that notification is taken as the echo it is and
  ignored; a genuinely new dialog announces itself through `PermissionRequest` or `PreToolUse`
  first, never through that notification alone.
- **Which of those signals you get depends on the harness.** Claude is installed with all ten hook
  groups — `UserPromptSubmit`, `Stop`, `SessionStart`, `PermissionRequest`, `Notification`, a
  `PreToolUse` on `AskUserQuestion`, an unmatched `PostToolUse` (the question's answer, a permission
  prompt's approval, and the cwd probe that re-homes a session that moves seconds later instead of
  at the turn's `Stop`), `SubagentStart` and `SubagentStop`, plus `StopFailure` (the USAGE LIMIT).
  Codex gets six of them: no `Notification`, no `StopFailure` and neither `*ToolUse` group, because
  it has no `AskUserQuestion` tool and its native
  `PermissionRequest` already covers waiting on you — which also means a Codex row approved out of a
  permission prompt stays red until the turn ends. Cursor gets five camelCase events —
  `sessionStart`, `beforeSubmitPrompt`, `stop`, `subagentStart`, `subagentStop` — and no permission
  event at all; orion runs `cursor-agent --force`, so waiting-on-you is simply not detectable there
  and a Cursor session never reaches NEEDS FEEDBACK, only busy or idle. Pi runs TypeScript extensions
  instead of shell hooks, so orion writes one managed extension into its global agent dir
  (`~/.pi/agent/extensions/orion.ts`, or `$PI_CODING_AGENT_DIR/extensions/` — global because pi loads
  those without the trust prompt a per-project `.pi/extensions/` raises) that maps pi's events onto the
  same names: `session_start` → `SessionStart`, `before_agent_start` → `UserPromptSubmit`,
  `agent_end` → `Stop` (it fires on an abort too, so a cancelled pi turn goes green on its own), the
  `ask_question` tool's start and end → `PreToolUse` / `PostToolUse`, and a blocking extension prompt
  mid-run → `PermissionRequest`. The file is env-guarded, so a `pi` you run outside orion loads it and
  does nothing. OpenCode runs TypeScript plugins the same way, so orion writes one managed plugin into
  its global config dir (`~/.config/opencode/plugins/orion.ts`, or `$XDG_CONFIG_HOME/opencode/plugins/`
  — globbed at startup with no trust prompt, and always on the list whatever `OPENCODE_CONFIG_DIR` adds)
  that maps its server events onto the same names: `chat.message` → `UserPromptSubmit` (the typed text
  along for RECENT PROMPTS), `session.status` idle / `session.idle` → `Stop` (an abort ends the same way,
  so a cancelled turn goes green on its own), `permission.asked` → `PermissionRequest` and
  `permission.replied` → the gated tool's `PostToolUse` (the one hook an approval fires), and the
  `question` tool's `question.asked` / `question.replied` → `PreToolUse` / `PostToolUse`. A subagent
  session's prompts post under the root session's id with the child as the origin, so its answer is its
  own next tool event. The file is env-guarded like pi's.
- **Sessions title themselves.** Create a session with the default name and the agent renames it after
  your first prompt — a 3-4 word title describing the ask (e.g. `Fix Login Redirect`), via a
  `orion rename <title>` command the CLI runs in its own turn (no extra API calls, no MCP server).
  Claude Code and Codex get the instruction injected through the `UserPromptSubmit` hook response — as
  `hookSpecificOutput.additionalContext`, the one envelope both read (the daemon sends it only while the
  session is untitled) — Pi's extension reads the same envelope and appends it to that run's system
  prompt, and OpenCode's plugin carries it into the turn's system prompt through OpenCode's
  system-transform hook; Cursor gets a managed `.cursor/rules/orion-title.mdc` project rule instead,
  since its hooks can't inject context. Titling is one-shot and never clobbers a name you typed or set
  with `r` — a late agent attempt is politely declined. `orion rename --force` overrides.
- **A Claude session's own name and its row stay tied.** `/rename <name>` inside Claude Code retitles
  the row within a moment — the same name then shows on the session's card, Claude's prompt box, its
  `/resume` picker and `/rc` list, and survives a restart. Claude fires no hook for `/rename`; it
  rewrites the window title (`✳ <name>`) and writes `custom-title.json` beside the transcript, so the
  DAEMON reads that file when the PTY's title changes (and on every hook), and adopts a title Claude
  did not hold before as if you had pressed `r`. The other way round, a name set in orion — typed at
  creation, set with `r`, or chosen by AUTO-TITLE — reaches Claude on your next prompt through the
  same `UserPromptSubmit` hook reply, as `hookSpecificOutput.sessionTitle`. Whichever side changed
  last wins; a name you set in orion is never undone by re-reading Claude's older one. Claude only —
  Codex and Cursor have no session name of their own.
- **A Claude session's card names the model it is on.** Switch with `/model` inside Claude Code and
  the card follows within a second (`claude opus` → `claude fable`, `[1m]` kept for the 1M context
  window). Claude fires no hook for that either; it writes `Set model to …` into the transcript at
  once, and a `model` line naming the exact id with each session's first prompt. The DAEMON checks the
  size of every live session's transcript each second and reads only what was appended since. The
  new model is what the row stores, so a restart or resume comes back on it and `orion spawn`
  inherits it. A session launched on the default model gets its model named after the first prompt.
  Built-in Claude only; a row on an exact id (a Bedrock id from `claude_models`) keeps it while
  Claude stays in that family.
- **Cards show what they were last asked.** The `prompt` field of the `UserPromptSubmit` payload —
  which every hooked harness sends — is condensed to one line in the hook receiver, kept on the AGENT
  row (the newest ten, in SQLite), and the newest is drawn on the session's card with an ago label.
  Pure capture: nothing is injected into the
  model's context and no extra turn runs. See [Sessions](sessions.md#recent-prompts).
- **The header counts what is waiting on the repo.** The GRID's header says
  `4 sessions  3 prs · 2 issues` for the
  selected project, the pull requests in the accent the PULL REQUESTS MODAL's rows wear and the
  issues in green — the open
  pull requests the OPEN PRS sweep already keeps warm for every project (drafts left out while
  `hide_draft_prs` is on), and the open issues, which a sweep of their own asks for one project per
  tick on a five-minute beat. Zero says nothing, and a narrow column
  drops the badge before the name. Each count is a button: a click opens the PULL REQUESTS or
  ISSUES MODAL for that project, as `v` and `i` do. See [Configuration](configuration.md#every-setting).
- **Ask the agent for a worktree and it moves there.** Tell a Claude session "do this in a worktree" and
  it runs `orion worktree <name>` instead of its own `EnterWorktree` tool (whose checkouts land under
  `<repo>/.claude/worktrees/` on a `worktree-*` branch). orion creates the checkout in its usual
  `<repo-name>-worktrees/<branch>` spot — or takes the existing one for that branch — re-homes the
  session's row under it at once, and the moment that turn ends restarts the CLI resumed inside the
  worktree, opening with a note saying where it now runs, so the conversation carries on there without
  you typing anything. Claude learns the rule from a short `--append-system-prompt` orion passes at
  spawn, plus a `Bash(orion worktree:*)` permission so the command never prompts; Pi gets the same
  appended prompt and reopens on the same note, and Grok Build gets it through `--rules`. Codex, Cursor, Muse and OpenCode have no system-prompt flag to learn
  the rule from, but run the same command when you ask. Codex then reopens on the same note —
  `codex resume <id> --cd <worktree> "<note>"`, the `--cd` because Codex otherwise reopens a resumed
  session in the directory its transcript recorded, the old checkout. Cursor and OpenCode resume silent
  and wait for your next prompt (`opencode --session <id> --prompt "<note>"` loads the session but never
  submits the note); Muse reboots fresh with no note (no resume flag mapped). The restart is the only way there: an agent CLI can't `cd` out of the
  directory it was started in.
- **Ask the agent for another session and it starts one.** Tell a Claude session "start a new orion
  session that fixes the login redirect" and it runs `orion spawn "<task>"`: the daemon starts a second
  agent beside it — same worktree, same harness, model and effort unless `--kind claude|codex|cursor|pi|muse|grok|opencode`
  names another — opening on that task as its first prompt, so it is working before you look. The new
  card appears on the grid on its own (default name, so it titles itself), and the session you
  asked from is untouched: no restart, no focus change. Claude learns this from the same appended system
  prompt as the worktree rule, plus a `Bash(orion spawn:*)` permission.
- **Ask the agent to show you a file and it opens in orion.** Say "open it" or "show me the examples"
  and the session runs `orion open <file>…`; every TUI attached to the daemon raises its file tabs on
  them — a modal with one tab per file, the focused one previewed with syntax highlighting (a
  markdown file as a rendered page), `Enter`
  editing it in place — so the agent puts the file in front of you instead of pasting it into the
  reply. Only when you ask: the appended prompt forbids opening anything unprompted, so an agent that
  wants you to look at its work names the path and waits. And text only: the CLI resolves the paths
  against the session's own directory and refuses a path that isn't there or isn't a text file (a NUL
  byte in its first 8 KiB, git's own test — a terminal has nothing to show for a PNG); the daemon only
  checks the caller is a known session and passes the agent's checkout along as the editor's working
  directory. Same appended prompt, plus a `Bash(orion open:*)` permission.
- **Everything persists in SQLite** (`~/.local/share/orion/orion.db` or the platform equivalent):
  projects, worktrees, agents (with kind + CLI session ids), links, and your last selection.
- **Sessions warm up, then get reaped.** The daemon can pre-spawn an agent CLI in the selected worktree
  before you ask for one, and pre-boot a worktree's dead sessions while your selection rests on it, so attaching
  lands on a booted screen instead of a booting shell. To bound what that costs, idle PTYs in worktrees
  no client is watching are killed after `session_idle_timeout` (5m by default) — working agents, ones
  waiting on you, ones whose backgrounded tool call is still running (Claude's `run_in_background`
  Bash or Monitor, a Codex shell command; the clock restarts when it ends), and terminals with a
  command running are all spared, and a reaped agent revives on the next attach with its
  conversation resumed. Until then its row's STATUS DOT is gray,
  whatever its last status was — a cold session shows what it last did, not what it is doing. Both halves of the PREWARM POOL are
  switchable — `prewarm_agents` and `prewarm_sessions`, `true` by default, on the SETTINGS OVERLAY's
  Sessions tab or by hand in CONFIG.JSON (see [Configuration](configuration.md)); switching the pool
  off drains its spares on the next sweep — and a warm spare nobody claims inside 15 min is reaped on
  its own, because it holds real memory and its context goes stale. A spare is a bare CLI at its
  prompt, so the CLI's own session list (Claude's `/list-agents`) shows it beside your sessions,
  named after the directory. The IDLE REAPER's check is a 15 s sweep (`ORION_IDLE_REAP_MS`), so the real
  latency is the timeout plus up to 15 s more; `session_idle_timeout` also takes `"off"`, which
  switches reaping off entirely.

## Pull requests

orion finds the pull request on each branch with `gh` and shows it on the band's rule and on its
cards' `#42 title` line. The row outlives
the pull request: once it is merged or closed the row stays, badged `merged` or `closed` (an open one is
badged `ready` — ready for review, the state and nothing more — and a draft is dimmed and badged `draft`;
one GitHub says cannot merge, its branch conflicting with the base or a check failing, is red end to end
and badged `conflicts` or `failing` instead, the PULL REQUEST PAGE saying the conflict beside the
state and marking a failed check on its Checks tab),
for as long as the checkout does — a worktree whose PR has shipped is the one
you are about to archive or delete, and the PR is what you check first. A merged one also takes over the
checkout's band: purple dot and purple branch name, so the checkout to
delete stands out from across the room (a session still running or asking there keeps its yellow or red —
that is not a checkout to pull out from under it). The name sweeps the way a running row's does for about
five seconds after orion sees the merge land, then holds still in solid purple — nothing about a landed
checkout is live, so it says so once; one found already merged (last run's cache, a first lookup) never
sweeps, and a merged checkout left lying around costs an idle orion no repaints. Jump to it with
`⌘K` and the pane reads the pull request as its [page](keys.md#the-pull-request-page) — description and
conversation, changes, commits, checks and reviews in tabs, one `gh pr view` for all of them — exactly
as the PULL REQUESTS MODAL does (whose list retires a pull request on merge, and which `hide_draft_prs` never thins — the
modal lists drafts too); `⌘E` shows its diff. Manual link
attachment is gone; links an earlier version saved stay in the database, so no data is discarded,
though the grid draws none of them.

This is the one part of orion the TUI asks for itself rather than the DAEMON: every `gh pr view`,
open-list query (`gh api graphql`) and `gh pr diff` — and the ISSUES MODAL's `gh issue list`, `gh issue view` and `gh issue comment` — is spawned by the client, which is why the lookups stop the moment you
quit, and why a machine with no `gh` — or one that is unauthenticated, or pointed at a checkout with no
remote — just shows no rows instead of an error. The selected project is asked about most: its
selected worktree's pull request and its open list on every tick, one process each, and its other
checkouts on a sweep that takes one of them per tick — so every band learns whether its branch
has merged without the cursor ever visiting it (the ROOT WORKTREE is left out; nobody deletes it over a
merge). Nothing is stacked while a call is in flight, and each is abandoned after 20 s. The other
projects' open pull requests and open issues are swept too, one project per tick on the five-minute
beat, so a project switched to shows lists minutes old at worst. The selected
worktree and the open list settle onto a steady 15 s beat; the swept checkouts onto 5 min, since a
merge reaches them sooner anyway — the moment a pull request drops out of the open list, the checkout on
its branch is asked again on the next tick, and turns purple seconds after the merge. An empty answer
backs off by doubling — out to 3 min for a branch that never grows a PR, 10 min for a project with none
open — so a machine with thirty repos does not cost thirty API calls a beat. Focusing the
terminal window pulls the next lookup forward, floored at a few seconds; `⌘R` is the one
gesture that asks straight away, every checkout of the project and its open issues included.

The open list is one GraphQL query per project rather than `gh pr list`, because a row's checks need
only GitHub's own verdict on them — the pass / fail / pending the pull request page shows — and `gh pr
list` asks for every check on every pull request instead, which on a busy repo times out every time. A
lookup that fails keeps the last list that worked on screen, and the PULL REQUESTS MODAL says
`couldn't refresh` under its filter until an answer lands.

Settings and hotkeys live in [Configuration](configuration.md). The process model, the IPC CODEC and
the crate layout are covered in more depth in [ARCHITECTURE.md](../ARCHITECTURE.md).
