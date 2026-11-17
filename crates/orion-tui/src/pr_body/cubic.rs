//! CUBIC: what the Cubic review bot says of a pull request, and what the
//! agent said back.
//!
//! - **Findings** are the review threads Cubic started that are not
//!   resolved, each with the severity its comment names, `P0` to `P3`
//!   ([`severity`]); a comment that names none has no severity.
//! - **Declarations** are the agent's lines in the PR body or a PR
//!   comment: `Cubic dismissals: <item>; <item>` for a finding it chose
//!   not to fix (with its reason), and `Cubic unresolved: <item>` for one
//!   that came back three times and ended the loop. The items may also
//!   follow on their own `- ` lines. `none` says there are none.
//!
//! An open P1 blocks the PR unless the agent declared it; a declared P1
//! sends it to Review instead (`super::verdict`).

use crate::pr_review::github::ReviewThread;

/// One open Cubic finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// 0 to 3; None when the comment names no severity.
    pub severity: Option<u8>,
    pub path: String,
    pub place: String,
}

/// Cubic on one pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cubic {
    pub open: Vec<Finding>,
    pub dismissed: Vec<String>,
    pub unresolved: Vec<String>,
}

impl Cubic {
    /// Open findings of severity `p`.
    pub fn open_at(&self, p: u8) -> usize {
        self.open.iter().filter(|f| f.severity == Some(p)).count()
    }

    /// The dismissed P1s that say why they weren't fixed: what the
    /// verdict lets stand without a Review.
    pub fn dismissed_p1_with_reason(&self) -> usize {
        self.dismissed
            .iter()
            .filter(|item| about_p1(item) && has_reason(item))
            .count()
    }

    /// The declared items about a P1: every one that doesn't name a lower
    /// severity (one that names none is taken as a P1, the case it is
    /// there for).
    pub fn declared_p1(&self) -> Vec<&str> {
        self.dismissed
            .iter()
            .chain(&self.unresolved)
            .map(String::as_str)
            .filter(|item| about_p1(item))
            .collect()
    }

    /// The Cubic row: `clean`, or what is open and declared.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.open.is_empty() {
            let by: Vec<String> = (0..=3)
                .filter_map(|p| match self.open_at(p) {
                    0 => None,
                    n => Some(format!("P{p} ×{n}")),
                })
                .collect();
            parts.push(match by.is_empty() {
                true => format!("{} open", self.open.len()),
                false => format!("{} open ({})", self.open.len(), by.join(", ")),
            });
        }
        if !self.dismissed.is_empty() {
            parts.push(format!("{} dismissed", self.dismissed.len()));
        }
        if !self.unresolved.is_empty() {
            parts.push(format!("{} unresolved", self.unresolved.len()));
        }
        match parts.is_empty() {
            true => "clean".to_string(),
            false => parts.join("; "),
        }
    }
}

/// Whether `login` is the Cubic bot (`cubic-dev-ai[bot]` and kin).
pub fn is_cubic(login: &str) -> bool {
    login.to_ascii_lowercase().contains("cubic")
}

/// The first severity `text` names: `P0`–`P3` standing as a word of its
/// own (`**P1**`, `<sup>P2</sup>`, `P1:`), not inside one (`MP3`, `P10`).
pub fn severity(text: &str) -> Option<u8> {
    let b = text.as_bytes();
    (0..b.len().saturating_sub(1)).find_map(|i| {
        let before_ok = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        let after_ok = b.get(i + 2).is_none_or(|c| !c.is_ascii_alphanumeric());
        (b[i] == b'P' && (b'0'..=b'3').contains(&b[i + 1]) && before_ok && after_ok)
            .then(|| b[i + 1] - b'0')
    })
}

/// The open findings among a PR's review threads.
pub fn findings(threads: &[ReviewThread]) -> Vec<Finding> {
    threads
        .iter()
        .filter(|t| !t.resolved)
        .filter_map(|t| {
            let first = t.comments.first()?;
            is_cubic(&first.author).then(|| Finding {
                severity: severity(&first.body),
                path: t.path.clone(),
                place: t.place(),
            })
        })
        .collect()
}

/// Whether a dismissal gives its reason: words after a `: `, ` — ` or
/// ` - ` (`P1 in a.rs: false positive, the lock is held`). A path's
/// `a.rs:12` is no reason.
pub fn has_reason(item: &str) -> bool {
    [": ", " — ", " - "].iter().any(|sep| {
        item.split_once(sep)
            .is_some_and(|(_, why)| why.split_whitespace().count() >= 2)
    })
}

/// Whether a declared item is about a P1: any that doesn't name a lower
/// severity (one that names none is taken as a P1, the case it is there
/// for).
fn about_p1(item: &str) -> bool {
    matches!(severity(item), None | Some(0) | Some(1))
}

