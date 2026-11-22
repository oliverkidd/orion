//! RULE REVIEW: the morning's sign-off of the rules orion harvested, a
//! sheet of its own over the grid — and, until it is signed off, a banner
//! across the top of the screen.
//!
//! At 09:00, or on any launch while rules wait, the sheet opens with every
//! rule waiting, each project under its name: the owner's own first
//! (YOURS), then the agent's (INFERRED BY THE AGENT), then the rules a
//! weekly scan found hold repo-wide (MOVE INTO THE AGENT DOCS). Everything
//! starts ticked.
//!
//! - `space` unticks a rule (it is dropped) or a promotion (turned down,
//!   never offered again).
//! - `n` writes a note on the rule under the cursor: on a ticked rule it
//!   says how it is wrong, and the rule is rewritten to fit; on an
//!   unticked one, why it goes. Every drop and note is a lesson later
//!   harvests read.
//! - `enter` asks to sign off, `enter` again does it (`knowledge::review::sign_off`):
//!   the branch is pushed, and orion merges its pull request once checks
//!   pass.
//! - `esc` closes the sheet with the picks kept; the BANNER then stays on
//!   the screen's top row, saying how long the rules have waited, and goes
//!   crimson after four hours. A click on it, or **Review rules** in the
//!   COMMAND PALETTE, opens the sheet again.
//!
//! The picks are kept per project in `<data_dir>/knowledge/<id>/review-draft.json`,
//! so a restart doesn't lose them.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use crate::app::{App, HitTarget, Overlay};
#[cfg(not(test))]
use crate::knowledge::local::Local;
use crate::knowledge::review::{self, Candidate, ProjectPending, SignOff, Verdict};
use crate::knowledge::store::Entry;
use crate::theme::Theme;

/// The draft file in a project's knowledge folder.
#[cfg(not(test))]
const DRAFT_FILE: &str = "review-draft.json";
/// After this long waiting, the banner goes from the attention tint to
/// full crimson.
const LOUD_AFTER_MS: i64 = 4 * 60 * orion_core::clock::MINUTE_MS;

/// What the TUI holds of the review between frames.
#[derive(Debug, Clone)]
pub struct ReviewState {
    /// Every project's pending review, as the last refresh found it.
    pub(crate) pending: Vec<ProjectPending>,
    /// Whether a refresh has landed since launch.
    pub(crate) loaded: bool,
    /// Still to decide whether this launch opens the sheet.
    pub(crate) launch_check: bool,
    /// The sheet should open at the next free moment.
    pub(crate) want_open: bool,
    /// The local date (`YYYY-MM-DD`) the sheet last opened on its own —
    /// on launch or at 09:00 — so the day's review opens once.
    pub(crate) opened_on: String,
}

impl Default for ReviewState {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            loaded: false,
            launch_check: true,
            want_open: false,
            opened_on: String::new(),
        }
    }
}

impl ReviewState {
    /// How many items wait, every project's.
    pub(crate) fn count(&self) -> usize {
        self.pending.iter().map(ProjectPending::count).sum()
    }
}

/// A pick on one item: kept or not, and the owner's note.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Pick {
    pub keep: bool,
    pub note: String,
}

/// What one row of the sheet is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    /// A project's name, and its pull request.
    Project(String),
    /// A section heading.
    Heading(&'static str),
    /// A rule: its project's index in the view, and the entry.
    Rule(usize, Box<Entry>),
    /// A promotion: its project's index, and the candidate.
    Promote(usize, Candidate),
}

impl Row {
    /// Whether it is an item the cursor lands on (a rule or a promotion).
    fn is_item(&self) -> bool {
        matches!(self, Row::Rule(..) | Row::Promote(..))
    }

    /// The key its pick is kept under: the entry's id, or `promote:` and
    /// the candidate's first id.
    fn key(&self) -> Option<String> {
        match self {
            Row::Rule(_, e) => Some(e.id.clone()),
            Row::Promote(_, c) => Some(format!("promote:{}", c.key())),
            _ => None,
        }
    }
}

