//! The PULL REQUESTS MODAL's two forms, each drawn in the reading pane's
//! place while the list stays up on the left — the ISSUES MODAL's editor,
//! the same way round:
//!
//! * **New pull request** (`Ctrl+t`): the branch it comes from — the one
//!   the Worktrees cursor's checkout is on, else the ROOT WORKTREE's — the
//!   branch it merges into — the project's base, in the order the COMMIT
//!   LIST measures a branch against (`commit_list::resolve_base`) — a
//!   title, a draft box and a description. A branch field lists the
//!   project's branches in the description's place while it has the caret
//!   (local ones for From, origin's for Into), narrowed as it is typed in.
//!   The title and description are filled from the commits between the
//!   two the way `gh pr create --fill` fills them — one commit's subject
//!   and body, or the branch's name and a list of subjects — and follow a
//!   new pair of branches until they are typed over. Enter pushes the
//!   branch to `origin` (setting its upstream when it has none) and runs
//!   `gh pr create`; the list is asked again, and the modal's cursor
//!   follows the new pull request onto its row.
//! * **Merge** (`Ctrl+x`): the pull request under the cursor, how — squash,
//!   a merge commit or a rebase, only those the repo allows (`gh repo
//!   view`) — whether its branch goes with it, and whether GitHub should
//!   wait for its checks and reviews (auto-merge). What GitHub said that
//!   stands in the way is spelled out first: a draft, conflicts, failing
//!   or running checks, a review still owed. Enter runs `gh pr merge`.
//!   The **Review** SETTINGS tab holds the defaults both open on.
//!
//! Every git and `gh` here runs off the loop — a key handler never blocks
//! — and the answers land in [`land_answer`] through `App::pr_actions_tx`,
//! matched to the form by ticket, so an answer for a form since closed is
//! only flashed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::ProjectId;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Overlay};
use crate::hints::Hint;
use crate::pr_modal::PullRequestsView;
use crate::pull_request::{run_piped, Checks, OpenPr, PrDetail};
use crate::text_input::{TextInput, TextView};
use crate::theme::Theme;
use crate::ui::{
    form_box, form_field, form_frame, form_label, form_text_box, fuzzy_highlight_styled,
    render_row, row_rect, truncate,
};

/// How long a push may run: a pre-push hook can run a test suite, and
/// the person who pressed Enter is watching the form say so.
const PUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// Everything else the forms send is one request — `gh pr create`, `gh pr
/// merge`, the branch's delete — or one question git answers locally.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// The rows the create form's one-line fields take, over the box under
/// them — From, Into, Title, Draft.
const FIELD_ROWS: u16 = 4;
/// How many commits a fill lists by subject: a pair of branches far
/// apart reads a page of them, not megabytes.
const FILL_SUBJECTS: usize = 50;
use crate::pr_preview::INDENT;

/// A form up in the PULL REQUESTS MODAL's reading pane — kept boxed on
/// the view (`PullRequestsView::form`), so the larger variant costs the
/// modal nothing while no form is up.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrForm {
    Create(CreateForm),
    Merge(MergeForm),
}

impl PrForm {
    /// The form's own ticket: an answer carrying another is for a form
    /// since closed.
    fn ticket(&self) -> u64 {
        match self {
            PrForm::Create(f) => f.ticket,
            PrForm::Merge(f) => f.ticket,
        }
    }

    /// Enter sent it and the answer is not in: it takes no keys but Esc.
    fn saving(&self) -> bool {
        match self {
            PrForm::Create(f) => f.saving.is_some(),
            PrForm::Merge(f) => f.saving.is_some(),
        }
    }

    /// GitHub (or git) refused what it sent: it says why on its frame and
    /// takes keys again, everything typed still in it.
    fn refused(&mut self, why: String) {
        let (saving, notice) = match self {
            PrForm::Create(f) => (&mut f.saving, &mut f.notice),
            PrForm::Merge(f) => (&mut f.saving, &mut f.notice),
        };
        *saving = None;
        *notice = Some(why);
    }
}

// ---- the new pull request ----

/// The create form's fields, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateField {
    From,
    Into,
    Title,
    Draft,
    Body,
}

impl CreateField {
    const ORDER: [CreateField; 5] = [
        CreateField::From,
        CreateField::Into,
        CreateField::Title,
        CreateField::Draft,
        CreateField::Body,
    ];

    /// The next field (or the one before), round either end.
    pub fn step(self, forward: bool) -> Self {
        let n = Self::ORDER.len();
        let at = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        }]
    }

    /// A branch field: it lists branches to pick from while it has the
    /// caret.
    pub fn is_branch(self) -> bool {
        matches!(self, CreateField::From | CreateField::Into)
    }
}

/// `Ctrl+t`: a new pull request, filled in before it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateForm {
    pub project: ProjectId,
    /// Where git and `gh` run: the project's main checkout.
    pub dir: PathBuf,
    /// The branch it comes from (`--head`) and the one it merges into
    /// (`--base`).
    pub from: TextInput,
    pub into: TextInput,
    pub title: TextInput,
    pub body: TextInput,
    pub draft: bool,
    pub field: CreateField,
    /// The branches From and Into pick from — the local ones, newest
    /// first, and origin's — None until they are read.
    pub heads: Option<Arc<[String]>>,
    pub bases: Option<Arc<[String]>>,
    /// The branch field with the caret was typed in since the caret came
    /// to it: its list narrows to the text then, and shows every branch,
    /// the cursor on the field's own, until then.
    pub typed: bool,
    /// The cursor in that list, among the rows it shows.
    pub pick: usize,
    /// What the fill last wrote into the title and description, and for
    /// which pair of branches: a field still holding what was filled
    /// follows a new pair; one typed over is the reader's.
    pub filled: (String, String),
    pub filled_for: Option<(String, String)>,
    /// How many commits From has that Into does not, as of the last fill.
    pub ahead: Option<usize>,
    /// This form's own ticket: an answer carrying another is for a form
    /// since closed.
    pub ticket: u64,
    /// What is in flight once Enter sent it — the push, then the create.
    pub saving: Option<String>,
    /// Why the last Enter went nowhere, shown on the frame until the next
    /// edit.
    pub notice: Option<String>,
    /// As of the last draw, for the mouse: each field's row, and the
    /// branch list's rows by index into what it shows.
    pub rows: Vec<(Rect, CreateField)>,
    pub picks: Vec<(Rect, usize)>,
}

impl CreateForm {
    fn new(project: ProjectId, dir: PathBuf, from: String, draft: bool) -> Self {
        Self {
            project,
            dir,
            from: TextInput::with_text(from),
            into: TextInput::new(),
            title: TextInput::new(),
            body: TextInput::multiline(),
            draft,
            field: CreateField::Title,
            heads: None,
            bases: None,
            typed: false,
            pick: 0,
            filled: (String::new(), String::new()),
            filled_for: None,
            ahead: None,
            ticket: crate::view_jobs::ticket(),
            saving: None,
            notice: None,
            rows: Vec::new(),
            picks: Vec::new(),
        }
    }

    /// The text field under the caret; None on the draft box.
    fn input_mut(&mut self) -> Option<&mut TextInput> {
        match self.field {
            CreateField::From => Some(&mut self.from),
            CreateField::Into => Some(&mut self.into),
            CreateField::Title => Some(&mut self.title),
            CreateField::Body => Some(&mut self.body),
            CreateField::Draft => None,
        }
    }

