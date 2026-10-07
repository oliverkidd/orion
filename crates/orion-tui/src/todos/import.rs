//! Pasting a list in: an indented Markdown list, as a notes app copies
//! one out, read into groups and items ([`parse`]). A bullet with bullets
//! under it is a group, one without is an item; a line that is no bullet
//! (a `Todo` heading) is passed over. A bullet indented further than the
//! one before it is under it — however far, a tab counting four spaces —
//! and one back at an earlier bullet's indent is that bullet's sibling,
//! so a list indented as a whole, or by two spaces, reads the same. A
//! GitHub task box (`[ ]`, `[x]`) is taken off with the bullet.

/// One bullet of a pasted list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Group { name: String, children: Vec<Node> },
    Item(String),
}

/// What top-level items with no group around them go into.
pub const INBOX: &str = "Inbox";

/// The bullets of `text` as a tree, top-level items gathered into one
/// [`INBOX`] group where the first of them stood.
pub fn parse(text: &str) -> Vec<Node> {
    // Each bullet's level: how many of the bullets still open above it
    // are indented less than it is.
    let mut open: Vec<usize> = Vec::new();
    let mut flat: Vec<(usize, String)> = Vec::new();
    for (indent, text) in text.lines().filter_map(bullet) {
        while open.last().is_some_and(|i| *i >= indent) {
            open.pop();
        }
        flat.push((open.len().min(MAX_LEVEL), text));
        open.push(indent);
    }
    let mut at = 0;
    let nodes = build(&flat, &mut at, 0);
    let mut out = Vec::with_capacity(nodes.len());
    let mut inbox: Option<usize> = None;
    for node in nodes {
        match node {
            Node::Item(text) => {
                let i = *inbox.get_or_insert_with(|| {
                    out.push(Node::Group {
                        name: INBOX.to_string(),
                        children: Vec::new(),
                    });
                    out.len() - 1
                });
                if let Node::Group { children, .. } = &mut out[i] {
                    children.push(Node::Item(text));
                }
            }
            group => out.push(group),
        }
    }
    out
}

/// How deep a pasted list nests before deeper bullets are taken as this
/// deep: the walk down it is recursive.
const MAX_LEVEL: usize = 16;

/// The nodes at `level` from `flat[*at]` on, each with the deeper ones
/// right after it as its children.
fn build(flat: &[(usize, String)], at: &mut usize, level: usize) -> Vec<Node> {
    let mut out = Vec::new();
    while let Some((l, text)) = flat.get(*at) {
        if *l < level {
            break;
        }
        *at += 1;
        let children = build(flat, at, level + 1);
        out.push(if children.is_empty() {
            Node::Item(text.clone())
        } else {
            Node::Group {
                name: text.clone(),
                children,
            }
        });
    }
    out
}

/// The bullets a line can start with: Markdown's, and the dots a notes
/// app or a word processor copies out.
const BULLETS: [&str; 6] = ["- ", "* ", "+ ", "• ", "◦ ", "▪ "];

/// A bullet line's indent, in columns — a tab four — and its text: the
/// bullet, a task box and a surrounding `**…**` taken off. None for any
/// other line.
fn bullet(line: &str) -> Option<(usize, String)> {
    let body = line.trim_start_matches([' ', '\t']);
    let lead = &line[..line.len() - body.len()];
    let indent = lead.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum();
    let rest = BULLETS.iter().find_map(|b| body.strip_prefix(b))?;
    let text = tidy(rest);
    (!text.is_empty()).then(|| (indent, text.to_string()))
}

/// `text` with a task box and a surrounding `**…**` taken off.
fn tidy(text: &str) -> &str {
    let mut text = text.trim();
    if let Some(after) = ["[ ] ", "[x] ", "[X] "]
        .iter()
        .find_map(|b| text.strip_prefix(b))
    {
        text = after.trim();
    }
    if let Some(inner) = text
        .strip_prefix("**")
        .and_then(|t| t.strip_suffix("**"))
        .filter(|t| !t.is_empty())
    {
        text = inner.trim();
    }
    text
}

