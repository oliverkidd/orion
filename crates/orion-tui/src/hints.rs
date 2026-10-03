//! KEY HINTS: the one way orion puts "this key does that" on screen.
//!
//! The convention (docs/keys.md, "How the screen is laid out"):
//!
//! * A MODAL's keys are on its bottom border, drawn by [`modal_block`] —
//!   `╰ Enter launch · ⇧Enter newline · Esc cancel ─╯` — and nowhere
//!   else. When they don't fit, whole hints go from the right end, the
//!   modal's first verb and its way out last ([`fit`]); a hint is never
//!   cut mid-word.
//! * The FOOTER carries hints only while no modal is up, and then about
//!   the thing the grid or the pane has under its cursor ([`footer_spans`]).
//! * A modal's explanation of the row or setting under its cursor is one
//!   dim line inside the frame, right above the bottom border
//!   ([`explain_area`] / [`draw_explain`]).
//!
//! Every key label comes from one formatter, [`KeyChord::display`]: a
//! registry action's from the live keymap ([`act`] — a rebind shows, an
//! unbound action drops out, a terminal without ⌘ sees the `^` twin), a
//! modal's own keys from the small [`Key`] table its key handler matches
//! against, so a hint can never name a key the modal does not answer to.

use crate::keymap::{Action, KeyChord, Keymap};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

/// What separates two hints.
pub const SEP: &str = " · ";

/// One hint: a key's label and what it does there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub key: String,
    pub does: String,
    /// Kept when the line is too narrow for every hint: a modal's way out
    /// and its main verb. The rest go from the right end first.
    pub keep: bool,
}

impl Hint {
    /// A hint for `key` (already a label) doing `does`. Esc is kept
    /// whatever the width: it is the way back.
    pub fn new(key: impl Into<String>, does: impl Into<String>) -> Self {
        let key = key.into();
        let keep = key.starts_with("Esc");
        Self {
            key,
            does: does.into(),
            keep,
        }
    }

    /// The same hint, kept however narrow the line gets.
    pub fn kept(mut self) -> Self {
        self.keep = true;
        self
    }

    /// The width of `key does`, in cells.
    pub fn width(&self) -> usize {
        self.key.chars().count() + 1 + self.does.chars().count()
    }
}

/// The label of a list of chords: the first `show` of the side this
/// terminal can press ([`Keymap::shown_chords`]'s rule), arrows run
/// together (`↑↓`), the rest joined by `/` — `Enter/y`, `PgUp/PgDn`.
fn label_of(chords: &[KeyChord], show: usize) -> String {
    let mut shown = crate::keymap::shown_side_of(chords);
    // A terminal that sends no ⌘ speaks no kitty protocol either: a chord
    // only that protocol carries (`⇧Enter`) gives way to its twin (`^J`).
    if !crate::keymap::cmd_shown() && shown.len() > 1 {
        let plain: Vec<KeyChord> = shown
            .iter()
            .copied()
            .filter(|c| crate::keymap::host_warning(c).0.is_fine())
            .collect();
        if !plain.is_empty() {
            shown = plain;
        }
    }
    shown.truncate(show.max(1));
    let mut out = String::new();
    let mut prev_arrow = false;
    for (i, chord) in shown.iter().enumerate() {
        let arrow = chord.mods.is_empty()
            && matches!(
                chord.code,
                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
            );
        if i > 0 && !(arrow && prev_arrow) {
            out.push('/');
        }
        out.push_str(&chord.display());
        prev_arrow = arrow;
    }
    out
}

/// The key label of a registry action: its first chord on the side this
/// terminal sends, as the keymap has it now. None when unbound.
pub fn key(keymap: &Keymap, action: Action) -> Option<String> {
    keymap.shown_first(action).map(|c| c.display())
}

/// [`key`], or `fallback` for an action bound to nothing — for prose
/// that has to name a way to do the thing either way.
pub fn key_or(keymap: &Keymap, action: Action, fallback: &str) -> String {
    key(keymap, action).unwrap_or_else(|| fallback.to_string())
}

