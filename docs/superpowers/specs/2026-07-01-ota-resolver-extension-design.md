# Design: OTA resolver extension — common standard inputs + union token set

**Date:** 2026-07-01 · **Status:** DRAFT — Codex-reviewed (ACCEPT-WITH-CHANGES), corroborated, folded in.
**Builds on:** `2026-07-01-ota-source-registry-and-strategies.md` (Tier-1, shipped commit `089407a`:
`ota_source_url_token` + generic destination-keyed resolver).

## Codex review (2026-07-01): ACCEPT-WITH-CHANGES — corroborated by Claude
Every factual claim verified vs source; the load-bearing ones:

| Claim | Verdict | Evidence |
|-------|---------|----------|
| Tier-1 resolver is destination-only (`run.rs` "get" arm calls `url_token(...,"destination",...)`) | CONFIRMED | run.rs:174/188/192 |
| `ota_source_url_token` already has `input_key` (PK incl. it); adding `hotel` is NO schema change | CONFIRMED | db_migrate.rs:892; repo ota_source_workflow.rs:71; no CHECK on input_key |
| `set-ota-url-token` hard-rejects non-`destination` | CONFIRMED | set_ota_catalog.rs:301 |
| New inputs need `VALID_PARAM_KEYS` + `ota_job_params.param_key` CHECK widened (table-rebuild) | CONFIRMED | common.rs:9; db_migrate.rs:823; rebuild pattern db_migrate.rs:108 |
| **Origin/currency defaults ALREADY live in DB — must NOT hardcode** | CONFIRMED | `global_config.default_origin=taiwan` → `origin_config[taiwan].currency=TWD` (verified live); CLAUDE.md:390 "config in Turso" |
| travel4u/agoda/google_flights need workflow+token registration (travel4u registered LIVE but not in seed) | CONFIRMED | seed has only settour/eztravel/besttour (ota_source_workflow.seed.sql) |

**Four open questions — RESOLVED (folded into the design below):**
1. **DISTINCT input_key discovery → option (b)** data-driven, WITH an ambiguity guard: query the
   distinct `input_key`s registered for `(source,product_type,placeholder)`, try only keys whose caller
   value is present, require **exactly one** to resolve; if `destination` AND `hotel` rows both exist and
   both caller values are present, fail loud unless only one yields a token. No name-convention, no
   binding table until ambiguity becomes real.
2. **`checkin` alias of `depart`** → keep (package: equal; hotel-only: trip start = depart).
3. **Defaults from DB, NOT hardcoded** → `origin`/`currency` resolve from `global_config.default_origin`
   → `origin_config`; only `rooms=1` may default in code (query shape, not provider/project data).
   Hardcoding `TPE`/`TWD` would violate no-hardcode and duplicate a DB fact.
4. **COMMON alias table stays CODE** → `depart→depart_date/checkin`, `return→return_date`, `pax→adults`
   are semantic API aliases (mapping logic), not provider data.

**Build-blocking gaps (must be in the plan):** `ota run` doesn't accept/persist `--origin`/`--currency`/
`--rooms`/`--hotel`; `VALID_PARAM_KEYS` + the CHECK lack them; `set-ota-url-token` rejects `hotel`; the
resolver always passes `"destination"`; no repo query for registered input_keys; and travel4u/agoda/
google_flights need workflow+token registration (+ seed) before the 6-source acceptance test passes.

## The requirement (Yang's, restated)
> *"First extract the common fields and map them, then keep a union set per distinct field."*

Tier-1 resolves every non-standard placeholder via ONE rule: a token row keyed on `destination`. That
covers besttour/settour/eztravel/travel4u, but **google_flights** and **agoda** have placeholders that
are neither standard dates/pax NOR destination-keyed: `{origin}`, `{currency}`, `{adults}`, `{rooms}`,
`{nights}`, `{checkin}` (caller/global inputs) and `{hotel_slug}` (keyed on a *hotel*, not a
destination). This extension generalizes the resolver so those two onboard as pure DB data too — no
per-source code.

