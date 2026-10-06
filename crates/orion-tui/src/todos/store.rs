//! The TODOS file: one project's list, on this machine only —
//! `<data dir>/todos/<fnv64 of the repo path>.json`, never in the repo.
//! Read whole when the modal opens, written whole after every change,
//! off the loop (`write_json_atomic`, so a crash mid-write never leaves
//! half a list) by one writer that always writes the newest copy of each
//! list: a slow write can never land after a later one. A file that does
//! not parse is moved aside to `<name>.corrupt-<secs>.json` and the modal
//! says so — never written over. One that cannot be read at all, or not
//! moved aside, makes the list READ-ONLY for the session: shown, edited
//! in memory, never saved over what is on disk.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

/// The file format's version: what [`TodoFile::version`] says.
pub const VERSION: u32 = 1;

/// One project's list: its groups, in the order they were made or
/// imported, and its items, in the order they were made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoFile {
    #[serde(default)]
    pub version: u32,
    /// The project's checkout, so a file can be told apart by hand.
    #[serde(default)]
    pub repo_path: PathBuf,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub items: Vec<Item>,
    /// The Linear team **Create in Triage** files into, once picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear_team: Option<String>,
    /// The next id to hand out, past every one ever handed out: a
    /// deleted item's id is never reused, so nothing that remembered it
    /// (an agent's Ack) lands on another item.
    #[serde(default)]
    pub next_id: u64,
    /// The file on disk could not be read, or not moved aside: nothing
    /// is saved over it this session.
    #[serde(skip)]
    pub read_only: bool,
}

/// A section of the list — `Emails`, `MCP fixes › storyline prompt` —
/// nested under `parent`, folded away while `collapsed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: u64,
    #[serde(default)]
    pub parent: Option<u64>,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
}

/// One thing to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub group: u64,
    pub text: String,
    /// Linear's scale: 0 none, 1 urgent, 2 high, 3 medium, 4 low — so an
    /// issue made from it carries the same.
    #[serde(default)]
    pub priority: u8,
    /// The local day it was written down: how long it has carried over.
    pub created: NaiveDate,
    /// When it was ticked, local time: struck through for the rest of
    /// that day, in the log after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<DateTime<Local>>,
    /// The Linear issue it is linked to, `RIP-412`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear: Option<String>,
    /// The agent session sent at it (`AgentId`), while there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The kind of state (`started`, `completed`, …) the linked issue was
    /// last seen in: it ticks the item only on the way into done, so an
    /// untick sticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear_seen: Option<String>,
}

impl TodoFile {
    /// An empty list for the checkout at `repo`.
    pub fn new(repo: &Path) -> Self {
        Self {
            version: VERSION,
            repo_path: repo.to_path_buf(),
            ..Self::default()
        }
    }
}

/// Where `repo`'s list lives. None in the unit tests unless one names a
/// folder ([`with_dir`]): the many that open the modal must never write
/// to the real data dir.
pub fn path_for(repo: &Path) -> Option<PathBuf> {
    let name = format!(
        "{:016x}.json",
        crate::review::fingerprint(&repo.to_string_lossy())
    );
    Some(dir()?.join(name))
}

fn dir() -> Option<PathBuf> {
    #[cfg(test)]
    let dir = DIR_OVERRIDE.with(|d| d.borrow().clone());
    #[cfg(not(test))]
    let dir = Some(orion_core::paths::data_dir().join("todos"));
    dir
}

