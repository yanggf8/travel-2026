//! Read model for the `validate data` activity-geo / via / requested_by lints
//! (spec 2026-11-20 §7.3, §5). Read-only; one SELECT per scan.

use libsql::{params, Connection};

/// One activity that carries a pin, a requester or a 「指定」-style title.
#[derive(Debug, Clone)]
pub struct ActivityGeoRow {
    pub plan_id: String,
    pub destination: String,
    pub day: i64,
    pub session: String,
    pub title: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub requested_by: Option<String>,
    /// `destination_config.lat/lon`; None when the destination or either column is missing.
    pub hub: Option<(f64, f64)>,
}

/// One stored via point with its parent-segment existence and the hub.
#[derive(Debug, Clone)]
pub struct ViaGeoRow {
    pub plan_id: String,
    pub destination: String,
    pub day: i64,
    pub sort_order: i64,
    pub via_order: i64,
    pub lat: f64,
    pub lon: f64,
    pub has_segment: bool,
    pub hub: Option<(f64, f64)>,
}

fn pair(a: Option<f64>, b: Option<f64>) -> Option<(f64, f64)> {
    a.zip(b)
}

/// Activities with any pin column set, a requested_by, or a title that may hold
/// the requester keyword (`title_keyword`, matched with LIKE).
pub async fn scan_activities(conn: &Connection, title_keyword: &str) -> Result<Vec<ActivityGeoRow>, String> {
    let mut rows = conn
        .query(
            "SELECT a.plan_id, a.destination, a.day_number, a.session_type, a.title, a.lat, a.lon, \
                    a.requested_by, d.lat, d.lon \
             FROM activities a LEFT JOIN destination_config d ON d.slug = a.destination \
             WHERE a.lat IS NOT NULL OR a.lon IS NOT NULL OR a.requested_by IS NOT NULL \
                OR instr(a.title, ?1) > 0 \
             ORDER BY a.plan_id, a.day_number, a.sort_order, a.id",
            params![title_keyword.to_string()],
        )
        .await
        .map_err(|e| format!("activity geo scan failed: {e}"))?;
    let col = |e: libsql::Error| format!("activity geo scan column failed: {e}");
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("activity geo scan row failed: {e}"))? {
        out.push(ActivityGeoRow {
            plan_id: r.get(0).map_err(col)?,
            destination: r.get(1).map_err(col)?,
            day: r.get(2).map_err(col)?,
            session: r.get(3).map_err(col)?,
            title: r.get(4).map_err(col)?,
            lat: r.get::<Option<f64>>(5).map_err(col)?,
            lon: r.get::<Option<f64>>(6).map_err(col)?,
            requested_by: r.get::<Option<String>>(7).map_err(col)?,
            hub: pair(r.get::<Option<f64>>(8).map_err(col)?, r.get::<Option<f64>>(9).map_err(col)?),
        });
    }
    Ok(out)
}

/// Every stored via point.
pub async fn scan_via(conn: &Connection) -> Result<Vec<ViaGeoRow>, String> {
    let mut rows = conn
        .query(
            "SELECT v.plan_id, v.destination, v.day_number, v.sort_order, v.via_order, v.lat, v.lon, \
                    EXISTS (SELECT 1 FROM day_route_segments s WHERE s.plan_id = v.plan_id \
                            AND s.destination = v.destination AND s.day_number = v.day_number \
                            AND s.sort_order = v.sort_order) AS has_seg, \
                    d.lat, d.lon \
             FROM day_route_segment_via v LEFT JOIN destination_config d ON d.slug = v.destination \
             ORDER BY v.plan_id, v.day_number, v.sort_order, v.via_order",
            params![],
        )
        .await
        .map_err(|e| format!("via geo scan failed: {e}"))?;
    let col = |e: libsql::Error| format!("via geo scan column failed: {e}");
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("via geo scan row failed: {e}"))? {
        out.push(ViaGeoRow {
            plan_id: r.get(0).map_err(col)?,
            destination: r.get(1).map_err(col)?,
            day: r.get(2).map_err(col)?,
            sort_order: r.get(3).map_err(col)?,
            via_order: r.get(4).map_err(col)?,
            lat: r.get(5).map_err(col)?,
            lon: r.get(6).map_err(col)?,
            has_segment: r.get::<i64>(7).map_err(col)? != 0,
            hub: pair(r.get::<Option<f64>>(8).map_err(col)?, r.get::<Option<f64>>(9).map_err(col)?),
        });
    }
    Ok(out)
}
