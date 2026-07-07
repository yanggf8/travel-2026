//! Destination-reference reads (areas/POIs/clusters/transit/tips/airports) — all keyed on `slug`
//! with a bound `?1` param (replaces `destination_ref.rs`'s `sql_quote()` interpolation). The DAL
//! owns the SQL + row mapping; the CLI renders the returned rows.

use libsql::Connection;
use std::collections::BTreeMap;

/// `(display_name, timezone, currency)` from `destination_config`; `None` if the slug is unknown.
pub async fn config_scalars(
    conn: &Connection,
    slug: &str,
) -> Result<Option<(String, String, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT display_name, timezone, currency FROM destination_config WHERE slug = ?1",
            libsql::params![slug.to_string()],
        )
        .await
        .map_err(|e| format!("failed to query destination_config: {e}"))?;
    let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read destination_config: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some((
        r.get(0).unwrap_or_default(),
        r.get(1).unwrap_or_default(),
        r.get(2).unwrap_or_default(),
    )))
}

#[derive(Debug, Clone)]
pub struct AreaRow {
    pub area_id: String,
    pub name: String,
    pub typ: String,
    pub vibe: String,
}

pub async fn areas(conn: &Connection, slug: &str) -> Result<Vec<AreaRow>, String> {
    let mut rows = query_slug(
        conn,
        "SELECT area_id, name, type, vibe FROM destination_areas WHERE slug = ?1 ORDER BY area_id",
        slug,
    )
    .await
    .map_err(|e| format!("failed to query destination_areas: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("area row: {e}"))? {
        out.push(AreaRow {
            area_id: r.get(0).unwrap_or_default(),
            name: r.get(1).unwrap_or_default(),
            typ: r.get(2).unwrap_or_default(),
            vibe: r.get(3).unwrap_or_default(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct PoiRow {
    pub title: String,
    pub area: String,
    pub nearest_station: String,
    pub duration_min: Option<i64>,
    pub booking_required: bool,
    pub booking_url: Option<String>,
    pub cost_estimate: Option<i64>,
    pub notes: Option<String>,
    pub hours: Option<String>,
}

pub async fn pois(conn: &Connection, slug: &str) -> Result<Vec<PoiRow>, String> {
    let mut rows = query_slug(
        conn,
        "SELECT poi_id, title, area, nearest_station, duration_min, booking_required, booking_url, \
         cost_estimate, notes, hours FROM destination_pois WHERE slug = ?1 ORDER BY poi_id",
        slug,
    )
    .await
    .map_err(|e| format!("failed to query destination_pois: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("poi row: {e}"))? {
        out.push(PoiRow {
            title: r.get(1).unwrap_or_default(),
            area: r.get(2).unwrap_or_default(),
            nearest_station: r.get(3).unwrap_or_default(),
            duration_min: r.get(4).ok(),
            booking_required: r.get::<i64>(5).unwrap_or(0) == 1,
            booking_url: opt_string(&r, 6),
            cost_estimate: r.get(7).ok(),
            notes: opt_string(&r, 8),
            hours: opt_string(&r, 9),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct ClusterRow {
    pub cluster_id: String,
    pub name: String,
    pub description: String,
    pub duration_min: Option<i64>,
    pub best_area: Option<String>,
}

pub async fn clusters(conn: &Connection, slug: &str) -> Result<Vec<ClusterRow>, String> {
    let mut rows = query_slug(
        conn,
        "SELECT cluster_id, name, description, duration_min, best_area \
         FROM destination_clusters WHERE slug = ?1 ORDER BY cluster_id",
        slug,
    )
    .await
    .map_err(|e| format!("failed to query destination_clusters: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("cluster row: {e}"))? {
        out.push(ClusterRow {
            cluster_id: r.get(0).unwrap_or_default(),
            name: r.get(1).unwrap_or_default(),
            description: r.get(2).unwrap_or_default(),
            duration_min: r.get(3).ok(),
            best_area: opt_string(&r, 4),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct TransitRow {
    pub pair_key: String,
    pub kind: String,
    pub minutes: i64,
    pub line: String,
    pub station_from: Option<String>,
    pub station_to: Option<String>,
}

pub async fn transit(conn: &Connection, slug: &str) -> Result<Vec<TransitRow>, String> {
    let mut rows = query_slug(
        conn,
        "SELECT pair_key, kind, minutes, line, station_from, station_to \
         FROM destination_transit WHERE slug = ?1 ORDER BY kind, pair_key",
        slug,
    )
    .await
    .map_err(|e| format!("failed to query destination_transit: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("transit row: {e}"))? {
        out.push(TransitRow {
            pair_key: r.get(0).unwrap_or_default(),
            kind: r.get(1).unwrap_or_default(),
            minutes: r.get(2).unwrap_or(0),
            line: r.get(3).unwrap_or_default(),
            station_from: opt_string(&r, 4),
            station_to: opt_string(&r, 5),
        });
    }
    Ok(out)
}

/// `area_id → [station]` from `destination_area_stations` (ordered by area_id, sort_order).
pub async fn stations_by_area(
    conn: &Connection,
    slug: &str,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    grouped_pairs(
        conn,
        "SELECT area_id, station FROM destination_area_stations WHERE slug = ?1 \
         ORDER BY area_id, sort_order",
        slug,
    )
    .await
}

/// `area_id → [tag]` from `destination_area_best_for`.
pub async fn best_for_by_area(
    conn: &Connection,
    slug: &str,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    grouped_pairs(
        conn,
        "SELECT area_id, tag FROM destination_area_best_for WHERE slug = ?1 \
         ORDER BY area_id, sort_order",
        slug,
    )
    .await
}

/// `cluster_id → [poi_id]` from `destination_cluster_pois`.
pub async fn pois_by_cluster(
    conn: &Connection,
    slug: &str,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    grouped_pairs(
        conn,
        "SELECT cluster_id, poi_id FROM destination_cluster_pois WHERE slug = ?1 \
         ORDER BY cluster_id, sort_order",
        slug,
    )
    .await
}

/// Tips for `slug`, ordered by sort_order.
pub async fn tips(conn: &Connection, slug: &str) -> Result<Vec<String>, String> {
    flat_list(
        conn,
        "SELECT tip FROM destination_tips WHERE slug = ?1 ORDER BY sort_order",
        slug,
    )
    .await
}

/// Airports for `slug`, ordered by sort_order.
pub async fn airports(conn: &Connection, slug: &str) -> Result<Vec<String>, String> {
    flat_list(
        conn,
        "SELECT airport FROM destination_airports WHERE slug = ?1 ORDER BY sort_order",
        slug,
    )
    .await
}

async fn query_slug(conn: &Connection, sql: &str, slug: &str) -> Result<libsql::Rows, libsql::Error> {
    conn.query(sql, libsql::params![slug.to_string()]).await
}

async fn grouped_pairs(
    conn: &Connection,
    sql: &str,
    slug: &str,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut rows = query_slug(conn, sql, slug)
        .await
        .map_err(|e| format!("query: {e}"))?;
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("row: {e}"))? {
        let k: String = r.get(0).unwrap_or_default();
        let v: String = r.get(1).unwrap_or_default();
        m.entry(k).or_default().push(v);
    }
    Ok(m)
}

async fn flat_list(conn: &Connection, sql: &str, slug: &str) -> Result<Vec<String>, String> {
    let mut rows = query_slug(conn, sql, slug)
        .await
        .map_err(|e| format!("query: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("row: {e}"))? {
        out.push(r.get::<String>(0).unwrap_or_default());
    }
    Ok(out)
}

fn opt_string(row: &libsql::Row, idx: i32) -> Option<String> {
    match row.get_value(idx).ok()? {
        libsql::Value::Text(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// Existence check for `(slug, poi_id)` — used by `set-poi-coords` to fail
/// loud before attempting the UPDATE.
pub async fn poi_coords_exists(conn: &Connection, slug: &str, poi_id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM destination_pois WHERE slug = ?1 AND poi_id = ?2",
            libsql::params![slug.to_string(), poi_id.to_string()],
        )
        .await
        .map_err(|e| format!("destination_pois existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("destination_pois existence row read failed: {e}"))?
        .is_some())
}

/// `set-poi-coords` write — sets lat/lon and (optionally, via COALESCE) refreshes
/// `source_url`/`confidence` for one `(slug, poi_id)` row. Destination-ref data is
/// slug-keyed GLOBAL data — NO audit triad / NO plans.version (mirrors the rest
/// of this module). Caller must check the returned affected-row count == 1 and
/// fail loud otherwise.
pub async fn set_poi_coords(
    conn: &Connection,
    slug: &str,
    poi_id: &str,
    lat: f64,
    lon: f64,
    source: Option<&str>,
    confidence: Option<&str>,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE destination_pois \
         SET lat = ?1, \
             lon = ?2, \
             source_url = COALESCE(?3, source_url), \
             confidence = COALESCE(?4, confidence) \
         WHERE slug = ?5 AND poi_id = ?6",
        libsql::params![lat, lon, source, confidence, slug.to_string(), poi_id.to_string()],
    )
    .await
    .map_err(|e| format!("destination_pois coords UPDATE failed: {e}"))
}

/// Upsert one `destination_transit` row (slug-keyed reference data, no audit
/// triad — mirrors the rest of this module). The caller computes `pair_key` via
/// `transit_key::primary_pair_key` (the SAME normalization `derive-routes`
/// looks up by), and passes the ORIGINAL display strings for `station_from` /
/// `station_to`. INSERT OR REPLACE on the (slug, pair_key) PK makes it
/// idempotent. Returns the affected-row count (caller asserts == 1).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_transit(
    conn: &Connection,
    slug: &str,
    pair_key: &str,
    kind: &str,
    minutes: i64,
    line: &str,
    station_from: &str,
    station_to: &str,
    source: Option<&str>,
    confidence: &str,
    fetched_at: &str,
) -> Result<u64, String> {
    conn.execute(
        "INSERT OR REPLACE INTO destination_transit \
         (slug, pair_key, kind, minutes, line, station_from, station_to, source_url, fetched_at, confidence) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        libsql::params![
            slug.to_string(),
            pair_key.to_string(),
            kind.to_string(),
            minutes,
            line.to_string(),
            station_from.to_string(),
            station_to.to_string(),
            source,
            fetched_at.to_string(),
            confidence.to_string(),
        ],
    )
    .await
    .map_err(|e| format!("destination_transit INSERT OR REPLACE failed: {e}"))
}
