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
//! The DIFF VIEWER needs no word from GitHub: it asks the same merge of its
//! branch and the base this repo already has, with no fetch ([`local`]),
//! the answer kept by the two commits — before a pull request exists, and
//! between the pages' asks. Its Enter on a conflicted file opens the
//! RESOLVER on the checkout it reads ([`resolve_in`]), and Esc comes back
//! to the viewer.
//!
//! **Settling them.** Enter on a conflicted file opens the RESOLVER at once,
//! and merges the base into the pull request's branch in the project's
//! checkout of it — what GitHub's own *Resolve conflicts* does. The modal is
//! three sections. Top left, where the merge stands: its steps (merge,
//! resolve, commit, push) and what to do next. Under that, the files git left
//! conflicted. On the right, the file while it has conflicts to settle,
//! each drawn as its two sides, the current change (the pull request's
//! branch) over the incoming one (the base), under a row of buttons; and the
//! LOG otherwise, what git and its hooks said as they said it. `l` flips
//! between the two, and Tab or a click reads a file back.
//!
//! `c`, `i` and `b` take the current side, the incoming side or both, `u`
//! puts the last one back, and Enter opens the file in the BUILT-IN EDITOR
//! at the conflict for anything the buttons can't say. A file is written
//! the moment a side is taken; nothing is staged until the merge is
//! committed, so Esc can leave it half done and Enter on a conflicted file
//! comes back to it. `⌘W` twice aborts it (`git merge --abort`), the checkout
//! back as it was.
//!
//! **Sending it.** `⌘S` stages the files and commits the merge, then pushes it
//! (`git_sync`'s PUSH), the modal up and the LOG running throughout. A
//! commit a hook stopped or a push that failed stays on screen with the
//! reason, and `⌘S` tries that step again; a push never commits twice. Once
//! the push lands the modal gives way to the PULL REQUESTS MODAL, on the pull
//! request's Checks tab with its page asked for fresh. Esc while git is at
//! work leaves it working: the view is PARKED on [`Known`], the FLASH says how
//! it ends, and Enter on a conflicted file comes back to it.
//!
//! A checkout with uncommitted changes, or one an agent is working in, is
//! never merged into, and one behind the pull request's head would merge
//! into old commits: the modal says why it did not start.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use orion_core::WorktreeId;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Overlay};
use crate::flash::Flash;
use crate::git_proc::{logged_git, read, remote_git, run, GitLog, LogKind, FETCH_TIMEOUT};
use crate::theme::Theme;

/// Lines one wheel notch scrolls the pane.
const WHEEL_LINES: i32 = 3;
/// Lines kept above a conflict the cursor moves to, so its context shows.
const CONTEXT: usize = 4;
/// The file list's share of the modal, and its floor.
const LIST_PCT: u16 = 28;
const MIN_LIST_W: u16 = 22;
/// A tab as the file pane draws it.
const TAB: &str = "    ";
/// The state panel's rows, borders in: where the merge goes, its four steps,
/// and two for what to do next.
const STATE_H: u16 = 9;
/// How long the merge, then the commit with its hooks, may run. A pre-commit
/// hook can run a test suite.
const MERGE_TIMEOUT: Duration = Duration::from_secs(300);
const COMMIT_TIMEOUT: Duration = Duration::from_secs(900);
/// The file list's least room under the state panel: its borders and a row.
const MIN_FILES_H: u16 = 3;

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
/// for its head commit and base — and the RESOLVER left at work, if one
/// was (`App::conflicts`).
#[derive(Debug, Default)]
pub struct Known {
    by_url: HashMap<String, Entry>,
    /// A RESOLVER Esc left with git at work, or with a result to read:
    /// answers land on it, and Enter on a conflicted file brings it back.
    parked: Option<Box<ConflictView>>,
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
    let fetched = fetch_base(dir, base, quit);
    let base_ref = format!("origin/{base}");
    if !has_commit(dir, &base_ref) {
        return Err(fetched
            .err()
            .unwrap_or_else(|| format!("no {base_ref} here")));
    }
    fetch_pull_head(dir, number, head_sha, quit)?;
    merge_tree(dir, &base_ref, head_sha)
}

/// `origin/<base>` fetched into `dir`'s repo: a pull request's base, for
/// the conflict check and the DIFF VIEWER (`event_loop::open_pr_review`).
pub(crate) fn fetch_base(dir: &Path, base: &str, quit: &AtomicBool) -> Result<(), String> {
    remote_git(
        dir,
        &["fetch", "--quiet", "--no-tags", "origin", base],
        FETCH_TIMEOUT,
        quit,
    )
}

/// Pull request `number`'s head commit `head_sha` into `dir`'s repo when it
/// is not there yet: fetched by its pull ref, which GitHub keeps for every
/// pull request — a fork's too. The conflict check and the DIFF VIEWER on a
/// pull request (`event_loop::open_pr_review`) both read it from here.
pub(crate) fn fetch_pull_head(
    dir: &Path,
    number: u64,
    head_sha: &str,
    quit: &AtomicBool,
) -> Result<(), String> {
    if has_commit(dir, head_sha) {
        return Ok(());
    }
    let pull = format!("refs/pull/{number}/head");
    remote_git(
        dir,
        &["fetch", "--quiet", "--no-tags", "origin", &pull],
        FETCH_TIMEOUT,
        quit,
    )
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

// ---- a branch's conflicts, worked out locally ----

/// What a branch conflicts in with its base, as [`local`] worked it out:
/// the DIFF VIEWER's `U` marks and its top line (`diff_actions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    /// The base as named — `origin/dev`.
    pub base: String,
    /// The files that conflict; empty when the two merge cleanly.
    pub files: Vec<String>,
}

/// How many head-and-base pairs [`local`] keeps the answer for before it
/// starts over: a handful of checkouts, each read at a few commits.
const LOCAL_KEPT: usize = 64;

/// [`local`]'s answers: the conflicted files, by head commit and base
/// commit.
type Merged = std::sync::Mutex<HashMap<(String, String), Vec<String>>>;

fn merged() -> &'static Merged {
    static MERGED: std::sync::OnceLock<Merged> = std::sync::OnceLock::new();
    MERGED.get_or_init(Default::default)
}

/// The files `head` — `HEAD`, or a pull request's tip — conflicts in with
/// the base `base_setting` names, resolved as the COMMIT LIST resolves it
/// (`commit_list::resolve_base_cached`): `git merge-tree` on the two
/// commits, touching no checkout and fetching nothing — base sync keeps
/// origin's copy fresh, so it works before a pull request exists. Kept by
/// the two commits' SHAs: asked again with nothing moved, it costs two
/// `rev-parse`s. Off the loop; it can take a second on a big repo.
pub(crate) fn local(root: &Path, base_setting: &str, head: &str) -> Result<Local, String> {
    let base = crate::commit_list::resolve_base_cached(root, base_setting)
        .ok_or_else(|| "no base branch to compare with".to_string())?;
    let commit = |rev: &str| {
        read(
            root,
            &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")],
        )
        .map(|sha| sha.trim().to_string())
    };
    let key = (commit(head)?, commit(&base)?);
    let kept = merged().lock().ok().and_then(|m| m.get(&key).cloned());
    let files = match kept {
        Some(files) => files,
        None => {
            let files = merge_tree(root, &key.1, &key.0)?;
            if let Ok(mut m) = merged().lock() {
                if m.len() >= LOCAL_KEPT {
                    m.clear();
                }
                m.insert(key, files.clone());
            }
            files
        }
    };
    Ok(Local { base, files })
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
    /// What a DIFF VIEWER's branch conflicts in ([`local`],
    /// `diff_actions::ask`).
    Local {
        ticket: u64,
        result: Result<Local, String>,
    },
    /// The files a pull request's head conflicts in with its base, for
    /// AUTOFIX's prompt ([`local`], `autofix::want_conflicts`).
    Autofix {
        url: String,
        head_sha: String,
        base: String,
        files: Result<Vec<String>, String>,
    },
}

