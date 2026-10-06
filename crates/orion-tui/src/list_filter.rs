//! Facet filters for the list modals — the PULL REQUESTS MODAL and the
//! LINEAR VIEW — on top of their fuzzy filter.
//!
//! The filter line stays the one source of truth: alongside the words the
//! fuzzy matcher reads, it carries `key:value` tokens (`label:bug`,
//! `p:high`, `status:"in progress"`, `-label:wip`) that narrow the rows by
//! a facet of theirs. [`parse`] splits the line into the two; [`matches`]
//! asks whether a row passes the tokens. Keys and values are
//! case-insensitive, and a value matches as a prefix of the row's value or
//! of any word in it (`p:hi` is High, `label:pdf` is `Export PDF`), with
//! `-`, `_` and spaces counting as the same thing (`status:in-progress`);
//! a quoted value matches the whole value only (`label:"bug"` is not
//! `bugfix`), which is how the FILTER PICK writes them.
//! Two tokens of one key are either/or; tokens of different keys must all
//! hold; a leading `-` excludes. A key the modal does not know is plain
//! text, and a key with no value yet (`label:` mid-typing) narrows nothing.
//!
//! The FILTER PICK (`⌘F`, [`FilterPick`]) writes the same tokens for you:
//! one column of each facet's values, ↑/↓ walking them all,
//! with how many rows carry each, `space` adding or removing the value's
//! token in the line ([`toggle`]).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::truncate;

/// One key a modal filters on: its canonical spelling (what [`toggle`]
/// writes and [`Token::key`] carries), the other spellings it answers to,
/// and the FILTER PICK's name for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FacetKey {
    pub key: &'static str,
    pub aliases: &'static [&'static str],
    pub title: &'static str,
}

impl FacetKey {
    pub const fn new(key: &'static str, title: &'static str) -> Self {
        Self {
            key,
            aliases: &[],
            title,
        }
    }

    pub const fn aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    fn answers(&self, key: &str) -> bool {
        self.key == key || self.aliases.contains(&key)
    }
}

/// One `key:value` token of the filter line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The canonical key ([`FacetKey::key`]).
    pub key: &'static str,
    /// The value, unquoted, as typed.
    pub value: String,
    /// The value as the matcher compares it ([`normalize`]).
    pub norm: String,
    /// Quoted: the row's value must be this one, not just start with it.
    pub exact: bool,
    /// `-key:value`: rows carrying it are left out.
    pub negated: bool,
    /// Where the token sits in the line, as a char range — what the
    /// search line lights.
    pub span: (usize, usize),
}

/// The filter line, split: the words for the fuzzy matcher and the
/// tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub text: String,
    pub tokens: Vec<Token>,
    /// Every recognised `key:` word's char range, a valueless one too —
    /// what the search line lights.
    pub spans: Vec<(usize, usize)>,
}

impl Parsed {
    /// Whether anything narrows the rows: text, or a token.
    pub fn is_active(&self) -> bool {
        !self.tokens.is_empty() || self.text.split_whitespace().next().is_some()
    }
}

/// The line's words, as char ranges: runs of non-space, a `"` holding
/// spaces inside a run until the next `"`.
fn words(query: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut quoted = false;
    let mut word = String::new();
    for (i, c) in query.chars().enumerate() {
        if c.is_whitespace() && !quoted {
            if let Some(s) = start.take() {
                out.push((s, i, std::mem::take(&mut word)));
            }
            continue;
        }
        if start.is_none() {
            start = Some(i);
        }
        if c == '"' {
            quoted = !quoted;
        }
        word.push(c);
    }
    if let Some(s) = start {
        out.push((s, query.chars().count(), word));
    }
    out
}

