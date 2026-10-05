> [!NOTE]
> A tool built for my own workflow and my colleagues'. It changes as we use it: keys, screens and features move. Fork it if you want something stable. There are no GitHub bots or PR checks here; the only workflow builds release binaries when a `v*` tag is pushed.

<div align="center">

# orion

**Mission control for your coding agents.**

Run **Claude Code**, **Codex**, **Cursor**, **Pi**, **Muse**, **Grok Build** and **OpenCode** across every project and git worktree you own — from one terminal, one keyboard, one grid. They keep working when you close it.

[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey?style=flat-square)](#install)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584?style=flat-square)](https://www.rust-lang.org)

[**Keys**](docs/keys.md) · [**Commands**](docs/commands.md) · [**Sessions**](docs/sessions.md) · [**Configuration**](docs/configuration.md) · [**How it works**](docs/how-it-works.md)

```sh
curl -fsSL https://raw.githubusercontent.com/oliverkidd/orion/main/install.sh | sh
```

<img src="assets/screenshot.png" alt="orion: project tabs with status dots across the top, a band of session cards per checkout, and the running session's terminal under them" width="100%">

</div>

---

## What it is

You start three agents in three terminal tabs. Five minutes later you don't know which one is waiting on a permission prompt, which one finished, and which one is still thinking.

orion replaces the reading with a grid and a color. Every session is a card — its name, what it runs on, the last thing you asked it — with a status dot. Cards sit in a band per checkout under a row of project tabs. A red `●1` on a tab tells you exactly where to look.

**No Electron, no browser, no server, no MCP.** One Rust binary and a unix socket.

## What you get

| | |
|---|---|
| **One grid, every session** | Project tabs across the top, a band per worktree, a card per session, and the live terminal of the card under the cursor. `j`/`k` walk the bands, `Enter` steps into the pane, `Esc` hands the keys back (`⇧Esc` is the agent's Esc). |
| **A daemon that owns the PTYs** | Quit the TUI, shut the laptop, come back tomorrow. The agents never stopped. |
| **Status marks you read instead of screens** | ● red waiting on you, ✕ red crashed, ● your theme's accent finished and unread (it shimmers until you look), ◐ a gold spinner mid-turn, ● gray at rest. Filled means look at me. |
| **Claude accounts** | Add a second Claude login in Settings → Agents (or at first run): its own config dir, your `CLAUDE.md`, settings and skills shared, signed in from inside orion. Each goes by the name you give it and its email, `Work (you@example.com)` — `r` renames it — and two signed in as one are flagged; a removed one's dir is listed, to add back or trash. A session that hits its usage limit goes red with Claude's reset time; `⇧C` carries it, conversation and all, onto the other account. |
| **A task box, not a picker** | `⌘N` opens the quick prompt: type the task, `Enter`, and an agent is working on it. |
| **Real git worktrees** | `⌘N` in the box flips the launch onto a fresh `git worktree`. Ignored `.env*` files from the main checkout are symlinked in by default. Delete asks before losing uncommitted work, `p` fetches and fast-forwards a band's checkout onto its remote, and `⇧P` pushes it (publishing a new branch). |
| **Pull requests, in tabs** | The pull request beside the cards — or under the cursor in `v` — reads like Cursor's page: `Description · Changes 11 · Commits 4 · ✗ Checks 44/45 · ✓ Reviews`. `Enter` on a file or a commit opens its diff, on a check its log. |
| **Linear, assigned to you** | `⌘L` lists issues assigned to you. Space marks, Enter starts one agent on the set and asks for a single PR, attached to the issues once it opens; `⌘U` attaches the set to an open PR you pick instead. Settings → Linear has every option — the link (on), the account, the task template — and tests the project's `LINEAR_API_KEY`, read from its `.env.local` or `.env`. |
| **Ghostty by default** | Settings picks Ghostty or Terminal.app. Stolen Command chords are unbound in Ghostty's config so they reach orion, and Ghostty is reloaded to pick them up. |
| **Diff, find, grep, browse** | `⌘E` the diff, wrapped and numbered in the file's colours — the uncommitted changes, everything the branch added (merges of `main` left out), or the commits you tick, together or one at a time — `⌘P` the file finder, `⌘⇧F` grep, `⌘B` the tree. Markdown opens as its rendered page; `Enter` edits any file in micro, Microsoft Edit or fresh — VS Code's keys, wrapped lines, the mouse (Settings → File editor) — and `⌘O` opens it in Cursor, VS Code, Sublime Text or Zed (Settings → Open in app). |
| **Your skills, browsable** | `⌘S` lists every agent skill on the machine — yours, the project's, Cursor's and Codex's, installed plugins' — searchable by name and description, each read in place. `Enter` edits one, `⌘N` starts a new one, `⌘W` moves one to the Trash. |
| **It follows you** | `orion ssh <host>` opens orion there. `orion tunnel <host>` puts that machine's TUI in a browser tab. |

## Supported harnesses

orion spawns these CLIs; it does not ship them. Install at least one, then pick it with `Tab` in the quick prompt.

| | Harness | CLI | Install |
|---|---|---|---|
| <img src="https://www.google.com/s2/favicons?domain=claude.com&sz=128" width="24" height="24" alt="Claude"> | [Claude](https://code.claude.com/docs/en/setup) | `claude` | `curl -fsSL https://claude.ai/install.sh \| bash` |
| <img src="https://www.google.com/s2/favicons?domain=openai.com&sz=128" width="24" height="24" alt="Codex"> | [Codex](https://github.com/openai/codex) | `codex` | `npm i -g @openai/codex` |
| <img src="https://www.google.com/s2/favicons?domain=cursor.com&sz=128" width="24" height="24" alt="Cursor"> | [Cursor](https://cursor.com/install) | `cursor-agent` | `curl -fsSL https://cursor.com/install \| bash` |
| <img src="https://www.google.com/s2/favicons?domain=pi.dev&sz=128" width="24" height="24" alt="Pi"> | [Pi](https://pi.dev) | `pi` | `curl -fsSL https://pi.dev/install.sh \| sh` |
| <img src="https://www.google.com/s2/favicons?domain=meta.com&sz=128" width="24" height="24" alt="Muse"> | [Muse](https://developer.meta.com/ai/lp/muse-code) | `muse` | `curl -fsSL https://dev.meta.ai/install.sh \| bash` |
| <img src="https://www.google.com/s2/favicons?domain=opencode.ai&sz=128" width="24" height="24" alt="OpenCode"> | [OpenCode](https://opencode.ai/docs/) | `opencode` | `curl -fsSL https://opencode.ai/install \| bash` |
| <img src="https://www.google.com/s2/favicons?domain=x.ai&sz=128" width="24" height="24" alt="Grok"> | [Grok](https://github.com/xai-org/grok-build) | `grok` | `curl -fsSL https://x.ai/cli/install.sh \| bash` |

## Install

macOS or Linux. The installer downloads a prebuilt binary from the latest GitHub release into `~/.local/bin`, and falls back to building from this repo when no release matches.

```sh
curl -fsSL https://raw.githubusercontent.com/oliverkidd/orion/main/install.sh | sh
```

Then it makes sure of what orion leans on: **git** (it stops without it), an **editor** files open in — fresh, unless fresh, micro or Microsoft Edit is already there, from Homebrew or else fresh's own quick-install script on Linux and micro's on a Mac without Homebrew — and **gh**, for pull requests and issues (Homebrew, or a pointer to its install page). Anything already there it leaves alone, without a word. `sh -s -- --no-deps` (or `ORION_NO_DEPS=1`) installs orion alone.

`orion doctor` checks all of it at any time — git, gh and its sign-in, the editor and what really opens, the Open in app editor, Ghostty's keybinds, the CLI of every agent you turned on, the project's `LINEAR_API_KEY`, docker compose projects a deleted checkout left behind — and prints the command that fixes whatever is missing. It never installs anything itself; first-run setup and Settings do, on `i` ([Configuration](docs/configuration.md#installing-editors-and-agent-clis)).

Every push to `main` publishes a release (`v1.0.N`, the patch bumped automatically; edit `[workspace.package] version` in `Cargo.toml` for a minor or major bump, or put `[skip release]` in the commit message to skip one). A running orion checks for a newer release hourly and shows it in the footer; `orion upgrade` installs it.

To build from source instead:

```sh
git clone https://github.com/oliverkidd/orion.git
cd orion
cargo install --path crates/orion --locked
```

That needs [Rust](https://rustup.rs) (`rustup`). Put `~/.cargo/bin` on your `PATH`.

`orion ssh` and `orion tunnel` need an OpenSSH client. `orion browser` / `orion tunnel` need `ttyd` on the machine that serves the TUI.

## Updating

You installed from a clone. To pick up later commits from GitHub (no local edits):

```sh
cd /path/to/orion          # the clone, not your app repo
git pull
cargo install --path crates/orion --locked
```

That rebuilds `~/.cargo/bin/orion`. An already-running daemon stays on the old binary until you restart it:

```sh
orion kill                 # stops the daemon and every session
orion                      # from the app repo you work in
```

Skip `orion kill` if nothing is running.

If you *did* change this clone, commit and push first, then on the other machine `git pull` and `cargo install` as above.

Once a `v*` release exists, `orion upgrade` downloads that binary instead. It refuses to overwrite a `cargo install` / `cargo build` binary unless you pass `--force`.

## Quickstart

**1. Open it in a git repo.** From Ghostty (or Terminal.app):

```sh
cd ~/code/my-app
orion
```

The first launch opens a short setup, a step at a time: which agents to turn on (they all start off) — a missing CLI installs from there with `i` — your Claude accounts, the editor files open in and the app `⌘O` hands them to, worktree defaults, Linear and the outside terminal, then a summary of what you chose and the keys to press next. `Esc` skips it, and Settings (`⌘,`) changes any of it later. Then the splash: `Enter` adds the repo as your first project and opens the grid. Or register one first: `orion add ~/code/my-app`.

**2. Start an agent.** `⌘N` (`Ctrl+N` if the terminal never sends ⌘) opens the quick prompt. Type the task, `Enter`. `Tab` picks the harness, `⌘/` the model, `⌘Y` steps the effort, `⌘.` the checkout (or a fresh worktree), `⌘P` the project — the box's header shows each with its key.

**3. Read the grid.** `↑` / `↓` walk the bands. `Enter` steps into the pane. `Esc` leaves it. `Space` on a card sends the next turn without opening the session. `^C` quits the TUI; sessions keep running in the daemon.

**4. Linear (optional).** Put `LINEAR_API_KEY=lin_api_…` in the project's `.env` or `.env.local`. `⌘L` lists issues assigned to the key's owner. Settings → Linear → **Test connection** says whose it is, and **Linear account** takes your email if the key is shared. Space marks, Enter launches one agent for the set.

orion reloads Ghostty's config itself after writing its keybinds (Ghostty 1.2+); on an older Ghostty, reload it (`⌘⇧,`) after the first launch so Command chords reach orion.

## Documentation

| | |
|---|---|
| [**Keys**](docs/keys.md) | Default bindings, the grid, worktree views, the mouse. All of it rebindable in Settings. |
| [**Commands**](docs/commands.md) | The `orion` CLI: `add`, `worktree`, `spawn`, `open`, `config`, `ssh`, `tunnel`, `kill`, `doctor`, `upgrade`. |
| [**Sessions**](docs/sessions.md) | The quick prompt, the grid, presets, issues, pull requests. |
| [**Configuration**](docs/configuration.md) | `config.json` and `config.local.json` (`orion config path` prints where; on macOS `~/Library/Application Support/dev.orion.orion/`), the settings overlay, project files, env overrides. |
| [**How it works**](docs/how-it-works.md) | The daemon, hook dialects, auto-title, worktree relocation. |
| [**Architecture**](ARCHITECTURE.md) | Process model, the IPC codec, crate layout. |

## Building

```sh
cargo build --release     # → target/release/orion
cargo test                # unit + end-to-end suite
```

`orion-core`, `orion-daemon`, `orion-tui`, `orion`. `vendor/vt100` is a patched terminal parser: rows scrolled out of a top-anchored region go to the scrollback ring.

Releases: push a `v*` tag and CI builds mac (arm/intel) and linux (x64/arm64) binaries. That is what `install.sh` downloads.

## License

MIT — see [LICENSE](LICENSE).

## Attribution

orion is a **customized fork** of [nebula](https://github.com/AgentSystemLabs/nebula) by [Cody Seibert](https://github.com/codyseibert) / [Agent System Labs](https://github.com/AgentSystemLabs), MIT licensed.

The grid, daemon, harnesses, worktrees and most of the TUI come from that project. This fork renames the binary and crates, and adds Mac Command chords, Ghostty as the default outside terminal, Linear browse/attach (`⌘L`), worktree `.env` symlinks, and a dirty-worktree delete prompt.

Upstream walkthrough: [A Complete Walkthrough of Nebula](https://youtu.be/ZBPqH36BPgI).
