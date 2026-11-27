//! DICTATION: say what goes in a text box, on this Mac.
//!
//! `⌘D` (`keymap::Action::Dictate`), or a click on the MIC CHIP every
//! text box wears on its top border, starts it; the same again, or Enter,
//! stops it and the words land at the caret as a paste would; Esc throws
//! them away. Held, `⌘D` listens only until it comes up. In a LOCKED PANE
//! the words go to the agent's own prompt, unsent, with a strip at the
//! pane's foot showing them as they come.
//!
//! The listening is `orion-listen` (`listen/orion-listen.swift`), which
//! the build compiles and bakes in ([`HELPER`]): written out once under
//! the data dir, one process per dictation, its findings one JSON line
//! each ([`Heard`]) read on a thread and handed to the loop. It runs
//! Apple's SpeechAnalyzer on macOS 26 and the older on-device
//! SFSpeechRecognizer on macOS 13 to 15 ([`Engine`]), and neither sends
//! audio anywhere. Off a Mac, over ssh, below macOS 13 or in a build with
//! no Swift toolchain there is no chip and `⌘D` does nothing.
//!
//! While it listens nothing is typed into the field: the box shows what
//! is heard as a GHOST at the caret — settled words in the text colour,
//! the engine's live guess muted after them — and its frame turns the
//! needs-you crimson. The words go in once, as a paste, when it finishes
//! (`event_loop::dictation_input`). Typing carries on meanwhile.

use crate::app::App;
use crate::theme::Theme;
use orion_core::SessionRef;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::Frame;
use serde::Deserialize;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The helper the build compiled for this Mac — empty off a Mac or with
/// no `swiftc` at build time (`build.rs`).
pub const HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/orion-listen"));

/// Whether dictation runs in this orion: switched on, on a Mac that can.
pub fn available(app: &App) -> bool {
    app.dictation_enabled && app.dictation_engine.is_some()
}

/// Whether `⌘D` is dictation's where the keys are now. Two modals had it
/// first and keep it — the PULL REQUESTS MODAL's ready/draft and the
/// NEEDS YOU BOARD's done (its reply panel is a text box, and dictates);
/// in them the MIC CHIP still works, a click away, and names no key.
pub fn key_free(app: &App) -> bool {
    use crate::app::Overlay;
    match &app.overlay {
        Some(Overlay::PullRequests(_)) => false,
        Some(Overlay::Palette(p)) => !(p.on_board() && p.board.reply.is_none()),
        _ => true,
    }
}

/// The microphone glyph (Nerd Font `md-microphone`), on the MIC CHIP.
pub const MIC: &str = "\u{f036c}";

/// How long a finish waits for the helper's last words before it takes
/// what it has.
pub const SETTLE_LIMIT: Duration = Duration::from_secs(3);

/// A press of `⌘D` held this long is HOLD TO TALK: letting go finishes.
pub const HOLD_AFTER: Duration = Duration::from_millis(400);

/// The bars of the level meter, quiet to loud.
const BARS: [char; 7] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'];

/// Which engine a Mac runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// SpeechAnalyzer, macOS 26 on.
    Analyzer,
    /// SFSpeechRecognizer on-device, macOS 13 to 15 — or a language
    /// SpeechAnalyzer has no model for. The chip says `basic`.
    Basic,
}

/// The engine this Mac would run, or None for no dictation here at all.
pub fn engine() -> Option<Engine> {
    static ENGINE: OnceLock<Option<Engine>> = OnceLock::new();
    *ENGINE.get_or_init(detect)
}

fn detect() -> Option<Engine> {
    if cfg!(test) || !cfg!(target_os = "macos") || HELPER.is_empty() {
        return None;
    }
    // Over ssh the keyboard and the microphone are another machine's.
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        return None;
    }
    let out = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    engine_for(&String::from_utf8_lossy(&out.stdout))
}

/// The engine for a `sw_vers -productVersion`.
pub fn engine_for(version: &str) -> Option<Engine> {
    let major: u32 = version.trim().split('.').next()?.parse().ok()?;
    match major {
        26.. => Some(Engine::Analyzer),
        13..=25 => Some(Engine::Basic),
        _ => None,
    }
}

