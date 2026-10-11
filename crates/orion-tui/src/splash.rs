//! First-run splash: a procedurally animated Orion filling the body while
//! the project tree is empty, kept faint so the words in front of it read
//! first. Every cell is computed per frame — a thin, grainy cloud (belt,
//! sword, Barnard's Loop) modulated by value noise, a sparse starfield
//! with a fainter layer behind it, the hunter's seven stars as dots on
//! dotted sticks — and the name materializes small in the hunter's chest,
//! between his shoulders and his belt, in a band the sky never paints.
//! Under it all, one line: the key that begins, or, with a newer release
//! waiting, the key that installs it.
//!
//! The sky is shades of the theme's accent, save Betelgeuse's warmth,
//! Rigel's ice and M42's rose; the wordmark runs from the theme's special
//! color through the accent, with a white glint. The shades are truecolor:
//! the 256 palette has no dim shade of most hues, which is why
//! `focus_tint` is truecolor too (see `theme`).
//!
//! The event loop ticks a repaint every [`FRAME`] while [`App::splash_active`]
//! holds; the scene itself is a pure function of elapsed time, so a missed
//! frame skips ahead instead of stuttering. The sky on its own
//! ([`draw_sky`]) is also what the empty GRID's welcome is drawn over,
//! ticked while [`App::welcome_active`] holds.

use crate::app::{App, Focus, HitTarget};
use crate::theme::{shade, Theme};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::Duration;

/// Repaint cadence while the splash is up.
pub const FRAME: Duration = Duration::from_millis(100);

/// Seconds to fade the sky in from black. Also hides the splash flashing
/// briefly on every launch before the daemon's first tree snapshot lands.
const FADE_IN: f32 = 1.2;

/// Density under which a cell is empty sky.
const DUST_FLOOR: f32 = 0.05;
/// The cloud's levels, thinnest first: the density each starts at, its
/// glyph, and its brightness as a share of the accent's. The cloud peaks
/// near 0.4, so the third level is its core.
const DUST: [(f32, char, f32); 3] = [
    (DUST_FLOOR, '.', 0.09),
    (0.118, '.', 0.128),
    (0.254, '·', 0.175),
];
/// How much of the dust is left out at random: 0 paints every cell over
/// the floor, 1 leaves the thinnest empty. Denser dust keeps more of its
/// cells, so the cloud reads as grain rather than a fill.
const GRAIN: f32 = 0.7;
/// One cell of empty sky in this many holds a twinkling star...
const NEAR_STARS: u32 = 120;
/// ...one of those in this many is a sparkle in the accent itself...
const SPARKLES: u32 = 111;
/// ...and one cell in this many holds a fainter, steadier star behind.
const FAR_STARS: u32 = 52;
/// Colors in the wordmark's gradient.
const MARK_STEPS: usize = 6;

/// 5-row bitmaps for O R I O N, three pixels wide (the N four). Drawn two
/// rows to a cell with half blocks, so a pixel is as tall as it is wide
/// and the name stands [`MARK_ROWS`] cells high.
const LETTERS: &[&[&str; 5]] = &[
    &["###", "#.#", "#.#", "#.#", "###"],
    &["##.", "#.#", "##.", "#.#", "#.#"],
    &["###", ".#.", ".#.", ".#.", "###"],
    &["###", "#.#", "#.#", "#.#", "###"],
    &["#..#", "##.#", "#.##", "#..#", "#..#"],
];
/// Cells the block-letter name stands high.
const MARK_ROWS: u16 = 3;
/// Columns kept clear either side of the name, between it and the
/// hunter's sides.
const MARK_CLEAR: u16 = 1;
/// Empty columns between two of its letters.
const MARK_GAP: usize = 1;
/// The name letter by letter, for a chest too narrow for the blocks.
const MARK_SPACED: &str = "O R I O N";
/// How far up the hunter the name sits, in constellation space: midway
/// between the shoulders and the belt.
const CHEST: f32 = -0.3;
/// Where his sides cross that height, inside which the name has to fit.
const CHEST_SIDES: (f32, f32) = (-0.42, 0.39);
/// The shoulders' and the belt's heights, which it has to fit between.
const CHEST_SPAN: (f32, f32) = (-0.62, 0.03);

