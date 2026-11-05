//! EDIT PULL REQUEST: the pull request under the cursor, all of it in one
//! form — its title, its base (a retarget), draft or ready, its labels
//! (from the repo's), reviewers and assignees (from the people the repo can
//! assign), and its state: closed with a comment and its branch's fate, or
//! reopened. `⌘;` opens it on the title, `⌘W` on its State row set to close,
//! and `Enter` on a closed row on its State row set to reopen, the caret in
//! the base.
//!
//! What GitHub says of the pull request — and what the repo offers — is one
//! GraphQL query as the form opens ([`EDIT_QUERY`]); every field starts on
//! what the row already knows and takes GitHub's word when it lands, unless
//! it was changed meanwhile. Enter sends only what changed ([`Plan`]), in
//! the order GitHub allows: a closed pull request can't be retargeted, so
//! it is reopened first, and one whose base was deleted can't be reopened,
//! so its base is put back first, at the commit it last pointed at — and
//! deleted again once the pull request has moved off it. Then `gh pr edit`,
//! `gh pr ready`, and `gh pr close` (the branch deleted after, as the merge
//! form does it, never a long-lived one).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::ProjectId;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{
    check, delete_branch, form, gh, keys, modal, put_form, run_piped, stacked_on, stays_note,
    Answer, PrForm, REQUEST_TIMEOUT,
};
use crate::app::App;
use crate::hints::Hint;
use crate::pr_preview::INDENT;
use crate::pull_request::{str_at, PrLabel, STATE_CLOSED, STATE_MERGED, STATE_OPEN};
use crate::text_input::{TextInput, TextView};
use crate::theme::Theme;
use crate::ui::{
    form_box, form_field, form_frame, form_label, form_text_box, fuzzy_highlight_styled,
    render_row, row_rect, truncate,
};

/// The one ask the form opens with: the pull request as it stands, and
/// what the repo offers its pick rows.
const EDIT_QUERY: &str = "query($owner: String!, $repo: String!, $number: Int!) { \
    repository(owner: $owner, name: $repo) { \
    pullRequest(number: $number) { title state isDraft baseRefName baseRefOid \
    headRefName headRefOid isCrossRepository author { login } baseRef { name } headRef { name } \
    labels(first: 50) { nodes { name } } \
    reviewRequests(first: 50) { nodes { requestedReviewer { ... on User { login } } } } \
    assignees(first: 50) { nodes { login } } } \
    labels(first: 100, orderBy: {field: NAME, direction: ASC}) { nodes { name color } } \
    assignableUsers(first: 100) { nodes { login } } } }";

/// How wide the edit form's labels are drawn: `Reviewers `.
const LABEL_W: usize = 10;

/// Where the form opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditStart {
    /// `⌘;`: on the title.
    Title,
    /// `⌘W`: on the State row, set to close.
    Close,
    /// `Enter` on a closed row: set to reopen, the caret in the base.
    Reopen,
}

/// What Enter does to the pull request's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateChange {
    #[default]
    Keep,
    Close,
    Reopen,
}

/// The edit form's rows, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditRow {
    Title,
    Base,
    Draft,
    Labels,
    Reviewers,
    Assignees,
    State,
    /// Closing: whether its branch goes too.
    Delete,
    /// Closing or reopening: what is left on it as it does.
    Comment,
}

impl EditRow {
    /// A row that lists what to pick from while it has the caret.
    pub fn picks(self) -> bool {
        matches!(
            self,
            EditRow::Base | EditRow::Labels | EditRow::Reviewers | EditRow::Assignees
        )
    }

    /// A row that ticks several of its list — the rest pick one.
    fn ticks(self) -> bool {
        self.picks() && self != EditRow::Base
    }

    fn label(self) -> &'static str {
        match self {
            EditRow::Title => "Title",
            EditRow::Base => "Base",
            EditRow::Draft => "Draft",
            EditRow::Labels => "Labels",
            EditRow::Reviewers => "Reviewers",
            EditRow::Assignees => "Assignees",
            EditRow::State => "State",
            EditRow::Delete => "Delete",
            EditRow::Comment => "Comment",
        }
    }
}

/// What GitHub says of the pull request: what the form's edits are
/// measured against.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    pub title: String,
    /// `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    pub draft: bool,
    pub base: String,
    /// The commit the base last pointed at — where a deleted base is put
    /// back to reopen on.
    pub base_oid: String,
    /// The base branch is gone from GitHub.
    pub base_gone: bool,
    pub head: String,
    pub head_oid: String,
    pub head_gone: bool,
    /// From a fork: its branch is not this repo's to delete or put back.
    pub fork: bool,
    pub author: String,
    pub labels: Vec<String>,
    pub reviewers: Vec<String>,
    pub assignees: Vec<String>,
}

impl Facts {
    fn closed(&self) -> bool {
        self.state == STATE_CLOSED
    }

    fn merged(&self) -> bool {
        self.state == STATE_MERGED
    }
}

/// What the repo offers the pick rows: its labels, and the people it can
/// assign (who can also be asked to review).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Offers {
    pub labels: Vec<PrLabel>,
    pub people: Vec<String>,
}

/// `⌘;`: the pull request under the cursor, edited in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditForm {
    pub project: ProjectId,
    pub dir: PathBuf,
    pub number: u64,
    pub url: String,
    /// What GitHub said — or, until it has, what the row knew.
    pub facts: Facts,
    /// GitHub's answer is in: Enter sends nothing before it.
    pub read: bool,
    pub title: TextInput,
    pub base: TextInput,
    pub draft: bool,
    pub labels: Vec<String>,
    pub reviewers: Vec<String>,
    pub assignees: Vec<String>,
    pub state: StateChange,
    pub delete_branch: bool,
    /// Why the branch can't go with it, once GitHub has said
    /// (`long_lived`): the box stays off.
    pub stays: Option<String>,
    pub comment: TextInput,
    pub row: EditRow,
    /// What a ticking row's list is narrowed by.
    pub filter: TextInput,
    /// The base row was typed in since the caret came to it: its list
    /// narrows to the text then.
    pub typed: bool,
    /// The cursor in the list under a pick row.
    pub pick: usize,
    /// Origin's branches, once read.
    pub branches: Option<Arc<[String]>>,
    pub offers: Offers,
    pub ticket: u64,
    pub saving: Option<String>,
    pub notice: Option<String>,
    /// As of the last draw: each row's rect and the list's rows.
    pub rows: Vec<(Rect, EditRow)>,
    pub picks: Vec<(Rect, usize)>,
}

impl EditForm {
    /// The rows Tab walks: Delete only while closing, Comment while closing
    /// or reopening.
    fn order(&self) -> Vec<EditRow> {
        let mut rows = vec![
            EditRow::Title,
            EditRow::Base,
            EditRow::Draft,
            EditRow::Labels,
            EditRow::Reviewers,
            EditRow::Assignees,
            EditRow::State,
        ];
        if self.state == StateChange::Close {
            rows.push(EditRow::Delete);
        }
        if self.state != StateChange::Keep {
            rows.push(EditRow::Comment);
        }
        rows
    }

