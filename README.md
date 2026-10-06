<div align="center">

# orion

**Mission control for your coding agents.**

Run Claude Code, Codex, Cursor, Pi, Muse, Grok and OpenCode across every project and worktree, from one terminal grid. They keep working when you close it.

```sh
curl -fsSL https://raw.githubusercontent.com/oliverkidd/orion/main/install.sh | sh
```

macOS · Linux · one Rust binary

<img src="assets/screenshot.png" alt="orion home screen: the ORION wordmark under a starfield" width="100%">

</div>

> [!NOTE]
> Built for my own workflow and my colleagues'. Keys and features move. Fork it if you want something stable.

## Get going

```sh
cd ~/code/my-app
orion
```

First launch walks you through setup (agents, accounts, editor). `⌘,` changes it later. `orion doctor` checks what's missing.

## Keys

| Key | Does |
|---|---|
| `⌘N` | New agent: type the task, `Enter` |
| `Tab` / `⌘.` | In the prompt: pick harness / checkout or new worktree |
| `↑` `↓` | Walk the grid |
| `Enter` / `Esc` | Step into a session / back out |
| `Space` | Send the next turn without opening |
| `⌘E` · `⌘P` · `⌘⇧F` · `⌘B` | Diff · find file · grep · file tree |
| `⌘L` | Linear issues assigned to you |
| `⌘S` | Browse agent skills |
| `⌘,` | Settings |
| `^C` | Quit the TUI (sessions keep running) |

No ⌘ in your terminal? Use `Ctrl`. Full list: [docs/keys.md](docs/keys.md).

**Status dots:** red ● waiting on you · red ✕ crashed · accent ● finished, unread · gold ◐ working · gray ● idle.

## Agents

orion runs these CLIs; install at least one.

| Harness | Install |
|---|---|
| [Claude](https://code.claude.com/docs/en/setup) | `curl -fsSL https://claude.ai/install.sh \| bash` |
| [Codex](https://github.com/openai/codex) | `npm i -g @openai/codex` |
| [Cursor](https://cursor.com/install) | `curl -fsSL https://cursor.com/install \| bash` |
| [Pi](https://pi.dev) | `curl -fsSL https://pi.dev/install.sh \| sh` |
| [Muse](https://developer.meta.com/ai/lp/muse-code) | `curl -fsSL https://dev.meta.ai/install.sh \| bash` |
| [OpenCode](https://opencode.ai/docs/) | `curl -fsSL https://opencode.ai/install \| bash` |
| [Grok](https://github.com/xai-org/grok-build) | `curl -fsSL https://x.ai/cli/install.sh \| bash` |

## Updating

```sh
orion upgrade
```

Versions say what an upgrade costs: a patch release (1.2.**3**) changes only the TUI and CLI, so orion reopens and every session keeps running; a minor release (1.**3**.0) changes the daemon, which has to restart to run the new code: **Upgrade orion** in the app does that for you, while `orion upgrade` leaves it to you when sessions are live (agents resume; terminals start a new shell).

From source: `cargo install --path crates/orion --locked`, then `orion kill && orion` to restart the daemon.

## Docs

[Keys](docs/keys.md) · [Commands](docs/commands.md) · [Sessions](docs/sessions.md) · [Configuration](docs/configuration.md) · [How it works](docs/how-it-works.md) · [Architecture](ARCHITECTURE.md)

## License

MIT. A customized fork of [nebula](https://github.com/AgentSystemLabs/nebula) by [Cody Seibert](https://github.com/codyseibert).
