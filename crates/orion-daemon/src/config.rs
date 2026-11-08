//! User settings, read from `paths::config_path()` with
//! `paths::config_local_path()` over it (JSON; see `orion_core::settings`).
//! Loaded fresh at each use so edits apply without restarting the daemon. A
//! missing file or unknown fields fall back to defaults; a malformed file is
//! logged and ignored rather than failing the operation that read it, and a
//! value this build can't read costs only its own key.

use orion_core::compose::WorktreeContainers;
use orion_core::{AgentKind, LaunchSpec};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Kill idle session PTYs in worktrees no client is looking at once
    /// they've gone unwatched this long: "1m" | "5m" | "15m" | "30m" | "1h"
    /// ("off" disables reaping entirely; any `<n>s`/`<n>m`/`<n>h` works).
    /// Bounds what prewarmed and walked-away-from sessions cost. Running
    /// or feedback-waiting agents, agents with a backgrounded tool call
    /// still running, and terminals with a command running are spared; a
    /// reaped session revives on the next attach or prewarm (agents resume
    /// their conversation). Malformed values fall back to the 5m default.
    pub session_idle_timeout: String,
    /// Per-harness deltas over the compiled-in registry (`harnesses` in
    /// config.json): repoint a program, rename a flag, switch a harness
    /// off, or define a whole new CLI. Merged by
    /// [`orion_core::harness::registry`]; a broken entry refuses its
    /// launches with the reason, never the whole daemon.
    pub harnesses: BTreeMap<String, orion_core::harness::HarnessOverride>,
    /// CLAUDE ACCOUNTS beyond the default one (`claude_accounts`): each
    /// a config dir under a stable id, launched as Claude's own row with
    /// `CLAUDE_CONFIG_DIR` pointing there. The TUI's Accounts tab and
    /// first-run onboarding edit the list; the daemon spawns, resumes and
    /// continues sessions on it through the registry.
    pub claude_accounts: Vec<orion_core::claude_account::ClaudeAccount>,
    /// PROJECT SETTINGS: one entry per project set up differently from the
    /// rest, keyed by the project's repo path as the store holds it. The
    /// TUI's Projects tab owns the map; the daemon reads the keys in it
    /// that are its to act on ([`Config::base_branch`],
    /// [`Config::setup_command`], [`Config::run_command`],
    /// [`Config::worktree_containers`]).
    pub projects: BTreeMap<PathBuf, ProjectConfig>,
    /// The harness `⌘N` launches on (the TUI's **New agents on**): a
    /// built-in's name or a Claude account's id. Read for what a SPARE
    /// WORKTREE boots ([`Config::spare_launch`]).
    pub quick_prompt_kind: String,
    /// The default Claude account's switch, where `harnesses.claude` says
    /// nothing (the TUI's Accounts tab). Off, no SPARE WORKTREE boots it.
    pub claude_enabled: bool,
    /// AWAY AND PHONE (`away`, the TUI's Connections tab): when you count
    /// as away, and what the phone bridge sends then.
    pub away: orion_core::away::AwaySettings,
}

/// One project's entry under `projects` — the rows of the TUI's Projects
/// tab, of which the daemon reads four. The rest (`open_command`) are the
/// TUI's and pass through unread.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// The branch the project's new WORKTREES start from when nobody
    /// named a base — a bare `orion worktree`, the QUICK PROMPT's **+ new
    /// worktree**. Empty (the default) means origin's own default branch,
    /// `origin/HEAD` as freshly fetched; a name (`dev`, `master`) means
    /// origin's fetched copy of that branch when origin has one, else the
    /// checkout's local ref of that name, else — a repo with no such
    /// branch at all — the default again, with a warning in the daemon
    /// log. Read through [`Config::base_branch`].
    pub base_branch: String,
    /// The SETUP COMMAND run once in each worktree orion creates for this
    /// project, in a setup row in its band, typed into Settings →
    /// Projects. Empty means there is none.
    pub setup_command: String,
    /// The RUN COMMAND a menu's **Run** starts in this project's
    /// worktrees, typed into Settings → Projects. Empty means there is
    /// none.
    pub run_command: String,
    /// ON WORKTREE DELETE: what a deleted worktree's docker compose
    /// projects get — empty or `off` (the default), `stop`, `remove`, or
    /// `remove+volumes`. Read through [`Config::worktree_containers`];
    /// anything unknown is `off` (see `containers`).
    pub worktree_containers: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            session_idle_timeout: DEFAULT_SESSION_IDLE_TIMEOUT.into(),
            harnesses: BTreeMap::new(),
            claude_accounts: Vec::new(),
            projects: BTreeMap::new(),
            quick_prompt_kind: AgentKind::Claude.as_str().into(),
            claude_enabled: false,
            away: Default::default(),
        }
    }
}

