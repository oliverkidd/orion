//! The DIFF VIEWER's COMMIT LIST: the branch's own commits, newest first,
//! so a branch an agent landed as several commits reads one commit at a
//! time.
//!
//! **What the branch is measured against** is where orion cuts its
//! worktrees from (`git::add_worktree_off_default` in the DAEMON): the
//! `worktree_base_branch` SETTING when it names a branch this repo has —
//! origin's copy first, then a local one — else `origin/HEAD`, else the
//! branch the ROOT WORKTREE is on, which is what a worktree is cut from in
//! a repo with no origin. The DAEMON fetches before it cuts; this reads
//! what the checkout already has and never touches the network. The
//! branch's own commits are `git log <merge-base>..HEAD` — HEAD is all it
//! needs, so a detached checkout lists the same way.
//!
//! **The rows**, top to bottom: **All changes** — the merge-base against
//! the working tree, everything the branch changed, committed or not —
//! while the branch has a commit of its own; **Uncommitted changes**, the
//! view the DIFF VIEWER always was, while the checkout is dirty; the
//! commits; and `… N older commits` past the last page read. Each row is a
//! [`DiffScope`]: choosing one lists its files ([`show_selected`]) and every
//! diff walked under it is taken against it (`git_diff::scoped_diff`). The
//! scope on screen changes only when its file list is in hand, so the
//! files listed and the diffs read for them always agree.
//!
//! **A long branch is read a page at a time.** The first [`COMMIT_PAGE`]
//! commits, with their counts, come back with the viewer's own listing
//! (`git_diff::read_opening`); the `older` row asks for the next page when
//! the cursor reaches it. Every read here is a BACKGROUND READ
//! (`view_jobs`), inline only in a view built without a handle.

use crate::app::{clamp_selection, window_start, DiffView};
use crate::git_diff::{run_git, DiffFile, DiffScope, LineChanges};
use crate::view_jobs::{Answer, DiffListing};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Commits read per page: the first comes with the viewer's listing, each
/// later one when the cursor reaches the `older` row. `git log` diffs
/// every commit it lists to count its lines, so a three-hundred-commit
/// branch is never diffed whole to show the top of it.
pub const COMMIT_PAGE: usize = 50;

/// `git log`'s format: a record mark, the fields between field marks, and
/// an end mark after the body, where `--numstat` takes over. Control
/// characters no name, subject or message carries, so nothing is quoted.
const LOG_FORMAT: &str = "--format=%x1e%H%x1f%h%x1f%P%x1f%aN%x1f%at%x1f%s%x1f%b%x1d";

/// The fields [`LOG_FORMAT`] separates, the body last: split no further,
/// so a body that holds the separator stays whole.
const LOG_FIELDS: usize = 7;
const RECORD: char = '\x1e';
const FIELD: char = '\x1f';
const BODY_END: char = '\x1d';

/// How much a row changed. `lines` is None where git couldn't say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stat {
    pub files: usize,
    pub lines: Option<LineChanges>,
}

/// One commit of the branch, as `git log` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    /// The abbreviated sha git prints for it.
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Author date, unix seconds.
    pub time: i64,
    pub subject: String,
    /// The message past its subject; empty for a one-line message.
    pub body: String,
    /// None for a merge: `git log` diffs no merge, so it has no counts of
    /// its own to show.
    pub stat: Option<Stat>,
}

impl Commit {
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// What choosing this commit shows: it against its first parent.
    pub fn scope(&self) -> DiffScope {
        DiffScope::Commit {
            sha: self.sha.clone(),
            parent: self.parents.first().cloned(),
        }
    }

    /// "2h ago", from `now_ms`.
    pub fn ago(&self, now_ms: i64) -> String {
        crate::hosts::ago_label(now_ms - self.time.saturating_mul(1000))
    }

    /// The commit's message as the diff pane heads each of its files with
    /// it: who and when, the subject, and the body.
    pub fn header(&self, now_ms: i64) -> Vec<String> {
        let mut meta = vec![self.short.clone(), self.author.clone(), self.ago(now_ms)];
        if self.is_merge() {
            meta.push("merge, shown against its first parent".to_string());
        }
        meta.retain(|part| !part.is_empty());
        let mut lines = vec![meta.join(" · "), String::new(), self.subject.clone()];
        if !self.body.is_empty() {
            lines.push(String::new());
            lines.extend(self.body.lines().map(str::to_string));
        }
        lines
    }
}

