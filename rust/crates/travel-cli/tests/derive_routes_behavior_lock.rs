//! Real-Turso behavior LOCK for `derive-routes` — derives ai_recommended transit
//! route segments from consecutive activity stations. Asserts confirmed-skip,
//! stale-ai replace, same-station skip, idempotent re-run, and audit surface.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn run_cmd(args: &[&str]) -> Option<String> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_credless(&stderr) {
        eprintln!("skipping derive-routes lock mid-test: {}", stderr.trim());
        return None;
    }
    assert!(
        out.status.success(),
        "travel {args:?} should succeed; stdout={stdout} stderr={stderr}"
    );
    Some(stdout)
}

fn teardown_dest(slug: &str) {
    let s = sql_lit(slug);
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_transit WHERE slug = {s}; \
         DELETE FROM destination_pois WHERE slug = {s}; \
         DELETE FROM destination_config WHERE slug = {s};"
    ));
}

fn teardown(plan: &str, dest: &str) {
    teardown_dest(dest);
    teardown_plan(plan, dest);
}

fn row_sql(plan: &str, dest: &str) -> String {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    format!(
        "SELECT day_number || '|' || sort_order || '|' || from_place || '|' || to_place || '|' || mode || '|' || source \
         || '|' || COALESCE(duration_min, -1) || '|' || COALESCE(notes, '') \
         FROM day_route_segments WHERE plan_id = {p} AND destination = {d} \
         ORDER BY day_number, sort_order"
    )
}

fn expected_rows() -> Vec<&'static str> {
    vec![
        "1|0|shinjuku|shibuya|transit|ai_recommended|10|JR Yamanote",
        "2|0|Existing From|Existing To|walking|confirmed|-1|",
        "3|0|asakusa|shibuya|transit|ai_recommended|30|Ginza Line",
    ]
}

fn assert_rows(plan: &str, dest: &str) {
    let rows = exec_ok(&row_sql(plan, dest)).column();
    assert_eq!(rows, expected_rows(), "day_route_segments mismatch");
}

fn assert_audit(plan: &str, op_count: i64, version: i64) {
    let p = sql_lit(plan);
    let ops = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p} AND command_type = 'derive-routes'"
    ));
    assert_eq!(
        ops.scalar().as_deref(),
        Some(op_count.to_string().as_str()),
        "derive-routes operation_runs count"
    );
    let ver = exec_ok(&format!("SELECT version AS v FROM plans WHERE plan_id = {p}"));
    assert_eq!(
        ver.scalar().as_deref(),
        Some(version.to_string().as_str()),
        "plans.version"
    );
    if op_count > 0 {
        let row = exec_ok(&format!(
            "SELECT version_before || '>' || COALESCE(version_after, -1) || '|' || status AS v \
             FROM operation_runs WHERE plan_id = {p} AND command_type = 'derive-routes' \
             ORDER BY version_after DESC LIMIT 1"
        ));
        assert_eq!(
            row.scalar().as_deref(),
            Some(format!("{}>{version}|completed", version - 1).as_str()),
            "latest derive-routes operation row"
        );
        let events = exec_ok(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = {p} AND event = 'route_segments_bulk_updated'"
        ));
        assert_eq!(
            events.scalar().as_deref(),
            Some("2"),
            "route_segments_bulk_updated event count (both scopes)"
        );
    }
}

