//! FOLLOW-UP BUGS: the bugs that came after a factory run's merge, for
//! its record (`runs::RunRecord::follow_ups`) and so for week in review
//! and the retro.
//!
//! - **At merge** ([`merge_landed`], and on every sweep for a merge it
//!   missed), the run's pull request is asked of `gh` once: the commits
//!   it landed as, and the lines it changed on the merged side
//!   (`changed_lines`, from its diff).
//! - **Once a day** ([`tick`], [`EVERY`]), and at launch unless a sweep
//!   ran in the last [`LAUNCH_GAP`], for each run merged in the last
//!   [`DAYS`] days: every later pull request merged into the
//!   same repo whose title starts with fix or bug ([`is_fix_title`]) and
//!   that touches the run's lines is a follow-up. "Touches" is `git
//!   blame` in the project's main checkout: the lines the fix changed,
//!   blamed just before it merged, name one of the run's commits. A fix
//!   whose merge commit the checkout hasn't fetched yet is held to the
//!   recorded ranges instead.
//! - **A ticket reopened** after the merge — Linear's history shows it
//!   leaving a done state for an open one — is a follow-up too.
//!
//! Each follow-up is recorded once, by what it names; a sweep looks over
//! the whole window again, so a sweep missed is caught by the next. A fix
//! found not to touch a run is kept on its record (`checked_fixes`), so
//! its diff isn't asked for again, and each diff is asked once a sweep.
//!
//! A follow-up is a slow fact — a fix lands days after the merge it
//! follows, and week in review reads them weekly — so a sweep a day is
//! plenty. Only runs with something in their record from the last
//! [`RECENT_DAYS`] are swept ([`recent`]): an older run's merge window has
//! closed, and one that never merged would otherwise be asked of `gh` on
//! every sweep forever. With no such run there is nothing to sweep, so
//! nothing is asked at all: the factory's builders switched off long
//! enough leave no recent records, and the sweep stops with them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use orion_core::clock::DAY_MS;
use orion_core::WorktreeId;
use serde_json::Value;

use super::runs::{self, ChangedLines, FollowUp, RunRecord};
use crate::app::App;
use crate::pull_request::rfc3339_ms;

/// How often the sweep runs while orion stays up.
pub(crate) const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// A launch sweeps unless the last sweep — this orion's or an earlier
/// one's, by the stamp ([`stamp_path`]) — is younger than this: an
/// upgrade's reopen, or a quit and relaunch, asks nothing again.
pub(crate) const LAUNCH_GAP: Duration = Duration::from_secs(3 * 60 * 60);
/// How recent a run's record must be to be swept at all ([`recent`]).
pub(crate) const RECENT_DAYS: i64 = 30;
/// How long after its merge a fix counts as a follow-up.
pub(crate) const DAYS: i64 = 14;
const GH_TIMEOUT: Duration = Duration::from_secs(60);

// ---- hooks the app calls ----

/// `App::note_merge_landed`: `worktree`'s pull request just merged.
/// When a run's record names it, the merge is recorded now.
pub(crate) fn merge_landed(app: &App, worktree: &WorktreeId) {
    if crate::oneshot::offline() {
        return;
    }
    let Some(Some(pr)) = app.pull_requests.get(worktree) else {
        return;
    };
    let Some(checkout) = app
        .tree
        .worktrees
        .iter()
        .find(|w| &w.id == worktree)
        .and_then(|w| app.tree.projects.iter().find(|p| p.id == w.project_id))
        .map(|p| p.repo_path.clone())
    else {
        return;
    };
    let url = pr.url.clone();
    tokio::spawn(async move {
        if let Some(record) = runs::all()
            .into_iter()
            .find(|r| r.pr_url.as_ref() == Some(&url))
        {
            record_merge(&record, &url, &checkout).await;
        }
    });
}

/// When the sweep last ran, as this orion knows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Swept {
    /// Whether the stamp has been read yet: until it has, the next sweep
    /// is the launch's.
    read: bool,
    /// The last sweep (epoch ms), this run's or, at launch, the stamp's.
    at_ms: Option<i64>,
}

