# Draft pull requests stay quiet, and the PR fixer leaves them alone

## Problem

In the PULL REQUESTS MODAL (`⌘U`), a draft whose branch conflicts with its base, or whose checks
fail, is drawn exactly like a ready pull request in trouble: crimson `↗`, full-brightness title,
crimson `conflicts` / `failing` badge. Its `draft` identity disappears. The same happens on every
other surface that paints a PR row (band rule, `/` palette, launcher, Linear work rows), because
they all take their colours from `pr_row::look` and their word from `PrStatus::word()`.

This was a deliberate rule ("a draft's conflict still needs a person"). The rule changes now:

1. **A draft is always faint**, end to end, whatever GitHub says about conflicts or checks. Its
   badge reads `draft`. Conflicts and failing checks stay visible where there is room to say them
   quietly: the PR PREVIEW border and the `is:conflicts` / `checks:failing` filters.
2. **The PR fixer never sends an agent unasked to a draft.** A draft isn't asking anyone for
   anything yet. `⌘G` still opens the AUTOFIX MODAL for a draft by hand. Marking the draft ready
   (if it is still broken) makes it eligible on the next open-PR list.

Consequence: the bug fixer and the feature builder open **draft** pull requests
(`docs/sessions.md` § The bug fixer and the feature builder). Once this lands, those drafts are no
longer auto-fixed when they conflict or fail. They are fixed once they are marked ready, or by
`⌘G`. That is the intended behaviour, but call it out in the commit message.

## Design: where the rule lives

"Trouble" means *needs a person*. Make that rule the single source of truth:
`PrStatus::trouble()` (`crates/orion-tui/src/pr_store.rs:208`) returns `None` for a draft. Every
consumer reads it there:

- `pr_modal.rs:1823`, `pr_modal.rs:1906` (modal row + status word)
- `palette.rs:456`, `launcher.rs:182`, `linear.rs:1787` (build `trouble` into their items)
- `PrStatus::word()` (`pr_store.rs:214`) → `draft` instead of `conflicts`
- `automations/pr_fixer.rs` `note_list` (`:382`) and `open_pr_row` (`:848`, `:860`)

Health itself (`status.health.conflicts`, `status.health.checks`) is unchanged and still read
directly by the PR PREVIEW, the facet filters and `pr_fixer::diagnose`. So the facts aren't lost,
only the "needs you" alarm.

`pr_row::look` also stops letting trouble outrank a draft, so a caller that builds `trouble` from
somewhere else still can't paint a draft red.

## Steps

### 1. `PrStatus::trouble()` skips drafts: `crates/orion-tui/src/pr_store.rs`

```rust
/// What the row goes red for: an open pull request that is asking for a
/// review. A merged or closed one is past needing its branch resolved,
/// and a draft isn't asking anyone yet: its conflicts and checks are
/// still in `health` for the surfaces that spell them out quietly (the
/// PR PREVIEW, the modal's filters), never the needs-you red.
pub fn trouble(&self) -> Option<Trouble> {
    (self.standing == Standing::Open).then(|| self.health.trouble()).flatten()
}
```

Update `word()`'s doc comment to match. `is_open()` is unchanged (a draft is still open for
everything else: merge form, ready/draft toggle, `⌘G`).

### 2. `pr_row::look`: a draft is faint whatever: `crates/orion-tui/src/pr_row.rs`

Change the guard at the top of `look` from `standing.is_open()` to `standing == Standing::Open`:

```rust
if trouble.is_some() && standing == Standing::Open {
```

Rewrite the last paragraph of the doc comment: trouble applies only to a pull request that is
ready for review. A draft keeps its faint, since it isn't asking anyone yet, and a merged or closed
one is past resolving.

### 3. Modal row: title and marks: `crates/orion-tui/src/pr_modal.rs`

- `row_line` (`:1865`): the title is faint for any draft:
  `let title_color = if stands.is_draft() { th.faint } else { th.text };`
  Update the doc comment above `row_line` (`:1797`) if needed ("the title (faint for a draft)" is
  already right).
- `status_spans` (`:1905`): on a draft, the checks mark and the review mark keep their glyphs
  (`✓ ◐ ✗ ○`) but take `th.faint` instead of `ok` / `warn` / `err` / `muted`. So nothing on a
  draft row is coloured. Implement with one `let tint = |c| if status.is_draft() { th.faint } else { c };`
  applied to both marks. Update its doc comment.

The `is` facet (`facet_values`, `:676`) reads `status.health.conflicts` directly, so
`is:draft is:conflicts` still finds a conflicting draft. Leave it as it is.

### 4. PR PREVIEW border: `crates/orion-tui/src/pr_preview.rs`

`state_word` (`:591`) currently paints a conflicting draft `● Conflicts` in `th.err`. Change it so
a draft says both, quietly:

```rust
match status.standing {
    Standing::Draft if status.health.conflicts => ("○ Draft · conflicts".into(), th.muted),
    Standing::Draft => ("○ Draft".into(), th.muted),
    Standing::Open if status.health.conflicts => ("● Conflicts".into(), th.err),
    Standing::Open => ("● Open".into(), th.ok),
    Standing::Merged => ("● Merged".into(), th.merged),
    Standing::Closed => ("● Closed".into(), th.faint),
}
```

Update the doc at `:538` that lists the border words. Check that whatever lays out the border
leaves room for the longer string. If it is width-budgeted, fall back to `○ Draft` when it doesn't
fit.

### 5. PR fixer leaves drafts alone: `crates/orion-tui/src/automations/pr_fixer.rs`

