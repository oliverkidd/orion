# Needs you, automations and connections

This plan adds the following to orion:
- one screen for everything waiting on you
- phone replies to agents while you are away
- a set of built-in automations: PR fixer, bug fixer, feature builder, week in review, meeting takeaways and blue sky builder
- links that open in each Claude account's own browser
- the Slack and Microsoft 365 connections those features need

Every screen is specified in full below, so this document is enough to build from. The signed-off mockups are a visual reference only: https://claude.ai/artifact/B1QFsWbqBqtFk3rE6YSSCt. Where the canvas and this document disagree, this document wins.

The plan is written against `main` at `ce0ffd1` ("A quieter orion: hairline frames, softer grays, a four-part footer"). Line numbers are approximate (`~`), so find code by the names given.

---

## Part 1. How this plan is run

The whole build is run by one **orchestrator**, a top-level Claude Code session. It sends each step to a **builder subagent**, reviews what comes back, commits it and tests it.

### 1.1 Orchestrator setup (do this first)

1. Open a Claude Code session in the orion repo and ask it to carry out this plan. Then move it into a worktree with the following command. orion moves the session there, and you carry on in the worktree.
   ```sh
   orion worktree needs-you --base main
   ```
   The branch `needs-you` is the **integration branch**. Every builder's work lands there as its own commit.
2. **Never push `needs-you` to `main` and never open a PR.** Every push to `main` is a release. The user decides when to ship.
3. Before any building starts, ask the user to confirm the **kickoff decisions** in 1.6. Use one AskUserQuestion call that covers all of them.
4. Create `docs/2026-11-07-needs-you/build-log.md` in the integration worktree. After each step lands, add a line: the step id, the commit sha, what was verified, and any deviation from this plan.

### 1.2 Spawning builders

- Each step in Part 3 goes to one builder, spawned with the Agent tool:
  - `subagent_type: "general-purpose"`
  - `model: "opus"` (Opus 5.5)
  - `effort: "medium"`. The user asked for medium effort for builders.
  - `isolation: "worktree"`, so each builder gets its own checkout.
- **Parallelism:** run at most **three builders at once**. Cargo builds are heavy, and the laptop also runs your agents. The parallel groups are listed in Part 2. Steps in one wave touch mostly separate files. Where two steps share a file, Part 2 says so and names who merges first.
- Start each builder with the **builder brief** (1.3), filled in with the step id. Do not paste the whole plan into the prompt. Tell the builder to read the plan file itself.

### 1.3 Builder brief (copy, fill in `<STEP>`)

```
You are building step <STEP> of the orion plan at
docs/2026-11-07-needs-you/needs-you.md, in your own git worktree.

Before anything else:
  git fetch origin && git reset --hard needs-you
(needs-you is a local branch in the main repo; if `git reset --hard needs-you`
fails, run `git -C <integration worktree path> rev-parse HEAD` and reset to that sha.)

Read, in this order: Part 2 (where your step sits), Part 4 (the shared rules),
your step in Part 3, and every file the step names. Then build exactly that step.

Rules:
- Keep to the step's scope. If you need something another step owns that
  isn't on needs-you yet, stop and report it instead of building it.
- Reuse what the step names. Do not add a second copy of a helper that
  exists (the one-shot model runner, the secrets helper, the needs-you card
  model, the automations registry, the preset system). If you think the
  shared piece needs changing, change it in place and say so in your report.
- Follow docs/keys.md conventions and the "input is not action" rule
  (crates/orion-tui/src/event_loop/activate.rs header). Every new overlay
  goes into the INPUT PARITY tables in event_loop.rs.
- Update the user docs your step names, in the same change.
- New daemon code under a new path: add the path to
  crates/orion-daemon/daemon-inputs.txt.
- Run the step's verification commands. Do NOT run the full e2e_pty suite
  unless the step says to; run only the named tests.
- Before you finish, run the `simplify` skill over your diff.
- Do NOT commit. Leave your changes in the worktree.

Report back (under 400 words): what you built, files touched, tests added
and their results, anything you skipped or changed from the plan and why,
and anything another step should know.
```

### 1.4 Reviewing a builder's work

When a builder reports:

1. Read its diff:
   ```sh
   git -C <builder worktree> diff needs-you
   git -C <builder worktree> status
   ```
2. Review it against this checklist:
   - **Plan drift:** does it do what the step says, the way the screen spec says? Labels, keys, order of sections, empty states.
   - **Centralised code:** no duplicated helpers. Shared pieces (Part 4) are used rather than copied. No new one-off `curl` wrappers where `linear::graphql`, `oneshot`, `secrets` or `http` exist.
   - **Ties to other steps:** does it expose what a later step needs (named in "Hands to")? Does it clash with a step running in parallel?
   - **Rules:** input parity tests updated, keymap tests pass, docs updated, `daemon-inputs.txt` updated, config compat tests pass.
   - **Tests:** the step's listed tests exist and pass.
