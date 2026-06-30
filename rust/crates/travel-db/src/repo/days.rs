//! `days`-table domain access shared by the itinerary mutations (`set-day-theme`, route segments,
//! …). DAL boundary: owns the `days` domain SQL; the audit triad stays in `travel-cli`.

use libsql::Connection;

/// True if a `days` row exists for `(plan_id, destination, day)`. Mutations require the parent day
/// to pre-exist (scaffolded separately) so a `completed` audit always implies a row actually
/// changed — callers fail loud when this is false.
pub async fn exists(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("days existence row read failed: {e}"))?
        .is_some())
}

/// Apply a `set-day-theme` write to the `days` row, always bumping `updated_at` to `now_db`:
/// - both `theme` and `theme_zh` present → set both
/// - only `theme` → set theme
/// - only `theme_zh` → set theme_zh
/// - neither → touch `updated_at` only (touchItinerary)
///
/// Bound params only; the SQL variant is chosen by which fields are `Some` (mirrors the prior
/// inline 4-way conditional exactly).
pub async fn set_theme(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    theme: Option<&str>,
    theme_zh: Option<&str>,
    now_db: &str,
) -> Result<(), String> {
    match (theme, theme_zh) {
        (Some(t), Some(tz)) => {
            conn.execute(
                "UPDATE days SET theme = ?1, theme_zh = ?2, updated_at = ?3 \
                 WHERE plan_id = ?4 AND destination = ?5 AND day_number = ?6",
                libsql::params![
                    t.to_string(),
                    tz.to_string(),
                    now_db.to_string(),
                    plan_id.to_string(),
                    destination.to_string(),
                    day
                ],
            )
            .await
            .map_err(|e| format!("days UPDATE (theme+zh) failed: {e}"))?;
        }
        (Some(t), None) => {
            conn.execute(
                "UPDATE days SET theme = ?1, updated_at = ?2 \
                 WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
                libsql::params![
                    t.to_string(),
                    now_db.to_string(),
                    plan_id.to_string(),
                    destination.to_string(),
                    day
                ],
            )
            .await
            .map_err(|e| format!("days UPDATE (theme) failed: {e}"))?;
        }
        (None, Some(tz)) => {
            conn.execute(
                "UPDATE days SET theme_zh = ?1, updated_at = ?2 \
                 WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
                libsql::params![
                    tz.to_string(),
                    now_db.to_string(),
                    plan_id.to_string(),
                    destination.to_string(),
                    day
                ],
            )
            .await
            .map_err(|e| format!("days UPDATE (theme_zh) failed: {e}"))?;
        }
        (None, None) => {
            conn.execute(
                "UPDATE days SET updated_at = ?1 \
                 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
                libsql::params![
                    now_db.to_string(),
                    plan_id.to_string(),
                    destination.to_string(),
                    day
                ],
            )
            .await
            .map_err(|e| format!("days UPDATE (touch) failed: {e}"))?;
        }
    }
    Ok(())
}
