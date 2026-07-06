# Drill-While-Comparing — Design

**Date:** 2026-07-06
**Status:** design approved (Yang, 2026-07-06); implementation pending
**Author:** Claude (design), to be plan-authored + corroborated by Codex per the multi-AI pipeline

## Problem

The enrichment-flow work (2026-07-06: content-depth signal + `derive-routes` cascade + Stage-3
rewire) fixed *why* a drill produced content-thin trips. But we have no repeatable way to prove the
enriched flow reaches AND EXCEEDS a known-good reference trip's richness — only per-day gap checks on
a single plan (`validate publish` content-depth WARN/INFO). A drill can pass every command and still
be quietly thinner than a real trip (kyoto-confirm-2026: 9 activities / 1 meal / 6 routes vs
okinawa-2026: 29 / 11 / 21).

**Goal:** give the drill loop a durable "is this plan richer than the reference?" oracle, and a
workflow that iterates until it is — with the **web-rendered dashboard page as the final acceptance
criterion**, not the numbers.

## Non-negotiable framing (Yang, 2026-07-06)

> 網頁渲染的結果是最終驗收標準。("The web-rendered result is the final acceptance criterion.")

The `compare content-depth` command is a **mid-loop oracle** to drive convergence. It is NOT the
final gate. The final gate is Yang looking at the deployed `-rs` dashboard page (drill vs okinawa,
side by side). Numbers passing while the page looks wrong = not done.

## The reference bar (measured 2026-07-06)

okinawa-2026 (`okinawa_2026`), a polished real trip, 5 days:

| axis | total | notes |
|------|------:|-------|
| activities | 29 | only 4/29 POI-linked, 5/29 timed — so POI-linkage & timing are NOT fair "better" axes |
| meals (noon+evening) | 11 | |
| routes | 21 | 21/21 have `duration_min > 0` (transit metadata) |
| ZH: days.theme_zh | 5/5 | |
| ZH: timesofday.focus_zh | 17/20 | |

## Part A — `compare content-depth` (the oracle)

New read-only command in the existing `compare` family.

```
travel compare content-depth --plan-id <drill> [--against <ref>]   # --against default: okinawa-2026
```

- Dispatch: `main.rs` arm `[group, sub, rest @ ..] if group == "compare" && sub == "content-depth"`
  (alongside `compare trips|dates|true-cost` at `main.rs:115-133`).
- Module: `rust/crates/travel-cli/src/compare_content_depth.rs`.
- Read-only: `db::connect_read`, no `plan_events` / `operation_runs` / `plans.version` (no audit —
  it mutates nothing).
- Plan/destination resolution: reuse the same `plan_id → destination` convention as other view
  commands (hyphen plan_id → underscore destination). Reference defaults to `okinawa-2026` /
  `okinawa_2026`.

### Axes computed for BOTH plans

Reuse the content-depth CTE shape from `validate.rs:1671-1680` (`day_rows` / `activity_counts` /
`meal_counts` / `route_counts`), extended:

| axis | source | quality gate (what counts) |
|------|--------|----------------------------|
| activities | `activities` COUNT per day | counted as-is |
| meals | `session_meals` where `session_type IN ('noon','evening')` and `TRIM(meal) <> ''` | label non-empty (real meal, not a blank row); `meal` is the column (`NOT NULL`, so blank = `''`) |
| routes | `day_route_segments` COUNT | **`duration_min IS NOT NULL AND duration_min > 0`** (has transit metadata) — metadata-less routes do NOT count |
| ZH coverage | **weighted** ratio: `(days.theme_zh non-empty + timesofday.focus_zh non-empty) / (days total + timesofday total)` — every ZH slot counts equally | ratio compared, not raw count |

**ZH formula — weighted, decided (2026-07-06).** Use `(day_zh + sess_zh) / (day_all + sess_all)`, NOT
the average of the two per-table ratios. Weighted is more honest: it counts each ZH slot equally, so a
plan cannot mask thin session-ZH (many slots) behind a perfect handful of day-themes. Rendered as
integer percent (floor). For okinawa (day 5/5, session 17/20) this is `22/25 = 88%` — the reference
bar. (Avg-of-ratios would report a misleading 92% by over-weighting the 5 day-themes 4× vs the 20
sessions.)

The quality gate is the anti-padding mechanism: you cannot win by dumping empty routes or blank meal
rows. This is what makes the verdict "genuinely better," not "bigger numbers."

### Output (plain text — agent-first, NO JSON)

```
CONTENT DEPTH — kyoto-confirm-2026  vs  okinawa-2026 (reference)

per-day:
  day  type        DRILL(a/m/r)   REF(a/m/r)
  1    arrival     7/2/5          7/2/5
  2    full        8/2/6          6/2/7
  ...

totals:
                        DRILL   REF    Δ      verdict
  activities             31     29    +2     >= ok
  meals (real)           13     11    +2     >= ok
  routes (w/ metadata)   22     21    +1     >= ok
  ZH coverage            100%   88%   +12pp  >= ok
  ----------------------------------------------------
  VERDICT: BETTER — all axes >= reference, 4 strictly greater, quality gate PASS
```

