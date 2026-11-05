//! The command-line surface: every `orion` subcommand, its arguments, and the
//! help each one prints. `main.rs` owns only the dispatch and the logging.
//!
//! Two rules keep `--help` readable, and both are load-bearing:
//!
//! * **Every doc comment is two paragraphs.** `clap_derive` takes the first
//!   paragraph as `about` and the whole comment as `long_about`, and the root's
//!   command list only ever renders `about` — so paragraph one is a single
//!   sentence under ~60 characters that has to stand alone, and the prose after
//!   the blank line is what `orion <command> --help` shows.
//! * **Examples are `after_help`**, which `after_long_help` falls back to, so
//!   one string serves both `-h` and `--help`. Clap runs it through the same
//!   wrapper as everything else: keep every line under ~78 characters or the
//!   hand-aligned columns reflow into a mess on a narrow terminal.
//!
//! Wrapping itself comes from clap's `wrap_help` feature (see Cargo.toml).
//! Without it `StyledStr::wrap` compiles to a no-op and no width setting here
//! does anything at all.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "orion",
    version,
    max_term_width = 100,
    about = "Terminal multiplexer for Claude Code agents",
    long_about = "Terminal multiplexer for Claude Code agents.\n\n\
        Orion keeps a tree — projects hold worktrees, worktrees hold sessions — \
        and a background daemon owns every PTY in it. \
        Agents keep running after the TUI quits, and their scrollback is replayed \
        when you come back.\n\n\
        A bare `orion` opens the TUI. The commands below drive the same tree from \
        a shell; `rename`, `worktree`, `spawn` and `open` are the ones an agent \
        runs on your behalf from inside a session.",
    after_help = ROOT_EXAMPLES
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
    /// Directory to add as a project — shorthand for `orion add <dir>`.
    ///
    /// A directory whose name collides with a subcommand needs the long form
    /// (`orion add setup`) or a `./` prefix.
    pub(crate) dir: Option<String>,
}

const ROOT_EXAMPLES: &str = "\
Examples:
  orion                            open the TUI (auto-starts the daemon)
  orion add ~/code/my-app          register a project
  orion ssh user@server            open orion on a remote machine

Run `orion <command> --help` for a command's flags and examples.";