- **`note_list`** (`:355`): after the `is_green` block and before the existing-watch block, drop
  a draft:

  ```rust
  // A draft isn't asking for anything yet: no unasked fixer. `⌘G`
  // still opens the form for it, and marking it ready brings it back.
  if status.is_draft() {
      app.autofix.forget(&pr.url);
      continue;
  }
  ```

  This also drops a watch started before the PR went back to draft. (With step 1, the
  `trouble().is_none()` check would already skip a *new* draft, but not an existing watch.) Leave
  `attempts` as they are. A draft that goes ready again picks up where it left off, and the ledger
  fingerprint still stops a repeat of an already-handled breakage.
- **`land_detail`** (`:466`): a body can land for a watched PR that has just turned draft before
  the next list does. After `let status = …` (`:484`), add:

  ```rust
  if status.is_draft() {
      app.autofix.forget(url);
      return;
  }
  ```

  Put it **after** the `pending_open` / `open_pending` block at the top, so `⌘G` on a draft still
  opens the form.
- **`open_pr_row`** (`:836`): the panel's YOUR OPEN PRS row for a draft would now fall through to
  `watching`, which isn't true. Add an arm before `is_green`:
  `None if status.is_draft() => ("draft, left alone".into(), Tone::Quiet, None),`
  (A fixer already running on it keeps its `fixing …` row; that arm comes first.)
- Update the module doc (`:1`) "Seeing it break" bullet: *a pull request the `gh` user opened that
  is ready for review and in trouble is watched; a draft is left alone until it is marked ready,
  `⌘G` aside.*

### 6. Tests

Update the tests that encode the old rule, and add the new ones:

- `pr_row.rs` `a_pull_request_in_trouble_wears_the_needs_you_red` (`:164`): replace
  `assert_eq!(look(Standing::Draft, Some(trouble), th), red)` with
  `assert_eq!(look(Standing::Draft, Some(trouble), th), look(Standing::Draft, None, th), "{name}: a draft stays faint")`.
  Update its doc comment.
- `pull_request.rs` `trouble_is_an_open_pull_requests_and_conflicts_come_first` (`:2622`): the
  final assertion becomes `assert_eq!(trouble(&pr), None, "a draft isn't asking anyone yet")`,
  and a further `pr.answered_draft = false` → `Some(Trouble::Conflicts)` shows it comes back once
  ready. Update the doc comment (`:2617`).
- `pr_store.rs` tests: add `a_drafts_word_is_draft_whatever_its_health`. A draft status with
  `health.conflicts = true` has `trouble() == None` and `word() == "draft"`, and the same status as
  `Standing::Open` has `word() == "conflicts"`.
- `pr_modal.rs` tests (module at `:2355`, helper `pr(number, title, is_draft)` at `:2365`): add a
  row test. A conflicting draft's `row_line` spans have the `↗`, the title, both marks and the
  `draft` word all in `th.faint`, and no span in `th.err`.
- `pr_preview.rs` (`:2531` area): a conflicting draft's `state_word` is
  `("○ Draft · conflicts", th.muted)`. The existing open-conflicts case stays `● Conflicts` /
  `th.err`.
- `pr_fixer.rs` tests (use `app_with`, `broken`, `listed`, `landed`, `sent_prompt`):
  - `a_draft_is_never_fixed_unasked`: `broken(true, true)` with `answered_draft: true` → after
    `listed`, `watching` is empty. Flip `answered_draft` to `false`, `listed` again → watched, and
    `landed` with a conflicting detail sends a prompt.
  - `a_pr_turned_draft_drops_its_watch`: watch a ready broken PR, then list it again as a draft →
    `watching` is empty. Separately, a body landing for a watched PR whose status is now draft
    sends nothing and forgets the watch.
  - `cmd_g_still_opens_the_form_for_a_draft`: with `pending_open` set to a draft's URL,
    `land_detail` opens the AUTOFIX MODAL (`app.overlay` is the form).
  - Panel: a draft's `open_pr_row` state is `draft, left alone`.
- `grep -rn 'draft' crates/orion/tests/e2e_tui.rs` and fix any e2e expectation of a red draft row.

### 7. Docs

- `docs/sessions.md:654`: "a draft dimmed with a `draft` badge, one GitHub says cannot merge red
  end to end…" → add that a draft stays dimmed and badged `draft` even when it conflicts or
  fails, the page's border saying `○ Draft · conflicts`.
- `docs/how-it-works.md:472`: same correction ("one GitHub says cannot merge" → "an open, ready
  one GitHub says cannot merge…", and a draft stays dimmed).
- `docs/sessions.md:743` (§ The PR fixer): "When one of your pull requests…" → "When one of your
  pull requests that is ready for review hits…". Add: "Drafts are left alone, including the drafts
  the bug fixer and feature builder open, until they are marked ready. `⌘G` still sends one by
  hand."

## Verification

```sh
cargo test -p orion-tui pr_row
cargo test -p orion-tui pull_request::tests::trouble
cargo test -p orion-tui pr_store
cargo test -p orion-tui pr_modal
cargo test -p orion-tui pr_preview
cargo test -p orion-tui pr_fixer
cargo test -p orion-tui
cargo test -p orion --test e2e_tui
cargo clippy --workspace --all-targets -- -D warnings
```

By hand: run orion on a repo with a conflicting draft PR, open `⌘U`. The draft row is faint end to
end with `draft` and faint marks. The preview border reads `○ Draft · conflicts`. `is:conflicts`
still lists it. `⌘J` shows it as `draft, left alone`, and no `autofix-<n>` session starts. `⌘G` on
it still opens the AUTOFIX MODAL.

## Release

Every change is in `crates/orion-tui` and `docs/`, none of it in
`crates/orion-daemon/daemon-inputs.txt`, so this is a **patch** release. No major: nothing anyone
relied on is removed (`⌘G` still covers drafts), though the commit message should say that drafts
are no longer auto-fixed.
