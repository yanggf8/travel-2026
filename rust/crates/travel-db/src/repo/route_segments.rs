//! `day_route_segments` domain writes for `set-route-segment` / `set-route-segments-bulk`.
//!
//! DAL boundary: owns the domain-table SQL (the `day_route_segments` DELETE/INSERT + the `days`
//! touch + the day-exists guard). The audit triad (`plan_events`/`plan_event_data`/`operation_runs`/
//! `plans.version`) stays in `travel-cli` (`cascade::common`) — this module never touches it.

use crate::repo::records::{ensure_unrecorded, RecordScope};
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
/// Refuses first when the slot carries a trip record (spec X3).
pub async fn delete_slot(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
) -> Result<(), String> {
    ensure_unrecorded(
        conn,
        plan_id,
        destination,
        RecordScope::LegSlot { day, sort_order },
    )
    .await?;
    delete_via_slot(conn, plan_id, destination, day, sort_order).await?;
    conn.execute(
        "DELETE FROM day_route_segments \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
        libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
    )
    .await
    .map_err(|e| format!("day_route_segments DELETE failed: {e}"))?;
    Ok(())
}

/// Delete every `ai_recommended` segment for one day (derive-routes stale cleanup).
/// Refuses first when any of those legs carries a trip record (spec X3).
pub async fn delete_ai_recommended_for_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<u64, String> {
    ensure_unrecorded(
        conn,
        plan_id,
        destination,
        RecordScope::LegsAiRecommendedOfDay { day },
    )
    .await?;
    conn.execute(
        "DELETE FROM day_route_segment_via WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
           AND sort_order IN (SELECT sort_order FROM day_route_segments \
                              WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
                                AND source = 'ai_recommended')",
        libsql::params![plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("day_route_segment_via ai_recommended DELETE failed: {e}"))?;
    conn.execute(
        "DELETE FROM day_route_segments WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND source = 'ai_recommended'",
        libsql::params![plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("day_route_segments ai_recommended DELETE failed: {e}"))
}

/// Delete every segment for `(day)` (bulk-replace path).
/// Refuses first when any leg of the day carries a trip record (spec X3).
pub async fn delete_all_for_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(), String> {
    ensure_unrecorded(conn, plan_id, destination, RecordScope::LegsOfDay { day }).await?;
    conn.execute(
        "DELETE FROM day_route_segment_via \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
        libsql::params![plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("day_route_segment_via DELETE failed: {e}"))?;
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

/// Rename a place label across every segment of one destination (both endpoint
/// columns). Used by `set-poi-title --segments` so a renamed POI keeps its
/// exact-key match with the segment labels that reference it. Returns
/// (from_count, to_count).
pub async fn rename_place_labels(
    conn: &Connection,
    destination: &str,
    old: &str,
    new: &str,
) -> Result<(u64, u64), String> {
    let from_n = conn
        .execute(
            "UPDATE day_route_segments SET from_place = ?1 \
             WHERE destination = ?2 AND from_place = ?3",
            libsql::params![
                new.to_string(),
                destination.to_string(),
                old.to_string()
            ],
        )
        .await
        .map_err(|e| format!("day_route_segments from_place rename failed: {e}"))?;
    let to_n = conn
        .execute(
            "UPDATE day_route_segments SET to_place = ?1 \
             WHERE destination = ?2 AND to_place = ?3",
            libsql::params![
                new.to_string(),
                destination.to_string(),
                old.to_string()
            ],
        )
        .await
        .map_err(|e| format!("day_route_segments to_place rename failed: {e}"))?;
    Ok((from_n, to_n))
}

/// Flip `ai_recommended` route segments to `confirmed`, scoped by optional day.
/// `day_route_segments` has no `session_type` or `updated_at` column.
pub async fn confirm_routes(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: Option<i64>,
) -> Result<u64, String> {
    let mut sql = String::from("UPDATE day_route_segments SET source = 'confirmed' WHERE 1=1");
    let mut binds: Vec<libsql::Value> = Vec::new();
    binds.push(libsql::Value::Text(plan_id.to_string()));
    sql.push_str(&format!(" AND plan_id = ?{}", binds.len()));
    binds.push(libsql::Value::Text(destination.to_string()));
    sql.push_str(&format!(" AND destination = ?{}", binds.len()));
    sql.push_str(" AND source = 'ai_recommended'");
    if let Some(day) = day {
        binds.push(libsql::Value::Integer(day));
        sql.push_str(&format!(" AND day_number = ?{}", binds.len()));
    }
    conn.execute(&sql, binds)
        .await
        .map_err(|e| format!("routes confirm recommendations failed: {e}"))
}

#[derive(Debug, Clone)]
pub struct RecommendedRoute {
    pub day_number: i64,
    pub sort_order: i64,
    pub from_place: String,
    pub to_place: String,
    pub mode: String,
}

pub async fn list_recommended_routes(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: Option<i64>,
) -> Result<Vec<RecommendedRoute>, String> {
    let mut sql = String::from(
        "SELECT day_number, sort_order, from_place, to_place, mode FROM day_route_segments WHERE 1=1",
    );
    let mut binds: Vec<libsql::Value> = Vec::new();
    binds.push(libsql::Value::Text(plan_id.to_string()));
    sql.push_str(&format!(" AND plan_id = ?{}", binds.len()));
    binds.push(libsql::Value::Text(destination.to_string()));
    sql.push_str(&format!(" AND destination = ?{}", binds.len()));
    sql.push_str(" AND source = 'ai_recommended'");
    if let Some(day) = day {
        binds.push(libsql::Value::Integer(day));
        sql.push_str(&format!(" AND day_number = ?{}", binds.len()));
    }
    sql.push_str(" ORDER BY day_number, sort_order");
    let mut rows = conn
        .query(&sql, binds)
        .await
        .map_err(|e| format!("list_recommended_routes failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(RecommendedRoute {
            day_number: row.get(0).map_err(|e| e.to_string())?,
            sort_order: row.get(1).map_err(|e| e.to_string())?,
            from_place: row.get(2).map_err(|e| e.to_string())?,
            to_place: row.get(3).map_err(|e| e.to_string())?,
            mode: row.get(4).map_err(|e| e.to_string())?,
        });
    }
    Ok(out)
}

// ── via points (day_route_segment_via) ───────────────────────────────────────
//
// Spec 2026-11-20 §6: ordered `lat,lon` waypoints per route segment, keyed by the
// segment's `(plan, dest, day, sort_order)` position. Stored only as coordinates so
// snapshot-maps / verify-site stay reproducible offline.

/// One OSRM waypoint (lat, lon).
pub type Via = (f64, f64);

/// Ordered via points of one segment (empty when none).
pub async fn list_via(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
) -> Result<Vec<Via>, String> {
    let mut rows = conn
        .query(
            "SELECT lat, lon FROM day_route_segment_via \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4 \
             ORDER BY via_order",
            libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
        )
        .await
        .map_err(|e| format!("day_route_segment_via SELECT failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let lat: f64 = row.get(0).map_err(|e| e.to_string())?;
        let lon: f64 = row.get(1).map_err(|e| e.to_string())?;
        out.push((lat, lon));
    }
    Ok(out)
}

/// Delete one slot's via rows.
pub async fn delete_via_slot(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM day_route_segment_via \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
        libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
    )
    .await
    .map_err(|e| format!("day_route_segment_via DELETE failed: {e}"))?;
    Ok(())
}

/// Replace one slot's whole via list (`via_order` 0..n). An empty list just clears it.
pub async fn replace_via(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
    via: &[Via],
) -> Result<(), String> {
    delete_via_slot(conn, plan_id, destination, day, sort_order).await?;
    for (i, (lat, lon)) in via.iter().enumerate() {
        conn.execute(
            "INSERT INTO day_route_segment_via \
                (plan_id, destination, day_number, sort_order, via_order, lat, lon) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                sort_order,
                i as i64,
                *lat,
                *lon
            ],
        )
        .await
        .map_err(|e| format!("day_route_segment_via INSERT failed: {e}"))?;
    }
    Ok(())
}

/// Every via list of one plan+destination keyed by `(day, sort_order)` — snapshot-maps
/// loads this once and matches it to the segments it already reads.
pub async fn list_all_via(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<std::collections::HashMap<(i64, i64), Vec<Via>>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, sort_order, lat, lon FROM day_route_segment_via \
             WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number, sort_order, via_order",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("day_route_segment_via SELECT failed: {e}"))?;
    let mut out: std::collections::HashMap<(i64, i64), Vec<Via>> = std::collections::HashMap::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let day: i64 = row.get(0).map_err(|e| e.to_string())?;
        let so: i64 = row.get(1).map_err(|e| e.to_string())?;
        let lat: f64 = row.get(2).map_err(|e| e.to_string())?;
        let lon: f64 = row.get(3).map_err(|e| e.to_string())?;
        out.entry((day, so)).or_default().push((lat, lon));
    }
    Ok(out)
}

/// A segment's identity as `carry_via_over` sees it (labels + via), taken BEFORE a rewrite.
#[derive(Debug, Clone, PartialEq)]
pub struct SegSnap {
    pub sort_order: i64,
    pub from_place: String,
    pub to_place: String,
    pub via: Vec<Via>,
}

/// Which old segments a rewrite replaces.
#[derive(Debug, Clone, Copy)]
pub enum SnapScope {
    /// One slot (`set-route-segment`).
    Slot(i64),
    /// Every segment of the day (`set-route-segments-bulk`).
    All,
    /// The `ai_recommended` segments of the day (`derive-routes`).
    AiRecommended,
}

/// Snapshot the segments a rewrite is about to replace (labels + via), in `sort_order`.
pub async fn snapshot_segments(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    scope: SnapScope,
) -> Result<Vec<SegSnap>, String> {
    let mut sql = String::from(
        "SELECT sort_order, from_place, to_place FROM day_route_segments \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
    );
    let mut binds: Vec<libsql::Value> = vec![
        libsql::Value::Text(plan_id.to_string()),
        libsql::Value::Text(destination.to_string()),
        libsql::Value::Integer(day),
    ];
    match scope {
        SnapScope::Slot(so) => {
            binds.push(libsql::Value::Integer(so));
            sql.push_str(" AND sort_order = ?4");
        }
        SnapScope::All => {}
        SnapScope::AiRecommended => sql.push_str(" AND source = 'ai_recommended'"),
    }
    sql.push_str(" ORDER BY sort_order");
    let mut rows = conn
        .query(&sql, binds)
        .await
        .map_err(|e| format!("route segment snapshot failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(SegSnap {
            sort_order: row.get(0).map_err(|e| e.to_string())?,
            from_place: row.get(1).map_err(|e| e.to_string())?,
            to_place: row.get(2).map_err(|e| e.to_string())?,
            via: Vec::new(),
        });
    }
    drop(rows);
    for s in &mut out {
        s.via = list_via(conn, plan_id, destination, day, s.sort_order).await?;
    }
    Ok(out)
}

