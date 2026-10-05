//! The SKILLS BROWSER (`⌘S`): every agent skill on this machine in one
//! searchable list — your own (`~/.claude/skills`), the selected
//! checkout's (`.claude/skills`, `.cursor/skills`, …), the other harnesses'
//! (`~/.cursor/skills`, `~/.codex/skills`, `~/.agents/skills`) and the
//! installed Claude Code plugins' — each read on the right, edited in the
//! BUILT-IN EDITOR, and moved to the Trash when it is done with.
//!
//! Nothing here picks a skill for a launch: an agent finds its skills on
//! its own. This is for keeping the files — seeing what is there and what
//! each one says, fixing the ones that have gone stale, dropping the rest.
//!
//! A skill is a folder holding a `SKILL.md` whose YAML frontmatter names
//! it and says when to use it. The folders are read off the loop (a
//! BACKGROUND READ, `view_jobs`), and a skill reached two ways — a
//! `~/.claude/skills` that is a symlink to `~/.cursor/skills`, say — is
//! listed once, under the folder its symlinks resolve to.
//!
//! A delete is a move into the Trash, never a removal: `~/.Trash` on
//! macOS (a rename, so Finder has no Put Back for it — drag it out), the
//! freedesktop.org Trash (`~/.local/share/Trash`) elsewhere, recorded so a
//! file manager can restore it. A plugin's skills are read-only — the
//! plugin owns them, and its next update would put back whatever was
//! changed or removed.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::app::{
    clamp_selection, window_start, App, ConfirmDialog, Overlay, PendingAction, PromptKind,
};
use crate::markdown::{self, Breaks};
use crate::text_input::TextInput;
use crate::theme::Theme;
use crate::ui::{
    centered_rect_pct, empty_list_row, fuzzy_highlight_styled, panel_block, render_row, row_rect,
    search_line, truncate, visible_positions, SPLIT_MODAL_PCT, SPLIT_PANE_LAYOUT_MIN,
};

const LIST_PCT: u16 = crate::pr_modal::LIST_PCT;
const MIN_LIST_W: u16 = crate::pr_modal::MIN_LIST_W;
const WHEEL_LINES: i32 = crate::pr_modal::WHEEL_LINES;

/// The file that makes a folder a skill.
pub const SKILL_FILE: &str = "SKILL.md";
/// How much of a SKILL.md is read: a skill is a page or two of
/// instructions, and the list holds every one of them while it is open.
const MAX_SKILL_BYTES: u64 = 256 * 1024;
/// The preview lists this many of a skill's other files, then counts the
/// rest.
const MAX_FILES: usize = 200;
/// How deep that list goes into a skill's own folders.
const MAX_FILE_DEPTH: usize = 6;
/// How many files it counts before it stops looking: a skill that vendors
/// a `node_modules` is not walked to the bottom.
const MAX_COUNTED: usize = 5_000;
/// Claude Code's limit on a skill's `name`.
const MAX_NAME: usize = 64;
/// The line of a new skill's SKILL.md the editor opens on: its
/// `description`, the one line every new skill needs written.
const STUB_DESCRIPTION_LINE: u64 = 3;

/// Where a skill was found — the badge its row wears. The order is the
/// list's: your own first, the checkout's, the other harnesses', the
/// plugins' last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// Claude Code's user skills: `$CLAUDE_CONFIG_DIR/skills`, else
    /// `~/.claude/skills`.
    User,
    /// The selected checkout's own: its `.claude/skills`, `.cursor/skills`,
    /// `.codex/skills` and `.agents/skills`.
    Project,
    /// Cursor's user skills, `~/.cursor/skills`, when that is not the same
    /// folder as Claude's.
    Cursor,
    /// Codex's: `$CODEX_HOME/skills`, else `~/.codex/skills`.
    Codex,
    /// The harness-neutral `~/.agents/skills`.
    Agents,
    /// An installed Claude Code plugin's — read-only.
    Plugin,
}

impl Source {
    pub fn badge(self) -> &'static str {
        match self {
            Source::User => "user",
            Source::Project => "project",
            Source::Cursor => "cursor",
            Source::Codex => "codex",
            Source::Agents => "agents",
            Source::Plugin => "plugin",
        }
    }

    /// The plugin owns the folder: nothing here edits it away.
    pub fn read_only(self) -> bool {
        self == Source::Plugin
    }
}

/// The Trash a delete moves a skill into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trash {
    /// macOS: `~/.Trash`, the folder Finder empties.
    Mac(PathBuf),
    /// Elsewhere: the freedesktop.org Trash (`$XDG_DATA_HOME/Trash`), each
    /// entry in its `files/` with an `info/<name>.trashinfo` saying where it
    /// came from, so a file manager can put it back.
    Freedesktop(PathBuf),
}

/// The folders the browser reads and writes, captured when it opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Places {
    pub home: Option<PathBuf>,
    /// Claude Code's config dir (`orion_core::paths::claude_config_dir`).
    pub claude: Option<PathBuf>,
    /// Codex's home: `$CODEX_HOME`, else `~/.codex`.
    pub codex: Option<PathBuf>,
    pub trash: Option<Trash>,
}

impl Places {
    /// This machine's, from the environment. A unit test gets the ones
    /// [`with_places`] pinned, or none at all — never the developer's own
    /// skills, and never their Trash.
    pub fn of_this_machine() -> Self {
        if cfg!(test) {
            return pinned().unwrap_or_default();
        }
        let home = orion_core::env::home_dir();
        let codex = orion_core::env::non_empty(orion_core::env::CODEX_HOME)
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".codex")));
        let trash = if cfg!(target_os = "macos") {
            home.as_ref().map(|h| Trash::Mac(h.join(".Trash")))
        } else {
            orion_core::env::non_empty("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join(".local/share")))
                .map(|data| Trash::Freedesktop(data.join("Trash")))
        };
        Self {
            claude: orion_core::paths::claude_config_dir(),
            home,
            codex,
            trash,
        }
    }

    /// Where a new skill goes: Claude Code's user skills folder.
    pub fn user_skills(&self) -> Option<PathBuf> {
        self.claude.as_ref().map(|c| c.join("skills"))
    }
}

