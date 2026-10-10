//! What the pull-request lookups learned, kept on disk between launches.
//!
//! Every answer `gh` gives — each checkout's own pull request (the PR ROW),
//! each project's open list (the PROJECT OPEN PRS GROUP), the body and
//! conversation the pane read, the diff the modal showed — is remembered
//! here, so the next launch paints all of it at once from what the last
//! one knew, and the lookups that would otherwise leave the panels blank
//! for their first seconds run underneath to bring it up to date. Stale for
//! a moment beats empty for a moment: a row that says `merged` in purple
//! before the sweep has confirmed it is still the right row to be looking
//! at.
//!
//! The cache is never the source of truth. GitHub is, and every hydrated
//! entry is re-asked on the same beats a fresh answer would be — the
//! project's list on arrival, the selected checkout on the next tick, its
//! neighbours one per tick, a cached body the moment the cursor rests on
//! its row. What lands replaces what was hydrated; what fails to land
//! leaves it (see `pull_request::Lookup`).
//!
//! Layout, under the DATA DIR (`ORION_DATA_DIR` isolates it for tests and
//! parallel instances, like everything else there):
//!
//! ```text
//! pr-cache/pull-requests.json   rows and lists, one document
//! pr-cache/details/<url>.json   one pull request's page: body, tabs, talk
//! pr-cache/diffs/<url>.diff     one whole `gh pr diff` per pull request
//! ```
//!
//! The document is rewritten whole, atomically, whenever something in it
//! changed — at most once per GIT POLL, and once more on quit. It is
//! small: a line or two per row. A page is tens of kilobytes — nine tenths
//! of what the cache holds — so each lives in a file of its own, written
//! only when that page was read again, and a busy repo's hundred pages are
//! never rewritten because one row's checks moved. Diffs are
//! big and change on their own schedule, so each lives in its own file,
//! read only when `g` asks for it. Pages and diffs are pruned along with
//! the document to the pull requests still on some row.
//!
//! Nothing here touches the disk unless an [`App`] carries a [`PrCache`]:
//! the main loop installs one at startup, the unit tests never do, so no
//! test can read or clobber the real user's cache.

use orion_core::clock::now_secs;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use orion_core::{ProjectId, WorktreeId};
use serde::{Deserialize, Serialize};

use crate::app::{App, OpenPrs, OPEN_PRS_MIN_AGE};
use crate::event_loop::{OPEN_PRS_RECHECK_MIN, OPEN_PRS_REFRESH};
use crate::pull_request::{OpenPr, PrDetail, PullRequest};

/// Directory under the DATA DIR everything below lives in.
const DIR: &str = "pr-cache";
/// The one document: rows, lists and bodies.
const STORE_FILE: &str = "pull-requests.json";
/// Where the per-pull-request diffs go, inside [`DIR`].
const DIFFS_DIR: &str = "diffs";
/// Where the per-pull-request pages go, inside [`DIR`].
const DETAILS_DIR: &str = "details";
/// Bumped when the document's shape changes incompatibly; an older
/// document is ignored rather than half-read. Field additions don't need
/// it — `#[serde(default)]` covers those. 2: an open row's `head` is the
/// checkout's branch (`pull_request::checkout_branch`), not `gh`'s bare
/// `headRefName` — a fork row cached under 1 would launch its PR SESSION
/// into whichever checkout of ours shares the fork branch's name. 3: a
/// body carries the PULL REQUEST PAGE's tabs — files, commits, checks,
/// reviews — and one cached under 2 would paint a pull request with none
/// of them, its counts reading zero rather than loading.
const VERSION: u32 = 3;

