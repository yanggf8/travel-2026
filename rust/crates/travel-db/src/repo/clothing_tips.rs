//! Clothing-advice bands the dashboard shows under each day's weather strip.
//!
//! GLOBAL reference data (not plan-keyed) — same family as `destination_transit`:
//! NO audit triad / NO plans.version. One row per band; the worker
//! (`workers/trip-dashboard-rs`, `render::day::clothing_tip`) evaluates them:
//!   primary group (`temp_low_max` / `temp_high_max` / `temp_else`) → first match
//!   in sort_order, then `swing_min`, then `rain_min` (first match each).
//! `text_zh` / `text_en` are templates; `{lo}` / `{hi}` = rounded low/high temp.

use libsql::Connection;

/// Every kind the worker understands. `temp_else` is the only one without a threshold.
pub const KINDS: [&str; 5] = ["temp_low_max", "temp_high_max", "temp_else", "swing_min", "rain_min"];

/// Placeholders the worker fills in a tip text.
pub const PLACEHOLDERS: [&str; 2] = ["lo", "hi"];

#[derive(Debug, Clone, PartialEq)]
pub struct ClothingTip {
    pub tip_id: String,
    pub kind: String,
    pub threshold: Option<f64>,
    pub sort_order: i64,
    pub text_zh: String,
    pub text_en: String,
}

/// Insert or fully replace one band (keyed by `tip_id`). Returns rows affected.
pub async fn upsert(conn: &Connection, tip: &ClothingTip) -> Result<u64, String> {
    conn.execute(
        "INSERT INTO clothing_tips (tip_id, kind, threshold, sort_order, text_zh, text_en, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now')) \
         ON CONFLICT(tip_id) DO UPDATE SET kind = excluded.kind, threshold = excluded.threshold, \
         sort_order = excluded.sort_order, text_zh = excluded.text_zh, text_en = excluded.text_en, \
         updated_at = excluded.updated_at",
        libsql::params![
            tip.tip_id.clone(),
            tip.kind.clone(),
            tip.threshold,
            tip.sort_order,
            tip.text_zh.clone(),
            tip.text_en.clone(),
        ],
    )
    .await
    .map_err(|e| format!("clothing_tips upsert failed: {e}"))
}

/// All bands in the worker's read order (`sort_order, tip_id`).
pub async fn list(conn: &Connection) -> Result<Vec<ClothingTip>, String> {
    let mut rows = conn
        .query(
            "SELECT tip_id, kind, threshold, sort_order, text_zh, text_en \
             FROM clothing_tips ORDER BY sort_order, tip_id",
            (),
        )
        .await
        .map_err(|e| format!("clothing_tips query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("clothing_tips row read failed: {e}"))?
    {
        out.push(ClothingTip {
            tip_id: r.get(0).map_err(|e| format!("clothing_tips.tip_id: {e}"))?,
            kind: r.get(1).map_err(|e| format!("clothing_tips.kind: {e}"))?,
            threshold: r.get::<Option<f64>>(2).map_err(|e| format!("clothing_tips.threshold: {e}"))?,
            sort_order: r.get(3).map_err(|e| format!("clothing_tips.sort_order: {e}"))?,
            text_zh: r.get(4).map_err(|e| format!("clothing_tips.text_zh: {e}"))?,
            text_en: r.get(5).map_err(|e| format!("clothing_tips.text_en: {e}"))?,
        });
    }
    Ok(out)
}

/// Row count (`validate data` warns when it is 0 — the dashboard then shows no tips).
pub async fn count(conn: &Connection) -> Result<i64, String> {
    let mut rows = conn
        .query("SELECT COUNT(*) FROM clothing_tips", ())
        .await
        .map_err(|e| format!("clothing_tips count failed: {e}"))?;
    let r = rows
        .next()
        .await
        .map_err(|e| format!("clothing_tips count read failed: {e}"))?
        .ok_or("clothing_tips count returned no row")?;
    r.get(0).map_err(|e| format!("clothing_tips count value: {e}"))
}