/// The `Cubic dismissals:` and `Cubic unresolved:` items in `texts` (the
/// body's notes and each comment), in the order said, without repeats.
pub fn declarations<'a>(texts: impl IntoIterator<Item = &'a str>) -> (Vec<String>, Vec<String>) {
    let (mut dismissed, mut unresolved) = (Vec::new(), Vec::new());
    for text in texts {
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next() {
            let Some((which, rest)) = declaration(line) else {
                continue;
            };
            let into = match which {
                Which::Dismissals => &mut dismissed,
                Which::Unresolved => &mut unresolved,
            };
            let mut items: Vec<String> = rest.split(';').map(str::to_string).collect();
            if rest.trim().is_empty() {
                while let Some(next) = lines.peek() {
                    let Some(item) = list_item(next) else {
                        break;
                    };
                    items.push(item.to_string());
                    lines.next();
                }
            }
            for item in items {
                let item = item.trim().trim_end_matches('.').trim();
                let none = matches!(
                    item.to_ascii_lowercase().as_str(),
                    "" | "none" | "-" | "n/a" | "nothing"
                );
                if !none && !into.iter().any(|i| i == item) {
                    into.push(item.to_string());
                }
            }
        }
    }
    (dismissed, unresolved)
}

enum Which {
    Dismissals,
    Unresolved,
}

/// A `Cubic dismissals:` / `Cubic unresolved:` line, bold or listed or
/// not, and what follows its colon.
fn declaration(line: &str) -> Option<(Which, &str)> {
    let line = line
        .trim()
        .trim_start_matches(['-', '*', '>', ' '])
        .trim_start();
    let (head, rest) = line.split_once(':')?;
    let head = head.trim().trim_matches('*').trim().to_ascii_lowercase();
    let rest = rest.trim_start_matches('*');
    match head.as_str() {
        "cubic dismissals" | "cubic dismissal" => Some((Which::Dismissals, rest)),
        "cubic unresolved" => Some((Which::Unresolved, rest)),
        _ => None,
    }
}

fn list_item(line: &str) -> Option<&str> {
    let line = line.trim_start();
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr_review::github::{Side, ThreadComment};

    fn thread(author: &str, body: &str, resolved: bool) -> ReviewThread {
        ReviewThread {
            id: "t".into(),
            path: "src/a.rs".into(),
            line: Some(3),
            start_line: None,
            original_line: Some(3),
            side: Side::Right,
            resolved,
            outdated: false,
            comments: vec![ThreadComment {
                id: "c".into(),
                author: author.into(),
                body: body.into(),
                at: String::new(),
                mine: false,
                pending: false,
            }],
        }
    }

    #[test]
    fn severity_is_read_as_a_word() {
        assert_eq!(severity("**P1**: null deref"), Some(1));
        assert_eq!(severity("<sup>P0</sup> data loss"), Some(0));
        assert_eq!(severity("P2"), Some(2));
        assert_eq!(severity("an MP3 file and P10"), None);
        assert_eq!(severity("P4 is not a level"), None);
        assert_eq!(severity("no level"), None);
    }

    #[test]
    fn only_cubics_unresolved_threads_are_findings() {
        let threads = [
            thread("cubic-dev-ai[bot]", "**P1** leak", false),
            thread("cubic-dev-ai[bot]", "**P0** gone", true),
            thread("oliver", "P0 my own note", false),
            thread("cubic-dev-ai", "style nit", false),
        ];
        let open = findings(&threads);
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].severity, Some(1));
        assert_eq!(open[0].place, "line 3");
        assert_eq!(open[1].severity, None);
    }

    #[test]
    fn declarations_are_read_inline_and_as_lists() {
        let body = "Notes\n\n**Cubic dismissals:** P1 in a.rs: false positive, the lock is held; P2 naming\nCubic unresolved:\n- P1 retry loop in b.rs\n- P1 cache key\n\nafter";
        let comment = "Cubic dismissals: none\ncubic unresolved: P1 cache key";
        let (dismissed, unresolved) = declarations([body, comment]);
        assert_eq!(
            dismissed,
            ["P1 in a.rs: false positive, the lock is held", "P2 naming"]
        );
        assert_eq!(unresolved, ["P1 retry loop in b.rs", "P1 cache key"]);
        let cubic = Cubic {
            open: vec![],
            dismissed,
            unresolved,
        };
        assert_eq!(cubic.declared_p1().len(), 3, "the P2 dismissal isn't a P1");
        assert_eq!(cubic.summary(), "2 dismissed; 2 unresolved");
    }

    #[test]
    fn the_summary_counts_by_severity() {
        let f = |s| Finding {
            severity: s,
            path: String::new(),
            place: String::new(),
        };
        let cubic = Cubic {
            open: vec![f(Some(1)), f(Some(1)), f(Some(3)), f(None)],
            ..Default::default()
        };
        assert_eq!(cubic.summary(), "4 open (P1 ×2, P3 ×1)");
        assert_eq!(Cubic::default().summary(), "clean");
    }
}
