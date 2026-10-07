//! Git status/diff readers for the diff modal.
//!
//! The readers are synchronous `std::process`; who calls them is what
//! changed. They used to run inside the key handler — opening the modal,
//! switching files — and the loop waited: 47 ms to open on a small checkout
//! and 11 ms per file walked under the INPUT LATENCY PROBE, 80 ms to over
//! a second for the `git status` alone on a large one. A view with
//! BACKGROUND READS (`view_jobs`) runs them on the blocking pool instead;
//! one without — every view a test builds — still calls them inline.

use crate::app::DiffView;
use std::path::Path;
use std::process::{Command, Output};

/// Keep pathological diffs from bloating the overlay state.
const MAX_DIFF_LINES: usize = 20_000;

/// One changed file from `git status --porcelain=v1 -z`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffFile {
    /// Path relative to the worktree root (for renames: the NEW path).
    pub path: String,
    /// Pre-rename path for R/C entries.
    pub orig_path: Option<String>,
    /// The two porcelain status columns, e.g. ['M',' '], [' ','M'], ['?','?'].
    pub xy: [char; 2],
    /// The file's own lines added and removed, as the DIFF VIEWER's list
    /// prints them on its right; None until counted, and for a binary file.
    pub lines: Option<LineChanges>,
}

impl DiffFile {
    pub fn is_untracked(&self) -> bool {
        self.xy == ['?', '?']
    }

    /// The raw two-character code for the list column ("M ", " M", "??", …).
    pub fn status_str(&self) -> String {
        self.xy.iter().collect()
    }
}

/// What the DIFF VIEWER's file list and diffs are of — what the COMMIT
/// LIST's cursor and ticks put on screen (`commit_list::CommitList::showing`)
/// when its file list last landed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum DiffScope {
    /// The working tree against HEAD, untracked files included: what the
    /// viewer always showed, and still opens on while there is any.
    #[default]
    Uncommitted,
    /// One commit against its first parent; `None` for a root commit,
    /// which is diffed against git's empty tree.
    Commit { sha: String, parent: Option<String> },
    /// Several ticked rows read TOGETHER: each [`Range`] one stretch of
    /// the branch, oldest first. One range is one ordinary diff; a file
    /// two of them touch shows each range's diff in turn (`scoped_diff`).
    Ranges(Vec<Range>),
}

/// One stretch of a branch a TOGETHER diff shows: `from` to `to`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Range {
    pub from: RangeStart,
    /// Where it ends: a commit, or None for the working tree, untracked
    /// files included.
    pub to: Option<String>,
    /// What the pane calls it where two ranges meet: `1ec007c..dde6cc9`.
    pub label: String,
}

/// Where a [`Range`] starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RangeStart {
    /// A commit: the first parent of the range's oldest one.
    Rev(String),
    /// git's empty tree: the range starts at a root commit.
    Empty,
    /// Where the range's end — HEAD, for the working tree — left `base`:
    /// the range runs from the branch's first commit, merges of the base
    /// included, and measured from the merge-base those merges add
    /// nothing of the base's own.
    MergeBase { base: String },
}

/// The commit a [`Range`] starts at, asked of git where it has to be.
fn range_start(root: &Path, range: &Range) -> Result<String, String> {
    match &range.from {
        RangeStart::Rev(rev) => Ok(rev.clone()),
        RangeStart::Empty => empty_tree(root).ok_or_else(|| "git hash-object failed".to_string()),
        RangeStart::MergeBase { base } => {
            let tip = range.to.as_deref().unwrap_or("HEAD");
            let output = run_git(root, &["merge-base", tip, base])?;
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if output.status.success() && !text.is_empty() {
                Ok(text)
            } else {
                Err(format!("no history in common with {base}"))
            }
        }
    }
}

/// What a diff with nothing to show says.
pub const NO_TEXT: &str = "(no textual changes)";

/// `git -C root`, with the locks git takes on its own initiative switched
/// off. `git status` and `git diff` refresh the index's stat cache as a
/// side effect and, when it is stale — it always is while an agent edits
/// files — write it back through `.git/index.lock`. The WORKTREES PANEL's
/// changed-files badge polls `status` every two seconds on the checkout
/// the user is working in, so left on, that write lands under the agent's
/// own `git add` / `commit` / `checkout`, which then fails with "Unable to
/// create '.git/index.lock': File exists" (issue #15). Nothing the TUI
/// runs needs the refresh persisted — the agent CLIs themselves run their
/// background git with `--no-optional-locks` for the same reason — and
/// commands that must lock (the BRANCH SWITCHER's switch, stash and commit) still do: `GIT_OPTIONAL_LOCKS=0`
/// only skips the optional ones. Every TUI-side git goes through here.
pub(crate) fn git_command(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).env("GIT_OPTIONAL_LOCKS", "0");
    cmd
}

pub(crate) fn run_git(root: &Path, args: &[&str]) -> Result<Output, String> {
    git_command(root)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

/// Changed files (staged + unstaged + untracked) in status order.
/// `Err` is a user-facing flash message.
pub fn changed_files(root: &Path) -> Result<Vec<DiffFile>, String> {
    let output = run_git(root, &["status", "--porcelain=v1", "-z", "-uall"])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git status failed: {}", stderr.trim()));
    }
    Ok(parse_status_z(&output.stdout))
}

/// Parse NUL-separated porcelain v1: `XY path\0`, and for X ∈ {R, C} a second
/// NUL-terminated field holds the ORIGINAL path (`XY new\0old\0`).
pub fn parse_status_z(bytes: &[u8]) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut fields = bytes.split(|b| *b == 0);
    while let Some(entry) = fields.next() {
        let entry = String::from_utf8_lossy(entry);
        if entry.len() < 4 {
            continue;
        }
        let mut chars = entry.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');
        let path = entry[3..].to_string();
        let orig_path = if matches!(x, 'R' | 'C') {
            fields
                .next()
                .map(|f| String::from_utf8_lossy(f).into_owned())
        } else {
            None
        };
        files.push(DiffFile {
            path,
            orig_path,
            xy: [x, y],
            lines: None,
        });
    }
    files
}

