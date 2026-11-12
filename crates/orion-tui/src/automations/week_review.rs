//! WEEK IN REVIEW as an automation: on its **When** (Fridays 08:30 out of
//! the box), by `r` on its panel, or by "Run week in review now" in `⌘⇧P`,
//! a run writes up every project that had changes in the week, one review
//! each ([`crate::week_review`] gathers, writes and keeps them). Its panel
//! lists the runs kept as REVIEWS, Enter opening one with a tab a project.
//!
//! * **The schedule** is checked once a minute while orion runs
//!   ([`CHECK`], `ORION_WEEK_REVIEW_CHECK_SECS`). A time missed while the
//!   laptop slept is run on the first check after it, once ([`due`]).
//! * **Whose** is just yours or the whole team's; Enter on it switches.
//!   **When** is written as a line: `Fri 08:30`.
//! * The note older releases kept a project (`reviews/p-<id>/note.txt`)
//!   becomes the extra steer, once ([`migrate_notes`]).

use super::config::{Schedule, Whose};
use super::{Automation, Dot, Panel, PanelList, PanelRow, Runner, SettingRow, Tone};
use crate::app::App;
use crate::hints::Key;
use chrono::{Datelike, Duration as Days, Local, NaiveDateTime, NaiveTime, Weekday};
use orion_core::ClientRequest;
use std::path::Path;
use std::time::{Duration, Instant};

/// How often the schedule is looked at.
pub const CHECK: Duration = Duration::from_secs(60);

/// When it runs while the config names no time.
pub fn default_when() -> Schedule {
    Schedule {
        day: "fri".into(),
        at: "08:30".into(),
    }
}

/// When it runs, as the config says or out of the box.
fn when_of(app: &App) -> Schedule {
    app.automations
        .settings
        .of(Automation::WeekReview)
        .when
        .unwrap_or_else(default_when)
}

/// `r` on its panel.
pub const RUN_NOW: Key = Key::new(&["r"], "run now");

/// What the panel's REVIEWS list says with none kept.
const NO_REVIEWS: &str = "No reviews yet · r writes one now";

/// The marker [`migrate_notes`] leaves once it has run.
const NOTES_MOVED: &str = ".notes-moved-to-extra-steer";

#[derive(Debug, Default)]
pub struct WeekReview {
    /// When the schedule was last looked at.
    checked: Option<Instant>,
    /// Since when it has been watched on: a time before it is never run
    /// late, so turning it on never runs it at once.
    on_since: Option<NaiveDateTime>,
}

/// `fri` `08:30` as a weekday and a time.
fn parts(when: &Schedule) -> Option<(Weekday, NaiveTime)> {
    let day = when.day.trim().get(..3)?.parse::<Weekday>().ok()?;
    let at = NaiveTime::parse_from_str(when.at.trim(), "%H:%M").ok()?;
    Some((day, at))
}

/// `Fri 08:30`, `friday 8:30`, `mon 9` read as a schedule.
pub fn parse_when(text: &str) -> Option<Schedule> {
    let mut words = text.split_whitespace();
    let day = words.next()?.get(..3)?.to_ascii_lowercase();
    let at = words.next()?;
    if words.next().is_some() {
        return None;
    }
    let at = match at.split_once(':') {
        Some(_) => NaiveTime::parse_from_str(at, "%H:%M").ok()?,
        None => NaiveTime::from_hms_opt(at.parse().ok()?, 0, 0)?,
    };
    let when = Schedule {
        day,
        at: at.format("%H:%M").to_string(),
    };
    parts(&when).map(|_| when)
}

/// `Fri 08:30`.
pub fn when_label(when: &Schedule) -> String {
    match parts(when) {
        Some((day, at)) => format!("{day} {}", at.format("%H:%M")),
        None => format!("{} {}", when.day, when.at),
    }
}

