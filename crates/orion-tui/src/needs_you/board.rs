//! The NEEDS YOU BOARD: what `⌘K` shows before anything is typed — the
//! cards of every source in three columns, a cursor that walks them, and
//! a section folded to `+N more  →` where its cards don't fit.
//!
//! The board keeps a snapshot of the cards ([`Board::set_cards`]), taken
//! as it opens and again as anything under it changes, so the keys and
//! the draw read the same list. Laying a column out is [`Board::slots`]'
//! and [`Board::column_layout`]'s; drawing it is [`draw`]'s.

use chrono::Utc;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::model::{Card, CardKey, Column, Section};
use crate::hints::Hint;
use crate::text_input::TextInput;
use crate::theme::Theme;

/// The board's own keys, as its key arm matches them and its border
/// spells them.
pub mod keys {
    use crate::hints::Key;

    pub const COLUMN: Key = crate::nav::SIDEWAYS.doing("column");
    pub const ITEM: Key = crate::nav::ROWS.doing("item");
    pub const JUMP: Key = Key::new(&["enter"], "jump to it");
    pub const DONE: Key = Key::new(&["cmd+d", "ctrl+d"], "done");
    pub const SNOOZE: Key = Key::new(&["cmd+z", "ctrl+z"], "snooze 1h");
    pub const BACK: Key = crate::nav::CLOSE.doing("back to the grid");

    #[cfg(test)]
    #[test]
    fn every_board_key_parses() {
        for key in [COLUMN, ITEM, JUMP, DONE, SNOOZE, BACK] {
            assert!(key.parses(), "{:?}", key.chords);
        }
    }
}

/// What the search field says before anything is typed.
pub const SEARCH: &str = "type to jump to any project, session, PR, issue or message";

/// A section's heading and the blank row under it.
const HEAD: u16 = 2;
/// A card: its two lines and the blank row after it.
const CARD: u16 = 3;
/// The blank row between two sections of a column.
const GAP: u16 = 1;

/// One stop of a column's cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// A card, by its index in [`Board::cards`].
    Card(usize),
    /// A folded section's `+N more  →`: → or Enter unfolds it.
    More(Section),
}

/// One section as a column lays it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Laid {
    pub section: Section,
    /// Rows from the top of the column.
    pub top: u16,
    pub height: u16,
    /// Its cards that are drawn, by index into [`Board::cards`]: every
    /// one when unfolded, which the draw then windows.
    pub cards: Vec<usize>,
    /// How many are folded into `+N more`.
    pub more: usize,
}

/// The board under `⌘K` while the query is empty.
#[derive(Debug, Clone, Default)]
pub struct Board {
    /// What is waiting, in board order, each section's cards together.
    pub cards: Vec<Card>,
    /// The column the cursor is in.
    pub column: usize,
    /// Each column's cursor, an index into its [`Board::slots`].
    rows: [usize; 3],
    /// The section `+N more  →` unfolded, which then has its column.
    pub expanded: Option<Section>,
    /// Each column's body height at the last draw.
    pub height: u16,
    /// Where each slot was drawn, for the pointer: `(rect, column, slot)`.
    pub hits: Vec<(Rect, usize, usize)>,
}

/// The rows a section of `n` cards needs to show them all.
fn needs(n: usize) -> u16 {
    HEAD + if n == 0 { 1 } else { n as u16 * CARD - 1 }
}

/// How `height` rows are shared by a column's two sections of `a` and `b`
/// cards: each its need while both fit, else the smaller its need and the
/// larger the rest, else half each.
fn split(height: u16, a: usize, b: usize) -> (u16, u16) {
    let avail = height.saturating_sub(GAP);
    let (need_a, need_b) = (needs(a), needs(b));
    let top = if need_a + need_b <= avail || need_a <= avail / 2 {
        need_a.min(avail)
    } else if need_b <= avail - avail / 2 {
        avail - need_b
    } else {
        avail / 2
    };
    (top, avail - top)
}

