//! `git grep` runner for the find-in-files overlay.
//!
//! `search` runs the grep to its end and is what a view with no BACKGROUND
//! READS handle calls inline (the unit tests). The live modal calls
//! `search_streaming` off the loop (`view_jobs`): `git grep` over a
//! ten-thousand-file checkout is 200 ms, and it used to be spent inside the
//! key handler once per character typed.

use crate::ui::truncate;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

/// Queries shorter than this don't search — one character would light up
//  half the repo.
pub const MIN_QUERY_LEN: usize = 2;
/// Hits beyond this are dropped (`truncated` tells the UI to say so).
pub const MAX_RESULTS: usize = 200;
/// Matched lines are clipped to this many chars to keep overlay state small.
const MAX_TEXT_LEN: usize = 250;

/// One `git grep` match.
#[derive(Debug, Clone, PartialEq)]
pub struct GrepHit {
    /// Path relative to the search root.
    pub path: String,
    /// 1-based line number.
    pub line: u64,
    /// The matched line, trimmed and clipped.
    pub text: String,
}

/// How the query is read: as the text itself, or — `regex` — as an
/// extended regular expression (`git grep -E`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mode {
    pub regex: bool,
}

/// The `git grep` arguments for `query` over `paths`: fixed-string or
/// extended regex as `mode` says, smart-case, tracked + untracked,
/// gitignore respected, binaries skipped.
fn grep_args<'a>(query: &'a str, mode: Mode, extra: &[&'a str], paths: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["grep", "-z", "-n", "-I", "--untracked", "--no-color"];
    args.push(if mode.regex { "-E" } else { "-F" });
    // Smart case: literal case only when the query has an uppercase char.
    if !query.chars().any(char::is_uppercase) {
        args.push("-i");
    }
    args.extend(extra);
    args.extend(["-e", query, "--"]);
    args.extend(paths);
    args
}

/// The selected hit's file as find in files previews it: read and
/// highlighted as the TREE BROWSER reads a file, with the query's matches
/// in it.
#[derive(Debug, Clone)]
pub struct GrepPreview {
    /// The file, relative to the checkout.
    pub path: String,
    /// Shared, not copied: the modal is cloned for every frame it draws,
    /// and a query typed on over the same file reads it once.
    pub file: Arc<crate::tree_browser::Preview>,
    /// [`matches_in`] the file, placed ([`located`]), in line order.
    pub matches: Vec<(u64, Range<usize>)>,
    /// The query and mode the matches are of: lit only while they are
    /// still the modal's.
    pub query: String,
    pub mode: Mode,
}

/// `path` read for the preview pane, its matches found — the file itself
/// taken from `read`, when that was of the same file, so typing on over
/// it costs a `git grep` of it and no more. Blocking. None only when
/// `cancel` fired: the cursor has moved on.
pub fn read_preview(
    root: &Path,
    path: &str,
    query: &str,
    mode: Mode,
    read: Option<Arc<crate::tree_browser::Preview>>,
    cancel: Option<&crate::view_jobs::Cancel>,
) -> Option<GrepPreview> {
    let file = match read {
        Some(file) => file,
        None => Arc::new(crate::tree_browser::file_preview(root, path, cancel)?),
    };
    if cancel.is_some_and(|c| c.is_cancelled()) {
        return None;
    }
    let matches = located(&file.text, &matches_in(root, path, query, mode));
    Some(GrepPreview {
        path: path.to_string(),
        matches,
        file,
        query: query.to_string(),
        mode,
    })
}

