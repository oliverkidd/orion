//! THE TOP of a factory pull request's body, written by orion:
//!
//! 1. the verdict with its reason (`**Skim**: visible change, pictures
//!    below`);
//! 2. what users get, in one plain sentence;
//! 3. the table, each row a dot and at most twelve words
//!    (`super::rows`): Size, Plan, Your calls, Pictures, Cubic, Gaps, and
//!    Rule files and Knowledge when something was touched;
//! 4. the first picture grid, when there is one;
//! 5. the Notes fold (`<details>`): what a row had to say past its cell,
//!    under the row's name, the proof the checks don't show, then the
//!    agent's own description;
//! 6. the hidden facts block.
//!
//! The whole top sits between `<!-- orion:top -->` and `<!-- orion:end
//! -->`; the agent's notes between their own markers inside the fold.
//! Whatever is outside the top — another bot's summary added below it —
//! is kept where it is, and a body rendered again from its own output
//! comes out byte for byte the same.

use super::facts::{self, Facts};
use super::rows::Top;
use super::verdict::Verdict;

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

/// The body with orion's top written over whatever top it had.
pub fn render(
    body: &str,
    facts: &Facts,
    verdict: &Verdict,
    top: &Top,
    grid: Option<&str>,
) -> String {
    let (notes, tail) = split(body);
    let mut out = String::new();
    out.push_str(TOP);
    out.push('\n');
    out.push_str(&above_fold(verdict, top));
    if let Some(url) = grid {
        out.push_str(&format!("\n![Pictures]({url})\n"));
    }
    if !notes.is_empty() || !top.notes.is_empty() {
        out.push_str("\n<details><summary>Notes</summary>\n");
        for note in &top.notes {
            out.push_str(&format!(
                "\n#### {}\n\n{}\n",
                note.heading,
                note.text.trim()
            ));
        }
        if !notes.is_empty() {
            out.push_str(&format!("\n{NOTES}\n\n{notes}\n\n{NOTES_END}\n"));
        }
        out.push_str("</details>\n");
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

/// The verdict line, the sentence and the table.
fn above_fold(verdict: &Verdict, top: &Top) -> String {
    let mut out = format!("{}\n\n", cell(&verdict.line()));
    let sentence = cell(&top.sentence);
    if !sentence.is_empty() {
        out.push_str(&format!("{sentence}\n\n"));
    }
    out.push_str("| | |\n|---|---|\n");
    for row in &top.rows {
        out.push_str(&format!(
            "| {} **{}** | {} |\n",
            row.dot.mark(),
            row.label,
            cell(&row.cell)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr_body::facts::Call;
    use crate::pr_body::rows::{Dot, Note, Row};
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
                label: "sign-in wording".into(),
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

    fn row(dot: Dot, label: &'static str, cell: &str) -> Row {
        Row {
            dot,
            label,
            cell: cell.into(),
        }
    }

    fn top() -> Top {
        Top {
            sentence: "Login no longer loops.".into(),
            rows: vec![
                row(Dot::Green, "Size", "+120 −30 in 4 files, built in 2h 10m"),
                row(Dot::Green, "Plan", "on plan"),
                row(Dot::Amber, "Your calls", "1: sign-in wording"),
                row(Dot::Green, "Pictures", "before and after, below"),
                row(Dot::Green, "Cubic", "clean"),
                row(Dot::Clear, "Gaps", "none"),
            ],
            notes: vec![Note {
                heading: "Your calls",
                text: "- [copy] Button text: Sign in (not Log in)".into(),
            }],
        }
    }

    const AGENT: &str = "## What and why\nFixes RIPLO-1.\n";

    #[test]
    fn the_top_is_laid_out_in_order() {
        let out = render(AGENT, &facts(), &verdict(), &top(), Some("https://x/g.png"));
        let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s} missing:\n{out}"));
        assert!(out.starts_with(
            "<!-- orion:top -->\n**Skim**: visible change, pictures below\n\n\
             Login no longer loops.\n\n| | |\n|---|---|\n\
             | 🟢 **Size** | +120 −30 in 4 files, built in 2h 10m |\n\
             | 🟢 **Plan** | on plan |\n\
             | 🟡 **Your calls** | 1: sign-in wording |\n"
        ));
        assert!(at("| ⚪ **Gaps** | none |") < at("![Pictures](https://x/g.png)"));
        assert!(at("![Pictures]") < at("<details><summary>Notes</summary>"));
        assert!(at("<details>") < at("#### Your calls\n\n- [copy] Button text"));
        assert!(at("#### Your calls") < at("## What and why"));
        assert!(at("## What and why") < at("</details>"));
        assert!(at("</details>") < at("<!-- orion:facts"));
        assert!(!out.contains("What users get"), "a sentence, not a row");
        assert!(!out.contains("unit test |"), "proof is no row");
        assert_eq!(facts::from_body(&out), Some(facts()));
    }

    #[test]
    fn every_row_starts_with_a_dot() {
        let out = render("", &facts(), &verdict(), &top(), None);
        let rows: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| |"))
            .collect();
        assert_eq!(rows.len(), 6);
        for line in rows {
            assert!(
                ["🟢", "🟡", "🔴", "⚪"]
                    .iter()
                    .any(|d| line.starts_with(&format!("| {d} **"))),
                "{line}"
            );
        }
    }

    #[test]
    fn rendering_its_own_output_changes_nothing() {
        let once = render(AGENT, &facts(), &verdict(), &top(), None);
        let twice = render(&once, &facts(), &verdict(), &top(), None);
        assert_eq!(once, twice);
        assert!(same(&once, &twice.replace('\n', "\r\n")));
        assert_eq!(once.matches("## What and why").count(), 1);
        assert_eq!(once.matches("#### Your calls").count(), 1);
        // Without notes of the agent's, the row notes alone stay single.
        let once = render("", &facts(), &verdict(), &top(), None);
        assert_eq!(render(&once, &facts(), &verdict(), &top(), None), once);
    }

    #[test]
    fn a_new_verdict_replaces_the_old_top_and_keeps_the_notes() {
        let once = render(AGENT, &facts(), &verdict(), &top(), None);
        let merge = Verdict {
            tier: Tier::Merge,
            reasons: vec![Reason {
                tier: Tier::Merge,
                text: "nothing risky, CI green".into(),
            }],
        };
        let mut calm = top();
        calm.notes.clear();
        let again = render(&once, &facts(), &merge, &calm, None);
        assert!(again.contains("**Merge**: nothing risky"));
        assert!(!again.contains("**Skim**"));
        assert!(!again.contains("#### Your calls"), "row notes are orion's");
        assert_eq!(again.matches("## What and why").count(), 1);
        assert_eq!(again.matches("<!-- orion:top -->").count(), 1);
    }

    #[test]
    fn another_bots_summary_stays_below_the_top() {
        let body = format!(
            "{AGENT}\n<!-- This is an auto-generated description by cubic. -->\n## Summary by cubic\nstuff"
        );
        let once = render(&body, &facts(), &verdict(), &top(), None);
        let end = once.find("<!-- orion:end -->").unwrap();
        assert!(once.find("## Summary by cubic").unwrap() > end);
        // Cubic rewriting its summary below the top leaves the top alone.
        let edited = once.replace("stuff", "new stuff");
        let again = render(&edited, &facts(), &verdict(), &top(), None);
        assert_eq!(again, edited);
    }

    #[test]
    fn an_empty_body_without_row_notes_has_no_fold() {
        let mut bare = top();
        bare.notes.clear();
        let out = render("", &facts(), &verdict(), &bare, None);
        assert!(!out.contains("<details>"), "{out}");
    }

    #[test]
    fn a_pipe_or_newline_cannot_break_the_table() {
        let mut t = top();
        t.rows[1].cell = "a | b\nc".into();
        t.sentence = "x\ny".into();
        let out = render("", &facts(), &verdict(), &t, None);
        assert!(out.contains("| 🟢 **Plan** | a \\| b c |"), "{out}");
        assert!(out.contains("\n\nx y\n\n"));
    }
}
