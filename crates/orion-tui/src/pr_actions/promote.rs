//! PROMOTE: the create form's second mode, for the pull request that takes
//! the project's base branch (`dev`) into the repo's default one (`main`).
//! What goes out is every pull request merged into the base that the
//! default does not have yet — its merge commit in `origin/main..origin/dev`,
//! which is what the promotion will carry — read from GitHub's merged list
//! and git's range, the latest merge first. The title and description are
//! written from that list ([`fill`]), and a promotion already open is found
//! (`gh pr list --head dev --base main`), so Enter updates it rather than
//! opening a second one.

use std::collections::HashSet;
use std::path::Path;

use super::{gh, REQUEST_TIMEOUT};
use crate::pull_request::{run_piped, str_at};

/// How many merged pull requests GitHub is asked for, most recently
/// touched first: a page. A base far ahead of the default lists the first
/// hundred, and says so.
pub const MERGED_PAGE: usize = 100;
/// How many commits of the range git lists: a promotion bigger than this
/// is read as far as this goes.
const RANGE_MAX: usize = 5000;
/// How many titles the filled title names, and how long it may run before
/// it names fewer.
const TITLE_TITLES: usize = 3;
const TITLE_MAX: usize = 72;

/// The merged pull requests into `$base`, newest touched first: what each
/// one's row in the list says, and its merge commit, which git places on
/// one side of the default or the other.
const MERGED_INTO_QUERY: &str = "query($owner: String!, $repo: String!, $base: String!, $n: Int!) { \
    repository(owner: $owner, name: $repo) { \
    pullRequests(states: MERGED, baseRefName: $base, first: $n, orderBy: {field: UPDATED_AT, direction: DESC}) { \
    nodes { number title url mergedAt headRefName author { login } mergeCommit { oid } } } } }";

/// One pull request going out with the promotion.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoingOut {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    /// Its head branch, which may name its Linear issue (`rip-412-…`).
    pub head: String,
    /// `mergedAt`, RFC 3339.
    pub merged_at: String,
    /// The Linear issues it carries, by identifier — filled on the loop
    /// from what orion knows of Linear ([`linear_ids`]).
    pub linear: Vec<String>,
}

/// The promotion's pull request, already open: Enter updates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPromotion {
    pub number: u64,
    pub url: String,
}

/// What the Promote mode reads for a pair of branches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Promotion {
    /// Latest merge first.
    pub prs: Vec<GoingOut>,
    /// GitHub listed a whole page: there may be more than it says.
    pub capped: bool,
    /// Commits the base has that the default does not; None when git
    /// could not say.
    pub ahead: Option<usize>,
    /// When the default branch last moved — its tip's commit time, unix
    /// seconds.
    pub moved_at: Option<i64>,
    pub open: Option<OpenPromotion>,
}

/// Read what promoting `base` into `default` would carry, from `dir`'s
/// checkout: the open promotion, GitHub's merged list, and git's range.
/// `Err` when GitHub could not be asked for the list.
pub async fn read(dir: &Path, base: &str, default: &str) -> Result<Promotion, String> {
    let open = async {
        let args = [
            "pr",
            "list",
            "--head",
            base,
            "--base",
            default,
            "--state",
            "open",
            "--json",
            "number,url",
        ];
        let out = run_piped(gh(dir, &args), "", REQUEST_TIMEOUT).await.ok();
        out.and_then(|out| parse_open(&out))
    };
    let query = format!("query={MERGED_INTO_QUERY}");
    let base_var = format!("base={base}");
    let n = format!("n={MERGED_PAGE}");
    let args = [
        "api",
        "graphql",
        "-F",
        "owner={owner}",
        "-F",
        "repo={repo}",
        "-F",
        &n,
        "-f",
        &base_var,
        "-f",
        &query,
    ];
    let (open, merged) = tokio::join!(open, run_piped(gh(dir, &args), "", REQUEST_TIMEOUT));
    let listed = parse_merged(&merged?).ok_or("GitHub's answer was not a list")?;
    let (dir, base, default) = (dir.to_path_buf(), base.to_string(), default.to_string());
    let (range, moved_at) = tokio::task::spawn_blocking(move || {
        let into = crate::commit_list::branch_ref(&dir, &default);
        let from = crate::commit_list::branch_ref(&dir, &base);
        match (from, into) {
            (Some(from), Some(into)) => (range(&dir, &from, &into), moved_at(&dir, &into)),
            (None, Some(into)) => (None, moved_at(&dir, &into)),
            _ => (None, None),
        }
    })
    .await
    .unwrap_or((None, None));
    let listed_n = listed.len();
    let prs = going_out(listed, range.as_ref(), moved_at);
    Ok(Promotion {
        // A whole page all going out: the page may have stopped short.
        capped: listed_n >= MERGED_PAGE && prs.len() == listed_n,
        prs,
        ahead: range.map(|r| r.len()),
        moved_at,
        open,
    })
}