    /// The row after `row` (or the one before), round either end.
    fn step(&self, forward: bool) -> EditRow {
        super::step_round(&self.order(), self.row, forward)
    }

    /// The caret onto `row`, its list on what it holds.
    pub fn focus(&mut self, row: EditRow) {
        self.row = row;
        self.typed = false;
        self.filter.clear();
        let base = self.base.trim().to_string();
        self.pick = match row {
            EditRow::Base => self
                .choices()
                .iter()
                .position(|(name, _)| *name == base)
                .unwrap_or(0),
            _ => 0,
        };
    }

    /// The states Enter can leave it in, for the State row to walk.
    fn state_choices(&self) -> &'static [StateChange] {
        if self.facts.merged() {
            &[StateChange::Keep]
        } else if self.facts.closed() {
            &[StateChange::Keep, StateChange::Reopen]
        } else {
            &[StateChange::Keep, StateChange::Close]
        }
    }

    /// The State row's word for `state`.
    fn state_word(&self, state: StateChange) -> &'static str {
        match state {
            StateChange::Keep if self.facts.merged() => "merged",
            StateChange::Keep if self.facts.closed() => "leave it closed",
            StateChange::Keep => "leave it open",
            StateChange::Close => "close it",
            StateChange::Reopen => "reopen it",
        }
    }

    /// The pull request's own branch, when it is this repo's to delete:
    /// not a fork's, still there, and not its base.
    pub(super) fn deletable(&self) -> Option<String> {
        let f = &self.facts;
        (self.read && !f.fork && !f.head_gone && !f.head.is_empty() && f.head != f.base)
            .then(|| f.head.clone())
    }

    /// The list a pick row shows, every option or — typed in — the fuzzy
    /// matches, best first: each name with the chars matched.
    pub fn choices(&self) -> Vec<(String, Vec<usize>)> {
        let mut all: Vec<String> = match self.row {
            EditRow::Base => self
                .branches
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|b| **b != self.facts.head)
                .cloned()
                .collect(),
            EditRow::Labels => self.offers.labels.iter().map(|l| l.name.clone()).collect(),
            EditRow::Reviewers => self
                .offers
                .people
                .iter()
                .filter(|p| **p != self.facts.author)
                .cloned()
                .collect(),
            EditRow::Assignees => self.offers.people.clone(),
            _ => return Vec::new(),
        };
        // What it already has stays pickable, whatever the repo listed.
        for held in self.ticked().into_iter().flatten() {
            if !all.contains(held) {
                all.push(held.clone());
            }
        }
        let query = match self.row {
            EditRow::Base if self.typed => self.base.trim(),
            EditRow::Base => "",
            _ => self.filter.trim(),
        };
        if query.is_empty() {
            return all.into_iter().map(|name| (name, Vec::new())).collect();
        }
        crate::fuzzy::rank(query, all.iter().map(String::as_str))
            .into_iter()
            .map(|(i, at)| (all[i].clone(), at))
            .collect()
    }

    /// What a ticking row has ticked.
    fn ticked(&self) -> Option<&Vec<String>> {
        match self.row {
            EditRow::Labels => Some(&self.labels),
            EditRow::Reviewers => Some(&self.reviewers),
            EditRow::Assignees => Some(&self.assignees),
            _ => None,
        }
    }

    fn ticked_mut(&mut self) -> Option<&mut Vec<String>> {
        match self.row {
            EditRow::Labels => Some(&mut self.labels),
            EditRow::Reviewers => Some(&mut self.reviewers),
            EditRow::Assignees => Some(&mut self.assignees),
            _ => None,
        }
    }

    /// The name under the list's cursor: ticked or unticked on a ticking
    /// row, taken into the base on the base row.
    fn take_pick(&mut self) {
        let Some((name, _)) = self.choices().get(self.pick).cloned() else {
            return;
        };
        self.notice = None;
        if self.row == EditRow::Base {
            self.base.set_text(name);
            self.typed = false;
        } else if let Some(ticked) = self.ticked_mut() {
            match ticked.iter().position(|t| *t == name) {
                Some(at) => {
                    ticked.remove(at);
                }
                None => ticked.push(name),
            }
        }
    }

    fn move_pick(&mut self, delta: i64) {
        let len = self.choices().len();
        self.pick = crate::app::clamp_selection(self.pick as i64 + delta, len);
    }

    /// The text field under the caret: the title, the base, a ticking
    /// row's filter, the comment.
    fn input_mut(&mut self) -> Option<&mut TextInput> {
        match self.row {
            EditRow::Title => Some(&mut self.title),
            EditRow::Base => Some(&mut self.base),
            EditRow::Comment => Some(&mut self.comment),
            row if row.ticks() => Some(&mut self.filter),
            _ => None,
        }
    }

    /// The facts GitHub just gave, taken into every field still holding
    /// what the form last knew — never over what was changed since.
    fn adopt(&mut self, facts: Facts) {
        let old = std::mem::replace(&mut self.facts, facts);
        let new = &self.facts;
        if self.title.as_str() == old.title {
            self.title.set_text(new.title.clone());
        }
        if self.base.trim() == old.base {
            self.base.set_text(new.base.clone());
        }
        if self.draft == old.draft {
            self.draft = new.draft;
        }
        for (now, was, is) in [
            (&mut self.labels, &old.labels, &new.labels),
            (&mut self.reviewers, &old.reviewers, &new.reviewers),
            (&mut self.assignees, &old.assignees, &new.assignees),
        ] {
            if now == was {
                *now = is.clone();
            }
        }
        if !self.state_choices().contains(&self.state) {
            self.state = StateChange::Keep;
        }
        self.read = true;
    }

    /// What Enter would send, measured against what GitHub said; or why it
    /// can't be sent.
    pub fn plan(&self) -> Result<Plan, String> {
        let f = &self.facts;
        let (title, base) = (self.title.trim(), self.base.trim());
        if !self.read {
            return Err(format!("reading #{} — try again in a moment", self.number));
        }
        if title.is_empty() {
            return Err("the pull request needs a title".into());
        }
        if base.is_empty() {
            return Err("which branch does it merge into?".into());
        }
        let retarget = base != f.base;
        let redraft = self.draft != f.draft;
        if f.merged() && (retarget || redraft) {
            return Err("it has merged — its base and draft state stay as they are".into());
        }
        let open_after = !f.closed() || self.state == StateChange::Reopen;
        if !open_after && (retarget || redraft) {
            return Err("a closed pull request keeps its base — reopen it to move it".into());
        }
        let mut plan = Plan {
            number: self.number,
            state: self.state,
            comment: self.comment.trim().to_string(),
            ..Plan::default()
        };
        if title != f.title {
            plan.flag("--title", title);
            plan.said.push("title".into());
        }
        if retarget {
            plan.flag("--base", base);
            plan.base = Some(base.to_string());
            plan.said.push(format!("base → {base}"));
        }
        let lists = [
            ("label", &self.labels, &f.labels, ""),
            ("reviewer", &self.reviewers, &f.reviewers, "review "),
            ("assignee", &self.assignees, &f.assignees, "assigned "),
        ];
        for (what, now, was, word) in lists {
            for name in now.iter().filter(|n| !was.contains(n)) {
                plan.flag(&format!("--add-{what}"), name);
                plan.said.push(format!("{word}+{name}"));
            }
            for name in was.iter().filter(|n| !now.contains(n)) {
                plan.flag(&format!("--remove-{what}"), name);
                plan.said.push(format!("{word}−{name}"));
            }
        }
        if redraft {
            plan.ready = Some(!self.draft);
        }
        if self.state == StateChange::Reopen {
            if f.base_gone {
                plan.restore_base = Some((f.base.clone(), f.base_oid.clone()));
            }
            if f.head_gone && !f.fork {
                plan.restore_head = Some((f.head.clone(), f.head_oid.clone()));
            }
        }
        if self.state == StateChange::Close && self.delete_branch && self.stays.is_none() {
            plan.delete = self.deletable();
        }
        if plan.is_empty() {
            return Err("nothing to change".into());
        }
        Ok(plan)
    }
}

