//! Plan-scoped FIT comparison notes (`plan_fit_notes`).
//!
//! `source_id = ''` is the section comparison paragraph. Any other `source_id`
//! is that agency's reason. `recommended = 1` is the single Recommended badge
//! for the plan+destination; the partial unique index rejects a second pick.
//! Lowest-price and price-delta badges are not stored — the dashboard computes
//! them from the offers it actually shows.
//!
//! Audit triad stays in `travel-cli` (`cascade::common`). This module only
//! writes the note rows.

use libsql::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitNote {
    pub recommended: i64,
    pub body_zh: String,
    pub body_en: String,
    /// Room size shown as its own card row. Empty when this note has none.
    pub room_zh: String,
    pub room_en: String,
}

pub async fn get(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    source_id: &str,
) -> Result<Option<FitNote>, String> {
    let mut rows = conn
        .query(
            "SELECT recommended, body_zh, body_en, room_zh, room_en FROM plan_fit_notes \
             WHERE plan_id = ?1 AND destination = ?2 AND source_id = ?3",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                source_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("plan_fit_notes read failed: {e}"))?;
    let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_fit_notes row read failed: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some(FitNote {
        recommended: row.get::<i64>(0).map_err(|e| e.to_string())?,
        body_zh: row.get::<String>(1).unwrap_or_default(),
        body_en: row.get::<String>(2).unwrap_or_default(),
        room_zh: row.get::<String>(3).unwrap_or_default(),
        room_en: row.get::<String>(4).unwrap_or_default(),
    }))
}

/// Drop the recommended flag on every other agency for this plan+destination
/// so the partial unique index can accept the new pick.
pub async fn clear_other_recommended(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    source_id: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE plan_fit_notes SET recommended = 0, updated_at = ?4 \
         WHERE plan_id = ?1 AND destination = ?2 AND source_id <> ?3 AND recommended = 1",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            source_id.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("plan_fit_notes clear-recommended failed: {e}"))?;
    Ok(())
}

pub async fn upsert(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    source_id: &str,
    note: &FitNote,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO plan_fit_notes \
            (plan_id, destination, source_id, recommended, body_zh, body_en, room_zh, room_en, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(plan_id, destination, source_id) DO UPDATE SET \
            recommended = excluded.recommended, \
            body_zh = excluded.body_zh, \
            body_en = excluded.body_en, \
            room_zh = excluded.room_zh, \
            room_en = excluded.room_en, \
            updated_at = excluded.updated_at",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            source_id.to_string(),
            note.recommended,
            note.body_zh.clone(),
            note.body_en.clone(),
            note.room_zh.clone(),
            note.room_en.clone(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("plan_fit_notes upsert failed: {e}"))?;
    Ok(())
}

/// Delete one note. Returns rows affected (0 when there was nothing to clear).
pub async fn delete(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    source_id: &str,
) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM plan_fit_notes WHERE plan_id = ?1 AND destination = ?2 AND source_id = ?3",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            source_id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("plan_fit_notes delete failed: {e}"))
}
