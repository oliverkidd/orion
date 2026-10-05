//! First-run splash: a procedurally animated Orion filling the body while
//! the project tree is empty, kept faint so the words in front of it read
//! first. Every cell is computed per frame — a thin, grainy cloud (belt,
//! sword, Barnard's Loop) modulated by value noise, a sparse starfield
//! with a fainter layer behind it, the hunter's seven stars as dots on
//! dotted sticks — and the wordmark materializes in a carved-out band the
//! sky never paints.
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

/// 5-row block bitmaps for O R I O N.
const LETTERS: &[&[&str; 5]] = &[
    &[".###.", "#...#", "#...#", "#...#", ".###."],
    &["####.", "#...#", "####.", "#.#..", "#..#."],
    &["###", ".#.", ".#.", ".#.", "###"],
    &[".###.", "#...#", "#...#", "#...#", ".###."],
    &["#...#", "##..#", "#.#.#", "#..##", "#...#"],
];

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

/// One wordmark row as per-cell spans: gradient across the word, a slow
/// shine sweeping through, and the blocks materializing from static
/// (`░` -> `▒` -> `█`) while the scene fades in.
fn wordmark_line(row: usize, t: f32, fade: f32, th: Theme) -> Line<'static> {
    let steps = mark_steps(th);
    let width: usize = LETTERS.iter().map(|l| l[0].len()).sum::<usize>() + 2 * (LETTERS.len() - 1);
    let block = if fade < 0.5 {
        "░"
    } else if fade < 0.85 {
        "▒"
    } else {
        "█"
    };
    let mut spans = Vec::new();
    let mut col = 0usize;
    for (i, letter) in LETTERS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
            col += 2;
        }
        for ch in letter[row].chars() {
            if ch == '#' {
                let u = col as f32 / width as f32;
                spans.push(Span::styled(
                    block,
                    Style::default()
                        .fg(mark_color(u, t, fade, &steps))
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::raw(" "));
            }
            col += 1;
        }
    }
    Line::from(spans)
}

