//! THE ROWS of a factory pull request's top, and of its card: each a dot
//! to judge it by at a glance, a label, and at most [`CELL_WORDS`] words.
//! Whatever a row has to say past that goes in the Notes fold, under the
//! row's name ([`Note`]).
//!
//! | Row | 🟢 | 🟡 | 🔴 | ⚪ |
//! |---|---|---|---|---|
//! | Size | under half the contract's limit for the kind | under the limit | over it | |
//! | Plan | on plan | off in places (`followed`, with deviations) | not followed | |
//! | Your calls | none | the count and their labels | | |
//! | Pictures | shown, and they show a change | | missing, or showing no difference | none needed |
//! | Cubic | clean | findings dismissed, unresolved or below P1 | an open P0, or an open P1 not declared | |
//! | Gaps | | the agent's short label | | none |
//! | Rule files | | the ones touched (only then) | | |
//! | Knowledge | | the docs touched (only then) | | |
//!
//! Size — lines added and removed, files, and the build time from the run
//! record (first stage to the pull request opening) — is for the owner to
//! judge only: nothing here goes back to an agent or sets a budget.
//!
//! "What users get" is the one sentence above the table
//! ([`Top::sentence`]); the proof the PR's checks don't already show goes
//! in the notes (`facts::beyond_checks`).

use super::contract::{Contract, Limit};
use super::cubic::Cubic;
use super::facts::{self, words, Facts, CALL_LABEL_WORDS, LABEL_WORDS};
use super::pictures::Pictures;
use super::verdict::{self, Kind, Size};
use crate::automations::linear_builder::Stage;
use crate::factory::runs::RunRecord;

/// The most words a cell holds, its dot left out.
pub const CELL_WORDS: usize = 12;

/// What a row's dot says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    Green,
    Amber,
    Red,
    /// Nothing to judge.
    Clear,
}

impl Dot {
    pub fn mark(self) -> &'static str {
        match self {
            Dot::Green => "🟢",
            Dot::Amber => "🟡",
            Dot::Red => "🔴",
            Dot::Clear => "⚪",
        }
    }
}

/// One row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub dot: Dot,
    pub label: &'static str,
    pub cell: String,
}

/// What goes in the Notes fold under `heading`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub heading: &'static str,
    pub text: String,
}

/// The top's sentence, rows and notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Top {
    /// What users get, in its first sentence.
    pub sentence: String,
    pub rows: Vec<Row>,
    pub notes: Vec<Note>,
}

/// What the rows are worked out from.
pub struct Inputs<'a> {
    pub facts: &'a Facts,
    /// The PR's size after the exclusions, and GitHub's own totals.
    pub size: Size,
    pub total: Size,
    pub kind: Kind,
    pub rules: &'a Contract,
    /// First stage to the pull request opening, from the run record.
    pub build_ms: Option<i64>,
    pub pictures: &'a Pictures,
    pub cubic: &'a Cubic,
    /// The contract's rule files the PR touches.
    pub rule_files: Vec<&'a str>,
    /// The risky paths it touches.
    pub risky: Vec<&'a str>,
    /// Its checks' names: proof that names one is left out.
    pub checks: Vec<&'a str>,
}

/// How long the build took, from `record`: its first stage to the first
/// time its pull request was open. None when it hasn't both.
pub(crate) fn build_ms(record: &RunRecord) -> Option<i64> {
    let start = record.stages.iter().map(|s| s.at_ms).min()?;
    let open = record
        .stages
        .iter()
        .filter(|s| s.stage == Stage::PrOpen.name())
        .map(|s| s.at_ms)
        .min()?;
    (open >= start).then_some(open - start)
}

/// `45m`, `2h 10m`, `3d 4h`.
pub fn duration_words(ms: i64) -> String {
    let minutes = (ms.max(0) + 30_000) / 60_000;
    let (days, hours, mins) = (minutes / (24 * 60), minutes / 60 % 24, minutes % 60);
    match (days, hours) {
        (0, 0) => format!("{}m", mins.max(1)),
        (0, h) if mins == 0 => format!("{h}h"),
        (0, h) => format!("{h}h {mins}m"),
        (d, 0) => format!("{d}d"),
        (d, h) => format!("{d}d {h}h"),
    }
}

