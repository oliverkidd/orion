//! THE FACTORY'S WEEK: the run records of a week said in plain words —
//! week in review's Factory section ([`summary`]) — and the weekly RETRO
//! that reads them.
//!
//! - **The summary** is orion's, not a model's: one line a run — what it
//!   built, where it asked you and how long it waited, the verdict and
//!   what you said of it, the fixes sent, Cubic findings dismissed and
//!   the bugs that followed it.
//! - **The retro** runs once week in review has written its week
//!   (`week_review::finish_run`): one [`oneshot`](crate::oneshot) ask
//!   through week in review's own writer (`week_review::write`), which
//!   reads the week's records and your corrections and proposes at most
//!   [`MAX_FIXES`] concrete fixes — an orion prompt change, a rule, a repo
//!   doc change.
//! - **Its card** (`week:<last day>`) carries the summary and the fixes,
//!   with *Open week in review* and *Approve fix k*. Approve starts a
//!   session through the QUICK PROMPT's own last step, in a fresh worktree
//!   of orion's checkout (a prompt change) or the project's, the fix's
//!   prompt its first turn. The card stays up, less that button, until
//!   every fix is started.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::cards::{self, Button, Card, Press, Recipient};
use super::runs::{RunKind, RunRecord};
use crate::app::App;
use crate::flash::Flash;

/// The most fixes one retro proposes.
pub(crate) const MAX_FIXES: usize = 3;
/// How much of an owner's correction or a question goes in the prompt.
const QUOTE_CUT: usize = 400;

// ---- the summary ----

/// The checkout a run's work is in: its record's, else its first
/// session's project's.
pub(crate) fn checkout_of(app: &App, record: &RunRecord) -> Option<PathBuf> {
    if let Some(dir) = &record.checkout {
        return Some(dir.clone());
    }
    record.sessions.iter().find_map(|id| {
        let agent = app.tree.agents.iter().find(|a| &a.id.0 == id)?;
        let wt = app
            .tree
            .worktrees
            .iter()
            .find(|w| w.id == agent.worktree_id)?;
        let project = app.tree.projects.iter().find(|p| p.id == wt.project_id)?;
        Some(project.repo_path.clone())
    })
}

/// The runs that did anything between `from_ms` and `to_ms`, the latest
/// first.
pub(crate) fn in_week(records: &[RunRecord], from_ms: i64, to_ms: i64) -> Vec<RunRecord> {
    let mut week: Vec<RunRecord> = records
        .iter()
        .filter(|r| r.active_between(from_ms, to_ms))
        .cloned()
        .collect();
    week.sort_by_key(|r| std::cmp::Reverse(r.last_at_ms()));
    week
}

/// The week's runs in plain words, one line a run.
pub(crate) fn summary(records: &[RunRecord]) -> Vec<String> {
    records.iter().map(run_line).collect()
}

/// One run in plain words: `RIPLO-12 Fix the login loop (bug fix):
/// merged Wed 12 Nov, verdict Merge, which you said was wrong. It asked
/// you 2 things (plan, product) and waited 3h 20m for your answers. …`
pub(crate) fn run_line(r: &RunRecord) -> String {
    let mut head = r.identifier.clone();
    if !r.title.trim().is_empty() {
        head.push(' ');
        head.push_str(r.title.trim());
    }
    match r.kind {
        Some(RunKind::Bug) => head.push_str(" (bug fix)"),
        Some(RunKind::Feature) => head.push_str(" (feature)"),
        None => {}
    }
    let mut state = match (r.merged_at_ms, &r.pr_url, r.stages.last()) {
        (Some(at), _, _) => format!("merged {}", day(at)),
        (None, Some(_), _) => "pull request open".to_string(),
        (None, None, Some(stage)) => format!("at {}", stage.stage),
        (None, None, None) => "started".to_string(),
    };
    if let Some(verdict) = &r.verdict {
        state.push_str(&format!(", verdict {}", capital(verdict)));
        match r.verdict_right {
            Some(true) => state.push_str(", which you said was right"),
            Some(false) => state.push_str(", which you said was wrong"),
            None => {}
        }
    }
    let mut sentences = vec![format!("{head}: {state}")];
    sentences.push(asked(r));
    if !r.fixer_sends.is_empty() {
        sentences.push(format!(
            "{} sent to fix its pull request",
            count(r.fixer_sends.len(), "fix was", "fixes were")
        ));
    }
    if !r.cubic_dismissals.is_empty() {
        sentences.push(format!(
            "{} dismissed",
            count(r.cubic_dismissals.len(), "Cubic finding", "Cubic findings")
        ));
    }
    if !r.follow_ups.is_empty() {
        let what: Vec<&str> = r.follow_ups.iter().map(|f| f.what.as_str()).collect();
        sentences.push(format!("A bug followed it: {}", what.join("; ")));
    }
    format!("{}.", sentences.join(". "))
}

