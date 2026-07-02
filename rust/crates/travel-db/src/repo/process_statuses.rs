//! `process_statuses` domain write — the cross-domain process-status upsert.
//!
//! DAL boundary: owns the `process_statuses` UPSERT SQL. The audit triad stays in `travel-cli`
//! (`cascade::common`).
//!
//! This is the sole home for the process-status upsert (a cross-domain concern used by the offer
//! cascade, promote/import, scaffold, etc.). The former byte-identical copy in `repo::plan_offers`
//! was consolidated here (2026-07-02); do not re-introduce a per-module duplicate.

use libsql::Connection;

/// UPSERT one `process_statuses` row (INSERT-on-missing so a plan whose ladder lacks this
/// `process_id` still records the change; ON CONFLICT re-applies status + updated_at). SQL is
/// byte-identical to the inline `set_status` in `cascade::select_offer`.
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
