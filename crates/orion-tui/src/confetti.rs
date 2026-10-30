//! MERGE CONFETTI: a pull request merged from orion (`⌘X` — not auto-merge,
//! which lands later, on GitHub) fires a cannon from the bottom middle of
//! the screen: paper in the merged purples with a few white sparks, and a
//! tag naming the pull request rising from the muzzle. It is drawn last,
//! over every modal, so closing the PULL REQUESTS MODAL mid-burst leaves it
//! flying; it takes no keys and no clicks. A second merge mid-burst fires
//! another volley on top.
//!
//! A piece's path is closed-form in time — linear drag, little on the shot
//! and a lot once it flutters down, swaying as it falls — so, like the
//! splash, a frame is a pure function of elapsed time and a missed tick
//! skips ahead. Space is measured in columns and half-rows, so a cell is two
//! units tall and the arcs come out round. Launch speed and gravity scale
//! with the screen's height, so the burst tops out at the same fraction of
//! a small split as of a full window.
//!
//! The event loop ticks a repaint every [`FRAME`] while [`App::confetti`]
//! holds a burst, and drops it once [`Confetti::done`]. With animations off
//! nothing fires.

use crate::app::App;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use std::time::{Duration, Instant};

/// Repaint cadence while a burst is in the air.
pub const FRAME: Duration = Duration::from_millis(33);

/// The screen height (rows) the speeds below are tuned for.
const BASE_ROWS: f32 = 34.0;
/// Gravity, in half-rows per second squared.
const GRAVITY: f32 = 60.0;
/// Drag on the way up: the shot carries.
const DRAG_UP: f32 = 0.5;
/// Drag on the way down: paper flutters.
const DRAG_DOWN: f32 = 3.2;
/// How far either side of straight up a piece may leave, in radians.
const CONE: f32 = 0.5;
/// Launch speeds, half-rows per second at [`BASE_ROWS`]: the slowest, and
/// how much faster the fastest goes.
const SPEED: f32 = 78.0;
const SPEED_SPREAD: f32 = 34.0;
/// Volleys a merge fires: seconds after it, and how many pieces.
const VOLLEYS: [(f32, usize); 3] = [(0.0, 36), (0.12, 24), (0.26, 16)];
/// Seconds a piece takes to fade into the cell behind it, at the end of
/// its life.
const FADE: f32 = 0.8;
/// The tag's life: seconds to fade in, to start fading out, and gone.
const TAG_IN: f32 = 0.2;
const TAG_OUT: f32 = 1.9;
const TAG_END: f32 = 2.4;
/// The tag climbs a row every `TAG_STEP` seconds, `TAG_RISE` rows in all.
const TAG_STEP: f32 = 0.15;
const TAG_RISE: f32 = 2.0;
/// The tag's ground, at full strength.
const TAG_BG: (u8, u8, u8) = (59, 33, 96);

/// What a piece is cut as: the glyphs it turns through as it spins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Strip,
    Dot,
    Diamond,
    Star,
}

impl Kind {
    fn frames(self) -> &'static [&'static str] {
        match self {
            Kind::Strip => &["▬", "▞", "▮", "▚"],
            Kind::Dot => &["•", "●", "•", "·"],
            Kind::Diamond => &["◆", "◇"],
            Kind::Star => &["✦", "✧", "⋆", "✧"],
        }
    }
}

/// One piece of paper, as fired: everything its path needs.
#[derive(Debug, Clone)]
struct Piece {
    /// Seconds after the burst's epoch it leaves the muzzle.
    born: f32,
    /// Columns either side of the middle it leaves from.
    x0: f32,
    /// Launch velocity, half-rows per second at [`BASE_ROWS`]; up is
    /// negative.
    vx: f32,
    vy: f32,
    kind: Kind,
    /// Index into [`palette`].
    color: usize,
    /// Glyph frames per second, and where in them it starts.
    spin: f32,
    phase: f32,
    /// The fall's sway: how fast, and how far (half-rows per second).
    sway_f: f32,
    sway_a: f32,
    /// Seconds it lives.
    life: f32,
}

