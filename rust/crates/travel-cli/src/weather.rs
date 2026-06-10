// `travel fetch-weather [--dest slug] [--all]` — port of
// src/cli/commands/weather.ts + src/services/weather-service.ts.
//
// Fetches a LIVE forecast from Open-Meteo (free, no API key) via curl — the
// TS service shells curl deliberately (Node's TCP stack does not resolve in
// some WSL2 environments), so we do the same with std::process::Command.
// Parsing the Open-Meteo JSON response is fine (external protocol); NO JSON is
// ever written to a DB column — the response maps to the typed days.weather_*
// columns:
//   weather_label, temp_low_c, temp_high_c, precipitation_pct, weather_code,
//   feels_like_low_c, feels_like_high_c, weather_source_id, weather_sourced_at
//
// DB write mirrors setDayWeather + saveWithTracking:
//   1. per-day UPDATE days.weather_* (+ updated_at)
//   2. per-day plan_events (dest_process + timeline, event=weather_updated)
//   3. operation_runs audit row
//   4. plans.version + 1

use libsql::Connection;
use serde_json::Value;
use std::process::Command;

const WMO_LABELS: &[(i64, &str)] = &[
    (0, "Clear sky"),
    (1, "Mainly clear"),
    (2, "Partly cloudy"),
    (3, "Overcast"),
    (45, "Foggy"),
    (48, "Depositing rime fog"),
    (51, "Light drizzle"),
    (53, "Moderate drizzle"),
    (55, "Dense drizzle"),
    (56, "Light freezing drizzle"),
    (57, "Dense freezing drizzle"),
    (61, "Slight rain"),
    (63, "Moderate rain"),
    (65, "Heavy rain"),
    (66, "Light freezing rain"),
    (67, "Heavy freezing rain"),
    (71, "Slight snowfall"),
    (73, "Moderate snowfall"),
    (75, "Heavy snowfall"),
    (77, "Snow grains"),
    (80, "Slight rain showers"),
    (81, "Moderate rain showers"),
    (82, "Violent rain showers"),
    (85, "Slight snow showers"),
    (86, "Heavy snow showers"),
    (95, "Thunderstorm"),
    (96, "Thunderstorm with slight hail"),
    (99, "Thunderstorm with heavy hail"),
];

fn wmo_label(code: i64) -> String {
    WMO_LABELS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| format!("WMO {code}"))
}

struct Forecast {
    date: String,
    temp_high_c: f64,
    temp_low_c: f64,
    feels_like_high_c: Option<f64>,
    feels_like_low_c: Option<f64>,
    precipitation_pct: Option<f64>,
    weather_code: i64,
    weather_label: String,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel fetch-weather [--dest <slug>] [--all]");
        return Ok(());
    }

    let mut dest_opt: Option<String> = None;
    let mut all = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                i += 1;
                dest_opt = args.get(i).cloned();
            }
            "--all" => all = true,
            _ => {}
        }
        i += 1;
    }

    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    if all {
        let destinations = list_destinations(&conn, &plan_id).await?;
        let mut total_updated = 0usize;
        for d in &destinations {
            match update_destination_weather(&conn, &plan_id, d).await {
                Ok(0) => println!("  {d}: outside 16-day window"),
                Ok(n) => {
                    println!("  {d}: updated {n} day(s)");
                    total_updated += n;
                }
                Err(e) => eprintln!("  {d}: failed — {e}"),
            }
        }
        if total_updated > 0 {
            finalize(&conn, &plan_id, "all destinations").await?;
        }
        return Ok(());
    }

    let dest = match &dest_opt {
        Some(d) => d.clone(),
        None => read_active_destination(&conn, &plan_id).await?,
    };

    let days = load_days(&conn, &plan_id, &dest).await?;
    if days.is_empty() {
        eprintln!("No itinerary days found. Run scaffold-itinerary first.");
        std::process::exit(1);
    }

    let first_date = days.first().map(|d| d.1.clone()).unwrap_or_default();
    let last_date = days.last().map(|d| d.1.clone()).unwrap_or_default();

    let forecasts = fetch_weather(&conn, &first_date, &last_date, &dest).await?;
    if forecasts.is_empty() {
        println!("No forecast data available (dates may be outside 16-day window).");
        return Ok(());
    }

    let sourced_at = now_rfc3339();
    let n = days.len().min(forecasts.len());
    for idx in 0..n {
        let (day_number, _date) = &days[idx];
        write_day_weather(&conn, &plan_id, &dest, *day_number, &forecasts[idx], &sourced_at).await?;
    }

    finalize(&conn, &plan_id, &dest).await?;

    println!("Weather updated for {} day(s) in {dest}:", forecasts.len());
    for idx in 0..forecasts.len().min(days.len()) {
        let f = &forecasts[idx];
        let feels_like = match (f.feels_like_low_c, f.feels_like_high_c) {
            (Some(lo), Some(hi)) => format!(" (體感 {}–{}°C)", fmt_temp(lo), fmt_temp(hi)),
            _ => String::new(),
        };
        let rain = f
            .precipitation_pct
            .map(|p| fmt_temp(p))
            .unwrap_or_else(|| "null".to_string());
        println!(
            "  Day {}: {} {}–{}°C{}, Rain: {}%",
            days[idx].0,
            f.weather_label,
            fmt_temp(f.temp_low_c),
            fmt_temp(f.temp_high_c),
            feels_like,
            rain
        );
    }

    Ok(())
}

