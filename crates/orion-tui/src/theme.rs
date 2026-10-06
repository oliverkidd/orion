//! Color theme: every color the UI draws, keyed by role rather than
//! hardcoded at the call site. The active theme is picked by name in the
//! settings overlay (`theme` in config.json) and lives on `App`, refreshed
//! by the event loop when the setting changes.
//!
//! A preset is an accent, a focus tint and a syntax color. Everything a
//! status says — needs you, working, done and not seen, the grays under
//! them — is a fixed 256-color value that no preset and no terminal
//! palette can move: two presets, or one preset in two terminals, never
//! disagree about which session wants you. The one status that follows the
//! preset is DONE, NOT SEEN, which takes the accent itself (see
//! [`done_for`]); the one gray a preset moves is `mono`'s text, a step
//! under its white accent. Presets stick to ANSI-16 and 256-color indexed values so
//! they render everywhere. One exception: `focus_tint` needs a near-black
//! shade of the accent that the 256 palette simply doesn't have (its
//! darkest chromatic steps start around 40%), so it's truecolor RGB —
//! supported by modern terminals including Terminal.app since macOS Tahoe.

use ratatui::style::Color;

/// What the BLACK BACKGROUND setting paints under every cell nothing else
/// colored. Truecolor rather than ANSI `Black`, which a terminal palette is
/// free to map to a dark gray (a stock Ghostty's is #1d1f21) — the very
/// gray the setting exists to get away from.
pub const BLACK_BACKGROUND: Color = Color::Rgb(0, 0, 0);

/// Names the settings overlay cycles through; `by_name` accepts them
/// case-insensitively and falls back to the first entry.
pub const THEMES: &[&str] = &[
    "default", "ocean", "forest", "rose", "amber", "lavender", "coral", "slate", "sand", "mono",
];

/// Semantic color roles for the whole TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Focus chrome: the selection rail, the heavy frame on the cursor's
    /// card, the lit band rule, the lit tab, modal frames, match
    /// highlights, key caps. Never a count, a link, an author or a pane
    /// title — the accent says where the keys are, nothing else.
    pub accent: Color,
    /// Text drawn on top of an `accent` background (focused title chip).
    pub on_accent: Color,
    /// Primary text: the name of a session that wants you, input text.
    pub text: Color,
    /// Secondary text: prompts, unlit titles, counts, links, PR titles,
    /// the name of a session at rest. Also what `dim` spans brighten to on
    /// an unfocused selection bar.
    pub muted: Color,
    /// Hints, the harness, ago labels, the AT REST status dot; also the
    /// unfocused selection-bar background.
    pub dim: Color,
    /// The quietest text: separators, drafts, a closed pull request, an
    /// archived or offline session, a reaped one's dot.
    pub faint: Color,
    /// Good news that is not a session's status: a passing check, an
    /// approved review, a file ticked off in review, a tool installed.
    pub ok: Color,
    /// DONE, NOT SEEN: a turn that finished while nobody was looking — the
    /// dot, the `done` tag, and the project tab's count. The preset's own
    /// accent, unless that accent is warm, gray or purple ([`done_for`]):
    /// a warm accent reads as the needs-you red or the working gold, a gray
    /// one as a session at rest, a purple one as a merged pull request.
    /// Reading the session takes it back to AT REST gray.
    pub done: Color,
    /// WORKING: the spinner a running session's dot turns into, and a
    /// running check. Also the few genuine warnings left (a hotkey nothing
    /// answers, a list that could not refresh).
    pub warn: Color,
    /// NEEDS YOU: a session stopped on a question or a permission, one
    /// that crashed (FAILED), a pull request that can't merge, a review
    /// asking for changes, destructive actions.
    pub err: Color,
    /// Syntax keywords and markdown `Important` callouts — no status.
    pub special: Color,
    /// A merged pull request: its arrow, badge and — while its checkout
    /// is still around — the checkout's whole band (MERGED BAND). Purple
    /// in every preset, the color GitHub paints a merge in.
    pub merged: Color,
    /// Lines and files a change adds, in every diff surface.
    pub added: Color,
    /// Lines and files a change removes.
    pub removed: Color,
    /// Files a change modifies (`M`).
    pub modified: Color,
    /// A terminal whose process has exited: its `exited` tag. A crashed
    /// *session* is `err` (it needs you); a dead shell is just over.
    pub exited: Color,
    /// Selected-row fill in the focused panel (a subtle raised surface,
    /// not a reverse-video slab).
    pub sel_bg: Color,
    /// Selected-row fill in unfocused panels (barely raised).
    pub sel_bg_dim: Color,
    /// Structural chrome: column rules, header underlines, dividers.
    /// Darker than `dim` so the frame recedes behind the content.
    pub edge: Color,
    /// Shades for the ONE-SHOT SWEEP a session takes when it starts
    /// needing you or crashes, `[tail, mid, head]`: the whole name sits on
    /// the tail shade while a brighter two-cell band sweeps across it.
    /// Rests on `err`.
    pub err_sweep: [Color; 3],
    /// The MERGED BAND's ONE-SHOT SWEEP, for the few seconds after a
    /// checkout's pull request is seen to land. Rests on `merged`.
    pub merged_sweep: [Color; 3],
    /// The UNREAD SHIMMER: a session whose finished turn nobody has looked
    /// at sweeps on this for as long as that stays true, and so does its
    /// project's tab. Rests on `done`, so a preset that moves `done` moves
    /// this with it.
    pub done_sweep: [Color; 3],
    /// Focused-surface background, behind the session pane or the card
    /// keys land in: the accent's own hue taken down to a near-black —
    /// OKLCH lightness 0.20 and chroma 0.04 in every preset (mono's is
    /// the one gray). On a black window it reads as the accent glowing
    /// faintly rather than as a gray slab, and it is darker than the gray
    /// it replaced, so dim text on it keeps more of its contrast.
    /// Truecolor by necessity (see module docs).
    pub focus_tint: Color,
}