/// The document on disk, and the pages beside it. Keyed the way the app
/// keys the same things: checkout rows by worktree id, open lists by
/// project id, bodies by URL.
/// Only *found* pull requests are written for the checkouts — a checkout
/// without one paints the same whether the fact is remembered or not, and
/// remembering it would only stop a new PR from showing up until the
/// backoff expired.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    /// Unix seconds; informational, for anyone reading the file.
    pub saved_at: u64,
    #[serde(default)]
    pub worktrees: HashMap<WorktreeId, PullRequest>,
    #[serde(default)]
    pub projects: HashMap<ProjectId, Vec<OpenPr>>,
    /// Each project's merged tail (`OpenPrs::merged`), beside its open
    /// list.
    #[serde(default)]
    pub merged: HashMap<ProjectId, Vec<OpenPr>>,
    /// The pages, each in a file of its own ([`PrCache::store_detail`]),
    /// never in the document: [`PrCache::load_store`] gathers them here,
    /// and [`PrCache::save_store`] writes the ones here out. Still read
    /// from a document written before they moved, which carried them.
    #[serde(default, skip_serializing)]
    pub details: HashMap<String, PrDetail>,
    /// AUTOFIX's ledger, by pull request URL: the breakage last sent or
    /// dismissed, so a relaunch does not ask about it again.
    #[serde(default)]
    pub autofix: HashMap<String, crate::autofix::Record>,
}

/// Where this instance keeps its cache. Cheap to clone: it is a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCache {
    root: PathBuf,
}

impl PrCache {
    /// A cache rooted at `root` — the tests' way in, with a temp dir.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The real one: `<data dir>/pr-cache`.
    pub fn default_location() -> Self {
        Self::at(orion_core::paths::data_dir().join(DIR))
    }

    fn store_path(&self) -> PathBuf {
        self.root.join(STORE_FILE)
    }

    fn diffs_dir(&self) -> PathBuf {
        self.root.join(DIFFS_DIR)
    }

    fn details_dir(&self) -> PathBuf {
        self.root.join(DETAILS_DIR)
    }

    /// The document and every page beside it, when there is a document
    /// this build can read. A missing, malformed or older-versioned file
    /// is simply no cache.
    ///
    /// A document written before the pages had files of their own carries
    /// them inline: each moves out to its file here, once — a page already
    /// in a file is the newer — and the document is written back without
    /// them.
    pub fn load_store(&self) -> Option<Store> {
        let path = self.store_path();
        let raw = std::fs::read_to_string(&path).ok()?;
        let mut store: Store = match serde_json::from_str(&raw) {
            Ok(store) => store,
            Err(err) => {
                tracing::warn!("ignoring malformed {}: {err}", path.display());
                return None;
            }
        };
        if store.version != VERSION {
            return None;
        }
        let inline = std::mem::replace(&mut store.details, self.load_details());
        if !inline.is_empty() {
            for (url, detail) in inline {
                if store.details.contains_key(&url) {
                    continue;
                }
                if let Err(err) = self.store_detail(&url, &detail) {
                    tracing::warn!("pull-request page not moved to its file: {err}");
                }
                store.details.insert(url, detail);
            }
            if let Err(err) = write_json_atomic(&path, &store) {
                tracing::warn!("pull-request cache not rewritten: {err}");
            }
        }
        Some(store)
    }

    /// Write the document whole, and each of `store`'s pages to its file.
    /// Atomic — a temp file beside it, then a rename — so a crash
    /// mid-write leaves the previous document, not half of the new one.
    /// A flush's `store` holds only the pages read since the last one
    /// ([`snapshot`]), so the rest are not written again.
    pub fn save_store(&self, store: &Store) -> std::io::Result<()> {
        write_json_atomic(&self.store_path(), store)?;
        for (url, detail) in &store.details {
            self.store_detail(url, detail)?;
        }
        Ok(())
    }

    /// Keep `detail` as the page last read for `url`, the URL beside it:
    /// two URLs can sanitise to one file name, and the URL inside is what
    /// [`load_details`](Self::load_details) keys the page by.
    fn store_detail(&self, url: &str, detail: &PrDetail) -> std::io::Result<()> {
        let path = self.details_dir().join(file_name(url, "json"));
        write_json_atomic(&path, &DetailFile { url, detail })
    }

