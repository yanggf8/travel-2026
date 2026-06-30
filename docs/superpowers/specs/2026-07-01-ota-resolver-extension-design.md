# Design: OTA resolver — product_type input contract (type covers IN → PROCESS → OUT)

**Date:** 2026-07-01 · **Status:** DRAFT — Codex-reviewed + corroborated. **Supersedes** this doc's own
earlier "flat COMMON map + ad-hoc input_key" approach (never built — the redirect below replaces it).
**Builds on:** `2026-07-01-ota-source-registry-and-strategies.md` (Tier-1, shipped `089407a`).

## The model (Yang's direction): the 4 product_types ARE the type system
`product_type ∈ {flight, hotel, fit, group_tour}` is the organizing contract across the whole pipeline —
"**type covers IN → PROCESS → OUT**":
- **IN** — each product_type declares its canonical inputs: which COMMON standard inputs it takes, and
  which DISTINCT **token-key roles** it needs. This contract lives in a **DB table** (`product_type_inputs`).
- **PROCESS** — the resolver dispatches on the source's product_type, reads that type's contract, fills
  COMMON from the standard-input map (+ DB defaults), and fills each DISTINCT role from
  `ota_source_url_token`.
- **OUT** — product_type already determines the offer kind (`offer_row_kind`: flight→flight, hotel→hotel,
  else→package — common.rs:79; used at common.rs:398, validated write_offers.rs:242). Already type-driven.

### The crux Codex resolved (corroborated): TYPE owns the contract, SOURCE owns the placeholder
A per-type contract must NOT enumerate literal placeholder names, because sources of the **same type use
different placeholder spellings**:
- `besttour/group_tour` → `{region_id}` vs `travel4u/group_tour` → `{area_code}` — both `group_tour`,
  both `input_key=destination`, different placeholder (corroborated live in `ota_source_url_token`).
- `settour/fit` has `{region_id}` but `eztravel/fit` does not — same divergence within `fit`.

So: **TYPE owns canonical inputs + token-key ROLES; SOURCE owns its placeholder→value binding** (the
existing `ota_source_url_token` rows). `group_tour` declares "needs a `destination` token_key"; besttour
binds that role to `{region_id}`, travel4u to `{area_code}`.

## Keying: deterministic-when-clear, **LLM judge when ambiguous** (Yang)
`ota_source_url_token.input_key` SURVIVES as the physical lookup key (it is already a PK component;
hotel tokens need a different namespace than destination tokens). The keying is a 3-tier fallback:
1. **Type contract (deterministic, authoritative):** the resolver knows from `product_type_inputs`
   which token-key roles the type uses, and looks up each DISTINCT placeholder's token by that key. This
   handles every clean case (all 6 sources today).
2. **Fail loud** when a required token row is missing (naming the placeholder + role).
3. **LLM-judge fallback when keying is genuinely AMBIGUOUS** — e.g. a placeholder that could resolve via
   more than one registered key, or a binding the contract can't disambiguate. Rather than stacking
   brittle precedence heuristics, the resolver surfaces the ambiguity to the agent (the coding agent
   already driving `ota run` + extraction is the parser/judge). No silent guessing, no fragile rule.
   (Today no source is ambiguous; this is the designed escape hatch, not built-now machinery — like the
   Tier-2 `custom:` seam.)

## `product_type_inputs` — the DB contract table (Codex's minimal schema)
```sql
CREATE TABLE product_type_inputs (
  product_type   TEXT NOT NULL,
  input_name     TEXT NOT NULL,                      -- canonical input/role name (NOT a placeholder spelling)
  input_class    TEXT NOT NULL CHECK(input_class IN ('common','token_key')),
  required       INTEGER NOT NULL DEFAULT 1 CHECK(required IN (0,1)),
  default_source TEXT CHECK(default_source IS NULL OR default_source IN ('caller','db','code')),
  sort_order     INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (product_type, input_name)
);
```
Seed (the 4 type contracts; data, so a 5th type = INSERT, no rebuild):

| product_type | rows (`input_name` `input_class` [default_source]) |
|---|---|
| `flight` | `destination` token_key · `depart` common · `return` common · `origin` common db · `currency` common db |
| `hotel` | `destination` token_key · `hotel` token_key · `depart` common · `nights` common · `pax` common · `rooms` common code · `currency` common db |
| `fit` | `destination` token_key · `depart` common · `return` common · `pax` common |
| `group_tour` | `destination` token_key |

