//! OPEN IN APP: the GUI editor `⌘O` hands a file to — and the OPEN MENU a
//! checkout — outside orion. Settings → General **Open in app**
//! (`outside_editor`) picks Cursor, VS Code, Sublime Text, Zed or the
//! system default; `auto`, the default, is the first of the four that is
//! installed, else the system default.
//!
//! An app is reached by the command-line tool inside its own bundle first —
//! `Cursor.app/…/bin/cursor`, `Visual Studio Code.app/…/bin/code`,
//! `Sublime Text.app/…/bin/subl`, `Zed.app/…/cli`, in `/Applications` or
//! `~/Applications` — which ships with the app whether or not its "install
//! shell command" was ever run, and which goes to a line. Then the tool on
//! PATH, past Cursor's AGENT CLI shim: `~/.local/bin/cursor` from the
//! `cursor-agent` installer is a shell script that only forwards to some
//! other `cursor` on PATH and otherwise fails with "No Cursor IDE
//! installation found", so taking it for the editor is how ⌘O used to do
//! nothing at all. Last, `open -a <App>`, which opens the file but can't
//! go to its line.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How much of a CLI script is read to tell the agent shim from the app's own.
const SHIM_SNIFF_BYTES: u64 = 16 * 1024;

/// The apps **Open in app** names, in the order `auto` tries them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    Cursor,
    VsCode,
    Sublime,
    Zed,
}

/// How an app's tool is told the file's line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Goto {
    /// `<root> --goto <file>:<line>`: the checkout's window — reused when
    /// it is already open — with the file at the line (Cursor, VS Code).
    Flag,
    /// `<root> <file>:<line>` (Sublime Text, Zed).
    Suffix,
}

struct Spec {
    app: App,
    /// The setting's word for it.
    word: &'static str,
    name: &'static str,
    bundle: &'static str,
    /// The tool inside the bundle.
    bundle_cli: &'static str,
    /// The tool's name on PATH.
    path_cli: &'static str,
    goto: Goto,
}

const APPS: &[Spec] = &[
    Spec {
        app: App::Cursor,
        word: "cursor",
        name: "Cursor",
        bundle: "Cursor.app",
        bundle_cli: "Contents/Resources/app/bin/cursor",
        path_cli: "cursor",
        goto: Goto::Flag,
    },
    Spec {
        app: App::VsCode,
        word: "vscode",
        name: "VS Code",
        bundle: "Visual Studio Code.app",
        bundle_cli: "Contents/Resources/app/bin/code",
        path_cli: "code",
        goto: Goto::Flag,
    },
    Spec {
        app: App::Sublime,
        word: "sublime",
        name: "Sublime Text",
        bundle: "Sublime Text.app",
        bundle_cli: "Contents/SharedSupport/bin/subl",
        path_cli: "subl",
        goto: Goto::Suffix,
    },
    Spec {
        app: App::Zed,
        word: "zed",
        name: "Zed",
        bundle: "Zed.app",
        bundle_cli: "Contents/MacOS/cli",
        path_cli: "zed",
        goto: Goto::Suffix,
    },
];

fn spec(app: App) -> &'static Spec {
    APPS.iter()
        .find(|spec| spec.app == app)
        .expect("every app has a spec")
}

/// The **Open in app** setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    /// The first of Cursor, VS Code, Sublime Text and Zed installed, else
    /// the system default.
    #[default]
    Auto,
    App(App),
    /// `open <file>` (`xdg-open` off macOS): whatever the system opens
    /// that kind of file with.
    System,
}

/// The setting's word for [`Choice::Auto`].
pub const AUTO: &str = "auto";

/// The setting's word for [`Choice::System`].
pub const SYSTEM: &str = "default";

/// The **Open in app** row's choices, in the order it cycles them.
pub const CHOICES: &[&str] = &[AUTO, "cursor", "vscode", "sublime", "zed", SYSTEM];

impl Choice {
    /// A stored word back to its choice; anything unknown is `auto`.
    pub fn parse(word: &str) -> Self {
        let word = word.trim();
        if word.eq_ignore_ascii_case(SYSTEM) {
            return Choice::System;
        }
        APPS.iter()
            .find(|spec| spec.word.eq_ignore_ascii_case(word))
            .map_or(Choice::Auto, |spec| Choice::App(spec.app))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Choice::Auto => AUTO,
            Choice::App(app) => spec(app).word,
            Choice::System => SYSTEM,
        }
    }
}

