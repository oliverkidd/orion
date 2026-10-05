//! GHOSTTY KEYBINDS: the lines orion keeps in the user's Ghostty config so
//! every ⌘ chord its keymap answers to reaches it. Ghostty claims dozens
//! of ⌘ chords for itself and never passes them to the program running
//! inside (`ghostty +list-keybinds --default`) — ⌘K clears the screen,
//! ⌘N opens a window, ⌘⇧P is its own palette — so the block releases
//! every ⌘ chord bound to an orion action, the user's rebinds included
//! ([`unbinds`]), plus the editing chords the built-in editor takes
//! ([`EDITOR_CHORDS`]), and nothing orion does not use. A few Ghostty chords are
//! never taken whatever the keymap says ([`NEVER_RELEASED`]): copy, paste,
//! quit, new tab and the window keys — all but ⌘W, which orion answers
//! itself, closing the agent or terminal in front rather than the window.
//! A chord macOS itself steals before a terminal can send it is not merely
//! released but bound to the bytes orion expects for it
//! ([`SENT_AS_KITTY`]) — ⌘. is Cancel, Escape's twin, everywhere on a Mac.
//!
//! The lines live in one marked block orion owns and rewrites in place.
//! Everything outside it is the user's and is never touched, and a block
//! already saying the right thing is not rewritten — so the file only
//! changes the first time, when a rebind adds or drops a ⌘ chord, or when
//! a newer orion ships a different keymap. Ghostty reads its config at
//! launch and on its own reload (⌘⇧,); orion asks the Ghostty it runs
//! inside for that reload itself when it changes the block, and only
//! falls back on asking the user when it cannot — and reloads one that
//! was launched before the block's last write ([`reload_ghostty`]).

use crate::keymap::{KeyChord, Keymap};
use crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};

/// The Ghostty chords orion never releases, in Ghostty's spelling, even
/// with an action rebound onto one: copy, paste, quit, new tab and the
/// window keys stay Ghostty's — copy only while Ghostty has a selection to
/// copy ([`PERFORMABLE`]). The tab digits (⌘1–⌘9) go to orion's
/// PROJECT TABS. Plain ⌘W is not among them: a stray one used to close the
/// whole orion window, so orion takes it (`Action::ClosePane`) and ⌘⇧W
/// stays the way to close the window.
pub const NEVER_RELEASED: &[&str] = &[
    "super+c",
    "super+v",
    "super+q",
    "super+shift+w",
    "super+alt+w",
    "super+alt+shift+w",
    "super+t",
    "super+shift+t",
    "super+enter",
    "super+ctrl+f",
];

/// First and last line of the block orion owns.
const BEGIN: &str =
    "# >>> orion keybinds (managed by orion; edits inside this block are replaced) >>>";
const END: &str = "# <<< orion keybinds <<<";

/// `chord` in Ghostty's trigger spelling — `super+shift+p`, `super+/`,
/// `super+arrow_up` — or None for a chord without ⌘, which Ghostty passes
/// on as it is. A shifted symbol is spelled by the key it is typed with
/// (`⌘?` is `super+shift+/`, the trigger Ghostty matches the press on).
pub fn trigger(chord: &KeyChord) -> Option<String> {
    if !chord.mods.contains(KeyModifiers::SUPER) {
        return None;
    }
    let mut shift = chord.mods.contains(KeyModifiers::SHIFT);
    let key = match chord.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => match crate::keymap::unshifted(c) {
            Some(base) => {
                shift = true;
                base.to_string()
            }
            None => c.to_lowercase().to_string(),
        },
        KeyCode::Up => "arrow_up".into(),
        KeyCode::Down => "arrow_down".into(),
        KeyCode::Left => "arrow_left".into(),
        KeyCode::Right => "arrow_right".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab | KeyCode::BackTab => "tab".into(),
        KeyCode::Esc => "escape".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "page_up".into(),
        KeyCode::PageDown => "page_down".into(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return None,
    };
    let mut out = String::from("super+");
    if chord.mods.contains(KeyModifiers::CONTROL) {
        out.push_str("ctrl+");
    }
    if chord.mods.contains(KeyModifiers::ALT) {
        out.push_str("alt+");
    }
    if shift {
        out.push_str("shift+");
    }
    out.push_str(&key);
    Some(out)
}