type Rgb = [f32; 3];
const WHITE: Rgb = [255.0; 3];
const GRAY: Rgb = [128.0; 3];
const BETELGEUSE: Rgb = [255.0, 178.0, 140.0];
const RIGEL: Rgb = [190.0, 220.0, 255.0];
const M42: Rgb = [225.0, 150.0, 190.0];

/// A star's color: the accent taken part of the way to white, or a hue of
/// its own.
#[derive(Clone, Copy)]
enum Hue {
    Pale(f32),
    Own(Rgb),
}

/// One of the hunter's stars: where it sits in constellation space ((0, 0)
/// is the belt, +x right, +y down the sword), its glyph, its twinkle
/// phase and its color.
struct Star(f32, f32, char, f32, Hue);

/// The hunter, in the drawing order the sticks use. The glyph follows the
/// magnitude: a dot for the five brightest, a middle dot for Mintaka and
/// Saiph.
const STARS: [Star; 7] = [
    Star(-0.56, -0.70, '•', 0.0, Hue::Own(BETELGEUSE)), // 0.5 — shoulder
    Star(0.50, -0.60, '•', 1.1, Hue::Pale(0.6)),        // Bellatrix, 1.6 — shoulder
    Star(-0.30, 0.08, '•', 2.0, Hue::Pale(0.78)),       // Alnitak, 1.8 — belt
    Star(0.00, 0.03, '•', 2.7, Hue::Pale(0.78)),        // Alnilam, 1.7 — belt
    Star(0.30, -0.02, '·', 3.4, Hue::Pale(0.78)),       // Mintaka, 2.2 — belt
    Star(-0.44, 0.78, '·', 4.2, Hue::Pale(0.45)),       // Saiph, 2.1 — knee
    Star(0.54, 0.74, '•', 5.0, Hue::Own(RIGEL)),        // 0.1 — foot
];

/// Stick figure: shoulders, sides, belt, base.
const EDGES: &[(usize, usize)] = &[
    (0, 1),
    (0, 2),
    (1, 4),
    (2, 3),
    (3, 4),
    (2, 5),
    (4, 6),
    (5, 6),
];

/// `c`'s RGB, white for a color with no fixed value.
fn rgb_of(c: Color) -> Rgb {
    crate::theme::rgb(c).map_or(WHITE, |(r, g, b)| [r, g, b].map(f32::from))
}

fn mix(a: Rgb, b: Rgb, u: f32) -> Rgb {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * u)
}

/// `c` taken `w` of the way to white.
fn pale(c: Rgb, w: f32) -> Rgb {
    mix(c, WHITE, w)
}

/// The sky's colors for one accent, worked out once a frame.
struct Palette {
    accent: Color,
    /// Under each of `DUST`'s levels.
    dust: [Color; DUST.len()],
    near_dim: Color,
    near_bright: Color,
    far_lit: Color,
    far_dim: Color,
    stick: Color,
    /// The sticks while the sky is still fading in.
    stick_fading: Color,
    /// Each of `STARS` at full brightness.
    stars: [Rgb; STARS.len()],
}

impl Palette {
    fn of(accent: Color) -> Self {
        let rgb = rgb_of(accent);
        let stick = mix(rgb, GRAY, 0.25);
        Self {
            accent,
            dust: DUST.map(|(_, _, level)| shade(rgb, level)),
            near_dim: shade(rgb, 0.25),
            near_bright: shade(pale(rgb, 0.6), 0.5),
            far_lit: shade(rgb, 0.44),
            far_dim: shade(rgb, 0.34),
            stick: shade(stick, 0.8),
            stick_fading: shade(stick, 0.52),
            stars: STARS.map(|Star(.., hue)| match hue {
                Hue::Pale(w) => pale(rgb, w),
                Hue::Own(c) => c,
            }),
        }
    }
}

fn hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(374_761_393)
        ^ (y as u32).wrapping_mul(668_265_263)
        ^ salt.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