/// Run `work` on the blocking pool and land its answer — inline where
/// there is no pool (the unit tests).
pub(crate) fn off_loop(app: &mut App, work: impl FnOnce() -> Answer + Send + 'static) {
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
        Answer::Local { ticket, result } => crate::diff_actions::land(app, ticket, result),
        Answer::Autofix {
            url,
            head_sha,
            base,
            files,
        } => crate::autofix::land_conflicts(app, &url, &head_sha, &base, files),
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
    log: &GitLog,
) -> Result<Prepared, String> {
    if merging(root) {
        log.say(LogKind::Note, "Picking up the merge already underway.");
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
    logged_git(
        root,
        &["fetch", "--no-tags", "origin", base],
        FETCH_TIMEOUT,
        quit,
        log,
    )?;
    let base_ref = format!("origin/{base}");
    let merge = ["merge", "--no-edit", &base_ref];
    match logged_git(root, &merge, MERGE_TIMEOUT, quit, log) {
        Ok(_) => Ok(Prepared::Clean),
        Err(why) => match unmerged(root) {
            // Even with nothing unmerged: a hook stopped it, and it's underway.
            Ok(files) if merging(root) => Ok(Prepared::Conflicts(files)),
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

/// Enter on a conflicted file of pull request `url`'s page: the RESOLVER up
/// at once, merging its base into its branch in the project's checkout of
/// it off the loop ([`land_prepared`]). A pull request nobody has checked out
/// is checked out first — the checkout a PR SESSION would run in
/// (`pr_checkout`) — and the merge follows. One Esc left at work comes back
/// as it was.
pub(crate) fn open(
    app: &mut App,
    url: &str,
    label: &str,
    path: Option<&str>,
    out: &mut Vec<orion_core::ClientRequest>,
) {
    if taken(app, |parked| parked.url == url) {
        return;
    }
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
    let pr = Pr {
        url,
        label,
        head_sha: &detail.head_sha,
    };
    start(app, worktree, detail.base, pr, path);
}

/// Enter on a conflicted file of a checkout's own DIFF VIEWER
/// (`diff_actions`): the RESOLVER on that checkout, merging origin's
/// `base` into it, as [`open`] does for a pull request's — `pr` naming the
/// branch's pull request where it has one, an empty URL where it has none.
/// A resolver PARKED on the same checkout comes back as it was.
pub(crate) fn resolve_in(
    app: &mut App,
    worktree: orion_core::Worktree,
    base: String,
    pr: Pr,
    path: Option<&str>,
) {
    if !taken(app, |parked| parked.worktree == worktree.id) {
        start(app, worktree, base, pr, path);
    }
}

/// The pull request a merge is for, as the RESOLVER names it: its URL
/// (empty for a branch with none), its `#42 title` label, and the head
/// commit a checkout behind it must pull first (empty: not checked).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pr<'a> {
    pub url: &'a str,
    pub label: &'a str,
    pub head_sha: &'a str,
}

/// One resolver at a time: true, with nothing new opened, while one is on
/// screen or one PARKED has git at work — or when the PARKED one is the
/// one asked for (`same`), which comes back as it was.
fn taken(app: &mut App, same: impl Fn(&ConflictView) -> bool) -> bool {
    // A checkout landing under one on screen doesn't start another.
    if let Some(Overlay::Conflicts(view)) = &app.overlay {
        app.flash = Some(Flash::note(format!(
            "⎇ {} is merging — finish that first",
            view.branch
        )));
        return true;
    }
    if let Some(parked) = &app.conflicts.parked {
        if same(parked) {
            unpark(app);
            return true;
        }
        if parked.running() {
            app.flash = Some(Flash::note(format!(
                "⎇ {} has a merge at work — try again once it lands",
                parked.branch
            )));
            return true;
        }
        // Stopped, with nothing running: what git left is still there, for
        // Enter on its file (mid-merge) or ⇧P (committed).
        app.conflicts.parked = None;
    }
    false
}

/// The RESOLVER up on `worktree`, merging `origin/<base>` into it off the
/// loop — or saying why not, while an agent works there or a sync runs.
fn start(app: &mut App, worktree: orion_core::Worktree, base: String, pr: Pr, path: Option<&str>) {
    let working = app
        .tree
        .agents
        .iter()
        .any(|a| a.worktree_id == worktree.id && !a.archived && app.spins(a));
    let refused = if working {
        Some(format!(
            "an agent is working in ⎇ {} — resolve its conflicts once it stops",
            worktree.branch
        ))
    } else if app.git_sync.inflight.contains_key(&worktree.id) {
        Some(format!(
            "⎇ {} is syncing — try again once it lands",
            worktree.branch
        ))
    } else {
        None
    };
    let mut view = ConflictView::new(
        worktree.path.clone(),
        worktree.branch.clone(),
        base.clone(),
        Vec::new(),
    );
    view.url = pr.url.to_string();
    view.label = pr.label.to_string();
    view.worktree = worktree.id.clone();
    view.want_path = path.map(str::to_string);
    view.stage = Stage::Merging;
    view.ticket = crate::view_jobs::ticket();
    let (ticket, log) = (view.ticket, view.log.clone());
    if let Some(why) = refused {
        log.say(LogKind::Bad, why.clone());
        view.stage = Stage::Refused;
        if raise(app, Box::new(view)).is_err() {
            app.flash = Some(Flash::failed(why));
        }
        return;
    }
    if let Err(view) = raise(app, Box::new(view)) {
        app.flash = Some(Flash::working(format!(
            "merging origin/{} into ⎇ {}…",
            view.base, view.branch
        )));
        app.conflicts.parked = Some(view);
    }
    let (root, branch, head_sha) = (worktree.path, worktree.branch, pr.head_sha.to_string());
    let quit = Arc::clone(&app.git_sync.quit);
    let work = move || Answer::Prepared {
        ticket,
        result: prepare(&root, &branch, &base, &head_sha, &quit, &log),
    };
    off_loop(app, work);
}

/// What the RESOLVER was opened over, and Esc puts back.
#[derive(Debug, Clone)]
pub enum Under {
    /// The PULL REQUESTS MODAL: the resolver is a level inside it.
    PullRequests(Box<crate::pr_modal::PullRequestsView>),
    /// A DIFF VIEWER, a checkout's or a pull request's (`diff_actions`).
    Diff(Box<crate::app::DiffView>),
}

/// `view` on screen — over the PULL REQUESTS MODAL or the DIFF VIEWER it
/// was asked from, as a level inside it, or over the panels. Something
/// else opened meanwhile is left alone, and `view` handed back.
fn raise(app: &mut App, mut view: Box<ConflictView>) -> Result<(), Box<ConflictView>> {
    match app.overlay.take() {
        Some(Overlay::PullRequests(under)) => {
            view.back = Some(Under::PullRequests(Box::new(under)));
        }
        Some(Overlay::Diff(under)) => view.back = Some(Under::Diff(Box::new(under))),
        None => {}
        other => {
            app.overlay = other;
            return Err(view);
        }
    }
    app.flash = None;
    app.overlay = Some(Overlay::Conflicts(view));
    app.dirty = true;
    Ok(())
}

/// The PARKED resolver back on screen, reading its files again: the editor,
/// or git, may have moved them on.
fn unpark(app: &mut App) {
    let Some(mut view) = app.conflicts.parked.take() else {
        return;
    };
    if !view.running() {
        view.reload();
    }
    if let Err(view) = raise(app, view) {
        app.conflicts.parked = Some(view);
    }
}

/// The resolver waiting on `ticket`, on screen or PARKED, and which.
fn waiting_on(app: &mut App, ticket: u64) -> Option<(&mut ConflictView, bool)> {
    if let Some(Overlay::Conflicts(view)) = &mut app.overlay {
        if view.ticket == ticket {
            return Some((view, false));
        }
    }
    app.conflicts
        .parked
        .as_mut()
        .filter(|view| view.ticket == ticket)
        .map(|view| (&mut **view, true))
}

/// The merge landed on the resolver waiting for it. On screen it stays up,
/// its log saying how it went. Left PARKED, the FLASH says it; there are
/// conflicts to settle or a refusal to read, and either way Enter on a
/// conflicted file starts over from what the checkout holds.
fn land_prepared(app: &mut App, ticket: u64, result: Result<Prepared, String>) {
    let Some((view, parked)) = waiting_on(app, ticket) else {
        return;
    };
    let branch = view.branch.clone();
    let base = view.base.clone();
    let flash = match result {
        Err(why) => {
            view.log.say(LogKind::Bad, why.clone());
            view.stage = Stage::Refused;
            Flash::failed(why)
        }
        Ok(Prepared::Clean) => {
            view.log.say(
                LogKind::Good,
                format!("Merged origin/{base} with nothing to resolve. Nothing is pushed yet."),
            );
            view.stage = Stage::ReadyToPush;
            Flash::done(format!(
                "merged origin/{base} into ⎇ {branch} with nothing left to resolve — push it to update the pull request"
            ))
        }
        Ok(Prepared::Conflicts(files)) => {
            let stopped = match files.len() {
                0 => "The merge stopped.".to_string(),
                n => format!("The merge stopped on {}.", crate::flash::plural(n, "file")),
            };
            view.log.say(
                LogKind::Note,
                format!("{stopped} Nothing is committed yet."),
            );
            view.files = files;
            view.stage = Stage::Resolving;
            let want = view.want_path.take();
            view.open_at(want.as_deref());
            Flash::note(format!(
                "⎇ {branch} is mid-merge — Enter on a conflicted file opens it"
            ))
        }
    };
    if parked {
        if view.stage != Stage::ReadyToPush {
            app.conflicts.parked = None;
        }
        app.flash = Some(flash);
    }
}

/// The merge for `url` is on GitHub, so its page stops marking the files
/// GitHub's stale answer still calls conflicted.
fn settled(app: &mut App, url: &str) {
    if let Some(entry) = app.conflicts.by_url.get_mut(url) {
        entry.asked = Asked::Files(Vec::new());
    }
}

// ---- the RESOLVER ----

/// How far the merge has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Fetching the base and merging it in.
    Merging,
    /// The merge never started: the log's last line says why.
    Refused,
    /// Stopped on conflicts — [`ConflictView::left`] says how many are
    /// still open; with none it waits for `⌘S`.
    Resolving,
    /// Staging and committing, hooks and all.
    Committing,
    /// The commit stopped — a hook, usually. The merge is still underway.
    CommitFailed,
    /// Committed with nothing to resolve; waiting for `⌘S` to push it.
    ReadyToPush,
    /// Pushing what was committed.
    Pushing,
    /// The push didn't land. The merge commit is made and kept.
    PushFailed,
}

/// What the right-hand pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// What the stage calls for: the file while a conflict is open, else
    /// the log.
    Auto,
    File,
    Log,
}

/// The CONFLICT RESOLVER: where the merge stands, its conflicted files, and
/// either the one under the cursor with its conflicts drawn to settle or
/// the log of what git said.
#[derive(Debug, Clone)]
pub struct ConflictView {
    pub stage: Stage,
    /// What the right-hand pane was asked to show.
    pub pane: Pane,
    /// What git and its hooks said, written off the loop as it comes.
    pub log: GitLog,
    /// When the stage at work started, for its clock.
    pub started: Instant,
    /// The answer this view is waiting for.
    pub ticket: u64,
    /// The file Enter was pressed on, until the merge's files are known.
    pub want_path: Option<String>,
    /// The log's top row; None follows its tail.
    pub log_top: Option<usize>,
    /// The log's top row as last drawn, whichever that was.
    pub log_at: usize,
    /// The log said every conflict is settled, and none has opened since.
    pub said_settled: bool,
    /// The PULL REQUESTS MODAL or DIFF VIEWER it was opened from: Esc
    /// puts it back.
    pub back: Option<Under>,
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
            stage: Stage::Resolving,
            pane: Pane::Auto,
            log: GitLog::default(),
            started: Instant::now(),
            ticket: 0,
            want_path: None,
            log_top: None,
            log_at: 0,
            said_settled: false,
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

    /// Whether git is at work, so there is a clock and a log to watch.
    pub(crate) fn running(&self) -> bool {
        matches!(
            self.stage,
            Stage::Merging | Stage::Committing | Stage::Pushing
        )
    }

    /// Whether leaving should keep this view: git at work, or a result
    /// that a fresh look at the checkout couldn't read back.
    fn parks(&self) -> bool {
        !matches!(self.stage, Stage::Resolving | Stage::Refused)
    }

    /// Whether a conflict still waits for a side to be taken.
    fn conflicts_open(&self) -> bool {
        self.stage == Stage::Resolving && self.left() > 0
    }

    /// On to `stage`, its log in the pane, following the tail.
    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.show_log();
    }

    /// The pane back to what the stage calls for, the log at its tail.
    fn show_log(&mut self) {
        self.pane = Pane::Auto;
        self.log_top = None;
    }

    /// Whether the merge can still be aborted: it hasn't been committed.
    fn abortable(&self) -> bool {
        matches!(self.stage, Stage::Resolving | Stage::CommitFailed)
    }

    /// What the right-hand pane is showing.
    fn showing(&self) -> Pane {
        match self.pane {
            Pane::Auto if self.conflicts_open() => Pane::File,
            Pane::Auto => Pane::Log,
            pane => pane,
        }
    }

    /// Tab or a click: read file `i`.
    fn pick(&mut self, i: usize) {
        self.select(i);
        self.note = None;
        self.pane = if self.conflicts_open() {
            Pane::Auto
        } else {
            Pane::File
        };
    }

    /// `l`: the log, or back to what the stage calls for.
    fn toggle_log(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.pane = match self.showing() {
            Pane::Log if self.conflicts_open() => Pane::Auto,
            Pane::Log => Pane::File,
            _ => Pane::Log,
        };
    }

    /// How long the stage at work has been at it.
    fn clock(&self) -> String {
        crate::pull_request::duration_label(self.started.elapsed().as_secs() as i64)
    }

    /// The last conflict just went: the log says so, and takes the pane.
    fn note_settled(&mut self) {
        if self.stage != Stage::Resolving || self.files.is_empty() || self.left() > 0 {
            self.said_settled = false;
            return;
        }
        if !self.said_settled {
            self.said_settled = true;
            self.log.say(
                LogKind::Good,
                "Every conflict is settled. Nothing is committed yet.",
            );
            self.show_log();
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
        self.note_settled();
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
    if view.stage != Stage::Resolving || view.showing() != Pane::File {
        return;
    }
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
            view.note_settled();
        }
        Err(why) => view.note = Some(Flash::failed(why)),
    }
}

