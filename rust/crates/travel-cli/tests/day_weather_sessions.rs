//! Real-Turso test for per-session weather: the `day_weather_sessions` repo
//! (replace_day upsert/delete, list ordering) and the `travel itinerary` day
//! header's `Sessions:` line. Fixtures are written through the repo (no network
//! fetch). zz_test_<nanos> ids; Guard armed right after the ids are bound;
//! skips cleanly without creds.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};
use std::process::Command;
use travel_db::repo::day_weather_sessions::{list, replace_day, SessionWeatherRow};

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
        .map_err(|e| format!("connect: {e}"))?;
    db.connect().map_err(|e| format!("open: {e}"))
}

fn row(day: i64, session: &str, code: i64, mm: f64, max: f64, prob: Option<i64>) -> SessionWeatherRow {
    SessionWeatherRow {
        day_number: day,
        session: session.into(),
        weather_code: code,
        precip_mm: mm,
        precip_max_mm_h: max,
        precip_prob_max: prob,
    }
}

#[tokio::test]
async fn day_weather_sessions_repo_and_itinerary_line() {
    let tag = nanos();
    let plan = format!("zz_test_{tag}_dws");
    let dest = format!("zz_test_{tag}_d");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping (no Turso creds): {e}");
            return;
        }
    };
    seed_plan(&plan, &dest, 0);
    db_exec(&format!(
        "INSERT OR REPLACE INTO days (plan_id, destination, day_number, date, theme, day_type, status, excursion, weather_label) \
         VALUES ('{plan}', '{dest}', 1, '2026-11-14', 'ZZ theme', 'full', 'confirmed', 0, 'Slight rain')"
    ))
    .expect("seed day");

    // Insert out of order; list() must return itinerary session order.
    let rows = vec![
        row(1, "evening", 3, 0.0, 0.0, Some(10)),
        row(1, "morning", 61, 2.0, 0.8, Some(70)),
        row(1, "noon", 63, 9.0, 5.0, None),
    ];
    replace_day(&conn, &plan, &dest, 1, &rows, "open_meteo", "2026-11-14T00:00:00Z").await.expect("replace");
    let got = list(&conn, &plan, &dest).await.expect("list");
    let keys: Vec<&str> = got.iter().map(|r| r.session.as_str()).collect();
    assert_eq!(keys, ["morning", "noon", "evening"]);
    assert_eq!(got[1].precip_prob_max, None);
    assert_eq!(got[0].precip_prob_max, Some(70));

    // Re-run replaces (no stale session, no duplicate PK error).
    replace_day(&conn, &plan, &dest, 1, &[row(1, "morning", 63, 4.5, 2.0, Some(80))], "open_meteo", "t2")
        .await
        .expect("replace again");
    let got = list(&conn, &plan, &dest).await.expect("list2");
    assert_eq!(got, vec![row(1, "morning", 63, 4.5, 2.0, Some(80))]);

    // itinerary day header shows the plain-text line.
    let out = Command::new(bin())
        .args(["itinerary", "--day", "1"])
        .env("TRAVEL_PLAN_ID", &plan)
        .output()
        .expect("run itinerary");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
    assert!(
        stdout.contains("Sessions: Morning Moderate rain 4.5mm (max 2mm/h)\n"),
        "got:\n{stdout}"
    );
}
