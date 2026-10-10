# My week in review

A window in orion that writes up what was finished in the last 7 days — merged pull requests, Linear issues done, todos ticked — as a short review to read aloud to a team with the product on screen: what changed, by product area, with what to show and what to say.

Design mock (private to Oliver until shared): https://claude.ai/artifact/DpAV2DLa7XRjGfte3oMvmW

Status: built on 2026-10-10 as `crates/orion-tui/src/week_review.rs`, from this plan. The layout, the prompt and the way it runs were settled first by running them against a real week (162 merged pull requests, four authors); the numbers below are from those runs. The names and pull request numbers in the examples are made up. "As built" at the end lists where the code departs from the plan.

## Why

orion already knows most of what was finished in a week, but in separate windows:

- The PULL REQUESTS MODAL lists every pull request merged in the last 7 days (`OpenPrs::merged`, `pull_request::MERGED_DAYS`).
- The LINEAR VIEW lists issues done in the last 7 days (`linear::DONE_DAYS`).
- The TODOS file records when each item was ticked (`todos::store::Item::done`).

Nothing joins them, and none of them says which changes matter. Gathering the list is orion's job; choosing the dozen-odd things worth saying aloud, and how to demo them, is a model's.

## How it works, in one pass

1. `⌘⇧Y` (or **My week in review** in the COMMAND PALETTE) opens the WEEK IN REVIEW modal for the selected project. orion starts fetching the merged pull requests' descriptions at once, so they are usually in hand before `Enter`.
2. The left panel sets the scope and sources, with live counts; the right panel lists exactly what would be sent.
3. `Enter` sends one request to a model, with everything it needs in the prompt. There is no agent session, no tools and nothing run in the checkout: orion sends text and gets text back.
4. The reply is checked, saved as a Markdown file in orion's data dir, and shown rendered. `⌘C` copies it.

Measured: about 14 s to fetch 162 descriptions and 40 to 50 s to write, so under a minute from `Enter`, and less when the fetch finished while the Compose screen was up.

## The three states of the modal

One overlay, `Overlay::WeekReview`, a split modal like the PULL REQUESTS MODAL (`ui::SPLIT_MODAL_PCT`, list on the left, page on the right).

### 1. Compose

Left panel, top to bottom:

- The period: `Last 7 days` and its dates. Fixed in the first version.
- **Scope**, two rows:
  - `Whose`: `Just mine` or `Everyone`. With `Everyone`, each item carries who did it, and the review names them.
  - `Where`: `This project` or `All projects`.
- **Sources**, a checklist with a count each: `Merged pull requests`, `Linear issues done` (hidden without a `LINEAR_API_KEY`), `Todos ticked` (hidden without a todos file; left out of `Everyone`, since todos are yours alone).
- **Agent**: one row, the model and effort. Read-only; changed in Settings.
- **Note**: one free-text field, sent verbatim ("lead with the new editor; skip layout tweaks"). It is remembered per project, so a standing instruction is typed once.

Right panel: every item that would be sent, grouped by source under section rules (`ui::section_rule`), each with how long ago it finished. A pull request row says which branch it merged into (`dev`, `main`), so it is clear where a change can be demoed.

Keys: `↑`/`↓` walk the rows, `Space` ticks, `←`/`→` change the scope row under the cursor, typing on the note row edits it, `Enter` sends, `Esc` closes. A source whose list has not landed yet shows `asking…` and `Enter` waits for it. With nothing to send, `Enter` says so and sends nothing.

### 2. Writing

Left panel:

- A **Reviews** list, newest first. Each row names its period and whose it is (`4 – 10 Oct · mine`); this one is marked `writing…`.
- **Included**, for the review under the cursor: its scope (`Just mine · this project`), then the items themselves under each source, not only the counts. `Tab` moves the keys onto the items to scroll them.

Right panel: what is being done and for how long — `fetched 86 pull request descriptions · 14s`, then `choosing what earns a line, and writing it` — and a sentence saying the window can be closed.