/// Every match of `query` in the one file `path`, as git finds them: by
/// line, the text each match is (`git grep -o`), in order along the line —
/// what the preview pane lights, in either mode. Empty when git can't say.
pub fn matches_in(root: &Path, path: &str, query: &str, mode: Mode) -> Vec<(u64, String)> {
    // The path as itself: `app/[id]/page.tsx` is no glob over `app/i/…`.
    let literal = format!(":(literal){path}");
    let args = grep_args(query, mode, &["-o"], &[&literal]);
    let Ok(output) = crate::git_diff::run_git(root, &args) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for record in output.stdout.split(|b| *b == b'\n') {
        let mut fields = record.splitn(3, |b| *b == 0);
        let (Some(file), Some(line), Some(text)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if file != path.as_bytes() {
            continue;
        }
        if let Ok(line) = String::from_utf8_lossy(line).parse::<u64>() {
            found.push((line, String::from_utf8_lossy(text).into_owned()));
        }
    }
    found
}

/// `found` matches placed in `text` as byte ranges: each one the next
/// place its line holds that text after the match before it. git's own
/// `--column` can't be leant on here — past a line's first match it
/// counts from 0, not 1. So a regex match whose text also appears earlier
/// on its line (`\bid` in `valid id`) lights the earlier copy, and one
/// with a tab in it — the preview has its tabs as spaces — isn't lit.
pub fn located(text: &str, found: &[(u64, String)]) -> Vec<(u64, Range<usize>)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut placed = Vec::with_capacity(found.len());
    let mut cursor = (0, 0);
    for (line, hit) in found {
        let Some(row) = lines.get((*line as usize).saturating_sub(1)) else {
            continue;
        };
        let from = if cursor.0 == *line { cursor.1 } else { 0 };
        if let Some(at) = row.get(from..).and_then(|rest| rest.find(hit.as_str())) {
            let range = from + at..from + at + hit.len();
            cursor = (*line, range.end.max(from + 1));
            placed.push((*line, range));
        }
    }
    placed
}

/// [`search`] for the live modal, run off the loop: the output is read as
/// git writes it and git is killed at the result cap — a two-letter query
/// in a big checkout matches a hundred thousand lines, and `search` reads
/// every one of them to keep two hundred — or as soon as `cancel` says the
/// query has moved on. `None` is that cancel: there is no answer to land.
pub fn search_streaming(
    root: &Path,
    query: &str,
    mode: Mode,
    cancel: &crate::view_jobs::Cancel,
) -> Option<Result<(Vec<GrepHit>, bool), String>> {
    use std::io::{BufRead, Read};
    let mut child = match crate::git_diff::git_command(root)
        .args(grep_args(query, mode, &[], &["."]))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return Some(Err(format!("failed to run git: {e}"))),
    };
    let mut hits = Vec::new();
    let mut truncated = false;
    let mut reader = std::io::BufReader::new(child.stdout.take()?);
    let mut record = Vec::new();
    loop {
        record.clear();
        match reader.read_until(b'\n', &mut record) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        let (mut parsed, _) = parse_grep_z(&record);
        if parsed.is_empty() {
            continue;
        }
        if hits.len() >= MAX_RESULTS {
            truncated = true;
            break;
        }
        hits.append(&mut parsed);
    }
    if truncated {
        let _ = child.kill();
        let _ = child.wait();
        return Some(Ok((hits, true)));
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    // git grep exits 1 for "no matches" — only >= 2 is an error.
    match child.wait().ok().and_then(|status| status.code()) {
        Some(0) | Some(1) => Some(Ok((hits, false))),
        _ => Some(Err(format!("git grep failed: {}", stderr.trim()))),
    }
}

/// Smart-case search over the checkout (tracked + untracked, gitignore
/// respected, binaries skipped), the query read as `mode` says. Returns
/// `(hits, truncated)`; `Err` is a user-facing message shown inside the
/// overlay — a regex git can't read among them.
pub fn search(root: &Path, query: &str, mode: Mode) -> Result<(Vec<GrepHit>, bool), String> {
    let output = crate::git_diff::run_git(root, &grep_args(query, mode, &[], &["."]))?;
    // git grep exits 1 for "no matches" — only >= 2 is an error.
    match output.status.code() {
        Some(0) => Ok(parse_grep_z(&output.stdout)),
        Some(1) => Ok((Vec::new(), false)),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("git grep failed: {}", stderr.trim()))
        }
    }
}

