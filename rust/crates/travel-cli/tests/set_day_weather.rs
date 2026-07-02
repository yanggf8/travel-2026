//! Deterministic real-Turso repo-level test for the migrated
//! travel_db::repo::itinerary::set_day_weather (the last inline SQL domain write
//! from weather.rs). Seeds a days row (plus parents), calls the repo fn with
//! FIXED forecast values (no network), asserts all 9 weather_* columns +
//! updated_at + weather_source_id='open_meteo' (literal).
//!
//! NOT an end-to-end CLI test (fetch-weather hits net + 16d window).
//! Credless skip; zztest{nanos} ids; panic-safe Guard teardown.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn is_credless(err: &str) -> bool {
    err.contains("turso auth login")
        || err.contains("Missing Turso")
        || err.contains("failed to connect to Turso")
        || err.contains("TRAVEL_TURSO")
        || err.contains("database URL")
}

fn exec_sql(sql: &str) {
    let _ = bin().args(["db", "exec", sql]).output();
}

async fn connect_write() -> Result<libsql::Connection, String> {
    let url = std::env::var("TRAVEL_TURSO_URL")
        .or_else(|_| std::env::var("TURSO_URL"))
        .map_err(|_| "TRAVEL_TURSO_URL not set".to_string())?;
    let token = std::env::var("TRAVEL_TURSO_WRITE_TOKEN")
        .or_else(|_| std::env::var("TURSO_TOKEN"))
        .map_err(|_| "TRAVEL_TURSO_WRITE_TOKEN not set".to_string())?;
    let db = libsql::Builder::new_remote(url, token)
        .build()
        .await
        .map_err(|e| format!("failed to connect to Turso: {e}"))?;
    db.connect()
        .map_err(|e| format!("failed to open Turso connection: {e}"))
}

#[tokio::test]
async fn set_day_weather_repo_deterministic() {
    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping set_day_weather test (no Turso creds): {}", e.trim());
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let plan = format!("zztest{}", nanos());
    let dest = format!("zztestdest{}", nanos());

    // pre-clean (best effort)
    let _ = conn
        .execute(
            "DELETE FROM days WHERE plan_id = ?1 AND destination = ?2",
            libsql::params![plan.clone(), dest.clone()],
        )
        .await;
    let _ = conn
        .execute(
            "DELETE FROM plan_metadata WHERE plan_id = ?1",
            libsql::params![plan.clone()],
        )
        .await;
    let _ = conn
        .execute("DELETE FROM plans WHERE plan_id = ?1", libsql::params![plan.clone()])
        .await;

    let _g = Guard::new({
        let plan = plan.clone();
        let dest = dest.clone();
        move || {
            exec_sql(&format!(
                "DELETE FROM days WHERE plan_id='{plan}' AND destination='{dest}'"
            ));
            exec_sql(&format!("DELETE FROM plan_metadata WHERE plan_id='{plan}'"));
            exec_sql(&format!("DELETE FROM plans WHERE plan_id='{plan}'"));
        }
    });

    // Seed minimal parent rows + one days row (weather cols start NULL/default)
    conn.execute(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES (?1, '4.2.0', 0)",
        libsql::params![plan.clone()],
    )
    .await
    .expect("seed plans");
    conn.execute(
        "INSERT INTO plan_metadata (plan_id, schema_version, active_destination) VALUES (?1, '4.2.0', ?2)",
        libsql::params![plan.clone(), dest.clone()],
    )
    .await
    .expect("seed plan_metadata");
    conn.execute(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) VALUES (?1, ?2, 1, '2026-07-03', 'full')",
        libsql::params![plan.clone(), dest.clone()],
    )
    .await
    .expect("seed days row");

    // FIXED deterministic inputs (no network, no chrono now inside test body)
    let label = "Clear sky";
    let temp_low_c = 22.0;
    let temp_high_c = 31.5;
    let feels_like_low_c: Option<f64> = Some(20.5);
    let feels_like_high_c: Option<f64> = Some(30.0);
    let precipitation_pct: Option<f64> = Some(10.0);
    let weather_code: i64 = 0;
    let sourced_at = "2026-07-02T10:00:00Z";
    let now_db = "2026-07-02 10:00:05";

    // Call the migrated repo fn
    travel_db::repo::itinerary::set_day_weather(
        &conn,
        &plan,
        &dest,
        1,
        label,
        temp_low_c,
        temp_high_c,
        feels_like_low_c,
        feels_like_high_c,
        precipitation_pct,
        weather_code,
        sourced_at,
        now_db,
    )
    .await
    .expect("set_day_weather");

    // Assert all 9 weather_* + updated_at; confirm literal 'open_meteo' and now_db bound for updated_at
    let mut rows = conn
        .query(
            "SELECT weather_label, temp_low_c, temp_high_c, feels_like_low_c, feels_like_high_c, \
                    precipitation_pct, weather_code, weather_source_id, weather_sourced_at, updated_at \
             FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan.clone(), dest.clone(), 1i64],
        )
        .await
        .expect("query after set");
    let row = rows
        .next()
        .await
        .expect("has row")
        .expect("row ok");

    let g_label: String = row.get(0).unwrap_or_default();
    assert_eq!(g_label, label);

    let g_low: f64 = row.get::<f64>(1).unwrap_or(0.0);
    assert_eq!(g_low, temp_low_c);

    let g_high: f64 = row.get::<f64>(2).unwrap_or(0.0);
    assert_eq!(g_high, temp_high_c);

    let g_fl: Option<f64> = row.get(3).unwrap_or(None);
    assert_eq!(g_fl, feels_like_low_c);

    let g_fh: Option<f64> = row.get(4).unwrap_or(None);
    assert_eq!(g_fh, feels_like_high_c);

    let g_precip: Option<f64> = row.get(5).unwrap_or(None);
    assert_eq!(g_precip, precipitation_pct);

    let g_code: i64 = row.get::<i64>(6).unwrap_or(0);
    assert_eq!(g_code, weather_code);

    let g_src: String = row.get(7).unwrap_or_default();
    assert_eq!(g_src, "open_meteo");

    let g_sourced: String = row.get(8).unwrap_or_default();
    assert_eq!(g_sourced, sourced_at);

    let g_updated: String = row.get(9).unwrap_or_default();
    assert_eq!(g_updated, now_db);
}
