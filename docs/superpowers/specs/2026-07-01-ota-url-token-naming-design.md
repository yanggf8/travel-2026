# Design: clearer naming for the OTA URL-parameter record

**Date:** 2026-07-01 · **Status:** DRAFT — for Codex review + corroboration.
**Context:** Tier-1 + Plan 1 shipped `ota_source_url_token` (the "union token set") + `set-ota-url-token`.
Yang: the naming is muddy and doesn't scale to a source with many parameters. Fix it BEFORE Plan 2 seeds
more rows.

## The problem (Yang's read of the current record)
A source URL has many parameters (agoda's has 8: `hotel_slug, city_slug, country, checkin, nights,
adults, rooms, currency`). Each DISTINCT one is one row in `ota_source_url_token`, whose columns are a
cryptic 4-tuple:
```
placeholder | input_key | input_value | token_value
```
Yang's clearer mental model of what each column IS:
1. **`placeholder`** — it's really **the URL parameter NAME** (the `{name}` slot in the URL template).
   "placeholder" is too generic to convey "this is a URL param".
2. **`input_key` / `input_value`** — these are **OUR internal/caller-side parameters** (like function/
   Rust parameters we pass in: `destination = tokyo`). Caller-facing, not URL-facing.
3. **`token_value`** — it's **the actual value** substituted into that URL parameter (the real value
   joined onto the URL-param name). "token" is jargon; it's just the resolved URL value.

So a row reads: *"when our internal parameter (input_key = input_value) is X, the URL parameter
(placeholder) takes actual value (token_value) Y."* The column names hide that sentence.

## Goal (Yang's option 1)
**Arrange each parameter as ONE clear, well-named record** so a source's many params read cleanly.
Rename the columns (and the table/command/repo fns/seed) so the three roles are obvious:
URL-param-name · the internal parameter it's keyed by · the actual URL value.

## Proposed renames (the crux Codex should sanity-check)
| Current | Proposed | Why |
|---------|----------|-----|
| table `ota_source_url_token` | `ota_source_url_param` | it's a per-URL-parameter record, not a "token" |
| col `placeholder` | `url_param` | the URL parameter name it fills |
| col `input_key` | `param_name` | our internal/caller parameter name (like a Rust param): `destination`/`hotel` |
| col `input_value` | `param_value` | that parameter's value: `tokyo` |
| col `token_value` | `url_value` | the actual value substituted into the URL param: `295`/`NRT` |
| command `set-ota-url-token` | `set-ota-url-param` | registers one URL-parameter record |
| repo `url_token(...)` | `url_param_value(...)` | returns the url_value for a (source, type, url_param, param_name, param_value) |
| repo `url_token_input_keys(...)` | `url_param_names(...)` | the internal param_names registered for a url_param |
| seed `ota_source_url_token.seed.sql` | `ota_source_url_param.seed.sql` | |

New row reads as a sentence: `set-ota-url-param besttour group_tour region_id  destination tokyo  295`
= "besttour group_tour's URL param `region_id`, keyed by our `destination=tokyo`, has URL value `295`."

## Consistency note (name alignment across the two tables)
`product_type_inputs.input_name` is ALSO "our internal parameter" (destination/depart/currency/…). If we
rename `input_key` → `param_name` here, consider whether `product_type_inputs.input_name` should become
`param_name` too, so BOTH tables call our caller-side inputs the same thing. (Open question for Codex —
consistency vs. churn; Plan 1 just shipped product_type_inputs so renaming it now is cheap-ish but wider.)

## Scope / blast radius (bounded — all callers are ours, few rows)
Rename touches ONLY the OTA url-token sites: `db_migrate.rs` (table DDL + rebuild/rename + seed helper),
`ota/run.rs` (resolver loop), `set_ota_catalog.rs` (`run_set_url_token`), `main.rs` (dispatch arm),
`repo/ota_source_workflow.rs` (`url_token`/`url_token_input_keys`), the seed file, and the 4 test files
that reference them. It MUST NOT touch the unrelated `token_value`/`placeholder` hits in
`share_token.rs`, `set_airport_transfer.rs`, `offers.rs`, `hotels.rs`, `freshness.rs`, `db_query_offers.rs`,
`view_bookings.rs` (grep for the OTA url-token context specifically).

## Migration (SQLite table rename)
The live `ota_source_url_token` has 5 rows (besttour/settour/eztravel/travel4u). Rename via a guarded,
idempotent rebuild (create `ota_source_url_param` with renamed cols → `INSERT … SELECT` copy → drop old →
done), OR `ALTER TABLE … RENAME TO` + `ALTER TABLE … RENAME COLUMN` (SQLite ≥3.25 supports RENAME COLUMN;
confirm libsql supports it) — Codex advise which is cleaner/safer given the CHECK-rebuild precedents.
Update the seed to the new names. Idempotency: skip if the new table already exists.

## Open questions for Codex
1. Are the proposed names right, or is there a better fit? (esp. `url_param` vs `url_param_name`;
   `param_name`/`param_value` vs keeping `input_*`; `url_value` vs `actual_value`.)
2. Rename `product_type_inputs.input_name` → `param_name` too for cross-table consistency, or leave it
   (churn vs. consistency)?
3. Migration mechanism: table+column RENAME vs. rebuild-and-copy — which is safer here (5 rows, live DB,
   consistent with the Tier-1/Plan-1 rebuild pattern)?
4. Is this rename worth doing NOW (before Plan 2 seeds agoda/google_flights), or should the names ship
   as-is and be renamed later? (I lean: now — Plan 2 will add ~5 more rows + the hotel param_name, so
   renaming after doubles the churn.)
5. Any caller/site the blast-radius list misses? Corroborate the grep.