/// Parse `git grep -z -n` records: `<path>\0<line>\0<text>\n`, stopping at
/// `MAX_RESULTS` (the bool says whether anything was dropped).
pub fn parse_grep_z(bytes: &[u8]) -> (Vec<GrepHit>, bool) {
    let mut hits = Vec::new();
    for record in bytes.split(|b| *b == b'\n') {
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, |b| *b == 0);
        let (Some(path), Some(line), Some(text)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(line) = String::from_utf8_lossy(line).parse::<u64>() else {
            continue;
        };
        if hits.len() >= MAX_RESULTS {
            return (hits, true);
        }
        let text = String::from_utf8_lossy(text);
        let text = text.trim();
        hits.push(GrepHit {
            path: String::from_utf8_lossy(path).into_owned(),
            line,
            text: truncate(text, MAX_TEXT_LEN),
        });
    }
    (hits, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    #[test]
    fn parse_grep_z_splits_path_line_text() {
        let bytes = b"src/a.rs\x007\x00    let x = grep_me();\nsrc/b file.rs\x0012\x00grep_me\n";
        let (hits, truncated) = parse_grep_z(bytes);
        assert!(!truncated);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "src/a.rs");
        assert_eq!(hits[0].line, 7);
        assert_eq!(hits[0].text, "let x = grep_me();", "text is trimmed");
        assert_eq!(hits[1].path, "src/b file.rs", "spaces in paths survive");
        assert_eq!(hits[1].line, 12);
    }

    #[test]
    fn parse_grep_z_caps_results() {
        let mut bytes = Vec::new();
        for i in 0..(MAX_RESULTS + 5) {
            bytes.extend_from_slice(format!("f.rs\x00{}\x00match\n", i + 1).as_bytes());
        }
        let (hits, truncated) = parse_grep_z(&bytes);
        assert_eq!(hits.len(), MAX_RESULTS);
        assert!(truncated);
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
        std::fs::write(repo.join("tracked.txt"), "needle in a haystack\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        repo
    }

    #[test]
    fn search_finds_tracked_and_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(repo.join("fresh.txt"), "another Needle here\n").unwrap();

        // Lowercase query is case-insensitive (smart case): both files hit.
        let (hits, truncated) = search(&repo, "needle", Mode::default()).unwrap();
        assert!(!truncated);
        assert_eq!(hits.len(), 2, "{hits:?}");
        let tracked = hits.iter().find(|h| h.path == "tracked.txt").unwrap();
        assert_eq!(tracked.line, 1);
        assert_eq!(tracked.text, "needle in a haystack");
        assert!(hits.iter().any(|h| h.path == "fresh.txt"), "{hits:?}");

        // An uppercase char makes the query literal: only fresh.txt hits.
        let (hits, _) = search(&repo, "Needle", Mode::default()).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, "fresh.txt");

        // No matches is Ok(empty), not an error (exit code 1).
        let (hits, _) = search(&repo, "no_such_string_zzz", Mode::default()).unwrap();
        assert!(hits.is_empty());
    }

    /// The query is the text itself until regex mode reads it as an
    /// extended regex; git's refusal of a broken one is the error shown.
    #[test]
    fn regex_mode_reads_the_query_as_a_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        std::fs::write(repo.join("pat.txt"), "a.c\nabc\naxxc\n").unwrap();
        let regex = Mode { regex: true };
        let lines = |mode| {
            let (hits, _) = search(&repo, "a.c", mode).unwrap();
            hits.iter().map(|h| h.line).collect::<Vec<_>>()
        };
        assert_eq!(lines(Mode::default()), [1], "the dot is a dot");
        assert_eq!(lines(regex), [1, 2], "the dot is any character");
        let (hits, _) = search(&repo, "ax+c", regex).unwrap();
        assert_eq!(hits.iter().map(|h| h.line).collect::<Vec<_>>(), [3]);
        let err = search(&repo, "a(b", regex).unwrap_err();
        assert!(err.contains("git grep failed"), "{err}");
    }

    /// Each match in one file, by line, as the byte range it takes —
    /// several on a line, case as the file has it, and a regex's as long
    /// as what it matched.
    #[test]
    fn matches_in_a_file_are_their_byte_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let repo = make_repo(&dir);
        let text = "x needle y NEEDLE\nnone\n  neeedle\n";
        std::fs::write(repo.join("m.txt"), text).unwrap();
        let ranges = |query, mode| located(text, &matches_in(&repo, "m.txt", query, mode));
        assert_eq!(ranges("needle", Mode::default()), [(1, 2..8), (1, 11..17)]);
        assert_eq!(
            ranges("ne+dle", Mode { regex: true }),
            [(1, 2..8), (1, 11..17), (3, 2..9)],
            "smart case: lower case matches either case"
        );
        assert!(matches_in(&repo, "gone.txt", "needle", Mode::default()).is_empty());
    }

    #[test]
    fn search_errors_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let err = search(dir.path(), "anything", Mode::default()).unwrap_err();
        assert!(err.contains("git grep failed"), "{err}");
    }
}
