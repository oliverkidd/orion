//! INSTALLS from inside orion: fresh, the BUILT-IN EDITOR, an agent's CLI,
//! or Ghostty, put on this machine by its own installer — Homebrew for
//! fresh (`brew install fresh-editor`) or Ghostty (`brew install --cask
//! ghostty`), the vendor's documented command for an agent (`curl -fsSL
//! https://claude.ai/install.sh | bash`).
//!
//! Setup's Claude page (the Claude CLI), the SETTINGS
//! OVERLAY's CLAUDE ACCOUNTS and harness rows, and the footer's word that
//! fresh is missing share it: `i` on a program that isn't on PATH shows its [`Plan`] — the line it
//! would run — and Enter runs that line in the editor modal's PTY, the
//! machinery a Claude sign-in uses (`claude_accounts::run`), so the
//! installer's own output, and any question it asks, are on screen. The
//! modal stays up when the installer exits, its title saying whether the
//! program is on PATH now, until Enter closes it ([`exited`], [`closed`]).
//! The rows read PATH as they draw, so they say `installed` at once.
//!
//! Without Homebrew an editor's plan is its install page alone: nothing is
//! run. `orion doctor` and `install.sh` name the same commands. Under test
//! nothing is ever run — [`run`] records the command for the test to read
//! ([`take_ran`]).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::app::App;

/// One install: the program it puts on PATH, the line it runs, and where
/// its maker documents it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The program on PATH once it is in: `fresh`, `claude`.
    pub program: String,
    /// What runs, as the user reads it before it does: `brew install
    /// fresh-editor`. Empty when there is nothing orion can run — see
    /// `link`.
    pub line: String,
    /// How the modal runs [`Plan::line`]; None when orion has nothing to
    /// run, and the plan is only its [`Plan::link`].
    pub command: Option<Command>,
    /// The official install instructions.
    pub link: &'static str,
}

/// A program and its arguments, as the modal spawns them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

impl Plan {
    /// Whether orion can run it here, rather than only point at its page.
    pub fn runnable(&self) -> bool {
        self.command.is_some()
    }

    /// The question a row asks before it runs: what Enter will do, or —
    /// with nothing to run — where to go instead.
    pub fn question(&self) -> String {
        if self.runnable() {
            format!(
                "Install {} — Enter runs `{}` here, in a terminal",
                self.program, self.line
            )
        } else {
            format!(
                "Homebrew isn't installed, so orion can't install {} itself — its install page: {}",
                self.program, self.link
            )
        }
    }

    /// The modal's title while it runs.
    pub fn title(&self) -> String {
        format!("Install {} · {}", self.program, self.line)
    }
}

/// fresh's Homebrew formula — `fresh-editor` installs `fresh` — and its
/// own install page.
const FRESH_FORMULA: &str = "fresh-editor";
const FRESH_LINK: &str = "https://github.com/sinelaw/fresh#installation";

/// How `editor` gets onto this machine: `brew install fresh-editor` with
/// Homebrew at `brew`, else fresh's install page. None for any other
/// command `ORION_EDITOR` names.
pub fn editor_plan(editor: &str, brew: Option<&Path>) -> Option<Plan> {
    (crate::editor::Kind::of(editor) == crate::editor::Kind::Fresh).then(|| {
        brew_plan(
            crate::editor::DEFAULT_EDITOR,
            FRESH_FORMULA,
            false,
            FRESH_LINK,
            brew,
        )
    })
}

/// Ghostty's download page, which documents the Homebrew cask too.
pub(crate) const GHOSTTY_LINK: &str = "https://ghostty.org/download";

/// The program [`ghostty_plan`] installs.
pub const GHOSTTY: &str = "ghostty";

/// Whether `program` is here: on PATH — or Ghostty's app in an
/// Applications folder, which is what ⇧T opens, whatever PATH holds (a
/// cask install leaves `ghostty` off a PATH without Homebrew's bin). A
/// test goes by its PATH alone.
pub fn installed(program: &str) -> bool {
    if let Some(app) = outside_app(program) {
        return !cfg!(test)
            && crate::outside_editor::find(app, &crate::outside_editor::Places::here()).is_some();
    }
    crate::config::program_installed(program)
        || (program == GHOSTTY && !cfg!(test) && crate::event_loop::ghostty_app().is_some())
}

