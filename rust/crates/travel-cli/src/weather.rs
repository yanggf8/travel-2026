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

/// The ONE place the session windows live: (itinerary session key, first hour,
/// end hour exclusive), JST wall clock. An hour belongs to a window by its START
/// time (12:00 -> noon, 14:00 -> afternoon); hours outside 09-22 are ignored.
/// CAVEAT: Open-Meteo's hourly `precipitation` / `precipitation_probability` at
/// stamp T cover the PRECEDING hour (T-1 .. T), so those two use the stamps
/// from+1 ..= to (morning 10,11,12; noon 13,14; afternoon 15..18; evening 19..22),
/// while `weather_code` (instantaneous) uses the start-hour stamps from ..< to.
/// Keys match `timesofday.session_type` so the dashboard attaches each line to
/// the same itinerary session.
pub const SESSION_WINDOWS: [(&str, u32, u32); 4] = [
    ("morning", 9, 12),
    ("noon", 12, 14),
    ("afternoon", 14, 18),
    ("evening", 18, 22),
];

/// Request params for the hourly block (same request as the daily one).
const HOURLY_PARAMS: &str = "precipitation,precipitation_probability,weather_code";

/// One Open-Meteo hour (any field may be null).
#[derive(Debug, Clone, Copy, PartialEq)]
struct HourSample {
    hour: u32,
    precip_mm: Option<f64>,
    prob: Option<f64>,
    code: Option<i64>,
}

/// One aggregated session.
#[derive(Debug, Clone, PartialEq)]
struct SessionAgg {
    session: &'static str,
    weather_code: i64,
    precip_mm: f64,
    precip_max_mm_h: f64,
    precip_prob_max: Option<i64>,
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Representative WMO code of a session: the MOST FREQUENT code among its hours;
/// a tie goes to the more severe one (higher WMO number: 95 > 81 > 61 > 3 > 0).
/// Deliberately NOT the maximum: one stormy hour must not label a mostly-dry
/// session (the daily code is that worst-hour value, which is the very problem).
fn representative_code(codes: &[i64]) -> Option<i64> {
    let mut counts: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
    for c in codes {
        *counts.entry(*c).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(code, n)| (*n, *code))
        .map(|(code, _)| code)
}

/// Aggregate one day's hours into per-session rows (itinerary order). A window
/// with no usable hour (no weather code among them) yields no row. Precipitation
/// totals/maxima are rounded to 0.1 mm; the probability is the window's max.
fn aggregate_sessions(hours: &[HourSample]) -> Vec<SessionAgg> {
    let mut out = Vec::new();
    for (session, from, to) in SESSION_WINDOWS {
        let in_window: Vec<&HourSample> =
            hours.iter().filter(|h| h.hour >= from && h.hour < to).collect();
        // Preceding-hour quantities: stamps from+1 ..= to.
        let in_precip: Vec<&HourSample> =
            hours.iter().filter(|h| h.hour > from && h.hour <= to).collect();
        let codes: Vec<i64> = in_window.iter().filter_map(|h| h.code).collect();
        let Some(weather_code) = representative_code(&codes) else {
            continue;
        };
        let mm: Vec<f64> = in_precip.iter().filter_map(|h| h.precip_mm).collect();
        let prob = in_precip
            .iter()
            .filter_map(|h| h.prob)
            .fold(None, |m: Option<f64>, p| Some(m.map_or(p, |x| x.max(p))));
        out.push(SessionAgg {
            session,
            weather_code,
            precip_mm: round1(mm.iter().sum()),
            precip_max_mm_h: round1(mm.iter().cloned().fold(0.0, f64::max)),
            precip_prob_max: prob.map(|p| p.round() as i64),
        });
    }
    out
}

/// Group Open-Meteo's `hourly` block by local date ("2026-11-14T09:00" ->
/// ("2026-11-14", 9)). Unparseable timestamps are skipped.
fn group_hourly(data: &Value) -> std::collections::HashMap<String, Vec<HourSample>> {
    let mut by_date: std::collections::HashMap<String, Vec<HourSample>> = Default::default();
    let Some(h) = data.get("hourly") else {
        return by_date;
    };
    let arr = |key: &str| -> Vec<Value> {
        h.get(key).and_then(|v| v.as_array()).cloned().unwrap_or_default()
    };
    let times = arr("time");
    let (mm, prob, codes) = (arr("precipitation"), arr("precipitation_probability"), arr("weather_code"));
    for (i, t) in times.iter().enumerate() {
        let Some(ts) = t.as_str() else { continue };
        let Some((date, clock)) = ts.split_once('T') else { continue };
        let Some(hour) = clock.get(..2).and_then(|x| x.parse::<u32>().ok()) else { continue };
        by_date.entry(date.to_string()).or_default().push(HourSample {
            hour,
            precip_mm: mm.get(i).and_then(|v| v.as_f64()),
            prob: prob.get(i).and_then(|v| v.as_f64()),
            code: codes.get(i).and_then(|v| v.as_i64()),
        });
    }
    by_date
}

/// The Open-Meteo forecast URL: daily block (unchanged) + hourly block.
fn open_meteo_url(lat: f64, lon: f64, tz: &str, start_date: &str, end_date: &str) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
         &daily=temperature_2m_max,temperature_2m_min,apparent_temperature_max,apparent_temperature_min,precipitation_probability_max,weather_code\
         &hourly={HOURLY_PARAMS}\
         &timezone={}&start_date={start_date}&end_date={end_date}",
        url_encode(tz)
    )
}