/// Split `query` into fuzzy text and the tokens of `keys`.
pub fn parse(query: &str, keys: &[FacetKey]) -> Parsed {
    let mut parsed = Parsed::default();
    let mut text: Vec<String> = Vec::new();
    for (start, end, word) in words(query) {
        let (negated, body) = match word.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, word.as_str()),
        };
        let facet = body.split_once(':').and_then(|(key, value)| {
            let key = key.to_lowercase();
            keys.iter()
                .find(|k| k.answers(&key))
                .map(|k| (k.key, value))
        });
        match facet {
            Some((key, value)) => {
                parsed.spans.push((start, end));
                let exact = value.len() > 1 && value.starts_with('"') && value.ends_with('"');
                let value = value.trim_matches('"');
                if !value.is_empty() {
                    parsed.tokens.push(Token {
                        key,
                        value: value.to_string(),
                        norm: normalize(value),
                        exact,
                        negated,
                        span: (start, end),
                    });
                }
            }
            None => text.push(word),
        }
    }
    parsed.text = text.join(" ");
    parsed
}

/// A value as the matcher compares it: lowercase, `-` and `_` as spaces,
/// runs of space as one.
pub fn normalize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars().flat_map(char::to_lowercase) {
        let c = if c == '-' || c == '_' { ' ' } else { c };
        if c.is_whitespace() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The rows the line leaves, best first, kept where its tokens pass
/// `values(row, key)`. Its words rank over `label(row)` — built only when
/// there are words to rank, every row in order otherwise — and a word
/// that is not in the label may name one of the row's facet values
/// instead (`high bug export` is High, labelled bug, about export): the
/// rows the whole text matches first, then those whose facets took some
/// words and whose label the rest. Each with the matched char positions
/// of its label.
pub fn narrow(
    parsed: &Parsed,
    keys: &[FacetKey],
    len: usize,
    label: impl Fn(usize) -> String,
    values: impl Fn(usize, &str) -> Vec<String>,
) -> Vec<(usize, Vec<usize>)> {
    let passes =
        |i: usize| parsed.tokens.is_empty() || matches(&parsed.tokens, |key| values(i, key));
    let words: Vec<String> = parsed.text.split_whitespace().map(normalize).collect();
    if words.is_empty() {
        return (0..len)
            .filter(|i| passes(*i))
            .map(|i| (i, Vec::new()))
            .collect();
    }
    let mut whole: Vec<(i32, usize, Vec<usize>)> = Vec::new();
    let mut by_facet: Vec<(i32, usize, Vec<usize>)> = Vec::new();
    for i in 0..len {
        if !passes(i) {
            continue;
        }
        let text = label(i);
        if let Some(m) = crate::fuzzy::fuzzy_match(&parsed.text, &text) {
            whole.push((m.score, i, m.positions));
            continue;
        }
        let facets: Vec<String> = keys.iter().flat_map(|k| values(i, k.key)).collect();
        let rest: Vec<&str> = words
            .iter()
            .filter(|w| !facets.iter().any(|v| word_prefix(&normalize(v), w)))
            .map(String::as_str)
            .collect();
        if rest.len() == words.len() {
            continue;
        }
        if rest.is_empty() {
            by_facet.push((0, i, Vec::new()));
        } else if let Some(m) = crate::fuzzy::fuzzy_match(&rest.join(" "), &text) {
            by_facet.push((m.score, i, m.positions));
        }
    }
    // Best score first; equal ones keep the list's own order.
    let best = |rows: &mut Vec<(i32, usize, Vec<usize>)>| {
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)))
    };
    best(&mut whole);
    best(&mut by_facet);
    whole
        .into_iter()
        .chain(by_facet)
        .map(|(_, i, positions)| (i, positions))
        .collect()
}

/// Whether `word` starts `value` or any word in it — both normalized.
fn word_prefix(value: &str, word: &str) -> bool {
    value.starts_with(word)
        || value
            .char_indices()
            .filter(|(_, c)| *c == ' ')
            .any(|(i, _)| value[i + 1..].starts_with(word))
}

/// The cursor's row among the `visible` ones: `selected` while it shows,
/// else the first that does — a filter, a tab or a refresh may have hidden
/// it. None with nothing showing.
pub fn cursor_in(visible: &[(usize, Vec<usize>)], selected: usize) -> Option<usize> {
    if visible.iter().any(|(i, _)| *i == selected) {
        Some(selected)
    } else {
        visible.first().map(|(i, _)| *i)
    }
}

