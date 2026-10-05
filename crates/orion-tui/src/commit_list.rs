//! The DIFF VIEWER's COMMIT LIST: what the branch added, newest first, each
//! commit a row with a box to tick, so a branch an agent landed as several
//! commits reads one commit at a time — or several at once.
//!
//! **What the branch is measured against** is where orion cuts its
//! worktrees from (`git::add_worktree_off_default` in the DAEMON): the
//! `worktree_base_branch` SETTING when it names a branch this repo has —
//! origin's copy first, then a local one — else `origin/HEAD`, else the
//! branch the ROOT WORKTREE is on, which is what a worktree is cut from in
//! a repo with no origin. The DAEMON fetches before it cuts; this reads
//! what the checkout already has and never touches the network.
//!
//! **The branch's own commits** are `git log --first-parent --no-merges
//! <merge-base>..HEAD`. Following first parents walks the line the branch
//! was committed on and never steps into the history of a branch merged
//! into it; `--no-merges` leaves out the merges themselves. So a branch
//! that merged `main` in to keep up lists what it added and nothing that
//! came in with `main`. HEAD is all it needs, so a detached checkout lists
//! the same way.
//!
//! **The rows**, top to bottom: **Uncommitted changes** while the checkout
//! is dirty, the commits, and `… N older commits` past the last page read.
//!
//! **What is on screen** ([`CommitList::showing`]): with nothing ticked,
//! the row under the cursor — `⇧←`/`⇧→` walk it from the files. Ticked
//! rows are read TOGETHER, as one diff, or ONE AT A TIME, stepped through
//! oldest first (`commit 2 of 3`). A clean checkout opens with every
//! commit ticked: the whole branch, what its pull request shows; a dirty
//! one opens on its uncommitted changes, nothing ticked. Nothing is
//! remembered from one opening to the next.
//!
//! **Together, with gaps** ([`CommitList::ranges`]). Ticked rows that sit
//! side by side on the branch read as one range, `git diff <parent of the
//! oldest> <newest>` — the working tree, when the uncommitted changes are
//! the newest. A row left unticked between two, or a merge, starts another
//! range: a diff across it would put back what was left out. A file that
//! two ranges touch reads as each range's diff in turn under its label
//! (`git_diff::scoped_diff`), never as a net diff that would have to
//! pretend the gap's commits never happened. The one exception is a run
//! that reaches down to the branch's first commit: it is measured from
//! the merge-base at its newest end, so it stays one range across the
//! merges of the base inside it, and with every commit ticked it is
//! exactly the branch's diff.
//!
//! Each choice is a [`DiffScope`] whose file list is read before it goes up
//! ([`show_selected`]), and every diff walked under it is taken against it
//! (`git_diff::scoped_diff`), so the files listed and the diffs read for
//! them always agree.
//!
//! **A long branch is read a page at a time.** The first [`COMMIT_PAGE`]
//! commits, with their counts, come back with the viewer's own listing
//! (`git_diff::read_opening`); the `older` row asks for the next page when
//! the cursor reaches it. Every read here is a BACKGROUND READ
//! (`view_jobs`), inline only in a view built without a handle.

use crate::app::{clamp_selection, DiffView};
use crate::diff_doc::Head;
use crate::git_diff::{run_git, DiffFile, DiffScope, LineChanges, Range, RangeStart};
use crate::view_jobs::{Answer, DiffListing};
use ratatui::layout::Rect;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

/// Commits read per page: the first comes with the viewer's listing, each
/// later one when the cursor reaches the `older` row. `git log` diffs
/// every commit it lists to count its lines, so a three-hundred-commit
/// branch is never diffed whole to show the top of it.
pub const COMMIT_PAGE: usize = 50;

/// The most commits a TOGETHER head names one by one before it says how
/// many more there are.
const HEAD_ITEMS: usize = 12;

/// `git log`'s format: a record mark, the fields between field marks, and
/// an end mark after the body, where `--numstat` takes over. Control
/// characters no name, subject or message carries, so nothing is quoted.
const LOG_FORMAT: &str = "--format=%x1e%H%x1f%h%x1f%P%x1f%aN%x1f%at%x1f%s%x1f%b%x1d";

/// The branch's own line: first parents only, merges left out.
const OWN_LINE: [&str; 2] = ["--first-parent", "--no-merges"];

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
    /// its own to show. The list never shows one, but a page can carry it.
    pub stat: Option<Stat>,
}

impl Commit {
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// What reading this commit alone shows: it against its first parent.
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

    /// Short sha, who and when: the dim line over a commit's subject.
    pub fn meta(&self, now_ms: i64) -> String {
        let mut meta = vec![self.short.clone(), self.author.clone(), self.ago(now_ms)];
        meta.retain(|part| !part.is_empty());
        meta.join(" · ")
    }