/// The last time `when` came round at or before `now`.
pub fn last_slot(when: &Schedule, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let (day, at) = parts(when)?;
    let back = (7 + now.weekday().num_days_from_monday() - day.num_days_from_monday()) % 7;
    let slot = (now.date() - Days::days(i64::from(back))).and_time(at);
    Some(if slot > now {
        slot - Days::days(7)
    } else {
        slot
    })
}

/// Whether a run is due at `now`: the last time `when` came round is after
/// the last run and after it was first watched on — a time slept through
/// runs on the first check after it, and only once.
pub fn due(
    when: &Schedule,
    now: NaiveDateTime,
    last_run: Option<NaiveDateTime>,
    on_since: NaiveDateTime,
) -> bool {
    let Some(slot) = last_slot(when, now) else {
        return false;
    };
    slot > on_since && last_run.is_none_or(|last| slot > last)
}

fn local(secs: i64) -> Option<NaiveDateTime> {
    chrono::DateTime::from_timestamp(secs, 0).map(|at| at.with_timezone(&Local).naive_local())
}

/// "Run week in review now": a run, and the Automations screen on it to
/// watch it go; why not in the footer when it can't start.
pub fn run_now(app: &mut App) {
    if let Err(why) = crate::week_review::run_now(app) {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "Week in review: {why}"
        )));
        return;
    }
    app.automations.selected = Automation::ALL
        .iter()
        .position(|a| *a == Automation::WeekReview)
        .unwrap_or_default();
    super::view::open(app);
}

/// Move what each project's note said into the extra steer, once: notes
/// already in it are not added again. The note files stay where they are.
pub fn migrate_notes(app: &mut App, root: &Path) {
    let marker = root.join(NOTES_MOVED);
    if marker.exists() {
        return;
    }
    let mut notes: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("p-"))
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("note.txt")).ok())
        .map(|note| note.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|note| !note.is_empty())
        .collect();
    notes.sort();
    notes.dedup();
    if !notes.is_empty() {
        let mut cfg = crate::config::Config::load();
        let own = cfg.automations.of_mut(Automation::WeekReview);
        let mut steer = own.extra_steer.trim().to_string();
        for note in notes {
            if !steer.contains(&note) {
                steer = if steer.is_empty() {
                    note
                } else {
                    format!("{steer} {note}")
                };
            }
        }
        own.extra_steer = steer;
        if let Err(err) = cfg.try_save() {
            tracing::warn!("week in review notes not moved: {err}");
            return;
        }
        app.automations.settings = cfg.automations;
    }
    let _ = std::fs::create_dir_all(root);
    let _ = std::fs::write(marker, "");
}

impl Runner for WeekReview {
    fn automation(&self) -> Automation {
        Automation::WeekReview
    }

    /// Once a minute: run it when its time has come round since the last
    /// run.
    fn tick(&mut self, app: &mut App, _out: &mut Vec<ClientRequest>) {
        if !app.automations.is_on(Automation::WeekReview) {
            self.on_since = None;
            return;
        }
        let Some(every) =
            orion_core::env::secs_override(orion_core::env::WEEK_REVIEW_CHECK_SECS, CHECK)
        else {
            return;
        };
        if self.checked.is_some_and(|at| at.elapsed() < every) {
            return;
        }
        self.checked = Some(Instant::now());
        let now = Local::now().naive_local();
        let on_since = *self.on_since.get_or_insert(now);
        if app.week_review.run.is_some() {
            return;
        }
        let when = when_of(app);
        let last = app
            .week_review
            .kept
            .first()
            .and_then(|s| local(s.meta.written_at));
        if due(&when, now, last, on_since) {
            if let Err(why) = crate::week_review::run_now(app) {
                app.flash = Some(crate::flash::Flash::failed(format!(
                    "Week in review: {why}"
                )));
            }
        }
    }