fn plural(n: usize, one: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {one}s"),
    }
}

fn bullets<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    items
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("- {s}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` when it fits in `at_most` words, else None.
fn short(text: &str, at_most: usize) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && words(text) <= at_most).then_some(text)
}

/// The first sentence of `text`, and whether there was more.
fn first_sentence(text: &str) -> (String, bool) {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut from = 0;
    while let Some(at) = text[from..].find(['.', '!', '?']).map(|i| from + i) {
        let rest = &text[at + 1..];
        if rest.starts_with(' ') && rest[1..].starts_with(|c: char| c.is_uppercase()) {
            return (text[..=at].to_string(), true);
        }
        from = at + 1;
    }
    (text, false)
}

/// The labels in `labels` that fit `room` words, `and N more` after.
fn fitting(labels: &[String], room: usize) -> String {
    let mut shown: Vec<&str> = Vec::new();
    let mut used = 0;
    for (i, label) in labels.iter().enumerate() {
        let more = labels.len() - i - 1;
        // Room for this one, and for `and N more` if any would follow.
        let tail = if more > 0 { 3 } else { 0 };
        if used + words(label) + tail > room && !shown.is_empty() {
            break;
        }
        shown.push(label);
        used += words(label);
    }
    match labels.len() - shown.len() {
        0 => shown.join(", "),
        more => format!("{} and {more} more", shown.join(", ")),
    }
}

/// The top from `i`.
pub fn top(i: &Inputs) -> Top {
    let mut t = Top::default();
    let (sentence, more) = first_sentence(&i.facts.what_users_get);
    t.sentence = sentence;
    if more {
        t.note("What users get", i.facts.what_users_get.trim().to_string());
    }
    size_row(&mut t, i);
    plan_row(&mut t, i.facts);
    calls_row(&mut t, i.facts);
    pictures_row(&mut t, i.facts, i.pictures);
    cubic_row(&mut t, i.cubic);
    gaps_row(&mut t, i.facts);
    paths_row(&mut t, "Rule files", &i.rule_files);
    let docs: Vec<&str> = i
        .facts
        .docs_touched
        .iter()
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
        .collect();
    paths_row(&mut t, "Knowledge", &docs);
    let proof = facts::beyond_checks(&i.facts.proof, &i.checks);
    if !proof.is_empty() {
        t.note("Proof", bullets(proof));
    }
    let mut risk = Vec::new();
    if !i.risky.is_empty() {
        risk.push(format!("- Risky paths: {}", i.risky.join(", ")));
    }
    if let Some(flag) = i
        .facts
        .flag
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
    {
        risk.push(format!("- Behind flag `{flag}`"));
    }
    if !risk.is_empty() {
        t.note("Risk", risk.join("\n"));
    }
    t
}

impl Top {
    fn row(&mut self, dot: Dot, label: &'static str, cell: impl Into<String>) {
        let mut cell: String = cell.into();
        // A cell never runs past its words: the whole of it goes in the
        // notes instead.
        if words(&cell) > CELL_WORDS {
            self.note(label, cell);
            cell = "see Notes".into();
        }
        self.rows.push(Row { dot, label, cell });
    }

    fn note(&mut self, heading: &'static str, text: String) {
        match self.notes.iter_mut().find(|n| n.heading == heading) {
            Some(n) => {
                n.text.push_str("\n\n");
                n.text.push_str(&text);
            }
            None => self.notes.push(Note { heading, text }),
        }
    }
}