3. If anything is off, send the builder specific comments with SendMessage (file, what's wrong, what to do), and wait for its next report. Repeat until it's right. After three rounds on the same problem, fix it yourself in the builder's worktree.
4. When it's right, commit in the builder's worktree. Use the repo's commit style: one plain sentence saying what the user gets, then a short body that names the step id. End the message with the attribution trailer your session asks for.
5. Bring the commit onto `needs-you` in the integration worktree:
   ```sh
   git cherry-pick <sha>
   ```
   Resolve any conflicts yourself. If a conflict changes behaviour, re-run that step's verification.
6. Run the **wave gate** (1.5) once every step of a wave has landed. Then start the next wave.

### 1.5 Wave gate (after each wave, in the integration worktree)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p orion-core -p orion-daemon -p orion-tui -p orion-fuzzy
cargo test -p orion --no-fail-fast --test e2e_tui --test help_cli --test config_cli --test install_sh
cargo test -p orion --test e2e_pty          # only when the wave touched orion-core or orion-daemon
```

Then take screenshots of the wave's new screens with `make shot SCENE=<name>`, using the scenes each step adds under `scripts/shot/scenes/`. Look at every PNG in `design-screenshots/` and compare it with that screen's spec in Part 5.
- If a screen is wrong, send it back to that step's builder, or fix it yourself if the fix is small.
- If anything fails, fix it before starting the next wave. Log the result in the build log.

### 1.6 Kickoff decisions (ask the user before Wave 1)

1. **Keys.**
   - ⌘K becomes the Needs-you board. Typing still jumps anywhere, as today.
   - Automations should be on ⌘J, but **⌘J is taken today by `toggle_launcher_pane`**. Proposal: Automations gets ⌘J, and `toggle_launcher_pane` moves to ⌘⇧J.
   - ⌘⇧Y (week review) is removed, so week in review lives only in Automations and ⌘⇧P.
   - ⌘⇧A toggles "away by hand".
   
   Ask the user to confirm the ⌘J swap.
2. **Release size.**
   - These changes touch the daemon, which makes it a minor release automatically.
   - Removing ⌘⇧Y and moving ⌘J change how people use orion, which by `CLAUDE.md` could earn a major (4.0.0).
   - Proposal: minor, because nothing is dropped outright (week review moves, the pane toggle moves). Ask.
3. **Lab folder** for blue sky ideas that aren't tied to a repo. Proposal: `~/Code/orion-lab`, created as a git repo on first use and added to orion as a project. Ask for the path.

Record the answers at the top of the build log and treat them as part of this plan.

### 1.7 Finishing

After Wave 5:
1. Run the full gate from 1.5, including `e2e_pty`.
2. Run `make ci`.
3. Take every scene's screenshot and check each against Part 5.
4. Report to the user:
   - what was built, step by step, with the commit shas on `needs-you`
   - what was tested, and how
   - any deviations
   - the **manual setup steps** in Part 6, filled in with anything learned while building (exact scopes, manifest file path)
   - how to ship: merge `needs-you` into `main` and push. That push is the release.

---

## Part 2. Waves and order

```
Wave 1  1A shared TUI foundations ─┐   1B daemon foundations ─┐
                                   │                          │
Wave 2  2A Needs-you board (⌘K) ◄──┤                          │
        2B Phone bridge + away  ◄──┼──────────────────────────┤
        2C Automations engine   ◄──┤
        2D Microsoft + calendar ◄──┤
        2E Browser per account  ◄──┘   (no dependencies; can run in any wave)
                                   │
Wave 3  3A Bug fixer + feature builder  (needs 2C, 1B)
        3B Week in review automation    (needs 2C)
        3C Slack in Needs-you board     (needs 2A, 2B)
Wave 4  4A Meeting takeaways            (needs 2A, 2C, 2D)
        4B Blue sky builder             (needs 2C, 1A)
Wave 5  5A Docs pass, screenshots, full verification (orchestrator, or one builder)
```

- **Wave 1:** 1A (orion-tui) and 1B (orion-core and orion-daemon) can run in parallel. Both add a protocol-free piece of `orion-core`, in different files; 1B owns `protocol.rs`.
- **Wave 2:** run 2A, 2B and 2C first, then 2D and 2E as slots free up.
  - 2A and 2C both add key actions in `keymap.rs` and overlays to the parity tables in `event_loop.rs`. Land 2A first, then rebase 2C onto it with cherry-pick.
  - 2B and 2D both fill rows in the Connections tab that 1A creates. Each owns its own section, so the conflict is small.
- **Wave 3:** 3A and 3B both work in `automations/`. Land 3A first.
- **Wave 4:** 4A and 4B both touch `todos/`. Land 4B first, because it changes the tab model and 4A writes into it.

---

## Part 3. Steps

Each step lists: **Builds**, **Files**, **Reuse**, **Hands to**, **Docs**, **Tests**, **Verify**.

### 1A. Shared TUI foundations

**Builds:** pieces that every later TUI step uses, so none of them invents its own copy.

1. **One-shot model runner, `crates/orion-tui/src/oneshot.rs`.**
   - Move `week_review::write` (~l.1000), its `parse_reply` (~l.1028) and `account_through` (~l.966) here, as a general runner:
     ```
     oneshot::run(Ask { prompt, system, account: Account, model, effort, timeout }) -> Result<Reply { text, cost }, String>
     oneshot::account_for(cfg, &AgentPreset) -> Account
     ```
     It keeps the exact command: `<program> -p --model --effort --tools "" --system-prompt … --no-session-persistence --output-format json`, prompt on stdin via `pull_request::run_piped`, the account's env applied.
   - Add `oneshot::run_json<T: DeserializeOwned>(ask) -> Result<T, String>`. It asks for JSON only, strips a ```json fence, and parses.
   - Week review then calls `oneshot::run` with no change in behaviour.
2. **Secrets helper, `crates/orion-core/src/secrets.rs`.**
   - `get(service, account) -> Option<String>`, `set(service, account, value) -> Result<()>` and `delete(service, account)`.
   - These run `/usr/bin/security find-generic-password -s -a -w`, `add-generic-password -U -s -a -w`, and `delete-generic-password`.
   - Use the service prefix `dev.orion.`.
   - Move `usage.rs`'s `keychain()` (~l.801) onto `secrets::get`.
   - This lives in `orion-core` so the daemon (2B) can read the Slack tokens.
3. **HTTP helper, `crates/orion-core/src/http.rs`.**
   - One function that runs `curl` with headers and a bearer token passed via `--config -` on stdin, never on argv, plus an optional body in a temp file. It returns `(status, body)`.
   - Model it on `linear::curl_graphql` (~l.5991) and `usage::get` (~l.520). Point those two at it.
   - Slack Web API calls (2B, 3C) and Microsoft Graph calls (2D, 4A) use it.
4. **Needs-you card model, `crates/orion-tui/src/needs_you/model.rs`.**
   ```
   enum Column { Agents, LinearAndReviews, SlackAndMeetings }
   enum Section { Agents, Linear, CodeReviews, Slack, Meetings }
   struct Card { key: CardKey, section, title, sub, since: DateTime, urgency: u8, target: CardTarget }
   enum CardTarget { Session(AgentId), LinearIssue{project,id}, PullRequest{project,url}, SlackMessage{permalink}, Meeting{id} }
   trait CardSource { fn cards(&self, app: &App) -> Vec<Card>; }
   ```
   - Plus a dismiss and snooze store, `<data_dir>/needs-you.json`, keyed by `CardKey`. Done hides a card until its source changes it. Snooze hides it for 1h.
   - Only the model and store go here. Drawing is 2A.
5. **Connections settings tab (empty shell).**
   - Add a `Connections` tab to `SETTINGS_TABS` (`config.rs` ~l.503), after Presets and before Keys.
   - Give it four section headers, each with a placeholder status row reading "not connected": AWAY AND PHONE, SLACK, MICROSOFT 365, GITHUB AND LINEAR.
   - Add the config structs, all `#[serde(default)]` and loaded from `config.json`:
     ```
     away: { screen_lock: bool=true, idle_minutes: u32=10, phone: Vec<PhoneKind>=[Question,Plan,Crash], quiet: Option<(HH:MM,HH:MM)>=22:00–07:00 }
     slack: { dms: bool=true, mentions: bool=true, threads: bool=true, channels: Vec<String>=[], keywords: Vec<String>=[], banner: BannerWhen=DmsAndMentions }
     microsoft: { client_id: String="", tenant: String="organizations", footer_meeting: bool=true, takeaways: bool=true }
     inbox: { code_reviews: bool=true, linear_mentions: bool=true }
     ```
   - Update the tab-titles test (`config.rs` ~l.4820) and the compat fixture test.
6. **System preset slots.**
   - Add `SystemPreset::{BugFixer, FeatureBuilder, MeetingTakeaways, BlueSky}` to `agent_presets.rs` (~l.92). Set `ALL` to 7, and add the `name`, `purpose`, `builtin_text` and `default_preset` arms.
   - Use placeholder `builtin_text` constants in each future step's module (`automations/bug_fixer.rs` etc. can start as stubs that only hold the text). Each step fills in its text.
   - Add the exhaustive match arms in `follow_label`, `presets_tab::explain_purpose`, `preset_overlays::preview`/`text_title`, and fix the index-based tests.

**Files:**
- `crates/orion-tui/src/{oneshot.rs,needs_you/mod.rs,needs_you/model.rs,week_review.rs,usage.rs,linear.rs,config.rs,agent_presets.rs,presets_tab.rs,preset_overlays.rs,lib.rs}`
- `crates/orion-core/src/{secrets.rs,http.rs,lib.rs}`

**Hands to:** all later steps.

**Docs:** `configuration.md` (Connections tab skeleton, new keys), `ARCHITECTURE.md` (oneshot, secrets, http, needs_you).

**Tests:**
- unit: `oneshot` parsing (json fence, `is_error`)
- unit: `secrets` (gated on macOS, using a throwaway service name; skip in CI if `security` is absent)
- unit: needs-you store (done and snooze expiry)
- config round-trip for the new keys

**Verify:** wave gate commands without e2e_pty. Week review still writes a review, which you can check by hand with `make dev`.

### 1B. Daemon foundations: why a session is waiting, and what it said

**Builds:**
1. **Hook payload.**
   - Add `tool_input: Option<serde_json::Value>` to `HookPayload` (`orion-daemon/src/hooks/mod.rs` ~l.160).
   - Add `ExitPlanMode` to the Claude `PreToolUse` matcher in `hooks/installer.rs` `CLAUDE_EVENTS` (~l.70), next to `AskUserQuestion`.
   - Treat `ExitPlanMode` like `asks_user` (`status.rs` ~l.149), so it enters NEEDS FEEDBACK.
   - **Verify first** with a real `claude` in plan mode whether plan approval already arrives as `PermissionRequest`. Keep whichever signal is reliable, and write down in the report which one it was.
2. **Waiting reason.**
   - Add `enum WaitReason { Question{ question, options: Vec<String>, multi: bool }, Plan{ plan: String }, Permission{ tool }, Crash{ exit_code } }` to `orion-core/src/entities.rs`.
   - Fill it from the hook's `tool_input`: AskUserQuestion's `questions[0]`, and ExitPlanMode's `plan`.
   - Store the latest one per agent in the daemon (memory is enough, rebuilt from the transcript on restart). Send it in `ServerEvent::StatusChanged` as `reason: Option<WaitReason>`.
   - Bump `PROTOCOL_VERSION` (`protocol.rs` ~l.11).
3. **Last response.**
   - Add `transcript::last_assistant_text(path) -> Option<String>` in a new `orion-daemon/src/transcript.rs`.
   - It reads the tail of the JSONL, using `session_title::transcript_tail` (~l.104) and growing the window up to 1 MiB if needed. It finds the last `type:"assistant"` line and joins its `text` blocks.
   - Use `Daemon.transcripts` (`registry.rs` ~l.149). Fall back to `continue_on::find_transcript` with `claude_projects_roots` (~l.3127).
   - Add a request `ClientRequest::LastResponse{req_id, id}` that replies with `ServerEvent::LastResponse{req_id, text, reason}`.
4. **Answer a waiting session.**
   - Add `Daemon::answer(id, Answer)`, where `Answer::{Option(usize), Text(String), ApprovePlan, Steer(String)}`.
   - It works out the keystrokes for Claude Code's question and plan dialogs, and writes them with `ClientRequest::Input` semantics.
   - **Verify the exact keystrokes** against the current `claude` in a PTY test (digits or arrows plus Enter; the plan dialog's "keep planning" option followed by text).
   - When the session is not alive, or the question is gone (daemon restarted), it falls back to `send_turn` (`registry.rs` ~l.2489) with plain text. A chosen option becomes "Answer to your question: <option text>".
   - Add `ClientRequest::Answer{req_id, id, answer}`.

**Files:**
- `crates/orion-core/src/{entities.rs,protocol.rs}`
- `crates/orion-daemon/src/{hooks/mod.rs,hooks/installer.rs,status.rs,registry.rs,server.rs,transcript.rs,lib.rs}`
- `daemon-inputs.txt`: no change needed, since `crates/orion-daemon` is already listed

**Hands to:** 2A (reasons on agent cards), 2B (phone messages and answers), 3A (plan approval).

**Docs:** `how-it-works.md` (hooks, waiting reasons), `ARCHITECTURE.md` (transcript reader).

**Tests:**
- unit: `last_assistant_text` on fixture JSONL files (text-only, tool-only, mixed)
- unit: the `WaitReason` parse
- e2e_pty: a fake agent script posts a `PreToolUse` AskUserQuestion hook with `tool_input`. The test checks `StatusChanged.reason`, and that `Answer::Text` reaches the PTY.

**Verify:** wave gate, including `e2e_pty`.

### 2A. Needs-you board on ⌘K

**Builds:** the screen in 5.1.
- Make the ⌘K palette's empty-query state a full-screen three-column board. Typing keeps today's flat jump list, unchanged.
- In `palette.rs`, keep the board state apart from `matches`, and add a column cursor.
- In `ui.rs` (~l.1976), branch the draw on `query.is_empty()`. The board uses the whole body area, not `PALETTE_SIZE`.
- ←/→ move between columns only while the query is empty (`event_loop.rs` ~l.8137). Otherwise they still move the text caret.
- Add per-card mouse hit-testing (~l.11659).
- Enter goes through `activate::palette_row`/`jump_to_target`, extended with the new `CardTarget`s.

**Sources** (each a `CardSource` from 1A):
- **Agents:** sessions that are NeedsFeedback or Terminated, plus finished-unseen ones.
  - Order: crashed, then waiting (longest first), then finished.
  - Sub-line: kind (the automation's name for automation sessions, else "agent") · project · reason text from 1B ("asks a question", "plan to approve", "brief to approve", "crashed, exit N", "draft PR #N ready", "done").
- **Linear:** your unread Linear notifications (mentions, new comments on issues you're involved in, assignments).
  - Query them with the `notifications` GraphQL connection through `linear::graphql` (~l.5934). **Verify the field names against Linear's schema first.**
  - Assignments that an automation has already picked up don't show here.
  - ⌘D marks the notification read.
- **Code reviews:** `PrMeta.review_requested` (`pull_request.rs` ~l.746) across project repos, plus every 5 minutes one `gh search prs --review-requested=@me --state=open --json …` call, which covers repos that aren't orion projects. Dedupe by URL.
- **Slack:** filled by 3C. Show the "No Slack messages waiting for you" empty state until then.
- **Meetings:** filled by 4A. Same empty state until then.

**Also:**
- Add a footer segment `⌘K needs you N` (`ui/footer.rs` `draw_footer_bar` ~l.436, next to the usage readout). N is red when above zero and green at zero. A click on it opens the board.
- Remove nothing else from ⌘K: the jump rows for projects, worktrees, PRs, Linear and todos stay for typed queries.

**Files:**
- `crates/orion-tui/src/{palette.rs,ui.rs,ui/footer.rs,event_loop.rs,event_loop/activate.rs,needs_you/*.rs,linear.rs,pull_request.rs,app.rs}`
- `scripts/shot/scenes/needs-you-busy.keys`, `needs-you-empty.keys`, plus fixtures

**Reuse:** `palette::session_tier`/`attention_rank` for agent order, `launcher::project_tally` for the footer count, the needs-you store from 1A.

**Hands to:** 3C and 4A, which add sources. Every automation, which shows up in the agents column through its session's kind.

**Docs:** `keys.md` (jump list section rewritten as "Needs you (⌘K)"), `sessions.md`, `configuration.md` (the GITHUB AND LINEAR rows become live).

**Tests:**
- unit: card order
- unit: "+N more" collapsing
- unit: the empty-state strings
- input parity (a click on a card is Enter on it)
- e2e_tui: ⌘K shows the board with the empty states; typing shows the flat list
- the old palette tests updated

**Verify:** wave gate, plus the screenshots above.

### 2B. Phone bridge and away mode (daemon)

**Builds:** the daemon side of 5.3 and 5.4.
1. **Slack client, `crates/orion-daemon/src/slack/`.**
   - `socket.rs` holds a Socket Mode websocket:
     - Add `tokio-tungstenite` with rustls to the workspace. Adding it changes `Cargo.lock`, which is a daemon input, so that alone makes the release minor.
     - Get the URL from `apps.connections.open` with the app token. Reconnect with backoff, and acknowledge every envelope.
   - `api.rs` holds `chat.postMessage`, `chat.update`, `conversations.open` and `auth.test` via `orion_core::http`.
   - Tokens come from `orion_core::secrets`:
     - `dev.orion.slack`/`app` (`xapp-…`)
     - `dev.orion.slack`/`bot` (`xoxb-…`)
     - `dev.orion.slack`/`user` (`xoxp-…`)
2. **Away detection, `crates/orion-daemon/src/away.rs`.**
   - Poll every 5 s.
   - Idle time comes from `HIDIdleTime` in `ioreg -c IOHIDSystem -d 4`, in nanoseconds.
   - Screen lock comes from `CGSSessionScreenIsLocked` in `ioreg -n Root -d1 -a`, under IOConsoleUsers. **Verify on the user's macOS first;** if absent, use `IOConsoleLocked`.
   - Add a manual flag through `ClientRequest::SetAway{on}`. It clears on the next key in any orion client: the TUI sends `SetAway{on:false}` on its first key after the flag was set.
   - Away = (screen lock on and locked) or (idle ≥ `away.idle_minutes`) or the manual flag.
   - **Meetings never count as away.**
   - Add a `ServerEvent::AwayChanged{away}` broadcast.
3. **Phone messages, `crates/orion-daemon/src/phone.rs`.**
   - Trigger: a session enters NeedsFeedback or Terminated (`apply_status_effects`, `registry.rs` ~l.294) while away, outside quiet hours, and of a kind in `away.phone`.
   - Post at once, with no delay. The DM goes to the installing user, using `auth.test`'s user id and `conversations.open`.
   - Message layout follows 5.4 exactly:
     - a context line: `● <kind> · <project> · <HH:MM>`
     - a header with the **session name**, never a ticket id
     - the **full** last response from `transcript::last_assistant_text`, split into 3000-character section blocks. It is never summarised.
     - then the question or plan
     - buttons: each option, then "Something else…"; for a plan, "Approve" plus a reply button
   - Keep `message ts ↔ agent id` in SQLite: add a table `phone_messages` in `store.rs` with a migration.
4. **Replies.**
   - Socket Mode `block_actions`: an option goes to `Daemon::answer(Option(i))` and Approve to `ApprovePlan`. "Something else…" and the reply button post an ephemeral "Reply in this thread" hint.
   - `message` events in the DM: a thread reply (`thread_ts` matches a stored message) goes to `Answer::Text` (or `Steer` for a plan) on that agent. A plain DM message goes to the most recent unanswered ask.
   - Ignore every user except the installing user.
   - After sending, post the thread reply "✓ Sent · working again".
   - When a session leaves NeedsFeedback by any other path (you answered at the desk), `chat.update` the message to add "Answered in Orion".
5. **Slack inbox events.** Forward the user-token message events (`message.im`, `message.mpim`, `message.channels`, `message.groups`) as `ServerEvent::SlackMessage{…}` without filtering. 3C filters them and builds cards.
6. **Slack app manifest.** Add `docs/2026-11-07-needs-you/slack-manifest.json` with Socket Mode on, the interactivity scopes, bot scopes `chat:write`, `im:write`, `im:history`, and user scopes `channels:history`, `groups:history`, `im:history`, `mpim:history`, `users:read`. Subscribe to the events in step 5 and `app_home_opened`.

**Files:**
- `crates/orion-daemon/src/{slack/mod.rs,slack/socket.rs,slack/api.rs,away.rs,phone.rs,store.rs,registry.rs,server.rs,lib.rs}`
- `crates/orion-core/src/protocol.rs`
- `Cargo.toml`, `Cargo.lock`
- `crates/orion-daemon/daemon-inputs.txt` (unchanged unless a path outside the daemon crate is added)

**TUI side (same step, small):**
- Fill the AWAY AND PHONE and SLACK sections of the Connections tab (5.2).
- `c` on the Slack status row opens three prompts for the tokens, saved through `secrets::set`. `o` deletes them.
- The status row shows the workspace name from `auth.test`.
- Add the ⌘⇧A action `away_toggle`.

**Docs:** `configuration.md` (Connections: away and Slack rows), `how-it-works.md` (phone bridge), `keys.md` (⌘⇧A), `commands.md` if any CLI is added.

**Tests:**
- unit: the away rule (lock, idle, manual, quiet hours)
- unit: message block building (long text split, buttons)
- unit: reply routing (thread vs plain DM, unknown user ignored)
- e2e_pty: with a fake Slack. Point the base URL at a local axum test server through an env var `ORION_SLACK_API_URL`, and the socket at a local websocket. Check that a waiting session posts one message and that a button reply reaches the PTY.

**Verify:** wave gate, including `e2e_pty`. Then a manual check on the user's workspace in Wave 5.

### 2C. Automations engine and screen (⌘J)

**Builds:** the screen in 5.5, and the engine every automation plugs into.
1. **Module `crates/orion-tui/src/automations/`.**
   ```
   mod.rs       registry: enum Automation { PrFixer, BugFixer, FeatureBuilder, WeekReview, MeetingTakeaways, BlueSky }
                trait Runner { fn tick(&mut self, app) ; fn panel(&self, app) -> Panel ; fn settings(&self) -> &'static [SettingRow] }
                running cap: count live sessions started by any automation; never start one past `automations.max_running`
   config.rs    automations: { max_running: u8=5, per: { <automation id>: { on: bool, extra_steer: String, labels: Vec<String>, when: Option<Schedule>, whose: Whose } } }
   view.rs      the Automations overlay (list left, panel right), modelled on clean_worktrees.rs
   pr_fixer.rs  today's autofix.rs, moved in (see 2)
   ```
2. **Move PR autofix in.**
   - `autofix.rs` becomes `automations/pr_fixer.rs` behind the `Runner` trait. Its behaviour stays exactly the same: ledger, attempts, stand-down, peers.
   - The `pr_autofix` key (`config.rs` ~l.1005) is retired. It loads as nothing and survives a save, per the compat rule. Its value seeds `automations.per.pr_fixer.on` once.
   - Its General settings row is removed.
3. **Tagging automation sessions.**
   - Every automation launch sets the session's kind tag. Add `automation: Option<String>` to the create request, or reuse the name prefix; prefer a field.
   - The tag drives the grid badge and the Needs-you kind label.
   - Persist it in the daemon's agents table. This touches `protocol.rs` and `store.rs`, so coordinate with 2B, which also bumps the protocol: land 2B first, then rebase.
4. **The overlay** (5.5):
   - list on the left, panel on the right
   - SETTINGS rows from `Runner::settings()`
   - RUNNING and DONE THIS WEEK from each runner's ledger
   - keys: Space on/off, Enter open (a preset row opens it in Settings › Presets; a list row jumps to its session), ⌘U pick another preset, Esc
   - add the action `automations` with default ⌘J; move `toggle_launcher_pane` to ⌘⇧J if the user agreed in 1.6
   - add the overlay to the INPUT PARITY tables and bump `the_overlay_table_covers_every_variant_exactly_once`
5. **Extra steer.** Each runner appends `extra_steer` to its preset text with `\n\nExtra steer: <text>` when it's not empty. Do this in one helper, `automations::compose(preset, context, extra)`.

**Files:**
- `crates/orion-tui/src/{automations/*,autofix.rs (removed),config.rs,keymap.rs,event_loop.rs,app.rs,ui.rs,agent_presets.rs}`
- `crates/orion-core/src/protocol.rs`
- `crates/orion-daemon/src/{store.rs,registry.rs,server.rs}`
- `scripts/shot/scenes/automations-*.keys`

**Hands to:** 3A, 3B, 4A and 4B, which each add one `Runner`.

**Docs:**
- `configuration.md`: drop the `pr_autofix` row; add the `automations` keys
- `keys.md`: ⌘J and ⌘⇧J
- `sessions.md`: rename PR autofix to PR fixer; add the Automations section
- `how-it-works.md`
- `ARCHITECTURE.md`

**Tests:**
- every existing autofix test keeps passing after the move (rename paths only)
- unit: the running cap
- unit: `compose` with and without extra steer
- input parity for the overlay
- config compat for the retired key

**Verify:** wave gate, including `e2e_pty`, because the protocol changed.

### 2D. Microsoft 365: sign-in, calendar and project descriptions

**Builds:**
1. **Module `crates/orion-tui/src/microsoft/`.**
   - `auth.rs` runs the device-code flow:
     - POST `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/devicecode` with `client_id` and scopes `offline_access User.Read Calendars.Read OnlineMeetings.Read OnlineMeetingTranscript.Read.All`.
     - Show the user code in a prompt, and open the verification URL in the browser chosen by 2E's helper.
     - Poll the token endpoint. Store the refresh token with `secrets::set("dev.orion.microsoft", "refresh", …)`, and refresh when needed.
   - `graph.rs` holds the Graph GET helpers on `orion_core::http`.
   - `calendar.rs` polls `GET /me/calendarView?startDateTime=now&endDateTime=+12h&$select=subject,start,end,isOnlineMeeting,onlineMeeting` every 5 minutes. It uses the Spotify poll pattern (`spotify.rs` `spawn_poll` ~l.114, wired in `event_loop.rs` ~l.410/732/748), with an env override for e2e.
2. **Footer.** Add a segment `▸ <subject> in <N>m · Teams`, shown when the next meeting starts within 60 minutes. "now" replaces the countdown while one is running. Draw it with the `spot` pattern in `ui/footer.rs` (~l.436).
3. **Connections › MICROSOFT 365 section.**
   - A status row, where `c` signs in and `o` signs out.
   - "Next meeting in the footer" and "Takeaways from Teams transcripts" rows.
   - `microsoft.client_id` and `tenant` come from config. While `client_id` is empty, the status row reads "needs your organisation's app id (see setup)".
4. **Settings › Projects rows** (5.7):
   - **Link to meeting takeaways** (on/off, default on). Stored as `projects.<path>.takeaways: bool`.
   - **What it is**: one line, `projects.<path>.about: String`.
     - It is written when a project is added (and lazily for existing projects on first need) by `oneshot::run` on the default Claude account at haiku or sonnet low effort. The prompt is the project's README head plus the last 20 merged PR titles.
     - Enter edits it. `^R` writes it again.
   - Add one shared helper `projects::about(app, project) -> String` that 3A and 4A use for routing.

**Files:**
- `crates/orion-tui/src/{microsoft/*,ui/footer.rs,event_loop.rs,config.rs,projects_about.rs (or inside config),app.rs}`

**Hands to:** 4A (transcripts and routing), 3A (routing tickets to a project when several projects share a Linear workspace).

**Docs:** `configuration.md` (Microsoft rows, Projects rows), `sessions.md` (footer meeting).

**Tests:**
- unit: device-code polling states (pending, slow_down, success, expired) against canned JSON
- unit: calendar parse
- unit: footer text
- unit: the `about` prompt builder
- e2e_tui: none; the network is off in e2e

**Verify:** wave gate. Then a manual sign-in in Wave 5.

### 2E. Links open in each account's browser

**Builds:**
1. Add an **Open links in** row per Claude account on the Accounts tab.
   - It cycles `default`, `Safari`, `Chrome`, `Firefox`, `Arc`, plus an optional profile name typed after the browser (Chrome `--profile-directory=<p>`, Firefox `-P <p>`).
   - Store it as `harnesses.<id>.browser: { app, profile }`.
2. Add one helper, `crates/orion-tui/src/open_url.rs`: `open_url(url, account: Option<&str>)` runs `open -a <App> [--args …] <url>`, or plain `open` for the default.
3. Find every call site that opens a URL (grep for `"open"` and `Command::new("open")` across orion-tui), and send each through the helper.
   - URLs opened from a session's pane, or from a row that belongs to a session, pass that session's account.
   - Others pass `None`.

**Files:** `crates/orion-tui/src/{open_url.rs,config.rs,claude_accounts.rs, call sites}`

**Docs:** `configuration.md` (Accounts row).

**Tests:**
- unit: the command line built for each browser and profile
- a grep-style test that no `Command::new("open")` is left outside `open_url.rs`

**Verify:** wave gate.

### 3A. Bug fixer and feature builder

**Builds:** two `Runner`s that share one implementation, `automations/linear_builder.rs`, set up with different preset, labels and name.
1. **Watch.**
   - Every 2 minutes, for each distinct Linear API key across projects, fetch issues assigned to you that carry one of the runner's labels and are in a backlog, unstarted or triage state. Reuse `linear::graphql` and `ISSUE_FIELDS`.
   - Skip issues already picked up. Keep a ledger at `<data_dir>/automations/linear-builder.json`: `issue id → {session, state}`.
   - A ticket with none of either runner's labels is ignored.
2. **Route to a project.**
   - If only one project uses that key, use it.
   - Otherwise ask `oneshot::run_json` with the ticket title and description plus each project's `about` (2D), and pick one.
3. **Group.**
   - When two or more new tickets land in one tick, one `oneshot::run_json` call groups them only where they clearly share a cause or component. The default is one ticket per group.
   - Before launching, also collect the titles and URLs of open draft PRs and of PRs merged in the last 14 days in that repo (from `app.open_prs` and the merged list), and pass them in the prompt as "Related work to check".
4. **Launch.**
   - One session per group, in a fresh worktree, through the existing Linear launch path: `LinearBatch::of` (~l.457), `start_issues` (~l.2616) and the PR attach on the remembered branch (~l.2759). That path already moves the issue to In Progress, assigns it, and attaches the PR.
   - Start in **plan mode**: the quick prompt's mode field already supports plan, so pass it through `AgentLaunchDraft`.
   - Tag the session with the automation (2C).
   - Respect `automations.max_running`. Waiting tickets show as "queued, N running".
   - The plan approval reaches the user through Needs-you (2A) and the phone (2B) via 1B's `Plan` reason. No extra code is needed.
5. **Preset text** (`builtin_text` for BugFixer and FeatureBuilder) must say:
   - Read the ticket(s) and the related work listed. Check the repo's `docs/decisions/` folders for the product areas touched, and read every decision there before planning. If no folder fits, create a new area folder.
   - Write a plan. Never go against a logged decision without saying so in the plan.
   - After approval, build. Reuse the design system and component library and the central registries; no new helpers where one exists.
   - Run unit tests and only the e2e specs that cover the change, **never the full e2e suite**.
   - Log any new product decision the user made while steering as a file under `docs/decisions/<area>/`.
   - Open a **draft** PR whose description has these sections:
     - **What and why** (two lines, tickets linked)
     - **Blast radius** (modules, callers of changed functions, screens; rated low, medium or high)
     - **Decisions** (files read; anything gone against)
     - **Reuse** (components and helpers used; anything new and why)
     - **Neighbours checked** (related flows and how they were checked)
     - **UI** (before and after for each state: empty, loading, error, full, narrow; only for UI changes)
     - **Performance** (before and after in ms, for changes to state)
     - **Check it yourself** (a 3-step flow)
6. **Panels** (5.5): the settings are On, **Labels** (Enter opens a multi-pick of the workspace's issue labels, fetched with `issueLabels` through `linear::graphql`), Preset and Extra steer. The lists are RUNNING and DONE THIS WEEK.

**Files:** `crates/orion-tui/src/{automations/linear_builder.rs,automations/mod.rs,linear.rs (expose what's needed, don't copy),agent_presets.rs}`

**Docs:** `sessions.md` (Bug fixer, Feature builder), `configuration.md` (labels key).

**Tests:**
- unit: label filter
- unit: ledger skip
- unit: grouping fallback (model error → one per ticket)
- unit: routing with one project vs several
- unit: running cap queueing
- unit: the preset text contains each PR section heading

**Verify:** wave gate. Then a manual run in Wave 5 on a real labelled test ticket.

### 3B. Week in review as an automation

**Builds:**
1. A `Runner` for week in review, using the panel in 5.5:
   - settings: On, **When** (default Fridays 08:30), **Whose** (just me or the whole team), Preset, Extra steer
   - `r` runs it now
   - the list shows saved reviews
2. **Gather from every project** that had changes in the last 7 days, with no ticks: PRs, done Linear issues and ticked todos, all through today's `week_review::gather` (~l.505).
   - Write **one review per project**, one `oneshot::run` each, in parallel, at most 3 at once. Validate each with today's `check` (~l.1135) and `finish` (~l.1222).
   - Store the run as `reviews/all/<to>/<project-slug>.md` plus `meta.json`, keeping the newest 12 runs.
3. **The review page** shows the run with **a tab per project that had changes**: `[`/`]` or `⇧←`/`⇧→` move between tabs, as in todos. Reuse the existing page renderer.
4. **Removals:**
   - Remove the ⌘⇧Y action and the review form. The Whose, Where, Range, sources and Note rows go.
   - The per-project `note.txt` content migrates once into the automation's `extra_steer`.
   - Add a ⌘⇧P command-palette action **"Run week in review now"**. It's an `ActionSpec` with no default key, so it shows in the palette automatically (`event_loop/menus.rs` ~l.53).
5. **Schedule.** Check the `When` time once a minute while the TUI is running. If the laptop was asleep at that time, run on the first check after it, once.

**Files:** `crates/orion-tui/src/{week_review.rs (slimmed: gather/check/finish/page stay), automations/week_review.rs, keymap.rs, event_loop.rs}`

**Docs:** `sessions.md` (Week in review rewritten), `keys.md` (⌘⇧Y gone).

**Tests:**
- every existing `gather`, `check` and `finish` test still passes
- unit: schedule (missed run, already run today)
- unit: per-project split skips projects with no changes
- input parity for the page tabs

**Verify:** wave gate.

### 3C. Slack in the Needs-you board

**Builds:**
1. A `CardSource` that keeps `ServerEvent::SlackMessage`s (2B) and turns the matching ones into Slack cards. The rules come from the Connections settings:
   - DMs
   - messages containing `<@your id>`
   - replies in threads you've posted in (track thread ids you've posted in from the same event stream)
   - every message in the listed channels
   - messages containing a keyword
2. **Card:** the sender's display name plus the first line of the message as the title, and `direct message` or `#channel · mentions you` as the sub-line. Enter opens the permalink: `chat.getPermalink`, done once and cached, opened with `open_url`.
3. **Clearing a card:**
   - A card clears itself when you post in the same conversation or thread later.
   - ⌘D clears it locally.
4. **Banner and sound:** for the `banner` setting's kinds, post `alerts::notify_text` with the existing sound gate (`event_loop/alerts.rs` ~l.110).
5. **Persistence:** keep the last 200 Slack events in memory, and save the visible cards in the needs-you store so they survive a TUI restart.

**Files:** `crates/orion-tui/src/{needs_you/slack.rs,event_loop.rs,app.rs}`

**Docs:** `sessions.md` (Slack column), `configuration.md`.

**Tests:**
- unit: each matching rule
- unit: clearing on your own reply
- unit: dedupe

**Verify:** wave gate.

### 4A. Meeting takeaways

**Builds:**
1. A `Runner`:
   - When a calendar event from 2D with an online meeting passes its end time, look up the meeting with `GET /me/onlineMeetings?$filter=JoinWebUrl eq '<url>'`, then poll `…/transcripts` every 2 minutes for up to 60 minutes. Fetch the content with `$format=text/vtt`.
   - **Verify early** that delegated `OnlineMeetingTranscript.Read.All` can read transcripts of meetings the user only attended. If it can't, report it. The fix is an admin application access policy, which goes in Part 6. Don't build around it.
2. **Extract** with one `oneshot::run_json` call (preset MeetingTakeaways plus extra steer). The input:
   - the transcript
   - your name and email
   - for each project with **Link to meeting takeaways** on: its name, `about`, and its todo group tree (names only)
   - the Blue sky list's groups
   
   It returns takeaways `{text, owner_initials, is_mine, project|blue_sky|null, group_path, priority}`. Takeaways that name you start accepted.
3. Show a card in Needs-you › Meetings: `<meeting> · N takeaways for you to place`.
4. **The takeaways screen** (5.8):
   - left: accept or leave out
   - right: where accepted items go in your todos, existing items dim, new ones highlighted
   - `m` opens the move picker over the right side: a filter field, then every linked project's groups and Blue sky, then `+ new group "<filter>"`
   - Enter adds the accepted takeaways with `todos::store`, at their group and priority. A takeaway that matches an open ticket gets the ticket id appended, as in the mockup.
5. **Panel** (5.5): On, Preset, Extra steer. The MEETINGS list shows: "3 to place", "2 added", "nothing for you", "no transcript".

**Files:** `crates/orion-tui/src/{automations/meeting_takeaways.rs,microsoft/transcripts.rs,needs_you/meetings.rs,takeaways_view.rs,todos/*}`

**Docs:** `sessions.md` (Meeting takeaways), `configuration.md`.

**Tests:**
- unit: vtt → text
- unit: extraction JSON parse with a bad reply → error card
- unit: placement into an existing group and a new group
- input parity for both overlays

**Verify:** wave gate, plus screenshot scenes for the screen and the picker.

### 4B. Blue sky builder

**Builds:**
1. A **Blue sky tab** that is the first tab in Todos (5.9).
   - Give `TodoView` a tab enum, `Tab::BlueSky | Tab::Project(ProjectId)`, in `todos/view.rs` (~l.209). Prepend the Blue sky tab in the tab strip only; `projects()` stays as is.
   - Its file is `<data_dir>/todos/blue-sky.json`, through `store::path_for` with a fixed key.
   - Check every place that maps a `TodoFile.repo_path` back to a project (`palette::todo_rows`, the merge-tick code, `done::lists`, `week_review::gather`), and make each handle Blue sky. Its done items do appear in DONE.
   - ⌘1 becomes Blue sky, and projects start at ⌘2.
2. **Each idea has a target.** `w` cycles through the projects and `lab`. Store it on the item as `target: Option<String>`.
3. **Enter starts the Blue sky builder** on the idea:
   - The session is tagged `blue sky` and uses the BlueSky preset (its own Runs on, e.g. the Personal account).
   - A repo target runs in a new worktree on branch `proto/<slug>` with a note never to open a PR.
   - A `lab` target runs in the lab project (1.6), in a folder `<slug>/`, created and added as a project on first use.
   - The preset text says: first write a short brief (for whom, what, built from what, what you'll get, size) and **wait for approval** in plan mode. Then build a working prototype. A web tool may also be published as a claude.ai artifact if the agent can, and its link noted. Finish with how to try it.
   - The brief reaches Needs-you and the phone as a `Plan`.
4. **The idea row's right column** shows: `● brief to approve` (red), `● building Nm` (yellow), `● built · <branch or link>` (green), or `not started`. `o` opens what it built.
5. **Panel** (5.5): On, Preset, Extra steer. The IDEAS list jumps to the idea on Enter.

**Files:** `crates/orion-tui/src/{todos/view.rs,todos/store.rs,todos/mod.rs,todos/done.rs,palette.rs,automations/blue_sky.rs,week_review.rs (gather)}`

**Docs:** `sessions.md` (Todos: Blue sky tab, Blue sky builder), `keys.md` (⌘1 shift).

**Tests:**
- every todos test still passes
- unit: tab order and ⌘1–9
- unit: Blue sky done items appear in DONE
- unit: target cycling
- input parity

**Verify:** wave gate.

### 5A. Docs pass, screenshots and full verification

1. Read every doc changed across the build, top to bottom, and fix gaps, contradictions and stale keys:
   - `README.md` core features: add Needs you, Automations, phone
   - `docs/keys.md`
   - `docs/sessions.md`
   - `docs/configuration.md`
   - `docs/how-it-works.md`
   - `ARCHITECTURE.md`
2. Take a screenshot of every new scene and compare each with Part 5.
3. Run the full gate from 1.5 and `make ci`.
4. Fill in Part 6 with exact values, and write the final report (1.7).

---

## Part 4. Shared rules for every step

- **One copy of each shared piece.** These modules are the only place each thing is done:
  - `oneshot` (model calls)
  - `orion_core::secrets` (Keychain)
  - `orion_core::http` (curl)
  - `needs_you::model` (cards)
  - `automations::{mod, compose}` (automation engine and preset composition)
  - `open_url` (opening links)
  - `projects::about` (project descriptions)
  - `linear::graphql` (Linear)
- **Labels.** Use the labels and words from Part 5 exactly. Never qualify a setting with the project name ("for web"), and never say "now" where "running" is meant.
- **No mail.** Nothing reads mail.
- **No delays** on phone messages.
- **Automation config is global, not per project.** Automations have no polling, schedule or grouping settings, except week in review's When.
- **Settings by tab:**
  - **Connections:** Away and phone, Slack, Microsoft 365, GitHub and Linear.
  - **Projects:** Link to meeting takeaways, What it is.
  - **Accounts:** Open links in.
  - **Presets:** reached from Settings or ⌘⇧P; nothing gets its own key.
- **Config compat.** Retired keys (`pr_autofix`, the week-review note file) load as nothing and survive a save. New keys default sensibly, so a config with none of them behaves as described.
- **e2e safety.** Every new poll gets an `orion_core::env` override, so e2e can switch it off, and is off when its connection isn't set up.
- **Commit message style:** for example, `The bug fixer picks up labelled Linear bugs and waits for your plan approval`.

---

## Part 5. Screen specs (the signed-off design)

All screens use orion's existing look: the hairline frames, softer grays and four-part footer now on `main`, accent keys, and dim explanation lines. Colours by meaning:
- red: waiting on you, or crashed
- yellow: working
- green: done
- blue: PRs
- purple: Linear and automation kind

### 5.1 Needs you (⌘K with nothing typed)

- **Layout:** the full body area.
- **Header row:** `Needs you`, the count, a search field ("type to jump to any project, session, PR, issue or message"), and the next meeting on the right (`▸ Standup in 12m · Teams`).
- **Three columns:**
  1. **AGENTS:** every agent type in one list, not grouped. Crashed first, then waiting on you (longest first), then finished. Each card has two lines: glyph, title, age; then `<kind> · <project> · <reason>`.
  2. **LINEAR** on top, **CODE REVIEWS** below.
  3. **SLACK** on top, **MEETINGS** below.
- A section with more cards than fit ends with `+N more  →`. → expands it.
- **Empty states**, one per section in the same style:
  - "No agents waiting for you"
  - "No Linear issues waiting for you"
  - "No code reviews waiting for you"
  - "No Slack messages waiting for you"
  - "No meetings waiting for you"
  
  Nothing else: no working counts, no "cleared today".
- **Keys line:** `type to jump · ←→ column · ↑↓ item · Enter jump to it · ⌘D done · ⌘Z snooze 1h · Esc back to the grid`.
- **Typing:** today's flat jump list, unchanged.
- **Footer:** adds `⌘K needs you N`.

### 5.2 Settings › Connections

One list, no side panel, sections in this order:

- **AWAY AND PHONE** `· Slack DM from Orion`
  - Away when the screen locks: `on`
  - Away after no keys or mouse: `10m`
  - Away by hand: `⌘⇧A`
  - Send to your phone while away: `questions, plans, crashes`
  - Quiet hours: `22:00 to 07:00`
- **SLACK** `· <workspace>`
  - Direct messages: `every message`
  - Mentions of you: `every channel`
  - Threads you're in: `new replies`
  - Channels, every message: `#…`
  - Keywords: `…`
  - Banner and sound: `DMs and mentions`
- **MICROSOFT 365** `· <email>`
  - Next meeting in the footer: `on`
  - Takeaways from Teams transcripts: `on`
- **GITHUB AND LINEAR** `· through gh and project keys`
  - Code reviews asked of you: `on`
  - Linear comments that mention you: `on`

Explanation line: "The screen locking counts as away. An agent that starts waiting then goes to your phone straight away."

Keys: `←→ change · Enter edit · c connect · o disconnect · Tab next tab · Esc close`.

### 5.3 Away

- You are away when the screen is locked, after N minutes with no keys or mouse, or by hand with ⌘⇧A.
- Being in a meeting is **not** away.
- While away, an agent that starts waiting, or crashes, goes to your phone at once.

### 5.4 Phone (Slack DM from the Orion bot)

**Question card:**
- the context line `● Agent · web · 10:42`
- the title: the **session name**
- the **full** last response
- the question in bold
- full-width option buttons, the last being `Something else…`, which starts a thread reply

**Plan card:**
- the same top part, with the full plan
- "Approve this plan?"
- an `Approve` button, with a small reply-icon button to its right that starts a thread reply

**Both cards:**
- No "or reply" hint text and no "full plan" link, because the full text is always shown.
- After a send, the thread shows your text and `✓ Sent · working again`.
- When the session is answered in orion, the message gains "Answered in Orion".
- A thread reply goes to that agent. A plain DM goes to the most recent unanswered ask.

### 5.5 Automations (⌘J)

- **Header:** `Automations` and, on the right, `N of at most 5 agents running ←→`, where ←→ changes the cap.
- **Left:** one row per automation, with a dot (working, waiting on you, idle, off) and a short state ("fixing #407", "1 plan waiting", "Fri 08:30", "3 to place", "1 brief waiting").
- **Right panel:** the name, a one-line description, **SETTINGS**, and the lists below.
- **Explanation:** "The preset says what the agent is told and what it runs on. Enter opens it in Settings › Presets."
- **Keys:** `Space on / off · Enter open · ⌘U pick another preset · Esc back`.

| Automation | Description | Settings | Lists |
|---|---|---|---|
| PR fixer | When one of your PRs hits conflicts or failing checks, fixes it and watches until it's green. Stops after 3 tries in a row that don't work. | On · Preset (`PR autofix · the PR's own agent`) · Extra steer | YOUR OPEN PRS (fixing checks, try 2 of 3 / green / gave up, needs you) · DONE THIS WEEK |
| Bug fixer | Picks up Linear tickets assigned to you that carry one of its labels, plans each one, waits for your approval, then builds to a draft PR. | On · Labels (`Bug · Regression · Customer bug · Enter to pick from Linear`) · Preset · Extra steer | RUNNING (ticket, title, plan to approve / building / queued, 5 running) · DONE THIS WEEK |
| Feature builder | Same, with its own labels | On · Labels · Preset · Extra steer | RUNNING · DONE THIS WEEK |
| Week in review (`r run now` at the top right) | Writes up the week from everything merged, done and ticked, so it's ready before the meeting. Also in ⌘⇧P as "Run week in review now". | On · When · Whose (`just me` / `whole team`) · Preset · Extra steer | REVIEWS (Week 45 · Fri 08:30 · 9 points · new). Enter opens it, with a tab per project. |
| Meeting takeaways | When a Teams meeting's transcript lands, pulls out the takeaways, accepts the ones that name you and places them in your todos for you to check. Covers all projects. | On · Preset · Extra steer | MEETINGS (3 to place / 2 added / nothing for you / no transcript) |
| Blue sky builder | Builds an idea from Todos › Blue sky when you press Enter on it: writes a brief, waits for your approval, then builds a prototype to try. | On · Preset · Extra steer | IDEAS (brief to approve / building / built). "Add ideas in Todos › Blue sky (⌘I)." |

### 5.6 Where everything lives

| Place | How to get there | What you do there |
|---|---|---|
| Grid | home | Every session; automation sessions are ordinary rows tagged with their automation |
| Needs you | ⌘K, or the footer count | Act on everything waiting; type to jump anywhere |
| Phone | Slack DM | Answer agents while away |
| Todos | ⌘I | Blue sky tab first, then a tab per project; accepted meeting takeaways land here |
| Automations | ⌘J | Each automation: on/off, preset, extra steer, what's running and what's done |
| Presets | Settings, or ⌘⇧P | What each automation says, and which account, model and effort it runs on |
| Connections | Settings, or ⌘⇧P | Slack, Microsoft 365, away and phone |

### 5.7 Settings › Projects (new rows per project)

- **Link to meeting takeaways:** on/off.
- **What it is:** one line Orion writes when the project is added. Enter edits it; `^R` writes it again.
- Explanation: "Orion writes "What it is" when you add a project, from its README and recent PRs. Edit it if takeaways land in the wrong place."

### 5.8 Meeting takeaways screen

- **Header:** `<meeting> · Thu 10:00 to 10:32 · Teams`, and below it: "3 of 6 takeaways are yours and already accepted. Check where they go, then Enter."
- **Left, FROM THE MEETING:** `✓` or `✗`, the text, and the owner's initials. Footnote: "✓ goes into your todos · ✗ left out. Ones that name you start accepted."
- **Right, WHERE THEY GO IN YOUR TODOS:**
  - the project name, then each group header in capitals with a rule
  - existing items dim, new ones highlighted green, with a linked ticket id where there is one
  - Blue sky is included
  - footnote: "Orion picked the project and group for each. m moves the selected one somewhere else."
- **Keys:** `Space accept / leave out · m move · ⌘P priority · Enter add N to todos · Esc later`.
- **Move picker:** replaces the right side.
  - Title: `MOVE "<text>" TO`.
  - A filter field, then rows of `project | group path | where it is now`, ending with `+ new group "<filter>"`.
  - Footnote: "Every project's groups are here, filtered as you type."
  - Keys: `type to filter · ↑↓ pick · Enter put it here · Esc leave it where it was`.

### 5.9 Todos › Blue sky tab

- **Tabs:** `Blue sky N` first, then the project tabs.
- **Body:** normal groups and rows. Each row shows the text, where it goes (a project name or `lab`), and its state (`● brief to approve` / `● building 18m` / `● built · proto/onboard` / `not started`).
- The DONE panel is as today.
- **Explanation:** "Enter starts the Blue sky builder on the idea. Its brief comes to Needs you to approve, and its state shows here."
- **Keys:** `⌘N new idea · w repo or lab · Enter build it · o open what it built · Space tick · Esc close`.

---

## Part 6. Manual setup the user does once

The final report repeats these steps, filled in with exact values.

1. **Slack app.**
   1. Go to api.slack.com/apps, choose **Create New App**, then **From a manifest**, and paste `docs/2026-11-07-needs-you/slack-manifest.json`.
   2. Install it to the workspace.
   3. Copy the **App-level token** (create one with `connections:write`), the **Bot token** and the **User token**.
   4. In orion, open Settings › Connections › Slack and press `c`, then paste the three tokens.
   5. Each teammate who uses orion installs their own copy, named "Orion – <name>".
2. **Microsoft 365** (as admin).
   1. In Entra, register an app "Orion": public client, "Allow public client flows" on, no redirect URI.
   2. Give it these delegated permissions: `User.Read`, `Calendars.Read`, `OnlineMeetings.Read`, `OnlineMeetingTranscript.Read.All`, `offline_access`. Grant admin consent.
   3. Put its client id and tenant id into `config.json`:
      ```json
      { "microsoft": { "client_id": "…", "tenant": "…" } }
      ```
      These ids are not secret, so teammates can use the same ones.
   4. In orion, open Settings › Connections › Microsoft 365 and press `c`.
   5. If step 4A found that attendee transcripts need it, add the application access policy it names.
3. **Teams transcription on by default.** In the Teams admin centre, open Meetings › Meeting policies and turn on transcription and "auto start transcription" for the policy your account uses.
4. **Linear.**
   - Turn on triage auto-assignment, so that ownership of a ticket follows your team's rules.
   - Choose the bug fixer's and the feature builder's labels in Automations (⌘J).
5. **Browsers.** In Settings › Accounts, set **Open links in** for each Claude account: Safari, Chrome or Firefox, as you use them today.
6. **Upgrade.** This is a minor release, so upgrading restarts the daemon and every session in it. Upgrade between tasks.