    /// The branches the field under the caret picks from.
    fn branches(&self) -> &[String] {
        let list = match self.field {
            CreateField::From => &self.heads,
            CreateField::Into => &self.bases,
            _ => return &[],
        };
        list.as_deref().unwrap_or_default()
    }

    /// The branch field's text, trimmed.
    fn branch_text(&self) -> &str {
        match self.field {
            CreateField::From => self.from.trim(),
            CreateField::Into => self.into.trim(),
            _ => "",
        }
    }

    /// The rows the branch list shows: every branch until the field is
    /// typed in, then the fuzzy matches of its text, best first — each an
    /// index into [`CreateForm::branches`] with the chars matched.
    pub fn choices(&self) -> Vec<(usize, Vec<usize>)> {
        let branches = self.branches();
        if !self.typed || self.branch_text().is_empty() {
            return (0..branches.len()).map(|i| (i, Vec::new())).collect();
        }
        crate::fuzzy::rank(self.branch_text(), branches.iter().map(String::as_str))
    }

    /// The caret onto `field`. A branch field's list opens on the branch
    /// it holds.
    fn focus(&mut self, field: CreateField) {
        self.field = field;
        self.typed = false;
        let current = self.branch_text().to_string();
        self.pick = self
            .branches()
            .iter()
            .position(|b| *b == current)
            .unwrap_or(0);
    }

    /// The branch under the list's cursor into its field. Whether one was
    /// there to take.
    fn take_pick(&mut self) -> bool {
        let Some((index, _)) = self.choices().get(self.pick).cloned() else {
            return false;
        };
        let Some(branch) = self.branches().get(index).cloned() else {
            return false;
        };
        match self.field {
            CreateField::From => self.from.set_text(branch),
            CreateField::Into => self.into.set_text(branch),
            _ => return false,
        }
        self.typed = false;
        true
    }

    /// The branch field with the caret was typed in: its list narrows to
    /// the text, the cursor on the best match.
    fn typed_in(&mut self) {
        self.typed = true;
        self.pick = 0;
    }

    /// The branch list's cursor `delta` rows on, held to the list.
    fn move_pick(&mut self, delta: i64) {
        let len = self.choices().len();
        self.pick = crate::app::clamp_selection(self.pick as i64 + delta, len);
    }

    /// The branch under the list's cursor into its field, and the caret on
    /// to the next one.
    fn choose_pick(&mut self) {
        self.take_pick();
        self.focus(self.field.step(true));
    }

    /// The pair of branches as typed, when both are there.
    fn pair(&self) -> Option<(String, String)> {
        let (from, into) = (self.from.trim(), self.into.trim());
        (!from.is_empty() && !into.is_empty()).then(|| (from.to_string(), into.to_string()))
    }
}

/// Where the Worktrees cursor's checkout is, when it is one of
/// `project`'s: the branch a new pull request comes from. Else the ROOT
/// WORKTREE's.
fn default_head(app: &App, project: &ProjectId) -> String {
    app.selected_worktree()
        .filter(|w| &w.project_id == project && !w.branch.is_empty())
        .or_else(|| {
            app.tree
                .worktrees
                .iter()
                .find(|w| &w.project_id == project && w.is_main)
        })
        .map(|w| w.branch.clone())
        .unwrap_or_default()
}

/// `Ctrl+t`: the create form for the modal's project, the caret on the
/// title, the branches and the fill read underneath.
pub(crate) fn open_create(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    if !dir.is_dir() {
        app.flash = Some(format!("repo path missing on disk: {}", dir.display()));
        return;
    }
    let from = default_head(app, &project);
    let config = crate::config::Config::load();
    let mut form = CreateForm::new(project, dir.clone(), from, config.pr_draft);
    if form.from.is_empty() {
        form.field = CreateField::From;
    }
    let ticket = form.ticket;
    put_form(app, PrForm::Create(form));
    if let Some(tx) = app.pr_actions_tx.clone() {
        let base_setting = config.worktree_base_branch.clone();
        tokio::task::spawn_blocking(move || {
            let (heads, bases, base) = read_branches(&dir, &base_setting);
            let _ = tx.send(Answer::Branches {
                ticket,
                heads,
                bases,
                base,
            });
        });
    }
    app.dirty = true;
}

/// The project's local branches, newest first; origin's, newest first
/// (`origin/HEAD` left out); and the branch a pull request merges into
/// by default, by name on origin.
fn read_branches(dir: &Path, base_setting: &str) -> (Vec<String>, Vec<String>, Option<String>) {
    let refs = |pattern: &str, format: &str| -> Vec<String> {
        let out = crate::git_diff::run_git(
            dir,
            &[
                "for-each-ref",
                "--sort=-committerdate",
                &format!("--format={format}"),
                pattern,
            ],
        );
        match out {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|name| !name.is_empty() && *name != "HEAD")
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    };
    let heads = refs("refs/heads", "%(refname:short)");
    let bases = refs("refs/remotes/origin", "%(refname:lstrip=3)");
    let base = crate::commit_list::resolve_base(dir, base_setting).map(|base| {
        base.strip_prefix("origin/")
            .map(str::to_string)
            .unwrap_or(base)
    });
    (heads, bases, base)
}

/// Ask for the fill of the form's pair of branches, once per pair.
fn request_fill(app: &mut App) {
    let tx = app.pr_actions_tx.clone();
    let Some(form) = create_form(app) else {
        return;
    };
    let Some(pair) = form.pair() else {
        return;
    };
    if form.filled_for.as_ref() == Some(&pair) {
        return;
    }
    form.filled_for = Some(pair.clone());
    let (dir, ticket) = (form.dir.clone(), form.ticket);
    let Some(tx) = tx else {
        return;
    };
    tokio::task::spawn_blocking(move || {
        let fill = read_commits(&dir, &pair.0, &pair.1);
        let _ = tx.send(Answer::Fill { ticket, pair, fill });
    });
}

/// What `from` has that `into` does not: how many commits, merges and
/// all — what the pull request would carry — and the newest
/// [`FILL_SUBJECTS`] that are not merges, oldest first, for the fill: each
/// its subject, and its body only when it is the one commit, whose whole
/// message is the description. `into` is origin's when origin has it.
/// None when git cannot say — a branch it does not know.
fn read_commits(dir: &Path, from: &str, into: &str) -> Option<Fill> {
    let base = crate::commit_list::branch_ref(dir, into).unwrap_or_else(|| into.to_string());
    let range = format!("{base}..{from}");
    let count = crate::git_diff::run_git(dir, &["rev-list", "--count", &range]).ok()?;
    if !count.status.success() {
        return None;
    }
    let ahead = String::from_utf8_lossy(&count.stdout).trim().parse().ok()?;
    let format = if ahead == 1 {
        "--format=%s%x1f%b%x1e"
    } else {
        "--format=%s%x1f%x1e"
    };
    let max = format!("--max-count={FILL_SUBJECTS}");
    let args = [
        "log",
        "--reverse",
        "--no-merges",
        format,
        &max,
        &range,
        "--",
    ];
    let out = crate::git_diff::run_git(dir, &args).ok()?;
    if !out.status.success() {
        return None;
    }
    Some(Fill {
        ahead,
        commits: parse_commits(&String::from_utf8_lossy(&out.stdout)),
    })
}