#[cfg(test)]
thread_local! {
    static PLACES_OVERRIDE: std::cell::RefCell<Option<Places>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn pinned() -> Option<Places> {
    PLACES_OVERRIDE.with(|p| p.borrow().clone())
}

#[cfg(not(test))]
fn pinned() -> Option<Places> {
    None
}

/// Pin [`Places::of_this_machine`] to `places` for the duration of `f`.
#[cfg(test)]
pub fn with_places<T>(places: Places, f: impl FnOnce() -> T) -> T {
    PLACES_OVERRIDE.with(|slot| {
        let prev = slot.replace(Some(places));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// One folder of skills, and what its rows are badged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub dir: PathBuf,
    pub source: Source,
    /// The plugin a [`Source::Plugin`] folder belongs to, which its skills'
    /// names carry as `plugin:skill` — the way Claude Code lists them.
    pub plugin: Option<String>,
}

impl Root {
    fn new(dir: PathBuf, source: Source) -> Self {
        Self {
            dir,
            source,
            plugin: None,
        }
    }
}

/// Every folder skills are read from, in the list's order. A folder that
/// is not there is simply skipped when it is read.
pub fn roots(places: &Places, checkout: Option<&Path>) -> Vec<Root> {
    let mut roots = Vec::new();
    if let Some(dir) = places.user_skills() {
        roots.push(Root::new(dir, Source::User));
    }
    if let Some(checkout) = checkout {
        for harness in [".claude", ".cursor", ".codex", ".agents"] {
            roots.push(Root::new(
                checkout.join(harness).join("skills"),
                Source::Project,
            ));
        }
    }
    if let Some(home) = &places.home {
        roots.push(Root::new(home.join(".cursor/skills"), Source::Cursor));
    }
    if let Some(codex) = &places.codex {
        roots.push(Root::new(codex.join("skills"), Source::Codex));
    }
    if let Some(home) = &places.home {
        roots.push(Root::new(home.join(".agents/skills"), Source::Agents));
    }
    if let Some(claude) = &places.claude {
        roots.extend(plugin_roots(claude));
    }
    roots
}

/// The `skills` folder of every installed Claude Code plugin, from the
/// install paths Claude Code records in `plugins/installed_plugins.json`
/// (an entry per install, keyed `plugin@marketplace`). The marketplaces'
/// own checkouts are not read: they hold what could be installed, not
/// what is.
fn plugin_roots(claude: &Path) -> Vec<Root> {
    let Ok(text) = std::fs::read_to_string(claude.join("plugins/installed_plugins.json")) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(plugins) = json.get("plugins").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for (key, entry) in plugins {
        let name = key.split('@').next().unwrap_or(key);
        // One install per scope in the current format, a single object in
        // the first one.
        let installs = match entry {
            serde_json::Value::Array(all) => all.iter().collect(),
            one => vec![one],
        };
        for install in installs {
            if let Some(path) = install.get("installPath").and_then(|p| p.as_str()) {
                roots.push(Root {
                    dir: PathBuf::from(path).join("skills"),
                    source: Source::Plugin,
                    plugin: Some(name.to_string()),
                });
            }
        }
    }
    roots
}

/// One skill, read whole: the list holds it while the modal is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The frontmatter's `name`, else the folder's; `plugin:name` for a
    /// plugin's.
    pub name: String,
    /// The frontmatter's `description`, on one line.
    pub description: String,
    pub source: Source,
    /// The folder as listed, under its root, symlinks and all — where the
    /// editor opens it.
    pub dir: PathBuf,
    /// The folder with every symlink resolved: what makes two listings one
    /// skill, and what a delete moves.
    pub resolved: PathBuf,
    /// The frontmatter's other fields, as written (`allowed-tools`,
    /// `model`, …).
    pub extra: Vec<(String, String)>,
    /// SKILL.md after its frontmatter.
    pub body: String,
    /// The folder's other files, relative and sorted.
    pub files: Vec<String>,
    /// How many more files there are than `files` holds.
    pub more_files: usize,
    /// What is wrong with the frontmatter, when something is.
    pub warning: Option<String>,
}

impl Skill {
    /// The text the filter matches and the row shows: the name, then the
    /// description.
    pub fn label(&self) -> String {
        if self.description.is_empty() {
            self.name.clone()
        } else {
            format!("{}  {}", self.name, self.description)
        }
    }
}

/// Read every skill under `roots`, in their order. A folder reached twice —
/// one root a symlink to another, or one skill linked into two roots — is
/// read once, under the first root that reaches it.
pub fn discover(roots: &[Root]) -> Vec<Skill> {
    let mut seen_roots = HashSet::new();
    let mut seen = HashSet::new();
    let mut skills = Vec::new();
    for root in roots {
        let Ok(canonical) = std::fs::canonicalize(&root.dir) else {
            continue;
        };
        if !seen_roots.insert(canonical) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&root.dir) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|dir| !is_hidden(dir) && dir.join(SKILL_FILE).is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let Ok(resolved) = std::fs::canonicalize(&dir) else {
                continue;
            };
            if seen.insert(resolved.clone()) {
                skills.push(read_skill(dir, resolved, root));
            }
        }
    }
    skills.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    skills
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

fn read_skill(dir: PathBuf, resolved: PathBuf, root: &Root) -> Skill {
    let text = read_capped(&dir.join(SKILL_FILE));
    let folder = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (front, body) = split_frontmatter(&text);
    let (mut fields, warning) = match front {
        Frontmatter::Fields(fields) => {
            let warning = fields
                .iter()
                .all(|(key, value)| key != "description" || value.trim().is_empty())
                .then(|| "no description — an agent decides when to use a skill by it".to_string());
            (fields, warning)
        }
        Frontmatter::Unclosed => (
            Vec::new(),
            Some("the frontmatter never closes — no second `---` line".into()),
        ),
        Frontmatter::Missing => (
            Vec::new(),
            Some("no frontmatter — an agent reads a skill's name and description from it".into()),
        ),
    };
    let mut take = |key: &str| {
        fields
            .iter()
            .position(|(k, _)| k == key)
            .map(|i| fields.remove(i).1)
            .unwrap_or_default()
    };
    let name = Some(take("name"))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or(folder);
    let description = take("description")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let name = match &root.plugin {
        Some(plugin) => format!("{plugin}:{name}"),
        None => name,
    };
    let (files, more_files) = other_files(&resolved);
    Skill {
        name,
        description,
        source: root.source,
        dir,
        resolved,
        extra: fields,
        body: body.to_string(),
        files,
        more_files,
        warning,
    }
}

/// A file's text, no more than [`MAX_SKILL_BYTES`] of it, any bytes that
/// are not UTF-8 replaced.
fn read_capped(path: &Path) -> String {
    let mut bytes = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(MAX_SKILL_BYTES).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Every file in a skill's folder but its SKILL.md, relative and in path
/// order, at most [`MAX_FILES`] of them — and how many more there were,
/// counted up to [`MAX_COUNTED`]. Hidden files are left out, and a
/// symlinked folder is listed but not entered, so a link back up the tree
/// cannot loop.
fn other_files(dir: &Path) -> (Vec<String>, usize) {
    let mut files = Vec::new();
    let mut more = 0;
    walk(dir, Path::new(""), 0, &mut files, &mut more);
    (files, more)
}

fn walk(dir: &Path, rel: &Path, depth: usize, files: &mut Vec<String>, more: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<std::fs::DirEntry> = entries
        .flatten()
        .filter(|entry| !is_hidden(&entry.path()))
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if files.len() + *more >= MAX_COUNTED {
            return;
        }
        let rel = rel.join(entry.file_name());
        if entry.file_type().is_ok_and(|t| t.is_dir()) && depth + 1 < MAX_FILE_DEPTH {
            walk(&entry.path(), &rel, depth + 1, files, more);
        } else if rel == Path::new(SKILL_FILE) {
            continue;
        } else if files.len() < MAX_FILES {
            files.push(rel.to_string_lossy().into_owned());
        } else {
            *more += 1;
        }
    }
}

/// What a SKILL.md opens with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frontmatter {
    /// A `---` block, closed, read into its top-level fields.
    Fields(Vec<(String, String)>),
    /// A `---` line with no second one: the whole file is the body.
    Unclosed,
    /// No `---` first line at all.
    Missing,
}

/// SKILL.md split into its frontmatter and the body after it. The block's
/// YAML is read only as deep as a skill's frontmatter goes — a key per
/// line, its value plain, quoted, a `|` / `>` block or a list — so a file
/// no YAML parser would take still shows its name and description rather
/// than nothing.
pub fn split_frontmatter(text: &str) -> (Frontmatter, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next().filter(|line| line.trim_end() == "---") else {
        return (Frontmatter::Missing, text);
    };
    let mut offset = first.len();
    let mut block = Vec::new();
    for line in lines {
        offset += line.len();
        let bare = line.trim_end();
        if bare == "---" || bare == "..." {
            return (Frontmatter::Fields(fields(&block)), &text[offset..]);
        }
        block.push(line.trim_end_matches(['\n', '\r']));
    }
    (Frontmatter::Unclosed, text)
}

/// The block's top-level `key: value` fields, in order. A line that is no
/// field — a comment, a stray indent, a key with spaces in it — is
/// skipped.
fn fields(lines: &[&str]) -> Vec<(String, String)> {
    let indented = |line: &str| line.starts_with([' ', '\t']);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if indented(line) || line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, head)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let plain_key = !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !plain_key {
            continue;
        }
        let head = head.trim();
        // What follows the key on later lines: anything indented, a list
        // written flush with its key, and the blank lines inside a block.
        let mut rest = Vec::new();
        while i < lines.len() {
            let next = lines[i];
            let blank_inside = next.trim().is_empty()
                && lines[i + 1..]
                    .iter()
                    .find(|l| !l.trim().is_empty())
                    .is_some_and(|l| indented(l));
            if indented(next) || blank_inside || (head.is_empty() && next.starts_with("- ")) {
                rest.push(next);
                i += 1;
            } else {
                break;
            }
        }
        out.push((key.to_string(), value(head, &rest)));
    }
    out
}

/// One field's value as text: a `|` block keeps its lines, a `>` block
/// and a plain value run on over several lines fold into one, a list
/// becomes `a, b, c`, and quotes come off.
fn value(head: &str, rest: &[&str]) -> String {
    let trimmed: Vec<&str> = rest.iter().map(|l| l.trim()).collect();
    if let Some(style) = head.chars().next().filter(|c| matches!(c, '|' | '>')) {
        let indent = rest
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let lines: Vec<&str> = rest
            .iter()
            .map(|l| l.get(indent..).unwrap_or("").trim_end())
            .collect();
        return if style == '|' {
            lines.join("\n").trim_end().to_string()
        } else {
            fold(&lines)
        };
    }
    if head.is_empty() {
        if trimmed.iter().any(|l| l.starts_with("- ") || *l == "-") {
            return trimmed
                .iter()
                .filter_map(|l| l.strip_prefix('-'))
                .map(|item| unquote(item.trim()))
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
        }
        return trimmed
            .iter()
            .filter(|l| !l.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
    }
    let mut joined = head.to_string();
    for line in trimmed.iter().filter(|l| !l.is_empty()) {
        joined.push(' ');
        joined.push_str(line);
    }
    if joined.starts_with(['"', '\'']) {
        return unquote(&joined);
    }
    // A plain value's comment starts at ` #`.
    match joined.find(" #") {
        Some(at) => joined[..at].trim_end().to_string(),
        None => joined,
    }
}

/// A `>` block's lines folded: a line break is a space, a blank line a
/// line break.
fn fold(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut pending_break = false;
    for line in lines {
        if line.is_empty() {
            pending_break = !out.is_empty();
            continue;
        }
        if pending_break {
            out.push('\n');
            pending_break = false;
        } else if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

/// A YAML scalar with its quotes off: `"…"` with its backslash escapes,
/// `'…'` with its doubled quote. Anything else as it is.
fn unquote(text: &str) -> String {
    if let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match (c, c == '\\') {
                (_, true) => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => out.push('\\'),
                },
                (c, false) => out.push(c),
            }
        }
        return out;
    }
    if let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    text.to_string()
}

