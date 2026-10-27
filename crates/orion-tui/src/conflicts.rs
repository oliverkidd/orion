//! CONFLICTS: a pull request GitHub says no longer merges, file by file,
//! and the CONFLICT RESOLVER that settles them in its checkout.
//!
//! **Which files.** GitHub says only *that* a pull request conflicts
//! (`mergeable: CONFLICTING`), never where. So once a page says so, the
//! project's repo is asked: its base fetched, its head fetched too when the
//! repo lacks it, and `git merge-tree --write-tree` merges the two without
//! touching a checkout. The files it names wear a red `U` on the page's
//! Changes tab, and the tab's label counts them. One answer per head commit
//! and base ([`Known`]): a push moves the head, and the next page that says
//! it still conflicts is asked about again.
//!
//! **Settling them.** Enter on a conflicted file merges the base into the
//! pull request's branch in the project's checkout of it — what GitHub's own
//! *Resolve conflicts* does — and opens the RESOLVER on that file: the
//! files git left conflicted down the left, the file on the right with each
//! conflict drawn as its two sides, the current change (the pull request's
//! branch) over the incoming one (the base), and a row of buttons on each.
//! `c`, `i` and `b` take the current side, the incoming side or both, `u`
//! puts the last one back, and Enter opens the file in the BUILT-IN EDITOR
//! at the conflict for anything the buttons can't say. A file is written
//! the moment a side is taken; nothing is staged until the merge is
//! committed, so Esc can leave it half done and Enter on a conflicted file
//! comes back to it. `⌘S` stages the files, commits the merge and pushes it
//! (`git_sync`'s PUSH); `⌘W` twice aborts it (`git merge --abort`), the
//! checkout back as it was.
//!
//! A checkout with uncommitted changes, or one an agent is working in, is
//! never merged into: the FLASH says why. A checkout behind the pull
//! request's head would merge into old commits, so it says to pull first.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::WorktreeId;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::git_proc::{read, remote_git, run, FETCH_TIMEOUT};
use crate::theme::Theme;

/// Lines one wheel notch scrolls the file.
const WHEEL_LINES: i32 = 3;
/// Lines kept above a conflict the cursor moves to, so its context shows.
const CONTEXT: usize = 4;
/// The file list's share of the modal, and its floor.
const LIST_PCT: u16 = 28;
const MIN_LIST_W: u16 = 22;
/// A tab as the file pane draws it.
const TAB: &str = "    ";

// ---- the file: its conflicts, parsed ----

/// Which side of a conflict to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The pull request's branch: git's `ours`, what HEAD has.
    Current,
    /// The base merged into it: git's `theirs`.
    Incoming,
    /// The current side, then the incoming one.
    Both,
}

impl Choice {
    pub const ALL: [Choice; 3] = [Choice::Current, Choice::Incoming, Choice::Both];

    /// The button's words.
    fn button(self) -> &'static str {
        match self {
            Choice::Current => "accept current",
            Choice::Incoming => "accept incoming",
            Choice::Both => "accept both",
        }
    }
}

/// One conflict as git wrote it into the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Each side's lines, line endings and all.
    pub current: Vec<String>,
    /// The common ancestor's, when `merge.conflictStyle` is `diff3` or
    /// `zdiff3`.
    pub base: Option<Vec<String>>,
    pub incoming: Vec<String>,
    /// What follows `<<<<<<<` and `>>>>>>>`: `HEAD`, `origin/main`.
    pub current_label: String,
    pub incoming_label: String,
    /// The hunk exactly as it stands in the file, markers included: what
    /// the file is written with while it is still open.
    pub raw: String,
}

impl Hunk {
    fn taken(&self, choice: Choice) -> String {
        match choice {
            Choice::Current => self.current.concat(),
            Choice::Incoming => self.incoming.concat(),
            Choice::Both => self.current.concat() + &self.incoming.concat(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Conflict(Hunk),
}

/// A file cut into its plain stretches and its conflicts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc {
    pub parts: Vec<Part>,
}

/// The text after a conflict marker of seven `c`s — `<<<<<<< HEAD` is
/// `Some("HEAD")`, a bare `=======` `Some("")` — or None for a line that
/// is not one. Eight or more is another marker size, so not one either.
fn marker(line: &str, c: char) -> Option<&str> {
    let body = line.trim_end_matches(['\n', '\r']);
    let n = body.chars().take_while(|&ch| ch == c).count();
    if n != 7 {
        return None;
    }
    let rest = &body[7..];
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ')
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Current,
    Base,
    Incoming,
}

/// `text` cut at its conflict markers. A conflict git never closed — the
/// file ends inside one — is left as plain text, as is everything git
/// would not have written: the parse never invents a conflict.
pub fn parse(text: &str) -> Doc {
    let mut parts = Vec::new();
    let mut plain = String::new();
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let Some(current_label) = marker(line, '<') else {
            plain.push_str(line);
            continue;
        };
        let mut hunk = Hunk {
            current: Vec::new(),
            base: None,
            incoming: Vec::new(),
            current_label: current_label.to_string(),
            incoming_label: String::new(),
            raw: line.to_string(),
        };
        let mut side = Side::Current;
        let mut closed = false;
        for line in lines.by_ref() {
            hunk.raw.push_str(line);
            if side == Side::Current && marker(line, '|').is_some() {
                hunk.base = Some(Vec::new());
                side = Side::Base;
            } else if side != Side::Incoming && marker(line, '=') == Some("") {
                side = Side::Incoming;
            } else if side == Side::Incoming && marker(line, '>').is_some() {
                hunk.incoming_label = marker(line, '>').unwrap_or_default().to_string();
                closed = true;
                break;
            } else {
                let lines = match side {
                    Side::Current => &mut hunk.current,
                    Side::Base => hunk.base.get_or_insert_with(Vec::new),
                    Side::Incoming => &mut hunk.incoming,
                };
                lines.push(line.to_string());
            }
        }
        if !closed {
            plain.push_str(&hunk.raw);
            continue;
        }
        if !plain.is_empty() {
            parts.push(Part::Text(std::mem::take(&mut plain)));
        }
        parts.push(Part::Conflict(hunk));
    }
    if !plain.is_empty() {
        parts.push(Part::Text(plain));
    }
    Doc { parts }
}

impl Doc {
    /// How many conflicts are left in it.
    pub fn conflicts(&self) -> usize {
        self.parts
            .iter()
            .filter(|p| matches!(p, Part::Conflict(_)))
            .count()
    }

    /// The file as it should be written: every settled conflict as the
    /// side taken, every open one as git left it.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => t.as_str(),
                Part::Conflict(h) => h.raw.as_str(),
            })
            .collect()
    }

    /// Settle the `n`th conflict still open; false when there is none.
    pub fn resolve(&mut self, n: usize, choice: Choice) -> bool {
        let Some(at) = self
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p, Part::Conflict(_)))
            .nth(n)
            .map(|(i, _)| i)
        else {
            return false;
        };
        let Part::Conflict(hunk) = &self.parts[at] else {
            return false;
        };
        self.parts[at] = Part::Text(hunk.taken(choice));
        true
    }
}

// ---- which files conflict ----

/// What a pull request's head and base were asked, and what came back.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    Asking,
    /// The files that conflict; empty when the repo merged them cleanly,
    /// or the merge was resolved here since.
    Files(Vec<String>),
    Failed,
}

#[derive(Debug, Clone)]
struct Entry {
    head_sha: String,
    base: String,
    asked: Asked,
}

/// Every pull request's conflicted files, by URL, as the repo answered
/// for its head commit and base — and the RESOLVER being opened, if one
/// is (`App::conflicts`).
#[derive(Debug, Default)]
pub struct Known {
    by_url: HashMap<String, Entry>,
    opening: Option<Opening>,
    /// A commit of the merge on its way, by ticket.
    committing: Option<u64>,
}

impl Known {
    /// The files `url` conflicts in at `head_sha` against `base`, once
    /// asked; None while asking, or for another head.
    pub fn files(&self, url: &str, head_sha: &str, base: &str) -> Option<&[String]> {
        let entry = self.by_url.get(url)?;
        if entry.head_sha != head_sha || entry.base != base {
            return None;
        }
        match &entry.asked {
            Asked::Files(files) => Some(files),
            Asked::Asking | Asked::Failed => None,
        }
    }
}

