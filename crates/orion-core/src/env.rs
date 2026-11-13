//! Names of the environment variables orion reads and sets, so the daemon,
//! the TUI, the CLI, the hook installers and the e2e tests all spell them
//! from one place — a typo here fails to build instead of silently falling
//! back to a default.

use std::path::PathBuf;
use std::time::Duration;

/// Id of the agent a hook or CLI invocation is running inside. Set on every
/// agent PTY, scrubbed from plain terminals.
pub const AGENT_ID: &str = "ORION_AGENT_ID";
/// Base URL of the daemon's hook receiver, set on agent PTYs.
pub const API_URL: &str = "ORION_API_URL";
/// Bearer token the hook receiver expects, set on agent PTYs.
pub const API_TOKEN: &str = "ORION_API_TOKEN";
/// Overrides the runtime dir holding the socket and pidfile.
pub const RUNTIME_DIR: &str = "ORION_RUNTIME_DIR";
/// Overrides the data dir holding the database, config and logs.
pub const DATA_DIR: &str = "ORION_DATA_DIR";
/// Set by `make dev`: this daemon is a DEV INSTANCE whose projects were
/// seeded from the real one's, so the checkouts it lists belong to the
/// real daemon. It leaves them where they are: no boot relocation of old
/// worktree folders and no SPARE WORKTREES cut, kept or swept — each of
/// those changes the real repos under the real daemon, which still has
/// the old paths.
pub const DEV_INSTANCE: &str = "ORION_DEV_INSTANCE";
/// Moves `config.json` alone — into a dotfiles checkout, say — leaving the
/// database, the logs and `config.local.json` in the data dir.
pub const CONFIG_FILE: &str = "ORION_CONFIG_FILE";
/// Replaces every agent CLI with one command line, taken verbatim (tests
/// stand in `/bin/sh` or a stub script for `claude`).
pub const AGENT_CMD: &str = "ORION_AGENT_CMD";
/// Idle-session reaper sweep period in ms; tests shorten it.
pub const IDLE_REAP_MS: &str = "ORION_IDLE_REAP_MS";
/// External-worktree sync probe period in ms; tests shorten it.
pub const WORKTREE_SYNC_MS: &str = "ORION_WORKTREE_SYNC_MS";
/// How often (ms, default 30 s) the DAEMON tends its SPARE WORKTREES:
/// cuts one for a project in use, refreshes or removes one; tests shorten it.
pub const SPARE_TICK_MS: &str = "ORION_SPARE_TICK_MS";
/// STACK STATUS poll period in ms (default 5s): how often the daemon asks
/// docker which compose stacks exist and run.
pub const STACK_POLL_MS: &str = "ORION_STACK_POLL_MS";
/// How often (ms, default 5 s) the DAEMON reads whether you are away —
/// the screen lock and the time since the last key — for the phone
/// bridge. `0` turns the reading off, as the e2e tests do: then only the
/// hand flag (⌘⇧A) makes you away. It runs only while Slack is connected.
pub const AWAY_POLL_MS: &str = "ORION_AWAY_POLL_MS";
/// How long (ms, default 20 s) after a phone answer goes into a dialog
/// the DAEMON looks again: a session still waiting on the same thing
/// didn't take it, and its phone message gets its buttons back. The e2e
/// tests shorten it.
pub const PHONE_ANSWER_CHECK_MS: &str = "ORION_PHONE_ANSWER_CHECK_MS";
/// The Slack Web API's base URL (default `https://slack.com/api`); the
/// e2e tests point it at a fake Slack. Socket Mode's websocket is
/// wherever that API's `apps.connections.open` says it is.
pub const SLACK_API_URL: &str = "ORION_SLACK_API_URL";
/// The three Slack tokens as `<app>,<bot>,<user>`, read in place of the
/// Keychain's `dev.orion.slack` items: the e2e tests' fake Slack, never a
/// real workspace's. `off` is no tokens at all, so a test or screenshot
/// daemon never signs in to the machine's real Slack (a second Socket
/// Mode connection would take a share of its button presses).
pub const SLACK_TOKENS: &str = "ORION_SLACK_TOKENS";
/// `RUST_LOG`-style tracing filter for both the daemon and the TUI.
pub const LOG: &str = "ORION_LOG";
/// Overrides the install script URL `orion upgrade` / `orion ssh` fetch.
pub const INSTALL_URL: &str = "ORION_INSTALL_URL";
/// Editor command the file modals open in place of fresh.
pub const EDITOR: &str = "ORION_EDITOR";
/// Cadence in seconds of the TUI's check for a newer published release
/// (the footer's `⇡ vX.Y.Z` update indicator); `0` turns it off, as the
/// e2e tests do so their footers never depend on what GitHub has published.
pub const UPDATE_CHECK_SECS: &str = "ORION_UPDATE_CHECK_SECS";
/// Cadence in seconds of the footer's Spotify poll; `0` turns it off, as the
/// e2e tests do so their footers never depend on what's playing.
pub const SPOTIFY_POLL_SECS: &str = "ORION_SPOTIFY_POLL_SECS";
/// Cadence in seconds of the Microsoft 365 calendar poll behind the
/// footer's next meeting (default 5 minutes); `0` turns it off, as the e2e
/// tests do so a run never calls Microsoft Graph.
pub const MICROSOFT_POLL_SECS: &str = "ORION_MICROSOFT_POLL_SECS";
/// Cadence in seconds of MEETING TAKEAWAYS' look for a finished Teams
/// meeting's transcript (default 2 minutes, for up to an hour after the
/// meeting ends); `0` turns it off, as the e2e tests and the screenshots
/// do, so a run never asks Microsoft Graph for a transcript.
pub const MEETING_TAKEAWAYS_POLL_SECS: &str = "ORION_MEETING_TAKEAWAYS_POLL_SECS";
/// Microsoft Graph's base URL (default `https://graph.microsoft.com/v1.0`);
/// the tests point it at a fake Graph serving canned JSON and VTT.
pub const MICROSOFT_GRAPH_URL: &str = "ORION_MICROSOFT_GRAPH_URL";
/// A Graph access token read in place of the Keychain's sign-in: the
/// tests' fake Graph, never a real tenant's.
pub const MICROSOFT_TOKEN: &str = "ORION_MICROSOFT_TOKEN";
/// `off`: a project's **What it is** line is never written by a model, as
/// the e2e tests set it, so adding a project never runs a Claude account.
pub const PROJECT_ABOUT: &str = "ORION_PROJECT_ABOUT";
/// Cadence in seconds of the NEEDS YOU poll — your Linear notifications
/// and the code reviews asked of you; `0` turns it off, as the e2e tests
/// and the screenshots do so a board never depends on Linear or GitHub.
pub const NEEDS_YOU_POLL_SECS: &str = "ORION_NEEDS_YOU_POLL_SECS";
/// Cadence in seconds of the bug fixer's and feature builder's look for
/// labelled Linear tickets assigned to you (default 2 minutes); `0` turns
/// it off, as the e2e tests and the screenshots do.
pub const LINEAR_BUILDER_POLL_SECS: &str = "ORION_LINEAR_BUILDER_POLL_SECS";
/// Cadence in seconds of the check for the WEEK IN REVIEW's scheduled
/// time (default a minute); `0` turns it off, so no run starts on its own.
pub const WEEK_REVIEW_CHECK_SECS: &str = "ORION_WEEK_REVIEW_CHECK_SECS";
/// Seconds the WEEK IN REVIEW waits before each try again of a project's
/// GitHub or Linear fetch that failed, and before a scheduled run that
/// could not fetch is run again (defaults 10, 30 and 90 seconds, and 15
/// minutes); `0` turns trying again off, so the first failure is final.
pub const WEEK_REVIEW_RETRY_SECS: &str = "ORION_WEEK_REVIEW_RETRY_SECS";
/// The folder the BLUE SKY BUILDER builds ideas tied to no project in
/// (default `~/Code/orion-blue-sky`): the e2e tests point it at a scratch
/// directory, so a run never makes the real one.
pub const BLUE_SKY_DIR: &str = "ORION_BLUE_SKY_DIR";
/// `off`: ACCOUNT USAGE asks no provider — as the e2e tests set it, so a
/// run never reads the machine's logins or calls out with them.
pub const USAGE: &str = "ORION_USAGE";
/// A file the TUI writes its INPUT LATENCY PROBE's timeline to
/// (`make perf`); unset, there is no probe.
pub const PERF_LOG: &str = "ORION_PERF_LOG";
/// Set by `orion upgrade` on the install script it runs, so the script
/// leaves the "daemon still running" note to the upgrade. `install.sh`
/// reads it by this name.
pub const UPGRADE_HANDOFF: &str = "ORION_UPGRADE_HANDOFF";
/// Claude Code's own override of its config dir (`~/.claude`), honoured
/// wherever orion reads Claude's settings or transcripts.
pub const CLAUDE_CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
/// Codex's own override of its home (`~/.codex`), where orion installs
/// its hooks.
pub const CODEX_HOME: &str = "CODEX_HOME";
/// Grok Build's own override of its home (`~/.grok`), under whose `hooks/`
/// orion installs its hooks.
pub const GROK_HOME: &str = "GROK_HOME";

