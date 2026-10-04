//! The shape a pull request takes wherever a row names one — the `↗`
//! glyph, the `#42 title` label and a trailing badge — and how a draft, a
//! merged and a closed pull request are told apart from an open one. A
//! BAND's rule and the PULL REQUESTS MODAL's rows both take their colors
//! here (`look`), so the two read as one.

use crate::pull_request::{Standing, Trouble};
use crate::theme::Theme;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// The colors of one pull request row: the arrow, the title, the PILL
/// ROW's rail and the state word in the trailing badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub glyph: Color,
    pub label: Color,
    pub rail: Color,
    pub badge: Color,
}

/// An open pull request is muted, arrow and title alike, with a dim
/// `ready`: a pull request is a fact about a checkout, not the focus (the
/// accent is the cursor's) and not a status (the colors are the
/// sessions'). A draft goes a step quieter, the faint gray, the whole way
/// down — so a row that isn't ready reads as such before its `draft`
/// badge is even read.
///
/// A merged pull request wears the theme's `merged` purple — the color the
/// PR PREVIEW paints that state in — on its arrow, rail and badge, with the
/// title left readable: the work landed, and the checkout under it is the
/// one about to be archived. A closed one is over and nothing on it asks
/// for anyone, so it is faint end to end, like a draft — never the red
/// that means a person has to act.
///
/// A pull request GitHub says cannot merge — its branch conflicts with the
/// base, or a check is failing (`trouble`) — does need a person, so its
/// arrow and badge take the crimson a session that needs you wears, the
/// title staying muted so the badge word (`conflicts`, `failing`) is the
/// loud part. It outranks a draft's faint — a draft's conflict still needs
/// a person — and applies only while the pull request is open: a merged or
/// closed one is past resolving.
pub fn look(standing: Standing, trouble: Option<Trouble>, th: Theme) -> Look {
    if trouble.is_some() && matches!(standing, Standing::Open | Standing::Draft) {
        return Look {
            glyph: th.err,
            label: th.muted,
            rail: th.err,
            badge: th.err,
        };
    }
    match standing {
        Standing::Open => Look {
            glyph: th.muted,
            label: th.muted,
            rail: th.muted,
            badge: th.dim,
        },
        Standing::Draft | Standing::Closed => Look {
            glyph: th.faint,
            label: th.faint,
            rail: th.faint,
            badge: th.faint,
        },
        Standing::Merged => Look {
            glyph: th.merged,
            label: th.muted,
            rail: th.merged,
            badge: th.merged,
        },
    }
}