/// `--kind` for `orion spawn`: one of the agent CLIs orion runs. A bare
/// `custom` is never accepted: custom harnesses carry a registry id the
/// flag cannot name, so they launch from the TUI picker and presets.
fn parse_agent_kind(s: &str) -> Result<orion_core::AgentKind, String> {
    orion_core::AgentKind::parse(s).ok_or_else(|| {
        format!(
            "unknown harness `{s}` — expected one of {} (custom harnesses launch from the TUI)",
            orion_core::AgentKind::ALL
                .iter()
                .filter(|k| **k != orion_core::AgentKind::Custom)
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Register a git checkout as a project.
    ///
    /// Adds a directory to the project list, named after the repository's
    /// root directory. Bare `orion <dir>` is the same
    /// command, so `orion .` and `orion add .` do the same thing.
    #[command(after_help = ADD_EXAMPLES)]
    Add {
        /// Path to a git repository (default: the current directory).
        #[arg(default_value = ".")]
        path: String,
    },
    /// Run the daemon that owns every session.
    ///
    /// The daemon holds every PTY, the store, git and agent status. The TUI
    /// auto-spawns it detached, so you rarely run this by hand — reach for it
    /// when you want to watch what the daemon is doing. Set ORION_LOG to
    /// change the log level.
    #[command(after_help = DAEMON_EXAMPLES)]
    Daemon {
        /// Stay attached to the terminal instead of logging to file.
        #[arg(long)]
        foreground: bool,
    },
    /// Shut the running daemon down (stops all sessions).
    ///
    /// Asks the daemon to exit cleanly; every session it owns stops with it.
    /// A daemon from a build on another protocol can't take that request, so
    /// it gets SIGTERM instead, which it handles the same clean way.
    /// Quitting the TUI does not do this — the daemon outlives its clients on
    /// purpose — so this is how you stop everything, and how you move onto a
    /// newly installed binary.
    #[command(after_help = KILL_EXAMPLES)]
    Kill,
    /// Title the session this command runs inside.
    ///
    /// Run from inside a orion agent session: it titles that session's row.
    /// Agents run it themselves to auto-title on your first prompt. Without
    /// --force it only fills in a title that is still missing, so an agent
    /// can never overwrite a name you chose.
    #[command(after_help = RENAME_EXAMPLES)]
    Rename {
        /// The new title; multiple words need no quotes.
        #[arg(required = true, num_args = 1..)]
        title: Vec<String>,
        /// Replace an existing title instead of only filling in a missing one.
        #[arg(long)]
        force: bool,
    },
    /// Move this session into a worktree of its project.
    ///
    /// Run from inside a orion agent session; agents run it when you ask them
    /// to work in a worktree. Creates the git worktree when the branch has
    /// none, re-homes the session onto it at once, and restarts the session
    /// resumed inside the new checkout as soon as the current turn ends.
    #[command(after_help = WORKTREE_EXAMPLES)]
    Worktree {
        /// Branch name; several words are joined with hyphens, none at all
        /// gets a random `<adj>-<noun>-<verb>` one.
        name: Vec<String>,
        /// Start point for a new branch (default: the project's Base branch,
        /// else origin's default branch, fetched).
        ///
        /// A branch name origin has means origin's copy of it, fetched first:
        /// `main` is `origin/main`, never this checkout's local branch. A tag,
        /// a SHA or a branch origin lacks is used as named.
        #[arg(long, value_name = "REF")]
        base: Option<String>,
    },
    /// Start another agent session beside this one.
    ///
    /// Run from inside a orion agent session; agents run it when you ask for
    /// a new orion session. The new session starts in the same worktree (or
    /// the one `--worktree` names), on the task you name as its first
    /// prompt, and shows up on the grid on its own — this session carries
    /// on untouched.
    #[command(after_help = SPAWN_EXAMPLES)]
    Spawn {
        /// The task the new session starts on; multiple words need no quotes.
        #[arg(required = true, num_args = 1..)]
        task: Vec<String>,
        /// Harness for the new session: claude, codex, cursor, grok or
        /// opencode.
        ///
        /// Defaults to the harness this session is running.
        #[arg(long, value_name = "KIND", value_parser = parse_agent_kind)]
        kind: Option<orion_core::AgentKind>,
        /// Start the session in this worktree of the project instead: the
        /// branch's worktree, created when it has none. Spaces become
        /// hyphens, as with `orion worktree`.
        #[arg(long, value_name = "NAME")]
        worktree: Option<String>,
        /// Start point for a new --worktree branch (default: the
        /// project's Base branch, else origin's default branch).
        #[arg(long, value_name = "REF", requires = "worktree")]
        base: Option<String>,
    },
    /// Show files to the user inside this orion.
    ///
    /// Run from inside a orion agent session; agents run it only when you
    /// ask to see a file, never unprompted. Text files only: an image or any
    /// other binary is refused, since a terminal has nothing to show for it.
    /// The files open in orion's file tabs — a modal with one tab per file,
    /// the focused one previewed, Enter editing it — in every orion
    /// attached to this daemon, and this session carries on untouched. A
    /// `:42` after a file opens its tab at line 42.
    #[command(after_help = OPEN_EXAMPLES)]
    Open {
        /// The files to show, relative to the current directory or
        /// absolute, each with an optional `:line`.
        #[arg(required = true, num_args = 1.., value_name = "FILE[:LINE]")]
        files: Vec<String>,
    },
    /// What an autofix session runs on itself.
    ///
    /// Run from inside an `autofix-<n>` session that orion started on a
    /// broken pull request, when another session on the branch is already
    /// fixing it. You never need to run it yourself, so `orion --help`
    /// leaves it out.
    #[command(subcommand, hide = true)]
    Autofix(AutofixCommand),
    /// Locate this machine's settings.
    ///
    /// Settings live in `config.json`, with `config.local.json` over it for
    /// what only makes sense on this machine, beside `agent_presets.json` and
    /// `ssh_hosts.json`. Changes apply without a restart.
    #[command(after_help = CONFIG_EXAMPLES)]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Open orion on a remote host over ssh.
    ///
    /// Connects with ssh and runs orion there, installing it on the remote
    /// first when it is missing, so what you drive is the remote's own daemon,
    /// sessions and settings. Destinations are remembered for the TUI's host
    /// picker (**SSH hosts** in its command palette, `⌘⇧P`).
    #[command(after_help = SSH_EXAMPLES)]
    Ssh {
        /// ssh destination, passed verbatim (e.g. user@server).
        host: String,
        /// Remote directory to start in (default: remote $HOME).
        path: Option<String>,
    },
    /// Open orion on its setup.
    ///
    /// Four pages: Welcome; Claude — its CLI (`i` installs it) and the
    /// accounts to sign in; your first project, its base branch and run
    /// command; and the keys that matter. The palette's Run setup opens
    /// the same thing.
    Setup,
    /// Open orion as a Mac app of its own.
    ///
    /// Makes Orion.app in ~/Applications on first use — a copy of the
    /// Ghostty on this Mac under orion's name and icon — and opens orion
    /// in it: in the Dock, Launchpad and Spotlight from then on. The app
    /// is a window onto this same orion, so sessions keep running and
    /// projects, settings and history stay where they are. A bare `orion`
    /// does the same on a Mac with Ghostty; this is the command that says
    /// why when the app can't be made. macOS only, and needs Ghostty.
    App,
    /// Check what orion needs on this machine.
    ///
    /// One line each for git, gh and its sign-in, fresh (or the editor
    /// `ORION_EDITOR` names), the editor app files open in, Orion.app and the
    /// Ghostty it is made from, the CLI of every agent turned on, and this
    /// project's LINEAR_API_KEY (where it was found, never the key) — with
    /// the command that fixes whatever is missing. It never installs
    /// anything. Exits non-zero only when something orion can't do without
    /// is missing: git, or the editor files open in.
    #[command(after_help = DOCTOR_EXAMPLES)]
    Doctor {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install the latest published orion over this one.
    ///
    /// Runs the install script for the newest release. Upgrading with a daemon
    /// running is safe: sessions keep running on the old binary until you
    /// restart it with `orion kill` (which stops all sessions). When the new
    /// build can't attach to that daemon, it says so and offers the restart.
    #[command(after_help = UPGRADE_EXAMPLES)]
    Upgrade {
        /// Upgrade even when running from a local cargo build.
        #[arg(long)]
        force: bool,
    },
    /// Installer hook: print the cutover note only when a live daemon is on
    /// a different build than this binary (see `make install` / install.sh).
    #[command(hide = true, name = "_stale-daemon-note")]
    StaleDaemonNote,
    /// Upgrade hook: print the protocol version this binary speaks, so the
    /// `orion upgrade` that installed it can tell whether it will still
    /// attach to the daemon left running.
    #[command(hide = true, name = "_protocol-version")]
    ProtocolVersion,
    /// Upgrade hook: print the stamp of the daemon code this binary carries,
    /// so the build it replaces can tell whether the running daemon is
    /// already that code and can keep its sessions.
    #[command(hide = true, name = "_daemon-fingerprint")]
    DaemonFingerprint,
    /// Installer hook: what `orion app` does, under the name every
    /// install.sh has called (see install.sh).
    #[command(hide = true, name = "_open-in-ghostty")]
    OpenApp,
}

const ADD_EXAMPLES: &str = "\
Examples:
  orion add .                     add the repo you are standing in
  orion add ~/code/my-app         add one by path
  orion ~/code/my-app             the same, without the subcommand";

const DAEMON_EXAMPLES: &str = "\
Examples:
  orion daemon --foreground       run it attached, logs on stdout
  ORION_LOG=debug orion daemon --foreground
                                   the same, at debug level";

const KILL_EXAMPLES: &str = "\
Examples:
  orion kill                      stop the daemon and every session";

const RENAME_EXAMPLES: &str = "\
Examples:
  orion rename Fix Login Redirect   title this session
  orion rename --force Auth Rework  replace a title already set";

const WORKTREE_EXAMPLES: &str = "\
Examples:
  orion worktree fix-login-redirect  branch off the configured base and move there
  orion worktree fix login redirect  the same; the words are slugified
  orion worktree                     invent a random branch name
  orion worktree hotfix --base v0.21.0
                                      branch from a named start point";

const SPAWN_EXAMPLES: &str = "\
Examples:
  orion spawn \"port the tests to the new fixture\"
  orion spawn --kind codex \"review the diff on this branch\"
  orion spawn --worktree fix-font-names \"stop matching font names by substring\"";

const OPEN_EXAMPLES: &str = "\
Examples:
  orion open README.md                one tab
  orion open src/main.rs docs/keys.md a tab each, in this order
  orion open src/app.rs:42            at line 42";

const SSH_EXAMPLES: &str = "\
Examples:
  orion ssh user@server           open the remote's orion
  orion ssh user@server /srv/app  start in a directory there";

const CONFIG_EXAMPLES: &str = "\
Examples:
  orion config path               where each settings file lives
  orion config harnesses          the harness registry launches read";

const DOCTOR_EXAMPLES: &str = "\
Examples:
  orion doctor                    check this machine, from the repo you work in
  orion doctor --json             the same, for a script";

const UPGRADE_EXAMPLES: &str = "\
Examples:
  orion upgrade                   install the latest release
  orion upgrade --force           do it over a local cargo build";

#[derive(Subcommand)]
pub(crate) enum AutofixCommand {
    /// Leave the fix to the session already on it: name the reason on
    /// this session's row and archive it. orion shows the reason.
    #[command(
        name = "stand-down",
        after_help = "Example:\n  orion autofix stand-down \"billing: already fixing the unit test\""
    )]
    StandDown {
        /// Why — which session is on it, and what it is doing; multiple
        /// words need no quotes.
        #[arg(required = true, num_args = 1.., value_name = "REASON")]
        reason: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print where each settings file lives.
    ///
    /// `ORION_CONFIG_FILE` moves `config.json` alone — into a dotfiles
    /// checkout, say; `ORION_DATA_DIR` moves them all.
    #[command(after_help = "Example:\n  orion config path")]
    Path,
    /// Print the effective harness registry: every harness orion knows —
    /// the built-ins, Claude accounts and `harnesses` map ids —
    /// with the program, flags, resume shape, hook dialect and defaults a
    /// launch actually uses. Copy a row into config.json `harnesses` to
    /// override it.
    #[command(after_help = "Example:\n  orion config harnesses")]
    Harnesses,
}