fn hash01(x: i32, y: i32, salt: u32) -> f32 {
    (hash(x, y, salt) & 0xffff) as f32 / 65535.0
}

/// One octave of smooth 2D value noise.
fn vnoise(x: f32, y: f32, salt: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash01(xi, yi, salt);
    let b = hash01(xi + 1, yi, salt);
    let c = hash01(xi, yi + 1, salt);
    let d = hash01(xi + 1, yi + 1, salt);
    a + (b - a) * sx + (c - a) * sy + (a - b - c + d) * sx * sy
}

fn rotate(x: f32, y: f32, rot: f32) -> (f32, f32) {
    let (s, c) = rot.sin_cos();
    (x * c - y * s, x * s + y * c)
}

/// Dust density at physical offset (dx, dy) from the hunter's belt: a
/// trace of the molecular cloud along the belt and sword, and Barnard's
/// Loop as a faint ring around them. The figure sways with `sway`; noise
/// shears with `drift` so the wisps move without spinning the hunter on
/// his head.
fn density(dx: f32, dy: f32, sway: f32, drift: f32) -> f32 {
    let (x, y) = rotate(dx, dy, sway);
    let r = (x * x + y * y).sqrt().max(0.001);
    let belt = (-(x * x * 2.4 + (y - 0.04) * (y - 0.04) * 16.0)).exp();
    let sword = (-((x + 0.07) * (x + 0.07) * 18.0 + (y - 0.38) * (y - 0.38) * 7.5)).exp();
    let loop_r = ((x * 1.05) * (x * 1.05) + (y * 0.88) * (y * 0.88)).sqrt();
    let barnard = (-((loop_r - 0.84) * 5.0) * ((loop_r - 0.84) * 5.0)).exp() * 0.36;
    let core = (-r * r * 9.0).exp() * 0.12;
    let (sa, ca) = (drift * 0.3).sin_cos();
    let (nx, ny) = (dx * ca - dy * sa, dx * sa + dy * ca);
    let wisp = 0.55 + 0.45 * vnoise(nx * 3.0 + 7.0, ny * 3.0 + 3.0, 991);
    wisp * ((belt * 0.75 + sword * 0.7 + core) * 0.17 + barnard)
}

/// Seconds into the scene that started at `epoch` — or, with animations
/// off, a moment well past the fade-in: the event loop doesn't tick a
/// still scene, so it holds one finished frame instead of whatever
/// instant a stray redraw lands on.
pub fn scene_time(app: &App, epoch: std::time::Instant) -> f32 {
    if app.animations {
        epoch.elapsed().as_secs_f32()
    } else {
        60.0
    }
}

/// How far the scene has faded in from black `t` seconds in: 0 -> 1 over
/// [`FADE_IN`], eased at both ends.
fn fade_at(t: f32) -> f32 {
    let raw = (t / FADE_IN).clamp(0.0, 1.0);
    raw * raw * (3.0 - 2.0 * raw)
}

/// The wordmark's gradient in steps: the theme's special color, through
/// its accent, to a pale accent.
fn mark_steps(th: Theme) -> [Color; MARK_STEPS] {
    let (special, accent) = (rgb_of(th.special), rgb_of(th.accent));
    std::array::from_fn(|i| {
        let u = i as f32 / (MARK_STEPS - 1) as f32;
        let c = if u < 0.5 {
            mix(special, accent, u / 0.5)
        } else {
            mix(accent, pale(accent, 0.55), (u - 0.5) / 0.5)
        };
        shade(c, 1.0)
    })
}

/// The wordmark's color at `u` (0 -> 1) across it: its step of the
/// gradient, with the slow shine sweeping through once the scene has
/// faded in.
fn mark_color(u: f32, t: f32, fade: f32, steps: &[Color; MARK_STEPS]) -> Color {
    let shine = (u * 5.0 - t * 1.4).sin() > 0.93;
    if shine && fade >= 1.0 {
        return Color::Indexed(231); // near-white glint
    }
    steps[(u * (MARK_STEPS - 1) as f32).round() as usize]
}

