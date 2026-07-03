//! Behavior lock for `shaping-adopt <candidate> <plan> --create-plan --dest <slug>`.
//!
//! This intentionally drives the real CLI against real Turso and locks the
//! current pre-DAL-migration seed shape performed by `adopt_candidate_to_new_plan`.

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, is_transient, nanos, teardown_plan, Guard};

static CREATE_PLAN_LOCK: Mutex<()> = Mutex::new(());

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

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn unique_tag() -> String {
    format!("zztest{}", nanos())
}

fn rows(stdout: &str) -> Vec<BTreeMap<String, String>> {
    stdout
        .lines()
        .filter(|line| line.contains(": "))
        .map(|line| {
            let mut row = BTreeMap::new();
            for part in line.split(", ") {
                if let Some((col, value)) = part.split_once(": ") {
                    row.insert(col.trim().to_string(), value.trim().to_string());
                }
            }
            row
        })
        .collect()
}

fn one_row(sql: &str) -> BTreeMap<String, String> {
    let stdout = db_exec(sql).expect("Turso credentials available");
    let parsed = rows(stdout.raw());
    assert_eq!(parsed.len(), 1, "expected one row for {sql}, got: {stdout}");
    parsed.into_iter().next().unwrap()
}

fn all_rows(sql: &str) -> Vec<BTreeMap<String, String>> {
    let stdout = db_exec(sql).expect("Turso credentials available");
    rows(stdout.raw())
}

fn count_where(table: &str, where_sql: &str) -> String {
    let row = one_row(&format!(
        "SELECT COUNT(*) AS row_count FROM {table} WHERE {where_sql}"
    ));
    row.get("row_count").cloned().unwrap_or_default()
}

fn teardown(run_id: &str, plan_id: &str, candidate_id: &str, dest_slug: &str) {
    let r = sql_lit(run_id);
    let c = sql_lit(candidate_id);
    let d = sql_lit(dest_slug);

    teardown_plan(plan_id, dest_slug);

    for sql in [
        format!("DELETE FROM shaping_candidate_flights WHERE candidate_id = {c};"),
        format!(
            "DELETE FROM shaping_candidate_flights WHERE candidate_id IN (SELECT candidate_id FROM shaping_candidates WHERE run_id = {r});"
        ),
        format!("DELETE FROM shaping_candidates WHERE candidate_id = {c};"),
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
        format!("DELETE FROM destination_airports WHERE slug = {d};"),
        format!("DELETE FROM destination_config WHERE slug = {d};"),
    ] {
        let _ = db_exec_teardown(&sql);
    }
}

fn seed(run_id: &str, candidate_id: &str, dest_slug: &str, dest_region: &str) {
    let r = sql_lit(run_id);
    let c = sql_lit(candidate_id);
    let d = sql_lit(dest_slug);
    let region = sql_lit(dest_region);

    db_exec(&format!(
        "INSERT INTO destination_config \
         (slug, display_name, ref_id, ref_path, timezone, currency, language, origin, lat, lon) \
         VALUES ({d}, 'ZZTest Osaka/Kyoto', {region}, 'zztest/ref', 'Asia/Tokyo', 'JPY', 'ja', 'taiwan', 34.6937, 135.5023);"
    ))
    .expect("seed destination_config");
    db_exec(&format!(
        "INSERT INTO destination_airports (slug, airport, sort_order) VALUES ({d}, 'KIX', 1);"
    ))
    .expect("seed destination_airports");
    db_exec(&format!(
        "INSERT INTO shaping_research_runs \
         (run_id, origin_code, pax, window_start, window_end, currency, exchange_rate_usd_twd, status, created_at, updated_at) \
         VALUES ({r}, 'TPE', 2, '2026-06-18', '2026-06-25', 'TWD', 32.0, 'ranked', '2026-06-01T00:00:00Z', '2026-06-01T00:00:00Z');"
    ))
    .expect("seed run");
    db_exec(&format!(
        "INSERT INTO shaping_candidates \
         (candidate_id, run_id, dest_code, depart_date, return_date, nights, flight_total_twd, leave_days, rank, verdict, adopted_plan_id) \
         VALUES ({c}, {r}, 'KIX', '2026-06-18', '2026-06-24', 6, 18000, 3, 1, 'keeper', NULL);"
    ))
    .expect("seed candidate");
    db_exec(&format!(
        "INSERT INTO shaping_rules \
         (run_id, aspect, role, kind, value_text, value_date, value_integer, notes, created_at) \
         VALUES \
         ({r}, 'budget', 'soft_preference', 'total_cap', NULL, NULL, 80000, 'lock fixture', '2026-06-01T00:00:00Z'), \
         ({r}, 'date', 'hard_constraint', 'return_no_later_than', NULL, '2026-06-25', NULL, 'lock fixture', '2026-06-01T00:00:00Z');"
    ))
    .expect("seed shaping_rules");
}