/// What a fill is made of ([`read_commits`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fill {
    /// Commits From has that Into does not, merges and all.
    pub ahead: usize,
    /// The newest of them that are not merges, oldest first: subject and
    /// body.
    pub commits: Vec<(String, String)>,
}

/// `%s%x1f%b%x1e` records: subject, then body.
fn parse_commits(text: &str) -> Vec<(String, String)> {
    text.split('\x1e')
        .filter_map(|record| {
            let (subject, body) = record.split_once('\x1f')?;
            let subject = subject.trim().to_string();
            (!subject.is_empty()).then(|| (subject, body.trim().to_string()))
        })
        .collect()
}

/// The title and description `gh pr create --fill` would write: one
/// commit's subject and body; for several, the branch's name made words
/// and a list of their subjects — ending `- …` when there were more than
/// [`FILL_SUBJECTS`] to list.
pub fn fill_text(from: &str, commits: &[(String, String)]) -> (String, String) {
    match commits {
        [] => (words_of(from), String::new()),
        [(subject, body)] => (subject.clone(), body.clone()),
        many => {
            let mut lines: Vec<String> = many.iter().map(|(s, _)| format!("- {s}")).collect();
            if many.len() >= FILL_SUBJECTS {
                lines.push("- …".into());
            }
            (words_of(from), lines.join("\n"))
        }
    }
}

/// `fix/login-redirect` → `Login redirect`: the last part of the branch's
/// name, its dashes and underscores spaces, the first letter a capital.
fn words_of(branch: &str) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch);
    let words = last.replace(['-', '_'], " ");
    let mut chars = words.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Enter on the create form: what is missing is said on the spot, and a
/// whole form goes to git and `gh` off the loop, the form held until the
/// answer lands.
fn submit_create(app: &mut App) {
    let tx = app.pr_actions_tx.clone();
    let open = |app: &App, from: &str| -> Option<u64> {
        let Some(Overlay::PullRequests(view)) = &app.overlay else {
            return None;
        };
        app.open_prs
            .get(&view.project)?
            .list
            .iter()
            .find(|pr| pr.head == from)
            .map(|pr| pr.number)
    };
    let (from, into) = match create_form(app) {
        Some(form) => (form.from.trim().to_string(), form.into.trim().to_string()),
        None => return,
    };
    let already = open(app, &from);
    let Some(form) = create_form(app) else {
        return;
    };
    if form.saving.is_some() {
        return;
    }
    let title = form.title.trim().to_string();
    form.notice = if from.is_empty() {
        Some("which branch is it from?".into())
    } else if into.is_empty() {
        Some("which branch does it merge into?".into())
    } else if from == into {
        Some("a branch can't merge into itself".into())
    } else if title.is_empty() {
        Some("the pull request needs a title".into())
    } else if let Some(number) = already {
        Some(format!("#{number} is already open from {from}"))
    } else if form.ahead == Some(0) {
        Some(format!("{from} has no commits that {into} doesn't"))
    } else {
        None
    };
    if form.notice.is_some() {
        return;
    }
    let Some(tx) = tx else {
        return;
    };
    form.saving = Some(format!("pushing {from} and opening the pull request…"));
    let (project, dir, ticket) = (form.project.clone(), form.dir.clone(), form.ticket);
    let (body, draft) = (form.body.as_str().to_string(), form.draft);
    tokio::spawn(async move {
        let result = create(&dir, &from, &into, &title, &body, draft).await;
        let _ = tx.send(Answer::Created {
            project,
            ticket,
            result,
        });
    });
}

/// Push `from` to origin and open the pull request: its URL, or why not.
async fn create(
    dir: &Path,
    from: &str,
    into: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<String, String> {
    push_branch(dir, from).await?;
    let mut args = vec![
        "pr",
        "create",
        "--head",
        from,
        "--base",
        into,
        "--title",
        title,
        "--body-file",
        "-",
    ];
    if draft {
        args.push("--draft");
    }
    let out = run_piped(gh(dir, &args), body, REQUEST_TIMEOUT).await?;
    // `gh` prints the new pull request's URL last.
    Ok(out
        .lines()
        .map(str::trim)
        .rfind(|line| line.starts_with("http"))
        .unwrap_or_default()
        .to_string())
}

/// `git push` of the local branch `branch` to the branch of that name on
/// origin, its upstream set to it when it tracks nothing yet. A branch
/// already there and up to date pushes nothing.
async fn push_branch(dir: &Path, branch: &str) -> Result<(), String> {
    let upstream = format!("{branch}@{{upstream}}");
    let ask = [
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        upstream.as_str(),
    ];
    let tracks = run_piped(git(dir, &ask), "", REQUEST_TIMEOUT).await.is_ok();
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let mut args = vec!["push", "--quiet"];
    if !tracks {
        args.push("--set-upstream");
    }
    args.extend(["origin", refspec.as_str()]);
    run_piped(git(dir, &args), "", PUSH_TIMEOUT)
        .await
        .map(|_| ())
}

// ---- the merge ----

/// How a merge lands on the base, in the order the form cycles them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    pub const ALL: [MergeMethod; 3] =
        [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase];

    /// The word the **Merge method** SETTING stores.
    pub const fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Squash => "squash",
            MergeMethod::Merge => "merge",
            MergeMethod::Rebase => "rebase",
        }
    }

    /// A stored word back to its method; anything unknown is a squash.
    pub fn parse(word: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(word.trim()))
            .unwrap_or(MergeMethod::Squash)
    }

    /// What the form calls it.
    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Squash => "squash and merge",
            MergeMethod::Merge => "merge commit",
            MergeMethod::Rebase => "rebase and merge",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            MergeMethod::Squash => "--squash",
            MergeMethod::Merge => "--merge",
            MergeMethod::Rebase => "--rebase",
        }
    }
}

/// The merge form's rows, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRow {
    Method,
    DeleteBranch,
    Auto,
}

impl MergeRow {
    const ORDER: [MergeRow; 3] = [MergeRow::Method, MergeRow::DeleteBranch, MergeRow::Auto];

    fn step(self, down: bool) -> Self {
        let at = Self::ORDER.iter().position(|r| *r == self).unwrap_or(0);
        let next = if down {
            (at + 1).min(Self::ORDER.len() - 1)
        } else {
            at.saturating_sub(1)
        };
        Self::ORDER[next]
    }
}

/// `Ctrl+x`: the merge of the pull request under the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeForm {
    pub project: ProjectId,
    pub dir: PathBuf,
    pub number: u64,
    pub url: String,
    pub title: String,
    /// The branch it merges into, and the one it comes from — the latter
    /// only when it is a branch of this repo, which is when deleting it
    /// means anything (a fork's lives elsewhere).
    pub base: String,
    pub branch: Option<String>,
    /// The row's own name for the branch, for the form's head line while
    /// the body that says whose branch it is has not landed.
    pub head: String,
    /// The pull request's body had not landed when the form opened: the
    /// branch, the base and the warnings are filled in when it does
    /// ([`detail_landed`]).
    pub pending: bool,
    pub method: MergeMethod,
    /// The methods the repo allows; every one until `gh repo view` says.
    pub allowed: Vec<MergeMethod>,
    pub delete_branch: bool,
    /// The **Delete merged branch** SETTING as the form opened: what
    /// [`MergeForm::apply_detail`] ticks the box to once it knows the
    /// branch is ours.
    pub delete_default: bool,
    /// The repo deletes merged branches itself.
    pub auto_delete: bool,
    /// `--auto`: GitHub merges once the checks and reviews it requires
    /// are in.
    pub auto: bool,
    pub row: MergeRow,
    /// What GitHub said that stands in the way, one line each.
    pub warnings: Vec<String>,
    pub ticket: u64,
    pub saving: Option<String>,
    pub notice: Option<String>,
    /// As of the last draw: each row's rect, for the mouse.
    pub rows: Vec<(Rect, MergeRow)>,
}