/// The files `url`'s page should mark: the repo's answer for the page's
/// head and base, while GitHub still says it conflicts.
pub(crate) fn of_page<'a>(app: &'a App, url: &str) -> Option<&'a [String]> {
    if !app.prs.status_or_open(url).health.conflicts {
        return None;
    }
    let detail = app.pr_detail.get(url)?;
    app.conflicts
        .files(url, &detail.head_sha, &detail.base)
        .filter(|files| !files.is_empty())
}

/// Ask `dir`'s repo which files `url` conflicts in — when its page says it
/// conflicts and its head and base have not been asked about yet.
pub(crate) fn want(app: &mut App, url: &str, dir: &Path) {
    let Some(detail) = app.pr_detail.get(url) else {
        return;
    };
    if !app.prs.status_or_open(url).health.conflicts
        || detail.head_sha.is_empty()
        || detail.base.is_empty()
        || !dir.is_dir()
    {
        return;
    }
    let (number, head_sha, base) = (detail.number, detail.head_sha.clone(), detail.base.clone());
    if app
        .conflicts
        .by_url
        .get(url)
        .is_some_and(|e| e.head_sha == head_sha && e.base == base)
    {
        return;
    }
    app.conflicts.by_url.insert(
        url.to_string(),
        Entry {
            head_sha: head_sha.clone(),
            base: base.clone(),
            asked: Asked::Asking,
        },
    );
    let (url, dir, quit) = (
        url.to_string(),
        dir.to_path_buf(),
        app.git_sync.quit.clone(),
    );
    let work = move || {
        let files = conflicted(&dir, number, &base, &head_sha, &quit);
        Answer::Found {
            url,
            head_sha,
            base,
            files,
        }
    };
    off_loop(app, work);
}

/// Whether `dir`'s repo has `rev` as a commit.
fn has_commit(dir: &Path, rev: &str) -> bool {
    read(dir, &["cat-file", "-e", &format!("{rev}^{{commit}}")]).is_ok()
}

/// The files pull request `number` conflicts in: `origin/<base>` fetched
/// fresh, the head fetched by its pull ref when the repo lacks it, and the
/// two merged in memory (`git merge-tree`, git 2.38 and later).
fn conflicted(
    dir: &Path,
    number: u64,
    base: &str,
    head_sha: &str,
    quit: &AtomicBool,
) -> Result<Vec<String>, String> {
    let fetched = remote_git(
        dir,
        &["fetch", "--quiet", "--no-tags", "origin", base],
        FETCH_TIMEOUT,
        quit,
    );
    let base_ref = format!("origin/{base}");
    if !has_commit(dir, &base_ref) {
        return Err(fetched
            .err()
            .unwrap_or_else(|| format!("no {base_ref} here")));
    }
    if !has_commit(dir, head_sha) {
        let pull = format!("refs/pull/{number}/head");
        remote_git(
            dir,
            &["fetch", "--quiet", "--no-tags", "origin", &pull],
            FETCH_TIMEOUT,
            quit,
        )?;
    }
    merge_tree(dir, &base_ref, head_sha)
}

/// `git merge-tree --write-tree --name-only -z`: exit 0 is a clean merge,
/// exit 1 a conflicted one whose files follow the tree's id.
fn merge_tree(dir: &Path, base: &str, head: &str) -> Result<Vec<String>, String> {
    let out = crate::git_diff::run_git(
        dir,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "-z",
            base,
            head,
        ],
    )?;
    match out.status.code() {
        Some(0) => Ok(Vec::new()),
        Some(1) => Ok(merge_tree_files(&out.stdout)),
        _ => Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .next()
            .unwrap_or("git merge-tree failed")
            .trim_start_matches("fatal: ")
            .to_string()),
    }
}

/// The file names `-z` lists after the tree's id, up to the empty field
/// that ends them.
fn merge_tree_files(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .split('\0')
        .skip(1)
        .take_while(|f| !f.is_empty())
        .map(str::to_string)
        .collect()
}

// ---- the off-loop answers ----

#[derive(Debug)]
pub enum Answer {
    /// Which files a pull request conflicts in ([`want`]).
    Found {
        url: String,
        head_sha: String,
        base: String,
        files: Result<Vec<String>, String>,
    },
    /// The checkout merged into, for the RESOLVER being opened ([`open`]).
    Prepared {
        ticket: u64,
        result: Result<Prepared, String>,
    },
    /// The merge committed ([`commit`]).
    Committed {
        ticket: u64,
        result: Result<(), String>,
    },
}

/// Run `work` on the blocking pool and land its answer — inline where
/// there is no pool (the unit tests).
fn off_loop(app: &mut App, work: impl FnOnce() -> Answer + Send + 'static) {
    match app.view_jobs.clone() {
        Some(jobs) => jobs.run(move || Some(crate::view_jobs::Answer::Conflicts(work()))),
        None => land(app, work()),
    }
}

/// Land an off-loop answer.
pub(crate) fn land(app: &mut App, answer: Answer) {
    match answer {
        Answer::Found {
            url,
            head_sha,
            base,
            files,
        } => {
            let Some(entry) = app.conflicts.by_url.get_mut(&url) else {
                return;
            };
            if entry.head_sha != head_sha || entry.base != base {
                return;
            }
            entry.asked = match files {
                Ok(files) => Asked::Files(files),
                Err(_) => Asked::Failed,
            };
        }
        Answer::Prepared { ticket, result } => land_prepared(app, ticket, result),
        Answer::Committed { ticket, result } => land_committed(app, ticket, result),
    }
    app.dirty = true;
}

// ---- the checkout, merged into ----

/// How a conflicted file was left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Conflict markers to settle.
    Text,
    /// Both sides changed a file git can't merge line by line.
    Binary,
    /// Deleted on the current side, changed on the incoming one.
    DeletedCurrent,
    /// Changed on the current side, deleted on the incoming one.
    DeletedIncoming,
}

/// One file git left conflicted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictFile {
    pub path: String,
    pub kind: Kind,
    /// Conflicts still open in it. A file with no markers to settle — a
    /// binary one, one deleted on one side — counts one until a side is
    /// taken.
    pub left: usize,
}

/// What [`prepare`] left the checkout in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// A merge stopped on these files.
    Conflicts(Vec<ConflictFile>),
    /// The base merged in without a conflict, and was committed.
    Clean,
}

/// A RESOLVER on its way up: the merge running off the loop.
#[derive(Debug, Clone)]
struct Opening {
    ticket: u64,
    url: String,
    label: String,
    path: Option<String>,
    worktree: WorktreeId,
    root: PathBuf,
    branch: String,
    base: String,
}