/// `text` with every `{action_id}` in it — an action of the keymap, by
/// the id its config row has (`{palette}`, `{open_outside}`) — replaced by
/// that action's key as the live keymap has it, or by its name where it
/// has none. How a sentence of explanation — a setting's, an action's —
/// names a key without typing one that a rebind would make a lie. A brace
/// that names no action is left as it is.
pub fn expand(text: &str, keymap: &Keymap) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let named = after.find('}').and_then(|close| {
            let id = &after[..close];
            crate::keymap::ACTIONS
                .iter()
                .find(|spec| spec.id == id)
                .map(|spec| (close, spec))
        });
        match named {
            Some((close, spec)) => {
                out.push_str(&key(keymap, spec.action).unwrap_or_else(|| spec.label.to_string()));
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A hint for a registry action, spelled from the live keymap. None when
/// the action is unbound: the COMMAND PALETTE reaches it, and a hint
/// never names a key that does nothing.
pub fn act(keymap: &Keymap, action: Action, does: &str) -> Option<Hint> {
    key(keymap, action).map(|key| Hint::new(key, does))
}

/// A hint for two or more registry actions read as one — `←→ step`,
/// `[] project tabs` — each action's first chord, run together.
pub fn acts(keymap: &Keymap, actions: &[Action], does: &str) -> Option<Hint> {
    let chords: Vec<KeyChord> = actions
        .iter()
        .filter_map(|a| keymap.shown_first(*a))
        .collect();
    if chords.len() != actions.len() {
        return None;
    }
    let all_arrows = chords.iter().all(|c| {
        c.mods.is_empty()
            && matches!(
                c.code,
                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
            )
    });
    let sep = if all_arrows || chords.iter().all(|c| c.display().chars().count() == 1) {
        ""
    } else {
        "/"
    };
    let key = chords
        .iter()
        .map(KeyChord::display)
        .collect::<Vec<_>>()
        .join(sep);
    Some(Hint::new(key, does))
}

/// A key a modal answers to on its own — not a registry action, so not
/// rebindable — in the one table its key handler matches with
/// [`Key::matches`] and its hint line spells with [`Key::hint`]. The
/// chords are config spellings (`"ctrl+r"`, `"cmd+o"`, `"esc"`), parsed
/// by [`KeyChord::parse`] like the keymap's defaults.
#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub chords: &'static [&'static str],
    pub does: &'static str,
    /// How many of the chords the label names — the first, unless
    /// [`Key::show`] says more (`↑↓`, `Enter/y`). The rest still match.
    show: usize,
}

impl Key {
    pub const fn new(chords: &'static [&'static str], does: &'static str) -> Self {
        Self {
            chords,
            does,
            show: 1,
        }
    }

    /// The label names the first `n` chords, not just the first.
    pub const fn show(mut self, n: usize) -> Self {
        self.show = n;
        self
    }

    /// Its chords, parsed. A spelling that does not parse is a bug the
    /// tables' tests catch ([`Key::parses`]).
    pub fn chords(&self) -> Vec<KeyChord> {
        self.chords
            .iter()
            .filter_map(|s| KeyChord::parse(s))
            .collect()
    }

    /// Every chord parses — what each modal's table test asserts.
    pub fn parses(&self) -> bool {
        self.chords.iter().all(|s| KeyChord::parse(s).is_some())
    }

    /// Whether `key` is one of this key's chords, however the terminal
    /// spelled it ([`KeyChord::from_event`]).
    pub fn matches(&self, key: &KeyEvent) -> bool {
        let pressed = KeyChord::from_event(key);
        self.chords().contains(&pressed)
    }

    /// Its label: the chords this terminal can press, `⌘C` where ⌘
    /// arrives and `^Y` where it does not.
    pub fn label(&self) -> String {
        label_of(&self.chords(), self.show)
    }

    /// The hint, `label does`.
    pub fn hint(&self) -> Hint {
        Hint::new(self.label(), self.does)
    }

    /// The hint with another verb — a key whose word depends on the row
    /// (`Esc clear` with a filter typed, `Esc close` without).
    pub fn hint_as(&self, does: impl Into<String>) -> Hint {
        Hint::new(self.label(), does)
    }
}

