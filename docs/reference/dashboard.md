# Trip Dashboard (Cloudflare Worker)

> Moved from CLAUDE.md 2026-09-28 (CLAUDE.md now holds rules only). This file holds the
> dashboard worker's full operational detail: architecture, auth/sharing, maps, deploy,
> troubleshooting. The hard rules (no OSM raster tiles, credits, `unset CLOUDFLARE_API_TOKEN`
> before deploy) are restated in CLAUDE.md.

## Workers

**Two workers exist** (both read Turso directly, SSR):
- `workers/trip-dashboard-rs/` — **Rust / workers-rs** (current; **at TS feature parity** as of 2026-06-29 — the booking-summary package-offer pricing, hotel access lines, per-day map landmarks, and Japan-only entry rows were the last render gaps, now ported with tests (commit `e7c2a89`); `airport_transfer_candidates` is intentionally out-of-scope — the legacy TS worker reads but never rendered it and no plan has rows. Audit: `.review/rs-worker-completeness-audit-2026-06-29.md`). Live at **`trip-dashboard-rs.yanggf.workers.dev`**. **Auth: GitHub OAuth for owner dashboard pages** (gated on immutable GitHub id `ALLOWED_GITHUB_ID` + `ALLOWED_LOGIN`; signed `__Host-td_session` cookie; styled sign-in / not-authorized pages; routes `/auth/login|callback|logout`) via the **shared `gwebcdb/crates/worker-github-oauth` crate** (the SAME crate the finance `plan-viewer-rs` worker uses — cross-repo path-dep like `turso-util`). **Sharing unchanged**: per-plan share tokens in `plan_share_tokens` → `?token=<tok>` still render for logged-out viewers (NOT OAuth-gated). `?token=` is viewer-only; owner access is OAuth-session-only; authenticated HTML is served `Cache-Control: private, no-store` because pages can contain bearer voucher/share URLs. OAuth config: secrets `SESSION_SECRET`/`GITHUB_CLIENT_ID`/`GITHUB_CLIENT_SECRET`/`PUBLIC_ORIGIN` + vars `ALLOWED_LOGIN`/`ALLOWED_GITHUB_ID` (deploy steps: `docs/plans/2026-06-23-dashboard-github-oauth.md`). Plus route diagrams (per-day + plan PNGs with numbered markers + route polylines, Rust route renderer→R2 buckets `MAPS`/`VOUCHERS`). Snapshots composite an ArcGIS Online static basemap (World Street Map → World Topo Map fallback, via the MapServer `export` endpoint in Web Mercator, aligned 1:1 with the renderer's own Mercator projection) under stored OSM-derived coordinates/road geometry; when ArcGIS is unreachable the background falls back to an Overpass-derived road web. They must never request or save `tile.openstreetmap.org` raster tiles, since the OSM tile policy prohibits automated/headless tile fetching and stored tile composites. Keep visible `© OpenStreetMap contributors` credit for the OSM-derived data, plus `© Esri` whenever a basemap is composited. Map PNGs are served `max-age=86400`, so verify a re-snapshot with `curl`, not a browser still holding the old image. Meal-pin `<label>｜map:<query>` links, pending-booking alerts, transit cheat-sheet, clickable flight links. Non-place activities (flights/airport steps/bare meals) are excluded from stop links + maps. **Share a plan:** `./bin/travel share-token` (mint) / `share-token --show` (fingerprints + status) / `share-token --show-full` (full sensitive URLs) / `share-token deactivate <token>`; or on the signed-in dashboard/plan page, **Copy share link** (clipboard = `?plan=<slug>&token=<share_token>` for viewers — no login for recipients). Plan: `docs/plans/2026-06-25-dashboard-share-link-copy.md`. **Refresh maps:** `./bin/travel snapshot-maps`.
- `archive/ts-dashboard-retired/` — **legacy TS worker**, RETIRED 2026-07-02 (undeployed; -rs reached full TS parity 2026-06-29). **Archived 2026-08-26** — the overdue archive-or-delete review resolved as ARCHIVE. The old `trip-dashboard.yanggf.workers.dev` 301-redirects → `-rs` via `workers/trip-dashboard-redirect/` (preserves path+query so old `?plan=…&token=…` share links still land). Operational details: [`docs/history/dashboard.md`](../history/dashboard.md).

## Plan-map split

`<plan-id>/plan.png` shows itinerary places with hotel and airport endpoints excluded from its bounds; `<plan-id>/plan-logistics.png` contains those hotel/airport endpoints. Per-day maps remain full route diagrams. **DOMESTIC (Taiwan) plans have NO airport segment, so there is no plan-logistics map at all** — snapshot-maps merges the booked stay's pin into plan.png (the stay is a real itinerary place for a self-drive trip) and records plan-logistics.png as skipped; the worker renders the logistics slot only when flights exist (a domestic page otherwise showed a 地圖尚未產生 placeholder wasting space). When flights DO exist (Japan), the worker renders the logistics map as a compact inset (`map-frame--inset`, ~360px 崁入小圖), not a second full-size map. The snapshot renderer composites an ArcGIS static basemap (Overpass road web as fallback) under stored OSM-derived coordinates and road geometry; it does not load OSM raster tiles.

Note: the CLI's `crate::checks` module (shift-left audit) holds shared lint predicates (`check_stop_linkable`, `stop_link_problem`, …) used by BOTH the read-only lints AND write-time guards in `set-route-segment`/`set-tod` — so a stop that won't form a valid Maps link is rejected at write time, not just flagged later. (The former `../travel-2026-dashboard-rs` worktree was removed once the branch merged; all dashboard-rs work is on `master` now.)

## Request path
```
Browser → Cloudflare Worker (SSR HTML) → Turso HTTP Pipeline API → normalized tables → assemble plan object → render
```

## Behavior
- **SSR-only** — no client JS for viewers; logged-in owner plan pages include minimal inline JS for **Copy share link** (clipboard). (The `-rs` worker has no edit mode; the retired legacy TS edit mode is in [`docs/history/dashboard.md`](../history/dashboard.md).)
- **Mobile-first** — phone-optimized day cards with weather (including feels-like temperature), transit, meals
- **Default ZH** — Traditional Chinese by default; `?lang=en` for English
- **ZH content** — All Chinese content stored in DB (`theme_zh`, `focus_zh`, `transit_notes_zh` scalars; ZH activities in `session_activities_zh`; meals in `session_meals`). No hardcoded content in Worker code. Content updates take effect instantly without redeploy. Use `set-day-theme --zh` for day themes, `set-tod-zh` (alias: `set-session-zh`) for session focus/transit/activities, `set-route-segment` for Chinese place names. For bulk new-destination ZH population: copy `scripts/set-kyoto-zh-sessions-v2.ts` pattern (parameterized Turso pipeline queries — required for Unicode/emoji content).
- **Anti-translate** — `lang="zh-TW"` + `<meta name="google" content="notranslate">` prevents browser auto-translation of the ZH page.
- **Multi-plan** — each plan accessed via `?plan=<slug>` (e.g., `tokyo-2026`, `kyoto-2026`). Slug derived from `active_destination` (underscores → hyphens). Root `/` shows plan index page listing all plans.
- **Plan nav** — hidden by default for privacy (shareable links show single plan only); add `&nav=1` to show pill-style plan switcher (plan list from DB via `listPlans()`)
- **Flight links** — Flight numbers in booking summary are clickable Google search links (opens new tab)
- **Activity links** — `https://` URLs embedded in activity text are auto-linkified (`renderActivityText()`); `\n` in activity text renders as `<br>`
- **Day card accents** — colored left border by day type: blue (arrival), green (full day), amber (departure)
- **Routes**: `/` (plan index), `/?plan=<slug>` (single plan, shareable), `/?plan=<slug>&nav=1` (with plan switcher), `/?plan=<slug>&lang=en` (EN), `/api/plan/<id>` (raw JSON)
- **Maps links** — Per-segment Google Maps direction links (transit/walking/driving) for every stop. Route segments stored in `day_route_segments` table, landmarks in `day_landmarks` table. Transit pill text must use place names, not service names (e.g., `成田T2 → 日暮里` not `Skyliner → 日暮里`)
- **候選位置小圖 (domestic candidate minimaps)** — Each `domestic_accommodations` candidate card with `latitude`/`longitude` renders an ArcGIS static-export basemap whose bbox covers the stay + the plan's itinerary points (`plan.png` legend stops): 📍 on the stay, blue labeled dots on the stops, Mercator-positioned inline, caption shows 距<最近景點> 約 N km, image links to Google Maps. Same ArcGIS static basemap as the route snapshots (never OSM raster tiles; keep `© Esri`). No stops → tight stay-centered fallback crop.
- **Secrets**: `TURSO_URL` + `TURSO_TOKEN` + `GOOGLE_MAPS_KEY` (optional) via `wrangler secret put` (server-side only, never sent to browser — except Maps key which is browser-visible by design; restrict via GCP Console referrer policy)
- **Self-contained** — no dependency on `src/` code, own `package.json` + `tsconfig.json`
- **Live URL** (canonical): `https://trip-dashboard-rs.yanggf.workers.dev/?plan=tokyo-2026` | `/?plan=kyoto-2026`. The old `trip-dashboard.yanggf.workers.dev` 301-redirects here (path+query preserved).
- **Itinerary formats**: Supports both session-based (Tokyo) and schedule-based (Kyoto) formats. See `src/skills/travel-shared/references/itinerary-formats.md`

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Itinerary shows blank/empty | Schedule-based format not converted | Check `render` handles both formats |
| Wrong plan content | Plan not synced to Turso | Run `./bin/travel db seed plans` |
| "Plan not found" error | Plan ID mismatch (underscore vs hyphen) | URL uses `tokyo-2026`, DB uses `tokyo_2026` |
| ZH content not showing | Missing `_zh` columns in DB | Run `set-tod-zh` CLI per session, or bulk-populate via `scripts/set-kyoto-zh-sessions-v2.ts` pattern |
| ZH UPDATE silently fails (rows_affected=0) | Inline SQL with Unicode/emoji fails encoding | Use parameterized Turso queries: `args:[{type:"text",value:"..."},{type:"integer",value:"1"}]` — integer value must be a string |
| Weather missing | Weather not fetched | Run `./bin/travel fetch-weather --dest <slug>` |
| Maps embed not showing | No `GOOGLE_MAPS_KEY` secret | `wrangler secret put GOOGLE_MAPS_KEY` (restrict key to Maps Embed API + referrer in GCP Console) |

## Deploy
```bash
# The LIVE worker is trip-dashboard-rs (Rust). Deploy:
cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy   # runs worker-build --release
# Set a secret (one-time): pipe from .env, e.g.
TURSO_TOKEN=$(grep '^TURSO_TOKEN=' ../../.env | cut -d= -f2-) && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put TURSO_TOKEN <<< "$TURSO_TOKEN"
```