/// Whether the token picks out `value`: the whole of it for an exact
/// token, else a prefix of it or of any word in it.
fn value_matches(token: &Token, value: &str) -> bool {
    if token.norm.is_empty() {
        return true;
    }
    let value = normalize(value);
    if token.exact {
        return value == token.norm;
    }
    word_prefix(&value, &token.norm)
}

/// Whether a row passes the tokens. `values(key)` is the row's values for
/// one key — none, one, or several (a row's labels).
pub fn matches(tokens: &[Token], values: impl Fn(&str) -> Vec<String>) -> bool {
    let mut keys: Vec<&str> = tokens.iter().map(|t| t.key).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter().all(|key| {
        let has = values(key);
        let hit = |t: &Token| has.iter().any(|v| value_matches(t, v));
        let mut wanted = false;
        let mut found = false;
        for token in tokens.iter().filter(|t| t.key == key) {
            if token.negated {
                if hit(token) {
                    return false;
                }
            } else {
                wanted = true;
                found = found || hit(token);
            }
        }
        found || !wanted
    })
}

/// The line's token for exactly `key:value`, not negated — what the
/// FILTER PICK ticks, and takes out again.
fn token_for<'a>(parsed: &'a Parsed, key: &str, value: &str) -> Option<&'a Token> {
    let value = normalize(value);
    parsed
        .tokens
        .iter()
        .find(|t| t.key == key && !t.negated && t.norm == value)
}

/// Whether the line already carries `key:value` (not negated) for exactly
/// this value — what the FILTER PICK ticks.
pub fn is_on(parsed: &Parsed, key: &str, value: &str) -> bool {
    token_for(parsed, key, value).is_some()
}

/// Add `key:"value"` to the line — quoted, so it picks out that value and
/// no other the picker counted apart — or take it out when it is already
/// there: the FILTER PICK's `space`. The caret goes to the end.
pub fn toggle(query: &mut TextInput, keys: &[FacetKey], key: &'static str, value: &str) {
    let parsed = parse(query, keys);
    let chars: Vec<char> = query.chars().collect();
    let found = token_for(&parsed, key, value);
    let next = match found {
        Some(token) => {
            let (start, end) = token.span;
            let before: String = chars[..start].iter().collect();
            let after: String = chars[end..].iter().collect();
            format!("{} {}", before.trim_end(), after.trim_start())
                .trim()
                .to_string()
        }
        None => {
            let line: String = chars.iter().collect();
            let line = line.trim_end();
            let token = format!("{key}:\"{value}\"");
            if line.is_empty() {
                token
            } else {
                format!("{line} {token}")
            }
        }
    };
    query.set_text(next);
}

// ---- the FILTER PICK ----

/// One value the FILTER PICK offers.
#[derive(Debug, Clone, PartialEq)]
pub struct PickValue {
    /// What the row says, and what [`toggle`] writes.
    pub value: String,
    /// The rows carrying it, among the ones the picker counts over.
    pub count: usize,
    /// A mark of its own before the name — a label's dot, a priority's
    /// bars — in the colour it wears on the rows.
    pub mark: Option<(String, Color)>,
}

/// One facet the FILTER PICK lists: its key and the values seen on the
/// rows.
#[derive(Debug, Clone, PartialEq)]
pub struct PickFacet {
    pub key: FacetKey,
    pub values: Vec<PickValue>,
}

/// Tally the values of one facet over `rows`: each value once, in the
/// order first seen, with how many rows carry it.
pub fn tally<'a, T: 'a>(
    rows: impl IntoIterator<Item = &'a T>,
    values: impl Fn(&T) -> Vec<String>,
) -> Vec<(String, usize)> {
    let mut seen: Vec<(String, usize)> = Vec::new();
    for row in rows {
        for value in values(row) {
            if value.is_empty() {
                continue;
            }
            match seen
                .iter_mut()
                .find(|(v, _)| v.eq_ignore_ascii_case(&value))
            {
                Some((_, n)) => *n += 1,
                None => seen.push((value, 1)),
            }
        }
    }
    seen
}

