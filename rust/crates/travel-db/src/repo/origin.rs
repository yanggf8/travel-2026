use libsql::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultOrigin {
    pub slug: String,
    pub airport: String,
    pub currency: String,
}

/// Read the default origin slug, its primary airport, and currency from seeded config tables.
pub async fn default_origin_airport_and_currency(
    conn: &Connection,
) -> Result<DefaultOrigin, String> {
    let mut rows = conn
        .query(
            "SELECT value FROM global_config WHERE key = ?1",
            libsql::params!["default_origin".to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Err("missing global_config row for key=default_origin".to_string());
    };
    let slug: String = row.get(0).map_err(|e| e.to_string())?;

    let mut rows = conn
        .query(
            "SELECT currency FROM origin_config WHERE slug = ?1",
            libsql::params![slug.clone()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Err(format!("missing origin_config row for slug={slug}"));
    };
    let currency: String = row.get(0).map_err(|e| e.to_string())?;

    let mut rows = conn
        .query(
            "SELECT airport FROM origin_airports WHERE slug = ?1 ORDER BY sort_order LIMIT 1",
            libsql::params![slug.clone()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Err(format!("missing origin_airports row for slug={slug}"));
    };
    let airport: String = row.get(0).map_err(|e| e.to_string())?;

    Ok(DefaultOrigin {
        slug,
        airport,
        currency,
    })
}