#[test]
fn derive_routes_write_surface_is_locked() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping derive-routes lock (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest-derive-{tag}");
    let dest = format!("zzderive_{tag}");

    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });
    teardown(&plan, &dest);
    seed_plan(&plan, &dest, 0);

    let p = sql_lit(&plan);
    let d = sql_lit(&dest);

    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ({d}, 'ZZ Derive Routes Test', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois (slug, poi_id, title, area, nearest_station, lat, lon, source_url, fetched_at, confidence) \
           VALUES ({d}, 'poi_shinjuku', 'Shinjuku', 'west', 'shinjuku', 35.69, 139.70, 'test', '2026-07-06', 'test'), \
                  ({d}, 'poi_shibuya', 'Shibuya', 'west', 'shibuya', 35.66, 139.70, 'test', '2026-07-06', 'test'), \
                  ({d}, 'poi_asakusa', 'Asakusa', 'east', 'asakusa', 35.71, 139.80, 'test', '2026-07-06', 'test'); \
         INSERT INTO destination_transit (slug, pair_key, kind, minutes, line, station_from, station_to, source_url, fetched_at, confidence) \
           VALUES ({d}, 'shinjuku_to_shibuya', 'rail', 10, 'JR Yamanote', 'shinjuku', 'shibuya', 'test', '2026-07-06', 'test'), \
                  ({d}, 'asakusa_to_shibuya', 'metro', 30, 'Ginza Line', 'asakusa', 'shibuya', 'test', '2026-07-06', 'test'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-10-01', 'full', 'draft', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 2, '2026-10-02', 'full', 'draft', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 3, '2026-10-03', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO activities (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, updated_at) \
           VALUES ('{tag}-d1-m', {p}, {d}, 'poi_shinjuku', 1, 'morning', 0, 'Shinjuku', '2020-01-01 00:00:00'), \
                  ('{tag}-d1-a', {p}, {d}, 'poi_shibuya', 1, 'afternoon', 0, 'Shibuya PM', '2020-01-01 00:00:00'), \
                  ('{tag}-d1-e', {p}, {d}, 'poi_shibuya', 1, 'evening', 0, 'Shibuya Eve', '2020-01-01 00:00:00'), \
                  ('{tag}-d2-m', {p}, {d}, 'poi_asakusa', 2, 'morning', 0, 'Asakusa', '2020-01-01 00:00:00'), \
                  ('{tag}-d2-a', {p}, {d}, 'poi_shibuya', 2, 'afternoon', 0, 'Shibuya D2', '2020-01-01 00:00:00'), \
                  ('{tag}-d3-m', {p}, {d}, 'poi_asakusa', 3, 'morning', 0, 'Asakusa D3', '2020-01-01 00:00:00'), \
                  ('{tag}-d3-a', {p}, {d}, 'poi_shibuya', 3, 'afternoon', 0, 'Shibuya D3', '2020-01-01 00:00:00'); \
         INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
           VALUES ({p}, {d}, 2, 0, 'Existing From', 'Existing To', 'walking', 'confirmed'), \
                  ({p}, {d}, 3, 0, 'Old From', 'Old To', 'walking', 'ai_recommended');"
    ))
    .is_none()
    {
        eprintln!("skipping derive-routes lock mid-test (credless on seed)");
        return;
    }

    let cmd_args = [
        "derive-routes",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ];

    let stdout = match run_cmd(&cmd_args) {
        Some(s) => s,
        None => return,
    };

    assert!(stdout.contains("Day 1: inserted 1 ai_recommended route segment(s)"));
    assert!(stdout.contains("Day 2: skipped - confirmed route segment(s) exist"));
    assert!(
        stdout.contains("Day 3: replaced 1 stale ai_recommended route segment(s) with 1 derived segment(s)")
    );
    assert!(stdout.contains(
        "Totals: days_scanned=3 days_written=2 inserted=2 deleted=1 skipped_confirmed=1"
    ));
    assert!(stdout.contains("✅"));

    assert_rows(&plan, &dest);

    let bad_same = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM day_route_segments WHERE plan_id = {p} AND destination = {d} \
         AND from_place = 'shibuya' AND to_place = 'shibuya'"
    ));
    assert_eq!(bad_same.scalar().as_deref(), Some("0"), "same-station leg must be skipped");

    let bad_stale = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM day_route_segments WHERE plan_id = {p} AND destination = {d} \
         AND from_place = 'Old From'"
    ));
    assert_eq!(bad_stale.scalar().as_deref(), Some("0"), "stale AI row must be replaced");

    assert_audit(&plan, 1, 1);

    let stdout2 = match run_cmd(&cmd_args) {
        Some(s) => s,
        None => return,
    };

    assert!(stdout2.contains(
        "Day 1: unchanged - existing ai_recommended route segment(s) already match"
    ));
    assert!(stdout2.contains("Day 2: skipped - confirmed route segment(s) exist"));
    assert!(stdout2.contains(
        "Day 3: unchanged - existing ai_recommended route segment(s) already match"
    ));
    assert!(stdout2.contains(
        "Totals: days_scanned=3 days_written=0 inserted=0 deleted=0 skipped_confirmed=1"
    ));

    assert_rows(&plan, &dest);
    assert_audit(&plan, 1, 1);
}