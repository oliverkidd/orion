//! First-run splash: a procedurally animated Orion filling the body while
//! the project tree is empty. Every cell is computed per frame — a dusty
//! molecular cloud (belt, sword, Barnard's Loop) modulated by value noise
//! picks a glyph from a dust ramp, the hunter's seven stars and sticks sit
//! on top, a hashed starfield twinkles in the empty sky, and the wordmark
//! materializes in a carved-out band the dust never paints.
//! Indexed colors only: Terminal.app has no truecolor.
//!
//! The event loop ticks a repaint every [`FRAME`] while [`App::splash_active`]
//! holds; the scene itself is a pure function of elapsed time, so a missed
//! frame skips ahead instead of stuttering. The sky on its own
//! ([`draw_sky`]) is also what the empty GRID's welcome is drawn over,
//! ticked while [`App::welcome_active`] holds.

use crate::app::{App, Focus, HitTarget};
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

/// Glyph ramp, thin dust -> bright core. Single-width chars only.
const RAMP: &[char] = &['.', ':', '·', '+', '*', '*', 'o', '@'];
/// 256-color ramp under `RAMP`: winter indigo -> steel -> a rose core
/// (the sword's nebula), same family as the old disc.
const DUST: &[u8] = &[17, 18, 54, 61, 97, 133, 139, 181];
/// Wordmark gradient, swept left to right.
const MARK: &[u8] = &[99, 105, 141, 177, 213, 219];

/// 5-row block bitmaps for O R I O N.
const LETTERS: &[&[&str; 5]] = &[
    &[".###.", "#...#", "#...#", "#...#", ".###."],
    &["####.", "#...#", "####.", "#.#..", "#..#."],
    &["###", ".#.", ".#.", ".#.", "###"],
    &[".###.", "#...#", "#...#", "#...#", ".###."],
    &["#...#", "##..#", "#.#.#", "#..##", "#...#"],
];