/// Whether a sweep is due at `now_ms`: at launch, unless the last one ran
/// within [`LAUNCH_GAP`]; after that, [`EVERY`] after the last. A stamp
/// from the future (the clock was set back) is no reason to wait.
fn due(swept: Swept, now_ms: i64) -> bool {
    let gap = match swept.read {
        false => LAUNCH_GAP,
        true => EVERY,
    };
    swept
        .at_ms
        .is_none_or(|at| at > now_ms || now_ms - at >= gap.as_millis() as i64)
}

/// The loop's beat: at launch and then every [`EVERY`] ([`due`]), one
/// sweep off the loop over every project's checkout.
pub(crate) fn tick(app: &mut App) {
    static SWEPT: Mutex<Swept> = Mutex::new(Swept {
        read: false,
        at_ms: None,
    });
    if crate::oneshot::offline() {
        return;
    }
    let now = orion_core::clock::now_ms();
    {
        let mut swept = SWEPT.lock().unwrap_or_else(|e| e.into_inner());
        if !swept.read {
            swept.at_ms = stamp_path().and_then(|path| read_stamp(&path));
        }
        let due_now = due(*swept, now);
        swept.read = true;
        if !due_now {
            return;
        }
        swept.at_ms = Some(now);
    }
    if let Some(path) = stamp_path() {
        write_stamp(&path, now);
    }
    let checkouts: Vec<PathBuf> = app
        .tree
        .projects
        .iter()
        .map(|p| p.repo_path.clone())
        .collect();
    tokio::spawn(sweep(checkouts));
}

/// Where the last sweep's time is kept, so a relaunch knows it:
/// `<data_dir>/automations/follow-ups-swept`, the epoch ms as text. None
/// under test, as the run records are.
fn stamp_path() -> Option<PathBuf> {
    (!cfg!(test)).then(|| crate::automations::state_dir().join("follow-ups-swept"))
}

fn read_stamp(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn write_stamp(path: &Path, at_ms: i64) {
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, at_ms.to_string()));
    if let Err(err) = written {
        tracing::warn!(path = %path.display(), "follow-up sweep stamp not written: {err}");
    }
}

/// Whether `record` is worth a sweep at `now_ms`: something in it — a
/// stage, the merge, a follow-up — happened in the last [`RECENT_DAYS`].
fn recent(record: &RunRecord, now_ms: i64) -> bool {
    now_ms - record.last_at_ms() <= RECENT_DAYS * DAY_MS
}

// ---- the sweep ----

/// One sweep: the merges not yet recorded, then each recent merge's
/// follow-ups. Nothing at all when no run is [`recent`].
async fn sweep(checkouts: Vec<PathBuf>) {
    let now = orion_core::clock::now_ms();
    let records: Vec<RunRecord> = runs::all().into_iter().filter(|r| recent(r, now)).collect();
    if records.is_empty() {
        return;
    }
    let repos: Vec<(PathBuf, String)> = checkouts
        .into_iter()
        .filter_map(|dir| {
            let web = crate::remote::repo_url(&dir).ok()?;
            Some((dir, web.to_ascii_lowercase()))
        })
        .collect();
    // The runs merged lately, each with its checkout.
    let mut recent: Vec<(RunRecord, PathBuf, i64)> = Vec::new();
    for record in records {
        let Some(url) = record.pr_url.clone() else {
            continue;
        };
        let Some(dir) = repo_of(&repos, &url) else {
            continue;
        };
        // Asked until `gh` has said what it landed as; a pull request
        // closed unmerged is not asked again.
        let record = match record.merge_commits.is_empty() {
            true if closed_unmerged(&url) => continue,
            true => match record_merge(&record, &url, &dir).await {
                Some(record) => record,
                None => continue,
            },
            false => record,
        };
        // A day over the window, so the last sweep inside it is not missed.
        match record.merged_at_ms {
            Some(merged) if now - merged <= (DAYS + 1) * DAY_MS => {
                recent.push((record, dir, merged))
            }
            _ => {}
        }
    }
    // Each repo's fixes, asked once from its earliest recent merge on.
    let mut fixes: HashMap<PathBuf, Vec<MergedFix>> = HashMap::new();
    for (_, dir, merged) in &recent {
        if fixes.contains_key(dir) {
            continue;
        }
        let earliest = recent
            .iter()
            .filter(|(_, d, _)| d == dir)
            .map(|(_, _, at)| *at)
            .min()
            .unwrap_or(*merged);
        fixes.insert(dir.clone(), merged_fixes(dir, earliest).await);
    }
    let mut diffs = Diffs::new();
    for (record, dir, merged) in recent {
        let mut found = Vec::new();
        let mut checked = Vec::new();
        for fix in fixes.get(&dir).into_iter().flatten() {
            let what = fix.what();
            let new = record.pr_url.as_deref() != Some(fix.url.as_str())
                && in_window(fix.merged_at_ms, merged)
                && !record.follow_ups.iter().any(|f| f.what == what)
                && !record.checked_fixes.contains(&fix.url);
            if !new {
                continue;
            }
            let Some(files) = fix_diff(&mut diffs, &dir, &fix.url).await else {
                continue;
            };
            match touches(&dir, fix, files, &record).await {
                Some(true) => found.push(FollowUp {
                    what,
                    at_ms: fix.merged_at_ms,
                }),
                Some(false) => checked.push(fix.url.clone()),
                None => {}
            }
        }
        if let Some(reopened) = reopened(&dir, &record, merged).await {
            found.push(reopened);
        }
        if !found.is_empty() || !checked.is_empty() {
            runs::update(&record.identifier, |r| {
                for f in found {
                    if !r.follow_ups.iter().any(|old| old.what == f.what) {
                        r.follow_ups.push(f);
                    }
                }
                for url in checked {
                    if !r.checked_fixes.contains(&url) {
                        r.checked_fixes.push(url);
                    }
                }
            });
        }
    }
}

