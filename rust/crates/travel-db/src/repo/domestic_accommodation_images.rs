//! `domestic_accommodation_images` — candidate gallery child rows.
//!
//! One row per photo of a `domestic_accommodations` stay (a room type, the lobby,
//! the sea view …). NORMALIZED on purpose: the dashboard needs several photos per
//! candidate and the alternative — a JSON array in an `image_url` column — is
//! banned project-wide. PK is `(accommodation_id, image_url)`, so re-adding the
//! same photo is a natural dedup rather than a duplicate row.

use libsql::Connection;

#[derive(Debug, Clone, PartialEq)]
pub struct DomesticAccommodationImageRow {
    pub accommodation_id: String,
    pub image_url: String,
    pub label: String,
    pub sort_order: i64,
    /// Denormalized for the CLI listing only (never written).
    pub hotel_name: String,
}

/// INSERT OR IGNORE one gallery photo. Returns affected rows: 1 = inserted,
/// 0 = this (accommodation_id, image_url) pair already exists (dedup, not an error).
pub async fn insert(
    conn: &Connection,
    accommodation_id: &str,
    image_url: &str,
    label: &str,
    sort_order: i64,
) -> Result<u64, String> {
    conn.execute(
        "INSERT OR IGNORE INTO domestic_accommodation_images \
         (accommodation_id, image_url, label, sort_order) VALUES (?1, ?2, ?3, ?4)",
        libsql::params![
            accommodation_id.to_string(),
            image_url.to_string(),
            label.to_string(),
            sort_order,
        ],
    )
    .await
    .map_err(|e| format!("domestic_accommodation_images INSERT failed: {e}"))
}

/// Next free `sort_order` for one accommodation (max + 1, 1-based when empty).
pub async fn next_sort_order(conn: &Connection, accommodation_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM domestic_accommodation_images \
             WHERE accommodation_id = ?1",
            libsql::params![accommodation_id.to_string()],
        )
        .await
        .map_err(|e| format!("domestic_accommodation_images MAX(sort_order) failed: {e}"))?;
    let next = match rows
        .next()
        .await
        .map_err(|e| format!("domestic_accommodation_images row read failed: {e}"))?
    {
        Some(r) => r.get::<i64>(0).unwrap_or(1),
        None => 1,
    };
    Ok(next)
}

/// True when a `domestic_accommodations` row with this id exists (fail-loud guard
/// for the CLI: a gallery photo must hang off a real stay).
pub async fn accommodation_exists(conn: &Connection, id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM domestic_accommodations WHERE id = ?1 LIMIT 1",
            libsql::params![id.to_string()],
        )
        .await
        .map_err(|e| format!("domestic_accommodations lookup failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("domestic_accommodations row read failed: {e}"))?
        .is_some())
}

/// Gallery rows for one destination, joined to the stay for the hotel name.
pub async fn list_by_destination(
    conn: &Connection,
    destination: &str,
) -> Result<Vec<DomesticAccommodationImageRow>, String> {
    let mut rows = conn
        .query(
            "SELECT g.accommodation_id, g.image_url, g.label, g.sort_order, a.hotel_name \
             FROM domestic_accommodation_images g \
             JOIN domestic_accommodations a ON a.id = g.accommodation_id \
             WHERE a.destination = ?1 \
             ORDER BY a.price_twd ASC, g.accommodation_id, g.sort_order",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("domestic_accommodation_images SELECT failed: {e}"))?;
    collect(&mut rows).await
}

/// Gallery rows for one accommodation id.
pub async fn list_by_accommodation(
    conn: &Connection,
    accommodation_id: &str,
) -> Result<Vec<DomesticAccommodationImageRow>, String> {
    let mut rows = conn
        .query(
            "SELECT g.accommodation_id, g.image_url, g.label, g.sort_order, a.hotel_name \
             FROM domestic_accommodation_images g \
             JOIN domestic_accommodations a ON a.id = g.accommodation_id \
             WHERE g.accommodation_id = ?1 ORDER BY g.sort_order",
            libsql::params![accommodation_id.to_string()],
        )
        .await
        .map_err(|e| format!("domestic_accommodation_images SELECT failed: {e}"))?;
    collect(&mut rows).await
}

async fn collect(
    rows: &mut libsql::Rows,
) -> Result<Vec<DomesticAccommodationImageRow>, String> {
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("domestic_accommodation_images row read failed: {e}"))?
    {
        out.push(DomesticAccommodationImageRow {
            accommodation_id: row.get(0).unwrap_or_default(),
            image_url: row.get(1).unwrap_or_default(),
            label: row.get(2).unwrap_or_default(),
            sort_order: row.get(3).unwrap_or(0),
            hotel_name: row.get(4).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// DELETE one photo. Returns affected rows (0 = no such pair).
pub async fn delete(
    conn: &Connection,
    accommodation_id: &str,
    image_url: &str,
) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM domestic_accommodation_images WHERE accommodation_id = ?1 AND image_url = ?2",
        libsql::params![accommodation_id.to_string(), image_url.to_string()],
    )
    .await
    .map_err(|e| format!("domestic_accommodation_images DELETE failed: {e}"))
}

/// DELETE every photo of one accommodation. Returns affected rows.
pub async fn delete_all_for(conn: &Connection, accommodation_id: &str) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM domestic_accommodation_images WHERE accommodation_id = ?1",
        libsql::params![accommodation_id.to_string()],
    )
    .await
    .map_err(|e| format!("domestic_accommodation_images DELETE failed: {e}"))
}
