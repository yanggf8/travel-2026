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
    /// `booked = 1`: the FIT that was actually booked (`set-fit-offer --booked`).
    pub booked: bool,
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
                    COALESCE(o.price_per_person, 0), COALESCE(o.hotel_name, ''), f.booked \
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
            booked: r.get::<i64>(5).unwrap_or(0) == 1,
        });
    }
    Ok(out)
}

/// `booked` of one listed offer; `None` when the offer is not in the plan's FIT list.
pub async fn booked_flag(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    offer_id: &str,
) -> Result<Option<bool>, String> {
    let mut rows = conn
        .query(
            "SELECT booked FROM plan_fit_offers \
             WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), offer_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_fit_offers booked read failed: {e}"))?;
    match rows.next().await.map_err(|e| e.to_string())? {
        Some(r) => Ok(Some(
            r.get::<i64>(0)
                .map_err(|e| format!("plan_fit_offers booked value: {e}"))?
                == 1,
        )),
        None => Ok(None),
    }
}

/// Offer ids other than `offer_id` currently marked booked for this plan+destination.
pub async fn other_booked(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    offer_id: &str,
) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT offer_id FROM plan_fit_offers \
             WHERE plan_id = ?1 AND destination = ?2 AND offer_id <> ?3 AND booked = 1 \
             ORDER BY offer_id",
            libsql::params![plan_id.to_string(), destination.to_string(), offer_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_fit_offers booked list failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(r.get::<String>(0).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Unmark every booked offer other than `offer_id` so the one-booked partial
/// unique index accepts the new mark. Returns rows changed.
pub async fn clear_other_booked(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    offer_id: &str,
    now_db: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE plan_fit_offers SET booked = 0, updated_at = ?4 \
         WHERE plan_id = ?1 AND destination = ?2 AND offer_id <> ?3 AND booked = 1",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            offer_id.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("plan_fit_offers clear-booked failed: {e}"))
}

/// Set `booked` on one listed offer. Returns rows changed (0 = not listed).
pub async fn set_booked(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    offer_id: &str,
    booked: bool,
    now_db: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE plan_fit_offers SET booked = ?4, updated_at = ?5 \
         WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            offer_id.to_string(),
            if booked { 1i64 } else { 0i64 },
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("plan_fit_offers set-booked failed: {e}"))
}

/// One plan+destination's FIT-list booking state, for `validate data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitBookingState {
    pub plan_id: String,
    pub destination: String,
    /// `process_4_accommodation` status ('' when the row is absent).
    pub p4_status: String,
    /// Listed FIT offers whose `offers` row exists (the ones the dashboard shows).
    pub listed: i64,
    /// Listed rows with `booked = 1` whose `offers` row exists — the worker JOINs
    /// offers, so a marker on a missing offer frames nothing on the page.
    pub booked: i64,
    /// The plan_offer_selection's offer (via plan_offers) shares a source_id
    /// with a listed FIT offer — the dashboard's selection-based booked path.
    pub selection_matches: bool,
}

/// FIT-list booking state for every live (non-deleted) plan+destination that
/// has a FIT list.
pub async fn booking_states(conn: &Connection) -> Result<Vec<FitBookingState>, String> {
    let mut rows = conn
        .query(
            "SELECT f.plan_id, f.destination, \
                    COALESCE((SELECT ps.status FROM process_statuses ps \
                              WHERE ps.plan_id = f.plan_id AND ps.destination = f.destination \
                                AND ps.process_id = 'process_4_accommodation'), '') AS p4, \
                    SUM(CASE WHEN o.id IS NOT NULL THEN 1 ELSE 0 END) AS listed, \
                    SUM(CASE WHEN f.booked = 1 AND o.id IS NOT NULL THEN 1 ELSE 0 END) AS booked_n, \
                    EXISTS(SELECT 1 FROM plan_offer_selection sel \
                           JOIN plan_offers po ON po.plan_id = sel.plan_id \
                             AND po.destination = sel.destination AND po.id = sel.selected_offer_id \
                           JOIN plan_fit_offers f2 ON f2.plan_id = sel.plan_id \
                             AND f2.destination = sel.destination \
                           JOIN offers o2 ON o2.id = f2.offer_id AND o2.source_id = po.source_id \
                           WHERE sel.plan_id = f.plan_id AND sel.destination = f.destination) AS sel_match \
             FROM plan_fit_offers f \
             JOIN plans p ON p.plan_id = f.plan_id AND p.deleted_at IS NULL \
             LEFT JOIN offers o ON o.id = f.offer_id \
             GROUP BY f.plan_id, f.destination \
             ORDER BY f.plan_id, f.destination",
            (),
        )
        .await
        .map_err(|e| format!("plan_fit_offers booking-state query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("plan_fit_offers booking-state row failed: {e}"))?
    {
        out.push(FitBookingState {
            plan_id: r.get(0).map_err(|e| e.to_string())?,
            destination: r.get(1).map_err(|e| e.to_string())?,
            p4_status: r.get(2).map_err(|e| e.to_string())?,
            listed: r.get(3).map_err(|e| e.to_string())?,
            booked: r.get(4).map_err(|e| e.to_string())?,
            selection_matches: r.get::<i64>(5).map_err(|e| e.to_string())? == 1,
        });
    }
    Ok(out)
}
