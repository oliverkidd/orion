# Needs you: build log

## Kickoff decisions (settled with the user on 2026-11-07)

1. **Keys.**
   - ⌘K becomes the Needs-you board. Typing still jumps anywhere, as today.
   - Automations gets ⌘J. `toggle_launcher_pane`, which holds ⌘J today, moves to ⌘⇧J.
   - ⌘⇧Y (week review) is removed, so week in review lives only in Automations and ⌘⇧P.
   - ⌘⇧A toggles "away by hand".
2. **Release size: minor.**
   - The daemon changes make it minor automatically.
   - No major bump: nothing is dropped outright, since week review and the pane toggle only move.
   - Leave `[workspace.package] version` in `Cargo.toml` alone.
3. **Blue sky folder.** Blue sky ideas that aren't tied to a repo are built in `~/Code/orion-blue-sky`, created as a git repo on first use and added to orion as a project. Never call it "lab", in code, settings or the UI. Its target is spelled `blue sky`.

## Steps

Each line: step id · commit sha · what was verified · deviations from the plan.