impl MergeForm {
    /// Take the branch, the base and what stands in the way from the pull
    /// request's row and, once it has landed, its body.
    fn apply_detail(&mut self, pr: &OpenPr, detail: Option<&PrDetail>) {
        self.pending = detail.is_none();
        self.branch = deletable_branch(pr, detail);
        self.base = detail.map(|d| d.base.clone()).unwrap_or_default();
        self.warnings = merge_warnings(pr, detail);
        self.delete_branch = self.delete_default && self.branch.is_some();
    }
}

/// What stands in the way of merging, as GitHub said it last.
fn merge_warnings(pr: &OpenPr, detail: Option<&PrDetail>) -> Vec<String> {
    let mut out = Vec::new();
    let health = detail.map_or(pr.health, |d| d.health);
    if detail.map_or(pr.is_draft, |d| d.is_draft) {
        out.push("it is a draft — GitHub merges it once it is marked ready".into());
    }
    if health.conflicts {
        out.push("it has conflicts to resolve first".into());
    }
    match health.checks {
        Checks::Failing => out.push("some checks are failing".into()),
        Checks::Pending => out.push("checks are still running".into()),
        Checks::Passing | Checks::Absent => {}
    }
    match detail.map(|d| d.review_decision.as_str()) {
        Some("CHANGES_REQUESTED") => out.push("changes were requested in review".into()),
        Some("REVIEW_REQUIRED") => out.push("a review is still required".into()),
        Some(_) => {}
        None => out.push("its details are still loading — the checks and reviews follow".into()),
    }
    out
}

/// `Ctrl+x`: the merge form for the pull request under the cursor, on
/// the **Review** tab's defaults, the repo asked which methods it allows.
pub(crate) fn open_merge(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
        app.flash = Some("no pull request selected".into());
        return;
    };
    let config = crate::config::Config::load();
    let mut form = MergeForm {
        project,
        dir: dir.clone(),
        number: pr.number,
        url: pr.url.clone(),
        title: pr.title.clone(),
        base: String::new(),
        branch: None,
        head: pr.head.clone(),
        pending: true,
        method: MergeMethod::parse(&config.pr_merge_method),
        allowed: MergeMethod::ALL.to_vec(),
        delete_branch: false,
        delete_default: config.pr_delete_branch,
        auto_delete: false,
        auto: false,
        row: MergeRow::Method,
        warnings: Vec::new(),
        ticket: crate::view_jobs::ticket(),
        saving: None,
        notice: None,
        rows: Vec::new(),
    };
    form.apply_detail(&pr, app.pr_detail.get(&pr.url));
    let ticket = form.ticket;
    put_form(app, PrForm::Merge(form));
    if let Some(tx) = app.pr_actions_tx.clone() {
        tokio::spawn(async move {
            let (allowed, auto_delete) = repo_merge_options(&dir).await;
            let _ = tx.send(Answer::MergeOptions {
                ticket,
                allowed,
                auto_delete,
            });
        });
    }
    app.dirty = true;
}

/// The pull request's head branch, when it is a branch of this repo and
/// so ours to delete: a fork's row names its owner (`owner/branch`, the
/// row's `head`), which never matches the body's bare head. None until
/// the body is in.
fn deletable_branch(pr: &OpenPr, detail: Option<&PrDetail>) -> Option<String> {
    detail
        .filter(|d| !d.head.is_empty() && d.head == pr.head && d.head != d.base)
        .map(|d| d.head.clone())
}

/// A pull request's body landed: a merge form opened on it before it did
/// takes its branch, its base and what stands in the way from it.
pub(crate) fn detail_landed(app: &mut App, url: &str) {
    if merge_form(app).is_none_or(|f| !f.pending || f.url != url) {
        return;
    }
    let Some(pr) = crate::pr_modal::selected_pr(app).filter(|pr| pr.url == url) else {
        return;
    };
    let Some(detail) = app.pr_detail.get(url).cloned() else {
        return;
    };
    if let Some(form) = merge_form(app) {
        form.apply_detail(&pr, Some(&detail));
    }
    app.dirty = true;
}

/// The merge methods the repo allows, and whether it deletes merged
/// branches itself. Every method, and no, when `gh` can't say.
async fn repo_merge_options(dir: &Path) -> (Vec<MergeMethod>, bool) {
    let out = crate::pull_request::gh(
        Some(dir),
        &[
            "repo",
            "view",
            "--json",
            "squashMergeAllowed,mergeCommitAllowed,rebaseMergeAllowed,deleteBranchOnMerge",
        ],
        crate::pull_request::TIMEOUT,
    )
    .await;
    out.as_deref()
        .and_then(parse_merge_options)
        .unwrap_or_else(|| (MergeMethod::ALL.to_vec(), false))
}

/// `gh repo view`'s answer to [`repo_merge_options`]'s fields.
fn parse_merge_options(json: &str) -> Option<(Vec<MergeMethod>, bool)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let allowed = |key: &str| v.get(key).and_then(|x| x.as_bool()).unwrap_or(true);
    let methods: Vec<MergeMethod> = [
        (MergeMethod::Squash, "squashMergeAllowed"),
        (MergeMethod::Merge, "mergeCommitAllowed"),
        (MergeMethod::Rebase, "rebaseMergeAllowed"),
    ]
    .into_iter()
    .filter(|(_, key)| allowed(key))
    .map(|(method, _)| method)
    .collect();
    let auto_delete = v
        .get("deleteBranchOnMerge")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    (!methods.is_empty()).then_some((methods, auto_delete))
}

/// Enter on the merge form: `gh pr merge` off the loop, the form held
/// until the answer lands.
fn submit_merge(app: &mut App) {
    let tx = app.pr_actions_tx.clone();
    let Some(form) = merge_form(app) else {
        return;
    };
    if form.saving.is_some() {
        return;
    }
    let Some(tx) = tx else {
        return;
    };
    form.notice = None;
    form.saving = Some(if form.auto {
        format!("turning on auto-merge for #{}…", form.number)
    } else {
        format!("merging #{}…", form.number)
    });
    let (project, dir, ticket, number) = (
        form.project.clone(),
        form.dir.clone(),
        form.ticket,
        form.number,
    );
    let (method, auto) = (form.method, form.auto);
    // A repo that deletes merged branches does it itself.
    let delete = (form.delete_branch && !form.auto_delete)
        .then(|| form.branch.clone())
        .flatten();
    let base = form.base.clone();
    tokio::spawn(async move {
        let result = merge(&dir, number, method, auto, delete.as_deref())
            .await
            .map(|deleted| merged_message(number, &base, method, auto, deleted));
        let _ = tx.send(Answer::Merged {
            project,
            ticket,
            result,
        });
    });
}