/// What the edit form sends, in the order [`save`] sends it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub number: u64,
    /// `gh pr edit`'s flags, each with its value.
    pub edit: Vec<String>,
    /// The base it moves to.
    pub base: Option<String>,
    /// `gh pr ready` (true) or `--undo` (false).
    pub ready: Option<bool>,
    pub state: StateChange,
    pub comment: String,
    /// Closing: its branch, to go after it.
    pub delete: Option<String>,
    /// Reopening: a deleted base, and a deleted head, put back first — by
    /// name, at a commit.
    pub restore_base: Option<(String, String)>,
    pub restore_head: Option<(String, String)>,
    /// What the edit changes, for the form's Saves row and the flash.
    pub said: Vec<String>,
}

impl Plan {
    fn flag(&mut self, flag: &str, value: &str) {
        self.edit.push(flag.to_string());
        self.edit.push(value.to_string());
    }

    fn is_empty(&self) -> bool {
        self.edit.is_empty() && self.ready.is_none() && self.state == StateChange::Keep
    }

    /// What it does, in a line: `title · base → main · +bug · reopen`.
    pub fn summary(&self) -> String {
        let mut parts = self.said.clone();
        match self.ready {
            Some(true) => parts.push("ready for review".into()),
            Some(false) => parts.push("back to draft".into()),
            None => {}
        }
        match self.state {
            StateChange::Close => parts.push("close".into()),
            StateChange::Reopen => parts.push("reopen".into()),
            StateChange::Keep => {}
        }
        parts.join(" · ")
    }
}

/// Send `plan` for the pull request in `dir`'s repo: what was done, or —
/// at the first refusal — what was done before it and why it stopped.
pub async fn save(dir: &Path, plan: Plan) -> Result<String, String> {
    let n = plan.number.to_string();
    let mut did: Vec<String> = Vec::new();
    let stopped = |did: &[String], why: String| {
        if did.is_empty() {
            why
        } else {
            format!("{}; then {why}", did.join("; "))
        }
    };
    let mut restored = None;
    if plan.state == StateChange::Reopen {
        if let Some((name, oid)) = &plan.restore_base {
            create_branch(dir, name, oid)
                .await
                .map_err(|why| format!("couldn't put {name} back to reopen on: {why}"))?;
            restored = Some(name.clone());
            did.push(format!("put {name} back"));
        }
        if let Some((name, oid)) = &plan.restore_head {
            if let Err(why) = create_branch(dir, name, oid).await {
                take_back(dir, restored.as_deref()).await;
                return Err(format!("couldn't put {name} back to reopen it: {why}"));
            }
            did.push(format!("put {name} back"));
        }
        let mut args = vec!["pr", "reopen", n.as_str()];
        if !plan.comment.is_empty() {
            args.extend(["--comment", plan.comment.as_str()]);
        }
        if let Err(why) = run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await {
            take_back(dir, restored.as_deref()).await;
            return Err(format!("couldn't reopen #{n}: {why}"));
        }
        did.push(format!("reopened #{n}"));
    }
    if !plan.edit.is_empty() {
        let mut args = vec!["pr", "edit", n.as_str()];
        args.extend(plan.edit.iter().map(String::as_str));
        if let Err(why) = run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await {
            return Err(stopped(&did, format!("couldn't edit #{n}: {why}")));
        }
        did.push(format!("#{n}: {}", plan.said.join(" · ")));
    }
    // A base put back only to reopen on goes again once nothing is on it.
    if let (Some(name), Some(_)) = (&restored, &plan.base) {
        if take_back(dir, Some(name)).await {
            did.push(format!("deleted {name} again"));
        } else {
            did.push(format!("{name} stays on GitHub"));
        }
    }
    if let Some(ready) = plan.ready {
        let mut args = vec!["pr", "ready", n.as_str()];
        if !ready {
            args.push("--undo");
        }
        if let Err(why) = run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await {
            return Err(stopped(
                &did,
                format!("couldn't change its draft state: {why}"),
            ));
        }
        did.push(if ready {
            "ready for review".into()
        } else {
            "a draft again".into()
        });
    }
    if plan.state == StateChange::Close {
        match super::close(dir, plan.number, &plan.comment, plan.delete.as_deref()).await {
            Ok(fate) => did.push(super::closed_message(plan.number, &fate)),
            Err(why) => return Err(stopped(&did, format!("couldn't close #{n}: {why}"))),
        }
    }
    Ok(did.join("; "))
}

/// Branch `name` made on GitHub at commit `oid`.
async fn create_branch(dir: &Path, name: &str, oid: &str) -> Result<(), String> {
    let at = format!("ref=refs/heads/{name}");
    let sha = format!("sha={oid}");
    let args = [
        "api",
        "-X",
        "POST",
        "repos/{owner}/{repo}/git/refs",
        "-f",
        &at,
        "-f",
        &sha,
    ];
    run_piped(gh(dir, &args), "", REQUEST_TIMEOUT)
        .await
        .map(|_| ())
}

/// A branch put back to reopen on, deleted again — only while no open pull
/// request is based on it. Whether it went.
async fn take_back(dir: &Path, name: Option<&str>) -> bool {
    let Some(name) = name else {
        return false;
    };
    stacked_on(dir, name).await.is_some_and(|on| on.is_empty()) && delete_branch(dir, name).await
}