/// The files behind what the COMMIT LIST puts on screen other than the
/// uncommitted changes, in path order: a commit's against its first
/// parent, or every file any of a TOGETHER scope's ranges touch — one that
/// ends at the working tree with its changes staged or not and every
/// untracked file. `Err` is a user-facing message.
pub fn scope_files(root: &Path, scope: &DiffScope) -> Result<Vec<DiffFile>, String> {
    match scope {
        DiffScope::Uncommitted => changed_files(root),
        DiffScope::Commit { sha, parent } => {
            let from = match parent {
                Some(from) => from.clone(),
                None => empty_tree(root).ok_or("git hash-object failed")?,
            };
            let mut files = name_status(root, &from, Some(sha))?;
            count_range_lines(root, &from, Some(sha), &mut files);
            Ok(files)
        }
        DiffScope::Ranges(ranges) => {
            let mut files: Vec<DiffFile> = Vec::new();
            for range in ranges {
                let from = range_start(root, range)?;
                let mut found = name_status(root, &from, range.to.as_deref())?;
                if range.to.is_none() {
                    found.extend(
                        changed_files(root)?
                            .into_iter()
                            .filter(DiffFile::is_untracked),
                    );
                }
                count_range_lines(root, &from, range.to.as_deref(), &mut found);
                for file in found {
                    match files.iter_mut().find(|f| f.path == file.path) {
                        Some(seen) => {
                            seen.xy[0] = combined_status(seen.xy[0], file.xy[0]);
                            seen.lines = match (seen.lines, file.lines) {
                                (Some(a), Some(b)) => Some(LineChanges {
                                    added: a.added + b.added,
                                    removed: a.removed + b.removed,
                                }),
                                (a, b) => a.or(b),
                            };
                        }
                        None => files.push(file),
                    }
                }
            }
            files.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(files)
        }
    }
}

/// `git diff --name-status` from `from` to `to` (the working tree for
/// None), renames detected.
fn name_status(root: &Path, from: &str, to: Option<&str>) -> Result<Vec<DiffFile>, String> {
    let mut args = vec!["diff", "--name-status", "-z", "-M", "--no-color", from];
    args.extend(to);
    args.push("--");
    let output = run_git(root, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git diff failed: {}", stderr.trim()));
    }
    Ok(parse_name_status_z(&output.stdout))
}

/// The list's status letter for a file two ranges both touch: added by the
/// first stays added (unless the second deletes it), deleted by the last
/// is deleted, and anything else changed it.
fn combined_status(first: char, then: char) -> char {
    match (first, then) {
        (a, b) if a == b => a,
        (_, 'D') => 'D',
        ('A', _) | ('?', _) => first,
        _ => 'M',
    }
}

/// Parse `git diff --name-status -z`: a status field (`M`, `A`, `D`, `T`,
/// or `R100` / `C75` with a score), then the path — and for a rename or a
/// copy two paths, the original first. The status letter fills the first
/// porcelain column, so the list's gutter reads and colours as it does for
/// a staged change.
pub fn parse_name_status_z(bytes: &[u8]) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut fields = bytes
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(status) = fields.next() {
        let Some(code) = status.chars().next() else {
            continue;
        };
        let Some(first) = fields.next() else {
            break;
        };
        let (path, orig_path) = if matches!(code, 'R' | 'C') {
            (fields.next().unwrap_or_default(), Some(first))
        } else {
            (first, None)
        };
        files.push(DiffFile {
            path,
            orig_path,
            xy: [code, ' '],
            lines: None,
        });
    }
    files
}

/// Lines added and removed across a checkout's uncommitted changes, as the
/// DIFF VIEWER shows them: tracked files against HEAD, staged or not, and
/// every line of an untracked file as added. What the LAUNCHER VIEW's cards
/// print after their changed-file count (CARD LINE COUNTS).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineChanges {
    pub added: u64,
    pub removed: u64,
}

impl LineChanges {
    pub fn is_empty(self) -> bool {
        self.added == 0 && self.removed == 0
    }
}

/// The biggest untracked file read to count its lines; a bigger one — a
/// build artifact, a dump — counts nothing rather than stall the read.
const UNTRACKED_FILE_CAP: u64 = 1 << 20;
/// The most untracked bytes one count reads, so a checkout full of new
/// files costs a bounded amount per poll; files past it count nothing.
const UNTRACKED_READ_BUDGET: u64 = 16 << 20;

/// Count the changed lines behind `files`, the list `changed_files` just
/// read there: one `git diff --numstat` for the tracked ones — against
/// HEAD, or git's empty tree on a checkout with no commit yet — and a read
/// from disk for each untracked one. None when git couldn't say.
pub fn line_changes(root: &Path, files: &[DiffFile]) -> Option<LineChanges> {
    let mut total = LineChanges::default();
    if files.iter().any(|f| !f.is_untracked()) {
        total = match tracked_line_changes(root, "HEAD") {
            Some(lines) => lines,
            // No commit yet: every tracked line is new.
            None if !has_head(root) => tracked_line_changes(root, &empty_tree(root)?)?,
            None => return None,
        };
    }
    total.added += untracked_lines(root, files);
    Some(total)
}

/// `git diff <base> --numstat`, summed.
fn tracked_line_changes(root: &Path, base: &str) -> Option<LineChanges> {
    tracked_numstat(root, base).map(|(_, lines)| lines)
}

/// `git diff <base> --numstat`: how many files, and their lines summed.
fn tracked_numstat(root: &Path, base: &str) -> Option<(usize, LineChanges)> {
    let output = run_git(
        root,
        &[
            "diff",
            base,
            "--numstat",
            "-z",
            "--no-color",
            "--no-ext-diff",
            "--",
        ],
    )
    .ok()?;
    output
        .status
        .success()
        .then(|| count_numstat_z(&output.stdout))
}

/// Git's empty tree in this repo's hash, for an unborn HEAD to diff from.
fn empty_tree(root: &Path) -> Option<String> {
    let output = run_git(root, &["hash-object", "-t", "tree", "/dev/null"]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Sum `git diff --numstat -z`: `added\tremoved\tpath\0` per file, and for
/// a rename or copy `added\tremoved\t\0old\0new\0`. A binary file reads
/// `-\t-` and adds nothing.
pub fn parse_numstat_z(bytes: &[u8]) -> LineChanges {
    count_numstat_z(bytes).1
}

/// [`parse_numstat_z`], with the number of files the records name.
fn count_numstat_z(bytes: &[u8]) -> (usize, LineChanges) {
    let mut files = 0;
    let mut total = LineChanges::default();
    for (_, lines) in numstat_records_z(bytes) {
        files += 1;
        let lines = lines.unwrap_or_default();
        total.added += lines.added;
        total.removed += lines.removed;
    }
    (files, total)
}

/// Each record of `git diff --numstat -z`: the file's path — a rename's or
/// a copy's new one — and its lines, None for a binary file (`-\t-`).
fn numstat_records_z(bytes: &[u8]) -> impl Iterator<Item = (String, Option<LineChanges>)> + '_ {
    let mut fields = bytes.split(|b| *b == 0);
    std::iter::from_fn(move || loop {
        let field = String::from_utf8_lossy(fields.next()?);
        let mut parts = field.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            // A rename: its two paths are the next two fields, old then new.
            fields.next();
            fields
                .next()
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .unwrap_or_default()
        } else {
            path.to_string()
        };
        let lines = match (added.parse(), removed.parse()) {
            (Ok(added), Ok(removed)) => Some(LineChanges { added, removed }),
            _ => None,
        };
        return Some((path, lines));
    })
}

