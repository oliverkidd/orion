//! ORION.APP: orion as a Mac app of its own — its name and constellation
//! icon in the Dock, Launchpad, Spotlight and ⌘-Tab, one click to open it,
//! and a second click that brings the running window back rather than
//! opening another.
//!
//! orion is drawn in a terminal, so the app is one: a copy of the
//! Ghostty.app already on this Mac, re-badged — its own bundle id, name
//! and icon, an ad-hoc signature — and kept in `~/Applications`. Being Ghostty, it
//! reads the user's Ghostty config — their font and theme — and then its
//! own ([`app_config`]), which makes the window orion's and clears
//! Ghostty's key table down to a handful (`ghostty_config`), so every
//! other ⌘ chord reaches orion. Nothing is written into the user's config.
//!
//! Nothing of orion's moves into it. The app is a window: what it runs is
//! the installed `orion`, the sessions live in the daemon, and the
//! projects, settings and history stay in the data dir. An orion opened
//! here finds everything the one in a terminal left, and `orion` in a
//! terminal goes on working beside it.
//!
//! macOS starts an app as its bundle's executable with no arguments, and
//! Ghostty is told what to run by argument, so that executable is a
//! LAUNCHER: a copy of this binary which, finding itself in that seat
//! ([`run_if_launcher`]), execs the Ghostty beside it with [`host_args`]:
//! the bundle's config file and the command.
//! A shell script there did not launch. A click in the Dock also hands the
//! app launchd's bare environment, so the launcher runs orion through the
//! user's login shell, where the PATH their agents' CLIs are on is set.
//!
//! The bundle is built when orion is first opened in it ([`open`]) and
//! rebuilt only when its STAMP no longer matches: this layout's version,
//! the Ghostty it was copied from, and where `orion` is installed. A
//! rebuild never happens under a running app, except as orion quits
//! inside it ([`refresh_on_quit`]) — the app is about to close.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The icon the app wears.
const ICON: &[u8] = include_bytes!("../assets/orion.icns");
/// Its bundle id: what makes it an app of its own to macOS, and what a
/// process inside it finds in `__CFBundleIdentifier` ([`inside`]).
pub const BUNDLE_ID: &str = "dev.orion.app";
/// Its name in the Dock, the menu bar and ⌘-Tab.
const NAME: &str = "Orion";
/// Bump when the bundle's layout, [`host_args`] or the icon changes, so
/// an existing copy is rebuilt. Its config ([`app_config`]) is in the
/// STAMP by itself.
const LAYOUT: u32 = 1;
const STAMP_FILE: &str = "orion-stamp";
/// Where the bundle keeps the path of the installed `orion` it runs.
const EXE_FILE: &str = "orion-exe";
/// The bundle's own Ghostty config, among its resources.
const CONFIG_FILE: &str = "orion.conf";
/// The icon's name among the bundle's resources, as `Info.plist` names it.
const ICON_NAME: &str = "orion";
/// The bundle's executable: the LAUNCHER.
const LAUNCHER: &str = "orion";
/// Ghostty's own binary, beside the launcher.
const HOST: &str = "ghostty";

/// The `Info.plist` keys that make the copy orion's.
const REPLACED: &[(&str, &str)] = &[
    ("CFBundleIdentifier", BUNDLE_ID),
    ("CFBundleName", NAME),
    ("CFBundleDisplayName", NAME),
    ("CFBundleIconFile", ICON_NAME),
    ("CFBundleExecutable", LAUNCHER),
];

/// The keys taken out: Ghostty's icon in its asset catalogue, which wins
/// over `CFBundleIconFile`, and what would make macOS list a second
/// Ghostty — its Dock tile plugin, its Services, the file types it opens
/// and its Spotlight keywords.
const REMOVED: &[&str] = &[
    "CFBundleIconName",
    "NSDockTilePlugIn",
    "NSServices",
    "CFBundleDocumentTypes",
    "UTExportedTypeDeclarations",
    "MDItemKeywords",
];

