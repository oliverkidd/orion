//! WHAT'S NEW: the releases since the one this machine last ran, shown as
//! the first page of setup on the launch after an upgrade (`onboard`),
//! however many releases the upgrade jumped — stacked newest first, and
//! followed by any setup steps added since.
//!
//! The notes are baked in at build time from git (`build.rs`): each
//! release's commits between its `v*` tag and the one before, so every
//! release ever cut has notes without anyone writing them twice. Which
//! releases are new is `seen_version` (config.local.json), stamped to this
//! build's version whenever setup closes — never lowered, so running an
//! older or a dev build in between does not show the same notes again.

use crate::config::Config;
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

/// The SETUP VERSION each release first shipped, for a config written
/// before `seen_version` existed: having seen a setup version means having
/// run at least the release that brought it. Only ever needed for those
/// configs — every launch from here on stamps `seen_version` — so it
/// never grows.
const SETUP_SHIPPED_IN: &[(u32, &str)] = &[(2, "1.0.17"), (3, "1.0.21")];

/// The last version this machine is known to have run: `seen_version`, or
/// for a config from before it, the release its setup version came with.
/// None when nothing says (a setup from before setup versions).
pub fn seen(cfg: &Config) -> Option<(u64, u64, u64)> {
    parse_version(&cfg.seen_version).or_else(|| {
        SETUP_SHIPPED_IN
            .iter()
            .rev()
            .find(|(setup, _)| cfg.setup_version >= *setup)
            .and_then(|(_, version)| parse_version(version))
    })
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
/// over a setup version's better word: what closing setup stamps. Whether
/// it moved.
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
    fn an_old_config_is_placed_by_its_setup_version() {
        let mut cfg = Config {
            onboarded: true,
            setup_version: 1,
            ..Config::default()
        };
        assert_eq!(seen(&cfg), None);
        cfg.setup_version = 2;
        assert_eq!(seen(&cfg), Some((1, 0, 17)));
        cfg.seen_version = "1.0.18".into();
        assert_eq!(seen(&cfg), Some((1, 0, 18)), "the stamp wins");
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
    }
}
