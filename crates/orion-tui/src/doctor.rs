//! `orion doctor`: what this machine has of what orion leans on, a line
//! each — git, gh and its sign-in, the **File editor** and what really
//! opens, the **Open in app** editor, Orion.app and the Ghostty it is made
//! from, the CLI of every agent turned on, and the project's
//! `LINEAR_API_KEY` (where it was found, never the key), and the docker
//! compose projects whose checkout is gone — with the command that fixes
//! whatever is missing. It never installs anything itself:
//! `install.sh` and the onboarding wizard do, with the same commands
//! (`install`).
//!
//! Only git, and an editor files can open in, are required; the exit code
//! is non-zero only when one of those is missing. Everything is probed
//! against a [`Machine`] — a PATH, a home, the Applications folders — so
//! the tests name their own.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::config::Config;
use crate::install::{which, Tools};

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// ✓ here and working.
    Ok,
    /// ✗ missing, or not working: the fix says what to run.
    Missing,
    /// – nothing to check: not in use, or not on this platform.
    Skipped,
}

impl Status {
    fn mark(self) -> &'static str {
        match self {
            Status::Ok => "✓",
            Status::Missing => "✗",
            Status::Skipped => "–",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    /// The command — or the page — that fixes a missing one.
    pub fix: Option<String>,
    /// orion can't do its job without it: missing, the exit code says so.
    pub required: bool,
}

impl Check {
    fn new(name: impl Into<String>, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
            fix: None,
            required: false,
        }
    }

    fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Missing, and orion can't do without it.
    pub fn fails(&self) -> bool {
        self.required && self.status == Status::Missing
    }
}

/// Where the checks look: the PATH programs are found and run on, the
/// home (Orion.app's), the folders holding an `Applications` folder, the
/// directory whose project is checked for Linear, and the variable that
/// steers the editor.
#[derive(Debug, Clone, Default)]
pub struct Machine {
    pub path: OsString,
    pub home: Option<PathBuf>,
    pub app_roots: Vec<PathBuf>,
    pub cwd: PathBuf,
    pub macos: bool,
    /// `ORION_EDITOR`.
    pub editor_env: Option<String>,
}

impl Machine {
    /// This machine, as orion would see it from this shell.
    pub fn here() -> Self {
        let home = orion_core::env::home_dir();
        let macos = cfg!(target_os = "macos");
        let mut app_roots = Vec::new();
        if macos {
            app_roots.push(PathBuf::from("/"));
            app_roots.extend(home.clone());
        }
        Self {
            path: std::env::var_os("PATH").unwrap_or_default(),
            home,
            app_roots,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            macos,
            editor_env: orion_core::env::non_empty(orion_core::env::EDITOR),
        }
    }

    fn find(&self, program: &str) -> Option<PathBuf> {
        which(&self.path, program)
    }

    fn tools(&self) -> Tools {
        Tools::on(&self.path, false)
    }
}

/// How long a probe (`gh auth status` asks GitHub) may take before it
/// counts as failed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Run `program args` with `m`'s PATH in `cwd`: whether it succeeded, and
/// its first line of output. None when it could not be started or ran
/// past [`PROBE_TIMEOUT`].
fn probe(m: &Machine, program: &Path, args: &[&str], cwd: &Path) -> Option<(bool, String)> {
    let (ok, out) = probe_output(m, program, args, cwd)?;
    let first = out.lines().map(str::trim).find(|l| !l.is_empty());
    Some((ok, first.unwrap_or_default().to_string()))
}

/// [`probe`] with all of its output rather than the first line.
fn probe_output(m: &Machine, program: &Path, args: &[&str], cwd: &Path) -> Option<(bool, String)> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("PATH", &m.path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut out);
    }
    Some((status.success(), out))
}

/// Every check, in the report's order.
pub fn checks(cfg: &Config, m: &Machine) -> Vec<Check> {
    let mut out = vec![git(m), gh(m), editor(cfg, m), open_in_app(cfg, m)];
    out.push(orion_app(m));
    out.extend(agents(cfg, m));
    out.push(linear(m));
    out.push(containers(cfg, m));
    out
}

