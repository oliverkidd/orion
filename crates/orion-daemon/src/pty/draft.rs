//! An UNSENT DRAFT: text typed into an agent's input box and not yet sent.
//!
//! It lives only in the CLI's memory, so the IDLE REAPER stopping the
//! process loses it, and the agent it revives opens on an empty box. Notes
//! parked in a box to pick up later were lost this way. So before an idle
//! agent is reaped, its screen is read for one: the ring replayed into a
//! screen at the PTY's size, the same replay an attaching client parses.
//!
//! The agent CLIs draw their input box alike: a row near the foot of the
//! screen opening on a prompt glyph and a space, each further line of the
//! draft on a row of its own beneath, and the box ending at a rule, a
//! half-block edge or a blank row:
//!
//! ```text
//! ──────────────────────      (Claude Code: `❯`, `>` in older builds)
//! ❯ some notes here
//!   second line
//! ──────────────────────
//!
//! › some notes here           (Codex: `›`, the box ending in a blank row)
//!   second line
//!
//! ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄      (Cursor: `→`, on a tinted bar)
//!  → some notes here
//!    second line
//! ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
//! ```
//!
//! `!` opens the row in Claude's and Cursor's shell mode, `&` in Cursor's
//! cloud mode. The box is the lowest such row: a past prompt in the
//! transcript above opens on the same glyph.
//!
//! An empty box shows a placeholder (`Try "refactor foo"`, `Ask Codex to do
//! anything`) drawn dim, and so is a suggestion trailing the caret; Cursor
//! draws its caret over the placeholder's first letter in inverse video,
//! as every CLI draws a caret of its own. So a draft is any text in the box
//! that is neither dim nor inverse. A screen with no such box (a harness
//! laid out another way) holds no draft as far as this can tell, and is
//! reaped as before. A box this misreads as holding text only keeps one
//! idle CLI running; the other way round loses what was typed.

use super::cursor::grid_size;

/// Glyphs an input box's first row opens on.
const PROMPT_GLYPHS: [&str; 6] = ["❯", ">", "›", "→", "!", "&"];

/// How far in from the left edge a box's glyph may sit (Cursor's bar
/// insets it by one).
const GLYPH_MAX_COL: u16 = 3;

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
    let Some((first, glyph_col)) = (0..rows)
        .rev()
        .find_map(|row| prompt_glyph_at(screen, row).map(|col| (row, col)))
    else {
        return false;
    };
    let last = (first + 1..rows)
        .find(|&row| ends_box(screen, row, cols))
        .unwrap_or(rows);
    (first..last).any(|row| {
        // The first row's glyph and the space after it are the box's own.
        let from = if row == first { glyph_col + 2 } else { 0 };
        (from..cols).any(|col| screen.cell(row, col).is_some_and(typed))
    })
}

/// The column of the prompt glyph `row` opens on, if it does: the row's
/// first drawn cell, near the left edge, followed by a space.
fn prompt_glyph_at(screen: &vt100::Screen, row: u16) -> Option<u16> {
    let col = (0..=GLYPH_MAX_COL).find(|&col| {
        screen
            .cell(row, col)
            .is_some_and(|cell| !cell.contents().trim().is_empty())
    })?;
    let glyph = screen.cell(row, col)?.contents();
    let spaced = screen
        .cell(row, col + 1)
        .is_some_and(|cell| cell.contents().trim().is_empty());
    (PROMPT_GLYPHS.contains(&glyph) && spaced).then_some(col)
}

/// A row that closes the box: blank, or drawn as a rule or a half-block
/// edge (`─`, `▀`, `▄` at least half its drawn cells, a label allowed).
fn ends_box(screen: &vt100::Screen, row: u16, cols: u16) -> bool {
    let drawn: Vec<&str> = (0..cols)
        .filter_map(|col| screen.cell(row, col))
        .map(vt100::Cell::contents)
        .filter(|s| !s.trim().is_empty())
        .collect();
    let edge = drawn
        .iter()
        .filter(|s| matches!(**s, "─" | "▀" | "▄"))
        .count();
    drawn.is_empty() || (edge >= 3 && edge * 2 >= drawn.len())
}