/// Sort a tally busiest first, then by name — for open-ended facets
/// (labels, authors, projects).
pub fn by_count(mut tally: Vec<(String, usize)>) -> Vec<(String, usize)> {
    tally.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    tally
}

/// A tally as the picker lists it, with no mark of its own.
pub fn plain_values(tally: Vec<(String, usize)>) -> Vec<PickValue> {
    tally
        .into_iter()
        .map(|(value, count)| PickValue {
            value,
            count,
            mark: None,
        })
        .collect()
}

/// A fixed set of words, in their own order, each with its count in
/// `tally` (0 when no row carries it) and the mark `mark` gives it.
pub fn fixed_values(
    words: &[&str],
    tally: &[(String, usize)],
    mark: impl Fn(&str) -> Option<(String, Color)>,
) -> Vec<PickValue> {
    words
        .iter()
        .map(|word| PickValue {
            value: (*word).to_string(),
            count: tally
                .iter()
                .find(|(v, _)| v.eq_ignore_ascii_case(word))
                .map_or(0, |(_, n)| *n),
            mark: mark(word),
        })
        .collect()
}

/// The FILTER PICK's cursor: which facet, and which of its values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FilterPick {
    pub facet: usize,
    pub value: usize,
}

/// Every value the picker lists, top to bottom, as `(facet, value)`: the
/// one column ↑/↓ walks, from a facet's last value on to the next's first.
fn flat(facets: &[PickFacet]) -> Vec<(usize, usize)> {
    facets
        .iter()
        .enumerate()
        .flat_map(|(f, facet)| (0..facet.values.len()).map(move |v| (f, v)))
        .collect()
}

impl FilterPick {
    /// ←/→: the next facet that has values, round either end, its first
    /// value under the cursor.
    pub fn step_facet(&mut self, facets: &[PickFacet], delta: i32) {
        let n = facets.len() as i32;
        let mut facet = self.facet as i32;
        for _ in 0..n {
            facet = (facet + delta.signum()).rem_euclid(n);
            if !facets[facet as usize].values.is_empty() {
                self.facet = facet as usize;
                self.value = 0;
                return;
            }
        }
    }

    /// ↑/↓: the next value down the one column, across facets, clamped
    /// at either end.
    pub fn step_value(&mut self, facets: &[PickFacet], delta: i32) {
        let all = flat(facets);
        if all.is_empty() {
            return;
        }
        let here = all
            .iter()
            .position(|p| *p == (self.facet, self.value))
            .unwrap_or(0);
        let next = (here as i64 + delta as i64).clamp(0, all.len() as i64 - 1) as usize;
        (self.facet, self.value) = all[next];
    }

    /// The cursor kept on the lists as they are now — the counts move as
    /// the filter does, and a value can drop out.
    pub fn clamp(&mut self, facets: &[PickFacet]) {
        self.facet = self.facet.min(facets.len().saturating_sub(1));
        let len = facets.get(self.facet).map_or(0, |f| f.values.len());
        if len == 0 {
            // Its values all gone: onto the nearest facet that has some.
            if let Some(&(f, v)) = flat(facets).iter().find(|(f, _)| *f >= self.facet) {
                (self.facet, self.value) = (f, v);
            } else if let Some(&(f, _)) = flat(facets).last() {
                (self.facet, self.value) = (f, 0);
            }
            return;
        }
        self.value = crate::app::clamp_selection(self.value as i64, len);
    }

    /// The facet key and value under the cursor.
    pub fn current<'a>(&self, facets: &'a [PickFacet]) -> Option<(&'a FacetKey, &'a PickValue)> {
        let facet = facets.get(self.facet)?;
        facet.values.get(self.value).map(|v| (&facet.key, v))
    }
}

