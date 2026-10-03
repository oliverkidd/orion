//! The DIFF VIEWER's reading pane as a person reads it, not as `git diff`
//! prints it.
//!
//! `git diff` writes for `patch`: a `diff --git` line, an `index` line,
//! `---`/`+++` and `@@ -12,7 +12,8 @@` before the first line of code, every
//! line prefixed with its sign and none of them numbered. The pane used to
//! show exactly that, one terminal row per line, cut off at the right edge
//! — a pager, not a review. Here the same text is read once, when it lands
//! ([`DiffDoc::build`]), into lines that say what they are:
//!
//! - the file's facts — added, deleted, renamed from, binary, how many
//!   lines each way — go to the pane's title ([`FileFacts`]) and the
//!   header lines that carried them are dropped;
//! - a hunk's `@@` becomes the line it starts on and the function it is in;
//! - each line of code keeps its old and new line numbers, its sign in a
//!   gutter of its own, and the syntax colours of the file's language
//!   (`syntax`), so an added line reads as code and not as a green smear.
//!
//! Drawing ([`DiffDoc::render`]) soft-wraps every line at the pane's width:
//! a long line of code continues on the rows under it, indented to where
//! its code starts, the numbers and the sign on its first row only. Nothing
//! is ever wider than the pane and nothing scrolls sideways. The pane
//! scrolls by rows; [`DiffDoc::rows_of`] counts them without building a
//! single span, from widths measured once at build time, so a 20 000-line
//! diff scrolls as cheaply as a short one.
//!
//! Above the diff sits the REVIEW HEAD ([`Head`]): what is on screen — a
//! commit's message, the commits ticked to be read together, a pull
//! request's title — wrapped as prose.

use crate::syntax::{Highlighter, TokenKind};
use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// What starts a combined diff's next range, at the start of a line of its
/// own, followed by that range's label (`git_diff::scoped_diff`). A record
/// separator: no line `git diff` prints ever starts with one.
pub const RANGE_MARK: char = '\u{1e}';

/// The row tint under an added line — a near-black green, truecolor like
/// `Theme::focus_tint`, the same in every preset: which way a line went is
/// not a matter of taste.
pub const ADD_BG: Color = Color::Rgb(16, 46, 28);
/// The tint under a removed line: the red twin of [`ADD_BG`].
pub const DEL_BG: Color = Color::Rgb(58, 20, 26);

/// The columns a line of code keeps for itself before the gutter's line
/// numbers give theirs up to it: on a pane narrower than this plus the
/// numbers, only the sign stays.
const MIN_CODE_W: usize = 12;

/// One line of the REVIEW HEAD over a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// Dim: who and when, a short sha.
    Meta(String),
    /// Bold: a subject, `commit 2 of 3 · …`, a pull request's title.
    Title(String),
    /// A message body's line, word-wrapped.
    Prose(String),
    /// One of several commits shown together: its short sha, its subject.
    Item {
        sha: String,
        text: String,
    },
    Blank,
}

/// Which way a line of code went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Add,
    Del,
    Context,
}

/// One logical line of the pane; drawing wraps it into one or more rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocLine {
    Head(Head),
    /// The rule under the head.
    Rule,
    /// A combined diff's next range starts here: its label.
    Range(String),
    /// A hunk: the line it starts on in the new file (the old one's, for a
    /// deletion) and the enclosing function git names after its `@@`.
    Hunk {
        line: u32,
        context: String,
    },
    /// One line of code: its numbers on each side and its syntax runs.
    Code {
        sign: Sign,
        old: Option<u32>,
        new: Option<u32>,
        runs: Vec<(TokenKind, String)>,
    },
    /// Something said about the file rather than shown of it — a missing
    /// newline at its end, a mode change, a binary file — at the code's
    /// column.
    Aside(String),
    /// Plain text across the whole pane: a placeholder (`loading…`), git's
    /// own error, a tree directory's summary.
    Text(String),
    /// Air between two hunks.
    Gap,
}

/// What a file's diff says about it: for the pane's title.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFacts {
    pub added: usize,
    pub removed: usize,
    /// `added`, `deleted`, `renamed`, `copied`, `mode changed`, `binary`;
    /// None for a plain edit — or for text that is not a diff at all.
    pub status: Option<&'static str>,
    /// A rename's or a copy's source path.
    pub from: Option<String>,
}

/// A diff read into [`DocLine`]s, under its REVIEW HEAD.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffDoc {
    pub lines: Vec<DocLine>,
    /// How many of `lines` the head takes, its rule included.
    pub head_lines: usize,
    /// Digits in the largest line number, for the gutter.
    pub num_w: usize,
    pub facts: FileFacts,
    rows: RowCache,
}