fn size_row(t: &mut Top, i: &Inputs) {
    let s = i.size;
    let mut cell = format!(
        "+{} −{} in {}",
        verdict::thousands(s.added),
        verdict::thousands(s.removed),
        plural(s.files as usize, "file")
    );
    if let Some(ms) = i.build_ms {
        cell.push_str(&format!(", built in {}", duration_words(ms)));
    }
    let limit: Limit = i.kind.limit(i.rules);
    let dot = match (s.over(limit), s.under_half(limit)) {
        (true, _) => Dot::Red,
        (false, true) => Dot::Green,
        (false, false) => Dot::Amber,
    };
    t.row(dot, "Size", cell);
    let mut notes = Vec::new();
    if dot != Dot::Green {
        notes.push(format!(
            "The {} limit is {} lines or {} files.",
            i.kind.word(),
            verdict::thousands(limit.lines),
            limit.files
        ));
    }
    if i.total != s {
        notes.push(format!(
            "GitHub counts +{} −{} in {}; lockfiles, changesets, docs, snapshots and JSON are \
             left out of the size.",
            verdict::thousands(i.total.added),
            verdict::thousands(i.total.removed),
            plural(i.total.files as usize, "file")
        ));
    }
    if !notes.is_empty() {
        t.note("Size", notes.join(" "));
    }
}

fn plan_row(t: &mut Top, facts: &Facts) {
    let plan = &facts.plan_followed;
    if !plan.off() {
        t.row(Dot::Green, "Plan", "on plan");
        return;
    }
    let dot = match plan.followed {
        true => Dot::Amber,
        false => Dot::Red,
    };
    let deviations: Vec<&str> = plan
        .deviations
        .iter()
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
        .collect();
    let why = short(&plan.label, LABEL_WORDS).or(match deviations.as_slice() {
        [one] => short(one, LABEL_WORDS),
        _ => None,
    });
    let cell = match (why, deviations.is_empty()) {
        (Some(why), _) => format!("off plan: {why}"),
        (None, true) => "off plan".to_string(),
        (None, false) => "off plan, see Notes".to_string(),
    };
    t.row(dot, "Plan", cell);
    if !deviations.is_empty() && !(deviations.len() == 1 && why == Some(deviations[0])) {
        t.note("Plan", bullets(deviations));
    }
}

fn calls_row(t: &mut Top, facts: &Facts) {
    let calls = &facts.your_calls;
    if calls.is_empty() {
        t.row(Dot::Green, "Your calls", "none");
        return;
    }
    let labels: Vec<String> = calls
        .iter()
        .map(
            |c| match short(&c.label, CALL_LABEL_WORDS).or(short(&c.call, CALL_LABEL_WORDS)) {
                Some(label) => label.to_string(),
                None => format!("a {} call", c.tag),
            },
        )
        .collect();
    let cell = format!("{}: {}", calls.len(), fitting(&labels, CELL_WORDS - 1));
    t.row(Dot::Amber, "Your calls", cell);
    let each: Vec<String> = calls
        .iter()
        .map(|c| {
            let alt = match c.alternative.trim() {
                "" => String::new(),
                alt => format!(" (not {alt})"),
            };
            format!("[{}] {}: {}{alt}", c.tag, c.call.trim(), c.picked.trim())
        })
        .collect();
    t.note("Your calls", bullets(each.iter().map(String::as_str)));
}

fn pictures_row(t: &mut Top, facts: &Facts, pictures: &Pictures) {
    let label = "Pictures";
    match (pictures.visible, pictures.shown, pictures.no_difference) {
        (false, false, false) => t.row(Dot::Clear, label, "none needed"),
        (_, true, _) => t.row(Dot::Green, label, "before and after, below"),
        (_, false, true) => {
            t.row(Dot::Red, label, "no visible difference in any frame");
            t.note(
                label,
                "Every frame of the pictures flow shows no visible difference: it never \
                 reached the state the change affects."
                    .into(),
            );
        }
        (true, false, false) => match verdict::pictures_gap(facts) {
            Some(why) => match short(&why, CELL_WORDS - 1) {
                Some(why) => t.row(Dot::Red, label, format!("missing: {why}")),
                None => {
                    t.row(Dot::Red, label, "missing, see Notes");
                    t.note(label, why);
                }
            },
            None => t.row(Dot::Red, label, "missing"),
        },
    }
}