/// The editing chords the BUILT-IN EDITOR takes — released whatever the
/// keymap says, so they reach micro, Edit or fresh instead of Ghostty:
/// the ones it takes as their Ctrl twins (`event_loop::cmd_as_ctrl`: ⌘S
/// save, ⌘Z undo, ⌘⇧Z redo, ⌘D next match, ⌘F find, ⌘A select all, ⌘X
/// cut), and the Mac editing chords it turns into the editor's own keys
/// (`editor::Kind::mac_key`): ⌘↑/⌘↓ and ⇧⌘↑/⇧⌘↓ to the file's ends —
/// Ghostty's jump to prompt — ⌥⌘↑/⌥⌘↓ a cursor above or below —
/// Ghostty's split up and down; ⌥⌘←/⌥⌘→ stay its split left and right —
/// ⌘L the line, ⌘⇧L every match, ⌘/ comment, ⌘⇧P the palette. ⌘←/⌘→
/// are not here: Ghostty types `^A`/`^E` for them, which the editor reads
/// as the line's ends, and which a shell outside orion still needs. Paste
/// stays Ghostty's; copy reaches orion when Ghostty has nothing selected
/// ([`PERFORMABLE`]).
///
/// Every typed field (`text_input`) takes three of them as well: ⌘A
/// selects the field's whole text, ⌘X cuts the selection, and ⇧⌘↑/⇧⌘↓
/// select to its ends. Its other
/// selection chords need nothing here — Ghostty binds no ⇧⌘←/⇧⌘→ or
/// ⌥⇧←/⌥⇧→, and its ⇧-arrow, ⇧Home/⇧End and ⇧PgUp/⇧PgDn binds are
/// `performable` (they adjust a terminal selection only when one exists),
/// so all of them arrive as modified keys.
pub const EDITOR_CHORDS: &[&str] = &[
    "super+s",
    "super+z",
    "super+shift+z",
    "super+d",
    "super+f",
    "super+a",
    "super+x",
    "super+arrow_up",
    "super+arrow_down",
    "super+shift+arrow_up",
    "super+shift+arrow_down",
    "super+alt+arrow_up",
    "super+alt+arrow_down",
    "super+l",
    "super+shift+l",
    "super+/",
    "super+shift+p",
];

/// The modals' own ⌘ verbs — the keys a modal's table matches while it
/// is up, which no registry action answers to, so [`keymap_unbinds`]
/// never sees them: each modal's verb shares its letter with the grid's
/// action for the same thing (⌘E changes, ⌘R refresh, ⌘O open outside,
/// ⌘N new, ⌘W close), and the few with no grid twin (⌘I edit, ⌘D
/// ready/draft, ⌘X merge) still have to reach orion. Released whatever
/// the keymap says, as [`EDITOR_CHORDS`] are.
const MODAL_KEYS: &[crate::hints::Key] = &[
    crate::pr_modal::keys::DIFF,
    crate::pr_modal::keys::NEW,
    crate::pr_modal::keys::MERGE,
    crate::pr_modal::keys::CLOSE,
    crate::pr_modal::keys::READY,
    crate::pr_modal::keys::LINEAR,
    crate::issues::keys::EDIT,
    crate::issues::keys::COMMENT,
    crate::issues::keys::BROWSER,
    crate::issues::keys::REFRESH,
    crate::linear::keys::STATUS,
    crate::linear::keys::ATTACH,
    crate::skills::keys::NEW,
    crate::skills::keys::TRASH,
    crate::preset_overlays::keys::DELETE,
    crate::ui::diff_keys::ALL,
    crate::ui::diff_keys::MODE,
    crate::ui::diff_keys::REVIEWED,
    crate::ui::diff_keys::TREE,
    crate::ui::finder_keys::FOCUS_ROW,
    crate::ui::finder_keys::SOURCE,
    crate::branch_switch::keys::FETCH,
    crate::hints::COPY_PATH,
    crate::hints::IN_CURSOR,
];