/// Outcome of matching a rewrite's new segments to the old ones, plus the DB effect.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViaCarryReport {
    /// `(new_sort_order, from, to, via_count)` — via moved onto the new segment.
    pub kept: Vec<(i64, String, String, usize)>,
    /// Old segments whose via was dropped: `(old_sort_order, from, to, via_count)`.
    pub cleared: Vec<(i64, String, String, usize)>,
}

/// PURE matching: for each new `(sort_order, from, to)` in order, take the first not yet
/// used old snapshot whose `from_place`/`to_place` text is identical. Returns
/// `(new index, old index)` pairs (every match, with or without via) so duplicates line
/// up one-to-one.
pub fn match_via_carry(old: &[SegSnap], new: &[(i64, String, String)]) -> Vec<(usize, usize)> {
    let mut used = vec![false; old.len()];
    let mut pairs = Vec::new();
    for (ni, (_, from, to)) in new.iter().enumerate() {
        if let Some(oi) = old
            .iter()
            .enumerate()
            .position(|(i, o)| !used[i] && &o.from_place == from && &o.to_place == to)
        {
            used[oi] = true;
            pairs.push((ni, oi));
        }
    }
    pairs
}

/// PURE: split a rewrite into kept / cleared via (no DB). `carry_via_over` applies it.
pub fn plan_via_carry(old: &[SegSnap], new: &[(i64, String, String)]) -> ViaCarryReport {
    let pairs = match_via_carry(old, new);
    let mut report = ViaCarryReport::default();
    let mut matched_old = vec![false; old.len()];
    for (ni, oi) in &pairs {
        matched_old[*oi] = true;
        let o = &old[*oi];
        if !o.via.is_empty() {
            let n = &new[*ni];
            report.kept.push((n.0, n.1.clone(), n.2.clone(), o.via.len()));
        }
    }
    for (oi, o) in old.iter().enumerate() {
        if !matched_old[oi] && !o.via.is_empty() {
            report
                .cleared
                .push((o.sort_order, o.from_place.clone(), o.to_place.clone(), o.via.len()));
        }
    }
    report
}