/// One line from the helper (see the top of `orion-listen.swift`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "ev", rename_all = "kebab-case")]
pub enum Heard {
    Engine {
        name: String,
    },
    Download {
        progress: f64,
    },
    Listening,
    Level {
        v: f64,
    },
    Text {
        settled: String,
        live: String,
    },
    Done {
        text: String,
    },
    Error {
        kind: String,
        message: String,
    },
    /// The helper's output ended: it finished, crashed or was killed.
    #[serde(skip)]
    Gone,
    #[serde(other)]
    Other,
}

/// Where the words go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The text field with the caret (`text_input::CaretField::id`).
    Field(u64),
    /// The agent's prompt in the locked pane.
    Pane(SessionRef),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// The helper is starting: the mic, the model.
    Starting,
    /// The language's model is downloading, first use only.
    Downloading(f64),
    Listening,
    /// Stopped: the last words are settling, until [`Dictation::settle_by`].
    Settling,
}

/// One dictation, from the press that started it to the paste.
pub struct Dictation {
    /// Tells this dictation's helper lines from a finished one's.
    pub id: u64,
    pub target: Target,
    pub phase: Phase,
    pub settled: String,
    pub live: String,
    /// The last few mic levels, oldest first, for the meter.
    pub levels: VecDeque<f64>,
    pub started: Instant,
    /// HOLD TO TALK: `⌘D` is still down, and letting it go finishes.
    pub held: bool,
    /// When the press that started it went down.
    pub pressed: Instant,
    /// A finish's deadline for the last words.
    pub settle_by: Option<Instant>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

impl Dictation {
    /// A dictation with no helper behind it — the one [`start`] gives
    /// its process to, and what the tests drive.
    pub fn new(id: u64, target: Target, now: Instant) -> Self {
        Self {
            id,
            target,
            phase: Phase::Starting,
            settled: String::new(),
            live: String::new(),
            levels: VecDeque::new(),
            started: now,
            held: false,
            pressed: now,
            settle_by: None,
            child: None,
            stdin: None,
        }
    }

    /// Everything heard so far, settled then live.
    pub fn text(&self) -> String {
        format!("{}{}", self.settled, self.live).trim().to_string()
    }

    /// Ask the helper to finish: the mic closes and the last words settle.
    pub fn stop(&mut self, now: Instant) {
        if self.phase == Phase::Settling {
            return;
        }
        self.phase = Phase::Settling;
        self.held = false;
        self.settle_by = Some(now + SETTLE_LIMIT);
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
        }
    }

    /// Take in a line from the helper. True when it ends the dictation:
    /// `done`, an error, or the helper gone.
    pub fn hear(&mut self, heard: &Heard) -> bool {
        match heard {
            Heard::Download { progress }
                if self.phase == Phase::Starting || matches!(self.phase, Phase::Downloading(_)) =>
            {
                self.phase = Phase::Downloading(*progress);
            }
            Heard::Listening if self.phase != Phase::Settling => self.phase = Phase::Listening,
            Heard::Level { v } => {
                self.levels.push_back(*v);
                while self.levels.len() > 6 {
                    self.levels.pop_front();
                }
            }
            Heard::Text { settled, live } => {
                self.settled = settled.clone();
                self.live = live.clone();
            }
            Heard::Done { text } => {
                self.settled = text.clone();
                self.live.clear();
                return true;
            }
            Heard::Error { .. } | Heard::Gone => return true,
            _ => {}
        }
        false
    }

    /// The meter, six bars, newest on the right.
    pub fn meter(&self) -> String {
        let mut bars: String = self
            .levels
            .iter()
            .map(|v| BARS[((v.clamp(0.0, 1.0) * 6.0).round() as usize).min(6)])
            .collect();
        while bars.chars().count() < 6 {
            bars.insert(0, BARS[0]);
        }
        bars
    }