/// The name a hint or a flash calls the system default by.
pub const SYSTEM_NAME: &str = "default app";

/// How a resolved app is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    /// The app's own command-line tool: goes to the line.
    Cli(PathBuf),
    /// `open -a <bundle>`: the file, at its top.
    Bundle(PathBuf),
    /// `open` / `xdg-open`.
    System,
}

/// An installed app to hand a file to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// What hints and flashes call it: `VS Code`, `default app`.
    pub name: &'static str,
    pub launcher: Launcher,
    goto: Goto,
}

/// Where apps are looked for: the folders holding an `Applications` folder
/// (`/` and the home directory, on macOS only) and the PATH.
#[derive(Debug, Clone, Default)]
pub struct Places {
    pub roots: Vec<PathBuf>,
    pub path: Option<OsString>,
}

impl Places {
    /// This machine's.
    pub fn here() -> Self {
        let mut roots = Vec::new();
        if cfg!(target_os = "macos") {
            roots.push(PathBuf::from("/"));
            roots.extend(orion_core::env::home_dir());
        }
        Self {
            roots,
            path: std::env::var_os("PATH"),
        }
    }
}

/// `app` as installed under `places`: its bundle's tool, a tool on PATH
/// (never the agent shim), or the bundle for `open -a`. None when it isn't
/// installed at all.
pub fn find(app: App, places: &Places) -> Option<Launcher> {
    let spec = spec(app);
    let bundles: Vec<PathBuf> = places
        .roots
        .iter()
        .map(|root| root.join("Applications").join(spec.bundle))
        .collect();
    if let Some(cli) = bundles
        .iter()
        .map(|bundle| bundle.join(spec.bundle_cli))
        .find(|cli| cli.is_file())
    {
        return Some(Launcher::Cli(cli));
    }
    if let Some(cli) = places.path.as_ref().and_then(|path| {
        std::env::split_paths(path)
            .map(|dir| dir.join(spec.path_cli))
            .find(|cli| cli.is_file() && !is_agent_shim(cli))
    }) {
        return Some(Launcher::Cli(cli));
    }
    bundles
        .into_iter()
        .find(|bundle| bundle.is_dir())
        .map(Launcher::Bundle)
}

/// Whether `cli` is Cursor's AGENT CLI shim rather than the editor's own
/// tool: a script out of the `cursor-agent` install that fails without
/// another `cursor` behind it. Told by what it says, not where it is —
/// the editor's real `cursor` is a script too.
pub fn is_agent_shim(cli: &Path) -> bool {
    use std::io::Read;
    let resolved = std::fs::canonicalize(cli).unwrap_or_else(|_| cli.to_path_buf());
    if resolved.to_string_lossy().contains("cursor-agent") {
        return true;
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&resolved)
        .and_then(|file| file.take(SHIM_SNIFF_BYTES).read_to_end(&mut bytes));
    if read.is_err() {
        return false;
    }
    let head = String::from_utf8_lossy(&bytes);
    head.starts_with("#!")
        && (head.contains("No Cursor IDE installation") || head.contains(".local/bin/agent"))
}

/// The setting resolved to an installed app, or why there is none: `auto`
/// with nothing installed is the system default, a named app that isn't
/// installed is the reason.
pub fn resolve(choice: Choice, places: &Places) -> Result<Target, String> {
    let target = |spec: &Spec, launcher| Target {
        name: spec.name,
        launcher,
        goto: spec.goto,
    };
    match choice {
        Choice::Auto => Ok(APPS
            .iter()
            .find_map(|spec| find(spec.app, places).map(|launcher| target(spec, launcher)))
            .unwrap_or_else(system)),
        Choice::App(app) => {
            let spec = spec(app);
            find(app, places)
                .map(|launcher| target(spec, launcher))
                .ok_or_else(|| format!("{} isn't installed (Settings → Open in app)", spec.name))
        }
        Choice::System => Ok(system()),
    }
}

fn system() -> Target {
    Target {
        name: SYSTEM_NAME,
        launcher: Launcher::System,
        goto: Goto::Suffix,
    }
}

/// The **Open in app** setting as this machine resolves it.
pub fn configured() -> Result<Target, String> {
    resolve(
        Choice::parse(&crate::config::Config::load().outside_editor),
        &Places::here(),
    )
}