/// DONE, NOT SEEN for a preset whose accent can't carry it: sky blue,
/// with its sweep.
const DONE_SKY: [Color; 3] = [Color::Indexed(81), Color::Indexed(117), Color::Indexed(195)];

/// DONE, NOT SEEN for an accent that can carry it: the accent, the sweep
/// climbing from it through two lighter steps of the same hue. Not for an
/// accent that is warm (it would read as needs-you or working), gray (at
/// rest) or within a shade of the merged purple (lavender's periwinkle is
/// two cube steps from it) — those presets take [`DONE_SKY`].
fn done_for(accent: Color) -> [Color; 3] {
    match accent {
        Color::Cyan => [Color::Cyan, Color::Indexed(123), Color::Indexed(195)],
        Color::Indexed(39) => [Color::Indexed(39), Color::Indexed(117), Color::Indexed(195)],
        Color::Indexed(114) => [
            Color::Indexed(114),
            Color::Indexed(151),
            Color::Indexed(194),
        ],
        Color::Indexed(110) => [
            Color::Indexed(110),
            Color::Indexed(153),
            Color::Indexed(195),
        ],
        _ => DONE_SKY,
    }
}

impl Default for Theme {
    fn default() -> Self {
        // The classic orion look: a cyan accent over the fixed status set.
        Self::with_accent(Color::Cyan, Color::Magenta, Color::Rgb(0, 27, 28))
    }
}

impl Theme {
    /// The fixed status set under a preset's `accent`, `special` and
    /// `focus_tint` — everything but those three is the same in every
    /// preset, `done` aside, which follows the accent ([`done_for`]).
    fn with_accent(accent: Color, special: Color, focus_tint: Color) -> Self {
        let done = done_for(accent);
        Self {
            accent,
            on_accent: Color::Black,
            text: Color::Indexed(255),
            muted: Color::Indexed(247),
            dim: Color::Indexed(243),
            faint: Color::Indexed(240),
            ok: Color::Indexed(71),    // green — passing, approved
            done: done[0],             // the accent, or sky
            warn: Color::Indexed(221), // gold — working
            err: Color::Indexed(197),  // crimson — needs you
            special,
            merged: Color::Indexed(135),   // purple — GitHub's merged
            added: Color::Indexed(108),    // sage
            removed: Color::Indexed(174),  // rose
            modified: Color::Indexed(180), // sand
            exited: Color::Indexed(173),   // copper
            sel_bg: Color::Indexed(237),
            sel_bg_dim: Color::Indexed(235),
            edge: Color::Indexed(238),
            err_sweep: [
                Color::Indexed(197),
                Color::Indexed(204),
                Color::Indexed(224),
            ],
            merged_sweep: [
                Color::Indexed(135),
                Color::Indexed(141),
                Color::Indexed(183),
            ],
            done_sweep: done,
            focus_tint,
        }
    }