/// `typed` as a skill's name, the way Claude Code takes one: lowercase
/// letters, digits and single hyphens, at most [`MAX_NAME`] long —
/// "Release notes" is `release-notes`.
pub fn skill_name(typed: &str) -> String {
    let mut name = String::new();
    for word in typed
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !name.is_empty() {
            name.push('-');
        }
        name.push_str(&word.to_ascii_lowercase());
    }
    name.truncate(MAX_NAME);
    name.trim_end_matches('-').to_string()
}

/// A new skill's SKILL.md: the frontmatter every skill needs, its
/// description left to be written.
fn stub(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: What this skill does, and when an agent should use it.\n---\n\n# {name}\n\n"
    )
}

/// Make `dir` a new skill named `name`. Refuses a folder that is already
/// there rather than writing into it.
pub fn make_skill(dir: &Path, name: &str) -> std::io::Result<()> {
    if std::fs::symlink_metadata(dir).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "there is a folder of that name already",
        ));
    }
    std::fs::create_dir_all(dir)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(SKILL_FILE))?;
    std::io::Write::write_all(&mut file, stub(name).as_bytes())
}

/// Move the skill folder `dir` into `trash`, under its own name or the
/// first of `name 2`, `name 3`, … the Trash does not hold yet, and return
/// where it landed. A rename, never a copy and a delete: across volumes it
/// fails, and the skill stays where it was.
pub fn move_to_trash(dir: &Path, trash: &Trash) -> std::io::Result<PathBuf> {
    // The confirm named a skill; a folder that stopped being one since —
    // moved, emptied, replaced — is not what was agreed to.
    if !dir.join(SKILL_FILE).is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} has no {SKILL_FILE} any more", dir.display()),
        ));
    }
    move_folder_to_trash(dir, trash)
}

/// [`move_to_trash`] for any folder — a removed Claude account's config
/// dir — with no check of what it holds.
pub fn move_folder_to_trash(dir: &Path, trash: &Trash) -> std::io::Result<PathBuf> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no folder name"))?;
    // macOS keeps the folder itself; freedesktop.org keeps it in `files/`
    // with its record in `info/`, and a name is free only in both.
    let (files, info) = match trash {
        Trash::Mac(files) => (files.clone(), None),
        Trash::Freedesktop(root) => (root.join("files"), Some(root.join("info"))),
    };
    let record = |candidate: &str| {
        info.as_ref()
            .map(|info| info.join(format!("{candidate}.trashinfo")))
    };
    let taken = |path: &Path| std::fs::symlink_metadata(path).is_ok();
    let candidate = std::iter::once(name.clone())
        .chain((2..).map(|n| format!("{name} {n}")))
        .find(|candidate| {
            !taken(&files.join(candidate)) && !record(candidate).is_some_and(|r| taken(&r))
        })
        .expect("an unbounded range always finds a free name");
    std::fs::create_dir_all(&files)?;
    let landed = files.join(&candidate);
    let Some(record) = record(&candidate) else {
        std::fs::rename(dir, &landed)?;
        return Ok(landed);
    };
    // The spec's order: claim the name with the record, then move — and
    // take the record back out when the move fails.
    std::fs::create_dir_all(record.parent().unwrap_or(&files))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&record)?;
    let moved = std::io::Write::write_all(&mut file, trash_info(dir).as_bytes())
        .and_then(|()| std::fs::rename(dir, &landed));
    if let Err(e) = moved {
        let _ = std::fs::remove_file(&record);
        return Err(e);
    }
    Ok(landed)
}

/// A freedesktop.org `.trashinfo` for `path`: where it came from, percent-
/// encoded, and when it went — in UTC, where the spec asks for local
/// time, so a file manager's "deleted" column is off by the zone at most.
fn trash_info(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut encoded = String::new();
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    let when = orion_core::crashlog::format_timestamp(orion_core::clock::now_secs());
    format!(
        "[Trash Info]\nPath={encoded}\nDeletionDate={}\n",
        when.trim_end_matches('Z')
    )
}

/// `path` with the home directory spelled `~`.
pub(crate) fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The modal's own state, the skills included: read when it opens and
/// dropped when it closes.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillsView {
    pub places: Places,
    /// The checkout whose own skills are listed; None lists the machine's
    /// alone.
    pub checkout: Option<PathBuf>,
    /// The title's project name, beside the checkout.
    pub project_name: String,
    /// The editor command Enter launches, captured at open time.
    pub editor: String,
    pub skills: Vec<Skill>,
    /// The listing this view is waiting on, by ticket (`view_jobs`). The
    /// rows meanwhile are the last listing's — none, on a fresh open.
    pub listing: Option<u64>,
    /// The folder to put the cursor on when the listing lands — a skill
    /// just made. None keeps it on the skill it is on.
    pub focus: Option<PathBuf>,
    pub query: TextInput,
    /// Index into `skills`, not into the filtered rows.
    pub selected: usize,
    pub scroll: u16,
    pub view_height: u16,
    pub body_lines: usize,
    /// Whole modal rect, written back during draw so clicks outside close.
    pub area: Rect,
    pub list_area: Rect,
    pub body_area: Rect,
    pub cursor_row: usize,
}

impl SkillsView {
    pub fn new(
        places: Places,
        checkout: Option<PathBuf>,
        project_name: String,
        editor: String,
    ) -> Self {
        Self {
            places,
            checkout,
            project_name,
            editor,
            skills: Vec::new(),
            listing: None,
            focus: None,
            query: TextInput::new(),
            selected: 0,
            scroll: 0,
            view_height: 0,
            body_lines: 0,
            area: Rect::default(),
            list_area: Rect::default(),
            body_area: Rect::default(),
            cursor_row: 0,
        }
    }

    pub fn max_scroll(&self) -> u16 {
        crate::app::max_scroll(self.body_lines, self.view_height)
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = crate::app::scrolled_by(self.scroll, delta, self.max_scroll());
    }

    /// A fresh listing: the cursor stays on the skill it was on, or goes
    /// to [`Self::focus`], or is clamped when its skill went.
    pub fn set_skills(&mut self, skills: Vec<Skill>) {
        let target = self
            .focus
            .take()
            .or_else(|| self.skills.get(self.selected).map(|s| s.resolved.clone()));
        let before = self.selected;
        self.selected = target
            .and_then(|dir| skills.iter().position(|s| s.resolved == dir))
            .unwrap_or_else(|| clamp_selection(self.selected as i64, skills.len()));
        if self.selected != before {
            self.scroll = 0;
        }
        self.skills = skills;
        self.listing = None;
    }

    fn visible(&self) -> Vec<(usize, Vec<usize>)> {
        visible_rows(&self.query, &self.skills)
    }

    /// The skill under the cursor: the selected one while the filter
    /// shows it, else the filter's best match.
    pub fn cursor(&self) -> Option<usize> {
        let filtering = self.query.split_whitespace().next().is_some();
        self.cursor_in(&if filtering {
            self.visible()
        } else {
            Vec::new()
        })
    }

    /// [`Self::cursor`] against rows the caller already ranked, so a draw
    /// or a step filters the list once.
    fn cursor_in(&self, visible: &[(usize, Vec<usize>)]) -> Option<usize> {
        if self.skills.is_empty() {
            return None;
        }
        if self.query.split_whitespace().next().is_none() {
            return Some(clamp_selection(self.selected as i64, self.skills.len()));
        }
        if visible.iter().any(|(i, _)| *i == self.selected) {
            Some(self.selected)
        } else {
            visible.first().map(|(i, _)| *i)
        }
    }

    pub fn selected_skill(&self) -> Option<&Skill> {
        self.skills.get(self.cursor()?)
    }
}

fn visible_rows(query: &TextInput, skills: &[Skill]) -> Vec<(usize, Vec<usize>)> {
    let labels: Vec<String> = skills.iter().map(Skill::label).collect();
    crate::fuzzy::rank(query.as_str(), labels.iter().map(String::as_str))
}