    /// Every page kept, by the URL in its file. One that cannot be read —
    /// half a write a crash left, another build's shape — is no page.
    fn load_details(&self) -> HashMap<String, PrDetail> {
        let Ok(entries) = std::fs::read_dir(self.details_dir()) else {
            return HashMap::new();
        };
        entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|entry| {
                let raw = std::fs::read_to_string(entry.path()).ok()?;
                let file: OwnedDetailFile = serde_json::from_str(&raw).ok()?;
                Some((file.url, file.detail))
            })
            .collect()
    }

    fn diff_path(&self, url: &str) -> PathBuf {
        self.diffs_dir().join(file_name(url, "diff"))
    }

    /// The last diff read for `url`, if one was kept. The file's first line
    /// is the URL it was fetched for, and a file whose line disagrees is
    /// not this pull request's — two URLs can sanitise to one name — so it
    /// is a miss, never someone else's diff.
    pub fn load_diff(&self, url: &str) -> Option<String> {
        let raw = std::fs::read_to_string(self.diff_path(url)).ok()?;
        let (header, diff) = raw.split_once('\n')?;
        (header == url).then(|| diff.to_string())
    }

    /// Keep `diff` as the last one read for `url`. Atomic, like the
    /// document.
    pub fn store_diff(&self, url: &str, diff: &str) -> std::io::Result<()> {
        let path = self.diff_path(url);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = temp_path(&path);
        std::fs::write(&tmp, format!("{url}\n{diff}"))?;
        std::fs::rename(&tmp, &path)
    }

    /// Drop every diff file that isn't one of `live`'s — the pull requests
    /// still on some row — and any temp file a crash left behind. The
    /// directory is orion's own, so anything in it that isn't a live diff
    /// is garbage by definition.
    pub fn prune_diffs(&self, live: &HashSet<String>) {
        prune_dir(&self.diffs_dir(), live, "diff");
    }

    /// [`prune_diffs`](Self::prune_diffs) for the pages: one whose pull
    /// request left every row goes, so the directory holds what is on
    /// screen and never a history of what was.
    pub fn prune_details(&self, live: &HashSet<String>) {
        prune_dir(&self.details_dir(), live, "json");
    }
}

/// A page's file: the URL it was read for, then the page.
#[derive(Serialize)]
struct DetailFile<'a> {
    url: &'a str,
    detail: &'a PrDetail,
}

/// [`DetailFile`] as it is read back.
#[derive(Deserialize)]
struct OwnedDetailFile {
    url: String,
    detail: PrDetail,
}