/// What a check's `fix:` says for an install: the line Settings → Tools
/// and setup run on `i`, else the page to get it from.
fn plan_fix(plan: crate::install::Plan) -> String {
    if plan.line.is_empty() {
        plan.link.into()
    } else {
        plan.line
    }
}

fn git(m: &Machine) -> Check {
    let fix = if m.macos {
        "xcode-select --install"
    } else {
        "https://git-scm.com/downloads"
    };
    match m.find("git") {
        None => Check::new(
            "git",
            Status::Missing,
            "not on PATH — orion needs git for every project",
        )
        .fix(fix)
        .required(),
        Some(git) => match probe(m, &git, &["--version"], &m.cwd) {
            Some((true, version)) => Check::new("git", Status::Ok, version).required(),
            _ => Check::new(
                "git",
                Status::Missing,
                format!("{} doesn't run", git.display()),
            )
            .fix(fix)
            .required(),
        },
    }
}

/// gh, which pull requests and issues are read with: installed, and
/// signed in.
fn gh(m: &Machine) -> Check {
    let Some(gh) = m.find("gh") else {
        let fix = m.tools().cli_plan("gh").map(plan_fix).unwrap_or_default();
        return Check::new(
            "gh",
            Status::Missing,
            "not on PATH — pull requests and issues need it",
        )
        .fix(fix);
    };
    match probe(m, &gh, &["auth", "status"], &m.cwd) {
        Some((true, _)) => Check::new("gh", Status::Ok, "signed in to GitHub"),
        _ => Check::new("gh", Status::Missing, "installed, not signed in").fix("gh auth login"),
    }
}

/// The **File editor**: the one chosen, and what opens when it is missing.
fn editor(cfg: &Config, m: &Machine) -> Check {
    let installed = |program: &str| m.find(program).is_some();
    let resolved = crate::editor::resolve(m.editor_env.as_deref(), &cfg.editor, installed);
    let program = resolved
        .command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let source = if m.editor_env.is_some() {
        " (ORION_EDITOR)"
    } else {
        ""
    };
    let fix_for = |editor: &str| {
        m.tools().editor_plan(editor).map(|plan| {
            if plan.runnable() {
                plan.line
            } else {
                plan.link.to_string()
            }
        })
    };
    match (&resolved.missing, installed(&program)) {
        (None, true) => Check::new(
            "File editor",
            Status::Ok,
            format!("{}{source}", resolved.command),
        )
        .required(),
        (Some(missing), _) => {
            let mut check = Check::new(
                "File editor",
                Status::Missing,
                format!(
                    "{missing}{source} isn't installed — files open in {} instead",
                    resolved.command
                ),
            );
            if let Some(fix) = fix_for(missing) {
                check = check.fix(fix);
            }
            check
        }
        (None, false) => {
            let mut check = Check::new(
                "File editor",
                Status::Missing,
                format!(
                    "{program}{source} isn't installed, and none of fresh, micro, edit or vim \
                     is either — no file can open"
                ),
            )
            .required();
            if let Some(fix) = fix_for(&program).or_else(|| fix_for(crate::editor::DEFAULT_EDITOR))
            {
                check = check.fix(fix);
            }
            check
        }
    }
}

/// **Open in app**: the app `⌘O` hands a file to, as this machine
/// resolves the setting.
fn open_in_app(cfg: &Config, m: &Machine) -> Check {
    use crate::outside_editor::{resolve, Choice, Places};
    let places = Places {
        roots: m.app_roots.clone(),
        path: Some(m.path.clone()),
    };
    let choice = Choice::parse(&cfg.outside_editor);
    match resolve(choice, &places) {
        Ok(target) => Check::new(
            "Open in app",
            Status::Ok,
            match choice {
                Choice::Auto => format!("auto → {}", target.name),
                _ => target.name.to_string(),
            },
        ),
        Err(why) => Check::new("Open in app", Status::Missing, why).fix(
            m.tools()
                .app_plan(choice.as_str())
                .map(plan_fix)
                .unwrap_or_else(|| "set Open in app to auto in Settings → Tools".into()),
        ),
    }
}