/// `brew install [--cask] <name>` with Homebrew at `brew`, else `link`
/// alone, nothing run.
fn brew_plan(
    program: &str,
    name: &str,
    cask: bool,
    link: &'static str,
    brew: Option<&Path>,
) -> Plan {
    let Some(brew) = brew else {
        return Plan {
            program: program.into(),
            line: String::new(),
            command: None,
            link,
        };
    };
    let mut args = vec!["install".to_string()];
    if cask {
        args.push("--cask".into());
    }
    args.push(name.into());
    Plan {
        program: program.into(),
        line: format!("brew {}", args.join(" ")),
        command: Some(Command {
            program: brew.display().to_string(),
            args,
        }),
        link,
    }
}

/// The command-line tools the Tools tab lists besides the editors: git,
/// which orion can't work without, and gh, for pull requests and issues —
/// each one's Homebrew formula and install page.
pub const CLIS: &[(&str, &str, &str)] = &[
    ("git", "git", "https://git-scm.com/downloads"),
    ("gh", "gh", "https://cli.github.com"),
];

/// How `program`, one of [`CLIS`], gets onto this machine.
pub fn cli_plan(program: &str, brew: Option<&Path>) -> Option<Plan> {
    let (_, formula, link) = CLIS.iter().find(|(name, _, _)| *name == program)?;
    Some(brew_plan(program, formula, false, link, brew))
}

/// The **Open in app** editors orion can install on a Mac: the word the
/// setting stores, Homebrew's cask for it, and its download page.
const APPS: &[(&str, &str, &str)] = &[
    ("cursor", "cursor", "https://cursor.com/download"),
    (
        "vscode",
        "visual-studio-code",
        "https://code.visualstudio.com/download",
    ),
    (
        "sublime",
        "sublime-text",
        "https://www.sublimetext.com/download",
    ),
];

/// The **Open in app** app `program` names, when it names one of [`APPS`].
fn outside_app(program: &str) -> Option<crate::outside_editor::App> {
    APPS.iter().find(|(word, _, _)| *word == program)?;
    match crate::outside_editor::Choice::parse(program) {
        crate::outside_editor::Choice::App(app) => Some(app),
        _ => None,
    }
}

/// How the **Open in app** choice `word` gets onto a Mac: its cask. None
/// for `auto`.
pub fn app_plan(word: &str, brew: Option<&Path>) -> Option<Plan> {
    let (word, cask, link) = APPS.iter().find(|(name, _, _)| *name == word)?;
    Some(brew_plan(word, cask, true, link, brew))
}

/// How Ghostty — Orion.app's base, and the terminal ⌘O opens in the
/// checkout — gets onto a Mac: `brew
/// install --cask ghostty` with Homebrew at `brew`, else its download page.
/// The cask puts `ghostty` on PATH as well as the app in /Applications.
pub fn ghostty_plan(brew: Option<&Path>) -> Plan {
    brew_plan(GHOSTTY, "ghostty", true, GHOSTTY_LINK, brew)
}

/// The agent CLIs orion can install, by the program a harness runs: each
/// one's install command, word for word from its maker's install page,
/// and that page. Codex's goes by what this machine has ([`agent_plan`]).
const AGENTS: &[(&str, &str, &str)] = &[
    (
        "claude",
        "curl -fsSL https://claude.ai/install.sh | bash",
        "https://code.claude.com/docs/en/setup",
    ),
    (
        "cursor-agent",
        "curl https://cursor.com/install -fsS | bash",
        "https://cursor.com/docs/cli/installation",
    ),
    // The same installer puts Cursor's CLI on PATH as `agent` too.
    (
        "agent",
        "curl https://cursor.com/install -fsS | bash",
        "https://cursor.com/docs/cli/installation",
    ),
    (
        "grok",
        "curl -fsSL https://x.ai/cli/install.sh | bash",
        "https://docs.x.ai/build/overview",
    ),
    (
        "opencode",
        "curl -fsSL https://opencode.ai/install | bash",
        "https://opencode.ai/docs/",
    ),
];

