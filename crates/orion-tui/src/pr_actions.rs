//! The PULL REQUESTS MODAL's forms, each drawn in the reading pane's
//! place while the list stays up on the left:
//!
//! * **New pull request** (`⌘N`), in one of two modes along its top
//!   (`←`/`→` there):
//!   - **Feature branch**: the branch it comes from — the one the
//!     Worktrees cursor's checkout is on, else the ROOT WORKTREE's — the
//!     branch it merges into — the project's base, in the order the
//!     COMMIT LIST measures a branch against (`commit_list::resolve_base`)
//!     — a title, a draft box, the branch's commits and a description. A
//!     branch field lists the project's branches in the list's place while
//!     it has the caret (local ones for From, origin's for Into), narrowed
//!     as it is typed in. The title and description are filled from the
//!     commits between the two the way `gh pr create --fill` fills them —
//!     one commit's subject and body, or the branch's name and a list of
//!     subjects — and follow a new pair of branches until they are typed
//!     over. Enter pushes the branch to `origin` (setting its upstream when
//!     it has none) and runs `gh pr create`.
//!   - **Promote** ([`promote`]): the project's base into the repo's
//!     default branch (`dev` → `main`), as origin has them — nothing is
//!     pushed. It lists every pull request merged into the base that the
//!     default lacks, with its Linear issues and author, and fills the
//!     title (`Release 5 Nov: …`) and a `## Going out` description from
//!     them. A promotion already open is said, and Enter updates its title
//!     and description (`gh pr edit`) rather than open a second. A form
//!     opened from the base branch itself starts in this mode.
//!
//!   Either way the list is asked again, and the modal's cursor follows
//!   the pull request onto its row.
//! * **Merge** (`⌘X`): the pull request under the cursor, how — squash,
//!   a merge commit or a rebase, only those the repo allows (`gh repo
//!   view`) — whether its branch goes with it, and whether GitHub should
//!   wait for its checks and reviews (auto-merge). What GitHub said that
//!   stands in the way is spelled out first: a draft, conflicts, failing
//!   or running checks, a review still owed. Enter runs `gh pr merge`;
//!   the pull requests stacked on the branch are moved onto its base
//!   before it is deleted, which GitHub would otherwise close them for,
//!   and go back if it won't go. A long-lived branch — the repo's default,
//!   a protected one, the base new worktrees start from — is never deleted,
//!   so a `dev` merged into `main` leaves what is based on it alone
//!   ([`long_lived`]). It opens on the method the project last merged by.
//! * **Close** (`⌘W`): the pull request under the cursor closed
//!   without merging — a comment left on it first if one is written, and
//!   its branch deleted on GitHub if that is ticked (only a branch of this
//!   repo's; a fork's lives elsewhere, and never a long-lived one), and
//!   kept while pull requests are stacked on it. Enter runs `gh pr close`.
//!
//! And one verb with no form: `⌘D` ([`toggle_draft`]) marks a draft
//! ready for review, or turns a ready one back into a draft (`gh pr
//! ready`, `--undo`) — undone by pressing it again.
//!
//! Every git and `gh` here runs off the loop — a key handler never blocks
//! — and the answers land in [`land_answer`] through `App::pr_actions_tx`,
//! matched to the form by ticket, so an answer for a form since closed is
//! only flashed.

pub mod promote;

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
use crate::git_proc::PUSH_TIMEOUT;
use crate::hints::Hint;
use crate::pr_modal::PullRequestsView;
use crate::pr_store::PrStatus;
use crate::pull_request::{run_piped, Checks, OpenPr, PrDetail};
use crate::text_input::{TextInput, TextView};
use crate::theme::Theme;
use crate::ui::{
    form_box, form_field, form_frame, form_label, form_text_box, fuzzy_highlight_styled,
    render_row, row_rect, truncate,
};

/// Everything else the forms send is one request — `gh pr create`, `gh pr
/// merge`, the branch's delete — or one question git answers locally.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// The rows the create form's top takes, over the list and the box under
/// it — the modes, what the mode says, From and Into, Title, Draft.
const FIELD_ROWS: u16 = 5;
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
    Close(CloseForm),
}

impl PrForm {
    /// The form's own ticket: an answer carrying another is for a form
    /// since closed.
    fn ticket(&self) -> u64 {
        match self {
            PrForm::Create(f) => f.ticket,
            PrForm::Merge(f) => f.ticket,
            PrForm::Close(f) => f.ticket,
        }
    }

    /// GitHub says the branch is long-lived: its box goes off and stays off.
    fn keep_branch(&mut self, why: String) {
        let (stays, delete_branch) = match self {
            PrForm::Merge(f) => (&mut f.stays, &mut f.delete_branch),
            PrForm::Close(f) => (&mut f.stays, &mut f.delete_branch),
            PrForm::Create(_) => return,
        };
        *stays = Some(why);
        *delete_branch = false;
    }

    /// Enter sent it and the answer is not in: it takes no keys but Esc.
    fn saving(&self) -> bool {
        match self {
            PrForm::Create(f) => f.saving.is_some(),
            PrForm::Merge(f) => f.saving.is_some(),
            PrForm::Close(f) => f.saving.is_some(),
        }
    }

    /// GitHub (or git) refused what it sent: it says why on its frame and
    /// takes keys again, everything typed still in it.
    fn refused(&mut self, why: String) {
        let (saving, notice) = match self {
            PrForm::Create(f) => (&mut f.saving, &mut f.notice),
            PrForm::Merge(f) => (&mut f.saving, &mut f.notice),
            PrForm::Close(f) => (&mut f.saving, &mut f.notice),
        };
        *saving = None;
        *notice = Some(why);
    }
}

// ---- the new pull request ----

/// The create form's two modes, along its top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateMode {
    /// A branch of yours into the base: From and Into are yours to pick,
    /// the branch's commits listed.
    Feature,
    /// The project's base branch into the repo's default one (`dev` →
    /// `main`), what goes out listed ([`promote`]).
    Promote,
}

/// The create form's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateField {
    /// The mode row along the top: `←`/`→` switch.
    Mode,
    From,
    Into,
    Title,
    Draft,
    Body,
}

impl CreateField {
    /// Tab order, Feature mode.
    const FEATURE: [CreateField; 6] = [
        CreateField::Mode,
        CreateField::From,
        CreateField::Into,
        CreateField::Title,
        CreateField::Draft,
        CreateField::Body,
    ];
    /// Tab order, Promote mode: its branches are the mode's.
    const PROMOTE: [CreateField; 4] = [
        CreateField::Mode,
        CreateField::Title,
        CreateField::Draft,
        CreateField::Body,
    ];

    /// A branch field: it lists branches to pick from while it has the
    /// caret.
    pub fn is_branch(self) -> bool {
        matches!(self, CreateField::From | CreateField::Into)
    }
}

/// `⌘N`: a new pull request, filled in before it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateForm {
    pub project: ProjectId,
    /// Where git and `gh` run: the project's main checkout.
    pub dir: PathBuf,
    pub mode: CreateMode,
    /// The mode was picked by hand: the branches landing never switch it.
    pub mode_chosen: bool,
    /// The project's base branch and the repo's default, once read: the
    /// Promote mode's From and Into.
    pub base: Option<String>,
    pub default: Option<String>,
    /// What the Promote mode read for that pair, once it is in — `Err`,
    /// why GitHub could not say — and the pair it was asked for.
    pub promotion: Option<Result<promote::Promotion, String>>,
    pub promotion_for: Option<(String, String)>,
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
    /// The newest of those commits, oldest first as the fill read them —
    /// the Feature mode's list, drawn newest first.
    pub commits: Vec<FillCommit>,
    /// Origin is being fetched: the branches and the count are as of the
    /// last fetch until it lands, and read again then.
    pub fetching: bool,
    /// This form's own ticket: an answer carrying another is for a form
    /// since closed.
    pub ticket: u64,
    /// What is in flight once Enter sent it — the push, then the create.
    pub saving: Option<String>,
    /// Why the last Enter went nowhere, shown on the frame until the next
    /// edit.
    pub notice: Option<String>,
    /// As of the last draw, for the mouse: each field's row, the branch
    /// list's rows by index into what it shows, and each mode's chip.
    pub rows: Vec<(Rect, CreateField)>,
    pub picks: Vec<(Rect, usize)>,
    pub mode_hits: Vec<(Rect, CreateMode)>,
}

impl CreateForm {
    /// A new pull request from `from`, its Draft box off.
    fn new(project: ProjectId, dir: PathBuf, from: String) -> Self {
        Self {
            project,
            dir,
            mode: CreateMode::Feature,
            mode_chosen: false,
            base: None,
            default: None,
            promotion: None,
            promotion_for: None,
            from: TextInput::with_text(from),
            into: TextInput::new(),
            title: TextInput::new(),
            body: TextInput::multiline(),
            draft: false,
            field: CreateField::Title,
            heads: None,
            bases: None,
            typed: false,
            pick: 0,
            filled: (String::new(), String::new()),
            filled_for: None,
            ahead: None,
            commits: Vec::new(),
            fetching: false,
            ticket: crate::view_jobs::ticket(),
            saving: None,
            notice: None,
            rows: Vec::new(),
            picks: Vec::new(),
            mode_hits: Vec::new(),
        }
    }