/// Orion.app, on a Mac: the Ghostty it is made from installed, and the app
/// made — the one place orion's ⌘ chords reach it.
fn orion_app(m: &Machine) -> Check {
    const NAME: &str = "Orion.app";
    if !m.macos {
        return Check::new(NAME, Status::Skipped, "macOS only");
    }
    if crate::event_loop::ghostty_app_in(&m.app_roots).is_none() {
        return Check::new(
            NAME,
            Status::Missing,
            "Ghostty isn't installed, and the app is made from it — ⌘ shortcuts answer to \
             their ^ twins, and the outside terminal opens in Terminal.app",
        )
        .fix(plan_fix(m.tools().ghostty_plan()));
    }
    match m
        .home
        .as_deref()
        .map(crate::app_bundle::bundle_in)
        .filter(|bundle| bundle.is_dir())
    {
        Some(bundle) => Check::new(
            NAME,
            Status::Ok,
            format!("at {}", crate::skills::tilde(&bundle, m.home.as_deref())),
        ),
        None => Check::new(
            NAME,
            Status::Missing,
            "not made yet — orion's ⌘ shortcuts work in it, and answer to their ^ twins in a \
             terminal",
        )
        .fix("orion app"),
    }
}

/// The CLI of every agent turned on.
fn agents(cfg: &Config, m: &Machine) -> Vec<Check> {
    let on: Vec<_> = cfg
        .raw_harness_registry()
        .into_iter()
        .filter(|entry| entry.enabled)
        .collect();
    if on.is_empty() {
        return vec![Check::new(
            "Agents",
            Status::Skipped,
            "none turned on — Settings → Agents",
        )];
    }
    let tools = m.tools();
    on.into_iter()
        .map(|entry| {
            let name = format!("Agent {}", entry.id);
            let program = entry.program.trim().to_string();
            match m.find(&program) {
                Some(path) => Check::new(
                    name,
                    Status::Ok,
                    crate::skills::tilde(&path, m.home.as_deref()),
                ),
                None => {
                    let check =
                        Check::new(name, Status::Missing, format!("`{program}` isn't on PATH"));
                    match tools.agent_plan(&program) {
                        Some(plan) if plan.runnable() => check.fix(plan.line),
                        Some(plan) => check.fix(plan.link),
                        None => check,
                    }
                }
            }
        })
        .collect()
}

/// The `LINEAR_API_KEY` of the project the current directory is in —
/// where it was found, never the key.
fn linear(m: &Machine) -> Check {
    let root = m
        .find("git")
        .and_then(|git| probe(m, &git, &["rev-parse", "--show-toplevel"], &m.cwd))
        .filter(|(ok, root)| *ok && !root.is_empty())
        .map_or_else(|| m.cwd.clone(), |(_, root)| PathBuf::from(root));
    let project = root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    match crate::linear::key_source(&root) {
        Some(source) => Check::new(
            "Linear",
            Status::Ok,
            format!("{project}'s LINEAR_API_KEY {}", source.label()),
        ),
        None => Check::new(
            "Linear",
            Status::Skipped,
            format!(
                "no LINEAR_API_KEY in {project}'s .env.local or .env — only the Linear view needs \
                 one"
            ),
        ),
    }
}

/// docker compose projects whose every container was started in a
/// directory that is gone — a worktree deleted while **Worktree
/// containers** was off, or outside orion — with the commands that tear
/// them down. Never required: only checkouts that run compose have any.
fn containers(cfg: &Config, m: &Machine) -> Check {
    use orion_core::compose;
    const NAME: &str = "Containers";
    let policy = compose::WorktreeContainers::parse(&cfg.worktree_containers);
    let setting = format!("Worktree containers: {}", policy.as_str());
    let Some(docker) = m.find("docker") else {
        return Check::new(NAME, Status::Skipped, "no docker CLI on PATH");
    };
    let listed = match probe_output(m, &docker, &compose::ps_args(), &m.cwd) {
        Some((true, out)) => out,
        _ => {
            return Check::new(
                NAME,
                Status::Skipped,
                "docker isn't answering — is OrbStack or Docker running?",
            )
        }
    };
    let orphans = compose::orphaned(&compose::parse_ps(&listed), Path::exists);
    if orphans.is_empty() {
        return Check::new(
            NAME,
            Status::Ok,
            format!("no compose project outlives its checkout ({setting})"),
        );
    }
    let names: Vec<&str> = orphans.iter().map(|(p, _)| p.as_str()).collect();
    let fix = names
        .iter()
        .map(|p| format!("docker compose -p {p} down --volumes"))
        .collect::<Vec<_>>()
        .join("; ");
    let hint = if policy == compose::WorktreeContainers::Off {
        " — Settings → General → Worktree containers cleans up on delete"
    } else {
        ""
    };
    Check::new(
        NAME,
        Status::Missing,
        format!(
            "{} compose project{} outlive{} a deleted checkout: {}{hint}",
            names.len(),
            if names.len() == 1 { "" } else { "s" },
            if names.len() == 1 { "s" } else { "" },
            names.join(", ")
        ),
    )
    .fix(fix)
}