/// `gh pr list --json number,url`'s first row.
fn parse_open(json: &str) -> Option<OpenPromotion> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let first = v.as_array()?.first()?;
    Some(OpenPromotion {
        number: first.get("number")?.as_u64()?,
        url: str_at(first, "url"),
    })
}

/// [`MERGED_INTO_QUERY`]'s rows, each with its merge commit's sha.
fn parse_merged(json: &str) -> Option<Vec<(GoingOut, String)>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let nodes = v
        .pointer("/data/repository/pullRequests/nodes")?
        .as_array()?;
    Some(
        nodes
            .iter()
            .filter_map(|n| {
                let pr = GoingOut {
                    number: n.get("number")?.as_u64()?,
                    title: str_at(n, "title"),
                    url: str_at(n, "url"),
                    author: n
                        .pointer("/author/login")
                        .and_then(|l| l.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    head: str_at(n, "headRefName"),
                    merged_at: str_at(n, "mergedAt"),
                    linear: Vec::new(),
                };
                let sha = n
                    .pointer("/mergeCommit/oid")
                    .and_then(|o| o.as_str())
                    .unwrap_or_default()
                    .to_string();
                Some((pr, sha))
            })
            .collect(),
    )
}

/// The commits ref `from` has that ref `into` does not — origin's copies
/// where origin has them (`commit_list::branch_ref`): what the promotion
/// carries.
fn range(dir: &Path, from: &str, into: &str) -> Option<HashSet<String>> {
    let max = format!("--max-count={RANGE_MAX}");
    let spec = format!("{into}..{from}");
    let out = crate::git_diff::run_git(dir, &["rev-list", &max, &spec]).ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    })
}

