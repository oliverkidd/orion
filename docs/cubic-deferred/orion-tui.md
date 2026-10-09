# Deferred cubic findings: orion-tui

## 2026-10-09 — `crates/orion-tui/src/fetch.rs` (fetch-consistency, Phase 1)

- **P1 · fetch.rs:197 · Cancelled answers can be observed before ticket validation.** `Ticket::asked()` is readable before `Flights::land()` says whether the ticket still stands, so a caller could observe a cancelled answer. Deferred at the 2-pass cap while three builders adopt the API; every wave-2 call site is checked in review to `land` before it observes. Follow-up: return the accepted stamp from `land` (`Landed { owed, asked }`) and drop `Ticket::asked`.
- **P2 · fetch.rs:66 · Public timestamps and mutable ticket fields.** `Asked::At` can be built by hand and `Ticket::{key, at}` are public, so a caller could bypass `fetch::now()`'s uniqueness or mangle a ticket. Follow-up: make `Ticket`'s fields private behind accessors, and build live stamps only through `fetch::now()` / tickets.

## 2026-10-09 — issues, git readers (fetch-consistency, Phase 4)

- **P2 · issues.rs:1149 · A failed conversation read isn't retried on a revisit within `FRESH`.** By design: `FRESH` (30s) is the retry backoff the plan asked for, so a flapping `gh` isn't hammered on every cursor move; the doc comment on `detail_due` says so.
- **P2 · commit_list.rs:212 · A still-fresh cached "no base" resolves twice per commit-list open.** `read` forgets the root's entry and re-resolves whenever the cache says `None`, so with no resolvable base every manual open runs the base lookup again inside the 5s negative TTL. Cheap (a manual open), deferred at the 2-pass cap. Follow-up: re-resolve only when the cached `None` is older than a second or two.

## 2026-10-09 — `PrStore` (fetch-consistency, Phase 2)

- **P1 · event_loop.rs:1818 · A checkout lookup that says MERGED doesn't take the row out of the open list.** Only a page (`land_pr_detail` → `drop_retired_pr`) retires a list row before the next list; a lookup answer only updates the store, so the row reads `merged` in the open list until the next list (≤15s) leaves it out. Predates this work (lookups never retired list rows). Deferred at the 2-pass cap; fixed in the final polish pass by retiring the row from `land_pull_request_at` too.