/// Each later fix's diff, asked of `gh` once a sweep: its pull request's
/// URL → its files, None when `gh` couldn't say.
type Diffs = HashMap<String, Option<Vec<FileChange>>>;

/// The files the fix at `url` changed, from `diffs` or asked of `gh`.
async fn fix_diff<'a>(diffs: &'a mut Diffs, dir: &Path, url: &str) -> Option<&'a [FileChange]> {
    if !diffs.contains_key(url) {
        let diff = crate::pull_request::gh(Some(dir), &["pr", "diff", url], GH_TIMEOUT).await;
        diffs.insert(url.to_string(), diff.map(|d| parse_diff(&d)));
    }
    diffs.get(url)?.as_deref()
}

/// The pull requests `gh` said closed without merging, this run of orion.
static CLOSED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn closed_unmerged(url: &str) -> bool {
    CLOSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|u| u == url)
}

/// The checkout whose GitHub repo `url` is a pull request of.
fn repo_of(repos: &[(PathBuf, String)], url: &str) -> Option<PathBuf> {
    let url = url.to_ascii_lowercase();
    repos
        .iter()
        .find(|(_, web)| url.starts_with(&format!("{web}/pull/")))
        .map(|(dir, _)| dir.clone())
}

/// Whether a fix merged at `at` falls in the [`DAYS`] after `merged`.
fn in_window(at: i64, merged: i64) -> bool {
    at > merged && at - merged <= DAYS * DAY_MS
}

/// Whether a pull request's title reads as a bug fix: its first word is
/// fix or bug (`fix:`, `fix(auth):`, `Fix the…`, `Bugfix`, `hotfix`).
pub(crate) fn is_fix_title(title: &str) -> bool {
    let first = title
        .trim()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "fix" | "fixes" | "fixed" | "fixing" | "bug" | "bugfix" | "hotfix"
    )
}

// ---- at merge ----

/// Ask `gh` how the pull request at `url` merged, and keep its commits
/// and changed lines on `record`. The record as it is now; None when it
/// hasn't merged or `gh` couldn't say — and one closed unmerged is
/// remembered as such ([`closed_unmerged`]). A record with no ticket
/// title takes the pull request's.
async fn record_merge(record: &RunRecord, url: &str, checkout: &Path) -> Option<RunRecord> {
    let view = crate::pull_request::gh(
        Some(checkout),
        &[
            "pr",
            "view",
            url,
            "--json",
            "state,title,mergedAt,mergeCommit,commits",
        ],
        GH_TIMEOUT,
    )
    .await?;
    let Some(merge) = parse_merge(&view) else {
        if is_closed(&view) {
            CLOSED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(url.to_string());
        }
        return None;
    };
    let diff = crate::pull_request::gh(Some(checkout), &["pr", "diff", url], GH_TIMEOUT).await?;
    let changed: Vec<ChangedLines> = parse_diff(&diff)
        .into_iter()
        .filter(|f| !f.new.is_empty())
        .map(|f| ChangedLines {
            path: f.path,
            ranges: f.new,
        })
        .collect();
    let checkout = checkout.to_path_buf();
    runs::update(&record.identifier, |r| {
        if r.title.trim().is_empty() {
            r.title = merge.title;
        }
        r.merged_at_ms.get_or_insert(merge.at_ms);
        r.checkout = Some(checkout);
        r.merge_commits = merge.commits;
        r.changed_lines = changed;
    });
    runs::read(&record.identifier)
}

