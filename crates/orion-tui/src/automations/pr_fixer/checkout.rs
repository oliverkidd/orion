//! THE CHECKOUT LOOK: before a fresh fixer is sent into a pull request's
//! checkout, look at the checkout itself. Someone orion can't see may be
//! at work there — a plain `claude` in a terminal, a session on another
//! machine, the owner — and a fixer beside them edits the same files and
//! pushes a second time. The sign is git and the files: a commit, merge,
//! rebase or reset in its reflog within [`ACTIVE`] (a fast-forward is a
//! sync, not work: orion's own pulls make those), or a tracked file
//! changed and still uncommitted that was written within [`ACTIVE`]. Seen
//! busy, the breakage waits ([`super::HOLD_RELOOK`]) and the flash says
//! why, once.
//!
//! Disk and git work, so off the UI thread: [`look`] asks on the blocking
//! pool and [`land`] keeps the answer for [`LOOK_TTL`], when the breakage
//! is decided again from the body in hand.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::app::App;

/// How recent a git write or a file edit has to be to mean someone is at
/// work in the checkout now.
pub const ACTIVE: Duration = Duration::from_secs(10 * 60);
/// How long a look answers for.
pub const LOOK_TTL: Duration = Duration::from_secs(20);
/// How long a look may be out before another is asked.
const LOOK_TIMEOUT: Duration = Duration::from_secs(30);

/// A look's answer: why the checkout is in use, or None.
#[derive(Debug, Clone)]
pub struct Look {
    pub at: Instant,
    pub busy: Option<String>,
}

/// Whether the checkouts at `paths` are in use, as far as a look in hand
/// says: `Some(Some(why))` busy, `Some(None)` free, None not known yet —
/// a look is then asked ([`look`]) and the breakage decided again when it
/// lands.
pub fn verdict(app: &mut App, url: &str, paths: &[PathBuf]) -> Option<Option<String>> {
    if paths.is_empty() {
        return Some(None);
    }
    if let Some(look) = app
        .autofix
        .checkouts
        .get(url)
        .filter(|l| l.at.elapsed() < LOOK_TTL)
    {
        return Some(look.busy.clone());
    }
    look(app, url, paths.to_vec());
    // Inline (the unit tests): in hand already.
    app.autofix
        .checkouts
        .get(url)
        .filter(|l| l.at.elapsed() < LOOK_TTL)
        .map(|l| l.busy.clone())
}

/// Ask whether the checkouts at `paths` are in use, on the blocking pool
/// — inline where there is none — unless a look is out.
fn look(app: &mut App, url: &str, paths: Vec<PathBuf>) {
    let out = app
        .autofix
        .looking
        .get(url)
        .is_some_and(|at| at.elapsed() < LOOK_TIMEOUT);
    if out {
        return;
    }
    app.autofix.looking.insert(url.to_string(), Instant::now());
    let url = url.to_string();
    match app.view_jobs.clone() {
        Some(jobs) => jobs.run(move || {
            let busy = busy_in_any(&paths, SystemTime::now());
            Some(crate::view_jobs::Answer::CheckoutLook { url, busy })
        }),
        None => {
            let busy = busy_in_any(&paths, SystemTime::now());
            land(app, url, busy);
        }
    }
}

/// A look landed: kept, and the watched breakage it was for decided again
/// on the next beat, from the body in hand.
pub fn land(app: &mut App, url: String, busy: Option<String>) {
    app.autofix.looking.remove(&url);
    if let Some(w) = app.autofix.watching.get_mut(&url) {
        w.relook = Some(Instant::now());
    }
    app.autofix.checkouts.insert(
        url,
        Look {
            at: Instant::now(),
            busy,
        },
    );
    app.dirty = true;
}

fn busy_in_any(paths: &[PathBuf], now: SystemTime) -> Option<String> {
    paths.iter().find_map(|p| busy_in(p, now))
}