fn cubic_row(t: &mut Top, cubic: &Cubic) {
    let declared = cubic.declared_p1().len();
    let red = cubic.open_at(0) > 0 || cubic.open_at(1) > declared;
    let clean = cubic.open.is_empty() && cubic.dismissed.is_empty() && cubic.unresolved.is_empty();
    let dot = match (red, clean) {
        (true, _) => Dot::Red,
        (false, true) => Dot::Green,
        (false, false) => Dot::Amber,
    };
    t.row(dot, "Cubic", cubic.summary());
    let mut notes = Vec::new();
    notes.extend(
        cubic
            .dismissed
            .iter()
            .map(|d| format!("- Dismissed: {}", d.trim())),
    );
    notes.extend(
        cubic
            .unresolved
            .iter()
            .map(|u| format!("- Unresolved: {}", u.trim())),
    );
    if !notes.is_empty() {
        t.note("Cubic", notes.join("\n"));
    }
}

fn gaps_row(t: &mut Top, facts: &Facts) {
    let gaps: Vec<&str> = facts
        .gaps
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .collect();
    if gaps.is_empty() {
        t.row(Dot::Clear, "Gaps", "none");
        return;
    }
    let cell = match short(&facts.gaps_label, LABEL_WORDS) {
        Some(label) => label.to_string(),
        None => match gaps.as_slice() {
            [one] if words(one) <= LABEL_WORDS => one.to_string(),
            _ => format!("{}, see Notes", plural(gaps.len(), "gap")),
        },
    };
    let whole = gaps.len() == 1 && cell == gaps[0];
    t.row(Dot::Amber, "Gaps", cell);
    if !whole {
        t.note("Gaps", bullets(gaps));
    }
}

