//! ACCOUNT USAGE (`⇧U`): how much is left on every linked account, on one
//! grid — a row per account, a column per window (SESSION, DAY, WEEK,
//! MONTH), each cell the share left and how long until it resets, and `-`
//! where the provider has no cap that wide.
//!
//! Two providers, each asked the way its own client asks:
//!
//! * **Claude**, once per login: `GET api.anthropic.com/api/oauth/usage`
//!   with the account's OAuth token — what Claude Code's own `/usage`
//!   reads. `five_hour` is SESSION, `seven_day` WEEK, and the model-scoped
//!   weeks (`seven_day_opus`, `seven_day_sonnet`) are rows of their own
//!   under the account. The token is Claude Code's: on a Mac a Keychain
//!   item named after the account's config dir ([`claude_service`]),
//!   elsewhere `.credentials.json` in the dir.
//! * **Cursor**, the one `cursor-agent` login: `GET
//!   cursor.com/api/usage-summary` with the session cookie its dashboard
//!   sends, built from `cursor-agent`'s token and account id. The billing
//!   cycle is MONTH.
//!
//! Both endpoints are undocumented, and Anthropic's answers a client that
//! asks every minute with 429s for good — so nothing here asks often: an
//! account is read at most every [`POLL`] (the loop's git beat calls
//! [`request`], which spaces itself), when the modal opens on a reading
//! older than that, or on `r`, which still waits [`RETRY_GAP`] between
//! two asks of one account. A failed ask keeps the last reading on screen,
//! marked with why.
//!
//! A token is only ever read — never refreshed. Claude Code's refresh
//! tokens rotate, so a refresh from here would sign the account out of
//! Claude Code. An expired token says so, and the next session on that
//! account renews it.
//!
//! Every ask runs on the blocking pool (`view_jobs`) and lands through
//! [`land`]; the readings — percentages and reset times, never a token,
//! an email or a cookie — are kept in `usage.json` in the DATA DIR, so the
//! grid paints from the last run while the next ask is out.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use orion_core::harness::HarnessDescriptor;

use crate::app::{App, Overlay};
use crate::claude_accounts::SignIn;
use crate::config::Config;
use crate::theme::Theme;

/// How often an account is read unasked.
pub const POLL: Duration = Duration::from_secs(15 * 60);

/// The least time between two asks of one account, `r` included.
pub const RETRY_GAP: Duration = Duration::from_secs(60);

/// The data-dir file the readings are kept in.
pub const CACHE_FILE: &str = "usage.json";

const CLAUDE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CURSOR_URL: &str = "https://cursor.com/api/usage-summary";
const TIMEOUT_SECS: &str = "20";

/// The Keychain item Claude Code keeps an unpinned login in; a pinned
/// config dir's adds `-<8 hex of sha256(dir)>`.
const CLAUDE_SERVICE: &str = "Claude Code-credentials";
const CURSOR_SERVICE: &str = "cursor-access-token";
const CURSOR_KEY_ACCOUNT: &str = "cursor-user";

// ---- the grid ----

/// The grid's columns, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Session,
    Day,
    Week,
    Month,
}

impl Window {
    pub const ALL: [Window; 4] = [Window::Session, Window::Day, Window::Week, Window::Month];

    pub fn title(self) -> &'static str {
        match self {
            Window::Session => "SESSION (5h)",
            Window::Day => "DAY",
            Window::Week => "WEEK",
            Window::Month => "MONTH",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One window of one account: how much of it is used, and when it resets.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// 0–100.
    pub used_pct: f64,
    /// Epoch seconds; None when the provider gave no time.
    pub resets_at: Option<i64>,
}

impl Cell {
    pub fn left_pct(&self) -> f64 {
        (100.0 - self.used_pct).clamp(0.0, 100.0)
    }
}

/// One line of the grid: a cell per [`Window`], None for `-`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub label: String,
    pub cells: [Option<Cell>; Window::ALL.len()],
}

impl Row {
    fn with(label: &str, window: Window, cell: Cell) -> Self {
        let mut row = Row {
            label: label.to_string(),
            ..Row::default()
        };
        row.cells[window.index()] = Some(cell);
        row
    }
}

/// What one ask of an account came back with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    /// The plan, as the provider names it: `max`, `pro`, `team`.
    #[serde(default)]
    pub plan: Option<String>,
    pub main: Row,
    /// Model-scoped weeks (Claude), the Auto / API split (Cursor).
    #[serde(default)]
    pub sub: Vec<Row>,
    /// Pay-as-you-go spend past the plan, in the provider's words.
    #[serde(default)]
    pub extra: Option<String>,
    /// Epoch seconds.
    pub fetched_at: i64,
}

/// Why an ask came back without a reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// No login to ask with.
    SignedOut,
    /// The token is past its time, or the provider refused it.
    Expired,
    /// The provider asked us to slow down.
    RateLimited,
    Failed(String),
}

impl Problem {
    fn says(&self, provider: Provider) -> String {
        match (self, provider) {
            (Problem::SignedOut, Provider::Claude) => "not signed in · Enter signs in".into(),
            (Problem::SignedOut, Provider::Cursor) => {
                "not signed in · run `cursor-agent login`".into()
            }
            (Problem::Expired, Provider::Claude) => {
                "login token expired · a session on it renews it, or Enter signs in".into()
            }
            (Problem::Expired, Provider::Cursor) => {
                "login token refused · run `cursor-agent login`".into()
            }
            (Problem::RateLimited, _) => {
                "the provider asked to slow down · tries again later".into()
            }
            (Problem::Failed(why), _) => why.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Cursor,
}

/// One account the grid lists, as config and the sign-in records name it.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    /// The harness id: `claude`, `claude-2`, `cursor`. The row's key.
    pub id: String,
    pub provider: Provider,
    /// `Work (a@b.co)`, `Cursor`.
    pub label: String,
    /// What the sign-in record says, for a Claude account.
    pub email: Option<String>,
    pub signed_out: bool,
    /// The dir whose hash names the Keychain item: the pinned dir, else
    /// this process's `CLAUDE_CONFIG_DIR`; None for the unsuffixed item.
    pub keychain_dir: Option<PathBuf>,
    /// The config dir, for `.credentials.json`.
    pub dir: Option<PathBuf>,
    /// Other accounts signed in as the same login: one subscription, one
    /// set of limits, so one row.
    pub also: Vec<String>,
}

/// Everything the grid knows, kept on the app so the modal opens on it.
#[derive(Debug, Default)]
pub struct Usage {
    pub accounts: Vec<Account>,
    pub readings: BTreeMap<String, Reading>,
    pub problems: BTreeMap<String, Problem>,
    pub inflight: BTreeSet<String>,
    asked: BTreeMap<String, Instant>,
    polled: Option<Instant>,
    /// When `accounts` was last built from config: the beat rebuilds it
    /// only every [`POLL`], open and `r` every time.
    listed: Option<Instant>,
    /// Where the readings are kept; None under test.
    cache: Option<PathBuf>,
    /// The readings changed since the cache was last written.
    unsaved: bool,
}

impl Usage {
    /// The readings the last run kept.
    pub fn load(cache: PathBuf) -> Self {
        let readings = std::fs::read(&cache)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            readings,
            cache: Some(cache),
            ..Self::default()
        }
    }

