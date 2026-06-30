# Design: DB-registered OTA sources + a typed strategy escape hatch

**Date:** 2026-07-01 · **Status:** DRAFT — Codex-reviewed (ACCEPT-WITH-CHANGES, folded in). No code yet.
**Author:** Claude (with Yang).
**Builds on:** `2026-06-30-ota-workflow-nodes.md` (the GET-only process-nodes + agent-first extraction).

## Codex review (2026-07-01): ACCEPT-WITH-CHANGES — corroborated by Claude

Codex reviewed this spec; I re-corroborated each finding vs source. The seam is sound, with three
changes (all folded in below):

| Claim | Verdict | Evidence |
|-------|---------|----------|
| Bug real (settour/eztravel templates have unfillable `{dest_code}`/`{region_id}`; only besttour resolves) | CONFIRMED | seed `ota_source_workflow.seed.sql:5`; `run.rs:179/186`; `ota_coverage.seed.sql:27` has only besttour/travel4u region rows. (Caveat: NRT/179900 are from Phase-A captures, in this spec not yet in seed.) |
| Region table is Chinese-`region_label`-keyed, not English slug | CONFIRMED | `db_migrate.rs:672` PK `(source_id,product_type,region_label)`; labels `東京`/`關東`/`mixed`. So keying tokens on `destination=tokyo` is a real schema+data change, NOT a rename. |
| Current `run` body lifts cleanly into a strategy | CONFIRMED | `run.rs:147-239` is linear; only the `{region_id}` branch is source-ish, and it's already generic (no `if source==`). |
| `set-ota-*` is the right pattern to mirror | CONFIRMED | `set_ota_catalog.rs` (validated UPSERT + audit); `set-ota-region` ≈ the shape `set-ota-url-token` generalizes. |
| `async_trait` already a dep | REFUTED (direct); transitive via libsql | no direct dep in either crate; no `Box<dyn>`/async_trait usage in the codebase → don't introduce it. |

**Three required changes (applied):**
1. **Build Tier 1 first; the bug is data-resolution, not strategy polymorphism.** Tier 1 = the token
   table + generic resolver + seed the 3 sources + tests proving all 3 resolve.
2. **Token table needs an `input_key` column** (not a naked `input_value`) so a placeholder can map
   from `destination` vs `origin` vs `pax` unambiguously. v1 supports `input_key='destination'` and
   fails loud on others.
3. **DEFER Tier 2 (the trait) entirely.** Keep `nav_kind` as the future discriminator and widen its
   CHECK to `'get' OR LIKE 'custom:%'`, but build NO strategy abstraction (no `async_trait`, no
   `dyn`) until a real diverged source exists — then a plain `match nav_kind { … }` (enum-style), not
   a trait-object plugin. The escape hatch is a documented seam, not speculative machinery.

## The requirement (Yang's, restated)

> *"I want a common, standard flow manager for OTA sources… so when a new OTA comes, we can easily
> make one… register the OTA source and manage it from the DB… and if some are truly too diverged,
> let's use a type to extend ourselves."*

Two-tier extensibility:
1. **Common path — register-and-run from the DB, zero code.** A new "normal" OTA is onboarded by
   INSERTing config rows (a `travel set-ota-workflow` command). The generic flow reads them. No Rust.
2. **Escape hatch — type-to-extend for the truly divergent.** A source that genuinely can't be config
   (real form-driving, multi-step nav, auth dance) gets ONE Rust trait impl; the DB row names which
   strategy it uses. Divergence is contained in a typed plugin, never smeared through the flow.

This mirrors the parser decision: config/generic by default; when something truly can't be config, it
lives in exactly one bounded place — here a typed strategy, not scattered `if source == "x"`.

## Why the current GET-only build misses the bar (the bug review found)

The shipped `ota/run.rs` + `ota_source_workflow` seed bakes source-specific URL tokens into templates
with **no uniform resolution**: settour needs `{dest_code}`=NRT + `{region_id}`=179900, eztravel needs
`{dest_code}`=TYO, besttour needs `{region_id}`=295. Only besttour resolves (295 is in
`ota_source_region_codes`); settour/eztravel fail loud on unfillable placeholders. So "add a new OTA"
is NOT yet a pure data operation — the per-source token mapping has no DB home. That is the gap this
spec closes.

