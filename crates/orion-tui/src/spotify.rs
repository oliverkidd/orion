//! The FOOTER's SPOTIFY READOUT: what the Spotify desktop app is playing,
//! `♪ Midnight City · M83  ⏮  ⏸ ⏭`, just left of the memory readout, with
//! its three glyphs for buttons. While a track plays its title and the
//! pause button shimmer green; paused, they sit grey.
//!
//! It talks to the app on this Mac over AppleScript (`osascript`), the way
//! the media keys reach it — no Spotify login, no Web API, no developer
//! app. A poll runs off the loop once a second while Spotify was last seen
//! playing or paused, and every five while it wasn't, so a closed Spotify
//! costs one cheap `is running` a beat; never two at once. Every script
//! asks `application "Spotify" is running` before it says a word to the
//! app, because a bare `tell` would launch it.
//!
//! The first ask raises macOS's "<terminal> wants to control Spotify"
//! AUTOMATION prompt. Denied, every later ask fails with AppleScript's
//! `-1743`: the poll stops for the rest of the run and the footer says once
//! where to allow it. Every other failure — a timeout, no `osascript`, a
//! line that doesn't parse — is "nothing playing", never a flash.
//!
//! There are no keys: the Mac's own media keys already reach Spotify from
//! anywhere. orion only shows what is playing, and offers the mouse.

use std::ops::Range;
use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::Span;

/// How often the poll re-runs while Spotify has a track up.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

/// How often it re-runs while Spotify is closed, stopped or empty.
pub const IDLE_INTERVAL: Duration = Duration::from_secs(5);

/// How long one `osascript` may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The poll: `<state>\t<title>\t<artist>`, or nothing at all when Spotify
/// isn't running or has nothing loaded.
const POLL: &str = r#"if application "Spotify" is running then
  tell application "Spotify"
    if player state is stopped then return ""
    set t to current track
    return (player state as text) & tab & (name of t) & tab & (artist of t)
  end tell
