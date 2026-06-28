# Design: `promote-offers` — bridge global `offers` → plan-scoped `plan_offers`

**Date:** 2026-06-28
**Status:** Approved (design); implementation pending. **Codex-reviewed 2026-06-28**
(verdict: sound; findings corroborated against source and folded in — see "Review
corrections" below).
**Author:** Claude (with Yang)

## Problem

The OTA sweep produced **20 real package offers** in the **global `offers` table**
(`tokyo_sep_2026`: eztravel 9, besttour 5, travel4u 5, settour 1). But the planning
cascade — `select-offer <offer-id> <date>` — reads from a **different, plan-scoped**
table family: `plan_offers` + `plan_offer_flights` / `plan_offer_hotels` /
`plan_offer_hotel_access` / `plan_offer_date_pricing`. These two table families are
**disjoint with no bridge between them**:

- `select-offer` queries `plan_offers` (`read_offer_data`, `select_offer.rs:556`),
  never the global `offers` table.
- `import-offers` reads `scrapes/*.json` **files**, not the `offers` table
  (`import_offers.rs:140-172`), so it can't help either.

Net effect: the proven OTA offers cannot flow into a plan. `promote-offers` fills
exactly that gap.

## What `select-offer` requires (verified against source)

For `select-offer <offer-id> <date>` to succeed against an offer, the offer must
exist in `plan_offers` for the active destination, and:

- A `plan_offer_date_pricing` row matching `<date>` supplies the `price_total` event
  KV (`select_offer.rs:647-669`). Without it, `price_total` is `"undefined"` — not a
  hard error, but we want it populated.
- `has_hotel()` (a `plan_offer_hotels` row) gates P4 → populated.
- `has_flight()` (≥1 `plan_offer_flights` row) gates P3 → populated.
- The P3_4 status must be in `researched`/`selecting`/`selected` for the
  `→selected` transition (`validate_transition`).

`promote-offers` therefore must, at minimum, write `plan_offers` + a date_pricing row
+ a hotel row, and leave P3_4 at `researched` (matching `import-offers`).

## Source data shape (global `offers`, verified live)

Flat columns (`schema.sql:516`):
`id, source_id, type, name, price_per_person, currency, region, destination,
departure_date, return_date, nights, availability, hotel_name, hotel_area, airline,
flight_outbound, flight_return, includes, scraped_at, source_file`.

All 20 current rows: `type='package'`, `hotel_name` + `departure_date` +
`price_per_person` set, **`flight_outbound`/`flight_return` = NULL** (flight detail is
only embedded in the offer-id string, e.g.
`eztravel_roundtrip-TPE-TYO_20260904_4n_TPE_15_30-NRT_20_00_15498`), `includes` empty,
`hotel_area` empty.

## Decision: flight handling = hotel-only, skip flights when NULL

`promote-offers` maps only the data that genuinely exists. When `flight_outbound` /
`flight_return` are NULL, it writes **no** `plan_offer_flights` rows. `select-offer`
then populates **P4 only**, not P3 — which is the honest outcome for these rows.

Rejected: regex-parsing the offer-id string to synthesize `flight_outbound`/`return`.
That would fabricate structured data from a display string (id formats vary per
source), violating the project's no-fabrication / no-hardcode rule. If a future scrape
populates the flight columns, `promote-offers` will carry them through unchanged.

## Mapping (`offers` → `plan_offers` family)

| Target | Source | Notes |
|---|---|---|
| `plan_offers.id, source_id, type, currency, availability, scraped_at` | direct | |
| `plan_offers.title` | `name` else `hotel_name` | `name` is empty for these rows |
| `plan_offers.price_per_person` | `price_per_person` | |
| `plan_offers.price_total` | `price_per_person × pax` | `--pax` default 2 (matches import-offers) |
| `plan_offers.product_code, duration_days, seats_remaining, package_subtype` | NULL | not present in `offers` |
| `plan_offer_date_pricing` (1 row) | `date=departure_date, price=price_per_person, availability, currency` | the date `select-offer` will match |
| `plan_offer_hotels` (if `hotel_name` non-empty) | `name=hotel_name, area=hotel_area` (NULL if empty), `slug/star_rating`=NULL | |
| `plan_offer_flights` | `flight_outbound`/`flight_return` **only if both non-NULL** | skipped for all 20 today |
| `plan_offer_includes` | split `includes` on a delimiter, skip if empty | none today |

The `offers` table has no per-date pricing of its own, so the single date_pricing row
is synthesized from `(departure_date, price_per_person)` — unambiguous since every row
has both. The flight-leg shape, when present, mirrors `import_offers.rs:492-521`
(direction outbound/return, airline at flight level from the outbound leg).

## Command

```
travel promote-offers --from-offers --dest <slug> [--plan-id <id>]
    [--source <id>] [--start <date>] [--end <date>] [--pax N] [--dry-run]
```

- `--from-offers` — required flag (explicit; reserves room for future sources without
  changing the default behavior of a bare `promote-offers`).
- `--dest <slug>` — destination to promote into (underscores, e.g. `tokyo_sep_2026`).
  Required: the global `offers.destination` is the selector, so it must be explicit.
- `--plan-id <id>` — the plan that owns the resulting `plan_offers` rows. Resolved via
  `plan_resolver::resolve_plan_id` (so `$TRAVEL_PLAN_ID` works).
- `--source <id>` — optional filter (e.g. only `eztravel`).
- `--start`/`--end` — optional `departure_date` range filter.
- `--pax N` — passengers for `price_total` (default 2).
- `--dry-run` — print what would be promoted; write nothing.

### Read

```sql
SELECT o.id, o.source_id, o.type, o.name, o.price_per_person, o.currency,
       o.availability, o.departure_date, o.hotel_name, o.hotel_area,
       o.flight_outbound, o.flight_return, o.includes, o.scraped_at
FROM offers o
JOIN (SELECT id, MAX(scraped_at) AS latest FROM offers
      WHERE destination = ?1 GROUP BY id) m
  ON o.id = m.id AND o.scraped_at = m.latest
WHERE o.destination = ?1
  [AND o.source_id = ?source]
  [AND o.departure_date >= ?start] [AND o.departure_date <= ?end]
ORDER BY o.source_id, o.id;
```

NOTE: `offers` PK is `(id, scraped_at)` — the same `id` can have multiple scrape
snapshots. The `MAX(scraped_at)` self-join (NOT a bare `SELECT *`) takes the **latest
snapshot per `id`** so a re-scrape supersedes, never duplicates. `scraped_at` is
`NOT NULL` (schema.sql:516) so no SQL-NULL pitfall in the `MAX`.

**Skip NULL price/date (Codex finding #2/#3).** `plan_offer_date_pricing.price` is
`NOT NULL` (schema.sql:633). The source columns `offers.price_per_person` and
`offers.departure_date` are *nullable*. Today all 20 rows have both (verified live:
`null_price=0, null_date=0`), but a strict insert of a future NULL-price/date row would
THROW at insert time. So `promote-offers` **skips any offer with NULL
`price_per_person` OR NULL `departure_date`**, prints a `— N skipped (no price/date)`
note, and does not abort the run. (Fail-soft per-offer, not fail-loud per-run: one bad
row must not block the good rows.)

### Write (per offer, mirroring import-offers' merge-by-id)

1. `delete_offer_rows(plan_id, dest, offer.id)` across the `plan_offer_*` family
   (reuse the exact table list + key logic from `import_offers.rs:407-438`).
2. Insert `plan_offers` (mapping above).
3. Insert one `plan_offer_date_pricing` row.
4. Insert `plan_offer_hotels` (+ no access lines — `offers` has none).
5. Insert `plan_offer_flights` only when flight columns are non-NULL.
6. `plan_offer_includes` from split `includes` (skip if empty).

### Status + audit (once, after all offers)

- `process_statuses` P3_4 → `researched` if currently null/pending/researching, via
  **upsert** (`INSERT … ON CONFLICT … DO UPDATE`, exactly `import_offers.rs:685-708`).
  This is load-bearing (Codex finding #5): `select-offer`'s later `set_status` is a bare
  `UPDATE` (`select_offer.rs:477`), so the P3_4 row **must already exist** before
  `select-offer` runs. A plain `INSERT` (no ON CONFLICT) would also work on a fresh plan
  but would throw on re-promote — use the upsert.
- Events: `package_offers_promoted` (dest_process + timeline) with KV
  `{source_id, offers_found, note}` — a distinct event name from
  `package_offers_imported` so the timeline shows provenance honestly.
- **Audit triad back-half:** call
  `cascade::common::record_operation(conn, plan_id, "promote-offers", &summary,
  version_before, version_after, &now_db)` — this writes the `operation_runs` row
  (`command_type='promote-offers'`, `command_summary="<dest>: <src>:<n>, …"`) **and**
  bumps `plans.version` in one call (Codex finding #4). Do NOT hand-roll the
  `operation_runs` INSERT + `plans.version` UPDATE — `import_offers.rs:355-371` does that
  by hand but that pattern is stale relative to current repo guidance (CLAUDE.md:55).

(Reuse `cascade::common` helpers: `record_operation`, `insert_event`, `insert_kv_rows`,
`next_dest_process_sort_order`, `next_timeline_sort_order`, `now_db_datetime`,
`now_rfc3339`, `read_version`. Emit the events FIRST, then call `record_operation` once
at the end — that ordering is the audit-triad contract.)

### Output (plain text, agent-first — no JSON)

```
Promoting offers for <dest> from global offers table[ (dry-run)]...
  <source>: <count> offer(s)[ — N skipped (no price/date)]
Saved to Turso (plan_offers).
```

Edge: no matching offers → `No offers to promote (no matching rows in offers table).`,
exit 0. The per-source `— N skipped (no price/date)` note appears only when ≥1 offer for
that source was dropped for NULL price/date.

## Module + dispatch

- **CREATE** `rust/crates/travel-cli/src/promote_offers.rs` — `PromoteOpts`,
  `parse_args`, `run`. Structurally parallels `import_offers.rs` but reads the `offers`
  table instead of files. **Copy** the delete-across-the-`plan_offer_*`-family logic
  (`import_offers.rs:407-438`) into this module — `import_offers::delete_offer_rows` is
  **private** (Codex finding #7b), so it can't be called; copy the table list + key
  logic verbatim. Use the `cascade::common` audit helpers; do NOT re-roll the triad.
- **EDIT** `rust/crates/travel-cli/src/main.rs` — add a dispatch arm
  `[cmd, rest @ ..] if cmd == "promote-offers"` mirroring the `import-offers` arm
  (lines 146-154), with `--help` text.
- **EDIT** `rust/crates/travel-cli/src/main.rs` — add `mod promote_offers;` alongside the
  other `mod` declarations (e.g. next to `mod import_offers;` at line 16). **There is no
  `lib.rs`** — modules are declared in `main.rs` (Codex finding #7a).

## A precondition the user must satisfy (out of scope for this command)

`select-offer` needs a **plan** (`plans` row + `plan_metadata.active_destination`).
Today only the `destination_config` slug + the global offers exist — there is no
`tokyo-sep-2026` plan. Creating that plan is a separate step (`shaping-adopt` or a
seed); `promote-offers` does NOT create plans. The spec documents this so the
end-to-end demo (`promote-offers` → `select-offer`) has a clear prerequisite, but the
prerequisite is not part of this command.

## Tests

Real-Turso integration test `rust/crates/travel-cli/tests/promote_offers.rs`
(skips cleanly if creds absent), mirroring the existing integration-test pattern:

1. **Seed** a few global `offers` rows (one with hotel + NULL flights = the real case;
   one with flights non-NULL to prove the flight path) + a minimal plan/plan_metadata.
2. **Run** the binary: `promote-offers --from-offers --dest <slug> --plan-id <id>`.
3. **Assert** `plan_offers` / `plan_offer_date_pricing` / `plan_offer_hotels` rows
   exist with the mapped values; `plan_offer_flights` present only for the
   flights-non-NULL offer; P3_4 = `researched`; one `operation_runs` row;
   `plans.version` bumped by 1.
4. **Then** run `select-offer <id> <departure_date>` and assert P4 → populated (and P3
   → populated only for the flights offer). Proves the bridge end-to-end.
5. `--dry-run` writes nothing (row counts unchanged).
6. Latest-snapshot: seed the same `id` twice with different `scraped_at`; assert only
   the latest is promoted.
7. Skip-NULL: seed one offer with NULL `price_per_person` (or NULL `departure_date`);
   assert it is skipped (no `plan_offers` row), the run still succeeds, and the good
   rows are promoted.
8. Re-promote idempotency: run twice; assert no duplicate rows and P3_4 stays
   `researched` (the upsert path, Codex finding #5).
9. **Teardown** all seeded rows.

## Verification

```
cd rust && cargo test -p travel-cli --test promote_offers   # green (or skip if no creds)
make check                                                  # build
# live demo (after a plan exists):
TRAVEL_PLAN_ID=<plan> ./bin/travel promote-offers --from-offers --dest tokyo_sep_2026 --dry-run
```

## Out of scope (YAGNI)

- Creating plans (separate `shaping-adopt`/seed step).
- Parsing flight detail from offer-id strings (fabrication; rejected above).
- Promoting non-package types — the mapping is type-agnostic, so flight/hotel offers
  promote too, but no current rows exercise those paths; tests cover the package +
  synthetic-flight cases only.

## Review corrections (Codex 2026-06-28, each corroborated against source)

All findings were verified against the cited file:line and live DB before folding in.

1. **Audit triad — use `record_operation`** (common.rs:291). The spec originally listed
   the helpers but omitted it; `import_offers.rs:355-371` hand-rolls operation_runs +
   plans.version, which is the stale pattern. Corrected in "Status + audit".
2. **P3_4 must be UPSERTed** — `select-offer`'s `set_status` is a bare `UPDATE`
   (select_offer.rs:477); the row must pre-exist. Corrected to require the
   `ON CONFLICT … DO UPDATE` upsert.
3. **No `lib.rs`** — modules are declared in `main.rs` (mod import_offers; line 16).
   Corrected the "module + dispatch" section.
4. **`delete_offer_rows` is private** (import_offers.rs:407) — copy the logic, don't
   call it. Corrected.
5. **Nullable source price/date** — `plan_offer_date_pricing.price` is NOT NULL but
   `offers.price_per_person`/`departure_date` are nullable. Added per-offer fail-soft
   skip (no current rows trigger it; future-proofing).
6. **Latest-snapshot needs a `MAX(scraped_at)` self-join**, not a bare `SELECT *`.
   Corrected the read query.

Findings #1 (mapping OK, provenance optional), #6 (flight decision OK) confirmed sound
with no change needed. `plan_offer_provenance` is intentionally left out (select-offer
doesn't need it; freshness/reporting can be added later if wanted — YAGNI for now).
