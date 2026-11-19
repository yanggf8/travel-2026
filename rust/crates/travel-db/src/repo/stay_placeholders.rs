//! Read model for the stay-placeholder lint (spec 2026-11-19 §2 A3): every text
//! location that can carry an undecided-stay placeholder (route endpoints,
//! activity titles) joined to the plan+destination's P4 status. Read-only.

use libsql::{params, Connection};

/// One scanned text with the plan+destination's P4 status.
#[derive(Debug, Clone)]
pub struct StayTextRow {
    /// Which column the text came from: `day_route_segments.from_place`,
    /// `day_route_segments.to_place` or `activities.title`.
    pub src: String,
    pub plan_id: String,
    pub day: i64,
    pub session: Option<String>,
    pub text: String,
    /// P4 (process_4_accommodation) status; empty when the plan has no P4 row.
    pub p4_status: String,
}

const QUERY: &str = "\
    SELECT 'day_route_segments.from_place' AS src, r.plan_id, r.day_number, NULL AS session, r.from_place AS text, \
           COALESCE(p.status, '') AS p4 \
    FROM day_route_segments r \
    LEFT JOIN process_statuses p ON p.plan_id = r.plan_id AND p.destination = r.destination \
         AND p.process_id = 'process_4_accommodation' \
    WHERE r.from_place IS NOT NULL \
    UNION ALL SELECT 'day_route_segments.to_place', r.plan_id, r.day_number, NULL, r.to_place, COALESCE(p.status, '') \
    FROM day_route_segments r \
    LEFT JOIN process_statuses p ON p.plan_id = r.plan_id AND p.destination = r.destination \
         AND p.process_id = 'process_4_accommodation' \
    WHERE r.to_place IS NOT NULL \
    UNION ALL SELECT 'activities.title', a.plan_id, a.day_number, a.session_type, a.title, COALESCE(p.status, '') \
    FROM activities a \
    LEFT JOIN process_statuses p ON p.plan_id = a.plan_id AND p.destination = a.destination \
         AND p.process_id = 'process_4_accommodation' \
    WHERE a.title IS NOT NULL";

/// Scan every placeholder-capable text. ANY query or row-iteration error is
/// returned (the original libsql message is kept so the caller can recognise a
/// missing table) — a truncated scan must never read as "no placeholders".
pub async fn scan(conn: &Connection) -> Result<Vec<StayTextRow>, String> {
    let mut rows = conn
        .query(QUERY, params![])
        .await
        .map_err(|e| format!("stay placeholder scan failed: {e}"))?;
    let mut out = Vec::new();
    loop {
        let row = match rows.next().await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => return Err(format!("stay placeholder scan row failed: {e}")),
        };
        let col = |e: libsql::Error| format!("stay placeholder scan column failed: {e}");
        out.push(StayTextRow {
            src: row.get(0).map_err(col)?,
            plan_id: row.get(1).map_err(col)?,
            day: row.get(2).map_err(col)?,
            session: row.get::<Option<String>>(3).map_err(col)?,
            text: row.get(4).map_err(col)?,
            p4_status: row.get(5).map_err(col)?,
        });
    }
    Ok(out)
}