/// [`hint_name`]'s last answer and when it was worked out.
static HINT_NAME: std::sync::Mutex<Option<(std::time::Instant, &'static str)>> =
    std::sync::Mutex::new(None);

/// What a hint calls the configured app: its name, or the one the setting
/// asked for when that isn't installed (pressing it then says so). Hints
/// are drawn every frame, so the answer is kept for a few seconds rather
/// than the config and the Applications folders read each time — or until
/// the setting changes ([`forget_hint_name`]).
pub fn hint_name() -> &'static str {
    const KEEP: Duration = Duration::from_secs(5);
    let mut kept = HINT_NAME.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, name)) = *kept {
        if at.elapsed() < KEEP {
            return name;
        }
    }
    let choice = Choice::parse(&crate::config::Config::load().outside_editor);
    let name = match resolve(choice, &Places::here()) {
        Ok(target) => target.name,
        Err(_) => match choice {
            Choice::App(app) => spec(app).name,
            _ => SYSTEM_NAME,
        },
    };
    *kept = Some((std::time::Instant::now(), name));
    name
}

/// The **Open in app** row changed: the hints name the new app on the
/// next frame rather than up to five seconds on.
pub fn forget_hint_name() {
    *HINT_NAME.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// What the settings row shows: the choice, `auto` with the app it lands
/// on, and a named app that isn't installed saying so.
pub fn value_label(stored: &str, places: &Places) -> String {
    let choice = Choice::parse(stored);
    match (choice, resolve(choice, places)) {
        (Choice::Auto, Ok(target)) => format!("auto · {}", target.name),
        (Choice::System, _) => "system default".into(),
        (Choice::App(app), Ok(_)) => spec(app).name.into(),
        (Choice::App(app), Err(_)) => format!("{} — not installed", spec(app).name),
        (Choice::Auto, Err(why)) => why,
    }
}

/// The apps installed under `places`, by name, in `auto`'s order.
pub fn installed_names(places: &Places) -> Vec<&'static str> {
    APPS.iter()
        .filter(|spec| find(spec.app, places).is_some())
        .map(|spec| spec.name)
        .collect()
}

impl Target {
    /// The program and arguments that open `file` — relative to `root`, or
    /// absolute — at `line`: the app's tool with the checkout as its window
    /// where it has one, else the file alone.
    pub fn file_command(&self, root: &Path, file: &str, line: u64) -> (PathBuf, Vec<String>) {
        let full = root.join(file).to_string_lossy().into_owned();
        let root = root.to_string_lossy().into_owned();
        match &self.launcher {
            Launcher::Cli(cli) => {
                let at = format!("{full}:{line}");
                let args = match self.goto {
                    Goto::Flag => vec![root, "--goto".into(), at],
                    Goto::Suffix => vec![root, at],
                };
                (cli.clone(), args)
            }
            Launcher::Bundle(bundle) => (
                PathBuf::from("open"),
                vec!["-a".into(), bundle.to_string_lossy().into_owned(), full],
            ),
            Launcher::System => (system_opener(), vec![full]),
        }
    }

    /// The program and arguments that open the directory `dir` as a
    /// window of its own.
    pub fn dir_command(&self, dir: &Path) -> (PathBuf, Vec<String>) {
        let dir = dir.to_string_lossy().into_owned();
        match &self.launcher {
            Launcher::Cli(cli) => (cli.clone(), vec![dir]),
            Launcher::Bundle(bundle) => (
                PathBuf::from("open"),
                vec!["-a".into(), bundle.to_string_lossy().into_owned(), dir],
            ),
            Launcher::System => (system_opener(), vec![dir]),
        }
    }
}