/// Env vars that identify an agent session to the daemon. They are set on
/// every agent PTY and must never leak into plain terminals.
pub const AGENT_SESSION_VARS: &[&str] = &[AGENT_ID, API_URL, API_TOKEN];

/// `TERM` every PTY child is given. The pane is orion's own grid — a vt100
/// parser the TUI repaints through ratatui — and that grid keeps 24-bit
/// colour whatever terminal orion itself runs in, so the child never
/// hears the host's `TERM` (`foot`, `xterm-ghostty`, `tmux-256color`).
pub const PANE_TERM: &str = "xterm-256color";
/// `COLORTERM` for the same grid: every `38;2;r;g;b` the child sends is
/// kept, so chalk-style detection may pick truecolor over the 256-colour
/// downsample `TERM` alone allows.
pub const PANE_COLORTERM: &str = "truecolor";
/// Colour overrides scrubbed from every PTY child. A `NO_COLOR` or a
/// `FORCE_COLOR=0` describes the shell the daemon happened to be started
/// from — an agent's tool shell, a CI job — or a login-only profile, not
/// the pane; Claude Code reads either as "no colour at all" and paints its
/// whole UI in the default foreground while the TUI around it stays
/// coloured (#37). The TUI still honours its own `NO_COLOR` on the way
/// out, so a user who wants none keeps none.
pub const PANE_COLOR_OVERRIDES: &[&str] = &["NO_COLOR", "FORCE_COLOR"];