/// Codex's install page, which documents both of its commands.
const CODEX_LINK: &str = "https://github.com/openai/codex";

/// How the CLI `program` gets onto this machine: its maker's documented
/// command, run through `/bin/sh -c` because most are `curl … | bash`.
/// Codex documents Homebrew (`brew install --cask codex`) and npm (`npm
/// install -g @openai/codex`): whichever this machine has, Homebrew first,
/// else its page. None for a CLI orion knows no installer for — a custom
/// harness, or a built-in repointed at another program.
pub fn agent_plan(program: &str, brew: Option<&Path>, npm: Option<&Path>) -> Option<Plan> {
    let program = crate::editor::program_name(program);
    if program == "codex" {
        let (line, command) = match (brew, npm) {
            (Some(brew), _) => (
                "brew install --cask codex",
                Command {
                    program: brew.display().to_string(),
                    args: vec!["install".into(), "--cask".into(), "codex".into()],
                },
            ),
            (None, Some(npm)) => (
                "npm install -g @openai/codex",
                Command {
                    program: npm.display().to_string(),
                    args: vec!["install".into(), "-g".into(), "@openai/codex".into()],
                },
            ),
            (None, None) => {
                return Some(Plan {
                    program: program.into(),
                    line: String::new(),
                    command: None,
                    link: CODEX_LINK,
                })
            }
        };
        return Some(Plan {
            program: program.into(),
            line: line.into(),
            command: Some(command),
            link: CODEX_LINK,
        });
    }
    let (_, line, link) = AGENTS.iter().find(|(name, _, _)| *name == program)?;
    Some(Plan {
        program: program.into(),
        line: (*line).into(),
        command: Some(Command {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), (*line).into()],
        }),
        link,
    })
}

