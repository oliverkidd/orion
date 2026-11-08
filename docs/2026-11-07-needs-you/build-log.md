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

- **baseline** · 1e68748 · e2e_tui 14/14 and clippy `-D warnings` clean on needs-you · five e2e_tui tests and one clippy lint were already broken on `main` by #42 (selection fill 236/234, ✶ spinner, no "Review #N" heading; `mention.rs` needless borrow). Fixed here. `cargo fmt --check` also fails on `main` under rustc 1.99's rustfmt in files no step touches; the wave gates check fmt only on files each step changed, and 5A formats the workspace once.
- **2E** · cfade52 · unit tests for every browser and profile command line, the grep test that `open_url.rs` is the only `open` caller, config round trip; `open -a` run by hand for Safari, Chrome (+profile) and Firefox (+profile) · Arc not installed, so untested. `-na … --args` is used for profiles (the plan said `open -a`), because `--args` reaches only a newly launched app.
- **1A** · 8fdd987 · unit tests for oneshot, secrets (a real Keychain round trip), http, the needs-you store and the config round trip; orion-tui builds with 2E · `secrets::set` changed in review to send the command to `security -i` on stdin, so a token never appears in `ps`. `oneshot::account_for` returns `Option<Account>`.
- **1B** · see `git log` ("The daemon knows why a session waits…") · WaitReason parse, transcript fixtures, keystroke tables; new e2e_pty test that a question hook carries its reason and an answer types into the dialog · checked against a real `claude` 2.1.319 in a PTY: plan approval arrives as both `PreToolUse ExitPlanMode` and `PermissionRequest`, and both are kept; `ExitPlanMode` clears like a permission prompt, not like `asks_user`. Keystrokes: a digit picks a single-select option, N+1 then text is "own words", `1` approves a plan, `↑` then text steers it, and each key is a separate write 300 ms apart. The code is in a new `answer.rs`.
- **Wave 1 gate** · fmt clean on every changed file · clippy `-D warnings` clean · core 74, daemon 357, fuzzy 16, tui 1967 · e2e_tui 14, help_cli 2, config_cli 6, install_sh 10 · e2e_pty 32/32. All pass.
- **2D** · see `git log` ("Microsoft 365 signs in from Connections…") · unit tests for the device-code states and AADSTS parsing, the calendar parse, footer text, the `about` prompt, config round trip and the c/o/Enter keys; the real device-code endpoint with a bogus client id gives AADSTS700038, shown as "Microsoft doesn't know microsoft.client_id…"; a real "What it is" run on orion wrote a correct line · the Microsoft email shows in the Status row, not after the section header. Added `oneshot::default_account` and `ORION_PROJECT_ABOUT=off`, which e2e and the shot/perf scripts set. There is no client id yet, so a real sign-in waits for Part 6.
