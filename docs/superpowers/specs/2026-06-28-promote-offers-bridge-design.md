# Design: `promote-offers` — bridge global `offers` → plan-scoped `plan_offers`

**Date:** 2026-06-28
**Status:** Approved (design); implementation pending
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
SELECT id, source_id, type, name, price_per_person, currency, availability,
       departure_date, hotel_name, hotel_area, flight_outbound, flight_return,
       includes, scraped_at
FROM offers
WHERE destination = ?1
  [AND source_id = ?source]
  [AND departure_date >= ?start] [AND departure_date <= ?end]
ORDER BY source_id, id;
```

NOTE: `offers` PK is `(id, scraped_at)` — the same `id` can have multiple scrape
snapshots. `promote-offers` takes the **latest `scraped_at` per `id`** (group by `id`,
`MAX(scraped_at)`) so a re-scrape supersedes, never duplicates.

### Write (per offer, mirroring import-offers' merge-by-id)

1. `delete_offer_rows(plan_id, dest, offer.id)` across the `plan_offer_*` family
   (reuse the exact table list + key logic from `import_offers.rs:407-438`).
2. Insert `plan_offers` (mapping above).
3. Insert one `plan_offer_date_pricing` row.
4. Insert `plan_offer_hotels` (+ no access lines — `offers` has none).
5. Insert `plan_offer_flights` only when flight columns are non-NULL.
6. `plan_offer_includes` from split `includes` (skip if empty).

### Status + audit (once, after all offers)

- `process_statuses` P3_4 → `researched` if currently null/pending/researching
  (identical to `import_offers.rs:268-282`).
- Events: `package_offers_promoted` (dest_process + timeline) with KV
  `{source_id, offers_found, note}` — a distinct event name from
  `package_offers_imported` so the timeline shows provenance honestly.
- `operation_runs` row, `command_type='promote-offers'`,
  `command_summary="<dest>: <src>:<n>, …"`.
- `plans.version` + 1.

(This is the audit triad. Reuse `cascade::common` helpers: `insert_event`,
`insert_kv_rows`, `new_run_id`, `next_dest_process_sort_order`,
`next_timeline_sort_order`, `now_db_datetime`, `now_rfc3339`, `read_version`.)

### Output (plain text, agent-first — no JSON)

```
Promoting offers for <dest> from global offers table[ (dry-run)]...
  <source>: <count> offer(s)[ — N skipped (no hotel)]
Saved to Turso (plan_offers).
```

Edge: no matching offers → `No offers to promote (no matching rows in offers table).`,
exit 0.

## Module + dispatch

- **CREATE** `rust/crates/travel-cli/src/promote_offers.rs` — `PromoteOpts`,
  `parse_args`, `run`. Structurally parallels `import_offers.rs` but reads the `offers`
  table instead of files. Reuse `delete_offer_rows`-equivalent logic and the
  `cascade::common` audit helpers (do NOT re-roll the triad).
- **EDIT** `rust/crates/travel-cli/src/main.rs` — add a dispatch arm
  `[cmd, rest @ ..] if cmd == "promote-offers"` mirroring the `import-offers` arm
  (lines 146-154), with `--help` text.
- **EDIT** `rust/crates/travel-cli/src/lib.rs` (or wherever modules are declared) —
  `mod promote_offers;`.

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
7. **Teardown** all seeded rows.

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