/// How a pull request merged: its title, when, and the commits it
/// landed as.
#[derive(Debug, PartialEq)]
struct Merge {
    title: String,
    at_ms: i64,
    commits: Vec<String>,
}

/// `gh pr view --json state,title,mergedAt,mergeCommit,commits`, for a
/// merged pull request.
fn parse_merge(json: &str) -> Option<Merge> {
    let v: Value = serde_json::from_str(json).ok()?;
    if v["state"].as_str() != Some("MERGED") {
        return None;
    }
    let at_ms = rfc3339_ms(v["mergedAt"].as_str()?)?;
    let merge = v["mergeCommit"]["oid"].as_str().map(String::from);
    let own = v["commits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["oid"].as_str().map(String::from));
    Some(Merge {
        title: v["title"].as_str().unwrap_or_default().to_string(),
        at_ms,
        commits: merge.into_iter().chain(own).collect(),
    })
}

/// Whether `gh pr view --json state`'s answer is a pull request closed
/// without merging.
fn is_closed(json: &str) -> bool {
    serde_json::from_str::<Value>(json).is_ok_and(|v| v["state"].as_str() == Some("CLOSED"))
}

// ---- diffs ----

/// One file of a unified diff: its path on each side, and the lines it
/// changed on each — removed or rewritten on the old side (or, for a
/// pure insertion, the line it went in after), added on the new.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FileChange {
    pub path: String,
    pub old_path: String,
    pub old: Vec<(u32, u32)>,
    pub new: Vec<(u32, u32)>,
}

/// The files of a unified diff (`gh pr diff`), each with its changed lines.
pub(crate) fn parse_diff(diff: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let (mut old_at, mut new_at) = (0u32, 0u32);
    // Past a file's first hunk header, `---` and `+++` are lines.
    let mut in_hunks = false;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            files.push(FileChange::default());
            in_hunks = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if let Some(head) = line.strip_prefix("@@ ") {
            if let Some((old, new)) = parse_hunk_head(head) {
                (old_at, new_at) = (old, new);
                in_hunks = true;
            }
        } else if !in_hunks {
            if let Some(path) = line.strip_prefix("--- ") {
                file.old_path = side_path(path, "a/");
            } else if let Some(path) = line.strip_prefix("+++ ") {
                file.path = side_path(path, "b/");
                if file.path.is_empty() {
                    file.path = file.old_path.clone();
                }
            }
        } else if line.starts_with('+') {
            push_line(&mut file.new, new_at);
            // Nothing old went: the line it was put in after.
            push_line(&mut file.old, old_at.saturating_sub(1).max(1));
            new_at += 1;
        } else if line.starts_with('-') {
            push_line(&mut file.old, old_at);
            old_at += 1;
        } else if line.starts_with(' ') {
            old_at += 1;
            new_at += 1;
        }
    }
    files
}

/// `a/src/x.rs` → `src/x.rs`; `/dev/null` → empty.
fn side_path(path: &str, prefix: &str) -> String {
    let path = path.split('\t').next().unwrap_or_default().trim();
    match path {
        "/dev/null" => String::new(),
        _ => path.strip_prefix(prefix).unwrap_or(path).to_string(),
    }
}

/// `-12,4 +12,6 @@ fn x()` → the first old and new line numbers.
fn parse_hunk_head(head: &str) -> Option<(u32, u32)> {
    let mut parts = head.split_whitespace();
    let start = |part: &str, sign: char| -> Option<u32> {
        part.strip_prefix(sign)?.split(',').next()?.parse().ok()
    };
    let old = start(parts.next()?, '-')?;
    let new = start(parts.next()?, '+')?;
    Some((old, new))
}

/// Add line `n` to `ranges`, growing the last run when it follows on.
fn push_line(ranges: &mut Vec<(u32, u32)>, n: u32) {
    match ranges.last_mut() {
        Some((_, end)) if (*end..=*end + 1).contains(&n) => *end = n,
        Some((start, end)) if (*start..=*end).contains(&n) => {}
        _ => ranges.push((n, n)),
    }
}