/// [`MODAL_KEYS`]' ⌘ chords that Ghostty would otherwise keep.
fn modal_unbinds() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for key in MODAL_KEYS {
        for chord in key.chords() {
            if let Some(t) = trigger(&chord).filter(|_| releases(&chord)) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// The ⌘ chords macOS turns into something else before the terminal can
/// encode them, each with the bytes the block makes Ghostty send in its
/// place. Unbinding one is not enough: the press still goes through
/// Cocoa's key handling, which reads it as a command, not a key.
///
/// * `⌘.` is Cancel — `cancelOperation:`, the command Escape sends — in
///   every Mac app, so with only `unbind` it reaches orion as an Escape
///   (at best one still carrying ⌘), and **Select worktree** closed the
///   new-agent box it was pressed in instead of opening its picker.
///   Ghostty's `csi:` action writes `ESC [` and the text after it, and
///   `46;9u` is the KITTY PROTOCOL's own spelling of ⌘. — codepoint 46,
///   modifiers 1 + 8 (super) — which crossterm reads as `Char('.')` with
///   SUPER, the chord the keymap binds. orion folds an Escape carrying ⌘
///   into the same chord ([`crate::keymap::untangle_cmd_period`]) for a
///   terminal or a config without this line.
///
/// A trigger here is bound this way only when the block would release it
/// anyway — some action answers to it — and the rest of the keymap's ⌘
/// chords are plain `unbind`s.
pub const SENT_AS_KITTY: &[(&str, &str)] = &[("super+.", "csi:46;9u")];

/// Ghostty actions the block keeps but marks `performable:` — taken by
/// Ghostty only when they can act, otherwise handed to orion as the key.
/// ⌘C copies Ghostty's own (mouse) selection when there is one; with
/// none — the case in orion, which draws its selections itself — it
/// arrives as ⌘C, and a text field copies its SELECTION, the editor its
/// own, and the session pane its drag selection. Bound as Ghostty binds
/// it by default (`ghostty +list-keybinds --default`).
pub const PERFORMABLE: &[(&str, &str)] = &[("super+c", "copy_to_clipboard:mixed")];

/// What the block binds `trigger` to: its [`SENT_AS_KITTY`] bytes, or
/// `unbind` — handed to the program inside as Ghostty encodes it.
pub fn action(trigger: &str) -> &'static str {
    SENT_AS_KITTY
        .iter()
        .find(|(t, _)| *t == trigger)
        .map_or("unbind", |(_, sent)| sent)
}

/// Whether the block releases `chord`: a ⌘ chord not on
/// [`NEVER_RELEASED`].
pub fn releases(chord: &KeyChord) -> bool {
    trigger(chord).is_some_and(|t| !NEVER_RELEASED.contains(&t.as_str()))
}

/// The triggers the block unbinds for `keymap`: every ⌘ chord any action
/// answers to, in the order the actions are declared, each once — a
/// digit twice, by its character and by its key (`super+0` and
/// `super+digit_0`), since Ghostty binds the tab digits both ways.
pub fn unbinds(keymap: &Keymap) -> Vec<String> {
    let mut out = keymap_unbinds(keymap);
    let modal = modal_unbinds();
    for chord in EDITOR_CHORDS
        .iter()
        .copied()
        .chain(modal.iter().map(String::as_str))
    {
        if !out.iter().any(|t| t == chord) {
            out.push(chord.to_string());
        }
    }
    out
}

/// [`unbinds`]' keymap half: every ⌘ chord an action answers to.
fn keymap_unbinds(keymap: &Keymap) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for index in 0..crate::keymap::ACTIONS.len() {
        for chord in keymap.chords_at(index) {
            if !releases(chord) {
                continue;
            }
            let Some(trigger) = trigger(chord) else {
                continue;
            };
            let digit = match chord.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    Some(trigger.replace(&format!("+{c}"), &format!("+digit_{c}")))
                }
                _ => None,
            };
            for t in std::iter::once(trigger).chain(digit) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// The block as written for `keymap`: the markers around one
/// `keybind = <trigger>=<action>` per [`unbinds`] entry — `unbind`, or
/// the bytes [`SENT_AS_KITTY`] sends for a chord macOS steals — and the
/// [`PERFORMABLE`] binds after them.
pub fn block(keymap: &Keymap) -> String {
    let mut out = String::from(BEGIN);
    out.push('\n');
    for trigger in unbinds(keymap) {
        out.push_str(&format!("keybind = {trigger}={}\n", action(&trigger)));
    }
    for (trigger, action) in PERFORMABLE {
        out.push_str(&format!("keybind = performable:{trigger}={action}\n"));
    }
    out.push_str(END);
    out.push('\n');
    out
}

/// `existing` with orion's block for `keymap` in it: an old block
/// replaced where it stands, otherwise the block appended after a blank
/// line.
pub fn with_block(existing: &str, keymap: &Keymap) -> String {
    let wanted = block(keymap);
    if let (Some(start), Some(end_at)) = (existing.find(BEGIN), existing.find(END)) {
        if end_at > start {
            let mut end = end_at + END.len();
            if existing[end..].starts_with('\n') {
                end += 1;
            }
            return format!("{}{wanted}{}", &existing[..start], &existing[end..]);
        }
    }
    let mut out = existing.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&wanted);
    out
}

/// Where orion's block stands in the text of a Ghostty config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// There, and what `keymap` would write.
    Current,
    /// There, but written for other keys — an older orion's, or before a
    /// rebind.
    Stale,
    Missing,
}

/// Whether `existing` holds orion's block for `keymap` as it would be
/// written now — what `orion doctor` reports.
pub fn block_state(existing: &str, keymap: &Keymap) -> BlockState {
    if with_block(existing, keymap) == existing {
        BlockState::Current
    } else if existing.contains(BEGIN) {
        BlockState::Stale
    } else {
        BlockState::Missing
    }
}