/// When the branch at ref `at` last moved: its tip's commit time.
fn moved_at(dir: &Path, at: &str) -> Option<i64> {
    let out = crate::git_diff::run_git(dir, &["log", "-1", "--format=%ct", at]).ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// The listed pull requests the promotion carries, latest merge first:
/// those whose merge commit is in the `range` git read — or, git unable to
/// say, those merged after the default last moved.
fn going_out(
    listed: Vec<(GoingOut, String)>,
    range: Option<&HashSet<String>>,
    moved_at: Option<i64>,
) -> Vec<GoingOut> {
    let mut out: Vec<GoingOut> = listed
        .into_iter()
        .filter(|(pr, sha)| match (range, moved_at) {
            (Some(range), _) => range.contains(sha),
            (None, Some(at)) => {
                crate::pull_request::rfc3339_secs(&pr.merged_at).is_some_and(|m| m > at)
            }
            (None, None) => true,
        })
        .map(|(pr, _)| pr)
        .collect();
    // RFC 3339 stamps in UTC sort as text.
    out.sort_by(|a, b| b.merged_at.cmp(&a.merged_at));
    out
}

/// The title and description the promotion is filled with: `Release 5
/// Nov: ` and the first titles — as many of [`TITLE_TITLES`] as fit
/// [`TITLE_MAX`], one at least — then `## Going out` and a line per pull
/// request, its Linear issues after it.
pub fn fill(prs: &[GoingOut], today: &str) -> (String, String) {
    let mut title = format!("Release {today}");
    let titles: Vec<&str> = prs
        .iter()
        .map(|pr| pr.title.trim())
        .filter(|t| !t.is_empty())
        .take(TITLE_TITLES)
        .collect();
    for (i, t) in titles.iter().enumerate() {
        let next = if i == 0 {
            format!("{title}: {t}")
        } else {
            format!("{title}, {t}")
        };
        if i > 0 && next.chars().count() > TITLE_MAX {
            break;
        }
        title = next;
    }
    if prs.is_empty() {
        return (title, String::new());
    }
    let mut body = String::from("## Going out\n");
    for pr in prs {
        body.push_str(&format!("- {} (#{})", pr.title.trim(), pr.number));
        if !pr.linear.is_empty() {
            body.push_str(&format!(" · {}", pr.linear.join(", ")));
        }
        body.push('\n');
    }
    (title, body.trim_end().to_string())
}

/// Today as the filled title names it: `5 Nov`.
pub fn today() -> String {
    chrono::Local::now().format(DAY).to_string()
}

/// The day `secs` (unix seconds) fell on, here: `2 Nov`.
pub fn day_of(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|at| at.with_timezone(&chrono::Local).format(DAY).to_string())
        .unwrap_or_default()
}

/// How a day is named: `5 Nov`.
const DAY: &str = "%-d %b";

