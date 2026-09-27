//! `route_place_geocodes` — the Nominatim result cache `snapshot-maps` reads
//! via `resolve_place`. Manual backfill lives here because Nominatim cannot
//! resolve every localized label (e.g. a Chinese place name in Japan); a
//! hand-verified coordinate entered through `set-place-geocode` must land in
//! the SAME row shape the renderer reads.

use libsql::Connection;

/// Upsert one manually verified coordinate under an existing query key. The
/// caller computes `query_key` with the exact `normalize_place`-based formula
/// `snapshot_maps::resolve_place` reads with, so the row is a guaranteed hit.
/// Replaces any previous entry (including a failure row) for that key.
pub async fn upsert_manual(
    conn: &Connection,
    query_key: &str,
    raw_place: &str,
    lat: f64,
    lon: f64,
    context: &str,
    fetched_at: &str,
) -> Result<u64, String> {
    conn.execute(
        "INSERT INTO route_place_geocodes \
         (query_key, raw_place, lat, lon, display_name, provider, confidence, review, fetched_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'manual', 'verified', 1, ?6) \
         ON CONFLICT(query_key) DO UPDATE SET \
         lat = excluded.lat, lon = excluded.lon, raw_place = excluded.raw_place, \
         display_name = excluded.display_name, provider = 'manual', \
         confidence = 'verified', failure_reason = NULL, \
         review = 1, fetched_at = excluded.fetched_at",
        libsql::params![
            query_key.to_string(),
            raw_place.to_string(),
            lat,
            lon,
            format!("{raw_place}, {context}"),
            fetched_at.to_string()
        ],
    )
    .await
    .map_err(|e| format!("route_place_geocodes manual upsert failed: {e}"))
}
