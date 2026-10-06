//! The PULL REQUESTS MODAL's forms, each drawn in the reading pane's
//! place while the list stays up on the left — the ISSUES MODAL's editor,
//! the same way round:
//!
//! * **New pull request** (`⌘N`): the branch it comes from — the one
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
//! * **Merge** (`⌘X`): the pull request under the cursor, how — squash,
//!   a merge commit or a rebase, only those the repo allows (`gh repo
//!   view`) — whether its branch goes with it, and whether GitHub should
//!   wait for its checks and reviews (auto-merge). What GitHub said that
//!   stands in the way is spelled out first: a draft, conflicts, failing
//!   or running checks, a review still owed. Enter runs `gh pr merge`.
//!   The **Review** SETTINGS tab holds the defaults both open on.
//! * **Close** (`⌘W`): the pull request under the cursor closed
//!   without merging — a comment left on it first if one is written, and
//!   its branch deleted on GitHub if that is ticked (only a branch of this
//!   repo's; a fork's lives elsewhere). Enter runs `gh pr close`.
//!
//! * **Review** (`⌘⇧R`, or Enter on the Reviews tab): approve the pull
//!   request under the cursor, request changes, or comment — a body
//!   optional on an approval and required otherwise; on one of the user's
//!   own only a comment, which is all GitHub takes from its author. Enter
//!   runs `gh pr review`, and the page reads the review back.
//!
//! And one verb with no form: `⌘D` ([`toggle_draft`]) marks a draft
//! ready for review, or turns a ready one back into a draft (`gh pr
//! ready`, `--undo`) — undone by pressing it again.
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
use crate::git_proc::PUSH_TIMEOUT;
use crate::hints::Hint;
use crate::pr_modal::PullRequestsView;
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
    Close(CloseForm),
    Review(ReviewForm),
}

impl PrForm {
    /// The form's own ticket: an answer carrying another is for a form
    /// since closed.
    fn ticket(&self) -> u64 {
        match self {
            PrForm::Create(f) => f.ticket,
            PrForm::Merge(f) => f.ticket,
            PrForm::Close(f) => f.ticket,
            PrForm::Review(f) => f.ticket,
        }
    }

    /// Enter sent it and the answer is not in: it takes no keys but Esc.
    fn saving(&self) -> bool {
        match self {
            PrForm::Create(f) => f.saving.is_some(),
            PrForm::Merge(f) => f.saving.is_some(),
            PrForm::Close(f) => f.saving.is_some(),
            PrForm::Review(f) => f.saving.is_some(),
        }
    }

