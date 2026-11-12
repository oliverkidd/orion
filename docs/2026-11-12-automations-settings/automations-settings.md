# Automations move into Settings, with a cap per automation; ⌘J shows what's running

## Context

Feedback after a day running the automations on v3.4.2:

- **Too many sessions at once.** The one cap (`automations.max_running`) was stepped up to 8 with `←→` in ⌘J's header. That's hard to find, and too blunt: 8 sessions, each cutting a worktree and building, slow the laptop to a crawl. People want the cap in Settings, and want each automation capped on its own, e.g. at most one bug fixer.
- **Config in the wrong place.** Every automation's settings live in the ⌘J modal: on/off, labels, when, whose, preset, extra steer. Everything else orion configures lives in Settings (⌘,). ⌘J should be an overview of what the automations are doing, and Settings should be where they're set up.

What we want:
- A Settings › **Automations** tab that holds every automation setting, including an overall cap and a per-automation cap.
- A lower default overall cap.
- ⌘J reduced to a running overview.

## Design

### Settings › Automations (new tab, between Presets and Connections)

```
AUTOMATIONS
  Sessions at once, all automations     3          ←→ 1–20
  (dim) Waiting-on-you sessions hold a place too, so approving or closing them frees it.

PR FIXER                                 · running 1 of at most 1
  On                                     on
  At most at once                        1          ←→ any, 1–10
  Preset                                 PR autofix · the PR's own agent
  Extra steer                            …
BUG FIXER                                · 1 plan waiting
  On / At most at once / Labels / Preset / Extra steer
FEATURE BUILDER
  On / At most at once / Labels / Preset / Extra steer
WEEK IN REVIEW
  On / When / Whose / Preset / Extra steer          (runs one session; no cap row)
MEETING TAKEAWAYS
  On / Preset / Extra steer                          (one-shot model calls; no cap row)
BLUE SKY BUILDER
  On / At most at once / Preset / Extra steer
```

- Rows come from each runner's `settings()` (`crates/orion-tui/src/automations/mod.rs:232`, `Automation::settings` at ~l.111), with a new `SettingRow::Cap` added to PR fixer, Bug fixer, Feature builder and Blue sky.
- Editing reuses the runner's `edit`, `text_of` and `set_text`, so the tab and ⌘J can't drift apart:
  - Enter edits a text row.
  - `←→` changes a value.
  - Enter on Preset opens it in Settings › Presets, as ⌘J does now.
  - `⌘U` picks another preset.
  - Enter on Labels opens the Linear label picker.
- The section header's right side shows the live state, from `Runner::panel`'s state line, e.g. "fixing #407" or "1 plan waiting".
- Keys line: `↑↓ row · ←→ change · Enter edit · Space on / off · ⌘U pick another preset · Tab next tab · Esc close`.

### Caps

- `AutomationsSettings` (`crates/orion-tui/src/automations/config.rs`):
  - `DEFAULT_MAX_RUNNING` drops from 5 to **3**. A saved value is kept as it is, so this config's 8 stays at 8 until it's changed in the new tab.
  - `PerAutomation` gains `max_running: Option<u8>`. None means "any": only the overall cap applies.
  - `step_cap` moves to a generic stepper, used by both rows.
- `crates/orion-tui/src/automations/mod.rs`:
  - `running(app)` stays as it is.
  - Add `running_of(app, which)`, which counts the tagged sessions plus the `launching` entries of that automation. `launching` needs to record which automation each launch is for, if it doesn't already.
  - `may_start(app)` becomes `may_start(app, which)`: `running(app) < cap()` **and** `running_of(app, which) < per cap`.
  - Update the four callers to pass their automation: `pr_fixer.rs:539`, `linear_builder.rs:503` (bug fixer or feature builder, whichever is launching), `blue_sky.rs:228` and `:298`.
  - Both `launch` functions pass `which` through.
- Defaults for the per-automation cap: PR fixer **1**, Bug fixer **1**, Feature builder **1**, Blue sky **any**. These apply only when the entry has no saved `max_running`.

### ⌘J: the running overview