    /// Whether `id` is due an ask: never while one is out, never inside
    /// [`RETRY_GAP`] of the last, and otherwise when `force` says so or the
    /// reading is [`POLL`] old.
    fn due(&self, id: &str, now: i64, force: bool) -> bool {
        if self.inflight.contains(id) {
            return false;
        }
        if let Some(at) = self.asked.get(id) {
            if at.elapsed() < RETRY_GAP || (!force && at.elapsed() < POLL) {
                return false;
            }
        }
        force
            || self
                .readings
                .get(id)
                .is_none_or(|r| now - r.fetched_at >= POLL.as_secs() as i64)
    }

    /// Rebuild the account list, and forget whatever was kept for an
    /// account that is no longer listed.
    fn relist(&mut self, accounts: Vec<Account>) {
        let listed: BTreeSet<&str> = accounts.iter().map(|a| a.id.as_str()).collect();
        let before = self.readings.len();
        self.readings.retain(|id, _| listed.contains(id.as_str()));
        self.unsaved |= self.readings.len() != before;
        self.problems.retain(|id, _| listed.contains(id.as_str()));
        self.asked.retain(|id, _| listed.contains(id.as_str()));
        self.accounts = accounts;
        self.listed = Some(Instant::now());
    }
}

/// The accounts `cfg` links, in the order the grid lists them: every
/// enabled Claude account, those signed in as one login folded into the
/// first, then Cursor while its CLI is installed.
pub fn accounts(cfg: &Config) -> Vec<Account> {
    let registry = cfg.harness_registry();
    let mut out: Vec<Account> = Vec::new();
    if cfg.usage_claude {
        for entry in registry
            .iter()
            .filter(|e| e.enabled && e.is_claude_account())
        {
            let email = crate::claude_accounts::email_of(entry);
            if let Some(email) = &email {
                let same = out.iter_mut().find(|a| {
                    a.email
                        .as_deref()
                        .is_some_and(|seen| seen.eq_ignore_ascii_case(email))
                });
                if let Some(first) = same {
                    first.also.push(entry.id.clone());
                    continue;
                }
            }
            out.push(claude_account(entry, email));
        }
    }
    if cfg.usage_cursor {
        if let Some(entry) = registry.iter().find(|e| {
            e.id == orion_core::AgentKind::Cursor.as_str()
                && e.enabled
                && crate::config::program_installed(&e.program)
        }) {
            out.push(Account {
                id: entry.id.clone(),
                provider: Provider::Cursor,
                label: "Cursor".into(),
                email: None,
                signed_out: false,
                keychain_dir: None,
                dir: None,
                also: Vec::new(),
            });
        }
    }
    out
}

fn claude_account(entry: &HarnessDescriptor, email: Option<String>) -> Account {
    let signed_out = crate::claude_accounts::record_of(entry)
        .and_then(|record| crate::claude_accounts::state_of(&record))
        == Some(SignIn::Out);
    let keychain_dir = entry.pinned_claude_config_dir().or_else(|| {
        orion_core::env::non_empty(orion_core::env::CLAUDE_CONFIG_DIR).map(PathBuf::from)
    });
    Account {
        id: entry.id.clone(),
        provider: Provider::Claude,
        label: entry.label.clone(),
        email,
        signed_out,
        keychain_dir,
        dir: crate::claude_accounts::dir_of(entry),
        also: Vec::new(),
    }
}

// ---- asking ----

/// One ask, landed.
#[derive(Debug)]
pub struct Answer {
    pub id: String,
    pub result: Result<Reading, Problem>,
}

/// Read the accounts from config again — what open and `r` do at once,
/// and the beat every [`POLL`].
fn relist(app: &mut App) {
    let before = app.usage.accounts.clone();
    app.usage.relist(accounts(&Config::load()));
    app.dirty |= app.usage.accounts != before;
}

/// Ask every account that is due. The beat calls it with `force` false
/// and it spaces itself: a look at most every [`RETRY_GAP`], the config
/// read again at most every [`POLL`]. `r` (`force`) reads the config and
/// asks every account at once, [`RETRY_GAP`] allowing.
pub fn request(app: &mut App, force: bool) {
    if usage_off() {
        return;
    }
    if !force && app.usage.polled.is_some_and(|at| at.elapsed() < RETRY_GAP) {
        return;
    }
    app.usage.polled = Some(Instant::now());
    if force || app.usage.listed.is_none_or(|at| at.elapsed() >= POLL) {
        relist(app);
    }
    let Some(jobs) = app.view_jobs.clone() else {
        return;
    };
    let now = orion_core::clock::now_secs() as i64;
    let due: Vec<Account> = app
        .usage
        .accounts
        .iter()
        .filter(|a| app.usage.due(&a.id, now, force))
        .cloned()
        .collect();
    for account in due {
        app.usage.asked.insert(account.id.clone(), Instant::now());
        app.usage.inflight.insert(account.id.clone());
        app.dirty = true;
        jobs.run(move || {
            Some(crate::view_jobs::Answer::Usage(Answer {
                result: fetch(&account),
                id: account.id,
            }))
        });
    }
}

/// An ask came back: keep the reading — or the last one, beside why the
/// new one failed. A signed-out account's numbers go: they are no one's.
pub fn land(app: &mut App, answer: Answer) {
    let Answer { id, result } = answer;
    app.usage.inflight.remove(&id);
    match result {
        Ok(reading) => {
            app.usage.problems.remove(&id);
            app.usage.readings.insert(id, reading);
            app.usage.unsaved = true;
        }
        Err(problem) => {
            if problem == Problem::SignedOut {
                app.usage.unsaved |= app.usage.readings.remove(&id).is_some();
            }
            app.usage.problems.insert(id, problem);
        }
    }
    app.dirty = true;
}