pub use orion_core::settings::DEFAULT_SESSION_IDLE_TIMEOUT;

impl Config {
    pub fn load() -> Self {
        let loaded = orion_core::settings::load::<Self>(
            &orion_core::paths::config_path(),
            &orion_core::paths::config_local_path(),
        );
        for problem in &loaded.problems {
            tracing::warn!("{problem}");
        }
        if !loaded.skipped.is_empty() {
            tracing::warn!(keys = ?loaded.skipped, "settings this build can't read keep their defaults");
        }
        loaded.value
    }

    /// `session_idle_timeout` parsed to a duration; None = reaping disabled.
    pub fn session_idle_timeout(&self) -> Option<std::time::Duration> {
        parse_timeout(&self.session_idle_timeout)
            .unwrap_or_else(|| parse_timeout(DEFAULT_SESSION_IDLE_TIMEOUT).expect("default parses"))
    }

    /// The BASE BRANCH set for the project checked out at `repo_path`, or
    /// None for the default (origin's own default branch). Whitespace is
    /// trimmed and a leading `origin/` dropped: `origin/master` means the
    /// same as `master` — origin's fetched copy when it has one — and
    /// spelling it out must not turn into a branch that tracks
    /// `origin/master` and aims its first push there.
    pub fn base_branch(&self, repo_path: &Path) -> Option<&str> {
        let name = self.projects.get(repo_path)?.base_branch.trim();
        let name = name.strip_prefix("origin/").unwrap_or(name).trim();
        (!name.is_empty()).then_some(name)
    }

    /// The launch `⌘N` makes on its own defaults — the harness
    /// `quick_prompt_kind` names, at that harness's default model and
    /// effort, in edit mode — when it is a Claude account that is on: what
    /// a SPARE WORKTREE's CLI boots as. Read the way the TUI's
    /// `Config::quick_prompt_harness` reads it: an account switched off or
    /// gone falls back to the default one. None when `⌘N` starts another
    /// harness, or the account it would start is off.
    pub fn spare_launch(&self) -> Option<LaunchSpec> {
        let registry = orion_core::harness::registry(&self.harnesses, &self.claude_accounts);
        let claude = || registry.iter().find(|e| e.id == AgentKind::Claude.as_str());
        let name = self.quick_prompt_kind.trim();
        let entry = match AgentKind::parse(name) {
            Some(AgentKind::Claude) => claude(),
            Some(_) => None,
            None => registry
                .iter()
                .find(|e| {
                    e.id == name && e.enabled && e.problem().is_none() && e.is_claude_account()
                })
                .or_else(claude),
        }?;
        // The default account's own switch is the legacy key wherever the
        // `harnesses` map says nothing, as the TUI folds it.
        let on = match self.harnesses.get(&entry.id).and_then(|o| o.enabled) {
            _ if entry.id != AgentKind::Claude.as_str() => entry.enabled,
            Some(enabled) => enabled,
            None => self.claude_enabled,
        };
        if !on || !entry.is_claude_account() || entry.problem().is_some() {
            return None;
        }
        let (kind, custom_harness) = entry.session_kind();
        Some(LaunchSpec {
            kind,
            custom_harness,
            model: entry.default_model().map(str::to_string),
            effort: entry.default_effort().map(str::to_string),
            mode: Default::default(),
        })
    }

    /// The ON WORKTREE DELETE policy of the project checked out at
    /// `repo_path`; no entry, or a word this build doesn't know, is `off`.
    pub fn worktree_containers(&self, repo_path: &Path) -> WorktreeContainers {
        self.projects
            .get(repo_path)
            .map_or(WorktreeContainers::Off, |p| {
                WorktreeContainers::parse(&p.worktree_containers)
            })
    }