/// Resolve, fetch, and write weather for one destination's full day range.
/// Returns the number of days updated (0 = outside forecast window / no days).
async fn update_destination_weather(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
) -> Result<usize, String> {
    let days = load_days(conn, plan_id, dest).await?;
    if days.is_empty() {
        return Err("no itinerary days".to_string());
    }
    let first_date = days.first().map(|d| d.1.clone()).unwrap_or_default();
    let last_date = days.last().map(|d| d.1.clone()).unwrap_or_default();
    let forecasts = fetch_weather(conn, &first_date, &last_date, dest).await?;
    if forecasts.is_empty() {
        return Ok(0);
    }
    let sourced_at = now_rfc3339();
    let n = days.len().min(forecasts.len());
    for idx in 0..n {
        write_day_weather(conn, plan_id, dest, days[idx].0, &forecasts[idx], &sourced_at).await?;
    }
    Ok(forecasts.len())
}

async fn list_destinations(conn: &Connection, plan_id: &str) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT slug FROM plan_destinations WHERE plan_id = ?1 ORDER BY slug",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_destinations query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_destinations row read failed: {e}"))?
    {
        out.push(row.get::<String>(0).unwrap_or_default());
    }
    Ok(out)
}

async fn read_active_destination(conn: &Connection, plan_id: &str) -> Result<String, String> {
    let mut rows = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_metadata row read failed: {e}"))?
    {
        let dest: String = row.get(0).unwrap_or_default();
        if !dest.is_empty() {
            return Ok(dest);
        }
    }
    Err(format!("plan_metadata.active_destination missing for plan_id={plan_id}"))
}

/// Load (day_number, date) ordered for a destination's days.
async fn load_days(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
) -> Result<Vec<(i64, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, date FROM days \
             WHERE plan_id = ?1 AND destination = ?2 ORDER BY day_number",
            libsql::params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("days query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days row read failed: {e}"))?
    {
        let n: i64 = row.get(0).unwrap_or_default();
        let d: String = row.get(1).unwrap_or_default();
        out.push((n, d));
    }
    Ok(out)
}

