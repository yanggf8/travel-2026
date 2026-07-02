# Deploy-day checklist — dashboard cutover, then D1 pilot

**Date:** 2026-07-02 · **Run order:** Phase 1 (cutover) → verify stable → Phase 2 (D1 pilot).
Do them in this order (Codex-advised): cut over first and confirm it's healthy before layering the D1
pilot on top. Each phase has a **STOP gate** — do not proceed if a gate fails; roll back instead.

All commands run from `workers/trip-dashboard-rs/` on Yang's Cloudflare account. `unset
CLOUDFLARE_API_TOKEN` is in each block on purpose (wrangler uses the OAuth login, not the env token).
Detailed rationale/gotchas live in the two source runbooks — this file is just the ordered driver:
- Cutover: `docs/plans/2026-07-02-dashboard-cutover-ts-to-rs.md`
- D1 pilot: `docs/plans/2026-07-02-dashboard-d1-mirror-pilot.md`

Prep (in-repo, already committed `b1dc335`): the `[env.production]` wrangler block, `src/d1_compare.rs`,
the `/diag/d1-compare` route, the `d1` feature. Nothing below changes code — it's deploy + config only.

---

## Pre-flight (2 min)

```bash
cd ~/b/travel-2026 && git pull --no-rebase        # get latest master
cd workers/trip-dashboard-rs
unset CLOUDFLARE_API_TOKEN && npx wrangler deploy --env production --dry-run   # must exit 0 (wasm builds)
```
**GATE 0:** dry-run prints "Total Upload …" and exits 0. If not, stop and fix the build first.

---

## Phase 1 — Cutover: reclaim `trip-dashboard.yanggf.workers.dev` for the `-rs` worker

### 1a. Baseline the -rs worker is healthy on its own URL
- Open `https://trip-dashboard-rs.yanggf.workers.dev` → owner login works, a plan renders.
- Open `…/?plan=okinawa-2026&token=<a valid share token>` in a logged-out window → renders.
  **GATE 1a:** both work. (If `-rs` itself is broken, do NOT cut over.)

### 1b. Point the GitHub OAuth app at the target URL (the #1 gotcha — do FIRST)
- GitHub → Settings → Developer settings → OAuth Apps → the dashboard app → **Authorization callback
  URL**: add `https://trip-dashboard.yanggf.workers.dev/auth/callback` (keep the `-rs` one too).

### 1c. Deploy -rs under the production name (reclaims the URL from the TS worker)
```bash
unset CLOUDFLARE_API_TOKEN && npx wrangler deploy --env production
```

### 1d. Set the production-env secrets (env-scoped — top-level secrets do NOT carry over)
```bash
cd ~/b/travel-2026
TU=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
TT=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
cd workers/trip-dashboard-rs
unset CLOUDFLARE_API_TOKEN
printf 'https://trip-dashboard.yanggf.workers.dev' | npx wrangler secret put PUBLIC_ORIGIN --env production   # ← the CHANGED one
printf '%s' "$TU" | npx wrangler secret put TURSO_URL --env production
printf '%s' "$TT" | npx wrangler secret put TURSO_TOKEN --env production
npx wrangler secret put SESSION_SECRET     --env production   # same value as -rs (paste when prompted)
npx wrangler secret put GITHUB_CLIENT_ID   --env production   # same as -rs
npx wrangler secret put GITHUB_CLIENT_SECRET --env production # same as -rs
```
(Vars `ALLOWED_LOGIN`/`ALLOWED_GITHUB_ID` come from `[env.production.vars]` — no secret put. R2
`MAPS`/`VOUCHERS` bind the same existing buckets.)

### 1e. Redeploy so the new secrets take effect
```bash
unset CLOUDFLARE_API_TOKEN && npx wrangler deploy --env production
```

### 1f. Smoke test on the RECLAIMED URL `https://trip-dashboard.yanggf.workers.dev`
- [ ] Owner login → OAuth callback → owner dashboard renders (proves the PUBLIC_ORIGIN/callback fix).
- [ ] Logout works.
- [ ] `…/?plan=<slug>&token=<share>` renders logged-out (share path intact).
- [ ] A per-day map image loads (R2 MAPS).
- [ ] `…/?lang=en` renders English.

