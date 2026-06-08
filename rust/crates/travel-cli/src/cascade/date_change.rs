// `travel set-dates` cascade write — date-anchor change trigger.
//
// Implements the write side of `process_1_date_anchor_change` for the
// `set-dates` command. This module is the Phase 4 port of the TS
// StateManager.setDateAnchor() + saveWithTracking('set-dates', ...) path,
// scoped to the date-change cascade only.
//
// Read the plan's spec — what this module must write and must NOT write
// is a strict list. The acceptance gate is byte-for-byte row parity
// against `npm run travel -- set-dates ...` for the disposable
// `test-set-dates-2026` plan (modulo volatile fields listed below).
//
// Writes (on the ACTIVE destination only):
//   1. date_anchors              — UPDATE start_date / end_date / days
//   2. cascade_dirty_flags       — flip dirty=1 for the 4 date-dependent
//                                  processes: process_3_4_packages,
//                                  process_3_transportation,
//                                  process_4_accommodation,
//                                  process_5_daily_itinerary
//   3. plan_events               — INSERT new event rows (date_anchor_changed
//                                  + 4 marked_dirty on dest_process; 1
//                                  date_anchor_changed + 1
//                                  marked_global_dirty + 4 marked_dirty
//                                  on timeline) — see assign_sort_order()
//                                  for the bucket ordering rule
//   4. plan_event_data           — INSERT KV child rows for each new event
//   5. operation_runs            — INSERT 1 row, command_type='set-dates'
//   6. plans.version             — +1 (atomic bump inside this function)
//
// Does NOT write:
//   - process_statuses         (cascade only marks dirty, does NOT reset
//                               status — P3/P4/P5 keep booked/researched)
//   - plan_root_date_anchor    (set-dates does not touch P1 root anchor)
//
// Volatile fields (ignored during diff):
//   - updated_at on every row
//   - operation_runs.run_id / started_at / completed_at
//   - plan_events.event_at and the event data payload
//
// Sort-order assignment (mirrors the TS in-memory append behavior):
//   - dest_process events:  per-(destination, process_id) bucket, assign
//     max(existing sort_order) + 1, +2, ... in emission order.
//   - timeline events:  GLOBAL timeline array index — assign
//     max(existing sort_order over ALL timeline rows) + 1, +2, ... in
//     emission order. TS does the same because timeline events share one
//     in-memory array indexed by append position.

use libsql::Connection;

/// The 4 date-dependent processes that get cascade-marked dirty on
/// `set-dates`. Mirrors state-manager.ts setDateAnchor() lines 620-625.
///
/// `process_3_*` is expanded via plan_schema_contract_nodes in the TS
/// cascade runner (wildcard.ts). The two concrete process_ids for our
/// test plans (and the live data) are:
///   - process_3_transportation
///   - process_3_4_packages
const DATE_DEPENDENT_DIRTY_TARGETS: &[&str] = &[
    "process_3_4_packages",
    "process_3_transportation",
    "process_4_accommodation",
    "process_5_daily_itinerary",
];

