# Archived TS — Phase 2 Rust read-port (snapshot, NON-destructive)

**Date:** 2026-06-08
**Status:** Reference snapshot only. The **live copies under `src/cli/` are still
in use and unchanged** — this is a copy, not a move. `npm run` still routes every
command through `ts-node src/cli/travel-update.ts`.

## Why a copy, not a removal

The 7 commands below have byte-parity Rust implementations in
`rust/crates/travel-cli` (the `travel` binary). But they CANNOT be deleted yet:

1. `package.json` scripts call `ts-node src/cli/travel-update.ts`, whose command
   **registry imports every command module** — removing a ported source breaks
   the registry for ALL commands.
2. The Rust binary is **not wired into `package.json`** (no `./bin/travel`
   routing). Only 7 of ~55 subcommands have Rust equivalents.

Per the decommission gate (CLAUDE.md / `docs/plans/2026-06-05-rust-cli-migration.md`):
TS is deleted only after `package.json` points to the Rust binaries with a TS
fallback. Until that cutover, these live in `src/cli/` AND are snapshotted here.

## The 7 ported commands → Rust equivalents

| Command | Live TS source | Rust module | Verified |
|---|---|---|---|
| `plans` | `src/cli/commands/plans.ts` | `plans.rs` | byte-parity |
| `query-offers` | `src/cli/commands/turso.ts` | `offers.rs` | byte-parity |
| `query-bookings` | `src/cli/commands/turso.ts` | `bookings.rs` | byte-parity |
| `check-freshness` | `src/cli/commands/turso.ts` | `freshness.rs` | byte-parity |
| `query-destination-ref` | `src/cli/commands/query-destination-ref.ts` | `destination_ref.rs` | byte-parity |
| `compare-dates` | `src/cli/compare-dates.ts` | `compare_dates.rs` | byte-parity |
| `compare-true-cost` | `src/cli/compare-true-cost.ts` | `compare_true_cost.rs` | byte-parity |

## ⚠️ `turso.ts` is PARTIAL

`commands/turso.ts` is snapshotted whole, but only **3 of its commands are
ported** (query-offers, query-bookings, check-freshness). It also contains
**NOT-yet-ported** commands that must NOT be removed from the live file:
`import-offers`, `sync-bookings`, `check-booking-integrity`. Do not treat this
whole file as decommissioned.

## Next step (gated)

Wire `package.json` → `./bin/travel <cmd>` (Rust-if-present, else ts-node
fallback) per the migration plan, THEN remove the live ported sources. See
memory `ts-archive-pending-phase2`.