/// Every line of the untracked `files`, read from disk within the caps
/// above; a directory (a nested repo), a symlink or a binary file counts
/// nothing, as it adds no text lines to the diff.
fn untracked_lines(root: &Path, files: &[DiffFile]) -> u64 {
    let mut budget = UNTRACKED_READ_BUDGET;
    files
        .iter()
        .filter(|f| f.is_untracked())
        .filter_map(|file| untracked_file_lines(root, &file.path, &mut budget))
        .sum()
}

/// One untracked file's lines, read from disk if it fits the caps above
/// and what is left of `budget`.
fn untracked_file_lines(root: &Path, path: &str, budget: &mut u64) -> Option<u64> {
    let path = root.join(path);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > UNTRACKED_FILE_CAP || meta.len() > *budget {
        return None;
    }
    *budget -= meta.len();
    std::fs::read(&path).ok().map(|bytes| count_lines(&bytes))
}

/// Give each of a checkout's uncommitted `files` (`changed_files`) its own
/// line counts: tracked ones from one `git diff --numstat` against HEAD —
/// or git's empty tree on a checkout with no commit yet — untracked ones
/// read from disk. `head_ok`: the checkout has a commit.
pub fn count_file_lines(root: &Path, head_ok: bool, files: &mut [DiffFile]) {
    if files.is_empty() {
        return;
    }
    let base = if head_ok {
        Some("HEAD".to_string())
    } else {
        empty_tree(root)
    };
    if let Some(base) = base {
        count_range_lines(root, &base, None, files);
    }
}

/// Give `files` — what `git diff from [to]` changed, and with no `to` the
/// untracked files beside it — their own line counts.
fn count_range_lines(root: &Path, from: &str, to: Option<&str>, files: &mut [DiffFile]) {
    let mut args = vec![
        "diff",
        "--numstat",
        "-z",
        "-M",
        "--no-color",
        "--no-ext-diff",
        from,
    ];
    args.extend(to);
    args.push("--");
    let counted = match run_git(root, &args) {
        Ok(output) if output.status.success() => numstat_by_path(&output.stdout),
        _ => std::collections::HashMap::new(),
    };
    let mut budget = UNTRACKED_READ_BUDGET;
    for file in files {
        file.lines = if file.is_untracked() {
            untracked_file_lines(root, &file.path, &mut budget)
                .map(|added| LineChanges { added, removed: 0 })
        } else {
            counted.get(&file.path).copied()
        };
    }
}

/// `git diff --numstat -z`'s records by path, a binary file left out.
fn numstat_by_path(bytes: &[u8]) -> std::collections::HashMap<String, LineChanges> {
    numstat_records_z(bytes)
        .filter_map(|(path, lines)| Some((path, lines?)))
        .collect()
}

/// A file's lines as `git diff --numstat` counts them: every newline, plus
/// a last line without one; none for a binary file (a NUL in its first
/// 8000 bytes, git's own test).
fn count_lines(bytes: &[u8]) -> u64 {
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return 0;
    }
    let newlines = bytes.iter().filter(|b| **b == b'\n').count() as u64;
    newlines + u64::from(bytes.last().is_some_and(|b| *b != b'\n'))
}

/// Every file in the checkout (tracked + untracked, gitignore respected) in
/// git listing order, for the fuzzy file finder — plus the ignored `.env*`
/// files after them: `.env.local` and its kind are gitignored by design,
/// yet they are files you open (and the ones `link_env_files` symlinks into
/// every worktree). `Err` is a user-facing flash message.
pub fn list_files(root: &Path) -> Result<Vec<String>, String> {
    let output = run_git(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git ls-files failed: {}", stderr.trim()));
    }
    let mut files = nul_separated(&output.stdout);
    files.extend(ignored_env_files(root));
    Ok(files)
}

/// The ignored, untracked ENV FILES (`orion_core::env_files`). None when
/// git can't say.
fn ignored_env_files(root: &Path) -> Vec<String> {
    let Ok(output) = run_git(
        root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--",
            orion_core::env_files::PATHSPEC,
        ],
    ) else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    nul_separated(&output.stdout)
        .into_iter()
        .filter(|path| orion_core::env_files::is_env_file(Path::new(path)))
        .collect()
}