/// Rule files or Knowledge: only when something was touched.
fn paths_row(t: &mut Top, label: &'static str, paths: &[&str]) {
    if paths.is_empty() {
        return;
    }
    t.row(
        Dot::Amber,
        label,
        verdict::some_of(paths, verdict::NAMES_SHOWN),
    );
    if paths.len() > verdict::NAMES_SHOWN {
        t.note(label, bullets(paths.iter().copied()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::runs::StageAt;
    use crate::pr_body::cubic::Finding;
    use crate::pr_body::facts::{Call, PlanFollowed};

    struct Fixture {
        facts: Facts,
        size: Size,
        total: Option<Size>,
        kind: Kind,
        rules: Contract,
        build_ms: Option<i64>,
        pictures: Pictures,
        cubic: Cubic,
        rule_files: Vec<&'static str>,
        risky: Vec<&'static str>,
        checks: Vec<&'static str>,
    }

    fn lines(added: u64, removed: u64, files: u64) -> Size {
        Size {
            lines: added + removed,
            files,
            added,
            removed,
        }
    }

    impl Fixture {
        fn new() -> Self {
            Fixture {
                facts: Facts {
                    what_users_get: "Login no longer loops.".into(),
                    ..Default::default()
                },
                size: lines(120, 30, 4),
                total: None,
                kind: Kind::Bug,
                rules: Contract::default(),
                build_ms: None,
                pictures: Pictures::default(),
                cubic: Cubic::default(),
                rule_files: vec![],
                risky: vec![],
                checks: vec![],
            }
        }

        fn top(&self) -> Top {
            top(&Inputs {
                facts: &self.facts,
                size: self.size,
                total: self.total.unwrap_or(self.size),
                kind: self.kind,
                rules: &self.rules,
                build_ms: self.build_ms,
                pictures: &self.pictures,
                cubic: &self.cubic,
                rule_files: self.rule_files.clone(),
                risky: self.risky.clone(),
                checks: self.checks.clone(),
            })
        }

        fn row(&self, label: &str) -> Row {
            self.top()
                .rows
                .into_iter()
                .find(|r| r.label == label)
                .unwrap_or_else(|| panic!("no {label} row"))
        }

        fn note(&self, heading: &str) -> Option<String> {
            self.top()
                .notes
                .into_iter()
                .find(|n| n.heading == heading)
                .map(|n| n.text)
        }
    }

    fn says(row: &Row, dot: Dot, cell: &str) {
        assert_eq!((row.dot, row.cell.as_str()), (dot, cell), "{row:?}");
    }

    #[test]
    fn the_rows_come_in_order_and_the_touched_ones_only_when_touched() {
        let mut f = Fixture::new();
        let labels = |f: &Fixture| f.top().rows.iter().map(|r| r.label).collect::<Vec<_>>();
        assert_eq!(
            labels(&f),
            ["Size", "Plan", "Your calls", "Pictures", "Cubic", "Gaps"]
        );
        f.rule_files = vec!["AGENTS.md"];
        f.facts.docs_touched = vec!["docs/auth.md".into()];
        assert_eq!(
            labels(&f),
            [
                "Size",
                "Plan",
                "Your calls",
                "Pictures",
                "Cubic",
                "Gaps",
                "Rule files",
                "Knowledge"
            ]
        );
        says(&f.row("Rule files"), Dot::Amber, "AGENTS.md");
        says(&f.row("Knowledge"), Dot::Amber, "docs/auth.md");
        assert_eq!(f.top().sentence, "Login no longer loops.");
    }

    #[test]
    fn the_size_row_reads_the_diff_stats_against_the_kind_s_limit() {
        // A bug's limit is 3,000 lines or 60 files.
        let mut f = Fixture::new();
        says(&f.row("Size"), Dot::Green, "+120 −30 in 4 files");
        assert_eq!(f.note("Size"), None);
        f.build_ms = Some((2 * 60 + 10) * 60_000);
        says(
            &f.row("Size"),
            Dot::Green,
            "+120 −30 in 4 files, built in 2h 10m",
        );
        f.size = lines(1_400, 200, 4);
        says(
            &f.row("Size"),
            Dot::Amber,
            "+1,400 −200 in 4 files, built in 2h 10m",
        );
        f.size = lines(10, 0, 30);
        assert_eq!(f.row("Size").dot, Dot::Amber, "half the files");
        f.size = lines(2_900, 200, 1);
        assert_eq!(f.row("Size").dot, Dot::Red);
        assert_eq!(
            f.note("Size").unwrap(),
            "The bug limit is 3,000 lines or 60 files."
        );
        f.kind = Kind::Feature;
        assert_eq!(f.row("Size").dot, Dot::Amber, "a feature may be bigger");
        // Lockfiles and the like are left out, and the notes say so.
        f.size = lines(1, 0, 1);
        f.total = Some(lines(501, 0, 2));
        says(
            &f.row("Size"),
            Dot::Green,
            "+1 −0 in 1 file, built in 2h 10m",
        );
        assert!(f
            .note("Size")
            .unwrap()
            .starts_with("GitHub counts +501 −0 in 2 files"));
    }

    #[test]
    fn the_build_time_runs_from_the_first_stage_to_the_pr_opening() {
        let stage = |stage: &str, at_ms| StageAt {
            stage: stage.into(),
            at_ms,
        };
        let mut record = RunRecord {
            stages: vec![stage("starting", 1_000), stage("planning", 5_000)],
            ..Default::default()
        };
        assert_eq!(build_ms(&record), None, "no PR yet");
        record.stages.push(stage("pr_open", 1_000 + 45 * 60_000));
        record.stages.push(stage("pr_open", 9_000_000));
        assert_eq!(build_ms(&record), Some(45 * 60_000));
        assert_eq!(duration_words(45 * 60_000), "45m");
        assert_eq!(duration_words(10_000), "1m");
        assert_eq!(duration_words(3 * 3_600_000), "3h");
        assert_eq!(duration_words(27 * 3_600_000), "1d 3h");
    }

    #[test]
    fn the_plan_row_is_green_on_plan_amber_off_in_places_red_not_followed() {
        let mut f = Fixture::new();
        says(&f.row("Plan"), Dot::Green, "on plan");
        f.facts.plan_followed = PlanFollowed {
            followed: true,
            deviations: vec!["kept the old cookie name, since the mobile app reads it".into()],
            label: "kept the old cookie name".into(),
        };
        says(
            &f.row("Plan"),
            Dot::Amber,
            "off plan: kept the old cookie name",
        );
        assert!(f.note("Plan").unwrap().contains("the mobile app reads it"));
        f.facts.plan_followed.followed = false;
        assert_eq!(f.row("Plan").dot, Dot::Red);
        // Facts from before labels: a short deviation is its own label; a
        // long one is never cut, it goes in the notes.
        f.facts.plan_followed.label = String::new();
        says(&f.row("Plan"), Dot::Red, "off plan, see Notes");
        f.facts.plan_followed.deviations = vec!["skipped the migration".into()];
        says(&f.row("Plan"), Dot::Red, "off plan: skipped the migration");
        assert_eq!(f.note("Plan"), None);
    }

    #[test]
    fn your_calls_are_counted_with_their_labels() {
        let mut f = Fixture::new();
        says(&f.row("Your calls"), Dot::Green, "none");
        let call = |label: &str, call: &str| Call {
            call: call.into(),
            picked: "Sign in".into(),
            alternative: "Log in".into(),
            tag: "copy".into(),
            label: label.into(),
        };
        f.facts.your_calls = vec![
            call("sign-in wording", "What the button on the login page says"),
            call("", "Empty state"),
            call("", "Whether the banner shows once a session or once ever"),
        ];
        says(
            &f.row("Your calls"),
            Dot::Amber,
            "3: sign-in wording, Empty state, a copy call",
        );
        assert!(f
            .note("Your calls")
            .unwrap()
            .contains("- [copy] What the button on the login page says: Sign in (not Log in)"));
        f.facts.your_calls = (0..6)
            .map(|n| call(&format!("call number {n}"), "x"))
            .collect();
        let cell = f.row("Your calls").cell;
        assert!(cell.ends_with(" more"), "{cell}");
        assert!(words(&cell) <= CELL_WORDS, "{cell}");
    }

    #[test]
    fn the_pictures_row_is_red_when_they_are_missing_or_show_nothing() {
        let mut f = Fixture::new();
        says(&f.row("Pictures"), Dot::Clear, "none needed");
        f.pictures.visible = true;
        says(&f.row("Pictures"), Dot::Red, "missing");
        f.facts.pictures_label = "needs two sessions".into();
        says(&f.row("Pictures"), Dot::Red, "missing: needs two sessions");
        f.facts.pictures_label = String::new();
        f.facts.gaps = vec![
            "No pictures: the change only shows with two sessions in one table, which the \
             single-session picture flow cannot stage"
                .into(),
        ];
        says(&f.row("Pictures"), Dot::Red, "missing, see Notes");
        assert!(f
            .note("Pictures")
            .unwrap()
            .starts_with("the change only shows"));
        f.pictures.no_difference = true;
        says(
            &f.row("Pictures"),
            Dot::Red,
            "no visible difference in any frame",
        );
        f.pictures.no_difference = false;
        f.pictures.shown = true;
        says(&f.row("Pictures"), Dot::Green, "before and after, below");
    }

    #[test]
    fn the_cubic_row_is_red_for_an_open_p0_or_undeclared_p1() {
        let finding = |p| Finding {
            severity: Some(p),
            path: "a".into(),
            place: "line 1".into(),
        };
        let mut f = Fixture::new();
        says(&f.row("Cubic"), Dot::Green, "clean");
        f.cubic.dismissed = vec!["P2 naming: matches the old module".into()];
        says(&f.row("Cubic"), Dot::Amber, "1 dismissed");
        assert_eq!(
            f.note("Cubic").unwrap(),
            "- Dismissed: P2 naming: matches the old module"
        );
        f.cubic.open = vec![finding(3)];
        assert_eq!(f.row("Cubic").dot, Dot::Amber, "a P3 left open");
        f.cubic.open = vec![finding(1)];
        assert_eq!(f.row("Cubic").dot, Dot::Red, "an open P1 no one declared");
        f.cubic.dismissed = vec!["P1 retry: the caller retries".into()];
        assert_eq!(f.row("Cubic").dot, Dot::Amber, "declared");
        f.cubic.open = vec![finding(0)];
        says(&f.row("Cubic"), Dot::Red, "1 open (P0 ×1); 1 dismissed");
    }

    #[test]
    fn the_gaps_row_is_clear_or_amber_and_short() {
        let mut f = Fixture::new();
        says(&f.row("Gaps"), Dot::Clear, "none");
        f.facts.gaps = vec!["no local browser run".into()];
        says(&f.row("Gaps"), Dot::Amber, "no local browser run");
        assert_eq!(f.note("Gaps"), None, "said whole in the cell");
        f.facts
            .gaps
            .push("the export of a 100k-row table wasn't timed".into());
        says(&f.row("Gaps"), Dot::Amber, "2 gaps, see Notes");
        f.facts.gaps_label = "no browser run; export untimed".into();
        says(&f.row("Gaps"), Dot::Amber, "no browser run; export untimed");
        assert_eq!(
            f.note("Gaps").unwrap(),
            "- no local browser run\n- the export of a 100k-row table wasn't timed"
        );
    }

    #[test]
    fn no_cell_runs_past_twelve_words() {
        let long = "word ".repeat(40);
        let mut f = Fixture::new();
        f.size = lines(123_456, 654_321, 1_234);
        f.build_ms = Some(400 * 3_600_000);
        f.facts.what_users_get = format!("{long}. And more.");
        f.facts.plan_followed = PlanFollowed {
            followed: false,
            deviations: vec![long.clone(), long.clone()],
            label: long.clone(),
        };
        f.facts.your_calls = (0..20)
            .map(|_| Call {
                call: long.clone(),
                label: long.clone(),
                tag: "ux".into(),
                ..Default::default()
            })
            .collect();
        f.facts.gaps = vec![long.clone(), long.clone()];
        f.facts.gaps_label = long.clone();
        f.facts.pictures_label = long.clone();
        f.pictures.visible = true;
        f.cubic.dismissed = (0..9).map(|n| format!("P{} x", n % 4)).collect();
        f.cubic.unresolved = vec!["P1 y".into()];
        f.cubic.open = (0..4)
            .map(|p| Finding {
                severity: Some(p),
                path: "a".into(),
                place: "b".into(),
            })
            .collect();
        let paths: Vec<&'static str> = vec!["a/very/long/path/name.md"; 9];
        f.rule_files = paths.clone();
        f.facts.docs_touched = paths.iter().map(|p| p.to_string()).collect();
        let top = f.top();
        assert_eq!(top.rows.len(), 8);
        for row in &top.rows {
            assert!(words(&row.cell) <= CELL_WORDS, "{row:?}");
            assert!(!row.cell.contains('…'), "never cut mid-sentence: {row:?}");
        }
        // Every long thing is in the notes, whole.
        assert!(top.notes.iter().any(|n| n.heading == "What users get"));
        assert!(top.notes.iter().any(|n| n.heading == "Cubic"));
    }

    #[test]
    fn proof_and_risk_go_in_the_notes_never_the_checks() {
        let mut f = Fixture::new();
        f.facts.proof = vec![
            "typecheck passed".into(),
            "unit tests pass".into(),
            "Ran the export locally".into(),
        ];
        f.facts.flag = Some("new-login".into());
        f.risky = vec!["src/auth.rs"];
        assert_eq!(f.note("Proof").unwrap(), "- Ran the export locally");
        assert_eq!(
            f.note("Risk").unwrap(),
            "- Risky paths: src/auth.rs\n- Behind flag `new-login`"
        );
        f.facts.proof.truncate(2);
        assert_eq!(f.note("Proof"), None, "nothing past the checks");
    }
}
