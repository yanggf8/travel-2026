# DAL migration: update-offer → repo::plan_offers::upsert_date_pricing (+ audit cleanup)

**Date:** 2026-07-02 · **Status:** READY (Codex-recommended + corroborated). Third mutation DAL migration.
**A 2-for-1:** (a) move the domain write into `repo::plan_offers`, (b) switch the hand-rolled audit to
`cascade::common::record_operation` (the stale pattern, like the set-day-theme/hotel/flight cleanup).
Byte-identical. **Pipeline:** this plan → Codex writes the behavior-lock test → Claude does the impl (2
edits, below the delegation threshold) → Claude verifies.

## Scope (update_offer.rs)
1. **The domain write** — `plan_offer_date_pricing` UPSERT (lines 133-152): `INSERT ... (plan_id,
   destination, offer_id, date, price, availability, seats_remaining, updated_at) VALUES (...) ON
   CONFLICT(plan_id, destination, offer_id, date) DO UPDATE SET price=excluded.price,
   availability=excluded.availability, seats_remaining=excluded.seats_remaining,
   updated_at=excluded.updated_at`. → NEW `repo::plan_offers::upsert_date_pricing(conn, plan_id, dest,
   offer_id, date, price, availability, seats_remaining, now_db)`. This is a POINT UPSERT — a SECOND
   date_pricing primitive, distinct from `insert_offer`'s bulk INSERT (which has `currency` + no ON
   CONFLICT). Do NOT stretch insert_offer; add a new fn (Codex).
2. **The hand-rolled audit** (lines 186-212): `new_run_id()` + `INSERT INTO operation_runs (... 'update-offer'
   ... 'completed' ...)` + `UPDATE plans SET version`. → replace with `cascade::common::record_operation(
   conn, plan_id, "update-offer", &summary, version_before, version_after, &now_db)`. `summary =
   "{offer_id} {date} {availability}"` (unchanged). Remove the now-dead local `new_run_id` if unused.

## What STAYS in the command (business logic)
- The existing-row read + fail-loud on missing (115-125) — no create-without-price.
- The MERGE: `merged_price = price.unwrap_or(existing.price)`, `merged_seats = seats.or(existing.seats_remaining)`
  (128-130) — omitted values keep the existing row's. The repo fn takes the ALREADY-MERGED values.
- The `offer_availability_updated` event + KV rows (154-184), incl. omitted→"undefined" rendering.
- CLI parse, source default "cli", version read, println.

## BYTE-IDENTITY RULES (corroborated vs source)
- `currency` is NOT in the UPSERT column set — it must remain untouched (the INSERT has no currency col).
- ON CONFLICT set stays EXACTLY `price, availability, seats_remaining, updated_at`.
- The merge (omitted price/seats preserve existing) stays in the command; the repo gets the merged values.
- Event KV renders omitted price/seats as "undefined" (157-159) — unchanged.
- `record_operation` does the identical operation_runs INSERT (status='completed', started=completed=now)
  + plans.version UPDATE — behavior-identical to the hand-rolled block (verified for set-day-theme et al.).
- No new transaction.

## Test oracle (Codex writes; NONE exists) — new rust/crates/travel-cli/tests/update_offer.rs
Real-Turso; arm `common::Guard` (shared DB). Seed a `zztest{nanos}` plan + active_destination + ONE
`plan_offer_date_pricing` row with price/availability/seats_remaining + explicit `currency`. Then:
1. `update-offer <offer-id> <date> limited 12345 2 agent --plan-id <plan>` → assert the row: price=12345,
   availability=limited, seats_remaining=2, **currency UNCHANGED**; one `operation_runs` row
   command_type='update-offer' summary "<offer_id> <date> limited"; `plans.version` +1.
2. Omitted-fields case: run again WITHOUT price/seats (only change availability) → assert prior
   price/seats PRESERVED, availability changed. (This locks the merge.)
Credless skip; teardown deletes the seeded plan + plan_offer_date_pricing + operation_runs + plan_events
rows.

## Verification (Claude)
Build clean; the new lock green pre- and post-migration; live smoke on a throwaway offer shows identical
date_pricing row + operation_runs + version. `record_operation` boundary: audit stays in cascade::common,
0 audit refs in repo::plan_offers.