/// The FILTER PICK's own keys.
pub(crate) mod keys {
    use crate::hints::Key;

    /// Open the picker — the same chord in every list modal.
    pub const FILTER: Key = Key::new(&["cmd+f", "ctrl+f"], "filter");
    pub const FACET: Key = Key::new(&["left", "right"], "facet").show(2);
    pub const VALUE: Key = Key::new(&["up", "down"], "pick").show(2);
    pub const TOGGLE: Key = Key::new(&["space"], "toggle");
    pub const DONE: Key = Key::new(&["enter", "esc"], "done").show(2);
    #[cfg(test)]
    pub const ALL: &[Key] = &[FILTER, FACET, VALUE, TOGGLE, DONE];
}

/// The keys along the modal's edge while the picker is up.
pub(crate) fn hints() -> Vec<crate::hints::Hint> {
    vec![
        keys::TOGGLE.hint().kept(),
        keys::VALUE.hint(),
        keys::FACET.hint(),
        keys::DONE.hint().kept(),
    ]
}

/// A key while a modal's FILTER PICK is up, on its `pick` and its filter
/// line `query`: its cursor moves, `space` adds or takes out the value's
/// token in the line, and Enter or Esc puts it away. True when the line
/// changed — the modal's rows to narrow behind it.
pub fn apply_key(
    pick: &mut Option<FilterPick>,
    query: &mut TextInput,
    keys: &[FacetKey],
    facets: &[PickFacet],
    key: &crossterm::event::KeyEvent,
) -> bool {
    let Some(cursor) = pick else {
        return false;
    };
    match handle_key(cursor, facets, key) {
        PickKey::Moved => false,
        PickKey::Done => {
            *pick = None;
            false
        }
        PickKey::Toggle => match cursor.current(facets) {
            Some((facet, value)) => {
                toggle(query, keys, facet.key, &value.value);
                true
            }
            None => false,
        },
    }
}

/// What a key does to the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKey {
    /// Moved its cursor, or swallowed the key.
    Moved,
    /// `space`: toggle this facet key's value in the line.
    Toggle,
    /// Enter / Esc: put it away.
    Done,
}

/// Keys while the picker is up: every key is its own.
pub fn handle_key(
    pick: &mut FilterPick,
    facets: &[PickFacet],
    key: &crossterm::event::KeyEvent,
) -> PickKey {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Up => pick.step_value(facets, -1),
        KeyCode::Down => pick.step_value(facets, 1),
        KeyCode::Left => pick.step_facet(facets, -1),
        KeyCode::Right | KeyCode::Tab => pick.step_facet(facets, 1),
        KeyCode::BackTab => pick.step_facet(facets, -1),
        _ if keys::TOGGLE.matches(key) => return PickKey::Toggle,
        _ if keys::DONE.matches(key) || keys::FILTER.matches(key) => return PickKey::Done,
        _ => {}
    }
    PickKey::Moved
}

