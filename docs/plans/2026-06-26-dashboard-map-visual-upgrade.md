# Dashboard Route Maps — Visual Upgrade (catch up to Wanderlog)

Status: **TIER 1 IMPLEMENTED (code, 2026-06-26); live visual confirmation PENDING.** Tier 2/3 not
started.
Reviewed by Codex (gpt-5.5, 2026-06-26); all 10 findings + 4 gotchas **corroborated against
source** (router.rs:44/51/54, map.rs:51, db_migrate.rs:319/341, check_maps_fresh.rs:25,
snapshot-maps.sh:173/190/191/198/214) and folded in below. Tier 1's *approach* is sound and
headless-safe (Codex #6); the corrections were two factual misstatements in Tier 1's wording and
three real Tier-2 blockers (renderer interface, per-leg cache spec, no-JSON-in-RDB) — all now
addressed in the text.

### Implementation note (Tier 1 — DONE in code)
Tier 1 landed in `scripts/snapshot-maps.sh` (`render_map()` heredoc only, +27/−10): CARTO Positron
muted basemap, metric scale bar (`L.control.scale`), white-cased + dashed route line, day-colored
teardrop SVG pins **anchored at the tip** (`iconAnchor:[14,40]`), and a burned-in
`© OpenStreetMap, © CARTO` credit. All keyless. Verified: `bash -n` passes; a harness proved the
CARTO `{r}` token stays a literal Leaflet template (not shell-expanded) while `${arr}` interpolates;
`${arr}` / `attributionControl:false` / `MAP_READY` / the screenshot call / the fail-loud PNG guard
are all untouched. **Delegation note:** assigned to `grok-composer-2.5-fast` via `--prompt-file`,
which hit the documented single-turn failure (narrated the change, wrote zero files) — applied the
verbatim spec by hand instead. **Still PENDING (agent-driven):** the agent re-runs the snapshot and
self-verifies the `okinawa-2026` captures — this artifact is for the dashboard/agent pipeline, not a
human eyeball. The agent escalates to the user ONLY on a real blocker (no Chrome on `:9222`, capture
fails the PNG guard). That re-run needs Chrome on `:9222` via `./bin/chromeport` — the retired-tool
dependency this plan flags below.

Scope owner: `scripts/snapshot-maps.sh` (the keyless static-PNG map renderer).

## Goal

The dashboard's per-day and plan-overview maps are keyless static PNGs: a self-contained
Leaflet page (OSM raster tiles + Leaflet from unpkg) is generated per day and screenshotted to
a PNG that is uploaded to the R2 bucket the worker serves (`<plan-id>/day-<n>.png`,
`<plan-id>/plan.png`). Compared side-by-side with **Wanderlog** (the benchmark consumer travel
planner), ours looks dated in three concrete ways:

1. **Markers** are flat colored circles with a number — Wanderlog uses day-colored *teardrop*
   pins that read as map pins, not buttons.
2. **The basemap** is raw `tile.openstreetmap.org`, which is busy/high-contrast — Wanderlog uses
   a muted basemap (CARTO Positron / Stadia) so the route + pins pop.
3. **The route line is a straight polyline** between consecutive stops — it cuts diagonally
   through buildings/water. Wanderlog either omits the line at island zoom or follows roads.
   (There is **no scale reference** on ours either.)

This plan upgrades the *visual layer only*, in tiers, and **preserves the keyless static-PNG
architecture** — no GCP Static Maps key, no interactive client map, no new worker route. (Per
the standing `no-gcp-maps-key` constraint and the SSR-only viewer design.)

Non-goal: changing where stops come from, the freshness/manifest pipeline, the R2 keys, or the
worker's `render/map.rs` consumer. Those stay exactly as they are.

## Current state (verified facts the plan relies on)

All in `scripts/snapshot-maps.sh` unless noted. Line numbers are at the time of writing.

- **One renderer**, `render_map()` (lines 168–225), builds a Leaflet HTML string and
  screenshots it. It receives stdin lines `"lat,lon,COLOR"` (already in itinerary order), turns
  them into a JS `pts` array (line 174), and:
  - tile layer: `L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png',{maxZoom:19})`
    (line 191) — **raw OSM**.
  - framing: single point → `setView(ll[0],14)`; else `fitBounds(ll,{padding:[45,45]})` (line 192).
  - **route line**: one `L.polyline(run,{color,weight:4,opacity:.85})` **per contiguous
    same-color run** (lines 194–197) — so each day is its own straight line; days are not chained.
  - **markers**: `L.divIcon` with an inline `<div>` — a 24px **circle** (`border-radius:50%`),
    day color background, white border, the 1-based index as text (lines 199–204).
  - readiness: sets `document.title='MAP_READY'` after 1200 ms (line 205); the screenshot waits
    9000 ms (line 212).
- **Colors**: `DAY_COLORS` (line 46) is a 7-entry hex palette; `process_day` (line 229) and the
  overview (line 269) pick `DAY_COLORS[(d-1) % 7]`. Per-day maps are single-color; the overview
  tags each day's coords with that day's color so `render_map`'s same-color-run logic draws N
  separate day lines.
- **Coords source**, `day_coords()` (lines 133–162): POIs first
  (`activities JOIN destination_pois`, ordered by `sort_order`); else the `day_route_segments`
  place sequence geocoded via `geocode_place()` (Nominatim, cached in `route_place_geocodes`).
  Order is itinerary order. This is the **stop sequence** the route line connects.
- **Screenshot driver**: `$CHROMEPORT screenshot "$data_url" --out … --width 640 --height 440
  --wait 9000` (line 211). `CHROMEPORT="./bin/chromeport"` (line 38). The script also uses
  `$CHROMEPORT db query` / `db exec` as its Turso CLI wrapper throughout.
  **Caveat (must be acknowledged, see Risks):** per `CLAUDE.md`, **chromeport is RETIRED** and
  OTA/browser work moved to **gwebcdb on WSLg**. This script was not migrated; it still shells to
  `./bin/chromeport`. The visual upgrade is *independent* of the driver — it only changes the
  generated HTML — but anyone re-running the script needs a working `./bin/chromeport screenshot`
  (CDP Chrome on :9222) or a gwebcdb equivalent. Migrating the driver is **out of scope here**
  but flagged.
- **Geocode context** is **hardcoded** `GEO_CONTEXT="Naha, Okinawa, Japan"` (line 44). The
  script is currently Okinawa-shaped. Any Tier-2 routing addition must not assume Okinawa beyond
  what already exists, and must degrade cleanly for a non-Okinawa plan.
- **Fail-loud invariants** (lines 214–223): a real PNG must exist, start with the PNG magic
  `89504e47`, and exceed `MIN_PNG_BYTES=200`; otherwise the stub is removed and the file is
  never uploaded. Every key's outcome is written to `map_artifacts`. **These must remain intact.**
- **Worker consumer**: `workers/trip-dashboard-rs/src/render/map.rs` + the `/map/*` route serve
  the PNGs from R2 by the `<plan-id>/day-<n>.png` / `<plan-id>/plan.png` keys. **No worker change
  is needed** for Tier 1/2 — the PNG content changes, the keys/routes do not. (The plan does not
  touch `map.rs`.)

## Tiers

### Tier 1 — Visual polish (pure HTML/CSS in `render_map`, no new deps, no new snapshot-time fetches)

All four changes are edits to the heredoc HTML inside `render_map()` (lines 178–207). No new
data, no new architecture, and **no new snapshot-time shell/API fetches** — Tier 1 stays within
the browser resources the page already loads (Leaflet CSS/JS from unpkg at line 180, OSM tiles at
line 191; CARTO Positron just swaps the tile host, still a browser-side tile fetch, not a new
shell call). Keyless. This is the **recommended first (and possibly only) pass** — highest visual
payoff per unit risk. (Corrected from an earlier "zero network calls" claim — the existing render
already loads Leaflet + tiles over the network; Tier 1 adds no *new* fetch the script makes.)

1. **Teardrop day-colored pins** — replace the circle `divIcon` (lines 200–203) with a teardrop
   shape carrying the same day color + the index number. Two options, both keyless and
   self-contained:
   - **Inline SVG path** in the `divIcon` html (a classic map-pin path, `fill` = day color, the
     number drawn as centered text). `iconSize`/`iconAnchor` set so the *tip* anchors on the
     coord (anchor at bottom-center, e.g. `iconSize:[28,40], iconAnchor:[14,40]`).
   - **CSS teardrop** (a rotated rounded square: `border-radius:50% 50% 50% 0; transform:
     rotate(-45deg)` with the number counter-rotated). Simpler CSS, slightly less crisp than SVG.
   - Recommendation: **inline SVG** — sharpest at screenshot scale, exact anchoring, trivial to
     color per-day. Keep the white halo (`stroke`/`border`) for contrast on the basemap.
   - **Anchor correctness is the load-bearing detail**: the current circle anchors at its center
     (`iconAnchor:[12,12]`); a teardrop must anchor at the **tip** or every pin shifts up by half
     its height. This is the one change most likely to silently misplace pins — call it out in
     the diff and eyeball one capture.

2. **Muted basemap** — swap the raw OSM tile URL (line 191) for a low-contrast basemap so the
   route and pins read clearly. Keyless options:
   - **CARTO Positron**: `https://{s}.basemaps.cartocdn.com/light_all/{z}/{x}/{y}{r}.png`
     (subdomains `a,b,c,d`; retina `{r}` supported). Free, no key, very muted. **Recommended.**
   - **Stadia AlidadeSmooth**: clean but **now requires an API key for production** — *reject*
     (would break keyless). Note explicitly so it isn't re-suggested.
   - Keep `tile.openstreetmap.org` as a documented fallback (CARTO has a fair-use policy; for our
     low volume — a handful of captures per snapshot run — it is within bounds, but note it).
   - **Attribution**: attribution is suppressed **two ways** today — the CSS hides
     `.leaflet-control-attribution` (line 185) **and** the map is created with
     `attributionControl:false` (line 190). So re-enabling Leaflet's tile-option attribution
     would NOT show it; the **only** way to credit is a **burned-in fixed `<div>`** in the HTML
     (`© OpenStreetMap, © CARTO`). CARTO's and OSM's tiles both *require* attribution. Since the
     control is already disabled today, this is a **pre-existing** posture, not a regression — but
     the plan should note it. **Decision needed from the user** (keep suppressed as today / add a
     small burned-in credit). Default: add the small burned-in credit (low effort, removes a
     latent licensing gap; the Leaflet attributionControl path is a dead end here).

3. **Scale bar** — add `L.control.scale({imperial:false, position:'bottomleft'}).addTo(map)`.
   One line, keyless, gives the reader a sense of distance (Wanderlog shows one). It is a Leaflet
   built-in control; the current CSS only hides attribution + zoom controls, so the scale will
   show. Trivial.

4. **Line styling** — make the straight polyline read as a *planned connector*, not a road:
   - **Cased line**: draw a thicker white/low-opacity line under the colored one (two
     `L.polyline` per run: a `weight:7,color:'#fff',opacity:.7` casing, then the colored
     `weight:4` on top). Improves contrast on the muted basemap.
   - **Dashed**: `dashArray:'6,8'` on the colored line signals "as-the-crow-flies connector,"
     visually distinguishing it from a real road and lowering the "why does the line cut through
     a building" reaction. (This is the cheap honest fix for the straight-line complaint without
     Tier 2.)
   - Recommendation: **cased + dashed** together. Both are pure Leaflet options, no deps.