    /// The commit's message as the REVIEW HEAD over each of its files: its
    /// subject — after `lead`, `commit 2 of 3` stepping one at a time — who
    /// and when, and the body.
    pub fn head(&self, now_ms: i64, lead: Option<&str>) -> Vec<Head> {
        let title = match lead {
            Some(lead) => format!("{lead} · {}", self.subject),
            None => self.subject.clone(),
        };
        let mut head = vec![Head::Title(title), Head::Meta(self.meta(now_ms))];
        if !self.body.is_empty() {
            head.push(Head::Blank);
            head.extend(self.body.lines().map(|l| Head::Prose(l.to_string())));
        }
        head
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
    /// HEAD's commit: what the uncommitted changes sit on.
    pub head: Option<String>,
    /// The branch's own commits since the merge-base, however many of them
    /// were read.
    pub total: usize,
    /// The first page of them, newest first.
    pub commits: Vec<Commit>,
    /// The **Uncommitted changes** row's counts; None while the checkout is
    /// clean, and then that row is not there.
    pub uncommitted: Option<Stat>,
}

/// Read the COMMIT LIST for the checkout at `root`, whose uncommitted
/// changes `git status` found to be `uncommitted`: the base, the merge-base,
/// the first page of commits and the uncommitted row's counts.
pub fn read(root: &Path, base_setting: &str, uncommitted: &[DiffFile]) -> CommitListing {
    let mut listing = CommitListing {
        uncommitted: (!uncommitted.is_empty()).then(|| Stat {
            files: uncommitted.len(),
            lines: crate::git_diff::line_changes(root, uncommitted),
        }),
        ..CommitListing::default()
    };
    // An unborn HEAD has no commit to list and nothing to merge-base.
    listing.head = crate::git_diff::head_oid(root);
    if listing.head.is_none() {
        return listing;
    }
    listing.base = resolve_base(root, base_setting);
    let Some(merge_base) = listing
        .base
        .as_deref()
        .and_then(|b| merge_base(root, "HEAD", b))
    else {
        return listing;
    };
    let total = count_since(root, &merge_base, "HEAD");
    if total > 0 {
        listing.commits = read_page(root, &merge_base, "HEAD", 0).unwrap_or_else(|err| {
            tracing::warn!(root = %root.display(), "{err}");
            Vec::new()
        });
        // A log that listed nothing leaves nothing to page through.
        listing.total = if listing.commits.is_empty() { 0 } else { total };
    }
    listing.merge_base = Some(merge_base);
    listing
}

/// The COMMIT LIST of a pull request whose head commit `tip` this repo
/// has — `event_loop::open_pr_review` — measured against `base`, the
/// branch it merges into (origin's copy first, then a local one): its own
/// commits, newest first, and no uncommitted row, since nothing checked
/// out is being read. None when the repo has no such commit, no such
/// base — never another branch standing in for it — or nothing in common
/// with it: the pull request is read from GitHub instead.
pub fn read_tip(root: &Path, base: &str, tip: &str) -> Option<CommitListing> {
    if !has_commit(root, tip) {
        return None;
    }
    let base = branch_ref(root, base)?;
    let merge_base = merge_base(root, tip, &base)?;
    let total = count_since(root, &merge_base, tip);
    let commits = match total {
        0 => Vec::new(),
        _ => read_page(root, &merge_base, tip, 0).ok()?,
    };
    Some(CommitListing {
        total: if commits.is_empty() { 0 } else { total },
        base: Some(base),
        merge_base: Some(merge_base),
        head: Some(tip.to_string()),
        commits,
        uncommitted: None,
    })
}

/// The ref the branch is measured against — see the module doc for the
/// order. The setting is read the way the DAEMON reads it: trimmed, a
/// leading `origin/` meaning the same as the bare name.
pub fn resolve_base(root: &Path, base_setting: &str) -> Option<String> {
    if let Some(base) = branch_ref(root, base_setting) {
        return Some(base);
    }
    git_line(
        root,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
    )
    .or_else(|| root_branch(root))
}

/// How many commits HEAD has that the base it is measured against
/// ([`resolve_base`]) does not, and how many the base has that HEAD does
/// not: a band's `⇡4 ⇣1`. The root's base is origin's copy of its own
/// branch, so there the two are what is unpushed and what is unpulled as
/// of the last fetch. One `git rev-list` over both sides; None when git
/// cannot say — no base to measure against, or no commit yet.
pub fn ahead_behind(root: &Path, base_setting: &str) -> Option<(usize, usize)> {
    let base = resolve_base_cached(root, base_setting)?;
    let range = format!("HEAD...{base}");
    let line = git_line(root, &["rev-list", "--left-right", "--count", &range])?;
    let mut counts = line.split_whitespace().map(str::parse::<usize>);
    match (counts.next(), counts.next()) {
        (Some(Ok(ahead)), Some(Ok(behind))) => Some((ahead, behind)),
        _ => None,
    }
}

/// How long [`ahead_behind`] trusts a base it resolved: the setting and
/// `origin/HEAD` it comes from rarely change, and resolving takes up to four
/// git processes on a poll that runs every couple of seconds.
const BASE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// [`resolve_base`] for `root` and `base_setting`, kept for [`BASE_TTL`].
pub(crate) fn resolve_base_cached(root: &Path, base_setting: &str) -> Option<String> {
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    type Resolved = HashMap<(std::path::PathBuf, String), (Instant, Option<String>)>;
    static BASES: OnceLock<Mutex<Resolved>> = OnceLock::new();
    let bases = BASES.get_or_init(Default::default);
    let key = (root.to_path_buf(), base_setting.to_string());
    if let Ok(known) = bases.lock() {
        if let Some((_, base)) = known.get(&key).filter(|(at, _)| at.elapsed() < BASE_TTL) {
            return base.clone();
        }
    }
    let base = resolve_base(root, base_setting);
    if let Ok(mut known) = bases.lock() {
        known.insert(key, (Instant::now(), base.clone()));
    }
    base
}

/// The ref a branch named `name` is read from: origin's copy when origin
/// has one — `origin/<name>` — else the local branch. A leading `origin/`
/// means the same as the bare name. None for no name, for `HEAD` (always
/// this checkout's, never a branch anyone meant) and for a branch the
/// repo has neither of.
pub(crate) fn branch_ref(root: &Path, name: &str) -> Option<String> {
    let name = name.trim();
    let name = name.strip_prefix("origin/").unwrap_or(name).trim();
    if name.is_empty() || name == "HEAD" {
        return None;
    }
    if has_commit(root, &format!("refs/remotes/origin/{name}")) {
        return Some(format!("origin/{name}"));
    }
    has_commit(root, &format!("refs/heads/{name}")).then(|| name.to_string())
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

/// Whether `rev` — a full ref, a sha — names a commit this repo has.
fn has_commit(root: &Path, rev: &str) -> bool {
    let spec = format!("{rev}^{{commit}}");
    git_line(root, &["rev-parse", "--verify", "--quiet", &spec]).is_some()
}

fn merge_base(root: &Path, tip: &str, base: &str) -> Option<String> {
    git_line(root, &["merge-base", tip, base])
}

/// How many of its own commits `tip` — HEAD, or a pull request's head —
/// has past `merge_base`: no diffs, so it costs the same for three commits
/// as for three hundred.
fn count_since(root: &Path, merge_base: &str, tip: &str) -> usize {
    let range = format!("{merge_base}..{tip}");
    let mut args = vec!["rev-list", "--count"];
    args.extend(OWN_LINE);
    args.push(&range);
    git_line(root, &args)
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

/// One page of the branch's own commits up to `tip`, newest first: the
/// `skip` newest left out, [`COMMIT_PAGE`] at most, each with its counts.
/// `Err` is a user-facing message.
pub fn read_page(
    root: &Path,
    merge_base: &str,
    tip: &str,
    skip: usize,
) -> Result<Vec<Commit>, String> {
    let range = format!("{merge_base}..{tip}");
    let max = format!("--max-count={COMMIT_PAGE}");
    let skip = format!("--skip={skip}");
    let mut args = vec![
        "log",
        LOG_FORMAT,
        "--numstat",
        "--no-color",
        "--no-show-signature",
    ];
    args.extend(OWN_LINE);
    args.extend([max.as_str(), skip.as_str(), range.as_str(), "--"]);
    let output = run_git(root, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git log failed: {}", stderr.trim()));
    }
    Ok(parse_log(&String::from_utf8_lossy(&output.stdout)))
}

/// One commit of the repo at `root`, by any name git takes for one — the
/// way into the viewer for a pull request's commit (`event_loop::
/// open_pr_review`). None when the repo has no such commit.
pub fn read_commit(root: &Path, rev: &str) -> Option<Commit> {
    let spec = format!("{rev}^{{commit}}");
    let output = run_git(
        root,
        &[
            "log",
            "-1",
            LOG_FORMAT,
            "--numstat",
            "--no-color",
            "--no-show-signature",
            &spec,
            "--",
        ],
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_log(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .next()
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

/// One row of the COMMIT LIST. Ordered top to bottom: the uncommitted
/// changes, the commits newest first, the `older` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Row {
    /// **Uncommitted changes**: the working tree against HEAD.
    Uncommitted,
    /// The commit at this index of `CommitList::commits`.
    Commit(usize),
    /// `… N older commits`: the next page, read when the cursor gets here.
    Older,
}

/// What the COMMIT LIST puts on screen ([`CommitList::showing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Showing {
    /// One row: the cursor's with nothing ticked, the only ticked one, or
    /// the step ONE AT A TIME is on.
    Row(Row),
    /// Two or more ticked rows read TOGETHER, top to bottom.
    Together(Vec<Row>),
}

/// The COMMIT LIST in an open DIFF VIEWER: what [`CommitListing`] read,
/// the cursor, the ticks, and the page in flight.
#[derive(Debug, Clone, Default)]
pub struct CommitList {
    /// False until the listing lands; the list says `reading commits…`.
    pub loaded: bool,
    pub base: Option<String>,
    pub merge_base: Option<String>,
    pub head: Option<String>,
    pub total: usize,
    /// Newest first. Shared with every frame's clone of the view rather
    /// than copied into it.
    pub commits: Arc<Vec<Commit>>,
    pub uncommitted: Option<Stat>,
    /// Index into the rows ([`CommitList::row`]).
    pub selected: usize,
    /// The ticked rows: never the `older` row.
    pub ticked: BTreeSet<Row>,
    /// Ticked rows read ONE AT A TIME rather than TOGETHER.
    pub one_at_a_time: bool,
    /// The ticked row ONE AT A TIME is on.
    pub step: Option<Row>,
    /// The next page in flight, by ticket.
    pub paging: Option<u64>,
    /// The first row on screen, as last drawn, and the cursor it was
    /// scrolled away from by the wheel: until the cursor moves, the list
    /// stays where the wheel put it.
    pub top: usize,
    pub pinned_at: Option<usize>,
    /// The panel, and each row's rect as last drawn, for the pointer.
    pub area: Rect,
    pub hits: Vec<(Rect, usize)>,
}

impl CommitList {
    /// The list a view opens with, before its listing lands.
    pub fn reading() -> Self {
        Self::default()
    }

    /// The listing landed. A dirty checkout opens on its uncommitted
    /// changes, nothing ticked; a clean one with every commit ticked, read
    /// together — the whole branch — the cursor on the newest.
    pub fn from_listing(listing: CommitListing) -> Self {
        let mut list = Self {
            loaded: true,
            base: listing.base,
            merge_base: listing.merge_base,
            head: listing.head,
            total: listing.total,
            commits: Arc::new(listing.commits),
            uncommitted: listing.uncommitted,
            ..Self::default()
        };
        if list.uncommitted.is_none() {
            list.ticked = (0..list.commits.len()).map(Row::Commit).collect();
        }
        list
    }

    /// Whether commits past the last page read are still to come.
    pub fn has_older(&self) -> bool {
        !self.commits.is_empty() && self.commits.len() < self.total
    }

    pub fn row_count(&self) -> usize {
        usize::from(self.uncommitted.is_some()) + self.commits.len() + usize::from(self.has_older())
    }

    /// The row at `index`, top to bottom.
    pub fn row(&self, index: usize) -> Option<Row> {
        let mut i = index;
        if self.uncommitted.is_some() {
            if i == 0 {
                return Some(Row::Uncommitted);
            }
            i -= 1;
        }
        if i < self.commits.len() {
            return Some(Row::Commit(i));
        }
        (i == self.commits.len() && self.has_older()).then_some(Row::Older)
    }

    pub fn index_of(&self, row: Row) -> Option<usize> {
        let first = usize::from(self.uncommitted.is_some());
        match row {
            Row::Uncommitted => self.uncommitted.is_some().then_some(0),
            Row::Commit(i) => (i < self.commits.len()).then_some(first + i),
            Row::Older => self.has_older().then_some(first + self.commits.len()),
        }
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.row(self.selected)
    }

    /// Clamped absolute cursor; true when it moved. ONE AT A TIME, landing
    /// on a ticked row steps onto it.
    pub fn select(&mut self, index: i64) -> bool {
        let clamped = clamp_selection(index, self.row_count());
        let changed = clamped != self.selected;
        self.selected = clamped;
        if self.one_at_a_time {
            if let Some(row) = self.selected_row().filter(|r| self.ticked.contains(r)) {
                self.step = Some(row);
            }
        }
        changed
    }

    /// The ticked rows, oldest first: the order ONE AT A TIME steps in.
    pub fn steps(&self) -> Vec<Row> {
        self.ticked.iter().rev().copied().collect()
    }

    /// Whether every commit read so far is ticked — the list's "all".
    pub fn all_ticked(&self) -> bool {
        !self.commits.is_empty()
            && (0..self.commits.len()).all(|i| self.ticked.contains(&Row::Commit(i)))
    }

    /// Tick the row under the cursor, or untick it. ONE AT A TIME, a step
    /// unticked from under the reader hands the steps to its older
    /// neighbour (the newer one when it was the oldest).
    pub fn toggle_tick(&mut self) {
        let Some(row) = self.selected_row().filter(|r| *r != Row::Older) else {
            return;
        };
        if !self.ticked.remove(&row) {
            self.ticked.insert(row);
            return;
        }
        if self.step == Some(row) {
            let steps = self.steps();
            self.step = steps
                .iter()
                .rev()
                .find(|r| **r > row)
                .or_else(|| steps.iter().find(|r| **r < row))
                .copied();
        }
    }

    /// `^A`: every commit ticked — or, when they all are already, nothing
    /// at all. The uncommitted row keeps its tick on the way up: whether
    /// the working tree belongs with the commits is the reader's call.
    pub fn tick_all(&mut self) {
        if self.all_ticked() {
            self.ticked.clear();
            self.step = None;
        } else {
            self.ticked.extend((0..self.commits.len()).map(Row::Commit));
        }
    }

    /// `row` alone on screen: nothing ticked, the cursor on it — a pull
    /// request opened on one of its commits.
    pub fn show_only(&mut self, row: Row) {
        let Some(index) = self.index_of(row) else {
            return;
        };
        self.ticked.clear();
        self.step = None;
        self.one_at_a_time = false;
        self.select(index as i64);
    }

    /// Read the ticked ONE AT A TIME from the oldest — the **Ticked
    /// commits** SETTING's `one at a time`, as a viewer opens. Nothing
    /// changes with fewer than two ticked.
    pub fn start_one_at_a_time(&mut self) {
        if self.ticked.len() < 2 {
            return;
        }
        self.one_at_a_time = true;
        self.step = self.steps().first().copied();
        self.follow_step();
    }

    /// `^G`: TOGETHER and ONE AT A TIME, the other way round. Stepping
    /// starts on the cursor's row when it is ticked, else on the oldest.
    pub fn toggle_mode(&mut self) {
        self.one_at_a_time = !self.one_at_a_time;
        if self.one_at_a_time {
            let here = self.selected_row().filter(|r| self.ticked.contains(r));
            self.step = here.or_else(|| self.steps().first().copied());
            self.follow_step();
        }
    }

    /// `⇧←` / `⇧→`: older or newer. With nothing ticked the cursor walks
    /// the list. ONE AT A TIME, the step walks the ticked rows. TOGETHER,
    /// it starts stepping: `⇧→` on the oldest — commit 1 — `⇧←` on the
    /// newest. True when what is on screen changed.
    pub fn step_by(&mut self, older: bool) -> bool {
        let steps = self.steps();
        if steps.is_empty() {
            let delta = if older { 1 } else { -1 };
            return self.select(self.selected as i64 + delta);
        }
        if steps.len() == 1 {
            return false;
        }
        if !self.one_at_a_time {
            self.one_at_a_time = true;
            self.step = if older { steps.last() } else { steps.first() }.copied();
            self.follow_step();
            return true;
        }
        let at = self
            .step
            .and_then(|s| steps.iter().position(|r| *r == s))
            .unwrap_or(0);
        let next = if older {
            at.checked_sub(1)
        } else {
            (at + 1 < steps.len()).then_some(at + 1)
        };
        match next {
            Some(next) => {
                self.step = Some(steps[next]);
                self.follow_step();
                true
            }
            None => false,
        }
    }

    /// The cursor onto the step, so the list's highlight is the commit on
    /// screen.
    fn follow_step(&mut self) {
        if let Some(index) = self.step.and_then(|s| self.index_of(s)) {
            self.selected = index;
        }
    }

    /// ONE AT A TIME's place: `(k, n)`, commit `k` of the `n` ticked.
    pub fn step_place(&self) -> Option<(usize, usize)> {
        if !self.one_at_a_time {
            return None;
        }
        let steps = self.steps();
        let at = steps.iter().position(|r| Some(*r) == self.step)?;
        (steps.len() > 1).then_some((at + 1, steps.len()))
    }

    /// What goes on screen: with nothing ticked, the cursor's row (none on
    /// the `older` row, which is a page to read); one ticked row, that
    /// row; more, all of them TOGETHER or the step ONE AT A TIME is on.
    pub fn showing(&self) -> Option<Showing> {
        let steps = self.steps();
        match steps.len() {
            0 => self
                .selected_row()
                .filter(|r| *r != Row::Older)
                .map(Showing::Row),
            1 => Some(Showing::Row(steps[0])),
            _ if self.one_at_a_time => {
                let step = self.step.filter(|s| steps.contains(s)).unwrap_or(steps[0]);
                Some(Showing::Row(step))
            }
            _ => Some(Showing::Together(self.ticked.iter().copied().collect())),
        }
    }

    /// Whether `row`'s changes are in the diff on screen.
    pub fn on_screen(&self, row: Row) -> bool {
        match self.showing() {
            Some(Showing::Row(shown)) => shown == row,
            Some(Showing::Together(rows)) => rows.contains(&row),
            None => false,
        }
    }

    /// What reading `row` alone shows; None for the `older` row.
    fn scope_of_row(&self, row: Row) -> Option<DiffScope> {
        match row {
            Row::Uncommitted => Some(DiffScope::Uncommitted),
            Row::Commit(i) => self.commits.get(i).map(Commit::scope),
            Row::Older => None,
        }
    }

    /// The scope behind what [`CommitList::showing`] says is on screen.
    pub fn scope_of(&self, showing: &Showing) -> Option<DiffScope> {
        match showing {
            Showing::Row(row) => self.scope_of_row(*row),
            Showing::Together(rows) => Some(DiffScope::Ranges(self.ranges(rows))),
        }
    }

    /// Whether the row at list index `newer` sits right on the one at
    /// `older` — the next row down — with nothing between them on the
    /// branch: no merge left out of the list.
    fn sits_on(&self, newer: usize, older: usize) -> bool {
        if older != newer + 1 {
            return false;
        }
        let parent = match self.row(newer) {
            Some(Row::Uncommitted) => self.head.as_ref(),
            Some(Row::Commit(i)) => self.commits[i].parents.first(),
            _ => None,
        };
        match self.row(older) {
            Some(Row::Commit(i)) => parent == Some(&self.commits[i].sha),
            _ => false,
        }
    }

    /// The ranges a TOGETHER diff of `rows` reads, oldest first — see the
    /// module doc for where one range ends and the next begins.
    pub fn ranges(&self, rows: &[Row]) -> Vec<Range> {
        let mut index: Vec<usize> = rows.iter().filter_map(|r| self.index_of(*r)).collect();
        index.sort_unstable();
        index.reverse();
        // The branch's first commit is the list's last — once every page
        // is in, or with every commit read so far ticked, standing in for
        // the ones not read yet.
        let first_commit = self
            .commits
            .len()
            .checked_sub(1)
            .and_then(|i| self.index_of(Row::Commit(i)));
        let from_the_start = first_commit.is_some()
            && index.first() == first_commit.as_ref()
            && (!self.has_older() || self.all_ticked());
        let mut runs: Vec<(Vec<usize>, bool)> = Vec::new();
        for (n, &i) in index.iter().enumerate() {
            if let Some((run, tail)) = runs.last_mut() {
                let below = *run.last().expect("a run holds a row");
                if below == i + 1 && (*tail || self.sits_on(i, below)) {
                    run.push(i);
                    continue;
                }
            }
            runs.push((vec![i], n == 0 && from_the_start));
        }
        runs.into_iter()
            .map(|(run, tail)| self.range_of(&run, tail))
            .collect()
    }

    /// One run of rows (list indices, oldest first) as a [`Range`].
    fn range_of(&self, run: &[usize], from_the_start: bool) -> Range {
        let (oldest, newest) = (run[0], run[run.len() - 1]);
        let commit = |index: usize| match self.row(index) {
            Some(Row::Commit(i)) => self.commits.get(i),
            _ => None,
        };
        let to = commit(newest).map(|c| c.sha.clone());
        let at_head = to.is_none() || to == self.head;
        let from = match (from_the_start, &self.base, &self.merge_base) {
            (true, _, Some(merge_base)) if at_head => RangeStart::Rev(merge_base.clone()),
            (true, Some(base), _) => RangeStart::MergeBase { base: base.clone() },
            _ => match commit(oldest) {
                Some(c) => c
                    .parents
                    .first()
                    .map_or(RangeStart::Empty, |p| RangeStart::Rev(p.clone())),
                // Only the uncommitted changes: they sit on HEAD.
                None => RangeStart::Rev(self.head.clone().unwrap_or_else(|| "HEAD".into())),
            },
        };
        let name =
            |index: usize| commit(index).map_or("working tree".to_string(), |c| c.short.clone());
        let commits = run.iter().filter(|i| commit(**i).is_some()).count();
        let label = match (run.len(), commits) {
            (1, 0) => "the uncommitted changes".to_string(),
            (1, _) => name(oldest),
            (_, n) => {
                let noun = if n == 1 { "commit" } else { "commits" };
                format!("{}..{} · {n} {noun}", name(oldest), name(newest))
            }
        };
        Range { from, to, label }
    }

    /// The REVIEW HEAD over each file of what `showing` puts on screen: a
    /// commit's message — `commit 2 of 3 · …` stepping one at a time — or,
    /// read together, what was ticked.
    pub fn head(&self, showing: &Showing, now_ms: i64) -> Vec<Head> {
        let lead = self.step_place().map(|(k, n)| format!("commit {k} of {n}"));
        match showing {
            Showing::Row(Row::Commit(i)) => self
                .commits
                .get(*i)
                .map(|c| c.head(now_ms, lead.as_deref()))
                .unwrap_or_default(),
            Showing::Row(Row::Uncommitted) => match lead {
                Some(lead) => vec![
                    Head::Title(format!("{lead} · the uncommitted changes")),
                    Head::Meta("the working tree against HEAD, untracked files included".into()),
                ],
                None => Vec::new(),
            },
            Showing::Row(Row::Older) => Vec::new(),
            Showing::Together(rows) => self.together_head(rows, now_ms),
        }
    }

    fn together_head(&self, rows: &[Row], now_ms: i64) -> Vec<Head> {
        let ranges = self.ranges(rows);
        let commits = rows.iter().filter(|r| matches!(r, Row::Commit(_))).count();
        let dirty = rows.contains(&Row::Uncommitted);
        let whole = self.all_ticked();
        let base = self.base.as_deref().unwrap_or("its base");
        let mut title = if whole {
            format!("The whole branch since {base}")
        } else {
            format!("{commits} commits together")
        };
        if dirty {
            title.push_str(" and the uncommitted changes");
        }
        let mut head = vec![Head::Title(title)];
        if whole {
            let noun = if self.total == 1 { "commit" } else { "commits" };
            head.push(Head::Meta(format!(
                "{} {noun}, as its pull request would show them",
                self.total.max(commits)
            )));
        }
        if ranges.len() > 1 {
            head.push(Head::Meta(format!(
                "in {} ranges — something unticked sits between them — so a file two of them touch shows each range's diff in turn",
                ranges.len()
            )));
        }
        head.push(Head::Blank);
        let pad = self.commits.first().map_or(7, |c| {
            unicode_width::UnicodeWidthStr::width(c.short.as_str())
        });
        for row in rows.iter().take(HEAD_ITEMS) {
            head.push(match row {
                Row::Commit(i) => {
                    let c = &self.commits[*i];
                    Head::Item {
                        sha: c.short.clone(),
                        text: format!("{} · {}", c.subject, c.ago(now_ms)),
                    }
                }
                _ => Head::Item {
                    sha: " ".repeat(pad),
                    text: "the uncommitted changes".into(),
                },
            });
        }
        if rows.len() > HEAD_ITEMS {
            head.push(Head::Meta(format!(
                "… and {} more",
                rows.len() - HEAD_ITEMS
            )));
        }
        head
    }

    /// The first row of the list on screen for a panel whose rows take
    /// `heights` (one entry per row) and `avail` lines: where the wheel
    /// left it while the cursor hasn't moved since, else the last top moved
    /// just enough to show the cursor's row whole.
    pub fn window_top(&self, heights: &[usize], avail: usize) -> usize {
        let len = heights.len();
        // The furthest top that still fills the panel.
        let mut max_top = len.saturating_sub(1);
        let mut used = 0;
        for (i, h) in heights.iter().enumerate().rev() {
            used += h;
            if used > avail {
                break;
            }
            max_top = i;
        }
        let mut top = self.top.min(max_top);
        if self.pinned_at == Some(self.selected) {
            return top;
        }
        let cursor = self.selected.min(len.saturating_sub(1));
        if cursor < top {
            top = cursor;
        }
        while top < cursor && heights[top..=cursor].iter().sum::<usize>() > avail {
            top += 1;
        }
        top
    }

    /// The wheel over the list: its rows scroll, the cursor stays.
    pub fn wheel(&mut self, delta: i64) {
        let last = self.row_count().saturating_sub(1) as i64;
        self.top = (self.top as i64 + delta).clamp(0, last) as usize;
        self.pinned_at = Some(self.selected);
    }
}

/// Put the COMMIT LIST read with the viewer's opening into `view` and put
/// up what it opens on: a clean checkout's whole branch, ticked.
pub fn install(view: &mut DiffView, listing: CommitListing) {
    let mut list = CommitList::from_listing(listing);
    if view.open_one_at_a_time {
        list.start_one_at_a_time();
    }
    view.commits = Some(Box::new(list));
    show_selected(view);
}

/// The COMMIT LIST's cursor, ticks or mode changed: put up what it now
/// says is on screen. The `older` row under the cursor asks for the next
/// page; anything else for its file list — unless it is the scope on
/// screen already, with no other asked for since, when only the head over
/// it changes (`commit 2 of 3` became `2 of 4`).
pub fn show_selected(view: &mut DiffView) {
    let Some(list) = &view.commits else {
        return;
    };
    if list.selected_row() == Some(Row::Older) {
        request_page(view);
    }
    let Some(list) = &view.commits else {
        return;
    };
    let Some(showing) = list.showing() else {
        return;
    };
    let Some(scope) = list.scope_of(&showing) else {
        return;
    };
    if scope == view.scope && view.listing.is_none() {
        let head = list.head(&showing, crate::app::now_ms());
        if head != view.head {
            view.set_head(head);
        }
        return;
    }
    request_scope(view, scope);
}

/// Put `scope` up in a view with no COMMIT LIST of its own — one commit of
/// a pull request — once its file list is read.
pub fn show_scope(view: &mut DiffView, scope: DiffScope) {
    request_scope(view, scope);
}

/// Ask for a scope's file list. The files and the diff on screen stay up
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

/// A scope's file list: the uncommitted changes the way `g` reads them —
/// `read_listing`, the stored ✓ marks restored — and any other through
/// `scope_files`.
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

/// A scope's file list came back. When it is the one the view waits on,
/// it becomes the scope on screen: the files replaced — the filter kept,
/// so a typed path follows the reader from commit to commit — the cursor
/// home on the first unreviewed file, the REVIEW HEAD for what is on
/// screen put over the diff, and that file's diff read.
///
/// The uncommitted changes' ✓ marks are stored on disk and come back with
/// their listing; any other scope's are kept for as long as the modal is
/// up, put away while another is on screen and brought back with it.
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
    // A view with no COMMIT LIST keeps the head it was opened with.
    if let Some(list) = &view.commits {
        let now = crate::app::now_ms();
        view.head = list
            .showing()
            .map(|showing| list.head(&showing, now))
            .unwrap_or_default();
    }
    view.header_read = false;
    // A diff read under the last scope must never land under this one's
    // name: a fresh id drops it, and the cache was of that scope too.
    view.id = crate::view_jobs::ticket();
    view.cache.clear();
    view.waiting = None;
    view.replace_files(files);
    // A file asked for before this scope's files were read — a pull
    // request's, from its Changes tab.
    if let Some(path) = view.want_path.take() {
        view.select_path(&path);
    }
    view.show_diff(None, String::new(), false);
    match error {
        Some(msg) => view.show_diff(None, msg, false),
        None => crate::git_diff::load_selected_diff(view),
    }
}

/// A scope's file list has outlasted `view_jobs::STALE_GRACE`: the last
/// scope's files come down and the list says `reading changes…`, rather
/// than leave them under the new one's name.
pub fn listing_slow(view: &mut DiffView, ticket: u64) {
    if view.listing != Some(ticket) || view.files.is_empty() {
        return;
    }
    view.waiting = None;
    if view.commits.is_some() {
        view.head.clear();
    }
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
    // The commit the first page was read down from: HEAD as it was, or a
    // pull request's head.
    let tip = list.head.clone().unwrap_or_else(|| "HEAD".into());
    let skip = list.commits.len();
    let ticket = crate::view_jobs::ticket();
    list.paging = Some(ticket);
    match jobs {
        Some(jobs) => jobs.run(move || {
            Some(Answer::CommitPage {
                ticket,
                result: read_page(&root, &merge_base, &tip, skip),
            })
        }),
        None => {
            let result = read_page(&root, &merge_base, &tip, skip);
            land_page(view, ticket, result);
        }
    }
}

/// The next page of commits came back. It joins the list — ticked, when
/// every commit before it was: "all" still means all — and a cursor that
/// was waiting on the `older` row, now the page's first commit, has that
/// put up. A page that read nothing (or failed) ends the list where it is.
pub fn land_page(view: &mut DiffView, ticket: u64, result: Result<Vec<Commit>, String>) {
    let Some(list) = &mut view.commits else {
        return;
    };
    if list.paging != Some(ticket) {
        return;
    }
    list.paging = None;
    let all = list.all_ticked();
    match result {
        Ok(page) if !page.is_empty() => {
            let from = list.commits.len();
            Arc::make_mut(&mut list.commits).extend(page);
            if all {
                list.ticked
                    .extend((from..list.commits.len()).map(Row::Commit));
            }
        }
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

    /// A branch's commits ahead of and behind its base: `feat` two commits
    /// on, and `main` — origin's copy, as a fetch leaves it — one on since
    /// `feat` was cut. Level with it, both are 0.
    #[test]
    fn ahead_behind_counts_both_sides_of_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        assert_eq!(ahead_behind(&repo, ""), Some((0, 0)));
        commit(&repo, "a.txt", "a\n", "feat: a");
        commit(&repo, "b.txt", "b\n", "feat: b");
        git(&repo, &["checkout", "-q", "main"]);
        commit(&repo, "m.txt", "m\n", "main: m");
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&repo, &["checkout", "-q", "feat"]);
        assert_eq!(ahead_behind(&repo, ""), Some((2, 1)));
        assert_eq!(ahead_behind(&repo, "origin/main"), Some((2, 1)));
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

    /// `main` moves on and is merged into `feat`, which then carries on —
    /// and `origin/main` follows, as a fetch would have it.
    fn merge_main(repo: &Path, file: &str) {
        git(repo, &["checkout", "-q", "main"]);
        commit(repo, file, "main's\n", &format!("main: {file}"));
        git(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(repo, &["checkout", "-q", "feat"]);
        git(repo, &["merge", "-q", "--no-edit", "main"]);
    }

    /// A pull request's commits are listed down from its head commit, not
    /// from wherever the checkout is: another branch checked out lists the
    /// same, there is no uncommitted row, and paging reads on from the
    /// same tip. A head or a base the repo does not have lists nothing.
    #[test]
    fn a_pull_requests_commits_are_read_from_its_tip() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        commit(&repo, "a.txt", "a\n", "first");
        commit(&repo, "b.txt", "b\n", "second");
        let tip = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("dirty.txt"), "x\n").unwrap();
        let listing = read_tip(&repo, "main", &tip).expect("the tip is here");
        assert_eq!(listing.base.as_deref(), Some("origin/main"));
        assert_eq!(listing.head.as_deref(), Some(tip.as_str()));
        assert_eq!(listing.total, 2);
        let subjects: Vec<&str> = listing.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["second", "first"]);
        assert!(listing.uncommitted.is_none(), "nothing checked out is read");
        let list = CommitList::from_listing(listing);
        assert!(list.all_ticked(), "the whole pull request");
        assert!(read_tip(&repo, "release", &tip).is_none(), "no such base");
        assert!(read_tip(&repo, "main", &"0".repeat(40)).is_none());
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
    fn the_head_is_the_subject_then_who_and_when_then_the_body() {
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
            commit.head(1_000_000 + 3 * 3_600_000, None),
            vec![
                Head::Title("Add retry".into()),
                Head::Meta("abc · Tess · 3h ago".into()),
                Head::Blank,
                Head::Prose("- one".into()),
                Head::Prose("- two".into()),
            ]
        );
        let bare = Commit {
            body: String::new(),
            ..commit
        };
        assert_eq!(
            bare.head(1_000_000, Some("commit 2 of 3")),
            vec![
                Head::Title("commit 2 of 3 · Add retry".into()),
                Head::Meta("abc · Tess · just now".into()),
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
    /// uncommitted row's counts.
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
        assert_eq!(listing.head, crate::git_diff::head_oid(&repo));
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

    /// The owner's rule: only what the branch added. A merge of `main`
    /// into the branch is not listed, and neither is anything that came in
    /// with it — before the merge or after `main` moved on again.
    #[test]
    fn merges_and_what_they_brought_in_are_never_listed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        commit(&repo, "a.txt", "a\n", "mine: a");
        merge_main(&repo, "m1.txt");
        commit(&repo, "b.txt", "b\n", "mine: b");
        merge_main(&repo, "m2.txt");
        commit(&repo, "c.txt", "c\n", "mine: c");
        // `main` moves on once more, not merged: still not the branch's.
        git(&repo, &["checkout", "-q", "main"]);
        commit(&repo, "m3.txt", "m3\n", "main: m3");
        git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&repo, &["checkout", "-q", "feat"]);

        let listing = read(&repo, "", &[]);
        let subjects: Vec<&str> = listing.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["mine: c", "mine: b", "mine: a"]);
        assert_eq!(listing.total, 3, "the count agrees with the log");
        assert!(listing.commits.iter().all(|c| !c.is_merge()));

        // Read together, all three are the branch's own diff: none of
        // main's files.
        let mut view = DiffView::new(repo.clone(), "feat".into(), Vec::new(), true);
        install(&mut view, listing);
        let paths: Vec<&str> = view.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b.txt", "c.txt"]);
        // `mine: b` alone sits on a merge: its parent, not main's commits.
        select(&mut view, Row::Commit(1));
        view.commits.as_mut().unwrap().tick_all();
        show_selected(&mut view);
        let paths: Vec<&str> = view.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["b.txt"]);
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
    /// listing, the next when the `older` row is reached — ticked like the
    /// rest when every commit before it was.
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
        assert!(list.all_ticked(), "a clean checkout opens on all of it");

        // The cursor reaches the `older` row: the page is read (inline,
        // with no jobs) and joins the list ticked.
        let last = view.commits.as_ref().unwrap().row_count() as i64 - 1;
        view.commits.as_mut().unwrap().select(last);
        show_selected(&mut view);
        let list = view.commits.as_ref().unwrap();
        assert_eq!(list.commits.len(), COMMIT_PAGE + 3);
        assert!(!list.has_older());
        assert!(list.all_ticked(), "all still means all");
        let Some(Row::Commit(i)) = list.selected_row() else {
            panic!("the cursor is on a commit");
        };
        assert_eq!(list.commits[i].subject, "c2");

        // Nothing ticked, the cursor's commit is what is on screen.
        view.commits.as_mut().unwrap().tick_all();
        show_selected(&mut view);
        let list = view.commits.as_ref().unwrap();
        assert_eq!(view.scope, list.commits[i].scope());
    }