/// `word` in the wordmark's gradient and shine, one cell per letter: the
/// name where there is no room for the block letters, or no call for
/// them.
pub fn wordmark_word(word: &str, t: f32, th: Theme) -> Vec<Span<'static>> {
    let fade = fade_at(t);
    let steps = mark_steps(th);
    let n = word.chars().count().max(1) as f32;
    word.chars()
        .enumerate()
        .map(|(i, ch)| {
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(mark_color(i as f32 / n, t, fade, &steps))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// Columns the block-letter name spans.
fn mark_width() -> usize {
    LETTERS.iter().map(|l| l[0].len()).sum::<usize>() + MARK_GAP * (LETTERS.len() - 1)
}

/// One cell row of the block-letter name as per-cell spans: two bitmap
/// rows to the cell, the gradient across the word, a slow shine sweeping
/// through, and the blocks materializing from static (`░` -> `▒`) while
/// the scene fades in.
fn wordmark_line(row: usize, t: f32, fade: f32, th: Theme) -> Line<'static> {
    let steps = mark_steps(th);
    let width = mark_width();
    let lit = |letter: &[&str; 5], r: usize, c: usize| {
        letter.get(r).is_some_and(|line| line.as_bytes()[c] == b'#')
    };
    let mut spans = Vec::new();
    let mut col = 0usize;
    for (i, letter) in LETTERS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(MARK_GAP)));
            col += MARK_GAP;
        }
        for c in 0..letter[0].len() {
            let glyph = match (lit(letter, 2 * row, c), lit(letter, 2 * row + 1, c)) {
                (false, false) => {
                    spans.push(Span::raw(" "));
                    col += 1;
                    continue;
                }
                _ if fade < 0.5 => "░",
                _ if fade < 0.85 => "▒",
                (true, true) => "█",
                (true, false) => "▀",
                (false, true) => "▄",
            };
            let u = col as f32 / width as f32;
            spans.push(Span::styled(
                glyph,
                Style::default()
                    .fg(mark_color(u, t, fade, &steps))
                    .add_modifier(Modifier::BOLD),
            ));
            col += 1;
        }
    }
    Line::from(spans)
}