### Verdict logic

Let each axis have `drill` and `ref` values (routes/meals already quality-gated; ZH as a ratio).

- **SHORT: `<axis list>`** if ANY axis `drill < ref`. The axis list is the worklist for the loop.
- **ALIGNED** if every axis `drill >= ref` but NONE strictly `>` (exact tie everywhere).
- **BETTER** if every axis `drill >= ref` AND at least one axis `drill > ref` AND the quality gate
  holds (routes counted only with metadata, meals only real, ZH ratio `>=` ref ratio).

`SHORT` takes precedence over `ALIGNED`/`BETTER` (any deficit → SHORT). Exit code 0 in all cases
(read-only diagnostic, never a hard failure) — the verdict is in the text, agents parse the text.

## Part B — the loop-until-BETTER drill workflow (documented, no new code)

Extends the Stage-3 flow. The compare `SHORT:` line is the per-iteration worklist:

```
scaffold → populate → derive-routes
  → compare content-depth --plan-id <drill>          # baseline, likely SHORT: meals(day 2,4,5)
  → agent enriches ONLY the SHORT axes (real meals on the named days, --recommended)
  → derive-routes --day N        (if routes went SHORT after activity edits)
  → compare content-depth        # re-check
  → repeat until verdict = BETTER            ← loop-until-BETTER, not loop-until-dry
→ validate publish --plan-id <drill>         # 0 blockers (same gate a real trip clears)
→ [Part C: final-product review]
```

Where this lives: a new "loop-until-BETTER against a reference" callout in
`src/skills/stage3-expand-itinerary/SKILL.md` (the compare command becomes the objective test that
the enrichment step converged), plus the drill-as-diagnostic note.

## Part C — final-product review (the real gate)

After `BETTER` + `validate publish` clean:

1. **Yang deploys** (only step Claude cannot run — needs Cloudflare login):
   `cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`
2. Claude mints share links: `./bin/travel share-token` → hands Yang
   `?plan=<drill>&token=<tok>` AND `?plan=okinawa-2026&token=<tok>`.
3. **Yang reviews the two rendered pages side by side.** This is the final acceptance decision — the
   numbers only got us here. Drill meals render with the `🤖 AI-recommended` badge (correct: labeled,
   real-but-unconfirmed synthetic content); okinawa is confirmed.
4. After review: `./bin/travel share-token deactivate <tok>` — drill is synthetic 🧪 data, not left
   live-linkable.

Constraints surfaced (no surprises):
- The drill is synthetic: activities from real kyoto_2026 reference POIs (real places), meals
  agent-authored `--recommended`. Not a bookable trip; token deactivated after review.
- Deploy is Yang-gated. Claude stops at the deploy line, hands the command, resumes on confirmation.

## The fresh drill plan

Re-run the enriched flow on a FRESH plan (not the patched-up kyoto-confirm-2026), so the comparison
is honest end-to-end. Candidate: reuse an existing 🧪 drill destination or a new one; decided at
execution time. The point is: scaffold-from-empty → the full flow → compare, not "top up an old
plan until the numbers pass."

## Testing

`rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs` — real-Turso, `common::`
harness (`bin`, `db_exec`, `seed_plan`, `teardown_plan`, `Guard` RAII, run serialized in background):

- Seed a rich reference plan + a thin drill plan → assert `SHORT: meals`.
- Top the drill above the ref on every axis → assert `BETTER`.
- Make the drill exactly tie the ref → assert `ALIGNED`.
- Seed the drill with metadata-less routes above the ref count → assert routes do NOT count (quality
  gate), verdict stays `SHORT`/`ALIGNED` not `BETTER` (anti-padding proof).
- Assert the table math (per-day + totals) matches the seeded rows.

## YAGNI — deliberately excluded

- **POI-linkage & timing axes** — the reference itself is thin there (4/29, 5/29); they'd fail every
  plan, not a fair "better" bar.
- **Weighted composite score** — hides which axis is short; per-axis is more diagnostic and gives the
  loop a precise worklist.
- **No dashboard/web surface for the compare** — it's an agent/terminal oracle. The web is where the
  *plans* are reviewed (Part C), not where the *comparison* is rendered.
- **No mutation / no audit triad** — read-only diagnostic.

## Pipeline

Design (this doc) by Claude, corroborated against source (main.rs dispatch, validate.rs CTE,
okinawa live measurements). Implementation plan authored + corroborated by Codex, then multi-AI
impl (Grok implements, Claude reviews line-by-line + verifies serialized against live Turso).