    /// `n` commits chained parent to child, newest first, HEAD on the
    /// newest; `merge_after` puts a merge (left out of the list, as `git
    /// log --no-merges` leaves it) between commit `k` and the one under it.
    fn list_with(
        commits: usize,
        dirty: bool,
        total: usize,
        merge_after: Option<usize>,
    ) -> CommitList {
        let commit = |n: usize| Commit {
            sha: format!("sha{n}"),
            short: format!("s{n}"),
            parents: vec![if merge_after == Some(n) {
                format!("merge{n}")
            } else {
                format!("sha{}", n + 1)
            }],
            author: "Tess".into(),
            time: 0,
            subject: format!("commit {n}"),
            body: String::new(),
            stat: None,
        };
        let mut list = CommitList::from_listing(CommitListing {
            base: Some("origin/main".into()),
            merge_base: Some("mb".into()),
            head: Some("sha0".into()),
            total,
            commits: (0..commits).map(commit).collect(),
            uncommitted: dirty.then(Stat::default),
        });
        list.ticked.clear();
        list
    }

    fn list_of(commits: usize, dirty: bool, total: usize) -> CommitList {
        list_with(commits, dirty, total, None)
    }

    /// The rows run Uncommitted, the commits newest first, then the
    /// `older` row; each is missing when there is nothing behind it.
    #[test]
    fn the_rows_are_only_the_ones_with_something_behind_them() {
        let rows = |list: &CommitList| -> Vec<Row> {
            (0..list.row_count()).filter_map(|i| list.row(i)).collect()
        };
        let full = list_of(2, true, 5);
        assert_eq!(
            rows(&full),
            [Row::Uncommitted, Row::Commit(0), Row::Commit(1), Row::Older]
        );
        assert_eq!(full.row(4), None);
        for (i, row) in rows(&full).into_iter().enumerate() {
            assert_eq!(full.index_of(row), Some(i));
        }
        let clean = list_of(2, false, 2);
        assert_eq!(rows(&clean), [Row::Commit(0), Row::Commit(1)]);
        assert_eq!(clean.index_of(Row::Older), None);
        assert_eq!(rows(&list_of(0, true, 0)), [Row::Uncommitted]);
        assert_eq!(CommitList::reading().row_count(), 0);
    }

