//! What the LINEAR VIEW last read, kept on disk between launches, as
//! `pr_cache` keeps the pull requests: each project's lists — both tabs'
//! issues with the comments read on them, the teams' states and members,
//! who the configured user is — so `⌘L` opens on them at once, the `⌘K`
//! palette finds an issue before any list has landed this run, and the
//! ask that brings them up to date runs underneath (stale while it
//! revalidates).
//!
//! The cache is never the source of truth: Linear is. A list hydrated from
//! here is asked again on the same beats a missing one is — the project's
//! prefetch as it is selected, `⌘L` opening — and what lands replaces it,
//! the comments read on an issue carried over as from any older list
//! (`linear::keep_comments`). One that fails to land leaves it.
//!
//! Layout, under the DATA DIR (`ORION_DATA_DIR` isolates it for tests and
//! parallel instances, like everything else there):
//!
//! ```text
//! linear-cache/issues.json   every project's lists, one document
//! ```
//!
//! The document is rewritten whole, atomically, when a list or a read of
//! comments landed since the last write — at most once per GIT POLL, and
//! once more on quit. Nothing here touches the disk unless an [`App`]
//! carries a [`LinearCache`]: the main loop installs one at startup, the
//! unit tests never do, so no test can read or clobber the real cache.

use std::collections::HashMap;
use std::path::PathBuf;

use orion_core::ProjectId;
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::linear::{LinearIssue, LinearList, LinearState, LinearTeam, LinearUser};

/// Directory under the DATA DIR the document lives in.
const DIR: &str = "linear-cache";
const STORE_FILE: &str = "issues.json";
/// Bumped when the document's shape changes incompatibly; an older one is
/// ignored rather than half-read. Field additions don't need it —
/// `#[serde(default)]` covers those.
const VERSION: u32 = 1;

/// The document on disk.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    /// Unix seconds; informational, for anyone reading the file.
    pub saved_at: u64,
    #[serde(default)]
    pub projects: HashMap<ProjectId, CachedList>,
}

/// One project's [`LinearList`] as the document keeps it. What a list
/// says only for the ask that read it — the issues whose attachments ran
/// past the page — is left out.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedList {
    #[serde(default)]
    pub list: Vec<LinearIssue>,
    #[serde(default)]
    pub states: HashMap<String, Vec<LinearState>>,
    #[serde(default)]
    pub me: Option<LinearUser>,
    #[serde(default)]
    pub members: HashMap<String, Vec<LinearUser>>,
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub more_mine: bool,
    #[serde(default)]
    pub teams: Vec<LinearTeam>,
}

impl CachedList {
    fn of(list: &LinearList) -> Self {
        Self {
            list: list.list.clone(),
            states: list.states.clone(),
            me: list.me.clone(),
            members: list.members.clone(),
            more: list.more,
            more_mine: list.more_mine,
            teams: list.teams.clone(),
        }
    }

    fn into_list(self) -> LinearList {
        LinearList {
            list: self.list,
            states: self.states,
            me: self.me,
            members: self.members,
            more: self.more,
            more_mine: self.more_mine,
            prs_cut: Default::default(),
            teams: self.teams,
        }
    }
}

/// Where this instance keeps its cache. Cheap to clone: it is a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearCache {
    root: PathBuf,
}

impl LinearCache {
    /// A cache rooted at `root` — the tests' way in, with a temp dir.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The real one: `<data dir>/linear-cache`.
    pub fn default_location() -> Self {
        Self::at(orion_core::paths::data_dir().join(DIR))
    }

    fn path(&self) -> PathBuf {
        self.root.join(STORE_FILE)
    }

    /// The document, when there is one this build can read. A missing,
    /// malformed or older-versioned file is simply no cache.
    pub fn load(&self) -> Option<Store> {
        let path = self.path();
        let raw = std::fs::read_to_string(&path).ok()?;
        let store: Store = match serde_json::from_str(&raw) {
            Ok(store) => store,
            Err(err) => {
                tracing::warn!("ignoring malformed {}: {err}", path.display());
                return None;
            }
        };
        (store.version == VERSION).then_some(store)
    }

    /// Write the document whole, atomically ([`crate::pr_cache::write_json_atomic`]).
    pub fn save(&self, store: &Store) -> std::io::Result<()> {
        crate::pr_cache::write_json_atomic(&self.path(), store)
    }
}