impl Piece {
    /// Where it is `t` seconds after leaving the muzzle, from the muzzle,
    /// at [`BASE_ROWS`]: columns across, half-rows down.
    fn at(&self, t: f32) -> (f32, f32) {
        let fall = GRAVITY / DRAG_UP;
        // Up until the drag and gravity stop it, then down from there.
        let apex = (1.0 - self.vy * DRAG_UP / GRAVITY).ln() / DRAG_UP;
        let up = t.min(apex);
        let carried = (1.0 - (-DRAG_UP * up).exp()) / DRAG_UP;
        let mut x = self.vx * carried;
        let mut y = fall * up + (self.vy - fall) * carried;
        if t > apex {
            let u = t - apex;
            let fall = GRAVITY / DRAG_DOWN;
            let carried = (1.0 - (-DRAG_DOWN * u).exp()) / DRAG_DOWN;
            x += self.vx * (-DRAG_UP * apex).exp() * carried
                + self.sway_a / self.sway_f
                    * ((apex * self.sway_f + self.phase).cos()
                        - (t * self.sway_f + self.phase).cos());
            y += fall * u - fall * carried;
        }
        (x, y)
    }

    fn glyph(&self, t: f32) -> &'static str {
        let frames = self.kind.frames();
        frames[(t * self.spin + self.phase) as usize % frames.len()]
    }
}

/// The tag a merge raises: what it says, and when.
#[derive(Debug, Clone)]
struct Tag {
    born: f32,
    text: String,
}

/// A burst in the air: every piece fired since the first merge of it.
#[derive(Debug, Clone)]
pub struct Confetti {
    epoch: Instant,
    pieces: Vec<Piece>,
    /// The newest merge's tag; an older one gives way.
    tag: Option<Tag>,
    /// Seconds after the epoch the last piece and the tag are gone.
    ends: f32,
}

impl Confetti {
    pub fn new(now: Instant) -> Self {
        Self {
            epoch: now,
            pieces: Vec::new(),
            tag: None,
            ends: 0.0,
        }
    }

    fn elapsed(&self, now: Instant) -> f32 {
        now.saturating_duration_since(self.epoch).as_secs_f32()
    }

    /// Fire a merge's volleys from `now`, its tag saying `text`. `seed`
    /// cuts the paper: the same seed, the same burst.
    pub fn fire(&mut self, now: Instant, seed: u64, text: String) {
        let at = self.elapsed(now);
        let mut rng = Rng(seed);
        for (after, count) in VOLLEYS {
            for _ in 0..count {
                let piece = cut(&mut rng, at + after);
                self.ends = self.ends.max(piece.born + piece.life);
                self.pieces.push(piece);
            }
        }
        self.ends = self.ends.max(at + TAG_END);
        self.tag = Some(Tag { born: at, text });
    }

    /// Every piece has landed or faded, and the tag with them.
    pub fn done(&self, now: Instant) -> bool {
        self.elapsed(now) >= self.ends
    }

    /// Paint the burst as it is at `now` over whatever `buf` already holds
    /// in `area`: a piece takes its cell's glyph and color, the cell's
    /// ground kept.
    pub fn draw(&self, buf: &mut Buffer, area: Rect, now: Instant, th: Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let elapsed = self.elapsed(now);
        let scale = area.height as f32 / BASE_ROWS;
        let mid = area.width as f32 / 2.0;
        let floor = area.height as f32 * 2.0 - 0.5;
        let colors = palette(th);
        for p in &self.pieces {
            let t = elapsed - p.born;
            if t < 0.0 || t >= p.life {
                continue;
            }
            let (dx, dy) = p.at(t);
            let (x, y) = (mid + p.x0 + dx * scale, floor + dy * scale);
            if x < 0.0 || y < 0.0 || x >= area.width as f32 || y >= area.height as f32 * 2.0 {
                continue;
            }
            let Some(cell) = buf.cell_mut((area.x + x as u16, area.y + (y / 2.0) as u16)) else {
                continue;
            };
            let fading = ((t - (p.life - FADE)) / FADE).clamp(0.0, 1.0);
            let fg = mix(colors[p.color], cell.bg, fading);
            cell.set_symbol(p.glyph(t)).set_fg(fg);
            cell.modifier = Modifier::empty();
        }
        if let Some(tag) = &self.tag {
            draw_tag(buf, area, elapsed - tag.born, &tag.text, th);
        }
    }
}

/// The tag over the muzzle: in, up two rows, held, and out.
fn draw_tag(buf: &mut Buffer, area: Rect, t: f32, text: &str, th: Theme) {
    if !(0.0..TAG_END).contains(&t) {
        return;
    }
    let strength = if t < TAG_IN {
        t / TAG_IN
    } else if t > TAG_OUT {
        1.0 - (t - TAG_OUT) / (TAG_END - TAG_OUT)
    } else {
        1.0
    };
    let rise = (t / TAG_STEP).min(TAG_RISE) as u16;
    let Some(y) = area.bottom().checked_sub(2 + rise).filter(|y| *y >= area.y) else {
        return;
    };
    let width = text.chars().count() as u16;
    let x0 = area.x + area.width.saturating_sub(width) / 2;
    let (r, g, b) = TAG_BG;
    for (i, ch) in text.chars().enumerate() {
        let x = x0 + i as u16;
        if x >= area.right() {
            break;
        }
        let Some(cell) = buf.cell_mut((x, y)) else {
            continue;
        };
        let ground = cell.bg;
        let mut tmp = [0; 4];
        cell.set_symbol(ch.encode_utf8(&mut tmp))
            .set_bg(mix(Color::Rgb(r, g, b), ground, 1.0 - strength))
            .set_fg(mix(th.merged_sweep[2], ground, 1.0 - strength));
        cell.modifier = Modifier::BOLD;
    }
}