/// Each line of `text` that has anything on it, as one item: its bullet,
/// if it has one, a task box, a surrounding `**…**` and a `;` closing it
/// taken off. What a paste of lines into a new item's field adds.
pub fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            let line = line.trim();
            let line = BULLETS
                .iter()
                .find_map(|b| line.strip_prefix(b))
                .unwrap_or(line);
            tidy(line.trim_end_matches(';')).to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

/// How many items `nodes` hold, nested ones included.
pub fn item_count(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            Node::Item(_) => 1,
            Node::Group { children, .. } => item_count(children),
        })
        .sum()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The list the feature was built for, as Apple Notes copies it out.
    pub(crate) const FIXTURE: &str = "Todo
- Emails
    - run plan
    - setup resend
    - setup templates
    - change all emails over
    - check lists created and hooks (ideally with local harness to check emails without having to deploy (even if just logs)
- **link sharing**
    - run desctructive migration to fix old links: docs/2026-09-29-share-link-doors/contract-follow-up.md [MAYBE NOT NEEDED]
- **UI**
    - get Zack to s
    - table panel fix to be small chips
    - workOS login pages etc
    - Later stuff
        - seeem to be reading the deck an awful lot. should we not have a read slide tool for just this slide requests?
        - insert from previous decks ui is a bit confusing could move to grid of just matches asdiscussed with toby
        - file history modal a bit too big / same vein as above could cleanup
        - learn about riplo ombaording card is not env scoped it should be
        - output design style to figma
- **MCP fixes**
    - MCP insert lsides from previoius decks and deck search etc
    - Rethink templates in scope of MCP creating custo m teampltes (links to deck review)
        - get toby new closing page in default themes
        - not able to delete stuff on later templates from master properly (block enable/disable format)
        - Not able to make custom layoyuts
        - import from ppt detection (theme too)
    - storyline prompt
        - review when its actually used
        - use toby message .md file
    - low desnity component liek claude
        - Review toby outbound decks for componentsi
        - Compare to AutoPresent slide library to see if any missing
    - Long tail improve components
        - server render best of bad bunch vs returning nothing
        - get toby messages in slack and quirk fixes message in
        - look at bhavna deck comments
- **Side quests**
    - chat to benj about setting up a meeting with small 3 man team to get them on platform
    - simplify design system
    - start workign on params thoughts?]
