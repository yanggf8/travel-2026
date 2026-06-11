# Dashboard comparison — old TS worker vs. new Rust worker

**Captured:** 2026-06-11 via `chromeport screenshot` (full-page, 480px-wide mobile viewport),
plan `okinawa-2026`, both sites live.

> **Caveat:** the OLD TS site was mid-fix when this was captured. Some gaps below (empty noon,
> missing meals) are exactly the bugs tracked in `docs/handoff-worker-noon-meals-transfers.md`;
> if those land, re-run the comparison (rerun steps at the bottom) for an apples-to-apples view.
> This doc is the **baseline** to measure old-site fixes against.

## URLs compared
- OLD (TS):  `https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026`  (no token required)
- NEW (Rust): `https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=<owner>`  (token-gated)

Screenshots: `/tmp/compare/old-ts.png` (465×7976) · `/tmp/compare/new-rs.png` (465×5707).

## Feature / content diff (grep of the live HTML)

| Feature | OLD (TS) | NEW (Rust) | Note |
|---|---|---|---|
| Noon (中午) **label** | ✅ | ✅ | both render the heading |
| Noon (中午) **content** | ❌ (empty) | ✅ | old has the label-map but no data slot → noon activities/meals fall through (the original bug) |
| 🍽 Meals | ❌ | ✅ | old drops all meals (incl. the Makishi lunch) |
| Map images (`/map/...png`) | ❌ | ✅ | old has no maps; new has plan + 5 per-day keyless OSM maps from R2 |
| Per-stop Google Maps links (`maps?q=lat,lon`) | ❌ | ✅ | new: tap a stop → exact place in Google Maps |
| Progressive disclosure (`<details>`) | ❌ | ✅ | new collapses PNR/CFM/booking minutiae behind a tap; old dumps inline |
| Transfer route + ¥340 price | ✅ | ✅ | parity |
| Flight CI120 / hotel AZAT / weather strip | ✅ | ✅ | parity |

## Page weight / density

| | OLD (TS) | NEW (Rust) |
|---|---|---|
| HTML bytes | 60,138 | 31,174 (~½) |
| Full-page height | 7,976 px | 5,707 px (~−30%) |

## Visual verdict
- **OLD:** wall-of-text, no maps, sparse booking block, weak hierarchy. Information-dense but hard to scan; the empty noon blocks read as "missing data."
- **NEW:** card-based day timeline, a map per day + a trip-wide map, weather strips with rain-gear tips, day-type colour accents, collapsed booking details. A genuine redesign, not a reskin — and lighter on the wire.

## What the NEW worker adds beyond bug-fixes
Even after the old site's noon/meals bugs are fixed, these remain new-worker-only advantages:
1. **Maps** (plan + per-day, keyless, chromeport-snapshotted → R2).
2. **Progressive disclosure** of booking minutiae.
3. **~½ the page weight**, ~30% shorter.
4. **Token-scoped access** (owner sees index + all plans; share links scope to one plan).
5. **Rust/WASM** stack (the migration goal); soft-deleted plans hidden end-to-end.

## Rerun (after old-site fixes land)
```bash
OWNER=<owner-token>
mkdir -p /tmp/compare
./bin/chromeport screenshot "https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026" \
  --out /tmp/compare/old-ts.png --width 480 --height 1400 --wait 6000 --full-page
./bin/chromeport screenshot "https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=$OWNER" \
  --out /tmp/compare/new-rs.png --width 480 --height 1400 --wait 6000 --full-page
# then diff the live HTML for the feature table above:
curl -s "https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026" > /tmp/compare/old.html
curl -s "https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=$OWNER" > /tmp/compare/new.html
for p in 中午 🍽 Asato 340 /map/okinawa "maps?q=26" "<details"; do
  printf "%-20s old=%s new=%s\n" "$p" "$(grep -c "$p" /tmp/compare/old.html)" "$(grep -c "$p" /tmp/compare/new.html)"
done
```