    /// The RUN COMMAND set for the project checked out at `repo_path` in
    /// Settings → Projects, trimmed; None when the project has no entry or
    /// the row is empty.
    pub fn run_command(&self, repo_path: &Path) -> Option<&str> {
        self.command_line(repo_path, |p| &p.run_command)
    }

    /// The SETUP COMMAND set for the project checked out at `repo_path`,
    /// read as [`Config::run_command`] is.
    pub fn setup_command(&self, repo_path: &Path) -> Option<&str> {
        self.command_line(repo_path, |p| &p.setup_command)
    }

    /// One of the project's typed command lines, trimmed; None for no
    /// entry or an empty row.
    fn command_line(&self, repo_path: &Path, field: fn(&ProjectConfig) -> &String) -> Option<&str> {
        self.projects
            .get(repo_path)
            .map(|p| field(p).trim())
            .filter(|c| !c.is_empty())
    }
}

/// "off"/"0" → Some(None); "<n>s"/"<n>m"/"<n>h" → Some(Some(d));
/// malformed → None (caller falls back to the default).
#[allow(clippy::option_option)]
fn parse_timeout(s: &str) -> Option<Option<std::time::Duration>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("off") || s == "0" {
        return Some(None);
    }
    let (digits, unit_secs) = match s.strip_suffix(['s', 'S']) {
        Some(d) => (d, 1),
        None => match s.strip_suffix(['m', 'M']) {
            Some(d) => (d, 60),
            None => (s.strip_suffix(['h', 'H'])?, 3_600),
        },
    };
    let n: u64 = digits.trim().parse().ok()?;
    if n == 0 {
        return Some(None);
    }
    Some(Some(std::time::Duration::from_secs(n * unit_secs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One mistyped key — or a key a newer orion changed the type of —
    /// takes its default without dragging every other setting down with it.
    #[test]
    fn an_unreadable_key_costs_only_that_key() {
        let obj = serde_json::json!({
            "projects": "nope",
            "session_idle_timeout": "1h",
        });
        let (cfg, skipped) =
            orion_core::settings::parse_lenient::<Config>(obj.as_object().unwrap());
        assert!(
            cfg.projects.is_empty(),
            "the unreadable key takes its default"
        );
        assert_eq!(cfg.session_idle_timeout, "1h");
        assert_eq!(skipped.len(), 1);
    }

    /// The daemon's half of the TUI's fixture test: every daemon key the
    /// newest release wrote still reads as written.
    #[test]
    fn the_newest_releases_config_still_loads_the_daemon_keys() {
        let raw = include_str!("../../orion-core/fixtures/config-2.2.1.json");
        let obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(raw).unwrap();
        let (cfg, skipped) = orion_core::settings::parse_lenient::<Config>(&obj);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(cfg.session_idle_timeout, "30m");
        assert_eq!(
            cfg.harnesses.get("claude").and_then(|h| h.enabled),
            Some(false),
            "the registry map reads"
        );
        assert_eq!(cfg.claude_accounts.len(), 1);
        assert_eq!(cfg.claude_accounts[0].id, "claude-work");
        assert!(!cfg.claude_accounts[0].enabled, "the account switched off");
        assert_eq!(
            cfg.run_command(Path::new("/Users/me/src/app")),
            Some("npm run dev"),
            "the project's run command reads"
        );
    }

    /// `claude_accounts` reads as the TUI writes it: two keys an entry,
    /// `enabled` only when off.
    #[test]
    fn claude_accounts_read_as_ids_and_dirs() {
        let cfg: Config = serde_json::from_str(
            r#"{"claude_accounts": [
                {"id": "claude-2", "config_dir": "~/.claude-2"},
                {"id": "claude-work", "config_dir": "/srv/claude", "enabled": false}
            ]}"#,
        )
        .unwrap();
        assert_eq!(cfg.claude_accounts.len(), 2);
        assert!(cfg.claude_accounts[0].enabled);
        assert!(!cfg.claude_accounts[1].enabled);
        assert!(Config::default().claude_accounts.is_empty());
    }

    /// A SPARE WORKTREE boots what `⌘N` launches, on its own defaults —
    /// the default account or another, never another harness, never one
    /// that is off.
    #[test]
    fn the_spare_boots_what_cmd_n_launches() {
        let cfg = |json: serde_json::Value| -> Config { serde_json::from_value(json).unwrap() };
        assert_eq!(Config::default().spare_launch(), None, "Claude starts off");
        let on = cfg(serde_json::json!({ "claude_enabled": true }));
        assert_eq!(
            on.spare_launch(),
            Some(LaunchSpec {
                kind: AgentKind::Claude,
                custom_harness: None,
                model: None,
                effort: None,
                mode: Default::default(),
            })
        );
        let tuned = cfg(serde_json::json!({
            "claude_enabled": true,
            "harnesses": { "claude": { "model_default": "opus", "effort_default": "high" } },
        }));
        let launch = tuned.spare_launch().unwrap();
        assert_eq!(
            (launch.model.as_deref(), launch.effort.as_deref()),
            (Some("opus"), Some("high"))
        );
        let work = serde_json::json!({
            "claude_enabled": true,
            "quick_prompt_kind": "claude-work",
            "claude_accounts": [{ "id": "claude-work", "config_dir": "/srv/claude" }],
            "harnesses": { "claude-work": { "model_default": "sonnet" } },
        });
        let launch = cfg(work.clone()).spare_launch().unwrap();
        assert_eq!(launch.kind, AgentKind::Claude);
        assert_eq!(launch.custom_harness.as_deref(), Some("claude-work"));
        assert_eq!(launch.model.as_deref(), Some("sonnet"));
        let mut gone = work.clone();
        gone["claude_accounts"][0]["enabled"] = false.into();
        assert_eq!(
            cfg(gone).spare_launch().unwrap().custom_harness,
            None,
            "an account switched off falls back to the default one"
        );
        let codex =
            cfg(serde_json::json!({ "claude_enabled": true, "quick_prompt_kind": "codex" }));
        assert_eq!(
            codex.spare_launch(),
            None,
            "⌘N on Codex keeps no Claude warm"
        );
        let off = cfg(serde_json::json!({
            "claude_enabled": true,
            "harnesses": { "claude": { "enabled": false } },
        }));
        assert_eq!(off.spare_launch(), None, "the map's switch wins");
    }

    #[test]
    fn session_idle_timeout_parses_and_falls_back() {
        use std::time::Duration;
        let timeout = |v: &str| {
            let cfg: Config =
                serde_json::from_str(&format!(r#"{{"session_idle_timeout": "{v}"}}"#)).unwrap();
            cfg.session_idle_timeout()
        };
        assert_eq!(timeout("1m"), Some(Duration::from_secs(60)));
        assert_eq!(timeout("5m"), Some(Duration::from_secs(300)));
        assert_eq!(timeout("15m"), Some(Duration::from_secs(900)));
        assert_eq!(timeout("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(timeout("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(
            timeout("2s"),
            Some(Duration::from_secs(2)),
            "seconds for tests"
        );
        assert_eq!(timeout("off"), None);
        assert_eq!(timeout("0"), None);
        // Malformed values fall back to the default, not to disabled.
        assert_eq!(timeout("soon"), Some(Duration::from_secs(300)));
        assert_eq!(
            Config::default().session_idle_timeout(),
            Some(Duration::from_secs(300))
        );
    }

    /// A project's base branch is its own: read under its repo path,
    /// trimmed and with `origin/` dropped; another project, or one with no
    /// entry, starts from origin's default.
    #[test]
    fn base_branch_is_read_per_project_and_normalizes() {
        let demo = Path::new("/tmp/demo");
        assert_eq!(Config::default().base_branch(demo), None);
        let base = |v: &str| {
            let cfg: Config = serde_json::from_str(&format!(
                r#"{{"projects": {{"/tmp/demo": {{"base_branch": "{v}"}}}}}}"#
            ))
            .unwrap();
            cfg.base_branch(demo).map(str::to_string)
        };
        assert_eq!(base(""), None);
        assert_eq!(base("   "), None, "blank is unset, not a branch called ' '");
        assert_eq!(base("master"), Some("master".into()));
        assert_eq!(base("  develop "), Some("develop".into()));
        assert_eq!(
            base("origin/master"),
            Some("master".into()),
            "origin/x is x: origin's copy is what the name already means"
        );
        assert_eq!(base("origin/"), None);
        let cfg: Config =
            serde_json::from_str(r#"{"projects": {"/tmp/demo": {"base_branch": "dev"}}}"#).unwrap();
        assert_eq!(cfg.base_branch(Path::new("/tmp/other")), None);
        let old: Config = serde_json::from_str(r#"{"worktree_base_branch": "dev"}"#).unwrap();
        assert_eq!(
            old.base_branch(demo),
            None,
            "the old global key is the TUI's to move, never read here"
        );
    }

    /// The Projects tab's `run_command`, under the project's repo path:
    /// trimmed, blank is unset, and the entry's other rows — the TUI's —
    /// don't stop the daemon reading it.
    #[test]
    fn run_command_is_read_per_project_and_blank_means_the_file() {
        assert_eq!(Config::default().run_command(Path::new("/tmp/demo")), None);
        let cfg: Config = serde_json::from_str(
            r#"{"projects": {
                "/tmp/demo": { "open_command": "open http://localhost:3000", "run_command": "  npm run dev " },
                "/tmp/blank": { "run_command": "   " },
                "/tmp/other": { "hide_root_worktree": false, "future_row": 1 }
            }}"#,
        )
        .unwrap();
        assert_eq!(cfg.run_command(Path::new("/tmp/demo")), Some("npm run dev"));
        assert_eq!(cfg.run_command(Path::new("/tmp/blank")), None);
        assert_eq!(cfg.run_command(Path::new("/tmp/other")), None);
        assert_eq!(cfg.run_command(Path::new("/tmp/unknown")), None);
        // A `projects` this build can't read costs the map, not the rest.
        let obj = serde_json::json!({
            "projects": { "/tmp/demo": { "run_command": ["npm", "run", "dev"] } },
            "session_idle_timeout": "1h",
        });
        let (cfg, skipped) =
            orion_core::settings::parse_lenient::<Config>(obj.as_object().unwrap());
        assert_eq!(cfg.run_command(Path::new("/tmp/demo")), None);
        assert_eq!(cfg.session_idle_timeout, "1h");
        assert_eq!(skipped.into_iter().collect::<Vec<_>>(), ["projects"]);
    }

    /// The Projects tab's `setup_command` reads as `run_command` does, and
    /// apart from it.
    #[test]
    fn setup_command_is_read_per_project_apart_from_the_run_command() {
        assert_eq!(
            Config::default().setup_command(Path::new("/tmp/demo")),
            None
        );
        let cfg: Config = serde_json::from_str(
            r#"{"projects": {
                "/tmp/demo": { "setup_command": " just setup-worktree  ", "run_command": "just start" },
                "/tmp/run-only": { "run_command": "just start" },
                "/tmp/blank": { "setup_command": "  " }
            }}"#,
        )
        .unwrap();
        let demo = Path::new("/tmp/demo");
        assert_eq!(cfg.setup_command(demo), Some("just setup-worktree"));
        assert_eq!(cfg.run_command(demo), Some("just start"));
        assert_eq!(cfg.setup_command(Path::new("/tmp/run-only")), None);
        assert_eq!(cfg.setup_command(Path::new("/tmp/blank")), None);
    }

    #[test]
    fn worktree_containers_default_off_and_read_the_policy_per_project() {
        let demo = Path::new("/tmp/demo");
        assert_eq!(
            Config::default().worktree_containers(demo),
            WorktreeContainers::Off
        );
        let read = |v: &str| {
            serde_json::from_str::<Config>(&format!(
                r#"{{"projects": {{"/tmp/demo": {{"worktree_containers": "{v}"}}}}}}"#
            ))
            .unwrap()
            .worktree_containers(demo)
        };
        assert_eq!(read("remove+volumes"), WorktreeContainers::RemoveVolumes);
        assert_eq!(read("stop"), WorktreeContainers::Stop);
        assert_eq!(
            read("nuke"),
            WorktreeContainers::Off,
            "unknown deletes nothing"
        );
        let cfg: Config = serde_json::from_str(r#"{"worktree_containers": "stop"}"#).unwrap();
        assert_eq!(
            cfg.worktree_containers(demo),
            WorktreeContainers::Off,
            "the old global key is the TUI's to move, never read here"
        );
    }
}
