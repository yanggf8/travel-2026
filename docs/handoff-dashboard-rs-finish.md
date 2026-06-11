# Handoff — finish the Rust/WASM dashboard (Tasks 5, 9, 10, 11)

Branch: **`dashboard-rs`** (worktree `/home/yanggf/b/travel-2026-dashboard-rs`).
Spec: `docs/superpowers/specs/2026-06-10-rust-dashboard-redesign-design.md`.
Plan: `docs/superpowers/plans/2026-06-10-rust-dashboard-redesign.md`.

## State (done, all committed on `dashboard-rs`)
Tasks 0–8 complete, TDD + two-stage reviewed, 37 host unit tests green, wasm compiles:
- `src/turso.rs` — Turso `/v2/pipeline` client + `decode_result` (Row = `BTreeMap<String, serde_json::Value>`; **Turso returns all scalars as JSON strings**).
- `src/auth.rs` — `AccessScope{Owner,Plan(slug),Denied}`, `resolve()` (constant-time owner compare, rejects empty token), `can_view_plan()`.
- `src/model.rs` — `Plan/Day/Session/Stop`, `assemble(...9 slices...)`, `SESSION_ORDER` (always 4 sessions → noon can't drop), tolerant POI title match (trim+lowercase), keyless `maps_link`.
- `src/render/{mod,session,day,map,summary,index}.rs` — `esc()` (text/dbl-quoted-attr), `esc_url_attr()` (URLs, preserves `&`), `page()`, `render_plan()`, booking summary (transfer route+price — the old "—" bug is fixed + tested), keyless map `<img>` + per-stop links.
- `src/styles.css` + `styles.rs`, `src/i18n.rs`.
- CLI: `plan_share_tokens` table + `share-token` command (CSPRNG token via getrandom) — already migrated into the live Turso DB; a token for `okinawa-2026` was minted during Task 3's test (mint a fresh one for real use, see Task 9).

The remaining tasks need **your interactive access**: real Windows Chrome (chromeport), `wrangler` + Cloudflare auth, an R2 bucket. Subagents can't do those.

---

## Task 5 — Seed `destination_pois.lat/lon` (sourced via chromeport)
**Needs:** real Chrome at :9222 (chromeport), `.env` Turso creds. The columns already exist? — verify; if not, add them.

1. Ensure columns exist (idempotent; `db_migrate.rs` may already add them — check first):
   ```bash
   ./bin/travel db exec "ALTER TABLE destination_pois ADD COLUMN lat REAL"   # ignore "duplicate column"
   ./bin/travel db exec "ALTER TABLE destination_pois ADD COLUMN lon REAL"
   ```
2. List the Okinawa POIs needing coords:
   ```bash
   ./bin/travel db exec "SELECT poi_id, title FROM destination_pois WHERE slug='okinawa_2026'"
   ```
   (8 POIs: shuri_castle is dropped from the itinerary but the POI row may remain — only the ones actually used in activities need coords for map pins, but seed all for completeness.)
3. For EACH poi, source real coordinates via chromeport (do NOT fabricate):
   ```bash
   ./bin/chromeport fetch url "https://www.google.com/maps/search/<poi+name+naha>" --source google
   # read lat/lon from the captured page text / resolved URL (the @lat,lon in a Maps URL)
   ```
   Record provenance in a note column or the existing provenance fields.
4. Backfill (one-shot SQL is acceptable for a data backfill):
   ```bash
   ./bin/travel db exec "UPDATE destination_pois SET lat=<v>, lon=<v> WHERE slug='okinawa_2026' AND poi_id='<id>'"
   ```
5. Verify none missing:
   ```bash
   ./bin/travel db exec "SELECT poi_id FROM destination_pois WHERE slug='okinawa_2026' AND (lat IS NULL OR lon IS NULL)"   # expect 0 rows
   ```
6. Commit `db_migrate.rs` if you changed it. (The coordinate data lives in Turso, not git.)

**Deferred design note (from Task 4 review):** the model matches activity→POI by title string (now trim+lowercase tolerant). The durable fix is a `poi_id` FK on `activities`. OPTIONAL here — only do it if you also update the CLI's activity-creation path. Otherwise the tolerant title match is sufficient for okinawa-2026 (activity titles were copied from POI titles, so they match).

---

## Task 9 — Router + auth gate + R2 map route + e2e
**Needs:** the router CODE (writable now by anyone), then `wrangler dev` + Cloudflare secrets + an R2 bucket to test live.

This is mostly code; see the plan's Task 9 for the full `router.rs` + `lib.rs` listing. Key contracts to honor (surfaced by the Task 7 reviews):

- **R2 key convention (MUST match `render/map.rs`):** day map = `<plan_id>/day-<n>.png`, plan map = `<plan_id>/plan.png`. The `/map/<plan_id>/<file>.png` route must use the **raw plan_id slug** as the R2 key prefix (no HTML-unescaping needed — slugs are `[a-z0-9-]`).
- **Slug sanitization:** before interpolating `slug` into SQL in `load_plan`, reject anything not matching `^[a-z0-9_-]+$` (the plan loader builds SQL by string interpolation; the slug comes from a token-scoped match or owner, but sanitize anyway). Better: this is the one spot that interpolates into SQL — consider it carefully.
- **Map-miss UX (from Task 7 review):** the `/map/*` route should, on R2 miss, return a tiny "map pending" placeholder PNG (or a 1x1 transparent) with a 404-or-200 — NOT a broken-image. Server-side only (no client JS). Decide 200+placeholder vs 404; placeholder avoids broken-image icons before Task 10 runs.
- **Auth before reads:** `auth::resolve` must run before any plan Turso read; index route (`/` no `?plan`) is Owner-only (403 otherwise); `?plan=<slug>` requires `can_view_plan(scope, slug)`.
- **Read token:** the worker uses ONE server-side Turso **read** token (`TURSO_TOKEN` secret). URL tokens are view-scope only.

Steps:
1. Write `src/router.rs` + update `src/lib.rs` per the plan's Task 9 (with the sanitization + map-miss handling above). `cargo build --target wasm32-unknown-unknown` to confirm it compiles.
2. Mint a fresh real share token for okinawa-2026 (the Task 3 test token was torn down):
   ```bash
   ./bin/travel share-token okinawa-2026   # note the token
   ```
3. Create the R2 bucket + set secrets (interactive Cloudflare auth):
   ```bash
   cd workers/trip-dashboard-rs
   npx wrangler r2 bucket create trip-dashboard-maps
   for s in TURSO_URL TURSO_TOKEN OWNER_TOKEN; do
     v=$(grep "^$s=" ../../.env | cut -d= -f2-); unset CLOUDFLARE_API_TOKEN && echo "$v" | npx wrangler secret put "$s"; done
   # OWNER_TOKEN: choose a long random string; it's the owner's master view token.
   ```
   (TURSO_TOKEN here must be a READ-capable token.)
4. `unset CLOUDFLARE_API_TOKEN && npx wrangler dev`, then verify the access matrix:
   ```bash
   curl "http://localhost:8787/?plan=okinawa-2026&token=$OWNER_TOKEN" | grep -o '中午\|Lunch\|Yui Rail\|波上宮' | sort -u   # content present
   curl "http://localhost:8787/?plan=okinawa-2026&token=<share-token>" -o /dev/null -w '%{http_code}\n'   # 200
   curl "http://localhost:8787/?plan=tokyo-2026&token=<okinawa-share-token>" -o /dev/null -w '%{http_code}\n'  # 403 (wrong plan)
   curl "http://localhost:8787/" -o /dev/null -w '%{http_code}\n'   # 403 (no token, no index)
   curl "http://localhost:8787/?token=$OWNER_TOKEN" -o /dev/null -w '%{http_code}\n'  # 200 (owner index)
   ```
   Confirm the noon/meals/transfers content the OLD TS worker dropped is all present.
5. Commit `src/router.rs` + `src/lib.rs`.

---

## Task 10 — chromeport → R2 map snapshot pipeline
**Needs:** real Chrome (chromeport) + wrangler (R2 put).

1. For each map level, drive a map view with the day's pins in real Chrome and snapshot a PNG. Decide the map UI to drive (a Google Maps URL centered on the stops, or an OSM export). It's a SCREENSHOT — no Maps API key.
   - Plan map: all okinawa_2026 stop coords → one PNG.
   - Day map (×5): that day's stop coords → one PNG.
2. Upload to R2 with the EXACT key convention from Task 9 / `render/map.rs`:
   ```bash
   npx wrangler r2 object put trip-dashboard-maps/okinawa-2026/plan.png --file plan.png
   npx wrangler r2 object put trip-dashboard-maps/okinawa-2026/day-1.png --file day-1.png
   # ... day-2..day-5.png
   ```
3. Verify the worker serves them: `curl -I "http://localhost:8787/map/okinawa-2026/day-2.png"` → `200 image/png`.
4. A `scripts/snapshot-maps.sh` (or a small CLI subcommand) capturing this is nice-to-have for re-runs; commit it.

---

## Task 11 — Deploy staging, parity check, cut over
**Needs:** wrangler deploy (Cloudflare auth) + your decision to flip production.

1. `cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy` — note the `*.workers.dev` URL.
2. Parity check ALL THREE plans + BOTH itinerary formats (session-based okinawa/tokyo, schedule-based kyoto):
   ```bash
   for p in okinawa-2026 tokyo-2026 kyoto-2026; do
     curl -s "https://<rs-worker-url>/?plan=$p&token=$OWNER_TOKEN" -o /tmp/$p.html; done
   ```
   Strip tags; confirm: okinawa noon/meals/transfers present; tokyo (session) renders; kyoto (schedule) renders without a blank itinerary. Compare design/RWD against the (bug-fixed) TS dashboard.
3. **Cutover decision (yours):** when the Rust worker is parity-or-better, point the production route/DNS at it and retire the TS worker.
4. Merge `dashboard-rs` → `master`. **MERGE CAVEAT:** Task 3 (`share-token`) was written against this branch's OLDER plan-resolution form (`env::var("TRAVEL_PLAN_ID")`). Master has since adopted `plan_resolver::resolve_plan_id(rest)` for all mutation arms (see master's `docs/handoff-cli-mutation-bugs.md`). When merging, reconcile the `share-token` dispatch arm in `main.rs` to use `plan_resolver::resolve_plan_id(rest)` like its neighbors, and confirm the `--plan-id`/`--dest` skip in `share_token.rs`'s parser stays consistent.

---

## Cross-cutting reminders (from the reviews)
- Keep all trip content in Turso; no hardcoded okinawa content in worker code.
- Both itinerary formats must keep working (session + schedule) — kyoto is the schedule-based regression check.
- Maps are keyless — chromeport snapshot → R2 → `<img>`; per-stop links are `google.com/maps?q=lat,lon`. Never add a GCP Static Maps API key (memory: `no-gcp-maps-key`).
- The worker is read-only; all writes via `./bin/travel`.