## The classification (data-driven, from the real templates)

Placeholder universe across all 6 sources (besttour, settour, eztravel, travel4u, google_flights,
agoda):

| Class | Placeholders | Filled from |
|-------|--------------|-------------|
| **COMMON** (a recurring search input — map ONCE, applies to every source) | `depart` / `depart_date`, `return` / `return_date`, `pax`, `adults`, `rooms`, `nights`, `checkin`, `origin`, `currency` | the standard-input map (caller flags + defaults) |
| **DISTINCT** (per-source/per-entity — accumulate in the UNION token set) | `dest`, `dest_code`, `city_slug`, `country`, `region_id`, `area_code` (destination-keyed); `hotel_slug` (hotel-keyed) | `ota_source_url_token`, looked up by the placeholder's `input_key` |

Two lookups, in order, for each placeholder:
1. **COMMON** → fill from the standard-input map (below). No DB.
2. **DISTINCT** → look up in `ota_source_url_token` for this `(source, product_type, placeholder)`,
   matching whatever `input_key` is registered for it (`destination` or `hotel`), using the caller's
   value for that input_key. Fail loud naming the placeholder if neither hits.

No `if source ==` anywhere; no precedence guessing — a placeholder is either a known COMMON name or a
registered DISTINCT token, else it is a fail-loud config error.

## Part 1 — COMMON: the standard-input map (extract once)