## Tier 1 — the common, DB-registered flow

### Standard input contract (same for every source)
The agent/user supplies generic search inputs only: `destination` (a canonical slug, e.g. `tokyo`),
`depart`, `return`, `nights`, `pax`. These are the `ota_job_params` already. **No source-specific
values are ever passed by the caller.**

### Generic URL-token resolution (generalize `ota_source_region_codes`)
A source's URL template has `{placeholders}`. The flow fills EACH one uniformly:
1. from a standard input (`{depart}`←depart, `{pax}`←pax, …), else
2. from a generic **per-source token table** keyed by the standard input value.

Rename/generalize the existing region table to a URL-token table (it is already exactly this shape,
just region-only today):
```
ota_source_url_token         -- NEW table (generalizes ota_source_region_codes; see migration note)
  source_id     TEXT NOT NULL
  product_type  TEXT NOT NULL
  placeholder   TEXT NOT NULL   -- the {name} it fills: 'region_id', 'dest_code', 'arr_airport', …
  input_key     TEXT NOT NULL   -- WHICH standard input it maps FROM: 'destination' (v1) | future 'origin'/'pax'/…
  input_value   TEXT NOT NULL   -- that input's value (e.g. 'tokyo')
  token_value   TEXT NOT NULL   -- the source-specific value (295 / 179900 / NRT / TYO)
  PRIMARY KEY (source_id, product_type, placeholder, input_key, input_value)
```
`input_key` (Codex change #2) makes resolution unambiguous: v1 only supports `input_key='destination'`
— the resolver fails loud if a placeholder needs any other input_key (no silent "assume destination").
Seed rows make every source pure data, e.g. for `(input_key=destination, input_value=tokyo)`:
- besttour/group_tour: `region_id → 295`
- settour/fit: `region_id → 179900` ; `dest_code → NRT`
- eztravel/fit: `dest_code → TYO`

> **Migration note (Codex change #4 / biggest risk):** the old `ota_source_region_codes` keys on the
> Chinese `region_label` (東京/關東/mixed); the new table keys on the canonical English `destination`
> slug. This is **NOT mechanically derivable** — `關東`, `東京｜東北`, `mixed` have no clean 1:1 slug.
> So do NOT auto-convert all region rows. Instead **explicitly hand-seed `ota_source_url_token` for the
> known, verified destinations only** (tokyo for the 3 live sources) via the checked-in seed; leave
> `ota_source_region_codes` in place untouched (orphaned-not-dropped) until a later, deliberate
> reconciliation. The token seed is the authoritative slug→value map; there is no auto-migration.

### The resolver (pure + one DB step) — generic, no source names
- pure `resolve_url(template, &map)` (already built, keep it).
- `run` builds the map: standard inputs first (`{depart}`/`{return}`/`{pax}`/`{nights}`); then for EACH
  remaining `{placeholder}`, look it up in `ota_source_url_token(source, product_type, placeholder,
  input_key='destination', input_value=<destination>)`. Fail loud if a placeholder is neither a
  standard input nor a token row (naming it). **No `if source == …` anywhere** — the resolver is one
  generic loop over the template's placeholders.

### Registering / managing a source from the DB
- `travel set-ota-workflow <source> <product_type> --nav <kind> --url-template <t> [--capture-url-contains s] [--settle-ms N] [--settle-marker m] [--note ...]` → upserts the `ota_source_workflow` row.
- `travel set-ota-url-token <source> <product_type> <placeholder> <input_key> <input_value> <token_value>` → upserts a token row.
- (Mirrors the existing `set-ota-source`/`set-ota-coverage`/`set-ota-region` catalog editors — DB-native, audited, no hand-SQL.)
**Onboarding a normal new OTA = run those setters N times. Zero Rust, zero rebuild.**

## Tier 2 — the escape hatch is a DEFERRED seam, not built now (Codex change #3 / YAGNI)

The current need is **data resolution, not strategy polymorphism** — Tier 1 makes all 3 verified
sources run with zero per-source code. So Tier 2 is **NOT built now**. What we reserve is only the
*seam*:
- `ota_source_workflow.nav_kind` is the discriminator. Its CHECK widens to
  `CHECK(nav_kind = 'get' OR nav_kind LIKE 'custom:%')` so a future `custom:<name>` row is storable;
  `run` dispatches `match nav_kind { "get" => <the current GET flow>, other => Err("nav_kind
  '{other}' has no registered strategy") }` — i.e. unknown/custom fails loud today.
- When (and only when) a genuinely-diverged source appears (e.g. liontravel form-drive), add ONE
  `match` arm `"custom:liontravel" => capture_liontravel(...)` — a plain async fn, **no trait, no
  `async_trait`, no `dyn`** (the crate uses neither; introducing them for one built-in flow is
  over-built). If multiple custom strategies later prove a shared interface is worth it, refactor THEN.

So Tier 2's contract: the flow stays identical for every source; divergence, if it ever arrives, is
one `match` arm + one async fn + one `custom:` DB row. The door is open without speculative machinery.

## What stays / what changes from the shipped GET-only build
- KEEP: `ota_source_workflow` table, `resolve_url` (pure), `claim_specific`, `get_params`, the
  `run --capture-only` shell of enqueue→claim→resolve→nav→settle→capture→print-no-offers, the tests.
- CHANGE (Tier 1 only): add the `ota_source_url_token` table; replace the region-only `region_id()`
  branch with a generic loop that resolves EVERY non-standard placeholder via `ota_source_url_token`
  (input_key='destination'); hand-seed besttour/settour/eztravel token rows so all 3 resolve; widen
  the `nav_kind` CHECK to allow `'custom:%'` + make `run` `match` on it (only `'get'` implemented);
  add the `set-ota-workflow`/`set-ota-url-token` registration commands.
- DEFER: Tier 2 custom strategies (no trait built); real form-driving (gets a `custom:` arm if/when a
  source needs it).

## "Onboard a new OTA" — the acceptance test for this design
- **Normal source (the common case):** `set-ota-workflow newsrc fit --nav get --url-template
  '…{dest_code}…{depart}…'` + `set-ota-url-token newsrc fit dest_code destination tokyo XXX` → `ota
  run --capture-only newsrc fit --destination tokyo --depart … --return …` works. **No code changed.**
- **Diverged source (rare):** add a `--nav custom:newsrc` row → `run` fails loud "no registered
  strategy" until someone adds one `match` arm + one async fn. The flow/job-lifecycle/write-offers are
  untouched. (No trait until a real one exists.)

## Migration / sequencing (Tier 1 only; additive)
1. NEW `ota_source_url_token` table (with `input_key`). Hand-seed the known tokens for the 3 verified
   sources at `destination=tokyo` (besttour region_id=295; settour region_id=179900 + dest_code=NRT;
   eztravel dest_code=TYO) in a checked-in seed. Leave `ota_source_region_codes` untouched (no
   auto-migration — labels aren't 1:1 slugs).
2. Generic resolver in `run`: standard inputs, then `ota_source_url_token` for every other placeholder
   (input_key='destination'); fail loud naming any unresolved placeholder. Replace the region-only
   branch. All 3 `run --capture-only` resolve.
3. Widen the `nav_kind` CHECK to `'get' OR LIKE 'custom:%'`; `run` `match`es on `nav_kind` (only
   `'get'` implemented; else fail loud).
4. `set-ota-workflow` + `set-ota-url-token` registration commands (mirror `set-ota-region`).
5. (DEFERRED, not now) the first `custom:` strategy — a plain `match` arm + async fn — only when a
   real diverged source needs it.

## Open questions — RESOLVED by the Codex review
1. **Token input** — v1 keys on `input_key='destination'` only; the `input_key` column keeps it
   extensible (origin/pax later) and the resolver fails loud on any non-destination key. (Codex #2.)
2. **trait vs enum** — neither now: a plain `match nav_kind` with one `'get'` arm. No `async_trait`,
   no `dyn` (not used anywhere in the crate). (Codex #3.)
3. **nav_kind CHECK** — widen to `CHECK(nav_kind = 'get' OR nav_kind LIKE 'custom:%')` + runtime
   fail-loud in the `match`. (Codex #3.)
4. **table** — NEW `ota_source_url_token` + hand-seed known destinations; do NOT auto-migrate the
   Chinese-label region rows (no clean 1:1 slug). (Codex #4 — flagged as the biggest risk.)