/// Whether a merge is underway in `root`.
fn merging(root: &Path) -> bool {
    read(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok()
}

/// Merge `origin/<base>` into `root`'s branch — fetched fresh first — and
/// list what it left conflicted. A merge already underway is picked up
/// where it was left. Refused, with git's or orion's reason, when the
/// checkout has uncommitted changes or does not have the pull request's
/// head commit yet.
pub(crate) fn prepare(
    root: &Path,
    branch: &str,
    base: &str,
    head_sha: &str,
    quit: &AtomicBool,
) -> Result<Prepared, String> {
    if merging(root) {
        return unmerged(root).map(Prepared::Conflicts);
    }
    let dirty = read(root, &["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.trim().is_empty() {
        return Err(format!(
            "⎇ {branch} has uncommitted changes — commit or stash them first"
        ));
    }
    let behind = !head_sha.is_empty()
        && read(root, &["merge-base", "--is-ancestor", head_sha, "HEAD"]).is_err();
    if behind {
        return Err(format!(
            "⎇ {branch} is behind the pull request — pull it first"
        ));
    }
    remote_git(
        root,
        &["fetch", "--quiet", "--no-tags", "origin", base],
        FETCH_TIMEOUT,
        quit,
    )?;
    let base_ref = format!("origin/{base}");
    match run(root, &["merge", "--no-edit", "--quiet", &base_ref]) {
        Ok(_) => Ok(Prepared::Clean),
        Err(why) => match unmerged(root) {
            Ok(files) if merging(root) && !files.is_empty() => Ok(Prepared::Conflicts(files)),
            _ => Err(why),
        },
    }
}

/// The files the index holds unmerged (`git ls-files -u -z`), each read to
/// count its conflicts.
fn unmerged(root: &Path) -> Result<Vec<ConflictFile>, String> {
    let listing = read(root, &["ls-files", "-u", "-z"])?;
    let mut stages: Vec<(String, [bool; 3])> = Vec::new();
    for record in listing.split('\0').filter(|r| !r.is_empty()) {
        // `<mode> <oid> <stage>\t<path>`
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        let stage: usize = meta
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if !(1..=3).contains(&stage) {
            continue;
        }
        match stages.iter_mut().find(|(p, _)| p == path) {
            Some((_, has)) => has[stage - 1] = true,
            None => {
                let mut has = [false; 3];
                has[stage - 1] = true;
                stages.push((path.to_string(), has));
            }
        }
    }
    Ok(stages
        .into_iter()
        .map(|(path, [_, current, incoming])| {
            let kind = match (current, incoming) {
                (false, _) => Kind::DeletedCurrent,
                (_, false) => Kind::DeletedIncoming,
                _ => match std::fs::read(root.join(&path)) {
                    Ok(bytes) if is_text(&bytes) => Kind::Text,
                    _ => Kind::Binary,
                },
            };
            let mut file = ConflictFile {
                path,
                kind,
                left: 0,
            };
            file.left = count(root, &file);
            file
        })
        .collect())
}

/// Text git would have written markers into: UTF-8 with no NUL in it.
fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// The conflicts still open in `file` as it is on disk.
fn count(root: &Path, file: &ConflictFile) -> usize {
    match file.kind {
        Kind::Text => std::fs::read_to_string(root.join(&file.path))
            .map(|t| parse(&t).conflicts())
            .unwrap_or(0),
        Kind::Binary | Kind::DeletedCurrent | Kind::DeletedIncoming => 1,
    }
}

/// Enter on a conflicted file of pull request `url`'s page: merge its base
/// into its branch in the project's checkout of it, off the loop, and
/// open the RESOLVER on `path` once that lands ([`land_prepared`]). A pull
/// request nobody has checked out is checked out first — the checkout a PR
/// SESSION would run in (`pr_checkout`) — and the merge follows.
pub(crate) fn open(
    app: &mut App,
    url: &str,
    label: &str,
    path: Option<&str>,
    out: &mut Vec<orion_core::ClientRequest>,
) {
    let Some(detail) = app.pr_detail.get(url).cloned() else {
        return;
    };
    // The open list names a fork's checkout for its owner; a pull request
    // gone from it is named as GitHub's page names its branch.
    let found = crate::pr_checkout::of_url(app, url).or_else(|| {
        let project = match &app.overlay {
            Some(Overlay::PullRequests(view)) => view.project.clone(),
            _ => app.selected_project()?.id.clone(),
        };
        let launch = crate::pull_request::PrLaunch {
            url: url.to_string(),
            head: detail.head.clone(),
            number: detail.number,
        };
        Some((project, launch))
    });
    let Some((project, launch)) = found else {
        return;
    };
    let Some(worktree) = crate::pr_checkout::worktree_of(app, &project, &launch.head).cloned()
    else {
        let then = crate::pr_checkout::Then::Resolve {
            url: url.to_string(),
            label: label.to_string(),
            path: path.map(str::to_string),
        };
        crate::pr_checkout::check_out(app, project, launch, then, out);
        return;
    };
    let working = app
        .tree
        .agents
        .iter()
        .any(|a| a.worktree_id == worktree.id && !a.archived && app.spins(a));
    if working {
        app.flash = Some(Flash::failed(format!(
            "an agent is working in ⎇ {} — resolve its conflicts once it stops",
            worktree.branch
        )));
        return;
    }
    if app.git_sync.inflight.contains_key(&worktree.id) {
        app.flash = Some(Flash::failed(format!(
            "⎇ {} is syncing — try again once it lands",
            worktree.branch
        )));
        return;
    }
    if app.conflicts.opening.is_some() {
        return;
    }
    let ticket = crate::view_jobs::ticket();
    app.conflicts.opening = Some(Opening {
        ticket,
        url: url.to_string(),
        label: label.to_string(),
        path: path.map(str::to_string),
        worktree: worktree.id.clone(),
        root: worktree.path.clone(),
        branch: worktree.branch.clone(),
        base: detail.base.clone(),
    });
    app.flash = Some(Flash::working(format!(
        "merging origin/{} into ⎇ {}…",
        detail.base, worktree.branch
    )));
    let (root, branch, base, head_sha) =
        (worktree.path, worktree.branch, detail.base, detail.head_sha);
    let quit = Arc::clone(&app.git_sync.quit);
    let work = move || Answer::Prepared {
        ticket,
        result: prepare(&root, &branch, &base, &head_sha, &quit),
    };
    off_loop(app, work);
}

/// The merge for the RESOLVER being opened landed. Conflicts open it — over
/// the PULL REQUESTS MODAL it was asked from, as a level inside it, or
/// over the panels — unless something else was opened meanwhile, which
/// is left alone and told how to come back.
fn land_prepared(app: &mut App, ticket: u64, result: Result<Prepared, String>) {
    let Some(opening) = app.conflicts.opening.take_if(|o| o.ticket == ticket) else {
        return;
    };
    let files = match result {
        Err(why) => {
            app.flash = Some(Flash::failed(why));
            return;
        }
        Ok(Prepared::Clean) => {
            settled(app, &opening.url);
            app.flash = Some(Flash::done(format!(
                "merged origin/{} into ⎇ {} with nothing left to resolve — push it to update the pull request",
                opening.base, opening.branch
            )));
            return;
        }
        Ok(Prepared::Conflicts(files)) => files,
    };
    let back = match app.overlay.take() {
        Some(Overlay::PullRequests(view)) => Some(Box::new(view)),
        None => None,
        other => {
            app.overlay = other;
            app.flash = Some(Flash::note(format!(
                "⎇ {} is mid-merge — Enter on a conflicted file opens it",
                opening.branch
            )));
            return;
        }
    };
    app.flash = None;
    let mut view = ConflictView::new(opening.root, opening.branch, opening.base, files);
    view.back = back;
    view.url = opening.url;
    view.label = opening.label;
    view.worktree = opening.worktree;
    view.open_at(opening.path.as_deref());
    app.overlay = Some(Overlay::Conflicts(Box::new(view)));
}

/// The merge for `url` is over — committed, or clean — so its page stops
/// marking the files GitHub's stale answer still calls conflicted.
fn settled(app: &mut App, url: &str) {
    if let Some(entry) = app.conflicts.by_url.get_mut(url) {
        entry.asked = Asked::Files(Vec::new());
    }
}

// ---- the RESOLVER ----

/// The CONFLICT RESOLVER: the merge's conflicted files, and the one under
/// the cursor with its conflicts drawn to settle.
#[derive(Debug, Clone)]
pub struct ConflictView {
    /// The PULL REQUESTS MODAL it was opened from: Esc puts it back.
    pub back: Option<Box<crate::pr_modal::PullRequestsView>>,
    pub url: String,
    /// `#42 Fix login`, for the title.
    pub label: String,
    pub worktree: WorktreeId,
    pub root: PathBuf,
    /// The pull request's branch, and the base merged into it.
    pub branch: String,
    pub base: String,
    pub files: Vec<ConflictFile>,
    pub selected: usize,
    /// The selected file, parsed — None for one with no markers.
    pub doc: Option<Doc>,
    /// The conflict the cursor is on, among the selected file's open ones.
    pub hunk: usize,
    /// The file pane's top line.
    pub scroll: usize,
    /// Each file as it was before a side was taken, newest last: `u`.
    pub undo: Vec<(String, String)>,
    /// The resolver's own line — what went wrong, or what a key did.
    pub note: Option<Flash>,
    /// `⌘W` was pressed once: the second aborts the merge.
    pub abort_armed: bool,
    /// Where it was last drawn, for the mouse: the modal, the file list,
    /// the file pane, each conflict's buttons and each file's row.
    pub area: Rect,
    pub list_area: Rect,
    pub body_area: Rect,
    pub buttons: Vec<(Rect, usize, Button)>,
    pub row_rects: Vec<(Rect, usize)>,
    pub view_height: u16,
}

/// A button over a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Take(Choice),
    Edit,
}

impl ConflictView {
    /// A resolver on `files`, the merge of `origin/<base>` into `branch`
    /// in `root`, belonging to no pull request yet.
    pub(crate) fn new(
        root: PathBuf,
        branch: String,
        base: String,
        files: Vec<ConflictFile>,
    ) -> Self {
        Self {
            back: None,
            url: String::new(),
            label: String::new(),
            worktree: WorktreeId(String::new()),
            root,
            branch,
            base,
            files,
            selected: 0,
            doc: None,
            hunk: 0,
            scroll: 0,
            undo: Vec::new(),
            note: None,
            abort_armed: false,
            area: Rect::default(),
            list_area: Rect::default(),
            body_area: Rect::default(),
            buttons: Vec::new(),
            row_rects: Vec::new(),
            view_height: 0,
        }
    }

    /// The cursor on `path`, else the first file with a conflict left.
    fn open_at(&mut self, path: Option<&str>) {
        let at = path
            .and_then(|p| self.files.iter().position(|f| f.path == p))
            .or_else(|| self.files.iter().position(|f| f.left > 0))
            .unwrap_or(0);
        self.select(at);
    }

    fn file(&self) -> Option<&ConflictFile> {
        self.files.get(self.selected)
    }

    /// Put the cursor on file `i`, read, its first conflict in view.
    fn select(&mut self, i: usize) {
        self.selected = i.min(self.files.len().saturating_sub(1));
        self.hunk = 0;
        self.reload();
        self.scroll_to_hunk();
    }

    /// Read the selected file again, and every file's count: the editor
    /// may have changed any of them. A file with no markers keeps the
    /// count its side being taken left it at — that was staged.
    pub(crate) fn reload(&mut self) {
        let root = self.root.clone();
        for file in self.files.iter_mut().filter(|f| f.kind == Kind::Text) {
            file.left = count(&root, file);
        }
        self.doc = self
            .file()
            .filter(|f| f.kind == Kind::Text)
            .map(|f| parse(&std::fs::read_to_string(root.join(&f.path)).unwrap_or_default()));
        let open = self.doc.as_ref().map_or(0, Doc::conflicts);
        self.hunk = self.hunk.min(open.saturating_sub(1));
    }

    /// The file pane's line the cursor's conflict starts on.
    fn hunk_line(&self) -> Option<usize> {
        rows(self.doc.as_ref()?)
            .iter()
            .position(|r| matches!(r, Row::Head { hunk } if *hunk == self.hunk))
    }

    fn scroll_to_hunk(&mut self) {
        self.scroll = self.hunk_line().map_or(0, |l| l.saturating_sub(CONTEXT));
    }

    /// The line `<<<<<<<` of the cursor's conflict is on in the file, for
    /// the editor; the first line when there is none.
    fn file_line(&self) -> u64 {
        let Some(doc) = &self.doc else {
            return 1;
        };
        let mut line = 1;
        let mut n = 0;
        for part in &doc.parts {
            match part {
                Part::Text(t) => line += t.split_inclusive('\n').count(),
                Part::Conflict(_) if n == self.hunk => return line as u64,
                Part::Conflict(h) => {
                    line += h.raw.split_inclusive('\n').count();
                    n += 1;
                }
            }
        }
        1
    }

    /// Conflicts still open across every file.
    fn left(&self) -> usize {
        self.files.iter().map(|f| f.left).sum()
    }

    /// `↓`/`↑`: the next or the previous conflict, on into the next file
    /// with one — round either end.
    fn step(&mut self, delta: i64) {
        let open = self.doc.as_ref().map_or(0, Doc::conflicts);
        let next = self.hunk as i64 + delta;
        if open > 0 && (0..open as i64).contains(&next) {
            self.hunk = next as usize;
            self.scroll_to_hunk();
            return;
        }
        let n = self.files.len();
        for k in 1..=n {
            let i = (self.selected as i64 + delta * k as i64).rem_euclid(n as i64) as usize;
            if self.files[i].left > 0 || i == self.selected {
                self.select(i);
                if delta < 0 {
                    let open = self.doc.as_ref().map_or(0, Doc::conflicts);
                    self.hunk = open.saturating_sub(1);
                    self.scroll_to_hunk();
                }
                return;
            }
        }
    }

    /// The next file with a conflict left, after this one.
    fn advance(&mut self) {
        let n = self.files.len();
        if let Some(i) = (1..n)
            .map(|k| (self.selected + k) % n)
            .find(|&i| self.files[i].left > 0)
        {
            self.select(i);
        }
    }
}

/// Take `choice` on the cursor's conflict — or, on a file with no
/// markers, on the file itself — and write it.
fn take(view: &mut ConflictView, choice: Choice) {
    let Some(file) = view.file().cloned() else {
        return;
    };
    view.abort_armed = false;
    if file.kind != Kind::Text {
        take_file(view, &file, choice);
        return;
    }
    let Some(doc) = &mut view.doc else {
        return;
    };
    let before = doc.text();
    if !doc.resolve(view.hunk, choice) {
        return;
    }
    let after = doc.text();
    if let Err(e) = std::fs::write(view.root.join(&file.path), &after) {
        view.note = Some(Flash::failed(format!("couldn't write {}: {e}", file.path)));
        view.reload();
        return;
    }
    view.undo.push((file.path.clone(), before));
    view.reload();
    if view.files[view.selected].left == 0 {
        view.note = Some(Flash::done(format!("{} resolved", file.path)));
        view.advance();
    } else {
        view.note = None;
        view.scroll_to_hunk();
    }
}

/// A side taken on a file with no markers: its version checked out and
/// staged, or the delete staged. Both sides of one can't be kept.
fn take_file(view: &mut ConflictView, file: &ConflictFile, choice: Choice) {
    let path = file.path.as_str();
    let deletes = matches!(
        (file.kind, choice),
        (Kind::DeletedCurrent, Choice::Current) | (Kind::DeletedIncoming, Choice::Incoming)
    );
    let done = match choice {
        Choice::Both => {
            view.note = Some(Flash::failed(format!(
                "{path} can't keep both sides — take one, or edit it"
            )));
            return;
        }
        _ if deletes => run(&view.root, &["rm", "--quiet", "--", path]).map(|_| "deleted"),
        Choice::Current | Choice::Incoming => {
            let side = if choice == Choice::Current {
                "--ours"
            } else {
                "--theirs"
            };
            run(&view.root, &["checkout", side, "--", path])
                .and_then(|_| run(&view.root, &["add", "--", path]))
                .map(|_| "kept")
        }
    };
    match done {
        Ok(what) => {
            view.files[view.selected].left = 0;
            view.note = Some(Flash::done(format!("{path} {what}")));
            view.advance();
        }
        Err(why) => view.note = Some(Flash::failed(why)),
    }
}

/// `u`: the file the last side was taken in, as it was before.
fn undo(view: &mut ConflictView) {
    let Some((path, text)) = view.undo.pop() else {
        view.note = Some(Flash::note("nothing to undo"));
        return;
    };
    if let Err(e) = std::fs::write(view.root.join(&path), text) {
        view.note = Some(Flash::failed(format!("couldn't write {path}: {e}")));
        return;
    }
    let i = view.files.iter().position(|f| f.path == path);
    if let Some(i) = i.filter(|&i| i != view.selected) {
        view.select(i);
    } else {
        view.reload();
        view.scroll_to_hunk();
    }
    view.note = Some(Flash::note(format!("put {path} back")));
}

/// Enter: the file in the BUILT-IN EDITOR at the cursor's conflict; the
/// resolver reads it again when the editor closes (`event_loop::close_vim`).
fn edit(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &app.overlay else {
        return;
    };
    let Some(file) = view.file().filter(|f| f.kind != Kind::DeletedCurrent) else {
        return;
    };
    let (root, path, line) = (view.root.clone(), file.path.clone(), view.file_line());
    let editor = crate::config::Config::load().editor_command();
    let size = crate::event_loop::vim_size_guess(app);
    crate::event_loop::spawn_editor_modal(app, &editor, &root, &path, line, size);
}

/// `⌘O`: the file in the OPEN IN APP editor, at the cursor's conflict.
fn outside(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &app.overlay else {
        return;
    };
    let Some(file) = view.file() else {
        return;
    };
    let (root, path, line) = (view.root.clone(), file.path.clone(), view.file_line());
    crate::event_loop::open_file_outside(app, &root, &path, line);
}

/// `⌘S`: every file staged and the merge committed, off the loop; the push
/// follows once it lands ([`land_committed`]). Refused while a conflict
/// is left.
fn commit(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &mut app.overlay else {
        return;
    };
    view.abort_armed = false;
    if app.conflicts.committing.is_some() {
        return;
    }
    let left = view.left();
    if left > 0 {
        let files = view.files.iter().filter(|f| f.left > 0).count();
        view.note = Some(Flash::failed(format!(
            "{} left in {} — settle {} before committing",
            crate::bundle::plural(left, "conflict"),
            crate::bundle::plural(files, "file"),
            if left == 1 { "it" } else { "them" }
        )));
        return;
    }
    let ticket = crate::view_jobs::ticket();
    app.conflicts.committing = Some(ticket);
    view.note = Some(Flash::working("committing the merge…"));
    let root = view.root.clone();
    let paths: Vec<String> = view
        .files
        .iter()
        .filter(|f| f.kind == Kind::Text)
        .map(|f| f.path.clone())
        .collect();
    let work = move || Answer::Committed {
        ticket,
        result: commit_merge(&root, &paths),
    };
    off_loop(app, work);
}

/// `git add` the settled files, then `git commit --no-edit`: the merge's
/// own message. Hooks run, detached (`git_proc`).
fn commit_merge(root: &Path, paths: &[String]) -> Result<(), String> {
    if !paths.is_empty() {
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        run(root, &args)?;
    }
    if !read(root, &["ls-files", "-u"])?.trim().is_empty() {
        return Err("git still has files unmerged — settle them first".into());
    }
    run(root, &["commit", "--no-edit", "--quiet"]).map(|_| ())
}

/// The merge committed: back to where the resolver was opened from, and
/// the branch pushed so the pull request updates. A refusal — a hook that
/// failed — stays in the resolver with git's reason.
fn land_committed(app: &mut App, ticket: u64, result: Result<(), String>) {
    if app.conflicts.committing != Some(ticket) {
        return;
    }
    app.conflicts.committing = None;
    let Some(Overlay::Conflicts(view)) = &mut app.overlay else {
        if result.is_err() {
            app.flash = Some(Flash::failed(
                "the merge didn't commit — open it again to see why",
            ));
        }
        return;
    };
    if let Err(why) = result {
        view.note = Some(Flash::failed(why));
        return;
    }
    let (url, worktree) = (view.url.clone(), view.worktree.clone());
    back(app);
    settled(app, &url);
    crate::git_sync::request(app, worktree, crate::git_sync::Op::PUSH);
}

/// `⌘W`, twice: `git merge --abort`, the checkout back as it was.
fn abort(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &mut app.overlay else {
        return;
    };
    if !view.abort_armed {
        view.abort_armed = true;
        view.note = Some(Flash::setup(format!(
            "{} again aborts the merge — every side taken is lost",
            keys::ABORT.label()
        )));
        return;
    }
    match run(&view.root, &["merge", "--abort"]) {
        Ok(_) => {
            let branch = view.branch.clone();
            back(app);
            app.flash = Some(Flash::done(format!(
                "merge aborted — ⎇ {branch} is back as it was"
            )));
        }
        Err(why) => {
            view.abort_armed = false;
            view.note = Some(Flash::failed(why));
        }
    }
}

/// Leave the resolver for the PULL REQUESTS MODAL it was opened from, or
/// the panels; a merge still underway is left as it is.
pub(crate) fn back(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = app.overlay.take() else {
        return;
    };
    match view.back {
        Some(back) => crate::pr_modal::reopen(app, *back),
        None => app.overlay = None,
    }
}

/// Esc: [`back`], saying the merge waits when one does.
fn leave(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &app.overlay else {
        return;
    };
    let waiting = merging(&view.root).then(|| view.branch.clone());
    back(app);
    if let Some(branch) = waiting {
        app.flash = Some(Flash::note(format!(
            "⎇ {branch} is still mid-merge — Enter on a conflicted file comes back to it"
        )));
    }
}

pub(crate) mod keys {
    use crate::hints::Key;

    pub const CURRENT: Key = Key::new(&["c"], "accept current");
    pub const INCOMING: Key = Key::new(&["i"], "accept incoming");
    pub const BOTH: Key = Key::new(&["b"], "accept both");
    pub const EDIT: Key = Key::new(&["enter"], "edit");
    pub const STEP: Key = Key::new(&["down", "up"], "conflict").show(2);
    pub const FILE: Key = Key::new(&["tab", "shift+tab"], "file");
    pub const UNDO: Key = Key::new(&["u"], "undo");
    pub const COMMIT: Key = Key::new(&["cmd+s", "ctrl+s"], "commit & push");
    pub const ABORT: Key = Key::new(&["cmd+w", "ctrl+w"], "abort merge");
    pub const OUTSIDE: Key = crate::hints::IN_CURSOR;
    pub const BACK: Key = Key::new(&["esc"], "back");

    #[cfg(test)]
    pub const ALL: &[Key] = &[
        CURRENT, INCOMING, BOTH, EDIT, STEP, FILE, UNDO, COMMIT, ABORT, OUTSIDE, BACK,
    ];

    /// The key for `choice`.
    pub fn of(choice: super::Choice) -> Key {
        match choice {
            super::Choice::Current => CURRENT,
            super::Choice::Incoming => INCOMING,
            super::Choice::Both => BOTH,
        }
    }
}

/// The keys along the bottom edge: the sides to take while one is left,
/// committing once none is.
pub(crate) fn hints(view: &ConflictView) -> Vec<crate::hints::Hint> {
    let mut hints = Vec::new();
    if view.file().is_some_and(|f| f.left > 0) {
        hints.extend([
            keys::CURRENT.hint().kept(),
            keys::INCOMING.hint().kept(),
            keys::BOTH.hint(),
        ]);
    }
    hints.push(keys::EDIT.hint());
    if view.left() == 0 {
        hints.push(keys::COMMIT.hint().kept());
    }
    hints.extend([keys::STEP.hint(), keys::FILE.hint()]);
    if !view.undo.is_empty() {
        hints.push(keys::UNDO.hint());
    }
    hints.extend([keys::ABORT.hint(), keys::OUTSIDE.hint(), keys::BACK.hint()]);
    hints
}

/// Keys in the RESOLVER.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    use crossterm::event::KeyCode;
    app.dirty = true;
    if keys::BACK.matches(&key) {
        return leave(app);
    }
    if keys::EDIT.matches(&key) {
        return edit(app);
    }
    if keys::OUTSIDE.matches(&key) {
        return outside(app);
    }
    if keys::COMMIT.matches(&key) {
        return commit(app);
    }
    if keys::ABORT.matches(&key) {
        return abort(app);
    }
    let Some(Overlay::Conflicts(view)) = &mut app.overlay else {
        return;
    };
    if let Some(choice) = Choice::ALL.into_iter().find(|c| keys::of(*c).matches(&key)) {
        return take(view, choice);
    }
    view.abort_armed = false;
    let page = usize::from(view.view_height.max(2) - 1);
    match key.code {
        KeyCode::Down => view.step(1),
        KeyCode::Up => view.step(-1),
        KeyCode::Tab => view.select((view.selected + 1) % view.files.len().max(1)),
        KeyCode::BackTab => {
            view.select((view.selected + view.files.len().max(1) - 1) % view.files.len().max(1))
        }
        KeyCode::PageDown => view.scroll += page,
        KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(page),
        _ if keys::UNDO.matches(&key) => undo(view),
        _ => {}
    }
}