/// Startup: every project's lists as the last run left them, for any
/// project the app has no list of yet — a list that has landed beats a
/// cached copy of it.
pub fn hydrate(app: &mut App) {
    let Some(store) = app.linear_cache.as_ref().and_then(LinearCache::load) else {
        return;
    };
    for (project, cached) in store.projects {
        app.linear
            .entry(project)
            .or_insert_with(|| cached.into_list());
    }
    app.dirty = true;
}

/// The document as the app would write it now.
pub fn snapshot(app: &App) -> Store {
    Store {
        version: VERSION,
        saved_at: orion_core::clock::now_secs(),
        projects: app
            .linear
            .iter()
            .map(|(project, list)| (project.clone(), CachedList::of(list)))
            .collect(),
    }
}

/// What a flush writes, and where — `None` when nothing landed since the
/// last one or the app has no cache. The dirty flag is taken either way,
/// so the caller writes once per change.
pub fn take_flush(app: &mut App) -> Option<(LinearCache, Store)> {
    if !std::mem::take(&mut app.linear_cache_dirty) {
        return None;
    }
    let cache = app.linear_cache.clone()?;
    Some((cache, snapshot(app)))
}

/// Write a flush out. A failure is logged, never surfaced: a cache that
/// couldn't be written is a slower next `⌘L`, not a broken one.
pub fn write(cache: &LinearCache, store: &Store) {
    if let Err(err) = cache.save(store) {
        tracing::warn!("Linear cache not written: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linear::tests::issue;

    fn cache() -> (tempfile::TempDir, LinearCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = LinearCache::at(dir.path().join(DIR));
        (dir, cache)
    }

    fn listed() -> LinearList {
        let mut read = issue("2", "ENG-2", "Logout");
        read.comments = vec![crate::linear::IssueComment {
            author: "Sam".into(),
            created_at: "2026-10-06T09:00:00.000Z".into(),
            body: "Still broken.".into(),
        }];
        LinearList {
            list: vec![issue("1", "ENG-1", "Login"), read],
            me: Some(LinearUser {
                id: "u1".into(),
                name: "Ana".into(),
            }),
            more: true,
            prs_cut: ["1".to_string()].into_iter().collect(),
            ..Default::default()
        }
    }

    /// A landed list goes to disk once, comments and all, and the next
    /// launch paints it before any list has landed — never over one that
    /// has.
    #[test]
    fn a_landed_list_comes_back_on_the_next_launch() {
        let (_dir, cache) = cache();
        let project = ProjectId("p1".into());
        let mut app = App::new();
        app.linear_cache = Some(cache.clone());
        app.linear.insert(project.clone(), listed());
        assert!(take_flush(&mut app).is_none(), "nothing landed yet");
        app.linear_cache_dirty = true;
        let (to, store) = take_flush(&mut app).expect("a flush");
        write(&to, &store);
        assert!(take_flush(&mut app).is_none(), "written once");

        let mut next = App::new();
        next.linear_cache = Some(cache.clone());
        hydrate(&mut next);
        let back = &next.linear[&project];
        assert_eq!(back.list, listed().list);
        assert_eq!(back.list[1].comments.len(), 1, "the comments read on it");
        assert_eq!(back.me.as_ref().map(|me| me.name.as_str()), Some("Ana"));
        assert!(back.more);
        assert!(back.prs_cut.is_empty(), "one ask's word, not kept");

        let mut landed = App::new();
        landed.linear_cache = Some(cache);
        landed.linear.insert(
            project.clone(),
            LinearList {
                list: vec![issue("9", "ENG-9", "Fresh")],
                ..Default::default()
            },
        );
        hydrate(&mut landed);
        assert_eq!(landed.linear[&project].list[0].identifier, "ENG-9");
    }

    /// A document from another build's shape, or a broken one, is no
    /// cache; an app with no cache never touches the disk.
    #[test]
    fn an_unreadable_document_is_no_cache() {
        let (_dir, cache) = cache();
        let older = Store {
            version: VERSION + 1,
            ..Default::default()
        };
        cache.save(&older).unwrap();
        assert!(cache.load().is_none());
        std::fs::write(cache.path(), "{ not json").unwrap();
        assert!(cache.load().is_none());

        let mut app = App::new();
        app.linear_cache_dirty = true;
        assert!(take_flush(&mut app).is_none());
    }
}