#[cfg(test)]
thread_local! {
    static DIR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with the lists kept in `dir`.
#[cfg(test)]
pub fn with_dir<T>(dir: PathBuf, f: impl FnOnce() -> T) -> T {
    DIR_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(dir));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// A list as [`load`] found it, and what to say if that was not simply
/// a file read.
#[derive(Debug)]
pub struct Loaded {
    pub file: TodoFile,
    pub problem: Option<String>,
}

/// `repo`'s list as the file has it: an empty list when there is no file
/// (or nowhere to keep one).
pub fn load(repo: &Path) -> Loaded {
    match path_for(repo) {
        Some(path) => load_from(&path, repo, orion_core::clock::now_secs()),
        None => Loaded {
            file: TodoFile::new(repo),
            problem: None,
        },
    }
}

/// [`load`] from `path`. Only a missing file is an empty list: one that
/// does not parse is moved aside (under `now`, secs) and the list starts
/// afresh; one that cannot be read — or moved — leaves the list READ-ONLY.
pub fn load_from(path: &Path, repo: &Path, now: u64) -> Loaded {
    let read_only = |problem: String| {
        tracing::warn!("{problem}");
        Loaded {
            file: TodoFile {
                read_only: true,
                ..TodoFile::new(repo)
            },
            problem: Some(problem),
        }
    };
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                file: TodoFile::new(repo),
                problem: None,
            }
        }
        Err(err) => {
            return read_only(format!(
                "couldn't read {} ({err}) — changes to these todos won't be saved",
                path.display()
            ))
        }
    };
    match serde_json::from_str::<TodoFile>(&raw) {
        Ok(file) => Loaded {
            file,
            problem: None,
        },
        Err(err) => {
            let aside = corrupt_path(path, now);
            if let Err(why) = std::fs::rename(path, &aside) {
                return read_only(format!(
                    "couldn't read {} ({err}) nor move it aside ({why}) — changes to these todos won't be saved",
                    path.display()
                ));
            }
            tracing::warn!(
                "unreadable {} ({err}), moved to {}",
                path.display(),
                aside.display()
            );
            Loaded {
                file: TodoFile::new(repo),
                problem: Some(format!(
                    "couldn't read this project's todos — moved the file to {} and started afresh",
                    aside.display()
                )),
            }
        }
    }
}

/// `abc.json` → `abc.corrupt-<now>.json`, beside it.
fn corrupt_path(path: &Path, now: u64) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{stem}.corrupt-{now}.json"))
}

/// The lists waiting to be written, the newest copy of each, and whether
/// a writer is on them.
#[derive(Debug, Default)]
struct Queue {
    waiting: Vec<(PathBuf, TodoFile)>,
    writing: bool,
}

impl Queue {
    const fn new() -> Self {
        Self {
            waiting: Vec::new(),
            writing: false,
        }
    }

    /// `file` is now what `path` should hold, over any copy still
    /// waiting. True when no writer is on the queue: the caller starts one.
    fn put(&mut self, path: PathBuf, file: TodoFile) -> bool {
        match self.waiting.iter_mut().find(|(p, _)| *p == path) {
            Some(slot) => slot.1 = file,
            None => self.waiting.push((path, file)),
        }
        !std::mem::replace(&mut self.writing, true)
    }

    /// The next copy to write, or — the queue empty — None, the writer
    /// stepping off it.
    fn next(&mut self) -> Option<(PathBuf, TodoFile)> {
        if self.waiting.is_empty() {
            self.writing = false;
            return None;
        }
        Some(self.waiting.remove(0))
    }
}

/// Every save in this process goes through the one queue.
#[cfg_attr(test, allow(dead_code))]
static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());

/// Write `file` to its place, off the loop — unless it is READ-ONLY.
pub fn save(file: &TodoFile) {
    if file.read_only {
        return;
    }
    let Some(path) = path_for(&file.repo_path) else {
        return;
    };
    // Under test, at once: a test reads back what it saved.
    #[cfg(test)]
    write(&path, file);
    #[cfg(not(test))]
    {
        let start = QUEUE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .put(path, file.clone());
        if start {
            std::thread::spawn(|| drain(&QUEUE, write));
        }
    }
}

/// Write what `queue` holds until it is empty, the newest copy of each
/// list at a time.
#[cfg_attr(test, allow(dead_code))]
fn drain(queue: &Mutex<Queue>, write: impl Fn(&Path, &TodoFile)) {
    loop {
        let next = queue.lock().unwrap_or_else(|e| e.into_inner()).next();
        let Some((path, file)) = next else {
            return;
        };
        write(&path, &file);
    }
}