/// The name as the hunter's chest has room for it: the block letters, the
/// name letter by letter on one row, or nothing where even that would
/// cross his sides — with the cells it takes, centered in the chest.
fn chest_mark(
    frame: &SkyFrame,
    area: Rect,
    t: f32,
    th: Theme,
) -> Option<(Rect, Vec<Line<'static>>)> {
    let at = frame.place(HUNTER_SPAN, 0.0);
    let (mid_x, mid_y) = at.cell(0.0, CHEST);
    let room_w = at.cell(CHEST_SIDES.1, CHEST).0 - at.cell(CHEST_SIDES.0, CHEST).0 - 1;
    let room_h = at.cell(0.0, CHEST_SPAN.1).1 - at.cell(0.0, CHEST_SPAN.0).1 - 1;
    let blocks = mark_width() as i32;
    let spaced = MARK_SPACED.chars().count() as i32;
    let (w, lines): (i32, Vec<Line<'static>>) =
        if room_w >= blocks + 2 * i32::from(MARK_CLEAR) && room_h > i32::from(MARK_ROWS) {
            let fade = fade_at(t);
            (
                blocks,
                (0..usize::from(MARK_ROWS))
                    .map(|row| wordmark_line(row, t, fade, th))
                    .collect(),
            )
        } else if room_w >= spaced + 2 * i32::from(MARK_CLEAR) && room_h >= 1 {
            (spaced, vec![Line::from(wordmark_word(MARK_SPACED, t, th))])
        } else {
            return None;
        };
    let h = lines.len() as i32;
    let rect = Rect {
        x: u16::try_from(mid_x - w / 2).ok()?,
        y: u16::try_from(mid_y - h / 2).ok()?,
        width: w as u16,
        height: h as u16,
    };
    (rect.intersection(area) == rect).then_some((rect, lines))
}

pub fn draw_splash(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    if area.width < 8 || area.height < 4 {
        return;
    }
    let t = scene_time(app, app.splash_epoch);

    // ---- the one line under the sky ----
    let mut lines: Vec<Line> = Vec::new();
    // Enter, spelled from the live keymap: it begins — back down to the
    // grid from HOME, into the repo orion was started in, or to the
    // open-project prompt — or, with a newer release waiting, installs it,
    // on the upgrade's own green and clickable like the footer's `⇡ v…`.
    // The jump list, `+` and the rest still work; the footer names them.
    let enter = crate::hints::key_or(&app.keymap, crate::keymap::Action::Activate, "Enter");
    let mut upgrade_row = match app.splash_upgrade().map(str::to_string) {
        Some(v) => {
            lines.push(Line::from(crate::ui::footer::upgrade_spans(
                app,
                &format!("⇡ v{v} available · {enter} to upgrade"),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            Some(lines.len() - 1)
        }
        None => {
            lines.push(Line::from(vec![
                Span::styled(enter, crate::hints::key_style(th)),
                Span::styled(" to begin", crate::hints::does_style(th)),
            ]));
            None
        }
    };

    // Bottom-anchored, a row clear of the body's edge where there is one.
    let block = |lines: &[Line]| {
        let w = (lines.iter().map(Line::width).max().unwrap_or(0) as u16).min(area.width);
        let h = (lines.len() as u16).min(area.height);
        Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + area.height - h - u16::from(area.height > h),
            width: w,
            height: h,
        }
    };
    let mut text = block(&lines);

    // ---- the name: in the hunter's chest, or with no room there, at the
    // head of the words as it always was on a small screen ----
    let mark = chest_mark(&SkyFrame::over(area, text), area, t, th)
        .filter(|(rect, _)| rect.bottom() < text.y);
    if mark.is_none() {
        let name = Line::from(vec![
            Span::styled("◆ ", Style::default().fg(th.accent)),
            Span::styled(
                "orion",
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ),
        ]);
        lines.splice(0..0, [name, Line::from("")]);
        upgrade_row = upgrade_row.map(|row| row + 2);
        text = block(&lines);
    }
    let mark_rect = mark.as_ref().map_or(Rect::default(), |(rect, _)| *rect);
    paint_sky(f.buffer_mut(), area, text, mark_rect, t, th.accent);
    if let Some((rect, rows)) = mark {
        f.render_widget(Paragraph::new(rows), rect);
    }
    f.render_widget(Paragraph::new(lines).centered(), text);
    // A click anywhere on HOME goes back down to the grid, as Esc does;
    // on the first-run splash it lands focus on the (invisible) projects
    // panel, where Enter opens the first project.
    let hit = if app.home {
        HitTarget::FooterHome
    } else {
        HitTarget::PanelBg(Focus::Projects)
    };
    app.hits.push((area, hit));
    // Ahead of HOME's own, so it wins under the pointer.
    if let Some(row) = upgrade_row {
        let y = text.y + row as u16;
        if y < text.bottom() {
            app.hits.insert(
                0,
                (
                    Rect {
                        y,
                        height: 1,
                        ..text
                    },
                    HitTarget::FooterUpgrade,
                ),
            );
        }
    }
}

/// How wide the hunter is stretched across the sky, and the cloud a
/// little past him, in the span [`SkyFrame::place`] takes.
const HUNTER_SPAN: f32 = 2.2;
const DUST_SPAN: f32 = 2.35;

/// The sky above the words: its middle, and the cells it has to stretch
/// the hunter over.
struct SkyFrame {
    cx: f32,
    cy: f32,
    width: f32,
    above: f32,
}

impl SkyFrame {
    fn over(area: Rect, text: Rect) -> Self {
        let above = text.y.saturating_sub(area.y).max(4);
        Self {
            cx: f32::from(area.x) + f32::from(area.width) / 2.0,
            cy: f32::from(area.y) + f32::from(above) / 2.0,
            width: f32::from(area.width),
            above: f32::from(above),
        }
    }

    /// A figure `span` wide stretched to fill the sky, turned by `rot`.
    /// The x and y scales are independent; a terminal cell is ~2x taller
    /// than wide, hence the factor 2 on y.
    fn place(&self, span: f32, rot: f32) -> Placement {
        Placement {
            cx: self.cx,
            cy: self.cy,
            sx: span / (0.42 * self.width).max(4.0),
            sy: 2.0 * span / (1.6 * self.above).max(4.0),
            rot,
        }
    }
}

/// The constellation and its starfield across `area`, `t` seconds into
/// the scene: the hunter centered in the sky above `text` and stretched
/// to fill it, dust and stars both kept off a band around `text` so the
/// words sit on clear black. The first-run splash and the empty GRID's
/// welcome (`ui::launcher_view`) are both drawn over it.
pub fn draw_sky(buf: &mut Buffer, area: Rect, text: Rect, t: f32, accent: Color) {
    paint_sky(buf, area, text, Rect::default(), t, accent);
}

/// [`draw_sky`], with a second band kept clear inside the hunter: `mark`,
/// where the splash draws the name. An empty `mark` clears nothing.
fn paint_sky(buf: &mut Buffer, area: Rect, text: Rect, mark: Rect, t: f32, accent: Color) {
    let fade = fade_at(t);
    let pal = Palette::of(accent);
    let frame = SkyFrame::over(area, text);
    // The bands the dust, the stars and the sticks never touch.
    let band = |r: Rect, dx: u16, dy: u16| {
        if r.is_empty() {
            return r;
        }
        Rect {
            x: r.x.saturating_sub(dx),
            y: r.y.saturating_sub(dy),
            width: r.width + 2 * dx,
            height: r.height + 2 * dy,
        }
        .intersection(area)
    };
    let clear = [band(text, 3, 1), band(mark, MARK_CLEAR, 1)];

    // A breath of sway keeps the hunter alive and readable. Dust drifts.
    let sway = (t * 0.4).sin() * 0.02;
    let drift = t * 0.5;
    let cloud = frame.place(DUST_SPAN, 0.0);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let (hx, hy) = (i32::from(x), i32::from(y));
            if !in_sky(area, &clear, hx, hy) {
                continue;
            }
            let d = density(
                (f32::from(x) - cloud.cx) * cloud.sx,
                (f32::from(y) - cloud.cy) * cloud.sy,
                sway,
                drift,
            );
            let cell =
                dust_cell(hx, hy, d * fade, &pal).or_else(|| star_cell(hx, hy, t, fade, &pal));
            if let Some((ch, fg)) = cell {
                buf[(x, y)].set_char(ch).set_fg(fg);
            }
        }
    }
    // The hunter spans most of the sky, a little inside the cloud.
    let at = frame.place(HUNTER_SPAN, sway);
    paint_hunter(buf, area, &clear, at, t, fade, &pal);
}

/// The dust in cell (`x`, `y`) at density `d`, if any. Each cell is kept
/// or left out at random, the thinner the dust the likelier left out, so
/// the cloud reads as grain.
fn dust_cell(x: i32, y: i32, d: f32, pal: &Palette) -> Option<(char, Color)> {
    let level = DUST.iter().rposition(|&(from, ..)| d >= from)?;
    let v = (d - DUST_FLOOR) / (1.0 - DUST_FLOOR);
    let keep = 1.0 - GRAIN * (1.0 - (v * 2.2).min(1.0));
    (hash01(x, y, 4242) < keep).then(|| (DUST[level].1, pal.dust[level]))
}

/// The star in empty-sky cell (`x`, `y`), if any: sparse near stars on
/// their own slow twinkle phases, the rare accent sparkle among them, and
/// a fainter, steadier layer behind once the sky has faded in.
fn star_cell(x: i32, y: i32, t: f32, fade: f32, pal: &Palette) -> Option<(char, Color)> {
    let h = hash(x, y, 12_345);
    if h.is_multiple_of(NEAR_STARS) {
        let phase = ((h >> 8) % 8) as f32 * 0.8;
        let tw = ((t * 0.875 + phase).sin() * 0.5 + 0.5) * fade;
        return if tw <= 0.25 {
            None
        } else if (h >> 4).is_multiple_of(SPARKLES) {
            Some(('+', pal.accent))
        } else if tw > 0.8 {
            Some(('·', pal.near_bright))
        } else {
            Some(('.', pal.near_dim))
        };
    }
    if fade <= 0.5 {
        return None;
    }
    let h = hash(x, y, 777);
    h.is_multiple_of(FAR_STARS).then(|| {
        let lit = (t * 0.315 + ((h >> 8) % 16) as f32 * 0.4).sin() > 0.0;
        ('.', if lit { pal.far_lit } else { pal.far_dim })
    })
}

/// Where the hunter sits: the belt's cell, the scales from constellation
/// space to cells, and the sway.
#[derive(Clone, Copy)]
struct Placement {
    cx: f32,
    cy: f32,
    sx: f32,
    sy: f32,
    rot: f32,
}

impl Placement {
    /// The cell constellation point (`px`, `py`) lands in.
    fn cell(self, px: f32, py: f32) -> (i32, i32) {
        let (x, y) = rotate(px, py, self.rot);
        (
            (self.cx + x / self.sx).round() as i32,
            (self.cy + y / self.sy).round() as i32,
        )
    }
}

/// Whether cell (`x`, `y`) is in `area` and off every band in `clear`.
fn in_sky(area: Rect, clear: &[Rect], x: i32, y: i32) -> bool {
    let Ok(x) = u16::try_from(x) else {
        return false;
    };
    let Ok(y) = u16::try_from(y) else {
        return false;
    };
    let at = ratatui::layout::Position { x, y };
    area.contains(at) && !clear.iter().any(|band| band.contains(at))
}

fn paint_hunter(
    buf: &mut Buffer,
    area: Rect,
    clear: &[Rect],
    at: Placement,
    t: f32,
    fade: f32,
    pal: &Palette,
) {
    if fade < 0.2 {
        return;
    }
    let cells = STARS.map(|Star(px, py, ..)| at.cell(px, py));
    // Sword hanging from the belt (Alnitak), through the nebula.
    let sword = [
        cells[2],
        at.cell(-0.08, 0.22),
        at.cell(-0.06, 0.40),
        at.cell(-0.04, 0.56),
    ];
    let stick = if fade > 0.8 {
        pal.stick
    } else {
        pal.stick_fading
    };
    let figure = EDGES.iter().map(|&(a, b)| (cells[a], cells[b]));
    for (a, b) in figure.chain(sword.windows(2).map(|w| (w[0], w[1]))) {
        stroke(buf, area, clear, a, b, stick);
    }
    for (i, Star(_, _, ch, phase, _)) in STARS.iter().enumerate() {
        let (x, y) = cells[i];
        if !in_sky(area, clear, x, y) {
            continue;
        }
        // The stars shimmer in brightness alone, the glyphs holding still:
        // a touch over their hue at the top, 38% under it at the bottom.
        let tw = (t * 0.455 + phase).sin() * 0.5 + 0.5;
        let k = 1.3 * (1.0 - 0.38 * (1.0 - tw)) * fade;
        buf[(x as u16, y as u16)]
            .set_char(*ch)
            .set_fg(shade(pal.stars[i], k));
    }
    // M42 — the fuzzy middle of the sword.
    let (mx, my) = sword[2];
    if in_sky(area, clear, mx, my) && fade > 0.4 {
        buf[(mx as u16, my as u16)]
            .set_char('+')
            .set_fg(shade(M42, fade));
    }
}

/// A dotted stick from `a` to `b`, a dot in every other cell between
/// them, so the figure is traced rather than drawn.
fn stroke(buf: &mut Buffer, area: Rect, clear: &[Rect], a: (i32, i32), b: (i32, i32), fg: Color) {
    let steps = (a.0 - b.0)
        .unsigned_abs()
        .max((a.1 - b.1).unsigned_abs())
        .max(1);
    for i in (1..steps).step_by(2) {
        let u = i as f32 / steps as f32;
        let x = a.0 as f32 + (b.0 - a.0) as f32 * u;
        let y = a.1 as f32 + (b.1 - a.1) as f32 * u;
        let (cx, cy) = (x.round() as i32, y.round() as i32);
        if in_sky(area, clear, cx, cy) {
            buf[(cx as u16, cy as u16)].set_char('·').set_fg(fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// The name's rows: three cells high, O-R-I-O-N across, drawn in
    /// half blocks once the scene has faded in and in static before.
    #[test]
    fn the_wordmark_is_orion_in_half_blocks() {
        assert_eq!(LETTERS.len(), 5, "O-R-I-O-N");
        assert_eq!(LETTERS[0], LETTERS[3], "both Os");
        assert_eq!(mark_width(), 20);
        let rows: Vec<String> = (0..usize::from(MARK_ROWS))
            .map(|row| {
                wordmark_line(row, 60.0, 1.0, Theme::default())
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(
            rows,
            [
                "█▀█ █▀▄ ▀█▀ █▀█ █▄ █",
                "█ █ █▀▄  █  █ █ █ ▀█",
                "▀▀▀ ▀ ▀ ▀▀▀ ▀▀▀ ▀  ▀",
            ]
        );
        let fading: String = wordmark_line(0, 0.3, 0.3, Theme::default())
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(fading.contains('░') && !fading.contains('█'), "{fading}");
    }

    /// The name sits inside the hunter: in block letters where his chest
    /// has the room, letter by letter where it is narrower, and nowhere in
    /// a sky too small for either.
    #[test]
    fn the_name_takes_what_room_the_chest_has() {
        let th = Theme::default();
        let mark = |w: u16, h: u16| {
            let area = Rect::new(0, 0, w, h);
            let text = Rect::new(w / 2 - 8, h - 2, 16, 1);
            let frame = SkyFrame::over(area, text);
            let at = frame.place(HUNTER_SPAN, 0.0);
            let shoulders = at.cell(0.0, CHEST_SPAN.0).1;
            let belt = at.cell(0.0, CHEST_SPAN.1).1;
            chest_mark(&frame, area, 60.0, th).map(|(rect, rows)| {
                assert!(
                    i32::from(rect.y) > shoulders,
                    "{rect:?} under the shoulders"
                );
                assert!(i32::from(rect.bottom()) <= belt, "{rect:?} over the belt");
                assert_eq!(rect.x + rect.width / 2, w / 2, "centered");
                (rect.width, rows.len())
            })
        };
        assert_eq!(mark(190, 50), Some((20, 3)));
        assert_eq!(mark(120, 40), Some((9, 1)));
        assert_eq!(mark(60, 20), None);
    }

    const SLATE: Color = Color::Indexed(110);

    fn sky() -> Buffer {
        let area = Rect::new(0, 0, 80, 24);
        let text = Rect::new(20, 20, 40, 3);
        let mut buf = Buffer::empty(area);
        draw_sky(&mut buf, area, text, 60.0, SLATE);
        buf
    }

    #[test]
    fn the_sky_paints_dust_and_the_hunter() {
        let buf = sky();
        let dust_colors = Palette::of(SLATE).dust;
        let dust = buf
            .content
            .iter()
            .filter(|c| dust_colors.contains(&c.fg))
            .count();
        let stars = buf.content.iter().filter(|c| c.symbol() == "•").count();
        assert!(dust > 40, "{dust} dust cells");
        assert!(stars >= 3, "{stars} constellation stars");
    }

    #[test]
    fn the_sky_stays_mostly_dark() {
        // Faint is the point: the cloud the splash used to fill reads as
        // grain now, and most of the sky is left black.
        let buf = sky();
        let painted = buf.content.iter().filter(|c| c.symbol() != " ").count();
        assert!(
            painted < buf.content.len() / 4,
            "{painted} of {} cells",
            buf.content.len()
        );
    }

    #[test]
    fn the_dust_stays_under_a_fifth_of_the_accent() {
        for name in crate::theme::THEMES {
            let accent = Theme::by_name(name).accent;
            let rgb = rgb_of(accent);
            for c in Palette::of(accent).dust {
                let Color::Rgb(r, g, b) = c else {
                    panic!("{name}: dust is truecolor, got {c:?}");
                };
                for (v, a) in [r, g, b].into_iter().zip(rgb) {
                    assert!(f32::from(v) <= a * 0.2, "{name}: {c:?} vs {rgb:?}");
                }
            }
        }
    }
}
