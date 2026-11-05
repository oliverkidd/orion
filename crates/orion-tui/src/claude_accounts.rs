//! CLAUDE ACCOUNTS in the TUI: who each one is signed in as, the names it
//! goes by, and the verbs that change it — add one (sharing the default
//! account's setup with it), rename it, sign it in or out, remove it —
//! and the config dirs on this machine no account runs in any more,
//! which can be added back or moved to the Trash.
//!
//! An account is a registry row that runs Claude in a config dir of its
//! own ([`HarnessDescriptor::is_claude_account`]): built-in Claude — the
//! DEFAULT ACCOUNT — or a `claude_accounts` entry, its sessions Claude's
//! with the account's id beside.
//! Everywhere one is listed it goes by its name and the email Claude
//! Code records it signed in as, `Work (a@b.co)` — `Claude (a@b.co)`
//! while it has no name of its own ([`label`], [`name_of`]); a card, a
//! list row, the quick prompt and a preset — short of room — say the
//! name alone, else the email, and only on a machine with more than one
//! account, where `claude` would not say which ([`short_name`]). The
//! name is the user's ([`rename`]): its `harnesses.<id>.label`, beside
//! its model and effort. The id and the dir never change with it —
//! sessions point back at the id, and the login lives in the dir.
//!
//! Removing an account leaves its dir, login and all, where it was —
//! unless the remove sends it to the Trash — and a later add whose name
//! comes to the same id adopts it, login and all.
//!
//! The record is `.claude.json` (`orion_core::claude_account`), and it
//! can run to megabytes — Claude Code rewrites it as sessions run. So it
//! is never read on the loop: [`request_refresh`] stats every account's
//! record on the blocking pool, re-reads only the ones whose size or
//! mtime moved, and wakes the loop when a name changed; a frame only
//! looks the answer up. Sign-in and sign-out are Claude Code's own `claude
//! auth login` / `claude auth logout`, run in the editor modal's PTY with
//! the account's `CLAUDE_CONFIG_DIR` ([`login_command`], [`run`]), and
//! their exit asks for a fresh read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use orion_core::claude_account::{ClaudeAccount, Record};
use orion_core::harness::HarnessDescriptor;
use orion_core::AgentKind;

use crate::app::App;
use crate::config::Config;
use crate::paths_util::Places;

/// What the default account's config dir shares with a new account, as
/// symlinks: its instructions, settings, skills, agents, commands,
/// plugins and keybindings — never its login (`.claude.json`, the
/// credentials), its transcripts (`projects`), history, sessions or
/// caches, which are what make it a different account.
pub const SHARED_SETUP: &[&str] = &[
    "CLAUDE.md",
    "settings.json",
    "skills",
    "agents",
    "commands",
    "plugins",
    "keybindings.json",
];

/// How often the loop's slow beat re-reads the records unasked, to catch
/// a `/login` typed in a session's own pane. Anything orion does itself —
/// a sign-in, an add — asks at once.
const POLL: Duration = Duration::from_secs(5);

// ---- who each account is signed in as ----

/// What an account's record says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// Signed in as this email.
    As(String),
    /// No account in the record: never signed in, or signed out since.
    Out,
}

/// The file a record was last read from, and its inode, size and mtime
/// then — the inode for a save that renames a fresh file into place.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    file: PathBuf,
    inode: u64,
    modified: Option<SystemTime>,
    len: u64,
}

/// One record's last reading. `stamp` None: there was no file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Known {
    stamp: Option<Stamp>,
    state: SignIn,
}

/// Process-wide in the app, where reads run on the blocking pool and
/// frames on the loop; per thread under test, so one test's accounts
/// never name another test's cards.
#[cfg(not(test))]
mod store {
    use super::*;
    use std::sync::Mutex;

    static KNOWN: Mutex<BTreeMap<PathBuf, Known>> = Mutex::new(BTreeMap::new());
    static SHORT: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    static TAGS: Mutex<BTreeMap<String, AccountTag>> = Mutex::new(BTreeMap::new());

