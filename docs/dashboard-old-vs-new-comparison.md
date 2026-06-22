# Dashboard comparison — old TS worker vs. new Rust worker

**Updated:** 2026-06-22 (FINAL PARITY EXAM — source-verified). Prior revision (2026-06-11)
is **SUPERSEDED**: it reported "meals=0" and "route segments missing" on the Rust worker.
Both were **subsequently fixed** and are now confirmed present in the code (see below). This
revision is a *code* audit (TS `render.ts`/`turso.ts` vs RS `src/`), independently
corroborated and cross-checked by a Codex review.

## URLs
- OLD (TS):  `https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026`  (no token)
- NEW (Rust): `https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=<owner>`

## What changed since 2026-06-11
The two gaps that revision flagged are **closed**:
- **Meals** — fixed in `fb2dcd1` ("render meal map-pins as clickable links"). `session_meals`
  is queried (`router.rs:191`) and rendered via `render_meal()` (`render/session.rs:21`),
  with the `<label>｜map:<query>` pin convention and the 備案 over-capture bug fixed. Tested.
- **Route segments** — `day_route_segments` queried (`router.rs:212`) and rendered as the
  "今日路線 / Today's route" block (`render/day.rs:111`, `render_route_block`). Tested.

## Feature parity (verified against current source, 2026-06-22)

### ✅ At parity (present in both)
Plan view + ZH-default + `?lang=en`; plan index (owner-only); day cards + day-type accents;
weather strip incl. **feels-like** + clothing tips; 4 fixed sessions; `focus_zh` /
`transit_notes_zh`; activity-text rendering (escape, `\n`→`<br>`, labeled map links, bare-URL
linkify); **meals** + meal map-pins; **route segments** ("今日路線"); POI stop list + Maps
links + ¥ cost badges; booking summary (flights, hotel incl. `name_zh` + grouped notes,
transfers with route+price); anti-translate meta + `lang="zh-TW"`; hotel Maps links.

### ✅ RS-only (richer than TS — keep)
- Keyless 3-level maps (plan + per-day PNGs) via R2 `MAPS` + `/map/*`.
- Voucher PDFs via R2 `VOUCHERS` + auth-gated `/voucher/*` with token threading.
- Token-scoped auth: `OWNER_TOKEN` + per-plan `plan_share_tokens` (`?token=`),
  constant-time compare (`auth.rs`). TS had only `ADMIN_TOKEN` (edit mode).
- Progressive disclosure (`<details>`); lighter HTML (~31KB vs ~60KB); soft-deleted plans
  hidden end-to-end.

### ❌ Absent in RS (TS has) — disposition
| # | Feature | TS source | Disposition |
|---|---|---|---|
| 1 | Edit mode (`?edit=`, pencils, `POST /api/edit`, `ADMIN_TOKEN`) | index.ts, render.ts | **Intentional** — RS is read-only; edits go via `./bin/travel` CLI (audit triad). |
| 2 | Per-activity ZH titles (`session_activities_zh`) | render.ts | **Intentional / documented limitation** — activity titles render in stored language for both langs; gloss inline. |
| 3 | Pending-booking / reservation-deadline alerts (`renderPendingAlerts`) | render.ts:1248 | **CLOSING before cutover** (in progress). |
| 4 | Transit cheat-sheet (`renderTransitSummary`) | render.ts:1286 | **CLOSING before cutover** (in progress). |
| 5 | Clickable flight-number search link (`flightLink`) | render.ts:979 | **CLOSING before cutover** (in progress). |
| 6 | Schedule-based itinerary format (`convertScheduleToSessions`) | render.ts | **Deferred** — dormant (active `okinawa-2026` is session-based; Kyoto archived). |
| 7 | Raw JSON API `/api/plan/<id>`, favicon, service-worker/offline | index.ts | **Deferred** — minor; no known consumer. |

## In-flight work (closing #3/#4/#5)
Implementation notes + the source-verified, Codex-corroborated correction list live in the
plan file `~/.claude/plans/ok-do-the-final-compiled-fiddle.md`. Key gotchas the port must
respect:
- #3 pending "meal" alerts come from **pending activities whose title matches a meal regex**,
  NOT from `session_meals` (render.ts:1253-1262).
- RS `Session.activities` is `Vec<String>` and `build_sessions` keeps only `title`
  (`model.rs:19-26,61-73`) — must become a struct carrying `booking_status`/`book_by`/
  `booking_url` before alerts can work.
- #4 must bump the hard-coded 10-statement pipeline to 12 (`router.rs:216`, `model.rs:90-95`)
  and add `transitCheat`/`dailyTransit`/`homeBase` i18n keys.

## Net assessment
The Rust worker is **at or above content parity** on everything except 3 deliberate-defer
items (#1/#2/#6/#7) and 3 in-flight closures (#3/#4/#5). Once #3–#5 land, the Rust worker
**meets or exceeds** the TS worker on every user-facing feature and is clear to reclaim the
primary URL.

## Rerun (rendered-text capture — the correct method, NOT screenshots)
```bash
OWNER=<owner-token>
OID=$(./bin/chromeport fetch url "https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026" --source dash | sed -n 's/^capture_id\t//p')
NID=$(./bin/chromeport fetch url "https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=$OWNER" --source dashrs | sed -n 's/^capture_id\t//p')
for pat in 中午 🍜 今日路線 需訂位 識名園 ¥340 預約期限; do
  o=$(./bin/chromeport db query "SELECT raw_text FROM captures WHERE capture_id='$OID'" | grep -oc "$pat")
  n=$(./bin/chromeport db query "SELECT raw_text FROM captures WHERE capture_id='$NID'" | grep -oc "$pat")
  printf "%-12s old=%s new=%s\n" "$pat" "$o" "$n"
done
```