/// `⌘S`: the browser, on this machine's skills and the selected
/// checkout's. Behind the closed-tabs splash no checkout is on screen, so
/// none is read.
pub(crate) fn open(app: &mut App) {
    let checkout = if app.projects_closed {
        None
    } else {
        app.selected_worktree().map(|w| w.path.clone())
    };
    let project_name = checkout
        .as_ref()
        .and_then(|_| app.selected_project())
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let editor = crate::config::Config::load().editor_command();
    let view = SkillsView::new(Places::of_this_machine(), checkout, project_name, editor);
    app.overlay = Some(Overlay::Skills(view));
    list(app);
}

/// The browser back from a confirm or a prompt that stood in for it, read
/// afresh: whatever that was may have changed the folders.
pub(crate) fn reopen(app: &mut App, view: SkillsView) {
    app.overlay = Some(Overlay::Skills(view));
    list(app);
}

/// The editor over the browser closed: what was saved may have renamed
/// or redescribed a skill.
pub(crate) fn editor_closed(app: &mut App) {
    if matches!(&app.overlay, Some(Overlay::Skills(_))) {
        list(app);
    }
}

/// Read the skill folders again — off the loop when the loop is running,
/// inline for a view built by a test.
fn list(app: &mut App) {
    let jobs = app.view_jobs.clone();
    let Some(Overlay::Skills(view)) = &mut app.overlay else {
        return;
    };
    let (places, checkout) = (view.places.clone(), view.checkout.clone());
    let read = move || discover(&roots(&places, checkout.as_deref()));
    match jobs {
        Some(jobs) => {
            let ticket = crate::view_jobs::ticket();
            view.listing = Some(ticket);
            jobs.run(move || {
                Some(crate::view_jobs::Answer::Skills {
                    ticket,
                    skills: read(),
                })
            });
        }
        None => view.set_skills(read()),
    }
    app.dirty = true;
}

/// A listing came back: hand it to the browser that asked, if it is still
/// the one on screen.
pub(crate) fn land(app: &mut App, ticket: u64, skills: Vec<Skill>) {
    if let Some(Overlay::Skills(view)) = &mut app.overlay {
        if view.listing == Some(ticket) {
            view.set_skills(skills);
        }
    }
}

/// The selected skill as the file overlays name a file — (folder,
/// `SKILL.md`, line 1) — for `⌘O`, which hands it to the OPEN IN APP
/// editor: the skill's folder as the window, its SKILL.md open in it.
pub(crate) fn selected_file(view: &SkillsView) -> Option<(PathBuf, String, u64)> {
    view.selected_skill()
        .map(|skill| (skill.dir.clone(), SKILL_FILE.to_string(), 1))
}

/// The footer's keys, the verbs first: the search row under the title
/// already says that typing filters.
/// The browser's own keys: one table [`handle_key`] matches and
/// [`hints`] spells, so the bottom border never names a key the browser
/// does not answer to. The letters are the filter's, so the verbs are
/// chords — the AGENT PRESETS list's `^A` / `^D`.
pub(crate) mod keys {
    use crate::hints::Key;

    pub const EDIT: Key = Key::new(&["enter"], "edit");
    pub const NEW: Key = Key::new(&["cmd+n", "ctrl+n"], "new");
    /// Every modal's remove verb: it asks first.
    pub const TRASH: Key = Key::new(&["cmd+w", "ctrl+w"], "trash");
    pub const REFRESH: Key = crate::issues::keys::REFRESH;
    /// `⌘O` is caught before the browser sees it (`overlay_file`); `^O`
    /// is its twin for a terminal that never sends ⌘.
    pub const IN_CURSOR: Key = crate::hints::IN_CURSOR;
    pub const COPY_PATH: Key = crate::hints::COPY_PATH;
    #[cfg(test)]
    pub const ALL: &[Key] = &[EDIT, NEW, TRASH, REFRESH, IN_CURSOR, COPY_PATH];
}

/// The bottom border's keys, for a browser that edits in `editor`.
pub(crate) fn hints(editor: &str) -> Vec<crate::hints::Hint> {
    vec![
        keys::EDIT.hint_as(format!("edit in {editor}")),
        keys::NEW.hint(),
        keys::TRASH.hint(),
        crate::hints::in_app_hint(),
        keys::COPY_PATH.hint(),
        keys::REFRESH.hint(),
        crate::hints::ESC_CLOSE.hint(),
    ]
}

pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    let Some(Overlay::Skills(view)) = &mut app.overlay else {
        return false;
    };
    view.query.insert_str(text);
    query_changed(app);
    true
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Skills(view)) = &mut app.overlay else {
        return;
    };
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let page = view.view_height.max(1) as i32;
    match key.code {
        // Esc closes the browser, filter and all (`closes_on_esc`); the
        // letters are the filter's, so the verbs are the chords of the
        // [`keys`] table.
        KeyCode::Down if shift => view.scroll_by(1),
        KeyCode::Up if shift => view.scroll_by(-1),
        KeyCode::Down => step(app, 1),
        KeyCode::Up => step(app, -1),
        KeyCode::PageDown => view.scroll_by(page),
        KeyCode::PageUp => view.scroll_by(-page),
        KeyCode::Home => view.scroll = 0,
        KeyCode::End => view.scroll = view.max_scroll(),
        _ if keys::EDIT.matches(&key) => edit_selected(app),
        _ if keys::NEW.matches(&key) => open_new_prompt(app),
        _ if keys::TRASH.matches(&key) => confirm_trash(app),
        _ if keys::REFRESH.matches(&key) => list(app),
        _ if keys::IN_CURSOR.matches(&key) => open_outside(app),
        _ if crate::hints::copies_path(&key, &view.query) => copy_path(app),
        _ => {
            if view.query.handle_key(&key).changed() {
                query_changed(app);
            }
        }
    }
    app.dirty = true;
}

/// The wheel walks the list or scrolls the page under it; a click puts
/// the cursor on a row, and a click on the row it is already on is Enter.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) {
    let Some(Overlay::Skills(view)) = &mut app.overlay else {
        return;
    };
    let (list, body) = (view.list_area, view.body_area);
    match mouse.kind {
        MouseEventKind::ScrollDown if body.contains(pos) => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp if body.contains(pos) => view.scroll_by(-WHEEL_LINES),
        MouseEventKind::ScrollDown if list.contains(pos) => step(app, 1),
        MouseEventKind::ScrollUp if list.contains(pos) => step(app, -1),
        MouseEventKind::Down(MouseButton::Left) => {
            let visible = view.visible();
            let first = window_start(view.cursor_row, list.height as usize);
            if let Some(row) = crate::list_hit::row_at(list, first, visible.len(), pos) {
                let index = visible[row].0;
                if view.cursor() == Some(index) {
                    edit_selected(app);
                } else {
                    view.selected = index;
                    view.scroll = 0;
                }
            }
        }
        _ => {}
    }
    app.dirty = true;
}

fn query_changed(app: &mut App) {
    let Some(Overlay::Skills(view)) = &mut app.overlay else {
        return;
    };
    view.scroll = 0;
    if let Some(i) = view.cursor() {
        view.selected = i;
    }
}

fn step(app: &mut App, delta: i32) {
    let Some(Overlay::Skills(view)) = &mut app.overlay else {
        return;
    };
    let visible = view.visible();
    if visible.is_empty() {
        return;
    }
    let here = view
        .cursor_in(&visible)
        .and_then(|c| visible.iter().position(|(i, _)| *i == c))
        .unwrap_or(0);
    let next = crate::app::clamp_selection(here as i64 + i64::from(delta), visible.len());
    view.selected = visible[next].0;
    view.scroll = 0;
}

/// Enter: the selected skill's SKILL.md in the BUILT-IN EDITOR — the
/// browser already shows its rendered page, so Enter is the edit. The
/// browser stays under it and reads the folders again when it closes.
fn edit_selected(app: &mut App) {
    let Some(Overlay::Skills(view)) = &app.overlay else {
        return;
    };
    let Some(skill) = view.selected_skill() else {
        return;
    };
    let (dir, editor) = (skill.dir.clone(), view.editor.clone());
    let size = crate::event_loop::vim_size_guess(app);
    crate::event_loop::spawn_editor_modal(app, &editor, &dir, SKILL_FILE, 1, size);
}

/// `^o`: what `⌘O` does — the skill's folder in the OPEN IN APP editor,
/// its SKILL.md open.
fn open_outside(app: &mut App) {
    let Some((dir, file, line)) = app.overlay.as_ref().and_then(|o| match o {
        Overlay::Skills(view) => selected_file(view),
        _ => None,
    }) else {
        return;
    };
    crate::event_loop::open_file_outside(app, &dir, &file, line);
}