- `common` inputs are filled from the standard-input map; `token_key` roles from `ota_source_url_token`.
- `default_source`: `caller` = must be passed/required; `db` = read from DB defaults; `code` = code default.
- The table declares **canonical inputs/roles, never placeholder spellings** (those stay per-source).

## COMMON standard-input map (aliases in CODE, defaults from DB)
The resolver builds a map covering COMMON inputs; aliases are a fixed code table, defaults are DB-sourced:
- **Aliases (code):** `depart` → `depart`/`depart_date`/`checkin`; `return` → `return`/`return_date`;
  `pax` → `pax`/`adults`. (Semantic API aliases = mapping logic, not provider data — Codex #4.)
- **Defaults from DB (never hardcoded — Codex #3, corroborated live):** `origin` ← `origin_airports`
  WHERE slug=`global_config.default_origin` ORDER BY sort_order LIMIT 1 (= TPE); `currency` ←
  `origin_config[default_origin].currency` (= TWD). `rooms` defaults to `1` in CODE (query shape, not
  provider/project data). Dates/destination/pax fail loud if unsupplied.
- A small `travel-db` reader (e.g. `repo::origin::default_origin_airport_and_currency(conn)`) supplies
  the DB defaults — the resolver is the first CLI consumer of these.

## How a source resolves (PROCESS, end to end)
For `ota run --capture-only <source> <product_type> --destination … [--hotel …] [--depart …] …`:
1. Load the workflow row; `match nav_kind` "get".
2. Read `product_type_inputs` for `<product_type>` → the canonical input contract.
3. For each COMMON input: fill from caller flag, else DB default (origin/currency), else code default
   (rooms), else fail loud if required. Apply code aliases so every placeholder spelling is covered.
4. For each `token_key` role: for every URL placeholder bound to that role, look up
   `ota_source_url_token(source, product_type, placeholder, input_key=<role>, input_value=<caller value>)`.
   Exactly-one clean hit → use it; missing → fail loud; **ambiguous → LLM-judge fallback** (above).
5. `resolve_url(template, map)` (pure; already fails loud naming any still-missing placeholder).

## What changes / stays / defers
- **CHANGE:** new `product_type_inputs` table + seed; new caller flags `--origin`/`--currency`/`--rooms`/
  `--hotel`; `VALID_PARAM_KEYS` + `ota_job_params.param_key` CHECK widened (idempotent table-rebuild,
  same hazard class as Tier-1 — mirror `migrate_ota_job_params_destination`); a `travel-db` DB-default
  reader; the resolver loop rewritten to be contract-driven; `set-ota-url-token` accepts
  `input_key ∈ {destination, hotel}`.
- **KEEP:** `ota_source_url_token` table shape (no change — `input_key` already exists), `resolve_url`/
  `find_placeholders`/`insert_param`, the job lifecycle, `nav_kind` match, the audit triad, all Tier-1
  tests, `offer_row_kind` (OUT already type-driven).
- **DEFER:** the LLM-judge ambiguity path is a documented seam (no source needs it yet); child pax;
  per-destination currency; any `input_key` beyond destination/hotel; Tier-2 `custom:` strategies.

## Build decomposition (2 sequential plans — Codex; Yang approved)
- **Plan 1 — contract + resolver mechanics:** `product_type_inputs` (+seed the 4 contracts), widen
  inputs/flags/CHECK, the DB-default reader, the contract-driven resolver loop with deterministic keying
  + the fail-loud/agent-fallback boundary, `set-ota-url-token` hotel support. Verified against the 4
  already-registered sources (no behavior change for them) + unit tests.
- **Plan 2 — source onboarding (data):** register `travel4u` (seed it — it is live but not in seed),
  `google_flights`, `agoda` as workflow+token rows (agoda needs `input_key='hotel'` for `{hotel_slug}`;
  real slug/dest values come from each source's live capture, not invented), + the 6-source acceptance
  test. Honest-seed rule: seed only proven values.

## Open items folded from review (no longer open)
- `input_key` survives (Codex #1, option a + contract validation). `checkin` aliases `depart` (Codex #2).
  Defaults DB-sourced (Codex #3). Aliases in code (Codex #4). group_tour/fit placeholder variance handled
  by TYPE-owns-role / SOURCE-owns-placeholder. `product_types` is closed by seed discipline (no CHECK).
- **Honest-seed risk (Codex):** agoda `{hotel_slug}` and google_flights `{dest}` real values are NOT
  known from the design — they come from each source's proven capture. Plan 2 seeds only proven values;
  do not invent slugs.
