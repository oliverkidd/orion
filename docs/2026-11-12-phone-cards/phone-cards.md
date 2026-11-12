# Better phone cards in Slack, and a LINEAR column of live tickets

## Context

The first real test of the phone bridge (2026-11-12, on v3.4.2) turned up these problems:

1. **Answering in your own words is clumsy.** `Something else…` only posts an ephemeral "Reply in this thread" hint. On a phone that hint is easy to miss, and opening the thread is a second hunt.
2. **Multi-select questions send only one pick.** Each option is a plain button, so the first tap answers. `answer.rs` then types that digit, `→` and `1`, which submits a single option.
3. **A question with several parts gets stuck.**
   - `WaitReason::Question` keeps only `questions[0]`, so the card shows question A alone.
   - Tapping an option answers A, and the phone message is marked answered.
   - Claude Code's dialog moves on to question B, but the session stays `needs_feedback`, so no new `StatusChanged` arrives and no new card is posted.
   - Every later reply then gets "This one was already answered", and the session sits on question B until someone answers it at the desk.
4. **The LINEAR column in Needs you is wrong in both directions.**
   - It lists unread *notifications*, whatever state their ticket is in, so done and cancelled tickets show up.
   - It leaves out the live tickets assigned to you that have no unread notification.

What we want:
- One tap where one tap will do.
- A text box right in Slack for your own words, with no thread hunting.
- Every part of a question, and every pick of a multi-select, answered in one go.
- A plain DM still answers the latest open card.
- LINEAR shows the tickets you'd act on, in the states orion already treats as live.

## Design

### The card (Slack message, bot DM)

Every card keeps the 5.4 top part: the context line, the session name as a header, and the full last response.

**Single question, single select.** This keeps today's one-tap buttons.
- One button per option.
- Then `Type an answer…`. It opens a modal: the question as its title, a multiline `plain_text_input`, and Submit. Submit sends `Answer::Text`.
- A context line under the buttons: `or just reply here`. A plain DM already answers the latest open card (`Target::Latest`), so this only says so.

**Multi-select, or several questions in one ask.** The message body shows every question in bold, each followed by its options as a dim list, so the question can be read without opening anything.
- One primary button, `Answer…`. It opens a modal with one block per question:
  - single select: a `radio_buttons` element
  - multi-select: a `checkboxes` element
  - under each, an optional `plain_text_input` labelled "Or in your own words". If it's filled in, it wins over the picks for that question.
- Submit sends the whole set as one answer.
- If you never open the modal, a plain DM or a thread reply still answers in words, as today. It goes in as the session's next turn, with the questions quoted back.

**Plan.**
- `Approve` (primary).
- `Suggest changes…`, in place of today's reply-icon button. It opens a modal with a multiline input and sends `Answer::Steer`.
- A thread reply or plain DM still steers.

**After any answer.** `chat.update` swaps the buttons for a context line that says what went in, e.g. `✓ Sent: Red, Blue · working again`. Nothing is posted in the thread. "Answered in Orion" stays as it is.

**A card that's already been answered.** Its buttons are gone, so this can't come up from a tap. A stale modal submit gets `response_action: errors` with "This one was already answered", shown inside the modal.

### Why modals

- They're Slack's supported way to collect structured input from a button, and they work the same on mobile and desktop.
- They need no new OAuth scope. `views.open` runs with the bot token and the press's `trigger_id`. Interactivity is already on in the manifest.
- `checkboxes` and `radio_buttons` inside a *message* are less reliable: their `state.values` reaches a later button's payload unevenly across clients. Inside a *modal*, `state.values` arrives in full on submit.

### Daemon changes

