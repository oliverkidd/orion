//! THE VERDICT: Merge, Skim, Review or Blocked, worked out from facts —
//! never from the agent's say-so. The worst tier any rule reaches wins.
//!
//! | Tier | When |
//! |---|---|
//! | Blocked | CI red; a conflict on a ready PR; an open Cubic P0, or an open P1 the agent hasn't declared; a visible change with no pictures |
//! | Review | a risky path from the contract; a Cubic P1 dismissed or unresolved; a `Hook: … skipped` trailer; a `security`, `dep` or `arch` call; over the size limit; a contract that doesn't parse |
//! | Skim | a visible change with pictures |
//! | Merge | everything else |
//!
//! Sizes leave out lockfiles, `.changeset/`, `docs/`, `*.snap` and
//! `*.json` ([`counted`]).

use super::contract::{self, Contract, Limit};
use super::cubic::Cubic;
use super::facts::Facts;
use super::pictures::Pictures;
use super::trailers::Hook;
use crate::pull_request::{CheckState, PrDetail};

/// The four tiers, mildest first: `max` is the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Merge,
    Skim,
    Review,
    Blocked,
}

impl Tier {
    pub fn word(self) -> &'static str {
        match self {
            Tier::Merge => "Merge",
            Tier::Skim => "Skim",
            Tier::Review => "Review",
            Tier::Blocked => "Blocked",
        }
    }

    /// As a run record keeps it.
    pub fn id(self) -> &'static str {
        match self {
            Tier::Merge => "merge",
            Tier::Skim => "skim",
            Tier::Review => "review",
            Tier::Blocked => "blocked",
        }
    }
}

/// What a PR is: a bug fix or a feature, for its size limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bug,
    Feature,
}

impl Kind {
    /// From a title, when the ledger doesn't say: `fix…` or `bug…` is a
    /// bug (`fix: x`, `fix(auth): x`, `bug - x`, `RIPLO-1: fix x`).
    pub fn of_title(title: &str) -> Self {
        let lower = title.trim().to_ascii_lowercase();
        let after_ids = lower
            .split_once(": ")
            .filter(|(head, _)| head.chars().any(|c| c.is_ascii_digit()) && head.contains('-'))
            .map_or(lower.as_str(), |(_, rest)| rest);
        let first: String = after_ids
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        match first.as_str() {
            "fix" | "fixes" | "fixed" | "bug" | "bugfix" | "hotfix" => Kind::Bug,
            _ => Kind::Feature,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Bug => "bug",
            Kind::Feature => "feature",
        }
    }

    fn limit(self, contract: &Contract) -> Limit {
        match self {
            Kind::Bug => contract.limits.bug,
            Kind::Feature => contract.limits.feature,
        }
    }
}

/// Why a rule reached its tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub tier: Tier,
    pub text: String,
}

/// The verdict and every reason found, the winning tier's first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub tier: Tier,
    pub reasons: Vec<Reason>,
}

impl Verdict {
    /// The reasons for the winning tier.
    pub fn why(&self) -> Vec<&str> {
        self.reasons
            .iter()
            .filter(|r| r.tier == self.tier)
            .map(|r| r.text.as_str())
            .collect()
    }

    /// The first line of the top: `**Skim**: visible change, pictures below`.
    pub fn line(&self) -> String {
        format!("**{}**: {}", self.tier.word(), self.why().join("; "))
    }
}

/// Changed lines and files, after the exclusions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Size {
    pub lines: u64,
    pub files: u64,
}

const LOCKFILES: [&str; 12] = [
    "cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "gemfile.lock",
    "poetry.lock",
    "composer.lock",
    "go.sum",
    "uv.lock",
];

/// Whether `path` counts towards the size: not a lockfile, a changeset,
/// a doc, a snapshot or JSON.
pub fn counted(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let in_dir =
        |dir: &str| lower.starts_with(&format!("{dir}/")) || lower.contains(&format!("/{dir}/"));
    !(LOCKFILES.contains(&name)
        || name.ends_with(".lock")
        || in_dir(".changeset")
        || in_dir("docs")
        || in_dir("__snapshots__")
        || name.ends_with(".snap")
        || name.ends_with(".json"))
}