/// The row's spans: `↗ `, the label cut to what `width` leaves after the
/// badge, then the badge (text and color) when there is one. The badge is
/// billed before the label so it never clips off the end of a narrow column.
pub fn spans(
    look: Look,
    label: &str,
    width: usize,
    badge: Option<(String, Color)>,
) -> Vec<Span<'static>> {
    let badge_len = badge.as_ref().map_or(0, |(b, _)| b.chars().count());
    let label_max = width.saturating_sub(3).saturating_sub(badge_len);
    let mut spans = vec![
        Span::styled("↗ ", Style::default().fg(look.glyph)),
        Span::styled(
            crate::ui::truncate(label, label_max),
            Style::default().fg(look.label),
        ),
    ];
    if let Some((badge, color)) = badge {
        spans.push(Span::styled(badge, Style::default().fg(color)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A draft is a different color from an open pull request in every
    /// part of the row — arrow, title, rail — in every theme preset, and
    /// that color is the faint gray, not a status color that would make a
    /// draft look like it needs someone. Neither takes the accent: that is
    /// the cursor's.
    #[test]
    fn a_draft_is_fainter_than_an_open_pull_request() {
        for name in crate::theme::THEMES {
            let th = Theme::by_name(name);
            let open = look(Standing::Open, None, th);
            let draft = look(Standing::Draft, None, th);
            assert_eq!(open.glyph, th.muted, "{name}");
            assert_ne!(open.glyph, th.accent, "{name}: the accent is the cursor's");
            assert_eq!(
                draft,
                Look {
                    glyph: th.faint,
                    label: th.faint,
                    rail: th.faint,
                    badge: th.faint,
                },
                "{name}"
            );
            assert_ne!(
                open.glyph, draft.glyph,
                "{name}: the arrow tells them apart"
            );
            assert_ne!(open.label, draft.label, "{name}: so does the title");
        }
    }

    /// A merged pull request takes the purple the PR PREVIEW paints that
    /// state in, on the arrow and the badge, so the row and the pane agree;
    /// a closed one is faint end to end — never the red that would make a
    /// dead pull request look like something that needs someone.
    #[test]
    fn merged_and_closed_rows_wear_the_preview_state_colors() {
        for name in crate::theme::THEMES {
            let th = Theme::by_name(name);
            let merged = look(Standing::Merged, None, th);
            assert_eq!(merged.glyph, th.merged, "{name}");
            assert_eq!(merged.badge, th.merged, "{name}");
            assert_eq!(merged.rail, th.merged, "{name}");
            assert_eq!(merged.label, th.muted, "{name}: the title stays readable");
            let closed = look(Standing::Closed, None, th);
            assert_eq!(closed.glyph, th.faint, "{name}");
            assert_eq!(closed.badge, th.faint, "{name}: no red on a closed PR");
            assert_eq!(closed.label, th.faint, "{name}");
            assert_ne!(
                merged.glyph, closed.glyph,
                "{name}: the arrow tells them apart"
            );
        }
    }

    /// A pull request in trouble — conflicts or a failing check — wears
    /// the needs-you crimson on its arrow, rail and badge in every theme,
    /// the same crimson whether the trouble is one or the other: the badge
    /// word tells them apart, the color says "needs you". The title stays
    /// muted so the word is the loud part. A draft's faint gives way to it;
    /// a merged or closed pull request is past resolving and keeps its own
    /// look.
    #[test]
    fn a_pull_request_in_trouble_wears_the_needs_you_red() {
        for name in crate::theme::THEMES {
            let th = Theme::by_name(name);
            let red = Look {
                glyph: th.err,
                label: th.muted,
                rail: th.err,
                badge: th.err,
            };
            for trouble in [Trouble::Conflicts, Trouble::FailingChecks] {
                assert_eq!(look(Standing::Open, Some(trouble), th), red, "{name}");
                assert_eq!(look(Standing::Draft, Some(trouble), th), red, "{name}");
                assert_eq!(
                    look(Standing::Merged, Some(trouble), th),
                    look(Standing::Merged, None, th),
                    "{name}: merged is past resolving"
                );
                assert_eq!(
                    look(Standing::Closed, Some(trouble), th),
                    look(Standing::Closed, None, th),
                    "{name}: so is closed"
                );
            }
            assert_ne!(
                red.glyph,
                look(Standing::Closed, None, th).glyph,
                "{name}: a closed pull request is not in trouble"
            );
        }
    }

    /// The badge keeps its cell budget: a long title shortens, the `draft`
    /// mark does not fall off the end.
    #[test]
    fn the_badge_is_billed_before_the_title() {
        let th = Theme::default();
        let rows = spans(
            look(Standing::Draft, None, th),
            "#9 A title far too long for the column",
            20,
            Some((" draft".into(), th.faint)),
        );
        let text: String = rows.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with(" draft"), "{text:?}");
        assert!(text.chars().count() <= 20, "{text:?}");
        assert_eq!(rows[0].style.fg, Some(th.faint), "a draft's arrow is faint");

        let plain = spans(look(Standing::Open, None, th), "#7 Attach links", 20, None);
        assert_eq!(plain.len(), 2, "no badge, no span for one");
        assert_eq!(plain[0].style.fg, Some(th.muted));
    }
}