/// The Linear issues `pr` carries: the ones Linear has it attached to, then
/// the ones its title or head branch names, of the teams `list` knows.
pub fn linear_ids(list: Option<&crate::linear::LinearList>, pr: &GoingOut) -> Vec<String> {
    let Some(list) = list else {
        return Vec::new();
    };
    let mut ids: Vec<String> = list
        .list
        .iter()
        .filter(|issue| issue.prs.iter().any(|p| same_pull(&p.url, &pr.url)))
        .map(|issue| issue.identifier.clone())
        .collect();
    let keys: HashSet<String> = list
        .teams
        .iter()
        .map(|t| t.key.to_ascii_uppercase())
        .chain(list.list.iter().filter_map(|i| {
            i.identifier
                .split_once('-')
                .map(|(key, _)| key.to_ascii_uppercase())
        }))
        .filter(|k| !k.is_empty())
        .collect();
    for id in named_ids(&format!("{} {}", pr.title, pr.head), &keys) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// Whether two pull request links are the same one, however each was
/// spelled.
fn same_pull(a: &str, b: &str) -> bool {
    let key = |url: &str| url.trim().trim_end_matches('/').to_ascii_lowercase();
    !a.is_empty() && key(a) == key(b)
}

/// The issue identifiers `text` names — `RIP-412`, `rip-412-fix-pdf` —
/// of the team `keys`, upper-cased, each once, in order.
pub fn named_ids(text: &str, keys: &HashSet<String>) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts =
            chars[i].is_ascii_alphabetic() && (i == 0 || !chars[i - 1].is_ascii_alphanumeric());
        if !starts {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_alphanumeric() {
            j += 1;
        }
        let key: String = chars[i..j].iter().collect::<String>().to_ascii_uppercase();
        let digits_at = j + 1;
        let mut k = digits_at;
        while k < chars.len() && chars[k].is_ascii_digit() {
            k += 1;
        }
        let ends = chars.get(k).is_none_or(|c| !c.is_ascii_alphanumeric());
        let dashed = chars.get(j) == Some(&'-');
        if dashed && k > digits_at && ends && keys.contains(&key) {
            let id = format!("{key}-{}", chars[digits_at..k].iter().collect::<String>());
            if !out.contains(&id) {
                out.push(id);
            }
            i = k;
        } else {
            i = j.max(i + 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64, title: &str, merged_at: &str) -> GoingOut {
        GoingOut {
            number,
            title: title.into(),
            url: format!("https://github.com/o/r/pull/{number}"),
            author: "oliver".into(),
            head: format!("branch-{number}"),
            merged_at: merged_at.into(),
            linear: Vec::new(),
        }
    }

    /// What git places past the default goes out, latest merge first; git
    /// unable to say, what merged after the default last moved.
    #[test]
    fn what_goes_out_is_what_the_default_lacks() {
        let listed = vec![
            (pr(1, "Old", "2026-11-01T10:00:00Z"), "aaa".to_string()),
            (pr(3, "Newest", "2026-11-04T10:00:00Z"), "ccc".to_string()),
            (pr(2, "New", "2026-11-03T10:00:00Z"), "bbb".to_string()),
        ];
        let range: HashSet<String> = ["bbb", "ccc"].map(String::from).into();
        let out = going_out(listed.clone(), Some(&range), None);
        assert_eq!(out.iter().map(|p| p.number).collect::<Vec<_>>(), [3, 2]);
        let moved = crate::pull_request::rfc3339_secs("2026-11-02T00:00:00Z");
        let out = going_out(listed.clone(), None, moved);
        assert_eq!(out.iter().map(|p| p.number).collect::<Vec<_>>(), [3, 2]);
        assert_eq!(going_out(listed, None, None).len(), 3);
    }

    #[test]
    fn the_fill_names_the_top_titles_and_lists_every_pull_request() {
        let mut a = pr(1342, "Fix PDF header", "");
        a.linear = vec!["RIP-412".into(), "RIP-415".into()];
        let b = pr(1338, "Fix login redirect", "");
        let c = pr(1335, "Week polish", "");
        let d = pr(1330, "Something else", "");
        let (title, body) = fill(&[a, b, c, d], "5 Nov");
        assert_eq!(
            title,
            "Release 5 Nov: Fix PDF header, Fix login redirect, Week polish"
        );
        assert_eq!(
            body,
            "## Going out\n\
             - Fix PDF header (#1342) · RIP-412, RIP-415\n\
             - Fix login redirect (#1338)\n\
             - Week polish (#1335)\n\
             - Something else (#1330)"
        );
        let long = pr(1, &"x".repeat(60), "");
        let (title, _) = fill(&[long.clone(), long], "5 Nov");
        assert_eq!(
            title.matches(", ").count(),
            0,
            "one title at least: {title}"
        );
        assert_eq!(
            fill(&[], "5 Nov"),
            ("Release 5 Nov".to_string(), String::new())
        );
    }

    #[test]
    fn identifiers_are_read_off_titles_and_branches_of_known_teams() {
        let keys: HashSet<String> = ["RIP".to_string()].into();
        assert_eq!(
            named_ids(
                "Fix PDF header (RIP-412) rip-415-fonts UTF-8 rip-41x",
                &keys
            ),
            ["RIP-412", "RIP-415"]
        );
        assert!(named_ids("ENG-12 trip-3", &keys).is_empty());
    }

    #[test]
    fn the_answers_parse() {
        assert_eq!(
            parse_open(r#"[{"number":1350,"url":"https://github.com/o/r/pull/1350"}]"#),
            Some(OpenPromotion {
                number: 1350,
                url: "https://github.com/o/r/pull/1350".into()
            })
        );
        assert_eq!(parse_open("[]"), None);
        let json = r#"{"data":{"repository":{"pullRequests":{"nodes":[
            {"number":7,"title":"Add","url":"u","mergedAt":"2026-11-04T10:00:00Z",
             "headRefName":"rip-9-add","author":{"login":"sam"},"mergeCommit":{"oid":"abc"}}]}}}}"#;
        let rows = parse_merged(json).unwrap();
        assert_eq!(rows[0].0.author, "sam");
        assert_eq!(rows[0].1, "abc");
    }
}