**Tier 1 acceptance (agent-driven — this artifact is for the agent/dashboard pipeline, not a human
eyeball):** the agent re-runs the snapshot for `okinawa-2026` and self-checks the captures:
(a) all expected keys (`plan.png` + each `day-N.png`) write a `map_artifacts` row with
`status=uploaded` and `byte_size > MIN_PNG_BYTES`; (b) each PNG passes the fail-loud guard (magic
`89504e47`, size). To verify the *visual* deltas without a human, the agent inspects the captured
PNG programmatically — e.g. pixel-sample for the muted CARTO palette vs the old high-contrast OSM,
detect the day-color marker hues, confirm the dashed line + scale text region — or, more robustly,
asserts on the generated HTML (the heredoc) that the CARTO tile URL, `L.control.scale`, the
`dashArray` line, the teardrop SVG with `iconAnchor:[14,40]`, and the `.credit` div are present
before the screenshot. The agent escalates to the user ONLY on a real blocker (no Chrome on
`:9222`, a capture that fails the PNG guard, or a CARTO/tile fetch failure). No worker redeploy
needed (PNG content only) — re-run `./bin/travel snapshot-maps` (or the script).

### Tier 2 — Road-following route line (keyless, at snapshot time)

This addresses the literal complaint ("lines are straight, not along real roads"). Replace each
day's straight polyline with a **road geometry** fetched **once at snapshot time** (not in the
browser, not at view time) from a keyless routing service, then drawn as a normal polyline. Still
a static PNG; still no client JS; still no GCP key.