    fn panel(&self, app: &App) -> Panel {
        let when = when_of(app);
        let mut rows = Vec::new();
        let (dot, state) = match &app.week_review.run {
            Some(run) => {
                let (back, all) = run.progress();
                let state = match run.fetching {
                    true => "asking GitHub".to_string(),
                    false => format!("writing {} of {all}", back + 1),
                };
                rows.push(PanelRow {
                    text: run.name(),
                    state: state.clone(),
                    tone: Tone::Working,
                    session: None,
                });
                (Dot::Working, state)
            }
            None => (Dot::Idle, when_label(&when)),
        };
        for saved in &app.week_review.kept {
            let meta = &saved.meta;
            let at = local(meta.written_at)
                .map(|at| at.format("%a %H:%M").to_string())
                .unwrap_or_default();
            let mut state = format!("{at} · {}", crate::week_review::points_label(meta.points()));
            if !meta.read {
                state.push_str(" · new");
            }
            rows.push(PanelRow {
                text: meta.name(),
                state,
                tone: if meta.read { Tone::Quiet } else { Tone::Done },
                session: None,
            });
        }
        Panel {
            dot,
            state,
            lists: vec![PanelList {
                title: "REVIEWS",
                empty: NO_REVIEWS,
                rows,
            }],
        }
    }

    /// Enter on **Whose**: just yours, or the whole team's.
    fn edit(&mut self, app: &mut App, row: SettingRow) -> bool {
        if row != SettingRow::Whose {
            return false;
        }
        super::view::change_settings(app, |s| {
            let own = s.of_mut(Automation::WeekReview);
            own.whose = match own.whose {
                Whose::JustMe => Whose::WholeTeam,
                Whose::WholeTeam => Whose::JustMe,
            };
        });
        true
    }

    fn text_of(&self, app: &App, row: SettingRow) -> Option<String> {
        (row == SettingRow::When).then(|| when_label(&when_of(app)))
    }

    fn set_text(&mut self, app: &mut App, row: SettingRow, text: &str) -> Result<(), String> {
        if row != SettingRow::When {
            return Err(format!("{} can't be written here", row.label()));
        }
        let when = parse_when(text)
            .ok_or_else(|| format!("\"{}\" isn't a day and a time, like Fri 08:30", text.trim()))?;
        super::view::change_settings(app, |s| s.of_mut(Automation::WeekReview).when = Some(when));
        Ok(())
    }

    /// Enter on a REVIEWS row: that run on its page, with a tab a project.
    fn open_row(&mut self, app: &mut App, index: usize) {
        crate::week_review::open(app, index, true);
    }

    fn hotkey(&self) -> Option<Key> {
        Some(RUN_NOW)
    }

