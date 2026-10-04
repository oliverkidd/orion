//! INSTALLS from inside orion: an editor the BUILT-IN EDITOR runs, or an
//! agent's CLI, put on this machine by its own installer — Homebrew for an
//! editor (`brew install micro`), the vendor's documented command for an
//! agent (`curl -fsSL https://claude.ai/install.sh | bash`).
//!
//! The onboarding wizard's Agents and Editor pages and the SETTINGS
//! OVERLAY's **File editor**, CLAUDE ACCOUNTS and harness rows share it:
//! `i` on a program that isn't on PATH shows its [`Plan`] — the line it
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
    /// The program on PATH once it is in: `micro`, `claude`.
    pub program: String,
    /// What runs, as the user reads it before it does: `brew install
    /// micro`. Empty when there is nothing orion can run — see `link`.
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

/// The editors orion can install, by the program the **File editor** row
/// names: its Homebrew formula and the editor's own install page.
const EDITORS: &[(&str, &str, &str)] = &[
    // The `fresh-editor` formula installs `fresh`.
    (
        "fresh",
        "fresh-editor",
        "https://github.com/sinelaw/fresh#installation",
    ),
    (
        "micro",
        "micro",
        "https://github.com/zyedidia/micro#installation",
    ),
    // Microsoft Edit: the `msedit` formula installs `edit`.
    ("edit", "msedit", "https://github.com/microsoft/edit"),
    (
        "nvim",
        "neovim",
        "https://github.com/neovim/neovim/blob/master/INSTALL.md",
    ),
    ("hx", "helix", "https://docs.helix-editor.com/install.html"),
    (
        "emacs",
        "emacs",
        "https://www.gnu.org/software/emacs/download.html",
    ),
];

/// The Homebrew formula that installs `editor` (any spelling the **File
/// editor** row takes: `/opt/homebrew/bin/micro`, `msedit`, `helix`).
#[cfg(test)]
pub fn editor_formula(editor: &str) -> Option<&'static str> {
    editor_spec(editor).map(|(_, formula, _)| formula)
}

fn editor_spec(editor: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let program = match crate::editor::program_name(editor) {
        "msedit" => "edit",
        "helix" => "hx",
        other => other,
    };
    EDITORS
        .iter()
        .copied()
        .find(|(name, _, _)| *name == program)
}

/// How `editor` gets onto this machine: `brew install <formula>` with
/// Homebrew at `brew`, else its install page. None for an editor orion has
/// no formula for (vim, which macOS ships, or a hand-typed command).
pub fn editor_plan(editor: &str, brew: Option<&Path>) -> Option<Plan> {
    let (program, formula, link) = editor_spec(editor)?;
    Some(match brew {
        Some(brew) => Plan {
            program: program.into(),
            line: format!("brew install {formula}"),
            command: Some(Command {
                program: brew.display().to_string(),
                args: vec!["install".into(), formula.into()],
            }),
            link,
        },
        None => Plan {
            program: program.into(),
            line: String::new(),
            command: None,
            link,
        },
    })
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
        "pi",
        "curl -fsSL https://pi.dev/install.sh | sh",
        "https://pi.dev",
    ),
    (
        "muse",
        "curl -fsSL https://dev.meta.ai/install.sh | bash",
        "https://dev.meta.ai",
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

/// Where Homebrew installs itself, by platform, for a `brew` a PATH
/// without its shellenv doesn't name.
const BREW_PREFIXES: &[&str] = &[
    "/opt/homebrew/bin/brew",
    "/usr/local/bin/brew",
    "/home/linuxbrew/.linuxbrew/bin/brew",
];

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
}

/// The plan behind a SETTINGS OVERLAY row whose program isn't on PATH —
/// the **File editor** row's editor, a CLAUDE ACCOUNT's or a harness's
/// CLI on its Enabled row. None on any other row, or when the program is
/// there.
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
    if spec.kind != SettingKind::Editor {
        return None;
    }
    let missing = cfg.editor_resolved().missing?;
    tools.editor_plan(&missing)
}

