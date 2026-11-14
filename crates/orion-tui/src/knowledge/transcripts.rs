//! Finding the TRANSCRIPTS a harvest reads: a Claude session by its id
//! under every `~/.claude*/projects/*/`, and — for a backfill, with no
//! snapshot to go on — every Claude and Cursor transcript of a project,
//! joined to merged pull requests by the branch it ran on or the PR URLs
//! it mentions.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead as _;
use std::path::{Path, PathBuf};

use super::condense::Shape;

/// How many mentions of a PR's URL join a Claude transcript to it.
pub(crate) const CLAUDE_MENTIONS: usize = 3;
/// How many join a Cursor transcript, which records no branch.
pub(crate) const CURSOR_MENTIONS: usize = 2;

/// Every Claude projects dir: `$CLAUDE_CONFIG_DIR`'s (or `~/.claude`'s),
/// and each `~/.claude*/projects` beside it — one per Claude account.
pub(crate) fn claude_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = orion_core::paths::claude_config_dir()
        .map(|d| d.join("projects"))
        .into_iter()
        .collect();
    if let Some(home) = orion_core::env::home_dir() {
        for entry in std::fs::read_dir(&home).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(".claude") {
                roots.push(entry.path().join("projects"));
            }
        }
    }
    roots.retain(|r| r.is_dir());
    roots.sort();
    roots.dedup();
    roots
}

/// `~/.cursor/projects`, where Cursor keeps its agent transcripts.
pub(crate) fn cursor_root() -> Option<PathBuf> {
    orion_core::env::home_dir()
        .map(|h| h.join(".cursor").join("projects"))
        .filter(|r| r.is_dir())
}

/// Claude session `id`'s transcript, in any cwd's folder under `roots`.
pub(crate) fn find_claude(roots: &[PathBuf], id: &str) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    if id.is_empty() || Path::new(&name).file_name() != Some(std::ffi::OsStr::new(&name)) {
        return None;
    }
    roots.iter().find_map(|root| {
        std::fs::read_dir(root)
            .ok()?
            .flatten()
            .map(|slug| slug.path().join(&name))
            .find(|f| f.is_file())
    })
}

/// A transcript on disk.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Found {
    pub shape: Shape,
    pub path: PathBuf,
}

impl Found {
    /// The session id, from the file name (Claude) or its folder (Cursor).
    pub(crate) fn session(&self) -> String {
        let stem = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned());
        stem.unwrap_or_default()
    }
}

/// Whether a Claude or Cursor projects folder (a cwd made a slug) is one
/// of `folder`'s checkouts: its slug carries the project's.
pub(crate) fn belongs(slug: &str, folder: &str) -> bool {
    let want: String = folder
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    !want.is_empty() && slug.contains(&want)
}

/// Every transcript of project `folder`: Claude's under `claude_roots`,
/// Cursor's under `cursor_root`'s `<slug>/agent-transcripts/`.
pub(crate) fn list(
    claude_roots: &[PathBuf],
    cursor_root: Option<&Path>,
    folder: &str,
) -> Vec<Found> {
    let mut found = Vec::new();
    let slugs = |root: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| belongs(&e.file_name().to_string_lossy(), folder))
            .map(|e| e.path())
            .collect()
    };
    for root in claude_roots {
        for slug in slugs(root) {
            found.extend(jsonl_in(&slug).into_iter().map(|path| Found {
                shape: Shape::Claude,
                path,
            }));
        }
    }
    if let Some(root) = cursor_root {
        for slug in slugs(root) {
            let transcripts = slug.join("agent-transcripts");
            let mut files = jsonl_in(&transcripts);
            for dir in std::fs::read_dir(&transcripts)
                .into_iter()
                .flatten()
                .flatten()
            {
                if dir.path().is_dir() {
                    files.extend(jsonl_in(&dir.path()));
                }
            }
            found.extend(files.into_iter().map(|path| Found {
                shape: Shape::Cursor,
                path,
            }));
        }
    }
    found.sort();
    found
}

fn jsonl_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "jsonl"))
        .collect()
}

/// What a transcript says about which pull requests it worked on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scan {
    pub found: Found,
    /// The `gitBranch` values its records carry (Claude only).
    pub branches: BTreeSet<String>,
    /// PR number → how many times `<repo>/pull/<n>` appears.
    pub mentions: BTreeMap<u64, usize>,
}

/// Read `found` for its branches and the mentions of `repo`'s pull
/// requests (`repo` as `github.com/owner/name`).
pub(crate) fn scan(found: &Found, repo: &str) -> Scan {
    let mut out = Scan {
        found: found.clone(),
        branches: BTreeSet::new(),
        mentions: BTreeMap::new(),
    };
    let Ok(file) = std::fs::File::open(&found.path) else {
        return out;
    };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        scan_line(&line, repo, &mut out);
    }
    out
}

