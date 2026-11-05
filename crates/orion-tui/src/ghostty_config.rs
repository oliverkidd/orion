//! GHOSTTY KEYS: what Orion.app's Ghostty keeps of its own keys. Ghostty
//! claims dozens of ⌘ chords for itself and never passes them to the
//! program running inside (`ghostty +list-keybinds --default`) — ⌘K
//! clears the screen, ⌘N opens a window, ⌘T a tab, ⌘⇧P is its own palette.
//! In Orion.app (`app_bundle`) the program inside is always orion, so the
//! app's own config ([`app_keybinds`]) clears Ghostty's whole table and
//! binds back only what a Mac app should still answer to itself
//! ([`KEPT`]): copy, paste, quit, close window, full screen and the text
//! size — and the Mac editing keys Ghostty types as the shell's own
//! ([`TYPED`]), which every agent's prompt inside orion counts on. Every
//! other chord, whatever the keymap or a rebind says, reaches orion as the
//! key it is. A chord macOS itself steals before a terminal
//! can send it is bound to the bytes orion expects for it
//! ([`SENT_AS_KITTY`]) — ⌘. is Cancel, Escape's twin, everywhere on a Mac.
//!
//! Nothing is written into the user's own Ghostty config: in a Ghostty
//! window of the user's own, Ghostty keeps its ⌘ chords for itself.

use crate::keymap::KeyChord;
use crossterm::event::{KeyCode, KeyModifiers};

/// The keys Orion.app's Ghostty keeps, each as Ghostty binds it by
/// default: `(trigger, action)`. Copy only while Ghostty has a selection
/// to copy ([`PERFORMABLE`]).
pub const KEPT: &[(&str, &str)] = &[
    ("super+q", "quit"),
    ("super+c", "copy_to_clipboard:mixed"),
    ("super+v", "paste_from_clipboard"),
    ("super+shift+w", "close_window"),
    ("super+enter", "toggle_fullscreen"),
    ("super+ctrl+f", "toggle_fullscreen"),
    ("super+=", "increase_font_size:1"),
    ("super++", "increase_font_size:1"),
    ("super+-", "decrease_font_size:1"),
    ("super+0", "reset_font_size"),
];

/// The Mac editing keys Ghostty types as the keys a shell reads, bound in
/// the app as it binds them by default: ⌘←/⌘→ the line's ends (`^A`,
/// `^E`) and ⌥←/⌥→ a word back and on (`⎋b`, `⎋f`). orion's fields, its
/// editor and the agents' own prompts all read those bytes. Ghostty's
/// fifth, ⌘⌫ erasing the line (`^U`), is not among them: ⌘⌫ is orion's
/// delete, and in a locked pane orion sends the agent its kill-line.
pub const TYPED: &[(&str, &str)] = &[
    ("super+arrow_left", "text:\\x01"),
    ("super+arrow_right", "text:\\x05"),
    ("alt+arrow_left", "esc:b"),
    ("alt+arrow_right", "esc:f"),
];

/// The [`KEPT`] keys bound `performable:` — taken by Ghostty only when
/// they can act, otherwise handed to orion as the key. ⌘C copies Ghostty's
/// own (mouse) selection when there is one; with none — the case in
/// orion, which draws its selections itself — it arrives as ⌘C, and a
/// text field copies its SELECTION, the editor its own, and the session
/// pane its drag selection.
pub const PERFORMABLE: &[&str] = &["super+c"];

/// The ⌘ chords macOS turns into something else before the terminal can
/// encode them, each with the bytes the app's config makes Ghostty send
/// in its place. Leaving one unbound is not enough: the press still goes
/// through Cocoa's key handling, which reads it as a command, not a key.
///
/// * `⌘.` is Cancel — `cancelOperation:`, the command Escape sends — in
///   every Mac app, so unbound it reaches orion as an Escape (at best one
///   still carrying ⌘), and **Select worktree** closed the new-agent box
///   it was pressed in instead of opening its picker. Ghostty's `csi:`
///   action writes `ESC [` and the text after it, and `46;9u` is the KITTY
///   PROTOCOL's own spelling of ⌘. — codepoint 46, modifiers 1 + 8 (super)
///   — which crossterm reads as `Char('.')` with SUPER, the chord the
///   keymap binds. orion folds an Escape carrying ⌘ into the same chord
///   ([`crate::keymap::untangle_cmd_period`]) for a terminal without it.
pub const SENT_AS_KITTY: &[(&str, &str)] = &[("super+.", "csi:46;9u")];

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

