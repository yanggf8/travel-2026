//! `process_statuses` domain write — the cross-domain process-status upsert.
//!
//! DAL boundary: owns the `process_statuses` UPSERT SQL. The audit triad stays in `travel-cli`
//! (`cascade::common`).
//!
//! This is the sole home for the process-status upsert. Its only caller is
//! `travel-cli`'s `cascade::status_machine::apply_path`, which every status writer
//! (set-process-status, select-offer, mark-booked, set/clear-accommodation,
//! flow-decision, promote/import-offers, scaffold-itinerary) goes through after planning
//! its path on the process's ladder (2026-11-11). The former byte-identical copy in
//! `repo::plan_offers` was consolidated here (2026-07-02); do not re-introduce a per-module
//! duplicate.

use libsql::Connection;

/// UPSERT one `process_statuses` row (INSERT-on-missing so a plan whose ladder lacks this
/// `process_id` still records the change; ON CONFLICT re-applies status + updated_at).
pub async fn upsert(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
    status: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(plan_id, destination, process_id) DO UPDATE SET \
            status = excluded.status, updated_at = excluded.updated_at",
        libsql::params![
            plan_id.to_string(),
            dest.to_string(),
            process_id.to_string(),
            status.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Read one process's status. A missing row, NULL or empty status is `None`.
pub async fn get(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT status FROM process_statuses \
             WHERE plan_id = ?1 AND destination = ?2 AND process_id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| format!("process_statuses query failed: {e}"))?;
    match rows
        .next()
        .await
        .map_err(|e| format!("process_statuses row read failed: {e}"))?
    {
        Some(row) => {
            let s: Option<String> = row.get(0).ok().flatten();
            Ok(s.filter(|s| !s.is_empty()))
        }
        None => Ok(None),
    }
}

/// One `process_statuses` row of a live (not soft-deleted) plan.
#[derive(Debug, Clone)]
pub struct ProcessStatusRow {
    pub plan_id: String,
    pub destination: String,
    pub process_id: String,
    /// Empty when the column is NULL.
    pub status: String,
}

/// Every `process_statuses` row whose plan is not soft-deleted (rows with no
/// `plans` row are included — an orphan is drift too). For `validate data`.
pub async fn list_live(conn: &Connection) -> Result<Vec<ProcessStatusRow>, String> {
    let mut rows = conn
        .query(
            "SELECT ps.plan_id, ps.destination, ps.process_id, COALESCE(ps.status, '') \
             FROM process_statuses ps LEFT JOIN plans p ON p.plan_id = ps.plan_id \
             WHERE p.deleted_at IS NULL \
             ORDER BY ps.plan_id, ps.destination, ps.process_id",
            (),
        )
        .await
        .map_err(|e| format!("process_statuses query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("process_statuses row read failed: {e}"))?
    {
        out.push(ProcessStatusRow {
            plan_id: row.get(0).map_err(|e| format!("plan_id: {e}"))?,
            destination: row.get(1).map_err(|e| format!("destination: {e}"))?,
            process_id: row.get(2).map_err(|e| format!("process_id: {e}"))?,
            status: row.get(3).map_err(|e| format!("status: {e}"))?,
        });
    }
    Ok(out)
}
