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
    /// What a factory agent runs to hand orion its pull request's facts.
    ///
    /// Run from inside a session the bug fixer or the feature builder
    /// started, once its draft pull request is open: orion writes the top
    /// of the PR's body from these facts, with its verdict. You never
    /// need to run it yourself, so `orion --help` leaves it out.
    #[command(subcommand, hide = true)]
    Pr(PrCommand),
    /// Read or build the decisions orion records at merge.
    ///
    /// After a pull request merges, orion reads the sessions that built it
    /// and records the decisions you made — rules, constraints, how you want
    /// agents to work — in `.orion/decisions/harvested/` in the repo,
    /// proposed in one pull request of yours (`orion/knowledge-<login>`).
    /// Once it merges, agents read the pages before they plan, beside the
    /// records written by hand in `.orion/decisions/`, which win. `for`
    /// prints the pages for the files they will touch, `backfill`
    /// records the pull requests merged before orion started, and
    /// `consolidate` merges the topics down to about 60.
    #[command(subcommand, after_help = KNOWLEDGE_EXAMPLES)]
    Knowledge(KnowledgeCommand),
    /// Take a ticket off the software factory.
    ///
    /// The bug fixer and the feature builder build Linear tickets on their
    /// own; `skip` is their cards' **Not for the factory**, from a shell.
    #[command(subcommand, after_help = FACTORY_EXAMPLES)]
    Factory(FactoryCommand),
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
    /// accounts to sign in; your first project, its base branch, setup
    /// command and run command; and the keys that matter. The palette's Run
    /// setup opens the same thing.
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
    /// Open a link in this session's account's browser.
    ///
    /// What a session's CLI runs, through the shim orion points its
    /// `BROWSER` at, to open a link itself — a newly published Artifact,
    /// an OAuth sign-in: the session's account's **Open links in** picks
    /// the browser and profile, as for a link clicked in its pane. Outside
    /// a session, or with no browser picked, it is plain `open <url>`.
    /// Exits non-zero when the link didn't open.
    #[command(hide = true, name = "open-link")]
    OpenLink {
        /// The link to open.
        url: String,
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
  orion config path               where each settings file lives";

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
pub(crate) enum PrCommand {
    /// A pull request's facts, for the top orion writes.
    ///
    /// `init` prints the skeleton; fill it in, then `set` hands it over.
    /// orion keeps the facts in a hidden block in the PR's body and
    /// works the verdict out from them and from GitHub.
    #[command(subcommand, after_help = PR_FACTS_EXAMPLES)]
    Facts(PrFactsCommand),
}

const PR_FACTS_EXAMPLES: &str = "\
Examples:
  orion pr facts init > /tmp/facts.json   the skeleton to fill in
  orion pr facts set /tmp/facts.json      hand it over for this branch's PR
  orion pr facts set - --pr 2108 < f.json the same from stdin, for PR #2108";

#[derive(Subcommand)]
pub(crate) enum PrFactsCommand {
    /// Print the facts skeleton to fill in.
    ///
    /// Every field, each holding what goes in it: what users get (at most
    /// two sentences), proof, gaps, your calls (each tagged product, arch,
    /// security, dep, scope, copy or ux), the flag, the docs touched and
    /// whether the plan was followed.
    #[command(after_help = "Example:\n  orion pr facts init > /tmp/facts.json")]
    Init,
    /// Hand orion the filled-in facts.
    ///
    /// Checks them — a placeholder left in, or a call without a known tag,
    /// is refused — and leaves them for orion, which rewrites the top of
    /// the PR's body on its next look.
    #[command(after_help = PR_FACTS_EXAMPLES)]
    Set {
        /// The filled-in JSON, or `-` to read it from stdin.
        file: String,
        /// The pull request, by URL or number (default: the current
        /// branch's).
        #[arg(long, value_name = "PR")]
        pr: Option<String>,
    },
}

const KNOWLEDGE_EXAMPLES: &str = "\
Examples:
  orion knowledge for src/auth/login.ts   the pages for a file
  orion knowledge backfill --project riplo-os --since 2026-06-01
                                          record what merged since June
  orion knowledge consolidate --project riplo-os
                                          merge the topics down to ~60";

#[derive(Subcommand)]
pub(crate) enum KnowledgeCommand {
    /// Print the recorded decisions for some files.
    ///
    /// Reads `.orion/decisions/harvested/` in the checkout it runs in.
    /// Maps the paths to topics by their words and by the paths the
    /// recorded entries cite, and prints at most 4 topic pages (2,400
    /// words). With no topic matching, prints the index.
    #[command(after_help = KNOWLEDGE_EXAMPLES)]
    For {
        /// The files or folders, relative to the current directory.
        #[arg(required = true, num_args = 1.., value_name = "PATH")]
        paths: Vec<String>,
        /// The project, by name or directory, to read from its main
        /// checkout (default: the checkout the current directory is in).
        #[arg(long, value_name = "NAME")]
        project: Option<String>,
    },
    /// Record the pull requests merged since a date.
    ///
    /// Lists the merged pull requests, joins each to the Claude and
    /// Cursor transcripts that worked on it (by branch, or by links to
    /// it), and harvests them oldest first with Opus, as at merge, into
    /// your knowledge pull request, a commit a batch; then checks every
    /// topic against the code as it is now. Pull requests recorded before
    /// are skipped, so it can be run again. Needs gh and a Claude account,
    /// not the TUI.
    #[command(
        after_help = KNOWLEDGE_EXAMPLES,
        // Two lines: on one it runs past a 60-column terminal.
        override_usage = "orion knowledge backfill --project <NAME>\n         --since <DATE>"
    )]
    Backfill {
        /// The project, by name or directory.
        #[arg(long, value_name = "NAME")]
        project: String,
        /// The first merge date to include, as YYYY-MM-DD.
        #[arg(long, value_name = "DATE")]
        since: String,
    },
    /// Merge the recorded topics down to about 60.
    ///
    /// Each area's topics go to Opus in one call, which merges near
    /// duplicates into at most 8 an area; the merged pages are rewritten
    /// with every rule and PR kept, the old ones deleted, and it is all
    /// proposed in your knowledge pull request as one commit. A backfill
    /// runs it at the end, and a harvest whenever it takes the topics
    /// past 60.
    #[command(after_help = KNOWLEDGE_EXAMPLES)]
    Consolidate {
        /// The project, by name or directory.
        #[arg(long, value_name = "NAME")]
        project: String,
    },
}