/// The wheel scrolls the file; a click on a file opens it, and one on a
/// conflict's button presses it.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let pos = Position::new(mouse.column, mouse.row);
    app.dirty = true;
    let Some(Overlay::Conflicts(view)) = &mut app.overlay else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollDown if view.body_area.contains(pos) => {
            view.scroll += WHEEL_LINES as usize;
        }
        MouseEventKind::ScrollUp if view.body_area.contains(pos) => {
            view.scroll = view.scroll.saturating_sub(WHEEL_LINES as usize);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(&(_, i)) = view.row_rects.iter().find(|(r, _)| r.contains(pos)) {
                view.select(i);
                return;
            }
            let Some(&(_, hunk, button)) = view.buttons.iter().find(|(r, ..)| r.contains(pos))
            else {
                return;
            };
            view.hunk = hunk;
            match button {
                Button::Take(choice) => take(view, choice),
                Button::Edit => edit(app),
            }
        }
        _ => {}
    }
}

// ---- drawing ----

/// One line of the file pane.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row<'a> {
    /// A line outside any conflict, numbered.
    Text { n: usize, text: &'a str },
    /// A conflict's `<<<<<<<`: its buttons.
    Head { hunk: usize },
    /// One of the conflict's lines, on its side.
    Line {
        hunk: usize,
        side: Side,
        text: &'a str,
    },
    /// Where the base's lines, then the incoming ones, start.
    Rule { hunk: usize, side: Side },
    /// A conflict's `>>>>>>>`.
    Tail { hunk: usize },
}