/// Each line's rows at the width last asked about. Counting them walks
/// every char of every line, and the pane asks on every frame it draws
/// and every key that scrolls it; the answer only changes with the width.
#[derive(Debug, Default)]
struct RowCache(std::sync::Mutex<Option<(u16, std::sync::Arc<Vec<u32>>)>>);

impl Clone for RowCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for RowCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for RowCache {}

/// How a pane `width` cells wide lays a [`DiffDoc`] out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Columns {
    width: usize,
    /// Whether the two line-number columns fit.
    numbers: bool,
    /// The cells before a line's code: the numbers, the sign, a space.
    gutter: usize,
}

impl Columns {
    fn code(&self) -> usize {
        self.width.saturating_sub(self.gutter).max(1)
    }
}

impl DiffDoc {
    /// Read `text` — one file's `git diff`, or the plain text the pane
    /// shows instead of one — under `head`. `path` picks the highlighter.
    pub fn build(head: &[Head], path: Option<&str>, text: &str) -> Self {
        let mut doc = DiffDoc::default();
        doc.lines.extend(head.iter().cloned().map(DocLine::Head));
        if !head.is_empty() {
            doc.lines.push(DocLine::Rule);
        }
        doc.head_lines = doc.lines.len();
        Reader::new(path).read(text, &mut doc);
        doc
    }

    /// The same diff under another head — a tick changed what the head says
    /// while the file on screen stayed the same.
    pub fn with_head(&self, head: &[Head]) -> Self {
        let mut lines: Vec<DocLine> = head.iter().cloned().map(DocLine::Head).collect();
        if !head.is_empty() {
            lines.push(DocLine::Rule);
        }
        let head_lines = lines.len();
        lines.extend(self.lines[self.head_lines..].iter().cloned());
        DiffDoc {
            lines,
            head_lines,
            num_w: self.num_w,
            facts: self.facts.clone(),
            rows: RowCache::default(),
        }
    }

    fn columns(&self, width: u16) -> Columns {
        let width = usize::from(width).max(1);
        let num_w = self.num_w.max(1);
        let numbers = width >= 2 * num_w + 4 + MIN_CODE_W;
        let gutter = if numbers {
            2 * num_w + 4
        } else if width > 2 + MIN_CODE_W / 2 {
            2
        } else {
            0
        };
        Columns {
            width,
            numbers,
            gutter,
        }
    }

    /// The rows line `i` takes in a pane `width` cells wide.
    pub fn rows_of(&self, i: usize, width: u16) -> usize {
        self.row_counts(width)[i] as usize
    }

    /// Every line's rows at `width`, from the cache when the width is the
    /// one it last counted at.
    fn row_counts(&self, width: u16) -> std::sync::Arc<Vec<u32>> {
        let count = || {
            let cols = self.columns(width);
            std::sync::Arc::new(
                self.lines
                    .iter()
                    .map(|l| self.line_rows(l, cols) as u32)
                    .collect::<Vec<u32>>(),
            )
        };
        let Ok(mut cache) = self.rows.0.lock() else {
            return count();
        };
        if let Some((at, rows)) = cache.as_ref() {
            if *at == width {
                return rows.clone();
            }
        }
        let rows = count();
        *cache = Some((width, rows.clone()));
        rows
    }

    fn line_rows(&self, line: &DocLine, cols: Columns) -> usize {
        match line {
            DocLine::Code { runs, .. } => code_breaks(runs_chars(runs), cols.code()).len(),
            DocLine::Head(Head::Item { sha, text }) => {
                let indent = item_indent(sha, cols.width);
                wrap_words(text, cols.width - indent).len()
            }
            DocLine::Head(Head::Meta(text) | Head::Title(text) | Head::Prose(text))
            | DocLine::Text(text) => wrap_words(text, cols.width).len(),
            DocLine::Range(label) => wrap_words(&range_text(label), cols.width).len(),
            DocLine::Hunk { line, context } => {
                wrap_words(&hunk_text(*line, context), cols.code()).len()
            }
            DocLine::Aside(text) => wrap_words(text, cols.code()).len(),
            DocLine::Head(Head::Blank) | DocLine::Rule | DocLine::Gap => 1,
        }
    }

    /// Every row the doc takes at `width`.
    pub fn total_rows(&self, width: u16) -> usize {
        self.row_counts(width).iter().map(|&r| r as usize).sum()
    }

    /// The rows the REVIEW HEAD takes at `width`, its rule included.
    pub fn head_rows(&self, width: u16) -> usize {
        self.row_counts(width)[..self.head_lines]
            .iter()
            .map(|&r| r as usize)
            .sum()
    }