/// [`EDIT_QUERY`]'s answer.
fn parse_read(json: &str) -> Option<(Facts, Offers)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let repo = v.pointer("/data/repository")?;
    let pr = repo.get("pullRequest")?;
    let names = |v: &serde_json::Value, path: &str, key: &str| -> Vec<String> {
        v.pointer(path)
            .and_then(|n| n.as_array())
            .into_iter()
            .flatten()
            .filter_map(|n| n.pointer(key).and_then(|s| s.as_str()))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    };
    let facts = Facts {
        title: str_at(pr, "title"),
        state: str_at(pr, "state"),
        draft: crate::pull_request::bool_at(pr, "isDraft"),
        base: str_at(pr, "baseRefName"),
        base_oid: str_at(pr, "baseRefOid"),
        base_gone: pr.get("baseRef").is_none_or(|r| r.is_null()),
        head: str_at(pr, "headRefName"),
        head_oid: str_at(pr, "headRefOid"),
        head_gone: pr.get("headRef").is_none_or(|r| r.is_null()),
        fork: crate::pull_request::bool_at(pr, "isCrossRepository"),
        author: pr
            .pointer("/author/login")
            .and_then(|l| l.as_str())
            .unwrap_or_default()
            .to_string(),
        labels: names(pr, "/labels/nodes", "/name"),
        reviewers: names(pr, "/reviewRequests/nodes", "/requestedReviewer/login"),
        assignees: names(pr, "/assignees/nodes", "/login"),
    };
    let labels = repo
        .pointer("/labels/nodes")
        .and_then(|n| n.as_array())
        .into_iter()
        .flatten()
        .map(|l| PrLabel {
            name: str_at(l, "name"),
            color: str_at(l, "color"),
        })
        .filter(|l| !l.name.is_empty())
        .collect();
    let offers = Offers {
        labels,
        people: names(repo, "/assignableUsers/nodes", "/login"),
    };
    Some((facts, offers))
}

// ---- opening, landing ----

/// The edit form for the pull request under the cursor, opened `at`: on
/// what the row knows, GitHub asked for the rest. A merged one has nothing
/// left to edit here that matters as much as what it merged: the footer
/// says so.
pub(crate) fn open(app: &mut App, at: EditStart) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
        return;
    };
    let status = app.prs.status_or_open(&pr.url);
    if status.standing == crate::pull_request::Standing::Merged {
        app.flash = Some(crate::flash::Flash::note(format!(
            "#{} has merged",
            pr.number
        )));
        return;
    }
    let closed = status.standing == crate::pull_request::Standing::Closed;
    let base = app
        .pr_detail
        .get(&pr.url)
        .map(|d| d.base.clone())
        .unwrap_or_default();
    let facts = Facts {
        title: status.title_or(&pr.title).to_string(),
        state: if closed { STATE_CLOSED } else { STATE_OPEN }.to_string(),
        draft: status.is_draft(),
        base: base.clone(),
        head: pr.head.clone(),
        labels: pr.meta.labels.iter().map(|l| l.name.clone()).collect(),
        ..Facts::default()
    };
    let (state, row) = match at {
        EditStart::Title => (StateChange::Keep, EditRow::Title),
        EditStart::Close if closed => (StateChange::Keep, EditRow::State),
        EditStart::Close => (StateChange::Close, EditRow::State),
        EditStart::Reopen if closed => (StateChange::Reopen, EditRow::Base),
        EditStart::Reopen => (StateChange::Keep, EditRow::Base),
    };
    let mut form = EditForm {
        project,
        dir: dir.clone(),
        number: pr.number,
        url: pr.url.clone(),
        title: TextInput::with_text(facts.title.clone()),
        base: TextInput::with_text(base),
        draft: facts.draft,
        labels: facts.labels.clone(),
        reviewers: Vec::new(),
        assignees: Vec::new(),
        facts,
        read: false,
        state,
        delete_branch: false,
        stays: None,
        comment: TextInput::multiline(),
        row,
        filter: TextInput::new(),
        typed: false,
        pick: 0,
        branches: None,
        offers: Offers::default(),
        ticket: crate::view_jobs::ticket(),
        saving: None,
        notice: None,
        rows: Vec::new(),
        picks: Vec::new(),
    };
    form.focus(row);
    let ticket = form.ticket;
    let number = form.number;
    put_form(app, PrForm::Edit(form));
    if let Some(tx) = app.pr_actions_tx.clone() {
        ask_read(&tx, dir.clone(), number, ticket);
        let tx_branches = tx;
        tokio::task::spawn_blocking(move || {
            let (_, bases, _) = super::read_branches(&dir, "");
            let _ = tx_branches.send(Answer::EditBranches { ticket, bases });
        });
    }
    app.dirty = true;
}

/// [`read`] off the loop, its answer to the form `ticket` names.
fn ask_read(
    tx: &tokio::sync::mpsc::UnboundedSender<Answer>,
    dir: PathBuf,
    number: u64,
    ticket: u64,
) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = read(&dir, number).await;
        let _ = tx.send(Answer::EditRead { ticket, result });
    });
}

/// Ask GitHub for pull request `number` as it stands, and what its repo
/// offers.
async fn read(dir: &Path, number: u64) -> Result<Box<(Facts, Offers)>, String> {
    let query = format!("query={EDIT_QUERY}");
    let number = format!("number={number}");
    let args = [
        "api",
        "graphql",
        "-F",
        "owner={owner}",
        "-F",
        "repo={repo}",
        "-F",
        &number,
        "-f",
        &query,
    ];
    let out = run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await?;
    parse_read(&out)
        .map(Box::new)
        .ok_or_else(|| "GitHub's answer had no pull request in it".into())
}

/// The edit form up in the modal, when it is the one `ticket` asked for.
fn edit_form(app: &mut App, ticket: Option<u64>) -> Option<&mut EditForm> {
    match form(app)? {
        PrForm::Edit(f) if ticket.is_none_or(|t| t == f.ticket) => Some(f),
        _ => None,
    }
}

/// GitHub's word on the pull request is in: the fields still on what the
/// form knew take it, the repo's offers fill the pick rows, and whether its
/// branch is long-lived is asked. Refused, the form says why — and Enter
/// stays held.
pub(crate) fn land_read(app: &mut App, ticket: u64, result: Result<Box<(Facts, Offers)>, String>) {
    let Some(form) = edit_form(app, Some(ticket)) else {
        return;
    };
    match result {
        Ok(read) => {
            let (facts, offers) = *read;
            form.adopt(facts);
            form.offers = offers;
            if form.row.picks() {
                form.focus(form.row);
            }
            super::ask_long_lived(app);
        }
        Err(why) => form.notice = Some(format!("couldn't read #{}: {why}", form.number)),
    }
}

/// Origin's branches are in, for the base row.
pub(crate) fn land_branches(app: &mut App, ticket: u64, bases: Vec<String>) {
    if let Some(form) = edit_form(app, Some(ticket)) {
        form.branches = Some(bases.into());
        if form.row == EditRow::Base && !form.typed {
            form.focus(EditRow::Base);
        }
    }
}

/// Enter's answer: done, the form closes onto the list and the footer says
/// what changed, the page and the list read again; refused, the form says
/// why on its frame and reads the pull request again — part of it may have
/// gone through — so a second Enter sends only what is still to do.
pub(crate) fn land_saved(
    app: &mut App,
    project: &ProjectId,
    ticket: u64,
    url: String,
    result: Result<String, String>,
) {
    super::read_again(app, project, url);
    match result {
        Ok(said) => {
            if edit_form(app, Some(ticket)).is_some() {
                super::close_form(app);
            }
            app.flash = Some(crate::flash::Flash::done(said));
        }
        Err(why) => {
            let Some(form) = edit_form(app, Some(ticket)) else {
                app.flash = Some(crate::flash::Flash::failed(why));
                return;
            };
            form.saving = None;
            form.notice = Some(why);
            let (dir, number) = (form.dir.clone(), form.number);
            if let Some(tx) = &app.pr_actions_tx {
                ask_read(tx, dir, number, ticket);
            }
        }
    }
}

