//! The one-line KEY HINTS along the footer, built from one shape: a list
//! of what each key does, spelled from the live keymap. An action with no
//! key in it is left out rather than printed as `—`, so a hint never
//! names a key that does nothing — the COMMAND PALETTE reaches the rest.

use crate::keymap::{Action, Keymap};

/// One entry of a hint line.
pub enum Hint<'a> {
    /// What `action` does, under the key this terminal can press for it.
    Act(Action, &'a str),
    /// A key that is not an action — an overlay's own — spelled out.
    Lit(&'a str, &'a str),
}

/// `key: does  key: does …`, skipping the actions bound to nothing.
pub fn line(keymap: &Keymap, hints: &[Hint]) -> String {
    joined(keymap, hints, "  ")
}

/// [`line`] with its entries joined by `sep`.
pub fn joined(keymap: &Keymap, hints: &[Hint], sep: &str) -> String {
    hints
        .iter()
        .filter_map(|hint| match hint {
            Hint::Act(action, does) => keymap
                .shown_first(*action)
                .map(|chord| format!("{}: {does}", chord.display())),
            Hint::Lit(key, does) => Some(format!("{key}: {does}")),
        })
        .collect::<Vec<_>>()
        .join(sep)
}

/// The chord for copying: `⌘C` where the terminal sends ⌘, `^y` where it
/// does not.
pub fn copy_key() -> &'static str {
    if crate::keymap::cmd_shown() {
        "⌘C"
    } else {
        "^y"
    }
}

/// The chord that hands what the cursor is on to Cursor: `⌘O`, or `^o`.
pub fn outside_key() -> &'static str {
    if crate::keymap::cmd_shown() {
        "⌘O"
    } else {
        "^o"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hint_line_leaves_the_unbound_actions_out() {
        let keymap = Keymap::default();
        let text = line(
            &keymap,
            &[
                Hint::Act(Action::FindFile, "go to file"),
                Hint::Act(Action::Help, "help"),
                Hint::Lit("Esc", "close"),
            ],
        );
        assert_eq!(text, "^p: go to file  Esc: close");
    }
}
