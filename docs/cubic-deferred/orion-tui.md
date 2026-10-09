# Deferred cubic findings: orion-tui

## 2026-10-09 — `crates/orion-tui/src/fetch.rs` (fetch-consistency, Phase 1)

- **P1 · fetch.rs:197 · Cancelled answers can be observed before ticket validation.** `Ticket::asked()` is readable before `Flights::land()` says whether the ticket still stands, so a caller could observe a cancelled answer. Deferred at the 2-pass cap while three builders adopt the API; every wave-2 call site is checked in review to `land` before it observes. Follow-up: return the accepted stamp from `land` (`Landed { owed, asked }`) and drop `Ticket::asked`.
- **P2 · fetch.rs:66 · Public timestamps and mutable ticket fields.** `Asked::At` can be built by hand and `Ticket::{key, at}` are public, so a caller could bypass `fetch::now()`'s uniqueness or mangle a ticket. Follow-up: make `Ticket`'s fields private behind accessors, and build live stamps only through `fetch::now()` / tickets.