/// The readings to write, when they changed since the last write — taken
/// on the loop's beat and written off it, one writer at a time, as
/// `pr_cache::take_flush` does.
pub fn take_flush(app: &mut App) -> Option<(PathBuf, BTreeMap<String, Reading>)> {
    if !std::mem::take(&mut app.usage.unsaved) {
        return None;
    }
    Some((app.usage.cache.clone()?, app.usage.readings.clone()))
}

/// Write a flush out. A cache that could not be written is a next launch
/// that opens on older numbers, never a broken one.
pub fn write_cache(path: &Path, readings: &BTreeMap<String, Reading>) {
    if let Err(err) = crate::pr_cache::write_json_atomic(path, readings) {
        tracing::warn!("usage cache not written: {err}");
    }
}

/// Read `account`'s token and ask its provider. Blocking: the pool's.
fn fetch(account: &Account) -> Result<Reading, Problem> {
    let now = orion_core::clock::now_secs() as i64;
    match account.provider {
        Provider::Claude => {
            if account.signed_out {
                return Err(Problem::SignedOut);
            }
            let creds = claude_credentials(account)?;
            if creds.expires_at_ms.is_some_and(|at| at <= now * 1000) {
                return Err(Problem::Expired);
            }
            let json = get_json(
                CLAUDE_URL,
                &[
                    format!("Authorization: Bearer {}", creds.token),
                    "anthropic-beta: oauth-2025-04-20".into(),
                ],
                "Claude",
            )?;
            Ok(claude_reading(&json, creds.plan, now))
        }
        Provider::Cursor => {
            let creds = cursor_credentials()?;
            let json = get_json(
                CURSOR_URL,
                &[format!(
                    "Cookie: WorkosCursorSessionToken={}%3A%3A{}",
                    creds.account_id, creds.token
                )],
                "Cursor",
            )?;
            Ok(cursor_reading(&json, now))
        }
    }
}

/// [`get`] with `Accept: application/json`, the body parsed; `who` names
/// the provider when it is not JSON.
fn get_json(url: &str, headers: &[String], who: &str) -> Result<Value, Problem> {
    let mut headers = headers.to_vec();
    headers.push("Accept: application/json".into());
    let body = get(url, &headers)?;
    serde_json::from_slice(&body)
        .map_err(|_| Problem::Failed(format!("{who} sent something that wasn't JSON")))
}

/// `GET url` through curl, the headers on its stdin config so no token
/// reaches argv. The body of a 2xx; a [`Problem`] for anything else.
fn get(url: &str, headers: &[String]) -> Result<Vec<u8>, Problem> {
    let mut config = format!("url = \"{}\"\n", config_escape(url));
    for header in headers {
        config.push_str(&format!("header = \"{}\"\n", config_escape(header)));
    }
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            TIMEOUT_SECS,
            "--config",
            "-",
            "-w",
            "\n%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Problem::Failed(format!("couldn't run curl: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|e| Problem::Failed(e.to_string()))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Problem::Failed(format!(
            "couldn't reach the provider: {}",
            err.lines().next().unwrap_or("curl failed")
        )));
    }
    let (body, status) = split_status(&output.stdout);
    match status {
        200..=299 => Ok(body.to_vec()),
        401 | 403 => Err(Problem::Expired),
        429 => Err(Problem::RateLimited),
        code => Err(Problem::Failed(format!(
            "the provider answered HTTP {code}"
        ))),
    }
}

/// curl's `-w "\n%{http_code}"` tail off the body.
fn split_status(out: &[u8]) -> (&[u8], u16) {
    let (body, tail) = match out.iter().rposition(|&b| b == b'\n') {
        Some(at) => (&out[..at], &out[at + 1..]),
        None => (&out[..0], out),
    };
    let code = std::str::from_utf8(tail)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    (body, code)
}

/// A value inside a curl config file's double quotes.
fn config_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

// ---- Claude ----

#[derive(Debug, Clone, PartialEq)]
struct ClaudeCreds {
    token: String,
    expires_at_ms: Option<i64>,
    plan: Option<String>,
}

/// The Keychain item Claude Code keeps the login of a CLI launched with
/// `CLAUDE_CONFIG_DIR` set to `dir` in — None for one launched without.
pub fn claude_service(dir: Option<&Path>) -> String {
    use sha2::{Digest, Sha256};
    match dir {
        None => CLAUDE_SERVICE.to_string(),
        Some(dir) => {
            let hash = Sha256::digest(dir.to_string_lossy().as_bytes());
            let hex: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
            format!("{CLAUDE_SERVICE}-{hex}")
        }
    }
}

fn claude_credentials(account: &Account) -> Result<ClaudeCreds, Problem> {
    let mut blob = None;
    if cfg!(target_os = "macos") {
        blob = keychain(&claude_service(account.keychain_dir.as_deref()), None);
    }
    if blob.is_none() {
        blob = account
            .dir
            .as_ref()
            .and_then(|dir| std::fs::read_to_string(dir.join(".credentials.json")).ok());
    }
    parse_claude_credentials(&blob.ok_or(Problem::SignedOut)?)
}