/// The single shared "rewrite keeps via when the endpoints text is unchanged" step used by
/// `set-route-segment`, `set-route-segments-bulk` and `derive-routes` (spec §6.3). Call it
/// AFTER the new segments are inserted (the old via rows were removed with the old
/// segments): matching new segments get their old via re-inserted at the NEW sort_order.
pub async fn carry_via_over(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    old: &[SegSnap],
    new: &[(i64, String, String)],
) -> Result<ViaCarryReport, String> {
    let report = plan_via_carry(old, new);
    for (ni, oi) in match_via_carry(old, new) {
        if !old[oi].via.is_empty() {
            replace_via(conn, plan_id, destination, day, new[ni].0, &old[oi].via).await?;
        }
    }
    Ok(report)
}

/// Plain-text lines for a carry report (`via kept:` / `via cleared:`), empty when neither.
pub fn format_via_report(day: i64, r: &ViaCarryReport) -> Vec<String> {
    let mut out = Vec::new();
    for (so, f, t, n) in &r.kept {
        out.push(format!("   via kept: D{day} S{so} {f}→{t} ({n} 點)"));
    }
    for (so, f, t, n) in &r.cleared {
        out.push(format!(
            "   via cleared: D{day} S{so} 舊 {f}→{t}（端點已變，{n} 點）"
        ));
    }
    out
}