/// Look up coordinates + timezone from destination_config (fail loud — the TS
/// service throws when coordinates are missing).
async fn read_coords(conn: &Connection, dest: &str) -> Result<(f64, f64, String), String> {
    let mut rows = conn
        .query(
            "SELECT lat, lon, timezone FROM destination_config WHERE slug = ?1",
            libsql::params![dest.to_string()],
        )
        .await
        .map_err(|e| format!("destination_config query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("destination_config row read failed: {e}"))?
    {
        let lat: Option<f64> = row.get(0).unwrap_or(None);
        let lon: Option<f64> = row.get(1).unwrap_or(None);
        let tz: String = row.get(2).unwrap_or_default();
        let tz = if tz.is_empty() { "Asia/Tokyo".to_string() } else { tz };
        match (lat, lon) {
            (Some(la), Some(lo)) => Ok((la, lo, tz)),
            _ => Err(format!(
                "No coordinates configured for {dest}. Add lat/lon in Turso destination_config."
            )),
        }
    } else {
        Err(format!("Destination not found: {dest}"))
    }
}

/// Live fetch from Open-Meteo via curl. Returns empty vec if the start date is
/// beyond the 16-day forecast window (matching the TS service).
async fn fetch_weather(
    conn: &Connection,
    start_date: &str,
    end_date: &str,
    dest: &str,
) -> Result<Vec<Forecast>, String> {
    let (lat, lon, tz) = read_coords(conn, dest).await?;

    // 16-day window check (Open-Meteo limit), same arithmetic as the TS service.
    if let (Ok(start), Some(today)) = (
        chrono::NaiveDate::parse_from_str(start_date, "%Y-%m-%d"),
        Some(chrono::Utc::now().date_naive()),
    ) {
        let days_ahead = (start - today).num_days();
        if days_ahead > 16 {
            eprintln!(
                "  [weather] Dates {start_date}–{end_date} are {days_ahead} days ahead — outside 16-day forecast window. Skipping."
            );
            return Ok(Vec::new());
        }
    }

    let tz_enc = url_encode(&tz);
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
         &daily=temperature_2m_max,temperature_2m_min,apparent_temperature_max,apparent_temperature_min,precipitation_probability_max,weather_code\
         &timezone={tz_enc}&start_date={start_date}&end_date={end_date}"
    );

    let output = Command::new("curl")
        .arg("-sf")
        .arg(&url)
        .output()
        .map_err(|e| format!("Open-Meteo API request failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Open-Meteo API request failed: curl exit {:?}",
            output.status.code()
        ));
    }
    let body = String::from_utf8_lossy(&output.stdout);
    let data: Value =
        serde_json::from_str(&body).map_err(|e| format!("Open-Meteo response parse failed: {e}"))?;

    let daily = data
        .get("daily")
        .ok_or_else(|| "No daily forecast data in Open-Meteo response".to_string())?;
    let times = daily
        .get("time")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "No daily forecast data in Open-Meteo response".to_string())?;

    let arr = |key: &str| -> Vec<Value> {
        daily
            .get(key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let t_max = arr("temperature_2m_max");
    let t_min = arr("temperature_2m_min");
    let a_max = arr("apparent_temperature_max");
    let a_min = arr("apparent_temperature_min");
    let precip = arr("precipitation_probability_max");
    let codes = arr("weather_code");

    let mut out = Vec::new();
    for idx in 0..times.len() {
        let code = codes.get(idx).and_then(|v| v.as_i64()).unwrap_or(0);
        out.push(Forecast {
            date: times[idx].as_str().unwrap_or_default().to_string(),
            temp_high_c: t_max.get(idx).and_then(|v| v.as_f64()).unwrap_or(0.0),
            temp_low_c: t_min.get(idx).and_then(|v| v.as_f64()).unwrap_or(0.0),
            feels_like_high_c: a_max.get(idx).and_then(|v| v.as_f64()),
            feels_like_low_c: a_min.get(idx).and_then(|v| v.as_f64()),
            precipitation_pct: precip.get(idx).and_then(|v| v.as_f64()),
            weather_code: code,
            weather_label: wmo_label(code),
        });
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn write_day_weather(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    day_number: i64,
    f: &Forecast,
    sourced_at: &str,
) -> Result<(), String> {
    let now_db = now_db_datetime();
    conn.execute(
        "UPDATE days SET \
            weather_label = ?1, temp_low_c = ?2, temp_high_c = ?3, \
            feels_like_low_c = ?4, feels_like_high_c = ?5, precipitation_pct = ?6, \
            weather_code = ?7, weather_source_id = 'open_meteo', weather_sourced_at = ?8, \
            updated_at = ?9 \
         WHERE plan_id = ?10 AND destination = ?11 AND day_number = ?12",
        libsql::params![
            f.weather_label.clone(),
            f.temp_low_c,
            f.temp_high_c,
            f.feels_like_low_c,
            f.feels_like_high_c,
            f.precipitation_pct,
            f.weather_code,
            sourced_at.to_string(),
            now_db,
            plan_id.to_string(),
            dest.to_string(),
            day_number
        ],
    )
    .await
    .map_err(|e| format!("days weather UPDATE failed: {e}"))?;

    // Emit weather_updated event (dest_process + timeline), like setDayWeather.
    let now_iso = now_rfc3339();
    let dp_so = next_dest_process_sort_order(conn, plan_id, dest, "process_5_daily_itinerary").await?;
    let tl_so = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(conn, plan_id, "dest_process", dest, "process_5_daily_itinerary", dp_so, "weather_updated", &now_iso).await?;
    insert_kv(conn, plan_id, "dest_process", dest, "process_5_daily_itinerary", dp_so,
        &[("day_number", day_number.to_string()), ("source_id", "open_meteo".to_string())]).await?;
    insert_event(conn, plan_id, "timeline", "", "process_5_daily_itinerary", tl_so, "weather_updated", &now_iso).await?;
    insert_kv(conn, plan_id, "timeline", "", "process_5_daily_itinerary", tl_so,
        &[("day_number", day_number.to_string()), ("source_id", "open_meteo".to_string())]).await?;

    let _ = &f.date; // date is informational; days rows are matched by day_number
    Ok(())
}

/// Bump plans.version + write operation_runs audit (saveWithTracking analog).
async fn finalize(conn: &Connection, plan_id: &str, summary: &str) -> Result<(), String> {
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;
    let run_id = new_run_id();
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, 'fetch-weather', ?3, 'completed', ?4, ?5, ?6, ?6)",
        libsql::params![
            run_id,
            plan_id.to_string(),
            summary.to_string(),
            version_before,
            version_after,
            now_db.clone()
        ],
    )
    .await
    .map_err(|e| format!("operation_runs INSERT failed: {e}"))?;
    conn.execute(
        "UPDATE plans SET version = ?1, updated_at = ?2 WHERE plan_id = ?3",
        libsql::params![version_after, now_db, plan_id.to_string()],
    )
    .await
    .map_err(|e| format!("plans UPDATE failed: {e}"))?;
    Ok(())
}

async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT version FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
    {
        return Ok(row.get::<i64>(0).unwrap_or_default());
    }
    Err(format!("plans row missing for plan_id={plan_id}"))
}