/// `doc` as the file pane's lines.
fn rows(doc: &Doc) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let mut n = 1;
    let mut hunk = 0;
    for part in &doc.parts {
        match part {
            Part::Text(t) => {
                for text in t.split_inclusive('\n') {
                    rows.push(Row::Text { n, text });
                    n += 1;
                }
            }
            Part::Conflict(h) => {
                rows.push(Row::Head { hunk });
                for text in &h.current {
                    rows.push(Row::Line {
                        hunk,
                        side: Side::Current,
                        text,
                    });
                }
                if let Some(base) = &h.base {
                    rows.push(Row::Rule {
                        hunk,
                        side: Side::Base,
                    });
                    for text in base {
                        rows.push(Row::Line {
                            hunk,
                            side: Side::Base,
                            text,
                        });
                    }
                }
                rows.push(Row::Rule {
                    hunk,
                    side: Side::Incoming,
                });
                for text in &h.incoming {
                    rows.push(Row::Line {
                        hunk,
                        side: Side::Incoming,
                        text,
                    });
                }
                rows.push(Row::Tail { hunk });
                n += h.raw.split_inclusive('\n').count();
                hunk += 1;
            }
        }
    }
    rows
}

/// A line of the file as the pane shows it: its ending gone, tabs spread.
fn shown(text: &str) -> String {
    text.trim_end_matches(['\n', '\r']).replace('\t', TAB)
}

