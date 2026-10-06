# PR autofix

Status: built (2026-10-06). Changes from the plan below, made while building:

- **Autofix instructions** (`autofix_preset`, Settings → Review → Autofix, and on the onboarding page). Empty means orion's built-in instructions; an agent preset's name puts that preset's prefix and postfix around the PR context (link, branches, failing checks, note) instead. The preset's agent, model and effort are ignored; the Autofix model and effort rows still decide those. A deleted preset falls back to the built-in instructions, and the footer says so.
- ⌘G in the pull requests modal opens the form as its own overlay, `Overlay::Autofix`, which keeps the PR modal in `AutofixForm::under` and puts it back on Esc or Enter. It is not a `PrForm` variant.
- Ledger matching works on the list row, which carries no check names. A breakage counts as handled when the head commit and the conflicts flag match, and, while checks are failing, the handled fingerprint names failing checks. So a new push, conflicts appearing or clearing, or checks failing after a conflicts-only send each re-arm the prompt.
- The onboarding modal widened to 92 columns so the nine-step strip still fits. The e2e seed now stamps `setup_version` with `orion_tui::onboard::SETUP_VERSION`.
- A stale test (`a_local_pull_request_is_reviewed_from_git_inside_the_pr_modal`) pressed ^G for the diff, which has been ^E since bbb7a85. It now presses ^E.

## What it does

When one of **your** open pull requests gets merge conflicts or failing checks, orion offers to send an agent to fix it. The agent works in the PR's worktree, fixes the conflicts and the failing unit, e2e and other checks, runs them locally until they pass, and then commits and pushes to the PR branch.

- **Setting:** `pr_autofix` is `off` / `ask` / `auto`. It lives in Settings → Review under a new "Autofix" group, and as a new onboarding page that existing users see as "New in Orion setup".
  - `ask` pops the **Autofix modal**. It names the PR and its issues and shows a checklist (Merge conflicts / Unit tests / E2E tests / Other) with the detected ones ticked, plus an optional note. Enter dispatches; Esc says not now.
  - `auto` dispatches straight away with everything detected ticked. It shows a flash and sends a desktop notification. After 3 attempts in a row on one PR without going green, it falls back to `ask`.
- **Agent:** the harness is the default one (the quick-prompt agent, `Config::quick_prompt_harness()`). Two more settings pick its model and effort, and both default to that harness's own defaults. There's no harness picker, to keep the settings short.
- **PR modal:** ⌘G on a PR ("autofix") opens the same checklist as a form inside the modal. This works whatever `pr_autofix` is set to, including `off`.

## How it works today (the facts this plan builds on)

- **All PR polling is in the TUI**, not the daemon.
  - `crates/orion-tui/src/pull_request.rs::list()` runs one GraphQL query (`LIST_QUERY`) per project. Its rows are `OpenPr { number, title, url, is_draft, health: Health, head }`.
  - `Health { conflicts, checks: Checks }`, and `Health::trouble() -> Option<Trouble::{Conflicts, FailingChecks}>`.
  - The list refreshes every 15s for the selected project (`OPEN_PRS_REFRESH`) and every 5min for the others (`OPEN_PRS_SWEEP_REFRESH`). The constants are in `crates/orion-tui/src/event_loop.rs:127-195`.
  - Answers land in `event_loop.rs::note_open_prs_answer` (~:1424).
- **Per-check detail** (names, workflow, state, log URL) exists only in `PrDetail.checks: Vec<PrCheck>`.
  - It is fetched on demand by `pull_request::detail(dir, number)`.
  - It lands in `event_loop.rs::land_pr_detail` (~:1817) through the `detail_tx` channel (set up ~:338-347).
  - `PrCheck { name, workflow, state: CheckState::{Failed, Running, Passed, Skipped}, word, started, completed, url }`.
  - `PrDetail` also has `base`, `head`, `head_sha` and `health`.