impl Board {
    /// Take a fresh snapshot of the cards, keeping the cursor on the card
    /// it was on when that card is still there.
    pub fn set_cards(&mut self, cards: Vec<Card>) {
        let kept: Vec<Option<CardKey>> = (0..3).map(|c| self.selected_key_in(c)).collect();
        self.cards = cards;
        for (column, key) in kept.into_iter().enumerate() {
            let slots = self.slots(column);
            let at = key.and_then(|key| {
                slots
                    .iter()
                    .position(|s| matches!(s, Slot::Card(i) if self.cards[*i].key == key))
            });
            self.rows[column] = at
                .unwrap_or(self.rows[column])
                .min(slots.len().saturating_sub(1));
        }
    }

    /// The cards of `section`, by index, in board order.
    pub fn section_cards(&self, section: Section) -> Vec<usize> {
        (0..self.cards.len())
            .filter(|&i| self.cards[i].section == section)
            .collect()
    }

    /// The board's count: every card on it.
    pub fn count(&self) -> usize {
        self.cards.len()
    }

    /// Column `column`'s sections as they are drawn in `self.height` rows.
    pub fn column_layout(&self, column: usize) -> Vec<Laid> {
        let sections = Column::ALL[column].sections();
        if let Some(open) = self.expanded.filter(|s| sections.contains(s)) {
            return vec![Laid {
                section: open,
                top: 0,
                height: self.height,
                cards: self.section_cards(open),
                more: 0,
            }];
        }
        let heights: Vec<(u16, u16)> = match sections {
            [_] => vec![(0, self.height)],
            [a, b] => {
                let (top, bottom) = split(
                    self.height,
                    self.section_cards(*a).len(),
                    self.section_cards(*b).len(),
                );
                vec![(0, top), (top + GAP, bottom)]
            }
            _ => unreachable!("a column holds one or two sections"),
        };
        sections
            .iter()
            .zip(heights)
            .map(|(&section, (top, height))| {
                let mut cards = self.section_cards(section);
                let mut more = 0;
                if needs(cards.len()) > height {
                    // Room for the `+N more` row under the cards that fit.
                    let shown = usize::from(height.saturating_sub(HEAD + 1) / CARD);
                    more = cards.len() - shown;
                    cards.truncate(shown);
                }
                Laid {
                    section,
                    top,
                    height,
                    cards,
                    more,
                }
            })
            .collect()
    }

    /// Column `column`'s cursor stops, top to bottom.
    pub fn slots(&self, column: usize) -> Vec<Slot> {
        self.column_layout(column)
            .into_iter()
            .flat_map(|laid| {
                let more = (laid.more > 0).then_some(Slot::More(laid.section));
                laid.cards.into_iter().map(Slot::Card).chain(more)
            })
            .collect()
    }

    /// The cursor's row in `column`.
    pub fn row(&self, column: usize) -> usize {
        self.rows[column]
    }

    /// The slot the cursor is on, if its column has any.
    pub fn selected_slot(&self) -> Option<Slot> {
        self.slots(self.column).get(self.rows[self.column]).copied()
    }

    /// The card the cursor is on.
    pub fn selected(&self) -> Option<&Card> {
        match self.selected_slot()? {
            Slot::Card(i) => self.cards.get(i),
            Slot::More(_) => None,
        }
    }

    fn selected_key_in(&self, column: usize) -> Option<CardKey> {
        match self.slots(column).get(self.rows[column])? {
            Slot::Card(i) => self.cards.get(*i).map(|c| c.key.clone()),
            Slot::More(_) => None,
        }
    }

    /// ↑/↓: the cursor `delta` rows along its column, held at the ends.
    pub fn step(&mut self, delta: i64) {
        let len = self.slots(self.column).len();
        self.rows[self.column] =
            crate::app::clamp_selection(self.rows[self.column] as i64 + delta, len);
    }

    /// Put the cursor on `row` of `column` — a click.
    pub fn select(&mut self, column: usize, row: usize) {
        if column != self.column {
            self.leave_column();
        }
        self.column = column.min(2);
        let len = self.slots(self.column).len();
        self.rows[self.column] = crate::app::clamp_selection(row as i64, len);
    }

    /// ←/→: on `+N more` → unfolds the section; in an unfolded one ←
    /// folds it back; else the cursor goes to the next column that way,
    /// held at the ends.
    pub fn sideways(&mut self, right: bool) {
        match self.selected_slot() {
            Some(Slot::More(section)) if right => return self.expand(section),
            _ if !right && self.expanded_here() => {
                self.collapse();
                return;
            }
            _ => {}
        }
        let to = if right {
            (self.column + 1).min(2)
        } else {
            self.column.saturating_sub(1)
        };
        if to != self.column {
            self.leave_column();
            self.column = to;
        }
    }