pub(crate) fn wmo_label(code: i64) -> String {
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
    /// Hourly-derived per-session weather (empty when the hourly block is missing).
    sessions: Vec<SessionAgg>,
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
    let assigned = assign_forecasts(&days, &forecasts);
    for (day_number, f) in &assigned {
        write_day_weather(&conn, &plan_id, &dest, *day_number, f, &sourced_at).await?;
    }

    finalize(&conn, &plan_id, &dest).await?;

    println!("✅ Weather updated for {} day(s) in {dest}:", assigned.len());
    for (day_number, f) in &assigned {
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
            day_number,
            f.weather_label,
            fmt_temp(f.temp_low_c),
            fmt_temp(f.temp_high_c),
            feels_like,
            rain
        );
        for a in &f.sessions {
            println!("      {}: {} {}mm (max {}mm/h)", a.session, wmo_label(a.weather_code), a.precip_mm, a.precip_max_mm_h);
        }
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
    let assigned = assign_forecasts(&days, &forecasts);
    for (day_number, f) in &assigned {
        write_day_weather(conn, plan_id, dest, *day_number, f, &sourced_at).await?;
    }
    Ok(assigned.len())
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

/// Pair each forecast with the trip day whose date equals the forecast date.
/// Dates drive the match (never position): a day with no forecast is skipped.
fn assign_forecasts<'a>(days: &[(i64, String)], forecasts: &'a [Forecast]) -> Vec<(i64, &'a Forecast)> {
    days.iter()
        .filter_map(|(n, d)| forecasts.iter().find(|f| &f.date == d).map(|f| (*n, f)))
        .collect()
}