/// Enter: the plan sent off the loop, the form held until it lands; or
/// why not, on the frame.
fn submit(app: &mut App) {
    let tx = app.pr_actions_tx.clone();
    let Some(form) = edit_form(app, None) else {
        return;
    };
    if form.saving.is_some() {
        return;
    }
    let plan = match form.plan() {
        Ok(plan) => plan,
        Err(why) => {
            form.notice = Some(why);
            return;
        }
    };
    let Some(tx) = tx else {
        return;
    };
    form.notice = None;
    form.saving = Some(format!("saving #{}: {}…", form.number, plan.summary()));
    let (project, dir, ticket, url) = (
        form.project.clone(),
        form.dir.clone(),
        form.ticket,
        form.url.clone(),
    );
    tokio::spawn(async move {
        let result = save(&dir, plan).await;
        let _ = tx.send(Answer::Edited {
            project,
            ticket,
            url,
            result,
        });
    });
}

// ---- keys, paste, mouse ----

pub(crate) fn hints(form: &EditForm) -> Vec<Hint> {
    let mut hints = Vec::new();
    match form.row {
        EditRow::Base => {
            hints.push(keys::CHOOSE.hint().kept());
            hints.push(keys::PICK.hint());
        }
        row if row.ticks() => {
            hints.push(keys::SAVE.hint().kept());
            hints.push(keys::TICK.hint());
            hints.push(keys::PICK.hint());
        }
        EditRow::Draft => {
            hints.push(keys::SAVE.hint().kept());
            hints.push(keys::DRAFT.hint());
        }
        EditRow::State => {
            hints.push(keys::SAVE.hint().kept());
            hints.push(keys::CHANGE.hint());
        }
        EditRow::Delete if form.deletable().is_some() => {
            hints.push(keys::SAVE.hint().kept());
            hints.push(keys::DELETE.hint());
        }
        EditRow::Comment => {
            hints.push(keys::SAVE.hint().kept());
            hints.push(crate::ui::task_keys::NEWLINE.hint());
        }
        _ => hints.push(keys::SAVE.hint().kept()),
    }
    hints.push(keys::FIELD.hint());
    hints.push(Hint::new("Esc", "cancel"));
    hints
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(form) = edit_form(app, None) else {
        return;
    };
    let row = form.row;
    let mut submit_now = false;
    match key.code {
        // Tab off a typed base takes its best match with it.
        _ if keys::FIELD.matches(&key) => {
            let forward = key.code == KeyCode::Tab
                && !key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::SHIFT);
            if row == EditRow::Base && form.typed {
                form.take_pick();
            }
            form.focus(form.step(forward));
        }
        KeyCode::Down | KeyCode::Up if row.picks() => {
            form.move_pick(if key.code == KeyCode::Down { 1 } else { -1 })
        }
        KeyCode::Enter if row == EditRow::Base => {
            form.take_pick();
            form.focus(form.step(true));
        }
        KeyCode::Char(' ') if row.ticks() => form.take_pick(),
        KeyCode::Char(' ') if row == EditRow::Draft => {
            form.draft = !form.draft;
            form.notice = None;
        }
        KeyCode::Char(' ') if row == EditRow::Delete => toggle_delete(form),
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if row == EditRow::State => {
            change_state(form, key.code != KeyCode::Left)
        }
        // ↑/↓ walk the comment's lines first; past its first line, and
        // from a one-line row, they move between the rows.
        KeyCode::Down | KeyCode::Up => {
            let down = key.code == KeyCode::Down;
            let consumed = form
                .input_mut()
                .is_some_and(|input| input.handle_key(&key).consumed());
            if !consumed && !(down && row == EditRow::Comment) {
                form.focus(form.step(down));
            }
        }
        KeyCode::Enter if !(row == EditRow::Comment && form.comment.takes_newline(&key)) => {
            submit_now = true
        }
        _ => {
            // The first letter typed into the base looks for another branch
            // rather than editing the one it holds.
            if row == EditRow::Base && !form.typed && super::is_char_key(&key) {
                form.base.clear();
            }
            if form
                .input_mut()
                .is_some_and(|input| input.handle_key(&key).changed())
            {
                form.notice = None;
                if row.picks() {
                    form.typed = true;
                    form.pick = 0;
                }
            }
        }
    }
    if submit_now {
        submit(app);
    }
}

/// The State row to its next choice (or the one before).
fn change_state(form: &mut EditForm, forward: bool) {
    form.state = super::step_round(form.state_choices(), form.state, forward);
    form.notice = None;
}

fn toggle_delete(form: &mut EditForm) {
    if form.deletable().is_some() && form.stays.is_none() {
        form.delete_branch = !form.delete_branch;
        form.notice = None;
    }
}

/// A paste into the text field under the caret — lines kept in the
/// comment, flattened anywhere else.
pub(crate) fn paste(form: &mut EditForm, text: &str) {
    let row = form.row;
    if row == EditRow::Base && !form.typed {
        form.base.clear();
    }
    if let Some(input) = form.input_mut() {
        input.insert_str(text);
        if row.picks() {
            form.typed = true;
            form.pick = 0;
        }
    }
}

/// The caret to `row`, as a click on it puts it.
pub(crate) fn focus(app: &mut App, row: EditRow) {
    if let Some(form) = edit_form(app, None) {
        form.focus(row);
    }
}

/// A click: on a row of the list, picks it as Space (or Enter) does; on a
/// row of the form, the caret there — a second on Draft, State or Delete
/// works it. The wheel walks a list.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, at: Position) {
    let Some(form) = edit_form(app, None) else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollDown if form.row.picks() => form.move_pick(1),
        MouseEventKind::ScrollUp if form.row.picks() => form.move_pick(-1),
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(&(_, pick)) = form.picks.iter().find(|(r, _)| r.contains(at)) {
                form.pick = pick;
                form.take_pick();
            } else if let Some(&(_, row)) = form.rows.iter().find(|(r, _)| r.contains(at)) {
                if form.row == row {
                    match row {
                        EditRow::Draft => form.draft = !form.draft,
                        EditRow::State => change_state(form, true),
                        EditRow::Delete => toggle_delete(form),
                        _ => {}
                    }
                } else {
                    form.focus(row);
                }
            }
        }
        _ => {}
    }
}

// ---- drawing ----

/// What the draw learned, for the live form (the draw works on a clone).
#[derive(Debug, Clone, Default)]
pub struct EditDrawn {
    rows: Vec<(Rect, EditRow)>,
    picks: Vec<(Rect, usize)>,
    comment_view: Option<TextView>,
}