/// How the app's window differs from a Ghostty window, whatever the
/// user's own config says. It is all orion: the background is the black
/// orion paints on ([`crate::theme::BLACK_BACKGROUND`]) and opaque, so the
/// title bar — which takes that colour — and the sliver left over at the
/// window's edge when it is not a whole number of cells both vanish into
/// it, the sliver shared out evenly; the title bar carries the window's
/// three buttons and no words; and the window opens filling the screen,
/// since a grid of agents wants the room. Ghostty's updater is off, since
/// an update would put Ghostty back where the app is; no windows are
/// restored on the next launch; closing the window never asks, since the
/// sessions live in the daemon; the app ends with its window; and it may
/// post the notifications orion asks it for (`event_loop::alerts`).
const WINDOW: &[(&str, &str)] = &[
    ("background", "#000000"),
    ("background-opacity", "1"),
    ("window-padding-x", "2"),
    ("window-padding-y", "2"),
    ("window-padding-balance", "true"),
    ("macos-titlebar-style", "transparent"),
    ("macos-titlebar-proxy-icon", "hidden"),
    ("macos-window-buttons", "visible"),
    ("title", "\" \""),
    ("maximize", "true"),
    ("auto-update", "off"),
    ("window-save-state", "never"),
    ("confirm-close-surface", "false"),
    ("quit-after-last-window-closed", "true"),
    ("desktop-notifications", "true"),
];

/// The app's own Ghostty config, read after the user's: [`WINDOW`], then
/// the key table (`ghostty_config::app_keybinds`).
pub fn app_config() -> String {
    let mut out = String::from(
        "# Orion.app's own Ghostty settings, read after yours. Written by orion when it\n\
         # builds the app, and replaced with it: change your Ghostty config, not this.\n",
    );
    for (key, value) in WINDOW {
        out.push_str(&format!("{key} = {value}\n"));
    }
    for bind in crate::ghostty_config::app_keybinds() {
        out.push_str(&format!("keybind = {bind}\n"));
    }
    out
}

/// The environment a Dock click gives an app, which [`open`] gives it too
/// so both start orion the same way. PATH is left to the login shell.
const LAUNCH_VARS: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "SSH_AUTH_SOCK",
    "__CF_USER_TEXT_ENCODING",
];
const LAUNCH_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The orion variables the app's orion keeps from the one that opened it:
/// the ones that point orion at another daemon, config or editor.
const KEPT_VARS: &[&str] = &[
    orion_core::env::RUNTIME_DIR,
    orion_core::env::DATA_DIR,
    orion_core::env::CONFIG_FILE,
    orion_core::env::EDITOR,
    orion_core::env::GHOSTTY_CONFIG,
    orion_core::env::LOG,
];

/// Whether orion is running inside the app right now. LaunchServices hands
/// its bundle id to every process an app starts.
pub fn inside() -> bool {
    orion_core::env::non_empty("__CFBundleIdentifier").as_deref() == Some(BUNDLE_ID)
}

/// Where the app lives: `~/Applications`, which Launchpad and Spotlight
/// read and which needs no admin rights to write.
pub fn bundle_path() -> Option<PathBuf> {
    Some(bundle_in(&orion_core::env::home_dir()?))
}

/// [`bundle_path`] for the home folder `home`.
pub fn bundle_in(home: &Path) -> PathBuf {
    home.join("Applications").join(format!("{NAME}.app"))
}

/// Whether to offer moving orion into its app — setup's App step: on a
/// local Mac with the Ghostty the app is made from, to an orion not
/// already in it, in tmux (a choice the app would undo) or on `orion
/// browser`'s page.
#[cfg(not(test))]
pub fn offered() -> bool {
    cfg!(target_os = "macos")
        && !orion_core::host::is_remote_session()
        && !inside()
        && std::env::var_os("TMUX").is_none()
        && std::env::var_os(orion_core::env::BROWSER).is_none()
        && crate::event_loop::ghostty_app().is_some()
}

/// [`offered`] under test: what [`with_offered`] set on this thread.
#[cfg(test)]
pub fn offered() -> bool {
    OFFERED.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    static OFFERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with [`offered`] answering `on` on this test's thread.
#[cfg(test)]
pub fn with_offered<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let was = OFFERED.with(|o| o.replace(on));
    let out = f();
    OFFERED.with(|o| o.set(was));
    out
}

/// Where `bundle` keeps its executables.
fn macos_dir(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("MacOS")
}