/// Where a run asked you, and how long it waited on you.
fn asked(r: &RunRecord) -> String {
    if r.questions.is_empty() {
        return "It never asked you anything".into();
    }
    let mut tags: Vec<&str> = r
        .questions
        .iter()
        .filter_map(|q| q.tag.as_deref())
        .collect();
    tags.sort_unstable();
    tags.dedup();
    let waited: i64 = r
        .questions
        .iter()
        .filter_map(|q| Some(q.answered_at_ms? - q.asked_at_ms))
        .filter(|ms| *ms > 0)
        .sum();
    let open = r
        .questions
        .iter()
        .filter(|q| q.answered_at_ms.is_none())
        .count();
    let mut text = format!(
        "It asked you {}",
        count(r.questions.len(), "thing", "things")
    );
    if !tags.is_empty() {
        text.push_str(&format!(" ({})", tags.join(", ")));
    }
    if waited > 0 {
        text.push_str(&format!(" and waited {} for your answers", span(waited)));
    }
    if open > 0 {
        text.push_str(&format!(
            "; {} still unanswered",
            if open == 1 { "1 is" } else { "some are" }
        ));
    }
    text
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn capital(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `Wed 12 Nov`, in local time.
fn day(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%a %-d %b")
                .to_string()
        })
        .unwrap_or_default()
}

/// `45m`, `3h 20m`, `2d 4h`.
pub(crate) fn span(ms: i64) -> String {
    let mins = (ms / orion_core::clock::MINUTE_MS).max(1);
    let (days, hours, mins) = (mins / (24 * 60), mins / 60 % 24, mins % 60);
    match (days, hours) {
        (0, 0) => format!("{mins}m"),
        (0, _) if mins == 0 => format!("{hours}h"),
        (0, _) => format!("{hours}h {mins}m"),
        (_, 0) => format!("{days}d"),
        _ => format!("{days}d {hours}h"),
    }
}

// ---- the retro ----

/// One fix the retro proposes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Fix {
    /// Ten words at most.
    pub title: String,
    /// `prompt` (orion's prompts), `rule` or `doc` (the repo's).
    pub kind: String,
    /// `orion`, or the project's name.
    pub target: String,
    /// Which runs show the need.
    pub why: String,
    /// The first prompt of the session that makes the change.
    pub prompt: String,
    /// Where that session starts, resolved as the retro lands.
    pub checkout: Option<PathBuf>,
}

/// A week's retro, as its card was posted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Retro {
    /// The week's last day, `YYYY-MM-DD`: the card is `week:<this>`.
    pub to: String,
    /// `Week 46`.
    pub name: String,
    pub summary: Vec<String>,
    pub fixes: Vec<Fix>,
    /// The fixes approved and started, numbered from 1.
    pub started: Vec<usize>,
    /// Why there are no fixes, when the ask failed.
    pub note: Option<String>,
}

impl Retro {
    fn card_key(&self) -> String {
        format!("week:{}", self.to)
    }
}