/// Remove every file in `dir` that isn't the `ext` file of one of
/// `live`'s URLs.
fn prune_dir(dir: &Path, live: &HashSet<String>, ext: &str) {
    let keep: HashSet<String> = live.iter().map(|url| file_name(url, ext)).collect();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !keep.contains(name.to_string_lossy().as_ref()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A file name for what is kept about one pull request — its diff, its
/// page: the URL with everything that isn't a letter or digit turned into
/// `_`. Readable in a directory listing, safe on every filesystem, and
/// unique enough for GitHub's `owner/repo/pull/N` shape — the file says
/// inside which URL it is for anyway ([`PrCache::load_diff`]).
fn file_name(url: &str, ext: &str) -> String {
    let stem: String = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{stem}.{ext}")
}

/// [`file_name`] for a diff.
pub fn diff_file_name(url: &str) -> String {
    file_name(url, "diff")
}

/// Write `value` to `path` as pretty JSON with a trailing newline, creating
/// the parent dir. Atomic — a temp file beside it, then a rename — so a
/// crash mid-write leaves the previous document, not half of the new one.
///
/// Each write has a temp file of its own ([`temp_path`]). Flushes run off
/// the loop, so two can overlap — a GIT POLL's and the one on quit — and
/// with one shared temp name the older snapshot's bytes could be the ones
/// renamed into place after the newer one's.
pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    let tmp = temp_path(path);
    if let Err(err) = std::fs::write(&tmp, &bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// A temp name beside `path` no other write uses: `<name>.<pid>.<n>.tmp`,
/// the process id keeping two instances apart and the counter two writes
/// of the same one.
fn temp_path(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.{}.{n}.tmp", std::process::id()))
}

/// Startup: read the document and paint the app from it. Every hydrated
/// entry is armed to be re-asked as soon as its beat allows — see
/// [`install`].
pub fn hydrate(app: &mut App) {
    let Some(store) = app.pr_cache.as_ref().and_then(PrCache::load_store) else {
        return;
    };
    install(app, store);
}

/// Put a document's contents into the app, without touching anything the
/// app already knows — a lookup that has answered beats a cached copy of
/// the same row.
///
/// * Checkout rows land as-is. Their recheck timers are left unarmed, so
///   the selected checkout is re-asked on the first tick and its
///   neighbours one per tick after — the same first minute a launch with
///   no cache spends, only with the rows already painted.
/// * Open lists land *due*: `at` sits past [`OPEN_PRS_MIN_AGE`] so arriving
///   at the project re-asks at once, and the step is the one a fresh answer
///   of the same shape would have left, so the backoff picks up where the
///   last run's did.
/// * Bodies land marked stale (`pr_detail_stale`): the pane shows them the
///   instant the cursor rests on the row, and the same rest that would
///   have fetched a missing body fetches a fresh copy over the top.
/// * Where each pull request stood goes to `App::prs` as
///   [`Asked::Cached`](crate::fetch::Asked::Cached): drawn at once, and
///   replaced by the first live answer whatever order they land in.
pub fn install(app: &mut App, store: Store) {
    use crate::fetch::Asked;
    use crate::pr_store::PrObservation;
    for pr in store.worktrees.values() {
        app.prs
            .observe(&pr.url, PrObservation::of_lookup(pr), Asked::Cached);
    }
    for pr in store.projects.values().flatten() {
        app.prs
            .observe(&pr.url, PrObservation::of_list_row(pr), Asked::Cached);
    }
    for pr in store.merged.values().flatten() {
        app.prs
            .observe(&pr.url, PrObservation::of_merged_row(pr), Asked::Cached);
    }
    for (url, detail) in &store.details {
        app.prs
            .observe(url, PrObservation::of_detail(detail), Asked::Cached);
    }
    for (worktree, pr) in store.worktrees {
        app.pull_requests.entry(worktree).or_insert(Some(pr));
    }
    let now = std::time::Instant::now();
    let at = now.checked_sub(OPEN_PRS_MIN_AGE).unwrap_or(now);
    let mut merged = store.merged;
    for (project, list) in store.projects {
        if app.open_prs.contains_key(&project) {
            continue;
        }
        let step = if list.is_empty() {
            OPEN_PRS_RECHECK_MIN
        } else {
            OPEN_PRS_REFRESH
        };
        let merged = merged.remove(&project).unwrap_or_default();
        app.open_prs.insert(
            project,
            OpenPrs {
                list,
                merged,
                at,
                due: now,
                step,
            },
        );
    }
    for (url, record) in store.autofix {
        app.autofix.ledger.entry(url).or_insert(record);
    }
    for (url, detail) in store.details {
        if app.pr_detail.contains_key(&url) {
            continue;
        }
        app.pr_detail.insert(url.clone(), detail);
        app.pr_detail_stale.insert(url);
    }
    app.dirty = true;
}

/// The document as the app would write it now, and the pages read since
/// the last flush (`App::pr_detail_unsaved`) — the only ones a flush has
/// to write.
pub fn snapshot(app: &App) -> Store {
    Store {
        version: VERSION,
        saved_at: now_secs(),
        worktrees: app
            .pull_requests
            .iter()
            .filter_map(|(worktree, pr)| Some((worktree.clone(), pr.clone()?)))
            .collect(),
        projects: app
            .open_prs
            .iter()
            .map(|(project, open)| (project.clone(), open.list.clone()))
            .collect(),
        merged: app
            .open_prs
            .iter()
            .filter(|(_, open)| !open.merged.is_empty())
            .map(|(project, open)| (project.clone(), open.merged.clone()))
            .collect(),
        details: app
            .pr_detail_unsaved
            .iter()
            .filter_map(|url| Some((url.clone(), app.pr_detail.get(url)?.clone())))
            .collect(),
        // Only open pull requests' — a merged or closed one never breaks
        // again.
        autofix: {
            let open: HashSet<&str> = app
                .open_prs
                .values()
                .flat_map(|open| open.list.iter().map(|pr| pr.url.as_str()))
                .collect();
            app.autofix
                .ledger
                .iter()
                .filter(|(url, _)| open.contains(url.as_str()))
                .map(|(url, record)| (url.clone(), record.clone()))
                .collect()
        },
    }
}

/// Everything a flush writes: the cache to write to, the document, and the
/// pull requests whose diffs may stay. `None` when nothing changed since
/// the last flush or the app has no cache; either way the dirty flag is
/// taken, so the caller — the loop, off-thread; the quit path, inline —
/// only ever writes once per change.
pub fn take_flush(app: &mut App) -> Option<(PrCache, Store, HashSet<String>)> {
    let autofix = std::mem::take(&mut app.autofix.dirty);
    if !std::mem::take(&mut app.pr_cache_dirty) && !autofix {
        return None;
    }
    let cache = app.pr_cache.clone()?;
    let store = snapshot(app);
    app.pr_detail_unsaved.clear();
    Some((cache, store, app.live_pr_urls()))
}

/// Write a flush out: the document and the pages read since the last
/// one, then the prune of the pages and diffs no row names. Failures are
/// logged, never surfaced — a cache that couldn't be written is a slower
/// next launch, not a broken one.
pub fn write_all(cache: &PrCache, store: &Store, live: &HashSet<String>) {
    if let Err(err) = cache.save_store(store) {
        tracing::warn!("pull-request cache not written: {err}");
    }
    cache.prune_details(live);
    cache.prune_diffs(live);
}

/// Keep the diff just read for `url`, when the app has a cache. Only a
/// diff with files in it is worth keeping: an empty one is a flash, not a
/// modal, and would be re-fetched to find that out anyway.
pub fn remember_diff(app: &App, url: &str, diff: &str) {
    let Some(cache) = &app.pr_cache else {
        return;
    };
    if crate::pull_request::split_unified_diff(diff).is_empty() {
        return;
    }
    if let Err(err) = cache.store_diff(url, diff) {
        tracing::warn!("pull-request diff not cached: {err}");
    }
}

/// The diff last read for `url`, when the app has a cache and kept one.
/// Read on the loop: it is a local file, and `g` is waiting on it.
pub fn recall_diff(app: &App, url: &str) -> Option<String> {
    app.pr_cache.as_ref()?.load_diff(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::{PrComment, STATE_MERGED, STATE_OPEN};

    fn cache() -> (tempfile::TempDir, PrCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = PrCache::at(dir.path().join("pr-cache"));
        (dir, cache)
    }

    fn pr(number: u64, state: &str) -> PullRequest {
        PullRequest {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: format!("PR {number}"),
            answered_state: state.into(),
            answered_draft: false,
            answered: Default::default(),
            activity: vec!["2024-04-25T19:55:42Z".into()],
        }
    }

    fn open(number: u64) -> OpenPr {
        OpenPr {
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/o/r/pull/{number}"),
            answered_draft: number % 2 == 1,
            answered: Default::default(),
            head: format!("head-{number}"),
            mine: false,
            head_sha: String::new(),
            meta: Default::default(),
        }
    }

    /// A row of a project's merged tail.
    fn merged(number: u64) -> OpenPr {
        let mut pr = open(number);
        pr.answered_draft = false;
        pr.meta.merged_at = "2024-04-26T21:44:55Z".into();
        pr
    }

    fn detail(number: u64) -> PrDetail {
        PrDetail {
            number,
            url: format!("https://github.com/o/r/pull/{number}"),
            title: format!("PR {number}"),
            answered_state: STATE_OPEN.into(),
            answered_draft: false,
            answered: Default::default(),
            author: "kate".into(),
            base: "main".into(),
            head: format!("head-{number}"),
            additions: 1,
            deletions: 2,
            changed_files: 3,
            body: "Closes #1\n\nMakes the row.".into(),
            comments: vec![PrComment {
                author: "steiza".into(),
                at: "2024-04-26T21:44:55Z".into(),
                review_state: "APPROVED".into(),
                body: "nice".into(),
            }],
            ..Default::default()
        }
    }

    fn store() -> Store {
        Store {
            version: VERSION,
            saved_at: 1,
            worktrees: [
                (WorktreeId("w1".into()), pr(7, STATE_OPEN)),
                (WorktreeId("w2".into()), pr(8, STATE_MERGED)),
            ]
            .into_iter()
            .collect(),
            projects: [
                (ProjectId("p1".into()), vec![open(7), open(9)]),
                (ProjectId("p2".into()), vec![]),
            ]
            .into_iter()
            .collect(),
            merged: [(ProjectId("p1".into()), vec![merged(5)])]
                .into_iter()
                .collect(),
            details: [(detail(7).url.clone(), detail(7))].into_iter().collect(),
            autofix: [(
                open(7).url,
                crate::autofix::Record {
                    handled: Some(crate::autofix::Fingerprint {
                        sha: "abc".into(),
                        conflicts: true,
                        checks: vec!["unit".into()],
                    }),
                    attempts: 1,
                },
            )]
            .into_iter()
            .collect(),
        }
    }

    /// The document survives the disk byte for byte — ids as JSON keys,
    /// the conversation's timestamps, a merged state, an empty list — and
    /// a missing file is simply no cache.
    #[test]
    fn the_document_round_trips() {
        let (_dir, cache) = cache();
        assert!(cache.load_store().is_none(), "nothing written yet");
        let store = store();
        cache.save_store(&store).unwrap();
        let back = cache.load_store().expect("readable");
        assert_eq!(back, store);
        let leftovers: Vec<_> = std::fs::read_dir(&cache.root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "the temp file was renamed into place");
    }

    /// A document from a build with a different shape, or one that isn't
    /// JSON at all, is ignored rather than half-read.
    #[test]
    fn a_foreign_or_broken_document_is_no_cache() {
        let (_dir, cache) = cache();
        let mut store = store();
        store.version = VERSION + 1;
        cache.save_store(&store).unwrap();
        assert!(cache.load_store().is_none(), "a newer version is not ours");
        std::fs::write(cache.store_path(), "{not json").unwrap();
        assert!(cache.load_store().is_none());
    }

    /// A document written before rows carried their health — the same
    /// version, one field short — reads back with every row healthy, the
    /// field's default, rather than being thrown away for its shape.
    #[test]
    fn a_document_from_before_health_reads_as_healthy() {
        let (_dir, cache) = cache();
        let mut raw = serde_json::to_value(store()).unwrap();
        fn strip(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(map) => {
                    map.remove("health");
                    map.values_mut().for_each(strip);
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
                _ => {}
            }
        }
        strip(&mut raw);
        assert!(!raw.to_string().contains("health"), "{raw}");
        std::fs::create_dir_all(cache.store_path().parent().unwrap()).unwrap();
        std::fs::write(cache.store_path(), raw.to_string()).unwrap();
        let mut rows = store();
        rows.details.clear();
        assert_eq!(cache.load_store().expect("readable"), rows);
    }

    /// The pages are files of their own, never in the document, and a
    /// flush writes only the ones read since the last: a row's checks
    /// moving does not rewrite every page. A page whose pull request left
    /// every row is pruned with it.
    #[test]
    fn pages_are_files_of_their_own_written_when_read() {
        let (_dir, cache) = cache();
        let mut app = App::new();
        app.pr_cache = Some(cache.clone());
        install(&mut app, store());
        let seven = detail(7).url;
        let page = |url: &str| cache.details_dir().join(file_name(url, "json"));

        // Hydrated pages are on disk already: nothing to write.
        app.pr_cache_dirty = true;
        let (_, flush, live) = take_flush(&mut app).expect("dirty");
        assert!(flush.details.is_empty(), "no page was read");
        write_all(&cache, &flush, &live);
        assert!(!page(&seven).exists());
        let document = std::fs::read_to_string(cache.store_path()).unwrap();
        assert!(!document.contains("details"), "{document}");
        assert!(!document.contains("Makes the row"), "{document}");

        // One page is read: that one is written, once.
        app.pr_detail_unsaved.insert(seven.clone());
        app.pr_cache_dirty = true;
        let (_, flush, live) = take_flush(&mut app).expect("dirty");
        assert_eq!(flush.details.keys().collect::<Vec<_>>(), [&seven]);
        assert!(app.pr_detail_unsaved.is_empty(), "spent");
        write_all(&cache, &flush, &live);
        assert!(page(&seven).exists());
        let back = cache.load_store().expect("readable");
        assert_eq!(back.details[&seven], detail(7));

        // Its pull request leaves every row: the page goes with it.
        std::fs::write(cache.details_dir().join("leftover.json.tmp"), "x").unwrap();
        cache.prune_details(&HashSet::new());
        assert_eq!(std::fs::read_dir(cache.details_dir()).unwrap().count(), 0);
        assert!(cache.load_store().expect("readable").details.is_empty());
    }

    /// A document written before the pages had files carries them inline:
    /// the first read moves each to its file — one already there is the
    /// newer — and writes the document back without them.
    #[test]
    fn a_document_with_its_pages_inline_moves_them_out() {
        let (_dir, cache) = cache();
        let mut old = serde_json::to_value(store()).unwrap();
        let mut newer = detail(9);
        newer.body = "read since".into();
        let mut stale = newer.clone();
        stale.body = "the document's".into();
        old["details"] = serde_json::json!({
            detail(7).url: detail(7),
            newer.url.clone(): stale,
        });
        std::fs::create_dir_all(cache.store_path().parent().unwrap()).unwrap();
        std::fs::write(cache.store_path(), old.to_string()).unwrap();
        cache.store_detail(&newer.url, &newer).unwrap();

        let back = cache.load_store().expect("readable");
        assert_eq!(back.details[&detail(7).url], detail(7));
        assert_eq!(back.details[&newer.url].body, "read since");
        let document = std::fs::read_to_string(cache.store_path()).unwrap();
        assert!(!document.contains("Makes the row"), "{document}");
        assert_eq!(cache.load_store().expect("readable"), back, "and stays so");
    }

    /// Hydration paints the rows, arms every list to be re-asked at once
    /// and marks every body stale — and never overwrites what a lookup
    /// already answered.
    #[test]
    fn hydration_paints_rows_and_arms_their_refresh() {
        let mut app = App::new();
        // A lookup that already answered for w1 wins over the cache.
        app.pull_requests
            .insert(WorktreeId("w1".into()), Some(pr(70, STATE_OPEN)));
        install(&mut app, store());

        assert_eq!(
            app.pull_requests[&WorktreeId("w1".into())]
                .as_ref()
                .map(|p| p.number),
            Some(70),
            "the live answer stays"
        );
        assert_eq!(
            app.pull_requests[&WorktreeId("w2".into())]
                .as_ref()
                .map(|p| p.badge()),
            Some("merged"),
            "a cached merged row paints purple from the first frame"
        );
        assert!(
            app.pr_lookup_due(&WorktreeId("w2".into())),
            "a hydrated row is re-asked on its first turn"
        );

        let p1 = ProjectId("p1".into());
        let p2 = ProjectId("p2".into());
        assert_eq!(app.open_prs[&p1].list.len(), 2);
        assert_eq!(app.open_prs[&p1].merged, vec![merged(5)]);
        assert_eq!(
            app.prs.status(&merged(5).url).map(|s| s.word()),
            Some("merged"),
            "a cached merged row is purple from the first frame too"
        );
        assert!(app.open_prs_lookup_due(&p1), "hydrated lists are due");
        assert_eq!(app.open_prs[&p1].step, OPEN_PRS_REFRESH);
        assert_eq!(
            app.open_prs[&p2].step, OPEN_PRS_RECHECK_MIN,
            "an empty list resumes the backoff, not the steady beat"
        );
        assert!(
            app.open_prs[&p1].at + OPEN_PRS_MIN_AGE <= std::time::Instant::now(),
            "old enough that arriving at the project re-asks at once"
        );

        let url = detail(7).url;
        assert!(app.pr_detail.contains_key(&url));
        assert!(app.pr_detail_stale.contains(&url));
        assert!(app.dirty);
    }

    /// What is written is what the app knows, minus the checkouts known to
    /// have no pull request — remembering those would only hide a new one.
    #[test]
    fn the_snapshot_writes_found_rows_only() {
        let mut app = App::new();
        install(&mut app, store());
        app.pull_requests.insert(WorktreeId("w3".into()), None);
        let snap = snapshot(&app);
        assert_eq!(snap.version, VERSION);
        let mut ids: Vec<&str> = snap.worktrees.keys().map(|w| w.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["w1", "w2"]);
        assert_eq!(snap.projects.len(), 2);
        assert_eq!(snap.merged[&ProjectId("p1".into())], vec![merged(5)]);
        assert!(
            snap.details.is_empty(),
            "a hydrated page is on disk already"
        );
        app.pr_detail_unsaved.insert(detail(7).url);
        assert_eq!(snapshot(&app).details.len(), 1, "one read since is not");
    }

    /// A flush is taken once per change, and only by an app with a cache.
    #[test]
    fn a_flush_is_taken_once_per_change() {
        let mut app = App::new();
        app.pr_cache_dirty = true;
        assert!(take_flush(&mut app).is_none(), "no cache, nothing to write");
        assert!(!app.pr_cache_dirty, "but the flag is spent");

        let (_dir, cache) = cache();
        app.pr_cache = Some(cache.clone());
        assert!(take_flush(&mut app).is_none(), "clean");
        app.pr_cache_dirty = true;
        let (to, store, _live) = take_flush(&mut app).expect("dirty");
        assert_eq!(to, cache);
        assert_eq!(store.version, VERSION);
        assert!(take_flush(&mut app).is_none(), "spent");
    }

    /// Diffs come back for their own URL only, and the prune keeps just
    /// the live pull requests' — temp files and strangers included.
    #[test]
    fn diffs_round_trip_per_url_and_prune_to_the_live_set() {
        let (_dir, cache) = cache();
        let seven = "https://github.com/o/r/pull/7";
        let nine = "https://github.com/o/r/pull/9";
        assert!(cache.load_diff(seven).is_none());
        cache.store_diff(seven, "diff --git a/x b/x\n+y\n").unwrap();
        cache.store_diff(nine, "diff --git a/z b/z\n+w\n").unwrap();
        assert_eq!(
            cache.load_diff(seven).as_deref(),
            Some("diff --git a/x b/x\n+y\n")
        );
        // A file whose header names another URL is a miss, not a wrong
        // diff — the sanitised names can collide, the headers can't.
        std::fs::write(
            cache.diff_path("https://github.com/o/r/pull/8"),
            "https://github.com/o_r/pull/8\nnot yours\n",
        )
        .unwrap();
        assert!(cache.load_diff("https://github.com/o/r/pull/8").is_none());
        std::fs::write(cache.diffs_dir().join("leftover.diff.tmp"), "x").unwrap();

        cache.prune_diffs(&[seven.to_string()].into_iter().collect());
        let mut left: Vec<String> = std::fs::read_dir(cache.diffs_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [diff_file_name(seven)]);
        assert!(cache.load_diff(nine).is_none());
    }

    /// The file name is the URL, readable and filesystem-safe.
    #[test]
    fn diff_file_names_are_readable_and_safe() {
        assert_eq!(
            diff_file_name("https://github.com/o/r/pull/42"),
            "github_com_o_r_pull_42.diff"
        );
        assert_eq!(
            diff_file_name("http://ghe.corp/Org-1/re.po/pull/1"),
            "ghe_corp_Org_1_re_po_pull_1.diff"
        );
    }

    /// The app-level helpers are no-ops without a cache, and only keep a
    /// diff that has files in it.
    #[test]
    fn diff_helpers_need_a_cache_and_a_real_diff() {
        let mut app = App::new();
        let url = "https://github.com/o/r/pull/7";
        remember_diff(&app, url, "diff --git a/x b/x\n+y\n");
        assert!(recall_diff(&app, url).is_none(), "no cache, nothing kept");

        let (_dir, cache) = cache();
        app.pr_cache = Some(cache);
        remember_diff(&app, url, "");
        assert!(
            recall_diff(&app, url).is_none(),
            "an empty diff is not kept"
        );
        remember_diff(&app, url, "diff --git a/x b/x\n+y\n");
        assert_eq!(
            recall_diff(&app, url).as_deref(),
            Some("diff --git a/x b/x\n+y\n")
        );
    }
}