The caller passes generic search inputs; the resolver builds a map covering every COMMON placeholder,
with **aliases** (one input fills several placeholder spellings) and **defaults** (so a source can use
a global field the caller didn't pass):

| Standard input | CLI flag | Fills placeholders | Default if unset |
|----------------|----------|--------------------|------------------|
| destination | `--destination` | (keys DISTINCT tokens; not itself a placeholder) | — (required when any DISTINCT token is destination-keyed) |
| depart | `--depart` | `depart`, `depart_date`, `checkin` | — |
| return | `--return` | `return`, `return_date` | — |
| nights | `--nights` | `nights` | — |
| pax | `--pax` | `pax`, `adults` | — |
| rooms | `--rooms` | `rooms` | `1` |
| origin | `--origin` | `origin` | **from DB**: `global_config.default_origin` → `origin_config` airport |
| currency | `--currency` | `currency` | **from DB**: `origin_config[default_origin].currency` (= TWD) |

- Aliases are a fixed, code-level table (e.g. `depart` → also fills `depart_date`/`checkin`). This is
  the "extract the common fields and map them" step — one shared mapping, not per-source. (Codex #4:
  semantic API aliases are mapping logic, not provider data → code, not DB.)
- **Origin/currency defaults come from DB, never hardcoded** (Codex #3, corroborated): resolve
  `global_config.default_origin` (`taiwan`) → `origin_config` row for currency (`TWD`) and the origin
  airport. `rooms` defaults to `1` in code (query shape, not provider/project data). Hardcoding `TPE`/
  `TWD` would violate the no-hardcode rule and duplicate a DB fact.
  - *Note:* `origin_config` carries `currency`/`country_code`/`timezone` but NOT the airport code (`TPE`).
    The plan must locate the origin-airport source (likely an `origin_config` column to add, or an
    existing per-origin field) — currency is unambiguously DB-sourced; airport source is a plan TODO.
- A COMMON placeholder the caller didn't supply, with no DB default and no code default → fail loud
  naming it (dates/destination/pax always fail loud; origin/currency/rooms have a default).
- `pax` derives `adults`. (No child-count modelling yet — YAGNI; add when a source needs `{child}`.)

## Part 2 — DISTINCT: the union token set (already built, add `input_key='hotel'`)

The union set IS the existing `ota_source_url_token` table; its `input_key` column already supports
this (Tier-1 reserved it). Extension:
- v1 supported `input_key='destination'` only. Add **`input_key='hotel'`** as the second allowed value,
  for `{hotel_slug}`-style per-entity tokens. The resolver, for a DISTINCT placeholder, reads the
  token row by the `input_key` the registration used and the caller's value for that key.
- `--hotel <slug-key>` becomes a caller input (the key side, e.g. `--hotel hotel-azat-naha`); the token
  row maps `(agoda, hotel, hotel_slug, hotel, <key>) → <real agoda slug>`. (For agoda, `city_slug` and
  `country` are destination-keyed tokens; `hotel_slug` is hotel-keyed.)
- `set-ota-url-token` accepts `input_key ∈ {destination, hotel}` (was destination-only). The resolver
  must know, per placeholder, WHICH input_key to use — see Open Question 1.

## How the two new sources onboard (the acceptance test)

**google_flights** (`flight`): all placeholders are COMMON except `{dest}` (destination token).
```
set-ota-workflow google_flights flight --nav get --url-template '…{dest}…{origin}…{depart_date}…{return_date}…{currency}…'
set-ota-url-token google_flights flight dest destination tokyo NRT   # or the slug Google expects
ota run --capture-only google_flights flight --destination tokyo --origin TPE --depart … --return …
   → {origin}=TPE (default ok), {currency}=TWD (default), dates from flags, {dest} from token. Resolves.
```
**agoda** (`hotel`): `{checkin}`/`{nights}`/`{adults}`/`{rooms}`/`{currency}` COMMON; `{city_slug}`/
`{country}` destination-tokens; `{hotel_slug}` hotel-token.
```
set-ota-url-token agoda hotel city_slug destination tokyo tokyo-jp
set-ota-url-token agoda hotel country   destination tokyo jp
set-ota-url-token agoda hotel hotel_slug hotel hotel-azat-naha azat-naha-the-real-agoda-slug
ota run --capture-only agoda hotel --destination tokyo --hotel hotel-azat-naha --depart … --nights 4 --pax 2
```
Zero Rust per source.

## What changes / what stays
- **CHANGE:** the resolver's per-placeholder fill — COMMON map (with aliases + defaults) first, then
  union-token lookup by `input_key`; new caller flags `--rooms`/`--origin`/`--currency`/`--hotel`;
  `VALID_PARAM_KEYS` gains the new inputs; `set-ota-url-token` accepts `input_key ∈ {destination,hotel}`.
- **KEEP:** `ota_source_url_token` table shape (no schema change — `input_key` already exists),
  `resolve_url` (pure), the job lifecycle, `nav_kind` match, the audit triad, all Tier-1 tests.
- **DEFER:** child pax, multi-currency-per-destination, any `input_key` beyond destination/hotel,
  Tier-2 `custom:` strategies.

## DISTINCT input_key discovery — the resolver algorithm (Codex #1, option b + guard)
For a DISTINCT placeholder `p` of `(source, product_type)`:
1. Query the registered input_keys: `SELECT DISTINCT input_key FROM ota_source_url_token WHERE
   source_id=? AND product_type=? AND placeholder=p`.
2. For each such `input_key`, if the caller supplied a value for that key (`destination` from
   `--destination`, `hotel` from `--hotel`), look up the token row. Collect the hits.
3. **Exactly one hit** → use it. **Zero** → fail loud "no url-token for {p} (tried input_keys …)".
   **More than one** (both `destination` and `hotel` rows resolve) → fail loud as ambiguous. This
   ambiguity guard means a placeholder must resolve to a single token; no silent precedence.
Data-driven: the token rows themselves declare which input_key a placeholder uses; no name-convention,
no binding table (defer (c) until ambiguity is real domain data).

## Open questions — RESOLVED by the Codex review
1. **input_key discovery** → option (b) data-driven + ambiguity guard (algorithm above). (Codex #1)
2. **`checkin` alias of `depart`** → keep; equal for packages, = trip start for hotel-only. (Codex #2)
3. **Defaults** → `origin`/`currency` from DB (`global_config.default_origin`→`origin_config`), never
   hardcoded; `rooms=1` in code (query shape); dates/destination/pax fail loud. (Codex #3, corroborated)
4. **COMMON alias table** → code (semantic API aliases, not provider data). (Codex #4)