`⌘W` stops it. `Esc` closes; the request keeps running, the footer says `Week in review: writing`, and it flashes `Week in review ready` when the text lands.

There is no session, so nothing appears on the grid and nothing is left to delete. If the request fails (the CLI is missing, not signed in, timed out), the row reads why and `⌘R` tries again.

### 3. Read

Left panel: the **Reviews** list only (each row with its period and whose it is, the new one marked `new` until read). The **Included** panel is for the wait and is gone here.

Right panel: the review, drawn as "The review's shape" describes, scrolled with `↑`/`↓`/`PageUp`/`PageDown`. Its border names the period and the scope.

Keys: `⌘C` copy as Markdown (the TODOS modal's copy chord), `⌘O` open the file in the editor, `⌘R` write this one again, `⌘N` back to Compose, `Tab` moves the keys between the list and the page, `Esc` closes.

Opening the modal lands on Read when a review exists for the current period, on Writing while one is being written, otherwise on Compose.

## What orion gathers

| Source | Read from | `Just mine` | `Everyone` |
|---|---|---|---|
| Merged pull requests | `app.open_prs[project].merged` — every one merged in the 7 days, no cap (`pull_request::rest_of_merged`) | `OpenPr::mine` | all, each with its author |
| Their descriptions | one fetch when the modal opens (below) | the same pull requests | the same |
| Linear issues done | `app.linear[project].list` where `status_type == "completed"`; description already fetched | `LinearIssue::mine` | all, each with `assignee` |
| Todos ticked | `app.todos[repo_path].items` where `done` is within 7 days | all of them | left out |

`All projects` runs the same reads over every project in `app.tree.projects` and removes duplicate Linear issues by id.

**Descriptions.** The lists carry titles, not bodies. `week_review::fetch_bodies` asks GitHub for the body, additions, deletions, changed-file count and base branch of each merged pull request in the period, 100 a page, through `pull_request::repo_graphql`. Each body is stripped of bot footers and blank lines and cut:

- to 600 characters ordinarily — the summary paragraph is what matters;
- to 6,000 characters when the pull request adds 5,000 lines or more and is not a release. A feature branch merged in one go often has only a commit list for a description, and the first 600 characters of that say nothing; this was the reason the biggest change of the test week was missed.

A pull request whose base is `main`, `master` or `production` is marked `RELEASE` in the material. The prompt leaves those out: they repeat what the week's other pull requests already say.

**Two additions to what the lists carry today.** The merged query (`LIST_QUERY`'s `merged` connection in `crates/orion-tui/src/pull_request.rs`) asks for `baseRefName`, kept on `PrMeta` as `base`, and for `author { login ... on User { name } }`, for the initials. When the merged list could not all be read (`App::merged_short`), the Compose panel says the count is short.

## How it runs

One request, no tools, built and sent by `week_review::write`:

```
claude -p --model <model> --effort <effort> --tools "" \
       --system-prompt "<one line: output exactly what is asked>" \
       --no-session-persistence --output-format json
```

with the prompt on stdin, as a child process the way orion runs `gh` (`tokio::process`, off the loop, killed on `⌘W` or quit). The JSON reply's `result` is the review; its `usage` and `total_cost_usd` go in the review's `.json` beside the file.

This needs Claude Code as the harness. Where the default agent is another harness, the first version says so on the Agent row and does not offer `Enter`; running it as an ordinary session in another harness is a later step.

Two settings in Settings → Review, mirroring `autofix_model` / `autofix_effort`:

- `review_model` — blank means Sonnet.
- `review_effort` — blank means `medium`.

### Why this, measured

All on the same week of 162 pull requests, `Everyone` scope:

| How | Time to write | Cost | Result |
|---|---|---|---|
| An agent session that fetches for itself, Opus low | 136 s | — | accurate but padded: 38 points, few demos |
| An agent session, material pre-fetched, Sonnet medium | 100 s | — | 18 points, good |
| An agent session, material pre-fetched, Opus medium | 138 s | — | 18 points, good |
| Five parallel readers (Sonnet, 21–28 s) then one writer (Opus medium, 102 s) | 123 s | — | misfiled the largest change; no faster |
| Five parallel readers (Haiku) | 79–107 s for the reading alone | — | slower than Sonnet; dropped |
| One request, no tools, with per-area counting, Sonnet medium | 112 s | — | good |
| **One request, no tools, no counting, Sonnet medium** | **38–51 s** | **$0.27–0.29** | **17 points, 16 with a Show; the best of the runs** |
| One request, no tools, no counting, Sonnet low | 29 s | $0.26 | 17 points, 13 with a Show, fewer Say lines |
| One request, no tools, no counting, Opus low | 65 s | $0.58 | 18 points, good |
| One request, no tools, no counting, Haiku | 139 s | $0.03 | one wrong owner; slowest |

What the runs showed:

- **The time is the model deliberating, not reading or tools.** A trivial request returns in 3 s. Splitting the reading across parallel readers saved nothing, because the writer still took as long on their notes as on the raw material.
- **Asking the model to count was the expensive part.** Requiring every pull request to be sorted into an area and the per-area "smaller fixes" to add up more than doubled the time. orion can count exactly what was not listed, so the model no longer does.
- **One pass over the whole week judges better than readers working apart.** A reader with a fifth of the week cannot tell that a commit-list merge is the week's biggest change.
- **Sonnet at medium is the blend.** Opus was slower and no better here; Haiku was slower and less accurate.

## The review's shape

By product area, every point numbered, owner and pull request at the right, demo lines under the points that have one.

```
# Week in review · 4 – 10 October
acme-app · everyone · 162 pull requests merged

AL Ada L · GR Grace · LT linus-t

## Assistant · AL
1. The assistant reuses pages from earlier documents · AL · #412
   - Show: Press ⌘J, type @, pick an earlier document: its pages are rebuilt here.
   - Say: Originals stay as hidden references linking back.
2. The assistant researches before it plans · AL · #431, #436

## Speed · AL, GR
1. Large documents open without waiting for every page · AL · #440
   - Show: Open the 600-page document: the editor appears almost at once.
   - Say: Page size drops from 5.5 MB to 45 KB.

+ 145 other pull requests merged, not listed
```

- **Areas.** Four to six, named in one to three words as a user would. The product's core comes first and its edges (accounts, sign-in, admin) last. Three or more measured speed, memory or size gains get an area of their own, `Speed`. A redesign that arrives as one large merge gets its own area.
- **Points.** Numbered from 1, two to four an area, 12 to 18 in the whole review however many pull requests there were. One change each, in at most nine words. One point can cover up to three related pull requests.
- **What earns a point.** Something a person outside the team that built it would want to know or see: a redesign or new capability, a measured gain, a change to something people do every day. Sizing, spacing, alignment, renaming, an edge case that now behaves: none of those.
- **Owner.** Exactly one person a point, the author of its first pull request. The area heading lists everyone who owns a point in it, most points first.
- **Show and Say.** `Show` is the demo: what to do on screen and what appears, at most 16 words. Every point that can be seen gets one. `Say` is the fact the screen does not show — a number, before to after, or the consequence — at most 14 words, only numbers the description states.
- **The last line** is orion's, not the model's: the number merged minus the number listed.

In a `Just mine` review the owner column, the headings' initials and the legend line are left out.

**Initials.** From the person's GitHub name when it has two or more words (`Ada L` is `AL`); else the first two letters of the name, or of the login when there is no name (`Grace` is `GR`), a camel-case or hyphenated login split at its capitals and hyphens (`linus-t` is `LT`). Two people with the same initials get a third letter. orion works these out and gives the model the legend.

**Drawing it.** The file is plain Markdown, so `⌘C` pastes cleanly anywhere. In the modal, `week_review::draw` reads the ` · <initials> · #<n>` tail off each numbered line and sets it in two right-aligned columns: the owner, then the pull request in the merged purple. An area heading's contributors are right-aligned so the list ENDS on the owner column: a heading with one contributor sits exactly over its points' owner, and a longer list grows leftwards. `Show` is drawn in green with the sweep the grid's merged rows use; `Say` in the muted text colour.

## The prompt

`week_review::prompt` builds this, filling the angle-bracket parts. Lines marked `[Everyone]` are left out of a `Just mine` review.

````
# The week in review: what it is and how to write it

This is read aloud to the whole company in about five minutes, with the product on screen. The reader walks through it area by area and demos what is worth seeing. It is not a changelog: most of what was merged does not belong in it.

Period: <4 – 10 October 2026>. Project: <name>. Scope: <everyone | just mine>.
People, by initials: <AL Ada L · GR Grace · …>.                              [Everyone]

## What earns a line

- A point is something a person OUTSIDE the team that built it would want to know or see. The test: would you stop the meeting to say this? If not, it is a smaller fix, however much work it was.
- By weight: (1) a redesign, or a capability that did not exist last week; (2) a measured gain in speed, memory or size; (3) a change to something people do every day. Those are points.
- Everything else is a smaller fix and gets no line: sizing, spacing, alignment, rounding, renaming, wording, placeholders, an edge case that now behaves, a glitch that is gone, a default that changed.
- Size is a signal. The handful of largest pull requests that are not marked RELEASE are where the week's biggest changes live: read those first, and make sure each one is either a point (or two or three, for a feature-branch merge) or is left out because no user would see it. A redesign that arrives as one huge merge gets an area of its own.
- A tweak to how an existing control behaves (how something toggles, snaps, aligns, resizes or is labelled) is a smaller fix, unless it removes something people fought with every day.
- 2 to 4 points an area. 12 to 18 points in the whole review, however many pull requests there were. Fewer and stronger beats complete.
- 4 to 6 areas, each named in one to three words the way a user would name that part of the product. Three or more measured speed, memory or size gains get an area of their own called `Speed`, and only measured gains go in it. Infrastructure a user would feel goes last under `Under the hood`, at most 3 points; otherwise it is left out.
- Lead with the product's core, what people come to it to do, and put its edges (accounts, sign-in, admin, settings) after. Inside that, order areas and points by how much there is to show.
- When two candidates compete for a line, the one in the core of the product wins over the one at its edge.
- A pull request that merges a feature branch IS the feature, however thin its description: its list of commits is its description. Read the list and pull out the two or three changes a user would notice most; each is its own point, and that one number may then appear on up to three points. Only a pull request marked RELEASE, which moves one standing branch into another, is left out.

## Show and Say

- `Show` is the demo. Every point that can be seen on screen gets one: what the presenter does on the dev site, then what appears. Most points have a Show. It names a place and an action ("Press ⌘K, type a word: results land first, pages after"), never "see the new look".
- If the description does not say enough to know what to click, write the point without a Show rather than guess.
- A new capability with no Show is a missed demo: read its description again for where it lives and what the user presses.
- `Say` is the fact the screen does not show: a number, before to after, or the consequence for the user. Only numbers the description states.
- At most 16 words for a Show, 14 for a Say. One of each at most.

## Writing a point

- ONE change, as the user meets it, in at most 9 words, present tense. Never two changes joined by a semicolon, "and" or a comma.
- Name the change. Never "lands", "improvements", "updates", "various".
- No "fix:"/"feat:" prefixes, file names, internal code names or ticket ids. Something behind a flag ends `(behind a flag)`.
- One point can cover up to 3 closely related pull requests. Its owner is the author of the first number listed. Exactly one owner a point. Any other pull request number appears once in the whole file.

## The shape, exactly

```
# Week in review · <4 – 10 October>
<project> · <everyone | just mine> · <N> pull requests merged

<AL Ada L · GR Grace · …>                                                    [Everyone]

## <Area> · <initials>, <initials>
1. <what changed> · <owner> · #<pr>
   - Show: <what to do on screen, then what appears>
   - Say: <the number or the consequence>
2. <what changed> · <owner> · #<pr>, #<pr>
```

- The heading lists the initials of everyone who owns a point in the area, most points first, separated by a comma and a space.   [Everyone]
- Do not count or list what you leave out: the smaller fixes, releases, dependency bumps, CI, tests and tooling simply do not appear. The total left unlisted is added afterwards.
- No introduction, no summary, nothing after the last point.

## Note from the person asking                                               [when there is one]

<the note, verbatim>

## The pull requests merged in the period

Each entry: number · author's initials · size · branch · title, then the start of its description (more of it for a very large pull request).

#<n> · <AL> · +<added>/-<removed> in <files> files · into <base>[ · RELEASE] · <title>
<description, cut as above>
---
…

## Linear issues done, todos ticked                                          [when those sources are ticked]

<identifier · title · done date · description, cut to 1,500 characters>

Write the review now: the Markdown and nothing else, starting with the `# Week in review` line.
````

What the test runs did and did not cover:

- Run with and without a note. Without one, the brief above still led with the product's core and pulled three points out of the week's largest merge. With a note naming what the team cares about, it followed the note. The note is still the way to say what matters in a given project.
- Run on pull requests alone. How Linear issues and todos fold in (most are the same work as a pull request, so they should enrich a point rather than add one) is not yet tried: run it with that section filled before building.
- Run on one repository. `All projects` puts several repositories' entries in one list; untried.

## Checking what comes back

The shape is strict so that orion can check it without a model, in `week_review::check`:

- every `#<n>` is a pull request that was sent; none appears on more than three points;
- every point's owner is the author of its first pull request, and every heading's initials are its points' owners.

Lengths are asked for in the brief and not checked: a point a word over is no reason to doubt a review, and warning about it would bury the two faults that are.

On the final brief, five of six runs passed all three; the sixth (Haiku) named a wrong owner. When a check fails, the review is still shown, with one line under its title saying which (`1 point names the wrong owner`).

orion then appends the last line itself: `+ <merged − listed> other pull requests merged, not listed`. That figure is exact by construction.

## Where the review is kept

`<DATA DIR>/reviews/<project id>/<YYYY-MM-DD>-<mine|everyone>.md`, the date being the period's last day; `all` in place of the project id for `All projects`. A second run for the same period and scope overwrites the file.

Beside it, `<same name>.json`: the scope, every item sent (source, reference, title, author), the model, effort, seconds and cost. It is what the **Included** panel and the Reviews rows read.

The last 12 reviews per project are kept; older files are deleted when a new one lands. This goes in the "What orion keeps" table in `docs/configuration.md`.

## Decisions to confirm

1. **The key.** `⌘⇧Y`, with `ctrl+shift+y` for Terminal.app. `⌘⇧W` would read better but Ghostty keeps it for good (`keymap::host_warning`). Free alternatives: `⌘⇧D`, `⌘⇧E`, `⌘⇧J`.
2. **"Yours" for pull requests** is "you opened it". A pull request you reviewed or merged but did not open is left out of `Just mine`.
3. **Initials or first names** in the owner column.
4. **The period is fixed at the last 7 days.** The lists hold exactly 7 days; other periods mean widening them first.
5. **Default model: Sonnet at medium effort**, on the measurements above.

## Risks

- **It depends on the `claude` command line tool's print mode** and on the flags above staying as they are. `week_review::write` is the one place that knows them.
- **The prompt is large**: about 135 KB for 162 pull requests. It goes on stdin, not the command line, so the OS limit on arguments does not apply.
- **A description can contain instructions.** Pull request text is other people's writing sent to a model. The model has no tools and its reply is only ever drawn as text and checked against the shape, so the worst a hostile description can do is spoil one review.
- **Cost is per run**: about 30 cents on Sonnet for a 162-pull-request week. The `.json` records it.

## Implementation steps

All paths are from the repository root.

1. **Try the untested parts by hand first.** Build the prompt as a text file for a real week with the Linear and todo sections filled, and once more for `All projects`; run each with the command in "How it runs"; adjust the brief until the checks pass and the Linear items enrich points rather than add them.

2. **New module `crates/orion-tui/src/week_review.rs`** (not `review.rs`, which is the diff viewer's reviewed-files store). Register it in `crates/orion-tui/src/lib.rs`. It holds:
   - `pub const DAYS: u32 = 7;` and `const KEEP: usize = 12;`
   - `struct Scope { whose: Whose, wher: Where }`, `enum Whose { Mine, Everyone }`, `enum Where { Project, All }`
   - `struct Material { prs, issues, todos }`, serialised into the review's `.json`
   - `fn gather(app, scope, picks, now) -> Material` — pure
   - `async fn fetch_bodies(dir, since) -> Option<Vec<PrBody>>` — the paged GraphQL fetch
   - `fn initials(people) -> Vec<(String, String)>` — pure
   - `fn prompt(material, bodies, legend, scope, note) -> String` — pure
   - `async fn write(prompt, model, effort) -> Result<Reply, String>` — the child process
   - `fn check(review, material) -> Vec<Problem>` and `fn finish(review, material) -> String` (appends the last line) — pure
   - `struct WeekReviewView`, and `open`, `handle_key`, `handle_mouse`, `paste`, `draw`, `hints`, `land`

3. **The overlay.** Add `WeekReview(Box<week_review::WeekReviewView>)` to `Overlay` in `crates/orion-tui/src/app.rs`, then one arm in each place `Overlay::Autofix` has one: `crates/orion-tui/src/ui.rs` (draw), `crates/orion-tui/src/event_loop.rs` (keys, mouse, paste, the overlay's name), `crates/orion-tui/src/overlay_close.rs`.

4. **The key and the palette.** Add `Action::WeekReview` and its `ActionSpec` beside `Action::Todos` in `crates/orion-tui/src/keymap.rs` (group `PROJECTS & WORKTREES`, defaults `&["cmd+shift+y", "ctrl+shift+y"]`); release the chord in `crates/orion-tui/src/ghostty_config.rs`; add the palette entry the way `Action::Todos` has one.

5. **Sending and landing.** A channel on the loop like `prs_tx`: `week_review::open` spawns `fetch_bodies`; `Enter` spawns `write` once the bodies are in; each answer lands through `week_review::land`, which checks, finishes, saves the two files, prunes the directory to `KEEP`, flashes `Week in review ready` and switches an open modal to Read. The request in flight lives on the app (`app.week_review_pending`), not the view, so closing the modal does not lose it.

6. **The merged query.** `baseRefName` and the author's name on `LIST_QUERY`'s `merged` nodes and `MERGED_QUERY` in `crates/orion-tui/src/pull_request.rs`, kept on `PrMeta` (`base`, `author_name`, both `#[serde(default)]`).

7. **Settings.** `review_model` and `review_effort` in `crates/orion-tui/src/config.rs`, `Config::review_launch()` after `Config::autofix_launch()`, two rows on the Review tab. The per-project note in `config.local.json`.

8. **Docs.** `docs/keys.md` (the key, and a row in the modals table), `docs/sessions.md` (a short section), `docs/configuration.md` (the settings, and the reviews row in "What orion keeps").

9. **A screenshot scene.** `scripts/shot/scenes/week-review.keys` (start with `Escape` to pass WHAT'S NEW) over the existing `pr-merged.json` fixture, for the Compose state.

No daemon code changes: nothing here goes through the daemon. Nothing in `crates/orion-daemon/daemon-inputs.txt` is touched, so this is a patch release.

## Tests

In `crates/orion-tui/src/week_review.rs`:

- `gather`: `Just mine` takes only your pull requests and issues; `Everyone` takes all with their authors and leaves todos out; only todos ticked inside the 7 days; an unticked source is left out; duplicate issues across projects removed.
- `initials`: two-word names, one-word names, logins with hyphens and capitals, and a clash.
- `prompt`: names every item; cuts an ordinary description at 600 characters and a 5,000-line one at 6,000; marks a pull request into `main` as `RELEASE`; leaves the `[Everyone]` lines out of a `Just mine` review; includes the note only when there is one.
- `check`: passes the shape above; reports an unknown number, a wrong owner, a heading that disagrees with its points, an over-long line.
- `finish`: the last line is merged minus distinct numbers listed.
- Compose: `Space` unticks a source and its group leaves the right panel; `Enter` with nothing ticked sends nothing and flashes why.
- Landing: a reply saves both files, flashes, switches the modal to Read, prunes to 12; a failure shows why and keeps Compose's choices.
- Drawing: one owner in a heading sits over its points' owner column; a drawn-screen test per state with `TestBackend`, as `pr_modal`'s tests do.

## Verification

```
cargo fmt --all -- --check
cargo clippy -p orion-tui --tests
cargo test -p orion-tui --lib -- week_review
cargo test -p orion-tui --lib
make shot SCENE=week-review
```

Then by hand in a real project: open the modal, check the counts against the PULL REQUESTS MODAL's Merged section and the LINEAR VIEW's Done rows, send it, time it, and read the review against the week.

## Out of scope for the first version

- Periods other than the last 7 days.
- Harnesses other than Claude Code.
- Scheduling it (every Friday at 4).
- Posting it anywhere (Slack, Linear, a pull request). `⌘C` and paste is the hand-off.
- Sessions run, time spent, or usage figures as a source.

## As built

- **The key's twin is `⇧W`**, not `ctrl+shift+y`: a terminal that sends no ⌘ does not reliably send `^⇧Y` either, and every `^` letter was taken.
- **The merged query is untouched.** `week_review::fetch_merged` asks for the base and head branches, the size and the author's name itself, in the same paged walk that brings the descriptions, so `PrMeta` gained nothing. A release is a standing branch (`dev`, `staging`, …) merged into `main`, `master` or `production`; a feature branch merged into `main` is not one, or a repository that merges straight to `main` would have every pull request left out.
- **The note is kept in the reviews folder** (`reviews/p-<project>/note.txt`), not in `config.local.json`.
- **Reviews keys.** `↑`/`↓` walk the reviews and `⇧↑`/`⇧↓`, `PgUp`/`PgDn`, `Home`/`End` scroll the page; there is no `Tab` to hand the keys to the page. `Tab` on Compose goes to the reviews, `⌘N` comes back.
- **orion renumbers.** `finish` numbers each area's points from 1 whatever the model wrote: one run numbered straight through the areas.
- **The legend runs most-merged first**, not in login order.
- **`All projects` writes pull requests as `<project>#<n>`** so two projects' `#12` stay apart. Untried against the model.
- **A live check** ships with the tests, ignored by default: `ORION_WEEK_REVIEW_LIVE=<checkout> cargo test -p orion-tui --lib -- week_review::tests::live --ignored --nocapture` fetches that checkout's real week, writes the review with the real model and holds it to its shape (`ORION_WEEK_REVIEW_SCOPE=mine` for just yours). Run on 2026-10-10: 165 pull requests fetched in 16 s and written in 75 s for $0.33 (`Everyone`); 35 fetched in 3 s and written in 39 s for $0.12 (`Just mine`); both held their shape.
- **It is written through one of orion's Claude accounts**, not a bare `claude`: `Config::review_account` is the **Review account** row's when one is picked, else the default agent's when that is a Claude account, else the first that is on. `week_review::write` runs that account's own `program` with its `launch_env` (the `CLAUDE_CONFIG_DIR` that pins it), Compose names it on a **Via** line, and a review's `.json` records it. Checked live on 2026-10-10 through both a built-in and an extra account.
- **Still untried against the model:** Linear issues and todos in the prompt, and `All projects`.
