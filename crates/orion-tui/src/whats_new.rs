//! WHAT'S NEW: the releases since the one this machine last ran, a sheet
//! of its own over the grid on the launch after an upgrade, however many
//! releases the upgrade jumped — stacked newest first. Shown once: closing
//! it stamps them seen.
//!
//! The notes are baked in at build time from git (`build.rs`): each
//! release's commits between its `v*` tag and the one before, so every
//! release ever cut has notes without anyone writing them twice. Which
//! releases are new is `seen_version` (config.local.json), stamped to this
//! build's version whenever the sheet or setup closes — never lowered, so
//! running an older or a dev build in between does not show the same
//! notes again.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::config::Config;
use crate::theme::Theme;
use crate::update_check::parse_version;

/// One published release, as git has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    pub version: &'static str,
    /// `YYYY-MM-DD`, the day its tag was cut.
    pub date: &'static str,
    /// Its commits, oldest first.
    pub changes: &'static [Change],
}

/// One commit of a release: its subject, and its body's paragraphs and
/// bullets, one line each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub title: &'static str,
    pub details: &'static [&'static str],
}

include!(concat!(env!("OUT_DIR"), "/releases.rs"));

#[cfg(test)]
thread_local! {
    static TEST_RELEASES: std::cell::Cell<Option<&'static [Release]>> =
        const { std::cell::Cell::new(None) };
}

/// Every release this build knows, newest first — on a test's thread, the
/// ones [`with_releases`] put there instead of what git had at build time.
fn releases() -> &'static [Release] {
    #[cfg(test)]
    if let Some(list) = TEST_RELEASES.with(|r| r.get()) {
        return list;
    }
    RELEASES
}

/// `f` with `list` as the releases this build knows, on this thread.
#[cfg(test)]
pub(crate) fn with_releases<T>(list: &'static [Release], f: impl FnOnce() -> T) -> T {
    let before = TEST_RELEASES.with(|r| r.replace(Some(list)));
    let out = f();
    TEST_RELEASES.with(|r| r.set(before));
    out
}

/// This build's version.
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The last version this machine is known to have run: `seen_version`.
/// None when nothing says (a config from before it).
pub fn seen(cfg: &Config) -> Option<(u64, u64, u64)> {
    parse_version(&cfg.seen_version)
}

/// How many releases a machine that says nothing of what it ran is shown
/// — the newest — rather than the whole history.
const UNPLACED_RELEASES: usize = 10;

/// The releases after `seen` up to this build, newest first, out of
/// `all` (newest first). The newest [`UNPLACED_RELEASES`] when `seen` is
/// None.
fn unseen_in(
    all: &'static [Release],
    seen: Option<(u64, u64, u64)>,
    current: &str,
) -> Vec<&'static Release> {
    let Some(current) = parse_version(current) else {
        return Vec::new();
    };
    all.iter()
        .filter(|r| {
            parse_version(r.version)
                .is_some_and(|v| v <= current && seen.is_none_or(|seen| v > seen))
        })
        .take(if seen.is_some() {
            usize::MAX
        } else {
            UNPLACED_RELEASES
        })
        .collect()
}

/// The releases this machine has not seen the notes of, newest first —
/// none on a first run, which is the whole of setup instead.
pub fn unseen(cfg: &Config) -> Vec<&'static Release> {
    if !cfg.onboarded {
        return Vec::new();
    }
    unseen_in(releases(), seen(cfg), current())
}

/// `seen_version` moved up to this build, never down — measured against
/// what [`seen`] knows, so a dev build behind the releases does not stamp
/// it down: what closing the sheet or setup stamps. Whether it moved.
pub fn stamp_seen(cfg: &mut Config) -> bool {
    let newer = match (parse_version(current()), seen(cfg)) {
        (Some(this), Some(seen)) => this > seen,
        (Some(_), None) => true,
        (None, _) => false,
    };
    if newer {
        cfg.seen_version = current().to_string();
    }
    newer
}

// ---- the sheet ----