    /// What a fresh list opens on: a dirty checkout's uncommitted changes,
    /// nothing ticked; a clean one's every commit, read together.
    #[test]
    fn a_dirty_checkout_opens_on_its_changes_and_a_clean_one_on_the_branch() {
        let dirty = CommitList::from_listing(CommitListing {
            uncommitted: Some(Stat::default()),
            commits: list_of(2, false, 2).commits.to_vec(),
            ..CommitListing::default()
        });
        assert!(dirty.ticked.is_empty());
        assert_eq!(dirty.showing(), Some(Showing::Row(Row::Uncommitted)));

        let clean = CommitList::from_listing(CommitListing {
            total: 2,
            commits: list_of(2, false, 2).commits.to_vec(),
            ..CommitListing::default()
        });
        assert!(clean.all_ticked());
        assert_eq!(
            clean.showing(),
            Some(Showing::Together(vec![Row::Commit(0), Row::Commit(1)]))
        );
        assert_eq!(clean.selected_row(), Some(Row::Commit(0)));
    }

    /// Ticking and the two ways to read what is ticked: nothing ticked is
    /// the cursor's row; ticked rows read together, or one at a time,
    /// oldest first, the cursor riding along; `^A` all and none.
    #[test]
    fn ticks_read_together_or_one_at_a_time() {
        let mut list = list_of(4, true, 4);
        list.select(2);
        assert_eq!(list.showing(), Some(Showing::Row(Row::Commit(1))));
        assert!(list.step_by(true), "⇧← walks the cursor older");
        assert_eq!(list.showing(), Some(Showing::Row(Row::Commit(2))));

        // Tick commits 0 and 2, and the uncommitted changes.
        list.toggle_tick();
        list.select(1);
        list.toggle_tick();
        list.select(0);
        list.toggle_tick();
        assert_eq!(
            list.showing(),
            Some(Showing::Together(vec![
                Row::Uncommitted,
                Row::Commit(0),
                Row::Commit(2)
            ]))
        );
        assert!(list.on_screen(Row::Commit(2)) && !list.on_screen(Row::Commit(1)));
        list.select(3);
        assert_eq!(
            list.showing(),
            Some(Showing::Together(vec![
                Row::Uncommitted,
                Row::Commit(0),
                Row::Commit(2)
            ])),
            "the cursor only aims the ticks now"
        );

        // ⇧→ from together starts stepping at the oldest.
        assert!(list.step_by(false));
        assert!(list.one_at_a_time);
        assert_eq!(list.showing(), Some(Showing::Row(Row::Commit(2))));
        assert_eq!(list.step_place(), Some((1, 3)));
        assert_eq!(
            list.selected_row(),
            Some(Row::Commit(2)),
            "the cursor rides along"
        );
        assert!(list.step_by(false));
        assert_eq!(list.showing(), Some(Showing::Row(Row::Commit(0))));
        assert!(list.step_by(false));
        assert_eq!(list.step_place(), Some((3, 3)));
        assert_eq!(list.showing(), Some(Showing::Row(Row::Uncommitted)));
        assert!(!list.step_by(false), "the newest ends it");
        assert!(list.step_by(true));
        assert_eq!(list.step_place(), Some((2, 3)));
        assert_eq!(
            list.head(&list.showing().unwrap(), 0)[0],
            Head::Title("commit 2 of 3 · commit 0".into())
        );

        // Unticking the step hands it to its older neighbour.
        list.toggle_tick();
        assert_eq!(list.showing(), Some(Showing::Row(Row::Commit(2))));
        assert_eq!(list.step_place(), Some((1, 2)));
        // Landing the cursor on a ticked row steps onto it; an unticked one
        // leaves the step where it is.
        list.select(2);
        assert_eq!(list.showing(), Some(Showing::Row(Row::Commit(2))));
        list.select(0);
        assert_eq!(list.showing(), Some(Showing::Row(Row::Uncommitted)));

        // ^G: back to together; ^A ticks every commit, then none at all.
        list.toggle_mode();
        assert!(!list.one_at_a_time);
        list.tick_all();
        assert!(list.all_ticked());
        assert!(
            list.ticked.contains(&Row::Uncommitted),
            "kept on the way up"
        );
        list.tick_all();
        assert!(list.ticked.is_empty());
        assert_eq!(list.showing(), Some(Showing::Row(Row::Uncommitted)));
    }