    /// The furthest a pane `height` rows tall scrolls: the last row on its
    /// bottom row.
    pub fn max_scroll(&self, width: u16, height: u16) -> usize {
        self.total_rows(width)
            .saturating_sub(usize::from(height.max(1)))
    }

    /// The line row `row` falls in, and how far into it: where a pane
    /// scrolled to `row` starts drawing.
    pub fn locate(&self, row: usize, width: u16) -> (usize, usize) {
        let mut seen = 0;
        for (i, &rows) in self.row_counts(width).iter().enumerate() {
            let rows = rows as usize;
            if seen + rows > row {
                return (i, row - seen);
            }
            seen += rows;
        }
        (self.lines.len(), 0)
    }

    /// The pane's rows from row `scroll`, `height` of them at most, each no
    /// wider than `width`.
    pub fn render(&self, scroll: usize, width: u16, height: u16, th: Theme) -> Vec<Line<'static>> {
        let cols = self.columns(width);
        let height = usize::from(height);
        let (first, mut skip) = self.locate(scroll, width);
        let mut out = Vec::with_capacity(height);
        for line in self.lines.iter().skip(first) {
            for row in self.line_lines(line, cols, th) {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                if out.len() == height {
                    return out;
                }
                out.push(row);
            }
        }
        out
    }

    /// One logical line, wrapped into its rows.
    fn line_lines(&self, line: &DocLine, cols: Columns, th: Theme) -> Vec<Line<'static>> {
        let plain = |text: &str, width: usize, style: Style, indent: usize| -> Vec<Line<'static>> {
            wrap_words(text, width)
                .into_iter()
                .map(|row| {
                    Line::from(vec![
                        Span::raw(" ".repeat(indent)),
                        Span::styled(row, style),
                    ])
                })
                .collect()
        };
        match line {
            DocLine::Head(Head::Meta(text)) => {
                plain(text, cols.width, Style::default().fg(th.dim), 0)
            }
            DocLine::Head(Head::Title(text)) => plain(
                text,
                cols.width,
                Style::default().fg(th.text).add_modifier(Modifier::BOLD),
                0,
            ),
            DocLine::Head(Head::Prose(text)) | DocLine::Text(text) => {
                plain(text, cols.width, Style::default(), 0)
            }
            DocLine::Head(Head::Item { sha, text }) => {
                let indent = item_indent(sha, cols.width);
                wrap_words(text, cols.width - indent)
                    .into_iter()
                    .enumerate()
                    .map(|(n, row)| {
                        let lead = if indent == 0 {
                            Span::raw("")
                        } else if n == 0 {
                            Span::styled(format!("{sha} "), Style::default().fg(th.accent))
                        } else {
                            Span::raw(" ".repeat(indent))
                        };
                        Line::from(vec![lead, Span::raw(row)])
                    })
                    .collect()
            }
            DocLine::Head(Head::Blank) | DocLine::Gap => vec![Line::default()],
            DocLine::Rule => vec![Line::from(Span::styled(
                "─".repeat(cols.width),
                Style::default().fg(th.edge),
            ))],
            DocLine::Range(label) => wrap_words(&range_text(label), cols.width)
                .into_iter()
                .map(|row| {
                    let fill = cols.width.saturating_sub(row.width());
                    Line::from(vec![
                        Span::styled(
                            row,
                            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled("─".repeat(fill), Style::default().fg(th.edge)),
                    ])
                })
                .collect(),
            DocLine::Hunk { line, context } => {
                let text = hunk_text(*line, context);
                let split = format!("line {line}").width();
                wrap_words(&text, cols.code())
                    .into_iter()
                    .enumerate()
                    .map(|(n, row)| {
                        let mut spans = vec![self.blank_gutter(cols, n == 0, th)];
                        if n == 0 && row.width() > split {
                            let (head, tail) = split_at_width(&row, split);
                            spans.push(Span::styled(head, Style::default().fg(th.accent)));
                            spans.push(Span::styled(tail, Style::default().fg(th.muted)));
                        } else {
                            let style = if n == 0 { th.accent } else { th.muted };
                            spans.push(Span::styled(row, Style::default().fg(style)));
                        }
                        Line::from(spans)
                    })
                    .collect()
            }
            DocLine::Aside(text) => wrap_words(text, cols.code())
                .into_iter()
                .map(|row| {
                    Line::from(vec![
                        Span::raw(" ".repeat(cols.gutter)),
                        Span::styled(
                            row,
                            Style::default().fg(th.dim).add_modifier(Modifier::ITALIC),
                        ),
                    ])
                })
                .collect(),
            DocLine::Code {
                sign,
                old,
                new,
                runs,
                ..
            } => self.code_lines(*sign, *old, *new, runs, cols, th),
        }
    }

    /// The gutter of a row that is not a line of code: `⋯` where a hunk
    /// starts, so the eye finds the break running down the numbers.
    fn blank_gutter(&self, cols: Columns, mark: bool, th: Theme) -> Span<'static> {
        if !mark || cols.gutter < 2 {
            return Span::raw(" ".repeat(cols.gutter));
        }
        let dots = format!("{:>w$}", "⋯", w = cols.gutter - 1);
        Span::styled(format!("{dots} "), Style::default().fg(th.dim))
    }

    fn code_lines(
        &self,
        sign: Sign,
        old: Option<u32>,
        new: Option<u32>,
        runs: &[(TokenKind, String)],
        cols: Columns,
        th: Theme,
    ) -> Vec<Line<'static>> {
        let bg = match sign {
            Sign::Add => Some(ADD_BG),
            Sign::Del => Some(DEL_BG),
            Sign::Context => None,
        };
        let tint = |style: Style| match bg {
            Some(bg) => style.bg(bg),
            None => style,
        };
        let styled: Vec<(String, Style)> = runs
            .iter()
            .map(|(kind, text)| (text.clone(), tint(crate::ui::token_style(*kind, th))))
            .collect();
        let code = cols.code();
        let num_w = self.num_w.max(1);
        let (mark, mark_style) = match sign {
            Sign::Add => ("+", Style::default().fg(th.ok).add_modifier(Modifier::BOLD)),
            Sign::Del => (
                "−",
                Style::default().fg(th.err).add_modifier(Modifier::BOLD),
            ),
            Sign::Context => (" ", Style::default()),
        };
        let number = |n: Option<u32>| match n {
            Some(n) => format!("{n:>num_w$}"),
            None => " ".repeat(num_w),
        };
        code_rows(&styled, code)
            .into_iter()
            .enumerate()
            .map(|(row, chunk)| {
                let mut spans = Vec::with_capacity(chunk.len() + 4);
                if cols.gutter > 0 {
                    let numbers = if cols.numbers && row == 0 {
                        format!("{} {} ", number(old), number(new))
                    } else if cols.numbers {
                        " ".repeat(2 * num_w + 2)
                    } else {
                        String::new()
                    };
                    if !numbers.is_empty() {
                        spans.push(Span::styled(numbers, tint(Style::default().fg(th.dim))));
                    }
                    let mark = if row == 0 { mark } else { " " };
                    spans.push(Span::styled(format!("{mark} "), tint(mark_style)));
                }
                let used: usize = chunk.iter().map(|s| s.content.width()).sum();
                spans.extend(chunk);
                if bg.is_some() && used < code {
                    spans.push(Span::styled(
                        " ".repeat(code - used),
                        tint(Style::default()),
                    ));
                }
                Line::from(spans)
            })
            .collect()
    }
}