// ── partial update / delete-with-renumber ────────────────────────────────────

/// One stored segment row (the columns `update-route-segment` edits plus provenance).
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentRow {
    pub sort_order: i64,
    pub from_place: String,
    pub to_place: String,
    pub mode: String,
    pub duration_min: Option<i64>,
    pub notes: Option<String>,
    pub start_time: Option<String>,
    pub source: String,
    pub record_status: Option<String>,
}

pub async fn read_segment(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
) -> Result<Option<SegmentRow>, String> {
    let mut rows = conn
        .query(
            "SELECT sort_order, from_place, to_place, mode, duration_min, notes, start_time, \
                    source, record_status \
             FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
            libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
        )
        .await
        .map_err(|e| format!("day_route_segments SELECT failed: {e}"))?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(SegmentRow {
        sort_order: row.get(0).map_err(|e| e.to_string())?,
        from_place: row.get(1).map_err(|e| e.to_string())?,
        to_place: row.get(2).map_err(|e| e.to_string())?,
        mode: row.get(3).map_err(|e| e.to_string())?,
        duration_min: row.get(4).ok().flatten(),
        notes: row.get(5).ok().flatten(),
        start_time: row.get(6).ok().flatten(),
        source: row.get(7).map_err(|e| e.to_string())?,
        record_status: row.get(8).ok().flatten(),
    }))
}