";

    fn names(nodes: &[Node]) -> Vec<&str> {
        nodes
            .iter()
            .filter_map(|n| match n {
                Node::Group { name, .. } => Some(name.as_str()),
                Node::Item(_) => None,
            })
            .collect()
    }

    fn children<'a>(nodes: &'a [Node], name: &str) -> &'a [Node] {
        nodes
            .iter()
            .find_map(|n| match n {
                Node::Group { name: n, children } if n == name => Some(children.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no group {name}"))
    }

    /// Five groups, the `Todo` heading passed over and the bold taken off
    /// their names, 29 items, nested where they were indented.
    #[test]
    fn the_fixture_reads_as_five_groups_and_29_items() {
        let nodes = parse(FIXTURE);
        assert_eq!(
            names(&nodes),
            ["Emails", "link sharing", "UI", "MCP fixes", "Side quests"]
        );
        assert_eq!(nodes.len(), 5);
        assert_eq!(item_count(&nodes), 29);
        let counts: Vec<usize> = names(&nodes)
            .iter()
            .map(|n| item_count(children(&nodes, n)))
            .collect();
        assert_eq!(counts, [5, 1, 8, 12, 3]);
        let ui = children(&nodes, "UI");
        assert_eq!(names(ui), ["Later stuff"]);
        assert_eq!(item_count(children(ui, "Later stuff")), 5);
        let mcp = children(&nodes, "MCP fixes");
        assert_eq!(
            names(mcp),
            [
                "Rethink templates in scope of MCP creating custo m teampltes (links to deck review)",
                "storyline prompt",
                "low desnity component liek claude",
                "Long tail improve components",
            ]
        );
        assert_eq!(
            mcp[0],
            Node::Item("MCP insert lsides from previoius decks and deck search etc".into())
        );
    }

    /// Tabs, or two spaces a level, read the same as four.
    #[test]
    fn tabs_and_two_space_indents_nest_too() {
        let want = vec![Node::Group {
            name: "A".into(),
            children: vec![
                Node::Group {
                    name: "B".into(),
                    children: vec![Node::Item("c".into())],
                },
                Node::Item("d".into()),
            ],
        }];
        assert_eq!(parse("- A\n\t- B\n\t\t* c\n\t+ d\n"), want);
        assert_eq!(parse("- A\n  - B\n    - c\n  - d\n"), want);
    }

    /// Items at the top with no group of their own go into one `Inbox`,
    /// where the first of them stood.
    #[test]
    fn loose_items_go_into_the_inbox() {
        let nodes = parse("- one\n- G\n    - x\n- two\n");
        assert_eq!(names(&nodes), [INBOX, "G"]);
        assert_eq!(
            children(&nodes, INBOX),
            [Node::Item("one".into()), Node::Item("two".into())]
        );
    }

    /// A list indented as a whole reads as one that is not.
    #[test]
    fn a_wholly_indented_list_reads_the_same() {
        let flush = parse("- A\n    - x\n    - y\n- B\n    - z\n");
        let shifted = parse("    - A\n        - x\n        - y\n    - B\n        - z\n");
        assert_eq!(flush, shifted);
        assert_eq!(names(&shifted), ["A", "B"]);
    }

    /// Task boxes come off with the bullet.
    #[test]
    fn task_boxes_are_taken_off() {
        let nodes = parse("- G\n  - [ ] open\n  - [x] done\n");
        assert_eq!(
            children(&nodes, "G"),
            [Node::Item("open".into()), Node::Item("done".into())]
        );
    }

    /// However deep a list nests, the walk down it stops at a depth.
    #[test]
    fn a_very_deep_list_is_capped() {
        let text: String = (0..200)
            .map(|i| format!("{}- x{i}\n", " ".repeat(i)))
            .collect();
        let mut nodes = parse(&text);
        let mut depth = 0;
        while let Some(Node::Group { children, .. }) = nodes.first().cloned() {
            nodes = children;
            depth += 1;
        }
        assert!(depth <= MAX_LEVEL + 1, "{depth}");
        assert_eq!(item_count(&parse(&text)), 200 - MAX_LEVEL);
    }

    /// An indent deeper than one level under its bullet is one level.
    #[test]
    fn an_overdeep_indent_is_one_level() {
        let nodes = parse("- G\n            - x\n");
        assert_eq!(item_count(children(&nodes, "G")), 1);
    }

    /// Lines pasted into a new item's field are an item each, with or
    /// without a bullet, and the `;` that listed them taken off.
    #[test]
    fn pasted_lines_are_an_item_each() {
        let text = "deck folders;\n  ◦ speaker notes;\n\n  ◦ smart model routing;\n  • [ ] the hint-unmet note.\n- **bold**\n";
        assert_eq!(
            lines(text),
            [
                "deck folders",
                "speaker notes",
                "smart model routing",
                "the hint-unmet note.",
                "bold",
            ]
        );
    }

    /// The dots a notes app copies out are bullets too.
    #[test]
    fn dots_are_bullets() {
        let nodes = parse("• G\n  ◦ x\n  ▪ y\n");
        assert_eq!(item_count(children(&nodes, "G")), 2);
    }

    #[test]
    fn plain_lines_are_no_bullets() {
        assert!(parse("Todo\njust a line\n-not a bullet\n").is_empty());
    }
}