/// The PR's size. GitHub lists only the first hundred files; whatever
/// lies past them is counted in full, since it can't be sorted.
pub fn size(pr: &PrDetail) -> Size {
    let (mut lines, mut files, mut listed_lines) = (0, 0, 0);
    for f in &pr.files {
        let n = f.additions + f.deletions;
        listed_lines += n;
        if counted(&f.path) {
            lines += n;
            files += 1;
        }
    }
    let unlisted_files = pr.changed_files.saturating_sub(pr.files.len() as u64);
    if unlisted_files > 0 {
        files += unlisted_files;
        lines += (pr.additions + pr.deletions).saturating_sub(listed_lines);
    }
    Size { lines, files }
}

/// The contract's paths the PR touches, by the list `of` picks.
pub fn touched<'a>(pr: &'a PrDetail, patterns: &[String]) -> Vec<&'a str> {
    pr.files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|p| contract::matches_any(patterns, p))
        .collect()
}

/// How many names a reason lists before `and N more`.
pub const NAMES_SHOWN: usize = 3;

/// `risky: a, b, c and 2 more`: the risky paths `pr` touches, as
/// `rules` name them; None when it touches none.
pub fn risky_note(pr: &PrDetail, rules: &Contract) -> Option<String> {
    let risky = touched(pr, &rules.risky_paths);
    (!risky.is_empty()).then(|| format!("risky: {}", some_of(&risky, NAMES_SHOWN)))
}

