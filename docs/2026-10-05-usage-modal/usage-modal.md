# Usage modal: how much is left on every linked account

## Goal

One key (default `⇧U`, also in the command palette as **Account usage**) opens a modal with a
grid. There is one row per linked account and one column per time window. Each cell shows how
much of that window's allowance is left and when it resets. A provider that has no cap at a
window's granularity shows `-` in that cell.

```
╭─ Account usage ──────────────────────────────────────────────────────────────────────╮
│ ACCOUNT                     SESSION (5h)      DAY     WEEK              MONTH         │
│ Claude (me@home.co)  max    62% · 2h14m       -       41% · Thu 09:00   -             │
│   Opus                      -                 -       88% · Thu 09:00   -             │
│ Work (me@work.co)    pro    ▲ 0% · 38m        -       12% · Mon 17:00   -             │
│ Cursor (me@home.co)  pro    -                 -       -                 73% · 24 Oct  │
│   Auto / API                -                 -       -                 73% / 91%     │
│ Old (me@old.co)             signed out — Enter signs in                               │
│                                                                                       │
│ fetched 2m ago · r refresh · enter details · esc close                                │
╰───────────────────────────────────────────────────────────────────────────────────────╯
```

- Each cell reads `<percent LEFT> · <reset>`. The percentage is remaining, not used, because the
  question is "how much do I have left". The reset is a countdown when it is under 24h and a
  weekday or date when it is further out.
- Cell colours: green when 30% or more is left, yellow from 10% to 30%, red under 10%, and red
  with `▲` at 0% or when a session has just hit the limit.
- Sub-rows (Claude's model-scoped weekly limits, Cursor's Auto/API split) only appear when the
  provider returns them.
- **Day** has no provider today, so every cell in it is `-`. Keep the column anyway: the user
  asked for a standard time grid, and a future provider might fill it.

## What the providers expose (researched 2026-10-05)

### Claude (each Claude account orion knows)

- **Endpoint:** `GET https://api.anthropic.com/api/oauth/usage`. Send the headers
  `Authorization: Bearer <accessToken>`, `anthropic-beta: oauth-2025-04-20` and
  `Accept: application/json`. It is undocumented: it is what Claude Code's own `/usage` reads, and
  community tools (ccstatusline, claude-hud, Claude-Code-Usage-Monitor) use it.
- **Response:**
  `{"five_hour": {"utilization": 23.0, "resets_at": "2026-10-05T14:00:00Z"}, "seven_day": {...},
  "seven_day_opus": {...}|null, "seven_day_sonnet": {...}|null, "extra_usage": {...}}`.
  `utilization` is the percentage used (0–100) and `resets_at` is ISO 8601.
  Map the fields to columns like this:
  - `five_hour` → SESSION
  - `seven_day` → WEEK
  - `seven_day_opus` and `seven_day_sonnet` → WEEK in the sub-rows
  - Ignore any other `seven_day_*` key that has a non-null value, but log it at `debug`.