fn write(path: &Path, file: &TodoFile) {
    if let Err(err) = crate::pr_cache::write_json_atomic(path, file) {
        tracing::warn!("failed to save {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample(repo: &Path) -> TodoFile {
        let mut file = TodoFile::new(repo);
        file.groups.push(Group {
            id: 1,
            parent: None,
            name: "Emails".into(),
            collapsed: true,
        });
        file.items.push(Item {
            id: 2,
            group: 1,
            text: "setup resend".into(),
            priority: 2,
            created: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(),
            done: Some(Local.with_ymd_and_hms(2026, 10, 6, 23, 30, 0).unwrap()),
            linear: Some("RIP-412".into()),
            agent: None,
            linear_seen: None,
        });
        file
    }

    /// What is saved is what comes back, the late-evening tick on its
    /// own local day included.
    #[test]
    fn a_list_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = PathBuf::from("/tmp/demo");
        with_dir(dir.path().to_path_buf(), || {
            let file = sample(&repo);
            save(&file);
            let path = path_for(&repo).unwrap();
            assert!(path.starts_with(dir.path()));
            assert!(path.to_string_lossy().ends_with(".json"));
            let back = load(&repo).file;
            assert_eq!(back, file);
            let done = back.items[0].done.unwrap();
            assert_eq!(
                done.date_naive(),
                NaiveDate::from_ymd_opt(2026, 10, 6).unwrap()
            );
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(raw.contains("\"repo_path\": \"/tmp/demo\""), "{raw}");
            assert!(!raw.contains("\"agent\""), "unset fields stay out");
        });
    }

    #[test]
    fn no_file_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let repo = PathBuf::from("/tmp/elsewhere");
        let path = dir.path().join("none.json");
        let loaded = load_from(&path, &repo, 1);
        assert_eq!(loaded.file, TodoFile::new(&repo));
        assert_eq!(loaded.file.version, VERSION);
        assert!(loaded.problem.is_none());
    }

    /// A file that does not parse is moved aside, never written over:
    /// what was in it is still on disk, under a name that says why.
    #[test]
    fn a_corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let repo = PathBuf::from("/tmp/demo");
        let path = dir.path().join("abc.json");
        std::fs::write(&path, "{ not json").unwrap();
        let loaded = load_from(&path, &repo, 1_700_000_000);
        let aside = dir.path().join("abc.corrupt-1700000000.json");
        assert!(loaded
            .problem
            .unwrap()
            .contains("abc.corrupt-1700000000.json"));
        assert!(!loaded.file.read_only, "afresh, and saved as usual");
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{ not json");
    }

    /// Different checkouts keep different files.
    #[test]
    fn each_checkout_has_its_own_file() {
        with_dir(PathBuf::from("/x"), || {
            let a = path_for(Path::new("/tmp/a")).unwrap();
            let b = path_for(Path::new("/tmp/b")).unwrap();
            assert_ne!(a, b);
            assert_eq!(a.parent(), Some(Path::new("/x")));
        });
    }

    /// A file there but unreadable — a directory in its place here — is
    /// no empty list to save over it: the list is READ-ONLY, and says so.
    #[test]
    fn an_unreadable_file_makes_the_list_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let repo = PathBuf::from("/tmp/demo");
        with_dir(dir.path().to_path_buf(), || {
            let path = path_for(&repo).unwrap();
            std::fs::create_dir_all(&path).unwrap();
            let loaded = load(&repo);
            assert!(loaded.file.read_only);
            assert!(loaded.problem.unwrap().contains("won't be saved"));
            save(&loaded.file);
            assert!(path.is_dir(), "nothing written over it");
        });
    }

    /// A corrupt file that cannot be moved aside — its folder read-only —
    /// is left as it is, and nothing is saved over it.
    #[test]
    fn a_corrupt_file_that_wont_move_is_never_saved_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc.json");
        std::fs::write(&path, "{ not json").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let loaded = load_from(&path, Path::new("/tmp/demo"), 7);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root moves it anyway; the read-only answer is what is under test.
        if path.exists() {
            assert!(loaded.file.read_only);
            assert!(loaded.problem.unwrap().contains("nor move it aside"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        }
    }

    /// The queue keeps only the newest copy of a list waiting, and one
    /// writer drains it — so a save can never land after a later one.
    #[test]
    fn saves_write_the_newest_copy_once() {
        let queue = Mutex::new(Queue::new());
        let a = PathBuf::from("/a.json");
        let mut v1 = TodoFile::new(Path::new("/r"));
        v1.linear_team = Some("v1".into());
        let mut v2 = v1.clone();
        v2.linear_team = Some("v2".into());
        assert!(queue.lock().unwrap().put(a.clone(), v1), "starts a writer");
        assert!(!queue.lock().unwrap().put(a.clone(), v2), "one is on it");
        assert!(!queue
            .lock()
            .unwrap()
            .put("/b.json".into(), TodoFile::default()));
        let written = std::cell::RefCell::new(Vec::new());
        drain(&queue, |path, file| {
            written
                .borrow_mut()
                .push((path.to_path_buf(), file.linear_team.clone()))
        });
        assert_eq!(
            written.into_inner(),
            [(a, Some("v2".into())), ("/b.json".into(), None)]
        );
        assert!(
            queue
                .lock()
                .unwrap()
                .put("/c.json".into(), TodoFile::default()),
            "the writer stepped off: the next save starts one"
        );
    }
}