/// Execute the date-anchor change write for `set-dates`.
///
/// `old_start` / `old_end` are the previous values read from
/// `date_anchors` before the UPDATE. They populate the
/// `from_dates` field in the event payload. `None` if there was no
/// prior anchor.
///
/// Returns the new `plans.version` (always `version_before + 1`).
#[allow(clippy::too_many_arguments)]
pub async fn execute(
    conn: &Connection,
    plan_id: &str,
    active_dest: &str,
    start: &str,
    end: &str,
    days: i64,
    reason: Option<&str>,
    old_start: Option<&str>,
    old_end: Option<&str>,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    // 1. Read current plans.version (we need it for operation_runs + the
    //    bump). Fail loud if the plan row is missing.
    let version_before = read_version(conn, plan_id).await.map_err(|e| e.to_string())?;
    let version_after = version_before + 1;

    // 2. UPDATE date_anchors (PK = plan_id + destination; UPDATE is safe,
    //    no ghost rows).
    conn.execute(
        "UPDATE date_anchors \
         SET start_date = ?1, end_date = ?2, days = ?3, updated_at = ?4 \
         WHERE plan_id = ?5 AND destination = ?6",
        libsql::params![
            start.to_string(),
            end.to_string(),
            days,
            now_db.clone(),
            plan_id.to_string(),
            active_dest.to_string()
        ],
    )
    .await.map_err(|e| e.to_string())?;

    // 3. Flip dirty=1 for the 4 date-dependent processes on the active
    //    destination. The TS cascade does NOT touch process_statuses.
    for proc in DATE_DEPENDENT_DIRTY_TARGETS {
        conn.execute(
            "UPDATE cascade_dirty_flags \
             SET dirty = 1, last_changed = ?1 \
             WHERE plan_id = ?2 AND destination = ?3 AND process_id = ?4",
            libsql::params![
                now_iso.clone(),
                plan_id.to_string(),
                active_dest.to_string(),
                proc.to_string()
            ],
        )
        .await.map_err(|e| e.to_string())?;
    }

    // 4. Compute sort_order for new events. See module docstring for the
    //    bucket rule.
    let dest_process_p1_so = next_dest_process_sort_order(
        conn, plan_id, active_dest, "process_1_date_anchor",
    )
    .await.map_err(|e| e.to_string())?;
    let dest_process_p34_so = next_dest_process_sort_order(
        conn, plan_id, active_dest, "process_3_4_packages",
    )
    .await.map_err(|e| e.to_string())?;
    let dest_process_p3t_so = next_dest_process_sort_order(
        conn, plan_id, active_dest, "process_3_transportation",
    )
    .await.map_err(|e| e.to_string())?;
    let dest_process_p4_so = next_dest_process_sort_order(
        conn, plan_id, active_dest, "process_4_accommodation",
    )
    .await.map_err(|e| e.to_string())?;
    let dest_process_p5_so = next_dest_process_sort_order(
        conn, plan_id, active_dest, "process_5_daily_itinerary",
    )
    .await.map_err(|e| e.to_string())?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await.map_err(|e| e.to_string())?;

    let reason_value = reason.unwrap_or("User updated dates").to_string();
    let from_dates = match (old_start, old_end) {
        (Some(s), Some(e)) => format!("{s} to {e}"),
        _ => "null".to_string(),
    };
    let to_dates = format!("{start} to {end}");

    // 5. INSERT new plan_events rows. The TS path emits them in a
    //    specific order; the sort_order bucket is per-group.
    //
    //    dest_process group (one bucket per (dest, process_id)):
    //      - process_1_date_anchor   -> date_anchor_changed (1)
    //      - process_3_4_packages    -> marked_dirty           (4 dirty events)
    //      - process_3_transportation
    //      - process_4_accommodation
    //      - process_5_daily_itinerary
    //
    //    timeline group (GLOBAL bucket, one shared index):
    //      - process_1_date_anchor   -> date_anchor_changed
    //      - process_1_date_anchor   -> marked_global_dirty
    //      - process_3_transportation -> marked_dirty
    //      - process_3_4_packages    -> marked_dirty
    //      - process_4_accommodation -> marked_dirty
    //      - process_5_daily_itinerary -> marked_dirty

    // --- dest_process: process_1_date_anchor date_anchor_changed ---
    insert_event(
        conn,
        plan_id,
        "dest_process",
        active_dest,
        "process_1_date_anchor",
        dest_process_p1_so,
        "date_anchor_changed",
        &now_iso,
    )
    .await.map_err(|e| e.to_string())?;
    insert_kv_rows(
        conn,
        plan_id,
        "dest_process",
        active_dest,
        "process_1_date_anchor",
        dest_process_p1_so,
        &[
            ("days", days.to_string()),
            ("end", end.to_string()),
            ("from_dates", from_dates.clone()),
            ("reason", reason_value.clone()),
            ("start", start.to_string()),
            ("to_dates", to_dates.clone()),
        ],
    )
    .await.map_err(|e| e.to_string())?;

    // --- dest_process: marked_dirty × 4 (in TS emission order:
    //     P3 transport, P3+4 packages, P4 accommodation, P5 itinerary) ---
    // (TS emits them in the same order as DATE_DEPENDENT_DIRTY_TARGETS:
    //  process_3_4_packages, process_3_transportation, process_4_accommodation,
    //  process_5_daily_itinerary. That is the same loop order.)
    let dest_dirty_targets: &[(&str, i64)] = &[
        ("process_3_4_packages", dest_process_p34_so),
        ("process_3_transportation", dest_process_p3t_so),
        ("process_4_accommodation", dest_process_p4_so),
        ("process_5_daily_itinerary", dest_process_p5_so),
    ];
    for (proc, so) in dest_dirty_targets {
        insert_event(
            conn,
            plan_id,
            "dest_process",
            active_dest,
            proc,
            *so,
            "marked_dirty",
            &now_iso,
        )
        .await.map_err(|e| e.to_string())?;
        insert_kv_rows(
            conn,
            plan_id,
            "dest_process",
            active_dest,
            proc,
            *so,
            &[("dirty", "true".to_string())],
        )
        .await.map_err(|e| e.to_string())?;
    }

    // --- timeline: 6 events appended in this order ---
    let timeline_events: &[(&str, &str)] = &[
        ("process_1_date_anchor", "date_anchor_changed"),
        ("process_1_date_anchor", "marked_global_dirty"),
        ("process_3_transportation", "marked_dirty"),
        ("process_3_4_packages", "marked_dirty"),
        ("process_4_accommodation", "marked_dirty"),
        ("process_5_daily_itinerary", "marked_dirty"),
    ];
    for (i, (proc, evt)) in timeline_events.iter().enumerate() {
        let so = timeline_base + i as i64;
        insert_event(
            conn,
            plan_id,
            "timeline",
            "",
            proc,
            so,
            evt,
            &now_iso,
        )
        .await.map_err(|e| e.to_string())?;
        let kv: &[(&str, String)] = match *evt {
            "date_anchor_changed" => &[
                ("days", days.to_string()),
                ("end", end.to_string()),
                ("from_dates", from_dates.clone()),
                ("reason", reason_value.clone()),
                ("start", start.to_string()),
                ("to_dates", to_dates.clone()),
            ],
            "marked_dirty" | "marked_global_dirty" => &[
                ("dirty", "true".to_string()),
            ],
            _ => &[],
        };
        insert_kv_rows(
            conn,
            plan_id,
            "timeline",
            "",
            proc,
            so,
            kv,
        )
        .await.map_err(|e| e.to_string())?;
    }

    // 6. INSERT operation_runs (one row, append-only).
    let run_id = new_run_id();
    let summary = format!("{start} {end}");
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, 'set-dates', ?3, 'completed', ?4, ?5, ?6, ?6)",
        libsql::params![
            run_id,
            plan_id.to_string(),
            summary,
            version_before,
            version_after,
            now_db.clone()
        ],
    )
    .await.map_err(|e| e.to_string())?;

    // 7. Bump plans.version (+1, atomic).
    conn.execute(
        "UPDATE plans SET version = ?1, updated_at = ?2 WHERE plan_id = ?3",
        libsql::params![version_after, now_db, plan_id.to_string()],
    )
    .await.map_err(|e| e.to_string())?;

    Ok(version_after)
}

