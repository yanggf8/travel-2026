//! End-to-end LOCK proving the whole point of `add-transit`: a pair added via the
//! CLI (NOT a raw INSERT) is found by the very next `derive-routes` run, even when
//! the activity stations are oddly cased/spaced. If the pair_key normalization ever
//! drifts between add-transit's write and derive-routes' lookup, the derived leg
//! comes back with NULL duration_min and this test fails.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
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
        return None;
    }
    assert!(out.status.success(), "travel {args:?} failed; stdout={stdout} stderr={stderr}");
    Some(stdout)
}

fn teardown(plan: &str, dest: &str) {
    let s = sql_lit(dest);
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_transit WHERE slug = {s}; \
         DELETE FROM destination_pois WHERE slug = {s}; \
         DELETE FROM destination_config WHERE slug = {s};"
    ));
    teardown_plan(plan, dest);
}

#[test]
fn add_transit_pair_is_found_by_derive_routes() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let n = nanos();
    let plan = format!("zztest-add-transit-{n}");
    let dest = format!("zz_transit_test_{n}");

    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });
    teardown(&plan, &dest);
    seed_plan(&plan, &dest, 0);

    let d = sql_lit(&dest);
    let p = sql_lit(&plan);
    // Two activities on day 1; stations are deliberately odd-cased/spaced so the
    // test exercises normalization, NOT an already-normalized key.
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ({d}, 'ZZ Add-Transit Test', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois (slug, poi_id, title, area, nearest_station, lat, lon, source_url, fetched_at, confidence) \
           VALUES ({d}, 'poi_a', 'Gyoen', 'w', 'Shinjuku   Gyoemmae', 35.68, 139.71, 'test', '2026-07-07', 'test'), \
                  ({d}, 'poi_b', 'Tocho', 'w', 'TOCHOMAE', 35.69, 139.69, 'test', '2026-07-07', 'test'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-11-01', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO activities (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, nearest_station, updated_at) \
           VALUES ('{n}-a1', {p}, {d}, 'poi_a', 1, 'morning', 0, 'Gyoen', 'Shinjuku   Gyoemmae', '2020-01-01 00:00:00'), \
                  ('{n}-a2', {p}, {d}, 'poi_b', 1, 'afternoon', 0, 'Tocho', 'TOCHOMAE', '2020-01-01 00:00:00');"
    ))
    .is_none()
    {
        eprintln!("skipping (credless on seed)");
        return;
    }

    // Add the pair via the CLI — no raw destination_transit INSERT.
    if run_cmd(&[
        "add-transit", &dest, "Shinjuku   Gyoemmae", "TOCHOMAE",
        "--minutes", "12", "--line", "Tokyo Metro", "--kind", "metro",
        "--source", "test", "--confidence", "verified",
    ])
    .is_none()
    {
        return;
    }

    // Derive routes for day 1.
    if run_cmd(&["derive-routes", "--plan-id", &plan, "--dest", &dest, "--day", "1"]).is_none() {
        return;
    }

    // The derived leg must have picked up the metadata (duration_min = 12).
    let Some(rows) = db_exec(&format!(
        "SELECT COALESCE(duration_min, -1) AS m FROM day_route_segments \
         WHERE plan_id = {p} AND destination = {d} AND day_number = 1 AND source = 'ai_recommended'"
    )) else {
        return;
    };
    let mins: Vec<String> = rows.column();
    assert!(!mins.is_empty(), "no derived route segment was written");
    assert!(
        mins.iter().any(|m| m == "12"),
        "derived leg did not pick up add-transit's 12 min (pair_key normalization drift?); got {mins:?}"
    );
}