/// Where `bundle` keeps its resources.
fn resources_dir(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("Resources")
}

/// The Ghostty beside `exe` when `exe` is a bundle's LAUNCHER —
/// `<name>.app/Contents/MacOS/orion` with Ghostty's binary next to it.
fn host_beside(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let seated = exe.file_name()? == LAUNCHER
        && macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    let host = macos.join(HOST);
    (seated && host.is_file()).then_some(host)
}

/// The installed `orion` a bundle runs, as its `orion-exe` names it; a
/// bare `orion` for the login shell's PATH to find when that file is gone
/// or names nothing.
fn installed_orion(resources: &Path) -> PathBuf {
    std::fs::read_to_string(resources.join(EXE_FILE))
        .ok()
        .map(|text| PathBuf::from(text.trim()))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("orion"))
}

/// What the login shell runs: `orion`, its own folder put on the front of
/// PATH so the `orion` an agent's hook calls is found. Spelled so sh, zsh,
/// bash and fish all read it alike.
fn hosted_command(orion: &Path) -> String {
    let quoted = orion_core::shell::single_quote(&orion.to_string_lossy());
    match orion.parent().filter(|dir| dir.is_absolute()) {
        Some(dir) => format!(
            "exec /usr/bin/env PATH={}:\"$PATH\" {quoted}",
            orion_core::shell::single_quote(&dir.to_string_lossy())
        ),
        None => format!("exec {quoted}"),
    }
}

/// What the LAUNCHER hands Ghostty: the bundle's config file, whatever
/// the launcher was itself passed (`open --args`: the folder to start
/// in), then orion through `shell` as a login, interactive one — the
/// window's one command.
///
/// Every argument is a `--key=value`, the command included, rather than
/// Ghostty's `-e` and the words after it: a Mac app opens any argument
/// that reads as a file, and `-e /bin/zsh …` got the app a second tab
/// running `/bin/zsh; exit` beside orion's.
pub fn host_args(config: &Path, shell: &str, orion: &Path, passed: &[OsString]) -> Vec<OsString> {
    use orion_core::shell::single_quote;
    let mut config_file = OsString::from("--config-file=");
    config_file.push(config);
    let mut args = vec![config_file];
    args.extend(passed.iter().cloned());
    args.push(
        format!(
            "--initial-command=shell:{} -l -i -c {}",
            single_quote(shell),
            single_quote(&hosted_command(orion))
        )
        .into(),
    );
    args
}

