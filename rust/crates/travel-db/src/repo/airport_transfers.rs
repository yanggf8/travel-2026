//! `airport_transfers` / `airport_transfer_candidates` domain writes for
//! `set-airport-transfer`.
//!
//! DAL boundary: owns the domain-table SQL. The audit triad
//! (`plan_events`/`plan_event_data`/`operation_runs`/`plans.version`) stays in
//! `travel-cli` (`cascade::common`) — this module never touches it.

use libsql::Connection;

#[derive(Debug, Clone)]
pub struct AirportTransferWrite {
    pub plan_id: String,
    pub destination: String,
    pub direction: String,
    pub status: String,
    pub selected_id: String,
    pub selected_title: String,
    pub selected_route: String,
    pub selected_duration_min: Option<i64>,
    pub selected_price_yen: Option<i64>,
    pub selected_schedule: Option<String>,
}

pub async fn upsert_transfer(
    conn: &Connection,
    w: &AirportTransferWrite,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO airport_transfers \
            (plan_id, destination, direction, status, selected_id, selected_title, \
             selected_route, selected_duration_min, selected_price_yen, selected_schedule, \
             selected_booking_url, selected_notes, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL, ?11) \
         ON CONFLICT(plan_id, destination, direction) DO UPDATE SET \
            status = ?4, \
            selected_id = ?5, \
            selected_title = ?6, \
            selected_route = ?7, \
            selected_duration_min = ?8, \
            selected_price_yen = ?9, \
            selected_schedule = ?10, \
            selected_booking_url = NULL, \
            selected_notes = NULL, \
            updated_at = ?11",
        libsql::params![
            w.plan_id.clone(),
            w.destination.clone(),
            w.direction.clone(),
            w.status.clone(),
            w.selected_id.clone(),
            w.selected_title.clone(),
            w.selected_route.clone(),
            w.selected_duration_min,
            w.selected_price_yen,
            w.selected_schedule.clone(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("airport_transfers upsert failed: {e}"))?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct AirportTransferCandidate {
    pub candidate_id: String,
    pub title: String,
    pub route: String,
    pub duration_min: Option<i64>,
    pub price_yen: Option<i64>,
    pub schedule: Option<String>,
}

pub async fn replace_candidates(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    direction: &str,
    candidates: &[AirportTransferCandidate],
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM airport_transfer_candidates \
         WHERE plan_id = ?1 AND destination = ?2 AND direction = ?3",
        libsql::params![plan_id.to_string(), dest.to_string(), direction.to_string()],
    )
    .await
    .map_err(|e| format!("airport_transfer_candidates DELETE failed: {e}"))?;
    for (i, cand) in candidates.iter().enumerate() {
        conn.execute(
            "INSERT INTO airport_transfer_candidates \
                (plan_id, destination, direction, candidate_id, title, route, \
                 duration_min, price_yen, schedule, booking_url, notes, sort_order, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, ?10, ?11)",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                direction.to_string(),
                cand.candidate_id.clone(),
                cand.title.clone(),
                cand.route.clone(),
                cand.duration_min,
                cand.price_yen,
                cand.schedule.clone(),
                i as i64,
                now_db.to_string()
            ],
        )
        .await
        .map_err(|e| format!("airport_transfer_candidates INSERT failed: {e}"))?;
    }
    Ok(())
}
