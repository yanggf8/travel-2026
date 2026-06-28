// Audit helper for GLOBAL catalog mutations (OTA provider catalog), the analogue of
// `cascade::common::record_operation` for plan-scoped mutations.
//
// Why a separate table: `operation_runs.plan_id` is NOT NULL, so it can't record a global
// (non-plan) catalog edit; and the legacy `events.data_text` column is a JSON-ish text
// field we don't touch. `catalog_runs` is the global audit log: one row per catalog CLI
// mutation. There is NO `plans.version` bump (the catalog is not plan-scoped).

use crate::cascade::common::{new_run_id, now_db_datetime};
use libsql::Connection;

/// Append one `catalog_runs` audit row for a global catalog mutation. Returns the run_id.
/// `command_type` e.g. "set-ota-coverage"; `summary` a short human description.
pub async fn record_catalog_run(
    conn: &Connection,
    command_type: &str,
    summary: &str,
) -> Result<String, String> {
    let run_id = new_run_id();
    let now = now_db_datetime();
    conn.execute(
        "INSERT INTO catalog_runs (run_id, command_type, command_summary, status, changed_at) \
         VALUES (?1, ?2, ?3, 'completed', ?4)",
        libsql::params![run_id.clone(), command_type.to_string(), summary.to_string(), now],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(run_id)
}