    /// The fields Tab walks in this mode.
    fn fields(&self) -> &'static [CreateField] {
        match self.mode {
            CreateMode::Feature => &CreateField::FEATURE,
            CreateMode::Promote => &CreateField::PROMOTE,
        }
    }

    /// The field after `field` (or the one before), round either end.
    pub fn step(&self, field: CreateField, forward: bool) -> CreateField {
        let order = self.fields();
        let n = order.len();
        let at = order.iter().position(|f| *f == field).unwrap_or(0);
        order[if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        }]
    }

    /// The Promote mode's pair — the base into the default — once both are
    /// known and are two branches.
    pub fn promote_pair(&self) -> Option<(String, String)> {
        let (base, default) = (self.base.as_ref()?, self.default.as_ref()?);
        (base != default).then(|| (base.clone(), default.clone()))
    }

    /// The promotion already open for the pair, once GitHub has said.
    pub fn open_promotion(&self) -> Option<&promote::OpenPromotion> {
        match &self.promotion {
            Some(Ok(p)) => p.open.as_ref(),
            _ => None,
        }
    }

    /// The text field under the caret; None on the mode row and the draft
    /// box.
    fn input_mut(&mut self) -> Option<&mut TextInput> {
        match self.field {
            CreateField::From => Some(&mut self.from),
            CreateField::Into => Some(&mut self.into),
            CreateField::Title => Some(&mut self.title),
            CreateField::Body => Some(&mut self.body),
            CreateField::Mode | CreateField::Draft => None,
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

    /// Text about to land in the branch field with the caret: the first
    /// of it since the caret came replaces the branch the field held — a
    /// search for another, not an edit of that one's name. Backspace and
    /// the arrows still edit the held name, as in any field.
    fn clear_held_branch(&mut self) {
        if self.field.is_branch() && !self.typed {
            if let Some(input) = self.input_mut() {
                input.clear();
            }
        }
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
        self.focus(self.step(self.field, true));
    }

    /// The form in `mode`. What the title and description still hold of
    /// the last fill follows the new mode's ([`refill`]).
    fn set_mode(&mut self, mode: CreateMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.notice = None;
        self.filled_for = None;
        if !self.fields().contains(&self.field) {
            self.field = CreateField::Mode;
        }
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

/// `⌘N`: the create form for the modal's project, the caret on the
/// title, the branches and the fill read underneath.
pub(crate) fn open_create(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    if !dir.is_dir() {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "repo path missing on disk: {}",
            dir.display()
        )));
        return;
    }
    let from = default_head(app, &project);
    let config = crate::config::Config::load();
    let mut form = CreateForm::new(project, dir.clone(), from);
    if form.from.is_empty() {
        form.field = CreateField::From;
    }
    let ticket = form.ticket;
    let tx = app.pr_actions_tx.clone();
    form.fetching = tx.is_some();
    put_form(app, PrForm::Create(form));
    if let Some(tx) = tx {
        let base_setting = config.project(&dir).base_branch;
        let quit = app.git_sync.quit.clone();
        // What the repo knows now, at once; then again once origin is
        // fetched, so a branch moved on there — a `dev` merged from
        // elsewhere — is counted as it is, not as this clone last saw it.
        tokio::task::spawn_blocking(move || {
            let send = |fetched: bool| {
                let (heads, bases, base) = read_branches(&dir, &base_setting);
                let _ = tx.send(Answer::Branches {
                    ticket,
                    heads,
                    bases,
                    base,
                    default: default_branch(&dir),
                    fetched,
                });
            };
            send(false);
            let args = ["fetch", "--quiet", "origin"];
            let _ = crate::git_proc::remote_git(&dir, &args, crate::git_proc::FETCH_TIMEOUT, &quit);
            send(true);
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

/// The repo's default branch, by name: what `origin/HEAD` points at here,
/// else what GitHub says. None when neither can.
fn default_branch(dir: &Path) -> Option<String> {
    let local = crate::git_diff::run_git(
        dir,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .filter(|out| out.status.success())
    .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    .and_then(|name| name.strip_prefix("origin/").map(str::to_string))
    .filter(|name| !name.is_empty());
    local.or_else(|| {
        let args = [
            "repo",
            "view",
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ];
        let out = std::process::Command::new(gh_program())
            .args(args)
            .current_dir(dir)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())?;
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!name.is_empty()).then_some(name)
    })
}

/// Fill the title and description for the form's mode: the Feature
/// mode's commits between its pair of branches, the Promote mode's list
/// of what goes out — each asked for once per pair.
fn refill(app: &mut App) {
    match create_form(app).map(|form| form.mode) {
        Some(CreateMode::Feature) => request_fill(app),
        Some(CreateMode::Promote) => request_promotion(app),
        None => {}
    }
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

/// The Promote mode's list for its pair, asked for once per pair — and,
/// already in, written into the title and description at once.
fn request_promotion(app: &mut App) {
    let tx = app.pr_actions_tx.clone();
    let Some((form, linear)) = create_form_and_linear(app) else {
        return;
    };
    let Some(pair) = form.promote_pair() else {
        return;
    };
    if form.promotion_for.as_ref() == Some(&pair) {
        fill_promotion(form, linear);
        return;
    }
    form.promotion_for = Some(pair.clone());
    form.promotion = None;
    let (dir, ticket) = (form.dir.clone(), form.ticket);
    let Some(tx) = tx else {
        return;
    };
    tokio::spawn(async move {
        let result = promote::read(&dir, &pair.0, &pair.1).await;
        let _ = tx.send(Answer::Promotion {
            ticket,
            pair,
            result,
        });
    });
}

/// The Promote mode's title and description, written from its list where
/// they still hold the last fill — never over what was typed — each pull
/// request's Linear issues read off `linear` on the way.
fn fill_promotion(form: &mut CreateForm, linear: Option<&crate::linear::LinearList>) {
    let Some(Ok(promotion)) = &mut form.promotion else {
        return;
    };
    for pr in &mut promotion.prs {
        if pr.linear.is_empty() {
            pr.linear = promote::linear_ids(linear, pr);
        }
    }
    let (title, body) = promote::fill(&promotion.prs, &promote::today());
    put_fill(form, title, body);
}

/// `title` and `body` into the form where it still holds what the last
/// fill wrote.
fn put_fill(form: &mut CreateForm, title: String, body: String) {
    if form.title.as_str() == form.filled.0 {
        form.title.set_text(title.clone());
    }
    if form.body.as_str() == form.filled.1 {
        form.body.set_text(body.clone());
    }
    form.filled = (title, body);
}

/// What `from` has that `into` does not: how many commits, merges and
/// all — what the pull request would carry — and the newest
/// [`FILL_SUBJECTS`] that are not merges, newest first: each its short
/// sha and subject, and its body only when it is the one commit, whose
/// whole message is the description. `into` is origin's when origin has
/// it; `from` is whichever of the two copies the pull request would carry
/// ([`head_ref`]). None when git cannot say — a branch it does not know.
fn read_commits(dir: &Path, from: &str, into: &str) -> Option<Fill> {
    let base = crate::commit_list::branch_ref(dir, into).unwrap_or_else(|| into.to_string());
    let head = head_ref(dir, from).unwrap_or_else(|| from.to_string());
    let range = format!("{base}..{head}");
    let count = crate::git_diff::run_git(dir, &["rev-list", "--count", &range]).ok()?;
    if !count.status.success() {
        return None;
    }
    let ahead = String::from_utf8_lossy(&count.stdout).trim().parse().ok()?;
    let format = if ahead == 1 {
        "--format=%h%x1f%s%x1f%b%x1e"
    } else {
        "--format=%h%x1f%s%x1f%x1e"
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

/// The copy of `from` a pull request from it carries: origin's when the
/// local branch is missing or has nothing origin's lacks — a `dev` left
/// behind while origin's moved on is not what goes up — else the local
/// branch, which the create pushes first. None when git knows neither.
fn head_ref(dir: &Path, from: &str) -> Option<String> {
    let local = format!("refs/heads/{from}");
    let remote = format!("refs/remotes/origin/{from}");
    let has = |rev: &str| crate::commit_list::has_commit(dir, rev);
    match (has(&local), has(&remote)) {
        (true, true) if contained(dir, &local, &remote) => Some(remote),
        (true, _) => Some(local),
        (false, true) => Some(remote),
        (false, false) => None,
    }
}

/// Whether every commit of `tip` is already in `into`.
fn contained(dir: &Path, tip: &str, into: &str) -> bool {
    crate::git_diff::run_git(dir, &["merge-base", "--is-ancestor", tip, into])
        .is_ok_and(|out| out.status.success())
}

/// What a fill is made of ([`read_commits`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fill {
    /// Commits From has that Into does not, merges and all.
    pub ahead: usize,
    /// The newest of them that are not merges, oldest first.
    pub commits: Vec<FillCommit>,
}

/// One commit of a [`Fill`]: its short sha, subject and body (the body
/// read only for a lone commit).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FillCommit {
    pub sha: String,
    pub subject: String,
    pub body: String,
}

/// `%h%x1f%s%x1f%b%x1e` records: short sha, subject, then body.
fn parse_commits(text: &str) -> Vec<FillCommit> {
    text.split('\x1e')
        .filter_map(|record| {
            let mut parts = record.splitn(3, '\x1f');
            let sha = parts.next()?.trim().to_string();
            let subject = parts.next()?.trim().to_string();
            let body = parts.next().unwrap_or_default().trim().to_string();
            (!subject.is_empty()).then_some(FillCommit { sha, subject, body })
        })
        .collect()
}

/// The title and description `gh pr create --fill` would write: one
/// commit's subject and body; for several, the branch's name made words
/// and a list of their subjects — ending `- …` when there were more than
/// [`FILL_SUBJECTS`] to list.
pub fn fill_text(from: &str, commits: &[FillCommit]) -> (String, String) {
    match commits {
        [] => (words_of(from), String::new()),
        [one] => (one.subject.clone(), one.body.clone()),
        many => {
            let mut lines: Vec<String> = many.iter().map(|c| format!("- {}", c.subject)).collect();
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
    if create_form(app).is_some_and(|form| form.mode == CreateMode::Promote) {
        submit_promotion(app);
        return;
    }
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
    } else if form.ahead == Some(0) && form.fetching {
        Some("fetching origin to check — try again in a moment".into())
    } else if form.ahead == Some(0) && !form.draft {
        Some(format!(
            "{from} has no commits that {into} doesn't — a draft can open on an empty one"
        ))
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
    let empty = form.ahead == Some(0);
    tokio::spawn(async move {
        let result = create(&dir, &from, &into, &title, &body, draft, empty).await;
        let _ = tx.send(Answer::Created {
            project,
            ticket,
            result,
        });
    });
}

/// Push `from` to origin and open the pull request: its URL, or why not.
/// `empty`, From has nothing Into lacks — which GitHub refuses, draft or
/// not — so an empty commit titled as the pull request goes on it first.
async fn create(
    dir: &Path,
    from: &str,
    into: &str,
    title: &str,
    body: &str,
    draft: bool,
    empty: bool,
) -> Result<String, String> {
    if empty {
        let (dir, from, title) = (dir.to_path_buf(), from.to_string(), title.to_string());
        tokio::task::spawn_blocking(move || commit_empty(&dir, &from, &title))
            .await
            .map_err(|e| e.to_string())??;
    }
    // Origin's copy already holding all of the local one, there is nothing
    // to push — and a push of a branch behind it would be refused.
    let carried = {
        let (dir, from) = (dir.to_path_buf(), from.to_string());
        tokio::task::spawn_blocking(move || head_ref(&dir, &from))
            .await
            .ok()
            .flatten()
    };
    if !carried.is_some_and(|head| head.starts_with("refs/remotes/")) {
        push_branch(dir, from).await?;
    }
    gh_create(dir, from, into, title, body, draft).await
}

/// Enter in the Promote mode: the base into the default, as origin has
/// them — nothing pushed — or, a promotion already open, its title and
/// description put to what the form holds.
fn submit_promotion(app: &mut App) {
    let tx = app.pr_actions_tx.clone();
    let Some(form) = create_form(app) else {
        return;
    };
    if form.saving.is_some() {
        return;
    }
    let pair = form.promote_pair();
    let title = form.title.trim().to_string();
    let ahead = match &form.promotion {
        Some(Ok(p)) => p.ahead,
        _ => None,
    };
    form.notice = match &pair {
        None if form.base.is_none() || form.default.is_none() => {
            Some("reading the branches — try again in a moment".into())
        }
        None => Some(
            "the project's base is the default branch — set another in Settings → Projects".into(),
        ),
        Some(_) if form.promotion.is_none() => {
            Some("reading what goes out — try again in a moment".into())
        }
        Some((base, default)) if ahead == Some(0) => {
            Some(format!("{base} has nothing {default} doesn't"))
        }
        Some(_) if title.is_empty() => Some("the pull request needs a title".into()),
        Some(_) => None,
    };
    let (Some((base, default)), None, Some(tx)) = (pair, &form.notice, tx) else {
        return;
    };
    let (project, dir, ticket) = (form.project.clone(), form.dir.clone(), form.ticket);
    let (body, draft) = (form.body.as_str().to_string(), form.draft);
    let open = form.open_promotion().cloned();
    form.saving = Some(match &open {
        Some(open) => format!("updating #{}…", open.number),
        None => format!("opening {base} → {default}…"),
    });
    tokio::spawn(async move {
        let answer = match open {
            Some(open) => {
                let n = open.number.to_string();
                let edit = gh(
                    &dir,
                    &["pr", "edit", &n, "--title", &title, "--body-file", "-"],
                );
                Answer::Updated {
                    project,
                    ticket,
                    number: open.number,
                    url: open.url,
                    result: run_piped(edit, &body, REQUEST_TIMEOUT).await.map(|_| ()),
                }
            }
            None => Answer::Created {
                project,
                ticket,
                result: gh_create(&dir, &base, &default, &title, &body, draft).await,
            },
        };
        let _ = tx.send(answer);
    });
}

/// `gh pr create` from `from` into `into`, as origin has them: the new
/// pull request's URL, or why not.
async fn gh_create(
    dir: &Path,
    from: &str,
    into: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<String, String> {
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

/// An empty commit on the local branch `branch`, messaged `message`, made
/// without the index or any worktree: one with the branch checked out and
/// edits in it keeps them as they were. A branch only origin has gets its
/// local copy here; a local copy behind origin's is not moved under
/// whoever has it checked out.
fn commit_empty(dir: &Path, branch: &str, message: &str) -> Result<(), String> {
    let git = |args: &[&str]| crate::remote::run_git(dir, args).map(|out| out.trim().to_string());
    let local = format!("refs/heads/{branch}");
    let head = head_ref(dir, branch).ok_or_else(|| format!("git doesn't know {branch}"))?;
    let old = git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{local}^{{commit}}"),
    ])
    .ok();
    let tip = git(&["rev-parse", "--verify", &format!("{head}^{{commit}}")])?;
    if old.as_ref().is_some_and(|old| *old != tip) {
        return Err(format!("{branch} is behind origin's — pull it first"));
    }
    let tree = format!("{tip}^{{tree}}");
    let new = git(&["commit-tree", &tree, "-p", &tip, "-m", message])?;
    // Only from where it was read: a commit landing meanwhile is not lost.
    let was = old.unwrap_or_else(|| "0".repeat(tip.len()));
    git(&["update-ref", &local, &new, &was]).map(|_| ())
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
    Bypass,
}

impl MergeRow {
    const ORDER: [MergeRow; 4] = [
        MergeRow::Method,
        MergeRow::DeleteBranch,
        MergeRow::Auto,
        MergeRow::Bypass,
    ];

    fn step(self, down: bool) -> Self {
        step_clamped(&Self::ORDER, self, down)
    }
}

/// The row after (`down`) or before `at` in a form's `order`, staying put
/// at either end — ↑/↓ through a form's rows.
pub(crate) fn step_clamped<T: Copy + PartialEq>(order: &[T], at: T, down: bool) -> T {
    let i = order.iter().position(|r| *r == at).unwrap_or(0);
    let next = if down {
        (i + 1).min(order.len() - 1)
    } else {
        i.saturating_sub(1)
    };
    order[next]
}

/// `⌘X`: the merge of the pull request under the cursor.
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
    /// The repo deletes merged branches itself.
    pub auto_delete: bool,
    /// Why the branch can't go with it, once GitHub has said
    /// ([`long_lived`]): the box stays off.
    pub stays: Option<String>,
    /// `--auto`: GitHub merges once the checks and reviews it requires
    /// are in.
    pub auto: bool,
    /// `--admin`: merge now past the branch's rules — a required review
    /// included — as only an admin on its bypass list can. Never with
    /// `auto`.
    pub bypass: bool,
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
    /// request's row and, once it has landed, its body; where it stands is
    /// `status`, `App::prs`'s word.
    fn apply_detail(&mut self, pr: &OpenPr, status: &PrStatus, detail: Option<&PrDetail>) {
        self.pending = detail.is_none();
        self.branch = deletable_branch(pr, detail);
        self.base = detail.map(|d| d.base.clone()).unwrap_or_default();
        self.warnings = merge_warnings(status, detail);
        // The box starts ticked wherever the branch is ours to delete.
        self.delete_branch = self.branch.is_some();
    }
}

/// What stands in the way of merging, as GitHub said it last: the draft
/// flag, conflicts and checks from `App::prs` — the newest answer that
/// knew, so a page the cache hydrated never freezes a warning a later
/// list has cleared — and the review from the body.
fn merge_warnings(status: &PrStatus, detail: Option<&PrDetail>) -> Vec<String> {
    let mut out = Vec::new();
    let health = status.health;
    if status.is_draft() {
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

/// `⌘X`: the merge form for the pull request under the cursor, on
/// the method this project last merged by, the branch to be deleted, and
/// the repo asked which methods it allows.
pub(crate) fn open_merge(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = crate::pr_modal::selected_open_pr(app) else {
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
        method: config.merge_method(&dir),
        allowed: MergeMethod::ALL.to_vec(),
        delete_branch: false,
        auto_delete: false,
        stays: None,
        auto: false,
        bypass: false,
        row: MergeRow::Method,
        warnings: Vec::new(),
        ticket: crate::view_jobs::ticket(),
        saving: None,
        notice: None,
        rows: Vec::new(),
    };
    form.apply_detail(
        &pr,
        &app.prs.status_or_open(&pr.url),
        app.pr_detail.get(&pr.url),
    );
    let ticket = form.ticket;
    put_form(app, PrForm::Merge(form));
    ask_long_lived(app);
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

/// A pull request's body landed: a merge or close form opened on it
/// before it did takes its branch and its base from it — and a merge form,
/// what stands in the way. A merge form opened on a body already here (the
/// cache's, say) keeps its branch and its ticks, but its warnings follow
/// every body that lands: the one it opened on may be hours behind.
pub(crate) fn detail_landed(app: &mut App, url: &str) {
    let waiting = match form(app) {
        Some(PrForm::Merge(f)) => f.url == url,
        Some(PrForm::Close(f)) => f.pending && f.url == url,
        _ => false,
    };
    if !waiting {
        return;
    }
    let Some(pr) = crate::pr_modal::selected_pr(app).filter(|pr| pr.url == url) else {
        return;
    };
    let Some(detail) = app.pr_detail.get(url).cloned() else {
        return;
    };
    let status = app.prs.status_or_open(url);
    match form(app) {
        Some(PrForm::Merge(form)) if form.pending => {
            form.apply_detail(&pr, &status, Some(&detail));
            ask_long_lived(app);
        }
        Some(PrForm::Merge(form)) => form.warnings = merge_warnings(&status, Some(&detail)),
        Some(PrForm::Close(form)) => {
            form.apply_detail(&pr, Some(&detail));
            ask_long_lived(app);
        }
        _ => {}
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
    let (method, auto, bypass) = (form.method, form.auto, form.bypass);
    // A repo that deletes merged branches does it itself.
    let delete = (form.delete_branch && !form.auto_delete)
        .then(|| form.branch.clone())
        .flatten();
    let base = form.base.clone();
    tokio::spawn(async move {
        let result = merge(&dir, number, method, auto, bypass, &base, delete.as_deref())
            .await
            .map(|fate| merged_message(number, &base, method, auto, &fate));
        let landed = (!auto).then_some((number, base));
        let _ = tx.send(Answer::Merged {
            project,
            ticket,
            result,
            landed,
            method: (dir, method),
        });
    });
}

/// Merge pull request `number` into `base` — or switch auto-merge on for
/// it — and delete `branch` on origin once it is merged
/// ([`retire_merged_branch`]). What became of it.
async fn merge(
    dir: &Path,
    number: u64,
    method: MergeMethod,
    auto: bool,
    bypass: bool,
    base: &str,
    branch: Option<&str>,
) -> Result<BranchFate, String> {
    let number = number.to_string();
    let mut args = vec!["pr", "merge", number.as_str(), method.flag()];
    if auto {
        args.push("--auto");
    } else if bypass {
        args.push("--admin");
    }
    run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await?;
    // Auto-merge lands later, on GitHub, with nothing here to delete the
    // branch after it: only a repo that deletes merged branches does then.
    let Some(branch) = branch.filter(|_| !auto) else {
        return Ok(BranchFate::Kept);
    };
    let setting = crate::config::Config::load().project(dir).base_branch;
    Ok(retire_merged_branch(dir, branch, base, &setting).await)
}

/// Delete `branch`, merged into `base`, from GitHub, unless it is
/// long-lived. GitHub moves the pull requests based on a merged branch onto
/// its base only when it deletes the branch itself; one deleted through the
/// API closes them. So they are moved here first, and go back if the branch
/// turns out to stay.
async fn retire_merged_branch(dir: &Path, branch: &str, base: &str, setting: &str) -> BranchFate {
    if let Some(guarded) = guard(dir, branch, setting).await {
        return guarded;
    }
    let Some(stacked) = stacked_on(dir, branch).await else {
        return BranchFate::Guarded(UNLISTED.into());
    };
    let (mut moved, mut held) = (Vec::new(), Vec::new());
    for pr in stacked {
        // One that won't move keeps the branch: the rest needn't try.
        if !base.is_empty() && held.is_empty() && retarget(dir, pr, base).await {
            moved.push(pr);
        } else {
            held.push(pr);
        }
    }
    if held.is_empty() && delete_branch(dir, branch).await {
        return BranchFate::Deleted { moved };
    }
    let mut stranded = Vec::new();
    for pr in moved {
        if !retarget(dir, pr, branch).await {
            stranded.push(pr);
        }
    }
    if held.is_empty() {
        BranchFate::Refused { stranded }
    } else {
        BranchFate::Held {
            stacked: held,
            stranded,
        }
    }
}

/// Why [`stacked_on`] couldn't help: the branch stays rather than guess.
const UNLISTED: &str = "GitHub would not say what is based on it";

/// The fate of a `branch` that must stay: long-lived, or nobody could say
/// it isn't. None when it may go.
async fn guard(dir: &Path, branch: &str, setting: &str) -> Option<BranchFate> {
    match long_lived(dir, branch, setting).await {
        Ok(None) => None,
        Ok(Some(why)) => Some(BranchFate::Guarded(why)),
        Err(_) => Some(BranchFate::Guarded(
            "GitHub would not say whether it is protected".into(),
        )),
    }
}

/// Why `branch` outlives any pull request merged from it: it is what other
/// work is based on, so "stacked on it" means every open pull request of
/// the repo's and deleting it is never what merging it meant. `dev` merged
/// into `main` is the shape. `setting` is the project's **Base branch**,
/// which names such a branch on a repo that doesn't protect it.
/// `Err` when GitHub could not say.
async fn long_lived(dir: &Path, branch: &str, setting: &str) -> Result<Option<String>, String> {
    if branch == crate::commit_list::base_name(setting) {
        return Ok(Some(format!("{branch} is what new worktrees start from")));
    }
    let view = gh(
        dir,
        &[
            "repo",
            "view",
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ],
    );
    if run_piped(view, "", REQUEST_TIMEOUT).await?.trim() == branch {
        return Ok(Some(format!("{branch} is the repo's default branch")));
    }
    let path = format!("repos/{{owner}}/{{repo}}/branches/{branch}");
    let read = gh(dir, &["api", &path, "--jq", ".protected"]);
    match run_piped(read, "", REQUEST_TIMEOUT).await?.trim() {
        "true" => Ok(Some(format!("{branch} is protected on GitHub"))),
        "false" => Ok(None),
        other => Err(other.to_string()),
    }
}

/// Ask GitHub whether the form's branch is long-lived, for the form to say
/// so before Enter. Not the safeguard — [`guard`] is, at the moment it
/// counts — so a question nobody could answer changes nothing here.
fn ask_long_lived(app: &mut App) {
    let Some(tx) = app.pr_actions_tx.clone() else {
        return;
    };
    let (dir, branch, ticket) = match form(app) {
        Some(PrForm::Merge(f)) => (f.dir.clone(), f.branch.clone(), f.ticket),
        Some(PrForm::Close(f)) => (f.dir.clone(), f.branch.clone(), f.ticket),
        _ => return,
    };
    let Some(branch) = branch else {
        return;
    };
    tokio::spawn(async move {
        let setting = crate::config::Config::load().project(&dir).base_branch;
        if let Ok(Some(why)) = long_lived(&dir, &branch, &setting).await {
            let _ = tx.send(Answer::BranchStays { ticket, why });
        }
    });
}

/// What became of a merged or closed pull request's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BranchFate {
    /// Not asked to go.
    Kept,
    /// Asked to go, but it must not: why. Nothing was touched.
    Guarded(String),
    /// Deleted on GitHub, these stacked on it moved onto its base first.
    Deleted { moved: Vec<u64> },
    /// Left on GitHub: `stacked` are based on it, and deleting it would
    /// have GitHub close them. `stranded` had moved, and would not go back.
    Held {
        stacked: Vec<u64>,
        stranded: Vec<u64>,
    },
    /// GitHub would not delete it. What had been moved off it went back,
    /// but for `stranded`.
    Refused { stranded: Vec<u64> },
}

impl BranchFate {
    /// What the flash adds for it, after the merge or the close.
    fn suffix(&self, base: &str) -> String {
        match self {
            Self::Kept => String::new(),
            Self::Deleted { moved } if moved.is_empty() => ", branch deleted".into(),
            Self::Deleted { moved } => {
                format!(", branch deleted; {} moved onto {base}", pr_list(moved))
            }
            Self::Guarded(why) => format!(", branch kept: {why}"),
            Self::Held { stacked, stranded } => format!(
                ", branch kept: {} stacked on it{}",
                pr_list(stacked),
                left_on(stranded, base)
            ),
            Self::Refused { stranded } => format!(
                ", branch kept: GitHub would not delete it{}",
                left_on(stranded, base)
            ),
        }
    }
}

/// `; #12 left on main`, for what moved and would not go back.
fn left_on(stranded: &[u64], base: &str) -> String {
    if stranded.is_empty() {
        return String::new();
    }
    format!("; {} left on {base}", pr_list(stranded))
}

/// `#12, #14`.
fn pr_list(numbers: &[u64]) -> String {
    numbers
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The open pull requests based on `branch` — stacked on it. None when
/// GitHub could not say, which leaves the branch where it is.
async fn stacked_on(dir: &Path, branch: &str) -> Option<Vec<u64>> {
    let list = gh(
        dir,
        &[
            "pr", "list", "--base", branch, "--state", "open", "--limit", "100", "--json", "number",
        ],
    );
    let out = run_piped(list, "", REQUEST_TIMEOUT).await.ok()?;
    parse_numbers(&out)
}

/// `gh pr list --json number`'s answer, as numbers.
fn parse_numbers(json: &str) -> Option<Vec<u64>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.as_array()?
        .iter()
        .map(|pr| pr.get("number")?.as_u64())
        .collect()
}

/// Base pull request `number` on `base`. Whether it moved.
async fn retarget(dir: &Path, number: u64, base: &str) -> bool {
    let number = number.to_string();
    let edit = gh(dir, &["pr", "edit", &number, "--base", base]);
    run_piped(edit, "", REQUEST_TIMEOUT).await.is_ok()
}

/// Delete `branch` on GitHub — only there: a checkout of it here is left
/// alone. Whether it went.
async fn delete_branch(dir: &Path, branch: &str) -> bool {
    let path = format!("repos/{{owner}}/{{repo}}/git/refs/heads/{branch}");
    let delete = gh(dir, &["api", "-X", "DELETE", &path]);
    run_piped(delete, "", REQUEST_TIMEOUT).await.is_ok()
}

/// The flash a merge leaves.
fn merged_message(
    number: u64,
    base: &str,
    method: MergeMethod,
    auto: bool,
    fate: &BranchFate,
) -> String {
    let into = if base.is_empty() {
        String::new()
    } else {
        format!(" into {base}")
    };
    let gone = fate.suffix(base);
    if auto {
        format!(
            "#{number} merges{into} once GitHub's requirements are met ({}){gone}",
            method.as_str()
        )
    } else {
        format!("merged #{number}{into} ({}){gone}", method.as_str())
    }
}

// ---- closing ----

/// The close form's rows, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRow {
    DeleteBranch,
    Comment,
}

impl CloseRow {
    /// Two rows: either way round lands on the other one.
    fn other(self) -> Self {
        match self {
            CloseRow::DeleteBranch => CloseRow::Comment,
            CloseRow::Comment => CloseRow::DeleteBranch,
        }
    }
}

/// `⌘W`: the pull request under the cursor closed without merging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseForm {
    pub project: ProjectId,
    pub dir: PathBuf,
    pub number: u64,
    pub url: String,
    pub title: String,
    /// The branch it would have merged into, and the one it comes from —
    /// the latter only when it is a branch of this repo ([`deletable_branch`]).
    pub base: String,
    pub branch: Option<String>,
    /// The row's own name for the branch, while the body has not landed.
    pub head: String,
    /// The pull request's body had not landed when the form opened: the
    /// branch and the base are filled in when it does ([`detail_landed`]).
    pub pending: bool,
    pub delete_branch: bool,
    /// Why the branch can't go with it, once GitHub has said
    /// ([`long_lived`]): the box stays off.
    pub stays: Option<String>,
    /// Left on the pull request as it closes, when anything is written.
    pub comment: TextInput,
    pub row: CloseRow,
    pub ticket: u64,
    pub saving: Option<String>,
    pub notice: Option<String>,
    /// As of the last draw: each row's rect, for the mouse.
    pub rows: Vec<(Rect, CloseRow)>,
}

impl CloseForm {
    /// Take the branch and the base from the pull request's row and, once
    /// it has landed, its body. A tick stays only on a branch that is ours.
    fn apply_detail(&mut self, pr: &OpenPr, detail: Option<&PrDetail>) {
        self.pending = detail.is_none();
        self.branch = deletable_branch(pr, detail);
        self.base = detail.map(|d| d.base.clone()).unwrap_or_default();
        self.delete_branch &= self.branch.is_some();
    }

    fn toggle_delete(&mut self) {
        if self.branch.is_some() && self.stays.is_none() {
            self.delete_branch = !self.delete_branch;
            self.notice = None;
        }
    }
}

/// `⌘W`: the close form for the pull request under the cursor, the
/// caret in its comment and the branch kept unless it is ticked.
pub(crate) fn open_close(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = crate::pr_modal::selected_open_pr(app) else {
        return;
    };
    let mut form = CloseForm {
        project,
        dir,
        number: pr.number,
        url: pr.url.clone(),
        title: pr.title.clone(),
        base: String::new(),
        branch: None,
        head: pr.head.clone(),
        pending: true,
        delete_branch: false,
        stays: None,
        comment: TextInput::multiline(),
        row: CloseRow::Comment,
        ticket: crate::view_jobs::ticket(),
        saving: None,
        notice: None,
        rows: Vec::new(),
    };
    form.apply_detail(&pr, app.pr_detail.get(&pr.url));
    put_form(app, PrForm::Close(form));
    ask_long_lived(app);
    app.dirty = true;
}

/// Enter on the close form: `gh pr close` off the loop, the form held
/// until the answer lands.
fn submit_close(app: &mut App) {
    let tx = app.pr_actions_tx.clone();
    let Some(form) = close_pr_form(app) else {
        return;
    };
    if form.saving.is_some() {
        return;
    }
    let Some(tx) = tx else {
        return;
    };
    form.notice = None;
    form.saving = Some(format!("closing #{}…", form.number));
    let (project, dir, ticket, number) = (
        form.project.clone(),
        form.dir.clone(),
        form.ticket,
        form.number,
    );
    let comment = form.comment.trim().to_string();
    let delete = form.delete_branch.then(|| form.branch.clone()).flatten();
    tokio::spawn(async move {
        let result = close(&dir, number, &comment, delete.as_deref())
            .await
            .map(|fate| closed_message(number, &fate));
        let _ = tx.send(Answer::Closed {
            project,
            ticket,
            result,
        });
    });
}

/// Close pull request `number` — `comment` left on it first, when there
/// is one — and delete `branch` on GitHub once it is closed, unless pull
/// requests are stacked on it. What became of it.
async fn close(
    dir: &Path,
    number: u64,
    comment: &str,
    branch: Option<&str>,
) -> Result<BranchFate, String> {
    let number = number.to_string();
    let mut args = vec!["pr", "close", number.as_str()];
    if !comment.is_empty() {
        args.extend(["--comment", comment]);
    }
    run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await?;
    let Some(branch) = branch else {
        return Ok(BranchFate::Kept);
    };
    let setting = crate::config::Config::load().project(dir).base_branch;
    Ok(retire_closed_branch(dir, branch, &setting).await)
}

/// Delete `branch`, its pull request closed unmerged, from GitHub. Nothing
/// merged, so nowhere to move what is stacked on it: the branch stays
/// rather than have GitHub close them — and stays if it is long-lived.
async fn retire_closed_branch(dir: &Path, branch: &str, setting: &str) -> BranchFate {
    if let Some(guarded) = guard(dir, branch, setting).await {
        return guarded;
    }
    match stacked_on(dir, branch).await {
        None => BranchFate::Guarded(UNLISTED.into()),
        Some(stacked) if !stacked.is_empty() => BranchFate::Held {
            stacked,
            stranded: Vec::new(),
        },
        Some(_) if delete_branch(dir, branch).await => BranchFate::Deleted { moved: Vec::new() },
        Some(_) => BranchFate::Refused {
            stranded: Vec::new(),
        },
    }
}

/// The flash a close leaves.
fn closed_message(number: u64, fate: &BranchFate) -> String {
    format!("closed #{number}{}", fate.suffix(""))
}

// ---- ready for review / draft ----

/// `⌘D`: the pull request under the cursor marked ready for review —
/// or, ready already, turned back into a draft — off the loop, the footer
/// saying so meanwhile. Whether it is a draft is `App::prs`'s word — the
/// newest answer that knew, the one the row's badge draws.
pub(crate) fn toggle_draft(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = crate::pr_modal::selected_open_pr(app) else {
        return;
    };
    let Some(tx) = app.pr_actions_tx.clone() else {
        return;
    };
    let ready = app.pr_is_draft(&pr.url);
    let number = pr.number;
    app.flash = Some(crate::flash::Flash::working(if ready {
        format!("marking #{number} ready for review…")
    } else {
        format!("turning #{number} into a draft…")
    }));
    let url = pr.url;
    tokio::spawn(async move {
        let n = number.to_string();
        let mut args = vec!["pr", "ready", n.as_str()];
        if !ready {
            args.push("--undo");
        }
        let result = run_piped(gh(&dir, &args), "", REQUEST_TIMEOUT)
            .await
            .map(|_| ());
        let _ = tx.send(Answer::Readied {
            project,
            url,
            number,
            ready,
            result,
        });
    });
    app.dirty = true;
}

// ---- running git and gh ----

/// `gh <args>` in `dir`, for [`run_piped`].
fn gh(dir: &Path, args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(gh_program());
    cmd.args(args).current_dir(dir);
    cmd
}

/// The `gh` the forms run: a stub under test, so nothing reaches GitHub.
fn gh_program() -> PathBuf {
    #[cfg(not(test))]
    let program = PathBuf::from("gh");
    #[cfg(test)]
    let program = tests::GH.with_borrow(|stub| stub.clone());
    program
}

/// `git <args>` in `dir`, DETACHED (`git_proc::detached`), for
/// [`run_piped`]: a push whose ssh wants a passphrase fails rather than
/// draw over the frame.
fn git(dir: &Path, args: &[&str]) -> tokio::process::Command {
    tokio::process::Command::from(crate::git_proc::detached(dir, args))
}

// ---- answers ----

/// What the forms' git and `gh` came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The create form's branches, the project's base to merge into, and
    /// the repo's default branch.
    Branches {
        ticket: u64,
        heads: Vec<String>,
        bases: Vec<String>,
        base: Option<String>,
        default: Option<String>,
        /// Read after origin was fetched (or the fetch gave up).
        fetched: bool,
    },
    /// The commits between a pair of branches, for the fill.
    Fill {
        ticket: u64,
        pair: (String, String),
        fill: Option<Fill>,
    },
    /// What promoting the pair — base into default — carries.
    Promotion {
        ticket: u64,
        pair: (String, String),
        result: Result<promote::Promotion, String>,
    },
    /// `gh pr create`: the new pull request's URL, or why not.
    Created {
        project: ProjectId,
        ticket: u64,
        result: Result<String, String>,
    },
    /// `gh pr edit` of a promotion already open: done, or why not.
    Updated {
        project: ProjectId,
        ticket: u64,
        number: u64,
        url: String,
        result: Result<(), String>,
    },
    /// The repo's merge methods, and whether it deletes merged branches.
    MergeOptions {
        ticket: u64,
        allowed: Vec<MergeMethod>,
        auto_delete: bool,
    },
    /// The form's branch is long-lived, and why ([`long_lived`]).
    BranchStays { ticket: u64, why: String },
    /// `gh pr merge`: what to flash, or why it refused. `landed` is the
    /// pull request's number and base when it merged now — not auto-merge,
    /// which lands later — for the MERGE CONFETTI. `method` is the repo
    /// and the method it took, for its next merge form to open on.
    Merged {
        project: ProjectId,
        ticket: u64,
        result: Result<String, String>,
        landed: Option<(u64, String)>,
        method: (PathBuf, MergeMethod),
    },
    /// `gh pr close`: what to flash, or why it refused.
    Closed {
        project: ProjectId,
        ticket: u64,
        result: Result<String, String>,
    },
    /// `gh pr ready`: pull request `number` marked `ready` for review (or
    /// turned into a draft), or why not.
    Readied {
        project: ProjectId,
        url: String,
        number: u64,
        ready: bool,
        result: Result<(), String>,
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
        _ => None,
    }
}

/// The create form, and Linear's list for its project — what the Promote
/// mode's Linear issues are read from — borrowed together.
fn create_form_and_linear(
    app: &mut App,
) -> Option<(&mut CreateForm, Option<&crate::linear::LinearList>)> {
    let Some(Overlay::PullRequests(view)) = &mut app.overlay else {
        return None;
    };
    let PrForm::Create(form) = view.form.as_deref_mut()? else {
        return None;
    };
    let linear = app.linear.get(&form.project);
    Some((form, linear))
}

fn merge_form(app: &mut App) -> Option<&mut MergeForm> {
    match form(app)? {
        PrForm::Merge(f) => Some(f),
        _ => None,
    }
}

fn close_pr_form(app: &mut App) -> Option<&mut CloseForm> {
    match form(app)? {
        PrForm::Close(f) => Some(f),
        _ => None,
    }
}

/// Close the form, the reading pane back.
fn close_form(app: &mut App) {
    if let Some(view) = modal(app) {
        view.form = None;
    }
}

/// Land one answer. A create, a merge, a close or a draft flipped is
/// flashed whether or not its form is still up — it happened either way —
/// and the project's list is asked for again; the rest only matter to the
/// form that asked.
pub(crate) fn land_answer(app: &mut App, answer: Answer) {
    match answer {
        Answer::Branches {
            ticket,
            heads,
            bases,
            base,
            default,
            fetched,
        } => land_branches(app, ticket, (heads, bases), (base, default), fetched),
        Answer::Fill { ticket, pair, fill } => land_fill(app, ticket, pair, fill),
        Answer::Promotion {
            ticket,
            pair,
            result,
        } => land_promotion(app, ticket, pair, result),
        Answer::Updated {
            project,
            ticket,
            number,
            url,
            result,
        } => match result {
            Ok(()) => {
                land_created(app, &project, ticket, url.clone());
                read_again(app, &project, url);
                app.flash = Some(crate::flash::Flash::done(format!(
                    "updated #{number}'s title and description"
                )));
            }
            Err(why) => refused(app, ticket, why, &format!("update #{number}")),
        },
        Answer::Created {
            project,
            ticket,
            result,
        } => match result {
            Ok(url) => land_created(app, &project, ticket, url),
            Err(why) => {
                refused(app, ticket, why, "open the pull request");
                // An empty commit may have gone on before the refusal:
                // counted again, the next Enter doesn't add another.
                if let Some(PrForm::Create(form)) = form_for(app, ticket) {
                    form.filled_for = None;
                    request_fill(app);
                }
            }
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
        Answer::BranchStays { ticket, why } => {
            if let Some(form) = form_for(app, ticket) {
                form.keep_branch(why);
            }
        }
        Answer::Merged {
            project,
            ticket,
            result,
            landed,
            method: (dir, method),
        } => {
            // Origin's base just moved: bring the project's root after it.
            // And the project's next merge form opens on the method this
            // one took — one gh refused teaches it nothing.
            if result.is_ok() {
                crate::config::persist(|cfg| cfg.remember_merge_method(&dir, method));
                crate::base_sync::request_now(app, &project);
                if let Some((number, base)) = &landed {
                    crate::confetti::fire(app, *number, base);
                }
            }
            land_done(app, &project, ticket, result, "merge")
        }
        Answer::Closed {
            project,
            ticket,
            result,
        } => land_done(app, &project, ticket, result, "close the pull request"),
        Answer::Readied {
            project,
            url,
            number,
            ready,
            result,
        } => land_readied(app, &project, url, number, ready, result),
    }
    app.dirty = true;
}

/// The create form's branches are read: Into takes the project's base when
/// nothing is typed there (and it is not From itself), the list under the
/// caret opens on its field's branch, and the fill is asked for — again
/// once origin is `fetched`, the pair's commits maybe moved. A form from
/// the base branch itself, its mode not picked by hand, is a promotion:
/// it goes over to the Promote mode.
fn land_branches(
    app: &mut App,
    ticket: u64,
    (heads, bases): (Vec<String>, Vec<String>),
    (base, default): (Option<String>, Option<String>),
    fetched: bool,
) {
    let Some(PrForm::Create(form)) = form_for(app, ticket) else {
        return;
    };
    if fetched {
        form.fetching = false;
        form.filled_for = None;
        form.promotion_for = None;
    }
    form.base = base.clone().or(form.base.take());
    form.default = default.or(form.default.take());
    if form.into.trim().is_empty() {
        if let Some(base) = base.filter(|b| *b != form.from.trim()) {
            form.into.set_text(base);
        }
    }
    form.heads = Some(heads.into());
    form.bases = Some(bases.into());
    let from_base = form.base.as_deref() == Some(form.from.trim());
    if !form.mode_chosen && from_base && form.promote_pair().is_some() {
        form.set_mode(CreateMode::Promote);
        form.field = CreateField::Title;
    }
    if !form.typed {
        form.focus(form.field);
    }
    refill(app);
}

/// A fill is in for the pair it was asked for: the count and the commits
/// go over the description, and the title and description take it
/// wherever they still hold the last fill — never over what was typed.
fn land_fill(app: &mut App, ticket: u64, pair: (String, String), fill: Option<Fill>) {
    let Some(PrForm::Create(form)) = form_for(app, ticket) else {
        return;
    };
    if form.filled_for.as_ref() != Some(&pair) || form.mode != CreateMode::Feature {
        return;
    }
    form.ahead = fill.as_ref().map(|fill| fill.ahead);
    form.commits = fill.map(|fill| fill.commits).unwrap_or_default();
    let (title, body) = fill_text(&pair.0, &form.commits);
    put_fill(form, title, body);
}

/// The Promote mode's list is in for the pair it was asked for: it goes
/// over the description, and fills the title and description where they
/// still hold the last fill.
fn land_promotion(
    app: &mut App,
    ticket: u64,
    pair: (String, String),
    result: Result<promote::Promotion, String>,
) {
    let Some((form, linear)) = create_form_and_linear(app) else {
        return;
    };
    if form.ticket != ticket || form.promotion_for.as_ref() != Some(&pair) {
        return;
    }
    form.promotion = Some(result);
    if form.mode == CreateMode::Promote {
        fill_promotion(form, linear);
    }
}

/// A merge or a close answered: done, the form closes onto the list —
/// asked for again, the row about to leave it — and the footer says what
/// happened; refused, the form says why (`what` it was trying to do).
fn land_done(
    app: &mut App,
    project: &ProjectId,
    ticket: u64,
    result: Result<String, String>,
    what: &str,
) {
    match result {
        Ok(said) => {
            if form_for(app, ticket).is_some() {
                close_form(app);
            }
            crate::pr_modal::request_list(app, project);
            app.flash = Some(crate::flash::Flash::done(said));
        }
        Err(why) => refused(app, ticket, why, what),
    }
}

/// Pull request `url` changed on GitHub: its body and the project's list
/// are read again, so the page and the row's badge follow. A body already
/// being read was asked before the change, so it owes a fresh read.
fn read_again(app: &mut App, project: &ProjectId, url: String) {
    app.pr_detail_inflight.want_fresh(&url);
    app.pr_detail_stale.insert(url);
    if crate::pr_modal::is_up(app) {
        crate::pr_modal::schedule_detail(app);
    }
    crate::pr_modal::request_list(app, project);
}

/// `gh pr ready` answered: the footer says which way pull request `number`
/// went, and its body and the list are read again so the page and the
/// row's badge say it too.
fn land_readied(
    app: &mut App,
    project: &ProjectId,
    url: String,
    number: u64,
    ready: bool,
    result: Result<(), String>,
) {
    app.flash = Some(match result {
        Ok(()) => {
            // GitHub has just done it: every badge says so now, and a list
            // asked before it can't put the old word back.
            app.prs.observe(
                &url,
                crate::pr_store::PrObservation::of_draft(!ready),
                crate::fetch::Asked::At(crate::fetch::now()),
            );
            read_again(app, project, url);
            crate::flash::Flash::done(if ready {
                format!("#{number} is ready for review")
            } else {
                format!("#{number} is a draft again")
            })
        }
        Err(why) => crate::flash::Flash::failed(if ready {
            format!("couldn't mark #{number} ready: {why}")
        } else {
            format!("couldn't turn #{number} into a draft: {why}")
        }),
    });
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
    app.flash = Some(crate::flash::Flash::done(if url.is_empty() {
        "opened the pull request".into()
    } else {
        format!("opened {url}")
    }));
}

/// A create, merge or close refused (`what` it was trying to do): the form that
/// sent it says why, or — closed since — the footer does.
fn refused(app: &mut App, ticket: u64, why: String, what: &str) {
    match form_for(app, ticket) {
        Some(form) => form.refused(why),
        None => {
            app.flash = Some(crate::flash::Flash::failed(format!(
                "couldn't {what}: {why}"
            )))
        }
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
    /// The create form's mode row: Feature branch or Promote.
    pub const MODE: Key = Key::new(&["left", "right", "space"], "mode").show(2);
    pub const DRAFT: Key = Key::new(&["space"], "draft");
    /// The merge form's rows.
    pub const OPTION: Key = Key::new(&["up", "down"], "option").show(2);
    pub const CHANGE: Key = Key::new(&["left", "right", "space"], "change").show(2);
    pub const MERGE: Key = Key::new(&["enter"], "merge");
    /// The close form's.
    pub const CLOSE: Key = Key::new(&["enter"], "close PR");
    pub const DELETE: Key = Key::new(&["space"], "delete branch");
    #[cfg(test)]
    pub const ALL: &[Key] = &[
        FIELD,
        PICK,
        CHOOSE,
        CREATE,
        MODE,
        DRAFT,
        OPTION,
        CHANGE,
        MERGE,
        CLOSE,
        DELETE,
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
            let create = match form.open_promotion() {
                Some(open) if form.mode == CreateMode::Promote => {
                    keys::CREATE.hint_as(format!("update #{}", open.number))
                }
                _ => keys::CREATE.hint(),
            };
            match form.field {
                field if field.is_branch() => {
                    hints.push(keys::CHOOSE.hint().kept());
                    hints.push(keys::PICK.hint());
                }
                CreateField::Mode => {
                    hints.push(create.kept());
                    hints.push(keys::MODE.hint());
                }
                CreateField::Draft => {
                    hints.push(create.kept());
                    hints.push(keys::DRAFT.hint());
                }
                CreateField::Body => {
                    hints.push(create.kept());
                    hints.push(crate::ui::task_keys::NEWLINE.hint());
                }
                _ => hints.push(create.kept()),
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
        PrForm::Close(form) => {
            let mut hints = vec![keys::CLOSE.hint().kept()];
            match form.row {
                CloseRow::DeleteBranch if form.branch.is_some() => hints.push(keys::DELETE.hint()),
                CloseRow::DeleteBranch => {}
                CloseRow::Comment => hints.push(crate::ui::task_keys::NEWLINE.hint()),
            }
            hints.push(keys::FIELD.hint());
            hints.push(Hint::new("Esc", "cancel"));
            hints
        }
    }
}

/// A key while a form is up: every key is the form's. Esc closes it — a
/// create, merge or close already sent still lands, and says so.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(form) = form(app) else {
        return;
    };
    if key.code == KeyCode::Esc {
        close_form(app);
    } else if form.saving() {
        // Nothing to change while it is on its way.
    } else {
        match form {
            PrForm::Create(_) => create_key(app, key),
            PrForm::Merge(_) => merge_key(app, key),
            PrForm::Close(_) => close_key(app, key),
        }
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
            form.focus(form.step(field, forward));
        }
        // ←/→ and Space on the mode row: the other mode.
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if field == CreateField::Mode => {
            let mode = match form.mode {
                CreateMode::Feature => CreateMode::Promote,
                CreateMode::Promote => CreateMode::Feature,
            };
            form.mode_chosen = true;
            form.set_mode(mode);
            fill = true;
        }
        KeyCode::Down | KeyCode::Up if field == CreateField::Mode => {
            form.focus(form.step(field, key.code == KeyCode::Down));
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
                form.focus(form.step(field, down));
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
            if is_char_key(&key) {
                form.clear_held_branch();
            }
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
        refill(app);
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

/// Keys on the close form: Tab between the box and the comment, Space
/// ticks the box, ↑/↓ step between them past the comment's first line, and
/// Enter closes — a line break in the comment on the NEWLINE chords.
fn close_key(app: &mut App, key: KeyEvent) {
    let Some(form) = close_pr_form(app) else {
        return;
    };
    let on_comment = form.row == CloseRow::Comment;
    match key.code {
        _ if keys::FIELD.matches(&key) => form.row = form.row.other(),
        KeyCode::Char(' ') if !on_comment => form.toggle_delete(),
        KeyCode::Down if !on_comment => form.row = CloseRow::Comment,
        KeyCode::Up if on_comment && !form.comment.handle_key(&key).consumed() => {
            form.row = CloseRow::DeleteBranch
        }
        KeyCode::Enter if !(on_comment && form.comment.takes_newline(&key)) => submit_close(app),
        _ if on_comment && form.comment.handle_key(&key).changed() => form.notice = None,
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
        MergeRow::DeleteBranch if form.branch.is_some() && form.stays.is_none() => {
            form.delete_branch = !form.delete_branch;
        }
        MergeRow::DeleteBranch => {}
        MergeRow::Auto => {
            form.auto = !form.auto;
            form.bypass &= !form.auto;
        }
        MergeRow::Bypass => {
            form.bypass = !form.bypass;
            form.auto &= !form.bypass;
        }
    }
}

/// A paste lands in the create form's field under the caret — lines kept
/// in the description, flattened anywhere else; in a branch field not yet
/// typed in, in place of the branch it held — or in the close form's
/// comment. True while a form is up.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(form) = form(app) else {
        return false;
    };
    if let PrForm::Close(form) = form {
        if form.saving.is_none() {
            form.row = CloseRow::Comment;
            form.comment.insert_str(text);
        }
        return true;
    }
    let PrForm::Create(form) = form else {
        return true;
    };
    if form.saving.is_some() {
        return true;
    }
    let field = form.field;
    form.clear_held_branch();
    if let Some(input) = form.input_mut() {
        input.insert_str(text);
        if field.is_branch() {
            form.typed_in();
        }
    }
    true
}

/// A key that types a character, rather than moving the caret or editing
/// round it.
fn is_char_key(key: &KeyEvent) -> bool {
    use crossterm::event::KeyModifiers;
    matches!(key.code, KeyCode::Char(_))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// The create form's caret to `to`, as a click on its row puts it: off a
/// branch field, the title and description fill for the branches it
/// leaves.
pub(crate) fn focus_create(app: &mut App, to: CreateField) {
    let Some(form) = create_form(app) else {
        return;
    };
    let fill = form.field.is_branch();
    form.focus(to);
    if fill {
        request_fill(app);
    }
}

/// The mouse while a form is up: a click puts the caret on a field —
/// on a branch in the list, takes it — or works a merge or close row as
/// Space does; the wheel walks a branch list. Nothing else, so a draft is
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
                if let Some(&(_, mode)) = form.mode_hits.iter().find(|(r, _)| r.contains(at)) {
                    form.mode_chosen = true;
                    form.set_mode(mode);
                    form.focus(CreateField::Mode);
                    fill = true;
                } else if let Some(&(_, pick)) = form.picks.iter().find(|(r, _)| r.contains(at)) {
                    form.pick = pick;
                    form.choose_pick();
                    fill = true;
                } else if let Some(&(_, to)) = form.rows.iter().find(|(r, _)| r.contains(at)) {
                    if to == CreateField::Draft && field == CreateField::Draft {
                        form.draft = !form.draft;
                    }
                    focus_create(app, to);
                    app.dirty = true;
                    return;
                }
            }
            _ => {}
        }
        if fill {
            refill(app);
        }
    } else if let Some(form) = merge_form(app) {
        if !clicked {
            return;
        }
        if let Some(&(_, row)) = form.rows.iter().find(|(r, _)| r.contains(at)) {
            form.row = row;
            change_merge_row(form, true);
        }
    } else if let Some(form) = close_pr_form(app) {
        if !clicked {
            return;
        }
        if let Some(&(_, row)) = form.rows.iter().find(|(r, _)| r.contains(at)) {
            if row == CloseRow::DeleteBranch {
                form.toggle_delete();
            }
            form.row = row;
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
    mode_hits: Vec<(Rect, CreateMode)>,
    merge_rows: Vec<(Rect, MergeRow)>,
    close_rows: Vec<(Rect, CloseRow)>,
    picks: Vec<(Rect, usize)>,
    /// The view the description — or the close form's comment — was
    /// drawn with.
    body_view: Option<TextView>,
}

/// Put what the draw learned back on the live form (the draw works on a
/// clone).
pub(crate) fn write_back(form: &mut PrForm, drawn: Drawn) {
    match form {
        PrForm::Create(form) => {
            form.rows = drawn.create_rows;
            form.mode_hits = drawn.mode_hits;
            form.picks = drawn.picks;
            if let Some(view) = drawn.body_view {
                form.body.set_view(view);
            }
        }
        PrForm::Merge(form) => form.rows = drawn.merge_rows,
        PrForm::Close(form) => {
            form.rows = drawn.close_rows;
            if let Some(view) = drawn.body_view {
                form.comment.set_view(view);
            }
        }
    }
}

/// Draw `form` into `area`, the reading pane's place.
pub(crate) fn draw(f: &mut Frame, area: Rect, form: &PrForm, focused: bool, th: Theme) -> Drawn {
    match form {
        PrForm::Create(form) => draw_create(f, area, form, focused, th),
        PrForm::Merge(form) => draw_merge(f, area, form, focused, th),
        PrForm::Close(form) => draw_close(f, area, form, focused, th),
    }
}

/// The Delete row's note for a branch that can't go: [`MergeForm::stays`].
fn stays_note(why: &str) -> String {
    format!("  {why} — it stays")
}

/// A `[x]` box.
pub(crate) fn check(on: bool) -> &'static str {
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
    let text = Style::default().fg(th.text);
    let caret = |field: CreateField| focused && form.field == field && form.saving.is_none();
    let width = usize::from(inner.width);
    let promote = form.mode == CreateMode::Promote;
    let pair = form.promote_pair();
    /// One line of the form into `rect`, kept in `rows` as `field`'s.
    fn put(
        f: &mut Frame,
        rows: &mut Vec<(Rect, CreateField)>,
        rect: Rect,
        spans: Vec<Span<'static>>,
        field: Option<CreateField>,
    ) {
        let line = crate::pr_preview::fit(spans, usize::from(rect.width));
        f.render_widget(Paragraph::new(line), rect);
        if let Some(field) = field {
            rows.push((rect, field));
        }
    }

    // The modes along the top, the one the form is in lit.
    if let Some(row) = row_rect(inner, 0) {
        let promote_label = match &pair {
            Some((base, default)) => format!(" Promote {base} → {default} "),
            None => " Promote ".to_string(),
        };
        let mut spans = vec![Span::raw(INDENT)];
        let mut x = row.x + INDENT.chars().count() as u16;
        let mut hits = Vec::new();
        for (mode, label) in [
            (CreateMode::Feature, " Feature branch ".to_string()),
            (CreateMode::Promote, promote_label),
        ] {
            if mode == CreateMode::Promote {
                spans.push(Span::raw("  "));
                x += 2;
            }
            let style = match (form.mode == mode, caret(CreateField::Mode)) {
                (true, true) => Style::default()
                    .fg(th.on_accent)
                    .bg(th.accent)
                    .add_modifier(Modifier::BOLD),
                (true, false) => text.bg(th.sel_bg).add_modifier(Modifier::BOLD),
                (false, _) => Style::default().fg(th.muted),
            };
            let w = (label.chars().count() as u16).min(row.right().saturating_sub(x));
            hits.push((Rect { x, width: w, ..row }, mode));
            x += w;
            spans.push(Span::styled(label, style));
        }
        put(
            f,
            &mut drawn.create_rows,
            row,
            spans,
            Some(CreateField::Mode),
        );
        drawn.mode_hits = hits;
    }
    // Under them, what the Promote mode has to say before anything else.
    if let (true, Some(row)) = (promote, row_rect(inner, 1)) {
        let line = match (&pair, form.open_promotion()) {
            (Some((base, default)), Some(open)) => vec![
                Span::raw(INDENT),
                Span::styled("● ", Style::default().fg(th.warn)),
                Span::styled(
                    format!("#{} {base} → {default} is already open · ", open.number),
                    dim,
                ),
                Span::styled(
                    "Enter updates its title and description",
                    Style::default().fg(th.accent),
                ),
            ],
            (Some(_), None) => Vec::new(),
            (None, _) => {
                let why = match (&form.base, &form.default) {
                    (Some(base), Some(_)) => format!(
                        "the project's base is {base}, the default branch — set another in Settings → Projects"
                    ),
                    _ => "reading the branches…".to_string(),
                };
                vec![Span::raw(INDENT), Span::styled(why, dim)]
            }
        };
        put(f, &mut drawn.create_rows, row, line, None);
    }

    // From and Into on one row: the mode's own in Promote, fields in
    // Feature.
    if let Some(row) = row_rect(inner, 2) {
        let half = row.width / 2;
        let left = Rect { width: half, ..row };
        let right = Rect {
            x: row.x + half,
            width: row.width - half,
            ..row
        };
        if promote {
            let side = |label: &str, name: Option<&String>| {
                vec![
                    form_label(label, false, th),
                    Span::styled(
                        name.cloned().unwrap_or_else(|| "…".into()),
                        text.add_modifier(Modifier::BOLD),
                    ),
                ]
            };
            put(
                f,
                &mut drawn.create_rows,
                left,
                side("From", form.base.as_ref()),
                None,
            );
            put(
                f,
                &mut drawn.create_rows,
                right,
                side("Into", form.default.as_ref()),
                None,
            );
        } else {
            let from = form_field(
                "From",
                &form.from,
                "a branch of yours",
                caret(CreateField::From),
                left,
                th,
            );
            put(
                f,
                &mut drawn.create_rows,
                left,
                from,
                Some(CreateField::From),
            );
            let into = form_field(
                "Into",
                &form.into,
                "the branch it merges into",
                caret(CreateField::Into),
                right,
                th,
            );
            put(
                f,
                &mut drawn.create_rows,
                right,
                into,
                Some(CreateField::Into),
            );
        }
    }
    if let Some(row) = row_rect(inner, 3) {
        let on = caret(CreateField::Title);
        let title = form_field("Title", &form.title, "(required)", on, row, th);
        put(
            f,
            &mut drawn.create_rows,
            row,
            title,
            Some(CreateField::Title),
        );
    }
    if let Some(row) = row_rect(inner, 4) {
        let draft = vec![
            form_label("Draft", caret(CreateField::Draft), th),
            Span::styled(check(form.draft), text),
            Span::styled(
                if form.draft {
                    "  opened as a draft"
                } else {
                    "  ready for review"
                },
                dim,
            ),
        ];
        put(
            f,
            &mut drawn.create_rows,
            row,
            draft,
            Some(CreateField::Draft),
        );
    }

    // Under the fields: the branch list while a branch field has the
    // caret; otherwise the commits — or what goes out — over the
    // description.
    let lower = Rect {
        y: inner.y.saturating_add(FIELD_ROWS),
        height: inner.height.saturating_sub(FIELD_ROWS),
        ..inner
    };
    if lower.height < 3 || lower.width < 4 {
        return drawn;
    }
    let listing = form.field.is_branch() && focused && form.saving.is_none();
    if listing {
        let title = match form.field {
            CreateField::From => "Your branches",
            _ => "Branches on origin",
        };
        let inner = form_box(f, lower, title, true, th);
        draw_picks(f, form, inner, &mut drawn, th);
        return drawn;
    }
    let (head, items, total) = if promote {
        going_out_lines(form, width, th)
    } else {
        commit_lines(form, th)
    };
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    if !head.is_empty() && lower.height >= DESC_MIN + 3 {
        let room = usize::from(lower.height - DESC_MIN - 3).min(LIST_MAX);
        lines.push(head);
        if items.len() <= room {
            lines.extend(items);
        } else if room > 0 {
            let shown = room - 1;
            lines.extend(items.into_iter().take(shown));
            lines.push(vec![Span::styled(
                format!("{INDENT}  … {} more", total.saturating_sub(shown)),
                dim,
            )]);
        }
    }
    let list_h = if lines.is_empty() {
        0
    } else {
        lines.len() as u16 + 2
    };
    for (i, spans) in lines.into_iter().enumerate() {
        if let Some(row) = row_rect(lower, i + 1) {
            put(f, &mut drawn.create_rows, row, spans, None);
        }
    }
    let box_area = Rect {
        y: lower.y + list_h,
        height: lower.height - list_h,
        ..lower
    };
    drawn.create_rows.push((box_area, CreateField::Body));
    drawn.body_view = form_text_box(
        f,
        box_area,
        "Description",
        &form.body,
        caret(CreateField::Body),
        "(no description)",
        th,
    );
    drawn
}

/// The rows a description box keeps at least, under the list.
const DESC_MIN: u16 = 5;
/// The most rows the list over the description takes past its header.
const LIST_MAX: usize = 8;

/// A list's header: its name in capitals, then what it says of itself.
fn list_head(name: &str, note: String, th: Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!("{INDENT}{name}"),
            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {note}"), Style::default().fg(th.dim)),
    ]
}

/// A list over the create form's description: its header, its rows, and
/// how many there are in all, for its `… N more`.
type ListLines = (Vec<Span<'static>>, Vec<Vec<Span<'static>>>, usize);

/// The Feature mode's list: the branch's commits, newest first, under how
/// many From has that Into lacks.
fn commit_lines(form: &CreateForm, th: Theme) -> ListLines {
    let into = form.into.trim();
    let mut note = match form.ahead {
        _ if form.pair().is_none() => "pick the branches".to_string(),
        None if form.filled_for.is_some() => "reading…".to_string(),
        None => String::new(),
        Some(0) if form.draft => format!("none not on {into} — opens on an empty one"),
        Some(n) => format!("{n} not on {into}"),
    };
    if form.fetching {
        note.push_str(" · fetching origin…");
    }
    let items: Vec<Vec<Span<'static>>> = form
        .commits
        .iter()
        .rev()
        .map(|c| {
            vec![
                Span::styled(format!("{INDENT}  {} ", c.sha), Style::default().fg(th.dim)),
                Span::styled(c.subject.clone(), Style::default().fg(th.text)),
            ]
        })
        .collect();
    let total = form.ahead.unwrap_or(0).max(items.len());
    (list_head("COMMITS", note, th), items, total)
}

/// The Promote mode's list: what goes out, latest merge first — each
/// pull request's number, title, Linear issues and author — under how
/// many, and since when.
fn going_out_lines(form: &CreateForm, width: usize, th: Theme) -> ListLines {
    let Some((base, default)) = form.promote_pair() else {
        return (Vec::new(), Vec::new(), 0);
    };
    let promotion = match &form.promotion {
        None => {
            let note = format!("reading what merged into {base}…");
            return (list_head("GOING OUT", note, th), Vec::new(), 0);
        }
        Some(Err(why)) => {
            let note = format!("couldn't ask GitHub: {why}");
            return (list_head("GOING OUT", note, th), Vec::new(), 0);
        }
        Some(Ok(p)) => p,
    };
    let n = promotion.prs.len();
    let count = match (n, promotion.capped) {
        (n, true) => format!("{n}+ PRs"),
        (1, false) => "1 PR".to_string(),
        (n, false) => format!("{n} PRs"),
    };
    let since = promotion
        .moved_at
        .map(|at| format!(" ({})", promote::day_of(at)))
        .unwrap_or_default();
    let note = format!("{count} merged into {base} since {default} last moved{since}");
    let number_w = promotion
        .prs
        .iter()
        .map(|pr| pr.number.to_string().len() + 1)
        .max()
        .unwrap_or(0);
    let lead = INDENT.chars().count() + 2 + number_w + 1;
    let items = promotion
        .prs
        .iter()
        .map(|pr| {
            let author = format!("  {}", pr.author);
            let mut ids = pr.linear.join(" ");
            let mut room =
                width.saturating_sub(lead + 1 + ids.chars().count() + author.chars().count() + 1);
            if room < crate::pr_preview::MIN_TEXT_W {
                ids.clear();
                room = width.saturating_sub(lead + 1 + author.chars().count() + 1);
            }
            let title = truncate(pr.title.trim(), room);
            let pad = room.saturating_sub(title.chars().count());
            vec![
                Span::styled(format!("{INDENT}⇡ "), Style::default().fg(th.merged)),
                Span::styled(
                    format!("{:<number_w$} ", format!("#{}", pr.number)),
                    Style::default().fg(th.dim),
                ),
                Span::styled(title, Style::default().fg(th.text)),
                Span::raw(" ".repeat(pad + 1)),
                Span::styled(ids, Style::default().fg(th.accent)),
                Span::styled(author, Style::default().fg(th.dim)),
            ]
        })
        .collect();
    (list_head("GOING OUT", note, th), items, n)
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
    let (delete_on, delete_note) = match (&form.stays, &form.branch, form.auto_delete) {
        (Some(why), ..) => (false, stays_note(why)),
        (_, None, _) if form.pending => (false, "  known once its details are in".to_string()),
        (_, None, _) => (false, "  not a branch of this repo — it stays".to_string()),
        (_, Some(_), true) => (
            true,
            "  the repo deletes merged branches itself".to_string(),
        ),
        (_, Some(_), false) if form.delete_branch && form.auto => (
            true,
            "  not after an auto-merge — the branch stays".to_string(),
        ),
        (_, Some(branch), false) if form.delete_branch => {
            (true, format!("  {branch} on GitHub, once merged"))
        }
        (_, Some(_), false) => (false, "  the branch stays on GitHub".to_string()),
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
    lines.push((
        Some(MergeRow::Bypass),
        vec![
            form_label("Bypass", on(MergeRow::Bypass), th),
            Span::styled(check(form.bypass), text),
            Span::styled(
                if form.bypass {
                    "  merge past the branch's rules, a required review included (admins)"
                } else {
                    "  keep to the branch's rules"
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

/// The rows over the close form's comment box — the title, the branches,
/// a gap and the delete box.
const CLOSE_ROWS: u16 = 4;

fn draw_close(f: &mut Frame, area: Rect, form: &CloseForm, focused: bool, th: Theme) -> Drawn {
    let (inner, foot_w) = form_frame(
        f,
        area,
        &format!("Close #{}", form.number),
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
    let on = |row: CloseRow| focused && form.row == row && form.saving.is_none();
    let into = if form.base.is_empty() {
        "its base".to_string()
    } else {
        form.base.clone()
    };
    let from = form.branch.clone().unwrap_or_else(|| form.head.clone());
    let delete_note = match (&form.stays, &form.branch) {
        (Some(why), _) => stays_note(why),
        (_, None) if form.pending => "  known once its details are in".to_string(),
        (_, None) => "  not a branch of this repo — it stays".to_string(),
        (_, Some(branch)) if form.delete_branch => format!("  {branch} on GitHub, once closed"),
        (_, Some(_)) => "  the branch stays on GitHub".to_string(),
    };
    let lines: [(Option<CloseRow>, Vec<Span<'static>>); CLOSE_ROWS as usize] = [
        (
            None,
            vec![Span::styled(
                format!("{INDENT}{}", form.title),
                text.add_modifier(Modifier::BOLD),
            )],
        ),
        (
            None,
            vec![Span::styled(
                format!("{INDENT}{from} → {into} · closes without merging"),
                dim,
            )],
        ),
        (None, Vec::new()),
        (
            Some(CloseRow::DeleteBranch),
            vec![
                form_label("Delete", on(CloseRow::DeleteBranch), th),
                Span::styled(check(form.delete_branch), text),
                Span::styled(delete_note, dim),
            ],
        ),
    ];
    for (i, (row, spans)) in lines.into_iter().enumerate() {
        let Some(rect) = row_rect(inner, i) else {
            break;
        };
        f.render_widget(Paragraph::new(crate::pr_preview::fit(spans, width)), rect);
        if let Some(row) = row {
            drawn.close_rows.push((rect, row));
        }
    }
    let box_area = Rect {
        y: inner.y.saturating_add(CLOSE_ROWS),
        height: inner.height.saturating_sub(CLOSE_ROWS),
        ..inner
    };
    if box_area.height < 3 || box_area.width < 4 {
        return drawn;
    }
    drawn.close_rows.push((box_area, CloseRow::Comment));
    drawn.body_view = form_text_box(
        f,
        box_area,
        "Comment",
        &form.comment,
        on(CloseRow::Comment),
        "(optional — left on the pull request as it closes)",
        th,
    );
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;

    thread_local! {
        /// What [`gh`] runs here: a stub, so nothing reaches GitHub.
        pub(super) static GH: RefCell<PathBuf> = RefCell::new(PathBuf::from("/nonexistent/gh"));
    }

    /// What a stub `gh` answers.
    struct Repo {
        default_branch: &'static str,
        /// `branches/<name>`'s `.protected`: a name not found is a 404.
        protected: &'static str,
        /// `pr list`'s answer.
        stacked: &'static str,
        delete_ok: bool,
        /// A `pr edit` whose arguments start with this fails.
        edit_fails: &'static str,
    }

    impl Default for Repo {
        fn default() -> Self {
            Self {
                default_branch: "main",
                protected: "false",
                stacked: "[]",
                delete_ok: true,
                edit_fails: "none",
            }
        }
    }

    /// Run `retire` against `repo`, on this thread; what it returned and
    /// every `gh` call it made, one line each.
    fn with_stub<T>(repo: Repo, retire: impl AsyncFnOnce(&Path) -> T) -> (T, Vec<String>) {
        let cases = format!(
            r#"  "repo view"*) echo {default} ;;
  "api repos/{{owner}}/{{repo}}/branches/"*) echo {protected} ;;
  "pr list"*) echo '{stacked}' ;;
  "pr edit {fails}"*) exit 1 ;;
  "pr edit "*) ;;
  "api -X DELETE"*) {delete} ;;"#,
            default = repo.default_branch,
            protected = repo.protected,
            stacked = repo.stacked,
            fails = repo.edit_fails,
            delete = if repo.delete_ok { ":" } else { "exit 1" },
        );
        let dir = tempfile::tempdir().unwrap();
        with_gh(&cases, dir.path(), retire)
    }

    /// Run `run` on this thread against a stub `gh` answering `cases` (a
    /// shell `case "$*"` body; anything else fails), in `dir`: what it
    /// returned and every `gh` call it made, one line each — and, for a
    /// call that pipes text in, `stdin: <text>` on the lines after it.
    pub(super) fn with_gh<T>(
        cases: &str,
        dir: &Path,
        run: impl AsyncFnOnce(&Path) -> T,
    ) -> (T, Vec<String>) {
        let bin = tempfile::tempdir().unwrap();
        let log = bin.path().join("calls");
        let stub = bin.path().join("gh");
        let script = format!(
            r#"#!/bin/sh
echo "$*" >> '{log}'
case "$*" in
  *"--body-file -"*|*"--input -"*) sed 's/^/stdin: /' >> '{log}' ;;
esac
case "$*" in
{cases}
  *) exit 2 ;;
esac
"#,
            log = log.display(),
        );
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        GH.with_borrow_mut(|gh| *gh = stub);
        let out = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run(dir));
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        (out, calls.lines().map(str::to_string).collect())
    }

    /// `git <args>` in `repo`, which must work: its output, trimmed.
    pub(super) fn git_ok(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo whose origin — as its remote refs say — has `main` and a
    /// `dev` two commits past it: the shas of `main`'s tip and of `dev`'s
    /// two, oldest first.
    pub(super) fn dev_ahead_of_main(repo: &Path) -> (String, String, String) {
        let git = |args: &[&str]| git_ok(repo, args);
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "Tess"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        let init = git(&["rev-parse", "HEAD"]);
        git(&["update-ref", "refs/remotes/origin/main", "main"]);
        git(&["checkout", "-q", "-b", "dev"]);
        git(&["commit", "-q", "--allow-empty", "-m", "Fix login (#1)"]);
        let one = git(&["rev-parse", "HEAD"]);
        git(&["commit", "-q", "--allow-empty", "-m", "Fix PDF header (#2)"]);
        let two = git(&["rev-parse", "HEAD"]);
        git(&["update-ref", "refs/remotes/origin/dev", "dev"]);
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        (init, one, two)
    }

    /// GitHub's merged list into `dev`: #2 and #1 on `dev`'s two commits,
    /// #0 on the one `main` already has.
    pub(super) fn merged_into_dev(init: &str, one: &str, two: &str) -> String {
        let node = |n: u64, title: &str, sha: &str, at: &str| {
            format!(
                r#"{{"number":{n},"title":"{title}","url":"https://github.com/o/r/pull/{n}","mergedAt":"{at}","headRefName":"rip-4{n}-x","author":{{"login":"sam"}},"mergeCommit":{{"oid":"{sha}"}}}}"#
            )
        };
        format!(
            r#"{{"data":{{"repository":{{"pullRequests":{{"nodes":[{},{},{}]}}}}}}}}"#,
            node(2, "Fix PDF header", two, "2026-11-04T10:00:00Z"),
            node(0, "Old", init, "2026-10-01T10:00:00Z"),
            node(1, "Fix login", one, "2026-11-03T10:00:00Z"),
        )
    }

    /// What goes out of `dev` is what `main` lacks, read from GitHub's list
    /// and git's range — and asking is all it does: nothing is written.
    #[test]
    fn a_promotion_reads_what_main_lacks_and_the_promotion_already_open() {
        let repo = tempfile::tempdir().unwrap();
        let (init, one, two) = dev_ahead_of_main(repo.path());
        let cases = format!(
            r#"  "pr list --head dev --base main"*) echo '[{{"number":9,"url":"https://github.com/o/r/pull/9"}}]' ;;
  "api graphql"*) echo '{}' ;;"#,
            merged_into_dev(&init, &one, &two)
        );
        let (read, calls) = with_gh(&cases, repo.path(), async |dir| {
            promote::read(dir, "dev", "main").await
        });
        let read = read.expect("read");
        assert_eq!(
            read.prs.iter().map(|p| p.number).collect::<Vec<_>>(),
            [2, 1]
        );
        assert_eq!(read.ahead, Some(2));
        assert_eq!(read.open.map(|o| o.number), Some(9));
        assert!(read.moved_at.is_some());
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(only_read(&calls), "{calls:?}");
        assert_eq!(default_branch(repo.path()).as_deref(), Some("main"));
    }

    /// The PULL REQUESTS MODAL as a 140×40 screen.
    pub(super) fn modal_screen(app: &mut App) -> String {
        use ratatui::backend::TestBackend;
        let mut term = ratatui::Terminal::new(TestBackend::new(140, 40)).unwrap();
        term.draw(|f| {
            let Some(Overlay::PullRequests(v)) = app.overlay.clone() else {
                panic!("no pull requests modal");
            };
            crate::pr_modal::draw(f, app, &v, app.theme, false);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..40)
            .map(|y| (0..140).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `⌘N` from `dev`, the project's base, opens on the Promote mode: dev
    /// into main, what goes out listed with its Linear issue, the title and
    /// description written from it. The promotion already open is said,
    /// and Enter updates it — `gh pr edit`, nothing pushed or created.
    #[test]
    fn a_form_from_the_base_promotes_it_and_enter_updates_the_open_one() {
        let repo = tempfile::tempdir().unwrap();
        let (init, one, two) = dev_ahead_of_main(repo.path());
        let cases = format!(
            r#"  "pr list --head dev --base main"*) echo '[{{"number":9,"url":"https://github.com/o/r/pull/9"}}]' ;;
  "api graphql"*) echo '{}' ;;
  "pr edit 9 "*) ;;"#,
            merged_into_dev(&init, &one, &two)
        );
        let today = promote::today();
        let (shot, calls) = with_gh(&cases, repo.path(), async |dir| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let project = ProjectId("p".into());
            let mut app = App::new();
            app.pr_actions_tx = Some(tx);
            app.linear.insert(
                project.clone(),
                crate::linear::LinearList {
                    teams: vec![crate::linear::LinearTeam {
                        id: "t".into(),
                        key: "RIP".into(),
                        name: "Riplo".into(),
                        triage_state: None,
                    }],
                    ..Default::default()
                },
            );
            let mut view = PullRequestsView::new(project.clone(), "demo".into(), dir.into());
            let form = CreateForm::new(project, dir.into(), "dev".into());
            let ticket = form.ticket;
            view.form = Some(Box::new(PrForm::Create(form)));
            app.overlay = Some(Overlay::PullRequests(view));
            land_answer(
                &mut app,
                Answer::Branches {
                    ticket,
                    heads: vec!["dev".into(), "main".into()],
                    bases: vec!["dev".into(), "main".into()],
                    base: Some("dev".into()),
                    default: Some("main".into()),
                    fetched: true,
                },
            );
            let form = create_form(&mut app).cloned().expect("the form");
            assert_eq!(form.mode, CreateMode::Promote, "from the base: a promotion");
            assert_eq!(form.field, CreateField::Title);
            let answer = rx.recv().await.expect("the promotion");
            land_answer(&mut app, answer);
            let form = create_form(&mut app).cloned().expect("the form");
            assert_eq!(
                form.title.as_str(),
                format!("Release {today}: Fix PDF header, Fix login")
            );
            assert_eq!(
                form.body.as_str(),
                "## Going out\n- Fix PDF header (#2) · RIP-42\n- Fix login (#1) · RIP-41"
            );
            let shot = modal_screen(&mut app);
            let hints = hints(view_form(&app));
            assert!(hints.iter().any(|h| h.does == "update #9"), "{hints:?}");
            handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
            let answer = rx.recv().await.expect("the update");
            land_answer(&mut app, answer);
            assert!(!form_up(&app), "updated: the form closes");
            let flash = format!("{:?}", app.flash);
            assert!(
                flash.contains("updated #9's title and description"),
                "{flash}"
            );
            shot
        });
        for want in [
            "Promote dev → main",
            "#9 dev → main is already open",
            "From   dev",
            "Into   main",
            "GOING OUT  2 PRs merged into dev since main last moved",
            "#2 Fix PDF header",
            "RIP-42  sam",
        ] {
            assert!(shot.contains(want), "{want}:\n{shot}");
        }
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("pr edit 9 --title Release")),
            "{calls:?}"
        );
        assert!(
            calls.contains(&"stdin: ## Going out".to_string()),
            "{calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("pr create")),
            "{calls:?}"
        );
    }

    /// The form up in the modal.
    fn view_form(app: &App) -> &PrForm {
        match &app.overlay {
            Some(Overlay::PullRequests(view)) => view.form.as_deref().expect("a form"),
            _ => panic!("no modal"),
        }
    }

    /// Promote with nothing open yet opens one — `gh pr create` from the
    /// base into the default as origin has them — and never pushes.
    #[test]
    fn a_promotion_with_none_open_creates_one_without_pushing() {
        let repo = tempfile::tempdir().unwrap();
        let (init, one, two) = dev_ahead_of_main(repo.path());
        let cases = format!(
            r#"  "pr list --head dev --base main"*) echo '[]' ;;
  "api graphql"*) echo '{}' ;;
  "pr create "*) cat >/dev/null; echo https://github.com/o/r/pull/12 ;;"#,
            merged_into_dev(&init, &one, &two)
        );
        let (created, calls) = with_gh(&cases, repo.path(), async |dir| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let project = ProjectId("p".into());
            let mut app = App::new();
            app.pr_actions_tx = Some(tx);
            let mut view = PullRequestsView::new(project.clone(), "demo".into(), dir.into());
            let mut form = CreateForm::new(project, dir.into(), "feature".into());
            form.base = Some("dev".into());
            form.default = Some("main".into());
            form.field = CreateField::Mode;
            view.form = Some(Box::new(PrForm::Create(form)));
            app.overlay = Some(Overlay::PullRequests(view));
            // The mode row's → picks Promote by hand.
            handle_key(&mut app, KeyEvent::from(KeyCode::Right));
            let answer = rx.recv().await.expect("the promotion");
            land_answer(&mut app, answer);
            let form = create_form(&mut app).cloned().expect("the form");
            assert_eq!(form.mode, CreateMode::Promote);
            assert!(form.open_promotion().is_none());
            handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
            let answer = rx.recv().await.expect("the create");
            matches!(answer, Answer::Created { result: Ok(_), .. })
        });
        // The repo has no origin to push to: a push tried would have
        // failed the create.
        assert!(created, "{calls:?}");
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("pr create --head dev --base main --title Release")),
            "{calls:?}"
        );
    }

    fn merged_into_main(repo: Repo, branch: &'static str) -> (BranchFate, Vec<String>) {
        with_stub(repo, async |dir| {
            retire_merged_branch(dir, branch, "main", "").await
        })
    }

    /// Nothing that changes GitHub: no base moved, nothing deleted.
    fn only_read(calls: &[String]) -> bool {
        !calls
            .iter()
            .any(|call| call.starts_with("pr edit") || call.contains("DELETE"))
    }

    /// `dev` merged into `main`, protected, with every open pull request
    /// based on it: they must not move.
    #[test]
    fn a_promotion_leaves_what_is_based_on_the_protected_branch_alone() {
        let (fate, calls) = merged_into_main(
            Repo {
                protected: "true",
                stacked: r#"[{"number":12},{"number":14}]"#,
                delete_ok: false,
                ..Repo::default()
            },
            "dev",
        );
        assert_eq!(
            fate,
            BranchFate::Guarded("dev is protected on GitHub".into())
        );
        assert!(only_read(&calls), "{calls:?}");
    }

    /// Nothing protects this `dev`, and the setting is all that names it:
    /// without it the same merge would delete the branch.
    #[test]
    fn the_worktree_base_setting_names_a_branch_nothing_protects() {
        for setting in ["dev", " dev ", "origin/dev", " origin/dev "] {
            let (fate, calls) = with_stub(
                Repo {
                    stacked: r#"[{"number":12}]"#,
                    ..Repo::default()
                },
                async |dir| retire_merged_branch(dir, "dev", "main", setting).await,
            );
            assert_eq!(
                fate,
                BranchFate::Guarded("dev is what new worktrees start from".into()),
                "{setting}"
            );
            assert!(calls.is_empty(), "the setting alone answers: {calls:?}");
        }
    }

    #[test]
    fn the_default_branch_stays_though_nothing_protects_it() {
        let (fate, calls) = with_stub(
            Repo {
                default_branch: "trunk",
                stacked: r#"[{"number":12}]"#,
                ..Repo::default()
            },
            async |dir| retire_merged_branch(dir, "trunk", "release", "").await,
        );
        assert_eq!(
            fate,
            BranchFate::Guarded("trunk is the repo's default branch".into())
        );
        assert!(only_read(&calls), "{calls:?}");
    }

    /// GitHub didn't answer: unsure keeps the branch and moves nothing.
    #[test]
    fn a_branch_nobody_can_vouch_for_stays() {
        for protected in ["", "null"] {
            let (fate, calls) = merged_into_main(
                Repo {
                    protected,
                    stacked: r#"[{"number":12}]"#,
                    ..Repo::default()
                },
                "feat",
            );
            assert!(matches!(fate, BranchFate::Guarded(_)), "{fate:?}");
            assert!(only_read(&calls), "{calls:?}");
        }
    }

    /// A real stack still does what it did: moved first, then deleted.
    #[test]
    fn a_feature_branch_goes_and_its_stack_moves_onto_the_base() {
        let (fate, calls) = merged_into_main(
            Repo {
                stacked: r#"[{"number":12},{"number":14}]"#,
                ..Repo::default()
            },
            "feat/x",
        );
        assert_eq!(
            fate,
            BranchFate::Deleted {
                moved: vec![12, 14]
            }
        );
        let at = |wanted: &str| calls.iter().position(|c| c.starts_with(wanted));
        assert!(at("pr edit 12 --base main").is_some(), "{calls:?}");
        assert!(at("api -X DELETE").is_some(), "{calls:?}");
        assert!(
            at("pr edit 14 --base main") < at("api -X DELETE"),
            "{calls:?}"
        );
    }

    /// The delete fails after the moves: they go back onto the branch that
    /// is still there.
    #[test]
    fn a_refused_delete_puts_the_stack_back() {
        let (fate, calls) = merged_into_main(
            Repo {
                stacked: r#"[{"number":12}]"#,
                delete_ok: false,
                ..Repo::default()
            },
            "feat",
        );
        assert_eq!(fate, BranchFate::Refused { stranded: vec![] });
        let edits: Vec<&String> = calls.iter().filter(|c| c.starts_with("pr edit")).collect();
        assert_eq!(
            edits,
            ["pr edit 12 --base main", "pr edit 12 --base feat"],
            "{calls:?}"
        );
    }

    /// One won't move: the ones before it go back, the ones after it are
    /// never tried, and nothing is deleted.
    #[test]
    fn a_stack_that_will_not_move_keeps_the_branch() {
        let (fate, calls) = merged_into_main(
            Repo {
                stacked: r#"[{"number":12},{"number":14},{"number":16}]"#,
                edit_fails: "14",
                ..Repo::default()
            },
            "feat",
        );
        assert_eq!(
            fate,
            BranchFate::Held {
                stacked: vec![14, 16],
                stranded: vec![],
            }
        );
        assert!(
            calls.iter().any(|c| c == "pr edit 12 --base feat"),
            "{calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("pr edit 16")),
            "{calls:?}"
        );
        assert!(!calls.iter().any(|c| c.contains("DELETE")), "{calls:?}");
    }

    /// One that moved and won't go back is left where it is, and named.
    #[test]
    fn what_will_not_go_back_is_named() {
        let (fate, _) = merged_into_main(
            Repo {
                stacked: r#"[{"number":12}]"#,
                delete_ok: false,
                edit_fails: "12 --base feat",
                ..Repo::default()
            },
            "feat",
        );
        assert_eq!(fate, BranchFate::Refused { stranded: vec![12] });
        assert_eq!(
            merged_message(10, "main", MergeMethod::Merge, false, &fate),
            "merged #10 into main (merge), branch kept: GitHub would not delete it; #12 left on main"
        );
    }

    #[test]
    fn closing_a_long_lived_branchs_pull_request_keeps_the_branch() {
        let (fate, calls) = with_stub(
            Repo {
                protected: "true",
                ..Repo::default()
            },
            async |dir| retire_closed_branch(dir, "dev", "").await,
        );
        assert_eq!(
            fate,
            BranchFate::Guarded("dev is protected on GitHub".into())
        );
        assert!(only_read(&calls), "{calls:?}");
    }

    #[test]
    fn closing_still_deletes_an_ordinary_branch_unless_something_is_stacked() {
        let (fate, _) = with_stub(Repo::default(), async |dir| {
            retire_closed_branch(dir, "feat", "").await
        });
        assert_eq!(fate, BranchFate::Deleted { moved: vec![] });
        let (fate, calls) = with_stub(
            Repo {
                stacked: r#"[{"number":12}]"#,
                ..Repo::default()
            },
            async |dir| retire_closed_branch(dir, "feat", "").await,
        );
        assert_eq!(
            fate,
            BranchFate::Held {
                stacked: vec![12],
                stranded: vec![],
            }
        );
        assert!(only_read(&calls), "{calls:?}");
    }

    #[test]
    fn stacked_pull_requests_are_read_from_gh() {
        assert_eq!(
            parse_numbers(r#"[{"number":12},{"number":14}]"#),
            Some(vec![12, 14])
        );
        assert_eq!(parse_numbers("[]"), Some(vec![]));
        assert_eq!(parse_numbers("not json"), None, "unsure keeps the branch");
        assert_eq!(parse_numbers(r#"[{"title":"x"}]"#), None);
    }

    #[test]
    fn the_flash_says_what_became_of_the_branch() {
        let merged = |fate| merged_message(10, "dev", MergeMethod::Squash, false, &fate);
        assert_eq!(merged(BranchFate::Kept), "merged #10 into dev (squash)");
        assert_eq!(
            merged(BranchFate::Deleted { moved: vec![] }),
            "merged #10 into dev (squash), branch deleted"
        );
        assert_eq!(
            merged(BranchFate::Deleted {
                moved: vec![12, 14]
            }),
            "merged #10 into dev (squash), branch deleted; #12, #14 moved onto dev"
        );
        let held = BranchFate::Held {
            stacked: vec![12],
            stranded: vec![],
        };
        assert_eq!(
            merged(held.clone()),
            "merged #10 into dev (squash), branch kept: #12 stacked on it"
        );
        assert_eq!(
            closed_message(10, &held),
            "closed #10, branch kept: #12 stacked on it"
        );
        assert_eq!(
            merged(BranchFate::Held {
                stacked: vec![12],
                stranded: vec![14],
            }),
            "merged #10 into dev (squash), branch kept: #12 stacked on it; #14 left on dev"
        );
        assert_eq!(
            merged(BranchFate::Guarded("dev is protected on GitHub".into())),
            "merged #10 into dev (squash), branch kept: dev is protected on GitHub"
        );
        assert_eq!(
            merged(BranchFate::Refused { stranded: vec![] }),
            "merged #10 into dev (squash), branch kept: GitHub would not delete it"
        );
    }

    fn commit(subject: &str, body: &str) -> FillCommit {
        FillCommit {
            sha: "abc1234".into(),
            subject: subject.into(),
            body: body.into(),
        }
    }

    #[test]
    fn the_fill_is_ghs() {
        assert_eq!(
            fill_text("fix/login-redirect", &[commit("Stop the bounce", "Why.")]),
            ("Stop the bounce".to_string(), "Why.".to_string())
        );
        assert_eq!(
            fill_text(
                "fix/login-redirect",
                &[commit("One", ""), commit("Two", "b")]
            ),
            ("Login redirect".to_string(), "- One\n- Two".to_string())
        );
        assert_eq!(
            fill_text("eager_sparrow", &[]),
            ("Eager sparrow".to_string(), String::new())
        );
        let many: Vec<FillCommit> = (0..FILL_SUBJECTS)
            .map(|i| commit(&format!("c{i}"), ""))
            .collect();
        let (_, body) = fill_text("x", &many);
        assert!(
            body.ends_with("- c49\n- …"),
            "a capped list says so: {body}"
        );
    }

    /// `dev` left behind locally while origin's moved on — the shape a
    /// branch merged into elsewhere leaves — is counted from origin's
    /// copy; a local `dev` with commits origin lacks is counted as is.
    #[test]
    fn a_stale_local_from_counts_from_origin() {
        let git = |repo: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .output()
                .expect("run git");
            assert!(out.status.success(), "git {args:?}");
        };
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "Tess"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git(repo, &["branch", "dev"]);
        git(repo, &["update-ref", "refs/remotes/origin/main", "main"]);
        git(repo, &["checkout", "-q", "dev"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", "on origin"]);
        git(repo, &["update-ref", "refs/remotes/origin/dev", "dev"]);
        git(repo, &["reset", "-q", "--hard", "main"]);

        assert_eq!(
            head_ref(repo, "dev").as_deref(),
            Some("refs/remotes/origin/dev")
        );
        assert_eq!(read_commits(repo, "dev", "main").map(|f| f.ahead), Some(1));

        git(repo, &["reset", "-q", "--hard", "origin/dev"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", "local only"]);
        assert_eq!(head_ref(repo, "dev").as_deref(), Some("refs/heads/dev"));
        assert_eq!(read_commits(repo, "dev", "main").map(|f| f.ahead), Some(2));
        assert_eq!(head_ref(repo, "nope"), None);
    }

    /// A draft from a branch with nothing new starts on an empty commit,
    /// the edits in its checkout left as they were; a local copy behind
    /// origin's is refused rather than moved.
    #[test]
    fn an_empty_commit_leaves_the_checkout_alone() {
        let git = |repo: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .output()
                .expect("run git");
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@t"]);
        git(repo, &["config", "user.name", "Tess"]);
        std::fs::write(repo.join("a.txt"), "one").unwrap();
        git(repo, &["add", "a.txt"]);
        git(repo, &["commit", "-q", "-m", "init"]);
        git(repo, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(repo.join("a.txt"), "two").unwrap();
        std::fs::write(repo.join("b.txt"), "new").unwrap();
        git(repo, &["add", "b.txt"]);
        let before = git(repo, &["status", "--porcelain"]);
        assert_eq!(read_commits(repo, "feat", "main").map(|f| f.ahead), Some(0));

        commit_empty(repo, "feat", "Start feat").unwrap();
        assert_eq!(read_commits(repo, "feat", "main").map(|f| f.ahead), Some(1));
        assert_eq!(
            git(repo, &["log", "-1", "--format=%s", "feat"]),
            "Start feat"
        );
        assert_eq!(git(repo, &["status", "--porcelain"]), before);

        git(repo, &["update-ref", "refs/remotes/origin/old", "main"]);
        commit_empty(repo, "old", "Start old").unwrap();
        assert_eq!(read_commits(repo, "old", "main").map(|f| f.ahead), Some(1));

        git(repo, &["update-ref", "refs/remotes/origin/feat", "feat"]);
        git(repo, &["update-ref", "refs/heads/feat", "main"]);
        assert!(commit_empty(repo, "feat", "again").is_err());
    }

    #[test]
    fn commits_parse_sha_subject_then_body() {
        let text = "aaa1111\x1fOne\x1f\x1e\nbbb2222\x1fTwo\x1fline 1\nline 2\n\x1e\n";
        let parsed = parse_commits(text);
        assert_eq!(
            parsed,
            vec![
                FillCommit {
                    sha: "aaa1111".into(),
                    ..commit("One", "")
                },
                FillCommit {
                    sha: "bbb2222".into(),
                    ..commit("Two", "line 1\nline 2")
                },
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
    fn bypass_and_auto_turn_each_other_off() {
        let mut form = MergeForm {
            project: ProjectId("p".into()),
            dir: PathBuf::new(),
            number: 1,
            url: String::new(),
            title: String::new(),
            base: String::new(),
            branch: None,
            head: String::new(),
            pending: false,
            method: MergeMethod::Squash,
            allowed: MergeMethod::ALL.to_vec(),
            delete_branch: false,
            auto_delete: false,
            stays: None,
            auto: false,
            bypass: false,
            row: MergeRow::Auto,
            warnings: Vec::new(),
            ticket: 0,
            saving: None,
            notice: None,
            rows: Vec::new(),
        };
        change_merge_row(&mut form, true);
        assert!(form.auto && !form.bypass);
        form.row = form.row.step(true);
        assert_eq!(form.row, MergeRow::Bypass, "the last row");
        change_merge_row(&mut form, true);
        assert!(form.bypass && !form.auto, "bypass clears auto");
        form.row = MergeRow::Auto;
        change_merge_row(&mut form, true);
        assert!(form.auto && !form.bypass, "auto clears bypass");
    }

    /// Tab walks the Feature mode's fields round from the mode row, and
    /// the Promote mode's skip the branches it fixes.
    #[test]
    fn tab_walks_the_create_form_round() {
        let mut form = CreateForm::new(ProjectId("p".into()), PathBuf::from("/x"), "f".into());
        let mut field = CreateField::Mode;
        for expected in [
            CreateField::From,
            CreateField::Into,
            CreateField::Title,
            CreateField::Draft,
            CreateField::Body,
            CreateField::Mode,
        ] {
            field = form.step(field, true);
            assert_eq!(field, expected);
        }
        assert_eq!(form.step(CreateField::Mode, false), CreateField::Body);
        form.mode = CreateMode::Promote;
        assert_eq!(form.step(CreateField::Mode, true), CreateField::Title);
        assert_eq!(form.step(CreateField::Title, false), CreateField::Mode);
    }

    #[test]
    fn the_hints_come_from_the_table() {
        let form = CreateForm::new(ProjectId("p".into()), PathBuf::from("/x"), "feature".into());
        for field in CreateField::FEATURE {
            let mut form = form.clone();
            form.field = field;
            crate::hints::assert_hints_from(&hints(&PrForm::Create(form)), keys::ALL);
        }
    }

    /// A branch field holding a branch takes the first letter typed — or a
    /// paste — in its place, a search for another; what follows is added
    /// to it. Nothing typed into the title clears it.
    #[test]
    fn typing_in_a_branch_field_starts_a_new_search() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        let mut view =
            PullRequestsView::new(ProjectId("p".into()), "demo".into(), PathBuf::from("/x"));
        let mut form =
            CreateForm::new(ProjectId("p".into()), PathBuf::from("/x"), "feature".into());
        form.title.set_text("Fix");
        form.focus(CreateField::From);
        view.form = Some(Box::new(PrForm::Create(form)));
        app.overlay = Some(Overlay::PullRequests(view));
        let create = |app: &mut App| create_form(app).cloned().expect("the form");
        let type_char = |app: &mut App, c: char| {
            handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        };

        type_char(&mut app, 'm');
        type_char(&mut app, 'a');
        assert_eq!(
            create(&mut app).from.as_str(),
            "ma",
            "replaced, then added to"
        );

        handle_key(&mut app, KeyEvent::from(KeyCode::Tab));
        assert!(paste(&mut app, "origin-main"));
        assert_eq!(create(&mut app).into.as_str(), "origin-main");

        handle_key(&mut app, KeyEvent::from(KeyCode::Tab));
        assert_eq!(create(&mut app).field, CreateField::Title);
        assert!(paste(&mut app, " login"));
        assert_eq!(
            create(&mut app).title.as_str(),
            "Fix login",
            "the title kept"
        );
    }

    /// A branch field's list shows every branch until it is typed in, the
    /// cursor on the field's own; typing narrows it, and Enter takes the
    /// best match.
    #[test]
    fn a_branch_field_picks_from_the_branches() {
        let mut form =
            CreateForm::new(ProjectId("p".into()), PathBuf::from("/x"), "feature".into());
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

    /// A merge that landed fires the MERGE CONFETTI — with the modal long
    /// closed too — and one that only switched auto-merge on, or was
    /// refused, fires nothing.
    #[test]
    fn a_landed_merge_fires_confetti_and_nothing_else_does() {
        let merged = |result: Result<String, String>, landed: Option<(u64, String)>| {
            let mut app = App::new();
            land_answer(
                &mut app,
                Answer::Merged {
                    project: ProjectId("p".into()),
                    ticket: 1,
                    result,
                    landed,
                    method: (PathBuf::from("/x"), MergeMethod::Squash),
                },
            );
            app.confetti.is_some()
        };
        assert!(merged(Ok("merged #482".into()), Some((482, "main".into()))));
        assert!(!merged(Ok("#482 merges once…".into()), None), "auto-merge");
        assert!(!merged(
            Err("not mergeable".into()),
            Some((482, "main".into()))
        ));
    }
}