/// The WHAT'S NEW sheet's live state: the releases it shows and the last
/// one seen before them, read as it opened — another orion stamping them
/// meanwhile doesn't empty it — the release under the cursor, and where it
/// was drawn, for a click outside.
#[derive(Debug, Clone, Default)]
pub struct WhatsNewView {
    pub releases: Vec<&'static Release>,
    pub seen: Option<(u64, u64, u64)>,
    pub row: usize,
    pub area: Rect,
}

/// Whether the sheet has anything to show this machine at startup:
/// releases it has not seen the notes of.
pub fn pending(cfg: &Config) -> bool {
    !unseen(cfg).is_empty()
}

/// Put the sheet up over the grid, on the newest release.
pub fn open(app: &mut App) {
    let cfg = Config::load();
    app.overlay = Some(Overlay::WhatsNew(WhatsNewView {
        releases: unseen(&cfg),
        seen: seen(&cfg),
        ..WhatsNewView::default()
    }));
    app.dirty = true;
}

/// Close the sheet and stamp this build's notes seen, so it does not come
/// back. A test without a config path does not persist.
pub fn dismiss(app: &mut App) {
    app.overlay = None;
    app.dirty = true;
    crate::config::persist(|cfg| {
        stamp_seen(cfg);
    });
}

/// The sheet's keys: one table [`handle_key`] matches and the border
/// spells.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const WALK: Key = Key::new(&["up", "down"], "release").show(2);
    pub const DONE: Key = Key::new(&["enter", "esc"], "done").show(2);
    #[cfg(test)]
    pub const ALL: &[Key] = &[WALK, DONE];
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::WhatsNew(view)) = &mut app.overlay else {
        return;
    };
    match key.code {
        _ if keys::DONE.matches(&key) => return dismiss(app),
        KeyCode::Down | KeyCode::Char('j') => {
            view.row = (view.row + 1).min(view.releases.len().saturating_sub(1));
        }
        KeyCode::Up | KeyCode::Char('k') => view.row = view.row.saturating_sub(1),
        _ => return,
    }
    app.dirty = true;
}

pub fn draw(f: &mut Frame, app: &mut App, view: &WhatsNewView, th: Theme) {
    let width = crate::onboard::sheet_inner_width(f.area());
    let body = notes(view, th, width);
    let hints = [keys::DONE.hint().kept(), keys::WALK.hint()];
    let area = crate::onboard::draw_sheet(
        f,
        " What's new in Orion ",
        None,
        body,
        "",
        &hints,
        false,
        th,
    );
    if let Some(Overlay::WhatsNew(live)) = &mut app.overlay {
        live.area = area;
    }
}