    /// Where a TOGETHER diff's ranges begin and end: side by side is one
    /// range from the oldest's parent; a row left out, or a merge, starts
    /// another; a run down to the branch's first commit is measured from
    /// the merge-base, across merges; the uncommitted changes end a range
    /// at the working tree.
    #[test]
    fn ticked_rows_become_ranges_split_at_gaps_and_merges() {
        let rev = |s: &str| RangeStart::Rev(s.into());
        let list = list_of(5, true, 5);
        let ranges = list.ranges(&[Row::Commit(1), Row::Commit(2)]);
        assert_eq!(
            ranges,
            [Range {
                from: rev("sha3"),
                to: Some("sha1".into()),
                label: "s2..s1 · 2 commits".into(),
            }]
        );
        let ranges = list.ranges(&[Row::Commit(0), Row::Commit(2)]);
        assert_eq!(ranges.len(), 2, "commit 1 left out between them");
        assert_eq!(
            (ranges[0].from.clone(), ranges[0].to.clone()),
            (rev("sha3"), Some("sha2".into()))
        );
        assert_eq!(
            (ranges[1].from.clone(), ranges[1].to.clone()),
            (rev("sha1"), Some("sha0".into()))
        );
        assert_eq!(ranges[1].label, "s0");

        let ranges = list.ranges(&[Row::Uncommitted, Row::Commit(0)]);
        assert_eq!(ranges.len(), 1, "the working tree sits on HEAD");
        assert_eq!(ranges[0].to, None);
        assert_eq!(ranges[0].from, rev("sha1"));
        assert_eq!(ranges[0].label, "s0..working tree · 1 commit");
        let ranges = list.ranges(&[Row::Uncommitted]);
        assert_eq!(ranges[0].from, rev("sha0"));

        // Down to the first commit: from the merge-base, HEAD's known one.
        let ranges = list.ranges(&[Row::Commit(3), Row::Commit(4)]);
        assert_eq!(
            ranges[0].from,
            RangeStart::MergeBase {
                base: "origin/main".into()
            },
            "not at HEAD: asked of git"
        );
        let all: Vec<Row> = (0..5).map(Row::Commit).collect();
        assert_eq!(list.ranges(&all)[0].from, rev("mb"));

        // A merge between commits 1 and 2 splits them — except in a run
        // that reaches the first commit.
        let merged = list_with(4, false, 4, Some(1));
        assert_eq!(merged.ranges(&[Row::Commit(1), Row::Commit(2)]).len(), 2);
        let all: Vec<Row> = (0..4).map(Row::Commit).collect();
        assert_eq!(merged.ranges(&all).len(), 1, "the whole branch is one diff");

        // With pages still unread, a run to the last commit read is not
        // from the start — unless every commit read is ticked.
        let paged = list_of(3, false, 9);
        assert_eq!(paged.ranges(&[Row::Commit(2)])[0].from, rev("sha3"));
        let all: Vec<Row> = (0..3).map(Row::Commit).collect();
        let mut paged = paged;
        paged.ticked = all.iter().copied().collect();
        assert_eq!(paged.ranges(&all)[0].from, rev("mb"));
    }

