//! The HAND-WRITTEN RECORDS: every file under `.orion/decisions/` but
//! orion's own `harvested/`, in whatever layout the repo keeps them.
//! They are authoritative and read-only to orion: the harvest's check
//! gets them as context ([`for_check`]) and drops a rule that contradicts
//! one, and every builder prompt lists them ([`listing`]).
//!
//! They are read from a [`Checkout`]: a folder on disk (a session's
//! worktree, orion's working clone), or a commit not checked out yet (the
//! base a builder's worktree is about to be cut from).

use std::path::{Path, PathBuf};

use super::store::{DECISIONS, DIR};

/// The biggest file read as a record or a page.
const FILE_MAX: u64 = 256 * 1024;
/// How much of a record's body the check reads, in chars.
const BODY_CHARS: usize = 600;
/// How much of all the records the check reads, in chars.
pub(crate) const CHECK_BUDGET: usize = 12_000;

/// Where repo files are read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Checkout {
    /// A checkout on disk.
    Dir(PathBuf),
    /// The tree of commit `rev` in the repo at `repo`.
    Rev { repo: PathBuf, rev: String },
}

impl Checkout {
    /// Every text file under `dir` (repo-relative), with its text, sorted
    /// by path.
    pub(crate) fn files(&self, dir: &str) -> Vec<(String, String)> {
        let mut files = match self {
            Checkout::Dir(root) => {
                let mut out = Vec::new();
                walk(root, &root.join(dir), &mut out);
                out
            }
            Checkout::Rev { repo, rev } => files_at(repo, rev, dir).unwrap_or_default(),
        };
        files.sort();
        files
    }
}

/// Every text file under `dir`, as paths relative to `root`.
fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(root, &path, out);
        } else if kind.is_file() && entry.metadata().is_ok_and(|m| m.len() <= FILE_MAX) {
            let (Ok(rel), Ok(text)) = (path.strip_prefix(root), std::fs::read_to_string(&path))
            else {
                continue;
            };
            out.push((rel.to_string_lossy().into_owned(), text));
        }
    }
}

/// Every text file under `dir` in commit `rev`: one `git ls-tree`, then
/// one `git cat-file --batch` for all of them.
fn files_at(repo: &Path, rev: &str, dir: &str) -> Result<Vec<(String, String)>, String> {
    use std::io::{Read as _, Write as _};
    let listing = crate::git_proc::read(repo, &["ls-tree", "-r", "-l", rev, "--", dir])?;
    let blobs: Vec<(String, String)> = listing
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let mut meta = meta.split_whitespace();
            let (_, kind, sha, size) = (meta.next()?, meta.next()?, meta.next()?, meta.next()?);
            let small = size.parse::<u64>().is_ok_and(|n| n <= FILE_MAX);
            (kind == "blob" && small).then(|| (sha.to_string(), path.to_string()))
        })
        .collect();
    if blobs.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = crate::git_diff::git_command(repo)
        .args(["cat-file", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let ids: String = blobs.iter().map(|(sha, _)| format!("{sha}\n")).collect();
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("git cat-file has no pipes".into());
    };
    let writer = std::thread::spawn(move || stdin.write_all(ids.as_bytes()));
    let mut out = Vec::new();
    let read = stdout.read_to_end(&mut out);
    let _ = writer.join();
    // Reaped whatever the read did: a child left unwaited is a zombie of
    // the TUI until it quits.
    if read.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    read.map_err(|e| e.to_string())?;
    // Each answer: `<sha> blob <size>\n<content>\n`.
    let mut files = Vec::new();
    let mut at = 0;
    for (_, path) in &blobs {
        let Some(end) = out[at..].iter().position(|&b| b == b'\n') else {
            break;
        };
        let header = String::from_utf8_lossy(&out[at..at + end]).into_owned();
        at += end + 1;
        let Some(size) = header
            .split_whitespace()
            .nth(2)
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        let body = out.get(at..at + size).unwrap_or_default();
        at += size + 1;
        if let Ok(text) = std::str::from_utf8(body) {
            files.push((path.clone(), text.to_string()));
        }
    }
    Ok(files)
}

/// One hand-written record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    /// Repo-relative.
    pub path: String,
    pub title: String,
    /// Its text after the title, front matter left out.
    pub body: String,
}

/// Whether the repo-relative `path` is a hand-written record: under
/// [`DECISIONS`], not under orion's [`DIR`], Markdown, and not a README.
pub(crate) fn is_record(path: &str) -> bool {
    let inside = path
        .strip_prefix(DECISIONS)
        .is_some_and(|rest| rest.starts_with('/'));
    let harvested = path
        .strip_prefix(DIR)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let markdown = name.ends_with(".md") || name.ends_with(".markdown");
    inside && !harvested && markdown && !name.starts_with("readme.")
}

/// The records among `files` (repo-relative path, text), by path.
pub(crate) fn from_files(files: &[(String, String)]) -> Vec<Record> {
    files
        .iter()
        .filter(|(path, _)| is_record(path))
        .map(|(path, text)| record(path, text))
        .collect()
}

/// The records in `checkout`.
pub(crate) fn records(checkout: &Checkout) -> Vec<Record> {
    from_files(&checkout.files(DECISIONS))
}

