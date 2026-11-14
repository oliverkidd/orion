//! THE TOP of a factory pull request's body, written by orion:
//!
//! 1. the verdict with its reason (`**Skim**: visible change, pictures
//!    below`);
//! 2. eight rows: What users get, Risk, Proof, Gaps, Your calls, Cubic,
//!    Rule files, Knowledge;
//! 3. the first picture grid, when there is one;
//! 4. the agent's own description, folded inside `<details>`;
//! 5. the hidden facts block.
//!
//! At most [`FOLD_WORDS`] words sit above the fold (1 and 2): the longest
//! cells are cut until they fit. The whole top sits between
//! `<!-- orion:top -->` and `<!-- orion:end -->`; the notes between their
//! own markers inside it. Whatever is outside the top — another bot's
//! summary added below it — is kept where it is, and a body rendered
//! again from its own output comes out byte for byte the same.

use super::facts::{self, Facts};
use super::verdict::Verdict;

pub const FOLD_WORDS: usize = 180;

const TOP: &str = "<!-- orion:top -->";
const END: &str = "<!-- orion:end -->";
const NOTES: &str = "<!-- orion:notes -->";
const NOTES_END: &str = "<!-- orion:notes-end -->";

/// Where other tools' blocks start in a body orion hasn't rendered yet:
/// they stay below the top rather than fold into the notes.
const FOREIGN: [&str; 3] = [
    "<!-- This is an auto-generated description by cubic",
    "<!-- codesmith:",
    "<!-- cubic:",
];

/// The rows' words, worked out by the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rows {
    pub risk: String,
    pub cubic: String,
    pub rule_files: String,
    pub knowledge: String,
}

/// The body with orion's top written over whatever top it had.
pub fn render(
    body: &str,
    facts: &Facts,
    verdict: &Verdict,
    rows: &Rows,
    grid: Option<&str>,
) -> String {
    let (notes, tail) = split(body);
    let mut out = String::new();
    out.push_str(TOP);
    out.push('\n');
    out.push_str(&above_fold(facts, verdict, rows));
    if let Some(url) = grid {
        out.push_str(&format!("\n![Pictures]({url})\n"));
    }
    if !notes.is_empty() {
        out.push_str(&format!(
            "\n<details><summary>Notes</summary>\n{NOTES}\n\n{notes}\n\n{NOTES_END}\n</details>\n"
        ));
    }
    out.push('\n');
    out.push_str(&facts::block(facts));
    out.push('\n');
    out.push_str(END);
    out.push('\n');
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(&tail);
        out.push('\n');
    }
    out
}

/// Whether two bodies say the same once GitHub's line endings and
/// trailing space are set aside: only a real change is written.
pub fn same(a: &str, b: &str) -> bool {
    normal(a).trim_end() == normal(b).trim_end()
}

