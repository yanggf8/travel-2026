# CLI bug — set-activity-title leaves poi_id stale (title↔poi_id inconsistency)

**Found:** 2026-06-12, while reconciling Okinawa Day-3 map data. **Data fixed; CLI guard pending.**

## The bug
`set-activity-title` (`rust/crates/travel-cli/src/set_activity.rs`, `run_title`, the UPDATE at ~line 207) does:
```sql
UPDATE activities SET title = ?1, updated_at = ?2 WHERE plan_id=? AND destination=? AND day_number=? AND session_type=? AND id=?
```
It changes the **title** but never touches **`poi_id`**. So when an activity is re-themed by editing its title (e.g. Day 3 changed from Shikinaen/Makishi to "iias 沖縄豊崎 / DMM 水族館"; Day 2 afternoon changed from the Prefectural Museum to "新都心購物"), the OLD `poi_id` stays linked — now pointing at a DIFFERENT place than the title says. The dashboard then attaches the wrong POI's coordinates/price/pin to an activity whose text describes somewhere else. Silently wrong (no error, no warning).

## Impact found (okinawa-2026)
- Day 3 morning: title "DMM かりゆし水族館" but poi_id=`shikinaen` (Shikinaen Garden) — WRONG
- Day 3 noon: title "iias 沖縄豊崎" but poi_id=`makishi_market` — WRONG
- Day 2 afternoon: title "新都心購物（サンエー…DFS）" but poi_id=`pref_museum` — WRONG

**Data already fixed** (2026-06-12): the 3 stale links were set to NULL via `db exec` (a wrong link is worse than no link — with NULL the renderer falls back to title-based handling, consistent with the displayed text). The 5 remaining poi_id links were verified to genuinely match their titles.

## The fix (write-time guard — agent-first, fail-loud)
When `set-activity-title` changes a title and the row currently HAS a `poi_id`, the link is now unverified. Options (pick fail-loud + agent-first):
1. **Clear poi_id on title change** (safest default): `SET title=?, poi_id=NULL, updated_at=?` — and PRINT a plain-text note: `note: cleared poi_id (was '<old>') — title changed; re-link with set-activity-poi if the place is unchanged`. The agent sees it and can re-link if appropriate.
2. **Refuse + require explicit intent:** reject the title change if poi_id is set, telling the agent to either pass `--keep-poi` (title is a cosmetic reword of the same place) or `--clear-poi`. More agent-friction; only if option 1 loses too much.

Recommended: **option 1** (clear + notify) — it makes the inconsistency structurally impossible while staying low-friction.

## Related
- This is the same shift-left/fail-proof class as `docs/lint-shift-left-audit.md`. Add a guard alongside the audit's bundle. Belongs in `set_activity.rs::run_title`.
- A backstop LINT could also flag stored rows where `poi_id` is set but the POI's title shares no token with the activity title (defense-in-depth for the `db exec` path), but the write-time clear is the primary fix.
- NOTE: `set_activity.rs` is being edited by the shift-left-write-guards workflow concurrently — apply this AFTER that lands to avoid conflict.