/// Merge pull request `number` — or switch auto-merge on for it — and
/// delete `branch` on origin once it is merged. Whether the branch went.
async fn merge(
    dir: &Path,
    number: u64,
    method: MergeMethod,
    auto: bool,
    branch: Option<&str>,
) -> Result<bool, String> {
    let number = number.to_string();
    let mut args = vec!["pr", "merge", number.as_str(), method.flag()];
    if auto {
        args.push("--auto");
    }
    run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await?;
    // Auto-merge lands later, on GitHub, with nothing here to delete the
    // branch after it: only a repo that deletes merged branches does then.
    let Some(branch) = branch.filter(|_| !auto) else {
        return Ok(false);
    };
    let path = format!("repos/{{owner}}/{{repo}}/git/refs/heads/{branch}");
    let delete = gh(dir, &["api", "-X", "DELETE", &path]);
    Ok(run_piped(delete, "", REQUEST_TIMEOUT).await.is_ok())
}

/// The flash a merge leaves.
fn merged_message(
    number: u64,
    base: &str,
    method: MergeMethod,
    auto: bool,
    deleted: bool,
) -> String {
    let into = if base.is_empty() {
        String::new()
    } else {
        format!(" into {base}")
    };
    let gone = if deleted { ", branch deleted" } else { "" };
    if auto {
        format!(
            "#{number} merges{into} once GitHub's requirements are met ({}){gone}",
            method.as_str()
        )
    } else {
        format!("merged #{number}{into} ({}){gone}", method.as_str())
    }
}

// ---- running git and gh ----

/// `gh <args>` in `dir`, for [`run_piped`].
fn gh(dir: &Path, args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(args).current_dir(dir);
    cmd
}

/// `git <args>` in `dir` the way every TUI-side git runs
/// (`git_diff::git_command`), for [`run_piped`] — and, as the BRANCH
/// SWITCHER runs the git that reaches a remote, in a session of its own
/// with no terminal to prompt on: a push whose ssh wants a passphrase
/// fails rather than draw over the frame.
fn git(dir: &Path, args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::from(crate::git_diff::git_command(dir));
    cmd.args(args).env("GIT_TERMINAL_PROMPT", "0");
    // SAFETY: setsid is async-signal-safe and touches nothing but the child.
    unsafe {
        cmd.pre_exec(crate::ipc::own_session);
    }
    cmd
}

// ---- answers ----

/// What the forms' git and `gh` came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The create form's branches, and the default to merge into.
    Branches {
        ticket: u64,
        heads: Vec<String>,
        bases: Vec<String>,
        base: Option<String>,
    },
    /// The commits between a pair of branches, for the fill.
    Fill {
        ticket: u64,
        pair: (String, String),
        fill: Option<Fill>,
    },
    /// `gh pr create`: the new pull request's URL, or why not.
    Created {
        project: ProjectId,
        ticket: u64,
        result: Result<String, String>,
    },
    /// The repo's merge methods, and whether it deletes merged branches.
    MergeOptions {
        ticket: u64,
        allowed: Vec<MergeMethod>,
        auto_delete: bool,
    },
    /// `gh pr merge`: what to flash, or why it refused.
    Merged {
        project: ProjectId,
        ticket: u64,
        result: Result<String, String>,
    },
}

/// The PULL REQUESTS MODAL, while it is what is up.
fn modal(app: &mut App) -> Option<&mut PullRequestsView> {
    match &mut app.overlay {
        Some(Overlay::PullRequests(view)) => Some(view),
        _ => None,
    }
}

/// Whether a form is up in the PULL REQUESTS MODAL: every key, paste and
/// click is the form's then.
pub(crate) fn form_up(app: &App) -> bool {
    matches!(&app.overlay, Some(Overlay::PullRequests(view)) if view.form.is_some())
}

/// The form up in the modal.
fn form(app: &mut App) -> Option<&mut PrForm> {
    modal(app)?.form.as_deref_mut()
}

/// Put `form` up in the modal's reading pane.
fn put_form(app: &mut App, form: PrForm) {
    if let Some(view) = modal(app) {
        view.form = Some(Box::new(form));
    }
}

/// The modal's form, when it is the one `ticket` was asked for.
fn form_for(app: &mut App, ticket: u64) -> Option<&mut PrForm> {
    form(app).filter(|form| form.ticket() == ticket)
}

fn create_form(app: &mut App) -> Option<&mut CreateForm> {
    match form(app)? {
        PrForm::Create(f) => Some(f),
        PrForm::Merge(_) => None,
    }
}

fn merge_form(app: &mut App) -> Option<&mut MergeForm> {
    match form(app)? {
        PrForm::Merge(f) => Some(f),
        PrForm::Create(_) => None,
    }
}

/// Close the form, the reading pane back.
fn close_form(app: &mut App) {
    if let Some(view) = modal(app) {
        view.form = None;
    }
}

/// Land one answer. A create or a merge is flashed whether or not its
/// form is still up — it happened either way — and the project's list is
/// asked for again; the rest only matter to the form that asked.
pub(crate) fn land_answer(app: &mut App, answer: Answer) {
    match answer {
        Answer::Branches {
            ticket,
            heads,
            bases,
            base,
        } => land_branches(app, ticket, heads, bases, base),
        Answer::Fill { ticket, pair, fill } => land_fill(app, ticket, pair, fill),
        Answer::Created {
            project,
            ticket,
            result,
        } => match result {
            Ok(url) => land_created(app, &project, ticket, url),
            Err(why) => refused(app, ticket, why, "open the pull request"),
        },
        Answer::MergeOptions {
            ticket,
            allowed,
            auto_delete,
        } => {
            if let Some(PrForm::Merge(form)) = form_for(app, ticket) {
                if !allowed.contains(&form.method) {
                    form.method = allowed[0];
                }
                form.allowed = allowed;
                form.auto_delete = auto_delete;
            }
        }
        Answer::Merged {
            project,
            ticket,
            result,
        } => match result {
            Ok(said) => {
                if form_for(app, ticket).is_some() {
                    close_form(app);
                }
                crate::pr_modal::request_list(app, &project);
                app.flash = Some(said);
            }
            Err(why) => refused(app, ticket, why, "merge"),
        },
    }
    app.dirty = true;
}

/// The create form's branches are read: Into takes the project's base when
/// nothing is typed there (and it is not From itself), the list under the
/// caret opens on its field's branch, and the fill is asked for.
fn land_branches(
    app: &mut App,
    ticket: u64,
    heads: Vec<String>,
    bases: Vec<String>,
    base: Option<String>,
) {
    let Some(PrForm::Create(form)) = form_for(app, ticket) else {
        return;
    };
    if form.into.trim().is_empty() {
        if let Some(base) = base.filter(|b| *b != form.from.trim()) {
            form.into.set_text(base);
        }
    }
    form.heads = Some(heads.into());
    form.bases = Some(bases.into());
    if !form.typed {
        form.focus(form.field);
    }
    request_fill(app);
}

/// A fill is in for the pair it was asked for: the count goes beside From,
/// and the title and description take it wherever they still hold the
/// last fill — never over what was typed.
fn land_fill(app: &mut App, ticket: u64, pair: (String, String), fill: Option<Fill>) {
    let Some(PrForm::Create(form)) = form_for(app, ticket) else {
        return;
    };
    if form.filled_for.as_ref() != Some(&pair) {
        return;
    }
    form.ahead = fill.as_ref().map(|fill| fill.ahead);
    let listed = fill
        .as_ref()
        .map_or(&[][..], |fill| fill.commits.as_slice());
    let (title, body) = fill_text(&pair.0, listed);
    if form.title.as_str() == form.filled.0 {
        form.title.set_text(title.clone());
    }
    if form.body.as_str() == form.filled.1 {
        form.body.set_text(body.clone());
    }
    form.filled = (title, body);
}

