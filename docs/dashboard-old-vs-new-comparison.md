# Dashboard comparison — old TS worker vs. new Rust worker

**Updated:** 2026-06-11 (CORRECTED). Captured via `chromeport fetch url` (full **rendered text**,
settle-waited — NOT screenshots), plan `okinawa-2026`, both sites live.

> **Correction note:** an earlier version of this doc concluded the old TS site was sparse/buggy
> (empty noon, no meals). That was WRONG — caused by (a) a mis-timed full-page *screenshot* that
> captured a partial render, and (b) grepping for the wrong meal emoji (`🍽️` vs the old site's
> actual `🍜`). A fresh `chromeport fetch url` text capture shows the old site is content-RICH.
> The real gaps are on the NEW Rust worker. Always compare via rendered-text capture, not screenshots.

## URLs
- OLD (TS):  `https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026`  (no token)
- NEW (Rust): `https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=<owner>`

## Feature parity (fresh rendered-text capture, same grep patterns both sides)

| Feature | OLD (TS) | NEW (Rust) | Verdict |
|---|---|---|---|
| Rendered text length | 5696 chars | 5871 chars | ~equal |
| Noon (中午) content | 5 | 4 | both render noon |
| **Meals (🍜)** | **9** | **0** | ❌ NEW BUG — see below |
| **Route segments (今日路線)** | **5 days** | **0** | ❌ NEW MISSING FEATURE — see below |
| Transit pills (→) | 73 | 23 | old has ~3× transit detail (route segments) |
| Reservations (需訂位) | 4 | 4 | parity |
| Shikinaen / AEON / ¥340 transfer | yes | yes | parity |

## Root cause of the two NEW-worker gaps (verified)

Data IS in Turso (so these are worker render/feature gaps, not missing data):
- `session_meals` for okinawa: **9 rows**. The Rust worker DOES query `session_meals` (1 ref in
  router.rs) but renders **0 meals** → a **render/assembly bug** (likely a join-key / session_type
  mismatch in `model::assemble` or `render::session`). The CLI `itinerary` view shows these meals
  fine, so the data path is proven.
- `day_route_segments` for okinawa: **21 rows**. The Rust worker queries it **0 times** → the
  per-day door-to-door routing ("今日路線" — 🚗/🚌/🚶 segments with times) is **not implemented**
  in the Rust worker at all.

## NEW-worker advantages (still real, but narrower than first claimed)
1. **Maps** — plan + 5 per-day keyless OSM maps (chromeport→R2). Old site has none.
2. **Progressive disclosure** — booking minutiae behind `<details>`.
3. **Page weight** — ~31KB raw HTML vs ~60KB (old ships more inline).
4. **Token-scoped access** — owner index + per-plan share links.
5. **Rust/WASM stack** + soft-deleted plans hidden end-to-end.

## Net assessment
The Rust worker is **NOT yet at content parity** with the old TS site. Before cutover it needs:
- **FIX:** meals rendering (queried but dropped — 9 rows → 0 shown).
- **ADD:** `day_route_segments` querying + a "今日路線" per-day route block (21 rows ignored).
Until then the old site is richer on itinerary/transit detail; the new site wins on maps, weight,
auth, and stack. Re-run the parity grep (below) after those two land.

## Rerun (rendered-text capture — the correct method)
```bash
OWNER=<owner-token>
OID=$(./bin/chromeport fetch url "https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026" --source dash | sed -n 's/^capture_id\t//p')
NID=$(./bin/chromeport fetch url "https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=$OWNER" --source dashrs | sed -n 's/^capture_id\t//p')
for pat in 中午 🍜 今日路線 需訂位 識名園 ¥340; do
  o=$(./bin/chromeport db query "SELECT raw_text FROM captures WHERE capture_id='$OID'" | grep -oc "$pat")
  n=$(./bin/chromeport db query "SELECT raw_text FROM captures WHERE capture_id='$NID'" | grep -oc "$pat")
  printf "%-12s old=%s new=%s\n" "$pat" "$o" "$n"
done
```
