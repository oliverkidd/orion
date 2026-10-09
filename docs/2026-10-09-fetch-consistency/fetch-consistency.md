# Fetch consistency: one answer per fact, newest asked wins

Status: built (2026-10-09). Changes from the plan below, made while building:

- **Stamps never tie by accident.** `fetch::now()` never hands out the same instant twice, and tickets are stamped with it, so the only answers that tie are ones sharing an ask on purpose (a recheck stamped with its list's time).
- **Phase 2, item 10:** a lookup carries the branch the checkout was on when it was asked, and is dropped if the checkout is on another branch by the time it lands. The lookup query has no head field, and a fork's or renamed branch would falsely mismatch.
- **Phase 2, extra:** titles are read from `PrStore` too, wherever two surfaces name the same pull request, so a rename shows on both at once. The store forgets a pull request no row names 10 min after its newest answer (`RETIRED_KEEP`).
- **Phase 2, item 6:** a recheck cut short keeps the known checks verdict, and when there is none it falls back to GitHub's own rollup word, so a busy failing pull request seen for the first time still goes red.
- **Phase 2, final pass:** a checkout's lookup that says merged or closed takes the row out of the open list at once, as a page does.
- **Phase 3, item 6:** links track attachment per issue (`attached_ids`), so relinking a branch keeps the issues it had. A link Linear refused three times reads `couldn't attach` in the worktree picker.
- **Phase 3, item 9:** the "more on Linear" note sits at the top of a tab, where the existing "showing the N most recently updated" note was.
- **Phase 4, item 2:** a save that comes back after you left its form keeps your text as a draft that opens with that issue's form next time.
- **Phase 4, item 7:** the commit list takes a cached base but asks again past a cached "no base", once.

Known limits, left as they are:

- When a page's own fold of the checks and the list's tally disagree, the store's verdict follows whichever was asked last.
- Undo compares values. If the daemon sends a field back to exactly the value a keypress showed, a refusal still puts the old value back.
- `fetch.rs` API hardening (`land` returning the accepted stamp, private ticket fields) is deferred; see `docs/cubic-deferred/orion-tui.md`.
- `tui_diff_steps_through_a_branch_one_commit_at_a_time` in `crates/orion/tests/e2e_tui.rs` fails on `main` (3526272) as well; it is not caused by this work.

Branch `fetch-consistency`, worktree `~/Documents/Projects/orion-worktrees/fetch-consistency`. All paths below are relative to that root; TUI code is under `crates/orion-tui/src/`.

## The problem

orion fetches the same facts with several independent calls and keeps copies in several stores. Screens read different copies, so they disagree. Seen live: the PULL REQUESTS MODAL's list said `ready` for riplo-os #1427 while its right pane said `● Conflicts`.

An audit of every fetch in the TUI found about 30 cases. Almost all come from four root causes:

| | Root cause | Example |
|---|---|---|
| **A** | **Copies with no one owner.** The same pull request lives in `app.open_prs`, `app.pr_detail`, `app.pull_requests`, autofix's watch and Linear's attachment metadata, and each screen picks its own preferred copy. | Modal list vs right pane; launcher band lags the list by up to 5 min; Linear's PR badge |
| **B** | **Answers are stamped when they land, not when they were asked.** A slow older fetch overwrites a newer one, and a "fetch again now" is dropped while a fetch is already in flight. | Linear ⌘S reverted by an in-flight list; issue edit reverted; a branch switch brings back the old branch's PR |
| **C** | **Unknown overwrites known.** | `mergeable: UNKNOWN` reads as clean; a failed git read blanks ⇡⇣; the slim PR query wipes meta |
| **D** | **Undo restores a whole old snapshot.** | A refused rename puts back an old status; an issue save sends a stale body |

## The rules (every phase follows these)

1. **One store per kind of entity, keyed by its id** (PR URL, Linear issue id, GitHub issue URL, worktree id). Fetchers write into it. Screens read from it. A screen never keeps its own copy of a fact it draws.
2. **Each fact carries when it was asked.** An answer only replaces a fact if it was asked later than the one already stored. Cached-from-disk facts lose to any live answer.
3. **Unknown never overwrites known.** A field the query did not ask for, a value GitHub has not computed (`UNKNOWN`), a failed fetch and a truncated page all mean "no answer". The last known value stays.
4. **Asking for fresh data never gets dropped.** If a fetch is already in flight, the request is remembered and a new fetch starts as soon as the current one lands.
5. **Undo puts back only the fields the action changed**, and only when nothing newer has replaced them since.

## How the work is split

| Wave | Builder | Phase | Depends on |
|---|---|---|---|
| 1 | A | Phase 1: shared primitives (`fetch.rs`) | — |
| 2 | B | Phase 2: `PrStore` and every pull request finding | Phase 1 |
| 2 | C | Phase 3: Linear | Phase 1 |
| 2 | D | Phase 4: GitHub issues, git readers, accounts, optimistic undo | Phase 1 |
| 3 | coordinator | Phase 5: final pass and polish | all |

Wave 2's three builders work at the same time in the same worktree. **File ownership** is listed in each phase. Outside your files, change only what your phase names, and keep those edits small. `crates/orion-tui/src/app.rs` and `crates/orion-tui/src/event_loop.rs` are shared: add your own fields and functions, and do not reorder or reformat other code.

### Rules for every builder

- Work only in this worktree. Don't commit, push, stash or switch branches; the coordinator commits.
- **Don't run `cargo fmt`** while other builders are working (it rewrites their files under them). The coordinator formats at the end.
- Another builder's half-finished edit can break the build. If `cargo check` fails in code you don't own, wait a minute and retry. Don't fix it yourself; tell the coordinator if it persists.
- **Don't write new tests.** Keep the existing suite compiling and passing: update existing tests your change legitimately breaks. In your report, list the behaviours you think deserve a test; the coordinator triages them at commit time and may ask you to write some.
- Match the surrounding code: its comment style (doc comments that explain *why*, in plain sentences), naming and idiom. Read a neighbouring function before writing a new one.
- Touch nothing in `crates/orion-core`, `crates/orion-daemon`, `vendor/`, `Cargo.toml` or `Cargo.lock`. This work is TUI-only, so it ships as a patch release (see `CLAUDE.md`). If you think a daemon change is needed, stop and ask.
- Verification while working: `cargo check -p orion-tui --all-targets`, then `cargo test -p orion-tui <filter>` for your area. Before reporting: `cargo test -p orion-tui` and `cargo clippy -p orion-tui --all-targets` (no new warnings in your files).
- Report: what you changed (file and function), each finding below marked done / partly / not done (with why), anything you changed outside your files, the tests you'd suggest, and anything you were unsure of.

---

## Phase 1: shared primitives (builder A)

**Owns:** new file `crates/orion-tui/src/fetch.rs`, and its `mod` line in `crates/orion-tui/src/lib.rs`. Nothing else.

Write a small, dependency-free module that every later phase uses. Suggested API (refine names to fit the codebase, but keep the behaviour):

```rust
/// When an answer was asked for. `Cached` came off disk and loses to any
/// live answer; `At` is the instant the fetch was started (not landed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Asked { Cached, At(std::time::Instant) }

/// One fact and when it was asked. `observe` keeps the newest-asked known
/// value: `None` (unknown, not asked, failed) never replaces a value, and an
/// answer asked before the stored one is dropped. Returns whether it changed.
pub struct Known<T> { value: Option<T>, asked: Option<Asked> }
impl<T: PartialEq> Known<T> {
    pub fn observe(&mut self, value: Option<T>, asked: Asked) -> bool;
    pub fn get(&self) -> Option<&T>;
    pub fn asked(&self) -> Option<Asked>;
}

/// Fetches in flight, one per key. `begin` hands back a ticket, or None while
/// one is already running (and the caller then calls `want_fresh`).
/// `want_fresh` marks the running fetch as owing another one, so a refresh
/// asked mid-flight is never lost. `land` returns None for a ticket that was
/// cancelled or superseded (drop the answer), else whether a fresh fetch is
/// owed (start one now). `cancel` forgets a flight so its answer is dropped
/// (e.g. the branch it was asked about has been switched away).
pub struct Flights<K> { /* HashMap<K, Flight { generation, asked, owed }>, next generation */ }
pub struct Ticket<K> { pub key: K, pub asked: std::time::Instant, generation: u64 }
impl<K: Eq + std::hash::Hash + Clone> Flights<K> {
    pub fn begin(&mut self, key: K, now: std::time::Instant) -> Option<Ticket<K>>;
    pub fn want_fresh(&mut self, key: &K) -> bool; // true: one is in flight, now owed
    pub fn land(&mut self, ticket: &Ticket<K>) -> Option<Landed>; // Landed { owed: bool }
    pub fn cancel(&mut self, key: &K);
    pub fn in_flight(&self, key: &K) -> bool;
}
```

`Ticket` must be `Send + Clone` so it can ride the existing `tokio` channels with the answer. Keep the module free of `App`. Write its doc comment as the canonical statement of rules 2–4 above, so later code can point at it.

**Done when:** `cargo check -p orion-tui --all-targets` passes and the module is used nowhere yet (waves 2 adopt it).

---

## Phase 2: `PrStore` and every pull request finding (builder B)

**Owns:** `pull_request.rs`, new `pr_store.rs`, `pr_modal.rs`, `pr_preview.rs`, `pr_row.rs`, `pr_cache.rs`, `pr_actions.rs`, `autofix.rs`, the PR rows in `palette.rs`, `launcher.rs` (`row_pr`, `card_pull_request`), `ui/footer.rs` PR text, `ui.rs` PR drawing; in `event_loop.rs` the PR functions (`land_pull_request`, `note_open_prs_answer`, `stale_details_behind`, `row_status`, `adopt_pr_state`, `drop_retired_pr`, `reask_checkouts_whose_pr_left`, `schedule_pr_detail`, `land_pr_detail`, `refetch_pr_detail`, and where lookups/lists/details are spawned); the PR fields on `App` in `app.rs`; in `linear.rs` only `work_of` and `IssuePr` (the PR badge). Also the lookup cancellation lines in `branch_switch.rs` (~1261–1281) and `event_loop.rs::apply_upsert` (~13657–13671).

### How it works today

- `pull_request::list` (GraphQL `LIST_QUERY`, falling back to `LIST_QUERY_SLIM`, then `recheck_failing`) → `app.open_prs[project].list: Vec<OpenPr>` via `note_open_prs_answer`. Every 15s for the selected project. Read by the modal's left list, OPEN PRS group, palette, Linear `work_of`, autofix `note_list`, and `launcher::row_pr` as fallback.
- `pull_request::detail` (`gh pr view N --json DETAIL_FIELDS`) → `app.pr_detail[url]` via `land_pr_detail`. Read by the preview pane / modal right pane (`pr_preview::state_word`), the merge form (`pr_actions.rs` ~907), ⌘D (`pr_actions.rs` ~1542), autofix `land_detail`.
- `pull_request::lookup` (`gh pr view` on a checkout's branch) → `app.pull_requests[worktree]` via `land_pull_request`. Every 15s for the selected checkout (`PR_REFRESH`), every 5 min for others (`PR_SWEEP_REFRESH`). Read by the sidebar PR ROW, footer, launcher band and card (`row_pr` prefers it over the list).
- `pr_cache::install` hydrates all three (or_insert) and marks details `pr_detail_stale`.
- `adopt_pr_state` copies a landed detail onto `app.pull_requests` only. `health()` reads `mergeable: UNKNOWN` as clean.

### What to build

1. **`pr_store.rs`: `PrStore` keyed by PR URL**, on `App` as `app.prs`. Per URL, a `PrFacts` of `Known<_>` fields (from `fetch.rs`): `title`, `state` (`OPEN`/`MERGED`/`CLOSED`), `is_draft`, `conflicts: bool`, `checks: Checks`, `head_sha`. One write path, `observe(url, PrObservation, Asked) -> bool`, where `PrObservation` has every field optional and constructors from a list row, a recheck, a detail and a lookup. One read path, `status(url) -> Option<PrStatus>` giving `Standing`, `Health` (unknown conflicts read as `false` for drawing, but the status also says whether conflicts are known), `title`.
2. **Parse `mergeable` as three-valued.** `CONFLICTING` → known true, `MERGEABLE` → known false, `UNKNOWN`/missing → unknown. The copies (`OpenPr`, `PullRequest`, `PrDetail`) must carry the unknown through to the observation. A `pr_cache` document written before this change must still load (`#[serde(default)]`); reading old cached values as known is fine since they are `Asked::Cached` and lose to the first live answer.
3. **Every fetcher observes into the store,** stamped with when it was asked: the list (one `Asked::At` per list answer, for every row), the recheck, the detail, the lookup. Hydration from `pr_cache` observes with `Asked::Cached`. Use `fetch::Flights` for lists (per project), details (per URL) and lookups (per worktree) so tickets carry the asked time and owed re-fetches are honoured. Replace `pr_inflight`, `pr_detail_inflight` and any list in-flight flag with them.
4. **Every screen that draws or decides on PR status reads `app.prs.status(url)`.** To find every reader, rename the status fields on the copies (e.g. `health` → `answered_health`, and the same for `state` / `is_draft` where screens read them) so the compiler lists every site, then route each through the store. Sites the audit found: `pr_modal.rs` row drawing (~1491, ~1563), its `is:` and `checks:` facets (~546), `pr_preview::state_word` (~551), `pr_row::look` callers, `palette.rs` PR rows (~419), `launcher::row_pr` (~173) and cards, `ui/footer.rs` (~135), `app.rs` sidebar PR ROW (~5830), `pr_actions.rs` merge form (~907, ~963, ~992) and ⌘D (~1542), `autofix.rs` (`note_list`, `land_detail`, `row_state` ~345, the green check ~398–408), `linear.rs::work_of` (~873–891). The copies keep their non-status data (list membership and order, `meta`, the detail's body, files, commits, per-check list).
5. **One checks verdict per PR.** Today the row's ✓/✗ mark and `checks:` filter use `OpenPr::checks()` (tally first) while the badge word, palette and sidebar use `health.checks` (rollup word), and they can differ. Make the list observation's `checks` the single value (`OpenPr::checks()`'s rule, after the recheck), and run `recheck_failing` for a row when either the rollup word or the tally says failing.
6. **The recheck reads only what it can trust.** In `FAILING_PART`, ask for `pageInfo { hasNextPage }` on `contexts(first: 100)`. When there are more pages, observe no checks for that row (unknown) instead of folding a partial list.
7. **The slim list keeps what it didn't ask.** When `list` falls back to `LIST_QUERY_SLIM`, carry each row's previous `meta` (and the viewer-dependent section) over from the last list by URL, and don't count that as a change. No more rows jumping sections every 15s.
8. **A retired PR stays retired.** When the store says `MERGED`/`CLOSED` from an answer asked after the list was asked, that row is left out of the landed list (`drop_retired_pr` + `note_open_prs_answer`).
9. **"Read this page again" is never lost.** `stale_details_behind`, `pr_actions.rs::read_again` (~1870) and `pr_modal::request_list` (~606) call `Flights::want_fresh`. A detail answer only clears `pr_detail_stale` if no fresh fetch is owed; an owed fetch starts as soon as the current one lands. The same for lists.
10. **A branch switch cancels the old branch's lookup.** In `branch_switch.rs` (~1261–1281) and `event_loop.rs::apply_upsert` (~13657–13671), where `pull_requests` is cleared for a worktree, also `Flights::cancel` its lookup. In `land_pull_request`, drop an answer whose ticket was cancelled, and drop one whose PR head branch no longer matches the worktree's branch.
11. **The merge form and ⌘D read the store.** The merge form's warnings ("has conflicts", "checks failing") come from `app.prs.status(url)` and are recomputed when a detail lands while the form is open (remove the `pending` gate that freezes it on a cached body). ⌘D picks ready vs `--undo` from the store's `is_draft`.
12. **Autofix never mistakes unknown for green.** "Gone green" (resetting `attempts`, forgetting the watch) needs conflicts *known* false and checks `Passing`. An unknown conflicts answer leaves the watch and attempt count alone, and `land_detail` with unknown conflicts and nothing else wrong does not forget the watch.
13. **Linear's PR badge reads the store.** `work_of` uses `app.prs.status(url)` for any PR URL the store knows. Linear's attachment metadata is only a fallback for a URL the store has never seen, and there `IssuePr::from_json` must read missing `status` / `draft` / `hasConflicts` as unknown (fields become `Option`), drawn neutrally, never as open. `rank()` sorts unknown after known.
14. **A cached page says so.** When the right pane shows a detail that came from the cache (`pr_detail_stale`) and no live answer has replaced it, show a dim `cached` (or `updating…` while a fetch is in flight) beside the state word in `pr_preview`. Keep it to one short word in the existing header.
15. **`pr_cache` writes can't race.** `write_json_atomic` (`pr_cache.rs` ~198) uses one `.json.tmp` for every write. Give each write its own temp name (pid plus a counter) so an older snapshot can never be renamed over a newer one.

**Done when:** for one PR, the modal row, modal right pane, preview pane, sidebar PR ROW, launcher band/card, palette row, Linear badge, merge form and autofix all derive status from `app.prs`, and grepping for reads of the renamed copy fields finds only `pr_store.rs` observation code and the copies' parsers.

---

## Phase 3: Linear (builder C)

**Owns:** `linear.rs` except `work_of` and `IssuePr` (builder B owns those), the Linear chip code in `todos/view.rs` (`land_linked` ~2066, the `first_time` refresh ~620, `linked_from_linear` ~649), and the Linear wiring in `event_loop.rs`.

### How it works today

- `fetch_lists` (~2876; `mine` ≤100 + `others` ≤250) → `app.linear[project]: LinearList`, replaced whole on landing (~1039). Only when the view opens (`open_on` ~962) or on ⌘R; one in flight per project; ⌘R ignored while one runs (~981). No timer, no disk cache.
- ⌘S `update_state` (~2919): optimistic write into `app.linear` (~1432), put back only on refusal (~1083–1101).
- `attach_issues` / `attach_pr` (~1197, ~3237): only a flash.
- `LinkStore` (`linear-links.json`, keyed by branch only; `remember` ~689, `take` ~725, `remember_submit` ~1109, `attach_new_prs` ~1148).
- `fetch_linked` / `request_linked` (~2941, ~3083) → `TodoView.linked` and `item.linear_seen` via `todos/view.rs::land_linked`. No in-flight guard.
- The cursor (`selected`) is a row index, clamped on refresh (~1042).

### What to build

1. **⌘S survives an in-flight list.** Stamp each issue's state with when it was asked: a list answer observes every issue with the list's asked time; ⌘S observes the new state with `now`. A list asked before the ⌘S doesn't revert it (rule 2). The simplest shape: keep, per issue id, the pending/landed local change and its asked time, and re-apply it over any list asked earlier.
2. **⌘S on one issue goes out in order.** One mutation in flight per issue. A second ⌘S while one runs replaces the queued target (latest wins) and is sent when the first lands. A refusal puts back the state from before *that* mutation only if nothing newer has been set since (rule 5); otherwise it leaves the row and asks for a fresh list.
3. **⌘R is never dropped.** Lists use `fetch::Flights` per project; ⌘R while a list is in flight marks it owed.
4. **The cursor follows the issue.** Keep the selected issue's id; after a refresh, put the cursor on that issue wherever it now sits (as `pr_modal` follows its row by URL), clamping only if it's gone.
5. **An attach shows at once.** When `attach_issues` / `attach_pr` succeeds, add the PR to those issues' `prs` in `app.linear` and record the branch in `LinkStore`, so the work column shows it without ⌘R.
6. **`LinkStore` links aren't lost.**
   - Key links by project as well as branch. Entries in an existing `linear-links.json` without a project load as today (match any project) via `#[serde(default)]`.
   - `remember` on a branch whose earlier link already attached starts a new pending link (reset `attached`).
   - `remember_submit` onto a worktree that already has an open PR attaches straight away instead of waiting for a PR that's already listed.
   - `take` doesn't spend a link until the attach succeeds: mark it in flight, confirm on success, and put it back for the next list on failure, with a small retry cap so a permanent refusal doesn't loop forever.
7. **Todo chips land in order and never go backwards.** `request_linked` uses `Flights` keyed by the todo dir. An answer asked before the last accepted one is dropped. `linear_seen` only moves forward. When a todo is linked, don't seed `linear_seen` from a possibly hours-old `app.linear`: the first live answer after linking sets the baseline and never ticks the todo.
8. **⌘S reaches the chips.** After a ⌘S lands, update `TodoView.linked` for that identifier in every view, parked ones included. A parked todos tab shown again refreshes its chips if they're older than the PR modal's `FRESH` (30s).
9. **Truncated pages are partial, not complete.** Ask `pageInfo { hasNextPage }` on `mine` and `others`; when there's more, show a dim "more on Linear" line at the end of that section. Raise `attachments(first: 10)` to 25 and ask its `hasNextPage`; when truncated, keep the PRs already known for that issue instead of clearing them.
10. **(Optional, if cheap.)** If `fetch_linked` returns the issue's id, match chips by id as well as identifier, so a team move that renames `ENG-12` still updates its chip. Skip if it needs anything beyond the existing query.

**Done when:** a ⌘S during an in-flight list stays put; two quick ⌘S end at the second state on both orion and Linear; the cursor stays on its issue across a reorder; links attach after a failure is retried; chips never re-tick an unticked todo.

---

## Phase 4: GitHub issues, git readers, accounts, optimistic undo (builder D)

**Owns:** `issues.rs`; in `event_loop.rs` the git readers (`request_git_changes` ~1020, `note_worktree_lines` ~1089, `note_worktree_ahead` ~1107, `note_worktree_changes` ~1154, `sweep_git_changes` ~1167, `reread_checkouts` ~1209) and the `is_deleting` upsert filter (~13310); `commit_list.rs` (`resolve_base` ~260, `resolve_base_cached` ~294); `clean_worktrees.rs`; `claude_accounts.rs`; `event_loop/optimistic.rs`.

### What to build

**GitHub issues (`issues.rs`)**

1. **An edit isn't reverted by an older list.** `request_list` (~732) returns early while a list is in flight; switch it to `fetch::Flights` so the edit's refresh is owed. Stamp the edited title/body with `now` when the edit lands (~1033–1056), so a list asked before it doesn't overwrite them.
2. **A save sends only what changed, and never overwrites a newer body.** `edit_via` (~540) always sends `--title` and the body. Send only the fields that differ from what the form opened with. Before sending a changed body, read the issue's current body. If it no longer matches what the form opened with, don't send it: flash "changed on GitHub since you opened it", and reload the form with the new text and the user's edit kept in the editor so nothing typed is lost.
3. **Comments refresh.** `schedule_detail` (~899) skips any URL already in `issue_detail`. Add an `issue_detail_at` timestamp (like `pr_detail_at`). Read the page again on visit when it's older than `FRESH` (30s) or when the list's `updated_at` for that issue is newer than the page. A failed fetch is retried on the next visit after a short backoff, not refused for the rest of the session (`issue_detail_failed`).
4. **A comment posted mid-fetch shows.** Posting (~1014–1016) uses `want_fresh` on the detail's flight, so a fetch already in flight is followed by a fresh one.

**Git readers (`event_loop.rs`, `commit_list.rs`, `clean_worktrees.rs`)**

5. **Readings stamped by when they were asked.** Both readers (`request_git_changes` for the selected checkout, `sweep_git_changes` for the rest) carry their asked time. Each worktree's readings keep the asked time of the last accepted answer, and an older answer is dropped. `reread_checkouts` marks an in-flight read as owed (`Flights::want_fresh`) instead of being skipped.
6. **A failed read keeps the last value.** `note_worktree_ahead`, `note_worktree_lines` and `note_worktree_changes` keep the previous value on `None`, instead of removing it.
7. **One base for ⇡⇣ and the commit list.** The band uses `resolve_base_cached` (60s, caches `None`); the diff viewer's commit list uses `resolve_base`. Use the cached one in both, cache `None` for at most 5s, and invalidate a root's entry on `reread_checkouts`, pull/push/base sync and branch switch.
8. **Clean unused worktrees takes its own answer.** Give each `Checked` result a ticket so the first open's answer can't fill the second open's modal (`clean_worktrees.rs` ~337–371).

**Claude accounts and optimistic undo**

9. **`claude_accounts` refreshes one at a time.** `refresh` / `refresh_now` (~161, ~218) get an in-flight guard; a forced refresh during a background one is owed, not run alongside it.
10. **Undo puts back only what the action changed.** `optimistic::undo` (~232–257) restores the whole row. Make a refused rename put back only the name, a refused archive only `archived`, and only if that field still holds the optimistic value. For a refused delete: while a row is being deleted, keep the latest daemon upsert and status for it instead of dropping them (`is_deleting` filter, `event_loop.rs` ~13310), and put that latest version back on refusal, not the keypress snapshot.

**Done when:** an issue edit survives an in-flight list; a body changed on GitHub is never overwritten; comments refresh within 30s of a visit; ⇡⇣ never blanks on one failed read and never shows a pre-pull count after the post-pull read; a refused action never rolls back a newer status.

---

## Phase 5: final pass (coordinator)

1. Check every numbered item in Phases 2–4 against the code: done, or recorded below with why not.
2. `cargo fmt --all`, `cargo clippy --workspace --all-targets`, `cargo test --workspace` (slow: it includes the e2e suites in `crates/orion/tests`).
3. `crates/orion/tests/e2e_tui.rs` uses `mergeable` in its fixtures: make sure it still matches the three-valued parse.
4. Confirm no path in `crates/orion-daemon/daemon-inputs.txt` changed (`git diff --stat main -- $(grep -v '^#' crates/orion-daemon/daemon-inputs.txt)` is empty), so this is a patch release.
5. Update `docs/how-it-works.md` if it describes PR, Linear or issue refreshing.
6. Manual check with `make dev`: open the PR modal on a repo with a conflicting PR; the list row and right pane agree, and the launcher band and Linear badge say the same.

## Verification commands

```bash
cargo check -p orion-tui --all-targets
cargo test -p orion-tui
cargo clippy -p orion-tui --all-targets
cargo fmt --all -- --check        # final pass only
cargo test --workspace            # final pass only (slow)
```

## Out of scope

- Moving any polling into the daemon.
- A generic store for every entity kind. Each phase builds the store its own entity needs, on the shared `fetch.rs` rules.
- Paging the PR list past `LIST_LIMIT` (100).