- **There is no unit vs e2e distinction** anywhere. This plan classifies checks by name.
- **There is no PR-health transition detection.** The only transition orion detects is "merged" (`land_pull_request` ~:1308), which drives an animation.
- **Unsolicited modal precedent:** `ServerEvent::FilesOpened` → `file_tabs::open` sets `app.overlay` directly (`file_tabs.rs:293`). Toasts are footer flashes (`crate::flash::Flash`). Desktop notifications go through `event_loop/alerts.rs::notify_desktop`, gated by `App::may_notify_desktop()`.
- **Launching a PR session with a first prompt** needs no protocol change:
  - Build `app::AgentLaunchDraft { pr: Some(PrLaunch::of(&open_pr)), starting_prompt: Some(..), name, follow, focus_pane, ..AgentLaunchDraft::new(root_worktree, kind, model, effort) }`.
  - Call `event_loop::create_agent(app, draft, out)` (~:10661). It sends `ClientRequest::CreatePrAgent`.
  - The daemon's `pr_worktree` (`crates/orion-daemon/src/registry.rs:841`) reuses or creates the worktree on `head`.
  - The PR modal's own launch is the reference: `quick_prompt::pr_launch_for` (`quick_prompt.rs:759`) and `pr_modal.rs:622`.
- **Settings template:** `pr_draft` (commit for 2026-10-04) touches these places:
  - `SettingKind` variant (`config.rs:529`)
  - `added_on` (`:666`)
  - spec in the Review tab (`:965`)
  - field (`:1551`) and default (`:1910`)
  - `value_label` (`:3226`) and `cycle_kind` (`:3417`)
  - test (`:4873`)
  - `docs/configuration.md:105`
- **Model/effort choices:** `config.rs::model_choices_in` (:205), `effort_choices_in` (:237), `fit_effort_in` (:310), `default_model(kind)` (:2376), `default_effort(kind)` (:2392).
- **Onboarding "what's new":**
  - `onboard.rs` `Page` enum (:41), with `added()` (:62), `offered()` (:71), `label()` (:79) and `pages()` (:97).
  - `SETUP_VERSION = 2` (:57). `pending()` (:134) shows only pages with `added() > setup_version`.
  - Template pages: Worktrees and Terminal. They use `WORKTREE_ROWS` / `TERMINAL_ROWS` (:151/:155), `setting_rows` (:166), `settings_page` (:1174), `page_body` (:827), `page_rows` (:1622), `hints` (:384), `explanation` (:491), `activate` (:1654), `toggle` (:1856) and `is_switch` (:456).
  - Test template: `an_older_setup_opens_on_whats_new` (:1911).
- **PR modal verb template:** ⌘D ready/draft (commit `bbb7a85`) for a plain verb, and ⌘X merge (`MergeForm`, `pr_actions.rs:764-835`, `draw_merge` :2185) for a form with `[x]` rows.
- **Free ⌘ letters:** none in the PR modal's own verbs clash with ⌘G. ⌘G is only used globally (`keymap.rs:805`) and in the diff viewer (`ui/diff_view.rs:52`).
  - Check that the PR modal swallows ⌘G before the global action does. Compare `modal_takes_cmd_w()`, `event_loop.rs:6378`.
  - If ⌘G is awkward, use ⌘B instead. Either way, add the chord to `ghostty_config.rs::MODAL_KEYS` (:147-160).
- **Shortcut conventions:** use ⌘ chords with the same-letter Ctrl twin (`Key::new(&["cmd+g","ctrl+g"], "autofix")`). One shortcut per action. Avoid ⌘T/Q/V/H/M.

## Design decisions

1. **Only PRs you authored.**
   - The list covers every open PR in the repo, including teammates'. Add `viewerDidAuthor` to `LIST_QUERY` and keep only those.
   - Also add `headRefOid`. That lets a new push re-arm the prompt and lets a handled failure stay quiet.
   - Both are scalars on the PR node. They don't touch `statusCheckRollup` contexts (the #106 504 problem).
2. **Wait for checks to settle before acting on failures.**
   - GitHub's rollup turns `FAILURE` as soon as one check fails, even while e2e is still running. On a failing edge, fetch the detail. If any check is `Running`, fetch it again every 60s until none are, or until 20 minutes pass. Then act, so one agent gets the whole picture.
   - **Conflicts on their own act at once.**