    pub fn by_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            // deep sky blue / steel blue
            "ocean" => Self::with_accent(
                Color::Indexed(39),
                Color::Indexed(75),
                Color::Rgb(3, 24, 38),
            ),
            // pale green / sage
            "forest" => Self::with_accent(
                Color::Indexed(114),
                Color::Indexed(108),
                Color::Rgb(10, 27, 10),
            ),
            // pink / violet
            "rose" => Self::with_accent(
                Color::Indexed(211),
                Color::Indexed(141),
                Color::Rgb(36, 14, 21),
            ),
            // orange / copper
            "amber" => Self::with_accent(
                Color::Indexed(214),
                Color::Indexed(173),
                Color::Rgb(32, 19, 1),
            ),
            // periwinkle / orchid
            "lavender" => Self::with_accent(
                Color::Indexed(147),
                Color::Indexed(176),
                Color::Rgb(20, 19, 39),
            ),
            // coral / cadet teal, the cooled-off complement
            "coral" => Self::with_accent(
                Color::Indexed(209),
                Color::Indexed(73),
                Color::Rgb(37, 15, 8),
            ),
            // dusty sky blue / steel blue
            "slate" => Self::with_accent(
                Color::Indexed(110),
                Color::Indexed(67),
                Color::Rgb(6, 23, 39),
            ),
            // tan / bronze
            "sand" => Self::with_accent(
                Color::Indexed(180),
                Color::Indexed(137),
                Color::Rgb(34, 18, 2),
            ),
            "mono" => Self {
                // Grayscale chrome for a terminal whose own palette is
                // colorful enough: the frame, titles and cursors go white
                // and gray, and only the statuses keep their color.
                //
                // A step under the accent, so a white-on-white match
                // highlight still stands out from the text around it.
                text: Color::Indexed(252),
                ..Self::with_accent(Color::White, Color::Indexed(245), Color::Rgb(22, 22, 22))
            },
            _ => Self::default(),
        }
    }
}

/// `c` (RGB components) at `level` of its brightness, the rest black —
/// truecolor, as `focus_tint` is, since the 256 palette has no dim shade
/// of most hues.
pub fn shade(c: [f32; 3], level: f32) -> Color {
    let v = |x: f32| (x * level).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(v(c[0]), v(c[1]), v(c[2]))
}