**GATE 1 (STOP):** all 5 pass → cutover is live; continue. If ANY fail (esp. login → likely the
callback/PUBLIC_ORIGIN mismatch), **ROLLBACK:**
```bash
cd ~/b/travel-2026/workers/trip-dashboard && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy
```
(redeploys the TS worker as `trip-dashboard`; old URL restored in one deploy). Then debug before retry.

### 1g. Let it bake (recommended: a day) before retiring the TS worker
Leave the TS worker dormant as instant rollback. When confident:
`cd ~/b/travel-2026/workers/trip-dashboard && npx wrangler delete` (optional).

---

## Phase 2 — D1 read-mirror pilot (only after Phase 1 is stable)

Compare-only; D1 never serves. See the D1 runbook for the full rationale.

### 2a. Create the D1 database
```bash
cd ~/b/travel-2026/workers/trip-dashboard-rs
unset CLOUDFLARE_API_TOKEN && npx wrangler d1 create trip-dashboard-mirror   # copy the printed database_id
```

### 2b. Bind it + set the flag (in the SAME env you deployed — `[env.production]` after cutover)
Add to `workers/trip-dashboard-rs/wrangler.toml`:
```toml
[[env.production.d1_databases]]
binding = "MIRROR_DB"
database_name = "trip-dashboard-mirror"
database_id = "<paste the id>"

[env.production.vars]
ALLOWED_LOGIN = "yanggf8"          # keep the existing vars
ALLOWED_GITHUB_ID = "48974237"
D1_COMPARE_ENABLED = "1"           # add this
```

### 2c. Load the 2 pilot tables' schema + a real snapshot into D1
Use the byte-identical schema for `plans` + `date_anchors` from the repo, then load current rows:
```bash
cd ~/b/travel-2026
./bin/travel db schema plans        # copy the CREATE TABLE
./bin/travel db schema date_anchors
# create them in D1:
cd workers/trip-dashboard-rs
npx wrangler d1 execute trip-dashboard-mirror --command "<CREATE TABLE plans …>; <CREATE TABLE date_anchors …>"
# dump rows from Turso → INSERTs → load (both tables are tiny):
#   ./bin/travel db exec "SELECT plan_id, schema_version FROM plans"        → INSERTs
#   ./bin/travel db exec "SELECT plan_id,destination,start_date,end_date,days FROM date_anchors" → INSERTs
npx wrangler d1 execute trip-dashboard-mirror --file /path/to/pilot-seed.sql
```

### 2d. Deploy + run the compare
```bash
unset CLOUDFLARE_API_TOKEN && npx wrangler deploy --env production
```
Open (owner logged in): `https://trip-dashboard.yanggf.workers.dev/diag/d1-compare`
→ plain-text report: `table  turso_rows  d1_rows  verdict`.

**GATE 2:** the route returns the report (not 404/403). 404 = flag/binding not picked up (recheck 2b +
redeploy). 403 = not logged in as owner.

### 2e. Read the delta = the pilot's output
- `MATCH` on both tables → the libSQL↔D1 dialect delta is trivial for these reads; a future D1-read-mirror
  phase is worth scoping (still flag-gated, still Turso-serving-primary).
- Any `ROW COUNT / COLUMN SET / VALUE DIFFERS` → that line names the exact delta to fix before D1 could
  ever serve. Record it; decision is yours.

### 2f. Teardown when done
```bash
# disable: set D1_COMPARE_ENABLED="0" (or remove) in wrangler.toml → deploy → /diag/d1-compare 404s
unset CLOUDFLARE_API_TOKEN && npx wrangler d1 delete trip-dashboard-mirror   # destroy the mirror
```
The OTA **write** path never moves to D1/Workers regardless — this pilot is publish-side, read-only.

---

## After the day
Update CLAUDE.md: cutover EXECUTED (the `-rs` worker now serves `trip-dashboard`; TS retired/dormant),
and the D1 pilot RESULT (the measured delta + go/no-go on a future D1 phase).