/// Copy the path of the file or skill under the cursor: `⌘C` where the
/// terminal sends ⌘, `^Y` everywhere.
pub const COPY_PATH: Key = Key::new(&["cmd+c", "ctrl+y"], "copy path");
/// Hand the file or folder under the cursor to Cursor.
pub const IN_CURSOR: Key = Key::new(&["cmd+o", "ctrl+o"], "open in app");

/// [`IN_CURSOR`]'s hint, named after the OPEN IN APP editor it opens:
/// `⌘O VS Code`.
pub fn in_app_hint() -> Hint {
    IN_CURSOR.hint_as(crate::outside_editor::hint_name())
}
/// The way out of every modal.
pub const ESC_CLOSE: Key = Key::new(&["esc"], "close");

/// `hints` cut to `width` cells: whole hints dropped from the right end,
/// the kept ones ([`Hint::keep`]) last of all, never a hint cut short.
pub fn fit(hints: &[Hint], width: usize) -> Vec<&Hint> {
    let mut shown: Vec<&Hint> = hints.iter().collect();
    let total = |shown: &[&Hint]| -> usize {
        shown.iter().map(|h| h.width()).sum::<usize>()
            + SEP.chars().count() * shown.len().saturating_sub(1)
    };
    // The extras go first, from the right; then the kept verbs; the way
    // out — Esc — last of all.
    while total(&shown) > width && !shown.is_empty() {
        let drop = shown
            .iter()
            .rposition(|h| !h.keep)
            .or_else(|| shown.iter().rposition(|h| !h.key.starts_with("Esc")))
            .unwrap_or(shown.len() - 1);
        shown.remove(drop);
    }
    shown
}

/// The spans of `hints` fitted to `width`: each key a step brighter than
/// what it does, the separators dimmest.
pub fn spans(hints: &[Hint], width: usize, th: Theme) -> Vec<Span<'static>> {
    let key = Style::default().fg(th.muted);
    let does = Style::default().fg(th.dim);
    let mut out = Vec::new();
    for (i, hint) in fit(hints, width).into_iter().enumerate() {
        if i > 0 {
            out.push(Span::styled(SEP, does));
        }
        out.push(Span::styled(hint.key.clone(), key));
        out.push(Span::styled(format!(" {}", hint.does), does));
    }
    out
}

/// `hints` as plain text, fitted to `width` — for tests and for the
/// rare place a hint is part of a sentence.
pub fn text(hints: &[Hint], width: usize) -> String {
    fit(hints, width)
        .iter()
        .map(|h| format!("{} {}", h.key, h.does))
        .collect::<Vec<_>>()
        .join(SEP)
}

/// The bottom border's hint line for a modal `width` cells wide: the
/// fitted hints with a space of air either side, leaving the corners and
/// a dash of rule at each end.
pub fn border_line(hints: &[Hint], width: u16, th: Theme) -> Line<'static> {
    let room = usize::from(width).saturating_sub(6);
    let mut line = spans(hints, room, th);
    if line.is_empty() {
        return Line::default();
    }
    line.insert(0, Span::raw(" "));
    line.push(Span::raw(" "));
    Line::from(line)
}

/// `block` with `hints` on its bottom border — THE place a modal's keys
/// go. `width` is the modal's outer width.
pub fn modal_block<'a>(block: Block<'a>, hints: &[Hint], width: u16, th: Theme) -> Block<'a> {
    let line = border_line(hints, width, th);
    if line.spans.is_empty() {
        return block;
    }
    block.title_bottom(line)
}

/// `hints` drawn over the bottom border of a modal made of more than one
/// frame — a list beside a reading pane — that has no single block to
/// hang them on: the same line [`modal_block`] puts on one, along the
/// bottom row of `area` (the whole modal), with `reserve` cells kept
/// clear at the right end for what a pane already shows there (its
/// scroll position).
pub fn draw_on_border(f: &mut Frame, area: Rect, hints: &[Hint], reserve: u16, th: Theme) {
    if area.height == 0 || area.width < 8 {
        return;
    }
    let width = area.width.saturating_sub(reserve);
    let line = border_line(hints, width, th);
    if line.spans.is_empty() {
        return;
    }
    // Where a block's own bottom title starts: right after the corner.
    let row = Rect {
        x: area.x + 1,
        y: area.y + area.height - 1,
        width: (line.width() as u16).min(width.saturating_sub(3)),
        height: 1,
    };
    f.render_widget(Paragraph::new(line), row);
}