/// Run `plan` in the editor modal, over whatever overlay is up. False,
/// with the footer saying why, when nothing was spawned — a plan with
/// nothing to run, or a PTY that would not open. Under test nothing ever
/// is: the command is recorded for the test to read instead.
pub fn run(app: &mut App, plan: &Plan) -> bool {
    let Some(command) = plan.command.clone() else {
        return false;
    };
    #[cfg(test)]
    {
        let _ = &app;
        RAN.with(|ran| ran.borrow_mut().push(command));
        true
    }
    #[cfg(not(test))]
    {
        let Some(tx) = app.vim_tx.clone() else {
            return false;
        };
        let (cols, rows) = crate::event_loop::vim_size_guess(app);
        app.vim_generation += 1;
        let home = orion_core::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        match crate::vim_term::VimTerm::spawn_cmd_env(
            &command.program,
            &command.args,
            &[],
            &home,
            plan.title(),
            cols,
            rows,
            app.vim_generation,
            tx,
        ) {
            Ok(mut term) => {
                term.install = Some(plan.program.clone());
                app.vim = Some(term);
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
    let Some(vim) = &mut app.vim else {
        return;
    };
    let Some(program) = vim.install.clone() else {
        return;
    };
    let installed = crate::config::program_installed(&program);
    vim.finished = Some(installed);
    vim.title = format!(
        "{} — {}",
        vim.title,
        if installed {
            "installed"
        } else {
            "not on PATH"
        }
    );
    app.dirty = true;
}

/// The install modal closed: say how it went where the user is looking —
/// the onboarding page, the settings overlay, else (only when it is still
/// not on PATH: the modal's title already said it went in) the footer —
/// and let the editor fallback be noted afresh.
pub fn closed(app: &mut App, program: &str) {
    let installed = crate::config::program_installed(program);
    let note = outcome(program, installed);
    app.editor_fallback_noted = None;
    match &mut app.overlay {
        Some(crate::app::Overlay::Onboard(view)) => view.note = Some(note),
        Some(crate::app::Overlay::Settings(view)) => view.info(note),
        // The footer leads with its own `✕`, so not the outcome's `✗`.
        _ if !installed => {
            let words = note.trim_start_matches("✗ ").to_string();
            app.flash = Some(crate::flash::Flash::failed(words));
        }
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

    /// Every editor the **File editor** row cycles through but vim has a
    /// formula, under each spelling the row takes; the formula is what
    /// `brew install` runs, with the brew this machine has.
    #[test]
    fn each_editor_installs_by_its_formula() {
        for (editor, formula) in [
            ("micro", "micro"),
            ("/opt/homebrew/bin/micro -autosu true", "micro"),
            ("edit", "msedit"),
            ("msedit", "msedit"),
            ("fresh", "fresh-editor"),
            ("nvim", "neovim"),
            ("hx", "helix"),
            ("helix", "helix"),
            ("emacs", "emacs"),
        ] {
            assert_eq!(editor_formula(editor), Some(formula), "{editor}");
        }
        assert_eq!(editor_formula("vim"), None, "macOS ships it");
        assert_eq!(editor_formula("code --wait"), None);
        for editor in crate::editor::EDITORS.iter().filter(|e| **e != "vim") {
            assert!(editor_formula(editor).is_some(), "{editor} has a formula");
        }

        let brew = Path::new("/opt/homebrew/bin/brew");
        let plan = editor_plan("edit", Some(brew)).unwrap();
        assert_eq!(plan.program, "edit", "the formula installs `edit`");
        assert_eq!(plan.line, "brew install msedit");
        assert_eq!(
            plan.command,
            Some(Command {
                program: "/opt/homebrew/bin/brew".into(),
                args: vec!["install".into(), "msedit".into()],
            })
        );
        assert!(plan.question().contains("`brew install msedit`"));
        assert_eq!(plan.title(), "Install edit · brew install msedit");
    }

    /// No Homebrew: the editor's own page, and nothing to run.
    #[test]
    fn without_homebrew_an_editor_is_its_install_page() {
        let plan = editor_plan("micro", None).unwrap();
        assert!(!plan.runnable());
        assert_eq!(plan.line, "");
        assert_eq!(plan.link, "https://github.com/zyedidia/micro#installation");
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
            ("pi", "curl -fsSL https://pi.dev/install.sh | sh"),
            ("muse", "curl -fsSL https://dev.meta.ai/install.sh | bash"),
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
        let plan = editor_plan("micro", Some(Path::new("/b/brew"))).unwrap();
        assert!(run(&mut app, &plan));
        assert_eq!(
            take_ran(),
            [Command {
                program: "/b/brew".into(),
                args: vec!["install".into(), "micro".into()],
            }]
        );
        assert!(!run(&mut app, &editor_plan("micro", None).unwrap()));
        assert!(take_ran().is_empty());
    }
}