- **Polling:** the endpoint returns persistent 429s when it is polled every 30–60s
  (anthropics/claude-code#30930). **Never poll it in the background.** Fetch only when the modal
  opens and the cache is older than 5 minutes, or when the user presses `r` (debounced to 60s per
  account). On a 429, keep the last snapshot and mark it `rate-limited · fetched 14m ago`.
- **Token location:** the OAuth blob is `{"claudeAiOauth": {"accessToken", "refreshToken",
  "expiresAt" (epoch ms), "subscriptionType", "rateLimitTier", ...}}`.
  - On macOS it is a generic-password Keychain item:
    - service `Claude Code-credentials` when `CLAUDE_CONFIG_DIR` is unset
    - service `Claude Code-credentials-<first 8 hex of sha256(config dir)>` when it is set
    - account `$USER`
    - Read it with `/usr/bin/security find-generic-password -s <service> -w`. Claude Code writes
      the item through `security` itself, so `security` is on its ACL and the read should not
      prompt.
    - The hashed path is the **expanded** dir. orion expands `~/` before it spawns (`env_pairs`
      in `crates/orion-core/src/harness.rs:391`), so hash
      `HarnessDescriptor::pinned_claude_config_dir()`.
    - For built-in Claude with no pinned dir, follow the same rule as `Record::of`: if orion's own
      `CLAUDE_CONFIG_DIR` env var is set, hash it. Otherwise use the unsuffixed service.
  - On Linux and other systems: `<config dir>/.credentials.json`, where the config dir defaults
    to `~/.claude`.
- **Never refresh the token yourself.** Refresh tokens rotate, so a refresh from orion would sign
  that Claude Code login out. When `expiresAt` has passed, show the row as
  `token expired — open a session on this account` and keep the last cached snapshot greyed out.
  Claude Code refreshes the token the next time a session runs on that account.
- **Accounts on the same login share limits.** `claude_accounts::same_account_groups` already
  finds config dirs signed in as the same email. Fetch once per email and show one row, with the
  other dirs named as `also: claude-3` in the details pane.
- **Plan label:** `subscriptionType` from the credential blob (`pro`, `max`, `team`, …), in the
  small column after the name.
- **Already known locally:** a session that stopped on `LimitReason::RateLimit`
  (`crates/orion-core/src/entities.rs:286`) means its account's SESSION or WEEK window is
  exhausted right now. Use it to mark the row `▲` before or without a fetch (see step 7).
- **Terms note:** using the OAuth token outside Claude Code is unsupported (jarrodwatts/claude-hud#287).
  The call is read-only, goes to Anthropic's own host and uses the user's own token. Keep it
  behind a config switch (`usage_claude`, default on) so it can be turned off.

### Cursor (the built-in `cursor` harness; orion has one Cursor login, not several)

- **Endpoint:** `GET https://cursor.com/api/usage-summary` with `Accept: application/json` and
  `Cookie: WorkosCursorSessionToken=<accountId>%3A%3A<accessToken>` (the two values joined by a
  URL-encoded `::`). A bare Bearer token gets a 401; the cookie has to carry both values.
- **Response:**
  `{"billingCycleStart", "billingCycleEnd", "membershipType", "individualUsage": {"plan":
  {"used", "limit", "remaining", "autoPercentUsed", "apiPercentUsed", "totalPercentUsed",
  "breakdown"}, "onDemand": {"enabled", "used", "limit"}}}`.
  Map the fields to the grid like this:
  - MONTH: `100 - totalPercentUsed`. If that field is missing, use
    `100 - max(autoPercentUsed, apiPercentUsed)`. Resets at `billingCycleEnd`.
  - Sub-row `Auto / API`: both percentages, shown as remaining.
  - Details pane: on-demand spend (`onDemand.used` and `onDemand.limit` are in cents).
  - Do not compute anything from `used`/`limit`. Free plans report their allowance under
    `breakdown.bonus`, so those two fields mislead.
- **Token location:**
  - On macOS: Keychain service `cursor-access-token`, account `cursor-user`, written by
    `cursor-agent login`. `cursor-agent` may not have put `security` on the item's ACL, so the
    first read may show a macOS "allow" prompt.
  - On Linux: probably `~/.config/cursor/auth.json`, key `accessToken`. **Not verified; check
    this on a Linux machine.**
  - Fallback: the Cursor IDE's `state.vscdb` key `cursorAuth/accessToken`. Read it through the
    `sqlite3` CLI, because orion-tui has no rusqlite.
- **Account id**, first match wins:
  1. `~/.cursor/cli-config.json` → `authInfo.authId`
  2. `~/.cursor/cli-config.json` → `authInfo.userId` (it may be a JSON number)
  3. The JWT's `sub` claim, taking the part after the last `|`
- **Email** for the row label: `authInfo.email` in `cli-config.json` (verify the key name).
- Cursor has no 5-hour or weekly caps, so its SESSION, DAY and WEEK cells are always `-`.

### Out of scope, noted for later

- **Codex:** its rollout logs (`~/.codex/sessions/**/rollout-*.jsonl`, `token_count` events)
  record `rate_limits.primary` (5h) and `rate_limits.secondary` (weekly) locally, with no network
  call. It would make a cheap third provider once the module exists.
- **Several Cursor logins:** orion has no Cursor-account concept to hang them on.
- **Claude's statusline `rate_limits` field** (v2.1.80+): orion does not own the user's
  statusline, so it cannot read this without taking the statusline over.

## Design inside orion

The closest existing pieces to copy:

| Need | Copy from |
|---|---|
| A read-only data modal with rows, j/k, Enter, Esc | `Overlay::Metrics` / `MetricsView` (`crates/orion-tui/src/app.rs:2207`), rendered at `crates/orion-tui/src/ui.rs:1823`, keys at `crates/orion-tui/src/event_loop.rs:6974`, key table `ui::metrics_keys` (`crates/orion-tui/src/ui.rs:3065`) |
| A fetch off the loop that lands through a channel | Linear: `linear_tx` / `linear_rx` (`crates/orion-tui/src/event_loop.rs:365`, landed at `:744` via `crate::linear::land_answer`) |
| HTTP that keeps secrets off argv | `linear::graphql` (`crates/orion-tui/src/linear.rs:1630`): `tokio::process::Command::new("curl")` with headers sent as `--config -` on stdin, `--max-time`, and `DeleteOnDrop` for temp files |
| Enumerating Claude accounts, their labels and emails | `Config::load().harness_registry()` filtered by `HarnessDescriptor::is_claude_account()`. Labels from `claude_accounts::label` / `name_of`, email from `claude_accounts::email_of`, shared logins from `claude_accounts::same_account_groups` |
| A rebindable hotkey plus a palette entry | `Action::Metrics` and its `ActionSpec` (`crates/orion-tui/src/keymap.rs:787`). Also add it to the "actions that need no selection" list at `crates/orion-tui/src/event_loop.rs:4780` |
| Data-dir cache | `orion_core::paths::data_dir()` |

There is no new crate dependency, except possibly the hash:
- **sha256:** run `/usr/bin/shasum -a 256` once per account. Only macOS needs it, and it is
  always present there. Do not add a crate for one hash.
- **ISO 8601:** write a small `parse_rfc3339_utc(&str) -> Option<i64>` that handles `Z` and
  `±hh:mm` offsets plus optional fractional seconds. Unit-test it. orion has no time crate and
  this is all it needs.

## Steps

### 1. Data model and provider fetchers: new file `crates/orion-tui/src/usage.rs`

Start with a module doc in the house style (see the top of `claude_accounts.rs`). It should say
what is fetched, from where, why fetches never poll, and why tokens are never refreshed.

```rust
pub enum Window { Session, Day, Week, Month }          // the grid's columns, in order
pub struct Cell { pub used_pct: f64, pub resets_at: Option<i64> /* epoch s */ }
pub struct Line { pub label: String, pub cells: [Option<Cell>; 4] } // None → "-"
pub enum Provider { Claude, Cursor }
pub struct AccountUsage {
    pub key: String,             // harness id ("claude", "claude-2", "cursor")
    pub provider: Provider,
    pub label: String,           // "Work (me@work.co)"
    pub plan: Option<String>,    // "max", "pro"
    pub main: Line,
    pub sub: Vec<Line>,          // Opus / Sonnet, Auto / API
    pub also: Vec<String>,       // other ids on the same login
    pub fetched_at: Option<i64>,
    pub state: FetchState,
}
pub enum FetchState { Fresh, Loading, Stale(String) /* why */, SignedOut, Expired, Error(String) }
pub struct UsageAnswer { pub key: String, pub result: Result<AccountUsage, FetchError> }
```

Write these functions:

- `fn claude_keychain_service(dir: Option<&Path>) -> String`. Pure; take the hash as an argument
  so tests can feed it.
- `async fn claude_credentials(entry: &HarnessDescriptor) -> Result<ClaudeCreds, FetchError>`.
  Read the token from the Keychain on macOS (`cfg!(target_os = "macos")`) and from the file
  elsewhere. Parse only `accessToken`, `expiresAt` and `subscriptionType`, and never log the
  token.
- `async fn fetch_claude(creds) -> Result<ClaudeUsageRaw, FetchError>`. Use curl with the headers
  sent through `--config -` on stdin, as `linear::graphql` does. Add `-w '%{http_code}'` so a 429
  or 401 becomes `FetchError::RateLimited` or `FetchError::Unauthorized`.
- `fn claude_lines(raw) -> (Line, Vec<Line>)`. Pure.
- `async fn cursor_credentials() -> Result<CursorCreds, FetchError>` and
  `async fn fetch_cursor(creds)`. Send the cookie through `--config -` too, never on argv.
- `fn cursor_lines(raw) -> (Line, Vec<Line>)`. Pure.
- `fn parse_rfc3339_utc`.
- `fn remaining_label(cell, now) -> String`, which produces `62% · 2h14m`, `41% · Thu 09:00` or
  `73% · 24 Oct`. Reuse the countdown style of `hosts::ago_label` (`crates/orion-tui/src/hosts.rs:75`).

### 2. Cache: `data_dir()/usage-cache.json`

- Store `{ "<key>": { "fetched_at": <epoch s>, "usage": <AccountUsage minus state> } }`. Load it
  when the modal opens so the grid paints immediately, then fetch whatever is older than
  `FRESH = 5 min`.
- The file holds percentages and reset times only. Never write tokens, emails or cookies to it;
  labels are recomputed from live config.

### 3. Fetch orchestration (`usage.rs`)

- `pub fn request(app: &mut App, force: bool)`:
  1. Build the account list: the Claude accounts first, in registry order, deduped by email
     through `same_account_groups`, then Cursor if the `cursor` harness is enabled and has
     credentials.
  2. For each account that is stale, or every account when `force` is set: skip it if it was
     fetched under 60s ago, otherwise mark it `Loading` and `tokio::spawn` the fetch.
  3. Send the result on `app.usage_tx`.
- `pub fn land_answer(app: &mut App, answer: UsageAnswer)`: update the view if it is open, write
  the cache, and keep the previous snapshot when the new state is `Stale`, `RateLimited` or
  `Expired`.
- Run fetches in parallel across accounts and at most one at a time per account (an `inflight`
  set on the view).

### 4. App and overlay wiring

- `crates/orion-tui/src/app.rs`:
  - Add `pub struct UsageView { rows: Vec<AccountUsage>, selected, scroll, area: Rect, list_area: Rect, inflight: BTreeSet<String>, detail: bool }`.
  - Add `Overlay::Usage(UsageView)`.
  - Add `pub usage_tx: Option<UnboundedSender<usage::UsageAnswer>>` on `App`.
- `crates/orion-tui/src/event_loop.rs`:
  - Create the channel next to Linear's (around line 365) and add a `usage_rx.recv()` arm next to
    `linear_rx` (around line 744).
  - Add `fn open_usage(app)`: build the view from the cache and call `usage::request(app, false)`.
  - Add `Action::Usage => open_usage(app)` in `run_action` (next to line 3913), and add
    `Action::Usage` to the no-selection list around line 4788.
  - Add the overlay's key handler next to `Overlay::Metrics` (line 6974):
    - j/k and ↑/↓ move the selection.
    - `r` calls `usage::request(app, true)`.
    - `Enter` on a signed-out or expired Claude row calls `claude_accounts::sign_in(app, id, email)`.
    - `Enter` on any other row toggles the details pane.
    - `Esc`, `q` and `⇧U` close the modal.
  - Mouse: clicks outside close it, as `overlay_close.rs` already does for Metrics. Add
    `Overlay::Usage(v) => v.area` at `crates/orion-tui/src/overlay_close.rs:37` and the variant to
    the list at `:101`.
  - Add `Some(Overlay::Usage(_)) => "Usage"` in `crates/orion-tui/src/perf.rs:162`.
  - Fix every exhaustive `match` on `Overlay` that the compiler flags. `cargo build` lists them.
- `crates/orion-tui/src/ui.rs`:
  - Add an `Overlay::Usage(view)` render arm that calls a new `fn draw_usage(f, area, view, now)`.
    Put it in `crates/orion-tui/src/ui/usage_view.rs` if it runs past about 150 lines, because
    `ui.rs` is already 5k lines.
  - Use fixed column widths:
    - ACCOUNT: flexible, truncated with `…`
    - plan: 5
    - SESSION, DAY, WEEK, MONTH: 16 each
  - Below about 90 columns of width, drop DAY first, then the plan column.
  - Write `view.area` and `view.list_area` back for hit-testing, as Metrics does.
  - Add `pub(crate) mod usage_keys` next to `metrics_keys` (line 3065) with `REFRESH`, `OPEN` and
    `CLOSE`, so the footer hints and the handler share one table (`crate::hints::Key`).

### 5. Hotkey

- In `crates/orion-tui/src/keymap.rs`, add `Action::Usage` after `Metrics` and an `ActionSpec`
  with:
  - `id: "usage"`
  - `label: "Account usage"`
  - `hint: "How much is left on each Claude and Cursor account — session, day, week, month"`
  - `group: "GENERAL"`
  - `scope: Scope::Global`
  - `defaults: &["shift+u"]`
- `⇧U` is free: bare `u` must stay unbound (test `stray_letters_quit_close_and_archive_nothing`),
  and `⌘⇧U`/`^U` are unarchive.
- Check the Ghostty passthrough in `crates/orion-tui/src/ghostty_config.rs`. `shift+u` is not a
  ⌘ chord, so it should need no entry, but confirm with the existing reachability test.

### 6. Config switches (`crates/orion-tui/src/config.rs`)

- Add `usage_claude: bool` (default `true`) and `usage_cursor: bool` (default `true`). When one
  is off, that provider's rows are hidden and it is never fetched.
- Follow how the existing bool settings are declared, defaulted and listed in the Settings
  overlay, and add a row to the table in `docs/configuration.md`.

### 7. Use what the daemon already knows (small, optional)

- In `open_usage` and on each frame, check `app.agents` for an agent with a
  `usage_limit.reason == LimitReason::RateLimit` whose harness id matches a row. If there is one,
  draw that row's SESSION cell as `▲ limit hit` until a fresh fetch says otherwise. This needs no
  network and covers the time when the endpoint is 429ing.
- Find where the card reads `usage_limit` (grep `usage_limit` in `crates/orion-tui/src/ui.rs`)
  for the field path.

### 8. Docs

- `docs/keys.md`: add `⇧U` to "The grid" table (around line 113): what each cell means, `r`,
  Enter, and that the modal never polls.
- `docs/configuration.md`: add the two switches.
- `docs/sessions.md`: in "Usage limits and a second account", add one line pointing at the
  usage modal.

## Tests (unit tests in `usage.rs`, plus event-loop tests next to the Metrics ones around `crates/orion-tui/src/event_loop.rs:28170`)

- `claude_keychain_service`:
  - no dir → `Claude Code-credentials`
  - a dir → `Claude Code-credentials-` plus 8 hex characters, given a known sha256
  - a `~/` dir in config hashes the expanded path
- `claude_lines` on a captured fixture:
  - with the Opus/Sonnet keys null
  - with them populated
  - with an unknown `seven_day_foo` key, which must be ignored
- `cursor_lines` on fixtures:
  - a pro plan with `totalPercentUsed`
  - a free plan with only `autoPercentUsed`/`apiPercentUsed` and `breakdown.bonus`, which gives
    MONTH = `100 - max`
- `parse_rfc3339_utc`: `Z`, `+01:00`, fractional seconds, and garbage → None.
- `remaining_label`: under 1h, under 24h, under 7d and over 7d. `None` reset → `62%`.
- Credential parsing:
  - an expired `expiresAt` → `FetchState::Expired`, with no fetch attempted
  - a missing `claudeAiOauth` → `SignedOut`
- Cache round trip: no token, email or cookie text appears in the written JSON.
- Event loop:
  - `run_action(Action::Usage)` opens `Overlay::Usage`
  - `Esc` closes it
  - `r` twice within 60s requests one fetch
  - a landed `RateLimited` answer keeps the previous cells and sets `Stale`
- Keymap: `shift+u` resolves to `Action::Usage` in `Scope::Global`, and bare `u` is still unbound.

Fetchers that shell out (`security`, `curl`) sit behind one small async trait or function
pointer, `Runner`, so tests inject canned outputs and never touch the Keychain or the network.

## Manual verification before building (run these yourself; they touch your credentials)

These confirm the endpoint shapes on your accounts. Each pipes the token straight into curl's
stdin config, so it never lands in your shell history or on screen.

```sh
# Claude, default account
security find-generic-password -s "Claude Code-credentials" -w \
  | python3 -c 'import json,sys;print("header = \"Authorization: Bearer "+json.load(sys.stdin)["claudeAiOauth"]["accessToken"]+"\"")' \
  | curl -sS --config - -H 'anthropic-beta: oauth-2025-04-20' https://api.anthropic.com/api/oauth/usage | python3 -m json.tool

# Claude, an extra account: list the suffixed services that exist
security dump-keychain | grep -o '"Claude Code-credentials[^"]*"' | sort -u
printf %s "$HOME/.claude-2" | shasum -a 256 | cut -c1-8   # should match one suffix above

# Cursor
ID=$(python3 -c 'import json,os;a=json.load(open(os.path.expanduser("~/.cursor/cli-config.json")))["authInfo"];print(a.get("authId") or a.get("userId"))')
security find-generic-password -s cursor-access-token -a cursor-user -w \
  | sed "s/^/header = \"Cookie: WorkosCursorSessionToken=${ID}%3A%3A/; s/\$/\"/" \
  | curl -sS --config - -H 'Accept: application/json' https://cursor.com/api/usage-summary | python3 -m json.tool
```

Save one redacted response from each provider as the test fixtures:
- `crates/orion-tui/src/usage/fixtures/claude.json`
- `crates/orion-tui/src/usage/fixtures/cursor.json`

Load them with `include_str!`.

## Build and check

```sh
cargo build -p orion-tui
cargo test -p orion-tui usage
cargo test -p orion-tui keymap
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Then run orion and check the modal by hand:

1. Press `⇧U`. Each Claude account and Cursor should appear.
2. Press `r` twice quickly. Only one fetch should go out per account; check with
   `RUST_LOG=orion_tui::usage=debug` in the log dir.
3. Sign one Claude account out from the Claude accounts page. Its row should read `signed out`,
   and Enter on it should start a sign-in.
4. Narrow the terminal below 90 columns. DAY, then the plan column, should drop.

## Sources

- Claude OAuth usage endpoint and response: Maciek-roboblog/Claude-Code-Usage-Monitor#202, anthropics/claude-code#99231
- 429s from polling: anthropics/claude-code#30930
- Keychain service naming with `CLAUDE_CONFIG_DIR`: sirmalloc/ccstatusline#573, robinebers/openusage#423
- Cursor `usage-summary` shape and auth: gist dmwyatt/1e9359b1862e7cbfe1e754fe4c8db764, noamsto/tmux-og#778
- Statusline `rate_limits` (not used here): anthropics/claude-code#45133