fn overlaps(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    a.iter()
        .any(|(s, e)| b.iter().any(|(bs, be)| s <= be && bs <= e))
}

// ---- the later fixes ----

/// A pull request merged after the run's, titled as a bug fix.
#[derive(Debug, Clone, PartialEq)]
struct MergedFix {
    number: u64,
    title: String,
    url: String,
    merged_at_ms: i64,
    merge_commit: Option<String>,
}

impl MergedFix {
    /// What its follow-up says: `#455 Fix the login loop again`.
    fn what(&self) -> String {
        format!("#{} {}", self.number, self.title)
    }
}

/// The fix-titled pull requests merged into the checkout's repo since
/// `since_ms`.
async fn merged_fixes(dir: &Path, since_ms: i64) -> Vec<MergedFix> {
    let Some(since) = chrono::DateTime::from_timestamp_millis(since_ms) else {
        return Vec::new();
    };
    let search = format!("merged:>={}", since.format("%Y-%m-%d"));
    let out = crate::pull_request::gh(
        Some(dir),
        &[
            "pr",
            "list",
            "--state",
            "merged",
            "--search",
            &search,
            "--limit",
            "200",
            "--json",
            "number,title,url,mergedAt,mergeCommit",
        ],
        GH_TIMEOUT,
    )
    .await;
    out.map(|json| parse_fixes(&json)).unwrap_or_default()
}

fn parse_fixes(json: &str) -> Vec<MergedFix> {
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let title = row["title"].as_str()?.to_string();
            if !is_fix_title(&title) {
                return None;
            }
            Some(MergedFix {
                number: row["number"].as_u64()?,
                title,
                url: row["url"].as_str()?.to_string(),
                merged_at_ms: rfc3339_ms(row["mergedAt"].as_str()?)?,
                merge_commit: row["mergeCommit"]["oid"].as_str().map(String::from),
            })
        })
        .collect()
}

/// Whether `fix`, which changed `files`, changed lines `record`'s pull
/// request changed: by blame on the checkout just before the fix merged,
/// else — the fix's merge commit not fetched there — by the recorded
/// ranges. None when only the ranges answered, and said no: blame may
/// say otherwise once the merge is fetched.
async fn touches(
    dir: &Path,
    fix: &MergedFix,
    files: &[FileChange],
    record: &RunRecord,
) -> Option<bool> {
    let ours: Vec<&str> = record
        .changed_lines
        .iter()
        .map(|c| c.path.as_str())
        .collect();
    let theirs: Vec<FileChange> = files
        .iter()
        .filter(|f| !f.old_path.is_empty() && ours.contains(&f.old_path.as_str()))
        .cloned()
        .collect();
    if theirs.is_empty() {
        return Some(false);
    }
    let (dir, merge, commits) = (
        dir.to_path_buf(),
        fix.merge_commit.clone(),
        record.merge_commits.clone(),
    );
    let changed = record.changed_lines.clone();
    tokio::task::spawn_blocking(move || {
        let fetched = merge
            .as_deref()
            .filter(|sha| crate::commit_list::has_commit(&dir, sha));
        match fetched {
            Some(sha) => Some(
                theirs
                    .iter()
                    .any(|f| blamed(&dir, sha, f).iter().any(|c| commits.contains(c))),
            ),
            None => theirs
                .iter()
                .any(|f| {
                    changed
                        .iter()
                        .any(|c| c.path == f.old_path && overlaps(&c.ranges, &f.old))
                })
                .then_some(true),
        }
    })
    .await
    .unwrap_or(None)
}

/// The commits that last wrote the lines `file` changed, as they stood
/// just before `merge`.
fn blamed(dir: &Path, merge: &str, file: &FileChange) -> Vec<String> {
    let parent = format!("{merge}^");
    let mut found = Vec::new();
    for (start, end) in &file.old {
        let range = format!("{start},{end}");
        let Ok(out) = crate::git_proc::read(
            dir,
            &[
                "blame",
                "--porcelain",
                "-L",
                &range,
                &parent,
                "--",
                &file.old_path,
            ],
        ) else {
            continue;
        };
        found.extend(blame_commits(&out));
    }
    found.sort();
    found.dedup();
    found
}

