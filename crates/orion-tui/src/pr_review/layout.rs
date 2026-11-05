//! A file's diff with its review threads set under their lines — the PR
//! REVIEW's right pane, top to bottom ([`items`]) — and the stops its
//! cursor walks ([`Stop`]): every line of code, and every thread.
//!
//! A thread on the new side sits under the line of that number on the
//! new side; one on the old side (a removed line) under that removed
//! line. A thread GitHub no longer places — a push moved the code out
//! from under it, or it is on the whole file — sits at the top of the
//! file, named by where it was written.

use super::github::{ReviewThread, Side};
use crate::diff_doc::{DiffDoc, DocLine, Sign};

/// One thing in the pane, in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// Line `i` of the file's diff.
    Line(usize),
    /// Thread `i` of the screen's threads.
    Thread(usize),
    /// The comment being written.
    Composer,
}

/// Where the cursor rests: a line of code, or a thread, by id — so it
/// stays on it as threads come and go around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    Line(usize),
    Thread(String),
}

/// Where the comment being written goes: under a line — after the
/// threads already on it — or at the foot of a thread it replies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerAt {
    Line(usize),
    Thread(usize),
}

/// The side and number line `i` of `doc` is commented on by: a removed
/// line by its old number, any other by its new one. None for what is not
/// a line of code.
pub fn side_line(doc: &DiffDoc, i: usize) -> Option<(Side, u32)> {
    match doc.lines.get(i)? {
        DocLine::Code {
            sign: Sign::Del,
            old: Some(old),
            ..
        } => Some((Side::Left, *old)),
        DocLine::Code { new: Some(new), .. } => Some((Side::Right, *new)),
        _ => None,
    }
}

/// Whether line `i` is a line of code — one the cursor rests on.
pub fn is_code(doc: &DiffDoc, i: usize) -> bool {
    matches!(doc.lines.get(i), Some(DocLine::Code { .. }))
}

/// Whether lines `a` and `b` are in one hunk: nothing but code between
/// them. A comment on a run of lines can't cross from one to the next.
pub fn same_hunk(doc: &DiffDoc, a: usize, b: usize) -> bool {
    (a.min(b)..=a.max(b)).all(|i| is_code(doc, i))
}

/// The line of `doc` thread `t` sits under; None for the top of the file.
pub fn anchor(doc: &DiffDoc, t: &ReviewThread) -> Option<usize> {
    let line = t.line?;
    let at = |want: Side, exact: bool| {
        doc.lines.iter().position(|l| match l {
            DocLine::Code { sign, old, new, .. } => match want {
                Side::Right => *new == Some(line) && *sign != Sign::Del,
                Side::Left if exact => *old == Some(line) && *sign == Sign::Del,
                Side::Left => *old == Some(line) && *sign == Sign::Context,
            },
            _ => false,
        })
    };
    match t.side {
        Side::Right => at(Side::Right, true),
        Side::Left => at(Side::Left, true).or_else(|| at(Side::Left, false)),
    }
}

/// Everything in the pane for the file at `path` of `doc`: the threads
/// GitHub doesn't place first, then each line with the threads on it under
/// it, oldest first — and the comment being written where it goes.
pub fn items(
    doc: &DiffDoc,
    path: &str,
    threads: &[ReviewThread],
    composer: Option<ComposerAt>,
) -> Vec<Item> {
    let mut placed: Vec<(Option<usize>, usize)> = threads
        .iter()
        .enumerate()
        .filter(|(_, t)| t.path == path)
        .map(|(i, t)| (anchor(doc, t), i))
        .collect();
    let first_said = |i: usize| threads[i].comments.first().map(|c| c.at.as_str());
    placed.sort_by(|(a, i), (b, j)| (a, first_said(*i)).cmp(&(b, first_said(*j))));
    let mut out = Vec::with_capacity(doc.lines.len() + placed.len() + 1);
    let mut next = placed.iter().peekable();
    let push_thread = |out: &mut Vec<Item>, i: usize| {
        out.push(Item::Thread(i));
        if composer == Some(ComposerAt::Thread(i)) {
            out.push(Item::Composer);
        }
    };
    while let Some((_, i)) = next.next_if(|(at, _)| at.is_none()) {
        push_thread(&mut out, *i);
    }
    for line in 0..doc.lines.len() {
        out.push(Item::Line(line));
        while let Some((_, i)) = next.next_if(|(at, _)| *at == Some(line)) {
            push_thread(&mut out, *i);
        }
        if composer == Some(ComposerAt::Line(line)) {
            out.push(Item::Composer);
        }
    }
    out
}

