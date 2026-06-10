# Handoff prompt — fix dropped noon sessions, meals & airport transfers in the JS/TS dashboard Worker

Copy everything below the line into a fresh Claude Code session opened at the repo root
(`/home/yanggf/b/travel-2026`). This is a self-contained TS-Worker bug-fix, independent of the
larger Rust dashboard redesign.

---

You are fixing bugs in the Cloudflare trip-dashboard Worker (TypeScript) at
`/home/yanggf/b/travel-2026/workers/trip-dashboard/`. Read `CLAUDE.md` (repo root) first for the
dashboard architecture, the Turso pipeline, and the deploy commands.

## Context
The `okinawa-2026` plan is fully populated in Turso (flights, hotel, 5-day session-based
itinerary, meals, airport transfers). The live page
`https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026` renders most of it correctly but
silently drops several pieces. All data is confirmed present in the DB — the gaps are in the
Worker's assembly/render code.

## Bug 1 — `noon` session is dropped (primary)
The plan uses **four** session types: `morning`, `noon`, `afternoon`, `evening` (tables
`timesofday`, `activities`, `session_meals`). The session **label map** in `src/render.ts`
(~lines 29–32) knows all four (`noon` → 中午). **But** the `SessionData` interface and its
initializer (`src/render.ts` ~lines 178–193) define only `morning`/`afternoon`/`evening` — **no
`noon` slot**. So any activity or meal binned to `noon` has nowhere to go and is dropped before
render. On the live page the 中午 blocks are empty and lunches (e.g. "Lunch: Makishi Public
Market") never appear.

Fix: add `noon` to the `SessionData` interface, the initializer object, and **every** place the
session set is hardcoded as three (grep `morning`/`afternoon`/`evening` across `src/render.ts`).
Keep order: morning → noon → afternoon → evening, matching the label map.

## Bug 2 — airport transfers render "—"
On the same page, 機場→飯店 and 飯店→機場 show "已規劃 / —" despite `airport_transfers` having full
`selected_title` / `selected_route` / `selected_duration_min` / `selected_price_yen` for
`okinawa-2026`. `src/turso.ts` already queries the table (query 10). Trace `src/render.ts`
~line 363 (`selected` object built from `tr.selected_title || tr.selected_id`) and find why
route/time/price aren't displayed. Determine if it's the same root cause as Bug 1 or a separate
field/keying mismatch, and fix it.

## Bug 3 — double-escaped ampersand (cosmetic)
Activity titles show a double-escaped ampersand (e.g. "Museum & Art", "Naha & Craft" rendering
with a literal `&amp;`). Find the over-escaping (likely `esc()` applied to already-escaped text)
and fix.

## Constraints
- Keep all trip content in Turso — do NOT hardcode okinawa content in Worker code.
- The Worker supports BOTH session-based (Tokyo, Okinawa) and schedule-based (Kyoto) formats —
  do not break either.
- Code change ⇒ redeploy required.

## Verify before claiming done
1. `cd workers/trip-dashboard && unset CLOUDFLARE_API_TOKEN && npx wrangler dev` → open
   `http://localhost:8787/?plan=okinawa-2026`; confirm Day 3 中午 shows the Makishi lunch, noon
   activities render, and both transfers show the Yui Rail route/time/price (not "—").
2. Spot-check `?plan=tokyo-2026` and `?plan=kyoto-2026` — no regression.
3. Deploy: `unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`.
4. `curl "https://trip-dashboard.yanggf.workers.dev/?plan=okinawa-2026"`, strip tags, confirm
   noon content + meals + transfers in the live HTML.

Report the exact lines changed, the root cause of each of the 3 issues, whether they shared a
cause, and the verification output. Solo repo — commit directly to `master`, no PR, end the commit
message with:
`Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`