    /// `r`: a run, now.
    fn hotkey_pressed(&mut self, app: &mut App) -> Result<(), String> {
        crate::week_review::run_now(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(date: &str, time: &str) -> NaiveDateTime {
        NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .unwrap()
            .and_time(NaiveTime::parse_from_str(time, "%H:%M").unwrap())
    }

    #[test]
    fn when_is_written_as_a_day_and_a_time() {
        let fri = parse_when("Fri 08:30").unwrap();
        assert_eq!(fri, default_when());
        assert_eq!(parse_when("friday 8:30"), Some(default_when()));
        assert_eq!(
            parse_when("mon 9"),
            Some(Schedule {
                day: "mon".into(),
                at: "09:00".into()
            })
        );
        assert_eq!(parse_when("someday 8:30"), None);
        assert_eq!(parse_when("fri 25:00"), None);
        assert_eq!(parse_when("fri"), None);
        assert_eq!(when_label(&fri), "Fri 08:30");
    }

    /// 2026-11-06 is a Friday.
    #[test]
    fn the_last_slot_is_this_weeks_or_last_weeks() {
        let fri = default_when();
        assert_eq!(
            last_slot(&fri, at("2026-11-06", "09:00")),
            Some(at("2026-11-06", "08:30"))
        );
        assert_eq!(
            last_slot(&fri, at("2026-11-06", "08:00")),
            Some(at("2026-10-30", "08:30")),
            "earlier on the day, last week's"
        );
        assert_eq!(
            last_slot(&fri, at("2026-11-09", "12:00")),
            Some(at("2026-11-06", "08:30"))
        );
    }

    #[test]
    fn a_missed_run_runs_once_and_one_already_run_does_not() {
        let fri = default_when();
        let watched = at("2026-11-02", "10:00");
        // Asleep through Friday 08:30, awake at 11:00: it runs.
        let last_week = Some(at("2026-10-30", "08:31"));
        assert!(due(&fri, at("2026-11-06", "11:00"), last_week, watched));
        // …and once: the run it made is after the slot.
        let ran = Some(at("2026-11-06", "11:00"));
        assert!(!due(&fri, at("2026-11-06", "11:01"), ran, watched));
        // Already run today by hand after the time: not again.
        let by_hand = Some(at("2026-11-06", "08:45"));
        assert!(!due(&fri, at("2026-11-06", "09:00"), by_hand, watched));
        // Run by hand before the time: the schedule still runs.
        let early = Some(at("2026-11-06", "07:00"));
        assert!(due(&fri, at("2026-11-06", "08:31"), early, watched));
        // Never run: the first slot after it was watched on.
        assert!(!due(&fri, at("2026-11-04", "09:00"), None, watched));
        assert!(due(&fri, at("2026-11-06", "08:30"), None, watched));
        // Turned on after the slot: not run at once.
        assert!(!due(
            &fri,
            at("2026-11-06", "12:00"),
            last_week,
            at("2026-11-06", "11:59")
        ));
    }

    #[test]
    fn notes_move_into_the_extra_steer_once() {
        let dir = tempfile::tempdir().unwrap();
        crate::config::with_config_path(dir.path().join("config.json"), || {
            let root = dir.path().join("reviews");
            for (project, note) in [
                ("p-a", "Lead with search."),
                ("p-b", "  "),
                ("p-c", "Lead with search.\n"),
            ] {
                std::fs::create_dir_all(root.join(project)).unwrap();
                std::fs::write(root.join(project).join("note.txt"), note).unwrap();
            }
            let mut app = App::new();
            migrate_notes(&mut app, &root);
            let steer = |app: &App| {
                app.automations
                    .settings
                    .of(Automation::WeekReview)
                    .extra_steer
            };
            assert_eq!(steer(&app), "Lead with search.");
            assert_eq!(
                crate::config::Config::load()
                    .automations
                    .of(Automation::WeekReview)
                    .extra_steer,
                "Lead with search."
            );
            assert!(
                root.join("p-a").join("note.txt").is_file(),
                "left where it was"
            );
            // Once: a steer edited since is not touched again.
            crate::automations::view::change_settings(&mut app, |s| {
                s.of_mut(Automation::WeekReview).extra_steer = "Short.".into()
            });
            migrate_notes(&mut app, &root);
            assert_eq!(steer(&app), "Short.");
        });
    }

    /// The panel: the When as its state, REVIEWS newest first, each
    /// `Week 45 · Fri 08:30 · 9 points · new` until read; the run being
    /// written heads it.
    #[test]
    fn the_panel_lists_the_runs_kept() {
        let mut app = App::new();
        let panel = WeekReview::default().panel(&app);
        assert_eq!(panel.state, "Fri 08:30");
        assert_eq!(panel.lists[0].title, "REVIEWS");
        assert!(panel.lists[0].rows.is_empty());
        let written_at = at("2026-11-06", "08:30")
            .and_local_timezone(Local)
            .unwrap()
            .timestamp();
        app.week_review.kept.push(crate::week_review::Saved {
            dir: "/nonexistent".into(),
            meta: crate::week_review::Meta {
                from: "2026-10-31".into(),
                to: "2026-11-06".into(),
                written_at,
                reviews: vec![crate::week_review::Kept {
                    points: 9,
                    ..Default::default()
                }],
                ..Default::default()
            },
        });
        let panel = WeekReview::default().panel(&app);
        let row = &panel.lists[0].rows[0];
        assert_eq!(row.text, "Week 45");
        assert_eq!(row.state, "Fri 08:30 · 9 points · new");
        assert_eq!(row.tone, Tone::Done);
    }
}