/// Draw the FILTER PICK into `area` (the reading pane's inside): a heading,
/// then one column — each facet's name, its values under it with their
/// tick, mark, name and count — scrolled to keep the cursor in view.
/// Returns nothing to hit-test: the picker is keys only.
pub fn draw_pick(
    f: &mut Frame,
    area: Rect,
    facets: &[PickFacet],
    parsed: &Parsed,
    pick: &FilterPick,
    th: Theme,
) {
    // Counts sit by their names, not across a wide pane.
    const MAX_W: usize = 48;
    let width = (area.width as usize).min(MAX_W);
    let head = vec![
        Line::from(Span::styled(
            "Filter",
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "space ticks a value; or just type it in the filter line",
            Style::default().fg(th.dim),
        )),
        Line::default(),
    ];
    let count_w = facets
        .iter()
        .flat_map(|f| &f.values)
        .map(|v| v.count.to_string().len())
        .max()
        .unwrap_or(1);
    let mut body: Vec<Line> = Vec::new();
    let mut cursor_line = 0;
    for (fi, facet) in facets.iter().enumerate() {
        if facet.values.is_empty() {
            continue;
        }
        if !body.is_empty() {
            body.push(Line::default());
        }
        let active = parsed.tokens.iter().any(|t| t.key == facet.key.key);
        let here = fi == pick.facet;
        body.push(Line::from(vec![
            Span::styled(
                facet.key.title.to_string(),
                Style::default()
                    .fg(if here { th.text } else { th.muted })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if active { " •" } else { "" },
                Style::default().fg(th.accent),
            ),
        ]));
        for (vi, value) in facet.values.iter().enumerate() {
            let at = here && vi == pick.value;
            if at {
                cursor_line = body.len();
            }
            let on = is_on(parsed, facet.key.key, &value.value);
            let mut name_style = Style::default().fg(if on { th.text } else { th.muted });
            if at {
                name_style = name_style.bg(th.sel_bg).add_modifier(Modifier::BOLD);
            }
            let mut spans = vec![
                Span::styled(if at { "▌" } else { " " }, Style::default().fg(th.accent)),
                Span::styled(if on { "✓ " } else { "  " }, Style::default().fg(th.accent)),
            ];
            let mut used = 3;
            if let Some((mark, color)) = &value.mark {
                spans.push(Span::styled(
                    format!("{mark} "),
                    Style::default().fg(*color),
                ));
                used += mark.chars().count() + 1;
            }
            let room = width.saturating_sub(used + count_w + 2);
            let name = truncate(&value.value, room);
            used += name.chars().count();
            spans.push(Span::styled(name, name_style));
            let pad = width.saturating_sub(used + count_w).max(1);
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(
                format!("{:>count_w$}", value.count),
                Style::default().fg(th.dim),
            ));
            body.push(Line::from(spans));
        }
    }
    if body.is_empty() {
        body.push(Line::from(Span::styled(
            "nothing to pick",
            Style::default().fg(th.dim),
        )));
    }
    let height = (area.height as usize).saturating_sub(head.len()).max(1);
    let start = crate::app::window_start(cursor_line, height);
    let lines: Vec<Line> = head
        .into_iter()
        .chain(body.into_iter().skip(start).take(height))
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: &[FacetKey] = &[
        FacetKey::new("label", "Label"),
        FacetKey::new("priority", "Priority").aliases(&["p"]),
        FacetKey::new("status", "Status"),
    ];

    fn values<'a>(pairs: &'a [(&'a str, &'a [&'a str])]) -> impl Fn(&str) -> Vec<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default()
        }
    }

    #[test]
    fn parse_splits_tokens_from_the_words() {
        let parsed = parse("fix p:high Label:Bug -label:wip login", KEYS);
        assert_eq!(parsed.text, "fix login");
        let got: Vec<(&str, &str, bool)> = parsed
            .tokens
            .iter()
            .map(|t| (t.key, t.value.as_str(), t.negated))
            .collect();
        assert_eq!(
            got,
            vec![
                ("priority", "high", false),
                ("label", "Bug", false),
                ("label", "wip", true)
            ]
        );
        assert_eq!(parsed.tokens[0].span, (4, 10));
    }

    #[test]
    fn quotes_hold_spaces_and_unknown_keys_are_text() {
        let parsed = parse("status:\"in progress\" http://x author:me", KEYS);
        assert_eq!(parsed.tokens.len(), 1);
        assert_eq!(parsed.tokens[0].value, "in progress");
        assert_eq!(parsed.text, "http://x author:me");
    }

    #[test]
    fn a_key_with_no_value_yet_narrows_nothing() {
        let parsed = parse("label:", KEYS);
        assert!(parsed.tokens.is_empty());
        assert_eq!(parsed.text, "");
        assert_eq!(parsed.spans, vec![(0, 6)]);
        assert!(!parsed.is_active());
    }

    #[test]
    fn values_match_as_a_prefix_of_any_word() {
        let row = [
            ("label", &["Export PDF", "bug"][..]),
            ("priority", &["High"][..]),
            ("status", &["In Progress"][..]),
        ];
        let ok = |q: &str| matches(&parse(q, KEYS).tokens, values(&row));
        assert!(ok("p:hi"));
        assert!(ok("label:pdf"));
        assert!(ok("status:in-progress"));
        assert!(ok("status:\"in progress\""));
        // Quoted is exact: the whole value, not a prefix of it.
        assert!(!ok("status:\"in prog\""));
        assert!(ok("label:\"bug\""));
        assert!(!ok("label:\"bu\""));
        assert!(!ok("p:low"));
        assert!(!ok("status:progressive"));
    }

    #[test]
    fn same_key_ors_different_keys_and_minus_excludes() {
        let row = [("label", &["bug"][..]), ("priority", &["Low"][..])];
        let ok = |q: &str| matches(&parse(q, KEYS).tokens, values(&row));
        assert!(ok("label:bug label:feature"));
        assert!(!ok("label:bug p:high"));
        assert!(ok("label:bug p:high p:low"));
        assert!(!ok("-label:bug"));
        assert!(ok("-label:wip"));
        // A row with no value for a key fails a positive token on it.
        assert!(!ok("status:todo"));
        assert!(ok("-status:todo"));
    }

    #[test]
    fn toggle_adds_then_takes_a_token_out() {
        let mut q = TextInput::with_text("fix");
        toggle(&mut q, KEYS, "label", "Export PDF");
        assert_eq!(q.as_str(), "fix label:\"Export PDF\"");
        toggle(&mut q, KEYS, "priority", "High");
        assert_eq!(q.as_str(), "fix label:\"Export PDF\" priority:\"High\"");
        toggle(&mut q, KEYS, "label", "export pdf");
        assert_eq!(q.as_str(), "fix priority:\"High\"");
        // An alias the user typed is still the same token.
        let mut q = TextInput::with_text("p:high bug");
        toggle(&mut q, KEYS, "priority", "High");
        assert_eq!(q.as_str(), "bug");
    }

    #[test]
    fn tally_counts_each_value_once_per_row() {
        let rows = [vec!["bug", "ui"], vec!["Bug"], vec![]];
        let t = by_count(tally(rows.iter(), |r: &Vec<&str>| {
            r.iter().map(|s| s.to_string()).collect()
        }));
        assert_eq!(t, vec![("bug".to_string(), 2), ("ui".to_string(), 1)]);
    }

    #[test]
    fn the_pick_walks_and_clamps() {
        let facet = |n: usize| PickFacet {
            key: KEYS[0],
            values: (0..n)
                .map(|i| PickValue {
                    value: i.to_string(),
                    count: 1,
                    mark: None,
                })
                .collect(),
        };
        let facets = vec![facet(3), facet(0), facet(1)];
        let mut pick = FilterPick::default();
        // ↓ walks one column: past the first facet's last value onto the
        // next facet that has any, and stops at the bottom.
        pick.step_value(&facets, 2);
        assert_eq!((pick.facet, pick.value), (0, 2));
        pick.step_value(&facets, 1);
        assert_eq!((pick.facet, pick.value), (2, 0));
        pick.step_value(&facets, 5);
        assert_eq!((pick.facet, pick.value), (2, 0));
        pick.step_value(&facets, -2);
        assert_eq!((pick.facet, pick.value), (0, 1));
        // ←/→ jump facets, round either end, over an empty one.
        pick.step_facet(&facets, 1);
        assert_eq!((pick.facet, pick.value), (2, 0));
        pick.step_facet(&facets, 1);
        assert_eq!(pick.facet, 0);
        pick.step_facet(&facets, -1);
        assert_eq!(pick.facet, 2);
        pick.value = 9;
        pick.clamp(&facets);
        assert_eq!(pick.value, 0);
        pick.facet = 1;
        pick.clamp(&facets);
        assert_eq!((pick.facet, pick.value), (2, 0), "off an emptied facet");
    }

    #[test]
    fn the_keys_parse() {
        for key in keys::ALL {
            assert!(key.parses(), "{key:?}");
        }
    }
}
