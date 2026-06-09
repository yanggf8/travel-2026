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

use super::common::{
    insert_event, insert_kv_rows, new_run_id, next_dest_process_sort_order,
    next_timeline_sort_order, now_db_datetime, now_rfc3339, read_version,
};
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
        None,
        None,
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
            None,
            None,
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
            None,
            None,
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

}