1. **`orion_core::entities::WaitReason::Question`** (`crates/orion-core/src/entities.rs:326`) carries every question:
   ```rust
   Question { questions: Vec<Asked> }   // Asked { question, header, options: Vec<String>, multi }
   ```
   - `WaitReason::question` (`entities.rs:371`) reads all of `questions[]`.
   - Update every match on it: `answer.rs`, `transcript.rs`, `phone.rs`, `crates/orion-tui/src/needs_you/agents.rs` (which shows the first question's text), and `crates/orion/tests/e2e_pty.rs`.
   - This changes the wire protocol, so bump the protocol number (`crates/orion-core/src/protocol.rs`).
2. **`orion_core::protocol::Answer`** (`protocol.rs:452`) gains:
   ```rust
   Answers(Vec<Picked>)   // Picked::Options(Vec<usize>) | Picked::Text(String), one per question
   ```
   - `Option(i)` and `Text` stay, for the one-tap and plain-DM paths. On a several-question ask they answer question 1 only, then fall back to words (see 4).
3. **`answer.rs::dialog_keys`** learns the multi-question dialog: a tab per question, then the review tab's `1. Submit answers`.
   - **Verify the exact keystrokes against a real `claude` in a PTY before writing them**, as step 1B did (record the version in the module header). Expected, to be confirmed:
     - single select: the digit picks and moves to the next tab
     - multi-select: each digit toggles, then `→` to the next tab
     - own words: the `N+1` digit focuses the text row, then the text, then Enter, which moves on
     - after the last tab: `1` on the review
   - Keep one write per keystroke, `KEY_GAP` apart.
4. **Any answer that doesn't cover every question** (e.g. a plain DM on a three-part ask): type nothing into the dialog. Press Esc to cancel it, then send the words as the next turn through `send_turn`, with the questions quoted:
   ```
   You asked:
   1. <q>
   2. <q>
   My answer: <words>
   ```
   This guarantees a session can never be left half-answered on a later tab, which is the bug in problem 3.
5. **`phone.rs`**:
   - `build` lays out the three card shapes above.
   - Add the action ids `orion_answer_modal`, `orion_type` and `orion_steer`. Keep `orion_option_<i>` and `orion_approve`.
   - Remove `Reply::Hint`, `REPLY_HINT` and the `post_ephemeral` call.
   - Add `pub fn modal_for(message: &PhoneMessage, reason: &WaitReason, action: &str) -> Value`. It builds the view, with `private_metadata = "<channel>:<ts>"`, so the submit finds its message.
   - Add `pub fn route_submission(payload, session) -> Option<Route>`. It reads `view.state.values` into `Reply::Answers(Vec<Picked>)`, `Reply::Words` or a steer.
   - After an answer goes in, update the card (`answered_with(blocks, summary)`), in place of `say(SENT, thread)`. An error still posts `✗ Not sent: …` in the thread, because the card has no room for it.
   - The phone message must hold the full `WaitReason`, so a modal can be built after a daemon restart. Add a `reason` JSON column to `phone_messages`: a new store migration in `crates/orion-daemon/src/store.rs`, which is daemon code and already in `daemon-inputs.txt`.
6. **`slack/mod.rs::handle`** (`crates/orion-daemon/src/slack/mod.rs:291`), on `interactive`:
   - `block_actions` with a modal action: `api.views_open(trigger_id, view)` straight away. The `trigger_id` lasts 3 seconds, so open the modal before any store or Slack call that can wait.
   - `view_submission`: route it and answer it.
   - **The ack for `view_submission` is the modal's reply.** Today `socket.rs` acks every envelope empty before handing it on, which closes the modal. That's fine for a good submit. For a stale submit, validate synchronously before acking, i.e. look the message up in the store, and ack with `{"envelope_id", "payload": {"response_action": "errors", "errors": {"<block_id>": "This one was already answered"}}}`. That means `socket::session` takes an `on_envelope` that can return an ack payload, for interactive `view_submission` only.
7. **`slack/api.rs`**: add `views_open(trigger_id, view)`, a bot-token call to `views.open`, beside `post` and `update`.
8. **Manifest** (`docs/2026-11-07-needs-you/slack-manifest.json`): no new scope or event is needed. Note in the build log that the existing install keeps working.

### LINEAR column

The column shows **one card per live ticket of yours**, drawn from two sources and merged by issue id.

**Source A, new: tickets assigned to you in a live state.**
- These are `triage` (Triage), `unstarted` (Todo) and `started` (In Progress, In Dev, In Review and the like).
- `backlog`, `completed` and `canceled` are left out. Duplicate is a `canceled` state, so it's left out too.
- Query, per distinct key (`crate::linear::keys_by_project`, `crates/orion-tui/src/linear.rs:6232`):
  ```graphql
  mine: issues(first: 50, orderBy: updatedAt,
    filter: { assignee: { isMe: { eq: true } },
              state: { type: { in: ["triage", "unstarted", "started"] } } }) { nodes { <ISSUE_FIELDS> } }
  ```
- Reuse `ISSUE_FIELDS` (`linear.rs:5572`) and `crate::linear::parse_issues` (`linear.rs:6070`), so each card has a `LinearIssue` with `status`, `status_type` and `state_color`.
- Name the state list once as `pub(crate) const LIVE_STATES` in `linear.rs`, next to `OPEN_STATES` (`linear.rs:5595`) and `WAITING_STATES` (`automations/linear_builder.rs:161`).
- Send it in the same request as the notifications query, as a second alias, so it costs no extra call.

**Source B: unread notifications.**
- These are mentions, comments and assignments, as in `crates/orion-tui/src/needs_you/linear.rs` today, already filtered to drop done or cancelled tickets and snoozed notes (uncommitted change in the working tree).
- Narrow it to the same `LIVE_STATES`, so a mention on a backlog ticket doesn't show either. Swap `CLOSED_STATES` for a check that the state is in `LIVE_STATES`.

**The card.**
- Title: `ENG-12 Fix login`.
- Second line: `<project> · <status name>`. If the ticket has unread notifications, add the newest one's wording, e.g. `web · In Progress · mentioned you`.
- The glyph comes from `crate::linear::state_glyph` (`linear.rs:4750`).
- Keep skipping tickets the bug fixer or feature builder has picked up (`linear_builder::picked_up`). That now applies to Source A too, not only to `issueAssignedToYou` notes.

**Order.**
- Cards with unread notifications first, newest first.
- Then the rest, by `status_rank` (`linear.rs:6203`): triage, then todo, then in progress.
- Do this by setting `Card` urgency: 2 for notified, 1 for triage, 0 for the rest. `model.rs:197` sorts by urgency, then by age.

**⌘D.**
- On a card with notifications: as today, which marks them read.
- On a card without: just the board's own done store. The card's `since` is the issue's `updatedAt`, so the card comes back when the ticket changes.

**The gate.** The poll runs while `inbox.linear_mentions` is on (`needs_you/mod.rs:168`). Rename the Connections row from "Linear comments that mention you" to "Linear tickets and mentions" so it covers both sources. The config key stays the same.

### SLACK column: Enter clears the card, and you can reply from orion

**Why cards don't clear.**
- A Slack card goes away only when orion hears your own later message in that conversation (`Waiting::answered_by`, `crates/orion-tui/src/needs_you/slack.rs:149`).
- Pressing Enter just opens the permalink (`crates/orion-tui/src/event_loop/activate.rs:102`). If you then reply in Slack while orion is closed, or your message isn't heard for any other reason, the card stays.
- In this install's `needs-you.json`, one DM (`D0B7JJXRTP1`) has two waiting cards and none of your own messages ever recorded. So in at least that conversation your replies aren't reaching the board.

**Changes:**

1. **Enter marks the card done, then opens Slack.**
   - Call the board's existing done path (`needs_you/mod.rs::done`, as ⌘D does) before `open_link`.
   - The done store already brings a card back when a newer message arrives (`since` gets later), so nothing new can be missed.
2. **`r` replies from orion.** On a Slack card, `r` opens a reply panel in place of the board's right two columns:
   - Header: `#channel` or `DM · <name>`, and "thread" if the card sits in a thread.
   - The conversation's last 6 messages, from `conversations.replies` for a thread or `conversations.history` for a top level. These use the user token through the existing `call` helper (`needs_you/slack.rs:515`), and need no new scope.
   - A multiline `TextInput`. `Enter` sends, `⇧Enter` adds a new line, and `Esc` cancels with the draft kept.
   - Sending uses `chat.postMessage` with the **user token**, so it goes as you, with `thread_ts` set for a thread. It's off the event loop, as `call` is.
   - After the send, the card is removed at once, and the flash says `Sent to #channel`. The echoed message event then also clears it through `answered_by`.
   - Keys line: `Enter send · ⇧Enter new line · Esc back`.
   - Add the key to the board's keys line: `r reply`.
3. **Manifest:** add the user scope `chat:write` to `docs/2026-11-07-needs-you/slack-manifest.json`.
   - Existing installs must reinstall the app, then paste the new `xoxp-` user token.
   - Until then, a send that fails with `missing_scope` shows: "Reply needs Slack's chat:write. Reinstall Orion from the manifest and paste the new user token in Settings › Connections."
4. **Find out why your own messages go unheard in `D0B7JJXRTP1`.**
   - Add a `tracing::debug!` in the daemon's `inbox_event` (`crates/orion-daemon/src/slack/mod.rs:326`) that logs each message's channel, user and subtype, but not its text.
   - Reply in that DM from Slack and check whether the event arrives at all.
   - If it doesn't arrive, Slack isn't sending your own messages for that conversation to the user-token subscription, and items 1 and 2 cover it.
   - If it does arrive, fix the rule that drops it, and add a test for it.

Tests:
- Enter on a Slack card marks it done, and it returns on a newer message.
- `r`, typing and Enter send one `chat.postMessage` with the right channel and `thread_ts`, then remove the card.
- `missing_scope` gives the reinstall message.
- Click parity: a click on the reply panel's Send is the same as Enter.

## Files

- `crates/orion-core/src/entities.rs`, `crates/orion-core/src/protocol.rs`
- `crates/orion-daemon/src/answer.rs`, `phone.rs`, `slack/mod.rs`, `slack/socket.rs`, `slack/api.rs`, `store.rs`, `transcript.rs`
- `crates/orion-tui/src/needs_you/agents.rs`, `crates/orion-tui/src/needs_you/linear.rs`
- `crates/orion/tests/e2e_pty.rs` (fake Slack: views.open and view_submission)
- Docs: `docs/sessions.md` (the phone section), `docs/how-it-works.md` if it describes the replies.

## Verification

```sh
cargo test -p orion-core -p orion-daemon -p orion-tui
cargo test -p orion --test e2e_pty a_waiting_session_reaches_the_phone_and_the_phone_answers_it
cargo test -p orion --test e2e_pty            # protocol and daemon changed
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

New unit tests:
- `build` gives each of the three card shapes.
- `modal_for` gives radios for single select, checkboxes for multi-select, and one input per question.
- `route_submission` reads checkboxes, radios and text, with text winning over picks.
- `dialog_keys` gives the multi-question keystrokes recorded from the real PTY.
- A partial answer gives Esc plus a quoted next turn.
- A stale submit acks with `errors`.
- LINEAR:
  - An assigned triage, todo or in-progress ticket with no notification gets a card.
  - Backlog, done and cancelled tickets get none, from either source.
  - A ticket with both an assignment and a mention is one card, ordered first.
  - A ticket the builder picked up gets no card.

New e2e_pty test against the fake Slack: a two-question ask (one single select, one multi-select) gets answered through a `view_submission`, and both answers reach the PTY's dialog.

By hand, on a phone, press ⌘⇧A to go away, then have a session:
1. ask a single question: tap an option, then on a second ask use `Type an answer…`
2. ask a multi-select question: tick two options, Submit
3. ask two questions in one go: answer both in the modal
4. show a plan: `Suggest changes…`, then Approve on the next one
5. ask a question you answer with a plain DM

In each case the card updates to `✓ Sent: …` and the session carries on.