/// The day's existing `sort_order`s (for the "no such slot" error message).
pub async fn list_sort_orders(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<Vec<i64>, String> {
    let mut rows = conn
        .query(
            "SELECT sort_order FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 ORDER BY sort_order",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("day_route_segments SELECT failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(row.get(0).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Other segments of the plan+destination with the same endpoint labels (any day), as
/// `(day, sort_order)` — for the "same endpoints, different via" warning.
pub async fn segments_with_endpoints(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    from_place: &str,
    to_place: &str,
) -> Result<Vec<(i64, i64)>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, sort_order FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 AND from_place = ?3 AND to_place = ?4 \
             ORDER BY day_number, sort_order",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                from_place.to_string(),
                to_place.to_string()
            ],
        )
        .await
        .map_err(|e| format!("day_route_segments SELECT failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push((
            row.get(0).map_err(|e| e.to_string())?,
            row.get(1).map_err(|e| e.to_string())?,
        ));
    }
    Ok(out)
}

/// Partial edit of one segment (same shape as `ActivityPatch`). `None` = leave alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SegmentPatch {
    pub from_place: Option<String>,
    pub to_place: Option<String>,
    pub mode: Option<String>,
    pub duration_min: Option<i64>,
    pub start_time: Option<String>,
    pub notes: Option<String>,
    /// `--append-notes` text (joined with `；`).
    pub append_notes: Option<String>,
    pub clear_duration: bool,
    pub clear_start_time: bool,
    pub clear_notes: bool,
}

/// Separator for appended route-segment notes (rendered on one line, spec Q2).
pub const SEGMENT_NOTE_SEP: &str = "；";

impl SegmentPatch {
    /// The row after the patch. Errs only for an empty `--append-notes`.
    pub fn apply(&self, before: &SegmentRow) -> Result<SegmentRow, String> {
        let mut after = before.clone();
        if let Some(v) = &self.from_place {
            after.from_place = v.clone();
        }
        if let Some(v) = &self.to_place {
            after.to_place = v.clone();
        }
        if let Some(v) = &self.mode {
            after.mode = v.clone();
        }
        if let Some(v) = self.duration_min {
            after.duration_min = Some(v);
        }
        if self.clear_duration {
            after.duration_min = None;
        }
        if let Some(v) = &self.start_time {
            after.start_time = Some(v.clone());
        }
        if self.clear_start_time {
            after.start_time = None;
        }
        if let Some(v) = &self.notes {
            after.notes = Some(v.clone());
        }
        if self.clear_notes {
            after.notes = None;
        }
        if let Some(add) = &self.append_notes {
            // The shared helper dedupes on whole LINES; route notes are one line joined
            // by `；`, so also treat an already-present `；`-part as "no change".
            let already = before
                .notes
                .as_deref()
                .map(|n| n.split(SEGMENT_NOTE_SEP).any(|p| p.trim() == add.trim()))
                .unwrap_or(false);
            if !already {
                if let Some(merged) = crate::repo::text_merge::append_note(
                    before.notes.as_deref(),
                    add,
                    SEGMENT_NOTE_SEP,
                )? {
                    after.notes = Some(merged);
                }
            } else if add.trim().is_empty() {
                return Err("note text to append must not be empty".to_string());
            }
        }
        Ok(after)
    }
}

/// Changed columns as `(column, from, to)` in fixed order (drives the audit KV + printout).
pub fn segment_diff(
    before: &SegmentRow,
    after: &SegmentRow,
) -> Vec<(&'static str, Option<String>, Option<String>)> {
    let mut out = Vec::new();
    let mut push = |col: &'static str, a: Option<String>, b: Option<String>| {
        if a != b {
            out.push((col, a, b));
        }
    };
    push("from_place", Some(before.from_place.clone()), Some(after.from_place.clone()));
    push("to_place", Some(before.to_place.clone()), Some(after.to_place.clone()));
    push("mode", Some(before.mode.clone()), Some(after.mode.clone()));
    push(
        "duration_min",
        before.duration_min.map(|n| n.to_string()),
        after.duration_min.map(|n| n.to_string()),
    );
    push("start_time", before.start_time.clone(), after.start_time.clone());
    push("notes", before.notes.clone(), after.notes.clone());
    out
}