/// `text` wrapped to a modal's explanation line `width` cells wide: at
/// most `max_rows` rows, the last ending in `…` when there is more.
pub fn explain_lines(text: &str, width: u16, max_rows: u16) -> Vec<String> {
    let room = usize::from(width).saturating_sub(2).max(1);
    let mut lines = crate::pr_preview::wrap(text.trim(), room);
    lines.retain(|l| !l.trim().is_empty());
    let max = usize::from(max_rows.max(1));
    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            *last = crate::ui::truncate(&format!("{last} …"), room);
        }
    }
    lines
}

/// `inner` split into the modal's body and its EXPLANATION — the dim
/// line (two, for a long one, where `max_rows` allows) at the bottom of
/// the frame, right above the bottom border where the keys are. Nothing
/// to say, or no room under a row of body, and the body is all of it.
pub fn explain_area(text: &str, inner: Rect, max_rows: u16) -> (Rect, Rect) {
    let rows = if text.trim().is_empty() || inner.height < 3 {
        0
    } else {
        (explain_lines(text, inner.width, max_rows).len() as u16).min(inner.height - 1)
    };
    let body = Rect {
        height: inner.height - rows,
        ..inner
    };
    let row = Rect {
        y: inner.y + inner.height - rows,
        height: rows,
        ..inner
    };
    (body, row)
}

/// The explanation in the rows [`explain_area`] kept for it: dim, wrapped
/// at the modal's width, the last row ending in `…` when there is more.
pub fn draw_explain(f: &mut Frame, row: Rect, text: &str, th: Theme) {
    draw_explain_as(f, row, text, Style::default().fg(th.dim));
}

/// [`draw_explain`] in another style — a notice in the heads-up color
/// takes the explanation's place for as long as it stands.
pub fn draw_explain_as(f: &mut Frame, row: Rect, text: &str, style: Style) {
    if row.height == 0 || row.width < 2 {
        return;
    }
    let lines: Vec<Line> = explain_lines(text, row.width, row.height)
        .into_iter()
        .map(|l| Line::from(Span::styled(format!(" {l}"), style)))
        .collect();
    // Right above the border, however many of its rows it takes.
    let used = (lines.len() as u16).min(row.height);
    let row = Rect {
        y: row.y + row.height - used,
        height: used,
        ..row
    };
    f.render_widget(Paragraph::new(lines), row);
}