async fn next_dest_process_sort_order(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'dest_process' \
               AND destination = ?2 AND process_id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX query failed: {e}"))?;
    if let Some(row) = rows.next().await.map_err(|e| format!("plan_events MAX read failed: {e}"))? {
        return Ok(row.get::<i64>(0).unwrap_or(-1) + 1);
    }
    Ok(0)
}

async fn next_timeline_sort_order(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'timeline'",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX(timeline) query failed: {e}"))?;
    if let Some(row) = rows.next().await.map_err(|e| format!("plan_events MAX(timeline) read failed: {e}"))? {
        return Ok(row.get::<i64>(0).unwrap_or(-1) + 1);
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    event: &str,
    event_at: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_events \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_events DELETE failed: {e}"))?;
    conn.execute(
        "INSERT INTO plan_events \
            (plan_id, scope, destination, process_id, sort_order, \
             event, event_at, from_state, to_state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, event.to_string(), event_at.to_string()],
    )
    .await
    .map_err(|e| format!("plan_events INSERT failed: {e}"))?;
    Ok(())
}

async fn insert_kv(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    kv: &[(&str, String)],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_event_data \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_event_data DELETE failed: {e}"))?;
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data \
                (plan_id, scope, destination, process_id, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, k.to_string(), v.clone()],
        )
        .await
        .map_err(|e| format!("plan_event_data INSERT failed: {e}"))?;
    }
    Ok(())
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Match JS number formatting: integers print without a decimal point.
fn fmt_temp(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v}");
        s
    }
}

fn new_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (n as u128);
    let p1 = (nanos & 0xFFFF_FFFF) as u32;
    let p2 = ((nanos >> 32) & 0xFFFF) as u16;
    let p3 = ((nanos >> 48) & 0x0FFF) as u16;
    let p4 = 0x8000 | (((nanos >> 60) & 0x3FFF) as u16);
    let p5 = (nanos as u64) ^ 0xDEAD_BEEF_CAFE_F00D;
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", p1, p2, p3, p4, p5)
}

fn now_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn now_db_datetime() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_known_and_unknown() {
        assert_eq!(wmo_label(0), "Clear sky");
        assert_eq!(wmo_label(61), "Slight rain");
        assert_eq!(wmo_label(123), "WMO 123");
    }

    #[test]
    fn encode_timezone() {
        assert_eq!(url_encode("Asia/Tokyo"), "Asia%2FTokyo");
    }

    #[test]
    fn temp_fmt() {
        assert_eq!(fmt_temp(10.0), "10");
        assert_eq!(fmt_temp(10.5), "10.5");
    }
}