/// The RESOLVER: the files down the left, the selected one on the right.
pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &ConflictView, th: Theme) {
    let area = crate::ui::centered_rect_pct(
        f.area(),
        crate::ui::SPLIT_MODAL_PCT.0,
        crate::ui::SPLIT_MODAL_PCT.1,
    );
    f.render_widget(Clear, area);
    let list_w = (area.width * LIST_PCT / 100)
        .max(MIN_LIST_W)
        .min(area.width.saturating_sub(crate::ui::SPLIT_PANE_LAYOUT_MIN));
    let [list_a, body_a] = Layout::horizontal([
        Constraint::Length(list_w),
        Constraint::Min(crate::ui::SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);

    // ---- left: the files ----
    let left = view.left();
    let title = if left == 0 {
        format!("Conflicts — {} · all resolved", view.label)
    } else {
        format!("Conflicts — {} · {} left", view.label, left)
    };
    let block = crate::ui::panel_block(&title, false, th);
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    let mut row_rects = Vec::new();
    let mut lines = vec![
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("origin/{}", view.base),
                Style::default().fg(th.modified),
            ),
            Span::styled(" into ", Style::default().fg(th.dim)),
            Span::styled(format!("⎇ {}", view.branch), Style::default().fg(th.added)),
        ]),
        Line::from(""),
    ];
    let width = usize::from(list_inner.width);
    for (i, file) in view.files.iter().enumerate() {
        let (mark, color) = if file.left == 0 {
            ("✓".to_string(), th.ok)
        } else {
            (file.left.to_string(), th.err)
        };
        let lead = 2 + mark.width() + 1;
        let path = crate::ui::truncate_left(&file.path, width.saturating_sub(lead + 1));
        let mut style = Style::default().fg(if file.left == 0 { th.muted } else { th.text });
        let mut mark_style = Style::default().fg(color);
        if i == view.selected {
            style = style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
            mark_style = mark_style.bg(th.sel_bg);
        }
        let pad = width.saturating_sub(lead + path.width());
        lines.push(Line::from(vec![
            Span::styled("  ", mark_style),
            Span::styled(format!("{mark} "), mark_style),
            Span::styled(format!("{path}{}", " ".repeat(pad)), style),
        ]));
        let y = list_inner.y + lines.len() as u16 - 1;
        if y < list_inner.y + list_inner.height {
            row_rects.push((
                Rect {
                    y,
                    height: 1,
                    ..list_inner
                },
                i,
            ));
        }
    }
    f.render_widget(Paragraph::new(lines), list_inner);

    // ---- right: the file ----
    let file = view.file();
    let title = file.map_or("", |f| f.path.as_str());
    let block = crate::ui::panel_block(title, true, th);
    let body = block.inner(body_a);
    f.render_widget(block, body_a);
    let note_h = u16::from(view.note.is_some());
    let text_area = Rect {
        height: body.height.saturating_sub(note_h),
        ..body
    };
    let mut buttons = Vec::new();
    let mut scroll = view.scroll;
    let mut lines: Vec<Line> = Vec::new();
    match (file, &view.doc) {
        (None, _) => lines.push(Line::from(Span::styled(
            "  nothing conflicted",
            Style::default().fg(th.dim),
        ))),
        (Some(file), Some(doc)) => {
            let rows = rows(doc);
            let gutter = rows.len().to_string().len().max(3);
            scroll = scroll.min(rows.len().saturating_sub(1));
            if doc.conflicts() == 0 {
                lines.push(Line::from(Span::styled(
                    format!("  ✓ no conflicts left in {}", file.path),
                    Style::default().fg(th.ok),
                )));
                scroll = 0;
            }
            let shown_rows = rows
                .iter()
                .skip(scroll)
                .take(usize::from(text_area.height).saturating_sub(lines.len()));
            for row in shown_rows {
                let y = text_area.y + lines.len() as u16;
                lines.push(file_row(row, view, gutter, text_area, y, &mut buttons, th));
            }
        }
        (Some(file), None) => {
            let (what, current, incoming) = match file.kind {
                Kind::DeletedCurrent => (
                    "deleted on the current side, changed on the incoming one",
                    "keep it deleted",
                    "keep the incoming file",
                ),
                Kind::DeletedIncoming => (
                    "changed on the current side, deleted on the incoming one",
                    "keep the current file",
                    "delete it",
                ),
                Kind::Binary | Kind::Text => (
                    "changed on both sides, and not a file git merges line by line",
                    "keep the current file",
                    "keep the incoming file",
                ),
            };
            let dim = Style::default().fg(th.dim);
            lines.push(Line::from(Span::styled(format!("  {what}"), dim)));
            lines.push(Line::from(""));
            if file.left == 0 {
                lines.push(Line::from(Span::styled(
                    "  ✓ settled",
                    Style::default().fg(th.ok),
                )));
            } else {
                let y = text_area.y + lines.len() as u16;
                let mut x = text_area.x + 2;
                let mut spans = vec![Span::raw("  ")];
                for (choice, does) in [(Choice::Current, current), (Choice::Incoming, incoming)] {
                    let label = format!("[{}] {does}", keys::of(choice).label());
                    let w = label.width() as u16;
                    buttons.push((Rect::new(x, y, w, 1), 0, Button::Take(choice)));
                    spans.push(Span::styled(label, Style::default().fg(th.accent)));
                    spans.push(Span::raw("   "));
                    x += w + 3;
                }
                lines.push(Line::from(spans));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), text_area);
    if let Some(note) = &view.note {
        let color = match note.kind {
            crate::flash::FlashKind::Failed => th.err,
            crate::flash::FlashKind::Setup | crate::flash::FlashKind::Working => th.warn,
            crate::flash::FlashKind::Done => th.ok,
            crate::flash::FlashKind::Note => th.muted,
        };
        let at = Rect {
            y: body.y + body.height.saturating_sub(1),
            height: 1,
            ..body
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("  {}", note.text),
                Style::default().fg(color),
            )),
            at,
        );
    }
    crate::hints::draw_on_border(f, area, &hints(view), 0, th);

    if let Some(Overlay::Conflicts(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = list_inner;
        v.body_area = text_area;
        v.buttons = buttons;
        v.row_rects = row_rects;
        v.view_height = text_area.height;
        v.scroll = scroll;
    }
}

