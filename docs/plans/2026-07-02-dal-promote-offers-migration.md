# DAL migration: promote-offers domain writes → repo::plan_offers

**Date:** 2026-07-02 · **Status:** READY (existing green oracle; Codex to review).
**Context:** second mutation-command DAL adoption (after `sync-bookings` `bea3552`). Move promote-offers'
inline `plan_offer_*` domain-write SQL into a NEW `rust/crates/travel-db/src/repo/plan_offers.rs`.
Byte-identical — pure consistency refactor. **Audit is already correct** (promote-offers uses
`cascade::common::record_operation`, line 280 — stays in travel-cli).
**Pipeline:** this plan → Codex review → Grok impl → Claude verify line-by-line. **No new test needed** —
`rust/crates/travel-cli/tests/promote_offers.rs` already asserts the mapped `plan_offer_*` rows + drives
`select-offer` end-to-end, and is GREEN against current code (verified).

## Scope — the domain writes to move (promote_offers.rs)
1. **delete_existing** (~371-391): loops a HARDCODED const table list [plan_offer_includes,
   plan_offer_hotel_access, plan_offer_date_pricing, plan_offer_best_value, plan_offer_flights,
   plan_offer_hotels, plan_offers] running `DELETE FROM {table} WHERE plan_id=?1 AND destination=?2 AND
   {id|offer_id}=?3`. The `{table}` and `{id/offer_id}` are FIXED code values (not user input) — safe,
   NOT sql_quote; preserve the fixed-list loop. → `plan_offers::delete_existing_for_offer(conn, plan_id,
   dest, offer_id)`.
2. **insert_offer** (~396-...): the cohesive block — `plan_offers` INSERT (always), `plan_offer_date_pricing`
   INSERT (always, one synthesized row), then CONDITIONAL: `plan_offer_hotels` (if hotel_name),
   `plan_offer_flights` (if BOTH flight cols non-NULL — no fabrication), `plan_offer_includes` (split
   `includes` on delimiters, skip if empty). → `plan_offers::insert_offer(conn, &PlanOfferWrite)` where
   `PlanOfferWrite` (a NEW struct in travel-db) carries all fields + the already-parsed
   hotels/flights/includes the command computed. Keep the exact conditional logic on the WRITE side? OR
   keep the "only-if-present" decision in the command and pass Option/empty vecs. DECIDE (open q1).
3. **upsert_process_status** (~558-...): `INSERT INTO process_statuses (...) ... ON CONFLICT ... DO UPDATE`
   → `plan_offers::upsert_process_status(conn, plan_id, dest, process_id, status, now_db)` OR reuse an
   existing process_statuses repo if one exists (check — open q2).

## What STAYS in promote_offers.rs (the command)
CLI/arg parsing, the offers→promotable filtering (NULL price/date exclusion), the includes-splitting +
hotels/flights presence decisions (business logic), the `record_operation` audit call (line 280), the
version read, the println/summary. Map to `PlanOfferWrite` at the boundary.

## Open questions for Codex review
1. Does the conditional insert logic (hotels/flights/includes only-if-present) belong in the repo fn
   (repo decides based on Option fields) or the command (passes only what should be written)? Mirror how
   offers.rs / route_segments.rs handle optional child rows.
2. Is there an existing `process_statuses` repo/helper to reuse for the upsert, or is a new fn on
   plan_offers correct? (grep repo/ for process_statuses.)
3. `now_db` handling: does promote-offers pass a now string to these INSERTs or use datetime('now')
   inline? Preserve whatever the current code does (byte-identity). Check each write.
4. New module `repo::plan_offers` vs adding to `repo::offers` (which owns GLOBAL `offers`, not
   `plan_offer_*`)? I lean NEW module (different table family). Confirm + note the mod.rs export.

## BYTE-IDENTITY RULES (verbatim SQL — Codex to corroborate the specifics)
- Preserve the delete const-table list + the id-vs-offer_id key choice EXACTLY.
- Preserve every INSERT's column set + the ON CONFLICT clause on process_statuses.
- Preserve the conditional gates (hotel only-if-name; flights only-if-both-non-NULL — NO fabrication;
  includes skip-if-empty).
- Preserve now/timestamp handling (inline vs passed) per current code.
- No new transaction. Audit (`record_operation`) stays in the command, unchanged.

## Verification
`make build` clean; `cargo test -p travel-cli --test promote_offers` stays GREEN (the lock); a live smoke
(promote a throwaway offer) yields identical plan_offer_* rows; `select-offer` still consumes them.
Reuse `PlanOfferWrite` for later `import_offers`/`select_offer` DAL migrations (import_offers hand-rolls
its audit — a future cleanup like set-day-theme/hotel/flight).