/// The open sheet.
#[derive(Debug, Clone, Default)]
pub struct RuleReviewView {
    pub(crate) projects: Vec<ProjectPending>,
    pub(crate) rows: Vec<Row>,
    /// Every pick, by [`Row::key`]; an item not in it is kept with no note.
    pub(crate) picks: BTreeMap<String, Pick>,
    /// The index into `rows` of the item under the cursor.
    pub(crate) cursor: usize,
    /// The note being written on the cursor's item.
    pub(crate) editing: Option<String>,
    /// The sign-off line is up.
    pub(crate) confirming: bool,
    pub area: Rect,
}

impl RuleReviewView {
    fn new(projects: Vec<ProjectPending>) -> Self {
        let mut picks = BTreeMap::new();
        let mut rows = Vec::new();
        for (i, p) in projects.iter().enumerate() {
            picks.extend(load_draft(&p.id));
            let pr =
                p.pr.as_ref()
                    .map(|pr| format!(" · PR #{}", pr.number))
                    .unwrap_or_default();
            rows.push(Row::Project(format!("{}{pr}", p.name)));
            let (yours, agents): (Vec<&Entry>, Vec<&Entry>) =
                p.rules.iter().partition(|e| e.is_owners());
            if !yours.is_empty() {
                rows.push(Row::Heading("YOURS"));
                rows.extend(yours.into_iter().map(|e| Row::Rule(i, Box::new(e.clone()))));
            }
            if !agents.is_empty() {
                rows.push(Row::Heading("INFERRED BY THE AGENT"));
                rows.extend(
                    agents
                        .into_iter()
                        .map(|e| Row::Rule(i, Box::new(e.clone()))),
                );
            }
            if !p.promote.is_empty() {
                rows.push(Row::Heading("MOVE INTO THE AGENT DOCS"));
                rows.extend(p.promote.iter().map(|c| Row::Promote(i, c.clone())));
            }
        }
        let cursor = rows.iter().position(Row::is_item).unwrap_or(0);
        Self {
            projects,
            rows,
            picks,
            cursor,
            ..Self::default()
        }
    }

    /// The pick on `row`.
    fn pick(&self, row: &Row) -> Pick {
        row.key()
            .and_then(|k| self.picks.get(&k).cloned())
            .unwrap_or(Pick {
                keep: true,
                note: String::new(),
            })
    }

    fn set_pick(&mut self, row: &Row, pick: Pick) {
        if let Some(k) = row.key() {
            self.picks.insert(k, pick);
        }
    }

    /// The items, every project's.
    fn items(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.is_item())
    }

    /// (kept, dropped, noted) over the rules.
    fn tally(&self) -> (usize, usize, usize) {
        let mut out = (0, 0, 0);
        for row in self.items().filter(|r| matches!(r, Row::Rule(..))) {
            let p = self.pick(row);
            match (p.keep, p.note.trim().is_empty()) {
                (true, true) => out.0 += 1,
                (true, false) => out.2 += 1,
                (false, _) => out.1 += 1,
            }
        }
        out
    }

    /// Move the cursor `by` items.
    fn step(&mut self, down: bool) {
        let items: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_item())
            .map(|(i, _)| i)
            .collect();
        let Some(at) = items.iter().position(|&i| i == self.cursor) else {
            return;
        };
        let next = match down {
            true => (at + 1).min(items.len().saturating_sub(1)),
            false => at.saturating_sub(1),
        };
        self.cursor = items[next];
    }

    /// Every project's sign-off, from the picks.
    fn sign_offs(&self) -> Vec<SignOff> {
        self.projects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut one = SignOff {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    root: p.root.clone(),
                    verdicts: Vec::new(),
                    promote: Vec::new(),
                    decline: Vec::new(),
                };
                for row in self.items() {
                    let pick = self.pick(row);
                    match row {
                        Row::Rule(at, e) if *at == i => one.verdicts.push(Verdict {
                            id: e.id.clone(),
                            keep: pick.keep,
                            note: pick.note.trim().to_string(),
                        }),
                        Row::Promote(at, c) if *at == i => match pick.keep {
                            true => one.promote.push(c.clone()),
                            false => one.decline.push(c.clone()),
                        },
                        _ => {}
                    }
                }
                one
            })
            .filter(|one| {
                !one.verdicts.is_empty() || !one.promote.is_empty() || !one.decline.is_empty()
            })
            .collect()
    }

    /// Keep the picks on disk, a file a project.
    fn save_drafts(&self) {
        for (i, p) in self.projects.iter().enumerate() {
            let mine: BTreeMap<String, Pick> = self
                .rows
                .iter()
                .filter(|r| matches!(r, Row::Rule(at, _) | Row::Promote(at, _) if *at == i))
                .filter_map(|r| Some((r.key()?, self.picks.get(&r.key()?)?.clone())))
                .collect();
            save_draft(&p.id, &mine);
        }
    }
}