    pub(super) fn known<T>(f: impl FnOnce(&mut BTreeMap<PathBuf, Known>) -> T) -> T {
        f(&mut KNOWN.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub(super) fn short<T>(f: impl FnOnce(&mut BTreeMap<String, String>) -> T) -> T {
        f(&mut SHORT.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub(super) fn tags<T>(f: impl FnOnce(&mut BTreeMap<String, AccountTag>) -> T) -> T {
        f(&mut TAGS.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

#[cfg(test)]
mod store {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static KNOWN: RefCell<BTreeMap<PathBuf, Known>> = const { RefCell::new(BTreeMap::new()) };
        static SHORT: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
        static TAGS: RefCell<BTreeMap<String, AccountTag>> = const { RefCell::new(BTreeMap::new()) };
    }

    pub(super) fn known<T>(f: impl FnOnce(&mut BTreeMap<PathBuf, Known>) -> T) -> T {
        KNOWN.with(|cell| f(&mut cell.borrow_mut()))
    }

    pub(super) fn short<T>(f: impl FnOnce(&mut BTreeMap<String, String>) -> T) -> T {
        SHORT.with(|cell| f(&mut cell.borrow_mut()))
    }

    pub(super) fn tags<T>(f: impl FnOnce(&mut BTreeMap<String, AccountTag>) -> T) -> T {
        TAGS.with(|cell| f(&mut cell.borrow_mut()))
    }
}

/// Who `record` was signed in as at the last read; None before the first
/// read has landed. Never touches the disk.
pub fn state_of(record: &Record) -> Option<SignIn> {
    store::known(|known| known.get(&record.file).map(|k| k.state.clone()))
}

/// Re-read every record in `records` whose file changed since the last
/// read — a stat each, and a parse of the ones that moved. True when what
/// one says changed. A record that can't be read just now (half written
/// by a Claude Code saving it) keeps what it said, and is read again on
/// the next pass.
pub fn refresh(records: &[Record]) -> bool {
    let mut changed = false;
    for record in records {
        let previous = store::known(|known| known.get(&record.file).cloned());
        let Some(now) = read(record, previous.as_ref()) else {
            continue;
        };
        if previous.as_ref() != Some(&now) {
            changed |= previous.map(|p| p.state).as_ref() != Some(&now.state);
            store::known(|known| known.insert(record.file.clone(), now));
        }
    }
    changed
}

/// One record's reading, or None to keep the last: it is unchanged, or
/// unreadable just now.
fn read(record: &Record, previous: Option<&Known>) -> Option<Known> {
    let file = record.current();
    let meta = match std::fs::metadata(file) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Some(Known {
                stamp: None,
                state: SignIn::Out,
            });
        }
        Err(_) => return None,
    };
    let stamp = Stamp {
        file: file.to_path_buf(),
        inode: std::os::unix::fs::MetadataExt::ino(&meta),
        modified: meta.modified().ok(),
        len: meta.len(),
    };
    if previous.is_some_and(|p| p.stamp.as_ref() == Some(&stamp)) {
        return None;
    }
    match orion_core::claude_account::read_email(file) {
        Ok(email) => Some(Known {
            stamp: Some(stamp),
            state: email.map_or(SignIn::Out, SignIn::As),
        }),
        // Not the error: serde's message can quote the file's values.
        Err(_) => {
            tracing::debug!(file = %file.display(), "Claude account record unreadable; read again next pass");
            None
        }
    }
}

/// Read every account the config names (off the loop in the app), and
/// what the cards call each one. True when anything an account shows
/// changed. Under test, only against a pinned config — never the
/// developer's own accounts.
pub fn refresh_now() -> bool {
    #[cfg(test)]
    if !crate::config::config_pinned() {
        return false;
    }
    let cfg = Config::load();
    let all = cfg.raw_harness_registry();
    let accounts: Vec<(String, Option<String>, Record)> = all
        .iter()
        .filter_map(|entry| Some((entry.id.clone(), name_of(entry), record_of(entry)?)))
        .collect();
    let records: Vec<Record> = accounts.iter().map(|(_, _, r)| r.clone()).collect();
    let mut changed = refresh(&records);
    // A name the user gave wins; else the email; else the id, as ever.
    let names: BTreeMap<String, String> = if accounts.len() > 1 {
        accounts
            .iter()
            .filter_map(|(id, name, record)| {
                let short = name.clone().or_else(|| match state_of(record)? {
                    SignIn::As(email) => Some(email),
                    SignIn::Out => None,
                })?;
                Some((id.clone(), short))
            })
            .collect()
    } else {
        BTreeMap::new()
    };
    store::short(|short| {
        if *short != names {
            *short = names;
            changed = true;
        }
    });
    let tags = if accounts.len() > 1 {
        let seeds: Vec<(String, String)> = accounts
            .iter()
            .map(|(id, name, record)| {
                let email = match state_of(record) {
                    Some(SignIn::As(email)) => Some(email),
                    _ => None,
                };
                (id.clone(), tag_seed(name.as_deref(), email.as_deref(), id))
            })
            .collect();
        account_tags(&seeds, &cfg.account_tags)
    } else {
        BTreeMap::new()
    };
    store::tags(|known| {
        if *known != tags {
            *known = tags;
            changed = true;
        }
    });
    changed
}

/// Ask for the records to be read again: on the blocking pool when the
/// loop is running, the answer landing through `app.accounts_tx`
/// ([`land_refresh`]); inline in a unit test, which has no loop. `force`
/// skips the slow beat's [`POLL`] spacing — for a sign-in that just
/// ended, an account just added. One read runs at a time
/// (`App::accounts_reads`): the beat skips while one runs, and a forced
/// read asked meanwhile is owed — it runs once the running one lands, so
/// it sees what changed after that one had read past it, and the two
/// never write the record store at once.
pub fn request_refresh(app: &mut App, force: bool) {
    if !force && app.accounts_polled.is_some_and(|at| at.elapsed() < POLL) {
        return;
    }
    let now = Instant::now();
    app.accounts_polled = Some(now);
    match app.accounts_tx.clone() {
        Some(tx) => {
            let ticket = if force {
                app.accounts_reads.begin_fresh((), now)
            } else {
                app.accounts_reads.begin((), now)
            };
            let Some(ticket) = ticket else {
                return;
            };
            tokio::task::spawn_blocking(move || {
                let changed = refresh_now();
                let _ = tx.send((ticket, changed));
            });
        }
        None => {
            if refresh_now() {
                app.dirty = true;
            }
        }
    }
}

/// A read off the loop finished: a changed name redraws, and a forced read
/// asked while it ran starts now.
pub fn land_refresh(app: &mut App, ticket: crate::fetch::Ticket<()>, changed: bool) {
    let Some(landed) = app.accounts_reads.land(&ticket) else {
        return;
    };
    if changed {
        app.dirty = true;
    }
    if landed.owed {
        request_refresh(app, true);
    }
}

// ---- where things are ----

/// Where Claude Code records who `entry` is signed in as, when it is a
/// Claude account: its pinned dir's record, or the default account's.
pub fn record_of(entry: &HarnessDescriptor) -> Option<Record> {
    if !entry.is_claude_account() {
        return None;
    }
    match entry.pinned_claude_config_dir() {
        Some(dir) => Record::of(Some(&dir)),
        None => Places::of_this_machine().claude_record,
    }
}

/// The config dir `entry` runs in, when it is a Claude account.
pub fn dir_of(entry: &HarnessDescriptor) -> Option<PathBuf> {
    if !entry.is_claude_account() {
        return None;
    }
    entry
        .pinned_claude_config_dir()
        .or_else(|| Places::of_this_machine().claude)
}

/// `path` as a person writes it: `~/.claude-2` under the home dir.
pub fn tilde(path: &Path) -> String {
    crate::paths_util::tilde(path, Places::of_this_machine().home.as_deref())
}

// ---- the names an account goes by ----

/// What an account is called while it has no name of its own: built-in
/// Claude's label, before the email it is signed in as.
pub const DEFAULT_NAME: &str = "Claude";

/// The name `entry` goes by when the user gave it one: its `harnesses`
/// label. None while it has none — `Claude` wherever it is listed. Read
/// off a raw registry row ([`Config::raw_harness_registry`]), not one
/// [`label`] already named.
pub fn name_of(entry: &HarnessDescriptor) -> Option<String> {
    let label = entry.label.trim();
    (!label.is_empty() && label != DEFAULT_NAME).then(|| label.to_string())
}

/// The name a Claude account goes by wherever it is listed: its label —
/// its name, `Claude` when it has none of its own — with who it is
/// signed in as, `Work (a@b.co)` or `Work (not signed in)`; the label
/// alone until the first read lands. A name that already holds the email
/// — typed as the label it was shown — is not given it twice.
pub fn label(base: &str, record: &Record) -> String {
    let base = match base.trim() {
        "" => DEFAULT_NAME,
        base => base,
    };
    match state_of(record) {
        Some(SignIn::As(email)) if base.to_lowercase().contains(&email.to_lowercase()) => {
            base.to_string()
        }
        Some(SignIn::As(email)) => format!("{base} ({email})"),
        Some(SignIn::Out) => format!("{base} (not signed in)"),
        None => base.to_string(),
    }
}

/// What a card, a list row, the quick prompt and a preset call a session's
/// harness when it is a Claude account on a machine with more than one:
/// its name when it has one, else the email it is signed in as. None
/// everywhere else — one account, an unnamed signed-out one, any other
/// harness — and the caller says the harness id as it always has. Read
/// off the last refresh, so a grid never opens the config to ask; a
/// rename asks for one at once.
pub fn short_name(kind: AgentKind, custom: Option<&str>) -> Option<String> {
    let id = orion_core::harness::registry_id(kind, custom);
    store::short(|short| short.get(id).cloned())
}

// ---- the ACCOUNT TAG: two letters a row wears for its account ----

/// The two letters a session row wears after its status dot to say which
/// Claude account it runs on, and the tint they are drawn in, on a
/// machine with more than one account. Fixed width, so a column of them
/// lines up and the model and effort beside them never move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountTag {
    pub letters: String,
    /// An index into [`crate::theme::ACCOUNT_TINTS`].
    pub tint: usize,
}

impl AccountTag {
    pub fn color(&self) -> ratatui::style::Color {
        let tints = crate::theme::ACCOUNT_TINTS;
        tints[self.tint % tints.len()]
    }
}

/// How many letters an [`AccountTag`] has.
pub const TAG_LEN: usize = 2;

/// What one account's tag is made from: the letters of its name, else of
/// its email's local part, else of its id — lowercase, letters and digits
/// only. `Work Laptop` → `wl…` (each word's first letter first), `Ops` →
/// `ops`, `alex.kidd@x.io` → `alexkidd`.
fn tag_seed(name: Option<&str>, email: Option<&str>, id: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    if let Some(name) = name {
        let words: Vec<String> = name
            .split_whitespace()
            .map(clean)
            .filter(|w| !w.is_empty())
            .collect();
        if words.len() > 1 {
            let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
            return initials + &words.concat();
        }
        if let Some(word) = words.into_iter().next() {
            return word;
        }
    }
    if let Some(local) = email.and_then(|e| e.split('@').next()) {
        let local = clean(local);
        if !local.is_empty() {
            return local;
        }
    }
    clean(id)
}

/// Every account's tag, from the seeds in registry order (the default
/// account first, so it keeps the first tint): the seed's first two
/// letters, or — when an account earlier in the list already has those —
/// its first letter with a later one, then with a digit. An override the
/// user set in `account_tags` wins, as written.
fn account_tags(
    seeds: &[(String, String)],
    overrides: &BTreeMap<String, crate::config::AccountTagOverride>,
) -> BTreeMap<String, AccountTag> {
    let mut taken: Vec<String> = overrides
        .values()
        .filter_map(|o| o.tag.as_deref().map(fit_tag))
        .collect();
    let mut out = BTreeMap::new();
    for (index, (id, seed)) in seeds.iter().enumerate() {
        let custom = overrides.get(id);
        let letters = match custom.and_then(|o| o.tag.as_deref()) {
            Some(tag) => fit_tag(tag),
            None => {
                let picked = pick_letters(seed, &taken);
                taken.push(picked.clone());
                picked
            }
        };
        let tint = custom.and_then(|o| o.tint).unwrap_or(index);
        out.insert(id.clone(), AccountTag { letters, tint });
    }
    out
}

/// A tag the user typed, held to [`TAG_LEN`] characters and padded out
/// to them, so the column stays put.
fn fit_tag(tag: &str) -> String {
    let mut letters: String = tag.trim().chars().take(TAG_LEN).collect();
    while letters.chars().count() < TAG_LEN {
        letters.push(' ');
    }
    letters
}

/// The first pair of letters from `seed` that no tag in `taken` has.
fn pick_letters(seed: &str, taken: &[String]) -> String {
    let chars: Vec<char> = seed.chars().collect();
    let Some(&first) = chars.first() else {
        return fit_tag("?");
    };
    let candidates = chars
        .iter()
        .skip(1)
        .map(|&second| format!("{first}{second}"))
        .chain((2..=9).map(|n| format!("{first}{n}")));
    candidates
        .into_iter()
        .find(|c| !taken.contains(c))
        .unwrap_or_else(|| fit_tag(&first.to_string()))
}

/// The tag for a session's harness: an [`AccountTag`] when it runs on a
/// Claude account on a machine with more than one, None everywhere else
/// — one account, any other harness — and the row keeps its width.
pub fn account_tag(kind: AgentKind, custom: Option<&str>) -> Option<AccountTag> {
    tag_of(orion_core::harness::registry_id(kind, custom))
}

/// [`account_tag`] by the account's registry id.
pub fn tag_of(id: &str) -> Option<AccountTag> {
    store::tags(|tags| tags.get(id.trim()).cloned())
}

/// Whether any account has a tag: a machine with more than one.
pub fn any_tags() -> bool {
    store::tags(|tags| !tags.is_empty())
}

/// The sessions that run on extra account `id`, archived ones too: what
/// a removal strands. Each stops resuming until the account is back, and
/// its transcript lives in the account's dir, so a Trash would take it.
pub fn sessions_on<'a>(agents: &'a [orion_core::Agent], id: &str) -> Vec<&'a orion_core::Agent> {
    agents
        .iter()
        .filter(|a| {
            a.custom_harness.is_some()
                && orion_core::harness::registry_id(a.kind, a.custom_harness.as_deref()) == id
        })
        .collect()
}

/// What the remove dialog says of the sessions [`sessions_on`] finds:
/// how many, the first few by name, and that **Continue on** (`how`)
/// carries each to another account first — to `same`, one signed in as
/// the same email, when there is one: same login, nothing lost. Empty
/// when nothing runs on it.
pub fn stranded_note(names: &[String], how: &str, same: Option<&str>) -> String {
    const SHOWN: usize = 3;
    if names.is_empty() {
        return String::new();
    }
    let mut listed: Vec<String> = names.iter().take(SHOWN).cloned().collect();
    if names.len() > SHOWN {
        listed.push(format!("{} more", names.len() - SHOWN));
    }
    let count = match names.len() {
        1 => "1 session runs".to_string(),
        n => format!("{n} sessions run"),
    };
    let to = same.map_or_else(
        || "another account".to_string(),
        |label| format!("{label} — the same login"),
    );
    format!(
        "{count} on it — {} — and stop resuming once it goes.\n{how} moves each to {to} first.",
        and_list(&listed)
    )
}

/// The email `entry` is signed in as, when it is a Claude account and the
/// last read says so.
pub fn email_of(entry: &HarnessDescriptor) -> Option<String> {
    match state_of(&record_of(entry)?)? {
        SignIn::As(email) => Some(email),
        SignIn::Out => None,
    }
}

/// Accounts signed in as one email — the trap a browser sign-in sets,
/// since claude.ai approves whichever account the browser is signed in to:
/// each email two or more accounts share, with those accounts' dirs, in
/// `entries` order.
pub fn same_account_groups(entries: &[HarnessDescriptor]) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for entry in entries {
        let (Some(email), Some(dir)) = (email_of(entry), dir_of(entry)) else {
            continue;
        };
        let dir = tilde(&dir);
        match groups
            .iter_mut()
            .find(|(seen, _)| seen.eq_ignore_ascii_case(&email))
        {
            Some((_, dirs)) => dirs.push(dir),
            None => groups.push((email, vec![dir])),
        }
    }
    groups.retain(|(_, dirs)| dirs.len() > 1);
    groups
}