/// The retro's brief, [`MAX_FIXES`] said in it.
fn brief() -> String {
    format!(
        "\
You are reviewing one week of orion's software factory. Its agents take Linear tickets, write a plan the owner approves, build it, open a pull request that orion gives a verdict (Merge, Skim, Review or Blocked), and fix it when CI or review breaks it; the owner merges.

Below is every run of the week: where the agent asked the owner and how long he took to answer, the owner's corrections (his messages that redirected an agent), the verdict and whether he agreed with it, Cubic findings dismissed, fixes sent, and the bugs that followed the merge.

Propose at most {MAX_FIXES} concrete fixes: the ones that would have saved the owner the most waiting, correcting or rework this week. Each is one of:
- `prompt`: a change to orion's own prompts for its agents (target `orion`);
- `rule`: a rule the project's agents should follow, written into the repo's agent instructions (target: the project);
- `doc`: a change to a doc in the project's repo (target: the project).
Propose a fix only when the records show the need, naming the runs that show it; propose none when nothing recurs or one run alone shows it. Never propose metrics, dashboards, alarms, or anything about usage.

Each fix's `prompt` is the first prompt of the agent session that makes the change: what to change, where, and why, with the owner's words quoted where they matter. That agent reads nothing else from this week.

Return JSON only, nothing before or after it:
{{\"fixes\":[{{\"title\":\"<at most 10 words>\",\"kind\":\"prompt|rule|doc\",\"target\":\"orion|<project>\",\"why\":\"<at most 30 words, naming the runs>\",\"prompt\":\"<the first prompt>\"}}]}}
"
    )
}

/// What the retro asks: the brief, the projects a fix may target, and
/// every run of the week in full.
pub(crate) fn prompt(records: &[RunRecord], projects: &[String], name: &str) -> String {
    let mut text = brief();
    text.push_str(&format!("\n{name}. Projects: {}.\n", projects.join(", ")));
    for r in records {
        text.push_str(&format!("\n## {} {}\n", r.identifier, r.title.trim()));
        text.push_str(&format!("{}\n", run_line(r)));
        if let Some(url) = &r.pr_url {
            text.push_str(&format!("Pull request: {url}\n"));
        }
        if !r.verdict_reasons.is_empty() {
            text.push_str(&format!(
                "Verdict reasons: {}\n",
                r.verdict_reasons.join("; ")
            ));
        }
        for q in &r.questions {
            let waited = q
                .answered_at_ms
                .map(|at| format!("answered after {}", span(at - q.asked_at_ms)))
                .unwrap_or_else(|| "never answered".into());
            text.push_str(&format!(
                "AGENT ASKED [{}] ({waited}): {}\n",
                q.tag.as_deref().unwrap_or("question"),
                clip(&q.text)
            ));
            if let Some(answer) = &q.answer {
                text.push_str(&format!("OWNER ANSWERED: {}\n", clip(answer)));
            }
        }
        for c in &r.corrections {
            text.push_str(&format!("OWNER CORRECTED: {}\n", clip(c)));
        }
        for d in &r.cubic_dismissals {
            text.push_str(&format!("CUBIC DISMISSED: {}\n", clip(d)));
        }
        for s in &r.fixer_sends {
            text.push_str(&format!("FIX SENT: {}\n", s.reason));
        }
    }
    text
}

/// `text` on one line, cut to [`QUOTE_CUT`] chars.
fn clip(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::ui::truncate(&flat, QUOTE_CUT)
}

/// The fixes in a reply: its JSON object, wherever it sits, at most
/// [`MAX_FIXES`] of them, each with a title and a prompt.
pub(crate) fn parse(reply: &str) -> Result<Vec<Fix>, String> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        fixes: Vec<Fix>,
    }
    let (Some(start), Some(end)) = (reply.find('{'), reply.rfind('}')) else {
        return Err("the retro answered without its JSON".into());
    };
    let parsed: Reply = serde_json::from_str(&reply[start..=end])
        .map_err(|e| format!("the retro's JSON didn't read: {e}"))?;
    Ok(parsed
        .fixes
        .into_iter()
        .filter(|f| !f.title.trim().is_empty() && !f.prompt.trim().is_empty())
        .take(MAX_FIXES)
        .collect())
}

/// Where a fix for `target` starts: orion's own checkout for `orion`, the
/// project of that name, else the one checkout every run of the week
/// shared.
pub(crate) fn checkout_for(app: &App, target: &str, weeks: &[PathBuf]) -> Option<PathBuf> {
    let target = target.trim();
    if target.eq_ignore_ascii_case("orion") {
        return orion_checkout(app);
    }
    if let Some(p) = app
        .tree
        .projects
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(target))
    {
        return Some(p.repo_path.clone());
    }
    let mut dirs = weeks.to_vec();
    dirs.sort();
    dirs.dedup();
    match dirs.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The project that is orion's own source: the one with its TUI crate.
