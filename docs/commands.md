# Commands

<sub>[← README](../README.md) · [Keys](keys.md) · [Commands](commands.md) · [Sessions](sessions.md) · [Configuration](configuration.md) · [How it works](how-it-works.md)</sub>

The `orion` CLI. Every command carries its own help — `orion <command> --help` is the full page,
flags and examples included, and `-h` is the one-screen reminder. `orion --version` (short `-V`)
prints the version of the binary you're running (`orion 1.0.0`) — the same version the TUI's
FOOTER carries at its left edge (with `⇡ vX.Y.Z` beside it once a newer release is published), and
what to check after `orion upgrade`. This page is the same surface in one place. Commands marked *(agents run this)* are the ones a coding agent invokes on
your behalf — see [How it works](how-it-works.md).

```
orion                      open the TUI (auto-starts the daemon)
orion add <dir>            register a git checkout as a project
orion daemon               run the daemon that owns every session
orion kill                 shut the running daemon down (stops all sessions)
orion rename <title>       title the session this runs inside          (agents run this)
orion worktree [name]      move this session into a worktree           (agents run this)
orion spawn <task>         start another agent session beside it       (agents run this)
orion open <file>…         show files in this orion's file tabs       (agents run this)
orion config <cmd>         locate this machine's settings
orion ssh <host>           open orion on a remote host over ssh
orion setup                open orion on its setup, every step
orion app                  open orion as a Mac app of its own (Orion.app)
orion doctor               check what orion needs on this machine
orion upgrade              install the latest published orion
```

## The TUI

```sh
orion                    # launch the TUI (auto-starts the daemon). With no project yet, a
                          # launch from inside a git repo offers it on the splash — Enter
                          # opens it; anywhere else, Enter browses for one. On a Mac with
                          # Ghostty this opens Orion.app and leaves the terminal
```

## Projects and the daemon

```sh
orion add <dir>          # add a repo as a project, named after its root directory
orion add .              # same, for the repo you're in (bare `orion <dir>` / `orion .` also work)
                          # — but a directory whose name collides with a subcommand needs the long
                          # form (`orion add setup`) or a `./` prefix, or bare `orion setup`
                          # opens setup instead of adding the directory
orion daemon             # run the daemon (normally auto-spawned)
orion daemon --foreground  # daemon with logs to stdout, for debugging
orion kill               # stop the daemon and all sessions cleanly
```

## What agents run for you

```sh
orion rename <title>     # title the current session (agents run this; --force to retitle)
orion worktree [name] [--base <ref>]  # move the current session into a worktree of its project,
                          # creating the branch if it's new (agents run this when you ask for a
                          # worktree; no name invents one; --base picks a new branch's start point,
                          # a branch name meaning origin's fetched copy — main is origin/main;
                          # without it the worktree_base_branch setting, else origin's default)
orion spawn <task> [--kind <claude|codex|cursor|grok|opencode>]  # start a new agent session beside the current
                          # one, in the same worktree, opening on <task> (agents run this when you
                          # ask for a new orion session; --kind defaults to this session's harness;
                          # custom harnesses launch from the TUI picker and presets, not --kind)
orion open <file>…       # show the files in this orion's FILE TABS — a modal with one tab per
                          # file, the focused one previewed, Enter editing it (agents run this only
                          # when you ask to see a file; text files only — an image or any other
                          # binary is refused, and the agent names the path instead)
```

## Settings

```sh
orion config path              # where config.json, config.local.json, agent_presets.json and
                                # ssh_hosts.json live
orion config harnesses         # print the effective harness registry: every harness with
                                # the program, flags, resume shape, hook dialect and defaults a
                                # launch uses. Copy a row into config.json `harnesses` to override it
```

See [Configuration](configuration.md#backup-restore-and-other-machines).

## Checking the machine

```sh
orion doctor             # one line for each thing orion leans on, with the fix for a missing one:
                          # git, gh (installed and signed in: `gh auth status`), fresh (or the
                          # editor ORION_EDITOR names), the Open in app editor,
                          # Orion.app and the Ghostty it is made from, the CLI of every
                          # agent turned on, and the LINEAR_API_KEY of the project you are
                          # standing in — where it was found, never the key — and the docker
                          # compose projects whose checkout is gone, each with the
                          # `docker compose -p <project> down --volumes` that removes it. It never installs
                          # anything; the fix is the command to run (the same ones `i` runs from
                          # Settings and first-run setup). Exits non-zero only when something
                          # orion can't do without is missing: git, or the editor files open in
orion doctor --json      # the same report as {"ok": …, "checks": [{name, status, detail, fix,
                          # required}, …]}; status is ok, missing or skipped
```

```
  ✓ git               git version 2.54.0
  ✓ gh                signed in to GitHub
  ✗ File editor       fresh isn't installed — no file can open
                      fix: brew install fresh-editor
  ✓ Open in app       auto → Cursor
  ✗ Orion.app         not made yet — orion's ⌘ shortcuts work in it
                      fix: orion app
  ✓ Agent claude      ~/.local/bin/claude
  ✗ Agent codex       `codex` isn't on PATH
                      fix: brew install --cask codex
  – Linear            no LINEAR_API_KEY in my-app's .env.local or .env — only the Linear view needs one

Everything orion needs is here.
```

See [Installing editors and agent CLIs](configuration.md#installing-editors-and-agent-clis).

## Other machines, other screens

```sh
orion ssh <host> [dir]   # open orion on a remote machine over ssh (installs it there if
                          # missing); destinations are remembered for the TUI's HOSTS PICKER
                          # (SSH hosts, `⌘⇧P`). Needs the OpenSSH client (`ssh`) on PATH here. The
                          # remote runs on its own settings
orion upgrade            # install the latest release (--force on a dev build). Swapping the
                          # binary doesn't touch a running daemon, so afterwards it shuts an idle
                          # one (no live sessions) down for you and the next launch starts on the
                          # new binary. With sessions live it leaves the daemon up — they'd die
                          # with it — and says to run `orion kill` when you're ready to restart,
                          # unless the release left the daemon's code alone (every patch release
                          # does), when the daemon you have is already the new one and stays.
                          # When the new build speaks another protocol, and so can't attach to
                          # that daemon, it says that too and offers the restart then and there
```

From source: `cargo install --path crates/orion --locked`, then `orion kill && orion` to restart the daemon.

The agent CLIs orion runs, one of which it needs: [Claude](https://code.claude.com/docs/en/setup), [Codex](https://github.com/openai/codex), [Cursor](https://cursor.com/install), [OpenCode](https://opencode.ai/docs/) and [Grok](https://github.com/xai-org/grok-build). Setup's Agents step installs the ones you turn on.