end if
return """#;

/// The flash a denied AUTOMATION prompt leaves, once.
pub const DENIED: &str =
    "Spotify: allow orion's terminal under System Settings → Privacy & Security → Automation";

/// What Spotify is playing — or has paused on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowPlaying {
    pub playing: bool,
    pub title: String,
    pub artist: String,
}

/// One of the readout's three buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// `⏮`: back a track — or, more than a few seconds in, back to the
    /// start of this one, as the media key does.
    Previous,
    /// `⏸` while playing, `⏵` while paused.
    PlayPause,
    /// `⏭`.
    Next,
}

impl Button {
    /// The command it sends, guarded so it can never launch Spotify.
    fn script(self) -> String {
        let command = match self {
            Button::Previous => "previous track",
            Button::PlayPause => "playpause",
            Button::Next => "next track",
        };
        format!(
            r#"if application "Spotify" is running then tell application "Spotify" to {command}"#
        )
    }
}

/// What a poll heard.
#[derive(Debug)]
pub enum Heard {
    /// What is up now; `None` for closed, stopped, empty or couldn't ask.
    Track(Option<NowPlaying>),
    /// The AUTOMATION prompt was denied: stop asking.
    Denied,
}

/// What a poll lands on the loop: what it heard, and the press count
/// (`App::spotify_seq`) it was sent under — so a poll that set out
/// before a click can't land after it and undo what the click showed.
#[derive(Debug)]
pub struct Answer {
    pub seq: u64,
    pub heard: Heard,
}

/// The poll's cadence while a track is up: `ORION_SPOTIFY_POLL_SECS` when
/// set, `None` when it is `0` (off — the e2e tests, whose footers must not
/// depend on what is playing) and anywhere but macOS, where there is no
/// AppleScript to ask; else [`DEFAULT_INTERVAL`].
pub fn interval() -> Option<Duration> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    orion_core::env::secs_override(orion_core::env::SPOTIFY_POLL_SECS, DEFAULT_INTERVAL)
}

/// Run one poll off the loop under press count `seq`; `tx` hears its
/// answer either way, which is what lets the loop count it back in.
pub fn spawn_poll(seq: u64, tx: tokio::sync::mpsc::UnboundedSender<Answer>) {
    tokio::spawn(async move {
        let heard = poll().await;
        let _ = tx.send(Answer { seq, heard });
    });
}

/// Send `button`'s command off the loop, then poll straight after it, so
/// the readout shows the new track or state without waiting out a beat.
pub fn spawn_command(button: Button, seq: u64, tx: tokio::sync::mpsc::UnboundedSender<Answer>) {
    tokio::spawn(async move {
        let heard = match run(&button.script()).await {
            Ran::Denied => Heard::Denied,
            Ran::Out(_) | Ran::Failed => poll().await,
        };
        let _ = tx.send(Answer { seq, heard });
    });
}

/// How one `osascript` went.
enum Ran {
    Out(String),
    Denied,
    Failed,
}

async fn poll() -> Heard {
    match run(POLL).await {
        Ran::Out(out) => Heard::Track(parse(&out)),
        Ran::Denied => Heard::Denied,
        Ran::Failed => Heard::Track(None),
    }
}

/// `script` through `osascript`, stdin closed and stderr kept for the
/// `-1743` that says the AUTOMATION prompt was denied.
async fn run(script: &str) -> Ran {
    let mut cmd = tokio::process::Command::new("osascript");
    cmd.args(["-e", script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let Ok(Ok(out)) = tokio::time::timeout(TIMEOUT, cmd.output()).await else {
        return Ran::Failed;
    };
    if out.status.success() {
        Ran::Out(String::from_utf8_lossy(&out.stdout).into_owned())
    } else if denied(&String::from_utf8_lossy(&out.stderr)) {
        Ran::Denied
    } else {
        Ran::Failed
    }
}

/// The poll's line as a track: `playing\tTitle\tArtist` (or `paused…`).
/// Empty — nothing to show — and anything without all three fields are
/// `None`. Only the first two tabs split, so a title is taken whole.
fn parse(stdout: &str) -> Option<NowPlaying> {
    let line = stdout.trim_end_matches(['\r', '\n']);
    let mut fields = line.splitn(3, '\t');
    let (state, title, artist) = (fields.next()?, fields.next()?, fields.next()?);
    let title = title.trim();
    if title.is_empty() {
        return None;
    }
    Some(NowPlaying {
        playing: state.trim() == "playing",
        title: title.to_string(),
        artist: artist.trim().to_string(),
    })
}

/// AppleScript's "Not authorised to send Apple events" — the AUTOMATION
/// prompt answered no.
fn denied(stderr: &str) -> bool {
    stderr.contains("-1743")
}

/// The fewest title cells the readout is worth drawing with.
const MIN_TITLE: usize = 8;
/// The title cells the artist steps aside for: below this the artist goes
/// before the title shrinks any further.
const KEEP_TITLE: usize = 12;
/// The fewest artist cells worth a ` · ` (three and the `…`).
const MIN_ARTIST: usize = 4;
/// Before the words.
const LEAD: &str = "♪ ";
/// Between the words and the buttons.
const GAP: &str = "  ";
/// Between title and artist.
const SEP: &str = " · ";

/// The spaces before `button`'s glyph. `⏮` and `⏭` draw two cells wide in
/// Ghostty, spilling into the space after them, where `⏸` and `⏵` keep to
/// one — so `⏮` takes two, and the toggle sits centred between them.
fn before(button: Button) -> &'static str {
    match button {
        Button::Previous => "",
        Button::PlayPause => "  ",
        Button::Next => " ",
    }
}

/// The readout as [`readout`] lays it out.
pub struct Readout {
    pub spans: Vec<Span<'static>>,
    /// The cells it takes, never more than the `max` it was given.
    pub width: u16,
    /// Each button with its columns, counted from the readout's left edge.
    pub buttons: [(Button, Range<u16>); 3],
}

/// The readout's spans, laid out in at most `max` cells, with the width
/// they take and each button's columns counted from their left edge. Every
/// budget is in screen cells, so a CJK or emoji title can't push the
/// buttons past `max`. Too narrow it shortens the artist first, then the
/// title down to [`KEEP_TITLE`], then drops the artist and shrinks the
/// title alone; under [`MIN_TITLE`] title cells it is `None`, nothing
/// drawn. The button under the pointer (`hovered`) lifts as the footer's
/// other buttons do (`ui::footer::footer_button_style`).
///
/// Playing, the title and the pause button are the upgrade's green
/// (`ui::footer::upgrade_ramp`), one band sweeping the title and then the
/// button at `sweep`'s phase — still when it is `None`, as with
/// animations off. Paused, both are dim.
pub fn readout(
    np: &NowPlaying,
    max: usize,
    th: crate::theme::Theme,
    hovered: Option<Button>,
    sweep: Option<usize>,
) -> Option<Readout> {
    use crate::branch_switch::{cells, fit};
    let toggle = if np.playing { "⏸" } else { "⏵" };
    let glyphs = [
        (Button::Previous, "⏮"),
        (Button::PlayPause, toggle),
        (Button::Next, "⏭"),
    ];
    let buttons_w = glyphs.iter().map(|(_, g)| cells(g)).sum::<usize>()
        + glyphs.iter().map(|(b, _)| before(*b).len()).sum::<usize>();
    let room = max.checked_sub(cells(LEAD) + cells(GAP) + buttons_w)?;
    if room < MIN_TITLE {
        return None;
    }
    let title_w = cells(&np.title);
    let artist_w = cells(&np.artist);
    let sep_w = cells(SEP);
    let (title, artist) = if np.artist.is_empty() {
        (fit(&np.title, room), None)
    } else if title_w + sep_w + artist_w <= room {
        (np.title.clone(), Some(np.artist.clone()))
    } else {
        // The title whole if it fits beside a shortened artist, else cut
        // to what does — so long as that keeps KEEP_TITLE of it (or all
        // of a shorter one).
        let beside = room
            .checked_sub(sep_w + MIN_ARTIST)
            .map(|r| title_w.min(r))
            .filter(|&t| t >= title_w.min(KEEP_TITLE));
        match beside {
            Some(t) => {
                let title = fit(&np.title, t);
                // A wide character the cut couldn't split leaves the title
                // a cell short: the artist takes it.
                let artist = fit(&np.artist, room - sep_w - cells(&title));
                (title, Some(artist))
            }
            None => (fit(&np.title, room), None),
        }
    };

    let dim = Style::default().fg(th.dim);
    let ramp = crate::ui::footer::upgrade_ramp(th);
    // The band's cells: the title's, then the pause button's just after.
    let title_len = title.chars().count();
    let mut spans = vec![Span::styled(LEAD, dim)];
    match (np.playing, sweep) {
        (true, Some(phase)) => spans.extend(crate::ui::sweep_spans(&title, dim, ramp, phase)),
        (true, None) => spans.push(Span::styled(title, dim.fg(ramp[0]))),
        (false, _) => spans.push(Span::styled(title, dim)),
    }
    if let Some(artist) = artist {
        spans.push(Span::styled(SEP, dim));
        spans.push(Span::styled(artist, dim));
    }
    spans.push(Span::raw(GAP));
    let mut x: u16 = spans.iter().map(|s| s.width() as u16).sum();
    let buttons = glyphs.map(|(button, glyph)| {
        let gap = before(button);
        spans.push(Span::raw(gap));
        x += gap.len() as u16;
        let lit = hovered == Some(button);
        let style = match (button, np.playing) {
            (Button::PlayPause, true) => {
                let green = match sweep {
                    Some(phase) => crate::ui::sweep_style(dim, ramp, phase, title_len, title_len),
                    None => dim.fg(ramp[0]),
                };
                if lit {
                    green.add_modifier(ratatui::style::Modifier::UNDERLINED)
                } else {
                    green
                }
            }
            _ => crate::ui::footer::footer_button_style(th, lit),
        };
        let w = cells(glyph) as u16;
        spans.push(Span::styled(glyph, style));
        x += w;
        (button, x - w..x)
    });
    Some(Readout {
        spans,
        width: x,
        buttons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn track(playing: bool, title: &str, artist: &str) -> NowPlaying {
        NowPlaying {
            playing,
            title: title.into(),
            artist: artist.into(),
        }
    }

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_poll_line_parses() {
        assert_eq!(
            parse("playing\tMidnight City\tM83\n"),
            Some(track(true, "Midnight City", "M83"))
        );
        assert_eq!(
            parse("paused\tMidnight City\tM83\n"),
            Some(track(false, "Midnight City", "M83"))
        );
        assert_eq!(parse(""), None, "closed, stopped or empty");
        assert_eq!(parse("\n"), None);
        assert_eq!(
            parse("playing\tA · B\tC\n"),
            Some(track(true, "A · B", "C")),
            "a title's own middle dot is the title's"
        );
        assert_eq!(
            parse("playing\tEpisode 4\t\n"),
            Some(track(true, "Episode 4", "")),
            "a podcast has no artist"
        );
        assert_eq!(parse("playing\tMidnight City"), None, "two fields");
        assert_eq!(parse("playing"), None, "one field");
        assert_eq!(parse("playing\t\tM83"), None, "no title");
    }

    #[test]
    fn a_denied_prompt_is_recognised() {
        assert!(denied(
            "execution error: Not authorised to send Apple events to Spotify. (-1743)"
        ));
        assert!(!denied(
            "execution error: Spotify got an error: Can't get current track. (-1728)"
        ));
        assert!(!denied(""));
    }

    #[test]
    fn the_readout_fits_and_names_its_buttons() {
        let th = crate::theme::Theme::default();
        let np = track(true, "Midnight City", "M83");
        let Readout {
            spans,
            width,
            buttons: cols,
        } = readout(&np, 60, th, None, None).unwrap();
        let line = text(&spans);
        assert_eq!(line, "♪ Midnight City · M83  ⏮  ⏸ ⏭");
        assert_eq!(usize::from(width), line.chars().count());
        let order: Vec<Button> = cols.iter().map(|(b, _)| *b).collect();
        assert_eq!(order, [Button::Previous, Button::PlayPause, Button::Next]);
        let at = |r: &Range<u16>| -> String {
            line.chars()
                .skip(r.start as usize)
                .take((r.end - r.start) as usize)
                .collect()
        };
        assert_eq!(at(&cols[0].1), "⏮");
        assert_eq!(at(&cols[1].1), "⏸");
        assert_eq!(at(&cols[2].1), "⏭");
        assert_eq!(cols[2].1.end, width, "ends on ⏭");

        let paused = track(false, "Midnight City", "M83");
        let Readout {
            spans,
            buttons: cols,
            ..
        } = readout(&paused, 60, th, None, None).unwrap();
        let line = text(&spans);
        assert_eq!(line, "♪ Midnight City · M83  ⏮  ⏵ ⏭");
        assert_eq!(
            line.chars().nth(cols[1].1.start as usize),
            Some('⏵'),
            "paused shows play"
        );
        assert_eq!(spans[1].style.fg, Some(th.dim), "and dims the title");
        let toggle = spans.iter().find(|s| s.content == "⏵").unwrap();
        assert_eq!(toggle.style.fg, Some(th.dim), "and the button");
        let spans = readout(&np, 60, th, None, None).unwrap().spans;
        let green = crate::ui::footer::upgrade_ramp(th)[0];
        assert_eq!(spans[1].style.fg, Some(green), "playing, it is green");
        let toggle = spans.iter().find(|s| s.content == "⏸").unwrap();
        assert_eq!(toggle.style.fg, Some(green), "and so is the button");
    }

    #[test]
    fn the_hovered_button_underlines() {
        let th = crate::theme::Theme::default();
        let np = track(true, "Midnight City", "M83");
        let spans = readout(&np, 60, th, Some(Button::Next), None)
            .unwrap()
            .spans;
        let lit: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(lit, ["⏭"]);
    }

    /// Playing, one green band sweeps the title and then the pause button
    /// — the text never changes under it, and the artist stays dim.
    #[test]
    fn a_playing_track_sweeps_its_title_then_the_button() {
        let th = crate::theme::Theme::default();
        let ramp = crate::ui::footer::upgrade_ramp(th);
        let np = track(true, "Midnight City", "M83");
        let title_len = "Midnight City".chars().count();
        for phase in 0..title_len + 8 {
            let spans = readout(&np, 60, th, None, Some(phase)).unwrap().spans;
            assert_eq!(text(&spans), "♪ Midnight City · M83  ⏮  ⏸ ⏭");
            let fg = |s: &str| spans.iter().find(|sp| sp.content == s).unwrap().style.fg;
            assert_eq!(fg("M83"), Some(th.dim));
            let head = phase % (title_len + 4);
            let toggle = match head.checked_sub(title_len) {
                Some(0) => ramp[2],
                Some(1) => ramp[1],
                _ => ramp[0],
            };
            assert_eq!(fg("⏸"), Some(toggle), "phase {phase}");
        }
        let lit = readout(&np, 60, th, None, Some(0)).unwrap().spans;
        assert_eq!(lit[1].content, "M");
        assert_eq!(
            lit[1].style.fg,
            Some(ramp[2]),
            "the band starts on the title"
        );
        let paused = track(false, "Midnight City", "M83");
        let spans = readout(&paused, 60, th, None, Some(0)).unwrap().spans;
        assert_eq!(spans[1].style.fg, Some(th.dim), "paused, nothing sweeps");
    }

    /// Narrowing: the artist shortens, then the title to twelve, then the
    /// artist goes and the title shrinks alone, and under eight title
    /// characters there is no readout at all.
    #[test]
    fn the_readout_gives_way_artist_first() {
        let th = crate::theme::Theme::default();
        let np = track(true, "Midnight City Remastered", "Anthony Gonzalez");
        let line = |max| readout(&np, max, th, None, None).map(|r| text(&r.spans));
        let full = "♪ Midnight City Remastered · Anthony Gonzalez  ⏮  ⏸ ⏭";
        let full_w = full.chars().count();
        assert_eq!(line(full_w).as_deref(), Some(full));
        assert_eq!(
            line(full_w - 6).as_deref(),
            Some("♪ Midnight City Remastered · Anthony G…  ⏮  ⏸ ⏭"),
            "the artist shortens first"
        );
        // 10 fixed + 12 title + 3 + 4 artist.
        assert_eq!(
            line(29).as_deref(),
            Some("♪ Midnight Ci… · Ant…  ⏮  ⏸ ⏭"),
            "then the title, down to twelve"
        );
        assert_eq!(
            line(28).as_deref(),
            Some("♪ Midnight City Rem…  ⏮  ⏸ ⏭"),
            "then the artist goes"
        );
        assert_eq!(line(18).as_deref(), Some("♪ Midnigh…  ⏮  ⏸ ⏭"));
        assert_eq!(line(17), None, "under eight title characters, nothing");
        for max in 18..=full_w {
            let w = line(max).unwrap().chars().count();
            assert!(w <= max, "{w} cells in a budget of {max}");
        }
    }

    /// A title of wide characters is budgeted by the cells it takes, not
    /// the characters it has: the buttons stay inside `max`, and their
    /// columns are where they are drawn.
    #[test]
    fn a_wide_title_cannot_push_the_buttons_out() {
        use crate::branch_switch::cells;
        let th = crate::theme::Theme::default();
        let np = track(true, "夜に駆ける 🎵 YOASOBI 夜に駆ける", "YOASOBI");
        for max in 18..=60 {
            let Some(Readout {
                spans,
                width,
                buttons: cols,
            }) = readout(&np, max, th, None, None)
            else {
                panic!("{max} cells fit a readout");
            };
            let line = text(&spans);
            assert_eq!(usize::from(width), cells(&line), "{line}");
            assert!(usize::from(width) <= max, "{width} > {max}: {line}");
            assert!(line.ends_with("⏮  ⏸ ⏭"), "{line}");
            let (_, next) = &cols[2];
            assert_eq!(next.end, width, "⏭ is the last cell: {line}");
        }
        assert!(readout(&np, 17, th, None, None).is_none());
    }
}
