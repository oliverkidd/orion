//! MODAL NAVIGATION: the keys every modal moves by, in one table — the one
//! place to change how orion's modals are walked.
//!
//! The rule (docs/keys.md, "Moving around a modal"):
//!
//! * `Tab` / `⇧Tab` — the big move: the keys to the next / the previous
//!   PANEL of the modal, round either end ([`PANELS`], [`panel`],
//!   [`cycle`]). The panel with the keys wears the accent.
//! * `←` / `→` — the small move sideways, inside the panel with the keys:
//!   its tabs, the value of the row under the cursor, a span, a fold
//!   ([`SIDEWAYS`], [`side`]). Never into another panel.
//! * `↑` / `↓` — the rows of the panel with the keys ([`ROWS`], [`row`]),
//!   never off its edge into another panel.
//! * `PgUp` / `PgDn`, `Home` / `End` — a page; the first and the last
//!   ([`PAGE`], [`ENDS`]).
//! * `Enter` — act on the row; on a list that feeds another panel, that
//!   panel takes the keys ([`OPEN`]).
//! * `Esc` — back a level: a selection, a filter, a panel, a page; from
//!   the top, close ([`CLOSE`]).
//!
//! A modal reads these through its own key table, naming each for what it
//! does there — `pub const SPAN: Key = nav::SIDEWAYS.doing("span")` — so
//! its hints and its handler stay one table ([`crate::hints`]) and the
//! chords stay here. `tests::modals_take_their_moves_from_here` fails on a
//! table that spells `tab`, `shift+tab`, `left` or `right` itself; the
//! few places that keep a key of their own for a reason are listed there
//! with the reason.

use crossterm::event::{KeyCode, KeyEvent};

use crate::hints::Key;

/// The next panel.
pub const NEXT_PANEL: Key = Key::new(&["tab"], "next panel");
/// The panel before.
pub const PREV_PANEL: Key = Key::new(&["shift+tab"], "previous panel");
/// Either way round — one hint for both.
pub const PANELS: Key = Key::new(&["tab", "shift+tab"], "panel");
/// Sideways inside the panel with the keys.
pub const SIDEWAYS: Key = Key::new(&["left", "right"], "tabs").show(2);
/// A modal's tabs from any of its panels, while a filter's caret has the
/// plain arrows ([`side_anywhere`]).
pub const SIDEWAYS_ANYWHERE: Key = Key::new(&["shift+left", "shift+right"], "tabs").show(2);
/// A modal's tabs over a filtered list: `←`/`→`, and `⇧←`/`⇧→` while the
/// filter's caret has the plain arrows — the LINEAR VIEW's and the TODOS
/// MODAL's.
pub const TABS: Key =
    Key::new(&["left", "right", "shift+left", "shift+right"], "tabs").show(2);
/// The rows of the panel with the keys.
pub const ROWS: Key = Key::new(&["up", "down"], "move").show(2);
/// A page up or down.
pub const PAGE: Key = Key::new(&["pgup", "pgdn"], "page").show(2);
/// The first row and the last.
pub const ENDS: Key = Key::new(&["home", "end"], "ends").show(2);
/// Act on the row under the cursor.
pub const OPEN: Key = Key::new(&["enter"], "open");
/// Back a level, and from the top the way out of every modal.
pub const CLOSE: Key = Key::new(&["esc"], "close");

/// Whether `key` is [`CLOSE`] — every modal's Esc arm asks this, so the
/// way back is spelled once.
pub fn closes(key: &KeyEvent) -> bool {
    CLOSE.matches(key)
}

/// `Tab` (true) or `⇧Tab` (false) — however the terminal spelled the
/// shifted one.
pub fn panel(key: &KeyEvent) -> Option<bool> {
    if NEXT_PANEL.matches(key) {
        Some(true)
    } else if PREV_PANEL.matches(key) {
        Some(false)
    } else {
        None
    }
}

/// Plain `←` (-1) or `→` (1).
pub fn side(key: &KeyEvent) -> Option<i32> {
    SIDEWAYS.matches(key).then(|| arrow(key))
}

/// `⇧←` (-1) or `⇧→` (1).
pub fn side_anywhere(key: &KeyEvent) -> Option<i32> {
    SIDEWAYS_ANYWHERE.matches(key).then(|| arrow(key))
}

/// Plain `↑` (-1) or `↓` (1).
pub fn row(key: &KeyEvent) -> Option<i32> {
    ROWS.matches(key).then(|| arrow(key))
}

fn arrow(key: &KeyEvent) -> i32 {
    match key.code {
        KeyCode::Left | KeyCode::Up => -1,
        _ => 1,
    }
}