pub(crate) fn write_back(form: &mut EditForm, drawn: EditDrawn) {
    form.rows = drawn.rows;
    form.picks = drawn.picks;
    if let Some(view) = drawn.comment_view {
        form.comment.set_view(view);
    }
}

/// A row's label, as wide as the widest.
fn label(row: EditRow) -> String {
    format!("{:<LABEL_W$}", row.label())
}

/// `a, b, c` — or a dim `—` with none.
fn names(list: &[String], th: Theme) -> Span<'static> {
    if list.is_empty() {
        Span::styled("—", Style::default().fg(th.dim))
    } else {
        Span::styled(list.join(", "), Style::default().fg(th.text))
    }
}

pub(crate) fn draw(
    f: &mut Frame,
    area: Rect,
    form: &EditForm,
    focused: bool,
    th: Theme,
) -> (u16, EditDrawn) {
    let (inner, foot_w) = form_frame(
        f,
        area,
        &format!("Edit #{}", form.number),
        form.notice.as_deref(),
        form.saving.as_deref(),
        focused,
        th,
    );
    let mut drawn = EditDrawn::default();
    let width = usize::from(inner.width);
    let dim = Style::default().fg(th.dim);
    let text = Style::default().fg(th.text);
    let on = |row: EditRow| focused && form.row == row && form.saving.is_none();
    let facts = &form.facts;
    let mut lines: Vec<(Option<EditRow>, Vec<Span<'static>>)> = Vec::new();
    lines.push((
        None,
        vec![Span::styled(
            format!("{INDENT}{}", facts.title),
            text.add_modifier(Modifier::BOLD),
        )],
    ));
    let state = if !form.read {
        "reading…".to_string()
    } else if facts.base_gone {
        format!(
            "{} · its base {} was deleted",
            facts.state.to_lowercase(),
            facts.base
        )
    } else {
        facts.state.to_lowercase()
    };
    lines.push((
        None,
        vec![Span::styled(
            format!("{INDENT}{} → {} · {state}", facts.head, facts.base),
            dim,
        )],
    ));
    lines.push((None, Vec::new()));

    // The rows: the one with the caret live, the rest as they stand, each
    // saying what it was when it has changed.
    let row_rect_at = |i: usize| row_rect(inner, i).unwrap_or_default();
    let was = |old: &str| -> Vec<Span<'static>> {
        if old.is_empty() {
            Vec::new()
        } else {
            vec![Span::styled(format!("  was {old}"), dim)]
        }
    };
    for row in form.order() {
        let i = lines.len();
        let spans = match row {
            EditRow::Comment => continue,
            EditRow::Title => form_field(
                &label(row),
                &form.title,
                "(required)",
                on(row),
                row_rect_at(i),
                th,
            ),
            EditRow::Base => {
                let base = form.base.trim();
                let mut s = form_field(&label(row), &form.base, "", on(row), row_rect_at(i), th);
                if on(row) {
                    // The list under it says the rest.
                } else if base != facts.base {
                    s.extend(was(&facts.base));
                } else if form.read && facts.base_gone {
                    s.push(Span::styled(
                        "  deleted on GitHub",
                        Style::default().fg(th.err),
                    ));
                }
                s
            }
            EditRow::Draft => vec![
                form_label(&label(row), on(row), th),
                Span::styled(check(form.draft), text),
                Span::styled(
                    if form.draft {
                        "  a draft"
                    } else {
                        "  ready for review"
                    },
                    dim,
                ),
            ],
            // The caret in its filter, and — nothing typed yet — what it
            // has ticked after it.
            row if row.ticks() && on(row) => {
                let mut s = form_field(&label(row), &form.filter, "", true, row_rect_at(i), th);
                let ticked = form.ticked().cloned().unwrap_or_default();
                if form.filter.is_empty() {
                    let say = if ticked.is_empty() {
                        "type to narrow · Space ticks".to_string()
                    } else {
                        ticked.join(", ")
                    };
                    s.push(Span::styled(format!(" {say}"), dim));
                }
                s
            }
            EditRow::Labels => vec![form_label(&label(row), false, th), names(&form.labels, th)],
            EditRow::Reviewers => vec![
                form_label(&label(row), false, th),
                names(&form.reviewers, th),
            ],
            EditRow::Assignees => vec![
                form_label(&label(row), false, th),
                names(&form.assignees, th),
            ],
            EditRow::State => {
                let several = form.state_choices().len() > 1;
                let mut s = vec![
                    form_label(&label(row), on(row), th),
                    Span::styled(
                        if several { "◂ " } else { "  " },
                        Style::default().fg(th.muted),
                    ),
                    Span::styled(
                        form.state_word(form.state),
                        text.add_modifier(Modifier::BOLD),
                    ),
                ];
                if several {
                    s.push(Span::styled(" ▸", Style::default().fg(th.muted)));
                }
                s
            }
            EditRow::Delete => {
                let note = match (&form.stays, form.deletable()) {
                    (Some(why), _) => stays_note(why),
                    (_, None) if !form.read => "  known once its details are in".to_string(),
                    (_, None) => "  not a branch of this repo — it stays".to_string(),
                    (_, Some(branch)) if form.delete_branch => {
                        format!("  {branch} on GitHub, once closed")
                    }
                    (_, Some(_)) => "  the branch stays on GitHub".to_string(),
                };
                vec![
                    form_label(&label(row), on(row), th),
                    Span::styled(check(form.delete_branch && form.stays.is_none()), text),
                    Span::styled(note, dim),
                ]
            }
        };
        lines.push((Some(row), spans));
    }
    // Reopening a pull request whose branches are gone: what is put back.
    if form.state == StateChange::Reopen {
        for note in restore_notes(form) {
            lines.push((
                None,
                vec![Span::styled(
                    format!("{INDENT}{:LABEL_W$}{note}", ""),
                    Style::default().fg(th.warn),
                )],
            ));
        }
    }
    lines.push((None, Vec::new()));
    let saves = match form.plan() {
        Ok(plan) => Span::styled(plan.summary(), Style::default().fg(th.accent)),
        Err(why) => Span::styled(why, dim),
    };
    lines.push((
        None,
        vec![
            form_label(&format!("{:<LABEL_W$}", "Saves"), false, th),
            saves,
        ],
    ));
    let used = lines.len() as u16;
    for (i, (row, spans)) in lines.into_iter().enumerate() {
        let Some(rect) = row_rect(inner, i) else {
            break;
        };
        f.render_widget(Paragraph::new(crate::pr_preview::fit(spans, width)), rect);
        if let Some(row) = row {
            drawn.rows.push((rect, row));
        }
    }

    // Under the rows: the list while a pick row has the caret, else the
    // comment while closing or reopening.
    let lower = Rect {
        y: inner.y.saturating_add(used + 1),
        height: inner.height.saturating_sub(used + 1),
        ..inner
    };
    if lower.height < 3 || lower.width < 4 {
        return (foot_w, drawn);
    }
    if form.row.picks() && on(form.row) {
        let title = match form.row {
            EditRow::Base => "Branches on origin",
            EditRow::Labels => "The repo's labels",
            _ => "People on the repo",
        };
        let list = form_box(f, lower, title, true, th);
        draw_picks(f, form, list, &mut drawn, th);
    } else if form.state != StateChange::Keep {
        drawn.rows.push((lower, EditRow::Comment));
        let empty = match form.state {
            StateChange::Reopen => "(optional — left on the pull request as it reopens)",
            _ => "(optional — left on the pull request as it closes)",
        };
        drawn.comment_view = form_text_box(
            f,
            lower,
            "Comment",
            &form.comment,
            on(EditRow::Comment),
            empty,
            th,
        );
    }
    (foot_w, drawn)
}

