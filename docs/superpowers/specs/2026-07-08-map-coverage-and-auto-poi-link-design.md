# Map-coverage guardrail + `set-activity-poi --auto` — Design

**Date:** 2026-07-08
**Status:** Approved (design); Codex-reviewed + Claude-corroborated against source.
**Motivating bug:** an activity created via `set-activity add` for a real place gets `poi_id=NULL`
(`run_add` in `set_activity.rs:1429-1517` calls `itinerary::insert_activity` with no `poi_id` and no
title→`destination_pois` matching). The dashboard route map for a day only plots activities whose
`poi_id` joins to a geocoded `destination_pois` row, so a day whose activities are all `poi_id=NULL`
renders an empty map (`地圖(尚未產生)`) — and NOTHING in the flow warned. `validate publish` passed
0 blockers / 0 warnings because its map check (`has_map_path`) is plan-wide all-or-nothing, not per-day.
Found + fixed by hand on `osaka-kyoto-redrill-2026` (4 manual `set-activity-poi` calls + `snapshot-maps`).

## Goal

Two complementary CLI changes so the flow **detects** the empty-map-day gap automatically and lets the
agent **fix** it in one command instead of N manual links:

- **A (detect):** a per-day map-coverage **WARN** in `validate publish`.
- **B (fix):** a `set-activity-poi --auto [--dest]` batch mode that links `poi_id=NULL` activities to
  their `destination_pois` match, deterministically, never guessing.

## Global constraints (verbatim project rules; every task inherits these)

- **Agent-first plain text only** — CLI stdout is plain text / table lines, NEVER JSON.
- **Fail loud, no local fallback** — read source of truth from Turso; a missing row THROWS, never
  falls back to a local file.
- **No fabricated coords** — never invent lat/lon; only link POIs that are already geocoded.
- **Audit triad on every mutation** — `plan_events` (+ `plan_event_data` KV) + `operation_runs` +
  `plans.version` bump, via `cascade::common::record_operation`. Reference data / read-only checks do
  NOT write audit.
- **Reuse existing SQL/helpers** — Part A reuses the `has_map_path` POI-join predicate; Part B reuses
  `set_activity_poi`'s existing resolve/assert/audit machinery.
- **Tests** — real-Turso behavior-lock integration tests on the `tests/common/mod.rs` harness
  (`bin`, `db_exec`→`Option<Rows>`, `seed_plan(plan,dest,version)`, `teardown_plan`, RAII `Guard`);
  run serialized (`--test-threads=1`) in the BACKGROUND.
- **Pipeline** — Codex designs/plans + writes the test plan; Grok (or hand-from-spec) implements;
  Claude reviews line-by-line + corroborates vs source + verifies byte-behavior serialized live.

---

## Part A — per-day map-coverage WARN in `validate publish`

### Behavior

After the existing plan-wide `has_map_path` block passes (`validate.rs:1283-1291`), run a per-day
scan over the plan's active destination. Emit ONE **WARN** per day that **has activities but zero
mappable stops and zero route segments**:

```
⚠ [map-coverage] day 5 has no geocoded stops and no route segments — its dashboard
   map will render empty. Link a real POI: set-activity-poi 5 <session> <poi_id>
   (or set-activity-poi --auto).
```