/// `⌘C` / `^y`: the selected skill's SKILL.md path, ready to paste into an
/// agent.
fn copy_path(app: &mut App) {
    let Some(Overlay::Skills(view)) = &app.overlay else {
        return;
    };
    let Some(skill) = view.selected_skill() else {
        return;
    };
    let file = skill.dir.join(SKILL_FILE);
    let label = format!("copied {}", tilde(&file, view.places.home.as_deref()));
    crate::event_loop::copy_and_flash(app, &file.to_string_lossy(), &label);
}

/// `^a`: ask for the new skill's name; the browser comes back behind it.
fn open_new_prompt(app: &mut App) {
    let Some(Overlay::Skills(view)) = &app.overlay else {
        return;
    };
    if view.places.user_skills().is_none() {
        app.flash = Some(crate::flash::Flash::setup(
            "no skills folder — neither CLAUDE_CONFIG_DIR nor HOME is set",
        ));
        return;
    }
    let view = Box::new(view.clone());
    crate::event_loop::open_prompt(app, PromptKind::NewSkill { view });
}

/// The NEW SKILL prompt's label: what the name becomes.
pub(crate) fn new_skill_label(view: &SkillsView) -> String {
    let home = view.places.home.as_deref();
    let dir = view
        .places
        .user_skills()
        .map(|dir| tilde(&dir, home))
        .unwrap_or_else(|| "skills".into());
    format!("name — a new folder in {dir}")
}

/// The NEW SKILL prompt's Enter: make `<user skills>/<name>/SKILL.md` from
/// the stub, put the browser back with the cursor on it, and open it in
/// the editor at its description.
pub(crate) fn create(app: &mut App, mut view: SkillsView, typed: &str) {
    let name = skill_name(typed);
    let Some(root) = view.places.user_skills() else {
        reopen(app, view);
        return;
    };
    if name.is_empty() {
        reopen(app, view);
        return;
    }
    let dir = root.join(&name);
    let home = view.places.home.clone();
    if let Err(e) = make_skill(&dir, &name) {
        app.flash = Some(match e.kind() {
            std::io::ErrorKind::AlreadyExists => crate::flash::Flash::note(format!(
                "{} is there already",
                tilde(&dir, home.as_deref())
            )),
            _ => crate::flash::Flash::failed(format!(
                "couldn't make {}: {e}",
                tilde(&dir, home.as_deref())
            )),
        });
        view.focus = std::fs::canonicalize(&dir).ok();
        reopen(app, view);
        return;
    }
    view.query.clear();
    view.focus = std::fs::canonicalize(&dir).ok();
    let editor = view.editor.clone();
    reopen(app, view);
    let size = crate::event_loop::vim_size_guess(app);
    crate::event_loop::spawn_editor_modal(
        app,
        &editor,
        &dir,
        SKILL_FILE,
        STUB_DESCRIPTION_LINE,
        size,
    );
}

/// `^d`: the CONFIRM DIALOG before a skill goes to the Trash, naming it and
/// the folder that would move. A plugin's skill is refused here.
fn confirm_trash(app: &mut App) {
    let Some(Overlay::Skills(view)) = &app.overlay else {
        return;
    };
    let Some(skill) = view.selected_skill().cloned() else {
        return;
    };
    if skill.source.read_only() {
        app.flash = Some(crate::flash::Flash::note(format!(
            "{} comes with its plugin — /plugin in Claude Code removes the plugin",
            skill.name
        )));
        return;
    }
    // A skill folder that is itself a link (into a repo of skills, say) is
    // uninstalled by trashing the link: what it points at is the source,
    // not this machine's copy.
    let linked = std::fs::symlink_metadata(&skill.dir).is_ok_and(|m| m.file_type().is_symlink());
    let (dir, shown) = if linked {
        let shown = format!(
            "{}\n(a link to {}; only the link moves)",
            skill.dir.display(),
            skill.resolved.display()
        );
        (skill.dir, shown)
    } else {
        let shown = skill.resolved.display().to_string();
        (skill.resolved, shown)
    };
    let view = Box::new(view.clone());
    app.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Move skill to the Trash".into(),
        message: format!("Move the skill '{}' to the Trash?\n{shown}", skill.name),
        action: PendingAction::TrashSkill {
            view,
            dir,
            name: skill.name,
        },
        area: Rect::default(),
    }));
}

/// The confirm's yes: the folder into the Trash, and the browser back on
/// what is left — the footer saying why when it could not go.
pub(crate) fn trash(app: &mut App, view: SkillsView, dir: PathBuf, name: String) {
    let failed = match &view.places.trash {
        None => Some("HOME is not set".to_string()),
        Some(trash) => move_to_trash(&dir, trash).err().map(|e| e.to_string()),
    };
    if let Some(why) = failed {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "couldn't move {name} to the Trash: {why}"
        )));
    }
    reopen(app, view);
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &SkillsView, th: Theme) {
    let area = centered_rect_pct(f.area(), SPLIT_MODAL_PCT.0, SPLIT_MODAL_PCT.1);
    f.render_widget(Clear, area);
    let list_w = (area.width * LIST_PCT / 100)
        .max(MIN_LIST_W)
        .min(area.width.saturating_sub(SPLIT_PANE_LAYOUT_MIN));
    let [list_a, body_a] = Layout::horizontal([
        Constraint::Length(list_w),
        Constraint::Min(SPLIT_PANE_LAYOUT_MIN),
    ])
    .areas(area);

    let skills = &view.skills;
    let visible = view.visible();
    let cursor = view.cursor_in(&visible);
    let cursor_row = cursor
        .and_then(|c| visible.iter().position(|(i, _)| *i == c))
        .unwrap_or(0);
    let count = if view.query.split_whitespace().next().is_some() {
        format!("{}/{}", visible.len(), skills.len())
    } else {
        skills.len().to_string()
    };
    let head = if view.project_name.is_empty() {
        format!("Skills ({count})")
    } else {
        format!("Skills — {} ({count})", view.project_name)
    };
    let title = if view.listing.is_some() && !skills.is_empty() {
        format!("{head}, reading…")
    } else {
        head
    };
    let block = panel_block(&title, true, th);
    let list_inner = block.inner(list_a);
    f.render_widget(block, list_a);
    if let Some(query_area) = row_rect(list_inner, 0) {
        let line = search_line(&view.query, "type to filter…", query_area, th);
        f.render_widget(Paragraph::new(line), query_area);
    }
    let rows_area = crate::ui::below_first_row(list_inner);
    if skills.is_empty() {
        let text = if view.listing.is_some() {
            "reading the skill folders…".to_string()
        } else {
            let dir = view
                .places
                .user_skills()
                .map(|dir| tilde(&dir, view.places.home.as_deref()))
                .unwrap_or_else(|| "~/.claude/skills".into());
            format!("no skills yet — {} makes one in {dir}", keys::NEW.label())
        };
        empty_list_row(f, rows_area, &text, th);
    } else if visible.is_empty() {
        empty_list_row(f, rows_area, "no skills match", th);
    }
    let start = window_start(cursor_row, rows_area.height as usize);
    for (row, (index, positions)) in visible.iter().enumerate().skip(start) {
        let Some(row_area) = row_rect(rows_area, row - start) else {
            break;
        };
        let skill = &skills[*index];
        let full = skill.label();
        let badge = skill.source.badge();
        let budget = (rows_area.width as usize).saturating_sub(2);
        let badge_w = badge.chars().count();
        let label = truncate(&full, budget.saturating_sub(badge_w + 2));
        let pos = visible_positions(positions, &label, &full);
        let used = label.chars().count();
        let mut spans = row_spans(&label, skill.name.chars().count(), pos, th);
        if used + badge_w < budget {
            spans.push(Span::raw(" ".repeat(budget - used - badge_w)));
            spans.push(Span::styled(badge, Style::default().fg(th.dim)));
        }
        render_row(f, row_area, spans, Some(*index) == cursor, true, th);
    }

    let current = cursor.and_then(|i| skills.get(i));
    let body_title = current.map_or("Skill", |s| s.name.as_str());
    let width = body_a.width.saturating_sub(2) as usize;
    let lines = current
        .map(|skill| preview_lines(skill, width, view.places.home.as_deref(), th))
        .unwrap_or_default();
    let mut block = panel_block(body_title, false, th);
    let body_inner = block.inner(body_a);
    let max_scroll = (lines.len() as u16).saturating_sub(body_inner.height.max(1));
    let scroll = view.scroll.min(max_scroll);
    if max_scroll > 0 {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {}/{} ", scroll + 1, lines.len()),
                Style::default().fg(th.dim),
            ))
            .right_aligned(),
        );
    }
    f.render_widget(block, body_a);
    let body_lines = lines.len();
    let shown: Vec<Line> = lines.into_iter().skip(scroll as usize).collect();
    f.render_widget(Paragraph::new(shown), body_inner);
    // The browser's keys, along its bottom edge — under both frames,
    // clear of the reading pane's scroll position.
    let reserve = if max_scroll > 0 { 12 } else { 0 };
    crate::hints::draw_on_border(
        f,
        area,
        &hints(crate::ui::editor_name(&view.editor)),
        reserve,
        th,
    );

    if let Some(Overlay::Skills(v)) = &mut app.overlay {
        v.area = area;
        v.list_area = rows_area;
        v.cursor_row = cursor_row;
        v.body_area = body_inner;
        v.view_height = body_inner.height;
        v.body_lines = body_lines;
        if let Some(index) = cursor {
            v.selected = index;
        }
        v.scroll = scroll;
    }
}