/// `claudeAiOauth`'s token, expiry and plan; [`Problem::SignedOut`] for a
/// blob with none.
fn parse_claude_credentials(blob: &str) -> Result<ClaudeCreds, Problem> {
    let json: Value = serde_json::from_str(blob.trim()).map_err(|_| Problem::SignedOut)?;
    let oauth = json.get("claudeAiOauth").ok_or(Problem::SignedOut)?;
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or(Problem::SignedOut)?;
    Ok(ClaudeCreds {
        token: token.to_string(),
        expires_at_ms: oauth.get("expiresAt").and_then(Value::as_i64),
        plan: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// `/api/oauth/usage`'s answer as the grid's rows. Unknown windows are
/// left out rather than guessed at.
fn claude_reading(json: &Value, plan: Option<String>, now: i64) -> Reading {
    let window = |key: &str| -> Option<Cell> {
        let w = json.get(key)?;
        Some(Cell {
            used_pct: w.get("utilization")?.as_f64()?,
            resets_at: w
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(crate::pull_request::rfc3339_secs),
        })
    };
    let mut main = Row::default();
    main.cells[Window::Session.index()] = window("five_hour");
    main.cells[Window::Week.index()] = window("seven_day");
    let sub = [("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet")]
        .into_iter()
        .filter_map(|(key, label)| Some(Row::with(label, Window::Week, window(key)?)))
        .collect();
    let extra = json.get("extra_usage").and_then(|x| {
        if !x.get("is_enabled")?.as_bool()? {
            return None;
        }
        let used = x.get("used_credits").and_then(Value::as_f64)?;
        Some(spend(
            "extra usage",
            used,
            x.get("monthly_limit").and_then(Value::as_f64),
        ))
    });
    Reading {
        plan,
        main,
        sub,
        extra,
        fetched_at: now,
    }
}

// ---- Cursor ----

#[derive(Debug, Clone, PartialEq)]
struct CursorCreds {
    token: String,
    account_id: String,
}

fn cursor_credentials() -> Result<CursorCreds, Problem> {
    let home = orion_core::env::home_dir().ok_or(Problem::SignedOut)?;
    let mut token = None;
    if cfg!(target_os = "macos") {
        token = keychain(CURSOR_SERVICE, Some(CURSOR_KEY_ACCOUNT));
    }
    if token.is_none() {
        token = [".config/cursor/auth.json", ".cursor/auth.json"]
            .iter()
            .filter_map(|rel| std::fs::read(home.join(rel)).ok())
            .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .find_map(|json| Some(json.get("accessToken")?.as_str()?.to_string()));
    }
    let token = token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or(Problem::SignedOut)?;
    let config = std::fs::read(home.join(".cursor/cli-config.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let account_id = config
        .as_ref()
        .and_then(cursor_account_id)
        .or_else(|| jwt_subject(&token))
        .ok_or(Problem::SignedOut)?;
    Ok(CursorCreds { token, account_id })
}

/// `authInfo.authId`, else `authInfo.userId` (a number in some builds),
/// with any `provider|` prefix taken off.
fn cursor_account_id(config: &Value) -> Option<String> {
    let info = config.get("authInfo")?;
    let id = ["authId", "userId"]
        .iter()
        .find_map(|key| match info.get(*key)? {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })?;
    Some(id.rsplit('|').next().unwrap_or(&id).to_string())
}

/// A JWT's `sub`, after its last `|`.
fn jwt_subject(token: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let json: Value = serde_json::from_slice(&bytes).ok()?;
    let sub = json.get("sub")?.as_str()?;
    Some(sub.rsplit('|').next().unwrap_or(sub).to_string())
}

/// `/api/usage-summary`'s answer as the grid's rows: the plan's share used
/// this billing cycle — `totalPercentUsed`, else the larger of the Auto and
/// API pools, else `used` of `limit` on a request-counted plan.
fn cursor_reading(json: &Value, now: i64) -> Reading {
    let plan = json.pointer("/individualUsage/plan");
    let pct = |key: &str| plan.and_then(|p| p.get(key)).and_then(Value::as_f64);
    let resets_at = json
        .get("billingCycleEnd")
        .and_then(Value::as_str)
        .and_then(crate::pull_request::rfc3339_secs);
    let (auto, api) = (pct("autoPercentUsed"), pct("apiPercentUsed"));
    let used = pct("totalPercentUsed")
        .or(match (auto, api) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (one, other) => one.or(other),
        })
        .or_else(|| match (pct("used"), pct("limit")) {
            (Some(used), Some(limit)) if limit > 0.0 => Some(100.0 * used / limit),
            _ => None,
        });
    let mut main = Row::default();
    main.cells[Window::Month.index()] = used.map(|used_pct| Cell {
        used_pct,
        resets_at,
    });
    let pool = |label: &str, used_pct: f64| {
        Row::with(
            label,
            Window::Month,
            Cell {
                used_pct,
                resets_at,
            },
        )
    };
    let sub = match (auto, api) {
        (Some(auto), Some(api)) => vec![pool("Auto", auto), pool("API", api)],
        _ => Vec::new(),
    };
    let extra = json.pointer("/individualUsage/onDemand").and_then(|d| {
        let used = d.get("used").and_then(Value::as_f64)?;
        if used <= 0.0 {
            return None;
        }
        Some(spend(
            "on-demand",
            used,
            d.get("limit").and_then(Value::as_f64),
        ))
    });
    Reading {
        plan: json
            .get("membershipType")
            .and_then(Value::as_str)
            .map(str::to_string),
        main,
        sub,
        extra,
        fetched_at: now,
    }
}

// ---- shared ----

/// A Keychain generic password's secret, through `security` — the tool
/// Claude Code saves with, so its items let it read them without a prompt.
fn keychain(service: &str, account: Option<&str>) -> Option<String> {
    let mut cmd = Command::new("/usr/bin/security");
    cmd.args(["find-generic-password", "-s", service]);
    if let Some(account) = account {
        cmd.args(["-a", account]);
    }
    let out = cmd.arg("-w").stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| unhex(text).unwrap_or_else(|| text.to_string()))
}

/// `security -w` prints a secret that is not plain text as hex: a JSON
/// blob saved that way comes back as `7b22…`.
fn unhex(text: &str) -> Option<String> {
    if !text.len().is_multiple_of(2) || !text.starts_with("7b") {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

/// Pay-as-you-go spend in cents, as the details line says it:
/// `on-demand $3.50 of $20.00`, or `on-demand $3.50` with no limit.
fn spend(what: &str, used_cents: f64, limit_cents: Option<f64>) -> String {
    let dollars = |cents: f64| format!("${:.2}", cents / 100.0);
    match limit_cents {
        Some(limit) => format!("{what} {} of {}", dollars(used_cents), dollars(limit)),
        None => format!("{what} {}", dollars(used_cents)),
    }
}

/// How long until `at`: `38m`, `2h14m`, `3d4h`, `19d`; `now` once it has
/// passed.
pub fn until(at: i64, now: i64) -> String {
    let s = at - now;
    if s <= 0 {
        return "now".into();
    }
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    match (d, h) {
        (0, 0) => format!("{}m", m.max(1)),
        (0, _) => format!("{h}h{m:02}m"),
        (1..=6, _) => format!("{d}d{h}h"),
        _ => format!("{d}d"),
    }
}

/// A cell as the grid writes it: `62% · 2h14m`, or `-`.
pub fn cell_text(cell: Option<&Cell>, now: i64) -> String {
    match cell {
        None => "-".into(),
        Some(cell) => {
            let left = format!("{:.0}%", cell.left_pct());
            match cell.resets_at {
                Some(at) => format!("{left} · {}", until(at, now)),
                None => left,
            }
        }
    }
}

// ---- the modal ----

/// The modal's own state; the readings live on the app ([`Usage`]).
#[derive(Debug, Clone, Default)]
pub struct UsageView {
    /// The cursor's row, and the account on it: a list rebuilt from config
    /// keeps the cursor on the account, not on the position.
    pub selected: usize,
    pub selected_id: Option<String>,
    pub area: Rect,
    pub list_area: Rect,
    /// The first account line of `list_area` and how many lines each
    /// account takes there, for a click.
    pub hit_lines: Vec<(u16, usize)>,
}

impl UsageView {
    /// The cursor's row in `accounts`: the account it was on, else the
    /// position, clamped.
    fn row_in(&self, accounts: &[Account]) -> usize {
        self.selected_id
            .as_ref()
            .and_then(|id| accounts.iter().position(|a| &a.id == id))
            .unwrap_or(self.selected)
            .min(accounts.len().saturating_sub(1))
    }

    fn select(&mut self, row: usize, accounts: &[Account]) {
        self.selected = row;
        self.selected_id = accounts.get(row).map(|a| a.id.clone());
    }
}

pub(crate) mod keys {
    use crate::hints::Key;

    pub const REFRESH: Key = Key::new(&["r"], "refresh");
    pub const SIGN_IN: Key = Key::new(&["enter"], "sign in");
    pub const CLOSE: Key = Key::new(&["esc", "q", "shift+u"], "close");
    #[cfg(test)]
    pub const ALL: &[Key] = &[REFRESH, SIGN_IN, CLOSE];
}

/// `⇧U`: the grid on the last readings, the account list read afresh,
/// and anything [`POLL`] old asked for.
pub(crate) fn open(app: &mut App) {
    app.overlay = Some(Overlay::Usage(UsageView::default()));
    if !usage_off() {
        relist(app);
    }
    app.usage.polled = None;
    request(app, false);
}

/// `ORION_USAGE=off`: ask no provider.
fn usage_off() -> bool {
    orion_core::env::non_empty(orion_core::env::USAGE).as_deref() == Some("off")
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(Overlay::Usage(view)) = &mut app.overlay else {
        return;
    };
    let accounts = &app.usage.accounts;
    let row = view.row_in(accounts) as i64;
    match key.code {
        _ if keys::CLOSE.matches(&key) => app.overlay = None,
        KeyCode::Char('j') | KeyCode::Down => {
            view.select(
                crate::app::clamp_selection(row + 1, accounts.len()),
                accounts,
            );
        }
        KeyCode::Char('k') | KeyCode::Up => {
            view.select(
                crate::app::clamp_selection(row - 1, accounts.len()),
                accounts,
            );
        }
        _ if keys::REFRESH.matches(&key) => request(app, true),
        _ if keys::SIGN_IN.matches(&key) => sign_in_selected(app),
        _ => {}
    }
    app.dirty = true;
}

pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, pos: Position) {
    let Some(Overlay::Usage(view)) = &mut app.overlay else {
        return;
    };
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) || !view.list_area.contains(pos) {
        return;
    }
    let line = pos.y - view.list_area.y;
    if let Some(row) = view
        .hit_lines
        .iter()
        .position(|&(first, n)| line >= first && usize::from(line - first) < n)
    {
        if view.row_in(&app.usage.accounts) == row {
            sign_in_selected(app);
        } else {
            view.select(row, &app.usage.accounts);
        }
    }
    app.dirty = true;
}

/// Enter: the selected Claude account's sign-in, when it is signed out or
/// its token was refused.
fn sign_in_selected(app: &mut App) {
    let Some(Overlay::Usage(view)) = &app.overlay else {
        return;
    };
    let accounts = &app.usage.accounts;
    let Some(account) = accounts.get(view.row_in(accounts)).cloned() else {
        return;
    };
    let wants = matches!(
        app.usage.problems.get(&account.id),
        Some(Problem::SignedOut | Problem::Expired)
    ) || account.signed_out;
    if account.provider == Provider::Claude && wants {
        app.overlay = None;
        crate::claude_accounts::sign_in(app, &account.id, account.email.as_deref());
    }
}

/// The harness ids with a session stopped at its usage limit right now.
fn limited_harnesses(app: &App) -> BTreeSet<&str> {
    app.tree
        .agents
        .iter()
        .filter(|a| {
            !a.archived
                && a.usage_limit
                    .as_ref()
                    .is_some_and(|l| l.reason == orion_core::LimitReason::RateLimit)
        })
        .filter_map(|a| match a.kind {
            orion_core::AgentKind::Custom => a.custom_harness.as_deref(),
            kind => Some(kind.as_str()),
        })
        .collect()
}

impl Account {
    /// Whether a session on this row's login — on it or on an account
    /// that shares it — is stopped at its limit.
    fn limit_hit(&self, limited: &BTreeSet<&str>) -> bool {
        std::iter::once(&self.id)
            .chain(&self.also)
            .any(|id| limited.contains(id.as_str()))
    }
}

const NAME_MIN: usize = 18;
const NAME_MAX: usize = 34;
const PLAN_W: usize = 6;
const CELL_W: usize = 14;
/// Below this share left a cell turns red…
const SCARCE_LEFT_PCT: f64 = 10.0;
/// …and below this one, yellow.
const LOW_LEFT_PCT: f64 = 30.0;

pub(crate) fn draw(f: &mut Frame, app: &mut App, view: &UsageView, th: Theme) {
    let now = orion_core::clock::now_secs() as i64;
    let screen = f.area();
    let all = Window::ALL.len();
    let widest = (NAME_MAX + PLAN_W + all * CELL_W + 3) as u16;
    let width = screen.width.saturating_sub(4).min(widest);
    // Narrow: DAY goes first — no provider caps a day yet — then the plan.
    let inner_w = width.saturating_sub(2) as usize;
    let show_day = inner_w > NAME_MIN + PLAN_W + all * CELL_W;
    let show_plan = inner_w > NAME_MIN + PLAN_W + (all - 1) * CELL_W;
    let windows: Vec<Window> = Window::ALL
        .into_iter()
        .filter(|w| show_day || *w != Window::Day)
        .collect();
    let fixed = windows.len() * CELL_W + if show_plan { PLAN_W } else { 0 };
    let name_w = inner_w.saturating_sub(fixed + 1).max(NAME_MIN / 2);

    let dim = Style::default().fg(th.dim);
    let header = Style::default().fg(th.muted).add_modifier(Modifier::BOLD);
    // A cell's colour: how much is left, dimmed while the reading is stale.
    let cell_style = |cell: Option<&Cell>, stale: bool| match cell {
        _ if stale => dim,
        None => Style::default().fg(th.faint),
        Some(c) if c.left_pct() < SCARCE_LEFT_PCT => Style::default().fg(th.err),
        Some(c) if c.left_pct() < LOW_LEFT_PCT => Style::default().fg(th.warn),
        Some(_) => Style::default().fg(th.ok),
    };

    let usage = &app.usage;
    let limited = limited_harnesses(app);
    let selected = view.row_in(&usage.accounts);
    let mut lines: Vec<Line> = Vec::new();
    let mut head = format!(" {:<name_w$}", "ACCOUNT");
    if show_plan {
        head.push_str(&format!("{:<PLAN_W$}", "PLAN"));
    }
    for w in &windows {
        head.push_str(&format!("{:<CELL_W$}", w.title()));
    }
    lines.push(Line::from(Span::styled(head, header)));
    let list_top = lines.len() as u16;
    let mut hit_lines = Vec::new();

    if usage.accounts.is_empty() {
        lines.push(Line::from(Span::styled(
            " no Claude account or Cursor login to read — add one in Settings → Agents",
            dim,
        )));
    }
    for (i, account) in usage.accounts.iter().enumerate() {
        let first = lines.len() as u16 - list_top;
        let reading = usage.readings.get(&account.id);
        let problem = usage.problems.get(&account.id);
        let stale = problem.is_some();
        let hit = account.limit_hit(&limited);
        let sel = |s: Style| {
            if i == selected {
                s.bg(th.sel_bg).add_modifier(Modifier::BOLD)
            } else {
                s
            }
        };
        let mut name = account.label.clone();
        if usage.inflight.contains(&account.id) {
            name.push_str(" …");
        }
        let name_style = Style::default().fg(if hit { th.err } else { th.text });
        let mut spans = vec![Span::styled(
            format!(
                " {:<name_w$}",
                crate::ui::truncate(&name, name_w.saturating_sub(1))
            ),
            sel(name_style),
        )];
        if show_plan {
            let plan = reading.and_then(|r| r.plan.as_deref()).unwrap_or("");
            spans.push(Span::styled(
                format!("{:<PLAN_W$}", crate::ui::truncate(plan, PLAN_W - 1)),
                sel(dim),
            ));
        }
        let signed_out = matches!(problem, Some(Problem::SignedOut))
            || (account.signed_out && reading.is_none());
        match reading {
            Some(reading) if !signed_out => {
                for w in &windows {
                    let cell = reading.main.cells[w.index()].as_ref();
                    let (text, style) = if hit && *w == Window::Session && cell.is_some() {
                        ("▲ limit hit".to_string(), Style::default().fg(th.err))
                    } else {
                        (cell_text(cell, now), cell_style(cell, stale))
                    };
                    spans.push(Span::styled(format!("{text:<CELL_W$}"), sel(style)));
                }
            }
            _ => {
                let text = match problem {
                    Some(p) => p.says(account.provider),
                    None if signed_out => Problem::SignedOut.says(account.provider),
                    None if usage.inflight.contains(&account.id) => "reading…".into(),
                    None => "not read yet · r reads it".into(),
                };
                let style = if signed_out || stale {
                    Style::default().fg(th.warn)
                } else {
                    dim
                };
                spans.push(Span::styled(
                    format!("{:<w$}", text, w = windows.len() * CELL_W),
                    sel(style),
                ));
            }
        }
        lines.push(Line::from(spans));
        if let Some(reading) = reading.filter(|_| !signed_out) {
            for sub in &reading.sub {
                let mut spans = vec![Span::styled(
                    format!("   {:<w$}", sub.label, w = name_w.saturating_sub(2)),
                    dim,
                )];
                if show_plan {
                    spans.push(Span::raw(" ".repeat(PLAN_W)));
                }
                for w in &windows {
                    let cell = sub.cells[w.index()].as_ref();
                    spans.push(Span::styled(
                        format!("{:<CELL_W$}", cell_text(cell, now)),
                        cell_style(cell, stale),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
        hit_lines.push((first, lines.len() - list_top as usize - first as usize));
    }

    // The selected account in words: when it was read, what went wrong,
    // what else shares it.
    lines.push(Line::from(""));
    if let Some(account) = usage.accounts.get(selected) {
        let mut notes: Vec<String> = Vec::new();
        if let Some(reading) = usage.readings.get(&account.id) {
            notes.push(format!(
                "read {}",
                crate::hosts::ago_label((now - reading.fetched_at) * 1000)
            ));
            if let Some(extra) = &reading.extra {
                notes.push(extra.clone());
            }
            if let Some(problem) = usage.problems.get(&account.id) {
                notes.push(problem.says(account.provider));
            }
        }
        if account.limit_hit(&limited) {
            notes.push("a session on it is stopped at its limit".into());
        }
        if !account.also.is_empty() {
            notes.push(format!("same login as {}", account.also.join(", ")));
        }
        if !notes.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(" {}", notes.join(" · ")),
                dim,
            )));
        }
    }
    lines.push(Line::from(Span::styled(
        " the share left in each window · - where the provider has no cap that wide",
        Style::default().fg(th.faint),
    )));

    let height = (lines.len() as u16 + 2).min(screen.height.saturating_sub(2));
    let area = crate::ui::centered_rect(screen, width, height);
    let hints = [
        keys::REFRESH.hint(),
        keys::SIGN_IN.hint(),
        crate::hints::ESC_CLOSE.hint(),
    ];
    let title = format!(" Account usage · read every {}m ", POLL.as_secs() / 60);
    let inner = crate::ui::render_modal_frame(f, area, title, &hints, th);
    f.render_widget(Paragraph::new(lines), inner);
    let selected_id = app.usage.accounts.get(selected).map(|a| a.id.clone());
    if let Some(Overlay::Usage(v)) = &mut app.overlay {
        v.area = area;
        v.selected = selected;
        v.selected_id = selected_id;
        v.hit_lines = hit_lines;
        v.list_area = Rect {
            x: inner.x,
            y: inner.y + list_top,
            width: inner.width,
            height: inner.height.saturating_sub(list_top),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_FIXTURE: &str = r#"{
        "five_hour": {"utilization": 38.0, "resets_at": "2026-10-05T14:00:00.000000+00:00"},
        "seven_day": {"utilization": 59, "resets_at": "2026-10-09T08:00:00Z"},
        "seven_day_oauth_apps": null,
        "seven_day_opus": {"utilization": 12.5, "resets_at": "2026-10-09T08:00:00Z"},
        "seven_day_sonnet": null,
        "seven_day_mystery": {"utilization": 99, "resets_at": null},
        "extra_usage": {"is_enabled": true, "monthly_limit": 5000, "used_credits": 1234, "utilization": 24.68}
    }"#;

    const CURSOR_PRO: &str = r#"{
        "billingCycleStart": "2026-09-24T03:32:15.933Z",
        "billingCycleEnd": "2026-10-24T03:32:15.933Z",
        "membershipType": "pro",
        "individualUsage": {
            "plan": {"used": 1200, "limit": 2000, "remaining": 800,
                     "autoPercentUsed": 20, "apiPercentUsed": 31, "totalPercentUsed": 27},
            "onDemand": {"enabled": true, "used": 350, "limit": 2000}
        }
    }"#;

    const CURSOR_FREE: &str = r#"{
        "billingCycleEnd": "2026-10-24T03:32:15.933Z",
        "membershipType": "free",
        "individualUsage": {"plan": {"autoPercentUsed": 4, "apiPercentUsed": 19, "breakdown": {"bonus": 19}}}
    }"#;

    /// Both providers' stamps — microseconds and an offset from Anthropic,
    /// milliseconds and `Z` from Cursor — read as GitHub's do.
    #[test]
    fn the_providers_reset_stamps_parse() {
        let secs = crate::pull_request::rfc3339_secs;
        assert_eq!(
            secs("2026-10-05T14:00:00.000000+00:00"),
            Some(1_791_208_800)
        );
        assert_eq!(
            secs("2026-10-05T15:00:00.123456+01:00"),
            Some(1_791_208_800)
        );
        assert_eq!(
            secs("2026-10-24T03:32:15.933Z"),
            secs("2026-10-24T03:32:15Z")
        );
    }

    #[test]
    fn the_keychain_item_hashes_the_pinned_dir() {
        assert_eq!(claude_service(None), "Claude Code-credentials");
        // sha256("/Users/me/.claude-2") starts 4b2a8c6e… — whatever it
        // is, the suffix is its first eight hex digits.
        let named = claude_service(Some(Path::new("/Users/me/.claude-2")));
        let suffix = named.strip_prefix("Claude Code-credentials-").unwrap();
        assert_eq!(suffix.len(), 8);
        assert!(suffix
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(
            claude_service(Some(Path::new("abc"))),
            "Claude Code-credentials-ba7816bf"
        );
        assert_ne!(
            named,
            claude_service(Some(Path::new("/Users/me/.claude-3")))
        );
    }

    #[test]
    fn claude_windows_land_in_session_and_week_with_model_weeks_below() {
        let json: Value = serde_json::from_str(CLAUDE_FIXTURE).unwrap();
        let r = claude_reading(&json, Some("max".into()), 7);
        assert_eq!(r.plan.as_deref(), Some("max"));
        assert_eq!(r.fetched_at, 7);
        let session = r.main.cells[Window::Session.index()].unwrap();
        assert_eq!(session.used_pct, 38.0);
        assert_eq!(session.resets_at, Some(1_791_208_800));
        assert_eq!(r.main.cells[Window::Week.index()].unwrap().used_pct, 59.0);
        assert!(r.main.cells[Window::Day.index()].is_none());
        assert!(r.main.cells[Window::Month.index()].is_none());
        // Opus is there, Sonnet is null, and an unknown week is left out.
        assert_eq!(r.sub.len(), 1);
        assert_eq!(r.sub[0].label, "Opus");
        assert_eq!(r.sub[0].cells[Window::Week.index()].unwrap().used_pct, 12.5);
        assert_eq!(r.extra.as_deref(), Some("extra usage $12.34 of $50.00"));
    }

    #[test]
    fn cursor_reads_the_cycle_total_and_splits_auto_from_api() {
        let json: Value = serde_json::from_str(CURSOR_PRO).unwrap();
        let r = cursor_reading(&json, 0);
        assert_eq!(r.plan.as_deref(), Some("pro"));
        let month = r.main.cells[Window::Month.index()].unwrap();
        assert_eq!(month.used_pct, 27.0);
        assert_eq!(
            month.resets_at,
            crate::pull_request::rfc3339_secs("2026-10-24T03:32:15Z")
        );
        for w in [Window::Session, Window::Day, Window::Week] {
            assert!(r.main.cells[w.index()].is_none());
        }
        let labels: Vec<&str> = r.sub.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Auto", "API"]);
        assert_eq!(r.extra.as_deref(), Some("on-demand $3.50 of $20.00"));
    }

    #[test]
    fn a_free_cursor_plan_takes_the_fuller_pool_never_used_of_limit() {
        let json: Value = serde_json::from_str(CURSOR_FREE).unwrap();
        let r = cursor_reading(&json, 0);
        assert_eq!(r.main.cells[Window::Month.index()].unwrap().used_pct, 19.0);
        assert_eq!(r.extra, None);
    }

    #[test]
    fn claude_credentials_read_the_token_or_say_signed_out() {
        let creds = parse_claude_credentials(
            r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r","expiresAt":1790000000000,"subscriptionType":"max"}}"#,
        )
        .unwrap();
        assert_eq!(creds.token, "tok");
        assert_eq!(creds.expires_at_ms, Some(1_790_000_000_000));
        assert_eq!(creds.plan.as_deref(), Some("max"));
        assert_eq!(parse_claude_credentials("{}"), Err(Problem::SignedOut));
        assert_eq!(
            parse_claude_credentials(r#"{"claudeAiOauth":{"accessToken":""}}"#),
            Err(Problem::SignedOut)
        );
        assert_eq!(
            parse_claude_credentials("not json"),
            Err(Problem::SignedOut)
        );
    }

    #[test]
    fn a_hex_secret_is_decoded_and_plain_text_left_alone() {
        assert_eq!(unhex("7b7d").as_deref(), Some("{}"));
        assert_eq!(unhex("{\"a\":1}"), None);
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn the_cursor_account_id_comes_from_the_cli_config_then_the_token() {
        let config: Value =
            serde_json::from_str(r#"{"authInfo":{"authId":"auth0|user_01ABC","email":"a@b.co"}}"#)
                .unwrap();
        assert_eq!(cursor_account_id(&config).as_deref(), Some("user_01ABC"));
        let numeric: Value = serde_json::from_str(r#"{"authInfo":{"userId":12345}}"#).unwrap();
        assert_eq!(cursor_account_id(&numeric).as_deref(), Some("12345"));
        assert_eq!(cursor_account_id(&serde_json::json!({})), None);

        use base64::Engine as _;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"google-oauth2|user_XYZ"}"#);
        assert_eq!(
            jwt_subject(&format!("h.{payload}.s")).as_deref(),
            Some("user_XYZ")
        );
        assert_eq!(jwt_subject("nope"), None);
    }

    #[test]
    fn cells_say_what_is_left_and_how_long_until_it_resets() {
        let now = 1_000_000;
        assert_eq!(cell_text(None, now), "-");
        let cell = |used_pct, resets_at| Cell {
            used_pct,
            resets_at,
        };
        assert_eq!(cell_text(Some(&cell(38.0, None)), now), "62%");
        assert_eq!(
            cell_text(Some(&cell(38.0, Some(now + 30))), now),
            "62% · 1m"
        );
        assert_eq!(
            cell_text(Some(&cell(0.0, Some(now + 2 * 3600 + 14 * 60))), now),
            "100% · 2h14m"
        );
        assert_eq!(
            cell_text(Some(&cell(59.0, Some(now + 3 * 86_400 + 4 * 3600))), now),
            "41% · 3d4h"
        );
        assert_eq!(
            cell_text(Some(&cell(73.0, Some(now + 19 * 86_400))), now),
            "27% · 19d"
        );
        assert_eq!(
            cell_text(Some(&cell(120.0, Some(now - 5))), now),
            "0% · now"
        );
    }

    #[test]
    fn curl_status_comes_off_the_tail() {
        assert_eq!(split_status(b"{\"a\":1}\n200"), (&b"{\"a\":1}"[..], 200));
        assert_eq!(split_status(b"\n429"), (&b""[..], 429));
        assert_eq!(split_status(b"garbage"), (&b""[..], 0));
        assert_eq!(config_escape(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn an_account_is_asked_once_per_poll_and_r_waits_out_the_retry_gap() {
        let mut usage = Usage::default();
        let now = 10_000_000;
        // Never read: due.
        assert!(usage.due("claude", now, false));
        // Read a minute ago: not due on the beat, due on `r`.
        usage.readings.insert(
            "claude".into(),
            Reading {
                plan: None,
                main: Row::default(),
                sub: Vec::new(),
                extra: None,
                fetched_at: now - 60,
            },
        );
        assert!(!usage.due("claude", now, false));
        assert!(usage.due("claude", now, true));
        // Read POLL ago: due.
        usage.readings.get_mut("claude").unwrap().fetched_at = now - POLL.as_secs() as i64;
        assert!(usage.due("claude", now, false));
        // Just asked: nothing, `r` included, until RETRY_GAP is up.
        usage.asked.insert("claude".into(), Instant::now());
        assert!(!usage.due("claude", now, true));
        // One out already: never twice.
        usage.asked.clear();
        usage.inflight.insert("claude".into());
        assert!(!usage.due("claude", now, true));
    }

    #[test]
    fn the_cache_keeps_numbers_and_never_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CACHE_FILE);
        let json: Value = serde_json::from_str(CLAUDE_FIXTURE).unwrap();
        let mut readings = BTreeMap::new();
        readings.insert(
            "claude".to_string(),
            claude_reading(&json, Some("max".into()), 5),
        );
        write_cache(&path, &readings);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("tok") && !text.contains('@'));
        let back = Usage::load(path);
        assert_eq!(back.readings, readings);
    }

    #[test]
    fn landing_keeps_the_last_reading_beside_why_the_next_failed() {
        let mut app = App::new();
        let reading = Reading {
            plan: Some("pro".into()),
            main: Row::default(),
            sub: Vec::new(),
            extra: None,
            fetched_at: 1,
        };
        app.usage.inflight.insert("claude".into());
        land(
            &mut app,
            Answer {
                id: "claude".into(),
                result: Ok(reading.clone()),
            },
        );
        assert!(app.usage.inflight.is_empty());
        land(
            &mut app,
            Answer {
                id: "claude".into(),
                result: Err(Problem::RateLimited),
            },
        );
        assert_eq!(app.usage.readings.get("claude"), Some(&reading));
        assert_eq!(
            app.usage.problems.get("claude"),
            Some(&Problem::RateLimited)
        );
        // Signed out takes the numbers away: they are no one's now.
        land(
            &mut app,
            Answer {
                id: "claude".into(),
                result: Err(Problem::SignedOut),
            },
        );
        assert!(app.usage.readings.is_empty());
    }

    #[test]
    fn a_landed_change_is_flushed_once_and_a_dropped_account_forgotten() {
        let mut app = App::new();
        app.usage.cache = Some(PathBuf::from("/nowhere/usage.json"));
        let reading = Reading {
            plan: None,
            main: Row::default(),
            sub: Vec::new(),
            extra: None,
            fetched_at: 1,
        };
        land(
            &mut app,
            Answer {
                id: "claude-2".into(),
                result: Ok(reading),
            },
        );
        let (_, readings) = take_flush(&mut app).expect("a landed reading is written");
        assert!(readings.contains_key("claude-2"));
        assert!(
            take_flush(&mut app).is_none(),
            "nothing new, nothing written"
        );
        // The account left config: its reading goes, and that is written.
        app.usage.relist(Vec::new());
        assert!(app.usage.readings.is_empty());
        assert!(take_flush(&mut app).is_some());
    }

    #[test]
    fn the_modal_keys_parse() {
        assert!(keys::ALL.iter().all(|k| k.parses()));
    }
}