fn scan_line(line: &str, repo: &str, out: &mut Scan) {
    const BRANCH: &str = "\"gitBranch\":\"";
    if let Some(at) = line.find(BRANCH) {
        let rest = &line[at + BRANCH.len()..];
        if let Some(end) = rest.find('"') {
            if end > 0 {
                out.branches.insert(rest[..end].to_string());
            }
        }
    }
    let needle = format!("{repo}/pull/");
    let mut from = 0;
    while let Some(at) = line[from..].find(&needle) {
        let start = from + at + needle.len();
        let digits: String = line[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(n) = digits.parse::<u64>() {
            *out.mentions.entry(n).or_default() += 1;
        }
        from = start;
    }
}

/// A merged pull request as a join sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Merged {
    pub number: u64,
    pub branch: String,
}

/// Each pull request's transcripts: a Claude one that ran on its branch
/// (never a shared one, `bases`) or names its URL [`CLAUDE_MENTIONS`]
/// times; a Cursor one that names it [`CURSOR_MENTIONS`] times.
pub(crate) fn join(prs: &[Merged], scans: &[Scan], bases: &[String]) -> BTreeMap<u64, Vec<Found>> {
    let mut joined: BTreeMap<u64, Vec<Found>> = BTreeMap::new();
    for pr in prs {
        for scan in scans {
            let mentions = scan.mentions.get(&pr.number).copied().unwrap_or(0);
            let hit = match scan.found.shape {
                Shape::Claude => {
                    (!pr.branch.is_empty()
                        && !bases.contains(&pr.branch)
                        && scan.branches.contains(&pr.branch))
                        || mentions >= CLAUDE_MENTIONS
                }
                Shape::Cursor => mentions >= CURSOR_MENTIONS,
            };
            if hit {
                joined
                    .entry(pr.number)
                    .or_default()
                    .push(scan.found.clone());
            }
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn transcripts_are_joined_by_branch_or_by_mentions() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude/projects");
        let cursor = home.path().join(".cursor/projects");
        let pr = "https://github.com/o/app/pull";
        // On the PR's branch.
        write(
            &claude.join("-src-app/s1.jsonl"),
            r#"{"type":"user","gitBranch":"fix-login","message":{"content":"go"}}"#,
        );
        // On the base branch, naming #13 three times.
        write(
            &claude.join("-wt-app-dev/s2.jsonl"),
            &format!(
                "{{\"gitBranch\":\"dev\",\"m\":\"{pr}/13 and {pr}/13\"}}\n{{\"gitBranch\":\"dev\",\"m\":\"{pr}/13 {pr}/130\"}}\n"
            ),
        );
        // Another project's.
        write(
            &claude.join("-src-other/s3.jsonl"),
            r#"{"gitBranch":"fix-login"}"#,
        );
        // Cursor, naming #13 twice and #12 once.
        write(
            &cursor.join("-src-app/agent-transcripts/c1/c1.jsonl"),
            &format!("{{\"role\":\"user\",\"t\":\"{pr}/13 {pr}/13 {pr}/12\"}}\n"),
        );

        let found = list(std::slice::from_ref(&claude), Some(&cursor), "app");
        assert_eq!(found.len(), 3, "{found:?}");
        let scans: Vec<Scan> = found.iter().map(|f| scan(f, "github.com/o/app")).collect();
        let s2 = scans.iter().find(|s| s.found.session() == "s2").unwrap();
        assert_eq!(s2.mentions.get(&13), Some(&3));
        assert_eq!(s2.mentions.get(&130), Some(&1));

        let prs = vec![
            Merged {
                number: 12,
                branch: "fix-login".into(),
            },
            Merged {
                number: 13,
                branch: "dev".into(),
            },
        ];
        let joined = join(&prs, &scans, &["dev".to_string()]);
        let sessions = |n: u64| -> Vec<String> {
            joined
                .get(&n)
                .into_iter()
                .flatten()
                .map(Found::session)
                .collect()
        };
        assert_eq!(sessions(12), vec!["s1"]);
        assert_eq!(sessions(13), vec!["s2", "c1"]);
    }

    #[test]
    fn a_claude_session_is_found_by_its_id() {
        let home = tempfile::tempdir().unwrap();
        let a = home.path().join(".claude/projects");
        let b = home.path().join(".claude-work/projects");
        write(&b.join("-src-app/abc.jsonl"), "{}");
        let roots = vec![a, b.clone()];
        assert_eq!(
            find_claude(&roots, "abc"),
            Some(b.join("-src-app/abc.jsonl"))
        );
        assert_eq!(find_claude(&roots, "nope"), None);
        assert_eq!(find_claude(&roots, "../x"), None);
    }
}