- **Routing source** (keyless options, in preference order):
  - **OSRM public demo** (`router.project-osrm.org`): `GET /route/v1/driving/{lon,lat;…}?overview=full&geometries=geojson`
    returns a road polyline. No key. **But** it is a *demo* server with no SLA / rate limits and
    car-only profiles — acceptable for our tiny volume but **must fail soft** (fall back to the
    straight line on any error/timeout, exactly like geocode failures already do).
  - **OpenRouteService**: better SLA but **requires a free API key** → conflicts with keyless;
    *reject* unless the user opts to store an ORS key as a secret (out of scope, would be a
    different decision than `no-gcp-maps-key`, which is specifically about Google).
  - **Self-hosted OSRM**: most robust, but operational overhead; out of scope for now.
  - Recommendation: **OSRM public demo with hard fail-soft to the straight line.**
- **BLOCKER — `render_map` interface must change first.** Today `render_map` receives ONE stdin
  format, `lat,lon,COLOR`, and reuses that same `pts` array for **both** the route line (line 193)
  **and** a numbered marker per point (line 198–204). A dense OSRM road geometry (dozens–hundreds
  of vertices) cannot be fed through that channel — every road vertex would become a numbered
  marker. **Tier 2 prerequisite:** split the renderer's inputs into TWO arrays — `stops`
  (the numbered markers, today's `pts`) and a separate `routes` (the drawable geometry per
  day-color run). Markers come from `stops`; the polyline is drawn from `routes` if present, else
  falls back to a straight line through `stops` (today's behavior). This is a real (small)
  interface change to `render_map`, not a drop-in — it must land **before** any OSRM wiring.
- **Where**: a new shell function `road_geometry()` called by `process_day`/overview before
  render that takes the ordered `lat,lon` stop list, calls OSRM **per leg** (segment i→i+1),
  parses the GeoJSON `coordinates` with a **JSON parser** (python3, NOT grep — the geometry is
  nested), converts `[lon,lat] → [lat,lon]`, and produces the day's road geometry for the new
  `routes` channel. Mirror `geocode_place`'s discipline exactly.
- **Cache design (per-leg, normalized, no JSON blob).** Two corrections over the earlier draft:
  - **Per-leg, not per-day.** Cache each leg (one OSRM call = one ordered pair of stop coords),
    not "an ordered hash of the day's stops" — the earlier draft contradicted itself (per-day hash
    vs. "keep per-segment granularity"). Per-leg keying lets one unroutable hop fall back to a
    straight connector while the rest of the day still road-follows, and maximizes cache reuse
    (the overview reuses the same legs).
  - **Cache key** = canonicalized `from_lat,from_lon → to_lat,to_lon` at fixed precision (e.g. 5
    dp) **plus** `provider` + `profile` + geometry option, so a profile/option change doesn't
    serve a stale geometry. Store **both** successes and explicit failures (like
    `route_place_geocodes.failure_reason`) so a second run makes **zero** OSRM calls.
  - **NO JSON in the RDB** (repo rule + `no-json-in-rdb`): do NOT store the OSRM GeoJSON blob.
    Existing map tables are fully-normalized scalar columns (`map_artifacts`,
    `route_place_geocodes` — `db_migrate.rs:319`/`:341`). Store the geometry as **normalized point
    rows**: a header table `route_road_legs(leg_key, provider, profile, status, fetched_at, …)`
    + a child `route_road_leg_points(leg_key, point_order, lat, lon)`. Add both via `db_migrate.rs`
    the idempotent way (`CREATE TABLE IF NOT EXISTS`), same as `route_place_geocodes`.
- **Walking vs driving**: keep Tier 2 to **`driving`** only. The earlier claim that the OSRM demo
  offers `driving`/`walking`/`cycling` is **unverified** — OSRM profiles depend on what the server
  was built with; the public demo's documented/working profile is `driving` (path
  `/route/v1/driving/…`). Do NOT build walking/cycling logic without live verification against the
  actual demo host. `driving` is the safe road-following default; per-profile selection is a later
  refinement, not Tier 2.