3. **De-dupe with a fingerprint.**
   - The fingerprint is `head_sha + conflicts + sorted failing check names`.
   - A per-PR ledger stores the last fingerprint that was dispatched or dismissed. Nothing re-prompts until it changes, i.e. a new push or a different set of failures.
   - The ledger is persisted in `pr_cache.rs::Store`, so a restart doesn't re-prompt. A PR that is already broken when orion starts prompts once.
4. **Never stack agents.** If a session named `autofix-<number>` is still alive in that PR's worktree, don't prompt or dispatch. Wait for it to finish.
5. **Don't ambush typing.** The popup only opens when `app.overlay.is_none()`. Otherwise it goes on `app.autofix.queue` and opens when the screen is free. When the window isn't focused, a desktop notification goes out as well.
6. **Classification** uses the lower-cased `workflow + " " + name`:
   - **E2E:** `e2e`, `end-to-end`, `playwright`, `cypress`, `integration`, `smoke`, `acceptance`.
   - **Unit:** `test`, `unit`, `jest`, `vitest`, `spec`, `pytest`, `nextest`.
   - **Other:** everything else (lint, typecheck, build). E2E is checked first, because "e2e tests" also contains "test".
7. **Latency:** the selected project is seen within about 15s, other projects within about 5min (the existing sweep). That's acceptable for v1. Don't add polling.

## Steps

### 1. List query: authorship and head SHA (`crates/orion-tui/src/pull_request.rs`)

- Add `viewerDidAuthor headRefOid` to the node fields of `LIST_QUERY` (~:537).
- Add these to `OpenPr` (~:546):
  ```rust
  /// Whether the signed-in `gh` user opened it — only these are autofixed.
  #[serde(default)]
  pub mine: bool,
  /// The head branch's tip commit (`headRefOid`).
  #[serde(default)]
  pub head_sha: String,
  ```
- Parse them where list nodes become `OpenPr` (next to `checkout_branch`, ~:702). Fix every `OpenPr { .. }` literal in tests (`grep -rn "OpenPr {" crates/orion-tui/src`).
- Unit test: a list node JSON carrying both fields parses them, and a cached `OpenPr` without them still deserialises.

### 2. Config (`crates/orion-tui/src/config.rs`)

- Add `#[derive(Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "lowercase")] pub enum AutofixMode { #[default] Off, Ask, Auto }`.
- Add these fields, with defaults in `impl Default for Config`:
  ```rust
  /// What orion does when one of your pull requests hits merge conflicts
  /// or failing checks: nothing, ask with the Autofix modal, or send the
  /// autofix agent straight away.
  pub pr_autofix: AutofixMode,          // Off
  /// The autofix agent's model; empty = the default harness's default.
  pub autofix_model: String,            // ""
  /// The autofix agent's effort; empty = the default harness's default.
  pub autofix_effort: String,           // ""
  ```
- Add `SettingKind::{PrAutofix, AutofixModel, AutofixEffort}`, each with an `added_on` date of `(2026, 10, 6)`.
- Specs: in the Review tab, add a new group `"Autofix"` straight after the "Pull requests" group:
  - `PrAutofix`, labelled "When a PR breaks", hint "Your pull request hits merge conflicts or failing checks: do nothing, ask (a modal to pick what to fix), or send the autofix agent at once".
  - `AutofixModel`, labelled "Autofix model", hint "Model for the autofix agent, which runs on your default agent".
  - `AutofixEffort`, labelled "Autofix effort".
- `value_label`:
  - `off` / `ask` / `auto`.
  - Model and effort show `default (<resolved>)` when empty, otherwise the value.
- `cycle_kind`:
  - Mode cycles Off → Ask → Auto.
  - Model cycles `"" + model_choices_in(kind)` and effort cycles `"" + effort_choices_in(kind)`, where `kind` comes from `self.quick_prompt_harness()`. Use the existing `cycle_owned` helper.