/// The stop `item` is, when it is one: a line of code, or a thread with
/// something said in it.
pub fn stop_of(item: Item, doc: &DiffDoc, threads: &[ReviewThread]) -> Option<Stop> {
    match item {
        Item::Line(i) if is_code(doc, i) => Some(Stop::Line(i)),
        Item::Thread(i) => threads
            .get(i)
            .filter(|t| !t.comments.is_empty())
            .map(|t| Stop::Thread(t.id.clone())),
        _ => None,
    }
}

/// Every stop among `items`, in order.
pub fn stops(items: &[Item], doc: &DiffDoc, threads: &[ReviewThread]) -> Vec<Stop> {
    items
        .iter()
        .filter_map(|item| stop_of(*item, doc, threads))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pr_review::github::ThreadComment;

    const DIFF: &str = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
        @@ -1,3 +1,3 @@\n one\n-two\n+deux\n three\n";

    fn thread(id: &str, line: Option<u32>, side: Side, at: &str) -> ReviewThread {
        ReviewThread {
            id: id.into(),
            path: "a.rs".into(),
            line,
            start_line: None,
            original_line: line.or(Some(9)),
            side,
            resolved: false,
            outdated: line.is_none(),
            comments: vec![ThreadComment {
                id: format!("{id}-c"),
                author: "ada".into(),
                body: "hm".into(),
                at: at.into(),
                mine: false,
                pending: false,
            }],
        }
    }

    fn doc() -> DiffDoc {
        DiffDoc::build(&[], Some("a.rs"), DIFF)
    }

    /// The index of the line of code reading `text`.
    fn line_of(doc: &DiffDoc, text: &str) -> usize {
        doc.lines
            .iter()
            .position(|l| match l {
                DocLine::Code { runs, .. } => {
                    runs.iter().map(|(_, t)| t.as_str()).collect::<String>() == text
                }
                _ => false,
            })
            .unwrap()
    }

    #[test]
    fn a_thread_sits_under_its_line_on_its_side() {
        let doc = doc();
        let two = line_of(&doc, "two");
        let deux = line_of(&doc, "deux");
        let three = line_of(&doc, "three");
        assert_eq!(side_line(&doc, two), Some((Side::Left, 2)));
        assert_eq!(side_line(&doc, deux), Some((Side::Right, 2)));
        assert_eq!(side_line(&doc, three), Some((Side::Right, 3)));
        assert_eq!(
            anchor(&doc, &thread("L", Some(2), Side::Left, "")),
            Some(two)
        );
        assert_eq!(
            anchor(&doc, &thread("R", Some(2), Side::Right, "")),
            Some(deux)
        );
        // The old side of an unchanged line is that line.
        assert_eq!(
            anchor(&doc, &thread("C", Some(3), Side::Left, "")),
            Some(three)
        );
        assert_eq!(anchor(&doc, &thread("O", None, Side::Right, "")), None);
    }

    #[test]
    fn the_pane_reads_top_down_with_threads_under_their_lines() {
        let doc = doc();
        let deux = line_of(&doc, "deux");
        let threads = vec![
            thread("later", Some(2), Side::Right, "2026-11-05T12:00:00Z"),
            thread("gone", None, Side::Right, "2026-11-01T00:00:00Z"),
            thread("first", Some(2), Side::Right, "2026-11-05T10:00:00Z"),
        ];
        let laid = items(&doc, "a.rs", &threads, Some(ComposerAt::Line(deux)));
        assert_eq!(laid[0], Item::Thread(1), "the outdated one on top");
        let at = laid.iter().position(|i| *i == Item::Line(deux)).unwrap();
        assert_eq!(
            &laid[at..at + 4],
            &[
                Item::Line(deux),
                Item::Thread(2),
                Item::Thread(0),
                Item::Composer
            ],
            "oldest first, the new comment after them"
        );
        let walked = stops(&laid, &doc, &threads);
        assert_eq!(walked[0], Stop::Thread("gone".into()));
        assert!(walked.contains(&Stop::Line(deux)));
        // The hunk's own line is not a stop.
        assert!(!walked.contains(&Stop::Line(0)));
        // Another file's threads stay out.
        assert!(items(&doc, "b.rs", &threads, None)
            .iter()
            .all(|i| matches!(i, Item::Line(_))));
    }

    #[test]
    fn a_run_of_lines_stays_in_its_hunk() {
        let text = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
            @@ -1,2 +1,2 @@\n one\n-two\n+deux\n@@ -10,2 +10,2 @@\n ten\n-eleven\n+onze\n";
        let doc = DiffDoc::build(&[], Some("a.rs"), text);
        let one = line_of(&doc, "one");
        let deux = line_of(&doc, "deux");
        let ten = line_of(&doc, "ten");
        assert!(same_hunk(&doc, one, deux));
        assert!(same_hunk(&doc, deux, one));
        assert!(!same_hunk(&doc, deux, ten));
    }
}