/// For a modal's tests: every key in its `table` parses, and every hint
/// it draws names one of them — or Esc, every modal's way out — so the
/// bottom border can only spell a key the modal's handler matches.
#[cfg(test)]
pub fn assert_hints_from(hints: &[Hint], table: &[Key]) {
    for key in table {
        assert!(key.parses(), "{:?} does not parse", key.chords);
    }
    let labels: Vec<String> = table.iter().map(Key::label).collect();
    for hint in hints {
        assert!(
            hint.key == "Esc" || labels.contains(&hint.key),
            "{} {} is not in the table {labels:?}",
            hint.key,
            hint.does
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn hints(pairs: &[(&str, &str)]) -> Vec<Hint> {
        pairs.iter().map(|(k, d)| Hint::new(*k, *d)).collect()
    }

    #[test]
    fn an_unbound_action_has_no_hint() {
        let keymap = Keymap::default();
        assert_eq!(
            act(&keymap, Action::FindFile, "go to file"),
            Some(Hint::new("^P", "go to file"))
        );
        assert_eq!(act(&keymap, Action::Help, "help"), None);
    }

    #[test]
    fn a_rebind_is_what_the_hint_says() {
        let mut keymap = Keymap::default();
        let index = crate::keymap::index_of(Action::NewTerminal).unwrap();
        keymap.bind(index, KeyChord::parse("ctrl+x").unwrap(), false);
        assert_eq!(key(&keymap, Action::NewTerminal).as_deref(), Some("^X"));
    }

    #[test]
    fn hints_read_key_then_verb_with_dots_between() {
        let line = hints(&[
            ("Enter", "launch"),
            ("⇧Enter", "newline"),
            ("Esc", "cancel"),
        ]);
        assert_eq!(
            text(&line, 80),
            "Enter launch · ⇧Enter newline · Esc cancel"
        );
    }

    /// Too narrow: whole hints go from the right end, the way out last —
    /// never a hint cut mid-word.
    #[test]
    fn a_narrow_line_drops_whole_hints_and_keeps_esc() {
        let line = hints(&[
            ("Enter", "launch"),
            ("Tab", "agent"),
            ("⇧Tab", "preset"),
            ("Esc", "cancel"),
        ]);
        assert_eq!(text(&line, 37), "Enter launch · Tab agent · Esc cancel");
        assert_eq!(text(&line, 26), "Enter launch · Esc cancel");
        assert_eq!(text(&line, 12), "Esc cancel");
        for width in 0..60 {
            let fitted = text(&line, width);
            assert!(fitted.chars().count() <= width || fitted == "Esc cancel");
            for part in fitted.split(SEP).filter(|p| !p.is_empty()) {
                assert!(
                    line.iter().any(|h| format!("{} {}", h.key, h.does) == part),
                    "{part:?} is a whole hint"
                );
            }
        }
    }

    #[test]
    fn a_modal_key_matches_every_spelling_and_shows_the_side_sent() {
        let copy = COPY_PATH;
        assert!(copy.parses());
        let ev = |code, mods| KeyEvent::new(code, mods);
        assert!(copy.matches(&ev(KeyCode::Char('c'), KeyModifiers::SUPER)));
        assert!(copy.matches(&ev(KeyCode::Char('y'), KeyModifiers::CONTROL)));
        assert!(!copy.matches(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        // No ⌘ from this terminal (the test default): the ^ twin.
        assert_eq!(copy.label(), "^Y");
        let confirm = Key::new(&["enter", "y"], "confirm");
        assert_eq!(confirm.label(), "Enter", "the first chord unless told");
        assert_eq!(confirm.show(2).label(), "Enter/y");
        let walk = Key::new(&["up", "down", "ctrl+n"], "move").show(2);
        assert_eq!(walk.label(), "↑↓");
    }

    /// Every modal's bottom border spells keys from the table its handler
    /// matches — and never a key the table does not have.
    #[test]
    fn every_modal_spells_its_keys_from_its_own_table() {
        use orion_core::{ProjectId, WorktreeId};
        let project = || ProjectId("p".into());
        let with = |table: &[Key], extra: &[Key]| {
            let mut all = table.to_vec();
            all.extend_from_slice(extra);
            all
        };
        let newline = crate::ui::task_keys::NEWLINE;

        assert_hints_from(&crate::skills::hints("micro"), crate::skills::keys::ALL);

        let mut issues = crate::issues::IssuesView::new(project(), "p".into(), "/tmp/p".into());
        assert_hints_from(&crate::issues::hints(&issues), crate::issues::keys::ALL);
        issues.query.insert_str("x");
        assert_hints_from(&crate::issues::hints(&issues), crate::issues::keys::ALL);

        let mut prs =
            crate::pr_modal::PullRequestsView::new(project(), "p".into(), "/tmp/p".into());
        for tab in crate::pr_preview::PrTab::ALL {
            prs.tabs.switch(tab);
            assert_hints_from(&crate::pr_modal::hints(&prs), crate::pr_modal::keys::ALL);
        }
        prs.tabs.switch(crate::pr_preview::PrTab::Description);

        let browse = crate::linear::LinearView::new(
            project(),
            "p".into(),
            "/tmp/p".into(),
            crate::linear::LinearMode::Browse,
        );
        assert_hints_from(&crate::linear::hints(&browse), crate::linear::keys::ALL);
        let attach = crate::linear::LinearView::new(
            project(),
            "p".into(),
            "/tmp/p".into(),
            crate::linear::LinearMode::Attach {
                pr_url: "u".into(),
                pr_number: 1,
                back: Box::new(prs),
            },
        );
        assert_hints_from(&crate::linear::hints(&attach), crate::linear::keys::ALL);

        assert_hints_from(
            &crate::branch_switch::hints(&crate::branch_switch::Stage::Pick),
            crate::branch_switch::keys::ALL,
        );

        let presets = crate::preset_overlays::AgentPresetsView::new(
            WorktreeId::from("w".to_string()),
            Vec::new(),
        );
        assert_hints_from(
            &crate::preset_overlays::list_hints(&presets),
            crate::preset_overlays::keys::ALL,
        );
        assert_hints_from(
            &crate::preset_overlays::editor_hints(),
            &with(crate::preset_overlays::keys::ALL, &[newline]),
        );

        let cfg = crate::config::Config::default();
        let mut onboard = crate::onboard::OnboardView::new(&cfg);
        for page in 0..8 {
            onboard.page = page;
            assert_hints_from(
                &crate::onboard::hints(&cfg, &onboard),
                crate::onboard::keys::ALL,
            );
        }

        use crate::ui::prompt_keys::{ASCEND, CANCEL, COMPLETE, DIVE, OK, PICK};
        assert_hints_from(
            &crate::ui::prompt_hints(true),
            &[OK, PICK, DIVE, ASCEND, COMPLETE, CANCEL],
        );
        use crate::ui::task_keys::{AGENT, PRESET, SUBMIT};
        assert_hints_from(
            &crate::ui::task_hints(&crate::app::PromptKind::AddProject),
            &[SUBMIT, newline, AGENT, PRESET],
        );
    }

    /// An explanation names a key by its action — `{palette}` — and the
    /// live keymap spells it, a rebind included; a brace naming nothing
    /// stays a brace.
    #[test]
    fn explanations_spell_their_keys_from_the_keymap() {
        let mut keymap = Keymap::default();
        assert_eq!(
            expand("Nothing is deleted — {palette} opens it again", &keymap),
            "Nothing is deleted — ^K opens it again"
        );
        let index = crate::keymap::index_of(Action::Palette).unwrap();
        keymap.bind(index, KeyChord::parse("f2").unwrap(), false);
        assert_eq!(expand("{palette} jumps", &keymap), "F2 jumps");
        assert_eq!(
            expand("{help} lists them", &keymap),
            "Keyboard shortcuts lists them"
        );
        assert_eq!(
            expand("json {\"open\"} {nope}", &keymap),
            "json {\"open\"} {nope}"
        );
    }

    /// No action's or setting's explanation types a ⌘ chord: every key it
    /// names is a `{token}` the keymap spells, so a rebind never leaves one
    /// behind saying the old key.
    #[test]
    fn no_explanation_types_a_cmd_chord() {
        let keymap = Keymap::default();
        let mut texts: Vec<String> = crate::keymap::ACTIONS
            .iter()
            .map(|spec| spec.hint.to_string())
            .collect();
        for tab in 0..crate::config::tab_count() {
            for index in 0..crate::config::tab_len(tab) {
                texts.push(crate::config::hint_at(tab, index));
            }
        }
        for text in texts {
            let chord = text.char_indices().any(|(i, c)| {
                c == '⌘' && text[i + c.len_utf8()..].starts_with(|n: char| !n.is_whitespace())
            });
            assert!(!chord, "a literal ⌘ chord: {text}");
            let expanded = expand(&text, &keymap);
            for spec in crate::keymap::ACTIONS {
                assert!(
                    !expanded.contains(&format!("{{{}}}", spec.id)),
                    "an unexpanded token: {expanded}"
                );
            }
        }
    }

    #[test]
    fn grouped_actions_run_together() {
        let keymap = Keymap::default();
        assert_eq!(
            acts(&keymap, &[Action::FocusLeft, Action::FocusRight], "step")
                .unwrap()
                .key,
            "←→"
        );
        assert_eq!(
            acts(
                &keymap,
                &[Action::PrevProjectTab, Action::NextProjectTab],
                "tabs"
            )
            .unwrap()
            .key,
            "[]"
        );
    }
}