/// Every char of a line of code's runs, in order.
fn runs_chars(runs: &[(TokenKind, String)]) -> impl Iterator<Item = char> + '_ {
    runs.iter().flat_map(|(_, text)| text.chars())
}

/// Where each row of a line of code starts, as char offsets, at `width`
/// cells — the first always at 0. A row breaks after its last space when
/// that leaves it at least half full, so a wrapped line goes on with a
/// whole word; with no such space, at the edge, mid-word.
fn code_breaks(text: impl Iterator<Item = char>, width: usize) -> Vec<usize> {
    let width = width.max(1);
    let cells: Vec<(char, usize)> = text.map(|c| (c, c.width().unwrap_or(0))).collect();
    let mut starts = vec![0];
    let mut used = 0;
    // Just past the row's last space, and the cells up to there.
    let mut space: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < cells.len() {
        let (ch, w) = cells[i];
        if used > 0 && used + w > width {
            let at = match space {
                Some((at, before)) if before * 2 >= width => {
                    used -= before;
                    at
                }
                _ => {
                    used = 0;
                    i
                }
            };
            starts.push(at);
            space = None;
            continue;
        }
        used += w;
        i += 1;
        if ch == ' ' {
            space = Some((i, used));
        }
    }
    starts
}

/// `runs` cut into rows at [`code_breaks`]: each row's spans, its runs'
/// styles kept across the cuts.
fn code_rows(runs: &[(String, Style)], width: usize) -> Vec<Vec<Span<'static>>> {
    let breaks = code_breaks(runs.iter().flat_map(|(text, _)| text.chars()), width);
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new(); breaks.len()];
    let mut row = 0;
    let mut at = 0;
    for (text, style) in runs {
        let mut piece = String::new();
        for ch in text.chars() {
            if row + 1 < breaks.len() && at == breaks[row + 1] {
                if !piece.is_empty() {
                    rows[row].push(Span::styled(std::mem::take(&mut piece), *style));
                }
                row += 1;
            }
            piece.push(ch);
            at += 1;
        }
        if !piece.is_empty() {
            rows[row].push(Span::styled(piece, *style));
        }
    }
    rows
}