/// The paper's colors: the merged purples, heavier on the ends of the
/// ramp, a periwinkle, and white for sparks.
fn palette(th: Theme) -> [Color; 7] {
    let [merged, mid, light] = th.merged_sweep;
    [
        merged,
        mid,
        light,
        Color::Indexed(105),
        merged,
        light,
        Color::Indexed(231),
    ]
}

/// `c` taken `t` of the way to `ground` (black for the terminal's own
/// background), in truecolor; `c` untouched at zero.
fn mix(c: Color, ground: Color, t: f32) -> Color {
    if t <= 0.0 {
        return c;
    }
    let (Some(from), to) = (
        crate::theme::rgb(c),
        crate::theme::rgb(ground).unwrap_or((0, 0, 0)),
    ) else {
        return c;
    };
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t.min(1.0)).round() as u8;
    Color::Rgb(lerp(from.0, to.0), lerp(from.1, to.1), lerp(from.2, to.2))
}

/// One piece, cut at random, leaving the muzzle `born` seconds in.
fn cut(rng: &mut Rng, born: f32) -> Piece {
    // Mostly straight up, thinning out toward a cone's edge.
    let angle = (rng.unit() + rng.unit() - 1.0) * CONE;
    let speed = SPEED + rng.unit() * SPEED_SPREAD;
    let kind = match rng.unit() {
        r if r < 0.5 => Kind::Strip,
        r if r < 0.7 => Kind::Dot,
        r if r < 0.85 => Kind::Diamond,
        _ => Kind::Star,
    };
    Piece {
        born,
        x0: (rng.unit() - 0.5) * 2.0,
        vx: angle.sin() * speed,
        vy: -angle.cos() * speed,
        kind,
        color: (rng.unit() * 7.0) as usize % 7,
        spin: 5.0 + rng.unit() * 9.0,
        phase: rng.unit() * std::f32::consts::TAU,
        sway_f: 2.0 + rng.unit() * 2.5,
        sway_a: 5.0 + rng.unit() * 9.0,
        life: 2.5 + rng.unit() * 0.9,
    }
}

/// SplitMix64 counted up from a seed: enough randomness to cut paper, and
/// repeatable.
struct Rng(u64);

impl Rng {
    /// The next number in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(1);
        (crate::branch_name::splitmix64(self.0) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Pull request `number` just merged into `base` from here: fire, on top
/// of any burst still in the air. Nothing with animations off.
pub fn fire(app: &mut App, number: u64, base: &str) {
    if !app.animations {
        return;
    }
    let now = Instant::now();
    let into = if base.is_empty() {
        String::new()
    } else {
        format!(" into {base}")
    };
    app.confetti.get_or_insert_with(|| Confetti::new(now)).fire(
        now,
        crate::branch_name::next_seed(),
        format!(" ✓ Merged #{number}{into} "),
    );
    app.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(seed: u64) -> (Confetti, Instant) {
        let now = Instant::now();
        let mut c = Confetti::new(now);
        c.fire(now, seed, " ✓ Merged #482 into main ".into());
        (c, now)
    }

    fn screen(c: &Confetti, area: Rect, now: Instant) -> Buffer {
        let mut buf = Buffer::empty(area);
        c.draw(&mut buf, area, now, Theme::default());
        buf
    }

    fn painted(buf: &Buffer) -> Vec<(u16, u16)> {
        let area = buf.area;
        let mut at = Vec::new();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if buf[(x, y)].symbol() != " " {
                    at.push((x, y));
                }
            }
        }
        at
    }

    #[test]
    fn pieces_rise_then_fall() {
        let (c, _) = burst(7);
        for p in &c.pieces {
            let apex = (1.0 - p.vy * DRAG_UP / GRAVITY).ln() / DRAG_UP;
            let (_, top) = p.at(apex);
            assert!(top < -30.0, "the shot carries: apex at {top}");
            let (_, early) = p.at(apex * 0.5);
            let (_, late) = p.at(apex + 1.0);
            assert!(early > top && late > top, "rises to the apex, then falls");
            // Within the screen it was tuned for.
            assert!(top > -2.0 * BASE_ROWS, "tops out on screen: {top}");
        }
    }