/// `gh pr create` answered with the new pull request's `url`: the form
/// closes onto the list, whose cursor follows the new row once the list
/// asked for here lands.
fn land_created(app: &mut App, project: &ProjectId, ticket: u64, url: String) {
    if form_for(app, ticket).is_some() {
        close_form(app);
        if let Some(view) = modal(app) {
            if !url.is_empty() {
                view.selected_url = Some(url.clone());
            }
            view.focus = crate::pr_modal::PrFocus::List;
        }
    }
    crate::pr_modal::request_list(app, project);
    app.flash = Some(if url.is_empty() {
        "opened the pull request".into()
    } else {
        format!("opened {url}")
    });
}

/// A create or merge refused (`what` it was trying to do): the form that
/// sent it says why, or — closed since — the footer does.
fn refused(app: &mut App, ticket: u64, why: String, what: &str) {
    match form_for(app, ticket) {
        Some(form) => form.refused(why),
        None => app.flash = Some(format!("couldn't {what}: {why}")),
    }
}

// ---- keys ----

/// The forms' own keys: one table [`handle_key`] matches and [`hints`]
/// spells.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const FIELD: Key = Key::new(&["tab", "shift+tab"], "field");
    /// A branch field's list.
    pub const PICK: Key = Key::new(&["up", "down"], "pick").show(2);
    pub const CHOOSE: Key = Key::new(&["enter"], "choose");
    pub const CREATE: Key = Key::new(&["enter"], "create PR");
    pub const DRAFT: Key = Key::new(&["space"], "draft");
    /// The merge form's rows.
    pub const OPTION: Key = Key::new(&["up", "down"], "option").show(2);
    pub const CHANGE: Key = Key::new(&["left", "right", "space"], "change").show(2);
    pub const MERGE: Key = Key::new(&["enter"], "merge");
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        FIELD,
        PICK,
        CHOOSE,
        CREATE,
        DRAFT,
        OPTION,
        CHANGE,
        MERGE,
        crate::ui::task_keys::NEWLINE,
    ];
}

/// The keys along the modal's bottom edge while a form is up.
pub(crate) fn hints(form: &PrForm) -> Vec<Hint> {
    if form.saving() {
        return vec![Hint::new("Esc", "close")];
    }
    match form {
        PrForm::Create(form) => {
            let mut hints = Vec::new();
            match form.field {
                field if field.is_branch() => {
                    hints.push(keys::CHOOSE.hint().kept());
                    hints.push(keys::PICK.hint());
                }
                CreateField::Draft => {
                    hints.push(keys::CREATE.hint().kept());
                    hints.push(keys::DRAFT.hint());
                }
                CreateField::Body => {
                    hints.push(keys::CREATE.hint().kept());
                    hints.push(crate::ui::task_keys::NEWLINE.hint());
                }
                _ => hints.push(keys::CREATE.hint().kept()),
            }
            hints.push(keys::FIELD.hint());
            hints.push(Hint::new("Esc", "cancel"));
            hints
        }
        PrForm::Merge(form) => vec![
            keys::MERGE
                .hint_as(if form.auto { "auto-merge" } else { "merge" })
                .kept(),
            keys::OPTION.hint(),
            keys::CHANGE.hint(),
            Hint::new("Esc", "cancel"),
        ],
    }
}

/// A key while a form is up: every key is the form's. Esc closes it — a
/// create or merge already sent still lands, and says so.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(form) = form(app) else {
        return;
    };
    let (is_create, saving) = (matches!(form, PrForm::Create(_)), form.saving());
    if key.code == KeyCode::Esc {
        close_form(app);
    } else if saving {
        // Nothing to change while it is on its way.
    } else if is_create {
        create_key(app, key);
    } else {
        merge_key(app, key);
    }
    app.dirty = true;
}

fn create_key(app: &mut App, key: KeyEvent) {
    let Some(form) = create_form(app) else {
        return;
    };
    let field = form.field;
    let mut submit = false;
    let mut fill = false;
    match key.code {
        // Tab off a typed branch field takes its best match with it.
        _ if keys::FIELD.matches(&key) => {
            let forward = key.code == KeyCode::Tab
                && !key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::SHIFT);
            if field.is_branch() && form.typed {
                form.take_pick();
            }
            fill = field.is_branch();
            form.focus(field.step(forward));
        }
        KeyCode::Down | KeyCode::Up if field.is_branch() => {
            form.move_pick(if key.code == KeyCode::Down { 1 } else { -1 })
        }
        KeyCode::Enter if field.is_branch() => {
            form.choose_pick();
            fill = true;
        }
        // ↑/↓ walk the description's lines first; past its first line,
        // and from a one-line field, they move between the fields.
        KeyCode::Down | KeyCode::Up => {
            let down = key.code == KeyCode::Down;
            let consumed = form
                .input_mut()
                .is_some_and(|input| input.handle_key(&key).consumed());
            if !consumed && !(down && field == CreateField::Body) {
                form.focus(field.step(down));
            }
        }
        KeyCode::Char(' ') if field == CreateField::Draft => form.draft = !form.draft,
        // A line break is the description's own; asked for on the title,
        // it steps down into the description rather than sending.
        _ if field == CreateField::Title && TextInput::is_newline_key(&key) => {
            form.focus(CreateField::Body)
        }
        KeyCode::Enter if !(field == CreateField::Body && form.body.takes_newline(&key)) => {
            submit = true
        }
        _ => {
            if form
                .input_mut()
                .is_some_and(|i| i.handle_key(&key).changed())
            {
                form.notice = None;
                if field.is_branch() {
                    form.typed_in();
                }
            }
        }
    }
    if fill {
        request_fill(app);
    }
    if submit {
        submit_create(app);
    }
}

fn merge_key(app: &mut App, key: KeyEvent) {
    let Some(form) = merge_form(app) else {
        return;
    };
    match key.code {
        KeyCode::Down | KeyCode::Tab => form.row = form.row.step(true),
        KeyCode::Up | KeyCode::BackTab => form.row = form.row.step(false),
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
            change_merge_row(form, key.code != KeyCode::Left)
        }
        KeyCode::Enter => submit_merge(app),
        _ => {}
    }
}

/// `←`/`→`/Space on a merge row: the next method the repo allows, or the
/// box flipped.
fn change_merge_row(form: &mut MergeForm, forward: bool) {
    form.notice = None;
    match form.row {
        MergeRow::Method => {
            let n = form.allowed.len().max(1);
            let at = form
                .allowed
                .iter()
                .position(|m| *m == form.method)
                .unwrap_or(0);
            let next = if forward {
                (at + 1) % n
            } else {
                (at + n - 1) % n
            };
            if let Some(method) = form.allowed.get(next) {
                form.method = *method;
            }
        }
        MergeRow::DeleteBranch if form.branch.is_some() => {
            form.delete_branch = !form.delete_branch;
        }
        MergeRow::DeleteBranch => {}
        MergeRow::Auto => form.auto = !form.auto,
    }
}

/// A paste lands in the create form's field under the caret — lines kept
/// in the description, flattened anywhere else. True while a form is up.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(form) = form(app) else {
        return false;
    };
    let PrForm::Create(form) = form else {
        return true;
    };
    if form.saving.is_some() {
        return true;
    }
    let field = form.field;
    if let Some(input) = form.input_mut() {
        input.insert_str(text);
        if field.is_branch() {
            form.typed_in();
        }
    }
    true
}

