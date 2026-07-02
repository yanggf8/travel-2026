//! Plan-lifecycle domain writes (soft-delete etc.) for `mark-plan-deleted`.
//!
//! DAL boundary: owns the domain-table SQL (the `plans` soft-delete UPDATE). The
//! audit triad (`operation_runs`/`plans.version`) stays in `travel-cli`
//! (`cascade::common`) — this module never touches it.
//!
//! (stub — bodies added by the plan_lifecycle DAL migration.)

use libsql::Connection;

/// Soft-delete: set plans.deleted_at (domain write only; version bump is the audit
/// back-half, done by cascade::common::record_operation in the CLI).
pub async fn soft_delete(conn: &Connection, plan_id: &str, now_db: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE plans SET deleted_at = datetime('now'), updated_at = ?2 WHERE plan_id = ?1",
        libsql::params![plan_id.to_string(), now_db.to_string()],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