⌘J keeps its key and becomes read-mostly:

```
Automations                                   2 of at most 3 running · caps in Settings
● PR fixer          fixing #407 · try 2 of 3       web       12m
● Bug fixer         ENG-412 plan to approve        api       40m
○ Feature builder   queued: ENG-399 (at its cap)
  Week in review    next Fri 09:30
  Meeting takeaways 3 to place
  Blue sky builder  off
```

- One row per automation, with its dot (`Dot`) and state.
- Under each automation, its live sessions: the session, project and age. The data comes from `Runner::panel` (its RUNNING lists), so no new bookkeeping is needed.
- **Keys:**
  - `↑↓` move
  - Enter on a session row jumps to it, as the panel's session rows do today
  - Enter on an automation row opens Settings › Automations at that section
  - Space turns it on or off, as a quick switch
  - `,` opens Settings › Automations
  - Esc goes back
- The cap's `←→`, the SETTINGS block and `⌘U` are removed from ⌘J.
- Week in review's `r run now` and the REVIEWS list stay reachable:
  - Enter on the Week in review row opens its latest review.
  - `r` runs it now. It's also in ⌘⇧P as today.
- Meeting takeaways: Enter on its row opens the MEETINGS list, and Enter there opens the takeaways screen as now.

### Code layout

- `crates/orion-tui/src/config.rs`:
  - Add `TabBody::Automations` and a `SettingsTab { title: "Automations", body: TabBody::Automations }` between Presets and Connections in `SETTINGS_TABS` (~l.574).
  - Rendering and key handling go in a new `crates/orion-tui/src/automations/settings_tab.rs`, the way `TabBody::Presets` and `TabBody::Accounts` have their own drawers.
  - Fix the tab-count tests near l.5463.
- `crates/orion-tui/src/automations/view.rs`:
  - Strip SETTINGS, the cap and the preset picker out of it.
  - Keep the dot, value and list drawing helpers, shared with `settings_tab.rs`.
  - Update its header comment and the `keys` module.
- Add a way to open Settings at a tab and section, e.g. `open_settings_at(app, "Automations", Some(which))` beside `open_settings` (`crates/orion-tui/src/event_loop.rs:9006`).
- INPUT PARITY tables in `event_loop.rs`: update the Automations overlay's entries, and add the new tab's click-equals-key rows.
- Docs:
  - `docs/keys.md`: ⌘J and the new tab's keys.
  - `docs/configuration.md`: `automations.max_running` default 3, `automations.per.<id>.max_running`.
  - `docs/sessions.md` and README, wherever they describe ⌘J as the place to configure.

No daemon change, so this is a patch release.

## Verification

```sh
cargo test -p orion-tui automations
cargo test -p orion-tui config
cargo test -p orion --test e2e_tui
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
make shot SCENE=automations          # update the scene to the overview
```

New unit tests:
- The per-automation cap stops a second bug fixer launch while the overall cap still has room. The overall cap stops a launch while the automation's own cap has room.
- A saved `max_running` of 8 survives a load and save. A missing one reads as 3.
- The settings tab lists each automation's rows in `settings()` order, plus Cap where it applies.
- Editing a row in the tab changes the same config key that the old ⌘J row did (Labels, When, Extra steer).
- ⌘J Enter on an automation row opens Settings at that section. Enter on a session row jumps to the session.

By hand:
- Set the Bug fixer to at most 1 and label two tickets. One session should start, and ⌘J should show the other as `queued (at its cap)`.
- Set the overall cap to 2 with three automations busy. Only two sessions should run.

---

## Also: the Needs-you count moves to the projects header

- The left-hand projects panel's header shows the total session count at its top right.
- Replace that count with the Needs-you total, e.g. `⌘K 4` in the red "waiting on you" colour. Show nothing when it's 0.
- Clicking it opens the board, the same as ⌘K.
- Remove the session count from that header. The footer's bottom-right agents and GB readout stays as the overall measure.
- Drop the footer's `⌘K needs you N` item, so the number shows in one place.
- Update the screenshot scenes that show the header, `docs/keys.md` and the README grid scene if they mention either count.