/// The same-account warning, worded for where the accounts are listed —
/// the settings overlay, the wizard — and short enough a line for both:
/// which dirs share an email, what that costs, and how to fix it.
pub fn same_account_warning(email: &str, dirs: &[String]) -> Vec<String> {
    let fix = [
        "  it in again, in a private browser window: sign in there as",
        "  the other account, or type its email (claude auth login --email).",
    ];
    let mut lines = vec![
        format!("⚠ {} are signed in as one account,", and_list(dirs)),
        format!("  {email}: one subscription, one limit. Enter on one signs"),
    ];
    lines.extend(fix.map(String::from));
    lines
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

// ---- signing in and out ----

/// One `claude auth …` run for an account, as the modal spawns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCommand {
    pub program: String,
    pub args: Vec<String>,
    /// The account's own environment — its `CLAUDE_CONFIG_DIR` — on top
    /// of orion's.
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// The modal's title.
    pub title: String,
}

/// `claude auth login` for `entry`, in its config dir: Claude Code's own
/// browser sign-in, with `--email` filling the login page when one is
/// given, and on a Mac that page opening in a private window
/// ([`private_browser`]). The CLI is the account's own program, so a
/// `harnesses` override of Claude's reaches the sign-in too.
pub fn login_command(entry: &HarnessDescriptor, email: Option<&str>) -> AuthCommand {
    let mut args = vec!["auth".to_string(), "login".to_string()];
    if let Some(email) = email.map(str::trim).filter(|e| !e.is_empty()) {
        args.extend(["--email".to_string(), email.to_string()]);
    }
    let mut command = auth_command(entry, args, "Sign in");
    if let Some(browser) = private_browser() {
        command
            .env
            .push(("BROWSER".into(), browser.display().to_string()));
    }
    command
}

/// `claude auth logout` for `entry`, in its config dir.
pub fn logout_command(entry: &HarnessDescriptor) -> AuthCommand {
    auth_command(
        entry,
        vec!["auth".to_string(), "logout".to_string()],
        "Sign out",
    )
}

fn auth_command(entry: &HarnessDescriptor, args: Vec<String>, verb: &str) -> AuthCommand {
    let places = Places::of_this_machine();
    let dir = dir_of(entry);
    let cwd = dir
        .clone()
        .filter(|dir| dir.is_dir())
        .or(places.home)
        .unwrap_or_else(|| PathBuf::from("/"));
    let shown = dir.map_or_else(|| entry.display_label().to_string(), |d| tilde(&d));
    AuthCommand {
        program: entry.program.trim().to_string(),
        args,
        env: entry.launch_env(),
        cwd,
        title: format!("{verb} · {shown}"),
    }
}

/// Browsers that open a private window from the command line, by bundle
/// id, with the flag that does it. Safari has no such flag.
const PRIVATE_BROWSERS: &[(&str, &str)] = &[
    ("com.google.Chrome", "--incognito"),
    ("org.mozilla.firefox", "--private-window"),
    ("com.brave.Browser", "--incognito"),
    ("com.microsoft.edgemac", "--inprivate"),
    ("com.vivaldi.Vivaldi", "--incognito"),
    ("org.chromium.Chromium", "--incognito"),
    ("com.operasoftware.Opera", "--private"),
];

/// The program a sign-in names as Claude Code's `BROWSER`:
/// [`private_browser_script`], written to orion's data dir. None when it
/// can't be written — Claude then opens its page the way it always does.
fn private_browser() -> Option<PathBuf> {
    let dir = if cfg!(test) {
        std::env::temp_dir().join("orion-unit-tests-browser")
    } else {
        orion_core::paths::data_dir()
    };
    write_private_browser(&dir).ok()
}