- Add `pub fn autofix_harness(&self) -> (AgentKind, Option<String>, Option<String>, Option<String>)`. It returns kind, custom, model and effort:
  - kind and custom come from `quick_prompt_harness()`.
  - model is `autofix_model` if it is non-empty and still in `model_choices_in(kind)`, otherwise `default_model(kind)`.
  - effort is the same, using `fit_effort_in`.
  - This means changing the default harness never sends an invalid model.
- Tests:
  - Extend `the_review_tab_holds_the_viewer_and_pull_request_defaults`.
  - Add `autofix_settings_round_trip`.
  - Add `autofix_harness_falls_back_when_model_no_longer_offered`.
  - Run the existing `tabs_cover_every_setting_once_and_rows_match` and `no_setting_ships_in_the_future_of_its_own_code`, which must pass.

### 3. Onboarding page (`crates/orion-tui/src/onboard.rs`)

- Add `Page::Autofix` after `Worktrees`:
  - `added() => 3`, `offered() => true`, `label() => "Autofix"`.
  - Add it to `pages()`.
- Bump `SETUP_VERSION` to `3`.
- Add `AUTOFIX_ROWS = [SettingKind::PrAutofix, SettingKind::AutofixModel, SettingKind::AutofixEffort]` with an arm in `setting_rows`.
- Add `Page::Autofix` to the shared arms in `page_body` (→ `settings_page`), `page_rows`, `hints`, `explanation`, `activate` and `toggle`. Look at how the Terminal page treats a non-bool row and do the same: Enter/Space cycles.
- Intro prose for `settings_page`: "When one of your pull requests hits merge conflicts or failing checks, orion can send an agent to fix it: resolve the conflicts, reproduce the failing unit and e2e tests locally, fix them, and push to the PR. Ask shows you what broke and lets you pick; Auto just goes."
- Add a `Chosen` line to the Ready page's `summary()`.
- Test: copy `an_older_setup_opens_on_whats_new` so that `setup_version = 2` opens on Autofix + Ready only.

### 4. Core module: `crates/orion-tui/src/autofix.rs` (new; add `mod autofix;` in `lib.rs`)

Keep it pure where possible, so the logic is unit-testable without an `App`.

```rust
pub enum Issue { Conflicts, Unit, E2e, Other }        // ORDER const, label(), key()
pub fn classify(check: &PrCheck) -> Issue             // rules in decision 6; never returns Conflicts
pub struct Diagnosis {
    pub conflicts: bool,
    pub unit: Vec<PrCheck>, pub e2e: Vec<PrCheck>, pub other: Vec<PrCheck>, // failed only
    pub running: bool,                                 // any CheckState::Running
}
pub fn diagnose(detail: &PrDetail) -> Diagnosis
impl Diagnosis { pub fn is_empty(&self) -> bool; pub fn fingerprint(&self, head_sha: &str) -> String;
                 pub fn summary(&self) -> String /* "merge conflicts · 2 unit · 1 e2e" */ }

/// Per-PR memory, persisted in pr_cache::Store.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Record { pub handled: String /*fingerprint*/, pub attempts: u8 }

/// Runtime state on App (`app.autofix: autofix::State`).
pub struct State {
    pub ledger: HashMap<String /*url*/, Record>,
    /// PRs in trouble awaiting a settled detail: url → (project, OpenPr, first_seen, next_fetch).
    pub watching: HashMap<String, Watch>,
    /// Ready prompts waiting for a free screen.
    pub queue: VecDeque<AutofixForm>,
}

pub fn prompt(pr: &OpenPr, detail: &PrDetail, picks: &Picks, note: &str) -> String
```

**Prompt text** (`prompt()`). Fill it from `detail.base`, `detail.head` and `pr.number`/`url`/`title`, and include only the ticked sections:

```
You are orion's PR autofixer for {url} (#{n} "{title}"), branch `{head}` into `{base}`.
This worktree is checked out on `{head}`. Fix the issues below, then commit and push to
this branch. Do not open a new pull request, force-push, merge the PR, or change unrelated code.

## Merge conflicts
`git fetch origin && git merge origin/{base}`. Resolve each conflict by understanding
both sides' intent; never drop one side's work silently. Regenerate lockfiles and
generated files instead of hand-merging them. Build, run the tests that cover the
conflicted files, and commit the merge.

## Failing unit tests
{for each: "- {name} ({workflow}) {url}"}
Read the failure logs (`gh pr checks {n}`, then `gh run view <run-id> --log-failed`).
Reproduce each failure locally with this repo's test command, fix the code (change a
test only when this PR intentionally changed the behaviour it checks), and re-run until
they pass.

## Failing e2e tests
{list}
Pull the failing test names out of the logs, run exactly those tests locally with this
repo's e2e command, fix, and re-run until they pass. Run a passing test 3 times to rule
out flakiness. If a test fails only in CI and passes reliably locally, say so rather than
guessing at a fix.

## Other failing checks
{list}   (lint, typecheck, build: reproduce locally, fix, re-run)

## Note from the user
{note}

When everything above passes locally, run the relevant suites once more, commit with a
message that says what you fixed, and `git push`. Then `gh pr checks {n} --watch`; if a
check you were asked to fix fails again, go round once more (at most twice). Finish with
a short summary: what you changed, and anything you could not fix and why.
```

**The form** (`AutofixForm`) is shared by the popup overlay and the PR-modal form:
- Fields:
  - `project: ProjectId`
  - `pr: OpenPr`
  - `detail: PrDetail`
  - `diagnosis: Diagnosis`
  - `picks: [bool; 4]`, pre-ticked from the diagnosis
  - `row`: one of the 4 issue rows or `Note`
  - `note: crate::text_input::TextInput`
  - `notice: Option<String>`, e.g. "tried 3 times"
  - `rows: Vec<(Rect, Row)>`, for the mouse
- Copy the shape of `pr_actions::MergeForm` / `MergeRow`:
  - Use `step(down)` for row movement.
  - Each row draws `[form_label, check(bool), dim note]`. The note shows the failing check names, e.g. "2 failing: test (ubuntu), vitest", or "none detected" in dim when the issue wasn't detected. Rows for undetected issues can still be ticked.
  - The Note row is a one-line `TextInput`.
- Keys: add a `pub mod keys` with a `#[cfg(test)] ALL`.
  - ↑/↓ and Tab move between rows. Space ticks; when the Note row has focus, Space types instead.
  - Enter dispatches. It is refused with a notice if nothing is ticked.
  - Esc is "not now".
- Draw with `form_frame` / `check` from `pr_actions.rs`. Make them `pub(crate)` if they aren't already.

**Dispatch** (`pub fn dispatch(app: &mut App, form: AutofixForm, out: &mut ..)`):
1. Get `(kind, custom, model, effort)` from `Config::load().autofix_harness()`.
2. Set `worktree = app.root_worktree(&form.project)?`.
3. Build `AgentLaunchDraft { name: format!("autofix-{}", pr.number), starting_prompt: Some(prompt(..)), pr: Some(PrLaunch::of(&form.pr)), focus_pane: false, follow: false, ..AgentLaunchDraft::new(worktree, kind, model, effort) }`. Then call `crate::event_loop::create_agent(app, draft, out)`. Match how `quick_launch.rs::draft` (:246) fills the rest.
4. Ledger: `handled = fingerprint`, `attempts += 1`. Remove the PR from `watching`.
5. Flash: `Flash::working(format!("Autofix sent to #{n}: {summary}"))`.

**Dismiss** (Esc): ledger `handled = fingerprint`, and `attempts` is left unchanged.

**Detection hooks:**
- `pub fn note_list(app, project, prs: &[OpenPr])` is called at the end of `note_open_prs_answer` with the new list.
  - Skip everything when `pr_autofix == Off`. Read the mode from the config the app mirrors; see `apply_config`, `event_loop.rs:8407`, and mirror `pr_autofix` onto `App` there.
  - For each `pr` with `pr.mine && pr.trouble().is_some()` that isn't already in `watching`, and whose ledger `handled` doesn't start with `pr.head_sha`: insert it into `watching` and request its detail right away.
  - Also, for each `pr.mine && pr.trouble().is_none() && !Checks::Pending`, reset `attempts = 0`, because the PR went green.