/// The report as `orion doctor` prints it: a line a check, its fix under
/// it, then a word on the whole.
pub fn render(checks: &[Check]) -> String {
    let width = checks
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for check in checks {
        out.push_str(&format!(
            "  {} {:<width$}  {}\n",
            check.status.mark(),
            check.name,
            check.detail
        ));
        if let Some(fix) = &check.fix {
            out.push_str(&format!("    {:<width$}  fix: {fix}\n", ""));
        }
    }
    let failed: Vec<&str> = checks
        .iter()
        .filter(|c| c.fails())
        .map(|c| c.name.as_str())
        .collect();
    out.push('\n');
    if failed.is_empty() {
        out.push_str("Everything orion needs is here.\n");
    } else {
        out.push_str(&format!("orion needs: {}.\n", failed.join(", ")));
    }
    out
}

/// The report as JSON: `{"ok": …, "checks": [{name, status, detail, fix,
/// required}, …]}`.
pub fn render_json(checks: &[Check]) -> String {
    let ok = !checks.iter().any(Check::fails);
    serde_json::to_string_pretty(&serde_json::json!({ "ok": ok, "checks": checks }))
        .unwrap_or_default()
}

/// `orion doctor [--json]`: print the report for this machine and the
/// config here. True when nothing required is missing.
pub fn run(json: bool) -> bool {
    let cfg = Config::load();
    let checks = checks(&cfg, &Machine::here());
    if json {
        println!("{}", render_json(&checks));
    } else {
        print!("{}", render(&checks));
    }
    !checks.iter().any(Check::fails)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine under a temp dir: a `bin` of stub programs as its whole
    /// PATH, a home, an Applications root, and a project folder.
    struct Stubs {
        dir: tempfile::TempDir,
    }

    impl Stubs {
        fn new() -> Self {
            let stubs = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            for sub in ["bin", "home", "root/Applications", "project"] {
                std::fs::create_dir_all(stubs.dir.path().join(sub)).unwrap();
            }
            stubs
        }

        /// A stub `name` running `body`.
        fn program(&self, name: &str, body: &str) -> &Self {
            use std::os::unix::fs::PermissionsExt;
            let path = self.dir.path().join("bin").join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            self
        }

        fn machine(&self) -> Machine {
            let root = self.dir.path();
            Machine {
                path: std::env::join_paths([root.join("bin")]).unwrap(),
                home: Some(root.join("home")),
                app_roots: vec![root.join("root")],
                cwd: root.join("project"),
                macos: true,
                editor_env: None,
            }
        }
    }

    /// Claude on, everything else off — Grok Build included.
    fn claude_only(extra: &str) -> Config {
        let json = format!(
            r#"{{"claude_enabled": true, "codex_enabled": false, "cursor_enabled": false,
                "pi_enabled": false, "muse_enabled": false, "opencode_enabled": false,
                "harnesses": {{"grok": {{"enabled": false}}}}{extra}}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    /// Compose projects whose directory is gone are listed, with the
    /// command that removes each; a live one is not, and no docker or a
    /// docker that won't answer is nothing to check.
    #[test]
    fn containers_lists_compose_projects_whose_checkout_is_gone() {
        let stubs = Stubs::new();
        let live = stubs.dir.path().join("project");
        stubs.program(
            "docker",
            &format!(
                "printf 'gone-a\\t/nowhere/a\\ngone-a\\t/nowhere/a\\nlive\\t{}\\n\\t\\n'",
                live.display()
            ),
        );
        let check = containers(&Config::default(), &stubs.machine());
        assert_eq!(check.status, Status::Missing, "{check:#?}");
        assert!(
            check
                .detail
                .starts_with("1 compose project outlives a deleted checkout: gone-a — Settings"),
            "{}",
            check.detail
        );
        assert_eq!(
            check.fix.as_deref(),
            Some("docker compose -p gone-a down --volumes")
        );
        assert!(!check.fails(), "never required");

        let stubs = Stubs::new();
        stubs.program("docker", "printf 'live\\t/\\n'");
        let cfg: Config = serde_json::from_str(r#"{"worktree_containers": "remove"}"#).unwrap();
        let check = containers(&cfg, &stubs.machine());
        assert_eq!(check.status, Status::Ok);
        assert!(
            check.detail.ends_with("(Worktree containers: remove)"),
            "{}",
            check.detail
        );

        let stubs = Stubs::new();
        stubs.program("docker", "exit 1");
        assert_eq!(containers(&cfg, &stubs.machine()).status, Status::Skipped);
        assert_eq!(
            containers(&cfg, &Stubs::new().machine()).status,
            Status::Skipped
        );
    }

    fn named<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no {name} in {checks:#?}"))
    }

    /// Everything there: every line ✓, nothing required missing.
    #[test]
    fn a_machine_with_everything_passes() {
        let stubs = Stubs::new();
        stubs
            .program("git", r#"echo "git version 9.9.9""#)
            .program("gh", "exit 0")
            .program("fresh", "exit 0")
            .program("claude", "exit 0");
        let checks = checks(&claude_only(""), &stubs.machine());
        assert_eq!(named(&checks, "git").detail, "git version 9.9.9");
        assert_eq!(named(&checks, "gh").status, Status::Ok);
        assert_eq!(named(&checks, "File editor").detail, "fresh");
        assert_eq!(named(&checks, "Agent claude").status, Status::Ok);
        assert!(!checks.iter().any(Check::fails));
        let report = render(&checks);
        assert!(report.contains("✓ git"), "{report}");
        assert!(
            report.ends_with("Everything orion needs is here.\n"),
            "{report}"
        );
    }

    /// No git: required, the fix named, the exit code non-zero.
    #[test]
    fn missing_git_is_the_one_failure() {
        let stubs = Stubs::new();
        stubs.program("fresh", "exit 0");
        let checks = checks(&claude_only(""), &stubs.machine());
        let git = named(&checks, "git");
        assert_eq!(git.status, Status::Missing);
        assert_eq!(git.fix.as_deref(), Some("xcode-select --install"));
        assert!(git.fails());
        assert!(render(&checks).contains("orion needs: git."));
        let json: serde_json::Value = serde_json::from_str(&render_json(&checks)).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["checks"][0]["status"], "missing");
    }

    /// gh there but signed out: ✗ with `gh auth login`; gh missing: its
    /// install, by Homebrew when there is one. Neither is required.
    #[test]
    fn gh_is_checked_for_its_sign_in() {
        let stubs = Stubs::new();
        stubs.program("git", "echo git").program("gh", "exit 1");
        let check = gh(&stubs.machine());
        assert_eq!(check.detail, "installed, not signed in");
        assert_eq!(check.fix.as_deref(), Some("gh auth login"));
        assert!(!check.fails());

        let stubs = Stubs::new();
        stubs.program("brew", "exit 0");
        assert_eq!(gh(&stubs.machine()).fix.as_deref(), Some("brew install gh"));
        let stubs = Stubs::new();
        assert_eq!(
            gh(&stubs.machine()).fix.as_deref(),
            Some("https://cli.github.com")
        );
    }

    /// The chosen editor missing: what opens instead, and the formula
    /// that installs it. Nothing at all to open files in: required.
    #[test]
    fn the_editor_says_what_really_opens() {
        let stubs = Stubs::new();
        stubs.program("vim", "exit 0").program("brew", "exit 0");
        let check = editor(&claude_only(r#", "editor": "micro""#), &stubs.machine());
        assert_eq!(check.status, Status::Missing);
        assert_eq!(
            check.detail,
            "micro isn't installed — files open in vim instead"
        );
        assert_eq!(check.fix.as_deref(), Some("brew install micro"));
        assert!(!check.fails(), "vim opens them");

        let stubs = Stubs::new();
        let check = editor(&claude_only(r#", "editor": "hx""#), &stubs.machine());
        assert!(check.fails(), "{check:?}");
        assert_eq!(
            check.fix.as_deref(),
            Some("https://docs.helix-editor.com/install.html"),
            "no Homebrew: the editor's page"
        );

        let stubs = Stubs::new();
        stubs.program("nvim", "exit 0");
        let mut m = stubs.machine();
        m.editor_env = Some("nvim".into());
        assert_eq!(editor(&claude_only(""), &m).detail, "nvim (ORION_EDITOR)");
    }

    /// Every agent turned on, by its CLI — a missing one with its maker's
    /// installer — and none at all said so.
    #[test]
    fn each_agent_on_is_checked_for_its_cli() {
        let stubs = Stubs::new();
        stubs.program("claude", "exit 0").program("npm", "exit 0");
        let mut cfg = claude_only("");
        cfg.codex_enabled = true;
        let all = agents(&cfg, &stubs.machine());
        assert_eq!(all.len(), 2, "{all:#?}");
        assert_eq!(named(&all, "Agent claude").status, Status::Ok);
        let codex = named(&all, "Agent codex");
        assert_eq!(codex.status, Status::Missing);
        assert_eq!(codex.fix.as_deref(), Some("npm install -g @openai/codex"));

        let none: Config = serde_json::from_str(
            r#"{"claude_enabled": false, "codex_enabled": false, "cursor_enabled": false,
                "pi_enabled": false, "muse_enabled": false, "opencode_enabled": false,
                "harnesses": {"grok": {"enabled": false}}}"#,
        )
        .unwrap();
        let all = agents(&none, &stubs.machine());
        assert_eq!(all[0].status, Status::Skipped);
    }

    /// Orion.app: ✗ without the Ghostty it is made from, ✗ until it is made,
    /// ✓ once it is in ~/Applications — each ✗ with the way to fix it.
    #[test]
    fn orion_app_and_the_ghostty_it_is_made_from() {
        let stubs = Stubs::new();
        let mut m = stubs.machine();
        let check = orion_app(&m);
        assert_eq!(check.status, Status::Missing);
        assert!(
            check.detail.starts_with("Ghostty isn't installed"),
            "{check:?}"
        );

        std::fs::create_dir_all(stubs.dir.path().join("root/Applications/Ghostty.app")).unwrap();
        let check = orion_app(&m);
        assert_eq!(check.status, Status::Missing);
        assert!(check.detail.starts_with("not made yet"), "{check:?}");
        assert_eq!(check.fix.as_deref(), Some("orion app"));

        let home = m.home.clone().expect("the stub machine has a home");
        std::fs::create_dir_all(crate::app_bundle::bundle_in(&home)).unwrap();
        let check = orion_app(&m);
        assert_eq!(check.status, Status::Ok);
        assert!(
            check.detail.ends_with("Applications/Orion.app"),
            "{check:?}"
        );

        m.macos = false;
        assert_eq!(orion_app(&m).status, Status::Skipped);
    }

    /// The project's key: where it was found, never what it is.
    #[test]
    fn the_linear_key_is_found_and_never_shown() {
        let stubs = Stubs::new();
        let m = stubs.machine();
        std::fs::write(
            m.cwd.join(".env.local"),
            "LINEAR_API_KEY=lin_api_never_print_me\n",
        )
        .unwrap();
        let check = linear(&m);
        assert_eq!(check.status, Status::Ok);
        assert!(check.detail.contains("found in .env.local"), "{check:?}");
        let all = vec![check];
        assert!(!render(&all).contains("never_print_me"));
        assert!(!render_json(&all).contains("never_print_me"));
    }
}