/// Each release since the one this machine last ran, newest first — its
/// version and day, then every change in it with what its commit said.
/// The cursor walks the releases; the one under it stays in sight, its
/// last line kept on screen.
fn notes(view: &WhatsNewView, th: Theme, width: u16) -> crate::onboard::Body {
    let mut body = crate::onboard::Body::new(width);
    let text = Style::default().fg(th.text);
    let dim = Style::default().fg(th.dim);
    let (releases, row) = (&view.releases, view.row);
    let since = view
        .seen
        .map(|(a, b, c)| format!(" since v{a}.{b}.{c}"))
        .unwrap_or_default();
    let n = releases.len();
    body.prose(
        &format!(
            "You're on v{} — {n} release{} {}{since}, newest first.",
            current(),
            if n == 1 { "" } else { "s" },
            if n == 1 { "is new" } else { "are new" },
        ),
        width,
        dim,
    );
    for (i, release) in releases.iter().enumerate() {
        body.blank();
        let on = i == row;
        body.lines.push(Line::from(vec![
            Span::styled(
                if on { " › " } else { "   " },
                Style::default().fg(th.accent),
            ),
            Span::styled(
                format!("v{}", release.version),
                Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}", release.date), dim),
        ]));
        for change in release.changes {
            body.item(
                "• ",
                change.title,
                3,
                width,
                text.add_modifier(Modifier::BOLD),
            );
            for detail in change.details {
                body.item("– ", detail, 5, width, dim);
            }
        }
        if on {
            body.selected = Some(body.lines.len() - 1);
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    static ALL: &[Release] = &[
        Release {
            version: "1.0.3",
            date: "2026-10-03",
            changes: &[Change {
                title: "Three",
                details: &[],
            }],
        },
        Release {
            version: "1.0.2",
            date: "2026-10-02",
            changes: &[],
        },
        Release {
            version: "1.0.1",
            date: "2026-10-01",
            changes: &[],
        },
    ];

    fn versions(list: Vec<&'static Release>) -> Vec<&'static str> {
        list.into_iter().map(|r| r.version).collect()
    }

    #[test]
    fn a_jump_stacks_every_release_since_the_one_seen() {
        assert_eq!(
            versions(unseen_in(ALL, Some((1, 0, 1)), "1.0.3")),
            ["1.0.3", "1.0.2"]
        );
        assert_eq!(
            versions(unseen_in(ALL, Some((1, 0, 2)), "1.0.3")),
            ["1.0.3"]
        );
        assert!(unseen_in(ALL, Some((1, 0, 3)), "1.0.3").is_empty());
    }

    #[test]
    fn nothing_past_this_build_and_everything_when_nothing_was_seen() {
        assert_eq!(
            versions(unseen_in(ALL, Some((1, 0, 1)), "1.0.2")),
            ["1.0.2"],
            "a newer release this build doesn't ship is not its news"
        );
        assert_eq!(
            versions(unseen_in(ALL, None, "1.0.3")),
            ["1.0.3", "1.0.2", "1.0.1"]
        );
        assert!(unseen_in(ALL, None, "dev").is_empty());
    }

    #[test]
    fn a_machine_that_says_nothing_sees_only_the_newest() {
        let many: &'static [Release] = Box::leak(
            (0..20)
                .rev()
                .map(|patch| Release {
                    version: Box::leak(format!("0.9.{patch}").into_boxed_str()),
                    date: "",
                    changes: &[],
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let shown = versions(unseen_in(many, None, "1.0.0"));
        assert_eq!(shown.len(), UNPLACED_RELEASES);
        assert_eq!(shown[0], "0.9.19", "newest first");
        assert_eq!(unseen_in(many, Some((0, 9, 0)), "1.0.0").len(), 19);
    }

    #[test]
    fn the_stamp_is_what_was_seen() {
        let mut cfg = Config {
            onboarded: true,
            ..Config::default()
        };
        assert_eq!(seen(&cfg), None);
        cfg.seen_version = "1.0.18".into();
        assert_eq!(seen(&cfg), Some((1, 0, 18)));
    }

    #[test]
    fn the_stamp_only_moves_up() {
        let mut cfg = Config::default();
        stamp_seen(&mut cfg);
        assert_eq!(cfg.seen_version, current());
        cfg.seen_version = "999.0.0".into();
        stamp_seen(&mut cfg);
        assert_eq!(cfg.seen_version, "999.0.0");
    }

    #[test]
    fn a_first_run_has_no_news() {
        let cfg = Config::default();
        assert!(unseen(&cfg).is_empty());
        assert!(!pending(&cfg));
    }

    fn sheet_text(app: &mut App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(96, 30)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    /// After an upgrade the sheet is up on its own: every release since
    /// the one this machine last ran, newest first, no setup step with
    /// them; ↑/↓ walk them, and closing it stamps this build seen, so it
    /// is shown once.
    #[test]
    fn the_sheet_shows_the_unseen_releases_once() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            crate::config::persist(|cfg| {
                cfg.onboarded = true;
                cfg.seen_version = "1.0.0".into();
            });
            with_releases(ALL, || {
                let cfg = Config::load();
                assert!(pending(&cfg));
                let mut app = App::new();
                open(&mut app);
                let shot = sheet_text(&mut app);
                assert!(shot.contains("What's new in Orion"), "{shot}");
                assert!(!shot.contains("Welcome"), "no setup page:\n{shot}");
                let (a, b) = (shot.find("v1.0.3").unwrap(), shot.find("v1.0.1").unwrap());
                assert!(a < b, "newest first:\n{shot}");
                assert!(shot.contains("Three"), "{shot}");
                handle_key(&mut app, KeyEvent::from(KeyCode::Down));
                let Some(Overlay::WhatsNew(view)) = &app.overlay else {
                    panic!("the sheet stays up");
                };
                assert_eq!(view.row, 1);
                handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
                assert!(app.overlay.is_none());
                assert_eq!(Config::load().seen_version, current());
            });
        });
    }
}
