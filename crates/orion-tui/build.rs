//! Bakes the RELEASE NOTES the WHAT'S NEW page reads (`src/whats_new.rs`)
//! into the build, from git: every push to `main` is a release (see
//! `.github/workflows/release.yml`), so a release's notes are the commits
//! between its `v*` tag and the one before — each commit's subject and
//! body. The release being built, whose tag does not exist yet, is the
//! commits since the newest tag, under this build's own version when that
//! is newer (the workflow stamps it into Cargo.toml before building).
//!
//! No git, no tags (a source tarball, a shallow checkout) is no notes, not
//! a failed build: the page has nothing to say and setup carries on.

use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A new tag — a release — changes what is in the notes. Only paths
    // that exist: Cargo reruns a script on every build for one that
    // doesn't. (The commits since the newest tag only count in a release
    // build, a fresh checkout, so HEAD is not watched.)
    if let Some(dir) = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        for path in ["refs/tags", "packed-refs"] {
            let path = PathBuf::from(dir.trim()).join(path);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let current = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let releases = releases(&current);
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("releases.rs"), render(&releases)).expect("OUT_DIR is writable");
}

struct Release {
    version: String,
    date: String,
    changes: Vec<Change>,
}

struct Change {
    title: String,
    details: Vec<String>,
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `update_check::parse_version`'s twin — a build script cannot use the
/// crate it builds.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// Every release, newest first.
fn releases(current: &str) -> Vec<Release> {
    // Each tag with the day it was cut, in one call.
    let Some(tags) = git(&[
        "for-each-ref",
        "--format=%(refname:short)%09%(creatordate:short)",
        "refs/tags/v*",
    ]) else {
        return Vec::new();
    };
    let mut tags: Vec<((u64, u64, u64), String, String)> = tags
        .lines()
        .filter_map(|line| {
            let (tag, date) = line.split_once('\t')?;
            Some((parse_version(tag)?, tag.to_string(), date.to_string()))
        })
        .collect();
    tags.sort();
    let mut out = Vec::new();
    for (i, (version, tag, date)) in tags.iter().enumerate() {
        // The first release is the whole history before it: say it was the
        // first, not everything that went into it.
        let range = match i {
            0 => format!("{tag}^!"),
            _ => format!("{}..{tag}", tags[i - 1].1),
        };
        out.push(Release {
            version: format!("{}.{}.{}", version.0, version.1, version.2),
            date: date.clone(),
            changes: changes(&range),
        });
    }
    // The release this build is: past the newest tag, under its own number.
    if let (Some(newest), Some(this)) = (tags.last(), parse_version(current)) {
        if this > newest.0 {
            let changes = changes(&format!("{}..HEAD", newest.1));
            if !changes.is_empty() {
                out.push(Release {
                    version: current.to_string(),
                    date: git(&["log", "-1", "--format=%cs", "HEAD"])
                        .map(|d| d.trim().to_string())
                        .unwrap_or_default(),
                    changes,
                });
            }
        }
    }
    out.reverse();
    out
}

/// The commits in `range`, oldest first — not merges, which say nothing
/// of their own — each one's subject, and its body as paragraphs and
/// bullets, trailers dropped.
fn changes(range: &str) -> Vec<Change> {
    let Some(log) = git(&[
        "log",
        "--reverse",
        "--no-merges",
        "--format=%s%x1f%b%x1e",
        range,
    ]) else {
        return Vec::new();
    };
    log.split('\x1e')
        .filter_map(|entry| {
            let (title, body) = entry.trim_start_matches('\n').split_once('\x1f')?;
            let title = title.trim();
            (!title.is_empty()).then(|| Change {
                title: title.to_string(),
                details: details(body),
            })
        })
        .collect()
}

/// A commit body's paragraphs and `- ` bullets, each joined onto one line
/// (the reader wraps them to its own width), without trailers such as
/// `Co-Authored-By:` or a `[skip release]` marker.
fn details(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open = false;
    for line in body.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        let trailer = ["co-authored-by:", "signed-off-by:", "[skip release]"]
            .iter()
            .any(|t| lower.starts_with(t));
        if trimmed.is_empty() || trailer {
            open = false;
            continue;
        }
        if let Some(bullet) = trimmed.strip_prefix("- ") {
            out.push(bullet.to_string());
            open = true;
        } else if open {
            let last = out.last_mut().expect("open means one was pushed");
            last.push(' ');
            last.push_str(trimmed);
        } else {
            out.push(trimmed.to_string());
            open = true;
        }
    }
    out
}

fn render(releases: &[Release]) -> String {
    let mut s = String::from("pub static RELEASES: &[Release] = &[\n");
    for r in releases {
        let _ = writeln!(
            s,
            "    Release {{ version: {:?}, date: {:?}, changes: &[",
            r.version, r.date
        );
        for c in &r.changes {
            let _ = writeln!(
                s,
                "        Change {{ title: {:?}, details: &{:?} }},",
                c.title, c.details
            );
        }
        s.push_str("    ] },\n");
    }
    s.push_str("];\n");
    s
}
