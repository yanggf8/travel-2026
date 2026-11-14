//! Per-session weather (`day_weather_sessions`): one row per
//! (plan, destination, day, session) with the session's representative WMO code,
//! total precipitation, peak hourly precipitation and peak precipitation
//! probability. Written ONLY by `travel fetch-weather` (inside its audit run, so
//! no separate triad here); read by `travel itinerary` and the dashboard worker.
//! `session` uses the itinerary's own keys: morning | noon | afternoon | evening.

use libsql::Connection;

/// The itinerary's session keys — the only values `session` may hold (enforced
/// here, not by a DB constraint).
pub const SESSION_KEYS: [&str; 4] = ["morning", "noon", "afternoon", "evening"];

pub fn is_session_key(s: &str) -> bool {
    SESSION_KEYS.contains(&s)
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionWeatherRow {
    pub day_number: i64,
    pub session: String,
    pub weather_code: i64,
    pub precip_mm: f64,
    pub precip_max_mm_h: f64,
    pub precip_prob_max: Option<i64>,
}

/// Replace one day's session rows: delete the day's old rows (a session that
/// lost its data must not keep a stale forecast), then upsert each given row.
pub async fn replace_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day_number: i64,
    rows: &[SessionWeatherRow],
    source: &str,
    fetched_at: &str,
) -> Result<(), String> {
    if let Some(bad) = rows.iter().find(|r| !is_session_key(&r.session)) {
        return Err(format!("day_weather_sessions: unknown session key '{}'", bad.session));
    }
    conn.execute(
        "DELETE FROM day_weather_sessions WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
        libsql::params![plan_id.to_string(), destination.to_string(), day_number],
    )
    .await
    .map_err(|e| format!("day_weather_sessions DELETE failed: {e}"))?;
    for r in rows {
        conn.execute(
            "INSERT INTO day_weather_sessions \
                (plan_id, destination, day_number, session, weather_code, precip_mm, \
                 precip_max_mm_h, precip_prob_max, source, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(plan_id, destination, day_number, session) DO UPDATE SET \
                weather_code = excluded.weather_code, precip_mm = excluded.precip_mm, \
                precip_max_mm_h = excluded.precip_max_mm_h, \
                precip_prob_max = excluded.precip_prob_max, \
                source = excluded.source, fetched_at = excluded.fetched_at",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day_number,
                r.session.clone(),
                r.weather_code,
                r.precip_mm,
                r.precip_max_mm_h,
                r.precip_prob_max,
                source.to_string(),
                fetched_at.to_string()
            ],
        )
        .await
        .map_err(|e| format!("day_weather_sessions upsert failed: {e}"))?;
    }
    Ok(())
}

/// All session rows of a destination's days, ordered by day then the itinerary's
/// session order (morning, noon, afternoon, evening).
pub async fn list(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<SessionWeatherRow>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, session, weather_code, precip_mm, precip_max_mm_h, precip_prob_max \
             FROM day_weather_sessions WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number, CASE session WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 \
                                               WHEN 'afternoon' THEN 2 ELSE 3 END",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("day_weather_sessions query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("day_weather_sessions row failed: {e}"))?
    {
        out.push(SessionWeatherRow {
            day_number: r.get(0).unwrap_or(0),
            session: r.get(1).unwrap_or_default(),
            weather_code: r.get(2).unwrap_or(0),
            precip_mm: r.get::<f64>(3).unwrap_or(0.0),
            precip_max_mm_h: r.get::<f64>(4).unwrap_or(0.0),
            precip_prob_max: r.get::<Option<i64>>(5).unwrap_or(None),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_itinerary_session_keys_are_valid() {
        for k in ["morning", "noon", "afternoon", "evening"] {
            assert!(is_session_key(k), "{k}");
        }
        for k in ["", "night", "Morning", "dawn"] {
            assert!(!is_session_key(k), "{k}");
        }
    }
}