/// Everything the COMMIT LIST opens on — read in the same job as the
/// viewer's `git status` (`git_diff::read_opening`).
#[derive(Debug, Clone, Default)]
pub struct CommitListing {
    /// The base as named — `origin/main`, `develop`; None when there is
    /// nothing to measure the branch against.
    pub base: Option<String>,
    /// Where HEAD left the base; None with no base, an unborn HEAD, or no
    /// history in common.
    pub merge_base: Option<String>,
    /// Commits since the merge-base, however many of them were read.
    pub total: usize,
    /// The first page of them, newest first.
    pub commits: Vec<Commit>,
    /// The **All changes** row's counts.
    pub branch: Option<Stat>,
    /// The **Uncommitted changes** row's counts; None while the checkout is
    /// clean, and then that row is not there.
    pub uncommitted: Option<Stat>,
}

/// Read the COMMIT LIST for the checkout at `root`, whose uncommitted
/// changes `git status` found to be `uncommitted`: the base, the merge-base,
/// the first page of commits and the counts of the rows above them.
pub fn read(root: &Path, base_setting: &str, uncommitted: &[DiffFile]) -> CommitListing {
    let mut listing = CommitListing {
        uncommitted: (!uncommitted.is_empty()).then(|| Stat {
            files: uncommitted.len(),
            lines: crate::git_diff::line_changes(root, uncommitted),
        }),
        ..CommitListing::default()
    };
    // An unborn HEAD has no commit to list and nothing to merge-base.
    if crate::git_diff::head_oid(root).is_none() {
        return listing;
    }
    listing.base = resolve_base(root, base_setting);
    let Some(merge_base) = listing.base.as_deref().and_then(|b| merge_base(root, b)) else {
        return listing;
    };
    let total = count_since(root, &merge_base);
    if total > 0 {
        listing.commits = read_page(root, &merge_base, 0).unwrap_or_else(|err| {
            tracing::warn!(root = %root.display(), "{err}");
            Vec::new()
        });
        // A log that listed nothing leaves nothing to page through.
        listing.total = if listing.commits.is_empty() { 0 } else { total };
        listing.branch =
            crate::git_diff::changes_since(root, &merge_base, uncommitted).map(|(files, lines)| {
                Stat {
                    files,
                    lines: Some(lines),
                }
            });
    }
    listing.merge_base = Some(merge_base);
    listing
}

/// The ref the branch is measured against — see the module doc for the
/// order. The setting is read the way the DAEMON reads it: trimmed, a
/// leading `origin/` meaning the same as the bare name.
pub fn resolve_base(root: &Path, base_setting: &str) -> Option<String> {
    let name = base_setting.trim();
    let name = name.strip_prefix("origin/").unwrap_or(name).trim();
    if !name.is_empty() && name != "HEAD" {
        if has_commit(root, &format!("refs/remotes/origin/{name}")) {
            return Some(format!("origin/{name}"));
        }
        if has_commit(root, &format!("refs/heads/{name}")) {
            return Some(name.to_string());
        }
    }
    git_line(
        root,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
    )
    .or_else(|| root_branch(root))
}

