//! Clothing-advice bands the dashboard shows under each day's weather strip.
//!
//! GLOBAL reference data (not plan-keyed) — same family as `destination_transit`:
//! NO audit triad / NO plans.version. One row per band; the worker
//! (`workers/trip-dashboard-rs`, `render::day::clothing_tip`) evaluates them:
//!   primary group (`temp_low_max` / `temp_high_max` / `temp_else`) → first match
//!   in sort_order, then `swing_min`, then `rain_min` (first match each).
//! `text_zh` / `text_en` are templates; `{lo}` / `{hi}` = rounded low/high temp.
//!
//! Each band also owns an ordered list of clothing ITEMS (`clothing_tip_items`,
//! child rows keyed `(tip_id, sort_order)`): the worker's weather overview counts,
//! over the trip's main days, on how many days each item's band matched — the
//! 「要帶的衣物」 packing summary. Items are content (zh/en pairs), never worker code.

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
    /// The band's complete item list, in display order (sort_order 1..N).
    pub items: Vec<ClothingTipItem>,
}

/// One clothing item a band recommends packing (`clothing_tip_items` row).
#[derive(Debug, Clone, PartialEq)]
pub struct ClothingTipItem {
    pub zh: String,
    pub en: String,
}

/// Insert or fully replace one band (keyed by `tip_id`) AND its item list, in one
/// transaction: the band row is upserted, its old items deleted, the given items
/// inserted with sort_order 1..N (an empty list leaves the band with no items).
/// Returns the band row's affected count.
pub async fn upsert(conn: &Connection, tip: &ClothingTip) -> Result<u64, String> {
    conn.execute("BEGIN", ())
        .await
        .map_err(|e| format!("clothing_tips BEGIN failed: {e}"))?;
    match upsert_inner(conn, tip).await {
        Ok(n) => {
            conn.execute("COMMIT", ())
                .await
                .map_err(|e| format!("clothing_tips COMMIT failed: {e}"))?;
            Ok(n)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(e)
        }
    }
}