/// The mouse while a form is up: a click puts the caret on a field —
/// on a branch in the list, takes it — or works a merge row as Space
/// does; the wheel walks a branch list. Nothing else, so a draft is
/// never dropped under the pointer; Esc is the way out.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, at: Position) {
    let clicked = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
    if form(app).is_none_or(|form| form.saving()) {
        return;
    }
    if let Some(form) = create_form(app) {
        let field = form.field;
        let mut fill = false;
        match mouse.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if field.is_branch() => form
                .move_pick(if mouse.kind == MouseEventKind::ScrollDown {
                    1
                } else {
                    -1
                }),
            _ if clicked => {
                if let Some(&(_, pick)) = form.picks.iter().find(|(r, _)| r.contains(at)) {
                    form.pick = pick;
                    form.choose_pick();
                    fill = true;
                } else if let Some(&(_, to)) = form.rows.iter().find(|(r, _)| r.contains(at)) {
                    if to == CreateField::Draft && field == CreateField::Draft {
                        form.draft = !form.draft;
                    }
                    fill = field.is_branch();
                    form.focus(to);
                }
            }
            _ => {}
        }
        if fill {
            request_fill(app);
        }
    } else if let Some(form) = merge_form(app) {
        if !clicked {
            return;
        }
        if let Some(&(_, row)) = form.rows.iter().find(|(r, _)| r.contains(at)) {
            form.row = row;
            change_merge_row(form, true);
        }
    }
    app.dirty = true;
}

// ---- drawing ----

/// What a form's draw left on screen, for [`write_back`]: the width its
/// foot took on the bottom border, its rows and picks, and the view the
/// description was drawn with.
#[derive(Debug, Clone, Default)]
pub struct Drawn {
    pub foot_w: u16,
    create_rows: Vec<(Rect, CreateField)>,
    merge_rows: Vec<(Rect, MergeRow)>,
    picks: Vec<(Rect, usize)>,
    body_view: Option<TextView>,
}

/// Put what the draw learned back on the live form (the draw works on a
/// clone).
pub(crate) fn write_back(form: &mut PrForm, drawn: Drawn) {
    match form {
        PrForm::Create(form) => {
            form.rows = drawn.create_rows;
            form.picks = drawn.picks;
            if let Some(view) = drawn.body_view {
                form.body.set_view(view);
            }
        }
        PrForm::Merge(form) => form.rows = drawn.merge_rows,
    }
}

/// Draw `form` into `area`, the reading pane's place.
pub(crate) fn draw(f: &mut Frame, area: Rect, form: &PrForm, focused: bool, th: Theme) -> Drawn {
    match form {
        PrForm::Create(form) => draw_create(f, area, form, focused, th),
        PrForm::Merge(form) => draw_merge(f, area, form, focused, th),
    }
}

/// A `[x]` box.
fn check(on: bool) -> &'static str {
    if on {
        "[x]"
    } else {
        "[ ]"
    }
}

fn draw_create(f: &mut Frame, area: Rect, form: &CreateForm, focused: bool, th: Theme) -> Drawn {
    let (inner, foot_w) = form_frame(
        f,
        area,
        "New pull request",
        form.notice.as_deref(),
        form.saving.as_deref(),
        focused,
        th,
    );
    let mut drawn = Drawn {
        foot_w,
        ..Drawn::default()
    };
    let dim = Style::default().fg(th.dim);
    let caret = |field: CreateField| focused && form.field == field && form.saving.is_none();
    let width = usize::from(inner.width);
    let line_of = |field: CreateField, label: &str, input: &TextInput, placeholder: &str| {
        form_field(label, input, placeholder, caret(field), width, th)
    };
    // From, with how far ahead it is once the fill has counted.
    let mut from = line_of(CreateField::From, "From", &form.from, "a branch of yours");
    if let Some(ahead) = form.ahead {
        let into = form.into.trim();
        let noun = if ahead == 1 { "commit" } else { "commits" };
        let note = if into.is_empty() {
            format!("  {ahead} {noun}")
        } else {
            format!("  {ahead} {noun} not on {into}")
        };
        from.push(Span::styled(note, dim));
    }
    let into = line_of(
        CreateField::Into,
        "Into",
        &form.into,
        "the branch it merges into",
    );
    let title = line_of(CreateField::Title, "Title", &form.title, "(required)");
    let draft = vec![
        form_label("Draft", caret(CreateField::Draft), th),
        Span::styled(check(form.draft), Style::default().fg(th.text)),
        Span::styled(
            if form.draft {
                "  opened as a draft"
            } else {
                "  ready for review"
            },
            dim,
        ),
    ];
    let rows = [
        (CreateField::From, from),
        (CreateField::Into, into),
        (CreateField::Title, title),
        (CreateField::Draft, draft),
    ];
    for (i, (field, spans)) in rows.into_iter().enumerate() {
        if let Some(row) = row_rect(inner, i) {
            f.render_widget(Paragraph::new(crate::pr_preview::fit(spans, width)), row);
            drawn.create_rows.push((row, field));
        }
    }

    // Under the fields: the branch list while a branch field has the
    // caret, the description otherwise.
    let box_area = Rect {
        y: inner.y.saturating_add(FIELD_ROWS),
        height: inner.height.saturating_sub(FIELD_ROWS),
        ..inner
    };
    if box_area.height < 3 || box_area.width < 4 {
        return drawn;
    }
    let listing = form.field.is_branch() && focused && form.saving.is_none();
    if listing {
        let title = match form.field {
            CreateField::From => "Your branches",
            _ => "Branches on origin",
        };
        let inner = form_box(f, box_area, title, true, th);
        draw_picks(f, form, inner, &mut drawn, th);
        return drawn;
    }
    drawn.create_rows.push((box_area, CreateField::Body));
    let on_body = caret(CreateField::Body);
    drawn.body_view = form_text_box(
        f,
        box_area,
        "Description",
        &form.body,
        on_body,
        "(no description)",
        th,
    );
    drawn
}

/// The branch list under a branch field: the rows [`CreateForm::choices`]
/// leaves, the cursor's kept in view.
fn draw_picks(f: &mut Frame, form: &CreateForm, area: Rect, drawn: &mut Drawn, th: Theme) {
    let say = |f: &mut Frame, text: &str| crate::ui::empty_list_row(f, area, text, th);
    let loaded = match form.field {
        CreateField::From => form.heads.is_some(),
        _ => form.bases.is_some(),
    };
    if !loaded {
        say(f, "reading branches…");
        return;
    }
    let choices = form.choices();
    if choices.is_empty() {
        say(
            f,
            if form.branches().is_empty() {
                "no branches to pick from"
            } else {
                "no branch matches — Enter keeps what is typed"
            },
        );
        return;
    }
    let height = usize::from(area.height.max(1));
    let pick = form.pick.min(choices.len() - 1);
    let top = crate::app::window_start(pick, height);
    let branches = form.branches();
    let budget = usize::from(area.width).saturating_sub(2);
    for (row, (index, positions)) in choices.iter().enumerate().skip(top).take(height) {
        let Some(rect) = row_rect(area, row - top) else {
            break;
        };
        let name = &branches[*index];
        let shown = truncate(name, budget);
        let positions = crate::ui::visible_positions(positions, &shown, name);
        let spans = fuzzy_highlight_styled(&shown, positions, Style::default().fg(th.text), th);
        render_row(f, rect, spans, row == pick, true, th);
        drawn.picks.push((rect, row));
    }
}

