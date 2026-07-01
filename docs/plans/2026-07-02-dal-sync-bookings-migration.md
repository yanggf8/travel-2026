# DAL migration: sync-bookings domain writes → repo::bookings

**Date:** 2026-07-02 · **Status:** READY TO BUILD (Codex-reviewed + corroborated).
**Context:** first mutation-command DAL adoption after the audit-triad cleanup (`7346e75`). Pattern:
domain writes → `travel-db` repo; audit stays in `cascade::common` (N/A here — sync-bookings writes no
audit triad). Behavior must be **byte-identical** — this is a consistency refactor, not a change.
**Pipeline:** this plan → Codex writes the test oracle → Grok does the migration → Claude verifies.

## Scope
Move the inline `bookings_*` domain-write SQL out of `rust/crates/travel-cli/src/sync_bookings.rs` into
the existing `rust/crates/travel-db/src/repo/bookings.rs` (today read-only: `book_by_deadlines`,
`query_current`). The command keeps CLI/TSV parsing, the created-vs-updated diff logic, the loop, and the
`println!`.

## Corroborated facts (Codex + Claude, vs source)
- Inline writes: snapshot SELECT `sync_bookings.rs:75-101`; stale delete `:104-118`; current upsert
  `:153-183` + payload delete/reinsert `:185-200`; event insert `:204-225` + event_data correlated
  insert `:227-235`.
- `repo::bookings` is read-only today (`bookings.rs:18-51`, `:98-135`); no write fn.
- sync-bookings writes NO audit triad (grep for operation_runs/plans.version/plan_events = none).
- Input-struct pattern to mirror: `OfferRow`+`offers::insert` (offers.rs:5-33,202-256);
  `SegmentWrite`+`insert_segment` (route_segments.rs:9-18,68-98); `HotelWrite`+`upsert` (hotels.rs:9-18).
- The extractor `BookingRow` lives in `booking_integrity.rs:19` — keep it in travel-cli.

## Decisions (Codex-recommended, corroborated)
1. **DAL structs in travel-db:** add `BookingCurrentWrite`, `BookingEventWrite`, `ExistingBooking` to
   `repo::bookings`. Keep the CLI extractor `BookingRow` in travel-cli; map to the write structs at the
   command boundary.
2. **Diff logic stays in the command** — the created-vs-updated comparison (`sync_bookings.rs:125-136`)
   is orchestration, not SQL.
3. **Keep `datetime('now')` inline in the repo SQL** (do NOT switch to a passed `now_db` here) — this
   command uses DB-side now for current rows (`:159,:162`) and events (`:213`); passing a Rust timestamp
   would change timestamp semantics. (Other repos pass now_db; sync-bookings is intentionally the
   exception to stay byte-identical.)

## Proposed repo API (repo::bookings, bound params, `Result<_, String>`)
- `current_snapshot_for_trips(conn, &[trip_id]) -> Result<HashMap<booking_key, ExistingBooking>, String>`
  (the diff read — lift `:75-101`).
- `delete_current_for_trip(conn, trip_id)` — the 2 deletes, **payload child first, then current** (`:104-118`).
- `upsert_current(conn, &BookingCurrentWrite)` — the `bookings_current` INSERT…ON CONFLICT + payload
  delete-then-reinsert-KV (`:153-200`).
- `insert_event(conn, &BookingEventWrite)` — the `bookings_events` INSERT + the `bookings_event_data`
  correlated-subquery KV insert (`:204-235`).

## BYTE-IDENTITY RISKS (must preserve exactly — Codex, corroborated)
1. **Snapshot BEFORE delete** — capture `existing` (`:75-101`) then delete (`:104-118`). Snapshot after
   delete would make every row look `created`.
2. **ON CONFLICT updates ONLY 6 cols** — `status, reference, book_by, booked_at, price_amount,
   updated_at` (NOT title/trip_id/destination/category/payload). Verified `:160-162`.
3. **event_data correlated subquery** — `SELECT ?1, event_at, … FROM bookings_events WHERE
   booking_key=?1 ORDER BY event_at DESC LIMIT 1` (`:229-231`). Do NOT replace with a passed ts /
   RETURNING — it changes timing/collision behavior.
4. **payload delete-then-reinsert order** — delete (`:185-190`) then `enumerate()`-ordered insert (`:192-200`).
5. **No new transaction** — current code is statement-by-statement, no explicit tx. Don't wrap the whole
   sync in one (would change partial-write/failure behavior).

## Test oracle (Codex writes; NONE exists today)
New `rust/crates/travel-cli/tests/sync_bookings.rs`: seed a unique `zztest{nanos}` plan/dest with enough
normalized rows to produce one booking → `travel sync-bookings --plan-id <p> --trip-id <t>` → assert
`bookings_current` row + `bookings_current_payload` ordered KV + one `bookings_events` `created` row +
`bookings_event_data` ordered KV. Then run sync AGAIN with a changed field → assert an `updated` event
(the diff path). Arm `common::Guard` immediately after ids known (shared Turso); pre-clean, no trailing
teardown. Skip cleanly when credless.

## Verification (Claude)
Build clean (no warnings); the new test green vs live Turso; a live smoke on a throwaway trip_id shows
identical `bookings_*` rows + the same "Synced N bookings" stdout as before the refactor.
