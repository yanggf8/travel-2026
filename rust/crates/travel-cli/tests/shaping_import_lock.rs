//! Behavior-lock for shaping-import's current candidate/flight write surface.
//!
//! This intentionally drives the CLI against real Turso and asserts the exact
//! rows written by the current pre-DAL implementation before that code migrates.

mod common;

use common::{bin, db_exec, db_exec_teardown, is_credless, is_transient, nanos, Guard};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

static SHAPING_IMPORT_LOCK: Mutex<()> = Mutex::new(());

fn run(args: &[&str]) -> Option<(bool, String, String)> {
    for attempt in 0..6 {
        let out = Command::new(bin())
            .args(args)
            .output()
            .expect("spawn travel binary");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !out.status.success() && is_credless(&stderr) {
            return None;
        }
        if !out.status.success() && is_transient(&stderr) && attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(400 * (attempt + 1)));
            continue;
        }
        return Some((out.status.success(), stdout, stderr));
    }
    unreachable!()
}

fn unique_run_id() -> String {
    format!("zztest-shaping-test-{}", nanos())
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn cleanup_run(run_id: &str) {
    let r = sql_lit(run_id);
    for sql in [
        format!(
            "DELETE FROM shaping_candidate_flights WHERE candidate_id IN (SELECT candidate_id FROM shaping_candidates WHERE run_id = {r});"
        ),
        format!("DELETE FROM shaping_candidates WHERE run_id = {r};"),
        format!("DELETE FROM shaping_rules WHERE run_id = {r};"),
        format!("DELETE FROM shaping_scrape_attempts WHERE run_id = {r};"),
        format!("DELETE FROM shaping_research_durations WHERE run_id = {r};"),
        format!("DELETE FROM shaping_research_destinations WHERE run_id = {r};"),
        format!(
            "DELETE FROM shaping_research_artifact_notes WHERE artifact_id IN (SELECT artifact_id FROM shaping_research_artifacts WHERE run_id = {r});"
        ),
        format!("DELETE FROM shaping_research_artifacts WHERE run_id = {r};"),
        format!(
            "DELETE FROM shaping_selected_offer_notes WHERE selection_id IN (SELECT selection_id FROM shaping_selected_offers WHERE run_id = {r});"
        ),
        format!("DELETE FROM shaping_selected_offers WHERE run_id = {r};"),
        format!("DELETE FROM shaping_tour_group_offer_notes WHERE run_id = {r};"),
        format!("DELETE FROM shaping_tour_group_offers WHERE run_id = {r};"),
        format!("DELETE FROM shaping_tour_group_scrape_attempts WHERE run_id = {r};"),
        format!("DELETE FROM shaping_research_runs WHERE run_id = {r};"),
    ] {
        let _ = db_exec_teardown(&sql);
    }
}

fn seed_run(run_id: &str) -> Option<()> {
    let r = sql_lit(run_id);
    db_exec(&format!(
        "INSERT INTO shaping_research_runs \
         (run_id, origin_code, pax, window_start, window_end, currency, exchange_rate_usd_twd, status, created_at, updated_at) \
         VALUES ({r}, 'TPE', 2, '2026-07-06', '2026-07-10', 'TWD', 32.0, 'started', '2026-06-01T00:00:00Z', '2026-06-01T00:00:00Z');"
    ))
    .map(|_| ())
}

fn write_import_file(path: &PathBuf, run_id: &str, candidate_id: &str) {
    let json = format!(
        r#"{{
  "candidates": [
    {{
      "candidateId": "{candidate_id}",
      "runId": "{run_id}",
      "destCode": "KIX",
      "departDate": "2026-07-06",
      "returnDate": "2026-07-10",
      "nights": 4,
      "flightTotalTwd": 23456,
      "verdict": "lock full import row",
      "flights": [
        {{
          "direction": "outbound",
          "airline": "STARLUX",
          "departTime": "2026-07-06T07:30:00+08:00",
          "arriveTime": "2026-07-06T11:10:00+09:00",
          "duration": "2h40m",
          "nonstop": true,
          "priceTotalTwd": 12345
        }},
        {{
          "direction": "inbound",
          "airline": null,
          "departTime": null,
          "arriveTime": null,
          "duration": null,
          "nonstop": false,
          "priceTotalTwd": null
        }}
      ]
    }}
  ],
  "attempts": [
    {{
      "runId": "{run_id}",
      "destCode": "KIX",
      "nights": 4,
      "status": "ok",
      "candidateCount": 1,
      "error": null
    }}
  ]
}}"#
    );
    std::fs::write(path, json).expect("write shaping-import JSON");
}