/// `git … -z` output as paths.
fn nul_separated(stdout: &[u8]) -> Vec<String> {
    stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

/// Does this checkout have any commit? Unborn HEAD changes the diff command.
pub fn has_head(root: &Path) -> bool {
    head_oid(root).is_some()
}

/// HEAD's commit OID, `None` on an unborn HEAD (or outside a repo). The
/// review marks are scoped to this: any HEAD move invalidates them.
pub fn head_oid(root: &Path) -> Option<String> {
    let output = run_git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Diff text for one file. Never fails: errors become the displayed text so
/// the modal survives a repo vanishing out from under it.
pub fn diff_for(root: &Path, file: &DiffFile, head_ok: bool) -> String {
    let head: &[&str] = if head_ok { &["HEAD"] } else { &[] };
    diff_between(root, file, head, false)
}

/// One file's diff under any scope: [`diff_for`] for the uncommitted
/// changes, and for the others the same `git diff` across one commit, or
/// across each of a TOGETHER scope's ranges — with renames detected, as
/// the scope's file list (`scope_files`) detected them.
///
/// A file one range touches reads as that range's diff and nothing else.
/// One that two or more touch reads as each range's diff in turn, oldest
/// first, each behind a `diff_doc::RANGE_MARK` line naming its range: the
/// commits between them were left out, and a diff that ran across them
/// would put their changes back in.
pub fn scoped_diff(root: &Path, scope: &DiffScope, file: &DiffFile, head_ok: bool) -> String {
    match scope {
        DiffScope::Uncommitted => diff_for(root, file, head_ok),
        DiffScope::Commit { sha, parent } => {
            let parent = match parent.clone().or_else(|| empty_tree(root)) {
                Some(parent) => parent,
                None => return "git hash-object failed".to_string(),
            };
            diff_between(root, file, &[parent.as_str(), sha.as_str()], true)
        }
        DiffScope::Ranges(ranges) => {
            let mut parts: Vec<(&str, String)> = Vec::new();
            for range in ranges {
                // An untracked file is only ever in the working tree.
                if file.is_untracked() && range.to.is_some() {
                    continue;
                }
                let from = match range_start(root, range) {
                    Ok(from) => from,
                    Err(msg) => return msg,
                };
                let mut revs = vec![from.as_str()];
                revs.extend(range.to.as_deref());
                let text = diff_between(root, file, &revs, true);
                if text != NO_TEXT {
                    parts.push((&range.label, text));
                }
            }
            match parts.len() {
                0 => NO_TEXT.to_string(),
                1 => parts.remove(0).1,
                _ => parts
                    .into_iter()
                    .map(|(label, text)| format!("{}{label}\n{text}", crate::diff_doc::RANGE_MARK))
                    .collect::<Vec<_>>()
                    .join("\n"),
            }
        }
    }
}

/// `git diff <revs> -- <file>`: against the working tree for one rev (or
/// none, the index), between the two for two. An untracked file is read
/// against `/dev/null` whatever the revs.
fn diff_between(root: &Path, file: &DiffFile, revs: &[&str], renames: bool) -> String {
    let output = if file.is_untracked() {
        // --no-index exits 1 when the files differ; only >= 2 is an error.
        run_git(
            root,
            &[
                "diff",
                "--no-index",
                "--no-color",
                "--",
                "/dev/null",
                &file.path,
            ],
        )
    } else {
        let mut args = vec!["diff"];
        args.extend(revs);
        if renames {
            args.push("-M");
        }
        args.extend(["--no-color", "--no-ext-diff", "--", &file.path]);
        if let Some(orig) = &file.orig_path {
            args.push(orig);
        }
        run_git(root, &args)
    };
    let output = match output {
        Ok(o) => o,
        Err(e) => return e,
    };
    let ok = if file.is_untracked() {
        matches!(output.status.code(), Some(0) | Some(1))
    } else {
        output.status.success()
    };
    if !ok {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return format!("git diff failed: {}", stderr.trim());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return NO_TEXT.to_string();
    }
    cap_lines(&text, MAX_DIFF_LINES, false)
}

/// What a capped text ends with, so the pane says why it stops short.
pub const TRUNCATED_MARK: &str = "\n… (truncated)";

/// The first `max` lines of `text`, joined, ending in [`TRUNCATED_MARK`]
/// when lines were dropped — or when `already_cut` says the caller trimmed
/// the text before handing it over (a byte cap), so the mark still shows.
/// Shared by the diff pane and the tree preview so both stop the same way.
pub fn cap_lines(text: &str, max: usize, already_cut: bool) -> String {
    let mut lines = text.lines();
    let mut out: String = lines.by_ref().take(max).collect::<Vec<_>>().join("\n");
    if lines.next().is_some() || already_cut {
        out.push_str(TRUNCATED_MARK);
    }
    out
}

/// Everything opening the DIFF VIEWER reads: the changed files, HEAD, and
/// the reviewed ✓ marks that still apply — each stored mark checked against
/// its file's diff as it is now, one `git diff` per mark, and the pruned
/// set written back. Off the loop for a view with BACKGROUND READS.
pub fn read_listing(root: &Path) -> Result<crate::view_jobs::DiffListing, String> {
    let mut files = changed_files(root)?;
    let head = head_oid(root);
    count_file_lines(root, head.is_some(), &mut files);
    let head_key = head.clone().unwrap_or_default();
    let stored = crate::review::load_marks(root, &head_key);
    let reviewed: std::collections::HashMap<String, u64> = files
        .iter()
        .filter_map(|file| {
            let mark = *stored.get(&file.path)?;
            let diff = diff_for(root, file, head.is_some());
            (crate::review::fingerprint(&diff) == mark).then(|| (file.path.clone(), mark))
        })
        .collect();
    if reviewed.len() != stored.len() {
        crate::review::store_marks(root, &head_key, &reviewed);
    }
    Ok(crate::view_jobs::DiffListing {
        files,
        head,
        reviewed,
        commits: None,
    })
}

/// What opening the DIFF VIEWER on a checkout reads: [`read_listing`], and
/// the COMMIT LIST beside it (`commit_list::read`), measured against the
/// `worktree_base_branch` SETTING's `base_setting`. One job rather than
/// two, so the view never has to decide what an empty `git status` means
/// before it knows whether the branch has commits to show instead.
pub fn read_opening(
    root: &Path,
    base_setting: &str,
) -> Result<crate::view_jobs::DiffListing, String> {
    let mut listing = read_listing(root)?;
    listing.commits = Some(crate::commit_list::read(root, base_setting, &listing.files));
    Ok(listing)
}

/// Put a listing into the view that was opened ahead of it (or built for
/// it): the files, HEAD, the marks — narrowed by whatever the filter holds
/// by now, reviewed files sunk, so the modal lands on the first unreviewed
/// file — and that file's diff. The COMMIT LIST read with it goes up too,
/// and a checkout with nothing uncommitted opens on the branch's changes
/// instead (`commit_list::install`).
///
/// A view opened on the badge's list (`event_loop::open_diff_view`) already
/// shows files, and maybe a reader who has moved among them: they stay on
/// the file they are on, wherever the fresh list puts it, and its diff is
/// read again only if it is no longer the same entry. A reader who has not
/// moved gets what a fresh open gives — the first unreviewed file.
pub fn fill_view(view: &mut DiffView, mut listing: crate::view_jobs::DiffListing) {
    let commits = listing.commits.take();
    let moved = !view.at_home() || view.scroll != 0;
    let before = view.selected_file().cloned();
    let head_ok = listing.head.is_some();
    let head_changed = !view.files.is_empty() && view.head_ok != head_ok;
    view.head_ok = head_ok;
    view.head_key = listing.head.unwrap_or_default();
    view.files = listing.files;
    view.reviewed = listing.reviewed;
    view.listing = None;
    view.recompute_matches();
    // The tree folds the fresh list the same way, keeping what the reader
    // folded; both lists then send the cursor home.
    if let Some(tree) = &view.tree {
        view.tree = Some(tree.rebuilt(&view.files, &view.filter));
    }
    view.selected = 0;
    // Home is the first unreviewed file — the flat list sinks the ✓ ones —
    // and the tree lands on it too.
    let home = view
        .matches
        .first()
        .map(|m| view.files[m.file].path.clone());
    let kept = before
        .as_ref()
        .filter(|_| moved)
        .is_some_and(|was| view.select_path(&was.path));
    if let (false, Some(path)) = (kept, home) {
        view.select_path(&path);
    }
    // Diffs read against the wrong idea of HEAD (the badge's list cannot
    // say whether there is one) are not worth keeping.
    if head_changed {
        view.cache.clear();
    }
    if head_changed || view.selected_file() != before.as_ref() {
        load_selected_diff(view);
    }
    if let Some(commits) = commits {
        crate::commit_list::install(view, commits);
    }
}

/// Reload `view.diff` for the currently selected file and reset the scroll.
/// A view whose diffs were fetched whole (a pull request) reads them out of
/// `prefetched` instead of shelling out — there is no local commit to ask
/// git about, and the text is already in hand. A directory row of the
/// tree list has no diff of its own: the pane lists what changed under
/// it. Every diff is taken against the COMMIT LIST row on screen
/// (`DiffView::scope`).
///
/// A view with BACKGROUND READS never waits on git here. A file this modal
/// has read before is on screen on this keypress, out of `DiffView::cache`,
/// and re-read behind it — an agent may have edited it since — with the
/// reader's place kept if the text changed. One it has not keeps the last
/// file's text up for `view_jobs::STALE_GRACE` while git runs, which is
/// longer than a diff takes; past that the pane says `loading…`. Either way
/// the answer comes back through [`land_diff`].
pub fn load_selected_diff(view: &mut DiffView) {
    view.waiting = None;
    let Some(file) = view.selected_file().cloned() else {
        let summary = match view.dir_summary() {
            Some(summary) => summary,
            // A row whose list came back empty — an empty commit — says so
            // under its message; a filter that hides every file leaves the
            // pane blank.
            None if view.files.is_empty() && view.listing.is_none() => {
                view.empty_note().to_string()
            }
            None => String::new(),
        };
        view.show_diff(None, summary, false);
        return;
    };
    if let Some(chunks) = &view.prefetched {
        let diff = chunks
            .get(&file.path)
            .cloned()
            .unwrap_or_else(|| "(no diff for this file)".to_string());
        view.show_diff(Some(&file.path), diff, false);
        return;
    }
    let Some(jobs) = view.jobs.clone() else {
        let diff = scoped_diff(&view.root, &view.scope, &file, view.head_ok);
        view.show_diff(Some(&file.path), diff, false);
        return;
    };
    if let Some(text) = view.cached(&file.path) {
        view.show_diff(Some(&file.path), text.to_string(), false);
    }
    let ticket = crate::view_jobs::ticket();
    view.waiting = Some(ticket);
    request_diff(view, &jobs, file, ticket, false);
}

fn request_diff(
    view: &DiffView,
    jobs: &crate::view_jobs::Jobs,
    file: DiffFile,
    ticket: u64,
    prefetch: bool,
) {
    let (root, head_ok, id) = (view.root.clone(), view.head_ok, view.id);
    let scope = view.scope.clone();
    let work = move || {
        Some(crate::view_jobs::Answer::DiffText {
            view: id,
            ticket,
            diff: scoped_diff(&root, &scope, &file, head_ok),
            path: file.path,
            prefetch,
        })
    };
    if prefetch {
        jobs.run(work);
    } else {
        jobs.run_with_grace(ticket, work);
    }
}

/// A background `git diff` came back. It is kept either way — it is the
/// freshest text there is for that file — and shown when it is the one the
/// cursor is waiting on. Then the row after the cursor is read ahead, once:
/// `↓` is the key this modal is walked with, and its next press finds the
/// text in hand.
pub fn land_diff(
    view: &mut DiffView,
    id: u64,
    ticket: u64,
    path: &str,
    diff: String,
    prefetch: bool,
) {
    if id != view.id {
        return;
    }
    view.cache_put(path, &diff);
    if prefetch || view.waiting != Some(ticket) {
        return;
    }
    view.waiting = None;
    let same_file = view.shown.as_deref() == Some(path);
    if !(same_file && view.diff == diff) {
        view.show_diff(Some(path), diff, same_file);
    }
    let next = view
        .file_after_cursor()
        .filter(|file| view.cached(&file.path).is_none())
        .cloned();
    if let (Some(file), Some(jobs)) = (next, view.jobs.clone()) {
        request_diff(view, &jobs, file, crate::view_jobs::ticket(), true);
    }
}

/// The diff in flight has outlasted the grace the last file's text was kept
/// for: say so, rather than leave one file's diff under another's name. A
/// COMMIT LIST row's file list that has is `commit_list::listing_slow`'s.
pub fn diff_slow(view: &mut DiffView, ticket: u64) {
    crate::commit_list::listing_slow(view, ticket);
    if view.waiting == Some(ticket)
        && view.shown.as_deref() != view.selected_file().map(|f| f.path.as_str())
    {
        view.show_diff(None, "loading…".to_string(), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn modified(path: &str) -> DiffFile {
        DiffFile {
            path: path.into(),
            orig_path: None,
            xy: [' ', 'M'],
            lines: None,
        }
    }

    fn listing(paths: &[&str], reviewed: &[&str]) -> crate::view_jobs::DiffListing {
        crate::view_jobs::DiffListing {
            files: paths.iter().map(|p| modified(p)).collect(),
            head: Some("abc123".into()),
            reviewed: reviewed.iter().map(|p| (p.to_string(), 1)).collect(),
            commits: None,
        }
    }

    /// A view opened on the badge's list, as `g` leaves it: files up, the
    /// `git status` that checks them still out.
    fn opened_on(paths: &[&str]) -> DiffView {
        // Not a repository: a diff read here is git's refusal, which is all
        // these tests need it to be.
        let mut view = DiffView::new(
            std::env::temp_dir().join("orion-no-such-checkout"),
            "main".into(),
            paths.iter().map(|p| modified(p)).collect(),
            true,
        );
        view.listing = Some(7);
        view
    }

    fn selected(view: &DiffView) -> Option<&str> {
        view.selected_file().map(|f| f.path.as_str())
    }

    /// The fresh list lands under a reader who has moved: they stay on the
    /// file they were reading, wherever it now sits.
    #[test]
    fn a_reader_who_moved_stays_on_their_file_when_the_fresh_list_lands() {
        let mut view = opened_on(&["a.rs", "b.rs", "c.rs"]);
        view.select(1);
        view.show_diff(Some("b.rs"), "the diff of b".into(), false);

        fill_view(&mut view, listing(&["new.rs", "a.rs", "b.rs", "c.rs"], &[]));
        assert_eq!(view.listing, None);
        assert_eq!(selected(&view), Some("b.rs"), "now the third row");
        assert_eq!(view.diff, "the diff of b", "and it was not read again");
    }

    /// …and if that file is no longer changed, they are put on the first.
    #[test]
    fn a_file_that_left_the_list_hands_the_cursor_to_the_top() {
        let mut view = opened_on(&["a.rs", "b.rs", "c.rs"]);
        view.select(1);
        fill_view(&mut view, listing(&["a.rs", "c.rs"], &[]));
        assert_eq!(selected(&view), Some("a.rs"));
    }

    /// A reader who has not moved gets what a fresh open gives: reviewed ✓
    /// files sunk, the cursor on the first that is not.
    #[test]
    fn an_unmoved_reader_lands_on_the_first_unreviewed_file() {
        let mut view = opened_on(&["a.rs", "b.rs"]);
        fill_view(&mut view, listing(&["a.rs", "b.rs"], &["a.rs"]));
        assert_eq!(selected(&view), Some("b.rs"));
        assert_eq!(view.head_key, "abc123");
    }

    /// The tree list lands a fresh listing the same way the flat one does:
    /// folded over the new files, the reader's folds kept, an unmoved
    /// reader on the first unreviewed file and a moved one on theirs.
    #[test]
    fn the_fresh_list_lands_in_the_tree_the_same_way() {
        let mut view = opened_on(&["src/a.rs", "src/b.rs"]);
        view.toggle_tree();
        fill_view(&mut view, listing(&["src/a.rs", "src/b.rs"], &["src/a.rs"]));
        assert!(view.tree.is_some(), "still the tree");
        assert_eq!(selected(&view), Some("src/b.rs"), "the first unreviewed");

        // The reader is on b.rs; a file that turned up since joins the tree
        // under them.
        fill_view(&mut view, listing(&["new.rs", "src/a.rs", "src/b.rs"], &[]));
        assert_eq!(selected(&view), Some("src/b.rs"));
        assert!(view.select_path("new.rs"), "a row of its own now");
    }

    /// The read-ahead walks the list that is showing: in the tree that
    /// means the next file down, directories stepped over.
    #[test]
    fn the_row_read_ahead_is_the_next_file_of_the_list_on_screen() {
        let mut view = opened_on(&["src/a.rs", "src/b.rs"]);
        assert_eq!(
            view.file_after_cursor().map(|f| f.path.as_str()),
            Some("src/b.rs")
        );

        view.toggle_tree();
        // Up onto the `src/` row: the file after it is still a.rs.
        view.select_path("src");
        assert_eq!(
            view.file_after_cursor().map(|f| f.path.as_str()),
            Some("src/a.rs")
        );
        view.select_path("src/b.rs");
        assert_eq!(view.file_after_cursor(), None, "the end of the list");
    }

    /// The cache holds what the budget allows, newest kept, and never an
    /// entry that is most of the budget by itself.
    #[test]
    fn the_diff_cache_stays_inside_its_budget() {
        let mut view = opened_on(&["a.rs"]);
        let chunk = "x".repeat(crate::app::DIFF_CACHE_ENTRY_MAX);
        for n in 0..8 {
            view.cache_put(&format!("f{n}.rs"), &chunk);
        }
        let held: usize = view.cache.iter().map(|(_, text)| text.len()).sum();
        assert!(held <= crate::app::DIFF_CACHE_BYTES, "{held} bytes held");
        assert!(view.cached("f7.rs").is_some(), "the newest is kept");
        assert!(view.cached("f0.rs").is_none(), "the oldest went first");

        view.cache_put("huge.rs", &"y".repeat(crate::app::DIFF_CACHE_ENTRY_MAX + 1));
        assert!(
            view.cached("huge.rs").is_none(),
            "too big to be worth holding"
        );
    }

    #[test]
    fn cap_lines_keeps_the_head_and_marks_what_it_dropped() {
        assert_eq!(cap_lines("a\nb\nc", 5, false), "a\nb\nc");
        assert_eq!(
            cap_lines("a\nb\nc\n", 3, false),
            "a\nb\nc",
            "trailing newline is not a line"
        );
        assert_eq!(
            cap_lines("a\nb\nc", 2, false),
            format!("a\nb{TRUNCATED_MARK}")
        );
        assert_eq!(
            cap_lines("a\nb", 2, true),
            format!("a\nb{TRUNCATED_MARK}"),
            "byte cap forces the mark"
        );
        assert_eq!(cap_lines("a\nb\nc", 0, false), TRUNCATED_MARK);
        assert_eq!(cap_lines("", 3, false), "");
    }

    #[test]
    fn parse_status_z_handles_all_statuses() {
        let bytes =
            b"M  staged.rs\0 M unstaged.rs\0?? new.txt\0D  gone.rs\0R  renamed.rs\0original.rs\0";
        let files = parse_status_z(bytes);
        assert_eq!(files.len(), 5);
        assert_eq!(files[0].path, "staged.rs");
        assert_eq!(files[0].xy, ['M', ' ']);
        assert_eq!(files[1].xy, [' ', 'M']);
        assert!(files[2].is_untracked());
        assert_eq!(files[2].status_str(), "??");
        assert_eq!(files[3].xy, ['D', ' ']);
        assert_eq!(files[4].path, "renamed.rs");
        assert_eq!(files[4].orig_path.as_deref(), Some("original.rs"));
        assert!(files[0].orig_path.is_none());
    }

    /// A rename's score is dropped and its two paths read original first;
    /// the status letter lands in the first column, as a staged change's.
    #[test]
    fn parse_name_status_z_reads_renames_and_scores() {
        let bytes = b"M\0src/a.rs\0A\0new.rs\0R087\0old.rs\0moved.rs\0D\0gone.rs\0";
        let files = parse_name_status_z(bytes);
        let summary: Vec<(String, Option<&str>, char)> = files
            .iter()
            .map(|f| (f.path.clone(), f.orig_path.as_deref(), f.xy[0]))
            .collect();
        assert_eq!(
            summary,
            [
                ("src/a.rs".to_string(), None, 'M'),
                ("new.rs".to_string(), None, 'A'),
                ("moved.rs".to_string(), Some("old.rs"), 'R'),
                ("gone.rs".to_string(), None, 'D'),
            ]
        );
        assert!(files.iter().all(|f| f.xy[1] == ' '));
        assert!(parse_name_status_z(b"").is_empty());
    }

    /// Against a real checkout: a commit's files and diffs are that commit's
    /// alone, a root commit's are read from the empty tree, and the
    /// branch's run from its merge-base to the working tree, untracked
    /// files and all.
    #[test]
    fn scope_files_and_scoped_diff_from_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        let root_sha = head_oid(&repo).unwrap();
        std::fs::write(repo.join("tracked.txt"), "new line\n").unwrap();
        std::fs::write(repo.join("second.txt"), "two\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "second"]);
        let second = head_oid(&repo).unwrap();
        std::fs::write(repo.join("wip.txt"), "wip\n").unwrap();

        let commit = DiffScope::Commit {
            sha: second,
            parent: Some(root_sha.clone()),
        };
        let files = scope_files(&repo, &commit).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["second.txt", "tracked.txt"]);
        let lines = |added, removed| Some(LineChanges { added, removed });
        assert_eq!(files[0].lines, lines(1, 0), "each file its own counts");
        assert_eq!(files[1].lines, lines(1, 1));
        let diff = scoped_diff(&repo, &commit, &files[1], true);
        assert!(
            diff.contains("-old line") && diff.contains("+new line"),
            "{diff}"
        );

        let root = DiffScope::Commit {
            sha: root_sha.clone(),
            parent: None,
        };
        let files = scope_files(&repo, &root).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].xy[0], 'A');
        assert!(scoped_diff(&repo, &root, &files[0], true).contains("+old line"));

        let branch = DiffScope::Ranges(vec![Range {
            from: RangeStart::Rev(root_sha.clone()),
            to: None,
            label: "the branch".into(),
        }]);
        let files = scope_files(&repo, &branch).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["second.txt", "tracked.txt", "wip.txt"]);
        assert!(files[2].is_untracked());
        assert_eq!(
            files[2].lines,
            lines(1, 0),
            "an untracked file read from disk"
        );
        assert!(scoped_diff(&repo, &branch, &files[2], true).contains("+wip"));
    }

    /// Two ranges with a commit left out between them: the files are both
    /// ranges' together, and a file both touch reads as each range's diff
    /// in turn, behind its label — never the left-out commit's change.
    #[test]
    fn ranges_with_a_gap_show_each_range_in_turn() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        let base = head_oid(&repo).unwrap();
        let step = |text: &str, other: Option<&str>, msg: &str| {
            std::fs::write(repo.join("tracked.txt"), text).unwrap();
            if let Some(other) = other {
                std::fs::write(repo.join(other), "x\n").unwrap();
            }
            git(&repo, &["add", "."]);
            git(&repo, &["commit", "-m", msg]);
            head_oid(&repo).unwrap()
        };
        let one = step("one\n", Some("a.txt"), "one");
        let skipped = step("skipped\n", None, "skipped");
        let three = step("three\n", Some("c.txt"), "three");
        let scope = DiffScope::Ranges(vec![
            Range {
                from: RangeStart::Rev(base),
                to: Some(one),
                label: "first".into(),
            },
            Range {
                from: RangeStart::Rev(skipped),
                to: Some(three),
                label: "second".into(),
            },
        ]);
        let files = scope_files(&repo, &scope).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "c.txt", "tracked.txt"]);
        let tracked = files.iter().find(|f| f.path == "tracked.txt").unwrap();
        let diff = scoped_diff(&repo, &scope, tracked, true);
        let mark = crate::diff_doc::RANGE_MARK;
        let first = diff.find(&format!("{mark}first")).expect("the first range");
        let second = diff
            .find(&format!("{mark}second"))
            .expect("the second range");
        assert!(first < second, "oldest first: {diff}");
        assert!(diff.contains("+one") && diff.contains("+three"), "{diff}");
        assert!(
            !diff.contains("+skipped"),
            "the left-out commit's line never shows as added: {diff}"
        );
        // A file one range touches is that range's diff alone.
        let a = files.iter().find(|f| f.path == "a.txt").unwrap();
        let diff = scoped_diff(&repo, &scope, a, true);
        assert!(!diff.contains(mark) && diff.contains("+x"), "{diff}");
    }

    /// A range from the branch's first commit is measured from where its
    /// end left the base: main merged in on the way adds nothing.
    #[test]
    fn a_range_from_the_merge_base_leaves_out_what_a_merge_brought_in() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        git(&repo, &["checkout", "-q", "-b", "feat"]);
        std::fs::write(repo.join("mine.txt"), "mine\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "mine"]);
        git(&repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("theirs.txt"), "theirs\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "theirs"]);
        git(&repo, &["checkout", "-q", "feat"]);
        git(&repo, &["merge", "-q", "--no-edit", "main"]);
        let scope = DiffScope::Ranges(vec![Range {
            from: RangeStart::MergeBase {
                base: "main".into(),
            },
            to: Some(head_oid(&repo).unwrap()),
            label: "all".into(),
        }]);
        let files = scope_files(&repo, &scope).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["mine.txt"], "theirs.txt came in with the merge");
    }

    fn git(repo: &PathBuf, args: &[&str]) {
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
    }

    fn make_repo(dir: &tempfile::TempDir) -> PathBuf {
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("tracked.txt"), "old line\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        repo
    }

    #[test]
    fn changed_files_and_diff_for_from_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(repo.join("tracked.txt"), "new line\n").unwrap();
        std::fs::write(repo.join("fresh.txt"), "hello\n").unwrap();

        let files = changed_files(&repo).unwrap();
        assert_eq!(files.len(), 2);
        let tracked = files.iter().find(|f| f.path == "tracked.txt").unwrap();
        let fresh = files.iter().find(|f| f.path == "fresh.txt").unwrap();
        assert!(fresh.is_untracked());

        assert!(has_head(&repo));
        let diff = diff_for(&repo, tracked, true);
        assert!(diff.contains("-old line"), "{diff}");
        assert!(diff.contains("+new line"), "{diff}");
        // Untracked goes through the --no-index exit-1 path.
        let diff = diff_for(&repo, fresh, true);
        assert!(diff.contains("+hello"), "{diff}");
    }

    #[test]
    fn list_files_includes_untracked_and_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(repo.join("fresh.txt"), "hello\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(repo.join("ignored.txt"), "nope\n").unwrap();

        let files = list_files(&repo).unwrap();
        assert!(files.contains(&"tracked.txt".to_string()), "{files:?}");
        assert!(files.contains(&"fresh.txt".to_string()), "{files:?}");
        assert!(!files.contains(&"ignored.txt".to_string()), "{files:?}");
    }

    /// Gitignored `.env*` files are still files you open: listed after
    /// the rest, at any depth — never one under `node_modules`, and no
    /// other ignored file.
    #[test]
    fn list_files_keeps_ignored_env_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(
            repo.join(".gitignore"),
            ".env.local\n.env\nnode_modules\nbuild.log\n",
        )
        .unwrap();
        std::fs::write(repo.join(".env.local"), "KEY=1\n").unwrap();
        std::fs::create_dir_all(repo.join("apps/web")).unwrap();
        std::fs::write(repo.join("apps/web/.env"), "KEY=2\n").unwrap();
        std::fs::create_dir_all(repo.join("node_modules/pkg")).unwrap();
        std::fs::write(repo.join("node_modules/pkg/.env"), "KEY=3\n").unwrap();
        std::fs::write(repo.join("build.log"), "noise\n").unwrap();

        let files = list_files(&repo).unwrap();
        assert!(files.contains(&".env.local".to_string()), "{files:?}");
        assert!(files.contains(&"apps/web/.env".to_string()), "{files:?}");
        assert!(
            !files.iter().any(|f| f.contains("node_modules")),
            "{files:?}"
        );
        assert!(!files.contains(&"build.log".to_string()), "{files:?}");
        assert_eq!(files.iter().filter(|f| *f == ".env.local").count(), 1);
    }

    #[test]
    fn list_files_errors_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let err = list_files(dir.path()).unwrap_err();
        assert!(err.contains("git ls-files failed"), "{err}");
    }

    #[test]
    fn head_oid_moves_with_commits() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        let first = head_oid(&repo).expect("committed repo has a HEAD");
        std::fs::write(repo.join("tracked.txt"), "new line\n").unwrap();
        git(&repo, &["commit", "-am", "second"]);
        let second = head_oid(&repo).expect("still has a HEAD");
        assert_ne!(first, second, "a commit moves the OID");
        assert!(head_oid(dir.path()).is_none(), "no repo, no OID");
    }

    #[test]
    fn unborn_head_falls_back_to_plain_diff() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("only.txt"), "content\n").unwrap();

        assert!(!has_head(&repo));
        let files = changed_files(&repo).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].is_untracked());
        let diff = diff_for(&repo, &files[0], false);
        assert!(diff.contains("+content"), "{diff}");
    }

    /// The WORKTREES PANEL badge polls `changed_files` every two seconds on
    /// the checkout the user's agent is working in. A plain `git status`
    /// rewrites the index through `.git/index.lock` whenever its stat cache
    /// is stale, and an agent mid-`git add` or `commit` then fails with
    /// "index.lock: File exists" (issue #15). The poll must read without
    /// ever writing.
    #[test]
    fn changed_files_never_rewrites_the_index() {
        use std::os::unix::fs::MetadataExt;
        use std::time::{Duration, UNIX_EPOCH};
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        let index = repo.join(".git").join("index");
        // A rewrite lands as a rename over the file, so the inode moves;
        // the mtime is the second witness in case a filesystem reuses it.
        let stamp = || {
            let meta = std::fs::metadata(&index).unwrap();
            (meta.ino(), meta.modified().unwrap())
        };
        // Same content, a different (whole-second) mtime: the index's
        // stat cache no longer matches, which is exactly what makes a
        // status want to write the refreshed cache back.
        let stale = |secs: u64| {
            let file = std::fs::File::options()
                .write(true)
                .open(repo.join("tracked.txt"))
                .unwrap();
            file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };

        stale(1_000_000_000);
        let before = stamp();
        changed_files(&repo).unwrap();
        assert_eq!(stamp(), before, "the status poll rewrote the index");

        // The control: with optional locks allowed, the same status does
        // rewrite it — so the assertion above is testing something.
        stale(1_000_000_100);
        let before = stamp();
        let plain = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .env("GIT_OPTIONAL_LOCKS", "1")
            .args(["status", "--porcelain=v1", "-z", "-uall"])
            .output()
            .unwrap();
        assert!(plain.status.success());
        assert_ne!(
            stamp(),
            before,
            "a plain status left the index alone, so this test proves nothing"
        );
    }

    /// Numstat records sum across files; a rename's two path fields are
    /// skipped rather than read as records, and a binary file's `-` adds
    /// nothing.
    #[test]
    fn parse_numstat_z_sums_files_renames_and_binaries() {
        let raw =
            b"3\t1\tsrc/a.rs\x00-\t-\tlogo.png\x0010\t2\t\x00old.rs\x00new.rs\x000\t4\tgone.rs\x00";
        assert_eq!(
            parse_numstat_z(raw),
            LineChanges {
                added: 13,
                removed: 7
            }
        );
        assert!(parse_numstat_z(b"").is_empty());
    }

    #[test]
    fn numstat_by_path_keys_renames_by_their_new_path_and_skips_binaries() {
        let raw =
            b"3\t1\tsrc/a.rs\x00-\t-\tlogo.png\x0010\t2\t\x00old.rs\x00new.rs\x000\t4\tgone.rs\x00";
        let counted = numstat_by_path(raw);
        let lines = |added, removed| Some(LineChanges { added, removed });
        assert_eq!(counted.get("src/a.rs").copied(), lines(3, 1));
        assert_eq!(counted.get("new.rs").copied(), lines(10, 2));
        assert_eq!(counted.get("gone.rs").copied(), lines(0, 4));
        assert_eq!(counted.get("logo.png"), None, "a binary file");
        assert_eq!(counted.len(), 3);
    }

    /// The uncommitted changes' own counts, the way the listing reads them:
    /// a staged edit and an unstaged one against HEAD, an untracked file
    /// from disk.
    #[test]
    fn read_listing_counts_each_uncommitted_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(repo.join("tracked.txt"), "new line\nmore\n").unwrap();
        std::fs::write(repo.join("wip.txt"), "a\nb\nc\n").unwrap();
        let files = read_listing(&repo).unwrap().files;
        let of = |path: &str| files.iter().find(|f| f.path == path).unwrap().lines;
        assert_eq!(
            of("tracked.txt"),
            Some(LineChanges {
                added: 2,
                removed: 1
            })
        );
        assert_eq!(
            of("wip.txt"),
            Some(LineChanges {
                added: 3,
                removed: 0
            })
        );
    }

    #[test]
    fn count_lines_counts_like_numstat() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"one\n"), 1);
        assert_eq!(count_lines(b"one\ntwo"), 2);
        assert_eq!(count_lines(b"a\x00b\n"), 0, "binary");
    }

    /// Against a real checkout: a staged edit, an unstaged one and an
    /// untracked file all count, the way the DIFF VIEWER would show them,
    /// and an unborn HEAD counts its tracked files from nothing.
    #[test]
    fn line_changes_counts_staged_unstaged_and_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = |args: &[&str]| {
            let out = run_git(repo, args).unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "1\n2\n3\n").unwrap();
        git(&["add", "a.txt"]);
        let unborn = changed_files(repo).unwrap();
        assert_eq!(
            line_changes(repo, &unborn),
            Some(LineChanges {
                added: 3,
                removed: 0
            }),
            "no commit yet: every tracked line is new"
        );
        git(&["commit", "-qm", "init"]);

        std::fs::write(repo.join("a.txt"), "1\nTWO\n3\n4\n").unwrap();
        git(&["add", "a.txt"]);
        std::fs::write(repo.join("a.txt"), "1\nTWO\n4\n").unwrap();
        std::fs::write(repo.join("new.txt"), "x\ny").unwrap();
        let files = changed_files(repo).unwrap();
        assert_eq!(
            line_changes(repo, &files),
            Some(LineChanges {
                added: 4,
                removed: 2
            }),
            "HEAD 1 2 3 -> 1 TWO 4 is +2 -2, and new.txt's two lines add"
        );
        assert_eq!(line_changes(repo, &[]), Some(LineChanges::default()));
    }

    #[test]
    fn changed_files_errors_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let err = changed_files(dir.path()).unwrap_err();
        assert!(err.contains("git status failed"), "{err}");
    }
}