/// The hunter in constellation space: (0, 0) is the belt, +x right, +y
/// down the sword. Order is the drawing order for the sticks.
const STARS: &[(f32, f32, u8, f32)] = &[
    (-0.56, -0.70, 210, 0.0), // Betelgeuse — shoulder, warm
    (0.50, -0.60, 189, 1.1),  // Bellatrix — shoulder
    (-0.30, 0.08, 231, 2.0),  // Alnitak — belt
    (0.00, 0.03, 231, 2.7),   // Alnilam — belt
    (0.30, -0.02, 231, 3.4),  // Mintaka — belt
    (-0.44, 0.78, 147, 4.2),  // Saiph — knee
    (0.54, 0.74, 159, 5.0),   // Rigel — foot, ice
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

/// Dust density at physical offset (dx, dy) from the hunter's belt: the
/// molecular cloud along the belt and sword, Barnard's Loop as a faint
/// ring, and a halo so the sky still fills the way the old disc did.
/// The figure sways with `sway`; noise shears with `drift` so the wisps
/// move without spinning the hunter on his head.
fn density(dx: f32, dy: f32, sway: f32, drift: f32) -> f32 {
    let (x, y) = rotate(dx, dy, sway);
    let r = (x * x + y * y).sqrt().max(0.001);
    let belt = (-(x * x * 2.4 + (y - 0.04) * (y - 0.04) * 16.0)).exp();
    let sword = (-((x + 0.07) * (x + 0.07) * 18.0 + (y - 0.38) * (y - 0.38) * 7.5)).exp();
    let loop_r = ((x * 1.05) * (x * 1.05) + (y * 0.88) * (y * 0.88)).sqrt();
    let barnard = (-((loop_r - 0.84) * 5.0) * ((loop_r - 0.84) * 5.0)).exp() * 0.52;
    let halo = (-r * 1.15).exp() * 0.55;
    let core = (-r * r * 9.0).exp() * 0.12;
    let (sa, ca) = (drift * 0.3).sin_cos();
    let (nx, ny) = (dx * ca - dy * sa, dx * sa + dy * ca);
    let wisp = 0.55 + 0.45 * vnoise(nx * 3.0 + 7.0, ny * 3.0 + 3.0, 991);
    (wisp * (belt * 0.75 + sword * 0.7 + barnard + halo + core)).min(1.0)
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

/// The wordmark's color at `u` (0 -> 1) across it: the gradient, with the
/// slow shine sweeping through once the scene has faded in.
fn mark_color(u: f32, t: f32, fade: f32) -> Color {
    let shine = (u * 5.0 - t * 1.4).sin() > 0.93;
    if shine && fade >= 1.0 {
        return Color::Indexed(231); // near-white glint
    }
    let gi = (u * (MARK.len() as f32 - 1.0)).round() as usize;
    Color::Indexed(MARK[gi])
}

/// `word` in the wordmark's gradient and shine, one cell per letter: the
/// name where there is no room for the block letters, or no call for
/// them.
pub fn wordmark_word(word: &str, t: f32) -> Vec<Span<'static>> {
    let fade = fade_at(t);
    let n = word.chars().count().max(1) as f32;
    word.chars()
        .enumerate()
        .map(|(i, ch)| {
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(mark_color(i as f32 / n, t, fade))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// One wordmark row as per-cell spans: gradient across the word, a slow
/// shine sweeping through, and the blocks materializing from static
/// (`░` -> `▒` -> `█`) while the scene fades in.
fn wordmark_line(row: usize, t: f32, fade: f32) -> Line<'static> {
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
                        .fg(mark_color(u, t, fade))
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
            lines.push(wordmark_line(row, t, fade));
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
            Span::styled(
                k.to_string(),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}"), Style::default().fg(th.dim)),
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
}

/// The constellation and its starfield across `area`, `t` seconds into
/// the scene: the hunter centered in the sky above `text` and stretched
/// to fill it, dust and stars both kept off a band around `text` so the
/// words sit on clear black. The first-run splash and the empty GRID's
/// welcome (`ui::launcher_view`) are both drawn over it.
pub fn draw_sky(buf: &mut Buffer, area: Rect, text: Rect, t: f32, accent: Color) {
    let fade = fade_at(t);
    // ---- hunter centered in the sky above the text ----
    let above = text.y.saturating_sub(area.y).max(4);
    let cx = f32::from(area.x) + f32::from(area.width) / 2.0;
    let cy = f32::from(area.y) + f32::from(above) / 2.0;
    // Independent x/y scales stretch the figure to fill the sky; a terminal
    // cell is ~2x taller than wide, hence the factor 2 on y.
    let sx = 2.35 / (0.42 * f32::from(area.width)).max(4.0);
    let sy = 2.0 * 2.35 / (1.6 * f32::from(above)).max(4.0);
    // Text carve: rows the dust and stars never touch.
    let carve = Rect {
        x: text.x.saturating_sub(3),
        y: text.y.saturating_sub(1),
        width: text.width + 6,
        height: text.height + 2,
    }
    .intersection(area);

    // A few degrees of sway keeps the hunter readable. Dust still drifts.
    let sway = (t * 0.4).sin() * 0.06;
    let drift = t * 0.22;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if x >= carve.left() && x < carve.right() && y >= carve.top() && y < carve.bottom() {
                continue;
            }
            let dx = (f32::from(x) - cx) * sx;
            let dy = (f32::from(y) - cy) * sy;
            let d = density(dx, dy, sway, drift) * fade;
            if d >= 0.055 {
                let v = ((d - 0.055) / 0.945).clamp(0.0, 1.0);
                let i = (v * (RAMP.len() as f32 - 1.0)).round() as usize;
                buf[(x, y)]
                    .set_char(RAMP[i])
                    .set_fg(Color::Indexed(DUST[i]));
                continue;
            }
            // Empty sky: sparse stars on their own twinkle phases, plus
            // the rare accent-colored sparkle.
            let h = hash(i32::from(x), i32::from(y), 12_345);
            if !h.is_multiple_of(53) {
                continue;
            }
            let phase = ((h >> 8) % 8) as f32 * 0.8;
            let tw = ((t * 2.5 + phase).sin() * 0.5 + 0.5) * fade;
            if tw <= 0.45 {
                continue;
            }
            if (h >> 4).is_multiple_of(111) {
                buf[(x, y)].set_char('+').set_fg(accent);
            } else if tw > 0.8 {
                buf[(x, y)].set_char('·').set_fg(Color::Indexed(189));
            } else {
                buf[(x, y)].set_char('.').set_fg(Color::Indexed(60));
            }
        }
    }
    // The hunter is drawn on a tighter scale than the cloud, so the seven
    // stars span the sky instead of sitting in a smudge at the core.
    let hsx = 1.45 / (0.42 * f32::from(area.width)).max(4.0);
    let hsy = 2.0 * 1.45 / (1.6 * f32::from(above)).max(4.0);
    paint_hunter(buf, area, carve, cx, cy, hsx, hsy, sway, t, fade);
}

fn to_cell(cx: f32, cy: f32, sx: f32, sy: f32, px: f32, py: f32, rot: f32) -> (i32, i32) {
    let (x, y) = rotate(px, py, rot);
    ((cx + x / sx).round() as i32, (cy + y / sy).round() as i32)
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
    cx: f32,
    cy: f32,
    sx: f32,
    sy: f32,
    rot: f32,
    t: f32,
    fade: f32,
) {
    if fade < 0.2 {
        return;
    }
    let cells: Vec<(i32, i32)> = STARS
        .iter()
        .map(|&(px, py, _, _)| to_cell(cx, cy, sx, sy, px, py, rot))
        .collect();
    for &(a, b) in EDGES {
        stroke(buf, area, carve, cells[a], cells[b], fade);
    }
    // Sword hanging from the belt, through the nebula.
    let sword = [
        to_cell(cx, cy, sx, sy, -0.08, 0.22, rot),
        to_cell(cx, cy, sx, sy, -0.06, 0.40, rot),
        to_cell(cx, cy, sx, sy, -0.04, 0.56, rot),
    ];
    stroke(buf, area, carve, cells[2], sword[0], fade);
    stroke(buf, area, carve, sword[0], sword[1], fade);
    stroke(buf, area, carve, sword[1], sword[2], fade);
    for (i, &(_px, _py, color, phase)) in STARS.iter().enumerate() {
        let (x, y) = cells[i];
        if !in_sky(area, carve, x, y) {
            continue;
        }
        let tw = ((t * 2.2 + phase).sin() * 0.5 + 0.5) * fade;
        let (ch, fg) = if i == 0 {
            // Betelgeuse holds a warmer pulse than the ice of Rigel.
            if tw > 0.55 {
                ('@', Color::Indexed(color))
            } else {
                ('o', Color::Indexed(203))
            }
        } else if i == 6 {
            if tw > 0.55 {
                ('@', Color::Indexed(color))
            } else {
                ('*', Color::Indexed(153))
            }
        } else if tw > 0.55 {
            ('*', Color::Indexed(231))
        } else {
            ('+', Color::Indexed(color))
        };
        buf[(x as u16, y as u16)].set_char(ch).set_fg(fg);
    }
    // M42 — the fuzzy middle of the sword.
    let (mx, my) = sword[1];
    if in_sky(area, carve, mx, my) && fade > 0.4 {
        buf[(mx as u16, my as u16)]
            .set_char('o')
            .set_fg(Color::Indexed(175));
    }
}

fn stroke(buf: &mut Buffer, area: Rect, carve: Rect, a: (i32, i32), b: (i32, i32), fade: f32) {
    let steps = (a.0 - b.0)
        .unsigned_abs()
        .max((a.1 - b.1).unsigned_abs())
        .max(1);
    for i in 1..steps {
        let u = i as f32 / steps as f32;
        let x = a.0 as f32 + (b.0 - a.0) as f32 * u;
        let y = a.1 as f32 + (b.1 - a.1) as f32 * u;
        let (cx, cy) = (x.round() as i32, y.round() as i32);
        if !in_sky(area, carve, cx, cy) {
            continue;
        }
        let cell = &mut buf[(cx as u16, cy as u16)];
        // Don't stamp over a named star or a bright dust core.
        if matches!(cell.symbol(), "@" | "o" | "*") {
            continue;
        }
        let dx = (b.0 - a.0) as f32;
        let dy = (b.1 - a.1) as f32;
        let ax = dx.abs();
        let ay = dy.abs();
        let ch = if ay < ax * 0.45 {
            '-'
        } else if ax < ay * 0.45 {
            '|'
        } else if dx.signum() == dy.signum() {
            '\\'
        } else {
            '/'
        };
        cell.set_char(ch)
            .set_fg(Color::Indexed(if fade > 0.8 { 146 } else { 103 }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    #[test]
    fn the_wordmark_is_orion_not_nebula() {
        assert_eq!(LETTERS.len(), 5, "O-R-I-O-N");
        assert_eq!(LETTERS[0][0], ".###.", "O");
        assert_eq!(LETTERS[1][0], "####.", "R");
        assert_eq!(LETTERS[2][0], "###", "I");
        assert_eq!(LETTERS[4][0], "#...#", "N");
        let row: String = wordmark_line(0, 60.0, 1.0)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(row.contains('█'));
        assert!(!row.contains("NEBULA"));
    }

    #[test]
    fn the_sky_paints_dust_and_the_hunter() {
        let area = Rect::new(0, 0, 80, 24);
        let text = Rect::new(20, 20, 40, 3);
        let mut buf = Buffer::empty(area);
        draw_sky(&mut buf, area, text, 60.0, Color::Indexed(81));
        let mut dust = 0usize;
        let mut named = 0usize;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let ch = buf[(x, y)].symbol();
                if RAMP.iter().any(|g| g.to_string() == ch) || ch == "·" {
                    dust += 1;
                }
                if ch == "@" || ch == "*" {
                    named += 1;
                }
            }
        }
        assert!(dust > 40, "{dust} sky cells");
        assert!(named >= 3, "{named} constellation stars");
    }
}