/// In-place UPDATE of the editable columns. Never touches `source` / `record_*`.
pub async fn update_segment_fields(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    after: &SegmentRow,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE day_route_segments \
             SET from_place = ?1, to_place = ?2, mode = ?3, duration_min = ?4, \
                 start_time = ?5, notes = ?6 \
             WHERE plan_id = ?7 AND destination = ?8 AND day_number = ?9 AND sort_order = ?10",
            libsql::params![
                after.from_place.clone(),
                after.to_place.clone(),
                after.mode.clone(),
                after.duration_min,
                after.start_time.clone(),
                after.notes.clone(),
                plan_id.to_string(),
                destination.to_string(),
                day,
                after.sort_order
            ],
        )
        .await
        .map_err(|e| format!("day_route_segments UPDATE failed: {e}"))?;
    if n == 0 {
        return Err(format!("no segment at day {day} sort_order {}", after.sort_order));
    }
    Ok(())
}

/// PURE: `(old, new)` sort_order pairs for the segments that shift down by one after
/// `removed` is deleted (the two-phase SQL below applies exactly this mapping).
pub fn shift_after_delete(sorts: &[i64], removed: i64) -> Vec<(i64, i64)> {
    sorts
        .iter()
        .copied()
        .filter(|s| *s > removed)
        .map(|s| (s, s - 1))
        .collect()
}

