<div align="center">

# orion

**Mission control for your coding agents.**

Run Claude Code, Codex, Cursor, Grok and OpenCode across every project and worktree, from one grid. They keep working when you close it.

macOS

<img src="assets/screenshot.png" alt="orion home screen: the ORION wordmark under a starfield" width="100%">

</div>

> [!NOTE]
> Built for my own workflow and my colleagues'. Keys and features move. Fork it if you want something stable.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/oliverkidd/orion/main/install.sh | sh
```

That is the whole install. It needs git. It also gets fresh (the editor files open in), gh and Ghostty if you don't have them, then opens orion as its own app, **Orion.app**.

## Run

- Open **Orion** from the Dock or Spotlight. Typing `orion` in a terminal opens the same app.
- **Over ssh or in tmux:** `orion` runs in the terminal it was typed in.

The first launch walks you through setup: Claude Code (one key installs it if it is missing) and its accounts, then your first project. `orion upgrade` gets the latest release.

## Core features

- **One grid for every agent.** Each git worktree of a project gets a band; each agent is a row in it showing what it is doing and whether it needs you.
- **Agents keep running.** A background daemon owns every session, so you can close orion and come back to the same agents and their scrollback.
- **A worktree per task.** Start an agent on a fresh worktree in one step, so parallel tasks never share a checkout.
- **Review without leaving.** Read the diff, find a file, search the code and open an editor on any checkout.
- **Pull requests and Linear.** See each checkout's pull request and checks, and start an agent straight from a Linear issue.
- **It tells you when to look.** A sound and a notification when an agent finishes, crashes or needs an answer.
- **Needs you.** One board for everything waiting on you: agents with a question or a plan, Linear issues, code reviews, Slack messages and meeting takeaways.
- **Your phone while you're away.** Lock the screen and an agent that starts waiting messages you on Slack, with its full last reply; your answer there goes straight back to it.
- **Automations.** A PR fixer, a bug fixer and a feature builder that pick up labelled Linear tickets and wait for your plan approval, a Friday week in review, Teams meeting takeaways placed in your todos, and a Blue sky builder that prototypes an idea.

| Key | Does |
|---|---|
| `⌘N` | New agent: type the task, `Enter` |
| `⌘⇧N` | New agent in a new worktree, the same way |
| `↑` `↓` · `Enter` · `Esc` | Walk the grid · step into a session · back out |
| `⌘K` | Needs you; type to jump to any project, worktree or session |
| `⌘J` | Automations |
| `⌘E` · `⌘P` · `⌘⇧F` | Diff · find file · search |
| `⌘L` | Linear issues |
| `⌘,` | Settings |

The `⌘` keys reach orion in Orion.app. Every key: [docs/keys.md](docs/keys.md).

## Docs

[Keys](docs/keys.md) · [Commands](docs/commands.md) · [Sessions](docs/sessions.md) · [Configuration](docs/configuration.md) · [How it works](docs/how-it-works.md) · [Architecture](ARCHITECTURE.md)

## License

MIT. A customized fork of [nebula](https://github.com/AgentSystemLabs/nebula) by [Cody Seibert](https://github.com/codyseibert).