/// Whether Orion.app's Ghostty hands `chord` to orion as the key it is: a
/// ⌘ chord that is neither one of [`KEPT`] nor one of [`TYPED`]. `⌘+` is
/// typed with shift, and kept as `⌘=` is.
pub fn releases(chord: &KeyChord) -> bool {
    trigger(chord).is_some_and(|t| {
        t != "super+shift+=" && !KEPT.iter().chain(TYPED).any(|(bound, _)| *bound == t)
    })
}

/// The `keybind` values of Orion.app's Ghostty config, in order: `clear`
/// — Ghostty's defaults and whatever the user's own config bound, gone —
/// then [`KEPT`], [`TYPED`] and [`SENT_AS_KITTY`].
pub fn app_keybinds() -> Vec<String> {
    let kept = KEPT.iter().map(|(trigger, action)| {
        let performable = if PERFORMABLE.contains(trigger) {
            "performable:"
        } else {
            ""
        };
        format!("{performable}{trigger}={action}")
    });
    let sent = TYPED
        .iter()
        .chain(SENT_AS_KITTY)
        .map(|(trigger, bytes)| format!("{trigger}={bytes}"));
    std::iter::once("clear".to_string())
        .chain(kept)
        .chain(sent)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(spec: &str) -> KeyChord {
        KeyChord::parse(spec).unwrap_or_else(|| panic!("{spec} parses"))
    }

    /// The app's table is cleared first, then holds exactly the kept keys
    /// and ⌘.'s bytes — nothing derived from the keymap, so a rebind never
    /// changes it.
    #[test]
    fn the_apps_table_is_cleared_then_holds_the_kept_keys() {
        let binds = app_keybinds();
        assert_eq!(binds[0], "clear");
        assert_eq!(
            binds.len(),
            1 + KEPT.len() + TYPED.len() + SENT_AS_KITTY.len()
        );
        assert!(binds.contains(&"super+arrow_left=text:\\x01".to_string()));
        assert!(binds.contains(&"alt+arrow_right=esc:f".to_string()));
        assert!(binds.contains(&"super+q=quit".to_string()));
        assert!(binds.contains(&"performable:super+c=copy_to_clipboard:mixed".to_string()));
        assert!(binds.contains(&"super+==increase_font_size:1".to_string()));
        assert_eq!(binds.last().unwrap(), "super+.=csi:46;9u");
        assert!(
            !binds.iter().any(|b| b.contains("unbind")),
            "cleared, not unbound one by one: {binds:?}"
        );
    }

    /// Everything Ghostty would have kept for tabs, splits, its palette
    /// and its screen reaches orion in the app; copy, paste, quit, the
    /// window keys and the text size stay Ghostty's.
    #[test]
    fn only_the_kept_keys_stay_ghosttys() {
        for spec in [
            "cmd+t",
            "cmd+n",
            "cmd+k",
            "cmd+d",
            "cmd+w",
            "cmd+shift+p",
            "cmd+1",
            "cmd+,",
            // The PULL REQUESTS MODAL's edit.
            "cmd+;",
            // A new agent in a new worktree.
            "cmd+shift+n",
        ] {
            assert!(releases(&chord(spec)), "{spec}");
        }
        for spec in [
            "cmd+c",
            "cmd+v",
            "cmd+q",
            "cmd+shift+w",
            "cmd+enter",
            "cmd+0",
            "cmd+=",
            "cmd+-",
        ] {
            assert!(!releases(&chord(spec)), "{spec}");
        }
        for spec in ["cmd+left", "cmd+right"] {
            assert!(
                !releases(&chord(spec)),
                "{spec} is typed as the shell's key"
            );
        }
        assert!(releases(&chord("cmd+backspace")), "⌘⌫ is orion's delete");
        assert!(
            !releases(&chord("ctrl+k")),
            "no ⌘: Ghostty passes it on as it is"
        );
    }

    /// ⌘. is sent as the kitty protocol spells it, whatever macOS makes
    /// of the press.
    #[test]
    fn cmd_period_is_sent_as_its_kitty_bytes() {
        for (trigger, sent) in SENT_AS_KITTY {
            let key = trigger.strip_prefix("super+").unwrap();
            let c = key.chars().next().unwrap();
            assert_eq!(*sent, format!("csi:{};{}u", c as u32, 1 + 8));
        }
    }

    #[test]
    fn chords_spell_as_ghostty_triggers() {
        assert_eq!(
            trigger(&chord("cmd+shift+p")).as_deref(),
            Some("super+shift+p")
        );
        assert_eq!(trigger(&chord("cmd+/")).as_deref(), Some("super+/"));
        assert_eq!(trigger(&chord("cmd+up")).as_deref(), Some("super+arrow_up"));
        assert_eq!(trigger(&chord("ctrl+p")), None);
    }
}