/// The commits `git blame --porcelain` names: each line's header starts
/// with its 40-character sha.
fn blame_commits(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter_map(|line| line.split(' ').next())
        .filter(|word| word.len() == 40 && word.chars().all(|c| c.is_ascii_hexdigit()))
        .map(String::from)
        .collect()
}

// ---- the ticket reopened ----

const HISTORY_QUERY: &str = "query($id: String!) { issue(id: $id) { history(first: 100) { \
    nodes { createdAt fromState { type } toState { type } } } } }";

/// The run's ticket reopened after `merged`: Linear's history shows it
/// leaving a done state for an open one.
async fn reopened(dir: &Path, record: &RunRecord, merged: i64) -> Option<FollowUp> {
    let key = crate::linear::read_linear_key(dir)?;
    let vars = serde_json::json!({ "id": record.identifier });
    let answer = crate::linear::graphql(&key, HISTORY_QUERY, vars)
        .await
        .ok()?;
    let at_ms = reopened_at(&answer, merged)?;
    Some(FollowUp {
        what: format!("{} reopened", record.identifier),
        at_ms,
    })
}

/// When Linear's issue history first shows it reopened after `merged`.
fn reopened_at(answer: &Value, merged: i64) -> Option<i64> {
    let closed = |t: &Value| matches!(t["type"].as_str(), Some("completed" | "canceled"));
    let nodes = answer
        .pointer("/data/issue/history/nodes")
        .or_else(|| answer.pointer("/issue/history/nodes"))?
        .as_array()?;
    nodes
        .iter()
        .filter(|n| closed(&n["fromState"]) && !n["toState"].is_null() && !closed(&n["toState"]))
        .filter_map(|n| rfc3339_ms(n["createdAt"].as_str()?))
        .filter(|at| in_window(*at, merged))
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/src/login.rs b/src/login.rs
index 1..2 100644
--- a/src/login.rs
+++ b/src/login.rs
@@ -10,5 +10,6 @@ fn login()
 a
-b
+B
+C
 d
 e
@@ -40,3 +41,3 @@ fn out()
 x
-y
+Y
 z
diff --git a/src/new.rs b/src/new.rs
new file mode 100644
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,2 @@
+one
+two
";

    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn after_launch(at_ms: i64) -> Swept {
        Swept {
            read: true,
            at_ms: Some(at_ms),
        }
    }

    #[test]
    fn a_launch_sweeps_unless_one_ran_in_the_last_three_hours() {
        let now = 1_000 * HOUR_MS;
        assert!(due(Swept::default(), now), "never swept");
        let stamped = |at_ms| Swept {
            read: false,
            at_ms: Some(at_ms),
        };
        assert!(!due(stamped(now - 2 * HOUR_MS), now), "a quick relaunch");
        assert!(due(stamped(now - 3 * HOUR_MS), now));
        assert!(due(stamped(now + HOUR_MS), now), "the clock set back");
    }

    #[test]
    fn after_launch_the_sweep_is_daily() {
        let now = 1_000 * HOUR_MS;
        assert!(!due(after_launch(now - 3 * HOUR_MS), now));
        assert!(!due(after_launch(now - 23 * HOUR_MS), now));
        assert!(due(after_launch(now - 24 * HOUR_MS), now));
    }

    #[test]
    fn the_stamp_survives_a_relaunch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automations").join("follow-ups-swept");
        assert_eq!(read_stamp(&path), None);
        write_stamp(&path, 1_234);
        assert_eq!(read_stamp(&path), Some(1_234));
    }

    #[test]
    fn only_a_run_active_in_the_last_thirty_days_is_swept() {
        let now = 100 * DAY_MS;
        let merged_at = |at| RunRecord {
            merged_at_ms: Some(at),
            ..Default::default()
        };
        assert!(recent(&merged_at(now - 30 * DAY_MS), now));
        assert!(!recent(&merged_at(now - 31 * DAY_MS), now));
        let followed = RunRecord {
            follow_ups: vec![FollowUp {
                what: "#5 fix".into(),
                at_ms: now - DAY_MS,
            }],
            ..merged_at(now - 40 * DAY_MS)
        };
        assert!(recent(&followed, now), "its latest moment counts");
        assert!(!recent(&RunRecord::default(), now), "a record of nothing");
    }

    #[test]
    fn a_diff_gives_each_files_changed_lines_on_both_sides() {
        let files = parse_diff(DIFF);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/login.rs");
        assert_eq!(files[0].old_path, "src/login.rs");
        assert_eq!(files[0].new, vec![(11, 12), (42, 42)]);
        assert_eq!(files[0].old, vec![(11, 11), (41, 41)]);
        assert_eq!(files[1].path, "src/new.rs");
        assert_eq!(files[1].old_path, "");
        assert_eq!(files[1].new, vec![(1, 2)]);
    }

    #[test]
    fn only_a_title_that_starts_with_fix_or_bug_is_a_fix() {
        for title in [
            "fix: login loop",
            "Fix(auth): x",
            "Bugfix for y",
            "bug: z",
            "hotfix x",
        ] {
            assert!(is_fix_title(title), "{title}");
        }
        for title in [
            "Fixture cleanup",
            "Add login",
            "feat: fix later",
            "Debug panel",
        ] {
            assert!(!is_fix_title(title), "{title}");
        }
    }

    #[test]
    fn a_merge_is_read_from_gh() {
        let json = r#"{"state":"MERGED","mergedAt":"2026-11-10T12:00:00Z",
            "mergeCommit":{"oid":"aaa"},"commits":[{"oid":"bbb"},{"oid":"ccc"}]}"#;
        let merge = parse_merge(json).unwrap();
        assert_eq!(merge.commits, ["aaa", "bbb", "ccc"]);
        assert_eq!(merge.at_ms, rfc3339_ms("2026-11-10T12:00:00Z").unwrap());
        assert_eq!(parse_merge(r#"{"state":"OPEN"}"#), None);
        assert!(is_closed(r#"{"state":"CLOSED"}"#));
        assert!(!is_closed(r#"{"state":"OPEN"}"#));
    }

    #[test]
    fn the_list_keeps_only_fixes() {
        let json = r#"[
            {"number":5,"title":"fix: login again","url":"u5","mergedAt":"2026-11-11T00:00:00Z","mergeCommit":{"oid":"f5"}},
            {"number":6,"title":"Add export","url":"u6","mergedAt":"2026-11-11T00:00:00Z","mergeCommit":{"oid":"f6"}}]"#;
        let fixes = parse_fixes(json);
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].what(), "#5 fix: login again");
        assert_eq!(fixes[0].merge_commit.as_deref(), Some("f5"));
    }

    #[test]
    fn blame_names_each_lines_commit() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let porcelain = format!("{sha} 11 11 1\nauthor Ada\n\tb\n{sha} 12 12\n\tc\n");
        assert_eq!(
            blame_commits(&porcelain),
            vec![sha.to_string(), sha.to_string()]
        );
    }

    #[test]
    fn a_reopen_after_the_merge_and_within_the_window_counts() {
        let merged = rfc3339_ms("2026-11-01T00:00:00Z").unwrap();
        let answer = serde_json::json!({"data": {"issue": {"history": {"nodes": [
            {"createdAt": "2026-10-30T00:00:00Z", "fromState": {"type": "completed"}, "toState": {"type": "started"}},
            {"createdAt": "2026-11-03T00:00:00Z", "fromState": {"type": "started"}, "toState": {"type": "completed"}},
            {"createdAt": "2026-11-05T00:00:00Z", "fromState": {"type": "completed"}, "toState": {"type": "unstarted"}},
            {"createdAt": "2026-11-30T00:00:00Z", "fromState": {"type": "completed"}, "toState": {"type": "started"}}
        ]}}}});
        assert_eq!(
            reopened_at(&answer, merged),
            rfc3339_ms("2026-11-05T00:00:00Z")
        );
    }

    #[test]
    fn a_pull_request_finds_its_checkout() {
        let repos = vec![(PathBuf::from("/r"), "https://github.com/o/r".to_string())];
        assert_eq!(
            repo_of(&repos, "https://github.com/O/R/pull/7"),
            Some(PathBuf::from("/r"))
        );
        assert_eq!(repo_of(&repos, "https://github.com/o/rr/pull/7"), None);
    }

    #[test]
    fn ranges_that_share_a_line_overlap() {
        assert!(overlaps(&[(10, 12)], &[(12, 20)]));
        assert!(!overlaps(&[(10, 12)], &[(13, 20)]));
    }
}