pub fn draw_splash(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    if area.width < 8 || area.height < 4 {
        return;
    }
    let t = scene_time(app, app.splash_epoch);
    let fade = fade_at(t);

    // ---- text block: wordmark, tagline, key hints, bottom-anchored ----
    let big = area.width >= 50 && area.height >= 18;
    let mut lines: Vec<Line> = Vec::new();
    if big {
        for row in 0..5 {
            lines.push(wordmark_line(row, t, fade, th));
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("◆ ", Style::default().fg(th.accent)),
            Span::styled(
                "orion",
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    // A newer release, under the wordmark: what it is and the key that
    // installs it — clickable, like the footer's `⇡ v…`.
    let upgrade_row = app.update_available.clone().map(|v| {
        let key = crate::hints::act(&app.keymap, crate::keymap::Action::Upgrade, "upgrade")
            .map_or_else(String::new, |h| format!(" · {} to upgrade", h.key));
        lines.push(Line::from(crate::ui::footer::upgrade_spans(
            app,
            &format!("⇡ v{v} available{key}"),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.len() - 1
    });
    lines.push(Line::from(""));
    if area.width >= 47 {
        lines.push(Line::from(Span::styled(
            "your agents keep running, even when you leave",
            Style::default().fg(th.dim),
        )));
        lines.push(Line::from(""));
    }
    // The ways into a project, each spelled from the live keymap: what
    // Enter opens, and the jump list (`⌘K`), whose rows are every project
    // and whose last row opens a folder. HOME leads with its way back.
    let key = |k: &str, label: &str| {
        vec![
            Span::styled(k.to_string(), crate::hints::key_style(th)),
            Span::styled(format!(" {label}"), crate::hints::does_style(th)),
        ]
    };
    let sep = || Span::styled("   ·   ", Style::default().fg(th.dim));
    let enter = crate::hints::key_or(&app.keymap, crate::keymap::Action::Activate, "Enter");
    let jump = crate::hints::key(&app.keymap, crate::keymap::Action::Palette);
    let mut hint = Vec::new();
    if app.home {
        // The way back down is the footer's to say, as it is everywhere
        // off the grid; the body offers the way elsewhere.
        if let Some(jump) = &jump {
            hint.extend(key(jump, "jump to any project"));
        }
    } else if !app.tree.has_projects() {
        // Started inside a repo, that repo is one key away; anywhere else
        // Enter asks for a folder. Nothing here asks for anything but one.
        match app.launch_repo_name() {
            Some(name) => {
                hint.extend(key(&enter, &format!("open {name}")));
                if let Some(jump) = &jump {
                    hint.push(sep());
                    hint.extend(key(jump, "another folder"));
                }
            }
            None => hint.extend(key(&enter, "open your first project")),
        }
    } else if app.projects_closed {
        // Every tab closed: the projects are all still there, so the jump
        // list has them; Enter still opens the repo orion was started in.
        let here = app.launch_repo.as_deref().and_then(|path| {
            app.tree
                .project_at_path(path)
                .map(|p| p.name.clone())
                .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        });
        if let Some(name) = here {
            hint.extend(key(&enter, &format!("open {name}")));
        }
        if let Some(jump) = &jump {
            if !hint.is_empty() {
                hint.push(sep());
            }
            hint.extend(key(jump, "your projects, or another folder"));
        }
    }
    lines.push(Line::from(hint));

    let block_w = (lines.iter().map(Line::width).max().unwrap_or(0) as u16).min(area.width);
    let block_h = (lines.len() as u16).min(area.height);
    let text = Rect {
        x: area.x + (area.width - block_w) / 2,
        y: area.y + area.height - block_h - u16::from(area.height > block_h),
        width: block_w,
        height: block_h,
    };

    draw_sky(f.buffer_mut(), area, text, t, th.accent);
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

/// The constellation and its starfield across `area`, `t` seconds into
/// the scene: the hunter centered in the sky above `text` and stretched
/// to fill it, dust and stars both kept off a band around `text` so the
/// words sit on clear black. The first-run splash and the empty GRID's
/// welcome (`ui::launcher_view`) are both drawn over it.
pub fn draw_sky(buf: &mut Buffer, area: Rect, text: Rect, t: f32, accent: Color) {
    let fade = fade_at(t);
    let pal = Palette::of(accent);
    // ---- hunter centered in the sky above the text ----
    let above = text.y.saturating_sub(area.y).max(4);
    let cx = f32::from(area.x) + f32::from(area.width) / 2.0;
    let cy = f32::from(area.y) + f32::from(above) / 2.0;
    // Independent x/y scales stretch a figure `span` wide to fill the sky;
    // a terminal cell is ~2x taller than wide, hence the factor 2 on y.
    let scales = |span: f32| {
        (
            span / (0.42 * f32::from(area.width)).max(4.0),
            2.0 * span / (1.6 * f32::from(above)).max(4.0),
        )
    };
    let (sx, sy) = scales(2.35);
    // Text carve: rows the dust and stars never touch.
    let carve = Rect {
        x: text.x.saturating_sub(3),
        y: text.y.saturating_sub(1),
        width: text.width + 6,
        height: text.height + 2,
    }
    .intersection(area);

    // A breath of sway keeps the hunter alive and readable. Dust drifts.
    let sway = (t * 0.4).sin() * 0.02;
    let drift = t * 0.5;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if x >= carve.left() && x < carve.right() && y >= carve.top() && y < carve.bottom() {
                continue;
            }
            let (hx, hy) = (i32::from(x), i32::from(y));
            let d = density(
                (f32::from(x) - cx) * sx,
                (f32::from(y) - cy) * sy,
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
    let (sx, sy) = scales(2.2);
    let at = Placement {
        cx,
        cy,
        sx,
        sy,
        rot: sway,
    };
    paint_hunter(buf, area, carve, at, t, fade, &pal);
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

fn in_sky(area: Rect, carve: Rect, x: i32, y: i32) -> bool {
    let Ok(x) = u16::try_from(x) else {
        return false;
    };
    let Ok(y) = u16::try_from(y) else {
        return false;
    };
    x >= area.left()
        && x < area.right()
        && y >= area.top()
        && y < area.bottom()
        && !(x >= carve.left() && x < carve.right() && y >= carve.top() && y < carve.bottom())
}

fn paint_hunter(
    buf: &mut Buffer,
    area: Rect,
    carve: Rect,
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
        stroke(buf, area, carve, a, b, stick);
    }
    for (i, Star(_, _, ch, phase, _)) in STARS.iter().enumerate() {
        let (x, y) = cells[i];
        if !in_sky(area, carve, x, y) {
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
    if in_sky(area, carve, mx, my) && fade > 0.4 {
        buf[(mx as u16, my as u16)]
            .set_char('+')
            .set_fg(shade(M42, fade));
    }
}

/// A dotted stick from `a` to `b`, a dot in every other cell between
/// them, so the figure is traced rather than drawn.
fn stroke(buf: &mut Buffer, area: Rect, carve: Rect, a: (i32, i32), b: (i32, i32), fg: Color) {
    let steps = (a.0 - b.0)
        .unsigned_abs()
        .max((a.1 - b.1).unsigned_abs())
        .max(1);
    for i in (1..steps).step_by(2) {
        let u = i as f32 / steps as f32;
        let x = a.0 as f32 + (b.0 - a.0) as f32 * u;
        let y = a.1 as f32 + (b.1 - a.1) as f32 * u;
        let (cx, cy) = (x.round() as i32, y.round() as i32);
        if in_sky(area, carve, cx, cy) {
            buf[(cx as u16, cy as u16)].set_char('·').set_fg(fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    #[test]
    fn the_wordmark_is_orion_not_nebula() {
        assert_eq!(LETTERS.len(), 5, "O-R-I-O-N");
        assert_eq!(LETTERS[0][0], ".###.", "O");
        assert_eq!(LETTERS[1][0], "####.", "R");
        assert_eq!(LETTERS[2][0], "###", "I");
        assert_eq!(LETTERS[4][0], "#...#", "N");
        let row: String = wordmark_line(0, 60.0, 1.0, Theme::default())
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(row.contains('█'));
        assert!(!row.contains("NEBULA"));
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