/// The first thing `main` does: when this binary is a bundle's LAUNCHER,
/// become the Ghostty beside it, running the installed orion. Returns at
/// once anywhere else; in that seat it only returns by exiting.
pub fn run_if_launcher() {
    use std::os::unix::process::CommandExt as _;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(host) = host_beside(&exe) else {
        return;
    };
    // `host_beside` found it at `<bundle>/Contents/MacOS/ghostty`.
    let Some(resources) = host.ancestors().nth(3).map(resources_dir) else {
        return;
    };
    // macOS's default shell when launchd names none.
    let shell = orion_core::env::non_empty("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    let passed: Vec<OsString> = std::env::args_os().skip(1).collect();
    let err = Command::new(&host)
        .args(host_args(
            &resources.join(CONFIG_FILE),
            &shell,
            &installed_orion(&resources),
            &passed,
        ))
        .exec();
    eprintln!("orion: couldn't start {}: {err}", host.display());
    std::process::exit(1);
}

/// The STAMP of a bundle copied from `source` to run `orion`: this
/// layout, that Ghostty and its build, where `orion` is, and the config
/// the bundle carries — on one line, so a config that changes changes it.
fn stamp(source: &Path, orion: &Path) -> String {
    format!(
        "{LAYOUT}:{}:{}:{}:{:016x}",
        source.display(),
        source_version(source),
        orion.display(),
        crate::review::fingerprint(&app_config())
    )
}

/// `source`'s build number, which moves with every Ghostty update; empty
/// when it can't be read.
fn source_version(source: &Path) -> String {
    Command::new("plutil")
        .args(["-extract", "CFBundleVersion", "raw", "-o", "-"])
        .arg(source.join("Contents/Info.plist"))
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Whether the bundle at `target` is whole and carries `stamp`.
fn is_current(target: &Path, stamp: &str) -> bool {
    let macos = macos_dir(target);
    macos.join(LAUNCHER).is_file()
        && macos.join(HOST).is_file()
        && std::fs::read_to_string(resources_dir(target).join(STAMP_FILE))
            .ok()
            .as_deref()
            == Some(stamp)
}

/// Whether the app at `target` is running: a process whose command line
/// starts with its Ghostty's path — spelled for `pgrep`, which reads the
/// path as a pattern, with every character that means something in one
/// escaped.
fn running(target: &Path) -> bool {
    let host = macos_dir(target).join(HOST);
    let mut pattern = String::from("^");
    for c in host.to_string_lossy().chars() {
        if r".[]()*+?^$|\{}".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    // `-a`: an orion asking from inside the app is its descendant, and
    // pgrep leaves its own ancestors out unless told not to.
    run(Command::new("pgrep").args(["-a", "-f"]).arg(pattern)).is_ok()
}

/// Build the bundle at `target` from the Ghostty at `source`, to run
/// `orion`: beside `target` first, then moved over whatever was there, so
/// a build that fails leaves the old app as it was.
fn build(source: &Path, target: &Path, orion: &Path, stamp: &str) -> Result<(), String> {
    let building = target.with_extension("building");
    let _ = std::fs::remove_dir_all(&building);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let built = fill(source, &building, orion, stamp).and_then(|()| {
        let _ = std::fs::remove_dir_all(target);
        std::fs::rename(&building, target).map_err(|e| e.to_string())
    });
    if built.is_err() {
        let _ = std::fs::remove_dir_all(&building);
    }
    built?;
    register(target);
    Ok(())
}

/// [`build`]'s copy, re-badge and signature, at `building`.
fn fill(source: &Path, building: &Path, orion: &Path, stamp: &str) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    run(Command::new("ditto").arg(source).arg(building))?;
    // What macOS noted on Ghostty's download — its quarantine — is not
    // this copy's to carry. Best effort.
    let _ = run(Command::new("xattr").arg("-cr").arg(building));
    let plist = building.join("Contents").join("Info.plist");
    for (key, value) in REPLACED {
        run(Command::new("plutil")
            .args(["-replace", key, "-string", value])
            .arg(&plist))?;
    }
    for key in REMOVED {
        // A Ghostty without the key has nothing to take out.
        let _ = run(Command::new("plutil").args(["-remove", key]).arg(&plist));
    }
    let resources = resources_dir(building);
    // Ghostty's Shortcuts actions, which would be listed twice.
    let _ = std::fs::remove_dir_all(resources.join("Metadata.appintents"));
    std::fs::write(resources.join(format!("{ICON_NAME}.icns")), ICON).map_err(io)?;
    std::fs::write(resources.join(CONFIG_FILE), app_config()).map_err(io)?;
    std::fs::write(
        resources.join(EXE_FILE),
        orion.as_os_str().as_encoded_bytes(),
    )
    .map_err(io)?;
    // Before the signature, which seals every file in the bundle.
    std::fs::write(resources.join(STAMP_FILE), stamp).map_err(io)?;
    std::fs::copy(orion, macos_dir(building).join(LAUNCHER)).map_err(io)?;
    sign(source, building)
}

/// Sign `bundle` ad hoc — the edits broke Ghostty's own signature, and
/// macOS runs nothing unsigned on Apple silicon — with the entitlements
/// `source` carries: Ghostty's binary, then the bundle around it.
fn sign(source: &Path, bundle: &Path) -> Result<(), String> {
    let entitlements = bundle.with_extension("entitlements");
    let carried = run(Command::new("codesign")
        .args(["-d", "--xml", "--entitlements"])
        .arg(&entitlements)
        .arg(source))
    .is_ok()
        && std::fs::metadata(&entitlements).is_ok_and(|m| m.len() > 0);
    let sign = |path: &Path, deep: bool| {
        let mut cmd = Command::new("codesign");
        cmd.args(["--force", "--sign", "-"]);
        if deep {
            cmd.arg("--deep");
        }
        if carried {
            cmd.arg("--entitlements").arg(&entitlements);
        }
        run(cmd.arg(path))
    };
    let signed = sign(&macos_dir(bundle).join(HOST), false).and_then(|()| sign(bundle, true));
    let _ = std::fs::remove_file(&entitlements);
    signed
}

/// Tell LaunchServices about `bundle` now, so its name and icon are known
/// straight away rather than at the next scan. Best effort.
fn register(bundle: &Path) {
    let _ = run(Command::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
         LaunchServices.framework/Support/lsregister",
    )
    .arg("-f")
    .arg(bundle));
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The Ghostty to copy, the bundle's place and the installed `orion`.
fn parts() -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let source = crate::event_loop::ghostty_app().ok_or_else(|| {
        format!(
            "Ghostty isn't installed, and Orion.app is made from it — get it from {}, or run \
             orion's installer again",
            crate::install::GHOSTTY_LINK
        )
    })?;
    let target = bundle_path().ok_or("there is no home folder to keep the app in")?;
    let orion =
        std::env::current_exe().map_err(|e| format!("couldn't find orion's binary: {e}"))?;
    Ok((source, target, orion))
}

/// What the app is opened with: a Dock click's environment
/// ([`LAUNCH_VARS`]) and the orion variables set here ([`KEPT_VARS`]).
fn launch_env() -> Vec<(&'static str, OsString)> {
    let kept = LAUNCH_VARS
        .iter()
        .chain(KEPT_VARS)
        .filter_map(|name| Some((*name, std::env::var_os(name).filter(|v| !v.is_empty())?)));
    std::iter::once(("PATH", OsString::from(LAUNCH_PATH)))
        .chain(kept)
        .collect()
}

/// Open orion in its app, in the folder this one runs in: the bundle
/// built first when it is missing or stale. An app already running is
/// brought to the front as it is. Ghostty makes its first window when the
/// app first comes to the front, which `open` asks for and macOS withholds
/// from someone busy in another app: the window is then one click on the
/// Dock icon away. What went wrong, when something did.
pub fn open() -> Result<(), String> {
    let (source, target, orion) = parts()?;
    let stamp = stamp(&source, &orion);
    if !is_current(&target, &stamp) && !(target.is_dir() && running(&target)) {
        build(&source, &target, &orion, &stamp)?;
    }
    let mut working_dir = OsString::from("--working-directory=");
    working_dir.push(
        std::env::current_dir()
            .ok()
            .or_else(orion_core::env::home_dir)
            .unwrap_or_else(|| PathBuf::from("/")),
    );
    let status = Command::new("open")
        .arg(&target)
        .arg("--args")
        .arg(working_dir)
        .env_clear()
        .envs(launch_env())
        .status()
        .map_err(|e| format!("couldn't run open: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open {} failed ({status})", target.display()))
    }
}