/// Read the current `plans.version` for a plan. THROWS if the plan row
/// is missing — there is no local-data fallback.
async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT version FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await.map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let v: i64 = row.get(0).map_err(|e| e.to_string())?;
        return Ok(v);
    }
    Err(format!("plans row missing for plan_id={plan_id}"))
}

/// Compute the next sort_order for a new event in a (scope='dest_process',
/// destination, process_id) bucket. Returns `MAX(existing) + 1`.
async fn next_dest_process_sort_order(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'dest_process' \
               AND destination = ?2 AND process_id = ?3",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                process_id.to_string()
            ],
        )
        .await.map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let m: i64 = row.get(0).map_err(|e| e.to_string())?;
        return Ok(m + 1);
    }
    Ok(0)
}

/// Compute the next sort_order for a new event in the GLOBAL timeline
/// bucket. Timeline events share one index across all process_ids —
/// they correspond to the in-memory `eventLog.event_log` array
/// position. Returns `MAX(existing) + 1`.
async fn next_timeline_sort_order(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'timeline'",
            libsql::params![plan_id.to_string()],
        )
        .await.map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let m: i64 = row.get(0).map_err(|e| e.to_string())?;
        return Ok(m + 1);
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    event: &str,
    event_at: &str,
) -> Result<(), String> {
    // The PK is (plan_id, scope, destination, process_id, sort_order).
    // DELETE-then-reinsert is unnecessary for a brand-new event_id, but
    // is kept for safety (matches the spec's "defensive" rule on event_data).
    conn.execute(
        "DELETE FROM plan_events \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![
            plan_id.to_string(),
            scope.to_string(),
            destination.to_string(),
            process_id.to_string(),
            sort_order
        ],
    )
    .await.map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO plan_events \
            (plan_id, scope, destination, process_id, sort_order, \
             event, event_at, from_state, to_state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        libsql::params![
            plan_id.to_string(),
            scope.to_string(),
            destination.to_string(),
            process_id.to_string(),
            sort_order,
            event.to_string(),
            event_at.to_string()
        ],
    )
    .await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn insert_kv_rows(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    kv: &[(&str, String)],
) -> Result<(), String> {
    // DELETE-then-reinsert for the KV child rows (defensive — even for
    // a fresh event_id, mirrors syncNormalizedTables' discipline).
    conn.execute(
        "DELETE FROM plan_event_data \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![
            plan_id.to_string(),
            scope.to_string(),
            destination.to_string(),
            process_id.to_string(),
            sort_order
        ],
    )
    .await.map_err(|e| e.to_string())?;
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data \
                (plan_id, scope, destination, process_id, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![
                plan_id.to_string(),
                scope.to_string(),
                destination.to_string(),
                process_id.to_string(),
                sort_order,
                k.to_string(),
                v.clone()
            ],
        )
        .await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// ISO-8601 / RFC 3339 timestamp for the `event_at` / `last_changed`