    /// Unfold `section`: it takes its column, the cursor where `+N more`
    /// was — on the first card it had folded away.
    pub fn expand(&mut self, section: Section) {
        let folded_at = self
            .column_layout(self.column)
            .into_iter()
            .find(|laid| laid.section == section)
            .map_or(0, |laid| laid.cards.len());
        self.expanded = Some(section);
        self.rows[self.column] = folded_at;
    }

    fn expanded_here(&self) -> bool {
        self.expanded
            .is_some_and(|s| s.column() == Column::ALL[self.column])
    }

    fn collapse(&mut self) {
        if let Some(section) = self.expanded.take() {
            // Back on the section's `+N more`, or its last card shown.
            let slots = self.slots(self.column);
            self.rows[self.column] = slots
                .iter()
                .position(|s| *s == Slot::More(section))
                .unwrap_or(self.rows[self.column].min(slots.len().saturating_sub(1)));
        }
    }

    fn leave_column(&mut self) {
        if self.expanded_here() {
            self.collapse();
        }
    }
}

/// The board's keys line, on its bottom border.
pub fn hints() -> Vec<Hint> {
    vec![
        Hint::new("type", "to jump"),
        keys::COLUMN.hint(),
        keys::ITEM.hint(),
        keys::JUMP.hint().kept(),
        keys::DONE.hint(),
        keys::SNOOZE.hint(),
        keys::BACK.hint().kept(),
    ]
}

/// A card's glyph and its color: what kind of thing waits.
fn glyph(card: &Card, th: Theme) -> (&'static str, ratatui::style::Color) {
    match card.section {
        Section::Agents => match card.urgency {
            2 => ("✕", th.err),
            1 => ("●", th.err),
            _ => ("●", th.done),
        },
        Section::Linear => ("◇", th.merged),
        Section::CodeReviews => ("↗", th.muted),
        Section::Slack => ("#", th.muted),
        Section::Meetings => ("▸", th.muted),
    }
}

