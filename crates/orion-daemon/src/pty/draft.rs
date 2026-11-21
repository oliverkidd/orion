//! An UNSENT DRAFT: text typed into an agent's input box and not yet sent.
//!
//! It lives only in the CLI's memory, so the IDLE REAPER stopping the
//! process loses it, and the agent it revives opens on an empty box. Notes
//! parked in a box to pick up later were lost this way. So before an idle
//! agent is reaped, its screen is read for one: the ring replayed into a
//! screen at the PTY's size, the same replay an attaching client parses.
//!
//! Claude Code's box sits between two rules (`────`) at the foot of its
//! screen, its first row opening on the prompt glyph (`❯`, `>` in older
//! builds, `!` in bash mode) and each further line of the draft on a row of
//! its own:
//!
//! ```text
//! ────────────────────────
//! ❯ some notes here
//!   second line
//! ────────────────────────
//!   ⏵⏵ auto mode on
//! ```
//!
//! An empty box shows a placeholder (`Try "refactor foo"`) drawn dim, and
//! so does a suggestion trailing the caret, so a draft is any text in the
//! box that is not dim. A screen with no such box (a dialog, a harness laid
//! out another way) holds no draft as far as this can tell, and is reaped
//! as before.

use super::cursor::grid_size;

/// Glyphs Claude Code opens its input box's first row on.
const PROMPT_GLYPHS: [&str; 3] = ["❯", ">", "!"];

/// Does a screen at `cols`×`rows` with `history` replayed into it show
/// text typed into an input box and not yet sent?
pub fn replay_holds_draft(cols: u16, rows: u16, history: &[u8]) -> bool {
    let (cols, rows) = grid_size(cols, rows);
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(history);
    screen_holds_draft(parser.screen())
}

/// Does this screen show text typed into an input box and not yet sent?
fn screen_holds_draft(screen: &vt100::Screen) -> bool {
    let (rows, cols) = screen.size();
    let rules: Vec<u16> = (0..rows).filter(|&r| is_rule(screen, r, cols)).collect();
    // The box is the lowest pair of rules with rows between them.
    let Some((top, bottom)) = rules
        .windows(2)
        .rev()
        .map(|pair| (pair[0], pair[1]))
        .find(|(top, bottom)| bottom - top >= 2)
    else {
        return false;
    };
    let first = top + 1;
    let opens_on_prompt = screen
        .cell(first, 0)
        .is_some_and(|cell| PROMPT_GLYPHS.contains(&cell.contents()));
    if !opens_on_prompt {
        return false;
    }
    (first..bottom).any(|row| {
        // The first row's glyph and the space after it are the box's own.
        let from = if row == first { 2 } else { 0 };
        (from..cols).any(|col| {
            screen
                .cell(row, col)
                .is_some_and(|cell| !cell.dim() && !cell.contents().trim().is_empty())
        })
    })
}

/// A row drawn as a rule: it opens and closes on `─`, and is at least half
/// rule (a rule may carry a label, as plan mode's does).
fn is_rule(screen: &vt100::Screen, row: u16, cols: u16) -> bool {
    let drawn: Vec<&str> = (0..cols)
        .filter_map(|col| screen.cell(row, col))
        .map(vt100::Cell::contents)
        .filter(|s| !s.trim().is_empty())
        .collect();
    let rule = drawn.iter().filter(|s| **s == "─").count();
    drawn.first() == Some(&"─")
        && drawn.last() == Some(&"─")
        && rule >= 3
        && rule * 2 >= drawn.len()
        && rule * 2 >= usize::from(cols) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = "────────────────────────────────────────";

    /// A Claude Code screen 40 wide: the alternate screen, a transcript
    /// line, then the box holding `box_rows` and the footer under it.
    fn claude_screen(box_rows: &[&str]) -> Vec<u8> {
        let mut out = String::from("\x1b[?1049h\x1b[H\x1b[2J");
        out.push_str("> an earlier prompt, sent\r\n\r\n");
        out.push_str(&format!("{RULE}\r\n"));
        for row in box_rows {
            out.push_str(&format!("{row}\r\n"));
        }
        out.push_str(&format!("{RULE}\r\n"));
        out.push_str("  ⏵⏵ auto mode on (shift+tab to cycle)");
        out.into_bytes()
    }

    fn holds(bytes: &[u8]) -> bool {
        replay_holds_draft(40, 24, bytes)
    }

    #[test]
    fn typed_text_in_the_box_is_a_draft() {
        assert!(holds(&claude_screen(&["❯ some notes here"])));
    }

    #[test]
    fn a_draft_on_a_later_line_counts() {
        assert!(holds(&claude_screen(&["❯ ", "  second line"])));
    }

    #[test]
    fn an_empty_box_holds_none() {
        assert!(!holds(&claude_screen(&["❯ "])));
    }

    /// The placeholder an empty box shows is drawn dim.
    #[test]
    fn the_dim_placeholder_is_no_draft() {
        assert!(!holds(&claude_screen(&[
            "❯ \x1b[2mTry \"refactor launcher.rs\"\x1b[22m"
        ])));
    }

    /// A suggestion trailing typed text is dim; the typed text still counts.
    #[test]
    fn typed_text_before_a_dim_suggestion_counts() {
        assert!(holds(&claude_screen(&["❯ /re\x1b[2mview\x1b[22m"])));
    }

    #[test]
    fn older_builds_and_bash_mode_open_on_their_own_glyphs() {
        assert!(holds(&claude_screen(&["> notes"])));
        assert!(holds(&claude_screen(&["! ls -la"])));
    }

    /// Between two rules but not an input box: a dialog's text is no draft.
    #[test]
    fn rules_around_anything_but_a_prompt_hold_none() {
        assert!(!holds(&claude_screen(&["Do you want to proceed?"])));
    }

    #[test]
    fn a_screen_with_no_box_holds_none() {
        assert!(!holds(b"$ ls\r\nCargo.toml  crates\r\n$ "));
        assert!(!holds(b""));
    }

    /// A short run of `─` inside text is not a rule.
    #[test]
    fn a_dash_run_inside_text_is_no_rule() {
        let bytes = "──── notes ────\r\n❯ text\r\n──── more ────".as_bytes();
        assert!(!holds(bytes));
    }

    /// A rule may carry a label, as plan mode's does.
    #[test]
    fn a_labelled_rule_still_bounds_the_box() {
        let mut bytes = String::from("\x1b[H\x1b[2J");
        bytes.push_str("──────────────── plan ────────────────\r\n");
        bytes.push_str("❯ notes\r\n");
        bytes.push_str(&format!("{RULE}\r\n"));
        assert!(holds(bytes.as_bytes()));
    }

    /// The box is the lowest one: rules higher up (a transcript's own) are
    /// not it.
    #[test]
    fn the_lowest_box_is_the_one_read() {
        let mut bytes = String::from("\x1b[H\x1b[2J");
        bytes.push_str(&format!("{RULE}\r\n❯ old text\r\n{RULE}\r\n"));
        bytes.push_str(&format!("{RULE}\r\n❯ \r\n{RULE}\r\n"));
        assert!(!holds(bytes.as_bytes()));
    }

    /// A squeezed pane must not panic the reaper.
    #[test]
    fn a_tiny_screen_does_not_panic() {
        assert!(!replay_holds_draft(0, 0, &claude_screen(&["❯ notes"])));
    }
}