/// Why the checkout at `dir` is in use at `now`, or None. Blocking.
pub fn busy_in(dir: &Path, now: SystemTime) -> Option<String> {
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let git_dir = PathBuf::from(git(&["rev-parse", "--absolute-git-dir"])?.trim());
    let now_s = now.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs() as i64;
    if let Some((at, what)) = last_reflog(&git_dir.join("logs").join("HEAD")) {
        let ago = now_s - at;
        if (0..ACTIVE.as_secs() as i64).contains(&ago) {
            return Some(format!("{what} there {}", ago_words(ago)));
        }
    }
    let status = git(&["status", "--porcelain=v1", "-z", "--untracked-files=no"])?;
    status
        .split('\0')
        .filter(|e| e.len() > 3)
        .filter_map(|e| {
            let file = dir.join(&e[3..]);
            let modified = std::fs::metadata(&file).ok()?.modified().ok()?;
            now.duration_since(modified).ok()
        })
        .filter(|ago| *ago < ACTIVE)
        .min()
        .map(|ago| {
            format!(
                "uncommitted changes there, edited {}",
                ago_words(ago.as_secs() as i64)
            )
        })
}

/// The newest work in a reflog: its time (epoch s) and what it was, the
/// reflog's own words up to the colon (`commit`, `commit (merge)`,
/// `rebase (finish)`). A fast-forward — a sync, orion's own pulls among
/// them — and a plain checkout are not work.
fn last_reflog(path: &Path) -> Option<(i64, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let (head, message) = line.split_once('\t')?;
    let mut fields = head.rsplitn(3, ' ');
    let _tz = fields.next()?;
    let at: i64 = fields.next()?.parse().ok()?;
    if message.contains("Fast-forward") || message.starts_with("checkout:") {
        return None;
    }
    let what = message
        .split(':')
        .next()
        .unwrap_or(message)
        .trim()
        .to_string();
    Some((at, format!("a git {what}")))
}

fn ago_words(secs: i64) -> String {
    match secs / 60 {
        0 => "just now".to_string(),
        1 => "a minute ago".to_string(),
        m => format!("{m} minutes ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "feat"]);
        std::fs::write(dir.path().join("a.txt"), "one").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "one"]);
        dir
    }

    #[test]
    fn a_fresh_commit_or_a_fresh_edit_is_someone_at_work() {
        let dir = repo();
        let now = SystemTime::now();
        let busy = busy_in(dir.path(), now).expect("just committed");
        assert!(busy.starts_with("a git commit"), "{busy}");

        let later = now + ACTIVE + Duration::from_secs(60);
        assert_eq!(busy_in(dir.path(), later), None, "long since, and clean");

        std::fs::write(dir.path().join("a.txt"), "two").unwrap();
        let busy = busy_in(
            dir.path(),
            SystemTime::now() + ACTIVE + Duration::from_secs(1),
        );
        assert_eq!(busy, None, "an edit long ago is no one at work");
        // A write that new is: look as of a moment after it, the commit
        // itself long gone.
        let reflog = std::fs::read_to_string(dir.path().join(".git/logs/HEAD")).unwrap();
        let old = reflog.replace(
            &reflog
                .split('\t')
                .next()
                .unwrap()
                .rsplit(' ')
                .nth(1)
                .unwrap()
                .to_string(),
            "1000000000",
        );
        std::fs::write(dir.path().join(".git/logs/HEAD"), old).unwrap();
        let busy = busy_in(dir.path(), SystemTime::now()).expect("edited just now");
        assert!(busy.starts_with("uncommitted changes there"), "{busy}");
    }

    #[test]
    fn a_fast_forward_is_a_sync_not_work() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("HEAD");
        std::fs::write(
            &log,
            "a b t <t@t> 1700000000 +0000\tcommit: one\na c t <t@t> 1700000100 +0000\tmerge @{u}: Fast-forward\n",
        )
        .unwrap();
        assert_eq!(last_reflog(&log), None);
        std::fs::write(
            &log,
            "a b t <t@t> 1700000000 +0000\tcommit (merge): Merge origin/dev\n",
        )
        .unwrap();
        assert_eq!(
            last_reflog(&log),
            Some((1700000000, "a git commit (merge)".into()))
        );
    }
}