/// Delete one segment and shift every later `sort_order` down by one, in the SAME repo
/// function for `day_route_segments` and `day_route_segment_via` (spec §4.3 M4). Two-phase
/// negative offset so the primary key never collides mid-way: phase 1 maps `s > removed`
/// to `-1 - s` (all negative, mutually distinct), phase 2 maps `n < 0` to `-n - 2` (= s-1).
/// Refuses first when the slot carries a trip record. Returns the deleted row, its via
/// count and how many later segments were renumbered. Caller owns the transaction.
pub async fn delete_segment_renumber(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
) -> Result<(SegmentRow, usize, i64), String> {
    ensure_unrecorded(
        conn,
        plan_id,
        destination,
        RecordScope::LegSlot { day, sort_order },
    )
    .await?;
    let Some(row) = read_segment(conn, plan_id, destination, day, sort_order).await? else {
        let have = list_sort_orders(conn, plan_id, destination, day).await?;
        return Err(format!(
            "no route segment at day {day} sort_order {sort_order}; existing sort_orders: {}",
            if have.is_empty() {
                "(none)".to_string()
            } else {
                have.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
            }
        ));
    };
    let via_count = list_via(conn, plan_id, destination, day, sort_order).await?.len();
    delete_via_slot(conn, plan_id, destination, day, sort_order).await?;
    conn.execute(
        "DELETE FROM day_route_segments \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
        libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
    )
    .await
    .map_err(|e| format!("day_route_segments DELETE failed: {e}"))?;
    let mut renumbered = 0i64;
    for table in ["day_route_segments", "day_route_segment_via"] {
        let phase1 = format!(
            "UPDATE {table} SET sort_order = -1 - sort_order \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order > ?4"
        );
        let n = conn
            .execute(
                &phase1,
                libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
            )
            .await
            .map_err(|e| format!("{table} renumber phase 1 failed: {e}"))?;
        let phase2 = format!(
            "UPDATE {table} SET sort_order = -sort_order - 2 \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order < 0"
        );
        conn.execute(
            &phase2,
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("{table} renumber phase 2 failed: {e}"))?;
        if table == "day_route_segments" {
            renumbered = n as i64;
        }
    }
    Ok((row, via_count, renumbered))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(so: i64, f: &str, t: &str, via: &[Via]) -> SegSnap {
        SegSnap {
            sort_order: so,
            from_place: f.into(),
            to_place: t.into(),
            via: via.to_vec(),
        }
    }
    fn nw(so: i64, f: &str, t: &str) -> (i64, String, String) {
        (so, f.into(), t.into())
    }

    #[test]
    fn carry_keeps_via_on_identical_endpoints_and_renumbers() {
        let old = [snap(0, "A", "B", &[(1.0, 2.0), (3.0, 4.0)]), snap(1, "B", "C", &[])];
        // New order swapped: the A→B segment moves to sort_order 1.
        let new = [nw(0, "B", "C"), nw(1, "A", "B")];
        let r = plan_via_carry(&old, &new);
        assert_eq!(r.kept, vec![(1, "A".into(), "B".into(), 2)]);
        assert!(r.cleared.is_empty());
    }

    #[test]
    fn carry_clears_via_when_endpoints_change_and_lists_it() {
        let old = [snap(0, "A", "B", &[(1.0, 2.0)])];
        let new = [nw(0, "A", "C")];
        let r = plan_via_carry(&old, &new);
        assert!(r.kept.is_empty());
        assert_eq!(r.cleared, vec![(0, "A".into(), "B".into(), 1)]);
        let lines = format_via_report(2, &r);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("via cleared: D2 S0") && lines[0].contains("A→B"));
    }

    #[test]
    fn carry_duplicate_endpoints_consumed_once() {
        let old = [snap(0, "A", "B", &[(1.0, 2.0)])];
        let new = [nw(0, "A", "B"), nw(1, "A", "B")];
        let r = plan_via_carry(&old, &new);
        assert_eq!(r.kept.len(), 1, "one old via may feed only one new segment");
        assert_eq!(r.kept[0].0, 0);
        assert!(r.cleared.is_empty());
        let lines = format_via_report(1, &r);
        assert!(lines[0].starts_with("   via kept: D1 S0"));
    }

    #[test]
    fn carry_vialess_old_segment_is_consumed_but_not_reported() {
        let old = [snap(0, "A", "B", &[]), snap(1, "A", "B", &[(9.0, 9.0)])];
        let new = [nw(0, "A", "B")];
        // First identical old (no via) is the match; the second old's via is cleared.
        let r = plan_via_carry(&old, &new);
        assert!(r.kept.is_empty());
        assert_eq!(r.cleared.len(), 1);
        assert_eq!(r.cleared[0].0, 1);
    }

    #[test]
    fn shift_after_delete_maps_later_slots_down_by_one() {
        assert_eq!(shift_after_delete(&[0, 1, 2, 3, 4], 1), vec![(2, 1), (3, 2), (4, 3)]);
        // Input with a hole keeps its hole (the shift is relative, per spec M4).
        assert_eq!(shift_after_delete(&[0, 1, 5, 6], 1), vec![(5, 4), (6, 5)]);
        assert!(shift_after_delete(&[0, 1], 1).is_empty());
    }

    /// The two phase formulas must map s -> s-1 and never collide mid-way.
    #[test]
    fn two_phase_formulas_are_collision_free_and_invert() {
        let removed = 1i64;
        let sorts = [2i64, 3, 4, 7];
        let phase1: Vec<i64> = sorts.iter().map(|s| -1 - s).collect();
        let mut sorted = phase1.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), phase1.len());
        assert!(phase1.iter().all(|n| *n < 0));
        let phase2: Vec<i64> = phase1.iter().map(|n| -n - 2).collect();
        assert_eq!(phase2, sorts.iter().map(|s| s - 1).collect::<Vec<_>>());
        let _ = removed;
    }

    fn row() -> SegmentRow {
        SegmentRow {
            sort_order: 0,
            from_place: "A".into(),
            to_place: "B".into(),
            mode: "driving".into(),
            duration_min: Some(20),
            notes: Some("old".into()),
            start_time: Some("09:00".into()),
            source: "ai_recommended".into(),
            record_status: None,
        }
    }

    #[test]
    fn patch_changes_only_given_fields_and_keeps_source() {
        let p = SegmentPatch { duration_min: Some(30), ..Default::default() };
        let after = p.apply(&row()).unwrap();
        assert_eq!(after.duration_min, Some(30));
        assert_eq!(after.source, "ai_recommended");
        let d = segment_diff(&row(), &after);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].0, "duration_min");
    }

    #[test]
    fn patch_clear_and_noop() {
        let p = SegmentPatch { clear_start_time: true, clear_notes: true, ..Default::default() };
        let after = p.apply(&row()).unwrap();
        assert_eq!(after.start_time, None);
        assert_eq!(after.notes, None);
        let same = SegmentPatch { duration_min: Some(20), ..Default::default() };
        assert!(segment_diff(&row(), &same.apply(&row()).unwrap()).is_empty());
    }

    #[test]
    fn patch_append_notes_uses_fullwidth_semicolon_and_is_idempotent() {
        let p = SegmentPatch { append_notes: Some("new".into()), ..Default::default() };
        let after = p.apply(&row()).unwrap();
        assert_eq!(after.notes.as_deref(), Some("old；new"));
        // Re-applying the same text on the merged row is "no change".
        let again = p.apply(&after).unwrap();
        assert_eq!(again.notes.as_deref(), Some("old；new"));
        assert!(segment_diff(&after, &again).is_empty());
        // NULL existing → just the added text.
        let mut none = row();
        none.notes = None;
        assert_eq!(p.apply(&none).unwrap().notes.as_deref(), Some("new"));
        // Empty append fails.
        let e = SegmentPatch { append_notes: Some("  ".into()), ..Default::default() };
        assert!(e.apply(&row()).is_err());
    }
}