- `pub fn land_detail(app, detail: &PrDetail)` is called from `land_pr_detail` for every detail (cheap map lookup).
  - If the URL is in `watching`, run `diagnose`.
  - If it is empty, unwatch it.
  - If `running` and `!conflicts_only` and less than 20min have passed since `first_seen`, set `next_fetch = now + 60s`.
  - If an `autofix-<n>` session is alive in a worktree on `pr.head`, leave the PR in `watching` and try again at the next fetch.
  - If the fingerprint equals the ledger's `handled`, unwatch it.
  - Otherwise build the form:
    - In `Auto` mode with `attempts < 3`, dispatch.
    - Otherwise (`Ask`, or `Auto` that has hit the cap, which sets the notice "Autofix has tried 3 times without the checks going green"), push to `queue` and send a desktop notification when `app.may_notify_desktop()`.
- `pub fn tick(app)` is called from the `GIT_POLL` arm (`event_loop.rs` ~:462-498).
  - Re-request detail for `watching` entries whose `next_fetch` has passed.
  - If `app.overlay.is_none()` and the queue is non-empty, pop the front into `app.overlay = Some(Overlay::Autofix(form))`.
- **Requesting a detail:** `pr_modal::schedule_detail` follows the modal's cursor, so it can't be reused. Add `pub(crate) fn request_detail(app, project, number)` in `pr_modal.rs` next to it. It spawns `pull_request::detail(dir, number)` onto the same `detail_tx` that `schedule_detail` uses, so the answer lands in `land_pr_detail` as usual. Use the project's root worktree path as `dir`.
- **"Is a session alive":** look in `app.tree.agents` for an agent whose name is `autofix-<n>`, whose worktree's branch is `pr.head`, and whose status is not exited or crashed.

**Desktop notification:** add `pub fn notify_text(title: &str, body: &str)` to `event_loop/alerts.rs`, reusing whatever `notify_desktop` shells out to. Use the title "Autofix" and the body `#{n} {title}: {summary}`.

**Persistence:**
- Add `#[serde(default)] autofix: HashMap<String, autofix::Record>` to `pr_cache.rs::Store`.
- Hydrate `app.autofix.ledger` from it at startup, and flush it with the rest on the git tick (`event_loop.rs` ~:492).
- Drop entries whose URL is no longer in any project's open list when flushing.

### 5. The popup overlay (`crates/orion-tui/src/app.rs`, `ui.rs`, `event_loop.rs`)

- Add `Overlay::Autofix(crate::autofix::AutofixForm)` with a doc comment, at `app.rs:2368`.
- Draw it as a centred modal with the title ` Autofix #{n} {title} ` and the explain line `{project}: {summary}`. Keys go on the bottom border through `hints::modal_block` / `hints::draw_on_border`, the same as the onboarding modal (`onboard.rs:621-691`).
- Route keys: add an arm at the per-overlay routing (`event_loop.rs` ~:7262) that calls `autofix::handle_key`. Enter → `dispatch` and close. Esc → dismiss and close; the next queued prompt opens on the next tick.
- Route mouse the same way `pr_actions::handle_mouse` handles the merge form.
- Add `app.autofix: autofix::State` to `App` and initialise it.

### 6. PR modal verb (`pr_modal.rs`, `pr_actions.rs`)

- Add `keys::AUTOFIX = Key::new(&["cmd+g", "ctrl+g"], "autofix")` and add it to `keys::ALL`.
- Add a match arm in `handle_key` (~:805-820). It opens `PrForm::Autofix(AutofixForm)` for `selected_pr`:
  - If `app.pr_detail[url]` is cached, build the form from it straight away.
  - Otherwise set `Flash::working("Reading checks…")`, call `request_detail`, and open the form when the detail lands. Use a `pending_autofix: Option<String>` URL on `PullRequestsView`, checked in `land_pr_detail`.