fn system_opener() -> PathBuf {
    PathBuf::from(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
}

/// How long [`launch`] waits for a tool to fail before taking it as
/// started: the editors' tools hand the file to the running app and exit
/// well inside it, and a refusal is quicker still.
const LAUNCH_GRACE: Duration = Duration::from_millis(1500);

/// Run `program args` and say whether the app took it: `Err` with the
/// tool's own complaint (its last line on stderr) when it can't be
/// started or fails inside [`LAUNCH_GRACE`]. One still running by then is
/// taken as started and reaped in the background.
pub fn launch(program: &Path, args: &[String]) -> Result<(), String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    if cfg!(test) {
        return Ok(());
    }
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run {}: {e}", program.display()))?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                let said = stderr.lines().map(str::trim).rfind(|l| !l.is_empty());
                return Err(match said {
                    Some(line) => line.to_string(),
                    None => format!("{} exited with {status}", program.display()),
                });
            }
            Ok(None) if started.elapsed() < LAUNCH_GRACE => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    let what = program.display().to_string();
    std::thread::spawn(move || {
        let mut sink = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut sink);
        }
        match child.wait() {
            Ok(status) if !status.success() => tracing::warn!(what, %status, "open in app failed"),
            Err(err) => tracing::warn!(what, %err, "open in app not reaped"),
            Ok(_) => {}
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A machine under a temp dir: `/` and a home, each able to hold an
    /// Applications folder, and a PATH of `bin` dirs.
    struct Machine {
        _dir: tempfile::TempDir,
        system: PathBuf,
        home: PathBuf,
        bin: PathBuf,
    }

    impl Machine {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let system = dir.path().join("system");
            let home = dir.path().join("home");
            let bin = dir.path().join("bin");
            for d in [&system, &home, &bin] {
                std::fs::create_dir_all(d).unwrap();
            }
            Self {
                _dir: dir,
                system,
                home,
                bin,
            }
        }

        fn places(&self) -> Places {
            Places {
                roots: vec![self.system.clone(), self.home.clone()],
                path: Some(std::env::join_paths([&self.bin]).unwrap()),
            }
        }

        fn bundle(&self, root: &Path, app: App) -> PathBuf {
            let bundle = root.join("Applications").join(spec(app).bundle);
            std::fs::create_dir_all(&bundle).unwrap();
            bundle
        }

        fn bundle_cli(&self, root: &Path, app: App) -> PathBuf {
            let cli = self.bundle(root, app).join(spec(app).bundle_cli);
            write(&cli, "#!/usr/bin/env bash\n");
            cli
        }
    }

    const SHIM: &str = "#!/bin/sh\nset -eu\n\nOTHER_CURSOR=$(find_cursor || true)\n\
        if [ -n \"${OTHER_CURSOR:-}\" ]; then\n  exec \"$OTHER_CURSOR\" \"$@\"\nelse\n  \
        echo \"Error: No Cursor IDE installation found.\" 1>&2\n  exit 1\nfi\n";

    #[test]
    fn the_setting_reads_back_and_unknown_words_are_auto() {
        for word in CHOICES {
            assert_eq!(Choice::parse(word).as_str(), *word);
        }
        assert_eq!(Choice::parse(" VSCode "), Choice::App(App::VsCode));
        assert_eq!(Choice::parse("textmate"), Choice::Auto);
        assert_eq!(Choice::parse(""), Choice::Auto);
    }

    /// The tool inside the bundle wins over one on PATH, in either
    /// Applications folder.
    #[test]
    fn the_bundles_own_tool_is_preferred() {
        let m = Machine::new();
        write(&m.bin.join("code"), "#!/bin/sh\n");
        let cli = m.bundle_cli(&m.home, App::VsCode);
        assert_eq!(find(App::VsCode, &m.places()), Some(Launcher::Cli(cli)));
        let system_cli = m.bundle_cli(&m.system, App::VsCode);
        assert_eq!(
            find(App::VsCode, &m.places()),
            Some(Launcher::Cli(system_cli)),
            "/Applications first"
        );
    }

    /// Cursor's AGENT CLI shim on PATH is never taken for the editor: with
    /// Cursor.app installed its bundle tool is used, without it the bundle
    /// alone (`open -a`), and with neither Cursor isn't installed.
    #[test]
    fn the_cursor_agent_shim_is_skipped() {
        let m = Machine::new();
        let shim = m.bin.join("cursor");
        write(&shim, SHIM);
        assert!(is_agent_shim(&shim));
        assert_eq!(
            find(App::Cursor, &m.places()),
            None,
            "the shim alone is no Cursor"
        );

        let bundle = m.bundle(&m.system, App::Cursor);
        assert_eq!(
            find(App::Cursor, &m.places()),
            Some(Launcher::Bundle(bundle.clone())),
            "open -a rather than the shim"
        );
        let cli = m.bundle_cli(&m.system, App::Cursor);
        assert!(!is_agent_shim(&cli), "the editor's own script is no shim");
        assert_eq!(find(App::Cursor, &m.places()), Some(Launcher::Cli(cli)));

        // A real `cursor` on PATH (the editor's "install shell command").
        let m = Machine::new();
        write(
            &m.bin.join("cursor"),
            "#!/usr/bin/env bash\nELECTRON=\"$CONTENTS/MacOS/Cursor\"\n",
        );
        assert_eq!(
            find(App::Cursor, &m.places()),
            Some(Launcher::Cli(m.bin.join("cursor")))
        );
    }

    /// No tool anywhere, only the bundle: `open -a`, which can't go to a
    /// line; nothing at all: not installed.
    #[test]
    fn a_bundle_without_a_tool_opens_with_open_a() {
        let m = Machine::new();
        assert_eq!(find(App::Sublime, &m.places()), None);
        let bundle = m.bundle(&m.home, App::Sublime);
        assert_eq!(
            find(App::Sublime, &m.places()),
            Some(Launcher::Bundle(bundle.clone()))
        );
        let target = resolve(Choice::App(App::Sublime), &m.places()).unwrap();
        let (program, args) = target.file_command(Path::new("/repo"), "src/a.rs", 7);
        assert_eq!(program, PathBuf::from("open"));
        assert_eq!(args, ["-a", &bundle.to_string_lossy(), "/repo/src/a.rs"]);
    }

    /// `auto` takes the first installed of Cursor → VS Code → Sublime →
    /// Zed, else the system default; a named app that isn't installed is
    /// a reason, never a quiet swap.
    #[test]
    fn auto_takes_the_first_installed_and_a_missing_choice_says_so() {
        let m = Machine::new();
        assert_eq!(
            resolve(Choice::Auto, &m.places()).unwrap().name,
            SYSTEM_NAME
        );
        m.bundle_cli(&m.system, App::Zed);
        assert_eq!(resolve(Choice::Auto, &m.places()).unwrap().name, "Zed");
        m.bundle_cli(&m.system, App::Sublime);
        assert_eq!(
            resolve(Choice::Auto, &m.places()).unwrap().name,
            "Sublime Text"
        );
        m.bundle_cli(&m.home, App::VsCode);
        assert_eq!(resolve(Choice::Auto, &m.places()).unwrap().name, "VS Code");
        write(&m.bin.join("cursor"), SHIM);
        assert_eq!(
            resolve(Choice::Auto, &m.places()).unwrap().name,
            "VS Code",
            "the shim doesn't make Cursor installed"
        );

        assert_eq!(
            resolve(Choice::App(App::Cursor), &m.places()),
            Err("Cursor isn't installed (Settings → Open in app)".into())
        );
        assert_eq!(
            resolve(Choice::System, &m.places()).unwrap().name,
            SYSTEM_NAME
        );
        assert_eq!(
            installed_names(&m.places()),
            ["VS Code", "Sublime Text", "Zed"]
        );
        assert_eq!(value_label("auto", &m.places()), "auto · VS Code");
        assert_eq!(value_label("cursor", &m.places()), "Cursor — not installed");
        assert_eq!(value_label("zed", &m.places()), "Zed");
        assert_eq!(value_label("default", &m.places()), "system default");
    }

    /// Each app is told the line its own way: `--goto` for Cursor and VS
    /// Code, a `:line` suffix for Sublime Text and Zed — all with the
    /// checkout as the window.
    #[test]
    fn each_app_goes_to_the_line_its_own_way() {
        let m = Machine::new();
        let code = m.bundle_cli(&m.system, App::VsCode);
        let subl = m.bundle_cli(&m.system, App::Sublime);
        let root = Path::new("/repo");
        let vscode = resolve(Choice::App(App::VsCode), &m.places()).unwrap();
        assert_eq!(
            vscode.file_command(root, "src/a.rs", 7),
            (
                code.clone(),
                vec!["/repo".into(), "--goto".into(), "/repo/src/a.rs:7".into()]
            )
        );
        assert_eq!(
            vscode.file_command(root, "/abs/b.md", 1).1,
            ["/repo", "--goto", "/abs/b.md:1"],
            "an absolute file stays absolute"
        );
        assert_eq!(vscode.dir_command(root), (code, vec!["/repo".into()]));
        let sublime = resolve(Choice::App(App::Sublime), &m.places()).unwrap();
        assert_eq!(
            sublime.file_command(root, "src/a.rs", 7),
            (subl, vec!["/repo".into(), "/repo/src/a.rs:7".into()])
        );
        let system = resolve(Choice::System, &m.places()).unwrap();
        let (program, args) = system.file_command(root, "src/a.rs", 7);
        assert!(program == Path::new("open") || program == Path::new("xdg-open"));
        assert_eq!(args, ["/repo/src/a.rs"], "no line to go to");
    }
}