#[cfg(not(test))]
fn draft_path(id: &str) -> std::path::PathBuf {
    Local::of(id).dir.join(DRAFT_FILE)
}

#[cfg(test)]
fn load_draft(_id: &str) -> BTreeMap<String, Pick> {
    BTreeMap::new()
}

#[cfg(test)]
fn save_draft(_id: &str, _picks: &BTreeMap<String, Pick>) {}

/// Forget project `id`'s picks: its sign-off went through.
pub(crate) fn clear_draft(id: &str) {
    save_draft(id, &BTreeMap::new());
}

/// Whether anything waiting is due a review now (`knowledge::review::cutoff`).
fn due(app: &App) -> bool {
    let cutoff = review::cutoff(chrono::Local::now());
    app.rule_review.pending.iter().any(|p| p.due(cutoff))
}

#[cfg(not(test))]
fn load_draft(id: &str) -> BTreeMap<String, Pick> {
    std::fs::read_to_string(draft_path(id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

#[cfg(not(test))]
fn save_draft(id: &str, picks: &BTreeMap<String, Pick>) {
    let path = draft_path(id);
    match picks.is_empty() {
        true => {
            let _ = std::fs::remove_file(path);
        }
        false => {
            let _ = crate::pr_cache::write_json_atomic(&path, picks);
        }
    }
}

// ---- the background's hooks ----

/// A refresh landed (`knowledge::runner::tick`): keep what it found, and
/// on the first one since launch ask for the sheet when anything is due.
pub(crate) fn refreshed(app: &mut App, found: Vec<ProjectPending>) {
    let before = app.rule_review.count();
    app.rule_review.pending = found;
    app.rule_review.loaded = true;
    if std::mem::take(&mut app.rule_review.launch_check) && due(app) {
        app.rule_review.want_open = true;
        app.rule_review.opened_on = chrono::Local::now().format("%Y-%m-%d").to_string();
    }
    if before != app.rule_review.count() {
        app.dirty = true;
    }
}

/// Open the sheet when it was asked for and nothing else is up.
pub(crate) fn maybe_open(app: &mut App) {
    if app.rule_review.want_open && app.overlay.is_none() && app.editor.is_none() {
        app.rule_review.want_open = false;
        if app.rule_review.count() > 0 {
            open(app);
        }
    }
}

/// Put the sheet up with every item waiting; with none, say so. Opening it
/// — or asking to — is when what waits is worked out again
/// (`knowledge::review::want_refresh`), each project's knowledge base
/// branch looked up afresh (`knowledge::share::forget_bases`).
pub(crate) fn open(app: &mut App) {
    crate::knowledge::review::want_refresh();
    crate::knowledge::share::forget_bases();
    if app.rule_review.count() == 0 {
        app.flash = Some(crate::flash::Flash::note("No rules wait on a review"));
        app.dirty = true;
        return;
    }
    let projects: Vec<ProjectPending> = app
        .rule_review
        .pending
        .iter()
        .filter(|p| p.count() > 0)
        .cloned()
        .collect();
    app.overlay = Some(Overlay::RuleReview(Box::new(RuleReviewView::new(projects))));
    app.dirty = true;
}

/// Close the sheet, the picks kept; the banner takes over.
pub(crate) fn dismiss(app: &mut App) {
    if let Some(Overlay::RuleReview(view)) = &app.overlay {
        view.save_drafts();
    }
    app.overlay = None;
    app.dirty = true;
}

/// The sheet's keys: one table [`handle_key`] matches and the border
/// spells.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const WALK: Key = Key::new(&["up", "down"], "move").also(&["k", "j"]).show(2);
    pub const TOGGLE: Key = Key::new(&["space"], "keep/drop");
    pub const NOTE: Key = Key::new(&["n"], "note");
    pub const SIGN: Key = Key::new(&["enter"], "sign off");
    pub const LATER: Key = crate::nav::CLOSE.doing("later");
    #[cfg(test)]
    pub const ALL: &[Key] = &[WALK, TOGGLE, NOTE, SIGN, LATER];
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::RuleReview(view)) = &mut app.overlay else {
        return;
    };
    // Writing a note: the keys are its text.
    if let Some(text) = &mut view.editing {
        match key.code {
            _ if crate::nav::closes(&key) => view.editing = None,
            KeyCode::Enter => {
                let note = text.trim().to_string();
                view.editing = None;
                if let Some(row) = view.rows.get(view.cursor).cloned() {
                    let mut pick = view.pick(&row);
                    pick.note = note;
                    view.set_pick(&row, pick);
                    view.save_drafts();
                }
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                text.push(c)
            }
            _ => return,
        }
        app.dirty = true;
        return;
    }
    if view.confirming {
        match key.code {
            KeyCode::Enter => {
                let all = view.sign_offs();
                let n = view.items().count();
                app.overlay = None;
                // What is being signed off waits on nothing more here.
                app.rule_review.pending.clear();
                app.flash = Some(crate::flash::Flash::working(format!(
                    "Signing off {}…",
                    crate::flash::plural(n, "item")
                )));
                review::sign_off(all);
            }
            _ => view.confirming = false,
        }
        app.dirty = true;
        return;
    }
    match key.code {
        _ if keys::LATER.matches(&key) => return dismiss(app),
        KeyCode::Down | KeyCode::Char('j') => view.step(true),
        KeyCode::Up | KeyCode::Char('k') => view.step(false),
        KeyCode::Char(' ') => {
            if let Some(row) = view.rows.get(view.cursor).cloned() {
                let mut pick = view.pick(&row);
                pick.keep = !pick.keep;
                view.set_pick(&row, pick);
                view.save_drafts();
            }
        }
        KeyCode::Char('n') => {
            if let Some(row @ Row::Rule(..)) = view.rows.get(view.cursor).cloned() {
                view.editing = Some(view.pick(&row).note);
            }
        }
        KeyCode::Enter => view.confirming = true,
        _ => return,
    }
    app.dirty = true;
}

/// The sheet: each project's items, the cursor's marked; under it the
/// tally, or the sign-off line, or the note being written.
pub fn draw(f: &mut Frame, app: &mut App, view: &RuleReviewView, th: Theme) {
    let width = crate::onboard::sheet_inner_width(f.area());
    let body = body(view, th, width);
    let hints = [
        keys::SIGN.hint().kept(),
        keys::TOGGLE.hint().kept(),
        keys::NOTE.hint(),
        keys::WALK.hint(),
        keys::LATER.hint().kept(),
    ];
    let today = chrono::Local::now().format("%a %-d %b").to_string();
    let explain = match (&view.editing, view.confirming) {
        (Some(_), _) => "enter saves the note · esc leaves it · a note on a kept rule rewrites it; on a dropped one it says why".to_string(),
        (None, true) => {
            let (kept, dropped, noted) = view.tally();
            format!("Sign off {} · {kept} keep · {dropped} drop · {noted} fix?   enter signs off · esc goes back", crate::flash::plural(kept + dropped + noted, "rule"))
        }
        (None, false) => "Nothing merges until you sign off. A note on a rule says what's wrong: orion rewrites it and learns from it.".to_string(),
    };
    let area = crate::onboard::draw_sheet(
        f,
        &format!(" Rule review · {today} "),
        None,
        body,
        &explain,
        &hints,
        false,
        th,
    );
    if let Some(Overlay::RuleReview(live)) = &mut app.overlay {
        live.area = area;
    }
}

/// The sheet's lines.
fn body(view: &RuleReviewView, th: Theme, width: u16) -> crate::onboard::Body {
    let mut body = crate::onboard::Body::new(width);
    let dim = Style::default().fg(th.dim);
    let (kept, dropped, noted) = view.tally();
    let promos = view
        .items()
        .filter(|r| matches!(r, Row::Promote(..)))
        .count();
    let mut lead = format!(
        "{} since your last sign-off · {kept} keep · {dropped} drop · {noted} fix",
        crate::flash::plural(kept + dropped + noted, "rule")
    );
    if promos > 0 {
        lead.push_str(&format!(
            " · {} for the agent docs",
            crate::flash::plural(promos, "rule")
        ));
    }
    body.prose(&lead, width, dim);
    let mut n = 0;
    for (i, row) in view.rows.iter().enumerate() {
        let on = i == view.cursor;
        match row {
            Row::Project(name) => {
                body.blank();
                body.lines.push(Line::from(Span::styled(
                    format!(" {name}"),
                    Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
                )));
            }
            Row::Heading(h) => {
                body.blank();
                body.lines.push(Line::from(Span::styled(
                    format!("   {h}"),
                    Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
                )));
            }
            Row::Rule(_, e) => {
                n += 1;
                let pick = view.pick(row);
                item_lines(
                    &mut body,
                    on,
                    pick.keep,
                    &format!("{n:>2}  {}", e.rule),
                    &format!("#{}", e.pr),
                    th,
                    width,
                );
                if !e.quote.trim().is_empty() {
                    let quote = format!("“{}”", crate::ui::truncate(e.quote.trim(), 160));
                    body.item("", &quote, 11, width, dim.add_modifier(Modifier::ITALIC));
                }
                let note = match (&view.editing, on) {
                    (Some(t), true) => Some(format!("{t}▏")),
                    _ => (!pick.note.trim().is_empty()).then(|| pick.note.clone()),
                };
                if let Some(note) = note {
                    body.item("✎ ", &note, 9, width, Style::default().fg(th.accent));
                }
                if on {
                    body.selected = Some(body.lines.len() - 1);
                }
            }
            Row::Promote(_, c) => {
                let pick = view.pick(row);
                item_lines(
                    &mut body,
                    on,
                    pick.keep,
                    &format!("    {}", c.rule),
                    "",
                    th,
                    width,
                );
                if !c.why.trim().is_empty() {
                    body.item("", c.why.trim(), 11, width, dim);
                }
                if on {
                    body.selected = Some(body.lines.len() - 1);
                }
            }
        }
    }
    body
}

/// One item's first lines: the cursor mark, its box, its text wrapped,
/// and `tag` (its PR) after the first line.
#[allow(clippy::too_many_arguments)]
fn item_lines(
    body: &mut crate::onboard::Body,
    on: bool,
    keep: bool,
    text: &str,
    tag: &str,
    th: Theme,
    width: u16,
) {
    let mark = if on { " › " } else { "   " };
    let check = if keep { "[✓] " } else { "[ ] " };
    let style = match keep {
        true => Style::default().fg(th.text),
        false => Style::default()
            .fg(th.faint)
            .add_modifier(Modifier::CROSSED_OUT),
    };
    let style = if on {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    };
    let hang = mark.chars().count() + check.chars().count();
    let tag_w = if tag.is_empty() {
        0
    } else {
        tag.chars().count() + 2
    };
    let room = usize::from(width).saturating_sub(hang + tag_w + 1).max(8);
    for (i, line) in crate::pr_preview::wrap(text, room).into_iter().enumerate() {
        let mut spans = Vec::new();
        if i == 0 {
            spans.push(Span::styled(mark, Style::default().fg(th.accent)));
            spans.push(Span::styled(
                check,
                Style::default().fg(if keep { th.ok } else { th.dim }),
            ));
        } else {
            spans.push(Span::raw(" ".repeat(hang)));
        }
        let len = line.chars().count();
        spans.push(Span::styled(line, style));
        if i == 0 && !tag.is_empty() {
            let pad = room.saturating_sub(len) + 2;
            spans.push(Span::styled(
                format!("{}{tag}", " ".repeat(pad)),
                Style::default().fg(th.dim),
            ));
        }
        body.lines.push(Line::from(spans));
    }
}

// ---- the banner ----

/// Whether the banner is up: something is due a review (a rule recorded
/// before this morning's 09:00, or a promotion), and the sheet isn't open.
pub(crate) fn banner_up(app: &App) -> bool {
    !matches!(app.overlay, Some(Overlay::RuleReview(_))) && due(app)
}

/// The banner's words: how many rules (and promotions) wait, whose, and
/// how long since this morning's review time.
fn banner_text(pending: &[ProjectPending], waited_ms: i64, older: bool) -> String {
    let rules: usize = pending.iter().map(|p| p.rules.len()).sum();
    let promos: usize = pending.iter().map(|p| p.promote.len()).sum();
    let names: Vec<&str> = pending
        .iter()
        .filter(|p| p.count() > 0)
        .map(|p| p.name.as_str())
        .collect();
    let lead = if older {
        "Yesterday's rules too · "
    } else {
        ""
    };
    let mut what = crate::flash::plural(rules, "rule");
    if promos > 0 {
        what.push_str(&format!(" and {promos} for the agent docs"));
    }
    let hour = 60 * orion_core::clock::MINUTE_MS;
    let age = match waited_ms / hour {
        0 => format!("{}m", (waited_ms / orion_core::clock::MINUTE_MS).max(1)),
        h => format!("{h}h"),
    };
    format!(
        " ◆ {lead}{what} wait for your sign-off · {}'s knowledge won't merge until you do · waiting {age}",
        names.join(", ")
    )
}

/// The banner on `area` (one row): how many rules wait and for how long,
/// on the attention tint — full crimson once they have waited four hours,
/// or a day's review was skipped — and a click opens the sheet.
pub(crate) fn draw_banner(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let now = chrono::Local::now();
    let since = review::cutoff(now);
    let waited = (now - since).num_milliseconds().max(0);
    let older = app
        .rule_review
        .pending
        .iter()
        .flat_map(|p| p.rules.iter())
        .any(|e| {
            chrono::DateTime::parse_from_rfc3339(&e.recorded_at)
                .is_ok_and(|at| at < since - chrono::Duration::days(1))
        });
    let bg = match waited >= LOUD_AFTER_MS || older {
        true => th.err,
        false => th.err_sweep[0],
    };
    let text = banner_text(&app.rule_review.pending, waited, older);
    let action = " review ▸ ";
    let room = usize::from(area.width).saturating_sub(action.chars().count());
    let shown = crate::ui::truncate(&text, room);
    let pad = room.saturating_sub(shown.chars().count());
    let style = Style::default()
        .fg(th.on_accent)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let line = Line::from(vec![
        Span::styled(format!("{shown}{}", " ".repeat(pad)), style),
        Span::styled(action, style.add_modifier(Modifier::UNDERLINED)),
    ]);
    f.render_widget(Paragraph::new(line), area);
    app.hits.push((area, HitTarget::RuleReviewBanner));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::store::ACTIVE;

    fn entry(id: &str, by: &str) -> Entry {
        Entry {
            id: id.into(),
            pr: 12,
            by: by.into(),
            rule: format!("rule {id}"),
            quote: "said so".into(),
            status: ACTIVE.into(),
            recorded_at: "2026-11-18T10:00:00Z".into(),
            ..Entry::default()
        }
    }

    fn pending() -> ProjectPending {
        ProjectPending {
            id: "01TEST-REVIEW-NONE".into(),
            name: "app".into(),
            root: "/nonexistent".into(),
            rules: vec![entry("k1-1", "owner"), entry("k1-2", "agent")],
            promote: vec![Candidate {
                ids: vec!["k0-1".into()],
                rule: "Go through the registry.".into(),
                why: "repo-wide".into(),
            }],
            pr: None,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_sheet_groups_yours_the_agents_and_the_promotions() {
        let view = RuleReviewView::new(vec![pending()]);
        let kinds: Vec<String> = view
            .rows
            .iter()
            .map(|r| match r {
                Row::Project(p) => format!("P:{p}"),
                Row::Heading(h) => h.to_string(),
                Row::Rule(_, e) => e.id.clone(),
                Row::Promote(_, c) => format!("promote:{}", c.key()),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "P:app",
                "YOURS",
                "k1-1",
                "INFERRED BY THE AGENT",
                "k1-2",
                "MOVE INTO THE AGENT DOCS",
                "promote:k0-1"
            ]
        );
        assert_eq!(view.cursor, 2, "on the first rule");
        assert_eq!(view.tally(), (2, 0, 0), "everything starts kept");
    }

    #[test]
    fn space_drops_n_notes_and_enter_twice_signs_off() {
        let mut app = App::new();
        app.rule_review.pending = vec![pending()];
        open(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::RuleReview(_))));
        handle_key(&mut app, key(KeyCode::Char(' ')));
        handle_key(&mut app, key(KeyCode::Down));
        handle_key(&mut app, key(KeyCode::Char('n')));
        for c in "too narrow".chars() {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        handle_key(&mut app, key(KeyCode::Enter));
        handle_key(&mut app, key(KeyCode::Down));
        handle_key(&mut app, key(KeyCode::Char(' ')));
        let Some(Overlay::RuleReview(view)) = &app.overlay else {
            panic!("the sheet is up");
        };
        assert_eq!(view.tally(), (0, 1, 1));
        let all = view.sign_offs();
        assert_eq!(all.len(), 1);
        assert_eq!(
            all[0].verdicts,
            vec![
                Verdict {
                    id: "k1-1".into(),
                    keep: false,
                    note: String::new()
                },
                Verdict {
                    id: "k1-2".into(),
                    keep: true,
                    note: "too narrow".into()
                },
            ]
        );
        assert!(all[0].promote.is_empty());
        assert_eq!(
            all[0].decline.len(),
            1,
            "the unticked promotion is turned down"
        );
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(matches!(&app.overlay, Some(Overlay::RuleReview(v)) if v.confirming));
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(
            matches!(&app.overlay, Some(Overlay::RuleReview(v)) if !v.confirming),
            "esc backs out of the sign-off line"
        );
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.overlay.is_none(), "esc closes it");
        assert!(banner_up(&app), "and the banner takes over");
    }

    #[test]
    fn the_first_refresh_opens_the_sheet_only_when_something_is_due() {
        let mut app = App::new();
        let mut later = pending();
        later.promote.clear();
        for e in &mut later.rules {
            e.recorded_at = (chrono::Local::now() + chrono::Duration::hours(1)).to_rfc3339();
        }
        refreshed(&mut app, vec![later]);
        assert!(!app.rule_review.want_open, "only rules recorded after nine");
        assert!(app.rule_review.loaded);
        refreshed(&mut app, vec![pending()]);
        assert!(!app.rule_review.want_open, "launch is decided once");

        let mut app = App::new();
        refreshed(&mut app, vec![pending()]);
        assert!(app.rule_review.want_open);
        maybe_open(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::RuleReview(_))));
    }

    #[test]
    fn the_banner_counts_the_rules_and_promotions_and_their_wait() {
        let text = banner_text(
            &[pending()],
            2 * 60 * orion_core::clock::MINUTE_MS + 5,
            false,
        );
        assert_eq!(
            text,
            " ◆ 2 rules and 1 for the agent docs wait for your sign-off · app's knowledge won't merge until you do · waiting 2h"
        );
        let text = banner_text(&[pending()], 0, true);
        assert!(text.starts_with(" ◆ Yesterday's rules too · "), "{text}");
        assert!(text.ends_with("waiting 1m"), "{text}");
    }

    #[test]
    fn every_key_is_in_the_table() {
        assert_eq!(keys::ALL.len(), 5);
    }
}