/// `a, b, c and 2 more`.
pub fn some_of(items: &[&str], at_most: usize) -> String {
    let shown: Vec<&str> = items.iter().take(at_most).copied().collect();
    match items.len().saturating_sub(at_most) {
        0 => shown.join(", "),
        more => format!("{} and {more} more", shown.join(", ")),
    }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1,234 lines in 18 files`.
pub fn size_words(size: Size) -> String {
    format!(
        "{} line{} in {} file{}",
        thousands(size.lines),
        if size.lines == 1 { "" } else { "s" },
        size.files,
        if size.files == 1 { "" } else { "s" }
    )
}

/// The verdict on `pr`, from its facts, the repo's contract (or why it
/// couldn't be read), Cubic, the commits' `Hook:` trailers, the pictures
/// and the kind.
pub fn verdict(
    facts: &Facts,
    pr: &PrDetail,
    contract: &Result<Contract, String>,
    cubic: &Cubic,
    trailers: &[Hook],
    pictures: &Pictures,
    kind: Kind,
) -> Verdict {
    let fallback = Contract::default();
    let rules = contract.as_ref().unwrap_or(&fallback);
    let mut reasons: Vec<Reason> = Vec::new();
    let mut say = |tier, text: String| reasons.push(Reason { tier, text });

    // Blocked.
    let red: Vec<&str> = pr
        .checks
        .iter()
        .filter(|c| c.state == CheckState::Failed)
        .map(|c| c.name.as_str())
        .collect();
    if !red.is_empty() {
        say(
            Tier::Blocked,
            format!("CI red ({})", some_of(&red, NAMES_SHOWN)),
        );
    }
    if !pr.answered_draft && pr.answered.conflicts == Some(true) {
        say(Tier::Blocked, format!("conflicts with {}", pr.base));
    }
    let p0 = cubic.open_at(0);
    if p0 > 0 {
        say(Tier::Blocked, format!("{p0} Cubic P0 open"));
    }
    let p1 = cubic.open_at(1);
    let declared = cubic.declared_p1();
    if p1 > declared.len() {
        say(
            Tier::Blocked,
            format!("{} Cubic P1 open", p1 - declared.len()),
        );
    }
    if pictures.visible && !pictures.shown {
        say(Tier::Blocked, "visible change with no pictures".into());
    }

    // Review.
    if let Err(why) = contract {
        say(Tier::Review, why.clone());
    }
    if let Some(note) = risky_note(pr, rules) {
        say(Tier::Review, note);
    }
    let dismissed_p1 = cubic
        .dismissed
        .iter()
        .filter(|d| declared.contains(&d.as_str()))
        .count();
    let unresolved_p1 = declared.len() - dismissed_p1;
    if dismissed_p1 > 0 {
        say(Tier::Review, format!("{dismissed_p1} Cubic P1 dismissed"));
    }
    if unresolved_p1 > 0 {
        say(Tier::Review, format!("{unresolved_p1} Cubic P1 unresolved"));
    }
    for hook in trailers.iter().filter(|h| h.skipped) {
        say(Tier::Review, format!("hook: {}", hook.text));
    }
    for call in &facts.your_calls {
        if matches!(call.tag.as_str(), "security" | "dep" | "arch") {
            say(Tier::Review, format!("[{}] call: {}", call.tag, call.call));
        }
    }
    let size = size(pr);
    let limit = kind.limit(rules);
    if size.lines > limit.lines || size.files > limit.files {
        say(
            Tier::Review,
            format!(
                "{} (over the {} limit of {} lines or {} files)",
                size_words(size),
                kind.word(),
                thousands(limit.lines),
                limit.files
            ),
        );
    }

    // Skim.
    if pictures.visible && pictures.shown {
        say(Tier::Skim, "visible change, pictures below".into());
    }

    let tier = reasons.iter().map(|r| r.tier).max().unwrap_or(Tier::Merge);
    if tier == Tier::Merge {
        let running = pr.checks.iter().any(|c| c.state == CheckState::Running);
        let text = match (running, pr.checks.is_empty()) {
            (true, _) => "nothing risky so far, checks still running",
            (false, true) => "nothing risky",
            (false, false) => "nothing risky, CI green",
        };
        reasons.push(Reason {
            tier,
            text: text.into(),
        });
    }
    // The winning tier's reasons first, the rest after, each in the order
    // found.
    reasons.sort_by_key(|r| std::cmp::Reverse(r.tier));
    Verdict { tier, reasons }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr_body::cubic::Finding;
    use crate::pr_body::facts::Call;
    use crate::pull_request::{Answered, PrCheck, PrFile};

    fn file(path: &str, lines: u64) -> PrFile {
        PrFile {
            path: path.into(),
            additions: lines,
            deletions: 0,
            change: String::new(),
        }
    }

    fn pr() -> PrDetail {
        PrDetail {
            base: "dev".into(),
            answered_draft: true,
            files: vec![file("src/a.rs", 10)],
            changed_files: 1,
            additions: 10,
            ..Default::default()
        }
    }

    struct In {
        facts: Facts,
        pr: PrDetail,
        contract: Result<Contract, String>,
        cubic: Cubic,
        trailers: Vec<Hook>,
        pictures: Pictures,
        kind: Kind,
    }

    impl In {
        fn new() -> Self {
            Self {
                facts: Facts::default(),
                pr: pr(),
                contract: Ok(Contract::default()),
                cubic: Cubic::default(),
                trailers: vec![],
                pictures: Pictures::default(),
                kind: Kind::Bug,
            }
        }

        fn run(&self) -> Verdict {
            verdict(
                &self.facts,
                &self.pr,
                &self.contract,
                &self.cubic,
                &self.trailers,
                &self.pictures,
                self.kind,
            )
        }
    }

    fn finding(p: u8) -> Finding {
        Finding {
            severity: Some(p),
            path: "a".into(),
            place: "line 1".into(),
        }
    }

    fn failed(name: &str) -> PrCheck {
        PrCheck {
            name: name.into(),
            workflow: String::new(),
            state: CheckState::Failed,
            word: "FAILURE".into(),
            started: String::new(),
            completed: String::new(),
            url: String::new(),
        }
    }

    fn assert_tier(input: &In, tier: Tier, says: &str) {
        let v = input.run();
        assert_eq!(v.tier, tier, "{v:?}");
        assert!(v.line().contains(says), "{} lacks {says}", v.line());
    }

    #[test]
    fn nothing_found_is_merge() {
        let v = In::new().run();
        assert_eq!(v.tier, Tier::Merge);
        assert_eq!(v.line(), "**Merge**: nothing risky");
        let mut i = In::new();
        i.pr.checks = vec![failed("unit")];
        i.pr.checks[0].state = CheckState::Passed;
        assert_eq!(i.run().line(), "**Merge**: nothing risky, CI green");
        i.pr.checks[0].state = CheckState::Running;
        assert_eq!(
            i.run().line(),
            "**Merge**: nothing risky so far, checks still running"
        );
    }

    // ---- Blocked, one rule each ----

    #[test]
    fn red_ci_blocks() {
        let mut i = In::new();
        i.pr.checks = vec![failed("unit")];
        assert_tier(&i, Tier::Blocked, "CI red (unit)");
    }

    #[test]
    fn a_conflict_blocks_a_ready_pr_only() {
        let mut i = In::new();
        i.pr.answered = Answered {
            conflicts: Some(true),
            checks: None,
        };
        assert_eq!(i.run().tier, Tier::Merge, "a draft's conflicts wait");
        i.pr.answered_draft = false;
        assert_tier(&i, Tier::Blocked, "conflicts with dev");
    }

    #[test]
    fn an_open_cubic_p0_blocks_even_when_declared() {
        let mut i = In::new();
        i.cubic.open = vec![finding(0)];
        i.cubic.dismissed = vec!["P0 it's fine".into()];
        assert_tier(&i, Tier::Blocked, "1 Cubic P0 open");
    }

    #[test]
    fn an_undeclared_cubic_p1_blocks() {
        let mut i = In::new();
        i.cubic.open = vec![finding(1), finding(1)];
        i.cubic.dismissed = vec!["P1 a: reason".into()];
        assert_tier(&i, Tier::Blocked, "1 Cubic P1 open");
    }

    #[test]
    fn a_visible_change_without_pictures_blocks() {
        let mut i = In::new();
        i.pictures.visible = true;
        assert_tier(&i, Tier::Blocked, "visible change with no pictures");
    }

    // ---- Review, one rule each ----

    #[test]
    fn a_risky_path_is_review() {
        let mut i = In::new();
        i.contract = Ok(Contract {
            risky_paths: vec!["src/**".into()],
            ..Contract::default()
        });
        assert_tier(&i, Tier::Review, "risky: src/a.rs");
    }

    #[test]
    fn a_declared_cubic_p1_is_review() {
        let mut i = In::new();
        i.cubic.open = vec![finding(1)];
        i.cubic.dismissed = vec!["P1 a: false positive".into()];
        assert_tier(&i, Tier::Review, "1 Cubic P1 dismissed");
        let mut i = In::new();
        i.cubic.open = vec![finding(1)];
        i.cubic.unresolved = vec!["P1 retry loop".into()];
        assert_tier(&i, Tier::Review, "1 Cubic P1 unresolved");
        let mut i = In::new();
        i.cubic.dismissed = vec!["P2 naming".into()];
        assert_eq!(i.run().tier, Tier::Merge, "a P2 dismissal is fine");
    }

    #[test]
    fn a_skipped_hook_is_review() {
        let mut i = In::new();
        i.trailers = vec![
            Hook {
                text: "lint ran".into(),
                skipped: false,
            },
            Hook {
                text: "e2e skipped (no browser)".into(),
                skipped: true,
            },
        ];
        assert_tier(&i, Tier::Review, "hook: e2e skipped (no browser)");
    }

    #[test]
    fn a_security_dep_or_arch_call_is_review() {
        for tag in ["security", "dep", "arch"] {
            let mut i = In::new();
            i.facts.your_calls = vec![Call {
                call: "token storage".into(),
                tag: tag.into(),
                ..Default::default()
            }];
            assert_tier(&i, Tier::Review, &format!("[{tag}] call: token storage"));
        }
        let mut i = In::new();
        i.facts.your_calls = vec![Call {
            tag: "copy".into(),
            ..Default::default()
        }];
        assert_eq!(i.run().tier, Tier::Merge);
    }

    #[test]
    fn the_size_limits_are_review_by_kind() {
        let mut i = In::new();
        i.pr.files = vec![file("src/a.rs", 3001)];
        assert_tier(
            &i,
            Tier::Review,
            "over the bug limit of 3,000 lines or 60 files",
        );
        i.kind = Kind::Feature;
        assert_eq!(i.run().tier, Tier::Merge);
        i.pr.files = (0..71).map(|n| file(&format!("src/{n}.rs"), 1)).collect();
        i.pr.changed_files = 71;
        assert_tier(&i, Tier::Review, "71 files");
        i.contract = Ok(Contract {
            limits: contract::Limits {
                feature: Limit {
                    lines: 10_000,
                    files: 100,
                },
                ..Default::default()
            },
            ..Contract::default()
        });
        assert_eq!(i.run().tier, Tier::Merge, "the repo's own limits win");
    }

    #[test]
    fn an_unreadable_contract_is_review() {
        let mut i = In::new();
        i.contract = Err(".orion/pr-contract.toml doesn't parse: x".into());
        assert_tier(&i, Tier::Review, "doesn't parse");
    }

    // ---- Skim ----

    #[test]
    fn a_visible_change_with_pictures_is_skim() {
        let mut i = In::new();
        i.pictures.visible = true;
        i.pictures.shown = true;
        let v = i.run();
        assert_eq!(v.line(), "**Skim**: visible change, pictures below");
    }

    // ---- precedence ----

    #[test]
    fn blocked_beats_review_beats_skim_beats_merge() {
        let mut i = In::new();
        i.pictures.visible = true;
        i.pictures.shown = true;
        assert_eq!(i.run().tier, Tier::Skim);
        i.trailers = vec![Hook {
            text: "e2e skipped".into(),
            skipped: true,
        }];
        assert_eq!(i.run().tier, Tier::Review);
        i.pr.checks = vec![failed("e2e")];
        let v = i.run();
        assert_eq!(v.tier, Tier::Blocked);
        // Only the winning tier's reasons make the line; the rest follow.
        assert_eq!(v.line(), "**Blocked**: CI red (e2e)");
        let tiers: Vec<Tier> = v.reasons.iter().map(|r| r.tier).collect();
        assert_eq!(tiers, [Tier::Blocked, Tier::Review, Tier::Skim]);
    }

    // ---- sizes ----

    #[test]
    fn sizes_leave_out_lockfiles_changesets_docs_snapshots_and_json() {
        for path in [
            "Cargo.lock",
            "web/package-lock.json",
            "pnpm-lock.yaml",
            "x/yarn.lock",
            ".changeset/brave-cats.md",
            "docs/plan.md",
            "crates/x/docs/a.md",
            "src/__snapshots__/a.tsx.snap",
            "tests/a.snap",
            "config/settings.json",
        ] {
            assert!(!counted(path), "{path} counted");
        }
        for path in ["src/docs.rs", "src/a.ts", "README.md", "src/jsonish.ts"] {
            assert!(counted(path), "{path} not counted");
        }
        let pr = PrDetail {
            files: vec![file("src/a.rs", 10), file("Cargo.lock", 500)],
            changed_files: 2,
            additions: 510,
            ..Default::default()
        };
        assert_eq!(
            size(&pr),
            Size {
                lines: 10,
                files: 1
            }
        );
    }

    #[test]
    fn files_past_the_listed_hundred_are_counted_in_full() {
        let pr = PrDetail {
            files: vec![file("Cargo.lock", 500)],
            changed_files: 3,
            additions: 600,
            ..Default::default()
        };
        assert_eq!(
            size(&pr),
            Size {
                lines: 100,
                files: 2
            }
        );
    }

    #[test]
    fn the_kind_comes_from_the_title_when_the_ledger_is_silent() {
        assert_eq!(Kind::of_title("fix: login loop"), Kind::Bug);
        assert_eq!(Kind::of_title("Fix(auth): x"), Kind::Bug);
        assert_eq!(Kind::of_title("RIPLO-12: fix the loop"), Kind::Bug);
        assert_eq!(Kind::of_title("bug - x"), Kind::Bug);
        assert_eq!(Kind::of_title("feat: export"), Kind::Feature);
        assert_eq!(Kind::of_title("prefix the names"), Kind::Feature);
    }
}