/// `text` split after its first `cells` cells.
fn split_at_width(text: &str, cells: usize) -> (String, String) {
    let mut used = 0;
    for (i, ch) in text.char_indices() {
        if used >= cells {
            return (text[..i].to_string(), text[i..].to_string());
        }
        used += ch.width().unwrap_or(0);
    }
    (text.to_string(), String::new())
}

/// The cells a commit's sha and its space take before its subject in a
/// head's list — none on a pane too narrow to spare them and still show
/// the subject.
fn item_indent(sha: &str, width: usize) -> usize {
    let indent = sha.width() + 1;
    if width >= indent + MIN_CODE_W {
        indent
    } else {
        0
    }
}

fn hunk_text(line: u32, context: &str) -> String {
    if context.is_empty() {
        format!("line {line}")
    } else {
        format!("line {line}  {context}")
    }
}

fn range_text(label: &str) -> String {
    format!("── {label} ")
}

/// `text` word-wrapped into rows no wider than `width` cells: breaks at
/// spaces, and a word wider than a row is broken across rows at the edge.
/// Always at least one row, so an empty line stays a line.
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    for (i, word) in text.split(' ').enumerate() {
        let w = word.width();
        let gap = usize::from(i > 0);
        if used + gap + w <= width {
            if i > 0 {
                row.push(' ');
            }
            row.push_str(word);
            used += gap + w;
            continue;
        }
        if w <= width {
            rows.push(std::mem::take(&mut row));
            row.push_str(word);
            used = w;
            continue;
        }
        // A word no row can hold: what fits of it here, the rest below.
        if used > 0 && used + gap < width {
            row.push(' ');
            used += 1;
        } else if used > 0 {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        for ch in word.chars() {
            let cw = ch.width().unwrap_or(0);
            if used > 0 && used + cw > width {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(ch);
            used += cw;
        }
    }
    rows.push(row);
    rows
}

/// The `git diff` text reader behind [`DiffDoc::build`].
struct Reader {
    path: Option<String>,
    /// Highlighters for each side: a block comment opened on a removed line
    /// must not paint the added ones, nor the other way round.
    new_hl: Highlighter,
    old_hl: Highlighter,
    old: u32,
    new: u32,
    /// Lines left in the hunk on each side, from its `@@` counts: what
    /// tells a removed `-- comment` from the next file's `--- a/x`.
    old_left: u32,
    new_left: u32,
    in_file: bool,
    hunks: usize,
    old_mode: Option<String>,
    max_num: u32,
}

impl Reader {
    fn new(path: Option<&str>) -> Self {
        let mut reader = Self {
            path: path.map(str::to_string),
            new_hl: Highlighter::plain(),
            old_hl: Highlighter::plain(),
            old: 0,
            new: 0,
            old_left: 0,
            new_left: 0,
            in_file: false,
            hunks: 0,
            old_mode: None,
            max_num: 0,
        };
        reader.reset_highlight();
        reader
    }

    /// Fresh highlighters: between two hunks lie lines nobody read, so
    /// whatever state the last one ended in is a guess.
    fn reset_highlight(&mut self) {
        let fresh = |path: &Option<String>| match path {
            Some(path) => Highlighter::for_path(path),
            None => Highlighter::plain(),
        };
        self.new_hl = fresh(&self.path);
        self.old_hl = fresh(&self.path);
    }

    fn read(&mut self, text: &str, doc: &mut DiffDoc) {
        for raw in text.lines() {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if (self.old_left > 0 || self.new_left > 0) && self.code(line, doc) {
                continue;
            }
            self.outside(line, doc);
        }
        // Three digits at least: a gutter that changes width from one
        // file to the next makes the code jump about.
        doc.num_w = self.max_num.to_string().len().max(3);
    }

    /// A line inside a hunk; false when it is not one after all (the hunk's
    /// counts were off), so the caller reads it as something else.
    fn code(&mut self, line: &str, doc: &mut DiffDoc) -> bool {
        let (sign, body) = match line.chars().next() {
            Some('+') => (Sign::Add, &line[1..]),
            Some('-') => (Sign::Del, &line[1..]),
            Some(' ') => (Sign::Context, &line[1..]),
            // Some tools strip the space off an empty context line.
            None => (Sign::Context, ""),
            Some('\\') => {
                doc.lines
                    .push(DocLine::Aside("no newline at the end of the file".into()));
                return true;
            }
            Some(_) => {
                self.old_left = 0;
                self.new_left = 0;
                return false;
            }
        };
        let text = body.replace('\t', "    ");
        let (old, new, runs) = match sign {
            Sign::Add => {
                self.new_left = self.new_left.saturating_sub(1);
                self.new += 1;
                (None, Some(self.new - 1), self.new_hl.line(&text))
            }
            Sign::Del => {
                self.old_left = self.old_left.saturating_sub(1);
                self.old += 1;
                (Some(self.old - 1), None, self.old_hl.line(&text))
            }
            Sign::Context => {
                self.old_left = self.old_left.saturating_sub(1);
                self.new_left = self.new_left.saturating_sub(1);
                self.old += 1;
                self.new += 1;
                self.old_hl.line(&text);
                (
                    Some(self.old - 1),
                    Some(self.new - 1),
                    self.new_hl.line(&text),
                )
            }
        };
        self.max_num = self.max_num.max(old.unwrap_or(0)).max(new.unwrap_or(0));
        match sign {
            Sign::Add => doc.facts.added += 1,
            Sign::Del => doc.facts.removed += 1,
            Sign::Context => {}
        }
        doc.lines.push(DocLine::Code {
            sign,
            old,
            new,
            runs,
        });
        true
    }

    /// Anything outside a hunk: a file's header, a hunk's `@@`, a range's
    /// mark — or, in text that is not a diff, a line of plain text.
    fn outside(&mut self, line: &str, doc: &mut DiffDoc) {
        let facts = &mut doc.facts;
        if let Some(label) = line.strip_prefix(RANGE_MARK) {
            doc.lines.push(DocLine::Range(label.to_string()));
            self.in_file = false;
            return;
        }
        if line.starts_with("diff --git ") || line.starts_with("diff --cc ") {
            self.in_file = true;
            self.hunks = 0;
            return;
        }
        if let Some((old, new, context)) = parse_hunk(line) {
            if self.hunks > 0 {
                doc.lines.push(DocLine::Gap);
            }
            self.hunks += 1;
            self.in_file = true;
            self.reset_highlight();
            (self.old, self.old_left) = old;
            (self.new, self.new_left) = new;
            // A pure deletion starts nowhere in the new file: name the old
            // line it starts on instead.
            let at = if new.1 == 0 && old.1 > 0 {
                old.0
            } else {
                new.0
            };
            doc.lines.push(DocLine::Hunk {
                line: at.max(1),
                context: context.replace('\t', "    "),
            });
            return;
        }
        if line.starts_with('\\') {
            doc.lines
                .push(DocLine::Aside("no newline at the end of the file".into()));
            return;
        }
        if !self.in_file {
            doc.lines.push(DocLine::Text(line.replace('\t', "    ")));
            return;
        }
        if line.starts_with("new file mode") {
            facts.status = Some("added");
        } else if line.starts_with("deleted file mode") {
            facts.status = Some("deleted");
        } else if let Some(from) = line.strip_prefix("rename from ") {
            facts.status = Some("renamed");
            facts.from = Some(from.to_string());
        } else if let Some(from) = line.strip_prefix("copy from ") {
            facts.status = Some("copied");
            facts.from = Some(from.to_string());
        } else if let Some(mode) = line.strip_prefix("old mode ") {
            self.old_mode = Some(mode.to_string());
        } else if let Some(mode) = line.strip_prefix("new mode ") {
            let was = self.old_mode.take().unwrap_or_default();
            facts.status.get_or_insert("mode changed");
            doc.lines
                .push(DocLine::Aside(format!("file mode {was} → {mode}")));
        } else if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
            if facts.status.is_none() {
                facts.status = Some("binary");
            }
            doc.lines.push(DocLine::Aside(
                "a binary file: there is no text to show".into(),
            ));
        } else if [
            "index ",
            "--- ",
            "+++ ",
            "similarity index",
            "dissimilarity index",
            "rename to ",
            "copy to ",
            "literal ",
            "delta ",
        ]
        .iter()
        .any(|p| line.starts_with(p))
            || line.is_empty()
        {
            // Said in the title, or nothing a reader needs.
        } else {
            doc.lines.push(DocLine::Text(line.to_string()));
        }
    }
}