    /// The head over a TOGETHER diff says what was ticked: the whole
    /// branch, or how many, each commit named — and, with a gap, that the
    /// ranges come in turn.
    #[test]
    fn the_together_head_names_what_was_ticked() {
        let mut list = list_of(3, false, 3);
        list.tick_all();
        let head = list.head(&list.showing().unwrap(), 0);
        assert_eq!(
            head[0],
            Head::Title("The whole branch since origin/main".into())
        );
        assert!(head.contains(&Head::Item {
            sha: "s2".into(),
            text: "commit 2 · just now".into()
        }));
        list.select(1);
        list.toggle_tick();
        let head = list.head(&list.showing().unwrap(), 0);
        assert_eq!(head[0], Head::Title("2 commits together".into()));
        assert!(
            matches!(&head[1], Head::Meta(m) if m.starts_with("in 2 ranges")),
            "{head:?}"
        );
    }

    #[test]
    fn the_wheel_scrolls_the_list_until_the_cursor_moves() {
        let mut list = list_of(10, false, 10);
        let heights = vec![3; 10];
        assert_eq!(list.window_top(&heights, 9), 0);
        list.select(5);
        assert_eq!(
            list.window_top(&heights, 9),
            3,
            "just enough to show the cursor"
        );
        list.top = 3;
        list.wheel(4);
        assert_eq!(
            list.window_top(&heights, 9),
            7,
            "the wheel moved it, the cursor did not"
        );
        list.wheel(10);
        assert_eq!(
            list.window_top(&heights, 9),
            7,
            "no further than a full panel"
        );
        list.select(6);
        assert_eq!(
            list.window_top(&heights, 9),
            6,
            "the cursor moved: it follows again"
        );
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

    /// A clean checkout opens on its whole branch; with nothing ticked a
    /// commit's row lists exactly its files, renames and deletes too, with
    /// its message over the diff.
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
        assert!(
            matches!(&view.scope, DiffScope::Ranges(r) if r.len() == 1),
            "nothing uncommitted: the branch's changes, {:?}",
            view.scope
        );
        assert_eq!(paths(&view), ["a.txt", "b.txt", "moved.txt"]);
        assert_eq!(
            view.head[0],
            Head::Title("The whole branch since origin/main".into())
        );

        view.commits.as_mut().unwrap().tick_all();
        select(&mut view, Row::Commit(0));
        assert_eq!(paths(&view), ["b.txt", "moved.txt"]);
        let moved = view.files.iter().find(|f| f.path == "moved.txt").unwrap();
        assert_eq!(moved.orig_path.as_deref(), Some("base.txt"));
        assert_eq!(moved.xy[0], 'R');
        assert_eq!(view.head[0], Head::Title("move base, add b".into()));
        assert_eq!(view.head[3], Head::Prose("- the body".into()));
        assert!(view.diff.contains("+beta"), "{}", view.diff);
        assert_eq!(view.scroll, 0, "the first file opens on the message");

        // The next file of the same commit opens past the message, which
        // is a scroll up.
        view.select(1);
        crate::git_diff::load_selected_diff(&mut view);
        assert!(view.diff.contains("rename from base.txt"), "{}", view.diff);
        assert_eq!(view.scroll, view.head_rows());
        assert_eq!(view.doc.facts.status, Some("renamed"));

        select(&mut view, Row::Commit(1));
        assert_eq!(paths(&view), ["a.txt"]);
        assert!(view.diff.contains("+alpha"));
        assert_eq!(view.scroll, 0, "a new commit opens on its message");
    }

