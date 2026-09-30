//! `plan_fit_offers` — which global `offers` rows the dashboard FIT comparison
//! lists for a plan+destination (`set-fit-offer`). The audit triad stays in
//! `travel-cli`.

use libsql::Connection;

/// A curated FIT offer as the CLI lists it back.
#[derive(Debug, Clone)]
pub struct FitOfferRow {
    pub offer_id: String,
    pub sort_order: i64,
    pub source_id: String,
    pub price_per_person: i64,
    pub hotel_name: String,
}

/// `(destination, type)` of a global offer, `None` if the id is unknown.
pub async fn offer_kind(
    conn: &Connection,
    offer_id: &str,
) -> Result<Option<(String, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT destination, type FROM offers WHERE id = ?1",
            libsql::params![offer_id.to_string()],
        )
        .await
        .map_err(|e| format!("offers lookup failed: {e}"))?;
    match rows.next().await.map_err(|e| e.to_string())? {
        Some(r) => Ok(Some((
            r.get::<Option<String>>(0).ok().flatten().unwrap_or_default(),
            r.get::<Option<String>>(1).ok().flatten().unwrap_or_default(),
        ))),
        None => Ok(None),
    }
}

/// Insert or re-order one curated offer.
pub async fn upsert(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    offer_id: &str,
    sort_order: i64,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO plan_fit_offers (plan_id, destination, offer_id, sort_order, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(plan_id, destination, offer_id) DO UPDATE SET \
           sort_order = excluded.sort_order, updated_at = excluded.updated_at",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            offer_id.to_string(),
            sort_order,
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("plan_fit_offers upsert failed: {e}"))?;
    Ok(())
}

/// Remove one curated offer. Returns rows deleted.
pub async fn delete(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    offer_id: &str,
) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM plan_fit_offers WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3",
        libsql::params![plan_id.to_string(), destination.to_string(), offer_id.to_string()],
    )
    .await
    .map_err(|e| format!("plan_fit_offers delete failed: {e}"))
}

/// Next free sort_order (max + 1, or 0).
pub async fn next_sort_order(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM plan_fit_offers \
             WHERE plan_id = ?1 AND destination = ?2",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("plan_fit_offers sort query failed: {e}"))?;
    Ok(match rows.next().await.map_err(|e| e.to_string())? {
        Some(r) => r.get::<i64>(0).unwrap_or(0),
        None => 0,
    })
}

/// The curated list, joined to `offers` for display (a dangling id shows empty fields).
pub async fn list(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<FitOfferRow>, String> {
    let mut rows = conn
        .query(
            "SELECT f.offer_id, f.sort_order, COALESCE(o.source_id, ''), \
                    COALESCE(o.price_per_person, 0), COALESCE(o.hotel_name, '') \
             FROM plan_fit_offers f LEFT JOIN offers o ON o.id = f.offer_id \
             WHERE f.plan_id = ?1 AND f.destination = ?2 \
             ORDER BY f.sort_order, f.offer_id",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("plan_fit_offers list failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(FitOfferRow {
            offer_id: r.get(0).unwrap_or_default(),
            sort_order: r.get(1).unwrap_or(0),
            source_id: r.get(2).unwrap_or_default(),
            price_per_person: r.get::<i64>(3).unwrap_or(0),
            hotel_name: r.get(4).unwrap_or_default(),
        });
    }
    Ok(out)
}