/// What reopening puts back first, a line each.
fn restore_notes(form: &EditForm) -> Vec<String> {
    let f = &form.facts;
    let short = |oid: &str| oid.chars().take(7).collect::<String>();
    let mut notes = Vec::new();
    if f.base_gone {
        let then = if form.base.trim() != f.base {
            format!(
                ", moves #{} onto {}, then deletes it again",
                form.number,
                form.base.trim()
            )
        } else {
            " and keeps it".to_string()
        };
        notes.push(format!(
            "{} was deleted: orion puts it back at {} to reopen on{then}",
            f.base,
            short(&f.base_oid)
        ));
    }
    if f.head_gone && !f.fork {
        notes.push(format!(
            "{} was deleted: orion puts it back at {} first",
            f.head,
            short(&f.head_oid)
        ));
    }
    notes
}

/// The list under a pick row: each option — ticked ones `[x]` on a ticking
/// row, a label in its colour — the cursor's kept in view.
fn draw_picks(f: &mut Frame, form: &EditForm, area: Rect, drawn: &mut EditDrawn, th: Theme) {
    let say = |f: &mut Frame, text: &str| crate::ui::empty_list_row(f, area, text, th);
    let loaded = match form.row {
        EditRow::Base => form.branches.is_some(),
        _ => form.read,
    };
    if !loaded {
        say(f, "reading…");
        return;
    }
    let choices = form.choices();
    if choices.is_empty() {
        say(f, "nothing matches");
        return;
    }
    let height = usize::from(area.height.max(1));
    let pick = form.pick.min(choices.len() - 1);
    let top = crate::app::window_start(pick, height);
    let ticked = form.ticked();
    let budget = usize::from(area.width).saturating_sub(6);
    for (row, (name, positions)) in choices.iter().enumerate().skip(top).take(height) {
        let Some(rect) = row_rect(area, row - top) else {
            break;
        };
        let mut spans = Vec::new();
        if let Some(ticked) = ticked {
            spans.push(Span::styled(
                format!("{} ", check(ticked.contains(name))),
                Style::default().fg(th.text),
            ));
        }
        if form.row == EditRow::Labels {
            let color = form
                .offers
                .labels
                .iter()
                .find(|l| l.name == *name)
                .and_then(|l| crate::theme::hex(&l.color))
                .unwrap_or(th.muted);
            spans.push(Span::styled("● ", Style::default().fg(color)));
        }
        let shown = truncate(name, budget);
        let positions = crate::ui::visible_positions(positions, &shown, name);
        spans.extend(fuzzy_highlight_styled(
            &shown,
            positions,
            Style::default().fg(th.text),
            th,
        ));
        render_row(f, rect, spans, row == pick, true, th);
        drawn.picks.push((rect, row));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::with_gh;
    use super::*;

    fn facts() -> Facts {
        Facts {
            title: "Fix login".into(),
            state: STATE_OPEN.into(),
            base: "main".into(),
            base_oid: "abc1234def".into(),
            head: "fix-login".into(),
            head_oid: "fed4321cba".into(),
            author: "oliver".into(),
            labels: vec!["bug".into()],
            reviewers: vec!["sam".into()],
            ..Facts::default()
        }
    }

    fn form_on(facts: Facts) -> EditForm {
        EditForm {
            project: ProjectId("p".into()),
            dir: PathBuf::from("/x"),
            number: 42,
            url: "https://github.com/o/r/pull/42".into(),
            title: TextInput::with_text(facts.title.clone()),
            base: TextInput::with_text(facts.base.clone()),
            draft: facts.draft,
            labels: facts.labels.clone(),
            reviewers: facts.reviewers.clone(),
            assignees: facts.assignees.clone(),
            facts,
            read: true,
            state: StateChange::Keep,
            delete_branch: false,
            stays: None,
            comment: TextInput::multiline(),
            row: EditRow::Title,
            filter: TextInput::new(),
            typed: false,
            pick: 0,
            branches: Some(["main", "dev", "fix-login"].map(String::from).into()),
            offers: Offers {
                labels: vec![
                    PrLabel {
                        name: "bug".into(),
                        color: "d73a4a".into(),
                    },
                    PrLabel {
                        name: "pdf".into(),
                        color: "0075ca".into(),
                    },
                ],
                people: vec!["oliver".into(), "sam".into(), "ada".into()],
            },
            ticket: 7,
            saving: None,
            notice: None,
            rows: Vec::new(),
            picks: Vec::new(),
        }
    }

    /// Only what changed is sent, each list as what it gains and loses.
    #[test]
    fn the_plan_sends_only_what_changed() {
        let mut form = form_on(facts());
        assert_eq!(form.plan(), Err("nothing to change".into()));
        form.title.set_text("Fix the login");
        form.base.set_text("dev");
        form.labels = vec!["pdf".into()];
        form.reviewers.push("ada".into());
        form.draft = true;
        let plan = form.plan().expect("a plan");
        assert_eq!(
            plan.edit,
            [
                "--title",
                "Fix the login",
                "--base",
                "dev",
                "--add-label",
                "pdf",
                "--remove-label",
                "bug",
                "--add-reviewer",
                "ada"
            ]
        );
        assert_eq!(plan.ready, Some(false));
        assert_eq!(
            plan.summary(),
            "title · base → dev · +pdf · −bug · review +ada · back to draft"
        );
        form.read = false;
        assert_eq!(
            form.plan(),
            Err("reading #42 — try again in a moment".into())
        );
    }

    /// A closed pull request keeps its base unless it is reopened; the
    /// author is never offered as a reviewer.
    #[test]
    fn a_closed_pull_request_moves_only_once_reopened() {
        let mut form = form_on(Facts {
            state: STATE_CLOSED.into(),
            base_gone: true,
            ..facts()
        });
        form.base.set_text("main2");
        assert!(form.plan().unwrap_err().contains("reopen it to move it"));
        form.state = StateChange::Reopen;
        let plan = form.plan().expect("a plan");
        assert_eq!(
            plan.restore_base,
            Some(("main".to_string(), "abc1234def".to_string()))
        );
        assert_eq!(plan.base.as_deref(), Some("main2"));
        assert_eq!(
            form.state_choices(),
            [StateChange::Keep, StateChange::Reopen]
        );
        form.row = EditRow::Reviewers;
        let offered: Vec<String> = form.choices().into_iter().map(|(n, _)| n).collect();
        assert_eq!(offered, ["sam", "ada"], "not the author");
    }

    /// GitHub's word lands on the fields still holding what the row knew,
    /// and never over what was changed meanwhile.
    #[test]
    fn what_github_says_fills_only_the_untouched_fields() {
        let mut form = form_on(Facts {
            reviewers: Vec::new(),
            ..facts()
        });
        form.read = false;
        form.reviewers.clear();
        form.title.set_text("Mine now");
        form.adopt(Facts {
            title: "GitHub's".into(),
            draft: true,
            ..facts()
        });
        assert_eq!(form.title.as_str(), "Mine now");
        assert!(form.draft);
        assert_eq!(form.reviewers, ["sam"]);
        assert!(form.read);
    }

    /// The pick rows: Space ticks and unticks on a ticking row; typing
    /// narrows the list; Enter on the base takes the branch.
    #[test]
    fn the_pick_rows_tick_and_take() {
        let mut form = form_on(facts());
        form.focus(EditRow::Labels);
        form.pick = 1;
        form.take_pick();
        assert_eq!(form.labels, ["bug", "pdf"]);
        form.take_pick();
        assert_eq!(form.labels, ["bug"]);
        form.focus(EditRow::Base);
        assert_eq!(form.pick, 0, "on main, its own");
        form.base.set_text("de");
        form.typed = true;
        form.pick = 0;
        form.take_pick();
        assert_eq!(form.base.as_str(), "dev");
        assert!(
            !form.choices().iter().any(|(n, _)| n == "fix-login"),
            "never its own head"
        );
    }

    /// GitHub's answer to the form's one ask.
    #[test]
    fn the_read_parses() {
        let json = r#"{"data":{"repository":{
            "pullRequest":{"title":"T","state":"CLOSED","isDraft":false,"baseRefName":"old",
              "baseRefOid":"abc","headRefName":"feat","headRefOid":"def","isCrossRepository":false,
              "author":{"login":"oliver"},"baseRef":null,"headRef":{"name":"feat"},
              "labels":{"nodes":[{"name":"bug"}]},
              "reviewRequests":{"nodes":[{"requestedReviewer":{"login":"sam"}},{"requestedReviewer":{}}]},
              "assignees":{"nodes":[{"login":"ada"}]}},
            "labels":{"nodes":[{"name":"bug","color":"d73a4a"}]},
            "assignableUsers":{"nodes":[{"login":"ada"},{"login":"sam"}]}}}}"#;
        let (facts, offers) = parse_read(json).expect("parses");
        assert!(facts.base_gone && !facts.head_gone);
        assert_eq!(facts.reviewers, ["sam"], "a team request is left out");
        assert_eq!(facts.assignees, ["ada"]);
        assert_eq!(offers.people, ["ada", "sam"]);
        assert_eq!(offers.labels[0].color, "d73a4a");
    }

    /// Reopening onto a deleted base: the base is put back at the commit it
    /// last pointed at, the pull request reopened and moved onto a live
    /// branch, and the base deleted again — in that order.
    #[test]
    fn reopening_onto_a_deleted_base_puts_it_back_then_takes_it_away() {
        let cases = r#"  "api -X POST repos/{owner}/{repo}/git/refs"*) echo '{}' ;;
  "pr reopen 42"*) ;;
  "pr edit 42 "*) ;;
  "pr list --base old"*) echo '[]' ;;
  "api -X DELETE repos/{owner}/{repo}/git/refs/heads/old") ;;"#;
        let plan = Plan {
            number: 42,
            edit: vec!["--base".into(), "main".into()],
            base: Some("main".into()),
            state: StateChange::Reopen,
            comment: "back on main".into(),
            restore_base: Some(("old".into(), "abc123".into())),
            said: vec!["base → main".into()],
            ..Plan::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let (said, calls) = with_gh(cases, dir.path(), async |dir| save(dir, plan).await);
        assert_eq!(
            said.as_deref(),
            Ok("put old back; reopened #42; #42: base → main; deleted old again")
        );
        assert_eq!(
            calls,
            [
                "api -X POST repos/{owner}/{repo}/git/refs -f ref=refs/heads/old -f sha=abc123",
                "pr reopen 42 --comment back on main",
                "pr edit 42 --base main",
                "pr list --base old --state open --limit 100 --json number",
                "api -X DELETE repos/{owner}/{repo}/git/refs/heads/old",
            ]
        );
    }

    /// A reopen GitHub refuses takes the base it put back away again, and
    /// moves nothing.
    #[test]
    fn a_refused_reopen_takes_the_base_back() {
        let cases = r#"  "api -X POST"*) echo '{}' ;;
  "pr reopen 42"*) echo 'head was force-pushed' >&2; exit 1 ;;
  "pr list --base old"*) echo '[]' ;;
  "api -X DELETE"*) ;;"#;
        let plan = Plan {
            number: 42,
            edit: vec!["--base".into(), "main".into()],
            base: Some("main".into()),
            state: StateChange::Reopen,
            restore_base: Some(("old".into(), "abc123".into())),
            ..Plan::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let (said, calls) = with_gh(cases, dir.path(), async |dir| save(dir, plan).await);
        assert_eq!(
            said,
            Err("couldn't reopen #42: head was force-pushed".into())
        );
        assert!(!calls.iter().any(|c| c.starts_with("pr edit")), "{calls:?}");
        assert!(
            calls.last().is_some_and(|c| c.contains("DELETE")),
            "{calls:?}"
        );
    }

    /// Closing: the edits first, while it is open, then the close with its
    /// comment, then its branch — never before the close.
    #[test]
    fn closing_saves_the_edits_then_closes_then_deletes_the_branch() {
        let cases = r#"  "pr edit 42 "*) ;;
  "pr close 42"*) ;;
  "repo view"*) echo main ;;
  "api repos/{owner}/{repo}/branches/"*) echo false ;;
  "pr list --base fix-login"*) echo '[]' ;;
  "api -X DELETE"*) ;;"#;
        let mut form = form_on(facts());
        form.title.set_text("Fix login, abandoned");
        form.state = StateChange::Close;
        form.delete_branch = true;
        form.comment.set_text("superseded by #43");
        let plan = form.plan().expect("a plan");
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.json");
        let (said, calls) = crate::config::with_config_path(cfg, || {
            with_gh(cases, dir.path(), async |dir| save(dir, plan).await)
        });
        assert_eq!(
            said.as_deref(),
            Ok("#42: title; closed #42, branch deleted")
        );
        let at = |want: &str| calls.iter().position(|c| c.starts_with(want));
        assert!(at("pr edit 42 --title") < at("pr close 42 --comment superseded by #43"));
        assert!(at("pr close 42") < at("api -X DELETE"), "{calls:?}");
    }

    #[test]
    fn the_hints_come_from_the_table() {
        let mut form = form_on(facts());
        form.state = StateChange::Close;
        for row in form.order() {
            form.row = row;
            crate::hints::assert_hints_from(&hints(&form), keys::ALL);
        }
    }
}