fn orion_checkout(app: &App) -> Option<PathBuf> {
    app.tree
        .projects
        .iter()
        .find(|p| p.repo_path.join("crates/orion-tui/Cargo.toml").is_file())
        .map(|p| p.repo_path.clone())
}

/// What the retro on `records` — the runs of the week ending `to`, called
/// `name` — asks, with the retro it fills and the checkouts its runs
/// worked in; None with no runs, when there is nothing to look back on.
/// Week in review sends it through its own writer, and its answer comes
/// back to [`landed`].
pub(crate) fn ask(
    app: &App,
    records: &[RunRecord],
    to: &str,
    name: &str,
) -> Option<(Retro, Vec<PathBuf>, String)> {
    if records.is_empty() {
        return None;
    }
    let mut projects: Vec<String> = app.tree.projects.iter().map(|p| p.name.clone()).collect();
    projects.push("orion".into());
    let retro = Retro {
        to: to.to_string(),
        name: name.to_string(),
        summary: summary(records),
        ..Retro::default()
    };
    let weeks = records.iter().filter_map(|r| checkout_of(app, r)).collect();
    let text = prompt(records, &projects, name);
    Some((retro, weeks, text))
}

/// The retro's reply landed: its fixes resolved to a checkout each, kept,
/// and the week's card posted.
pub(crate) fn landed(
    app: &mut App,
    mut retro: Retro,
    weeks: &[PathBuf],
    reply: Result<String, String>,
) {
    match reply.and_then(|text| parse(&text)) {
        Ok(fixes) => {
            retro.fixes = fixes
                .into_iter()
                .map(|f| Fix {
                    checkout: checkout_for(app, &f.target, weeks),
                    ..f
                })
                .collect();
        }
        Err(why) => {
            tracing::warn!("factory retro: {why}");
            retro.note = Some(why);
        }
    }
    keep(app, retro.clone());
    cards::post(app, card(&retro));
}

/// The week's card: the summary, the fixes, and their buttons.
pub(crate) fn card(retro: &Retro) -> Card {
    let mut body: Vec<String> = retro
        .summary
        .iter()
        .map(|line| format!("• {}", cards::escape(line)))
        .collect();
    if body.is_empty() {
        body.push("No factory runs this week.".into());
    }
    match (&retro.note, retro.fixes.is_empty()) {
        (Some(why), _) => body.push(format!(
            "_The retro couldn't be written: {}_",
            cards::escape(why)
        )),
        (None, true) => body.push("*Fixes proposed*: none this week.".into()),
        (None, false) => {
            body.push("*Fixes proposed*".into());
            for (i, fix) in retro.fixes.iter().enumerate() {
                let k = i + 1;
                let mut line = format!(
                    "*{k}.* {} ({}, in {}): {}",
                    cards::escape(&fix.title),
                    cards::escape(&fix.kind),
                    cards::escape(&fix.target),
                    cards::escape(&fix.why)
                );
                if retro.started.contains(&k) {
                    line.push_str(" ✓ started");
                }
                body.push(line);
            }
        }
    }
    let mut buttons = vec![Button::tap("open", "Open week in review")];
    for k in 1..=retro.fixes.len() {
        if !retro.started.contains(&k) {
            buttons.push(Button::tap(
                format!("approve:{k}"),
                format!("Approve fix {k}"),
            ));
        }
    }
    Card {
        key: retro.card_key(),
        to: Recipient::Owner,
        context: "Week in review".into(),
        title: format!("{}: the factory", retro.name),
        body,
        buttons,
        image: None,
    }
}