    #[test]
    fn the_path_is_continuous_over_the_apex() {
        let (c, _) = burst(3);
        for p in &c.pieces {
            let apex = (1.0 - p.vy * DRAG_UP / GRAVITY).ln() / DRAG_UP;
            let (x0, y0) = p.at(apex - 0.001);
            let (x1, y1) = p.at(apex + 0.001);
            assert!((x1 - x0).abs() < 0.2 && (y1 - y0).abs() < 0.2);
        }
    }

    #[test]
    fn it_leaves_from_the_bottom_middle() {
        let (c, now) = burst(11);
        let area = Rect::new(0, 0, 120, 40);
        let buf = screen(&c, area, now + Duration::from_millis(150));
        // The tag is bold; paper never is.
        let pieces: Vec<_> = painted(&buf)
            .into_iter()
            .filter(|&(x, y)| !buf[(x, y)].modifier.contains(Modifier::BOLD))
            .collect();
        assert!(!pieces.is_empty(), "the first volley is out");
        for (x, y) in pieces {
            assert!((50..70).contains(&x), "near the middle: column {x}");
            assert!(y > 25, "near the bottom: row {y}");
        }
    }

    #[test]
    fn it_fills_the_upper_screen_mid_flight() {
        let (c, now) = burst(5);
        let area = Rect::new(0, 0, 120, 40);
        let buf = screen(&c, area, now + Duration::from_millis(1200));
        let high = painted(&buf).iter().filter(|&&(_, y)| y < 20).count();
        assert!(high > 20, "pieces near the top at their apex: {high}");
    }

    #[test]
    fn the_tag_rises_from_the_muzzle_and_names_the_merge() {
        let (c, now) = burst(1);
        let area = Rect::new(0, 0, 80, 30);
        let row = |buf: &Buffer, y: u16| -> String {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        };
        let first = screen(&c, area, now + Duration::from_millis(10));
        assert!(row(&first, 28).contains("✓ Merged #482 into main"));
        let risen = screen(&c, area, now + Duration::from_millis(1000));
        assert!(row(&risen, 26).contains("✓ Merged #482 into main"));
        assert!(!row(&risen, 28).contains("Merged"));
    }

    #[test]
    fn a_burst_ends_and_draws_nothing_after() {
        let (c, now) = burst(9);
        assert!(!c.done(now + Duration::from_secs(2)));
        let after = now + Duration::from_secs(4);
        assert!(c.done(after));
        let area = Rect::new(0, 0, 80, 30);
        assert!(painted(&screen(&c, area, after)).is_empty());
    }

    #[test]
    fn a_second_merge_mid_burst_keeps_it_going() {
        let (mut c, now) = burst(2);
        let later = now + Duration::from_secs(2);
        c.fire(later, 8, " ✓ Merged #483 ".into());
        assert!(
            !c.done(now + Duration::from_secs(4)),
            "the second volley flies on"
        );
        let area = Rect::new(0, 0, 80, 30);
        let buf = screen(&c, area, later + Duration::from_millis(10));
        let row: String = (0..80).map(|x| buf[(x, 28)].symbol().to_string()).collect();
        assert!(
            row.contains("#483") && !row.contains("#482"),
            "the newest tag"
        );
    }

    #[test]
    fn the_same_seed_cuts_the_same_burst() {
        let (a, now) = burst(42);
        let mut b = Confetti::new(now);
        b.fire(now, 42, " ✓ Merged #482 into main ".into());
        let area = Rect::new(0, 0, 80, 30);
        let at = now + Duration::from_millis(700);
        assert_eq!(screen(&a, area, at), screen(&b, area, at));
    }

    #[test]
    fn a_tiny_screen_does_not_panic() {
        let (c, now) = burst(4);
        for (w, h) in [(0, 0), (1, 1), (3, 2), (10, 1)] {
            let area = Rect::new(2, 3, w, h);
            let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
            for ms in [0, 100, 900, 2000] {
                c.draw(
                    &mut buf,
                    area,
                    now + Duration::from_millis(ms),
                    Theme::default(),
                );
            }
        }
    }

    #[test]
    fn nothing_fires_with_animations_off() {
        let mut app = App::new();
        app.animations = false;
        fire(&mut app, 482, "main");
        assert!(app.confetti.is_none());
        app.animations = true;
        fire(&mut app, 482, "main");
        assert!(app.confetti.is_some());
    }
}