fn normal(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// The agent's notes and the foreign tail of `body`.
fn split(body: &str) -> (String, String) {
    let body = normal(body);
    let (notes, tail) = match (body.find(TOP), body.find(END)) {
        (Some(top), Some(end)) if top < end => {
            let before = body[..top].trim();
            let inside = &body[top..end];
            let kept = match (inside.find(NOTES), inside.find(NOTES_END)) {
                (Some(a), Some(b)) if a < b => inside[a + NOTES.len()..b].trim(),
                _ => "",
            };
            let notes = [before, kept]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            (notes, body[end + END.len()..].to_string())
        }
        _ => match FOREIGN.iter().filter_map(|m| body.find(m)).min() {
            Some(at) => (body[..at].to_string(), body[at..].to_string()),
            None => (body.clone(), String::new()),
        },
    };
    (
        facts::strip_blocks(&notes).trim().to_string(),
        facts::strip_blocks(&tail).trim().to_string(),
    )
}

/// One table cell: one line, no pipes to break the table.
fn cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

fn list(items: &[String], empty: &str) -> String {
    let items: Vec<&str> = items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    match items.is_empty() {
        true => empty.to_string(),
        false => items.join("; "),
    }
}

/// The calls, `copy`/`ux` ones included: `[copy] Button text: Sign in
/// (not Log in)`.
fn calls(facts: &Facts) -> String {
    let each: Vec<String> = facts
        .your_calls
        .iter()
        .map(|c| {
            let alt = match c.alternative.trim() {
                "" => String::new(),
                alt => format!(" (not {alt})"),
            };
            format!("[{}] {}: {}{alt}", c.tag, c.call.trim(), c.picked.trim())
        })
        .collect();
    list(&each, "none")
}

/// The verdict line and the table, cut to [`FOLD_WORDS`].
fn above_fold(facts: &Facts, verdict: &Verdict, rows: &Rows) -> String {
    let line = cell(&verdict.line());
    let labels = [
        "What users get",
        "Risk",
        "Proof",
        "Gaps",
        "Your calls",
        "Cubic",
        "Rule files",
        "Knowledge",
    ];
    let mut cells: Vec<Vec<String>> = [
        cell(&facts.what_users_get),
        cell(&rows.risk),
        cell(&list(&facts.proof, "none given")),
        cell(&list(&facts.gaps, "none")),
        cell(&calls(facts)),
        cell(&rows.cubic),
        cell(&rows.rule_files),
        cell(&rows.knowledge),
    ]
    .into_iter()
    .map(|c| c.split(' ').map(str::to_string).collect())
    .collect();
    let fixed = words(&line) + labels.iter().map(|l| words(l)).sum::<usize>();
    let mut cut = vec![false; cells.len()];
    loop {
        let total = fixed + cells.iter().map(Vec::len).sum::<usize>();
        if total <= FOLD_WORDS {
            break;
        }
        let Some(longest) = (0..cells.len()).max_by_key(|&i| cells[i].len()) else {
            break;
        };
        if cells[longest].len() <= 1 {
            break;
        }
        cells[longest].pop();
        cut[longest] = true;
    }
    let mut out = format!("{line}\n\n| | |\n|---|---|\n");
    for (i, label) in labels.iter().enumerate() {
        let mut text = cells[i].join(" ");
        if cut[i] {
            text.push('…');
        }
        out.push_str(&format!("| **{label}** | {text} |\n"));
    }
    out
}

fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr_body::facts::Call;
    use crate::pr_body::verdict::{Reason, Tier};

    fn facts() -> Facts {
        Facts {
            what_users_get: "Login no longer loops.".into(),
            proof: vec!["unit test".into()],
            your_calls: vec![Call {
                call: "Button text".into(),
                picked: "Sign in".into(),
                alternative: "Log in".into(),
                tag: "copy".into(),
            }],
            ..Default::default()
        }
    }

    fn verdict() -> Verdict {
        Verdict {
            tier: Tier::Skim,
            reasons: vec![Reason {
                tier: Tier::Skim,
                text: "visible change, pictures below".into(),
            }],
        }
    }

    fn rows() -> Rows {
        Rows {
            risk: "10 lines in 1 file; on plan".into(),
            cubic: "clean".into(),
            rule_files: "none touched".into(),
            knowledge: "none".into(),
        }
    }

    const AGENT: &str = "## What and why\nFixes RIPLO-1.\n";

    #[test]
    fn the_top_is_laid_out_in_order() {
        let out = render(
            AGENT,
            &facts(),
            &verdict(),
            &rows(),
            Some("https://x/g.png"),
        );
        let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s} missing:\n{out}"));
        assert!(out.starts_with("<!-- orion:top -->\n**Skim**: visible change, pictures below\n"));
        assert!(at("| **What users get** | Login no longer loops. |") < at("| **Risk** |"));
        assert!(at("| **Knowledge** |") < at("![Pictures](https://x/g.png)"));
        assert!(at("![Pictures]") < at("<details><summary>Notes</summary>"));
        assert!(at("## What and why") < at("</details>"));
        assert!(at("</details>") < at("<!-- orion:facts"));
        assert!(out.contains("| **Your calls** | [copy] Button text: Sign in (not Log in) |"));
        assert_eq!(facts::from_body(&out), Some(facts()));
    }

    #[test]
    fn rendering_its_own_output_changes_nothing() {
        let once = render(AGENT, &facts(), &verdict(), &rows(), None);
        let twice = render(&once, &facts(), &verdict(), &rows(), None);
        assert_eq!(once, twice);
        assert!(same(&once, &twice.replace('\n', "\r\n")));
        assert_eq!(once.matches("## What and why").count(), 1);
    }

    #[test]
    fn a_new_verdict_replaces_the_old_top_and_keeps_the_notes() {
        let once = render(AGENT, &facts(), &verdict(), &rows(), None);
        let merge = Verdict {
            tier: Tier::Merge,
            reasons: vec![Reason {
                tier: Tier::Merge,
                text: "nothing risky, CI green".into(),
            }],
        };
        let again = render(&once, &facts(), &merge, &rows(), None);
        assert!(again.contains("**Merge**: nothing risky"));
        assert!(!again.contains("**Skim**"));
        assert_eq!(again.matches("## What and why").count(), 1);
        assert_eq!(again.matches("<!-- orion:top -->").count(), 1);
    }

    #[test]
    fn another_bots_summary_stays_below_the_top() {
        let body = format!(
            "{AGENT}\n<!-- This is an auto-generated description by cubic. -->\n## Summary by cubic\nstuff"
        );
        let once = render(&body, &facts(), &verdict(), &rows(), None);
        let end = once.find("<!-- orion:end -->").unwrap();
        assert!(once.find("## Summary by cubic").unwrap() > end);
        // Cubic rewriting its summary below the top leaves the top alone.
        let edited = once.replace("stuff", "new stuff");
        let again = render(&edited, &facts(), &verdict(), &rows(), None);
        assert_eq!(again, edited);
    }

    #[test]
    fn the_fold_holds_at_most_180_words() {
        let mut long = facts();
        long.what_users_get = "word ".repeat(150);
        long.proof = vec!["proof ".repeat(80)];
        let out = render("", &long, &verdict(), &rows(), None);
        let fold = &out[..out.find("<!-- orion:facts").unwrap()];
        let counted = fold
            .lines()
            .filter(|l| !l.starts_with("<!--") && !l.starts_with("|---"))
            .flat_map(|l| l.split_whitespace())
            .filter(|w| *w != "|" && *w != "**")
            .count();
        // Labels are bold-wrapped, so `**What` and `get**` are one word each.
        assert!(counted <= FOLD_WORDS, "{counted} words:\n{fold}");
        assert!(fold.contains("word…"));
        assert!(!out.contains("<details>"), "an empty body has no notes");
    }

    #[test]
    fn a_pipe_or_newline_cannot_break_the_table() {
        let mut f = facts();
        f.what_users_get = "a | b\nc".into();
        let out = render("", &f, &verdict(), &rows(), None);
        assert!(out.contains("| **What users get** | a \\| b c |"));
    }
}