fn first_row(stdout: &str) -> HashMap<String, String> {
    let line = stdout
        .lines()
        .find(|l| l.contains(": "))
        .unwrap_or_else(|| panic!("expected db exec row, got: {stdout}"));
    line.split(", ")
        .filter_map(|part| {
            let (k, v) = part.split_once(": ")?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

fn rows(stdout: &str) -> Vec<HashMap<String, String>> {
    stdout
        .lines()
        .filter(|l| l.contains(": "))
        .map(|line| {
            line.split(", ")
                .filter_map(|part| {
                    let (k, v) = part.split_once(": ")?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn imports_candidate_and_flights_then_ranks_run() {
    let _lock = SHAPING_IMPORT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let run_id = unique_run_id();
    let candidate_id = format!("{run_id}-KIX-2026-07-06-4n");
    let import_path = std::env::temp_dir().join(format!("{run_id}-import.json"));

    cleanup_run(&run_id);
    let _guard = Guard::new({
        let run_id = run_id.clone();
        let import_path = import_path.clone();
        move || {
            cleanup_run(&run_id);
            let _ = std::fs::remove_file(&import_path);
        }
    });

    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    write_import_file(&import_path, &run_id, &candidate_id);

    let Some((ok, stdout, stderr)) = run(&[
        "shaping-import",
        "--run",
        &run_id,
        "--file",
        import_path.to_str().unwrap(),
    ]) else {
        eprintln!("skipping: no Turso credentials");
        return;
    };
    assert!(
        ok,
        "shaping-import failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&format!(
            "Imported 1 candidates for {run_id} (1 total), ranked"
        )),
        "unexpected shaping-import stdout: {stdout}"
    );

    let candidate = db_exec(&format!(
        "SELECT candidate_id, run_id, dest_code, depart_date, return_date, nights, \
                flight_total_twd, leave_days, rank, verdict, \
                CASE WHEN adopted_plan_id IS NULL THEN 1 ELSE 0 END AS adopted_plan_id_is_null \
         FROM shaping_candidates \
         WHERE run_id = {} AND candidate_id = {}",
        sql_lit(&run_id),
        sql_lit(&candidate_id)
    ))
    .unwrap()
    .raw()
    .to_string();
    let c = first_row(&candidate);
    assert_eq!(c.get("candidate_id"), Some(&candidate_id));
    assert_eq!(c.get("run_id"), Some(&run_id));
    assert_eq!(c.get("dest_code"), Some(&"KIX".to_string()));
    assert_eq!(c.get("depart_date"), Some(&"2026-07-06".to_string()));
    assert_eq!(c.get("return_date"), Some(&"2026-07-10".to_string()));
    assert_eq!(c.get("nights"), Some(&"4".to_string()));
    assert_eq!(c.get("flight_total_twd"), Some(&"23456".to_string()));
    assert_eq!(c.get("leave_days"), Some(&"5".to_string()));
    assert_eq!(c.get("rank"), Some(&"1".to_string()));
    assert_eq!(c.get("verdict"), Some(&"lock full import row".to_string()));
    assert_eq!(c.get("adopted_plan_id_is_null"), Some(&"1".to_string()));

    let flights = db_exec(&format!(
        "SELECT candidate_id, direction, airline, depart_time, arrive_time, duration, nonstop, price_total_twd, \
                CASE WHEN airline IS NULL THEN 1 ELSE 0 END AS airline_is_null, \
                CASE WHEN depart_time IS NULL THEN 1 ELSE 0 END AS depart_time_is_null, \
                CASE WHEN arrive_time IS NULL THEN 1 ELSE 0 END AS arrive_time_is_null, \
                CASE WHEN duration IS NULL THEN 1 ELSE 0 END AS duration_is_null, \
                CASE WHEN price_total_twd IS NULL THEN 1 ELSE 0 END AS price_total_twd_is_null \
         FROM shaping_candidate_flights \
         WHERE candidate_id = {} \
         ORDER BY direction",
        sql_lit(&candidate_id)
    ))
    .unwrap()
    .raw()
    .to_string();
    let flight_rows = rows(&flights);
    assert_eq!(
        flight_rows.len(),
        2,
        "expected 2 flight rows, got: {flights}"
    );
    let inbound = flight_rows
        .iter()
        .find(|r| r.get("direction") == Some(&"inbound".to_string()))
        .expect("inbound flight row");
    let outbound = flight_rows
        .iter()
        .find(|r| r.get("direction") == Some(&"outbound".to_string()))
        .expect("outbound flight row");

    assert_eq!(outbound.get("candidate_id"), Some(&candidate_id));
    assert_eq!(outbound.get("airline"), Some(&"STARLUX".to_string()));
    assert_eq!(
        outbound.get("depart_time"),
        Some(&"2026-07-06T07:30:00+08:00".to_string())
    );
    assert_eq!(
        outbound.get("arrive_time"),
        Some(&"2026-07-06T11:10:00+09:00".to_string())
    );
    assert_eq!(outbound.get("duration"), Some(&"2h40m".to_string()));
    assert_eq!(outbound.get("nonstop"), Some(&"1".to_string()));
    assert_eq!(outbound.get("price_total_twd"), Some(&"12345".to_string()));
    assert_eq!(outbound.get("airline_is_null"), Some(&"0".to_string()));
    assert_eq!(outbound.get("depart_time_is_null"), Some(&"0".to_string()));
    assert_eq!(outbound.get("arrive_time_is_null"), Some(&"0".to_string()));
    assert_eq!(outbound.get("duration_is_null"), Some(&"0".to_string()));
    assert_eq!(
        outbound.get("price_total_twd_is_null"),
        Some(&"0".to_string())
    );

    assert_eq!(inbound.get("candidate_id"), Some(&candidate_id));
    assert_eq!(inbound.get("nonstop"), Some(&"0".to_string()));
    assert_eq!(inbound.get("airline_is_null"), Some(&"1".to_string()));
    assert_eq!(inbound.get("depart_time_is_null"), Some(&"1".to_string()));
    assert_eq!(inbound.get("arrive_time_is_null"), Some(&"1".to_string()));
    assert_eq!(inbound.get("duration_is_null"), Some(&"1".to_string()));
    assert_eq!(
        inbound.get("price_total_twd_is_null"),
        Some(&"1".to_string())
    );

    let run_row = db_exec(&format!(
        "SELECT status FROM shaping_research_runs WHERE run_id = {}",
        sql_lit(&run_id)
    ))
    .unwrap()
    .raw()
    .to_string();
    let run_fields = first_row(&run_row);
    assert_eq!(run_fields.get("status"), Some(&"ranked".to_string()));
}