/// Write [`private_browser_script`] into `dir` — only when it has changed
/// — executable, and say where.
fn write_private_browser(dir: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("claude-sign-in-browser");
    let script = private_browser_script();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(script.as_str()) {
        std::fs::create_dir_all(dir)?;
        std::fs::write(&path, &script)?;
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

/// The browser `claude auth login` opens its page with: a script that
/// opens it in a private window, where claude.ai asks who is signing in
/// rather than approving whichever account the browser is already signed
/// in to — the trap [`same_account_groups`] flags. The page's localhost
/// redirect still finishes the sign-in, so there is no code to paste. The
/// default browser goes first when it has a private mode, then
/// [`PRIVATE_BROWSERS`] in order; with none of them installed the page
/// opens as it always did.
fn private_browser_script() -> String {
    let mut script = String::from(
        r#"#!/bin/sh
# Written by orion: the browser `claude auth login` opens (BROWSER). It opens
# Claude's sign-in page in a private window, so claude.ai asks which account
# is signing in rather than approving the one this browser is signed in to.
url=$1
private() { open -nb "$1" --args "$2" "$url" 2>/dev/null; }
default=$(osascript -l JavaScript -e 'ObjC.import("AppKit");
var app = $.NSWorkspace.sharedWorkspace.URLForApplicationToOpenURL($.NSURL.URLWithString("https://claude.ai"));
app.isNil() ? "" : $.NSBundle.bundleWithURL(app).bundleIdentifier.js' 2>/dev/null | tr '[:upper:]' '[:lower:]')
case $default in
"#,
    );
    for (id, flag) in PRIVATE_BROWSERS {
        let lower = id.to_ascii_lowercase();
        script.push_str(&format!("{lower}) private {id} {flag} && exit 0 ;;\n"));
    }
    script.push_str("esac\n");
    for (id, flag) in PRIVATE_BROWSERS {
        script.push_str(&format!("private {id} {flag} && exit 0\n"));
    }
    script.push_str("exec open \"$url\"\n");
    script
}

/// Run `command` in the editor modal, over whatever overlay is up: the
/// user answers the CLI there (a pasted code, a browser hand-off), and its
/// exit closes the modal and re-reads the accounts. False, with the
/// footer saying why, when nothing was spawned. Under test nothing ever
/// is — the command is recorded for the test to read instead, so a run
/// never opens anyone's browser.
pub fn run(app: &mut App, command: AuthCommand) -> bool {
    #[cfg(test)]
    {
        let _ = &app;
        RAN.with(|ran| ran.borrow_mut().push(command));
        true
    }
    #[cfg(not(test))]
    {
        let Some(tx) = app.editor_tx.clone() else {
            return false;
        };
        let (cols, rows) = crate::event_loop::editor_size_guess(app);
        app.editor_generation += 1;
        match crate::editor_term::EditorTerm::spawn_cmd_env(
            &command.program,
            &command.args,
            &command.env,
            &command.cwd,
            command.title,
            cols,
            rows,
            app.editor_generation,
            tx,
        ) {
            Ok(mut term) => {
                term.account_auth = true;
                app.editor = Some(term);
                app.dirty = true;
                true
            }
            Err(msg) => {
                app.flash = Some(crate::flash::Flash::failed(msg));
                false
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static RAN: std::cell::RefCell<Vec<AuthCommand>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The `claude auth` runs [`run`] was asked for on this test's thread,
/// taken.
#[cfg(test)]
pub fn take_ran() -> Vec<AuthCommand> {
    RAN.with(|ran| std::mem::take(&mut *ran.borrow_mut()))
}

/// The modal running `claude auth` closed: read the accounts again now,
/// so the row it was opened from says who it is signed in as.
pub fn auth_closed(app: &mut App) {
    request_refresh(app, true);
}

/// Sign account `id` in: `claude auth login` in the modal, `email`
/// filling Claude's login page when one is given. False when nothing was
/// started — no such account, or a spawn that failed (which flashes why).
pub fn sign_in(app: &mut App, id: &str, email: Option<&str>) -> bool {
    account(id).is_some_and(|entry| run(app, login_command(&entry, email)))
}

/// Sign account `id` out: `claude auth logout` in the modal.
pub fn sign_out(app: &mut App, id: &str) -> bool {
    account(id).is_some_and(|entry| run(app, logout_command(&entry)))
}

/// The registry row of Claude account `id`, named as it is listed.
fn account(id: &str) -> Option<HarnessDescriptor> {
    Config::load()
        .harness_registry()
        .into_iter()
        .find(|entry| entry.id == id && entry.is_claude_account())
}

/// The [`SHARED_SETUP`] entries the default account has to share — what
/// an add asks about, and none means there is nothing to ask.
pub fn shareable(cfg: &Config) -> Vec<&'static str> {
    let Some(from) = default_dir(cfg) else {
        return Vec::new();
    };
    SHARED_SETUP
        .iter()
        .copied()
        .filter(|name| std::fs::symlink_metadata(from.join(name)).is_ok())
        .collect()
}

// ---- adding and removing ----

/// A new account, before anything is created: its id, its config dir as
/// the entry stores it, that dir resolved, and the name it goes by —
/// empty for none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccount {
    pub id: String,
    pub config_dir: String,
    pub dir: PathBuf,
    pub name: String,
}

/// The account a short name makes: `Work` → `claude-work` in
/// `~/.claude-work`, going by `Work` as typed (a dir already there is
/// adopted, its login and all — the add says so); nothing typed → the
/// first `claude-2`, `claude-3`, … whose id is free and whose dir is not
/// there yet, with no name. `Err` says why the name can't be used.
pub fn plan_new(cfg: &Config, name: &str) -> Result<NewAccount, String> {
    let Some(home) = Places::of_this_machine().home else {
        return Err("no home directory to put a new account in".into());
    };
    let taken = |id: &str| id_taken(cfg, id);
    let make = |id: String| {
        let dir = home.join(format!(".{id}"));
        NewAccount {
            config_dir: stored_dir(&home, &dir),
            id,
            dir,
            name: name.trim().to_string(),
        }
    };
    let slug = slug(name);
    if slug.is_empty() {
        if !name.trim().is_empty() {
            return Err("name it with letters or digits".into());
        }
        let free = (2..)
            .map(|n| format!("claude-{n}"))
            .find(|id| !taken(id) && !home.join(format!(".{id}")).exists())
            .expect("an unbounded range always finds a free name");
        return Ok(make(free));
    }
    let id = match slug.strip_prefix("claude") {
        Some("") => return Err("`claude` is the default account — name the new one".into()),
        Some(rest) if rest.starts_with('-') => slug.clone(),
        _ => format!("claude-{slug}"),
    };
    let all = cfg.raw_harness_registry();
    if cfg.harnesses.contains_key(&id) && !all.iter().any(|e| e.id == id && e.is_claude_account()) {
        return Err(format!(
            "config.json's harnesses already defines `{id}` — pick another name, or replace \
             that entry with a claude_accounts one (docs/configuration.md)"
        ));
    }
    if taken(&id) {
        return Err(format!("`{id}` is already a harness — pick another name"));
    }
    Ok(make(id))
}

/// Whether `id` is spoken for: a registry row, a `claude_accounts` entry
/// (a broken one too) or a `harnesses` key.
fn id_taken(cfg: &Config, id: &str) -> bool {
    cfg.raw_harness_registry()
        .iter()
        .any(|entry| entry.id == id)
        || cfg.claude_accounts.iter().any(|a| a.id.trim() == id)
        || cfg.harnesses.contains_key(id)
}

/// `dir` as a `claude_accounts` entry stores it: `~/.claude-work` under
/// `$HOME`, which keeps the entry true on another machine; whole
/// anywhere else — a test's home is not `$HOME`.
fn stored_dir(home: &Path, dir: &Path) -> String {
    match dir.strip_prefix(home) {
        Ok(rest) if orion_core::env::home_dir().as_deref() == Some(home) => {
            format!("~/{}", rest.display())
        }
        _ => dir.display().to_string(),
    }
}

/// Whether some account runs in `dir` ([`account_dirs`]).
fn dir_in_use(cfg: &Config, dir: &Path) -> bool {
    account_dirs(cfg).iter().any(|have| have == dir)
}

/// Every dir an account runs in: the default one's (its own, or the one
/// its `env` pins) and each one a registry row pins.
fn account_dirs(cfg: &Config) -> Vec<PathBuf> {
    default_dir(cfg)
        .into_iter()
        .chain(
            cfg.raw_harness_registry()
                .iter()
                .filter_map(HarnessDescriptor::pinned_claude_config_dir),
        )
        .collect()
}

/// `name` as an id's tail: lowercase letters and digits, anything else a
/// single hyphen, none at the ends.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Create `new`'s config dir (kept as it is when it is already there),
/// share the default account's setup into it when `share`, and add it to
/// config.json under its name, offered from now on. The answer is the
/// line the footer shows — which says so when the dir was already on
/// this machine, and who its login is, since that login came with it.
pub fn add(new: &NewAccount, share: bool) -> Result<String, String> {
    use std::os::unix::fs::DirBuilderExt;
    let mut cfg = Config::load();
    if cfg.claude_accounts.iter().any(|a| a.id.trim() == new.id) {
        return Err(format!("`{}` is already an account", new.id));
    }
    let existed = new.dir.is_dir();
    // Its login lives here (the credentials, on Linux): the user's alone.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&new.dir)
        .map_err(|e| format!("couldn't create {}: {e}", tilde(&new.dir)))?;
    let from = default_dir(&cfg).filter(|from| from != &new.dir);
    let shared = match (&from, share) {
        (Some(from), true) => share_setup(from, &new.dir)
            .map_err(|e| format!("couldn't link the setup into {}: {e}", tilde(&new.dir)))?,
        _ => Vec::new(),
    };
    cfg.claude_accounts.push(ClaudeAccount {
        id: new.id.clone(),
        config_dir: new.config_dir.clone(),
        name: String::new(),
        enabled: true,
    });
    set_name(&mut cfg, &new.id, &new.name);
    cfg.save()
        .map_err(|e| format!("couldn't save settings: {e}"))?;
    let shown = match new.name.trim() {
        "" => new.id.clone(),
        name => format!("{name} ({})", new.id),
    };
    let mut note = format!("added {shown} in {}", tilde(&new.dir));
    if let (Some(from), false) = (&from, shared.is_empty()) {
        note.push_str(&format!(
            ", sharing {} from {}",
            shared.join(", "),
            tilde(from)
        ));
    }
    // The last read of the dir, from when an account ran in it, says who
    // it is; never a read here.
    let login = Record::of(Some(&new.dir)).and_then(|record| state_of(&record));
    note.push_str(&match (existed, login) {
        (true, Some(SignIn::As(email))) => format!(
            " — it was already on this machine, signed in as {email}: Enter on it to sign in \
             as someone else"
        ),
        (true, _) => " — it was already on this machine, its login and history with it".into(),
        (false, _) => " — Enter on it to sign it in".into(),
    });
    Ok(note)
}

/// What account `id`'s name prompt starts with: the name it goes by now
/// ([`name_of`]), empty while it has none.
pub fn current_name(cfg: &Config, id: &str) -> String {
    cfg.raw_harness_registry()
        .iter()
        .find(|entry| entry.id == id && entry.is_claude_account())
        .and_then(name_of)
        .unwrap_or_default()
}

/// Give account `id` the name `name` — empty takes its name away, back to
/// `Claude (a@b.co)`. Only the name moves: the id its sessions point back
/// at, and the dir its login lives in, stay as they are. Every account
/// keeps it in one place, its `harnesses.<id>.label`, beside its model and
/// effort. The answer is the line the footer shows.
pub fn rename(id: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    let mut cfg = Config::load();
    let is_default = AgentKind::parse(id) == Some(AgentKind::Claude);
    if !cfg
        .raw_harness_registry()
        .iter()
        .any(|entry| entry.id == id && entry.is_claude_account())
    {
        return Err(format!("`{id}` is no Claude account to rename"));
    }
    set_name(&mut cfg, id, name);
    cfg.save()
        .map_err(|e| format!("couldn't save settings: {e}"))?;
    Ok(match name {
        "" if is_default => "the default account goes by Claude again".to_string(),
        "" => format!("{id} goes by Claude and its email again"),
        name => format!("{id} goes by {name} now"),
    })
}

/// Account `id`'s name as its `harnesses` label; empty takes it away, and
/// an entry that said nothing but the name goes with it.
fn set_name(cfg: &mut Config, id: &str, name: &str) {
    let name = name.trim();
    let over = cfg.harnesses.entry(id.to_string()).or_default();
    over.label = (!name.is_empty()).then(|| name.to_string());
    if *over == orion_core::harness::HarnessOverride::default() {
        cfg.harnesses.remove(id);
    }
}

/// Who `dir` is signed in as, at the last read: a dir an account ran in
/// before, about to be adopted by an add.
pub fn dir_state(dir: &Path) -> Option<SignIn> {
    state_of(&Record::of(Some(dir))?)
}

/// The default account's config dir: the one built-in Claude's `env`
/// pins, else Claude Code's own default.
pub fn default_dir(cfg: &Config) -> Option<PathBuf> {
    cfg.raw_harness_registry()
        .iter()
        .find(|entry| entry.id == AgentKind::Claude.as_str())
        .and_then(|claude| claude.pinned_claude_config_dir())
        .or_else(|| Places::of_this_machine().claude)
}

/// Link every [`SHARED_SETUP`] entry `from` holds into `to`, where `to`
/// does not hold it yet: a symlink to the original, so an edit in either
/// account is an edit in both. Nothing already in `to` is replaced, and
/// nothing outside the list is touched. The names linked, in list order.
pub fn share_setup(from: &Path, to: &Path) -> std::io::Result<Vec<&'static str>> {
    let mut linked = Vec::new();
    for name in SHARED_SETUP {
        let source = from.join(name);
        let link = to.join(name);
        if std::fs::symlink_metadata(&source).is_err() || std::fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        std::os::unix::fs::symlink(&source, &link)?;
        linked.push(*name);
    }
    Ok(linked)
}

/// Take the `claude_accounts` entry `id` out of config.json — with its
/// `harnesses` deltas, and the ⌘N default when it named it — and, when
/// `trash` says so, move its config dir to the Trash. A dir another
/// account (the default one included) also runs in is never moved. The
/// answer is the line the footer shows.
pub fn remove(id: &str, trash: bool) -> Result<String, String> {
    let mut cfg = Config::load();
    let Some(at) = cfg.claude_accounts.iter().position(|a| a.id.trim() == id) else {
        return Err(format!(
            "`{id}` is not in claude_accounts — edit config.json to change it"
        ));
    };
    let account = cfg.claude_accounts.remove(at);
    cfg.harnesses.remove(id);
    if cfg.quick_prompt_kind.trim() == id {
        cfg.quick_prompt_kind = AgentKind::Claude.as_str().into();
    }
    let dir = account.dir();
    let shown = tilde(&dir);
    let shared = dir_in_use(&cfg, &dir);
    cfg.save()
        .map_err(|e| format!("couldn't save settings: {e}"))?;
    if !trash || !dir.exists() {
        return Ok(format!("removed {id} — {shown} stays on disk"));
    }
    if shared {
        return Ok(format!(
            "removed {id} — {shown} stays: another account runs in it"
        ));
    }
    Ok(match Places::of_this_machine().trash {
        Some(bin) => match crate::paths_util::move_folder_to_trash(&dir, &bin) {
            Ok(_) => format!("removed {id} and moved {shown} to the Trash"),
            Err(e) => format!("removed {id}, but couldn't move {shown} to the Trash: {e}"),
        },
        None => format!("removed {id} — no Trash here, so {shown} stays on disk"),
    })
}

#[cfg(test)]
mod tests {
    use super::{account_tags, tag_seed, AccountTag};
    use crate::config::AccountTagOverride;
    use std::collections::BTreeMap as Map;

    fn tags_of(seeds: &[(&str, &str)], overrides: &Map<String, AccountTagOverride>) -> Vec<String> {
        let seeds: Vec<(String, String)> = seeds
            .iter()
            .map(|(id, seed)| (id.to_string(), seed.to_string()))
            .collect();
        let tags = account_tags(&seeds, overrides);
        seeds
            .iter()
            .map(|(id, _)| tags[id].letters.clone())
            .collect()
    }

    #[test]
    fn tag_seeds_come_from_the_name_then_the_email_then_the_id() {
        assert!(tag_seed(Some("Work Laptop"), Some("a@b.co"), "claude-2").starts_with("wl"));
        assert_eq!(tag_seed(Some("Ops"), None, "claude-2"), "ops");
        assert_eq!(tag_seed(None, Some("Rob.Kidd@x.io"), "claude-2"), "robkidd");
        assert_eq!(tag_seed(None, None, "claude-2"), "claude2");
    }

    #[test]
    fn tags_never_collide_and_keep_two_columns() {
        let tags = tags_of(
            &[
                ("claude", "oliver"),
                ("claude-2", "olive"),
                ("claude-3", "o"),
            ],
            &Map::new(),
        );
        assert_eq!(tags, ["ol", "oi", "o2"]);
        assert!(tags.iter().all(|t| t.chars().count() == super::TAG_LEN));
    }

    #[test]
    fn an_override_wins_and_is_never_taken_by_another() {
        let mut overrides = Map::new();
        overrides.insert(
            "claude-2".to_string(),
            AccountTagOverride {
                tag: Some("ol".into()),
                tint: Some(4),
            },
        );
        let seeds = [("claude", "oliver"), ("claude-2", "work")];
        assert_eq!(tags_of(&seeds, &overrides), ["oi", "ol"]);
        let seeds: Vec<(String, String)> = seeds
            .iter()
            .map(|(id, seed)| (id.to_string(), seed.to_string()))
            .collect();
        let tags = account_tags(&seeds, &overrides);
        assert_eq!(
            tags["claude-2"],
            AccountTag {
                letters: "ol".into(),
                tint: 4
            }
        );
        assert_eq!(tags["claude"].tint, 0, "registry order picks the rest");
    }

    use super::*;

    /// A temp home holding the default account's dir, its record in
    /// `~/.claude.json` as Claude Code keeps it, and a pinned config.
    struct Machine {
        home: tempfile::TempDir,
        config: PathBuf,
    }

    impl Machine {
        fn new(config: &str) -> Self {
            let home = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(home.path().join(".claude")).unwrap();
            let config_path = home.path().join("config.json");
            std::fs::write(&config_path, config).unwrap();
            Self {
                home,
                config: config_path,
            }
        }

        fn run<T>(&self, f: impl FnOnce() -> T) -> T {
            let places = Places::in_home(self.home.path());
            crate::paths_util::with_places(places, || {
                crate::config::with_config_path(self.config.clone(), f)
            })
        }

        fn sign(&self, file: &str, email: Option<&str>) {
            let path = self.home.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let body = match email {
                Some(email) => serde_json::json!({
                    "numStartups": 1,
                    "oauthAccount": {"emailAddress": email, "accountUuid": "u"}
                }),
                None => serde_json::json!({"numStartups": 1}),
            };
            std::fs::write(path, body.to_string()).unwrap();
        }
    }

    fn label_of(cfg: &Config, id: &str) -> String {
        cfg.effective_harness_by_id(id).display_label().to_string()
    }

    /// Every account is named after the email its own record holds — the
    /// default one's in `~/.claude.json`, an extra one's inside its dir —
    /// and only once a read has landed.
    #[test]
    fn accounts_are_named_after_their_records() {
        let m = Machine::new("{}");
        let two = m.home.path().join(".claude-2");
        std::fs::write(
            &m.config,
            serde_json::json!({"claude_accounts": [
                {"id": "claude-2", "config_dir": two.display().to_string()}
            ]})
            .to_string(),
        )
        .unwrap();
        m.run(|| {
            assert_eq!(
                label_of(&Config::load(), "claude"),
                "Claude",
                "nothing read yet"
            );
            m.sign(".claude.json", Some("a@b.co"));
            std::fs::create_dir_all(&two).unwrap();
            assert!(refresh_now());
            let cfg = Config::load();
            assert_eq!(label_of(&cfg, "claude"), "Claude (a@b.co)");
            assert_eq!(label_of(&cfg, "claude-2"), "Claude (not signed in)");
            assert_eq!(
                short_name(AgentKind::Claude, None).as_deref(),
                Some("a@b.co")
            );
            assert_eq!(short_name(AgentKind::Claude, Some("claude-2")), None);
            m.sign(".claude-2/.claude.json", Some("c@d.co"));
            assert!(refresh_now());
            assert!(!refresh_now(), "nothing moved since");
            let cfg = Config::load();
            assert_eq!(label_of(&cfg, "claude-2"), "Claude (c@d.co)");
            assert_eq!(
                short_name(AgentKind::Claude, Some("claude-2")).as_deref(),
                Some("c@d.co")
            );
            assert!(same_account_groups(&cfg.harness_registry()).is_empty());
        });
    }

    /// One account alone is `claude` on a card, as it always was: the
    /// email would only repeat what every card says.
    #[test]
    fn a_lone_account_keeps_its_short_name() {
        let m = Machine::new("{}");
        m.sign(".claude.json", Some("a@b.co"));
        m.run(|| {
            refresh_now();
            assert_eq!(label_of(&Config::load(), "claude"), "Claude (a@b.co)");
            assert_eq!(short_name(AgentKind::Claude, None), None);
        });
    }

    /// A name typed as the label it was shown — email and all — is not
    /// given the email a second time; any other name is.
    #[test]
    fn a_name_holding_the_email_is_not_doubled() {
        let m = Machine::new("{}");
        m.sign(".claude.json", Some("a@b.co"));
        m.run(|| {
            refresh_now();
            let record = record_of(&Config::load().raw_harness_registry()[0]).unwrap();
            assert_eq!(label("Claude (A@b.co)", &record), "Claude (A@b.co)");
            assert_eq!(label("Personal", &record), "Personal (a@b.co)");
        });
    }

    /// A `harnesses` entry is no account, whatever its `env` pins and
    /// whatever an older release let it say about hooks: it runs the
    /// program its id names, with no email to its name.
    #[test]
    fn a_harnesses_entry_is_no_account() {
        let m = Machine::new("{}");
        let b = m.home.path().join(".claude-b");
        std::fs::write(
            &m.config,
            serde_json::json!({"harnesses": {
                "claude-b": {"label": "Claude B", "program": "claude", "hooks": "claude",
                             "resume_flag": "--resume",
                             "env": {"CLAUDE_CONFIG_DIR": b.display().to_string()}}
            }})
            .to_string(),
        )
        .unwrap();
        m.sign(".claude-b/.claude.json", Some("b@b.co"));
        m.run(|| {
            refresh_now();
            let cfg = Config::load();
            assert_eq!(label_of(&cfg, "claude-b"), "Claude B");
            let entry = cfg.effective_harness_by_id("claude-b");
            assert!(!entry.is_claude_account());
            assert_eq!(entry.program, "claude-b");
            assert!(!cfg
                .account_rows()
                .contains(&crate::config::AccountRow::Account("claude-b".into())));
            assert!(rename("claude-b", "X").is_err(), "no Claude account");
        });
    }

    /// Two dirs signed in as one email are flagged with how to fix it.
    #[test]
    fn the_same_email_twice_is_flagged() {
        let m = Machine::new("{}");
        let two = m.home.path().join(".claude-2");
        std::fs::write(
            &m.config,
            serde_json::json!({"claude_accounts": [
                {"id": "claude-2", "config_dir": two.display().to_string()}
            ]})
            .to_string(),
        )
        .unwrap();
        m.sign(".claude.json", Some("a@b.co"));
        m.sign(".claude-2/.claude.json", Some("A@b.co"));
        m.run(|| {
            refresh_now();
            let groups = same_account_groups(&Config::load().harness_registry());
            assert_eq!(
                groups,
                [(
                    "a@b.co".to_string(),
                    vec!["~/.claude".to_string(), "~/.claude-2".to_string()]
                )]
            );
            let warning = same_account_warning(&groups[0].0, &groups[0].1).join("\n");
            assert!(warning.contains("~/.claude and ~/.claude-2"), "{warning}");
            assert!(warning.contains("private browser window"), "{warning}");
            assert!(warning.contains("--email"), "{warning}");
        });
    }

    /// The sign-in is Claude Code's own, in the account's dir, its CLI the
    /// account's program; `--email` only when one was typed.
    #[test]
    fn sign_in_runs_claude_auth_in_the_accounts_dir() {
        let m = Machine::new("{}");
        let two = m.home.path().join(".claude-2");
        std::fs::create_dir_all(&two).unwrap();
        let mut entry = orion_core::harness::builtin("claude").unwrap();
        entry.id = "claude-2".into();
        entry
            .env
            .insert("CLAUDE_CONFIG_DIR".into(), two.display().to_string());
        // The sign-in's browser is orion's private-window opener.
        let browser = |env: &[(String, String)]| {
            env.iter()
                .find(|(name, _)| name == "BROWSER")
                .map(|(_, value)| PathBuf::from(value))
        };
        m.run(|| {
            let login = login_command(&entry, Some(" b@b.co "));
            assert_eq!(login.program, "claude");
            assert_eq!(login.args, ["auth", "login", "--email", "b@b.co"]);
            assert_eq!(
                login.env[0],
                ("CLAUDE_CONFIG_DIR".to_string(), two.display().to_string())
            );
            let opener = browser(&login.env).expect("the private-window opener");
            assert!(opener.ends_with("claude-sign-in-browser"), "{opener:?}");
            assert!(opener.is_file());
            assert_eq!(login.cwd, two);
            assert_eq!(login.title, "Sign in · ~/.claude-2");
            assert_eq!(login_command(&entry, Some("")).args, ["auth", "login"]);
            let logout = logout_command(&entry);
            assert_eq!(logout.args, ["auth", "logout"]);
            assert_eq!(logout.env, login.env[..1], "no browser to sign out");
            // The default account runs as orion was started: no dir of its
            // own to set.
            let claude = orion_core::harness::builtin("claude").unwrap();
            let login = login_command(&claude, None);
            assert!(login.env.iter().all(|(name, _)| name == "BROWSER"));
            assert_eq!(login.cwd, m.home.path().join(".claude"));
            assert_eq!(login.title, "Sign in · ~/.claude");
        });
    }

    /// The sign-in's browser opens the page in a private window: the
    /// default browser's when it has one, else the first installed of the
    /// rest — and as Claude would itself when none of them is there.
    #[test]
    fn the_sign_in_page_opens_in_a_private_window() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = dir.path().join("log");
        // `open` logs each call and finds only the apps in $INSTALLED;
        // `osascript` names $DEFAULT as the default browser.
        let stub = |name: &str, body: String| {
            let path = bin.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        stub(
            "open",
            format!(
                r#"echo "$*" >> '{}'
[ "$1" = -nb ] || exit 0
case " $INSTALLED " in *" $2 "*) exit 0 ;; *) exit 1 ;; esac"#,
                log.display()
            ),
        );
        stub("osascript", r#"printf '%s\n' "$DEFAULT""#.into());
        let script = write_private_browser(dir.path()).unwrap();
        let url = "http://localhost:4321/callback?code=x";
        let run = |default: &str, installed: &str| -> Vec<String> {
            let _ = std::fs::remove_file(&log);
            let status = std::process::Command::new(&script)
                .arg(url)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("DEFAULT", default)
                .env("INSTALLED", installed)
                .status()
                .unwrap();
            assert!(status.success());
            let calls = std::fs::read_to_string(&log).unwrap();
            calls.lines().map(String::from).collect()
        };
        assert_eq!(
            run("com.brave.Browser", "com.google.Chrome com.brave.Browser"),
            [format!("-nb com.brave.Browser --args --incognito {url}")],
            "the default browser first"
        );
        assert_eq!(
            run("com.apple.Safari", "org.mozilla.firefox"),
            [
                format!("-nb com.google.Chrome --args --incognito {url}"),
                format!("-nb org.mozilla.firefox --args --private-window {url}"),
            ],
            "Safari has no private flag: the first of the rest installed"
        );
        let calls = run("com.apple.Safari", "");
        assert_eq!(calls.len(), PRIVATE_BROWSERS.len() + 1);
        assert_eq!(calls.last().unwrap(), url, "none: the page as before");
    }

    #[test]
    fn a_new_account_is_numbered_or_named() {
        let m = Machine::new("{}");
        m.run(|| {
            let cfg = Config::load();
            let auto = plan_new(&cfg, "").unwrap();
            assert_eq!(auto.id, "claude-2");
            assert_eq!(auto.dir, m.home.path().join(".claude-2"));
            std::fs::create_dir_all(m.home.path().join(".claude-2")).unwrap();
            assert_eq!(
                plan_new(&cfg, "  ").unwrap().id,
                "claude-3",
                "a dir there is skipped"
            );
            let work = plan_new(&cfg, "Work Laptop!").unwrap();
            assert_eq!(work.id, "claude-work-laptop");
            assert_eq!(plan_new(&cfg, "claude-b").unwrap().id, "claude-b");
            assert_eq!(
                plan_new(&cfg, "2").unwrap().dir,
                m.home.path().join(".claude-2"),
                "a named dir already there is adopted"
            );
            assert!(plan_new(&cfg, "claude").is_err());
            assert!(plan_new(&cfg, "codex").is_ok(), "`claude-codex`, not Codex");
            assert!(plan_new(&cfg, "!!").is_err());
        });
        // A hand-written harness by that id is the user's to replace.
        std::fs::write(
            &m.config,
            r#"{"harnesses": {"claude-b": {"program": "/bin/claude-b", "hooks": "claude"}}}"#,
        )
        .unwrap();
        m.run(|| {
            let err = plan_new(&Config::load(), "b").unwrap_err();
            assert!(
                err.contains("harnesses already defines `claude-b`"),
                "{err}"
            );
        });
    }

    /// Adding makes the dir, links what the default account has of the
    /// shared setup — never its login, transcripts or anything already in
    /// the new dir — and writes the two-key entry.
    #[test]
    fn adding_an_account_shares_the_setup_and_nothing_else() {
        let m = Machine::new("{}");
        let main = m.home.path().join(".claude");
        std::fs::write(main.join("CLAUDE.md"), "be terse").unwrap();
        std::fs::write(main.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(main.join("skills/review")).unwrap();
        std::fs::create_dir_all(main.join("projects/-x")).unwrap();
        std::fs::write(main.join(".credentials.json"), "secret").unwrap();
        m.run(|| {
            let new = plan_new(&Config::load(), "").unwrap();
            std::fs::create_dir_all(&new.dir).unwrap();
            std::fs::write(new.dir.join("settings.json"), r#"{"own": true}"#).unwrap();
            let note = add(&new, true).unwrap();
            assert!(
                note.contains("sharing CLAUDE.md, skills from ~/.claude"),
                "{note}"
            );
            assert_eq!(
                std::fs::read_link(new.dir.join("CLAUDE.md")).unwrap(),
                main.join("CLAUDE.md")
            );
            assert_eq!(
                std::fs::read_to_string(new.dir.join("settings.json")).unwrap(),
                r#"{"own": true}"#,
                "never replaced"
            );
            for private in ["projects", ".credentials.json", ".claude.json"] {
                assert!(!new.dir.join(private).exists(), "{private}");
            }
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&m.config).unwrap()).unwrap();
            assert_eq!(
                saved["claude_accounts"],
                serde_json::json!([{"id": "claude-2", "config_dir": new.dir.display().to_string()}])
            );
            assert!(add(&new, true).is_err(), "once is enough");
            // Without sharing, the dir starts empty.
            let bare = plan_new(&Config::load(), "bare").unwrap();
            let note = add(&bare, false).unwrap();
            assert!(!note.contains("sharing"), "{note}");
            assert_eq!(std::fs::read_dir(&bare.dir).unwrap().count(), 0);
        });
    }

    /// Removing takes the entry, its deltas and the ⌘N default with it,
    /// and leaves the dir where it is unless asked.
    #[test]
    fn removing_an_account_keeps_its_dir_by_default() {
        let m = Machine::new("{}");
        let two = m.home.path().join(".claude-2");
        std::fs::create_dir_all(&two).unwrap();
        std::fs::write(
            &m.config,
            serde_json::json!({
                "claude_accounts": [{"id": "claude-2", "config_dir": two.display().to_string()}],
                "harnesses": {"claude-2": {"model_default": "opus"}},
                "quick_prompt_kind": "claude-2"
            })
            .to_string(),
        )
        .unwrap();
        m.run(|| {
            let note = remove("claude-2", false).unwrap();
            assert!(note.contains("~/.claude-2 stays on disk"), "{note}");
            let cfg = Config::load();
            assert!(cfg.claude_accounts.is_empty());
            assert!(!cfg.harnesses.contains_key("claude-2"));
            assert_eq!(cfg.quick_prompt_kind, "claude");
            assert!(two.is_dir());
            assert!(remove("claude-2", false).is_err(), "gone already");
        });
    }

    /// The default account and `claude-2`, in `m`'s home, both signed in.
    fn two_accounts(m: &Machine, first: &str, second: &str) -> PathBuf {
        let two = m.home.path().join(".claude-2");
        std::fs::write(
            &m.config,
            serde_json::json!({"claude_accounts": [
                {"id": "claude-2", "config_dir": two.display().to_string()}
            ]})
            .to_string(),
        )
        .unwrap();
        m.sign(".claude.json", Some(first));
        m.sign(".claude-2/.claude.json", Some(second));
        two
    }

    fn agent(kind: AgentKind, custom: Option<&str>) -> orion_core::Agent {
        orion_core::Agent {
            id: orion_core::AgentId("a".into()),
            worktree_id: orion_core::WorktreeId("w".into()),
            name: "a".into(),
            status: orion_core::AgentStatus::NeedsFeedback,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind,
            custom_harness: custom.map(str::to_string),
            model: None,
            effort: None,
            session_id: Some("sid".into()),
            sort_order: 0,
            status_changed_at: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
        }
    }

    /// A removal strands the sessions on that account alone — archived
    /// ones too — and the dialog names them and where they can go.
    #[test]
    fn removing_an_account_names_the_sessions_it_strands() {
        let named = |name: &str, kind, custom| orion_core::Agent {
            name: name.into(),
            ..agent(kind, custom)
        };
        let agents = vec![
            named("one", AgentKind::Claude, Some("claude-2")),
            orion_core::Agent {
                archived: true,
                ..named("two", AgentKind::Claude, Some("claude-2"))
            },
            named("default", AgentKind::Claude, None),
            named("other", AgentKind::Claude, Some("claude-3")),
        ];
        let on: Vec<&str> = sessions_on(&agents, "claude-2")
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(on, ["one", "two"]);
        assert!(sessions_on(&agents, "claude-4").is_empty());

        assert_eq!(stranded_note(&[], "Continue on (⇧C)", None), "");
        let names: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        let note = stranded_note(&names, "Continue on (⇧C)", Some("Personal (a@b.co)"));
        assert!(
            note.starts_with("5 sessions run on it — a, b, c and 2 more"),
            "{note}"
        );
        assert!(note.contains("Continue on (⇧C) moves each to Personal (a@b.co) — the same login"));
        let one = stranded_note(&names[..1], "Continue on (⇧C)", None);
        assert!(one.starts_with("1 session runs on it — a —"), "{one}");
        assert!(one.contains("to another account first"), "{one}");
    }

    /// **Continue on** names the other account by its email, and says so
    /// when that email is the session's own: same login, same limit.
    #[test]
    fn continue_on_names_the_account_and_marks_the_same_one() {
        let m = Machine::new("{}");
        two_accounts(&m, "a@b.co", "other@d.co");
        m.run(|| {
            refresh_now();
            assert_eq!(
                Config::load().continue_targets(&agent(AgentKind::Claude, None)),
                [("claude-2".to_string(), "Claude (other@d.co)".to_string())]
            );
        });
        m.sign(".claude-2/.claude.json", Some("a@b.co"));
        m.run(|| {
            refresh_now();
            let cfg = Config::load();
            assert_eq!(
                cfg.continue_targets(&agent(AgentKind::Claude, None)),
                [(
                    "claude-2".to_string(),
                    "Claude (a@b.co) · same account".to_string()
                )]
            );
            assert_eq!(
                cfg.continue_targets(&agent(AgentKind::Claude, Some("claude-2"))),
                [(
                    "claude".to_string(),
                    "Claude (a@b.co) · same account".to_string()
                )]
            );
        });
    }

    /// Short of room — a card, the quick prompt, a preset — an account is
    /// its email once there are two; a signed-out one keeps its id.
    #[test]
    fn short_of_room_an_account_is_its_email() {
        let m = Machine::new("{}");
        two_accounts(&m, "a@b.co", "c@d.co");
        m.run(|| {
            refresh_now();
            let cfg = Config::load();
            assert_eq!(
                crate::quick_prompt::harness_name(AgentKind::Claude, None),
                "a@b.co"
            );
            assert_eq!(
                crate::agent_picker::session_harness_badge_in(
                    &agent(AgentKind::Claude, Some("claude-2")),
                    &cfg
                ),
                "c@d.co"
            );
            assert_eq!(
                crate::quick_prompt::harness_name(AgentKind::Codex, None),
                "codex"
            );
            let preset = crate::agent_presets::AgentPreset {
                name: "r".into(),
                kind: AgentKind::Claude,
                custom_harness: Some("claude-2".into()),
                follow_box: None,
                model: Some("opus".into()),
                effort: None,
                prefix: String::new(),
                postfix: String::new(),
                skip_task: false,
                system: None,
            };
            assert_eq!(preset.spec_label(), "c@d.co · opus");
        });
        m.sign(".claude-2/.claude.json", None);
        m.run(|| {
            refresh_now();
            assert_eq!(
                crate::agent_picker::session_harness_badge_in(
                    &agent(AgentKind::Claude, Some("claude-2")),
                    &Config::load()
                ),
                "claude-2",
                "not its whole `Claude (not signed in)` label"
            );
        });
    }

    /// `⌘N` can start on any account: the Accounts tab's **New agents on** row
    /// steps through them right after `claude`, names one by its email,
    /// and one switched off steps aside to Claude.
    #[test]
    fn the_quick_prompt_can_start_on_another_account() {
        let m = Machine::new("{}");
        two_accounts(&m, "a@b.co", "c@d.co");
        m.run(|| {
            refresh_now();
            let mut cfg = Config::load();
            assert_eq!(
                cfg.quick_prompt_choices()[..3],
                ["claude", "claude-2", "codex"]
            );
            // The default account reads as its name and email too, not
            // the bare `claude` it is stored as.
            assert_eq!(
                cfg.value_label(crate::config::SettingKind::QuickPromptKind),
                "Claude (a@b.co)"
            );
            assert!(rename("claude", "Personal").is_ok());
            assert_eq!(
                Config::load().value_label(crate::config::SettingKind::QuickPromptKind),
                "Personal (a@b.co)"
            );
            let (tab, row) =
                crate::config::locate(crate::config::SettingKind::QuickPromptKind).unwrap();
            cfg.cycle(tab, row, 1);
            assert_eq!(cfg.quick_prompt_kind, "claude-2");
            assert_eq!(
                cfg.quick_prompt_harness(),
                (AgentKind::Claude, Some("claude-2".to_string()))
            );
            assert_eq!(
                cfg.value_label(crate::config::SettingKind::QuickPromptKind),
                "Claude (c@d.co)"
            );
            let launch = crate::quick_prompt::QuickLaunch::from_config(
                crate::quick_prompt::QuickTarget::Worktree(orion_core::WorktreeId("w".into())),
                &cfg,
            );
            assert_eq!(
                (launch.kind, launch.custom.as_deref()),
                (AgentKind::Claude, Some("claude-2"))
            );
            cfg.claude_accounts[0].enabled = false;
            assert_eq!(cfg.quick_prompt_harness(), (AgentKind::Claude, None));
        });
    }

    /// An account an older release let follow Claude's model and effort
    /// gets its own copy of them, so it launches as it did; from then on
    /// each account's are its own, and a change to one leaves the other.
    #[test]
    fn an_account_keeps_the_defaults_it_followed_as_its_own() {
        let m = Machine::new("{}");
        let two = m.home.path().join(".claude-2");
        std::fs::write(
            &m.config,
            serde_json::json!({
                "claude_model": "opus",
                "claude_effort": "high",
                "claude_accounts": [{"id": "claude-2", "config_dir": two.display().to_string()}]
            })
            .to_string(),
        )
        .unwrap();
        m.run(|| {
            let mut cfg = Config::load();
            let row = cfg.effective_harness_by_id("claude-2");
            assert_eq!(
                (row.default_model(), row.default_effort()),
                (Some("opus"), Some("high"))
            );
            let (_, model) = cfg
                .agent_rows()
                .iter()
                .position(|(id, f)| id == "claude-2" && *f == crate::config::HarnessField::Model)
                .map(|i| (0, cfg.agent_rows_from() + i))
                .unwrap();
            cfg.cycle(crate::config::accounts_tab(), model, 1);
            let row = cfg.effective_harness_by_id("claude-2");
            assert_ne!(row.default_model(), Some("opus"), "its own now");
            assert_eq!(
                cfg.effective_harness_by_id("claude").default_model(),
                Some("opus"),
                "Claude's stays"
            );
            assert!(cfg.harnesses["claude-2"].model_default.is_some());
            // Its switch is its entry's.
            let (_, enabled) = cfg
                .agent_rows()
                .iter()
                .position(|(id, f)| id == "claude-2" && *f == crate::config::HarnessField::Enabled)
                .map(|i| (0, cfg.agent_rows_from() + i))
                .unwrap();
            cfg.cycle(crate::config::accounts_tab(), enabled, 0);
            assert!(!cfg.claude_accounts[0].enabled);
            assert!(cfg.harnesses["claude-2"].enabled.is_none());
        });
    }

    /// The Accounts tab lists every account under its own section, each
    /// harness section of an added account headed with its dir, and the
    /// same-account warning under the accounts it is about.
    #[test]
    fn the_agents_tab_lists_the_accounts_and_warns_once() {
        let m = Machine::new("{}");
        two_accounts(&m, "a@b.co", "a@b.co");
        m.run(|| {
            refresh_now();
            let cfg = Config::load();
            use crate::config::{AccountRow, SettingsRow};
            assert_eq!(
                cfg.account_rows(),
                [
                    AccountRow::Account("claude".into()),
                    AccountRow::Account("claude-2".into()),
                    AccountRow::Add
                ]
            );
            assert_eq!(cfg.section_title("claude"), "Claude (a@b.co)");
            assert_eq!(cfg.section_title("claude-2"), "Claude (a@b.co) · ~/.claude-2");
            assert_eq!(
                cfg.account_value(&AccountRow::Account("claude-2".into())),
                "on · ~/.claude-2 · same as ~/.claude · tag a3"
            );
            assert_eq!(cfg.account_value(&AccountRow::Add), "~/.claude-3");
            let rows = crate::config::settings_rows(crate::config::accounts_tab(), 0);
            let add = crate::config::ACCOUNTS_HEAD.len() + 2;
            let at = rows
                .iter()
                .position(|row| *row == SettingsRow::Setting(add))
                .unwrap();
            assert!(
                matches!(&rows[at + 1], SettingsRow::Note(note)
                    if note.starts_with("⚠ ~/.claude and ~/.claude-2 are signed in as one account")),
                "{rows:?}"
            );
            let notes = rows
                .iter()
                .filter(|row| matches!(row, SettingsRow::Note(_)))
                .count();
            assert_eq!(notes, 4, "one warning, four lines");
        });
    }

    /// A hand-edited entry the registry can't take is named under the
    /// section, with why, rather than vanishing.
    #[test]
    fn an_entry_left_out_says_why() {
        let m = Machine::new(
            r#"{"claude_accounts": [
                {"id": "claude-2", "config_dir": ".claude-2"},
                {"id": "codex", "config_dir": "/srv/codex"}
            ]}"#,
        );
        m.run(|| {
            let notes = Config::load().account_notes();
            assert_eq!(notes.len(), 2, "{notes:?}");
            assert!(
                notes[0].contains("give an absolute or ~/ path"),
                "{notes:?}"
            );
            assert!(notes[1].contains("collides with a built-in"), "{notes:?}");
            assert!(notes.iter().all(|n| n.ends_with("left out")));
        });
    }

    /// `m`'s config, written whole.
    fn config(m: &Machine, value: serde_json::Value) {
        std::fs::write(&m.config, value.to_string()).unwrap();
    }

    /// A name goes before the email wherever the account is listed, and
    /// is what a card says short of room — signed in or not, since a name
    /// says which account better than an id does.
    #[test]
    fn a_name_goes_before_the_email() {
        let m = Machine::new("{}");
        let work = m.home.path().join(".claude-work");
        config(
            &m,
            serde_json::json!({"claude_accounts": [
                {"id": "claude-work", "config_dir": work.display().to_string(), "name": "Work"}
            ]}),
        );
        m.sign(".claude.json", Some("a@b.co"));
        std::fs::create_dir_all(&work).unwrap();
        m.run(|| {
            refresh_now();
            let cfg = Config::load();
            assert_eq!(label_of(&cfg, "claude-work"), "Work (not signed in)");
            assert_eq!(label_of(&cfg, "claude"), "Claude (a@b.co)");
            assert_eq!(
                short_name(AgentKind::Claude, Some("claude-work")).as_deref(),
                Some("Work"),
                "named, so not its id even signed out"
            );
            assert_eq!(
                short_name(AgentKind::Claude, None).as_deref(),
                Some("a@b.co"),
                "unnamed: its email, as ever"
            );
            assert_eq!(
                cfg.section_title("claude-work"),
                "Work (not signed in) · ~/.claude-work"
            );
        });
        m.sign(".claude-work/.claude.json", Some("w@corp.co"));
        m.run(|| {
            refresh_now();
            assert_eq!(label_of(&Config::load(), "claude-work"), "Work (w@corp.co)");
            assert_eq!(
                short_name(AgentKind::Claude, Some("claude-work")).as_deref(),
                Some("Work")
            );
        });
    }

    /// What the name prompt types stays as typed — trimmed — and goes
    /// into the account's `harnesses` entry; nothing typed is no name at
    /// all, and no entry.
    #[test]
    fn an_added_account_keeps_the_name_as_typed() {
        let m = Machine::new("{}");
        m.run(|| {
            let new = plan_new(&Config::load(), "  Work Laptop ").unwrap();
            assert_eq!(
                (new.id.as_str(), new.name.as_str()),
                ("claude-work-laptop", "Work Laptop")
            );
            let note = add(&new, false).unwrap();
            assert!(
                note.starts_with("added Work Laptop (claude-work-laptop) in"),
                "{note}"
            );
            let unnamed = plan_new(&Config::load(), "").unwrap();
            assert_eq!(unnamed.name, "");
            add(&unnamed, false).unwrap();
            let saved: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&m.config).unwrap()).unwrap();
            assert_eq!(
                saved["harnesses"]["claude-work-laptop"]["label"],
                "Work Laptop"
            );
            assert!(saved["claude_accounts"][0].get("name").is_none(), "{saved}");
            assert!(saved["harnesses"].get(&unnamed.id).is_none(), "{saved}");
        });
    }

    /// A rename moves the name and nothing else: the id sessions point
    /// back at and the dir the login lives in stay. Every account's name
    /// is its `harnesses` label, beside its model and effort, gone with
    /// the delta when cleared.
    #[test]
    fn renaming_moves_only_the_name() {
        let m = Machine::new("{}");
        let two = m.home.path().join(".claude-2");
        config(
            &m,
            serde_json::json!({
                "claude_accounts": [{"id": "claude-2", "config_dir": two.display().to_string()}],
                "harnesses": {"claude-2": {"model_default": "opus"}}
            }),
        );
        m.sign(".claude.json", Some("a@b.co"));
        m.run(|| {
            refresh_now();
            assert_eq!(current_name(&Config::load(), "claude-2"), "");
            assert_eq!(
                rename("claude-2", " Work ").unwrap(),
                "claude-2 goes by Work now"
            );
            let cfg = Config::load();
            let account = &cfg.claude_accounts[0];
            assert_eq!(
                (account.id.as_str(), account.config_dir.clone()),
                ("claude-2", two.display().to_string())
            );
            assert_eq!(current_name(&cfg, "claude-2"), "Work");
            let over = &cfg.harnesses["claude-2"];
            assert_eq!(
                (over.label.as_deref(), over.model_default.as_deref()),
                (Some("Work"), Some("opus")),
                "beside its model"
            );
            rename("claude-2", "").unwrap();
            let cfg = Config::load();
            assert_eq!(current_name(&cfg, "claude-2"), "");
            assert_eq!(cfg.harnesses["claude-2"].label, None, "the model stays");

            // The default account.
            rename("claude", "Personal").unwrap();
            let cfg = Config::load();
            assert_eq!(cfg.harnesses["claude"].label.as_deref(), Some("Personal"));
            assert_eq!(label_of(&cfg, "claude"), "Personal (a@b.co)");
            assert_eq!(current_name(&cfg, "claude"), "Personal");
            assert!(refresh_now(), "the cards' short name moved");
            assert_eq!(
                short_name(AgentKind::Claude, None).as_deref(),
                Some("Personal")
            );
            let note = rename("claude", "").unwrap();
            assert!(note.contains("goes by Claude again"), "{note}");
            let cfg = Config::load();
            assert!(!cfg.harnesses.contains_key("claude"), "nothing left of it");
            assert_eq!(label_of(&cfg, "claude"), "Claude (a@b.co)");

            assert!(rename("codex", "X").is_err(), "no Claude account");
        });
    }

    /// The user's report: add `work`, sign it in, remove it, add another
    /// under a different name — and the old identity shows up again.
    /// orion's own caches hold nothing stale (a different name is a fresh
    /// dir, signed out); what carries the old login is the kept dir,
    /// which any name that comes to the same id (`Work`, `work!`) adopts
    /// whole — and the add says so when it happens.
    #[test]
    fn removing_and_adding_again_never_hides_the_old_login() {
        let m = Machine::new("{}");
        let home = m.home.path().to_path_buf();
        m.sign(".claude.json", Some("me@home.co"));
        m.run(|| {
            let work = plan_new(&Config::load(), "work").unwrap();
            add(&work, false).unwrap();
            m.sign(".claude-work/.claude.json", Some("me@work.co"));
            refresh_now();
            let cfg = Config::load();
            assert_eq!(label_of(&cfg, "claude-work"), "work (me@work.co)");
            assert_eq!(
                short_name(AgentKind::Claude, Some("claude-work")).as_deref(),
                Some("work")
            );

            let note = remove("claude-work", false).unwrap();
            assert!(note.contains("stays on disk"), "{note}");
            refresh_now();
            let cfg = Config::load();
            assert_eq!(short_name(AgentKind::Claude, Some("claude-work")), None);
            assert_eq!(
                dir_state(&home.join(".claude-work")),
                Some(SignIn::As("me@work.co".into()))
            );

            // Another name: another dir, nothing of the old login in it.
            let personal = plan_new(&cfg, "Personal").unwrap();
            assert_eq!(personal.dir, home.join(".claude-personal"));
            add(&personal, false).unwrap();
            refresh_now();
            let cfg = Config::load();
            assert_eq!(
                label_of(&cfg, "claude-personal"),
                "Personal (not signed in)"
            );

            // A name that comes to the old id takes the old dir, login and
            // all — and says so, before and after.
            let again = plan_new(&cfg, "Work").unwrap();
            assert_eq!(
                (again.id.as_str(), again.name.as_str()),
                ("claude-work", "Work")
            );
            let note = add(&again, false).unwrap();
            assert!(
                note.contains("already on this machine, signed in as me@work.co"),
                "{note}"
            );
            let cfg = Config::load();
            assert_eq!(label_of(&cfg, "claude-work"), "Work (me@work.co)");
        });
    }

    #[test]
    fn the_shared_setup_never_includes_the_login_or_the_history() {
        for private in [
            ".claude.json",
            ".credentials.json",
            "projects",
            "history.jsonl",
            "sessions",
            "todos",
            "statsig",
            "shell-snapshots",
        ] {
            assert!(!SHARED_SETUP.contains(&private), "{private}");
        }
    }
}