    /// `0:07`, how long it has listened.
    pub fn clock(&self, now: Instant) -> String {
        let secs = now.duration_since(self.started).as_secs();
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

impl Drop for Dictation {
    /// Gone without a finish — thrown away, the app quitting — the helper
    /// goes with it.
    fn drop(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

/// Where the helper is written out: under the data dir, named by its
/// hash, so an upgrade's new helper sits beside the old one rather than
/// over a binary something may be running.
fn helper_path() -> PathBuf {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(HELPER);
    let short: String = hash.iter().take(6).map(|b| format!("{b:02x}")).collect();
    orion_core::paths::data_dir()
        .join("bin")
        .join(format!("orion-listen-{short}"))
}

fn install() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = helper_path();
    if path.exists() {
        return Ok(path);
    }
    let dir = path.parent().expect("a file under bin/");
    std::fs::create_dir_all(dir)?;
    let partial = dir.join(format!(".orion-listen-{}", std::process::id()));
    std::fs::write(&partial, HELPER)?;
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// Start a dictation for `target`: the helper runs, and each line it
/// prints reaches the loop through `tx` as `(id, Heard)`.
pub fn start(
    id: u64,
    target: Target,
    words: &[String],
    tx: tokio::sync::mpsc::UnboundedSender<(u64, Heard)>,
    now: Instant,
) -> std::io::Result<Dictation> {
    let path = install()?;
    let mut cmd = Command::new(path);
    let words: Vec<&str> = words
        .iter()
        .map(|w| w.trim())
        .filter(|w| !w.is_empty() && !w.contains(','))
        .collect();
    if !words.is_empty() {
        cmd.arg("--words").arg(words.join(","));
    }
    // A recording in place of the microphone: the screenshot harness's
    // scenes (`scripts/shot/scenes/dictation-*`) talk through one.
    if let Some(file) = std::env::var_os("ORION_DICTATION_FILE") {
        cmd.arg("--file").arg(file);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().expect("piped");
    let stdin = child.stdin.take();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(heard) = serde_json::from_str::<Heard>(&line) {
                if tx.send((id, heard)).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send((id, Heard::Gone));
    });
    let mut dictation = Dictation::new(id, target, now);
    dictation.child = Some(child);
    dictation.stdin = stdin;
    Ok(dictation)
}

/// What the footer says for a helper's error.
pub fn error_line(kind: &str, message: &str) -> String {
    match kind {
        "mic-denied" => "Dictation can't hear you: turn on Orion (or your terminal) in System Settings › Privacy & Security › Microphone".into(),
        "speech-denied" => "Dictation needs Speech Recognition: turn on Orion (or your terminal) in System Settings › Privacy & Security › Speech Recognition".into(),
        _ => format!("Dictation stopped: {message}"),
    }
}

/// The Microphone page of System Settings.
pub const MIC_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

// ---------------------------------------------------------------- drawing

/// The box around a field: its frame's corners, found on the frame drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame4 {
    left: u16,
    right: u16,
    top: u16,
    bottom: u16,
}

fn symbol(buf: &Buffer, x: u16, y: u16) -> &str {
    buf.cell(Position::new(x, y)).map_or("", |c| c.symbol())
}

/// The frame around `area` — the box a field sits in — read off the
/// drawn frame: a top-right corner a few rows above the field and just
/// past its right edge, the top-left on that row, the bottom-right under
/// it. None for a field drawn in no box.
fn find_frame(buf: &Buffer, area: Rect) -> Option<Frame4> {
    let edge = buf.area;
    for top in (area.y.saturating_sub(6)..area.y).rev() {
        for right in area.right()..(area.right() + 4).min(edge.right()) {
            if !matches!(symbol(buf, right, top), "╮" | "┐") {
                continue;
            }
            let left = (area.x.saturating_sub(4)..area.x)
                .rev()
                .find(|&x| matches!(symbol(buf, x, top), "╭" | "┌"))?;
            let bottom = (area.bottom()..edge.bottom())
                .find(|&y| matches!(symbol(buf, right, y), "╯" | "┘"))?;
            return Some(Frame4 {
                left,
                right,
                top,
                bottom,
            });
        }
    }
    None
}

fn is_frame_glyph(s: &str) -> bool {
    matches!(
        s,
        "─" | "│"
            | "╭"
            | "╮"
            | "╰"
            | "╯"
            | "┌"
            | "┐"
            | "└"
            | "┘"
            | "├"
            | "┤"
            | "┬"
            | "┴"
    )
}

/// Paint `spans` from `(x, y)` on, one cell a char, as far as `limit`.
fn put(buf: &mut Buffer, x: u16, y: u16, limit: u16, spans: &[(String, Style)]) -> u16 {
    let mut at = x;
    for (text, style) in spans {
        for c in text.chars() {
            if at >= limit {
                return at;
            }
            if let Some(cell) = buf.cell_mut(Position::new(at, y)) {
                cell.set_char(c);
                cell.set_style(*style);
            }
            at += 1;
        }
    }
    at
}

fn width(spans: &[(String, Style)]) -> u16 {
    spans
        .iter()
        .map(|(t, _)| t.chars().count())
        .sum::<usize>()
        .try_into()
        .unwrap_or(u16::MAX)
}

/// The MIC CHIP's words for where things stand.
fn chip(app: &App, now: Instant) -> Vec<(String, Style)> {
    let th = app.theme;
    let muted = Style::default().fg(th.muted);
    let red = Style::default().fg(th.err);
    let key =
        crate::hints::key(&app.keymap, crate::keymap::Action::Dictate).filter(|_| key_free(app));
    match &app.dictation {
        None if app.dictation_blocked => vec![(format!(" mic blocked {MIC} "), red)],
        None => {
            let mut spans = Vec::new();
            if let Some(key) = key {
                spans.push((format!(" {key}"), muted));
            }
            spans.push((format!(" {MIC} "), muted));
            if app.dictation_engine == Some(Engine::Basic) {
                spans.push(("basic ".into(), Style::default().fg(th.warn)));
            }
            spans
        }
        Some(d) => match d.phase {
            Phase::Downloading(p) if p > 0.0 => vec![(
                format!(" ↓ speech model {:.0}% ", p * 100.0),
                Style::default().fg(th.accent),
            )],
            Phase::Settling => vec![(" settling… ".into(), muted)],
            _ => vec![
                (" ● ".into(), red.add_modifier(Modifier::BOLD)),
                (
                    format!(
                        "{} {} {} ",
                        if d.held { "HOLD" } else { "REC" },
                        d.clock(now),
                        d.meter()
                    ),
                    red,
                ),
            ],
        },
    }
}

/// The keys a frame's bottom edge names while it listens.
fn listening_hints(app: &App, d: &Dictation) -> Vec<(String, Style)> {
    let th = app.theme;
    let key = Style::default().fg(th.text);
    let word = Style::default().fg(th.dim);
    if d.phase == Phase::Settling {
        return vec![(" putting the last words in… ".into(), word)];
    }
    let dictate = crate::hints::key_or(&app.keymap, crate::keymap::Action::Dictate, "⌘D");
    if d.held {
        return vec![
            (" let go of ".into(), word),
            (dictate, key),
            (" to finish · ".into(), word),
            ("Esc".into(), key),
            (" throw away ".into(), word),
        ];
    }
    vec![
        (" ".into(), word),
        (dictate, key),
        (" or ".into(), word),
        ("Enter".into(), key),
        (" done · ".into(), word),
        ("Esc".into(), key),
        (" throw away ".into(), word),
    ]
}

/// Draw dictation over the frame: the MIC CHIP on the box with the
/// caret, and while it listens the box's crimson frame, its keys and the
/// GHOST of what is heard — or, for a pane, the strip at its foot. Run
/// after everything else is drawn, since it reads the frames it sits on.
pub fn draw(f: &mut Frame, app: &mut App) {
    app.dictation_chip = None;
    if !available(app) {
        return;
    }
    let now = Instant::now();
    let field = crate::text_input::caret_field().filter(|_| app.editor.is_none());
    if let Some(d) = &app.dictation {
        if let Target::Pane(_) = d.target {
            draw_strip(f, app, now);
            return;
        }
    }
    let Some(field) = field else {
        return;
    };
    let mine = match &app.dictation {
        Some(d) => d.target == Target::Field(field.id),
        None => true,
    };
    if !mine {
        return;
    }
    let spans = chip(app, now);
    let w = width(&spans);
    let buf = f.buffer_mut();
    let frame = find_frame(buf, field.area);
    let mut chip_at = None;
    if let Some(fr) = frame {
        // On the top edge, a dash short of the corner, over dashes only:
        // never over the box's title.
        let x = fr.right.saturating_sub(1 + w);
        let clear = x > fr.left + 1 && (x..fr.right - 1).all(|x| symbol(buf, x, fr.top) == "─");
        if clear {
            chip_at = Some(Rect::new(x, fr.top, w, 1));
        }
    } else if !field.multiline && field.area.width > w + 4 {
        // A one-line field in no box: the end of its own row, if bare.
        let x = field.area.right() - w;
        let y = field.area.y;
        if (x..field.area.right()).all(|x| symbol(buf, x, y) == " ") && field.cell.x < x {
            chip_at = Some(Rect::new(x, y, w, 1));
        }
    }
    if let Some(d) = &app.dictation {
        let th = app.theme;
        if let Some(fr) = frame {
            recolor_frame(buf, fr, th);
            let hints = listening_hints(app, d);
            if fr.right > fr.left + 4 {
                for x in fr.left + 1..fr.right {
                    if let Some(cell) = buf.cell_mut(Position::new(x, fr.bottom)) {
                        cell.set_symbol("─");
                        cell.set_style(Style::default().fg(th.err));
                    }
                }
                let limit = fr.right.saturating_sub(1);
                if width(&hints) + 2 < fr.right - fr.left {
                    put(buf, fr.left + 2, fr.bottom, limit, &hints);
                }
            }
        }
        draw_ghost(buf, &field, d, th);
    }
    if let Some(at) = chip_at {
        put(buf, at.x, at.y, at.right(), &spans);
        app.dictation_chip = Some(at);
    }
}

/// The frame's lines in crimson: it is listening.
fn recolor_frame(buf: &mut Buffer, fr: Frame4, th: Theme) {
    let mut paint = |x: u16, y: u16| {
        if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
            if is_frame_glyph(cell.symbol()) {
                cell.set_fg(th.err);
            }
        }
    };
    for x in fr.left..=fr.right {
        paint(x, fr.top);
        paint(x, fr.bottom);
    }
    for y in fr.top..=fr.bottom {
        paint(fr.left, y);
        paint(fr.right, y);
    }
}

/// What is heard, at the caret: settled words in the text colour, the
/// live guess muted, a crimson caret after — flowing on through the
/// field's rows, its tail shown when it runs past them.
fn draw_ghost(buf: &mut Buffer, field: &crate::text_input::CaretField, d: &Dictation, th: Theme) {
    let area = field.area;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut cells: Vec<(char, Style)> = Vec::new();
    let settled = d.settled.trim_start();
    let gap = !settled.is_empty() && field.char_before.is_some_and(|c| !c.is_whitespace());
    if gap {
        cells.push((' ', Style::default().fg(th.text)));
    }
    cells.extend(settled.chars().map(|c| (c, Style::default().fg(th.text))));
    let live = if settled.is_empty() {
        d.live.trim_start()
    } else {
        d.live.as_str()
    };
    cells.extend(live.chars().map(|c| (c, Style::default().fg(th.muted))));
    let empty = cells.is_empty();
    if empty && d.phase == Phase::Listening {
        cells.push((' ', Style::default()));
    }
    // The room from the caret to the end of the field.
    let rows = if field.multiline {
        area.bottom().saturating_sub(field.cell.y)
    } else {
        1
    };
    let first = area.right().saturating_sub(field.cell.x) as usize;
    let room = first + (rows.saturating_sub(1) as usize) * area.width as usize;
    if room < 2 {
        return;
    }
    // One cell kept for the caret; a tail that runs past starts with `…`.
    if cells.len() + 1 > room {
        let keep = room - 2;
        let tail = cells.split_off(cells.len() - keep);
        cells = vec![('…', Style::default().fg(th.muted))];
        cells.extend(tail);
    }
    cells.push((' ', Style::default().bg(th.err)));
    let (mut x, mut y) = (field.cell.x, field.cell.y);
    for (c, style) in cells {
        if x >= area.right() {
            x = area.x;
            y += 1;
        }
        if y >= area.bottom() {
            break;
        }
        if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
            cell.set_char(c);
            cell.set_style(style);
        }
        x += 1;
    }
    if empty && d.phase == Phase::Listening {
        let hint = [(" listening…".to_string(), Style::default().fg(th.dim))];
        put(buf, x, y, area.right(), &hint);
    }
}

/// A pane's dictation: a framed strip across the pane's foot, its words
/// wrapped in it — the agent's own prompt is the agent's to draw.
fn draw_strip(f: &mut Frame, app: &mut App, now: Instant) {
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
    let Some(d) = &app.dictation else { return };
    let pane = app.term_area;
    if pane.width < 20 || pane.height < 6 {
        return;
    }
    let th = app.theme;
    let inner_w = pane.width.saturating_sub(4).max(1) as usize;
    let text_len = d.settled.chars().count() + d.live.chars().count() + 1;
    let lines = text_len.div_ceil(inner_w).clamp(1, 4) as u16;
    let height = lines + 2;
    let area = Rect::new(
        pane.x + 1,
        pane.bottom().saturating_sub(height + 1),
        pane.width - 2,
        height,
    );
    let title: Vec<Span> = chip(app, now)
        .into_iter()
        .map(|(t, s)| Span::styled(t, s))
        .collect();
    let hints: Vec<Span> = listening_hints(app, d)
        .into_iter()
        .map(|(t, s)| Span::styled(t, s))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.err))
        .title(Line::from(title))
        .title_bottom(Line::from(hints));
    // The tail, when it outgrows the strip.
    let room = inner_w * lines as usize;
    let mut settled: Vec<char> = d.settled.trim_start().chars().collect();
    let mut live: Vec<char> = d.live.chars().collect();
    let total = settled.len() + live.len() + 1;
    if total > room {
        let mut cut = total - room + 1;
        let from_settled = cut.min(settled.len());
        settled.drain(..from_settled);
        cut -= from_settled;
        live.drain(..cut.min(live.len()));
        settled.insert(0, '…');
    }
    let body = Line::from(vec![
        Span::styled(
            settled.into_iter().collect::<String>(),
            Style::default().fg(th.text),
        ),
        Span::styled(
            live.into_iter().collect::<String>(),
            Style::default().fg(th.muted),
        ),
        Span::styled(" ", Style::default().bg(th.err)),
    ]);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(body).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_picks_the_engine() {
        assert_eq!(engine_for("26.6.2"), Some(Engine::Analyzer));
        assert_eq!(engine_for("27.0"), Some(Engine::Analyzer));
        assert_eq!(engine_for("15.4.1"), Some(Engine::Basic));
        assert_eq!(engine_for("13.0"), Some(Engine::Basic));
        assert_eq!(engine_for("12.7"), None);
        assert_eq!(engine_for("nonsense"), None);
    }

    #[test]
    fn the_helpers_lines_parse() {
        let parse = |s: &str| serde_json::from_str::<Heard>(s).unwrap();
        assert_eq!(parse(r#"{"ev":"listening"}"#), Heard::Listening);
        assert_eq!(
            parse(r#"{"live":"b","ev":"text","settled":"a "}"#),
            Heard::Text {
                settled: "a ".into(),
                live: "b".into()
            }
        );
        assert_eq!(parse(r#"{"ev":"level","v":0.5}"#), Heard::Level { v: 0.5 });
        assert_eq!(parse(r#"{"ev":"probe","analyzer":"en_GB"}"#), Heard::Other);
    }

    #[test]
    fn a_dictation_follows_what_it_hears() {
        let now = Instant::now();
        let mut d = Dictation::new(1, Target::Field(7), now);
        assert!(!d.hear(&Heard::Download { progress: 0.5 }));
        assert_eq!(d.phase, Phase::Downloading(0.5));
        assert!(!d.hear(&Heard::Listening));
        assert_eq!(d.phase, Phase::Listening);
        d.hear(&Heard::Text {
            settled: "Open a new worktree, ".into(),
            live: "then run".into(),
        });
        assert_eq!(d.text(), "Open a new worktree, then run");
        for v in [0.0, 1.0, 0.5] {
            d.hear(&Heard::Level { v });
        }
        assert_eq!(d.meter(), "▁▁▁▁▇▄");
        d.stop(now);
        assert_eq!(d.phase, Phase::Settling);
        assert!(
            !d.hear(&Heard::Listening),
            "a late `listening` doesn't undo the stop"
        );
        assert_eq!(d.phase, Phase::Settling);
        assert!(d.hear(&Heard::Done {
            text: "Open a new worktree, then run the tests.".into()
        }));
        assert_eq!(d.text(), "Open a new worktree, then run the tests.");
        assert_eq!(d.clock(now + Duration::from_secs(67)), "1:07");
    }
}