/// The branch the ROOT WORKTREE has checked out — `git worktree list`
/// always prints the main checkout first. None while it is detached.
fn root_branch(root: &Path) -> Option<String> {
    let output = run_git(root, &["worktree", "list", "--porcelain"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let main = text.split("\n\n").next()?;
    main.lines()
        .find_map(|line| line.strip_prefix("branch refs/heads/"))
        .map(str::to_string)
}

fn has_commit(root: &Path, full_ref: &str) -> bool {
    let spec = format!("{full_ref}^{{commit}}");
    git_line(root, &["rev-parse", "--verify", "--quiet", &spec]).is_some()
}

fn merge_base(root: &Path, base: &str) -> Option<String> {
    git_line(root, &["merge-base", "HEAD", base])
}

/// How many commits HEAD has past `merge_base` — no diffs, so it costs the
/// same for three commits as for three hundred.
fn count_since(root: &Path, merge_base: &str) -> usize {
    let range = format!("{merge_base}..HEAD");
    git_line(root, &["rev-list", "--count", &range])
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// What a git that succeeded printed, trimmed; None for a failure or for
/// nothing printed.
fn git_line(root: &Path, args: &[&str]) -> Option<String> {
    let output = run_git(root, args).ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// One page of the branch's commits, newest first: the `skip` newest left
/// out, [`COMMIT_PAGE`] at most, each with its counts. `Err` is a
/// user-facing message.
pub fn read_page(root: &Path, merge_base: &str, skip: usize) -> Result<Vec<Commit>, String> {
    let range = format!("{merge_base}..HEAD");
    let max = format!("--max-count={COMMIT_PAGE}");
    let skip = format!("--skip={skip}");
    let output = run_git(
        root,
        &[
            "log",
            LOG_FORMAT,
            "--numstat",
            "--no-color",
            "--no-show-signature",
            &max,
            &skip,
            &range,
            "--",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git log failed: {}", stderr.trim()));
    }
    Ok(parse_log(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse [`LOG_FORMAT`] with `--numstat`: per commit, a record mark, its
/// fields, the body up to the end mark, and then one `added\tremoved\tpath`
/// line per file it changed (`-\t-` for a binary one, which adds nothing).
pub fn parse_log(text: &str) -> Vec<Commit> {
    text.split(RECORD)
        .filter_map(|record| {
            let (head, numstat) = record.split_once(BODY_END)?;
            let mut fields = head.splitn(LOG_FIELDS, FIELD);
            let sha = fields.next()?.trim().to_string();
            let short = fields.next()?.to_string();
            let parents: Vec<String> = fields
                .next()?
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let author = fields.next()?.to_string();
            let time = fields.next()?.parse().unwrap_or(0);
            let subject = fields.next()?.to_string();
            let body = fields.next().unwrap_or_default().trim_end().to_string();
            if sha.is_empty() {
                return None;
            }
            let stat = (parents.len() <= 1).then(|| numstat_stat(numstat));
            Some(Commit {
                sha,
                short,
                parents,
                author,
                time,
                subject,
                body,
                stat,
            })
        })
        .collect()
}

/// The files and lines of one commit's `--numstat` lines.
fn numstat_stat(text: &str) -> Stat {
    let mut files = 0;
    let mut lines = LineChanges::default();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(removed), Some(_path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        files += 1;
        lines.added += added.parse::<u64>().unwrap_or(0);
        lines.removed += removed.parse::<u64>().unwrap_or(0);
    }
    Stat {
        files,
        lines: Some(lines),
    }
}

/// One row of the COMMIT LIST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// **All changes**: the merge-base against the working tree.
    Branch,
    /// **Uncommitted changes**: the working tree against HEAD.
    Uncommitted,
    /// The commit at this index of `CommitList::commits`.
    Commit(usize),
    /// `… N older commits`: the next page, read when the cursor gets here.
    Older,
}

/// The COMMIT LIST in an open DIFF VIEWER: what [`CommitListing`] read,
/// the cursor, and the page in flight.
#[derive(Debug, Clone, Default)]
pub struct CommitList {
    /// False until the listing lands; the strip says `reading commits…`.
    pub loaded: bool,
    pub base: Option<String>,
    pub merge_base: Option<String>,
    pub total: usize,
    /// Newest first. Shared with every frame's clone of the view rather
    /// than copied into it.
    pub commits: Arc<Vec<Commit>>,
    pub branch: Option<Stat>,
    pub uncommitted: Option<Stat>,
    /// Index into the rows ([`CommitList::row`]).
    pub selected: usize,
    /// The next page in flight, by ticket.
    pub paging: Option<u64>,
    /// The strip, and its rows, as last drawn — for the pointer.
    pub area: Rect,
    pub list_area: Rect,
}

impl CommitList {
    /// The list a view opens with, before its listing lands.
    pub fn reading() -> Self {
        Self::default()
    }

    /// The listing landed. The cursor starts on what the view shows first:
    /// the uncommitted changes while there are any, else the branch's.
    pub fn from_listing(listing: CommitListing) -> Self {
        let mut list = Self {
            loaded: true,
            base: listing.base,
            merge_base: listing.merge_base,
            total: listing.total,
            commits: Arc::new(listing.commits),
            branch: listing.branch,
            uncommitted: listing.uncommitted,
            ..Self::default()
        };
        list.selected = list.index_of(Row::Uncommitted).unwrap_or(0);
        list
    }

    fn has_branch(&self) -> bool {
        !self.commits.is_empty() && self.merge_base.is_some()
    }

    /// Whether commits past the last page read are still to come.
    pub fn has_older(&self) -> bool {
        !self.commits.is_empty() && self.commits.len() < self.total
    }

    pub fn row_count(&self) -> usize {
        usize::from(self.has_branch())
            + usize::from(self.uncommitted.is_some())
            + self.commits.len()
            + usize::from(self.has_older())
    }

    /// The row at `index`, top to bottom.
    pub fn row(&self, index: usize) -> Option<Row> {
        let mut i = index;
        for (present, row) in [
            (self.has_branch(), Row::Branch),
            (self.uncommitted.is_some(), Row::Uncommitted),
        ] {
            if present {
                if i == 0 {
                    return Some(row);
                }
                i -= 1;
            }
        }
        if i < self.commits.len() {
            return Some(Row::Commit(i));
        }
        (i == self.commits.len() && self.has_older()).then_some(Row::Older)
    }

    pub fn index_of(&self, row: Row) -> Option<usize> {
        (0..self.row_count()).find(|&i| self.row(i) == Some(row))
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.row(self.selected)
    }

    /// Clamped absolute selection; true when it moved.
    pub fn select(&mut self, index: i64) -> bool {
        let clamped = clamp_selection(index, self.row_count());
        let changed = clamped != self.selected;
        self.selected = clamped;
        changed
    }

    /// What choosing `row` shows; None for the `older` row, which is a
    /// page to read rather than something to show.
    pub fn scope_of(&self, row: Row) -> Option<DiffScope> {
        match row {
            Row::Branch => Some(DiffScope::Branch {
                merge_base: self.merge_base.clone()?,
            }),
            Row::Uncommitted => Some(DiffScope::Uncommitted),
            Row::Commit(i) => self.commits.get(i).map(Commit::scope),
            Row::Older => None,
        }
    }

    /// The commit a scope is of, when it is one of this list's.
    pub fn commit_of(&self, scope: &DiffScope) -> Option<&Commit> {
        match scope {
            DiffScope::Commit { sha, .. } => self.commits.iter().find(|c| &c.sha == sha),
            _ => None,
        }
    }

    /// First row of the strip's stateless follow-window.
    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.selected, height)
    }
}

/// Put the COMMIT LIST read with the viewer's opening into `view`. A
/// checkout with nothing uncommitted has its branch's changes put up
/// instead, the cursor on **All changes**.
pub fn install(view: &mut DiffView, listing: CommitListing) {
    view.commits = Some(CommitList::from_listing(listing));
    if view.files.is_empty() && view.scope == DiffScope::Uncommitted {
        show_selected(view);
    }
}

/// The COMMIT LIST's cursor moved: put its row up. The `older` row asks
/// for the next page; any other for its file list — unless it is the scope
/// on screen already, with no other row's list asked for since.
pub fn show_selected(view: &mut DiffView) {
    let Some(list) = &view.commits else {
        return;
    };
    let Some(row) = list.selected_row() else {
        return;
    };
    if row == Row::Older {
        request_page(view);
        return;
    }
    let Some(scope) = list.scope_of(row) else {
        return;
    };
    if scope == view.scope && view.listing.is_none() {
        return;
    }
    request_scope(view, scope);
}

/// Ask for a row's file list. The files and the diff on screen stay up
/// meanwhile — `view_jobs::STALE_GRACE`, then [`listing_slow`] — and the
/// answer lands in [`land_scope`].
fn request_scope(view: &mut DiffView, scope: DiffScope) {
    let ticket = crate::view_jobs::ticket();
    view.listing = Some(ticket);
    let root = view.root.clone();
    let Some(jobs) = view.jobs.clone() else {
        let result = read_scope(&root, &scope);
        land_scope(view, ticket, scope, result);
        return;
    };
    jobs.run_with_grace(ticket, move || {
        let result = read_scope(&root, &scope);
        Some(Answer::ScopeFiles {
            ticket,
            scope,
            result,
        })
    });
}

/// A row's file list: the uncommitted changes the way `g` reads them —
/// `read_listing`, the stored ✓ marks restored — and any other row's
/// through `scope_files`.
fn read_scope(root: &Path, scope: &DiffScope) -> Result<DiffListing, String> {
    if *scope == DiffScope::Uncommitted {
        return crate::git_diff::read_listing(root);
    }
    Ok(DiffListing {
        files: crate::git_diff::scope_files(root, scope)?,
        head: None,
        reviewed: HashMap::new(),
        commits: None,
    })
}

/// A row's file list came back. When it is the one the view waits on, its
/// scope becomes the one on screen: the files replaced — the filter kept,
/// so a typed path follows the reader from commit to commit — the cursor
/// home on the first unreviewed file, a commit's message put up as the
/// diff's header, and that file's diff read.
///
/// The uncommitted changes' ✓ marks are stored on disk and come back with
/// their listing; any other row's are kept for as long as the modal is up,
/// put away while another row is on screen and brought back with it.
pub fn land_scope(
    view: &mut DiffView,
    ticket: u64,
    scope: DiffScope,
    result: Result<DiffListing, String>,
) {
    if view.listing != Some(ticket) {
        return;
    }
    view.listing = None;
    let (listing, error) = match result {
        Ok(listing) => (Some(listing), None),
        Err(msg) => (None, Some(msg)),
    };
    let old = std::mem::replace(&mut view.scope, scope);
    let marks = std::mem::take(&mut view.reviewed);
    if old != DiffScope::Uncommitted && !marks.is_empty() {
        view.scope_marks.insert(old, marks);
    }
    let files = match listing {
        Some(listing) if view.scope == DiffScope::Uncommitted => {
            view.head_ok = listing.head.is_some();
            view.head_key = listing.head.unwrap_or_default();
            view.reviewed = listing.reviewed;
            listing.files
        }
        listing => {
            let files = listing.map(|l| l.files).unwrap_or_default();
            let mut marks = view.scope_marks.remove(&view.scope).unwrap_or_default();
            marks.retain(|path, _| files.iter().any(|f| &f.path == path));
            view.reviewed = marks;
            files
        }
    };
    let now = crate::app::now_ms();
    view.header = view
        .commits
        .as_ref()
        .and_then(|list| list.commit_of(&view.scope))
        .map(|commit| commit.header(now))
        .unwrap_or_default();
    view.header_read = false;
    // A diff read under the last scope must never land under this one's
    // name: a fresh id drops it, and the cache was of that scope too.
    view.id = crate::view_jobs::ticket();
    view.cache.clear();
    view.waiting = None;
    view.replace_files(files);
    view.show_diff(None, String::new(), false);
    match error {
        Some(msg) => view.show_diff(None, msg, false),
        None => crate::git_diff::load_selected_diff(view),
    }
}

/// A row's file list has outlasted `view_jobs::STALE_GRACE`: the last
/// row's files come down and the list says `reading changes…`, rather
/// than leave them under the new row's name.
pub fn listing_slow(view: &mut DiffView, ticket: u64) {
    if view.listing != Some(ticket) || view.files.is_empty() {
        return;
    }
    view.waiting = None;
    view.header.clear();
    view.replace_files(Vec::new());
    view.show_diff(None, "loading…".to_string(), false);
}

/// Read the page after the last one, once: a page already in flight, or
/// none left, asks for nothing.
fn request_page(view: &mut DiffView) {
    let root = view.root.clone();
    let jobs = view.jobs.clone();
    let Some(list) = &mut view.commits else {
        return;
    };
    let Some(merge_base) = list.merge_base.clone() else {
        return;
    };
    if list.paging.is_some() || !list.has_older() {
        return;
    }
    let skip = list.commits.len();
    let ticket = crate::view_jobs::ticket();
    list.paging = Some(ticket);
    match jobs {
        Some(jobs) => jobs.run(move || {
            Some(Answer::CommitPage {
                ticket,
                result: read_page(&root, &merge_base, skip),
            })
        }),
        None => {
            let result = read_page(&root, &merge_base, skip);
            land_page(view, ticket, result);
        }
    }
}

/// The next page of commits came back. It joins the list, and a cursor
/// that was waiting on the `older` row — now the page's first commit —
/// has that commit put up. A page that read nothing (or failed) ends the
/// list where it is.
pub fn land_page(view: &mut DiffView, ticket: u64, result: Result<Vec<Commit>, String>) {
    let Some(list) = &mut view.commits else {
        return;
    };
    if list.paging != Some(ticket) {
        return;
    }
    list.paging = None;
    match result {
        Ok(page) if !page.is_empty() => Arc::make_mut(&mut list.commits).extend(page),
        Ok(_) => list.total = list.commits.len(),
        Err(err) => {
            tracing::warn!("{err}");
            list.total = list.commits.len();
        }
    }
    list.select(list.selected as i64);
    show_selected(view);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo on `main` with one commit, `origin/HEAD` pointing at it — the
    /// shape a worktree cut by the DAEMON sees — and `feat` checked out on
    /// top of it.
    fn branch_repo(dir: &tempfile::TempDir) -> PathBuf {
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "Tess"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "init"]);
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        git(&repo, &["checkout", "-qb", "feat"]);
        repo
    }

    fn commit(repo: &Path, file: &str, text: &str, message: &str) {
        std::fs::write(repo.join(file), text).unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-qm", message]);
    }

    #[test]
    fn parse_log_reads_fields_body_and_counts() {
        let text = "\x1eaaa111\x1faaa\x1fppp\x1fTess\x1f1700000000\x1fAdd retry\x1fWhy:\n- the hook drops\n\n\x1d\n\n3\t1\tsrc/a.rs\n-\t-\tlogo.png\n10\t0\tsrc/{old => new}.rs\n\
                    \x1ebbb222\x1fbbb\x1fp1 p2\x1fAnn\x1f1700000100\x1fMerge main\x1f\x1d\n\
                    \x1eccc333\x1fccc\x1f\x1fTess\x1f1690000000\x1fEmpty\x1f\x1d\n";
        let commits = parse_log(text);
        assert_eq!(commits.len(), 3);
        let first = &commits[0];
        assert_eq!(first.sha, "aaa111");
        assert_eq!(first.short, "aaa");
        assert_eq!(first.parents, vec!["ppp"]);
        assert_eq!(first.author, "Tess");
        assert_eq!(first.time, 1_700_000_000);
        assert_eq!(first.subject, "Add retry");
        assert_eq!(
            first.body, "Why:\n- the hook drops",
            "trailing blanks trimmed"
        );
        assert_eq!(
            first.stat,
            Some(Stat {
                files: 3,
                lines: Some(LineChanges {
                    added: 13,
                    removed: 1
                })
            }),
            "a binary file counts as a file and adds no lines"
        );
        assert!(commits[1].is_merge());
        assert_eq!(commits[1].stat, None, "a merge has no counts of its own");
        assert_eq!(commits[2].parents, Vec::<String>::new(), "a root commit");
        assert_eq!(commits[2].stat.map(|s| s.files), Some(0), "an empty commit");
        assert_eq!(
            commits[2].scope(),
            DiffScope::Commit {
                sha: "ccc333".into(),
                parent: None
            }
        );
    }

    #[test]
    fn the_header_is_who_and_when_then_the_whole_message() {
        let commit = Commit {
            sha: "abc".into(),
            short: "abc".into(),
            parents: vec!["p".into()],
            author: "Tess".into(),
            time: 1_000,
            subject: "Add retry".into(),
            body: "- one\n- two".into(),
            stat: None,
        };
        assert_eq!(
            commit.header(1_000_000 + 3 * 3_600_000),
            vec!["abc · Tess · 3h ago", "", "Add retry", "", "- one", "- two"]
        );
        let bare = Commit {
            body: String::new(),
            parents: vec!["p1".into(), "p2".into()],
            ..commit
        };
        assert_eq!(
            bare.header(1_000_000),
            vec![
                "abc · Tess · just now · merge, shown against its first parent",
                "",
                "Add retry"
            ]
        );
    }

    /// The base is the setting's branch when the repo has one — origin's
    /// copy first, `origin/` spelt out or not — else `origin/HEAD`, else
    /// the ROOT WORKTREE's branch, and a detached root has none.
    #[test]
    fn resolve_base_follows_the_daemons_order() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        git(&repo, &["branch", "develop", "main"]);
        assert_eq!(resolve_base(&repo, ""), Some("origin/main".into()));
        assert_eq!(resolve_base(&repo, "  "), Some("origin/main".into()));
        assert_eq!(
            resolve_base(&repo, "develop"),
            Some("develop".into()),
            "a local branch when origin has none"
        );
        git(
            &repo,
            &["update-ref", "refs/remotes/origin/develop", "main"],
        );
        assert_eq!(
            resolve_base(&repo, "origin/develop"),
            Some("origin/develop".into()),
            "origin's copy wins, however it is spelt"
        );
        assert_eq!(
            resolve_base(&repo, "no-such-branch"),
            Some("origin/main".into()),
            "a name the repo lacks falls back"
        );

        // No origin at all: the branch the root checkout is on — from a
        // linked worktree too.
        git(
            &repo,
            &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
        );
        assert_eq!(resolve_base(&repo, ""), Some("feat".into()));
        let linked = dir.path().join("linked");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                linked.to_str().unwrap(),
                "main",
            ],
        );
        assert_eq!(resolve_base(&linked, ""), Some("feat".into()));
        git(&repo, &["checkout", "-q", "--detach"]);
        assert_eq!(
            resolve_base(&linked, ""),
            None,
            "a detached root names no branch"
        );
    }

    /// The commits since the base, newest first with their counts, and the
    /// rows above them: the whole branch counted to the working tree, its
    /// untracked file included, and the uncommitted changes.
    #[test]
    fn read_lists_the_branch_and_counts_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        commit(&repo, "a.txt", "1\n2\n", "add a");
        commit(&repo, "b.txt", "1\n", "add b");
        std::fs::write(repo.join("a.txt"), "1\n").unwrap();
        std::fs::write(repo.join("new.txt"), "x\ny\nz\n").unwrap();
        let dirty = crate::git_diff::changed_files(&repo).unwrap();

        let listing = read(&repo, "", &dirty);
        assert_eq!(listing.base.as_deref(), Some("origin/main"));
        assert_eq!(listing.total, 2);
        let subjects: Vec<&str> = listing.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["add b", "add a"], "newest first");
        assert_eq!(
            listing.commits[1].stat,
            Some(Stat {
                files: 1,
                lines: Some(LineChanges {
                    added: 2,
                    removed: 0
                })
            })
        );
        assert_eq!(
            listing.branch,
            Some(Stat {
                files: 3,
                lines: Some(LineChanges {
                    added: 5,
                    removed: 0
                })
            }),
            "a.txt's one line, b.txt's and new.txt's three"
        );
        assert_eq!(
            listing.uncommitted,
            Some(Stat {
                files: 2,
                lines: Some(LineChanges {
                    added: 3,
                    removed: 1
                })
            })
        );

        // Detached on the same commit, the list is the same.
        git(&repo, &["checkout", "-q", "--detach"]);
        let detached = read(&repo, "", &dirty);
        assert_eq!(detached.total, 2);
        assert_eq!(detached.commits[0].subject, "add b");
    }

    #[test]
    fn read_says_nothing_is_ahead_and_survives_an_unborn_head() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        let listing = read(&repo, "", &[]);
        assert_eq!(listing.base.as_deref(), Some("origin/main"));
        assert!(listing.merge_base.is_some());
        assert_eq!(listing.total, 0);
        assert!(listing.commits.is_empty());
        assert_eq!(listing.uncommitted, None, "a clean checkout");

        let unborn = dir.path().join("unborn");
        std::fs::create_dir(&unborn).unwrap();
        git(&unborn, &["init", "-q", "-b", "main"]);
        let listing = read(&unborn, "", &[]);
        assert_eq!(listing.base, None);
        assert_eq!(listing.total, 0);
    }

    /// A long branch is read a page at a time: the first page with the
    /// listing, the next when the `older` row is reached.
    #[test]
    fn a_long_branch_is_read_a_page_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        for n in 0..COMMIT_PAGE + 3 {
            git(
                &repo,
                &["commit", "-q", "--allow-empty", "-m", &format!("c{n}")],
            );
        }
        let listing = read(&repo, "", &[]);
        assert_eq!(listing.total, COMMIT_PAGE + 3);
        assert_eq!(listing.commits.len(), COMMIT_PAGE);
        let mut view = DiffView::new(repo.clone(), "feat".into(), Vec::new(), true);
        install(&mut view, listing);
        let list = view.commits.as_ref().unwrap();
        assert_eq!(list.row(list.row_count() - 1), Some(Row::Older));

        // The cursor reaches the `older` row: the page is read (inline,
        // with no jobs) and the cursor is on its first commit, shown.
        let last = view.commits.as_ref().unwrap().row_count() as i64 - 1;
        view.commits.as_mut().unwrap().select(last);
        show_selected(&mut view);
        let list = view.commits.as_ref().unwrap();
        assert_eq!(list.commits.len(), COMMIT_PAGE + 3);
        assert!(!list.has_older());
        let Some(Row::Commit(i)) = list.selected_row() else {
            panic!("the cursor is on a commit");
        };
        assert_eq!(list.commits[i].subject, "c2");
        assert_eq!(view.scope, list.commits[i].scope());
    }

    fn list_of(commits: usize, dirty: bool, total: usize) -> CommitList {
        let commit = |n: usize| Commit {
            sha: format!("sha{n}"),
            short: format!("s{n}"),
            parents: vec![format!("sha{}", n + 1)],
            author: "Tess".into(),
            time: 0,
            subject: format!("commit {n}"),
            body: String::new(),
            stat: None,
        };
        CommitList::from_listing(CommitListing {
            base: Some("origin/main".into()),
            merge_base: Some("mb".into()),
            total,
            commits: (0..commits).map(commit).collect(),
            branch: None,
            uncommitted: dirty.then(Stat::default),
        })
    }

    /// The rows run All, Uncommitted, the commits newest first, then the
    /// `older` row; each is missing when there is nothing behind it.
    #[test]
    fn the_rows_are_only_the_ones_with_something_behind_them() {
        let rows = |list: &CommitList| -> Vec<Row> {
            (0..list.row_count()).filter_map(|i| list.row(i)).collect()
        };
        let full = list_of(2, true, 5);
        assert_eq!(
            rows(&full),
            [
                Row::Branch,
                Row::Uncommitted,
                Row::Commit(0),
                Row::Commit(1),
                Row::Older
            ]
        );
        assert_eq!(
            full.selected_row(),
            Some(Row::Uncommitted),
            "a dirty checkout opens on it"
        );
        assert_eq!(full.row(5), None);

        let clean = list_of(2, false, 2);
        assert_eq!(rows(&clean), [Row::Branch, Row::Commit(0), Row::Commit(1)]);
        assert_eq!(
            clean.selected_row(),
            Some(Row::Branch),
            "a clean one on the branch"
        );

        let nothing_ahead = list_of(0, true, 0);
        assert_eq!(
            rows(&nothing_ahead),
            [Row::Uncommitted],
            "no All without a commit"
        );
        assert_eq!(CommitList::reading().row_count(), 0);
    }

    #[test]
    fn each_row_names_what_it_shows() {
        let list = list_of(2, true, 3);
        assert_eq!(
            list.scope_of(Row::Branch),
            Some(DiffScope::Branch {
                merge_base: "mb".into()
            })
        );
        assert_eq!(
            list.scope_of(Row::Uncommitted),
            Some(DiffScope::Uncommitted)
        );
        assert_eq!(
            list.scope_of(Row::Commit(1)),
            Some(DiffScope::Commit {
                sha: "sha1".into(),
                parent: Some("sha2".into())
            })
        );
        assert_eq!(list.scope_of(Row::Older), None);
        let scope = list.scope_of(Row::Commit(0)).unwrap();
        assert_eq!(
            list.commit_of(&scope).map(|c| c.subject.as_str()),
            Some("commit 0")
        );
        assert_eq!(list.index_of(Row::Commit(0)), Some(2));
    }

    // ---- against a real checkout, through a view with no jobs ----

    fn opened(repo: &Path) -> DiffView {
        let listing = crate::git_diff::read_opening(repo, "").unwrap();
        let mut view = DiffView::new(repo.to_path_buf(), "feat".into(), Vec::new(), true);
        crate::git_diff::fill_view(&mut view, listing);
        view
    }

    fn paths(view: &DiffView) -> Vec<&str> {
        view.files.iter().map(|f| f.path.as_str()).collect()
    }

    fn select(view: &mut DiffView, row: Row) {
        let list = view.commits.as_mut().unwrap();
        let index = list.index_of(row).unwrap();
        list.select(index as i64);
        show_selected(view);
    }

    /// A clean checkout with commits opens on the whole branch; a commit's
    /// row lists exactly its files, renames and deletes too, with its
    /// message over the diff.
    #[test]
    fn a_commit_shows_exactly_its_own_files_under_its_message() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        commit(&repo, "a.txt", "alpha\n", "add a");
        git(&repo, &["mv", "base.txt", "moved.txt"]);
        std::fs::write(repo.join("b.txt"), "beta\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(
            &repo,
            &["commit", "-qm", "move base, add b", "-m", "- the body"],
        );

        let mut view = opened(&repo);
        assert_eq!(
            view.scope,
            DiffScope::Branch {
                merge_base: view.commits.as_ref().unwrap().merge_base.clone().unwrap()
            },
            "nothing uncommitted: the branch's changes"
        );
        assert_eq!(paths(&view), ["a.txt", "b.txt", "moved.txt"]);
        assert!(
            view.header.is_empty(),
            "the branch has no message of its own"
        );

        select(&mut view, Row::Commit(0));
        assert_eq!(paths(&view), ["b.txt", "moved.txt"]);
        let moved = view.files.iter().find(|f| f.path == "moved.txt").unwrap();
        assert_eq!(moved.orig_path.as_deref(), Some("base.txt"));
        assert_eq!(moved.xy[0], 'R');
        assert_eq!(view.header[2], "move base, add b");
        assert_eq!(view.header[4], "- the body");
        assert!(view.diff.contains("+beta"), "{}", view.diff);
        assert_eq!(view.scroll, 0, "the first file opens on the message");

        // The next file of the same commit opens past the message, which
        // is a scroll up.
        view.select(1);
        crate::git_diff::load_selected_diff(&mut view);
        assert!(view.diff.contains("rename from base.txt"), "{}", view.diff);
        assert_eq!(view.scroll as usize, view.header_rows());

        select(&mut view, Row::Commit(1));
        assert_eq!(paths(&view), ["a.txt"]);
        assert!(view.diff.contains("+alpha"));
        assert_eq!(view.scroll, 0, "a new commit opens on its message");
    }

    /// ✓ marks taken on a commit last while the modal is up: put away
    /// while another row is on screen, back with it; the uncommitted
    /// changes' are the stored ones, untouched by either.
    #[test]
    fn marks_follow_their_row() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        commit(&repo, "a.txt", "alpha\n", "add a");
        std::fs::write(repo.join("dirty.txt"), "wip\n").unwrap();
        crate::review::with_store_path(dir.path().join("reviewed.json"), || {
            let mut view = opened(&repo);
            assert_eq!(
                view.scope,
                DiffScope::Uncommitted,
                "dirty: the view it always was"
            );
            assert_eq!(paths(&view), ["dirty.txt"]);

            select(&mut view, Row::Commit(0));
            assert_eq!(view.toggle_reviewed(), Some(false));
            assert!(view.reviewed.contains_key("a.txt"));

            select(&mut view, Row::Uncommitted);
            assert!(
                view.reviewed.is_empty(),
                "the commit's mark is not the checkout's"
            );
            select(&mut view, Row::Commit(0));
            assert!(view.reviewed.contains_key("a.txt"), "and it came back");
        });
    }

    /// An answer for a row the cursor has since left is dropped; the one
    /// it waits on lands.
    #[test]
    fn a_stale_listing_never_lands() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        commit(&repo, "a.txt", "alpha\n", "add a");
        let mut view = opened(&repo);
        let before = view.scope.clone();
        view.listing = Some(2);
        let other = view.commits.as_ref().unwrap().commits[0].scope();
        let files = |paths: &[&str]| DiffListing {
            files: paths
                .iter()
                .map(|p| DiffFile {
                    path: p.to_string(),
                    orig_path: None,
                    xy: ['M', ' '],
                })
                .collect(),
            head: None,
            reviewed: HashMap::new(),
            commits: None,
        };
        land_scope(&mut view, 1, other.clone(), Ok(files(&["late.rs"])));
        assert_eq!(view.scope, before, "a ticket nobody waits on is dropped");
        land_scope(&mut view, 2, other.clone(), Ok(files(&["x.rs"])));
        assert_eq!(view.scope, other);
        assert_eq!(paths(&view), ["x.rs"]);
        assert_eq!(view.listing, None);
    }
}