/// The panel after `at` in `order` — or the one before — round either
/// end: what `Tab` / `⇧Tab` hand the keys to. `at` missing from `order`
/// starts it from the first.
pub fn cycle<T: Copy + PartialEq>(order: &[T], at: T, forward: bool) -> T {
    let Some(&first) = order.first() else {
        return at;
    };
    let Some(i) = order.iter().position(|p| *p == at) else {
        return first;
    };
    let n = order.len();
    order[if forward { (i + 1) % n } else { (i + n - 1) % n }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn every_key_parses() {
        for key in [
            NEXT_PANEL,
            PREV_PANEL,
            PANELS,
            SIDEWAYS,
            SIDEWAYS_ANYWHERE,
            TABS,
            ROWS,
            PAGE,
            ENDS,
            OPEN,
            CLOSE,
        ] {
            assert!(key.parses(), "{:?}", key.chords);
        }
    }

    /// `⇧Tab` is the panel before however it arrives — BackTab, or Tab
    /// with shift under the kitty protocol — and a shifted arrow is not a
    /// plain one.
    #[test]
    fn the_moves_read_from_the_keys() {
        let none = KeyModifiers::NONE;
        let shift = KeyModifiers::SHIFT;
        assert_eq!(panel(&ev(KeyCode::Tab, none)), Some(true));
        assert_eq!(panel(&ev(KeyCode::BackTab, none)), Some(false));
        assert_eq!(panel(&ev(KeyCode::BackTab, shift)), Some(false));
        assert_eq!(panel(&ev(KeyCode::Tab, shift)), Some(false));
        assert_eq!(side(&ev(KeyCode::Left, none)), Some(-1));
        assert_eq!(side(&ev(KeyCode::Right, none)), Some(1));
        assert_eq!(side(&ev(KeyCode::Right, shift)), None);
        assert_eq!(side_anywhere(&ev(KeyCode::Right, shift)), Some(1));
        assert_eq!(side_anywhere(&ev(KeyCode::Right, none)), None);
        assert_eq!(row(&ev(KeyCode::Up, none)), Some(-1));
        assert_eq!(row(&ev(KeyCode::Down, none)), Some(1));
        assert_eq!(row(&ev(KeyCode::Down, shift)), None);
        assert_eq!(SIDEWAYS.doing("span").label(), "←→");
        assert_eq!(PANELS.label(), "Tab");
    }

    #[test]
    fn cycle_goes_round_either_end() {
        let order = ['a', 'b', 'c'];
        assert_eq!(cycle(&order, 'a', true), 'b');
        assert_eq!(cycle(&order, 'c', true), 'a');
        assert_eq!(cycle(&order, 'a', false), 'c');
        assert_eq!(cycle(&order, 'z', true), 'a');
        assert_eq!(cycle(&['a', 'b'], 'b', false), 'a');
    }

    /// Where a key table may still spell a move's chord itself, and why.
    /// Anything else that does is a modal walking its own way: read the
    /// move from this module instead.
    const OWN_KEYS: &[(&str, &str, &str)] = &[
        // The QUICK PROMPT is a text box: Tab and ⇧Tab are Claude Code's
        // own (the harness, the mode), as in the CLIs it launches.
        ("ui.rs", "AGENT", "the quick prompt's harness, as in Claude Code"),
        ("ui.rs", "MODE", "the quick prompt's mode, as in Claude Code"),
        // A page of prose: Tab walks its links, as a browser does.
        ("ui.rs", "LINK", "the markdown page's links, as in a browser"),
        // A path being typed: a shell's completion and directory walk.
        ("ui.rs", "DIVE", "the path prompt's directory walk"),
        ("ui.rs", "ASCEND", "the path prompt's directory walk"),
        ("ui.rs", "COMPLETE", "the path prompt's completion, as in a shell"),
        ("open_repo.rs", "BROWSE", "the open-project prompt's path browser"),
        // A menu's submenu is the same list a level down, not a panel.
        ("ui.rs", "INTO", "a menu's submenu"),
        ("ui.rs", "BACK", "a menu's submenu"),
        // One panel under a tab strip: Tab is the big move, the next tab,
        // and ←/→ the row's value or the strip's own.
        ("ui.rs", "FOLD", "the settings overlay: one panel, ←/→ fold"),
        ("ui.rs", "NEXT_TAB", "the settings overlay: one panel, Tab the next tab"),
        ("ui.rs", "PREV_TAB", "the settings overlay: one panel, ⇧Tab the tab before"),
        ("ui.rs", "CYCLE", "the settings overlay: one panel, ←/→ the row's value"),
        ("file_tabs.rs", "SWITCH", "the file tabs: one panel under its strip"),
        ("conflicts.rs", "FILE", "the conflict resolver: one panel, Tab the next file"),
        ("onboard.rs", "NEXT_PAGE", "first-run setup's pages, a wizard"),
        ("onboard.rs", "PREV_PAGE", "first-run setup's pages, a wizard"),
        // The grid's session pane, not a modal: the grid has the arrows.
        ("pr_preview.rs", "NEXT_TAB", "the pane's pull request page: the grid has the arrows"),
        ("pr_preview.rs", "PREV_TAB", "the pane's pull request page: the grid has the arrows"),
        ("pr_preview.rs", "TABS", "the pane's pull request page: the grid has the arrows"),
        // A form's fields: Tab the next field, ←/→ (and Space) its value,
        // Space beside them.
        ("pr_actions.rs", "FIELD", "a form: Tab the next field, ↓ too"),
        ("pr_actions.rs", "MODE", "a form row: ←/→ or Space"),
        ("pr_actions.rs", "CHANGE", "a form row: ←/→ or Space"),
        ("preset_overlays.rs", "FIELD", "a form: Tab the next field, ↓ too"),
        ("preset_overlays.rs", "WORKTREE", "the preset picker's own Tab"),
        // The LINEAR VIEW's new-issue box: Tab its team.
        ("linear.rs", "TEAM", "the new-issue box: Tab the next team"),
    ];

    /// The chords that are this module's to spell.
    const MOVES: &[&str] = &[
        "\"tab\"",
        "\"shift+tab\"",
        "\"left\"",
        "\"right\"",
        "\"esc\"",
    ];

    /// The files that turn keys into bytes or names, not into moves: a
    /// terminal's encoding, the keymap's parser, the key-combo display.
    const ENCODERS: &[&str] = &[
        "keys.rs",
        "keymap.rs",
        "ghostty_config.rs",
        "key_combo.rs",
        "perf.rs",
    ];

    /// Every key table in the crate takes `Tab`, `⇧Tab`, `←`, `→` and
    /// `Esc` from here, but for the ones [`OWN_KEYS`] names; and no key
    /// handler asks for a bare `KeyCode::Esc` — it asks [`closes`].
    #[test]
    fn modals_take_their_moves_from_here() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stray = Vec::new();
        let mut seen = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if !name.ends_with(".rs") || name == "nav.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                // A handler's own Esc check, outside the tests: `closes`.
                if !ENCODERS.contains(&name.as_str()) && name != "tests.rs" {
                    // Up to the file's test module, however it is spelled.
                    let lines: Vec<&str> = text.lines().collect();
                    let live = lines
                        .windows(2)
                        .position(|w| {
                            w[0].trim() == "#[cfg(test)]"
                                && w[1].trim_start().trim_start_matches("pub(crate) ").starts_with("mod tests")
                        })
                        .unwrap_or(lines.len());
                    for (i, line) in lines[..live].iter().enumerate() {
                        // Making an Esc to send on is not asking for one.
                        if line.contains("KeyCode::Esc") && !line.contains("KeyEvent::new(KeyCode::Esc") {
                            stray.push(format!(
                                "{}:{} asks for KeyCode::Esc — use nav::closes",
                                path.display(),
                                i + 1
                            ));
                        }
                    }
                }
                // A table's `const NAME: Key = Key::new(&[…], …)`, the
                // chords on the same line or the next.
                let lines: Vec<&str> = text.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    let Some(at) = line.find("const ") else {
                        continue;
                    };
                    let decl = &line[at + 6..];
                    let Some((konst, rest)) = decl.split_once(':') else {
                        continue;
                    };
                    if !rest.trim_start().starts_with("Key =") {
                        continue;
                    }
                    // Up to the statement's `;`, a line or two on.
                    let mut body = rest.to_string();
                    for next in lines.iter().skip(i + 1).take(3) {
                        if body.contains(';') {
                            break;
                        }
                        body.push_str(next);
                    }
                    let body = body.split(';').next().unwrap_or("");
                    if !body.contains("Key::new(") || !MOVES.iter().any(|m| body.contains(m)) {
                        continue;
                    }
                    let konst = konst.trim();
                    if OWN_KEYS.iter().any(|(f, k, _)| *f == name && *k == konst) {
                        seen.push((name.clone(), konst.to_string()));
                    } else {
                        stray.push(format!("{}:{} {konst}", path.display(), i + 1));
                    }
                }
            }
        }
        assert!(
            stray.is_empty(),
            "these spell Tab, ⇧Tab, ←, → or Esc themselves — take them from \
             crate::nav (`nav::PANELS.doing(\"…\")`, `nav::CLOSE.also(&[\"q\"])`, \
             `nav::closes(&key)`), or list the key in nav::tests::OWN_KEYS \
             with why:\n{}",
            stray.join("\n")
        );
        for (file, konst, _) in OWN_KEYS {
            assert!(
                seen.iter().any(|(f, k)| f == file && k == konst),
                "{file} {konst} is in OWN_KEYS but spells none of the moves now: take it off"
            );
        }
    }
}
