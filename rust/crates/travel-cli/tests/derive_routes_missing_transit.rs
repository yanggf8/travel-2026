//! Behavior LOCK for A2: `derive-routes` must REPORT station pairs whose derived
//! leg has no destination_transit metadata (duration_min NULL), pointing the agent
//! at `add-transit`. Without this, a metadata-less leg is silently written and only
//! surfaces later as a SHORT verdict / a blank transit time, with no worklist.

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
fn derive_routes_reports_missing_transit_pairs() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let n = nanos();
    let plan = format!("zztest-missing-transit-{n}");
    let dest = format!("zz_missing_transit_{n}");

    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });
    teardown(&plan, &dest);
    seed_plan(&plan, &dest, 0);

    let d = sql_lit(&dest);
    let p = sql_lit(&plan);
    // Day 1: two activities at DISTINCT stations with NO destination_transit row →
    // the derived leg gets NULL duration_min and must be reported.
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ({d}, 'ZZ Missing Transit', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois (slug, poi_id, title, area, nearest_station, lat, lon, source_url, fetched_at, confidence) \
           VALUES ({d}, 'poi_a', 'A', 'w', 'Nakameguro', 35.64, 139.69, 'test', '2026-07-08', 'test'), \
                  ({d}, 'poi_b', 'B', 'w', 'Daikanyama', 35.65, 139.70, 'test', '2026-07-08', 'test'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-11-01', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO activities (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, nearest_station, updated_at) \
           VALUES ('{n}-a1', {p}, {d}, 'poi_a', 1, 'morning', 0, 'A', 'Nakameguro', '2020-01-01 00:00:00'), \
                  ('{n}-a2', {p}, {d}, 'poi_b', 1, 'afternoon', 0, 'B', 'Daikanyama', '2020-01-01 00:00:00');"
    ))
    .is_none()
    {
        eprintln!("skipping (credless on seed)");
        return;
    }

    let Some(stdout) = run_cmd(&["derive-routes", "--plan-id", &plan, "--dest", &dest, "--day", "1"]) else {
        return;
    };

    // Must name the missing pair AND point at add-transit.
    assert!(
        stdout.contains("Nakameguro") && stdout.contains("Daikanyama"),
        "report should name the missing pair; stdout={stdout}"
    );
    assert!(
        stdout.to_lowercase().contains("missing") && stdout.contains("add-transit"),
        "report should flag missing metadata + point at add-transit; stdout={stdout}"
    );
}