/// The config file Ghostty reads on macOS, by its own rule: the first of
/// Application Support's `config.ghostty` / `config` and the XDG dir's
/// `config.ghostty` / `config` that exists with something in it, else
/// Application Support's `config.ghostty` (what Ghostty creates first).
pub fn config_path(home: &Path, xdg_config_home: Option<&Path>) -> PathBuf {
    let app_support = home.join("Library/Application Support/com.mitchellh.ghostty");
    let xdg = xdg_config_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".config"))
        .join("ghostty");
    let candidates = [
        app_support.join("config.ghostty"),
        app_support.join("config"),
        xdg.join("config.ghostty"),
        xdg.join("config"),
    ];
    candidates
        .iter()
        .find(|path| std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0))
        .cloned()
        .unwrap_or_else(|| app_support.join("config.ghostty"))
}

/// Put orion's block for `keymap` in the file at `path`, creating it (and
/// its folder) when missing. True when the file changed.
pub fn ensure(path: &Path, keymap: &Keymap) -> std::io::Result<bool> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let updated = with_block(&existing, keymap);
    if updated == existing {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, updated)?;
    Ok(true)
}

/// Whether orion is running inside Ghostty right now.
pub fn inside_ghostty() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|v| v.eq_ignore_ascii_case("ghostty"))
}

/// The first Ghostty that reloads its config on `SIGUSR2`. An older one
/// takes the signal's default action and quits, so it is never sent one.
const RELOADS_ON_SIGUSR2: (u32, u32) = (1, 2);

/// Whether `version` — `TERM_PROGRAM_VERSION` as Ghostty sets it, `1.3.1`
/// or `1.2.0-dev+abc` — is a Ghostty that reloads on `SIGUSR2`.
fn reloads_on_sigusr2(version: &str) -> bool {
    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    let (Some(Ok(major)), Some(Ok(minor))) = (
        parts.next().map(str::parse::<u32>),
        parts.next().map(str::parse::<u32>),
    ) else {
        return false;
    };
    (major, minor) >= RELOADS_ON_SIGUSR2
}

/// The Ghostty process `pid` runs under, read off `table` — `ps -axo
/// pid=,ppid=,etime=,comm=` output: the nearest ancestor whose command is
/// Ghostty's own binary (`…/Ghostty.app/Contents/MacOS/ghostty`), with
/// the seconds it has been running. None when the chain ends without one,
/// or loops.
fn ghostty_ancestor(table: &str, pid: u32) -> Option<(u32, u64)> {
    let rows: std::collections::HashMap<u32, (u32, &str, &str)> = table
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (pid, ppid, etime) = (fields.next()?, fields.next()?, fields.next()?);
            // The command is the rest of the line: an app path may hold spaces.
            let comm = line.split_once(etime)?.1.trim();
            Some((pid.parse().ok()?, (ppid.parse().ok()?, etime, comm)))
        })
        .collect();
    let mut at = rows.get(&pid)?.0;
    for _ in 0..rows.len() {
        let (ppid, etime, comm) = rows.get(&at)?;
        if Path::new(comm)
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("ghostty"))
        {
            return Some((at, elapsed_secs(etime)?));
        }
        at = *ppid;
    }
    None
}

/// The Ghostty orion runs inside, when it is one that can be asked to
/// reload, and how long it has been running: None outside Ghostty, on a
/// Ghostty too old to take the signal ([`RELOADS_ON_SIGUSR2`]), or when
/// its process is not found.
fn reloadable_ghostty() -> Option<(u32, u64)> {
    if !inside_ghostty() {
        return None;
    }
    let version = std::env::var("TERM_PROGRAM_VERSION").unwrap_or_default();
    if !reloads_on_sigusr2(&version) {
        return None;
    }
    let ps = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,etime=,comm="])
        .output()
        .ok()?;
    ghostty_ancestor(&String::from_utf8_lossy(&ps.stdout), std::process::id())
}

/// Seconds in `etime` — `ps -o etime=`'s `[[dd-]hh:]mm:ss`.
fn elapsed_secs(etime: &str) -> Option<u64> {
    let etime = etime.trim();
    let (days, clock) = match etime.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, etime),
    };
    let mut secs = 0;
    for part in clock.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + secs)
}

/// Whether a Ghostty `running` seconds started before `path` was last
/// written — so the config it holds is older than the file. One second of
/// slack for `ps`'s whole-second clock.
fn started_before_write(running: u64, path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|written_ago| running > written_ago.as_secs() + 1)
}