fn draw_merge(f: &mut Frame, area: Rect, form: &MergeForm, focused: bool, th: Theme) -> Drawn {
    let (inner, foot_w) = form_frame(
        f,
        area,
        &format!("Merge #{}", form.number),
        form.notice.as_deref(),
        form.saving.as_deref(),
        focused,
        th,
    );
    let mut drawn = Drawn {
        foot_w,
        ..Drawn::default()
    };
    let width = usize::from(inner.width);
    let dim = Style::default().fg(th.dim);
    let text = Style::default().fg(th.text);
    let on = |row: MergeRow| focused && form.row == row && form.saving.is_none();
    let mut lines: Vec<(Option<MergeRow>, Vec<Span<'static>>)> = Vec::new();
    lines.push((
        None,
        vec![Span::styled(
            format!("{INDENT}{}", form.title),
            text.add_modifier(Modifier::BOLD),
        )],
    ));
    let into = if form.base.is_empty() {
        "its base".to_string()
    } else {
        form.base.clone()
    };
    let from = form.branch.clone().unwrap_or_else(|| form.head.clone());
    lines.push((
        None,
        vec![Span::styled(format!("{INDENT}{from} → {into}"), dim)],
    ));
    lines.push((None, Vec::new()));

    let others = form.allowed.len() > 1;
    let mut method = vec![
        form_label("Method", on(MergeRow::Method), th),
        Span::styled(
            if others { "◂ " } else { "  " },
            Style::default().fg(th.muted),
        ),
        Span::styled(form.method.label(), text.add_modifier(Modifier::BOLD)),
    ];
    if others {
        method.push(Span::styled(" ▸", Style::default().fg(th.muted)));
    }
    lines.push((Some(MergeRow::Method), method));
    let (delete_on, delete_note) = match (&form.branch, form.auto_delete) {
        (None, _) if form.pending => (false, "  known once its details are in".to_string()),
        (None, _) => (false, "  not a branch of this repo — it stays".to_string()),
        (Some(_), true) => (
            true,
            "  the repo deletes merged branches itself".to_string(),
        ),
        (Some(_), false) if form.delete_branch && form.auto => (
            true,
            "  not after an auto-merge — the branch stays".to_string(),
        ),
        (Some(branch), false) if form.delete_branch => {
            (true, format!("  {branch} on GitHub, once merged"))
        }
        (Some(_), false) => (false, "  the branch stays on GitHub".to_string()),
    };
    lines.push((
        Some(MergeRow::DeleteBranch),
        vec![
            form_label("Delete", on(MergeRow::DeleteBranch), th),
            Span::styled(check(delete_on), text),
            Span::styled(delete_note, dim),
        ],
    ));
    lines.push((
        Some(MergeRow::Auto),
        vec![
            form_label("Auto", on(MergeRow::Auto), th),
            Span::styled(check(form.auto), text),
            Span::styled(
                if form.auto {
                    "  GitHub merges once its checks and reviews are in"
                } else {
                    "  merge now"
                },
                dim,
            ),
        ],
    ));
    if !form.warnings.is_empty() {
        lines.push((None, Vec::new()));
        for warning in &form.warnings {
            lines.push((
                None,
                vec![Span::styled(
                    format!("{INDENT}⚠ {warning}"),
                    Style::default().fg(th.warn),
                )],
            ));
        }
    }
    for (i, (row, spans)) in lines.into_iter().enumerate() {
        let Some(rect) = row_rect(inner, i) else {
            break;
        };
        f.render_widget(Paragraph::new(crate::pr_preview::fit(spans, width)), rect);
        if let Some(row) = row {
            drawn.merge_rows.push((rect, row));
        }
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fill_is_ghs() {
        assert_eq!(
            fill_text(
                "fix/login-redirect",
                &[("Stop the bounce".into(), "Why.".into())]
            ),
            ("Stop the bounce".to_string(), "Why.".to_string())
        );
        assert_eq!(
            fill_text(
                "fix/login-redirect",
                &[("One".into(), String::new()), ("Two".into(), "b".into())]
            ),
            ("Login redirect".to_string(), "- One\n- Two".to_string())
        );
        assert_eq!(
            fill_text("eager_sparrow", &[]),
            ("Eager sparrow".to_string(), String::new())
        );
        let many: Vec<(String, String)> = (0..FILL_SUBJECTS)
            .map(|i| (format!("c{i}"), String::new()))
            .collect();
        let (_, body) = fill_text("x", &many);
        assert!(
            body.ends_with("- c49\n- …"),
            "a capped list says so: {body}"
        );
    }

    #[test]
    fn commits_parse_subject_then_body() {
        let text = "One\x1f\x1e\nTwo\x1fline 1\nline 2\n\x1e\n";
        assert_eq!(
            parse_commits(text),
            vec![
                ("One".to_string(), String::new()),
                ("Two".to_string(), "line 1\nline 2".to_string())
            ]
        );
    }

    #[test]
    fn merge_options_keep_what_the_repo_allows() {
        assert_eq!(
            parse_merge_options(
                r#"{"squashMergeAllowed":true,"mergeCommitAllowed":false,"rebaseMergeAllowed":true,"deleteBranchOnMerge":true}"#
            ),
            Some((vec![MergeMethod::Squash, MergeMethod::Rebase], true))
        );
        assert_eq!(
            parse_merge_options("{}"),
            Some((MergeMethod::ALL.to_vec(), false))
        );
        assert_eq!(parse_merge_options("nope"), None);
        assert_eq!(MergeMethod::parse("REBASE"), MergeMethod::Rebase);
        assert_eq!(MergeMethod::parse("whatever"), MergeMethod::Squash);
    }

    #[test]
    fn tab_walks_the_create_form_round() {
        let mut field = CreateField::From;
        for expected in [
            CreateField::Into,
            CreateField::Title,
            CreateField::Draft,
            CreateField::Body,
            CreateField::From,
        ] {
            field = field.step(true);
            assert_eq!(field, expected);
        }
        assert_eq!(CreateField::From.step(false), CreateField::Body);
    }

    #[test]
    fn the_hints_come_from_the_table() {
        let form = CreateForm::new(
            ProjectId("p".into()),
            PathBuf::from("/x"),
            "feature".into(),
            false,
        );
        for field in CreateField::ORDER {
            let mut form = form.clone();
            form.field = field;
            crate::hints::assert_hints_from(&hints(&PrForm::Create(form)), keys::ALL);
        }
    }

    /// A branch field's list shows every branch until it is typed in, the
    /// cursor on the field's own; typing narrows it, and Enter takes the
    /// best match.
    #[test]
    fn a_branch_field_picks_from_the_branches() {
        let mut form = CreateForm::new(
            ProjectId("p".into()),
            PathBuf::from("/x"),
            "feature".into(),
            false,
        );
        form.heads = Some(["main", "feature", "fix/login"].map(String::from).into());
        form.focus(CreateField::From);
        assert_eq!(form.choices().len(), 3);
        assert_eq!(form.pick, 1, "on the field's own branch");
        form.from.set_text("login");
        form.typed_in();
        assert_eq!(form.choices().len(), 1);
        assert!(form.take_pick());
        assert_eq!(form.from.as_str(), "fix/login");
    }
}