/// One line of the file pane — `y` its screen row — with a conflict's
/// buttons recorded where they land.
fn file_row(
    row: &Row,
    view: &ConflictView,
    gutter: usize,
    area: Rect,
    y: u16,
    buttons: &mut Vec<(Rect, usize, Button)>,
    th: Theme,
) -> Line<'static> {
    let dim = Style::default().fg(th.faint);
    let room = usize::from(area.width).saturating_sub(gutter + 4);
    let side_color = |side: Side| match side {
        Side::Current => th.added,
        Side::Base => th.faint,
        Side::Incoming => th.modified,
    };
    let on = |hunk: usize| hunk == view.hunk;
    match *row {
        Row::Text { n, text } => Line::from(vec![
            Span::styled(format!("{n:>gutter$}   "), dim),
            Span::styled(
                crate::ui::truncate(&shown(text), room),
                Style::default().fg(th.text),
            ),
        ]),
        Row::Line { hunk, side, text } => {
            let color = side_color(side);
            let text_color = if side == Side::Base { th.dim } else { th.text };
            Line::from(vec![
                Span::styled(" ".repeat(gutter), dim),
                Span::styled(
                    if on(hunk) { " ┃ " } else { " │ " },
                    Style::default().fg(color),
                ),
                Span::styled(
                    crate::ui::truncate(&shown(text), room),
                    Style::default().fg(text_color),
                ),
            ])
        }
        Row::Head { hunk } => {
            let rail = Style::default().fg(th.added);
            let mut spans = vec![
                Span::styled(" ".repeat(gutter), dim),
                Span::styled(if on(hunk) { " ┏ " } else { " ┌ " }, rail),
                Span::styled(
                    format!("current · ⎇ {}", view.branch),
                    rail.add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
            ];
            let mut x = area.x + spans.iter().map(|s| s.content.width()).sum::<usize>() as u16;
            // The keys are the footer's to say; the buttons say what.
            let choices = Choice::ALL.map(|c| (Button::Take(c), c.button()));
            for (button, label) in choices.into_iter().chain([(Button::Edit, "edit")]) {
                let w = label.width() as u16;
                if x + w > area.x + area.width {
                    break;
                }
                buttons.push((Rect::new(x, y, w, 1), hunk, button));
                let color = if on(hunk) { th.accent } else { th.dim };
                spans.push(Span::styled(label.to_string(), Style::default().fg(color)));
                spans.push(Span::styled(" · ", dim));
                x += w + 3;
            }
            spans.pop();
            Line::from(spans)
        }
        Row::Rule { hunk, side } => {
            let color = side_color(side);
            let words = match side {
                Side::Base => "base".to_string(),
                _ => format!("incoming · origin/{}", view.base),
            };
            Line::from(vec![
                Span::styled(" ".repeat(gutter), dim),
                Span::styled(
                    if on(hunk) { " ┣ " } else { " ├ " },
                    Style::default().fg(color),
                ),
                Span::styled(
                    words,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ])
        }
        Row::Tail { hunk } => Line::from(vec![
            Span::styled(" ".repeat(gutter), dim),
            Span::styled(
                if on(hunk) { " ┗" } else { " └" },
                Style::default().fg(th.modified),
            ),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
fn main() {
<<<<<<< HEAD
    feature();
=======
    upstream();
    more();
>>>>>>> origin/main
}
";

    /// A conflict comes out as its two sides and its labels; the text
    /// around it is kept, byte for byte.
    #[test]
    fn parse_reads_both_sides_and_keeps_the_rest() {
        let doc = parse(FILE);
        assert_eq!(doc.conflicts(), 1);
        let Part::Conflict(h) = &doc.parts[1] else {
            panic!("{doc:?}");
        };
        assert_eq!(h.current, vec!["    feature();\n"]);
        assert_eq!(h.incoming, vec!["    upstream();\n", "    more();\n"]);
        assert_eq!(h.current_label, "HEAD");
        assert_eq!(h.incoming_label, "origin/main");
        assert_eq!(h.base, None);
        assert_eq!(
            doc.text(),
            FILE,
            "an open conflict writes back as git left it"
        );
    }

    /// Each choice writes its side — both is current then incoming — and
    /// the conflict is gone.
    #[test]
    fn resolving_writes_the_side_taken() {
        let take = |choice| {
            let mut doc = parse(FILE);
            assert!(doc.resolve(0, choice));
            assert_eq!(doc.conflicts(), 0);
            doc.text()
        };
        assert_eq!(take(Choice::Current), "fn main() {\n    feature();\n}\n");
        assert_eq!(
            take(Choice::Incoming),
            "fn main() {\n    upstream();\n    more();\n}\n"
        );
        assert_eq!(
            take(Choice::Both),
            "fn main() {\n    feature();\n    upstream();\n    more();\n}\n"
        );
        assert!(!parse(FILE).resolve(1, Choice::Current), "no second one");
    }

    /// diff3's base section is read apart from either side, and CRLF
    /// endings are markers all the same.
    #[test]
    fn parse_reads_a_diff3_base_and_crlf_markers() {
        let text =
            "a\r\n<<<<<<< ours\r\nx\r\n||||||| base\r\nb\r\n=======\r\ny\r\n>>>>>>> theirs\r\n";
        let doc = parse(text);
        let Part::Conflict(h) = &doc.parts[1] else {
            panic!("{doc:?}");
        };
        assert_eq!(h.base, Some(vec!["b\r\n".to_string()]));
        assert_eq!(h.current, vec!["x\r\n"]);
        assert_eq!(h.incoming, vec!["y\r\n"]);
        assert_eq!(doc.text(), text);
    }

    /// A conflict the file never closes, and markers of another size, are
    /// plain text: nothing to resolve.
    #[test]
    fn unclosed_and_wrong_sized_markers_are_text() {
        let unclosed = "<<<<<<< HEAD\nx\n=======\ny\n";
        assert_eq!(parse(unclosed).conflicts(), 0);
        assert_eq!(parse(unclosed).text(), unclosed);
        let rst = "Title\n=======\n<<<<<<<< eight\n";
        assert_eq!(parse(rst).conflicts(), 0);
    }

    /// Two conflicts are settled one at a time, the `n`th counted among
    /// the ones still open.
    #[test]
    fn the_nth_conflict_counts_only_open_ones() {
        let two = format!("{FILE}{FILE}");
        let mut doc = parse(&two);
        assert_eq!(doc.conflicts(), 2);
        assert!(doc.resolve(1, Choice::Incoming));
        assert!(doc.resolve(0, Choice::Current));
        assert_eq!(
            doc.text(),
            "fn main() {\n    feature();\n}\nfn main() {\n    upstream();\n    more();\n}\n"
        );
    }

    #[test]
    fn merge_tree_names_follow_the_tree_id() {
        let out = b"4b825dc\0a.txt\0dir/b.rs\0\0";
        assert_eq!(merge_tree_files(out), vec!["a.txt", "dir/b.rs"]);
        assert!(merge_tree_files(b"4b825dc\0").is_empty());
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
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

    /// A committer for `repo`'s commits, unsigned.
    fn identify(repo: &Path) {
        for (k, v) in [
            ("user.email", "t@t"),
            ("user.name", "t"),
            ("commit.gpgsign", "false"),
        ] {
            git(repo, &["config", k, v]);
        }
    }

    /// `origin` with `main` and `feature` editing the same line of a.txt,
    /// feature deleting gone.txt that main edits, and a clone on
    /// `feature`: the checkout a pull request's conflicts are settled in.
    fn conflicted_clone(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        let origin = dir.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        identify(&origin);
        std::fs::write(origin.join("a.txt"), "one\nshared\nthree\n").unwrap();
        std::fs::write(origin.join("gone.txt"), "old\n").unwrap();
        std::fs::write(origin.join("same.txt"), "same\n").unwrap();
        git(&origin, &["add", "."]);
        git(&origin, &["commit", "-q", "-m", "init"]);
        git(&origin, &["switch", "-q", "-c", "feature"]);
        std::fs::write(origin.join("a.txt"), "one\nfeature\nthree\n").unwrap();
        git(&origin, &["rm", "-q", "gone.txt"]);
        git(&origin, &["commit", "-q", "-am", "feature"]);
        git(&origin, &["switch", "-q", "main"]);
        std::fs::write(origin.join("a.txt"), "one\nupstream\nthree\n").unwrap();
        std::fs::write(origin.join("gone.txt"), "edited\n").unwrap();
        git(&origin, &["commit", "-q", "-am", "upstream"]);
        let clone = dir.path().join("clone");
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                "-b",
                "feature",
                origin.to_str().unwrap(),
                "clone",
            ],
        );
        identify(&clone);
        (origin, clone)
    }

    /// merge-tree names exactly the files that conflict, a checkout left
    /// as it was.
    #[test]
    fn the_repo_names_the_conflicted_files() {
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let head = git(&clone, &["rev-parse", "HEAD"]);
        let files = conflicted(&clone, 7, "main", &head, &AtomicBool::new(false)).unwrap();
        assert_eq!(files, vec!["a.txt", "gone.txt"]);
        assert!(!merging(&clone), "nothing merged into the checkout");
    }

    /// The merge stops on both files, each read for what it needs; taking
    /// sides and committing leaves a merge commit with no markers in it.
    #[test]
    fn a_merge_is_prepared_settled_and_committed() {
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let head = git(&clone, &["rev-parse", "HEAD"]);
        let quit = AtomicBool::new(false);
        let Prepared::Conflicts(files) = prepare(&clone, "feature", "main", &head, &quit).unwrap()
        else {
            panic!("expected conflicts");
        };
        let kinds: Vec<(&str, Kind, usize)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.kind, f.left))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("a.txt", Kind::Text, 1),
                ("gone.txt", Kind::DeletedCurrent, 1)
            ]
        );
        // Opened again mid-merge: the same files, nothing merged twice.
        assert!(matches!(
            prepare(&clone, "feature", "main", &head, &quit),
            Ok(Prepared::Conflicts(again)) if again == files
        ));

        let mut view = ConflictView::new(clone.clone(), "feature".into(), "main".into(), files);
        view.open_at(None);
        crate::hints::assert_hints_from(&hints(&view), keys::ALL);
        take(&mut view, Choice::Both);
        let a = std::fs::read_to_string(clone.join("a.txt")).unwrap();
        assert_eq!(a, "one\nfeature\nupstream\nthree\n");
        assert_eq!(view.selected, 1, "on to the next file with a conflict");
        undo(&mut view);
        assert_eq!(view.selected, 0, "undo goes back to the file");
        assert_eq!(view.files[0].left, 1);
        take(&mut view, Choice::Incoming);
        assert_eq!(view.selected, 1);
        take(&mut view, Choice::Both);
        assert!(view
            .note
            .as_ref()
            .is_some_and(|n| n.kind == crate::flash::FlashKind::Failed));
        take(&mut view, Choice::Current);
        view.reload();
        assert_eq!(view.left(), 0, "a side taken stays taken: {:?}", view.note);
        assert!(!clone.join("gone.txt").exists(), "kept deleted");
        crate::hints::assert_hints_from(&hints(&view), keys::ALL);
        assert!(hints(&view).iter().any(|h| h.does == "commit & push"));

        commit_merge(&clone, &["a.txt".to_string()]).unwrap();
        assert!(!merging(&clone));
        assert_eq!(
            git(&clone, &["rev-list", "--count", "--merges", "HEAD"]),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(clone.join("a.txt")).unwrap(),
            "one\nupstream\nthree\n"
        );
    }

    /// A checkout with changes of its own is never merged into.
    #[test]
    fn a_dirty_checkout_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        std::fs::write(clone.join("same.txt"), "mine\n").unwrap();
        let why = prepare(&clone, "feature", "main", "", &AtomicBool::new(false)).unwrap_err();
        assert!(why.contains("uncommitted changes"), "{why}");
        assert!(!merging(&clone));
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let Some(Overlay::Conflicts(v)) = app.overlay.clone() else {
                panic!("no resolver");
            };
            draw(f, app, &v, app.theme);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// GitHub says #7 conflicts: the repo names its files, the page marks
    /// them, Enter on one merges the base into its checkout and opens the
    /// RESOLVER there — `⌘E` still diffs it — and Esc leaves the merge
    /// waiting.
    #[test]
    fn enter_on_a_conflicted_file_opens_the_resolver_in_its_checkout() {
        use crate::pr_preview::RowAct;
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let mut app = App::new();
        let project = orion_core::ProjectId("p".into());
        app.tree.projects.push(orion_core::Project {
            id: project.clone(),
            name: "demo".into(),
            repo_path: clone.clone(),
            sort_order: 0,
        });
        let checkout = orion_core::Worktree {
            id: WorktreeId("w".into()),
            project_id: project,
            path: clone.clone(),
            branch: "feature".into(),
            is_main: false,
            sort_order: 0,
        };
        let url = "https://github.com/o/r/pull/7".to_string();
        let detail = crate::pull_request::PrDetail {
            number: 7,
            url: url.clone(),
            title: "t".into(),
            base: "main".into(),
            head: "feature".into(),
            head_sha: git(&clone, &["rev-parse", "HEAD"]),
            answered: crate::pull_request::Answered {
                conflicts: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        app.prs.observe(
            &url,
            crate::pr_store::PrObservation::of_detail(&detail),
            crate::fetch::Asked::Cached,
        );
        app.pr_detail.insert(url.clone(), detail);

        want(&mut app, &url, &clone);
        let files = of_page(&app, &url).map(<[String]>::to_vec);
        assert_eq!(files, Some(vec!["a.txt".into(), "gone.txt".into()]));
        assert!(!merging(&clone), "naming them merges nothing");

        let mut out = Vec::new();
        let act = |path: &str| RowAct::File(path.into());
        crate::pr_preview::run_act(&mut app, 7, &url, "#7 t", act("a.txt"), false, &mut out);
        assert!(
            !matches!(app.overlay, Some(Overlay::Conflicts(_))),
            "⌘E diffs it"
        );
        assert!(!merging(&clone));
        app.overlay = None;

        // Nobody has it checked out: Enter asks the DAEMON for the PR
        // SESSION's checkout, and the merge follows once it is there.
        out.clear();
        crate::pr_preview::run_act(&mut app, 7, &url, "#7 t", act("gone.txt"), true, &mut out);
        let [orion_core::ClientRequest::CreatePrWorktree { req_id, head, .. }] = out.as_slice()
        else {
            panic!("no checkout asked for: {out:?}");
        };
        assert_eq!(head, "feature");
        assert!(!merging(&clone), "nothing merged before the checkout");
        let req_id = *req_id;
        crate::event_loop::handle_server_event(
            &mut app,
            orion_core::ServerEvent::EntityUpserted {
                entity: orion_core::Entity::Worktree(checkout),
            },
            &mut out,
        );
        crate::event_loop::handle_server_event(
            &mut app,
            orion_core::ServerEvent::Ack {
                req_id,
                created: Some(orion_core::EntityId::Worktree(WorktreeId("w".into()))),
            },
            &mut out,
        );
        let Some(Overlay::Conflicts(view)) = &app.overlay else {
            panic!("no resolver: {:?}", app.flash);
        };
        assert!(merging(&clone));
        assert_eq!(view.file().map(|f| f.path.as_str()), Some("gone.txt"));
        handle_key(&mut app, KeyEvent::from(crossterm::event::KeyCode::BackTab));
        let shot = screen(&mut app, 140, 30);
        for words in [
            "Conflicts — #7 t · 2 left",
            "origin/main into ⎇ feature",
            "current · ⎇ feature",
            "accept current · accept incoming · accept both · edit",
            "incoming · origin/main",
            "upstream",
        ] {
            assert!(shot.contains(words), "{words:?} in\n{shot}");
        }

        handle_key(&mut app, KeyEvent::from(crossterm::event::KeyCode::Esc));
        assert!(app.overlay.is_none());
        assert!(merging(&clone), "Esc leaves the merge as it was");
        assert!(app
            .flash
            .as_deref()
            .is_some_and(|f| f.contains("mid-merge")));
        crate::pr_preview::run_act(&mut app, 7, &url, "#7 t", act("a.txt"), true, &mut out);
        let Some(Overlay::Conflicts(view)) = &app.overlay else {
            panic!("no resolver the second time: {:?}", app.flash);
        };
        assert_eq!(view.file().map(|f| f.path.as_str()), Some("a.txt"));
        let w = crossterm::event::KeyModifiers::SUPER;
        let abort = KeyEvent::new(crossterm::event::KeyCode::Char('w'), w);
        handle_key(&mut app, abort);
        assert!(merging(&clone), "one press only arms it");
        handle_key(&mut app, abort);
        assert!(!merging(&clone), "{:?}", app.flash);
        assert!(app.overlay.is_none());
    }

    /// The pane's rows: numbered text, then each conflict's head, sides
    /// and tail — the line numbers going on past it as the file's do.
    #[test]
    fn rows_number_the_file_around_each_conflict() {
        let doc = parse(FILE);
        let rows = rows(&doc);
        assert_eq!(
            rows[0],
            Row::Text {
                n: 1,
                text: "fn main() {\n"
            }
        );
        assert_eq!(rows[1], Row::Head { hunk: 0 });
        assert!(matches!(
            rows[2],
            Row::Line {
                side: Side::Current,
                ..
            }
        ));
        assert_eq!(
            rows[3],
            Row::Rule {
                hunk: 0,
                side: Side::Incoming
            }
        );
        assert_eq!(rows[6], Row::Tail { hunk: 0 });
        assert_eq!(rows[7], Row::Text { n: 8, text: "}\n" });
    }
}