/// Where Homebrew installs itself — Apple silicon, then Intel — for a
/// `brew` a PATH without its shellenv doesn't name.
const BREW_PREFIXES: &[&str] = &["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];

/// The first `program` in the directories `path` lists — a PATH lookup.
pub fn which(path: &OsStr, program: &str) -> Option<PathBuf> {
    let program = program.trim();
    if program.is_empty() {
        return None;
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// What this machine installs with: Homebrew — on PATH, or where it puts
/// itself — and npm.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tools {
    pub brew: Option<PathBuf>,
    pub npm: Option<PathBuf>,
}

impl Tools {
    /// This machine's, off the PATH programs are looked up on
    /// (`config::search_path`) — and, outside a test, where Homebrew puts
    /// itself.
    pub fn here() -> Self {
        Self::on(&crate::config::search_path(), !cfg!(test))
    }

    /// The tools on `path`; `prefixes` looks where Homebrew installs itself
    /// too, for a shell that never ran its shellenv.
    pub fn on(path: &OsStr, prefixes: bool) -> Self {
        let brew = which(path, "brew").or_else(|| {
            prefixes
                .then(|| {
                    BREW_PREFIXES
                        .iter()
                        .map(PathBuf::from)
                        .find(|brew| brew.is_file())
                })
                .flatten()
        });
        Self {
            brew,
            npm: which(path, "npm"),
        }
    }

    pub fn editor_plan(&self, editor: &str) -> Option<Plan> {
        editor_plan(editor, self.brew.as_deref())
    }

    pub fn agent_plan(&self, program: &str) -> Option<Plan> {
        agent_plan(program, self.brew.as_deref(), self.npm.as_deref())
    }

    pub fn cli_plan(&self, program: &str) -> Option<Plan> {
        cli_plan(program, self.brew.as_deref())
    }

    pub fn app_plan(&self, word: &str) -> Option<Plan> {
        app_plan(word, self.brew.as_deref())
    }

    pub fn ghostty_plan(&self) -> Plan {
        ghostty_plan(self.brew.as_deref())
    }
}

/// The plan behind a SETTINGS OVERLAY row whose program isn't on PATH —
/// a CLAUDE ACCOUNT's or a harness's CLI on its Enabled row. None on any
/// other row, or when the program is there.
pub fn settings_row_plan(
    cfg: &crate::config::Config,
    tab: usize,
    index: usize,
    tools: &Tools,
) -> Option<Plan> {
    use crate::config::{AccountRow, HarnessField, SettingKind};
    if tab == crate::config::agents_tab() {
        let id = match (cfg.account_row(index), cfg.agent_row(index)) {
            (Some(AccountRow::Account(id)), _) => id,
            (None, Some((id, HarnessField::Enabled))) => id,
            _ => return None,
        };
        let program = cfg.effective_harness_by_id(&id).program;
        return (!crate::config::program_installed(&program))
            .then(|| tools.agent_plan(&program))
            .flatten();
    }
    let spec = crate::config::setting_at(tab, index)?;
    match spec.kind {
        SettingKind::OutsideEditor => {
            let word = cfg.outside_editor.trim();
            (!installed(word)).then(|| tools.app_plan(word)).flatten()
        }
        SettingKind::Git => (!installed("git")).then(|| tools.cli_plan("git")).flatten(),
        SettingKind::Gh => (!installed("gh")).then(|| tools.cli_plan("gh")).flatten(),
        _ => None,
    }
}

/// Run `plan` in the editor modal, over whatever overlay is up. False,
/// with the footer saying why, when nothing was spawned — a plan with
/// nothing to run, or a PTY that would not open. Under test nothing ever
/// is: the command is recorded for the test to read instead.
pub fn run(app: &mut App, plan: &Plan) -> bool {
    let Some(command) = plan.command.clone() else {
        return false;
    };
    app.flash = Some(crate::flash::Flash::working(format!(
        "installing {} — {}…",
        plan.program, plan.line
    )));
    #[cfg(test)]
    {
        RAN.with(|ran| ran.borrow_mut().push(command));
        true
    }
    #[cfg(not(test))]
    {
        let Some(tx) = app.editor_tx.clone() else {
            return false;
        };
        let (cols, rows) = crate::event_loop::editor_size_guess(app);
        app.editor_generation += 1;
        let home = orion_core::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        match crate::editor_term::EditorTerm::spawn_cmd_env(
            &command.program,
            &command.args,
            &[],
            &home,
            plan.title(),
            cols,
            rows,
            app.editor_generation,
            tx,
        ) {
            Ok(mut term) => {
                term.install = Some(plan.program.clone());
                app.editor = Some(term);
                app.dirty = true;
                true
            }
            Err(msg) => {
                app.flash = Some(crate::flash::Flash::failed(msg));
                false
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static RAN: std::cell::RefCell<Vec<Command>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The installs [`run`] was asked for on this test's thread, taken.
#[cfg(test)]
pub fn take_ran() -> Vec<Command> {
    RAN.with(|ran| std::mem::take(&mut *ran.borrow_mut()))
}

/// What an install's end says about `program`: on PATH now, or still not
/// — an installer that put it somewhere new and told the shell's profile
/// leaves orion's own PATH as it was.
pub fn outcome(program: &str, installed: bool) -> String {
    if installed {
        format!("✓ {program} is installed")
    } else if program == GHOSTTY {
        // An app, not a CLI: missing means the install itself failed.
        format!("✗ {program} didn't install — the installer's output said why; i tries again")
    } else {
        format!(
            "✗ {program} still isn't on orion's PATH — if its installer added it to your shell \
             profile, start orion again from a new shell"
        )
    }
}

/// The installer in the modal exited: the modal stays, its title saying
/// how it went, until Enter closes it ([`closed`]).
pub fn exited(app: &mut App) {
    let Some(vim) = &mut app.editor else {
        return;
    };
    let Some(program) = vim.install.clone() else {
        return;
    };
    let installed = installed(&program);
    vim.finished = Some(installed);
    vim.title = format!(
        "{} — {}",
        vim.title,
        match (installed, program.as_str()) {
            (true, _) => "installed",
            (false, GHOSTTY) => "failed",
            (false, _) => "not on PATH",
        }
    );
    app.flash = Some(outcome_flash(&program, installed, " — Enter closes"));
    app.dirty = true;
}

/// [`outcome`] as a footer line, `then` after it. The footer leads with
/// its own mark, so not the outcome's.
fn outcome_flash(program: &str, installed: bool, then: &str) -> crate::flash::Flash {
    let note = outcome(program, installed);
    let words = format!("{}{then}", note.trim_start_matches(['✓', '✗', ' ']));
    if installed {
        crate::flash::Flash::done(words)
    } else {
        crate::flash::Flash::failed(words)
    }
}

/// The install modal closed: say how it went in the footer, in place of
/// the `installing…` [`run`] put there, and where the user is looking —
/// the onboarding page, the settings overlay.
pub fn closed(app: &mut App, program: &str) {
    let installed = installed(program);
    let note = outcome(program, installed);
    app.flash = Some(outcome_flash(program, installed, ""));
    match &mut app.overlay {
        Some(crate::app::Overlay::Onboard(view)) => view.note = Some(note),
        Some(crate::app::Overlay::Settings(view)) => view.info(note),
        _ => {}
    }
    app.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        path
    }

    /// An install's end, as the page and the footer say it: Ghostty is an
    /// app, so missing after its installer means the install failed.
    #[test]
    fn an_install_says_how_it_went() {
        assert_eq!(outcome("micro", true), "✓ micro is installed");
        assert!(outcome("micro", false).contains("isn't on orion's PATH"));
        assert!(outcome(GHOSTTY, false).starts_with("✗ ghostty didn't install"));
        let done = outcome_flash("micro", true, " — Enter closes");
        assert_eq!(done.kind, crate::flash::FlashKind::Done);
        assert_eq!(done.text, "micro is installed — Enter closes");
        let failed = outcome_flash(GHOSTTY, false, "");
        assert_eq!(failed.kind, crate::flash::FlashKind::Failed);
        assert!(failed.text.starts_with("ghostty didn't install"));
    }

    /// Ghostty: the cask with Homebrew, its download page without.
    #[test]
    fn ghostty_installs_by_its_cask() {
        let brew = Path::new("/opt/homebrew/bin/brew");
        let plan = ghostty_plan(Some(brew));
        assert_eq!(plan.line, "brew install --cask ghostty");
        assert_eq!(
            plan.command.unwrap().args,
            vec!["install", "--cask", "ghostty"]
        );
        let page = ghostty_plan(None);
        assert!(!page.runnable());
        assert_eq!(page.link, "https://ghostty.org/download");
    }

    /// The Tools tab's command-line rows and Open in app apps install by
    /// Homebrew, or name their page without it.
    #[test]
    fn tools_rows_install_by_homebrew() {
        let brew = Path::new("/opt/homebrew/bin/brew");
        assert_eq!(cli_plan("gh", Some(brew)).unwrap().line, "brew install gh");
        assert_eq!(
            cli_plan("git", None).unwrap().link,
            "https://git-scm.com/downloads"
        );
        assert!(cli_plan("hg", Some(brew)).is_none());
        let plan = app_plan("vscode", Some(brew)).unwrap();
        assert_eq!(plan.line, "brew install --cask visual-studio-code");
        assert!(app_plan("auto", Some(brew)).is_none());
    }

    /// fresh installs by its formula — `fresh-editor` puts `fresh` on
    /// PATH — with the brew this machine has; any other command has no
    /// plan.
    #[test]
    fn fresh_installs_by_its_formula() {
        let brew = Path::new("/opt/homebrew/bin/brew");
        let plan = editor_plan("fresh", Some(brew)).unwrap();
        assert_eq!(plan.program, "fresh", "the formula installs `fresh`");
        assert_eq!(plan.line, "brew install fresh-editor");
        assert_eq!(
            plan.command,
            Some(Command {
                program: "/opt/homebrew/bin/brew".into(),
                args: vec!["install".into(), "fresh-editor".into()],
            })
        );
        assert!(plan.question().contains("`brew install fresh-editor`"));
        assert_eq!(plan.title(), "Install fresh · brew install fresh-editor");
        assert!(editor_plan("/opt/homebrew/bin/fresh", Some(brew)).is_some());
        assert_eq!(editor_plan("vim", Some(brew)), None);
        assert_eq!(editor_plan("code --wait", Some(brew)), None);
    }

    /// No Homebrew: fresh's own page, and nothing to run.
    #[test]
    fn without_homebrew_fresh_is_its_install_page() {
        let plan = editor_plan("fresh", None).unwrap();
        assert!(!plan.runnable());
        assert_eq!(plan.line, "");
        assert_eq!(plan.link, "https://github.com/sinelaw/fresh#installation");
        assert!(plan.question().contains(plan.link), "{}", plan.question());
    }

    /// Each agent's command is its maker's, run through `sh -c` for the
    /// pipe; Codex's follows what the machine has.
    #[test]
    fn each_agent_installs_by_its_documented_command() {
        for (program, line) in [
            ("claude", "curl -fsSL https://claude.ai/install.sh | bash"),
            (
                "cursor-agent",
                "curl https://cursor.com/install -fsS | bash",
            ),
            ("grok", "curl -fsSL https://x.ai/cli/install.sh | bash"),
            ("opencode", "curl -fsSL https://opencode.ai/install | bash"),
        ] {
            let plan = agent_plan(program, None, None).unwrap();
            assert_eq!(plan.line, line, "{program}");
            assert_eq!(
                plan.command,
                Some(Command {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), line.into()],
                }),
                "{program}"
            );
        }
        let brew = Path::new("/usr/local/bin/brew");
        let npm = Path::new("/usr/local/bin/npm");
        assert_eq!(
            agent_plan("codex", Some(brew), Some(npm)).unwrap().line,
            "brew install --cask codex"
        );
        let plan = agent_plan("codex", None, Some(npm)).unwrap();
        assert_eq!(plan.line, "npm install -g @openai/codex");
        assert_eq!(plan.command.unwrap().program, "/usr/local/bin/npm");
        assert!(!agent_plan("codex", None, None).unwrap().runnable());
        assert_eq!(agent_plan("my-wrapper", Some(brew), Some(npm)), None);
        assert_eq!(
            agent_plan("/Users/me/.local/bin/claude", None, None)
                .unwrap()
                .program,
            "claude",
            "a harness's program by any path"
        );
    }

    /// Every built-in harness's program has an installer.
    #[test]
    fn every_builtin_harness_has_an_installer() {
        for kind in orion_core::AgentKind::ALL {
            let Some(entry) = orion_core::harness::builtin(kind.as_str()) else {
                continue;
            };
            assert!(
                agent_plan(&entry.program, None, None).is_some(),
                "{}",
                entry.program
            );
        }
    }

    /// Homebrew and npm off a PATH: the first of each.
    #[test]
    fn the_tools_come_off_the_path() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let brew = stub(two.path(), "brew");
        let npm = stub(one.path(), "npm");
        let path = std::env::join_paths([one.path(), two.path()]).unwrap();
        assert_eq!(
            Tools::on(&path, false),
            Tools {
                brew: Some(brew),
                npm: Some(npm)
            }
        );
        let empty = tempfile::tempdir().unwrap();
        let none = std::env::join_paths([empty.path()]).unwrap();
        assert_eq!(Tools::on(&none, false), Tools::default());
    }

    /// The run is recorded under test — never spawned — and a plan with
    /// nothing to run starts nothing.
    #[test]
    fn a_run_is_recorded_and_never_spawned() {
        let mut app = App::new();
        let plan = editor_plan("fresh", Some(Path::new("/b/brew"))).unwrap();
        assert!(run(&mut app, &plan));
        assert_eq!(
            take_ran(),
            [Command {
                program: "/b/brew".into(),
                args: vec!["install".into(), "fresh-editor".into()],
            }]
        );
        assert!(!run(&mut app, &editor_plan("fresh", None).unwrap()));
        assert!(take_ran().is_empty());
    }
}
