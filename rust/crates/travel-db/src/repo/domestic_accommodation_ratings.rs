//! `domestic_accommodation_ratings` — guest ratings, one row per review SOURCE.
//!
//! Kept per-source on purpose: Booking.com scores out of 10, Google out of 5.
//! Collapsing them into one number would invent a rating nobody published, so the
//! scale travels with the score and the dashboard renders each source separately.

use libsql::Connection;

#[derive(Debug, Clone, PartialEq)]
pub struct RatingRow {
    pub accommodation_id: String,
    pub source: String,
    pub score: f64,
    pub scale: f64,
    pub review_count: Option<i64>,
    pub checked_at: String,
    /// Denormalized for the CLI listing only (never written).
    pub hotel_name: String,
}

/// Upsert one source's rating (re-checking a source overwrites it, and refreshes
/// `checked_at` — a rating read months ago should not look current).
pub async fn upsert(
    conn: &Connection,
    accommodation_id: &str,
    source: &str,
    score: f64,
    scale: f64,
    review_count: Option<i64>,
) -> Result<u64, String> {
    conn.execute(
        "INSERT INTO domestic_accommodation_ratings \
         (accommodation_id, source, score, scale, review_count, checked_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now')) \
         ON CONFLICT(accommodation_id, source) DO UPDATE SET \
           score = excluded.score, scale = excluded.scale, \
           review_count = excluded.review_count, checked_at = excluded.checked_at",
        libsql::params![
            accommodation_id.to_string(),
            source.to_string(),
            score,
            scale,
            review_count,
        ],
    )
    .await
    .map_err(|e| format!("domestic_accommodation_ratings UPSERT failed: {e}"))
}

/// Ratings for one destination, joined to the stay for the hotel name.
pub async fn list_by_destination(
    conn: &Connection,
    destination: &str,
) -> Result<Vec<RatingRow>, String> {
    let mut rows = conn
        .query(
            "SELECT r.accommodation_id, r.source, r.score, r.scale, r.review_count, r.checked_at, a.hotel_name \
             FROM domestic_accommodation_ratings r \
             JOIN domestic_accommodations a ON a.id = r.accommodation_id \
             WHERE a.destination = ?1 ORDER BY a.price_twd ASC, r.source",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("domestic_accommodation_ratings SELECT failed: {e}"))?;
    collect(&mut rows).await
}

/// Ratings for one accommodation id.
pub async fn list_by_accommodation(
    conn: &Connection,
    accommodation_id: &str,
) -> Result<Vec<RatingRow>, String> {
    let mut rows = conn
        .query(
            "SELECT r.accommodation_id, r.source, r.score, r.scale, r.review_count, r.checked_at, a.hotel_name \
             FROM domestic_accommodation_ratings r \
             JOIN domestic_accommodations a ON a.id = r.accommodation_id \
             WHERE r.accommodation_id = ?1 ORDER BY r.source",
            libsql::params![accommodation_id.to_string()],
        )
        .await
        .map_err(|e| format!("domestic_accommodation_ratings SELECT failed: {e}"))?;
    collect(&mut rows).await
}

async fn collect(rows: &mut libsql::Rows) -> Result<Vec<RatingRow>, String> {
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("domestic_accommodation_ratings row read failed: {e}"))?
    {
        out.push(RatingRow {
            accommodation_id: row.get(0).unwrap_or_default(),
            source: row.get(1).unwrap_or_default(),
            score: row.get(2).unwrap_or(0.0),
            scale: row.get(3).unwrap_or(0.0),
            review_count: row.get(4).ok(),
            checked_at: row.get(5).unwrap_or_default(),
            hotel_name: row.get(6).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// DELETE one source's rating. Returns affected rows (0 = no such pair).
pub async fn delete(
    conn: &Connection,
    accommodation_id: &str,
    source: &str,
) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM domestic_accommodation_ratings WHERE accommodation_id = ?1 AND source = ?2",
        libsql::params![accommodation_id.to_string(), source.to_string()],
    )
    .await
    .map_err(|e| format!("domestic_accommodation_ratings DELETE failed: {e}"))
}