- **Rate-limit / fail-soft contract (stricter than "sleep between calls").** Per-leg routing
  means up to `stops − 1` calls per day. Required contract: short `--max-time`; identifying UA;
  cache **successes and failures** (zero OSRM calls on a clean re-run); sleep between live calls;
  and — load-bearing — **an OSRM error/timeout must NEVER set the script's `FAILED` flag or the
  fail-loud PNG path** (lines 214–223). OSRM failure → emit no geometry for that leg → straight-
  line fallback. The fail-loud guard stays strictly for actual *PNG* failures.
- **Caveat**: road routing only makes sense **within** a contiguous reachable area. An
  island/over-water hop (e.g. flight legs, or a stop the route engine can't reach) returns no
  route → fall back to the straight line **for that leg only**. Per-leg granularity (above) is
  what makes one unroutable hop not kill the whole day's line. Also note OSRM **snaps stop coords
  to the nearest road**, so a road line may not start exactly under a pin (pin is the true POI,
  line starts at the snapped road point) — acceptable, or add a tiny straight connector from pin
  to road start if it reads badly.

**Tier 2 acceptance:** for a day with road-connected stops, the line follows streets; for an
unroutable hop, it falls back to a (dashed) straight connector with no error; the cache table is
populated; a second run makes **zero** OSRM calls; fail-loud PNG invariants still hold.

### Tier 3 — Interactive map / automatic route optimization — **NOT recommended**

Wanderlog also offers a live interactive map and reorders stops to minimize travel. Both break
our design:
- An **interactive client map** means shipping client JS + tiles to the viewer and (for a good
  experience) a tile/map key — directly against the keyless static-PNG + SSR-only design.
- **Route optimization** (reordering stops) is an *itinerary* change, not a map-rendering change —
  it belongs in the CLI/itinerary layer with the user in the loop, not silently in the snapshot
  script. The current map deliberately renders the itinerary order the user chose.

**Recommendation: skip Tier 3.** If the user ever wants interactivity, that's a separate
architectural decision (would revisit `no-gcp-maps-key` and SSR-only), not a map-polish task.

## Recommendation (sequencing)

1. **Do Tier 1 first** — it is the bulk of the perceived gap (pins + basemap + scale + line
   styling), is pure HTML/CSS in one function, adds no network calls, and cannot regress the
   pipeline (PNG content only). Ship and look at it before deciding on Tier 2.
2. **Then Tier 2** (OSRM road-following with fail-soft + Turso cache) only if, after Tier 1, the
   straight lines still bother the user. The dashed-cased Tier-1 line may already be "honest
   enough."
3. **Skip Tier 3.**

## Files to edit

- **Tier 1**: `scripts/snapshot-maps.sh` only — the `render_map()` heredoc (lines ~178–207): the
  tile URL, the marker `divIcon`, add `L.control.scale`, the polyline styling, and (if chosen)
  a burned-in attribution `<div>`. No other file.
- **Tier 2**: `scripts/snapshot-maps.sh` — split `render_map`'s input into `stops` + `routes`
  arrays (interface change, prerequisite), add `road_geometry()` fn + call site — **and** two new
  normalized Turso tables `route_road_legs` + `route_road_leg_points` (added via `db_migrate.rs`
  the same way `route_place_geocodes` is: idempotent `CREATE TABLE IF NOT EXISTS`, scalar columns,
  **no JSON blob**). No worker change.
- **Neither tier** touches `workers/trip-dashboard-rs/src/render/map.rs`, the `/map/*` route, the
  R2 keys, `map_artifacts`, or the freshness logic.

## Risks / ambiguity (read before implementing)

- **Teardrop anchor**: a teardrop must anchor at its **tip** (`iconAnchor` = bottom-center), not
  its center like the current circle. Get this wrong and every pin shifts off its coordinate.
  This is the single highest-risk Tier-1 change — verify against one capture.
- **chromeport is retired**: the script still shells to `./bin/chromeport screenshot` and
  `db query`/`db exec`. The visual upgrade doesn't depend on the driver, but re-running the
  script requires a working chromeport (CDP Chrome :9222) **or** migrating the driver to gwebcdb
  first. Driver migration is **out of scope** here; flag it so the runner isn't surprised by a
  retired-tool dependency. (If the runner hits a chromeport failure, that's a pre-existing
  blocker, not something this plan introduces.)
- **Basemap fair-use + attribution**: CARTO/OSM tiles require attribution; the current CSS hides
  the attribution control (pre-existing). The plan offers a burned-in credit `<div>` as the
  correct fix. **User decision**: keep suppressed (status quo) vs. add the small credit (default:
  add). CARTO's free tiles have a fair-use cap; our volume (a few captures per run) is well
  under, but note it.
- **Stadia / ORS need keys** → both rejected to preserve keyless. Only CARTO Positron (basemap)
  and OSRM public demo (routing) are keyless; if the user later accepts a key for one of these,
  that's a separate decision from `no-gcp-maps-key` (which is Google-specific).
- **OSRM demo has no SLA / rate limits** and is car-only on the public host. Tier 2 must
  **fail soft** to the straight line on any error/timeout and **cache** results in Turso so
  re-runs don't hammer it — mirroring the existing Nominatim geocode discipline exactly. Without
  fail-soft, a routing outage could blank or fail a map; the fail-loud PNG guard would then
  *correctly* refuse to upload, but the day's map would be missing — so fail-soft-to-straight is
  mandatory, not optional.
- **Okinawa-hardcoded context**: `GEO_CONTEXT` is Okinawa-specific (line 44). Tier 2 routing must
  not add new Okinawa assumptions; it operates on already-resolved coords, so it's destination-
  agnostic by construction — but verify it degrades cleanly (fall back to straight line) for any
  plan whose stops aren't road-reachable.
- **Fail-loud + manifest invariants are sacrosanct**: lines 214–223 (PNG magic/size guard) and
  the `record_artifact` calls must remain exactly as they are. Neither tier may weaken them.
- **Browser/worker cache (will hide the new look on re-run).** `/map/*` responses are served with
  `Cache-Control: public, max-age=86400` (router.rs:54). Re-uploading the same R2 keys with new
  PNG content will **not** show immediately — a viewer's browser/CDN may serve the day-old cached
  PNG. After a Tier-1/2 re-run, verify with a hard refresh / cache-busting query / incognito, and
  note in the acceptance step that the visible change can lag up to 24h without a purge. (No code
  change implied — just an expectation to set.)
- **`check-maps-fresh` is blind to route-only edits.** `day_route_segments` has no `updated_at`
  (documented at `check_maps_fresh.rs:25`), so changing route geometry alone won't make the lint
  report "stale." Tier 2 must not rely on that lint for cache invalidation — re-run snapshot
  after any route edit regardless (the per-leg cache key changes only when coords change, which is
  the correct invalidation trigger anyway).
- **Tile readiness is time-based, not load-based.** The capture waits a fixed 9000 ms (line 212)
  / 1200 ms `MAP_READY` (line 205); a valid-magic PNG can still contain half-loaded tiles. The
  fail-loud guard checks bytes, not visual completeness. Switching basemap host (CARTO) doesn't
  change this, but the agent should pixel-check the captures for blank/partial tiles (e.g. reject a
  capture whose center region is the `#eef` placeholder background). (Pre-existing; not introduced
  by this plan.)
- **Verification is agent-driven, not human eyeball.** These captures are an agent artifact for the
  dashboard pipeline — Yang does not hand-review them. "Looks Wanderlog-grade" must be turned into
  agent-checkable assertions: HTML-template assertions before the screenshot (CARTO URL, scale,
  dashArray, teardrop `iconAnchor:[14,40]`, `.credit`) + post-capture PNG checks (magic/size,
  `map_artifacts` rows, palette/marker-hue pixel sampling, not-blank center). The agent runs the
  snapshot and these checks itself and only pings the user on a real blocker. (A side-by-side
  against the saved Wanderlog reference is a bonus the agent can render, not a required human gate.)

## Deferred / explicit non-goals

- Migrating the screenshot driver from chromeport → gwebcdb (separate task; flagged).
- Per-segment walking/driving profile selection (Tier 2 refinement).
- Interactive client map, route optimization/stop-reordering (Tier 3 — recommended against).
- Any worker (`-rs`) change — keys, routes, `map.rs`, R2 layout all unchanged.