/// columns (which the TS path sets to `new Date().toISOString()`).
fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // Render as YYYY-MM-DDTHH:MM:SS.sssZ (matches JS Date.toISOString()).
    let (year, month, day, hour, min, sec) = civil_from_unix(secs);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{:03}Z",
        nanos / 1_000_000
    )
}

/// SQLite-friendly `YYYY-MM-DD HH:MM:SS` UTC timestamp (matches
/// `datetime('now')` but computed in Rust so we use one consistent
/// timestamp across the writes in this command).
fn now_db_datetime() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day, hour, min, sec) = civil_from_unix(secs);
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}"
    )
}

/// Howard Hinnant's civil-from-days algorithm. Returns
/// (year, month, day, hour, min, sec) for a unix epoch in seconds.
fn civil_from_unix(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32, hour, min, sec)
}

/// Generate a fresh `operation_runs.run_id`. The TS path uses
/// `crypto.randomUUID()`; this is an equivalent 36-char UUIDv4-shaped
/// string (8-4-4-4-12 hex groups) derived from the system clock + a
/// static mix. The value is VOLATILE — the diff normalizes run_id out.
fn new_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (n as u128);
    let p1 = (nanos & 0xFFFF_FFFF) as u32;
    let p2 = ((nanos >> 32) & 0xFFFF) as u16;
    let p3 = ((nanos >> 48) & 0x0FFF) as u16;
    let p4 = 0x8000 | (((nanos >> 60) & 0x3FFF) as u16);
    let p5 = (nanos as u64) ^ 0xDEAD_BEEF_CAFE_F00D;
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        p1, p2, p3, p4, p5
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors state-manager.ts setDateAnchor lines 620-625 — the order
    /// matches the emission order used by the cascade loop.
    #[test]
    fn dirty_targets_match_ts() {
        assert_eq!(
            DATE_DEPENDENT_DIRTY_TARGETS,
            &[
                "process_3_4_packages",
                "process_3_transportation",
                "process_4_accommodation",
                "process_5_daily_itinerary",
            ]
        );
    }

    #[test]
    fn civil_from_unix_epoch() {
        // 1970-01-01 00:00:00 UTC
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        // 2026-02-17 12:00:00 UTC = 1771329600 (verified)
        assert_eq!(civil_from_unix(1_771_329_600), (2026, 2, 17, 12, 0, 0));
        // 2026-06-08 00:00:00 UTC = 1780876800 (verified)
        assert_eq!(civil_from_unix(1_780_876_800), (2026, 6, 8, 0, 0, 0));
    }

    #[test]
    fn rfc3339_format() {
        // Fixed seed: 2026-06-08 12:34:56.789Z
        // 2026-06-08 12:34:56 UTC = 1770438896
        let s = now_rfc3339();
        // Just assert it has the YYYY-MM-DDTHH:MM:SS.sssZ shape.
        assert!(s.len() >= 24, "got: {s}");
        let bytes = s.as_bytes();
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b'T');
        assert_eq!(bytes[13], b':');
        assert_eq!(bytes[16], b':');
        assert_eq!(bytes[19], b'.');
        assert_eq!(s.as_bytes()[s.len() - 1], b'Z');
    }
}