/// A press on a `week:` card (`factory::cards::on_action`).
pub(crate) fn on_action(app: &mut App, press: Press) {
    let Some(to) = press.key.strip_prefix("week:") else {
        return;
    };
    let Some(mut retro) = find(app, to) else {
        tracing::warn!("factory retro: no retro kept for {}", press.key);
        return;
    };
    match (press.action, press.action.strip_prefix("approve:")) {
        ("open", _) => {
            let at = app
                .week_review
                .kept
                .iter()
                .position(|s| s.meta.to == retro.to)
                .unwrap_or(0);
            let live = usize::from(app.week_review.run.is_some());
            crate::week_review::open(app, at + live, false);
            // Slack took the buttons off with the press: put them back.
            cards::post(app, card(&retro));
        }
        (_, Some(k)) if press.by_owner => {
            let k: usize = k.parse().unwrap_or(0);
            let Some(fix) = retro.fixes.get(k.wrapping_sub(1)).cloned() else {
                return;
            };
            if retro.started.contains(&k) {
                return;
            }
            match start_fix(app, &fix) {
                Ok(()) => {
                    retro.started.push(k);
                    app.flash = Some(Flash::done(format!("Fix {k} started: {}", fix.title)));
                }
                Err(why) => {
                    app.flash = Some(Flash::failed(format!("Fix {k} not started: {why}")));
                }
            }
            keep(app, retro.clone());
            if retro.started.len() >= retro.fixes.len() {
                cards::close(app, press.key, "Every fix started");
            } else {
                cards::post(app, card(&retro));
            }
        }
        (other, _) => tracing::info!(
            "factory retro: {other} on {} (owner: {})",
            press.key,
            press.by_owner
        ),
    }
}

/// Start the session that makes `fix`: a fresh worktree of its
/// checkout's project, named for it, the fix's prompt its first turn —
/// the QUICK PROMPT's own last step, as though typed into a box there.
fn start_fix(app: &mut App, fix: &Fix) -> Result<(), String> {
    use crate::quick_prompt::{QuickLaunch, QuickTarget};
    let Some(checkout) = &fix.checkout else {
        return Err(format!("no project here is {}", fix.target));
    };
    let Some(project) = app
        .tree
        .projects
        .iter()
        .find(|p| &p.repo_path == checkout)
        .map(|p| p.id.clone())
    else {
        return Err(format!("{} is no longer a project", checkout.display()));
    };
    let slug = match crate::branch_name::title_slug(&fix.title) {
        slug if slug.is_empty() => "factory-fix".to_string(),
        slug => slug,
    };
    let branch = crate::branch_name::unused(&slug, &app.project_branches(&project));
    let target = QuickTarget::NewWorktree {
        project,
        branch,
        existing: false,
    };
    let launch = QuickLaunch::from_config(target, &crate::config::Config::load());
    let mut out = Vec::new();
    crate::event_loop::submit_approved(app, launch, fix.prompt.clone(), &mut out);
    app.outbox.extend(out);
    Ok(())
}

// ---- keeping ----

/// `<data dir>/automations/retros`, one file a week.
fn dir() -> PathBuf {
    crate::automations::state_dir().join("retros")
}

/// Keep `retro` on the app, and on disk when the loop installed week in
/// review's directory (never under test).
fn keep(app: &mut App, retro: Retro) {
    if app.week_review.root.is_some() {
        let path = dir().join(format!("{}.json", safe(&retro.to)));
        if let Err(err) = crate::pr_cache::write_json_atomic(&path, &retro) {
            tracing::warn!("factory retro not kept: {err}");
        }
    }
    app.week_review.retros.insert(retro.to.clone(), retro);
}

/// The retro of the week ending `to`: on the app, else on disk.
fn find(app: &mut App, to: &str) -> Option<Retro> {
    if let Some(retro) = app.week_review.retros.get(to) {
        return Some(retro.clone());
    }
    app.week_review.root.as_ref()?;
    let bytes = std::fs::read(dir().join(format!("{}.json", safe(to)))).ok()?;
    let retro: Retro = serde_json::from_slice(&bytes).ok()?;
    app.week_review.retros.insert(to.to_string(), retro.clone());
    Some(retro)
}

fn safe(name: &str) -> String {
    name.chars()
        .map(|c| match c.is_ascii_alphanumeric() || c == '-' {
            true => c,
            false => '_',
        })
        .collect()
}