    /// GitHub (or git) refused what it sent: it says why on its frame and
    /// takes keys again, everything typed still in it.
    fn refused(&mut self, why: String) {
        let (saving, notice) = match self {
            PrForm::Create(f) => (&mut f.saving, &mut f.notice),
            PrForm::Merge(f) => (&mut f.saving, &mut f.notice),
            PrForm::Close(f) => (&mut f.saving, &mut f.notice),
            PrForm::Review(f) => (&mut f.saving, &mut f.notice),
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

/// `⌘N`: a new pull request, filled in before it is sent.
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
            fetching: false,
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
    let mut form = CreateForm::new(project, dir.clone(), from, config.pr_draft);
    if form.from.is_empty() {
        form.field = CreateField::From;
    }
    let ticket = form.ticket;
    let tx = app.pr_actions_tx.clone();
    form.fetching = tx.is_some();
    put_form(app, PrForm::Create(form));
    if let Some(tx) = tx {
        let base_setting = config.worktree_base_branch.clone();
        let quit = app.branch_switch.quit.clone();
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
/// message is the description. `into` is origin's when origin has it;
/// `from` is whichever of the two copies the pull request would carry
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
    } else if form.ahead == Some(0) && form.fetching {
        Some("fetching origin to check — try again in a moment".into())
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
    /// The **Delete merged branch** SETTING as the form opened: what
    /// [`MergeForm::apply_detail`] ticks the box to once it knows the
    /// branch is ours.
    pub delete_default: bool,
    /// The repo deletes merged branches itself.
    pub auto_delete: bool,
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

/// `⌘X`: the merge form for the pull request under the cursor, on
/// the **Review** tab's defaults, the repo asked which methods it allows.
pub(crate) fn open_merge(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
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
        bypass: false,
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

/// A pull request's body landed: a merge or close form opened on it
/// before it did takes its branch and its base from it — and a merge form,
/// what stands in the way.
pub(crate) fn detail_landed(app: &mut App, url: &str) {
    let waiting = match form(app) {
        Some(PrForm::Merge(f)) => f.pending && f.url == url,
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
    match form(app) {
        Some(PrForm::Merge(form)) => form.apply_detail(&pr, Some(&detail)),
        Some(PrForm::Close(form)) => form.apply_detail(&pr, Some(&detail)),
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
        let result = merge(&dir, number, method, auto, bypass, delete.as_deref())
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
    bypass: bool,
    branch: Option<&str>,
) -> Result<bool, String> {
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
        return Ok(false);
    };
    Ok(delete_branch(dir, branch).await)
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
        if self.branch.is_some() {
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
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
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
        comment: TextInput::multiline(),
        row: CloseRow::Comment,
        ticket: crate::view_jobs::ticket(),
        saving: None,
        notice: None,
        rows: Vec::new(),
    };
    form.apply_detail(&pr, app.pr_detail.get(&pr.url));
    put_form(app, PrForm::Close(form));
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
            .map(|deleted| closed_message(number, deleted));
        let _ = tx.send(Answer::Closed {
            project,
            ticket,
            result,
        });
    });
}

/// Close pull request `number` — `comment` left on it first, when there
/// is one — and delete `branch` on GitHub once it is closed. Whether the
/// branch went.
async fn close(
    dir: &Path,
    number: u64,
    comment: &str,
    branch: Option<&str>,
) -> Result<bool, String> {
    let number = number.to_string();
    let mut args = vec!["pr", "close", number.as_str()];
    if !comment.is_empty() {
        args.extend(["--comment", comment]);
    }
    run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await?;
    match branch {
        Some(branch) => Ok(delete_branch(dir, branch).await),
        None => Ok(false),
    }
}

/// The flash a close leaves.
fn closed_message(number: u64, deleted: bool) -> String {
    let gone = if deleted { ", branch deleted" } else { "" };
    format!("closed #{number}{gone}")
}

// ---- reviewing ----

/// What a review says: the three `gh pr review` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

impl ReviewVerdict {
    /// Every verdict, in the order `←`/`→` walk them.
    const ORDER: &'static [ReviewVerdict] = &[
        ReviewVerdict::Approve,
        ReviewVerdict::RequestChanges,
        ReviewVerdict::Comment,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ReviewVerdict::Approve => "Approve",
            ReviewVerdict::RequestChanges => "Request changes",
            ReviewVerdict::Comment => "Comment",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            ReviewVerdict::Approve => "--approve",
            ReviewVerdict::RequestChanges => "--request-changes",
            ReviewVerdict::Comment => "--comment",
        }
    }

    /// GitHub takes an approval with nothing written; the other two need
    /// a word.
    fn needs_body(self) -> bool {
        self != ReviewVerdict::Approve
    }

    /// The Enter hint: what sending it does.
    fn verb(self) -> &'static str {
        match self {
            ReviewVerdict::Approve => "approve",
            ReviewVerdict::RequestChanges => "request changes",
            ReviewVerdict::Comment => "comment",
        }
    }

    /// The flash it leaves, sent and done.
    fn done(self, number: u64) -> String {
        match self {
            ReviewVerdict::Approve => format!("approved #{number}"),
            ReviewVerdict::RequestChanges => format!("requested changes on #{number}"),
            ReviewVerdict::Comment => format!("reviewed #{number} with a comment"),
        }
    }
}

/// The review form's rows, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewRow {
    Verdict,
    Body,
}

impl ReviewRow {
    /// Two rows: either way round lands on the other one.
    fn other(self) -> Self {
        match self {
            ReviewRow::Verdict => ReviewRow::Body,
            ReviewRow::Body => ReviewRow::Verdict,
        }
    }
}

/// `⌘⇧R`, and Enter on the Reviews tab: a review of the pull request
/// under the cursor — approve it, ask for changes, or comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewForm {
    pub project: ProjectId,
    pub dir: PathBuf,
    pub number: u64,
    pub url: String,
    pub title: String,
    /// The signed-in user opened it: GitHub takes only a comment from its
    /// author, so that is the one verdict offered.
    pub mine: bool,
    pub verdict: ReviewVerdict,
    /// The review's body — optional on an approval.
    pub body: TextInput,
    pub row: ReviewRow,
    pub ticket: u64,
    pub saving: Option<String>,
    pub notice: Option<String>,
    /// As of the last draw: each row's rect, for the mouse.
    pub rows: Vec<(Rect, ReviewRow)>,
}