    /// ONE AT A TIME through a real branch: each step's own files under
    /// `commit k of n`.
    #[test]
    fn stepping_one_at_a_time_reads_each_ticked_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = branch_repo(&dir);
        commit(&repo, "a.txt", "a\n", "add a");
        commit(&repo, "b.txt", "b\n", "add b");
        commit(&repo, "c.txt", "c\n", "add c");
        let mut view = opened(&repo);
        assert_eq!(paths(&view), ["a.txt", "b.txt", "c.txt"]);
        // Untick the middle one: a and c, together, in two ranges.
        select(&mut view, Row::Commit(1));
        view.commits.as_mut().unwrap().toggle_tick();
        show_selected(&mut view);
        assert_eq!(paths(&view), ["a.txt", "c.txt"]);
        assert!(matches!(&view.scope, DiffScope::Ranges(r) if r.len() == 2));

        view.commits.as_mut().unwrap().toggle_mode();
        show_selected(&mut view);
        assert_eq!(paths(&view), ["a.txt"], "commit 1 of 2 is the oldest");
        assert_eq!(view.head[0], Head::Title("commit 1 of 2 · add a".into()));
        view.commits.as_mut().unwrap().step_by(false);
        show_selected(&mut view);
        assert_eq!(paths(&view), ["c.txt"]);
        assert_eq!(view.head[0], Head::Title("commit 2 of 2 · add c".into()));
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

    /// An answer for a scope the list has since left is dropped; the one
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