/// A route segment's position and endpoints, as a neighbour in itinerary order.
#[derive(Debug, Clone, PartialEq)]
pub struct NeighbourSegment {
    pub day: i64,
    pub sort_order: i64,
    pub from_place: String,
    pub to_place: String,
}

/// Neighbour by ORDER (not sort_order±1): `forward=false` = previous (same day with a lower
/// sort_order, else the last segment of the previous NON-variant day with segments),
/// `forward=true` = next (same day higher, else the first segment of the next non-variant day).
/// A day whose `days.variant_of` is set is an alternative to another day, never a neighbour.
pub async fn neighbour_segment(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
    forward: bool,
) -> Result<Option<NeighbourSegment>, String> {
    let (cmp_same, cmp_day, ord) = if forward { (">", ">", "ASC") } else { ("<", "<", "DESC") };
    let sql = format!(
        "SELECT s.day_number, s.sort_order, s.from_place, s.to_place FROM day_route_segments s \
         WHERE s.plan_id = ?1 AND s.destination = ?2 \
           AND ((s.day_number = ?3 AND s.sort_order {cmp_same} ?4) \
             OR (s.day_number {cmp_day} ?3 AND NOT EXISTS (SELECT 1 FROM days d \
                  WHERE d.plan_id = s.plan_id AND d.destination = s.destination \
                    AND d.day_number = s.day_number AND d.variant_of IS NOT NULL))) \
         ORDER BY s.day_number {ord}, s.sort_order {ord} LIMIT 1"
    );
    let mut rows = conn
        .query(
            &sql,
            libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
        )
        .await
        .map_err(|e| format!("neighbour segment SELECT failed: {e}"))?;
    let Some(r) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(NeighbourSegment {
        day: r.get(0).map_err(|e| e.to_string())?,
        sort_order: r.get(1).map_err(|e| e.to_string())?,
        from_place: r.get(2).map_err(|e| e.to_string())?,
        to_place: r.get(3).map_err(|e| e.to_string())?,
    }))
}