/// A colour a service hands us as hex — a GitHub label's `d73a4a`, a
/// Linear state's `#f2c94c` — as truecolor. None for anything else, the
/// caller's own role colour standing in.
pub fn hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some(Color::Rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// The RGB a terminal most likely shows for `c`: xterm's defaults for the
/// sixteen named colors, the 6×6×6 cube and the gray ramp for the rest of
/// the 256.
pub fn rgb(c: Color) -> Option<(u8, u8, u8)> {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let index = match c {
        Color::Rgb(r, g, b) => return Some((r, g, b)),
        Color::Indexed(i) => i,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Reset => return None,
    };
    Some(match index {
        0..=15 => ANSI[usize::from(index)],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reads_a_services_colour_with_or_without_its_hash() {
        assert_eq!(hex("d73a4a"), Some(Color::Rgb(0xd7, 0x3a, 0x4a)));
        assert_eq!(hex("#F2C94C"), Some(Color::Rgb(0xf2, 0xc9, 0x4c)));
        assert_eq!(hex(""), None);
        assert_eq!(hex("#fff"), None);
        assert_eq!(hex("zzzzzz"), None);
        assert_eq!(hex("ééé"), None);
    }

    /// Red, green, blue coordinates (0..=5) of a 6x6x6 cube color.
    fn cube(c: Color) -> Option<[u8; 3]> {
        match c {
            Color::Indexed(n @ 16..=231) => {
                let n = n - 16;
                Some([n / 36, (n / 6) % 6, n % 6])
            }
            _ => None,
        }
    }

    /// How far two cube colors sit apart, in steps; None when either is
    /// not a cube color and the distance is not ours to measure.
    fn cube_steps(a: Color, b: Color) -> Option<u8> {
        let (a, b) = (cube(a)?, cube(b)?);
        Some((0..3).map(|i| a[i].abs_diff(b[i])).sum())
    }

    #[test]
    fn by_name_covers_all_presets_and_falls_back() {
        for name in THEMES {
            // Every listed preset must parse (and not accidentally be a
            // misspelling that falls back to default without noticing).
            let theme = Theme::by_name(name);
            if *name != "default" {
                assert_ne!(theme, Theme::default(), "{name} fell back to default");
            }
        }
        assert_eq!(Theme::by_name("no-such-theme"), Theme::default());
        assert_eq!(Theme::by_name(" Ocean "), Theme::by_name("ocean"));

        // DONE, NOT SEEN has to stay readable as itself: never a color
        // another status, a passing check or the at-rest gray already wears.
        for name in THEMES {
            let th = Theme::by_name(name);
            assert_ne!(th.done, th.ok, "{name}: done reads as a passing check");
            assert_ne!(th.done, th.warn, "{name}: done reads as working");
            assert_ne!(th.done, th.err, "{name}: done reads as needs-you");
            assert_ne!(th.done, th.dim, "{name}: done reads as at rest");
            assert_ne!(th.done, th.special, "{name}: done reads as a keyword");
        }
    }

    /// Everything a status says is the same in every preset, so two
    /// presets never disagree about which session wants you. A preset is
    /// an accent, a focus tint and a syntax color — and the DONE color
    /// that follows the accent.
    #[test]
    fn statuses_are_fixed_in_every_preset() {
        let base = Theme::default();
        for name in THEMES {
            let th = Theme::by_name(name);
            for (role, a, b) in [
                ("ok", th.ok, base.ok),
                ("warn", th.warn, base.warn),
                ("err", th.err, base.err),
                ("merged", th.merged, base.merged),
                ("muted", th.muted, base.muted),
                ("dim", th.dim, base.dim),
                ("faint", th.faint, base.faint),
                ("added", th.added, base.added),
                ("removed", th.removed, base.removed),
                ("modified", th.modified, base.modified),
                ("exited", th.exited, base.exited),
            ] {
                assert_eq!(a, b, "{name}: {role} moved with the preset");
            }
            assert_eq!(
                th.err_sweep, base.err_sweep,
                "{name}: the needs-you sweep moved"
            );
        }
    }

    /// DONE, NOT SEEN is the preset's accent — the user's own color for
    /// "look here" — unless that accent is warm (it would read as the
    /// needs-you crimson or the working gold), gray (at rest) or purple
    /// (merged), in which case it is sky blue. The rule is checked against
    /// the accent's hue, so a preset whose accent changes can't keep a
    /// stale answer.
    #[test]
    fn done_follows_the_accent_unless_it_is_warm_gray_or_purple() {
        for name in THEMES {
            let th = Theme::by_name(name);
            let carries = match cube(th.accent) {
                Some([r, g, b]) => {
                    let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
                    let gray = hi - lo <= 1;
                    let warm = r >= g && r > b;
                    // Purple enough to pass for a merge: within a shade of it.
                    let purple = cube_steps(th.accent, th.merged).is_some_and(|s| s < 3);
                    !(gray || warm || purple)
                }
                // ANSI cyan carries it; ANSI white is the one gray.
                None => th.accent == Color::Cyan,
            };
            if carries {
                assert_eq!(th.done, th.accent, "{name}: done should be the accent");
            } else {
                assert_eq!(th.done_sweep, DONE_SKY, "{name}: done should be sky");
            }
        }
    }

    /// Working is the gold spinner, and no preset may hand that gold to
    /// the accent: a running session would pass for the cursor.
    #[test]
    fn working_never_reads_as_the_focus() {
        for name in THEMES {
            let th = Theme::by_name(name);
            assert_ne!(th.warn, th.accent, "{name}: working reads as the cursor");
            assert_ne!(th.warn, th.edge, "{name}: working reads as a quiet edge");
        }
    }

    /// An unread finish can sit right under a merged checkout's purple
    /// band, and the two mean opposite things — so done keeps a whole hue
    /// away from the merge, not a shade: it is no color the merged sweep
    /// passes through, and — when both sit in the 256-color cube — it is
    /// at least three steps from the merge there.
    #[test]
    fn done_is_never_a_shade_of_merged() {
        for name in THEMES {
            let th = Theme::by_name(name);
            assert!(
                !th.merged_sweep.contains(&th.done),
                "{name}: the merged sweep flashes the done color"
            );
            if let Some(steps) = cube_steps(th.done, th.merged) {
                assert!(
                    steps >= 3,
                    "{name}: done is {steps} cube steps from merged: a shade, not a hue"
                );
            }
        }
    }

    /// The UNREAD SHIMMER rests on that preset's `done` — a preset that
    /// moves the one moves the other — brightens toward its head, and is
    /// nobody else's sweep.
    #[test]
    fn done_sweep_rests_on_done_in_every_preset() {
        for name in THEMES {
            let th = Theme::by_name(name);
            assert_eq!(th.done_sweep[0], th.done, "{name}: the sweep rests on done");
            let [tail, mid, head] = th.done_sweep;
            assert!(
                tail != mid && mid != head && tail != head,
                "{name}: a flat band"
            );
            for other in [th.err_sweep, th.merged_sweep] {
                assert!(
                    th.done_sweep.iter().all(|c| !other.contains(c)),
                    "{name}: the done sweep borrows a shade from another"
                );
            }
        }
    }

    /// The accent has to stand out from the text it highlights, and the
    /// text from the muted tier under it, and so on down — in every
    /// preset, including the grayscale one, which is the preset that moves
    /// those roles.
    #[test]
    fn accent_and_the_four_grays_stay_apart_in_every_preset() {
        for name in THEMES {
            let th = Theme::by_name(name);
            assert_ne!(th.accent, th.text, "{name}: a match highlight vanishes");
            assert_ne!(th.text, th.muted, "{name}: secondary text reads as primary");
            assert_ne!(th.muted, th.dim, "{name}: secondary text reads as a hint");
            assert_ne!(th.dim, th.faint, "{name}: a hint reads as a separator");
            assert_ne!(
                th.accent, th.muted,
                "{name}: an unfocused title reads as focused"
            );
        }
    }

    /// A merged pull request is purple in every preset, and that purple is
    /// nobody else's: not the unread `done` a preset may move around, and
    /// not a status that would make a landed pull request look like it
    /// needs someone.
    #[test]
    fn merged_is_purple_and_its_own_color_in_every_preset() {
        for name in THEMES {
            let th = Theme::by_name(name);
            assert_eq!(th.merged, Color::Indexed(135), "{name}: merged is purple");
            assert_ne!(th.merged, th.special, "{name}: merged reads as a keyword");
            assert_ne!(th.merged, th.done, "{name}: merged reads as unread done");
            assert_ne!(th.merged, th.ok, "{name}: merged reads as passing");
            assert_ne!(th.merged, th.warn, "{name}: merged reads as working");
            assert_ne!(th.merged, th.err, "{name}: merged reads as needs-you");
            assert_ne!(th.merged, th.accent, "{name}: merged reads as the cursor");
            assert_ne!(th.merged, th.dim, "{name}: merged reads as a draft");
            assert_eq!(
                th.merged_sweep[0], th.merged,
                "{name}: the sweep rests on merged"
            );
            assert_ne!(
                th.merged_sweep, th.err_sweep,
                "{name}: the sweep reads as needs-you"
            );
        }
    }

    /// `focus_tint` fills the surface keys land in, so it has to be seen —
    /// much darker and it reads as plain black (issue #6) — read as the
    /// accent rather than as gray, which every channel near one value is
    /// on a black window, and stay dark enough that dim text keeps its
    /// contrast: `#666` holds over 3.1:1 of the 3.7:1 it has on black.
    /// Mono is the one gray preset, and takes the one gray tint.
    #[test]
    fn focus_tint_is_visible_hued_and_dark() {
        // WCAG relative luminance.
        let luminance = |rgb: [u8; 3]| {
            let lin = |c: u8| {
                let c = f64::from(c) / 255.0;
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
        };
        for name in THEMES {
            let th = Theme::by_name(name);
            let Color::Rgb(r, g, b) = th.focus_tint else {
                panic!("{name}: focus_tint must be truecolor RGB");
            };
            let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
            assert!(
                hi >= 22,
                "{name}: focus_tint {:?} reads as black",
                (r, g, b)
            );
            assert!(
                luminance([r, g, b]) <= 0.009,
                "{name}: focus_tint {:?} costs dim text its contrast",
                (r, g, b)
            );
            if *name == "mono" {
                assert_eq!(hi, lo, "mono: a gray preset takes a gray tint");
            } else {
                assert!(
                    hi - lo >= 15,
                    "{name}: focus_tint {:?} reads as gray",
                    (r, g, b)
                );
            }
        }
    }
}