/// Ask Ghostty `pid` to reload its config — what ⌘⇧, does — with
/// `SIGUSR2`. True when the signal went.
fn reload(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-USR2", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}

/// The Ghostty and config write a reload was last sent for — `<pid>
/// <mtime secs>` in the runtime dir — so a Ghostty launched before the
/// write is reloaded once, not by every orion started in it after.
fn reloaded_marker() -> PathBuf {
    orion_core::paths::runtime_dir().join("ghostty-reloaded")
}

/// What [`reloaded_marker`] holds for Ghostty `pid` and the file at `path`
/// as it stands now; None when the file's time cannot be read.
fn reload_stamp(pid: u32, path: &Path) -> Option<String> {
    let written = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let secs = written
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{pid} {secs}"))
}

/// Bring the Ghostty orion runs inside up to date with the block in
/// `path`: orion just rewrote it (`changed`), or Ghostty was launched
/// before its last write and has not been reloaded for it since. Until it
/// reloads the old block stands — ⌘1 is still Ghostty's tab key and ⌘.
/// still macOS's Cancel, which reaches orion as a bare Escape and closes
/// the new-agent box. True when Ghostty was asked to reload.
fn reload_ghostty(path: &Path, changed: bool) -> bool {
    let Some((pid, running)) = reloadable_ghostty() else {
        return false;
    };
    let stamp = reload_stamp(pid, path);
    let marker = reloaded_marker();
    let done_already = stamp.is_some()
        && std::fs::read_to_string(&marker)
            .ok()
            .as_deref()
            .map(str::trim)
            == stamp.as_deref();
    if !changed && (done_already || !started_before_write(running, path)) {
        return false;
    }
    if !reload(pid) {
        return false;
    }
    if let Some(stamp) = stamp {
        let _ = std::fs::write(&marker, stamp);
    }
    true
}

/// The startup pass, and the one after every change to the keymap or the
/// Ghostty settings: with the `ghostty_keybinds` SETTING on, on a local
/// Mac, and Ghostty in the picture — orion running inside it, or it being
/// the **Outside terminal** — make sure the block for the config's keymap
/// (its `keybindings` over the defaults) is in Ghostty's config
/// (`ORION_GHOSTTY_CONFIG` names another file, or `off` none). The flash
/// to show when the file changed — Ghostty has to reload it — or could
/// not be written; None when there was nothing to do. Unit tests never get
/// past the first check: they must not write the machine's real config.
pub fn ensure_for(cfg: &crate::config::Config) -> Option<crate::flash::Flash> {
    if cfg!(test)
        || !cfg.ghostty_keybinds
        || !cfg!(target_os = "macos")
        || orion_core::host::is_remote_session()
    {
        return None;
    }
    let path = match std::env::var(orion_core::env::GHOSTTY_CONFIG) {
        Ok(v) if v.eq_ignore_ascii_case("off") => return None,
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v),
        _ => {
            let ghostty_in_use = inside_ghostty()
                || (cfg.outside_terminal() == crate::config::OutsideTerminal::Ghostty
                    && crate::event_loop::ghostty_app().is_some());
            if !ghostty_in_use {
                return None;
            }
            let home = PathBuf::from(std::env::var_os("HOME")?);
            let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
            config_path(&home, xdg.as_deref())
        }
    };
    let changed = match ensure(&path, &cfg.keymap()) {
        Ok(changed) => changed,
        Err(e) => {
            return Some(crate::flash::Flash::failed(format!(
                "couldn't update Ghostty's config {}: {e}",
                path.display()
            )))
        }
    };
    let reloaded = reload_ghostty(&path, changed);
    match (changed, reloaded) {
        (true, true) => Some(crate::flash::Flash::done(format!(
            "updated orion's keybinds in {} and reloaded Ghostty",
            path.display()
        ))),
        (true, false) => Some(crate::flash::Flash::setup(format!(
            "updated orion's keybinds in {} — reload Ghostty's config (⌘⇧,) to use ⌘ chords",
            path.display()
        ))),
        (false, true) => Some(crate::flash::Flash::done(
            "reloaded Ghostty's config so orion's ⌘ keys reach it",
        )),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// orion finds the Ghostty it runs under through the shell and
    /// `login` between them, by the binary's name whatever the app path
    /// holds, and finds none under another terminal or in a looping table.
    #[test]
    fn the_ghostty_ancestor_is_found_up_the_chain() {
        let table = "    1     0 2-01:00:00 /sbin/launchd
 1588     1    12:34 /Applications/Ghostty.app/Contents/MacOS/ghostty
 1591  1588    12:30 /usr/bin/login
 1593  1591    12:30 -/bin/zsh
 2355  1593    00:07 orion
  900     1    01:00 /Applications/Other Term.app/Contents/MacOS/Other Term
  901   900    01:00 -/bin/zsh
  902   901    00:30 orion
  7     8    00:01 a
  8     7    00:01 b
  9     8    00:01 orion
";
        assert_eq!(ghostty_ancestor(table, 2355), Some((1588, 754)));
        assert_eq!(ghostty_ancestor(table, 902), None);
        assert_eq!(ghostty_ancestor(table, 9), None);
        assert_eq!(ghostty_ancestor(table, 4242), None);
    }

    /// `ps`'s elapsed time reads in every width it prints.
    #[test]
    fn elapsed_times_read_in_every_width() {
        assert_eq!(elapsed_secs("   00:07\n"), Some(7));
        assert_eq!(elapsed_secs("12:34"), Some(754));
        assert_eq!(elapsed_secs("01:02:03"), Some(3723));
        assert_eq!(elapsed_secs("2-01:02:03"), Some(2 * 86_400 + 3723));
        assert_eq!(elapsed_secs(""), None);
    }

    /// Only a Ghostty from 1.2 on is sent `SIGUSR2`: an older one would
    /// quit on it, and an unreadable version is taken as older.
    #[test]
    fn only_a_ghostty_that_reloads_on_sigusr2_is_signalled() {
        for v in ["1.2.0", "1.3.1", "1.2.0-dev+abc", "2.0.0"] {
            assert!(reloads_on_sigusr2(v), "{v}");
        }
        for v in ["1.1.3", "1.0.0", "0.9", "", "tip", "1"] {
            assert!(!reloads_on_sigusr2(v), "{v}");
        }
    }

    /// The editor's ⌘ shortcuts are released with an empty keymap too,
    /// once each, and none of Ghostty's own copy, paste or close keys.
    #[test]
    fn the_editors_chords_are_always_released() {
        let all = unbinds(&Keymap::default());
        for chord in EDITOR_CHORDS {
            assert_eq!(all.iter().filter(|t| t == chord).count(), 1, "{all:?}");
            assert!(!NEVER_RELEASED.contains(chord));
        }
    }

    /// Every Mac editing chord the editor turns into its own key is in the
    /// block, spelled as [`trigger`] spells it — and ⌘←/⌘→, which Ghostty
    /// types as `^A`/`^E`, and ⌥⌘←/⌥⌘→, its split keys, are left to it.
    #[test]
    fn the_mac_editing_chords_are_released() {
        let all = unbinds(&Keymap::default());
        for spec in [
            "cmd+up",
            "cmd+down",
            "shift+cmd+up",
            "shift+cmd+down",
            "alt+cmd+up",
            "alt+cmd+down",
            "cmd+l",
            "shift+cmd+l",
            "cmd+/",
            "shift+cmd+p",
        ] {
            let t = trigger(&KeyChord::parse(spec).unwrap()).unwrap();
            assert!(all.contains(&t), "{spec} ({t}) not released");
        }
        for kept in [
            "super+arrow_left",
            "super+arrow_right",
            "super+alt+arrow_left",
            "super+alt+arrow_right",
        ] {
            assert!(!all.iter().any(|t| t == kept), "{kept} released");
        }
    }

    /// A typed field's ⌘ chords reach it: ⌘A — Ghostty's `select_all` —
    /// and ⇧⌘↑/⇧⌘↓ — its `jump_to_prompt` — are released, while ⇧⌘←/⇧⌘→,
    /// which Ghostty binds to nothing, need no line and get none.
    #[test]
    fn the_text_fields_cmd_chords_reach_orion() {
        let all = unbinds(&Keymap::default());
        let cmd_chords = |key: crate::hints::Key| {
            key.chords()
                .into_iter()
                .filter(|c| c.mods.contains(KeyModifiers::SUPER))
                .collect::<Vec<_>>()
        };
        use crate::text_input::keys::{SELECT_ALL, SELECT_LINE};
        for chord in cmd_chords(SELECT_ALL) {
            let t = trigger(&chord).unwrap();
            assert_eq!(t, "super+a");
            assert!(all.contains(&t), "{t} not released");
        }
        for spec in ["shift+cmd+up", "shift+cmd+down"] {
            let t = trigger(&KeyChord::parse(spec).unwrap()).unwrap();
            assert!(all.contains(&t), "{spec} ({t}) not released");
        }
        let line = cmd_chords(SELECT_LINE);
        assert_eq!(line.len(), 2);
        for chord in line {
            let t = trigger(&chord).unwrap();
            assert!(
                !all.contains(&t),
                "{t} is not Ghostty's: nothing to release"
            );
        }
    }

    /// The default keymap's ⌘ chords, in the order the actions are
    /// declared — and nothing without ⌘.
    #[test]
    fn the_block_unbinds_every_cmd_chord_the_keymap_binds() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap_unbinds(&keymap),
            [
                "super+k",
                "super+1",
                "super+digit_1",
                "super+2",
                "super+digit_2",
                "super+3",
                "super+digit_3",
                "super+4",
                "super+digit_4",
                "super+5",
                "super+digit_5",
                "super+6",
                "super+digit_6",
                "super+7",
                "super+digit_7",
                "super+8",
                "super+digit_8",
                "super+9",
                "super+digit_9",
                "super+e",
                "super+r",
                "super+u",
                "super+l",
                "super+shift+a",
                "super+shift+u",
                "super+backspace",
                "super+w",
                "super+n",
                "super+/",
                "super+y",
                "super+shift+/",
                "super+.",
                "super+p",
                "super+shift+f",
                "super+b",
                "super+s",
                "super+j",
                "super+f",
                "super+g",
                "super+,",
                "super+shift+r",
                "super+shift+g",
                "super+shift+p",
                "super+o",
            ]
        );
        let block = block(&keymap);
        assert!(block.starts_with(&format!("{BEGIN}\nkeybind = super+k=unbind\n")));
        assert!(block.ends_with(&format!(
            "keybind = super+shift+l=unbind\n\
             keybind = super+i=unbind\n\
             keybind = performable:super+c=copy_to_clipboard:mixed\n{END}\n"
        )));
        assert!(
            !block.contains("super+c=unbind"),
            "⌘C stays Ghostty's copy while it has a selection"
        );
        assert!(
            !block.contains("ctrl+"),
            "a ^ twin is never Ghostty's to give"
        );
        assert!(block.contains("\nkeybind = super+.=csi:46;9u\n"), "{block}");
        assert!(!block.contains("super+.=unbind"));
        assert!(
            !block.contains("super+m="),
            "⌘M is no key of orion's: Ghostty keeps it"
        );
    }

    /// Every chord macOS steals is sent as the KITTY PROTOCOL spells the
    /// chord itself — `CSI <codepoint> ; <1 + modifier bits> u` — so it
    /// arrives as the very chord the keymap binds, and is written only
    /// while some action answers to it.
    #[test]
    fn a_chord_macos_steals_is_sent_as_its_kitty_sequence() {
        for (trigger, sent) in SENT_AS_KITTY {
            let spec = trigger.replace("super", "cmd");
            let chord = KeyChord::parse(&spec).unwrap();
            assert_eq!(super::trigger(&chord).as_deref(), Some(*trigger));
            let KeyCode::Char(c) = chord.code else {
                panic!("{trigger}: not a character key");
            };
            let mut bits = 0;
            for (held, bit) in [
                (KeyModifiers::SHIFT, 1),
                (KeyModifiers::ALT, 2),
                (KeyModifiers::CONTROL, 4),
                (KeyModifiers::SUPER, 8),
            ] {
                if chord.mods.contains(held) {
                    bits += bit;
                }
            }
            assert_eq!(*sent, format!("csi:{};{}u", c as u32, 1 + bits));
            assert_eq!(action(trigger), *sent);
        }
        assert_eq!(action("super+k"), "unbind");
        // Rebound off ⌘., the line goes with it: Ghostty has it back.
        let mut keymap = Keymap::default();
        let worktree =
            crate::keymap::index_of(crate::keymap::Action::SelectLaunchWorktree).unwrap();
        keymap.bind(worktree, KeyChord::parse("cmd+u").unwrap(), false);
        assert!(!block(&keymap).contains("super+."));
    }

    /// Every ⌘ chord an action answers to is released — the derivation
    /// covers the registry, not a list kept beside it.
    #[test]
    fn every_registry_cmd_chord_is_released() {
        let keymap = Keymap::default();
        let released = unbinds(&keymap);
        for index in 0..crate::keymap::ACTIONS.len() {
            for chord in keymap.chords_at(index) {
                match trigger(chord) {
                    Some(t) => assert!(released.contains(&t), "{t} not released"),
                    None => assert!(!chord.mods.contains(KeyModifiers::SUPER)),
                }
            }
        }
    }

    /// Every ⌘ chord a modal's table answers to reaches orion: the block
    /// releases it — or it is one Ghostty never gives up (copy), which
    /// then arrives only when Ghostty has nothing to do with it.
    #[test]
    fn every_modal_cmd_chord_is_released() {
        let released = unbinds(&Keymap::default());
        let tables: &[&[crate::hints::Key]] = &[
            crate::pr_modal::keys::ALL,
            crate::pr_actions::keys::ALL,
            crate::issues::keys::ALL,
            crate::linear::keys::ALL,
            crate::skills::keys::ALL,
            crate::preset_overlays::keys::ALL,
            crate::ui::diff_keys::ALL_KEYS,
            crate::branch_switch::keys::ALL,
            &[
                crate::ui::finder_keys::ATTACH,
                crate::ui::finder_keys::FOCUS_ROW,
                crate::ui::finder_keys::SOURCE,
            ],
        ];
        for table in tables {
            for key in *table {
                for chord in key.chords() {
                    let Some(t) = trigger(&chord) else {
                        continue;
                    };
                    assert!(
                        released.contains(&t) || NEVER_RELEASED.contains(&t.as_str()),
                        "{t} ({}) is not released",
                        key.does
                    );
                }
            }
        }
    }

    /// A rebind is the block: a ⌘ chord bound in Settings → Hotkeys is
    /// released, the default it replaced is handed back to Ghostty, and
    /// copy, paste, quit and the window keys are never taken.
    #[test]
    fn a_rebind_moves_the_block_with_it() {
        let mut keymap = Keymap::default();
        let palette = crate::keymap::index_of(crate::keymap::Action::Palette).unwrap();
        keymap.bind(palette, KeyChord::parse("cmd+shift+k").unwrap(), false);
        let released = unbinds(&keymap);
        assert!(released.contains(&"super+shift+k".to_string()));
        assert!(!released.contains(&"super+k".to_string()));
        keymap.bind(palette, KeyChord::parse("cmd+q").unwrap(), true);
        keymap.bind(palette, KeyChord::parse("cmd+0").unwrap(), true);
        let released = unbinds(&keymap);
        assert!(
            !released.contains(&"super+q".to_string()),
            "⌘Q stays Ghostty's"
        );
        assert!(released.contains(&"super+0".to_string()));
        assert!(released.contains(&"super+digit_0".to_string()));
        assert_ne!(block(&keymap), block(&Keymap::default()));
    }

    #[test]
    fn triggers_are_spelled_the_way_ghostty_reads_them() {
        let t = |spec: &str| trigger(&KeyChord::parse(spec).unwrap());
        assert_eq!(t("cmd+shift+p").as_deref(), Some("super+shift+p"));
        assert_eq!(t("cmd+?").as_deref(), Some("super+shift+/"));
        assert_eq!(t("shift+cmd+/").as_deref(), Some("super+shift+/"));
        assert_eq!(t("cmd+up").as_deref(), Some("super+arrow_up"));
        assert_eq!(t("ctrl+cmd+pgdn").as_deref(), Some("super+ctrl+page_down"));
        assert_eq!(t("ctrl+k"), None);
        assert_eq!(t("k"), None);
    }

    #[test]
    fn the_block_is_appended_after_the_users_own_lines() {
        let keymap = Keymap::default();
        assert_eq!(with_block("", &keymap), block(&keymap));
        assert_eq!(
            with_block("font-size = 14", &keymap),
            format!("font-size = 14\n\n{}", block(&keymap))
        );
    }

    #[test]
    fn an_old_block_is_replaced_where_it_stands() {
        let keymap = Keymap::default();
        let old = format!("theme = x\n{BEGIN}\nkeybind = super+t=unbind\n{END}\nfont-size = 14\n");
        assert_eq!(
            with_block(&old, &keymap),
            format!("theme = x\n{}font-size = 14\n", block(&keymap))
        );
        let current = with_block(&old, &keymap);
        assert_eq!(
            with_block(&current, &keymap),
            current,
            "a right block stays put"
        );
    }

    #[test]
    fn ensure_writes_once_and_creates_the_folder() {
        let keymap = Keymap::default();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.ghostty");
        assert!(ensure(&path, &keymap).unwrap());
        assert!(!ensure(&path, &keymap).unwrap(), "already right: untouched");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), block(&keymap));
        // A rebind rewrites it.
        let mut rebound = keymap.clone();
        let index = crate::keymap::index_of(crate::keymap::Action::Skills).unwrap();
        rebound.bind(index, KeyChord::parse("cmd+shift+u").unwrap(), false);
        assert!(ensure(&path, &rebound).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("keybind = super+shift+u=unbind"));
        // ⌘S stays released all the same: it is the editor's save.
        assert!(text.contains("keybind = super+s=unbind"));
    }

    #[test]
    fn the_path_is_the_first_non_empty_config_ghostty_reads() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let app_support = home.join("Library/Application Support/com.mitchellh.ghostty");
        let xdg = home.join(".config/ghostty");
        assert_eq!(
            config_path(home, None),
            app_support.join("config.ghostty"),
            "nothing yet: where Ghostty creates its own"
        );
        std::fs::create_dir_all(&xdg).unwrap();
        std::fs::write(xdg.join("config"), "font-size = 14\n").unwrap();
        assert_eq!(config_path(home, None), xdg.join("config"));
        std::fs::create_dir_all(&app_support).unwrap();
        std::fs::write(app_support.join("config.ghostty"), "").unwrap();
        assert_eq!(
            config_path(home, None),
            xdg.join("config"),
            "an empty file is passed over"
        );
        std::fs::write(app_support.join("config.ghostty"), "theme = x\n").unwrap();
        assert_eq!(config_path(home, None), app_support.join("config.ghostty"));
    }

    #[test]
    fn xdg_config_home_moves_the_xdg_candidates() {
        let home = tempfile::tempdir().unwrap();
        let xdg_home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(xdg_home.path().join("ghostty")).unwrap();
        let file = xdg_home.path().join("ghostty/config.ghostty");
        std::fs::write(&file, "theme = x\n").unwrap();
        assert_eq!(config_path(home.path(), Some(xdg_home.path())), file);
    }
}