/// Draw the board over `area`: the frame, the header row with the search
/// field and, on its right, the next meeting (`▸ Standup in 12m · Teams`,
/// `crate::microsoft::next_meeting`), and the three columns. Writes back
/// where it drew.
pub fn draw(
    f: &mut Frame,
    board: &mut Board,
    query: &TextInput,
    meeting: Option<&str>,
    area: Rect,
    th: Theme,
) {
    let inner = crate::ui::render_modal_frame(f, area, "", &hints(), th);
    if inner.height < 4 || inner.width < 12 {
        return;
    }
    // The header: `Needs you  3   type to jump…`.
    let count = board.count();
    let count_color = if count > 0 { th.err } else { th.ok };
    let lead = vec![
        Span::styled(
            " Needs you",
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {count}"), Style::default().fg(count_color)),
        Span::raw("    "),
    ];
    let lead_w: u16 = lead.iter().map(|s| s.width() as u16).sum();
    f.render_widget(
        Paragraph::new(Line::from(lead)),
        Rect { height: 1, ..inner },
    );
    let meeting_w = meeting.map_or(0, |m| m.chars().count() as u16 + 2);
    if let Some(meeting) = meeting.filter(|_| lead_w + meeting_w < inner.width) {
        f.render_widget(
            Paragraph::new(Span::styled(meeting, Style::default().fg(th.muted)))
                .alignment(ratatui::layout::Alignment::Right),
            Rect {
                x: inner.right().saturating_sub(meeting_w),
                width: meeting_w - 1,
                height: 1,
                ..inner
            },
        );
    }
    let search = Rect {
        x: inner.x + lead_w,
        y: inner.y,
        width: inner.width.saturating_sub(lead_w + meeting_w),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(crate::ui::search_line(query, SEARCH, search, th)),
        search,
    );

    // A cell in from the frame on either side.
    let body = Rect {
        x: inner.x + 1,
        y: inner.y + 2,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(2),
    };
    board.height = body.height;
    board.hits.clear();
    let gutter = 2;
    let width = body.width.saturating_sub(gutter * 2) / 3;
    for column in 0..3 {
        let x = body.x + column as u16 * (width + gutter);
        let col = Rect {
            x,
            width: if column == 2 {
                body.right().saturating_sub(x)
            } else {
                width
            },
            ..body
        };
        draw_column(f, board, column, col, th);
    }
}

fn draw_column(f: &mut Frame, board: &mut Board, column: usize, area: Rect, th: Theme) {
    let focused = board.column == column;
    let cursor = board.row(column);
    let now = Utc::now();
    let mut slot = 0;
    for laid in board.column_layout(column) {
        let top = area.y + laid.top;
        let bottom = (top + laid.height).min(area.bottom());
        let mut y = top;
        let total = board.section_cards(laid.section).len();
        let heading = vec![
            Span::styled(
                laid.section.title(),
                Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if total > 0 {
                    format!("  {total}")
                } else {
                    String::new()
                },
                Style::default().fg(th.dim),
            ),
        ];
        f.render_widget(
            Paragraph::new(Line::from(heading)),
            Rect {
                y,
                height: 1,
                ..area
            },
        );
        y += HEAD;
        if laid.cards.is_empty() && laid.more == 0 {
            f.render_widget(
                Paragraph::new(Span::styled(
                    crate::ui::truncate(laid.section.empty(), usize::from(area.width)),
                    Style::default().fg(th.dim),
                )),
                Rect {
                    y: y.min(bottom.saturating_sub(1)),
                    height: 1,
                    ..area
                },
            );
            continue;
        }
        // An unfolded section windows its cards on the cursor.
        let fit = usize::from(bottom.saturating_sub(y) + 1) / usize::from(CARD);
        let first = if board.expanded == Some(laid.section) {
            crate::app::window_start(cursor.saturating_sub(slot), fit.max(1))
        } else {
            0
        };
        for (n, &i) in laid.cards.iter().enumerate() {
            let this = slot + n;
            if n < first {
                continue;
            }
            if y + 2 > bottom {
                break;
            }
            let rect = Rect {
                y,
                height: 2,
                ..area
            };
            draw_card(f, &board.cards[i], rect, focused && cursor == this, now, th);
            board.hits.push((rect, column, this));
            y += CARD;
        }
        slot += laid.cards.len();
        if laid.more > 0 {
            let rect = Rect {
                y: y.min(bottom.saturating_sub(1)),
                height: 1,
                ..area
            };
            let spans = vec![Span::styled(
                format!("+{} more  →", laid.more),
                Style::default().fg(th.dim),
            )];
            crate::ui::render_row(f, rect, spans, focused && cursor == slot, true, th);
            board.hits.push((rect, column, slot));
            slot += 1;
        }
    }
}

fn draw_card(
    f: &mut Frame,
    card: &Card,
    rect: Rect,
    selected: bool,
    now: chrono::DateTime<Utc>,
    th: Theme,
) {
    let (mark, color) = glyph(card, th);
    let age = crate::hosts::ago_short((now - card.since).num_seconds());
    // The marker column, the glyph and its space, the age and its gap.
    let room = usize::from(rect.width).saturating_sub(4 + age.chars().count() + 2);
    let title = crate::ui::truncate(&card.title, room);
    let pad = room.saturating_sub(title.chars().count());
    let first = vec![
        Span::styled(format!("{mark} "), Style::default().fg(color)),
        Span::styled(title, Style::default().fg(th.text)),
        Span::raw(" ".repeat(pad + 2)),
        Span::styled(age, Style::default().fg(th.dim)),
    ];
    let sub = crate::ui::truncate(&card.sub, usize::from(rect.width).saturating_sub(4));
    let second = vec![
        Span::raw("  "),
        Span::styled(sub, Style::default().fg(th.dim)),
    ];
    crate::ui::render_row_lines(f, rect, vec![first, second], selected, true, th);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::needs_you::model::CardTarget;
    use chrono::DateTime;

    fn cards(section: Section, n: usize) -> Vec<Card> {
        (0..n)
            .map(|i| {
                Card::new(
                    section,
                    CardTarget::Meeting {
                        id: format!("{section:?}{i}"),
                    },
                    format!("card {i}"),
                    "",
                    DateTime::from_timestamp(i as i64, 0).unwrap(),
                    0,
                )
            })
            .collect()
    }

    fn board(height: u16, sections: &[(Section, usize)]) -> Board {
        let mut board = Board {
            height,
            ..Board::default()
        };
        board.set_cards(sections.iter().flat_map(|(s, n)| cards(*s, *n)).collect());
        board
    }

    #[test]
    fn a_section_that_does_not_fit_folds_into_more() {
        // 20 rows: a heading, then six cards of three rows and the `+N`.
        let b = board(20, &[(Section::Agents, 9)]);
        let laid = &b.column_layout(0)[0];
        assert_eq!((laid.cards.len(), laid.more), (5, 4));
        assert_eq!(b.slots(0).last(), Some(&Slot::More(Section::Agents)));
        // Everything fits: no `+N more`.
        let b = board(40, &[(Section::Agents, 9)]);
        assert_eq!(b.column_layout(0)[0].more, 0);
    }

    #[test]
    fn two_sections_share_a_column_the_small_one_whole() {
        let b = board(30, &[(Section::Linear, 1), (Section::CodeReviews, 20)]);
        let laid = b.column_layout(1);
        assert_eq!(laid[0].more, 0, "the one issue shows whole");
        assert_eq!(laid[1].top, needs(1) + GAP);
        assert!(laid[1].more > 0, "the reviews fold");
    }

    #[test]
    fn right_on_more_unfolds_and_left_folds_back() {
        let mut b = board(20, &[(Section::Agents, 9)]);
        b.step(100);
        assert_eq!(b.selected_slot(), Some(Slot::More(Section::Agents)));
        b.sideways(true);
        assert_eq!(b.expanded, Some(Section::Agents));
        assert_eq!(b.selected().map(|c| c.title.as_str()), Some("card 5"));
        assert_eq!(b.slots(0).len(), 9, "every card, no `+N more`");
        b.sideways(false);
        assert_eq!(b.expanded, None);
        assert_eq!(b.selected_slot(), Some(Slot::More(Section::Agents)));
        assert_eq!(b.column, 0);
    }

    #[test]
    fn the_arrows_walk_columns_and_items_held_at_the_ends() {
        let mut b = board(
            40,
            &[
                (Section::Agents, 2),
                (Section::Linear, 1),
                (Section::CodeReviews, 1),
            ],
        );
        b.sideways(false);
        assert_eq!(b.column, 0);
        b.step(1);
        assert_eq!(b.selected().map(|c| c.title.as_str()), Some("card 1"));
        b.sideways(true);
        b.step(1);
        assert_eq!(b.selected().map(|c| c.section), Some(Section::CodeReviews));
        b.sideways(true);
        b.sideways(true);
        assert_eq!(b.column, 2);
        assert_eq!(b.selected(), None, "nothing in Slack or meetings");
    }

    #[test]
    fn a_refresh_keeps_the_cursor_on_its_card() {
        let mut b = board(40, &[(Section::Agents, 3)]);
        b.step(2);
        let key = b.selected().unwrap().key.clone();
        let mut fresh = cards(Section::Agents, 3);
        fresh.rotate_left(2);
        b.set_cards(fresh);
        assert_eq!(b.selected().map(|c| &c.key), Some(&key));
    }

    /// The header carries the next meeting on its right, and nothing
    /// there without one.
    #[test]
    fn the_header_names_the_next_meeting() {
        use ratatui::backend::TestBackend;
        let draw_with = |meeting: Option<&str>| {
            let mut terminal = ratatui::Terminal::new(TestBackend::new(100, 20)).unwrap();
            let mut b = board(0, &[]);
            terminal
                .draw(|f| {
                    let area = f.area();
                    draw(
                        f,
                        &mut b,
                        &TextInput::new(),
                        meeting,
                        area,
                        Theme::default(),
                    )
                })
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..100u16)
                .map(|x| buf[(x, 1)].symbol().to_string())
                .collect::<String>()
        };
        let header = draw_with(Some("▸ Standup in 12m · Teams"));
        assert!(
            header.trim_end().ends_with("▸ Standup in 12m · Teams │"),
            "{header}"
        );
        assert!(header.contains("type to jump"), "{header}");
        assert!(!draw_with(None).contains('▸'));
    }

    #[test]
    fn every_section_says_it_is_empty_in_the_same_words() {
        for section in Section::ALL {
            let words = section.empty();
            assert!(
                words.starts_with("No ") && words.ends_with(" waiting for you"),
                "{words}"
            );
        }
    }
}
