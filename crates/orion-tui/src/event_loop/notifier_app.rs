//! The macOS NOTIFIER APP: a copy of Homebrew's `terminal-notifier.app`
//! re-badged as *orion* — its own bundle id, name and the constellation
//! icon (`assets/orion.icns`, drawn by `assets/orion-icon.swift`) — so a
//! desktop notification arrives under orion's name and logo instead of
//! Script Editor's, and clicking it brings the terminal back.
//!
//! Built on first use under `<data_dir>/notifier/orion.app` and rebuilt
//! only when its STAMP — this layout's version plus the source bundle's
//! path, which moves on every `brew upgrade` — no longer matches. With no
//! `terminal-notifier` installed there is nothing to copy, and
//! `alerts::notify_desktop` falls back to `osascript`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The icon the notifier app wears.
const ICON: &[u8] = include_bytes!("../../assets/orion.icns");
/// Its bundle id: the key macOS files its notification settings under.
const BUNDLE_ID: &str = "dev.orion.notifier";
/// Bump when the bundle layout or the icon changes, so an existing copy
/// is rebuilt.
const LAYOUT: u32 = 1;
const STAMP_FILE: &str = "orion-stamp";

/// The notifier app's executable, built on the first call in this process
/// and remembered (a failed build too — it is a debug line once, not one
/// per notification). Blocks while the build runs; call it off the UI
/// thread.
pub(super) fn executable() -> Option<&'static Path> {
    static EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE.get_or_init(|| {
        let source = source_bundle()?;
        let target = orion_core::paths::data_dir()
            .join("notifier")
            .join("orion.app");
        match ensure(&source, &target) {
            Ok(exe) => Some(exe),
            Err(err) => {
                tracing::debug!(%err, "notifier app not built; falling back to osascript");
                None
            }
        }
    })
    .as_deref()
}

/// The `terminal-notifier.app` behind the `terminal-notifier` on PATH (or
/// Homebrew's usual prefixes). Homebrew's is a wrapper script beside the
/// bundle; a bare bundle executable names its bundle three levels up.
fn source_bundle() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    for dir in dirs {
        let Ok(bin) = dir.join("terminal-notifier").canonicalize() else {
            continue;
        };
        let candidates = [
            bin.parent()
                .and_then(Path::parent)
                .map(|p| p.join("terminal-notifier.app")),
            bin.ancestors().nth(3).map(Path::to_path_buf),
        ];
        if let Some(app) = candidates.into_iter().flatten().find(|p| {
            p.extension().is_some_and(|e| e == "app")
                && p.join("Contents/MacOS/terminal-notifier").is_file()
        }) {
            return Some(app);
        }
    }
    None
}

/// Build `target` from `source` unless its stamp says it already is, and
/// return its executable.
fn ensure(source: &Path, target: &Path) -> Result<PathBuf, String> {
    let exe = target.join("Contents/MacOS/terminal-notifier");
    let stamp_path = target.join("Contents/Resources").join(STAMP_FILE);
    let stamp = format!("{LAYOUT}:{}", source.display());
    if exe.is_file() && std::fs::read_to_string(&stamp_path).ok().as_deref() == Some(&stamp) {
        return Ok(exe);
    }
    let _ = std::fs::remove_dir_all(target);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    run(Command::new("ditto").arg(source).arg(target))?;
    let resources = target.join("Contents/Resources");
    let _ = std::fs::remove_file(resources.join("Terminal.icns"));
    std::fs::write(resources.join("orion.icns"), ICON).map_err(|e| e.to_string())?;
    let plist = target.join("Contents/Info.plist");
    for (key, value) in [
        ("CFBundleIdentifier", BUNDLE_ID),
        ("CFBundleIconFile", "orion"),
        ("CFBundleName", "orion"),
        ("CFBundleDisplayName", "orion"),
    ] {
        run(Command::new("plutil")
            .args(["-replace", key, "-string", value])
            .arg(&plist))?;
    }
    // The edits broke the original signature; an ad-hoc one is what
    // Notification Center needs to accept the bundle as an app.
    run(Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(target))?;
    // Register it so Notification Center picks up the new icon straight
    // away rather than at its next LaunchServices scan. Best effort.
    let _ = Command::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
         LaunchServices.framework/Support/lsregister",
    )
    .arg("-f")
    .arg(target)
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();
    std::fs::write(&stamp_path, stamp).map_err(|e| e.to_string())?;
    Ok(exe)
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

/// The bundle id of the terminal orion runs in, for the notification's
/// click to bring back: LaunchServices hands its own to every process a
/// GUI app starts (`__CFBundleIdentifier`), with `TERM_PROGRAM` as the
/// fallback for the terminals that set it. `None` leaves the click to
/// open nothing rather than guess.
pub(super) fn terminal_bundle_id() -> Option<String> {
    if let Some(id) = orion_core::env::non_empty("__CFBundleIdentifier") {
        if id != BUNDLE_ID
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            return Some(id);
        }
    }
    let id = match orion_core::env::non_empty("TERM_PROGRAM")?.as_str() {
        "ghostty" => "com.mitchellh.ghostty",
        "Apple_Terminal" => "com.apple.Terminal",
        "iTerm.app" => "com.googlecode.iterm2",
        "WezTerm" => "com.github.wez.wezterm",
        "vscode" => "com.microsoft.VSCode",
        _ => return None,
    };
    Some(id.into())
}