/// Whether this process is a `make dev` DEV INSTANCE ([`DEV_INSTANCE`]).
pub fn dev_instance() -> bool {
    non_empty(DEV_INSTANCE).is_some_and(|v| v != "0")
}

/// The value of `var`, treating unset and empty the same way — an empty
/// override is how a caller says "use the default".
pub fn non_empty(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// A cadence in seconds read from `var` — [`UPDATE_CHECK_SECS`],
/// [`SPOTIFY_POLL_SECS`] — through [`parse_secs`].
pub fn secs_override(var: &str, default: Duration) -> Option<Duration> {
    parse_secs(non_empty(var).as_deref(), default)
}

/// A seconds override as a cadence: unset (or empty) and anything that
/// isn't a whole number are `default`; `0` is `None`, off.
pub fn parse_secs(value: Option<&str>, default: Duration) -> Option<Duration> {
    match value.map(str::parse::<u64>) {
        None | Some(Err(_)) => Some(default),
        Some(Ok(0)) => None,
        Some(Ok(secs)) => Some(Duration::from_secs(secs)),
    }
}

/// `$HOME`, when the environment has one. Read as an `OsString` so a
/// non-UTF-8 home still resolves — every `~/` expansion goes through here.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seconds_override_reads_as_a_cadence() {
        let default = Duration::from_secs(60);
        assert_eq!(parse_secs(None, default), Some(default), "unset");
        assert_eq!(parse_secs(Some("0"), default), None, "0 turns it off");
        assert_eq!(
            parse_secs(Some("90"), default),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            parse_secs(Some("soon"), default),
            Some(default),
            "nonsense is the default"
        );
        assert_eq!(parse_secs(Some("-1"), default), Some(default));
    }

    #[test]
    fn non_empty_treats_unset_and_empty_alike() {
        let var = format!("ORION_TEST_NON_EMPTY_{}", std::process::id());
        assert_eq!(non_empty(&var), None);
        std::env::set_var(&var, "");
        assert_eq!(non_empty(&var), None);
        std::env::set_var(&var, "x");
        assert_eq!(non_empty(&var).as_deref(), Some("x"));
        std::env::remove_var(&var);
    }
}