async fn upsert_inner(conn: &Connection, tip: &ClothingTip) -> Result<u64, String> {
    let affected = conn
        .execute(
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
        .map_err(|e| format!("clothing_tips upsert failed: {e}"))?;
    if affected != 1 {
        return Err(format!(
            "clothing_tips upsert affected {affected} rows (expected 1) for {}",
            tip.tip_id
        ));
    }
    conn.execute(
        "DELETE FROM clothing_tip_items WHERE tip_id = ?1",
        libsql::params![tip.tip_id.clone()],
    )
    .await
    .map_err(|e| format!("clothing_tip_items delete failed: {e}"))?;
    for (idx, item) in tip.items.iter().enumerate() {
        let n = conn
            .execute(
                "INSERT INTO clothing_tip_items (tip_id, sort_order, item_zh, item_en) \
                 VALUES (?1, ?2, ?3, ?4)",
                libsql::params![
                    tip.tip_id.clone(),
                    (idx + 1) as i64,
                    item.zh.clone(),
                    item.en.clone()
                ],
            )
            .await
            .map_err(|e| format!("clothing_tip_items insert failed: {e}"))?;
        if n != 1 {
            return Err(format!(
                "clothing_tip_items insert affected {n} rows (expected 1) for {} item {}",
                tip.tip_id,
                idx + 1
            ));
        }
    }
    Ok(affected)
}

/// All bands in the worker's read order (`sort_order, tip_id`), each with its
/// items in sort_order.
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
            items: Vec::new(),
        });
    }
    let mut rows = conn
        .query(
            "SELECT tip_id, item_zh, item_en FROM clothing_tip_items ORDER BY tip_id, sort_order",
            (),
        )
        .await
        .map_err(|e| format!("clothing_tip_items query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("clothing_tip_items row read failed: {e}"))?
    {
        let tip_id: String = r.get(0).map_err(|e| format!("clothing_tip_items.tip_id: {e}"))?;
        let item = ClothingTipItem {
            zh: r.get(1).map_err(|e| format!("clothing_tip_items.item_zh: {e}"))?,
            en: r.get(2).map_err(|e| format!("clothing_tip_items.item_en: {e}"))?,
        };
        // An orphan item (its band deleted by raw SQL) has no band to attach to.
        if let Some(tip) = out.iter_mut().find(|t| t.tip_id == tip_id) {
            tip.items.push(item);
        }
    }
    Ok(out)
}

/// Count of items attached to an existing band — orphans left by a raw-SQL band
/// delete are not counted, matching what `list` returns (`validate data` notes when it is 0 while bands exist — the
/// weather overview's packing summary then shows nothing).
pub async fn item_count(conn: &Connection) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM clothing_tip_items \
             WHERE tip_id IN (SELECT tip_id FROM clothing_tips)",
            (),
        )
        .await
        .map_err(|e| format!("clothing_tip_items count failed: {e}"))?;
    let r = rows
        .next()
        .await
        .map_err(|e| format!("clothing_tip_items count read failed: {e}"))?
        .ok_or("clothing_tip_items count returned no row")?;
    r.get(0).map_err(|e| format!("clothing_tip_items count value: {e}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory libSQL with the two tables as `db_migrate.rs` creates them.
    /// `items_check` lets a test make an item insert fail (to prove the rollback).
    async fn mem_db(items_check: &str) -> Connection {
        let db = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE clothing_tips (tip_id TEXT PRIMARY KEY, kind TEXT NOT NULL, \
             threshold REAL, sort_order INTEGER NOT NULL, text_zh TEXT NOT NULL, \
             text_en TEXT NOT NULL, updated_at DATETIME NOT NULL DEFAULT (datetime('now')))",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            &format!(
                "CREATE TABLE clothing_tip_items (tip_id TEXT NOT NULL, sort_order INTEGER NOT NULL, \
                 item_zh TEXT NOT NULL, item_en TEXT NOT NULL {items_check}, \
                 PRIMARY KEY (tip_id, sort_order))"
            ),
            (),
        )
        .await
        .unwrap();
        conn
    }

    fn tip(id: &str, sort: i64, text: &str, items: &[(&str, &str)]) -> ClothingTip {
        ClothingTip {
            tip_id: id.into(),
            kind: "rain_min".into(),
            threshold: Some(30.0),
            sort_order: sort,
            text_zh: text.into(),
            text_en: text.into(),
            items: items
                .iter()
                .map(|(zh, en)| ClothingTipItem { zh: (*zh).into(), en: (*en).into() })
                .collect(),
        }
    }

    #[tokio::test]
    async fn upsert_replaces_band_and_items_and_list_returns_them_in_order() {
        let conn = mem_db("").await;
        assert_eq!(upsert(&conn, &tip("rain", 20, "a", &[("雨傘", "Umbrella"), ("雨衣", "Raincoat")])).await, Ok(1));
        assert_eq!(upsert(&conn, &tip("cold", 10, "b", &[("圍巾", "Scarf")])).await, Ok(1));
        let all = list(&conn).await.unwrap();
        assert_eq!(all.iter().map(|t| t.tip_id.as_str()).collect::<Vec<_>>(), ["cold", "rain"]);
        assert_eq!(all[1].items, tip("x", 0, "", &[("雨傘", "Umbrella"), ("雨衣", "Raincoat")]).items);

        // Same tip_id: the given list is the band's COMPLETE list (old items gone).
        assert_eq!(upsert(&conn, &tip("rain", 20, "c", &[("折疊傘", "Compact umbrella")])).await, Ok(1));
        let all = list(&conn).await.unwrap();
        assert_eq!(all[1].text_zh, "c");
        assert_eq!(all[1].items, tip("x", 0, "", &[("折疊傘", "Compact umbrella")]).items);
        // No items → the band keeps no items; the other band is untouched.
        assert_eq!(upsert(&conn, &tip("rain", 20, "d", &[])).await, Ok(1));
        let all = list(&conn).await.unwrap();
        assert!(all[1].items.is_empty());
        assert_eq!(all[0].items.len(), 1);
        assert_eq!(item_count(&conn).await, Ok(1));
    }

    #[tokio::test]
    async fn failed_item_insert_rolls_back_the_band_too() {
        let conn = mem_db("CHECK (item_zh <> 'boom')").await;
        upsert(&conn, &tip("rain", 20, "old", &[("雨傘", "Umbrella")])).await.unwrap();
        let err = upsert(&conn, &tip("rain", 20, "new", &[("雨衣", "Raincoat"), ("boom", "x")]))
            .await
            .unwrap_err();
        assert!(err.contains("clothing_tip_items insert failed"), "{err}");
        let all = list(&conn).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].text_zh, "old", "band row must roll back with its items");
        assert_eq!(all[0].items, tip("x", 0, "", &[("雨傘", "Umbrella")]).items);
    }
}