/// A cell holding something the user typed: drawn, and neither dim (a
/// placeholder, a suggestion) nor inverse (a caret).
fn typed(cell: &vt100::Cell) -> bool {
    !cell.dim() && !cell.inverse() && !cell.contents().trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = "────────────────────────────────────────";

    /// A Claude Code screen 40 wide: the alternate screen, a transcript
    /// line, then the box holding `box_rows` and the footer under it.
    fn claude_screen(box_rows: &[&str]) -> Vec<u8> {
        let mut out = String::from("\x1b[?1049h\x1b[H\x1b[2J");
        out.push_str("❯ an earlier prompt, sent\r\n\r\n");
        out.push_str(&format!("{RULE}\r\n"));
        for row in box_rows {
            out.push_str(&format!("{row}\r\n"));
        }
        out.push_str(&format!("{RULE}\r\n"));
        out.push_str("  ⏵⏵ auto mode on (shift+tab to cycle)");
        out.into_bytes()
    }

    /// A Codex screen: a past prompt, the box ending in a blank row, then
    /// its footer.
    fn codex_screen(box_rows: &[&str]) -> Vec<u8> {
        let mut out = String::from("\x1b[?1049h\x1b[H\x1b[2J");
        out.push_str("› an earlier prompt, sent\r\n\r\n• done\r\n\r\n");
        for row in box_rows {
            out.push_str(&format!("{row}\r\n"));
        }
        out.push_str("\r\n  gpt default · ~/repo\r\n  ← for agents · ? for shortcuts");
        out.into_bytes()
    }

    /// A Cursor screen (cursor-agent 2026.08.31): the box on a bar between
    /// half-block edges, its glyph inset by one and dim while the box is
    /// empty, the model and checkout under it.
    fn cursor_screen(box_rows: &[&str]) -> Vec<u8> {
        let bar = "▄".repeat(40);
        let foot = "▀".repeat(40);
        let mut out = String::from("\x1b[H\x1b[2J");
        out.push_str(" → an earlier prompt, sent\r\n\r\n");
        out.push_str(&format!("{bar}\r\n"));
        for row in box_rows {
            out.push_str(&format!(" {row}\r\n"));
        }
        out.push_str(&format!("{foot}\r\n"));
        out.push_str("  \x1b[2mComposer 2.5\x1b[22m\r\n  \x1b[2m~/repo · main\x1b[22m");
        out.into_bytes()
    }

    fn holds(bytes: &[u8]) -> bool {
        replay_holds_draft(40, 24, bytes)
    }

    #[test]
    fn typed_text_in_the_box_is_a_draft() {
        assert!(holds(&claude_screen(&["❯ some notes here"])));
        assert!(holds(&codex_screen(&["› some notes here"])));
        assert!(holds(&cursor_screen(&["→ some notes here"])));
    }

    #[test]
    fn a_draft_on_a_later_line_counts() {
        assert!(holds(&claude_screen(&["❯ ", "  second line"])));
        assert!(holds(&codex_screen(&["› ", "  second line"])));
        assert!(holds(&cursor_screen(&["→ ", "  second line"])));
    }

    #[test]
    fn an_empty_box_holds_none() {
        assert!(!holds(&claude_screen(&["❯ "])));
        assert!(!holds(&codex_screen(&["› "])));
        assert!(!holds(&cursor_screen(&["\x1b[2m→\x1b[22m "])));
    }

    /// The placeholder an empty box shows is drawn dim.
    #[test]
    fn the_dim_placeholder_is_no_draft() {
        assert!(!holds(&claude_screen(&[
            "❯ \x1b[2mTry \"refactor launcher.rs\"\x1b[22m"
        ])));
        assert!(!holds(&codex_screen(&[
            "› \x1b[2mAsk Codex to do anything\x1b[22m"
        ])));
    }

    /// Cursor draws its caret over the placeholder's first letter.
    #[test]
    fn a_caret_over_the_placeholder_is_no_draft() {
        assert!(!holds(&cursor_screen(&[
            "\x1b[2m→\x1b[22m \x1b[7mP\x1b[27m\x1b[2mlan, search, build anything\x1b[22m"
        ])));
    }

    /// A caret after typed text leaves the text a draft.
    #[test]
    fn typed_text_before_the_caret_counts() {
        assert!(holds(&cursor_screen(&["→ notes\x1b[7m \x1b[27m"])));
    }

    /// A suggestion trailing typed text is dim; the typed text still counts.
    #[test]
    fn typed_text_before_a_dim_suggestion_counts() {
        assert!(holds(&claude_screen(&["❯ /re\x1b[2mview\x1b[22m"])));
    }

    #[test]
    fn older_builds_and_shell_modes_open_on_their_own_glyphs() {
        assert!(holds(&claude_screen(&["> notes"])));
        assert!(holds(&claude_screen(&["! ls -la"])));
        assert!(holds(&cursor_screen(&["& move this to the cloud"])));
    }

    /// A past prompt above an empty box is the transcript's, not a draft.
    #[test]
    fn the_lowest_box_is_the_one_read() {
        let mut bytes = String::from("\x1b[H\x1b[2J");
        bytes.push_str(&format!("{RULE}\r\n❯ old text\r\n{RULE}\r\n"));
        bytes.push_str(&format!("{RULE}\r\n❯ \r\n{RULE}\r\n"));
        assert!(!holds(bytes.as_bytes()));
    }

    /// The box ends at its rule or blank row: the footer under it is not
    /// the draft.
    #[test]
    fn the_footer_under_the_box_is_no_draft() {
        let mut bytes = String::from("\x1b[H\x1b[2J");
        bytes.push_str("› \r\n\r\n  footer text\r\n");
        assert!(!holds(bytes.as_bytes()));
    }

    #[test]
    fn a_screen_with_no_box_holds_none() {
        assert!(!holds(b"$ ls\r\nCargo.toml  crates\r\n$ "));
        assert!(!holds(b""));
    }

    /// A glyph run into a word, or far in from the edge, opens no box.
    #[test]
    fn a_glyph_without_its_space_or_far_in_opens_no_box() {
        assert!(!holds("->arrow text".as_bytes()));
        assert!(!holds("        → indented text".as_bytes()));
    }

    /// A squeezed pane must not panic the reaper.
    #[test]
    fn a_tiny_screen_does_not_panic() {
        assert!(!replay_holds_draft(0, 0, &claude_screen(&["❯ notes"])));
    }
}
