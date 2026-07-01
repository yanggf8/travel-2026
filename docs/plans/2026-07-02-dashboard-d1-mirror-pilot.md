# Runbook: D1 read-mirror pilot (OTA execution spec Phase G)

**Date:** 2026-07-02 · **Status:** CODE PREPARED (compile-safe, inert); pilot gated on owner action.
**Sign-off:** given (Yang, 2026-07-02). **Owner action required:** `wrangler d1 create` + schema/data
load + deploy run on Yang's Cloudflare account — Claude cannot provision D1 or deploy.

## Goal & guardrails (Codex-advised, adopted)

Measure the **libSQL↔D1 SQL-dialect / type / ordering delta** on real data, to decide whether D1 could
ever serve dashboard reads — **without risking the live dashboard**. Hard rules:
- **Compare-only. D1 NEVER serves live traffic.** Turso stays the sole serving source.
- **Owner-gated + flag-gated.** The compare route is owner-session-only AND returns 404 unless the pilot
  is explicitly enabled. A normal deploy with no D1 database behaves exactly as today.
- **Not wired into `turso::pipeline`.** A separate module (`src/d1_compare.rs`) so it can't subtly
  change live query behavior.
- **Read-both-and-compare**, not mirror-and-serve.

## What Claude prepared (in-repo, compile-safe, INERT)

- **`workers/trip-dashboard-rs/src/d1_compare.rs`** — the compare module: reads the pilot tables from
  BOTH Turso (via `turso::pipeline`) and D1 (via the `MIRROR_DB` binding), diffs field-by-field, and
  returns a plain-text delta report. `enabled(env)` returns true only when the flag AND binding exist.
- **Route `/diag/d1-compare`** in `router.rs` — owner-only (403 otherwise); returns 404 when
  `d1_compare::enabled()` is false. Plain-text, `private, no-store`.
- **`Cargo.toml`** — enabled the `worker` crate's `d1` feature (needed for `env.d1()`). Verified: the
  worker still builds to wasm cleanly (`wrangler deploy --dry-run` exit 0); with no `MIRROR_DB`
  binding + no flag, the route is 404 and nothing changes.
- **Pilot tables (in `d1_compare.rs::PILOT_QUERIES`):** `plans` (small stable core) + `date_anchors`
  (per-(plan,destination) detail the dashboard reads a lot). Deliberately tiny — the goal is to surface
  dialect/type/order deltas, not mirror the DB. Edit that const to change the set.

No live behavior changed until the owner provisions D1 + sets the flag.

## Pilot steps (you run these)

1. **Create the D1 database:**
   ```bash
   cd workers/trip-dashboard-rs
   unset CLOUDFLARE_API_TOKEN && npx wrangler d1 create trip-dashboard-mirror
   ```
   Copy the printed `database_id`.
2. **Bind it + set the flag** — add to `wrangler.toml` (top-level, or the `[env.production]` block if
   piloting on the reclaimed URL):
   ```toml
   [[d1_databases]]
   binding = "MIRROR_DB"
   database_name = "trip-dashboard-mirror"
   database_id = "<paste the id>"

   [vars]
   D1_COMPARE_ENABLED = "1"
   ```
   (For the production env, use `[[env.production.d1_databases]]` and `[env.production.vars]`.)
3. **Load schema + a snapshot of the 2 pilot tables into D1** (the delta is only meaningful on real
   data). Export from Turso and import to D1:
   ```bash
   # schema for just the 2 tables (from the repo schema or `./bin/travel db schema plans|date_anchors`)
   npx wrangler d1 execute trip-dashboard-mirror --command "CREATE TABLE plans (...); CREATE TABLE date_anchors (...);"
   # data: dump the rows from Turso and INSERT into D1 (small tables — a handful of INSERTs)
   #   ./bin/travel db exec "SELECT ... FROM plans"  → turn into INSERTs → wrangler d1 execute --file dump.sql
   ```
   Keep the D1 schema BYTE-for-byte from the repo schema so any delta is a true dialect delta, not a
   schema typo.
4. **Deploy** (`--env production` if on the reclaimed URL, else plain):
   ```bash
   unset CLOUDFLARE_API_TOKEN && npx wrangler deploy   # (or --env production)
   ```
5. **Run the compare** (owner-logged-in browser or authed curl):
   `https://<worker-url>/diag/d1-compare` → plain-text report:
   ```
   table            turso_rows  d1_rows  verdict
   plans                     3        3  MATCH
   date_anchors              3        3  row 1 key 'days': VALUE DIFFERS (turso=5 d1="5")   ← example delta
   ```
6. **Read the deltas** — each mismatch names the exact libSQL↔D1 difference (row count / column set /
   value / type-representation). That list IS the pilot's output: what would need fixing before D1 could
   serve reads.

## Teardown / decision

- **Disable:** remove `D1_COMPARE_ENABLED` (or set to "0") + redeploy → route 404s, D1 untouched.
- **Destroy:** `npx wrangler d1 delete trip-dashboard-mirror` when done.
- **Decision gate:** if the delta is empty/trivial → a future Phase (D1 read-mirror serving, still
  behind a flag) is worth scoping. If the delta is substantial → document it and keep Turso. Either way
  the OTA **write** path never moves to D1/Workers (OTA execution is local-only).

## Notes
- `values_equal` in `d1_compare.rs` tolerates the number-vs-string representation delta (Turso pipeline
  decodes typed scalars; D1 returns JSON) so it flags real value differences, not representation noise —
  but a representation-only difference is itself a documented dialect finding.
- The pilot is publish-side only; it reuses the dashboard's READ Turso token, never the write token.