/// A row's text: the name as plain text and the description after it dim,
/// each with the filter's matches lit. `name_len` is where the one ends.
fn row_spans(label: &str, name_len: usize, positions: &[usize], th: Theme) -> Vec<Span<'static>> {
    let name: String = label.chars().take(name_len).collect();
    let rest: String = label.chars().skip(name_len).collect();
    let (in_name, in_rest): (Vec<usize>, Vec<usize>) =
        positions.iter().partition(|&&p| p < name_len);
    let in_rest: Vec<usize> = in_rest.into_iter().map(|p| p - name_len).collect();
    let mut spans = fuzzy_highlight_styled(&name, &in_name, Style::default(), th);
    spans.extend(fuzzy_highlight_styled(
        &rest,
        &in_rest,
        Style::default().fg(th.dim),
        th,
    ));
    spans
}

/// The reading pane: the frontmatter compactly — the description, where
/// the folder is, the other fields — then the body as a rendered page,
/// then the skill's other files.
fn preview_lines(
    skill: &Skill,
    width: usize,
    home: Option<&Path>,
    th: Theme,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(th.dim);
    let width = width.max(20);
    let mut lines: Vec<Line<'static>> = if skill.description.is_empty() {
        vec![Line::from(Span::styled("(no description)", dim))]
    } else {
        markdown::render(
            &skill.description,
            width,
            Breaks::Reflow,
            Style::default().add_modifier(Modifier::BOLD),
            th,
        )
    };
    let mut place = format!("{} · {}", skill.source.badge(), tilde(&skill.dir, home));
    if skill.source.read_only() {
        place.push_str(" · read-only");
    }
    lines.push(Line::from(Span::styled(truncate(&place, width), dim)));
    // Reached through a symlink: where the files really are, which is what
    // a delete would move.
    if skill.dir != skill.resolved {
        let resolved = format!("→ {}", tilde(&skill.resolved, home));
        lines.push(Line::from(Span::styled(truncate(&resolved, width), dim)));
    }
    for (key, value) in &skill.extra {
        let field = format!(
            "{key}: {}",
            value.split_whitespace().collect::<Vec<_>>().join(" ")
        );
        lines.push(Line::from(Span::styled(truncate(&field, width), dim)));
    }
    if let Some(warning) = &skill.warning {
        lines.push(Line::from(Span::styled(
            truncate(&format!("⚠ {warning}"), width),
            Style::default().fg(th.warn),
        )));
    }
    lines.push(Line::from(""));
    lines.extend(markdown::render(
        skill.body.trim_start_matches(['\n', '\r']),
        width,
        Breaks::Reflow,
        Style::default(),
        th,
    ));
    if !skill.files.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Files",
            Style::default().fg(th.muted).add_modifier(Modifier::BOLD),
        )));
        for file in &skill.files {
            lines.push(Line::from(Span::styled(
                truncate(&format!("  {file}"), width),
                dim,
            )));
        }
        if skill.more_files > 0 {
            let counted_all = skill.files.len() + skill.more_files < MAX_COUNTED;
            let plus = if counted_all { "" } else { "+" };
            lines.push(Line::from(Span::styled(
                format!("  … and {}{plus} more", skill.more_files),
                dim,
            )));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::ClientRequest;

    fn write_skill(root: &Path, folder: &str, text: &str) -> PathBuf {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), text).unwrap();
        dir
    }

    fn front(name: &str, description: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\nBody of {name}.\n"
        )
    }

    fn names(skills: &[Skill]) -> Vec<(&str, Source)> {
        skills.iter().map(|s| (s.name.as_str(), s.source)).collect()
    }

    /// A home with Claude's skills folder, the way the owner's machine has
    /// it: `~/.claude/skills` a symlink to `~/.cursor/skills`.
    fn linked_home() -> (tempfile::TempDir, Places) {
        let home = tempfile::tempdir().unwrap();
        let cursor = home.path().join(".cursor/skills");
        std::fs::create_dir_all(&cursor).unwrap();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(&cursor, home.path().join(".claude/skills")).unwrap();
        let places = Places {
            home: Some(home.path().to_path_buf()),
            claude: Some(home.path().join(".claude")),
            codex: Some(home.path().join(".codex")),
            trash: Some(Trash::Mac(home.path().join(".Trash"))),
        };
        (home, places)
    }

    #[test]
    fn a_symlinked_skills_folder_lists_each_skill_once_as_the_users() {
        let (home, places) = linked_home();
        write_skill(
            &home.path().join(".cursor/skills"),
            "simplify",
            &front("simplify", "Clean up a diff"),
        );
        let skills = discover(&roots(&places, None));
        assert_eq!(names(&skills), [("simplify", Source::User)]);
        let skill = &skills[0];
        assert_eq!(skill.dir, home.path().join(".claude/skills/simplify"));
        assert_eq!(
            skill.resolved,
            std::fs::canonicalize(home.path().join(".cursor/skills/simplify")).unwrap(),
            "the folder its symlinks resolve to"
        );
    }

    #[test]
    fn one_skill_linked_into_two_folders_is_listed_under_the_first() {
        let (home, places) = linked_home();
        let codex = home.path().join(".codex/skills");
        std::fs::create_dir_all(&codex).unwrap();
        let real = write_skill(&codex, "shared", &front("shared", "In both"));
        std::os::unix::fs::symlink(&real, home.path().join(".cursor/skills/shared")).unwrap();
        write_skill(&codex, "codex-only", &front("codex-only", "Codex's alone"));
        let skills = discover(&roots(&places, None));
        assert_eq!(
            names(&skills),
            [("shared", Source::User), ("codex-only", Source::Codex)]
        );
    }

    #[test]
    fn the_checkouts_skills_are_project_and_sort_after_the_users() {
        let (home, places) = linked_home();
        write_skill(
            &home.path().join(".claude/skills"),
            "zeta",
            &front("zeta", "Mine"),
        );
        let checkout = tempfile::tempdir().unwrap();
        write_skill(
            &checkout.path().join(".claude/skills"),
            "deploy",
            &front("deploy", "Ship it"),
        );
        write_skill(
            &checkout.path().join(".cursor/skills"),
            "alpha",
            &front("alpha", "Cursor rule"),
        );
        // A folder with no SKILL.md, and a hidden one, are not skills.
        std::fs::create_dir_all(checkout.path().join(".claude/skills/notes")).unwrap();
        write_skill(
            &checkout.path().join(".claude/skills"),
            ".draft",
            &front("draft", "Hidden"),
        );
        let skills = discover(&roots(&places, Some(checkout.path())));
        assert_eq!(
            names(&skills),
            [
                ("zeta", Source::User),
                ("alpha", Source::Project),
                ("deploy", Source::Project),
            ]
        );
        let no_checkout = discover(&roots(&places, None));
        assert_eq!(names(&no_checkout), [("zeta", Source::User)]);
    }

    #[test]
    fn installed_plugins_skills_are_read_only_and_namespaced() {
        let (home, places) = linked_home();
        let install = home
            .path()
            .join(".claude/plugins/cache/market/review/1.0.0");
        write_skill(
            &install.join("skills"),
            "pr-check",
            &front("pr-check", "Review a PR"),
        );
        let json = serde_json::json!({
            "version": 2,
            "plugins": {
                "review@market": [{ "scope": "user", "installPath": install }],
                "gone@market": [{ "scope": "user", "installPath": "/no/such/place" }],
            }
        });
        std::fs::write(
            home.path().join(".claude/plugins/installed_plugins.json"),
            json.to_string(),
        )
        .unwrap();
        let skills = discover(&roots(&places, None));
        assert_eq!(names(&skills), [("review:pr-check", Source::Plugin)]);
        assert!(skills[0].source.read_only());
    }

    #[test]
    fn frontmatter_fields_read_plain_quoted_block_and_list_values() {
        let text = "---\n\
name: release-notes\n\
description: >\n  Writes release notes\n  from merged PRs.\n\
allowed-tools:\n  - Read\n  - \"Bash(git log:*)\"\n\
model: 'claude-opus'  \n\
# a comment\n\
license: MIT # the usual\n\
---\nBody\n";
        let (front, body) = split_frontmatter(text);
        assert_eq!(body, "Body\n");
        let Frontmatter::Fields(fields) = front else {
            panic!("expected fields, got {front:?}");
        };
        assert_eq!(
            fields,
            [
                ("name".to_string(), "release-notes".to_string()),
                (
                    "description".to_string(),
                    "Writes release notes from merged PRs.".to_string()
                ),
                (
                    "allowed-tools".to_string(),
                    "Read, Bash(git log:*)".to_string()
                ),
                ("model".to_string(), "claude-opus".to_string()),
                ("license".to_string(), "MIT".to_string()),
            ]
        );
    }

    #[test]
    fn a_literal_block_keeps_its_lines_and_a_plain_value_runs_on() {
        let text = "---\ndescription: |\n  one\n\n  two\nname: long\n  folded on\n---\n";
        let (Frontmatter::Fields(fields), body) = split_frontmatter(text) else {
            panic!("expected fields");
        };
        assert_eq!(body, "");
        assert_eq!(fields[0], ("description".into(), "one\n\ntwo".into()));
        assert_eq!(fields[1], ("name".into(), "long folded on".into()));
    }

    #[test]
    fn odd_frontmatter_falls_back_to_the_folder_and_says_why() {
        let root = tempfile::tempdir().unwrap();
        let skills_dir = root.path().join("skills");
        write_skill(
            &skills_dir,
            "bare",
            "# Just a page\n\nNo frontmatter here.\n",
        );
        write_skill(
            &skills_dir,
            "open-ended",
            "---\nname: never-closed\n# Page\n",
        );
        write_skill(
            &skills_dir,
            "silent",
            "---\r\nname: quiet\r\n---\r\nBody\r\n",
        );
        write_skill(
            &skills_dir,
            "weird",
            "---\n: no key\nnot a field\n  stray indent\nname:\n---\n",
        );
        let skills = discover(&[Root::new(skills_dir, Source::User)]);
        let by_folder = |folder: &str| {
            skills
                .iter()
                .find(|s| s.dir.ends_with(folder))
                .unwrap_or_else(|| panic!("{folder} not listed"))
        };

        let bare = by_folder("bare");
        assert_eq!(bare.name, "bare");
        assert_eq!(bare.description, "");
        assert!(bare.body.starts_with("# Just a page"));
        assert!(bare.warning.as_deref().unwrap().contains("no frontmatter"));

        let open = by_folder("open-ended");
        assert_eq!(open.name, "open-ended", "an unclosed block names nothing");
        assert!(open.body.starts_with("---\nname: never-closed"));
        assert!(open.warning.as_deref().unwrap().contains("never closes"));

        let silent = by_folder("silent");
        assert_eq!(silent.name, "quiet", "CRLF line ends");
        assert_eq!(silent.body, "Body\r\n");
        assert!(silent
            .warning
            .as_deref()
            .unwrap()
            .contains("no description"));

        let weird = by_folder("weird");
        assert_eq!(weird.name, "weird", "an empty name is the folder's");
        assert!(weird.extra.is_empty());
    }

    #[test]
    fn the_other_files_are_listed_relative_without_skill_md_or_dotfiles() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_skill(root.path(), "s", &front("s", "d"));
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(dir.join("references/guide.md"), "").unwrap();
        std::fs::write(dir.join("run.sh"), "").unwrap();
        std::fs::write(dir.join(".DS_Store"), "").unwrap();
        let skills = discover(&[Root::new(root.path().to_path_buf(), Source::User)]);
        assert_eq!(skills[0].files, ["references/guide.md", "run.sh"]);
        assert_eq!(skills[0].more_files, 0);
    }

    #[test]
    fn the_filter_matches_names_and_descriptions() {
        let skill = |name: &str, description: &str| Skill {
            name: name.into(),
            description: description.into(),
            source: Source::User,
            dir: PathBuf::from(format!("/s/{name}")),
            resolved: PathBuf::from(format!("/s/{name}")),
            extra: Vec::new(),
            body: String::new(),
            files: Vec::new(),
            more_files: 0,
            warning: None,
        };
        let mut view = SkillsView::new(Places::default(), None, String::new(), "vi".into());
        view.set_skills(vec![
            skill("simplify", "Clean up a diff"),
            skill("release-notes", "Write the changelog"),
            skill("audit-perf", "Find slow queries"),
        ]);
        let shown = |view: &SkillsView| -> Vec<String> {
            view.visible()
                .iter()
                .map(|(i, _)| view.skills[*i].name.clone())
                .collect()
        };
        view.query.set_text("changelog");
        assert_eq!(shown(&view), ["release-notes"], "by description");
        view.query.set_text("simp");
        assert_eq!(shown(&view), ["simplify"], "by name");
        assert_eq!(
            view.selected_skill().map(|s| s.name.as_str()),
            Some("simplify")
        );
        view.query.set_text("zzz");
        assert!(shown(&view).is_empty());
        assert_eq!(view.selected_skill(), None);
    }

    #[test]
    fn a_fresh_listing_keeps_the_cursor_on_its_skill() {
        let root = tempfile::tempdir().unwrap();
        for name in ["b", "c"] {
            write_skill(root.path(), name, &front(name, "x"));
        }
        let roots = [Root::new(root.path().to_path_buf(), Source::User)];
        let mut view = SkillsView::new(Places::default(), None, String::new(), "vi".into());
        view.set_skills(discover(&roots));
        view.selected = 1;
        write_skill(root.path(), "a", &front("a", "x"));
        view.set_skills(discover(&roots));
        assert_eq!(view.selected_skill().map(|s| s.name.as_str()), Some("c"));
        view.focus = Some(std::fs::canonicalize(root.path().join("a")).unwrap());
        view.set_skills(discover(&roots));
        assert_eq!(view.selected_skill().map(|s| s.name.as_str()), Some("a"));
    }

    #[test]
    fn the_trash_takes_the_folder_under_a_free_name() {
        let home = tempfile::tempdir().unwrap();
        let trash = Trash::Mac(home.path().join(".Trash"));
        let skills = home.path().join("skills");
        let first = write_skill(&skills, "notes", &front("notes", "x"));
        std::fs::write(first.join("extra.md"), "kept").unwrap();
        let landed = move_to_trash(&first, &trash).unwrap();
        assert_eq!(landed, home.path().join(".Trash/notes"));
        assert!(!first.exists(), "moved, not copied");
        assert_eq!(
            std::fs::read_to_string(landed.join("extra.md")).unwrap(),
            "kept"
        );

        let second = write_skill(&skills, "notes", &front("notes", "y"));
        assert_eq!(
            move_to_trash(&second, &trash).unwrap(),
            home.path().join(".Trash/notes 2"),
            "a name the Trash holds already gets a suffix"
        );
        assert!(
            home.path().join(".Trash/notes/SKILL.md").exists(),
            "the first one is untouched"
        );
    }

    #[test]
    fn the_trash_refuses_a_folder_that_is_no_skill() {
        let home = tempfile::tempdir().unwrap();
        let trash = Trash::Mac(home.path().join(".Trash"));
        let plain = home.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(move_to_trash(&plain, &trash).is_err());
        assert!(plain.exists());
    }

    #[test]
    fn the_freedesktop_trash_records_where_each_skill_came_from() {
        let home = tempfile::tempdir().unwrap();
        let trash = Trash::Freedesktop(home.path().join("Trash"));
        let dir = write_skill(
            &home.path().join("my skills"),
            "notes",
            &front("notes", "x"),
        );
        let landed = move_to_trash(&dir, &trash).unwrap();
        assert_eq!(landed, home.path().join("Trash/files/notes"));
        let info = std::fs::read_to_string(home.path().join("Trash/info/notes.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="), "{info}");
        assert!(info.contains("/my%20skills/notes\n"), "{info}");
        assert!(info.contains("DeletionDate="), "{info}");

        let again = write_skill(
            &home.path().join("my skills"),
            "notes",
            &front("notes", "y"),
        );
        assert_eq!(
            move_to_trash(&again, &trash).unwrap(),
            home.path().join("Trash/files/notes 2")
        );
        assert!(home.path().join("Trash/info/notes 2.trashinfo").exists());
    }

    #[test]
    fn a_typed_name_becomes_a_skill_name() {
        assert_eq!(skill_name("Release notes"), "release-notes");
        assert_eq!(skill_name("  fix--the_bug!  "), "fix-the-bug");
        assert_eq!(skill_name("✨"), "");
        assert_eq!(skill_name(&"a".repeat(80)).len(), MAX_NAME);
    }

    #[test]
    fn a_new_skill_is_a_stub_that_lists_and_is_never_written_over() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("skills/release-notes");
        make_skill(&dir, "release-notes").unwrap();
        let skills = discover(&[Root::new(root.path().join("skills"), Source::User)]);
        assert_eq!(names(&skills), [("release-notes", Source::User)]);
        assert!(!skills[0].description.is_empty());
        let text = std::fs::read_to_string(dir.join(SKILL_FILE)).unwrap();
        assert_eq!(
            text.lines()
                .nth(STUB_DESCRIPTION_LINE as usize - 1)
                .map(|l| l.starts_with("description:")),
            Some(true),
            "the editor opens on the description"
        );
        std::fs::write(dir.join(SKILL_FILE), "mine").unwrap();
        assert!(make_skill(&dir, "release-notes").is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join(SKILL_FILE)).unwrap(),
            "mine"
        );
    }

    /// The browser on a pinned home and an empty config, keys pressed the
    /// way the event loop hands them over.
    fn at_home(places: Places, f: impl FnOnce(&mut App)) {
        let config = tempfile::tempdir().unwrap();
        crate::config::with_config_path(config.path().join("config.json"), || {
            with_places(places, || {
                let mut app = App::new();
                open(&mut app);
                f(&mut app);
            })
        });
    }

    fn press(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        let mut out: Vec<ClientRequest> = Vec::new();
        crate::event_loop::handle_overlay_key(app, KeyEvent::new(code, mods), &mut out);
        assert!(out.is_empty(), "nothing goes to the daemon");
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn browser(app: &App) -> &SkillsView {
        match &app.overlay {
            Some(Overlay::Skills(view)) => view,
            other => panic!("expected the browser, got {other:?}"),
        }
    }

    /// `^d` asks first, naming the folder its symlinks resolve to; the yes
    /// moves that folder to the Trash and lists what is left.
    #[test]
    fn delete_asks_then_moves_the_skill_to_the_trash() {
        let (home, places) = linked_home();
        let skills = home.path().join(".cursor/skills");
        write_skill(&skills, "keep", &front("keep", "Stays"));
        write_skill(&skills, "old", &front("old", "Goes"));
        at_home(places, |app| {
            type_text(app, "old");
            press(app, KeyCode::Char('w'), KeyModifiers::CONTROL);
            let Some(Overlay::Confirm(confirm)) = &app.overlay else {
                panic!("expected the confirm, got {:?}", app.overlay);
            };
            let resolved = std::fs::canonicalize(skills.join("old")).unwrap();
            assert!(
                confirm.message.contains(&resolved.display().to_string()),
                "{}",
                confirm.message
            );

            press(app, KeyCode::Enter, KeyModifiers::NONE);
            assert!(home.path().join(".Trash/old/SKILL.md").exists());
            assert!(!resolved.exists());
            assert_eq!(names(&browser(app).skills), [("keep", Source::User)]);
        });
    }

    /// A skill that is a link into somewhere else (a repo of skills) loses
    /// the link to the Trash; the folder it points at stays put.
    #[test]
    fn deleting_a_linked_skill_trashes_the_link_not_its_source() {
        let (home, places) = linked_home();
        let source = write_skill(
            &home.path().join("repo/skills"),
            "shared",
            &front("shared", "Lives in a repo"),
        );
        std::os::unix::fs::symlink(&source, home.path().join(".cursor/skills/shared")).unwrap();
        at_home(places, |app| {
            type_text(app, "shared");
            press(app, KeyCode::Char('w'), KeyModifiers::CONTROL);
            let Some(Overlay::Confirm(confirm)) = &app.overlay else {
                panic!("expected the confirm, got {:?}", app.overlay);
            };
            assert!(
                confirm.message.contains("only the link moves"),
                "{}",
                confirm.message
            );

            press(app, KeyCode::Enter, KeyModifiers::NONE);
            let landed = home.path().join(".Trash/shared");
            assert!(std::fs::symlink_metadata(&landed)
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(source.join(SKILL_FILE).is_file(), "the source stays");
            assert!(std::fs::symlink_metadata(home.path().join(".cursor/skills/shared")).is_err());
        });
    }

    /// Esc on the confirm keeps the skill and puts the browser back.
    #[test]
    fn backing_out_of_the_confirm_keeps_the_skill() {
        let (home, places) = linked_home();
        write_skill(
            &home.path().join(".cursor/skills"),
            "old",
            &front("old", "x"),
        );
        at_home(places, |app| {
            press(app, KeyCode::Char('w'), KeyModifiers::CONTROL);
            assert!(matches!(app.overlay, Some(Overlay::Confirm(_))));
            press(app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(names(&browser(app).skills), [("old", Source::User)]);
            assert!(home.path().join(".cursor/skills/old/SKILL.md").exists());
        });
    }

    #[test]
    fn a_plugin_skill_is_never_offered_to_the_trash() {
        let mut view = SkillsView::new(Places::default(), None, String::new(), "vi".into());
        view.set_skills(vec![Skill {
            name: "review:pr-check".into(),
            description: String::new(),
            source: Source::Plugin,
            dir: "/p/skills/pr-check".into(),
            resolved: "/p/skills/pr-check".into(),
            extra: Vec::new(),
            body: String::new(),
            files: Vec::new(),
            more_files: 0,
            warning: None,
        }]);
        let mut app = App::new();
        app.overlay = Some(Overlay::Skills(view));
        press(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert!(
            matches!(app.overlay, Some(Overlay::Skills(_))),
            "no confirm"
        );
        assert!(app.flash.as_deref().unwrap().contains("plugin"));
    }

    /// `^a` asks for a name; Enter makes the skill — through the symlinked
    /// folder — and lands the cursor on it. Esc on the prompt is the
    /// browser again, nothing made.
    #[test]
    fn a_new_skill_from_the_prompt_lands_under_the_cursor() {
        let (home, places) = linked_home();
        at_home(places, |app| {
            press(app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            assert!(
                matches!(&app.overlay, Some(Overlay::Prompt(p)) if matches!(p.kind, PromptKind::NewSkill { .. })),
                "{:?}",
                app.overlay
            );
            press(app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(browser(app).skills.is_empty());

            press(app, KeyCode::Char('n'), KeyModifiers::CONTROL);
            type_text(app, "Release notes");
            press(app, KeyCode::Enter, KeyModifiers::NONE);
            let made = home.path().join(".cursor/skills/release-notes/SKILL.md");
            assert!(made.is_file(), "made through the symlinked folder");
            assert_eq!(
                browser(app).selected_skill().map(|s| s.name.as_str()),
                Some("release-notes")
            );
        });
    }

    /// INPUT PARITY: Enter, and a click on the row the cursor is already
    /// on, open the same SKILL.md in the editor; a click on another row
    /// only moves the cursor there.
    #[test]
    fn enter_and_a_click_on_the_cursor_row_both_edit() {
        use ratatui::{backend::TestBackend, Terminal};
        let (home, places) = linked_home();
        let skills = home.path().join(".cursor/skills");
        write_skill(&skills, "alpha", &front("alpha", "First"));
        write_skill(&skills, "beta", &front("beta", "Second"));
        at_home(places, |app| {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            app.vim_tx = Some(tx);
            // A shell stands in for the editor (`sh +1 SKILL.md` spawns fine).
            if let Some(Overlay::Skills(view)) = &mut app.overlay {
                view.editor = "/bin/sh".into();
            }
            let edited = |app: &mut App| {
                let mut vim = app.vim.take().expect("the editor is up");
                vim.kill();
                (vim.cwd.clone(), vim.file.clone())
            };

            press(app, KeyCode::Enter, KeyModifiers::NONE);
            let alpha = (
                home.path().join(".claude/skills/alpha"),
                SKILL_FILE.to_string(),
            );
            assert_eq!(edited(app), alpha);

            let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
            terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
            let list = browser(app).list_area;
            let click = |app: &mut App, row: u16| {
                let pos = Position::new(list.x + 2, list.y + row);
                let mouse = MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: pos.x,
                    row: pos.y,
                    modifiers: KeyModifiers::NONE,
                };
                handle_mouse(app, mouse, pos);
            };
            click(app, 1);
            assert!(app.vim.is_none(), "another row: the cursor moves, no more");
            assert_eq!(
                browser(app).selected_skill().map(|s| s.name.as_str()),
                Some("beta")
            );
            click(app, 1);
            let beta = (
                home.path().join(".claude/skills/beta"),
                SKILL_FILE.to_string(),
            );
            assert_eq!(edited(app), beta);
        });
    }
}