/// As orion quits inside its app: rebuild a bundle whose STAMP has gone
/// stale — Ghostty was updated, `orion` moved, or a newer orion lays the
/// bundle out differently — so the next click opens the new one. The one
/// rebuild under a running app, which is closing with orion.
pub fn refresh_on_quit() {
    if !inside() {
        return;
    }
    let Ok((source, target, orion)) = parts() else {
        return;
    };
    let stamp = stamp(&source, &orion);
    if !target.is_dir() || is_current(&target, &stamp) {
        return;
    }
    eprintln!("orion: updating {}…", target.display());
    if let Err(why) = build(&source, &target, &orion, &stamp) {
        eprintln!("orion: couldn't update it: {why}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a bundle's own executable, with Ghostty's binary beside it, is
    /// a launcher: the installed orion never is, wherever it is kept.
    #[test]
    fn only_the_bundles_executable_is_a_launcher() {
        let tmp = tempfile::tempdir().unwrap();
        let macos = tmp.path().join("Orion.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let launcher = macos.join(LAUNCHER);
        std::fs::write(&launcher, "").unwrap();
        assert_eq!(host_beside(&launcher), None, "no Ghostty beside it");
        std::fs::write(macos.join(HOST), "").unwrap();
        assert_eq!(host_beside(&launcher), Some(macos.join(HOST)));
        assert_eq!(host_beside(&macos.join(HOST)), None, "Ghostty itself");

        let bin = tmp.path().join("bin/MacOS");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(HOST), "").unwrap();
        assert_eq!(host_beside(&bin.join(LAUNCHER)), None, "not in a bundle");
        assert_eq!(host_beside(Path::new("/Users/me/.local/bin/orion")), None);
    }

    /// Ghostty is started with the bundle's config, the folder `open`
    /// passed, then orion through a login shell with its own folder on
    /// PATH — no argument a bare word.
    #[test]
    fn the_launcher_runs_orion_through_the_login_shell() {
        let args = host_args(
            Path::new("/Users/me/Applications/Orion.app/Contents/Resources/orion.conf"),
            "/bin/zsh",
            Path::new("/Users/me/.local/bin/orion"),
            &["--working-directory=/Users/me/code/app".into()],
        );
        assert_eq!(
            args,
            [
                "--config-file=/Users/me/Applications/Orion.app/Contents/Resources/orion.conf",
                "--working-directory=/Users/me/code/app",
                concat!(
                    "--initial-command=shell:'/bin/zsh' -l -i -c ",
                    r#"'exec /usr/bin/env PATH='\''/Users/me/.local/bin'\'':"$PATH" "#,
                    r#"'\''/Users/me/.local/bin/orion'\'''"#,
                ),
            ]
            .map(OsString::from)
        );
        assert!(
            args.iter()
                .all(|arg| arg.to_string_lossy().starts_with("--")),
            "a bare word would be opened as a file"
        );
    }

    /// The app's config makes the window orion's and ends on the cleared
    /// key table; a stamp carries it, so a config that changes rebuilds
    /// the app.
    #[test]
    fn the_apps_config_sets_the_window_then_the_keys() {
        let config = app_config();
        for line in [
            "background = #000000",
            "macos-window-buttons = visible",
            "title = \" \"",
            "auto-update = off",
            "keybind = clear",
            "keybind = super+q=quit",
        ] {
            assert!(config.lines().any(|l| l == line), "{line}:\n{config}");
        }
        let window = config.find("quit-after-last-window-closed").unwrap();
        assert!(window < config.find("keybind = clear").unwrap());
        let stamp = stamp(
            Path::new("/Applications/Ghostty.app"),
            Path::new("/bin/orion"),
        );
        assert_eq!(stamp.split(':').count(), 5, "{stamp}");
    }

    /// A bundle whose `orion-exe` is gone, or names a file that is, runs
    /// whatever `orion` the login shell finds.
    #[test]
    fn a_bundle_without_its_orion_falls_back_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(installed_orion(tmp.path()), Path::new("orion"));
        std::fs::write(tmp.path().join(EXE_FILE), "/nowhere/orion").unwrap();
        assert_eq!(installed_orion(tmp.path()), Path::new("orion"));
        assert_eq!(hosted_command(Path::new("orion")), "exec 'orion'");

        let exe = tmp.path().join("orion");
        std::fs::write(&exe, "").unwrap();
        std::fs::write(tmp.path().join(EXE_FILE), format!("{}\n", exe.display())).unwrap();
        assert_eq!(installed_orion(tmp.path()), exe);
    }

    /// A bundle is current only when whole and stamped for this Ghostty
    /// and this orion.
    #[test]
    fn a_bundle_is_current_only_with_its_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("Orion.app");
        let contents = target.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::create_dir_all(contents.join("Resources")).unwrap();
        std::fs::write(contents.join("Resources").join(STAMP_FILE), "1:a::b").unwrap();
        assert!(!is_current(&target, "1:a::b"), "no launcher, no Ghostty");
        std::fs::write(contents.join("MacOS").join(LAUNCHER), "").unwrap();
        std::fs::write(contents.join("MacOS").join(HOST), "").unwrap();
        assert!(is_current(&target, "1:a::b"));
        assert!(!is_current(&target, "2:a::b"), "a newer layout");
    }

    /// The app is opened with a Dock click's PATH, not this terminal's.
    #[test]
    fn the_app_opens_with_a_dock_clicks_path() {
        let env = launch_env();
        assert_eq!(env[0], ("PATH", OsString::from(LAUNCH_PATH)));
        assert_eq!(env.iter().filter(|(name, _)| *name == "PATH").count(), 1);
    }
}