impl ReviewForm {
    /// The verdicts this pull request can be given.
    fn verdicts(&self) -> &'static [ReviewVerdict] {
        if self.mine {
            &[ReviewVerdict::Comment]
        } else {
            ReviewVerdict::ORDER
        }
    }

    /// `←`/`→`/Space: the next verdict, round either end.
    fn step_verdict(&mut self, forward: bool) {
        let order = self.verdicts();
        let n = order.len();
        let at = order.iter().position(|v| *v == self.verdict).unwrap_or(0);
        let next = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        self.verdict = order[next];
        self.notice = None;
    }
}

/// `⌘⇧R`: the review form for the pull request under the cursor —
/// opening on Approve with the verdict under the caret, or, on one of the
/// user's own, on Comment with the caret in the box.
pub(crate) fn open_review(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
        return;
    };
    let (verdict, row) = if pr.mine {
        (ReviewVerdict::Comment, ReviewRow::Body)
    } else {
        (ReviewVerdict::Approve, ReviewRow::Verdict)
    };
    let form = ReviewForm {
        project,
        dir,
        number: pr.number,
        url: pr.url.clone(),
        title: pr.title.clone(),
        mine: pr.mine,
        verdict,
        body: TextInput::multiline(),
        row,
        ticket: crate::view_jobs::ticket(),
        saving: None,
        notice: None,
        rows: Vec::new(),
    };
    put_form(app, PrForm::Review(form));
    app.dirty = true;
}

/// Enter on the review form: `gh pr review` off the loop, the form held
/// until the answer lands. A verdict that needs a word and has none says
/// so and puts the caret in the box instead.
fn submit_review(app: &mut App) {
    let tx = app.pr_actions_tx.clone();
    let Some(form) = review_form(app) else {
        return;
    };
    if form.saving.is_some() {
        return;
    }
    let body = form.body.trim().to_string();
    if form.verdict.needs_body() && body.is_empty() {
        form.notice = Some(format!("{} needs a comment", form.verdict.label()));
        form.row = ReviewRow::Body;
        return;
    }
    let Some(tx) = tx else {
        return;
    };
    form.notice = None;
    form.saving = Some(format!("reviewing #{}…", form.number));
    let (project, dir, ticket, number, url, verdict) = (
        form.project.clone(),
        form.dir.clone(),
        form.ticket,
        form.number,
        form.url.clone(),
        form.verdict,
    );
    tokio::spawn(async move {
        let n = number.to_string();
        let mut args = vec!["pr", "review", n.as_str(), verdict.flag()];
        if !body.is_empty() {
            args.extend(["--body", body.as_str()]);
        }
        let result = run_piped(gh(&dir, &args), "", REQUEST_TIMEOUT)
            .await
            .map(|_| verdict.done(number));
        let _ = tx.send(Answer::Reviewed {
            project,
            url,
            ticket,
            result,
        });
    });
}

// ---- ready for review / draft ----