const FACTORY_EXAMPLES: &str = "\
Examples:
  orion factory skip ENG-7              off the factory, its status kept
  orion factory skip crash-fix --backlog
                                        by its session's name, to Backlog";

#[derive(Subcommand)]
pub(crate) enum FactoryCommand {
    /// Take a ticket off the factory for good.
    ///
    /// Labels the ticket `factory-skip`, which the intake never picks up
    /// until someone removes it, comments "Taken off the factory by
    /// <you>", closes the draft pull request its session opened, archives
    /// the session and marks the ticket parked in the builder ledger. Its
    /// status stays where it is, unless --backlog. Each step prints how it
    /// went. Works with or without orion running.
    #[command(after_help = FACTORY_EXAMPLES)]
    Skip {
        /// The ticket (`ENG-7`) or the name of the session working on it.
        target: String,
        /// Move the ticket to its team's Backlog too.
        #[arg(long)]
        backlog: bool,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("orion").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    #[test]
    fn pr_facts_init_and_set_parse() {
        assert!(matches!(
            parse(&["pr", "facts", "init"]).command,
            Some(Command::Pr(PrCommand::Facts(PrFactsCommand::Init)))
        ));
        let Some(Command::Pr(PrCommand::Facts(PrFactsCommand::Set { file, pr }))) =
            parse(&["pr", "facts", "set", "f.json"]).command
        else {
            panic!("not a set");
        };
        assert_eq!((file.as_str(), pr), ("f.json", None));
        let Some(Command::Pr(PrCommand::Facts(PrFactsCommand::Set { file, pr }))) =
            parse(&["pr", "facts", "set", "-", "--pr", "2108"]).command
        else {
            panic!("not a set");
        };
        assert_eq!((file.as_str(), pr.as_deref()), ("-", Some("2108")));
    }

    #[test]
    fn open_link_takes_one_url() {
        let url = "https://claude.ai/code/artifact/abc?x=1&y=2";
        let Some(Command::OpenLink { url: got }) = parse(&["open-link", url]).command else {
            panic!("not an open-link");
        };
        assert_eq!(got, url);
        assert!(Cli::try_parse_from(["orion", "open-link"]).is_err());
        assert!(Cli::try_parse_from(["orion", "open-link", "a", "b"]).is_err());
    }

    #[test]
    fn factory_skip_takes_a_target_and_an_optional_backlog() {
        let Some(Command::Factory(FactoryCommand::Skip { target, backlog })) =
            parse(&["factory", "skip", "ENG-7"]).command
        else {
            panic!("not a skip");
        };
        assert_eq!((target.as_str(), backlog), ("ENG-7", false));
        let Some(Command::Factory(FactoryCommand::Skip { target, backlog })) =
            parse(&["factory", "skip", "crash-fix", "--backlog"]).command
        else {
            panic!("not a skip");
        };
        assert_eq!((target.as_str(), backlog), ("crash-fix", true));
        assert!(Cli::try_parse_from(["orion", "factory", "skip"]).is_err());
        assert!(Cli::try_parse_from(["orion", "factory", "skip", "a", "b"]).is_err());
    }

    #[test]
    fn pr_facts_set_needs_a_file() {
        assert!(Cli::try_parse_from(["orion", "pr", "facts", "set"]).is_err());
    }
}