/// Load (day_number, date) ordered for a destination's TRIP days (variants excluded —
/// a variant shares its main day's date and keeps no weather of its own).
async fn load_days(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
) -> Result<Vec<(i64, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, date FROM days \
             WHERE plan_id = ?1 AND destination = ?2 AND variant_of IS NULL ORDER BY day_number",
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

/// Total attempts per fetch and the fixed pause between them. Retries are done
/// here, not by curl `--retry`: curl honours Retry-After (unbounded wait) and
/// keeps stdout already written by a failed attempt, which corrupts the JSON.
/// Worst case: 3 x 20 s (--max-time) + 2 x 2 s pauses = ~64 s.
const OPEN_METEO_MAX_ATTEMPTS: u32 = 3;
const OPEN_METEO_RETRY_DELAY_SECS: u64 = 2;

/// curl arguments for one Open-Meteo attempt (bounded connect and total time).
fn open_meteo_curl_args(url: &str) -> Vec<String> {
    ["-sf", "--connect-timeout", "10", "--max-time", "20", url]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Transient curl failures worth another attempt: 28 timeout, 7 cannot
/// connect, 35 TLS handshake, 22 HTTP error under `-f` (retried always).
fn is_transient_curl_exit(code: Option<i32>) -> bool {
    matches!(code, Some(28) | Some(7) | Some(35) | Some(22))
}

/// Final error after the attempts are exhausted (or a non-transient exit).
/// The fetch happens before any write for this destination, so nothing from
/// this fetch was saved; in an all-destinations run, earlier destinations may
/// already have been written.
fn open_meteo_failure_message(code: Option<i32>, attempts: u32) -> String {
    let tail = "Nothing was written for this fetch (in an all-destinations run, destinations finished earlier were already saved).";
    match code {
        Some(28) => format!(
            "Open-Meteo did not respond (curl exit 28: connection/TLS handshake timed out) after {attempts} attempt(s). The API host is intermittently unresponsive for a few minutes at a time - retry in ~5 minutes. {tail}"
        ),
        Some(7) => format!(
            "Could not connect to Open-Meteo (curl exit 7) after {attempts} attempt(s). Check network connectivity or retry in a few minutes. {tail}"
        ),
        Some(6) => format!(
            "Could not resolve the Open-Meteo host name (curl exit 6: DNS failure) after {attempts} attempt(s). Check DNS/network. {tail}"
        ),
        Some(22) => format!(
            "Open-Meteo returned an HTTP error (curl exit 22) after {attempts} attempt(s), e.g. 429 rate limit (the free tier allows limited calls per minute/hour/day) or a server error. Wait a few minutes and retry, and avoid repeated runs. {tail}"
        ),
        Some(c) => format!(
            "Open-Meteo API request failed: curl exit {c} after {attempts} attempt(s). {tail}"
        ),
        None => format!(
            "Open-Meteo API request failed: curl was terminated by a signal after {attempts} attempt(s). {tail}"
        ),
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

    let url = open_meteo_url(lat, lon, &tz, start_date, end_date);

    let mut attempt = 0;
    let output = loop {
        attempt += 1;
        let out = Command::new("curl")
            .args(open_meteo_curl_args(&url))
            .output()
            .map_err(|e| format!("Open-Meteo API request failed: {e}"))?;
        if out.status.success() {
            break out;
        }
        let code = out.status.code();
        if attempt < OPEN_METEO_MAX_ATTEMPTS && is_transient_curl_exit(code) {
            tokio::time::sleep(std::time::Duration::from_secs(OPEN_METEO_RETRY_DELAY_SECS)).await;
            continue;
        }
        return Err(open_meteo_failure_message(code, attempt));
    };
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

    let hourly = group_hourly(&data);
    let mut out = Vec::new();
    for idx in 0..times.len() {
        let date = times[idx].as_str().unwrap_or_default().to_string();
        let sessions = hourly.get(&date).map(|h| aggregate_sessions(h)).unwrap_or_default();
        let code = codes.get(idx).and_then(|v| v.as_i64()).unwrap_or(0);
        out.push(Forecast {
            date,
            temp_high_c: t_max.get(idx).and_then(|v| v.as_f64()).unwrap_or(0.0),
            temp_low_c: t_min.get(idx).and_then(|v| v.as_f64()).unwrap_or(0.0),
            feels_like_high_c: a_max.get(idx).and_then(|v| v.as_f64()),
            feels_like_low_c: a_min.get(idx).and_then(|v| v.as_f64()),
            precipitation_pct: precip.get(idx).and_then(|v| v.as_f64()),
            weather_code: code,
            weather_label: wmo_label(code),
            sessions,
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
    travel_db::repo::itinerary::set_day_weather(
        conn,
        plan_id,
        dest,
        day_number,
        &f.weather_label,
        f.temp_low_c,
        f.temp_high_c,
        f.feels_like_low_c,
        f.feels_like_high_c,
        f.precipitation_pct,
        f.weather_code,
        sourced_at,
        &now_db,
    )
    .await?;

    let rows: Vec<travel_db::repo::day_weather_sessions::SessionWeatherRow> = f
        .sessions
        .iter()
        .map(|a| travel_db::repo::day_weather_sessions::SessionWeatherRow {
            day_number,
            session: a.session.to_string(),
            weather_code: a.weather_code,
            precip_mm: a.precip_mm,
            precip_max_mm_h: a.precip_max_mm_h,
            precip_prob_max: a.precip_prob_max,
        })
        .collect();
    travel_db::repo::day_weather_sessions::replace_day(conn, plan_id, dest, day_number, &rows, "open_meteo", sourced_at)
        .await?;

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
/// Delegates the operation_runs INSERT + version bump to the shared
/// audit-triad helper so this path can't drift from the canonical one.
async fn finalize(conn: &Connection, plan_id: &str, summary: &str) -> Result<(), String> {
    let now_db = crate::cascade::common::now_db_datetime();
    let version_before = crate::cascade::common::read_version(conn, plan_id).await?;
    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "fetch-weather",
        summary,
        version_before,
        version_before + 1,
        &now_db,
    )
    .await?;
    Ok(())
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
    fn curl_args_have_timeouts_no_curl_retry_and_url_last() {
        let a = open_meteo_curl_args("https://x.test/f?a=1");
        for (flag, val) in [("--connect-timeout", "10"), ("--max-time", "20")] {
            let i = a.iter().position(|s| s == flag).unwrap_or_else(|| panic!("missing {flag}"));
            assert_eq!(a[i + 1], val, "value of {flag}");
        }
        assert!(!a.iter().any(|s| s.starts_with("--http")), "HTTP version must not be forced");
        assert!(!a.iter().any(|s| s.starts_with("--retry")), "curl must not retry itself");
        assert_eq!(a.last().unwrap(), "https://x.test/f?a=1");
    }

    #[test]
    fn failure_message_per_exit_code() {
        let m = open_meteo_failure_message(Some(28), 3);
        assert!(m.contains("did not respond") && m.contains("exit 28") && m.contains("3 attempt"));
        assert!(m.contains("retry in ~5 minutes") && m.contains("Nothing was written"));
        assert!(open_meteo_failure_message(Some(22), 3).contains("429"));
        assert!(open_meteo_failure_message(Some(22), 3).contains("free tier"));
        assert!(open_meteo_failure_message(Some(7), 3).contains("Could not connect"));
        assert!(open_meteo_failure_message(Some(6), 1).contains("DNS"));
        let o = open_meteo_failure_message(Some(56), 1);
        assert!(o.contains("curl exit 56 ") && o.contains("Nothing was written"));
        assert!(open_meteo_failure_message(None, 1).contains("signal"));
    }

    #[test]
    fn transient_curl_exits_retry_others_do_not() {
        for c in [28, 7, 35, 22] {
            assert!(is_transient_curl_exit(Some(c)), "exit {c} should retry");
        }
        for c in [6, 1, 3, 0] {
            assert!(!is_transient_curl_exit(Some(c)), "exit {c} should not retry");
        }
        assert!(!is_transient_curl_exit(None));
    }

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

    fn fc(date: &str) -> Forecast {
        Forecast {
            date: date.to_string(),
            temp_high_c: 20.0,
            temp_low_c: 10.0,
            feels_like_high_c: None,
            feels_like_low_c: None,
            precipitation_pct: None,
            weather_code: 0,
            weather_label: "Clear sky".to_string(),
            sessions: Vec::new(),
        }
    }

    #[test]
    fn weather_matches_by_date_with_variant_of_non_final_day() {
        // trip days 1,2,4 (day 3 is a variant, filtered out upstream) on consecutive dates
        let days = vec![
            (1, "2026-12-01".to_string()),
            (2, "2026-12-02".to_string()),
            (4, "2026-12-03".to_string()),
        ];
        let f = vec![fc("2026-12-01"), fc("2026-12-02"), fc("2026-12-03")];
        let got: Vec<(i64, String)> =
            assign_forecasts(&days, &f).into_iter().map(|(n, x)| (n, x.date.clone())).collect();
        assert_eq!(got.len(), 3);
        assert_eq!(got[2], (4, "2026-12-03".to_string()));
    }

    #[test]
    fn assign_forecasts_skips_days_without_forecast() {
        let days = vec![(1, "2026-12-01".to_string()), (2, "2026-12-09".to_string())];
        let f = vec![fc("2026-12-01")];
        let got = assign_forecasts(&days, &f);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, 1);
    }

    fn hs(hour: u32, mm: Option<f64>, prob: Option<f64>, code: Option<i64>) -> HourSample {
        HourSample { hour, precip_mm: mm, prob, code }
    }

    #[test]
    fn session_windows_cover_09_to_22_without_gaps_or_overlap() {
        assert_eq!(SESSION_WINDOWS[0], ("morning", 9, 12));
        for w in SESSION_WINDOWS.windows(2) {
            assert_eq!(w[0].2, w[1].1, "windows must be contiguous");
        }
        assert_eq!(SESSION_WINDOWS[3].2, 22);
    }

    #[test]
    fn code_uses_start_hour_stamps_precip_uses_preceding_hour_stamps() {
        // mm and prob at stamp h both equal h, so the sums show exactly which stamps count.
        let hours: Vec<HourSample> = (0..24).map(|h| hs(h, Some(h as f64), Some(h as f64), Some(61))).collect();
        let a = aggregate_sessions(&hours);
        let by = |k: &str| a.iter().find(|s| s.session == k).unwrap().clone();
        assert_eq!(a.len(), 4);
        assert_eq!(by("morning").precip_mm, 33.0); // stamps 10,11,12
        assert_eq!(by("noon").precip_mm, 27.0); // 13,14
        assert_eq!(by("afternoon").precip_mm, 66.0); // 15..18
        assert_eq!(by("evening").precip_mm, 82.0); // 19..22; 23 and 0-9 never count
        assert_eq!(by("morning").precip_max_mm_h, 12.0);
        assert_eq!(by("evening").precip_prob_max, Some(22));
        assert_eq!(by("noon").precip_prob_max, Some(14));
        // Code stamps: a dry-start window keeps its own code even when rain stamps differ.
        let mut h2: Vec<HourSample> = (9..13).map(|h| hs(h, Some(0.0), Some(0.0), Some(3))).collect();
        h2[3].code = Some(65); // stamp 12 belongs to noon's code window, not morning's
        assert_eq!(aggregate_sessions(&h2)[0].weather_code, 3);
    }

    #[test]
    fn aggregation_empty_hours_or_missing_codes_yield_no_row() {
        assert!(aggregate_sessions(&[]).is_empty());
        // only morning has data
        let a = aggregate_sessions(&[hs(10, Some(0.0), None, Some(3))]);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].session, "morning");
        assert_eq!(a[0].precip_prob_max, None);
        // hour present but null code -> no row
        assert!(aggregate_sessions(&[hs(10, Some(1.0), Some(50.0), None)]).is_empty());
    }

    #[test]
    fn representative_code_is_most_frequent_ties_to_severe_never_plain_max() {
        assert_eq!(representative_code(&[61, 61, 61, 65]), Some(61)); // not the max
        assert_eq!(representative_code(&[3, 63, 3, 63]), Some(63)); // tie -> severe
        assert_eq!(representative_code(&[0]), Some(0));
        assert_eq!(representative_code(&[]), None);
    }

    #[test]
    fn aggregation_rounds_mm_to_one_decimal_and_reports_max_hour() {
        let hours = [
            hs(9, Some(50.0), Some(99.0), Some(61)), // stamp 9 covers 08-09: not morning precip
            hs(10, Some(0.04), None, Some(61)),
            hs(11, Some(1.26), Some(80.4), Some(61)),
            hs(12, Some(0.1), Some(90.5), Some(61)),
        ];
        let a = aggregate_sessions(&hours);
        assert_eq!(a[0].precip_mm, 1.4); // 1.40 -> 1.4
        assert_eq!(a[0].precip_max_mm_h, 1.3);
        assert_eq!(a[0].precip_prob_max, Some(91)); // 90.5 rounds up
    }

    #[test]
    fn hourly_grouping_splits_by_local_date() {
        let data: Value = serde_json::json!({"hourly": {
            "time": ["2026-11-14T09:00", "2026-11-14T10:00", "2026-11-15T09:00"],
            "precipitation": [1.0, null, 2.0],
            "precipitation_probability": [10, 20, 30],
            "weather_code": [61, 61, 3]
        }});
        let g = group_hourly(&data);
        assert_eq!(g["2026-11-14"].len(), 2);
        assert_eq!(g["2026-11-14"][1].precip_mm, None);
        assert_eq!(g["2026-11-15"][0].hour, 9);
        assert!(group_hourly(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn url_requests_daily_and_hourly_in_one_call() {
        let u = open_meteo_url(35.0, 135.7, "Asia/Tokyo", "2026-11-14", "2026-11-17");
        assert!(u.contains("&daily=temperature_2m_max,"), "{u}");
        assert!(u.contains("&hourly=precipitation,precipitation_probability,weather_code"), "{u}");
        assert!(u.contains("&timezone=Asia%2FTokyo&start_date=2026-11-14&end_date=2026-11-17"), "{u}");
    }

    #[test]
    fn temp_fmt() {
        assert_eq!(fmt_temp(10.0), "10");
        assert_eq!(fmt_temp(10.5), "10.5");
    }
}