- Add `PrForm::Autofix` to every `PrForm` match listed for the with-form variant in `pr_actions.rs`: `ticket` / `saving` / `refused`, `hints`, `handle_key`, `paste`, `handle_mouse`, `write_back` and `draw`. Each one delegates to `autofix.rs`.
- Add hints in both the page branch and the list branch of `hints()` (~:1000, ~:1023).
- Add the chord to `ghostty_config.rs::MODAL_KEYS`. Make sure ⌘G in the PR modal doesn't fire the global ⌘G action (`keymap.rs:805`); follow `modal_takes_cmd_w` (`event_loop.rs:6378`) if it does.
- The PR modal path ignores the ledger and the `pr_autofix` mode. It is an explicit request.

### 7. Docs

- `docs/configuration.md`: add rows for `pr_autofix`, `autofix_model` and `autofix_effort` next to `pr_draft` (:105).
- `docs/keys.md`:
  - Add a Modal-verbs row `| ⌘G | autofix: send an agent to fix conflicts / failing checks | pull requests |`.
  - Mention ⌘G in the pull requests Views row (:253).
  - Add a Views row for the Autofix modal.
- `ARCHITECTURE.md`: add a paragraph on `autofix.rs` next to the pr_actions paragraph (:34). Cover the TUI-side detection, the fingerprint ledger, and dispatch through `CreatePrAgent`.

## Tests

Unit tests in `autofix.rs`:
- `classify` puts "e2e tests (shard 2)" under E2E, "unit tests" and "vitest" under Unit, and "lint" and "typecheck" under Other.
- `diagnose`: only failed checks are listed, and `running` is set when any check is running.
- The fingerprint is stable under check order and changes with `head_sha`.
- `prompt` includes only the ticked sections and the note, and names base and head.
- `note_list` ignores PRs that aren't `mine`, and ignores everything when the mode is `Off`.
- `land_detail` re-arms while checks are running, acts on conflicts-only straight away, and stays quiet when the fingerprint matches the ledger.
- In `Auto` mode, the 4th attempt goes to the queue with the notice instead of dispatching. Going green resets `attempts`.
- The queue opens only when `overlay.is_none()`.
- The form: Enter with nothing ticked is refused, and Esc records `handled` without counting an attempt.

Elsewhere:
- **Config and onboarding:** the tests listed in steps 2 and 3.
- **PR modal:** `hints::assert_hints_from(&hints(view), keys::ALL)` keeps passing. Add a test that ⌘G with a cached detail opens `PrForm::Autofix` with conflicts pre-ticked.
- **E2E** (`crates/orion/tests/e2e_tui.rs`): add `tui_autofix_asks_when_my_pr_conflicts`.
  - Stub `gh` on PATH, as in `tui_issues_are_prefetched_before_the_modal_opens` (:594).
  - The stub answers the GraphQL list with one PR (`viewerDidAuthor: true`, `mergeable: CONFLICTING`) and `gh pr view` with a detail.
  - Seed the config with `{"onboarded": true, "setup_version": 3, "pr_autofix": "ask"}` and add the project.
  - `wait_for_text("Autofix #1")`, then check that "[x] Merge conflicts" is shown. Send Esc and `wait_for_gone`.

## Verification

```sh
cargo test -p orion-tui autofix
cargo test -p orion-tui config
cargo test -p orion-tui onboard
cargo test -p orion-tui pr_modal
cargo test -p orion --test e2e_tui tui_autofix
make ci        # fmt check + lint + full test
```

Manual check on a real repo:
1. Set `pr_autofix` to `ask`.
2. Push a branch whose PR conflicts with `main`. Within about 15s, with that project selected, the Autofix modal pops.
3. Press Enter. An `autofix-<n>` session appears in the PR's worktree and gets to work.
4. Close the session without fixing. No second prompt appears until a new push or a different failure.
5. Press ⌘G on the PR in the pull requests modal. The form opens even with the setting `off`.

## Out of scope for v1

- Detection while the TUI isn't running. Polling lives in the TUI, so moving it into the daemon is a separate job.
- Teammates' PRs.
- Review comments ("changes requested") as an issue type. This would be an easy fifth row later.
- Faster polling for non-selected projects.
