//! `day_route_segments` domain writes for `set-route-segment` / `set-route-segments-bulk`.
//!
//! DAL boundary: owns the domain-table SQL (the `day_route_segments` DELETE/INSERT + the `days`
//! touch + the day-exists guard). The audit triad (`plan_events`/`plan_event_data`/`operation_runs`/
//! `plans.version`) stays in `travel-cli` (`cascade::common`) — this module never touches it.

use libsql::Connection;

/// One route segment to write (the typed payload for an INSERT row).
#[derive(Debug, Clone, Default)]
pub struct SegmentWrite {
    pub from_place: String,
    pub to_place: String,
    pub mode: String,
    pub duration_min: Option<i64>,
    pub notes: Option<String>,
    pub start_time: Option<String>,
}

/// True if a `days` row exists for `(plan_id, destination, day)` — the route-segment writes require
/// the parent day to pre-exist. Thin re-export of `repo::days::exists` (single source of truth).
pub async fn day_exists(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<bool, String> {
    crate::repo::days::exists(conn, plan_id, destination, day).await
}

/// Delete the segment at one `(day, sort_order)` slot (single-segment path).
pub async fn delete_slot(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM day_route_segments \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
        libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
    )
    .await
    .map_err(|e| format!("day_route_segments DELETE failed: {e}"))?;
    Ok(())
}

/// Delete every segment for `(day)` (bulk-replace path).
pub async fn delete_all_for_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM day_route_segments \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
        libsql::params![plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("day_route_segments DELETE failed: {e}"))?;
    Ok(())
}

/// Insert one route segment at `(day, sort_order)`. `err_label` is interpolated only into the
/// error message (never the SQL) so the bulk path can name the failing index.
pub async fn insert_segment(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
    seg: &SegmentWrite,
    source: &str,
    err_label: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO day_route_segments \
            (plan_id, destination, day_number, sort_order, from_place, to_place, \
             mode, duration_min, notes, start_time, source) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            sort_order,
            seg.from_place.clone(),
            seg.to_place.clone(),
            seg.mode.clone(),
            seg.duration_min,
            seg.notes.clone(),
            seg.start_time.clone(),
            source.to_string()
        ],
    )
    .await
    .map_err(|e| format!("day_route_segments INSERT{err_label} failed: {e}"))?;
    Ok(())
}

/// Touch `days.updated_at` for the affected day (mirrors the TS touchItinerary side-effect).
pub async fn touch_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET updated_at = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
        libsql::params![now_db.to_string(), plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("days touch UPDATE failed: {e}"))?;
    Ok(())
}