/// `u`: the file the last side was taken in, as it was before.
fn undo(view: &mut ConflictView) {
    if view.stage != Stage::Resolving {
        return;
    }
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
/// resolver reads it again when the editor closes (`event_loop::close_editor`).
fn edit(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &app.overlay else {
        return;
    };
    if !matches!(view.stage, Stage::Resolving | Stage::CommitFailed) {
        return;
    }
    let Some(file) = view.file().filter(|f| f.kind != Kind::DeletedCurrent) else {
        return;
    };
    let (root, path, line) = (view.root.clone(), file.path.clone(), view.file_line());
    let editor = crate::config::Config::load().editor_command();
    let size = crate::event_loop::editor_size_guess(app);
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

/// `⌘S`: the step that is next. Every file staged and the merge committed,
/// off the loop, its push following once it lands ([`land_committed`]); or,
/// committed already, the push alone. Refused while a conflict is left.
fn commit(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &mut app.overlay else {
        return;
    };
    view.abort_armed = false;
    match view.stage {
        Stage::ReadyToPush | Stage::PushFailed => {
            let ticket = view.ticket;
            return start_push(app, ticket);
        }
        Stage::Resolving | Stage::CommitFailed => {}
        Stage::Merging | Stage::Refused | Stage::Committing | Stage::Pushing => return,
    }
    let left = view.left();
    if left > 0 {
        let files = view.files.iter().filter(|f| f.left > 0).count();
        view.note = Some(Flash::failed(format!(
            "{} left in {} — settle {} before committing",
            crate::flash::plural(left, "conflict"),
            crate::flash::plural(files, "file"),
            if left == 1 { "it" } else { "them" }
        )));
        return;
    }
    let ticket = crate::view_jobs::ticket();
    view.ticket = ticket;
    view.enter(Stage::Committing);
    view.started = Instant::now();
    view.note = None;
    // Undo ends here, and a big file's copies needn't be carried along.
    view.undo.clear();
    let root = view.root.clone();
    let log = view.log.clone();
    let paths: Vec<String> = view
        .files
        .iter()
        .filter(|f| f.kind == Kind::Text)
        .map(|f| f.path.clone())
        .collect();
    let quit = Arc::clone(&app.git_sync.quit);
    let work = move || Answer::Committed {
        ticket,
        result: commit_merge(&root, &paths, &quit, &log),
    };
    off_loop(app, work);
}

/// `git add` the settled files, then `git commit --no-edit`: the merge's
/// own message. Hooks run, detached (`git_proc`), and what they say goes in
/// `log`.
fn commit_merge(
    root: &Path,
    paths: &[String],
    quit: &AtomicBool,
    log: &GitLog,
) -> Result<(), String> {
    if !paths.is_empty() {
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        log.say(LogKind::Command, format!("$ git {}", args.join(" ")));
        run(root, &args)?;
    }
    if !read(root, &["ls-files", "-u"])?.trim().is_empty() {
        return Err("git still has files unmerged — settle them first".into());
    }
    logged_git(root, &["commit", "--no-edit"], COMMIT_TIMEOUT, quit, log)
}

/// The merge committed: on to the push. A refusal — a hook that failed —
/// stays on the resolver with git's reason, and `⌘S` tries again.
fn land_committed(app: &mut App, ticket: u64, result: Result<(), String>) {
    let Some((view, parked)) = waiting_on(app, ticket) else {
        return;
    };
    if let Err(why) = result {
        view.log
            .say(LogKind::Bad, format!("The commit stopped: {why}"));
        view.enter(Stage::CommitFailed);
        if parked {
            app.flash = Some(Flash::failed(format!(
                "the merge didn't commit: {why} — Enter on a conflicted file comes back to it"
            )));
        }
        return;
    }
    view.log.say(LogKind::Good, "Merge committed.");
    start_push(app, ticket);
}

/// The push, through `git_sync` like any other so the band rules and the
/// one-per-checkout rule hold, its output landing in the resolver's log. A
/// sync that wouldn't start says why in the FLASH; the log takes that.
fn start_push(app: &mut App, ticket: u64) {
    let Some((view, _)) = waiting_on(app, ticket) else {
        return;
    };
    view.enter(Stage::Pushing);
    view.started = Instant::now();
    view.note = None;
    let (worktree, log) = (view.worktree.clone(), view.log.clone());
    app.git_sync.logs.insert(worktree.clone(), log);
    crate::git_sync::request(app, worktree.clone(), crate::git_sync::Op::PUSH);
    // Started, it took the log; another push already going answers this view
    // too when it lands. Anything else — a pull running, whose landing would
    // tell this view nothing — means it can't wait.
    app.git_sync.logs.remove(&worktree);
    if matches!(
        app.git_sync.inflight.get(&worktree),
        Some(crate::git_sync::Op::Push { .. })
    ) {
        return;
    }
    let why = app
        .flash
        .as_deref()
        .unwrap_or("the push couldn't start")
        .to_string();
    if let Some((view, _)) = waiting_on(app, ticket) {
        view.log.say(LogKind::Bad, why);
        view.stage = Stage::PushFailed;
    }
}

/// A PUSH of `worktree` landed (`git_sync::land`, which has already set the
/// FLASH). The resolver showing it — or parked on it, or one a `⇧P` from the
/// grid beat to it — hears how it went. Sent, it gives way to the pull
/// request's Checks tab, unless it was left parked: nobody is pulled off
/// whatever they went to. Not sent, it stays with the reason.
pub(crate) fn push_landed(
    app: &mut App,
    worktree: &WorktreeId,
    outcome: &crate::git_sync::Outcome,
) {
    use crate::git_sync::Outcome;
    let wants = |view: &ConflictView| {
        &view.worktree == worktree
            && matches!(
                view.stage,
                Stage::Pushing | Stage::PushFailed | Stage::ReadyToPush
            )
    };
    let on_screen = matches!(&app.overlay, Some(Overlay::Conflicts(view)) if wants(view));
    let parked = app.conflicts.parked.as_deref().is_some_and(wants);
    if !on_screen && !parked {
        return;
    }
    match outcome {
        Outcome::Pushed { .. } | Outcome::Published { .. } | Outcome::NothingToPush { .. } => {
            if on_screen {
                land_on_pull_request(app);
            } else if let Some(view) = app.conflicts.parked.take() {
                settled(app, &view.url);
            }
        }
        _ => {
            let said = app
                .flash
                .as_deref()
                .unwrap_or("the push didn't land")
                .to_string();
            let view = if on_screen {
                match &mut app.overlay {
                    Some(Overlay::Conflicts(view)) => Some(&mut **view),
                    _ => None,
                }
            } else {
                app.conflicts.parked.as_deref_mut()
            };
            if let Some(view) = view {
                let kind = if matches!(outcome, Outcome::ConfirmPush { .. }) {
                    LogKind::Note
                } else {
                    LogKind::Bad
                };
                view.log.say(kind, said);
                view.enter(Stage::PushFailed);
            }
            if on_screen {
                app.flash = None;
            }
        }
    }
    app.dirty = true;
}

/// The merge is on GitHub: the PULL REQUESTS MODAL, on the pull request's
/// Checks tab, where the rest of its story is told — or, merged from a DIFF
/// VIEWER, that viewer again, read afresh.
fn land_on_pull_request(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = app.overlay.take() else {
        return;
    };
    settled(app, &view.url);
    let back = match view.back {
        Some(Under::Diff(diff)) => return crate::diff_actions::reopen(app, *diff),
        Some(Under::PullRequests(back)) => Some(*back),
        None => None,
    };
    crate::pr_modal::land_on(app, back, &view.url, crate::pr_preview::PrTab::Checks);
}

/// `⌘W`, twice: `git merge --abort`, the checkout back as it was.
fn abort(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &mut app.overlay else {
        return;
    };
    if !view.abortable() {
        return;
    }
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
            let (branch, under) = (view.branch.clone(), view.back.take());
            close(app, under);
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

/// Leave the resolver for the PULL REQUESTS MODAL or DIFF VIEWER it was
/// opened from, or the panels; a merge still underway is left as it is, and a view with git
/// at work or a result to read is PARKED.
pub(crate) fn back(app: &mut App) {
    let Some(Overlay::Conflicts(mut view)) = app.overlay.take() else {
        return;
    };
    let under = view.back.take();
    if view.parks() {
        app.conflicts.parked = Some(view);
    }
    close(app, under);
}

/// What the resolver stood on back up: the modal or the viewer, or the
/// panels.
fn close(app: &mut App, under: Option<Under>) {
    match under {
        Some(Under::PullRequests(under)) => crate::pr_modal::reopen(app, *under),
        Some(Under::Diff(under)) => crate::diff_actions::reopen(app, *under),
        None => app.overlay = None,
    }
}

/// Esc: [`back`], the FLASH saying what goes on without the modal.
fn leave(app: &mut App) {
    let Some(Overlay::Conflicts(view)) = &app.overlay else {
        return;
    };
    let branch = view.branch.clone();
    let flash = match view.stage {
        Stage::Merging => Some(Flash::working(format!("merging into ⎇ {branch}…"))),
        Stage::Committing => Some(Flash::working(format!("committing ⎇ {branch}'s merge…"))),
        Stage::Pushing => Some(Flash::working(format!("pushing ⎇ {branch}…"))),
        Stage::CommitFailed | Stage::Resolving => merging(&view.root).then(|| {
            Flash::note(format!(
                "⎇ {branch} is still mid-merge — Enter on a conflicted file comes back to it"
            ))
        }),
        Stage::ReadyToPush | Stage::PushFailed => Some(Flash::note(format!(
            "⎇ {branch}'s merge is committed, not pushed — Enter on a conflicted file comes back to it"
        ))),
        Stage::Refused => None,
    };
    back(app);
    if flash.is_some() {
        app.flash = flash;
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
    pub const LOG: Key = Key::new(&["l"], "log");
    pub const COMMIT: Key = Key::new(&["cmd+s", "ctrl+s"], "commit & push");
    pub const ABORT: Key = Key::new(&["cmd+w", "ctrl+w"], "abort merge");
    pub const OUTSIDE: Key = crate::hints::IN_CURSOR;
    pub const BACK: Key = Key::new(&["esc"], "back");

    #[cfg(test)]
    pub const ALL: &[Key] = &[
        CURRENT, INCOMING, BOTH, EDIT, STEP, FILE, UNDO, LOG, COMMIT, ABORT, OUTSIDE, BACK,
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

/// The keys along the bottom edge: the ones the stage has a use for, what
/// comes next first.
pub(crate) fn hints(view: &ConflictView) -> Vec<crate::hints::Hint> {
    let reading = view.showing();
    let has_files = !view.files.is_empty();
    let browse = || {
        let log = if reading == Pane::Log {
            keys::LOG.hint_as("file")
        } else {
            keys::LOG.hint()
        };
        if has_files {
            vec![keys::FILE.hint(), log]
        } else {
            Vec::new()
        }
    };
    let mut hints = Vec::new();
    match view.stage {
        Stage::Merging | Stage::Committing | Stage::Pushing => {
            hints.extend(browse());
            hints.push(keys::BACK.hint_as("leave, it keeps going").kept());
        }
        Stage::Refused => hints.push(keys::BACK.hint().kept()),
        Stage::Resolving => {
            let open_here = view.file().is_some_and(|f| f.left > 0) && reading == Pane::File;
            if open_here {
                hints.extend([
                    keys::CURRENT.hint().kept(),
                    keys::INCOMING.hint().kept(),
                    keys::BOTH.hint(),
                ]);
            }
            if view.left() == 0 {
                hints.push(keys::COMMIT.hint().kept());
            }
            hints.push(keys::EDIT.hint());
            hints.push(keys::STEP.hint());
            hints.extend(browse());
            if !view.undo.is_empty() {
                hints.push(keys::UNDO.hint());
            }
            hints.extend([keys::ABORT.hint(), keys::OUTSIDE.hint(), keys::BACK.hint()]);
        }
        Stage::CommitFailed => {
            hints.push(keys::COMMIT.hint_as("try the commit again").kept());
            hints.push(keys::EDIT.hint());
            hints.extend(browse());
            hints.extend([keys::ABORT.hint(), keys::OUTSIDE.hint(), keys::BACK.hint()]);
        }
        Stage::ReadyToPush => {
            hints.push(keys::COMMIT.hint_as("push").kept());
            hints.push(keys::BACK.hint());
        }
        Stage::PushFailed => {
            hints.push(keys::COMMIT.hint_as("push again").kept());
            hints.extend(browse());
            hints.push(keys::BACK.hint());
        }
    }
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
    let on_log = view.showing() == Pane::Log;
    match key.code {
        KeyCode::Down if on_log => scroll_log(view, 1),
        KeyCode::Up if on_log => scroll_log(view, -1),
        KeyCode::PageDown if on_log => scroll_log(view, page as i64),
        KeyCode::PageUp if on_log => scroll_log(view, -(page as i64)),
        KeyCode::Down => view.step(1),
        KeyCode::Up => view.step(-1),
        KeyCode::Tab if !view.files.is_empty() => view.pick((view.selected + 1) % view.files.len()),
        KeyCode::BackTab if !view.files.is_empty() => {
            view.pick((view.selected + view.files.len() - 1) % view.files.len())
        }
        KeyCode::PageDown => view.scroll += page,
        KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(page),
        _ if keys::LOG.matches(&key) => view.toggle_log(),
        _ if keys::UNDO.matches(&key) => undo(view),
        _ => {}
    }
}

/// Move the log `by` rows; the draw takes it back to following the tail once
/// it reaches the end.
fn scroll_log(view: &mut ConflictView, by: i64) {
    let top = view.log_top.unwrap_or(view.log_at) as i64;
    view.log_top = Some((top + by).max(0) as usize);
}

/// The wheel scrolls the pane; a click on a file opens it, and one on a
/// conflict's button presses it.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let pos = Position::new(mouse.column, mouse.row);
    app.dirty = true;
    let Some(Overlay::Conflicts(view)) = &mut app.overlay else {
        return;
    };
    let on_log = view.showing() == Pane::Log;
    match mouse.kind {
        MouseEventKind::ScrollDown if view.body_area.contains(pos) && on_log => {
            scroll_log(view, i64::from(WHEEL_LINES));
        }
        MouseEventKind::ScrollUp if view.body_area.contains(pos) && on_log => {
            scroll_log(view, -i64::from(WHEEL_LINES));
        }
        MouseEventKind::ScrollDown if view.body_area.contains(pos) => {
            view.scroll += WHEEL_LINES as usize;
        }
        MouseEventKind::ScrollUp if view.body_area.contains(pos) => {
            view.scroll = view.scroll.saturating_sub(WHEEL_LINES as usize);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(&(_, i)) = view.row_rects.iter().find(|(r, _)| r.contains(pos)) {
                view.pick(i);
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

/// A step of the merge, as the state panel draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Done,
    /// Waiting on you.
    Next,
    /// git is at it.
    Running,
    Failed,
    Later,
}

/// The state panel's four steps: what each is called just now, how far along
/// it is, and a word beside it.
fn steps(view: &ConflictView) -> [(Mark, String, String); 4] {
    let stage = view.stage;
    let left = view.left();
    let clock = view.clock();
    let merge = match stage {
        Stage::Merging => (
            Mark::Running,
            format!("Merging origin/{}", view.base),
            clock.clone(),
        ),
        Stage::Refused => (Mark::Failed, "Merge not started".to_string(), String::new()),
        _ => (
            Mark::Done,
            format!("Merged origin/{}", view.base),
            String::new(),
        ),
    };
    let resolve = match stage {
        Stage::Merging | Stage::Refused => (Mark::Later, "Resolve conflicts", String::new()),
        Stage::Resolving if left > 0 => (Mark::Next, "Resolve conflicts", format!("{left} left")),
        _ if view.files.is_empty() => (Mark::Done, "Nothing to resolve", String::new()),
        _ => (Mark::Done, "Resolve conflicts", String::new()),
    };
    let commit = match stage {
        Stage::Merging | Stage::Refused => (Mark::Later, String::new()),
        Stage::Resolving if left > 0 => (Mark::Later, String::new()),
        Stage::Resolving => (Mark::Next, String::new()),
        Stage::Committing => (Mark::Running, clock.clone()),
        Stage::CommitFailed => (Mark::Failed, "stopped".to_string()),
        Stage::ReadyToPush | Stage::Pushing | Stage::PushFailed => (Mark::Done, String::new()),
    };
    let push = match stage {
        Stage::Pushing => (Mark::Running, clock),
        Stage::PushFailed => (Mark::Failed, "commit kept".to_string()),
        Stage::ReadyToPush => (Mark::Next, String::new()),
        _ => (Mark::Later, String::new()),
    };
    [
        merge,
        (resolve.0, resolve.1.to_string(), resolve.2),
        (commit.0, "Commit the merge".to_string(), commit.1),
        (push.0, "Push to origin".to_string(), push.1),
    ]
}

/// The state panel's last line: what to do now, or that there is nothing to
/// do but wait.
fn next_step(view: &ConflictView, th: Theme) -> Line<'static> {
    let key = |k: String| {
        Span::styled(
            k,
            Style::default().fg(th.accent).add_modifier(Modifier::BOLD),
        )
    };
    let word = |w: &str| Span::styled(w.to_string(), Style::default().fg(th.text));
    let label = |w: &str| Span::styled(format!("{w}  "), Style::default().fg(th.dim));
    if view.abort_armed {
        return Line::from(vec![
            key(keys::ABORT.label()),
            word(" again aborts. Every side taken is lost"),
        ]);
    }
    let commit = keys::COMMIT.label();
    let spans = match view.stage {
        Stage::Merging => vec![label("Wait"), word("fetching and merging")],
        Stage::Committing => vec![label("Wait"), word("committing, hooks and all")],
        Stage::Pushing => vec![label("Wait"), word("pushing to origin")],
        Stage::Refused => vec![label("Next"), word("sort that out, then come back")],
        Stage::Resolving if view.left() == 0 => {
            vec![label("Next"), key(commit), word(" commit & push")]
        }
        Stage::Resolving => vec![
            label("Next"),
            word("take a side: "),
            key(keys::CURRENT.label()),
            word(" "),
            key(keys::INCOMING.label()),
            word(" "),
            key(keys::BOTH.label()),
            word(", or "),
            key(keys::EDIT.label()),
            word(" to edit"),
        ],
        Stage::CommitFailed => vec![
            label("Next"),
            word("fix it, then "),
            key(commit),
            word(" again"),
        ],
        Stage::ReadyToPush => vec![label("Next"), key(commit), word(" push")],
        Stage::PushFailed => vec![label("Next"), key(commit), word(" pushes again")],
    };
    Line::from(spans)
}

/// The sentence pinned under the log: where that leaves you.
fn log_footer(view: &ConflictView, th: Theme) -> Option<(String, ratatui::style::Color)> {
    let commit = keys::COMMIT.label();
    match view.stage {
        Stage::Resolving if view.left() == 0 => Some((
            format!("Nothing is committed yet. {commit} commits the merge and pushes it."),
            th.warn,
        )),
        Stage::Resolving => Some((
            format!(
                "{} still open. {} goes back to the file.",
                crate::flash::plural(view.left(), "conflict"),
                keys::LOG.label()
            ),
            th.warn,
        )),
        Stage::CommitFailed => Some((
            format!("The merge is still underway and nothing is lost. Fix it, then {commit}."),
            th.err,
        )),
        Stage::PushFailed => Some((
            format!("The merge commit is made, it just isn't on GitHub yet. {commit} won't commit twice."),
            th.err,
        )),
        Stage::ReadyToPush => Some((
            format!("{commit} pushes it to update the pull request."),
            th.warn,
        )),
        Stage::Refused => Some(("The merge never started, so nothing changed.".to_string(), th.err)),
        Stage::Merging | Stage::Committing | Stage::Pushing => None,
    }
}

/// The RESOLVER: where the merge stands over its files on the left, and the
/// file or the log on the right.
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
    let [column, body_a] = Layout::horizontal([
        Constraint::Length(list_w),
        Constraint::Min(crate::ui::SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);
    let state_h = STATE_H.min(column.height.saturating_sub(MIN_FILES_H));
    let [state_a, list_a] =
        Layout::vertical([Constraint::Length(state_h), Constraint::Min(0)]).areas(column);

    // ---- top left: where the merge stands ----
    let title = format!("Merge — {}", view.label);
    let block = crate::ui::panel_block(&title, false, th);
    let state_inner = block.inner(state_a);
    f.render_widget(block, state_a);
    let spinner = crate::app::spinner_frame(app.spin_phase());
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("origin/{}", view.base),
            Style::default().fg(th.modified),
        ),
        Span::styled(" into ", Style::default().fg(th.dim)),
        Span::styled(format!("⎇ {}", view.branch), Style::default().fg(th.added)),
    ])];
    for (mark, name, beside) in steps(view) {
        let (glyph, glyph_color, name_style) = match mark {
            Mark::Done => ("✓", th.ok, Style::default().fg(th.muted)),
            Mark::Next => (
                "●",
                th.accent,
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ),
            Mark::Running => (
                spinner,
                th.warn,
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
            ),
            Mark::Failed => (
                "✕",
                th.err,
                Style::default().fg(th.err).add_modifier(Modifier::BOLD),
            ),
            Mark::Later => ("○", th.faint, Style::default().fg(th.faint)),
        };
        let beside_color = if mark == Mark::Failed { th.err } else { th.dim };
        lines.push(Line::from(vec![
            Span::styled(format!("{glyph} "), Style::default().fg(glyph_color)),
            Span::styled(name, name_style),
            Span::styled(format!("  {beside}"), Style::default().fg(beside_color)),
        ]));
    }
    // The steps a row each, however narrow; what's next wraps under them.
    let [steps_a, next_a] =
        Layout::vertical([Constraint::Length(lines.len() as u16), Constraint::Min(0)])
            .areas(state_inner);
    f.render_widget(Paragraph::new(lines), steps_a);
    f.render_widget(
        Paragraph::new(next_step(view, th)).wrap(Wrap { trim: false }),
        next_a,
    );

    // ---- under it: the files ----
    let settled = view.files.iter().filter(|f| f.left == 0).count();
    let title = format!("Files · {settled} of {} settled", view.files.len());
    let block = crate::ui::panel_block(&title, false, th);
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    let reading_file = view.showing() == Pane::File;
    let mut row_rects = Vec::new();
    let mut lines = Vec::new();
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
        if i == view.selected && reading_file {
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

    // ---- right: the file, or the log ----
    let file = view.file();
    let title = if reading_file {
        file.map_or(String::new(), |f| f.path.clone())
    } else {
        let doing = match view.stage {
            Stage::Merging => " · merging",
            Stage::Refused => " · merge not started",
            Stage::Resolving if view.left() == 0 => " · waiting for you",
            Stage::Resolving => "",
            Stage::Committing => " · committing",
            Stage::CommitFailed => " · commit failed",
            Stage::ReadyToPush => " · waiting for you",
            Stage::Pushing => " · pushing",
            Stage::PushFailed => " · push failed",
        };
        format!("Log{doing}")
    };
    let block = crate::ui::panel_block(&title, true, th);
    let body = block.inner(body_a);
    f.render_widget(block, body_a);
    let note_h = u16::from(view.note.is_some());
    let text_area = Rect {
        height: body.height.saturating_sub(note_h),
        ..body
    };
    let mut buttons = Vec::new();
    let mut scroll = view.scroll;
    let mut log_top = view.log_top;
    let mut log_at = view.log_at;
    if reading_file {
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
                    for (choice, does) in [(Choice::Current, current), (Choice::Incoming, incoming)]
                    {
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
    } else {
        log_at = draw_log(f, view, text_area, &mut log_top, th);
    }
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
        v.log_top = log_top;
        v.log_at = log_at;
    }
}

/// The log in `area`, what stands to be done pinned under it. Returns the top
/// row drawn; `top` is put back to following the tail once it reaches the end.
fn draw_log(
    f: &mut Frame,
    view: &ConflictView,
    area: Rect,
    top: &mut Option<usize>,
    th: Theme,
) -> usize {
    let width = usize::from(area.width).saturating_sub(3).max(1);
    let foot = log_footer(view, th);
    let foot_rows = foot
        .as_ref()
        .map_or(Vec::new(), |(text, _)| crate::pr_preview::wrap(text, width));
    let foot_h = (foot_rows.len() as u16).min(area.height.saturating_sub(1));
    let [log_a, foot_a] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(foot_h)]).areas(area);
    let mut rows: Vec<Line> = Vec::new();
    for line in view.log.lines() {
        let style = match line.kind {
            LogKind::Command => Style::default().fg(th.muted),
            LogKind::Output => Style::default().fg(th.dim),
            LogKind::Good => Style::default().fg(th.ok),
            LogKind::Note => Style::default().fg(th.warn),
            LogKind::Bad => Style::default().fg(th.err),
        };
        for row in crate::pr_preview::wrap(&line.text, width) {
            rows.push(Line::from(Span::styled(format!(" {row}"), style)));
        }
    }
    let room = usize::from(log_a.height);
    let last = rows.len().saturating_sub(room);
    let at = top.unwrap_or(last).min(last);
    if at >= last {
        *top = None;
    } else {
        *top = Some(at);
    }
    let shown: Vec<Line> = rows.into_iter().skip(at).take(room).collect();
    f.render_widget(Paragraph::new(shown), log_a);
    if let Some((_, color)) = foot {
        let lines: Vec<Line> = foot_rows
            .into_iter()
            .map(|row| Line::from(Span::styled(format!(" {row}"), Style::default().fg(color))))
            .collect();
        f.render_widget(Paragraph::new(lines), foot_a);
    }
    at
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
pub(crate) mod tests {
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

    pub(crate) fn git(repo: &Path, args: &[&str]) -> String {
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
    pub(crate) fn identify(repo: &Path) {
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
    pub(crate) fn conflicted_clone(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
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
        let log = GitLog::default();
        let Prepared::Conflicts(files) =
            prepare(&clone, "feature", "main", &head, &quit, &log).unwrap()
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
            prepare(&clone, "feature", "main", &head, &quit, &log),
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

        commit_merge(&clone, &["a.txt".to_string()], &quit, &log).unwrap();
        let said = log.lines();
        assert!(
            said.iter().any(|l| l.text.contains("CONFLICT")),
            "the merge's own words: {said:?}"
        );
        assert!(
            said.iter().any(|l| l.text == "$ git commit --no-edit"),
            "{said:?}"
        );
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
        let quit = AtomicBool::new(false);
        let why = prepare(&clone, "feature", "main", "", &quit, &GitLog::default()).unwrap_err();
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
            "Merge — #7 t",
            "origin/main into ⎇ feature",
            "✓ Merged origin/main",
            "● Resolve conflicts  2 left",
            "○ Commit the merge",
            "○ Push to origin",
            "Files · 0 of 2 settled",
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

    /// A project with `clone` checked out on `feature` as worktree `w`, and
    /// pull request #7 its page says conflicts, up in the PULL REQUESTS
    /// MODAL.
    fn app_with_pull_request(clone: &Path) -> (App, String) {
        let mut app = App::new();
        let project = orion_core::ProjectId("p".into());
        app.tree.projects.push(orion_core::Project {
            id: project.clone(),
            name: "demo".into(),
            repo_path: clone.to_path_buf(),
            sort_order: 0,
        });
        app.tree.worktrees.push(orion_core::Worktree {
            id: WorktreeId("w".into()),
            project_id: project.clone(),
            path: clone.to_path_buf(),
            branch: "feature".into(),
            is_main: false,
            sort_order: 0,
        });
        let url = "https://github.com/o/r/pull/7".to_string();
        let detail = crate::pull_request::PrDetail {
            number: 7,
            url: url.clone(),
            title: "t".into(),
            base: "main".into(),
            head: "feature".into(),
            head_sha: git(clone, &["rev-parse", "HEAD"]),
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
        want(&mut app, &url, clone);
        let row = |number: u64, head: &str, url: String| crate::pull_request::OpenPr {
            number,
            title: "t".into(),
            url,
            answered_draft: false,
            answered: Default::default(),
            head: head.into(),
            mine: false,
            head_sha: String::new(),
            meta: Default::default(),
        };
        let now = std::time::Instant::now();
        app.open_prs.insert(
            project.clone(),
            crate::app::OpenPrs {
                merged: Vec::new(),
                // Another first, so landing on #7 has to go looking for it.
                list: vec![
                    row(3, "other", "https://github.com/o/r/pull/3".into()),
                    row(7, "feature", url.clone()),
                ],
                at: now,
                due: now + std::time::Duration::from_secs(60),
                step: std::time::Duration::from_secs(60),
            },
        );
        app.overlay = Some(Overlay::PullRequests(
            crate::pr_modal::PullRequestsView::new(project, "demo".into(), clone.to_path_buf()),
        ));
        (app, url)
    }

    fn key(code: crossterm::event::KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn press_cmd_s(app: &mut App) {
        handle_key(
            app,
            KeyEvent::new(
                crossterm::event::KeyCode::Char('s'),
                crossterm::event::KeyModifiers::SUPER,
            ),
        );
    }

    fn the_view(app: &App) -> &ConflictView {
        match &app.overlay {
            Some(Overlay::Conflicts(view)) => view,
            other => panic!("no resolver: {other:?}"),
        }
    }

    fn log_of(view: &ConflictView) -> String {
        view.log
            .lines()
            .into_iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Open #7's resolver and settle both its files.
    fn settle(app: &mut App, url: &str) {
        crate::pr_preview::run_act(
            app,
            7,
            url,
            "#7 t",
            crate::pr_preview::RowAct::File("a.txt".into()),
            true,
            &mut Vec::new(),
        );
        assert_eq!(the_view(app).stage, Stage::Resolving, "{:?}", app.flash);
        assert_eq!(the_view(app).showing(), Pane::File, "a conflict: the file");
        handle_key(app, key(crossterm::event::KeyCode::Char('i')));
        handle_key(app, key(crossterm::event::KeyCode::Char('c')));
    }

    /// The whole road: every conflict settled hands the pane to the log and
    /// says what's next; a hook's stop stays up with its words, and ⌘S tries
    /// again; a failed push stays up too, and ⌘S pushes without committing
    /// twice; the push landing is the pull request's Checks tab.
    #[test]
    fn the_merge_runs_to_the_pull_request_in_one_modal() {
        use crate::git_sync::{Answer as SyncAnswer, Op, Outcome};
        use crossterm::event::KeyCode;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let (mut app, url) = app_with_pull_request(&clone);
        settle(&mut app, &url);

        let view = the_view(&app);
        assert_eq!(view.left(), 0);
        assert_eq!(view.showing(), Pane::Log, "settled: the log");
        assert!(log_of(view).contains("Nothing is committed yet"));
        let shot = screen(&mut app, 140, 30);
        for words in [
            "✓ Resolve conflicts",
            "● Commit the merge",
            "Next  ",
            "Log · waiting for you",
            "Nothing is committed yet.",
            "Files · 2 of 2 settled",
        ] {
            assert!(shot.contains(words), "{words:?} in\n{shot}");
        }
        crate::hints::assert_hints_from(&hints(the_view(&app)), keys::ALL);

        // `l` reads a file back, and Tab does too; `l` returns to the log.
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(the_view(&app).showing(), Pane::File);
        handle_key(&mut app, key(KeyCode::Char('l')));
        assert_eq!(the_view(&app).showing(), Pane::Log);

        // A hook says no: the modal stays, its words in the log.
        let hook = clone.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho hook-said-no >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        press_cmd_s(&mut app);
        let view = the_view(&app);
        assert_eq!(view.stage, Stage::CommitFailed, "{}", log_of(view));
        assert!(log_of(view).contains("hook-said-no"), "{}", log_of(view));
        assert!(merging(&clone), "the merge is still underway");
        crate::hints::assert_hints_from(&hints(view), keys::ALL);
        let shot = screen(&mut app, 140, 30);
        assert!(shot.contains("✕ Commit the merge"), "{shot}");

        // Fixed, ⌘S commits, then the push goes (git_sync, which the tests
        // land by hand).
        std::fs::remove_file(&hook).unwrap();
        press_cmd_s(&mut app);
        let view = the_view(&app);
        assert_eq!(view.stage, Stage::Pushing, "{}", log_of(view));
        assert!(!merging(&clone), "committed");
        assert!(app.status_anim_active(), "the log and its clock repaint");
        assert!(app.git_sync.inflight.contains_key(&WorktreeId("w".into())));
        crate::hints::assert_hints_from(&hints(the_view(&app)), keys::ALL);
        let merges = git(&clone, &["rev-list", "--count", "--merges", "HEAD"]);
        assert_eq!(merges, "1");

        // The push fails: the modal stays, saying so.
        let landed = |outcome| SyncAnswer {
            worktree: WorktreeId("w".into()),
            op: Op::PUSH,
            outcome,
        };
        crate::git_sync::land(&mut app, landed(Outcome::Failed("no network".into())));
        let view = the_view(&app);
        assert_eq!(view.stage, Stage::PushFailed);
        assert!(log_of(view).contains("no network"), "{}", log_of(view));
        crate::hints::assert_hints_from(&hints(view), keys::ALL);
        let shot = screen(&mut app, 140, 30);
        assert!(shot.contains("✕ Push to origin"), "{shot}");
        assert!(shot.contains("✓ Commit the merge"), "{shot}");

        // ⌘W no longer aborts anything: the merge is committed.
        let abort = KeyEvent::new(KeyCode::Char('w'), crossterm::event::KeyModifiers::SUPER);
        handle_key(&mut app, abort);
        handle_key(&mut app, abort);
        assert_eq!(the_view(&app).stage, Stage::PushFailed);

        // ⌘S pushes again, and doesn't commit again.
        press_cmd_s(&mut app);
        assert_eq!(the_view(&app).stage, Stage::Pushing);
        let merges = git(&clone, &["rev-list", "--count", "--merges", "HEAD"]);
        assert_eq!(merges, "1");

        // It lands: the PULL REQUESTS MODAL, on #7's Checks.
        crate::git_sync::land(
            &mut app,
            landed(Outcome::Pushed {
                upstream: "origin/feature".into(),
                commits: 2,
            }),
        );
        let Some(Overlay::PullRequests(view)) = &app.overlay else {
            panic!("not the pull request: {:?}", app.overlay);
        };
        assert_eq!(view.tabs.tab, crate::pr_preview::PrTab::Checks);
        assert_eq!(view.selected_url.as_deref(), Some(url.as_str()));
        assert_eq!(view.selected, 1, "the cursor on #7's row");
        assert_eq!(
            app.pending_pr_detail.as_ref().map(|(p, _)| p.url.as_str()),
            Some(url.as_str()),
            "its page asked for fresh, read moments ago or not"
        );
        assert!(of_page(&app, &url).is_none(), "the marks are gone");
        assert!(app.conflicts.parked.is_none());
        assert!(app.flash.as_deref().is_some_and(|f| f.contains("pushed")));
    }

    /// Esc while git is at work leaves it working: the answer still lands,
    /// and Enter on a conflicted file comes back to the same view. A push
    /// that lands then doesn't pull anyone off what they went to.
    #[test]
    fn esc_leaves_the_push_running_and_enter_comes_back() {
        use crate::git_sync::{Answer as SyncAnswer, Op, Outcome};
        use crate::pr_preview::RowAct;
        use crossterm::event::KeyCode;
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let (mut app, url) = app_with_pull_request(&clone);
        settle(&mut app, &url);
        press_cmd_s(&mut app);
        assert_eq!(the_view(&app).stage, Stage::Pushing);

        handle_key(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.overlay, Some(Overlay::PullRequests(_))));
        assert!(app.flash.as_deref().is_some_and(|f| f.contains("pushing")));
        assert!(app.conflicts.parked.is_some());

        let file = RowAct::File("a.txt".into());
        crate::pr_preview::run_act(&mut app, 7, &url, "#7 t", file, true, &mut Vec::new());
        assert_eq!(the_view(&app).stage, Stage::Pushing, "the same one, back");
        assert!(the_view(&app).back.is_some(), "Esc still goes to the modal");
        assert!(app.conflicts.parked.is_none());

        handle_key(&mut app, key(KeyCode::Esc));
        crate::git_sync::land(
            &mut app,
            SyncAnswer {
                worktree: WorktreeId("w".into()),
                op: Op::PUSH,
                outcome: Outcome::Pushed {
                    upstream: "origin/feature".into(),
                    commits: 2,
                },
            },
        );
        assert!(app.conflicts.parked.is_none(), "it's over");
        assert!(of_page(&app, &url).is_none());
        assert!(matches!(app.overlay, Some(Overlay::PullRequests(_))));
    }

    /// A pull running in the checkout refuses the push, and its landing
    /// would say nothing to the resolver: it doesn't wait for one, and ⌘S
    /// tries again.
    #[test]
    fn a_push_a_pull_refused_does_not_leave_the_view_waiting() {
        use crate::git_sync::Op;
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let (mut app, url) = app_with_pull_request(&clone);
        settle(&mut app, &url);
        let id = WorktreeId("w".into());
        app.git_sync.inflight.insert(id.clone(), Op::Pull);
        press_cmd_s(&mut app);
        let view = the_view(&app);
        assert_eq!(view.stage, Stage::PushFailed, "{}", log_of(view));
        assert!(!app.git_sync.logs.contains_key(&id), "no log left behind");
        app.git_sync.inflight.remove(&id);
        press_cmd_s(&mut app);
        assert_eq!(the_view(&app).stage, Stage::Pushing);
    }

    /// A checkout landing under a resolver on screen doesn't start a second
    /// one, whose answer would have nowhere to go.
    #[test]
    fn one_resolver_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let (mut app, url) = app_with_pull_request(&clone);
        settle(&mut app, &url);
        let ticket = the_view(&app).ticket;
        open(&mut app, &url, "#7 t", None, &mut Vec::new());
        assert_eq!(the_view(&app).ticket, ticket, "the same one");
        assert!(app.flash.as_deref().is_some_and(|f| f.contains("finish")));
        assert!(app.conflicts.parked.is_none());
    }

    /// A checkout that can't be merged into says so in the modal, in the
    /// same steps, rather than a line in the footer.
    #[test]
    fn a_refusal_says_why_in_the_modal() {
        use crate::pr_preview::RowAct;
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        std::fs::write(clone.join("same.txt"), "mine\n").unwrap();
        let (mut app, url) = app_with_pull_request(&clone);
        let file = RowAct::File("a.txt".into());
        crate::pr_preview::run_act(&mut app, 7, &url, "#7 t", file, true, &mut Vec::new());
        let view = the_view(&app);
        assert_eq!(view.stage, Stage::Refused);
        crate::hints::assert_hints_from(&hints(view), keys::ALL);
        let shot = screen(&mut app, 140, 30);
        for words in [
            "✕ Merge not started",
            "uncommitted changes",
            "nothing changed",
        ] {
            assert!(shot.contains(words), "{words:?} in\n{shot}");
        }
        handle_key(&mut app, key(crossterm::event::KeyCode::Esc));
        assert!(matches!(app.overlay, Some(Overlay::PullRequests(_))));
        assert!(app.conflicts.parked.is_none());
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