/// One side of a hunk: where it starts and how many lines it has.
type Side = (u32, u32);

/// `@@ -a[,b] +c[,d] @@ context`: each side's start and count, and the
/// context. A count left out is one.
fn parse_hunk(line: &str) -> Option<(Side, Side, String)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, context) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let side = |s: &str| -> Option<Side> {
        match s.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    Some((side(old)?, side(new)?, context.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = concat!(
        "diff --git a/src/main.rs b/src/main.rs\n",
        "index 7527576..f0276da 100644\n",
        "--- a/src/main.rs\n",
        "+++ b/src/main.rs\n",
        "@@ -1,3 +1,4 @@ fn main() {\n",
        " fn main() {\n",
        "-    println!(\"hello\");\n",
        "+    println!(\"hello, a much longer line that should wrap when the pane is narrow\");\n",
        "+    // a comment\n",
        " }\n",
        "\\ No newline at end of file",
    );

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_headers_become_facts_and_the_code_keeps_its_numbers() {
        let doc = DiffDoc::build(&[], Some("src/main.rs"), DIFF);
        assert_eq!(doc.facts.added, 2);
        assert_eq!(doc.facts.removed, 1);
        assert_eq!(doc.facts.status, None, "a plain edit");
        assert_eq!(
            doc.lines[0],
            DocLine::Hunk {
                line: 1,
                context: "fn main() {".into()
            },
            "the diff --git, index, ---/+++ lines are gone"
        );
        let numbers: Vec<(Sign, Option<u32>, Option<u32>)> = doc
            .lines
            .iter()
            .filter_map(|l| match l {
                DocLine::Code { sign, old, new, .. } => Some((*sign, *old, *new)),
                _ => None,
            })
            .collect();
        assert_eq!(
            numbers,
            [
                (Sign::Context, Some(1), Some(1)),
                (Sign::Del, Some(2), None),
                (Sign::Add, None, Some(2)),
                (Sign::Add, None, Some(3)),
                (Sign::Context, Some(3), Some(4)),
            ]
        );
        assert!(matches!(doc.lines.last(), Some(DocLine::Aside(_))));
        // The comment is coloured as one.
        let comment = doc.lines.iter().find_map(|l| match l {
            DocLine::Code { runs, .. } if runs.iter().any(|(_, t)| t.contains("a comment")) => {
                Some(runs.clone())
            }
            _ => None,
        });
        assert!(comment
            .unwrap()
            .iter()
            .any(|(kind, _)| *kind == TokenKind::Comment));
    }

    /// A removed line that starts `-- ` reads `--- …` in the diff: the
    /// hunk's counts say it is code, not the next file's header.
    #[test]
    fn a_removed_sql_comment_is_not_a_header() {
        let diff = "diff --git a/q.sql b/q.sql\n--- a/q.sql\n+++ b/q.sql\n@@ -1,2 +1,1 @@\n--- old note\n select 1;";
        let doc = DiffDoc::build(&[], Some("q.sql"), diff);
        assert_eq!(doc.facts.removed, 1);
        assert!(doc.lines.iter().any(|l| matches!(
            l,
            DocLine::Code { sign: Sign::Del, runs, .. }
                if runs.iter().map(|(_, t)| t.as_str()).collect::<String>() == "-- old note"
        )));
    }

    #[test]
    fn new_deleted_renamed_and_binary_files_say_so() {
        let added = "diff --git a/n.txt b/n.txt\nnew file mode 100644\nindex 0000000..1\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+hi";
        assert_eq!(DiffDoc::build(&[], None, added).facts.status, Some("added"));
        let gone = "diff --git a/n.txt b/n.txt\ndeleted file mode 100644\n--- a/n.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-hi";
        let doc = DiffDoc::build(&[], None, gone);
        assert_eq!(doc.facts.status, Some("deleted"));
        assert_eq!(
            doc.lines[0],
            DocLine::Hunk {
                line: 1,
                context: String::new()
            }
        );
        let moved =
            "diff --git a/a.rs b/b.rs\nsimilarity index 90%\nrename from a.rs\nrename to b.rs";
        let doc = DiffDoc::build(&[], None, moved);
        assert_eq!(doc.facts.status, Some("renamed"));
        assert_eq!(doc.facts.from.as_deref(), Some("a.rs"));
        assert!(doc.lines.is_empty(), "nothing else to show");
        let binary = "diff --git a/x.png b/x.png\nindex 1..2 100644\nBinary files a/x.png and b/x.png differ";
        let doc = DiffDoc::build(&[], None, binary);
        assert_eq!(doc.facts.status, Some("binary"));
        assert!(matches!(&doc.lines[0], DocLine::Aside(t) if t.contains("binary")));
    }

    #[test]
    fn plain_text_stays_plain_text() {
        let doc = DiffDoc::build(&[], None, "loading…");
        assert_eq!(doc.lines, [DocLine::Text("loading…".into())]);
        let doc = DiffDoc::build(&[Head::Title("subject".into())], None, "");
        assert_eq!(doc.head_lines, 2, "the head and its rule");
    }

    /// The core promise: at any width, no row of the pane is wider than
    /// the pane — code, prose, hunk headers and range labels alike — and a
    /// long line's continuation rows carry no number and no sign.
    #[test]
    fn no_row_is_ever_wider_than_the_pane() {
        let head = [
            Head::Meta("1ec007c · Dana · 2h ago".into()),
            Head::Title("Add a retry helper with exponential backoff for the webhook dispatcher".into()),
            Head::Blank,
            Head::Prose("The webhook dispatcher drops events when the endpoint restarts; retrying with a doubling wait rides it out.".into()),
            Head::Item {
                sha: "dde6cc9".into(),
                text: "Try six times, because five was not always enough for a cold start".into(),
            },
        ];
        let text = format!("{RANGE_MARK}1ec007c..dde6cc9 (3 commits)\n{DIFF}\n{RANGE_MARK}a very long range label that goes on and on past any pane");
        let doc = DiffDoc::build(&head, Some("src/main.rs"), &text);
        let th = Theme::default();
        for width in [1u16, 5, 12, 20, 33, 47, 80, 120] {
            let total = doc.total_rows(width);
            let rows = doc.render(0, width, u16::MAX, th);
            assert_eq!(rows.len(), total, "rows_of agrees with render at {width}");
            for row in &rows {
                assert!(
                    row.width() <= usize::from(width),
                    "{} > {width}: {:?}",
                    row.width(),
                    text_of(row)
                );
            }
        }
        // At 40 columns the long println wraps; its second row is all code.
        let rows: Vec<String> = doc
            .render(0, 40, u16::MAX, th)
            .iter()
            .map(text_of)
            .collect();
        let first = rows
            .iter()
            .position(|r| r.contains("println!(\"hello, a"))
            .expect("the long line");
        assert!(rows[first].starts_with("      2 + "), "{:?}", rows[first]);
        assert_eq!(
            &rows[first + 1][..10],
            "          ",
            "a continuation row has no number and no sign: {:?}",
            rows[first + 1]
        );
        assert_ne!(
            rows[first + 1].chars().nth(10),
            Some(' '),
            "it goes on with a word: {:?}",
            rows[first + 1]
        );
    }

    #[test]
    fn scrolling_lands_mid_line_and_clamps_at_the_end() {
        let doc = DiffDoc::build(&[], Some("src/main.rs"), DIFF);
        let width = 30;
        let total = doc.total_rows(width);
        assert_eq!(doc.max_scroll(width, 4), total - 4);
        assert_eq!(doc.max_scroll(width, 200), 0, "it all fits");
        let all = doc.render(0, width, u16::MAX, Theme::default());
        for start in 0..total {
            let window = doc.render(start, width, 3, Theme::default());
            let expect: Vec<String> = all[start..(start + 3).min(total)]
                .iter()
                .map(text_of)
                .collect();
            let got: Vec<String> = window.iter().map(text_of).collect();
            assert_eq!(got, expect, "from row {start}");
        }
    }

    #[test]
    fn a_new_head_keeps_the_diff_under_it() {
        let doc = DiffDoc::build(&[Head::Title("one".into())], Some("src/main.rs"), DIFF);
        let swapped = doc.with_head(&[Head::Meta("a".into()), Head::Title("two".into())]);
        assert_eq!(swapped.head_lines, 3);
        assert_eq!(&swapped.lines[3..], &doc.lines[2..]);
        assert_eq!(swapped.facts, doc.facts);
        assert_eq!(doc.with_head(&[]).head_lines, 0);
    }

    /// Code wraps after a space when that leaves the row at least half
    /// full, and at the edge when it would not.
    #[test]
    fn code_wraps_at_a_word_where_it_can() {
        assert_eq!(code_breaks("aaa bbb ccc".chars(), 8), [0, 8]);
        assert_eq!(
            code_breaks("a bcdefghijkl".chars(), 8),
            [0, 8],
            "a space too early"
        );
        assert_eq!(code_breaks("".chars(), 8), [0]);
        assert_eq!(code_breaks("abcdefgh".chars(), 8), [0], "an exact fit");
        let style = Style::default();
        let rows = code_rows(&[("aaa ".into(), style), ("bbb ccc".into(), style)], 8);
        let text: Vec<String> = rows
            .iter()
            .map(|r| r.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(text, ["aaa bbb ", "ccc"]);
    }

    #[test]
    fn wrap_words_breaks_at_spaces_and_long_words_at_the_edge() {
        assert_eq!(wrap_words("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap_words("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap_words("", 4), [""]);
        assert_eq!(wrap_words("ab abcdefgh", 5), ["ab ab", "cdefg", "h"]);
        for row in wrap_words("日本語のテキストを折り返す", 5) {
            assert!(row.width() <= 5, "{row}");
        }
    }
}