- **Severity: WARN** — shows in `## Warnings`, counts toward the warning total, does NOT block
  publish (publish readiness = 0 blockers; warnings don't block). Rationale: a pure arrival/departure
  day can legitimately be map-less; blocking would pressure fabricated pins.
- **Edge case — a day with ZERO activities does NOT warn.** That is the content-depth thin-day INFO's
  job. Map-coverage WARN fires only on a day that HAS activities but none mappable.

### SQL (corroborated — Codex C1)

The naive "GROUP BY the `has_map_path` POI JOIN" is **wrong**: `has_map_path`'s predicate is an inner
`JOIN destination_pois` (`validate.rs:1552-1557`), so a day whose activities are all `poi_id=NULL`
produces zero JOIN rows and vanishes under a plain group — i.e. it silently misses exactly the bad
days. Drive the query from an `activity_days` CTE and LEFT JOIN the mappable + route counts:

```sql
WITH activity_days AS (
  SELECT day_number, COUNT(*) AS activity_count
  FROM activities
  WHERE plan_id = ?1 AND destination = ?2
  GROUP BY day_number
),
mappable_days AS (
  SELECT a.day_number, COUNT(*) AS mappable_count
  FROM activities a
  JOIN destination_pois p
    ON p.slug = a.destination AND p.poi_id = a.poi_id
  WHERE a.plan_id = ?1 AND a.destination = ?2
    AND p.lat IS NOT NULL AND p.lon IS NOT NULL
    AND TRIM(CAST(p.lat AS TEXT)) <> ''
    AND TRIM(CAST(p.lon AS TEXT)) <> ''
  GROUP BY a.day_number
),
route_days AS (
  SELECT day_number, COUNT(*) AS route_count
  FROM day_route_segments
  WHERE plan_id = ?1 AND destination = ?2
  GROUP BY day_number
)
SELECT ad.day_number
FROM activity_days ad
LEFT JOIN mappable_days md USING (day_number)
LEFT JOIN route_days  rd USING (day_number)
WHERE COALESCE(md.mappable_count, 0) = 0
  AND COALESCE(rd.route_count, 0) = 0
ORDER BY ad.day_number;
```

The `mappable_days` CTE is byte-identical to the `has_map_path` POI predicate (only `GROUP BY
a.day_number` added) — that is the "reuse existing SQL" requirement.

### Placement / shape

- New async helper `map_coverage_gaps(conn, plan_id, destination) -> Result<Vec<i64>, String>` in
  `validate.rs`, called from `run_publish` after the map-path block.
- For each returned day, push an `Issue { category: "map-coverage", severity: Warning, ... }`.
- Read-only: NO mutation, NO audit.

### Tests (Part A)

Behavior-lock integration test `tests/validate_publish_map_coverage.rs`:
1. seed plan + dest + a geocoded `destination_pois` row + 2 days;
   day 1 activity linked to the geocoded POI, day 2 activity with `poi_id=NULL`, no route segments.
2. run `validate publish`; assert stdout contains the day-2 map-coverage WARN and NOT a day-1 one.
3. NEGATIVE: a day 3 with ZERO activities → assert no map-coverage WARN for day 3.
4. NEGATIVE: give day 2 a `day_route_segments` row → assert the WARN disappears (route path counts).

---

## Part B — `set-activity-poi --auto [--dest]`

### Behavior

New mode on the existing `set-activity-poi` command. When `--auto` is present:

1. **Mutually exclusive with positionals.** `--auto` + any of `<day> <session> <poi_id>` → FAIL LOUD.
2. Scan ALL activities with `poi_id IS NULL` for the resolved `(plan, destination)`, in **stable
   order** `ORDER BY day_number, session_type, sort_order, id` (Codex C4 — deterministic, and two
   activities MAY legitimately share one poi_id: `activities.poi_id` is nullable with no UNIQUE,
   `destination_pois` PK is `(slug, poi_id)`; repeat visits are valid, do NOT reject duplicates).
3. For each, attempt a title→`destination_pois.title` match using the **deterministic, never-guess**
   rule below. Only consider POIs that are **geocoded** (lat/lon present) — linking an ungeocoded POI
   would not render a map either, so it is pointless; report it as `matched but POI ungeocoded`.
4. For each successful link: `UPDATE activities SET poi_id=? …` (require `rows_affected == 1`).
5. **ONE batched audit op for the whole run** (Codex C3 — matches `set-route-segments-bulk`,
   `set_route_segment.rs:628`): emit per-link `activity_poi_linked` events if desired, but call
   `record_operation` ONCE with summary `auto-linked N POI(s)` and one `plans.version` bump. If a
   libsql transaction wraps the multi-UPDATE cleanly, use it so a mid-run failure can't leave mutation
   ahead of audit; otherwise fail-loud-and-stop (partial links are valid + re-runnable — `--auto` is
   idempotent because already-linked rows are skipped on the next run).
6. **Zero NULL-poi activities** → print `nothing to link` and exit 0 (not an error).

### Match rule — "exact + strip-gloss unambiguous substring" (corroborated — Codex C2, real data)

Given an activity title and the set of GEOCODED `destination_pois.title` for the dest:

1. **Exact** case-insensitive equality (after stripping a trailing ZH gloss from the activity title)
   → link.
2. Else **substring, either direction** (case-insensitive, gloss-stripped) → link IF exactly ONE POI
   matches; if >1 → ambiguous (manual).
3. **(DROPPED)** the shared-leading-token rule from the first draft is removed. Codex flagged it as an
   unsafe guess (e.g. `Universal CityWalk` → `Universal Studios Japan`), and real data confirms
   exact+substring already covers the good cases.

Any activity yielding 0 matches OR >1 matches OR only an ungeocoded match is **NOT linked** and is
reported in the manual bucket. The known miss (`Shinsaibashi-suji Shopping` vs POI `Shinsaibashi
Shopping Arcade` — "-suji"/"Arcade" differ, no substring either way) correctly lands in manual with a
`closest POI: <id>?` hint — it must be a human/agent judgment call, not an auto-guess
(LLM-judge-when-brittle).

**ZH-gloss strip:** an activity title like `Dotonbori 道頓堀` carries an appended Traditional-Chinese
gloss. Strip a trailing run of CJK/kana characters (and separating whitespace) before matching. The
POI titles are ASCII/romaji, so the gloss never helps a match and must be removed first. (`Kuromon
Ichiba Market 黑門市場` → `Kuromon Ichiba Market`; `Osaka Castle 大阪城` → `Osaka Castle`.)

Real corroboration (osaka_kyoto_2026 — the 4 activities linked by hand during the bug fix):

| Activity title (authored)              | POI title                    | Rule step | Result |
|----------------------------------------|------------------------------|-----------|--------|
| `Dotonbori 道頓堀`                      | `Dotonbori Canal`            | 2 (substr)| link ✓ |
| `Kuromon Ichiba Market 黑門市場`         | `Kuromon Ichiba Market`      | 1 (exact) | link ✓ |
| `Osaka Castle 大阪城`                    | `Osaka Castle`               | 1 (exact) | link ✓ |
| `Shinsaibashi-suji Shopping 心齋橋筋商店街`| `Shinsaibashi Shopping Arcade`| none     | manual (closest: `shinsaibashi_shopping`?) |

So exact+substring auto-links 3/4; the 4th is honestly deferred to manual. That is the intended,
no-guess behavior.

### Output (plain text)

```
📍 set-activity-poi --auto (osaka_kyoto_2026)
✅ linked 3:
   D1 afternoon "Dotonbori 道頓堀"            → dotonbori
   D1 afternoon "Kuromon Ichiba Market 黑門市場" → kuromon_market
   D5 morning   "Osaka Castle 大阪城"          → osaka_castle
⚠ 2 unlinked (link manually with set-activity-poi <day> <session> <poi_id> --match "..."):
   D1 afternoon "Shinsaibashi-suji Shopping 心齋橋筋商店街" — no unambiguous POI (closest: shinsaibashi_shopping?)
   D1 morning   "Namba Parks 難波公園"          — no POI match
Version 41 → 42 (auto-linked 3 POI(s)).
```

### Reuse

- Keep the current single-link path (`<day> <session> <poi_id>`) exactly as-is; `--auto` is an
  additional branch in `run`/`parse_args` that dispatches to a new `execute_auto`.
- `execute_auto` reuses `poi_exists`/geocode-check, `itinerary::set_activity_poi` (the UPDATE), and
  `record_operation` — no new DAL write beyond a `list NULL-poi activities` read and a `list geocoded
  POIs` read.

### Tests (Part B)

Behavior-lock integration test `tests/set_activity_poi_auto.rs`:
1. seed plan + dest + geocoded POIs (`dotonbori`/`Dotonbori Canal`, `osaka_castle`/`Osaka Castle`) +
   an UNGEOCODED POI + activities: an exact-match, a substring+gloss match, an ambiguous case (2 POIs
   share the substring), a no-match, and one whose only match is the ungeocoded POI.
2. run `set-activity-poi --auto`; assert: the exact + substring ones get linked (`poi_id` set in DB),
   the ambiguous/no-match/ungeocoded ones stay `poi_id=NULL` and appear in the `⚠ unlinked` list.
3. assert EXACTLY ONE `operation_runs` row for the run + `plans.version` bumped by 1 (batched audit).
4. IDEMPOTENCE: run `--auto` again → `nothing to link` (already-linked rows skipped), no new op row
   beyond the no-op, version unchanged.
5. FAIL-LOUD: `--auto 1 morning foo` (mixing `--auto` with positionals) → non-zero exit.

---

## Non-goals (YAGNI)

- No content-depth "mapped-days N/M" axis (wrong layer — Part A is the right home; the oracle stays
  activities/meals/routes/ZH).
- No change to `set-activity add` (a post-add POI hint was considered as "Part C" and deferred).
- No louder `snapshot-maps`/`check-maps-fresh` skip aggregation (the signal already exists; deferred).
- No auto-geocoding of unmatched activities (no fabricated coords — hard rule).

## File map

- **A:** `rust/crates/travel-cli/src/validate.rs` — new `map_coverage_gaps` helper + call in
  `run_publish`; test `tests/validate_publish_map_coverage.rs`.
- **B:** `rust/crates/travel-cli/src/set_activity_poi.rs` — `--auto` parse branch + `execute_auto`;
  a `list_null_poi_activities` + `list_geocoded_pois` read (in `set_activity_poi.rs` or a
  `travel-db` repo read fn — implementer's call, must stay a READ, no new audit-writing repo fn);
  test `tests/set_activity_poi_auto.rs`.
- Docs: add both to `docs/reference/CLI.md`; update the Skill Decision Tree row for
  "derived leg / empty map" and `set-activity-poi`.