/// `text` as a record: its first `# ` heading for a title (else its file
/// name), front matter left out of the body.
fn record(path: &str, text: &str) -> Record {
    let mut lines = text.lines().peekable();
    if lines.peek().is_some_and(|l| l.trim() == "---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }
    let mut title = String::new();
    let mut body = Vec::new();
    for line in lines {
        match line.trim().strip_prefix("# ") {
            Some(heading) if title.is_empty() => title = heading.trim().to_string(),
            _ => body.push(line),
        }
    }
    if title.is_empty() {
        let name = path.rsplit('/').next().unwrap_or(path);
        title = name.split('.').next().unwrap_or(name).to_string();
    }
    Record {
        path: path.to_string(),
        title,
        body: body.join("\n").trim().to_string(),
    }
}

/// The records as the check-and-merge reads them: every title and path
/// first, then the start of each body while [`CHECK_BUDGET`] chars allow
/// — past that, titles only, and with too many even for those, a count
/// of the rest. Empty with none.
pub(crate) fn for_check(records: &[Record]) -> String {
    /// Room kept for the closing count.
    const TAIL: usize = 40;
    let len = |s: &str| s.chars().count();
    let heads: Vec<String> = records
        .iter()
        .map(|r| format!("### {} ({})\n", r.title, r.path))
        .collect();
    let mut body_room = CHECK_BUDGET.saturating_sub(heads.iter().map(|h| len(h)).sum());
    let mut out = String::new();
    for (n, (r, head)) in records.iter().zip(&heads).enumerate() {
        let mut piece = head.clone();
        let flat = r.body.split_whitespace().collect::<Vec<_>>().join(" ");
        let body = format!("{}\n\n", crate::ui::truncate(&flat, BODY_CHARS));
        if !flat.is_empty() && len(&body) <= body_room {
            body_room -= len(&body);
            piece.push_str(&body);
        }
        if len(&out) + len(&piece) + TAIL > CHECK_BUDGET {
            out.push_str(&format!("… and {} more records\n", records.len() - n));
            break;
        }
        out.push_str(&piece);
    }
    out.trim_end().to_string()
}

/// The records in the checkout at `dir`, as the check reads them
/// ([`for_check`]).
pub(crate) fn for_check_in(dir: &Path) -> String {
    for_check(&records(&Checkout::Dir(dir.to_path_buf())))
}

/// The records as a builder's prompt lists them: title and path a line.
pub(crate) fn listing(records: &[Record]) -> String {
    records
        .iter()
        .map(|r| format!("- {} — `{}`", r.title, r.path))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A checkout with two records in the repo's own layout, a README,
    /// and orion's harvested pages beside them.
    pub(crate) fn decisions(root: &Path) {
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            ".orion/decisions/auth/0001-sessions.md",
            "---\nstatus: accepted\n---\n# Sessions last 30 days\n\nDecided with legal. Never shorter.\n",
        );
        write(
            ".orion/decisions/0002-billing.md",
            "Invoices are monthly.\n",
        );
        write(".orion/decisions/README.md", "# How we write records\n");
        write(
            ".orion/decisions/harvested/index.md",
            &format!("{}\n# Knowledge: app\n", super::super::store::MARKER),
        );
        write(".orion/decisions/harvested/auth/login.md", "# auth/login\n");
    }

    #[test]
    fn records_are_every_markdown_file_but_orions_and_readmes() {
        let dir = tempfile::tempdir().unwrap();
        decisions(dir.path());
        let found = records(&Checkout::Dir(dir.path().to_path_buf()));
        assert_eq!(
            found,
            vec![
                Record {
                    path: ".orion/decisions/0002-billing.md".into(),
                    title: "0002-billing".into(),
                    body: "Invoices are monthly.".into(),
                },
                Record {
                    path: ".orion/decisions/auth/0001-sessions.md".into(),
                    title: "Sessions last 30 days".into(),
                    body: "Decided with legal. Never shorter.".into(),
                },
            ]
        );
        assert_eq!(
            listing(&found),
            "- 0002-billing — `.orion/decisions/0002-billing.md`\n- Sessions last 30 days — `.orion/decisions/auth/0001-sessions.md`"
        );
        assert!(!is_record(".orion/decisions-old/x.md"));
        assert!(!is_record(".orion/decisions/harvested/x.md"));
    }

    #[test]
    fn the_check_reads_records_within_its_budget() {
        let many: Vec<Record> = (0..40)
            .map(|n| Record {
                path: format!(".orion/decisions/{n:04}.md"),
                title: format!("Record {n}"),
                body: "word ".repeat(400),
            })
            .collect();
        let text = for_check(&many);
        assert!(text.chars().count() <= CHECK_BUDGET, "{}", text.len());
        assert!(text.starts_with("### Record 0 (.orion/decisions/0000.md)\nword word"));
        assert!(
            text.contains("### Record 39 (.orion/decisions/0039.md)"),
            "titles past the bodies"
        );
        assert_eq!(for_check(&[]), "");
    }

    #[test]
    fn a_commit_reads_like_a_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        crate::git_proc::run(repo, &["init", "-q"]).unwrap();
        decisions(repo);
        crate::git_proc::run(repo, &["add", "."]).unwrap();
        crate::git_proc::run(
            repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "one",
            ],
        )
        .unwrap();
        // Changed on disk after the commit: the commit's text is read.
        std::fs::write(repo.join(".orion/decisions/0002-billing.md"), "changed").unwrap();
        let at = Checkout::Rev {
            repo: repo.to_path_buf(),
            rev: "HEAD".into(),
        };
        let files = at.files(DECISIONS);
        assert_eq!(files.len(), 5);
        assert!(files.contains(&(
            ".orion/decisions/0002-billing.md".into(),
            "Invoices are monthly.\n".into()
        )));
        assert_eq!(records(&at).len(), 2);
    }
}