#[test]
fn locks_full_shaping_adopt_create_plan_seed_shape() {
    let _lock = CREATE_PLAN_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS one").is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }

    let tag = unique_tag();
    let run_id = format!("{tag}_run");
    let candidate_id = format!("{tag}_cand_kix_20260618_6n");
    let plan_id = format!("{tag}_plan");
    let dest_slug = format!("{tag}_dest");
    let dest_region = format!("{tag}_region");

    teardown(&run_id, &plan_id, &candidate_id, &dest_slug);
    let _guard = Guard::new({
        let (run_id, plan_id, candidate_id, dest_slug) = (
            run_id.clone(),
            plan_id.clone(),
            candidate_id.clone(),
            dest_slug.clone(),
        );
        move || teardown(&run_id, &plan_id, &candidate_id, &dest_slug)
    });

    seed(&run_id, &candidate_id, &dest_slug, &dest_region);

    let Some((ok, stdout, stderr)) = run(&[
        "shaping-adopt",
        &candidate_id,
        &plan_id,
        "--create-plan",
        "--dest",
        &dest_slug,
    ]) else {
        eprintln!("skipping: no Turso credentials");
        return;
    };
    assert!(
        ok,
        "shaping-adopt --create-plan failed\nstdout={stdout}\nstderr={stderr}"
    );

    let plans = one_row(&format!(
        "SELECT schema_version, version FROM plans WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        plans.get("schema_version").map(String::as_str),
        Some("4.2.0")
    );
    assert_eq!(plans.get("version").map(String::as_str), Some("1"));

    let metadata = one_row(&format!(
        "SELECT schema_version, active_destination FROM plan_metadata WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        metadata.get("schema_version").map(String::as_str),
        Some("4.2.0")
    );
    assert_eq!(
        metadata.get("active_destination").map(String::as_str),
        Some(dest_slug.as_str())
    );

    let plan_dest = one_row(&format!(
        "SELECT slug, display_name, status FROM plan_destinations WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        plan_dest.get("slug").map(String::as_str),
        Some(dest_slug.as_str())
    );
    assert_eq!(
        plan_dest.get("display_name").map(String::as_str),
        Some("ZZTest Osaka/Kyoto")
    );
    assert_eq!(plan_dest.get("status").map(String::as_str), Some("active"));

    let details = one_row(&format!(
        "SELECT destination, origin_city, region, primary_airport FROM destination_details WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        details.get("destination").map(String::as_str),
        Some(dest_slug.as_str())
    );
    assert_eq!(details.get("origin_city").map(String::as_str), Some("TPE"));
    assert_eq!(
        details.get("region").map(String::as_str),
        Some(dest_region.as_str())
    );
    assert_eq!(
        details.get("primary_airport").map(String::as_str),
        Some("KIX")
    );

    let city = one_row(&format!(
        "SELECT destination, city_slug, display_name, role, nights FROM destination_cities WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        city.get("destination").map(String::as_str),
        Some(dest_slug.as_str())
    );
    assert_eq!(
        city.get("city_slug").map(String::as_str),
        Some(dest_slug.as_str())
    );
    assert_eq!(
        city.get("display_name").map(String::as_str),
        Some("ZZTest Osaka/Kyoto")
    );
    assert_eq!(city.get("role").map(String::as_str), Some("primary"));
    assert_eq!(city.get("nights").map(String::as_str), Some("6"));

    let anchor = one_row(&format!(
        "SELECT destination, start_date, end_date, days FROM date_anchors WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        anchor.get("destination").map(String::as_str),
        Some(dest_slug.as_str())
    );
    assert_eq!(
        anchor.get("start_date").map(String::as_str),
        Some("2026-06-18")
    );
    assert_eq!(
        anchor.get("end_date").map(String::as_str),
        Some("2026-06-24")
    );
    assert_eq!(anchor.get("days").map(String::as_str), Some("7"));

    let statuses = all_rows(&format!(
        "SELECT process_id, status FROM process_statuses WHERE plan_id = {} AND destination = {} ORDER BY process_id",
        sql_lit(&plan_id),
        sql_lit(&dest_slug)
    ));
    let status_pairs: BTreeMap<String, String> = statuses
        .iter()
        .map(|row| {
            (
                row.get("process_id").cloned().unwrap_or_default(),
                row.get("status").cloned().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(status_pairs.len(), 6);
    assert_eq!(
        status_pairs
            .get("process_1_date_anchor")
            .map(String::as_str),
        Some("confirmed")
    );
    assert_eq!(
        status_pairs
            .get("process_2_destination")
            .map(String::as_str),
        Some("confirmed")
    );
    assert_eq!(
        status_pairs
            .get("process_3_transportation")
            .map(String::as_str),
        Some("pending")
    );
    assert_eq!(
        status_pairs.get("process_3_4_packages").map(String::as_str),
        Some("pending")
    );
    assert_eq!(
        status_pairs
            .get("process_4_accommodation")
            .map(String::as_str),
        Some("pending")
    );
    assert_eq!(
        status_pairs
            .get("process_5_daily_itinerary")
            .map(String::as_str),
        Some("pending")
    );

    let state = one_row(&format!(
        "SELECT project, version, current_focus, active_destination FROM event_log_state WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        state.get("project").map(String::as_str),
        Some("japan-travel")
    );
    assert_eq!(state.get("version").map(String::as_str), Some("3.0"));
    assert_eq!(state.get("current_focus").map(String::as_str), Some(""));
    assert_eq!(
        state.get("active_destination").map(String::as_str),
        Some(dest_slug.as_str())
    );

    let log_dest = one_row(&format!(
        "SELECT destination, status FROM event_log_destinations WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        log_dest.get("destination").map(String::as_str),
        Some(dest_slug.as_str())
    );
    assert_eq!(log_dest.get("status").map(String::as_str), Some("active"));

    assert_eq!(
        count_where("plan_events", &format!("plan_id = {}", sql_lit(&plan_id))),
        "1"
    );
    assert_eq!(
        count_where(
            "plan_events",
            &format!(
                "plan_id = {} AND scope = 'timeline' AND destination = '' AND process_id = '' AND sort_order = 0 AND event = 'shaping_candidate_adopted'",
                sql_lit(&plan_id)
            )
        ),
        "1"
    );

    let event_data = all_rows(&format!(
        "SELECT key, value FROM plan_event_data WHERE plan_id = {} AND scope = 'timeline' AND destination = '' AND process_id = '' AND sort_order = 0 ORDER BY rowid",
        sql_lit(&plan_id)
    ));
    let event_keys: Vec<&str> = event_data
        .iter()
        .map(|row| row.get("key").map(String::as_str).unwrap_or_default())
        .collect();
    assert_eq!(
        event_keys,
        vec![
            "candidate_id",
            "run_id",
            "depart_date",
            "return_date",
            "dest_code",
            "shaping_count",
            "shaping_summary",
        ]
    );
    let event_kv: BTreeMap<String, String> = event_data
        .iter()
        .map(|row| {
            (
                row.get("key").cloned().unwrap_or_default(),
                row.get("value").cloned().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(event_kv.len(), 7);
    assert_eq!(
        event_kv.get("candidate_id").map(String::as_str),
        Some(candidate_id.as_str())
    );
    assert_eq!(
        event_kv.get("run_id").map(String::as_str),
        Some(run_id.as_str())
    );
    assert_eq!(
        event_kv.get("depart_date").map(String::as_str),
        Some("2026-06-18")
    );
    assert_eq!(
        event_kv.get("return_date").map(String::as_str),
        Some("2026-06-24")
    );
    assert_eq!(event_kv.get("dest_code").map(String::as_str), Some("KIX"));
    assert_eq!(event_kv.get("shaping_count").map(String::as_str), Some("2"));
    assert_eq!(
        event_kv.get("shaping_summary").map(String::as_str),
        Some(
            "budget/soft_preference/total_cap=80000; date/hard_constraint/return_no_later_than=2026-06-25"
        )
    );

    let operation = one_row(&format!(
        "SELECT command_type, command_summary, status, version_before, version_after FROM operation_runs WHERE plan_id = {}",
        sql_lit(&plan_id)
    ));
    assert_eq!(
        operation.get("command_type").map(String::as_str),
        Some("shaping-adopt")
    );
    assert_eq!(
        operation.get("command_summary").map(String::as_str),
        Some(candidate_id.as_str())
    );
    assert_eq!(
        operation.get("status").map(String::as_str),
        Some("completed")
    );
    assert_eq!(
        operation.get("version_before").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        operation.get("version_after").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        count_where(
            "operation_runs",
            &format!("plan_id = {}", sql_lit(&plan_id))
        ),
        "1"
    );

    let candidate = one_row(&format!(
        "SELECT adopted_plan_id FROM shaping_candidates WHERE candidate_id = {}",
        sql_lit(&candidate_id)
    ));
    assert_eq!(
        candidate.get("adopted_plan_id").map(String::as_str),
        Some(plan_id.as_str())
    );

    let run = one_row(&format!(
        "SELECT status FROM shaping_research_runs WHERE run_id = {}",
        sql_lit(&run_id)
    ));
    assert_eq!(run.get("status").map(String::as_str), Some("adopted"));
}