/// Every retro kept on the app, for a test to read.
#[cfg(test)]
pub(crate) fn kept(app: &App) -> &std::collections::HashMap<String, Retro> {
    &app.week_review.retros
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::cards::CardState;

    #[test]
    fn the_brief_asks_for_at_most_max_fixes() {
        assert!(brief().contains(&format!("at most {MAX_FIXES} concrete fixes")));
        assert!(brief().contains(r#"{"fixes":[{"title":"#));
    }
    use crate::factory::runs::{FixerSend, FollowUp, Question, StageAt};

    const H: i64 = 60 * 60 * 1000;

    fn record() -> RunRecord {
        RunRecord {
            identifier: "RIPLO-12".into(),
            title: "Fix the login loop".into(),
            kind: Some(RunKind::Bug),
            questions: vec![
                Question {
                    text: "Approve the plan?".into(),
                    tag: Some("plan".into()),
                    asked_at_ms: 0,
                    answered_at_ms: Some(3 * H + 20 * 60_000),
                    answer: Some("Yes".into()),
                },
                Question {
                    text: "Which redirect?".into(),
                    tag: Some("product".into()),
                    asked_at_ms: 4 * H,
                    answered_at_ms: None,
                    answer: None,
                },
            ],
            stages: vec![StageAt {
                stage: "building".into(),
                at_ms: H,
            }],
            pr_url: Some("https://github.com/o/r/pull/7".into()),
            verdict: Some("merge".into()),
            verdict_right: Some(false),
            cubic_dismissals: vec!["P1: unused import".into()],
            fixer_sends: vec![FixerSend {
                at_ms: 5 * H,
                reason: "ci: lint".into(),
                to: "fix login".into(),
                fresh: false,
            }],
            corrections: vec!["No, keep the old redirect for admins.".into()],
            merged_at_ms: Some(6 * H),
            follow_ups: vec![FollowUp {
                what: "#9 fix: login again".into(),
                at_ms: 30 * H,
            }],
            ..RunRecord::default()
        }
    }

    #[test]
    fn a_run_reads_in_plain_words() {
        let line = run_line(&record());
        assert!(
            line.starts_with("RIPLO-12 Fix the login loop (bug fix): merged "),
            "{line}"
        );
        assert!(
            line.contains(", verdict Merge, which you said was wrong. "),
            "{line}"
        );
        assert!(
            line.contains(
                "It asked you 2 things (plan, product) and waited 3h 20m for your answers; 1 is still unanswered."
            ),
            "{line}"
        );
        assert!(
            line.contains("1 fix was sent to fix its pull request."),
            "{line}"
        );
        assert!(line.contains("1 Cubic finding dismissed."), "{line}");
        assert!(
            line.ends_with("A bug followed it: #9 fix: login again."),
            "{line}"
        );
        let quiet = RunRecord {
            identifier: "RIPLO-3".into(),
            ..RunRecord::default()
        };
        assert_eq!(
            run_line(&quiet),
            "RIPLO-3: started. It never asked you anything."
        );
    }

    #[test]
    fn a_span_reads_as_minutes_hours_or_days() {
        assert_eq!(span(30_000), "1m");
        assert_eq!(span(45 * 60_000), "45m");
        assert_eq!(span(3 * H), "3h");
        assert_eq!(span(3 * H + 20 * 60_000), "3h 20m");
        assert_eq!(span(52 * H), "2d 4h");
    }

    #[test]
    fn only_the_weeks_runs_are_summarised_latest_first() {
        let old = RunRecord {
            identifier: "RIPLO-1".into(),
            merged_at_ms: Some(1),
            ..RunRecord::default()
        };
        let mut late = record();
        late.identifier = "RIPLO-2".into();
        let week = in_week(&[old, record(), late], 2 * H, 40 * H);
        let ids: Vec<&str> = week.iter().map(|r| r.identifier.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&"RIPLO-1"));
    }

    #[test]
    fn the_prompt_carries_corrections_and_questions() {
        let text = prompt(&[record()], &["riplo-os".into(), "orion".into()], "Week 46");
        assert!(text.contains("Projects: riplo-os, orion."));
        assert!(text.contains("OWNER CORRECTED: No, keep the old redirect for admins."));
        assert!(text.contains("AGENT ASKED [plan] (answered after 3h 20m): Approve the plan?"));
        assert!(text.contains("AGENT ASKED [product] (never answered): Which redirect?"));
        assert!(text.contains("FIX SENT: ci: lint"));
    }

    #[test]
    fn a_reply_gives_at_most_three_fixes() {
        let fix = r#"{"title":"Ask fewer plan questions","kind":"prompt","target":"orion","why":"RIPLO-12","prompt":"Change it"}"#;
        let reply = format!("Here:\n{{\"fixes\":[{fix},{fix},{fix},{fix}]}}\nDone");
        let fixes = parse(&reply).unwrap();
        assert_eq!(fixes.len(), 3);
        assert_eq!(fixes[0].target, "orion");
        assert!(parse("no json").is_err());
        let empty = r#"{"fixes":[{"title":"","prompt":"x"}]}"#;
        assert!(parse(empty).unwrap().is_empty());
    }

    #[test]
    fn a_target_finds_its_checkout() {
        use orion_core::{Project, ProjectId};
        let mut app = App::new();
        app.tree.projects.push(Project {
            id: ProjectId("p".into()),
            name: "riplo-os".into(),
            repo_path: "/tmp/riplo-os".into(),
            sort_order: 0,
        });
        assert_eq!(
            checkout_for(&app, "Riplo-OS", &[]),
            Some(PathBuf::from("/tmp/riplo-os"))
        );
        let one = [PathBuf::from("/x"), PathBuf::from("/x")];
        assert_eq!(checkout_for(&app, "web", &one), Some(PathBuf::from("/x")));
        assert_eq!(checkout_for(&app, "web", &[]), None);
    }

    fn landed_retro(app: &mut App) -> Retro {
        let retro = Retro {
            to: "2026-11-14".into(),
            name: "Week 46".into(),
            summary: summary(&[record()]),
            ..Retro::default()
        };
        let reply = r#"{"fixes":[
            {"title":"Ask fewer plan questions","kind":"prompt","target":"orion","why":"RIPLO-12","prompt":"a"},
            {"title":"Note the admin redirect","kind":"rule","target":"nowhere","why":"RIPLO-12","prompt":"b"}]}"#;
        landed(app, retro, &[], Ok(reply.into()));
        kept(app)["2026-11-14"].clone()
    }

    #[test]
    fn the_retro_posts_one_card_with_a_button_a_fix() {
        let mut app = App::new();
        let retro = landed_retro(&mut app);
        assert_eq!(retro.fixes.len(), 2);
        let posted = &app.factory_cards[0];
        assert_eq!(posted.card.key, "week:2026-11-14");
        let labels: Vec<&str> = posted
            .card
            .buttons
            .iter()
            .map(|b| b.label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["Open week in review", "Approve fix 1", "Approve fix 2"]
        );
        assert!(posted.card.body[0].starts_with("• RIPLO-12 Fix the login loop"));
        assert!(posted
            .card
            .body
            .iter()
            .any(|l| l == "*2.* Note the admin redirect (rule, in nowhere): RIPLO-12"));
    }

    #[test]
    fn a_fix_with_no_checkout_is_not_started_and_its_button_stays() {
        let mut app = App::new();
        landed_retro(&mut app);
        crate::factory::cards::on_action(&mut app, "week:2026-11-14", "approve:2", None, true);
        assert!(app.outbox.iter().all(|r| !matches!(
            r,
            orion_core::ClientRequest::CreateWorktree { .. }
                | orion_core::ClientRequest::CreateAgent { .. }
        )));
        assert!(kept(&app)["2026-11-14"].started.is_empty());
        let posted = &app.factory_cards[0];
        assert_eq!(
            posted.state,
            CardState::Open,
            "posted again, buttons and all"
        );
        assert_eq!(posted.card.buttons.len(), 3);
        // Only the owner approves.
        crate::factory::cards::on_action(&mut app, "week:2026-11-14", "approve:1", None, false);
        assert!(kept(&app)["2026-11-14"].started.is_empty());
    }

    #[test]
    fn a_failed_retro_still_posts_the_summary() {
        let mut app = App::new();
        let retro = Retro {
            to: "2026-11-14".into(),
            name: "Week 46".into(),
            summary: summary(&[record()]),
            ..Retro::default()
        };
        landed(&mut app, retro, &[], Err("timed out".into()));
        let card = &app.factory_cards[0].card;
        assert_eq!(card.buttons.len(), 1);
        assert!(card
            .body
            .iter()
            .any(|l| l == "_The retro couldn't be written: timed out_"));
    }
}