/// `⌘D`: the pull request under the cursor marked ready for review —
/// or, ready already, turned back into a draft — off the loop, the footer
/// saying so meanwhile. Whether it is a draft is the body's word once it
/// has landed, the row's until then.
pub(crate) fn toggle_draft(app: &mut App) {
    let Some(view) = modal(app) else {
        return;
    };
    let (project, dir) = (view.project.clone(), view.dir.clone());
    let Some(pr) = crate::pr_modal::selected_pr(app) else {
        return;
    };
    let Some(tx) = app.pr_actions_tx.clone() else {
        return;
    };
    let ready = app
        .pr_detail
        .get(&pr.url)
        .map_or(pr.is_draft, |d| d.is_draft);
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
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(args).current_dir(dir);
    cmd
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
    /// The create form's branches, and the default to merge into.
    Branches {
        ticket: u64,
        heads: Vec<String>,
        bases: Vec<String>,
        base: Option<String>,
        /// Read after origin was fetched (or the fetch gave up).
        fetched: bool,
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
    /// `gh pr close`: what to flash, or why it refused.
    Closed {
        project: ProjectId,
        ticket: u64,
        result: Result<String, String>,
    },
    /// `gh pr review`: what to flash, or why it refused.
    Reviewed {
        project: ProjectId,
        url: String,
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

fn review_form(app: &mut App) -> Option<&mut ReviewForm> {
    match form(app)? {
        PrForm::Review(f) => Some(f),
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
            fetched,
        } => land_branches(app, ticket, heads, bases, base, fetched),
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
        } => land_done(app, &project, ticket, result, "merge"),
        Answer::Closed {
            project,
            ticket,
            result,
        } => land_done(app, &project, ticket, result, "close the pull request"),
        Answer::Reviewed {
            project,
            url,
            ticket,
            result,
        } => land_reviewed(app, &project, url, ticket, result),
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
/// once origin is `fetched`, the pair's commits maybe moved.
fn land_branches(
    app: &mut App,
    ticket: u64,
    heads: Vec<String>,
    bases: Vec<String>,
    base: Option<String>,
    fetched: bool,
) {
    let Some(PrForm::Create(form)) = form_for(app, ticket) else {
        return;
    };
    if fetched {
        form.fetching = false;
        form.filled_for = None;
    }
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
/// are read again, so the page and the row's badge follow.
fn read_again(app: &mut App, project: &ProjectId, url: String) {
    app.pr_detail_stale.insert(url);
    if crate::pr_modal::is_up(app) {
        crate::pr_modal::schedule_detail(app);
    }
    crate::pr_modal::request_list(app, project);
}

/// `gh pr review` answered: done, the form closes onto the Reviews tab,
/// the pull request's body and the list read again so the page and the
/// row's badge carry the review; refused, the form says why.
fn land_reviewed(
    app: &mut App,
    project: &ProjectId,
    url: String,
    ticket: u64,
    result: Result<String, String>,
) {
    match result {
        Ok(said) => {
            if form_for(app, ticket).is_some() {
                close_form(app);
                if let Some(view) = modal(app) {
                    if view.tabs.switch(crate::pr_preview::PrTab::Reviews) {
                        view.scroll = 0;
                    }
                }
            }
            read_again(app, project, url);
            app.flash = Some(crate::flash::Flash::done(said));
        }
        Err(why) => refused(app, ticket, why, "submit the review"),
    }
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
    pub const DRAFT: Key = Key::new(&["space"], "draft");
    /// The merge form's rows.
    pub const OPTION: Key = Key::new(&["up", "down"], "option").show(2);
    pub const CHANGE: Key = Key::new(&["left", "right", "space"], "change").show(2);
    pub const MERGE: Key = Key::new(&["enter"], "merge");
    /// The close form's.
    pub const CLOSE: Key = Key::new(&["enter"], "close PR");
    pub const DELETE: Key = Key::new(&["space"], "delete branch");
    /// The review form's.
    pub const REVIEW: Key = Key::new(&["enter"], "submit review");
    pub const VERDICT: Key = Key::new(&["left", "right", "space"], "verdict").show(2);
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
        CLOSE,
        DELETE,
        REVIEW,
        VERDICT,
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
        PrForm::Review(form) => {
            let mut hints = vec![keys::REVIEW.hint_as(form.verdict.verb()).kept()];
            match form.row {
                ReviewRow::Verdict if form.verdicts().len() > 1 => hints.push(keys::VERDICT.hint()),
                ReviewRow::Verdict => {}
                ReviewRow::Body => hints.push(crate::ui::task_keys::NEWLINE.hint()),
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
            PrForm::Review(_) => review_key(app, key),
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

/// Keys on the review form: Tab between the verdict and the box,
/// `←`/`→`/Space change the verdict, ↑/↓ step between them past the box's
/// first line, a letter typed on the verdict lands in the box, and Enter
/// sends — a line break in the box on the NEWLINE chords.
fn review_key(app: &mut App, key: KeyEvent) {
    let Some(form) = review_form(app) else {
        return;
    };
    let on_body = form.row == ReviewRow::Body;
    match key.code {
        _ if keys::FIELD.matches(&key) => form.row = form.row.other(),
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if !on_body => {
            form.step_verdict(key.code != KeyCode::Left)
        }
        KeyCode::Down if !on_body => form.row = ReviewRow::Body,
        KeyCode::Up if on_body && !form.body.handle_key(&key).consumed() => {
            form.row = ReviewRow::Verdict
        }
        KeyCode::Enter if !(on_body && form.body.takes_newline(&key)) => submit_review(app),
        _ if !on_body && is_char_key(&key) => {
            form.row = ReviewRow::Body;
            if form.body.handle_key(&key).changed() {
                form.notice = None;
            }
        }
        _ if on_body && form.body.handle_key(&key).changed() => form.notice = None,
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
    if let PrForm::Review(form) = form {
        if form.saving.is_none() {
            form.row = ReviewRow::Body;
            form.body.insert_str(text);
            form.notice = None;
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
    } else if let Some(form) = review_form(app) {
        if !clicked {
            return;
        }
        if let Some(&(_, row)) = form.rows.iter().find(|(r, _)| r.contains(at)) {
            if row == ReviewRow::Verdict {
                form.step_verdict(true);
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
    merge_rows: Vec<(Rect, MergeRow)>,
    close_rows: Vec<(Rect, CloseRow)>,
    review_rows: Vec<(Rect, ReviewRow)>,
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
        PrForm::Review(form) => {
            form.rows = drawn.review_rows;
            if let Some(view) = drawn.body_view {
                form.body.set_view(view);
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
        PrForm::Review(form) => draw_review(f, area, form, focused, th),
    }
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
    if form.fetching {
        from.push(Span::styled("  fetching origin…", dim));
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
    let delete_note = match &form.branch {
        None if form.pending => "  known once its details are in".to_string(),
        None => "  not a branch of this repo — it stays".to_string(),
        Some(branch) if form.delete_branch => format!("  {branch} on GitHub, once closed"),
        Some(_) => "  the branch stays on GitHub".to_string(),
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

/// The rows over the review form's box — the title, what it is, a gap
/// and the verdict.
const REVIEW_ROWS: u16 = 4;

fn draw_review(f: &mut Frame, area: Rect, form: &ReviewForm, focused: bool, th: Theme) -> Drawn {
    let (inner, foot_w) = form_frame(
        f,
        area,
        &format!("Review #{}", form.number),
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
    let muted = Style::default().fg(th.muted);
    let on = |row: ReviewRow| focused && form.row == row && form.saving.is_none();
    let others = form.verdicts().len() > 1;
    let tint = match form.verdict {
        ReviewVerdict::Approve => th.ok,
        ReviewVerdict::RequestChanges => th.err,
        ReviewVerdict::Comment => th.text,
    };
    let about = if form.mine {
        "your own pull request — GitHub takes only a comment from its author"
    } else {
        "your review, posted to GitHub"
    };
    let mut verdict = vec![
        form_label("Verdict", on(ReviewRow::Verdict), th),
        Span::styled(if others { "◂ " } else { "  " }, muted),
        Span::styled(
            form.verdict.label(),
            Style::default().fg(tint).add_modifier(Modifier::BOLD),
        ),
    ];
    if others {
        verdict.push(Span::styled(" ▸", muted));
    }
    let lines: [(Option<ReviewRow>, Vec<Span<'static>>); REVIEW_ROWS as usize] = [
        (
            None,
            vec![Span::styled(
                format!("{INDENT}{}", form.title),
                text.add_modifier(Modifier::BOLD),
            )],
        ),
        (None, vec![Span::styled(format!("{INDENT}{about}"), dim)]),
        (None, Vec::new()),
        (Some(ReviewRow::Verdict), verdict),
    ];
    for (i, (row, spans)) in lines.into_iter().enumerate() {
        let Some(rect) = row_rect(inner, i) else {
            break;
        };
        f.render_widget(Paragraph::new(crate::pr_preview::fit(spans, width)), rect);
        if let Some(row) = row {
            drawn.review_rows.push((rect, row));
        }
    }
    let box_area = Rect {
        y: inner.y.saturating_add(REVIEW_ROWS),
        height: inner.height.saturating_sub(REVIEW_ROWS),
        ..inner
    };
    if box_area.height < 3 || box_area.width < 4 {
        return drawn;
    }
    drawn.review_rows.push((box_area, ReviewRow::Body));
    drawn.body_view = form_text_box(
        f,
        box_area,
        "Comment",
        &form.body,
        on(ReviewRow::Body),
        if form.verdict.needs_body() {
            "(required — what the review says)"
        } else {
            "(optional — left with the approval)"
        },
        th,
    );
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
            delete_default: false,
            auto_delete: false,
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

    /// A branch field holding a branch takes the first letter typed — or a
    /// paste — in its place, a search for another; what follows is added
    /// to it. Nothing typed into the title clears it.
    #[test]
    fn typing_in_a_branch_field_starts_a_new_search() {
        use crossterm::event::KeyModifiers;
        let mut app = App::new();
        let mut view =
            PullRequestsView::new(ProjectId("p".into()), "demo".into(), PathBuf::from("/x"));
        let mut form = CreateForm::new(
            ProjectId("p".into()),
            PathBuf::from("/x"),
            "feature".into(),
            false,
        );
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
